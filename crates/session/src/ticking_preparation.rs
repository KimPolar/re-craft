//! The ticking-holder future between FULL generation and chunk activation.
//!
//! The caller supplies actual holder futures and controls when the queued main
//! thread callback runs. A loaded chunk or a simulation ticket alone is not a
//! completed holder future, and this module does not publish world changes.

use std::collections::{BTreeMap, VecDeque};

use anyhow::{Result, ensure};
use native_world::loaded::ChunkPos;

/// Identity of a specific FULL generation future, including its holder epoch.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct FullFutureId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FullFutureResult {
    Full,
    Unavailable,
    Exceptional,
}

/// A FULL request and its observed completion, if already finished.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FullFuture {
    pub id: FullFutureId,
    pub result: Option<FullFutureResult>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TickingFutureState {
    Waiting,
    Available,
    Unavailable,
    Exceptional,
}

#[derive(Clone, Copy, Debug)]
struct Dependency {
    position: ChunkPos,
    future: FullFuture,
}

#[derive(Clone, Copy, Debug)]
enum Queued {
    Prepare,
    Unavailable,
}

/// One `ChunkMap.prepareTickingChunk` future. Multiple holders may share FULL
/// futures; the caller must deliver completion to each dependent request in the
/// order those notifications actually arrive.
pub struct TickingPreparation {
    center: ChunkPos,
    dependencies: Vec<Dependency>,
    queued: Option<Queued>,
    state: TickingFutureState,
}

/// Java's `getChunkRangeFuture` requests rows in Z order, then X order, with
/// wrapping Java integer coordinates. The middle item is the center chunk.
pub fn ticking_dependencies(center: ChunkPos) -> [ChunkPos; 9] {
    std::array::from_fn(|index| ChunkPos {
        x: center.x.wrapping_add((index % 3) as i32 - 1),
        z: center.z.wrapping_add((index / 3) as i32 - 1),
    })
}

impl TickingPreparation {
    /// Request FULL futures in Java's order. A missing holder stops further
    /// requests and queues an unavailable result, even if prior futures remain
    /// pending. Starting work does not itself establish ticking readiness.
    pub fn start(
        center: ChunkPos,
        mut request: impl FnMut(ChunkPos) -> Option<FullFuture>,
    ) -> Result<Self> {
        let mut preparation = Self {
            center,
            dependencies: Vec::with_capacity(9),
            queued: None,
            state: TickingFutureState::Waiting,
        };
        for position in ticking_dependencies(center) {
            let Some(future) = request(position) else {
                preparation.queued = Some(Queued::Unavailable);
                return Ok(preparation);
            };
            ensure!(
                preparation
                    .dependencies
                    .iter()
                    .all(|entry| entry.future.id != future.id),
                "FULL future identity is duplicated within a ticking request"
            );
            preparation
                .dependencies
                .push(Dependency { position, future });
        }
        preparation.check_completions();
        Ok(preparation)
    }

    /// Requested futures, including those scheduled before a missing holder.
    pub fn dependencies(&self) -> impl ExactSizeIterator<Item = (ChunkPos, FullFuture)> + '_ {
        self.dependencies
            .iter()
            .map(|entry| (entry.position, entry.future))
    }

    pub fn state(&self) -> TickingFutureState {
        self.state
    }

    pub fn center(&self) -> ChunkPos {
        self.center
    }

    /// The executor may still contain a callback after the future is cancelled.
    pub fn has_queued_callback(&self) -> bool {
        self.queued.is_some()
    }

    /// Deliver completion of one observed FULL future. A future can complete
    /// only once; repeated or late completion leaves this request unchanged.
    pub fn complete(&mut self, id: FullFutureId, result: FullFutureResult) -> Result<bool> {
        if self.state != TickingFutureState::Waiting || self.queued.is_some() {
            return Ok(false);
        }
        let entry = self
            .dependencies
            .iter_mut()
            .find(|entry| entry.future.id == id);
        let Some(entry) = entry else {
            anyhow::bail!("FULL future is not part of this ticking request")
        };
        if entry.future.result.is_some() {
            return Ok(false);
        }
        entry.future.result = Some(result);
        self.check_completions();
        Ok(true)
    }

    fn check_completions(&mut self) {
        if self
            .dependencies
            .iter()
            .any(|entry| entry.future.result.is_none())
        {
            return;
        }
        if self
            .dependencies
            .iter()
            .any(|entry| entry.future.result == Some(FullFutureResult::Exceptional))
        {
            self.state = TickingFutureState::Exceptional;
        } else if self
            .dependencies
            .iter()
            .any(|entry| entry.future.result == Some(FullFutureResult::Unavailable))
        {
            self.queued = Some(Queued::Unavailable);
        } else {
            self.queued = Some(Queued::Prepare);
        }
    }

    /// Model completing the holder's future as unloaded when its ticket drops.
    pub fn cancel(&mut self) -> bool {
        if self.state != TickingFutureState::Waiting {
            return false;
        }
        self.state = TickingFutureState::Unavailable;
        true
    }

    /// Execute the queued main-thread callback. Java postprocesses the center,
    /// unpacks saved ticks, then checks the chunk's send-sync future before
    /// completing. `after_start` must perform that check and any synchronous
    /// send notification or enqueue its deferred notification. A failure makes
    /// the future exceptional; callers must account for earlier side effects.
    pub fn dispatch(
        &mut self,
        mut postprocess: impl FnMut(ChunkPos) -> Result<()>,
        mut unpack: impl FnMut(ChunkPos) -> Result<()>,
        mut after_start: impl FnMut(ChunkPos) -> Result<()>,
    ) -> Result<bool> {
        let Some(queued) = self.queued.take() else {
            return Ok(false);
        };
        if self.state != TickingFutureState::Waiting {
            return Ok(true);
        }
        match queued {
            Queued::Unavailable => self.state = TickingFutureState::Unavailable,
            Queued::Prepare => {
                // If either callback fails or unwinds, this request cannot be
                // retried as though no side effects had happened.
                self.state = TickingFutureState::Exceptional;
                postprocess(self.center)?;
                unpack(self.center)?;
                after_start(self.center)?;
                self.state = TickingFutureState::Available;
            }
        }
        Ok(true)
    }
}

/// Identity of one holder's ticking future, including its holder epoch.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TickingRequestId(pub u64);

/// Coordinate overlapping ticking requests and the ordered main-thread queue.
/// FULL completion events must be supplied in their observed delivery order.
#[derive(Default)]
pub struct TickingPreparations {
    futures: BTreeMap<FullFutureId, Option<FullFutureResult>>,
    subscribers: BTreeMap<FullFutureId, Vec<TickingRequestId>>,
    requests: BTreeMap<TickingRequestId, TickingPreparation>,
    queued: VecDeque<TickingRequestId>,
}

impl TickingPreparations {
    /// Register the actual holder request. Shared FULL futures are reused;
    /// contradictory observations are rejected instead of replacing history.
    pub fn start(
        &mut self,
        id: TickingRequestId,
        center: ChunkPos,
        mut request: impl FnMut(ChunkPos) -> Option<FullFuture>,
    ) -> Result<()> {
        ensure!(
            !self.requests.contains_key(&id),
            "ticking request identity is already registered"
        );
        let mut inconsistent = false;
        let preparation = TickingPreparation::start(center, |position| {
            let mut future = request(position)?;
            if let Some(known) = self.futures.get(&future.id) {
                match (*known, future.result) {
                    (Some(expected), Some(observed)) if expected != observed => inconsistent = true,
                    (Some(expected), None) => future.result = Some(expected),
                    (None, Some(_)) => inconsistent = true,
                    _ => {}
                }
            }
            Some(future)
        })?;
        ensure!(!inconsistent, "shared FULL future observations disagree");
        for (_, future) in preparation.dependencies() {
            self.futures.entry(future.id).or_insert(future.result);
        }
        if preparation.has_queued_callback() {
            self.queued.push_back(id);
        } else if preparation.state() == TickingFutureState::Waiting {
            for (_, future) in preparation.dependencies() {
                if future.result.is_none() {
                    self.subscribers.entry(future.id).or_default().push(id);
                }
            }
        }
        self.requests.insert(id, preparation);
        Ok(())
    }

    /// Deliver one FULL completion to dependent requests. CompletableFuture's
    /// dependent stack wakes later registrations first for the same future.
    pub fn complete(&mut self, id: FullFutureId, result: FullFutureResult) -> Result<bool> {
        let state = self
            .futures
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("unknown FULL future"))?;
        if state.is_some() {
            return Ok(false);
        }
        *state = Some(result);
        for request_id in self
            .subscribers
            .remove(&id)
            .unwrap_or_default()
            .into_iter()
            .rev()
        {
            let preparation = self
                .requests
                .get_mut(&request_id)
                .expect("registered ticking request");
            let already_queued = preparation.has_queued_callback();
            preparation.complete(id, result)?;
            if !already_queued && preparation.has_queued_callback() {
                self.queued.push_back(request_id);
            }
        }
        Ok(true)
    }

    pub fn state(&self, id: TickingRequestId) -> Option<TickingFutureState> {
        self.requests.get(&id).map(TickingPreparation::state)
    }

    pub fn cancel(&mut self, id: TickingRequestId) -> Option<bool> {
        self.requests.get_mut(&id).map(TickingPreparation::cancel)
    }

    pub fn queued_len(&self) -> usize {
        self.queued.len()
    }

    /// Run one executor callback. If the caller does not consume it, the same
    /// callback stays at the head; failed postprocessing itself is not retried.
    pub fn dispatch_next<T>(
        &mut self,
        dispatch: impl FnOnce(&mut TickingPreparation) -> Result<T>,
    ) -> Result<Option<(TickingRequestId, T)>> {
        let Some(id) = self.queued.pop_front() else {
            return Ok(None);
        };
        let preparation = self.requests.get_mut(&id).expect("queued ticking request");
        let result = dispatch(preparation);
        if preparation.has_queued_callback() {
            self.queued.push_front(id);
        }
        result.map(|value| Some((id, value)))
    }

    /// Remove a terminal request after its queued callback has been drained.
    pub fn retire(&mut self, id: TickingRequestId) -> Result<bool> {
        let Some(preparation) = self.requests.get(&id) else {
            return Ok(false);
        };
        ensure!(
            preparation.state() != TickingFutureState::Waiting
                && !preparation.has_queued_callback(),
            "ticking request still has work"
        );
        let future_ids: Vec<_> = preparation
            .dependencies()
            .map(|(_, future)| future.id)
            .collect();
        self.requests.remove(&id);
        for future_id in future_ids {
            if let Some(subscribers) = self.subscribers.get_mut(&future_id) {
                subscribers.retain(|&request| request != id);
                if subscribers.is_empty() {
                    self.subscribers.remove(&future_id);
                }
            }
        }
        Ok(true)
    }

    /// Discard a holder-future identity only after no tracked request uses it.
    pub fn forget_full_future(&mut self, id: FullFutureId) -> Result<bool> {
        ensure!(
            self.requests.values().all(|preparation| preparation
                .dependencies()
                .all(|(_, future)| future.id != id)),
            "FULL future is still used by a ticking request"
        );
        Ok(self.futures.remove(&id).is_some())
    }
}

#[cfg(test)]
#[path = "ticking_preparation_tests.rs"]
mod tests;
