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
    fn try_existing_chicken(&mut self, position: [i32; 3], world: &World) -> Result<bool> {
        self.effects.try_existing_chicken(position, world)
    }
    fn spawn_chicken_jockey(
        &mut self,
        position: [i32; 3],
        world: &World,
        random: &mut RegionRandom,
    ) -> Result<bool> {
        self.effects.spawn_chicken_jockey(position, world, random)
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
            Ok(false)
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
        assert_eq!(isolated.effects.calls, 1);
        assert_eq!(context.effects.calls, 0);
        Ok(())
    }
}
