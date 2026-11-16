//! Player-loading ticket requests and the 26.3 throttled dispatcher.
//!
//! The caller controls the dispatcher and main-executor steps, and supplies
//! actual entity-ticking future completions. This does not manufacture chunk
//! holders, generation futures, or completion events from loaded terrain.

use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    sync::Arc,
};

use native_persistence::chunk_tickets::TicketKind;
use native_world::loaded::ChunkPos;

use crate::{
    loading_chunks::unloaded_level, simulation_chunks::SimulationChunks,
    simulation_tickets::SimulationTickets,
};

const MAX_PLAYER_DISTANCE: u8 = 32;
const ABSENT_PLAYER_DISTANCE: u8 = 34;
const MAX_IN_FLIGHT: usize = 4;
const PLAYER_SOURCE: u64 = 0;

fn pack(position: ChunkPos) -> u64 {
    u64::from(position.x as u32) | (u64::from(position.z as u32) << 32)
}

fn unpack(key: u64) -> ChunkPos {
    ChunkPos {
        x: key as u32 as i32,
        z: (key >> 32) as u32 as i32,
    }
}

fn mix(key: u64) -> u64 {
    let hash = key.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let hash = hash ^ (hash >> 32);
    hash ^ (hash >> 16)
}

/// Insertion and iteration of fastutil's default LongOpenHashSet. Clear keeps
/// its table capacity, which affects the order of later player movements.
#[derive(Clone, Debug)]
struct OrderedLongSet {
    slots: Vec<Option<u64>>,
    zero: bool,
    len: usize,
}

impl Default for OrderedLongSet {
    fn default() -> Self {
        Self {
            slots: vec![None; 32],
            zero: false,
            len: 0,
        }
    }
}

impl OrderedLongSet {
    fn slot(&self, key: u64) -> usize {
        let mask = self.slots.len() - 1;
        let mut index = mix(key) as usize & mask;
        while self.slots[index].is_some_and(|present| present != key) {
            index = (index + 1) & mask;
        }
        index
    }

    fn insert(&mut self, key: u64) {
        if key == 0 {
            if self.zero {
                return;
            }
            self.zero = true;
        } else {
            let index = self.slot(key);
            if self.slots[index].is_some() {
                return;
            }
            self.slots[index] = Some(key);
        }
        let previous = self.len;
        self.len += 1;
        if previous >= self.slots.len() * 3 / 4 {
            let size = self.slots.len() * 2;
            let old = std::mem::replace(&mut self.slots, vec![None; size]);
            for key in old.into_iter().rev().flatten() {
                let index = self.slot(key);
                self.slots[index] = Some(key);
            }
        }
    }

    fn ordered(&self) -> impl Iterator<Item = u64> + '_ {
        self.zero
            .then_some(0)
            .into_iter()
            .chain(self.slots.iter().rev().flatten().copied())
    }

    fn take_all(&mut self) -> Vec<u64> {
        let result = self.ordered().collect();
        self.clear();
        result
    }

    fn clear(&mut self) {
        self.slots.fill(None);
        self.zero = false;
        self.len = 0;
    }
}

#[derive(Clone, Debug)]
struct QueuedChunk {
    previous: Option<u64>,
    next: Option<u64>,
    tasks: usize,
}

/// A priority bucket preserves insertion order while allowing constant-time
/// cancellation and movement between priorities.
#[derive(Clone, Debug, Default)]
struct PriorityBucket {
    nodes: HashMap<u64, QueuedChunk>,
    first: Option<u64>,
    last: Option<u64>,
}

impl PriorityBucket {
    fn append(&mut self, key: u64, tasks: usize) {
        if let Some(existing) = self.nodes.get_mut(&key) {
            existing.tasks += tasks;
            return;
        }
        if let Some(last) = self.last {
            self.nodes.get_mut(&last).expect("last queued chunk").next = Some(key);
        } else {
            self.first = Some(key);
        }
        self.nodes.insert(
            key,
            QueuedChunk {
                previous: self.last,
                next: None,
                tasks,
            },
        );
        self.last = Some(key);
    }

    fn remove(&mut self, key: u64) -> Option<usize> {
        let node = self.nodes.remove(&key)?;
        if let Some(previous) = node.previous {
            self.nodes
                .get_mut(&previous)
                .expect("previous queued chunk")
                .next = node.next;
        } else {
            self.first = node.next;
        }
        if let Some(next) = node.next {
            self.nodes
                .get_mut(&next)
                .expect("next queued chunk")
                .previous = node.previous;
        } else {
            self.last = node.previous;
        }
        Some(node.tasks)
    }

    fn pop(&mut self) -> Option<(u64, usize)> {
        let first = self.first?;
        Some((first, self.remove(first).expect("first queued chunk")))
    }
}

#[derive(Clone, Debug)]
enum DispatcherTask {
    Reprioritize {
        key: u64,
        level: u8,
    },
    Release {
        key: u64,
        clear: bool,
        remove_ticket: bool,
    },
    Submit {
        key: u64,
        priority: u8,
    },
    Poll,
}

#[derive(Clone, Debug)]
enum MainTask {
    Request { key: u64, batch: u64 },
    AddTicket(u64),
    RemoveTicket(u64),
    FutureCompleted(u64),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlayerLoadingTicketChange {
    pub position: ChunkPos,
    pub added: bool,
    pub changed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlayerLoadingFutureOutcome {
    /// The future completed with a result, including an unloaded result.
    Completed,
    /// Exceptional completion does not run Java's release callback.
    Exceptional,
}

/// A single registration against one player's holder-future executor.
#[derive(Debug)]
pub struct PlayerLoadingRelease {
    identity: Arc<()>,
    id: u64,
    position: ChunkPos,
}

impl PlayerLoadingRelease {
    pub fn position(&self) -> ChunkPos {
        self.position
    }
}

#[derive(Debug)]
pub(crate) struct PlayerLoading {
    identity: Arc<()>,
    distance: SimulationChunks,
    players: BTreeSet<ChunkPos>,
    updated: OrderedLongSet,
    view_distance: u8,
    queue_levels: HashMap<u64, u8>,
    commands: [VecDeque<DispatcherTask>; 4],
    priorities: Vec<PriorityBucket>,
    sleeping: bool,
    acquired: BTreeSet<ChunkPos>,
    main: VecDeque<MainTask>,
    batches: HashMap<u64, usize>,
    next_batch: u64,
    pending_releases: OrderedLongSet,
    registered_releases: HashMap<u64, ChunkPos>,
    next_release: u64,
}

impl Default for PlayerLoading {
    fn default() -> Self {
        Self {
            identity: Arc::new(()),
            distance: SimulationChunks::with_max_level(MAX_PLAYER_DISTANCE + 1),
            players: BTreeSet::new(),
            updated: OrderedLongSet::default(),
            view_distance: 0,
            queue_levels: HashMap::new(),
            commands: std::array::from_fn(|_| VecDeque::new()),
            priorities: vec![PriorityBucket::default(); usize::from(unloaded_level()) + 1],
            sleeping: true,
            acquired: BTreeSet::new(),
            main: VecDeque::new(),
            batches: HashMap::new(),
            next_batch: 0,
            pending_releases: OrderedLongSet::default(),
            registered_releases: HashMap::new(),
            next_release: 0,
        }
    }
}

impl PlayerLoading {
    pub(crate) fn identity(&self) -> &Arc<()> {
        &self.identity
    }

    pub fn add_player(&mut self, position: ChunkPos) {
        if self.players.insert(position) {
            self.distance.add_ticket(position, PLAYER_SOURCE, 0);
        }
    }

    pub fn remove_player(&mut self, position: ChunkPos) {
        if self.players.remove(&position) {
            self.distance.remove_ticket(position, PLAYER_SOURCE, 0);
        }
    }

    pub fn has_pending_updates(&self) -> bool {
        self.distance.has_pending_updates()
    }

    fn distance_at(&self, key: u64) -> u8 {
        let level = self.distance.level(unpack(key));
        if level > MAX_PLAYER_DISTANCE {
            ABSENT_PLAYER_DISTANCE
        } else {
            level
        }
    }

    pub fn set_view_distance(&mut self, distance: u8) {
        assert!(distance <= MAX_PLAYER_DISTANCE);
        let changes: Vec<_> = self.distance.tracked_levels().collect();
        for (position, level) in changes {
            self.ticket_transition(
                pack(position),
                level,
                level <= self.view_distance,
                level <= distance,
            );
        }
        self.view_distance = distance;
    }

    pub fn run_updates(&mut self) {
        let updated = &mut self.updated;
        self.distance
            .run_updates_budget_observed(usize::MAX, |position, _, _| {
                updated.insert(pack(position))
            });
        for key in self.updated.take_all() {
            let old = self
                .queue_levels
                .get(&key)
                .copied()
                .unwrap_or(ABSENT_PLAYER_DISTANCE);
            let next = self.distance_at(key);
            if old == next {
                continue;
            }
            self.commands[0].push_back(DispatcherTask::Reprioritize { key, level: next });
            self.ticket_transition(
                key,
                next,
                old <= self.view_distance,
                next <= self.view_distance,
            );
        }
    }

    fn ticket_transition(&mut self, key: u64, level: u8, before: bool, after: bool) {
        match (before, after) {
            (false, true) => self.commands[2].push_back(DispatcherTask::Submit {
                key,
                priority: level,
            }),
            (true, false) => self.commands[1].push_back(DispatcherTask::Release {
                key,
                clear: true,
                remove_ticket: true,
            }),
            _ => {}
        }
    }

    /// Run one task from the priority-ordered dispatcher executor.
    pub fn step_dispatcher(&mut self) -> bool {
        let Some(task) = self.commands.iter_mut().find_map(VecDeque::pop_front) else {
            return false;
        };
        match task {
            DispatcherTask::Reprioritize { key, level } => {
                let old = self
                    .queue_levels
                    .get(&key)
                    .copied()
                    .unwrap_or(ABSENT_PLAYER_DISTANCE);
                if let Some(bucket) = self.priorities.get_mut(usize::from(old))
                    && let Some(tasks) = bucket.remove(key)
                {
                    self.priorities[usize::from(level)].append(key, tasks);
                }
                if level >= ABSENT_PLAYER_DISTANCE {
                    self.queue_levels.remove(&key);
                } else {
                    self.queue_levels.insert(key, level);
                }
            }
            DispatcherTask::Submit { key, priority } => {
                self.priorities[usize::from(priority)].append(key, 1);
                self.wake();
            }
            DispatcherTask::Release {
                key,
                clear,
                remove_ticket,
            } => {
                if clear {
                    for bucket in &mut self.priorities {
                        bucket.remove(key);
                    }
                }
                self.acquired.remove(&unpack(key));
                self.wake();
                if remove_ticket {
                    self.main.push_back(MainTask::RemoveTicket(key));
                }
            }
            DispatcherTask::Poll => {
                let next = if self.acquired.len() < MAX_IN_FLIGHT {
                    self.priorities.iter_mut().find_map(PriorityBucket::pop)
                } else {
                    None
                };
                if let Some((key, count)) = next {
                    self.acquired.insert(unpack(key));
                    let batch = self.next_batch;
                    self.next_batch = self
                        .next_batch
                        .checked_add(1)
                        .expect("ticket batch overflow");
                    self.batches.insert(batch, count);
                    for _ in 0..count {
                        self.main.push_back(MainTask::Request { key, batch });
                    }
                } else {
                    self.sleeping = true;
                }
            }
        }
        true
    }

    fn wake(&mut self) {
        if self.sleeping {
            self.sleeping = false;
            self.commands[3].push_back(DispatcherTask::Poll);
        }
    }

    /// Run one queued main-executor task against this player's ticket store.
    pub fn step_main(
        &mut self,
        tickets: &mut SimulationTickets,
        mut observe: impl FnMut(PlayerLoadingTicketChange),
    ) -> bool {
        let Some(task) = self.main.pop_front() else {
            return false;
        };
        match task {
            MainTask::Request { key, batch } => {
                self.main.push_back(MainTask::AddTicket(key));
                let remaining = self.batches.get_mut(&batch).expect("ticket batch exists");
                *remaining -= 1;
                if *remaining == 0 {
                    self.batches.remove(&batch);
                    self.commands[3].push_back(DispatcherTask::Poll);
                }
            }
            MainTask::AddTicket(key) => {
                if self.distance_at(key) <= self.view_distance {
                    let position = unpack(key);
                    let changed = tickets.add_ticket(position, TicketKind::PlayerLoading, 31);
                    self.pending_releases.insert(key);
                    observe(PlayerLoadingTicketChange {
                        position,
                        added: true,
                        changed,
                    });
                } else {
                    self.commands[1].push_back(DispatcherTask::Release {
                        key,
                        clear: false,
                        remove_ticket: false,
                    });
                }
            }
            MainTask::RemoveTicket(key) => {
                let position = unpack(key);
                let changed = tickets.remove_ticket(position, TicketKind::PlayerLoading, 31);
                observe(PlayerLoadingTicketChange {
                    position,
                    added: false,
                    changed,
                });
            }
            MainTask::FutureCompleted(key) => self.commands[1].push_back(DispatcherTask::Release {
                key,
                clear: false,
                remove_ticket: false,
            }),
        }
        true
    }

    /// Attach still-active loading tickets to actual holder entity-ticking
    /// futures. A failed registration keeps the pending set, as in Java.
    pub fn register_releases(
        &mut self,
        tickets: &SimulationTickets,
        mut register: impl FnMut(PlayerLoadingRelease) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        for key in self.pending_releases.ordered() {
            let position = unpack(key);
            if tickets.has_ticket_kind(position, TicketKind::PlayerLoading) {
                let id = self.next_release;
                self.next_release = id
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("player loading registration overflow"))?;
                self.registered_releases.insert(id, position);
                let token = PlayerLoadingRelease {
                    identity: Arc::clone(&self.identity),
                    id,
                    position,
                };
                if let Err(error) = register(token) {
                    self.registered_releases.remove(&id);
                    return Err(error);
                }
            }
        }
        self.pending_releases.clear();
        Ok(())
    }

    pub fn entity_future_completed(
        &mut self,
        token: &PlayerLoadingRelease,
        outcome: PlayerLoadingFutureOutcome,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            Arc::ptr_eq(&self.identity, &token.identity)
                && self.registered_releases.get(&token.id) == Some(&token.position),
            "player loading future registration is stale or belongs to another session"
        );
        self.registered_releases.remove(&token.id);
        if outcome == PlayerLoadingFutureOutcome::Completed {
            self.main
                .push_back(MainTask::FutureCompleted(pack(token.position)));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "player_loading_tests.rs"]
mod tests;
