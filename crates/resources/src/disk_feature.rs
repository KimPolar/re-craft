//! 26.3 disk features, including their ordered postprocessing marks.
use std::collections::BTreeSet;

use anyhow::{Context, Result, bail, ensure};
use native_core::{
    normal_noise::NormalNoise,
    random::{DecorationRandom, LegacyRandom},
};
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
    /// Resolve an integer property's actual block-state definition. None means
    /// the property is absent or has a non-integer type.
    fn integer_property_bounds(
        &mut self,
        _state: &Self::State,
        _name: &str,
    ) -> Result<Option<(i32, i32)>> {
        bail!("integer block properties are unavailable")
    }
    fn with_integer_property(
        &mut self,
        _state: Self::State,
        _name: &str,
        _value: i32,
    ) -> Result<Self::State> {
        bail!("integer block properties are unavailable")
    }
    fn fluid_in_tag(&mut self, _state: &Self::State, _tag: &ResourceId) -> Result<bool> {
        bail!("unsupported survival fluid tag")
    }
    fn is_liquid(&mut self, _state: &Self::State) -> Result<bool> {
        bail!("unsupported block liquid property")
    }
    fn inside_build_height(&mut self, _y: i32) -> Result<bool> {
        bail!("world build height unavailable")
    }
    /// The world must provide current raw generation brightness and support.
    fn mushroom_survives(&mut self, _pos: [i32; 3]) -> Result<bool> {
        bail!("mushroom survival context unavailable")
    }
    fn would_survive(&mut self, state: &Value, pos: [i32; 3]) -> Result<bool> {
        let raw = state
            .as_str()
            .or_else(|| state.get("id").and_then(Value::as_str))
            .context("missing survival block")?;
        let id = ResourceId::parse(raw)?;
        ensure!(
            id.namespace == "minecraft",
            "unsupported would_survive block"
        );
        if matches!(id.path.as_str(), "brown_mushroom" | "red_mushroom") {
            return self.mushroom_survives(pos);
        }
        if let Some(tag) = vegetation_support(&id) {
            let below = self.block([pos[0], pos[1].wrapping_sub(1), pos[2]])?;
            return self.in_tag(&below, &ResourceId::parse(tag)?);
        }
        if id.path == "cactus" {
            let lava = ResourceId::parse("minecraft:lava")?;
            for [dx, dz] in [[0, -1], [1, 0], [0, 1], [-1, 0]] {
                let neighbor =
                    self.block([pos[0].wrapping_add(dx), pos[1], pos[2].wrapping_add(dz)])?;
                if self.is_solid(&neighbor)? || self.fluid_in_tag(&neighbor, &lava)? {
                    return Ok(false);
                }
            }
            let below = self.block([pos[0], pos[1].wrapping_sub(1), pos[2]])?;
            if !self.block_is(&below, &id)?
                && !self.in_tag(&below, &ResourceId::parse("minecraft:supports_cactus")?)?
            {
                return Ok(false);
            }
            let above = self.block([pos[0], pos[1].wrapping_add(1), pos[2]])?;
            return Ok(!self.is_liquid(&above)?);
        }
        ensure!(id.path == "sugar_cane", "unsupported would_survive block");
        let below = [pos[0], pos[1].wrapping_sub(1), pos[2]];
        let base = self.block(below)?;
        if self.block_is(&base, &id)? {
            return Ok(true);
        }
        if !self.in_tag(&base, &ResourceId::parse("minecraft:supports_sugar_cane")?)? {
            return Ok(false);
        }
        let adjacent = ResourceId::parse("minecraft:supports_sugar_cane_adjacently")?;
        for [dx, dz] in [[0, -1], [1, 0], [0, 1], [-1, 0]] {
            let neighbor = self.block([
                below[0].wrapping_add(dx),
                below[1],
                below[2].wrapping_add(dz),
            ])?;
            if self.fluid_in_tag(&neighbor, &adjacent)? || self.in_tag(&neighbor, &adjacent)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

fn vegetation_support(id: &ResourceId) -> Option<&'static str> {
    if id.namespace != "minecraft" {
        return None;
    }
    match id.path.as_str() {
        "oak_sapling" | "spruce_sapling" | "birch_sapling" | "jungle_sapling"
        | "acacia_sapling" | "dark_oak_sapling" | "cherry_sapling" | "pale_oak_sapling"
        | "short_grass" | "fern" | "bush" | "dandelion" | "poppy" | "blue_orchid" | "allium"
        | "azure_bluet" | "red_tulip" | "orange_tulip" | "white_tulip" | "pink_tulip"
        | "oxeye_daisy" | "cornflower" | "lily_of_the_valley" | "firefly_bush"
        | "closed_eyeblossom" | "sweet_berry_bush" => Some("minecraft:supports_vegetation"),
        "short_dry_grass" | "tall_dry_grass" | "dead_bush" => {
            Some("minecraft:supports_dry_vegetation")
        }
        _ => None,
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Predicate {
    True,
    Blocks([i32; 3], Vec<ResourceId>),
    Tag([i32; 3], ResourceId),
    Fluids([i32; 3], Vec<ResourceId>),
    Solid([i32; 3]),
    InsideWorldBounds([i32; 3]),
    WouldSurvive([i32; 3], ResourceId, Value),
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

    /// Java decodes these predicate classes as distinct objects and they do
    /// not implement value equality. `true` is a shared singleton instead.
    pub fn has_independent_identity(&self) -> bool {
        !matches!(self.0, Predicate::True)
    }
    pub fn needs_sugar_cane_survival(&self) -> bool {
        self.0.needs_survival("sugar_cane")
    }
    pub fn needs_survival(&self, name: &str) -> bool {
        self.0.needs_survival(name)
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
            "inside_world_bounds" => {
                let offset = offset(value)?;
                ensure!(
                    offset.iter().all(|n| (-16..=16).contains(n)),
                    "world bounds offset exceeds Java codec"
                );
                Self::InsideWorldBounds(offset)
            }
            "would_survive" => {
                let state = value.get("state").context("missing survival state")?;
                let raw = state
                    .as_str()
                    .or_else(|| state.get("id").and_then(Value::as_str))
                    .context("missing survival block id")?;
                Self::WouldSurvive(offset(value)?, ResourceId::parse(raw)?, state.clone())
            }
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
            Self::WouldSurvive(_, id, _)
                if id.namespace == "minecraft" && id.path == "sugar_cane" =>
            {
                tags.insert(
                    ResourceId::parse("minecraft:supports_sugar_cane").expect("constant tag"),
                );
                tags.insert(
                    ResourceId::parse("minecraft:supports_sugar_cane_adjacently")
                        .expect("constant tag"),
                );
            }
            Self::WouldSurvive(_, id, _)
                if id.namespace == "minecraft"
                    && matches!(id.path.as_str(), "brown_mushroom" | "red_mushroom") =>
            {
                tags.insert(
                    ResourceId::parse("minecraft:overrides_mushroom_light_requirement")
                        .expect("constant tag"),
                );
            }
            Self::WouldSurvive(_, id, _) if id.namespace == "minecraft" && id.path == "cactus" => {
                tags.insert(ResourceId::parse("minecraft:supports_cactus").expect("constant tag"));
            }
            Self::WouldSurvive(_, id, _) if vegetation_support(id).is_some() => {
                tags.insert(
                    ResourceId::parse(vegetation_support(id).unwrap()).expect("constant tag"),
                );
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
            Self::WouldSurvive(_, id, _) => {
                blocks.insert(id.clone());
            }
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
            Self::InsideWorldBounds(offset) => world.inside_build_height(at(offset)[1])?,
            Self::WouldSurvive(offset, _, state) => world.would_survive(state, at(offset))?,
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
    fn needs_survival(&self, name: &str) -> bool {
        match self {
            Self::WouldSurvive(_, id, _) => id.namespace == "minecraft" && id.path == name,
            Self::Not(v) => v.needs_survival(name),
            Self::All(values) | Self::Any(values) => values.iter().any(|v| v.needs_survival(name)),
            _ => false,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Provider<S> {
    State(S),
    Weighted {
        entries: Vec<(i32, S)>,
        total: i32,
    },
    Rules {
        fallback: Option<Box<Provider<S>>>,
        rules: Vec<(Predicate, Provider<S>)>,
    },
    NoiseThreshold {
        noise: NormalNoise,
        scale: f32,
        threshold: f32,
        high_chance: f32,
        default: S,
        low: Vec<S>,
        high: Vec<S>,
    },
    RandomizedInt {
        source: Box<Provider<S>>,
        property: String,
        values: IntProvider,
    },
}
impl<S: Clone> Provider<S> {
    /// Holder strings refer to the active data-pack state-provider registry.
    pub(crate) fn load(
        resources: &mut impl ResourceProvider,
        value: &Value,
        resolve: &mut impl FnMut(&Value) -> Result<S>,
    ) -> Result<Self> {
        Self::decode_inner(
            value,
            resolve,
            0,
            &mut Some(resources),
            &mut BTreeSet::new(),
        )
    }
    pub(crate) fn needs_survival(&self, name: &str) -> bool {
        match self {
            Self::Rules { fallback, rules } => {
                fallback.as_ref().is_some_and(|v| v.needs_survival(name))
                    || rules
                        .iter()
                        .any(|(p, v)| p.needs_survival(name) || v.needs_survival(name))
            }
            Self::RandomizedInt { source, .. } => source.needs_survival(name),
            _ => false,
        }
    }
    pub(crate) fn decode(
        value: &Value,
        resolve: &mut impl FnMut(&Value) -> Result<S>,
        depth: u8,
    ) -> Result<Self> {
        Self::decode_inner(value, resolve, depth, &mut None, &mut BTreeSet::new())
    }
    fn decode_inner(
        value: &Value,
        resolve: &mut impl FnMut(&Value) -> Result<S>,
        depth: u8,
        resources: &mut Option<&mut dyn ResourceProvider>,
        active: &mut BTreeSet<String>,
    ) -> Result<Self> {
        ensure!(depth < 32, "state provider nesting is too deep");
        if let Some(name) = value.as_str() {
            let id = ResourceId::parse(name)?;
            let path = format!(
                "data/{}/worldgen/block_state_provider/{}.json",
                id.namespace, id.path
            );
            ensure!(
                active.insert(path.clone()),
                "cyclic state provider reference {path}"
            );
            let result = (|| {
                let bytes = resources
                    .as_mut()
                    .context("named state provider requires active resources")?
                    .read_data(&path)?
                    .with_context(|| format!("missing state provider {path}"))?;
                let data: Value = serde_json::from_slice(&bytes)
                    .with_context(|| format!("invalid state provider {path}"))?;
                Self::decode_inner(&data, resolve, depth + 1, resources, active)
            })();
            active.remove(&path);
            return result;
        }
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
            "weighted" => {
                let values = value
                    .get("entries")
                    .and_then(Value::as_array)
                    .context("missing weighted states")?;
                ensure!(
                    !values.is_empty() && values.len() <= 4096,
                    "invalid weighted state count"
                );
                let mut entries = Vec::with_capacity(values.len());
                let mut total = 0i32;
                for value in values {
                    let weight = i32::try_from(
                        value
                            .get("weight")
                            .and_then(Value::as_i64)
                            .context("invalid state weight")?,
                    )?;
                    ensure!(weight >= 0, "negative state weight");
                    total = total
                        .checked_add(weight)
                        .context("state weight sum overflow")?;
                    entries.push((
                        weight,
                        resolve(value.get("data").context("missing weighted state")?)?,
                    ));
                }
                ensure!(total > 0, "weighted state provider has no positive weight");
                Ok(Self::Weighted { entries, total })
            }
            "noise_threshold" => {
                let seed = value
                    .get("seed")
                    .and_then(Value::as_i64)
                    .context("invalid noise provider seed")?;
                let params = crate::noise::decode_normal_noise(&serde_json::to_vec(
                    value.get("noise").context("missing provider noise")?,
                )?)?;
                let noise = params
                    .create(&mut LegacyRandom::new(seed))
                    .map_err(anyhow::Error::msg)?;
                let number = |key: &str| -> Result<f32> {
                    let result = value
                        .get(key)
                        .and_then(Value::as_f64)
                        .with_context(|| format!("invalid noise provider {key}"))?
                        as f32;
                    ensure!(result.is_finite(), "invalid noise provider {key}");
                    Ok(result)
                };
                let scale = number("scale")?;
                let threshold = number("threshold")?;
                let high_chance = number("high_chance")?;
                ensure!(
                    scale > 0.0
                        && (-1.0..=1.0).contains(&threshold)
                        && (0.0..=1.0).contains(&high_chance),
                    "invalid noise threshold provider range"
                );
                let parse_states = |name: &str, resolve: &mut _| -> Result<Vec<S>> {
                    let list = value
                        .get(name)
                        .and_then(Value::as_array)
                        .with_context(|| format!("missing {name}"))?;
                    ensure!(
                        !list.is_empty() && list.len() <= 4096,
                        "invalid noise provider state list"
                    );
                    list.iter().map(resolve).collect()
                };
                let default = resolve(
                    value
                        .get("default_state")
                        .context("missing default state")?,
                )?;
                let low = parse_states("low_states", resolve)?;
                let high = parse_states("high_states", resolve)?;
                Ok(Self::NoiseThreshold {
                    noise,
                    scale,
                    threshold,
                    high_chance,
                    default,
                    low,
                    high,
                })
            }
            "rule_based" => {
                let fallback = value
                    .get("fallback")
                    .map(|v| {
                        Self::decode_inner(v, resolve, depth + 1, resources, active).map(Box::new)
                    })
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
                        Self::decode_inner(
                            entry.get("then").context("missing rule state")?,
                            resolve,
                            depth + 1,
                            resources,
                            active,
                        )?,
                    ));
                }
                Ok(Self::Rules { fallback, rules })
            }
            "randomized_int" => {
                let property = value
                    .get("property")
                    .and_then(Value::as_str)
                    .context("missing randomized property")?
                    .to_owned();
                let values = IntProvider::parse(
                    value.get("values").context("missing randomized values")?,
                    0,
                )?;
                let source = Self::decode_inner(
                    value.get("source").context("missing randomized source")?,
                    resolve,
                    depth + 1,
                    resources,
                    active,
                )?;
                Ok(Self::RandomizedInt {
                    source: Box::new(source),
                    property,
                    values,
                })
            }
            other => bail!("unsupported disk state provider {other}"),
        }
    }
    pub(crate) fn tags(&self, tags: &mut BTreeSet<ResourceId>) {
        if let Self::RandomizedInt { source, .. } = self {
            source.tags(tags);
        }
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
    pub(crate) fn blocks(&self, blocks: &mut BTreeSet<ResourceId>) {
        if let Self::RandomizedInt { source, .. } = self {
            source.blocks(blocks);
        }
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
    pub(crate) fn states<'a>(&'a self, states: &mut Vec<&'a S>) {
        match self {
            Self::State(state) => states.push(state),
            Self::Weighted { entries, .. } => states.extend(entries.iter().map(|(_, state)| state)),
            Self::NoiseThreshold {
                default, low, high, ..
            } => {
                states.push(default);
                states.extend(low.iter());
                states.extend(high.iter());
            }
            Self::Rules { fallback, rules } => {
                if let Some(fallback) = fallback {
                    fallback.states(states);
                }
                for (_, provider) in rules {
                    provider.states(states);
                }
            }
            Self::RandomizedInt { source, .. } => source.states(states),
        }
    }
    pub(crate) fn state<W: DiskWorld<State = S>>(
        &self,
        world: &mut W,
        random: &mut DecorationRandom,
        pos: [i32; 3],
    ) -> Result<Option<S>>
    where
        S: Eq,
    {
        match self {
            Self::State(state) => Ok(Some(state.clone())),
            Self::Weighted { entries, total } => {
                let mut choice = random
                    .next_int_bound(*total)
                    .context("invalid state weight total")?;
                for (weight, state) in entries {
                    choice -= weight;
                    if choice < 0 {
                        return Ok(Some(state.clone()));
                    }
                }
                unreachable!("validated state weight sum")
            }
            Self::NoiseThreshold {
                noise,
                scale,
                threshold,
                high_chance,
                default,
                low,
                high,
            } => {
                let factor = f64::from(*scale);
                let sample = noise.sample(
                    f64::from(pos[0]) * factor,
                    f64::from(pos[1]) * factor,
                    f64::from(pos[2]) * factor,
                );
                let selected = if sample < *threshold {
                    Some(low)
                } else if random.next_float() < *high_chance {
                    Some(high)
                } else {
                    None
                };
                match selected {
                    Some(states) => Ok(Some(
                        states[random
                            .next_int_bound(states.len() as i32)
                            .expect("nonempty state list") as usize]
                            .clone(),
                    )),
                    None => Ok(Some(default.clone())),
                }
            }
            Self::Rules { fallback, rules } => {
                for (predicate, provider) in rules {
                    if predicate.test(world, pos)? {
                        if let Some(state) = provider.state(world, random, pos)? {
                            return Ok(Some(state));
                        }
                    }
                }
                if let Some(fallback) = fallback {
                    fallback.state(world, random, pos)
                } else {
                    Ok(None)
                }
            }
            Self::RandomizedInt {
                source,
                property,
                values,
            } => {
                let Some(state) = source.state(world, random, pos)? else {
                    return Ok(None);
                };
                let Some((min, max)) = world.integer_property_bounds(&state, property)? else {
                    return Ok(Some(state));
                };
                let value = values.sample(random);
                ensure!(
                    (min..=max).contains(&value),
                    "randomized property {property} value {value} is outside {min}..={max}"
                );
                Ok(Some(world.with_integer_property(state, property, value)?))
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
    pub fn needs_sugar_cane_survival(&self) -> bool {
        self.needs_survival("sugar_cane")
    }
    pub fn needs_survival(&self, name: &str) -> bool {
        self.target.needs_survival(name) || self.provider.needs_survival(name)
    }
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
                        if let Some(state) = self.provider.state(world, random, pos)? {
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

#[cfg(test)]
mod randomized_provider_tests {
    use super::*;
    use serde_json::json;
    use std::{collections::BTreeMap, path::PathBuf, process::Command};
    #[derive(Clone, Debug, Eq, PartialEq)]
    struct State {
        name: String,
        age: Option<i32>,
        berries: Option<bool>,
    }
    fn resolve(v: &Value) -> Result<State> {
        let name = v
            .as_str()
            .or_else(|| v.get("id").and_then(Value::as_str))
            .context("missing state")?;
        Ok(State {
            name: name.into(),
            age: (name == "minecraft:cave_vines").then_some(0),
            berries: matches!(name, "minecraft:cave_vines" | "minecraft:cave_vines_plant")
                .then_some(false),
        })
    }
    struct World;
    impl OreWorld for World {
        type State = State;
        fn block(&mut self, _: [i32; 3]) -> Result<State> {
            unreachable!()
        }
        fn block_is(&mut self, _: &State, _: &ResourceId) -> Result<bool> {
            unreachable!()
        }
        fn in_tag(&mut self, _: &State, _: &ResourceId) -> Result<bool> {
            unreachable!()
        }
        fn is_air(&mut self, _: &State) -> Result<bool> {
            unreachable!()
        }
        fn set_block(&mut self, _: [i32; 3], _: &State, _: i32) -> Result<bool> {
            unreachable!()
        }
    }
    impl DiskWorld for World {
        fn is_solid(&mut self, _: &State) -> Result<bool> {
            unreachable!()
        }
        fn fluid_is(&mut self, _: &State, _: &ResourceId) -> Result<bool> {
            unreachable!()
        }
        fn set_disk_block(&mut self, _: [i32; 3], _: &State) -> Result<bool> {
            unreachable!()
        }
        fn mark_postprocessing(&mut self, _: [i32; 3]) -> Result<()> {
            unreachable!()
        }
        fn integer_property_bounds(&mut self, s: &State, name: &str) -> Result<Option<(i32, i32)>> {
            Ok((name == "age" && s.age.is_some()).then_some((0, 25)))
        }
        fn with_integer_property(&mut self, mut s: State, name: &str, value: i32) -> Result<State> {
            ensure!(
                name == "age" && s.age.is_some() && (0..=25).contains(&value),
                "invalid property"
            );
            s.age = Some(value);
            Ok(s)
        }
    }
    struct Packs(BTreeMap<String, Vec<u8>>);
    impl ResourceProvider for Packs {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.0.get(path).cloned())
        }
    }
    fn randomized(property: &str, source: Value) -> Value {
        json!({"type":"minecraft:randomized_int","source":source,"property":property,"values":{"type":"minecraft:uniform","min_inclusive":23,"max_inclusive":26}})
    }
    fn weighted() -> Value {
        json!({"type":"minecraft:weighted","entries":[{"data":"minecraft:cave_vines","weight":2},{"data":"minecraft:cave_vines_plant","weight":1},{"data":"minecraft:stone","weight":1}]})
    }
    #[test]
    fn nested_holders_use_active_resources_and_reject_cycles() {
        let path = |name: &str| format!("data/example/worldgen/block_state_provider/{name}.json");
        let mut packs = Packs(BTreeMap::from([
            (
                path("outer"),
                serde_json::to_vec(&randomized("age", json!("example:inner"))).unwrap(),
            ),
            (path("inner"), serde_json::to_vec(&weighted()).unwrap()),
        ]));
        let provider = Provider::load(&mut packs, &json!("example:outer"), &mut resolve).unwrap();
        let mut states = Vec::new();
        provider.states(&mut states);
        assert_eq!(states.len(), 3);
        let mut a = DecorationRandom::new(7);
        let mut b = a.clone();
        let inline = Provider::decode(&randomized("age", weighted()), &mut resolve, 0).unwrap();
        for _ in 0..12 {
            assert_eq!(
                provider.state(&mut World, &mut a, [0; 3]).ok(),
                inline.state(&mut World, &mut b, [0; 3]).ok()
            );
        }
        assert_eq!(a.next_long(), b.next_long());
        packs.0.insert(
            path("inner"),
            serde_json::to_vec(&randomized("age", json!("example:outer"))).unwrap(),
        );
        assert!(
            Provider::load(&mut packs, &json!("example:outer"), &mut resolve)
                .unwrap_err()
                .to_string()
                .contains("cyclic")
        );
        assert!(
            Provider::decode(&randomized("age", json!("example:inner")), &mut resolve, 0).is_err()
        );
    }
    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn randomized_int_matches_java_property_lookup_and_random_consumption() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        assert_eq!(manifest["id"], "26.3");
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for lib in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("RandomizedStateOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/RandomizedStateOracle.java"),
        )
        .unwrap();
        let run = |c: &mut Command| {
            let o = c.output().unwrap();
            assert!(
                o.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&o.stderr),
                String::from_utf8_lossy(&o.stdout)
            );
            String::from_utf8(o.stdout).unwrap()
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let actual = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("RandomizedStateOracle"));
        let mut expected = Vec::new();
        for property in ["age", "berries", "missing"] {
            for seed in [0, 1, -1, 42, 987654321] {
                let provider =
                    Provider::decode(&randomized(property, weighted()), &mut resolve, 0).unwrap();
                let mut random = DecorationRandom::new(seed);
                let mut samples = Vec::new();
                for _ in 0..12 {
                    let item = provider.state(&mut World, &mut random, [0; 3]);
                    samples.push(match item {
                        Err(_) => "ERR".into(),
                        Ok(Some(s)) => {
                            let extra = match (s.age, s.berries) {
                                (Some(age), Some(berries)) => {
                                    format!(",age={age},berries={berries}")
                                }
                                (None, Some(berries)) => format!(",berries={berries}"),
                                _ => String::new(),
                            };
                            format!("{}{extra}", s.name)
                        }
                        Ok(None) => panic!("empty state"),
                    });
                }
                expected.push(format!(
                    "I {property} {seed} {} R {}",
                    samples.join(";"),
                    random.next_long()
                ));
            }
        }
        let actual: Vec<_> = actual.lines().filter(|v| v.starts_with("I ")).collect();
        assert_eq!(actual.len(), expected.len());
        for (a, b) in actual.iter().zip(&expected) {
            assert_eq!(*a, b);
        }
    }
}
