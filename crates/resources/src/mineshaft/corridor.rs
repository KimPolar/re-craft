//! Corridor placement in an unpublished generation draft.
use anyhow::{Result, ensure};
use native_core::{beardifier::BoundingBox, random::DecorationRandom};

use super::{
    MineshaftGraphPiece, MineshaftGraphPieceKind, MineshaftPlacementOutcome,
    MineshaftPlacementWorld, MineshaftSupportWorld, MineshaftVariant, place_vertical_support,
    placement::blocked,
};
use crate::{structure_piece::StructureFrame, structure_start::HorizontalDirection};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CorridorBlock {
    Planks,
    Fence { connected: HorizontalDirection },
    WallTorch { facing: HorizontalDirection },
    Cobweb,
    Rail { north_south: bool },
}

/// Additional operations needed by the whole corridor pass. Entity callbacks
/// must reproduce the Java creation path and its use of the passed RNG.
pub trait MineshaftCorridorGenerationWorld: MineshaftMinecartWorld + MineshaftSupportWorld {
    fn upper_face_sturdy(&mut self, position: [i32; 3]) -> Result<bool>;
    fn set_floor_planks(&mut self, position: [i32; 3], variant: MineshaftVariant) -> Result<()>;
    fn is_variant_planks(&mut self, position: [i32; 3], variant: MineshaftVariant) -> Result<bool>;
    /// Read one floor block; check both non-air and solid-render on that state.
    fn solid_render_floor(&mut self, position: [i32; 3]) -> Result<bool>;
    /// Can be overridden for instrumented worlds; normal implementations use
    /// the shared Java-compatible eligibility and minecart creation path.
    fn create_chest_minecart(
        &mut self,
        frame: StructureFrame,
        local: [i32; 3],
        clip: BoundingBox,
        variant: MineshaftVariant,
        random: &mut DecorationRandom,
    ) -> Result<bool> {
        try_mineshaft_chest_minecart(frame, local, clip, variant, self, random)
    }
    /// Set the spawner and configure a cave spider if the block entity exists.
    fn set_spider_spawner(
        &mut self,
        position: [i32; 3],
        random: &mut DecorationRandom,
    ) -> Result<()>;
}

/// Construct an entity independently of the decoration RNG. Returning None
/// models an unavailable or disabled entity type, which consumes no loot seed.
pub trait MineshaftMinecartWorld: MineshaftCorridorWorld {
    type Minecart;
    fn new_chest_minecart(&mut self, position: [f64; 3]) -> Result<Option<Self::Minecart>>;
    /// Set the abandoned-mineshaft loot table and seed, then add the entity.
    fn add_chest_minecart(&mut self, entity: Self::Minecart, loot_seed: i64) -> Result<()>;
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

fn put<W: MineshaftCorridorWorld + ?Sized>(
    frame: StructureFrame,
    clip: BoundingBox,
    local: [i32; 3],
    mut block: CorridorBlock,
    variant: MineshaftVariant,
    world: &mut W,
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
        CorridorBlock::Rail { north_south } => {
            let local = if north_south {
                HorizontalDirection::North
            } else {
                HorizontalDirection::East
            };
            let facing = frame.horizontal_facing(local);
            CorridorBlock::Rail {
                north_south: matches!(
                    facing,
                    HorizontalDirection::North | HorizontalDirection::South
                ),
            }
        }
        other => other,
    };
    world.set_corridor_block(p, block, variant)
}

/// Create a chest minecart after checking its local block and floor. A true
/// result means the location qualified, even if the entity type was disabled.
/// On error, discard the world draft and random stream.
pub fn try_mineshaft_chest_minecart<W: MineshaftMinecartWorld + ?Sized>(
    frame: StructureFrame,
    local: [i32; 3],
    clip: BoundingBox,
    variant: MineshaftVariant,
    world: &mut W,
    random: &mut DecorationRandom,
) -> Result<bool> {
    validate(frame, clip)?;
    let pos = frame.world_position(local);
    if !inside(clip, pos)
        || !world.is_air(pos)?
        || world.is_air([pos[0], pos[1].wrapping_sub(1), pos[2]])?
    {
        return Ok(false);
    }
    let north_south = random.next_bool();
    put(
        frame,
        clip,
        local,
        CorridorBlock::Rail { north_south },
        variant,
        world,
    )?;
    let center = [
        f64::from(pos[0]) + 0.5,
        f64::from(pos[1]) + 0.5,
        f64::from(pos[2]) + 0.5,
    ];
    if let Some(entity) = world.new_chest_minecart(center)? {
        let loot_seed = random.next_long();
        world.add_chest_minecart(entity, loot_seed)?;
    }
    Ok(true)
}

fn validate(frame: StructureFrame, clip: BoundingBox) -> Result<()> {
    ensure!(frame.orientation.is_some(), "corridor orientation missing");
    ensure!(
        (0..3).all(|i| clip.min[i] <= clip.max[i]),
        "invalid corridor clip"
    );
    Ok(())
}

fn interior(
    frame: StructureFrame,
    clip: BoundingBox,
    local: [i32; 3],
    world: &mut impl MineshaftCorridorWorld,
) -> Result<bool> {
    let p = frame.world_position([local[0], local[1].wrapping_add(1), local[2]]);
    Ok(inside(clip, p) && p[1] < world.ocean_floor_height(p[0], p[2])?)
}

fn cave_air(
    frame: StructureFrame,
    clip: BoundingBox,
    local: [i32; 3],
    variant: MineshaftVariant,
    world: &mut impl MineshaftCorridorWorld,
) -> Result<()> {
    let p = frame.world_position(local);
    if inside(clip, p) && !world.protected_block(p, variant)? {
        world.set_cave_air(p)?;
    }
    Ok(())
}

/// Run one complete corridor piece pass. Entity operations are delegated to
/// the world implementation; both draft and state must be discarded on error.
pub fn place_corridor(
    piece: &MineshaftGraphPiece,
    placed_spider: &mut bool,
    variant: MineshaftVariant,
    world: &mut impl MineshaftCorridorGenerationWorld,
    clip: BoundingBox,
    random: &mut DecorationRandom,
) -> Result<MineshaftPlacementOutcome> {
    let MineshaftGraphPieceKind::Corridor {
        rails,
        spiders,
        sections,
    } = piece.kind
    else {
        anyhow::bail!("piece is not a mineshaft corridor");
    };
    let direction = piece
        .direction
        .ok_or_else(|| anyhow::anyhow!("corridor orientation missing"))?;
    let axis = if matches!(
        direction,
        HorizontalDirection::North | HorizontalDirection::South
    ) {
        2
    } else {
        0
    };
    let span = |i: usize| i64::from(piece.bounds.max[i]) - i64::from(piece.bounds.min[i]) + 1;
    ensure!(
        (1..=4).contains(&sections)
            && span(axis) == i64::from(sections) * 5
            && span(2 - axis) == 3
            && span(1) == 3
            && !(rails && spiders)
            && (!*placed_spider || spiders),
        "invalid corridor state or bounds"
    );
    let frame = StructureFrame {
        bounds: piece.bounds,
        orientation: Some(direction),
    };
    validate(frame, clip)?;
    if blocked(world, piece.bounds, clip)? {
        return Ok(MineshaftPlacementOutcome::Blocked);
    }
    let length = sections * 5 - 1;
    for y in 0..=1 {
        for x in 0..=2 {
            for z in 0..=length {
                cave_air(frame, clip, [x, y, z], variant, world)?;
            }
        }
    }
    for x in 0..=2 {
        for z in 0..=length {
            if random.next_float() <= 0.8 {
                cave_air(frame, clip, [x, 2, z], variant, world)?;
            }
        }
    }
    if spiders {
        for y in 0..=1 {
            for x in 0..=2 {
                for z in 0..=length {
                    let local = [x, y, z];
                    if random.next_float() <= 0.6 && interior(frame, clip, local, world)? {
                        put(frame, clip, local, CorridorBlock::Cobweb, variant, world)?;
                    }
                }
            }
        }
    }
    for section in 0..sections {
        let z = 2 + section * 5;
        place_corridor_beam(frame, z, variant, world, clip, random)?;
        for (x, dz, chance) in [
            (0, -1, 0.1),
            (2, -1, 0.1),
            (0, 1, 0.1),
            (2, 1, 0.1),
            (0, -2, 0.05),
            (2, -2, 0.05),
            (0, 2, 0.05),
            (2, 2, 0.05),
        ] {
            place_corridor_cobweb(frame, [x, 2, z + dz], chance, variant, world, clip, random)?;
        }
        for local in [[2, 0, z - 1], [0, 0, z + 1]] {
            if random.next_int_bound(100).expect("positive bound") == 0 {
                world.create_chest_minecart(frame, local, clip, variant, random)?;
            }
        }
        if spiders && !*placed_spider {
            let local = [
                1,
                0,
                z - 1 + random.next_int_bound(3).expect("positive bound"),
            ];
            let p = frame.world_position(local);
            if inside(clip, p) && interior(frame, clip, local, world)? {
                *placed_spider = true;
                world.set_spider_spawner(p, random)?;
            }
        }
    }
    for x in 0..=2 {
        for z in 0..=length {
            let local = [x, -1, z];
            if interior(frame, clip, local, world)? {
                let p = frame.world_position(local);
                if !world.upper_face_sturdy(p)? {
                    world.set_floor_planks(p, variant)?;
                }
            }
        }
    }
    for index in 0..if sections > 1 { 2 } else { 1 } {
        let z = if index == 0 { 2 } else { length - 2 };
        for x in [0, 2] {
            let local = [x, -1, z];
            let p = frame.world_position(local);
            if inside(clip, p) && world.is_variant_planks(p, variant)? {
                place_vertical_support(frame, local, variant, clip, world)?;
            }
        }
    }
    if rails {
        for z in 0..=length {
            let floor = frame.world_position([1, -1, z]);
            if inside(clip, floor) && world.solid_render_floor(floor)? {
                let chance = if interior(frame, clip, [1, 0, z], world)? {
                    0.7
                } else {
                    0.9
                };
                if random.next_float() < chance {
                    put(
                        frame,
                        clip,
                        [1, 0, z],
                        CorridorBlock::Rail { north_south: true },
                        variant,
                        world,
                    )?;
                }
            }
        }
    }
    Ok(MineshaftPlacementOutcome::Carved)
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
