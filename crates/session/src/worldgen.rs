//! Intermediate terrain, selected features and lighting for fresh worlds.
//! Full structure and feature coverage, persistence and playable world
//! integration are not yet implemented.
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use glam::IVec3;
use native_core::beardifier::Beardifier;
use native_core::random::DecorationRandom;
use native_persistence::heightmap::BlockHeightFlags;
use native_resources::{
    biome_source::{BiomeSource, ClimateVolume},
    carver::{CarvingMask, schedule::CarverCache},
    density::DensityEvaluator,
    feature_order::{FeatureSchedule, SelectedFeature},
    noise_stage::{BaseNoiseColumn, NoiseMaterial, NoiseStageGenerator},
    packs::{ResourceId, ResourceProvider},
    structure_start::{
        BiomeQuery, Heightmap, SimpleStructure, SimpleStructureStart, StartEnvironment,
    },
    surface_pass::{SurfaceBlockKind, SurfaceStageChunk, apply_surface},
    surface_rules::{SeededSurfaceInputs, SurfaceRule, SurfaceRuleCompiler},
};
use native_world::biome_zoom::{obfuscate_seed, quart_position};
use serde_json::Value;

mod carving;
pub mod chunk_ticks;
mod entity_order;
mod generated_beehive;
mod generated_brushable;
mod generated_chest;
mod generated_creature_random;
mod generated_donkey;
mod generated_drowned;
mod generated_farm_animal;
mod generated_horse;
mod generated_hut_mobs;
mod generated_hut_piece;
mod generated_minecart;
mod generated_mineshaft_piece;
mod generated_sheep;
mod generated_spawner;
mod generated_zombie_nautilus;
pub mod jigsaw;
mod local_difficulty;
pub mod mineshaft;
mod mob_collision;
mod mob_obstruction;
mod mob_placement;
mod mob_spawner;
pub mod ocean_ruin;
mod ore;
pub mod postprocessing;
mod postprocessing_world;
pub mod schedule;
pub mod structure_batch;
mod structure_data;
mod structure_hash_order;
pub mod structure_references;
mod terrain_batch;
pub mod tick_index;
pub use carving::CarverBlockState;
pub use generated_beehive::GeneratedBeeNest;
pub use generated_brushable::GeneratedBrushable;
pub use generated_chest::{GeneratedEmptyChest, GeneratedLootChest};
pub use generated_creature_random::{FreshCreatureRandom, fresh_entity_uuid_from_entropy};
pub use generated_donkey::draft_fresh_donkey;
pub use generated_drowned::draft_fresh_structure_drowned;
pub use generated_farm_animal::{draft_fresh_farm_animal, draft_fresh_jockey_chicken};
pub use generated_horse::draft_fresh_horse;
pub use generated_hut_mobs::{draft_fresh_hut_cat, draft_fresh_hut_witch};
pub use generated_hut_piece::SavedSwampHutPiece;
pub use generated_minecart::draft_fresh_mineshaft_chest_minecart;
pub use generated_mineshaft_piece::SavedMineshaftPiece;
pub use generated_sheep::draft_fresh_sheep;
pub use generated_spawner::{GeneratedCaveSpiderSpawner, GeneratedMonsterSpawner};
pub use generated_zombie_nautilus::draft_fresh_structure_zombie_nautilus;
pub use local_difficulty::LocalDifficulty;
pub use mob_placement::{
    GeneratedCreatureCollisionProgress, GeneratedCreatureInstanceRuleProgress,
    GeneratedCreatureObstructionProgress, GeneratedCreaturePose, GeneratedCreatureProbe,
    GeneratedCreatureProbeProgress, GeneratedCreatureRuleProgress, GeneratedMobPlacementView,
};
pub use mob_spawner::{
    FreshCreatureConstruction, GeneratedSheepSpawner, SheepSpawnInputs, SheepSpawnProgress,
};
pub use ore::{
    DecorationRegion, MixedHutOceanPlacementContext, MixedJigsawPlacementContext,
    MixedMineshaftPlacementContext, MixedOceanRuinPlacement, OceanRuinPlacementContext,
    PlacedDecorationFeature, PreparedDecoration, ScheduledDecorationFeature,
};
pub use postprocessing_world::{
    GeneratedPostprocessing, InitialBonusChestAttempt, PostprocessedGeneratedRegion,
    PostprocessingAttempt,
};
pub use terrain_batch::{
    FreshBatchLight, FreshTerrainBatch, PostprocessedChunkComponents, PostprocessedChunkDraft,
    PostprocessingExpansionCheckpoint,
};

/// Block entities created by a monster-room feature in intermediate terrain.
#[derive(Clone, Debug)]
pub enum GeneratedMonsterRoomEntity {
    Chest(GeneratedLootChest),
    EmptyChest(GeneratedEmptyChest),
    Spawner(GeneratedMonsterSpawner),
    CaveSpiderSpawner(GeneratedCaveSpiderSpawner),
    Brushable(GeneratedBrushable),
    /// A protochunk DUMMY chest not yet materialized by getBlockEntity.
    PendingChest {
        position: [i32; 3],
        state: native_world::loaded::BlockState,
    },
}

/// Result of serializing one generation-time entity. Java does not add a
/// passenger as a separate protochunk entry; a saved compound can be empty.
#[derive(Clone)]
pub enum GeneratedEntityRecord {
    Passenger,
    Saved(native_persistence::nbt::Tag),
}

/// One runtime entity visited by Java's addFreshEntityWithPassengers.
#[derive(Clone)]
pub struct GeneratedEntityAddition {
    pub block_position: [i32; 3],
    pub record: GeneratedEntityRecord,
}

fn stage_generated_entity(
    world: &native_world::loaded::World,
    records: &mut Arc<
        BTreeMap<native_world::loaded::ChunkPos, Arc<Vec<native_persistence::nbt::Tag>>>,
    >,
    addition: GeneratedEntityAddition,
) -> Result<()> {
    let [x, _, z] = addition.block_position;
    let target = native_world::loaded::ChunkPos {
        x: x.div_euclid(16),
        z: z.div_euclid(16),
    };
    ensure!(
        world.chunks.contains_key(&target),
        "generated entity destination chunk is missing"
    );
    if let GeneratedEntityRecord::Saved(data) = addition.record {
        ensure!(
            matches!(data, native_persistence::nbt::Tag::Compound(_)),
            "generated entity data must be a compound"
        );
        native_persistence::nbt::write(&native_persistence::nbt::NamedTag {
            name: "".into(),
            value: data.clone(),
        })?;
        Arc::make_mut(Arc::make_mut(records).entry(target).or_default()).push(data);
    }
    Ok(())
}

/// A complete BIOMES-stage chunk in Z, X, Y order, independent of rendering.
pub struct QuartBiomeChunk {
    chunk: [i32; 2],
    seed: i64,
    min_y: i32,
    height: i32,
    values: Vec<ResourceId>,
}

/// A terrain-stage result ready for the remaining generation stages. The
/// embedded chunk is deliberately not marked FULL or safe for world saving.
pub struct MaterializedTerrain {
    world: native_world::loaded::World,
    position: native_world::loaded::ChunkPos,
    seed: i64,
    min_y: i32,
    height: i32,
    postprocessing: Arc<[bool]>,
    noise_postprocessing: Option<Arc<native_core::postprocessing::PostProcessing>>,
    surface_postprocessing: Option<Arc<native_core::postprocessing::PostProcessing>>,
    carver_postprocessing: Option<Arc<native_core::postprocessing::PostProcessing>>,
}

/// A collection of intermediate generated chunks sharing one world registry.
/// Individual feature attempts may have modified these terrain-stage chunks;
/// they are not ready for play or saving.
pub struct CarvedTerrainWorld {
    world: Arc<native_world::loaded::World>,
    seed: i64,
    min_y: i32,
    height: i32,
    // Completed only when every selected feature has been processed with
    // structures disabled; lighting and later generation are still pending.
    completed_decoration: std::collections::BTreeSet<native_world::loaded::ChunkPos>,
    // Once light has been published, clearing chunk arrays cannot recreate the
    // fresh engine's earlier allocation and propagation history.
    has_published_light: bool,
    // Direct feature section writes do not update Java's primed heightmap.
    ore_ocean_floor_heights: BTreeMap<native_world::loaded::ChunkPos, Arc<[i32; 256]>>,
    feature_world_surface_heights: BTreeMap<native_world::loaded::ChunkPos, Arc<[i32; 256]>>,
    final_heightmaps: BTreeMap<native_world::loaded::ChunkPos, Arc<ore::FinalHeightmaps>>,
    postprocessing: BTreeMap<native_world::loaded::ChunkPos, Arc<[bool]>>,
    noise_postprocessing:
        BTreeMap<native_world::loaded::ChunkPos, Arc<native_core::postprocessing::PostProcessing>>,
    surface_postprocessing:
        BTreeMap<native_world::loaded::ChunkPos, Arc<native_core::postprocessing::PostProcessing>>,
    carver_postprocessing:
        BTreeMap<native_world::loaded::ChunkPos, Arc<native_core::postprocessing::PostProcessing>>,
    feature_postprocessing:
        BTreeMap<native_world::loaded::ChunkPos, Arc<native_core::postprocessing::PostProcessing>>,
    generated_bee_nests: Arc<BTreeMap<[i32; 3], GeneratedBeeNest>>,
    generated_monster_entities: Arc<BTreeMap<[i32; 3], Option<GeneratedMonsterRoomEntity>>>,
    generated_entity_order:
        BTreeMap<native_world::loaded::ChunkPos, Arc<entity_order::LivePositions>>,
    generated_pending_order:
        BTreeMap<native_world::loaded::ChunkPos, Arc<entity_order::PendingPositions>>,
    generated_entities:
        Arc<BTreeMap<native_world::loaded::ChunkPos, Arc<Vec<native_persistence::nbt::Tag>>>>,
}

/// Registration history for a newly created generation light engine. The
/// scheduler must route every INITIALIZE_LIGHT registration through this
/// tracker. Supported block checks are modeled as drained before activation;
/// other light-engine work requires a different tracker.
pub struct FreshGenerationLightTracker {
    dimension: String,
    seed: i64,
    min_y: i32,
    height: i32,
    has_skylight: bool,
    registered: BTreeMap<native_world::loaded::ChunkPos, Arc<native_world::loaded::Chunk>>,
    allocated: BTreeMap<native_world::loaded::ChunkPos, BTreeSet<i32>>,
    sky_sources: BTreeMap<native_world::loaded::ChunkPos, Arc<[i32; 256]>>,
    checked_blocks: BTreeSet<String>,
    zero_layer: Arc<[u8; native_world::loaded::LIGHT_BYTES]>,
}

fn generated_state_is_air(
    registry: &native_resources::block_registry::GeneratedBlockRegistry,
    state: &native_world::loaded::BlockState,
) -> Option<bool> {
    if !registry.contains(&state.name) {
        return None;
    }
    let air = matches!(
        state.name.as_str(),
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    );
    if air && !state.properties.is_empty() {
        return None;
    }
    Some(air)
}

impl FreshGenerationLightTracker {
    /// Start only when the associated Java-equivalent engine is freshly empty.
    pub fn new(terrain: &CarvedTerrainWorld, has_skylight: bool) -> Result<Self> {
        ensure!(!terrain.has_published_light, "light was already published");
        ensure!(
            terrain
                .world
                .chunks
                .values()
                .all(|c| !c.block_light_correct),
            "fresh engine cannot contain propagated chunks"
        );
        Ok(Self {
            dimension: terrain.world.dimension.clone(),
            seed: terrain.seed,
            min_y: terrain.min_y,
            height: terrain.height,
            has_skylight,
            registered: BTreeMap::new(),
            allocated: BTreeMap::new(),
            sky_sources: BTreeMap::new(),
            checked_blocks: BTreeSet::new(),
            zero_layer: Arc::new([0; native_world::loaded::LIGHT_BYTES]),
        })
    }

    fn validate_terrain(&self, terrain: &CarvedTerrainWorld) -> Result<()> {
        ensure!(!terrain.has_published_light, "light was already published");
        ensure!(
            self.dimension == terrain.world.dimension
                && self.seed == terrain.seed
                && self.min_y == terrain.min_y
                && self.height == terrain.height,
            "light tracker belongs to different terrain"
        );
        ensure!(
            self.registered
                .keys()
                .all(|pos| terrain.completed_decoration.contains(pos)),
            "registered light chunk has not completed features"
        );
        self.validate_world(&terrain.world)
    }

    fn validate_world(&self, world: &native_world::loaded::World) -> Result<()> {
        ensure!(
            world
                .chunks
                .values()
                .all(|chunk| !chunk.block_light_correct),
            "fresh light engine has propagated chunks"
        );
        for (pos, original) in &self.registered {
            let Some(current) = world.chunks.get(pos) else {
                bail!("registered light chunk has changed: {pos:?} is missing");
            };
            if !Arc::ptr_eq(current, original) {
                for (y, before) in &original.sections {
                    let Some(after) = current.sections.get(y) else {
                        bail!("registered light chunk has changed: {pos:?} section {y} is missing");
                    };
                    if let (Some(before), Some(after)) = (&before.blocks, &after.blocks) {
                        if let Some(index) =
                            before.iter().zip(after.iter()).position(|(a, b)| a != b)
                        {
                            let position = [
                                pos.x * 16 + (index % 16) as i32,
                                i32::from(*y) * 16 + (index / 256) as i32,
                                pos.z * 16 + ((index / 16) % 16) as i32,
                            ];
                            bail!(
                                "registered light chunk has changed: {position:?}: {:?} -> {:?}",
                                world.state(before[index]),
                                world.state(after[index])
                            );
                        }
                    }
                }
                bail!("registered light chunk has changed: {pos:?} has no differing block found");
            }
        }
        Ok(())
    }

    /// Register the completed chunk in the same order as the light engine.
    pub fn register_chunk(
        &mut self,
        terrain: &CarvedTerrainWorld,
        pos: native_world::loaded::ChunkPos,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
    ) -> Result<()> {
        self.validate_terrain(terrain)?;
        ensure!(
            !self.registered.contains_key(&pos),
            "light chunk is already registered"
        );
        ensure!(
            terrain.completed_decoration.contains(&pos),
            "light initialization requires completed features"
        );
        let max_y = self
            .min_y
            .checked_add(self.height)
            .context("light height overflow")?;
        native_world::block_light::LightBounds::new(self.min_y, max_y)
            .context("unsupported light height")?;
        let mut checked = BTreeSet::new();
        let statuses = native_world::generation_light::initialized_nonempty_sections(
            &terrain.world,
            self.min_y.div_euclid(16)..max_y.div_euclid(16),
            [pos],
            |state| {
                let air = generated_state_is_air(registry, state)?;
                checked.insert(state.name.clone());
                Some(air)
            },
        )
        .context("registered light sections are missing or unsupported")?;
        // Validate packed section coordinates before modifying the tracker.
        native_world::generation_light::VisibleLightSnapshot::fresh_after_section_updates(
            true,
            self.has_skylight,
            statuses.iter().copied(),
            [],
        )
        .context("registered light section is out of range")?;
        let chunk = terrain
            .world
            .chunks
            .get(&pos)
            .context("light chunk is missing")?;
        for (position, y) in statuses {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let neighbor = native_world::loaded::ChunkPos {
                        x: position.x + dx,
                        z: position.z + dz,
                    };
                    self.allocated
                        .entry(neighbor)
                        .or_default()
                        .extend(y - 1..=y + 1);
                }
            }
        }
        self.checked_blocks.extend(checked);
        self.registered.insert(pos, Arc::clone(chunk));
        Ok(())
    }

    /// Complete the fresh INITIALIZE_LIGHT inputs for one chunk. Source
    /// heights are computed before registering so a failure leaves no partial
    /// registration behind, including in dimensions without sky light.
    pub fn initialize_chunk(
        &mut self,
        terrain: &CarvedTerrainWorld,
        pos: native_world::loaded::ChunkPos,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
    ) -> Result<()> {
        self.validate_terrain(terrain)?;
        ensure!(
            !self.registered.contains_key(&pos),
            "light chunk is already registered"
        );
        ensure!(
            terrain.completed_decoration.contains(&pos),
            "light initialization requires completed features"
        );
        let max_y = self
            .min_y
            .checked_add(self.height)
            .context("light height overflow")?;
        let bounds = native_world::block_light::LightBounds::new(self.min_y, max_y)
            .context("unsupported light height")?;
        let sources =
            native_world::sky_light::generated_chunk_sky_sources(&terrain.world, bounds, pos)
                .with_context(|| {
                    format!(
                        "generated sky sources are incomplete or unsupported: {}",
                        native_world::sky_light::generated_chunk_sky_issue(
                            &terrain.world,
                            bounds,
                            pos
                        )
                    )
                })?;
        self.register_chunk(terrain, pos, registry)?;
        self.sky_sources.insert(pos, Arc::from(sources));
        Ok(())
    }

    /// Complete source heights for a chunk whose section status was already
    /// registered in this fresh-engine revision and has not changed.
    pub fn initialize_registered_sources(
        &mut self,
        terrain: &CarvedTerrainWorld,
        pos: native_world::loaded::ChunkPos,
    ) -> Result<()> {
        self.validate_terrain(terrain)?;
        ensure!(
            self.registered.contains_key(&pos),
            "light chunk is not registered"
        );
        ensure!(
            !self.sky_sources.contains_key(&pos),
            "sky sources already initialized"
        );
        let max_y = self
            .min_y
            .checked_add(self.height)
            .context("light height overflow")?;
        let bounds = native_world::block_light::LightBounds::new(self.min_y, max_y)
            .context("unsupported light height")?;
        let sources =
            native_world::sky_light::generated_chunk_sky_sources(&terrain.world, bounds, pos)
                .with_context(|| {
                    format!(
                        "generated sky sources are incomplete or unsupported: {}",
                        native_world::sky_light::generated_chunk_sky_issue(
                            &terrain.world,
                            bounds,
                            pos
                        )
                    )
                })?;
        self.sky_sources.insert(pos, Arc::from(sources));
        Ok(())
    }

    /// Return source heights while registered generation data matches this
    /// tracked revision. The array uses X + 16 * Z within the chunk.
    pub fn sky_sources(
        &self,
        terrain: &CarvedTerrainWorld,
        pos: native_world::loaded::ChunkPos,
    ) -> Result<Arc<[i32; 256]>> {
        self.validate_terrain(terrain)?;
        self.sky_sources
            .get(&pos)
            .cloned()
            .context("sky sources were not initialized")
    }

    /// Calculate block-light values for a hypothetical sequence of activated
    /// chunks, starting from this fresh engine revision. This does not advance
    /// the engine, apply sky light, or publish any arrays.
    pub fn calculate_block_light_for_active_chunks(
        &self,
        terrain: &CarvedTerrainWorld,
        center: native_world::loaded::ChunkPos,
        active: &BTreeSet<native_world::loaded::ChunkPos>,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
    ) -> Result<native_world::generation_light::VisibleLayers> {
        self.calculate_block_light_in_world(terrain, &terrain.world, center, active, registry)
    }

    fn calculate_block_light_in_world(
        &self,
        terrain: &CarvedTerrainWorld,
        light_world: &native_world::loaded::World,
        center: native_world::loaded::ChunkPos,
        active: &BTreeSet<native_world::loaded::ChunkPos>,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
    ) -> Result<native_world::generation_light::VisibleLayers> {
        ensure!(
            active.iter().all(|pos| self.registered.contains_key(pos)),
            "active light chunk was not registered"
        );
        let mut covered = BTreeSet::new();
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                covered.insert(native_world::loaded::ChunkPos {
                    x: center.x.checked_add(dx).context("light X overflow")?,
                    z: center.z.checked_add(dz).context("light Z overflow")?,
                });
            }
        }
        let storage = self.snapshot(terrain, covered, registry)?;
        let max_y = self
            .min_y
            .checked_add(self.height)
            .context("light height overflow")?;
        let bounds = native_world::block_light::LightBounds::new(self.min_y, max_y)
            .context("unsupported light height")?;
        native_world::generation_block_light::generated_block_light_from_active_chunks(
            light_world,
            bounds,
            center,
            active,
            &storage,
        )
        .with_context(|| {
            format!(
                "active generated block-light inputs are incomplete or unsupported: {}",
                native_world::generation_block_light::generated_block_light_issue(
                    light_world,
                    bounds,
                    center,
                    &storage
                )
            )
        })
    }

    /// Preview sky light for activated chunks in a fresh engine. Registered
    /// chunks without initialized sources and unregistered chunks are
    /// assumed to retain their never-updated default sky-source heightmap.
    pub fn calculate_sky_light_for_active_chunks(
        &self,
        terrain: &CarvedTerrainWorld,
        center: native_world::loaded::ChunkPos,
        active: &BTreeSet<native_world::loaded::ChunkPos>,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
    ) -> Result<native_world::generation_light::VisibleLayers> {
        self.calculate_sky_light_in_world(terrain, &terrain.world, center, active, registry)
    }

    fn calculate_sky_light_in_world(
        &self,
        terrain: &CarvedTerrainWorld,
        light_world: &native_world::loaded::World,
        center: native_world::loaded::ChunkPos,
        active: &BTreeSet<native_world::loaded::ChunkPos>,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
    ) -> Result<native_world::generation_light::VisibleLayers> {
        ensure!(self.has_skylight, "dimension has no sky-light engine");
        ensure!(
            active.iter().all(|pos| self.sky_sources.contains_key(pos)),
            "active sky-light chunk was not initialized"
        );
        let mut covered = BTreeSet::new();
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                let pos = native_world::loaded::ChunkPos {
                    x: center.x.checked_add(dx).context("light X overflow")?,
                    z: center.z.checked_add(dz).context("light Z overflow")?,
                };
                covered.insert(pos);
            }
        }
        let storage = self.snapshot(terrain, covered, registry)?;
        let max_y = self
            .min_y
            .checked_add(self.height)
            .context("light height overflow")?;
        let bounds = native_world::block_light::LightBounds::new(self.min_y, max_y)
            .context("unsupported light height")?;
        let registered = self.registered.keys().copied().collect();
        let initialized = self.sky_sources.keys().copied().collect();
        native_world::generation_sky_light::generated_sky_light_with_initialization_history(
            light_world,
            bounds,
            center,
            active,
            &storage,
            &registered,
            &initialized,
        )
        .context("active generated sky-light inputs are incomplete or unsupported")
    }

    /// Capture one immutable engine revision. Registered-chunk edits outside
    /// the explicitly tracked generation path invalidate this tracker.
    pub fn snapshot(
        &self,
        terrain: &CarvedTerrainWorld,
        covered_columns: impl IntoIterator<Item = native_world::loaded::ChunkPos>,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
    ) -> Result<native_world::generation_light::VisibleLightSnapshot> {
        self.validate_terrain(terrain)?;
        ensure!(
            self.checked_blocks
                .iter()
                .all(|name| registry.contains(name)),
            "registered block is absent from the registry"
        );
        let mut snapshot =
            native_world::generation_light::VisibleLightSnapshot::new(true, self.has_skylight);
        for pos in covered_columns {
            ensure!(
                (-2_097_152..=2_097_151).contains(&pos.x)
                    && (-2_097_152..=2_097_151).contains(&pos.z),
                "light column is out of range"
            );
            let mut block = native_world::generation_light::VisibleLayers::new();
            let mut sky = native_world::generation_light::VisibleLayers::new();
            if let Some(sections) = self.allocated.get(&pos) {
                for &sy in sections {
                    block.insert(sy, Arc::clone(&self.zero_layer));
                    if self.has_skylight {
                        sky.insert(sy, Arc::clone(&self.zero_layer));
                    }
                }
            }
            snapshot.insert_complete_column(
                pos,
                native_world::generation_light::VisibleLightColumn::new(block, sky),
            );
        }
        Ok(snapshot)
    }
}

/// Generation lighting with monotonically activated source chunks. Most
/// columns are calculated from the activation inputs; tracked inactive tree
/// and cave-vine checks preserve their updated columns instead. Activations
/// overlapping that history use incremental propagation. Section removals,
/// restored light and live-world edits are not supported here.
pub struct StagedGenerationLightEngine {
    fresh: FreshGenerationLightTracker,
    active: BTreeSet<native_world::loaded::ChunkPos>,
    frozen_world: Option<Arc<native_world::loaded::World>>,
    incremental_columns: BTreeMap<
        native_world::loaded::ChunkPos,
        native_world::generation_light::VisibleLightColumn,
    >,
}

// These worlds share the same append-only state registry lineage. Compare only
// changed sections and cache block-state pairs within each changed chunk.
fn unchanged_chunk_light_behavior(
    before_world: &native_world::loaded::World,
    current_world: &native_world::loaded::World,
    before: &native_world::loaded::Chunk,
    current: &native_world::loaded::Chunk,
    min_y: i32,
    height: i32,
) -> Result<bool> {
    let end = min_y.checked_add(height).context("light height overflow")?;
    let mut compared = BTreeMap::new();
    for y in min_y.div_euclid(16)..end.div_euclid(16) {
        let y = i8::try_from(y).context("light section out of range")?;
        let (Some(a), Some(b)) = (before.sections.get(&y), current.sections.get(&y)) else {
            return Ok(false);
        };
        if Arc::ptr_eq(a, b) {
            continue;
        }
        let (Some(a), Some(b)) = (&a.blocks, &b.blocks) else {
            return Ok(false);
        };
        for (&a, &b) in a.iter().zip(b.iter()) {
            if a == b {
                continue;
            }
            let equal = if let Some(&equal) = compared.get(&(a, b)) {
                equal
            } else {
                let old = before_world.state(a).context("old light state missing")?;
                let new = current_world
                    .state(b)
                    .context("current light state missing")?;
                let equal = native_world::block_light::same_light_behavior(old, new)
                    .context("unsupported light state change")?;
                compared.insert((a, b), equal);
                equal
            };
            if !equal {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

impl StagedGenerationLightEngine {
    pub fn new(fresh: FreshGenerationLightTracker) -> Self {
        Self {
            fresh,
            active: BTreeSet::new(),
            frozen_world: None,
            incremental_columns: BTreeMap::new(),
        }
    }

    /// Block writes must not touch registered chunks or the input halo of any
    /// activated chunk. Probe only nearby positions, even for a large batch.
    pub(super) fn allows_writes(
        &self,
        center: native_world::loaded::ChunkPos,
        radius: i64,
    ) -> bool {
        fn any_near(
            center: native_world::loaded::ChunkPos,
            radius: i64,
            contains: impl Fn(&native_world::loaded::ChunkPos) -> bool,
        ) -> bool {
            (-radius..=radius).any(|dz| {
                (-radius..=radius).any(|dx| {
                    let x = i32::try_from(i64::from(center.x) + dx);
                    let z = i32::try_from(i64::from(center.z) + dz);
                    x.ok()
                        .zip(z.ok())
                        .is_some_and(|(x, z)| contains(&native_world::loaded::ChunkPos { x, z }))
                })
            })
        }
        !any_near(center, radius, |pos| {
            self.fresh.registered.contains_key(pos)
        }) && !any_near(center, radius + 1, |pos| self.active.contains(pos))
    }

    fn validate_active_world(&self, world: &native_world::loaded::World) -> Result<()> {
        Self::validate_active_world_parts(
            self.frozen_world.as_ref(),
            &self.active,
            self.fresh.min_y,
            self.fresh.height,
            world,
        )
    }

    fn validate_active_world_parts(
        frozen: Option<&Arc<native_world::loaded::World>>,
        active: &BTreeSet<native_world::loaded::ChunkPos>,
        min_y: i32,
        height: i32,
        world: &native_world::loaded::World,
    ) -> Result<()> {
        if let Some(frozen) = frozen {
            if !std::ptr::eq(frozen.as_ref(), world) {
                for active in active {
                    for dz in -1i32..=1 {
                        for dx in -1i32..=1 {
                            let neighbor = native_world::loaded::ChunkPos {
                                x: active.x.checked_add(dx).context("light X overflow")?,
                                z: active.z.checked_add(dz).context("light Z overflow")?,
                            };
                            let unchanged =
                                match (frozen.chunks.get(&neighbor), world.chunks.get(&neighbor)) {
                                    (Some(before), Some(after)) => {
                                        Arc::ptr_eq(before, after)
                                            || unchanged_chunk_light_behavior(
                                                frozen, world, before, after, min_y, height,
                                            )?
                                    }
                                    (None, None) => true,
                                    _ => false,
                                };
                            ensure!(unchanged, "active light neighborhood changed");
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Register another completed chunk. Unregistered terrain outside every
    /// active chunk's 3x3 neighborhood may have changed since activation;
    /// registered chunks and all active input neighborhoods must be unchanged.
    pub fn initialize_chunk(
        &mut self,
        terrain: &CarvedTerrainWorld,
        pos: native_world::loaded::ChunkPos,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
    ) -> Result<()> {
        self.validate_active_world(&terrain.world)?;
        ensure!(
            !self.fresh.registered.contains_key(&pos),
            "light chunk is already registered"
        );
        ensure!(
            terrain.completed_decoration.contains(&pos),
            "light initialization requires completed features"
        );
        let mut affected = Vec::with_capacity(9);
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                affected.push(native_world::loaded::ChunkPos {
                    x: pos.x.checked_add(dx).context("light X overflow")?,
                    z: pos.z.checked_add(dz).context("light Z overflow")?,
                });
            }
        }
        let updated = if affected
            .iter()
            .any(|p| self.incremental_columns.contains_key(p))
        {
            let max_y = self
                .fresh
                .min_y
                .checked_add(self.fresh.height)
                .context("light height overflow")?;
            let sections = native_world::generation_light::initialized_nonempty_sections(
                &terrain.world,
                self.fresh.min_y.div_euclid(16)..max_y.div_euclid(16),
                [pos],
                |state| generated_state_is_air(registry, state),
            )
            .context("registered light sections are missing or unsupported")?;
            if sections.is_empty() {
                None
            } else {
                let before = self.snapshot(terrain, affected.iter().copied(), registry)?;
                let after = before
                    .allocate_nonempty_sections(&sections, &self.active)
                    .context(
                        "registration light section allocation is incomplete or unsupported",
                    )?;
                Some(
                    affected
                        .iter()
                        .map(|&p| {
                            Ok((
                                p,
                                after
                                    .complete_column(p)
                                    .context("registration light column missing")?,
                            ))
                        })
                        .collect::<Result<Vec<_>>>()?,
                )
            }
        } else {
            None
        };
        self.fresh.initialize_chunk(terrain, pos, registry)?;
        if let Some(columns) = updated {
            self.incremental_columns.extend(columns);
        }
        if self.frozen_world.is_some() {
            self.frozen_world = Some(Arc::clone(&terrain.world));
        }
        Ok(())
    }

    /// Capture a revision without modifying the world or a previous snapshot.
    pub fn snapshot(
        &self,
        terrain: &CarvedTerrainWorld,
        covered: impl IntoIterator<Item = native_world::loaded::ChunkPos>,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
    ) -> Result<native_world::generation_light::VisibleLightSnapshot> {
        self.snapshot_for(terrain, covered, registry, &self.active)
    }

    /// Activate one initialized chunk and return the requested visible data.
    /// Failed calculation leaves the prior activation state untouched.
    pub fn activate(
        &mut self,
        terrain: &CarvedTerrainWorld,
        pos: native_world::loaded::ChunkPos,
        covered: impl IntoIterator<Item = native_world::loaded::ChunkPos>,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
    ) -> Result<native_world::generation_light::VisibleLightSnapshot> {
        self.fresh.validate_terrain(terrain)?;
        ensure!(
            self.fresh.sky_sources.contains_key(&pos),
            "light chunk is not initialized"
        );
        ensure!(!self.active.contains(&pos), "light chunk is already active");
        let mut overlaps = false;
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                let affected = native_world::loaded::ChunkPos {
                    x: pos.x.checked_add(dx).context("light X overflow")?,
                    z: pos.z.checked_add(dz).context("light Z overflow")?,
                };
                overlaps |= self.incremental_columns.contains_key(&affected);
            }
        }
        if let Some(frozen) = &self.frozen_world {
            for dz in -1i32..=1 {
                for dx in -1i32..=1 {
                    let neighbor = native_world::loaded::ChunkPos {
                        x: pos.x.checked_add(dx).context("light X overflow")?,
                        z: pos.z.checked_add(dz).context("light Z overflow")?,
                    };
                    let same = match (
                        frozen.chunks.get(&neighbor),
                        terrain.world.chunks.get(&neighbor),
                    ) {
                        (Some(before), Some(now)) => {
                            Arc::ptr_eq(before, now)
                                || unchanged_chunk_light_behavior(
                                    frozen,
                                    &terrain.world,
                                    before,
                                    now,
                                    self.fresh.min_y,
                                    self.fresh.height,
                                )?
                        }
                        (None, None) => true,
                        _ => false,
                    };
                    ensure!(same, "light inputs changed before activation");
                }
            }
        }
        let covered: BTreeSet<_> = covered.into_iter().collect();
        if overlaps {
            let mut area = Vec::with_capacity(25);
            for dz in -2i32..=2 {
                for dx in -2i32..=2 {
                    area.push(native_world::loaded::ChunkPos {
                        x: pos.x.checked_add(dx).context("light X overflow")?,
                        z: pos.z.checked_add(dz).context("light Z overflow")?,
                    });
                }
            }
            let light_world = self.frozen_world.as_deref().unwrap_or(&terrain.world);
            let max_y = self
                .fresh
                .min_y
                .checked_add(self.fresh.height)
                .context("light height overflow")?;
            let bounds = native_world::block_light::LightBounds::new(self.fresh.min_y, max_y)
                .context("unsupported light bounds")?;
            let before = self.snapshot(terrain, area.iter().copied(), registry)?;
            let after_block = native_world::generation_block_light::activate_generated_block_light(
                light_world,
                bounds,
                &before,
                pos,
            )
            .context("incremental block light activation inputs are incomplete or unsupported")?;
            let after = if self.fresh.has_skylight {
                native_world::generation_sky_light::activate_generated_sky_light(
                    light_world,
                    bounds,
                    &after_block,
                    pos,
                    &self.fresh.sky_sources,
                )
                .with_context(|| {
                    let missing_sources: Vec<_> = [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)]
                        .into_iter()
                        .filter_map(|(dx, dz)| {
                            let p = native_world::loaded::ChunkPos {
                                x: pos.x.checked_add(dx)?,
                                z: pos.z.checked_add(dz)?,
                            };
                            (!self.fresh.sky_sources.contains_key(&p)).then_some(p)
                        })
                        .collect();
                    let missing_sections: Vec<_> = (self.fresh.min_y.div_euclid(16) - 1
                        ..=max_y.div_euclid(16))
                        .filter(|&sy| after_block.sky_section_allocated(pos, sy) != Some(true))
                        .collect();
                    format!("incremental sky light activation inputs are incomplete or unsupported: missing sources {missing_sources:?}, missing target sections {missing_sections:?}")
                })?
            } else {
                after_block
            };
            let mut result = self.snapshot(terrain, covered.iter().copied(), registry)?;
            let mut updated = Vec::with_capacity(area.len());
            for &column in &area {
                let value = after
                    .complete_column(column)
                    .context("activation light column missing")?;
                if covered.contains(&column) {
                    result.insert_complete_column(column, value.clone());
                }
                updated.push((column, value));
            }
            self.incremental_columns.extend(updated);
            self.active.insert(pos);
            if self.frozen_world.is_none() {
                self.frozen_world = Some(Arc::clone(&terrain.world));
            }
            return Ok(result);
        }
        let mut active = self.active.clone();
        active.insert(pos);
        let snapshot = self.snapshot_for(terrain, covered, registry, &active)?;
        if self.frozen_world.is_none() {
            self.frozen_world = Some(Arc::clone(&terrain.world));
        }
        self.active = active;
        Ok(snapshot)
    }

    fn snapshot_for(
        &self,
        terrain: &CarvedTerrainWorld,
        covered: impl IntoIterator<Item = native_world::loaded::ChunkPos>,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
        active: &BTreeSet<native_world::loaded::ChunkPos>,
    ) -> Result<native_world::generation_light::VisibleLightSnapshot> {
        use native_world::generation_light::{VisibleLayers, VisibleLightColumn};
        use native_world::loaded::ChunkPos;

        let covered: BTreeSet<_> = covered.into_iter().collect();
        let mut snapshot = self
            .fresh
            .snapshot(terrain, covered.iter().copied(), registry)?;
        for pos in covered {
            if let Some(column) = self.incremental_columns.get(&pos) {
                snapshot.insert_complete_column(pos, column.clone());
                continue;
            }
            // Propagation loses at least one level per horizontal step, so a
            // source more than one chunk away cannot reach this column.
            let affected = (-1i32..=1).any(|dz| {
                (-1i32..=1).any(|dx| {
                    pos.x
                        .checked_add(dx)
                        .zip(pos.z.checked_add(dz))
                        .is_some_and(|(x, z)| active.contains(&ChunkPos { x, z }))
                })
            });
            if !affected {
                continue;
            }
            // Unregistered proto-chunk writes do not enqueue light updates.
            // Replay against the immutable blocks used for the activation.
            let light_world = self.frozen_world.as_deref().unwrap_or(&terrain.world);
            let block = self.fresh.calculate_block_light_in_world(
                terrain,
                light_world,
                pos,
                active,
                registry,
            )?;
            let sky = if self.fresh.has_skylight {
                self.fresh.calculate_sky_light_in_world(
                    terrain,
                    light_world,
                    pos,
                    active,
                    registry,
                )?
            } else {
                VisibleLayers::new()
            };
            let mut visible_block = VisibleLayers::new();
            for (y, layer) in block {
                if snapshot
                    .block_section_allocated(pos, y)
                    .context("uncovered block-light column")?
                {
                    visible_block.insert(y, layer);
                }
            }
            let mut visible_sky = VisibleLayers::new();
            for (y, layer) in sky {
                if snapshot
                    .sky_section_allocated(pos, y)
                    .context("uncovered sky-light column")?
                {
                    visible_sky.insert(y, layer);
                }
            }
            snapshot
                .insert_complete_column(pos, VisibleLightColumn::new(visible_block, visible_sky));
        }
        Ok(snapshot)
    }
}

impl CarvedTerrainWorld {
    /// Capture both world-generation maps and the four final maps immediately
    /// after terrain, before neighboring features can mutate this chunk.
    /// Direct feature section writes intentionally leave all six maps stale.
    pub fn initialize_worldgen_heightmaps(
        &mut self,
        pos: native_world::loaded::ChunkPos,
        tags: &native_resources::heightmap_tags::HeightmapTags,
    ) -> Result<()> {
        use native_world::loaded::BlockStateId;
        ensure!(
            !self.ore_ocean_floor_heights.contains_key(&pos)
                && !self.feature_world_surface_heights.contains_key(&pos)
                && !self.final_heightmaps.contains_key(&pos),
            "generation heightmaps were already initialized"
        );
        let base_x = pos.x.checked_mul(16).context("chunk X overflow")?;
        let base_z = pos.z.checked_mul(16).context("chunk Z overflow")?;
        base_x.checked_add(15).context("chunk X overflow")?;
        base_z.checked_add(15).context("chunk Z overflow")?;
        let max_y = self
            .min_y
            .checked_add(self.height)
            .context("generation height overflow")?;
        ensure!(
            self.min_y.rem_euclid(16) == 0 && self.height > 0 && self.height.rem_euclid(16) == 0,
            "invalid generation height"
        );
        let chunk = self
            .world
            .chunks
            .get(&pos)
            .context("generation chunk is missing")?;
        let mut ocean = [self.min_y; 256];
        let mut surface = [self.min_y; 256];
        let mut need_surface = [true; 256];
        let mut need_ocean = [true; 256];
        let mut motion = [self.min_y; 256];
        let mut no_leaves = [self.min_y; 256];
        let mut need_motion = [true; 256];
        let mut need_no_leaves = [true; 256];
        let mut remaining = 1024;
        let mut cache = BTreeMap::<BlockStateId, (bool, bool, bool, bool)>::new();
        let mut classify = |id: BlockStateId| -> Result<(bool, bool, bool, bool)> {
            if let Some(flags) = cache.get(&id) {
                return Ok(*flags);
            }
            let state = self
                .world
                .state(id)
                .context("generation block state is missing")?;
            let air = matches!(
                state.name.as_str(),
                "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
            );
            let motion_tags = tags.for_block(&state.name)?;
            let has_fluid = native_world::fluid::FluidState::from_block(state)
                .map_err(anyhow::Error::msg)?
                .is_some();
            ensure!(
                !air || !motion_tags.blocks_motion,
                "air in the heightmap motion tag is unsupported"
            );
            let flags = (
                air,
                motion_tags.blocks_motion,
                motion_tags.blocks_motion_no_leaves,
                has_fluid,
            );
            cache.insert(id, flags);
            Ok(flags)
        };
        for section_y in (self.min_y.div_euclid(16)..max_y.div_euclid(16)).rev() {
            let section = chunk
                .sections
                .get(&i8::try_from(section_y)?)
                .context("generation section is missing")?;
            let blocks = section
                .blocks
                .as_ref()
                .context("generation section blocks are missing")?;
            let first = blocks[0];
            if classify(first)?.0 && blocks.iter().all(|&id| id == first) {
                continue;
            }
            for column in 0..256 {
                if !need_surface[column]
                    && !need_ocean[column]
                    && !need_motion[column]
                    && !need_no_leaves[column]
                {
                    continue;
                }
                for local_y in (0..16usize).rev() {
                    let (is_air, blocks_motion, blocks_motion_no_leaves, has_fluid) =
                        classify(blocks[column + local_y * 256])?;
                    let first_available = section_y * 16 + local_y as i32 + 1;
                    if need_surface[column] && !is_air {
                        surface[column] = first_available;
                        need_surface[column] = false;
                        remaining -= 1;
                    }
                    if need_ocean[column] && blocks_motion {
                        ocean[column] = first_available;
                        need_ocean[column] = false;
                        remaining -= 1;
                    }
                    // Java's priming loop skips air before applying any predicate.
                    if !is_air && need_motion[column] && (blocks_motion || has_fluid) {
                        motion[column] = first_available;
                        need_motion[column] = false;
                        remaining -= 1;
                    }
                    if !is_air && need_no_leaves[column] && (blocks_motion_no_leaves || has_fluid) {
                        no_leaves[column] = first_available;
                        need_no_leaves[column] = false;
                        remaining -= 1;
                    }
                    if !need_surface[column]
                        && !need_ocean[column]
                        && !need_motion[column]
                        && !need_no_leaves[column]
                    {
                        break;
                    }
                }
            }
            if remaining == 0 {
                break;
            }
        }
        self.final_heightmaps.insert(
            pos,
            Arc::new(ore::FinalHeightmaps {
                values: Box::new([surface, ocean, motion, no_leaves]),
                primed: 0b1111,
            }),
        );
        self.ore_ocean_floor_heights.insert(pos, Arc::new(ocean));
        self.feature_world_surface_heights
            .insert(pos, Arc::new(surface));
        Ok(())
    }

    /// Current generation-only values, including intentionally stale values
    /// after a direct feature section write. They are not final heightmaps.
    pub fn worldgen_heightmaps(
        &self,
        pos: native_world::loaded::ChunkPos,
    ) -> Option<(&[i32; 256], &[i32; 256])> {
        Some((
            self.feature_world_surface_heights.get(&pos)?.as_ref(),
            self.ore_ocean_floor_heights.get(&pos)?.as_ref(),
        ))
    }

    /// Encode the currently present maps without priming or recomputing them.
    /// This is one component of a future chunk save, not permission to save it.
    pub fn generated_heightmap_data(
        &self,
        pos: native_world::loaded::ChunkPos,
    ) -> Result<native_persistence::nbt::Tag> {
        use native_persistence::heightmap::{
            GeneratedHeightmaps, HeightBounds, encode_generated_heightmaps,
        };
        ensure!(
            self.world.chunks.contains_key(&pos),
            "generation chunk is missing"
        );
        let final_maps = self.final_heightmaps.get(&pos);
        let final_map = |index: usize| {
            final_maps
                .and_then(|maps| (maps.primed & (1 << index) != 0).then_some(&maps.values[index]))
        };
        let maps = GeneratedHeightmaps {
            world_surface: final_map(0),
            ocean_floor: final_map(1),
            motion_blocking: final_map(2),
            motion_blocking_no_leaves: final_map(3),
            world_surface_wg: self
                .feature_world_surface_heights
                .get(&pos)
                .map(AsRef::as_ref),
            ocean_floor_wg: self.ore_ocean_floor_heights.get(&pos).map(AsRef::as_ref),
        };
        encode_generated_heightmaps(
            HeightBounds {
                min_y: self.min_y,
                max_y: self
                    .min_y
                    .checked_add(self.height)
                    .context("generation height overflow")?,
            },
            &maps,
        )
    }

    /// Start with a generated chunk without copying its block sections.
    pub fn new(first: MaterializedTerrain) -> Self {
        let MaterializedTerrain {
            world,
            position,
            seed,
            min_y,
            height,
            postprocessing,
            noise_postprocessing,
            surface_postprocessing,
            carver_postprocessing,
        } = first;
        Self {
            world: Arc::new(world),
            seed,
            min_y,
            height,
            completed_decoration: std::collections::BTreeSet::new(),
            has_published_light: false,
            ore_ocean_floor_heights: BTreeMap::new(),
            feature_world_surface_heights: BTreeMap::new(),
            final_heightmaps: BTreeMap::new(),
            postprocessing: BTreeMap::from([(position, postprocessing)]),
            noise_postprocessing: noise_postprocessing
                .into_iter()
                .map(|value| (position, value))
                .collect(),
            surface_postprocessing: surface_postprocessing
                .into_iter()
                .map(|value| (position, value))
                .collect(),
            carver_postprocessing: carver_postprocessing
                .into_iter()
                .map(|value| (position, value))
                .collect(),
            feature_postprocessing: BTreeMap::new(),
            generated_bee_nests: Arc::new(BTreeMap::new()),
            generated_monster_entities: Arc::new(BTreeMap::new()),
            generated_entity_order: BTreeMap::new(),
            generated_pending_order: BTreeMap::new(),
            generated_entities: Arc::new(BTreeMap::new()),
        }
    }

    /// Remap a chunk's local block and biome IDs without replacing existing
    /// chunks or fluid flags. The caller must also ensure the generator and
    /// active packs match; dimension, seed and bounds alone cannot prove that.
    pub fn insert(&mut self, terrain: &MaterializedTerrain) -> Result<()> {
        ensure!(
            terrain.world.dimension == self.world.dimension,
            "terrain dimension differs"
        );
        ensure!(terrain.seed == self.seed, "terrain seed differs");
        ensure!(
            terrain.min_y == self.min_y && terrain.height == self.height,
            "terrain height range differs"
        );
        ensure!(
            !self.postprocessing.contains_key(&terrain.position),
            "terrain chunk already exists"
        );
        let inserted = Arc::make_mut(&mut self.world)
            .import_chunk(&terrain.world, terrain.position)
            .context("cannot import generated terrain chunk")?;
        ensure!(inserted, "terrain chunk already exists in world");
        self.postprocessing
            .insert(terrain.position, Arc::clone(&terrain.postprocessing));
        if let Some(noise) = &terrain.noise_postprocessing {
            self.noise_postprocessing
                .insert(terrain.position, Arc::clone(noise));
        }
        if let Some(surface) = &terrain.surface_postprocessing {
            self.surface_postprocessing
                .insert(terrain.position, Arc::clone(surface));
        }
        if let Some(carver) = &terrain.carver_postprocessing {
            self.carver_postprocessing
                .insert(terrain.position, Arc::clone(carver));
        }
        Ok(())
    }

    /// A snapshot retains its existing chunks when another chunk is inserted.
    pub fn snapshot(&self) -> Arc<native_world::loaded::World> {
        Arc::clone(&self.world)
    }

    /// Capture the visible state of a fresh, unpropagated light engine after
    /// INITIALIZE_LIGHT has registered these feature-complete chunks. The
    /// caller must provide the complete engine registration set and guarantee
    /// the registered section occupancy has not changed since registration.
    pub fn fresh_generation_light(
        &self,
        registered_chunks: impl IntoIterator<Item = native_world::loaded::ChunkPos>,
        covered_columns: impl IntoIterator<Item = native_world::loaded::ChunkPos>,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
        has_skylight: bool,
    ) -> Result<native_world::generation_light::VisibleLightSnapshot> {
        ensure!(!self.has_published_light, "light was already published");
        let registered: Vec<_> = registered_chunks.into_iter().collect();
        ensure!(
            registered
                .iter()
                .all(|pos| self.completed_decoration.contains(pos)),
            "light initialization requires completed features"
        );
        ensure!(
            self.world
                .chunks
                .values()
                .all(|chunk| !chunk.block_light_correct),
            "fresh light engine cannot contain propagated chunks"
        );
        let max_y = self
            .min_y
            .checked_add(self.height)
            .context("light height overflow")?;
        native_world::block_light::LightBounds::new(self.min_y, max_y)
            .context("unsupported light height")?;
        native_world::generation_light::VisibleLightSnapshot::fresh_after_chunk_initialization(
            &self.world,
            self.min_y.div_euclid(16)..max_y.div_euclid(16),
            registered,
            covered_columns,
            true,
            has_skylight,
            |state| generated_state_is_air(registry, state),
        )
        .context("registered light sections are missing or unsupported")
    }

    /// Calculate a draft block-light snapshot after every feature center that
    /// can write into the three-by-three input area has completed. The caller
    /// must discard the result if any of those blocks are subsequently edited.
    /// This does not initialize sky light, publish arrays, or finish the chunk.
    pub fn calculate_generated_block_light(
        &self,
        center: native_world::loaded::ChunkPos,
    ) -> Result<native_world::generation_light::VisibleLayers> {
        let bounds = self.generated_light_bounds(center)?;
        native_world::generation_block_light::initialize_generated_block_light(
            &self.world,
            bounds,
            center,
        )
        .context("generated block-light inputs are missing or unsupported")
    }

    /// Calculate draft sky light for a dimension known to have skylight.
    /// The result is invalid after subsequent block edits and is not published.
    pub fn calculate_generated_sky_light(
        &self,
        center: native_world::loaded::ChunkPos,
    ) -> Result<native_world::generation_light::VisibleLayers> {
        let bounds = self.generated_light_bounds(center)?;
        native_world::generation_sky_light::initialize_generated_sky_light(
            &self.world,
            bounds,
            center,
        )
        .context("generated sky-light inputs are missing or unsupported")
    }

    /// Publish calculated light arrays into an intermediate chunk snapshot.
    /// The caller must resolve the active dimension type's skylight capability.
    /// This neither changes the generation status nor makes the chunk saveable.
    pub fn initialize_generated_lighting(
        &mut self,
        center: native_world::loaded::ChunkPos,
        has_sky_light: bool,
    ) -> Result<()> {
        use native_world::loaded::Section;

        let block = self.calculate_generated_block_light(center)?;
        let sky = if has_sky_light {
            Some(self.calculate_generated_sky_light(center)?)
        } else {
            None
        };
        let mut chunk = (**self
            .world
            .chunks
            .get(&center)
            .context("light chunk is missing")?)
        .clone();
        for (&section_y, block_layer) in &block {
            let key = i8::try_from(section_y).context("light section outside supported range")?;
            let section = chunk.sections.entry(key).or_insert_with(|| {
                Arc::new(Section {
                    blocks: None,
                    biomes: None,
                    block_light: None,
                    sky_light: None,
                })
            });
            let section = Arc::make_mut(section);
            section.block_light = Some(Box::new(**block_layer));
            section.sky_light = match &sky {
                Some(layers) => Some(Box::new(
                    **layers
                        .get(&section_y)
                        .context("sky-light section missing")?,
                )),
                None => None,
            };
        }
        chunk.block_light_correct = true;
        Arc::make_mut(&mut self.world)
            .chunks
            .insert(center, Arc::new(chunk));
        self.has_published_light = true;
        Ok(())
    }

    /// Initialize using the cached dimension settings selected from the same
    /// world preset and active data packs as the generated terrain.
    pub fn initialize_lighting_for_dimension(
        &mut self,
        center: native_world::loaded::ChunkPos,
        settings: &native_resources::dimension_lighting::GeneratedDimensionLighting,
    ) -> Result<()> {
        let dimension = ResourceId::parse(&self.world.dimension)?;
        ensure!(
            dimension == settings.dimension,
            "lighting dimension differs from terrain"
        );
        ensure!(
            self.min_y == settings.min_y && self.height == settings.height,
            "lighting height differs from terrain"
        );
        let name = settings
            .dimension_type
            .as_ref()
            .map(|id| format!("{}:{}", id.namespace, id.path));
        if let Some(existing) = &self.world.dimension_type {
            ensure!(
                Some(existing) == name.as_ref(),
                "lighting dimension type differs from terrain"
            );
        }
        if let Some(existing) = &self.world.inline_dimension_type {
            ensure!(
                Some(existing) == settings.inline_dimension_type.as_ref(),
                "inline lighting dimension type differs from terrain"
            );
        }
        self.initialize_generated_lighting(center, settings.has_skylight)?;
        if self.world.dimension_type != name
            || self.world.inline_dimension_type != settings.inline_dimension_type
        {
            let world = Arc::make_mut(&mut self.world);
            world.dimension_type = name;
            world.inline_dimension_type = settings.inline_dimension_type.clone();
        }
        Ok(())
    }

    fn generated_light_bounds(
        &self,
        center: native_world::loaded::ChunkPos,
    ) -> Result<native_world::block_light::LightBounds> {
        for dz in -2i32..=2 {
            for dx in -2i32..=2 {
                let position = native_world::loaded::ChunkPos {
                    x: center.x.checked_add(dx).context("light chunk X overflow")?,
                    z: center.z.checked_add(dz).context("light chunk Z overflow")?,
                };
                ensure!(
                    self.completed_decoration.contains(&position),
                    "surrounding feature centers have not all completed"
                );
            }
        }
        let max_y = self
            .min_y
            .checked_add(self.height)
            .context("light height overflow")?;
        native_world::block_light::LightBounds::new(self.min_y, max_y)
            .context("unsupported generated light bounds")
    }

    /// Known nests produced by tree decoration in these intermediate chunks.
    /// Other block-entity data may still be unavailable.
    pub fn generated_bee_nests(&self) -> impl Iterator<Item = &GeneratedBeeNest> {
        self.generated_bee_nests.values()
    }

    /// Known generated chests and spawners, including unresolved chest
    /// placeholders. These intermediate chunks are not yet ready to save.
    pub fn generated_monster_entities(&self) -> impl Iterator<Item = &GeneratedMonsterRoomEntity> {
        self.generated_monster_entities
            .values()
            .filter_map(Option::as_ref)
    }

    /// Records captured by the generation path for this chunk, in insertion
    /// order. Other unimplemented feature and mob sources may still be missing.
    pub fn recorded_generated_entities(
        &self,
        pos: native_world::loaded::ChunkPos,
    ) -> Result<&[native_persistence::nbt::Tag]> {
        ensure!(
            self.world.chunks.contains_key(&pos),
            "generated entity chunk is missing"
        );
        Ok(self
            .generated_entities
            .get(&pos)
            .map(|values| values.as_slice())
            .unwrap_or_default())
    }

    /// Prepare known block entities after checking every block in this chunk.
    /// This intermediate list does not authorize saving the rest of the chunk.
    pub fn generated_block_entity_data(
        &self,
        pos: native_world::loaded::ChunkPos,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
    ) -> Result<native_persistence::nbt::Tag> {
        self.generated_block_entity_data_with(pos, |state| {
            registry
                .is_canonical_state(&state.name, &state.properties)
                .then(|| registry.has_block_entity(&state.name))
                .flatten()
        })
    }

    fn generated_block_entity_data_with(
        &self,
        pos: native_world::loaded::ChunkPos,
        mut has_entity: impl FnMut(&native_world::loaded::BlockState) -> Option<bool>,
    ) -> Result<native_persistence::nbt::Tag> {
        use native_persistence::nbt::{Tag, TagType};
        use native_world::loaded::BlockState;

        let chunk = self
            .world
            .chunks
            .get(&pos)
            .context("generated chunk is missing")?;
        ensure!(chunk.position == pos, "generated chunk position mismatch");
        ensure!(
            self.min_y.rem_euclid(16) == 0 && self.height > 0 && self.height % 16 == 0,
            "invalid generated height"
        );
        let end_y = self
            .min_y
            .checked_add(self.height)
            .context("generated height overflow")?;
        let origin_x = pos
            .x
            .checked_mul(16)
            .context("generated chunk X overflow")?;
        let origin_z = pos
            .z
            .checked_mul(16)
            .context("generated chunk Z overflow")?;
        // Positions are ordered by X, then Y and Z. Restrict iteration to
        // this chunk's X columns before checking its Z coordinate.
        let x_range = [origin_x, i32::MIN, i32::MIN]..=[origin_x + 15, i32::MAX, i32::MAX];
        let belongs_z = |p: [i32; 3]| p[2].div_euclid(16) == pos.z;
        let mut entities = BTreeMap::<[i32; 3], (BlockState, Tag)>::new();
        let mut pending_positions = std::collections::BTreeSet::new();
        for (&p, nest) in self
            .generated_bee_nests
            .range(x_range.clone())
            .filter(|(p, _)| belongs_z(**p))
        {
            ensure!(p == nest.position, "generated nest position mismatch");
            let state = self
                .world
                .block(p[0], p[1], p[2])
                .context("generated nest block is missing")?;
            ensure!(
                state.name == "minecraft:bee_nest",
                "generated nest block mismatch"
            );
            ensure!(
                entities
                    .insert(p, (state.clone(), nest.block_entity()))
                    .is_none(),
                "duplicate generated block entity"
            );
        }
        for (&p, entity) in self
            .generated_monster_entities
            .range(x_range)
            .filter(|(p, _)| belongs_z(**p))
        {
            let (state, data) = match entity.as_ref().context("generated spawner is unfinished")? {
                GeneratedMonsterRoomEntity::Chest(chest) => {
                    ensure!(p == chest.position, "generated chest position mismatch");
                    let expected = chest.block_state();
                    (
                        BlockState {
                            name: expected.name.to_string_lossy(),
                            properties: expected
                                .properties
                                .iter()
                                .map(|(k, v)| (k.to_string_lossy(), v.to_string_lossy()))
                                .collect(),
                        },
                        chest.block_entity(),
                    )
                }
                GeneratedMonsterRoomEntity::Spawner(spawner) => {
                    ensure!(p == spawner.position, "generated spawner position mismatch");
                    (
                        BlockState {
                            name: "minecraft:spawner".into(),
                            properties: BTreeMap::new(),
                        },
                        spawner.block_entity(),
                    )
                }
                GeneratedMonsterRoomEntity::CaveSpiderSpawner(spawner) => {
                    ensure!(p == spawner.position, "generated spawner position mismatch");
                    (
                        BlockState {
                            name: "minecraft:spawner".into(),
                            properties: BTreeMap::new(),
                        },
                        spawner.block_entity(),
                    )
                }
                GeneratedMonsterRoomEntity::Brushable(brushable) => {
                    ensure!(
                        p == brushable.position,
                        "generated brushable position mismatch"
                    );
                    (brushable.state.clone(), brushable.block_entity())
                }
                GeneratedMonsterRoomEntity::EmptyChest(chest) => {
                    ensure!(
                        p == chest.position,
                        "generated empty chest position mismatch"
                    );
                    (chest.state.clone(), chest.block_entity())
                }
                GeneratedMonsterRoomEntity::PendingChest { position, state } => {
                    ensure!(p == *position, "pending chest position mismatch");
                    pending_positions.insert(p);
                    (
                        state.clone(),
                        Tag::Compound(vec![
                            ("id".into(), Tag::String("DUMMY".into())),
                            ("x".into(), Tag::Int(p[0])),
                            ("y".into(), Tag::Int(p[1])),
                            ("z".into(), Tag::Int(p[2])),
                        ]),
                    )
                }
            };
            ensure!(
                entities.insert(p, (state, data)).is_none(),
                "duplicate generated block entity"
            );
        }
        let mut resolved = vec![None; self.world.state_count()];
        let mut values = BTreeMap::new();
        for sy in (self.min_y / 16)..(end_y / 16) {
            let section = chunk
                .sections
                .get(&i8::try_from(sy)?)
                .context("generated section is missing")?;
            let blocks = section
                .blocks
                .as_ref()
                .context("generated block data is missing")?;
            for (index, id) in blocks.iter().enumerate() {
                let state = self
                    .world
                    .state(*id)
                    .context("generated block state is missing")?;
                let is_entity = match resolved[id.0 as usize] {
                    Some(value) => value,
                    None => {
                        let value = has_entity(state).with_context(|| {
                            format!(
                                "generated block entity classification is unknown for {state:?}"
                            )
                        })?;
                        resolved[id.0 as usize] = Some(value);
                        value
                    }
                };
                if is_entity {
                    let p = [
                        origin_x + (index % 16) as i32,
                        sy * 16 + (index / 256) as i32,
                        origin_z + ((index / 16) % 16) as i32,
                    ];
                    let (expected, data) = entities
                        .remove(&p)
                        .context("generated block entity data is missing")?;
                    ensure!(&expected == state, "generated block entity state mismatch");
                    values.insert(p, data);
                }
            }
        }
        ensure!(
            entities.is_empty(),
            "generated block entity has no matching block"
        );
        let pending = self.generated_pending_order.get(&pos);
        ensure!(
            pending.map_or(pending_positions.is_empty(), |history| history.members()
                == &pending_positions),
            "generated pending block entity order does not match positions"
        );
        let empty_live = entity_order::LivePositions::default();
        let live = self
            .generated_entity_order
            .get(&pos)
            .map(Arc::as_ref)
            .unwrap_or(&empty_live);
        let order = live.saving_order_with_pending(pending.map(Arc::as_ref))?;
        let ordered = order
            .into_iter()
            .map(|p| {
                values
                    .remove(&p)
                    .context("generated block entity order has an unknown position")
            })
            .collect::<Result<Vec<_>>>()?;
        ensure!(
            values.is_empty(),
            "generated block entity order is missing positions"
        );
        Ok(Tag::List {
            element_type: TagType::Compound,
            values: ordered,
        })
    }

    /// Return the already-primed first-available Y without recomputing stale
    /// generation heightmaps. None means the requested map is not primed.
    pub fn primed_mob_spawn_height(
        &self,
        x: i32,
        z: i32,
        kind: native_resources::mob_spawn_placement::MobSpawnHeightmap,
    ) -> Result<Option<i32>> {
        use native_resources::mob_spawn_placement::MobSpawnHeightmap;
        let pos = native_world::loaded::ChunkPos {
            x: x.div_euclid(16),
            z: z.div_euclid(16),
        };
        ensure!(
            self.world.chunks.contains_key(&pos),
            "mob spawn heightmap chunk is missing"
        );
        let index = (x.rem_euclid(16) + z.rem_euclid(16) * 16) as usize;
        let map = match kind {
            MobSpawnHeightmap::WorldSurface => 0,
            MobSpawnHeightmap::OceanFloor => 1,
            MobSpawnHeightmap::MotionBlocking => 2,
            MobSpawnHeightmap::MotionBlockingNoLeaves => 3,
        };
        Ok(self
            .final_heightmaps
            .get(&pos)
            .filter(|maps| maps.primed & (1 << map) != 0)
            .map(|maps| maps.values[map][index]))
    }

    /// Z, X, Y order, with Y measured from this collection's minimum height.
    pub fn fluid_postprocessing(&self, pos: native_world::loaded::ChunkPos) -> Option<&[bool]> {
        self.postprocessing.get(&pos).map(AsRef::as_ref)
    }

    /// Only the ordered noise-fill marks; later generation marks are separate.
    pub fn noise_postprocessing(
        &self,
        pos: native_world::loaded::ChunkPos,
    ) -> Option<&native_core::postprocessing::PostProcessing> {
        self.noise_postprocessing.get(&pos).map(AsRef::as_ref)
    }

    /// Ordered marks through the surface stage; carver marks are not included.
    pub fn surface_postprocessing(
        &self,
        pos: native_world::loaded::ChunkPos,
    ) -> Option<&native_core::postprocessing::PostProcessing> {
        self.surface_postprocessing.get(&pos).map(AsRef::as_ref)
    }

    /// Ordered marks through the carver stage, when all prior history is known.
    pub fn carver_postprocessing(
        &self,
        pos: native_world::loaded::ChunkPos,
    ) -> Option<&native_core::postprocessing::PostProcessing> {
        self.carver_postprocessing.get(&pos).map(AsRef::as_ref)
    }

    /// Marks added by features only, in their original per-section order.
    /// The preceding carver list must be retained separately.
    pub fn feature_postprocessing(
        &self,
        pos: native_world::loaded::ChunkPos,
    ) -> Option<&native_core::postprocessing::PostProcessing> {
        self.feature_postprocessing.get(&pos).map(AsRef::as_ref)
    }

    /// Marks through the features executed so far, if the complete preceding
    /// carver history is available. The chunk may still need more features.
    pub fn postprocessing_marks_to_date(
        &self,
        pos: native_world::loaded::ChunkPos,
    ) -> Option<native_core::postprocessing::PostProcessing> {
        let mut marks = self.carver_postprocessing.get(&pos)?.as_ref().clone();
        if let Some(features) = self.feature_postprocessing.get(&pos) {
            if !marks.append(features) {
                return None;
            }
        }
        Some(marks)
    }

    /// Encode the complete retained history for this intermediate chunk.
    /// This does not verify other chunk data or permit writing a world.
    pub fn generated_postprocessing_data(
        &self,
        pos: native_world::loaded::ChunkPos,
    ) -> Result<native_persistence::nbt::Tag> {
        ensure!(
            self.world.chunks.contains_key(&pos),
            "generated chunk is missing"
        );
        let marks = self
            .postprocessing_marks_to_date(pos)
            .context("generation postprocessing history is missing")?;
        Ok(native_persistence::postprocessing::encode(&marks))
    }
}

/// Resolve supported generated block states without a generated block report.
pub fn resolve_generated_terrain_state(
    value: &Value,
    registry: &native_resources::block_registry::GeneratedBlockRegistry,
) -> Result<native_world::loaded::BlockState> {
    let (name, properties) = registry.resolve(value)?;
    Ok(native_world::loaded::BlockState { name, properties })
}

impl MaterializedTerrain {
    pub fn world(&self) -> &native_world::loaded::World {
        &self.world
    }

    /// Z, X, Y order, matching the generated terrain's original flags.
    pub fn fluid_postprocessing(&self) -> &[bool] {
        &self.postprocessing
    }

    /// Only the ordered noise-fill marks, not the later surface/carver marks.
    pub fn noise_postprocessing(&self) -> Option<&native_core::postprocessing::PostProcessing> {
        self.noise_postprocessing.as_deref()
    }

    /// Ordered marks through the surface stage; carver marks are not included.
    pub fn surface_postprocessing(&self) -> Option<&native_core::postprocessing::PostProcessing> {
        self.surface_postprocessing.as_deref()
    }

    /// Ordered marks through the carver stage, when all prior history is known.
    pub fn carver_postprocessing(&self) -> Option<&native_core::postprocessing::PostProcessing> {
        self.carver_postprocessing.as_deref()
    }

    /// Query a local X/Z and absolute Y without exposing the storage layout.
    pub fn fluid_postprocessing_at(&self, x: u8, y: i32, z: u8) -> Option<bool> {
        if x >= 16 || z >= 16 {
            return None;
        }
        let y = usize::try_from(y.checked_sub(self.min_y)?).ok()?;
        let height = usize::try_from(self.height).ok()?;
        if y >= height {
            return None;
        }
        self.postprocessing
            .get((usize::from(z) * 16 + usize::from(x)) * height + y)
            .copied()
    }

    pub fn min_y(&self) -> i32 {
        self.min_y
    }
    pub fn height(&self) -> i32 {
        self.height
    }
    /// Transfer the chunk and its unconsumed postprocessing flags together.
    pub fn into_parts(
        self,
    ) -> (
        native_world::loaded::World,
        Arc<[bool]>,
        Option<Arc<native_core::postprocessing::PostProcessing>>,
        Option<Arc<native_core::postprocessing::PostProcessing>>,
        Option<Arc<native_core::postprocessing::PostProcessing>>,
    ) {
        (
            self.world,
            self.postprocessing,
            self.noise_postprocessing,
            self.surface_postprocessing,
            self.carver_postprocessing,
        )
    }
}

/// Resolve a generated terrain-stage chunk against the active block-state
/// registry. The resolver must return the complete state, including defaults,
/// and reject identifiers or properties absent from that registry.
pub fn materialize_carved_terrain(
    terrain: SurfaceStageChunk,
    biomes: &QuartBiomeChunk,
    dimension: &str,
    mut resolve: impl FnMut(&Value) -> Result<native_world::loaded::BlockState>,
) -> Result<MaterializedTerrain> {
    use native_world::loaded::{
        BIOME_COUNT, BLOCK_COUNT, BiomeId, BlockStateId, Chunk, ChunkPos, SavedBlockEntities,
        Section, World,
    };

    let dimension = ResourceId::parse(dimension)?;
    ensure!(
        terrain.chunk_position == biomes.chunk,
        "terrain and biome chunk positions differ"
    );
    ensure!(
        terrain.min_y == biomes.min_y && terrain.height == biomes.height,
        "terrain and biome height ranges differ"
    );
    ensure!(
        terrain.height > 0 && terrain.min_y % 16 == 0 && terrain.height % 16 == 0,
        "invalid terrain height range"
    );
    let max_y = terrain
        .min_y
        .checked_add(terrain.height)
        .context("terrain height overflow")?;
    let first_section = terrain.min_y.div_euclid(16);
    let end_section = max_y.div_euclid(16);
    i8::try_from(first_section).context("first terrain section is out of range")?;
    i8::try_from(end_section - 1).context("last terrain section is out of range")?;
    for coord in terrain.chunk_position {
        coord
            .checked_mul(16)
            .and_then(|v| v.checked_add(15))
            .context("terrain chunk coordinate overflow")?;
    }
    let height = usize::try_from(terrain.height)?;
    let expected = height
        .checked_mul(256)
        .context("terrain volume size overflow")?;
    ensure!(
        terrain.states.len() == expected && terrain.fluid_updates.len() == expected,
        "invalid terrain volume length"
    );
    ensure!(
        biomes.values.len() == height / 4 * 16,
        "invalid biome volume length"
    );

    let position = ChunkPos {
        x: terrain.chunk_position[0],
        z: terrain.chunk_position[1],
    };
    let mut world = World::new(format!("{}:{}", dimension.namespace, dimension.path));
    world.set_biome_world_seed(Some(biomes.seed));
    let mut state_cache: HashMap<*const Value, BlockStateId> = HashMap::new();
    let mut biome_cache: BTreeMap<&ResourceId, BiomeId> = BTreeMap::new();
    let mut sections = BTreeMap::new();
    for section_y in first_section..end_section {
        let offset = usize::try_from(section_y * 16 - terrain.min_y)?;
        let mut blocks = Box::new([BlockStateId(0); BLOCK_COUNT]);
        for y in 0..16usize {
            for z in 0..16usize {
                for x in 0..16usize {
                    let source = &terrain.states[(z * 16 + x) * height + offset + y];
                    let key = Arc::as_ptr(source);
                    let id = if let Some(id) = state_cache.get(&key) {
                        *id
                    } else {
                        let state = resolve(source)?;
                        let id = world.intern_state(state);
                        state_cache.insert(key, id);
                        id
                    };
                    blocks[x + z * 16 + y * 256] = id;
                }
            }
        }
        let mut biome_values = Box::new([BiomeId(0); BIOME_COUNT]);
        for y in 0..4usize {
            for z in 0..4usize {
                for x in 0..4usize {
                    let biome = &biomes.values[(z * 4 + x) * (height / 4) + offset / 4 + y];
                    let id = if let Some(id) = biome_cache.get(biome) {
                        *id
                    } else {
                        let id = world.intern_biome(format!("{}:{}", biome.namespace, biome.path));
                        biome_cache.insert(biome, id);
                        id
                    };
                    biome_values[x + z * 4 + y * 16] = id;
                }
            }
        }
        sections.insert(
            i8::try_from(section_y)?,
            Arc::new(Section {
                blocks: Some(blocks),
                biomes: Some(biome_values),
                block_light: None,
                sky_light: None,
            }),
        );
    }
    world.chunks.insert(
        position,
        Arc::new(Chunk {
            position,
            status: Some("minecraft:terrain".into()),
            block_light_correct: false,
            sections,
        }),
    );
    world
        .saved_block_entities
        .insert(position, SavedBlockEntities::Unavailable);
    Ok(MaterializedTerrain {
        world,
        position,
        seed: biomes.seed,
        min_y: terrain.min_y,
        height: terrain.height,
        postprocessing: Arc::from(terrain.fluid_updates.into_boxed_slice()),
        noise_postprocessing: terrain.noise_postprocessing.map(Arc::new),
        surface_postprocessing: terrain.surface_postprocessing.map(Arc::new),
        carver_postprocessing: terrain.carver_postprocessing.map(Arc::new),
    })
}
impl QuartBiomeChunk {
    /// Construct from decoded saved biome IDs in Z, X, Y order. The caller
    /// supplies the saved world's seed and ensures IDs match its active packs.
    pub fn from_values(
        chunk: [i32; 2],
        seed: i64,
        min_y: i32,
        height: i32,
        values: Vec<ResourceId>,
    ) -> Result<Self> {
        ensure!(
            height > 0
                && min_y % 16 == 0
                && height % 16 == 0
                && min_y.checked_add(height).is_some(),
            "invalid biome height range"
        );
        let expected = usize::try_from(height / 4)?
            .checked_mul(16)
            .context("biome volume size overflow")?;
        ensure!(values.len() == expected, "invalid biome volume length");
        Ok(Self {
            chunk,
            seed,
            min_y,
            height,
            values,
        })
    }
    pub fn chunk_position(&self) -> [i32; 2] {
        self.chunk
    }
    pub fn min_y(&self) -> i32 {
        self.min_y
    }
    pub fn height(&self) -> i32 {
        self.height
    }
    pub fn values(&self) -> &[ResourceId] {
        &self.values
    }
    fn at(&self, quart: IVec3) -> Option<&ResourceId> {
        if quart.x.div_euclid(4) != self.chunk[0] || quart.z.div_euclid(4) != self.chunk[1] {
            return None;
        }
        let min = self.min_y.div_euclid(4);
        let height = self.height / 4;
        let y = quart.y.clamp(min, min + height - 1) - min;
        let x = quart.x.rem_euclid(4) as usize;
        let z = quart.z.rem_euclid(4) as usize;
        self.values.get((z * 4 + x) * height as usize + y as usize)
    }
}

/// Biome chunks actually generated or loaded for the surrounding region.
/// All entries must come from the same active data packs as the generator.
pub struct BiomeNeighborhood {
    seed: i64,
    zoom_seed: i64,
    min_y: i32,
    height: i32,
    chunks: BTreeMap<[i32; 2], Arc<QuartBiomeChunk>>,
}

/// Selected decoration features for one chunk. Structure placements still
/// have to run before the corresponding feature step.
pub struct DecorationPlan<'a> {
    origin: [i32; 3],
    decoration_seed: i64,
    features: Vec<SelectedFeature<'a>>,
    schedule: &'a FeatureSchedule,
    biomes: &'a BiomeNeighborhood,
    generator_min_y: i32,
    generator_height: i32,
    sea_level: i32,
}

/// Indices into the referenced-structure plan and selected-feature list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecorationAction {
    Structure(usize),
    Feature(usize),
}

fn ordered_decoration_actions(
    structures: &[(i32, i32)],
    features: &[(i32, i32)],
) -> Result<Vec<DecorationAction>> {
    for entries in [structures, features] {
        ensure!(
            entries.iter().all(|&(step, index)| step >= 0 && index >= 0),
            "negative decoration index"
        );
        ensure!(
            entries.windows(2).all(|pair| pair[0] < pair[1]),
            "decoration entries are not strictly ordered"
        );
    }
    let mut output = Vec::with_capacity(structures.len() + features.len());
    let (mut s, mut f) = (0, 0);
    while s < structures.len() || f < features.len() {
        if s < structures.len() && (f == features.len() || structures[s].0 <= features[f].0) {
            output.push(DecorationAction::Structure(s));
            s += 1;
        } else {
            output.push(DecorationAction::Feature(f));
            f += 1;
        }
    }
    Ok(output)
}

impl DecorationPlan<'_> {
    pub fn origin(&self) -> [i32; 3] {
        self.origin
    }
    pub fn decoration_seed(&self) -> i64 {
        self.decoration_seed
    }
    pub fn features(&self) -> &[SelectedFeature<'_>] {
        &self.features
    }
    /// Java reseeds the shared decoration RNG for every selected feature.
    pub fn feature_random(&self, feature: &SelectedFeature<'_>) -> DecorationRandom {
        let mut random = DecorationRandom::new(0);
        random.set_feature_seed(self.decoration_seed, feature.index, feature.step);
        random
    }

    /// Java places all referenced structures before selected features in each
    /// decoration step, retaining their separate registry-derived seed indices.
    pub fn actions_with_structures(
        &self,
        structures: &[structure_batch::PlannedStructureDecoration<'_>],
    ) -> Result<Vec<DecorationAction>> {
        ensure!(
            structures
                .iter()
                .all(|structure| !structure.starts.is_empty()),
            "planned structure has no starts"
        );
        let structures = structures
            .iter()
            .map(|structure| (structure.entry.step, structure.entry.index))
            .collect::<Vec<_>>();
        let features = self
            .features
            .iter()
            .map(|feature| (feature.step, feature.index))
            .collect::<Vec<_>>();
        ordered_decoration_actions(&structures, &features)
    }

    /// Interleave a complete mixed structure plan with the selected features.
    pub fn actions_with_mixed_structures(
        &self,
        structures: &[structure_batch::PlannedMixedStructureDecoration<'_>],
    ) -> Result<Vec<DecorationAction>> {
        ensure!(
            structures
                .iter()
                .all(|structure| !structure.starts.is_empty()),
            "planned structure has no starts"
        );
        let structures = structures
            .iter()
            .map(|structure| (structure.entry.step, structure.entry.index))
            .collect::<Vec<_>>();
        let features = self
            .features
            .iter()
            .map(|feature| (feature.step, feature.index))
            .collect::<Vec<_>>();
        ordered_decoration_actions(&structures, &features)
    }
}
impl BiomeNeighborhood {
    pub fn new(seed: i64, min_y: i32, height: i32) -> Result<Self> {
        ensure!(
            height > 0
                && min_y % 16 == 0
                && height % 16 == 0
                && min_y.checked_add(height).is_some(),
            "invalid biome height range"
        );
        Ok(Self {
            seed,
            zoom_seed: obfuscate_seed(seed),
            min_y,
            height,
            chunks: BTreeMap::new(),
        })
    }
    pub fn insert(&mut self, chunk: Arc<QuartBiomeChunk>) -> Result<()> {
        ensure!(
            chunk.seed == self.seed && chunk.min_y == self.min_y && chunk.height == self.height,
            "biome chunk seed or height does not match the region"
        );
        ensure!(
            !self.chunks.contains_key(&chunk.chunk),
            "duplicate biome chunk"
        );
        self.chunks.insert(chunk.chunk, chunk);
        Ok(())
    }
    pub fn chunk(&self, position: [i32; 2]) -> Option<&Arc<QuartBiomeChunk>> {
        self.chunks.get(&position)
    }
    fn validate(&self, center: [i32; 2], seed: i64, min_y: i32, height: i32) -> Result<()> {
        ensure!(
            self.seed == seed && self.min_y == min_y && self.height == height,
            "biome neighborhood does not match generator"
        );
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                let x = center[0]
                    .checked_add(dx)
                    .context("neighbor chunk X overflow")?;
                let z = center[1]
                    .checked_add(dz)
                    .context("neighbor chunk Z overflow")?;
                ensure!(
                    self.chunks.contains_key(&[x, z]),
                    "missing biome neighbor {x},{z}"
                );
            }
        }
        Ok(())
    }
    /// Apply Java's seeded biome zoom and clamp the selected quart Y.
    pub fn biome_at_block(&self, x: i32, y: i32, z: i32) -> Result<&ResourceId> {
        let quart = quart_position(self.zoom_seed, IVec3::new(x, y, z));
        let key = [quart.x.div_euclid(4), quart.z.div_euclid(4)];
        self.chunks
            .get(&key)
            .with_context(|| format!("missing biome neighbor {},{}", key[0], key[1]))?
            .at(quart)
            .context("biome coordinate missing from neighbor")
    }

    /// Direct noise-biome lookup, without the seeded block-biome zoom.
    pub fn noise_biome_at_block(&self, x: i32, y: i32, z: i32) -> Result<&ResourceId> {
        let quart = IVec3::new(x.div_euclid(4), y.div_euclid(4), z.div_euclid(4));
        let key = [quart.x.div_euclid(4), quart.z.div_euclid(4)];
        self.chunks
            .get(&key)
            .with_context(|| format!("missing biome neighbor {},{}", key[0], key[1]))?
            .at(quart)
            .context("biome coordinate missing from neighbor")
    }
}

/// A supported structure start, retaining every piece in decoration order.
#[derive(Clone)]
pub enum PreparedStructureStart {
    Simple(SimpleStructureStart),
    OceanRuin(ocean_ruin::PreparedStart),
    Mineshaft(mineshaft::PreparedStart),
    Jigsaw(jigsaw::PreparedStart),
}

/// Batch-only extension; decoration cannot consume jigsaw pieces yet.
pub(super) enum BatchPreparedStructureStart {
    Supported(PreparedStructureStart),
    Jigsaw(jigsaw::PreparedStart),
}

/// A generator configured from the dimension's active biome source and noise
/// settings. It must be used with the same data-pack stack throughout its life.
pub struct TerrainGenerator {
    /// Identifies generators compiled together from one immutable input stack.
    pair_identity: Arc<()>,
    noise: NoiseStageGenerator,
    spawn_target: Option<Value>,
    disable_mob_generation: Option<bool>,
    structure_context: bool,
    source: BiomeSource,
    possible_biomes: std::collections::BTreeSet<ResourceId>,
    possible_biomes_ordered: Vec<ResourceId>,
    surface: SurfaceRule,
    seed: i64,
    min_y: i32,
    height: i32,
}

struct StructureStartEnvironment<'a> {
    noise: &'a NoiseStageGenerator,
    evaluator: &'a mut DensityEvaluator,
    biome_rules: &'a native_resources::structure_biomes::StructureBiomes,
    structure: &'a ResourceId,
    column_source: BiomeSource,
    point_source: BiomeSource,
    climate: Option<ClimateVolume>,
    columns: &'a mut HashMap<[i32; 2], BaseNoiseColumn>,
    flags: &'a [BlockHeightFlags; 4],
    min_y: i32,
    max_y: i32,
}
impl StartEnvironment for StructureStartEnvironment<'_> {
    fn vertical_bounds(&self) -> (i32, i32) {
        (self.min_y, self.max_y)
    }
    fn sea_level(&self) -> i32 {
        self.noise.sea_level()
    }
    fn first_occupied_height(&mut self, x: i32, z: i32, map: Heightmap) -> Result<i32> {
        if !self.columns.contains_key(&[x, z]) {
            self.columns
                .insert([x, z], self.noise.base_column(self.evaluator, x, z)?);
        }
        self.columns[&[x, z]].first_occupied_height(self.min_y, |material| {
            let index = match material {
                NoiseMaterial::DefaultBlock => 0,
                NoiseMaterial::DefaultFluid => 1,
                NoiseMaterial::Lava => 2,
                NoiseMaterial::Air => 3,
            };
            let flags = self.flags[index];
            Ok(match map {
                Heightmap::WorldSurface => !flags.is_air,
                Heightmap::OceanFloor => flags.blocks_motion,
            })
        })
    }
    fn biome_allowed(&mut self, quart: [i32; 3], query: BiomeQuery) -> Result<bool> {
        let biome = match query {
            BiomeQuery::Point => {
                self.point_source
                    .sample_quart(self.noise.router(), self.evaluator, quart)
            }
            BiomeQuery::Column if self.column_source.uses_bulk_climate() => {
                if self.climate.is_none() {
                    let first = self
                        .min_y
                        .checked_sub(1)
                        .context("biome column minimum overflow")?
                        .div_euclid(4);
                    let last = self.max_y.div_euclid(4);
                    self.climate = Some(ClimateVolume::sample(
                        self.noise.router(),
                        self.evaluator,
                        [quart[0], first, quart[2]],
                        [1, u32::try_from(last - first + 1)?, 1],
                    )?);
                }
                self.climate
                    .as_ref()
                    .unwrap()
                    .select(&mut self.column_source, quart)?
            }
            BiomeQuery::Column => {
                self.column_source
                    .sample_quart(self.noise.router(), self.evaluator, quart)
            }
        };
        self.biome_rules.permits(self.structure, biome)
    }
}

impl mineshaft::MineshaftEnvironment for StructureStartEnvironment<'_> {
    fn generator_min_y(&self) -> i32 {
        self.noise.router().min_y
    }
}

impl jigsaw::Environment for StructureStartEnvironment<'_> {
    fn dimension_bounds(&self) -> (i32, i32) {
        (self.min_y, self.max_y)
    }

    fn height_context(&self) -> native_resources::height_provider::HeightContext {
        native_resources::height_provider::HeightContext {
            min_y: self.min_y.max(self.noise.router().min_y),
            height: (self.max_y - self.min_y + 1).min(self.noise.router().height),
            sea_level: self.noise.sea_level(),
        }
    }

    fn biome_allowed(&mut self, quart: [i32; 3], query: BiomeQuery) -> Result<bool> {
        StartEnvironment::biome_allowed(self, quart, query)
    }

    fn first_free_height(
        &mut self,
        x: i32,
        z: i32,
        map: native_resources::jigsaw_placement::StartHeightmap,
    ) -> Result<i32> {
        use native_resources::jigsaw_placement::StartHeightmap;
        if !self.columns.contains_key(&[x, z]) {
            self.columns
                .insert([x, z], self.noise.base_column(self.evaluator, x, z)?);
        }
        self.columns[&[x, z]]
            .first_occupied_height(self.min_y, |material| {
                let index = match material {
                    NoiseMaterial::DefaultBlock => 0,
                    NoiseMaterial::DefaultFluid => 1,
                    NoiseMaterial::Lava => 2,
                    NoiseMaterial::Air => 3,
                };
                let flags = self.flags[index];
                Ok(match map {
                    StartHeightmap::WorldSurface | StartHeightmap::WorldSurfaceWg => !flags.is_air,
                    StartHeightmap::OceanFloor | StartHeightmap::OceanFloorWg => {
                        flags.blocks_motion
                    }
                    StartHeightmap::MotionBlocking => flags.blocks_motion || flags.has_fluid,
                    StartHeightmap::MotionBlockingNoLeaves => {
                        flags.blocks_motion_no_leaves || flags.has_fluid
                    }
                })
            })?
            .checked_add(1)
            .context("jigsaw base height overflow")
    }
}

impl TerrainGenerator {
    /// The caller must guarantee no old-chunk blending or structure adjustment.
    /// The dimension must fully contain the noise-settings height range.
    pub fn without_terrain_adjustments(
        provider: &mut impl ResourceProvider,
        definition: &Value,
        seed: i64,
        min_y: i32,
        height: i32,
    ) -> Result<Self> {
        Self::build(provider, definition, seed, min_y, height, false)
    }
    /// Compile a generator accepting per-chunk structure contexts. Old-chunk blending is absent.
    pub fn with_structure_context(
        provider: &mut impl ResourceProvider,
        definition: &Value,
        seed: i64,
        min_y: i32,
        height: i32,
    ) -> Result<Self> {
        Self::build(provider, definition, seed, min_y, height, true)
    }
    /// Compile independent structure and terrain generators from the same
    /// immutable resource stack. Their biome caches can advance independently.
    pub fn paired_with_structure_context(
        provider: &mut impl ResourceProvider,
        definition: &Value,
        seed: i64,
        min_y: i32,
        height: i32,
    ) -> Result<(Self, Self)> {
        let first = Self::with_structure_context(provider, definition, seed, min_y, height)?;
        let mut second = Self::with_structure_context(provider, definition, seed, min_y, height)?;
        second.pair_identity = Arc::clone(&first.pair_identity);
        Ok((first, second))
    }
    fn build(
        provider: &mut impl ResourceProvider,
        definition: &Value,
        seed: i64,
        min_y: i32,
        height: i32,
        structure_context: bool,
    ) -> Result<Self> {
        let kind = ResourceId::parse(
            definition
                .get("type")
                .and_then(Value::as_str)
                .context("missing chunk generator type")?,
        )?;
        ensure!(
            kind.namespace == "minecraft" && kind.path == "noise",
            "unsupported chunk generator"
        );
        let source = BiomeSource::load(
            provider,
            definition
                .get("biome_source")
                .context("missing biome source")?,
        )?;
        let settings = definition
            .get("settings")
            .context("missing noise settings")?;
        let bytes = if let Some(name) = settings.as_str() {
            let id = ResourceId::parse(name)?;
            let path = format!(
                "data/{}/worldgen/noise_settings/{}.json",
                id.namespace, id.path
            );
            provider
                .read_data(&path)?
                .with_context(|| format!("missing noise settings {name}"))?
        } else {
            serde_json::to_vec(settings)?
        };
        let parsed: Value = serde_json::from_slice(&bytes).context("invalid noise settings")?;
        let disable_mob_generation = parsed
            .get("disable_mob_generation")
            .map(|value| {
                value
                    .as_bool()
                    .context("disable_mob_generation must be boolean")
            })
            .transpose()?;
        let mut compiler = SurfaceRuleCompiler::new(provider);
        let surface = compiler.compile(
            parsed
                .get("material_rule")
                .context("missing surface material rule")?,
        )?;
        drop(compiler);
        let noise = if structure_context {
            NoiseStageGenerator::from_json_with_structure_context(provider, &bytes, seed)?
        } else {
            NoiseStageGenerator::from_json_without_terrain_adjustments(provider, &bytes, seed)?
        };
        let end = min_y
            .checked_add(height)
            .context("dimension height overflow")?;
        let noise_end = noise
            .router()
            .min_y
            .checked_add(noise.router().height)
            .context("noise height overflow")?;
        ensure!(
            height > 0
                && height % 16 == 0
                && min_y % 16 == 0
                && noise.router().height > 0
                && min_y <= noise.router().min_y
                && end >= noise_end
                && height <= 4096,
            "dimension must fully contain the noise height range"
        );
        let possible_biomes = source.possible_biomes();
        let possible_biomes_ordered = source.possible_biomes_in_order();
        Ok(Self {
            pair_identity: Arc::new(()),
            noise,
            spawn_target: parsed.get("spawn_target").cloned(),
            disable_mob_generation,
            structure_context,
            source,
            possible_biomes,
            possible_biomes_ordered,
            surface,
            seed,
            min_y,
            height,
        })
    }
    pub fn min_y(&self) -> i32 {
        self.min_y
    }
    pub fn height(&self) -> i32 {
        self.height
    }
    /// NoiseBasedChunkGenerator inherits ChunkGenerator's fixed spawn height.
    pub fn spawn_height(&self) -> i32 {
        64
    }
    pub fn sea_level(&self) -> i32 {
        self.noise.sea_level()
    }
    /// Absent in incomplete synthetic settings; Java requires this field.
    pub fn disable_mob_generation(&self) -> Option<bool> {
        self.disable_mob_generation
    }
    /// Create the random source for one new WorldGenRegion centered on this
    /// chunk. Reuse it throughout that region's generation step.
    pub fn region_random(
        &self,
        chunk: native_world::loaded::ChunkPos,
    ) -> native_resources::worldgen_region_random::RegionRandom {
        native_resources::worldgen_region_random::RegionRandom::new(
            self.seed,
            self.noise.router().random,
            chunk.x,
            chunk.z,
        )
    }
    /// Compute the initial origin with the same immutable data-pack stack
    /// used to build this generator, before requesting generated chunks.
    pub fn spawn_origin(&self, provider: &mut impl ResourceProvider) -> Result<[i32; 2]> {
        let target = self
            .spawn_target
            .as_ref()
            .context("noise settings have no spawn_target")?;
        let compiled = native_resources::spawn_origin::NoiseSpawnTargets::compile(
            provider,
            target,
            self.seed,
            self.noise.router().random,
        )?;
        Ok(compiled.origin_chunk(&mut DensityEvaluator::default()))
    }
    pub fn biome_neighborhood(&self) -> Result<BiomeNeighborhood> {
        BiomeNeighborhood::new(self.seed, self.min_y(), self.height())
    }
    /// Complete set of biome IDs exposed by this generator's biome source.
    pub fn possible_biomes(&self) -> &std::collections::BTreeSet<ResourceId> {
        &self.possible_biomes
    }
    /// Compile the generator's global decoration order from the active biome
    /// data and validated placed-feature registry, preserving source order.
    pub fn feature_schedule(
        &self,
        provider: &mut impl ResourceProvider,
        registry: &std::collections::BTreeSet<ResourceId>,
    ) -> Result<native_resources::feature_order::FeatureSchedule> {
        native_resources::feature_order::FeatureSchedule::load(
            provider,
            &self.possible_biomes_ordered,
            registry,
        )
    }
    /// Prepare Java's feature subset from all biome cells in the 3x3 chunk
    /// neighborhood. The schedule and biome chunks must use the same active
    /// packs and generator settings; source order and bounds are checked here.
    pub fn decoration_plan<'a>(
        &self,
        schedule: &'a FeatureSchedule,
        neighborhood: &'a BiomeNeighborhood,
        target: [i32; 2],
    ) -> Result<DecorationPlan<'a>> {
        ensure!(
            schedule.possible_biomes() == self.possible_biomes_ordered,
            "feature schedule uses a different biome source"
        );
        neighborhood.validate(target, self.seed, self.min_y, self.height)?;
        let mut present = std::collections::BTreeSet::new();
        for dz in -1..=1 {
            for dx in -1..=1 {
                let key = [
                    target[0].checked_add(dx).context("neighbor X overflow")?,
                    target[1].checked_add(dz).context("neighbor Z overflow")?,
                ];
                present.extend(neighborhood.chunks[&key].values.iter());
            }
        }
        let origin = [
            target[0].checked_mul(16).context("decoration X overflow")?,
            self.min_y,
            target[1].checked_mul(16).context("decoration Z overflow")?,
        ];
        let mut random = DecorationRandom::new(0);
        let decoration_seed = random.set_decoration_seed(self.seed, origin[0], origin[2]);
        Ok(DecorationPlan {
            origin,
            decoration_seed,
            features: schedule.select(present),
            schedule,
            biomes: neighborhood,
            generator_min_y: self.noise.router().min_y,
            generator_height: self.noise.router().height,
            sea_level: self.noise.sea_level(),
        })
    }
    /// Check a set's candidate chunk after the biome-source filter, including
    /// concentric rings and exclusion references. This does not generate starts.
    pub fn is_structure_candidate<P: ResourceProvider>(
        &self,
        provider: &mut P,
        evaluator: &mut DensityEvaluator,
        cache: &mut native_resources::structure_placement::StructureSetCache,
        biome_rules: &native_resources::structure_biomes::StructureBiomes,
        id: &ResourceId,
        chunk: [i32; 2],
    ) -> Result<bool> {
        cache.is_structure_chunk_for_biomes_with_rings(
            provider,
            biome_rules,
            &self.possible_biomes,
            id,
            self.seed,
            chunk,
            |provider, _, placement| {
                self.concentric_structure_positions(provider, evaluator, placement)
            },
        )
    }
    /// Select one set entry and invoke its start generator in Java's order.
    /// Structure-specific generation must still implement its own biome and
    /// height checks; an unsupported type must return an error.
    #[allow(clippy::too_many_arguments)]
    pub fn try_generate_structure_set<P: ResourceProvider, T>(
        &self,
        provider: &mut P,
        evaluator: &mut DensityEvaluator,
        cache: &mut native_resources::structure_placement::StructureSetCache,
        biome_rules: &native_resources::structure_biomes::StructureBiomes,
        id: &ResourceId,
        chunk: [i32; 2],
        existing: impl FnMut(&ResourceId) -> Result<bool>,
        generate: impl FnMut(
            &mut P,
            &native_resources::structure_placement::StructureEntry,
        ) -> Result<Option<T>>,
    ) -> Result<native_resources::structure_placement::StructureSetStart<T>> {
        cache.try_generate_start(
            provider,
            biome_rules,
            &self.possible_biomes,
            id,
            self.seed,
            chunk,
            existing,
            generate,
            |provider, _, placement| {
                self.concentric_structure_positions(provider, evaluator, placement)
            },
        )
    }
    /// Prepare jigsaw starts after structure-set selection using the active
    /// biome source and unadjusted terrain. This does not place or save pieces.
    #[allow(clippy::too_many_arguments)]
    pub fn try_generate_jigsaw_structure_set<P: ResourceProvider>(
        &self,
        provider: &mut P,
        evaluator: &mut DensityEvaluator,
        cache: &mut native_resources::structure_placement::StructureSetCache,
        biome_rules: &native_resources::structure_biomes::StructureBiomes,
        id: &ResourceId,
        chunk: [i32; 2],
        existing: impl FnMut(&ResourceId) -> Result<bool>,
        mut classify: impl FnMut(&Value) -> Result<BlockHeightFlags>,
    ) -> Result<native_resources::structure_placement::StructureSetStart<jigsaw::PreparedStart>>
    {
        let mut columns = HashMap::new();
        let mut structure_evaluator = DensityEvaluator::default();
        let mut flags = None;
        let max_y = self
            .min_y
            .checked_add(self.height - 1)
            .context("dimension height overflow")?;
        self.try_generate_structure_set(
            provider,
            evaluator,
            cache,
            biome_rules,
            id,
            chunk,
            existing,
            |provider, entry| {
                let path = format!(
                    "data/{}/worldgen/structure/{}.json",
                    entry.id.namespace, entry.id.path
                );
                let bytes = provider
                    .read_data(&path)?
                    .with_context(|| format!("missing structure {path}"))?;
                let definition: Value = serde_json::from_slice(&bytes)
                    .with_context(|| format!("invalid structure {path}"))?;
                if flags.is_none() {
                    flags = Some([
                        classify(self.noise.default_block())?,
                        classify(self.noise.default_fluid())?,
                        classify(&serde_json::json!("minecraft:lava"))?,
                        classify(&serde_json::json!("minecraft:air"))?,
                    ]);
                }
                let mut env = StructureStartEnvironment {
                    noise: &self.noise,
                    evaluator: &mut structure_evaluator,
                    biome_rules,
                    structure: &entry.id,
                    column_source: self.source.fresh_sampler(),
                    point_source: self.source.fresh_sampler(),
                    climate: None,
                    columns: &mut columns,
                    flags: flags.as_ref().unwrap(),
                    min_y: self.min_y,
                    max_y,
                };
                jigsaw::PreparedStart::generate(provider, &definition, &mut env, self.seed, chunk)
                    .with_context(|| format!("in structure {path}"))
            },
        )
    }

    /// Attempt the supported simple structure types using this generator's
    /// unadjusted base heights and noise-biome source. Other structure types
    /// return an error, so they cannot silently lose their generation attempt.
    /// The caller resolves air and heightmap tags from the active block registry.
    #[allow(clippy::too_many_arguments)]
    pub fn try_generate_simple_structure_set<P: ResourceProvider>(
        &self,
        provider: &mut P,
        evaluator: &mut DensityEvaluator,
        cache: &mut native_resources::structure_placement::StructureSetCache,
        biome_rules: &native_resources::structure_biomes::StructureBiomes,
        id: &ResourceId,
        chunk: [i32; 2],
        existing: impl FnMut(&ResourceId) -> Result<bool>,
        classify: impl FnMut(&Value) -> Result<BlockHeightFlags>,
    ) -> Result<native_resources::structure_placement::StructureSetStart<SimpleStructureStart>>
    {
        use native_resources::structure_placement::StructureSetStart;
        Ok(
            match self.try_generate_supported_structure_set_impl(
                provider,
                evaluator,
                cache,
                biome_rules,
                id,
                chunk,
                existing,
                classify,
                false,
                false,
            )? {
                StructureSetStart::Existing => StructureSetStart::Existing,
                StructureSetStart::NotCandidate => StructureSetStart::NotCandidate,
                StructureSetStart::Rejected => StructureSetStart::Rejected,
                StructureSetStart::Generated {
                    structure,
                    start:
                        BatchPreparedStructureStart::Supported(PreparedStructureStart::Simple(start)),
                } => StructureSetStart::Generated { structure, start },
                StructureSetStart::Generated {
                    start:
                        BatchPreparedStructureStart::Supported(
                            PreparedStructureStart::OceanRuin(_)
                            | PreparedStructureStart::Mineshaft(_)
                            | PreparedStructureStart::Jigsaw(_),
                        )
                        | BatchPreparedStructureStart::Jigsaw(_),
                    ..
                } => unreachable!("ocean ruins are disabled"),
            },
        )
    }

    /// Generate supported simple and ocean-ruin starts after normal structure
    /// set selection. Other eligible types still fail rather than disappear.
    #[allow(clippy::too_many_arguments)]
    pub fn try_generate_supported_structure_set<P: ResourceProvider>(
        &self,
        provider: &mut P,
        evaluator: &mut DensityEvaluator,
        cache: &mut native_resources::structure_placement::StructureSetCache,
        biome_rules: &native_resources::structure_biomes::StructureBiomes,
        id: &ResourceId,
        chunk: [i32; 2],
        existing: impl FnMut(&ResourceId) -> Result<bool>,
        classify: impl FnMut(&Value) -> Result<BlockHeightFlags>,
    ) -> Result<native_resources::structure_placement::StructureSetStart<PreparedStructureStart>>
    {
        use native_resources::structure_placement::StructureSetStart;
        Ok(
            match self.try_generate_supported_structure_set_impl(
                provider,
                evaluator,
                cache,
                biome_rules,
                id,
                chunk,
                existing,
                classify,
                true,
                false,
            )? {
                StructureSetStart::Existing => StructureSetStart::Existing,
                StructureSetStart::NotCandidate => StructureSetStart::NotCandidate,
                StructureSetStart::Rejected => StructureSetStart::Rejected,
                StructureSetStart::Generated {
                    structure,
                    start: BatchPreparedStructureStart::Supported(start),
                } => StructureSetStart::Generated { structure, start },
                StructureSetStart::Generated {
                    start: BatchPreparedStructureStart::Jigsaw(_),
                    ..
                } => unreachable!("jigsaws are disabled"),
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn try_generate_batch_structure_set<P: ResourceProvider>(
        &self,
        provider: &mut P,
        evaluator: &mut DensityEvaluator,
        cache: &mut native_resources::structure_placement::StructureSetCache,
        biome_rules: &native_resources::structure_biomes::StructureBiomes,
        id: &ResourceId,
        chunk: [i32; 2],
        existing: impl FnMut(&ResourceId) -> Result<bool>,
        classify: impl FnMut(&Value) -> Result<BlockHeightFlags>,
    ) -> Result<native_resources::structure_placement::StructureSetStart<BatchPreparedStructureStart>>
    {
        self.try_generate_supported_structure_set_impl(
            provider,
            evaluator,
            cache,
            biome_rules,
            id,
            chunk,
            existing,
            classify,
            true,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn try_generate_supported_structure_set_impl<P: ResourceProvider>(
        &self,
        provider: &mut P,
        evaluator: &mut DensityEvaluator,
        cache: &mut native_resources::structure_placement::StructureSetCache,
        biome_rules: &native_resources::structure_biomes::StructureBiomes,
        id: &ResourceId,
        chunk: [i32; 2],
        existing: impl FnMut(&ResourceId) -> Result<bool>,
        mut classify: impl FnMut(&Value) -> Result<BlockHeightFlags>,
        ocean_ruins: bool,
        jigsaws: bool,
    ) -> Result<native_resources::structure_placement::StructureSetStart<BatchPreparedStructureStart>>
    {
        let mut columns = HashMap::new();
        let mut structure_evaluator = DensityEvaluator::default();
        let mut flags = None;
        let max_y = self
            .min_y
            .checked_add(self.height - 1)
            .context("dimension height overflow")?;
        self.try_generate_structure_set(
            provider,
            evaluator,
            cache,
            biome_rules,
            id,
            chunk,
            existing,
            |provider, entry| {
                let path = format!(
                    "data/{}/worldgen/structure/{}.json",
                    entry.id.namespace, entry.id.path
                );
                let bytes = provider
                    .read_data(&path)?
                    .with_context(|| format!("missing structure {path}"))?;
                let definition: Value = serde_json::from_slice(&bytes)
                    .with_context(|| format!("invalid structure {path}"))?;
                let center_heightmap =
                    native_resources::structure_start::template_structure_center_heightmap(
                        &definition,
                    )
                    .with_context(|| format!("in structure {path}"))?;
                let structure_type = ResourceId::parse(
                    definition
                        .get("type")
                        .and_then(Value::as_str)
                        .context("missing structure type")?,
                )?;
                let mineshaft = ocean_ruins
                    && structure_type.namespace == "minecraft"
                    && structure_type.path == "mineshaft";
                let jigsaw = jigsaws
                    && structure_type.namespace == "minecraft"
                    && structure_type.path == "jigsaw";
                let structure = if center_heightmap.is_none() && !mineshaft && !jigsaw {
                    Some(
                        SimpleStructure::decode(&definition)
                            .with_context(|| format!("in structure {path}"))?,
                    )
                } else {
                    None
                };
                if flags.is_none() {
                    flags = Some([
                        classify(self.noise.default_block())?,
                        classify(self.noise.default_fluid())?,
                        classify(&serde_json::json!("minecraft:lava"))?,
                        classify(&serde_json::json!("minecraft:air"))?,
                    ]);
                }
                let mut env = StructureStartEnvironment {
                    noise: &self.noise,
                    evaluator: &mut structure_evaluator,
                    biome_rules,
                    structure: &entry.id,
                    column_source: self.source.fresh_sampler(),
                    point_source: self.source.fresh_sampler(),
                    climate: None,
                    columns: &mut columns,
                    flags: flags.as_ref().unwrap(),
                    min_y: self.min_y,
                    max_y,
                };
                if jigsaw {
                    return Ok(jigsaw::PreparedStart::generate(
                        provider,
                        &definition,
                        &mut env,
                        self.seed,
                        chunk,
                    )?
                    .map(BatchPreparedStructureStart::Jigsaw));
                }
                if mineshaft {
                    return Ok(mineshaft::PreparedStart::generate(
                        &definition,
                        &mut env,
                        self.seed,
                        chunk,
                    )?
                    .map(|start| {
                        BatchPreparedStructureStart::Supported(PreparedStructureStart::Mineshaft(
                            start,
                        ))
                    }));
                }
                if ocean_ruins
                    && structure_type.namespace == "minecraft"
                    && structure_type.path == "ocean_ruin"
                {
                    return Ok(ocean_ruin::PreparedStart::generate(
                        provider,
                        &definition,
                        &mut env,
                        self.seed,
                        chunk,
                    )?
                    .map(|start| {
                        BatchPreparedStructureStart::Supported(PreparedStructureStart::OceanRuin(
                            start,
                        ))
                    }));
                }
                if let Some(heightmap) = center_heightmap
                    && native_resources::structure_start::center_generation_point(
                        &mut env, chunk, heightmap,
                    )?
                    .is_none()
                {
                    return Ok(None);
                }
                let structure = match structure {
                    Some(structure) => structure,
                    None => SimpleStructure::decode(&definition)
                        .with_context(|| format!("in structure {path}"))?,
                };
                Ok(structure
                    .generate(&mut env, self.seed, chunk)?
                    .map(|start| {
                        BatchPreparedStructureStart::Supported(PreparedStructureStart::Simple(
                            start,
                        ))
                    }))
            },
        )
    }
    /// Resolve concentric-ring candidates against the active biome source.
    /// Per-search history is isolated; as with Java's worker-local climate
    /// cache, exact ties may depend on the prior history of that worker.
    pub fn concentric_structure_positions(
        &self,
        provider: &mut impl ResourceProvider,
        evaluator: &mut DensityEvaluator,
        placement: &native_resources::structure_placement::ConcentricRingsPlacement,
    ) -> Result<Vec<[i32; 2]>> {
        let previous = evaluator.replace_structure_terrain_context(None);
        let result = self.concentric_structure_positions_unadjusted(provider, evaluator, placement);
        evaluator.replace_structure_terrain_context(previous);
        result
    }
    fn concentric_structure_positions_unadjusted(
        &self,
        provider: &mut impl ResourceProvider,
        evaluator: &mut DensityEvaluator,
        placement: &native_resources::structure_placement::ConcentricRingsPlacement,
    ) -> Result<Vec<[i32; 2]>> {
        let selector = placement.preferred_biomes();
        let allowed = native_resources::surface_biomes::SurfaceBiomes::load(
            provider,
            std::slice::from_ref(selector),
        )?;
        let possible = &self.possible_biomes;
        let mut preferred = std::collections::BTreeSet::new();
        for biome in possible {
            if allowed.matches(selector, biome)? {
                preferred.insert(biome.clone());
            }
        }
        let all_preferred = preferred.len() == possible.len();
        let mut source = self.source.fresh_sampler();
        placement.positions_with_search(self.seed, |initial, random| {
            // Empty and universal holder sets do not require climate sampling;
            // the reservoir still consumes the same random draws when universal.
            if preferred.is_empty() {
                return Ok(None);
            }
            source.reset_search_history();
            native_resources::structure_placement::find_ring_biome(initial, random, |quart| {
                if all_preferred {
                    return Ok(true);
                }
                let biome = source.sample_quart(self.noise.router(), evaluator, quart);
                Ok(preferred.contains(biome))
            })
        })
    }
    /// Call in the same chunk order as the world generator; equal-distance
    /// climate matches reuse the previous lookup, as in the Java thread.
    pub fn generate_biomes(
        &mut self,
        chunk: [i32; 2],
        evaluator: &mut DensityEvaluator,
    ) -> Result<QuartBiomeChunk> {
        let previous = evaluator.replace_structure_terrain_context(None);
        let result = self.generate_biomes_unadjusted(chunk, evaluator);
        evaluator.replace_structure_terrain_context(previous);
        result
    }
    fn generate_biomes_unadjusted(
        &mut self,
        chunk: [i32; 2],
        evaluator: &mut DensityEvaluator,
    ) -> Result<QuartBiomeChunk> {
        let x = chunk[0].checked_mul(4).context("quart X overflow")?;
        let z = chunk[1].checked_mul(4).context("quart Z overflow")?;
        x.checked_add(3).context("quart X overflow")?;
        z.checked_add(3).context("quart Z overflow")?;
        let min_y = self.min_y();
        let height = self.height();
        let min = [x, min_y.div_euclid(4), z];
        let sy = (height / 4) as usize;
        let climate = if self.source.uses_bulk_climate() {
            Some(ClimateVolume::sample(
                self.noise.router(),
                evaluator,
                min,
                [4, sy as u32, 4],
            )?)
        } else {
            None
        };
        let end = self.source.is_end();
        let end_y_independent = end
            && !self
                .noise
                .router()
                .function(native_resources::noise_router::RouterField::Erosion)
                .depends_on_y();
        let mut end_cache = if end {
            vec![None; if end_y_independent { 1 } else { sy }]
        } else {
            Vec::new()
        };
        let mut values = vec![None; 16 * sy];
        // ChunkAccess walks sections bottom-up, then local X, Y, Z.
        for section in 0..height / 16 {
            for lx in 0..4 {
                for ly in 0..4 {
                    for lz in 0..4 {
                        let y = section * 4 + ly;
                        let point = [x + lx, min[1] + y, z + lz];
                        let biome = if let Some(climate) = &climate {
                            climate
                                .select(&mut self.source, point)
                                .context("climate sample is absent")?
                                .clone()
                        } else if end {
                            let index = if end_y_independent { 0 } else { y as usize };
                            if end_cache[index].is_none() {
                                end_cache[index] = Some(
                                    self.source
                                        .sample_quart(self.noise.router(), evaluator, point)
                                        .clone(),
                                );
                            }
                            end_cache[index].as_ref().unwrap().clone()
                        } else {
                            self.source
                                .sample_quart(self.noise.router(), evaluator, point)
                                .clone()
                        };
                        let index = (lz as usize * 4 + lx as usize) * sy + y as usize;
                        values[index] = Some(biome);
                    }
                }
            }
        }
        let values = values
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .context("incomplete biome volume")?;
        QuartBiomeChunk::from_values(chunk, self.seed, min_y, height, values)
    }
    /// Generate the intermediate noise volume before surface rules or carvers.
    pub fn generate_noise(
        &self,
        evaluator: &mut DensityEvaluator,
        chunk: [i32; 2],
    ) -> Result<native_resources::noise_stage::NoiseStageChunk> {
        self.noise
            .generate(evaluator, chunk[0], chunk[1])?
            .expand_to_dimension(self.min_y(), self.height())
    }

    /// The result is an intermediate SURFACE-stage chunk; it must not be saved
    /// or treated as a complete generated chunk.
    pub fn generate_surface(
        &self,
        provider: &mut impl ResourceProvider,
        evaluator: &mut DensityEvaluator,
        chunk: [i32; 2],
        neighborhood: &BiomeNeighborhood,
        classify: impl FnMut(&Value) -> Result<SurfaceBlockKind>,
    ) -> Result<SurfaceStageChunk> {
        neighborhood.validate(chunk, self.seed, self.min_y(), self.height())?;
        let blocks = self.generate_noise(evaluator, chunk)?;
        let preliminary = self
            .noise
            .preliminary_surface(evaluator, chunk[0], chunk[1])?;
        let x = chunk[0].checked_mul(16).context("block X overflow")?;
        let z = chunk[1].checked_mul(16).context("block Z overflow")?;
        let min = [x, self.noise.router().min_y, z];
        let size = [16, self.noise.router().height as u32, 16];
        let mut inputs = if self.structure_context {
            SeededSurfaceInputs::with_structure_context(
                provider,
                &self.surface,
                self.seed,
                self.noise.random_algorithm(),
                min,
                size,
                evaluator.structure_terrain_context().cloned(),
            )?
        } else {
            SeededSurfaceInputs::without_terrain_adjustments(
                provider,
                &self.surface,
                self.seed,
                self.noise.random_algorithm(),
                min,
                size,
            )?
        };
        apply_surface(
            &blocks,
            &self.surface,
            &mut inputs,
            &preliminary,
            self.noise.sea_level(),
            classify,
            |x, y, z| Ok(neighborhood.biome_at_block(x, y, z)?.clone()),
        )
    }

    /// Generate a surface with already generated structure pieces affecting this chunk.
    pub fn generate_surface_with_structure_context(
        &self,
        provider: &mut impl ResourceProvider,
        evaluator: &mut DensityEvaluator,
        chunk: [i32; 2],
        neighborhood: &BiomeNeighborhood,
        structure: Arc<Beardifier>,
        classify: impl FnMut(&Value) -> Result<SurfaceBlockKind>,
    ) -> Result<SurfaceStageChunk> {
        ensure!(
            self.structure_context,
            "generator was compiled without structure context"
        );
        let previous = evaluator.replace_structure_terrain_context(Some(structure));
        let result = self.generate_surface(provider, evaluator, chunk, neighborhood, classify);
        evaluator.replace_structure_terrain_context(previous);
        result
    }

    /// Generate the fresh-terrain carving mask. The caller must still apply it
    /// to the surface chunk with aquifer and block-replacement rules.
    pub fn generate_carving_mask(
        &mut self,
        provider: &mut impl ResourceProvider,
        evaluator: &mut DensityEvaluator,
        cache: &mut CarverCache,
        chunk: [i32; 2],
    ) -> Result<CarvingMask> {
        let min_y = self.noise.router().min_y;
        let height = self.noise.router().height;
        let previous = evaluator.replace_structure_terrain_context(None);
        let result = cache.generate(provider, self.seed, chunk, min_y, height, |source| {
            let x = source[0]
                .checked_mul(4)
                .context("carver biome quart X overflow")?;
            let z = source[1]
                .checked_mul(4)
                .context("carver biome quart Z overflow")?;
            Ok(self
                .source
                .sample_quart(self.noise.router(), evaluator, [x, 0, z])
                .clone())
        });
        evaluator.replace_structure_terrain_context(previous);
        result
    }

    /// Produce fresh terrain after surface rules and carvers. This is still an
    /// intermediate chunk: structures, features and lighting remain. Block
    /// classification must be stable for the active registry and pack stack.
    pub fn generate_carved_surface<P: ResourceProvider>(
        &mut self,
        provider: &mut P,
        evaluator: &mut DensityEvaluator,
        cache: &mut CarverCache,
        chunk: [i32; 2],
        neighborhood: &BiomeNeighborhood,
        tags: &native_resources::carver::tags::CarverBlockTags,
        mut classify: impl FnMut(&Value) -> Result<CarverBlockState>,
    ) -> Result<SurfaceStageChunk> {
        let mut result =
            self.generate_surface(provider, evaluator, chunk, neighborhood, |state| {
                let facts = classify(state)?;
                Ok(if facts.is_air {
                    SurfaceBlockKind::Air
                } else if facts.has_fluid {
                    SurfaceBlockKind::Fluid
                } else {
                    SurfaceBlockKind::Solid
                })
            })?;
        let mask = self.generate_carving_mask(provider, evaluator, cache, chunk)?;
        self.apply_carving_to_surface(
            provider,
            evaluator,
            &mut result,
            &mask,
            tags,
            &mut classify,
        )?;
        Ok(result)
    }

    /// Generate surface and carvers with one chunk-specific structure context.
    #[allow(clippy::too_many_arguments)]
    pub fn generate_carved_surface_with_structure_context<P: ResourceProvider>(
        &mut self,
        provider: &mut P,
        evaluator: &mut DensityEvaluator,
        cache: &mut CarverCache,
        chunk: [i32; 2],
        neighborhood: &BiomeNeighborhood,
        tags: &native_resources::carver::tags::CarverBlockTags,
        structure: Arc<Beardifier>,
        classify: impl FnMut(&Value) -> Result<CarverBlockState>,
    ) -> Result<SurfaceStageChunk> {
        ensure!(
            self.structure_context,
            "generator was compiled without structure context"
        );
        let previous = evaluator.replace_structure_terrain_context(Some(structure));
        let result = self.generate_carved_surface(
            provider,
            evaluator,
            cache,
            chunk,
            neighborhood,
            tags,
            classify,
        );
        evaluator.replace_structure_terrain_context(previous);
        result
    }

    /// Apply a mask to a fresh surface-stage chunk from this generator. On
    /// error the partially modified chunk must be discarded.
    pub fn apply_carving_to_surface<P: ResourceProvider>(
        &mut self,
        provider: &mut P,
        evaluator: &mut DensityEvaluator,
        chunk: &mut SurfaceStageChunk,
        mask: &CarvingMask,
        tags: &native_resources::carver::tags::CarverBlockTags,
        mut classify: impl FnMut(&Value) -> Result<CarverBlockState>,
    ) -> Result<()> {
        ensure!(
            chunk.min_y == self.min_y && chunk.height == self.height,
            "surface chunk height does not match generator"
        );
        ensure!(
            chunk.carver_postprocessing.is_none(),
            "carving was already applied"
        );
        if !mask.is_empty() {
            let structure = evaluator.structure_terrain_context().cloned();
            let aquifer = self
                .noise
                .carving_aquifer(evaluator, chunk.chunk_position)?;
            let mut inputs = carving::FreshCarvingInputs::new(
                provider,
                tags,
                aquifer,
                &self.noise,
                &self.surface,
                &mut self.source,
                self.seed,
                self.structure_context,
                structure,
                &mut classify,
            );
            native_resources::carver::apply::apply_mask(chunk, mask, &mut inputs)?;
        } else {
            chunk.carver_postprocessing = chunk.surface_postprocessing.clone();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn generated_chunks_share_a_world_without_losing_flags_or_snapshots() {
        use native_world::loaded::{BlockState, ChunkPos};

        let make = |position: [i32; 2], seed, min_y, dimension: &str, reverse: bool| {
            let stone = Arc::new(json!("minecraft:stone"));
            let water = Arc::new(json!("minecraft:water"));
            let states = (0..4096)
                .map(|index| {
                    Arc::clone(if (index % 2 == 0) ^ reverse {
                        &stone
                    } else {
                        &water
                    })
                })
                .collect();
            let updates = (0..4096).map(|index| index % 17 == 0).collect();
            let mut noise = native_core::postprocessing::PostProcessing::new(min_y, 16).unwrap();
            noise.mark(position[0] * 16, min_y, position[1] * 16);
            noise.mark(position[0] * 16, min_y, position[1] * 16);
            let mut surface = noise.clone();
            surface.mark(position[0] * 16 + 1, min_y, position[1] * 16);
            let terrain = SurfaceStageChunk {
                chunk_position: position,
                min_y,
                height: 16,
                states,
                fluid_updates: updates,
                noise_postprocessing: Some(noise),
                surface_postprocessing: Some(surface),
                carver_postprocessing: None,
            };
            let biomes = QuartBiomeChunk::from_values(
                position,
                seed,
                min_y,
                16,
                vec![
                    ResourceId::parse(if reverse {
                        "minecraft:desert"
                    } else {
                        "minecraft:plains"
                    })
                    .unwrap();
                    64
                ],
            )
            .unwrap();
            materialize_carved_terrain(terrain, &biomes, dimension, |value| {
                Ok(BlockState {
                    name: value.as_str().unwrap().to_owned(),
                    properties: BTreeMap::new(),
                })
            })
            .unwrap()
        };

        let first_pos = ChunkPos { x: -1, z: 0 };
        let second_pos = ChunkPos { x: 0, z: 0 };
        let mut assembled =
            CarvedTerrainWorld::new(make([-1, 0], 42, -16, "minecraft:overworld", false));
        let before = assembled.snapshot();
        let second = make([0, 0], 42, -16, "minecraft:overworld", true);
        let second_flags = second.fluid_postprocessing().to_vec();
        assembled.insert(&second).unwrap();
        let after = assembled.snapshot();
        assert_eq!(before.chunks.len(), 1);
        assert_eq!(after.chunks.len(), 2);
        assert!(Arc::ptr_eq(
            &before.chunks[&first_pos],
            &after.chunks[&first_pos]
        ));
        assert_eq!(before.block(0, -16, 0), None);
        assert_eq!(after.block(-16, -16, 0).unwrap().name, "minecraft:stone");
        assert_eq!(after.block(0, -16, 0).unwrap().name, "minecraft:water");
        assert_eq!(after.biome(-1, -16, 0), Some("minecraft:plains"));
        assert_eq!(after.biome(0, -16, 0), Some("minecraft:desert"));
        assert_eq!(
            assembled
                .surface_postprocessing(first_pos)
                .unwrap()
                .sections()[0],
            [0, 0, 1]
        );
        assert_eq!(
            assembled
                .surface_postprocessing(second_pos)
                .unwrap()
                .sections()[0],
            [0, 0, 1]
        );
        assert_eq!(
            assembled
                .noise_postprocessing(first_pos)
                .unwrap()
                .sections()[0],
            [0, 0]
        );
        assert_eq!(
            assembled
                .noise_postprocessing(second_pos)
                .unwrap()
                .sections()[0],
            [0, 0]
        );
        assert_eq!(
            assembled.fluid_postprocessing(second_pos),
            Some(second_flags.as_slice())
        );
        assert_eq!(
            assembled.fluid_postprocessing(ChunkPos { x: 1, z: 0 }),
            None
        );

        for invalid in [
            make([0, 0], 42, -16, "minecraft:overworld", false),
            make([1, 0], 43, -16, "minecraft:overworld", false),
            make([1, 0], 42, 0, "minecraft:overworld", false),
            make([1, 0], 42, -16, "minecraft:the_nether", false),
        ] {
            assert!(assembled.insert(&invalid).is_err());
            assert!(Arc::ptr_eq(&after, &assembled.snapshot()));
            assert_eq!(
                assembled.fluid_postprocessing(second_pos),
                Some(second_flags.as_slice())
            );
        }
    }

    #[test]
    fn materialized_terrain_preserves_states_biomes_and_postprocessing() {
        let position = [-2, 3];
        let height = 32usize;
        let stone = Arc::new(json!("minecraft:stone"));
        let water = Arc::new(json!({"id":"minecraft:water", "properties":{"level":"3"}}));
        let mut states = Vec::new();
        let mut updates = Vec::new();
        for z in 0..16usize {
            for x in 0..16usize {
                for y in 0..height {
                    states.push(Arc::clone(if (x + z + y) % 3 == 0 {
                        &water
                    } else {
                        &stone
                    }));
                    updates.push((x + y * 3 + z * 7) % 13 == 0);
                }
            }
        }
        let biome_values: Vec<_> = (0..16 * height / 4)
            .map(|i| {
                ResourceId::parse(if i % 2 == 0 {
                    "minecraft:plains"
                } else {
                    "minecraft:desert"
                })
                .unwrap()
            })
            .collect();
        let biomes =
            QuartBiomeChunk::from_values(position, 123, -16, height as i32, biome_values.clone())
                .unwrap();
        let terrain = SurfaceStageChunk {
            chunk_position: position,
            min_y: -16,
            height: height as i32,
            states,
            fluid_updates: updates.clone(),
            noise_postprocessing: None,
            surface_postprocessing: None,
            carver_postprocessing: None,
        };
        let mut resolved = 0;
        let generated =
            materialize_carved_terrain(terrain, &biomes, "minecraft:overworld", |value| {
                resolved += 1;
                if value.is_string() {
                    Ok(native_world::loaded::BlockState {
                        name: "minecraft:stone".into(),
                        properties: BTreeMap::new(),
                    })
                } else {
                    Ok(native_world::loaded::BlockState {
                        name: "minecraft:water".into(),
                        properties: BTreeMap::from([("level".into(), "3".into())]),
                    })
                }
            })
            .unwrap();
        assert_eq!(resolved, 2, "resolve each shared state once");
        assert_eq!(generated.fluid_postprocessing(), updates);
        assert_eq!((generated.min_y(), generated.height()), (-16, 32));
        assert_eq!(generated.fluid_postprocessing_at(0, -16, 0), Some(true));
        assert_eq!(
            generated.fluid_postprocessing_at(15, 15, 15),
            Some(updates[(15 * 16 + 15) * height + 31])
        );
        assert_eq!(generated.fluid_postprocessing_at(16, 0, 0), None);
        assert_eq!(generated.fluid_postprocessing_at(0, -17, 0), None);
        assert_eq!(generated.fluid_postprocessing_at(0, 16, 0), None);
        let world = generated.world();
        assert_eq!(world.biome_zoom_seed(), Some(obfuscate_seed(123)));
        let chunk = &world.chunks[&native_world::loaded::ChunkPos { x: -2, z: 3 }];
        assert_eq!(chunk.status.as_deref(), Some("minecraft:terrain"));
        assert!(!chunk.block_light_correct);
        for section in chunk.sections.values() {
            assert!(section.block_light.is_none() && section.sky_light.is_none());
        }
        for z in 0..16usize {
            for x in 0..16usize {
                for y in 0..height {
                    let block = world
                        .block(
                            position[0] * 16 + x as i32,
                            -16 + y as i32,
                            position[1] * 16 + z as i32,
                        )
                        .unwrap();
                    assert_eq!(
                        block.name,
                        if (x + z + y) % 3 == 0 {
                            "minecraft:water"
                        } else {
                            "minecraft:stone"
                        }
                    );
                    if block.name == "minecraft:water" {
                        assert_eq!(block.properties.get("level").map(String::as_str), Some("3"));
                    }
                    let biome = &biome_values[((z / 4) * 4 + x / 4) * (height / 4) + y / 4];
                    assert_eq!(
                        world.biome(
                            position[0] * 16 + x as i32,
                            -16 + y as i32,
                            position[1] * 16 + z as i32
                        ),
                        Some(format!("{}:{}", biome.namespace, biome.path).as_str())
                    );
                }
            }
        }
        let (world, flags, noise, surface, carver) = generated.into_parts();
        assert_eq!(flags.as_ref(), updates);
        assert!(noise.is_none());
        assert!(surface.is_none());
        assert!(carver.is_none());
        assert_eq!(world.chunks.len(), 1);
    }

    #[test]
    fn materialized_terrain_rejects_bad_ranges_and_unresolved_states() {
        let biomes = QuartBiomeChunk::from_values(
            [0, 0],
            0,
            0,
            16,
            vec![ResourceId::parse("minecraft:plains").unwrap(); 64],
        )
        .unwrap();
        let terrain = || SurfaceStageChunk {
            chunk_position: [0, 0],
            min_y: 0,
            height: 16,
            states: vec![Arc::new(json!("minecraft:stone")); 4096],
            fluid_updates: vec![false; 4096],
            noise_postprocessing: None,
            surface_postprocessing: None,
            carver_postprocessing: None,
        };
        assert!(
            materialize_carved_terrain(
                terrain(),
                &biomes,
                "minecraft:overworld",
                |_| anyhow::bail!("unregistered block")
            )
            .is_err()
        );
        let mut bad = terrain();
        bad.fluid_updates.pop();
        assert!(
            materialize_carved_terrain(bad, &biomes, "minecraft:overworld", |_| unreachable!())
                .is_err()
        );
        let mut bad = terrain();
        bad.chunk_position = [1, 0];
        assert!(
            materialize_carved_terrain(bad, &biomes, "minecraft:overworld", |_| unreachable!())
                .is_err()
        );
        let bad_biomes = QuartBiomeChunk::from_values(
            [i32::MAX, 0],
            0,
            0,
            16,
            vec![ResourceId::parse("minecraft:plains").unwrap(); 64],
        )
        .unwrap();
        let mut bad = terrain();
        bad.chunk_position = [i32::MAX, 0];
        assert!(
            materialize_carved_terrain(bad, &bad_biomes, "minecraft:overworld", |_| unreachable!())
                .is_err()
        );
        let high = QuartBiomeChunk::from_values(
            [0, 0],
            0,
            2048,
            16,
            vec![ResourceId::parse("minecraft:plains").unwrap(); 64],
        )
        .unwrap();
        let mut bad = terrain();
        bad.min_y = 2048;
        assert!(
            materialize_carved_terrain(bad, &high, "minecraft:overworld", |_| unreachable!())
                .is_err()
        );
    }
    struct Data;
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(if path.starts_with("data/minecraft/worldgen/biome/") {
                Some(br#"{"has_precipitation":true,"temperature":0.8,"carvers":[]}"#.to_vec())
            } else if path == "data/minecraft/tags/block/uncarvable.json" {
                Some(br#"{"values":[]}"#.to_vec())
            } else if path.starts_with("data/minecraft/worldgen/noise/") {
                Some(br#"{"base_octave":-4,"octave_count":2}"#.to_vec())
            } else {
                None
            })
        }
        fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
            ensure!(folder == "worldgen/biome", "unexpected registry");
            [
                "plains",
                "the_end",
                "end_highlands",
                "end_midlands",
                "small_end_islands",
                "end_barrens",
            ]
            .into_iter()
            .map(ResourceId::parse)
            .collect()
        }
    }
    fn definition() -> Value {
        json!({"type":"noise","biome_source":{"type":"fixed","biome":"minecraft:plains"},"settings":{
            "noise":{"min_y":0,"height":16},"legacy_random_source":false,"sea_level":8,
            "default_block":"minecraft:stone","default_fluid":"minecraft:water",
            "material_rule":{"type":"block","result_state":"minecraft:dirt"},
            "noise_router":{"final_density":1,"temperature":0,"vegetation":0,"continents":0,
                "erosion":0,"depth":0,"ridges":0,"chunk_surface_level":8}
        }})
    }
    #[test]
    fn generator_spawn_origin_uses_settings_and_validates_missing_targets() {
        let mut data = Data;
        let mut config = definition();
        let generator =
            TerrainGenerator::without_terrain_adjustments(&mut data, &config, 42, 0, 16).unwrap();
        assert_eq!(generator.spawn_height(), 64);
        assert!(generator.spawn_origin(&mut data).is_err());
        config["settings"]["spawn_target"] = json!([]);
        let generator =
            TerrainGenerator::without_terrain_adjustments(&mut data, &config, 42, 0, 16).unwrap();
        assert_eq!(generator.spawn_origin(&mut data).unwrap(), [0, 0]);
        config["settings"]["spawn_target"] = json!([{"minecraft:missing": [0, 1]}]);
        let generator =
            TerrainGenerator::without_terrain_adjustments(&mut data, &config, 42, 0, 16).unwrap();
        assert!(generator.spawn_origin(&mut data).is_err());
    }
    #[test]
    fn generator_uses_noise_settings_for_region_random() {
        use native_resources::{density::WorldgenRandom, worldgen_region_random::RegionRandom};
        use native_world::loaded::ChunkPos;

        for (legacy, algorithm) in [
            (true, WorldgenRandom::Legacy),
            (false, WorldgenRandom::Xoroshiro),
        ] {
            let mut config = definition();
            config["settings"]["legacy_random_source"] = json!(legacy);
            let generator =
                TerrainGenerator::without_terrain_adjustments(&mut Data, &config, 42, 0, 16)
                    .unwrap();
            let mut actual = generator.region_random(ChunkPos { x: -13, z: 17 });
            let mut expected = RegionRandom::new(42, algorithm, -13, 17);
            assert_eq!(actual.next_long(), expected.next_long());
        }
    }
    #[test]
    fn generator_reads_mob_generation_switch_and_rejects_invalid_values() {
        let mut data = Data;
        let mut config = definition();
        let generator =
            TerrainGenerator::without_terrain_adjustments(&mut data, &config, 42, 0, 16).unwrap();
        assert_eq!(generator.disable_mob_generation(), None);
        for disabled in [false, true] {
            config["settings"]["disable_mob_generation"] = json!(disabled);
            let generator =
                TerrainGenerator::without_terrain_adjustments(&mut data, &config, 42, 0, 16)
                    .unwrap();
            assert_eq!(generator.disable_mob_generation(), Some(disabled));
        }
        config["settings"]["disable_mob_generation"] = json!("false");
        assert!(
            TerrainGenerator::without_terrain_adjustments(&mut data, &config, 42, 0, 16).is_err()
        );
    }
    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn fresh_batch_tracks_light_registration_between_feature_steps() {
        use native_resources::{
            carver::tags::CarverBlockTags, dimension_lighting::GeneratedDimensionLighting,
        };
        use native_world::loaded::{BlockState, ChunkPos};
        struct FeatureData {
            crossing: bool,
            brown: bool,
        }
        impl ResourceProvider for FeatureData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/pig_variant/temperate.json" {
                    return Ok(Some(br#"{"spawn_conditions":[{"priority":0}]}"#.to_vec()));
                }
                if path == "data/minecraft/pig_sound_variant/classic.json" {
                    return Ok(Some(b"{}".to_vec()));
                }
                if path == "data/minecraft/worldgen/biome/plains.json" {
                    let mushroom = if self.brown {
                        "minecraft:brown_mushroom"
                    } else {
                        "minecraft:red_mushroom"
                    };
                    let mut features = vec![
                        json!({"feature":{"type":"minecraft:simple_block","to_place":{"id":mushroom},"schedule_tick":true},"placement":[{"type":"minecraft:offset","x":8,"y":8,"z":8}]}),
                    ];
                    if self.crossing {
                        features.push(json!({"feature":{"type":"minecraft:simple_block","to_place":{"id":"minecraft:melon"}},"placement":[{"type":"minecraft:offset","x":-1,"y":8,"z":8}]}));
                    }
                    return Ok(Some(serde_json::to_vec(
                        &json!({"has_precipitation":true,"temperature":0.8,"carvers":[],"features":[features]}),
                    )?));
                }
                if path == "data/minecraft/tags/block/blocks_motion_in_heightmap.json"
                    || path == "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json"
                {
                    return Ok(Some(
                        br#"{"values":["minecraft:stone","minecraft:dirt","minecraft:melon"]}"#
                            .to_vec(),
                    ));
                }
                if path == "data/minecraft/tags/block/overrides_mushroom_light_requirement.json" {
                    return Ok(Some(br#"{"values":[]}"#.to_vec()));
                }
                if path
                    == "data/minecraft/tags/worldgen/biome/spawns_warm_variant_farm_animals.json"
                {
                    return Ok(Some(br#"{"values":["minecraft:plains"]}"#.to_vec()));
                }
                if matches!(
                    path,
                    "data/minecraft/tags/block/animals_spawnable_on.json"
                        | "data/minecraft/tags/block/goats_spawnable_on.json"
                        | "data/minecraft/tags/block/rabbits_spawnable_on.json"
                ) {
                    return Ok(Some(
                        br#"{"values":["minecraft:stone","minecraft:dirt"]}"#.to_vec(),
                    ));
                }
                Data.read_data(path)
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                if folder == "pig_variant" {
                    return Ok(vec![ResourceId::parse("minecraft:temperate")?]);
                }
                if folder == "pig_sound_variant" {
                    return Ok(vec![ResourceId::parse("minecraft:classic")?]);
                }
                Data.list_data(folder)
            }
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        for crossing in [false, true] {
            let generated_registry = resources.vanilla_generated_block_registry().unwrap();
            let mut block_registry: BTreeSet<String> =
                generated_registry.ids().map(str::to_owned).collect();
            block_registry.extend(
                [
                    "air",
                    "cave_air",
                    "void_air",
                    "bedrock",
                    "stone",
                    "dirt",
                    "water",
                    "lava",
                    "red_mushroom",
                    "melon",
                ]
                .into_iter()
                .map(|name| format!("minecraft:{name}")),
            );
            let mut data = FeatureData {
                crossing,
                brown: false,
            };
            let tags = CarverBlockTags::load(&mut data, &block_registry).unwrap();
            let generator =
                TerrainGenerator::without_terrain_adjustments(&mut data, &definition(), 42, 0, 16)
                    .unwrap();
            let mut batch =
                FreshTerrainBatch::new_without_structures(generator, tags, "minecraft:overworld")
                    .unwrap();
            let lighting = GeneratedDimensionLighting {
                dimension: ResourceId::parse("minecraft:overworld").unwrap(),
                dimension_type: None,
                inline_dimension_type: None,
                min_y: 0,
                height: 16,
                has_skylight: true,
                ambient_light: Some(0.0),
            };
            let center = ChunkPos { x: 0, z: 0 };
            let east = ChunkPos { x: 1, z: 0 };
            assert!(batch.request_light_initialization(center).is_err());
            batch
                .enable_decoration(&mut data, &BTreeSet::new(), block_registry, &lighting)
                .unwrap();
            batch
                .enable_light_initialization(generated_registry)
                .unwrap();
            assert!(batch.request_light_initialization_many([]).is_err());
            assert!(
                batch
                    .request_light_initialization_many([center, ChunkPos { x: i32::MAX, z: 0 }])
                    .is_err()
            );
            assert_eq!(batch.schedule().requested_chunks(), 0);
            batch
                .request_light_initialization_many([center, center])
                .unwrap();
            let mut classify = |v: &Value| -> Result<CarverBlockState> {
                let id = v
                    .as_str()
                    .or_else(|| v.get("id").and_then(Value::as_str))
                    .context("state ID")?;
                Ok(CarverBlockState {
                    is_air: id == "minecraft:air",
                    has_fluid: matches!(id, "minecraft:water" | "minecraft:lava"),
                })
            };
            let mut resolve = |v: &Value| -> Result<BlockState> {
                Ok(BlockState {
                    name: v
                        .as_str()
                        .or_else(|| v.get("id").and_then(Value::as_str))
                        .context("state ID")?
                        .into(),
                    properties: BTreeMap::new(),
                })
            };
            while batch
                .step(&mut data, &mut classify, &mut resolve)
                .unwrap()
                .is_some()
            {}
            assert_eq!(
                batch.schedule().completed(center),
                Some(schedule::GenerationStage::InitializeLight)
            );
            let before = batch.snapshot().unwrap();
            assert_eq!(before.block(8, 8, 8).unwrap().name, "minecraft:dirt");
            assert!(batch.seal_for_light_propagation().is_err());
            batch.request_features(east).unwrap();
            let mut before_attempt = None;
            loop {
                if batch
                    .schedule()
                    .next_ready()
                    .is_some_and(|(_, stage)| stage == schedule::GenerationStage::Features)
                {
                    before_attempt = batch.snapshot();
                }
                match batch.step(&mut data, &mut classify, &mut resolve) {
                    Ok(Some(_)) => {}
                    Ok(None) if !crossing => break,
                    Err(error) if crossing => {
                        assert!(
                            format!("{error:#}").contains("registered light chunk has changed")
                        );
                        break;
                    }
                    other => panic!("unexpected batch result {other:?}"),
                }
            }
            if crossing {
                assert!(Arc::ptr_eq(
                    &before_attempt.unwrap(),
                    &batch.snapshot().unwrap()
                ));
                assert_eq!(
                    batch.schedule().completed(east),
                    Some(schedule::GenerationStage::Terrain)
                );
                assert!(!batch.is_complete());
                continue;
            }
            let after = batch.snapshot().unwrap();
            assert_eq!(
                after.block(24, 8, 8).unwrap().name,
                "minecraft:red_mushroom"
            );
            assert!(Arc::ptr_eq(&before.chunks[&center], &after.chunks[&center]));
            let (schedule, _, terrain, light) = batch.into_parts().unwrap();
            assert_eq!(
                schedule.completed(east),
                Some(schedule::GenerationStage::Features)
            );
            let terrain = terrain.unwrap();
            let (FreshBatchLight::Initialized(tracker), registry) = light.unwrap() else {
                panic!("expected registered but unpropagated lighting");
            };
            assert_eq!(tracker.sky_sources(&terrain, center).unwrap()[0], 16);
            let visible = tracker
                .snapshot(&terrain, [east, ChunkPos { x: 3, z: 0 }], &registry)
                .unwrap();
            assert_eq!(visible.sky_light([24, 8, 8]), Some(0));
            assert_eq!(visible.sky_light([48, 8, 8]), Some(15));
        }
        // Prepare a frozen 3x3 registration batch, then propagate exactly one
        // chunk. Brown mushrooms emit level one in the no-sky dimension.
        let generated_registry = resources.vanilla_generated_block_registry().unwrap();
        let mut block_registry: BTreeSet<String> =
            generated_registry.ids().map(str::to_owned).collect();
        block_registry.extend(
            [
                "air",
                "cave_air",
                "void_air",
                "bedrock",
                "stone",
                "dirt",
                "water",
                "lava",
                "melon",
                "brown_mushroom",
            ]
            .into_iter()
            .map(|name| format!("minecraft:{name}")),
        );
        let mut data = FeatureData {
            crossing: false,
            brown: true,
        };
        let tags = CarverBlockTags::load(&mut data, &block_registry).unwrap();
        let mob_height_tags =
            native_resources::heightmap_tags::HeightmapTags::load(&mut data, &block_registry)
                .unwrap();
        let ground_spawn_tags =
            native_resources::worldgen_mob_rules::GroundSpawnTags::load(&mut data, &block_registry)
                .unwrap();
        let mut spawn_settings = definition();
        spawn_settings["settings"]["disable_mob_generation"] = json!(false);
        let generator =
            TerrainGenerator::without_terrain_adjustments(&mut data, &spawn_settings, 42, 0, 16)
                .unwrap();
        let mut batch =
            FreshTerrainBatch::new_without_structures(generator, tags, "minecraft:overworld")
                .unwrap();
        let lighting = GeneratedDimensionLighting {
            dimension: ResourceId::parse("minecraft:overworld").unwrap(),
            dimension_type: None,
            inline_dimension_type: None,
            min_y: 0,
            height: 16,
            has_skylight: false,
            ambient_light: Some(0.0),
        };
        batch
            .enable_decoration(&mut data, &BTreeSet::new(), block_registry, &lighting)
            .unwrap();
        batch
            .enable_light_initialization(generated_registry)
            .unwrap();
        let center = ChunkPos { x: 0, z: 0 };
        let east = ChunkPos { x: 1, z: 0 };
        let later = ChunkPos { x: 2, z: 0 };
        assert!(batch.mob_spawn_biome(center).is_err());
        for z in -1..=1 {
            for x in -1..=1 {
                batch
                    .request_light_initialization(ChunkPos { x, z })
                    .unwrap();
            }
        }
        batch.request_features(later).unwrap();
        assert!(batch.seal_for_light_propagation().is_err());
        let mut classify = |v: &Value| -> Result<CarverBlockState> {
            let id = v
                .as_str()
                .or_else(|| v.get("id").and_then(Value::as_str))
                .context("state ID")?;
            Ok(CarverBlockState {
                is_air: id == "minecraft:air",
                has_fluid: matches!(id, "minecraft:water" | "minecraft:lava"),
            })
        };
        let mut resolve = |v: &Value| -> Result<BlockState> {
            Ok(BlockState {
                name: v
                    .as_str()
                    .or_else(|| v.get("id").and_then(Value::as_str))
                    .context("state ID")?
                    .into(),
                properties: BTreeMap::new(),
            })
        };
        while batch
            .step(&mut data, &mut classify, &mut resolve)
            .unwrap()
            .is_some()
        {}
        assert!(batch.request_light(center).is_err());
        batch.seal_for_light_propagation().unwrap();
        assert!(batch.request(east).is_err());
        assert!(batch.request_features(east).is_err());
        assert!(batch.request_light_initialization(east).is_err());
        assert!(batch.request_light(east).is_err()); // eastern 3x3 is not fully initialized
        assert!(batch.is_complete());
        let before = batch.light_snapshot([center]).unwrap();
        let original = batch.snapshot().unwrap();
        let before_view = batch.lit_snapshot([center]).unwrap();
        assert!(batch.lit_render_snapshot([center]).is_err());
        assert!(batch.lit_render_snapshot([]).is_err());
        assert!(Arc::ptr_eq(
            &before_view,
            &batch.lit_snapshot([center]).unwrap()
        ));
        assert!(batch.lit_snapshot([ChunkPos { x: 100, z: 100 }]).is_err());
        assert!(Arc::ptr_eq(&original, &batch.lit_snapshot([]).unwrap()));
        assert!(Arc::ptr_eq(
            &original.chunks[&east],
            &before_view.chunks[&east]
        ));
        assert!(!Arc::ptr_eq(
            &original.chunks[&center],
            &before_view.chunks[&center]
        ));
        assert_eq!(original.light(8, 8, 8, false), None);
        assert_eq!(before_view.light(8, 8, 8, false), Some(0));
        assert_eq!(before.block_light([8, 8, 8]), Some(0));
        batch.request_light(center).unwrap();
        assert_eq!(
            batch.step(&mut data, &mut classify, &mut resolve).unwrap(),
            Some((center, schedule::GenerationStage::Light))
        );
        assert!(
            batch
                .step(&mut data, &mut classify, &mut resolve)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            batch.mob_spawn_biome(center).unwrap(),
            ResourceId::parse("minecraft:plains").unwrap()
        );
        {
            use native_resources::mob_spawn_placement::{
                MobSpawnHeightmap, TopMobSpawnPosition, on_ground_spawn_position_ok,
            };
            let border = crate::respawn::RespawnBorder {
                min_x: -100.0,
                max_x: 100.0,
                min_z: -100.0,
                max_z: 100.0,
            };
            let tag = BTreeSet::new();
            let mut view = GeneratedMobPlacementView::new(
                &batch,
                MobSpawnHeightmap::MotionBlockingNoLeaves,
                border,
                &tag,
                &mob_height_tags,
            )
            .unwrap();
            let mut search = TopMobSpawnPosition::new(8, 8, 0, false);
            let top = loop {
                if let Some(pos) = search.poll(&mut view, 1).unwrap() {
                    break pos;
                }
            };
            assert_eq!(top, IVec3::new(8, 16, 8));
            assert!(on_ground_spawn_position_ok(&mut view, top).unwrap());
            let tag = BTreeSet::from(["minecraft:void_air".into()]);
            let mut view = GeneratedMobPlacementView::new(
                &batch,
                MobSpawnHeightmap::MotionBlockingNoLeaves,
                border,
                &tag,
                &mob_height_tags,
            )
            .unwrap();
            assert!(!on_ground_spawn_position_ok(&mut view, top).unwrap());
            assert!(
                on_ground_spawn_position_ok(&mut view, IVec3::new(1000, 16, 8)).is_ok_and(|ok| !ok)
            );
        }
        // The snapshot has never had these maps requested; a query should
        // still work without materializing or modifying any terrain blocks.
        batch.forget_final_heightmaps_for_test(center);
        {
            use native_resources::mob_spawn_placement::{MobPlacementWorld, MobSpawnHeightmap};
            let before = batch.snapshot().unwrap();
            let border = crate::respawn::RespawnBorder {
                min_x: -100.0,
                max_x: 100.0,
                min_z: -100.0,
                max_z: 100.0,
            };
            let tag = BTreeSet::new();
            for kind in [
                MobSpawnHeightmap::WorldSurface,
                MobSpawnHeightmap::OceanFloor,
                MobSpawnHeightmap::MotionBlocking,
                MobSpawnHeightmap::MotionBlockingNoLeaves,
            ] {
                assert_eq!(batch.primed_mob_spawn_height(8, 8, kind).unwrap(), None);
                let mut view =
                    GeneratedMobPlacementView::new(&batch, kind, border, &tag, &mob_height_tags)
                        .unwrap();
                assert_eq!(view.height(8, 8).unwrap(), 16);
                assert_eq!(view.height(8, 8).unwrap(), 16);
            }
            assert!(Arc::ptr_eq(&before, &batch.snapshot().unwrap()));
        }
        let mut mob_spawns = native_resources::mob_spawns::CreatureSpawns::default();
        let attributes = json!({"gameplay/natural_mob_spawns":{
            "spawns_by_category":{"creature":[{"type":"minecraft:sheep","count":2,"weight":1}]},
            "spawn_costs":{}
        }});
        mob_spawns.apply(
            native_resources::mob_spawns::CreatureSpawnLayer::from_attributes(&attributes, |_| {
                Some(false)
            })
            .unwrap()
            .unwrap(),
        );
        let mut planner = batch
            .plan_creature_spawning(center, true, 0.999_999_9, mob_spawns.clone())
            .unwrap();
        let native_resources::worldgen_mob_spawner::CreatureSpawnProgress::Attempt(attempt) =
            planner.next(1).unwrap()
        else {
            panic!("expected creature attempt")
        };
        assert!((0..16).contains(&attempt.x) && (0..16).contains(&attempt.z));
        {
            use native_resources::mob_spawn_placement::MobSpawnHeightmap;
            use native_resources::worldgen_mob_spawner::CreatureSpawnProgress;
            let border = crate::respawn::RespawnBorder {
                min_x: -100.0,
                max_x: 100.0,
                min_z: -100.0,
                max_z: 100.0,
            };
            let tag = BTreeSet::new();
            let mut view = GeneratedMobPlacementView::new(
                &batch,
                MobSpawnHeightmap::MotionBlockingNoLeaves,
                border,
                &tag,
                &mob_height_tags,
            )
            .unwrap();
            let mut probe = GeneratedCreatureProbe::new(attempt.clone(), 0, false, true);
            assert!(probe.collision_geometry(IVec3::ZERO).is_err());
            let top = loop {
                match probe.poll(&mut planner, &mut view, 1).unwrap() {
                    GeneratedCreatureProbeProgress::Pending => continue,
                    GeneratedCreatureProbeProgress::NeedsCollision(pos) => break pos,
                    GeneratedCreatureProbeProgress::Rejected => {
                        panic!("unexpected placement rejection")
                    }
                }
            };
            assert_eq!(top, IVec3::new(attempt.x, 16, attempt.z));
            let geometry = probe.collision_geometry(top).unwrap();
            assert!(geometry.position[0] >= f64::from(0.9_f32));
            assert!(geometry.position[2] >= f64::from(0.9_f32));
            assert_eq!(geometry.position[1], 16.0);
            assert!(probe.collision_geometry(top + IVec3::Y).is_err());
            assert_eq!(
                probe.check_collision(&mut planner, &view).unwrap(),
                GeneratedCreatureCollisionProgress::NeedsSpawnRules(geometry)
            );
            assert_eq!(
                planner.next(0).unwrap(),
                CreatureSpawnProgress::Attempt(attempt.clone())
            );
            let mut rule_planner = batch
                .plan_creature_spawning(center, true, 0.999_999_9, mob_spawns.clone())
                .unwrap();
            assert_eq!(
                rule_planner.next(1).unwrap(),
                CreatureSpawnProgress::Attempt(attempt.clone())
            );
            let mut rule_probe = GeneratedCreatureProbe::new(attempt.clone(), 0, false, true);
            assert!(
                rule_probe
                    .check_spawn_rules(&mut rule_planner, &mut view, &ground_spawn_tags)
                    .is_err()
            );
            loop {
                match rule_probe.poll(&mut rule_planner, &mut view, 1).unwrap() {
                    GeneratedCreatureProbeProgress::Pending => continue,
                    GeneratedCreatureProbeProgress::NeedsCollision(_) => break,
                    GeneratedCreatureProbeProgress::Rejected => {
                        panic!("unexpected rule probe placement rejection")
                    }
                }
            }
            assert_eq!(
                rule_probe
                    .check_collision(&mut rule_planner, &view)
                    .unwrap(),
                GeneratedCreatureCollisionProgress::NeedsSpawnRules(geometry)
            );
            let light_pos = [
                geometry.position[0].floor() as i32,
                geometry.position[1].floor() as i32,
                geometry.position[2].floor() as i32,
            ];
            assert!(
                batch
                    .light_snapshot([center])
                    .unwrap()
                    .raw_brightness(light_pos, 0)
                    .unwrap()
                    <= 8
            );
            assert_eq!(
                rule_probe
                    .check_spawn_rules(&mut rule_planner, &mut view, &ground_spawn_tags)
                    .unwrap(),
                GeneratedCreatureRuleProgress::Rejected
            );
            let CreatureSpawnProgress::Attempt(next) = rule_planner.next(0).unwrap() else {
                panic!("expected retry");
            };
            assert_eq!(
                (next.x, next.z, next.attempt),
                (attempt.x, attempt.z, attempt.attempt + 1)
            );
            // Exercise the successful boundary of the same generated-ground
            // path with a controlled visible light layer at brightness nine.
            let mut light = native_world::generation_light::VisibleLightSnapshot::new(true, false);
            light.insert_complete_column(
                center,
                native_world::generation_light::VisibleLightColumn::new(
                    BTreeMap::from([(1, Arc::new([0x99; native_world::loaded::LIGHT_BYTES]))]),
                    BTreeMap::new(),
                ),
            );
            view.set_rule_light_for_test(center, light);
            let mut accepted_planner = batch
                .plan_creature_spawning(center, true, 0.999_999_9, mob_spawns.clone())
                .unwrap();
            assert_eq!(
                accepted_planner.next(1).unwrap(),
                CreatureSpawnProgress::Attempt(attempt.clone())
            );
            let mut accepted = GeneratedCreatureProbe::new(attempt.clone(), 0, false, true);
            loop {
                match accepted.poll(&mut accepted_planner, &mut view, 1).unwrap() {
                    GeneratedCreatureProbeProgress::Pending => continue,
                    GeneratedCreatureProbeProgress::NeedsCollision(_) => break,
                    GeneratedCreatureProbeProgress::Rejected => {
                        panic!("unexpected accepted probe placement rejection")
                    }
                }
            }
            assert_eq!(
                accepted
                    .check_collision(&mut accepted_planner, &view)
                    .unwrap(),
                GeneratedCreatureCollisionProgress::NeedsSpawnRules(geometry)
            );
            assert_eq!(
                accepted
                    .check_spawn_rules(&mut accepted_planner, &mut view, &ground_spawn_tags)
                    .unwrap(),
                GeneratedCreatureRuleProgress::NeedsConstruction(geometry)
            );
            assert_eq!(
                accepted_planner.next(0).unwrap(),
                CreatureSpawnProgress::Attempt(attempt.clone())
            );
            assert!(
                accepted
                    .check_spawn_rules(&mut accepted_planner, &mut view, &ground_spawn_tags)
                    .is_err()
            );
            // The harness represents a successful external constructor. The
            // entity itself is not yet implemented by this generation path.
            let pose = accepted.mark_constructed(&mut accepted_planner).unwrap();
            assert_eq!(pose.geometry, geometry);
            assert!((0.0..360.0).contains(&pose.yaw));
            assert_eq!(accepted_planner.created_attempt(), Some(attempt.clone()));
            assert!(accepted_planner.next(0).is_err());
            assert_eq!(
                accepted
                    .check_instance_rules(&mut accepted_planner, &mut view)
                    .unwrap(),
                GeneratedCreatureInstanceRuleProgress::Rejected
            );
            let CreatureSpawnProgress::Attempt(next) = accepted_planner.next(0).unwrap() else {
                panic!("expected post-construction retry");
            };
            assert_eq!(
                (next.member, next.attempt),
                (attempt.member, attempt.attempt + 1)
            );
            assert!(accepted.mark_constructed(&mut accepted_planner).is_err());

            let mut light = native_world::generation_light::VisibleLightSnapshot::new(true, false);
            light.insert_complete_column(
                center,
                native_world::generation_light::VisibleLightColumn::new(
                    BTreeMap::from([(1, Arc::new([0xff; native_world::loaded::LIGHT_BYTES]))]),
                    BTreeMap::new(),
                ),
            );
            view.set_rule_light_for_test(center, light);
            let mut bright_planner = batch
                .plan_creature_spawning(center, true, 0.999_999_9, mob_spawns.clone())
                .unwrap();
            assert_eq!(
                bright_planner.next(1).unwrap(),
                CreatureSpawnProgress::Attempt(attempt.clone())
            );
            let mut bright = GeneratedCreatureProbe::new(attempt.clone(), 0, false, true);
            loop {
                match bright.poll(&mut bright_planner, &mut view, 1).unwrap() {
                    GeneratedCreatureProbeProgress::Pending => continue,
                    GeneratedCreatureProbeProgress::NeedsCollision(_) => break,
                    GeneratedCreatureProbeProgress::Rejected => {
                        panic!("unexpected bright probe rejection")
                    }
                }
            }
            assert_eq!(
                bright.check_collision(&mut bright_planner, &view).unwrap(),
                GeneratedCreatureCollisionProgress::NeedsSpawnRules(geometry)
            );
            assert_eq!(
                bright
                    .check_spawn_rules(&mut bright_planner, &mut view, &ground_spawn_tags)
                    .unwrap(),
                GeneratedCreatureRuleProgress::NeedsConstruction(geometry)
            );
            let bright_pose = bright.mark_constructed(&mut bright_planner).unwrap();
            assert_eq!(
                bright
                    .check_instance_rules(&mut bright_planner, &mut view)
                    .unwrap(),
                GeneratedCreatureInstanceRuleProgress::NeedsObstruction(bright_pose)
            );
            assert_eq!(bright_planner.created_attempt(), Some(attempt.clone()));
            assert!(
                bright
                    .check_instance_rules(&mut bright_planner, &mut view)
                    .is_err()
            );
            // The harness's constructed sheep has the verified unscaled box;
            // spawn_scale for this type is 1.0.
            let entity_bounds = native_world::movement::BoundingBox {
                min: glam::DVec3::from_array(geometry.min),
                max: glam::DVec3::from_array(geometry.max),
            };
            // Exercise the generated registry and repeated-state fluid cache
            // on a real in-bounds section before the out-of-height box scan.
            use super::mob_obstruction::FluidScanWorld;
            let ground = IVec3::new(
                geometry.position[0].floor() as i32,
                15,
                geometry.position[2].floor() as i32,
            );
            assert!(!view.has_fluid(ground).unwrap());
            assert!(!view.has_fluid(ground).unwrap());
            let missing_bounds = native_world::movement::BoundingBox {
                min: entity_bounds.min + glam::DVec3::new(1000.0, 0.0, 0.0),
                max: entity_bounds.max + glam::DVec3::new(1000.0, 0.0, 0.0),
            };
            assert!(
                bright
                    .check_obstruction(&mut bright_planner, &mut view, missing_bounds)
                    .is_err()
            );
            assert_eq!(bright_planner.created_attempt(), Some(attempt.clone()));
            assert_eq!(
                bright
                    .check_obstruction(&mut bright_planner, &mut view, entity_bounds)
                    .unwrap(),
                GeneratedCreatureObstructionProgress::NeedsFinalization(bright_pose)
            );
            assert!(
                bright
                    .check_obstruction(&mut bright_planner, &mut view, entity_bounds)
                    .is_err()
            );
            let tag = BTreeSet::from(["minecraft:void_air".into()]);
            let mut view = GeneratedMobPlacementView::new(
                &batch,
                MobSpawnHeightmap::MotionBlockingNoLeaves,
                border,
                &tag,
                &mob_height_tags,
            )
            .unwrap();
            let mut denied = GeneratedCreatureProbe::new(attempt.clone(), 0, false, true);
            loop {
                match denied.poll(&mut planner, &mut view, 1).unwrap() {
                    GeneratedCreatureProbeProgress::Pending => continue,
                    GeneratedCreatureProbeProgress::Rejected => break,
                    GeneratedCreatureProbeProgress::NeedsCollision(_) => {
                        panic!("tagged void air was accepted")
                    }
                }
            }
            assert!(denied.poll(&mut planner, &mut view, 1).is_err());
            drop(view);

            let position = geometry
                .position
                .map(|coordinate| coordinate.floor() as i32);
            let tags =
                native_resources::worldgen_region_random::SheepBiomeTags::load(&mut data).unwrap();
            let mut random = native_resources::worldgen_region_random::RegionRandom::new(
                42,
                native_resources::density::WorldgenRandom::Legacy,
                0,
                0,
            );
            let mut expected_random = random.clone();
            let mut group = native_resources::worldgen_region_random::AnimalGroupData::default();
            let mut expected_group = group;
            assert!(
                batch
                    .initialize_sheep([1_000_000, 0, 0], &tags, &mut random, &mut group, false)
                    .is_err()
            );
            let expected = native_resources::worldgen_region_random::finalize_sheep(
                &mut expected_random,
                &mut expected_group,
                false,
                true,
                false,
            );
            assert_eq!(
                batch
                    .initialize_sheep(position, &tags, &mut random, &mut group, false)
                    .unwrap(),
                expected
            );
            assert_eq!(group, expected_group);
            assert_eq!(random.next_long(), expected_random.next_long());
            let variants = native_resources::animal_variants::FarmAnimalSpawnVariants::load_active(
                &mut data,
                native_resources::animal_variants::FarmAnimal::Pig,
            )
            .unwrap();
            let mut random = native_resources::worldgen_region_random::RegionRandom::new(
                91,
                native_resources::density::WorldgenRandom::Legacy,
                0,
                0,
            );
            let mut expected_random = random.clone();
            let mut group = native_resources::worldgen_region_random::AnimalGroupData::default();
            let mut expected_group = group;
            assert!(
                batch
                    .initialize_farm_animal(
                        [1_000_000, 0, 0],
                        &variants,
                        &mut random,
                        &mut group,
                        false
                    )
                    .is_err()
            );
            let expected = variants
                .finalize(
                    &ResourceId::parse("minecraft:plains").unwrap(),
                    &mut expected_random,
                    &mut expected_group,
                    false,
                )
                .unwrap();
            assert_eq!(
                batch
                    .initialize_farm_animal(position, &variants, &mut random, &mut group, false)
                    .unwrap(),
                expected
            );
            assert_eq!(group, expected_group);
            assert_eq!(random.next_long(), expected_random.next_long());
            struct ConditionalPigData;
            impl ResourceProvider for ConditionalPigData {
                fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                    Ok(None)
                }
                fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                    Ok(match path {
                        "data/minecraft/pig_variant/temperate.json" => Some(br#"{"spawn_conditions":[{"priority":0}]}"#.to_vec()),
                        "data/minecraft/pig_variant/moon.json" => Some(br#"{"spawn_conditions":[{"priority":1,"condition":{"type":"minecraft:moon_brightness","range":{"min":0.9}}}]}"#.to_vec()),
                        "data/minecraft/pig_sound_variant/classic.json" | "data/minecraft/worldgen/biome/plains.json" => Some(b"{}".to_vec()),
                        _ => None,
                    })
                }
                fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                    let names: &[&str] = match folder {
                        "pig_variant" => &["minecraft:temperate", "minecraft:moon"],
                        "pig_sound_variant" => &["minecraft:classic"],
                        "worldgen/biome" => &["minecraft:plains"],
                        _ => &[],
                    };
                    names.iter().map(|id| ResourceId::parse(id)).collect()
                }
            }
            let conditional =
                native_resources::animal_variants::FarmAnimalSpawnVariants::load_active(
                    &mut ConditionalPigData,
                    native_resources::animal_variants::FarmAnimal::Pig,
                )
                .unwrap();
            let moon = native_resources::moon_phase::Environment {
                base: native_resources::moon_phase::Phase::NewMoon,
                biomes: BTreeMap::new(),
                has_biome_layer: false,
                tracks: vec![native_resources::moon_phase::Track::parse(
                    &json!({"clock":"example:clock","period_ticks":20}),
                    &json!({"keyframes":[{"ticks":0,"value":"full_moon"},{"ticks":10,"value":"new_moon"}]}),
                ).unwrap()],
            };
            let mut random = native_resources::worldgen_region_random::RegionRandom::new(
                91,
                native_resources::density::WorldgenRandom::Legacy,
                0,
                0,
            );
            let mut original = random.clone();
            let mut group = native_resources::worldgen_region_random::AnimalGroupData::default();
            assert!(
                batch
                    .initialize_farm_animal(position, &conditional, &mut random, &mut group, false)
                    .is_err()
            );
            assert_eq!(random.next_long(), original.next_long());
            for (ticks, expected) in [(0, "minecraft:moon"), (10, "minecraft:temperate")] {
                let clocks = BTreeMap::from([(ResourceId::parse("example:clock").unwrap(), ticks)]);
                let actual = batch
                    .initialize_farm_animal_with_moon(
                        position,
                        &conditional,
                        &mut random,
                        &mut group,
                        false,
                        Some((&moon, &clocks)),
                    )
                    .unwrap();
                assert_eq!(actual.variant, Some(ResourceId::parse(expected).unwrap()));
            }
            let saved = || GeneratedEntityAddition {
                block_position: position,
                record: GeneratedEntityRecord::Saved(native_persistence::nbt::Tag::Compound(vec![
                    ("test".into(), native_persistence::nbt::Tag::Int(7)),
                ])),
            };
            assert!(
                bright
                    .record_finalized(&mut bright_planner, &mut batch, vec![])
                    .is_err()
            );
            assert!(
                bright
                    .record_finalized(
                        &mut bright_planner,
                        &mut batch,
                        vec![
                            saved(),
                            GeneratedEntityAddition {
                                block_position: [1_000_000, 0, 0],
                                record: GeneratedEntityRecord::Passenger,
                            },
                        ]
                    )
                    .is_err()
            );
            assert!(
                batch
                    .recorded_generated_entities(center)
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(bright_planner.created_attempt(), Some(attempt));
            let mut expected_planner = bright_planner.clone();
            expected_planner.finish_entity(true).unwrap();
            let mut transaction_random = random.clone();
            let mut transaction_group = group;
            let mut expected_random = transaction_random.clone();
            let mut expected_group = transaction_group;
            assert!(
                bright
                    .finalize_sheep_and_record(
                        &mut bright_planner,
                        &mut batch,
                        position,
                        &tags,
                        &mut transaction_random,
                        &mut transaction_group,
                        false,
                        |_| anyhow::bail!("serialization failed"),
                    )
                    .is_err()
            );
            assert!(
                bright
                    .finalize_sheep_and_record(
                        &mut bright_planner,
                        &mut batch,
                        position,
                        &tags,
                        &mut transaction_random,
                        &mut transaction_group,
                        false,
                        |_| Ok(vec![
                            saved(),
                            GeneratedEntityAddition {
                                block_position: [1_000_000, 0, 0],
                                record: GeneratedEntityRecord::Passenger,
                            }
                        ]),
                    )
                    .is_err()
            );
            assert!(
                batch
                    .recorded_generated_entities(center)
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(transaction_group, expected_group);
            assert_eq!(
                transaction_random.clone().next_long(),
                expected_random.clone().next_long()
            );
            let expected = native_resources::worldgen_region_random::finalize_sheep(
                &mut expected_random,
                &mut expected_group,
                false,
                true,
                false,
            );
            let initialized = bright
                .finalize_fresh_sheep_and_record(
                    &mut bright_planner,
                    &mut batch,
                    &tags,
                    &mut transaction_random,
                    &mut transaction_group,
                    [0, 1, 0, 2],
                )
                .unwrap();
            assert_eq!(initialized, expected);
            assert_eq!(transaction_group, expected_group);
            assert_eq!(transaction_random.next_long(), expected_random.next_long());
            assert_eq!(batch.recorded_generated_entities(center).unwrap().len(), 1);
            assert_eq!(
                batch.recorded_generated_entities(center).unwrap()[0],
                draft_fresh_sheep([0, 1, 0, 2], geometry.position, bright_pose.yaw, expected)
                    .unwrap()
            );
            assert_eq!(
                bright_planner.next(0).unwrap(),
                expected_planner.next(0).unwrap()
            );
            assert!(
                bright
                    .record_finalized(&mut bright_planner, &mut batch, vec![saved()])
                    .is_err()
            );
        }
        assert_eq!(
            batch.schedule().completed(center),
            Some(schedule::GenerationStage::Light)
        );
        let after = batch.light_snapshot([center]).unwrap();
        let after_view = batch.lit_snapshot([center]).unwrap();
        assert!(!Arc::ptr_eq(&before_view, &after_view));
        assert!(Arc::ptr_eq(
            &after_view,
            &batch.lit_snapshot([center]).unwrap()
        ));
        for y in -16..32 {
            for z in 0..16 {
                for x in 0..16 {
                    let pos = [x, y, z];
                    assert_eq!(
                        after_view.light(x, y, z, false),
                        after.block_light(pos),
                        "block light {pos:?}"
                    );
                    assert_eq!(
                        after_view.light(x, y, z, true),
                        after.sky_light(pos),
                        "sky light {pos:?}"
                    );
                }
            }
        }
        assert_eq!(before_view.light(8, 8, 8, false), Some(0));
        assert_eq!(after_view.light(8, 8, 8, false), Some(1));
        assert_eq!(
            after_view.chunks[&center].status.as_deref(),
            Some("minecraft:terrain")
        );
        assert!(!after_view.chunks[&center].block_light_correct);
        let render = batch.lit_render_snapshot([center]).unwrap();
        assert_eq!(render.light(8, 8, 8, false), Some(1));
        assert_eq!(render.light(16, 8, 8, true), Some(0));
        assert!(Arc::ptr_eq(
            &original.chunks[&ChunkPos { x: 2, z: 0 }],
            &render.chunks[&ChunkPos { x: 2, z: 0 }]
        ));
        assert!(!Arc::ptr_eq(&original.chunks[&east], &render.chunks[&east]));
        assert_eq!(after.block_light([8, 8, 8]), Some(1));
        assert_eq!(after.sky_light([8, 8, 8]), Some(0));
        assert_eq!(before.block_light([8, 8, 8]), Some(0));
        assert!(!batch.snapshot().unwrap().chunks[&center].block_light_correct);
        assert_eq!(
            batch.schedule().completed(later),
            Some(schedule::GenerationStage::Features)
        );
        let cached = batch.lit_snapshot([center]).unwrap();
        batch.request_light_initialization(later).unwrap();
        assert_eq!(
            batch.step(&mut data, &mut classify, &mut resolve).unwrap(),
            Some((later, schedule::GenerationStage::InitializeLight))
        );
        assert!(batch.is_complete());
        assert!(!Arc::ptr_eq(
            &cached,
            &batch.lit_snapshot([center]).unwrap()
        ));
        // This feature can reach the initialized eastern chunk, but its
        // actual mushroom placement stays in the new center chunk.
        let adjacent = ChunkPos { x: 3, z: 0 };
        batch.request_features(adjacent).unwrap();
        while batch
            .step(&mut data, &mut classify, &mut resolve)
            .unwrap()
            .is_some()
        {}
        assert_eq!(
            batch.schedule().completed(adjacent),
            Some(schedule::GenerationStage::Features)
        );
        assert_eq!(
            batch.snapshot().unwrap().block(56, 8, 8).unwrap().name,
            "minecraft:brown_mushroom"
        );
        let distant = ChunkPos { x: 6, z: 0 };
        let nearby: Vec<_> = (-1..=1)
            .flat_map(|dz| {
                (-1..=1).map(move |dx| ChunkPos {
                    x: distant.x + dx,
                    z: distant.z + dz,
                })
            })
            .collect();
        for &pos in &nearby {
            batch.request_features(pos).unwrap();
        }
        assert!(batch.request_light(distant).is_err());
        while batch
            .step(&mut data, &mut classify, &mut resolve)
            .unwrap()
            .is_some()
        {}
        for &pos in &nearby {
            assert_eq!(
                batch.schedule().completed(pos),
                Some(schedule::GenerationStage::Features)
            );
            batch.request_light_initialization(pos).unwrap();
            assert!(batch.request_features(ChunkPos { x: 12, z: 0 }).is_err());
        }
        while batch
            .step(&mut data, &mut classify, &mut resolve)
            .unwrap()
            .is_some()
        {}
        batch.request_light(distant).unwrap();
        assert!(batch.request_features(ChunkPos { x: 12, z: 0 }).is_err());
        while batch
            .step(&mut data, &mut classify, &mut resolve)
            .unwrap()
            .is_some()
        {}
        assert_eq!(
            batch.schedule().completed(distant),
            Some(schedule::GenerationStage::Light)
        );
        let extended = batch.light_snapshot([center]).unwrap();
        for y in -16..32 {
            for z in 0..16 {
                for x in 0..16 {
                    let pos = [x, y, z];
                    assert_eq!(extended.block_light(pos), after.block_light(pos));
                    assert_eq!(extended.sky_light(pos), after.sky_light(pos));
                }
            }
        }
        let original = batch.snapshot().unwrap();
        assert!(batch.spawned_section_data(center).is_err());
        assert!(batch.spawned_chunk_draft(center, 42, vec![]).is_err());
        assert!(batch.spawned_heightmap_data(center).is_err());
        assert!(batch.spawned_block_entity_data(center).is_err());
        assert!(batch.spawned_tick_data(center).is_err());
        assert!(batch.spawned_postprocessing_data(center).is_err());
        assert!(batch.begin_spawned_postprocessing(center).is_err());
        assert!(batch.request_spawn_without_mobs(center, true).is_err());
        assert_eq!(
            batch.schedule().completed(center),
            Some(schedule::GenerationStage::Light)
        );
        assert!(
            batch
                .publish_completed_spawn(
                    center,
                    &[
                        GeneratedEntityAddition {
                            block_position: [8, 16, 8],
                            record: GeneratedEntityRecord::Saved(
                                native_persistence::nbt::Tag::Compound(vec![])
                            )
                        },
                        GeneratedEntityAddition {
                            block_position: [1_000_000, 0, 0],
                            record: GeneratedEntityRecord::Passenger
                        },
                    ]
                )
                .is_err()
        );
        assert_eq!(batch.recorded_generated_entities(center).unwrap().len(), 1);
        assert_eq!(
            batch.schedule().completed(center),
            Some(schedule::GenerationStage::Light)
        );
        assert!(batch.is_complete());
        let sheep_tags =
            native_resources::worldgen_region_random::SheepBiomeTags::load(&mut data).unwrap();
        let empty = BTreeSet::new();
        let inputs = SheepSpawnInputs {
            border: crate::respawn::RespawnBorder {
                min_x: -100.0,
                max_x: 100.0,
                min_z: -100.0,
                max_z: 100.0,
            },
            prevent_inside: &empty,
            height_tags: &mob_height_tags,
            ground_tags: &ground_spawn_tags,
            biome_tags: &sheep_tags,
            has_ceiling: false,
        };
        let mut unsupported = native_resources::mob_spawns::CreatureSpawns::default();
        unsupported.apply(native_resources::mob_spawns::CreatureSpawnLayer::from_attributes(
            &json!({"gameplay/natural_mob_spawns":{"spawns_by_category":{"creature":[{"type":"minecraft:pig","count":1,"weight":1}]},"spawn_costs":{}}}), |_| Some(false),
        ).unwrap().unwrap());
        let mut unknown =
            GeneratedSheepSpawner::new(&batch, center, 0.999_999_9, unsupported, inputs).unwrap();
        let mut no_uuid =
            || -> Result<[i32; 4]> { anyhow::bail!("unsupported creature was constructed") };
        for _ in 0..2 {
            assert!(
                unknown
                    .advance(&mut batch, &mut no_uuid)
                    .unwrap_err()
                    .to_string()
                    .contains("unsupported generated creature")
            );
        }
        let probability = [0.5, 0.75, 0.9, 0.99]
            .into_iter()
            .find(|&probability| {
                let mut planner = batch
                    .plan_creature_spawning(center, true, probability, mob_spawns.clone())
                    .unwrap();
                matches!(
                    planner.next(1).unwrap(),
                    native_resources::worldgen_mob_spawner::CreatureSpawnProgress::Attempt(_)
                )
            })
            .expect("seed must produce a sheep group");
        struct TimedData<'a> {
            base: &'a mut FeatureData,
            dimension: Value,
            timeline: Value,
        }
        impl ResourceProvider for TimedData<'_> {
            fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                self.base.read_asset(path)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                match path {
                    "data/example/dimension_type/test.json" => {
                        Ok(Some(serde_json::to_vec(&self.dimension)?))
                    }
                    "data/example/timeline/test.json" => {
                        Ok(Some(serde_json::to_vec(&self.timeline)?))
                    }
                    _ => self.base.read_data(path),
                }
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                self.base.list_data(folder)
            }
        }
        let mut timed = TimedData {
            base: &mut data,
            dimension: json!({"attributes":attributes,"timelines":["example:test"]}),
            timeline: json!({"clock":"example:clock","period_ticks":20,"tracks":{
                "gameplay/creature_world_gen_spawn_probability":{"keyframes":[{"ticks":0,"value":probability}]}
            }}),
        };
        let categories = resources.vanilla_entity_category_registry().unwrap();
        let environment =
            native_resources::creature_timeline::CreatureSpawnEnvironment::load_with_registry(
                &mut timed,
                "example:test",
                &categories,
            )
            .unwrap();
        assert!(
            GeneratedSheepSpawner::from_verified_environment(
                &batch,
                &mut timed,
                center,
                &environment,
                |_| None,
                &categories,
                inputs,
            )
            .is_err()
        );
        let mut driver = GeneratedSheepSpawner::from_verified_environment(
            &batch,
            &mut timed,
            center,
            &environment,
            |_| Some(7),
            &categories,
            inputs,
        )
        .unwrap();
        let mut stale =
            GeneratedSheepSpawner::new(&batch, center, probability, mob_spawns.clone(), inputs)
                .unwrap();
        assert!(driver.schedule_spawn(&mut batch).is_err());
        let mut light = native_world::generation_light::VisibleLightSnapshot::new(true, false);
        light.insert_complete_column(
            center,
            native_world::generation_light::VisibleLightColumn::new(
                BTreeMap::from([(1, Arc::new([0xff; native_world::loaded::LIGHT_BYTES]))]),
                BTreeMap::new(),
            ),
        );
        driver.set_rule_light_for_test(center, light.clone());
        for (name, species) in [
            ("pig", native_resources::animal_variants::FarmAnimal::Pig),
            ("cow", native_resources::animal_variants::FarmAnimal::Cow),
            (
                "chicken",
                native_resources::animal_variants::FarmAnimal::Chicken,
            ),
        ] {
            let mut spawns = native_resources::mob_spawns::CreatureSpawns::default();
            spawns.apply(native_resources::mob_spawns::CreatureSpawnLayer::from_attributes(
                &json!({"gameplay/natural_mob_spawns":{"spawns_by_category":{"creature":[{"type":format!("minecraft:{name}"),"count":1,"weight":1}]},"spawn_costs":{}}}), |_| Some(false),
            ).unwrap().unwrap());
            let mut farm =
                GeneratedSheepSpawner::new(&batch, center, 0.999_999_9, spawns, inputs).unwrap();
            let variants = Arc::new(
                native_resources::animal_variants::FarmAnimalSpawnVariants::load_active(
                    &mut resources,
                    species,
                )
                .unwrap(),
            );
            farm.enable_farm_animal(variants).unwrap();
            farm.set_rule_light_for_test(center, light.clone());
            let mut constructed = 0;
            let mut construct = |id: &ResourceId| {
                assert_eq!(id.path, name);
                constructed += 1;
                Ok(FreshCreatureConstruction {
                    uuid: [0, 2, 0, constructed],
                    chicken_egg_time: (name == "chicken").then_some(if constructed == 1 {
                        5999
                    } else {
                        6000
                    }),
                })
            };
            let mut prepared = false;
            let mut invalid_timer_rejected = false;
            for _ in 0..10_000 {
                match farm.advance_with_construction(&mut batch, &mut construct) {
                    Err(error) if name == "chicken" && !invalid_timer_rejected => {
                        assert!(
                            error
                                .to_string()
                                .contains("invalid constructed chicken egg timer")
                        );
                        invalid_timer_rejected = true;
                    }
                    Err(error) => panic!("unexpected {name} spawn error: {error:#}"),
                    Ok(SheepSpawnProgress::Prepared(_)) => {
                        prepared = true;
                        break;
                    }
                    Ok(SheepSpawnProgress::Finished) => break,
                    Ok(SheepSpawnProgress::Pending | SheepSpawnProgress::Rejected) => {}
                }
            }
            assert!(prepared, "no {name} was prepared");
            assert_eq!(invalid_timer_rejected, name == "chicken");
            let [addition] = farm.prepared_additions() else {
                panic!("unexpected pending animal count")
            };
            let GeneratedEntityRecord::Saved(tag) = &addition.record else {
                panic!("animal was not saved")
            };
            assert!(
                matches!(tag.get("id"), Some(native_persistence::nbt::Tag::String(id)) if id.matches(&format!("minecraft:{name}")))
            );
            if name == "chicken" {
                assert_eq!(
                    tag.get("EggLayTime"),
                    Some(&native_persistence::nbt::Tag::Int(6000))
                );
            }
            drop(farm);
            assert_eq!(batch.recorded_generated_entities(center).unwrap().len(), 1);
        }
        let mut abandoned =
            GeneratedSheepSpawner::new(&batch, center, probability, mob_spawns.clone(), inputs)
                .unwrap();
        abandoned.set_rule_light_for_test(center, light);
        let mut next_uuid = 10;
        let mut uuid = || {
            let value = [0, 1, 0, next_uuid];
            next_uuid += 1;
            Ok(value)
        };
        let mut staged = false;
        for _ in 0..10_000 {
            match abandoned.advance(&mut batch, &mut uuid).unwrap() {
                SheepSpawnProgress::Prepared(_) => {
                    staged = true;
                    break;
                }
                SheepSpawnProgress::Finished => break,
                SheepSpawnProgress::Pending | SheepSpawnProgress::Rejected => {}
            }
        }
        assert!(staged);
        drop(abandoned);
        assert_eq!(batch.recorded_generated_entities(center).unwrap().len(), 1);
        let mut prepared = 0;
        let mut finished = false;
        for _ in 0..10_000 {
            match driver.advance(&mut batch, &mut uuid).unwrap() {
                SheepSpawnProgress::Prepared(_) => prepared += 1,
                SheepSpawnProgress::Finished => {
                    finished = true;
                    break;
                }
                SheepSpawnProgress::Pending | SheepSpawnProgress::Rejected => {}
            }
        }
        assert!(finished && prepared > 0);
        assert_eq!(batch.recorded_generated_entities(center).unwrap().len(), 1);
        driver.schedule_spawn(&mut batch).unwrap();
        assert_eq!(
            batch.recorded_generated_entities(center).unwrap().len(),
            1 + prepared
        );
        assert!(stale.advance(&mut batch, &mut uuid).is_err());
        assert!(driver.schedule_spawn(&mut batch).is_err());
        assert_eq!(
            batch.step(&mut data, &mut classify, &mut resolve).unwrap(),
            Some((center, schedule::GenerationStage::Spawn))
        );
        assert!(batch.is_complete());
        assert!(Arc::ptr_eq(&original, &batch.snapshot().unwrap()));
        let heightmaps = batch.spawned_heightmap_data(center).unwrap();
        let draft = batch.spawned_chunk_draft(center, 42, vec![]).unwrap();
        let recorded_draft = batch.spawned_recorded_chunk_draft(center, 42).unwrap();
        let native_persistence::nbt::Tag::List { values, .. } =
            recorded_draft.value.get("entities").unwrap()
        else {
            panic!("missing generated entities")
        };
        assert_eq!(
            values.len(),
            batch.recorded_generated_entities(center).unwrap().len()
        );
        assert!(values.len() > 1);
        assert_eq!(
            values[0].get("id"),
            Some(&native_persistence::nbt::Tag::String(
                "minecraft:sheep".into()
            ))
        );
        assert_eq!(
            values[0].get("UUID"),
            Some(&native_persistence::nbt::Tag::IntArray(vec![0, 1, 0, 2]))
        );
        assert_ne!(draft, recorded_draft);
        assert_eq!(
            draft.value.get("Status"),
            Some(&native_persistence::nbt::Tag::String(
                "minecraft:spawn".into()
            ))
        );
        assert_eq!(
            draft.value.get("LastUpdate"),
            Some(&native_persistence::nbt::Tag::Long(42))
        );
        assert!(draft.value.get("isLightOn").is_none());
        let native_persistence::nbt::Tag::List {
            values: entities, ..
        } = batch.spawned_block_entity_data(center).unwrap()
        else {
            panic!("missing block entities")
        };
        assert!(entities.is_empty());
        assert!(heightmaps.get("WORLD_SURFACE_WG").is_some());
        assert!(heightmaps.get("OCEAN_FLOOR_WG").is_some());
        let marks = batch.spawned_postprocessing_data(center).unwrap();
        let marks = native_persistence::postprocessing::decode(&marks, 0, 16).unwrap();
        assert_eq!(marks.sections().len(), 1);
        struct TracePostprocessing(Vec<[i32; 3]>);
        impl postprocessing::PostprocessingWorld for TracePostprocessing {
            type State = ();
            type Fluid = ();
            fn block(&mut self, pos: [i32; 3]) -> anyhow::Result<()> {
                self.0.push(pos);
                Ok(())
            }
            fn fluid(&mut self, _: &()) -> anyhow::Result<Option<()>> {
                Ok(None)
            }
            fn tick_fluid(&mut self, _: [i32; 3], _: &(), _: &()) -> anyhow::Result<()> {
                unreachable!()
            }
            fn is_liquid_block(&mut self, _: &()) -> anyhow::Result<bool> {
                Ok(false)
            }
            fn tick_liquid_block(&mut self, _: [i32; 3], _: &()) -> anyhow::Result<()> {
                unreachable!()
            }
            fn update_neighbor_shapes(&mut self, _: [i32; 3], _: &()) -> anyhow::Result<()> {
                Ok(())
            }
            fn set_block(&mut self, _: [i32; 3], _: &(), _: i32) -> anyhow::Result<()> {
                unreachable!()
            }
            fn clear_processed_section(&mut self, _: usize) -> anyhow::Result<()> {
                Ok(())
            }
        }
        impl postprocessing::PostprocessingChunk for TracePostprocessing {
            fn pending_block_entity_positions(&mut self) -> anyhow::Result<Vec<[i32; 3]>> {
                Ok(Vec::new())
            }
            fn promote_pending_block_entity(&mut self, _: [i32; 3]) -> anyhow::Result<()> {
                unreachable!()
            }
            fn clear_pending_block_entities(&mut self) -> anyhow::Result<()> {
                Ok(())
            }
            fn apply_upgrade_data(&mut self) -> anyhow::Result<()> {
                Ok(())
            }
        }
        let expected: Vec<_> = marks
            .sections()
            .iter()
            .enumerate()
            .flat_map(|(section, values)| values.iter().map(move |&packed| (section, packed)))
            .map(|(section, packed)| marks.unpack(section, packed, center.x, center.z).unwrap())
            .collect();
        let mut converted = batch
            .begin_converted_postprocessing_region([center], 42, 10)
            .unwrap();
        let mut trace = TracePostprocessing(Vec::new());
        while !converted
            .tasks
            .get_mut(&center)
            .unwrap()
            .poll(&mut trace, 1)
            .unwrap()
        {}
        assert_eq!(trace.0, expected);
        assert!(batch.begin_spawned_postprocessing(distant).is_err());
        assert!(
            batch
                .begin_converted_postprocessing_region([center, center], 42, 10)
                .is_err()
        );
        assert!(
            batch
                .begin_converted_postprocessing_region([center, distant], 42, 10)
                .is_err()
        );
        let (block_ticks, fluid_ticks) = batch.spawned_tick_data(center).unwrap();
        let native_persistence::nbt::Tag::List { values: ticks, .. } = block_ticks else {
            panic!("missing block ticks")
        };
        assert_eq!(ticks.len(), 1);
        assert_eq!(
            ticks[0].get("i"),
            Some(&native_persistence::nbt::Tag::String(
                "minecraft:brown_mushroom".into()
            ))
        );
        assert_eq!(
            ticks[0].get("t"),
            Some(&native_persistence::nbt::Tag::Int(0))
        );
        let saved = converted.ticks.chunk(center).unwrap().blocks.pack(42);
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].kind, "minecraft:brown_mushroom");
        assert_eq!(saved[0].delay, 0);
        assert!(
            converted
                .ticks
                .chunk(center)
                .unwrap()
                .blocks
                .peek()
                .is_none()
        );
        assert_eq!(
            converted
                .ticks
                .schedule(
                    chunk_ticks::TickRegistry::Block,
                    &saved[0].kind,
                    saved[0].position,
                    1,
                    0
                )
                .unwrap(),
            chunk_ticks::ScheduleOutcome::Duplicate
        );
        assert_eq!(converted.ticks.next_sub_tick(), 11);
        let blocks = &mut converted.ticks.chunk_mut(center).unwrap().blocks;
        blocks.unpack(42);
        let activated = blocks.poll().unwrap();
        assert_eq!(activated.position, saved[0].position);
        assert_eq!(activated.trigger, 42);
        assert_eq!(activated.sub_tick, -1);
        assert!(blocks.poll().is_none());
        let original = batch.snapshot().unwrap();
        assert!(
            batch
                .begin_generated_postprocessing_with_neighbors([center, center], [center], 42, 10)
                .is_err()
        );
        assert!(
            batch
                .begin_generated_postprocessing_with_neighbors([center], [center, center], 42, 10)
                .is_err()
        );
        assert!(
            batch
                .begin_generated_postprocessing_with_neighbors([distant], [center], 42, 10)
                .is_err()
        );
        let mut generated = batch
            .begin_generated_postprocessing_with_neighbors([center], [center], 42, 10)
            .unwrap();
        while !generated.poll(center, 4).unwrap() {}
        let mut processed = generated.finish().unwrap();
        assert!(Arc::ptr_eq(&original, &processed.world));
        assert!(processed.edits.is_empty());
        assert!(processed.light_checks.is_empty());
        assert_eq!(processed.ticks().registration_order(), &[center]);
        let mut captured = processed
            .ticks()
            .captured_indexed_queue(chunk_ticks::TickRegistry::Block, &[center])
            .unwrap();
        let due = captured.collect_due(42, 65_536, |_| true);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].kind, "minecraft:brown_mushroom");
        assert_eq!(due[0].position, saved[0].position);
        let light = batch.postprocessed_light_snapshot(&processed).unwrap();
        let spawn_light = batch.light_snapshot([center]).unwrap();
        assert_eq!(
            light.block_light([8, 8, 8]),
            spawn_light.block_light([8, 8, 8])
        );
        assert_eq!(light.sky_light([8, 8, 8]), spawn_light.sky_light([8, 8, 8]));
        let postprocessed_sections = batch
            .postprocessed_sections_data(&processed, [center])
            .unwrap()
            .remove(&center)
            .unwrap();
        let poi = batch
            .postprocessed_missing_poi_snapshot(&processed, [center])
            .unwrap();
        assert!(poi[&center].is_none());
        assert!(
            batch
                .postprocessed_missing_poi_snapshot(&processed, [center, center])
                .is_err()
        );
        assert!(
            batch
                .postprocessed_missing_poi_snapshot(&processed, [distant])
                .is_err()
        );
        assert_eq!(
            postprocessed_sections,
            batch.spawned_section_data(center).unwrap()
        );
        let mut components = batch
            .postprocessed_chunk_components(&processed, [center])
            .unwrap();
        let component = components.remove(&center).unwrap();
        assert!(components.is_empty());
        let mut drafts = batch
            .postprocessed_chunk_drafts(&processed, [center])
            .unwrap();
        let full = drafts.remove(&center).unwrap();
        assert_eq!(
            full.chunk.value.get("Status"),
            Some(&native_persistence::nbt::Tag::String(
                "minecraft:full".into()
            ))
        );
        assert_eq!(
            full.chunk.value.get("LastUpdate"),
            Some(&native_persistence::nbt::Tag::Long(42))
        );
        assert_eq!(
            full.chunk.value.get("isLightOn"),
            Some(&native_persistence::nbt::Tag::Byte(1))
        );
        assert!(full.chunk.value.get("entities").is_none());
        // Exercise structure serialization through both actual chunk draft paths.
        let structure_id = ResourceId::parse("example:treasure").unwrap();
        let point = [center.x * 16 + 9, 8, center.z * 16 + 9];
        let structure = native_resources::structure_start::SimpleStructureStart {
            kind: native_resources::structure_start::SimpleStructureKind::BuriedTreasure,
            chunk: [center.x, center.z],
            generation_point: [center.x * 16 + 8, 8, center.z * 16 + 8],
            orientation: None,
            bounds: native_core::beardifier::BoundingBox::new(point, point).unwrap(),
            terrain_adjustment: native_core::beardifier::TerrainAdjustment::None,
        };
        let bounds = structure_references::StructureStartBounds::from_simple(
            structure_id.clone(),
            &structure,
        )
        .unwrap();
        let references =
            structure_references::collect_ordered_structure_references(center, |source| {
                Ok(if source == center {
                    vec![bounds.clone()]
                } else {
                    vec![]
                })
            })
            .unwrap();
        let mut starts = BTreeMap::new();
        for x in center.x - 8..=center.x + 8 {
            for z in center.z - 8..=center.z + 8 {
                starts.insert(ChunkPos { x, z }, BTreeMap::new());
            }
        }
        starts.insert(center, BTreeMap::from([(structure_id, structure)]));
        batch.set_structure_stages_for_test(Some(structure_batch::PreparedStructureStages {
            starts,
            ocean_starts: BTreeMap::new(),
            mineshafts: BTreeMap::new(),
            jigsaw_starts: BTreeMap::new(),
            huts: BTreeMap::new(),
            references: BTreeMap::from([(center, references)]),
            terrain: BTreeMap::new(),
        }));
        let structure_nbt = batch.structure_data(center).unwrap();
        let spawn_with_structures = batch.spawned_chunk_draft(center, 42, vec![]).unwrap();
        assert_eq!(
            spawn_with_structures.value.get("structures"),
            Some(&structure_nbt)
        );
        let full_with_structures = batch
            .postprocessed_chunk_drafts(&processed, [center])
            .unwrap()
            .remove(&center)
            .unwrap();
        assert_eq!(
            full_with_structures.chunk.value.get("structures"),
            Some(&structure_nbt)
        );
        assert!(full_with_structures.entity_records.is_none());
        assert!(
            batch
                .postprocessed_region_drafts(&processed, [center], 0)
                .is_err()
        );
        // Exercise mineshaft metadata through the same chunk draft paths.
        let mine_id = ResourceId::parse("minecraft:mineshaft").unwrap();
        let x = center.x * 16 + 2;
        let z = center.z * 16 + 2;
        let piece = native_resources::mineshaft::MineshaftGraphPiece {
            bounds: native_core::beardifier::BoundingBox::new([x, 1, z], [x + 8, 6, z + 8])
                .unwrap(),
            direction: None,
            depth: 0,
            kind: native_resources::mineshaft::MineshaftGraphPieceKind::Room { entrances: vec![] },
        };
        let mine = mineshaft::PreparedStart {
            chunk: [center.x, center.z],
            generation_point: [x + 6, 1, z - 2],
            terrain_adjustment: native_core::beardifier::TerrainAdjustment::None,
            variant: native_resources::mineshaft::MineshaftVariant::Normal,
            pieces: vec![
                SavedMineshaftPiece::capture(
                    &piece,
                    native_resources::mineshaft::MineshaftVariant::Normal,
                    false,
                )
                .unwrap(),
            ],
        };
        let bounds = mine.reference_bounds(mine_id.clone()).unwrap();
        let refs = structure_references::collect_ordered_structure_references(center, |source| {
            Ok(if source == center {
                vec![bounds.clone()]
            } else {
                vec![]
            })
        })
        .unwrap();
        let mut starts = BTreeMap::new();
        for sx in center.x - 8..=center.x + 8 {
            for sz in center.z - 8..=center.z + 8 {
                starts.insert(ChunkPos { x: sx, z: sz }, BTreeMap::new());
            }
        }
        let expected = mine.to_tag(&mine_id).unwrap();
        batch.set_structure_stages_for_test(Some(structure_batch::PreparedStructureStages {
            starts,
            ocean_starts: BTreeMap::new(),
            mineshafts: BTreeMap::from([(center, BTreeMap::from([(mine_id, mine)]))]),
            jigsaw_starts: BTreeMap::new(),
            huts: BTreeMap::new(),
            references: BTreeMap::from([(center, refs)]),
            terrain: BTreeMap::new(),
        }));
        assert!(batch.structure_starts(center).is_none());
        assert!(batch.mineshaft_starts(center).is_some());
        let mine_data = batch.structure_data(center).unwrap();
        assert_eq!(
            mine_data
                .get("starts")
                .and_then(|v| v.get("minecraft:mineshaft")),
            Some(&expected)
        );
        assert_eq!(
            batch
                .spawned_chunk_draft(center, 42, vec![])
                .unwrap()
                .value
                .get("structures"),
            Some(&mine_data)
        );
        assert_eq!(
            batch
                .postprocessed_chunk_drafts(&processed, [center])
                .unwrap()[&center]
                .chunk
                .value
                .get("structures"),
            Some(&mine_data)
        );
        batch.set_structure_stages_for_test(None);
        assert_eq!(full.entity_records, component.entity_records);
        let expected_entities = full.entity_records.as_ref().map(|records| {
            native_persistence::generated_chunk::assemble_entity_chunk(center, records).unwrap()
        });
        assert_eq!(full.entity_chunk, expected_entities);
        assert_eq!(component.sections, postprocessed_sections);
        assert_eq!(
            component.heightmaps,
            processed.heightmaps[&center].to_tag().unwrap()
        );
        assert_eq!(
            component.block_entities,
            batch.spawned_block_entity_data(center).unwrap()
        );
        assert_eq!(
            component.entity_records.as_deref(),
            batch
                .complete_generated_entity_records_shared(center)
                .unwrap()
                .as_deref()
        );
        let native_persistence::nbt::Tag::List {
            values: saved_blocks,
            ..
        } = component.block_ticks
        else {
            panic!("missing level block ticks")
        };
        assert_eq!(saved_blocks.len(), 1);
        assert_eq!(
            saved_blocks[0].get("i"),
            Some(&native_persistence::nbt::Tag::String(
                "minecraft:brown_mushroom".into()
            ))
        );
        assert_eq!(
            saved_blocks[0].get("t"),
            Some(&native_persistence::nbt::Tag::Int(0))
        );
        let native_persistence::nbt::Tag::List {
            values: saved_fluids,
            ..
        } = component.fluid_ticks
        else {
            panic!("missing level fluid ticks")
        };
        assert!(saved_fluids.is_empty());
        assert!(
            batch
                .postprocessed_sections_data(&processed, [distant])
                .is_err()
        );
        assert!(
            batch
                .postprocessed_sections_data(&processed, [center, center])
                .is_err()
        );
        assert!(batch.postprocessed_sections_data(&processed, []).is_err());
        // Exercise the encoder with a changed block and its queued light check.
        let mut changed_world = (*processed.world).clone();
        let air = native_world::loaded::BlockState {
            name: "minecraft:air".into(),
            properties: Default::default(),
        };
        assert_eq!(
            native_world::block_light::same_light_behavior(
                changed_world.block(8, 8, 8).unwrap(),
                &air
            ),
            Some(false)
        );
        let air_id = changed_world.intern_state(air);
        assert!(changed_world.set_block_id(8, 8, 8, air_id).unwrap());
        processed.world = Arc::new(changed_world);
        processed.sealed_world = Arc::clone(&processed.world);
        processed
            .light_checks
            .push(native_world::loaded::BlockPosition { x: 8, y: 8, z: 8 });
        processed.sealed_checks = processed.light_checks.clone();
        processed
            .edits
            .push(native_world::loaded::BlockPosition { x: 8, y: 8, z: 8 });
        processed.sealed_edits = processed.edits.clone();
        let changed_components = batch
            .postprocessed_chunk_components(&processed, [center])
            .unwrap();
        assert_eq!(
            changed_components[&center].block_entities,
            batch.spawned_block_entity_data(center).unwrap()
        );
        let changed_sections = batch
            .postprocessed_sections_data(&processed, [center])
            .unwrap()
            .remove(&center)
            .unwrap();
        let changed = native_persistence::palette::decode_sections(
            &native_persistence::nbt::Tag::Compound(vec![("sections".into(), changed_sections)]),
        )
        .unwrap();
        let updated_light = batch.postprocessed_light_snapshot(&processed).unwrap();
        let rendered = batch
            .postprocessed_render_snapshot(&processed, [center])
            .unwrap();
        assert_eq!(rendered.block(8, 8, 8).unwrap().name, "minecraft:air");
        assert_eq!(
            rendered.light(8, 8, 8, false),
            updated_light.block_light([8, 8, 8])
        );
        assert!(rendered.chunks.contains_key(&ChunkPos { x: -1, z: -1 }));
        assert!(
            batch
                .postprocessed_render_snapshot(&processed, [distant])
                .is_err()
        );
        let section_light = changed
            .iter()
            .find(|section| section.y == 0)
            .unwrap()
            .block_light
            .as_ref()
            .unwrap();
        let nibble_index = 8 + 8 * 16 + 8 * 256;
        let encoded_light = (section_light[nibble_index / 2] >> (4 * (nibble_index % 2))) & 15;
        assert_eq!(Some(encoded_light), updated_light.block_light([8, 8, 8]));
        assert_ne!(
            updated_light.block_light([8, 8, 8]),
            spawn_light.block_light([8, 8, 8])
        );
        assert_eq!(
            changed
                .iter()
                .find(|section| section.y == 0)
                .unwrap()
                .blocks
                .as_ref()
                .unwrap()
                .block(8, 8, 8)
                .unwrap()
                .name
                .to_string_lossy(),
            "minecraft:air"
        );
        assert_eq!(
            original.block(8, 8, 8).unwrap().name,
            "minecraft:brown_mushroom"
        );
        let mut chest_world = (*processed.world).clone();
        let chest = native_world::loaded::BlockState {
            name: "minecraft:chest".into(),
            properties: resources
                .vanilla_generated_block_registry()
                .unwrap()
                .default_properties("minecraft:chest")
                .unwrap(),
        };
        let chest_id = chest_world.intern_state(chest);
        assert!(chest_world.set_block_id(8, 8, 8, chest_id).unwrap());
        processed.world = Arc::new(chest_world);
        processed.sealed_world = Arc::clone(&processed.world);
        let error = batch
            .postprocessed_chunk_components(&processed, [center])
            .err()
            .expect("changed block entity must be rejected");
        assert!(format!("{error:#}").contains("postprocessing changed a block entity"));
        processed.world = Arc::clone(&original);
        processed.sealed_world = Arc::clone(&original);
        processed.light_checks.clear();
        processed.sealed_checks.clear();
        processed.edits.clear();
        processed.sealed_edits.clear();
        processed
            .edits
            .push(native_world::loaded::BlockPosition { x: 8, y: 8, z: 8 });
        assert!(
            batch
                .postprocessed_chunk_components(&processed, [center])
                .is_err()
        );
        processed.edits.clear();
        processed
            .light_checks
            .push(native_world::loaded::BlockPosition { x: 8, y: 8, z: 8 });
        assert!(batch.postprocessed_light_snapshot(&processed).is_err());
        assert!(
            batch
                .postprocessed_render_snapshot(&processed, [center])
                .is_err()
        );
        assert!(
            batch
                .postprocessed_missing_poi_snapshot(&processed, [center])
                .is_err()
        );
        assert!(
            batch
                .postprocessed_sections_data(&processed, [center])
                .is_err()
        );
        processed.light_checks.clear();
        let revision = processed.batch_revision;
        processed.batch_revision = revision.map(|value| value.wrapping_add(1));
        assert!(batch.postprocessed_light_snapshot(&processed).is_err());
        processed.batch_revision = revision;
        processed.chunks.clear();
        assert!(batch.postprocessed_light_snapshot(&processed).is_err());
        processed.chunks.insert(center);
        assert_eq!(
            processed.heightmaps[&center].to_tag().unwrap(),
            batch.converted_full_heightmap_data(center).unwrap()
        );
        assert_eq!(
            processed.ticks.chunk(center).unwrap().blocks.pack(42),
            saved
        );
        let native_persistence::nbt::Tag::List { values: ticks, .. } = fluid_ticks else {
            panic!("missing fluid ticks")
        };
        assert!(ticks.is_empty());
        let sections = batch.spawned_section_data(center).unwrap();
        let decoded = native_persistence::palette::decode_sections(
            &native_persistence::nbt::Tag::Compound(vec![("sections".into(), sections)]),
        )
        .unwrap();
        let build = decoded.iter().find(|section| section.y == 0).unwrap();
        assert_eq!(
            build
                .blocks
                .as_ref()
                .unwrap()
                .block(8, 8, 8)
                .unwrap()
                .name
                .to_string_lossy(),
            "minecraft:brown_mushroom"
        );
        assert_eq!(
            build
                .biomes
                .as_ref()
                .unwrap()
                .biome(0, 0, 0)
                .unwrap()
                .to_string_lossy(),
            "minecraft:plains"
        );
        assert!(build.block_light.is_some());
        assert!(decoded.iter().all(|section| section.sky_light.is_none()));
        let spawn_light = batch.light_snapshot([center]).unwrap();
        for y in -16..32 {
            for z in 0..16 {
                for x in 0..16 {
                    let pos = [x, y, z];
                    assert_eq!(spawn_light.block_light(pos), extended.block_light(pos));
                    assert_eq!(spawn_light.sky_light(pos), extended.sky_light(pos));
                }
            }
        }
        // Retain a postprocessing edit while extending the original batch far
        // outside every previously generated chunk and active light halo.
        let mut edited = (*processed.world).clone();
        let air = edited.intern_state(native_world::loaded::BlockState {
            name: "minecraft:air".into(),
            properties: Default::default(),
        });
        assert!(edited.set_block_id(8, 8, 8, air).unwrap());
        processed.world = Arc::new(edited);
        processed.sealed_world = Arc::clone(&processed.world);
        let position = native_world::loaded::BlockPosition { x: 8, y: 8, z: 8 };
        processed.edits.push(position);
        processed.sealed_edits.push(position);
        processed.light_checks.push(position);
        processed.sealed_checks.push(position);
        let checkpoint = batch
            .checkpoint_postprocessing_expansion(&processed)
            .unwrap();
        let old_world = Arc::clone(&processed.world);
        processed.edits.push(position);
        assert!(
            batch
                .rebase_postprocessing_after_expansion(&mut processed, &checkpoint)
                .is_err()
        );
        assert!(Arc::ptr_eq(&old_world, &processed.world));
        processed.edits.pop();
        let far = ChunkPos { x: 64, z: 0 };
        batch.request_features(far).unwrap();
        while batch
            .step(&mut data, &mut classify, &mut resolve)
            .unwrap()
            .is_some()
        {}
        batch.request_spawn_without_mobs(distant, false).unwrap();
        while batch
            .step(&mut data, &mut classify, &mut resolve)
            .unwrap()
            .is_some()
        {}
        // A preview must include both the processed target and the converted
        // neighbor, preserving the neighbor's unprocessed work and live ticks.
        let pending_marks = batch.spawned_postprocessing_data(distant).unwrap();
        let mut mixed = batch
            .begin_generated_postprocessing_with_neighbors([center], [center, distant], 42, 10)
            .unwrap();
        assert!(
            batch
                .converted_chunk_drafts(&mut mixed, [center, distant])
                .is_err()
        );
        while !mixed.poll(center, 256).unwrap() {}
        let tick_position = [distant.x * 16 + 1, 8, distant.z * 16 + 1];
        mixed
            .region
            .ticks
            .schedule(
                chunk_ticks::TickRegistry::Block,
                "minecraft:stone",
                tick_position,
                4,
                0,
            )
            .unwrap();
        assert!(batch.converted_chunk_drafts(&mut mixed, [center]).is_err());
        assert!(
            batch
                .converted_chunk_drafts(&mut mixed, [center, center, distant])
                .is_err()
        );
        let drafts = batch
            .converted_chunk_drafts(&mut mixed, [center, distant])
            .unwrap();
        assert_eq!(drafts.len(), 2);
        let neighbor = &drafts[&distant].chunk;
        assert_eq!(
            neighbor.value.get("Status"),
            Some(&native_persistence::nbt::Tag::String(
                "minecraft:full".into()
            ))
        );
        assert_eq!(neighbor.value.get("PostProcessing"), Some(&pending_marks));
        let Some(native_persistence::nbt::Tag::List { values: ticks, .. }) =
            neighbor.value.get("block_ticks")
        else {
            panic!("missing block ticks")
        };
        assert!(ticks.iter().any(|tick| tick.get("i")
            == Some(&native_persistence::nbt::Tag::String(
                "minecraft:stone".into()
            ))
            && tick.get("t") == Some(&native_persistence::nbt::Tag::Int(4))));
        assert_eq!(
            native_persistence::nbt::read(&native_persistence::nbt::write(neighbor).unwrap())
                .unwrap(),
            *neighbor
        );
        let mut deferred = mixed.finish().unwrap();
        assert_eq!(
            deferred.deferred_chunks().collect::<Vec<_>>(),
            vec![distant]
        );
        // Entity source coverage is intentionally incomplete in this fixture;
        // even with all terrain chunks present, no region set may be staged.
        assert!(batch.converted_region_drafts(&deferred, 1).is_err());
        let deferred_checkpoint = batch
            .checkpoint_postprocessing_expansion(&deferred)
            .unwrap();
        // Even a new pass with the same visible blocks and tasks is a distinct
        // snapshot: its tick and block entity state cannot reuse this proof.
        let mut different = batch
            .resume_generated_postprocessing(&deferred)
            .unwrap()
            .finish()
            .unwrap();
        assert!(
            batch
                .rebase_postprocessing_after_expansion(&mut different, &deferred_checkpoint)
                .is_err()
        );
        let farther = ChunkPos { x: 80, z: 0 };
        batch.request_features(farther).unwrap();
        while batch
            .step(&mut data, &mut classify, &mut resolve)
            .unwrap()
            .is_some()
        {}
        batch
            .rebase_postprocessing_after_expansion(&mut deferred, &deferred_checkpoint)
            .unwrap();
        assert_eq!(
            deferred.deferred_postprocessing_data(distant).unwrap(),
            pending_marks
        );
        assert!(
            deferred
                .ticks()
                .chunk(distant)
                .unwrap()
                .blocks
                .pack(42)
                .iter()
                .any(|tick| tick.kind == "minecraft:stone"
                    && tick.position == tick_position
                    && tick.delay == 4)
        );
        let mut mixed = batch.resume_generated_postprocessing(&deferred).unwrap();
        mixed.request_converted_chunk(distant).unwrap();
        while !mixed.poll(distant, 256).unwrap() {}
        assert_eq!(mixed.finish().unwrap().deferred_chunks().count(), 0);
        assert!(batch.validate_postprocessing(&processed).is_err());
        batch
            .rebase_postprocessing_after_expansion(&mut processed, &checkpoint)
            .unwrap();
        batch.validate_postprocessing(&processed).unwrap();
        let reads = processed.read_chunks.clone();
        processed.read_chunks.insert(ChunkPos {
            x: i32::MAX,
            z: i32::MAX,
        });
        assert!(batch.validate_postprocessing(&processed).is_err());
        processed.read_chunks = reads;
        batch.validate_postprocessing(&processed).unwrap();
        assert_eq!(
            processed.world.block(8, 8, 8).unwrap().name,
            "minecraft:air"
        );
        assert_eq!(
            processed.source.block(8, 8, 8).unwrap().name,
            "minecraft:brown_mushroom"
        );
        assert!(processed.world.chunks.contains_key(&far));
        assert!(batch.postprocessed_light_snapshot(&processed).is_ok());
        let mut resumed = batch.resume_generated_postprocessing(&processed).unwrap();
        assert!(resumed.poll(center, 256).is_err());
        batch
            .register_generated_postprocessing_chunk(&mut resumed, distant)
            .unwrap();
        while !resumed.poll(distant, 256).unwrap() {}
        let extended = resumed.finish().unwrap();
        batch.validate_postprocessing(&extended).unwrap();
        assert!(extended.chunks.contains(&center));
        assert!(extended.chunks.contains(&distant));
        assert!(!processed.chunks.contains(&distant));
        assert_eq!(extended.world.block(8, 8, 8).unwrap().name, "minecraft:air");
        assert_eq!(extended.ticks().registration_order(), &[center, distant]);
        assert!(batch.request_spawn_without_mobs(center, false).is_err());
        let (schedule, _, terrain, light) = batch.into_parts().unwrap();
        assert_eq!(
            schedule.completed(center),
            Some(schedule::GenerationStage::Spawn)
        );
        let (FreshBatchLight::Staged(engine), registry) = light.unwrap() else {
            panic!("expected propagated light history")
        };
        assert_eq!(
            engine
                .snapshot(&terrain.unwrap(), [center], &registry)
                .unwrap()
                .block_light([8, 8, 8]),
            Some(1)
        );
    }
    #[test]
    fn fresh_batch_executes_features_with_initial_light_and_rolls_back_failures() {
        use native_resources::{
            carver::tags::CarverBlockTags, dimension_lighting::GeneratedDimensionLighting,
        };
        use native_world::loaded::{BlockState, ChunkPos};
        struct FeatureData {
            unsupported: bool,
        }
        impl ResourceProvider for FeatureData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/worldgen/biome/plains.json" {
                    let kind = if self.unsupported {
                        "example:unsupported"
                    } else {
                        "minecraft:simple_block"
                    };
                    return Ok(Some(serde_json::to_vec(
                        &json!({"has_precipitation":true,"temperature":0.8,"carvers":[],"features":[[
                            {"feature":{"type":kind,"to_place":{"id":"minecraft:melon"}},"placement":[{"type":"minecraft:offset","x":8,"y":8,"z":8}]},
                            {"feature":{"type":"minecraft:simple_block","to_place":{"id":"minecraft:red_mushroom"}},"placement":[{"type":"minecraft:offset","x":9,"y":8,"z":8}]}
                        ]]}),
                    )?));
                }
                if path == "data/minecraft/tags/block/blocks_motion_in_heightmap.json"
                    || path == "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json"
                {
                    return Ok(Some(
                        br#"{"values":["minecraft:stone","minecraft:dirt","minecraft:melon"]}"#
                            .to_vec(),
                    ));
                }
                if path == "data/minecraft/tags/block/overrides_mushroom_light_requirement.json" {
                    return Ok(Some(br#"{"values":[]}"#.to_vec()));
                }
                Data.read_data(path)
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                Data.list_data(folder)
            }
        }
        let registry: BTreeSet<_> = [
            "air",
            "cave_air",
            "void_air",
            "bedrock",
            "stone",
            "dirt",
            "water",
            "lava",
            "melon",
            "red_mushroom",
        ]
        .into_iter()
        .map(|s| format!("minecraft:{s}"))
        .collect();
        let center = ChunkPos { x: 0, z: 0 };
        for (has_sky, unsupported) in [(true, false), (false, false), (false, true)] {
            let mut data = FeatureData { unsupported };
            let tags = CarverBlockTags::load(&mut data, &registry).unwrap();
            let generator =
                TerrainGenerator::without_terrain_adjustments(&mut data, &definition(), 42, 0, 16)
                    .unwrap();
            let mut batch =
                FreshTerrainBatch::new_without_structures(generator, tags, "minecraft:overworld")
                    .unwrap();
            assert!(batch.request_features(center).is_err());
            assert!(batch.spawn_heightmaps(center).is_err());
            let lighting = GeneratedDimensionLighting {
                dimension: ResourceId::parse("minecraft:overworld").unwrap(),
                dimension_type: Some(ResourceId::parse("example:custom_type").unwrap()),
                inline_dimension_type: None,
                min_y: 0,
                height: 16,
                has_skylight: has_sky,
                ambient_light: Some(0.0),
            };
            let mut invalid = lighting.clone();
            invalid.height = 32;
            assert!(
                batch
                    .enable_decoration(&mut data, &BTreeSet::new(), registry.clone(), &invalid)
                    .is_err()
            );
            batch
                .enable_decoration(&mut data, &BTreeSet::new(), registry.clone(), &lighting)
                .unwrap();
            batch.request_features(center).unwrap();
            let mut classify = |v: &Value| -> Result<CarverBlockState> {
                let name = v
                    .as_str()
                    .or_else(|| v.get("id").and_then(Value::as_str))
                    .context("state ID")?;
                Ok(CarverBlockState {
                    is_air: name == "minecraft:air",
                    has_fluid: matches!(name, "minecraft:water" | "minecraft:lava"),
                })
            };
            let mut resolve = |v: &Value| -> Result<BlockState> {
                Ok(BlockState {
                    name: v
                        .as_str()
                        .or_else(|| v.get("id").and_then(Value::as_str))
                        .context("state ID")?
                        .into(),
                    properties: BTreeMap::new(),
                })
            };
            let mut before_feature = None;
            loop {
                if batch
                    .schedule()
                    .next_ready()
                    .is_some_and(|(_, stage)| stage == schedule::GenerationStage::Features)
                {
                    before_feature = batch.snapshot();
                }
                match batch.step(&mut data, &mut classify, &mut resolve) {
                    Ok(Some(_)) => {}
                    Ok(None) if !unsupported => break,
                    Err(_) if unsupported => break,
                    other => panic!("unexpected batch result {other:?}"),
                }
            }
            let before = before_feature.unwrap();
            assert_eq!(
                before.dimension_type.as_deref(),
                Some("example:custom_type")
            );
            assert_eq!(before.chunks.len(), 9);
            assert_eq!(before.block(8, 8, 8).unwrap().name, "minecraft:dirt");
            if unsupported {
                assert!(Arc::ptr_eq(&before, &batch.snapshot().unwrap()));
                assert_eq!(
                    batch.schedule().completed(center),
                    Some(schedule::GenerationStage::Terrain)
                );
                assert!(!batch.is_complete());
                continue;
            }
            assert!(batch.is_complete());
            assert_eq!(
                batch.schedule().completed(center),
                Some(schedule::GenerationStage::Features)
            );
            let maps = batch
                .spawn_heightmaps(center)
                .unwrap()
                .expect("primed generation heightmaps");
            use native_persistence::heightmap::SpawnHeightmapKind;
            for z in 0..16 {
                for x in 0..16 {
                    assert_eq!(
                        maps.highest(SpawnHeightmapKind::WorldSurface, x, z),
                        Some(15)
                    );
                    assert_eq!(maps.highest(SpawnHeightmapKind::OceanFloor, x, z), Some(15));
                    assert_eq!(
                        maps.highest(SpawnHeightmapKind::MotionBlocking, x, z),
                        Some(15)
                    );
                }
            }
            let after = batch.snapshot().unwrap();
            assert_eq!(after.block(8, 8, 8).unwrap().name, "minecraft:melon");
            assert_eq!(
                after.block(9, 8, 8).unwrap().name,
                if has_sky {
                    "minecraft:dirt"
                } else {
                    "minecraft:red_mushroom"
                }
            );
            assert_eq!(before.block(8, 8, 8).unwrap().name, "minecraft:dirt");
        }
    }
    #[test]
    fn fresh_terrain_batch_runs_dependencies_and_stops_after_failure() {
        use native_resources::carver::tags::CarverBlockTags;
        use native_world::loaded::{BlockState, ChunkPos};
        let mut data = Data;
        let registry: BTreeSet<_> = ["air", "bedrock", "stone", "dirt", "water", "lava"]
            .into_iter()
            .map(|s| format!("minecraft:{s}"))
            .collect();
        let make_batch = |data: &mut Data| {
            let tags = CarverBlockTags::load(data, &registry).unwrap();
            let generator =
                TerrainGenerator::without_terrain_adjustments(data, &definition(), 42, 0, 16)
                    .unwrap();
            FreshTerrainBatch::new_without_structures(generator, tags, "minecraft:overworld")
                .unwrap()
        };
        let mut classify = |v: &Value| -> Result<CarverBlockState> {
            let name = v.as_str().context("expected fixture state")?;
            Ok(CarverBlockState {
                is_air: name == "minecraft:air",
                has_fluid: matches!(name, "minecraft:water" | "minecraft:lava"),
            })
        };
        let mut resolve = |v: &Value| -> Result<BlockState> {
            Ok(BlockState {
                name: v.as_str().context("expected fixture state")?.into(),
                properties: BTreeMap::new(),
            })
        };
        let center = ChunkPos { x: 0, z: 0 };
        let east = ChunkPos { x: 1, z: 0 };
        let mut batch = make_batch(&mut data);
        assert!(batch.is_complete());
        assert!(batch.request(ChunkPos { x: i32::MAX, z: 0 }).is_err());
        assert!(batch.is_complete());
        batch.request(center).unwrap();
        batch.request(east).unwrap();
        let mut first_snapshot = None;
        let mut terrain_steps = Vec::new();
        while let Some((pos, stage)) = batch.step(&mut data, &mut classify, &mut resolve).unwrap() {
            if stage == schedule::GenerationStage::Terrain {
                terrain_steps.push(pos);
                if first_snapshot.is_none() {
                    first_snapshot = batch.snapshot();
                }
            }
        }
        assert!(batch.is_complete());
        assert_eq!(terrain_steps.len(), 2);
        let first = first_snapshot.unwrap();
        assert_eq!(first.chunks.len(), 1);
        let (schedule, biomes, world, light) = batch.into_parts().unwrap();
        assert!(light.is_none());
        assert_eq!(
            schedule.completed(center),
            Some(schedule::GenerationStage::Terrain)
        );
        assert_eq!(
            schedule.completed(east),
            Some(schedule::GenerationStage::Terrain)
        );
        assert!(biomes.chunk([0, 0]).is_some() && biomes.chunk([1, 0]).is_some());
        let snapshot = world.unwrap().snapshot();
        assert_eq!(snapshot.chunks.len(), 2);
        assert_eq!(snapshot.block(0, 0, 0).unwrap().name, "minecraft:dirt");
        assert_eq!(snapshot.block(16, 0, 0).unwrap().name, "minecraft:dirt");
        assert_eq!(first.chunks.len(), 1);
        let mut failing = make_batch(&mut data);
        failing.request(center).unwrap();
        let mut reject = |_: &Value| -> Result<BlockState> { anyhow::bail!("unsupported block") };
        loop {
            match failing.step(&mut data, &mut classify, &mut reject) {
                Ok(Some(_)) => {}
                Err(error) => {
                    assert!(error.to_string().contains("Terrain"));
                    break;
                }
                Ok(None) => panic!("terrain should have failed"),
            }
        }
        assert_eq!(
            failing.schedule().completed(center),
            Some(schedule::GenerationStage::Biomes)
        );
        assert!(failing.snapshot().is_none());
        assert!(!failing.is_complete());
        assert!(
            failing
                .step(&mut data, &mut classify, &mut resolve)
                .is_err()
        );
        assert!(failing.request(east).is_err());
        assert!(failing.into_parts().is_err());
    }
    #[test]
    fn schedule_runs_biomes_and_carved_terrain_transactionally() {
        use native_resources::carver::tags::CarverBlockTags;
        use native_world::loaded::{BlockState, ChunkPos};
        use schedule::{GenerationSchedule, GenerationStage};
        let mut data = Data;
        let mut generator =
            TerrainGenerator::without_terrain_adjustments(&mut data, &definition(), 42, 0, 16)
                .unwrap();
        let mut evaluator = DensityEvaluator::default();
        let mut neighborhood = generator.biome_neighborhood().unwrap();
        let mut plan = GenerationSchedule::new();
        let center = ChunkPos { x: 0, z: 0 };
        plan.request(center, GenerationStage::Terrain).unwrap();
        assert!(
            plan.run_biomes(&mut generator, &mut evaluator, &mut neighborhood, center)
                .is_err()
        );
        while !plan.ready(center, GenerationStage::Terrain) {
            let (pos, stage) = plan.next_ready().unwrap();
            match stage {
                GenerationStage::Biomes => plan
                    .run_biomes(&mut generator, &mut evaluator, &mut neighborhood, pos)
                    .unwrap(),
                // This test has no structure generation enabled.
                GenerationStage::StructureStarts | GenerationStage::StructureReferences => {
                    plan.complete(pos, stage).unwrap()
                }
                _ => panic!("unexpected stage {stage:?}"),
            }
        }
        assert!(neighborhood.chunk([0, 0]).is_some());
        assert!(
            plan.run_biomes(&mut generator, &mut evaluator, &mut neighborhood, center)
                .is_err()
        );
        let registry: BTreeSet<_> = ["air", "bedrock", "stone", "dirt", "water", "lava"]
            .into_iter()
            .map(|s| format!("minecraft:{s}"))
            .collect();
        let tags = CarverBlockTags::load(&mut data, &registry).unwrap();
        let mut cache = CarverCache::default();
        let mut assembled = None;
        let classify = |v: &Value| -> Result<CarverBlockState> {
            let name = v.as_str().context("expected simple fixture state")?;
            Ok(CarverBlockState {
                is_air: name == "minecraft:air",
                has_fluid: matches!(name, "minecraft:water" | "minecraft:lava"),
            })
        };
        let resolve = |v: &Value| -> Result<BlockState> {
            Ok(BlockState {
                name: v.as_str().context("expected simple fixture state")?.into(),
                properties: BTreeMap::new(),
            })
        };
        assert!(
            plan.run_terrain(
                &mut generator,
                &mut data,
                &mut evaluator,
                &mut cache,
                &neighborhood,
                &tags,
                &mut assembled,
                center,
                "minecraft:overworld",
                true,
                classify,
                resolve
            )
            .is_err()
        );
        assert_eq!(plan.completed(center), Some(GenerationStage::Biomes));
        assert!(
            plan.run_terrain(
                &mut generator,
                &mut data,
                &mut evaluator,
                &mut cache,
                &neighborhood,
                &tags,
                &mut assembled,
                center,
                "minecraft:overworld",
                false,
                classify,
                |_| anyhow::bail!("unsupported fixture state")
            )
            .is_err()
        );
        assert_eq!(plan.completed(center), Some(GenerationStage::Biomes));
        assert!(assembled.is_none());
        // Retry with a new generator and caches after a failed generation.
        let mut generator =
            TerrainGenerator::without_terrain_adjustments(&mut data, &definition(), 42, 0, 16)
                .unwrap();
        let mut evaluator = DensityEvaluator::default();
        let mut cache = CarverCache::default();
        plan.run_terrain(
            &mut generator,
            &mut data,
            &mut evaluator,
            &mut cache,
            &neighborhood,
            &tags,
            &mut assembled,
            center,
            "minecraft:overworld",
            false,
            classify,
            resolve,
        )
        .unwrap();
        let terrain = assembled.as_ref().unwrap().snapshot();
        assert_eq!(plan.completed(center), Some(GenerationStage::Terrain));
        assert_eq!(
            terrain.chunks[&center].status.as_deref(),
            Some("minecraft:terrain")
        );
        for y in 0..16 {
            for z in 0..16 {
                for x in 0..16 {
                    assert_eq!(terrain.block(x, y, z).unwrap().name, "minecraft:dirt");
                }
            }
        }
        let east = ChunkPos { x: 1, z: 0 };
        plan.request(east, GenerationStage::Terrain).unwrap();
        while !plan.ready(east, GenerationStage::Terrain) {
            let (pos, stage) = plan.next_ready().unwrap();
            match stage {
                GenerationStage::Biomes => plan
                    .run_biomes(&mut generator, &mut evaluator, &mut neighborhood, pos)
                    .unwrap(),
                GenerationStage::StructureStarts | GenerationStage::StructureReferences => {
                    plan.complete(pos, stage).unwrap()
                }
                _ => panic!("unexpected stage {stage:?}"),
            }
        }
        assert!(
            plan.run_terrain(
                &mut generator,
                &mut data,
                &mut evaluator,
                &mut cache,
                &neighborhood,
                &tags,
                &mut assembled,
                east,
                "minecraft:the_nether",
                false,
                classify,
                resolve
            )
            .is_err()
        );
        assert_eq!(plan.completed(east), Some(GenerationStage::Biomes));
        assert!(Arc::ptr_eq(
            &terrain,
            &assembled.as_ref().unwrap().snapshot()
        ));
        let mut generator =
            TerrainGenerator::without_terrain_adjustments(&mut data, &definition(), 42, 0, 16)
                .unwrap();
        let mut evaluator = DensityEvaluator::default();
        let mut cache = CarverCache::default();
        plan.run_terrain(
            &mut generator,
            &mut data,
            &mut evaluator,
            &mut cache,
            &neighborhood,
            &tags,
            &mut assembled,
            east,
            "minecraft:overworld",
            false,
            classify,
            resolve,
        )
        .unwrap();
        assert_eq!(plan.completed(east), Some(GenerationStage::Terrain));
        assert!(!terrain.chunks.contains_key(&east));
        assert!(
            assembled
                .as_ref()
                .unwrap()
                .snapshot()
                .chunks
                .contains_key(&east)
        );
    }
    #[test]
    fn generator_compiles_feature_schedule_from_its_biome_source() {
        use native_resources::feature_order::FeatureKey;
        struct BiomeData;
        impl ResourceProvider for BiomeData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                Ok((path == "data/minecraft/worldgen/biome/plains.json").then(|| br#"{"features":[["minecraft:ore_iron","minecraft:ore_gold"],[],["minecraft:ore_diamond"]]}"#.to_vec()))
            }
        }
        let generator =
            TerrainGenerator::without_terrain_adjustments(&mut Data, &definition(), 42, 0, 16)
                .unwrap();
        let ids: Vec<_> = ["ore_iron", "ore_gold", "ore_diamond"]
            .into_iter()
            .map(|s| ResourceId::parse(s).unwrap())
            .collect();
        let registry = ids.iter().cloned().collect();
        let schedule = generator
            .feature_schedule(&mut BiomeData, &registry)
            .unwrap();
        assert_eq!(
            schedule.steps(),
            vec![
                vec![
                    FeatureKey::Registered(ids[0].clone()),
                    FeatureKey::Registered(ids[1].clone())
                ],
                vec![],
                vec![FeatureKey::Registered(ids[2].clone())]
            ]
        );
    }

    struct PlanningData;
    impl ResourceProvider for PlanningData {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            let features = match path {
                "data/minecraft/worldgen/biome/plains.json" => Some(json!([
                    ["test:f0", "test:f2"],
                    ["test:f3"],
                    [],
                    ["test:f6"]
                ])),
                "data/minecraft/worldgen/biome/desert.json" => {
                    Some(json!([["test:f1", "test:f2"], [], ["test:f4"]]))
                }
                "data/minecraft/worldgen/biome/forest.json" => {
                    Some(json!([["test:f1", "test:f2"], ["test:f5"]]))
                }
                _ => None,
            };
            match features {
                Some(features) => Ok(Some(serde_json::to_vec(&json!({"features":features}))?)),
                None => Data.read_data(path),
            }
        }
        fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
            ensure!(folder == "worldgen/biome", "unexpected registry");
            ["plains", "desert", "forest"]
                .into_iter()
                .map(ResourceId::parse)
                .collect()
        }
    }
    fn planning_definition() -> Value {
        let mut value = definition();
        let parameters = json!({"temperature":0,"humidity":0,"continentalness":0,"erosion":0,"depth":0,"weirdness":0,"offset":0});
        value["biome_source"] = json!({"type":"minecraft:multi_noise","biomes":[
            {"biome":"minecraft:plains","parameters":parameters},
            {"biome":"minecraft:desert","parameters":parameters},
            {"biome":"minecraft:forest","parameters":parameters}
        ]});
        value
    }
    fn planning_neighborhood(
        generator: &TerrainGenerator,
        target: [i32; 2],
        mask: u32,
    ) -> BiomeNeighborhood {
        let mut result = generator.biome_neighborhood().unwrap();
        for dz in -1..=1 {
            for dx in -1..=1 {
                let chunk = [target[0] + dx, target[1] + dz];
                let mut values = vec![ResourceId::parse("example:other").unwrap(); 128];
                let candidate = match (dx, dz) {
                    (-1, -1) => Some((1, "plains")),
                    (1, 1) => Some((2, "desert")),
                    (-1, 1) => Some((4, "forest")),
                    _ => None,
                };
                if let Some((bit, biome)) = candidate {
                    if mask & bit != 0 {
                        values[127] = ResourceId::parse(biome).unwrap();
                    }
                }
                result
                    .insert(Arc::new(
                        QuartBiomeChunk::from_values(chunk, generator.seed, -16, 32, values)
                            .unwrap(),
                    ))
                    .unwrap();
            }
        }
        result
    }

    #[test]
    fn decoration_actions_keep_structure_and_feature_step_order() {
        use DecorationAction::{Feature, Structure};
        assert_eq!(
            ordered_decoration_actions(
                &[(0, 2), (3, 0), (3, 5), (7, 1)],
                &[(0, 0), (2, 3), (3, 0), (3, 4), (11, 0)]
            )
            .unwrap(),
            vec![
                Structure(0),
                Feature(0),
                Feature(1),
                Structure(1),
                Structure(2),
                Feature(2),
                Feature(3),
                Structure(3),
                Feature(4)
            ]
        );
        assert_eq!(ordered_decoration_actions(&[], &[]).unwrap(), vec![]);
        assert!(ordered_decoration_actions(&[(2, 1), (2, 1)], &[]).is_err());
        assert!(ordered_decoration_actions(&[(3, 0), (2, 1)], &[]).is_err());
        assert!(ordered_decoration_actions(&[], &[(0, -1)]).is_err());
    }

    #[test]
    fn decoration_plan_checks_neighborhood_and_source() {
        let generator = TerrainGenerator::without_terrain_adjustments(
            &mut PlanningData,
            &planning_definition(),
            42,
            -16,
            32,
        )
        .unwrap();
        let registry = (0..7)
            .map(|i| ResourceId::parse(&format!("test:f{i}")).unwrap())
            .collect();
        let schedule = generator
            .feature_schedule(&mut PlanningData, &registry)
            .unwrap();
        let mut neighborhood = planning_neighborhood(&generator, [-2, 3], 3);
        let plan = generator
            .decoration_plan(&schedule, &neighborhood, [-2, 3])
            .unwrap();
        assert_eq!(plan.origin, [-32, -16, 48]);
        let entry = native_resources::structure_decoration::StructureDecorationEntry {
            id: ResourceId::parse("test:structure").unwrap(),
            step: 0,
            index: 5,
        };
        let start = SimpleStructureStart {
            kind: native_resources::structure_start::SimpleStructureKind::BuriedTreasure,
            chunk: [-2, 3],
            generation_point: [-24, 0, 56],
            orientation: None,
            bounds: native_core::beardifier::BoundingBox::new([-23, 0, 57], [-23, 0, 57]).unwrap(),
            terrain_adjustment: native_core::beardifier::TerrainAdjustment::None,
        };
        let structure = structure_batch::PlannedStructureDecoration {
            entry: &entry,
            starts: vec![&start],
        };
        let actions = plan.actions_with_structures(&[structure]).unwrap();
        let mixed = [structure_batch::PlannedMixedStructureDecoration {
            entry: &entry,
            starts: vec![structure_batch::PlannedStructureStart::Simple(&start)],
        }];
        assert_eq!(plan.actions_with_mixed_structures(&mixed).unwrap(), actions);
        assert_eq!(actions.first(), Some(&DecorationAction::Structure(0)));
        assert_eq!(
            actions
                .iter()
                .filter(|action| matches!(action, DecorationAction::Feature(_)))
                .count(),
            plan.features.len()
        );
        let names: Vec<_> = plan.features.iter().map(|f| f.feature).collect();
        assert!(
            !names.contains(&&native_resources::feature_order::FeatureKey::Registered(
                ResourceId::parse("test:f5").unwrap()
            ))
        );
        assert!(
            names.contains(&&native_resources::feature_order::FeatureKey::Registered(
                ResourceId::parse("test:f4").unwrap()
            ))
        );
        neighborhood.chunks.remove(&[-3, 2]);
        assert!(
            generator
                .decoration_plan(&schedule, &neighborhood, [-2, 3])
                .is_err()
        );
        let wrong = FeatureSchedule::load(
            &mut PlanningData,
            &[
                ResourceId::parse("desert").unwrap(),
                ResourceId::parse("plains").unwrap(),
                ResourceId::parse("forest").unwrap(),
            ],
            &registry,
        )
        .unwrap();
        assert!(
            generator
                .decoration_plan(
                    &wrong,
                    &planning_neighborhood(&generator, [-2, 3], 3),
                    [-2, 3]
                )
                .is_err()
        );
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
    fn decoration_selection_and_randomness_match_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        assert_eq!(manifest["id"], "26.3");
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for lib in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("DecorationPlanOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/DecorationPlanOracle.java"),
        )
        .unwrap();
        let run = |cmd: &mut Command| {
            let out = cmd.output().unwrap();
            assert!(
                out.status.success(),
                "Java decoration oracle failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8(out.stdout).unwrap()
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let registry = (0..7)
            .map(|i| ResourceId::parse(&format!("test:f{i}")).unwrap())
            .collect();
        let mut cases = Vec::new();
        let mut rust = Vec::new();
        for seed in [0i64, 1, -1, i64::MIN] {
            for mask in 0..8u32 {
                let target = [mask as i32 - 4, 3 - mask as i32];
                let mut present = vec![3, 3];
                for bit in 0..3 {
                    if mask & (1 << bit) != 0 {
                        present.push(bit);
                    }
                }
                cases.push(json!({"seed":seed,"x":target[0],"z":target[1],"present":present}));
                let generator = TerrainGenerator::without_terrain_adjustments(
                    &mut PlanningData,
                    &planning_definition(),
                    seed,
                    -16,
                    32,
                )
                .unwrap();
                let schedule = generator
                    .feature_schedule(&mut PlanningData, &registry)
                    .unwrap();
                let neighborhood = planning_neighborhood(&generator, target, mask);
                let plan = generator
                    .decoration_plan(&schedule, &neighborhood, target)
                    .unwrap();
                assert_eq!(plan.origin, [target[0] * 16, -16, target[1] * 16]);
                let mut features = Vec::new();
                for feature in &plan.features {
                    let native_resources::feature_order::FeatureKey::Registered(id) =
                        feature.feature
                    else {
                        panic!("unexpected inline feature")
                    };
                    let id: i32 = id.path.strip_prefix('f').unwrap().parse().unwrap();
                    let mut random = plan.feature_random(feature);
                    features.push(json!([feature.step, feature.index, id, random.next_long()]));
                }
                rust.push(json!({"deco":plan.decoration_seed,"features":features}));
            }
        }
        let input = temp.path().join("decoration.json");
        std::fs::write(
            &input,
            serde_json::to_vec(
                &json!({"biomes":[[[0,2],[3],[],[6]],[[1,2],[],[4]],[[1,2],[5]]],"cases":cases}),
            )
            .unwrap(),
        )
        .unwrap();
        let output = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("DecorationPlanOracle")
            .arg(input));
        let expected: Vec<Value> = serde_json::from_str(output.lines().last().unwrap()).unwrap();
        assert_eq!(rust, expected);
        assert_eq!(rust.len(), 32);
    }
    #[test]
    fn noise_biome_lookup_does_not_apply_seeded_block_zoom() {
        let mut neighbors = BiomeNeighborhood::new(42, 0, 16).unwrap();
        for z in -1..=1 {
            for x in -1..=1 {
                let id = ResourceId::parse(&format!("example:biome_{}_{}", x + 1, z + 1)).unwrap();
                neighbors
                    .insert(Arc::new(
                        QuartBiomeChunk::from_values([x, z], 42, 0, 16, vec![id; 64]).unwrap(),
                    ))
                    .unwrap();
            }
        }
        let mut different = false;
        for z in -8_i32..=8 {
            for x in -8_i32..=8 {
                let noise = neighbors.noise_biome_at_block(x, 1000, z).unwrap();
                assert_eq!(
                    noise.path,
                    format!("biome_{}_{}", x.div_euclid(16) + 1, z.div_euclid(16) + 1)
                );
                if noise != neighbors.biome_at_block(x, 15, z).unwrap() {
                    different = true;
                }
            }
        }
        assert!(different, "fixture must cross a zoomed biome boundary");
    }

    #[test]
    fn generates_biomes_and_surface_only_with_complete_matching_neighbors() {
        let mut data = Data;
        assert!(
            TerrainGenerator::without_terrain_adjustments(&mut data, &definition(), 42, -16, 16)
                .is_err()
        );
        let mut generator =
            TerrainGenerator::without_terrain_adjustments(&mut data, &definition(), 42, 0, 16)
                .unwrap();
        let mut evaluator = DensityEvaluator::default();
        let mut neighbors = generator.biome_neighborhood().unwrap();
        let classify = |v: &Value| -> Result<SurfaceBlockKind> {
            Ok(match v.as_str().unwrap() {
                "minecraft:air" => SurfaceBlockKind::Air,
                "minecraft:water" | "minecraft:lava" => SurfaceBlockKind::Fluid,
                _ => SurfaceBlockKind::Solid,
            })
        };
        assert!(
            generator
                .generate_surface(&mut data, &mut evaluator, [0, 0], &neighbors, classify)
                .is_err()
        );
        for z in -1..=1 {
            for x in -1..=1 {
                let generated = generator.generate_biomes([x, z], &mut evaluator).unwrap();
                let biome = Arc::new(if [x, z] == [0, 0] {
                    QuartBiomeChunk::from_values([x, z], 42, 0, 16, generated.values().to_vec())
                        .unwrap()
                } else {
                    generated
                });
                assert_eq!(biome.values().len(), 64);
                assert!(biome.values().iter().all(|id| id.path == "plains"));
                neighbors.insert(biome).unwrap();
            }
        }
        assert_eq!(
            neighbors.biome_at_block(0, -1000, 0).unwrap().path,
            "plains"
        );
        assert_eq!(
            neighbors.biome_at_block(15, 1000, 15).unwrap().path,
            "plains"
        );
        let result = generator
            .generate_surface(&mut data, &mut evaluator, [0, 0], &neighbors, classify)
            .unwrap();
        assert_eq!(result.states.len(), 4096);
        assert!(
            result
                .states
                .iter()
                .all(|v| v.as_str() == Some("minecraft:dirt"))
        );
        let mut wrong = BiomeNeighborhood::new(99, 0, 16).unwrap();
        let biome = Arc::clone(neighbors.chunk([0, 0]).unwrap());
        assert!(wrong.insert(biome).is_err());
        assert!(QuartBiomeChunk::from_values([0, 0], 42, 0, 16, Vec::new()).is_err());
    }

    #[test]
    fn structure_context_reaches_noise_and_surface_rules_and_is_restored() {
        use native_core::beardifier::{BoundingBox, RigidPiece, TerrainAdjustment};
        let mut data = Data;
        let mut definition = definition();
        definition["settings"]["noise_router"]["final_density"] = json!({
            "type":"add","left":-0.25,"right":{"type":"beardifier"}
        });
        definition["settings"]["material_rule"] = json!({
            "type":"ore_vein", "density":{"type":"beardifier"}, "richness":1,
            "filler_gap":-1, "raw_ore_chance":1, "ore_block":"minecraft:dirt",
            "raw_ore_block":"minecraft:dirt", "filler_block":"minecraft:dirt"
        });
        let mut generator =
            TerrainGenerator::with_structure_context(&mut data, &definition, 42, 0, 16).unwrap();
        let without =
            TerrainGenerator::without_terrain_adjustments(&mut data, &definition, 42, 0, 16)
                .unwrap();
        let mut evaluator = DensityEvaluator::default();
        let mut neighbors = generator.biome_neighborhood().unwrap();
        for z in -1..=1 {
            for x in -1..=1 {
                neighbors
                    .insert(Arc::new(
                        generator.generate_biomes([x, z], &mut evaluator).unwrap(),
                    ))
                    .unwrap();
            }
        }
        let bounds = BoundingBox::new([0, 5, 0], [2, 7, 2]).unwrap();
        let structure = Arc::new(Beardifier::new(
            vec![RigidPiece {
                bounds,
                adjustment: TerrainAdjustment::Bury,
                ground_delta: 0,
            }],
            vec![],
            Some(BoundingBox::new([-24, -19, -24], [26, 31, 26]).unwrap()),
        ));
        let classify = |v: &Value| -> Result<SurfaceBlockKind> {
            Ok(match v.as_str().unwrap() {
                "minecraft:air" => SurfaceBlockKind::Air,
                "minecraft:water" | "minecraft:lava" => SurfaceBlockKind::Fluid,
                _ => SurfaceBlockKind::Solid,
            })
        };
        assert!(
            without
                .generate_surface_with_structure_context(
                    &mut data,
                    &mut evaluator,
                    [0, 0],
                    &neighbors,
                    Arc::clone(&structure),
                    classify
                )
                .is_err()
        );
        let index = (16 + 1) * 16 + 5;
        let plain = generator
            .generate_surface(&mut data, &mut evaluator, [0, 0], &neighbors, classify)
            .unwrap();
        assert_eq!(plain.states[index].as_str(), Some("minecraft:water"));
        let adjusted = generator
            .generate_surface_with_structure_context(
                &mut data,
                &mut evaluator,
                [0, 0],
                &neighbors,
                Arc::clone(&structure),
                classify,
            )
            .unwrap();
        assert_eq!(adjusted.states[index].as_str(), Some("minecraft:dirt"));
        assert!(evaluator.structure_terrain_context().is_none());
        let registry = ["air", "stone", "water", "lava", "dirt"]
            .into_iter()
            .map(|s| format!("minecraft:{s}"))
            .collect();
        let tags =
            native_resources::carver::tags::CarverBlockTags::load(&mut data, &registry).unwrap();
        let mut cache = CarverCache::default();
        let carved = generator
            .generate_carved_surface_with_structure_context(
                &mut data,
                &mut evaluator,
                &mut cache,
                [0, 0],
                &neighbors,
                &tags,
                Arc::clone(&structure),
                |v| {
                    Ok(CarverBlockState {
                        is_air: v.as_str() == Some("minecraft:air"),
                        has_fluid: matches!(v.as_str(), Some("minecraft:water" | "minecraft:lava")),
                    })
                },
            )
            .unwrap();
        assert_eq!(carved.states[index].as_str(), Some("minecraft:dirt"));
        assert!(evaluator.structure_terrain_context().is_none());
        let previous = Arc::new(Beardifier::new(vec![], vec![], None));
        evaluator.replace_structure_terrain_context(Some(Arc::clone(&previous)));
        assert!(
            generator
                .generate_surface_with_structure_context(
                    &mut data,
                    &mut evaluator,
                    [i32::MAX, 0],
                    &neighbors,
                    structure,
                    classify
                )
                .is_err()
        );
        assert!(Arc::ptr_eq(
            &previous,
            evaluator.structure_terrain_context().unwrap()
        ));
    }

    #[test]
    fn carving_surface_repair_uses_structure_context() {
        use native_core::beardifier::{BoundingBox, RigidPiece, TerrainAdjustment};
        let mut data = Data;
        let mut config = definition();
        config["settings"]["material_rule"] = json!({
            "type":"ore_vein", "density":{"type":"beardifier"}, "richness":1,
            "filler_gap":-1, "raw_ore_chance":1, "ore_block":"minecraft:stone",
            "raw_ore_block":"minecraft:stone", "filler_block":"minecraft:stone"
        });
        let mut generator =
            TerrainGenerator::with_structure_context(&mut data, &config, 42, 0, 16).unwrap();
        let registry = ["air", "stone", "water", "lava", "dirt", "grass_block"]
            .into_iter()
            .map(|s| format!("minecraft:{s}"))
            .collect();
        let tags =
            native_resources::carver::tags::CarverBlockTags::load(&mut data, &registry).unwrap();
        let bounds = BoundingBox::new([0, 5, 0], [2, 7, 2]).unwrap();
        let structure = Arc::new(Beardifier::new(
            vec![RigidPiece {
                bounds,
                adjustment: TerrainAdjustment::Bury,
                ground_delta: 0,
            }],
            vec![],
            Some(BoundingBox::new([-24, -19, -24], [26, 31, 26]).unwrap()),
        ));
        let mut evaluator = DensityEvaluator::default();
        let mut mask = CarvingMask::new(1, 8).unwrap();
        native_resources::carver::carve_ellipsoid(
            &mut mask,
            [0, 0],
            [1.5, 6.0, 1.5],
            0.5,
            1.0,
            |_, _, _, y| y != 6,
        )
        .unwrap();
        let index = (16 + 1) * 16;
        let make_chunk = || {
            let mut chunk = SurfaceStageChunk {
                chunk_position: [0, 0],
                min_y: 0,
                height: 16,
                states: vec![Arc::new(json!("minecraft:stone")); 4096],
                fluid_updates: vec![false; 4096],
                noise_postprocessing: None,
                surface_postprocessing: None,
                carver_postprocessing: None,
            };
            chunk.states[index + 5] = Arc::new(json!("minecraft:dirt"));
            chunk.states[index + 6] = Arc::new(json!("minecraft:grass_block"));
            chunk
        };
        let classify = |v: &Value| -> Result<CarverBlockState> {
            Ok(CarverBlockState {
                is_air: v.as_str() == Some("minecraft:air"),
                has_fluid: matches!(v.as_str(), Some("minecraft:water" | "minecraft:lava")),
            })
        };
        let mut plain = make_chunk();
        generator
            .apply_carving_to_surface(
                &mut data,
                &mut evaluator,
                &mut plain,
                &mask,
                &tags,
                classify,
            )
            .unwrap();
        assert_eq!(plain.states[index + 5].as_str(), Some("minecraft:dirt"));
        evaluator.replace_structure_terrain_context(Some(structure));
        let mut adjusted = make_chunk();
        generator
            .apply_carving_to_surface(
                &mut data,
                &mut evaluator,
                &mut adjusted,
                &mask,
                &tags,
                classify,
            )
            .unwrap();
        assert_eq!(adjusted.states[index + 5].as_str(), Some("minecraft:stone"));
        assert_eq!(adjusted.states[index + 6].as_str(), Some("minecraft:water"));
    }

    #[test]
    fn end_chunk_respects_y_dependent_datapack_erosion() {
        let mut data = Data;
        let mut config = definition();
        config["biome_source"] = json!({"type":"minecraft:the_end"});
        config["settings"]["noise_router"]["erosion"] = json!({"type":"gradient","axis":"y",
            "from_coordinate":0,"to_coordinate":16,"from_value":-0.5,"to_value":0.7});
        let mut generator =
            TerrainGenerator::without_terrain_adjustments(&mut data, &config, 42, 0, 16).unwrap();
        let chunk = generator
            .generate_biomes([65, 0], &mut DensityEvaluator::default())
            .unwrap();
        let names: Vec<_> = chunk.values()[0..4]
            .iter()
            .map(|id| id.path.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "small_end_islands",
                "end_barrens",
                "end_midlands",
                "end_highlands"
            ]
        );
        for column in chunk.values().chunks_exact(4) {
            assert_eq!(
                column.iter().map(|id| id.path.as_str()).collect::<Vec<_>>(),
                names
            );
        }
        let center = generator
            .generate_biomes([0, 0], &mut DensityEvaluator::default())
            .unwrap();
        assert!(center.values().iter().all(|id| id.path == "the_end"));
    }

    #[test]
    fn biome_generation_ignores_chunk_structure_context() {
        use native_core::beardifier::{BoundingBox, RigidPiece, TerrainAdjustment};
        let mut data = Data;
        let mut config = definition();
        config["biome_source"] = json!({"type":"minecraft:the_end"});
        config["settings"]["noise_router"]["erosion"] =
            json!({"type":"add", "left":-0.5, "right":{"type":"beardifier"}});
        let mut generator =
            TerrainGenerator::with_structure_context(&mut data, &config, 42, 0, 16).unwrap();
        let mut evaluator = DensityEvaluator::default();
        let baseline = generator.generate_biomes([65, 0], &mut evaluator).unwrap();
        let bounds = BoundingBox::new([1040, 0, 0], [1055, 16, 15]).unwrap();
        let structure = Arc::new(Beardifier::new(
            vec![RigidPiece {
                bounds,
                adjustment: TerrainAdjustment::Bury,
                ground_delta: 0,
            }],
            vec![],
            Some(BoundingBox::new([1016, -24, -24], [1079, 40, 39]).unwrap()),
        ));
        evaluator.replace_structure_terrain_context(Some(Arc::clone(&structure)));
        let adjusted = generator.generate_biomes([65, 0], &mut evaluator).unwrap();
        assert_eq!(baseline.values(), adjusted.values());
        assert!(Arc::ptr_eq(
            &structure,
            evaluator.structure_terrain_context().unwrap()
        ));
        assert!(
            generator
                .generate_biomes([i32::MAX, 0], &mut evaluator)
                .is_err()
        );
        assert!(Arc::ptr_eq(
            &structure,
            evaluator.structure_terrain_context().unwrap()
        ));
    }

    #[test]
    fn fresh_structure_batch_runs_hut_features_and_retains_mobs() {
        use native_resources::{
            carver::tags::CarverBlockTags, cat_variants::CatSpawnVariants,
            dimension_lighting::GeneratedDimensionLighting, structure_biomes::StructureBiomes,
            structure_piece::swamp_hut::HutMob, structure_placement::StructureSetCache,
        };
        use native_world::loaded::{BlockState, ChunkPos};
        struct HutData {
            predicate: &'static str,
        }
        impl ResourceProvider for HutData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                let value = match path {
                    "data/example/worldgen/structure/hut.json" => Some(
                        json!({"type":"minecraft:swamp_hut","biomes":["minecraft:plains"],"step":"surface_structures"}),
                    ),
                    "data/example/worldgen/structure_set/hut.json" => Some(
                        json!({"placement":{"type":"minecraft:random_spread","spacing":32,"separation":0,"salt":0},"structures":[{"structure":"example:hut","weight":1}]}),
                    ),
                    "data/minecraft/worldgen/biome/plains.json" => Some(
                        json!({"has_precipitation":true,"temperature":0.8,"carvers":[],"features":[]}),
                    ),
                    "data/minecraft/cat_variant/black.json" => Some(json!({})),
                    "data/example/cat_variant/structure_cat.json" => {
                        let condition = match self.predicate {
                            "structure" => {
                                json!({"type":"minecraft:structure","structures":"example:hut"})
                            }
                            "biome" => json!({"type":"biome","biomes":"minecraft:plains"}),
                            "moon" => {
                                json!({"type":"minecraft:moon_brightness","range":{"min":0.9}})
                            }
                            _ => unreachable!(),
                        };
                        Some(json!({"spawn_conditions":[{"condition":condition,"priority":0}]}))
                    }
                    "data/minecraft/tags/block/blocks_motion_in_heightmap.json"
                    | "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json" => {
                        Some(json!({"values":["minecraft:stone","minecraft:dirt"]}))
                    }
                    _ => None,
                };
                match value {
                    Some(value) => Ok(Some(serde_json::to_vec(&value)?)),
                    None => Data.read_data(path),
                }
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                match folder {
                    "worldgen/structure" => Ok(vec![ResourceId::parse("example:hut")?]),
                    "cat_variant" => Ok(vec![
                        ResourceId::parse("minecraft:black")?,
                        ResourceId::parse("example:structure_cat")?,
                    ]),
                    "cat_sound_variant" => Ok(vec![ResourceId::parse("minecraft:classic")?]),
                    _ => Data.list_data(folder),
                }
            }
        }
        for (fail_mob, native_conditions, predicate, phase, fail_clock, expected_variant) in [
            (false, false, "structure", 0, false, "example:structure_cat"),
            (true, false, "structure", 0, false, "example:structure_cat"),
            (false, true, "structure", 0, false, "example:structure_cat"),
            (false, true, "biome", 0, false, "example:structure_cat"),
            (false, true, "moon", 0, false, "example:structure_cat"),
            (false, true, "moon", 4, false, "minecraft:black"),
            (false, true, "moon", 0, true, "minecraft:black"),
        ] {
            let mut data = HutData { predicate };
            let mut config = definition();
            config["settings"]["noise"]["height"] = json!(96);
            config["settings"]["material_rule"] =
                json!({"type":"block","result_state":"minecraft:stone"});
            config["settings"]["noise_router"]["final_density"] = json!({"type":"gradient","axis":"y","from_coordinate":0,"to_coordinate":96,"from_value":1,"to_value":-1});
            let (generator, terrain_generator) =
                TerrainGenerator::paired_with_structure_context(&mut data, &config, 42, 0, 96)
                    .unwrap();
            let structure = ResourceId::parse("example:hut").unwrap();
            let set = ResourceId::parse("example:hut").unwrap();
            let [x, z] = StructureSetCache::default()
                .load(&mut data, &set)
                .unwrap()
                .placement
                .potential_chunk(42, [0, 0]);
            let target = ChunkPos { x, z };
            let requested: Vec<_> = (-1..=1)
                .flat_map(|dz| {
                    (-1..=1).map(move |dx| ChunkPos {
                        x: x + dx,
                        z: z + dz,
                    })
                })
                .collect();
            let rules = StructureBiomes::load(&mut data, [structure.clone()]).unwrap();
            let mut structures =
                structure_batch::StructureGenerationBatch::new(generator, rules, vec![set])
                    .unwrap();
            let preparation = structure_batch::StructurePreparationPlan::new(
                requested
                    .iter()
                    .copied()
                    .map(|pos| (pos, schedule::GenerationStage::Features)),
            )
            .unwrap();
            preparation.request_structures(&mut structures).unwrap();
            while structures
                .step(&mut data, &mut |value| {
                    let name = value.as_str().unwrap();
                    Ok(BlockHeightFlags {
                        is_air: name == "minecraft:air",
                        blocks_motion: matches!(name, "minecraft:stone" | "minecraft:dirt"),
                        blocks_motion_no_leaves: matches!(
                            name,
                            "minecraft:stone" | "minecraft:dirt"
                        ),
                        has_fluid: matches!(name, "minecraft:water" | "minecraft:lava"),
                    })
                })
                .unwrap()
                .is_some()
            {}
            let blocks: BTreeSet<_> = [
                "air",
                "cave_air",
                "void_air",
                "bedrock",
                "stone",
                "dirt",
                "water",
                "lava",
                "spruce_planks",
                "oak_log",
                "oak_fence",
                "spruce_stairs",
                "potted_red_mushroom",
                "crafting_table",
                "cauldron",
            ]
            .into_iter()
            .map(|name| format!("minecraft:{name}"))
            .collect();
            let tags = CarverBlockTags::load(&mut data, &blocks).unwrap();
            let mut batch = preparation
                .into_expandable_terrain_batch(
                    structures,
                    terrain_generator,
                    tags,
                    "minecraft:overworld",
                )
                .unwrap();
            let before = batch.structure_starts(target).unwrap()[&structure].bounds;
            let lighting = GeneratedDimensionLighting {
                dimension: ResourceId::parse("minecraft:overworld").unwrap(),
                dimension_type: None,
                inline_dimension_type: None,
                min_y: 0,
                height: 96,
                has_skylight: true,
                ambient_light: Some(0.0),
            };
            batch
                .enable_decoration(&mut data, &BTreeSet::new(), blocks, &lighting)
                .unwrap();
            for &pos in &requested {
                batch.request_features(pos).unwrap();
            }
            let variants = CatSpawnVariants::load_active(&mut data).unwrap();
            let conditions = variants.prepare_world_conditions(&mut data).unwrap();
            let moon = native_resources::moon_phase::Environment {
                base: native_resources::moon_phase::Phase::FullMoon,
                biomes: BTreeMap::from([("minecraft:plains".into(), Some(Some(native_resources::moon_phase::Phase::ThirdQuarter)))]),
                has_biome_layer: true,
                tracks: vec![native_resources::moon_phase::Track::parse(&json!({"clock":"example:clock", "period_ticks":20}), &json!({"keyframes":[{"ticks":0,"value":"full_moon"},{"ticks":10,"value":"new_moon"}]})).unwrap()],
            };
            let clocks = if fail_clock {
                BTreeMap::new()
            } else {
                BTreeMap::from([(
                    ResourceId::parse("example:clock").unwrap(),
                    if phase == 4 { 10 } else { 0 },
                )])
            };
            let mut construct = |mob| {
                ensure!(!fail_mob, "injected mob initialization failure");
                let (id, seed) = match mob {
                    HutMob::Witch => ("minecraft:witch", 1),
                    HutMob::Cat => ("minecraft:cat", 2),
                };
                FreshCreatureRandom::from_seed(&ResourceId::parse(id)?, seed)
            };
            let mut condition =
                |key: &crate::worldgen::structure_batch::TerrainStartKey,
                 pos: [i32; 3],
                 _: &native_world::loaded::World,
                 value: &Value,
                 bounds: native_core::beardifier::BoundingBox,
                 _: &[crate::worldgen::structure_batch::StructurePieceRevision]|
                 -> Result<bool> {
                    assert_eq!(key, &(structure.clone(), target));
                    assert_eq!(value["type"], "minecraft:structure");
                    assert_ne!(bounds.min[1], before.min[1]);
                    assert!(
                        (0..3)
                            .all(|axis| (bounds.min[axis]..=bounds.max[axis]).contains(&pos[axis]))
                    );
                    Ok(true)
                };
            let mut saw_failure = false;
            loop {
                let previous = batch.snapshot();
                let mut classify = |value: &Value| {
                    let name = value.as_str().unwrap();
                    Ok(CarverBlockState {
                        is_air: name == "minecraft:air",
                        has_fluid: matches!(name, "minecraft:water" | "minecraft:lava"),
                    })
                };
                let mut resolve = |value: &Value| {
                    Ok(BlockState {
                        name: value.as_str().unwrap().into(),
                        properties: BTreeMap::new(),
                    })
                };
                let result = if native_conditions && predicate == "moon" {
                    batch.step_with_hut_clocks(
                        &mut data,
                        &mut classify,
                        &mut resolve,
                        &variants,
                        &conditions,
                        &moon,
                        &clocks,
                    )
                } else if native_conditions {
                    batch.step_with_native_hut_support(
                        &mut data,
                        &mut classify,
                        &mut resolve,
                        &variants,
                        &conditions,
                        phase,
                    )
                } else {
                    batch.step_with_hut_support(
                        &mut data,
                        &mut classify,
                        &mut resolve,
                        &variants,
                        &mut construct,
                        &mut condition,
                    )
                };
                match result {
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(error) => {
                        assert!(
                            fail_mob || fail_clock,
                            "unexpected generation failure: {error:#}"
                        );
                        assert!(format!("{error:#}").contains(if fail_mob {
                            "injected mob initialization failure"
                        } else {
                            "moon phase environment or world clock unavailable"
                        }));
                        assert!(Arc::ptr_eq(&previous.unwrap(), &batch.snapshot().unwrap()));
                        assert!(batch.structure_starts(target).is_none());
                        saw_failure = true;
                        break;
                    }
                }
            }
            if fail_mob || fail_clock {
                assert!(saw_failure);
                continue;
            }
            let start = &batch.structure_starts(target).unwrap()[&structure];
            assert_ne!(start.bounds.min[1], before.min[1]);
            let entity_count: usize = requested
                .iter()
                .map(|&pos| batch.recorded_generated_entities(pos).unwrap().len())
                .sum();
            assert_eq!(entity_count, 2);
            let cats: Vec<_> = requested.iter().flat_map(|&pos| batch.recorded_generated_entities(pos).unwrap().iter()).filter(|entity| matches!(entity.get("id"), Some(native_persistence::nbt::Tag::String(id)) if id.matches("minecraft:cat"))).collect();
            assert_eq!(cats.len(), 1);
            assert!(
                matches!(cats[0].get("variant"), Some(native_persistence::nbt::Tag::String(id)) if id.matches(expected_variant))
            );
            let world = batch.snapshot().unwrap();
            assert!((start.bounds.min[0]..=start.bounds.max[0]).any(|x| {
                (start.bounds.min[1]..=start.bounds.max[1]).any(|y| {
                    (start.bounds.min[2]..=start.bounds.max[2]).any(|z| {
                        world
                            .block(x, y, z)
                            .is_some_and(|block| block.name == "minecraft:spruce_planks")
                    })
                })
            }));
            let structure_data = batch.structure_data(target).unwrap();
            let children = match structure_data
                .get("starts")
                .unwrap()
                .get("example:hut")
                .unwrap()
                .get("Children")
                .unwrap()
            {
                native_persistence::nbt::Tag::List { values, .. } => values,
                _ => panic!("hut children are not a list"),
            };
            assert_eq!(
                children[0].get("Witch"),
                Some(&native_persistence::nbt::Tag::Byte(1))
            );
            assert_eq!(
                children[0].get("Cat"),
                Some(&native_persistence::nbt::Tag::Byte(1))
            );
            assert_eq!(
                children[0].get("BB"),
                Some(&native_persistence::nbt::Tag::IntArray(
                    start
                        .bounds
                        .min
                        .into_iter()
                        .chain(start.bounds.max)
                        .collect()
                ))
            );
        }
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn fresh_structure_batch_schedules_ocean_ruin_decoration() -> Result<()> {
        run_ocean_structure_batch(false, false)?;
        run_ocean_structure_batch(true, false)?;
        run_ocean_structure_batch(true, true)
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn fresh_batch_automatically_decorates_or_rolls_back_jigsaw() -> Result<()> {
        use flate2::{Compression, write::GzEncoder};
        use native_persistence::{
            heightmap::BlockHeightFlags,
            nbt::{self, NamedTag, Tag, TagType},
        };
        use native_resources::{
            carver::tags::CarverBlockTags, dimension_lighting::GeneratedDimensionLighting,
            structure_biomes::StructureBiomes, structure_placement::StructureSetCache,
        };
        use native_world::loaded::{BlockState, ChunkPos};
        use std::io::Write;

        struct FixtureData {
            vanilla: native_resources::packs::ResourceStack,
            template: Vec<u8>,
        }
        impl ResourceProvider for FixtureData {
            fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                self.vanilla.read_asset(path)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                let value = match path {
                    "data/example/worldgen/structure/test.json" => Some(
                        json!({"type":"minecraft:jigsaw","biomes":["minecraft:plains"],"step":"surface_structures","start_pool":"example:test","size":1,"start_height":{"absolute":8},"use_expansion_hack":false,"max_distance_from_center":80,"liquid_settings":"ignore_waterlogging"}),
                    ),
                    "data/example/worldgen/structure_set/test.json" => Some(
                        json!({"placement":{"type":"minecraft:random_spread","spacing":32,"separation":0,"salt":0},"structures":[{"structure":"example:test","weight":1}]}),
                    ),
                    "data/example/worldgen/template_pool/test.json" => Some(
                        json!({"fallback":"minecraft:empty","elements":[{"weight":1,"element":{"element_type":"minecraft:single_pool_element","location":"example:test","processors":"minecraft:empty","projection":"rigid"}}]}),
                    ),
                    "data/minecraft/worldgen/biome/plains.json" => Some(
                        json!({"has_precipitation":true,"temperature":0.8,"carvers":[],"features":[]}),
                    ),
                    _ => None,
                };
                if let Some(value) = value {
                    return Ok(Some(serde_json::to_vec(&value)?));
                }
                if path == "data/example/structure/test.nbt" {
                    return Ok(Some(self.template.clone()));
                }
                if path.starts_with("data/minecraft/tags/") {
                    return self.vanilla.read_data(path);
                }
                if let Some(bytes) = Data.read_data(path)? {
                    return Ok(Some(bytes));
                }
                self.vanilla.read_data(path)
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                match folder {
                    "worldgen/structure" => Ok(vec![ResourceId::parse("example:test")?]),
                    "worldgen/biome" => Ok(vec![ResourceId::parse("minecraft:plains")?]),
                    _ => self.vanilla.list_data(folder),
                }
            }
        }
        let list = |element_type, values| Tag::List {
            element_type,
            values,
        };
        for unsupported in [false, true] {
            let mut block = vec![
                (
                    "pos".into(),
                    list(TagType::Int, vec![Tag::Int(0), Tag::Int(0), Tag::Int(0)]),
                ),
                ("state".into(), Tag::Int(0)),
            ];
            if unsupported {
                block.push(("nbt".into(), Tag::Compound(vec![])));
            }
            let document = NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![
                    ("DataVersion".into(), Tag::Int(5023)),
                    (
                        "size".into(),
                        list(TagType::Int, vec![Tag::Int(1), Tag::Int(1), Tag::Int(1)]),
                    ),
                    (
                        "palette".into(),
                        list(
                            TagType::Compound,
                            vec![Tag::Compound(vec![(
                                "id".into(),
                                Tag::String("minecraft:stone_bricks".into()),
                            )])],
                        ),
                    ),
                    (
                        "blocks".into(),
                        list(TagType::Compound, vec![Tag::Compound(block)]),
                    ),
                    ("entities".into(), list(TagType::Compound, vec![])),
                ]),
            };
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&nbt::write(&document)?)?;
            let game =
                native_resources::default_minecraft_dir().context("Minecraft directory missing")?;
            let mut vanilla = native_resources::packs::ResourceStack::open(&game)?;
            let blocks = vanilla.vanilla_block_ids()?;
            let registry = vanilla.vanilla_generated_block_registry()?;
            let mut data = FixtureData {
                vanilla,
                template: encoder.finish()?,
            };
            let (generator, terrain_generator) = TerrainGenerator::paired_with_structure_context(
                &mut data,
                &definition(),
                42,
                0,
                16,
            )?;
            let structure = ResourceId::parse("example:test")?;
            let mut cache = StructureSetCache::default();
            let [x, z] = cache
                .load(&mut data, &structure)?
                .placement
                .potential_chunk(42, [0, 0]);
            let target = ChunkPos { x, z };
            let rules = StructureBiomes::load(&mut data, [structure.clone()])?;
            let mut structures = structure_batch::StructureGenerationBatch::new(
                generator,
                rules,
                vec![structure.clone()],
            )?;
            let preparation = structure_batch::StructurePreparationPlan::new([(
                target,
                schedule::GenerationStage::Features,
            )])?;
            preparation.request_structures(&mut structures)?;
            while structures
                .step(&mut data, &mut |value| {
                    let name = value.as_str().context("expected block")?;
                    Ok(BlockHeightFlags {
                        is_air: name == "minecraft:air",
                        blocks_motion: matches!(name, "minecraft:stone" | "minecraft:dirt"),
                        blocks_motion_no_leaves: matches!(
                            name,
                            "minecraft:stone" | "minecraft:dirt"
                        ),
                        has_fluid: matches!(name, "minecraft:water" | "minecraft:lava"),
                    })
                })?
                .is_some()
            {}
            let origin = structures.jigsaw_starts(target).context("missing jigsaw")?[&structure]
                .pieces[0]
                .origin;
            let tags = CarverBlockTags::load(&mut data, &blocks)?;
            let mut batch = preparation.into_expandable_terrain_batch(
                structures,
                terrain_generator,
                tags,
                "minecraft:overworld",
            )?;
            let lighting = GeneratedDimensionLighting {
                dimension: ResourceId::parse("minecraft:overworld")?,
                dimension_type: None,
                inline_dimension_type: None,
                min_y: 0,
                height: 16,
                has_skylight: true,
                ambient_light: Some(0.0),
            };
            batch.enable_decoration(&mut data, &BTreeSet::new(), blocks.clone(), &lighting)?;
            batch.enable_light_initialization(registry)?;
            batch.request_features(target)?;
            let mut failed = false;
            loop {
                let before = batch.snapshot();
                let result = batch.step(
                    &mut data,
                    &mut |value| {
                        let name = value.as_str().context("expected block")?;
                        Ok(CarverBlockState {
                            is_air: name == "minecraft:air",
                            has_fluid: matches!(name, "minecraft:water" | "minecraft:lava"),
                        })
                    },
                    &mut |value| {
                        let name = value.as_str().context("expected block")?;
                        Ok(BlockState {
                            name: name.into(),
                            properties: BTreeMap::new(),
                        })
                    },
                );
                match result {
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(error) if unsupported => {
                        ensure!(
                            format!("{error:#}").contains("jigsaw block entity placement"),
                            "unexpected failure: {error:#}"
                        );
                        let before = before.context("missing pre-failure terrain")?;
                        assert!(Arc::ptr_eq(
                            &before,
                            &batch.snapshot().context("missing terrain")?
                        ));
                        assert_ne!(
                            batch.schedule().completed(target),
                            Some(schedule::GenerationStage::Features)
                        );
                        failed = true;
                        break;
                    }
                    Err(error) => return Err(error),
                }
            }
            assert_eq!(failed, unsupported);
            if !unsupported {
                assert_eq!(
                    batch.schedule().completed(target),
                    Some(schedule::GenerationStage::Features)
                );
                assert_eq!(
                    batch
                        .snapshot()
                        .context("missing terrain")?
                        .block(origin[0], origin[1], origin[2])
                        .context("missing placed block")?
                        .name,
                    "minecraft:stone_bricks"
                );
                assert!(batch.structure_data(target).is_ok());
            }
        }
        Ok(())
    }

    fn run_ocean_structure_batch(mixed: bool, dynamic_biomes: bool) -> Result<()> {
        use flate2::{Compression, write::GzEncoder};
        use native_persistence::{
            heightmap::BlockHeightFlags,
            nbt::{self, NamedTag, Tag, TagType},
        };
        use native_resources::{
            carver::tags::CarverBlockTags, dimension_lighting::GeneratedDimensionLighting,
            drowned_spawn::MainHandRoll, ore_feature::OreBlockTags,
            structure_biomes::StructureBiomes, structure_placement::StructureSetCache,
            worldgen_region_random::RegionRandom,
        };
        use native_world::loaded::{BlockState, ChunkPos, World};
        use std::io::Write;
        struct NoDrowned;
        impl Clone for NoDrowned {
            fn clone(&self) -> Self {
                Self
            }
        }
        impl ocean_ruin::DrownedMarkerContext for NoDrowned {
            fn special_multiplier(&mut self, _: [i32; 3], _: &World) -> Result<f32> {
                anyhow::bail!("unexpected drowned")
            }
            fn halloween(&mut self) -> Result<bool> {
                anyhow::bail!("unexpected drowned")
            }
            fn try_existing_chicken(&mut self, _: [i32; 3], _: &World) -> Result<bool> {
                anyhow::bail!("unexpected drowned")
            }
            fn spawn_chicken_jockey(
                &mut self,
                _: [i32; 3],
                _: &World,
                _: &mut RegionRandom,
            ) -> Result<bool> {
                anyhow::bail!("unexpected drowned")
            }
            fn enchant_weapon(
                &mut self,
                _: [i32; 3],
                _: &World,
                _: &mut RegionRandom,
                _: MainHandRoll,
                _: f32,
            ) -> Result<Tag> {
                anyhow::bail!("unexpected drowned")
            }
            fn more_frequent_drowned_biome(&mut self, _: [i32; 3], _: &World) -> Result<bool> {
                anyhow::bail!("unexpected drowned")
            }
            fn spawn_zombie_nautilus(
                &mut self,
                _: [i32; 3],
                _: &World,
                _: &mut RegionRandom,
                _: bool,
            ) -> Result<bool> {
                anyhow::bail!("unexpected drowned")
            }
        }
        impl ocean_ruin::DrownedSpawnEffects for NoDrowned {
            fn try_existing_chicken(&mut self, pos: [i32; 3], world: &World) -> Result<bool> {
                ocean_ruin::DrownedMarkerContext::try_existing_chicken(self, pos, world)
            }
            fn spawn_chicken_jockey(
                &mut self,
                pos: [i32; 3],
                world: &World,
                random: &mut RegionRandom,
                _: &BiomeNeighborhood,
            ) -> Result<bool> {
                ocean_ruin::DrownedMarkerContext::spawn_chicken_jockey(self, pos, world, random)
            }
            fn enchant_weapon(
                &mut self,
                pos: [i32; 3],
                world: &World,
                random: &mut RegionRandom,
                weapon: MainHandRoll,
                special: f32,
            ) -> Result<Tag> {
                ocean_ruin::DrownedMarkerContext::enchant_weapon(
                    self, pos, world, random, weapon, special,
                )
            }
            fn spawn_zombie_nautilus(
                &mut self,
                pos: [i32; 3],
                world: &World,
                random: &mut RegionRandom,
                persistent: bool,
            ) -> Result<bool> {
                ocean_ruin::DrownedMarkerContext::spawn_zombie_nautilus(
                    self, pos, world, random, persistent,
                )
            }
        }
        struct FixtureData {
            vanilla: native_resources::packs::ResourceStack,
            template: Vec<u8>,
            mixed: bool,
        }
        impl ResourceProvider for FixtureData {
            fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                self.vanilla.read_asset(path)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                let json = match path {
                    "data/example/worldgen/structure/ruin.json" => Some(
                        json!({"type":"minecraft:ocean_ruin","biomes":["minecraft:plains"],"step":"surface_structures","biome_temp":"warm","large_probability":0.0,"cluster_probability":0.0}),
                    ),
                    "data/example/worldgen/structure_set/ruin.json" => Some(
                        json!({"placement":{"type":"minecraft:random_spread","spacing":32,"separation":0,"salt":0},"structures":[{"structure":"example:ruin","weight":1}]}),
                    ),
                    "data/example/worldgen/structure/hut.json" if self.mixed => Some(
                        json!({"type":"minecraft:swamp_hut","biomes":["minecraft:plains"],"step":"surface_structures"}),
                    ),
                    "data/example/worldgen/structure_set/hut.json" if self.mixed => Some(
                        json!({"placement":{"type":"minecraft:random_spread","spacing":32,"separation":0,"salt":0},"structures":[{"structure":"example:hut","weight":1}]}),
                    ),
                    "data/minecraft/cat_variant/black.json" if self.mixed => Some(json!({})),
                    "data/example/cat_variant/structure_cat.json" if self.mixed => Some(
                        json!({"spawn_conditions":[{"condition":{"type":"minecraft:structure","structures":"example:hut"},"priority":0}]}),
                    ),
                    "data/minecraft/worldgen/biome/plains.json" => Some(
                        json!({"has_precipitation":true,"temperature":0.8,"carvers":[],"features":[]}),
                    ),
                    "data/minecraft/tags/worldgen/biome/more_frequent_drowned_spawns.json" => {
                        Some(json!({"values":[]}))
                    }
                    _ => None,
                };
                if let Some(value) = json {
                    return Ok(Some(serde_json::to_vec(&value)?));
                }
                if path.starts_with("data/minecraft/structure/underwater_ruin/") {
                    return Ok(Some(self.template.clone()));
                }
                if path.starts_with("data/minecraft/tags/") {
                    return self.vanilla.read_data(path);
                }
                if let Some(bytes) = Data.read_data(path)? {
                    return Ok(Some(bytes));
                }
                self.vanilla.read_data(path)
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                match folder {
                    "worldgen/structure" => {
                        let mut ids = vec![ResourceId::parse("example:ruin")?];
                        if self.mixed {
                            ids.push(ResourceId::parse("example:hut")?);
                        }
                        Ok(ids)
                    }
                    "worldgen/biome" => Ok(vec![ResourceId::parse("minecraft:plains")?]),
                    "cat_variant" if self.mixed => Ok(vec![
                        ResourceId::parse("minecraft:black")?,
                        ResourceId::parse("example:structure_cat")?,
                    ]),
                    "cat_sound_variant" if self.mixed => {
                        Ok(vec![ResourceId::parse("minecraft:classic")?])
                    }
                    _ => self.vanilla.list_data(folder),
                }
            }
        }
        let list = |element_type, values| Tag::List {
            element_type,
            values,
        };
        let document = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                (
                    "size".into(),
                    list(TagType::Int, vec![Tag::Int(1), Tag::Int(1), Tag::Int(1)]),
                ),
                (
                    "palette".into(),
                    list(
                        TagType::Compound,
                        vec![Tag::Compound(vec![(
                            "id".into(),
                            Tag::String("minecraft:stone_bricks".into()),
                        )])],
                    ),
                ),
                (
                    "blocks".into(),
                    list(
                        TagType::Compound,
                        vec![Tag::Compound(vec![
                            (
                                "pos".into(),
                                list(TagType::Int, vec![Tag::Int(0), Tag::Int(0), Tag::Int(0)]),
                            ),
                            ("state".into(), Tag::Int(0)),
                        ])],
                    ),
                ),
                ("entities".into(), list(TagType::Compound, vec![])),
            ]),
        };
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&nbt::write(&document)?)?;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut vanilla = native_resources::packs::ResourceStack::open(&game)?;
        let blocks = vanilla.vanilla_block_ids()?;
        let registry = vanilla.vanilla_generated_block_registry()?;
        let mut data = FixtureData {
            vanilla,
            template: encoder.finish()?,
            mixed,
        };
        let structure = ResourceId::parse("example:ruin")?;
        let set = ResourceId::parse("example:ruin")?;
        let mut config = definition();
        config["settings"]["noise"]["height"] = json!(96);
        config["settings"]["material_rule"] =
            json!({"type":"block","result_state":"minecraft:stone"});
        config["settings"]["noise_router"]["final_density"] = json!({"type":"gradient","axis":"y","from_coordinate":0,"to_coordinate":96,"from_value":1,"to_value":-1});
        let (generator, terrain_generator) =
            TerrainGenerator::paired_with_structure_context(&mut data, &config, 42, 0, 96)?;
        let [x, z] = StructureSetCache::default()
            .load(&mut data, &set)?
            .placement
            .potential_chunk(42, [0, 0]);
        let target = ChunkPos { x, z };
        let mut expected_region_random = generator.region_random(target);
        let mut kinds = vec![structure.clone()];
        let mut sets = vec![set];
        if mixed {
            kinds.push(ResourceId::parse("example:hut")?);
            sets.push(ResourceId::parse("example:hut")?);
        }
        let rules = StructureBiomes::load(&mut data, kinds)?;
        let mut structures =
            structure_batch::StructureGenerationBatch::new(generator, rules, sets)?;
        let preparation = structure_batch::StructurePreparationPlan::new([(
            target,
            schedule::GenerationStage::Features,
        )])?;
        preparation.request_structures(&mut structures)?;
        while structures
            .step(&mut data, &mut |value| {
                let name = value.as_str().context("expected block")?;
                Ok(BlockHeightFlags {
                    is_air: name == "minecraft:air",
                    blocks_motion: matches!(name, "minecraft:stone" | "minecraft:dirt"),
                    blocks_motion_no_leaves: matches!(name, "minecraft:stone" | "minecraft:dirt"),
                    has_fluid: matches!(name, "minecraft:water" | "minecraft:lava"),
                })
            })?
            .is_some()
        {}
        let tags = CarverBlockTags::load(&mut data, &blocks)?;
        let mut batch = preparation.into_expandable_terrain_batch(
            structures,
            terrain_generator,
            tags,
            "minecraft:overworld",
        )?;
        let initial = batch
            .ocean_ruin_starts(target)
            .context("ruin start missing")?[&structure]
            .pieces[0]
            .selection
            .position;
        assert_eq!(initial[1], 90);
        let lighting = GeneratedDimensionLighting {
            dimension: ResourceId::parse("minecraft:overworld")?,
            dimension_type: None,
            inline_dimension_type: None,
            min_y: 0,
            height: 96,
            has_skylight: true,
            ambient_light: Some(0.0),
        };
        batch.enable_decoration(&mut data, &BTreeSet::new(), blocks.clone(), &lighting)?;
        let ice = OreBlockTags::load(
            &mut data,
            &blocks,
            &BTreeSet::from([ResourceId::parse("minecraft:ice")?]),
        )?;
        let water = BTreeSet::from([
            "minecraft:water".to_owned(),
            "minecraft:flowing_water".to_owned(),
        ]);
        let bubbles = BTreeSet::new();
        let mut ocean = OceanRuinPlacementContext {
            ice_tags: &ice,
            water_fluids: &water,
            bubble_fluids: &bubbles,
            registry: &registry,
            sea_level: 8,
            region_random: RegionRandom::Legacy(native_core::random::LegacyRandom::new(0)),
            construct: || -> Result<Option<FreshCreatureRandom>> {
                anyhow::bail!("unexpected drowned")
            },
            context: NoDrowned,
        };
        let variants = native_resources::cat_variants::CatSpawnVariants::load_active(&mut data)?;
        let conditions = if mixed {
            Some(variants.prepare_world_conditions(&mut data)?)
        } else {
            None
        };
        let biome_tags = if dynamic_biomes {
            Some(native_resources::drowned_spawn::DrownedBiomeTags::load(
                &mut data,
            )?)
        } else {
            None
        };
        let mut inputs = biome_tags
            .as_ref()
            .map(|biome_tags| ocean_ruin::DrownedPlacementInputs {
                ice_tags: &ice,
                water_fluids: &water,
                bubble_fluids: &bubbles,
                registry: &registry,
                biome_tags,
                sea_level: 8,
                construct: || -> Result<Option<FreshCreatureRandom>> {
                    bail!("unexpected drowned")
                },
                effects: NoDrowned,
                difficulty_at: |_: [i32; 3], _: &World, _: &BiomeNeighborhood| {
                    Ok(ocean_ruin::GenerationDifficultyInputs {
                        difficulty: native_persistence::world::Difficulty::Normal,
                        overworld_clock_time: 0,
                        moon_brightness: 1.0,
                    })
                },
                halloween_at: || Ok(false),
                last_region_random: None,
            });
        batch.request_features(target)?;
        loop {
            let mut classify = |value: &Value| {
                let name = value.as_str().context("expected block")?;
                Ok(CarverBlockState {
                    is_air: name == "minecraft:air",
                    has_fluid: matches!(name, "minecraft:water" | "minecraft:lava"),
                })
            };
            let mut resolve = |value: &Value| {
                Ok(BlockState {
                    name: value.as_str().context("expected block")?.to_owned(),
                    properties: BTreeMap::new(),
                })
            };
            let result = if dynamic_biomes {
                batch.step_with_drowned_environment(
                    &mut data,
                    &mut classify,
                    &mut resolve,
                    inputs.as_mut().context("missing drowned inputs")?,
                    &variants,
                    conditions.as_ref().context("missing cat conditions")?,
                    &|_, _| Ok(0),
                )?
            } else if mixed {
                batch.step_with_native_mixed_environment(
                    &mut data,
                    &mut classify,
                    &mut resolve,
                    &mut ocean,
                    &variants,
                    conditions.as_ref().unwrap(),
                    &|_, _| Ok(0),
                )?
            } else {
                batch.step_with_ocean_support(&mut data, &mut classify, &mut resolve, &mut ocean)?
            };
            if result.is_none() {
                break;
            }
        }
        let start = &batch
            .ocean_ruin_starts(target)
            .context("ruin start missing")?[&structure];
        let position = start.pieces[0].selection.position;
        assert_ne!(position[1], initial[1]);
        assert_eq!(
            batch
                .snapshot()
                .context("missing terrain")?
                .block(position[0], position[1], position[2])
                .context("missing ruin block")?
                .name,
            "minecraft:stone_bricks"
        );
        assert_eq!(
            batch.schedule().completed(target),
            Some(schedule::GenerationStage::Features)
        );
        if mixed {
            if dynamic_biomes {
                assert!(inputs.as_ref().unwrap().last_region_random.is_some());
            }
            assert_eq!(batch.recorded_generated_entities(target)?.len(), 2);
            let data = batch.structure_data(target)?;
            let saved_hut = data
                .get("starts")
                .and_then(|tag| tag.get("example:hut"))
                .context("missing hut")?;
            let Some(native_persistence::nbt::Tag::List { values, .. }) = saved_hut.get("Children")
            else {
                bail!("missing hut children")
            };
            assert_eq!(
                values[0].get("Witch"),
                Some(&native_persistence::nbt::Tag::Byte(1))
            );
            assert_eq!(
                values[0].get("Cat"),
                Some(&native_persistence::nbt::Tag::Byte(1))
            );
            let cat = batch.recorded_generated_entities(target)?.iter().find(|entity| matches!(entity.get("id"), Some(native_persistence::nbt::Tag::String(id)) if id.matches("minecraft:cat"))).context("missing cat")?;
            assert!(
                matches!(cat.get("variant"), Some(native_persistence::nbt::Tag::String(id)) if id.matches("example:structure_cat"))
            );
        } else {
            assert_eq!(
                ocean.region_random.next_long(),
                expected_region_random.next_long()
            );
        }
        assert!(
            batch
                .structure_data(target)?
                .get("starts")
                .and_then(|tag| tag.get("example:ruin"))
                .is_some()
        );
        Ok(())
    }

    #[test]
    fn fresh_structure_batch_decorates_and_retains_buried_treasure_start() {
        use native_resources::{
            carver::tags::CarverBlockTags, dimension_lighting::GeneratedDimensionLighting,
            structure_biomes::StructureBiomes, structure_placement::StructureSetCache,
        };
        use native_world::loaded::{BlockState, ChunkPos};
        struct TreasureData;
        impl ResourceProvider for TreasureData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                let value = match path {
                    "data/example/worldgen/structure/treasure.json" => Some(
                        json!({"type":"minecraft:buried_treasure","biomes":["minecraft:plains"],"step":"surface_structures"}),
                    ),
                    "data/example/worldgen/structure_set/treasure.json" => Some(
                        json!({"placement":{"type":"minecraft:random_spread","spacing":32,"separation":0,"salt":0},"structures":[{"structure":"example:treasure","weight":1}]}),
                    ),
                    "data/minecraft/worldgen/biome/plains.json" => Some(
                        json!({"has_precipitation":true,"temperature":0.8,"carvers":[],"features":[]}),
                    ),
                    "data/minecraft/tags/block/blocks_motion_in_heightmap.json"
                    | "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json" => {
                        Some(
                            json!({"values":["minecraft:stone","minecraft:dirt","minecraft:sand","minecraft:chest"]}),
                        )
                    }
                    _ => None,
                };
                if let Some(value) = value {
                    Ok(Some(serde_json::to_vec(&value)?))
                } else {
                    Data.read_data(path)
                }
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                if folder == "worldgen/structure" {
                    Ok(vec![ResourceId::parse("example:treasure")?])
                } else {
                    Data.list_data(folder)
                }
            }
        }
        let mut data = TreasureData;
        let mut config = definition();
        config["settings"]["noise"]["height"] = json!(96);
        config["settings"]["material_rule"] =
            json!({"type":"block","result_state":"minecraft:stone"});
        config["settings"]["noise_router"]["final_density"] = json!({"type":"gradient","axis":"y","from_coordinate":0,"to_coordinate":96,"from_value":1,"to_value":-1});
        let (generator, terrain_generator) =
            TerrainGenerator::paired_with_structure_context(&mut data, &config, 42, 0, 96).unwrap();
        let structure = ResourceId::parse("example:treasure").unwrap();
        let set = ResourceId::parse("example:treasure").unwrap();
        let [x, z] = StructureSetCache::default()
            .load(&mut data, &set)
            .unwrap()
            .placement
            .potential_chunk(42, [0, 0]);
        let target = ChunkPos { x, z };
        let rules = StructureBiomes::load(&mut data, [structure.clone()]).unwrap();
        let mut structures =
            structure_batch::StructureGenerationBatch::new(generator, rules, vec![set]).unwrap();
        let preparation = structure_batch::StructurePreparationPlan::new([(
            target,
            schedule::GenerationStage::Features,
        )])
        .unwrap();
        preparation.request_structures(&mut structures).unwrap();
        while structures
            .step(&mut data, &mut |value| {
                let name = value.as_str().unwrap();
                Ok(BlockHeightFlags {
                    is_air: name == "minecraft:air",
                    blocks_motion: matches!(name, "minecraft:stone" | "minecraft:dirt"),
                    blocks_motion_no_leaves: matches!(name, "minecraft:stone" | "minecraft:dirt"),
                    has_fluid: matches!(name, "minecraft:water" | "minecraft:lava"),
                })
            })
            .unwrap()
            .is_some()
        {}
        let blocks: BTreeSet<_> = [
            "air", "cave_air", "void_air", "bedrock", "stone", "dirt", "water", "lava", "sand",
            "chest",
        ]
        .into_iter()
        .map(|name| format!("minecraft:{name}"))
        .collect();
        let tags = CarverBlockTags::load(&mut data, &blocks).unwrap();
        let unrelated =
            TerrainGenerator::with_structure_context(&mut data, &config, 42, 0, 96).unwrap();
        let other_rules = StructureBiomes::load(&mut data, [structure.clone()]).unwrap();
        let other_generator =
            TerrainGenerator::with_structure_context(&mut data, &config, 42, 0, 96).unwrap();
        let other =
            structure_batch::StructureGenerationBatch::new(other_generator, other_rules, vec![])
                .unwrap();
        let other_tags = CarverBlockTags::load(&mut data, &blocks).unwrap();
        assert!(
            preparation
                .into_expandable_terrain_batch(other, unrelated, other_tags, "minecraft:overworld")
                .is_err()
        );
        let mut batch = preparation
            .into_expandable_terrain_batch(
                structures,
                terrain_generator,
                tags,
                "minecraft:overworld",
            )
            .unwrap();
        assert_eq!(
            batch.structure_starts(target).unwrap()[&structure]
                .bounds
                .min[1],
            90
        );
        let lighting = GeneratedDimensionLighting {
            dimension: ResourceId::parse("minecraft:overworld").unwrap(),
            dimension_type: None,
            inline_dimension_type: None,
            min_y: 0,
            height: 96,
            has_skylight: true,
            ambient_light: Some(0.0),
        };
        batch
            .enable_decoration(&mut data, &BTreeSet::new(), blocks, &lighting)
            .unwrap();
        batch.request_features(target).unwrap();
        while batch
            .step(
                &mut data,
                &mut |value| {
                    let name = value.as_str().unwrap();
                    Ok(CarverBlockState {
                        is_air: name == "minecraft:air",
                        has_fluid: matches!(name, "minecraft:water" | "minecraft:lava"),
                    })
                },
                &mut |value| {
                    Ok(BlockState {
                        name: value.as_str().unwrap().into(),
                        properties: BTreeMap::new(),
                    })
                },
            )
            .unwrap()
            .is_some()
        {}
        let start = &batch.structure_starts(target).unwrap()[&structure];
        assert_ne!(start.bounds.min[1], 90);
        assert_eq!(
            batch
                .snapshot()
                .unwrap()
                .block(
                    start.bounds.min[0],
                    start.bounds.min[1],
                    start.bounds.min[2]
                )
                .unwrap()
                .name,
            "minecraft:chest"
        );
        let data = batch.structure_data(target).unwrap();
        let start_tag = data.get("starts").unwrap().get("example:treasure").unwrap();
        let children = match start_tag.get("Children").unwrap() {
            native_persistence::nbt::Tag::List { values, .. } => values,
            _ => panic!("structure children are not a list"),
        };
        assert_eq!(
            children[0].get("BB"),
            Some(&native_persistence::nbt::Tag::IntArray(
                start
                    .bounds
                    .min
                    .into_iter()
                    .chain(start.bounds.max)
                    .collect()
            ))
        );
        let next = ChunkPos {
            x: target.x + 3,
            z: target.z,
        };
        let expansion = structure_batch::StructurePreparationPlan::new([(
            next,
            schedule::GenerationStage::Features,
        )])
        .unwrap();
        let old_world = batch.snapshot().unwrap();
        assert!(batch.request_features(next).is_err());
        assert!(batch.finish_structure_expansion(&expansion).is_err());
        assert_eq!(batch.structure_data(target).unwrap(), data);
        batch.request_structure_expansion(&expansion).unwrap();
        assert!(batch.request_features(target).is_err());
        while batch
            .step_structure_expansion(&mut TreasureData, &mut |value| {
                let name = value.as_str().unwrap();
                Ok(BlockHeightFlags {
                    is_air: name == "minecraft:air",
                    blocks_motion: matches!(name, "minecraft:stone" | "minecraft:dirt"),
                    blocks_motion_no_leaves: matches!(name, "minecraft:stone" | "minecraft:dirt"),
                    has_fluid: matches!(name, "minecraft:water" | "minecraft:lava"),
                })
            })
            .unwrap()
            .is_some()
        {}
        batch.finish_structure_expansion(&expansion).unwrap();
        assert_eq!(batch.structure_data(target).unwrap(), data);
        assert!(Arc::ptr_eq(&batch.snapshot().unwrap(), &old_world));
        batch.request_features(next).unwrap();
        while batch
            .step(
                &mut TreasureData,
                &mut |value| {
                    let name = value.as_str().unwrap();
                    Ok(CarverBlockState {
                        is_air: name == "minecraft:air",
                        has_fluid: matches!(name, "minecraft:water" | "minecraft:lava"),
                    })
                },
                &mut |value| {
                    Ok(BlockState {
                        name: value.as_str().unwrap().into(),
                        properties: BTreeMap::new(),
                    })
                },
            )
            .unwrap()
            .is_some()
        {}
        assert_eq!(batch.structure_data(target).unwrap(), data);
        let new_world = batch.snapshot().unwrap();
        for y in 0..96 {
            for z in 0..16 {
                for x in 0..16 {
                    let (x, z) = (target.x * 16 + x, target.z * 16 + z);
                    assert_eq!(old_world.block(x, y, z), new_world.block(x, y, z));
                }
            }
        }
        assert!(batch.into_parts().is_err());
    }

    #[test]
    fn simple_structure_set_uses_base_noise_and_fixed_biome() {
        use native_core::beardifier::Beardifier;
        use native_resources::{
            structure_biomes::StructureBiomes,
            structure_placement::{StructureSetCache, StructureSetStart},
        };
        use native_world::loaded::ChunkPos;
        struct StructureData(BTreeMap<String, Vec<u8>>);
        impl ResourceProvider for StructureData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if let Some(value) = self.0.get(path) {
                    Ok(Some(value.clone()))
                } else if path.starts_with("data/minecraft/structure/underwater_ruin/") {
                    Ok(self.0.get("ruin-template").cloned())
                } else {
                    Data.read_data(path)
                }
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                if folder == "worldgen/structure" {
                    Ok(vec![
                        ResourceId::parse("example:pyramid")?,
                        ResourceId::parse("example:absent")?,
                    ])
                } else if folder == "worldgen/structure_set" {
                    Ok(vec![
                        ResourceId::parse("example:sparse")?,
                        ResourceId::parse("example:pyramid")?,
                    ])
                } else {
                    Data.list_data(folder)
                }
            }
        }
        let mut data=StructureData(BTreeMap::from([
            ("data/example/worldgen/structure/pyramid.json".into(),serde_json::to_vec(&json!({"type":"minecraft:desert_pyramid","biomes":["minecraft:plains"],"step":"surface_structures","terrain_adaptation":"bury"})).unwrap()),
            ("data/example/worldgen/structure/absent.json".into(),serde_json::to_vec(&json!({"type":"minecraft:desert_pyramid","biomes":["minecraft:plains"],"step":"surface_structures"})).unwrap()),
            ("data/example/worldgen/structure_set/pyramid.json".into(),serde_json::to_vec(&json!({"placement":{"type":"minecraft:random_spread","spacing":1,"separation":0,"salt":0},"structures":[{"structure":"example:pyramid","weight":1}]})).unwrap()),
        ]));
        let mut generator =
            TerrainGenerator::with_structure_context(&mut data, &definition(), 42, 0, 16).unwrap();
        let structure_id = ResourceId::parse("example:pyramid").unwrap();
        let rules = StructureBiomes::load(&mut data, [structure_id.clone()]).unwrap();
        let set_id = ResourceId::parse("example:pyramid").unwrap();
        let mut cache = StructureSetCache::default();
        let mut evaluator = DensityEvaluator::default();
        let classify = |v: &Value| -> Result<BlockHeightFlags> {
            let id = v.as_str().unwrap();
            Ok(BlockHeightFlags {
                is_air: id == "minecraft:air",
                blocks_motion: id == "minecraft:stone",
                blocks_motion_no_leaves: id == "minecraft:stone",
                has_fluid: matches!(id, "minecraft:water" | "minecraft:lava"),
            })
        };
        let result = generator
            .try_generate_simple_structure_set(
                &mut data,
                &mut evaluator,
                &mut cache,
                &rules,
                &set_id,
                [0, 0],
                |_| Ok(false),
                classify,
            )
            .unwrap();
        let StructureSetStart::Generated { structure, start } = result else {
            panic!("expected generated structure")
        };
        assert_eq!(structure, structure_id);
        assert_eq!(start.generation_point, [8, 15, 8]);
        let adjustment = Beardifier::for_chunk([0, 0], &[start.terrain_piece()]).unwrap();
        assert!(adjustment.affected_bounds().is_some());
        let existing = generator
            .try_generate_simple_structure_set(
                &mut data,
                &mut evaluator,
                &mut cache,
                &rules,
                &set_id,
                [0, 0],
                |_| Ok(true),
                |_| panic!("existing start must not classify blocks"),
            )
            .unwrap();
        assert!(matches!(existing, StructureSetStart::Existing));

        // An eligible template-based structure must still stop generation
        // until its pieces are supported, even when its center check passes.
        let path = "data/example/worldgen/structure/pyramid.json";
        let original = data.0.get(path).unwrap().clone();
        for definition in [
            json!({"type":"minecraft:shipwreck","biomes":["minecraft:plains"],"is_beached":true}),
            json!({"type":"minecraft:igloo","biomes":["minecraft:plains"]}),
            json!({"type":"minecraft:ocean_ruin","biomes":["minecraft:plains"],"biome_temp":"warm","large_probability":0.3,"cluster_probability":0.9}),
        ] {
            data.0
                .insert(path.into(), serde_json::to_vec(&definition).unwrap());
            let error = generator
                .try_generate_simple_structure_set(
                    &mut data,
                    &mut evaluator,
                    &mut cache,
                    &rules,
                    &set_id,
                    [0, 0],
                    |_| Ok(false),
                    classify,
                )
                .err()
                .expect("eligible unsupported structure must fail");
            assert!(format!("{error:#}").contains("unsupported simple structure"));
        }
        // A generated mineshaft is retained as an intermediate start.
        for variant in ["normal", "mesa"] {
            let definition = json!({"type":"minecraft:mineshaft","biomes":["minecraft:plains"],"mineshaft_type":variant,"step":"underground_structures","terrain_adaptation":"beard_thin"});
            data.0
                .insert(path.into(), serde_json::to_vec(&definition).unwrap());
            let outcome = generator
                .try_generate_supported_structure_set(
                    &mut data,
                    &mut evaluator,
                    &mut cache,
                    &rules,
                    &set_id,
                    [0, 0],
                    |_| Ok(false),
                    classify,
                )
                .unwrap();
            let StructureSetStart::Generated {
                structure,
                start: PreparedStructureStart::Mineshaft(start),
            } = outcome
            else {
                panic!("expected mineshaft start")
            };
            assert_eq!(structure, structure_id);
            assert!(!start.pieces.is_empty());
            assert!(start.reference_bounds(structure).is_ok());
        }
        {
            let sparse = ResourceId::parse("example:mine_sparse").unwrap();
            data.0.insert("data/example/worldgen/structure_set/mine_sparse.json".into(), serde_json::to_vec(&json!({"placement":{"type":"minecraft:random_spread","spacing":32,"separation":0,"salt":0},"structures":[{"structure":"example:pyramid","weight":1}]})).unwrap());
            let [x, z] = StructureSetCache::default()
                .load(&mut data, &sparse)
                .unwrap()
                .placement
                .potential_chunk(42, [0, 0]);
            let target = ChunkPos { x, z };
            let batch_generator =
                TerrainGenerator::with_structure_context(&mut data, &definition(), 42, 0, 16)
                    .unwrap();
            let batch_rules = StructureBiomes::load(&mut data, [structure_id.clone()]).unwrap();
            let mut batch = structure_batch::StructureGenerationBatch::new(
                batch_generator,
                batch_rules,
                vec![sparse],
            )
            .unwrap();
            batch.request_references(target).unwrap();
            while batch
                .step(&mut data, &mut |v| classify(v))
                .unwrap()
                .is_some()
            {}
            assert!(batch.is_complete());
            assert!(batch.starts(target).is_none());
            let start = &batch.mineshaft_starts(target).unwrap()[&structure_id];
            assert!(!start.pieces.is_empty());
            let packed = (target.x as u32 as u64 | ((target.z as u32 as u64) << 32)) as i64;
            assert!(batch.references(target).unwrap()[&structure_id].contains(&packed));
            let order = batch
                .unambiguous_terrain_start_order(target)
                .unwrap()
                .unwrap();
            assert!(order.contains(&(structure_id.clone(), target)));
            let actual = batch.beardifier_for_chunk(target, &order).unwrap();
            assert!(actual.affected_bounds().is_some());
            let mut pieces = Vec::new();
            for (id, source) in &order {
                let start = &batch.mineshaft_starts(*source).unwrap()[id];
                pieces.extend(start.pieces.iter().map(|piece| {
                    native_core::beardifier::StructurePiece {
                        bounds: piece.piece.bounds,
                        adjustment: start.terrain_adjustment,
                        ground_delta: 0,
                        projection: native_core::beardifier::PieceProjection::NonPool,
                        junctions: vec![],
                    }
                }));
            }
            let expected = Beardifier::for_chunk([target.x, target.z], &pieces).unwrap();
            for y in [-32, 0, 32, 64] {
                for dx in [0, 8, 15] {
                    for dz in [0, 8, 15] {
                        let x = target.x * 16 + dx;
                        let z = target.z * 16 + dz;
                        assert_eq!(
                            actual.sample(x, y, z).to_bits(),
                            expected.sample(x, y, z).to_bits()
                        );
                    }
                }
            }
            let schedule =
                native_resources::structure_decoration::StructureDecorationSchedule::load(
                    &mut data,
                )
                .unwrap();
            let mixed = batch.mixed_decoration_plan(target, &schedule).unwrap();
            let entry = mixed
                .iter()
                .find(|entry| entry.entry.id == structure_id)
                .unwrap();
            let scheduled: Vec<_> = entry
                .starts
                .iter()
                .map(|start| {
                    assert!(matches!(
                        start,
                        structure_batch::PlannedStructureStart::Mineshaft(_)
                    ));
                    let [x, z] = start.chunk();
                    (x as u32 as u64 | ((z as u32 as u64) << 32)) as i64
                })
                .collect();
            assert_eq!(
                scheduled,
                batch.reference_order(target, &structure_id).unwrap()
            );
            assert!(batch.decoration_plan(target, &schedule).is_err());
            let plan = structure_batch::StructurePreparationPlan::new([(
                target,
                schedule::GenerationStage::StructureReferences,
            )])
            .unwrap();
            let added = batch.prepare_additional_stages(&plan, None).unwrap();
            assert_eq!(
                added.mineshafts[&target][&structure_id].pieces.len(),
                start.pieces.len()
            );
            assert!(added.has_mineshaft_reference(target).unwrap());
            let planned = added.mixed_decoration_plan(target, &schedule).unwrap();
            assert!(
                planned
                    .iter()
                    .any(|entry| entry.starts.iter().any(|start| matches!(
                        start,
                        structure_batch::PlannedStructureStart::Mineshaft(_)
                    )))
            );
            assert!(added.mixed_commit_keys(&planned, false).is_ok());
            let (_, transferred) = batch
                .into_terrain_stages(BTreeMap::from([(target, order)]))
                .unwrap();
            assert!(
                !transferred.mineshafts[&target][&structure_id]
                    .pieces
                    .is_empty()
            );
            assert!(
                transferred
                    .mixed_decoration_plan(target, &schedule)
                    .unwrap()
                    .iter()
                    .any(|entry| entry.starts.iter().any(|start| matches!(
                        start,
                        structure_batch::PlannedStructureStart::Mineshaft(_)
                    )))
            );
        }
        // The generalized start path can retain a multi-piece ruin without
        // changing the simple-only scheduler's conservative behavior.
        {
            use flate2::{Compression, write::GzEncoder};
            use native_persistence::nbt::{self, NamedTag, Tag, TagType};
            use std::io::Write;
            let list = |element_type, values| Tag::List {
                element_type,
                values,
            };
            let template = NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![
                    ("DataVersion".into(), Tag::Int(5023)),
                    (
                        "size".into(),
                        list(TagType::Int, vec![Tag::Int(2), Tag::Int(3), Tag::Int(4)]),
                    ),
                    (
                        "palette".into(),
                        list(TagType::Compound, vec![Tag::Compound(vec![])]),
                    ),
                    ("blocks".into(), list(TagType::Compound, vec![])),
                    ("entities".into(), list(TagType::Compound, vec![])),
                ]),
            };
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&nbt::write(&template).unwrap()).unwrap();
            data.0
                .insert("ruin-template".into(), encoder.finish().unwrap());
            let ruin = json!({"type":"minecraft:ocean_ruin","biomes":["minecraft:plains"],"step":"surface_structures","biome_temp":"cold","large_probability":1.0,"cluster_probability":1.0,"terrain_adaptation":"beard_thin"});
            data.0
                .insert(path.into(), serde_json::to_vec(&ruin).unwrap());
            let result = generator
                .try_generate_supported_structure_set(
                    &mut data,
                    &mut evaluator,
                    &mut cache,
                    &rules,
                    &set_id,
                    [0, 0],
                    |_| Ok(false),
                    classify,
                )
                .unwrap();
            let StructureSetStart::Generated {
                structure,
                start: PreparedStructureStart::OceanRuin(start),
            } = result
            else {
                panic!("expected a multi-piece ocean ruin")
            };
            assert_eq!(structure, structure_id);
            assert_eq!(start.generation_point, [8, 15, 8]);
            assert!(start.pieces.len() > 1);
            assert!(start.reference_bounds(structure).is_ok());

            let sparse_id = ResourceId::parse("example:ruin_sparse").unwrap();
            data.0.insert(
                "data/example/worldgen/structure_set/ruin_sparse.json".into(),
                serde_json::to_vec(&json!({"placement":{"type":"minecraft:random_spread","spacing":32,"separation":0,"salt":0},"structures":[{"structure":"example:pyramid","weight":1}]})).unwrap(),
            );
            let [x, z] = StructureSetCache::default()
                .load(&mut data, &sparse_id)
                .unwrap()
                .placement
                .potential_chunk(42, [0, 0]);
            let target = ChunkPos { x, z };
            let batch_generator =
                TerrainGenerator::with_structure_context(&mut data, &definition(), 42, 0, 16)
                    .unwrap();
            let batch_rules = StructureBiomes::load(&mut data, [structure_id.clone()]).unwrap();
            let mut batch = structure_batch::StructureGenerationBatch::new(
                batch_generator,
                batch_rules,
                vec![sparse_id],
            )
            .unwrap();
            batch.request_starts(target).unwrap();
            assert!(
                batch
                    .step(&mut data, &mut |v| classify(v))
                    .unwrap()
                    .is_some()
            );
            assert!(batch.is_complete());
            assert!(batch.starts(target).is_none());
            let scheduled = &batch.ocean_ruin_starts(target).unwrap()[&structure_id];
            assert!(scheduled.pieces.len() > 1);
            let piece_count = scheduled.pieces.len();
            batch.request_references(target).unwrap();
            while batch
                .step(&mut data, &mut |v| classify(v))
                .unwrap()
                .is_some()
            {}
            let packed =
                i64::from((target.x as u32 as u64 | ((target.z as u32 as u64) << 32)) as i64);
            assert!(batch.references(target).unwrap()[&structure_id].contains(&packed));
            let decoration_schedule =
                native_resources::structure_decoration::StructureDecorationSchedule::load(
                    &mut data,
                )
                .unwrap();
            let mixed = batch
                .mixed_decoration_plan(target, &decoration_schedule)
                .unwrap();
            assert_eq!(mixed.len(), 1);
            assert_eq!(mixed[0].entry.id, structure_id);
            assert_eq!(mixed[0].entry.index, 1);
            let planned_chunks: Vec<_> = mixed[0]
                .starts
                .iter()
                .map(|start| {
                    assert!(matches!(
                        start,
                        structure_batch::PlannedStructureStart::OceanRuin(_)
                    ));
                    let [x, z] = start.chunk();
                    (x as u32 as u64 | ((z as u32 as u64) << 32)) as i64
                })
                .collect();
            assert_eq!(
                planned_chunks,
                batch.reference_order(target, &structure_id).unwrap()
            );
            assert!(batch.decoration_plan(target, &decoration_schedule).is_err());
            let order = batch
                .unambiguous_terrain_start_order(target)
                .unwrap()
                .unwrap();
            assert!(order.contains(&(structure_id.clone(), target)));
            assert!(
                batch
                    .beardifier_for_chunk(target, &order)
                    .unwrap()
                    .affected_bounds()
                    .is_some()
            );
            let plan = structure_batch::StructurePreparationPlan::new([(
                target,
                schedule::GenerationStage::StructureReferences,
            )])
            .unwrap();
            let mut added = batch.prepare_additional_stages(&plan, None).unwrap();
            assert_eq!(
                added.ocean_starts[&target][&structure_id].pieces.len(),
                piece_count
            );
            assert!(added.references[&target].membership()[&structure_id].contains(&packed));
            let mixed = added
                .mixed_decoration_plan(target, &decoration_schedule)
                .unwrap();
            let keys = added.mixed_commit_keys(&mixed, false).unwrap();
            let mut updated: Vec<Vec<_>> = mixed
                .iter()
                .map(|entry| {
                    entry
                        .starts
                        .iter()
                        .map(|start| match start {
                            structure_batch::PlannedStructureStart::Simple(start) => {
                                PreparedStructureStart::Simple((*start).clone())
                            }
                            structure_batch::PlannedStructureStart::OceanRuin(start) => {
                                PreparedStructureStart::OceanRuin((*start).clone())
                            }
                            structure_batch::PlannedStructureStart::Mineshaft(start) => {
                                PreparedStructureStart::Mineshaft((*start).clone())
                            }
                            structure_batch::PlannedStructureStart::Jigsaw(start) => {
                                PreparedStructureStart::Jigsaw((*start).clone())
                            }
                        })
                        .collect()
                })
                .collect();
            let PreparedStructureStart::OceanRuin(first) = &mut updated[0][0] else {
                panic!("missing ocean ruin")
            };
            first.pieces[0].selection.position[1] -= 12;
            first.pieces[0].bounds.min[1] -= 12;
            first.pieces[0].bounds.max[1] -= 12;
            let next_y = first.pieces[0].selection.position[1];
            drop(mixed);
            added.commit_mixed_starts(keys, updated, BTreeMap::new());
            let mixed = added
                .mixed_decoration_plan(target, &decoration_schedule)
                .unwrap();
            assert!(
                matches!(mixed[0].starts[0], structure_batch::PlannedStructureStart::OceanRuin(start) if start.pieces[0].selection.position[1] == next_y)
            );
            drop(mixed);
            let (_, transferred) = batch
                .into_terrain_stages(BTreeMap::from([(target, order)]))
                .unwrap();
            assert_eq!(
                transferred.ocean_starts[&target][&structure_id]
                    .pieces
                    .len(),
                piece_count
            );
            assert!(transferred.terrain[&target].affected_bounds().is_some());
            data.0.remove("ruin-template");
            assert!(
                generator
                    .try_generate_supported_structure_set(
                        &mut data,
                        &mut evaluator,
                        &mut cache,
                        &rules,
                        &set_id,
                        [0, 0],
                        |_| Ok(false),
                        classify,
                    )
                    .is_err()
            );
        }
        data.0.insert(path.into(), original);

        // Exercise the same generator through the actual stage planner. A
        // sparse set keeps this fixture fast while still producing a start.
        data.0.insert(
            "data/example/worldgen/structure_set/sparse.json".into(),
            serde_json::to_vec(&json!({"placement":{"type":"minecraft:random_spread","spacing":32,"separation":0,"salt":0},"structures":[{"structure":"example:pyramid","weight":1}]})).unwrap(),
        );
        let selected_generator =
            TerrainGenerator::with_structure_context(&mut data, &definition(), 42, 0, 16).unwrap();
        let automatic_generator =
            TerrainGenerator::with_structure_context(&mut data, &definition(), 42, 0, 16).unwrap();
        let automatic = structure_batch::StructureGenerationBatch::from_registry(
            automatic_generator,
            &mut data,
        )
        .unwrap();
        assert_eq!(
            automatic.structure_sets(),
            &[
                ResourceId::parse("example:pyramid").unwrap(),
                ResourceId::parse("example:sparse").unwrap()
            ]
        );
        let order = vec![
            ResourceId::parse("example:sparse").unwrap(),
            ResourceId::parse("example:pyramid").unwrap(),
        ];
        let selected = structure_batch::StructureGenerationBatch::from_registry_order(
            selected_generator,
            &mut data,
            order.clone(),
        )
        .unwrap();
        assert_eq!(selected.structure_sets(), order);
        let incomplete_generator =
            TerrainGenerator::with_structure_context(&mut data, &definition(), 42, 0, 16).unwrap();
        assert!(
            structure_batch::StructureGenerationBatch::from_registry_order(
                incomplete_generator,
                &mut data,
                vec![order[0].clone()]
            )
            .is_err()
        );
        let duplicate_generator =
            TerrainGenerator::with_structure_context(&mut data, &definition(), 42, 0, 16).unwrap();
        assert!(
            structure_batch::StructureGenerationBatch::from_registry_order(
                duplicate_generator,
                &mut data,
                vec![order[0].clone(), order[0].clone()]
            )
            .is_err()
        );
        let sparse = ResourceId::parse("example:sparse").unwrap();
        let [x, z] = cache
            .load(&mut data, &sparse)
            .unwrap()
            .placement
            .potential_chunk(42, [0, 0]);
        let target = ChunkPos { x, z };
        let mut neighborhood = generator.biome_neighborhood().unwrap();
        for dz in -1..=1 {
            for dx in -1..=1 {
                neighborhood
                    .insert(Arc::new(
                        generator
                            .generate_biomes([x + dx, z + dz], &mut evaluator)
                            .unwrap(),
                    ))
                    .unwrap();
            }
        }
        let mut batch =
            super::structure_batch::StructureGenerationBatch::new(generator, rules, vec![sparse])
                .unwrap();
        batch.request_references(target).unwrap();
        assert!(batch.references(target).is_none());
        let mut operations = 0;
        while batch
            .step(&mut data, &mut |v| {
                let id = v.as_str().unwrap();
                Ok(BlockHeightFlags {
                    is_air: id == "minecraft:air",
                    blocks_motion: id == "minecraft:stone",
                    blocks_motion_no_leaves: id == "minecraft:stone",
                    has_fluid: matches!(id, "minecraft:water" | "minecraft:lava"),
                })
            })
            .unwrap()
            .is_some()
        {
            operations += 1;
        }
        assert_eq!(operations, 290);
        assert!(batch.is_complete());
        assert!(batch.starts(target).unwrap().contains_key(&structure_id));
        let decoration_schedule =
            native_resources::structure_decoration::StructureDecorationSchedule::load(&mut data)
                .unwrap();
        let plan = batch.decoration_plan(target, &decoration_schedule).unwrap();
        assert_eq!(plan.len(), 1);
        assert_eq!(
            (&plan[0].entry.id, plan[0].entry.step, plan[0].entry.index),
            (&structure_id, 4, 1)
        );
        let source_order: Vec<_> = plan[0].starts.iter().map(|start| start.chunk).collect();
        let expected_source_order: Vec<_> = batch
            .reference_order(target, &structure_id)
            .unwrap()
            .iter()
            .map(|&packed| {
                let bits = packed as u64;
                [bits as u32 as i32, (bits >> 32) as u32 as i32]
            })
            .collect();
        assert_eq!(source_order, expected_source_order);
        let packed = ((z as u32 as u64) << 32 | x as u32 as u64) as i64;
        assert!(batch.references(target).unwrap()[&structure_id].contains(&packed));
        assert!(
            batch
                .reference_order(target, &structure_id)
                .unwrap()
                .contains(&packed)
        );
        assert_eq!(
            batch
                .reference_order(target, &structure_id)
                .unwrap()
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>(),
            batch.references(target).unwrap()[&structure_id],
        );
        let order: Vec<_> = batch
            .terrain_start_keys(target)
            .unwrap()
            .into_iter()
            .collect();
        assert!(order.contains(&(structure_id.clone(), target)));
        let java_order = batch
            .terrain_start_order(target, std::slice::from_ref(&structure_id))
            .unwrap();
        assert_eq!(
            batch.unambiguous_terrain_start_order(target).unwrap(),
            Some(java_order.clone())
        );
        assert_eq!(
            batch.unambiguous_terrain_orders([target]).unwrap()[&target],
            java_order
        );
        assert!(batch.unambiguous_terrain_orders([target, target]).is_err());
        assert_eq!(
            batch
                .terrain_start_order_with_identity_hashes(
                    target,
                    &BTreeMap::from([(structure_id.clone(), 1234)])
                )
                .unwrap(),
            java_order,
        );
        assert!(
            batch
                .terrain_start_order_with_identity_hashes(target, &BTreeMap::new())
                .is_err()
        );
        assert_eq!(
            java_order
                .iter()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            order.iter().cloned().collect()
        );
        assert!(batch.terrain_start_order(target, &[]).is_err());
        assert!(
            batch
                .terrain_start_order(target, &[structure_id.clone(), structure_id.clone()])
                .is_err()
        );
        assert!(batch.beardifier_for_chunk(target, &java_order).is_ok());
        assert!(batch.beardifier_for_chunk(target, &[]).is_err());
        let mut duplicates = order.clone();
        duplicates.push(order[0].clone());
        assert!(batch.beardifier_for_chunk(target, &duplicates).is_err());
        let context = batch.beardifier_for_chunk(target, &order).unwrap();
        assert!(context.sample(x * 16, 64, z * 16) > 0.0);
        let registry = ["air", "stone", "water", "lava", "dirt"]
            .into_iter()
            .map(|s| format!("minecraft:{s}"))
            .collect();
        let tags =
            native_resources::carver::tags::CarverBlockTags::load(&mut data, &registry).unwrap();
        let classify = |v: &Value| {
            Ok(CarverBlockState {
                is_air: v.as_str() == Some("minecraft:air"),
                has_fluid: matches!(v.as_str(), Some("minecraft:water" | "minecraft:lava")),
            })
        };
        let mut direct =
            TerrainGenerator::with_structure_context(&mut data, &definition(), 42, 0, 16).unwrap();
        let expected = direct
            .generate_carved_surface_with_structure_context(
                &mut data,
                &mut DensityEvaluator::default(),
                &mut CarverCache::default(),
                [x, z],
                &neighborhood,
                &tags,
                context,
                classify,
            )
            .unwrap();
        let actual = batch
            .generate_carved_surface(
                &mut data,
                target,
                &neighborhood,
                &tags,
                &mut CarverCache::default(),
                &order,
                classify,
            )
            .unwrap();
        assert_eq!(actual.states, expected.states);
        assert_eq!(actual.fluid_updates, expected.fluid_updates);

        // The regular generation schedule also requires starts around each
        // neighboring biome chunk, so prepare all nine reference stages.
        for dz in -1..=1 {
            for dx in -1..=1 {
                if dx != 0 || dz != 0 {
                    batch
                        .request_references(ChunkPos {
                            x: x + dx,
                            z: z + dz,
                        })
                        .unwrap();
                }
            }
        }
        while batch
            .step(&mut data, &mut |v| {
                let id = v.as_str().unwrap();
                Ok(BlockHeightFlags {
                    is_air: id == "minecraft:air",
                    blocks_motion: id == "minecraft:stone",
                    blocks_motion_no_leaves: id == "minecraft:stone",
                    has_fluid: matches!(id, "minecraft:water" | "minecraft:lava"),
                })
            })
            .unwrap()
            .is_some()
        {}
        let prepare_schedule = |batch: &super::structure_batch::StructureGenerationBatch,
                                neighborhood: &BiomeNeighborhood| {
            use super::schedule::{GenerationSchedule, GenerationStage};
            let mut schedule = GenerationSchedule::new();
            schedule.request(target, GenerationStage::Terrain).unwrap();
            loop {
                let (pos, stage) = schedule.next_ready().unwrap();
                if stage == GenerationStage::Terrain {
                    assert_eq!(pos, target);
                    break;
                }
                match stage {
                    GenerationStage::StructureStarts => assert!(batch.starts(pos).is_some()),
                    GenerationStage::StructureReferences => {
                        assert!(batch.references(pos).is_some())
                    }
                    GenerationStage::Biomes => {
                        assert!(neighborhood.chunk([pos.x, pos.z]).is_some())
                    }
                    _ => panic!("unexpected generation stage"),
                }
                schedule.complete(pos, stage).unwrap();
            }
            schedule
        };
        let mut schedule = prepare_schedule(&batch, &neighborhood);
        let mut assembled = None;
        schedule
            .run_terrain_with_structures(
                &mut batch,
                &mut data,
                &mut CarverCache::default(),
                &neighborhood,
                &tags,
                &mut assembled,
                target,
                "minecraft:overworld",
                &order,
                classify,
                |value| {
                    Ok(native_world::loaded::BlockState {
                        name: value.as_str().unwrap().into(),
                        properties: BTreeMap::new(),
                    })
                },
            )
            .unwrap();
        assert_eq!(
            schedule.completed(target),
            Some(super::schedule::GenerationStage::Terrain)
        );
        let snapshot = assembled.as_ref().unwrap().snapshot();
        for local_z in 0..16usize {
            for local_x in 0..16usize {
                for y in 0..16usize {
                    let block = snapshot
                        .block(x * 16 + local_x as i32, y as i32, z * 16 + local_z as i32)
                        .unwrap();
                    assert_eq!(
                        block.name,
                        actual.states[(local_z * 16 + local_x) * 16 + y]
                            .as_str()
                            .unwrap()
                    );
                }
            }
        }
        let mut failed_schedule = prepare_schedule(&batch, &neighborhood);
        let mut failed_world = None;
        assert!(
            failed_schedule
                .run_terrain_with_structures(
                    &mut batch,
                    &mut data,
                    &mut CarverCache::default(),
                    &neighborhood,
                    &tags,
                    &mut failed_world,
                    target,
                    "minecraft:overworld",
                    &order,
                    classify,
                    |_| anyhow::bail!("unknown block"),
                )
                .is_err()
        );
        assert!(failed_world.is_none());
        assert_eq!(
            failed_schedule.completed(target),
            Some(super::schedule::GenerationStage::Biomes)
        );
        assert!(batch.references(target).is_none());

        // Make the structure's initial Y=64 piece overlap generated terrain
        // and let a material rule depend on the beardifier contribution.
        let mut tall = definition();
        tall["settings"]["noise"]["height"] = json!(96);
        tall["settings"]["material_rule"] = json!({
            "type":"ore_vein", "density":{"type":"beardifier"}, "richness":1,
            "filler_gap":-1, "raw_ore_chance":1, "ore_block":"minecraft:dirt",
            "raw_ore_block":"minecraft:dirt", "filler_block":"minecraft:dirt"
        });
        let mut tall_generator =
            TerrainGenerator::with_structure_context(&mut data, &tall, 42, 0, 96).unwrap();
        let mut tall_neighborhood = tall_generator.biome_neighborhood().unwrap();
        for dz in -1..=1 {
            for dx in -1..=1 {
                tall_neighborhood
                    .insert(Arc::new(
                        tall_generator
                            .generate_biomes([x + dx, z + dz], &mut DensityEvaluator::default())
                            .unwrap(),
                    ))
                    .unwrap();
            }
        }
        let tall_rules = StructureBiomes::load(&mut data, [structure_id.clone()]).unwrap();
        let mut tall_batch = super::structure_batch::StructureGenerationBatch::new(
            tall_generator,
            tall_rules,
            vec![ResourceId::parse("example:sparse").unwrap()],
        )
        .unwrap();
        tall_batch.request_references(target).unwrap();
        while tall_batch
            .step(&mut data, &mut |v| {
                let id = v.as_str().unwrap();
                Ok(BlockHeightFlags {
                    is_air: id == "minecraft:air",
                    blocks_motion: id == "minecraft:stone",
                    blocks_motion_no_leaves: id == "minecraft:stone",
                    has_fluid: matches!(id, "minecraft:water" | "minecraft:lava"),
                })
            })
            .unwrap()
            .is_some()
        {}
        let tall_order: Vec<_> = tall_batch
            .terrain_start_keys(target)
            .unwrap()
            .into_iter()
            .collect();
        let with_structure = tall_batch
            .generate_carved_surface(
                &mut data,
                target,
                &tall_neighborhood,
                &tags,
                &mut CarverCache::default(),
                &tall_order,
                classify,
            )
            .unwrap();
        let mut without =
            TerrainGenerator::without_terrain_adjustments(&mut data, &tall, 42, 0, 96).unwrap();
        let without_structure = without
            .generate_carved_surface(
                &mut data,
                &mut DensityEvaluator::default(),
                &mut CarverCache::default(),
                [x, z],
                &tall_neighborhood,
                &tags,
                classify,
            )
            .unwrap();
        assert!(
            with_structure
                .states
                .iter()
                .zip(&without_structure.states)
                .any(|(a, b)| a != b),
            "generated structure must change terrain material"
        );
        for dz in -1..=1 {
            for dx in -1..=1 {
                if dx != 0 || dz != 0 {
                    tall_batch
                        .request_references(ChunkPos {
                            x: x + dx,
                            z: z + dz,
                        })
                        .unwrap();
                }
            }
        }
        while tall_batch
            .step(&mut data, &mut |v| {
                let id = v.as_str().unwrap();
                Ok(BlockHeightFlags {
                    is_air: id == "minecraft:air",
                    blocks_motion: id == "minecraft:stone",
                    blocks_motion_no_leaves: id == "minecraft:stone",
                    has_fluid: matches!(id, "minecraft:water" | "minecraft:lava"),
                })
            })
            .unwrap()
            .is_some()
        {}
        let mut tall_schedule = prepare_schedule(&tall_batch, &tall_neighborhood);
        let mut tall_world = None;
        tall_schedule
            .run_terrain_with_structures(
                &mut tall_batch,
                &mut data,
                &mut CarverCache::default(),
                &tall_neighborhood,
                &tags,
                &mut tall_world,
                target,
                "minecraft:overworld",
                &tall_order,
                classify,
                |value| {
                    Ok(native_world::loaded::BlockState {
                        name: value.as_str().unwrap().into(),
                        properties: BTreeMap::new(),
                    })
                },
            )
            .unwrap();
        let tall_snapshot = tall_world.unwrap().snapshot();
        let mut adjusted_blocks = 0;
        for local_z in 0..16usize {
            for local_x in 0..16usize {
                for y in 0..96usize {
                    let index = (local_z * 16 + local_x) * 96 + y;
                    let block = tall_snapshot
                        .block(x * 16 + local_x as i32, y as i32, z * 16 + local_z as i32)
                        .unwrap();
                    assert_eq!(block.name, with_structure.states[index].as_str().unwrap());
                    adjusted_blocks += usize::from(
                        with_structure.states[index] != without_structure.states[index],
                    );
                }
            }
        }
        assert!(adjusted_blocks > 0);

        // Transfer the actual structure generator and verified intermediate
        // stages into the same terrain batch used by the client.
        let fresh_tags =
            native_resources::carver::tags::CarverBlockTags::load(&mut data, &registry).unwrap();
        let mut fresh = FreshTerrainBatch::new_with_prepared_structures(
            tall_batch,
            BTreeMap::from([(target, tall_order)]),
            fresh_tags,
            "minecraft:overworld",
        )
        .unwrap();
        fresh.request(target).unwrap();
        while fresh
            .step(&mut data, &mut |value| classify(value), &mut |value| {
                Ok(native_world::loaded::BlockState {
                    name: value.as_str().unwrap().into(),
                    properties: BTreeMap::new(),
                })
            })
            .unwrap()
            .is_some()
        {}
        assert!(fresh.is_complete());
        let actual_world = fresh.snapshot().unwrap();
        for local_z in 0..16usize {
            for local_x in 0..16usize {
                for y in 0..96usize {
                    let wx = x * 16 + local_x as i32;
                    let wz = z * 16 + local_z as i32;
                    assert_eq!(
                        actual_world.block(wx, y as i32, wz),
                        tall_snapshot.block(wx, y as i32, wz)
                    );
                }
            }
        }
        assert!(fresh.request_features(target).is_err());
        assert!(fresh.into_parts().is_err());

        // A single reference stage does not cover the larger terrain plan.
        // Reject that request before recording any generation work.
        let generator =
            TerrainGenerator::with_structure_context(&mut data, &definition(), 42, 0, 16).unwrap();
        let rules = StructureBiomes::load(&mut data, [structure_id.clone()]).unwrap();
        let mut partial = super::structure_batch::StructureGenerationBatch::new(
            generator,
            rules,
            vec![ResourceId::parse("example:sparse").unwrap()],
        )
        .unwrap();
        partial.request_references(target).unwrap();
        while partial
            .step(&mut data, &mut |v| {
                let id = v.as_str().unwrap();
                Ok(BlockHeightFlags {
                    is_air: id == "minecraft:air",
                    blocks_motion: id == "minecraft:stone",
                    blocks_motion_no_leaves: id == "minecraft:stone",
                    has_fluid: matches!(id, "minecraft:water" | "minecraft:lava"),
                })
            })
            .unwrap()
            .is_some()
        {}
        let partial_order = partial
            .terrain_start_keys(target)
            .unwrap()
            .into_iter()
            .collect();
        let partial_tags =
            native_resources::carver::tags::CarverBlockTags::load(&mut data, &registry).unwrap();
        let mut partial_fresh = FreshTerrainBatch::new_with_prepared_structures(
            partial,
            BTreeMap::from([(target, partial_order)]),
            partial_tags,
            "minecraft:overworld",
        )
        .unwrap();
        assert!(partial_fresh.request(target).is_err());
        assert_eq!(partial_fresh.schedule().requested_chunks(), 0);

        data.0.insert(
            "data/example/worldgen/structure/pyramid.json".into(),
            serde_json::to_vec(
                &json!({"type":"minecraft:stronghold","biomes":["minecraft:plains"]}),
            )
            .unwrap(),
        );
        let generator =
            TerrainGenerator::with_structure_context(&mut data, &definition(), 42, 0, 16).unwrap();
        let rules = StructureBiomes::load(&mut data, [structure_id]).unwrap();
        let mut failing = super::structure_batch::StructureGenerationBatch::new(
            generator,
            rules,
            vec![ResourceId::parse("example:sparse").unwrap()],
        )
        .unwrap();
        failing.request_references(target).unwrap();
        let error = loop {
            match failing.step(&mut data, &mut |_| {
                panic!("unsupported structure must fail before classification")
            }) {
                Ok(Some(_)) => {}
                Ok(None) => panic!("unsupported structure was silently skipped"),
                Err(error) => break error,
            }
        };
        assert!(format!("{error:#}").contains("unsupported simple structure"));
        assert!(!failing.is_complete());
        assert!(failing.starts(target).is_none());
        assert!(failing.references(target).is_none());
        assert!(failing.request_references(target).is_err());
    }

    #[test]
    fn carving_uses_noise_height_not_larger_dimension_height() {
        let mut data = Data;
        let mut generator =
            TerrainGenerator::without_terrain_adjustments(&mut data, &definition(), 42, -16, 48)
                .unwrap();
        let mut evaluator = DensityEvaluator::default();
        let mut cache = CarverCache::default();
        let mask = generator
            .generate_carving_mask(&mut data, &mut evaluator, &mut cache, [0, 0])
            .unwrap();
        assert!(mask.is_empty());
        assert_eq!((mask.min_y(), mask.max_y()), (1, 8));
    }
    #[test]
    fn ring_search_shortcuts_preserve_random_results_for_fixed_biomes() {
        use native_resources::structure_placement::{ConcentricRingsPlacement, find_ring_biome};
        let mut data = Data;
        let generator =
            TerrainGenerator::without_terrain_adjustments(&mut data, &definition(), 42, 0, 16)
                .unwrap();
        for (biome, all) in [("minecraft:plains", true), ("minecraft:the_end", false)] {
            let placement=ConcentricRingsPlacement::decode(&json!({"type":"concentric_rings","distance":8,"spread":3,"count":5,"salt":0,"preferred_biomes":biome})).unwrap();
            let actual = generator
                .concentric_structure_positions(
                    &mut data,
                    &mut DensityEvaluator::default(),
                    &placement,
                )
                .unwrap();
            let expected = placement
                .positions_with_search(42, |initial, rng| {
                    find_ring_biome(initial, rng, |_| Ok(all))
                })
                .unwrap();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn fresh_terrain_connects_biome_carvers_aquifer_and_block_tag() {
        use native_resources::carver::tags::CarverBlockTags;
        use std::collections::BTreeSet;
        struct CarvingData;
        impl ResourceProvider for CarvingData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                Ok(if path == "data/minecraft/worldgen/biome/plains.json" {
                    Some(br#"{"has_precipitation":true,"temperature":0.8,"carvers":["minecraft:test"]}"#.to_vec())
                } else if path == "data/minecraft/worldgen/carver/test.json" {
                    Some(br#"{"type":"minecraft:cave","probability":1,"y":{"absolute":4},"count":1,"thickness":2,"room_vertical_radius_multiplier":1,"horizontal_radius_multiplier":1,"vertical_radius_multiplier":1,"floor_level":-1}"#.to_vec())
                } else if path == "data/minecraft/tags/block/uncarvable.json" {
                    Some(br#"{"values":["minecraft:bedrock"]}"#.to_vec())
                } else if path.starts_with("data/minecraft/worldgen/noise/") {
                    Some(br#"{"base_octave":-4,"octave_count":2}"#.to_vec())
                } else {
                    None
                })
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                ensure!(folder == "worldgen/biome", "unexpected registry");
                Ok(vec![ResourceId::parse("minecraft:plains")?])
            }
        }
        let mut data = CarvingData;
        let registry: BTreeSet<_> = ["air", "bedrock", "stone", "dirt", "water", "lava"]
            .into_iter()
            .map(|v| format!("minecraft:{v}"))
            .collect();
        let tags = CarverBlockTags::load(&mut data, &registry).unwrap();
        let mut generator =
            TerrainGenerator::without_terrain_adjustments(&mut data, &definition(), 42, 0, 16)
                .unwrap();
        let mut evaluator = DensityEvaluator::default();
        let mut neighborhood = generator.biome_neighborhood().unwrap();
        for x in -1..=1 {
            for z in -1..=1 {
                neighborhood
                    .insert(Arc::new(
                        generator.generate_biomes([x, z], &mut evaluator).unwrap(),
                    ))
                    .unwrap();
            }
        }
        let mut cache = CarverCache::default();
        let expected = generator
            .generate_carving_mask(&mut data, &mut evaluator, &mut cache, [0, 0])
            .unwrap();
        assert!(!expected.is_empty());
        let generated = generator
            .generate_carved_surface(
                &mut data,
                &mut evaluator,
                &mut cache,
                [0, 0],
                &neighborhood,
                &tags,
                |state| {
                    let id = state.as_str().unwrap_or("");
                    Ok(CarverBlockState {
                        is_air: id == "minecraft:air",
                        has_fluid: id == "minecraft:water" || id == "minecraft:lava",
                    })
                },
            )
            .unwrap();
        for x in 0..16u8 {
            for z in 0..16u8 {
                for y in 0..16i32 {
                    let index = (usize::from(z) * 16 + usize::from(x)) * 16 + y as usize;
                    let block = generated.states[index].as_str().unwrap();
                    let wanted = if expected.contains(x, y, z) {
                        if y < 8 {
                            "minecraft:water"
                        } else {
                            "minecraft:air"
                        }
                    } else {
                        "minecraft:dirt"
                    };
                    assert_eq!(block, wanted, "({x},{y},{z})");
                    assert!(!generated.fluid_updates[index]);
                }
            }
        }
    }
}
