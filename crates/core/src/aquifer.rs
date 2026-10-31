//! Minecraft's noise-based aquifer center, fluid-level and boundary calculations.
//!
//! A caller supplies the density samplers, global picker and chunk sampling limit.
use std::collections::HashMap;

use crate::{
    normal_noise::{LegacyNoiseFactory, NoiseSeedFactory, NoiseSeedSource, XoroshiroNoiseFactory},
    random::{LegacyRandom, XoroshiroRandom},
};

enum Factory {
    Legacy(LegacyNoiseFactory),
    Xoroshiro(XoroshiroNoiseFactory),
}

/// Keep one locator per generated chunk so cached positions have a bounded lifetime.
pub struct AquiferLocator {
    factory: Factory,
    centers: HashMap<[i32; 3], [i32; 3]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AquiferCenter {
    pub grid: [i32; 3],
    pub position: [i32; 3],
    pub distance_squared: i32,
}

/// A block-state identity with the block checks needed by aquifers.
pub trait AquiferBlock: Copy + Eq {
    fn air() -> Self;
    fn lava() -> Self;
    fn is_air(self) -> bool;
    fn is_water(self) -> bool;
    fn is_lava(self) -> bool;
}

/// Density sources and global fluid picker used while calculating a center.
pub trait AquiferSampler {
    type Block: AquiferBlock;
    fn barrier(&mut self, x: i32, y: i32, z: i32) -> f32;
    fn global(&mut self, x: i32, y: i32, z: i32) -> FluidStatus<Self::Block>;
    fn surface(&mut self, x: i32, z: i32) -> f32;
    fn exclusion(&mut self, x: i32, y: i32, z: i32) -> f32;
    fn floodedness(&mut self, x: i32, y: i32, z: i32) -> f32;
    fn spread(&mut self, x: i32, y: i32, z: i32) -> f32;
    fn lava(&mut self, x: i32, y: i32, z: i32) -> f32;
}

/// A chunk-scoped aquifer. Keep the sampler's configuration fixed for its lifetime.
pub struct AquiferGenerator<T> {
    locator: AquiferLocator,
    levels: AquiferFluidLevels,
    statuses: HashMap<[i32; 3], FluidStatus<T>>,
    skip_sampling_above_y: i32,
}

/// Coordinates for the constructor's stepped surface volume, in Z/X order.
#[derive(Clone, Copy, Debug)]
pub struct AquiferSurfaceArea {
    pub min_x: i32,
    pub min_z: i32,
    pub size_x: u32,
    pub size_z: u32,
}

impl AquiferSurfaceArea {
    pub fn for_chunk(chunk_x: i32, chunk_z: i32) -> Option<Self> {
        fn axis(chunk: i32) -> Option<(i32, u32)> {
            let origin = chunk.checked_mul(16)?;
            let min_grid = origin.checked_sub(5)?.div_euclid(16);
            let max_grid = origin.checked_add(10)?.div_euclid(16).checked_add(1)?;
            let min = min_grid.checked_mul(16)?;
            let max = max_grid.checked_mul(16)?.checked_add(9)? & !3;
            Some((
                min,
                u32::try_from((i64::from(max) - i64::from(min)) / 4 + 1).ok()?,
            ))
        }
        let (min_x, size_x) = axis(chunk_x)?;
        let (min_z, size_z) = axis(chunk_z)?;
        Some(Self {
            min_x,
            min_z,
            size_x,
            size_z,
        })
    }
}

impl<T: AquiferBlock> AquiferGenerator<T> {
    pub fn new(seed: i64, legacy: bool, skip_sampling_above_y: i32) -> Self {
        Self {
            locator: AquiferLocator::new(seed, legacy),
            levels: AquiferFluidLevels::new(),
            statuses: HashMap::new(),
            skip_sampling_above_y,
        }
    }

    /// Initialize the skip threshold and cached surfaces from Java-order volume values.
    pub fn from_surface_volume(
        seed: i64,
        legacy: bool,
        area: AquiferSurfaceArea,
        values: &[f32],
    ) -> Result<Self, &'static str> {
        let count = (area.size_x as usize)
            .checked_mul(area.size_z as usize)
            .ok_or("surface size overflow")?;
        if area.size_x == 0 || area.size_z == 0 || values.len() != count {
            return Err("invalid surface volume size");
        }
        let mut generator = Self::new(seed, legacy, 0);
        let mut max = i32::MIN;
        for z in 0..area.size_z {
            for x in 0..area.size_x {
                let px = i32::try_from(i64::from(area.min_x) + i64::from(x) * 4)
                    .map_err(|_| "surface X overflow")?;
                let pz = i32::try_from(i64::from(area.min_z) + i64::from(z) * 4)
                    .map_err(|_| "surface Z overflow")?;
                let height = java_floor(f64::from(
                    values[(z as usize) * (area.size_x as usize) + x as usize],
                ));
                generator.levels.surfaces.insert((px, pz), height);
                max = max.max(height);
            }
        }
        generator.skip_sampling_above_y = max
            .wrapping_add(20)
            .div_euclid(12)
            .wrapping_sub(1)
            .wrapping_mul(12)
            .wrapping_add(10);
        Ok(generator)
    }

    pub fn sample<S: AquiferSampler<Block = T>>(
        &mut self,
        sampler: &mut S,
        x: i32,
        y: i32,
        z: i32,
        density: f64,
    ) -> AquiferResult<T> {
        if density > 0.0 {
            return AquiferResult {
                block: None,
                schedule_fluid_update: false,
            };
        }
        let global = sampler.global(x, y, z);
        let current = global.at(y);
        if y > self.skip_sampling_above_y || current.is_lava() {
            return AquiferResult {
                block: Some(current),
                schedule_fluid_update: false,
            };
        }
        let centers = self.locator.nearest(x, y, z);
        let distances = centers.map(|c| c.distance_squared);
        // resolve invokes callbacks sequentially; RefCell shares the sampler without
        // precomputing centers or density functions that an early branch won't use.
        let sampler = std::cell::RefCell::new(sampler);
        resolve(
            y,
            density,
            self.skip_sampling_above_y,
            distances,
            &mut |at| {
                if at == y {
                    global
                } else {
                    sampler.borrow_mut().global(x, at, z)
                }
            },
            &mut |i| {
                let center = centers[i];
                *self.statuses.entry(center.grid).or_insert_with(|| {
                    self.levels.compute(
                        &mut **sampler.borrow_mut(),
                        center.position[0],
                        center.position[1],
                        center.position[2],
                    )
                })
            },
            &mut || sampler.borrow_mut().barrier(x, y, z),
        )
    }
}

/// Vanilla's DimensionType.WAY_BELOW_MIN_Y for the 26.3 packed Y range.
pub const WAY_BELOW_MIN_Y: i32 = -32_512;

/// Scoped to a chunk; cached surface samples must not outlive their sampler.
pub struct AquiferFluidLevels {
    surfaces: HashMap<(i32, i32), i32>,
}

impl Default for AquiferFluidLevels {
    fn default() -> Self {
        Self::new()
    }
}

impl AquiferFluidLevels {
    pub fn new() -> Self {
        Self {
            surfaces: HashMap::new(),
        }
    }

    pub fn compute<S: AquiferSampler>(
        &mut self,
        sampler: &mut S,
        x: i32,
        y: i32,
        z: i32,
    ) -> FluidStatus<S::Block> {
        const OFFSETS: [(i32, i32); 13] = [
            (0, 0),
            (-2, -1),
            (-1, -1),
            (0, -1),
            (1, -1),
            (-3, 0),
            (-2, 0),
            (-1, 0),
            (1, 0),
            (-2, 1),
            (-1, 1),
            (0, 1),
            (1, 1),
        ];
        let global = sampler.global(x, y, z);
        let mut min_surface = i32::MAX;
        let upper = y.wrapping_add(12);
        let lower = y.wrapping_sub(12);
        let mut center_flooded = false;
        for (dx, dz) in OFFSETS {
            let sx = x.wrapping_add(dx.wrapping_mul(16));
            let sz = z.wrapping_add(dz.wrapping_mul(16));
            let key = (sx & !3, sz & !3);
            let surface = *self
                .surfaces
                .entry(key)
                .or_insert_with(|| java_floor(f64::from(sampler.surface(key.0, key.1))));
            let adjusted = surface.wrapping_add(8);
            let center = dx == 0 && dz == 0;
            if center && lower > adjusted {
                return global;
            }
            let near_surface = upper > adjusted;
            if near_surface || center {
                let nearby = sampler.global(sx, adjusted, sz);
                if !nearby.at(adjusted).is_air() {
                    if center {
                        center_flooded = true;
                    }
                    if near_surface {
                        return nearby;
                    }
                }
            }
            min_surface = min_surface.min(surface);
        }
        let level =
            compute_surface_level(sampler, x, y, z, global.level, min_surface, center_flooded);
        let mut block = global.block;
        if level <= -10 && level != WAY_BELOW_MIN_Y && block != S::Block::lava() {
            let noise = sampler.lava(x.div_euclid(64), y.div_euclid(40), z.div_euclid(64));
            if f64::from(noise).abs() > 0.3 {
                block = S::Block::lava();
            }
        }
        FluidStatus { level, block }
    }
}

fn java_floor(value: f64) -> i32 {
    let truncated = value as i32;
    if value < f64::from(truncated) {
        truncated.wrapping_sub(1)
    } else {
        truncated
    }
}

fn compute_surface_level<S: AquiferSampler>(
    sampler: &mut S,
    x: i32,
    y: i32,
    z: i32,
    global_level: i32,
    min_surface: i32,
    center_flooded: bool,
) -> i32 {
    let (low, high) = if sampler.exclusion(x, y, z) > 0.0 {
        (-1.0, -1.0)
    } else {
        let height = min_surface.wrapping_add(8).wrapping_sub(y);
        let influence = if center_flooded {
            let fraction = f64::from(height) / 64.0;
            if fraction < 0.0 {
                1.0
            } else if fraction > 1.0 {
                0.0
            } else {
                1.0 + fraction * (0.0 - 1.0)
            }
        } else {
            0.0
        };
        let flooded = f64::from(sampler.floodedness(x, y, z)).clamp(-1.0, 1.0);
        // Mth.map(value, 1, 0, from, to) performs inverse lerp then lerp.
        let t = (influence - 1.0) / (0.0 - 1.0);
        let high_threshold = -0.3 + t * (0.8 - (-0.3));
        let low_threshold = -0.8 + t * (0.4 - (-0.8));
        (flooded - low_threshold, flooded - high_threshold)
    };
    if high > 0.0 {
        return global_level;
    }
    if low <= 0.0 || low.is_nan() {
        return WAY_BELOW_MIN_Y;
    }
    let gx = x.div_euclid(16);
    let gy = y.div_euclid(40);
    let gz = z.div_euclid(16);
    let base = gy.wrapping_mul(40).wrapping_add(20);
    let value = sampler.spread(gx, gy, gz) * 10.0;
    let quantized = java_floor(f64::from(value) / 3.0).wrapping_mul(3);
    min_surface.min(base.wrapping_add(quantized))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FluidStatus<T> {
    pub level: i32,
    pub block: T,
}

impl<T: AquiferBlock> FluidStatus<T> {
    pub fn at(self, y: i32) -> T {
        if y < self.level { self.block } else { T::air() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AquiferResult<T> {
    /// `None` means the density selects the dimension's default solid block.
    pub block: Option<T>,
    pub schedule_fluid_update: bool,
}

pub fn similarity(first: i32, second: i32) -> f64 {
    1.0 - f64::from(second.wrapping_sub(first)) / 25.0
}

/// The barrier noise is sampled only if needed and shared across pressure pairs.
pub fn pressure<T: AquiferBlock>(
    y: i32,
    first: FluidStatus<T>,
    second: FluidStatus<T>,
    cached_barrier: &mut f64,
    barrier: &mut impl FnMut() -> f32,
) -> f64 {
    let a = first.at(y);
    let b = second.at(y);
    if (a.is_lava() && b.is_water()) || (a.is_water() && b.is_lava()) {
        return 2.0;
    }
    let difference = first.level.wrapping_sub(second.level).wrapping_abs();
    if difference == 0 {
        return 0.0;
    }
    let middle = 0.5 * f64::from(first.level.wrapping_add(second.level));
    let offset = f64::from(y) + 0.5 - middle;
    let half_difference = f64::from(difference) / 2.0;
    let gap = half_difference - offset.abs();
    let slope = if offset > 0.0 {
        let value = 0.0 + gap;
        if value > 0.0 {
            value / 1.5
        } else {
            value / 2.5
        }
    } else {
        let value = 3.0 + gap;
        if value > 0.0 {
            value / 3.0
        } else {
            value / 10.0
        }
    };
    let noise = if !(-2.0..=2.0).contains(&slope) {
        0.0
    } else {
        if cached_barrier.is_nan() {
            *cached_barrier = f64::from(barrier());
        }
        *cached_barrier
    };
    2.0 * (noise + slope)
}

/// Resolve the local fluid after the four nearest centers have been identified.
/// Status and barrier callbacks are lazy so unchanged or distant cells need no
/// extra density samples. Distances must be in nearest-first order.
pub fn resolve<T: AquiferBlock>(
    y: i32,
    density: f64,
    skip_sampling_above_y: i32,
    distances: [i32; 4],
    global: &mut impl FnMut(i32) -> FluidStatus<T>,
    local: &mut impl FnMut(usize) -> FluidStatus<T>,
    barrier: &mut impl FnMut() -> f32,
) -> AquiferResult<T> {
    let solid = || AquiferResult {
        block: None,
        schedule_fluid_update: false,
    };
    let fluid = |block, schedule_fluid_update| AquiferResult {
        block: Some(block),
        schedule_fluid_update,
    };
    if density > 0.0 {
        return solid();
    }
    let global_here = global(y).at(y);
    if y > skip_sampling_above_y || global_here.is_lava() {
        return fluid(global_here, false);
    }
    let first = local(0);
    let result = first.at(y);
    let s01 = similarity(distances[0], distances[1]);
    let threshold = similarity(100, 144);
    if s01 <= 0.0 {
        let schedule = s01 >= threshold && first != local(1);
        return fluid(result, schedule);
    }
    let below = y.wrapping_sub(1);
    if result.is_water() && global(below).at(below).is_lava() {
        return fluid(result, true);
    }
    let mut cached_barrier = f64::NAN;
    let second = local(1);
    let p01 = s01 * pressure(y, first, second, &mut cached_barrier, barrier);
    if density + p01 > 0.0 {
        return solid();
    }
    let third = local(2);
    let s02 = similarity(distances[0], distances[2]);
    if s02 > 0.0 {
        let p02 = s01 * s02 * pressure(y, first, third, &mut cached_barrier, barrier);
        if density + p02 > 0.0 {
            return solid();
        }
    }
    let s12 = similarity(distances[1], distances[2]);
    if s12 > 0.0 {
        let p12 = s01 * s12 * pressure(y, second, third, &mut cached_barrier, barrier);
        if density + p12 > 0.0 {
            return solid();
        }
    }
    let schedule = first != second
        || (s12 >= threshold && second != third)
        || (s02 >= threshold && first != third)
        || (s02 >= threshold
            && similarity(distances[0], distances[3]) >= threshold
            && first != local(3));
    fluid(result, schedule)
}

impl AquiferLocator {
    pub fn new(seed: i64, legacy: bool) -> Self {
        let factory = if legacy {
            let root = LegacyRandom::new(seed).fork_noise_factory();
            Factory::Legacy(root.random_for("minecraft:aquifer").fork_noise_factory())
        } else {
            let root = XoroshiroRandom::new(seed).fork_noise_factory();
            Factory::Xoroshiro(root.random_for("minecraft:aquifer").fork_noise_factory())
        };
        Self {
            factory,
            centers: HashMap::new(),
        }
    }

    pub fn center(&mut self, grid: [i32; 3]) -> [i32; 3] {
        *self.centers.entry(grid).or_insert_with(|| {
            let [x, y, z] = grid;
            let (dx, dy, dz) = match &self.factory {
                Factory::Legacy(factory) => {
                    let mut random = factory.at(x, y, z);
                    (
                        random.next_int_bound(10).unwrap(),
                        random.next_int_bound(9).unwrap(),
                        random.next_int_bound(10).unwrap(),
                    )
                }
                Factory::Xoroshiro(factory) => {
                    let mut random = factory.at(x, y, z);
                    (
                        random.next_int_bound(10).unwrap(),
                        random.next_int_bound(9).unwrap(),
                        random.next_int_bound(10).unwrap(),
                    )
                }
            };
            // Java stores each center in a packed BlockPos: X/Z use 26 bits, Y 12.
            [
                (x.wrapping_mul(16).wrapping_add(dx) << 6) >> 6,
                (y.wrapping_mul(12).wrapping_add(dy) << 20) >> 20,
                (z.wrapping_mul(16).wrapping_add(dz) << 6) >> 6,
            ]
        })
    }

    /// Returns nearest first; a later center wins ties just as in Java.
    pub fn nearest(&mut self, x: i32, y: i32, z: i32) -> [AquiferCenter; 4] {
        let base = [
            x.wrapping_sub(5) >> 4,
            y.wrapping_add(1).div_euclid(12),
            z.wrapping_sub(5) >> 4,
        ];
        let empty = AquiferCenter {
            grid: [0; 3],
            position: [0; 3],
            distance_squared: i32::MAX,
        };
        let mut nearest = [empty; 4];
        for dx in 0..=1 {
            for dy in -1i32..=1 {
                for dz in 0..=1 {
                    let grid = [
                        base[0].wrapping_add(dx),
                        base[1].wrapping_add(dy),
                        base[2].wrapping_add(dz),
                    ];
                    let position = self.center(grid);
                    let delta = [
                        position[0].wrapping_sub(x),
                        position[1].wrapping_sub(y),
                        position[2].wrapping_sub(z),
                    ];
                    let distance_squared = delta
                        .into_iter()
                        .fold(0i32, |sum, d| sum.wrapping_add(d.wrapping_mul(d)));
                    if let Some(index) = nearest
                        .iter()
                        .position(|old| old.distance_squared >= distance_squared)
                    {
                        for i in (index + 1..4).rev() {
                            nearest[i] = nearest[i - 1];
                        }
                        nearest[index] = AquiferCenter {
                            grid,
                            position,
                            distance_squared,
                        };
                    }
                }
            }
        }
        nearest
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AquiferBlock, AquiferGenerator, AquiferLocator, AquiferSampler, FluidStatus, pressure,
        resolve,
    };
    use std::cell::Cell;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Block {
        Air,
        Water,
        Lava,
    }
    impl AquiferBlock for Block {
        fn air() -> Self {
            Self::Air
        }
        fn lava() -> Self {
            Self::Lava
        }
        fn is_air(self) -> bool {
            self == Self::Air
        }
        fn is_water(self) -> bool {
            self == Self::Water
        }
        fn is_lava(self) -> bool {
            self == Self::Lava
        }
    }

    #[test]
    fn known_legacy_centers_and_order() {
        let mut locator = AquiferLocator::new(0, true);
        assert_eq!(locator.center([-3, -6, 2]), [-47, -69, 33]);
        let nearest = locator.nearest(-32, -64, 48);
        assert_eq!(
            nearest.map(|center| center.position),
            [
                [-41, -60, 52],
                [-30, -56, 56],
                [-24, -71, 53],
                [-48, -67, 49],
            ]
        );
        assert!(
            nearest
                .windows(2)
                .all(|pair| pair[0].distance_squared <= pair[1].distance_squared)
        );
    }

    #[test]
    fn pressure_shares_barrier_and_water_above_lava_schedules_update() {
        let water = FluidStatus {
            level: 10,
            block: Block::Water,
        };
        let lava = FluidStatus {
            level: 10,
            block: Block::Lava,
        };
        let air = FluidStatus {
            level: 8,
            block: Block::Air,
        };
        let samples = Cell::new(0);
        let mut barrier = || {
            samples.set(samples.get() + 1);
            0.25
        };
        let mut cache = f64::NAN;
        assert_eq!(pressure(9, water, lava, &mut cache, &mut barrier), 2.0);
        assert_eq!(samples.get(), 0);
        pressure(9, water, air, &mut cache, &mut barrier);
        pressure(9, water, air, &mut cache, &mut barrier);
        assert_eq!(samples.get(), 1);

        let result = resolve(
            9,
            -1.0,
            100,
            [0, 1, 2, 3],
            &mut |y| if y == 8 { lava } else { water },
            &mut |_| water,
            &mut barrier,
        );
        assert_eq!(result.block, Some(Block::Water));
        assert!(result.schedule_fluid_update);
        assert_eq!(samples.get(), 1);
    }

    #[test]
    fn repeated_samples_reuse_center_status_and_surface() {
        struct Sampler {
            surfaces: usize,
        }
        impl AquiferSampler for Sampler {
            type Block = Block;
            fn barrier(&mut self, _: i32, _: i32, _: i32) -> f32 {
                0.0
            }
            fn global(&mut self, _: i32, _: i32, _: i32) -> FluidStatus<Block> {
                FluidStatus {
                    level: 63,
                    block: Block::Water,
                }
            }
            fn surface(&mut self, _: i32, _: i32) -> f32 {
                self.surfaces += 1;
                80.0
            }
            fn exclusion(&mut self, _: i32, _: i32, _: i32) -> f32 {
                0.0
            }
            fn floodedness(&mut self, _: i32, _: i32, _: i32) -> f32 {
                0.0
            }
            fn spread(&mut self, _: i32, _: i32, _: i32) -> f32 {
                0.0
            }
            fn lava(&mut self, _: i32, _: i32, _: i32) -> f32 {
                0.0
            }
        }
        let mut sampler = Sampler { surfaces: 0 };
        let mut generator = AquiferGenerator::new(1234, false, 200);
        let first = generator.sample(&mut sampler, 0, 0, 0, -1.0);
        let initial = sampler.surfaces;
        assert!(initial > 0);
        assert_eq!(generator.sample(&mut sampler, 0, 0, 0, -1.0), first);
        assert_eq!(sampler.surfaces, initial);
    }

    #[test]
    fn chunk_surface_volume_sets_skip_and_primed_cache() {
        let area = super::AquiferSurfaceArea::for_chunk(0, -1).unwrap();
        assert_eq!(
            (area.min_x, area.min_z, area.size_x, area.size_z),
            (-16, -32, 11, 11)
        );
        let values = vec![10.75; (area.size_x * area.size_z) as usize];
        let generator =
            super::AquiferGenerator::<Block>::from_surface_volume(0, false, area, &values).unwrap();
        assert_eq!(generator.skip_sampling_above_y, 22);
        assert_eq!(generator.levels.surfaces.len(), 121);
        assert_eq!(generator.levels.surfaces[&(-16, -32)], 10);
        assert!(
            super::AquiferGenerator::<Block>::from_surface_volume(0, false, area, &values[..120])
                .is_err()
        );
    }
}
