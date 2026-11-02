//! 26.3 fallen-tree layout, including its independent stump placement.
use crate::{placed_feature::IntProvider, tree_fancy_trunk::LogAxis};
use anyhow::{Context, Result, ensure};
use native_core::random::DecorationRandom;
use serde_json::Value;

pub trait FallenTreeWorld {
    fn replaceable(&mut self, pos: [i32; 3]) -> Result<bool>;
    /// Test the upper face of the block below, using the supplied tree position.
    fn sturdy_below(&mut self, pos: [i32; 3]) -> Result<bool>;
    /// Sample the trunk provider and perform vanilla's block-and-neighbor update.
    fn place_log(
        &mut self,
        pos: [i32; 3],
        axis: Option<LogAxis>,
        random: &mut DecorationRandom,
    ) -> Result<()>;
    /// Mark up to two non-air blocks above the placed log for postprocessing.
    fn postprocess_above(&mut self, pos: [i32; 3]) -> Result<()>;
    fn decorate_stump(&mut self, pos: [i32; 3], random: &mut DecorationRandom) -> Result<()>;
    fn decorate_logs(&mut self, logs: &[[i32; 3]], random: &mut DecorationRandom) -> Result<()>;
}

pub struct FallenTreeLayout {
    length: IntProvider,
}
impl FallenTreeLayout {
    /// Decode the 0..=16 vanilla log-length provider.
    pub fn decode(value: &Value) -> Result<Self> {
        let length = IntProvider::parse(
            value
                .get("log_length")
                .context("missing fallen log length")?,
            0,
        )?;
        let (min, max) = length.bounds();
        ensure!(
            (0..=16).contains(&min) && (0..=16).contains(&max) && min <= max,
            "invalid fallen log length"
        );
        Ok(Self { length })
    }
    /// Java returns success even if the horizontal log cannot be placed.
    pub fn place(
        &self,
        world: &mut impl FallenTreeWorld,
        random: &mut DecorationRandom,
        origin: [i32; 3],
    ) -> Result<bool> {
        world.place_log(origin, None, random)?;
        world.postprocess_above(origin)?;
        world.decorate_stump(origin, random)?;
        let (dx, dz, axis) = match random.next_int_bound(4).unwrap() {
            0 => (0, -1, LogAxis::Z),
            1 => (1, 0, LogAxis::X),
            2 => (0, 1, LogAxis::Z),
            _ => (-1, 0, LogAxis::X),
        };
        let length = self.length.sample(random) - 2;
        let gap = 2 + random.next_int_bound(2).unwrap();
        let mut start = [
            origin[0].wrapping_add(dx * gap),
            origin[1].wrapping_add(1),
            origin[2].wrapping_add(dz * gap),
        ];
        for _ in 0..6 {
            if world.replaceable(start)? && world.sturdy_below(start)? {
                break;
            }
            start[1] = start[1].wrapping_sub(1);
        }
        let mut current = start;
        let mut gaps = 0;
        for _ in 0..length {
            if !world.replaceable(current)? {
                return Ok(true);
            }
            if world.sturdy_below(current)? {
                gaps = 0;
            } else {
                gaps += 1;
                if gaps > 2 {
                    return Ok(true);
                }
            }
            current[0] = current[0].wrapping_add(dx);
            current[2] = current[2].wrapping_add(dz);
        }
        let mut logs = Vec::with_capacity(length.max(0) as usize);
        for i in 0..length {
            let pos = [
                start[0].wrapping_add(dx * i),
                start[1],
                start[2].wrapping_add(dz * i),
            ];
            world.place_log(pos, Some(axis), random)?;
            world.postprocess_above(pos)?;
            logs.push(pos);
        }
        world.decorate_logs(&logs, random)?;
        Ok(true)
    }
}
