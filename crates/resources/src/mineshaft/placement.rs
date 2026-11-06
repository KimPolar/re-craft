//! Mineshaft piece placement in a caller-owned, unpublished generation draft.
use anyhow::{Result, ensure};
use native_core::beardifier::BoundingBox;

use super::{MineshaftGraphPiece, MineshaftGraphPieceKind, MineshaftVariant};
use crate::{structure_piece::StructureFrame, structure_start::HorizontalDirection};

/// Callbacks preserve world read and write ordering; implementations must
/// retain the side effects of setting cave air in their generation draft.
pub trait MineshaftPlacementWorld {
    fn blocking_biome(&mut self, position: [i32; 3]) -> Result<bool>;
    /// Match BlockState.liquid(), not merely a nonempty waterlogged fluid.
    fn liquid_block(&mut self, position: [i32; 3]) -> Result<bool>;
    /// Match the variant's planks, log, fence and iron chain blocks.
    fn protected_block(&mut self, position: [i32; 3], variant: MineshaftVariant) -> Result<bool>;
    fn set_cave_air(&mut self, position: [i32; 3]) -> Result<()>;
}

/// Additional world operations used by a crossing's supports and floor.
pub trait MineshaftCrossingWorld: MineshaftPlacementWorld {
    fn is_air(&mut self, position: [i32; 3]) -> Result<bool>;
    /// Java's OCEAN_FLOOR_WG height, the first position above the floor.
    fn ocean_floor_height(&mut self, x: i32, z: i32) -> Result<i32>;
    /// Read the existing block and test whether its upper face is sturdy.
    fn upper_face_sturdy(&mut self, position: [i32; 3]) -> Result<bool>;
    /// StructurePiece.placeBlock, including fluid and shape side effects.
    fn set_piece_planks(&mut self, position: [i32; 3], variant: MineshaftVariant) -> Result<()>;
    /// Direct WorldGenLevel.setBlock(..., 2), used for floor support.
    fn set_floor_planks(&mut self, position: [i32; 3], variant: MineshaftVariant) -> Result<()>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MineshaftPlacementOutcome {
    Blocked,
    Carved,
}

fn inside(bounds: BoundingBox, position: [i32; 3]) -> bool {
    (0..3).all(|axis| (bounds.min[axis]..=bounds.max[axis]).contains(&position[axis]))
}

fn blocked(
    world: &mut impl MineshaftPlacementWorld,
    piece: BoundingBox,
    clip: BoundingBox,
) -> Result<bool> {
    let lo = std::array::from_fn::<_, 3, _>(|i| piece.min[i].wrapping_sub(1).max(clip.min[i]));
    let hi = std::array::from_fn::<_, 3, _>(|i| piece.max[i].wrapping_add(1).min(clip.max[i]));
    let center = std::array::from_fn(|i| lo[i].wrapping_add(hi[i]) / 2);
    if world.blocking_biome(center)? {
        return Ok(true);
    }
    for x in lo[0]..=hi[0] {
        for z in lo[2]..=hi[2] {
            if world.liquid_block([x, lo[1], z])? || world.liquid_block([x, hi[1], z])? {
                return Ok(true);
            }
        }
    }
    for x in lo[0]..=hi[0] {
        for y in lo[1]..=hi[1] {
            if world.liquid_block([x, y, lo[2]])? || world.liquid_block([x, y, hi[2]])? {
                return Ok(true);
            }
        }
    }
    for z in lo[2]..=hi[2] {
        for y in lo[1]..=hi[1] {
            if world.liquid_block([lo[0], y, z])? || world.liquid_block([hi[0], y, z])? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn carve(
    world: &mut impl MineshaftPlacementWorld,
    frame: StructureFrame,
    clip: BoundingBox,
    variant: MineshaftVariant,
    local: [i32; 3],
) -> Result<()> {
    let position = frame.world_position(local);
    if inside(clip, position) && !world.protected_block(position, variant)? {
        world.set_cave_air(position)?;
    }
    Ok(())
}

fn fill(
    world: &mut impl MineshaftPlacementWorld,
    frame: StructureFrame,
    clip: BoundingBox,
    variant: MineshaftVariant,
    lo: [i32; 3],
    hi: [i32; 3],
) -> Result<()> {
    for y in lo[1]..=hi[1] {
        for x in lo[0]..=hi[0] {
            for z in lo[2]..=hi[2] {
                carve(world, frame, clip, variant, [x, y, z])?;
            }
        }
    }
    Ok(())
}

/// Place a room or stairs pass with Java's clipping and read/write order.
/// On callback failure the caller must discard the entire world draft.
pub fn place_room_or_stairs(
    piece: &MineshaftGraphPiece,
    variant: MineshaftVariant,
    world: &mut impl MineshaftPlacementWorld,
    clip: BoundingBox,
) -> Result<MineshaftPlacementOutcome> {
    let b = piece.bounds;
    let span = |axis: usize| i64::from(b.max[axis]) - i64::from(b.min[axis]) + 1;
    ensure!(
        (0..3).all(|axis| clip.min[axis] <= clip.max[axis]),
        "invalid mineshaft clip"
    );
    match &piece.kind {
        MineshaftGraphPieceKind::Room { entrances } => {
            ensure!(
                piece.direction.is_none()
                    && (8..=13).contains(&span(0))
                    && (5..=10).contains(&span(1))
                    && (8..=13).contains(&span(2)),
                "invalid mineshaft room"
            );
            for entrance in entrances {
                ensure!(
                    (0..3).all(|axis| {
                        let n = i64::from(entrance.max[axis]) - i64::from(entrance.min[axis]) + 1;
                        n > 0 && n <= 8
                    }),
                    "invalid mineshaft entrance"
                );
                ensure!(
                    entrance.max[1] >= i32::MIN + 2,
                    "mineshaft entrance Y out of range"
                );
            }
        }
        MineshaftGraphPieceKind::Stairs => {
            let direction = piece
                .direction
                .ok_or_else(|| anyhow::anyhow!("stairs orientation is missing"))?;
            let axis = if matches!(
                direction,
                HorizontalDirection::North | HorizontalDirection::South
            ) {
                2
            } else {
                0
            };
            ensure!(
                span(axis) == 9 && span(2 - axis) == 3 && span(1) == 8,
                "invalid mineshaft stairs"
            );
        }
        _ => anyhow::bail!("mineshaft piece carving is not implemented"),
    }
    if blocked(world, b, clip)? {
        return Ok(MineshaftPlacementOutcome::Blocked);
    }
    let frame = StructureFrame {
        bounds: b,
        orientation: piece.direction,
    };
    match &piece.kind {
        MineshaftGraphPieceKind::Room { entrances } => {
            fill(
                world,
                frame,
                clip,
                variant,
                [b.min[0], b.min[1] + 1, b.min[2]],
                [b.max[0], (b.min[1] + 3).min(b.max[1]), b.max[2]],
            )?;
            for entrance in entrances {
                fill(
                    world,
                    frame,
                    clip,
                    variant,
                    [entrance.min[0], entrance.max[1] - 2, entrance.min[2]],
                    entrance.max,
                )?;
            }
            let lo = [b.min[0], b.min[1] + 4, b.min[2]];
            let hi = b.max;
            let dx = (hi[0] - lo[0] + 1) as f32;
            let dy = (hi[1] - lo[1] + 1) as f32;
            let dz = (hi[2] - lo[2] + 1) as f32;
            let cx = lo[0] as f32 + dx / 2.0;
            let cz = lo[2] as f32 + dz / 2.0;
            for y in lo[1]..=hi[1] {
                let fy = (y - lo[1]) as f32 / dy;
                for x in lo[0]..=hi[0] {
                    let fx = (x as f32 - cx) / (dx * 0.5);
                    for z in lo[2]..=hi[2] {
                        let fz = (z as f32 - cz) / (dz * 0.5);
                        if fx * fx + fy * fy + fz * fz <= 1.05_f32 {
                            carve(world, frame, clip, variant, [x, y, z])?;
                        }
                    }
                }
            }
        }
        MineshaftGraphPieceKind::Stairs => {
            fill(world, frame, clip, variant, [0, 5, 0], [2, 7, 1])?;
            fill(world, frame, clip, variant, [0, 0, 7], [2, 2, 8])?;
            for i in 0..5 {
                fill(
                    world,
                    frame,
                    clip,
                    variant,
                    [0, 5 - i - if i < 4 { 1 } else { 0 }, 2 + i],
                    [2, 7 - i, 2 + i],
                )?;
            }
        }
        _ => unreachable!("piece validated before placement"),
    }
    Ok(MineshaftPlacementOutcome::Carved)
}

/// Place one single- or double-floor crossing. Its direction is saved
/// separately, so its carving coordinates are already world coordinates.
/// Discard the world draft if any callback fails.
pub fn place_crossing(
    piece: &MineshaftGraphPiece,
    variant: MineshaftVariant,
    world: &mut impl MineshaftCrossingWorld,
    clip: BoundingBox,
) -> Result<MineshaftPlacementOutcome> {
    let b = piece.bounds;
    let MineshaftGraphPieceKind::Crossing { two_floors } = piece.kind else {
        anyhow::bail!("piece is not a mineshaft crossing");
    };
    ensure!(piece.direction.is_some(), "crossing direction is missing");
    ensure!(
        (0..3).all(|i| clip.min[i] <= clip.max[i])
            && i64::from(b.max[0]) - i64::from(b.min[0]) == 4
            && i64::from(b.max[2]) - i64::from(b.min[2]) == 4
            && i64::from(b.max[1]) - i64::from(b.min[1]) == if two_floors { 6 } else { 2 }
            && b.min[1] > i32::MIN
            && b.max[1] < i32::MAX,
        "invalid crossing bounds or clip"
    );
    if blocked(world, b, clip)? {
        return Ok(MineshaftPlacementOutcome::Blocked);
    }
    let frame = StructureFrame {
        bounds: b,
        orientation: None,
    };
    let [x0, y0, z0] = b.min;
    let [x1, y1, z1] = b.max;
    let lower_top = if two_floors { y0 + 2 } else { y1 };
    fill(
        world,
        frame,
        clip,
        variant,
        [x0 + 1, y0, z0],
        [x1 - 1, lower_top, z1],
    )?;
    fill(
        world,
        frame,
        clip,
        variant,
        [x0, y0, z0 + 1],
        [x1, lower_top, z1 - 1],
    )?;
    if two_floors {
        fill(
            world,
            frame,
            clip,
            variant,
            [x0 + 1, y1 - 2, z0],
            [x1 - 1, y1, z1],
        )?;
        fill(
            world,
            frame,
            clip,
            variant,
            [x0, y1 - 2, z0 + 1],
            [x1, y1, z1 - 1],
        )?;
        fill(
            world,
            frame,
            clip,
            variant,
            [x0 + 1, y0 + 3, z0 + 1],
            [x1 - 1, y0 + 3, z1 - 1],
        )?;
    }
    for (x, z) in [
        (x0 + 1, z0 + 1),
        (x0 + 1, z1 - 1),
        (x1 - 1, z0 + 1),
        (x1 - 1, z1 - 1),
    ] {
        let above = [x, y1 + 1, z];
        if inside(clip, above) && !world.is_air(above)? {
            for y in y0..=y1 {
                let p = [x, y, z];
                if inside(clip, p) && !world.protected_block(p, variant)? {
                    world.set_piece_planks(p, variant)?;
                }
            }
        }
    }
    for x in x0..=x1 {
        for z in z0..=z1 {
            let above = [x, y0, z];
            if inside(clip, above) && y0 < world.ocean_floor_height(x, z)? {
                let floor = [x, y0 - 1, z];
                if !world.upper_face_sturdy(floor)? {
                    world.set_floor_planks(floor, variant)?;
                }
            }
        }
    }
    Ok(MineshaftPlacementOutcome::Carved)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct World {
        biome_blocked: bool,
        liquids: usize,
        protected: usize,
        writes: usize,
    }
    impl MineshaftPlacementWorld for World {
        fn blocking_biome(&mut self, _: [i32; 3]) -> Result<bool> {
            Ok(self.biome_blocked)
        }
        fn liquid_block(&mut self, _: [i32; 3]) -> Result<bool> {
            self.liquids += 1;
            Ok(false)
        }
        fn protected_block(&mut self, _: [i32; 3], _: MineshaftVariant) -> Result<bool> {
            self.protected += 1;
            Ok(true)
        }
        fn set_cave_air(&mut self, _: [i32; 3]) -> Result<()> {
            self.writes += 1;
            Ok(())
        }
    }
    impl MineshaftCrossingWorld for World {
        fn is_air(&mut self, _: [i32; 3]) -> Result<bool> {
            Ok(false)
        }
        fn ocean_floor_height(&mut self, _: i32, _: i32) -> Result<i32> {
            Ok(100)
        }
        fn upper_face_sturdy(&mut self, _: [i32; 3]) -> Result<bool> {
            Ok(false)
        }
        fn set_piece_planks(&mut self, _: [i32; 3], _: MineshaftVariant) -> Result<()> {
            self.writes += 1;
            Ok(())
        }
        fn set_floor_planks(&mut self, p: [i32; 3], _: MineshaftVariant) -> Result<()> {
            assert_eq!(p[1], 49);
            self.writes += 1;
            Ok(())
        }
    }

    #[test]
    fn crossing_floor_can_write_below_the_clip() {
        let bounds = BoundingBox::new([0, 50, 0], [4, 52, 4]).unwrap();
        let piece = MineshaftGraphPiece {
            bounds,
            direction: Some(HorizontalDirection::West),
            depth: 1,
            kind: MineshaftGraphPieceKind::Crossing { two_floors: false },
        };
        let mut world = World {
            biome_blocked: false,
            liquids: 0,
            protected: 0,
            writes: 0,
        };
        assert_eq!(
            place_crossing(&piece, MineshaftVariant::Normal, &mut world, bounds).unwrap(),
            MineshaftPlacementOutcome::Carved
        );
        assert_eq!(world.writes, 25);
    }

    #[test]
    fn biome_block_and_protected_blocks_do_not_write() {
        let bounds = BoundingBox::new([0, 50, 0], [8, 54, 8]).unwrap();
        let room = MineshaftGraphPiece {
            bounds,
            direction: None,
            depth: 0,
            kind: MineshaftGraphPieceKind::Room { entrances: vec![] },
        };
        let clip = BoundingBox::new([-2, 40, -2], [15, 70, 15]).unwrap();
        let mut world = World {
            biome_blocked: true,
            liquids: 0,
            protected: 0,
            writes: 0,
        };
        assert_eq!(
            place_room_or_stairs(&room, MineshaftVariant::Normal, &mut world, clip).unwrap(),
            MineshaftPlacementOutcome::Blocked
        );
        assert_eq!((world.liquids, world.protected, world.writes), (0, 0, 0));
        world.biome_blocked = false;
        assert_eq!(
            place_room_or_stairs(&room, MineshaftVariant::Normal, &mut world, clip).unwrap(),
            MineshaftPlacementOutcome::Carved
        );
        assert!(world.liquids > 0 && world.protected > 0);
        assert_eq!(world.writes, 0);
    }

    #[test]
    fn rejects_unsupported_piece_before_world_access() {
        let bounds = BoundingBox::new([0, 50, 0], [2, 52, 9]).unwrap();
        let corridor = MineshaftGraphPiece {
            bounds,
            direction: Some(HorizontalDirection::South),
            depth: 1,
            kind: MineshaftGraphPieceKind::Corridor {
                rails: true,
                spiders: false,
                sections: 2,
            },
        };
        let mut world = World {
            biome_blocked: false,
            liquids: 0,
            protected: 0,
            writes: 0,
        };
        assert!(
            place_room_or_stairs(&corridor, MineshaftVariant::Normal, &mut world, bounds).is_err()
        );
        assert_eq!((world.liquids, world.protected, world.writes), (0, 0, 0));
    }
}
