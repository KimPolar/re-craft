//! Activate saved tick delays only after the corresponding preparation check.
//!
//! This does not establish the Java holder's ticking readiness or authorize a
//! save. New schedules remain separate from the original pending entries.

use anyhow::{Context, Result, ensure};

use super::{
    CollectedBlockTick, GeneratedTickOrder, LoadedSession, PendingSavedTickActivation,
    ScheduledFluid, generated_block_key, generated_fluid_key,
};
use crate::worldgen::chunk_ticks::{ChunkTicks, SavedTick, ScheduledTick};
use native_world::loaded::ChunkPos;

type Order = (i128, i32, i64, u64);

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
        if let Some(time) = block_time {
            self.generated_block_order
                .as_mut()
                .expect("validated saved block index")
                .queue
                .unpack_container(pos, time)
                .expect("validated saved block container");
        }
        if let Some(time) = fluid_time {
            self.generated_fluid_order
                .as_mut()
                .expect("validated saved fluid index")
                .queue
                .unpack_container(pos, time)
                .expect("validated saved fluid container");
        }
        self.pending_saved_tick_activation.remove(&pos);
        Ok(())
    }
}
