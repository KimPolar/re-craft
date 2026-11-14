//! Background disk capture for a frozen, partial session save.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, TryRecvError},
};

use anyhow::{Context, Result};
use native_persistence::world::{SessionSaveSources, WorldReader};

use crate::loaded::FrozenPartialSave;

/// Starting a worker can fail without consuming the frozen session.
pub struct SaveCaptureStartError {
    frozen: Box<FrozenPartialSave>,
    error: anyhow::Error,
}

impl SaveCaptureStartError {
    pub fn into_parts(self) -> (FrozenPartialSave, anyhow::Error) {
        (*self.frozen, self.error)
    }
}

impl std::fmt::Debug for SaveCaptureStartError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SaveCaptureStartError")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// The session stays on the caller's thread while the worker reads disk. The
/// result is still partial and does not authorize writing a playable world.
#[must_use]
pub struct PartialSaveCaptureWorker {
    frozen: Option<FrozenPartialSave>,
    result: Receiver<Result<SessionSaveSources>>,
    cancelled: Arc<AtomicBool>,
}

/// Completion always returns the frozen session, including on read failure or
/// worker panic. A pending worker remains owned by the caller.
#[must_use]
pub enum PartialSaveCapturePoll {
    Pending(PartialSaveCaptureWorker),
    Ready {
        frozen: FrozenPartialSave,
        result: Result<SessionSaveSources>,
    },
}

impl PartialSaveCaptureWorker {
    pub fn start(
        frozen: FrozenPartialSave,
        mut reader: WorldReader,
    ) -> std::result::Result<Self, SaveCaptureStartError> {
        let validate = || -> Result<()> {
            let session = reader
                .session_guard()
                .context("save capture requires a locked world reader")?;
            let stamp = session.read_stamp()?;
            anyhow::ensure!(
                frozen.belongs_to_world(&stamp),
                "save reader belongs to another world session"
            );
            anyhow::ensure!(
                reader.dimension()
                    == &native_persistence::world::DimensionId::parse(frozen.dimension())?,
                "save reader belongs to another dimension"
            );
            Ok(())
        };
        if let Err(error) = validate() {
            return Err(SaveCaptureStartError {
                frozen: Box::new(frozen),
                error,
            });
        }
        let positions = frozen.source_positions().clone();
        Self::spawn(frozen, move || {
            reader.capture_session_save_sources(positions)
        })
    }

    fn spawn(
        frozen: FrozenPartialSave,
        work: impl FnOnce() -> Result<SessionSaveSources> + Send + 'static,
    ) -> std::result::Result<Self, SaveCaptureStartError> {
        let (send, result) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let thread = std::thread::Builder::new()
            .name("minecraft-save-capture".into())
            .spawn(move || {
                if worker_cancelled.load(Ordering::Acquire) {
                    return;
                }
                let capture = work();
                if !worker_cancelled.load(Ordering::Acquire) {
                    let _ = send.send(capture);
                }
            });
        if let Err(error) = thread {
            return Err(SaveCaptureStartError {
                frozen: Box::new(frozen),
                error: error.into(),
            });
        }
        Ok(Self {
            frozen: Some(frozen),
            result,
            cancelled,
        })
    }

    /// Poll without blocking the window thread. The ready result always
    /// carries the frozen session alongside either the capture or its error.
    pub fn poll(self) -> PartialSaveCapturePoll {
        let result = match self.result.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(anyhow::anyhow!(
                "save capture worker stopped unexpectedly"
            ))),
        };
        match result {
            None => PartialSaveCapturePoll::Pending(self),
            Some(result) => PartialSaveCapturePoll::Ready {
                frozen: self.into_frozen(),
                result,
            },
        }
    }

    /// Recover the session after success, failure, or cancellation. A read
    /// already in progress may finish in the background; it cannot write.
    pub fn into_frozen(mut self) -> FrozenPartialSave {
        self.cancelled.store(true, Ordering::Release);
        self.frozen
            .take()
            .expect("worker retains its frozen session")
    }

    #[cfg(test)]
    pub(crate) fn start_with_test_work(
        frozen: FrozenPartialSave,
        work: impl FnOnce() -> Result<SessionSaveSources> + Send + 'static,
    ) -> std::result::Result<Self, SaveCaptureStartError> {
        Self::spawn(frozen, work)
    }
}

impl Drop for PartialSaveCaptureWorker {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}
