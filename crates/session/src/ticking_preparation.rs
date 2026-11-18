//! The ticking-holder future between FULL generation and chunk activation.
//!
//! The caller supplies actual holder futures and controls when the queued main
//! thread callback runs. A loaded chunk or a simulation ticket alone is not a
//! completed holder future, and this module does not publish world changes.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Weak},
};

use anyhow::{Result, ensure};
use native_world::loaded::ChunkPos;

#[path = "entity_ticking.rs"]
mod entity;
pub use entity::{
    EntityListenerId, EntityLoadingLinks, EntityRequestId, EntityTickingHolder,
    EntityTickingTransition, TickingExecutorEvent, entity_ticking_dependencies,
};

/// Identity of a specific FULL generation future. Holder futures include their
/// epoch; Java can return the same unavailable future for different holders.
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
    identity: Arc<()>,
    center: ChunkPos,
    dependencies: Vec<Dependency>,
    queued: Option<Queued>,
    running: bool,
    holder: bool,
    retained_by_holder: Option<Weak<()>>,
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
            identity: Arc::new(()),
            center,
            dependencies: Vec::with_capacity(9),
            queued: None,
            running: false,
            holder: true,
            retained_by_holder: None,
            state: TickingFutureState::Waiting,
        };
        for position in ticking_dependencies(center) {
            let Some(mut future) = request(position) else {
                preparation.queued = Some(Queued::Unavailable);
                return Ok(preparation);
            };
            for entry in preparation
                .dependencies
                .iter_mut()
                .filter(|entry| entry.future.id == future.id)
            {
                let observed = match (entry.future.result, future.result) {
                    (Some(a), Some(b)) => {
                        ensure!(a == b, "shared FULL future observations disagree");
                        Some(a)
                    }
                    (Some(result), None) | (None, Some(result)) => Some(result),
                    (None, None) => None,
                };
                entry.future.result = observed;
                future.result = observed;
            }
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

    /// Fork a queued callback for off-thread preparation. Its holder remains
    /// pending unless externally completed, and only the holder may publish.
    pub fn fork_queued_callback(&self) -> Result<Self> {
        ensure!(
            self.state == TickingFutureState::Waiting
                && matches!(self.queued, Some(Queued::Prepare))
                && !self.running,
            "ticking callback is not ready for background preparation"
        );
        Ok(Self {
            identity: Arc::clone(&self.identity),
            center: self.center,
            dependencies: self.dependencies.clone(),
            queued: self.queued,
            running: false,
            holder: false,
            retained_by_holder: None,
            state: self.state,
        })
    }

    /// Reserve the queued callback at its executor position and return a
    /// private worker copy. Once started, external completion of the holder
    /// future does not stop the callback's postprocess or unpack side effects.
    pub fn begin_background_callback(&mut self) -> Result<Self> {
        ensure!(self.holder, "a worker copy cannot own the ticking callback");
        let worker = self.fork_queued_callback()?;
        self.reserve_background_callback()?;
        Ok(worker)
    }

    fn reserve_background_callback(&mut self) -> Result<()> {
        ensure!(
            self.holder && self.has_ready_callback(),
            "ticking callback is not ready to start"
        );
        self.queued = None;
        self.running = true;
        Ok(())
    }

    pub fn is_running_callback(&self) -> bool {
        self.running
    }

    pub(crate) fn callback_identity(&self) -> &Arc<()> {
        &self.identity
    }

    pub(crate) fn has_ready_callback(&self) -> bool {
        !self.running
            && self.state == TickingFutureState::Waiting
            && matches!(self.queued, Some(Queued::Prepare))
    }

    pub(crate) fn accepts_prepared_callback(&self) -> bool {
        self.holder && (self.running || self.has_ready_callback())
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
        let mut found = false;
        let mut completed = false;
        for entry in self
            .dependencies
            .iter_mut()
            .filter(|entry| entry.future.id == id)
        {
            found = true;
            if entry.future.result.is_none() {
                entry.future.result = Some(result);
                completed = true;
            }
        }
        ensure!(found, "FULL future is not part of this ticking request");
        if !completed {
            return Ok(false);
        }
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

    /// Complete the holder future exceptionally without suppressing a
    /// callback that has already begun executing.
    pub fn complete_exceptionally(&mut self) -> bool {
        if self.state != TickingFutureState::Waiting {
            return false;
        }
        self.state = TickingFutureState::Exceptional;
        true
    }

    /// Finish an already-started callback. If the holder future was completed
    /// while work was running, preserve that outcome but still run the side
    /// effects, just like CompletableFuture's already-executing task.
    pub fn finish_background_callback(
        &mut self,
        mut postprocess: impl FnMut(ChunkPos) -> Result<()>,
        mut unpack: impl FnMut(ChunkPos) -> Result<()>,
        mut after_start: impl FnMut(ChunkPos) -> Result<()>,
    ) -> Result<bool> {
        ensure!(self.running, "ticking callback is not running");
        self.running = false;
        let waiting = self.state == TickingFutureState::Waiting;
        if waiting {
            // A failed callback completes a still-pending future exceptionally.
            self.state = TickingFutureState::Exceptional;
        }
        postprocess(self.center)?;
        unpack(self.center)?;
        after_start(self.center)?;
        if waiting {
            self.state = TickingFutureState::Available;
        }
        Ok(true)
    }

    pub(crate) fn dispatch_prepared(
        &mut self,
        postprocess: impl FnMut(ChunkPos) -> Result<()>,
        unpack: impl FnMut(ChunkPos) -> Result<()>,
        after_start: impl FnMut(ChunkPos) -> Result<()>,
    ) -> Result<bool> {
        if self.running {
            self.finish_background_callback(postprocess, unpack, after_start)
        } else {
            self.dispatch(postprocess, unpack, after_start)
        }
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
        ensure!(!self.running, "ticking callback is already running");
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FullSubscriber {
    Block(TickingRequestId),
    Entity(EntityRequestId),
}

/// Coordinate overlapping ticking requests and the ordered main-thread queue.
/// FULL completion events must be supplied in their observed delivery order.
#[derive(Default)]
pub struct TickingPreparations {
    identity: Arc<()>,
    futures: BTreeMap<FullFutureId, Option<FullFutureResult>>,
    subscribers: BTreeMap<FullFutureId, Vec<FullSubscriber>>,
    requests: BTreeMap<TickingRequestId, TickingPreparation>,
    queued: VecDeque<TickingRequestId>,
    running: Option<TickingRequestId>,
    entity: Option<entity::EntityTracker>,
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
            self.note_block_queued(id);
        } else if preparation.state() == TickingFutureState::Waiting {
            // A request has at most nine dependencies, so deduplicate without
            // allocating for every chunk entering the ticking range.
            let mut subscribed = [FullFutureId(0); 9];
            let mut count = 0;
            for (_, future) in preparation.dependencies() {
                if future.result.is_none() && !subscribed[..count].contains(&future.id) {
                    subscribed[count] = future.id;
                    count += 1;
                    self.subscribers
                        .entry(future.id)
                        .or_default()
                        .push(FullSubscriber::Block(id));
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
        for subscriber in self
            .subscribers
            .remove(&id)
            .unwrap_or_default()
            .into_iter()
            .rev()
        {
            match subscriber {
                FullSubscriber::Block(request_id) => {
                    let preparation = self
                        .requests
                        .get_mut(&request_id)
                        .expect("registered ticking request");
                    let already_queued = preparation.has_queued_callback();
                    preparation.complete(id, result)?;
                    if !already_queued && preparation.has_queued_callback() {
                        self.queued.push_back(request_id);
                        self.note_block_queued(request_id);
                    }
                }
                FullSubscriber::Entity(request_id) => {
                    self.complete_entity_dependency(request_id, id, result)?;
                }
            }
        }
        Ok(true)
    }

    /// A generation future can complete before a ticking request subscribes.
    /// The holder retains that result; deliver it here only if this executor
    /// already knows its identity.
    pub(crate) fn complete_if_registered(
        &mut self,
        id: FullFutureId,
        result: FullFutureResult,
    ) -> Result<bool> {
        if self.futures.contains_key(&id) {
            self.complete(id, result)
        } else {
            Ok(false)
        }
    }

    pub fn state(&self, id: TickingRequestId) -> Option<TickingFutureState> {
        self.requests.get(&id).map(TickingPreparation::state)
    }

    pub fn cancel(&mut self, id: TickingRequestId) -> Option<bool> {
        self.requests.get_mut(&id).map(TickingPreparation::cancel)
    }

    pub fn complete_exceptionally(&mut self, id: TickingRequestId) -> Option<bool> {
        self.requests
            .get_mut(&id)
            .map(TickingPreparation::complete_exceptionally)
    }

    pub fn queued_len(&self) -> usize {
        self.queued.len()
    }

    #[cfg(test)]
    pub(crate) fn tracked_full_future_count(&self) -> usize {
        self.futures.len()
    }

    /// The running callback has left the queue but still occupies the
    /// executor. Later callbacks and the next simulation tick must wait.
    pub fn running_id(&self) -> Option<TickingRequestId> {
        self.running
    }

    /// Fork the head before starting a background worker. If starting the
    /// worker fails, the holder and executor queue remain untouched.
    pub fn fork_next_background(&self) -> Result<Option<(TickingRequestId, TickingPreparation)>> {
        ensure!(
            self.running.is_none(),
            "a ticking callback is already running"
        );
        let Some(&id) = self.queued.front() else {
            return Ok(None);
        };
        let preparation = self.requests.get(&id).expect("queued ticking request");
        Ok(Some((id, preparation.fork_queued_callback()?)))
    }

    /// Reserve the same queue head from which the submitted worker was forked.
    /// A cancelled or unavailable callback must be dispatched normally; a
    /// stale or different fork cannot consume another holder's position.
    pub fn begin_next_background(
        &mut self,
        fork: &TickingPreparation,
    ) -> Result<Option<TickingRequestId>> {
        ensure!(
            self.running.is_none(),
            "a ticking callback is already running"
        );
        let Some(&id) = self.queued.front() else {
            return Ok(None);
        };
        let preparation = self.requests.get_mut(&id).expect("queued ticking request");
        ensure!(
            !fork.holder
                && fork.has_ready_callback()
                && Arc::ptr_eq(&preparation.identity, &fork.identity),
            "background worker belongs to another ticking callback"
        );
        preparation.reserve_background_callback()?;
        self.queued.pop_front();
        self.running = Some(id);
        Ok(Some(id))
    }

    /// Attempt to finish the current callback in its original executor slot.
    /// If the caller cannot apply its result yet, the slot remains occupied.
    pub fn finish_background<T>(
        &mut self,
        finish: impl FnOnce(&mut TickingPreparation) -> Result<T>,
    ) -> Result<Option<(TickingRequestId, T)>> {
        let Some(id) = self.running else {
            return Ok(None);
        };
        let preparation = self.requests.get_mut(&id).expect("running ticking request");
        let result = finish(preparation);
        if !preparation.is_running_callback() {
            self.running = None;
        }
        result.map(|value| Some((id, value)))
    }

    /// Run one executor callback. If the caller does not consume it, the same
    /// callback stays at the head; failed postprocessing itself is not retried.
    pub fn dispatch_next<T>(
        &mut self,
        dispatch: impl FnOnce(&mut TickingPreparation) -> Result<T>,
    ) -> Result<Option<(TickingRequestId, T)>> {
        ensure!(
            self.running.is_none(),
            "a ticking callback is already running"
        );
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
            !preparation
                .retained_by_holder
                .as_ref()
                .is_some_and(|holder| holder.strong_count() != 0)
                && preparation.state() != TickingFutureState::Waiting
                && !preparation.has_queued_callback()
                && !preparation.is_running_callback(),
            "ticking request is still owned or has work"
        );
        let future_ids: Vec<_> = preparation
            .dependencies()
            .map(|(_, future)| future.id)
            .collect();
        self.requests.remove(&id);
        for future_id in future_ids {
            if let Some(subscribers) = self.subscribers.get_mut(&future_id) {
                subscribers.retain(|&request| request != FullSubscriber::Block(id));
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
                .all(|(_, future)| future.id != id))
                && self.entity_future_is_unused(id),
            "FULL future is still used by a ticking request"
        );
        Ok(self.futures.remove(&id).is_some())
    }

    /// Remove cached future identities that no active or draining block or
    /// entity request still references. A holder retains its own completion
    /// and can register the same future again if it is requested later.
    pub(crate) fn prune_unused_full_futures(&mut self) {
        let mut used = BTreeSet::new();
        for request in self.requests.values() {
            used.extend(request.dependencies().map(|(_, future)| future.id));
        }
        self.collect_entity_full_futures(&mut used);
        self.futures.retain(|id, _| used.contains(id));
    }

    pub(crate) fn required_full_positions(&self) -> BTreeSet<ChunkPos> {
        let mut positions = BTreeSet::new();
        for request in self.requests.values() {
            positions.extend(request.dependencies().filter_map(|(position, future)| {
                matches!(future.result, None | Some(FullFutureResult::Full)).then_some(position)
            }));
        }
        self.collect_entity_full_positions(&mut positions);
        positions
    }
}

/// A holder's loading-ticket transition and currently exposed ticking future.
///
/// The caller must provide the real loading-ticket level, not a simulation
/// ticket or the presence of a chunk in memory. FULL-neighbor futures still
/// come from the actual holders. This does not model accessible or entity
/// futures, status notifications, send synchronization, or world persistence.
pub struct TickingHolder {
    center: ChunkPos,
    queue_identity: Arc<()>,
    holder_identity: Arc<()>,
    applied_level: i32,
    ticket_level: i32,
    current: Option<TickingRequestId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TickingHolderTransition {
    Unchanged,
    Promoted(TickingRequestId),
    Demoted {
        previous: TickingRequestId,
        completed_as_unavailable: bool,
    },
}

impl TickingHolder {
    /// Java starts each newly constructed holder with an inaccessible old
    /// level; its initial ticket level is applied by a subsequent update.
    pub fn new(center: ChunkPos, initial_level: i32, queue: &TickingPreparations) -> Self {
        Self {
            center,
            queue_identity: Arc::clone(&queue.identity),
            holder_identity: Arc::new(()),
            applied_level: i32::MAX,
            ticket_level: initial_level,
            current: None,
        }
    }

    /// Changing the level alone does not update the exposed future.
    pub fn set_ticket_level(&mut self, level: i32) {
        self.ticket_level = level;
    }

    pub fn needs_new_ticking_request(&self) -> bool {
        self.applied_level > 32 && self.ticket_level <= 32
    }

    pub fn current_request(&self) -> Option<TickingRequestId> {
        self.current
    }

    /// Return the currently exposed ticking future, not an older callback
    /// that may still run after this holder has been demoted.
    pub fn state(&self, queue: &TickingPreparations) -> Result<TickingFutureState> {
        ensure!(
            Arc::ptr_eq(&self.queue_identity, &queue.identity),
            "ticking holder belongs to another executor"
        );
        match self.current {
            Some(id) => queue
                .state(id)
                .ok_or_else(|| anyhow::anyhow!("holder ticking future is missing")),
            None => Ok(TickingFutureState::Unavailable),
        }
    }

    /// Apply one observed loading-ticket update. On promotion the caller must
    /// supply a fresh request identity and the real neighboring FULL futures.
    /// Demotion replaces the exposed future immediately, but an already
    /// started or queued callback remains in its original executor position.
    pub fn update(
        &mut self,
        queue: &mut TickingPreparations,
        request_id: Option<TickingRequestId>,
        request_full: impl FnMut(ChunkPos) -> Option<FullFuture>,
    ) -> Result<TickingHolderTransition> {
        ensure!(
            Arc::ptr_eq(&self.queue_identity, &queue.identity),
            "ticking holder belongs to another executor"
        );
        let old_ticking = self.applied_level <= 32;
        let new_ticking = self.ticket_level <= 32;
        let transition = match (old_ticking, new_ticking) {
            (false, true) => {
                ensure!(
                    self.current.is_none(),
                    "holder already has a ticking future"
                );
                let id = request_id
                    .ok_or_else(|| anyhow::anyhow!("new ticking request identity is required"))?;
                queue.start(id, self.center, request_full)?;
                queue
                    .requests
                    .get_mut(&id)
                    .expect("new ticking request")
                    .retained_by_holder = Some(Arc::downgrade(&self.holder_identity));
                self.current = Some(id);
                TickingHolderTransition::Promoted(id)
            }
            (true, false) => {
                let id = self
                    .current
                    .ok_or_else(|| anyhow::anyhow!("holder ticking future is missing"))?;
                ensure!(
                    queue
                        .requests
                        .get(&id)
                        .and_then(|request| request.retained_by_holder.as_ref())
                        .and_then(Weak::upgrade)
                        .is_some_and(|holder| Arc::ptr_eq(&holder, &self.holder_identity)),
                    "holder ticking future is not retained"
                );
                let completed_as_unavailable = queue
                    .cancel(id)
                    .ok_or_else(|| anyhow::anyhow!("holder ticking future is missing"))?;
                queue
                    .requests
                    .get_mut(&id)
                    .expect("retained ticking request")
                    .retained_by_holder = None;
                self.current = None;
                TickingHolderTransition::Demoted {
                    previous: id,
                    completed_as_unavailable,
                }
            }
            _ => TickingHolderTransition::Unchanged,
        };
        self.applied_level = self.ticket_level;
        Ok(transition)
    }
}

#[cfg(test)]
#[path = "ticking_preparation_tests.rs"]
mod tests;
