//! Java spring placement using the active block registry and fluid tick queue.
use std::collections::BTreeSet;

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};

use crate::{ore_feature::OreWorld, packs::ResourceId};

pub trait SpringWorld: OreWorld {
    /// Apply WorldGenLevel.setBlock effects, or reject unsupported effects.
    fn set_spring_block(&mut self, pos: [i32; 3], state: &Self::State) -> Result<bool>;
    fn schedule_spring_tick(&mut self, pos: [i32; 3], fluid: &ResourceId) -> Result<()>;
}

pub struct SpringFeature<S> {
    state: S,
    fluid: ResourceId,
    requires_below: bool,
    rock_count: i32,
    hole_count: i32,
    blocks: Vec<ResourceId>,
    tag: Option<ResourceId>,
}

impl<S: Clone + Eq> SpringFeature<S> {
    pub fn decode(value: &Value, resolve: &mut impl FnMut(&Value) -> Result<S>) -> Result<Self> {
        let kind = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("missing spring feature type")?,
        )?;
        ensure!(
            kind.namespace == "minecraft" && kind.path == "spring_feature",
            "wrong spring feature type"
        );
        let fluid = value.get("state").context("missing spring fluid state")?;
        let id = ResourceId::parse(
            fluid
                .get("id")
                .and_then(Value::as_str)
                .context("missing spring fluid id")?,
        )?;
        ensure!(
            id.namespace == "minecraft",
            "unsupported spring fluid namespace"
        );
        let (block, source) = match id.path.as_str() {
            "water" => ("minecraft:water", true),
            "lava" => ("minecraft:lava", true),
            "flowing_water" => ("minecraft:water", false),
            "flowing_lava" => ("minecraft:lava", false),
            _ => bail!("unsupported spring fluid"),
        };
        let props = match fluid.get("properties") {
            Some(v) => Some(v.as_object().context("invalid spring fluid properties")?),
            None => None,
        };
        if let Some(props) = props {
            for key in props.keys() {
                ensure!(
                    key == "falling" || (!source && key == "level"),
                    "unsupported spring fluid property"
                );
            }
        }
        let falling = match props.and_then(|p| p.get("falling")) {
            None => true,
            Some(v) => match v.as_str() {
                Some("true") => true,
                Some("false") => false,
                _ => bail!("invalid falling property"),
            },
        };
        let level = if source {
            0
        } else {
            let amount: i32 = match props.and_then(|p| p.get("level")) {
                None => 1,
                Some(level) => level
                    .as_str()
                    .context("invalid flowing spring level")?
                    .parse()?,
            };
            ensure!((1..=8).contains(&amount), "invalid flowing spring level");
            8 - amount + if falling { 8 } else { 0 }
        };
        let state = resolve(&json!({"id":block,"properties":{"level":level.to_string()}}))?;
        let valid = value
            .get("valid_blocks")
            .context("missing spring valid blocks")?;
        let (blocks, tag) = match valid {
            Value::String(s) if s.starts_with('#') => {
                (Vec::new(), Some(ResourceId::parse(&s[1..])?))
            }
            Value::String(s) => (vec![ResourceId::parse(s)?], None),
            Value::Array(values) => {
                ensure!(values.len() <= 4096, "too many spring blocks");
                (
                    values
                        .iter()
                        .map(|v| ResourceId::parse(v.as_str().context("invalid spring block")?))
                        .collect::<Result<Vec<_>>>()?,
                    None,
                )
            }
            _ => bail!("invalid spring valid blocks"),
        };
        let number = |key: &str, default: i32| -> Result<i32> {
            value
                .get(key)
                .map(|v| {
                    i32::try_from(v.as_i64().context("invalid spring count")?).map_err(Into::into)
                })
                .unwrap_or(Ok(default))
        };
        Ok(Self {
            state,
            fluid: id,
            requires_below: value
                .get("requires_block_below")
                .map(|v| v.as_bool().context("invalid spring requires_block_below"))
                .transpose()?
                .unwrap_or(true),
            rock_count: number("rock_count", 4)?,
            hole_count: number("hole_count", 1)?,
            blocks,
            tag,
        })
    }
    pub fn required_tags(&self) -> BTreeSet<ResourceId> {
        self.tag.iter().cloned().collect()
    }
    pub fn required_blocks(&self) -> BTreeSet<ResourceId> {
        self.blocks.iter().cloned().collect()
    }
    fn valid<W: SpringWorld<State = S>>(&self, world: &mut W, pos: [i32; 3]) -> Result<bool> {
        let state = world.block(pos)?;
        if let Some(tag) = &self.tag {
            return world.in_tag(&state, tag);
        }
        for block in &self.blocks {
            if world.block_is(&state, block)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub fn place<W: SpringWorld<State = S>>(&self, world: &mut W, pos: [i32; 3]) -> Result<bool> {
        let at = |dx: i32, dy: i32, dz: i32| {
            [
                pos[0].wrapping_add(dx),
                pos[1].wrapping_add(dy),
                pos[2].wrapping_add(dz),
            ]
        };
        if !self.valid(world, at(0, 1, 0))?
            || (self.requires_below && !self.valid(world, at(0, -1, 0))?)
        {
            return Ok(false);
        }
        let current = world.block(pos)?;
        if !world.is_air(&current)? && !self.valid(world, pos)? {
            return Ok(false);
        }
        let neighbors = [
            at(-1, 0, 0),
            at(1, 0, 0),
            at(0, 0, -1),
            at(0, 0, 1),
            at(0, -1, 0),
        ];
        let mut rocks = 0;
        for p in neighbors {
            if self.valid(world, p)? {
                rocks += 1;
            }
        }
        let mut holes = 0;
        for p in neighbors {
            let state = world.block(p)?;
            if world.is_air(&state)? {
                holes += 1;
            }
        }
        if rocks != self.rock_count || holes != self.hole_count {
            return Ok(false);
        }
        world.set_spring_block(pos, &self.state)?;
        world.schedule_spring_tick(pos, &self.fluid)?;
        Ok(true)
    }
}
