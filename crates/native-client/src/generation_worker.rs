//! Prepare a fresh-generation batch away from the window thread.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
};

use anyhow::{Context, Result, ensure};
use glam::IVec3;
use native_resources::packs::ResourceProvider;
use native_session::worldgen::{CarverBlockState, FreshTerrainBatch, schedule::GenerationStage};
use native_world::loaded::{BlockState, ChunkPos};
use serde_json::Value;

use crate::mesh_worker::MeshJob;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GenerationPhase {
    Planning,
    Generating,
    Lighting,
    Spawning,
    PreparingMesh,
    SearchingSpawn,
    Ready,
    Cancelled,
    Failed,
}

#[derive(Clone, Copy, Debug)]
pub struct GenerationProgress {
    pub phase: GenerationPhase,
    pub completed_steps: u64,
    pub last_step: Option<(ChunkPos, GenerationStage)>,
    /// Number of times an initial spawn search resumed after chunk expansion.
    pub spawn_search_resumptions: u64,
}

pub struct GenerationReady {
    pub batch: FreshTerrainBatch,
    pub mesh: MeshJob,
    /// A same-pack generation-settings draft; not yet validated for saving.
    pub worldgen_settings_draft: Option<native_persistence::nbt::NamedTag>,
    /// Present when the worker chose its target from the generator's spawn origin.
    pub spawn_origin: Option<ChunkPos>,
    /// Initial world-spawn suggestion, not the collision-checked player spawn.
    pub suggested_spawn: Option<IVec3>,
    /// Starting clock ticks from the same pack stack, when prepared for a new world.
    pub initial_clocks: Option<Arc<crate::fresh_generation::InitialWorldClocks>>,
    pub entity_collision_registry:
        Option<Arc<native_resources::entity_collision::EntityCollisionRegistry>>,
    pub level_settings: Option<crate::world_creation::FrozenLevelSettings>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitialSpawnProgress {
    Pending,
    NeedsChunk(ChunkPos),
    Found(IVec3),
}

impl GenerationReady {
    /// Prepare the separate player-spawn search using explicitly supplied
    /// entity data. Missing entity chunks remain unknown rather than empty.
    pub fn start_player_spawn_search(
        &mut self,
        player: native_session::PlayerState,
        tags: Arc<native_resources::heightmap_tags::HeightmapTags>,
        entities: impl IntoIterator<Item = (ChunkPos, native_session::loaded::SavedEntityData)>,
        candidates: Option<native_session::world_spawn::WorldSpawnCandidates>,
    ) -> Result<(
        native_session::loaded::LoadedSession,
        native_session::world_spawn::ResumableWorldSpawn,
    )> {
        let suggestion = self
            .suggested_spawn
            .context("world spawn suggestion missing")?;
        let search = native_session::world_spawn::ResumableWorldSpawn::new(suggestion, candidates)?;
        let mut session = self.start_session_with_spawn_heightmaps(player, tags)?;
        for (pos, data) in entities {
            session.set_initial_entity_chunk(pos, data)?;
        }
        Ok((session, search))
    }

    pub fn poll_player_spawn(
        &self,
        session: &native_session::loaded::LoadedSession,
        search: &mut native_session::world_spawn::ResumableWorldSpawn,
        ceiling_spawn_height: Option<i32>,
        budget: usize,
    ) -> Result<native_session::loaded::GeneratedPlayerSpawnProgress> {
        let targets = self
            .mesh
            .targets
            .as_deref()
            .context("generated session targets missing")?;
        session.poll_generated_player_spawn(search, ceiling_spawn_height, targets, budget)
    }

    /// Poll within the currently available chunk so reaching the next chunk
    /// requests generation instead of treating absent terrain as an error.
    /// Create a new search and session after expanding the batch.
    pub fn poll_initial_spawn(
        &self,
        session: &native_session::loaded::LoadedSession,
        search: &mut native_session::world_spawn::InitialWorldSpawn,
        ceiling_spawn_height: Option<i32>,
        budget: usize,
    ) -> Result<InitialSpawnProgress> {
        let targets = self
            .mesh
            .targets
            .as_deref()
            .context("generated session targets missing")?;
        if let Some(found) =
            session.poll_generated_initial_spawn(search, ceiling_spawn_height, targets, 0)?
        {
            return Ok(InitialSpawnProgress::Found(found));
        }
        let pos = search.requested_position();
        let chunk = ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        };
        if !targets.contains(&chunk) {
            return Ok(InitialSpawnProgress::NeedsChunk(chunk));
        }
        let column = pos.x.rem_euclid(16) as usize * 16 + pos.z.rem_euclid(16) as usize;
        if let Some(found) = session.poll_generated_initial_spawn(
            search,
            ceiling_spawn_height,
            targets,
            budget.min(256 - column),
        )? {
            return Ok(InitialSpawnProgress::Found(found));
        }
        let pos = search.requested_position();
        let next = ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        };
        Ok(if targets.contains(&next) {
            InitialSpawnProgress::Pending
        } else {
            InitialSpawnProgress::NeedsChunk(next)
        })
    }

    pub fn initial_spawn_search_at_origin(
        &self,
        session: &native_session::loaded::LoadedSession,
    ) -> Result<native_session::world_spawn::InitialWorldSpawn> {
        self.initial_spawn_search(
            session,
            self.spawn_origin
                .context("worker has no computed spawn origin")?,
        )
    }

    /// Start the initial spawn suggestion search at the origin calculated
    /// from this batch's unchanged generator and data-pack stack.
    pub fn initial_spawn_search(
        &self,
        session: &native_session::loaded::LoadedSession,
        origin: ChunkPos,
    ) -> Result<native_session::world_spawn::InitialWorldSpawn> {
        let targets = self
            .mesh
            .targets
            .as_ref()
            .context("generated session targets missing")?;
        session.generated_initial_spawn_search(
            origin,
            self.batch.spawn_height(),
            self.batch.sea_level(),
            targets,
        )
    }

    /// Prepare spawn heightmaps with tags from the same immutable data-pack
    /// stack as this batch. Already primed generation values are preserved.
    pub fn start_session_with_spawn_heightmaps(
        &mut self,
        player: native_session::PlayerState,
        tags: Arc<native_resources::heightmap_tags::HeightmapTags>,
    ) -> Result<native_session::loaded::LoadedSession> {
        let mut session = self.start_session(player)?;
        session.set_spawn_heightmap_tags(tags)?;
        let targets = self
            .mesh
            .targets
            .as_ref()
            .context("generated session targets missing")?;
        for &pos in targets.iter() {
            if let Some(maps) = self.batch.spawn_heightmaps(pos)? {
                session.complete_generated_spawn_heightmaps(pos, maps)?;
            } else {
                session.prime_generated_spawn_heightmaps(pos)?;
            }
        }
        Ok(session)
    }

    /// Construct a transient session from fully lit targets. The caller must
    /// choose and validate the player spawn, and keep the batch for generation;
    /// this does not create or save a Java world.
    pub fn start_session(
        &mut self,
        player: native_session::PlayerState,
    ) -> Result<native_session::loaded::LoadedSession> {
        use native_session::{
            inventory::PlayerInventory,
            loaded::{BuildBounds, LoadedSession},
        };
        let targets = self
            .mesh
            .targets
            .as_ref()
            .context("generated session requires light-complete targets")?;
        ensure!(!targets.is_empty(), "generated session has no targets");
        ensure!(
            self.batch.is_complete(),
            "generated session has pending generation work"
        );
        let current = self.batch.lit_render_snapshot(targets.iter().copied())?;
        ensure!(
            Arc::ptr_eq(&current, &self.mesh.world),
            "generated session mesh snapshot is stale"
        );
        let (min_y, max_y) = self.batch.build_bounds()?;
        ensure!(
            player.position.is_finite(),
            "invalid generated player position"
        );
        let absolute = player.position.as_dvec3() + self.mesh.origin.as_dvec3();
        ensure!(
            absolute.x >= f64::from(i32::MIN)
                && absolute.x < f64::from(i32::MAX)
                && absolute.z >= f64::from(i32::MIN)
                && absolute.z < f64::from(i32::MAX)
                && absolute.y >= f64::from(min_y)
                && absolute.y <= f64::from(max_y),
            "generated player position is outside supported bounds"
        );
        let player_chunk = ChunkPos {
            x: (absolute.x.floor() as i32).div_euclid(16),
            z: (absolute.z.floor() as i32).div_euclid(16),
        };
        ensure!(
            targets.contains(&player_chunk),
            "generated player is outside light-complete targets"
        );
        let mut world = generated_session_world(&self.mesh.world, targets);
        ensure!(
            world.chunks.len() == targets.len(),
            "generated session target is missing"
        );
        for chunk in world.chunks.values_mut() {
            for sy in min_y.div_euclid(16) - 1..=max_y.div_euclid(16) {
                let section = chunk
                    .sections
                    .get(&i8::try_from(sy)?)
                    .context("generated session light section is missing")?;
                ensure!(
                    section.block_light.is_some() && section.sky_light.is_some(),
                    "generated session light data is missing"
                );
                if sy >= min_y.div_euclid(16) && sy < max_y.div_euclid(16) {
                    ensure!(
                        section.blocks.is_some(),
                        "generated session blocks are missing"
                    );
                }
            }
            Arc::make_mut(chunk).block_light_correct = true;
        }
        for &pos in targets.iter() {
            let stage = self.batch.schedule().completed(pos);
            let payload = generated_session_block_entities(stage, || {
                self.batch.spawned_block_entity_data(pos)
            });
            world.saved_block_entities.insert(pos, payload);
        }
        let mut session = LoadedSession::new(
            world,
            self.mesh.origin,
            BuildBounds { min_y, max_y },
            player,
        )
        .map_err(anyhow::Error::msg)?;
        for &pos in targets.iter() {
            if self
                .batch
                .schedule()
                .completed(pos)
                .is_some_and(|stage| stage >= GenerationStage::Spawn)
            {
                if let Some(records) = self.batch.complete_generated_entity_records(pos)? {
                    session
                        .set_initial_entity_chunk(pos, generated_session_entities(pos, records))?;
                }
            }
        }
        session.set_player_inventory(PlayerInventory::default());
        if let Some(registry) = &self.entity_collision_registry {
            session.set_entity_collision_registry(Arc::clone(registry))?;
        }
        if let Some(level) = &self.level_settings {
            let settings = level.session;
            session.set_game_mode(settings.mode);
            session.set_game_master_permission(settings.allow_commands);
            session.set_world_seed(Some(settings.seed));
            session.set_survival_rules(native_session::survival::SurvivalRules {
                difficulty: settings.difficulty,
                ..Default::default()
            });
            session.set_player_vitals(native_session::survival::PlayerVitals {
                hardcore: settings.hardcore,
                ..Default::default()
            });
        }
        if let Some(initial) = &self.initial_clocks {
            use native_persistence::world::{DimensionId, SavedClock};
            let clocks = initial
                .ticks
                .iter()
                .map(|(id, &total_ticks)| {
                    let id = DimensionId::parse(&format!("{}:{}", id.namespace, id.path))?;
                    Ok((
                        id,
                        SavedClock {
                            total_ticks,
                            ..SavedClock::default()
                        },
                    ))
                })
                .collect::<Result<BTreeMap<_, _>>>()?;
            session.set_registered_clocks(Arc::new(clocks.keys().cloned().collect()));
            session.set_world_clocks(clocks, initial.advance_time);
            if let Some((dimension_type, clock)) = &initial.command_data.default_clock {
                let clock = clock.as_ref().map(|id| DimensionId {
                    namespace: id.namespace.clone(),
                    path: id.path.clone(),
                });
                session.set_default_clock(Arc::clone(dimension_type), clock);
            }
            if let Some(timelines) = &initial.command_data.timelines {
                session.set_timelines(Arc::clone(timelines));
            }
            if let Some(markers) = &initial.command_data.markers {
                session.set_time_markers(Arc::clone(markers));
            }
        }
        Ok(session)
    }
}

/// Build a transient session input; this is not a complete entity-region
/// document and must not be used to save a Java world.
fn generated_session_entities(
    pos: ChunkPos,
    records: &[native_persistence::nbt::Tag],
) -> native_session::loaded::SavedEntityData {
    use native_persistence::{
        nbt::{NamedTag, Tag, TagType},
        world::{ChunkPos as SavedChunkPos, SavedEntityChunk},
    };
    use native_session::loaded::SavedEntityData;

    if records.is_empty() {
        return SavedEntityData::Empty;
    }
    SavedEntityData::Present(Arc::new(SavedEntityChunk {
        position: SavedChunkPos { x: pos.x, z: pos.z },
        document: Arc::new(NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "Entities".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: records.to_vec(),
                },
            )]),
        }),
    }))
}

/// A lit protochunk may still change during generation. Only publish block
/// entities after SPAWN and after the batch verifies all block/entity pairs.
fn generated_session_block_entities(
    stage: Option<GenerationStage>,
    load: impl FnOnce() -> Result<native_persistence::nbt::Tag>,
) -> native_world::loaded::SavedBlockEntities {
    use native_persistence::nbt::{self, NamedTag, Tag, TagType};
    use native_world::loaded::SavedBlockEntities;

    if !stage.is_some_and(|stage| stage >= GenerationStage::Spawn) {
        return SavedBlockEntities::Unavailable;
    }
    match load() {
        Ok(Tag::List { values, .. }) if values.is_empty() => SavedBlockEntities::Empty,
        Ok(
            tag @ Tag::List {
                element_type: TagType::Compound,
                ..
            },
        ) => {
            match nbt::write(&NamedTag {
                name: "".into(),
                value: tag,
            }) {
                Ok(bytes) => SavedBlockEntities::Encoded(bytes.into()),
                Err(_) => SavedBlockEntities::Unavailable,
            }
        }
        _ => SavedBlockEntities::Unavailable,
    }
}

/// A generated light snapshot includes neighboring chunks. Its transient
/// session must not import their ticks or block-entity data before streaming.
fn generated_session_world(
    source: &native_world::loaded::World,
    targets: &BTreeSet<ChunkPos>,
) -> native_world::loaded::World {
    let mut world = source.clone();
    world.chunks.retain(|pos, _| targets.contains(pos));
    world.missing_chunks.retain(|pos| targets.contains(pos));
    world
        .saved_block_ticks
        .retain(|pos, _| targets.contains(pos));
    world
        .saved_fluid_ticks
        .retain(|pos, _| targets.contains(pos));
    world
        .saved_block_entities
        .retain(|pos, _| targets.contains(pos));
    world
        .live_block_entities
        .retain(|pos, _| targets.contains(pos));
    world
}

pub enum GenerationResult {
    Ready(GenerationReady),
    Cancelled,
    Failed(anyhow::Error),
}

pub struct GenerationWorker {
    cancelled: Arc<AtomicBool>,
    progress: Arc<Mutex<GenerationProgress>>,
    result: Receiver<GenerationResult>,
}

enum GenerationTargets {
    Explicit {
        targets: BTreeSet<ChunkPos>,
        origin: IVec3,
    },
    SpawnOrigin,
}

impl GenerationWorker {
    /// Generate and select the initial spawn, then process creature spawning
    /// without reopening or changing the selected world data-pack stack.
    /// This produces intermediate chunks; it does not save or start play.
    pub fn start_initial_world<F>(
        prepare: F,
        revision: u64,
        biome_blend: u8,
        ceiling_spawn_height: Option<i32>,
    ) -> Result<Self>
    where
        F: FnOnce() -> Result<crate::fresh_generation::PreparedInitialWorld> + Send + 'static,
    {
        Self::spawn(move |cancelled, progress| {
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let crate::fresh_generation::PreparedInitialWorld {
                batch,
                mut resources,
                mut classify,
                mut resolve,
                height_tags,
                worldgen_settings,
                creatures,
                spawn_mobs,
                initial_clocks,
                entity_collision_registry,
                level_settings,
                ..
            } = prepare()?;
            let Some(mut ready) = generate_spawn_origin(
                batch,
                &mut resources,
                &mut classify,
                &mut resolve,
                revision,
                biome_blend,
                cancelled,
                progress,
            )?
            else {
                return Ok(None);
            };
            ready.worldgen_settings_draft = Some(worldgen_settings);
            ready.initial_clocks = Some(initial_clocks);
            ready.entity_collision_registry = entity_collision_registry;
            ready.level_settings = level_settings;
            let Some(mut ready) = complete_spawn_search(
                ready,
                &mut resources,
                &mut classify,
                &mut resolve,
                height_tags,
                biome_blend,
                ceiling_spawn_height,
                cancelled,
                progress,
            )?
            else {
                return Ok(None);
            };
            let targets = checked_spawn_targets(&mut ready)?;
            let mut build = |batch: &FreshTerrainBatch,
                             resources: &mut native_resources::packs::ResourceStack,
                             pos| {
                creatures
                    .as_ref()
                    .context("creature spawn inputs are unavailable")?
                    .build_spawner(batch, resources, pos)
            };
            let mut construct = |id: &native_resources::packs::ResourceId| {
                Ok(native_session::worldgen::FreshCreatureRandom::from_entropy(id)?.construction)
            };
            if !complete_creature_spawning_with_inputs(
                &mut ready,
                &targets,
                &mut resources,
                &mut classify,
                &mut resolve,
                &mut build,
                &mut construct,
                spawn_mobs,
                cancelled,
                progress,
            )? {
                return Ok(None);
            }
            Ok(Some(ready))
        })
    }

    /// Use the active data-pack-derived spawn configuration and independent
    /// entity RNGs on the same worker as the chunk spawn operation.
    pub fn complete_creature_spawning_from_packs<C, R, F>(
        ready: GenerationReady,
        prepare: F,
    ) -> Result<Self>
    where
        F: FnOnce() -> Result<(
                native_resources::packs::ResourceStack,
                C,
                R,
                crate::fresh_generation::PreparedCreatureSpawning,
            )> + Send
            + 'static,
        C: FnMut(&Value) -> Result<CarverBlockState>,
        R: FnMut(&Value) -> Result<BlockState>,
    {
        Self::complete_creature_spawning(ready, move || {
            let (resources, classify, resolve, spawn) = prepare()?;
            let build =
                move |batch: &FreshTerrainBatch,
                      resources: &mut native_resources::packs::ResourceStack,
                      pos| spawn.build_spawner(batch, resources, pos);
            let construct = |id: &native_resources::packs::ResourceId| {
                Ok(native_session::worldgen::FreshCreatureRandom::from_entropy(id)?.construction)
            };
            Ok((resources, classify, resolve, build, construct))
        })
    }

    /// Complete generation-time creature spawning for the mesh targets on a
    /// worker. The supplied builder must use this batch's unchanged packs,
    /// dimension, clocks and tags. No chunk is marked SPAWN until its driver
    /// finishes; cancellation discards the owned batch and all staged records.
    pub fn complete_creature_spawning<P, C, R, M, K, F>(
        mut ready: GenerationReady,
        prepare: F,
    ) -> Result<Self>
    where
        F: FnOnce() -> Result<(P, C, R, M, K)> + Send + 'static,
        P: ResourceProvider,
        C: FnMut(&Value) -> Result<CarverBlockState>,
        R: FnMut(&Value) -> Result<BlockState>,
        M: FnMut(
            &FreshTerrainBatch,
            &mut P,
            ChunkPos,
        ) -> Result<native_session::worldgen::GeneratedSheepSpawner>,
        K: FnMut(
            &native_resources::packs::ResourceId,
        ) -> Result<native_session::worldgen::FreshCreatureConstruction>,
    {
        Self::spawn(move |cancelled, progress| {
            let targets = checked_spawn_targets(&mut ready)?;
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let (mut provider, mut classify, mut resolve, mut build_spawner, mut construct) =
                prepare()?;
            if !complete_creature_spawning_with_inputs(
                &mut ready,
                &targets,
                &mut provider,
                &mut classify,
                &mut resolve,
                &mut build_spawner,
                &mut construct,
                true,
                cancelled,
                progress,
            )? {
                return Ok(None);
            }
            Ok(Some(ready))
        })
    }

    /// Construct the batch and its resources on the worker thread. They must
    /// use the same immutable pack stack and have decoration/light configured.
    /// This finite batch does not save or start play.
    pub fn start<P, C, R, F>(
        prepare: F,
        targets: BTreeSet<ChunkPos>,
        revision: u64,
        biome_blend: u8,
        origin: IVec3,
    ) -> Result<Self>
    where
        F: FnOnce() -> Result<(FreshTerrainBatch, P, C, R)> + Send + 'static,
        P: ResourceProvider,
        C: FnMut(&Value) -> Result<CarverBlockState>,
        R: FnMut(&Value) -> Result<BlockState>,
    {
        Self::start_with_targets(
            prepare,
            GenerationTargets::Explicit { targets, origin },
            revision,
            biome_blend,
        )
    }

    /// Compute the generator's spawn origin on the worker and generate its
    /// first LIGHT-complete chunk. More chunks can be required to find spawn.
    pub fn start_at_spawn<P, C, R, F>(prepare: F, revision: u64, biome_blend: u8) -> Result<Self>
    where
        F: FnOnce() -> Result<(FreshTerrainBatch, P, C, R)> + Send + 'static,
        P: ResourceProvider,
        C: FnMut(&Value) -> Result<CarverBlockState>,
        R: FnMut(&Value) -> Result<BlockState>,
    {
        Self::start_with_targets(
            prepare,
            GenerationTargets::SpawnOrigin,
            revision,
            biome_blend,
        )
    }

    /// Select the initial world-spawn suggestion on a background thread,
    /// generating further chunks as needed with the same resource provider.
    /// The returned suggestion still requires the separate player spawn and
    /// entity collision checks before it can be used to start play.
    pub fn start_spawn_search<P, C, R, F>(
        prepare: F,
        revision: u64,
        biome_blend: u8,
        ceiling_spawn_height: Option<i32>,
    ) -> Result<Self>
    where
        F: FnOnce() -> Result<(
                FreshTerrainBatch,
                P,
                C,
                R,
                Arc<native_resources::heightmap_tags::HeightmapTags>,
                native_persistence::nbt::NamedTag,
            )> + Send
            + 'static,
        P: ResourceProvider,
        C: FnMut(&Value) -> Result<CarverBlockState>,
        R: FnMut(&Value) -> Result<BlockState>,
    {
        Self::spawn(move |cancelled, progress| {
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let (batch, mut provider, mut classify, mut resolve, tags, worldgen_settings) =
                prepare()?;
            let Some(mut ready) = generate_spawn_origin(
                batch,
                &mut provider,
                &mut classify,
                &mut resolve,
                revision,
                biome_blend,
                cancelled,
                progress,
            )?
            else {
                return Ok(None);
            };
            ready.worldgen_settings_draft = Some(worldgen_settings);
            complete_spawn_search(
                ready,
                &mut provider,
                &mut classify,
                &mut resolve,
                tags,
                biome_blend,
                ceiling_spawn_height,
                cancelled,
                progress,
            )
        })
    }

    /// Extend a completed batch with one more light-complete target. The
    /// resource provider and resolvers must use the same immutable data-pack
    /// stack as the original batch. Restart any spawn search after expansion:
    /// newly generated features can change previously inspected columns.
    pub fn expand<P, C, R, F>(
        ready: GenerationReady,
        target: ChunkPos,
        prepare: F,
        revision: u64,
        biome_blend: u8,
    ) -> Result<Self>
    where
        F: FnOnce() -> Result<(P, C, R)> + Send + 'static,
        P: ResourceProvider,
        C: FnMut(&Value) -> Result<CarverBlockState>,
        R: FnMut(&Value) -> Result<BlockState>,
    {
        Self::spawn(move |cancelled, progress| {
            ensure!(
                revision != ready.mesh.revision,
                "expanded mesh needs a new revision"
            );
            let targets = ready
                .mesh
                .targets
                .as_deref()
                .context("generated session targets missing")?;
            ensure!(
                !targets.contains(&target),
                "generation target is already ready"
            );
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let (mut provider, mut classify, mut resolve) = prepare()?;
            expand_with_inputs(
                ready,
                target,
                &mut provider,
                &mut classify,
                &mut resolve,
                revision,
                biome_blend,
                cancelled,
                progress,
            )
        })
    }

    fn start_with_targets<P, C, R, F>(
        prepare: F,
        selection: GenerationTargets,
        revision: u64,
        biome_blend: u8,
    ) -> Result<Self>
    where
        F: FnOnce() -> Result<(FreshTerrainBatch, P, C, R)> + Send + 'static,
        P: ResourceProvider,
        C: FnMut(&Value) -> Result<CarverBlockState>,
        R: FnMut(&Value) -> Result<BlockState>,
    {
        Self::spawn(move |cancelled, progress| {
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            // Preserve early validation for explicit targets before opening resources.
            let explicit = match selection {
                GenerationTargets::Explicit { targets, origin } => {
                    let initialized = initialized_neighbors(&targets)?;
                    Some((targets, origin, initialized))
                }
                GenerationTargets::SpawnOrigin => None,
            };
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let (mut batch, mut provider, mut classify, mut resolve) = prepare()?;
            ensure!(
                batch.is_complete() && batch.schedule().requested_chunks() == 0,
                "generation batch must be fresh"
            );
            let Some((targets, origin, initialized)) = explicit else {
                return generate_spawn_origin(
                    batch,
                    &mut provider,
                    &mut classify,
                    &mut resolve,
                    revision,
                    biome_blend,
                    cancelled,
                    progress,
                );
            };
            for pos in initialized {
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                batch.request_light_initialization(pos)?;
            }
            set_phase(progress, GenerationPhase::Generating);
            while let Some((pos, stage)) = {
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                generation_step(&mut batch, &mut provider, &mut classify, &mut resolve)?
            } {
                note_step(progress, pos, stage);
            }
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            batch.seal_for_light_propagation()?;
            set_phase(progress, GenerationPhase::Lighting);
            for &pos in &targets {
                batch.request_light(pos)?;
            }
            while let Some((pos, stage)) = {
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                generation_step(&mut batch, &mut provider, &mut classify, &mut resolve)?
            } {
                note_step(progress, pos, stage);
            }
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            set_phase(progress, GenerationPhase::PreparingMesh);
            let mesh = MeshJob::for_generated(&mut batch, targets, revision, biome_blend, origin)?;
            Ok(Some(GenerationReady {
                batch,
                mesh,
                worldgen_settings_draft: None,
                spawn_origin: None,
                suggested_spawn: None,
                initial_clocks: None,
                entity_collision_registry: None,
                level_settings: None,
            }))
        })
    }

    fn spawn(
        task: impl FnOnce(&AtomicBool, &Mutex<GenerationProgress>) -> Result<Option<GenerationReady>>
        + Send
        + 'static,
    ) -> Result<Self> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Mutex::new(GenerationProgress {
            phase: GenerationPhase::Planning,
            completed_steps: 0,
            last_step: None,
            spawn_search_resumptions: 0,
        }));
        let worker_cancelled = Arc::clone(&cancelled);
        let worker_progress = Arc::clone(&progress);
        let (sender, result) = mpsc::channel();
        thread::Builder::new()
            .name("minecraft-world-generator".into())
            .spawn(move || {
                let outcome = match task(&worker_cancelled, &worker_progress) {
                    Ok(Some(ready)) if !worker_cancelled.load(Ordering::Relaxed) => {
                        GenerationResult::Ready(ready)
                    }
                    Ok(_) => GenerationResult::Cancelled,
                    Err(error) => GenerationResult::Failed(error),
                };
                let phase = match &outcome {
                    GenerationResult::Ready(_) => GenerationPhase::Ready,
                    GenerationResult::Cancelled => GenerationPhase::Cancelled,
                    GenerationResult::Failed(_) => GenerationPhase::Failed,
                };
                set_phase(&worker_progress, phase);
                let _ = sender.send(outcome);
            })
            .context("cannot start world generation worker")?;
        Ok(Self {
            cancelled,
            progress,
            result,
        })
    }

    /// Cancellation is cooperative between steps; resource setup, an in-flight
    /// chunk calculation or light snapshot is allowed to finish first.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
    pub fn progress(&self) -> GenerationProgress {
        *self.progress.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn try_result(&self) -> Result<Option<GenerationResult>> {
        match self.result.try_recv() {
            Ok(result) => Ok(Some(result)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                anyhow::bail!("world generation worker stopped unexpectedly")
            }
        }
    }
}

fn checked_spawn_targets(ready: &mut GenerationReady) -> Result<BTreeSet<ChunkPos>> {
    let targets = ready
        .mesh
        .targets
        .as_deref()
        .context("generated session targets missing")?
        .clone();
    ensure!(!targets.is_empty(), "generated session has no targets");
    ensure!(
        ready.batch.is_complete(),
        "generation batch has pending work"
    );
    let current = ready.batch.lit_render_snapshot(targets.iter().copied())?;
    ensure!(
        Arc::ptr_eq(&current, &ready.mesh.world),
        "generated mesh snapshot is stale"
    );
    Ok(targets)
}

/// Run on the generation thread while retaining the original resource stack.
fn complete_creature_spawning_with_inputs<P, C, R, M, K>(
    ready: &mut GenerationReady,
    targets: &BTreeSet<ChunkPos>,
    provider: &mut P,
    classify: &mut C,
    resolve: &mut R,
    build_spawner: &mut M,
    construct: &mut K,
    spawn_mobs: bool,
    cancelled: &AtomicBool,
    progress: &Mutex<GenerationProgress>,
) -> Result<bool>
where
    P: ResourceProvider,
    C: FnMut(&Value) -> Result<CarverBlockState>,
    R: FnMut(&Value) -> Result<BlockState>,
    M: FnMut(
        &FreshTerrainBatch,
        &mut P,
        ChunkPos,
    ) -> Result<native_session::worldgen::GeneratedSheepSpawner>,
    K: FnMut(
        &native_resources::packs::ResourceId,
    ) -> Result<native_session::worldgen::FreshCreatureConstruction>,
{
    set_phase(progress, GenerationPhase::Spawning);
    for &pos in targets {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(false);
        }
        if ready.batch.schedule().completed(pos) == Some(GenerationStage::Spawn) {
            continue;
        }
        ensure!(
            ready.batch.schedule().completed(pos) == Some(GenerationStage::Light),
            "creature spawning requires a light-complete target"
        );
        if !spawn_mobs || ready.batch.mob_generation_disabled()? {
            ready.batch.request_spawn_without_mobs(pos, spawn_mobs)?;
        } else {
            let mut spawner = build_spawner(&ready.batch, provider, pos)?;
            loop {
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(false);
                }
                match spawner.advance_with_construction(&mut ready.batch, construct)? {
                    native_session::worldgen::SheepSpawnProgress::Finished => break,
                    native_session::worldgen::SheepSpawnProgress::Pending
                    | native_session::worldgen::SheepSpawnProgress::Rejected
                    | native_session::worldgen::SheepSpawnProgress::Prepared(_) => {}
                }
            }
            if cancelled.load(Ordering::Relaxed) {
                return Ok(false);
            }
            spawner.schedule_spawn(&mut ready.batch)?;
        }
        let completed = generation_step(&mut ready.batch, provider, classify, resolve)?;
        ensure!(
            completed == Some((pos, GenerationStage::Spawn)),
            "unexpected generation step after creature spawning"
        );
        note_step(progress, pos, GenerationStage::Spawn);
    }
    Ok(true)
}

fn complete_spawn_search(
    mut ready: GenerationReady,
    provider: &mut impl ResourceProvider,
    classify: &mut impl FnMut(&Value) -> Result<CarverBlockState>,
    resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
    tags: Arc<native_resources::heightmap_tags::HeightmapTags>,
    biome_blend: u8,
    ceiling_spawn_height: Option<i32>,
    cancelled: &AtomicBool,
    progress: &Mutex<GenerationProgress>,
) -> Result<Option<GenerationReady>> {
    let mut previous_search: Option<native_session::world_spawn::InitialWorldSpawn> = None;
    let mut checkpoint: Option<InitialSearchCheckpoint> = None;
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        set_phase(progress, GenerationPhase::SearchingSpawn);
        let (min_y, _) = ready.batch.build_bounds()?;
        let player = native_session::PlayerState {
            position: glam::Vec3::new(8.5, min_y as f32, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let session = ready.start_session_with_spawn_heightmaps(player, Arc::clone(&tags))?;
        let fresh_search = ready.initial_spawn_search_at_origin(&session)?;
        let mut search = match (previous_search.take(), checkpoint.take()) {
            (Some(search), Some(previous))
                if search.fallback_position() == fresh_search.fallback_position()
                    && previous.matches(&ready.batch)? =>
            {
                let mut progress = progress.lock().unwrap_or_else(|e| e.into_inner());
                progress.spawn_search_resumptions =
                    progress.spawn_search_resumptions.saturating_add(1);
                search
            }
            _ => fresh_search,
        };
        let target = loop {
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            match ready.poll_initial_spawn(&session, &mut search, ceiling_spawn_height, 256)? {
                InitialSpawnProgress::Pending => continue,
                InitialSpawnProgress::Found(pos) => {
                    ready.suggested_spawn = Some(pos);
                    return Ok(Some(ready));
                }
                InitialSpawnProgress::NeedsChunk(pos) => break pos,
            }
        };
        checkpoint = Some(InitialSearchCheckpoint::capture(&ready)?);
        previous_search = Some(search);
        drop(session);
        let revision = ready
            .mesh
            .revision
            .checked_add(1)
            .context("mesh revision overflow")?;
        let Some(expanded) = expand_with_inputs(
            ready,
            target,
            provider,
            classify,
            resolve,
            revision,
            biome_blend,
            cancelled,
            progress,
        )?
        else {
            return Ok(None);
        };
        ready = expanded;
    }
}

/// Search results remain valid only while every previously available target's
/// blocks and the generation-time heightmaps still match. Light-only changes
/// cannot affect the initial column search. This holds the shared terrain
/// snapshot rather than copying its section arrays.
struct InitialSearchCheckpoint {
    world: Arc<native_world::loaded::World>,
    heightmaps: BTreeMap<ChunkPos, Option<native_persistence::heightmap::SpawnHeightmaps>>,
}

impl InitialSearchCheckpoint {
    fn capture(ready: &GenerationReady) -> Result<Self> {
        let world = ready
            .batch
            .snapshot()
            .context("search terrain is unavailable")?;
        let targets = ready
            .mesh
            .targets
            .as_deref()
            .context("search targets are unavailable")?;
        let mut heightmaps = BTreeMap::new();
        for &pos in targets {
            ensure!(world.chunks.contains_key(&pos), "search target is missing");
            heightmaps.insert(pos, ready.batch.spawn_heightmaps(pos)?);
        }
        Ok(Self { world, heightmaps })
    }

    fn matches(&self, batch: &FreshTerrainBatch) -> Result<bool> {
        let current = batch.snapshot().context("search terrain is unavailable")?;
        self.matches_snapshot(&current, |pos| batch.spawn_heightmaps(pos))
    }

    fn matches_snapshot(
        &self,
        current: &native_world::loaded::World,
        mut heightmaps: impl FnMut(
            ChunkPos,
        )
            -> Result<Option<native_persistence::heightmap::SpawnHeightmaps>>,
    ) -> Result<bool> {
        if self.world.dimension != current.dimension
            || self.world.dimension_type != current.dimension_type
            || !self
                .world
                .states()
                .all(|(id, state)| current.state(id) == Some(state))
        {
            return Ok(false);
        }
        for (pos, maps) in &self.heightmaps {
            let Some(before) = self.world.chunks.get(pos) else {
                return Ok(false);
            };
            let Some(after) = current.chunks.get(pos) else {
                return Ok(false);
            };
            if !same_spawn_blocks(before, after) || *maps != heightmaps(*pos)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

fn same_spawn_blocks(
    before: &Arc<native_world::loaded::Chunk>,
    after: &Arc<native_world::loaded::Chunk>,
) -> bool {
    if Arc::ptr_eq(before, after) {
        return true;
    }
    if before.position != after.position || before.sections.len() != after.sections.len() {
        return false;
    }
    before.sections.iter().all(|(y, old)| {
        after
            .sections
            .get(y)
            .is_some_and(|new| Arc::ptr_eq(old, new) || old.blocks == new.blocks)
    })
}

fn generate_spawn_origin(
    mut batch: FreshTerrainBatch,
    provider: &mut impl ResourceProvider,
    classify: &mut impl FnMut(&Value) -> Result<CarverBlockState>,
    resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
    revision: u64,
    biome_blend: u8,
    cancelled: &AtomicBool,
    progress: &Mutex<GenerationProgress>,
) -> Result<Option<GenerationReady>> {
    ensure!(
        batch.is_complete() && batch.schedule().requested_chunks() == 0,
        "generation batch must be fresh"
    );
    let spawn = batch.spawn_origin(provider)?;
    let targets = BTreeSet::from([spawn]);
    let origin = IVec3::new(
        spawn.x.checked_mul(16).context("spawn origin X overflow")?,
        0,
        spawn.z.checked_mul(16).context("spawn origin Z overflow")?,
    );
    for pos in initialized_neighbors(&targets)? {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        batch.request_light_initialization(pos)?;
    }
    set_phase(progress, GenerationPhase::Generating);
    while let Some((pos, stage)) = {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        generation_step(&mut batch, provider, classify, resolve)?
    } {
        note_step(progress, pos, stage);
    }
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    batch.seal_for_light_propagation()?;
    set_phase(progress, GenerationPhase::Lighting);
    batch.request_light(spawn)?;
    while let Some((pos, stage)) = {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        generation_step(&mut batch, provider, classify, resolve)?
    } {
        note_step(progress, pos, stage);
    }
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    set_phase(progress, GenerationPhase::PreparingMesh);
    let mesh = MeshJob::for_generated(&mut batch, targets, revision, biome_blend, origin)?;
    Ok(Some(GenerationReady {
        batch,
        mesh,
        worldgen_settings_draft: None,
        spawn_origin: Some(spawn),
        suggested_spawn: None,
        initial_clocks: None,
        entity_collision_registry: None,
        level_settings: None,
    }))
}

fn expand_with_inputs(
    ready: GenerationReady,
    target: ChunkPos,
    provider: &mut impl ResourceProvider,
    classify: &mut impl FnMut(&Value) -> Result<CarverBlockState>,
    resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
    revision: u64,
    biome_blend: u8,
    cancelled: &AtomicBool,
    progress: &Mutex<GenerationProgress>,
) -> Result<Option<GenerationReady>> {
    let GenerationReady {
        mut batch,
        mesh,
        spawn_origin,
        suggested_spawn: _,
        worldgen_settings_draft,
        initial_clocks,
        entity_collision_registry,
        level_settings,
    } = ready;
    ensure!(
        revision != mesh.revision,
        "expanded mesh needs a new revision"
    );
    let mut targets = mesh
        .targets
        .as_deref()
        .context("generated session targets missing")?
        .clone();
    ensure!(batch.is_complete(), "generation batch has pending work");
    ensure!(!targets.is_empty(), "generated session has no targets");
    let previous = batch.lit_render_snapshot(targets.iter().copied())?;
    ensure!(
        Arc::ptr_eq(&previous, &mesh.world),
        "generated mesh snapshot is stale"
    );
    ensure!(targets.insert(target), "generation target is already ready");
    let initialized = initialized_neighbors(&BTreeSet::from([target]))?;
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    set_phase(progress, GenerationPhase::Generating);
    for &pos in &initialized {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        if batch
            .schedule()
            .completed(pos)
            .is_none_or(|stage| stage < GenerationStage::Features)
        {
            batch.request_features(pos)?;
        }
    }
    while let Some((pos, stage)) = {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        generation_step(&mut batch, provider, classify, resolve)?
    } {
        note_step(progress, pos, stage);
    }
    set_phase(progress, GenerationPhase::Lighting);
    for &pos in &initialized {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        if batch.schedule().completed(pos) == Some(GenerationStage::Features) {
            batch.request_light_initialization(pos)?;
        }
    }
    while let Some((pos, stage)) = {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        generation_step(&mut batch, provider, classify, resolve)?
    } {
        note_step(progress, pos, stage);
    }
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    if batch
        .schedule()
        .completed(target)
        .is_none_or(|stage| stage < GenerationStage::Light)
    {
        batch.request_light(target)?;
    }
    while let Some((pos, stage)) = {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        generation_step(&mut batch, provider, classify, resolve)?
    } {
        note_step(progress, pos, stage);
    }
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    set_phase(progress, GenerationPhase::PreparingMesh);
    let mesh = MeshJob::for_generated(&mut batch, targets, revision, biome_blend, mesh.origin)?;
    Ok(Some(GenerationReady {
        batch,
        mesh,
        worldgen_settings_draft,
        spawn_origin,
        suggested_spawn: None,
        initial_clocks,
        entity_collision_registry,
        level_settings,
    }))
}

fn initialized_neighbors(targets: &BTreeSet<ChunkPos>) -> Result<BTreeSet<ChunkPos>> {
    ensure!(!targets.is_empty(), "no generation targets");
    let mut initialized = BTreeSet::new();
    for pos in targets {
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                initialized.insert(ChunkPos {
                    x: pos.x.checked_add(dx).context("generation X overflow")?,
                    z: pos.z.checked_add(dz).context("generation Z overflow")?,
                });
            }
        }
    }
    Ok(initialized)
}

fn generation_step(
    batch: &mut FreshTerrainBatch,
    provider: &mut impl ResourceProvider,
    classify: &mut impl FnMut(&Value) -> Result<CarverBlockState>,
    resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
) -> Result<Option<(ChunkPos, GenerationStage)>> {
    let next = batch.schedule().next_ready();
    batch
        .step(provider, classify, resolve)
        .with_context(|| format!("generation step {next:?} failed"))
}

impl Drop for GenerationWorker {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn set_phase(progress: &Mutex<GenerationProgress>, phase: GenerationPhase) {
    progress.lock().unwrap_or_else(|e| e.into_inner()).phase = phase;
}
fn note_step(progress: &Mutex<GenerationProgress>, pos: ChunkPos, stage: GenerationStage) {
    let mut progress = progress.lock().unwrap_or_else(|e| e.into_inner());
    progress.completed_steps = progress.completed_steps.saturating_add(1);
    progress.last_step = Some((pos, stage));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn generated_session_block_entities_preserve_verified_data() {
        use native_persistence::nbt::{self, Tag, TagType};
        use native_resources::structure_start::HorizontalDirection;
        use native_session::worldgen::GeneratedLootChest;
        use native_world::loaded::SavedBlockEntities;

        let chest = GeneratedLootChest::new(
            [3, 8, 2],
            HorizontalDirection::West,
            "minecraft:chests/simple_dungeon",
            -7,
        )
        .unwrap();
        let list = Tag::List {
            element_type: TagType::Compound,
            values: vec![chest.block_entity()],
        };
        let payload =
            generated_session_block_entities(Some(GenerationStage::Spawn), || Ok(list.clone()));
        let SavedBlockEntities::Encoded(bytes) = &payload else {
            panic!("verified generated chest was not encoded");
        };
        assert_eq!(nbt::read(bytes).unwrap().value, list);
        let error = native_session::saved_chest::read(
            &payload,
            ChunkPos { x: 0, z: 0 },
            IVec3::new(3, 8, 2),
        )
        .unwrap_err();
        assert!(error.to_string().contains("loot table must be generated"));

        let empty = || {
            Ok(Tag::List {
                element_type: TagType::Compound,
                values: vec![],
            })
        };
        assert_eq!(
            generated_session_block_entities(Some(GenerationStage::Spawn), empty),
            SavedBlockEntities::Empty
        );
        assert_eq!(
            generated_session_block_entities(Some(GenerationStage::Light), || panic!(
                "premature load"
            )),
            SavedBlockEntities::Unavailable
        );
        assert_eq!(
            generated_session_block_entities(Some(GenerationStage::Spawn), || anyhow::bail!(
                "incomplete record"
            )),
            SavedBlockEntities::Unavailable
        );
        assert_eq!(
            generated_session_block_entities(Some(GenerationStage::Spawn), || Ok(Tag::Int(1))),
            SavedBlockEntities::Unavailable
        );
    }

    #[test]
    fn generated_session_retains_only_selected_chunk_metadata() {
        use native_world::loaded::{
            Chunk, SavedBlockEntities, SavedBlockTick, SavedFluidTick, World,
        };
        let home = ChunkPos { x: 0, z: 0 };
        let neighbor = ChunkPos { x: -1, z: 0 };
        let mut source = World::new("minecraft:overworld");
        for pos in [home, neighbor] {
            source.chunks.insert(
                pos,
                Arc::new(Chunk {
                    position: pos,
                    status: None,
                    block_light_correct: false,
                    sections: BTreeMap::new(),
                }),
            );
            source.saved_block_ticks.insert(
                pos,
                vec![SavedBlockTick {
                    block: "minecraft:sand".into(),
                    position: [pos.x * 16, 2, 0],
                    delay: 0,
                    priority: 0,
                }],
            );
            source.saved_fluid_ticks.insert(
                pos,
                vec![SavedFluidTick {
                    fluid: "minecraft:water".into(),
                    position: [pos.x * 16, 3, 0],
                    delay: 0,
                    priority: 0,
                }],
            );
            source
                .saved_block_entities
                .insert(pos, SavedBlockEntities::Empty);
            source
                .live_block_entities
                .insert(pos, SavedBlockEntities::Empty);
            source.missing_chunks.insert(pos);
        }
        let world = generated_session_world(&source, &BTreeSet::from([home]));
        assert_eq!(world.chunks.len(), 1);
        assert!(Arc::ptr_eq(&world.chunks[&home], &source.chunks[&home]));
        assert_eq!(
            world.saved_block_ticks.keys().copied().collect::<Vec<_>>(),
            vec![home]
        );
        assert_eq!(
            world.saved_fluid_ticks.keys().copied().collect::<Vec<_>>(),
            vec![home]
        );
        assert_eq!(
            world
                .saved_block_entities
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            vec![home]
        );
        assert_eq!(
            world
                .live_block_entities
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            vec![home]
        );
        assert_eq!(world.missing_chunks, BTreeSet::from([home]));
        assert_eq!(source.saved_block_ticks.len(), 2);
        assert_eq!(source.saved_fluid_ticks.len(), 2);
        assert_eq!(source.chunks.len(), 2);
    }

    #[test]
    fn resuming_unchanged_spawn_search_avoids_repeating_columns() -> Result<()> {
        use native_session::world_spawn::{
            InitialWorldSpawn, SharedSpawnWorld, SpawnColumnHeights,
        };
        struct SearchWorld {
            queries: usize,
        }
        impl SharedSpawnWorld for SearchWorld {
            fn min_y(&self) -> i32 {
                0
            }
            fn max_y(&self) -> i32 {
                15
            }
            fn ceiling_spawn_height(&self) -> Option<i32> {
                None
            }
            fn heights(&mut self, _: i32, _: i32) -> Result<SpawnColumnHeights> {
                self.queries += 1;
                Ok(SpawnColumnHeights {
                    motion_blocking: 0,
                    world_surface: 0,
                    ocean_floor: 0,
                })
            }
            fn has_fluid(&mut self, _: IVec3) -> Result<bool> {
                Ok(false)
            }
            fn full_top_face(&mut self, pos: IVec3) -> Result<bool> {
                Ok(pos.x >= 16 && pos.y == 0)
            }
            fn clear_player(&mut self, _: IVec3) -> Result<bool> {
                unreachable!("initial spawn only checks columns")
            }
        }
        let mut resumed_world = SearchWorld { queries: 0 };
        let mut resumed = InitialWorldSpawn::new(0, 0, 8)?;
        assert_eq!(resumed.poll(&mut resumed_world, 256)?, None);
        assert_eq!(resumed.requested_position(), IVec3::new(16, 8, 0));
        let result = resumed.poll(&mut resumed_world, 256)?;
        assert_eq!(result, Some(IVec3::new(16, 1, 0)));
        assert_eq!(resumed_world.queries, 257);

        let mut restart_world = SearchWorld { queries: 0 };
        let mut before_expansion = InitialWorldSpawn::new(0, 0, 8)?;
        assert_eq!(before_expansion.poll(&mut restart_world, 256)?, None);
        let mut restarted = InitialWorldSpawn::new(0, 0, 8)?;
        assert_eq!(restarted.poll(&mut restart_world, 512)?, result);
        assert_eq!(restart_world.queries, 513);
        Ok(())
    }

    #[test]
    fn initial_search_checkpoint_detects_changed_blocks_registry_and_heightmaps() -> Result<()> {
        use native_persistence::heightmap::{HeightBounds, SpawnHeightmaps};
        use native_world::loaded::{BLOCK_COUNT, Chunk, Section, World};
        let pos = ChunkPos { x: 0, z: 0 };
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: Default::default(),
        });
        let stone = world.intern_state(BlockState {
            name: "minecraft:stone".into(),
            properties: Default::default(),
        });
        world.chunks.insert(
            pos,
            Arc::new(Chunk {
                position: pos,
                status: None,
                block_light_correct: false,
                sections: BTreeMap::from([(
                    0,
                    Arc::new(Section {
                        blocks: Some(Box::new([air; BLOCK_COUNT])),
                        biomes: None,
                        block_light: None,
                        sky_light: None,
                    }),
                )]),
            }),
        );
        let heights = SpawnHeightmaps::from_first_available(
            HeightBounds {
                min_y: 0,
                max_y: 16,
            },
            &[0; 256],
            &[0; 256],
            &[0; 256],
        )?;
        let checkpoint = InitialSearchCheckpoint {
            world: Arc::new(world.clone()),
            heightmaps: BTreeMap::from([(pos, Some(heights.clone()))]),
        };
        let same = |_: ChunkPos| Ok(Some(heights.clone()));
        assert!(checkpoint.matches_snapshot(&world, same)?);
        let mut light_only = world.clone();
        Arc::make_mut(
            Arc::make_mut(light_only.chunks.get_mut(&pos).unwrap())
                .sections
                .get_mut(&0)
                .unwrap(),
        )
        .block_light = Some(Box::new([15; 2048]));
        assert!(checkpoint.matches_snapshot(&light_only, same)?);
        let mut changed = world.clone();
        Arc::make_mut(
            Arc::make_mut(changed.chunks.get_mut(&pos).unwrap())
                .sections
                .get_mut(&0)
                .unwrap(),
        )
        .blocks
        .as_mut()
        .unwrap()[0] = stone;
        assert!(!checkpoint.matches_snapshot(&changed, same)?);
        let updated = SpawnHeightmaps::from_first_available(
            HeightBounds {
                min_y: 0,
                max_y: 16,
            },
            &[1; 256],
            &[0; 256],
            &[0; 256],
        )?;
        assert!(!checkpoint.matches_snapshot(&world, |_| Ok(Some(updated.clone())))?);
        assert!(!checkpoint.matches_snapshot(&world, |_| Ok(None))?);
        let mut wrong_registry = World::new("minecraft:overworld");
        wrong_registry.intern_state(BlockState {
            name: "minecraft:stone".into(),
            properties: Default::default(),
        });
        wrong_registry.chunks = world.chunks.clone();
        assert!(!checkpoint.matches_snapshot(&wrong_registry, same)?);
        let mut missing = world.clone();
        missing.chunks.clear();
        assert!(!checkpoint.matches_snapshot(&missing, same)?);
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn initial_world_worker_keeps_selected_pack_through_creature_spawning() -> Result<()> {
        use crate::fresh_generation::prepare;
        use native_resources::datapacks::DataPackSettings;
        use serde_json::json;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let temp = tempfile::tempdir()?;
        let world = temp.path().join("world");
        let pack = world.join("datapacks/compact");
        for dir in [
            "data/example/worldgen/world_preset",
            "data/example/worldgen/biome",
            "data/example/dimension_type",
            "data/example/world_clock",
            "data/example/timeline",
            "data/minecraft/tags/block",
        ] {
            std::fs::create_dir_all(pack.join(dir))?;
        }
        std::fs::write(
            pack.join("pack.mcmeta"),
            br#"{"pack":{"description":"test","min_format":121,"max_format":121}}"#,
        )?;
        let generator = json!({"type":"minecraft:noise","biome_source":{"type":"minecraft:fixed","biome":"example:flat"},"settings":{
            "noise":{"min_y":0,"height":16},"legacy_random_source":false,"sea_level":8,
            "default_block":"minecraft:stone","default_fluid":"minecraft:water","disable_mob_generation":false,
            "spawn_target":[],"material_rule":{"type":"minecraft:block","result_state":"minecraft:dirt"},
            "noise_router":{"final_density":1,"temperature":0,"vegetation":0,"continents":0,"erosion":0,"depth":0,"ridges":0,"chunk_surface_level":8}
        }});
        let preset = json!({"dimensions":{"minecraft:overworld":{"type":"example:compact","generator":generator}}});
        std::fs::write(
            pack.join("data/example/worldgen/world_preset/compact.json"),
            serde_json::to_vec(&preset)?,
        )?;
        let mut disabled_preset = preset.clone();
        disabled_preset["dimensions"]["minecraft:overworld"]["generator"]["settings"]["disable_mob_generation"] =
            json!(true);
        std::fs::write(
            pack.join("data/example/worldgen/world_preset/compact_disabled.json"),
            serde_json::to_vec(&disabled_preset)?,
        )?;
        let dimension = json!({"min_y":0,"height":16,"has_skylight":true,"has_ceiling":false,"ambient_light":0.0,"default_clock":"example:creatures","timelines":["example:creatures"]});
        std::fs::write(
            pack.join("data/example/dimension_type/compact.json"),
            serde_json::to_vec(&dimension)?,
        )?;
        std::fs::write(pack.join("data/example/world_clock/creatures.json"), b"{}")?;
        let timeline = json!({"clock":"example:creatures","period_ticks":20,"time_markers":{"example:wake":7},"tracks":{
            "minecraft:gameplay/creature_world_gen_spawn_probability":{
                "modifier":"override","keyframes":[{"ticks":0,"value":0.75},{"ticks":10,"value":0.0}]
            }
        }});
        std::fs::write(
            pack.join("data/example/timeline/creatures.json"),
            serde_json::to_vec(&timeline)?,
        )?;
        let biome = json!({"has_precipitation":true,"temperature":0.8,"downfall":0.4,"carvers":[],"features":[],"attributes":{
            "minecraft:gameplay/creature_world_gen_spawn_probability":0.0,
            "minecraft:gameplay/natural_mob_spawns":{"spawn_costs":{},"spawns_by_category":{"creature":[{"type":"minecraft:sheep","count":1,"weight":1}]}}
        }});
        std::fs::write(
            pack.join("data/example/worldgen/biome/flat.json"),
            serde_json::to_vec(&biome)?,
        )?;
        std::fs::write(
            pack.join("data/minecraft/tags/block/animals_spawnable_on.json"),
            br#"{"replace":true,"values":["minecraft:dirt"]}"#,
        )?;
        for (spawn_mobs, disabled, species, feature_kind) in [
            (false, false, "minecraft:sheep", 0),
            (true, true, "minecraft:sheep", 0),
            (true, true, "minecraft:sheep", 1),
            (true, true, "minecraft:sheep", 2),
            (true, false, "minecraft:sheep", 0),
            (true, false, "minecraft:sheep", 1),
            (true, false, "minecraft:horse", 0),
            (true, false, "minecraft:donkey", 0),
        ] {
            let mut selected_biome = biome.clone();
            if feature_kind != 0 {
                let simple = json!({"feature":{"type":"minecraft:simple_block", "to_place":{"id":"minecraft:short_grass"}},"placement":[]});
                let configured = if feature_kind == 1 {
                    simple["feature"].clone()
                } else {
                    json!({"type":"minecraft:random_boolean_selector", "feature_true":simple, "feature_false":simple})
                };
                selected_biome["features"] = json!([[{
                    "feature": configured,
                    "placement": if feature_kind == 1 {json!([{"type":"minecraft:offset","x":8,"y":8,"z":8}])} else {json!([{"type":"minecraft:count","count":0}])}
                }]]);
            }
            selected_biome["attributes"]["minecraft:gameplay/natural_mob_spawns"]["spawns_by_category"]
                ["creature"][0]["type"] = json!(species);
            std::fs::write(
                pack.join("data/example/worldgen/biome/flat.json"),
                serde_json::to_vec(&selected_biome)?,
            )?;
            let collision_height_tags = if disabled
                || (spawn_mobs && !disabled && species == "minecraft:sheep")
            {
                let mut resources = native_resources::packs::ResourceStack::open(&game)?;
                let warnings = resources.configure_world_data(
                    &world,
                    &DataPackSettings {
                        enabled: vec!["vanilla".into(), "file/compact".into()],
                        disabled: vec![],
                        enabled_features: vec!["minecraft:vanilla".into()],
                    },
                )?;
                assert!(warnings.is_empty());
                let blocks = resources.vanilla_block_ids()?;
                Some(Arc::new(
                    native_resources::heightmap_tags::HeightmapTags::load(&mut resources, &blocks)?,
                ))
            } else {
                None
            };
            let game = game.clone();
            let world = world.clone();
            let worker = GenerationWorker::start_initial_world(
                move || {
                    let mut settings =
                        crate::world_creation::WorldCreationSettings::new("Test".into());
                    settings.seed = crate::chat_input::ChatInput::field("42");
                    settings.mode = crate::world_creation::GameMode::Creative;
                    settings.difficulty = crate::world_creation::Difficulty::Easy;
                    settings.generate_structures = false;
                    settings.preset = native_resources::packs::ResourceId::parse(if disabled {
                        "example:compact_disabled"
                    } else {
                        "example:compact"
                    })?;
                    let frozen = settings.freeze(
                        game,
                        Some((
                            world,
                            DataPackSettings {
                                enabled: vec!["vanilla".into(), "file/compact".into()],
                                disabled: vec![],
                                enabled_features: vec!["minecraft:vanilla".into()],
                            },
                        )),
                    );
                    let prepared = prepare(frozen.generation)?;
                    let border = native_session::respawn::RespawnBorder {
                        min_x: -100.0,
                        max_x: 100.0,
                        min_z: -100.0,
                        max_z: 100.0,
                    };
                    let mut inputs = prepared.into_new_world_inputs(border, spawn_mobs, true)?;
                    let mut wrong_seed = frozen.level.clone();
                    wrong_seed.session.seed += 1;
                    assert!(inputs.set_level_settings(wrong_seed).is_err());
                    let mut wrong_packs = frozen.level.clone();
                    wrong_packs.packs.as_mut().unwrap().enabled.clear();
                    assert!(inputs.set_level_settings(wrong_packs).is_err());
                    inputs.set_level_settings(frozen.level)?;
                    assert_eq!(inputs.creatures.is_some(), spawn_mobs && !disabled);
                    assert_eq!(
                        inputs.initial_clocks.ticks.get(
                            &native_resources::packs::ResourceId::parse("example:creatures")?
                        ),
                        Some(&0)
                    );
                    Ok(inputs)
                },
                80,
                0,
                None,
            )?;
            let mut ready = match worker.result.recv_timeout(Duration::from_secs(60))? {
                GenerationResult::Ready(ready) => ready,
                GenerationResult::Failed(error) => return Err(error),
                GenerationResult::Cancelled => anyhow::bail!("initial world cancelled"),
            };
            let level = ready
                .level_settings
                .as_ref()
                .context("level settings missing")?;
            let level_draft = level.draft_level_data(
                &native_persistence::world::RespawnData::default(),
                123,
                None,
                false,
            )?;
            let level_data = level_draft.value.get("Data").unwrap();
            assert_eq!(
                level_data.get("GameType"),
                Some(&native_persistence::nbt::Tag::Int(1))
            );
            assert!(level_data.get("DataPacks").is_some());
            let targets = ready
                .mesh
                .targets
                .as_deref()
                .context("targets missing")?
                .clone();
            assert_eq!(
                ready.initial_clocks.as_ref().unwrap().ticks.get(
                    &native_resources::packs::ResourceId::parse("example:creatures")?
                ),
                Some(&0)
            );
            assert!(ready.suggested_spawn.is_some());
            assert!(
                targets
                    .iter()
                    .all(|pos| ready.batch.schedule().completed(*pos)
                        == Some(GenerationStage::Spawn))
            );
            for &pos in &targets {
                use native_persistence::nbt::Tag;
                assert_eq!(
                    ready
                        .batch
                        .complete_generated_entity_records(pos)?
                        .is_some(),
                    feature_kind != 2
                );
                let source = ready.batch.spawned_heightmap_data(pos)?;
                let full = ready.batch.converted_full_heightmap_data(pos)?;
                for name in [
                    "WORLD_SURFACE",
                    "OCEAN_FLOOR",
                    "MOTION_BLOCKING",
                    "MOTION_BLOCKING_NO_LEAVES",
                ] {
                    match source.get(name) {
                        Some(value) => assert_eq!(full.get(name), Some(value)),
                        None => assert!(
                            matches!(full.get(name), Some(Tag::LongArray(words)) if words.iter().all(|&word| word == 0))
                        ),
                    }
                }
                assert!(full.get("WORLD_SURFACE_WG").is_none());
                assert!(full.get("OCEAN_FLOOR_WG").is_none());
            }
            let count: usize = targets
                .iter()
                .map(|&pos| ready.batch.recorded_generated_entities(pos).unwrap().len())
                .sum();
            if spawn_mobs && !disabled {
                assert!(count > 0, "expected generated {species}");
                for &pos in targets.iter() {
                    for entity in ready.batch.recorded_generated_entities(pos)? {
                        assert!(
                            matches!(entity.get("id"), Some(native_persistence::nbt::Tag::String(id)) if id.matches(species))
                        );
                    }
                }
                let target = *targets
                    .iter()
                    .find(|&&pos| {
                        !ready
                            .batch
                            .recorded_generated_entities(pos)
                            .unwrap()
                            .is_empty()
                    })
                    .unwrap();
                let position = IVec3::new(target.x * 16 + 8, 16, target.z * 16 + 8);
                let player = native_session::PlayerState {
                    position: (position - ready.mesh.origin).as_vec3()
                        + glam::Vec3::new(0.5, 0.0, 0.5),
                    yaw: 0.0,
                    pitch: 0.0,
                    vertical_velocity: 0.0,
                    on_ground: true,
                    pose: native_session::PlayerPose::Standing,
                };
                let session = ready.start_session(player)?;
                let Some(native_session::loaded::SavedEntityData::Present(chunk)) =
                    session.saved_entity_chunk(target)
                else {
                    panic!("generated {species} entity chunk is missing");
                };
                assert_eq!(
                    chunk.entities(),
                    ready.batch.recorded_generated_entities(target)?
                );
            } else {
                assert_eq!(count, 0);
            }
            if disabled {
                use native_session::loaded::SavedEntityData;
                let target = *targets.iter().next().unwrap();
                let position = IVec3::new(target.x * 16 + 8, 16, target.z * 16 + 8);
                let player = native_session::PlayerState {
                    position: (position - ready.mesh.origin).as_vec3()
                        + glam::Vec3::new(0.5, 0.0, 0.5),
                    yaw: 0.0,
                    pitch: 0.0,
                    vertical_velocity: 0.0,
                    on_ground: true,
                    pose: native_session::PlayerPose::Standing,
                };
                let mut session = ready.start_session(player)?;
                session.set_spawn_heightmap_tags(Arc::clone(
                    collision_height_tags.as_ref().unwrap(),
                ))?;
                if feature_kind == 2 {
                    assert!(
                        targets
                            .iter()
                            .all(|&pos| session.saved_entity_chunk(pos).is_none())
                    );
                    assert!(session.generated_spawn_clear(position, &targets).is_err());
                } else {
                    assert!(targets.iter().all(|&pos| matches!(
                        session.saved_entity_chunk(pos),
                        Some(SavedEntityData::Empty)
                    )));
                    assert!(session.generated_spawn_clear(position, &targets)?);
                }
            }
            if spawn_mobs && !disabled && species == "minecraft:sheep" {
                use native_session::loaded::SavedEntityData;
                let target = *targets
                    .iter()
                    .find(|&&pos| {
                        !ready
                            .batch
                            .recorded_generated_entities(pos)
                            .unwrap()
                            .is_empty()
                    })
                    .unwrap();
                let entities = ready.batch.recorded_generated_entities(target)?.to_vec();
                let origin = ready.mesh.origin;
                let position = IVec3::new(target.x * 16 + 8, 16, target.z * 16 + 8);
                let player = native_session::PlayerState {
                    position: (position - origin).as_vec3() + glam::Vec3::new(0.5, 0.0, 0.5),
                    yaw: 0.0,
                    pitch: 0.0,
                    vertical_velocity: 0.0,
                    on_ground: true,
                    pose: native_session::PlayerPose::Standing,
                };
                let registry = ready
                    .entity_collision_registry
                    .take()
                    .context("collision registry missing")?;
                let mut without_registry = ready.start_session(player)?;
                without_registry.set_spawn_heightmap_tags(Arc::clone(
                    collision_height_tags.as_ref().unwrap(),
                ))?;
                let Some(SavedEntityData::Present(chunk)) =
                    without_registry.saved_entity_chunk(target)
                else {
                    panic!("generated sheep entity chunk is missing");
                };
                assert_eq!(chunk.entities(), entities);
                assert!(
                    without_registry
                        .generated_spawn_clear(position, &targets)
                        .is_err()
                );
                ready.entity_collision_registry = Some(registry);
                let mut session = ready.start_session(player)?;
                session.set_spawn_heightmap_tags(collision_height_tags.unwrap())?;
                assert!(session.generated_spawn_clear(position, &targets)?);
            }
            if !spawn_mobs {
                use native_session::{GameSession, PlayerCommand, SystemMessageKind};
                let target = *targets.iter().next().unwrap();
                let origin = ready.mesh.origin;
                let player = native_session::PlayerState {
                    position: glam::Vec3::new(
                        (target.x * 16 + 8 - origin.x) as f32 + 0.5,
                        (16 - origin.y) as f32,
                        (target.z * 16 + 8 - origin.z) as f32 + 0.5,
                    ),
                    yaw: 0.0,
                    pitch: 0.0,
                    vertical_velocity: 0.0,
                    on_ground: true,
                    pose: native_session::PlayerPose::Standing,
                };
                let mut session = ready.start_session(player)?;
                let snapshot = session.snapshot();
                assert!(targets.iter().all(|pos| matches!(
                    snapshot.world.loaded().unwrap().saved_block_entities.get(pos),
                    Some(native_world::loaded::SavedBlockEntities::Empty)
                )));
                assert_eq!(
                    session.snapshot().game_mode,
                    native_session::GameMode::Creative
                );
                session.update(Duration::from_millis(150));
                session.submit_command(PlayerCommand::QueryWorldSeed);
                for text in [
                    "/time query time",
                    "/time query example:creatures",
                    "/time set example:wake",
                ] {
                    session.submit_command(PlayerCommand::WorldTime(Arc::new(
                        native_session::time_command::parse(text).unwrap(),
                    )));
                }
                session.update(Duration::from_millis(50));
                let snapshot = session.snapshot();
                let messages = &snapshot.system_messages;
                assert!(
                    messages
                        .iter()
                        .any(|message| matches!(message.kind, SystemMessageKind::WorldSeed(42)))
                );
                assert!(messages.iter().any(|message| matches!(
                    &message.kind,
                    SystemMessageKind::TimeClock { name, ticks } if name.as_ref() == "example:creatures" && (3..=4).contains(ticks)
                )));
                assert!(messages.iter().any(|message| matches!(
                    &message.kind,
                    SystemMessageKind::TimeTimeline { name, ticks, repetitions: false } if name.as_ref() == "example:creatures" && (3..=4).contains(ticks)
                )));
                assert!(matches!(
                    messages.last().unwrap().kind,
                    SystemMessageKind::TimeSaveUnavailable
                ));
            }
            assert_eq!(worker.progress().phase, GenerationPhase::Ready);
        }
        Ok(())
    }

    #[test]
    fn cancellation_and_errors_report_terminal_state() {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = GenerationWorker::spawn(move |cancelled, progress| {
            set_phase(progress, GenerationPhase::Generating);
            note_step(progress, ChunkPos { x: 0, z: 0 }, GenerationStage::Biomes);
            entered_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            assert!(cancelled.load(Ordering::Relaxed));
            Ok(None)
        })
        .unwrap();
        entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(worker.progress().phase, GenerationPhase::Generating);
        assert_eq!(worker.progress().completed_steps, 1);
        worker.cancel();
        release_tx.send(()).unwrap();
        assert!(matches!(
            worker.result.recv_timeout(Duration::from_secs(3)).unwrap(),
            GenerationResult::Cancelled
        ));
        assert_eq!(worker.progress().phase, GenerationPhase::Cancelled);
        let worker = GenerationWorker::spawn(|_, _| anyhow::bail!("fixture failure")).unwrap();
        let GenerationResult::Failed(error) =
            worker.result.recv_timeout(Duration::from_secs(3)).unwrap()
        else {
            panic!("expected failure")
        };
        assert!(error.to_string().contains("fixture failure"));
        assert_eq!(worker.progress().phase, GenerationPhase::Failed);
    }

    #[test]
    fn invalid_targets_fail_before_resource_setup() {
        struct UnusedProvider;
        impl ResourceProvider for UnusedProvider {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
        }
        type Prepared = (
            FreshTerrainBatch,
            UnusedProvider,
            fn(&Value) -> Result<CarverBlockState>,
            fn(&Value) -> Result<BlockState>,
        );
        fn never_prepare() -> Result<Prepared> {
            panic!("resource setup must not run")
        }
        for targets in [
            BTreeSet::new(),
            BTreeSet::from([ChunkPos { x: i32::MAX, z: 0 }]),
        ] {
            let worker =
                GenerationWorker::start(never_prepare, targets, 0, 0, IVec3::ZERO).unwrap();
            let GenerationResult::Failed(_) =
                worker.result.recv_timeout(Duration::from_secs(3)).unwrap()
            else {
                panic!("expected invalid targets to fail")
            };
            assert_eq!(worker.progress().phase, GenerationPhase::Failed);
        }
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and a full generation batch"]
    fn installed_overworld_generates_a_lit_mesh_job() -> Result<()> {
        installed_overworld_generation(false)
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and a full generation batch"]
    fn installed_overworld_generates_computed_spawn_origin() -> Result<()> {
        use crate::fresh_generation::{FreshGenerationSettings, prepare};
        use native_session::{PlayerPose, PlayerState};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let settings = |minecraft_dir| FreshGenerationSettings {
            minecraft_dir,
            preset: "minecraft:normal".into(),
            dimension: "minecraft:overworld".into(),
            seed: 42,
            bonus_chest: false,
            structures_enabled: false,
            data_packs: None,
        };
        let expected = prepare(settings(game.clone()))?.spawn_origin()?;
        let worker_game = game.clone();
        let worker = GenerationWorker::start_at_spawn(
            move || Ok(prepare(settings(worker_game))?.into_worker_inputs()),
            13,
            0,
        )?;
        let mut ready = match worker.result.recv_timeout(Duration::from_secs(300))? {
            GenerationResult::Ready(ready) => ready,
            GenerationResult::Failed(error) => return Err(error),
            GenerationResult::Cancelled => anyhow::bail!("generation was cancelled"),
        };
        let targets = BTreeSet::from([expected]);
        assert_eq!(ready.spawn_origin, Some(expected));
        assert_eq!(ready.mesh.targets.as_deref(), Some(&targets));
        assert_eq!(
            ready.mesh.origin,
            IVec3::new(expected.x * 16, 0, expected.z * 16)
        );
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let registry = resources.vanilla_block_ids()?;
        let tags = Arc::new(native_resources::heightmap_tags::HeightmapTags::load(
            &mut resources,
            &registry,
        )?);
        let player = PlayerState {
            position: glam::Vec3::new(8.5, 150.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: PlayerPose::Standing,
        };
        let session = ready.start_session_with_spawn_heightmaps(player, tags)?;
        let mut search = ready.initial_spawn_search_at_origin(&session)?;
        let found = session.poll_generated_initial_spawn(&mut search, None, &targets, 256)?;
        if let Some(pos) = found {
            assert_eq!(
                (pos.x.div_euclid(16), pos.z.div_euclid(16)),
                (expected.x, expected.z)
            );
        } else {
            let next = search.requested_position();
            assert_ne!(
                (next.x.div_euclid(16), next.z.div_euclid(16)),
                (expected.x, expected.z)
            );
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and fresh generation"]
    fn installed_overworld_automatically_selects_initial_spawn() -> Result<()> {
        use crate::fresh_generation::{FreshGenerationSettings, prepare};
        use native_session::{PlayerPose, PlayerState};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let worker_game = game.clone();
        let worker = GenerationWorker::start_spawn_search(
            move || {
                prepare(FreshGenerationSettings {
                    minecraft_dir: worker_game,
                    preset: "minecraft:normal".into(),
                    dimension: "minecraft:overworld".into(),
                    seed: 42,
                    bonus_chest: false,
                    structures_enabled: false,
                    data_packs: None,
                })?
                .into_spawn_worker_inputs()
            },
            30,
            0,
            None,
        )?;
        let mut ready = match worker.result.recv_timeout(Duration::from_secs(600))? {
            GenerationResult::Ready(ready) => ready,
            GenerationResult::Failed(error) => return Err(error),
            GenerationResult::Cancelled => anyhow::bail!("generation was cancelled"),
        };
        let found = ready.suggested_spawn.context("spawn suggestion missing")?;
        eprintln!(
            "spawn origin {:?}, suggestion {found:?}, generated targets {}",
            ready.spawn_origin,
            ready
                .mesh
                .targets
                .as_ref()
                .map_or(0, |targets| targets.len())
        );
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let registry = resources.vanilla_block_ids()?;
        let tags = Arc::new(native_resources::heightmap_tags::HeightmapTags::load(
            &mut resources,
            &registry,
        )?);
        let (min_y, _) = ready.batch.build_bounds()?;
        let player = PlayerState {
            position: glam::Vec3::new(8.5, min_y as f32, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: PlayerPose::Standing,
        };
        let session = ready.start_session_with_spawn_heightmaps(player, tags)?;
        let mut search = ready.initial_spawn_search_at_origin(&session)?;
        loop {
            match ready.poll_initial_spawn(&session, &mut search, None, 256)? {
                InitialSpawnProgress::Pending => continue,
                InitialSpawnProgress::Found(pos) => {
                    assert_eq!(pos, found);
                    break;
                }
                InitialSpawnProgress::NeedsChunk(pos) => {
                    anyhow::bail!("missing completed search chunk {pos:?}")
                }
            }
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and multiple generated chunks"]
    fn installed_spawn_search_expands_when_first_chunk_has_no_candidate() -> Result<()> {
        use crate::fresh_generation::{FreshGenerationSettings, prepare};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let settings = |minecraft_dir| FreshGenerationSettings {
            minecraft_dir,
            preset: "minecraft:normal".into(),
            dimension: "minecraft:overworld".into(),
            seed: 42,
            bonus_chest: false,
            structures_enabled: false,
            data_packs: None,
        };
        let worker_game = game.clone();
        let origin = ChunkPos { x: 0, z: 0 };
        let worker = GenerationWorker::start(
            move || Ok(prepare(settings(worker_game))?.into_worker_inputs()),
            BTreeSet::from([origin]),
            40,
            0,
            IVec3::ZERO,
        )?;
        let mut ready = match worker.result.recv_timeout(Duration::from_secs(300))? {
            GenerationResult::Ready(ready) => ready,
            GenerationResult::Failed(error) => return Err(error),
            GenerationResult::Cancelled => anyhow::bail!("generation was cancelled"),
        };
        // This location is an intentionally chosen search origin; its first
        // 256 columns have no candidate in this 26.3 seed, unlike the noise
        // generator's real origin, and exercise the expansion path.
        ready.spawn_origin = Some(origin);
        let (_, mut provider, mut classify, mut resolve, tags, _) =
            prepare(settings(game))?.into_spawn_worker_inputs()?;
        let cancelled = AtomicBool::new(false);
        let progress = Mutex::new(GenerationProgress {
            phase: GenerationPhase::Planning,
            completed_steps: 0,
            last_step: None,
            spawn_search_resumptions: 0,
        });
        let ready = complete_spawn_search(
            ready,
            &mut provider,
            &mut classify,
            &mut resolve,
            tags,
            0,
            None,
            &cancelled,
            &progress,
        )?
        .context("spawn selection cancelled")?;
        let targets = ready.mesh.targets.as_deref().context("targets missing")?;
        assert!(targets.contains(&ChunkPos { x: 1, z: 0 }));
        assert!(targets.len() >= 2);
        assert!(ready.suggested_spawn.is_some());
        assert!(ready.mesh.revision > 40);
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and a full generation batch"]
    fn installed_overworld_expands_to_adjacent_lit_mesh() -> Result<()> {
        installed_overworld_generation(true)
    }

    fn installed_overworld_generation(expand: bool) -> Result<()> {
        use crate::fresh_generation::{FreshGenerationSettings, prepare};

        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mesh_game = game.clone();
        let targets = BTreeSet::from([ChunkPos { x: 0, z: 0 }]);
        let worker = GenerationWorker::start(
            move || {
                Ok(prepare(FreshGenerationSettings {
                    minecraft_dir: game,
                    preset: "minecraft:normal".into(),
                    dimension: "minecraft:overworld".into(),
                    seed: 42,
                    bonus_chest: false,
                    structures_enabled: false,
                    data_packs: None,
                })?
                .into_worker_inputs())
            },
            targets.clone(),
            7,
            0,
            IVec3::ZERO,
        )?;
        let result = worker.result.recv_timeout(Duration::from_secs(300))?;
        let mut generated = match result {
            GenerationResult::Ready(generated) => generated,
            GenerationResult::Failed(error) => {
                return Err(error.context(format!("generation progress: {:?}", worker.progress())));
            }
            GenerationResult::Cancelled => anyhow::bail!("generation was cancelled"),
        };
        assert_eq!(worker.progress().phase, GenerationPhase::Ready);
        assert_eq!(generated.mesh.targets.as_deref(), Some(&targets));
        assert_eq!(generated.mesh.revision, 7);
        assert!(
            generated
                .mesh
                .world
                .chunks
                .contains_key(&ChunkPos { x: 0, z: 0 })
        );
        {
            use native_session::{GameSession, PlayerPose, PlayerState};
            let player = PlayerState {
                position: glam::Vec3::new(8.5, 150.0, 8.5),
                yaw: 0.0,
                pitch: 0.0,
                vertical_velocity: 0.0,
                on_ground: false,
                pose: PlayerPose::Standing,
            };
            let mut resources = native_resources::packs::ResourceStack::open(&mesh_game)?;
            let registry = resources.vanilla_block_ids()?;
            let tags = Arc::new(native_resources::heightmap_tags::HeightmapTags::load(
                &mut resources,
                &registry,
            )?);
            let session = generated.start_session_with_spawn_heightmaps(player, tags)?;
            let snapshot = session.snapshot();
            let world = snapshot
                .world
                .loaded()
                .context("generated session world missing")?;
            assert_eq!(world.chunks.len(), 1);
            assert!(world.chunks[&ChunkPos { x: 0, z: 0 }].block_light_correct);
            assert!(
                session
                    .spawn_heightmaps_for_chunk(ChunkPos { x: 0, z: 0 })
                    .context("generated spawn heightmaps missing")?
                    .is_complete()
            );
            let mut unavailable = std::collections::BTreeMap::<String, usize>::new();
            let mut unsuitable = 0;
            let mut candidates = 0;
            for z in 0..16 {
                for x in 0..16 {
                    match session.generated_spawn_column_candidate(x, z, None, &targets) {
                        Ok(Some(pos)) => {
                            assert_eq!(pos.x, x);
                            assert_eq!(pos.z, z);
                            candidates += 1;
                        }
                        Ok(None) => unsuitable += 1,
                        Err(error) => *unavailable.entry(format!("{error:#}")).or_default() += 1,
                    }
                }
            }
            assert!(
                unavailable.is_empty(),
                "unavailable spawn columns: {unavailable:?}"
            );
            assert_eq!(candidates + unsuitable, 256);
            let mut search = generated.initial_spawn_search(&session, ChunkPos { x: 0, z: 0 })?;
            let progress = generated.poll_initial_spawn(&session, &mut search, None, 512)?;
            if candidates == 0 {
                assert_eq!(
                    progress,
                    InitialSpawnProgress::NeedsChunk(ChunkPos { x: 1, z: 0 })
                );
            } else {
                assert!(matches!(progress, InitialSpawnProgress::Found(_)));
            }
        }
        let mesher = crate::mesh_worker::MeshWorker::start(
            native_resources::packs::ResourceStack::open(&mesh_game)?,
        )?;
        mesher.submit(MeshJob {
            revision: generated.mesh.revision,
            biome_blend: generated.mesh.biome_blend,
            origin: generated.mesh.origin,
            world: Arc::clone(&generated.mesh.world),
            targets: generated.mesh.targets.clone(),
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        let mesh = loop {
            if let Some(result) = mesher.try_result()? {
                break result;
            }
            anyhow::ensure!(
                std::time::Instant::now() < deadline,
                "generated mesh timed out"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(mesh.revision, 7);
        assert_eq!(mesh.targets.as_deref(), Some(&targets));
        let scene = mesh.scene?;
        assert_eq!(scene.chunks.len(), 1);
        assert!(scene.chunks.contains_key(&ChunkPos { x: 0, z: 0 }));
        assert!(scene.opaque_len() > 0, "generated terrain mesh is empty");
        if !expand {
            return Ok(());
        }

        let east = ChunkPos { x: 1, z: 0 };
        let worker = GenerationWorker::expand(
            generated,
            east,
            move || {
                let (_, provider, classify, resolve) = prepare(FreshGenerationSettings {
                    minecraft_dir: mesh_game,
                    preset: "minecraft:normal".into(),
                    dimension: "minecraft:overworld".into(),
                    seed: 42,
                    bonus_chest: false,
                    structures_enabled: false,
                    data_packs: None,
                })?
                .into_worker_inputs();
                Ok((provider, classify, resolve))
            },
            8,
            0,
        )?;
        let mut generated = match worker.result.recv_timeout(Duration::from_secs(300))? {
            GenerationResult::Ready(generated) => generated,
            GenerationResult::Failed(error) => return Err(error),
            GenerationResult::Cancelled => anyhow::bail!("generation was cancelled"),
        };
        let expanded_targets = BTreeSet::from([ChunkPos { x: 0, z: 0 }, east]);
        assert_eq!(generated.mesh.targets.as_deref(), Some(&expanded_targets));
        {
            use native_session::{PlayerPose, PlayerState};
            let player = PlayerState {
                position: glam::Vec3::new(8.5, 150.0, 8.5),
                yaw: 0.0,
                pitch: 0.0,
                vertical_velocity: 0.0,
                on_ground: false,
                pose: PlayerPose::Standing,
            };
            let game = std::env::var_os("MINECRAFT_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
            let mut resources = native_resources::packs::ResourceStack::open(&game)?;
            let registry = resources.vanilla_block_ids()?;
            let tags = Arc::new(native_resources::heightmap_tags::HeightmapTags::load(
                &mut resources,
                &registry,
            )?);
            let session = generated.start_session_with_spawn_heightmaps(player, tags)?;
            let mut search = generated.initial_spawn_search(&session, ChunkPos { x: 0, z: 0 })?;
            let first =
                session.poll_generated_initial_spawn(&mut search, None, &expanded_targets, 256)?;
            if first.is_none() {
                let pos = search.requested_position();
                assert_eq!(
                    (pos.x.div_euclid(16), pos.z.div_euclid(16)),
                    (east.x, east.z)
                );
                let next = session.poll_generated_initial_spawn(
                    &mut search,
                    None,
                    &expanded_targets,
                    256,
                )?;
                if let Some(found) = next {
                    assert_eq!(
                        (found.x.div_euclid(16), found.z.div_euclid(16)),
                        (east.x, east.z)
                    );
                } else {
                    let pos = search.requested_position();
                    assert_ne!(
                        (pos.x.div_euclid(16), pos.z.div_euclid(16)),
                        (east.x, east.z)
                    );
                }
            }
        }
        mesher.submit(generated.mesh);
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        let mesh = loop {
            if let Some(result) = mesher.try_result()? {
                break result;
            }
            anyhow::ensure!(
                std::time::Instant::now() < deadline,
                "expanded mesh timed out"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(mesh.revision, 8);
        assert_eq!(mesh.targets.as_deref(), Some(&expanded_targets));
        let scene = mesh.scene?;
        assert_eq!(scene.chunks.len(), 2);
        assert!(scene.chunks.contains_key(&east));
        assert!(scene.opaque_len() > 0);
        Ok(())
    }
}
