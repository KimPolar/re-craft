//! Entity-ticking futures share FULL dependencies with block-ticking work.
//!
//! The caller still supplies actual holder futures and must preserve emitted
//! events in the same main-executor order as other work in the game loop.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Weak},
};

use anyhow::{Result, ensure};
use native_world::loaded::ChunkPos;

use crate::simulation_player::{
    PlayerLoadingFutureOutcome, PlayerLoadingRelease, PlayerSimulation,
};

use super::{
    Dependency, FullFuture, FullFutureId, FullFutureResult, FullSubscriber, TickingFutureState,
    TickingPreparations, TickingRequestId,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EntityRequestId(pub u64);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EntityListenerId(pub u64);

/// An observed enqueue or completion, in the order Java runs dependents.
/// Exceptional entity completions are cleanup events, not main-executor work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TickingExecutorEvent {
    BlockQueued(TickingRequestId),
    EntityListener {
        listener: EntityListenerId,
        state: TickingFutureState,
    },
}

/// `prepareEntityTickingChunk` requests FULL in Z-major order, radius two.
pub fn entity_ticking_dependencies(center: ChunkPos) -> [ChunkPos; 25] {
    std::array::from_fn(|index| ChunkPos {
        x: center.x.wrapping_add((index % 5) as i32 - 2),
        z: center.z.wrapping_add((index / 5) as i32 - 2),
    })
}

struct EntityPreparation {
    dependencies: Vec<Dependency>,
    state: TickingFutureState,
    listeners: Vec<EntityListenerId>,
    retained_by_holder: Option<Weak<()>>,
}

impl EntityPreparation {
    fn start(
        center: ChunkPos,
        mut request: impl FnMut(ChunkPos) -> Option<FullFuture>,
    ) -> Result<Self> {
        let mut preparation = Self {
            dependencies: Vec::with_capacity(25),
            state: TickingFutureState::Waiting,
            listeners: Vec::new(),
            retained_by_holder: None,
        };
        for position in entity_ticking_dependencies(center) {
            let Some(mut future) = request(position) else {
                preparation.state = TickingFutureState::Unavailable;
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
        preparation.update_state();
        Ok(preparation)
    }

    fn update_state(&mut self) {
        if self
            .dependencies
            .iter()
            .any(|entry| entry.future.result.is_none())
        {
            return;
        }
        self.state = if self
            .dependencies
            .iter()
            .any(|entry| entry.future.result == Some(FullFutureResult::Exceptional))
        {
            TickingFutureState::Exceptional
        } else if self
            .dependencies
            .iter()
            .any(|entry| entry.future.result == Some(FullFutureResult::Unavailable))
        {
            TickingFutureState::Unavailable
        } else {
            TickingFutureState::Available
        };
    }

    fn complete(&mut self, id: FullFutureId, result: FullFutureResult) -> Result<bool> {
        if self.state != TickingFutureState::Waiting {
            return Ok(false);
        }
        let mut found = false;
        let mut changed = false;
        for entry in self
            .dependencies
            .iter_mut()
            .filter(|entry| entry.future.id == id)
        {
            found = true;
            if entry.future.result.is_none() {
                entry.future.result = Some(result);
                changed = true;
            }
        }
        ensure!(found, "FULL future is not part of this entity request");
        if changed {
            self.update_state();
        }
        Ok(changed)
    }
}

#[derive(Default)]
pub(super) struct EntityTracker {
    requests: BTreeMap<EntityRequestId, EntityPreparation>,
    claimed_listeners: BTreeSet<EntityListenerId>,
    events: VecDeque<TickingExecutorEvent>,
    next_listener: u64,
}

impl TickingPreparations {
    /// Enable ordered entity and block events before registering any futures.
    pub fn with_entity_tracking() -> Self {
        Self {
            entity: Some(EntityTracker::default()),
            ..Self::default()
        }
    }

    pub(super) fn note_block_queued(&mut self, id: TickingRequestId) {
        if let Some(entity) = &mut self.entity {
            entity
                .events
                .push_back(TickingExecutorEvent::BlockQueued(id));
        }
    }

    /// Inspect the next event without changing the shared executor order.
    pub fn peek_executor_event(&self) -> Option<TickingExecutorEvent> {
        self.entity.as_ref()?.events.front().copied()
    }

    /// Pop one observed event. Normal entity events schedule the listener on
    /// its owning executor; exceptional events only discard its registration.
    pub fn next_executor_event(&mut self) -> Option<TickingExecutorEvent> {
        let entity = self.entity.as_mut()?;
        let event = entity.events.pop_front()?;
        if let TickingExecutorEvent::EntityListener { listener, .. } = event {
            entity.claimed_listeners.remove(&listener);
        }
        Some(event)
    }

    /// Start one actual holder's entity-ticking request. A missing holder
    /// completes the returned future as unavailable immediately.
    pub fn start_entity(
        &mut self,
        id: EntityRequestId,
        center: ChunkPos,
        mut request: impl FnMut(ChunkPos) -> Option<FullFuture>,
    ) -> Result<()> {
        let entity = self
            .entity
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("entity tracking is not enabled"))?;
        ensure!(
            !entity.requests.contains_key(&id),
            "entity request identity is already registered"
        );
        let mut inconsistent = false;
        let preparation = EntityPreparation::start(center, |position| {
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
        for entry in &preparation.dependencies {
            self.futures
                .entry(entry.future.id)
                .or_insert(entry.future.result);
        }
        if preparation.state == TickingFutureState::Waiting {
            let mut subscribed = [FullFutureId(0); 25];
            let mut count = 0;
            for entry in &preparation.dependencies {
                if entry.future.result.is_none() && !subscribed[..count].contains(&entry.future.id)
                {
                    subscribed[count] = entry.future.id;
                    count += 1;
                    self.subscribers
                        .entry(entry.future.id)
                        .or_default()
                        .push(FullSubscriber::Entity(id));
                }
            }
        }
        self.entity
            .as_mut()
            .expect("entity tracker exists")
            .requests
            .insert(id, preparation);
        Ok(())
    }

    pub fn entity_state(&self, id: EntityRequestId) -> Option<TickingFutureState> {
        self.entity
            .as_ref()?
            .requests
            .get(&id)
            .map(|request| request.state)
    }

    /// Register a `thenAccept`-style listener against this exact future.
    pub fn register_entity_listener(
        &mut self,
        id: EntityRequestId,
        listener: EntityListenerId,
    ) -> Result<()> {
        let entity = self
            .entity
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("entity tracking is not enabled"))?;
        let request = entity
            .requests
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("entity future is missing"))?;
        ensure!(
            entity.claimed_listeners.insert(listener),
            "entity listener identity is already in use"
        );
        if request.state == TickingFutureState::Waiting {
            request.listeners.push(listener);
        } else {
            entity
                .events
                .push_back(TickingExecutorEvent::EntityListener {
                    listener,
                    state: request.state,
                });
        }
        Ok(())
    }

    fn register_unavailable_listener(&mut self, listener: EntityListenerId) -> Result<()> {
        let entity = self
            .entity
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("entity tracking is not enabled"))?;
        ensure!(
            entity.claimed_listeners.insert(listener),
            "entity listener identity is already in use"
        );
        entity
            .events
            .push_back(TickingExecutorEvent::EntityListener {
                listener,
                state: TickingFutureState::Unavailable,
            });
        Ok(())
    }

    fn allocate_listener(&mut self) -> Result<EntityListenerId> {
        let entity = self
            .entity
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("entity tracking is not enabled"))?;
        loop {
            let listener = EntityListenerId(entity.next_listener);
            entity.next_listener = entity
                .next_listener
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("entity listener identity overflow"))?;
            if !entity.claimed_listeners.contains(&listener) {
                return Ok(listener);
            }
        }
    }

    pub(super) fn complete_entity_dependency(
        &mut self,
        request: EntityRequestId,
        id: FullFutureId,
        result: FullFutureResult,
    ) -> Result<()> {
        let entity = self.entity.as_mut().expect("entity tracker exists");
        let preparation = entity
            .requests
            .get_mut(&request)
            .expect("registered entity request");
        preparation.complete(id, result)?;
        if preparation.state != TickingFutureState::Waiting {
            // Completing the allOf-dependent future runs its listeners in the
            // observed Java order. Direct completion uses the opposite order.
            for listener in preparation.listeners.drain(..) {
                entity
                    .events
                    .push_back(TickingExecutorEvent::EntityListener {
                        listener,
                        state: preparation.state,
                    });
            }
        }
        Ok(())
    }

    /// Complete a pending holder future as unavailable on ticket demotion.
    pub fn cancel_entity(&mut self, id: EntityRequestId) -> Result<bool> {
        let entity = self
            .entity
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("entity tracking is not enabled"))?;
        let request = entity
            .requests
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("entity future is missing"))?;
        if request.state != TickingFutureState::Waiting {
            return Ok(false);
        }
        request.state = TickingFutureState::Unavailable;
        for listener in request.listeners.drain(..).rev() {
            entity
                .events
                .push_back(TickingExecutorEvent::EntityListener {
                    listener,
                    state: TickingFutureState::Unavailable,
                });
        }
        Ok(true)
    }

    pub fn retire_entity(&mut self, id: EntityRequestId) -> Result<bool> {
        let entity = self
            .entity
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("entity tracking is not enabled"))?;
        let Some(request) = entity.requests.get(&id) else {
            return Ok(false);
        };
        ensure!(
            request.state != TickingFutureState::Waiting
                && request.listeners.is_empty()
                && !request
                    .retained_by_holder
                    .as_ref()
                    .is_some_and(|holder| holder.strong_count() != 0),
            "entity request is still owned or has work"
        );
        let ids: Vec<_> = request
            .dependencies
            .iter()
            .map(|entry| entry.future.id)
            .collect();
        entity.requests.remove(&id);
        for future in ids {
            if let Some(subscribers) = self.subscribers.get_mut(&future) {
                subscribers.retain(|&subscriber| subscriber != FullSubscriber::Entity(id));
                if subscribers.is_empty() {
                    self.subscribers.remove(&future);
                }
            }
        }
        Ok(true)
    }

    pub(super) fn entity_future_is_unused(&self, id: FullFutureId) -> bool {
        self.entity.as_ref().is_none_or(|entity| {
            entity.requests.values().all(|request| {
                request
                    .dependencies
                    .iter()
                    .all(|entry| entry.future.id != id)
            })
        })
    }

    pub(super) fn collect_entity_full_futures(&self, used: &mut BTreeSet<FullFutureId>) {
        if let Some(entity) = &self.entity {
            for request in entity.requests.values() {
                used.extend(request.dependencies.iter().map(|entry| entry.future.id));
            }
        }
    }
}

/// Retains player-loading release tokens until the corresponding holder
/// future completes. The caller must deliver events in the shared executor's
/// order and keep this registry alive while registrations are pending.
pub struct EntityLoadingLinks {
    queue_identity: Arc<()>,
    registrations: BTreeMap<EntityListenerId, PlayerLoadingRelease>,
}

impl EntityLoadingLinks {
    pub fn new(queue: &TickingPreparations) -> Result<Self> {
        ensure!(queue.entity.is_some(), "entity tracking is not enabled");
        Ok(Self {
            queue_identity: Arc::clone(&queue.identity),
            registrations: BTreeMap::new(),
        })
    }

    /// Register the token only against its own chunk's current holder future.
    pub fn register(
        &mut self,
        queue: &mut TickingPreparations,
        holder: &EntityTickingHolder,
        token: PlayerLoadingRelease,
    ) -> Result<()> {
        ensure!(
            Arc::ptr_eq(&self.queue_identity, &queue.identity),
            "player loading links belong to another executor"
        );
        ensure!(
            token.position() == holder.center,
            "player loading holder position differs"
        );
        let listener = queue.allocate_listener()?;
        holder.register_listener(queue, listener)?;
        self.registrations.insert(listener, token);
        Ok(())
    }

    /// Deliver an observed entity event. Normal results enqueue a release on
    /// the player's main executor; exceptions only discard the registration.
    pub fn deliver(
        &mut self,
        queue: &TickingPreparations,
        player: &mut PlayerSimulation,
        listener: EntityListenerId,
        state: TickingFutureState,
    ) -> Result<()> {
        ensure!(
            Arc::ptr_eq(&self.queue_identity, &queue.identity),
            "player loading links belong to another executor"
        );
        let outcome = match state {
            TickingFutureState::Available | TickingFutureState::Unavailable => {
                PlayerLoadingFutureOutcome::Completed
            }
            TickingFutureState::Exceptional => PlayerLoadingFutureOutcome::Exceptional,
            TickingFutureState::Waiting => anyhow::bail!("entity future has not completed"),
        };
        let token = self
            .registrations
            .get(&listener)
            .ok_or_else(|| anyhow::anyhow!("player loading listener is not registered"))?;
        player.player_loading_future_completed(token, outcome)?;
        self.registrations.remove(&listener);
        Ok(())
    }

    pub fn pending_len(&self) -> usize {
        self.registrations.len()
    }

    pub(crate) fn has_pending_at(&self, position: ChunkPos) -> bool {
        self.registrations
            .values()
            .any(|token| token.position() == position)
    }
}

/// The entity-ticking portion of a holder's loading-ticket lifecycle.
pub struct EntityTickingHolder {
    center: ChunkPos,
    queue_identity: Arc<()>,
    holder_identity: Arc<()>,
    applied_level: i32,
    ticket_level: i32,
    current: Option<EntityRequestId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntityTickingTransition {
    Unchanged,
    Promoted(EntityRequestId),
    Demoted {
        previous: EntityRequestId,
        completed_as_unavailable: bool,
    },
}

impl EntityTickingHolder {
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

    pub fn set_ticket_level(&mut self, level: i32) {
        self.ticket_level = level;
    }
    pub fn needs_new_entity_request(&self) -> bool {
        self.applied_level > 31 && self.ticket_level <= 31
    }
    pub fn current_request(&self) -> Option<EntityRequestId> {
        self.current
    }

    pub fn state(&self, queue: &TickingPreparations) -> Result<TickingFutureState> {
        ensure!(
            Arc::ptr_eq(&self.queue_identity, &queue.identity),
            "entity holder belongs to another executor"
        );
        match self.current {
            Some(id) => queue
                .entity_state(id)
                .ok_or_else(|| anyhow::anyhow!("holder entity future is missing")),
            None => Ok(TickingFutureState::Unavailable),
        }
    }

    /// The caller must update block ticking first, as ChunkHolder does, and
    /// must supply this holder's actual 5×5 FULL futures on promotion.
    pub fn update(
        &mut self,
        queue: &mut TickingPreparations,
        request_id: Option<EntityRequestId>,
        request_full: impl FnMut(ChunkPos) -> Option<FullFuture>,
    ) -> Result<EntityTickingTransition> {
        ensure!(
            Arc::ptr_eq(&self.queue_identity, &queue.identity),
            "entity holder belongs to another executor"
        );
        let transition = match (self.applied_level <= 31, self.ticket_level <= 31) {
            (false, true) => {
                ensure!(
                    self.current.is_none(),
                    "holder already has an entity future"
                );
                let id = request_id
                    .ok_or_else(|| anyhow::anyhow!("new entity request identity is required"))?;
                queue.start_entity(id, self.center, request_full)?;
                queue
                    .entity
                    .as_mut()
                    .expect("entity tracker exists")
                    .requests
                    .get_mut(&id)
                    .expect("new entity request")
                    .retained_by_holder = Some(Arc::downgrade(&self.holder_identity));
                self.current = Some(id);
                EntityTickingTransition::Promoted(id)
            }
            (true, false) => {
                let id = self
                    .current
                    .ok_or_else(|| anyhow::anyhow!("holder entity future is missing"))?;
                ensure!(
                    queue
                        .entity
                        .as_ref()
                        .and_then(|entity| entity.requests.get(&id))
                        .and_then(|request| request.retained_by_holder.as_ref())
                        .and_then(Weak::upgrade)
                        .is_some_and(|owner| Arc::ptr_eq(&owner, &self.holder_identity)),
                    "holder entity future is not retained"
                );
                let completed_as_unavailable = queue.cancel_entity(id)?;
                queue
                    .entity
                    .as_mut()
                    .expect("entity tracker exists")
                    .requests
                    .get_mut(&id)
                    .expect("retained entity request")
                    .retained_by_holder = None;
                self.current = None;
                EntityTickingTransition::Demoted {
                    previous: id,
                    completed_as_unavailable,
                }
            }
            _ => EntityTickingTransition::Unchanged,
        };
        self.applied_level = self.ticket_level;
        Ok(transition)
    }

    /// The default unloaded holder future completes normally immediately.
    pub fn register_listener(
        &self,
        queue: &mut TickingPreparations,
        listener: EntityListenerId,
    ) -> Result<()> {
        ensure!(
            Arc::ptr_eq(&self.queue_identity, &queue.identity),
            "entity holder belongs to another executor"
        );
        match self.current {
            Some(id) => queue.register_entity_listener(id, listener),
            None => queue.register_unavailable_listener(listener),
        }
    }
}

#[cfg(test)]
#[path = "entity_ticking_tests.rs"]
mod tests;
