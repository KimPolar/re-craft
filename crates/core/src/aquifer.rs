//! The positional random centers used by Minecraft's noise-based aquifers.
//!
//! This is only the center selection step, not a fluid simulation.
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
    fn is_water(self) -> bool;
    fn is_lava(self) -> bool;
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
    use super::{AquiferBlock, AquiferLocator, FluidStatus, pressure, resolve};
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
}
