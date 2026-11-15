//! Simulation ticket propagation and iteration order used by Java 26.3.
//!
//! A ticket controls simulation independently of the render window. The
//! resulting order is observable because block ticks share a level RNG. This
//! tracker models the ticket graph only; the caller must still check whether
//! a chunk has reached the required status before ticking it.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use native_world::loaded::ChunkPos;

const MAX_LEVEL: u8 = 33;
const ENTITY_TICKING_LEVEL: u8 = 31;
const BLOCK_TICKING_LEVEL: u8 = 32;
const SOURCE_COORDINATE: i32 = 1_875_066;
const SOURCE: u64 = pack(ChunkPos {
    x: SOURCE_COORDINATE,
    z: SOURCE_COORDINATE,
});

const fn pack(pos: ChunkPos) -> u64 {
    (pos.x as u32 as u64) | ((pos.z as u32 as u64) << 32)
}

const fn unpack(key: u64) -> ChunkPos {
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

/// The insertion, removal and traversal behavior of Java's default fastutil
/// Long2ByteOpenHashMap. Iteration follows the table, not coordinate order.
#[derive(Clone, Debug)]
struct Levels {
    slots: Vec<Option<(u64, u8)>>,
    zero: Option<u8>,
    len: usize,
}

impl Default for Levels {
    fn default() -> Self {
        Self {
            slots: vec![None; 32],
            zero: None,
            len: 0,
        }
    }
}

impl Levels {
    fn find(&self, key: u64) -> usize {
        let mask = self.slots.len() - 1;
        let mut slot = mix(key) as usize & mask;
        while self.slots[slot].is_some_and(|(other, _)| other != key) {
            slot = (slot + 1) & mask;
        }
        slot
    }

    fn get(&self, key: u64) -> u8 {
        if key == 0 {
            self.zero.unwrap_or(MAX_LEVEL)
        } else {
            self.slots[self.find(key)].map_or(MAX_LEVEL, |(_, level)| level)
        }
    }

    fn set(&mut self, key: u64, level: u8) {
        if level >= MAX_LEVEL {
            self.remove(key);
            return;
        }
        if key == 0 {
            if self.zero.replace(level).is_some() {
                return;
            }
        } else {
            let slot = self.find(key);
            if let Some(entry) = &mut self.slots[slot] {
                entry.1 = level;
                return;
            }
            self.slots[slot] = Some((key, level));
        }
        let previous = self.len;
        self.len += 1;
        if previous >= self.slots.len() * 3 / 4 {
            self.rehash(self.slots.len() * 2);
        }
    }

    fn remove(&mut self, key: u64) {
        if key == 0 {
            if self.zero.take().is_none() {
                return;
            }
        } else {
            let mut hole = self.find(key);
            if self.slots[hole].is_none() {
                return;
            }
            let mask = self.slots.len() - 1;
            let mut cursor = (hole + 1) & mask;
            while let Some(entry) = self.slots[cursor] {
                let home = mix(entry.0) as usize & mask;
                let move_entry = if hole <= cursor {
                    hole >= home || home > cursor
                } else {
                    hole >= home && home > cursor
                };
                if move_entry {
                    self.slots[hole] = Some(entry);
                    hole = cursor;
                }
                cursor = (cursor + 1) & mask;
            }
            self.slots[hole] = None;
        }
        self.len -= 1;
        if self.slots.len() > 32 && self.len < (self.slots.len() * 3 / 4) / 4 {
            self.rehash(self.slots.len() / 2);
        }
    }

    fn rehash(&mut self, size: usize) {
        let previous = std::mem::replace(&mut self.slots, vec![None; size]);
        for entry in previous.into_iter().rev().flatten() {
            let slot = self.find(entry.0);
            self.slots[slot] = Some(entry);
        }
    }

    fn iter(&self) -> impl Iterator<Item = (u64, u8)> + '_ {
        self.zero
            .map(|level| (0, level))
            .into_iter()
            .chain(self.slots.iter().rev().filter_map(|entry| *entry))
    }
}

#[derive(Clone, Copy, Debug)]
struct Pending {
    level: u8,
    priority: u8,
    serial: u64,
}

/// Tracks simulation tickets. Ticket kinds are caller-provided stable IDs;
/// tickets with the same kind and level at one position are deduplicated.
#[derive(Clone, Debug)]
pub struct SimulationChunks {
    sources: BTreeMap<u64, BTreeSet<(u64, i32)>>,
    levels: Levels,
    pending: HashMap<u64, Pending>,
    queues: [VecDeque<(u64, u64)>; MAX_LEVEL as usize + 1],
    next_serial: u64,
}

impl Default for SimulationChunks {
    fn default() -> Self {
        Self {
            sources: BTreeMap::new(),
            levels: Levels::default(),
            pending: HashMap::new(),
            queues: std::array::from_fn(|_| VecDeque::new()),
            next_serial: 0,
        }
    }
}

impl SimulationChunks {
    /// The player simulation ticket has entity-ticking level 31 minus the
    /// simulation distance, capped at zero by Java's DistanceManager.
    pub fn player_ticket_level(distance: u32) -> i32 {
        ENTITY_TICKING_LEVEL.saturating_sub(distance.min(u32::from(u8::MAX)) as u8) as i32
    }

    /// Add a ticket and schedule propagation. Returns false for a duplicate.
    pub fn add_ticket(&mut self, pos: ChunkPos, kind: u64, level: i32) -> bool {
        let key = pack(pos);
        let before = self.source_level(key);
        if !self.sources.entry(key).or_default().insert((kind, level)) {
            return false;
        }
        if level < before {
            self.check_edge(SOURCE, key, level, true);
        }
        true
    }

    /// Activate a decoded ticket store in Java's order. This only installs
    /// initial simulation tickets; callers must manage their timeouts, chunk
    /// loading, and subsequent ticket changes before advancing game ticks.
    pub fn activate_saved_tickets(
        &mut self,
        tickets: &native_persistence::chunk_tickets::SavedChunkTickets,
    ) {
        for ticket in tickets.activation_order() {
            if ticket.kind.simulates() {
                self.add_ticket(
                    ChunkPos {
                        x: ticket.position.x,
                        z: ticket.position.z,
                    },
                    ticket.kind.simulation_key(),
                    ticket.level,
                );
            }
        }
    }

    /// Apply a locked world's captured tickets only if its epoch and
    /// dimension still match; a failed check leaves the tracker unchanged.
    pub fn activate_captured_tickets(
        &mut self,
        captured: &native_persistence::world::CapturedChunkTickets,
        session: &native_persistence::save::WorldSessionGuard,
        dimension: &native_persistence::world::DimensionId,
    ) -> anyhow::Result<()> {
        let _lease = captured.lock_current_for(session, dimension)?;
        self.activate_saved_tickets(captured.tickets());
        Ok(())
    }

    /// Remove an existing ticket and schedule propagation. Java notifies the
    /// graph even when a different ticket still supplies the same level.
    pub fn remove_ticket(&mut self, pos: ChunkPos, kind: u64, level: i32) -> bool {
        let key = pack(pos);
        let Some(tickets) = self.sources.get_mut(&key) else {
            return false;
        };
        if !tickets.remove(&(kind, level)) {
            return false;
        }
        if tickets.is_empty() {
            self.sources.remove(&key);
        }
        self.check_edge(SOURCE, key, self.source_level(key), false);
        true
    }

    /// TicketStorage reports a single graph update after removing all stale
    /// simulation tickets in the same chunk.
    pub(crate) fn remove_expired_tickets(&mut self, pos: ChunkPos, tickets: &[(u64, i32)]) {
        let key = pack(pos);
        let Some(sources) = self.sources.get_mut(&key) else {
            debug_assert!(tickets.is_empty(), "simulation ticket source missing");
            return;
        };
        let mut removed = false;
        for ticket in tickets {
            let existed = sources.remove(ticket);
            debug_assert!(existed, "simulation ticket source missing");
            removed |= existed;
        }
        if sources.is_empty() {
            self.sources.remove(&key);
        }
        if removed {
            self.check_edge(SOURCE, key, self.source_level(key), false);
        }
    }

    /// Apply all pending changes. Call after all ticket changes for a tick,
    /// before selecting chunks, to preserve Java's propagation history.
    pub fn run_updates(&mut self) {
        self.run_updates_budget(usize::MAX);
    }

    /// Process at most `budget` graph nodes and return the unused budget.
    /// The remaining work can be resumed without changing its order.
    pub fn run_updates_budget(&mut self, budget: usize) -> usize {
        let mut remaining = budget;
        while remaining > 0 {
            let Some((key, next)) = self.pop() else {
                break;
            };
            remaining -= 1;
            let previous = self.levels.get(key);
            if next < previous {
                self.levels.set(key, next);
                self.check_neighbors(key, next, true);
            } else if next > previous {
                self.levels.set(key, MAX_LEVEL);
                if next != MAX_LEVEL {
                    self.enqueue(key, next, next);
                }
                self.check_neighbors(key, previous, false);
            }
        }
        remaining
    }

    pub fn has_pending_updates(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn level(&self, pos: ChunkPos) -> u8 {
        self.levels.get(pack(pos))
    }

    pub fn in_entity_ticking_range(&self, pos: ChunkPos) -> bool {
        self.level(pos) <= ENTITY_TICKING_LEVEL
    }

    pub fn in_block_ticking_range(&self, pos: ChunkPos) -> bool {
        self.level(pos) <= BLOCK_TICKING_LEVEL
    }

    /// Java's `ChunkMap.forEachBlockTickingChunk` starts with this ordered
    /// entity-ticking iteration and then checks each holder's ticking status.
    pub fn entity_ticking_order(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.levels
            .iter()
            .filter_map(|(key, level)| (level <= ENTITY_TICKING_LEVEL).then(|| unpack(key)))
    }

    /// Missing FULL chunks in the 3×3 dependency area around each entity-
    /// ticking candidate. Java also requires the center's postprocessing and
    /// ticking future to finish; this list alone does not prove readiness.
    /// Return `None` while ticket changes are still propagating.
    pub fn missing_full_tick_dependencies(
        &self,
        mut is_full: impl FnMut(ChunkPos) -> bool,
    ) -> Option<Vec<ChunkPos>> {
        if self.has_pending_updates() {
            return None;
        }
        let mut seen = BTreeSet::new();
        let mut missing = Vec::new();
        for center in self.entity_ticking_order() {
            for dx in -1_i32..=1 {
                for dz in -1_i32..=1 {
                    let position = ChunkPos {
                        x: center.x.wrapping_add(dx),
                        z: center.z.wrapping_add(dz),
                    };
                    if seen.insert(position) && !is_full(position) {
                        missing.push(position);
                    }
                }
            }
        }
        Some(missing)
    }

    fn source_level(&self, key: u64) -> i32 {
        self.sources
            .get(&key)
            .and_then(|tickets| tickets.iter().map(|(_, level)| *level).min())
            .unwrap_or(i32::MAX)
    }

    fn computed_level(&self, key: u64, excluded: u64, initial: u8) -> u8 {
        let pos = unpack(key);
        let mut result = initial;
        for dx in -1_i32..=1 {
            for dz in -1_i32..=1 {
                let neighbor = pack(ChunkPos {
                    x: pos.x.wrapping_add(dx),
                    z: pos.z.wrapping_add(dz),
                });
                let neighbor = if neighbor == key { SOURCE } else { neighbor };
                if neighbor == excluded {
                    continue;
                }
                let candidate = if neighbor == SOURCE {
                    self.source_level(key).clamp(0, i32::from(MAX_LEVEL)) as u8
                } else {
                    self.levels.get(neighbor).saturating_add(1).min(MAX_LEVEL)
                };
                result = result.min(candidate);
                if result == 0 {
                    return result;
                }
            }
        }
        result
    }

    fn check_edge(&mut self, source: u64, target: u64, level: i32, decreasing: bool) {
        if target == SOURCE {
            return;
        }
        let level = level.clamp(0, i32::from(MAX_LEVEL)) as u8;
        let current = self.levels.get(target);
        let old = self.pending.get(&target).copied();
        let computed = old.map_or(current, |pending| pending.level);
        let next = if decreasing {
            computed.min(level)
        } else {
            self.computed_level(target, source, level)
        };
        if current != next {
            self.enqueue(target, next, current.min(next));
        } else if old.is_some() {
            self.pending.remove(&target);
        }
    }

    fn enqueue(&mut self, key: u64, level: u8, priority: u8) {
        if let Some(pending) = self.pending.get_mut(&key)
            && pending.priority == priority
        {
            pending.level = level;
            return;
        }
        let serial = self.next_serial;
        self.next_serial = self.next_serial.wrapping_add(1);
        self.pending.insert(
            key,
            Pending {
                level,
                priority,
                serial,
            },
        );
        self.queues[usize::from(priority)].push_back((key, serial));
    }

    fn pop(&mut self) -> Option<(u64, u8)> {
        for queue in &mut self.queues {
            while let Some((key, serial)) = queue.pop_front() {
                if self
                    .pending
                    .get(&key)
                    .is_some_and(|item| item.serial == serial)
                {
                    return self.pending.remove(&key).map(|item| (key, item.level));
                }
            }
        }
        None
    }

    fn check_neighbors(&mut self, key: u64, level: u8, decreasing: bool) {
        if decreasing && level >= MAX_LEVEL - 1 {
            return;
        }
        let pos = unpack(key);
        for dx in -1_i32..=1 {
            for dz in -1_i32..=1 {
                let target = pack(ChunkPos {
                    x: pos.x.wrapping_add(dx),
                    z: pos.z.wrapping_add(dz),
                });
                if target == key {
                    continue;
                }
                let candidate = level.saturating_add(1).min(MAX_LEVEL);
                if decreasing {
                    self.check_edge(key, target, i32::from(candidate), true);
                } else {
                    let observed = self
                        .pending
                        .get(&target)
                        .map_or_else(|| self.levels.get(target), |pending| pending.level);
                    if candidate == observed {
                        self.check_edge(key, target, i32::from(MAX_LEVEL), false);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_ticket_propagates_separate_entity_and_block_ranges() {
        let mut tracker = SimulationChunks::default();
        let center = ChunkPos { x: -3, z: 7 };
        assert!(tracker.add_ticket(center, 1, SimulationChunks::player_ticket_level(5)));
        tracker.run_updates();
        assert!(tracker.in_entity_ticking_range(ChunkPos { x: 2, z: 12 }));
        assert!(!tracker.in_entity_ticking_range(ChunkPos { x: 3, z: 12 }));
        assert!(tracker.in_block_ticking_range(ChunkPos { x: 3, z: 13 }));
        assert!(!tracker.in_block_ticking_range(ChunkPos { x: 4, z: 13 }));
        assert_eq!(tracker.entity_ticking_order().count(), 121);
        assert!(tracker.remove_ticket(center, 1, SimulationChunks::player_ticket_level(5)));
        tracker.run_updates();
        assert_eq!(tracker.entity_ticking_order().count(), 0);
    }

    #[test]
    fn ticking_dependencies_include_one_full_chunk_halo() {
        let mut tracker = SimulationChunks::default();
        let center = ChunkPos { x: -2, z: 3 };
        tracker.add_ticket(center, 1, SimulationChunks::player_ticket_level(5));
        assert!(tracker.missing_full_tick_dependencies(|_| false).is_none());
        tracker.run_updates();
        let mut observed = BTreeSet::new();
        let missing = tracker
            .missing_full_tick_dependencies(|pos| {
                assert!(observed.insert(pos), "dependency was queried twice");
                pos == center
            })
            .unwrap();
        assert_eq!(observed.len(), 13 * 13);
        assert_eq!(missing.len(), 13 * 13 - 1);
        assert!(!missing.contains(&center));
        assert!(missing.contains(&ChunkPos { x: -8, z: 9 }));
        assert!(!observed.contains(&ChunkPos { x: -9, z: 9 }));
    }
}
