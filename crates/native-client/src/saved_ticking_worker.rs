//! Prepare a saved chunk's ticking callback away from the event loop.
//!
//! The caller must supply the actual queued holder callback or its private
//! fork, then retain that holder's position in the tick executor. This worker
//! does not infer holder readiness, publish chunks, or authorize a world save.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
};

use anyhow::{Context, Result, ensure};
use native_persistence::{
    save::{WorldReadStamp, WorldSessionGuard},
    world::{ChunkArea, ChunkPos as SavedChunkPos, DimensionId, WorldReader},
};
use native_resources::{
    datapack_snapshot::DataPackSnapshot, datapacks::DataPackSettings, packs::ResourceStack,
};
use native_session::{
    loaded::{
        BuildBounds, GenerationTerrainLiveDraft, GenerationTerrainStage, LoadedSession,
        SavedTickQueueCapture,
    },
    ticking_preparation::{TickingPreparation, ticking_dependencies},
    worldgen::{
        GeneratedLavaRules,
        saved_postprocessing::{SavedPostprocessingInputs, SavedTickingBatch, SavedTickingResult},
    },
};

/// Frozen settings from the same live session and data-pack snapshot.
pub(crate) struct SavedTickingRequest {
    pub minecraft_dir: PathBuf,
    pub world_dir: PathBuf,
    pub world_session: WorldSessionGuard,
    pub dimension: DimensionId,
    pub bounds: BuildBounds,
    pub packs: Arc<DataPackSnapshot>,
    pub selection: DataPackSettings,
    pub water_source_conversion: bool,
    pub lava_rules: GeneratedLavaRules,
}

struct Work {
    request: SavedTickingRequest,
    callback: TickingPreparation,
    terrain: GenerationTerrainLiveDraft,
    ticks: SavedTickQueueCapture,
    stamp: WorldReadStamp,
}

/// A completed private pass. Supported block changes and schedules can be
/// reconciled at the captured tick; lighting, entities and saving still need
/// their own completion checks.
pub(crate) struct PreparedSavedTicking {
    terrain: GenerationTerrainStage,
    result: SavedTickingResult,
}

/// An unsuccessful live check retains the private result for a later retry or
/// for reconciling effects that the no-effect path does not support yet.
pub(crate) struct SavedTickingApplyError {
    pub prepared: PreparedSavedTicking,
    pub error: anyhow::Error,
}

impl PreparedSavedTicking {
    pub fn result(&self) -> &SavedTickingResult {
        &self.result
    }

    /// The caller must invoke this at the original callback's executor position.
    /// Save coordination and the remaining chunk-holder lifecycle are separate.
    pub fn apply_no_effect(
        self,
        session: &mut LoadedSession,
        callback: &mut TickingPreparation,
    ) -> std::result::Result<(), SavedTickingApplyError> {
        match session.apply_no_effect_saved_ticking(callback, &self.terrain, &self.result) {
            Ok(()) => {
                crate::generation_staging::discard_async(self);
                Ok(())
            }
            Err(error) => Err(SavedTickingApplyError {
                prepared: self,
                error,
            }),
        }
    }

    /// Apply supported block and schedule effects at the captured tick and
    /// callback position. Save coordination and holder lifecycle are separate.
    pub fn apply_scheduled(
        self,
        session: &mut LoadedSession,
        callback: &mut TickingPreparation,
    ) -> std::result::Result<(), SavedTickingApplyError> {
        match session.apply_scheduled_saved_ticking(callback, &self.terrain, &self.result) {
            Ok(()) => {
                crate::generation_staging::discard_async(self);
                Ok(())
            }
            Err(error) => Err(SavedTickingApplyError {
                prepared: self,
                error,
            }),
        }
    }
}

pub(crate) struct SavedTickingWorker {
    cancelled: Arc<AtomicBool>,
    result: Receiver<Result<PreparedSavedTicking>>,
    finished: bool,
}

impl SavedTickingWorker {
    /// Capture cheap shared session state now and validate it on the worker.
    /// The caller can fork the executor head first, start the worker, then
    /// reserve that same callback; a failed spawn leaves the queue untouched.
    /// FULL neighbors and registration still come from the real holder.
    pub fn start(
        original: &TickingPreparation,
        session: &LoadedSession,
        request: SavedTickingRequest,
    ) -> Result<Self> {
        let center = original.center();
        let callback = original.fork_queued_callback()?;
        let stamp = request.world_session.read_stamp()?;
        let terrain = session.capture_saved_ticking_terrain_draft(
            &stamp,
            &request.dimension,
            request.bounds,
            ticking_dependencies(center),
        )?;
        let ticks = session.capture_saved_tick_queues()?;
        let work = Work {
            request,
            callback,
            terrain,
            ticks,
            stamp,
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let (sender, result) = mpsc::channel();
        crate::generation_staging::spawn_with_payload(
            work,
            move |work| {
                let prepared = prepare(work, &worker_cancelled);
                let _ = sender.send(prepared);
            },
            |task| {
                thread::Builder::new()
                    .name("saved-ticking-preparation".into())
                    .spawn(task)
                    .map(|_| ())
            },
        )
        .map_err(|(work, error)| {
            crate::generation_staging::discard_async(work);
            anyhow::Error::new(error).context("cannot start saved ticking preparation")
        })?;
        Ok(Self {
            cancelled,
            result,
            finished: false,
        })
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Return at most one result without waiting for disk or worker activity.
    pub fn try_result(&mut self) -> Result<Option<PreparedSavedTicking>> {
        if self.finished {
            return Ok(None);
        }
        let result = match self.result.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return Ok(None),
            Err(TryRecvError::Disconnected) => {
                self.finished = true;
                anyhow::bail!("saved ticking preparation stopped unexpectedly");
            }
        };
        self.finished = true;
        if self.cancelled.load(Ordering::Acquire) {
            if let Ok(prepared) = result {
                crate::generation_staging::discard_async(prepared);
            }
            anyhow::bail!("saved ticking preparation was cancelled");
        }
        result.map(Some)
    }
}

impl Drop for SavedTickingWorker {
    fn drop(&mut self) {
        self.cancel();
        crate::generation_staging::discard_receiver(&mut self.result);
    }
}

fn prepare(mut work: Work, cancelled: &AtomicBool) -> Result<PreparedSavedTicking> {
    let check = || {
        ensure!(
            !cancelled.load(Ordering::Acquire),
            "saved ticking preparation was cancelled"
        );
        ensure!(
            work.stamp.is_current(),
            "saved ticking world changed during preparation"
        );
        Ok(())
    };
    check()?;
    let mut provider = ResourceStack::open(&work.request.minecraft_dir)?;
    let warnings = provider
        .configure_snapshot_data(Arc::clone(&work.request.packs), &work.request.selection)?;
    ensure!(
        warnings.is_empty(),
        "saved ticking data packs could not be restored: {}",
        warnings.join("; ")
    );
    let ids = provider.vanilla_block_ids()?;
    let defaults = provider
        .vanilla_generated_block_registry()?
        .default_states();
    check()?;
    let mut reader = WorldReader::open_with_session(
        &work.request.world_dir,
        work.request.dimension.clone(),
        work.request.world_session.clone(),
    )?;
    check()?;
    let area = read_saved_full_area(&mut reader, work.callback.center(), check)?;
    check()?;
    let saved_terrain =
        Arc::new(area.materialize_with_defaults(&work.request.dimension, &defaults)?);
    let terrain = work
        .terrain
        .bind_saved_source(saved_terrain)?
        .validate_checked(check)?;
    check()?;
    let mut batch = SavedTickingBatch::begin_with_bound_live_session(
        &mut provider,
        SavedPostprocessingInputs {
            area: &area,
            dimension: &work.request.dimension,
            defaults: &defaults,
            block_ids: &ids,
            bounds: work.request.bounds,
            registration: work.ticks.registration_order(),
            game_time: work.ticks.game_time(),
            next_sub_tick: work.ticks.next_sub_tick(),
            light: None,
        },
        &terrain,
        &work.ticks,
        |_| Ok(()),
        work.request.water_source_conversion,
        work.request.lava_rules,
    )?;
    check()?;
    ensure!(
        batch.dispatch_checked(&mut work.callback, work.ticks.game_time(), check)?,
        "saved ticking callback was not queued"
    );
    let result = batch.finish()?;
    check()?;
    Ok(PreparedSavedTicking { terrain, result })
}

/// Keep cancellation and epoch checks between individual chunk reads so a
/// cancelled or superseded request does not need to decode the entire area.
pub(super) fn read_saved_full_area(
    reader: &mut WorldReader,
    center: native_world::loaded::ChunkPos,
    check: impl Fn() -> Result<()>,
) -> Result<ChunkArea> {
    center.x.checked_sub(1).context("chunk X range overflow")?;
    center.x.checked_add(1).context("chunk X range overflow")?;
    center.z.checked_sub(1).context("chunk Z range overflow")?;
    center.z.checked_add(1).context("chunk Z range overflow")?;
    let mut chunks = BTreeMap::new();
    for position in ticking_dependencies(center) {
        check()?;
        let chunk = reader.read_chunk(position.x, position.z)?;
        check()?;
        let chunk = chunk
            .with_context(|| format!("saved ticking FULL neighbor is missing at {position:?}"))?;
        ensure!(
            chunk.is_full(),
            "saved ticking neighbor is not FULL at {position:?}"
        );
        chunks.insert(
            SavedChunkPos {
                x: position.x,
                z: position.z,
            },
            Arc::new(chunk),
        );
    }
    Ok(ChunkArea {
        center: SavedChunkPos {
            x: center.x,
            z: center.z,
        },
        radius: 1,
        chunks,
        missing: Vec::new(),
    })
}
