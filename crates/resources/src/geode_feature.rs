//! The geometry and ordered world effects of the Java 26.3 geode feature.
//!
//! The caller supplies block-state providers and the actual world effects. An
//! error must discard the enclosing generation attempt, including its RNG.
use anyhow::{Result, ensure};
use native_core::{
    normal_noise::{NormalNoiseParameters, Normalization},
    random::{DecorationRandom, LegacyRandom},
};

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
