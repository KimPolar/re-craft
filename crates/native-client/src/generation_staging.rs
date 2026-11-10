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

use anyhow::Result;
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
    Cancelling,
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

/// Failure to start staging leaves the generated world available for retry.
pub struct StagingStartError {
    pub generation: GenerationReady,
    pub error: anyhow::Error,
}

/// Keep ownership on the caller until a worker has actually started.
fn spawn_with_payload<T: Send + 'static>(
    payload: T,
    task: impl FnOnce(T) + Send + 'static,
    spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> std::io::Result<()>,
) -> std::result::Result<(), (T, std::io::Error)> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let worker = Box::new(move || {
        if let Ok(payload) = receiver.recv() {
            task(payload);
        }
    });
    if let Err(error) = spawn(worker) {
        return Err((payload, error));
    }
    sender.send(payload).map_err(|error| {
        (
            error.0,
            std::io::Error::other("staging worker stopped before accepting generation"),
        )
    })
}

// A queued result can own a large temporary directory or generated world.
// Closing the receiver off-thread avoids freeing either on the event loop.
fn discard_async<T: Send + 'static>(value: T) {
    let _ = thread::Builder::new()
        .name("world-staging-discard".into())
        .spawn(move || drop(value));
}

pub(crate) fn discard_receiver<T: Send + 'static>(receiver: &mut Receiver<T>) {
    match receiver.try_recv() {
        Ok(value) => discard_async(value),
        Err(TryRecvError::Empty) => {
            let (_, replacement) = mpsc::channel();
            discard_async(std::mem::replace(receiver, replacement));
        }
        Err(TryRecvError::Disconnected) => {}
    }
}

pub struct InitialWorldStagingWorker {
    cancelled: Arc<AtomicBool>,
    progress: Arc<Mutex<StagingProgress>>,
    result: Receiver<StagingResult>,
    original_hut_generation: Option<Arc<crate::fresh_generation::InitialHutGeneration>>,
    cleanup: Mutex<Option<Receiver<StagingResult>>>,
    #[cfg(test)]
    cleanup_observer: Mutex<Option<Box<dyn FnOnce() + Send>>>,
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
        Self::start_recoverable(
            generation,
            parent,
            structures,
            last_played_ms,
            region_timestamp,
        )
        .map_err(|failure| failure.error)
    }

    /// Return the generation result if the worker cannot be started.
    pub fn start_recoverable(
        generation: GenerationReady,
        parent: PathBuf,
        structures: bool,
        last_played_ms: i64,
        region_timestamp: u32,
    ) -> std::result::Result<Self, StagingStartError> {
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
        .map_err(|failure| failure.error)
    }

    fn start_inner(
        generation: GenerationReady,
        parent: PathBuf,
        structures: bool,
        last_played_ms: i64,
        region_timestamp: u32,
        #[cfg(test)] mut observer: Option<
            Box<dyn for<'a> FnMut(InitialWorldStageProgress<'a>) -> Result<()> + Send>,
        >,
    ) -> std::result::Result<Self, StagingStartError> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let original_hut_generation = generation.hut_generation.clone();
        let progress = Arc::new(Mutex::new(StagingProgress {
            phase: StagingPhase::Preparing,
            chunks_started: 0,
            last_chunk: None,
        }));
        let (sender, result) = mpsc::channel();
        let worker_cancelled = Arc::clone(&cancelled);
        let worker_progress = Arc::clone(&progress);
        spawn_with_payload(
            generation,
            move |mut generation| {
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
            },
            |task| {
                thread::Builder::new()
                    .name("world-staging".into())
                    .spawn(task)
                    .map(|_| ())
            },
        )
        .map_err(|(generation, error)| StagingStartError {
            generation,
            error: anyhow::Error::new(error).context("cannot start world staging worker"),
        })?;
        Ok(Self {
            cancelled,
            progress,
            result,
            original_hut_generation,
            cleanup: Mutex::new(None),
            #[cfg(test)]
            cleanup_observer: Mutex::new(None),
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

    #[cfg(test)]
    pub(crate) fn observe_cleanup(&self, observer: impl FnOnce() + Send + 'static) {
        *self.cleanup_observer.lock().unwrap() = Some(Box::new(observer));
    }

    pub fn try_result(&self) -> Result<Option<StagingResult>> {
        let mut cleanup = self
            .cleanup
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(receiver) = cleanup.as_ref() {
            return match receiver.try_recv() {
                Ok(result) => {
                    *cleanup = None;
                    Ok(Some(result))
                }
                Err(TryRecvError::Empty) => Ok(None),
                Err(TryRecvError::Disconnected) => {
                    anyhow::bail!("world staging cleanup stopped unexpectedly")
                }
            };
        }
        match self.result.try_recv() {
            Ok(StagingResult::Ready {
                mut generation,
                world,
            }) if self.cancelled.load(Ordering::Acquire) => {
                // Staging commits the generation context on success. If the
                // UI cancelled before receiving that success, discard the
                // unpublished stage and restore the exact retry input.
                generation.hut_generation = self.original_hut_generation.clone();
                self.progress
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .phase = StagingPhase::Cancelling;
                let (sender, receiver) = mpsc::channel();
                let progress = Arc::clone(&self.progress);
                #[cfg(test)]
                let observer = self.cleanup_observer.lock().unwrap().take();
                let launched = spawn_with_payload(
                    (generation, world),
                    move |(generation, world)| {
                        #[cfg(test)]
                        if let Some(observer) = observer {
                            observer();
                        }
                        drop(world);
                        progress
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .phase = StagingPhase::Cancelled;
                        let _ = sender.send(StagingResult::Cancelled(generation));
                    },
                    |task| {
                        thread::Builder::new()
                            .name("world-staging-cleanup".into())
                            .spawn(task)
                            .map(|_| ())
                    },
                );
                match launched {
                    Ok(()) => {
                        *cleanup = Some(receiver);
                        Ok(None)
                    }
                    Err(((generation, world), _)) => {
                        drop(world);
                        self.progress
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .phase = StagingPhase::Cancelled;
                        Ok(Some(StagingResult::Cancelled(generation)))
                    }
                }
            }
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
        discard_receiver(&mut self.result);
        if let Some(context) = self.original_hut_generation.take() {
            discard_async(context);
        }
        if let Some(receiver) = self
            .cleanup
            .get_mut()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            discard_async(receiver);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discarding_a_queued_result_does_not_wait_for_cleanup() {
        #[derive(Debug)]
        struct SlowDrop {
            entered: mpsc::SyncSender<()>,
            resume: mpsc::Receiver<()>,
        }
        impl Drop for SlowDrop {
            fn drop(&mut self) {
                self.entered.send(()).unwrap();
                self.resume
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .unwrap();
            }
        }
        let (entered, entered_rx) = mpsc::sync_channel(1);
        let (resume, resume_rx) = mpsc::sync_channel(1);
        let (sender, mut receiver) = mpsc::channel();
        sender
            .send(SlowDrop {
                entered,
                resume: resume_rx,
            })
            .unwrap();
        discard_receiver(&mut receiver);
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        resume.send(()).unwrap();
    }

    #[test]
    fn failed_worker_launch_returns_payload_and_success_transfers_it_once() {
        let (payload, error) = spawn_with_payload(
            String::from("generated world"),
            |_| panic!("failed worker must not run"),
            |_| Err(std::io::Error::other("thread unavailable")),
        )
        .unwrap_err();
        assert_eq!(payload, "generated world");
        assert_eq!(error.to_string(), "thread unavailable");

        let (payload, error) = spawn_with_payload(
            payload,
            |_| {},
            |task| {
                drop(task);
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(payload, "generated world");
        assert!(error.to_string().contains("before accepting generation"));

        let (sender, receiver) = mpsc::channel();
        spawn_with_payload(
            payload,
            move |value| sender.send(value).unwrap(),
            |task| thread::Builder::new().spawn(task).map(|_| ()),
        )
        .unwrap();
        assert_eq!(
            receiver
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap(),
            "generated world"
        );
        assert!(receiver.recv().is_err());
    }

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
