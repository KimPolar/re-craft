//! WorldGenRegion's random source and the common animal spawn-finalization steps.
//!
//! Species-specific initialization precedes `finalize_animal_base`.
//! These functions do not construct or store an entity.

use anyhow::{Context, Result, ensure};
use native_core::{
    normal_noise::{NoiseSeedFactory, NoiseSeedSource},
    random::{LegacyRandom, XoroshiroRandom},
};
use std::collections::{BTreeMap, BTreeSet};

use crate::{
    density::WorldgenRandom,
    packs::{ResourceId, ResourceProvider},
    surface_biomes,
};

/// The active world's farm-animal variant biome tags, including pack overrides.
#[derive(Clone)]
pub struct SheepBiomeTags {
    registry: BTreeSet<ResourceId>,
    warm: BTreeSet<ResourceId>,
    cold: BTreeSet<ResourceId>,
}

impl SheepBiomeTags {
    pub fn load(provider: &mut impl ResourceProvider) -> Result<Self> {
        let registry: BTreeSet<_> = provider.list_data("worldgen/biome")?.into_iter().collect();
        ensure!(!registry.is_empty(), "biome registry is empty");
        let mut cache = BTreeMap::new();
        let mut resolve = |name: &str| -> Result<BTreeSet<ResourceId>> {
            let id = ResourceId::parse(name)?;
            Ok(surface_biomes::resolve_tag(
                provider,
                &id,
                &registry,
                &mut cache,
                &mut BTreeSet::new(),
            )?
            .unwrap_or_default())
        };
        let warm = resolve("minecraft:spawns_warm_variant_farm_animals")
            .context("cannot resolve warm farm-animal biome tag")?;
        let cold = resolve("minecraft:spawns_cold_variant_farm_animals")
            .context("cannot resolve cold farm-animal biome tag")?;
        Ok(Self {
            registry,
            warm,
            cold,
        })
    }

    pub fn membership(&self, biome: &ResourceId) -> Result<(bool, bool)> {
        ensure!(
            self.registry.contains(biome),
            "biome is absent from the active registry"
        );
        Ok((self.warm.contains(biome), self.cold.contains(biome)))
    }
}

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

/// The group state created by Horse.finalizeSpawn. The first horse selects a
/// coat shared by the group; markings and attributes are drawn per horse.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HorseGroupData {
    pub animal: AnimalGroupData,
    pub variant: Option<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HorseInitialization {
    pub variant: u8,
    pub markings: u8,
    pub max_health: f32,
    pub movement_speed: f64,
    pub jump_strength: f64,
    pub animal: AnimalBaseInitialization,
}

/// Apply the 26.3 Horse and AbstractHorse finalization in Java's draw order.
/// The region RNG is independent from both the spawner and entity RNGs.
pub fn finalize_horse(
    random: &mut RegionRandom,
    group: &mut HorseGroupData,
    has_follow_range_bonus: bool,
) -> HorseInitialization {
    let variant = *group.variant.get_or_insert_with(|| {
        random
            .next_int_bound(7)
            .expect("positive horse variant count") as u8
    });
    let markings = random
        .next_int_bound(5)
        .expect("positive horse markings count") as u8;
    let max_health = 15.0_f32
        + random.next_int_bound(8).expect("positive health bound") as f32
        + random.next_int_bound(9).expect("positive health bound") as f32;
    let movement_speed = (f64::from(0.45_f32)
        + random.next_double() * 0.3
        + random.next_double() * 0.3
        + random.next_double() * 0.3)
        * 0.25;
    let jump_strength = f64::from(0.4_f32)
        + random.next_double() * 0.2
        + random.next_double() * 0.2
        + random.next_double() * 0.2;
    let animal = finalize_animal_base(random, &mut group.animal, has_follow_range_bonus);
    HorseInitialization {
        variant,
        markings,
        max_health,
        movement_speed,
        jump_strength,
        animal,
    }
}

/// Wool colors that may be selected for a naturally spawned sheep.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SheepSpawnColor {
    White,
    Black,
    Gray,
    LightGray,
    Brown,
    Pink,
}

impl SheepSpawnColor {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::White => "white",
            Self::Black => "black",
            Self::Gray => "gray",
            Self::LightGray => "light_gray",
            Self::Brown => "brown",
            Self::Pink => "pink",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SheepInitialization {
    pub color: SheepSpawnColor,
    pub animal: AnimalBaseInitialization,
}

/// Apply Sheep.finalizeSpawn's color selection followed by the animal base.
/// The booleans are membership in the resolved warm and cold biome tags;
/// warm takes precedence if a data pack puts a biome in both. The caller
/// supplies the region's persistent RNG and the current spawn group data.
pub fn finalize_sheep(
    random: &mut RegionRandom,
    group: &mut AnimalGroupData,
    has_follow_range_bonus: bool,
    warm_biome: bool,
    cold_biome: bool,
) -> SheepInitialization {
    use SheepSpawnColor::*;
    let (first, second, third, fourth, common) = if warm_biome {
        (Gray, LightGray, White, Black, Brown)
    } else if cold_biome {
        (LightGray, Gray, White, Brown, Black)
    } else {
        (Black, Gray, LightGray, Brown, White)
    };
    let color = match random.next_int_bound(100).expect("positive bound") {
        0..=4 => first,
        5..=9 => second,
        10..=14 => third,
        15..=17 => fourth,
        _ => {
            if random.next_int_bound(500).expect("positive bound") == 499 {
                Pink
            } else {
                common
            }
        }
    };
    SheepInitialization {
        color,
        animal: finalize_animal_base(random, group, has_follow_range_bonus),
    }
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
    fn sheep_biome_tags_follow_layering_nested_tags_and_registry() {
        #[derive(Default)]
        struct Data(BTreeMap<String, Vec<Vec<u8>>>);
        impl ResourceProvider for Data {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data_stack(&mut self, path: &str) -> Result<Vec<Vec<u8>>> {
                Ok(self.0.get(path).cloned().unwrap_or_default())
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                ensure!(folder == "worldgen/biome", "unexpected registry");
                ["minecraft:plains", "example:warm", "example:cold"]
                    .into_iter()
                    .map(ResourceId::parse)
                    .collect()
            }
        }
        let mut data = Data::default();
        data.0.insert("data/minecraft/tags/worldgen/biome/spawns_warm_variant_farm_animals.json".into(), vec![
            br#"{"values":["minecraft:plains"]}"#.to_vec(),
            br##"{"replace":true,"values":["#example:nested",{"id":"example:missing","required":false}]}"##.to_vec(),
        ]);
        data.0.insert(
            "data/example/tags/worldgen/biome/nested.json".into(),
            vec![br#"{"values":["example:warm"]}"#.to_vec()],
        );
        data.0.insert(
            "data/minecraft/tags/worldgen/biome/spawns_cold_variant_farm_animals.json".into(),
            vec![br#"{"values":["example:cold","example:warm"]}"#.to_vec()],
        );
        let tags = SheepBiomeTags::load(&mut data).unwrap();
        assert_eq!(
            tags.membership(&ResourceId::parse("minecraft:plains").unwrap())
                .unwrap(),
            (false, false)
        );
        assert_eq!(
            tags.membership(&ResourceId::parse("example:warm").unwrap())
                .unwrap(),
            (true, true)
        );
        assert_eq!(
            tags.membership(&ResourceId::parse("example:cold").unwrap())
                .unwrap(),
            (false, true)
        );
        assert!(
            tags.membership(&ResourceId::parse("example:missing").unwrap())
                .is_err()
        );
        data.0.insert(
            "data/example/tags/worldgen/biome/nested.json".into(),
            vec![br##"{"values":["#example:nested"]}"##.to_vec()],
        );
        assert!(SheepBiomeTags::load(&mut data).is_err());
    }

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

    #[test]
    fn sheep_respects_warm_tag_precedence_and_selects_all_natural_colors() {
        for algorithm in [WorldgenRandom::Legacy, WorldgenRandom::Xoroshiro] {
            let mut warm_random = RegionRandom::new(42, algorithm, -2, 7);
            let mut both_random = warm_random.clone();
            let mut warm_group = AnimalGroupData::default();
            let mut both_group = warm_group;
            let mut colors = std::collections::BTreeSet::new();
            for _ in 0..10_000 {
                let warm = finalize_sheep(&mut warm_random, &mut warm_group, false, true, false);
                let both = finalize_sheep(&mut both_random, &mut both_group, false, true, true);
                assert_eq!(warm, both);
                colors.insert(warm.color.as_str());
            }
            assert_eq!(warm_random.next_long(), both_random.next_long());
            assert_eq!(warm_group, both_group);
            assert_eq!(
                colors,
                ["black", "brown", "gray", "light_gray", "pink", "white"]
                    .into_iter()
                    .collect()
            );
        }
    }
}
