//! Lake carving in the Java decoration order, including scheduled air ticks.
use crate::{
    disk_feature::{BlockPredicate, DiskWorld, Provider},
    packs::ResourceId,
};
use anyhow::{Context, Result, ensure};
use native_core::random::DecorationRandom;
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub trait LakeWorld: DiskWorld {
    fn min_y(&self) -> i32;
    fn set_lake_block(&mut self, pos: [i32; 3], state: &Self::State) -> Result<bool>;
    fn schedule_lake_air_tick(&mut self, pos: [i32; 3]) -> Result<()>;
    fn should_freeze_lake(&mut self, pos: [i32; 3]) -> Result<bool>;
}

pub struct LakeFeature<S> {
    fluid: Provider<S>,
    barrier: Provider<S>,
    air: S,
    ice: S,
    can_place: BlockPredicate,
    replace: BlockPredicate,
    replace_barrier: BlockPredicate,
}
impl<S: Clone + Eq> LakeFeature<S> {
    pub fn decode(value: &Value, resolve: &mut impl FnMut(&Value) -> Result<S>) -> Result<Self> {
        let id = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("missing lake type")?,
        )?;
        ensure!(
            id.namespace == "minecraft" && id.path == "lake",
            "wrong lake feature type"
        );
        let pred = |name: &str| -> Result<BlockPredicate> {
            BlockPredicate::decode(
                value
                    .get(name)
                    .with_context(|| format!("missing lake {name}"))?,
            )
        };
        Ok(Self {
            fluid: Provider::decode(
                value.get("fluid").context("missing lake fluid")?,
                resolve,
                0,
            )?,
            barrier: Provider::decode(
                value.get("barrier").context("missing lake barrier")?,
                resolve,
                0,
            )?,
            air: resolve(&json!({"id":"minecraft:cave_air"}))?,
            ice: resolve(&json!({"id":"minecraft:ice"}))?,
            can_place: pred("can_place_feature")?,
            replace: pred("can_replace_with_air_or_fluid")?,
            replace_barrier: pred("can_replace_with_barrier")?,
        })
    }
    pub fn required_tags(&self) -> BTreeSet<ResourceId> {
        let mut v = self.can_place.required_tags();
        v.extend(self.replace.required_tags());
        v.extend(self.replace_barrier.required_tags());
        self.fluid.tags(&mut v);
        self.barrier.tags(&mut v);
        v
    }
    pub fn required_blocks(&self) -> BTreeSet<ResourceId> {
        let mut v = self.can_place.required_blocks();
        v.extend(self.replace.required_blocks());
        v.extend(self.replace_barrier.required_blocks());
        self.fluid.blocks(&mut v);
        self.barrier.blocks(&mut v);
        v
    }
    /// States which the configured fluid provider can select. Integer
    /// properties may still vary when the provider is evaluated.
    pub fn possible_fluid_states(&self) -> Vec<&S> {
        let mut states = Vec::new();
        self.fluid.states(&mut states);
        states
    }
    pub fn needs_survival(&self, name: &str) -> bool {
        self.can_place.needs_survival(name)
            || self.replace.needs_survival(name)
            || self.replace_barrier.needs_survival(name)
            || self.fluid.needs_survival(name)
            || self.barrier.needs_survival(name)
    }
    fn mark_above<W: LakeWorld<State = S>>(world: &mut W, pos: [i32; 3]) -> Result<()> {
        let mut p = pos;
        for _ in 0..2 {
            p[1] = p[1].wrapping_add(1);
            let state = world.block(p)?;
            if world.is_air(&state)? {
                break;
            }
            world.mark_postprocessing(p)?;
        }
        Ok(())
    }
    pub fn place<W: LakeWorld<State = S>>(
        &self,
        world: &mut W,
        random: &mut DecorationRandom,
        origin: [i32; 3],
    ) -> Result<bool> {
        if origin[1] <= world.min_y().wrapping_add(4) {
            return Ok(false);
        }
        let origin = [
            origin[0].wrapping_sub(8),
            origin[1].wrapping_sub(4),
            origin[2].wrapping_sub(8),
        ];
        let index = |x: usize, z: usize, y: usize| (x * 16 + z) * 8 + y;
        let at = |x: usize, z: usize, y: usize| {
            [
                origin[0].wrapping_add(x as i32),
                origin[1].wrapping_add(y as i32),
                origin[2].wrapping_add(z as i32),
            ]
        };
        let mut mask = [false; 2048];
        let blobs = random.next_int_bound(4).expect("positive bound") + 4;
        for _ in 0..blobs {
            let sx = random.next_double() * 6.0 + 3.0;
            let sy = random.next_double() * 4.0 + 2.0;
            let sz = random.next_double() * 6.0 + 3.0;
            let cx = random.next_double() * (16.0 - sx - 2.0) + 1.0 + sx / 2.0;
            let cy = random.next_double() * (8.0 - sy - 4.0) + 2.0 + sy / 2.0;
            let cz = random.next_double() * (16.0 - sz - 2.0) + 1.0 + sz / 2.0;
            for x in 1..15 {
                for z in 1..15 {
                    for y in 1..7 {
                        let dx = (x as f64 - cx) / (sx / 2.0);
                        let dy = (y as f64 - cy) / (sy / 2.0);
                        let dz = (z as f64 - cz) / (sz / 2.0);
                        if dx * dx + dy * dy + dz * dz < 1.0 {
                            mask[index(x, z, y)] = true;
                        }
                    }
                }
            }
        }
        let fluid = self
            .fluid
            .state(world, random, origin)?
            .context("lake fluid provider returned no state")?;
        let boundary = |x: usize, z: usize, y: usize| -> bool {
            !mask[index(x, z, y)]
                && ((x < 15 && mask[index(x + 1, z, y)])
                    || (x > 0 && mask[index(x - 1, z, y)])
                    || (z < 15 && mask[index(x, z + 1, y)])
                    || (z > 0 && mask[index(x, z - 1, y)])
                    || (y < 7 && mask[index(x, z, y + 1)])
                    || (y > 0 && mask[index(x, z, y - 1)]))
        };
        for x in 0..16 {
            for z in 0..16 {
                for y in 0..8 {
                    if boundary(x, z, y) {
                        let p = at(x, z, y);
                        let old = world.block(p)?;
                        if (y >= 4 && world.is_liquid(&old)?)
                            || (y < 4 && !world.is_solid(&old)? && old != fluid)
                            || !self.can_place.test(world, p)?
                        {
                            return Ok(false);
                        }
                    }
                }
            }
        }
        for x in 0..16 {
            for z in 0..16 {
                for y in 0..8 {
                    if mask[index(x, z, y)] {
                        let p = at(x, z, y);
                        if !self.replace.test(world, p)? {
                            continue;
                        }
                        let upper = y >= 4;
                        world.set_lake_block(p, if upper { &self.air } else { &fluid })?;
                        if upper {
                            world.schedule_lake_air_tick(p)?;
                            Self::mark_above(world, p)?;
                        }
                    }
                }
            }
        }
        let barrier = self
            .barrier
            .state(world, random, origin)?
            .context("lake barrier provider returned no state")?;
        if !world.is_air(&barrier)? {
            for x in 0..16 {
                for z in 0..16 {
                    for y in 0..8 {
                        if boundary(x, z, y)
                            && (y < 4 || random.next_int_bound(2).expect("positive bound") != 0)
                        {
                            let p = at(x, z, y);
                            let old = world.block(p)?;
                            if world.is_solid(&old)? && self.replace_barrier.test(world, p)? {
                                world.set_lake_block(p, &barrier)?;
                                Self::mark_above(world, p)?;
                            }
                        }
                    }
                }
            }
        }
        if world.fluid_in_tag(&fluid, &ResourceId::parse("minecraft:water")?)? {
            for x in 0..16 {
                for z in 0..16 {
                    let p = at(x, z, 4);
                    if world.should_freeze_lake(p)? && self.replace.test(world, p)? {
                        world.set_lake_block(p, &self.ice)?;
                    }
                }
            }
        }
        Ok(true)
    }
}
