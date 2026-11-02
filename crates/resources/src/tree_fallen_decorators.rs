//! Trunk vines and log-attached decorations used by fallen trees.
use crate::tree_leaf_update::decorator_positions;
use anyhow::{Result, ensure};
use native_core::random::DecorationRandom;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    Down,
    Up,
    North,
    South,
    West,
    East,
}
impl Direction {
    fn offset(self, p: [i32; 3]) -> [i32; 3] {
        let (x, y, z) = match self {
            Self::Down => (0, -1, 0),
            Self::Up => (0, 1, 0),
            Self::North => (0, 0, -1),
            Self::South => (0, 0, 1),
            Self::West => (-1, 0, 0),
            Self::East => (1, 0, 0),
        };
        [
            p[0].wrapping_add(x),
            p[1].wrapping_add(y),
            p[2].wrapping_add(z),
        ]
    }
}
pub trait FallenDecorationWorld {
    fn is_air(&mut self, pos: [i32; 3]) -> Result<bool>;
    /// The vine's attachment points back toward its supporting log.
    fn place_vine(&mut self, pos: [i32; 3], attachment: Direction) -> Result<()>;
    /// Sample the active state provider and perform the decorator write.
    fn place_attached(&mut self, pos: [i32; 3], random: &mut DecorationRandom) -> Result<()>;
}

pub fn place_trunk_vines(
    world: &mut impl FallenDecorationWorld,
    random: &mut DecorationRandom,
    logs: &[[i32; 3]],
) -> Result<()> {
    for log in decorator_positions(logs)? {
        for (direction, attachment) in [
            (Direction::West, Direction::East),
            (Direction::East, Direction::West),
            (Direction::North, Direction::South),
            (Direction::South, Direction::North),
        ] {
            if random.next_int_bound(3).unwrap() > 0 {
                let pos = direction.offset(log);
                if world.is_air(pos)? {
                    world.place_vine(pos, attachment)?;
                }
            }
        }
    }
    Ok(())
}

pub fn place_attached_to_logs(
    world: &mut impl FallenDecorationWorld,
    random: &mut DecorationRandom,
    logs: &[[i32; 3]],
    probability: f32,
    directions: &[Direction],
) -> Result<()> {
    ensure!(
        probability.is_finite() && (0.0..=1.0).contains(&probability),
        "invalid attached decoration probability"
    );
    ensure!(
        !directions.is_empty() && directions.len() <= 4096,
        "invalid attached decoration directions"
    );
    let mut logs = decorator_positions(logs)?;
    for i in (2..=logs.len()).rev() {
        let j = random.next_int_bound(i32::try_from(i)?).unwrap() as usize;
        logs.swap(i - 1, j);
    }
    for log in logs {
        let direction = directions[random
            .next_int_bound(i32::try_from(directions.len())?)
            .unwrap() as usize];
        let pos = direction.offset(log);
        if random.next_float() <= probability && world.is_air(pos)? {
            world.place_attached(pos, random)?;
        }
    }
    Ok(())
}
