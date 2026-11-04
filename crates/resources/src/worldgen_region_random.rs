//! WorldGenRegion's random source and the common animal spawn-finalization steps.
//!
//! Species-specific initialization must run before `finalize_animal_base`.
//! This does not construct, finalize completely, or store an entity.

use native_core::{
    normal_noise::{NoiseSeedFactory, NoiseSeedSource},
    random::{LegacyRandom, XoroshiroRandom},
};

use crate::density::WorldgenRandom;

/// The independent random source owned by one Java WorldGenRegion instance.
/// Keep it alive for every operation in that region; do not recreate it for
/// each animal or substitute the chunk spawner's decoration random source.
#[derive(Clone, Debug)]
pub enum RegionRandom {
    Legacy(LegacyRandom),
    Xoroshiro(XoroshiroRandom),
}

impl RegionRandom {
    pub fn new(seed: i64, algorithm: WorldgenRandom, chunk_x: i32, chunk_z: i32) -> Self {
        let x = chunk_x.wrapping_mul(16);
        let z = chunk_z.wrapping_mul(16);
        const NAME: &str = "minecraft:worldgen_region_random";
        match algorithm {
            WorldgenRandom::Legacy => {
                let root = LegacyRandom::new(seed).fork_noise_factory();
                let factory = root.random_for(NAME).fork_noise_factory();
                Self::Legacy(factory.at(x, 0, z))
            }
            WorldgenRandom::Xoroshiro => {
                let root = XoroshiroRandom::new(seed).fork_noise_factory();
                let factory = root.random_for(NAME).fork_noise_factory();
                Self::Xoroshiro(factory.at(x, 0, z))
            }
        }
    }

    pub fn next_float(&mut self) -> f32 {
        match self {
            Self::Legacy(random) => random.next_float(),
            Self::Xoroshiro(random) => random.next_float(),
        }
    }

    pub fn next_double(&mut self) -> f64 {
        match self {
            Self::Legacy(random) => random.next_double(),
            Self::Xoroshiro(random) => random.next_double(),
        }
    }

    pub fn next_int(&mut self) -> i32 {
        match self {
            Self::Legacy(random) => random.next_int(),
            Self::Xoroshiro(random) => random.next_int(),
        }
    }

    pub fn next_int_bound(&mut self, bound: i32) -> Option<i32> {
        match self {
            Self::Legacy(random) => random.next_int_bound(bound),
            Self::Xoroshiro(random) => random.next_int_bound(bound),
        }
    }

    pub fn next_bool(&mut self) -> bool {
        match self {
            Self::Legacy(random) => random.next_bool(),
            Self::Xoroshiro(random) => random.next_bool(),
        }
    }

    pub fn next_long(&mut self) -> i64 {
        match self {
            Self::Legacy(random) => random.next_long(),
            Self::Xoroshiro(random) => random.next_long(),
        }
    }
}

/// The AgeableMob portion of a species' potentially richer group data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimalGroupData {
    pub size: i32,
    pub should_spawn_baby: bool,
    pub baby_spawn_chance: f32,
}

impl Default for AnimalGroupData {
    fn default() -> Self {
        Self {
            size: 0,
            should_spawn_baby: true,
            baby_spawn_chance: 0.05,
        }
    }
}

/// Values changed by AgeableMob.finalizeSpawn and Mob.finalizeSpawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimalBaseInitialization {
    /// When true, call the species' `getBabyStartAge` and set its age.
    pub set_baby_age: bool,
    /// A newly added permanent ADD_MULTIPLIED_BASE modifier, if absent before.
    pub follow_range_bonus: Option<f64>,
    pub left_handed: bool,
}

/// Apply only the common superclass steps, after species-specific logic.
/// The caller owns the actual group data, random source and entity attributes.
pub fn finalize_animal_base(
    random: &mut RegionRandom,
    group: &mut AnimalGroupData,
    has_follow_range_bonus: bool,
) -> AnimalBaseInitialization {
    let set_baby_age =
        group.should_spawn_baby && group.size > 0 && random.next_float() <= group.baby_spawn_chance;
    group.size = group.size.wrapping_add(1);
    let follow_range_bonus = if has_follow_range_bonus {
        None
    } else {
        Some(0.0 + 0.11485000000000001 * (random.next_double() - random.next_double()))
    };
    let left_handed = random.next_float() < 0.05_f32;
    AnimalBaseInitialization {
        set_baby_age,
        follow_range_bonus,
        left_handed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_animal_skips_baby_draw_and_existing_bonus_skips_double_draws() {
        for algorithm in [WorldgenRandom::Legacy, WorldgenRandom::Xoroshiro] {
            let mut random = RegionRandom::new(42, algorithm, -2, 7);
            let mut expected = random.clone();
            let mut group = AnimalGroupData::default();
            let first = finalize_animal_base(&mut random, &mut group, true);
            assert!(!first.set_baby_age);
            assert_eq!(first.follow_range_bonus, None);
            assert_eq!(first.left_handed, expected.next_float() < 0.05);
            assert_eq!(group.size, 1);
            group.baby_spawn_chance = 1.0;
            let second = finalize_animal_base(&mut random, &mut group, false);
            assert!(expected.next_float() <= 1.0);
            let bonus =
                0.0 + 0.11485000000000001 * (expected.next_double() - expected.next_double());
            assert_eq!(second.follow_range_bonus, Some(bonus));
            assert!(second.set_baby_age);
            assert_eq!(second.left_handed, expected.next_float() < 0.05);
            assert_eq!(group.size, 2);
            assert_eq!(random.next_long(), expected.next_long());
        }
    }
}
