//! Ordering of the due-container index used by Java's `LevelTicks`.
//!
//! The index models the default fastutil Long2LongOpenHashMap, including its
//! zero key and removal while traversing. It does not decide which chunks are
//! loaded or which tick container is registered.

use native_world::loaded::ChunkPos;

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
    let h = key.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let h = h ^ (h >> 32);
    h ^ (h >> 16)
}

/// The next scheduled time for each chunk, in Java fastutil map order.
#[derive(Clone, Debug)]
pub struct LevelTickIndex {
    slots: Vec<Option<(u64, i64)>>,
    zero: Option<i64>,
    len: usize,
}

#[derive(Clone, Copy)]
pub(super) enum DueEntryAction {
    Take,
    Keep,
    Remove,
    Update(i64),
}

impl Default for LevelTickIndex {
    fn default() -> Self {
        Self {
            slots: vec![None; 32],
            zero: None,
            len: 0,
        }
    }
}

impl LevelTickIndex {
    fn find(&self, key: u64) -> usize {
        let mask = self.slots.len() - 1;
        let mut slot = mix(key) as usize & mask;
        while self.slots[slot].is_some_and(|(other, _)| other != key) {
            slot = (slot + 1) & mask;
        }
        slot
    }

    /// Insert or replace a chunk's next trigger time.
    pub fn insert(&mut self, pos: ChunkPos, trigger: i64) {
        let key = pack(pos);
        if key == 0 {
            if self.zero.replace(trigger).is_some() {
                return;
            }
        } else {
            let slot = self.find(key);
            if let Some(entry) = &mut self.slots[slot] {
                entry.1 = trigger;
                return;
            }
            self.slots[slot] = Some((key, trigger));
        }
        let previous = self.len;
        self.len += 1;
        if previous >= self.slots.len() * 3 / 4 {
            self.rehash(self.slots.len() * 2);
        }
    }

    pub fn get(&self, pos: ChunkPos) -> Option<i64> {
        let key = pack(pos);
        if key == 0 {
            self.zero
        } else {
            self.slots[self.find(key)].map(|(_, trigger)| trigger)
        }
    }

    /// Remove a container as Java does when a chunk is unregistered.
    pub fn remove(&mut self, pos: ChunkPos) -> bool {
        let key = pack(pos);
        if key == 0 {
            if self.zero.take().is_none() {
                return false;
            }
        } else {
            let slot = self.find(key);
            if self.slots[slot].is_none() {
                return false;
            }
            self.shift_keys(slot, None);
        }
        self.len -= 1;
        if self.slots.len() > 32 && self.len < (self.slots.len() * 3 / 4) / 4 {
            self.rehash(self.slots.len() / 2);
        }
        true
    }

    fn shift_keys(&mut self, mut hole: usize, mut wrapped: Option<&mut Vec<u64>>) {
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
                if cursor < hole
                    && let Some(wrapped) = &mut wrapped
                {
                    wrapped.push(entry.0);
                }
                self.slots[hole] = Some(entry);
                hole = cursor;
            }
            cursor = (cursor + 1) & mask;
        }
        self.slots[hole] = None;
    }

    fn rehash(&mut self, size: usize) {
        let old = std::mem::replace(&mut self.slots, vec![None; size]);
        for entry in old.into_iter().rev().flatten() {
            let slot = self.find(entry.0);
            self.slots[slot] = Some(entry);
        }
    }

    /// Remove due, tickable entries in the map iterator's order. Entries for
    /// which `tickable` is false stay registered. A later collection must
    /// reinsert any container that still has a head tick.
    pub fn take_due(
        &mut self,
        time: i64,
        mut tickable: impl FnMut(ChunkPos) -> bool,
    ) -> Vec<ChunkPos> {
        self.inspect_due(time, |pos| {
            if tickable(pos) {
                DueEntryAction::Take
            } else {
                DueEntryAction::Keep
            }
        })
    }

    /// Reconcile stale entries while traversing, without changing the slot
    /// order of entries whose trigger is updated in place.
    pub(super) fn inspect_due(
        &mut self,
        time: i64,
        mut inspect: impl FnMut(ChunkPos) -> DueEntryAction,
    ) -> Vec<ChunkPos> {
        let mut result = Vec::new();
        let mut remaining = self.len;
        if let Some(trigger) = self.zero {
            remaining -= 1;
            let pos = unpack(0);
            if trigger <= time {
                match inspect(pos) {
                    DueEntryAction::Take => {
                        self.zero = None;
                        self.len -= 1;
                        result.push(pos);
                    }
                    DueEntryAction::Remove => {
                        self.zero = None;
                        self.len -= 1;
                    }
                    DueEntryAction::Update(trigger) => self.zero = Some(trigger),
                    DueEntryAction::Keep => {}
                }
            }
        }
        let mut cursor = self.slots.len() as isize;
        let mut wrapped = Vec::new();
        while remaining > 0 {
            cursor -= 1;
            let (slot, from_wrapped) = loop {
                if cursor < 0 {
                    let key = wrapped[(-cursor - 1) as usize];
                    break (self.find(key), true);
                }
                if self.slots[cursor as usize].is_some() {
                    break (cursor as usize, false);
                }
                cursor -= 1;
            };
            let (key, trigger) = self.slots[slot].expect("iterator entry is present");
            remaining -= 1;
            let pos = unpack(key);
            if trigger <= time {
                let action = inspect(pos);
                match action {
                    DueEntryAction::Take | DueEntryAction::Remove => {
                        if from_wrapped {
                            self.remove(pos);
                        } else {
                            self.shift_keys(slot, Some(&mut wrapped));
                            self.len -= 1;
                        }
                        if matches!(action, DueEntryAction::Take) {
                            result.push(pos);
                        }
                    }
                    DueEntryAction::Update(next) => self.slots[slot] = Some((key, next)),
                    DueEntryAction::Keep => {}
                }
            }
        }
        result
    }
}
