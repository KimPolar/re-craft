//! Intermediate terrain, selected features and lighting for fresh worlds.
//! Full structure and feature coverage, persistence and playable world
//! integration are not yet implemented.
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
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
mod generated_beehive;
mod generated_chest;
mod generated_spawner;
mod ore;
pub mod postprocessing;
pub mod schedule;
mod terrain_batch;
pub use carving::CarverBlockState;
pub use generated_beehive::GeneratedBeeNest;
pub use generated_chest::GeneratedLootChest;
pub use generated_spawner::GeneratedMonsterSpawner;
pub use ore::{
    DecorationRegion, PlacedDecorationFeature, PreparedDecoration, ScheduledDecorationFeature,
};
pub use terrain_batch::{FreshBatchLight, FreshTerrainBatch};

/// Block entities created by a monster-room feature in intermediate terrain.
#[derive(Clone, Debug)]
pub enum GeneratedMonsterRoomEntity {
    Chest(GeneratedLootChest),
    Spawner(GeneratedMonsterSpawner),
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
}

/// Registration history for a newly created generation light engine. The
/// scheduler must route every INITIALIZE_LIGHT registration through this
/// tracker and stop using it once other light-engine work has been scheduled.
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
            ensure!(
                world
                    .chunks
                    .get(pos)
                    .is_some_and(|chunk| Arc::ptr_eq(chunk, original)),
                "registered light chunk has changed"
            );
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

    /// Return source heights only while the registered generation data remains
    /// unchanged. The array uses X + 16 * Z within the chunk.
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
        .context("active generated block-light inputs are incomplete or unsupported")
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

    /// Capture one immutable engine revision. Any later registered-chunk edit
    /// invalidates this tracker before another snapshot or registration.
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

/// A frozen batch of initialized generation chunks with monotonically
/// activated light sources. This is separate from the live world: it cannot
/// accept section updates, registered-chunk edits, restored light or new registrations.
/// Every initialized chunk that affects the batch must be registered before
/// the tracker is moved here. Requested columns need their 3x3 neighborhood;
/// uninitialized chunks must retain their default sky-source heightmaps.
/// Once propagation starts, reads use the blocks seen at activation. Further
/// activation requires its 3x3 input neighborhood to retain equivalent light
/// behavior; registered chunks must remain completely unchanged.
pub struct StagedGenerationLightEngine {
    fresh: FreshGenerationLightTracker,
    active: BTreeSet<native_world::loaded::ChunkPos>,
    frozen_world: Option<Arc<native_world::loaded::World>>,
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
        }
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
        self.initialize_generated_lighting(center, settings.has_skylight)?;
        if self.world.dimension_type != name {
            Arc::make_mut(&mut self.world).dimension_type = name;
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

    /// Known monster-room entities; these chunks are not yet ready to save.
    pub fn generated_monster_entities(&self) -> impl Iterator<Item = &GeneratedMonsterRoomEntity> {
        self.generated_monster_entities
            .values()
            .filter_map(Option::as_ref)
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
}

/// A generator configured from the dimension's active biome source and noise
/// settings. It must be used with the same data-pack stack throughout its life.
pub struct TerrainGenerator {
    noise: NoiseStageGenerator,
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
            noise,
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
        mut classify: impl FnMut(&Value) -> Result<BlockHeightFlags>,
    ) -> Result<native_resources::structure_placement::StructureSetStart<SimpleStructureStart>>
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
                let structure = SimpleStructure::decode(&definition)
                    .with_context(|| format!("in structure {path}"))?;
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
                structure.generate(&mut env, self.seed, chunk)
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
                if path == "data/minecraft/worldgen/biome/plains.json" {
                    let mushroom = if self.brown {
                        "minecraft:brown_mushroom"
                    } else {
                        "minecraft:red_mushroom"
                    };
                    let mut features = vec![
                        json!({"feature":{"type":"minecraft:simple_block","to_place":{"id":mushroom}},"placement":[{"type":"minecraft:offset","x":8,"y":8,"z":8}]}),
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
                Data.read_data(path)
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
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
                min_y: 0,
                height: 16,
                has_skylight: true,
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
            batch.request_light_initialization(center).unwrap();
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
        let generator =
            TerrainGenerator::without_terrain_adjustments(&mut data, &definition(), 42, 0, 16)
                .unwrap();
        let mut batch =
            FreshTerrainBatch::new_without_structures(generator, tags, "minecraft:overworld")
                .unwrap();
        let lighting = GeneratedDimensionLighting {
            dimension: ResourceId::parse("minecraft:overworld").unwrap(),
            dimension_type: None,
            min_y: 0,
            height: 16,
            has_skylight: false,
        };
        batch
            .enable_decoration(&mut data, &BTreeSet::new(), block_registry, &lighting)
            .unwrap();
        batch
            .enable_light_initialization(generated_registry)
            .unwrap();
        let center = ChunkPos { x: 0, z: 0 };
        let east = ChunkPos { x: 1, z: 0 };
        for z in -1..=1 {
            for x in -1..=1 {
                batch
                    .request_light_initialization(ChunkPos { x, z })
                    .unwrap();
            }
        }
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
        let (schedule, _, terrain, light) = batch.into_parts().unwrap();
        assert_eq!(
            schedule.completed(center),
            Some(schedule::GenerationStage::Light)
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
            let lighting = GeneratedDimensionLighting {
                dimension: ResourceId::parse("minecraft:overworld").unwrap(),
                dimension_type: Some(ResourceId::parse("example:custom_type").unwrap()),
                min_y: 0,
                height: 16,
                has_skylight: has_sky,
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
    fn simple_structure_set_uses_base_noise_and_fixed_biome() {
        use native_core::beardifier::Beardifier;
        use native_resources::{
            structure_biomes::StructureBiomes,
            structure_placement::{StructureSetCache, StructureSetStart},
        };
        struct StructureData(BTreeMap<String, Vec<u8>>);
        impl ResourceProvider for StructureData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if let Some(value) = self.0.get(path) {
                    Ok(Some(value.clone()))
                } else {
                    Data.read_data(path)
                }
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                Data.list_data(folder)
            }
        }
        let mut data=StructureData(BTreeMap::from([
            ("data/example/worldgen/structure/pyramid.json".into(),serde_json::to_vec(&json!({"type":"minecraft:desert_pyramid","biomes":["minecraft:plains"],"terrain_adaptation":"bury"})).unwrap()),
            ("data/example/worldgen/structure_set/pyramid.json".into(),serde_json::to_vec(&json!({"placement":{"type":"minecraft:random_spread","spacing":1,"separation":0,"salt":0},"structures":[{"structure":"example:pyramid","weight":1}]})).unwrap()),
        ]));
        let generator =
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
