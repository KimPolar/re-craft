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
use native_session::worldgen::{
    CarverBlockState, FreshTerrainBatch,
    schedule::GenerationStage,
    structure_batch::{StructureGenerationBatch, StructurePreparationPlan},
};
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
    Postprocessing,
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
    /// Number of times player spawn search resumed after unchanged inputs.
    pub player_spawn_search_resumptions: u64,
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
    /// Collision-checked initial player position, if requested by the caller.
    pub player_spawn: Option<IVec3>,
    /// Starting clock ticks from the same pack stack, when prepared for a new world.
    pub initial_clocks: Option<Arc<crate::fresh_generation::InitialWorldClocks>>,
    /// Frozen at new-world time zero; this is not a running-session clock.
    pub(crate) hut_generation: Option<Arc<crate::fresh_generation::InitialHutGeneration>>,
    pub entity_collision_registry:
        Option<Arc<native_resources::entity_collision::EntityCollisionRegistry>>,
    pub level_settings: Option<crate::world_creation::FrozenLevelSettings>,
    pub(crate) pack_snapshot: Option<Arc<native_resources::datapack_snapshot::DataPackSnapshot>>,
    pub(crate) initial_respawn: Option<InitialRespawnSettings>,
    /// Isolated postprocessing output; it is not yet a playable world.
    pub postprocessed: Option<PostprocessedGeneration>,
}

/// Inputs for a finite, structure-aware generation request. They must all
/// originate from the same immutable resource stack and dimension settings.
pub(crate) struct StructuredGenerationInputs<P, C, R, H, F> {
    pub structures: StructureGenerationBatch,
    /// Paired terrain generator enables later structure-aware expansion.
    pub terrain_generator: Option<native_session::worldgen::TerrainGenerator>,
    pub height_tags: Option<Arc<native_resources::heightmap_tags::HeightmapTags>>,
    pub provider: P,
    pub classify: C,
    pub resolve: R,
    pub classify_height: H,
    pub terrain_tags: native_resources::carver::tags::CarverBlockTags,
    pub dimension: String,
    pub configure_terrain: F,
    pub worldgen_settings_draft: Option<native_persistence::nbt::NamedTag>,
    pub pack_snapshot: Option<Arc<native_resources::datapack_snapshot::DataPackSnapshot>>,
    pub hut_generation: Option<Arc<crate::fresh_generation::InitialHutGeneration>>,
}

pub struct PostprocessedGeneration {
    pub region: native_session::worldgen::PostprocessedGeneratedRegion,
    pub chunks: BTreeMap<ChunkPos, native_session::worldgen::PostprocessedChunkDraft>,
    /// Exact queues after activation, including their absolute sub-tick order.
    pub activated_ticks: native_session::worldgen::chunk_ticks::ActivatedRegionTicks,
    /// A fresh mesh snapshot for the postprocessed subset of the old targets.
    pub mesh: Option<MeshJob>,
    sealed_world: Arc<native_world::loaded::World>,
    sealed_mesh: Option<MeshJob>,
    initial_bonus_chest: Option<InitialBonusChest>,
}

/// A feature resolved from the same data-pack stack as the generation batch,
/// and the server-level random stream at the start of feature placement.
pub(crate) struct InitialBonusChestRequest {
    pub feature: Option<native_resources::bonus_chest::BonusChestFeature>,
    pub level_random: native_core::random::LegacyRandom,
}

struct InitialBonusChest {
    position: Option<IVec3>,
    level_random: native_core::random::LegacyRandom,
}

impl PostprocessedGeneration {
    /// `None` means no initial bonus was requested; `Some(None)` means the
    /// feature was absent or did not find a valid location.
    pub(crate) fn initial_bonus_chest_position(&self) -> Option<Option<IVec3>> {
        self.initial_bonus_chest
            .as_ref()
            .map(|bonus| bonus.position)
    }
}

#[derive(Clone)]
pub(crate) struct InitialRespawnSettings {
    pub border: native_persistence::world::SavedWorldBorder,
    pub radius: i32,
    pub adventure: bool,
    pub ceiling_spawn_height: Option<i32>,
    pub height_tags: Arc<native_resources::heightmap_tags::HeightmapTags>,
}

/// A border snapshot cannot reveal whether the border is moving. Retain the
/// original saved settings only when they match the checked initial bounds.
fn validated_initial_border(
    border: native_session::respawn::RespawnBorder,
    saved: Option<native_persistence::world::SavedWorldBorder>,
) -> Result<Option<native_persistence::world::SavedWorldBorder>> {
    ensure!(border.is_valid(), "initial spawn border is invalid");
    if let Some(saved) = saved {
        ensure!(
            native_session::respawn::RespawnBorder::from_saved(saved, 0)? == border,
            "saved world border differs from initial spawn bounds"
        );
    }
    Ok(saved)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitialSpawnProgress {
    Pending,
    NeedsChunk(ChunkPos),
    Found(IVec3),
}

/// World rules and independent random seed for the initial player search.
#[derive(Clone, Copy)]
pub struct InitialPlayerSpawnOptions {
    pub radius: i32,
    pub adventure: bool,
    pub random_seed: i64,
    /// Original settings, including border movement; absent means respawn
    /// cannot yet be configured from the evaluated initial edges alone.
    pub saved_border: Option<native_persistence::world::SavedWorldBorder>,
}

/// World rules frozen alongside the create-world settings before starting.
pub(crate) struct StructuredInitialOptions {
    pub border: native_session::respawn::RespawnBorder,
    pub spawn_mobs: bool,
    pub advance_time: bool,
    pub water_source_conversion: bool,
    pub player: InitialPlayerSpawnOptions,
    pub ceiling_spawn_height: Option<i32>,
    #[cfg(test)]
    initial_bonus_seed: Option<i64>,
}

/// A transient player and matching level metadata. This is not a completed
/// world and cannot be saved until chunk and saved-data validation succeeds.
pub struct GeneratedInitialPlayer {
    pub session: native_session::loaded::LoadedSession,
    pub uuid: [i32; 4],
    pub level_data: native_persistence::nbt::NamedTag,
}

/// A staged initial world and its matching, unadvanced local session.
/// The staging tree is temporary and is not yet safe to publish as a world.
pub struct StagedInitialWorld {
    pub initial: GeneratedInitialPlayer,
    pub stage: native_persistence::save::FreshWorldStage,
}

impl GenerationReady {
    /// Stage the exact postprocessed targets and a new player together.
    /// Pack files and remaining generation/save dependencies are not yet
    /// established, so the result must not be published as a playable world.
    pub fn stage_initial_world(
        &mut self,
        parent: &std::path::Path,
        last_played_ms: i64,
        region_timestamp: u32,
    ) -> Result<StagedInitialWorld> {
        use native_persistence::{
            save::FreshWorldDraft,
            world::{DimensionId, SavedClock},
        };
        use native_session::GameSession;
        ensure!(
            self.postprocessed.is_some(),
            "initial world has not been postprocessed"
        );
        let initial = self.start_initial_player_session(last_played_ms)?;
        let snapshot = initial.session.snapshot();
        ensure!(snapshot.tick == 0, "initial session has already advanced");
        let processed = self
            .postprocessed
            .as_ref()
            .context("initial world has not been postprocessed")?;
        ensure!(
            snapshot.game_time == processed.region.ticks().game_time(),
            "initial session time differs from postprocessing"
        );
        let targets = self
            .mesh
            .targets
            .as_ref()
            .context("generated targets are missing")?;
        let regions = self.batch.postprocessed_region_drafts(
            &processed.region,
            targets.iter().copied(),
            region_timestamp,
        )?;
        let settings = self
            .level_settings
            .as_ref()
            .context("initial level settings are missing")?;
        let worldgen = self
            .worldgen_settings_draft
            .as_ref()
            .context("world generation settings are missing")?;
        let clock_data = self
            .initial_clocks
            .as_ref()
            .context("initial world clocks are missing")?;
        let clocks = clock_data
            .ticks
            .iter()
            .map(|(id, &total_ticks)| {
                Ok((
                    DimensionId::parse(&format!("{}:{}", id.namespace, id.path))?,
                    SavedClock {
                        total_ticks,
                        ..SavedClock::default()
                    },
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let player = initial.session.draft_new_player_state()?;
        let mut draft = FreshWorldDraft::new(
            initial.level_data.clone(),
            worldgen.clone(),
            player.document().clone(),
            initial.uuid,
            settings.session.seed,
            clocks,
        )?;
        let dimension = DimensionId::parse(&self.mesh.world.dimension)?;
        for ((_, _, _), region) in regions {
            draft.add_region(dimension.clone(), region)?;
        }
        let mut stage = draft.stage(parent)?;
        match (&settings.packs, &self.pack_snapshot) {
            (Some(_), Some(snapshot)) => stage.install_data_packs(snapshot)?,
            (None, None) => {}
            _ => anyhow::bail!("generation data pack snapshot does not match level settings"),
        }
        Ok(StagedInitialWorld { initial, stage })
    }

    /// Create the initial local player from the checked spawn, retaining one
    /// identity in both the player state and the matching level-data draft.
    pub fn start_initial_player_session(
        &mut self,
        last_played_ms: i64,
    ) -> Result<GeneratedInitialPlayer> {
        if self
            .worldgen_settings_draft
            .as_ref()
            .and_then(|draft| draft.value.get("data"))
            .and_then(|data| data.get("bonus_chest"))
            == Some(&native_persistence::nbt::Tag::Byte(1))
        {
            ensure!(
                self.postprocessed
                    .as_ref()
                    .is_some_and(|processed| processed.initial_bonus_chest.is_some()),
                "initial bonus chest has not been processed"
            );
        }
        let spawn = self.player_spawn.context("initial player spawn missing")?;
        ensure!(
            self.initial_respawn.is_some(),
            "initial player respawn settings missing"
        );
        // Validate the matching settings before generating a player identity.
        self.draft_initial_level_data(last_played_ms, None, true)?;
        let local =
            spawn.as_dvec3() - self.mesh.origin.as_dvec3() + glam::DVec3::new(0.5, 0.0, 0.5);
        let position = local.as_vec3();
        ensure!(
            position.as_dvec3() == local,
            "initial player position cannot be represented exactly"
        );
        let player = native_session::PlayerState {
            position,
            yaw: std::f32::consts::PI,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let mut session = self.start_session(player)?;
        let targets = self
            .mesh
            .targets
            .as_ref()
            .context("generated session targets missing")?;
        ensure!(
            session.generated_spawn_clear(spawn, targets)?,
            "initial player spawn is obstructed"
        );
        let uuid = session.initialize_new_player()?;
        let level_data = self.draft_initial_level_data(last_played_ms, Some(uuid), true)?;
        Ok(GeneratedInitialPlayer {
            session,
            uuid,
            level_data,
        })
    }

    /// The shared world spawn is chosen in the overworld. A preview of another
    /// dimension does not establish or replace the overworld's spawn.
    pub fn initial_world_spawn(&self) -> Result<Option<native_persistence::world::RespawnData>> {
        let Some(position) = self.suggested_spawn else {
            return Ok(None);
        };
        let dimension = native_persistence::world::DimensionId::parse(&self.mesh.world.dimension)?;
        if dimension.namespace != "minecraft" || dimension.path != "overworld" {
            return Ok(None);
        }
        Ok(Some(native_persistence::world::RespawnData {
            dimension,
            position: position.to_array(),
            yaw: 0.0,
            pitch: 0.0,
        }))
    }

    /// Draft metadata using the world spawn actually selected by this batch.
    /// Other world files and FULL chunk conversion remain prerequisites for saving.
    pub fn draft_initial_level_data(
        &self,
        last_played_ms: i64,
        singleplayer_uuid: Option<[i32; 4]>,
        initialized: bool,
    ) -> Result<native_persistence::nbt::NamedTag> {
        use native_persistence::nbt::Tag;
        let settings = self
            .level_settings
            .as_ref()
            .context("initial level settings missing")?;
        let spawn = self
            .initial_world_spawn()?
            .context("initial overworld spawn missing")?;
        let worldgen = self
            .worldgen_settings_draft
            .as_ref()
            .context("world generation settings missing")?;
        ensure!(
            worldgen.value.get("data").and_then(|data| data.get("seed"))
                == Some(&Tag::Long(settings.session.seed)),
            "world generation seed differs from initial level settings"
        );
        settings.draft_level_data(&spawn, last_played_ms, singleplayer_uuid, initialized)
    }

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
        if let Some(initial) = &self.initial_respawn {
            ensure!(
                *initial.height_tags == *tags,
                "spawn heightmap tags differ from initial world generation"
            );
        }
        let mut session = self.start_session_inner(player)?;
        self.install_spawn_heightmaps(&mut session, tags)?;
        self.hut_generation = None;
        Ok(session)
    }

    pub(crate) fn install_spawn_heightmaps(
        &mut self,
        session: &mut native_session::loaded::LoadedSession,
        tags: Arc<native_resources::heightmap_tags::HeightmapTags>,
    ) -> Result<()> {
        let targets = self
            .mesh
            .targets
            .as_ref()
            .context("generated session targets missing")?;
        let mut selected = Vec::with_capacity(targets.len());
        for &pos in targets.iter() {
            let maps = if let Some(processed) = &self.postprocessed {
                Some(
                    processed
                        .region
                        .heightmaps()
                        .get(&pos)
                        .context("postprocessed spawn heightmaps are missing")?
                        .spawn_heightmaps(),
                )
            } else {
                self.batch.spawn_heightmaps(pos)?
            };
            selected.push((pos, maps));
        }
        session.set_spawn_heightmap_tags(tags)?;
        for (pos, maps) in selected {
            if let Some(maps) = maps {
                session.complete_generated_spawn_heightmaps(pos, maps)?;
            } else {
                session.prime_generated_spawn_heightmaps(pos)?;
            }
        }
        Ok(())
    }

    /// Construct an experimental transient session from fully lit targets.
    /// Supported block and fluid ticks use the captured cross-chunk order
    /// while their indexes remain synchronized. This does not create or save
    /// a Java world; the caller must choose and validate the player spawn.
    pub fn start_session(
        &mut self,
        player: native_session::PlayerState,
    ) -> Result<native_session::loaded::LoadedSession> {
        let mut session = self.start_session_inner(player)?;
        if let Some(initial) = &self.initial_respawn {
            let tags = Arc::clone(&initial.height_tags);
            self.install_spawn_heightmaps(&mut session, tags)?;
        }
        self.hut_generation = None;
        Ok(session)
    }

    fn start_session_inner(
        &mut self,
        player: native_session::PlayerState,
    ) -> Result<native_session::loaded::LoadedSession> {
        use native_session::{
            inventory::PlayerInventory,
            loaded::{BuildBounds, LoadedSession},
        };
        let targets = Arc::clone(
            self.mesh
                .targets
                .as_ref()
                .context("generated session requires light-complete targets")?,
        );
        ensure!(!targets.is_empty(), "generated session has no targets");
        ensure!(
            self.batch.is_complete(),
            "generated session has pending generation work"
        );
        if let Some(processed) = &self.postprocessed {
            self.batch.validate_postprocessing(&processed.region)?;
            if self.worldgen_settings_draft.is_some() || processed.initial_bonus_chest.is_some() {
                let settings = self
                    .worldgen_settings_draft
                    .as_ref()
                    .context("initial bonus chest settings are missing")?;
                let bonus_enabled = settings
                    .value
                    .get("data")
                    .and_then(|data| data.get("bonus_chest"))
                    == Some(&native_persistence::nbt::Tag::Byte(1));
                ensure!(
                    bonus_enabled == processed.initial_bonus_chest.is_some(),
                    "initial bonus chest settings changed after postprocessing"
                );
            }
            ensure!(
                Arc::ptr_eq(&processed.region.world, &processed.sealed_world),
                "postprocessed region was replaced"
            );
            if let Some(Some(pos)) = processed.initial_bonus_chest_position() {
                ensure!(
                    processed
                        .region
                        .world
                        .block(pos.x, pos.y, pos.z)
                        .is_some_and(|state| state.name == "minecraft:chest"),
                    "initial bonus chest is missing from postprocessed terrain"
                );
            }
            ensure!(
                processed.activated_ticks == processed.region.ticks().activated_snapshot(),
                "postprocessed tick snapshot changed"
            );
            let mesh = processed
                .mesh
                .as_ref()
                .context("postprocessed mesh is missing")?;
            let sealed = processed
                .sealed_mesh
                .as_ref()
                .context("sealed postprocessed mesh is missing")?;
            ensure!(
                Arc::ptr_eq(&mesh.world, &sealed.world)
                    && mesh.revision == sealed.revision
                    && mesh.biome_blend == sealed.biome_blend
                    && mesh.origin == sealed.origin
                    && mesh.targets == sealed.targets,
                "postprocessed mesh snapshot is stale"
            );
            ensure!(
                mesh.targets.as_ref() == Some(&targets) && mesh.origin == self.mesh.origin,
                "postprocessed session targets or origin differ"
            );
        } else {
            let current = self.batch.lit_render_snapshot(targets.iter().copied())?;
            ensure!(
                Arc::ptr_eq(&current, &self.mesh.world),
                "generated session mesh snapshot is stale"
            );
        }
        let mesh = self
            .postprocessed
            .as_ref()
            .and_then(|p| p.mesh.as_ref())
            .unwrap_or(&self.mesh);
        let (min_y, max_y) = self.batch.build_bounds()?;
        ensure!(
            player.position.is_finite(),
            "invalid generated player position"
        );
        let absolute = player.position.as_dvec3() + mesh.origin.as_dvec3();
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
        let mut world = generated_session_world(&mesh.world, &targets);
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
                if let Some(processed) = &self.postprocessed {
                    processed.region.block_entity_data(pos)
                } else {
                    self.batch.spawned_block_entity_data(pos)
                }
            });
            world.saved_block_entities.insert(pos, payload);
        }
        let initial_random = self
            .postprocessed
            .as_ref()
            .and_then(|processed| processed.initial_bonus_chest.as_ref())
            .map(|bonus| bonus.level_random.clone());
        let mut session = if let Some(random) = initial_random {
            LoadedSession::new_with_initial_level_random(
                world,
                mesh.origin,
                BuildBounds { min_y, max_y },
                player,
                random,
            )
        } else {
            LoadedSession::new(world, mesh.origin, BuildBounds { min_y, max_y }, player)
        }
        .map_err(anyhow::Error::msg)?;
        if let Some(tags) = self.batch.leaf_support_tags() {
            session.set_leaf_support_tags(tags);
        }
        if let Some(processed) = &self.postprocessed {
            let mut selected = native_session::worldgen::chunk_ticks::ActivatedRegionTicks {
                game_time: processed.activated_ticks.game_time,
                next_sub_tick: processed.activated_ticks.next_sub_tick,
                chunks: BTreeMap::new(),
            };
            for pos in targets.iter() {
                selected.chunks.insert(
                    *pos,
                    processed
                        .activated_ticks
                        .chunks
                        .get(pos)
                        .context("postprocessed tick container is missing")?
                        .clone(),
                );
            }
            session.install_generated_ticks(&selected)?;
            let region_ticks = processed.region.ticks();
            let activation: Vec<_> = region_ticks
                .registration_order()
                .iter()
                .copied()
                .filter(|pos| targets.contains(pos))
                .collect();
            let block_order = region_ticks.captured_indexed_queue_for(
                native_session::worldgen::chunk_ticks::TickRegistry::Block,
                &activation,
            )?;
            session.install_generated_block_order(&selected, block_order)?;
            let fluid_order = region_ticks.captured_indexed_queue_for(
                native_session::worldgen::chunk_ticks::TickRegistry::Fluid,
                &activation,
            )?;
            session.install_generated_fluid_order(&selected, fluid_order)?;
            let water = processed
                .region
                .water_source_conversion()
                .context("postprocessed water conversion rule is missing")?;
            session.set_source_conversion(water, false);
        }
        session
            .set_has_skylight(self.batch.has_skylight()?)
            .map_err(anyhow::Error::msg)?;
        if let Some(spawn) = self.initial_world_spawn()? {
            session.set_world_spawn(spawn.clone())?;
            if let Some(initial) = &self.initial_respawn {
                session.set_world_respawn_support(native_session::loaded::WorldRespawnSupport {
                    spawn,
                    border: initial.border,
                    max_health: 20.0,
                    radius: initial.radius,
                    adventure: initial.adventure,
                    ceiling_spawn_height: initial.ceiling_spawn_height,
                })?;
            }
        }
        session.set_new_player_last_death()?;
        session.set_saved_respawn_absent()?;
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
            if let Some(environment) = &initial.fluid_environment.fast_lava {
                session
                    .set_fast_lava_environment(Arc::clone(environment))
                    .map_err(anyhow::Error::msg)?;
            }
            if let Some(environment) = &initial.fluid_environment.water_evaporation {
                session.set_water_evaporation_environment(Arc::clone(environment));
            }
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
    /// Resolve one frozen create-world selection through structure generation,
    /// initial creatures and player spawn. The result remains unsaveable.
    pub(crate) fn start_structured_initial_world(
        frozen: crate::world_creation::FrozenWorldCreation,
        options: StructuredInitialOptions,
        revision: u64,
        biome_blend: u8,
    ) -> Result<Self> {
        frozen.validate()?;
        ensure!(
            frozen.generation.structures_enabled,
            "structures must be enabled"
        );
        ensure!(
            frozen.generation.dimension == "minecraft:overworld",
            "initial world spawn requires the overworld"
        );
        ensure!(
            options.player.adventure
                == (frozen.level.session.mode == native_session::GameMode::Adventure),
            "player spawn game mode differs from frozen settings"
        );
        ensure!(options.player.radius >= 0, "player spawn radius is invalid");
        validated_initial_border(options.border, options.player.saved_border)?;
        Self::spawn(move |cancelled, progress| {
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let crate::world_creation::FrozenWorldCreation { generation, level } = frozen;
            let game = generation.minecraft_dir.clone();
            let preset = generation.preset.clone();
            let dimension = generation.dimension.clone();
            let selection = generation
                .data_packs
                .as_ref()
                .map(|(_, selection)| selection.clone());
            let child = Self::start_selected_structures_spawn_search(
                generation,
                revision,
                biome_blend,
                options.ceiling_spawn_height,
            )?;
            let Some(mut ready) = wait_nested_generation(child, cancelled, progress)? else {
                return Ok(None);
            };
            ensure!(
                ready
                    .worldgen_settings_draft
                    .as_ref()
                    .and_then(|draft| draft.value.get("data"))
                    .and_then(|data| data.get("seed"))
                    == Some(&native_persistence::nbt::Tag::Long(level.session.seed)),
                "generation seed differs from frozen level settings"
            );
            let mut resources = native_resources::packs::ResourceStack::open(&game)?;
            match (&ready.pack_snapshot, &selection) {
                (Some(snapshot), Some(selection)) => {
                    snapshot.verify()?;
                    let warnings =
                        resources.configure_snapshot_data(Arc::clone(snapshot), selection)?;
                    ensure!(
                        warnings.is_empty(),
                        "data pack selection needs attention: {}",
                        warnings.join("; ")
                    );
                }
                (None, None) => {}
                _ => anyhow::bail!("generation data pack snapshot does not match frozen settings"),
            }
            let disabled = if options.spawn_mobs {
                ready.batch.mob_generation_disabled()?
            } else {
                false
            };
            let context = crate::fresh_generation::structured_initial_context(
                &mut resources,
                &preset,
                &dimension,
                options.border,
                options.spawn_mobs,
                disabled,
                options.advance_time,
            )?;
            if let Some(hut) = &ready.hut_generation {
                ensure!(
                    hut.clocks == context.clocks.ticks,
                    "hut generation clocks differ from initial world clocks"
                );
            }
            let registry = Arc::new(resources.vanilla_generated_block_registry()?);
            let (classify, resolve) = crate::fresh_generation::block_classifiers(registry);
            ready.initial_clocks = Some(context.clocks);
            ready.entity_collision_registry = context.collision_registry;
            ready.level_settings = Some(level);
            let child = Self::complete_structured_player_spawn_from_packs(
                ready,
                move || Ok((resources, classify, resolve, context.creatures)),
                #[cfg(test)]
                options.initial_bonus_seed,
                options.spawn_mobs,
                options.water_source_conversion,
                options.border,
                options.player,
                options.ceiling_spawn_height,
                biome_blend,
            )?;
            wait_nested_generation(child, cancelled, progress)
        })
    }

    /// Run the isolated post-generation callbacks and assemble chunk drafts on
    /// the worker, in the caller's explicit chunk order. A failure or
    /// cancellation discards the owned batch and all partial results.
    pub fn postprocess(
        ready: GenerationReady,
        positions: Vec<ChunkPos>,
        game_time: i64,
        next_sub_tick: i64,
        water_source_conversion: bool,
        mesh_revision: u64,
    ) -> Result<Self> {
        Self::postprocess_inner(
            ready,
            positions,
            game_time,
            next_sub_tick,
            water_source_conversion,
            mesh_revision,
            None,
        )
    }

    /// Place the initial bonus only after postprocessing and before player
    /// spawn search. The feature must come from the batch's data-pack stack.
    pub(crate) fn postprocess_with_initial_bonus_chest(
        ready: GenerationReady,
        positions: Vec<ChunkPos>,
        game_time: i64,
        next_sub_tick: i64,
        water_source_conversion: bool,
        mesh_revision: u64,
        bonus: InitialBonusChestRequest,
    ) -> Result<Self> {
        Self::postprocess_inner(
            ready,
            positions,
            game_time,
            next_sub_tick,
            water_source_conversion,
            mesh_revision,
            Some(bonus),
        )
    }

    fn postprocess_inner(
        mut ready: GenerationReady,
        positions: Vec<ChunkPos>,
        game_time: i64,
        next_sub_tick: i64,
        water_source_conversion: bool,
        mesh_revision: u64,
        bonus: Option<InitialBonusChestRequest>,
    ) -> Result<Self> {
        ensure!(!positions.is_empty(), "no postprocessing chunks");
        let unique: BTreeSet<_> = positions.iter().copied().collect();
        ensure!(
            unique.len() == positions.len(),
            "duplicate postprocessing chunk"
        );
        ensure!(
            ready.postprocessed.is_none(),
            "generation was already postprocessed"
        );
        ensure!(
            mesh_revision != ready.mesh.revision,
            "postprocessed mesh needs a new revision"
        );
        let bonus_origin = if bonus.is_some() {
            ensure!(
                ready.player_spawn.is_none(),
                "bonus chest must precede player spawn search"
            );
            ensure!(
                ready
                    .worldgen_settings_draft
                    .as_ref()
                    .and_then(|draft| draft.value.get("data"))
                    .and_then(|data| data.get("bonus_chest"))
                    == Some(&native_persistence::nbt::Tag::Byte(1)),
                "initial world settings do not enable a bonus chest"
            );
            let origin = ready
                .suggested_spawn
                .context("initial world spawn is missing")?;
            ensure!(
                unique.contains(&ChunkPos {
                    x: origin.x.div_euclid(16),
                    z: origin.z.div_euclid(16)
                }),
                "bonus chest spawn chunk is not selected"
            );
            Some(origin)
        } else {
            ensure!(
                ready
                    .worldgen_settings_draft
                    .as_ref()
                    .and_then(|draft| draft.value.get("data"))
                    .and_then(|data| data.get("bonus_chest"))
                    != Some(&native_persistence::nbt::Tag::Byte(1)),
                "bonus chest settings require initial bonus postprocessing"
            );
            None
        };
        Self::spawn(move |cancelled, progress| {
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            set_phase(progress, GenerationPhase::Postprocessing);
            let mut stage = ready.batch.begin_generated_postprocessing(
                positions.iter().copied(),
                game_time,
                next_sub_tick,
            )?;
            stage.set_water_source_conversion(water_source_conversion)?;
            for &pos in &positions {
                loop {
                    if cancelled.load(Ordering::Relaxed) {
                        return Ok(None);
                    }
                    if stage.poll(pos, 256)? {
                        break;
                    }
                }
                let mut state = progress.lock().unwrap_or_else(|error| error.into_inner());
                state.completed_steps = state.completed_steps.saturating_add(1);
            }
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let initial_bonus_chest = if let Some(mut bonus) = bonus {
                let origin = bonus_origin.context("initial bonus chest origin is missing")?;
                let position = match bonus.feature {
                    Some(feature) => stage
                        .place_initial_bonus_chest(
                            &feature,
                            &mut bonus.level_random,
                            origin.to_array(),
                        )?
                        .map(IVec3::from_array),
                    None => None,
                };
                Some(InitialBonusChest {
                    position,
                    level_random: bonus.level_random,
                })
            } else {
                None
            };
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let region = stage.finish()?;
            let activated_ticks = region.ticks().activated_snapshot();
            let chunks = ready.batch.postprocessed_chunk_drafts(&region, positions)?;
            let targets: BTreeSet<_> = ready
                .mesh
                .targets
                .as_deref()
                .context("generated session targets missing")?
                .intersection(&region.chunks)
                .copied()
                .collect();
            let mesh = if targets.is_empty() {
                None
            } else {
                Some(MeshJob::for_postprocessed(
                    &ready.batch,
                    &region,
                    targets,
                    mesh_revision,
                    ready.mesh.biome_blend,
                    ready.mesh.origin,
                )?)
            };
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let sealed_world = Arc::clone(&region.world);
            let sealed_mesh = mesh.clone();
            ready.postprocessed = Some(PostprocessedGeneration {
                region,
                chunks,
                activated_ticks,
                mesh,
                sealed_world,
                sealed_mesh,
                initial_bonus_chest,
            });
            Ok(Some(ready))
        })
    }

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
        Self::start_initial_world_impl(
            prepare,
            revision,
            biome_blend,
            ceiling_spawn_height,
            None,
            None,
        )
    }

    /// Also resolve the initial player's collision-checked position, expanding
    /// terrain on demand using the same data-pack and creature-spawn inputs.
    pub fn start_initial_world_with_player<F>(
        prepare: F,
        revision: u64,
        biome_blend: u8,
        ceiling_spawn_height: Option<i32>,
        player: InitialPlayerSpawnOptions,
    ) -> Result<Self>
    where
        F: FnOnce() -> Result<crate::fresh_generation::PreparedInitialWorld> + Send + 'static,
    {
        Self::start_initial_world_impl(
            prepare,
            revision,
            biome_blend,
            ceiling_spawn_height,
            Some(player),
            None,
        )
    }

    fn start_initial_world_impl<F>(
        prepare: F,
        revision: u64,
        biome_blend: u8,
        ceiling_spawn_height: Option<i32>,
        player: Option<InitialPlayerSpawnOptions>,
        initial_level_random: Option<native_core::random::LegacyRandom>,
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
                bonus_chest_feature,
                creatures,
                spawn_mobs,
                border,
                initial_clocks,
                entity_collision_registry,
                level_settings,
                ..
            } = prepare()?;
            let pack_snapshot = resources.data_pack_snapshot();
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
            ready.pack_snapshot = pack_snapshot;
            let Some(mut ready) = complete_spawn_search(
                ready,
                &mut resources,
                &mut classify,
                &mut resolve,
                Arc::clone(&height_tags),
                None,
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
            if player.is_some() || bonus_chest_feature.is_some() {
                if let Some(feature) = bonus_chest_feature {
                    for &pos in &targets {
                        require_initial_heightmaps(pos, &ready.batch.spawned_heightmap_data(pos)?)?;
                    }
                    let Some(result) = complete_bonus_postprocessing(
                        ready,
                        feature,
                        initial_level_random.unwrap_or_else(native_session::fresh_level_random),
                        &mut resources,
                        &mut classify,
                        &mut resolve,
                        None::<&mut crate::fresh_generation::HeightClassifier>,
                        &mut build,
                        &mut construct,
                        spawn_mobs,
                        true,
                        biome_blend,
                        cancelled,
                        progress,
                    )?
                    else {
                        return Ok(None);
                    };
                    ready = result;
                } else {
                    let next_revision = ready
                        .mesh
                        .revision
                        .checked_add(1)
                        .context("mesh revision overflow")?;
                    let postprocessor = Self::postprocess(
                        ready,
                        targets.iter().copied().collect(),
                        0,
                        0,
                        true,
                        next_revision,
                    )?;
                    let Some(result) = wait_nested_generation(postprocessor, cancelled, progress)?
                    else {
                        return Ok(None);
                    };
                    ready = result;
                }
            }
            if let Some(player) = player {
                ensure!(player.radius >= 0, "player spawn radius is invalid");
                if let Some(level) = &ready.level_settings {
                    ensure!(
                        player.adventure
                            == (level.session.mode == native_session::GameMode::Adventure),
                        "player spawn game mode differs from the frozen world settings"
                    );
                }
                let initial_respawn =
                    validated_initial_border(border, player.saved_border)?.map(|border| {
                        InitialRespawnSettings {
                            border,
                            radius: player.radius,
                            adventure: player.adventure,
                            ceiling_spawn_height,
                            height_tags: Arc::clone(&height_tags),
                        }
                    });
                let Some(mut ready) = complete_player_spawn(
                    ready,
                    &mut resources,
                    &mut classify,
                    &mut resolve,
                    &mut build,
                    &mut construct,
                    spawn_mobs,
                    height_tags,
                    None,
                    biome_blend,
                    ceiling_spawn_height,
                    border,
                    player,
                    cancelled,
                    progress,
                )?
                else {
                    return Ok(None);
                };
                ready.initial_respawn = initial_respawn;
                if let Some(snapshot) = &ready.pack_snapshot {
                    snapshot.verify()?;
                }
                return Ok(Some(ready));
            }
            if let Some(snapshot) = &ready.pack_snapshot {
                snapshot.verify()?;
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

    /// Complete creature and collision-checked player spawning in a
    /// structure-aware intermediate batch, using its original pack snapshot.
    /// The caller's creature builder must use the same world clock settings.
    pub(crate) fn complete_structured_player_spawn<C, R, M, K, F>(
        mut ready: GenerationReady,
        prepare: F,
        #[cfg(test)] initial_bonus_seed: Option<i64>,
        spawn_mobs: bool,
        water_source_conversion: bool,
        border: native_session::respawn::RespawnBorder,
        player: InitialPlayerSpawnOptions,
        ceiling_spawn_height: Option<i32>,
        biome_blend: u8,
    ) -> Result<Self>
    where
        F: FnOnce() -> Result<(native_resources::packs::ResourceStack, C, R, M, K)>
            + Send
            + 'static,
        C: FnMut(&Value) -> Result<CarverBlockState>,
        R: FnMut(&Value) -> Result<BlockState>,
        M: FnMut(
            &FreshTerrainBatch,
            &mut native_resources::packs::ResourceStack,
            ChunkPos,
        ) -> Result<native_session::worldgen::GeneratedSheepSpawner>,
        K: FnMut(
            &native_resources::packs::ResourceId,
        ) -> Result<native_session::worldgen::FreshCreatureConstruction>,
    {
        Self::spawn(move |cancelled, progress| {
            ensure!(
                ready.batch.supports_structure_expansion(),
                "structure expansion is unavailable"
            );
            ensure!(
                ready.player_spawn.is_none(),
                "player spawn was already selected"
            );
            ensure!(player.radius >= 0, "player spawn radius is invalid");
            if let Some(level) = &ready.level_settings {
                ensure!(
                    player.adventure == (level.session.mode == native_session::GameMode::Adventure),
                    "player spawn game mode differs from the frozen world settings"
                );
            }
            let bonus_enabled = match ready
                .worldgen_settings_draft
                .as_ref()
                .and_then(|settings| settings.value.get("data"))
                .and_then(|data| data.get("bonus_chest"))
            {
                Some(native_persistence::nbt::Tag::Byte(0)) => false,
                Some(native_persistence::nbt::Tag::Byte(1)) => true,
                _ => anyhow::bail!("initial bonus chest setting is missing or invalid"),
            };
            let saved_border = validated_initial_border(border, player.saved_border)?;
            let targets = checked_spawn_targets(&mut ready)?;
            ensure!(
                targets
                    .iter()
                    .all(|&pos| ready.batch.schedule().completed(pos)
                        == Some(GenerationStage::Light)),
                "player spawn requires targets before creature spawning"
            );
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let (mut provider, mut classify, mut resolve, mut build, mut construct) = prepare()?;
            let snapshot = provider.data_pack_snapshot();
            ensure!(
                match (&ready.pack_snapshot, &snapshot) {
                    (Some(expected), Some(actual)) => Arc::ptr_eq(expected, actual),
                    (None, None) => true,
                    _ => false,
                },
                "player spawn requires the original data-pack snapshot"
            );
            if let Some(snapshot) = &snapshot {
                snapshot.verify()?;
            }
            let bonus_feature = if bonus_enabled {
                Some(native_resources::bonus_chest::BonusChestFeature::load(
                    &mut provider,
                )?)
            } else {
                None
            };
            let blocks = provider.vanilla_block_ids()?;
            let tags = Arc::new(native_resources::heightmap_tags::HeightmapTags::load(
                &mut provider,
                &blocks,
            )?);
            let registry = Arc::new(provider.vanilla_generated_block_registry()?);
            let mut classify_height =
                crate::fresh_generation::height_classifier(registry, Arc::clone(&tags));
            if ready.entity_collision_registry.is_none() {
                ready.entity_collision_registry = provider
                    .vanilla_entity_collision_registry()
                    .ok()
                    .map(Arc::new);
            }
            if !complete_creature_spawning_with_inputs(
                &mut ready,
                &targets,
                &mut provider,
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
            if bonus_enabled {
                for &pos in &targets {
                    require_initial_heightmaps(pos, &ready.batch.spawned_heightmap_data(pos)?)?;
                }
            }
            let ready = if let Some(feature) = bonus_feature {
                let level_random = {
                    #[cfg(test)]
                    {
                        initial_bonus_seed
                            .map(native_core::random::LegacyRandom::new)
                            .unwrap_or_else(native_session::fresh_level_random)
                    }
                    #[cfg(not(test))]
                    {
                        native_session::fresh_level_random()
                    }
                };
                let Some(result) = complete_bonus_postprocessing(
                    ready,
                    feature,
                    level_random,
                    &mut provider,
                    &mut classify,
                    &mut resolve,
                    Some(&mut classify_height),
                    &mut build,
                    &mut construct,
                    spawn_mobs,
                    water_source_conversion,
                    biome_blend,
                    cancelled,
                    progress,
                )?
                else {
                    return Ok(None);
                };
                result
            } else {
                let next_revision = ready
                    .mesh
                    .revision
                    .checked_add(1)
                    .context("mesh revision overflow")?;
                let postprocessor = Self::postprocess(
                    ready,
                    targets.iter().copied().collect(),
                    0,
                    0,
                    water_source_conversion,
                    next_revision,
                )?;
                let Some(result) = wait_nested_generation(postprocessor, cancelled, progress)?
                else {
                    return Ok(None);
                };
                result
            };
            let Some(mut ready) = complete_player_spawn(
                ready,
                &mut provider,
                &mut classify,
                &mut resolve,
                &mut build,
                &mut construct,
                spawn_mobs,
                Arc::clone(&tags),
                Some(&mut classify_height),
                biome_blend,
                ceiling_spawn_height,
                border,
                player,
                cancelled,
                progress,
            )?
            else {
                return Ok(None);
            };
            ready.initial_respawn = saved_border.map(|border| InitialRespawnSettings {
                border,
                radius: player.radius,
                adventure: player.adventure,
                ceiling_spawn_height,
                height_tags: tags,
            });
            if let Some(snapshot) = snapshot {
                snapshot.verify()?;
            }
            Ok(Some(ready))
        })
    }

    /// Use creature spawning rules resolved from the captured pack stack.
    /// A missing creature configuration is accepted only when the generator
    /// or game rule disables mob generation.
    pub(crate) fn complete_structured_player_spawn_from_packs<C, R, F>(
        ready: GenerationReady,
        prepare: F,
        #[cfg(test)] initial_bonus_seed: Option<i64>,
        spawn_mobs: bool,
        water_source_conversion: bool,
        border: native_session::respawn::RespawnBorder,
        player: InitialPlayerSpawnOptions,
        ceiling_spawn_height: Option<i32>,
        biome_blend: u8,
    ) -> Result<Self>
    where
        F: FnOnce() -> Result<(
                native_resources::packs::ResourceStack,
                C,
                R,
                Option<crate::fresh_generation::PreparedCreatureSpawning>,
            )> + Send
            + 'static,
        C: FnMut(&Value) -> Result<CarverBlockState>,
        R: FnMut(&Value) -> Result<BlockState>,
    {
        Self::complete_structured_player_spawn(
            ready,
            move || {
                let (provider, classify, resolve, spawn) = prepare()?;
                let build = move |batch: &FreshTerrainBatch,
                                  provider: &mut native_resources::packs::ResourceStack,
                                  pos| {
                    spawn
                        .as_ref()
                        .context("creature spawn inputs are unavailable")?
                        .build_spawner(batch, provider, pos)
                };
                let construct = |id: &native_resources::packs::ResourceId| {
                    Ok(
                        native_session::worldgen::FreshCreatureRandom::from_entropy(id)?
                            .construction,
                    )
                };
                Ok((provider, classify, resolve, build, construct))
            },
            #[cfg(test)]
            initial_bonus_seed,
            spawn_mobs,
            water_source_conversion,
            border,
            player,
            ceiling_spawn_height,
            biome_blend,
        )
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

    /// Prepare structure starts and references on the worker before generating
    /// a finite set of lit chunks. The result is intermediate and unsaveable.
    pub(crate) fn start_with_structures<P, C, R, H, K, F>(
        prepare: F,
        targets: BTreeSet<ChunkPos>,
        revision: u64,
        biome_blend: u8,
        origin: IVec3,
    ) -> Result<Self>
    where
        F: FnOnce() -> Result<StructuredGenerationInputs<P, C, R, H, K>> + Send + 'static,
        P: ResourceProvider,
        C: FnMut(&Value) -> Result<CarverBlockState>,
        R: FnMut(&Value) -> Result<BlockState>,
        H: FnMut(&Value) -> Result<native_persistence::heightmap::BlockHeightFlags>,
        K: FnOnce(&mut FreshTerrainBatch, &mut P) -> Result<()>,
    {
        Self::start_structured(
            prepare,
            GenerationTargets::Explicit { targets, origin },
            revision,
            biome_blend,
            None,
        )
    }

    /// Capture the selected data packs on the worker, then prepare the
    /// structure-aware spawn-origin chunk from that same snapshot.
    pub(crate) fn start_selected_structures_at_spawn(
        settings: crate::fresh_generation::FreshGenerationSettings,
        revision: u64,
        biome_blend: u8,
    ) -> Result<Self> {
        Self::start_with_structures_at_spawn(
            move || crate::fresh_generation::prepare_with_structures(settings),
            revision,
            biome_blend,
        )
    }

    /// Find the initial world spawn, expanding structures and terrain on the
    /// same worker and frozen pack stack when the search needs another chunk.
    pub(crate) fn start_selected_structures_spawn_search(
        settings: crate::fresh_generation::FreshGenerationSettings,
        revision: u64,
        biome_blend: u8,
        ceiling_spawn_height: Option<i32>,
    ) -> Result<Self> {
        Self::start_structured(
            move || crate::fresh_generation::prepare_with_structures(settings),
            GenerationTargets::SpawnOrigin,
            revision,
            biome_blend,
            Some(ceiling_spawn_height),
        )
    }

    /// Compute the generator's initial spawn origin before preparing the
    /// structures and lighting needed for that chunk.
    pub(crate) fn start_with_structures_at_spawn<P, C, R, H, K, F>(
        prepare: F,
        revision: u64,
        biome_blend: u8,
    ) -> Result<Self>
    where
        F: FnOnce() -> Result<StructuredGenerationInputs<P, C, R, H, K>> + Send + 'static,
        P: ResourceProvider,
        C: FnMut(&Value) -> Result<CarverBlockState>,
        R: FnMut(&Value) -> Result<BlockState>,
        H: FnMut(&Value) -> Result<native_persistence::heightmap::BlockHeightFlags>,
        K: FnOnce(&mut FreshTerrainBatch, &mut P) -> Result<()>,
    {
        Self::start_structured(
            prepare,
            GenerationTargets::SpawnOrigin,
            revision,
            biome_blend,
            None,
        )
    }

    fn start_structured<P, C, R, H, K, F>(
        prepare: F,
        selection: GenerationTargets,
        revision: u64,
        biome_blend: u8,
        spawn_search: Option<Option<i32>>,
    ) -> Result<Self>
    where
        F: FnOnce() -> Result<StructuredGenerationInputs<P, C, R, H, K>> + Send + 'static,
        P: ResourceProvider,
        C: FnMut(&Value) -> Result<CarverBlockState>,
        R: FnMut(&Value) -> Result<BlockState>,
        H: FnMut(&Value) -> Result<native_persistence::heightmap::BlockHeightFlags>,
        K: FnOnce(&mut FreshTerrainBatch, &mut P) -> Result<()>,
    {
        Self::spawn(move |cancelled, progress| {
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let explicit = match selection {
                GenerationTargets::Explicit { targets, origin } => {
                    let initialized = initialized_neighbors(&targets)?;
                    let plan = StructurePreparationPlan::new(
                        initialized
                            .iter()
                            .copied()
                            .map(|pos| (pos, GenerationStage::InitializeLight)),
                    )?;
                    Some((targets, origin, initialized, plan))
                }
                GenerationTargets::SpawnOrigin => None,
            };
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let StructuredGenerationInputs {
                mut structures,
                terrain_generator,
                height_tags,
                mut provider,
                mut classify,
                mut resolve,
                mut classify_height,
                terrain_tags,
                dimension,
                configure_terrain,
                worldgen_settings_draft,
                pack_snapshot,
                hut_generation,
            } = prepare()?;
            ensure!(
                structures.is_complete(),
                "structure batch has unfinished or failed work"
            );
            let (targets, origin, initialized, plan, spawn_origin) =
                if let Some((targets, origin, initialized, plan)) = explicit {
                    (targets, origin, initialized, plan, None)
                } else {
                    if cancelled.load(Ordering::Relaxed) {
                        return Ok(None);
                    }
                    let spawn = structures.spawn_origin(&mut provider)?;
                    let origin = IVec3::new(
                        spawn.x.checked_mul(16).context("spawn origin X overflow")?,
                        0,
                        spawn.z.checked_mul(16).context("spawn origin Z overflow")?,
                    );
                    let targets = BTreeSet::from([spawn]);
                    let initialized = initialized_neighbors(&targets)?;
                    let plan = StructurePreparationPlan::new(
                        initialized
                            .iter()
                            .copied()
                            .map(|pos| (pos, GenerationStage::InitializeLight)),
                    )?;
                    (targets, origin, initialized, plan, Some(spawn))
                };
            plan.request_structures(&mut structures)?;
            set_phase(progress, GenerationPhase::Generating);
            while let Some((pos, stage)) = {
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                structures.step(&mut provider, &mut classify_height)?
            } {
                note_step(progress, pos, stage);
            }
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let mut batch = if let Some(generator) = terrain_generator {
                plan.into_expandable_terrain_batch(structures, generator, terrain_tags, &dimension)?
            } else {
                plan.into_terrain_batch(structures, terrain_tags, &dimension)?
            };
            configure_terrain(&mut batch, &mut provider)?;
            let Some(mut ready) = generate_targets(
                batch,
                &mut provider,
                &mut classify,
                &mut resolve,
                targets,
                initialized,
                revision,
                biome_blend,
                origin,
                cancelled,
                progress,
                hut_generation,
            )?
            else {
                return Ok(None);
            };
            ready.spawn_origin = spawn_origin;
            ready.worldgen_settings_draft = worldgen_settings_draft;
            ready.pack_snapshot = pack_snapshot;
            if let Some(ceiling_spawn_height) = spawn_search {
                return complete_spawn_search(
                    ready,
                    &mut provider,
                    &mut classify,
                    &mut resolve,
                    height_tags.context("structure spawn height tags are missing")?,
                    Some(&mut classify_height),
                    biome_blend,
                    ceiling_spawn_height,
                    cancelled,
                    progress,
                );
            }
            Ok(Some(ready))
        })
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
                None,
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

    /// Prepare only the additional structure dependencies before extending a
    /// structure-aware batch. The provider must use its original pack snapshot.
    pub(crate) fn expand_with_structures<C, R, H, F>(
        mut ready: GenerationReady,
        target: ChunkPos,
        prepare: F,
        revision: u64,
        biome_blend: u8,
    ) -> Result<Self>
    where
        F: FnOnce() -> Result<(native_resources::packs::ResourceStack, C, R, H)> + Send + 'static,
        C: FnMut(&Value) -> Result<CarverBlockState>,
        R: FnMut(&Value) -> Result<BlockState>,
        H: FnMut(&Value) -> Result<native_persistence::heightmap::BlockHeightFlags>,
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
            let (mut provider, mut classify, mut resolve, mut classify_height) = prepare()?;
            let current_snapshot = provider.data_pack_snapshot();
            ensure!(
                match (&ready.pack_snapshot, &current_snapshot) {
                    (Some(expected), Some(actual)) => Arc::ptr_eq(expected, actual),
                    (None, None) => true,
                    _ => false,
                },
                "structure expansion requires the original data-pack snapshot"
            );
            if !prepare_structure_expansion(
                &mut ready.batch,
                target,
                &mut provider,
                &mut classify_height,
                cancelled,
                progress,
            )? {
                return Ok(None);
            }
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
            let (batch, mut provider, mut classify, mut resolve) = prepare()?;
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
            generate_targets(
                batch,
                &mut provider,
                &mut classify,
                &mut resolve,
                targets,
                initialized,
                revision,
                biome_blend,
                origin,
                cancelled,
                progress,
                None,
            )
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
            player_spawn_search_resumptions: 0,
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

fn require_initial_heightmaps(pos: ChunkPos, maps: &native_persistence::nbt::Tag) -> Result<()> {
    for name in [
        "WORLD_SURFACE",
        "OCEAN_FLOOR",
        "MOTION_BLOCKING",
        "MOTION_BLOCKING_NO_LEAVES",
    ] {
        ensure!(
            maps.get(name).is_some(),
            "initial bonus chest requires generated {name} heightmap for chunk {}, {}",
            pos.x,
            pos.z
        );
    }
    Ok(())
}

fn checked_spawn_targets(ready: &mut GenerationReady) -> Result<BTreeSet<ChunkPos>> {
    ensure!(
        ready.postprocessed.is_none(),
        "postprocessed generation cannot be respawned"
    );
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
        let completed = generation_step(
            &mut ready.batch,
            provider,
            classify,
            resolve,
            ready.hut_generation.as_deref(),
        )?;
        ensure!(
            completed == Some((pos, GenerationStage::Spawn)),
            "unexpected generation step after creature spawning"
        );
        note_step(progress, pos, GenerationStage::Spawn);
    }
    Ok(true)
}

/// Complete an initial bonus attempt without committing a partial placement.
/// Only a neighbor actually requested by the feature is generated and added.
#[allow(clippy::too_many_arguments)]
fn complete_bonus_postprocessing<C, R, H, M, K>(
    mut ready: GenerationReady,
    feature: Option<native_resources::bonus_chest::BonusChestFeature>,
    mut level_random: native_core::random::LegacyRandom,
    provider: &mut native_resources::packs::ResourceStack,
    classify: &mut C,
    resolve: &mut R,
    mut structure_height: Option<&mut H>,
    build_spawner: &mut M,
    construct: &mut K,
    spawn_mobs: bool,
    water_source_conversion: bool,
    biome_blend: u8,
    cancelled: &AtomicBool,
    progress: &Mutex<GenerationProgress>,
) -> Result<Option<GenerationReady>>
where
    C: FnMut(&Value) -> Result<CarverBlockState>,
    R: FnMut(&Value) -> Result<BlockState>,
    H: FnMut(&Value) -> Result<native_persistence::heightmap::BlockHeightFlags>,
    M: FnMut(
        &FreshTerrainBatch,
        &mut native_resources::packs::ResourceStack,
        ChunkPos,
    ) -> Result<native_session::worldgen::GeneratedSheepSpawner>,
    K: FnMut(
        &native_resources::packs::ResourceId,
    ) -> Result<native_session::worldgen::FreshCreatureConstruction>,
{
    use native_session::worldgen::InitialBonusChestAttempt;
    ensure!(
        ready.batch.supports_structure_expansion() == structure_height.is_some(),
        "bonus generation structure inputs do not match the batch"
    );
    let origin = ready
        .suggested_spawn
        .context("initial world spawn is missing")?;
    let mut targets = checked_spawn_targets(&mut ready)?;
    let mut stage = ready
        .batch
        .begin_generated_postprocessing(targets.iter().copied(), 0, 0)?;
    stage.set_water_source_conversion(water_source_conversion)?;
    set_phase(progress, GenerationPhase::Postprocessing);
    for &pos in &targets {
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            if stage.poll(pos, 256)? {
                break;
            }
        }
        let mut state = progress.lock().unwrap_or_else(|error| error.into_inner());
        state.completed_steps = state.completed_steps.saturating_add(1);
    }
    let position = loop {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let Some(feature) = &feature else { break None };
        match stage.try_place_initial_bonus_chest(feature, &mut level_random, origin.to_array())? {
            InitialBonusChestAttempt::Complete(pos) => break pos.map(IVec3::from_array),
            InitialBonusChestAttempt::NeedsChunk(needed) => {
                ensure!(
                    !targets.contains(&needed),
                    "bonus chest requested an already converted chunk"
                );
                let mut completed = stage.finish()?;
                let checkpoint = ready
                    .batch
                    .checkpoint_postprocessing_expansion(&completed)?;
                let revision = ready
                    .mesh
                    .revision
                    .checked_add(1)
                    .context("mesh revision overflow")?;
                if let Some(classify_height) = &mut structure_height {
                    if !prepare_structure_expansion(
                        &mut ready.batch,
                        needed,
                        provider,
                        *classify_height,
                        cancelled,
                        progress,
                    )? {
                        return Ok(None);
                    }
                }
                let Some(mut expanded) = expand_with_inputs(
                    ready,
                    needed,
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
                expanded.suggested_spawn = Some(origin);
                targets = checked_spawn_targets(&mut expanded)?;
                if !complete_creature_spawning_with_inputs(
                    &mut expanded,
                    &targets,
                    provider,
                    classify,
                    resolve,
                    build_spawner,
                    construct,
                    spawn_mobs,
                    cancelled,
                    progress,
                )? {
                    return Ok(None);
                }
                require_initial_heightmaps(
                    needed,
                    &expanded.batch.spawned_heightmap_data(needed)?,
                )?;
                expanded
                    .batch
                    .rebase_postprocessing_after_expansion(&mut completed, &checkpoint)?;
                stage = expanded.batch.resume_generated_postprocessing(&completed)?;
                expanded
                    .batch
                    .register_generated_postprocessing_chunk(&mut stage, needed)?;
                set_phase(progress, GenerationPhase::Postprocessing);
                loop {
                    if cancelled.load(Ordering::Relaxed) {
                        return Ok(None);
                    }
                    if stage.poll(needed, 256)? {
                        break;
                    }
                }
                let mut state = progress.lock().unwrap_or_else(|error| error.into_inner());
                state.completed_steps = state.completed_steps.saturating_add(1);
                drop(state);
                ready = expanded;
            }
        }
    };
    let region = stage.finish()?;
    ensure!(
        targets.is_subset(&region.chunks),
        "bonus postprocessing is missing a target"
    );
    let chunks = ready
        .batch
        .postprocessed_chunk_drafts(&region, region.chunks.iter().copied())?;
    let revision = ready
        .mesh
        .revision
        .checked_add(1)
        .context("mesh revision overflow")?;
    let mesh = MeshJob::for_postprocessed(
        &ready.batch,
        &region,
        targets,
        revision,
        ready.mesh.biome_blend,
        ready.mesh.origin,
    )?;
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    ready.postprocessed = Some(PostprocessedGeneration {
        activated_ticks: region.ticks().activated_snapshot(),
        sealed_world: Arc::clone(&region.world),
        sealed_mesh: Some(mesh.clone()),
        mesh: Some(mesh),
        region,
        chunks,
        initial_bonus_chest: Some(InitialBonusChest {
            position,
            level_random,
        }),
    });
    Ok(Some(ready))
}

/// Resolve the player after spawn-stage entity generation. Resume after
/// expansion only if previously examined terrain and entities still match.
fn complete_player_spawn<P, C, R, M, K>(
    mut ready: GenerationReady,
    provider: &mut P,
    classify: &mut C,
    resolve: &mut R,
    build_spawner: &mut M,
    construct: &mut K,
    spawn_mobs: bool,
    tags: Arc<native_resources::heightmap_tags::HeightmapTags>,
    mut structure_height: Option<
        &mut dyn FnMut(&Value) -> Result<native_persistence::heightmap::BlockHeightFlags>,
    >,
    biome_blend: u8,
    ceiling_spawn_height: Option<i32>,
    border: native_session::respawn::RespawnBorder,
    options: InitialPlayerSpawnOptions,
    cancelled: &AtomicBool,
    progress: &Mutex<GenerationProgress>,
) -> Result<Option<GenerationReady>>
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
    use native_session::{
        loaded::GeneratedPlayerSpawnProgress as SpawnProgress, world_spawn::WorldSpawnCandidates,
    };
    let suggestion = ready
        .suggested_spawn
        .context("world spawn suggestion missing")?;
    ensure!(border.is_valid(), "player spawn border is invalid");
    let x = f64::from(suggestion.x);
    let z = f64::from(suggestion.z);
    ensure!(
        x >= border.min_x && x < border.max_x && z >= border.min_z && z < border.max_z,
        "world spawn outside the border is not supported for fresh player generation"
    );
    let distance = (x - border.min_x)
        .min(border.max_x - x)
        .min(z - border.min_z)
        .min(border.max_z - z);
    let candidates = if options.adventure {
        None
    } else {
        let first = WorldSpawnCandidates::new(suggestion, options.radius, distance, 0)?;
        let mut random = native_core::random::LegacyRandom::new(options.random_seed);
        let offset = random
            .next_int_bound(first.candidate_count())
            .context("invalid player spawn candidate count")?;
        Some(WorldSpawnCandidates::new(
            suggestion,
            options.radius,
            distance,
            offset,
        )?)
    };
    let mut previous_search = None;
    let mut checkpoint: Option<PlayerSearchCheckpoint> = None;
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        set_phase(progress, GenerationPhase::SearchingSpawn);
        let player = native_session::PlayerState {
            position: (suggestion - ready.mesh.origin).as_vec3() + glam::Vec3::new(0.5, 0.0, 0.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let (session, fresh_search) = ready.start_player_spawn_search(
            player,
            Arc::clone(&tags),
            std::iter::empty(),
            candidates.clone(),
        )?;
        let mut search = match (previous_search.take(), checkpoint.take()) {
            (Some(search), Some(previous)) if previous.matches(&ready)? => {
                let mut progress = progress.lock().unwrap_or_else(|e| e.into_inner());
                progress.player_spawn_search_resumptions =
                    progress.player_spawn_search_resumptions.saturating_add(1);
                search
            }
            _ => fresh_search,
        };
        let needed = loop {
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            match ready.poll_player_spawn(&session, &mut search, ceiling_spawn_height, 256)? {
                SpawnProgress::Pending => continue,
                SpawnProgress::Found(pos) => {
                    ready.player_spawn = Some(pos);
                    return Ok(Some(ready));
                }
                SpawnProgress::NeedsTerrain(pos) => break pos,
                SpawnProgress::NeedsEntities(pos) => anyhow::bail!(
                    "generated entity data for spawn chunk {}, {} is incomplete",
                    pos.x,
                    pos.z
                ),
            }
        };
        checkpoint = Some(PlayerSearchCheckpoint::capture(&ready)?);
        previous_search = Some(search);
        drop(session);
        let postprocessed = if let Some(processed) = ready.postprocessed.take() {
            let checkpoint = ready
                .batch
                .checkpoint_postprocessing_expansion(&processed.region)?;
            Some((processed, checkpoint))
        } else {
            None
        };
        let revision = ready
            .mesh
            .revision
            .checked_add(1)
            .context("mesh revision overflow")?;
        if let Some(classify_height) = &mut structure_height {
            if !prepare_structure_expansion(
                &mut ready.batch,
                needed,
                provider,
                classify_height,
                cancelled,
                progress,
            )? {
                return Ok(None);
            }
        }
        let Some(mut expanded) = expand_with_inputs(
            ready,
            needed,
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
        expanded.suggested_spawn = Some(suggestion);
        let targets = checked_spawn_targets(&mut expanded)?;
        if !complete_creature_spawning_with_inputs(
            &mut expanded,
            &targets,
            provider,
            classify,
            resolve,
            build_spawner,
            construct,
            spawn_mobs,
            cancelled,
            progress,
        )? {
            return Ok(None);
        }
        if let Some((mut processed, checkpoint)) = postprocessed {
            expanded
                .batch
                .rebase_postprocessing_after_expansion(&mut processed.region, &checkpoint)?;
            let mut stage = expanded
                .batch
                .resume_generated_postprocessing(&processed.region)?;
            expanded
                .batch
                .register_generated_postprocessing_chunk(&mut stage, needed)?;
            set_phase(progress, GenerationPhase::Postprocessing);
            loop {
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                if stage.poll(needed, 256)? {
                    break;
                }
            }
            let region = stage.finish()?;
            let targets = expanded
                .mesh
                .targets
                .as_deref()
                .context("generated session targets missing")?;
            ensure!(
                targets.is_subset(&region.chunks),
                "expanded postprocessing is missing a target"
            );
            let revision = expanded
                .mesh
                .revision
                .checked_add(1)
                .context("postprocessed mesh revision overflow")?;
            let mesh = MeshJob::for_postprocessed(
                &expanded.batch,
                &region,
                targets.clone(),
                revision,
                expanded.mesh.biome_blend,
                expanded.mesh.origin,
            )?;
            processed.chunks = expanded
                .batch
                .postprocessed_chunk_drafts(&region, region.chunks.iter().copied())?;
            processed.activated_ticks = region.ticks().activated_snapshot();
            processed.sealed_world = Arc::clone(&region.world);
            processed.sealed_mesh = Some(mesh.clone());
            processed.mesh = Some(mesh);
            processed.region = region;
            expanded.postprocessed = Some(processed);
        }
        ready = expanded;
    }
}

fn complete_spawn_search(
    mut ready: GenerationReady,
    provider: &mut impl ResourceProvider,
    classify: &mut impl FnMut(&Value) -> Result<CarverBlockState>,
    resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
    tags: Arc<native_resources::heightmap_tags::HeightmapTags>,
    mut structure_height: Option<
        &mut dyn FnMut(&Value) -> Result<native_persistence::heightmap::BlockHeightFlags>,
    >,
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
        if let Some(classify_height) = &mut structure_height {
            if !prepare_structure_expansion(
                &mut ready.batch,
                target,
                provider,
                classify_height,
                cancelled,
                progress,
            )? {
                return Ok(None);
            }
        }
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

fn prepare_structure_expansion(
    batch: &mut FreshTerrainBatch,
    target: ChunkPos,
    provider: &mut impl ResourceProvider,
    classify_height: &mut impl FnMut(&Value) -> Result<native_persistence::heightmap::BlockHeightFlags>,
    cancelled: &AtomicBool,
    progress: &Mutex<GenerationProgress>,
) -> Result<bool> {
    let initialized = initialized_neighbors(&BTreeSet::from([target]))?;
    let plan = StructurePreparationPlan::new(
        initialized
            .into_iter()
            .map(|pos| (pos, GenerationStage::InitializeLight)),
    )?;
    if cancelled.load(Ordering::Relaxed) {
        return Ok(false);
    }
    batch.request_structure_expansion(&plan)?;
    set_phase(progress, GenerationPhase::Generating);
    while let Some((pos, stage)) = {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(false);
        }
        batch.step_structure_expansion(provider, classify_height)?
    } {
        note_step(progress, pos, stage);
    }
    if cancelled.load(Ordering::Relaxed) {
        return Ok(false);
    }
    batch.finish_structure_expansion(&plan)?;
    Ok(true)
}

/// Search results remain valid only while every previously available target's
/// blocks and the generation-time heightmaps still match. Light-only changes
/// cannot affect the initial column search. This holds the shared terrain
/// snapshot rather than copying its section arrays.
struct InitialSearchCheckpoint {
    world: Arc<native_world::loaded::World>,
    heightmaps: BTreeMap<ChunkPos, Option<native_persistence::heightmap::SpawnHeightmaps>>,
}

/// Player collisions also depend on generated entities. Keep the existing
/// immutable entity records shared and compare their completeness and content.
struct PlayerSearchCheckpoint {
    terrain: InitialSearchCheckpoint,
    postprocessed: bool,
    entities: BTreeMap<ChunkPos, Option<Arc<Vec<native_persistence::nbt::Tag>>>>,
}

impl PlayerSearchCheckpoint {
    fn capture(ready: &GenerationReady) -> Result<Self> {
        let postprocessed = ready.postprocessed.is_some();
        let terrain = if let Some(processed) = &ready.postprocessed {
            let targets = ready
                .mesh
                .targets
                .as_deref()
                .context("search targets are unavailable")?;
            let mut heightmaps = BTreeMap::new();
            for &pos in targets {
                ensure!(
                    processed.region.world.chunks.contains_key(&pos),
                    "search target is missing"
                );
                let maps = processed
                    .region
                    .heightmaps()
                    .get(&pos)
                    .context("postprocessed spawn heightmaps are missing")?;
                heightmaps.insert(pos, Some(maps.spawn_heightmaps()));
            }
            InitialSearchCheckpoint {
                world: Arc::clone(&processed.region.world),
                heightmaps,
            }
        } else {
            InitialSearchCheckpoint::capture(ready)?
        };
        let mut entities = BTreeMap::new();
        for &pos in terrain.heightmaps.keys() {
            entities.insert(
                pos,
                ready.batch.complete_generated_entity_records_shared(pos)?,
            );
        }
        Ok(Self {
            terrain,
            postprocessed,
            entities,
        })
    }

    fn matches(&self, ready: &GenerationReady) -> Result<bool> {
        let targets = ready
            .mesh
            .targets
            .as_deref()
            .context("search targets are unavailable")?;
        if self
            .terrain
            .heightmaps
            .keys()
            .any(|pos| !targets.contains(pos))
        {
            return Ok(false);
        }
        let unchanged = match (self.postprocessed, &ready.postprocessed) {
            (true, Some(processed)) => {
                self.terrain
                    .matches_snapshot(&processed.region.world, |pos| {
                        Ok(Some(
                            processed
                                .region
                                .heightmaps()
                                .get(&pos)
                                .context("postprocessed spawn heightmaps are missing")?
                                .spawn_heightmaps(),
                        ))
                    })?
            }
            (false, None) => self.terrain.matches(&ready.batch)?,
            _ => false,
        };
        if !unchanged {
            return Ok(false);
        }
        self.entities_match(|pos| ready.batch.complete_generated_entity_records_shared(pos))
    }

    fn entities_match(
        &self,
        mut entities: impl FnMut(ChunkPos) -> Result<Option<Arc<Vec<native_persistence::nbt::Tag>>>>,
    ) -> Result<bool> {
        for (&pos, before) in &self.entities {
            let after = entities(pos)?;
            let equal = match (before, &after) {
                (None, None) => true,
                (Some(before), Some(after)) => {
                    Arc::ptr_eq(before, after) || before.as_ref() == after.as_ref()
                }
                _ => false,
            };
            if !equal {
                return Ok(false);
            }
        }
        Ok(true)
    }
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

#[allow(clippy::too_many_arguments)]
fn generate_targets(
    mut batch: FreshTerrainBatch,
    provider: &mut impl ResourceProvider,
    classify: &mut impl FnMut(&Value) -> Result<CarverBlockState>,
    resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
    targets: BTreeSet<ChunkPos>,
    initialized: BTreeSet<ChunkPos>,
    revision: u64,
    biome_blend: u8,
    origin: IVec3,
    cancelled: &AtomicBool,
    progress: &Mutex<GenerationProgress>,
    hut_generation: Option<Arc<crate::fresh_generation::InitialHutGeneration>>,
) -> Result<Option<GenerationReady>> {
    ensure!(
        batch.is_complete() && batch.schedule().requested_chunks() == 0,
        "generation batch must be fresh"
    );
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    batch.request_light_initialization_many(initialized)?;
    set_phase(progress, GenerationPhase::Generating);
    while let Some((pos, stage)) = {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        generation_step(
            &mut batch,
            provider,
            classify,
            resolve,
            hut_generation.as_deref(),
        )?
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
        generation_step(
            &mut batch,
            provider,
            classify,
            resolve,
            hut_generation.as_deref(),
        )?
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
        pack_snapshot: None,
        spawn_origin: None,
        suggested_spawn: None,
        player_spawn: None,
        initial_clocks: None,
        hut_generation,
        entity_collision_registry: None,
        level_settings: None,
        initial_respawn: None,
        postprocessed: None,
    }))
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
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    batch.request_light_initialization_many(initialized_neighbors(&targets)?)?;
    set_phase(progress, GenerationPhase::Generating);
    while let Some((pos, stage)) = {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        generation_step(&mut batch, provider, classify, resolve, None)?
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
        generation_step(&mut batch, provider, classify, resolve, None)?
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
        pack_snapshot: None,
        spawn_origin: Some(spawn),
        suggested_spawn: None,
        player_spawn: None,
        initial_clocks: None,
        hut_generation: None,
        entity_collision_registry: None,
        level_settings: None,
        initial_respawn: None,
        postprocessed: None,
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
        player_spawn: _,
        worldgen_settings_draft,
        pack_snapshot,
        initial_clocks,
        hut_generation,
        entity_collision_registry,
        level_settings,
        initial_respawn,
        postprocessed,
    } = ready;
    ensure!(
        postprocessed.is_none(),
        "postprocessed generation cannot be expanded"
    );
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
        generation_step(
            &mut batch,
            provider,
            classify,
            resolve,
            hut_generation.as_deref(),
        )?
    } {
        note_step(progress, pos, stage);
    }
    set_phase(progress, GenerationPhase::Lighting);
    let mut registrations = Vec::new();
    for &pos in &initialized {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        if batch.schedule().completed(pos) == Some(GenerationStage::Features) {
            registrations.push(pos);
        }
    }
    if !registrations.is_empty() {
        batch.request_light_initialization_many(registrations)?;
    }
    while let Some((pos, stage)) = {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        generation_step(
            &mut batch,
            provider,
            classify,
            resolve,
            hut_generation.as_deref(),
        )?
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
        generation_step(
            &mut batch,
            provider,
            classify,
            resolve,
            hut_generation.as_deref(),
        )?
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
        pack_snapshot,
        spawn_origin,
        suggested_spawn: None,
        player_spawn: None,
        initial_clocks,
        hut_generation,
        entity_collision_registry,
        level_settings,
        initial_respawn,
        postprocessed: None,
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
    hut: Option<&crate::fresh_generation::InitialHutGeneration>,
) -> Result<Option<(ChunkPos, GenerationStage)>> {
    let next = batch.schedule().next_ready();
    let result = if let Some(hut) = hut {
        batch.step_with_hut_clocks(
            provider,
            classify,
            resolve,
            &hut.variants,
            &hut.conditions,
            &hut.moon,
            &hut.clocks,
        )
    } else {
        batch.step(provider, classify, resolve)
    };
    result.with_context(|| format!("generation step {next:?} failed"))
}

impl Drop for GenerationWorker {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn wait_nested_generation(
    worker: GenerationWorker,
    cancelled: &AtomicBool,
    progress: &Mutex<GenerationProgress>,
) -> Result<Option<GenerationReady>> {
    let base = *progress.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if cancelled.load(Ordering::Relaxed) {
            worker.cancel();
        }
        publish_nested_progress(progress, base, worker.progress());
        let result = worker
            .result
            .recv_timeout(std::time::Duration::from_millis(25));
        // The worker may finish between the first snapshot and receiving its result.
        publish_nested_progress(progress, base, worker.progress());
        match result {
            Ok(GenerationResult::Ready(ready)) => return Ok(Some(ready)),
            Ok(GenerationResult::Cancelled) => return Ok(None),
            Ok(GenerationResult::Failed(error)) => return Err(error),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                anyhow::bail!("generation worker stopped unexpectedly")
            }
        }
    }
}

fn publish_nested_progress(
    progress: &Mutex<GenerationProgress>,
    base: GenerationProgress,
    current: GenerationProgress,
) {
    let mut published = progress.lock().unwrap_or_else(|e| e.into_inner());
    let phase = if matches!(
        current.phase,
        GenerationPhase::Ready | GenerationPhase::Cancelled | GenerationPhase::Failed
    ) {
        published.phase
    } else {
        current.phase
    };
    *published = GenerationProgress {
        phase,
        completed_steps: base.completed_steps.saturating_add(current.completed_steps),
        last_step: current.last_step.or(base.last_step),
        spawn_search_resumptions: base
            .spawn_search_resumptions
            .saturating_add(current.spawn_search_resumptions),
        player_spawn_search_resumptions: base
            .player_spawn_search_resumptions
            .saturating_add(current.player_spawn_search_resumptions),
    };
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

    #[test]
    fn nested_generation_propagates_cancellation_and_failure() -> Result<()> {
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
        let child = GenerationWorker::spawn(move |cancelled, progress| {
            set_phase(progress, GenerationPhase::Generating);
            note_step(progress, ChunkPos { x: 0, z: 0 }, GenerationStage::Biomes);
            entered_tx.send(())?;
            while !cancelled.load(Ordering::Relaxed) {
                std::thread::park_timeout(std::time::Duration::from_millis(5));
            }
            Ok(None)
        })?;
        let parent = GenerationWorker::spawn(move |cancelled, progress| {
            progress
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .completed_steps = 7;
            wait_nested_generation(child, cancelled, progress)
        })?;
        entered_rx.recv_timeout(std::time::Duration::from_secs(10))?;
        parent.cancel();
        assert!(matches!(
            parent
                .result
                .recv_timeout(std::time::Duration::from_secs(10))?,
            GenerationResult::Cancelled
        ));
        assert_eq!(parent.progress().phase, GenerationPhase::Cancelled);
        assert_eq!(parent.progress().completed_steps, 8);
        let child = GenerationWorker::spawn(|_, _| anyhow::bail!("nested generation failed"))?;
        let parent = GenerationWorker::spawn(move |cancelled, progress| {
            wait_nested_generation(child, cancelled, progress)
        })?;
        match parent
            .result
            .recv_timeout(std::time::Duration::from_secs(10))?
        {
            GenerationResult::Failed(error) => {
                assert!(format!("{error:#}").contains("nested generation failed"))
            }
            _ => anyhow::bail!("nested generation failure was lost"),
        }
        Ok(())
    }

    #[test]
    fn structured_initial_world_rejects_inconsistent_settings_before_start() {
        let creation = crate::world_creation::WorldCreationSettings::new("World".into());
        let options = || StructuredInitialOptions {
            border: native_session::respawn::RespawnBorder {
                min_x: -100.0,
                max_x: 100.0,
                min_z: -100.0,
                max_z: 100.0,
            },
            spawn_mobs: true,
            advance_time: true,
            water_source_conversion: true,
            player: InitialPlayerSpawnOptions {
                radius: 0,
                adventure: false,
                random_seed: 0,
                saved_border: None,
            },
            ceiling_spawn_height: None,
            initial_bonus_seed: None,
        };
        let mut frozen = creation.freeze(std::path::PathBuf::new(), None);
        frozen.level.session.seed ^= 1;
        assert!(GenerationWorker::start_structured_initial_world(frozen, options(), 1, 0).is_err());
        let mut frozen = creation.freeze(std::path::PathBuf::new(), None);
        frozen.generation.structures_enabled = false;
        assert!(GenerationWorker::start_structured_initial_world(frozen, options(), 1, 0).is_err());
        let frozen = creation.freeze(std::path::PathBuf::new(), None);
        let mut mismatched = options();
        mismatched.player.adventure = true;
        assert!(
            GenerationWorker::start_structured_initial_world(frozen, mismatched, 1, 0).is_err()
        );
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn selected_packs_generate_structures_on_worker() -> Result<()> {
        selected_packs_generate_structures_on_worker_impl(false)
    }

    fn verify_generated_structure_region_with_java(
        game: &std::path::Path,
        parent: &std::path::Path,
        ready: &GenerationReady,
        sources: &BTreeSet<ChunkPos>,
    ) -> Result<()> {
        use native_persistence::save::{FreshRegionDraft, FreshRegionKind};
        use native_resources::packs::ResourceId;
        use std::{fs::OpenOptions, path::PathBuf, process::Command};

        let key = ResourceId::parse("example:treasure")?;
        let (pos, chest) = sources
            .iter()
            .find_map(|&pos| {
                let start = ready.batch.structure_starts(pos)?.get(&key)?;
                let [x, y, z] = start.bounds.min;
                if ready
                    .batch
                    .structure_reference_sources(pos)
                    .ok()?
                    .contains(&pos)
                    && ready.mesh.world.block(x, y, z)?.name == "minecraft:chest"
                {
                    Some((pos, [x, y, z]))
                } else {
                    None
                }
            })
            .context("no generated treasure start with its own reference and chest")?;
        let processed = ready
            .postprocessed
            .as_ref()
            .context("postprocessing missing")?;
        let draft = ready
            .batch
            .postprocessed_chunk_drafts(&processed.region, [pos])?
            .remove(&pos)
            .context("generated treasure chunk missing")?;
        let directory = parent.join("structure-region-verification");
        std::fs::create_dir(&directory)?;
        let rx = pos.x.div_euclid(32);
        let rz = pos.z.div_euclid(32);
        let mut region = FreshRegionDraft::new(FreshRegionKind::Terrain, rx, rz);
        region.add(
            native_persistence::world::ChunkPos { x: pos.x, z: pos.z },
            &draft.chunk,
            1,
        )?;
        let path = directory.join(format!("r.{rx}.{rz}.mca"));
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)?;
        ensure!(
            region.write_staged(&mut file)?.is_empty(),
            "external chunk not expected"
        );
        drop(file);
        std::fs::write(
            directory.join("expected.nbt"),
            native_persistence::nbt::write(&draft.chunk)?,
        )?;

        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
            .join("bin");
        let version = game.join("versions/26.3");
        let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        ensure!(manifest["id"] == "26.3", "wrong Minecraft version");
        let mut paths = vec![version.join("26.3.jar"), directory.clone()];
        for library in manifest["libraries"]
            .as_array()
            .context("libraries missing")?
        {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let classpath = std::env::join_paths(paths)?;
        let source = directory.join("GeneratedStructureRegionOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/GeneratedStructureRegionOracle.java"),
        )?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java oracle did not compile: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(classpath)
            .arg("GeneratedStructureRegionOracle")
            .arg(&directory)
            .arg(pos.x.to_string())
            .arg(pos.z.to_string())
            .args(chest.map(|coordinate| coordinate.to_string()))
            .output()?;
        ensure!(
            output.status.success(),
            "Java could not round-trip generated region: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn generated_structure_region_round_trips_through_java() -> Result<()> {
        selected_packs_generate_structures_on_worker_impl(true)
    }

    fn selected_packs_generate_structures_on_worker_impl(verify_java: bool) -> Result<()> {
        use native_resources::{datapacks::DataPackSettings, packs::ResourceId};
        use serde_json::json;

        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let temp = tempfile::tempdir()?;
        let world = temp.path().join("world");
        let pack = world.join("datapacks/structures");
        let files = [
            (
                "pack.mcmeta",
                json!({"pack":{"description":"test","min_format":121,"max_format":121}}),
            ),
            (
                "data/example/worldgen/biome/test.json",
                json!({"has_precipitation":true,"temperature":0.8,"carvers":[],"features":[]}),
            ),
            (
                "data/example/worldgen/biome/bonus.json",
                json!({"has_precipitation":true,"temperature":0.8,"carvers":[],"features":[]}),
            ),
            (
                "data/example/worldgen/structure/treasure.json",
                json!({"type":"minecraft:buried_treasure","biomes":["example:test"],"step":"surface_structures"}),
            ),
            (
                "data/example/worldgen/structure_set/treasure.json",
                json!({"placement":{"type":"minecraft:random_spread","spacing":1,"separation":0,"salt":0},"structures":[{"structure":"example:treasure","weight":1}]}),
            ),
            (
                "data/example/worldgen/world_preset/test.json",
                json!({"dimensions":{"minecraft:overworld":{
                    "type":{"min_y":80,"height":16,"has_skylight":true,"ambient_light":0.0},
                    "generator":{"type":"minecraft:noise","biome_source":{"type":"minecraft:fixed","biome":"example:test"},"settings":{
                        "noise":{"min_y":80,"height":16},"legacy_random_source":false,"sea_level":88,
                        "default_block":"minecraft:stone","default_fluid":"minecraft:water","spawn_target":[],
                        "material_rule":{"type":"block","result_state":"minecraft:stone"},
                        "noise_router":{"final_density":{"type":"gradient","axis":"y","from_coordinate":80,"to_coordinate":96,"from_value":1,"to_value":-1},
                            "temperature":0,"vegetation":0,"continents":0,"erosion":0,"depth":0,"ridges":0,"chunk_surface_level":88}
                    }}
                }}}),
            ),
        ];
        for (path, contents) in files {
            let path = pack.join(path);
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(path, serde_json::to_vec(&contents)?)?;
        }
        let preset_path = pack.join("data/example/worldgen/world_preset/test.json");
        let mut spawn_preset: Value = serde_json::from_slice(&std::fs::read(&preset_path)?)?;
        spawn_preset["dimensions"]["minecraft:overworld"]["generator"]["settings"]["disable_mob_generation"] =
            json!(true);
        spawn_preset["dimensions"]["minecraft:overworld"]["generator"]["settings"]["noise_router"]
            ["final_density"] = json!({
            "type":"range_choice",
            "input":{"type":"gradient","axis":"x","from_coordinate":0,"to_coordinate":32,"from_value":0,"to_value":32},
            "min_inclusive":20,"max_exclusive":1000000,
            "when_in_range":{"type":"gradient","axis":"y","from_coordinate":80,"to_coordinate":96,"from_value":2,"to_value":-1},
            "when_out_of_range":-1
        });
        std::fs::write(
            preset_path.with_file_name("spawn.json"),
            serde_json::to_vec(&spawn_preset)?,
        )?;
        let mut bonus_preset = spawn_preset.clone();
        bonus_preset["dimensions"]["minecraft:overworld"]["generator"]["biome_source"]["biome"] =
            json!("example:bonus");
        bonus_preset["dimensions"]["minecraft:overworld"]["generator"]["settings"]["sea_level"] =
            json!(80);
        bonus_preset["dimensions"]["minecraft:overworld"]["generator"]["settings"]["noise_router"]
            ["final_density"]["min_inclusive"] = json!(32);
        std::fs::write(
            preset_path.with_file_name("bonus_spawn.json"),
            serde_json::to_vec(&bonus_preset)?,
        )?;
        let selection = DataPackSettings {
            enabled: vec!["vanilla".into(), "file/structures".into()],
            disabled: vec![],
            enabled_features: vec!["minecraft:vanilla".into()],
        };
        let mut creation =
            crate::world_creation::WorldCreationSettings::new("Structured test".into());
        creation.seed = crate::chat_input::ChatInput::field("42");
        creation.preset = ResourceId::parse("example:bonus_spawn")?;
        creation.mode = crate::world_creation::GameMode::Creative;
        creation.bonus_chest = true;
        let frozen = creation.freeze(game.clone(), Some((world.clone(), selection)));
        let saved_border = native_persistence::world::SavedWorldBorder::default();
        let initial_worker = GenerationWorker::start_structured_initial_world(
            frozen,
            StructuredInitialOptions {
                border: native_session::respawn::RespawnBorder::from_saved(saved_border, 0)?,
                spawn_mobs: true,
                advance_time: false,
                water_source_conversion: true,
                player: InitialPlayerSpawnOptions {
                    radius: 0,
                    adventure: false,
                    random_seed: 3,
                    saved_border: Some(saved_border),
                },
                ceiling_spawn_height: None,
                initial_bonus_seed: Some(42),
            },
            50,
            0,
        )?;
        let mut initial_ready = match initial_worker
            .result
            .recv_timeout(std::time::Duration::from_secs(180))?
        {
            GenerationResult::Ready(ready) => ready,
            GenerationResult::Failed(error) => return Err(error),
            GenerationResult::Cancelled => anyhow::bail!("structured initial world cancelled"),
        };
        assert_eq!(
            initial_ready.level_settings.as_ref().unwrap().name,
            "Structured test"
        );
        assert_eq!(
            initial_ready.level_settings.as_ref().unwrap().session.seed,
            42
        );
        assert!(!initial_ready.initial_clocks.as_ref().unwrap().advance_time);
        assert!(initial_ready.postprocessed.is_some());
        let processed = initial_ready.postprocessed.as_ref().unwrap();
        let pos = processed
            .initial_bonus_chest_position()
            .flatten()
            .context("bonus chest was not placed")?;
        assert_eq!(pos.x.rem_euclid(16), 15);
        assert!(
            initial_ready
                .mesh
                .targets
                .as_deref()
                .unwrap()
                .contains(&ChunkPos {
                    x: pos.x.div_euclid(16) + 1,
                    z: pos.z.div_euclid(16),
                })
        );
        assert_eq!(
            processed
                .region
                .world
                .block(pos.x, pos.y, pos.z)
                .unwrap()
                .name,
            "minecraft:chest"
        );
        assert_eq!(
            processed
                .region
                .world
                .block(pos.x + 1, pos.y, pos.z)
                .unwrap()
                .name,
            "minecraft:torch"
        );
        let chest_chunk = processed
            .chunks
            .get(&ChunkPos {
                x: pos.x.div_euclid(16),
                z: pos.z.div_euclid(16),
            })
            .context("bonus chest chunk is missing")?;
        let Some(native_persistence::nbt::Tag::List { values, .. }) =
            chest_chunk.chunk.value.get("block_entities")
        else {
            anyhow::bail!("bonus chest block entity is missing");
        };
        assert!(values.iter().any(|entity| {
            entity.get("x") == Some(&native_persistence::nbt::Tag::Int(pos.x))
                && entity.get("y") == Some(&native_persistence::nbt::Tag::Int(pos.y))
                && entity.get("z") == Some(&native_persistence::nbt::Tag::Int(pos.z))
                && matches!(entity.get("LootTable"), Some(native_persistence::nbt::Tag::String(id)) if id.matches("minecraft:chests/spawn_bonus_chest"))
        }));
        assert!(initial_ready.pack_snapshot.is_some());
        assert!(initial_ready.player_spawn.is_some());
        let initial = initial_ready.start_initial_player_session(123)?;
        assert_eq!(
            native_session::GameSession::snapshot(&initial.session).game_mode,
            native_session::GameMode::Creative
        );
        assert_eq!(
            initial
                .level_data
                .value
                .get("Data")
                .and_then(|data| data.get("GameType")),
            Some(&native_persistence::nbt::Tag::Int(1))
        );
        assert!(
            initial_ready
                .stage_initial_world(temp.path(), 123, 1)
                .is_err()
        );
        let spawn_worker = GenerationWorker::start_selected_structures_spawn_search(
            crate::fresh_generation::FreshGenerationSettings {
                minecraft_dir: game.clone(),
                preset: "example:spawn".into(),
                dimension: "minecraft:overworld".into(),
                seed: 42,
                bonus_chest: false,
                structures_enabled: true,
                data_packs: Some((
                    world.clone(),
                    DataPackSettings {
                        enabled: vec!["vanilla".into(), "file/structures".into()],
                        disabled: vec![],
                        enabled_features: vec!["minecraft:vanilla".into()],
                    },
                )),
            },
            60,
            0,
            None,
        )?;
        let mut spawned = match spawn_worker
            .result
            .recv_timeout(std::time::Duration::from_secs(120))?
        {
            GenerationResult::Ready(ready) => ready,
            GenerationResult::Failed(error) => return Err(error),
            GenerationResult::Cancelled => anyhow::bail!("structure spawn search cancelled"),
        };
        assert_eq!(spawned.spawn_origin, Some(ChunkPos { x: 0, z: 0 }));
        assert!(spawned.mesh.targets.as_deref().unwrap().len() >= 2);
        assert!(spawned.suggested_spawn.context("world spawn missing")?.x >= 16);
        assert!(
            spawned
                .batch
                .structure_starts(ChunkPos { x: 1, z: 0 })
                .unwrap()
                .contains_key(&ResourceId::parse("example:treasure")?)
        );
        let snapshot = Arc::clone(
            spawned
                .pack_snapshot
                .as_ref()
                .context("spawn snapshot missing")?,
        );
        let mut spawn_resources = native_resources::packs::ResourceStack::open(&game)?;
        let selection = DataPackSettings {
            enabled: vec!["vanilla".into(), "file/structures".into()],
            disabled: vec![],
            enabled_features: vec!["minecraft:vanilla".into()],
        };
        ensure!(
            spawn_resources
                .configure_snapshot_data(snapshot, &selection)?
                .is_empty(),
            "data pack selection changed"
        );
        let targets = spawned.mesh.targets.as_deref().unwrap().clone();
        let suggestion = spawned.suggested_spawn.unwrap();
        let origin = IVec3::new(24, suggestion.y, 8);
        spawned.suggested_spawn = Some(origin);
        let radius = 24;
        let first =
            native_session::world_spawn::WorldSpawnCandidates::new(origin, radius, 76.0, 0)?;
        let mut selected = None;
        for seed in 0..10_000 {
            let mut random = native_core::random::LegacyRandom::new(seed);
            let offset = random.next_int_bound(first.candidate_count()).unwrap();
            let (x, z) = native_session::world_spawn::WorldSpawnCandidates::new(
                origin, radius, 76.0, offset,
            )?
            .next()
            .unwrap();
            let chunk = ChunkPos {
                x: x.div_euclid(16),
                z: z.div_euclid(16),
            };
            if x >= 24 && !targets.contains(&chunk) {
                selected = Some((seed, chunk));
                break;
            }
        }
        let (random_seed, expected_chunk) = selected.context("no expansion candidate")?;
        let registry = Arc::new(spawn_resources.vanilla_generated_block_registry()?);
        let player_worker = GenerationWorker::complete_structured_player_spawn_from_packs(
            spawned,
            move || {
                use native_session::worldgen::resolve_generated_terrain_state;
                use native_world::fluid::FluidState;
                let for_classify = Arc::clone(&registry);
                let classify = move |value: &Value| {
                    let block = resolve_generated_terrain_state(value, &for_classify)?;
                    Ok(CarverBlockState {
                        is_air: matches!(
                            block.name.as_str(),
                            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                        ),
                        has_fluid: FluidState::from_block(&block)
                            .map_err(anyhow::Error::msg)?
                            .is_some(),
                    })
                };
                let resolve =
                    move |value: &Value| resolve_generated_terrain_state(value, &registry);
                Ok((spawn_resources, classify, resolve, None))
            },
            None,
            true,
            true,
            native_session::respawn::RespawnBorder {
                min_x: -100.0,
                max_x: 100.0,
                min_z: -100.0,
                max_z: 100.0,
            },
            InitialPlayerSpawnOptions {
                radius,
                adventure: false,
                random_seed,
                saved_border: None,
            },
            None,
            0,
        )?;
        let player_ready = match player_worker
            .result
            .recv_timeout(std::time::Duration::from_secs(120))?
        {
            GenerationResult::Ready(ready) => ready,
            GenerationResult::Failed(error) => return Err(error),
            GenerationResult::Cancelled => anyhow::bail!("structure player spawn cancelled"),
        };
        assert!(player_ready.player_spawn.is_some());
        assert!(
            player_ready
                .mesh
                .targets
                .as_deref()
                .unwrap()
                .contains(&expected_chunk)
        );
        assert!(player_ready.mesh.targets.as_deref().unwrap().len() > targets.len());
        assert!(player_ready.postprocessed.is_some());
        assert!(player_ready.pack_snapshot.as_ref().is_some());
        let processed = player_ready.postprocessed.as_ref().unwrap();
        let selected = player_ready.mesh.targets.as_deref().unwrap();
        let preview = player_ready.batch.preview_structure_regions(
            &processed.region,
            selected.iter().copied(),
            1,
        )?;
        assert_eq!(preview.chunk_count, selected.len());
        assert!(preview.terrain_regions > 0);
        assert!(!preview.referenced_start_chunks.is_empty());
        assert!(preview.referenced_start_chunks.is_subset(selected));
        if verify_java {
            assert_eq!(
                preview,
                player_ready.batch.preview_structure_regions(
                    &processed.region,
                    selected.iter().rev().copied(),
                    1,
                )?
            );
            let duplicate = selected
                .iter()
                .copied()
                .chain(selected.iter().next().copied());
            assert!(
                player_ready
                    .batch
                    .preview_structure_regions(&processed.region, duplicate, 1,)
                    .is_err()
            );
            verify_generated_structure_region_with_java(
                &game,
                temp.path(),
                &player_ready,
                &preview.referenced_start_chunks,
            )?;
            return Ok(());
        }
        assert!(
            player_ready
                .batch
                .postprocessed_region_drafts(&processed.region, selected.iter().copied(), 1,)
                .is_err()
        );
        for pos in targets {
            assert_eq!(
                player_ready.batch.complete_generated_entity_records(pos)?,
                Some([].as_slice())
            );
        }
        let worker = GenerationWorker::start_selected_structures_at_spawn(
            crate::fresh_generation::FreshGenerationSettings {
                minecraft_dir: game.clone(),
                preset: "example:test".into(),
                dimension: "minecraft:overworld".into(),
                seed: 42,
                bonus_chest: false,
                structures_enabled: true,
                data_packs: Some((
                    world,
                    DataPackSettings {
                        enabled: vec!["vanilla".into(), "file/structures".into()],
                        disabled: vec![],
                        enabled_features: vec!["minecraft:vanilla".into()],
                    },
                )),
            },
            71,
            0,
        )?;
        let ready = match worker
            .result
            .recv_timeout(std::time::Duration::from_secs(120))?
        {
            GenerationResult::Ready(ready) => ready,
            GenerationResult::Failed(error) => return Err(error),
            GenerationResult::Cancelled => anyhow::bail!("structure generation cancelled"),
        };
        let target = ChunkPos { x: 0, z: 0 };
        assert_eq!(ready.spawn_origin, Some(target));
        let id = ResourceId::parse("example:treasure")?;
        let start = &ready
            .batch
            .structure_starts(target)
            .context("structure starts missing")?[&id];
        assert_eq!(
            ready
                .mesh
                .world
                .block(
                    start.bounds.min[0],
                    start.bounds.min[1],
                    start.bounds.min[2]
                )
                .unwrap()
                .name,
            "minecraft:chest"
        );
        assert_eq!(
            ready
                .worldgen_settings_draft
                .as_ref()
                .unwrap()
                .value
                .get("data")
                .and_then(|v| v.get("seed")),
            Some(&native_persistence::nbt::Tag::Long(42))
        );
        let snapshot = Arc::clone(
            ready
                .pack_snapshot
                .as_ref()
                .context("pack snapshot missing")?,
        );
        std::fs::write(pack.join("pack.mcmeta"), b"{}")?;
        snapshot.verify()?;
        let staged = tempfile::tempdir()?;
        snapshot.copy_into(staged.path())?;
        let copied = std::fs::read(staged.path().join("datapacks/structures/pack.mcmeta"))?;
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&copied)?["pack"]["description"],
            "test"
        );
        let mismatch_worker = GenerationWorker::start_selected_structures_at_spawn(
            crate::fresh_generation::FreshGenerationSettings {
                minecraft_dir: game.clone(),
                preset: "example:test".into(),
                dimension: "minecraft:overworld".into(),
                seed: 42,
                bonus_chest: false,
                structures_enabled: true,
                data_packs: Some((
                    staged.path().to_path_buf(),
                    DataPackSettings {
                        enabled: vec!["vanilla".into(), "file/structures".into()],
                        disabled: vec![],
                        enabled_features: vec!["minecraft:vanilla".into()],
                    },
                )),
            },
            71,
            0,
        )?;
        let mismatch_ready = match mismatch_worker
            .result
            .recv_timeout(std::time::Duration::from_secs(120))?
        {
            GenerationResult::Ready(ready) => ready,
            GenerationResult::Failed(error) => return Err(error),
            GenerationResult::Cancelled => anyhow::bail!("structure generation cancelled"),
        };
        let wrong_snapshot = Arc::clone(&snapshot);
        let other_game = game.clone();
        let mismatch_worker = GenerationWorker::expand_with_structures(
            mismatch_ready,
            ChunkPos { x: 1, z: 0 },
            move || {
                let mut provider = native_resources::packs::ResourceStack::open(&other_game)?;
                let selection = DataPackSettings {
                    enabled: vec!["vanilla".into(), "file/structures".into()],
                    disabled: vec![],
                    enabled_features: vec!["minecraft:vanilla".into()],
                };
                ensure!(
                    provider
                        .configure_snapshot_data(wrong_snapshot, &selection)?
                        .is_empty(),
                    "data pack selection changed"
                );
                Ok((
                    provider,
                    |_: &Value| -> Result<CarverBlockState> {
                        anyhow::bail!("unexpected block classification")
                    },
                    |_: &Value| -> Result<BlockState> {
                        anyhow::bail!("unexpected block resolution")
                    },
                    |_: &Value| -> Result<native_persistence::heightmap::BlockHeightFlags> {
                        anyhow::bail!("unexpected height classification")
                    },
                ))
            },
            72,
            0,
        )?;
        match mismatch_worker
            .result
            .recv_timeout(std::time::Duration::from_secs(120))?
        {
            GenerationResult::Failed(error) => {
                assert!(format!("{error:#}").contains("original data-pack snapshot"))
            }
            _ => anyhow::bail!("a different data-pack snapshot was accepted"),
        }
        let original_world = Arc::clone(&ready.mesh.world);
        let original_structure = ready.batch.structure_data(target)?;
        let next = ChunkPos { x: 1, z: 0 };
        let comparison_game = game.clone();
        let comparison_world = staged.path().to_path_buf();
        let worker = GenerationWorker::expand_with_structures(
            ready,
            next,
            move || {
                use native_resources::{heightmap_tags::HeightmapTags, packs::ResourceStack};
                use native_session::worldgen::resolve_generated_terrain_state;
                use native_world::fluid::FluidState;
                let mut provider = ResourceStack::open(&game)?;
                let selection = DataPackSettings {
                    enabled: vec!["vanilla".into(), "file/structures".into()],
                    disabled: vec![],
                    enabled_features: vec!["minecraft:vanilla".into()],
                };
                ensure!(
                    provider
                        .configure_snapshot_data(snapshot, &selection)?
                        .is_empty(),
                    "data pack selection changed"
                );
                let blocks = provider.vanilla_block_ids()?;
                let tags = HeightmapTags::load(&mut provider, &blocks)?;
                let registry = Arc::new(provider.vanilla_generated_block_registry()?);
                let for_height = Arc::clone(&registry);
                let height = move |value: &Value| {
                    let block = resolve_generated_terrain_state(value, &for_height)?;
                    let motion = tags.for_block(&block.name)?;
                    Ok(native_persistence::heightmap::BlockHeightFlags {
                        is_air: matches!(
                            block.name.as_str(),
                            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                        ),
                        blocks_motion: motion.blocks_motion,
                        blocks_motion_no_leaves: motion.blocks_motion_no_leaves,
                        has_fluid: FluidState::from_block(&block)
                            .map_err(anyhow::Error::msg)?
                            .is_some(),
                    })
                };
                let for_classify = Arc::clone(&registry);
                let classify = move |value: &Value| {
                    let block = resolve_generated_terrain_state(value, &for_classify)?;
                    Ok(CarverBlockState {
                        is_air: matches!(
                            block.name.as_str(),
                            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                        ),
                        has_fluid: FluidState::from_block(&block)
                            .map_err(anyhow::Error::msg)?
                            .is_some(),
                    })
                };
                let resolve =
                    move |value: &Value| resolve_generated_terrain_state(value, &registry);
                Ok((provider, classify, resolve, height))
            },
            72,
            0,
        )?;
        let expanded = match worker
            .result
            .recv_timeout(std::time::Duration::from_secs(120))?
        {
            GenerationResult::Ready(ready) => ready,
            GenerationResult::Failed(error) => return Err(error),
            GenerationResult::Cancelled => anyhow::bail!("structure expansion cancelled"),
        };
        assert_eq!(
            expanded.mesh.targets.as_deref(),
            Some(&BTreeSet::from([target, next]))
        );
        assert_eq!(expanded.batch.structure_data(target)?, original_structure);
        assert!(
            expanded
                .batch
                .structure_starts(next)
                .context("expanded structure starts missing")?
                .contains_key(&id)
        );
        for y in 80..96 {
            for z in 0..16 {
                for x in 0..16 {
                    assert_eq!(
                        original_world.block(x, y, z),
                        expanded.mesh.world.block(x, y, z)
                    );
                }
            }
        }
        let combined = GenerationWorker::start_with_structures(
            move || {
                crate::fresh_generation::prepare_with_structures(
                    crate::fresh_generation::FreshGenerationSettings {
                        minecraft_dir: comparison_game,
                        preset: "example:test".into(),
                        dimension: "minecraft:overworld".into(),
                        seed: 42,
                        bonus_chest: false,
                        structures_enabled: true,
                        data_packs: Some((
                            comparison_world,
                            DataPackSettings {
                                enabled: vec!["vanilla".into(), "file/structures".into()],
                                disabled: vec![],
                                enabled_features: vec!["minecraft:vanilla".into()],
                            },
                        )),
                    },
                )
            },
            BTreeSet::from([target, next]),
            73,
            0,
            IVec3::ZERO,
        )?;
        let combined = match combined
            .result
            .recv_timeout(std::time::Duration::from_secs(120))?
        {
            GenerationResult::Ready(ready) => ready,
            GenerationResult::Failed(error) => return Err(error),
            GenerationResult::Cancelled => anyhow::bail!("combined structure generation cancelled"),
        };
        for pos in [target, next] {
            assert_eq!(
                expanded.batch.structure_data(pos)?,
                combined.batch.structure_data(pos)?
            );
            for y in 80..96 {
                for z in 0..16 {
                    for x in 0..16 {
                        let (x, z) = (pos.x * 16 + x, pos.z * 16 + z);
                        assert_eq!(
                            expanded.mesh.world.block(x, y, z),
                            combined.mesh.world.block(x, y, z)
                        );
                        for sky in [false, true] {
                            assert_eq!(
                                expanded.mesh.world.light(x, y, z, sky),
                                combined.mesh.world.light(x, y, z, sky)
                            );
                        }
                    }
                }
            }
        }
        assert!(expanded.batch.into_parts().is_err());
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn structured_worker_decorates_hut_using_initial_clocks() -> Result<()> {
        use native_resources::{
            carver::tags::CarverBlockTags,
            cat_variants::CatSpawnVariants,
            dimension_lighting::GeneratedDimensionLighting,
            packs::{ResourceId, ResourceStack},
            structure_placement::StructureSetCache,
        };
        use native_session::worldgen::{TerrainGenerator, resolve_generated_terrain_state};
        use native_world::fluid::FluidState;
        use serde_json::json;
        struct Data;
        impl ResourceProvider for Data {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path.starts_with("data/minecraft/worldgen/noise/") {
                    return Ok(Some(br#"{"base_octave":-4,"octave_count":2}"#.to_vec()));
                }
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
                    "data/minecraft/tags/block/uncarvable.json" => Some(json!({"values":[]})),
                    "data/minecraft/tags/block/blocks_motion_in_heightmap.json"
                    | "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json" => {
                        Some(json!({"values":["minecraft:stone"]}))
                    }
                    "data/minecraft/cat_variant/black.json" => Some(json!({})),
                    "data/example/cat_variant/moon_cat.json" => Some(
                        json!({"spawn_conditions":[{"condition":{"type":"minecraft:moon_brightness","range":{"min":0.9}},"priority":0}]}),
                    ),
                    _ => None,
                };
                value
                    .map(|v| serde_json::to_vec(&v).map_err(Into::into))
                    .transpose()
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                let names: &[&str] = match folder {
                    "worldgen/biome" => &["minecraft:plains"],
                    "worldgen/structure_set" | "worldgen/structure" => &["example:hut"],
                    "cat_variant" => &["minecraft:black", "example:moon_cat"],
                    "cat_sound_variant" => &["minecraft:classic"],
                    _ => &[],
                };
                names.iter().map(|id| ResourceId::parse(id)).collect()
            }
        }
        let set = ResourceId::parse("example:hut")?;
        let [x, z] = StructureSetCache::default()
            .load(&mut Data, &set)?
            .placement
            .potential_chunk(42, [0, 0]);
        let target = ChunkPos { x, z };
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = ResourceStack::open(&game)?;
        let blocks = resources.vanilla_block_ids()?;
        let registry = resources.vanilla_generated_block_registry()?;
        let prepare = move || {
            let mut provider = Data;
            let definition = json!({"type":"minecraft:noise","biome_source":{"type":"minecraft:fixed","biome":"minecraft:plains"},"settings":{
                "noise":{"min_y":0,"height":96},"legacy_random_source":false,"sea_level":48,
                "default_block":"minecraft:stone","default_fluid":"minecraft:water","spawn_target":[],
                "material_rule":{"type":"block","result_state":"minecraft:stone"},
                "noise_router":{"final_density":{"type":"gradient","axis":"y","from_coordinate":0,"to_coordinate":96,"from_value":1,"to_value":-1},"temperature":0,"vegetation":0,"continents":0,"erosion":0,"depth":0,"ridges":0,"chunk_surface_level":48}
            }});
            let generator =
                TerrainGenerator::with_structure_context(&mut provider, &definition, 42, 0, 96)?;
            let structures = StructureGenerationBatch::from_registry(generator, &mut provider)?;
            let terrain_tags = CarverBlockTags::load(&mut provider, &blocks)?;
            let variants = CatSpawnVariants::load_active(&mut provider)?;
            let conditions = variants.prepare_world_conditions(&mut provider)?;
            let clock = ResourceId::parse("example:clock")?;
            let moon = native_resources::moon_phase::Environment {
                base: native_resources::moon_phase::Phase::NewMoon,
                biomes: BTreeMap::new(),
                has_biome_layer: false,
                tracks: vec![native_resources::moon_phase::Track::parse(
                    &json!({"clock":"example:clock","period_ticks":20}),
                    &json!({"keyframes":[{"ticks":0,"value":"full_moon"},{"ticks":10,"value":"new_moon"}]}),
                )?],
            };
            let hut_generation = Some(Arc::new(crate::fresh_generation::InitialHutGeneration {
                variants,
                conditions,
                moon,
                clocks: BTreeMap::from([(clock, 0)]),
            }));
            let registry = Arc::new(registry);
            let for_height = Arc::clone(&registry);
            let classify_height = move |value: &Value| {
                let state = resolve_generated_terrain_state(value, &for_height)?;
                let is_air = matches!(
                    state.name.as_str(),
                    "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                );
                let has_fluid = FluidState::from_block(&state)
                    .map_err(anyhow::Error::msg)?
                    .is_some();
                Ok(native_persistence::heightmap::BlockHeightFlags {
                    is_air,
                    blocks_motion: !is_air && !has_fluid,
                    blocks_motion_no_leaves: !is_air && !has_fluid,
                    has_fluid,
                })
            };
            let for_classify = Arc::clone(&registry);
            let classify = move |value: &Value| {
                let state = resolve_generated_terrain_state(value, &for_classify)?;
                Ok(CarverBlockState {
                    is_air: matches!(
                        state.name.as_str(),
                        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                    ),
                    has_fluid: FluidState::from_block(&state)
                        .map_err(anyhow::Error::msg)?
                        .is_some(),
                })
            };
            let for_resolve = Arc::clone(&registry);
            let resolve = move |value: &Value| resolve_generated_terrain_state(value, &for_resolve);
            let configure_terrain = move |batch: &mut FreshTerrainBatch, provider: &mut Data| {
                let lighting = GeneratedDimensionLighting {
                    dimension: ResourceId::parse("minecraft:overworld")?,
                    dimension_type: None,
                    min_y: 0,
                    height: 96,
                    has_skylight: true,
                    ambient_light: Some(0.0),
                };
                batch.enable_decoration(provider, &BTreeSet::new(), blocks, &lighting)?;
                batch.enable_light_initialization((*registry).clone())
            };
            Ok(StructuredGenerationInputs {
                structures,
                terrain_generator: None,
                height_tags: None,
                provider,
                classify,
                resolve,
                classify_height,
                terrain_tags,
                dimension: "minecraft:overworld".into(),
                configure_terrain,
                worldgen_settings_draft: None,
                pack_snapshot: None,
                hut_generation,
            })
        };
        let worker = GenerationWorker::start_with_structures(
            prepare,
            BTreeSet::from([target]),
            71,
            0,
            IVec3::ZERO,
        )?;
        let mut ready = match worker
            .result
            .recv_timeout(std::time::Duration::from_secs(180))?
        {
            GenerationResult::Ready(ready) => ready,
            GenerationResult::Failed(error) => return Err(error),
            GenerationResult::Cancelled => anyhow::bail!("hut generation cancelled"),
        };
        let mut records = Vec::new();
        for dz in -1..=1 {
            for dx in -1..=1 {
                records.extend(
                    ready
                        .batch
                        .recorded_generated_entities(ChunkPos {
                            x: x + dx,
                            z: z + dz,
                        })?
                        .iter(),
                );
            }
        }
        let cats: Vec<_> = records.iter().filter(|record| matches!(record.get("id"), Some(native_persistence::nbt::Tag::String(id)) if id.matches("minecraft:cat"))).collect();
        assert_eq!(cats.len(), 1);
        assert!(
            matches!(cats[0].get("variant"), Some(native_persistence::nbt::Tag::String(id)) if id.matches("example:moon_cat"))
        );
        assert_eq!(records.len(), 2);
        let structure_data = ready.batch.structure_data(target)?;
        let children = match structure_data
            .get("starts")
            .unwrap()
            .get("example:hut")
            .unwrap()
            .get("Children")
            .unwrap()
        {
            native_persistence::nbt::Tag::List { values, .. } => values,
            _ => anyhow::bail!("hut children are not a list"),
        };
        assert_eq!(
            children[0].get("Witch"),
            Some(&native_persistence::nbt::Tag::Byte(1))
        );
        assert_eq!(
            children[0].get("Cat"),
            Some(&native_persistence::nbt::Tag::Byte(1))
        );
        drop(cats);
        drop(records);
        assert!(ready.hut_generation.is_some());
        ready.start_session(native_session::PlayerState {
            position: glam::Vec3::new((x * 16 + 8) as f32 + 0.5, 80.0, (z * 16 + 8) as f32 + 0.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        })?;
        assert!(ready.hut_generation.is_none());
        assert!(ready.batch.into_parts().is_err());
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn worker_prepares_structures_before_lit_terrain() -> Result<()> {
        use native_resources::{
            carver::tags::CarverBlockTags,
            dimension_lighting::GeneratedDimensionLighting,
            packs::{ResourceId, ResourceStack},
            structure_placement::StructureSetCache,
        };
        use native_session::worldgen::{TerrainGenerator, resolve_generated_terrain_state};
        use native_world::fluid::FluidState;
        use serde_json::json;

        struct Data;
        impl ResourceProvider for Data {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path.starts_with("data/minecraft/worldgen/noise/") {
                    return Ok(Some(br#"{"base_octave":-4,"octave_count":2}"#.to_vec()));
                }
                let value = match path {
                    "data/example/worldgen/structure/treasure.json" => Some(
                        json!({"type":"minecraft:buried_treasure","biomes":["minecraft:plains"],"step":"surface_structures"}),
                    ),
                    "data/example/worldgen/structure_set/treasure.json" => Some(
                        json!({"placement":{"type":"minecraft:random_spread","spacing":1,"separation":0,"salt":0},"structures":[{"structure":"example:treasure","weight":1}]}),
                    ),
                    "data/minecraft/worldgen/biome/plains.json" => Some(
                        json!({"has_precipitation":true,"temperature":0.8,"carvers":[],"features":[]}),
                    ),
                    "data/minecraft/tags/block/uncarvable.json" => Some(json!({"values":[]})),
                    "data/minecraft/tags/block/blocks_motion_in_heightmap.json"
                    | "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json" => {
                        Some(
                            json!({"values":["minecraft:stone","minecraft:sand","minecraft:chest"]}),
                        )
                    }
                    _ => None,
                };
                Ok(value.map(|v| serde_json::to_vec(&v).unwrap()))
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                match folder {
                    "worldgen/biome" => Ok(vec![ResourceId::parse("minecraft:plains")?]),
                    "worldgen/structure_set" | "worldgen/structure" => {
                        Ok(vec![ResourceId::parse("example:treasure")?])
                    }
                    _ => Ok(vec![]),
                }
            }
        }
        let set_id = ResourceId::parse("example:treasure")?;
        let [x, z] = StructureSetCache::default()
            .load(&mut Data, &set_id)?
            .placement
            .potential_chunk(42, [0, 0]);
        let target = ChunkPos { x, z };
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let registry = ResourceStack::open(&game)?.vanilla_generated_block_registry()?;
        let blocks: BTreeSet<_> = registry.ids().map(str::to_owned).collect();
        let pack_source = tempfile::tempdir()?;
        let snapshot = Arc::new(
            native_resources::datapack_snapshot::DataPackSnapshot::capture(pack_source.path())?,
        );
        let worldgen_settings = native_persistence::nbt::NamedTag {
            name: "settings".into(),
            value: native_persistence::nbt::Tag::Long(42),
        };
        assert_eq!(target, ChunkPos { x: 0, z: 0 });
        let mut baseline: Option<(
            Arc<native_world::loaded::World>,
            native_persistence::nbt::Tag,
        )> = None;
        for mode in 0..4 {
            let from_spawn = mode != 0;
            let missing_spawn = mode == 2;
            let cancel_during_setup = mode == 3;
            let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
            let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
            let registry = registry.clone();
            let blocks = blocks.clone();
            let worker_snapshot = Arc::clone(&snapshot);
            let worker_settings = worldgen_settings.clone();
            let prepare = move || {
                if cancel_during_setup {
                    entered_tx.send(())?;
                    release_rx.recv()?;
                }
                let mut provider = Data;
                let mut definition = json!({"type":"minecraft:noise","biome_source":{"type":"minecraft:fixed","biome":"minecraft:plains"},"settings":{
                    "noise":{"min_y":80,"height":16},"legacy_random_source":false,"sea_level":88,
                    "default_block":"minecraft:stone","default_fluid":"minecraft:water","spawn_target":[],
                    "material_rule":{"type":"block","result_state":"minecraft:stone"},
                    "noise_router":{"final_density":{"type":"gradient","axis":"y","from_coordinate":80,"to_coordinate":96,"from_value":1,"to_value":-1},
                        "temperature":0,"vegetation":0,"continents":0,"erosion":0,"depth":0,"ridges":0,"chunk_surface_level":88}
                }});
                if missing_spawn {
                    definition["settings"]
                        .as_object_mut()
                        .unwrap()
                        .remove("spawn_target");
                }
                let generator = TerrainGenerator::with_structure_context(
                    &mut provider,
                    &definition,
                    42,
                    80,
                    16,
                )?;
                let structures = StructureGenerationBatch::from_registry(generator, &mut provider)?;
                let terrain_tags = CarverBlockTags::load(&mut provider, &blocks)?;
                let registry = Arc::new(registry);
                let for_height = Arc::clone(&registry);
                let classify_height = move |value: &Value| {
                    let state = resolve_generated_terrain_state(value, &for_height)?;
                    let is_air = matches!(
                        state.name.as_str(),
                        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                    );
                    let has_fluid = FluidState::from_block(&state)
                        .map_err(anyhow::Error::msg)?
                        .is_some();
                    Ok(native_persistence::heightmap::BlockHeightFlags {
                        is_air,
                        blocks_motion: !is_air && !has_fluid,
                        blocks_motion_no_leaves: !is_air && !has_fluid,
                        has_fluid,
                    })
                };
                let for_classify = Arc::clone(&registry);
                let classify = move |value: &Value| {
                    let state = resolve_generated_terrain_state(value, &for_classify)?;
                    Ok(CarverBlockState {
                        is_air: matches!(
                            state.name.as_str(),
                            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                        ),
                        has_fluid: FluidState::from_block(&state)
                            .map_err(anyhow::Error::msg)?
                            .is_some(),
                    })
                };
                let for_resolve = Arc::clone(&registry);
                let resolve =
                    move |value: &Value| resolve_generated_terrain_state(value, &for_resolve);
                let configure_terrain =
                    move |batch: &mut FreshTerrainBatch, provider: &mut Data| {
                        let lighting = GeneratedDimensionLighting {
                            dimension: ResourceId::parse("minecraft:overworld")?,
                            dimension_type: None,
                            min_y: 80,
                            height: 16,
                            has_skylight: true,
                            ambient_light: Some(0.0),
                        };
                        batch.enable_decoration(provider, &BTreeSet::new(), blocks, &lighting)?;
                        batch.enable_light_initialization((*registry).clone())
                    };
                Ok(StructuredGenerationInputs {
                    structures,
                    terrain_generator: None,
                    height_tags: None,
                    provider,
                    classify,
                    resolve,
                    classify_height,
                    terrain_tags,
                    dimension: "minecraft:overworld".into(),
                    configure_terrain,
                    worldgen_settings_draft: Some(worker_settings),
                    pack_snapshot: Some(worker_snapshot),
                    hut_generation: None,
                })
            };
            let worker = if from_spawn {
                GenerationWorker::start_with_structures_at_spawn(prepare, 71, 0)?
            } else {
                GenerationWorker::start_with_structures(
                    prepare,
                    BTreeSet::from([target]),
                    71,
                    0,
                    IVec3::ZERO,
                )?
            };
            if cancel_during_setup {
                entered_rx.recv_timeout(std::time::Duration::from_secs(10))?;
                worker.cancel();
                release_tx.send(())?;
            }
            let outcome = worker
                .result
                .recv_timeout(std::time::Duration::from_secs(120))?;
            if cancel_during_setup {
                assert!(matches!(outcome, GenerationResult::Cancelled));
                assert_eq!(worker.progress().completed_steps, 0);
                continue;
            }
            if missing_spawn {
                let GenerationResult::Failed(error) = outcome else {
                    anyhow::bail!("missing spawn target was accepted")
                };
                assert!(format!("{error:#}").contains("spawn_target"));
                assert_eq!(worker.progress().completed_steps, 0);
                continue;
            }
            let ready = match outcome {
                GenerationResult::Ready(ready) => ready,
                GenerationResult::Failed(error) => return Err(error),
                GenerationResult::Cancelled => anyhow::bail!("structure generation cancelled"),
            };
            assert_eq!(ready.spawn_origin, from_spawn.then_some(target));
            assert_eq!(
                ready.worldgen_settings_draft.as_ref(),
                Some(&worldgen_settings)
            );
            assert!(Arc::ptr_eq(
                ready.pack_snapshot.as_ref().unwrap(),
                &snapshot
            ));
            ready.pack_snapshot.as_ref().unwrap().verify()?;
            assert_eq!(
                ready.mesh.targets.as_deref(),
                Some(&BTreeSet::from([target]))
            );
            let id = ResourceId::parse("example:treasure")?;
            let start = &ready
                .batch
                .structure_starts(target)
                .context("structure starts missing")?[&id];
            assert_ne!(start.bounds.min[1], 90);
            assert_eq!(
                ready
                    .mesh
                    .world
                    .block(
                        start.bounds.min[0],
                        start.bounds.min[1],
                        start.bounds.min[2]
                    )
                    .unwrap()
                    .name,
                "minecraft:chest"
            );
            let structure_data = ready.batch.structure_data(target)?;
            assert!(
                structure_data
                    .get("starts")
                    .unwrap()
                    .get("example:treasure")
                    .is_some()
            );
            if let Some((world, expected_structures)) = &baseline {
                assert_eq!(&structure_data, expected_structures);
                for y in 80..96 {
                    for z in 0..16 {
                        for x in 0..16 {
                            assert_eq!(ready.mesh.world.block(x, y, z), world.block(x, y, z));
                            for sky in [false, true] {
                                assert_eq!(
                                    ready.mesh.world.light(x, y, z, sky),
                                    world.light(x, y, z, sky)
                                );
                            }
                        }
                    }
                }
            } else {
                baseline = Some((Arc::clone(&ready.mesh.world), structure_data));
            }
            assert!(ready.batch.into_parts().is_err());
        }
        Ok(())
    }

    #[test]
    fn bonus_generation_rejects_every_missing_final_heightmap() {
        use native_persistence::nbt::Tag;
        let pos = ChunkPos { x: -2, z: 3 };
        let names = [
            "WORLD_SURFACE",
            "OCEAN_FLOOR",
            "MOTION_BLOCKING",
            "MOTION_BLOCKING_NO_LEAVES",
        ];
        let all = || {
            names
                .iter()
                .map(|&name| (name.into(), Tag::LongArray(vec![])))
                .collect::<Vec<_>>()
        };
        assert!(require_initial_heightmaps(pos, &Tag::Compound(all())).is_ok());
        for missing in 0..names.len() {
            let mut entries = all();
            entries.remove(missing);
            let error = require_initial_heightmaps(pos, &Tag::Compound(entries)).unwrap_err();
            assert!(error.to_string().contains(names[missing]));
        }
    }
    use std::time::Duration;

    #[test]
    fn initial_respawn_preserves_original_border_and_rejects_mismatched_edges() {
        use native_persistence::world::SavedWorldBorder;
        use native_session::respawn::RespawnBorder;
        for saved in [
            SavedWorldBorder::default(),
            SavedWorldBorder {
                center_x: 13.0,
                center_z: -8.0,
                size: 200.0,
                lerp_target: 200.0,
                ..Default::default()
            },
            SavedWorldBorder {
                center_x: 29_999_900.0,
                center_z: 0.0,
                size: 400.0,
                lerp_target: 400.0,
                ..Default::default()
            },
            SavedWorldBorder {
                size: 200.0,
                lerp_target: 400.0,
                lerp_time: 100,
                ..Default::default()
            },
        ] {
            let bounds = RespawnBorder::from_saved(saved, 0).unwrap();
            assert_eq!(
                validated_initial_border(bounds, Some(saved)).unwrap(),
                Some(saved)
            );
            assert_eq!(validated_initial_border(bounds, None).unwrap(), None);
        }
        assert!(
            validated_initial_border(
                RespawnBorder {
                    min_x: -10.0,
                    max_x: 10.0,
                    min_z: -20.0,
                    max_z: 20.0
                },
                Some(SavedWorldBorder::default())
            )
            .is_err()
        );
        assert!(
            validated_initial_border(
                RespawnBorder {
                    min_x: f64::NAN,
                    max_x: 1.0,
                    min_z: -1.0,
                    max_z: 1.0
                },
                None
            )
            .is_err()
        );
    }

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
    fn resuming_player_search_avoids_rechecking_failed_candidates() -> Result<()> {
        use native_session::world_spawn::{
            ResumableWorldSpawn, SharedSpawnWorld, SpawnColumnHeights, WorldSpawnCandidates,
        };
        struct SearchWorld {
            target: (i32, i32),
            loaded: bool,
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
            fn heights(&mut self, x: i32, z: i32) -> Result<SpawnColumnHeights> {
                self.queries += 1;
                if (x, z) == self.target && !self.loaded {
                    anyhow::bail!("target chunk not loaded");
                }
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
                Ok((pos.x, pos.z) == self.target && pos.y == 0)
            }
            fn clear_player(&mut self, _: IVec3) -> Result<bool> {
                Ok(true)
            }
        }
        let origin = IVec3::new(8, 8, 8);
        let candidates = WorldSpawnCandidates::new(origin, 10, 100.0, 0)?;
        let target = candidates.clone().nth(32).unwrap();
        let mut world = SearchWorld {
            target,
            loaded: false,
            queries: 0,
        };
        let mut search = ResumableWorldSpawn::new(origin, Some(candidates.clone()))?;
        assert!(search.poll(&mut world, 64).is_err());
        assert_eq!(
            (search.requested_position().x, search.requested_position().z),
            target
        );
        world.loaded = true;
        let resumed = search.poll(&mut world, 64)?;
        assert_eq!(resumed, Some(IVec3::new(target.0, 1, target.1)));
        assert_eq!(world.queries, 34);
        let mut restart_world = SearchWorld {
            target,
            loaded: false,
            queries: 0,
        };
        let mut previous = ResumableWorldSpawn::new(origin, Some(candidates.clone()))?;
        assert!(previous.poll(&mut restart_world, 64).is_err());
        restart_world.loaded = true;
        let mut restarted = ResumableWorldSpawn::new(origin, Some(candidates))?;
        assert_eq!(restarted.poll(&mut restart_world, 64)?, resumed);
        assert_eq!(restart_world.queries, 66);
        Ok(())
    }

    #[test]
    fn player_search_checkpoint_detects_changed_or_unknown_entities() -> Result<()> {
        use native_persistence::nbt::Tag;
        use native_world::loaded::World;
        let home = ChunkPos { x: 0, z: 0 };
        let unknown = ChunkPos { x: 1, z: 0 };
        let records = Arc::new(vec![Tag::Compound(vec![(
            "id".into(),
            Tag::String("minecraft:pig".into()),
        )])]);
        let checkpoint = PlayerSearchCheckpoint {
            terrain: InitialSearchCheckpoint {
                world: Arc::new(World::new("minecraft:overworld")),
                heightmaps: BTreeMap::new(),
            },
            postprocessed: false,
            entities: BTreeMap::from([(home, Some(Arc::clone(&records))), (unknown, None)]),
        };
        assert!(checkpoint.entities_match(|pos| Ok((pos == home).then(|| Arc::clone(&records))))?);
        assert!(
            checkpoint.entities_match(|pos| Ok(
                (pos == home).then(|| Arc::new(records.as_ref().clone()))
            ))?
        );
        assert!(!checkpoint.entities_match(|pos| Ok((pos == home).then(|| Arc::new(Vec::new()))))?);
        assert!(!checkpoint.entities_match(|pos| Ok(
            (pos == home).then(|| Arc::new(vec![Tag::Compound(vec![])]))
        ))?);
        assert!(!checkpoint.entities_match(|_| Ok(None))?);
        assert!(!checkpoint.entities_match(|_| Ok(Some(Arc::clone(&records))))?);
        assert!(
            checkpoint
                .entities_match(|_| anyhow::bail!("entity records unavailable"))
                .is_err()
        );
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
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/generation-test-temp");
        std::fs::create_dir_all(&root)?;
        let temp = tempfile::Builder::new()
            .prefix("initial-world-")
            .tempdir_in(root)?;
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
        let dimension = json!({"min_y":0,"height":16,"has_skylight":true,"has_ceiling":false,"ambient_light":0.0,"default_clock":"example:creatures","timelines":["example:creatures"],"attributes":{"minecraft:gameplay/fast_lava":true,"minecraft:gameplay/water_evaporates":true}});
        std::fs::write(
            pack.join("data/example/dimension_type/compact.json"),
            serde_json::to_vec(&dimension)?,
        )?;
        std::fs::write(pack.join("data/example/world_clock/creatures.json"), b"{}")?;
        let timeline = json!({"clock":"example:creatures","period_ticks":20,"time_markers":{"example:wake":7},"tracks":{
            "minecraft:gameplay/creature_world_gen_spawn_probability":{
                "modifier":"override","keyframes":[{"ticks":0,"value":0.75},{"ticks":10,"value":0.0}]
            },
            "minecraft:gameplay/fast_lava":{"modifier":"xor","keyframes":[{"ticks":0,"value":false},{"ticks":2,"value":true}]},
            "minecraft:gameplay/water_evaporates":{"modifier":"xor","keyframes":[{"ticks":0,"value":false},{"ticks":2,"value":true}]}
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
        let mut bonus_preset = preset.clone();
        bonus_preset["dimensions"]["minecraft:overworld"]["generator"]["settings"]["noise_router"]
            ["final_density"] = json!({
                "type":"minecraft:gradient", "axis":"y",
                "from_coordinate":0, "to_coordinate":16, "from_value":1, "to_value":-1
        });
        std::fs::write(
            pack.join("data/example/worldgen/world_preset/bonus.json"),
            serde_json::to_vec(&bonus_preset)?,
        )?;
        let bonus_game = game.clone();
        let bonus_world = world.clone();
        let bonus_worker = GenerationWorker::start_initial_world_impl(
            move || {
                let packs = DataPackSettings {
                    enabled: vec!["vanilla".into(), "file/compact".into()],
                    disabled: vec![],
                    enabled_features: vec!["minecraft:vanilla".into()],
                };
                let prepared = prepare(crate::fresh_generation::FreshGenerationSettings {
                    minecraft_dir: bonus_game,
                    preset: "example:bonus".into(),
                    dimension: "minecraft:overworld".into(),
                    seed: 42,
                    bonus_chest: true,
                    structures_enabled: false,
                    data_packs: Some((bonus_world, packs.clone())),
                })?;
                let mut inputs = prepared.into_new_world_inputs(
                    native_session::respawn::RespawnBorder {
                        min_x: -100.0,
                        max_x: 100.0,
                        min_z: -100.0,
                        max_z: 100.0,
                    },
                    false,
                    true,
                )?;
                inputs.set_level_settings(crate::world_creation::FrozenLevelSettings {
                    name: "Bonus World".into(),
                    session: crate::world_creation::InitialSessionSettings {
                        seed: 42,
                        mode: native_session::GameMode::Survival,
                        difficulty: native_persistence::world::Difficulty::Normal,
                        hardcore: false,
                        allow_commands: false,
                    },
                    packs: Some(packs),
                })?;
                Ok(inputs)
            },
            70,
            0,
            None,
            None,
            Some(native_core::random::LegacyRandom::new(42)),
        )?;
        let bonus_ready = match bonus_worker.result.recv_timeout(Duration::from_secs(120))? {
            GenerationResult::Ready(ready) => ready,
            GenerationResult::Failed(error) => return Err(error),
            GenerationResult::Cancelled => anyhow::bail!("bonus world generation cancelled"),
        };
        let processed = bonus_ready
            .postprocessed
            .as_ref()
            .context("bonus postprocessing missing")?;
        let chest = processed
            .initial_bonus_chest_position()
            .flatten()
            .context("bonus chest missing")?;
        assert_eq!(chest.x.rem_euclid(16), 15);
        let neighbor = ChunkPos {
            x: chest.x.div_euclid(16) + 1,
            z: chest.z.div_euclid(16),
        };
        assert!(
            bonus_ready
                .mesh
                .targets
                .as_deref()
                .unwrap()
                .contains(&neighbor)
        );
        assert_eq!(
            processed
                .region
                .world
                .block(chest.x, chest.y, chest.z)
                .unwrap()
                .name,
            "minecraft:chest"
        );
        assert_eq!(
            processed
                .region
                .world
                .block(chest.x + 1, chest.y, chest.z)
                .unwrap()
                .name,
            "minecraft:torch"
        );
        assert_ne!(
            processed
                .initial_bonus_chest
                .as_ref()
                .unwrap()
                .level_random
                .state(),
            native_core::random::LegacyRandom::new(42).state()
        );
        assert!(bonus_ready.player_spawn.is_none());
        let chunk = processed
            .chunks
            .get(&ChunkPos {
                x: chest.x.div_euclid(16),
                z: chest.z.div_euclid(16),
            })
            .context("bonus chunk draft is missing")?;
        let Some(native_persistence::nbt::Tag::List { values, .. }) =
            chunk.chunk.value.get("block_entities")
        else {
            anyhow::bail!("saved bonus block entities are missing");
        };
        assert!(values.iter().any(|entity| {
            matches!(entity.get("id"), Some(native_persistence::nbt::Tag::String(id)) if id.matches("minecraft:chest"))
                && entity.get("x") == Some(&native_persistence::nbt::Tag::Int(chest.x))
                && entity.get("y") == Some(&native_persistence::nbt::Tag::Int(chest.y))
                && entity.get("z") == Some(&native_persistence::nbt::Tag::Int(chest.z))
                && matches!(entity.get("LootTable"), Some(native_persistence::nbt::Tag::String(id)) if id.matches("minecraft:chests/spawn_bonus_chest"))
        }));
        let original_bonus_entities = values.clone();
        // Force the first player candidate outside the already postprocessed
        // targets so the real spawn worker must extend the same bonus world.
        let targets = bonus_ready.mesh.targets.as_ref().unwrap().as_ref().clone();
        let suggestion = bonus_ready.suggested_spawn.unwrap();
        let radius = 24;
        let border = native_session::respawn::RespawnBorder {
            min_x: -100.0,
            max_x: 100.0,
            min_z: -100.0,
            max_z: 100.0,
        };
        let distance = (f64::from(suggestion.x) - border.min_x)
            .min(border.max_x - f64::from(suggestion.x))
            .min(f64::from(suggestion.z) - border.min_z)
            .min(border.max_z - f64::from(suggestion.z));
        let count = native_session::world_spawn::WorldSpawnCandidates::new(
            suggestion, radius, distance, 0,
        )?
        .candidate_count();
        let random_seed = (0..10_000)
            .find(|&seed| {
                let mut random = native_core::random::LegacyRandom::new(seed);
                let offset = random.next_int_bound(count).unwrap();
                let (x, z) = native_session::world_spawn::WorldSpawnCandidates::new(
                    suggestion, radius, distance, offset,
                )
                .unwrap()
                .next()
                .unwrap();
                !targets.contains(&ChunkPos {
                    x: x.div_euclid(16),
                    z: z.div_euclid(16),
                })
            })
            .context("no seed requests an ungenerated spawn chunk")?;
        let bonus_random_state = processed
            .initial_bonus_chest
            .as_ref()
            .unwrap()
            .level_random
            .state();
        let search_checkpoint = PlayerSearchCheckpoint::capture(&bonus_ready)?;
        let inputs = prepare(crate::fresh_generation::FreshGenerationSettings {
            minecraft_dir: game.clone(),
            preset: "example:bonus".into(),
            dimension: "minecraft:overworld".into(),
            seed: 42,
            bonus_chest: true,
            structures_enabled: false,
            data_packs: Some((
                world.clone(),
                DataPackSettings {
                    enabled: vec!["vanilla".into(), "file/compact".into()],
                    disabled: vec![],
                    enabled_features: vec!["minecraft:vanilla".into()],
                },
            )),
        })?
        .into_new_world_inputs(border, false, true)?;
        let mut resources = inputs.resources;
        let mut classify = inputs.classify;
        let mut resolve = inputs.resolve;
        let mut build =
            |_: &FreshTerrainBatch, _: &mut native_resources::packs::ResourceStack, _: ChunkPos| {
                anyhow::bail!("mob spawning should be disabled")
            };
        let mut construct = |_: &native_resources::packs::ResourceId| {
            anyhow::bail!("mob spawning should be disabled")
        };
        let progress = Mutex::new(GenerationProgress {
            phase: GenerationPhase::Planning,
            completed_steps: 0,
            last_step: None,
            spawn_search_resumptions: 0,
            player_spawn_search_resumptions: 0,
        });
        let mut expanded = complete_player_spawn(
            bonus_ready,
            &mut resources,
            &mut classify,
            &mut resolve,
            &mut build,
            &mut construct,
            false,
            inputs.height_tags,
            None,
            0,
            None,
            border,
            InitialPlayerSpawnOptions {
                radius,
                adventure: false,
                random_seed,
                saved_border: None,
            },
            &AtomicBool::new(false),
            &progress,
        )?
        .context("postprocessed player spawn was cancelled")?;
        assert!(expanded.player_spawn.is_some());
        assert!(expanded.mesh.targets.as_ref().unwrap().len() > targets.len());
        assert!(progress.lock().unwrap().player_spawn_search_resumptions > 0);
        assert!(search_checkpoint.matches(&expanded)?);
        let processed = expanded.postprocessed.as_ref().unwrap();
        assert_eq!(processed.initial_bonus_chest_position(), Some(Some(chest)));
        assert_eq!(
            processed
                .initial_bonus_chest
                .as_ref()
                .unwrap()
                .level_random
                .state(),
            bonus_random_state
        );
        assert_eq!(
            processed
                .region
                .world
                .block(chest.x, chest.y, chest.z)
                .unwrap()
                .name,
            "minecraft:chest"
        );
        let bonus_chunk = ChunkPos {
            x: chest.x.div_euclid(16),
            z: chest.z.div_euclid(16),
        };
        let Some(native_persistence::nbt::Tag::List { values, .. }) = processed.chunks
            [&bonus_chunk]
            .chunk
            .value
            .get("block_entities")
        else {
            anyhow::bail!("expanded bonus block entities are missing");
        };
        assert_eq!(values, &original_bonus_entities);
        assert_eq!(
            processed.chunks.len(),
            expanded.mesh.targets.as_ref().unwrap().len()
        );
        let processed = expanded.postprocessed.as_mut().unwrap();
        let original_world = Arc::clone(&processed.region.world);
        let changed = Arc::make_mut(&mut processed.region.world);
        let air = changed.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: Default::default(),
        });
        changed.set_block_id(chest.x, chest.y, chest.z, air)?;
        assert!(!search_checkpoint.matches(&expanded)?);
        expanded.postprocessed.as_mut().unwrap().region.world = original_world;
        for (spawn_mobs, disabled, species, feature_kind) in [
            (false, false, "minecraft:sheep", 0),
            (true, true, "minecraft:sheep", 0),
            (true, true, "minecraft:sheep", 1),
            (true, true, "minecraft:sheep", 2),
            (true, true, "minecraft:sheep", 3),
            (true, true, "minecraft:sheep", 4),
            (true, false, "minecraft:sheep", 0),
            (true, false, "minecraft:sheep", 1),
            (true, false, "minecraft:sheep", 2),
            (true, false, "minecraft:sheep", 3),
            (true, false, "minecraft:horse", 0),
            (true, false, "minecraft:donkey", 0),
        ] {
            let has_skylight = !disabled || feature_kind % 2 == 0;
            let mut selected_dimension = dimension.clone();
            selected_dimension["has_skylight"] = json!(has_skylight);
            std::fs::write(
                pack.join("data/example/dimension_type/compact.json"),
                serde_json::to_vec(&selected_dimension)?,
            )?;
            let mut selected_biome = biome.clone();
            if feature_kind != 0 {
                let simple = json!({"feature":{"type":"minecraft:simple_block", "to_place":{"id":"minecraft:short_grass"}},"placement":[]});
                let configured = match feature_kind {
                    1 => simple["feature"].clone(),
                    2 => {
                        json!({"type":"minecraft:random_boolean_selector", "feature_true":simple, "feature_false":simple})
                    }
                    3 => json!("minecraft:oak_bees_002"),
                    4 => json!("minecraft:fallen_oak_tree"),
                    _ => unreachable!(),
                };
                selected_biome["features"] = json!([[{
                    "feature": configured,
                    "placement": if feature_kind <= 2 {json!([{"type":"minecraft:offset","x":8,"y":8,"z":8}])} else {json!([{"type":"minecraft:count","count":0}])}
                }]]);
            }
            selected_biome["attributes"]["minecraft:gameplay/natural_mob_spawns"]["spawns_by_category"]
                ["creature"][0]["type"] = json!(species);
            std::fs::write(
                pack.join("data/example/worldgen/biome/flat.json"),
                serde_json::to_vec(&selected_biome)?,
            )?;
            let collision_height_tags = if !spawn_mobs
                || disabled
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
            let prepare_inputs =
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
                };
            let resolve_player = !spawn_mobs
                || (spawn_mobs && !disabled && species == "minecraft:sheep" && feature_kind == 0);
            let worker = if resolve_player {
                GenerationWorker::start_initial_world_with_player(
                    prepare_inputs,
                    80,
                    0,
                    None,
                    InitialPlayerSpawnOptions {
                        radius: 10,
                        adventure: false,
                        random_seed: 1234,
                        saved_border: Some(native_persistence::world::SavedWorldBorder {
                            size: 200.0,
                            lerp_target: 200.0,
                            ..Default::default()
                        }),
                    },
                )?
            } else {
                GenerationWorker::start_initial_world(prepare_inputs, 80, 0, None)?
            };
            let mut ready = match worker.result.recv_timeout(Duration::from_secs(60))? {
                GenerationResult::Ready(ready) => ready,
                GenerationResult::Failed(error) => return Err(error),
                GenerationResult::Cancelled => anyhow::bail!("initial world cancelled"),
            };
            if !spawn_mobs {
                assert!(worker.progress().player_spawn_search_resumptions > 0);
            }
            let level_draft = ready.draft_initial_level_data(123, None, false)?;
            let level_data = level_draft.value.get("Data").unwrap();
            assert_eq!(
                level_data.get("spawn").and_then(|spawn| spawn.get("pos")),
                Some(&native_persistence::nbt::Tag::IntArray(
                    ready.suggested_spawn.unwrap().to_array().to_vec()
                ))
            );
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
            if resolve_player {
                let initial = ready
                    .initial_respawn
                    .as_ref()
                    .context("initial respawn settings missing")?;
                assert_eq!(initial.radius, 10);
                assert_eq!(
                    native_session::respawn::RespawnBorder::from_saved(initial.border, 0)?,
                    native_session::respawn::RespawnBorder {
                        min_x: -100.0,
                        max_x: 100.0,
                        min_z: -100.0,
                        max_z: 100.0
                    }
                );
                let spawn = ready.player_spawn.context("player spawn missing")?;
                assert!(
                    targets.len() > 1,
                    "player search should expand beyond the initial target"
                );
                let suggestion = ready.suggested_spawn.unwrap();
                let distance = (f64::from(suggestion.x) + 100.0)
                    .min(100.0 - f64::from(suggestion.x))
                    .min(f64::from(suggestion.z) + 100.0)
                    .min(100.0 - f64::from(suggestion.z));
                let first = native_session::world_spawn::WorldSpawnCandidates::new(
                    suggestion, 10, distance, 0,
                )?;
                let offset = native_core::random::LegacyRandom::new(1234)
                    .next_int_bound(first.candidate_count())
                    .unwrap();
                let expected = native_session::world_spawn::WorldSpawnCandidates::new(
                    suggestion, 10, distance, offset,
                )?
                .next()
                .unwrap();
                assert_eq!((spawn.x, spawn.z), expected);
                let player = native_session::PlayerState {
                    position: (spawn - ready.mesh.origin).as_vec3()
                        + glam::Vec3::new(0.5, 0.0, 0.5),
                    yaw: 0.0,
                    pitch: 0.0,
                    vertical_velocity: 0.0,
                    on_ground: false,
                    pose: native_session::PlayerPose::Standing,
                };
                let session = ready.start_session_with_spawn_heightmaps(
                    player,
                    Arc::clone(collision_height_tags.as_ref().unwrap()),
                )?;
                assert!(session.generated_spawn_clear(spawn, &targets)?);
                let initial = ready.start_initial_player_session(123)?;
                let player_draft = initial.session.draft_new_player_state()?;
                assert_eq!(
                    native_session::player_data::read_saved_position(player_draft.document())?,
                    Some(spawn.as_dvec3() + glam::DVec3::new(0.5, 0.0, 0.5))
                );
                assert_eq!(
                    player_draft.document().value.get("UUID"),
                    Some(&native_persistence::nbt::Tag::IntArray(
                        initial.uuid.to_vec()
                    ))
                );
                let data = initial.level_data.value.get("Data").unwrap();
                assert_eq!(
                    data.get("singleplayer_uuid"),
                    Some(&native_persistence::nbt::Tag::IntArray(
                        initial.uuid.to_vec()
                    ))
                );
                assert_eq!(
                    data.get("initialized"),
                    Some(&native_persistence::nbt::Tag::Byte(1))
                );
                assert_eq!(
                    data.get("GameType"),
                    Some(&native_persistence::nbt::Tag::Int(1))
                );
                assert_eq!(
                    player_draft.document().value.get("XpSeed"),
                    Some(&native_persistence::nbt::Tag::Int(0))
                );
                let previous = ready.player_spawn.take();
                assert!(ready.start_initial_player_session(123).is_err());
                ready.player_spawn = previous;
                ready.player_spawn = Some(spawn - IVec3::Y);
                assert!(ready.start_initial_player_session(123).is_err());
                ready.player_spawn = previous;
            } else {
                assert!(ready.player_spawn.is_none());
            }
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
                    true
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
                use native_session::GameSession;
                assert_eq!(
                    session.snapshot().world_spawn.as_ref().unwrap().position,
                    ready.suggested_spawn.unwrap().to_array()
                );
                assert_eq!(
                    session.snapshot().last_death,
                    native_session::LastDeathLocation::Absent
                );
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
                use native_session::GameSession;
                session.set_precipitation_environment(Arc::new(BTreeMap::new()), None);
                session
                    .publish_initial_weather_terrain()
                    .map_err(anyhow::Error::msg)?;
                assert_eq!(
                    session
                        .snapshot()
                        .weather_terrain
                        .as_ref()
                        .unwrap()
                        .has_skylight,
                    Some(has_skylight)
                );
                if ready.initial_respawn.is_none() {
                    session.set_spawn_heightmap_tags(Arc::clone(
                        collision_height_tags.as_ref().unwrap(),
                    ))?;
                }
                assert!(targets.iter().all(|&pos| matches!(
                    session.saved_entity_chunk(pos),
                    Some(SavedEntityData::Empty)
                )));
                assert!(session.generated_spawn_clear(position, &targets)?);
                assert!(session.fast_lava_enabled());
                assert_eq!(session.water_evaporates(), Some(true));
                session.update(Duration::from_millis(100));
                assert!(!session.fast_lava_enabled());
                assert_eq!(session.water_evaporates(), Some(false));
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
                if ready.initial_respawn.is_none() {
                    without_registry.set_spawn_heightmap_tags(Arc::clone(
                        collision_height_tags.as_ref().unwrap(),
                    ))?;
                }
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
                if ready.initial_respawn.is_none() {
                    session.set_spawn_heightmap_tags(collision_height_tags.unwrap())?;
                }
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
            if !spawn_mobs {
                let automatic = ready
                    .postprocessed
                    .as_ref()
                    .context("initial player worker did not postprocess its targets")?;
                assert_eq!(automatic.region.chunks, targets);
                assert_eq!(automatic.initial_bonus_chest_position(), None);
                let staged = ready.stage_initial_world(temp.path(), 123, 1)?;
                assert!(staged.stage.path().join("level.dat").is_file());
                drop(staged);
                // The remaining checks exercise an independent, manually
                // assembled pass against the original protochunk batch.
                ready.postprocessed.take();
                let positions: Vec<_> = targets.iter().copied().collect();
                assert!(positions.len() >= 2);
                let mut incremental =
                    ready
                        .batch
                        .begin_generated_postprocessing([positions[0]], 0, 0)?;
                assert!(
                    ready
                        .batch
                        .register_generated_postprocessing_chunk(&mut incremental, positions[1])
                        .is_err()
                );
                while !incremental.poll(positions[0], 256)? {}
                let first_result = incremental.finish()?;
                let mut incremental = ready.batch.resume_generated_postprocessing(&first_result)?;
                assert!(incremental.poll(positions[0], 256).is_err());
                ready
                    .batch
                    .register_generated_postprocessing_chunk(&mut incremental, positions[1])?;
                assert!(
                    ready
                        .batch
                        .register_generated_postprocessing_chunk(&mut incremental, positions[1])
                        .is_err()
                );
                while !incremental.poll(positions[1], 256)? {}
                let incremental = incremental.finish()?;
                ready.batch.validate_postprocessing(&incremental)?;
                assert_eq!(first_result.ticks().registration_order(), &positions[..1]);
                assert_eq!(first_result.chunks.len(), 1);
                assert_eq!(incremental.ticks().registration_order(), &positions[..2]);
                assert_eq!(incremental.chunks, positions[..2].iter().copied().collect());
                let drafts = ready
                    .batch
                    .postprocessed_chunk_drafts(&incremental, positions[..2].iter().copied())?;
                for pos in &positions[..2] {
                    let draft = &drafts[pos].chunk;
                    assert_eq!(
                        draft.value.get("Status"),
                        Some(&native_persistence::nbt::Tag::String(
                            "minecraft:full".into()
                        ))
                    );
                    assert!(draft.value.get("Heightmaps").is_some());
                    assert!(draft.value.get("block_entities").is_some());
                }
                let next_revision = ready
                    .mesh
                    .revision
                    .checked_add(1)
                    .context("mesh revision overflow")?;
                // Exercise the intermediate worker with an absent feature:
                // this must preserve the level RNG and happen before player
                // spawn search. Restore the original fixture afterwards.
                let original_spawn = ready.player_spawn.take();
                let original_worldgen = ready.worldgen_settings_draft.clone();
                use native_persistence::nbt::Tag;
                let Tag::Compound(root) =
                    &mut ready.worldgen_settings_draft.as_mut().unwrap().value
                else {
                    panic!("worldgen root is not a compound");
                };
                let Tag::Compound(data) = &mut root
                    .iter_mut()
                    .find(|(name, _)| name.matches("data"))
                    .unwrap()
                    .1
                else {
                    panic!("worldgen data is not a compound");
                };
                data.iter_mut()
                    .find(|(name, _)| name.matches("bonus_chest"))
                    .unwrap()
                    .1 = Tag::Byte(1);
                let random = native_core::random::LegacyRandom::new(1234);
                let expected_state = random.state();
                let worker = GenerationWorker::postprocess_with_initial_bonus_chest(
                    ready,
                    positions,
                    0,
                    0,
                    true,
                    next_revision,
                    InitialBonusChestRequest {
                        feature: None,
                        level_random: random,
                    },
                )?;
                let mut ready = match worker.result.recv_timeout(Duration::from_secs(120))? {
                    GenerationResult::Ready(ready) => ready,
                    GenerationResult::Failed(error) => return Err(error),
                    GenerationResult::Cancelled => anyhow::bail!("postprocessing cancelled"),
                };
                let processed = ready.postprocessed.as_mut().unwrap();
                assert_eq!(processed.initial_bonus_chest_position(), Some(None));
                assert_eq!(
                    processed
                        .initial_bonus_chest
                        .as_ref()
                        .unwrap()
                        .level_random
                        .state(),
                    expected_state
                );
                ready.player_spawn = original_spawn;
                ready.start_initial_player_session(123)?;
                ready.worldgen_settings_draft = original_worldgen;
                assert!(ready.start_initial_player_session(123).is_err());
                ready.postprocessed.as_mut().unwrap().initial_bonus_chest = None;
                let source_biome = pack.join("data/example/worldgen/biome/flat.json");
                let expected_biome = std::fs::read(&source_biome)?;
                std::fs::write(&source_biome, br#"{"changed_after_generation":true}"#)?;
                let staged = ready.stage_initial_world(temp.path(), 123, 1)?;
                let staged_biome = staged
                    .stage
                    .path()
                    .join("datapacks/compact/data/example/worldgen/biome/flat.json");
                assert_eq!(std::fs::read(staged_biome)?, expected_biome);
                std::fs::write(&source_biome, &expected_biome)?;
                let mut reader = native_persistence::world::WorldReader::open(
                    staged.stage.path(),
                    native_persistence::world::DimensionId::parse("overworld")?,
                )?;
                assert_eq!(reader.singleplayer_uuid_words()?, Some(staged.initial.uuid));
                assert_eq!(reader.saved_world_seed()?, Some(42));
                let spawn = reader.world_spawn()?;
                assert!(
                    reader
                        .read_chunk(
                            spawn.position[0].div_euclid(16),
                            spawn.position[2].div_euclid(16)
                        )?
                        .is_some()
                );
                assert_eq!(
                    reader.read_player()?.unwrap().value.get("UUID"),
                    Some(&native_persistence::nbt::Tag::IntArray(
                        staged.initial.uuid.to_vec()
                    ))
                );
                let mut staged_resources = native_resources::packs::ResourceStack::open(
                    &std::env::var_os("MINECRAFT_DIR")
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap()),
                )?;
                let saved_packs = reader.data_pack_settings()?;
                assert!(
                    staged_resources
                        .configure_world_data(
                            staged.stage.path(),
                            &native_resources::datapacks::DataPackSettings {
                                enabled: saved_packs.enabled,
                                disabled: saved_packs.disabled,
                                enabled_features: saved_packs.enabled_features,
                            }
                        )?
                        .is_empty()
                );
                assert!(
                    staged_resources
                        .selected_data_pack_ids()
                        .unwrap()
                        .contains(&"file/compact")
                );
                assert_eq!(
                    staged_resources
                        .read_data("data/example/worldgen/biome/flat.json")?
                        .unwrap(),
                    expected_biome
                );
                drop(staged_resources);
                drop(reader);
                let StagedInitialWorld { initial, stage } = staged;
                let published = stage.publish(temp.path(), "Created", "New World")?;
                assert_eq!(
                    std::fs::read(
                        published
                            .path
                            .join("datapacks/compact/data/example/worldgen/biome/flat.json")
                    )?,
                    expected_biome
                );
                let saved = native_persistence::world::WorldReader::open_with_session(
                    &published.path,
                    native_persistence::world::DimensionId::parse("overworld")?,
                    published.session().clone(),
                )?;
                assert_eq!(saved.singleplayer_uuid_words()?, Some(initial.uuid));
                assert!(
                    native_persistence::save::WorldWriteGuard::acquire(&published.path).is_err()
                );
            }
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
            player_spawn_search_resumptions: 0,
        });
        let ready = complete_spawn_search(
            ready,
            &mut provider,
            &mut classify,
            &mut resolve,
            tags,
            None,
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
