//! Live Java 26.3 chunk-ticket storage and simulation-level updates.
//!
//! Ticket expiration is driven by the server tick and by the actual chunk
//! holder's save readiness, not by rendering or wall-clock time. The caller
//! must provide a current readiness observation for every queried chunk.

use native_persistence::chunk_tickets::{SavedChunkTicket, SavedChunkTickets, TicketKind};
use native_world::loaded::ChunkPos;

use crate::simulation_chunks::SimulationChunks;

/// The state consulted by Java's `TicketStorage.canTicketExpire`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChunkSaveReadiness {
    /// No updating chunk holder exists.
    Absent,
    /// A holder exists but its save future is not ready.
    Pending,
    /// The holder's save future is ready.
    Ready,
}

fn pack(pos: ChunkPos) -> u64 {
    u64::from(pos.x as u32) | (u64::from(pos.z as u32) << 32)
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

/// The active store uses fastutil's Long2ObjectOpenHashMap with expected size
/// four. Its iteration and iterator-removal behavior affect graph update order.
#[derive(Clone, Debug)]
struct Buckets {
    slots: Vec<Option<(u64, Vec<SavedChunkTicket>)>>,
    zero: Option<Vec<SavedChunkTicket>>,
    len: usize,
}

impl Default for Buckets {
    fn default() -> Self {
        Self {
            slots: vec![None; 8],
            zero: None,
            len: 0,
        }
    }
}

impl Buckets {
    fn find(&self, key: u64) -> usize {
        let mask = self.slots.len() - 1;
        let mut index = mix(key) as usize & mask;
        while self.slots[index]
            .as_ref()
            .is_some_and(|(other, _)| *other != key)
        {
            index = (index + 1) & mask;
        }
        index
    }

    fn get_mut(&mut self, key: u64) -> Option<&mut Vec<SavedChunkTicket>> {
        if key == 0 {
            self.zero.as_mut()
        } else {
            let index = self.find(key);
            self.slots[index].as_mut().map(|(_, tickets)| tickets)
        }
    }

    fn get_or_insert(&mut self, key: u64) -> &mut Vec<SavedChunkTicket> {
        let exists = if key == 0 {
            self.zero.is_some()
        } else {
            self.slots[self.find(key)].is_some()
        };
        if !exists {
            if key == 0 {
                self.zero = Some(Vec::new());
            } else {
                let index = self.find(key);
                self.slots[index] = Some((key, Vec::new()));
            }
            let previous = self.len;
            self.len += 1;
            if previous >= self.slots.len() * 3 / 4 {
                self.rehash(self.slots.len() * 2);
            }
        }
        self.get_mut(key).expect("ticket bucket was inserted")
    }

    fn rehash(&mut self, size: usize) {
        let old = std::mem::replace(&mut self.slots, vec![None; size]);
        for entry in old.into_iter().rev().flatten() {
            let index = self.find(entry.0);
            self.slots[index] = Some(entry);
        }
    }

    fn shift_keys(&mut self, mut hole: usize, mut wrapped: Option<&mut Vec<u64>>) {
        let mask = self.slots.len() - 1;
        let mut cursor = (hole + 1) & mask;
        while let Some((key, _)) = &self.slots[cursor] {
            let home = mix(*key) as usize & mask;
            let move_entry = if hole <= cursor {
                hole >= home || home > cursor
            } else {
                hole >= home && home > cursor
            };
            if move_entry {
                if cursor < hole
                    && let Some(wrapped) = &mut wrapped
                {
                    wrapped.push(*key);
                }
                self.slots[hole] = self.slots[cursor].take();
                hole = cursor;
            }
            cursor = (cursor + 1) & mask;
        }
        self.slots[hole] = None;
    }

    fn remove_bucket(&mut self, key: u64) -> bool {
        if key == 0 {
            if self.zero.take().is_none() {
                return false;
            }
        } else {
            let index = self.find(key);
            if self.slots[index].is_none() {
                return false;
            }
            self.shift_keys(index, None);
        }
        self.len -= 1;
        // fastutil does not shrink below 16 slots through map removal.
        if self.slots.len() > 16 && self.len < (self.slots.len() * 3 / 4) / 4 {
            self.rehash(self.slots.len() / 2);
        }
        true
    }

    fn iter(&self) -> impl Iterator<Item = &SavedChunkTicket> {
        self.zero
            .iter()
            .chain(
                self.slots
                    .iter()
                    .rev()
                    .filter_map(|entry| entry.as_ref().map(|(_, tickets)| tickets)),
            )
            .flat_map(|tickets| tickets.iter())
    }
}

/// Owns both the active ticket buckets and their simulation graph so their
/// ordering and levels cannot diverge when tickets expire or move.
#[derive(Clone, Debug, Default)]
pub struct SimulationTickets {
    buckets: Buckets,
    graph: SimulationChunks,
}

impl SimulationTickets {
    /// Activate saved entries in Java's order. Duplicate type/level entries
    /// refresh the earlier entry's timeout as `TicketStorage.addTicket` does.
    pub fn from_saved(saved: &SavedChunkTickets) -> Self {
        let mut store = Self::default();
        for ticket in saved.activation_order() {
            store.insert(*ticket);
        }
        store
    }

    pub fn from_capture(
        captured: &native_persistence::world::CapturedChunkTickets,
        session: &native_persistence::save::WorldSessionGuard,
        dimension: &native_persistence::world::DimensionId,
    ) -> anyhow::Result<Self> {
        let _lease = captured.lock_current_for(session, dimension)?;
        Ok(Self::from_saved(captured.tickets()))
    }

    pub fn tracker(&self) -> &SimulationChunks {
        &self.graph
    }

    pub fn run_updates(&mut self) {
        self.graph.run_updates();
    }

    pub fn run_updates_budget(&mut self, budget: usize) -> usize {
        self.graph.run_updates_budget(budget)
    }

    /// Iterate active tickets in Java's active-map order.
    pub fn tickets(&self) -> impl Iterator<Item = &SavedChunkTicket> {
        self.buckets.iter()
    }

    /// Add a live ticket. Returns false if an existing ticket was refreshed.
    pub fn add_ticket(&mut self, pos: ChunkPos, kind: TicketKind, level: i32) -> bool {
        self.insert(SavedChunkTicket {
            position: native_persistence::world::ChunkPos { x: pos.x, z: pos.z },
            kind,
            level,
            ticks_left: kind.timeout().unwrap_or(0),
        })
    }

    fn insert(&mut self, ticket: SavedChunkTicket) -> bool {
        let pos = ChunkPos {
            x: ticket.position.x,
            z: ticket.position.z,
        };
        let bucket = self.buckets.get_or_insert(pack(pos));
        if let Some(existing) = bucket
            .iter_mut()
            .find(|existing| existing.kind == ticket.kind && existing.level == ticket.level)
        {
            existing.ticks_left = ticket.kind.timeout().unwrap_or(0);
            return false;
        }
        bucket.push(ticket);
        if ticket.kind.simulates() {
            self.graph
                .add_ticket(pos, ticket.kind.simulation_key(), ticket.level);
        }
        true
    }

    pub fn remove_ticket(&mut self, pos: ChunkPos, kind: TicketKind, level: i32) -> bool {
        let key = pack(pos);
        let Some(tickets) = self.buckets.get_mut(key) else {
            return false;
        };
        let Some(index) = tickets
            .iter()
            .position(|ticket| ticket.kind == kind && ticket.level == level)
        else {
            return false;
        };
        tickets.remove(index);
        if tickets.is_empty() {
            self.buckets.remove_bucket(key);
        }
        if kind.simulates() {
            let removed = self.graph.remove_ticket(pos, kind.simulation_key(), level);
            debug_assert!(removed, "simulation ticket source missing");
        }
        true
    }

    /// Replace all tickets of a kind in the original active-map order.
    pub fn replace_level(&mut self, kind: TicketKind, level: i32) {
        let tickets: Vec<_> = self
            .tickets()
            .filter(|ticket| ticket.kind == kind)
            .map(|ticket| {
                (
                    ChunkPos {
                        x: ticket.position.x,
                        z: ticket.position.z,
                    },
                    ticket.level,
                )
            })
            .collect();
        for (pos, old_level) in tickets {
            self.remove_ticket(pos, kind, old_level);
            self.add_ticket(pos, kind, level);
        }
    }

    /// Decrease eligible timers once and remove expired entries in Java's
    /// active-map iterator order. The graph remains pending until run_updates.
    pub fn purge_stale(
        &mut self,
        mut readiness: impl FnMut(ChunkPos) -> ChunkSaveReadiness,
    ) -> usize {
        let mut remaining = self.buckets.len;
        let mut expired = 0;
        if let Some(tickets) = self.buckets.zero.as_mut() {
            remaining -= 1;
            expired += purge_bucket(&mut self.graph, 0, tickets, &mut readiness);
            if tickets.is_empty() {
                self.buckets.zero = None;
                self.buckets.len -= 1;
            }
        }
        let mut cursor = self.buckets.slots.len() as isize;
        let mut wrapped = Vec::new();
        while remaining > 0 {
            cursor -= 1;
            let (index, from_wrapped) = loop {
                if cursor < 0 {
                    let key = wrapped[(-cursor - 1) as usize];
                    break (self.buckets.find(key), true);
                }
                if self.buckets.slots[cursor as usize].is_some() {
                    break (cursor as usize, false);
                }
                cursor -= 1;
            };
            let (key, tickets) = self.buckets.slots[index]
                .as_mut()
                .expect("ticket iterator entry is present");
            let key = *key;
            remaining -= 1;
            expired += purge_bucket(&mut self.graph, key, tickets, &mut readiness);
            if tickets.is_empty() {
                if from_wrapped {
                    self.buckets.remove_bucket(key);
                } else {
                    self.buckets.shift_keys(index, Some(&mut wrapped));
                    self.buckets.len -= 1;
                }
            }
        }
        expired
    }
}

fn purge_bucket(
    graph: &mut SimulationChunks,
    key: u64,
    tickets: &mut Vec<SavedChunkTicket>,
    readiness: &mut impl FnMut(ChunkPos) -> ChunkSaveReadiness,
) -> usize {
    let position = unpack(key);
    let mut removed = Vec::new();
    let mut count = 0;
    tickets.retain_mut(|ticket| {
        let can_expire = ticket.kind.timeout().is_some()
            && (ticket.kind.can_expire_if_unloaded()
                || readiness(position) != ChunkSaveReadiness::Pending);
        if !can_expire {
            return true;
        }
        ticket.ticks_left = ticket.ticks_left.wrapping_sub(1);
        if ticket.ticks_left >= 0 {
            return true;
        }
        count += 1;
        if ticket.kind.simulates() {
            removed.push((ticket.kind.simulation_key(), ticket.level));
        }
        false
    });
    if !removed.is_empty() {
        graph.remove_expired_tickets(position, &removed);
    }
    count
}
