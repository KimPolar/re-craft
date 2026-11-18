//! Resolve holder FULL requests from a locked saved world without blocking the
//! event loop. A saved chunk must also attach to the matching live session
//! before its holder future can complete.

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread,
};

use anyhow::{Context, Result, ensure};
use native_persistence::{
    nbt,
    save::{WorldReadStamp, WorldSessionGuard},
    world::{DimensionId, WorldReader},
};
use native_session::{
    loaded::LoadedSession,
    loading_holders::{FullGenerationRequest, LoadingHolders, LoadingHoldersOwner},
};
use native_world::loaded::ChunkPos;

use crate::chunk_stream::ChunkStreamer;

const MAX_PENDING: usize = 128;
const READER_QUEUE: usize = 8;

struct ReadRequest {
    id: u64,
    position: ChunkPos,
    stamp: WorldReadStamp,
}

struct ReadReply {
    id: u64,
    result: ReadResult,
}

enum ReadResult {
    Full([u8; 32]),
    Missing,
    Incomplete,
    Failed(String),
    Stale,
}

enum PendingState {
    Unsent,
    Reading,
    Full([u8; 32]),
    Missing,
    Incomplete,
    Failed(String),
}

struct Pending {
    id: u64,
    request: FullGenerationRequest,
    state: PendingState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SavedFullUnavailable {
    Missing,
    Incomplete,
}

/// The FULL reader and holder coordinator must be retired together. A missing
/// or unfinished saved chunk remains pending for the generation path; this
/// bridge never manufactures a completed future for it.
pub(crate) struct SavedFullStream {
    owner: LoadingHoldersOwner,
    guard: WorldSessionGuard,
    dimension: DimensionId,
    stamp: WorldReadStamp,
    requests: Option<SyncSender<ReadRequest>>,
    replies: Receiver<ReadReply>,
    stopped: Arc<AtomicBool>,
    pending: VecDeque<Pending>,
    next_id: u64,
}

impl SavedFullStream {
    /// Open the world on the worker, not on the UI/simulation thread.
    pub fn start(
        world_dir: PathBuf,
        dimension: DimensionId,
        guard: WorldSessionGuard,
        session: &LoadedSession,
        holders: &LoadingHolders,
    ) -> Result<Self> {
        let stamp = guard.read_stamp()?;
        session.validate_saved_holder_world(&stamp, &dimension)?;
        let (requests, receiver) = mpsc::sync_channel::<ReadRequest>(READER_QUEUE);
        let (sender, replies) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = Arc::clone(&stopped);
        let worker_guard = guard.clone();
        let worker_dimension = dimension.clone();
        thread::Builder::new()
            .name("minecraft-holder-full-reader".into())
            .spawn(move || {
                let mut reader = None;
                while let Ok(request) = receiver.recv() {
                    if worker_stopped.load(Ordering::Acquire) {
                        break;
                    }
                    let result = if !request.stamp.is_current() {
                        ReadResult::Stale
                    } else {
                        let mut opening_error = None;
                        if reader.is_none() {
                            match WorldReader::open_with_session(
                                &world_dir,
                                worker_dimension.clone(),
                                worker_guard.clone(),
                            ) {
                                Ok(opened) => reader = Some(opened),
                                Err(error) => opening_error = Some(format!("{error:#}")),
                            }
                        }
                        match &mut reader {
                            Some(reader) => {
                                match reader.read_chunk(request.position.x, request.position.z) {
                                    Ok(None) => ReadResult::Missing,
                                    Ok(Some(chunk)) if !chunk.is_full() => ReadResult::Incomplete,
                                    Ok(Some(chunk)) => match nbt::fingerprint(&chunk.document) {
                                        Ok(source) => ReadResult::Full(source),
                                        Err(error) => ReadResult::Failed(format!("{error:#}")),
                                    },
                                    Err(error) => ReadResult::Failed(format!("{error:#}")),
                                }
                            }
                            None => ReadResult::Failed(opening_error.expect("opening failed")),
                        }
                    };
                    if matches!(result, ReadResult::Failed(_)) {
                        // A later explicit retry must be able to reopen a
                        // repaired path or recover from a transient read.
                        reader = None;
                    }
                    let result = if request.stamp.is_current() {
                        result
                    } else {
                        ReadResult::Stale
                    };
                    if sender
                        .send(ReadReply {
                            id: request.id,
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .context("cannot start saved FULL reader")?;
        Ok(Self {
            owner: holders.owner(),
            guard,
            dimension,
            stamp,
            requests: Some(requests),
            replies,
            stopped,
            pending: VecDeque::new(),
            next_id: 0,
        })
    }

    /// Service disk requests and publish only results still matching the
    /// attached session. The caller controls tick and executor ordering.
    pub fn service(
        &mut self,
        holders: &mut LoadingHolders,
        streamer: &mut ChunkStreamer,
        session: &mut LoadedSession,
    ) -> Result<usize> {
        ensure!(
            self.requests.is_some() && !self.stopped.load(Ordering::Acquire),
            "saved FULL reader is closed"
        );
        ensure!(
            holders.belongs_to_owner(&self.owner),
            "saved FULL reader belongs to another holder coordinator"
        );
        let current = self.guard.read_stamp()?;
        ensure!(
            current.epoch() == self.stamp.epoch() && self.stamp.is_current(),
            "saved FULL world changed; reopen the holder coordinator"
        );
        session.validate_saved_holder_world(&self.stamp, &self.dimension)?;
        streamer.validate_holder_world(&self.stamp, &self.dimension)?;
        session.set_holder_dependencies(
            &self.stamp,
            &self.dimension,
            Arc::clone(holders.required_full_positions()),
        )?;
        streamer.service(session)?;

        loop {
            match self.replies.try_recv() {
                Ok(reply) => {
                    let Some(pending) = self.pending.iter_mut().find(|entry| entry.id == reply.id)
                    else {
                        // A demoted holder may have cancelled this read.
                        continue;
                    };
                    ensure!(
                        matches!(pending.state, PendingState::Reading),
                        "saved FULL read completed twice"
                    );
                    pending.state = match reply.result {
                        ReadResult::Full(source) => PendingState::Full(source),
                        ReadResult::Missing => PendingState::Missing,
                        ReadResult::Incomplete => PendingState::Incomplete,
                        ReadResult::Failed(error) => PendingState::Failed(error),
                        ReadResult::Stale => anyhow::bail!("saved FULL read became stale"),
                    };
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    anyhow::bail!("saved FULL reader stopped unexpectedly")
                }
            }
        }

        let _lease = self.stamp.lock_current()?;
        let mut completed = 0;
        for _ in 0..self.pending.len() {
            let pending = self.pending.pop_front().expect("pending read exists");
            match holders.generation_request_needed(&pending.request) {
                Ok(false) => {
                    holders.discard_unneeded_generation_request(pending.request)?;
                    continue;
                }
                Err(error) => {
                    self.pending.push_front(pending);
                    return Err(error);
                }
                Ok(true) => {}
            }
            if let PendingState::Failed(error) = &pending.state {
                let error = anyhow::anyhow!(
                    "saved FULL read failed at {:?}: {error}",
                    pending.request.position()
                );
                self.pending.push_front(pending);
                return Err(error);
            }
            let ready = if let PendingState::Full(source) = pending.state {
                match session.saved_full_chunk_attached(
                    &self.stamp,
                    &self.dimension,
                    pending.request.position(),
                    source,
                ) {
                    Ok(ready) => ready,
                    Err(error) => {
                        self.pending.push_front(pending);
                        return Err(error);
                    }
                }
            } else {
                false
            };
            if ready {
                holders.complete_full_generation(pending.request)?;
                completed += 1;
            } else {
                self.pending.push_back(pending);
            }
        }
        drop(_lease);

        while self.pending.len() < MAX_PENDING {
            let next = self
                .next_id
                .checked_add(1)
                .context("saved FULL request identity exhausted")?;
            let Some(request) = holders.next_generation_request() else {
                break;
            };
            let id = self.next_id;
            self.next_id = next;
            self.pending.push_back(Pending {
                id,
                request,
                state: PendingState::Unsent,
            });
        }
        for pending in &mut self.pending {
            if !matches!(pending.state, PendingState::Unsent) {
                continue;
            }
            let request = ReadRequest {
                id: pending.id,
                position: pending.request.position(),
                stamp: self.stamp.clone(),
            };
            match self
                .requests
                .as_ref()
                .context("saved FULL reader is closed")?
                .try_send(request)
            {
                Ok(()) => pending.state = PendingState::Reading,
                Err(TrySendError::Full(_)) => break,
                Err(TrySendError::Disconnected(_)) => {
                    anyhow::bail!("saved FULL reader stopped unexpectedly")
                }
            }
        }
        Ok(completed)
    }

    /// Missing or unfinished saved chunks need generation, not a fake FULL result.
    pub fn unavailable_positions(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.pending.iter().filter_map(|entry| {
            matches!(
                entry.state,
                PendingState::Missing | PendingState::Incomplete
            )
            .then_some(entry.request.position())
        })
    }

    /// Transfer a missing/incomplete request to the generation path while
    /// preserving its original holder and executor identity.
    pub fn take_unavailable(&mut self) -> Option<(FullGenerationRequest, SavedFullUnavailable)> {
        let index = self.pending.iter().position(|entry| {
            matches!(
                entry.state,
                PendingState::Missing | PendingState::Incomplete
            )
        })?;
        let pending = self.pending.remove(index)?;
        let kind = match pending.state {
            PendingState::Missing => SavedFullUnavailable::Missing,
            PendingState::Incomplete => SavedFullUnavailable::Incomplete,
            _ => unreachable!("unavailable state was checked"),
        };
        Some((pending.request, kind))
    }

    /// Retry transient read errors only when the caller decides to do so.
    pub fn retry_failed(&mut self) -> usize {
        let mut retried = 0;
        for pending in &mut self.pending {
            if matches!(pending.state, PendingState::Failed(_)) {
                pending.state = PendingState::Unsent;
                retried += 1;
            }
        }
        retried
    }

    /// Return all owned requests before replacing this bridge while retaining
    /// the same holder coordinator. Dropping both with the world needs no wait.
    pub fn return_pending(&mut self, holders: &mut LoadingHolders) -> Result<()> {
        ensure!(
            holders.belongs_to_owner(&self.owner),
            "saved FULL reader belongs to another holder coordinator"
        );
        for pending in &self.pending {
            holders.generation_request_needed(&pending.request)?;
        }
        self.stopped.store(true, Ordering::Release);
        self.requests.take();
        while let Some(pending) = self.pending.pop_front() {
            holders.defer_generation_request(pending.request)?;
        }
        Ok(())
    }
}

impl Drop for SavedFullStream {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.requests.take();
    }
}
