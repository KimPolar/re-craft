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
    use super::AquiferLocator;

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
}
