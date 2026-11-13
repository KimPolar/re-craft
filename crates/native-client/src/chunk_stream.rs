//! Read saved chunks on a worker; publish completed chunks at session tick boundaries.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use native_persistence::{
    heightmap::{HeightBounds, SpawnHeightmaps},
    save::WorldReadStamp,
    world::{ChunkArea, ChunkPos as SavedChunkPos, WorldReader},
};
use native_resources::heightmap_tags::HeightmapTags;
use native_session::{
    GameSession,
    loaded::{LoadedSession, SavedEntityData, SavedPoiData},
};
use native_world::loaded::{ChunkPos, World};

type Defaults = BTreeMap<String, BTreeMap<String, String>>;
type Window = (ChunkPos, u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ActiveWindows {
    primary: Window,
    respawn: Option<Window>,
}

impl ActiveWindows {
    fn contains(self, pos: ChunkPos) -> bool {
        within(pos, self.primary) || self.respawn.is_some_and(|window| within(pos, window))
    }
}

struct Response {
    pos: ChunkPos,
    result: Result<Option<ChunkPayload>, String>,
    stale: bool,
    entities_only: bool,
    stamp: Option<WorldReadStamp>,
}

#[derive(Clone)]
struct Request {
    pos: ChunkPos,
    entities_only: bool,
    shape_required: bool,
    stamp: Option<WorldReadStamp>,
}

struct ChunkPayload {
    world: Option<World>,
    entities: SavedEntityData,
    poi: SavedPoiData,
    heightmaps: Option<SpawnHeightmaps>,
}

pub struct ChunkStreamer {
    requests: Option<SyncSender<Request>>,
    responses: Receiver<Response>,
    window: Arc<Mutex<Option<ActiveWindows>>>,
    stop: Arc<AtomicBool>,
    center: Option<ChunkPos>,
    respawn: Option<Window>,
    radius: u32,
    targets: Vec<ChunkPos>,
    settled_prefix: usize,
    #[cfg(test)]
    skipped_target_checks: u64,
    pending: BTreeSet<ChunkPos>,
    awaiting_attachment: BTreeSet<ChunkPos>,
    /// Other queued terrain superseded an absent disk read; wait for its
    /// tick-boundary validation before deciding whether another read is needed.
    awaiting_competing_attachment: BTreeSet<ChunkPos>,
    missing: BTreeSet<ChunkPos>,
    failed: BTreeMap<ChunkPos, (Instant, u32)>,
    entity_retry: BTreeMap<ChunkPos, (Instant, u32)>,
    shape_required: BTreeSet<ChunkPos>,
    _world_guard: Option<native_persistence::save::WorldSessionGuard>,
    observed_epoch: Option<u64>,
    generation_owner: Arc<()>,
    dimension: Option<native_persistence::world::DimensionId>,
}

/// Disk reads found no terrain for these chunks at `stamp`. A generator must
/// still capture and validate all terrain, entity, POI and live dependencies;
/// this is a scheduling hint, not permission to write or attach a chunk.
#[derive(Clone)]
pub(crate) struct MissingGenerationTargets {
    targets: Vec<ChunkPos>,
    stamp: WorldReadStamp,
    owner: Arc<()>,
}

/// A captured disk snapshot only. Live-state and generation completeness must
/// still be checked before the result can affect gameplay or be saved.
pub(crate) struct CapturedMissingGeneration {
    schedule: native_session::worldgen::schedule::GenerationSchedule,
    context: native_persistence::world::GenerationContextSnapshot,
    observed: MissingGenerationTargets,
}

impl CapturedMissingGeneration {
    /// Recheck scheduling inputs; this does not validate unsaved neighbor state.
    pub fn is_current(&self, streamer: &ChunkStreamer, session: &LoadedSession) -> bool {
        self.observed.is_current(streamer, session)
    }

    pub fn targets(&self) -> &[ChunkPos] {
        self.observed.targets()
    }
}

pub(crate) struct MissingGenerationCaptureWorker {
    targets: MissingGenerationTargets,
    cancelled: Arc<AtomicBool>,
    result: Receiver<Result<CapturedMissingGeneration>>,
    finished: bool,
}

impl MissingGenerationCaptureWorker {
    pub fn start(
        targets: MissingGenerationTargets,
        streamer: &ChunkStreamer,
        session: &LoadedSession,
        mut reader: WorldReader,
    ) -> Result<Self> {
        use native_session::worldgen::schedule::{GenerationSchedule, GenerationStage};
        anyhow::ensure!(!targets.targets.is_empty(), "no missing generation targets");
        anyhow::ensure!(
            targets.is_current(streamer, session),
            "missing chunk observations are stale"
        );
        let guard = reader
            .session_guard()
            .context("generation capture requires a locked world")?;
        anyhow::ensure!(
            targets.stamp.belongs_to_session(&guard),
            "generation reader belongs to another world"
        );
        anyhow::ensure!(
            streamer.dimension.as_ref() == Some(reader.dimension()),
            "generation reader belongs to another dimension"
        );
        let positions = targets.targets.clone();
        let stamp = targets.stamp.clone();
        let observed = targets.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let (sender, result) = mpsc::channel();
        thread::Builder::new()
            .name("missing-generation-capture".into())
            .spawn(move || {
                let work = (|| -> Result<CapturedMissingGeneration> {
                    anyhow::ensure!(
                        !worker_cancelled.load(Ordering::Acquire),
                        "generation capture cancelled"
                    );
                    anyhow::ensure!(stamp.is_current(), "missing chunk observations are stale");
                    let mut schedule = GenerationSchedule::new();
                    schedule.request_many(
                        positions
                            .iter()
                            .copied()
                            .map(|pos| (pos, GenerationStage::Full)),
                    )?;
                    anyhow::ensure!(
                        !worker_cancelled.load(Ordering::Acquire),
                        "generation capture cancelled"
                    );
                    let context = reader.capture_generation_context(
                        schedule
                            .requirements()
                            .map(|(pos, _)| SavedChunkPos { x: pos.x, z: pos.z }),
                    )?;
                    anyhow::ensure!(
                        !worker_cancelled.load(Ordering::Acquire),
                        "generation capture cancelled"
                    );
                    anyhow::ensure!(stamp.is_current(), "missing chunk observations are stale");
                    for pos in positions {
                        let source = context
                            .inputs()
                            .sources()
                            .chunks()
                            .get(&SavedChunkPos { x: pos.x, z: pos.z })
                            .context("generation target was not captured")?;
                        anyhow::ensure!(
                            source.terrain.is_none()
                                && source.entities.is_none()
                                && source.poi.is_none(),
                            "missing generation target {}, {} has existing saved data",
                            pos.x,
                            pos.z
                        );
                    }
                    Ok(CapturedMissingGeneration {
                        schedule,
                        context,
                        observed,
                    })
                })();
                let _ = sender.send(work);
            })
            .context("cannot start missing generation capture")?;
        Ok(Self {
            targets,
            cancelled,
            result,
            finished: false,
        })
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn try_result(
        &mut self,
        streamer: &ChunkStreamer,
        session: &LoadedSession,
    ) -> Result<Option<CapturedMissingGeneration>> {
        if self.finished {
            return Ok(None);
        }
        let result = match self.result.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return Ok(None),
            Err(TryRecvError::Disconnected) => {
                self.finished = true;
                anyhow::bail!("missing generation capture stopped unexpectedly");
            }
        };
        self.finished = true;
        if self.cancelled.load(Ordering::Acquire) || !self.targets.is_current(streamer, session) {
            if let Ok(captured) = result {
                crate::generation_staging::discard_async(captured);
            }
            anyhow::bail!("missing generation capture was cancelled or became stale");
        }
        Ok(Some(result?))
    }
}

impl Drop for MissingGenerationCaptureWorker {
    fn drop(&mut self) {
        self.cancel();
        crate::generation_staging::discard_receiver(&mut self.result);
    }
}

/// Generator and restored saved terrain are still read-only. Live inputs,
/// lighting, generation completeness and save dependencies remain unchecked.
pub(crate) struct PreparedMissingGeneration {
    schedule: native_session::worldgen::schedule::GenerationSchedule,
    generator: crate::fresh_generation::existing::PreparedExistingGenerator,
    terrain: crate::fresh_generation::existing::RestoredSavedTerrain,
    observed: MissingGenerationTargets,
}

impl PreparedMissingGeneration {
    pub fn is_current(&self, streamer: &ChunkStreamer, session: &LoadedSession) -> bool {
        self.observed.is_current(streamer, session)
    }

    pub fn targets(&self) -> &[ChunkPos] {
        self.observed.targets()
    }

    /// The returned observation must be rechecked alongside the independent
    /// live-state and disk guards before generated data is applied or saved.
    pub fn into_checked_parts(
        self,
        streamer: &ChunkStreamer,
        session: &LoadedSession,
    ) -> Result<(
        native_session::worldgen::schedule::GenerationSchedule,
        crate::fresh_generation::existing::PreparedExistingGenerator,
        crate::fresh_generation::existing::RestoredSavedTerrain,
        MissingGenerationTargets,
    )> {
        if !self.is_current(streamer, session) {
            crate::generation_staging::discard_async(self);
            anyhow::bail!("prepared missing generation is stale");
        }
        Ok((self.schedule, self.generator, self.terrain, self.observed))
    }
}

pub(crate) struct MissingGenerationPrepareStartError {
    pub captured: CapturedMissingGeneration,
    pub error: anyhow::Error,
}

pub(crate) struct MissingGenerationPrepareWorker {
    observed: MissingGenerationTargets,
    cancelled: Arc<AtomicBool>,
    result: Receiver<Result<PreparedMissingGeneration>>,
    finished: bool,
}

impl MissingGenerationPrepareWorker {
    /// Keep the input available for retry if a worker cannot be started.
    pub fn start(
        captured: CapturedMissingGeneration,
        streamer: &ChunkStreamer,
        session: &LoadedSession,
        minecraft_dir: std::path::PathBuf,
    ) -> std::result::Result<Self, MissingGenerationPrepareStartError> {
        if !captured.is_current(streamer, session) {
            return Err(MissingGenerationPrepareStartError {
                captured,
                error: anyhow::anyhow!("missing generation capture is stale"),
            });
        }
        let observed = captured.observed.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let (sender, result) = mpsc::channel();
        crate::generation_staging::spawn_with_payload(
            captured,
            move |captured| {
                let work = (|| -> Result<PreparedMissingGeneration> {
                    anyhow::ensure!(
                        !worker_cancelled.load(Ordering::Acquire),
                        "generation preparation cancelled"
                    );
                    anyhow::ensure!(
                        captured.observed.stamp.is_current(),
                        "missing generation capture is stale"
                    );
                    let CapturedMissingGeneration {
                        schedule,
                        context,
                        observed,
                    } = captured;
                    let mut generator =
                        crate::fresh_generation::existing::prepare_existing_generator(
                            context,
                            &minecraft_dir,
                        )?;
                    anyhow::ensure!(
                        !worker_cancelled.load(Ordering::Acquire),
                        "generation preparation cancelled"
                    );
                    let terrain = generator.restore_captured_terrain()?;
                    anyhow::ensure!(
                        !worker_cancelled.load(Ordering::Acquire),
                        "generation preparation cancelled"
                    );
                    anyhow::ensure!(
                        observed.stamp.is_current(),
                        "missing generation capture is stale"
                    );
                    Ok(PreparedMissingGeneration {
                        schedule,
                        generator,
                        terrain,
                        observed,
                    })
                })();
                let _ = sender.send(work);
            },
            |task| {
                thread::Builder::new()
                    .name("missing-generation-prepare".into())
                    .spawn(task)
                    .map(|_| ())
            },
        )
        .map_err(|(captured, error)| MissingGenerationPrepareStartError {
            captured,
            error: anyhow::Error::new(error).context("cannot start missing generation preparation"),
        })?;
        Ok(Self {
            observed,
            cancelled,
            result,
            finished: false,
        })
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn try_result(
        &mut self,
        streamer: &ChunkStreamer,
        session: &LoadedSession,
    ) -> Result<Option<PreparedMissingGeneration>> {
        if self.finished {
            return Ok(None);
        }
        let result = match self.result.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return Ok(None),
            Err(TryRecvError::Disconnected) => {
                self.finished = true;
                anyhow::bail!("missing generation preparation stopped unexpectedly");
            }
        };
        self.finished = true;
        if self.cancelled.load(Ordering::Acquire) || !self.observed.is_current(streamer, session) {
            if let Ok(prepared) = result {
                crate::generation_staging::discard_async(prepared);
            }
            anyhow::bail!("missing generation preparation was cancelled or became stale");
        }
        Ok(Some(result?))
    }
}

impl Drop for MissingGenerationPrepareWorker {
    fn drop(&mut self) {
        self.cancel();
        crate::generation_staging::discard_receiver(&mut self.result);
    }
}

/// Biomes and their generator remain isolated until live and disk inputs are
/// independently revalidated and the remaining generation stages complete.
pub(crate) struct BiomedMissingGeneration {
    work: crate::fresh_generation::existing::CapturedBiomeSchedule,
    generator: crate::fresh_generation::existing::PreparedExistingGenerator,
    terrain: crate::fresh_generation::existing::RestoredSavedTerrain,
    evaluator: native_resources::density::DensityEvaluator,
    observed: MissingGenerationTargets,
}

impl BiomedMissingGeneration {
    pub fn is_current(&self, streamer: &ChunkStreamer, session: &LoadedSession) -> bool {
        self.observed.is_current(streamer, session)
    }

    pub fn targets(&self) -> &[ChunkPos] {
        self.observed.targets()
    }

    pub fn into_checked_parts(
        self,
        streamer: &ChunkStreamer,
        session: &LoadedSession,
    ) -> Result<(
        crate::fresh_generation::existing::CapturedBiomeSchedule,
        crate::fresh_generation::existing::PreparedExistingGenerator,
        crate::fresh_generation::existing::RestoredSavedTerrain,
        native_resources::density::DensityEvaluator,
        MissingGenerationTargets,
    )> {
        if !self.is_current(streamer, session) {
            crate::generation_staging::discard_async(self);
            anyhow::bail!("biome generation is stale");
        }
        Ok((
            self.work,
            self.generator,
            self.terrain,
            self.evaluator,
            self.observed,
        ))
    }
}

pub(crate) struct MissingGenerationBiomeStartError {
    pub prepared: PreparedMissingGeneration,
    pub error: anyhow::Error,
}

pub(crate) struct MissingGenerationBiomeWorker {
    observed: MissingGenerationTargets,
    cancelled: Arc<AtomicBool>,
    result: Receiver<Result<BiomedMissingGeneration>>,
    finished: bool,
}

impl MissingGenerationBiomeWorker {
    /// Retain the prepared capture if the worker cannot be started.
    pub fn start(
        prepared: PreparedMissingGeneration,
        streamer: &ChunkStreamer,
        session: &LoadedSession,
    ) -> std::result::Result<Self, MissingGenerationBiomeStartError> {
        if !prepared.is_current(streamer, session) {
            return Err(MissingGenerationBiomeStartError {
                prepared,
                error: anyhow::anyhow!("prepared missing generation is stale"),
            });
        }
        let observed = prepared.observed.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let (sender, result) = mpsc::channel();
        crate::generation_staging::spawn_with_payload(
            prepared,
            move |prepared| {
                let result = (|| -> Result<BiomedMissingGeneration> {
                    use native_session::worldgen::schedule::GenerationStage;
                    anyhow::ensure!(
                        !worker_cancelled.load(Ordering::Acquire),
                        "biome generation cancelled"
                    );
                    anyhow::ensure!(
                        prepared.observed.stamp.is_current(),
                        "prepared missing generation is stale"
                    );
                    let PreparedMissingGeneration {
                        schedule,
                        mut generator,
                        terrain,
                        observed,
                    } = prepared;
                    let mut work =
                        generator.begin_captured_generation_schedule_checked(schedule, || {
                            anyhow::ensure!(
                                !worker_cancelled.load(Ordering::Acquire),
                                "biome generation cancelled"
                            );
                            anyhow::ensure!(
                                observed.stamp.is_current(),
                                "prepared missing generation is stale"
                            );
                            Ok(())
                        })?;
                    let positions: Vec<_> = work
                        .schedule()
                        .requirements()
                        .filter_map(|(pos, required)| {
                            (required >= GenerationStage::Biomes
                                && work
                                    .schedule()
                                    .completed(pos)
                                    .is_some_and(|stage| stage < GenerationStage::Biomes))
                            .then_some(pos)
                        })
                        .collect();
                    let mut evaluator = native_resources::density::DensityEvaluator::default();
                    for position in positions {
                        anyhow::ensure!(
                            !worker_cancelled.load(Ordering::Acquire),
                            "biome generation cancelled"
                        );
                        anyhow::ensure!(
                            observed.stamp.is_current(),
                            "prepared missing generation is stale"
                        );
                        generator.run_captured_biomes(&mut work, &mut evaluator, position)?;
                    }
                    anyhow::ensure!(
                        !worker_cancelled.load(Ordering::Acquire),
                        "biome generation cancelled"
                    );
                    anyhow::ensure!(
                        observed.stamp.is_current(),
                        "prepared missing generation is stale"
                    );
                    Ok(BiomedMissingGeneration {
                        work,
                        generator,
                        terrain,
                        evaluator,
                        observed,
                    })
                })();
                let _ = sender.send(result);
            },
            |task| {
                thread::Builder::new()
                    .name("missing-generation-biomes".into())
                    .spawn(task)
                    .map(|_| ())
            },
        )
        .map_err(|(prepared, error)| MissingGenerationBiomeStartError {
            prepared,
            error: anyhow::Error::new(error).context("cannot start biome generation"),
        })?;
        Ok(Self {
            observed,
            cancelled,
            result,
            finished: false,
        })
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn try_result(
        &mut self,
        streamer: &ChunkStreamer,
        session: &LoadedSession,
    ) -> Result<Option<BiomedMissingGeneration>> {
        if self.finished {
            return Ok(None);
        }
        let result = match self.result.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return Ok(None),
            Err(TryRecvError::Disconnected) => {
                self.finished = true;
                anyhow::bail!("biome generation stopped unexpectedly");
            }
        };
        self.finished = true;
        if self.cancelled.load(Ordering::Acquire) || !self.observed.is_current(streamer, session) {
            if let Ok(work) = result {
                crate::generation_staging::discard_async(work);
            }
            anyhow::bail!("biome generation was cancelled or became stale");
        }
        Ok(Some(result?))
    }
}

impl Drop for MissingGenerationBiomeWorker {
    fn drop(&mut self) {
        self.cancel();
        crate::generation_staging::discard_receiver(&mut self.result);
    }
}

/// A bounded terrain pass. Saved terrain remains separate and needs its own
/// restoration and live-state validation before later stages can use it.
pub(crate) struct TerrainBatchMissingGeneration {
    state: BiomedMissingGeneration,
    generated: Vec<ChunkPos>,
    saved: Vec<(
        ChunkPos,
        native_session::worldgen::schedule::GenerationStage,
    )>,
    remaining: usize,
}

impl TerrainBatchMissingGeneration {
    pub fn is_current(&self, streamer: &ChunkStreamer, session: &LoadedSession) -> bool {
        self.state.is_current(streamer, session)
    }
    pub fn generated(&self) -> &[ChunkPos] {
        &self.generated
    }
    pub fn saved(
        &self,
    ) -> &[(
        ChunkPos,
        native_session::worldgen::schedule::GenerationStage,
    )] {
        &self.saved
    }
    pub fn remaining(&self) -> usize {
        self.remaining
    }

    /// Continue with the same generator and evaluator after rechecking the
    /// observed targets. This does not establish live or save safety.
    pub fn into_checked_state(
        self,
        streamer: &ChunkStreamer,
        session: &LoadedSession,
    ) -> Result<BiomedMissingGeneration> {
        if !self.state.is_current(streamer, session) {
            crate::generation_staging::discard_async(self);
            anyhow::bail!("terrain generation is stale");
        }
        Ok(self.state)
    }
}

pub(crate) struct MissingGenerationTerrainStartError {
    pub biomed: BiomedMissingGeneration,
    pub error: anyhow::Error,
}

pub(crate) struct MissingGenerationTerrainWorker {
    observed: MissingGenerationTargets,
    cancelled: Arc<AtomicBool>,
    result: Receiver<Result<TerrainBatchMissingGeneration>>,
    finished: bool,
}

impl MissingGenerationTerrainWorker {
    /// For multiple structure keys the caller must supply the corresponding
    /// Java iteration order. Single-key and empty sets have a unique order.
    pub fn start(
        biomed: BiomedMissingGeneration,
        streamer: &ChunkStreamer,
        session: &LoadedSession,
        limit: usize,
        structure_orders: BTreeMap<ChunkPos, Vec<native_resources::packs::ResourceId>>,
    ) -> std::result::Result<Self, MissingGenerationTerrainStartError> {
        if !biomed.is_current(streamer, session) || limit == 0 {
            let error = if limit == 0 {
                "terrain generation limit must be positive"
            } else {
                "biome generation is stale"
            };
            return Err(MissingGenerationTerrainStartError {
                biomed,
                error: anyhow::anyhow!(error),
            });
        }
        let observed = biomed.observed.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let (sender, result) = mpsc::channel();
        crate::generation_staging::spawn_with_payload(
            biomed,
            move |mut state| {
                let result = (|| -> Result<TerrainBatchMissingGeneration> {
                    use crate::fresh_generation::existing::CapturedTerrainInput;
                    use native_session::worldgen::schedule::GenerationStage;
                    anyhow::ensure!(
                        !worker_cancelled.load(Ordering::Acquire),
                        "terrain generation cancelled"
                    );
                    anyhow::ensure!(
                        state.observed.stamp.is_current(),
                        "biome generation is stale"
                    );
                    let mut seen = BTreeSet::new();
                    let positions: Vec<_> = state
                        .observed
                        .targets()
                        .iter()
                        .copied()
                        .chain(state.work.schedule().requirements().filter_map(
                            |(pos, required)| (required >= GenerationStage::Terrain).then_some(pos),
                        ))
                        .filter(|pos| seen.insert(*pos))
                        .collect();
                    let mut generated = Vec::new();
                    let mut saved = Vec::new();
                    let mut remaining = 0;
                    for position in positions {
                        anyhow::ensure!(
                            !worker_cancelled.load(Ordering::Acquire),
                            "terrain generation cancelled"
                        );
                        anyhow::ensure!(
                            state.observed.stamp.is_current(),
                            "biome generation is stale"
                        );
                        if state
                            .work
                            .schedule()
                            .completed(position)
                            .is_some_and(|stage| stage >= GenerationStage::Terrain)
                        {
                            continue;
                        }
                        let input = state
                            .generator
                            .captured_terrain_input(&state.work, position)?;
                        let (resume, keys) = match input {
                            CapturedTerrainInput::Saved { stage } => {
                                anyhow::ensure!(
                                    state.terrain.world().chunks.contains_key(&position),
                                    "saved terrain was not restored"
                                );
                                saved.push((position, stage));
                                continue;
                            }
                            CapturedTerrainInput::New { structure_keys } => (false, structure_keys),
                            CapturedTerrainInput::Resume { structure_keys } => {
                                (true, structure_keys)
                            }
                        };
                        if generated.len() == limit {
                            remaining += 1;
                            continue;
                        }
                        let order = if let Some(order) = structure_orders.get(&position) {
                            let actual: BTreeSet<_> = order.iter().cloned().collect();
                            let expected: BTreeSet<_> = keys.iter().cloned().collect();
                            anyhow::ensure!(
                                order.len() == actual.len() && actual == expected,
                                "structure order does not cover references exactly"
                            );
                            order.as_slice()
                        } else {
                            anyhow::ensure!(
                                keys.len() <= 1,
                                "Java structure order is required for terrain {},{}",
                                position.x,
                                position.z
                            );
                            keys.as_slice()
                        };
                        if resume {
                            state.generator.run_captured_saved_terrain(
                                &mut state.work,
                                &mut state.evaluator,
                                position,
                                order,
                            )?;
                        } else {
                            state
                                .generator
                                .run_captured_new_terrain_with_structure_order(
                                    &mut state.work,
                                    &mut state.evaluator,
                                    position,
                                    order,
                                )?;
                        }
                        generated.push(position);
                    }
                    anyhow::ensure!(
                        !worker_cancelled.load(Ordering::Acquire),
                        "terrain generation cancelled"
                    );
                    anyhow::ensure!(
                        state.observed.stamp.is_current(),
                        "biome generation is stale"
                    );
                    Ok(TerrainBatchMissingGeneration {
                        state,
                        generated,
                        saved,
                        remaining,
                    })
                })();
                let _ = sender.send(result);
            },
            |task| {
                thread::Builder::new()
                    .name("missing-generation-terrain".into())
                    .spawn(task)
                    .map(|_| ())
            },
        )
        .map_err(|(biomed, error)| MissingGenerationTerrainStartError {
            biomed,
            error: anyhow::Error::new(error).context("cannot start terrain generation"),
        })?;
        Ok(Self {
            observed,
            cancelled,
            result,
            finished: false,
        })
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn try_result(
        &mut self,
        streamer: &ChunkStreamer,
        session: &LoadedSession,
    ) -> Result<Option<TerrainBatchMissingGeneration>> {
        if self.finished {
            return Ok(None);
        }
        let result = match self.result.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return Ok(None),
            Err(TryRecvError::Disconnected) => {
                self.finished = true;
                anyhow::bail!("terrain generation stopped unexpectedly");
            }
        };
        self.finished = true;
        if self.cancelled.load(Ordering::Acquire) || !self.observed.is_current(streamer, session) {
            if let Ok(work) = result {
                crate::generation_staging::discard_async(work);
            }
            anyhow::bail!("terrain generation was cancelled or became stale");
        }
        Ok(Some(result?))
    }
}

impl Drop for MissingGenerationTerrainWorker {
    fn drop(&mut self) {
        self.cancel();
        crate::generation_staging::discard_receiver(&mut self.result);
    }
}

impl MissingGenerationTargets {
    pub fn targets(&self) -> &[ChunkPos] {
        &self.targets
    }

    pub fn is_current(&self, streamer: &ChunkStreamer, session: &LoadedSession) -> bool {
        if !Arc::ptr_eq(&self.owner, &streamer.generation_owner) || !self.stamp.is_current() {
            return false;
        }
        streamer
            .missing_generation_targets(session, self.targets.len())
            .is_some_and(|current| {
                current.stamp.epoch() == self.stamp.epoch() && current.targets == self.targets
            })
    }
}

fn select_generation_targets(
    shape_required: &BTreeSet<ChunkPos>,
    targets: &[ChunkPos],
    limit: usize,
    mut eligible: impl FnMut(ChunkPos) -> bool,
) -> Vec<ChunkPos> {
    let mut seen = BTreeSet::new();
    shape_required
        .iter()
        .chain(targets.iter())
        .copied()
        .filter(|pos| eligible(*pos) && seen.insert(*pos))
        .take(limit)
        .collect()
}

impl ChunkStreamer {
    /// Prioritize absent chunks needed by deferred shape updates, followed by
    /// the active streaming order. Return no snapshot if the world or session
    /// moved since `service` last refreshed the cache.
    pub(crate) fn missing_generation_targets(
        &self,
        session: &LoadedSession,
        limit: usize,
    ) -> Option<MissingGenerationTargets> {
        let guard = self._world_guard.as_ref()?;
        let stamp = guard.read_stamp().ok()?;
        let center = self.center?;
        if self.observed_epoch != Some(stamp.epoch())
            || Some(center) != session.player_chunk()
            || self.respawn != session.requested_respawn_window()
            || self.shape_required != session.required_shape_update_chunks()
        {
            return None;
        }
        let pending_data = session.pending_stream_data_positions();
        let targets =
            select_generation_targets(&self.shape_required, &self.targets, limit, |pos| {
                self.missing.contains(&pos)
                    && !self.pending.contains(&pos)
                    && !self.awaiting_attachment.contains(&pos)
                    && !self.awaiting_competing_attachment.contains(&pos)
                    && !pending_data.contains(&pos)
                    && !session.has_chunk_or_pending(pos)
            });
        if !stamp.is_current() {
            return None;
        }
        Some(MissingGenerationTargets {
            targets,
            stamp,
            owner: Arc::clone(&self.generation_owner),
        })
    }

    pub fn radius(&self) -> u32 {
        self.radius
    }

    pub fn set_radius(&mut self, radius: u32) {
        let radius = radius.max(1);
        if self.radius != radius {
            self.radius = radius;
            // Force the next service call to rebuild the worker window and
            // update the session even when the player has not moved.
            self.center = None;
        }
    }

    pub fn start(
        mut reader: WorldReader,
        defaults: Defaults,
        radius: u32,
        missing: BTreeSet<ChunkPos>,
        heightmap_context: Option<(HeightBounds, Arc<HeightmapTags>)>,
    ) -> Result<Self> {
        let guard = reader.session_guard();
        let dimension = reader.dimension().clone();
        let epoch = guard
            .as_ref()
            .map(|guard| guard.read_stamp().map(|stamp| stamp.epoch()))
            .transpose()?;
        // The caller's missing set predates this stamp and may have raced a writer.
        let missing = if guard.is_some() {
            BTreeSet::new()
        } else {
            missing
        };
        let mut streamer = Self::with_entity_source(radius, missing, move |pos, entities_only| {
            if entities_only {
                let sidecars = reader.read_chunk_sidecars(pos.x, pos.z)?;
                return Ok(Some(ChunkPayload {
                    world: None,
                    entities: entity_data_from_read(sidecars.entities, pos),
                    poi: poi_data_from_read(sidecars.poi, pos),
                    heightmaps: None,
                }));
            }
            let Some(bundle) = reader.read_chunk_bundle(pos.x, pos.z)? else {
                return Ok(None);
            };
            let saved = SavedChunkPos { x: pos.x, z: pos.z };
            let chunk = Arc::new(bundle.terrain);
            let area = ChunkArea {
                center: saved,
                radius: 0,
                chunks: BTreeMap::from([(saved, Arc::clone(&chunk))]),
                missing: Vec::new(),
            };
            let world = area.materialize_with_defaults(reader.dimension(), &defaults)?;
            let entities = entity_data_from_read(bundle.sidecars.entities, pos);
            let poi = poi_data_from_read(bundle.sidecars.poi, pos);
            let heightmaps = heightmap_context.as_ref().and_then(|(bounds, tags)| {
                match native_session::spawn_heightmaps::prepare(&chunk, &world, *bounds, tags) {
                    Ok(maps) => Some(maps),
                    Err(error) => {
                        eprintln!(
                            "Could not prepare spawn heightmaps for chunk {}, {}: {error:#}",
                            pos.x, pos.z
                        );
                        None
                    }
                }
            });
            Ok(Some(ChunkPayload {
                world: Some(world),
                entities,
                poi,
                heightmaps,
            }))
        })?;
        streamer._world_guard = guard;
        streamer.observed_epoch = epoch;
        streamer.dimension = Some(dimension);
        Ok(streamer)
    }

    #[cfg(test)]
    fn with_source(
        radius: u32,
        missing: BTreeSet<ChunkPos>,
        mut read: impl FnMut(ChunkPos) -> Result<Option<World>> + Send + 'static,
    ) -> Result<Self> {
        Self::with_entity_source(radius, missing, move |pos, _| {
            Ok(read(pos)?.map(|world| ChunkPayload {
                world: Some(world),
                entities: SavedEntityData::Unavailable,
                poi: SavedPoiData::Unavailable,
                heightmaps: None,
            }))
        })
    }

    fn with_entity_source(
        radius: u32,
        missing: BTreeSet<ChunkPos>,
        mut read: impl FnMut(ChunkPos, bool) -> Result<Option<ChunkPayload>> + Send + 'static,
    ) -> Result<Self> {
        let (requests, request_rx) = mpsc::sync_channel::<Request>(8);
        let (response_tx, responses) = mpsc::channel();
        let window = Arc::new(Mutex::new(None::<ActiveWindows>));
        let worker_window = Arc::clone(&window);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        thread::Builder::new()
            .name("minecraft-chunk-reader".into())
            .spawn(move || {
                while let Ok(request) = request_rx.recv() {
                    let pos = request.pos;
                    if worker_stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let current = *worker_window
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let stale = !request.shape_required
                        && current.is_some_and(|windows| !windows.contains(pos));
                    let result = if stale {
                        Ok(None)
                    } else {
                        read(pos, request.entities_only).map_err(|error| format!("{error:#}"))
                    };
                    if response_tx
                        .send(Response {
                            pos,
                            result,
                            stale,
                            entities_only: request.entities_only,
                            stamp: request.stamp,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .context("cannot start the chunk reader thread")?;
        Ok(Self {
            requests: Some(requests),
            responses,
            window,
            stop,
            center: None,
            respawn: None,
            radius: radius.max(1),
            targets: Vec::new(),
            settled_prefix: 0,
            #[cfg(test)]
            skipped_target_checks: 0,
            pending: BTreeSet::new(),
            awaiting_attachment: BTreeSet::new(),
            awaiting_competing_attachment: BTreeSet::new(),
            missing,
            failed: BTreeMap::new(),
            entity_retry: BTreeMap::new(),
            shape_required: BTreeSet::new(),
            _world_guard: None,
            observed_epoch: None,
            generation_owner: Arc::new(()),
            dimension: None,
        })
    }

    pub fn service(&mut self, session: &mut LoadedSession) -> Result<()> {
        let active_stamp = match &self._world_guard {
            Some(guard) => match guard.read_stamp() {
                Ok(stamp) => Some(stamp),
                Err(_) => return Ok(()),
            },
            None => None,
        };
        let epoch = active_stamp.as_ref().map(WorldReadStamp::epoch);
        if self.observed_epoch != epoch {
            self.missing.clear();
            self.failed.clear();
            self.entity_retry.clear();
            self.settled_prefix = 0;
            self.center = None;
            self.observed_epoch = epoch;
        }
        let stale = session.take_stale_streamed_chunks();
        let stale_received = !stale.is_empty();
        for pos in stale {
            self.missing.remove(&pos);
            self.failed.remove(&pos);
            self.awaiting_attachment.remove(&pos);
            self.awaiting_competing_attachment.remove(&pos);
            if session.has_resident_or_parked_chunk(pos) {
                self.entity_retry.insert(pos, (Instant::now(), 0));
            }
        }
        let center = session
            .player_chunk()
            .context("player is outside the supported block coordinate range")?;
        let moved = self.center != Some(center);
        let respawn = session.requested_respawn_window();
        let windows_changed = moved || self.respawn != respawn;
        let windows = ActiveWindows {
            primary: (center, self.radius),
            respawn,
        };
        let shape_required = session.required_shape_update_chunks();
        if self.shape_required != shape_required {
            self.shape_required = shape_required;
            self.awaiting_competing_attachment
                .retain(|pos| windows.contains(*pos) || self.shape_required.contains(pos));
            self.missing
                .retain(|pos| windows.contains(*pos) || self.shape_required.contains(pos));
            self.failed
                .retain(|pos, _| windows.contains(*pos) || self.shape_required.contains(pos));
        }
        if windows_changed {
            self.settled_prefix = 0;
            self.center = Some(center);
            self.respawn = respawn;
            *self
                .window
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(windows);
            let mut seen = BTreeSet::new();
            self.targets = respawn
                .into_iter()
                .flat_map(|(center, radius)| targets(center, radius))
                .chain(targets(center, self.radius))
                .filter(|pos| seen.insert(*pos))
                .collect();
            // Missing entries are only useful in an active window. Keeping
            // every absent chunk ever visited grows without bound and can
            // suppress a fresh disk read when a distant area is revisited.
            self.missing
                .retain(|pos| windows.contains(*pos) || self.shape_required.contains(pos));
            self.awaiting_competing_attachment
                .retain(|pos| windows.contains(*pos) || self.shape_required.contains(pos));
            self.entity_retry.retain(|pos, _| windows.contains(*pos));
            self.failed
                .retain(|pos, _| windows.contains(*pos) || self.shape_required.contains(pos));
            for &pos in &self.targets {
                if matches!(
                    session.saved_entity_chunk(pos),
                    Some(SavedEntityData::Unavailable)
                ) || (session.can_retry_poi_chunk(pos)
                    && matches!(
                        session.saved_poi_chunk(pos),
                        Some(SavedPoiData::Unavailable)
                    ))
                {
                    self.entity_retry
                        .entry(pos)
                        .or_insert((Instant::now() + Duration::from_secs(5), 0));
                }
            }
            if moved {
                session.queue_chunk_window(center, self.radius);
            }
            session.queue_respawn_window(respawn);
            let snapshot = session.snapshot();
            let world = snapshot
                .world
                .loaded()
                .context("streamed session has no loaded world")?;
            for pos in self
                .targets
                .iter()
                .filter(|pos| within(**pos, windows.primary) && self.missing.contains(pos))
            {
                if !world.missing_chunks.contains(pos) {
                    session.queue_streamed_chunk(
                        *pos,
                        None,
                        None,
                        None,
                        None,
                        active_stamp.clone(),
                    );
                }
            }
        }
        let mut received = false;
        loop {
            match self.responses.try_recv() {
                Ok(response) => {
                    received = true;
                    self.pending.remove(&response.pos);
                    // The player may move while an I/O operation is running.
                    // Do not let an obsolete missing/error result poison a
                    // future visit to that chunk.
                    if response.stale
                        || (!windows.contains(response.pos)
                            && !self.shape_required.contains(&response.pos))
                        || response
                            .stamp
                            .as_ref()
                            .is_some_and(|stamp| !stamp.is_current())
                    {
                        continue;
                    }
                    match response.result {
                        Ok(Some(ChunkPayload {
                            world,
                            entities,
                            poi,
                            heightmaps,
                        })) => {
                            session.queue_streamed_chunk(
                                response.pos,
                                world,
                                Some(entities),
                                Some(poi),
                                heightmaps,
                                response.stamp,
                            );
                            self.awaiting_attachment.insert(response.pos);
                        }
                        Ok(None) if response.entities_only => {
                            schedule_retry(&mut self.entity_retry, response.pos);
                        }
                        Ok(None) => {
                            self.failed.remove(&response.pos);
                            if session.has_chunk_or_pending(response.pos) {
                                self.missing.remove(&response.pos);
                                if !session.has_resident_or_parked_chunk(response.pos) {
                                    self.awaiting_competing_attachment.insert(response.pos);
                                }
                                continue;
                            }
                            self.missing.insert(response.pos);
                            if within(response.pos, (center, self.radius)) {
                                session.queue_streamed_chunk(
                                    response.pos,
                                    None,
                                    None,
                                    None,
                                    None,
                                    response.stamp,
                                );
                            }
                        }
                        Err(error) => {
                            if response.entities_only {
                                schedule_retry(&mut self.entity_retry, response.pos);
                            } else if !session.has_resident_or_parked_chunk(response.pos) {
                                // A queued competing chunk can still fail validation.
                                // Keep the retry until it is actually resident.
                                schedule_retry(&mut self.failed, response.pos);
                            } else {
                                self.failed.remove(&response.pos);
                            }
                            eprintln!(
                                "Could not load chunk {}, {}: {error}",
                                response.pos.x, response.pos.z
                            );
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    anyhow::bail!("chunk reader stopped unexpectedly")
                }
            }
        }
        // The competing source may still be rejected at the tick boundary.
        // It need not come from this disk read, so do not infer its sidecars
        // from the absent terrain response.
        let competing_settled: Vec<_> = if self.awaiting_competing_attachment.is_empty() {
            Vec::new()
        } else {
            let pending = session.pending_stream_data_positions();
            self.awaiting_competing_attachment
                .difference(&pending)
                .copied()
                .collect()
        };
        let competing_settled_any = !competing_settled.is_empty();
        for pos in competing_settled {
            self.awaiting_competing_attachment.remove(&pos);
        }
        // Reads are only applied by the session at a tick boundary. A valid
        // region record can still fail position or data validation there, so
        // do not treat a successful read as a successful attachment.
        let attached: Vec<_> = if self.awaiting_attachment.is_empty() {
            Vec::new()
        } else {
            let pending = session.pending_stream_data_positions();
            self.awaiting_attachment
                .difference(&pending)
                .copied()
                .collect()
        };
        for pos in attached {
            self.awaiting_attachment.remove(&pos);
            if !windows.contains(pos) && !self.shape_required.contains(&pos) {
                continue;
            }
            if !session.has_resident_or_parked_chunk(pos) {
                schedule_retry(&mut self.failed, pos);
                continue;
            }
            self.failed.remove(&pos);
            if matches!(
                session.saved_entity_chunk(pos),
                None | Some(SavedEntityData::Unavailable)
            ) || (session.can_retry_poi_chunk(pos)
                && matches!(
                    session.saved_poi_chunk(pos),
                    None | Some(SavedPoiData::Unavailable)
                ))
            {
                schedule_retry(&mut self.entity_retry, pos);
            } else {
                self.entity_retry.remove(&pos);
            }
        }
        // Another path may have attached the chunk while its failed read was
        // waiting. A stale terrain error must not block entity-only retries.
        self.failed
            .retain(|pos, _| !session.has_resident_or_parked_chunk(*pos));
        let now = Instant::now();
        let entity_retry_due = self.entity_retry.iter().any(|(pos, (at, _))| {
            *at <= now
                && !self.pending.contains(pos)
                && !self.awaiting_attachment.contains(pos)
                && !self.failed.contains_key(pos)
                && !self.missing.contains(pos)
        });
        let terrain_retry_due = self.failed.iter().any(|(pos, (at, _))| {
            *at <= now
                && !self.pending.contains(pos)
                && !self.awaiting_attachment.contains(pos)
                && !self.missing.contains(pos)
                && !session.has_chunk_or_pending(*pos)
        });
        let retry_due = entity_retry_due || terrain_retry_due;
        // Shape updates can depend on saved chunks outside the render window.
        // Recheck every service call, including after a full request queue.
        for &pos in &self.shape_required {
            if self.missing.contains(&pos)
                || self.pending.contains(&pos)
                || self.awaiting_attachment.contains(&pos)
                || session.has_chunk_or_pending(pos)
                || self.failed.get(&pos).is_some_and(|(at, _)| *at > now)
            {
                continue;
            }
            match self
                .requests
                .as_ref()
                .context("chunk reader is closed")?
                .try_send(Request {
                    pos,
                    entities_only: false,
                    shape_required: true,
                    stamp: active_stamp.clone(),
                }) {
                Ok(()) => {
                    self.pending.insert(pos);
                }
                Err(TrySendError::Full(_)) => break,
                Err(TrySendError::Disconnected(_)) => {
                    anyhow::bail!("chunk reader stopped unexpectedly")
                }
            }
        }
        if windows_changed || received || retry_due || stale_received || competing_settled_any {
            if stale_received || competing_settled_any {
                self.settled_prefix = 0;
            }
            while let Some(&pos) = self.targets.get(self.settled_prefix) {
                if !self.missing.contains(&pos)
                    && !self.failed.contains_key(&pos)
                    && !session.has_resident_or_parked_chunk(pos)
                {
                    break;
                }
                self.settled_prefix += 1;
            }
            // Entity retries can become due for already settled chunks.
            // Rescan in the original target order only when a retry is due.
            let start = if retry_due { 0 } else { self.settled_prefix };
            #[cfg(test)]
            {
                self.skipped_target_checks += start as u64;
            }
            for &pos in &self.targets[start..] {
                let retry = self
                    .entity_retry
                    .get(&pos)
                    .is_some_and(|(at, _)| *at <= now);
                let terrain_retry = self.failed.get(&pos).is_some_and(|(at, _)| *at <= now);
                if self.missing.contains(&pos)
                    || (self.failed.contains_key(&pos) && !terrain_retry)
                    || self.pending.contains(&pos)
                    || self.awaiting_attachment.contains(&pos)
                    || (session.has_chunk_or_pending(pos) && !retry)
                {
                    continue;
                }
                match self
                    .requests
                    .as_ref()
                    .context("chunk reader is closed")?
                    .try_send(Request {
                        pos,
                        entities_only: retry && session.has_chunk_or_pending(pos),
                        shape_required: false,
                        stamp: active_stamp.clone(),
                    }) {
                    Ok(()) => {
                        self.pending.insert(pos);
                    }
                    Err(TrySendError::Full(_)) => break,
                    Err(TrySendError::Disconnected(_)) => {
                        anyhow::bail!("chunk reader stopped unexpectedly")
                    }
                }
            }
        }
        for error in session.take_stream_errors() {
            eprintln!("Could not attach streamed {error}");
        }
        Ok(())
    }
}

fn schedule_retry(retries: &mut BTreeMap<ChunkPos, (Instant, u32)>, pos: ChunkPos) {
    let attempts = retries
        .get(&pos)
        .map_or(0, |(_, attempts)| attempts.saturating_add(1));
    let delay = 5_u64.saturating_mul(1_u64 << attempts.min(4));
    retries.insert(pos, (Instant::now() + Duration::from_secs(delay), attempts));
}

pub(crate) fn read_entity_data(reader: &mut WorldReader, pos: ChunkPos) -> SavedEntityData {
    entity_data_from_read(reader.read_entity_chunk(pos.x, pos.z), pos)
}

fn entity_data_from_read(
    result: Result<Option<native_persistence::world::SavedEntityChunk>>,
    pos: ChunkPos,
) -> SavedEntityData {
    match result {
        Ok(None) => SavedEntityData::Empty,
        Ok(Some(chunk)) => SavedEntityData::Present(Arc::new(chunk)),
        Err(error) => {
            eprintln!(
                "Could not read entity chunk {}, {}: {error:#}",
                pos.x, pos.z
            );
            SavedEntityData::Unavailable
        }
    }
}

pub(crate) fn read_poi_data(reader: &mut WorldReader, pos: ChunkPos) -> SavedPoiData {
    poi_data_from_read(reader.read_poi_chunk(pos.x, pos.z), pos)
}

fn poi_data_from_read(
    result: Result<Option<native_persistence::world::SavedPoiChunk>>,
    pos: ChunkPos,
) -> SavedPoiData {
    match result {
        Ok(None) => SavedPoiData::Missing,
        Ok(Some(chunk)) => SavedPoiData::Present(Arc::new(chunk)),
        Err(error) => {
            eprintln!("Could not read POI chunk {}, {}: {error:#}", pos.x, pos.z);
            SavedPoiData::Unavailable
        }
    }
}

impl Drop for ChunkStreamer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.requests.take();
    }
}

fn within(pos: ChunkPos, (center, radius): Window) -> bool {
    (i64::from(pos.x) - i64::from(center.x)).abs() <= i64::from(radius)
        && (i64::from(pos.z) - i64::from(center.z)).abs() <= i64::from(radius)
}

fn targets(center: ChunkPos, radius: u32) -> Vec<ChunkPos> {
    let mut positions = Vec::new();
    let radius = i64::from(radius);
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            let (Ok(x), Ok(z)) = (
                i32::try_from(i64::from(center.x) + dx),
                i32::try_from(i64::from(center.z) + dz),
            ) else {
                continue;
            };
            // A chunk whose blocks cannot be represented by the renderer remains unavailable.
            if x.checked_mul(16).is_none() || z.checked_mul(16).is_none() {
                continue;
            }
            positions.push(ChunkPos { x, z });
        }
    }
    positions.sort_by_key(|pos| {
        let dx = (i64::from(pos.x) - i64::from(center.x)).abs();
        let dz = (i64::from(pos.z) - i64::from(center.z)).abs();
        (dx.max(dz), dx * dx + dz * dz, pos.z, pos.x)
    });
    positions
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{IVec3, Vec3};
    use native_session::{PlayerCommand, PlayerState, loaded::BuildBounds};
    use native_world::loaded::{BLOCK_COUNT, BlockState, Chunk, Section};
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    #[cfg(windows)]
    fn locked_world_fixture() -> (tempfile::TempDir, WorldReader) {
        use flate2::{Compression, write::GzEncoder};
        use native_persistence::{
            nbt::{self, NamedTag, Tag},
            world::DimensionId,
        };
        use std::io::Write;
        let parent =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/stream-lock-tests");
        std::fs::create_dir_all(&parent).unwrap();
        let dir = tempfile::Builder::new()
            .prefix("world-")
            .tempdir_in(parent)
            .unwrap();
        let level = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "Data".into(),
                Tag::Compound(vec![("DataVersion".into(), Tag::Int(5023))]),
            )]),
        };
        let mut output = GzEncoder::new(Vec::new(), Compression::default());
        output.write_all(&nbt::write(&level).unwrap()).unwrap();
        std::fs::write(dir.path().join("level.dat"), output.finish().unwrap()).unwrap();
        let reader =
            WorldReader::open_for_session(dir.path(), DimensionId::parse("overworld").unwrap())
                .unwrap();
        (dir, reader)
    }

    #[cfg(windows)]
    fn wait_for_world_unlock(root: &std::path::Path) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if native_persistence::save::WorldWriteGuard::acquire(root).is_ok() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "chunk reader kept the world locked after stopping"
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn generation_targets_prioritize_shape_dependencies_without_duplicates() {
        let a = ChunkPos { x: -2, z: 0 };
        let b = ChunkPos { x: 3, z: 1 };
        let c = ChunkPos { x: 0, z: 0 };
        let d = ChunkPos { x: 1, z: 0 };
        let shape = BTreeSet::from([a, b]);
        let window = [c, a, d, b];
        assert_eq!(
            select_generation_targets(&shape, &window, 3, |_| true),
            vec![a, b, c]
        );
        assert_eq!(
            select_generation_targets(&shape, &window, 9, |pos| pos != b),
            vec![a, c, d]
        );
        assert!(select_generation_targets(&shape, &window, 0, |_| true).is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn missing_generation_requests_reject_stale_or_competing_inputs() {
        use native_persistence::{
            nbt::{self, NamedTag, Tag},
            world::DimensionId,
        };
        use native_session::worldgen::schedule::GenerationStage;
        let (world, reader) = locked_world_fixture();
        std::fs::create_dir_all(world.path().join("data/minecraft")).unwrap();
        std::fs::create_dir_all(world.path().join("datapacks")).unwrap();
        let settings = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                (
                    "data".into(),
                    Tag::Compound(vec![("seed".into(), Tag::Long(42))]),
                ),
            ]),
        };
        std::fs::write(
            world.path().join("data/minecraft/world_gen_settings.dat"),
            nbt::write(&settings).unwrap(),
        )
        .unwrap();
        let guard = reader.session_guard().unwrap();
        let home = ChunkPos { x: 0, z: 0 };
        let player = PlayerState {
            position: Vec3::new(8.5, 1.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let mut session = LoadedSession::new(
            chunk_world(home),
            IVec3::ZERO,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        let mut streamer =
            ChunkStreamer::start(reader, BTreeMap::new(), 1, BTreeSet::new(), None).unwrap();
        assert!(streamer.missing_generation_targets(&session, 3).is_none());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            streamer.service(&mut session).unwrap();
            session.update(Duration::from_millis(50));
            if streamer.pending.is_empty() && streamer.missing.len() == 8 {
                break;
            }
            assert!(Instant::now() < deadline, "missing chunk reads timed out");
            thread::sleep(Duration::from_millis(2));
        }
        let batch = streamer.missing_generation_targets(&session, 3).unwrap();
        assert_eq!(batch.targets.len(), 3);
        assert!(batch.is_current(&streamer, &session));
        let wrong_dimension = WorldReader::open_with_session(
            world.path(),
            DimensionId::parse("the_nether").unwrap(),
            guard.clone(),
        )
        .unwrap();
        assert!(
            MissingGenerationCaptureWorker::start(
                batch.clone(),
                &streamer,
                &session,
                wrong_dimension
            )
            .is_err()
        );
        let (_other_world, other_reader) = locked_world_fixture();
        assert!(
            MissingGenerationCaptureWorker::start(batch.clone(), &streamer, &session, other_reader)
                .is_err()
        );
        let capture_reader = WorldReader::open_with_session(
            world.path(),
            DimensionId::parse("overworld").unwrap(),
            guard.clone(),
        )
        .unwrap();
        let mut capture = MissingGenerationCaptureWorker::start(
            batch.clone(),
            &streamer,
            &session,
            capture_reader,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let captured = loop {
            if let Some(result) = capture.try_result(&streamer, &session).unwrap() {
                break result;
            }
            assert!(
                Instant::now() < deadline,
                "generation input capture timed out"
            );
            thread::sleep(Duration::from_millis(2));
        };
        assert_eq!(captured.context.inputs().settings().seed(), 42);
        assert!(captured.is_current(&streamer, &session));
        assert_eq!(captured.targets(), batch.targets.as_slice());
        for target in &batch.targets {
            assert_eq!(
                captured.schedule.required(*target),
                Some(GenerationStage::Full)
            );
        }
        assert!(captured.schedule.requested_chunks() > batch.targets.len());
        assert_eq!(
            captured.context.inputs().sources().chunks().len(),
            captured.schedule.requested_chunks()
        );
        for target in &batch.targets {
            let source = &captured.context.inputs().sources().chunks()[&SavedChunkPos {
                x: target.x,
                z: target.z,
            }];
            assert!(source.terrain.is_none() && source.entities.is_none() && source.poi.is_none());
        }
        assert!(capture.try_result(&streamer, &session).unwrap().is_none());
        let capture_reader = WorldReader::open_with_session(
            world.path(),
            DimensionId::parse("overworld").unwrap(),
            guard.clone(),
        )
        .unwrap();
        let mut cancelled_capture = MissingGenerationCaptureWorker::start(
            batch.clone(),
            &streamer,
            &session,
            capture_reader,
        )
        .unwrap();
        cancelled_capture.cancel();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match cancelled_capture.try_result(&streamer, &session) {
                Ok(None) => {}
                Ok(Some(_)) => panic!("cancelled generation capture was accepted"),
                Err(error) => {
                    assert!(error.to_string().contains("cancelled"));
                    break;
                }
            }
            assert!(
                Instant::now() < deadline,
                "cancelled generation capture timed out"
            );
            thread::sleep(Duration::from_millis(2));
        }
        // A terrain miss alone does not prove that an entity sidecar is absent.
        // Simulate an external writer that bypasses the session lock.
        let first = batch.targets[0];
        let entity = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("Position".into(), Tag::IntArray(vec![first.x, first.z])),
                (
                    "Entities".into(),
                    Tag::List {
                        element_type: nbt::TagType::Compound,
                        values: vec![],
                    },
                ),
            ]),
        };
        let mut compressor =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut compressor, &nbt::write(&entity).unwrap()).unwrap();
        let compressed = compressor.finish().unwrap();
        assert!(compressed.len() + 5 <= 4096);
        let mut region = vec![0u8; 3 * 4096];
        let index = (first.x.rem_euclid(32) + first.z.rem_euclid(32) * 32) as usize;
        region[index * 4..index * 4 + 4].copy_from_slice(&((2u32 << 8) | 1).to_be_bytes());
        region[8192..8196].copy_from_slice(&((compressed.len() + 1) as u32).to_be_bytes());
        region[8196] = 2;
        region[8197..8197 + compressed.len()].copy_from_slice(&compressed);
        let entity_dir = world.path().join("dimensions/minecraft/overworld/entities");
        std::fs::create_dir_all(&entity_dir).unwrap();
        std::fs::write(
            entity_dir.join(format!(
                "r.{}.{}.mca",
                first.x.div_euclid(32),
                first.z.div_euclid(32)
            )),
            region,
        )
        .unwrap();
        let capture_reader = WorldReader::open_with_session(
            world.path(),
            DimensionId::parse("overworld").unwrap(),
            guard.clone(),
        )
        .unwrap();
        let mut sidecar_capture = MissingGenerationCaptureWorker::start(
            batch.clone(),
            &streamer,
            &session,
            capture_reader,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match sidecar_capture.try_result(&streamer, &session) {
                Ok(None) => {}
                Ok(Some(_)) => panic!("existing entity sidecar was accepted"),
                Err(error) => {
                    assert!(
                        error.to_string().contains("existing saved data"),
                        "{error:#}"
                    );
                    break;
                }
            }
            assert!(
                Instant::now() < deadline,
                "sidecar generation capture timed out"
            );
            thread::sleep(Duration::from_millis(2));
        }
        let capture_reader = WorldReader::open_with_session(
            world.path(),
            DimensionId::parse("overworld").unwrap(),
            guard.clone(),
        )
        .unwrap();
        let mut stale_capture = MissingGenerationCaptureWorker::start(
            batch.clone(),
            &streamer,
            &session,
            capture_reader,
        )
        .unwrap();
        session.queue_loaded_chunk(first, chunk_world(first));
        assert!(!batch.is_current(&streamer, &session));
        assert!(!captured.is_current(&streamer, &session));
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match stale_capture.try_result(&streamer, &session) {
                Ok(None) => {}
                Ok(Some(_)) => panic!("stale generation capture was accepted"),
                Err(error) => {
                    assert!(error.to_string().contains("stale"));
                    break;
                }
            }
            assert!(
                Instant::now() < deadline,
                "stale generation capture timed out"
            );
            thread::sleep(Duration::from_millis(2));
        }
        let next = streamer.missing_generation_targets(&session, 3).unwrap();
        assert!(!next.targets.contains(&first));
        let foreign = ChunkStreamer::with_source(1, BTreeSet::new(), |_| Ok(None)).unwrap();
        assert!(!next.is_current(&foreign, &session));
        drop(guard.acquire_write().unwrap());
        assert!(!next.stamp.is_current());
        assert!(!next.is_current(&streamer, &session));
        assert!(streamer.missing_generation_targets(&session, 3).is_none());
        drop(batch);
        drop(next);
        drop(capture);
        drop(cancelled_capture);
        drop(sidecar_capture);
        drop(stale_capture);
        drop(captured);
        drop(streamer);
        drop(guard);
        wait_for_world_unlock(world.path());
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn missing_generation_prepares_captured_world_off_thread() -> Result<()> {
        use native_persistence::{
            nbt,
            world::DimensionId,
            worldgen_settings::{NewWorldOptions, draft_baked_worldgen_settings},
        };
        use native_session::worldgen::schedule::GenerationStage;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let (world, reader) = locked_world_fixture();
        let guard = reader.session_guard().context("session guard missing")?;
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let worldgen = draft_baked_worldgen_settings(
            &mut resources,
            "minecraft:normal",
            NewWorldOptions {
                seed: 42,
                generate_structures: false,
                bonus_chest: false,
            },
        )?;
        std::fs::create_dir_all(world.path().join("data/minecraft"))?;
        std::fs::create_dir_all(world.path().join("datapacks"))?;
        std::fs::write(
            world.path().join("data/minecraft/world_gen_settings.dat"),
            nbt::write(&worldgen)?,
        )?;
        let player = PlayerState {
            position: Vec3::new(8.5, 80.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let mut session = LoadedSession::new(
            World::new("minecraft:overworld"),
            IVec3::ZERO,
            BuildBounds {
                min_y: -64,
                max_y: 320,
            },
            player,
        )
        .map_err(anyhow::Error::msg)?;
        let mut streamer = ChunkStreamer::start(reader, BTreeMap::new(), 1, BTreeSet::new(), None)?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            streamer.service(&mut session)?;
            session.update(Duration::from_millis(50));
            if streamer.pending.is_empty() && streamer.missing.len() == 9 {
                break;
            }
            anyhow::ensure!(Instant::now() < deadline, "missing chunk reads timed out");
            thread::sleep(Duration::from_millis(2));
        }
        let targets = streamer
            .missing_generation_targets(&session, 1)
            .context("targets missing")?;
        let target = targets.targets()[0];
        let reader = WorldReader::open_with_session(
            world.path(),
            DimensionId::parse("overworld")?,
            guard.clone(),
        )?;
        let mut capture =
            MissingGenerationCaptureWorker::start(targets, &streamer, &session, reader)?;
        let deadline = Instant::now() + Duration::from_secs(30);
        let captured = loop {
            if let Some(result) = capture.try_result(&streamer, &session)? {
                break result;
            }
            anyhow::ensure!(Instant::now() < deadline, "generation capture timed out");
            thread::sleep(Duration::from_millis(2));
        };
        let mut prepare =
            MissingGenerationPrepareWorker::start(captured, &streamer, &session, game.clone())
                .map_err(|failure| failure.error)?;
        let deadline = Instant::now() + Duration::from_secs(60);
        let prepared = loop {
            if let Some(result) = prepare.try_result(&streamer, &session)? {
                break result;
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "generation preparation timed out"
            );
            thread::sleep(Duration::from_millis(2));
        };
        assert_eq!(prepared.targets(), &[target]);
        assert_eq!(
            prepared.schedule.required(target),
            Some(GenerationStage::Full)
        );
        assert_eq!(prepared.generator.inputs().settings().seed(), 42);
        assert!(prepared.terrain.world().chunks.is_empty());
        assert!(prepared.is_current(&streamer, &session));
        let (schedule, generator, terrain, observed) =
            prepared.into_checked_parts(&streamer, &session)?;
        let prepared = PreparedMissingGeneration {
            schedule,
            generator,
            terrain,
            observed,
        };
        let live = prepared
            .generator
            .capture_live_inputs(&session, &prepared.terrain)?;
        prepared.generator.verify_live_inputs(&session, &live)?;
        let capture_again = || -> Result<CapturedMissingGeneration> {
            let targets = streamer
                .missing_generation_targets(&session, 1)
                .context("targets missing")?;
            let reader = WorldReader::open_with_session(
                world.path(),
                DimensionId::parse("overworld")?,
                guard.clone(),
            )?;
            let mut worker =
                MissingGenerationCaptureWorker::start(targets, &streamer, &session, reader)?;
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                if let Some(result) = worker.try_result(&streamer, &session)? {
                    return Ok(result);
                }
                anyhow::ensure!(Instant::now() < deadline, "generation capture timed out");
                thread::sleep(Duration::from_millis(2));
            }
        };
        let captured = capture_again()?;
        let mut prepare_biomes =
            MissingGenerationPrepareWorker::start(captured, &streamer, &session, game.clone())
                .map_err(|failure| failure.error)?;
        let deadline = Instant::now() + Duration::from_secs(60);
        let prepared_biomes = loop {
            if let Some(result) = prepare_biomes.try_result(&streamer, &session)? {
                break result;
            }
            anyhow::ensure!(Instant::now() < deadline, "biome preparation timed out");
            thread::sleep(Duration::from_millis(2));
        };
        let mut biome_worker =
            MissingGenerationBiomeWorker::start(prepared_biomes, &streamer, &session)
                .map_err(|failure| failure.error)?;
        let deadline = Instant::now() + Duration::from_secs(120);
        let biomed = loop {
            if let Some(result) = biome_worker.try_result(&streamer, &session)? {
                break result;
            }
            anyhow::ensure!(Instant::now() < deadline, "biome generation timed out");
            thread::sleep(Duration::from_millis(2));
        };
        assert_eq!(biomed.targets(), &[target]);
        assert_eq!(
            biomed.work.schedule().completed(target),
            Some(GenerationStage::Biomes)
        );
        assert!(
            biomed
                .work
                .schedule()
                .ready(target, GenerationStage::Terrain)
        );
        assert!(biomed.work.biomes().chunk([target.x, target.z]).is_some());
        let biome_live = biomed
            .generator
            .capture_live_inputs(&session, &biomed.terrain)?;
        biomed.generator.verify_live_inputs(&session, &biome_live)?;
        let (work, generator, terrain, evaluator, observed) =
            biomed.into_checked_parts(&streamer, &session)?;
        let biomed = BiomedMissingGeneration {
            work,
            generator,
            terrain,
            evaluator,
            observed,
        };
        let failure = match MissingGenerationTerrainWorker::start(
            biomed,
            &streamer,
            &session,
            0,
            BTreeMap::new(),
        ) {
            Ok(_) => anyhow::bail!("zero terrain budget was accepted"),
            Err(failure) => failure,
        };
        assert!(failure.error.to_string().contains("positive"));
        let mut terrain_worker = MissingGenerationTerrainWorker::start(
            failure.biomed,
            &streamer,
            &session,
            1,
            BTreeMap::new(),
        )
        .map_err(|failure| failure.error)?;
        let deadline = Instant::now() + Duration::from_secs(120);
        let first_terrain = loop {
            if let Some(result) = terrain_worker.try_result(&streamer, &session)? {
                break result;
            }
            anyhow::ensure!(Instant::now() < deadline, "terrain generation timed out");
            thread::sleep(Duration::from_millis(2));
        };
        assert_eq!(first_terrain.generated(), &[target]);
        assert!(first_terrain.saved().is_empty());
        assert!(first_terrain.remaining() > 0);
        let first_remaining = first_terrain.remaining();
        assert_eq!(
            first_terrain.state.work.schedule().completed(target),
            Some(GenerationStage::Terrain)
        );
        assert!(
            first_terrain
                .state
                .work
                .new_terrain_chunk(SavedChunkPos {
                    x: target.x,
                    z: target.z
                })
                .is_some()
        );
        let biomed = first_terrain.into_checked_state(&streamer, &session)?;
        let mut second_terrain_worker =
            MissingGenerationTerrainWorker::start(biomed, &streamer, &session, 1, BTreeMap::new())
                .map_err(|failure| failure.error)?;
        let deadline = Instant::now() + Duration::from_secs(120);
        let second_terrain = loop {
            if let Some(result) = second_terrain_worker.try_result(&streamer, &session)? {
                break result;
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "second terrain generation timed out"
            );
            thread::sleep(Duration::from_millis(2));
        };
        assert_eq!(second_terrain.generated().len(), 1);
        assert_ne!(second_terrain.generated()[0], target);
        assert_eq!(second_terrain.remaining() + 1, first_remaining);
        assert!(
            second_terrain
                .state
                .work
                .new_terrain_chunk(SavedChunkPos {
                    x: target.x,
                    z: target.z
                })
                .is_some()
        );
        let captured = capture_again()?;
        let mut prepare_cancelled_biomes =
            MissingGenerationPrepareWorker::start(captured, &streamer, &session, game.clone())
                .map_err(|failure| failure.error)?;
        let deadline = Instant::now() + Duration::from_secs(60);
        let prepared_cancelled_biomes = loop {
            if let Some(result) = prepare_cancelled_biomes.try_result(&streamer, &session)? {
                break result;
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "cancelled biome preparation timed out"
            );
            thread::sleep(Duration::from_millis(2));
        };
        let mut cancelled_biomes =
            MissingGenerationBiomeWorker::start(prepared_cancelled_biomes, &streamer, &session)
                .map_err(|failure| failure.error)?;
        cancelled_biomes.cancel();
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            match cancelled_biomes.try_result(&streamer, &session) {
                Ok(None) => {}
                Ok(Some(_)) => anyhow::bail!("cancelled biome generation was accepted"),
                Err(error) => {
                    anyhow::ensure!(
                        error.to_string().contains("cancelled"),
                        "unexpected cancellation error: {error:#}"
                    );
                    break;
                }
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "cancelled biome generation timed out"
            );
            thread::sleep(Duration::from_millis(2));
        }
        let captured = capture_again()?;
        let mut cancelled =
            MissingGenerationPrepareWorker::start(captured, &streamer, &session, game.clone())
                .map_err(|failure| failure.error)?;
        cancelled.cancel();
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            match cancelled.try_result(&streamer, &session) {
                Ok(None) => {}
                Ok(Some(_)) => anyhow::bail!("cancelled preparation was accepted"),
                Err(error) => {
                    anyhow::ensure!(
                        error.to_string().contains("cancelled"),
                        "unexpected cancellation error: {error:#}"
                    );
                    break;
                }
            }
            anyhow::ensure!(Instant::now() < deadline, "cancelled preparation timed out");
            thread::sleep(Duration::from_millis(2));
        }
        let stale_capture = capture_again()?;
        session.queue_loaded_chunk(target, chunk_world(target));
        assert!(!prepared.is_current(&streamer, &session));
        assert!(!second_terrain.is_current(&streamer, &session));
        assert!(
            second_terrain
                .state
                .generator
                .verify_live_inputs(&session, &biome_live)
                .is_err()
        );
        assert!(
            prepared
                .generator
                .verify_live_inputs(&session, &live)
                .is_err()
        );
        let failure =
            match MissingGenerationPrepareWorker::start(stale_capture, &streamer, &session, game) {
                Ok(_) => anyhow::bail!("stale capture was accepted"),
                Err(failure) => failure,
            };
        assert!(failure.error.to_string().contains("stale"));
        assert!(!failure.captured.is_current(&streamer, &session));
        drop(failure);
        let failure = match MissingGenerationBiomeWorker::start(prepared, &streamer, &session) {
            Ok(_) => anyhow::bail!("stale biome generation was accepted"),
            Err(failure) => failure,
        };
        assert!(failure.error.to_string().contains("stale"));
        assert!(
            failure
                .prepared
                .into_checked_parts(&streamer, &session)
                .is_err()
        );
        assert!(
            second_terrain
                .into_checked_state(&streamer, &session)
                .is_err()
        );
        drop(second_terrain_worker);
        drop(terrain_worker);
        drop(cancelled_biomes);
        drop(prepare_cancelled_biomes);
        drop(biome_worker);
        drop(prepare_biomes);
        drop(cancelled);
        drop(prepare);
        drop(capture);
        drop(streamer);
        drop(guard);
        wait_for_world_unlock(world.path());
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn missing_generation_preserves_saved_terrain_neighbor() -> Result<()> {
        use native_persistence::{
            nbt::{self, NamedTag, Tag, TagType},
            world::DimensionId,
            worldgen_settings::{NewWorldOptions, draft_baked_worldgen_settings},
        };
        use native_session::worldgen::schedule::GenerationStage;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let (world, reader) = locked_world_fixture();
        let guard = reader.session_guard().context("session guard missing")?;
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let settings = draft_baked_worldgen_settings(
            &mut resources,
            "minecraft:normal",
            NewWorldOptions {
                seed: 42,
                generate_structures: false,
                bonus_chest: false,
            },
        )?;
        std::fs::create_dir_all(world.path().join("data/minecraft"))?;
        std::fs::create_dir_all(world.path().join("datapacks"))?;
        std::fs::write(
            world.path().join("data/minecraft/world_gen_settings.dat"),
            nbt::write(&settings)?,
        )?;
        let saved_pos = ChunkPos { x: 1, z: 0 };
        let sections = (-4i8..20)
            .map(|y| {
                Tag::Compound(vec![
                    ("Y".into(), Tag::Byte(y)),
                    (
                        "block_states".into(),
                        Tag::Compound(vec![(
                            "palette".into(),
                            Tag::List {
                                element_type: TagType::String,
                                values: vec![Tag::String("minecraft:air".into())],
                            },
                        )]),
                    ),
                    (
                        "biomes".into(),
                        Tag::Compound(vec![(
                            "palette".into(),
                            Tag::List {
                                element_type: TagType::String,
                                values: vec![Tag::String("minecraft:plains".into())],
                            },
                        )]),
                    ),
                ])
            })
            .collect();
        let saved = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("xPos".into(), Tag::Int(saved_pos.x)),
                ("zPos".into(), Tag::Int(saved_pos.z)),
                ("Status".into(), Tag::String("minecraft:full".into())),
                ("custom".into(), Tag::Int(73)),
                (
                    "sections".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: sections,
                    },
                ),
                (
                    "structures".into(),
                    Tag::Compound(vec![
                        ("starts".into(), Tag::Compound(vec![])),
                        ("References".into(), Tag::Compound(vec![])),
                    ]),
                ),
            ]),
        };
        let bytes = nbt::write(&saved)?;
        let sectors = (bytes.len() + 5).div_ceil(4096);
        anyhow::ensure!(sectors < 256, "saved terrain fixture is too large");
        let mut region = vec![0; 8192 + sectors * 4096];
        region[4..8].copy_from_slice(&((2u32 << 8) | sectors as u32).to_be_bytes());
        region[8192..8196].copy_from_slice(&u32::try_from(bytes.len() + 1)?.to_be_bytes());
        region[8196] = 3;
        region[8197..8197 + bytes.len()].copy_from_slice(&bytes);
        let region_dir = world.path().join("dimensions/minecraft/overworld/region");
        std::fs::create_dir_all(&region_dir)?;
        let region_path = region_dir.join("r.0.0.mca");
        std::fs::write(&region_path, &region)?;
        let player = PlayerState {
            position: Vec3::new(8.5, 80.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let mut session = LoadedSession::new(
            World::new("minecraft:overworld"),
            IVec3::ZERO,
            BuildBounds {
                min_y: -64,
                max_y: 320,
            },
            player,
        )
        .map_err(anyhow::Error::msg)?;
        let mut streamer = ChunkStreamer::start(reader, BTreeMap::new(), 1, BTreeSet::new(), None)?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            streamer.service(&mut session)?;
            session.update(Duration::from_millis(50));
            if streamer.pending.is_empty()
                && streamer.missing.len() == 8
                && session.has_chunk_or_pending(saved_pos)
            {
                break;
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "saved terrain streaming timed out"
            );
            thread::sleep(Duration::from_millis(2));
        }
        let targets = streamer
            .missing_generation_targets(&session, 1)
            .context("targets missing")?;
        let target = targets.targets()[0];
        let reader = WorldReader::open_with_session(
            world.path(),
            DimensionId::parse("overworld")?,
            guard.clone(),
        )?;
        let mut capture =
            MissingGenerationCaptureWorker::start(targets, &streamer, &session, reader)?;
        let deadline = Instant::now() + Duration::from_secs(60);
        let captured = loop {
            if let Some(result) = capture.try_result(&streamer, &session)? {
                break result;
            }
            anyhow::ensure!(Instant::now() < deadline, "capture timed out");
            thread::sleep(Duration::from_millis(2));
        };
        let mut prepare =
            MissingGenerationPrepareWorker::start(captured, &streamer, &session, game)
                .map_err(|failure| failure.error)?;
        let deadline = Instant::now() + Duration::from_secs(60);
        let prepared = loop {
            if let Some(result) = prepare.try_result(&streamer, &session)? {
                break result;
            }
            anyhow::ensure!(Instant::now() < deadline, "preparation timed out");
            thread::sleep(Duration::from_millis(2));
        };
        assert!(prepared.terrain.world().chunks.contains_key(&saved_pos));
        let mut biome = MissingGenerationBiomeWorker::start(prepared, &streamer, &session)
            .map_err(|failure| failure.error)?;
        let deadline = Instant::now() + Duration::from_secs(120);
        let biomed = loop {
            if let Some(result) = biome.try_result(&streamer, &session)? {
                break result;
            }
            anyhow::ensure!(Instant::now() < deadline, "biome generation timed out");
            thread::sleep(Duration::from_millis(2));
        };
        let mut terrain =
            MissingGenerationTerrainWorker::start(biomed, &streamer, &session, 1, BTreeMap::new())
                .map_err(|failure| failure.error)?;
        let deadline = Instant::now() + Duration::from_secs(120);
        let result = loop {
            if let Some(result) = terrain.try_result(&streamer, &session)? {
                break result;
            }
            anyhow::ensure!(Instant::now() < deadline, "terrain generation timed out");
            thread::sleep(Duration::from_millis(2));
        };
        assert_eq!(result.generated(), &[target]);
        assert!(result.saved().contains(&(saved_pos, GenerationStage::Full)));
        assert_eq!(
            result.state.work.schedule().completed(saved_pos),
            Some(GenerationStage::Biomes)
        );
        assert!(
            result
                .state
                .work
                .new_terrain_chunk(SavedChunkPos {
                    x: saved_pos.x,
                    z: saved_pos.z
                })
                .is_none()
        );
        assert_eq!(
            result
                .state
                .terrain
                .world()
                .block(16, 0, 0)
                .map(|state| state.name.as_str()),
            Some("minecraft:air")
        );
        assert_eq!(std::fs::read(&region_path)?, region);
        let state = result.into_checked_state(&streamer, &session)?;
        let mut cancelled =
            MissingGenerationTerrainWorker::start(state, &streamer, &session, 1, BTreeMap::new())
                .map_err(|failure| failure.error)?;
        cancelled.cancel();
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            match cancelled.try_result(&streamer, &session) {
                Err(error) => {
                    assert!(error.to_string().contains("cancelled"));
                    break;
                }
                Ok(None) => {}
                Ok(Some(_)) => anyhow::bail!("cancelled terrain was published"),
            }
            anyhow::ensure!(Instant::now() < deadline, "terrain cancellation timed out");
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(std::fs::read(&region_path)?, region);
        drop(cancelled);
        drop(terrain);
        drop(biome);
        drop(prepare);
        drop(capture);
        drop(streamer);
        drop(guard);
        wait_for_world_unlock(world.path());
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn streamer_retains_the_world_lock_until_it_stops() {
        let (world, reader) = locked_world_fixture();
        let streamer =
            ChunkStreamer::start(reader, BTreeMap::new(), 1, BTreeSet::new(), None).unwrap();
        assert!(native_persistence::save::WorldWriteGuard::acquire(world.path()).is_err());
        drop(streamer);
        wait_for_world_unlock(world.path());
    }

    #[cfg(windows)]
    #[test]
    fn closing_does_not_release_the_lock_while_a_background_read_is_running() {
        let (world, reader) = locked_world_fixture();
        let guard = reader.session_guard().unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let mut streamer = ChunkStreamer::with_source(1, BTreeSet::new(), move |_| {
            let _hold_reader = &reader;
            started_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            Ok(None)
        })
        .unwrap();
        streamer._world_guard = Some(guard);
        let player = PlayerState {
            position: Vec3::new(8.5, 1.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let mut session = LoadedSession::new(
            chunk_world(ChunkPos { x: 0, z: 0 }),
            IVec3::ZERO,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        streamer.service(&mut session).unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        drop(streamer);
        assert!(native_persistence::save::WorldWriteGuard::acquire(world.path()).is_err());
        release_tx.send(()).unwrap();
        wait_for_world_unlock(world.path());
    }

    #[cfg(windows)]
    #[test]
    fn stopped_reader_does_not_unlock_a_still_active_session() {
        let (world, reader) = locked_world_fixture();
        let guard = reader.session_guard().unwrap();
        let mut streamer = ChunkStreamer::with_source(1, BTreeSet::new(), move |_| {
            let _hold_reader = &reader;
            panic!("simulated reader failure");
        })
        .unwrap();
        streamer._world_guard = Some(guard);
        let player = PlayerState {
            position: Vec3::new(8.5, 1.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let mut session = LoadedSession::new(
            chunk_world(ChunkPos { x: 0, z: 0 }),
            IVec3::ZERO,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while streamer.service(&mut session).is_ok() {
            assert!(Instant::now() < deadline, "reader failure was not detected");
            thread::sleep(Duration::from_millis(2));
        }
        assert!(native_persistence::save::WorldWriteGuard::acquire(world.path()).is_err());
        drop(streamer);
        wait_for_world_unlock(world.path());
    }

    #[cfg(windows)]
    #[test]
    fn writer_invalidates_queued_results_and_retries_missing_chunks() {
        let (_world, reader) = locked_world_fixture();
        let guard = reader.session_guard().unwrap();
        let stamp = guard.read_stamp().unwrap();
        let home = ChunkPos { x: 0, z: 0 };
        let target = ChunkPos { x: 1, z: 0 };
        let missing = ChunkPos { x: 0, z: 1 };
        let player = PlayerState {
            position: Vec3::new(8.5, 1.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let mut session = LoadedSession::new(
            chunk_world(home),
            IVec3::ZERO,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        session.queue_streamed_chunk(
            target,
            Some(chunk_world(target)),
            Some(SavedEntityData::Empty),
            Some(SavedPoiData::Missing),
            None,
            Some(stamp.clone()),
        );
        session.queue_streamed_chunk(missing, None, None, None, None, Some(stamp));
        let mut streamer =
            ChunkStreamer::with_entity_source(1, BTreeSet::from([target]), move |pos, _| {
                Ok((pos == target).then(|| ChunkPayload {
                    world: Some(chunk_world(pos)),
                    entities: SavedEntityData::Empty,
                    poi: SavedPoiData::Missing,
                    heightmaps: None,
                }))
            })
            .unwrap();
        streamer.observed_epoch = Some(guard.read_stamp().unwrap().epoch());
        streamer._world_guard = Some(guard.clone());
        drop(guard.acquire_write().unwrap());
        session.update(Duration::from_millis(50));
        assert!(!session.has_resident_or_parked_chunk(target));
        assert!(
            !session
                .snapshot()
                .world
                .loaded()
                .unwrap()
                .missing_chunks
                .contains(&missing)
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            streamer.service(&mut session).unwrap();
            session.update(Duration::from_millis(50));
            if session.has_resident_or_parked_chunk(target) {
                break;
            }
            assert!(Instant::now() < deadline, "stale chunk was not retried");
            thread::sleep(Duration::from_millis(2));
        }
        assert!(matches!(
            session.saved_entity_chunk(target),
            Some(SavedEntityData::Empty)
        ));
        assert!(matches!(
            session.saved_poi_chunk(target),
            Some(SavedPoiData::Missing)
        ));
    }

    #[test]
    fn retries_unavailable_entities_or_poi_for_a_resident_chunk() {
        for retry_poi in [false, true] {
            let home = ChunkPos { x: 0, z: 0 };
            let player = PlayerState {
                position: Vec3::new(8.5, 1.0, 8.5),
                yaw: 0.0,
                pitch: 0.0,
                vertical_velocity: 0.0,
                on_ground: false,
                pose: native_session::PlayerPose::Standing,
            };
            let mut session = LoadedSession::new(
                chunk_world(home),
                IVec3::ZERO,
                BuildBounds {
                    min_y: 0,
                    max_y: 32,
                },
                player,
            )
            .unwrap();
            session
                .set_initial_entity_chunk(
                    home,
                    if retry_poi {
                        SavedEntityData::Empty
                    } else {
                        SavedEntityData::Unavailable
                    },
                )
                .unwrap();
            session
                .set_initial_poi_chunk(
                    home,
                    if retry_poi {
                        SavedPoiData::Unavailable
                    } else {
                        SavedPoiData::Missing
                    },
                )
                .unwrap();
            let mut streamer =
                ChunkStreamer::with_entity_source(1, BTreeSet::new(), move |pos, entities_only| {
                    if pos == home {
                        assert!(entities_only);
                    }
                    Ok((pos == home).then_some(ChunkPayload {
                        world: None,
                        entities: SavedEntityData::Empty,
                        poi: SavedPoiData::Missing,
                        heightmaps: None,
                    }))
                })
                .unwrap();
            streamer.service(&mut session).unwrap();
            assert!(streamer.entity_retry.contains_key(&home));
            streamer.entity_retry.get_mut(&home).unwrap().0 = Instant::now();
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                streamer.service(&mut session).unwrap();
                session.update(Duration::from_millis(50));
                if matches!(
                    session.saved_entity_chunk(home),
                    Some(SavedEntityData::Empty)
                ) && matches!(session.saved_poi_chunk(home), Some(SavedPoiData::Missing))
                {
                    break;
                }
                assert!(Instant::now() < deadline, "entity retry did not complete");
                thread::sleep(Duration::from_millis(2));
            }
            streamer.service(&mut session).unwrap();
            assert!(!streamer.entity_retry.contains_key(&home));
        }
    }

    #[test]
    fn transient_terrain_errors_retry_with_backoff_and_recover() {
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
        let home = ChunkPos { x: 0, z: 0 };
        let target = ChunkPos { x: 1, z: 0 };
        let player = PlayerState {
            position: Vec3::new(8.5, 1.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let mut session = LoadedSession::new(
            chunk_world(home),
            IVec3::ZERO,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        let missing: BTreeSet<_> = targets(home, 1)
            .into_iter()
            .filter(|&pos| pos != home && pos != target)
            .collect();
        let attempts = Arc::new(AtomicUsize::new(0));
        let worker_attempts = Arc::clone(&attempts);
        let mut streamer = ChunkStreamer::with_source(1, missing, move |pos| {
            assert_eq!(pos, target);
            if worker_attempts.fetch_add(1, AtomicOrdering::SeqCst) < 2 {
                anyhow::bail!("temporary read error")
            }
            Ok(Some(chunk_world(pos)))
        })
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        for expected_attempt in 0..=1 {
            loop {
                streamer.service(&mut session).unwrap();
                session.update(Duration::from_millis(50));
                if streamer
                    .failed
                    .get(&target)
                    .is_some_and(|(_, n)| *n == expected_attempt)
                {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "terrain failure was not recorded"
                );
                thread::sleep(Duration::from_millis(2));
            }
            assert!(!streamer.missing.contains(&target));
            assert!(!session.has_resident_or_parked_chunk(target));
            assert!(streamer.failed[&target].0 > Instant::now());
            streamer.failed.get_mut(&target).unwrap().0 = Instant::now();
        }
        loop {
            streamer.service(&mut session).unwrap();
            session.update(Duration::from_millis(50));
            if session.has_resident_or_parked_chunk(target) {
                break;
            }
            assert!(Instant::now() < deadline, "terrain retry did not recover");
            thread::sleep(Duration::from_millis(2));
        }
        streamer.service(&mut session).unwrap();
        assert_eq!(attempts.load(AtomicOrdering::SeqCst), 3);
        assert!(!streamer.failed.contains_key(&target));
        assert!(!streamer.missing.contains(&target));
    }

    #[test]
    fn rejected_entity_and_poi_attachments_retry_after_the_tick() {
        use native_persistence::{
            nbt::{NamedTag, Tag},
            world::{SavedEntityChunk, SavedPoiChunk},
        };
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

        let home = ChunkPos { x: 0, z: 0 };
        let target = ChunkPos { x: 1, z: 0 };
        let player = PlayerState {
            position: Vec3::new(8.5, 1.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let mut session = LoadedSession::new(
            chunk_world(home),
            IVec3::ZERO,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        let missing = targets(home, 1)
            .into_iter()
            .filter(|&pos| pos != home && pos != target)
            .collect();
        let attempts = Arc::new(AtomicUsize::new(0));
        let worker_attempts = Arc::clone(&attempts);
        let mut streamer =
            ChunkStreamer::with_entity_source(1, missing, move |pos, entities_only| {
                assert_eq!(pos, target);
                let attempt = worker_attempts.fetch_add(1, AtomicOrdering::SeqCst);
                if attempt == 0 {
                    assert!(!entities_only);
                    let document = Arc::new(NamedTag {
                        name: "".into(),
                        value: Tag::Compound(vec![]),
                    });
                    let wrong = SavedChunkPos { x: 9, z: 9 };
                    Ok(Some(ChunkPayload {
                        world: Some(chunk_world(target)),
                        entities: SavedEntityData::Present(Arc::new(SavedEntityChunk {
                            position: wrong,
                            document: Arc::clone(&document),
                        })),
                        poi: SavedPoiData::Present(Arc::new(SavedPoiChunk {
                            position: wrong,
                            document,
                            sections: BTreeMap::new(),
                        })),
                        heightmaps: None,
                    }))
                } else {
                    assert!(entities_only);
                    Ok(Some(ChunkPayload {
                        world: None,
                        entities: SavedEntityData::Empty,
                        poi: SavedPoiData::Missing,
                        heightmaps: None,
                    }))
                }
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !streamer.awaiting_attachment.contains(&target) {
            streamer.service(&mut session).unwrap();
            assert!(Instant::now() < deadline, "initial read did not finish");
            thread::sleep(Duration::from_millis(2));
        }
        // Repeated render frames must not start another read before the tick.
        for _ in 0..5 {
            streamer.service(&mut session).unwrap();
        }
        assert_eq!(attempts.load(AtomicOrdering::SeqCst), 1);
        session.update(Duration::from_millis(50));
        streamer.service(&mut session).unwrap();
        assert!(session.has_resident_or_parked_chunk(target));
        assert!(matches!(
            session.saved_entity_chunk(target),
            Some(SavedEntityData::Unavailable)
        ));
        assert!(streamer.entity_retry.contains_key(&target));
        streamer.entity_retry.get_mut(&target).unwrap().0 = Instant::now();
        loop {
            streamer.service(&mut session).unwrap();
            session.update(Duration::from_millis(50));
            if matches!(
                session.saved_entity_chunk(target),
                Some(SavedEntityData::Empty)
            ) && matches!(session.saved_poi_chunk(target), Some(SavedPoiData::Missing))
            {
                streamer.service(&mut session).unwrap();
                break;
            }
            assert!(
                Instant::now() < deadline,
                "attachment retry did not recover"
            );
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(attempts.load(AtomicOrdering::SeqCst), 2);
        assert!(!streamer.entity_retry.contains_key(&target));
    }

    #[test]
    fn rejected_terrain_attachment_retries_without_marking_the_chunk_missing() {
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
        let home = ChunkPos { x: 0, z: 0 };
        let target = ChunkPos { x: 1, z: 0 };
        let player = PlayerState {
            position: Vec3::new(8.5, 1.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let mut session = LoadedSession::new(
            chunk_world(home),
            IVec3::ZERO,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        let missing = targets(home, 1)
            .into_iter()
            .filter(|&pos| pos != home && pos != target)
            .collect();
        let attempts = Arc::new(AtomicUsize::new(0));
        let worker_attempts = Arc::clone(&attempts);
        let mut streamer = ChunkStreamer::with_source(1, missing, move |pos| {
            assert_eq!(pos, target);
            if worker_attempts.fetch_add(1, AtomicOrdering::SeqCst) == 0 {
                Ok(Some(World::new("minecraft:the_nether")))
            } else {
                Ok(Some(chunk_world(target)))
            }
        })
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !streamer.failed.contains_key(&target) {
            streamer.service(&mut session).unwrap();
            session.update(Duration::from_millis(50));
            assert!(
                Instant::now() < deadline,
                "terrain rejection was not recorded"
            );
            thread::sleep(Duration::from_millis(2));
        }
        assert!(!streamer.missing.contains(&target));
        assert!(!session.has_resident_or_parked_chunk(target));
        streamer.failed.get_mut(&target).unwrap().0 = Instant::now();
        loop {
            streamer.service(&mut session).unwrap();
            session.update(Duration::from_millis(50));
            if session.has_resident_or_parked_chunk(target) {
                streamer.service(&mut session).unwrap();
                break;
            }
            assert!(
                Instant::now() < deadline,
                "terrain attachment did not recover"
            );
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(attempts.load(AtomicOrdering::SeqCst), 2);
        assert!(!streamer.failed.contains_key(&target));
        assert!(!streamer.missing.contains(&target));
    }

    #[test]
    fn entity_only_failures_do_not_mark_resident_terrain_missing() {
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
        let home = ChunkPos { x: 0, z: 0 };
        let player = PlayerState {
            position: Vec3::new(8.5, 1.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let mut session = LoadedSession::new(
            chunk_world(home),
            IVec3::ZERO,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        session
            .set_initial_entity_chunk(home, SavedEntityData::Unavailable)
            .unwrap();
        session
            .set_initial_poi_chunk(home, SavedPoiData::Missing)
            .unwrap();
        let missing: BTreeSet<_> = targets(home, 1)
            .into_iter()
            .filter(|&pos| pos != home)
            .collect();
        let attempts = Arc::new(AtomicUsize::new(0));
        let worker_attempts = Arc::clone(&attempts);
        let mut streamer =
            ChunkStreamer::with_entity_source(1, missing, move |pos, entities_only| {
                assert_eq!(pos, home);
                assert!(entities_only);
                match worker_attempts.fetch_add(1, AtomicOrdering::SeqCst) {
                    0 => anyhow::bail!("temporary entity read failure"),
                    1 => Ok(None),
                    _ => Ok(Some(ChunkPayload {
                        world: None,
                        entities: SavedEntityData::Empty,
                        poi: SavedPoiData::Missing,
                        heightmaps: None,
                    })),
                }
            })
            .unwrap();
        streamer.service(&mut session).unwrap();
        // Another loader can supply terrain while an old failure is pending.
        streamer.failed.insert(home, (Instant::now(), 0));
        streamer.service(&mut session).unwrap();
        assert!(!streamer.failed.contains_key(&home));
        let deadline = Instant::now() + Duration::from_secs(5);
        for expected_attempt in 1..=2 {
            streamer.entity_retry.get_mut(&home).unwrap().0 = Instant::now();
            loop {
                streamer.service(&mut session).unwrap();
                session.update(Duration::from_millis(50));
                if streamer
                    .entity_retry
                    .get(&home)
                    .is_some_and(|(_, n)| *n == expected_attempt)
                {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "entity retry failure was not recorded"
                );
                thread::sleep(Duration::from_millis(2));
            }
            assert!(!streamer.missing.contains(&home));
            assert!(!streamer.failed.contains_key(&home));
            assert!(session.has_resident_or_parked_chunk(home));
        }
        streamer.entity_retry.get_mut(&home).unwrap().0 = Instant::now();
        loop {
            streamer.service(&mut session).unwrap();
            session.update(Duration::from_millis(50));
            if matches!(
                session.saved_entity_chunk(home),
                Some(SavedEntityData::Empty)
            ) {
                break;
            }
            assert!(Instant::now() < deadline, "entity retry did not recover");
            thread::sleep(Duration::from_millis(2));
        }
        streamer.service(&mut session).unwrap();
        assert_eq!(attempts.load(AtomicOrdering::SeqCst), 3);
        assert!(!streamer.entity_retry.contains_key(&home));
    }

    #[test]
    fn obsolete_missing_and_error_responses_do_not_poison_future_loads() {
        let home = ChunkPos { x: 0, z: 0 };
        let player = PlayerState {
            position: Vec3::new(8.5, 1.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let mut session = LoadedSession::new(
            chunk_world(home),
            IVec3::ZERO,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        let mut streamer = ChunkStreamer::with_source(1, BTreeSet::new(), |_| Ok(None)).unwrap();
        let (sender, receiver) = mpsc::channel();
        streamer.responses = receiver;
        streamer.center = Some(home);
        let missing = ChunkPos { x: 4, z: 0 };
        let failed = ChunkPos { x: 5, z: 0 };
        streamer.pending.extend([missing, failed]);
        sender
            .send(Response {
                pos: missing,
                result: Ok(None),
                stale: false,
                entities_only: false,
                stamp: None,
            })
            .unwrap();
        sender
            .send(Response {
                pos: failed,
                result: Err("transient".into()),
                stale: false,
                entities_only: false,
                stamp: None,
            })
            .unwrap();
        streamer.service(&mut session).unwrap();
        assert!(!streamer.missing.contains(&missing));
        assert!(!streamer.failed.contains_key(&failed));
        assert!(!streamer.pending.contains(&missing));
        assert!(!streamer.pending.contains(&failed));
    }

    #[test]
    fn absent_or_failed_read_does_not_hide_a_concurrently_delivered_chunk() {
        for (already_attached, sidecars_known, failed_read) in [
            (false, true, false),
            (true, true, false),
            (false, false, false),
            (true, false, false),
            (false, true, true),
            (true, true, true),
            (false, false, true),
            (true, false, true),
        ] {
            let home = ChunkPos { x: 0, z: 0 };
            let target = ChunkPos { x: 1, z: 0 };
            let player = PlayerState {
                position: Vec3::new(8.5, 1.0, 8.5),
                yaw: 0.0,
                pitch: 0.0,
                vertical_velocity: 0.0,
                on_ground: false,
                pose: native_session::PlayerPose::Standing,
            };
            let mut session = LoadedSession::new(
                chunk_world(home),
                IVec3::ZERO,
                BuildBounds {
                    min_y: 0,
                    max_y: 32,
                },
                player,
            )
            .unwrap();
            if sidecars_known {
                session.queue_loaded_chunk_with_poi(
                    target,
                    chunk_world(target),
                    SavedEntityData::Empty,
                    Some(SavedPoiData::Missing),
                    None,
                );
            } else {
                session.queue_loaded_chunk(target, chunk_world(target));
            }
            if already_attached {
                session.update(Duration::from_millis(50));
                assert!(session.has_resident_or_parked_chunk(target));
            }

            let mut streamer =
                ChunkStreamer::with_source(1, BTreeSet::new(), |_| Ok(None)).unwrap();
            let (response_tx, responses) = mpsc::channel();
            let (requests, request_rx) = mpsc::sync_channel(8);
            streamer.responses = responses;
            streamer.requests = Some(requests);
            streamer.center = Some(home);
            streamer.targets = vec![target];
            streamer.pending.insert(target);
            response_tx
                .send(Response {
                    pos: target,
                    result: if failed_read {
                        Err("temporary read failure".into())
                    } else {
                        Ok(None)
                    },
                    stale: false,
                    entities_only: false,
                    stamp: None,
                })
                .unwrap();

            streamer.service(&mut session).unwrap();
            assert!(
                !streamer.missing.contains(&target),
                "delivered chunk was cached as absent"
            );
            assert!(
                !streamer.entity_retry.contains_key(&target),
                "sidecars must not be inferred from an absent terrain read"
            );
            session.update(Duration::from_millis(50));
            assert!(session.has_resident_or_parked_chunk(target));
            streamer.service(&mut session).unwrap();
            assert!(!streamer.failed.contains_key(&target));
            assert!(
                request_rx.try_recv().is_err(),
                "resident terrain should not be re-read"
            );
            if sidecars_known {
                assert!(matches!(
                    session.saved_entity_chunk(target),
                    Some(SavedEntityData::Empty)
                ));
                assert!(matches!(
                    session.saved_poi_chunk(target),
                    Some(SavedPoiData::Missing)
                ));
            } else {
                assert!(matches!(
                    session.saved_entity_chunk(target),
                    Some(SavedEntityData::Unavailable)
                ));
            }
        }
    }

    #[test]
    fn absent_or_failed_read_retries_if_competing_chunk_is_rejected() {
        for failed_read in [false, true] {
            let home = ChunkPos { x: 0, z: 0 };
            let target = ChunkPos { x: 1, z: 0 };
            let player = PlayerState {
                position: Vec3::new(8.5, 1.0, 8.5),
                yaw: 0.0,
                pitch: 0.0,
                vertical_velocity: 0.0,
                on_ground: false,
                pose: native_session::PlayerPose::Standing,
            };
            let mut session = LoadedSession::new(
                chunk_world(home),
                IVec3::ZERO,
                BuildBounds {
                    min_y: 0,
                    max_y: 32,
                },
                player,
            )
            .unwrap();
            session.queue_loaded_chunk(target, World::new("minecraft:the_nether"));
            let mut streamer =
                ChunkStreamer::with_source(1, BTreeSet::new(), |_| Ok(None)).unwrap();
            let (response_tx, responses) = mpsc::channel();
            let (requests, request_rx) = mpsc::sync_channel(8);
            streamer.responses = responses;
            streamer.requests = Some(requests);
            streamer.center = Some(home);
            streamer.targets = vec![target];
            streamer.pending.insert(target);
            response_tx
                .send(Response {
                    pos: target,
                    result: if failed_read {
                        Err("temporary read failure".into())
                    } else {
                        Ok(None)
                    },
                    stale: false,
                    entities_only: false,
                    stamp: None,
                })
                .unwrap();
            streamer.service(&mut session).unwrap();
            if failed_read {
                assert!(streamer.failed.contains_key(&target));
            } else {
                assert!(streamer.awaiting_competing_attachment.contains(&target));
            }
            assert!(request_rx.try_recv().is_err());
            session.update(Duration::from_millis(50));
            assert!(!session.has_chunk_or_pending(target));
            if failed_read {
                streamer.failed.get_mut(&target).unwrap().0 = Instant::now();
            }
            streamer.service(&mut session).unwrap();
            let request = request_rx
                .try_recv()
                .expect("rejected terrain was not retried");
            assert_eq!(request.pos, target);
            assert!(!request.entities_only);
            assert!(!streamer.missing.contains(&target));
        }
    }

    fn chunk_world(pos: ChunkPos) -> World {
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let stone = world.intern_state(BlockState {
            name: "minecraft:stone".into(),
            properties: BTreeMap::new(),
        });
        let mut blocks = Box::new([air; BLOCK_COUNT]);
        for z in 0..16 {
            for x in 0..16 {
                blocks[x + z * 16] = stone;
            }
        }
        world.chunks.insert(
            pos,
            Arc::new(Chunk {
                block_light_correct: false,
                position: pos,
                status: Some("minecraft:full".into()),
                sections: BTreeMap::from([(
                    0,
                    Arc::new(Section {
                        blocks: Some(blocks),
                        biomes: None,
                        block_light: None,
                        sky_light: None,
                    }),
                )]),
            }),
        );
        world
    }

    #[test]
    fn requests_outside_chunk_needed_by_live_shape_update() {
        use native_world::loaded::SavedFluidTick;
        for exists in [true, false] {
            let home = ChunkPos { x: 0, z: 0 };
            let source_chunk = ChunkPos { x: 1, z: 0 };
            let target = ChunkPos { x: 2, z: 0 };
            let mut source = chunk_world(source_chunk);
            let water = source.intern_state(BlockState {
                name: "minecraft:water".into(),
                properties: BTreeMap::from([("level".into(), "0".into())]),
            });
            let stone = source.intern_state(BlockState {
                name: "minecraft:stone".into(),
                properties: BTreeMap::new(),
            });
            source.set_block_id(30, 3, 8, water).unwrap();
            for (x, y, z) in [(30, 2, 8), (31, 2, 8), (29, 3, 8), (30, 3, 7), (30, 3, 9)] {
                source.set_block_id(x, y, z, stone).unwrap();
            }
            source.saved_fluid_ticks.insert(
                source_chunk,
                vec![SavedFluidTick {
                    fluid: "minecraft:water".into(),
                    position: [30, 3, 8],
                    delay: 0,
                    priority: 0,
                }],
            );
            let mut world = chunk_world(home);
            assert!(world.import_chunk(&source, source_chunk).unwrap());
            let player = PlayerState {
                position: Vec3::new(8.5, 1.0, 8.5),
                yaw: 0.0,
                pitch: 0.0,
                vertical_velocity: 0.0,
                on_ground: false,
                pose: native_session::PlayerPose::Standing,
            };
            let mut session = LoadedSession::new(
                world,
                IVec3::ZERO,
                BuildBounds {
                    min_y: 0,
                    max_y: 32,
                },
                player,
            )
            .unwrap();
            let missing = targets(home, 1)
                .into_iter()
                .filter(|pos| *pos != home && *pos != source_chunk)
                .collect();
            let (sent, received) = mpsc::channel();
            let mut streamer = ChunkStreamer::with_source(1, missing, move |pos| {
                sent.send(pos).unwrap();
                Ok((pos == target && exists).then(|| chunk_world(pos)))
            })
            .unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut requests = 0;
            loop {
                streamer.service(&mut session).unwrap();
                session.update(Duration::from_millis(50));
                requests += received.try_iter().filter(|pos| *pos == target).count();
                if requests > 0
                    && ((exists && session.pending_lichen_update_chunks().next().is_none())
                        || (!exists && streamer.missing.contains(&target)))
                {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "outside chunk was not read and applied"
                );
                thread::sleep(Duration::from_millis(2));
            }
            if !exists {
                for _ in 0..10 {
                    streamer.service(&mut session).unwrap();
                    session.update(Duration::from_millis(50));
                    requests += received.try_iter().filter(|pos| *pos == target).count();
                }
                assert_eq!(requests, 1, "missing target was requested repeatedly");
                assert!(
                    session
                        .pending_lichen_update_chunks()
                        .any(|pos| pos == target)
                );
            }
            assert!(requests > 0);
            assert!(
                !session
                    .snapshot()
                    .world
                    .loaded()
                    .unwrap()
                    .chunks
                    .contains_key(&target)
            );
        }
    }

    #[test]
    fn requests_outside_chunk_needed_by_a_stair_update() {
        use native_world::loaded::SavedFluidTick;
        let home = ChunkPos { x: 0, z: 0 };
        let source_chunk = ChunkPos { x: 1, z: 0 };
        let target = ChunkPos { x: 2, z: 0 };
        let mut source = chunk_world(source_chunk);
        let water = source.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "4".into())]),
        });
        let stone = source.intern_state(BlockState {
            name: "minecraft:stone".into(),
            properties: BTreeMap::new(),
        });
        let stair = source.intern_state(BlockState {
            name: "minecraft:oak_stairs".into(),
            properties: [
                ("facing", "east"),
                ("half", "bottom"),
                ("shape", "inner_right"),
                ("waterlogged", "false"),
            ]
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect(),
        });
        source.set_block_id(30, 3, 8, water).unwrap();
        source.set_block_id(31, 3, 8, stair).unwrap();
        for (x, y, z) in [(30, 2, 8), (29, 3, 8), (30, 3, 7), (30, 3, 9)] {
            source.set_block_id(x, y, z, stone).unwrap();
        }
        source.saved_fluid_ticks.insert(
            source_chunk,
            vec![SavedFluidTick {
                fluid: "minecraft:flowing_water".into(),
                position: [30, 3, 8],
                delay: 0,
                priority: 0,
            }],
        );
        let mut world = chunk_world(home);
        assert!(world.import_chunk(&source, source_chunk).unwrap());
        let player = PlayerState {
            position: Vec3::new(8.5, 1.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let mut session = LoadedSession::new(
            world,
            IVec3::ZERO,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        session.set_vanilla_block_ids(Arc::new(
            [
                "minecraft:air",
                "minecraft:stone",
                "minecraft:oak_stairs",
                "minecraft:water",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        ));
        let missing = targets(home, 1)
            .into_iter()
            .filter(|pos| *pos != home && *pos != source_chunk)
            .collect();
        let (sent, received) = mpsc::channel();
        let mut streamer = ChunkStreamer::with_source(1, missing, move |pos| {
            sent.send(pos).unwrap();
            Ok((pos == target).then(|| chunk_world(pos)))
        })
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut requested = false;
        loop {
            streamer.service(&mut session).unwrap();
            session.update(Duration::from_millis(50));
            requested |= received.try_iter().any(|pos| pos == target);
            if requested && session.required_shape_update_chunks().is_empty() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "stair neighbor was not read and applied"
            );
            thread::sleep(Duration::from_millis(2));
        }
        let snapshot = session.snapshot();
        let world = snapshot.world.loaded().unwrap();
        assert_eq!(
            world.block(31, 3, 8).unwrap().properties["shape"],
            "straight"
        );
        assert!(!world.chunks.contains_key(&target));
    }

    #[test]
    fn changing_radius_without_moving_loads_and_unloads_chunks() {
        let home = ChunkPos { x: 0, z: 0 };
        let far = ChunkPos { x: 2, z: 0 };
        let player = PlayerState {
            position: Vec3::new(8.5, 1.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let mut session = LoadedSession::new(
            chunk_world(home),
            IVec3::ZERO,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        let mut streamer = ChunkStreamer::with_source(1, BTreeSet::new(), move |pos| {
            Ok((pos == far).then(|| chunk_world(pos)))
        })
        .unwrap();
        streamer.service(&mut session).unwrap();
        session.update(Duration::from_millis(50));
        assert!(
            !session
                .snapshot()
                .world
                .loaded()
                .unwrap()
                .chunks
                .contains_key(&far)
        );
        streamer.set_radius(2);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !session
            .snapshot()
            .world
            .loaded()
            .unwrap()
            .chunks
            .contains_key(&far)
        {
            streamer.service(&mut session).unwrap();
            session.update(Duration::from_millis(50));
            assert!(
                Instant::now() < deadline,
                "expanded window did not load the distant chunk"
            );
            thread::sleep(Duration::from_millis(2));
        }
        streamer.set_radius(1);
        streamer.service(&mut session).unwrap();
        session.update(Duration::from_millis(50));
        assert!(
            !session
                .snapshot()
                .world
                .loaded()
                .unwrap()
                .chunks
                .contains_key(&far)
        );
        streamer.set_radius(2);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !session
            .snapshot()
            .world
            .loaded()
            .unwrap()
            .chunks
            .contains_key(&far)
        {
            streamer.service(&mut session).unwrap();
            session.update(Duration::from_millis(50));
            assert!(
                Instant::now() < deadline,
                "expanded window did not reload the distant chunk"
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn missing_cache_expires_outside_window_and_rechecks_on_return() {
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

        let home = ChunkPos { x: 0, z: 0 };
        let target = ChunkPos { x: 2, z: 0 };
        let player = PlayerState {
            position: Vec3::new(8.5, 1.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let mut session = LoadedSession::new(
            chunk_world(home),
            IVec3::ZERO,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        let reads = Arc::new(AtomicUsize::new(0));
        let worker_reads = Arc::clone(&reads);
        let mut streamer = ChunkStreamer::with_source(2, BTreeSet::new(), move |pos| {
            if pos == target && worker_reads.fetch_add(1, AtomicOrdering::SeqCst) > 0 {
                Ok(Some(chunk_world(pos)))
            } else {
                Ok(None)
            }
        })
        .unwrap();
        // Simulate negative cache entries accumulated during a long journey.
        streamer
            .missing
            .extend((-10_000..=10_000).map(|x| ChunkPos { x, z: 100 }));
        let deadline = Instant::now() + Duration::from_secs(8);
        while !streamer.missing.contains(&target) {
            streamer.service(&mut session).unwrap();
            session.update(Duration::from_millis(50));
            assert!(Instant::now() < deadline, "first read did not finish");
            thread::sleep(Duration::from_millis(2));
        }
        assert!(streamer.missing.len() <= 25);
        assert_eq!(reads.load(AtomicOrdering::SeqCst), 1);
        assert!(
            session
                .snapshot()
                .world
                .loaded()
                .unwrap()
                .missing_chunks
                .contains(&target)
        );

        streamer.set_radius(1);
        streamer.service(&mut session).unwrap();
        session.update(Duration::from_millis(50));
        assert!(!streamer.missing.contains(&target));
        assert!(
            !session
                .snapshot()
                .world
                .loaded()
                .unwrap()
                .missing_chunks
                .contains(&target)
        );

        streamer.set_radius(2);
        while !session.has_resident_or_parked_chunk(target) {
            streamer.service(&mut session).unwrap();
            session.update(Duration::from_millis(50));
            assert!(
                Instant::now() < deadline,
                "returned chunk was not rechecked"
            );
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(reads.load(AtomicOrdering::SeqCst), 2);
        assert!(
            !session
                .snapshot()
                .world
                .loaded()
                .unwrap()
                .missing_chunks
                .contains(&target)
        );
    }

    #[test]
    fn settled_prefix_skips_completed_chunks_but_keeps_nearest_first_order() {
        let home = ChunkPos { x: 0, z: 0 };
        let player = PlayerState {
            position: Vec3::new(8.5, 1.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let mut session = LoadedSession::new(
            chunk_world(home),
            IVec3::ZERO,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        let (reads_tx, reads_rx) = mpsc::channel();
        let (permit_tx, permit_rx) = mpsc::channel();
        let mut streamer =
            ChunkStreamer::with_entity_source(6, BTreeSet::new(), move |pos, entities_only| {
                assert!(!entities_only);
                permit_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                reads_tx.send(pos).unwrap();
                Ok(Some(ChunkPayload {
                    world: Some(chunk_world(pos)),
                    entities: SavedEntityData::Empty,
                    poi: SavedPoiData::Missing,
                    heightmaps: None,
                }))
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while session.snapshot().world.loaded().unwrap().chunks.len() < 169 {
            streamer.service(&mut session).unwrap();
            permit_tx.send(()).unwrap();
            session.update(Duration::from_millis(50));
            assert!(
                Instant::now() < deadline,
                "streamer did not fill the window"
            );
            thread::sleep(Duration::from_millis(2));
        }
        let reads: Vec<_> = reads_rx.try_iter().collect();
        let expected: Vec<_> = targets(home, 6)
            .into_iter()
            .filter(|&pos| pos != home)
            .collect();
        assert_eq!(reads, expected, "request order changed");
        eprintln!(
            "skipped {} settled target checks",
            streamer.skipped_target_checks
        );
        assert!(
            streamer.skipped_target_checks > 1_000,
            "settled targets should not be rescanned on every response"
        );
        assert!(streamer.settled_prefix > 100);
    }

    #[test]
    fn delayed_neighbor_is_a_barrier_then_becomes_walkable() {
        let home = ChunkPos { x: 0, z: 0 };
        let neighbor = ChunkPos { x: 1, z: 0 };
        let player = PlayerState {
            position: Vec3::new(15.4, 1.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let mut session = LoadedSession::new(
            chunk_world(home),
            IVec3::ZERO,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let mut streamer = ChunkStreamer::with_source(1, BTreeSet::new(), move |pos| {
            if pos == neighbor {
                started_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                Ok(Some(chunk_world(pos)))
            } else {
                Ok(None)
            }
        })
        .unwrap();
        streamer.service(&mut session).unwrap();
        started_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        session.submit_command(PlayerCommand::Move {
            forward: 0.0,
            strafe: 1.0,
            jump: false,
        });
        for _ in 0..30 {
            streamer.service(&mut session).unwrap();
            session.update(Duration::from_millis(50));
        }
        assert!(session.snapshot().player.position.x <= 15.700_01);
        assert!(
            !session
                .snapshot()
                .world
                .loaded()
                .unwrap()
                .chunks
                .contains_key(&neighbor)
        );
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !session
            .snapshot()
            .world
            .loaded()
            .unwrap()
            .chunks
            .contains_key(&neighbor)
            && Instant::now() < deadline
        {
            streamer.service(&mut session).unwrap();
            session.update(Duration::from_millis(50));
            thread::sleep(Duration::from_millis(2));
        }
        assert!(
            session
                .snapshot()
                .world
                .loaded()
                .unwrap()
                .chunks
                .contains_key(&neighbor)
        );
        for _ in 0..15 {
            streamer.service(&mut session).unwrap();
            session.update(Duration::from_millis(50));
        }
        let snapshot = session.snapshot();
        assert!(snapshot.player.position.x + snapshot.origin.x as f32 > 16.0);
    }

    #[test]
    fn distant_respawn_waits_for_the_background_chunk_reader() {
        use native_persistence::world::{
            DimensionId, PlayerRespawnConfig, RespawnData, SavedWorldBorder,
        };
        use native_resources::{
            packs::{ResourceId, ResourceProvider},
            respawn_environment::RespawnEnvironmentRegistry,
            respawn_tags::RespawnBlockTags,
        };
        use native_session::loaded::SavedRespawnSupport;
        use native_world::loaded::BIOME_COUNT;

        struct Provider;
        impl ResourceProvider for Provider {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                Ok(match path {
                    "data/minecraft/dimension_type/overworld.json" => {
                        Some(br#"{"attributes":{}}"#.to_vec())
                    }
                    "data/minecraft/worldgen/biome/plains.json" => {
                        Some(br#"{"attributes":{}}"#.to_vec())
                    }
                    _ => None,
                })
            }
            fn read_data_stack(&mut self, _: &str) -> Result<Vec<Vec<u8>>> {
                Ok(vec![br#"{"values":[]}"#.to_vec()])
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                Ok(match folder {
                    "worldgen/biome" => vec![ResourceId::parse("minecraft:plains")?],
                    "timeline" => vec![],
                    _ => unreachable!(),
                })
            }
        }
        for (immediate, edge) in [(false, false), (true, false), (false, true), (true, true)] {
            let home = ChunkPos { x: 0, z: 0 };
            let remote = ChunkPos {
                x: 1_000_000,
                z: -1_000_000,
            };
            let local = if edge { 15 } else { 8 };
            let spawn = IVec3::new(remote.x * 16 + local, 1, remote.z * 16 + local);
            let mut distant = chunk_world(remote);
            let bed = distant.intern_state(BlockState {
                name: "minecraft:red_bed".into(),
                properties: BTreeMap::from([
                    ("facing".into(), "north".into()),
                    ("part".into(), "head".into()),
                    ("occupied".into(), "false".into()),
                ]),
            });
            distant
                .set_block_id(spawn.x, spawn.y, spawn.z, bed)
                .unwrap();
            let biome = distant.intern_biome("minecraft:plains".into());
            let chunk = distant.chunks.get_mut(&remote).unwrap();
            Arc::make_mut(Arc::make_mut(chunk).sections.get_mut(&0).unwrap()).biomes =
                Some(Box::new([biome; BIOME_COUNT]));
            let player = PlayerState {
                position: Vec3::new(8.5, 20.0, 8.5),
                yaw: 0.0,
                pitch: 0.0,
                vertical_velocity: 0.0,
                on_ground: false,
                pose: native_session::PlayerPose::Standing,
            };
            let mut session = LoadedSession::new(
                chunk_world(home),
                IVec3::ZERO,
                BuildBounds {
                    min_y: 0,
                    max_y: 32,
                },
                player,
            )
            .unwrap();
            let ids = [
                "air",
                "stone",
                "grass_block",
                "dirt",
                "stone_bricks",
                "red_bed",
            ]
            .into_iter()
            .map(|id| format!("minecraft:{id}"))
            .collect();
            session
                .set_saved_respawn_support(SavedRespawnSupport {
                    config: PlayerRespawnConfig {
                        data: RespawnData {
                            dimension: DimensionId::parse("overworld").unwrap(),
                            position: spawn.to_array(),
                            yaw: 0.0,
                            pitch: 0.0,
                        },
                        forced: false,
                    },
                    environment: Arc::new(
                        RespawnEnvironmentRegistry::load(&mut Provider, "minecraft:overworld")
                            .unwrap(),
                    ),
                    tags: Arc::new(RespawnBlockTags::load(&mut Provider, ids).unwrap()),
                    border: SavedWorldBorder::default(),
                    max_health: 20.0,
                })
                .unwrap();
            session.set_player_vitals(native_session::survival::PlayerVitals {
                health: 1.0,
                ..Default::default()
            });
            session.set_death_rules(native_session::DeathRules {
                immediate_respawn: immediate,
                ..Default::default()
            });
            let (started_tx, started_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let mut streamer = ChunkStreamer::with_source(1, BTreeSet::new(), move |pos| {
                if pos == remote {
                    started_tx.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    Ok(Some(distant.clone()))
                } else if within(pos, (remote, 1)) {
                    Ok(Some(chunk_world(pos)))
                } else {
                    Ok(None)
                }
            })
            .unwrap();
            for _ in 0..100 {
                streamer.service(&mut session).unwrap();
                session.update(Duration::from_millis(50));
                if session.snapshot().death.is_some() {
                    break;
                }
            }
            assert!(
                session.snapshot().death.is_some(),
                "player must die from the fall"
            );
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                streamer.service(&mut session).unwrap();
                match started_rx.try_recv() {
                    Ok(()) => break,
                    Err(mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    error => panic!("respawn chunk reader did not start: {error:?}"),
                }
            }
            assert!(!session.snapshot().death.unwrap().can_respawn);
            release_tx.send(()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while session
                .snapshot()
                .death
                .is_some_and(|death| !death.can_respawn)
                && Instant::now() < deadline
            {
                streamer.service(&mut session).unwrap();
                session.update(Duration::from_millis(50));
                thread::sleep(Duration::from_millis(2));
            }
            let before = session.snapshot();
            if !immediate {
                assert!(
                    before.death.unwrap().can_respawn,
                    "immediate={immediate} edge={edge}"
                );
                assert!(!before.world.loaded().unwrap().chunks.contains_key(&remote));
                session.submit_command(PlayerCommand::Respawn);
                session.update(Duration::from_millis(50));
            }
            let after = session.snapshot();
            assert!(after.death.is_none(), "immediate={immediate} edge={edge}");
            let absolute = after.player.position.as_dvec3() + after.origin.as_dvec3();
            assert!((absolute.x - f64::from(spawn.x)).abs() < 4.0);
            assert!((absolute.z - f64::from(spawn.z)).abs() < 4.0);
            assert!(after.world.loaded().unwrap().chunks.contains_key(&remote));
        }
    }

    #[test]
    fn distant_world_spawn_waits_for_streamed_heightmaps_and_entities() {
        use native_persistence::{
            nbt::{NamedTag, Tag},
            world::{DimensionId, LoadedChunk, RespawnData, SavedWorldBorder},
        };
        use native_resources::packs::ResourceProvider;
        use native_session::loaded::WorldRespawnSupport;

        struct Provider;
        impl ResourceProvider for Provider {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data_stack(&mut self, _: &str) -> Result<Vec<Vec<u8>>> {
                Ok(vec![br#"{"values":["minecraft:stone","minecraft:dirt","minecraft:grass_block","minecraft:stone_bricks"]}"#.to_vec()])
            }
        }
        for (immediate, edge, moving, outside) in [
            (false, false, false, false),
            (true, false, false, false),
            (false, true, false, false),
            (true, true, false, false),
            (false, false, true, false),
            (true, false, true, false),
            (false, true, true, false),
            (true, true, true, false),
            (false, false, true, true),
            (true, false, true, true),
        ] {
            let home = ChunkPos { x: 0, z: 0 };
            let remote = ChunkPos {
                x: 1_000_000,
                z: -1_000_000,
            };
            let local = if edge { 15 } else { 8 };
            let spawn = IVec3::new(
                remote.x * 16 + if outside { 31 } else { local },
                if moving { 31 } else { 5 },
                remote.z * 16 + local,
            );
            let player = PlayerState {
                position: Vec3::new(8.5, 20.0, 8.5),
                yaw: 0.0,
                pitch: 0.0,
                vertical_velocity: 0.0,
                on_ground: false,
                pose: native_session::PlayerPose::Standing,
            };
            let mut session = LoadedSession::new(
                chunk_world(home),
                IVec3::ZERO,
                BuildBounds {
                    min_y: 0,
                    max_y: 32,
                },
                player,
            )
            .unwrap();
            let ids = ["air", "stone", "dirt", "grass_block", "stone_bricks"]
                .into_iter()
                .map(|name| format!("minecraft:{name}"))
                .collect();
            let tags = Arc::new(HeightmapTags::load(&mut Provider, &ids).unwrap());
            session.set_spawn_heightmap_tags(Arc::clone(&tags)).unwrap();
            session.set_saved_respawn_absent().unwrap();
            session
                .set_world_respawn_support(WorldRespawnSupport {
                    spawn: RespawnData {
                        dimension: DimensionId::parse("overworld").unwrap(),
                        position: spawn.to_array(),
                        yaw: 25.0,
                        pitch: 10.0,
                    },
                    border: if outside {
                        SavedWorldBorder {
                            center_x: f64::from(remote.x * 16 + 8),
                            center_z: f64::from(remote.z * 16 + 8),
                            size: 16.0,
                            lerp_target: 8.0,
                            lerp_time: 100_000,
                            ..Default::default()
                        }
                    } else if moving {
                        SavedWorldBorder {
                            lerp_target: 59_999_000.0,
                            lerp_time: 100_000,
                            ..Default::default()
                        }
                    } else {
                        SavedWorldBorder::default()
                    },
                    max_health: 20.0,
                    radius: 0,
                    adventure: moving,
                    ceiling_spawn_height: None,
                })
                .unwrap();
            session.set_player_vitals(native_session::survival::PlayerVitals {
                health: 1.0,
                ..Default::default()
            });
            session.set_death_rules(native_session::DeathRules {
                immediate_respawn: immediate,
                ..Default::default()
            });
            let (started_tx, started_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let mut streamer =
                ChunkStreamer::with_entity_source(1, BTreeSet::new(), move |pos, _| {
                    if pos == remote {
                        started_tx.send(()).unwrap();
                        release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    }
                    if !within(pos, (remote, 1)) {
                        return Ok(None);
                    }
                    let world = chunk_world(pos);
                    let saved = LoadedChunk {
                        position: SavedChunkPos { x: pos.x, z: pos.z },
                        data_version: 5023,
                        status: Some("minecraft:full".into()),
                        document: Arc::new(NamedTag {
                            name: "".into(),
                            value: Tag::Compound(vec![]),
                        }),
                        sections: Vec::new(),
                    };
                    let maps = native_session::spawn_heightmaps::prepare(
                        &saved,
                        &world,
                        HeightBounds {
                            min_y: 0,
                            max_y: 32,
                        },
                        &tags,
                    )?;
                    Ok(Some(ChunkPayload {
                        world: Some(world),
                        entities: SavedEntityData::Empty,
                        poi: SavedPoiData::Missing,
                        heightmaps: Some(maps),
                    }))
                })
                .unwrap();
            for _ in 0..100 {
                streamer.service(&mut session).unwrap();
                session.update(Duration::from_millis(50));
                if session.snapshot().death.is_some() {
                    break;
                }
            }
            assert!(session.snapshot().death.is_some());
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                streamer.service(&mut session).unwrap();
                match started_rx.try_recv() {
                    Ok(()) => break,
                    Err(mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(2))
                    }
                    error => panic!("world spawn reader did not start: {error:?}"),
                }
            }
            assert!(!session.snapshot().death.unwrap().can_respawn);
            for _ in 0..12 {
                streamer.service(&mut session).unwrap();
                session.update(Duration::from_millis(50));
            }
            let waiting = session.snapshot().death.unwrap();
            assert!(!waiting.can_respawn);
            assert_eq!(waiting.respawn_pending, immediate);
            release_tx.send(()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while session
                .snapshot()
                .death
                .is_some_and(|death| !death.can_respawn)
                && Instant::now() < deadline
            {
                streamer.service(&mut session).unwrap();
                session.update(Duration::from_millis(50));
                thread::sleep(Duration::from_millis(2));
            }
            if !immediate {
                assert!(session.snapshot().death.unwrap().can_respawn, "edge={edge}");
                assert!(
                    !session
                        .snapshot()
                        .world
                        .loaded()
                        .unwrap()
                        .chunks
                        .contains_key(&remote)
                );
                session.submit_command(PlayerCommand::Respawn);
                session.update(Duration::from_millis(50));
                let deadline = Instant::now() + Duration::from_secs(5);
                while session.snapshot().death.is_some() && Instant::now() < deadline {
                    streamer.service(&mut session).unwrap();
                    session.update(Duration::from_millis(50));
                    thread::sleep(Duration::from_millis(2));
                }
            }
            let after = session.snapshot();
            assert!(
                after.death.is_none(),
                "immediate={immediate}, edge={edge}, moving={moving}, outside={outside}"
            );
            let absolute = after.player.position.as_dvec3() + after.origin.as_dvec3();
            assert_eq!(
                absolute,
                glam::DVec3::new(
                    f64::from(if outside { remote.x * 16 + 8 } else { spawn.x }) + 0.5,
                    1.0,
                    f64::from(spawn.z) + 0.5,
                )
            );
            assert!(after.world.loaded().unwrap().chunks.contains_key(&remote));
        }
    }

    #[test]
    fn targets_are_nearest_first_and_coordinate_overflow_is_excluded() {
        let center = ChunkPos { x: -1, z: 1 };
        let positions = targets(center, 1);
        assert_eq!(positions.len(), 9);
        assert_eq!(positions[0], center);
        assert!(positions.iter().all(|pos| within(*pos, (center, 1))));
        assert!(targets(ChunkPos { x: i32::MAX, z: 0 }, 1).is_empty());
    }

    #[test]
    fn reads_neighbor_from_anvil_without_modifying_world_files() {
        use flate2::{
            Compression,
            write::{GzEncoder, ZlibEncoder},
        };
        use native_persistence::{
            nbt::{self, NamedTag, Tag, TagType},
            world::DimensionId,
        };
        use std::io::Write;

        struct HeightTagsProvider;
        impl native_resources::packs::ResourceProvider for HeightTagsProvider {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data_stack(&mut self, _: &str) -> Result<Vec<Vec<u8>>> {
                Ok(vec![br#"{"values":["minecraft:stone","minecraft:dirt","minecraft:grass_block","minecraft:stone_bricks"]}"#.to_vec()])
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let level = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "Data".into(),
                Tag::Compound(vec![("DataVersion".into(), Tag::Int(5023))]),
            )]),
        };
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&nbt::write(&level).unwrap()).unwrap();
        let level_bytes = gzip.finish().unwrap();
        std::fs::write(temp.path().join("level.dat"), &level_bytes).unwrap();
        let mut region = vec![0u8; 4 * 4096];
        for x in 0..=1 {
            let (palette, floor_index, air_index) = if x == 0 {
                (vec!["minecraft:air", "minecraft:stone"], 1u64, 0u64)
            } else {
                (vec!["minecraft:stone", "minecraft:air"], 0u64, 1u64)
            };
            let words: Vec<i64> = (0..256)
                .map(|word| {
                    let value = if word < 16 { floor_index } else { air_index };
                    (0..16).fold(0u64, |bits, offset| bits | (value << (offset * 4))) as i64
                })
                .collect();
            let section = Tag::Compound(vec![
                ("Y".into(), Tag::Byte(0)),
                (
                    "block_states".into(),
                    Tag::Compound(vec![
                        (
                            "palette".into(),
                            Tag::List {
                                element_type: TagType::String,
                                values: palette.iter().map(|s| Tag::String((*s).into())).collect(),
                            },
                        ),
                        ("data".into(), Tag::LongArray(words)),
                    ]),
                ),
            ]);
            let chunk = NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![
                    ("DataVersion".into(), Tag::Int(5023)),
                    ("xPos".into(), Tag::Int(x)),
                    ("zPos".into(), Tag::Int(0)),
                    ("Status".into(), Tag::String("minecraft:full".into())),
                    (
                        "sections".into(),
                        Tag::List {
                            element_type: TagType::Compound,
                            values: vec![section],
                        },
                    ),
                ]),
            };
            let mut zlib = ZlibEncoder::new(Vec::new(), Compression::default());
            zlib.write_all(&nbt::write(&chunk).unwrap()).unwrap();
            let compressed = zlib.finish().unwrap();
            assert!(compressed.len() + 5 <= 4096);
            let sector = x as usize + 2;
            region[x as usize * 4..x as usize * 4 + 4]
                .copy_from_slice(&(((sector as u32) << 8) | 1).to_be_bytes());
            let base = sector * 4096;
            region[base..base + 4].copy_from_slice(&((compressed.len() + 1) as u32).to_be_bytes());
            region[base + 4] = 2;
            region[base + 5..base + 5 + compressed.len()].copy_from_slice(&compressed);
        }
        let region_dir = temp.path().join("dimensions/minecraft/overworld/region");
        std::fs::create_dir_all(&region_dir).unwrap();
        let region_path = region_dir.join("r.0.0.mca");
        std::fs::write(&region_path, &region).unwrap();
        let entity_document = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("Position".into(), Tag::IntArray(vec![1, 0])),
                (
                    "Entities".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![Tag::Compound(vec![
                            ("id".into(), Tag::String("minecraft:pig".into())),
                            ("unknown".into(), Tag::Int(42)),
                        ])],
                    },
                ),
            ]),
        };
        let mut zlib = ZlibEncoder::new(Vec::new(), Compression::default());
        zlib.write_all(&nbt::write(&entity_document).unwrap())
            .unwrap();
        let compressed = zlib.finish().unwrap();
        assert!(compressed.len() + 5 <= 4096);
        let mut entity_region = vec![0u8; 3 * 4096];
        entity_region[4..8].copy_from_slice(&((2u32 << 8) | 1).to_be_bytes());
        entity_region[8192..8196].copy_from_slice(&((compressed.len() + 1) as u32).to_be_bytes());
        entity_region[8196] = 2;
        entity_region[8197..8197 + compressed.len()].copy_from_slice(&compressed);
        let entity_dir = temp.path().join("dimensions/minecraft/overworld/entities");
        std::fs::create_dir_all(&entity_dir).unwrap();
        let entity_path = entity_dir.join("r.0.0.mca");
        std::fs::write(&entity_path, &entity_region).unwrap();
        let poi_document = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                (
                    "Sections".into(),
                    Tag::Compound(vec![(
                        "0".into(),
                        Tag::Compound(vec![
                            ("Valid".into(), Tag::Byte(1)),
                            (
                                "Records".into(),
                                Tag::List {
                                    element_type: TagType::Compound,
                                    values: vec![Tag::Compound(vec![
                                        ("pos".into(), Tag::IntArray(vec![16, 1, 0])),
                                        ("type".into(), Tag::String("minecraft:home".into())),
                                        ("free_tickets".into(), Tag::Int(0)),
                                    ])],
                                },
                            ),
                        ]),
                    )]),
                ),
            ]),
        };
        let mut zlib = ZlibEncoder::new(Vec::new(), Compression::default());
        zlib.write_all(&nbt::write(&poi_document).unwrap()).unwrap();
        let compressed_poi = zlib.finish().unwrap();
        let mut poi_region = vec![0u8; 3 * 4096];
        poi_region[4..8].copy_from_slice(&((2u32 << 8) | 1).to_be_bytes());
        poi_region[8192..8196].copy_from_slice(&((compressed_poi.len() + 1) as u32).to_be_bytes());
        poi_region[8196] = 2;
        poi_region[8197..8197 + compressed_poi.len()].copy_from_slice(&compressed_poi);
        let poi_dir = temp.path().join("dimensions/minecraft/overworld/poi");
        std::fs::create_dir_all(&poi_dir).unwrap();
        let poi_path = poi_dir.join("r.0.0.mca");
        std::fs::write(&poi_path, &poi_region).unwrap();
        let dimension = DimensionId::parse("minecraft:overworld").unwrap();
        let mut reader = WorldReader::open(temp.path(), dimension).unwrap();
        let area = reader.read_area(SavedChunkPos { x: 0, z: 0 }, 0).unwrap();
        let world = area.materialize(reader.dimension()).unwrap();
        let player = PlayerState {
            position: Vec3::new(15.4, 1.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let mut session = LoadedSession::new(
            world,
            IVec3::ZERO,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        let registry = ["air", "stone", "dirt", "grass_block", "stone_bricks"]
            .into_iter()
            .map(|name| format!("minecraft:{name}"))
            .collect();
        let tags = Arc::new(HeightmapTags::load(&mut HeightTagsProvider, &registry).unwrap());
        session.set_spawn_heightmap_tags(Arc::clone(&tags)).unwrap();
        session
            .prepare_initial_spawn_heightmaps(&area.chunks[&SavedChunkPos { x: 0, z: 0 }])
            .unwrap();
        assert_eq!(session.spawn_column_heights(0, 0).unwrap().world_surface, 0);
        assert!(session.spawn_column_heights(16, 0).is_err());
        let mut streamer = ChunkStreamer::start(
            reader,
            BTreeMap::new(),
            0,
            BTreeSet::new(),
            Some((
                HeightBounds {
                    min_y: 0,
                    max_y: 32,
                },
                tags,
            )),
        )
        .unwrap();
        session.submit_command(PlayerCommand::Move {
            forward: 0.0,
            strafe: 1.0,
            jump: false,
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while {
            let snapshot = session.snapshot();
            snapshot.player.position.x + snapshot.origin.x as f32 <= 16.0
                && Instant::now() < deadline
        } {
            streamer.service(&mut session).unwrap();
            session.update(Duration::from_millis(50));
            thread::sleep(Duration::from_millis(2));
        }
        let snapshot = session.snapshot();
        assert!(snapshot.player.position.x + snapshot.origin.x as f32 > 16.0);
        assert!(
            snapshot
                .world
                .loaded()
                .unwrap()
                .chunks
                .contains_key(&ChunkPos { x: 1, z: 0 })
        );
        let Some(SavedEntityData::Present(entities)) =
            session.saved_entity_chunk(ChunkPos { x: 1, z: 0 })
        else {
            panic!("streamed entity data was not preserved");
        };
        assert_eq!(*entities.document, entity_document);
        let Some(SavedPoiData::Present(poi)) = session.saved_poi_chunk(ChunkPos { x: 1, z: 0 })
        else {
            panic!("streamed POI data was not preserved")
        };
        assert_eq!(*poi.document, poi_document);
        assert_eq!(
            session.spawn_column_heights(16, 0).unwrap(),
            native_session::world_spawn::SpawnColumnHeights {
                motion_blocking: 0,
                world_surface: 0,
                ocean_floor: 0,
            }
        );
        assert_eq!(
            std::fs::read(temp.path().join("level.dat")).unwrap(),
            level_bytes
        );
        assert_eq!(std::fs::read(region_path).unwrap(), region);
        assert_eq!(std::fs::read(entity_path).unwrap(), entity_region);
        assert_eq!(std::fs::read(poi_path).unwrap(), poi_region);

        // A missing region really is empty, but a truncated region must not
        // become a false negative for future entity collision checks.
        let bad_path = entity_dir.join("r.1.0.mca");
        std::fs::write(&bad_path, [1, 2, 3]).unwrap();
        let mut reader = WorldReader::open(
            temp.path(),
            DimensionId::parse("minecraft:overworld").unwrap(),
        )
        .unwrap();
        assert!(matches!(
            read_entity_data(&mut reader, ChunkPos { x: 64, z: 0 }),
            SavedEntityData::Empty
        ));
        assert!(matches!(
            read_entity_data(&mut reader, ChunkPos { x: 32, z: 0 }),
            SavedEntityData::Unavailable
        ));
        assert!(reader.read_chunk(1, 0).unwrap().is_some());
        assert_eq!(std::fs::read(bad_path).unwrap(), [1, 2, 3]);
        let bad_poi = poi_dir.join("r.1.0.mca");
        std::fs::write(&bad_poi, [1, 2, 3]).unwrap();
        assert!(matches!(
            read_poi_data(&mut reader, ChunkPos { x: 64, z: 0 }),
            SavedPoiData::Missing
        ));
        assert!(matches!(
            read_poi_data(&mut reader, ChunkPos { x: 32, z: 0 }),
            SavedPoiData::Unavailable
        ));
        assert_eq!(std::fs::read(bad_poi).unwrap(), [1, 2, 3]);
    }
}
