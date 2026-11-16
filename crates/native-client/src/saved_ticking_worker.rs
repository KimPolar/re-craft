//! Prepare a saved chunk's ticking callback away from the event loop.
//!
//! The caller must supply the actual queued holder callback and keep its
//! position in the tick executor. This worker does not infer holder readiness,
//! publish generated chunks, or authorize a world save.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
};

use anyhow::{Result, ensure};
use native_persistence::world::{ChunkArea, DimensionId};
use native_resources::{
    datapack_snapshot::DataPackSnapshot, datapacks::DataPackSettings, packs::ResourceStack,
};
use native_session::{
    loaded::{
        BuildBounds, GenerationTerrainStage, GenerationTerrainStageDraft, LoadedSession,
        SavedTickQueueCapture,
    },
    ticking_preparation::{TickingPreparation, ticking_dependencies},
    worldgen::{
        GeneratedLavaRules,
        saved_postprocessing::{SavedPostprocessingInputs, SavedTickingBatch, SavedTickingResult},
    },
};
use native_world::loaded::World;

/// Frozen settings from the same live session and data-pack snapshot.
pub(crate) struct SavedTickingRequest {
    pub minecraft_dir: PathBuf,
    pub dimension: DimensionId,
    pub bounds: BuildBounds,
    pub area: ChunkArea,
    pub saved_terrain: Arc<World>,
    pub packs: Arc<DataPackSnapshot>,
    pub selection: DataPackSettings,
    pub water_source_conversion: bool,
    pub lava_rules: GeneratedLavaRules,
}

struct Work {
    request: SavedTickingRequest,
    callback: TickingPreparation,
    terrain: GenerationTerrainStageDraft,
    ticks: SavedTickQueueCapture,
}

/// A completed private pass. Block, light and entity effects still need
/// reconciliation; schedule-only results require the exact captured tick.
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

    /// Apply schedule-only effects at the captured tick and callback position.
    /// Save coordination and the remaining chunk-holder lifecycle are separate.
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
    /// The requested FULL neighbors and registration order still come from the
    /// holder; resident terrain by itself does not establish readiness.
    pub fn start(
        original: &TickingPreparation,
        session: &LoadedSession,
        request: SavedTickingRequest,
    ) -> Result<Self> {
        let center = original.center();
        ensure!(
            request.area.center.x == center.x && request.area.center.z == center.z,
            "saved postprocessing area has the wrong center"
        );
        let callback = original.fork_queued_callback()?;
        let terrain = session.capture_generation_terrain_stage_draft(
            Arc::clone(&request.saved_terrain),
            ticking_dependencies(center),
        )?;
        let ticks = session.capture_saved_tick_queues()?;
        let work = Work {
            request,
            callback,
            terrain,
            ticks,
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
        Ok(())
    };
    check()?;
    let terrain = work.terrain.validate_checked(check)?;
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
    let mut batch = SavedTickingBatch::begin_with_bound_live_session(
        &mut provider,
        SavedPostprocessingInputs {
            area: &work.request.area,
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
