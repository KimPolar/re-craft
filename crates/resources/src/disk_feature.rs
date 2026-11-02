//! 26.3 disk features, including their ordered postprocessing marks.
use std::collections::BTreeSet;

use anyhow::{Context, Result, bail, ensure};
use native_core::random::DecorationRandom;
use serde_json::Value;

use crate::{
    ore_feature::OreWorld,
    packs::{ResourceId, ResourceProvider},
    placed_feature::IntProvider,
};

/// The implementation must use the active world's block and fluid registry.
pub trait DiskWorld: OreWorld {
    fn is_solid(&mut self, state: &Self::State) -> Result<bool>;
    fn fluid_is(&mut self, state: &Self::State, id: &ResourceId) -> Result<bool>;
    fn set_disk_block(&mut self, pos: [i32; 3], state: &Self::State) -> Result<bool>;
    fn mark_postprocessing(&mut self, pos: [i32; 3]) -> Result<()>;
}

#[derive(Clone, Debug)]
enum Predicate {
    True,
    Blocks([i32; 3], Vec<ResourceId>),
    Tag([i32; 3], ResourceId),
    Fluids([i32; 3], Vec<ResourceId>),
    Solid([i32; 3]),
    Not(Box<Predicate>),
    Any(Vec<Predicate>),
    All(Vec<Predicate>),
}

/// A data-pack block predicate shared by features and placement filters.
#[derive(Clone, Debug)]
pub struct BlockPredicate(Predicate);
impl BlockPredicate {
    pub fn decode(value: &Value) -> Result<Self> {
        Ok(Self(Predicate::decode(value, 0)?))
    }
    pub fn test<W: DiskWorld>(&self, world: &mut W, pos: [i32; 3]) -> Result<bool> {
        self.0.test(world, pos)
    }
    pub fn required_tags(&self) -> BTreeSet<ResourceId> {
        let mut tags = BTreeSet::new();
        self.0.tags(&mut tags);
        tags
    }
    pub fn required_blocks(&self) -> BTreeSet<ResourceId> {
        let mut blocks = BTreeSet::new();
        self.0.blocks(&mut blocks);
        blocks
    }
}

fn offset(value: &Value) -> Result<[i32; 3]> {
    let Some(value) = value.get("offset") else {
        return Ok([0; 3]);
    };
    let values = value
        .as_array()
        .filter(|v| v.len() == 3)
        .context("invalid predicate offset")?;
    let mut result = [0; 3];
    for (target, value) in result.iter_mut().zip(values) {
        *target = i32::try_from(value.as_i64().context("invalid predicate offset")?)?;
    }
    Ok(result)
}

fn ids(value: &Value) -> Result<Vec<ResourceId>> {
    if let Some(name) = value.as_str() {
        return Ok(vec![ResourceId::parse(name)?]);
    }
    let entries = value
        .as_array()
        .context("invalid predicate registry entries")?;
    ensure!(entries.len() <= 4096, "too many predicate registry entries");
    entries
        .iter()
        .map(|v| ResourceId::parse(v.as_str().context("invalid predicate id")?))
        .collect()
}

impl Predicate {
    fn decode(value: &Value, depth: u8) -> Result<Self> {
        ensure!(depth < 32, "block predicate nesting is too deep");
        let id = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("missing block predicate type")?,
        )?;
        ensure!(
            id.namespace == "minecraft",
            "unsupported block predicate namespace"
        );
        Ok(match id.path.as_str() {
            "true" => Self::True,
            "matching_blocks" => {
                let v = value.get("blocks").context("missing predicate blocks")?;
                if let Some(tag) = v.as_str().and_then(|s| s.strip_prefix('#')) {
                    Self::Tag(offset(value)?, ResourceId::parse(tag)?)
                } else {
                    Self::Blocks(offset(value)?, ids(v)?)
                }
            }
            "matching_block_tag" => Self::Tag(
                offset(value)?,
                ResourceId::parse(
                    value
                        .get("tag")
                        .and_then(Value::as_str)
                        .context("missing block tag")?,
                )?,
            ),
            "matching_fluids" => Self::Fluids(
                offset(value)?,
                ids(value.get("fluids").context("missing predicate fluids")?)?,
            ),
            "solid" => Self::Solid(offset(value)?),
            "not" => Self::Not(Box::new(Self::decode(
                value.get("predicate").context("missing nested predicate")?,
                depth + 1,
            )?)),
            "all_of" | "any_of" => {
                let values = value
                    .get("predicates")
                    .and_then(Value::as_array)
                    .context("missing nested predicates")?;
                ensure!(values.len() <= 4096, "too many nested predicates");
                let predicates = values
                    .iter()
                    .map(|v| Self::decode(v, depth + 1))
                    .collect::<Result<Vec<_>>>()?;
                if id.path == "all_of" {
                    Self::All(predicates)
                } else {
                    Self::Any(predicates)
                }
            }
            other => bail!("unsupported disk block predicate {other}"),
        })
    }

    fn tags(&self, tags: &mut BTreeSet<ResourceId>) {
        match self {
            Self::Tag(_, id) => {
                tags.insert(id.clone());
            }
            Self::Not(value) => value.tags(tags),
            Self::Any(values) | Self::All(values) => {
                for value in values {
                    value.tags(tags);
                }
            }
            _ => {}
        }
    }

    fn blocks(&self, blocks: &mut BTreeSet<ResourceId>) {
        match self {
            Self::Blocks(_, ids) => blocks.extend(ids.iter().cloned()),
            Self::Not(value) => value.blocks(blocks),
            Self::Any(values) | Self::All(values) => {
                for value in values {
                    value.blocks(blocks);
                }
            }
            _ => {}
        }
    }

    fn test<W: DiskWorld>(&self, world: &mut W, pos: [i32; 3]) -> Result<bool> {
        let at = |offset: &[i32; 3]| std::array::from_fn(|i| pos[i].wrapping_add(offset[i]));
        Ok(match self {
            Self::True => true,
            Self::Blocks(offset, ids) => {
                let state = world.block(at(offset))?;
                let mut found = false;
                for id in ids {
                    if world.block_is(&state, id)? {
                        found = true;
                        break;
                    }
                }
                found
            }
            Self::Tag(offset, id) => {
                let state = world.block(at(offset))?;
                world.in_tag(&state, id)?
            }
            Self::Fluids(offset, ids) => {
                let state = world.block(at(offset))?;
                let mut found = false;
                for id in ids {
                    if world.fluid_is(&state, id)? {
                        found = true;
                        break;
                    }
                }
                found
            }
            Self::Solid(offset) => {
                let state = world.block(at(offset))?;
                world.is_solid(&state)?
            }
            Self::Not(child) => !child.test(world, pos)?,
            Self::Any(children) => {
                let mut found = false;
                for child in children {
                    if child.test(world, pos)? {
                        found = true;
                        break;
                    }
                }
                found
            }
            Self::All(children) => {
                let mut found = true;
                for child in children {
                    if !child.test(world, pos)? {
                        found = false;
                        break;
                    }
                }
                found
            }
        })
    }
}

#[derive(Clone, Debug)]
enum Provider<S> {
    State(S),
    Rules {
        fallback: Option<Box<Provider<S>>>,
        rules: Vec<(Predicate, Provider<S>)>,
    },
}
impl<S: Clone> Provider<S> {
    fn decode(
        value: &Value,
        resolve: &mut impl FnMut(&Value) -> Result<S>,
        depth: u8,
    ) -> Result<Self> {
        ensure!(depth < 32, "state provider nesting is too deep");
        if value.get("id").is_some() {
            return Ok(Self::State(resolve(value)?));
        }
        let id = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("missing state provider type")?,
        )?;
        ensure!(
            id.namespace == "minecraft",
            "unsupported state provider namespace"
        );
        match id.path.as_str() {
            "simple" => Ok(Self::State(resolve(
                value.get("state").context("missing provider state")?,
            )?)),
            "rule_based" => {
                let fallback = value
                    .get("fallback")
                    .map(|v| Self::decode(v, resolve, depth + 1).map(Box::new))
                    .transpose()?;
                let entries = value
                    .get("rules")
                    .and_then(Value::as_array)
                    .context("missing state provider rules")?;
                ensure!(entries.len() <= 4096, "too many state provider rules");
                let mut rules = Vec::with_capacity(entries.len());
                for entry in entries {
                    rules.push((
                        Predicate::decode(
                            entry.get("if_true").context("missing rule predicate")?,
                            0,
                        )?,
                        Self::decode(
                            entry.get("then").context("missing rule state")?,
                            resolve,
                            depth + 1,
                        )?,
                    ));
                }
                Ok(Self::Rules { fallback, rules })
            }
            other => bail!("unsupported disk state provider {other}"),
        }
    }
    fn tags(&self, tags: &mut BTreeSet<ResourceId>) {
        if let Self::Rules { fallback, rules } = self {
            if let Some(fallback) = fallback {
                fallback.tags(tags);
            }
            for (predicate, provider) in rules {
                predicate.tags(tags);
                provider.tags(tags);
            }
        }
    }
    fn blocks(&self, blocks: &mut BTreeSet<ResourceId>) {
        if let Self::Rules { fallback, rules } = self {
            if let Some(fallback) = fallback {
                fallback.blocks(blocks);
            }
            for (predicate, provider) in rules {
                predicate.blocks(blocks);
                provider.blocks(blocks);
            }
        }
    }
    fn state<W: DiskWorld<State = S>>(&self, world: &mut W, pos: [i32; 3]) -> Result<Option<S>>
    where
        S: Eq,
    {
        match self {
            Self::State(state) => Ok(Some(state.clone())),
            Self::Rules { fallback, rules } => {
                for (predicate, provider) in rules {
                    if predicate.test(world, pos)? {
                        if let Some(state) = provider.state(world, pos)? {
                            return Ok(Some(state));
                        }
                    }
                }
                if let Some(fallback) = fallback {
                    fallback.state(world, pos)
                } else {
                    Ok(None)
                }
            }
        }
    }
}

/// The world remains responsible for all block write and postprocessing effects.
pub struct DiskFeature<S> {
    target: Predicate,
    provider: Provider<S>,
    radius: IntProvider,
    half_height: i32,
}
impl<S: Clone + Eq> DiskFeature<S> {
    pub fn load(
        provider: &mut impl ResourceProvider,
        id: &ResourceId,
        resolve: &mut impl FnMut(&Value) -> Result<S>,
    ) -> Result<Self> {
        let path = format!("data/{}/worldgen/feature/{}.json", id.namespace, id.path);
        let data = provider
            .read_data(&path)?
            .with_context(|| format!("missing feature {path}"))?;
        Self::decode(
            &serde_json::from_slice::<Value>(&data)
                .with_context(|| format!("invalid feature {path}"))?,
            resolve,
        )
    }
    pub fn decode(value: &Value, resolve: &mut impl FnMut(&Value) -> Result<S>) -> Result<Self> {
        let id = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("missing feature type")?,
        )?;
        ensure!(
            id.namespace == "minecraft" && id.path == "disk",
            "wrong disk feature type"
        );
        let half_height = i32::try_from(
            value
                .get("half_height")
                .and_then(Value::as_i64)
                .context("missing disk half height")?,
        )?;
        ensure!((0..=4).contains(&half_height), "invalid disk half height");
        let radius = IntProvider::parse(value.get("radius").context("missing disk radius")?, 0)?;
        let (min, max) = radius.bounds();
        ensure!(min >= 0 && max <= 8, "invalid disk radius");
        Ok(Self {
            target: Predicate::decode(value.get("target").context("missing disk target")?, 0)?,
            provider: Provider::decode(
                value
                    .get("state_provider")
                    .context("missing disk state provider")?,
                resolve,
                0,
            )?,
            radius,
            half_height,
        })
    }
    pub fn required_tags(&self) -> BTreeSet<ResourceId> {
        let mut tags = BTreeSet::new();
        self.target.tags(&mut tags);
        self.provider.tags(&mut tags);
        tags
    }
    /// Direct block identifiers referenced by target and provider predicates.
    pub fn required_blocks(&self) -> BTreeSet<ResourceId> {
        let mut blocks = BTreeSet::new();
        self.target.blocks(&mut blocks);
        self.provider.blocks(&mut blocks);
        blocks
    }
    pub fn place<W: DiskWorld<State = S>>(
        &self,
        world: &mut W,
        random: &mut DecorationRandom,
        origin: [i32; 3],
    ) -> Result<bool> {
        let radius = self.radius.sample(random);
        let mut placed = false;
        // BlockPos.betweenClosed iterates X fastest, then Y, then Z.
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                if dx * dx + dz * dz > radius * radius {
                    continue;
                }
                let x = origin[0].wrapping_add(dx);
                let z = origin[2].wrapping_add(dz);
                let mut previous = false;
                for dy in (-self.half_height..=self.half_height).rev() {
                    let pos = [x, origin[1].wrapping_add(dy), z];
                    if self.target.test(world, pos)? {
                        if let Some(state) = self.provider.state(world, pos)? {
                            world.set_disk_block(pos, &state)?;
                            if !previous {
                                let mut above = pos;
                                for _ in 0..2 {
                                    above[1] = above[1].wrapping_add(1);
                                    let state = world.block(above)?;
                                    if world.is_air(&state)? {
                                        break;
                                    }
                                    world.mark_postprocessing(above)?;
                                }
                            }
                            previous = true;
                            placed = true;
                        }
                    } else {
                        previous = false;
                    }
                }
            }
        }
        Ok(placed)
    }
}
