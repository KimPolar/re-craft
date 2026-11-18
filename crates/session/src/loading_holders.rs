//! Connect loading-ticket changes to holder futures and generation requests.
//!
//! The owner of this coordinator must drive the real player-loading and main
//! executors, validate generation results against the current world, and run
//! queued ticking callbacks in their emitted order. Merely creating a holder
//! or requesting its FULL future never marks a chunk ready.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use native_world::loaded::ChunkPos;

use crate::{
    distance_updates::{DistanceDriverIdentity, DistanceUpdateDriver, HolderIdentity},
    full_generation::{FullFutureIds, FullGenerationHolder, FullGenerationOwner},
    loading_chunks::{LoadingLevelChange, unloaded_level},
    simulation_player::{PlayerLoadingRelease, PlayerSimulation},
    ticking_preparation::{
        EntityListenerId, EntityLoadingLinks, EntityRequestId, EntityTickingHolder,
        EntityTickingTransition, FullFuture, FullFutureResult, TickingExecutorEvent,
        TickingFutureState, TickingHolder, TickingHolderTransition, TickingPreparations,
        TickingRequestId, entity_ticking_dependencies, ticking_dependencies,
    },
};

struct Holder {
    identity: HolderIdentity,
    level: u8,
    generation: FullGenerationHolder,
    block: TickingHolder,
    entity: EntityTickingHolder,
    generation_task: Option<GenerationTask>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum GenerationTaskState {
    Queued,
    Dispatched,
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct GenerationTask {
    id: u64,
    state: GenerationTaskState,
}

/// One request to produce a FULL chunk for a particular live holder. The
/// caller must retain its world stamp and validate the resulting chunk before
/// delivering completion back to this coordinator.
pub struct FullGenerationRequest {
    position: ChunkPos,
    owner: FullGenerationOwner,
    task: u64,
}

/// Identity of one holder coordinator, including across world reopenings.
#[derive(Clone)]
pub struct LoadingHoldersOwner(Arc<()>);

impl FullGenerationRequest {
    pub fn position(&self) -> ChunkPos {
        self.position
    }
}

/// Live holder instances and their ordered block/entity ticking executor.
/// This coordinator is tied to one distance-manager driver and one world.
pub struct LoadingHolders {
    identity: DistanceDriverIdentity,
    owner: Arc<()>,
    holders: BTreeMap<ChunkPos, Holder>,
    positions: BTreeMap<u64, ChunkPos>,
    full_ids: FullFutureIds,
    preparations: TickingPreparations,
    links: EntityLoadingLinks,
    requests: VecDeque<FullGenerationRequest>,
    retired_blocks: BTreeMap<TickingRequestId, ChunkPos>,
    retired_entities: BTreeMap<EntityRequestId, ChunkPos>,
    required_full: Arc<BTreeSet<ChunkPos>>,
    required_full_dirty: bool,
    next_holder: u64,
    next_block: u64,
    next_entity: u64,
    next_task: u64,
}

impl LoadingHolders {
    pub fn new() -> Result<Self> {
        let preparations = TickingPreparations::with_entity_tracking();
        let links = EntityLoadingLinks::new(&preparations)?;
        Ok(Self {
            identity: DistanceDriverIdentity::default(),
            owner: Arc::new(()),
            holders: BTreeMap::new(),
            positions: BTreeMap::new(),
            full_ids: FullFutureIds::default(),
            preparations,
            links,
            requests: VecDeque::new(),
            retired_blocks: BTreeMap::new(),
            retired_entities: BTreeMap::new(),
            required_full: Arc::new(BTreeSet::new()),
            required_full_dirty: false,
            next_holder: 0,
            next_block: 0,
            next_entity: 0,
            next_task: 0,
        })
    }

    pub fn owner(&self) -> LoadingHoldersOwner {
        LoadingHoldersOwner(Arc::clone(&self.owner))
    }

    pub fn belongs_to_owner(&self, owner: &LoadingHoldersOwner) -> bool {
        Arc::ptr_eq(&self.owner, &owner.0)
    }

    /// Check a dispatched request without completing it. A demoted holder may
    /// no longer need a saved read, while a replacement holder cannot reuse it.
    pub fn generation_request_needed(&self, request: &FullGenerationRequest) -> Result<bool> {
        let holder = self
            .holders
            .get(&request.position)
            .context("FULL request holder no longer exists")?;
        ensure!(
            holder.generation_task
                == Some(GenerationTask {
                    id: request.task,
                    state: GenerationTaskState::Dispatched,
                })
                && holder.generation.owns(&request.owner),
            "FULL generation request is no longer current"
        );
        Ok(holder.generation.allows_full())
    }

    /// Chunks whose FULL data is still needed by a live holder or by an older
    /// queued/running callback. This is a retention and prefetch hint, not a
    /// statement that their futures have completed.
    pub fn required_full_positions(&mut self) -> &Arc<BTreeSet<ChunkPos>> {
        if self.required_full_dirty {
            let mut required = self.preparations.required_full_positions();
            required.extend(self.holders.iter().filter_map(|(&position, holder)| {
                (holder.generation.allows_full() && holder.generation.has_full_future())
                    .then_some(position)
            }));
            if self.required_full.as_ref() != &required {
                self.required_full = Arc::new(required);
            }
            self.required_full_dirty = false;
        }
        &self.required_full
    }

    /// Take one FULL request without waiting for generation or disk I/O.
    /// A request not yet dispatched is discarded if its holder was demoted.
    pub fn next_generation_request(&mut self) -> Option<FullGenerationRequest> {
        while let Some(request) = self.requests.pop_front() {
            let Some(holder) = self.holders.get_mut(&request.position) else {
                continue;
            };
            if holder.generation_task
                != Some(GenerationTask {
                    id: request.task,
                    state: GenerationTaskState::Queued,
                })
            {
                continue;
            }
            if !holder.generation.allows_full() {
                holder.generation_task = None;
                continue;
            }
            holder.generation_task = Some(GenerationTask {
                id: request.task,
                state: GenerationTaskState::Dispatched,
            });
            return Some(request);
        }
        None
    }

    /// Return a not-yet-completed request to the end of the worker queue.
    /// Callers should apply their own retry/backoff policy before doing this.
    pub fn defer_generation_request(&mut self, request: FullGenerationRequest) -> Result<()> {
        let holder = self
            .holders
            .get_mut(&request.position)
            .context("FULL request holder no longer exists")?;
        ensure!(
            holder.generation_task
                == Some(GenerationTask {
                    id: request.task,
                    state: GenerationTaskState::Dispatched,
                })
                && holder.generation.owns(&request.owner),
            "FULL generation request is no longer current"
        );
        holder.generation_task = Some(GenerationTask {
            id: request.task,
            state: GenerationTaskState::Queued,
        });
        self.requests.push_back(request);
        Ok(())
    }

    /// Drop an in-flight request only after its holder no longer permits FULL.
    /// A request still needed by a holder must instead be deferred for retry.
    pub fn discard_unneeded_generation_request(
        &mut self,
        request: FullGenerationRequest,
    ) -> Result<()> {
        let holder = self
            .holders
            .get_mut(&request.position)
            .context("FULL request holder no longer exists")?;
        ensure!(
            holder.generation_task
                == Some(GenerationTask {
                    id: request.task,
                    state: GenerationTaskState::Dispatched,
                })
                && holder.generation.owns(&request.owner)
                && !holder.generation.allows_full(),
            "FULL generation request is current or belongs to another holder"
        );
        holder.generation_task = None;
        Ok(())
    }

    /// Deliver one independently validated FULL result. An old task may finish
    /// after a level change, but it cannot complete a replacement holder.
    pub fn complete_full_generation(&mut self, request: FullGenerationRequest) -> Result<()> {
        let holder = self
            .holders
            .get_mut(&request.position)
            .context("FULL request holder no longer exists")?;
        ensure!(
            holder.generation_task
                == Some(GenerationTask {
                    id: request.task,
                    state: GenerationTaskState::Dispatched,
                }),
            "FULL generation request is no longer current"
        );
        let completion = holder
            .generation
            .complete_full(&request.owner, &mut self.full_ids)?;
        holder.generation_task = None;
        if let Some((id, result)) = completion {
            self.preparations.complete_if_registered(id, result)?;
        }
        Ok(())
    }

    pub fn block_state(&self, position: ChunkPos) -> Result<TickingFutureState> {
        match self.holders.get(&position) {
            Some(holder) => holder.block.state(&self.preparations),
            None => Ok(TickingFutureState::Unavailable),
        }
    }

    /// Return the last applied FULL result for this holder, if one exists.
    /// A waiting future or disallowed FULL status does not count as success.
    pub fn full_result(&self, position: ChunkPos) -> Option<FullFutureResult> {
        self.holders.get(&position)?.generation.full_result()
    }

    pub fn entity_state(&self, position: ChunkPos) -> Result<TickingFutureState> {
        match self.holders.get(&position) {
            Some(holder) => holder.entity.state(&self.preparations),
            None => Ok(TickingFutureState::Unavailable),
        }
    }

    /// Inspect the next event before taking it, so a blocked worker start does
    /// not discard the event or allow a later callback to overtake it.
    pub fn peek_executor_event(&self) -> Option<TickingExecutorEvent> {
        self.preparations.peek_executor_event()
    }

    /// The event loop must dispatch each block event at this executor position
    /// before taking later events; entity events go to `deliver_entity_event`.
    pub fn next_executor_event(&mut self) -> Option<TickingExecutorEvent> {
        self.preparations.next_executor_event()
    }

    pub fn preparations_mut(&mut self) -> &mut TickingPreparations {
        self.required_full_dirty = true;
        &mut self.preparations
    }

    pub fn deliver_entity_event(
        &mut self,
        player: &mut PlayerSimulation,
        listener: EntityListenerId,
        state: TickingFutureState,
    ) -> Result<()> {
        self.links
            .deliver(&self.preparations, player, listener, state)
    }

    /// Retire demoted requests only after their callbacks and listeners have
    /// drained. Completed FULL identities can then leave the executor cache.
    pub fn reap_retired(&mut self) {
        let before = self.retired_blocks.len() + self.retired_entities.len();
        self.retired_blocks
            .retain(|id, _| !matches!(self.preparations.retire(*id), Ok(true) | Ok(false)));
        self.retired_entities
            .retain(|id, _| !matches!(self.preparations.retire_entity(*id), Ok(true) | Ok(false)));
        if before != self.retired_blocks.len() + self.retired_entities.len() {
            self.preparations.prune_unused_full_futures();
            self.required_full_dirty = true;
        }
    }

    /// Forget a holder only after the world owner has finished its actual
    /// unload/save and all older callbacks and generation tasks have drained.
    pub fn retire_unloaded_holder(&mut self, position: ChunkPos) -> Result<bool> {
        self.reap_retired();
        let Some(holder) = self.holders.get(&position) else {
            return Ok(false);
        };
        ensure!(
            holder.level >= unloaded_level()
                && holder.generation_task.is_none()
                && holder.block.current_request().is_none()
                && holder.entity.current_request().is_none()
                && !self.retired_blocks.values().any(|&pos| pos == position)
                && !self.retired_entities.values().any(|&pos| pos == position)
                && !self.links.has_pending_at(position),
            "loading holder still has active work"
        );
        let identity = holder.identity;
        self.holders.remove(&position);
        self.positions.remove(&identity.id);
        self.required_full_dirty = true;
        self.reap_retired();
        Ok(true)
    }

    fn position(&self, identity: HolderIdentity) -> Result<ChunkPos> {
        let position = *self
            .positions
            .get(&identity.id)
            .context("loading holder identity is unknown")?;
        ensure!(
            self.holders.get(&position).map(|holder| holder.identity) == Some(identity),
            "loading holder identity changed"
        );
        Ok(position)
    }

    fn full_futures(
        &mut self,
        positions: impl IntoIterator<Item = ChunkPos>,
    ) -> Result<BTreeMap<ChunkPos, FullFuture>> {
        let mut futures = BTreeMap::new();
        for position in positions {
            let Some(holder) = self.holders.get_mut(&position) else {
                // Java stops requesting the range at the first missing holder.
                break;
            };
            let future = holder.generation.request_full(&mut self.full_ids)?;
            if future.result.is_none() && holder.generation_task.is_none() {
                let task = self.next_task;
                self.next_task = self
                    .next_task
                    .checked_add(1)
                    .context("FULL generation task identity exhausted")?;
                holder.generation_task = Some(GenerationTask {
                    id: task,
                    state: GenerationTaskState::Queued,
                });
                self.requests.push_back(FullGenerationRequest {
                    position,
                    owner: holder.generation.owner(),
                    task,
                });
            }
            futures.insert(position, future);
        }
        Ok(futures)
    }
}

impl DistanceUpdateDriver for LoadingHolders {
    fn identity(&self) -> &DistanceDriverIdentity {
        &self.identity
    }

    fn schedule(&mut self, change: LoadingLevelChange) -> Result<Option<HolderIdentity>> {
        if let Some(holder) = self.holders.get_mut(&change.position) {
            if holder.level == change.level {
                return Ok(None);
            }
            holder.level = change.level;
            holder.block.set_ticket_level(i32::from(change.level));
            holder.entity.set_ticket_level(i32::from(change.level));
            return Ok(Some(holder.identity));
        }
        if change.level >= unloaded_level() {
            return Ok(None);
        }
        let id = self.next_holder;
        self.next_holder = self
            .next_holder
            .checked_add(1)
            .context("loading holder identity exhausted")?;
        // Java's identity hash is process-specific. This stable native hash
        // preserves one ordering within this coordinator, not cross-JVM IDs.
        let identity = HolderIdentity {
            id,
            identity_hash: (id ^ (id >> 32)) as u32,
        };
        let level = i32::from(change.level);
        self.holders.insert(
            change.position,
            Holder {
                identity,
                level: change.level,
                generation: FullGenerationHolder::default(),
                block: TickingHolder::new(change.position, level, &self.preparations),
                entity: EntityTickingHolder::new(change.position, level, &self.preparations),
                generation_task: None,
            },
        );
        self.positions.insert(id, change.position);
        Ok(Some(identity))
    }

    fn update_highest_allowed_status(&mut self, identity: HolderIdentity) -> Result<()> {
        let position = self.position(identity)?;
        self.required_full_dirty = true;
        let holder = self.holders.get_mut(&position).expect("scheduled holder");
        if let Some((id, result)) = holder
            .generation
            .update_highest_allowed_status(i32::from(holder.level))
        {
            self.preparations.complete_if_registered(id, result)?;
        }
        Ok(())
    }

    fn update_futures(&mut self, identity: HolderIdentity) -> Result<()> {
        let position = self.position(identity)?;
        self.required_full_dirty = true;
        let block_needed = self.holders[&position].block.needs_new_ticking_request();
        let block_futures = if block_needed {
            self.full_futures(ticking_dependencies(position))?
        } else {
            BTreeMap::new()
        };
        let block_id = if block_needed {
            let id = self.next_block;
            self.next_block = self
                .next_block
                .checked_add(1)
                .context("block request exhausted")?;
            Some(TickingRequestId(id))
        } else {
            None
        };
        let block_transition = self
            .holders
            .get_mut(&position)
            .expect("scheduled holder")
            .block
            .update(&mut self.preparations, block_id, |neighbor| {
                block_futures.get(&neighbor).copied()
            })?;
        if let TickingHolderTransition::Demoted { previous, .. } = block_transition {
            self.retired_blocks.insert(previous, position);
        }

        let entity_needed = self.holders[&position].entity.needs_new_entity_request();
        let entity_futures = if entity_needed {
            self.full_futures(entity_ticking_dependencies(position))?
        } else {
            BTreeMap::new()
        };
        let entity_id = if entity_needed {
            let id = self.next_entity;
            self.next_entity = self
                .next_entity
                .checked_add(1)
                .context("entity request exhausted")?;
            Some(EntityRequestId(id))
        } else {
            None
        };
        let entity_transition = self
            .holders
            .get_mut(&position)
            .expect("scheduled holder")
            .entity
            .update(&mut self.preparations, entity_id, |neighbor| {
                entity_futures.get(&neighbor).copied()
            })?;
        if let EntityTickingTransition::Demoted { previous, .. } = entity_transition {
            self.retired_entities.insert(previous, position);
        }
        Ok(())
    }

    fn register_player_loading_release(&mut self, token: PlayerLoadingRelease) -> Result<()> {
        let holder = self
            .holders
            .get(&token.position())
            .context("player-loading holder is missing")?;
        self.links
            .register(&mut self.preparations, &holder.entity, token)
    }
}

#[cfg(test)]
#[path = "loading_holders_tests.rs"]
mod tests;
