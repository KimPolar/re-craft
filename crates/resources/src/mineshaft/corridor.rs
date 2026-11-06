//! Corridor beam and cobweb placement in an unpublished generation draft.
use anyhow::{Result, ensure};
use native_core::{beardifier::BoundingBox, random::DecorationRandom};

use super::{MineshaftPlacementWorld, MineshaftVariant};
use crate::{structure_piece::StructureFrame, structure_start::HorizontalDirection};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CorridorBlock {
    Planks,
    Fence { connected: HorizontalDirection },
    WallTorch { facing: HorizontalDirection },
    Cobweb,
}

/// Structure-piece writes include the rotated block state, fluid scheduling,
/// shape postprocessing and the normal world-write side effects.
pub trait MineshaftCorridorWorld: MineshaftPlacementWorld {
    fn is_air(&mut self, position: [i32; 3]) -> Result<bool>;
    fn ocean_floor_height(&mut self, x: i32, z: i32) -> Result<i32>;
    /// Read the block and test its given world-space face.
    fn face_sturdy(&mut self, position: [i32; 3], face: [i32; 3]) -> Result<bool>;
    fn set_corridor_block(
        &mut self,
        position: [i32; 3],
        block: CorridorBlock,
        variant: MineshaftVariant,
    ) -> Result<()>;
}

fn inside(b: BoundingBox, p: [i32; 3]) -> bool {
    (0..3).all(|i| (b.min[i]..=b.max[i]).contains(&p[i]))
}

fn put(
    frame: StructureFrame,
    clip: BoundingBox,
    local: [i32; 3],
    mut block: CorridorBlock,
    variant: MineshaftVariant,
    world: &mut impl MineshaftCorridorWorld,
) -> Result<()> {
    let p = frame.world_position(local);
    if !inside(clip, p) || world.protected_block(p, variant)? {
        return Ok(());
    }
    block = match block {
        CorridorBlock::Fence { connected } => CorridorBlock::Fence {
            connected: frame.horizontal_facing(connected),
        },
        CorridorBlock::WallTorch { facing } => CorridorBlock::WallTorch {
            facing: frame.horizontal_facing(facing),
        },
        other => other,
    };
    world.set_corridor_block(p, block, variant)
}

fn validate(frame: StructureFrame, clip: BoundingBox) -> Result<()> {
    ensure!(frame.orientation.is_some(), "corridor orientation missing");
    ensure!(
        (0..3).all(|i| clip.min[i] <= clip.max[i]),
        "invalid corridor clip"
    );
    Ok(())
}

/// Place the fence columns, crossbeam, and possible torches at one local Z.
/// On failure the caller must discard both world draft and random state.
pub fn place_corridor_beam(
    frame: StructureFrame,
    z: i32,
    variant: MineshaftVariant,
    world: &mut impl MineshaftCorridorWorld,
    clip: BoundingBox,
    random: &mut DecorationRandom,
) -> Result<bool> {
    use HorizontalDirection::{East, North, South, West};
    validate(frame, clip)?;
    for x in 0..=2 {
        let p = frame.world_position([x, 3, z]);
        if !inside(clip, p) || world.is_air(p)? {
            return Ok(false);
        }
    }
    for (x, connected) in [(0, West), (2, East)] {
        for y in 0..=1 {
            put(
                frame,
                clip,
                [x, y, z],
                CorridorBlock::Fence { connected },
                variant,
                world,
            )?;
        }
    }
    if random.next_int_bound(4).expect("positive bound") == 0 {
        for x in [0, 2] {
            put(
                frame,
                clip,
                [x, 2, z],
                CorridorBlock::Planks,
                variant,
                world,
            )?;
        }
    } else {
        for x in 0..=2 {
            put(
                frame,
                clip,
                [x, 2, z],
                CorridorBlock::Planks,
                variant,
                world,
            )?;
        }
        for (dz, facing) in [(-1, South), (1, North)] {
            if random.next_float() < 0.05 {
                put(
                    frame,
                    clip,
                    [1, 2, z.wrapping_add(dz)],
                    CorridorBlock::WallTorch { facing },
                    variant,
                    world,
                )?;
            }
        }
    }
    Ok(true)
}

/// Try a cobweb at a local point. Java tests interior before drawing a random
/// value, then visits neighbors in DOWN, UP, NORTH, SOUTH, WEST, EAST order.
/// `true` means the placement conditions passed; clipping or a protected
/// block can still prevent the write. Discard world and RNG on callback error.
pub fn place_corridor_cobweb(
    frame: StructureFrame,
    local: [i32; 3],
    probability: f32,
    variant: MineshaftVariant,
    world: &mut impl MineshaftCorridorWorld,
    clip: BoundingBox,
    random: &mut DecorationRandom,
) -> Result<bool> {
    validate(frame, clip)?;
    ensure!(
        probability.is_finite() && (0.0..=1.0).contains(&probability),
        "invalid cobweb probability"
    );
    let above = frame.world_position([local[0], local[1].wrapping_add(1), local[2]]);
    if !inside(clip, above)
        || above[1] >= world.ocean_floor_height(above[0], above[2])?
        || random.next_float() >= probability
    {
        return Ok(false);
    }
    let p = frame.world_position(local);
    let mut count = 0;
    for direction in [
        [0, -1, 0],
        [0, 1, 0],
        [0, 0, -1],
        [0, 0, 1],
        [-1, 0, 0],
        [1, 0, 0],
    ] {
        let neighbor = std::array::from_fn(|i| p[i].wrapping_add(direction[i]));
        let face = direction.map(|v| -v);
        if inside(clip, neighbor) && world.face_sturdy(neighbor, face)? {
            count += 1;
            if count >= 2 {
                put(frame, clip, local, CorridorBlock::Cobweb, variant, world)?;
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct World {
        writes: Vec<([i32; 3], CorridorBlock)>,
    }
    impl MineshaftPlacementWorld for World {
        fn blocking_biome(&mut self, _: [i32; 3]) -> Result<bool> {
            unreachable!()
        }
        fn liquid_block(&mut self, _: [i32; 3]) -> Result<bool> {
            unreachable!()
        }
        fn protected_block(&mut self, _: [i32; 3], _: MineshaftVariant) -> Result<bool> {
            Ok(false)
        }
        fn set_cave_air(&mut self, _: [i32; 3]) -> Result<()> {
            unreachable!()
        }
    }
    impl MineshaftCorridorWorld for World {
        fn is_air(&mut self, _: [i32; 3]) -> Result<bool> {
            Ok(false)
        }
        fn ocean_floor_height(&mut self, _: i32, _: i32) -> Result<i32> {
            Ok(40)
        }
        fn face_sturdy(&mut self, _: [i32; 3], _: [i32; 3]) -> Result<bool> {
            Ok(true)
        }
        fn set_corridor_block(
            &mut self,
            p: [i32; 3],
            b: CorridorBlock,
            _: MineshaftVariant,
        ) -> Result<()> {
            self.writes.push((p, b));
            Ok(())
        }
    }

    #[test]
    fn outside_ceiling_skips_beam_without_consuming_randomness() {
        let frame = StructureFrame {
            bounds: BoundingBox::new([0, 30, 0], [2, 32, 9]).unwrap(),
            orientation: Some(HorizontalDirection::South),
        };
        let clip = BoundingBox::new([0, 30, 0], [2, 32, 9]).unwrap();
        let mut world = World::default();
        let mut random = DecorationRandom::new(42);
        let mut untouched = random.clone();
        assert!(
            !place_corridor_beam(
                frame,
                2,
                MineshaftVariant::Normal,
                &mut world,
                clip,
                &mut random
            )
            .unwrap()
        );
        assert!(world.writes.is_empty());
        assert_eq!(random.next_long(), untouched.next_long());
    }

    #[test]
    fn supported_interior_places_web_at_rotated_position() {
        let frame = StructureFrame {
            bounds: BoundingBox::new([0, 30, 0], [9, 32, 2]).unwrap(),
            orientation: Some(HorizontalDirection::West),
        };
        let clip = BoundingBox::new([-2, 29, -2], [12, 35, 4]).unwrap();
        let mut world = World::default();
        let mut random = DecorationRandom::new(0);
        assert!(
            place_corridor_cobweb(
                frame,
                [1, 1, 3],
                1.0,
                MineshaftVariant::Normal,
                &mut world,
                clip,
                &mut random
            )
            .unwrap()
        );
        assert_eq!(world.writes, vec![([6, 31, 1], CorridorBlock::Cobweb)]);
    }
}
