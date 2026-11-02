//! Underwater magma placement, with Java's column scan and ordered random draws.
use crate::{ore_feature::OreWorld, packs::ResourceId};
use anyhow::{Context, Result, ensure};
use native_core::random::DecorationRandom;
use serde_json::{Value, json};

/// Occlusion must test the specified face, not merely block solidity.
pub trait MagmaWorld: OreWorld {
    fn face_is_full(&mut self, state: &Self::State, direction: [i32; 3]) -> Result<bool>;
    fn set_magma(&mut self, pos: [i32; 3], state: &Self::State) -> Result<bool>;
}

/// Known vanilla states with the same occlusion result on every face.
pub fn vanilla_full_occlusion(name: &str) -> Option<bool> {
    match name {
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" | "minecraft:water"
        | "minecraft:lava" => Some(false),
        "minecraft:stone"
        | "minecraft:deepslate"
        | "minecraft:netherrack"
        | "minecraft:basalt"
        | "minecraft:blackstone"
        | "minecraft:ancient_debris"
        | "minecraft:dirt"
        | "minecraft:grass_block"
        | "minecraft:mud"
        | "minecraft:clay"
        | "minecraft:sand"
        | "minecraft:sandstone"
        | "minecraft:gravel"
        | "minecraft:snow_block"
        | "minecraft:podzol"
        | "minecraft:coarse_dirt"
        | "minecraft:mycelium"
        | "minecraft:tuff"
        | "minecraft:calcite"
        | "minecraft:granite"
        | "minecraft:diorite"
        | "minecraft:andesite"
        | "minecraft:bedrock"
        | "minecraft:red_sand"
        | "minecraft:moss_block"
        | "minecraft:magma_block" => Some(true),
        _ => None,
    }
}

pub struct UnderwaterMagma<S> {
    floor_range: i32,
    radius: i32,
    probability: f32,
    state: S,
    water: ResourceId,
}
impl<S: Clone + Eq> UnderwaterMagma<S> {
    pub fn decode(value: &Value, resolve: &mut impl FnMut(&Value) -> Result<S>) -> Result<Self> {
        let kind = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("missing magma feature type")?,
        )?;
        ensure!(
            kind.namespace == "minecraft" && kind.path == "underwater_magma",
            "wrong magma feature type"
        );
        let int = |key: &str| -> Result<i32> {
            Ok(i32::try_from(
                value
                    .get(key)
                    .and_then(Value::as_i64)
                    .context("invalid magma integer")?,
            )?)
        };
        let floor_range = int("floor_search_range")?;
        let radius = int("placement_radius_around_floor")?;
        let probability = value
            .get("placement_probability_per_valid_position")
            .and_then(Value::as_f64)
            .context("invalid magma probability")? as f32;
        ensure!(
            (0..=512).contains(&floor_range)
                && (0..=64).contains(&radius)
                && probability.is_finite()
                && (0.0..=1.0).contains(&probability),
            "invalid magma feature configuration"
        );
        Ok(Self {
            floor_range,
            radius,
            probability,
            state: resolve(&json!({"id":"minecraft:magma_block"}))?,
            water: ResourceId::parse("minecraft:water")?,
        })
    }
    fn is_water<W: MagmaWorld<State = S>>(&self, world: &mut W, pos: [i32; 3]) -> Result<bool> {
        let state = world.block(pos)?;
        world.block_is(&state, &self.water)
    }
    fn scan<W: MagmaWorld<State = S>>(
        &self,
        world: &mut W,
        pos: [i32; 3],
        direction: i32,
    ) -> Result<Option<i32>> {
        let mut p = pos;
        for _ in 1..self.floor_range {
            if !self.is_water(world, p)? {
                break;
            }
            p[1] = p[1].wrapping_add(direction);
        }
        Ok((!self.is_water(world, p)?).then_some(p[1]))
    }
    fn valid<W: MagmaWorld<State = S>>(&self, world: &mut W, p: [i32; 3]) -> Result<bool> {
        let state = world.block(p)?;
        if world.is_air(&state)? || world.block_is(&state, &self.water)? {
            return Ok(false);
        }
        for (delta, face) in [
            ([0, -1, 0], [0, 1, 0]),
            ([0, 0, -1], [0, 0, 1]),
            ([1, 0, 0], [-1, 0, 0]),
            ([0, 0, 1], [0, 0, -1]),
            ([-1, 0, 0], [1, 0, 0]),
        ] {
            let q = [
                p[0].wrapping_add(delta[0]),
                p[1].wrapping_add(delta[1]),
                p[2].wrapping_add(delta[2]),
            ];
            let neighbor = world.block(q)?;
            if !world.face_is_full(&neighbor, face)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
    pub fn place<W: MagmaWorld<State = S>>(
        &self,
        world: &mut W,
        random: &mut DecorationRandom,
        pos: [i32; 3],
    ) -> Result<bool> {
        if !self.is_water(world, pos)? {
            return Ok(false);
        }
        self.scan(world, pos, 1)?;
        let Some(floor) = self.scan(world, pos, -1)? else {
            return Ok(false);
        };
        let lo = [
            pos[0]
                .checked_sub(self.radius)
                .context("magma coordinate overflow")?,
            floor
                .checked_sub(self.radius)
                .context("magma coordinate overflow")?,
            pos[2]
                .checked_sub(self.radius)
                .context("magma coordinate overflow")?,
        ];
        let hi = [
            pos[0]
                .checked_add(self.radius)
                .context("magma coordinate overflow")?,
            floor
                .checked_add(self.radius)
                .context("magma coordinate overflow")?,
            pos[2]
                .checked_add(self.radius)
                .context("magma coordinate overflow")?,
        ];
        let mut placed = false;
        for z in lo[2]..=hi[2] {
            for y in lo[1]..=hi[1] {
                for x in lo[0]..=hi[0] {
                    let p = [x, y, z];
                    if random.next_float() < self.probability && self.valid(world, p)? {
                        world.set_magma(p, &self.state)?;
                        placed = true;
                    }
                }
            }
        }
        Ok(placed)
    }
}
