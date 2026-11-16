//! Tick containers created when a generated ProtoChunk becomes a LevelChunk.
//!
//! Saved proto ticks remain pending during postprocessing. New ticks can be
//! scheduled meanwhile, but must not duplicate the pending identities.
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

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

fn tick_order(tick: &ScheduledTick) -> (i64, i32, i64) {
    (tick.trigger, tick.priority, tick.sub_tick)
}

fn canonical(kind: &str) -> Result<String> {
    let id = ResourceId::parse(kind)?;
    Ok(format!("{}:{}", id.namespace, id.path))
}

/// Models Java's pending list, uniqueness set and PriorityQueue. Construct
/// one separately for blocks and fluids; type IDs must be from the active
/// registry. This alone does not make a generated chunk playable or saveable.
#[derive(Clone, Eq, PartialEq)]
pub struct ChunkTicks {
    chunk: ChunkPos,
    pending: Option<Vec<SavedTick>>,
    identities: BTreeSet<([i32; 3], String)>,
    heap: Vec<ScheduledTick>,
}

impl ChunkTicks {
    /// Whether the container has no saved, queued, or reserved tick identities.
    pub(crate) fn is_empty(&self) -> bool {
        self.pending.as_ref().is_none_or(Vec::is_empty)
            && self.heap.is_empty()
            && self.identities.is_empty()
    }

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

    /// Saved entries that have not yet been unpacked by the ticking callback.
    pub(crate) fn pending_saved(&self) -> Option<&[SavedTick]> {
        self.pending.as_deref()
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

    /// Snapshot Java's queue after pending entries are unpacked at `time`.
    /// Draining a clone preserves the queue's tie behavior without changing
    /// the original container or collapsing duplicate saved entries.
    pub fn activated(&self, time: i64) -> Vec<ScheduledTick> {
        let mut copy = self.clone();
        copy.unpack(time);
        let mut result = Vec::with_capacity(copy.heap.len());
        while let Some(tick) = copy.poll() {
            result.push(tick);
        }
        result
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
#[derive(Clone)]
pub struct ConvertedChunkTicks {
    pub blocks: ChunkTicks,
    pub fluids: ChunkTicks,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TickRegistry {
    Block,
    Fluid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScheduleOutcome {
    Added,
    Duplicate,
    MissingChunk,
}

/// One ordered schedule request in a fork of the live level queues. Even
/// rejected requests consume the shared level sub-tick counter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CapturedTickAttempt {
    pub registry: TickRegistry,
    pub tick: ScheduledTick,
    pub outcome: ScheduleOutcome,
}

/// Tick access for an isolated converted region. All block and fluid
/// callbacks share the level's sub-tick counter, including schedules rejected
/// as duplicates or sent to a chunk without a registered tick container.
#[derive(Clone)]
pub struct RegionTicks {
    chunks: BTreeMap<ChunkPos, ConvertedChunkTicks>,
    game_time: i64,
    next_sub_tick: i64,
    registration: Vec<ChunkPos>,
    indexes: Option<RegionTickIndexes>,
    live_attempts: Option<Vec<CapturedTickAttempt>>,
}

#[derive(Clone)]
struct RegionTickIndexes {
    blocks: super::tick_index::LevelTickIndex,
    fluids: super::tick_index::LevelTickIndex,
}

/// The activated queues of one converted chunk in Java's poll order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivatedChunkTicks {
    pub blocks: Vec<ScheduledTick>,
    pub fluids: Vec<ScheduledTick>,
}

/// A detached view retaining absolute triggers and the shared sub-tick counter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivatedRegionTicks {
    pub game_time: i64,
    pub next_sub_tick: i64,
    pub chunks: BTreeMap<ChunkPos, ActivatedChunkTicks>,
}

/// Detached, activated containers for one registry. The caller supplies the
/// level scheduler's container traversal order on every collection; that order
/// matters when two container heads compare equal.
#[derive(Clone, Eq, PartialEq)]
pub struct ActiveRegionQueue {
    chunks: BTreeMap<ChunkPos, Arc<ChunkTicks>>,
}

/// Activated containers and the stateful Java-order index of their next ticks.
#[derive(Clone, Eq, PartialEq)]
pub struct IndexedRegionQueue {
    queue: ActiveRegionQueue,
    index: super::tick_index::LevelTickIndex,
    registration: Vec<ChunkPos>,
}

impl Default for IndexedRegionQueue {
    fn default() -> Self {
        Self {
            queue: ActiveRegionQueue {
                chunks: BTreeMap::new(),
            },
            index: super::tick_index::LevelTickIndex::default(),
            registration: Vec::new(),
        }
    }
}

impl ActiveRegionQueue {
    fn compare_heads(&self, left: ChunkPos, right: ChunkPos) -> Ordering {
        let a = self.chunks[&left]
            .peek()
            .expect("queued container has a head");
        let b = self.chunks[&right]
            .peek()
            .expect("queued container has a head");
        (a.priority, a.sub_tick).cmp(&(b.priority, b.sub_tick))
    }

    fn push_container(&self, heap: &mut Vec<ChunkPos>, chunk: ChunkPos) {
        heap.push(chunk);
        let mut child = heap.len() - 1;
        while child > 0 {
            let parent = (child - 1) / 2;
            if !self.compare_heads(heap[child], heap[parent]).is_lt() {
                break;
            }
            heap.swap(child, parent);
            child = parent;
        }
    }

    fn pop_container(&self, heap: &mut Vec<ChunkPos>) -> Option<ChunkPos> {
        let last = heap.pop()?;
        if heap.is_empty() {
            return Some(last);
        }
        let result = std::mem::replace(&mut heap[0], last);
        let mut parent = 0;
        loop {
            let left = parent * 2 + 1;
            if left >= heap.len() {
                break;
            }
            let right = left + 1;
            let child = if right < heap.len() && self.compare_heads(heap[right], heap[left]).is_lt()
            {
                right
            } else {
                left
            };
            if !self.compare_heads(heap[child], heap[parent]).is_lt() {
                break;
            }
            heap.swap(parent, child);
            parent = child;
        }
        Some(result)
    }

    /// Collect at most `limit` due ticks before executing any callbacks.
    /// `traversal` lists tickable containers in the scheduler's map iteration
    /// order. Omitted chunks are not tickable and keep their queued ticks.
    pub fn collect_due(
        &mut self,
        traversal: &[ChunkPos],
        time: i64,
        limit: usize,
    ) -> Result<Vec<ScheduledTick>> {
        let mut seen = BTreeSet::new();
        for pos in traversal {
            ensure!(self.chunks.contains_key(pos), "unknown tick container");
            ensure!(seen.insert(*pos), "duplicate tick container");
        }
        Ok(self.collect_validated(traversal, time, limit).0)
    }

    fn collect_validated(
        &mut self,
        traversal: &[ChunkPos],
        time: i64,
        limit: usize,
    ) -> (Vec<ScheduledTick>, Vec<(ChunkPos, i64)>) {
        let mut heap = Vec::new();
        for &pos in traversal {
            if self.chunks[&pos]
                .peek()
                .is_some_and(|tick| tick.trigger <= time)
            {
                self.push_container(&mut heap, pos);
            }
        }
        let mut collected = Vec::new();
        let mut reschedule = Vec::new();
        while collected.len() < limit {
            let Some(pos) = self.pop_container(&mut heap) else {
                break;
            };
            collected.push(
                Arc::make_mut(self.chunks.get_mut(&pos).unwrap())
                    .poll()
                    .unwrap(),
            );
            while collected.len() < limit {
                let Some(next) = self.chunks[&pos].peek() else {
                    break;
                };
                if next.trigger > time {
                    break;
                }
                if let Some(&other) = heap.first() {
                    let other = self.chunks[&other].peek().unwrap();
                    if (next.priority, next.sub_tick) > (other.priority, other.sub_tick) {
                        break;
                    }
                }
                collected.push(
                    Arc::make_mut(self.chunks.get_mut(&pos).unwrap())
                        .poll()
                        .unwrap(),
                );
            }
            if let Some(next) = self.chunks[&pos].peek() {
                if collected.len() < limit && next.trigger <= time {
                    self.push_container(&mut heap, pos);
                } else {
                    reschedule.push((pos, next.trigger));
                }
            }
        }
        // Java iterates PriorityQueue's backing array here; it does not poll
        // the heap. This order affects the hash index on the next game tick.
        for pos in heap {
            reschedule.push((pos, self.chunks[&pos].peek().unwrap().trigger));
        }
        (collected, reschedule)
    }

    /// Schedule after collection or execution, as with LevelTicks.schedule.
    pub fn schedule(&mut self, tick: ScheduledTick) -> Result<ScheduleOutcome> {
        let pos = ChunkPos {
            x: tick.position[0].div_euclid(16),
            z: tick.position[2].div_euclid(16),
        };
        let Some(chunk) = self.chunks.get_mut(&pos) else {
            return Ok(ScheduleOutcome::MissingChunk);
        };
        Ok(if Arc::make_mut(chunk).schedule(tick)? {
            ScheduleOutcome::Added
        } else {
            ScheduleOutcome::Duplicate
        })
    }
}

impl IndexedRegionQueue {
    /// Register or replace a chunk's container, retaining the Java index's
    /// registration history. Pending saved ticks are activated separately.
    pub fn add_container(
        &mut self,
        pos: ChunkPos,
        ticks: ChunkTicks,
    ) -> Result<Option<ChunkTicks>> {
        ensure!(
            ticks.chunk == pos,
            "tick container belongs to another chunk"
        );
        if let Some(head) = ticks.peek() {
            self.index.insert(pos, head.trigger);
        }
        let previous = self.queue.chunks.insert(pos, Arc::new(ticks));
        if previous.is_none() {
            self.registration.push(pos);
        }
        Ok(previous.map(Arc::unwrap_or_clone))
    }

    /// Unpack the registered container's pending ticks at level time.
    pub fn unpack_container(&mut self, pos: ChunkPos, time: i64) -> Result<()> {
        let ticks = self
            .queue
            .chunks
            .get_mut(&pos)
            .ok_or_else(|| anyhow::anyhow!("tick container is not registered"))?;
        let ticks = Arc::make_mut(ticks);
        let before = ticks.peek().map(tick_order);
        ticks.unpack(time);
        let after = ticks.peek().map(tick_order);
        if after != before
            && let Some(head) = after
        {
            self.index.insert(pos, head.0);
        }
        Ok(())
    }

    /// Detach a container and its index entry when a chunk leaves the level.
    pub fn remove_container(&mut self, pos: ChunkPos) -> Option<ChunkTicks> {
        self.index.remove(pos);
        let removed = self.queue.chunks.remove(&pos);
        if removed.is_some() {
            self.registration.retain(|&position| position != pos);
        }
        removed.map(Arc::unwrap_or_clone)
    }

    pub fn container(&self, pos: ChunkPos) -> Option<&ChunkTicks> {
        self.queue.chunks.get(&pos).map(Arc::as_ref)
    }

    /// Order in which currently registered containers were attached.
    pub fn registration_order(&self) -> &[ChunkPos] {
        &self.registration
    }

    /// Collect due ticks in Java's container-map and intra-tick order.
    pub fn collect_due(
        &mut self,
        time: i64,
        limit: usize,
        mut tickable: impl FnMut(ChunkPos) -> bool,
    ) -> Vec<ScheduledTick> {
        use super::tick_index::DueEntryAction;
        let queue = &self.queue;
        let traversal = self.index.inspect_due(time, |pos| {
            let Some(head) = queue.chunks.get(&pos).and_then(|ticks| ticks.peek()) else {
                return DueEntryAction::Remove;
            };
            if head.trigger > time {
                DueEntryAction::Update(head.trigger)
            } else if tickable(pos) {
                DueEntryAction::Take
            } else {
                DueEntryAction::Keep
            }
        });
        let (ticks, reschedule) = self.queue.collect_validated(&traversal, time, limit);
        for (pos, trigger) in reschedule {
            self.index.insert(pos, trigger);
        }
        ticks
    }

    /// Update the index only when scheduling changes a container's head.
    pub fn schedule(&mut self, tick: ScheduledTick) -> Result<ScheduleOutcome> {
        let pos = ChunkPos {
            x: tick.position[0].div_euclid(16),
            z: tick.position[2].div_euclid(16),
        };
        let before = self
            .queue
            .chunks
            .get(&pos)
            .and_then(|ticks| ticks.peek())
            .map(tick_order);
        let outcome = self.queue.schedule(tick)?;
        if outcome == ScheduleOutcome::Added {
            let after = self.queue.chunks[&pos].peek().map(tick_order);
            if after != before {
                self.index
                    .insert(pos, after.expect("added tick has a head").0);
            }
        }
        Ok(outcome)
    }
}

impl RegionTicks {
    pub fn new(
        chunks: BTreeMap<ChunkPos, ConvertedChunkTicks>,
        game_time: i64,
        next_sub_tick: i64,
    ) -> Result<Self> {
        let registration: Vec<_> = chunks.keys().copied().collect();
        Self::new_ordered(chunks, game_time, next_sub_tick, &registration)
    }

    /// Register converted containers in the supplied level registration order.
    pub fn new_ordered(
        chunks: BTreeMap<ChunkPos, ConvertedChunkTicks>,
        game_time: i64,
        next_sub_tick: i64,
        registration: &[ChunkPos],
    ) -> Result<Self> {
        for (pos, ticks) in &chunks {
            ensure!(
                ticks.blocks.chunk == *pos && ticks.fluids.chunk == *pos,
                "tick container belongs to another chunk"
            );
        }
        let mut seen = BTreeSet::new();
        let mut indexes = RegionTickIndexes {
            blocks: super::tick_index::LevelTickIndex::default(),
            fluids: super::tick_index::LevelTickIndex::default(),
        };
        for &pos in registration {
            ensure!(seen.insert(pos), "duplicate tick container registration");
            let ticks = chunks
                .get(&pos)
                .ok_or_else(|| anyhow::anyhow!("unknown tick container registration"))?;
            if let Some(head) = ticks.blocks.peek() {
                indexes.blocks.insert(pos, head.trigger);
            }
            if let Some(head) = ticks.fluids.peek() {
                indexes.fluids.insert(pos, head.trigger);
            }
        }
        ensure!(
            seen.len() == chunks.len(),
            "missing tick container registration"
        );
        Ok(Self {
            chunks,
            game_time,
            next_sub_tick,
            registration: registration.to_vec(),
            indexes: Some(indexes),
            live_attempts: None,
        })
    }

    /// Fork the actual level queues without rebuilding their hash-index
    /// histories or prematurely unpacking any saved chunk ticks. This should
    /// run on a worker thread; the caller must recheck its live capture.
    pub(crate) fn from_indexed_queues(
        blocks: &IndexedRegionQueue,
        fluids: &IndexedRegionQueue,
        game_time: i64,
        next_sub_tick: i64,
    ) -> Result<Self> {
        ensure!(
            blocks.registration == fluids.registration,
            "block and fluid tick registration orders differ"
        );
        let registration = &blocks.registration;
        ensure!(
            registration.len() == blocks.queue.chunks.len()
                && registration.len() == fluids.queue.chunks.len(),
            "tick registration and containers differ"
        );
        let mut chunks = BTreeMap::new();
        for &pos in registration {
            let block = blocks
                .container(pos)
                .ok_or_else(|| anyhow::anyhow!("block tick container is missing"))?;
            let fluid = fluids
                .container(pos)
                .ok_or_else(|| anyhow::anyhow!("fluid tick container is missing"))?;
            ensure!(
                block.chunk == pos && fluid.chunk == pos,
                "tick container belongs to another chunk"
            );
            ensure!(
                chunks
                    .insert(
                        pos,
                        ConvertedChunkTicks {
                            blocks: block.clone(),
                            fluids: fluid.clone(),
                        },
                    )
                    .is_none(),
                "duplicate tick container registration"
            );
        }
        Ok(Self {
            chunks,
            game_time,
            next_sub_tick,
            registration: registration.clone(),
            indexes: Some(RegionTickIndexes {
                blocks: blocks.index.clone(),
                fluids: fluids.index.clone(),
            }),
            live_attempts: Some(Vec::new()),
        })
    }

    pub fn chunk(&self, pos: ChunkPos) -> Option<&ConvertedChunkTicks> {
        self.chunks.get(&pos)
    }

    /// Register one newly converted chunk after earlier postprocessing has
    /// already scheduled ticks. Existing containers and the shared sub-tick
    /// counter are preserved. Replacement is rejected because it would discard
    /// their pending ticks and registration history.
    pub fn register_chunk(&mut self, pos: ChunkPos, ticks: ConvertedChunkTicks) -> Result<()> {
        ensure!(
            ticks.blocks.chunk == pos && ticks.fluids.chunk == pos,
            "tick container belongs to another chunk"
        );
        ensure!(
            !self.chunks.contains_key(&pos),
            "tick container is already registered"
        );
        if let Some(indexes) = &mut self.indexes {
            if let Some(head) = ticks.blocks.peek() {
                indexes.blocks.insert(pos, head.trigger);
            }
            if let Some(head) = ticks.fluids.peek() {
                indexes.fluids.insert(pos, head.trigger);
            }
        }
        self.chunks.insert(pos, ticks);
        self.registration.push(pos);
        Ok(())
    }

    pub fn chunk_mut(&mut self, pos: ChunkPos) -> Option<&mut ConvertedChunkTicks> {
        if self.chunks.contains_key(&pos) {
            // Direct edits bypass LevelTicks' onTickAdded callback.
            self.indexes = None;
        }
        self.chunks.get_mut(&pos)
    }

    pub fn registration_order(&self) -> &[ChunkPos] {
        &self.registration
    }

    pub fn next_sub_tick(&self) -> i64 {
        self.next_sub_tick
    }

    /// Attempts made since the live queue fork, in execution order.
    pub(crate) fn live_attempts(&self) -> Option<&[CapturedTickAttempt]> {
        self.live_attempts.as_deref()
    }

    /// World time shared by all schedules in this isolated pass.
    pub fn game_time(&self) -> i64 {
        self.game_time
    }

    /// Unpack both saved containers after this chunk's postprocessing, keeping
    /// the recorded LevelTicks index mutations for subsequent callbacks.
    pub(super) fn unpack_chunk(&mut self, pos: ChunkPos) -> Result<()> {
        let indexes = self
            .indexes
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("tick index history is unavailable"))?;
        let chunk = self
            .chunks
            .get_mut(&pos)
            .ok_or_else(|| anyhow::anyhow!("tick container is missing"))?;
        match (
            chunk.blocks.pending.is_some(),
            chunk.fluids.pending.is_some(),
        ) {
            (false, false) => return Ok(()),
            (true, true) => {}
            _ => anyhow::bail!("saved tick containers have inconsistent activation"),
        }
        for (ticks, index) in [
            (&mut chunk.blocks, &mut indexes.blocks),
            (&mut chunk.fluids, &mut indexes.fluids),
        ] {
            let before = ticks.peek().map(tick_order);
            ticks.unpack(self.game_time);
            let after = ticks.peek().map(tick_order);
            if after != before
                && let Some(head) = after
            {
                index.insert(pos, head.0);
            }
        }
        Ok(())
    }

    /// Capture every converted queue as it would look after activation at the
    /// pass's game time. The original pending lists remain untouched.
    pub fn activated_snapshot(&self) -> ActivatedRegionTicks {
        ActivatedRegionTicks {
            game_time: self.game_time,
            next_sub_tick: self.next_sub_tick,
            chunks: self
                .chunks
                .iter()
                .map(|(&pos, ticks)| {
                    (
                        pos,
                        ActivatedChunkTicks {
                            blocks: ticks.blocks.activated(self.game_time),
                            fluids: ticks.fluids.activated(self.game_time),
                        },
                    )
                })
                .collect(),
        }
    }

    /// Clone one registry and unpack its pending ticks at this region's time.
    pub fn activated_queue(&self, registry: TickRegistry) -> ActiveRegionQueue {
        ActiveRegionQueue {
            chunks: self
                .chunks
                .iter()
                .map(|(&pos, ticks)| {
                    let mut ticks = match registry {
                        TickRegistry::Block => ticks.blocks.clone(),
                        TickRegistry::Fluid => ticks.fluids.clone(),
                    };
                    ticks.unpack(self.game_time);
                    (pos, Arc::new(ticks))
                })
                .collect(),
        }
    }

    /// Rebuild an index from the current activated heads in this order. This
    /// discards earlier insertion history; use `captured_indexed_queue` when
    /// reproducing a live level whose registration history was recorded.
    pub fn activated_indexed_queue(
        &self,
        registry: TickRegistry,
        registration: &[ChunkPos],
    ) -> Result<IndexedRegionQueue> {
        let queue = self.activated_queue(registry);
        let mut seen = BTreeSet::new();
        let mut index = super::tick_index::LevelTickIndex::default();
        for &pos in registration {
            ensure!(seen.insert(pos), "duplicate tick container registration");
            let ticks = queue
                .chunks
                .get(&pos)
                .ok_or_else(|| anyhow::anyhow!("unknown tick container registration"))?;
            if let Some(tick) = ticks.peek() {
                index.insert(pos, tick.trigger);
            }
        }
        ensure!(
            seen.len() == queue.chunks.len(),
            "missing tick container registration"
        );
        Ok(IndexedRegionQueue {
            queue,
            index,
            registration: registration.to_vec(),
        })
    }

    /// Activate a clone while retaining the actual recorded index mutations.
    /// The order is the caller's real sequence of chunk activation events.
    pub fn captured_indexed_queue(
        &self,
        registry: TickRegistry,
        activation: &[ChunkPos],
    ) -> Result<IndexedRegionQueue> {
        ensure!(
            activation.len() == self.chunks.len(),
            "missing tick container activation"
        );
        self.captured_indexed_queue_for(registry, activation)
    }

    /// Retain the recorded index while activating only currently resident
    /// chunks. Other registered containers keep their pending ticks untouched.
    pub fn captured_indexed_queue_for(
        &self,
        registry: TickRegistry,
        activation: &[ChunkPos],
    ) -> Result<IndexedRegionQueue> {
        let indexes = self
            .indexes
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("tick index history was invalidated by direct edits"))?;
        let mut seen = BTreeSet::new();
        for pos in activation {
            ensure!(
                self.chunks.contains_key(pos),
                "unknown tick container activation"
            );
            ensure!(seen.insert(*pos), "duplicate tick container activation");
        }
        let chunks = self
            .chunks
            .iter()
            .map(|(&pos, ticks)| {
                (
                    pos,
                    Arc::new(match registry {
                        TickRegistry::Block => ticks.blocks.clone(),
                        TickRegistry::Fluid => ticks.fluids.clone(),
                    }),
                )
            })
            .collect();
        let index = match registry {
            TickRegistry::Block => indexes.blocks.clone(),
            TickRegistry::Fluid => indexes.fluids.clone(),
        };
        let mut queue = IndexedRegionQueue {
            queue: ActiveRegionQueue { chunks },
            index,
            registration: self.registration.clone(),
        };
        for &pos in activation {
            queue.unpack_container(pos, self.game_time)?;
        }
        Ok(queue)
    }

    /// Match LevelAccessor.createTick followed by LevelTicks.schedule. A
    /// missing container discards the tick in Java; the outcome lets callers
    /// track that event while retaining the consumed sub-tick number.
    pub fn schedule(
        &mut self,
        registry: TickRegistry,
        kind: &str,
        position: [i32; 3],
        delay: i32,
        priority: i32,
    ) -> Result<ScheduleOutcome> {
        let kind = canonical(kind)?;
        ensure!((-3..=3).contains(&priority), "invalid tick priority");
        let sub_tick = self.next_sub_tick;
        self.next_sub_tick = self.next_sub_tick.wrapping_add(1);
        let tick = ScheduledTick {
            kind,
            position,
            trigger: self.game_time.wrapping_add(i64::from(delay)),
            priority,
            sub_tick,
        };
        let recorded = self.live_attempts.as_ref().map(|_| tick.clone());
        let pos = ChunkPos {
            x: position[0].div_euclid(16),
            z: position[2].div_euclid(16),
        };
        let outcome = if let Some(chunk) = self.chunks.get_mut(&pos) {
            let ticks = match registry {
                TickRegistry::Block => &mut chunk.blocks,
                TickRegistry::Fluid => &mut chunk.fluids,
            };
            let before = ticks.peek().map(tick_order);
            let added = ticks.schedule(tick)?;
            let after = ticks.peek().map(tick_order);
            if added
                && after != before
                && let Some(head) = after
                && let Some(indexes) = &mut self.indexes
            {
                match registry {
                    TickRegistry::Block => indexes.blocks.insert(pos, head.0),
                    TickRegistry::Fluid => indexes.fluids.insert(pos, head.0),
                }
            }
            if added {
                ScheduleOutcome::Added
            } else {
                ScheduleOutcome::Duplicate
            }
        } else {
            ScheduleOutcome::MissingChunk
        };
        if let (Some(attempts), Some(tick)) = (&mut self.live_attempts, recorded) {
            attempts.push(CapturedTickAttempt {
                registry,
                tick,
                outcome,
            });
        }
        Ok(outcome)
    }
}

#[cfg(test)]
mod live_capture_tests {
    use super::*;

    #[test]
    fn live_capture_records_added_duplicate_and_missing_schedules_in_order() -> Result<()> {
        let center = ChunkPos { x: 0, z: 0 };
        let mut blocks = IndexedRegionQueue::default();
        let mut fluids = IndexedRegionQueue::default();
        blocks.add_container(
            center,
            ChunkTicks::from_saved(
                center,
                vec![SavedTick {
                    kind: "minecraft:sand".into(),
                    position: [1, 4, 1],
                    delay: 8,
                    priority: 0,
                }],
            )?,
        )?;
        fluids.add_container(center, ChunkTicks::from_saved(center, vec![])?)?;
        let mut region = RegionTicks::from_indexed_queues(&blocks, &fluids, 100, i64::MAX - 1)?;
        assert_eq!(
            region.schedule(TickRegistry::Block, "sand", [1, 4, 1], 2, 0)?,
            ScheduleOutcome::Duplicate
        );
        assert_eq!(
            region.schedule(TickRegistry::Fluid, "water", [2, 4, 1], 5, 0)?,
            ScheduleOutcome::Added
        );
        let before_retry = region.clone();
        assert_eq!(
            region.schedule(TickRegistry::Fluid, "water", [2, 4, 1], 7, 0)?,
            ScheduleOutcome::Duplicate
        );
        assert_eq!(
            region.schedule(TickRegistry::Block, "sand", [48, 4, 1], 2, 0)?,
            ScheduleOutcome::MissingChunk
        );
        let attempts = region.live_attempts().unwrap();
        assert_eq!(
            attempts
                .iter()
                .map(|entry| entry.outcome)
                .collect::<Vec<_>>(),
            [
                ScheduleOutcome::Duplicate,
                ScheduleOutcome::Added,
                ScheduleOutcome::Duplicate,
                ScheduleOutcome::MissingChunk,
            ]
        );
        assert_eq!(
            attempts
                .iter()
                .map(|entry| entry.tick.sub_tick)
                .collect::<Vec<_>>(),
            [i64::MAX - 1, i64::MAX, i64::MIN, i64::MIN + 1]
        );
        assert_eq!(attempts[1].tick.kind, "minecraft:water");
        assert_eq!(attempts[1].tick.trigger, 105);
        assert_eq!(region.next_sub_tick(), i64::MIN + 2);
        assert_eq!(before_retry.live_attempts().unwrap().len(), 2);
        assert!(fluids.container(center).unwrap().pack(100).is_empty());
        assert_eq!(blocks.container(center).unwrap().pack(100).len(), 1);
        Ok(())
    }

    #[test]
    fn captured_indexes_keep_registration_and_copy_only_changed_containers() -> Result<()> {
        let first = ChunkPos { x: 0, z: 0 };
        let second = ChunkPos { x: 1, z: 0 };
        let mut blocks = IndexedRegionQueue::default();
        let mut fluids = IndexedRegionQueue::default();
        for queue in [&mut blocks, &mut fluids] {
            queue.add_container(first, ChunkTicks::from_saved(first, vec![])?)?;
            queue.add_container(second, ChunkTicks::from_saved(second, vec![])?)?;
        }
        blocks.schedule(ScheduledTick {
            kind: "minecraft:sand".into(),
            position: [1, 4, 1],
            trigger: 107,
            priority: 0,
            sub_tick: 2,
        })?;
        let original = blocks.clone();
        assert!(Arc::ptr_eq(
            &original.queue.chunks[&first],
            &blocks.queue.chunks[&first]
        ));
        assert!(Arc::ptr_eq(
            &original.queue.chunks[&second],
            &blocks.queue.chunks[&second]
        ));
        blocks.schedule(ScheduledTick {
            kind: "minecraft:gravel".into(),
            position: [2, 4, 1],
            trigger: 105,
            priority: 0,
            sub_tick: 3,
        })?;
        assert!(!Arc::ptr_eq(
            &original.queue.chunks[&first],
            &blocks.queue.chunks[&first]
        ));
        assert!(Arc::ptr_eq(
            &original.queue.chunks[&second],
            &blocks.queue.chunks[&second]
        ));
        assert_eq!(original.container(first).unwrap().pack(100).len(), 1);
        assert_eq!(blocks.container(first).unwrap().pack(100).len(), 2);

        let region = RegionTicks::from_indexed_queues(&blocks, &fluids, 100, 4)?;
        assert_eq!(region.registration_order(), &[first, second]);
        let mut actual = region.captured_indexed_queue_for(TickRegistry::Block, &[])?;
        let mut expected = blocks.clone();
        assert_eq!(
            actual.collect_due(110, 10, |_| true),
            expected.collect_due(110, 10, |_| true)
        );
        assert_eq!(blocks.registration_order(), &[first, second]);

        let replaced = blocks.add_container(first, ChunkTicks::from_saved(first, vec![])?)?;
        assert!(replaced.is_some());
        assert_eq!(blocks.registration_order(), &[first, second]);
        blocks.remove_container(first).unwrap();
        blocks.add_container(first, ChunkTicks::from_saved(first, vec![])?)?;
        assert_eq!(blocks.registration_order(), &[second, first]);
        assert!(RegionTicks::from_indexed_queues(&blocks, &fluids, 100, 4).is_err());
        Ok(())
    }
}
