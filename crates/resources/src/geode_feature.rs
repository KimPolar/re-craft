//! The geometry and ordered world effects of the Java 26.3 geode feature.
//!
//! The caller supplies block-state providers and the actual world effects. An
//! error must discard the enclosing generation attempt, including its RNG.
use crate::{
    disk_feature::{DiskWorld, Provider},
    packs::ResourceId,
    placed_feature::IntProvider,
};
use anyhow::{Context, Result, ensure};
use native_core::{
    normal_noise::{NormalNoiseParameters, Normalization},
    random::{DecorationRandom, LegacyRandom},
};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeodeLayer {
    Filling,
    Inner,
    AlternateInner,
    Middle,
    Outer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Down,
    Up,
    North,
    South,
    West,
    East,
}
const DIRECTIONS: [(Direction, [i32; 3]); 6] = [
    (Direction::Down, [0, -1, 0]),
    (Direction::Up, [0, 1, 0]),
    (Direction::North, [0, 0, -1]),
    (Direction::South, [0, 0, 1]),
    (Direction::West, [-1, 0, 0]),
    (Direction::East, [1, 0, 0]),
];

/// Block providers and all world state use the active data pack and registry.
pub trait GeodeWorld {
    type State: Clone;
    fn block(&mut self, pos: [i32; 3]) -> Result<Self::State>;
    fn is_air_or_invalid(&mut self, state: &Self::State) -> Result<bool>;
    fn may_replace(&mut self, state: &Self::State) -> Result<bool>;
    fn layer_state(
        &mut self,
        layer: GeodeLayer,
        pos: [i32; 3],
        random: &mut DecorationRandom,
    ) -> Result<Self::State>;
    fn air(&mut self) -> Result<Self::State>;
    fn set_block(&mut self, pos: [i32; 3], state: &Self::State) -> Result<()>;
    /// Schedule a zero-delay tick if the position contains any fluid.
    fn schedule_fluid(&mut self, pos: [i32; 3]) -> Result<()>;
    fn placement_count(&self) -> usize;
    fn placement_state(
        &mut self,
        index: usize,
        direction: Direction,
        existing: &Self::State,
    ) -> Result<Self::State>;
    fn can_grow(&mut self, state: &Self::State) -> Result<bool>;
}

/// Integer-provider sampling is delegated to retain data-pack and RNG semantics.
pub trait GeodeNumbers {
    fn distribution_points(&self, random: &mut DecorationRandom) -> i32;
    fn outer_wall_distance(&self, random: &mut DecorationRandom) -> i32;
    fn outer_wall_max(&self) -> i32;
    fn point_offset(&self, random: &mut DecorationRandom) -> i32;
}

#[derive(Clone, Debug)]
pub struct GeodeShape {
    pub filling: f64,
    pub inner: f64,
    pub middle: f64,
    pub outer: f64,
    pub crack_chance: f64,
    pub crack_size: f64,
    pub crack_point_offset: i32,
    pub alternate_chance: f64,
    pub placement_chance: f64,
    pub placement_requires_alternate: bool,
    pub min_offset: i32,
    pub max_offset: i32,
    pub noise_multiplier: f64,
    pub invalid_threshold: i32,
}

/// World effects that depend on the active block registry and chunk stage.
pub trait GeodeEnvironment: DiskWorld {
    fn set_geode_block(&mut self, pos: [i32; 3], state: &Self::State) -> Result<()>;
    fn schedule_geode_fluid(&mut self, pos: [i32; 3]) -> Result<()>;
    fn geode_placement_state(
        &mut self,
        state: &Self::State,
        direction: Direction,
        existing: &Self::State,
    ) -> Result<Self::State>;
    fn geode_can_grow(&mut self, state: &Self::State) -> Result<bool>;
}

#[derive(Clone)]
enum BlockSet {
    Tag(ResourceId),
    Blocks(Vec<ResourceId>),
}
impl BlockSet {
    fn decode(value: &Value) -> Result<Self> {
        if let Some(s) = value.as_str() {
            return if let Some(tag) = s.strip_prefix('#') {
                Ok(Self::Tag(ResourceId::parse(tag)?))
            } else {
                Ok(Self::Blocks(vec![ResourceId::parse(s)?]))
            };
        }
        let list = value.as_array().context("invalid geode block set")?;
        ensure!(list.len() <= 4096, "too many geode block entries");
        Ok(Self::Blocks(
            list.iter()
                .map(|v| ResourceId::parse(v.as_str().context("invalid geode block")?))
                .collect::<Result<_>>()?,
        ))
    }
    fn contains<W: DiskWorld>(&self, world: &mut W, state: &W::State) -> Result<bool> {
        match self {
            Self::Tag(tag) => world.in_tag(state, tag),
            Self::Blocks(ids) => {
                for id in ids {
                    if world.block_is(state, id)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }
    fn tags(&self, tags: &mut BTreeSet<ResourceId>) {
        if let Self::Tag(id) = self {
            tags.insert(id.clone());
        }
    }
    fn blocks(&self, blocks: &mut BTreeSet<ResourceId>) {
        if let Self::Blocks(ids) = self {
            blocks.extend(ids.iter().cloned());
        }
    }
}

struct Numbers {
    wall: IntProvider,
    points: IntProvider,
    offset: IntProvider,
}
impl GeodeNumbers for Numbers {
    fn distribution_points(&self, random: &mut DecorationRandom) -> i32 {
        self.points.sample(random)
    }
    fn outer_wall_distance(&self, random: &mut DecorationRandom) -> i32 {
        self.wall.sample(random)
    }
    fn outer_wall_max(&self) -> i32 {
        self.wall.bounds().1
    }
    fn point_offset(&self, random: &mut DecorationRandom) -> i32 {
        self.offset.sample(random)
    }
}

/// A configured geode decoded from the active data-pack feature definition.
pub struct GeodeFeature<S> {
    shape: GeodeShape,
    numbers: Numbers,
    layers: [Provider<S>; 5],
    placements: Vec<S>,
    air: S,
    cannot_replace: BlockSet,
    invalid_blocks: BlockSet,
}
impl<S: Clone + Eq> GeodeFeature<S> {
    pub fn decode(value: &Value, resolve: &mut impl FnMut(&Value) -> Result<S>) -> Result<Self> {
        let id = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("missing geode type")?,
        )?;
        ensure!(
            id.namespace == "minecraft" && id.path == "geode",
            "wrong geode type"
        );
        let blocks = value
            .get("blocks")
            .and_then(Value::as_object)
            .context("missing geode blocks")?;
        let layers = value
            .get("layers")
            .and_then(Value::as_object)
            .context("missing geode layers")?;
        let crack = value
            .get("crack")
            .and_then(Value::as_object)
            .context("missing geode crack")?;
        let number = |obj: &serde_json::Map<String, Value>,
                      name: &str,
                      default: f64,
                      min: f64,
                      max: f64|
         -> Result<f64> {
            let n = match obj.get(name) {
                Some(v) => v.as_f64().with_context(|| format!("invalid {name}"))?,
                None => default,
            };
            ensure!(n.is_finite() && (min..=max).contains(&n), "invalid {name}");
            Ok(n)
        };
        let integer = |obj: &serde_json::Map<String, Value>,
                       name: &str,
                       default: Option<i32>|
         -> Result<i32> {
            match obj.get(name) {
                Some(v) => Ok(i32::try_from(
                    v.as_i64().with_context(|| format!("invalid {name}"))?,
                )?),
                None => default.with_context(|| format!("missing {name}")),
            }
        };
        let obj = value.as_object().context("invalid geode feature")?;
        let shape = GeodeShape {
            filling: number(layers, "filling", 1.7, 0.01, 50.0)?,
            inner: number(layers, "inner_layer", 2.2, 0.01, 50.0)?,
            middle: number(layers, "middle_layer", 3.2, 0.01, 50.0)?,
            outer: number(layers, "outer_layer", 4.2, 0.01, 50.0)?,
            crack_chance: number(crack, "generate_crack_chance", 1.0, 0.0, 1.0)?,
            crack_size: number(crack, "base_crack_size", 2.0, 0.0, 5.0)?,
            crack_point_offset: integer(crack, "crack_point_offset", Some(2))?,
            alternate_chance: number(obj, "use_alternate_layer0_chance", 0.0, 0.0, 1.0)?,
            placement_chance: number(obj, "use_potential_placements_chance", 0.35, 0.0, 1.0)?,
            placement_requires_alternate: match obj.get("placements_require_layer0_alternate") {
                Some(v) => v.as_bool().context("invalid placement requirement")?,
                None => true,
            },
            min_offset: integer(obj, "min_gen_offset", Some(-16))?,
            max_offset: integer(obj, "max_gen_offset", Some(16))?,
            noise_multiplier: number(obj, "noise_multiplier", 0.05, 0.0, 1.0)?,
            invalid_threshold: integer(obj, "invalid_blocks_threshold", None)?,
        };
        ensure!(
            (0..=10).contains(&shape.crack_point_offset),
            "invalid crack point offset"
        );
        let parse_int =
            |name: &str, default: IntProvider, min: i32, max: i32| -> Result<IntProvider> {
                let provider = match obj.get(name) {
                    Some(v) => IntProvider::parse(v, 0)?,
                    None => default,
                };
                let (a, b) = provider.bounds();
                ensure!(a >= min && b <= max && a <= b, "invalid {name} bounds");
                Ok(provider)
            };
        let numbers = Numbers {
            wall: parse_int("outer_wall_distance", IntProvider::Uniform(4, 5), 1, 20)?,
            points: parse_int("distribution_points", IntProvider::Uniform(3, 4), 1, 20)?,
            offset: parse_int("point_offset", IntProvider::Uniform(1, 2), 0, 10)?,
        };
        let get = |name: &str| {
            blocks
                .get(name)
                .with_context(|| format!("missing geode {name}"))
        };
        let layers = [
            Provider::decode(get("filling_provider")?, resolve, 0)?,
            Provider::decode(get("inner_layer_provider")?, resolve, 0)?,
            Provider::decode(get("alternate_inner_layer_provider")?, resolve, 0)?,
            Provider::decode(get("middle_layer_provider")?, resolve, 0)?,
            Provider::decode(get("outer_layer_provider")?, resolve, 0)?,
        ];
        let entries = get("inner_placements")?
            .as_array()
            .context("invalid geode placements")?;
        ensure!(
            !entries.is_empty() && entries.len() <= 4096,
            "invalid geode placement count"
        );
        let placements = entries
            .iter()
            .map(&mut *resolve)
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            shape,
            numbers,
            layers,
            placements,
            air: resolve(&json!({"id":"minecraft:air"}))?,
            cannot_replace: BlockSet::decode(get("cannot_replace")?)?,
            invalid_blocks: BlockSet::decode(get("invalid_blocks")?)?,
        })
    }
    pub fn required_tags(&self) -> BTreeSet<ResourceId> {
        let mut out = BTreeSet::new();
        self.cannot_replace.tags(&mut out);
        self.invalid_blocks.tags(&mut out);
        for p in &self.layers {
            p.tags(&mut out);
        }
        out
    }
    pub fn required_blocks(&self) -> BTreeSet<ResourceId> {
        let mut out = BTreeSet::new();
        self.cannot_replace.blocks(&mut out);
        self.invalid_blocks.blocks(&mut out);
        for p in &self.layers {
            p.blocks(&mut out);
        }
        out
    }
    pub fn needs_survival(&self, name: &str) -> bool {
        self.layers.iter().any(|p| p.needs_survival(name))
    }
    pub fn place<W: GeodeEnvironment<State = S>>(
        &self,
        world: &mut W,
        random: &mut DecorationRandom,
        world_seed: i64,
        origin: [i32; 3],
    ) -> Result<bool> {
        let mut adapter = Adapter {
            feature: self,
            world,
        };
        place_geode(
            &mut adapter,
            random,
            world_seed,
            origin,
            &self.shape,
            &self.numbers,
        )
    }
}
struct Adapter<'a, S, W> {
    feature: &'a GeodeFeature<S>,
    world: &'a mut W,
}
impl<S: Clone + Eq, W: GeodeEnvironment<State = S>> GeodeWorld for Adapter<'_, S, W> {
    type State = S;
    fn block(&mut self, p: [i32; 3]) -> Result<S> {
        self.world.block(p)
    }
    fn is_air_or_invalid(&mut self, s: &S) -> Result<bool> {
        Ok(self.world.is_air(s)? || self.feature.invalid_blocks.contains(self.world, s)?)
    }
    fn may_replace(&mut self, s: &S) -> Result<bool> {
        Ok(!self.feature.cannot_replace.contains(self.world, s)?)
    }
    fn layer_state(&mut self, l: GeodeLayer, p: [i32; 3], r: &mut DecorationRandom) -> Result<S> {
        let i = match l {
            GeodeLayer::Filling => 0,
            GeodeLayer::Inner => 1,
            GeodeLayer::AlternateInner => 2,
            GeodeLayer::Middle => 3,
            GeodeLayer::Outer => 4,
        };
        self.feature.layers[i]
            .state(self.world, r, p)?
            .context("geode provider returned no state")
    }
    fn air(&mut self) -> Result<S> {
        Ok(self.feature.air.clone())
    }
    fn set_block(&mut self, p: [i32; 3], s: &S) -> Result<()> {
        self.world.set_geode_block(p, s)
    }
    fn schedule_fluid(&mut self, p: [i32; 3]) -> Result<()> {
        self.world.schedule_geode_fluid(p)
    }
    fn placement_count(&self) -> usize {
        self.feature.placements.len()
    }
    fn placement_state(&mut self, i: usize, d: Direction, s: &S) -> Result<S> {
        self.world
            .geode_placement_state(&self.feature.placements[i], d, s)
    }
    fn can_grow(&mut self, s: &S) -> Result<bool> {
        self.world.geode_can_grow(s)
    }
}

fn offset(p: [i32; 3], d: [i32; 3]) -> [i32; 3] {
    [
        p[0].wrapping_add(d[0]),
        p[1].wrapping_add(d[1]),
        p[2].wrapping_add(d[2]),
    ]
}
fn distance(a: [i32; 3], b: [i32; 3]) -> f64 {
    let x = f64::from(a[0]) - f64::from(b[0]);
    let y = f64::from(a[1]) - f64::from(b[1]);
    let z = f64::from(a[2]) - f64::from(b[2]);
    x * x + y * y + z * z
}
fn safe_set<W: GeodeWorld>(world: &mut W, pos: [i32; 3], state: &W::State) -> Result<()> {
    let existing = world.block(pos)?;
    if world.may_replace(&existing)? {
        world.set_block(pos, state)?;
    }
    Ok(())
}

/// Generate one geode. The world seed initializes noise independently of the
/// decoration RNG; early invalid-block rejection still consumes its draws.
pub fn place_geode<W: GeodeWorld>(
    world: &mut W,
    random: &mut DecorationRandom,
    world_seed: i64,
    origin: [i32; 3],
    shape: &GeodeShape,
    numbers: &impl GeodeNumbers,
) -> Result<bool> {
    ensure!(numbers.outer_wall_max() > 0, "invalid outer wall maximum");
    ensure!(
        world.placement_count() > 0 && world.placement_count() <= i32::MAX as usize,
        "invalid geode placement count"
    );
    // Keep pathological pack ranges from causing overflow or an unbounded job.
    ensure!(
        shape.min_offset <= shape.max_offset
            && i64::from(shape.max_offset) - i64::from(shape.min_offset) <= 128,
        "invalid geode generation range"
    );
    let count = numbers.distribution_points(random);
    ensure!((1..=20).contains(&count), "invalid geode point count");
    let noise = NormalNoiseParameters {
        base_amplitude: 1.0,
        base_octave: -4,
        octave_count: 1,
        normalization: Normalization::Legacy,
        amplitude_modifiers: vec![],
    }
    .create(&mut LegacyRandom::new(world_seed))
    .map_err(anyhow::Error::msg)?;
    let adjustment = f64::from(count) / f64::from(numbers.outer_wall_max());
    let filling = 1.0 / shape.filling.sqrt();
    let inner = 1.0 / (shape.inner + adjustment).sqrt();
    let middle = 1.0 / (shape.middle + adjustment).sqrt();
    let outer = 1.0 / (shape.outer + adjustment).sqrt();
    let crack_threshold = 1.0
        / (shape.crack_size
            + random.next_double() / 2.0
            + if count > 3 { adjustment } else { 0.0 })
        .sqrt();
    let crack = f64::from(random.next_float()) < shape.crack_chance;
    let mut points = Vec::with_capacity(count as usize);
    let mut invalid = 0;
    for _ in 0..count {
        let p = offset(
            origin,
            [
                numbers.outer_wall_distance(random),
                numbers.outer_wall_distance(random),
                numbers.outer_wall_distance(random),
            ],
        );
        let state = world.block(p)?;
        if world.is_air_or_invalid(&state)? {
            invalid += 1;
            if invalid > shape.invalid_threshold {
                return Ok(false);
            }
        }
        points.push((p, numbers.point_offset(random)));
    }
    let mut cracks = Vec::new();
    if crack {
        let direction = random.next_int_bound(4).unwrap();
        let d = count * 2 + 1;
        let (x, z) = match direction {
            0 => (d, 0),
            1 => (0, d),
            2 => (d, d),
            _ => (0, 0),
        };
        for y in [7, 5, 1] {
            cracks.push(offset(origin, [x, y, z]));
        }
    }
    let mut placements = Vec::new();
    // BlockPos.betweenClosed iterates x first, then y, then z.
    for z in shape.min_offset..=shape.max_offset {
        for y in shape.min_offset..=shape.max_offset {
            for x in shape.min_offset..=shape.max_offset {
                let p = offset(origin, [x, y, z]);
                let perturb =
                    f64::from(noise.sample(f64::from(p[0]), f64::from(p[1]), f64::from(p[2])))
                        * shape.noise_multiplier;
                let density = points.iter().fold(0.0, |v, (q, n)| {
                    v + (1.0 / (distance(p, *q) + f64::from(*n)).sqrt() + perturb)
                });
                if density < outer {
                    continue;
                }
                if density >= filling {
                    let state = world.layer_state(GeodeLayer::Filling, p, random)?;
                    safe_set(world, p, &state)?;
                    continue;
                }
                let crack_density = cracks.iter().fold(0.0, |v, q| {
                    v + (1.0 / (distance(p, *q) + f64::from(shape.crack_point_offset)).sqrt()
                        + perturb)
                });
                if crack && crack_density >= crack_threshold {
                    let air = world.air()?;
                    safe_set(world, p, &air)?;
                    for (_, d) in DIRECTIONS {
                        world.schedule_fluid(offset(p, d))?;
                    }
                    continue;
                }
                let layer = if density >= inner {
                    let alternate = f64::from(random.next_float()) < shape.alternate_chance;
                    let layer = if alternate {
                        GeodeLayer::AlternateInner
                    } else {
                        GeodeLayer::Inner
                    };
                    let state = world.layer_state(layer, p, random)?;
                    safe_set(world, p, &state)?;
                    if (!shape.placement_requires_alternate || alternate)
                        && f64::from(random.next_float()) < shape.placement_chance
                    {
                        placements.push(p);
                    }
                    continue;
                } else if density >= middle {
                    GeodeLayer::Middle
                } else {
                    GeodeLayer::Outer
                };
                let state = world.layer_state(layer, p, random)?;
                safe_set(world, p, &state)?;
            }
        }
    }
    for p in placements {
        let choice = random
            .next_int_bound(world.placement_count() as i32)
            .unwrap() as usize;
        for (direction, d) in DIRECTIONS {
            let q = offset(p, d);
            let existing = world.block(q)?;
            let state = world.placement_state(choice, direction, &existing)?;
            if world.can_grow(&existing)? {
                safe_set(world, q, &state)?;
                break;
            }
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definition() -> Value {
        json!({
            "type":"minecraft:geode",
            "blocks":{
                "filling_provider":{"id":"minecraft:air"},
                "inner_layer_provider":{"id":"minecraft:amethyst_block"},
                "alternate_inner_layer_provider":{"id":"minecraft:budding_amethyst"},
                "middle_layer_provider":{"id":"minecraft:calcite"},
                "outer_layer_provider":{"id":"minecraft:smooth_basalt"},
                "inner_placements":["minecraft:amethyst_cluster"],
                "cannot_replace":"#minecraft:features_cannot_replace",
                "invalid_blocks":["minecraft:bedrock","minecraft:water"]
            },
            "layers":{}, "crack":{}, "invalid_blocks_threshold":1
        })
    }
    fn resolve(v: &Value) -> Result<String> {
        Ok(v.as_str()
            .or_else(|| v.get("id").and_then(Value::as_str))
            .context("missing block")?
            .into())
    }
    #[test]
    fn decode_defaults_tags_and_integer_provider_bounds() {
        let feature = GeodeFeature::decode(&definition(), &mut resolve).unwrap();
        assert_eq!(feature.shape.filling, 1.7);
        assert_eq!(feature.shape.crack_chance, 1.0);
        assert_eq!(feature.shape.placement_chance, 0.35);
        assert_eq!(feature.numbers.wall.bounds(), (4, 5));
        assert_eq!(feature.numbers.points.bounds(), (3, 4));
        assert_eq!(feature.numbers.offset.bounds(), (1, 2));
        assert_eq!(
            feature.required_tags(),
            BTreeSet::from([ResourceId::parse("minecraft:features_cannot_replace").unwrap()])
        );
        assert_eq!(
            feature.required_blocks(),
            BTreeSet::from([
                ResourceId::parse("minecraft:bedrock").unwrap(),
                ResourceId::parse("minecraft:water").unwrap()
            ])
        );
        let mut bad = definition();
        bad["outer_wall_distance"] =
            json!({"type":"minecraft:uniform","min_inclusive":0,"max_inclusive":6});
        assert!(GeodeFeature::decode(&bad, &mut resolve).is_err());
        let mut bad = definition();
        bad["crack"]["crack_point_offset"] = json!(11);
        assert!(GeodeFeature::decode(&bad, &mut resolve).is_err());
        let mut bad = definition();
        bad["blocks"]["inner_placements"] = json!([]);
        assert!(GeodeFeature::decode(&bad, &mut resolve).is_err());
    }
}
