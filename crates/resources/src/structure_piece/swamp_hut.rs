//! Java 26.3 swamp-hut piece geometry on a caller-provided generation world.
//! The caller implements block side effects and creates the structure mobs.
use anyhow::{Result, ensure};
use native_core::beardifier::BoundingBox;

use super::{ScatteredGroundHeight, StructureFrame};
use crate::structure_start::{HorizontalDirection, SimpleStructureKind, SimpleStructureStart};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HutBlock {
    SprucePlanks,
    OakLog,
    OakFence,
    Air,
    PottedRedMushroom,
    CraftingTable,
    Cauldron,
    SpruceStairs {
        facing: HorizontalDirection,
        shape: &'static str,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HutMob {
    Witch,
    Cat,
}

/// Callbacks operate on the live generation draft in Java call order.
/// `place_piece_block` includes the structure-piece fluid and shape checks;
/// support writes use Java's direct setBlock path instead.
pub trait SwampHutWorld {
    fn motion_blocking_no_leaves(&mut self, x: i32, z: i32) -> Result<i32>;
    fn place_piece_block(&mut self, position: [i32; 3], block: HutBlock) -> Result<()>;
    fn min_y(&self) -> i32;
    fn support_replaceable(&mut self, position: [i32; 3]) -> Result<bool>;
    fn place_support_log(&mut self, position: [i32; 3]) -> Result<()>;
    /// Attempt Java's persistent STRUCTURE spawn, including initialization.
    /// An unavailable entity type can return success without adding an entity.
    fn spawn_structure_mob(&mut self, mob: HutMob, position: [i32; 3]) -> Result<()>;
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SwampHutState {
    pub ground: ScatteredGroundHeight,
    pub spawned_witch: bool,
    pub spawned_cat: bool,
}

fn inside(bounds: BoundingBox, p: [i32; 3]) -> bool {
    (0..3).all(|i| bounds.min[i] <= p[i] && p[i] <= bounds.max[i])
}

fn put(
    world: &mut impl SwampHutWorld,
    frame: StructureFrame,
    clip: BoundingBox,
    local: [i32; 3],
    block: HutBlock,
) -> Result<()> {
    let position = frame.world_position(local);
    if !inside(clip, position) {
        return Ok(());
    }
    let block = match block {
        HutBlock::SpruceStairs { facing, shape } => HutBlock::SpruceStairs {
            facing: frame.horizontal_facing(facing),
            shape: frame
                .stair_shape(facing, shape)
                .expect("static stair shape"),
        },
        other => other,
    };
    world.place_piece_block(position, block)
}

fn fill(
    world: &mut impl SwampHutWorld,
    frame: StructureFrame,
    clip: BoundingBox,
    min: [i32; 3],
    max: [i32; 3],
    block: HutBlock,
) -> Result<()> {
    for y in min[1]..=max[1] {
        for x in min[0]..=max[0] {
            for z in min[2]..=max[2] {
                put(world, frame, clip, [x, y, z], block)?;
            }
        }
    }
    Ok(())
}

/// Run one synchronous piece pass within Java's supplied decoration box.
/// On callback failure the caller must discard the world draft, piece and state.
pub fn place_swamp_hut(
    piece: &mut SimpleStructureStart,
    state: &mut SwampHutState,
    world: &mut impl SwampHutWorld,
    clip: BoundingBox,
) -> Result<bool> {
    ensure!(
        piece.kind == SimpleStructureKind::SwampHut && piece.orientation.is_some(),
        "invalid swamp hut piece"
    );
    if !state
        .ground
        .update_average(piece, clip, 0, |x, z| world.motion_blocking_no_leaves(x, z))?
    {
        return Ok(false);
    }
    let frame = StructureFrame {
        bounds: piece.bounds,
        orientation: piece.orientation,
    };
    use HorizontalDirection::{East, North, South, West};
    use HutBlock::*;
    for (min, max, block) in [
        ([1, 1, 1], [5, 1, 7], SprucePlanks),
        ([1, 4, 2], [5, 4, 7], SprucePlanks),
        ([2, 1, 0], [4, 1, 0], SprucePlanks),
        ([2, 2, 2], [3, 3, 2], SprucePlanks),
        ([1, 2, 3], [1, 3, 6], SprucePlanks),
        ([5, 2, 3], [5, 3, 6], SprucePlanks),
        ([2, 2, 7], [4, 3, 7], SprucePlanks),
        ([1, 0, 2], [1, 3, 2], OakLog),
        ([5, 0, 2], [5, 3, 2], OakLog),
        ([1, 0, 7], [1, 3, 7], OakLog),
        ([5, 0, 7], [5, 3, 7], OakLog),
    ] {
        fill(world, frame, clip, min, max, block)?;
    }
    for (pos, block) in [
        ([2, 3, 2], OakFence),
        ([3, 3, 7], OakFence),
        ([1, 3, 4], Air),
        ([5, 3, 4], Air),
        ([5, 3, 5], Air),
        ([1, 3, 5], PottedRedMushroom),
        ([3, 2, 6], CraftingTable),
        ([4, 2, 6], Cauldron),
        ([1, 2, 1], OakFence),
        ([5, 2, 1], OakFence),
    ] {
        put(world, frame, clip, pos, block)?;
    }
    for (min, max, facing) in [
        ([0, 4, 1], [6, 4, 1], North),
        ([0, 4, 2], [0, 4, 7], East),
        ([6, 4, 2], [6, 4, 7], West),
        ([0, 4, 8], [6, 4, 8], South),
    ] {
        fill(
            world,
            frame,
            clip,
            min,
            max,
            SpruceStairs {
                facing,
                shape: "straight",
            },
        )?;
    }
    for (pos, facing, shape) in [
        ([0, 4, 1], North, "outer_right"),
        ([6, 4, 1], North, "outer_left"),
        ([0, 4, 8], South, "outer_left"),
        ([6, 4, 8], South, "outer_right"),
    ] {
        put(world, frame, clip, pos, SpruceStairs { facing, shape })?;
    }
    for z in [2, 7] {
        for x in [1, 5] {
            let mut pos = frame.world_position([x, -1, z]);
            if !inside(clip, pos) {
                continue;
            }
            while world.support_replaceable(pos)? && pos[1] > world.min_y().wrapping_add(1) {
                world.place_support_log(pos)?;
                pos[1] = pos[1].wrapping_sub(1);
            }
        }
    }
    let mob_pos = frame.world_position([2, 2, 5]);
    if inside(clip, mob_pos) {
        if !state.spawned_witch {
            state.spawned_witch = true;
            world.spawn_structure_mob(HutMob::Witch, mob_pos)?;
        }
        if !state.spawned_cat {
            state.spawned_cat = true;
            world.spawn_structure_mob(HutMob::Cat, mob_pos)?;
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_core::beardifier::TerrainAdjustment;

    #[derive(Default)]
    struct World {
        writes: Vec<([i32; 3], HutBlock)>,
        mobs: Vec<(HutMob, [i32; 3])>,
    }
    impl SwampHutWorld for World {
        fn motion_blocking_no_leaves(&mut self, _: i32, _: i32) -> Result<i32> {
            Ok(64)
        }
        fn place_piece_block(&mut self, p: [i32; 3], b: HutBlock) -> Result<()> {
            self.writes.push((p, b));
            Ok(())
        }
        fn min_y(&self) -> i32 {
            -64
        }
        fn support_replaceable(&mut self, p: [i32; 3]) -> Result<bool> {
            Ok(p[1] > 59)
        }
        fn place_support_log(&mut self, p: [i32; 3]) -> Result<()> {
            self.writes.push((p, HutBlock::OakLog));
            Ok(())
        }
        fn spawn_structure_mob(&mut self, mob: HutMob, p: [i32; 3]) -> Result<()> {
            self.mobs.push((mob, p));
            Ok(())
        }
    }

    #[test]
    fn hut_places_mobs_once_and_retries_when_clipped() {
        let mut piece = SimpleStructureStart {
            kind: SimpleStructureKind::SwampHut,
            chunk: [0, 0],
            generation_point: [0, 0, 0],
            orientation: Some(HorizontalDirection::South),
            bounds: BoundingBox::new([3, 64, 3], [9, 70, 11]).unwrap(),
            terrain_adjustment: TerrainAdjustment::None,
        };
        let mut state = SwampHutState::default();
        let mut world = World::default();
        let no_spawn = BoundingBox::new([0, -64, 0], [4, 319, 15]).unwrap();
        assert!(place_swamp_hut(&mut piece, &mut state, &mut world, no_spawn).unwrap());
        assert!(world.mobs.is_empty() && !state.spawned_witch && !state.spawned_cat);
        let all = BoundingBox::new([0, -64, 0], [15, 319, 15]).unwrap();
        assert!(place_swamp_hut(&mut piece, &mut state, &mut world, all).unwrap());
        assert_eq!(
            world.mobs,
            [(HutMob::Witch, [5, 66, 8]), (HutMob::Cat, [5, 66, 8])]
        );
        assert!(place_swamp_hut(&mut piece, &mut state, &mut world, all).unwrap());
        assert_eq!(world.mobs.len(), 2);
    }
}
