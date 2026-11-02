//! Placement modifiers for data-pack features. Feature execution itself is
//! separate: callers must consume the same random source between positions.
use anyhow::{Context, Result, bail, ensure};
use native_core::random::DecorationRandom;
use serde_json::Value;

use crate::packs::{ResourceId, ResourceProvider};

#[derive(Clone, Debug)]
enum IntProvider {
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
impl IntProvider {
    fn parse(value: &Value, depth: u8) -> Result<Self> {
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
    fn bounds(&self) -> (i32, i32) {
        match self {
            Self::Constant(v) => (*v, *v),
            Self::Uniform(a, b) | Self::Biased(a, b) | Self::Trapezoid(a, b, _) => (*a, *b),
            Self::Weighted(items, _) => items
                .iter()
                .map(|(_, p)| p.bounds())
                .fold((i32::MAX, i32::MIN), |(a, b), (c, d)| (a.min(c), b.max(d))),
        }
    }
    fn sample(&self, random: &mut DecorationRandom) -> i32 {
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
        }
    }
}

#[derive(Clone, Debug)]
enum Modifier {
    Count(IntProvider),
    InSquare,
    Height(HeightProvider),
    Rarity(i32),
    Heightmap(String),
    Biome,
    Offset(IntProvider, IntProvider, IntProvider),
}

/// A decoded placed feature. Unknown modifiers or providers are rejected.
pub struct PlacedFeature {
    pub feature: Value,
    modifiers: Vec<Modifier>,
}
impl PlacedFeature {
    /// Each codec decode creates a distinct uniform/trapezoid height-provider
    /// object in 26.3. Since HeightRangePlacement is a record containing that
    /// object, separately decoded placed features cannot be equal when this
    /// modifier is present, even if their JSON is identical.
    pub fn has_independent_height_provider(&self) -> bool {
        self.modifiers.iter().any(|modifier| {
            matches!(
                modifier,
                Modifier::Height(HeightProvider::Uniform(..) | HeightProvider::Trapezoid(..))
            )
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
                "in_square" => Modifier::InSquare,
                "height_range" => Modifier::Height(HeightProvider::parse(
                    entry.get("height").context("missing height")?,
                )?),
                "rarity_filter" => {
                    let chance = integer(entry, "chance")?;
                    ensure!(chance > 0, "invalid rarity");
                    Modifier::Rarity(chance)
                }
                "heightmap" => {
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
                    Modifier::Heightmap(name.to_owned())
                }
                "biome" => Modifier::Biome,
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
    /// Java's first available Y in this heightmap.
    fn heightmap(&mut self, kind: &str, x: i32, z: i32) -> Result<i32>;
    /// The environment checks the top-level placed feature at this position.
    fn biome_allows(&mut self, position: [i32; 3]) -> Result<bool>;
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
                Modifier::Count(provider) => {
                    let remaining = frame
                        .remaining
                        .get_or_insert_with(|| provider.sample(random));
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
                            Modifier::Biome => env.biome_allows(position)?.then_some(position),
                            Modifier::Offset(x, y, z) => Some([
                                position[0].wrapping_add(x.sample(random)),
                                position[1].wrapping_add(y.sample(random)),
                                position[2].wrapping_add(z.sample(random)),
                            ]),
                            Modifier::Count(_) => unreachable!(),
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
