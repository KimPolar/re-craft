//! Placement of individual structure pieces into a caller-provided world.
//!
//! The world must implement Java block updates and block-entity creation; this
//! module does not persist chunks or evaluate the generated chest's loot.
use crate::{
    packs::ResourceId,
    structure_start::{HorizontalDirection, SimpleStructureKind, SimpleStructureStart},
};
use anyhow::{Context, Result, ensure};
use native_core::{
    beardifier::BoundingBox,
    random::{DecorationRandom, LegacyRandom, XoroshiroRandom},
};
use serde_json::Value;

/// Decoration passes use `DecorationRandom` seeded for the current feature;
/// direct random sources are useful for callers that already have one.
pub trait TreasureRandom {
    fn next_long(&mut self) -> i64;
}
impl TreasureRandom for LegacyRandom {
    fn next_long(&mut self) -> i64 {
        LegacyRandom::next_long(self)
    }
}
impl TreasureRandom for XoroshiroRandom {
    fn next_long(&mut self) -> i64 {
        XoroshiroRandom::next_long(self)
    }
}
impl TreasureRandom for DecorationRandom {
    fn next_long(&mut self) -> i64 {
        DecorationRandom::next_long(self)
    }
}

/// Block access and updates use world coordinates, including neighboring blocks.
/// Implementations must expose the current, already-decorated world state.
pub trait TreasureWorld {
    fn vertical_bounds(&self) -> (i32, i32);
    /// First available Y above the OCEAN_FLOOR_WG heightmap, not occupied Y.
    fn ocean_floor_height(&mut self, x: i32, z: i32) -> Result<i32>;
    fn block(&mut self, position: [i32; 3]) -> Result<Value>;
    fn is_air(&mut self, state: &Value) -> Result<bool>;
    fn is_solid_render(&mut self, state: &Value) -> Result<bool>;
    /// Equivalent to Java's setBlockAndUpdate, including the caller's update semantics.
    fn set_block_and_update(&mut self, position: [i32; 3], state: &Value) -> Result<()>;
    /// Place the default chest with this facing. Return whether a chest block entity exists afterward.
    fn set_chest(&mut self, position: [i32; 3], facing: HorizontalDirection) -> Result<bool>;
    fn set_chest_loot(&mut self, position: [i32; 3], table: &str, seed: i64) -> Result<()>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BuriedTreasureOutcome {
    NoFoundation,
    Found {
        position: [i32; 3],
        chest_placed: bool,
        loot_seed: Option<i64>,
    },
}

fn block_id(state: &Value) -> Result<ResourceId> {
    ResourceId::parse(
        state
            .as_str()
            .or_else(|| state.get("id").and_then(Value::as_str))
            .context("block state has no id")?,
    )
}
fn is_one_of(state: &Value, names: &[&str]) -> Result<bool> {
    let id = block_id(state)?;
    Ok(id.namespace == "minecraft" && names.contains(&id.path.as_str()))
}
fn is_air_or_liquid(world: &mut impl TreasureWorld, state: &Value) -> Result<bool> {
    Ok(world.is_air(state)? || is_one_of(state, &["water", "lava"])?)
}
fn relative(p: [i32; 3], offset: [i32; 3]) -> Result<[i32; 3]> {
    Ok([
        p[0].checked_add(offset[0]).context("block X overflow")?,
        p[1].checked_add(offset[1]).context("block Y overflow")?,
        p[2].checked_add(offset[2]).context("block Z overflow")?,
    ])
}
fn offset(dir: HorizontalDirection) -> [i32; 3] {
    match dir {
        HorizontalDirection::North => [0, 0, -1],
        HorizontalDirection::East => [1, 0, 0],
        HorizontalDirection::South => [0, 0, 1],
        HorizontalDirection::West => [-1, 0, 0],
    }
}
fn opposite(dir: HorizontalDirection) -> HorizontalDirection {
    match dir {
        HorizontalDirection::North => HorizontalDirection::South,
        HorizontalDirection::South => HorizontalDirection::North,
        HorizontalDirection::East => HorizontalDirection::West,
        HorizontalDirection::West => HorizontalDirection::East,
    }
}
fn clockwise(dir: HorizontalDirection) -> HorizontalDirection {
    match dir {
        HorizontalDirection::North => HorizontalDirection::East,
        HorizontalDirection::East => HorizontalDirection::South,
        HorizontalDirection::South => HorizontalDirection::West,
        HorizontalDirection::West => HorizontalDirection::North,
    }
}
fn chest_facing(world: &mut impl TreasureWorld, p: [i32; 3]) -> Result<HorizontalDirection> {
    let directions = [
        HorizontalDirection::North,
        HorizontalDirection::East,
        HorizontalDirection::South,
        HorizontalDirection::West,
    ];
    let mut neighbor = None;
    for direction in directions {
        let state = world.block(relative(p, offset(direction))?)?;
        if is_one_of(&state, &["chest"])? {
            return Ok(HorizontalDirection::North);
        }
        if world.is_solid_render(&state)? {
            if neighbor.is_some() {
                neighbor = None;
                break;
            }
            neighbor = Some(direction);
        }
    }
    if let Some(direction) = neighbor {
        return Ok(opposite(direction));
    }
    let mut facing = HorizontalDirection::North;
    let state = world.block(relative(p, offset(facing))?)?;
    if world.is_solid_render(&state)? {
        facing = opposite(facing);
    }
    let state = world.block(relative(p, offset(facing))?)?;
    if world.is_solid_render(&state)? {
        facing = clockwise(facing);
    }
    let state = world.block(relative(p, offset(facing))?)?;
    if world.is_solid_render(&state)? {
        facing = opposite(facing);
    }
    Ok(facing)
}

/// Apply one buried-treasure piece. On error discard the partially modified
/// world draft and random state. `writable` is Java's chunk decoration box.
pub fn place_buried_treasure(
    piece: &mut SimpleStructureStart,
    world: &mut impl TreasureWorld,
    writable: BoundingBox,
    random: &mut impl TreasureRandom,
) -> Result<BuriedTreasureOutcome> {
    ensure!(
        piece.kind == SimpleStructureKind::BuriedTreasure,
        "piece is not buried treasure"
    );
    let (min_y, max_y) = world.vertical_bounds();
    ensure!(
        min_y <= max_y && i64::from(max_y) - i64::from(min_y) <= 4096,
        "invalid world height"
    );
    let [x, _, z] = piece.bounds.min;
    let height = world.ocean_floor_height(x, z)?;
    ensure!(
        i64::from(height) >= i64::from(min_y) && i64::from(height) <= i64::from(max_y) + 1,
        "ocean floor height outside world"
    );
    if height <= min_y {
        return Ok(BuriedTreasureOutcome::NoFoundation);
    }
    let sand = Value::String("minecraft:sand".to_owned());
    for y in (min_y + 1..=height).rev() {
        let p = [x, y, z];
        let current = world.block(p)?;
        let below = world.block(relative(p, [0, -1, 0])?)?;
        if !is_one_of(
            &below,
            &["sandstone", "stone", "andesite", "granite", "diorite"],
        )? {
            continue;
        }
        let fill = if is_air_or_liquid(world, &current)? {
            sand.clone()
        } else {
            current
        };
        // Direction.values(): down, up, north, south, west, east.
        for direction in [
            [0, -1, 0],
            [0, 1, 0],
            [0, 0, -1],
            [0, 0, 1],
            [-1, 0, 0],
            [1, 0, 0],
        ] {
            let neighbor = relative(p, direction)?;
            let state = world.block(neighbor)?;
            if !is_air_or_liquid(world, &state)? {
                continue;
            }
            let neighbor_below = world.block(relative(neighbor, [0, -1, 0])?)?;
            let place = if direction != [0, 1, 0] && is_air_or_liquid(world, &neighbor_below)? {
                &below
            } else {
                &fill
            };
            world.set_block_and_update(neighbor, place)?;
        }
        piece.bounds = BoundingBox::new(p, p).map_err(anyhow::Error::msg)?;
        let inside = (0..3).all(|i| p[i] >= writable.min[i] && p[i] <= writable.max[i]);
        if !inside || is_one_of(&world.block(p)?, &["chest"])? {
            return Ok(BuriedTreasureOutcome::Found {
                position: p,
                chest_placed: false,
                loot_seed: None,
            });
        }
        let facing = chest_facing(world, p)?;
        let has_entity = world.set_chest(p, facing)?;
        let seed = if has_entity {
            let seed = random.next_long();
            world.set_chest_loot(p, "minecraft:chests/buried_treasure", seed)?;
            Some(seed)
        } else {
            None
        };
        return Ok(BuriedTreasureOutcome::Found {
            position: p,
            chest_placed: true,
            loot_seed: seed,
        });
    }
    Ok(BuriedTreasureOutcome::NoFoundation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;
    struct World {
        blocks: BTreeMap<[i32; 3], Value>,
        loot: Option<i64>,
        facing: Option<HorizontalDirection>,
    }
    impl TreasureWorld for World {
        fn vertical_bounds(&self) -> (i32, i32) {
            (0, 20)
        }
        fn ocean_floor_height(&mut self, _: i32, _: i32) -> Result<i32> {
            Ok(6)
        }
        fn block(&mut self, p: [i32; 3]) -> Result<Value> {
            Ok(self
                .blocks
                .get(&p)
                .cloned()
                .unwrap_or_else(|| json!("minecraft:air")))
        }
        fn is_air(&mut self, s: &Value) -> Result<bool> {
            Ok(s.as_str() == Some("minecraft:air"))
        }
        fn is_solid_render(&mut self, s: &Value) -> Result<bool> {
            Ok(matches!(
                s.as_str(),
                Some("minecraft:stone" | "minecraft:sand")
            ))
        }
        fn set_block_and_update(&mut self, p: [i32; 3], s: &Value) -> Result<()> {
            self.blocks.insert(p, s.clone());
            Ok(())
        }
        fn set_chest(&mut self, p: [i32; 3], f: HorizontalDirection) -> Result<bool> {
            self.blocks.insert(p, json!("minecraft:chest"));
            self.facing = Some(f);
            Ok(true)
        }
        fn set_chest_loot(&mut self, _: [i32; 3], table: &str, seed: i64) -> Result<()> {
            assert_eq!(table, "minecraft:chests/buried_treasure");
            self.loot = Some(seed);
            Ok(())
        }
    }
    fn piece() -> SimpleStructureStart {
        let p = [9, 90, 9];
        SimpleStructureStart {
            kind: SimpleStructureKind::BuriedTreasure,
            chunk: [0, 0],
            generation_point: [8, 10, 8],
            orientation: None,
            bounds: BoundingBox::new(p, p).unwrap(),
            terrain_adjustment: native_core::beardifier::TerrainAdjustment::None,
        }
    }
    #[test]
    fn places_chest_loot_and_honors_writable_box() {
        let make_world = || World {
            blocks: BTreeMap::from([([9, 5, 9], json!("minecraft:stone"))]),
            loot: None,
            facing: None,
        };
        let mut world = make_world();
        let mut placed = piece();
        let mut random = LegacyRandom::new(42);
        let expected = LegacyRandom::new(42).next_long();
        let result = place_buried_treasure(
            &mut placed,
            &mut world,
            BoundingBox::new([0, 0, 0], [15, 20, 15]).unwrap(),
            &mut random,
        )
        .unwrap();
        assert_eq!(
            result,
            BuriedTreasureOutcome::Found {
                position: [9, 6, 9],
                chest_placed: true,
                loot_seed: Some(expected)
            }
        );
        assert_eq!(world.loot, Some(expected));
        assert_eq!(placed.bounds.min, [9, 6, 9]);
        assert!(world.facing.is_some());
        let mut clipped = make_world();
        let mut p = piece();
        let mut random = LegacyRandom::new(42);
        let result = place_buried_treasure(
            &mut p,
            &mut clipped,
            BoundingBox::new([0, 0, 0], [8, 20, 15]).unwrap(),
            &mut random,
        )
        .unwrap();
        assert!(matches!(
            result,
            BuriedTreasureOutcome::Found {
                chest_placed: false,
                ..
            }
        ));
        assert_eq!(clipped.loot, None);
        assert_eq!(random.next_long(), expected);
    }
}
