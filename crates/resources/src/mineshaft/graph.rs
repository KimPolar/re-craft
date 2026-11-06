//! Recursive piece layout before vertical placement and block generation.

use super::{
    InitialMineshaftRoom, MineshaftPieceKind, MineshaftVariant, choose_piece_kind, find_corridor,
    find_crossing, find_stairs,
};
use crate::structure_start::HorizontalDirection as Direction;
use anyhow::{Result, ensure};
use native_core::{beardifier::BoundingBox, random::LegacyRandom};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MineshaftGraphPieceKind {
    Room {
        entrances: Vec<BoundingBox>,
    },
    Corridor {
        rails: bool,
        spiders: bool,
        sections: i32,
    },
    Crossing {
        two_floors: bool,
    },
    Stairs,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MineshaftGraphPiece {
    pub bounds: BoundingBox,
    pub direction: Option<Direction>,
    pub depth: i32,
    pub kind: MineshaftGraphPieceKind,
}

/// Piece order and entrance geometry before the structure is moved vertically.
/// This is not yet sufficient to write or place a complete mineshaft.
pub struct MineshaftGraph {
    pub variant: MineshaftVariant,
    pub pieces: Vec<MineshaftGraphPiece>,
    occupied: Vec<BoundingBox>,
    random: LegacyRandom,
}

impl MineshaftGraph {
    pub fn new(seed: i64, chunk: [i32; 2], variant: MineshaftVariant) -> Result<Self> {
        let (bounds, variant, random) =
            InitialMineshaftRoom::new(seed, chunk, variant)?.into_parts();
        // Every generated child is within a small bounded distance of the room.
        // Reject coordinates near the integer limit instead of overflowing.
        for axis in [0, 2] {
            ensure!(
                bounds.min[axis] >= i32::MIN + 256 && bounds.max[axis] <= i32::MAX - 256,
                "mineshaft graph coordinates out of range"
            );
        }
        let mut graph = Self {
            variant,
            pieces: vec![MineshaftGraphPiece {
                bounds,
                direction: None,
                depth: 0,
                kind: MineshaftGraphPieceKind::Room {
                    entrances: Vec::new(),
                },
            }],
            occupied: vec![bounds],
            random,
        };
        graph.add_room_children()?;
        Ok(graph)
    }

    /// Retain the random stream for the subsequent vertical placement step.
    pub fn into_parts(self) -> (MineshaftVariant, Vec<MineshaftGraphPiece>, LegacyRandom) {
        (self.variant, self.pieces, self.random)
    }

    fn draw(&mut self, bound: i32) -> i32 {
        self.random.next_int_bound(bound).expect("positive bound")
    }

    fn add_piece(
        &mut self,
        origin: [i32; 3],
        direction: Direction,
        parent_depth: i32,
    ) -> Result<Option<BoundingBox>> {
        let root = self.pieces[0].bounds;
        if parent_depth > 8
            || (origin[0] - root.min[0]).abs() > 80
            || (origin[2] - root.min[2]).abs() > 80
        {
            return Ok(None);
        }
        let kind = choose_piece_kind(&mut self.random);
        let bounds = match kind {
            MineshaftPieceKind::Corridor => {
                find_corridor(&mut self.random, origin, direction, &self.occupied)?
            }
            MineshaftPieceKind::Crossing => {
                find_crossing(&mut self.random, origin, direction, &self.occupied)?
            }
            MineshaftPieceKind::Stairs => find_stairs(origin, direction, &self.occupied)?,
        };
        let Some(bounds) = bounds else {
            return Ok(None);
        };
        let kind = match kind {
            MineshaftPieceKind::Corridor => {
                let rails = self.draw(3) == 0;
                let spiders = !rails && self.draw(23) == 0;
                let axis = if matches!(direction, Direction::North | Direction::South) {
                    2
                } else {
                    0
                };
                MineshaftGraphPieceKind::Corridor {
                    rails,
                    spiders,
                    sections: (bounds.max[axis] - bounds.min[axis] + 1) / 5,
                }
            }
            MineshaftPieceKind::Crossing => MineshaftGraphPieceKind::Crossing {
                two_floors: bounds.max[1] - bounds.min[1] + 1 > 3,
            },
            MineshaftPieceKind::Stairs => MineshaftGraphPieceKind::Stairs,
        };
        let index = self.pieces.len();
        self.occupied.push(bounds);
        self.pieces.push(MineshaftGraphPiece {
            bounds,
            direction: Some(direction),
            depth: parent_depth + 1,
            kind,
        });
        self.add_children(index)?;
        Ok(Some(bounds))
    }

    fn add_room_children(&mut self) -> Result<()> {
        use Direction::*;
        let b = self.pieces[0].bounds;
        let height = (b.max[1] - b.min[1] - 3).max(1);
        for direction in [North, South, West, East] {
            let span = if matches!(direction, North | South) {
                b.max[0] - b.min[0] + 1
            } else {
                b.max[2] - b.min[2] + 1
            };
            let mut pos = 0;
            while pos < span {
                pos += self.draw(span);
                if pos + 3 > span {
                    break;
                }
                let y = b.min[1] + self.draw(height) + 1;
                let origin = match direction {
                    North => [b.min[0] + pos, y, b.min[2] - 1],
                    South => [b.min[0] + pos, y, b.max[2] + 1],
                    West => [b.min[0] - 1, y, b.min[2] + pos],
                    East => [b.max[0] + 1, y, b.min[2] + pos],
                };
                if let Some(child) = self.add_piece(origin, direction, 0)? {
                    let (min, max) = match direction {
                        North => (
                            [child.min[0], child.min[1], b.min[2]],
                            [child.max[0], child.max[1], b.min[2] + 1],
                        ),
                        South => (
                            [child.min[0], child.min[1], b.max[2] - 1],
                            [child.max[0], child.max[1], b.max[2]],
                        ),
                        West => (
                            [b.min[0], child.min[1], child.min[2]],
                            [b.min[0] + 1, child.max[1], child.max[2]],
                        ),
                        East => (
                            [b.max[0] - 1, child.min[1], child.min[2]],
                            [b.max[0], child.max[1], child.max[2]],
                        ),
                    };
                    let entrance = BoundingBox::new(min, max).map_err(anyhow::Error::msg)?;
                    if let MineshaftGraphPieceKind::Room { entrances } = &mut self.pieces[0].kind {
                        entrances.push(entrance);
                    }
                }
                pos += 4;
            }
        }
        Ok(())
    }

    fn add_children(&mut self, index: usize) -> Result<()> {
        use Direction::*;
        let piece = &self.pieces[index];
        let b = piece.bounds;
        let depth = piece.depth;
        let direction = piece.direction.expect("child direction");
        match piece.kind {
            MineshaftGraphPieceKind::Stairs => {
                let origin = match direction {
                    North => [b.min[0], b.min[1], b.min[2] - 1],
                    South => [b.min[0], b.min[1], b.max[2] + 1],
                    West => [b.min[0] - 1, b.min[1], b.min[2]],
                    East => [b.max[0] + 1, b.min[1], b.min[2]],
                };
                self.add_piece(origin, direction, depth)?;
            }
            MineshaftGraphPieceKind::Crossing { two_floors } => {
                let exits = [
                    (North, [b.min[0] + 1, b.min[1], b.min[2] - 1]),
                    (South, [b.min[0] + 1, b.min[1], b.max[2] + 1]),
                    (West, [b.min[0] - 1, b.min[1], b.min[2] + 1]),
                    (East, [b.max[0] + 1, b.min[1], b.min[2] + 1]),
                ];
                let order = match direction {
                    North => [0, 2, 3],
                    South => [1, 2, 3],
                    West => [0, 1, 2],
                    East => [0, 1, 3],
                };
                for i in order {
                    self.add_piece(exits[i].1, exits[i].0, depth)?;
                }
                if two_floors {
                    for i in [0, 2, 3, 1] {
                        if self.random.next_bool() {
                            let mut p = exits[i].1;
                            p[1] += 4;
                            self.add_piece(p, exits[i].0, depth)?;
                        }
                    }
                }
            }
            MineshaftGraphPieceKind::Corridor { .. } => {
                let choice = self.draw(4);
                let y = b.min[1] - 1 + self.draw(3);
                let (p, d) = match (direction, choice) {
                    (North, 0..=1) => ([b.min[0], y, b.min[2] - 1], North),
                    (North, 2) => ([b.min[0] - 1, y, b.min[2]], West),
                    (North, _) => ([b.max[0] + 1, y, b.min[2]], East),
                    (South, 0..=1) => ([b.min[0], y, b.max[2] + 1], South),
                    (South, 2) => ([b.min[0] - 1, y, b.max[2] - 3], West),
                    (South, _) => ([b.max[0] + 1, y, b.max[2] - 3], East),
                    (West, 0..=1) => ([b.min[0] - 1, y, b.min[2]], West),
                    (West, 2) => ([b.min[0], y, b.min[2] - 1], North),
                    (West, _) => ([b.min[0], y, b.max[2] + 1], South),
                    (East, 0..=1) => ([b.max[0] + 1, y, b.min[2]], East),
                    (East, 2) => ([b.max[0] - 3, y, b.min[2] - 1], North),
                    (East, _) => ([b.max[0] - 3, y, b.max[2] + 1], South),
                };
                self.add_piece(p, d, depth)?;
                if depth < 8 {
                    let along_z = matches!(direction, North | South);
                    let axis = if along_z { 2 } else { 0 };
                    let mut pos = b.min[axis] + 3;
                    while pos + 3 <= b.max[axis] {
                        let choice = self.draw(5);
                        let exit = match (along_z, choice) {
                            (true, 0) => Some(([b.min[0] - 1, b.min[1], pos], West)),
                            (true, 1) => Some(([b.max[0] + 1, b.min[1], pos], East)),
                            (false, 0) => Some(([pos, b.min[1], b.min[2] - 1], North)),
                            (false, 1) => Some(([pos, b.min[1], b.max[2] + 1], South)),
                            _ => None,
                        };
                        if let Some((p, d)) = exit {
                            self.add_piece(p, d, depth + 1)?;
                        }
                        pos += 5;
                    }
                }
            }
            MineshaftGraphPieceKind::Room { .. } => unreachable!("root uses room builder"),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_matches_known_java_result() {
        let graph = MineshaftGraph::new(i64::MIN, [0, 0], MineshaftVariant::Normal).unwrap();
        let (_, pieces, mut random) = graph.into_parts();
        assert_eq!(pieces.len(), 150);
        let MineshaftGraphPieceKind::Room { entrances } = &pieces[0].kind else {
            panic!("missing room")
        };
        assert_eq!(entrances.len(), 4);
        assert_eq!(random.next_long(), 8_190_460_250_141_936_893);
    }

    #[test]
    fn graph_rejects_overflowing_coordinates() {
        assert!(MineshaftGraph::new(0, [i32::MAX, 0], MineshaftVariant::Normal).is_err());
        assert!(MineshaftGraph::new(0, [134_217_720, 0], MineshaftVariant::Normal).is_err());
    }
}
