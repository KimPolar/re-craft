//! 26.3 bee-nest tree decoration. The world owns block-entity persistence.
use crate::tree_leaf_update::decorator_positions;
use anyhow::{Result, ensure};
use native_core::random::DecorationRandom;

pub trait BeehiveWorld {
    fn is_air(&mut self, pos: [i32; 3]) -> Result<bool>;
    /// Place a south-facing nest and return whether its block entity exists.
    fn place_bee_nest(&mut self, pos: [i32; 3]) -> Result<bool>;
    /// Store generated bees, with their ticks-in-hive values and minimum 600 ticks.
    fn store_bees(&mut self, pos: [i32; 3], ticks: &[i32]) -> Result<()>;
}

pub struct BeehiveDecorator {
    probability: f32,
}
impl BeehiveDecorator {
    pub fn new(probability: f32) -> Result<Self> {
        ensure!(
            probability.is_finite() && (0.0..=1.0).contains(&probability),
            "invalid beehive probability"
        );
        Ok(Self { probability })
    }

    /// Run after trunk and leaf placement and before tree leaf finalization.
    pub fn place(
        &self,
        world: &mut impl BeehiveWorld,
        random: &mut DecorationRandom,
        logs: &[[i32; 3]],
        leaves: &[[i32; 3]],
    ) -> Result<()> {
        let logs = decorator_positions(logs)?;
        if logs.is_empty() {
            return Ok(());
        }
        let leaves = decorator_positions(leaves)?;
        if random.next_float() >= self.probability {
            return Ok(());
        }
        let y = if let Some(leaf) = leaves.first() {
            leaf[1].wrapping_sub(1).max(logs[0][1].wrapping_add(1))
        } else {
            logs[0][1]
                .wrapping_add(1)
                .wrapping_add(random.next_int_bound(3).unwrap())
                .min(logs.last().unwrap()[1])
        };
        let mut candidates = Vec::new();
        for log in logs.iter().filter(|pos| pos[1] == y) {
            for [dx, dz] in [[1, 0], [0, 1], [-1, 0]] {
                candidates.push([log[0].wrapping_add(dx), log[1], log[2].wrapping_add(dz)]);
            }
        }
        for i in (2..=candidates.len()).rev() {
            let j = random
                .next_int_bound(i32::try_from(i)?)
                .expect("positive shuffle length") as usize;
            candidates.swap(i - 1, j);
        }
        for pos in candidates {
            let front = [pos[0], pos[1], pos[2].wrapping_add(1)];
            if world.is_air(pos)? && world.is_air(front)? {
                if world.place_bee_nest(pos)? {
                    let count = 2 + random.next_int_bound(2).unwrap();
                    let ticks = (0..count)
                        .map(|_| random.next_int_bound(599).unwrap())
                        .collect::<Vec<_>>();
                    world.store_bees(pos, &ticks)?;
                }
                break;
            }
        }
        Ok(())
    }
}
