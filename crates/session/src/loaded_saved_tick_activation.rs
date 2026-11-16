//! Activate saved tick delays only after the corresponding preparation check.
//!
//! This does not establish the Java holder's ticking readiness or authorize a
//! save. New schedules remain separate from the original pending entries.

use std::{collections::BTreeSet, sync::Arc};

use anyhow::{Context, Result, ensure};

use super::{
    CollectedBlockTick, FluidTickKey, GeneratedTickOrder, GenerationTerrainStage, LoadedSession,
    PendingSavedTickActivation, ScheduledFluid, generated_block_key, generated_fluid_key,
};
use crate::worldgen::chunk_ticks::{
    CapturedTickAttempt, ChunkTicks, IndexedRegionQueue, RegionTicks, SavedTick, ScheduleOutcome,
    ScheduledTick, TickRegistry,
};
use native_world::loaded::ChunkPos;

type Order = (i128, i32, i64, u64);

struct NewTickSchedules {
    blocks: Vec<(Order, CollectedBlockTick)>,
    fluids: Vec<(Order, FluidTickKey)>,
    next_sub_tick: i64,
}

/// Immutable live LevelTicks indexes captured at one simulation boundary.
/// Holder readiness, terrain, lighting, entities, POIs and disk state require
/// their own checks before a postprocessing result can be published.
#[derive(Clone)]
pub struct SavedTickQueueCapture {
    session: Arc<()>,
    blocks: Arc<IndexedRegionQueue>,
    fluids: Arc<IndexedRegionQueue>,
    base_time: i64,
    game_time: i64,
    next_sub_tick: i64,
    tick: u64,
    packs: Option<native_persistence::world::WorldDataPackSnapshot>,
}

impl SavedTickQueueCapture {
    pub fn game_time(&self) -> i64 {
        self.game_time
    }

    pub fn next_sub_tick(&self) -> i64 {
        self.next_sub_tick
    }

    /// Actual attachment order of the registered tick containers.
    pub fn registration_order(&self) -> &[ChunkPos] {
        self.blocks.registration_order()
    }

    pub(crate) fn belongs_to_terrain(&self, terrain: &GenerationTerrainStage) -> bool {
        Arc::ptr_eq(&self.session, &terrain.session)
    }

    pub(crate) fn data_packs(&self) -> Option<&native_persistence::world::WorldDataPackSnapshot> {
        self.packs.as_ref()
    }

    /// Deep-copy only on the worker thread, preserving the original index and
    /// heap layouts, pending entries, and the shared sub-tick counter.
    pub(crate) fn isolated_region_ticks(&self) -> Result<RegionTicks> {
        RegionTicks::from_indexed_queues(
            &self.blocks,
            &self.fluids,
            self.game_time,
            self.next_sub_tick,
        )
    }

    /// Compute the only queue change permitted by a no-effect callback. Run
    /// on the worker, since the captured index can cover many loaded chunks.
    pub(crate) fn expected_after_unpack(
        &self,
        center: ChunkPos,
    ) -> Result<(IndexedRegionQueue, IndexedRegionQueue)> {
        let mut blocks = self.blocks.as_ref().clone();
        let mut fluids = self.fluids.as_ref().clone();
        blocks.unpack_container(center, self.game_time)?;
        fluids.unpack_container(center, self.game_time)?;
        Ok((blocks, fluids))
    }

    /// Replay every schedule attempt before the holder unpacks its saved
    /// ticks. Duplicate and missing-container attempts still consume order.
    /// This is deliberately a worker-side check of both complete indexes.
    pub(crate) fn replay_postprocessing_schedules(
        &self,
        center: ChunkPos,
        attempts: &[CapturedTickAttempt],
        next_sub_tick: i64,
        final_blocks: &IndexedRegionQueue,
        final_fluids: &IndexedRegionQueue,
    ) -> Result<()> {
        let mut blocks = self.blocks.as_ref().clone();
        let mut fluids = self.fluids.as_ref().clone();
        let mut expected_sub_tick = self.next_sub_tick;
        for attempt in attempts {
            ensure!(
                attempt.tick.sub_tick == expected_sub_tick,
                "postprocessing schedule order differs from its captured level"
            );
            expected_sub_tick = expected_sub_tick.wrapping_add(1);
            let queue = match attempt.registry {
                TickRegistry::Block => &mut blocks,
                TickRegistry::Fluid => &mut fluids,
            };
            ensure!(
                queue.schedule(attempt.tick.clone())? == attempt.outcome,
                "postprocessing schedule outcome differs from its captured level"
            );
        }
        ensure!(
            expected_sub_tick == next_sub_tick,
            "postprocessing sub-tick counter does not match its schedule attempts"
        );
        blocks.unpack_container(center, self.game_time)?;
        fluids.unpack_container(center, self.game_time)?;
        ensure!(
            &blocks == final_blocks && &fluids == final_fluids,
            "postprocessing schedules do not reproduce the captured indexes"
        );
        Ok(())
    }
}

impl LoadedSession {
    /// Apply a completed saved-chunk callback only when postprocessing made
    /// no block, lighting, entity, or new scheduling changes. The real holder
    /// future stays waiting while the worker runs and becomes available only
    /// after both live tick queues have been activated. A no-effect result may
    /// outlive its captured tick: saved delays are unpacked at the current
    /// level time after rechecking the live queues. Completed marks are still
    /// not saveable until the coordinated persistence path exists. The caller
    /// must preserve the actual holder callback and simulation tick order;
    /// surviving a later tick does not prove that dispatching later is correct.
    pub fn apply_no_effect_saved_ticking(
        &mut self,
        future: &mut crate::ticking_preparation::TickingPreparation,
        terrain: &GenerationTerrainStage,
        result: &crate::worldgen::saved_postprocessing::SavedTickingResult,
    ) -> Result<()> {
        let capture = result.verify_no_effect_live_callback(
            terrain,
            future,
            self.water_source_conversion,
            crate::worldgen::GeneratedLavaRules {
                fast_lava: self.fast_lava,
                source_conversion: self.lava_source_conversion,
            },
        )?;
        let stamp = capture
            .data_packs()
            .context("captured data packs are unavailable")?
            .stamp()
            .clone();
        let _read_lease = stamp.lock_current()?;
        self.verify_generation_terrain_stage(terrain)?;
        self.verify_no_effect_tick_environment(capture)?;
        let source = self.verify_ready_saved_ticking_input(future, terrain)?;
        let dispatched = future.dispatch(
            |position| {
                self.activate_pending_saved_ticks(position)?;
                self.mark_saved_ticking_completed(position, source);
                Ok(())
            },
            |_| Ok(()),
            |_| Ok(()),
        )?;
        ensure!(dispatched, "saved ticking callback was not queued");
        Ok(())
    }

    /// Apply a callback whose only effects are scheduled ticks. The caller
    /// must run it at the holder's original executor position; the exact tick
    /// and queues are required because delayed replay could reorder callbacks.
    /// Completed marks still cannot be written to disk.
    pub fn apply_scheduled_saved_ticking(
        &mut self,
        future: &mut crate::ticking_preparation::TickingPreparation,
        terrain: &GenerationTerrainStage,
        result: &crate::worldgen::saved_postprocessing::SavedTickingResult,
    ) -> Result<()> {
        let (capture, attempts) = result.verify_scheduled_live_callback(
            terrain,
            future,
            self.water_source_conversion,
            crate::worldgen::GeneratedLavaRules {
                fast_lava: self.fast_lava,
                source_conversion: self.lava_source_conversion,
            },
        )?;
        let stamp = capture
            .data_packs()
            .context("captured data packs are unavailable")?
            .stamp()
            .clone();
        let _read_lease = stamp.lock_current()?;
        self.verify_generation_terrain_stage(terrain)?;
        self.verify_saved_tick_queues(capture)?;
        let source = self.verify_ready_saved_ticking_input(future, terrain)?;
        let center = future.center();
        let plan = self.plan_postprocessing_schedules(
            capture,
            center,
            attempts,
            result.region.ticks().next_sub_tick(),
        )?;
        let pending = self
            .pending_saved_tick_activation
            .get(&center)
            .context("saved chunk ticks are not pending")?;
        let (block_moves, fluid_moves, _, _) =
            self.plan_pending_saved_tick_activation(center, pending, self.scheduled_tick())?;
        let moved_blocks: BTreeSet<_> = block_moves.iter().map(|entry| entry.new).collect();
        let moved_fluids: BTreeSet<_> = fluid_moves.iter().map(|entry| entry.new).collect();
        ensure!(
            plan.blocks
                .iter()
                .all(|(order, _)| !moved_blocks.contains(order))
                && plan
                    .fluids
                    .iter()
                    .all(|(order, _)| !moved_fluids.contains(order)),
            "postprocessing schedule collides with a saved tick activation"
        );
        let dispatched = future.dispatch(
            |position| {
                // The background replay already computed the exact indexes.
                // Move the typed pending entries before installing new ones,
                // then swap in those indexes without cloning them on this thread.
                self.activate_pending_saved_ticks_inner(position, false)?;
                self.install_postprocessing_schedules(&plan);
                self.generated_block_order
                    .as_mut()
                    .expect("validated block index")
                    .queue = Arc::clone(&result.block_ticks);
                self.generated_fluid_order
                    .as_mut()
                    .expect("validated fluid index")
                    .queue = Arc::clone(&result.fluid_ticks);
                self.mark_saved_ticking_completed(position, source);
                Ok(())
            },
            |_| Ok(()),
            |_| Ok(()),
        )?;
        ensure!(dispatched, "saved ticking callback was not queued");
        Ok(())
    }

    fn verify_ready_saved_ticking_input(
        &self,
        future: &crate::ticking_preparation::TickingPreparation,
        terrain: &GenerationTerrainStage,
    ) -> Result<[u8; 32]> {
        let center = future.center();
        let dependencies = crate::ticking_preparation::ticking_dependencies(center);
        ensure!(
            future.dependencies().map(|(pos, _)| pos).eq(dependencies),
            "saved ticking holder dependencies changed"
        );
        for position in dependencies {
            let current = self
                .world
                .chunks
                .get(&position)
                .context("FULL neighbor is not resident")?;
            ensure!(
                current.status.as_deref() == Some("minecraft:full")
                    && terrain
                        .chunks
                        .get(&position)
                        .and_then(|chunk| chunk.live.as_ref())
                        .is_some_and(|captured| Arc::ptr_eq(captured, current)),
                "FULL neighbor changed during saved ticking preparation"
            );
            ensure!(
                self.generated_block_order
                    .as_ref()
                    .and_then(|order| order.queue.container(position))
                    .is_some()
                    && self
                        .generated_fluid_order
                        .as_ref()
                        .and_then(|order| order.queue.container(position))
                        .is_some(),
                "FULL neighbor tick containers are no longer registered"
            );
        }
        ensure!(
            matches!(
                self.world.saved_block_entities.get(&center),
                Some(native_world::loaded::SavedBlockEntities::Empty)
            ) && !self.world.live_block_entities.contains_key(&center),
            "saved block entities need live reconciliation"
        );
        let source = self
            .world
            .saved_chunk_source(center)
            .context("saved chunk source is missing")?;
        let pending = self
            .pending_saved_tick_activation
            .get(&center)
            .context("saved chunk ticks are not pending")?;
        let (_, _, block_time, fluid_time) =
            self.plan_pending_saved_tick_activation(center, pending, self.scheduled_tick())?;
        ensure!(
            block_time == Some(self.game_time) && fluid_time == Some(self.game_time),
            "saved ticking clock changed"
        );
        Ok(source)
    }

    fn mark_saved_ticking_completed(&mut self, position: ChunkPos, source: [u8; 32]) {
        self.checked_chunk_preparation.insert(position, source);
        self.rejected_chunk_preparation.remove(&position);
        // Completed marks still need coordinated persistence.
        self.tick_dirty_chunks.insert(position);
        if self.unresolved_chunk_preparation.is_none() {
            self.unresolved_chunk_preparation = Some(format!(
                "chunk {}, {}: completed postprocessing is not yet coordinated for saving",
                position.x, position.z
            ));
        }
        self.random_tick_revision = None;
    }

    fn plan_postprocessing_schedules(
        &self,
        capture: &SavedTickQueueCapture,
        center: ChunkPos,
        attempts: &[CapturedTickAttempt],
        next_sub_tick: i64,
    ) -> Result<NewTickSchedules> {
        let dependencies = crate::ticking_preparation::ticking_dependencies(center);
        let mut block_sequence = self.block_tick_sequence;
        let mut fluid_sequence = self.fluid_sequence;
        let mut seen_blocks = BTreeSet::new();
        let mut seen_fluids = BTreeSet::new();
        let mut blocks = Vec::new();
        let mut fluids = Vec::new();
        for attempt in attempts {
            if attempt.outcome == ScheduleOutcome::MissingChunk {
                continue;
            }
            let tick = &attempt.tick;
            let pos = ChunkPos {
                x: tick.position[0].div_euclid(16),
                z: tick.position[2].div_euclid(16),
            };
            ensure!(
                dependencies.contains(&pos),
                "postprocessing schedules a chunk outside the captured FULL neighbors"
            );
            let due = i128::from(tick.trigger) - i128::from(capture.base_time);
            match attempt.registry {
                TickRegistry::Block => {
                    let entry = generated_block_key(tick)
                        .context("unsupported postprocessing block tick")?;
                    let pending = self.has_scheduled_block(entry);
                    if attempt.outcome == ScheduleOutcome::Duplicate {
                        ensure!(
                            pending || seen_blocks.contains(&entry.identity()),
                            "duplicate postprocessing block tick is absent from the session"
                        );
                        continue;
                    }
                    ensure!(
                        !pending && seen_blocks.insert(entry.identity()),
                        "postprocessing block tick conflicts with the session"
                    );
                    let order = (due, tick.priority, tick.sub_tick, block_sequence);
                    ensure!(
                        !self.has_block_order(entry, order),
                        "postprocessing block order is occupied"
                    );
                    block_sequence = block_sequence.wrapping_add(1);
                    blocks.push((order, entry));
                }
                TickRegistry::Fluid => {
                    let key = generated_fluid_key(tick)
                        .context("unsupported postprocessing fluid tick")?;
                    let pending = self.fluid_pending.contains(&key);
                    if attempt.outcome == ScheduleOutcome::Duplicate {
                        ensure!(
                            pending || seen_fluids.contains(&key),
                            "duplicate postprocessing fluid tick is absent from the session"
                        );
                        continue;
                    }
                    ensure!(
                        !pending && seen_fluids.insert(key),
                        "postprocessing fluid tick conflicts with the session"
                    );
                    let order = (due, tick.priority, tick.sub_tick, fluid_sequence);
                    ensure!(
                        !self.fluid_ticks.contains_key(&order),
                        "postprocessing fluid order is occupied"
                    );
                    fluid_sequence = fluid_sequence.wrapping_add(1);
                    fluids.push((order, key));
                }
            }
        }
        Ok(NewTickSchedules {
            blocks,
            fluids,
            next_sub_tick,
        })
    }

    fn has_scheduled_block(&self, entry: CollectedBlockTick) -> bool {
        match entry {
            CollectedBlockTick::Air(key) => self.air_pending.contains(&key),
            CollectedBlockTick::Sand(key) => self.sand_pending.contains(&key),
            CollectedBlockTick::Button(key) => self.button_pending.contains(&key),
            CollectedBlockTick::Soil(pos, kind) => {
                self.soil_pending.contains(&(pos.x, pos.y, pos.z, kind))
            }
            CollectedBlockTick::Lamp(key) => self.lamp_pending.contains(&key),
            CollectedBlockTick::Ice(key) => self.ice_pending.contains(&key),
            CollectedBlockTick::Bubble(key) => self.bubble_pending.contains(&key),
            CollectedBlockTick::Sensor(key) => self.sensor_pending.contains(&key),
            CollectedBlockTick::Leaf(key) => self.leaf_pending.contains(&key),
            CollectedBlockTick::Speleothem(key) => self.speleothem_pending.contains(&key),
        }
    }

    fn has_block_order(&self, entry: CollectedBlockTick, order: Order) -> bool {
        match entry {
            CollectedBlockTick::Air(_) => self.air_ticks.contains_key(&order),
            CollectedBlockTick::Sand(_) => self.sand_ticks.contains_key(&order),
            CollectedBlockTick::Button(_) => self.button_ticks.contains_key(&order),
            CollectedBlockTick::Soil(..) => self.soil_ticks.contains_key(&order),
            CollectedBlockTick::Lamp(_) => self.lamp_ticks.contains_key(&order),
            CollectedBlockTick::Ice(_) => self.ice_ticks.contains_key(&order),
            CollectedBlockTick::Bubble(_) => self.bubble_ticks.contains_key(&order),
            CollectedBlockTick::Sensor(_) => self.sensor_ticks.contains_key(&order),
            CollectedBlockTick::Leaf(_) => self.leaf_ticks.contains_key(&order),
            CollectedBlockTick::Speleothem(_) => self.speleothem_ticks.contains_key(&order),
        }
    }

    fn install_postprocessing_schedules(&mut self, plan: &NewTickSchedules) {
        for &(order, entry) in &plan.blocks {
            let (_, [x, _, z]) = entry.identity();
            self.mark_tick_chunk_dirty(x, z);
            match entry {
                CollectedBlockTick::Air(key) => {
                    self.air_pending.insert(key);
                    self.air_ticks.insert(order, key);
                }
                CollectedBlockTick::Sand(key) => {
                    self.sand_pending.insert(key);
                    self.sand_ticks.insert(order, key);
                }
                CollectedBlockTick::Button(key) => {
                    self.button_pending.insert(key);
                    self.button_ticks.insert(order, key);
                }
                CollectedBlockTick::Soil(pos, kind) => {
                    self.soil_pending.insert((pos.x, pos.y, pos.z, kind));
                    self.soil_ticks.insert(order, (pos, kind));
                }
                CollectedBlockTick::Lamp(key) => {
                    self.lamp_pending.insert(key);
                    self.lamp_ticks.insert(order, key);
                }
                CollectedBlockTick::Ice(key) => {
                    self.ice_pending.insert(key);
                    self.ice_ticks.insert(order, key);
                }
                CollectedBlockTick::Bubble(key) => {
                    self.bubble_pending.insert(key);
                    self.bubble_ticks.insert(order, key);
                }
                CollectedBlockTick::Sensor(key) => {
                    self.sensor_pending.insert(key);
                    self.sensor_ticks.insert(order, key);
                }
                CollectedBlockTick::Leaf(key) => {
                    self.leaf_pending.insert(key);
                    self.leaf_ticks.insert(order, key);
                }
                CollectedBlockTick::Speleothem(key) => {
                    self.speleothem_pending.insert(key);
                    self.speleothem_ticks.insert(order, key);
                }
            }
        }
        for &(order, key) in &plan.fluids {
            self.mark_tick_chunk_dirty(key.x, key.z);
            self.fluid_pending.insert(key);
            self.fluid_ticks.insert(order, ScheduledFluid { key });
        }
        self.block_tick_sequence = self
            .block_tick_sequence
            .wrapping_add(plan.blocks.len() as u64);
        self.fluid_sequence = self.fluid_sequence.wrapping_add(plan.fluids.len() as u64);
        self.next_sub_tick = plan.next_sub_tick;
    }

    /// A callback proven to have no scheduled or world effects does not read
    /// the live queues. Other ticks may advance while it is prepared, but its
    /// session, data packs and indexed clock must still match.
    fn verify_no_effect_tick_environment(&self, captured: &SavedTickQueueCapture) -> Result<()> {
        let current_packs = self
            .world_data_packs
            .as_ref()
            .context("session data packs are not captured")?;
        let captured_packs = captured
            .packs
            .as_ref()
            .context("captured data packs are unavailable")?;
        ensure!(
            self.require_saved_chunk_sources
                && self.initial_game_time_set
                && !self.advancing_tick
                && Arc::ptr_eq(&self.generation_identity, &captured.session)
                && self.tick >= captured.tick
                && Arc::ptr_eq(current_packs.packs(), captured_packs.packs())
                && current_packs.selection() == captured_packs.selection()
                && current_packs.stamp().is_current(),
            "saved ticking session or data packs changed"
        );
        let blocks = self
            .generated_block_order
            .as_ref()
            .context("live block tick index is unavailable")?;
        let fluids = self
            .generated_fluid_order
            .as_ref()
            .context("live fluid tick index is unavailable")?;
        let time = i64::try_from(i128::from(captured.base_time) + self.scheduled_tick())
            .context("live tick clock is outside the indexed range")?;
        ensure!(
            blocks.base_time == captured.base_time
                && fluids.base_time == captured.base_time
                && time == self.game_time,
            "live tick indexes or clock changed"
        );
        Ok(())
    }

    /// Capture the current registered queues without cloning every tick on the
    /// simulation thread. Later mutations use copy-on-write per container.
    pub fn capture_saved_tick_queues(&self) -> Result<SavedTickQueueCapture> {
        ensure!(
            self.require_saved_chunk_sources && self.initial_game_time_set && !self.advancing_tick,
            "saved tick queues can only be captured at a session tick boundary"
        );
        let blocks = self
            .generated_block_order
            .as_ref()
            .context("live block tick index is unavailable")?;
        let fluids = self
            .generated_fluid_order
            .as_ref()
            .context("live fluid tick index is unavailable")?;
        ensure!(
            blocks.base_time == fluids.base_time,
            "live tick indexes use different clocks"
        );
        let time = i64::try_from(i128::from(blocks.base_time) + self.scheduled_tick())
            .context("live tick clock is outside the indexed range")?;
        ensure!(
            time == self.game_time,
            "live tick clock differs from level time"
        );
        ensure!(
            blocks.queue.registration_order() == fluids.queue.registration_order(),
            "live block and fluid registrations differ"
        );
        Ok(SavedTickQueueCapture {
            session: Arc::clone(&self.generation_identity),
            blocks: Arc::clone(&blocks.queue),
            fluids: Arc::clone(&fluids.queue),
            base_time: blocks.base_time,
            game_time: self.game_time,
            next_sub_tick: self.next_sub_tick,
            tick: self.tick,
            packs: self.world_data_packs.clone(),
        })
    }

    /// A completed worker result may only use this snapshot if neither the
    /// level clock nor either registered queue changed since capture.
    pub fn verify_saved_tick_queues(&self, captured: &SavedTickQueueCapture) -> Result<()> {
        ensure!(
            self.require_saved_chunk_sources
                && self.initial_game_time_set
                && !self.advancing_tick
                && Arc::ptr_eq(&self.generation_identity, &captured.session)
                && self.tick == captured.tick
                && self.game_time == captured.game_time
                && self.next_sub_tick == captured.next_sub_tick
                && match (&self.world_data_packs, &captured.packs) {
                    (Some(current), Some(previous)) => {
                        Arc::ptr_eq(current.packs(), previous.packs())
                            && current.selection() == previous.selection()
                            && current.stamp().is_current()
                    }
                    (None, None) => true,
                    _ => false,
                },
            "saved tick session or clock changed"
        );
        let blocks = self
            .generated_block_order
            .as_ref()
            .context("live block tick index is unavailable")?;
        let fluids = self
            .generated_fluid_order
            .as_ref()
            .context("live fluid tick index is unavailable")?;
        ensure!(
            blocks.base_time == captured.base_time
                && fluids.base_time == captured.base_time
                && Arc::ptr_eq(&blocks.queue, &captured.blocks)
                && Arc::ptr_eq(&fluids.queue, &captured.fluids),
            "saved tick queues changed"
        );
        Ok(())
    }
}

struct BlockMove {
    old: Order,
    new: Order,
    tick: CollectedBlockTick,
}

struct FluidMove {
    old: Order,
    new: Order,
    tick: ScheduledFluid,
}

fn saved_blocks(pending: &PendingSavedTickActivation) -> Vec<SavedTick> {
    pending
        .blocks
        .iter()
        .map(|tick| SavedTick {
            kind: tick.block.clone(),
            position: tick.position,
            delay: tick.delay,
            priority: tick.priority,
        })
        .collect()
}

fn saved_fluids(pending: &PendingSavedTickActivation) -> Vec<SavedTick> {
    pending
        .fluids
        .iter()
        .map(|tick| SavedTick {
            kind: tick.fluid.clone(),
            position: tick.position,
            delay: tick.delay,
            priority: tick.priority,
        })
        .collect()
}

fn validate_index(
    order: &GeneratedTickOrder,
    pos: ChunkPos,
    saved: Vec<SavedTick>,
    now: i128,
) -> Result<i64> {
    let time = i64::try_from(i128::from(order.base_time) + now)
        .context("saved tick activation time is outside the indexed clock")?;
    let expected = ChunkTicks::from_saved(pos, saved)?;
    let pending = expected
        .pending_saved()
        .context("saved ticks were activated")?;
    ensure!(
        pending
            .iter()
            .all(|tick| time.checked_add(i64::from(tick.delay)).is_some()),
        "saved tick activation would overflow the indexed clock"
    );
    let existing = order
        .queue
        .container(pos)
        .context("pending saved tick container is not resident")?;
    ensure!(
        existing.pending_saved() == Some(pending),
        "pending saved tick container changed"
    );
    Ok(time)
}

impl LoadedSession {
    /// Check every original entry and both indexes before changing any queue.
    fn plan_pending_saved_tick_activation(
        &self,
        pos: ChunkPos,
        pending: &PendingSavedTickActivation,
        now: i128,
    ) -> Result<(Vec<BlockMove>, Vec<FluidMove>, Option<i64>, Option<i64>)> {
        ensure!(
            now >= pending.registered_at,
            "saved tick clock moved backwards"
        );
        ensure!(
            self.world
                .saved_block_ticks
                .get(&pos)
                .map_or(&[][..], Vec::as_slice)
                == pending.blocks.as_slice()
                && self
                    .world
                    .saved_fluid_ticks
                    .get(&pos)
                    .map_or(&[][..], Vec::as_slice)
                    == pending.fluids.as_slice(),
            "pending saved tick source changed"
        );
        let blocks = ChunkTicks::from_saved(pos, saved_blocks(pending))?;
        let fluids = ChunkTicks::from_saved(pos, saved_fluids(pending))?;
        let block_time = self
            .generated_block_order
            .as_ref()
            .map(|order| validate_index(order, pos, saved_blocks(pending), now))
            .transpose()?;
        let fluid_time = self
            .generated_fluid_order
            .as_ref()
            .map(|order| validate_index(order, pos, saved_fluids(pending), now))
            .transpose()?;
        let block_entries = blocks
            .pending_saved()
            .context("missing saved block ticks")?;
        let fluid_entries = fluids
            .pending_saved()
            .context("missing saved fluid ticks")?;
        let mut block_moves = Vec::with_capacity(block_entries.len());
        for (index, tick) in block_entries.iter().enumerate() {
            let subtick = index as i64 - block_entries.len() as i64;
            let due = pending.registered_at + i128::from(tick.delay);
            let start = (due, tick.priority, subtick, 0);
            let end = (due, tick.priority, subtick, u64::MAX);
            let entry = generated_block_key(&ScheduledTick {
                kind: tick.kind.clone(),
                position: tick.position,
                trigger: 0,
                priority: tick.priority,
                sub_tick: subtick,
            })
            .context("unsupported pending saved block tick")?;
            macro_rules! find {
                ($queue:expr, $make:expr) => {
                    $queue.range(start..=end).find_map(|(order, &key)| {
                        let actual: CollectedBlockTick = $make(key);
                        (actual == entry).then_some(*order)
                    })
                };
            }
            let old = match entry {
                CollectedBlockTick::Air(_) => find!(self.air_ticks, CollectedBlockTick::Air),
                CollectedBlockTick::Sand(_) => find!(self.sand_ticks, CollectedBlockTick::Sand),
                CollectedBlockTick::Button(_) => {
                    find!(self.button_ticks, CollectedBlockTick::Button)
                }
                CollectedBlockTick::Soil(..) => {
                    find!(self.soil_ticks, |(p, kind)| CollectedBlockTick::Soil(
                        p, kind
                    ))
                }
                CollectedBlockTick::Lamp(_) => find!(self.lamp_ticks, CollectedBlockTick::Lamp),
                CollectedBlockTick::Ice(_) => find!(self.ice_ticks, CollectedBlockTick::Ice),
                CollectedBlockTick::Bubble(_) => {
                    find!(self.bubble_ticks, CollectedBlockTick::Bubble)
                }
                CollectedBlockTick::Sensor(_) => {
                    find!(self.sensor_ticks, CollectedBlockTick::Sensor)
                }
                CollectedBlockTick::Leaf(_) => find!(self.leaf_ticks, CollectedBlockTick::Leaf),
                CollectedBlockTick::Speleothem(_) => {
                    find!(self.speleothem_ticks, CollectedBlockTick::Speleothem)
                }
            }
            .context("pending saved block tick changed")?;
            let new = (now + i128::from(tick.delay), old.1, old.2, old.3);
            macro_rules! vacant {
                ($queue:expr) => {
                    old == new || !$queue.contains_key(&new)
                };
            }
            ensure!(
                match entry {
                    CollectedBlockTick::Air(_) => vacant!(self.air_ticks),
                    CollectedBlockTick::Sand(_) => vacant!(self.sand_ticks),
                    CollectedBlockTick::Button(_) => vacant!(self.button_ticks),
                    CollectedBlockTick::Soil(..) => vacant!(self.soil_ticks),
                    CollectedBlockTick::Lamp(_) => vacant!(self.lamp_ticks),
                    CollectedBlockTick::Ice(_) => vacant!(self.ice_ticks),
                    CollectedBlockTick::Bubble(_) => vacant!(self.bubble_ticks),
                    CollectedBlockTick::Sensor(_) => vacant!(self.sensor_ticks),
                    CollectedBlockTick::Leaf(_) => vacant!(self.leaf_ticks),
                    CollectedBlockTick::Speleothem(_) => vacant!(self.speleothem_ticks),
                },
                "pending saved block tick would overwrite another schedule"
            );
            block_moves.push(BlockMove {
                old,
                new,
                tick: entry,
            });
        }
        let mut fluid_moves = Vec::with_capacity(fluid_entries.len());
        for (index, tick) in fluid_entries.iter().enumerate() {
            let subtick = index as i64 - fluid_entries.len() as i64;
            let due = pending.registered_at + i128::from(tick.delay);
            let start = (due, tick.priority, subtick, 0);
            let end = (due, tick.priority, subtick, u64::MAX);
            let key = generated_fluid_key(&ScheduledTick {
                kind: tick.kind.clone(),
                position: tick.position,
                trigger: 0,
                priority: tick.priority,
                sub_tick: subtick,
            })
            .context("unsupported pending saved fluid tick")?;
            let (&old, &scheduled) = self
                .fluid_ticks
                .range(start..=end)
                .find(|(_, scheduled)| scheduled.key == key)
                .context("pending saved fluid tick changed")?;
            let new = (now + i128::from(tick.delay), old.1, old.2, old.3);
            ensure!(
                old == new || !self.fluid_ticks.contains_key(&new),
                "pending saved fluid tick would overwrite another schedule"
            );
            fluid_moves.push(FluidMove {
                old,
                new,
                tick: scheduled,
            });
        }
        Ok((block_moves, fluid_moves, block_time, fluid_time))
    }

    pub(super) fn activate_pending_saved_ticks(&mut self, pos: ChunkPos) -> Result<()> {
        self.activate_pending_saved_ticks_inner(pos, true)
    }

    fn activate_pending_saved_ticks_inner(
        &mut self,
        pos: ChunkPos,
        unpack_indexes: bool,
    ) -> Result<()> {
        let Some(pending) = self.pending_saved_tick_activation.get(&pos) else {
            return Ok(());
        };
        let now = self.scheduled_tick();
        let (blocks, fluids, block_time, fluid_time) =
            self.plan_pending_saved_tick_activation(pos, pending, now)?;
        for change in blocks {
            macro_rules! move_tick {
                ($queue:expr) => {{
                    let tick = $queue.remove(&change.old).expect("validated saved tick");
                    let previous = $queue.insert(change.new, tick);
                    debug_assert!(previous.is_none());
                }};
            }
            match change.tick {
                CollectedBlockTick::Air(_) => move_tick!(self.air_ticks),
                CollectedBlockTick::Sand(_) => move_tick!(self.sand_ticks),
                CollectedBlockTick::Button(_) => move_tick!(self.button_ticks),
                CollectedBlockTick::Soil(..) => move_tick!(self.soil_ticks),
                CollectedBlockTick::Lamp(_) => move_tick!(self.lamp_ticks),
                CollectedBlockTick::Ice(_) => move_tick!(self.ice_ticks),
                CollectedBlockTick::Bubble(_) => move_tick!(self.bubble_ticks),
                CollectedBlockTick::Sensor(_) => move_tick!(self.sensor_ticks),
                CollectedBlockTick::Leaf(_) => move_tick!(self.leaf_ticks),
                CollectedBlockTick::Speleothem(_) => move_tick!(self.speleothem_ticks),
            }
        }
        for change in fluids {
            let tick = self
                .fluid_ticks
                .remove(&change.old)
                .expect("validated saved fluid tick");
            debug_assert_eq!(tick.key, change.tick.key);
            let previous = self.fluid_ticks.insert(change.new, tick);
            debug_assert!(previous.is_none());
        }
        if let Some(time) = block_time.filter(|_| unpack_indexes) {
            Arc::make_mut(
                &mut self
                    .generated_block_order
                    .as_mut()
                    .expect("validated saved block index")
                    .queue,
            )
            .unpack_container(pos, time)
            .expect("validated saved block container");
        }
        if let Some(time) = fluid_time.filter(|_| unpack_indexes) {
            Arc::make_mut(
                &mut self
                    .generated_fluid_order
                    .as_mut()
                    .expect("validated saved fluid index")
                    .queue,
            )
            .unpack_container(pos, time)
            .expect("validated saved fluid container");
        }
        self.pending_saved_tick_activation.remove(&pos);
        Ok(())
    }
}
