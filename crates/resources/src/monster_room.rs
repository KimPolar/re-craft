//! Java 26.3 monster-room feature and its block-entity creation boundary.
use crate::structure_start::HorizontalDirection;
use anyhow::Result;
use native_core::random::DecorationRandom;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoomBlock {
    CaveAir,
    Cobblestone,
    MossyCobblestone,
    Chest(HorizontalDirection),
    Spawner,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoomMob {
    Skeleton,
    Zombie,
    Spider,
}
pub trait MonsterRoomWorld {
    type State;
    fn min_y(&self) -> i32;
    fn block(&mut self, pos: [i32; 3]) -> Result<Self::State>;
    fn is_solid(&mut self, state: &Self::State) -> Result<bool>;
    fn is_solid_render(&mut self, state: &Self::State) -> Result<bool>;
    fn is_air(&mut self, state: &Self::State) -> Result<bool>;
    fn is_chest(&mut self, state: &Self::State) -> Result<bool>;
    fn is_spawner(&mut self, state: &Self::State) -> Result<bool>;
    fn may_replace(&mut self, state: &Self::State) -> Result<bool>;
    /// Apply a flags=2 WorldGenLevel write, including block-entity effects.
    fn set_block(&mut self, pos: [i32; 3], block: RoomBlock) -> Result<()>;
    fn chest_entity_exists(&mut self, pos: [i32; 3]) -> Result<bool>;
    fn set_chest_loot(&mut self, pos: [i32; 3], seed: i64) -> Result<()>;
    fn spawner_entity_exists(&mut self, pos: [i32; 3]) -> Result<bool>;
    /// Setting an existing spawner can consume RNG while resolving spawn data.
    fn set_spawner_mob(
        &mut self,
        pos: [i32; 3],
        mob: RoomMob,
        random: &mut DecorationRandom,
    ) -> Result<()>;
}
fn offset(p: [i32; 3], x: i32, y: i32, z: i32) -> [i32; 3] {
    [
        p[0].wrapping_add(x),
        p[1].wrapping_add(y),
        p[2].wrapping_add(z),
    ]
}
fn direction(d: HorizontalDirection) -> [i32; 3] {
    match d {
        HorizontalDirection::North => [0, 0, -1],
        HorizontalDirection::East => [1, 0, 0],
        HorizontalDirection::South => [0, 0, 1],
        HorizontalDirection::West => [-1, 0, 0],
    }
}
fn relative(p: [i32; 3], d: HorizontalDirection) -> [i32; 3] {
    let v = direction(d);
    offset(p, v[0], v[1], v[2])
}
fn opposite(d: HorizontalDirection) -> HorizontalDirection {
    match d {
        HorizontalDirection::North => HorizontalDirection::South,
        HorizontalDirection::East => HorizontalDirection::West,
        HorizontalDirection::South => HorizontalDirection::North,
        HorizontalDirection::West => HorizontalDirection::East,
    }
}
fn clockwise(d: HorizontalDirection) -> HorizontalDirection {
    match d {
        HorizontalDirection::North => HorizontalDirection::East,
        HorizontalDirection::East => HorizontalDirection::South,
        HorizontalDirection::South => HorizontalDirection::West,
        HorizontalDirection::West => HorizontalDirection::North,
    }
}
const HORIZONTAL: [HorizontalDirection; 4] = [
    HorizontalDirection::North,
    HorizontalDirection::East,
    HorizontalDirection::South,
    HorizontalDirection::West,
];
fn empty(w: &mut impl MonsterRoomWorld, p: [i32; 3]) -> Result<bool> {
    let b = w.block(p)?;
    w.is_air(&b)
}
fn safe_set(w: &mut impl MonsterRoomWorld, p: [i32; 3], replacement: RoomBlock) -> Result<()> {
    let old = w.block(p)?;
    if w.may_replace(&old)? {
        w.set_block(p, replacement)?;
    }
    Ok(())
}
fn chest_facing(w: &mut impl MonsterRoomWorld, p: [i32; 3]) -> Result<HorizontalDirection> {
    let mut neighbor = None;
    for d in HORIZONTAL {
        let b = w.block(relative(p, d))?;
        if w.is_chest(&b)? {
            return Ok(HorizontalDirection::North);
        }
        if w.is_solid_render(&b)? {
            if neighbor.is_some() {
                neighbor = None;
                break;
            }
            neighbor = Some(d);
        }
    }
    if let Some(d) = neighbor {
        return Ok(opposite(d));
    }
    let mut facing = HorizontalDirection::North;
    let b = w.block(relative(p, facing))?;
    if w.is_solid_render(&b)? {
        facing = opposite(facing);
    }
    let b = w.block(relative(p, facing))?;
    if w.is_solid_render(&b)? {
        facing = clockwise(facing);
    }
    let b = w.block(relative(p, facing))?;
    if w.is_solid_render(&b)? {
        facing = opposite(facing);
    }
    Ok(facing)
}

/// Attempt one monster room; the caller must discard the entire draft on error.
pub fn place_monster_room(
    w: &mut impl MonsterRoomWorld,
    random: &mut DecorationRandom,
    origin: [i32; 3],
) -> Result<bool> {
    let rx = random.next_int_bound(2).unwrap() + 2;
    let rz = random.next_int_bound(2).unwrap() + 2;
    let (min_x, max_x, min_z, max_z) = (-rx - 1, rx + 1, -rz - 1, rz + 1);
    let mut openings = 0;
    for x in min_x..=max_x {
        for y in -1..=4 {
            for z in min_z..=max_z {
                let p = offset(origin, x, y, z);
                let b = w.block(p)?;
                let solid = w.is_solid(&b)?;
                if (y == -1 || y == 4) && !solid {
                    return Ok(false);
                }
                if (x == min_x || x == max_x || z == min_z || z == max_z)
                    && y == 0
                    && empty(w, p)?
                    && empty(w, offset(p, 0, 1, 0))?
                {
                    openings += 1;
                }
            }
        }
    }
    if !(1..=5).contains(&openings) {
        return Ok(false);
    }
    for x in min_x..=max_x {
        for y in (-1..=3).rev() {
            for z in min_z..=max_z {
                let p = offset(origin, x, y, z);
                let b = w.block(p)?;
                if x == min_x || x == max_x || y == -1 || z == min_z || z == max_z {
                    if p[1] >= w.min_y() {
                        let below = w.block(offset(p, 0, -1, 0))?;
                        if !w.is_solid(&below)? {
                            w.set_block(p, RoomBlock::CaveAir)?;
                            continue;
                        }
                    }
                    if w.is_solid(&b)? && !w.is_chest(&b)? {
                        let mossy = y == -1 && random.next_int_bound(4).unwrap() != 0;
                        safe_set(
                            w,
                            p,
                            if mossy {
                                RoomBlock::MossyCobblestone
                            } else {
                                RoomBlock::Cobblestone
                            },
                        )?;
                    }
                } else if !w.is_chest(&b)? && !w.is_spawner(&b)? {
                    safe_set(w, p, RoomBlock::CaveAir)?;
                }
            }
        }
    }
    for _ in 0..2 {
        for _ in 0..3 {
            let p = offset(
                origin,
                random.next_int_bound(rx * 2 + 1).unwrap() - rx,
                0,
                random.next_int_bound(rz * 2 + 1).unwrap() - rz,
            );
            if !empty(w, p)? {
                continue;
            }
            let mut solid = 0;
            for d in HORIZONTAL {
                let b = w.block(relative(p, d))?;
                if w.is_solid(&b)? {
                    solid += 1;
                }
            }
            if solid != 1 {
                continue;
            }
            let facing = chest_facing(w, p)?;
            safe_set(w, p, RoomBlock::Chest(facing))?;
            if w.chest_entity_exists(p)? {
                let seed = random.next_long();
                w.set_chest_loot(p, seed)?;
            }
            break;
        }
    }
    safe_set(w, origin, RoomBlock::Spawner)?;
    if w.spawner_entity_exists(origin)? {
        let mob = match random.next_int_bound(4).unwrap() {
            0 => RoomMob::Skeleton,
            1 | 2 => RoomMob::Zombie,
            _ => RoomMob::Spider,
        };
        w.set_spawner_mob(origin, mob, random)?;
    }
    Ok(true)
}
