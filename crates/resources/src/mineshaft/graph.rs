//! Recursive piece layout before vertical placement and block generation.

use super::{
    InitialMineshaftRoom, MineshaftPieceKind, MineshaftVariant, choose_piece_kind, find_corridor,
    find_crossing, find_stairs,
};
use crate::structure_start::HorizontalDirection as Direction;
use anyhow::{Context, Result, ensure};
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
    variant: MineshaftVariant,
    pieces: Vec<MineshaftGraphPiece>,
    occupied: Vec<BoundingBox>,
    random: LegacyRandom,
}

/// The surface height is sampled at [`MineshaftGraph::center`] before placement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MineshaftElevation {
    Normal { sea_level: i32, min_y: i32 },
    Mesa { sea_level: i32, surface_height: i32 },
}

/// A vertically positioned layout, still awaiting decoration and persistence.
pub struct PlacedMineshaft {
    pub variant: MineshaftVariant,
    pub pieces: Vec<MineshaftGraphPiece>,
    pub offset_y: i32,
    pub generation_point: [i32; 3],
    random: LegacyRandom,
}

impl PlacedMineshaft {
    pub fn into_parts(self) -> (MineshaftVariant, Vec<MineshaftGraphPiece>, LegacyRandom) {
        (self.variant, self.pieces, self.random)
    }
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

    pub fn pieces(&self) -> &[MineshaftGraphPiece] {
        &self.pieces
    }

    /// The inclusive bounding-box center used for mesa surface sampling.
    pub fn center(&self) -> [i32; 3] {
        Self::center_of(self.bounds())
    }

    fn center_of(bounds: BoundingBox) -> [i32; 3] {
        std::array::from_fn(|axis| {
            (i64::from(bounds.min[axis])
                + (i64::from(bounds.max[axis]) - i64::from(bounds.min[axis]) + 1) / 2)
                as i32
        })
    }

    fn bounds(&self) -> BoundingBox {
        let mut bounds = self.pieces[0].bounds;
        for piece in &self.pieces[1..] {
            for axis in 0..3 {
                bounds.min[axis] = bounds.min[axis].min(piece.bounds.min[axis]);
                bounds.max[axis] = bounds.max[axis].max(piece.bounds.max[axis]);
            }
        }
        bounds
    }

    /// Apply the 26.3 vertical placement; `surface_height` must come from the
    /// unadjusted WORLD_SURFACE_WG heightmap at the graph center.
    pub fn place(mut self, elevation: MineshaftElevation) -> Result<PlacedMineshaft> {
        let bounds = self.bounds();
        let center = Self::center_of(bounds);
        let target = match (self.variant, elevation) {
            (MineshaftVariant::Normal, MineshaftElevation::Normal { sea_level, min_y }) => {
                let limit = i64::from(sea_level) - 10;
                let mut top =
                    i64::from(bounds.max[1]) - i64::from(bounds.min[1]) + 1 + i64::from(min_y) + 1;
                if top < limit {
                    let range = i32::try_from(limit - top)
                        .context("mineshaft elevation range too large")?;
                    top += i64::from(self.draw(range));
                }
                top - i64::from(bounds.max[1])
            }
            (
                MineshaftVariant::Mesa,
                MineshaftElevation::Mesa {
                    sea_level,
                    surface_height,
                },
            ) => {
                let mut target = i64::from(sea_level);
                if surface_height > sea_level {
                    let range = i32::try_from(i64::from(surface_height) - target + 1)
                        .context("mineshaft elevation range too large")?;
                    target += i64::from(self.draw(range));
                }
                target - i64::from(center[1])
            }
            _ => anyhow::bail!("mineshaft elevation does not match its variant"),
        };
        let offset_y = i32::try_from(target).context("mineshaft elevation offset overflow")?;
        let shifted_y = |y: i32| {
            y.checked_add(offset_y)
                .context("mineshaft piece Y overflow")
        };
        let root = self.pieces[0].bounds;
        let generation_point = [root.min[0] + 6, shifted_y(50)?, root.min[2] - 2];
        for piece in &mut self.pieces {
            piece.bounds.min[1] = shifted_y(piece.bounds.min[1])?;
            piece.bounds.max[1] = shifted_y(piece.bounds.max[1])?;
            if let MineshaftGraphPieceKind::Room { entrances } = &mut piece.kind {
                for entrance in entrances {
                    entrance.min[1] = shifted_y(entrance.min[1])?;
                    entrance.max[1] = shifted_y(entrance.max[1])?;
                }
            }
        }
        Ok(PlacedMineshaft {
            variant: self.variant,
            pieces: self.pieces,
            offset_y,
            generation_point,
            random: self.random,
        })
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

    #[test]
    fn mesa_at_or_below_sea_level_moves_every_piece_without_a_random_draw() {
        let graph = MineshaftGraph::new(42, [4, -1], MineshaftVariant::Mesa).unwrap();
        let center = graph.center();
        let original = graph.pieces().to_vec();
        let (_, _, mut unshifted_random) = MineshaftGraph::new(42, [4, -1], MineshaftVariant::Mesa)
            .unwrap()
            .into_parts();
        let placed = graph
            .place(MineshaftElevation::Mesa {
                sea_level: 63,
                surface_height: 50,
            })
            .unwrap();
        assert_eq!(placed.offset_y, 63 - center[1]);
        assert_eq!(placed.generation_point, [72, 50 + placed.offset_y, -16]);
        for (before, after) in original.iter().zip(&placed.pieces) {
            assert_eq!(after.bounds.min[1], before.bounds.min[1] + placed.offset_y);
            assert_eq!(after.bounds.max[1], before.bounds.max[1] + placed.offset_y);
            if let (
                MineshaftGraphPieceKind::Room { entrances: old },
                MineshaftGraphPieceKind::Room { entrances: new },
            ) = (&before.kind, &after.kind)
            {
                for (old, new) in old.iter().zip(new) {
                    assert_eq!(new.min[1], old.min[1] + placed.offset_y);
                    assert_eq!(new.max[1], old.max[1] + placed.offset_y);
                }
            }
        }
        let (_, _, mut placed_random) = placed.into_parts();
        assert_eq!(placed_random.next_long(), unshifted_random.next_long());
    }

    #[test]
    fn elevation_rejects_mismatched_variant_and_extreme_ranges() {
        let graph = MineshaftGraph::new(0, [0, 0], MineshaftVariant::Normal).unwrap();
        assert!(
            graph
                .place(MineshaftElevation::Mesa {
                    sea_level: 63,
                    surface_height: 70
                })
                .is_err()
        );
        let graph = MineshaftGraph::new(0, [0, 0], MineshaftVariant::Mesa).unwrap();
        assert!(
            graph
                .place(MineshaftElevation::Mesa {
                    sea_level: i32::MIN,
                    surface_height: i32::MAX
                })
                .is_err()
        );
    }
}
