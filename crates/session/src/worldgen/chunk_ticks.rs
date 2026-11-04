//! Tick containers created when a generated ProtoChunk becomes a LevelChunk.
//!
//! Saved proto ticks remain pending during postprocessing. New ticks can be
//! scheduled meanwhile, but must not duplicate the pending identities.
use std::{cmp::Ordering, collections::BTreeSet};

use anyhow::{Result, ensure};
use native_resources::packs::ResourceId;
use native_world::loaded::ChunkPos;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedTick {
    pub kind: String,
    pub position: [i32; 3],
    pub delay: i32,
    pub priority: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScheduledTick {
    pub kind: String,
    pub position: [i32; 3],
    pub trigger: i64,
    pub priority: i32,
    pub sub_tick: i64,
}

fn compare(a: &ScheduledTick, b: &ScheduledTick) -> Ordering {
    (a.trigger, a.priority, a.sub_tick).cmp(&(b.trigger, b.priority, b.sub_tick))
}

fn canonical(kind: &str) -> Result<String> {
    let id = ResourceId::parse(kind)?;
    Ok(format!("{}:{}", id.namespace, id.path))
}

/// Models Java's pending list, uniqueness set and PriorityQueue. Construct
/// one separately for blocks and fluids; type IDs must be from the active
/// registry. This alone does not make a generated chunk playable or saveable.
pub struct ChunkTicks {
    chunk: ChunkPos,
    pending: Option<Vec<SavedTick>>,
    identities: BTreeSet<([i32; 3], String)>,
    heap: Vec<ScheduledTick>,
}

impl ChunkTicks {
    pub fn from_saved(chunk: ChunkPos, mut ticks: Vec<SavedTick>) -> Result<Self> {
        ensure!(ticks.len() <= i32::MAX as usize, "too many pending ticks");
        let mut identities = BTreeSet::new();
        for tick in &mut ticks {
            tick.kind = canonical(&tick.kind)?;
            Self::validate(chunk, tick.position, tick.priority)?;
            // Java retains duplicate saved ticks but deduplicates new schedules.
            identities.insert((tick.position, tick.kind.clone()));
        }
        Ok(Self {
            chunk,
            pending: Some(ticks),
            identities,
            heap: Vec::new(),
        })
    }

    fn validate(chunk: ChunkPos, pos: [i32; 3], priority: i32) -> Result<()> {
        ensure!(
            pos[0].div_euclid(16) == chunk.x && pos[2].div_euclid(16) == chunk.z,
            "tick is outside its chunk"
        );
        ensure!((-3..=3).contains(&priority), "invalid tick priority");
        Ok(())
    }

    pub fn contains(&self, kind: &str, pos: [i32; 3]) -> Result<bool> {
        Ok(self.identities.contains(&(pos, canonical(kind)?)))
    }

    /// A duplicate is ignored even if the old tick is still pending.
    pub fn schedule(&mut self, mut tick: ScheduledTick) -> Result<bool> {
        Self::validate(self.chunk, tick.position, tick.priority)?;
        tick.kind = canonical(&tick.kind)?;
        if !self.identities.insert((tick.position, tick.kind.clone())) {
            return Ok(false);
        }
        self.push(tick);
        Ok(true)
    }

    /// Activate pending ticks at the supplied world time, preserving Java's
    /// negative sub-tick order. New postprocessing ticks are already queued.
    pub fn unpack(&mut self, time: i64) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        let count = pending.len() as i64;
        for (index, tick) in pending.into_iter().enumerate() {
            self.push(ScheduledTick {
                kind: tick.kind,
                position: tick.position,
                trigger: time.wrapping_add(i64::from(tick.delay)),
                priority: tick.priority,
                sub_tick: index as i64 - count,
            });
        }
    }

    pub fn peek(&self) -> Option<&ScheduledTick> {
        self.heap.first()
    }

    pub fn poll(&mut self) -> Option<ScheduledTick> {
        let last = self.heap.pop()?;
        let result = if self.heap.is_empty() {
            last
        } else {
            let result = std::mem::replace(&mut self.heap[0], last);
            let mut parent = 0;
            loop {
                let left = parent * 2 + 1;
                if left >= self.heap.len() {
                    break;
                }
                let right = left + 1;
                let child = if right < self.heap.len()
                    && compare(&self.heap[right], &self.heap[left]).is_lt()
                {
                    right
                } else {
                    left
                };
                if !compare(&self.heap[child], &self.heap[parent]).is_lt() {
                    break;
                }
                self.heap.swap(parent, child);
                parent = child;
            }
            result
        };
        self.identities
            .remove(&(result.position, result.kind.clone()));
        Some(result)
    }

    /// Java writes pending entries first, then queued entries stably sorted by
    /// sub-tick order, using signed wrapping subtraction and narrowing to i32.
    pub fn pack(&self, time: i64) -> Vec<SavedTick> {
        let mut saved = self.pending.clone().unwrap_or_default();
        let mut queued: Vec<_> = self.heap.iter().collect();
        queued.sort_by_key(|tick| tick.sub_tick);
        saved.extend(queued.into_iter().map(|tick| SavedTick {
            kind: tick.kind.clone(),
            position: tick.position,
            delay: tick.trigger.wrapping_sub(time) as i32,
            priority: tick.priority,
        }));
        saved
    }

    fn push(&mut self, tick: ScheduledTick) {
        self.heap.push(tick);
        let mut child = self.heap.len() - 1;
        while child > 0 {
            let parent = (child - 1) / 2;
            if !compare(&self.heap[child], &self.heap[parent]).is_lt() {
                break;
            }
            self.heap.swap(child, parent);
            child = parent;
        }
    }
}

/// Tick containers for the converted chunk, kept separate by registry.
pub struct ConvertedChunkTicks {
    pub blocks: ChunkTicks,
    pub fluids: ChunkTicks,
}
