//! World-backed biome and difficulty checks for structure drowned.
use anyhow::{Context, Result, ensure};
use native_persistence::{nbt::Tag, world::Difficulty};
use native_resources::{
    drowned_spawn::{DrownedBiomeTags, MainHandRoll},
    worldgen_region_random::RegionRandom,
};
use native_world::loaded::{ChunkPos, World};

use super::DrownedMarkerContext;
use crate::worldgen::{BiomeNeighborhood, LocalDifficulty};

/// Creates a fresh chicken with the active variants while delegating other
/// drowned effects. All callback clones must own independent mutable state.
#[derive(Clone)]
pub struct DrownedChickenEffects<E, C, B> {
    pub other: E,
    variants: std::sync::Arc<native_resources::animal_variants::FarmAnimalSpawnVariants>,
    construct: C,
    biome_at: B,
    pending: Option<Tag>,
}

impl<E, C, B> DrownedChickenEffects<E, C, B> {
    pub fn new(
        variants: std::sync::Arc<native_resources::animal_variants::FarmAnimalSpawnVariants>,
        construct: C,
        biome_at: B,
        other: E,
    ) -> Result<Self> {
        ensure!(
            variants.species() == native_resources::animal_variants::FarmAnimal::Chicken,
            "expected chicken variants"
        );
        Ok(Self {
            other,
            variants,
            construct,
            biome_at,
            pending: None,
        })
    }
}

impl<E, C, B> DrownedSpawnEffects for DrownedChickenEffects<E, C, B>
where
    E: DrownedSpawnEffects,
    C: Clone + FnMut() -> Result<Option<crate::worldgen::FreshCreatureRandom>>,
    B: Clone + FnMut([i32; 3], &World) -> Result<native_resources::packs::ResourceId>,
{
    fn try_existing_chicken(&mut self, position: [i32; 3], world: &World) -> Result<bool> {
        self.other.try_existing_chicken(position, world)
    }
    fn spawn_chicken_jockey(
        &mut self,
        position: [i32; 3],
        world: &World,
        random: &mut RegionRandom,
    ) -> Result<bool> {
        ensure!(
            self.pending.is_none(),
            "previous jockey chicken was not consumed"
        );
        let mut construct = self.construct.clone();
        let Some(chicken) = construct()? else {
            self.construct = construct;
            return Ok(false);
        };
        let mut biome_at = self.biome_at.clone();
        let biome = biome_at(position, world)?;
        let coords = [
            f64::from(position[0]) + 0.5,
            f64::from(position[1]),
            f64::from(position[2]) + 0.5,
        ];
        let tag = crate::worldgen::draft_fresh_jockey_chicken(
            &chicken,
            coords,
            0.0,
            &biome,
            &self.variants,
            random,
        )?;
        self.construct = construct;
        self.biome_at = biome_at;
        self.pending = Some(tag);
        Ok(true)
    }
    fn take_created_chicken(&mut self) -> Result<Option<Tag>> {
        Ok(self.pending.take())
    }
    fn enchant_weapon(
        &mut self,
        position: [i32; 3],
        world: &World,
        random: &mut RegionRandom,
        weapon: MainHandRoll,
        special: f32,
    ) -> Result<Tag> {
        self.other
            .enchant_weapon(position, world, random, weapon, special)
    }
    fn spawn_zombie_nautilus(
        &mut self,
        position: [i32; 3],
        world: &World,
        random: &mut RegionRandom,
        persistent: bool,
    ) -> Result<bool> {
        self.other
            .spawn_zombie_nautilus(position, world, random, persistent)
    }
}

/// Uses the active mob-spawn equipment provider while delegating passenger
/// operations to the caller. Prepared providers are shared between clones.
#[derive(Clone)]
pub struct DrownedEquipmentEffects<E> {
    pub passengers: E,
    trident: std::sync::Arc<native_resources::enchantment_provider::DifficultyProvider>,
    fishing_rod: std::sync::Arc<native_resources::enchantment_provider::DifficultyProvider>,
}

impl<E> DrownedEquipmentEffects<E> {
    /// Enchantability values must come from the active item prototypes. The
    /// item registry and both providers must use the same frozen pack stack.
    pub fn load(
        provider: &mut impl native_resources::packs::ResourceProvider,
        known_items: &std::collections::BTreeSet<native_resources::packs::ResourceId>,
        trident_enchantability: Option<i32>,
        fishing_rod_enchantability: Option<i32>,
        passengers: E,
    ) -> Result<Self> {
        use native_resources::{enchantment_provider::DifficultyProvider, packs::ResourceId};
        let id = ResourceId::parse("minecraft:mob_spawn_equipment")?;
        let trident = DifficultyProvider::load(
            provider,
            &id,
            known_items,
            &ResourceId::parse("minecraft:trident")?,
            trident_enchantability,
        )?;
        let fishing_rod = DifficultyProvider::load(
            provider,
            &id,
            known_items,
            &ResourceId::parse("minecraft:fishing_rod")?,
            fishing_rod_enchantability,
        )?;
        Ok(Self {
            passengers,
            trident: std::sync::Arc::new(trident),
            fishing_rod: std::sync::Arc::new(fishing_rod),
        })
    }
}

impl<E: DrownedSpawnEffects> DrownedSpawnEffects for DrownedEquipmentEffects<E> {
    fn try_existing_chicken(&mut self, position: [i32; 3], world: &World) -> Result<bool> {
        self.passengers.try_existing_chicken(position, world)
    }
    fn spawn_chicken_jockey(
        &mut self,
        position: [i32; 3],
        world: &World,
        random: &mut RegionRandom,
    ) -> Result<bool> {
        self.passengers
            .spawn_chicken_jockey(position, world, random)
    }
    fn take_created_chicken(&mut self) -> Result<Option<Tag>> {
        self.passengers.take_created_chicken()
    }
    fn enchant_weapon(
        &mut self,
        _: [i32; 3],
        _: &World,
        random: &mut RegionRandom,
        weapon: MainHandRoll,
        special: f32,
    ) -> Result<Tag> {
        let (id, provider) = match weapon {
            MainHandRoll::Trident => ("minecraft:trident", &self.trident),
            MainHandRoll::FishingRod => ("minecraft:fishing_rod", &self.fishing_rod),
            MainHandRoll::Unchanged => anyhow::bail!("drowned has no weapon to enchant"),
        };
        let selected = provider.enchant(random, special)?;
        let mut fields = vec![
            ("id".into(), Tag::String(id.into())),
            ("count".into(), Tag::Int(1)),
        ];
        if !selected.is_empty() {
            let enchantments = selected
                .into_iter()
                .map(|entry| {
                    let id = format!("{}:{}", entry.id.namespace, entry.id.path);
                    (id.as_str().into(), Tag::Int(entry.level))
                })
                .collect();
            fields.push((
                "components".into(),
                Tag::Compound(vec![(
                    "minecraft:enchantments".into(),
                    Tag::Compound(enchantments),
                )]),
            ));
        }
        Ok(Tag::Compound(fields))
    }
    fn spawn_zombie_nautilus(
        &mut self,
        position: [i32; 3],
        world: &World,
        random: &mut RegionRandom,
        persistent: bool,
    ) -> Result<bool> {
        self.passengers
            .spawn_zombie_nautilus(position, world, random, persistent)
    }
}

/// Current values observed from the active level at a spawn position.
#[derive(Clone, Copy, Debug)]
pub struct GenerationDifficultyInputs {
    pub difficulty: Difficulty,
    pub overworld_clock_time: i64,
    pub moon_brightness: f32,
}

/// Resources and mutable effects for structure drowned during a terrain batch.
/// The batch supplies its current biome neighborhood at the FEATURES stage.
/// Callback clones must isolate all mutable state until the pass succeeds.
#[derive(Clone)]
pub struct DrownedPlacementInputs<'a, C, E, D, H> {
    pub ice_tags: &'a native_resources::ore_feature::OreBlockTags,
    pub water_fluids: &'a std::collections::BTreeSet<String>,
    pub bubble_fluids: &'a std::collections::BTreeSet<String>,
    pub registry: &'a native_resources::block_registry::GeneratedBlockRegistry,
    pub biome_tags: &'a DrownedBiomeTags,
    pub sea_level: i32,
    pub construct: C,
    pub effects: E,
    pub difficulty_at: D,
    pub halloween_at: H,
    /// Region RNG after the most recent successful mixed FEATURES pass.
    pub last_region_random: Option<RegionRandom>,
}

/// Remaining world operations that can create or modify entities and items.
/// Cloning must isolate all mutable state until the enclosing region commits.
pub trait DrownedSpawnEffects: Clone {
    fn try_existing_chicken(&mut self, position: [i32; 3], world: &World) -> Result<bool>;
    fn spawn_chicken_jockey(
        &mut self,
        position: [i32; 3],
        world: &World,
        random: &mut RegionRandom,
    ) -> Result<bool>;
    fn take_created_chicken(&mut self) -> Result<Option<Tag>> {
        Ok(None)
    }
    fn enchant_weapon(
        &mut self,
        position: [i32; 3],
        world: &World,
        random: &mut RegionRandom,
        weapon: MainHandRoll,
        special: f32,
    ) -> Result<Tag>;
    fn spawn_zombie_nautilus(
        &mut self,
        position: [i32; 3],
        world: &World,
        random: &mut RegionRandom,
        persistent: bool,
    ) -> Result<bool>;
}

/// Uses the generated biome neighborhood and active biome tag. Callers supply
/// live clock/moon values, the holiday check, and entity/item effects. Both
/// callbacks and `effects` must clone independently for transactional retries.
#[derive(Clone)]
pub struct DrownedGenerationContext<'a, E, D, H> {
    pub biomes: &'a BiomeNeighborhood,
    pub biome_tags: &'a DrownedBiomeTags,
    pub difficulty_at: D,
    pub halloween_at: H,
    pub effects: E,
}

impl<E, D, H> DrownedMarkerContext for DrownedGenerationContext<'_, E, D, H>
where
    E: DrownedSpawnEffects,
    D: Clone + FnMut([i32; 3], &World) -> Result<GenerationDifficultyInputs>,
    H: Clone + FnMut() -> Result<bool>,
{
    fn special_multiplier(&mut self, position: [i32; 3], world: &World) -> Result<f32> {
        let chunk = ChunkPos {
            x: position[0].div_euclid(16),
            z: position[2].div_euclid(16),
        };
        ensure!(
            world.chunks.contains_key(&chunk),
            "drowned difficulty chunk is unavailable"
        );
        let inputs = (self.difficulty_at)(position, world)?;
        Ok(LocalDifficulty::for_generation(
            inputs.difficulty,
            inputs.overworld_clock_time,
            inputs.moon_brightness,
        )
        .special_multiplier())
    }
    fn halloween(&mut self) -> Result<bool> {
        (self.halloween_at)()
    }
    fn try_existing_chicken(&mut self, _: [i32; 3], _: &World) -> Result<bool> {
        // WorldGenRegion.getEntities always returns an empty list, including
        // entities that were already added to its protochunks.
        Ok(false)
    }
    fn spawn_chicken_jockey(
        &mut self,
        position: [i32; 3],
        world: &World,
        random: &mut RegionRandom,
    ) -> Result<bool> {
        self.effects.spawn_chicken_jockey(position, world, random)
    }
    fn take_created_chicken(&mut self) -> Result<Option<Tag>> {
        self.effects.take_created_chicken()
    }
    fn enchant_weapon(
        &mut self,
        position: [i32; 3],
        world: &World,
        random: &mut RegionRandom,
        weapon: MainHandRoll,
        special: f32,
    ) -> Result<Tag> {
        self.effects
            .enchant_weapon(position, world, random, weapon, special)
    }
    fn more_frequent_drowned_biome(&mut self, position: [i32; 3], _: &World) -> Result<bool> {
        let biome = self
            .biomes
            .biome_at_block(position[0], position[1], position[2])
            .context("drowned biome is unavailable")?;
        self.biome_tags.more_frequent(biome)
    }
    fn spawn_zombie_nautilus(
        &mut self,
        position: [i32; 3],
        world: &World,
        random: &mut RegionRandom,
        persistent: bool,
    ) -> Result<bool> {
        self.effects
            .spawn_zombie_nautilus(position, world, random, persistent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::QuartBiomeChunk;
    use native_resources::packs::{ResourceId, ResourceProvider};
    use native_world::loaded::Chunk;
    use std::{collections::BTreeMap, sync::Arc};

    struct Data;
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(
                (path == "data/minecraft/tags/worldgen/biome/more_frequent_drowned_spawns.json")
                    .then(|| br#"{"values":["minecraft:river"]}"#.to_vec()),
            )
        }
        fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
            ensure!(folder == "worldgen/biome", "unexpected registry");
            ["minecraft:ocean", "minecraft:river"]
                .into_iter()
                .map(ResourceId::parse)
                .collect()
        }
    }
    #[derive(Clone, Default)]
    struct Effects {
        calls: usize,
    }
    impl DrownedSpawnEffects for Effects {
        fn try_existing_chicken(&mut self, _: [i32; 3], _: &World) -> Result<bool> {
            self.calls += 1;
            Ok(true)
        }
        fn spawn_chicken_jockey(
            &mut self,
            _: [i32; 3],
            _: &World,
            _: &mut RegionRandom,
        ) -> Result<bool> {
            self.calls += 1;
            Ok(false)
        }
        fn enchant_weapon(
            &mut self,
            _: [i32; 3],
            _: &World,
            _: &mut RegionRandom,
            _: MainHandRoll,
            _: f32,
        ) -> Result<Tag> {
            self.calls += 1;
            Ok(Tag::Compound(vec![]))
        }
        fn spawn_zombie_nautilus(
            &mut self,
            _: [i32; 3],
            _: &World,
            _: &mut RegionRandom,
            _: bool,
        ) -> Result<bool> {
            self.calls += 1;
            Ok(false)
        }
    }

    #[test]
    fn uses_world_generation_difficulty_and_zoomed_active_biome_tag() -> Result<()> {
        let tags = DrownedBiomeTags::load(&mut Data)?;
        let ocean = ResourceId::parse("minecraft:ocean")?;
        let river = ResourceId::parse("minecraft:river")?;
        let seed = 87;
        let mut biomes = BiomeNeighborhood::new(seed, 0, 16)?;
        for z in -1..=1 {
            for x in -1..=1 {
                let mut values = Vec::new();
                for _z in 0..4 {
                    for local_x in 0..4 {
                        for _y in 0..4 {
                            values.push(if local_x < 2 {
                                ocean.clone()
                            } else {
                                river.clone()
                            });
                        }
                    }
                }
                biomes.insert(Arc::new(QuartBiomeChunk::from_values(
                    [x, z],
                    seed,
                    0,
                    16,
                    values,
                )?))?;
            }
        }
        let mut world = World::new("minecraft:overworld");
        let difficulty_at = |_: [i32; 3], _: &World| {
            Ok(GenerationDifficultyInputs {
                difficulty: Difficulty::Hard,
                overworld_clock_time: 0,
                moon_brightness: 1.0,
            })
        };
        let mut context = DrownedGenerationContext {
            biomes: &biomes,
            biome_tags: &tags,
            difficulty_at,
            halloween_at: || Ok(true),
            effects: Effects::default(),
        };
        assert!(context.special_multiplier([1, 8, 1], &world).is_err());
        let pos = ChunkPos { x: 0, z: 0 };
        world.chunks.insert(
            pos,
            Arc::new(Chunk {
                position: pos,
                status: None,
                block_light_correct: false,
                sections: BTreeMap::new(),
            }),
        );
        assert_eq!(context.special_multiplier([1, 8, 1], &world)?, 0.125);
        assert!(context.halloween()?);
        let mut found_zoom_difference = false;
        for x in 0..16 {
            for z in 0..16 {
                let zoomed = biomes.biome_at_block(x, 8, z)?;
                let raw = biomes.noise_biome_at_block(x, 8, z)?;
                assert_eq!(
                    context.more_frequent_drowned_biome([x, 8, z], &world)?,
                    zoomed == &river
                );
                found_zoom_difference |= zoomed != raw;
            }
        }
        assert!(
            found_zoom_difference,
            "fixture must exercise seeded biome zoom"
        );
        let mut isolated = context.clone();
        assert!(!isolated.try_existing_chicken([1, 8, 1], &world)?);
        assert_eq!(isolated.effects.calls, 0);
        assert_eq!(context.effects.calls, 0);
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn chicken_effects_prepare_active_variant_and_isolate_failures() -> Result<()> {
        use native_core::random::LegacyRandom;
        use native_resources::animal_variants::{FarmAnimal, FarmAnimalSpawnVariants};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let variants = Arc::new(FarmAnimalSpawnVariants::load_active(
            &mut resources,
            FarmAnimal::Chicken,
        )?);
        let id = ResourceId::parse("minecraft:chicken")?;
        let creature = crate::worldgen::FreshCreatureRandom::from_seed(&id, 42)?;
        let saved = creature.clone();
        let mut effects = DrownedChickenEffects::new(
            variants.clone(),
            move || Ok(Some(saved.clone())),
            |_, _: &World| Ok(ResourceId::parse("minecraft:plains")?),
            Effects::default(),
        )?;
        let world = World::new("minecraft:overworld");
        let mut random = RegionRandom::Legacy(LegacyRandom::new(99));
        let mut expected_random = random.clone();
        let expected = crate::worldgen::draft_fresh_jockey_chicken(
            &creature,
            [3.5, 10.0, -1.5],
            0.0,
            &ResourceId::parse("minecraft:plains")?,
            &variants,
            &mut expected_random,
        )?;
        assert!(effects.spawn_chicken_jockey([3, 10, -2], &world, &mut random)?);
        assert!(
            effects
                .spawn_chicken_jockey([3, 10, -2], &world, &mut random)
                .is_err()
        );
        assert_eq!(effects.take_created_chicken()?, Some(expected));
        assert_eq!(effects.take_created_chicken()?, None);
        assert_eq!(random.next_long(), expected_random.next_long());

        let mut invalid = creature.clone();
        invalid.construction.chicken_egg_time = None;
        let mut failed = DrownedChickenEffects::new(
            variants,
            move || Ok(Some(invalid.clone())),
            |_, _: &World| Ok(ResourceId::parse("minecraft:plains")?),
            Effects::default(),
        )?;
        let mut before = random.clone();
        assert!(
            failed
                .spawn_chicken_jockey([3, 10, -2], &world, &mut random)
                .is_err()
        );
        assert_eq!(random.next_long(), before.next_long());
        assert!(failed.take_created_chicken()?.is_none());
        Ok(())
    }
}
