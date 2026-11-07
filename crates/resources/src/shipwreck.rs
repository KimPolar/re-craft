//! Java 26.3 shipwreck piece selection and initial template bounds.
//!
//! Selection alone does not place the template, process its markers or make a
//! complete structure start. Callers must not treat it as generated terrain.
use anyhow::{Context, Result, ensure};
use native_core::{beardifier::BoundingBox, random::LegacyRandom};
use serde_json::Value;

use crate::{
    packs::ResourceId,
    structure_placement::large_feature,
    structure_start::{Heightmap, StartEnvironment},
    structure_transform::{self, Mirror, Rotation},
};

pub const PIVOT: [i32; 3] = [4, 0, 15];

const BEACHED: [&str; 11] = [
    "with_mast",
    "sideways_full",
    "sideways_fronthalf",
    "sideways_backhalf",
    "rightsideup_full",
    "rightsideup_fronthalf",
    "rightsideup_backhalf",
    "with_mast_degraded",
    "rightsideup_full_degraded",
    "rightsideup_fronthalf_degraded",
    "rightsideup_backhalf_degraded",
];
const OCEAN: [&str; 20] = [
    "with_mast",
    "upsidedown_full",
    "upsidedown_fronthalf",
    "upsidedown_backhalf",
    "sideways_full",
    "sideways_fronthalf",
    "sideways_backhalf",
    "rightsideup_full",
    "rightsideup_fronthalf",
    "rightsideup_backhalf",
    "with_mast_degraded",
    "upsidedown_full_degraded",
    "upsidedown_fronthalf_degraded",
    "upsidedown_backhalf_degraded",
    "sideways_full_degraded",
    "sideways_fronthalf_degraded",
    "sideways_backhalf_degraded",
    "rightsideup_full_degraded",
    "rightsideup_fronthalf_degraded",
    "rightsideup_backhalf_degraded",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Piece {
    pub template: String,
    pub position: [i32; 3],
    pub rotation: Rotation,
    pub beached: bool,
}

/// Decode the required structure-specific setting, without resolving biomes.
pub fn decode_beached(definition: &Value) -> Result<bool> {
    let id = ResourceId::parse(
        definition
            .get("type")
            .and_then(Value::as_str)
            .context("missing structure type")?,
    )?;
    ensure!(
        id.namespace == "minecraft" && id.path == "shipwreck",
        "not a shipwreck structure"
    );
    definition
        .get("is_beached")
        .and_then(Value::as_bool)
        .context("invalid shipwreck is_beached")
}

/// Consume rotation first and then the template index, as generatePieces does.
pub fn select_piece(random: &mut LegacyRandom, position: [i32; 3], beached: bool) -> Piece {
    let rotation = Rotation::random(random);
    let templates: &[&str] = if beached { &BEACHED } else { &OCEAN };
    let index = random
        .next_int_bound(templates.len() as i32)
        .expect("nonempty template list") as usize;
    Piece {
        template: format!("minecraft:shipwreck/{}", templates[index]),
        position,
        rotation,
        beached,
    }
}

pub fn select_for_chunk(seed: i64, chunk: [i32; 2], beached: bool) -> Result<Piece> {
    Ok(select_with_chunk_random(seed, chunk, beached)?.0)
}

/// Return the stream after selection so oversized pieces can consume it next.
pub fn select_with_chunk_random(
    seed: i64,
    chunk: [i32; 2],
    beached: bool,
) -> Result<(Piece, LegacyRandom)> {
    let x = chunk[0].checked_mul(16).context("chunk X overflow")?;
    let z = chunk[1].checked_mul(16).context("chunk Z overflow")?;
    let mut random = large_feature(seed, chunk[0], chunk[1]);
    let piece = select_piece(&mut random, [x, 90, z], beached);
    Ok((piece, random))
}

impl Piece {
    pub fn bounds(&self, size: [i32; 3]) -> Result<BoundingBox> {
        structure_transform::bounding_box(size, self.position, Mirror::None, self.rotation, PIVOT)
    }
}

/// Java 26.3 tests X and Y, including the seemingly unusual Y test.
pub fn too_big_for_region(size: [i32; 3]) -> bool {
    size[0] > 32 || size[1] > 32
}

/// The caller supplies the appropriate generation or decoration random source.
pub fn beached_height(lowest: i32, template_height: i32, random_offset: i32) -> Result<i32> {
    ensure!(
        (0..3).contains(&random_offset),
        "invalid shipwreck random offset"
    );
    Ok(lowest
        .wrapping_sub(template_height / 2)
        .wrapping_sub(random_offset))
}

/// Align oversized pieces during start generation, before region decoration.
/// The returned bounding box remains the constructor's initial box; Java
/// refreshes it only when the template is subsequently postprocessed.
pub fn align_during_start(
    piece: &mut Piece,
    size: [i32; 3],
    environment: &mut impl StartEnvironment,
    random: &mut LegacyRandom,
) -> Result<(BoundingBox, bool)> {
    ensure!(
        size.iter().all(|v| *v >= 0),
        "negative shipwreck template size"
    );
    let bounds = piece.bounds(size)?;
    if !too_big_for_region(size) {
        return Ok((bounds, false));
    }
    let span_x = bounds.max[0].wrapping_sub(bounds.min[0]).wrapping_add(1);
    let span_z = bounds.max[2].wrapping_sub(bounds.min[2]).wrapping_add(1);
    // getLowestY's parameters differ from getMeanFirstOccupiedHeight's.
    // ShipwreckStructure passes the same arguments to both methods.
    let (x, z, width, depth) = if piece.beached {
        (bounds.min[0], span_x, bounds.min[2], span_z)
    } else {
        (bounds.min[0], bounds.min[2], span_x, span_z)
    };
    let mut values = [0; 4];
    for (value, [px, pz]) in values.iter_mut().zip([
        [x, z],
        [x, z.wrapping_add(depth)],
        [x.wrapping_add(width), z],
        [x.wrapping_add(width), z.wrapping_add(depth)],
    ]) {
        *value = environment.first_occupied_height(px, pz, Heightmap::WorldSurface)?;
    }
    let y = if piece.beached {
        beached_height(
            *values.iter().min().unwrap(),
            size[1],
            random.next_int_bound(3).unwrap(),
        )?
    } else {
        values.into_iter().fold(0_i32, i32::wrapping_add) / 4
    };
    piece.position[1] = y;
    Ok((bounds, true))
}

/// Current heightmaps used when the first chunk decorates a small shipwreck.
pub trait HeightWorld {
    fn max_y(&self) -> i32;
    /// Returns the first available height, matching WorldGenLevel.getHeight.
    fn height(&mut self, map: Heightmap, x: i32, z: i32) -> Result<i32>;
}

/// Align a small piece once, before placing its blocks. Large pieces are
/// adjusted earlier at structure-start time and are intentionally skipped.
/// The callback draws nextInt(3) from the current decoration random stream.
pub fn align_during_decoration(
    piece: &mut Piece,
    size: [i32; 3],
    height_adjusted: &mut bool,
    world: &mut impl HeightWorld,
    mut next_offset: impl FnMut() -> i32,
) -> Result<()> {
    ensure!(
        size.iter().all(|v| *v >= 0),
        "negative shipwreck template size"
    );
    if *height_adjusted || too_big_for_region(size) {
        return Ok(());
    }
    let area = size[0]
        .checked_mul(size[2])
        .context("shipwreck template area overflow")?;
    let map = if piece.beached {
        Heightmap::WorldSurface
    } else {
        Heightmap::OceanFloor
    };
    let mut lowest = world.max_y().wrapping_add(1);
    let average = if area == 0 {
        world.height(map, piece.position[0], piece.position[2])?
    } else {
        let mut sum = 0_i32;
        for z in 0..size[2] {
            for x in 0..size[0] {
                let y = world.height(
                    map,
                    piece.position[0].wrapping_add(x),
                    piece.position[2].wrapping_add(z),
                )?;
                sum = sum.wrapping_add(y);
                lowest = lowest.min(y);
            }
        }
        sum / area
    };
    let height = if piece.beached {
        beached_height(lowest, size[1], next_offset())?
    } else {
        average
    };
    piece.position[1] = height;
    *height_adjusted = true;
    Ok(())
}

/// Access the already placed block entity directly below a DATA marker.
pub trait MarkerWorld {
    fn is_randomizable_container(&mut self, position: [i32; 3]) -> Result<bool>;
    fn set_loot_table(&mut self, position: [i32; 3], table: &str, seed: i64) -> Result<()>;
}

/// Unknown markers leave both the world and random stream untouched.
pub fn handle_data_marker(
    world: &mut impl MarkerWorld,
    random: &mut impl crate::structure_piece::TreasureRandom,
    marker: &str,
    position: [i32; 3],
) -> Result<()> {
    let table = match marker {
        "map_chest" => "minecraft:chests/shipwreck_map",
        "treasure_chest" => "minecraft:chests/shipwreck_treasure",
        "supply_chest" => "minecraft:chests/shipwreck_supply",
        _ => return Ok(()),
    };
    let below = [position[0], position[1].wrapping_sub(1), position[2]];
    if world.is_randomizable_container(below)? {
        world.set_loot_table(below, table, random.next_long())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn settings_and_oversized_dimension_checks_are_strict() {
        assert!(decode_beached(&json!({"type":"shipwreck","is_beached":true})).unwrap());
        assert!(decode_beached(&json!({"type":"shipwreck"})).is_err());
        assert!(decode_beached(&json!({"type":"ocean_ruin","is_beached":false})).is_err());
        assert!(!too_big_for_region([32, 32, 100]));
        assert!(too_big_for_region([33, 1, 1]));
        assert!(too_big_for_region([1, 33, 1]));
        assert!(beached_height(60, 9, 3).is_err());
    }

    #[test]
    fn alignment_is_one_time_and_uses_unrotated_footprint() -> Result<()> {
        struct World(Vec<(Heightmap, i32, i32)>);
        impl HeightWorld for World {
            fn max_y(&self) -> i32 {
                319
            }
            fn height(&mut self, map: Heightmap, x: i32, z: i32) -> Result<i32> {
                self.0.push((map, x, z));
                Ok(x + z)
            }
        }
        let mut piece = Piece {
            template: "minecraft:shipwreck/with_mast".into(),
            position: [10, 90, 20],
            rotation: Rotation::Clockwise90,
            beached: false,
        };
        let mut world = World(vec![]);
        let mut adjusted = false;
        align_during_decoration(&mut piece, [2, 3, 3], &mut adjusted, &mut world, || {
            panic!("ocean piece must not draw")
        })?;
        assert_eq!(piece.position, [10, 31, 20]);
        assert_eq!(
            world.0,
            vec![
                (Heightmap::OceanFloor, 10, 20),
                (Heightmap::OceanFloor, 11, 20),
                (Heightmap::OceanFloor, 10, 21),
                (Heightmap::OceanFloor, 11, 21),
                (Heightmap::OceanFloor, 10, 22),
                (Heightmap::OceanFloor, 11, 22)
            ]
        );
        align_during_decoration(&mut piece, [2, 3, 3], &mut adjusted, &mut world, || {
            panic!("already adjusted")
        })?;
        assert_eq!(world.0.len(), 6);
        Ok(())
    }

    #[test]
    fn marker_requires_an_existing_randomizable_container() -> Result<()> {
        struct World {
            exists: bool,
            visits: usize,
            loot: Option<(String, i64)>,
        }
        impl MarkerWorld for World {
            fn is_randomizable_container(&mut self, pos: [i32; 3]) -> Result<bool> {
                assert_eq!(pos, [3, 9, 4]);
                self.visits += 1;
                Ok(self.exists)
            }
            fn set_loot_table(&mut self, pos: [i32; 3], table: &str, seed: i64) -> Result<()> {
                assert_eq!(pos, [3, 9, 4]);
                self.loot = Some((table.into(), seed));
                Ok(())
            }
        }
        let mut world = World {
            exists: false,
            visits: 0,
            loot: None,
        };
        let mut random = LegacyRandom::new(4);
        let mut expected = random.clone();
        handle_data_marker(&mut world, &mut random, "unknown", [3, 10, 4])?;
        handle_data_marker(&mut world, &mut random, "map_chest", [3, 10, 4])?;
        assert_eq!(world.visits, 1);
        assert_eq!(random.next_long(), expected.next_long());
        world.exists = true;
        handle_data_marker(&mut world, &mut random, "map_chest", [3, 10, 4])?;
        assert_eq!(
            world.loot,
            Some((
                "minecraft:chests/shipwreck_map".into(),
                expected.next_long()
            ))
        );
        Ok(())
    }
}
