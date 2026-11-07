//! Shared 26.3 speleothem column placement for cluster and individual features.
use crate::{packs::ResourceId, placed_feature::IntProvider};
use anyhow::{Context, Result, bail, ensure};
use native_core::random::DecorationRandom;
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerticalDirection {
    Up,
    Down,
}
impl VerticalDirection {
    fn step(self) -> i32 {
        match self {
            Self::Up => 1,
            Self::Down => -1,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Thickness {
    Base,
    Middle,
    Frustum,
    Tip,
    TipMerge,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Column {
    pub floor: Option<i32>,
    pub ceiling: Option<i32>,
}

/// Match Column.scan with the cluster's empty-or-water and inverse predicates.
/// Both directions start by testing the origin, and the range is exclusive.
pub fn scan_column(
    start: [i32; 3],
    range: i32,
    empty_or_water: &mut impl FnMut([i32; 3]) -> Result<bool>,
) -> Result<Option<Column>> {
    if !empty_or_water(start)? {
        return Ok(None);
    }
    let mut scan = |step: i32| -> Result<Option<i32>> {
        let mut pos = start;
        for _ in 1..range {
            if !empty_or_water(pos)? {
                break;
            }
            pos[1] = pos[1].wrapping_add(step);
        }
        Ok((!empty_or_water(pos)?).then_some(pos[1]))
    };
    let ceiling = scan(1)?;
    let floor = scan(-1)?;
    Ok(Some(Column { floor, ceiling }))
}

/// Cluster edge probability uses Java's float clamped-map before widening.
pub fn edge_chance(
    radius_x: i32,
    radius_z: i32,
    x: i32,
    z: i32,
    max_edge: i32,
    minimum: f32,
) -> f64 {
    let edge = radius_x
        .wrapping_sub(x.wrapping_abs())
        .min(radius_z.wrapping_sub(z.wrapping_abs()));
    let t = edge as f32 / max_edge as f32;
    let result = if t < 0.0 {
        minimum
    } else if t > 1.0 {
        1.0
    } else {
        minimum + t * (1.0 - minimum)
    };
    f64::from(result)
}

/// Draw the cluster height with the same float casts and Gaussian cache as Java.
pub fn column_height(
    random: &mut DecorationRandom,
    x: i32,
    z: i32,
    density: f32,
    height: i32,
    center_distance: i32,
    deviation: i32,
) -> i32 {
    if random.next_float() > density {
        return 0;
    }
    let distance = x.wrapping_abs().wrapping_add(z.wrapping_abs());
    let t = f64::from(distance) / f64::from(center_distance);
    let half = f64::from(height) / 2.0;
    let mean = (if t < 0.0 {
        half
    } else if t > 1.0 {
        0.0
    } else {
        half + t * -half
    }) as f32;
    let value = mean + (random.next_gaussian() as f32) * deviation as f32;
    let value = if value < 0.0 {
        0.0
    } else if value > height as f32 {
        height as f32
    } else {
        value
    };
    value as i32
}

/// The caller resolves the base block, pointed block, replaceable holder set,
/// fluid tags and flag-2 writes from one unchanged data-pack snapshot.
pub trait SpeleothemWorld {
    type State;
    fn block(&mut self, position: [i32; 3]) -> Result<Self::State>;
    fn is_base(&mut self, state: &Self::State) -> Result<bool>;
    fn is_replaceable(&mut self, state: &Self::State) -> Result<bool>;
    fn is_water_at(&mut self, position: [i32; 3]) -> Result<bool>;
    fn set_base(&mut self, position: [i32; 3]) -> Result<()>;
    fn set_pointed(
        &mut self,
        position: [i32; 3],
        direction: VerticalDirection,
        thickness: Thickness,
        waterlogged: bool,
    ) -> Result<()>;
}

/// Additional level queries and writes needed by the cluster feature.
pub trait ClusterWorld: SpeleothemWorld {
    fn is_empty_or_water(&mut self, state: &Self::State) -> Result<bool>;
    fn is_lava(&mut self, state: &Self::State) -> Result<bool>;
    /// Match the data-pack block and fluid tags used by Java's pool test.
    fn can_place_pool(&mut self, position: [i32; 3]) -> Result<bool>;
    fn set_water(&mut self, position: [i32; 3]) -> Result<()>;
}

/// Sampling is supplied by the decoded providers from the active data packs.
pub trait ClusterNumbers {
    fn height(&self, random: &mut DecorationRandom) -> i32;
    fn wetness(&self, random: &mut DecorationRandom) -> f32;
    fn density(&self, random: &mut DecorationRandom) -> f32;
    fn radius(&self, random: &mut DecorationRandom) -> i32;
    fn layer(&self, random: &mut DecorationRandom) -> i32;
}

#[derive(Clone, Copy, Debug)]
pub struct ClusterConfig {
    pub search_range: i32,
    pub height_difference: i32,
    pub height_deviation: i32,
    pub edge_chance: f32,
    pub edge_distance: i32,
    pub center_distance: i32,
}

#[derive(Clone, Debug)]
enum FloatProvider {
    Constant(f32),
    Uniform(f32, f32),
    Trapezoid(f32, f32, f32),
    ClampedNormal {
        mean: f32,
        deviation: f32,
        min: f32,
        max: f32,
    },
}

impl FloatProvider {
    fn parse(value: &Value) -> Result<Self> {
        let number = |value: &Value| -> Result<f32> {
            let n = value.as_f64().context("invalid float provider value")? as f32;
            ensure!(n.is_finite(), "invalid float provider value");
            Ok(n)
        };
        if value.is_number() {
            return Ok(Self::Constant(number(value)?));
        }
        let field = |name: &str| -> Result<f32> {
            number(
                value
                    .get(name)
                    .with_context(|| format!("missing float provider {name}"))?,
            )
        };
        let id = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("missing float provider type")?,
        )?;
        ensure!(
            id.namespace == "minecraft",
            "unsupported float provider namespace"
        );
        Ok(match id.path.as_str() {
            "constant" => Self::Constant(field("value")?),
            "uniform" => {
                let (min, max) = (field("min_inclusive")?, field("max_exclusive")?);
                ensure!(min < max, "invalid uniform float range");
                Self::Uniform(min, max)
            }
            "trapezoid" => {
                let (min, max, plateau) = (field("min")?, field("max")?, field("plateau")?);
                ensure!(
                    min <= max && plateau <= max - min,
                    "invalid trapezoid float range"
                );
                Self::Trapezoid(min, max, plateau)
            }
            "clamped_normal" => {
                let (mean, deviation, min, max) = (
                    field("mean")?,
                    field("deviation")?,
                    field("min")?,
                    field("max")?,
                );
                ensure!(min <= max, "invalid clamped normal range");
                Self::ClampedNormal {
                    mean,
                    deviation,
                    min,
                    max,
                }
            }
            _ => bail!("unsupported speleothem float provider"),
        })
    }
    fn bounds(&self) -> (f32, f32) {
        match *self {
            Self::Constant(v) => (v, v),
            Self::Uniform(a, b) | Self::Trapezoid(a, b, _) => (a, b),
            Self::ClampedNormal { min, max, .. } => (min, max),
        }
    }
    fn sample(&self, random: &mut DecorationRandom) -> f32 {
        match *self {
            Self::Constant(v) => v,
            Self::Uniform(a, b) => a + random.next_float() * (b - a),
            Self::Trapezoid(a, b, p) => {
                let left = (b - a - p) / 2.0;
                let right = b - a - left;
                a + random.next_float() * right + random.next_float() * left
            }
            Self::ClampedNormal {
                mean,
                deviation,
                min,
                max,
            } => {
                let value = mean + (random.next_gaussian() as f32) * deviation;
                if value < min {
                    min
                } else if value > max {
                    max
                } else {
                    value
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
enum Replaceable {
    Tag(ResourceId),
    Blocks(Vec<ResourceId>),
}

/// Decoded 26.3 cluster definition, with state and holder references retained
/// for the caller's data-pack-aware world adapter.
pub struct SpeleothemClusterFeature<S> {
    pub config: ClusterConfig,
    base: S,
    pointed: S,
    base_id: ResourceId,
    pointed_id: ResourceId,
    replaceable: Replaceable,
    height: IntProvider,
    radius: IntProvider,
    layer: IntProvider,
    density: FloatProvider,
    wetness: FloatProvider,
}

impl<S> SpeleothemClusterFeature<S> {
    pub fn decode(value: &Value, resolve: &mut impl FnMut(&Value) -> Result<S>) -> Result<Self> {
        let id = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("missing speleothem feature type")?,
        )?;
        ensure!(
            id.namespace == "minecraft" && id.path == "speleothem_cluster",
            "wrong speleothem feature type"
        );
        let field = |name: &str| {
            value
                .get(name)
                .with_context(|| format!("missing speleothem {name}"))
        };
        let state_id = |state: &Value| -> Result<ResourceId> {
            ResourceId::parse(
                state
                    .as_str()
                    .or_else(|| state.get("id").and_then(Value::as_str))
                    .context("missing speleothem block ID")?,
            )
        };
        let base_value = field("base_block")?;
        let pointed_value = field("pointed_block")?;
        let base_id = state_id(base_value)?;
        let pointed_id = state_id(pointed_value)?;
        let base = resolve(base_value)?;
        let pointed = resolve(pointed_value)?;
        let raw = field("replaceable_blocks")?;
        let replaceable = if let Some(tag) = raw.as_str().and_then(|s| s.strip_prefix('#')) {
            Replaceable::Tag(ResourceId::parse(tag)?)
        } else {
            let names = if let Some(name) = raw.as_str() {
                vec![name]
            } else {
                raw.as_array()
                    .context("invalid replaceable blocks")?
                    .iter()
                    .map(|v| v.as_str().context("invalid replaceable block"))
                    .collect::<Result<Vec<_>>>()?
            };
            Replaceable::Blocks(
                names
                    .into_iter()
                    .map(ResourceId::parse)
                    .collect::<Result<_>>()?,
            )
        };
        let integer = |name: &str, lo: i32, hi: i32| -> Result<i32> {
            let n = i32::try_from(
                field(name)?
                    .as_i64()
                    .context("invalid speleothem integer")?,
            )?;
            ensure!((lo..=hi).contains(&n), "speleothem integer out of range");
            Ok(n)
        };
        let provider = |name: &str, lo: i32, hi: i32| -> Result<IntProvider> {
            let p = IntProvider::parse(field(name)?, 0)?;
            let (min, max) = p.bounds();
            ensure!(min >= lo && max <= hi, "speleothem provider out of range");
            Ok(p)
        };
        let float_provider = |name: &str| -> Result<FloatProvider> {
            let p = FloatProvider::parse(field(name)?)?;
            let (min, max) = p.bounds();
            ensure!(
                min >= 0.0 && max <= 2.0,
                "speleothem float provider out of range"
            );
            Ok(p)
        };
        let edge_chance = field("chance_of_speleothem_at_max_distance_from_center")?
            .as_f64()
            .context("invalid edge chance")? as f32;
        ensure!(
            edge_chance.is_finite() && (0.0..=1.0).contains(&edge_chance),
            "invalid edge chance"
        );
        Ok(Self {
            config: ClusterConfig {
                search_range: integer("floor_to_ceiling_search_range", 1, 512)?,
                height_difference: integer("max_stalagmite_stalactite_height_diff", 0, 64)?,
                height_deviation: integer("height_deviation", 1, 64)?,
                edge_chance,
                edge_distance: integer(
                    "max_distance_from_edge_affecting_chance_of_speleothem",
                    1,
                    64,
                )?,
                center_distance: integer("max_distance_from_center_affecting_height_bias", 1, 64)?,
            },
            base,
            pointed,
            base_id,
            pointed_id,
            replaceable,
            height: provider("height", 1, 128)?,
            radius: provider("radius", 1, 128)?,
            layer: provider("speleothem_block_layer_thickness", 0, 128)?,
            density: float_provider("density")?,
            wetness: float_provider("wetness")?,
        })
    }
    pub fn base(&self) -> &S {
        &self.base
    }
    pub fn pointed(&self) -> &S {
        &self.pointed
    }
    pub fn base_id(&self) -> &ResourceId {
        &self.base_id
    }
    pub fn pointed_id(&self) -> &ResourceId {
        &self.pointed_id
    }
    pub fn replaceable_tag(&self) -> Option<&ResourceId> {
        if let Replaceable::Tag(tag) = &self.replaceable {
            Some(tag)
        } else {
            None
        }
    }
    pub fn replaceable_blocks(&self) -> Option<&[ResourceId]> {
        if let Replaceable::Blocks(ids) = &self.replaceable {
            Some(ids)
        } else {
            None
        }
    }
    pub fn required_tags(&self) -> BTreeSet<ResourceId> {
        let mut tags =
            BTreeSet::from([ResourceId::parse("minecraft:base_stone_overworld").unwrap()]);
        if let Some(tag) = self.replaceable_tag() {
            tags.insert(tag.clone());
        }
        tags
    }
    pub fn required_blocks(&self) -> BTreeSet<ResourceId> {
        let mut blocks = BTreeSet::from([
            self.base_id.clone(),
            self.pointed_id.clone(),
            ResourceId::parse("minecraft:water").unwrap(),
        ]);
        if let Some(ids) = self.replaceable_blocks() {
            blocks.extend(ids.iter().cloned());
        }
        blocks
    }
}

impl<S> ClusterNumbers for SpeleothemClusterFeature<S> {
    fn height(&self, r: &mut DecorationRandom) -> i32 {
        self.height.sample(r)
    }
    fn wetness(&self, r: &mut DecorationRandom) -> f32 {
        self.wetness.sample(r)
    }
    fn density(&self, r: &mut DecorationRandom) -> f32 {
        self.density.sample(r)
    }
    fn radius(&self, r: &mut DecorationRandom) -> i32 {
        self.radius.sample(r)
    }
    fn layer(&self, r: &mut DecorationRandom) -> i32 {
        self.layer.sample(r)
    }
}

impl ClusterConfig {
    /// Apply the Java feature in its X-then-Z order to an unpublished draft.
    /// The caller must discard its world and RNG when an operation errors.
    pub fn place(
        &self,
        world: &mut impl ClusterWorld,
        numbers: &impl ClusterNumbers,
        random: &mut DecorationRandom,
        origin: [i32; 3],
    ) -> Result<bool> {
        let state = world.block(origin)?;
        if !world.is_empty_or_water(&state)? {
            return Ok(false);
        }
        let height = numbers.height(random);
        let wetness = numbers.wetness(random);
        let density = numbers.density(random);
        let rx = numbers.radius(random);
        let rz = numbers.radius(random);
        anyhow::ensure!(rx >= 0 && rz >= 0, "invalid speleothem radius");
        for x in -rx..=rx {
            for z in -rz..=rz {
                let chance = edge_chance(rx, rz, x, z, self.edge_distance, self.edge_chance);
                let position = [
                    origin[0].wrapping_add(x),
                    origin[1],
                    origin[2].wrapping_add(z),
                ];
                self.place_column(
                    world,
                    numbers,
                    random,
                    position,
                    [x, z],
                    wetness,
                    chance,
                    height,
                    density,
                )?;
            }
        }
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    fn place_column(
        &self,
        world: &mut impl ClusterWorld,
        numbers: &impl ClusterNumbers,
        random: &mut DecorationRandom,
        position: [i32; 3],
        offset: [i32; 2],
        wetness: f32,
        chance: f64,
        height: i32,
        density: f32,
    ) -> Result<()> {
        let column = scan_column(position, self.search_range, &mut |pos| {
            let state = world.block(pos)?;
            world.is_empty_or_water(&state)
        })?;
        let Some(Column { floor, ceiling }) = column else {
            return Ok(());
        };
        if floor.is_none() && ceiling.is_none() {
            return Ok(());
        }
        let mut floor = floor;
        let wet = random.next_float() < wetness;
        if wet
            && let Some(y) = floor
            && world.can_place_pool([position[0], y, position[2]])?
        {
            world.set_water([position[0], y, position[2]])?;
            floor = Some(y.wrapping_sub(1));
        }
        let upper_choice = random.next_double() < chance;
        let mut upper = 0;
        if let Some(y) = ceiling
            && upper_choice
        {
            let pos = [position[0], y, position[2]];
            let state = world.block(pos)?;
            if !world.is_lava(&state)? {
                replace_base_layer(world, pos, VerticalDirection::Up, numbers.layer(random))?;
                let limit = floor.map_or(height, |f| height.min(y.wrapping_sub(f)));
                upper = column_height(
                    random,
                    offset[0],
                    offset[1],
                    density,
                    limit,
                    self.center_distance,
                    self.height_deviation,
                );
            }
        }
        let lower_choice = random.next_double() < chance;
        let mut lower = 0;
        if let Some(y) = floor
            && lower_choice
        {
            let pos = [position[0], y, position[2]];
            let state = world.block(pos)?;
            if !world.is_lava(&state)? {
                replace_base_layer(world, pos, VerticalDirection::Down, numbers.layer(random))?;
                lower = if ceiling.is_some() {
                    let low = self.height_difference.wrapping_neg();
                    let width = self.height_difference.wrapping_sub(low).wrapping_add(1);
                    let offset = random
                        .next_int_bound(width)
                        .context("invalid height difference")?
                        .wrapping_add(low);
                    upper.wrapping_add(offset).max(0)
                } else {
                    column_height(
                        random,
                        offset[0],
                        offset[1],
                        density,
                        height,
                        self.center_distance,
                        self.height_deviation,
                    )
                };
            }
        }
        if let (Some(f), Some(c)) = (floor, ceiling)
            && c.wrapping_sub(upper) <= f.wrapping_add(lower)
        {
            let minimum = c.wrapping_sub(upper).max(f.wrapping_add(1));
            let maximum = f.wrapping_add(lower).min(c.wrapping_sub(1));
            let width = maximum
                .wrapping_add(1)
                .wrapping_sub(minimum)
                .wrapping_add(1);
            let split = random
                .next_int_bound(width)
                .context("invalid speleothem overlap")?
                .wrapping_add(minimum);
            upper = c.wrapping_sub(split);
            lower = split.wrapping_sub(1).wrapping_sub(f);
        }
        let merge = random.next_bool()
            && upper > 0
            && lower > 0
            && matches!((floor, ceiling), (Some(f),Some(c)) if upper.wrapping_add(lower) == c.wrapping_sub(f).wrapping_sub(1));
        if let Some(y) = ceiling {
            grow(
                world,
                [position[0], y.wrapping_sub(1), position[2]],
                VerticalDirection::Down,
                upper,
                merge,
            )?;
        }
        if let Some(y) = floor {
            grow(
                world,
                [position[0], y.wrapping_add(1), position[2]],
                VerticalDirection::Up,
                lower,
                merge,
            )?;
        }
        Ok(())
    }
}

/// Java tests the replaceable holder before writing the default base state.
pub fn place_base_if_possible(
    world: &mut impl SpeleothemWorld,
    position: [i32; 3],
) -> Result<bool> {
    let current = world.block(position)?;
    if !world.is_replaceable(&current)? {
        return Ok(false);
    }
    world.set_base(position)?;
    Ok(true)
}

/// Place from the supporting block toward the tip, even through occupied
/// positions, and determine waterlogging independently at each position.
pub fn grow(
    world: &mut impl SpeleothemWorld,
    start: [i32; 3],
    direction: VerticalDirection,
    height: i32,
    merge: bool,
) -> Result<()> {
    let step = direction.step();
    let support = [start[0], start[1].wrapping_sub(step), start[2]];
    let state = world.block(support)?;
    if !world.is_base(&state)? {
        return Ok(());
    }
    for index in 0..height {
        let thickness = if height >= 3 && index == 0 {
            Thickness::Base
        } else if index < height - 2 {
            Thickness::Middle
        } else if index == height - 2 {
            Thickness::Frustum
        } else if merge {
            Thickness::TipMerge
        } else {
            Thickness::Tip
        };
        let position = [
            start[0],
            start[1].wrapping_add(index.wrapping_mul(step)),
            start[2],
        ];
        let waterlogged = world.is_water_at(position)?;
        world.set_pointed(position, direction, thickness, waterlogged)?;
    }
    Ok(())
}

/// Place a contiguous base layer, stopping at the first non-replaceable block.
pub fn replace_base_layer(
    world: &mut impl SpeleothemWorld,
    start: [i32; 3],
    direction: VerticalDirection,
    count: i32,
) -> Result<()> {
    for index in 0..count {
        let position = [
            start[0],
            start[1].wrapping_add(index.wrapping_mul(direction.step())),
            start[2],
        ];
        if !place_base_if_possible(world, position)? {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cluster_codec_rejects_out_of_range_providers_and_retains_tag() {
        let mut value = json!({
            "type": "minecraft:speleothem_cluster",
            "base_block": "minecraft:sulfur", "pointed_block": "minecraft:sulfur_spike",
            "replaceable_blocks": "#minecraft:sulfur_spike_replaceable_blocks",
            "floor_to_ceiling_search_range": 12, "height": 4, "radius": 2,
            "max_stalagmite_stalactite_height_diff": 1, "height_deviation": 3,
            "speleothem_block_layer_thickness": 2, "density": 0.7, "wetness": 0.0,
            "chance_of_speleothem_at_max_distance_from_center": 0.1,
            "max_distance_from_edge_affecting_chance_of_speleothem": 3,
            "max_distance_from_center_affecting_height_bias": 8
        });
        let decode =
            |value: &Value| SpeleothemClusterFeature::decode(value, &mut |v| Ok(v.clone()));
        let decoded = decode(&value).unwrap();
        assert_eq!(decoded.required_tags().len(), 2);
        assert!(decoded.replaceable_blocks().is_none());
        value["radius"] =
            json!({"type":"minecraft:uniform", "min_inclusive":1, "max_inclusive":129});
        assert!(decode(&value).is_err());
        value["radius"] = json!(2);
        value["density"] =
            json!({"type":"minecraft:uniform", "min_inclusive":0.1, "max_exclusive":2.1});
        assert!(decode(&value).is_err());
        value["density"] = json!(0.5);
        value["floor_to_ceiling_search_range"] = json!(0);
        assert!(decode(&value).is_err());
    }
}
