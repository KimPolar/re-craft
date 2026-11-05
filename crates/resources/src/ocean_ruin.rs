//! Java 26.3 ocean-ruin template selection before block placement.
use anyhow::{Result, ensure};
use native_core::random::LegacyRandom;
use serde_json::Value;

pub use crate::structure_transform::Rotation;
use crate::{
    packs::ResourceId,
    structure_placement::large_feature,
    structure_transform::{Mirror, block_world_position},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Temperature {
    Warm,
    Cold,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub temperature: Temperature,
    pub large_probability: f32,
    pub cluster_probability: f32,
}

impl Settings {
    pub fn decode(value: &Value) -> Result<Self> {
        use anyhow::Context;
        let id = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("missing structure type")?,
        )?;
        ensure!(
            id.namespace == "minecraft" && id.path == "ocean_ruin",
            "not an ocean ruin structure"
        );
        let temperature = match value.get("biome_temp").and_then(Value::as_str) {
            Some("warm") => Temperature::Warm,
            Some("cold") => Temperature::Cold,
            _ => anyhow::bail!("invalid ocean ruin biome temperature"),
        };
        let probability = |name: &str| -> Result<f32> {
            let value = value
                .get(name)
                .and_then(Value::as_f64)
                .with_context(|| format!("invalid ocean ruin {name}"))?
                as f32;
            ensure!(
                value.is_finite() && (0.0..=1.0).contains(&value),
                "invalid ocean ruin {name}"
            );
            Ok(value)
        };
        Ok(Self {
            temperature,
            large_probability: probability("large_probability")?,
            cluster_probability: probability("cluster_probability")?,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub template: String,
    pub position: [i32; 3],
    pub rotation: Rotation,
    pub integrity: f32,
    pub large: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataMarkerKind {
    Chest,
    Drowned,
    Other,
}

/// World operations for an ocean ruin DATA marker, in Java call order.
/// Spawn implementations create a persistent STRUCTURE drowned, finalize it
/// against the current difficulty and add it with passengers before returning.
pub trait MarkerWorld {
    fn fluid_in_water_tag(&mut self, position: [i32; 3]) -> Result<bool>;
    /// Attempt setBlock with the default chest state and Java flag 2.
    fn set_default_chest(&mut self, position: [i32; 3], waterlogged: bool) -> Result<()>;
    fn chest_entity_exists(&mut self, position: [i32; 3]) -> Result<bool>;
    fn set_chest_loot(&mut self, position: [i32; 3], table: &str, seed: i64) -> Result<()>;
    /// Returns false only when Java's entity creation returned null.
    fn create_and_spawn_drowned(&mut self, position: [i32; 3]) -> Result<bool>;
    fn sea_level(&mut self) -> Result<i32>;
    /// Set default water or air with Java flag 2 after a successful spawn.
    fn clear_drowned_marker(&mut self, position: [i32; 3], water: bool) -> Result<()>;
}

/// Execute one already-clipped DATA marker. Callers must discard their world
/// draft and random state on error, including callback-owned spawn state.
pub fn handle_data_marker(
    world: &mut impl MarkerWorld,
    random: &mut impl crate::structure_piece::TreasureRandom,
    kind: DataMarkerKind,
    position: [i32; 3],
    large: bool,
) -> Result<()> {
    match kind {
        DataMarkerKind::Chest => {
            let waterlogged = world.fluid_in_water_tag(position)?;
            world.set_default_chest(position, waterlogged)?;
            if world.chest_entity_exists(position)? {
                let table = if large {
                    "minecraft:chests/underwater_ruin_big"
                } else {
                    "minecraft:chests/underwater_ruin_small"
                };
                world.set_chest_loot(position, table, random.next_long())?;
            }
        }
        DataMarkerKind::Drowned => {
            if world.create_and_spawn_drowned(position)? {
                let water = position[1] <= world.sea_level()?;
                world.clear_drowned_marker(position, water)?;
            }
        }
        DataMarkerKind::Other => {}
    }
    Ok(())
}

/// Access to the current world used by Java's ocean-floor adjustment.
pub trait FloorWorld {
    fn min_y(&self) -> i32;
    /// First available Y of the current OCEAN_FLOOR_WG heightmap.
    fn ocean_floor_height(&mut self, x: i32, z: i32) -> Result<i32>;
    /// True for air, a fluid in the active water tag, or a block in the
    /// active ice tag. Waterlogged blocks must use their actual fluid state.
    fn floor_scan_passable(&mut self, position: [i32; 3]) -> Result<bool>;
}

/// Find the postprocessing origin before processing or placing a piece.
pub fn floor_position(
    piece: &Piece,
    size: [i32; 3],
    world: &mut impl FloorWorld,
) -> Result<[i32; 3]> {
    let [x, _, z] = piece.position;
    let initial_y = world.ocean_floor_height(x, z)?;
    let start = [x, initial_y, z];
    let end = block_world_position(
        [size[0].wrapping_sub(1), 0, size[2].wrapping_sub(1)],
        start,
        Mirror::None,
        piece.rotation,
        [0; 3],
    );
    let mut lowest = 512_i32;
    let reference_y = initial_y.wrapping_sub(1);
    let mut deep_columns = 0_i32;
    for z in start[2].min(end[2])..=start[2].max(end[2]) {
        for x in start[0].min(end[0])..=start[0].max(end[0]) {
            let mut y = reference_y;
            loop {
                let passable = world.floor_scan_passable([x, y, z])?;
                if !passable || y <= world.min_y().wrapping_add(1) {
                    break;
                }
                y = y.wrapping_sub(1);
            }
            lowest = lowest.min(y);
            if y < reference_y.wrapping_sub(2) {
                deep_columns = deep_columns.wrapping_add(1);
            }
        }
    }
    let width = start[0].wrapping_sub(end[0]).wrapping_abs();
    let adjusted = if reference_y.wrapping_sub(lowest) > 2 && deep_columns > width.wrapping_sub(2) {
        lowest.wrapping_add(1)
    } else {
        initial_y
    };
    Ok([x, adjusted, z])
}

fn add_piece(
    out: &mut Vec<Piece>,
    random: &mut LegacyRandom,
    position: [i32; 3],
    rotation: Rotation,
    temperature: Temperature,
    large: bool,
    integrity: f32,
) {
    let index = random.next_int_bound(if large { 4 } else { 8 }).unwrap() as usize;
    let number = if large {
        [1, 2, 3, 8][index]
    } else {
        index + 1
    };
    let prefixes: &[(&str, f32)] = match (temperature, large) {
        (Temperature::Warm, true) => &[("big_warm", integrity)],
        (Temperature::Warm, false) => &[("warm", integrity)],
        (Temperature::Cold, true) => &[
            ("big_brick", integrity),
            ("big_cracked", 0.7),
            ("big_mossy", 0.5),
        ],
        (Temperature::Cold, false) => &[("brick", integrity), ("cracked", 0.7), ("mossy", 0.5)],
    };
    for &(prefix, integrity) in prefixes {
        let number = if prefix == "big_warm" {
            index + 4
        } else {
            number
        };
        out.push(Piece {
            template: format!("minecraft:underwater_ruin/{prefix}_{number}"),
            position,
            rotation,
            integrity,
            large,
        });
    }
}

fn random_between(random: &mut LegacyRandom, min: i32, max: i32) -> i32 {
    min + random.next_int_bound(max - min + 1).unwrap()
}

fn intersects(a: [i32; 4], b: [i32; 4]) -> bool {
    a[0] <= b[2] && a[2] >= b[0] && a[1] <= b[3] && a[3] >= b[1]
}

fn bounds(a: [i32; 2], b: [i32; 2]) -> [i32; 4] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[0].max(b[0]),
        a[1].max(b[1]),
    ]
}

/// The caller supplies the structure's already-seeded Java random source.
/// The returned records are intermediate; they do not place template blocks.
pub fn select_pieces(
    random: &mut LegacyRandom,
    origin: [i32; 3],
    temperature: Temperature,
    large_probability: f32,
    cluster_probability: f32,
) -> Result<Vec<Piece>> {
    ensure!(
        large_probability.is_finite() && (0.0..=1.0).contains(&large_probability),
        "invalid large probability"
    );
    ensure!(
        cluster_probability.is_finite() && (0.0..=1.0).contains(&cluster_probability),
        "invalid cluster probability"
    );
    let rotation = Rotation::random(random);
    let large = random.next_float() <= large_probability;
    let mut pieces = Vec::new();
    add_piece(
        &mut pieces,
        random,
        origin,
        rotation,
        temperature,
        large,
        if large { 0.9 } else { 0.8 },
    );
    if !large || random.next_float() > cluster_probability {
        return Ok(pieces);
    }

    let base = [origin[0], origin[2]];
    let offset = rotation.transform_horizontal([15, 15]);
    let other = [
        base[0].wrapping_add(offset[0]),
        base[1].wrapping_add(offset[1]),
    ];
    let main = bounds(base, other);
    let anchor = [main[0], main[1]];
    let mut positions = Vec::with_capacity(8);
    for (xbase, xmin, xmax, zbase, zmin, zmax) in [
        (-16, 1, 8, 16, 1, 7),
        (-16, 1, 8, 0, 1, 7),
        (-16, 1, 8, -16, 4, 8),
        (0, 1, 7, 16, 1, 7),
        (0, 1, 7, -16, 4, 6),
        (16, 1, 7, 16, 3, 8),
        (16, 1, 7, 0, 1, 7),
        (16, 1, 7, -16, 4, 8),
    ] {
        let x = xbase + random_between(random, xmin, xmax);
        let z = zbase + random_between(random, zmin, zmax);
        positions.push([anchor[0].wrapping_add(x), anchor[1].wrapping_add(z)]);
    }
    let count = random_between(random, 4, 8);
    for _ in 0..count {
        if positions.is_empty() {
            break;
        }
        let selected = random.next_int_bound(positions.len() as i32).unwrap() as usize;
        let pos = positions.remove(selected);
        let rotation = Rotation::random(random);
        let offset = rotation.transform_horizontal([5, 6]);
        let end = [
            pos[0].wrapping_add(offset[0]),
            pos[1].wrapping_add(offset[1]),
        ];
        if !intersects(main, bounds(pos, end)) {
            add_piece(
                &mut pieces,
                random,
                [pos[0], 90, pos[1]],
                rotation,
                temperature,
                false,
                0.8,
            );
        }
    }
    Ok(pieces)
}

/// Select the initial pieces using Java's large-feature seed for this chunk.
pub fn select_for_chunk(seed: i64, chunk: [i32; 2], settings: Settings) -> Result<Vec<Piece>> {
    let x = chunk[0]
        .checked_mul(16)
        .ok_or_else(|| anyhow::anyhow!("chunk X overflow"))?;
    let z = chunk[1]
        .checked_mul(16)
        .ok_or_else(|| anyhow::anyhow!("chunk Z overflow"))?;
    let mut random = large_feature(seed, chunk[0], chunk[1]);
    select_pieces(
        &mut random,
        [x, 90, z],
        settings.temperature,
        settings.large_probability,
        settings.cluster_probability,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct MarkerTestWorld {
        calls: Vec<String>,
        water: bool,
        chest: bool,
        spawned: bool,
        sea: i32,
    }
    impl MarkerWorld for MarkerTestWorld {
        fn fluid_in_water_tag(&mut self, _: [i32; 3]) -> Result<bool> {
            self.calls.push("fluid".into());
            Ok(self.water)
        }
        fn set_default_chest(&mut self, _: [i32; 3], waterlogged: bool) -> Result<()> {
            self.calls.push(format!("chest:{waterlogged}"));
            Ok(())
        }
        fn chest_entity_exists(&mut self, _: [i32; 3]) -> Result<bool> {
            self.calls.push("entity".into());
            Ok(self.chest)
        }
        fn set_chest_loot(&mut self, _: [i32; 3], table: &str, seed: i64) -> Result<()> {
            self.calls.push(format!("loot:{table}:{seed}"));
            Ok(())
        }
        fn create_and_spawn_drowned(&mut self, _: [i32; 3]) -> Result<bool> {
            self.calls.push("spawn".into());
            Ok(self.spawned)
        }
        fn sea_level(&mut self) -> Result<i32> {
            self.calls.push("sea".into());
            Ok(self.sea)
        }
        fn clear_drowned_marker(&mut self, _: [i32; 3], water: bool) -> Result<()> {
            self.calls.push(format!("clear:{water}"));
            Ok(())
        }
    }

    #[test]
    fn marker_operations_preserve_java_order_and_random_consumption() {
        let mut world = MarkerTestWorld {
            water: true,
            chest: true,
            sea: 63,
            ..Default::default()
        };
        let mut random = LegacyRandom::new(42);
        let mut expected = random.clone();
        let loot_seed = expected.next_long();
        handle_data_marker(
            &mut world,
            &mut random,
            DataMarkerKind::Chest,
            [3, 4, 5],
            true,
        )
        .unwrap();
        assert_eq!(
            world.calls,
            [
                "fluid".to_string(),
                "chest:true".into(),
                "entity".into(),
                format!("loot:minecraft:chests/underwater_ruin_big:{loot_seed}")
            ]
        );
        assert_eq!(random.next_long(), expected.next_long());
        world.calls.clear();
        world.chest = false;
        let mut expected = random.clone();
        handle_data_marker(
            &mut world,
            &mut random,
            DataMarkerKind::Chest,
            [3, 4, 5],
            false,
        )
        .unwrap();
        assert_eq!(world.calls, ["fluid", "chest:true", "entity"]);
        assert_eq!(random.next_long(), expected.next_long());
        world.calls.clear();
        handle_data_marker(
            &mut world,
            &mut random,
            DataMarkerKind::Drowned,
            [3, 63, 5],
            false,
        )
        .unwrap();
        assert_eq!(world.calls, ["spawn"]);
        world.calls.clear();
        world.spawned = true;
        handle_data_marker(
            &mut world,
            &mut random,
            DataMarkerKind::Drowned,
            [3, 63, 5],
            false,
        )
        .unwrap();
        assert_eq!(world.calls, ["spawn", "sea", "clear:true"]);
        world.calls.clear();
        handle_data_marker(
            &mut world,
            &mut random,
            DataMarkerKind::Drowned,
            [3, 64, 5],
            false,
        )
        .unwrap();
        assert_eq!(world.calls, ["spawn", "sea", "clear:false"]);
        world.calls.clear();
        handle_data_marker(
            &mut world,
            &mut random,
            DataMarkerKind::Other,
            [3, 4, 5],
            false,
        )
        .unwrap();
        assert!(world.calls.is_empty());
    }

    struct Floor {
        height: i32,
        bottom: i32,
        fail: bool,
    }
    impl FloorWorld for Floor {
        fn min_y(&self) -> i32 {
            -64
        }
        fn ocean_floor_height(&mut self, _: i32, _: i32) -> Result<i32> {
            Ok(self.height)
        }
        fn floor_scan_passable(&mut self, p: [i32; 3]) -> Result<bool> {
            ensure!(!self.fail, "missing column");
            Ok(p[1] > self.bottom)
        }
    }

    #[test]
    fn adjusts_to_deep_floor_and_propagates_read_failure() {
        let piece = Piece {
            template: "minecraft:test".into(),
            position: [4, 90, -5],
            rotation: Rotation::Clockwise90,
            integrity: 0.8,
            large: false,
        };
        let mut floor = Floor {
            height: 64,
            bottom: 58,
            fail: false,
        };
        assert_eq!(
            floor_position(&piece, [5, 4, 3], &mut floor).unwrap(),
            [4, 59, -5]
        );
        floor.bottom = 61;
        assert_eq!(
            floor_position(&piece, [5, 4, 3], &mut floor).unwrap(),
            [4, 64, -5]
        );
        floor.fail = true;
        assert!(floor_position(&piece, [5, 4, 3], &mut floor).is_err());
    }
    #[test]
    fn rejects_invalid_probabilities_without_consuming_randomness() {
        let mut random = LegacyRandom::new(42);
        let before = random.state();
        assert!(select_pieces(&mut random, [0, 90, 0], Temperature::Warm, f32::NAN, 0.5).is_err());
        assert!(select_pieces(&mut random, [0, 90, 0], Temperature::Warm, 0.5, 1.1).is_err());
        assert_eq!(random.state(), before);
    }
}
