//! Run initial-world staging off the window thread, with cooperative cancellation.
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
};

use anyhow::{Context, Result};
use native_persistence::save::{FreshRegionKind, FreshWorldStageProgress};
use native_persistence::world::{ChunkPos, DimensionId};

use crate::generation_worker::{
    GenerationReady, InitialWorldStageProgress, StagedInitialWorld, StagedStructureInitialWorld,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StagingPhase {
    Preparing,
    Validating,
    WritingMetadata,
    WritingChunk,
    WritingExternalChunk,
    VerifyingChunk,
    VerifyingMetadata,
    Sealing,
    InstallingDataPacks,
    Ready,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug)]
pub struct StagingProgress {
    pub phase: StagingPhase,
    /// Number of chunk writes started, including terrain, entities and POIs.
    pub chunks_started: u64,
    pub last_chunk: Option<(DimensionId, FreshRegionKind, ChunkPos)>,
}

pub enum StagedWorld {
    Regular(StagedInitialWorld),
    Structured(StagedStructureInitialWorld),
}

pub enum StagingResult {
    Ready {
        generation: GenerationReady,
        world: StagedWorld,
    },
    Cancelled(GenerationReady),
    Failed {
        generation: GenerationReady,
        error: anyhow::Error,
    },
}

pub struct InitialWorldStagingWorker {
    cancelled: Arc<AtomicBool>,
    progress: Arc<Mutex<StagingProgress>>,
    result: Receiver<StagingResult>,
}

#[derive(Debug)]
struct Cancelled;
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("world staging cancelled")
    }
}
impl std::error::Error for Cancelled {}

fn checkpoint(
    cancelled: &AtomicBool,
    progress: &Mutex<StagingProgress>,
    event: InitialWorldStageProgress<'_>,
) -> Result<()> {
    if cancelled.load(Ordering::Acquire) {
        return Err(Cancelled.into());
    }
    let mut current = progress.lock().unwrap_or_else(|error| error.into_inner());
    let (phase, chunk) = match event {
        InitialWorldStageProgress::Preparing => (StagingPhase::Preparing, None),
        InitialWorldStageProgress::InstallingDataPacks => (StagingPhase::InstallingDataPacks, None),
        InitialWorldStageProgress::Finished => (StagingPhase::Sealing, None),
        InitialWorldStageProgress::Saving(event) => match event {
            FreshWorldStageProgress::Validating => (StagingPhase::Validating, None),
            FreshWorldStageProgress::WritingMetadata => (StagingPhase::WritingMetadata, None),
            FreshWorldStageProgress::WritingChunk {
                dimension,
                kind,
                position,
            } => {
                current.chunks_started = current.chunks_started.saturating_add(1);
                (
                    StagingPhase::WritingChunk,
                    Some((dimension.clone(), kind, position)),
                )
            }
            FreshWorldStageProgress::WritingExternalChunk {
                dimension,
                kind,
                position,
            } => (
                StagingPhase::WritingExternalChunk,
                Some((dimension.clone(), kind, position)),
            ),
            FreshWorldStageProgress::VerifyingChunk {
                dimension,
                kind,
                position,
            } => (
                StagingPhase::VerifyingChunk,
                Some((dimension.clone(), kind, position)),
            ),
            FreshWorldStageProgress::VerifyingMetadata => (StagingPhase::VerifyingMetadata, None),
            FreshWorldStageProgress::Sealing | FreshWorldStageProgress::Finished => {
                (StagingPhase::Sealing, None)
            }
        },
    };
    current.phase = phase;
    if chunk.is_some() {
        current.last_chunk = chunk;
    }
    Ok(())
}

impl InitialWorldStagingWorker {
    /// This creates an inspection stage; it does not publish a playable world.
    pub fn start(
        generation: GenerationReady,
        parent: PathBuf,
        structures: bool,
        last_played_ms: i64,
        region_timestamp: u32,
    ) -> Result<Self> {
        Self::start_inner(
            generation,
            parent,
            structures,
            last_played_ms,
            region_timestamp,
            #[cfg(test)]
            None,
        )
    }

    #[cfg(test)]
    pub(crate) fn start_observed(
        generation: GenerationReady,
        parent: PathBuf,
        structures: bool,
        last_played_ms: i64,
        region_timestamp: u32,
        observer: impl for<'a> FnMut(InitialWorldStageProgress<'a>) -> Result<()> + Send + 'static,
    ) -> Result<Self> {
        Self::start_inner(
            generation,
            parent,
            structures,
            last_played_ms,
            region_timestamp,
            Some(Box::new(observer)),
        )
    }

    fn start_inner(
        mut generation: GenerationReady,
        parent: PathBuf,
        structures: bool,
        last_played_ms: i64,
        region_timestamp: u32,
        #[cfg(test)] mut observer: Option<
            Box<dyn for<'a> FnMut(InitialWorldStageProgress<'a>) -> Result<()> + Send>,
        >,
    ) -> Result<Self> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Mutex::new(StagingProgress {
            phase: StagingPhase::Preparing,
            chunks_started: 0,
            last_chunk: None,
        }));
        let (sender, result) = mpsc::channel();
        let worker_cancelled = Arc::clone(&cancelled);
        let worker_progress = Arc::clone(&progress);
        thread::Builder::new()
            .name("world-staging".into())
            .spawn(move || {
                let mut report = |event: InitialWorldStageProgress<'_>| {
                    checkpoint(&worker_cancelled, &worker_progress, event)?;
                    #[cfg(test)]
                    if let Some(observer) = &mut observer {
                        observer(event)?;
                        if worker_cancelled.load(Ordering::Acquire) {
                            return Err(Cancelled.into());
                        }
                    }
                    Ok(())
                };
                let staged = if structures {
                    generation
                        .stage_structure_initial_world_with_progress(
                            &parent,
                            last_played_ms,
                            region_timestamp,
                            &mut report,
                        )
                        .map(StagedWorld::Structured)
                } else {
                    generation
                        .stage_initial_world_with_progress(
                            &parent,
                            last_played_ms,
                            region_timestamp,
                            &mut report,
                        )
                        .map(StagedWorld::Regular)
                };
                let outcome = match staged {
                    Ok(world) => StagingResult::Ready { generation, world },
                    Err(error) if error.is::<Cancelled>() => StagingResult::Cancelled(generation),
                    Err(error) => StagingResult::Failed { generation, error },
                };
                worker_progress
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .phase = match &outcome {
                    StagingResult::Ready { .. } => StagingPhase::Ready,
                    StagingResult::Cancelled(_) => StagingPhase::Cancelled,
                    StagingResult::Failed { .. } => StagingPhase::Failed,
                };
                let _ = sender.send(outcome);
            })
            .context("cannot start world staging worker")?;
        Ok(Self {
            cancelled,
            progress,
            result,
        })
    }

    /// A request takes effect at the next checkpoint, not during one file operation.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn progress(&self) -> StagingProgress {
        self.progress
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub fn try_result(&self) -> Result<Option<StagingResult>> {
        match self.result.try_recv() {
            Ok(result) => Ok(Some(result)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                anyhow::bail!("world staging worker stopped unexpectedly")
            }
        }
    }
}

impl Drop for InitialWorldStagingWorker {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checkpoints_report_chunks_and_stop_only_for_requested_cancellation() {
        let cancelled = AtomicBool::new(false);
        let progress = Mutex::new(StagingProgress {
            phase: StagingPhase::Preparing,
            chunks_started: 0,
            last_chunk: None,
        });
        let dimension = DimensionId::parse("overworld").unwrap();
        let position = ChunkPos { x: -2, z: 4 };
        checkpoint(
            &cancelled,
            &progress,
            InitialWorldStageProgress::Saving(FreshWorldStageProgress::WritingChunk {
                dimension: &dimension,
                kind: FreshRegionKind::Terrain,
                position,
            }),
        )
        .unwrap();
        let saved = progress.lock().unwrap().clone();
        assert_eq!(saved.phase, StagingPhase::WritingChunk);
        assert_eq!(saved.chunks_started, 1);
        assert_eq!(
            saved.last_chunk,
            Some((dimension, FreshRegionKind::Terrain, position))
        );
        cancelled.store(true, Ordering::Release);
        assert!(
            checkpoint(&cancelled, &progress, InitialWorldStageProgress::Finished)
                .unwrap_err()
                .is::<Cancelled>()
        );
        assert_eq!(progress.lock().unwrap().phase, StagingPhase::WritingChunk);
    }
}
