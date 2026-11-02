//! Placement modifiers for data-pack features. Feature execution itself is
//! separate: callers must consume the same random source between positions.
use anyhow::{Context, Result, bail, ensure};
use native_core::{random::DecorationRandom, simplex::Simplex2D};
use serde_json::Value;
use std::sync::OnceLock;

use crate::{
    disk_feature::BlockPredicate,
    packs::{ResourceId, ResourceProvider},
};

#[derive(Clone, Debug)]
pub(crate) enum IntProvider {
    Constant(i32),
    Uniform(i32, i32),
    Biased(i32, i32),
    Trapezoid(i32, i32, i32),
    Weighted(Vec<(i32, IntProvider)>, i32),
}

fn integer(value: &Value, key: &str) -> Result<i32> {
    value
        .get(key)
        .and_then(Value::as_i64)
        .and_then(|v| i32::try_from(v).ok())
        .with_context(|| format!("missing or invalid {key}"))
}
fn kind(value: &Value) -> Result<String> {
    let id = ResourceId::parse(
        value
            .get("type")
            .and_then(Value::as_str)
            .context("missing type")?,
    )?;
    ensure!(
        id.namespace == "minecraft",
        "unsupported provider namespace"
    );
    Ok(id.path)
}
fn heightmap_name(entry: &Value) -> Result<String> {
    let name = entry
        .get("heightmap")
        .and_then(Value::as_str)
        .context("missing heightmap")?;
    ensure!(
        [
            "WORLD_SURFACE_WG",
            "WORLD_SURFACE",
            "OCEAN_FLOOR_WG",
            "OCEAN_FLOOR",
            "MOTION_BLOCKING",
            "MOTION_BLOCKING_NO_LEAVES"
        ]
        .contains(&name),
        "unsupported heightmap"
    );
    Ok(name.to_owned())
}
impl IntProvider {
    pub(crate) fn parse(value: &Value, depth: u8) -> Result<Self> {
        ensure!(depth < 32, "integer provider nesting is too deep");
        if let Some(value) = value.as_i64() {
            return Ok(Self::Constant(i32::try_from(value)?));
        }
        Ok(match kind(value)?.as_str() {
            "constant" => Self::Constant(integer(value, "value")?),
            "uniform" => {
                let min = integer(value, "min_inclusive")?;
                let max = integer(value, "max_inclusive")?;
                ensure!(
                    min <= max && i64::from(max) - i64::from(min) < i64::from(i32::MAX),
                    "invalid uniform integer range"
                );
                Self::Uniform(min, max)
            }
            "biased_to_bottom" => {
                let min = integer(value, "min_inclusive")?;
                let max = integer(value, "max_inclusive")?;
                ensure!(
                    min <= max && i64::from(max) - i64::from(min) < i64::from(i32::MAX),
                    "invalid biased integer range"
                );
                Self::Biased(min, max)
            }
            "trapezoid" => {
                let min = integer(value, "min")?;
                let max = integer(value, "max")?;
                let plateau = integer(value, "plateau")?;
                let width = i64::from(max) - i64::from(min);
                ensure!(
                    width >= 0
                        && width < i64::from(i32::MAX)
                        && plateau >= 0
                        && i64::from(plateau) <= width,
                    "invalid trapezoid integer range"
                );
                Self::Trapezoid(min, max, plateau)
            }
            "weighted_list" => {
                let list = value
                    .get("distribution")
                    .and_then(Value::as_array)
                    .context("missing weighted distribution")?;
                ensure!(
                    !list.is_empty() && list.len() <= 65536,
                    "invalid weighted distribution"
                );
                let mut total = 0i32;
                let mut items = Vec::with_capacity(list.len());
                for item in list {
                    let weight = integer(item, "weight")?;
                    ensure!(weight >= 0, "invalid weight");
                    total = total.checked_add(weight).context("weight sum overflow")?;
                    items.push((
                        weight,
                        Self::parse(
                            item.get("data").context("missing weighted data")?,
                            depth + 1,
                        )?,
                    ));
                }
                ensure!(total > 0, "weighted distribution has no positive weight");
                Self::Weighted(items, total)
            }
            name => bail!("unsupported integer provider {name}"),
        })
    }
    pub(crate) fn bounds(&self) -> (i32, i32) {
        match self {
            Self::Constant(v) => (*v, *v),
            Self::Uniform(a, b) | Self::Biased(a, b) | Self::Trapezoid(a, b, _) => (*a, *b),
            Self::Weighted(items, _) => items
                .iter()
                .map(|(_, p)| p.bounds())
                .fold((i32::MAX, i32::MIN), |(a, b), (c, d)| (a.min(c), b.max(d))),
        }
    }
    pub(crate) fn sample(&self, random: &mut DecorationRandom) -> i32 {
        match self {
            Self::Constant(v) => *v,
            Self::Uniform(a, b) => a.wrapping_add(random.next_int_bound(b - a + 1).unwrap()),
            Self::Biased(a, b) => {
                let bound = random.next_int_bound(b - a + 1).unwrap() + 1;
                a.wrapping_add(random.next_int_bound(bound).unwrap())
            }
            Self::Trapezoid(a, b, plateau) => {
                if *plateau == 0 && *b == a.wrapping_neg() {
                    return random.next_int_bound(b + 1).unwrap()
                        - random.next_int_bound(b + 1).unwrap();
                }
                let width = b - a;
                if *plateau == width {
                    return a.wrapping_add(random.next_int_bound(width + 1).unwrap());
                }
                let slope = (width - plateau) / 2;
                let high = width - slope;
                a.wrapping_add(random.next_int_bound(high + 1).unwrap())
                    .wrapping_add(random.next_int_bound(slope + 1).unwrap())
            }
            Self::Weighted(items, total) => {
                let mut choice = random.next_int_bound(*total).unwrap();
                for (weight, provider) in items {
                    choice -= weight;
                    if choice < 0 {
                        return provider.sample(random);
                    }
                }
                unreachable!("validated weight sum")
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Anchor {
    Absolute(i32),
    AboveBottom(i32),
    BelowTop(i32),
}
impl Anchor {
    fn parse(v: &Value) -> Result<Self> {
        let object = v.as_object().context("height anchor must be an object")?;
        ensure!(object.len() == 1, "invalid height anchor");
        if object.contains_key("absolute") {
            Ok(Self::Absolute(integer(v, "absolute")?))
        } else if object.contains_key("above_bottom") {
            Ok(Self::AboveBottom(integer(v, "above_bottom")?))
        } else if object.contains_key("below_top") {
            Ok(Self::BelowTop(integer(v, "below_top")?))
        } else {
            bail!("unsupported height anchor")
        }
    }
    fn resolve(self, min_y: i32, height: i32) -> i32 {
        match self {
            Self::Absolute(v) => v,
            Self::AboveBottom(v) => min_y.wrapping_add(v),
            Self::BelowTop(v) => min_y.wrapping_add(height).wrapping_sub(1).wrapping_sub(v),
        }
    }
}
#[derive(Clone, Debug)]
enum HeightProvider {
    Constant(Anchor),
    Uniform(Anchor, Anchor),
    Trapezoid(Anchor, Anchor, i32),
    VeryBiased(Anchor, Anchor, i32),
}
impl HeightProvider {
    fn parse(v: &Value) -> Result<Self> {
        if v.get("type").is_none() {
            return Ok(Self::Constant(Anchor::parse(v)?));
        }
        let min = || Anchor::parse(v.get("min_inclusive").context("missing minimum height")?);
        let max = || Anchor::parse(v.get("max_inclusive").context("missing maximum height")?);
        match kind(v)?.as_str() {
            "uniform" => Ok(Self::Uniform(min()?, max()?)),
            "trapezoid" => {
                let plateau = v
                    .get("plateau")
                    .map(|_| integer(v, "plateau"))
                    .transpose()?
                    .unwrap_or(0);
                ensure!(plateau >= 0, "negative plateau");
                Ok(Self::Trapezoid(min()?, max()?, plateau))
            }
            "very_biased_to_bottom" => {
                let inner = v
                    .get("inner")
                    .map(|_| integer(v, "inner"))
                    .transpose()?
                    .unwrap_or(1);
                ensure!(inner >= 1, "invalid inner height range");
                Ok(Self::VeryBiased(min()?, max()?, inner))
            }
            name => bail!("unsupported height provider {name}"),
        }
    }
    fn sample(&self, random: &mut DecorationRandom, min_y: i32, height: i32) -> Result<i32> {
        let between = |r: &mut DecorationRandom, a: i32, b: i32| -> Result<i32> {
            let width = i64::from(b) - i64::from(a) + 1;
            ensure!(
                width > 0 && width <= i64::from(i32::MAX),
                "height range overflow"
            );
            Ok(a.wrapping_add(r.next_int_bound(width as i32).unwrap()))
        };
        match self {
            Self::Constant(a) => Ok(a.resolve(min_y, height)),
            Self::Uniform(a, b) => {
                let a = a.resolve(min_y, height);
                let b = b.resolve(min_y, height);
                if a > b { Ok(a) } else { between(random, a, b) }
            }
            Self::Trapezoid(a, b, p) => {
                let a = a.resolve(min_y, height);
                let b = b.resolve(min_y, height);
                if a > b {
                    return Ok(a);
                }
                let width = i64::from(b) - i64::from(a);
                ensure!(width <= i64::from(i32::MAX), "height range overflow");
                if i64::from(*p) >= width {
                    return between(random, a, b);
                }
                let slope = ((width - i64::from(*p)) / 2) as i32;
                let high = width as i32 - slope;
                Ok(a.wrapping_add(between(random, 0, high)?)
                    .wrapping_add(between(random, 0, slope)?))
            }
            Self::VeryBiased(a, b, inner) => {
                let a = a.resolve(min_y, height);
                let b = b.resolve(min_y, height);
                if b.wrapping_sub(a).wrapping_sub(*inner).wrapping_add(1) <= 0 {
                    return Ok(a);
                }
                let next = |random: &mut DecorationRandom, min: i32, max: i32| -> Result<i32> {
                    if min >= max {
                        Ok(min)
                    } else {
                        between(random, min, max)
                    }
                };
                let first = next(random, a.wrapping_add(*inner), b)?;
                let second = next(random, a, first.wrapping_sub(1))?;
                next(random, a, second.wrapping_sub(1).wrapping_add(*inner))
            }
        }
    }
}

#[derive(Clone, Debug)]
enum Modifier {
    Count(IntProvider),
    NoiseThreshold {
        level: f64,
        below: i32,
        above: i32,
    },
    NoiseBased {
        ratio: i32,
        factor: f64,
        offset: f64,
    },
    InSquare,
    Height(HeightProvider),
    Rarity(i32),
    Heightmap(String),
    SurfaceRelativeThreshold(String, i32, i32),
    EnvironmentScan {
        direction: i32,
        target: BlockPredicate,
        allowed: BlockPredicate,
        max_steps: i32,
    },
    Biome,
    BlockPredicate(BlockPredicate),
    Offset(IntProvider, IntProvider, IntProvider),
}

/// A decoded placed feature. Unknown modifiers or providers are rejected.
pub struct PlacedFeature {
    pub feature: Value,
    modifiers: Vec<Modifier>,
}
impl PlacedFeature {
    pub fn needs_sugar_cane_survival(&self) -> bool {
        self.needs_survival("sugar_cane")
    }
    pub fn needs_survival(&self, name: &str) -> bool {
        self.modifiers.iter().any(|modifier| match modifier {
            Modifier::BlockPredicate(p) => p.needs_survival(name),
            Modifier::EnvironmentScan {
                target, allowed, ..
            } => target.needs_survival(name) || allowed.needs_survival(name),
            _ => false,
        })
    }
    pub fn required_tags(&self) -> std::collections::BTreeSet<ResourceId> {
        self.modifiers
            .iter()
            .filter_map(|m| match m {
                Modifier::BlockPredicate(p) => Some(p.required_tags()),
                Modifier::EnvironmentScan {
                    target, allowed, ..
                } => {
                    let mut tags = target.required_tags();
                    tags.extend(allowed.required_tags());
                    Some(tags)
                }
                _ => None,
            })
            .flatten()
            .collect()
    }
    pub fn required_blocks(&self) -> std::collections::BTreeSet<ResourceId> {
        self.modifiers
            .iter()
            .filter_map(|m| match m {
                Modifier::BlockPredicate(p) => Some(p.required_blocks()),
                Modifier::EnvironmentScan {
                    target, allowed, ..
                } => {
                    let mut blocks = target.required_blocks();
                    blocks.extend(allowed.required_blocks());
                    Some(blocks)
                }
                _ => None,
            })
            .flatten()
            .collect()
    }
    /// Each codec decode creates a distinct uniform/trapezoid height-provider
    /// object in 26.3. Since HeightRangePlacement is a record containing that
    /// object, separately decoded placed features cannot be equal when this
    /// modifier is present, even if their JSON is identical.
    pub fn has_independent_height_provider(&self) -> bool {
        self.modifiers.iter().any(|modifier| {
            matches!(
                modifier,
                Modifier::Height(
                    HeightProvider::Uniform(..)
                        | HeightProvider::Trapezoid(..)
                        | HeightProvider::VeryBiased(..)
                )
            )
        })
    }

    /// These modifiers contain an independently decoded Java object whose
    /// equality is identity-based. Distinct placed objects cannot compare equal.
    pub fn has_independent_modifier(&self) -> bool {
        self.has_independent_height_provider()
            || self.modifiers.iter().any(|modifier| {
                matches!(
                    modifier,
                    Modifier::BlockPredicate(predicate) if predicate.has_independent_identity()
                ) || matches!(modifier,Modifier::EnvironmentScan{target,allowed,..} if target.has_independent_identity() || allowed.has_independent_identity())
            })
    }
    pub fn load(provider: &mut impl ResourceProvider, id: &ResourceId) -> Result<Self> {
        let path = format!(
            "data/{}/worldgen/placed_feature/{}.json",
            id.namespace, id.path
        );
        let bytes = provider
            .read_data(&path)?
            .with_context(|| format!("missing placed feature {path}"))?;
        let value: Value = serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid placed feature {path}"))?;
        Self::decode(&value).with_context(|| format!("in placed feature {path}"))
    }
    pub fn decode(value: &Value) -> Result<Self> {
        let feature = value
            .get("feature")
            .context("missing configured feature")?
            .clone();
        ensure!(
            feature.is_string() || feature.is_object(),
            "invalid configured feature"
        );
        if let Some(id) = feature.as_str() {
            ResourceId::parse(id)?;
        }
        let entries = value
            .get("placement")
            .and_then(Value::as_array)
            .context("missing placement modifiers")?;
        ensure!(entries.len() <= 128, "too many placement modifiers");
        let mut modifiers = Vec::with_capacity(entries.len());
        for entry in entries {
            modifiers.push(match kind(entry)?.as_str() {
                "count" => {
                    let p = IntProvider::parse(entry.get("count").context("missing count")?, 0)?;
                    let (min, max) = p.bounds();
                    ensure!(min >= 0 && max <= 4096, "unsupported count range");
                    Modifier::Count(p)
                }
                "noise_threshold_count" => Modifier::NoiseThreshold {
                    level: entry
                        .get("noise_level")
                        .and_then(Value::as_f64)
                        .context("missing noise level")?,
                    below: integer(entry, "below_noise")?,
                    above: integer(entry, "above_noise")?,
                },
                "noise_based_count" => Modifier::NoiseBased {
                    ratio: integer(entry, "noise_to_count_ratio")?,
                    factor: entry
                        .get("noise_factor")
                        .and_then(Value::as_f64)
                        .context("missing noise factor")?,
                    offset: match entry.get("noise_offset") {
                        None => 0.0,
                        Some(v) => v.as_f64().context("invalid noise offset")?,
                    },
                },
                "in_square" => Modifier::InSquare,
                "height_range" => Modifier::Height(HeightProvider::parse(
                    entry.get("height").context("missing height")?,
                )?),
                "rarity_filter" => {
                    let chance = integer(entry, "chance")?;
                    ensure!(chance > 0, "invalid rarity");
                    Modifier::Rarity(chance)
                }
                "heightmap" => Modifier::Heightmap(heightmap_name(entry)?),
                "surface_relative_threshold_filter" => Modifier::SurfaceRelativeThreshold(
                    heightmap_name(entry)?,
                    entry
                        .get("min_inclusive")
                        .map(|_| integer(entry, "min_inclusive"))
                        .transpose()?
                        .unwrap_or(i32::MIN),
                    entry
                        .get("max_inclusive")
                        .map(|_| integer(entry, "max_inclusive"))
                        .transpose()?
                        .unwrap_or(i32::MAX),
                ),
                "environment_scan" => {
                    let direction = match entry.get("direction_of_search").and_then(Value::as_str) {
                        Some("up") => 1,
                        Some("down") => -1,
                        _ => bail!("invalid environment scan direction"),
                    };
                    let max_steps = integer(entry, "max_steps")?;
                    ensure!(
                        (1..=32).contains(&max_steps),
                        "invalid environment scan length"
                    );
                    let target = BlockPredicate::decode(
                        entry
                            .get("target_condition")
                            .context("missing scan target")?,
                    )?;
                    let allowed = match entry.get("allowed_search_condition") {
                        Some(v) => BlockPredicate::decode(v)?,
                        None => {
                            BlockPredicate::decode(&serde_json::json!({"type":"minecraft:true"}))?
                        }
                    };
                    Modifier::EnvironmentScan {
                        direction,
                        target,
                        allowed,
                        max_steps,
                    }
                }
                "biome" => Modifier::Biome,
                "block_predicate_filter" => Modifier::BlockPredicate(BlockPredicate::decode(
                    entry
                        .get("predicate")
                        .context("missing placement block predicate")?,
                )?),
                "offset" => {
                    let parse = |name| -> Result<IntProvider> {
                        let provider = IntProvider::parse(
                            entry
                                .get(name)
                                .with_context(|| format!("missing offset {name}"))?,
                            0,
                        )?;
                        let (min, max) = provider.bounds();
                        ensure!(min >= -16 && max <= 16, "offset range exceeds Java codec");
                        Ok(provider)
                    };
                    Modifier::Offset(parse("x")?, parse("y")?, parse("z")?)
                }
                name => bail!("unsupported placement modifier {name}"),
            });
        }
        Ok(Self { feature, modifiers })
    }
    pub fn positions(
        &self,
        origin: [i32; 3],
        min_y: i32,
        height: i32,
    ) -> Result<PlacementPositions<'_>> {
        ensure!(
            height > 0 && min_y.checked_add(height).is_some(),
            "invalid generation height"
        );
        Ok(PlacementPositions {
            feature: self,
            min_y,
            height,
            stack: vec![Frame {
                position: origin,
                remaining: None,
            }],
            done: false,
            failed: false,
        })
    }
}

/// Missing or unsupported world data must return an error, never a guessed height or biome.
pub trait PlacementEnvironment {
    /// Minimum Y of the actual level, which may differ from generator bounds.
    fn min_y(&self) -> i32;
    fn inside_build_height(&self, _y: i32) -> Result<bool> {
        bail!("world build height unavailable")
    }
    /// Java's first available Y in this heightmap.
    fn heightmap(&mut self, kind: &str, x: i32, z: i32) -> Result<i32>;
    /// The environment checks the top-level placed feature at this position.
    fn biome_allows(&mut self, position: [i32; 3]) -> Result<bool>;
    /// Evaluate against current world blocks, including earlier placements.
    fn block_predicate(
        &mut self,
        _predicate: &BlockPredicate,
        _position: [i32; 3],
    ) -> Result<bool> {
        bail!("placement block predicates are unavailable")
    }
}
struct Frame {
    position: [i32; 3],
    remaining: Option<i32>,
}
/// A depth-first cursor matching Java's nested placement callbacks. The caller
/// executes each feature before requesting the next position, using the same RNG.
pub struct PlacementPositions<'a> {
    feature: &'a PlacedFeature,
    min_y: i32,
    height: i32,
    stack: Vec<Frame>,
    done: bool,
    failed: bool,
}

// Biome.BIOME_INFO_NOISE is a single fixed-seed simplex, independent of the
// world's seed and the placement RNG. Its float result is widened to a double.
fn biome_info_noise(pos: [i32; 3], factor: f64) -> f64 {
    static NOISE: OnceLock<Simplex2D> = OnceLock::new();
    let noise = NOISE.get_or_init(|| Simplex2D::new(2345, true));
    f64::from(noise.sample(f64::from(pos[0]) / factor, f64::from(pos[2]) / factor))
}

impl PlacementPositions<'_> {
    pub fn next(
        &mut self,
        random: &mut DecorationRandom,
        env: &mut impl PlacementEnvironment,
    ) -> Result<Option<[i32; 3]>> {
        ensure!(!self.failed, "placement cursor previously failed");
        if self.done {
            return Ok(None);
        }
        self.failed = true;
        let result = self.advance(random, env);
        if result.is_ok() {
            self.failed = false;
        }
        result
    }
    fn advance(
        &mut self,
        random: &mut DecorationRandom,
        env: &mut impl PlacementEnvironment,
    ) -> Result<Option<[i32; 3]>> {
        if self.feature.modifiers.is_empty() {
            self.done = true;
            return Ok(Some(self.stack[0].position));
        }
        loop {
            let depth = self.stack.len() - 1;
            let Some(frame) = self.stack.last_mut() else {
                self.done = true;
                return Ok(None);
            };
            let position = frame.position;
            let output = match &self.feature.modifiers[depth] {
                modifier @ (Modifier::Count(_)
                | Modifier::NoiseThreshold { .. }
                | Modifier::NoiseBased { .. }) => {
                    let remaining = frame.remaining.get_or_insert_with(|| match modifier {
                        Modifier::Count(provider) => provider.sample(random),
                        Modifier::NoiseThreshold {
                            level,
                            below,
                            above,
                        } => {
                            if biome_info_noise(position, 200.0) < *level {
                                *below
                            } else {
                                *above
                            }
                        }
                        Modifier::NoiseBased {
                            ratio,
                            factor,
                            offset,
                        } => ((biome_info_noise(position, *factor) + offset) * f64::from(*ratio))
                            .ceil() as i32,
                        _ => unreachable!(),
                    });
                    if *remaining <= 0 {
                        None
                    } else {
                        *remaining -= 1;
                        Some(position)
                    }
                }
                modifier => {
                    if frame.remaining.is_some() {
                        None
                    } else {
                        frame.remaining = Some(0);
                        match modifier {
                            Modifier::InSquare => Some([
                                position[0].wrapping_add(random.next_int_bound(16).unwrap()),
                                position[1],
                                position[2].wrapping_add(random.next_int_bound(16).unwrap()),
                            ]),
                            Modifier::Height(provider) => Some([
                                position[0],
                                provider.sample(random, self.min_y, self.height)?,
                                position[2],
                            ]),
                            Modifier::Rarity(chance) => (random.next_float()
                                < 1.0f32 / (*chance as f32))
                                .then_some(position),
                            Modifier::Heightmap(kind) => {
                                let y = env.heightmap(kind, position[0], position[2])?;
                                (y > env.min_y()).then_some([position[0], y, position[2]])
                            }
                            Modifier::SurfaceRelativeThreshold(kind, min, max) => {
                                let height =
                                    i64::from(env.heightmap(kind, position[0], position[2])?);
                                (height + i64::from(*min) <= i64::from(position[1])
                                    && i64::from(position[1]) <= height + i64::from(*max))
                                .then_some(position)
                            }
                            Modifier::EnvironmentScan {
                                direction,
                                target,
                                allowed,
                                max_steps,
                            } => {
                                let mut p = position;
                                if !env.block_predicate(allowed, p)? {
                                    None
                                } else {
                                    let mut found = false;
                                    let mut outside = false;
                                    for _ in 0..*max_steps {
                                        if env.block_predicate(target, p)? {
                                            found = true;
                                            break;
                                        }
                                        p[1] = p[1].wrapping_add(*direction);
                                        if !env.inside_build_height(p[1])? {
                                            outside = true;
                                            break;
                                        }
                                        if !env.block_predicate(allowed, p)? {
                                            break;
                                        }
                                    }
                                    if !outside && (found || env.block_predicate(target, p)?) {
                                        Some(p)
                                    } else {
                                        None
                                    }
                                }
                            }
                            Modifier::Biome => env.biome_allows(position)?.then_some(position),
                            Modifier::BlockPredicate(predicate) => env
                                .block_predicate(predicate, position)?
                                .then_some(position),
                            Modifier::Offset(x, y, z) => Some([
                                position[0].wrapping_add(x.sample(random)),
                                position[1].wrapping_add(y.sample(random)),
                                position[2].wrapping_add(z.sample(random)),
                            ]),
                            Modifier::Count(_)
                            | Modifier::NoiseThreshold { .. }
                            | Modifier::NoiseBased { .. } => unreachable!(),
                        }
                    }
                }
            };
            if let Some(position) = output {
                if depth + 1 == self.feature.modifiers.len() {
                    return Ok(Some(position));
                }
                self.stack.push(Frame {
                    position,
                    remaining: None,
                });
            } else {
                self.stack.pop();
                if self.stack.is_empty() {
                    self.done = true;
                    return Ok(None);
                }
            }
        }
    }
}
