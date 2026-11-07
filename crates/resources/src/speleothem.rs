//! Shared 26.3 speleothem column placement for cluster and individual features.
use anyhow::{Context, Result};
use native_core::random::DecorationRandom;

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
