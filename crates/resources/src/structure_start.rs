//! Initial structure starts for 26.3's simple surface structures.
//!
//! These are generation-time piece bounds, before decoration or block placement.
//! Callers must provide vanilla-compatible biome and base-height sampling.
use anyhow::{Context, Result, bail, ensure};
use native_core::beardifier::{BoundingBox, PieceProjection, StructurePiece, TerrainAdjustment};
use serde_json::Value;

use crate::{packs::ResourceId, structure_placement::large_feature};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Heightmap {
    WorldSurface,
    OceanFloor,
}

/// Java uses different resolvers for the preliminary column and final point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BiomeQuery {
    Column,
    Point,
}

/// Sampling must use the unadjusted base generator, as Java does during structure starts.
pub trait StartEnvironment {
    /// Inclusive dimension bounds used for the preliminary biome-column check.
    fn vertical_bounds(&self) -> (i32, i32);
    fn sea_level(&self) -> i32;
    fn first_occupied_height(&mut self, x: i32, z: i32, map: Heightmap) -> Result<i32>;
    fn biome_allowed(&mut self, quart: [i32; 3], query: BiomeQuery) -> Result<bool>;
}

/// Center-column and final biome checks for structures using Java's
/// onTopOfChunkCenter. A point does not imply that its pieces are supported.
pub fn center_generation_point(
    env: &mut impl StartEnvironment,
    chunk: [i32; 2],
    map: Heightmap,
) -> Result<Option<[i32; 3]>> {
    let (min_y, max_y) = env.vertical_bounds();
    ensure!(
        min_y <= max_y && i64::from(max_y) - i64::from(min_y) <= 4096,
        "invalid dimension bounds"
    );
    let x = chunk[0]
        .checked_mul(16)
        .and_then(|x| x.checked_add(8))
        .context("chunk X overflow")?;
    let z = chunk[1]
        .checked_mul(16)
        .and_then(|z| z.checked_add(8))
        .context("chunk Z overflow")?;
    let (qx, qz) = (x.div_euclid(4), z.div_euclid(4));
    let mut possible = false;
    for qy in min_y.wrapping_sub(1).div_euclid(4)..=max_y.div_euclid(4) {
        if env.biome_allowed([qx, qy, qz], BiomeQuery::Column)? {
            possible = true;
            break;
        }
    }
    if !possible {
        return Ok(None);
    }
    let y = env.first_occupied_height(x, z, map)?;
    if !env.biome_allowed([qx, y.div_euclid(4), qz], BiomeQuery::Point)? {
        return Ok(None);
    }
    Ok(Some([x, y, z]))
}

/// Identify necessary center checks for structures using onTopOfChunkCenter.
/// A positive candidate still needs its structure implementation and any
/// additional biome checks before it can generate.
pub fn structure_center_precheck_heightmap(value: &Value) -> Result<Option<Heightmap>> {
    let id = ResourceId::parse(
        value
            .get("type")
            .and_then(Value::as_str)
            .context("missing structure type")?,
    )?;
    let map = match (id.namespace.as_str(), id.path.as_str()) {
        ("minecraft", "shipwreck") => {
            if value
                .get("is_beached")
                .and_then(Value::as_bool)
                .context("invalid shipwreck settings")?
            {
                Heightmap::WorldSurface
            } else {
                Heightmap::OceanFloor
            }
        }
        ("minecraft", "igloo") => Heightmap::WorldSurface,
        ("minecraft", "ocean_ruin") => {
            crate::ocean_ruin::Settings::decode(value)?;
            Heightmap::OceanFloor
        }
        ("minecraft", "ocean_monument") => Heightmap::OceanFloor,
        _ => return Ok(None),
    };
    decode_terrain_adjustment(value)?;
    Ok(Some(map))
}

/// Resolve the structure's terrain adaptation from its active definition.
pub fn decode_terrain_adjustment(value: &Value) -> Result<TerrainAdjustment> {
    let adjustment = value
        .get("terrain_adaptation")
        .map(|v| v.as_str().context("invalid terrain adaptation"))
        .transpose()?
        .unwrap_or("none");
    match adjustment {
        "none" => Ok(TerrainAdjustment::None),
        "bury" => Ok(TerrainAdjustment::Bury),
        "beard_thin" => Ok(TerrainAdjustment::BeardThin),
        "beard_box" => Ok(TerrainAdjustment::BeardBox),
        "encapsulate" => Ok(TerrainAdjustment::Encapsulate),
        _ => bail!("unsupported terrain adaptation {adjustment}"),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SimpleStructureKind {
    DesertPyramid,
    JungleTemple,
    SwampHut,
    BuriedTreasure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HorizontalDirection {
    North,
    East,
    South,
    West,
}

#[derive(Clone, Debug)]
pub struct SimpleStructure {
    pub kind: SimpleStructureKind,
    pub terrain_adjustment: TerrainAdjustment,
}

#[derive(Clone, Debug)]
pub struct SimpleStructureStart {
    pub kind: SimpleStructureKind,
    pub chunk: [i32; 2],
    /// Position used for the final biome check; not the piece's origin.
    pub generation_point: [i32; 3],
    pub orientation: Option<HorizontalDirection>,
    pub bounds: BoundingBox,
    pub terrain_adjustment: TerrainAdjustment,
}

impl SimpleStructureStart {
    pub fn terrain_piece(&self) -> StructurePiece {
        StructurePiece {
            bounds: self.bounds,
            adjustment: self.terrain_adjustment,
            ground_delta: 0,
            projection: PieceProjection::NonPool,
            junctions: vec![],
        }
    }
}

impl SimpleStructure {
    /// Decode the structure type and terrain adjustment from active pack data.
    /// Biome selectors and other settings are resolved separately by the caller.
    pub fn decode(value: &Value) -> Result<Self> {
        let id = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("missing structure type")?,
        )?;
        let kind = match (id.namespace.as_str(), id.path.as_str()) {
            ("minecraft", "desert_pyramid") => SimpleStructureKind::DesertPyramid,
            ("minecraft", "jungle_temple") => SimpleStructureKind::JungleTemple,
            ("minecraft", "swamp_hut") => SimpleStructureKind::SwampHut,
            ("minecraft", "buried_treasure") => SimpleStructureKind::BuriedTreasure,
            _ => bail!("unsupported simple structure {}:{}", id.namespace, id.path),
        };
        let terrain_adjustment = decode_terrain_adjustment(value)?;
        Ok(Self {
            kind,
            terrain_adjustment,
        })
    }

    /// Attempt one start after set placement and existing-start checks.
    /// No piece blocks, entities or loot are generated by this method.
    pub fn generate(
        &self,
        env: &mut impl StartEnvironment,
        seed: i64,
        chunk: [i32; 2],
    ) -> Result<Option<SimpleStructureStart>> {
        let (min_y, max_y) = env.vertical_bounds();
        ensure!(
            min_y <= max_y && i64::from(max_y) - i64::from(min_y) <= 4096,
            "invalid dimension bounds"
        );
        let x = chunk[0].checked_mul(16).context("chunk X overflow")?;
        let z = chunk[1].checked_mul(16).context("chunk Z overflow")?;
        let center_x = x.checked_add(8).context("block X overflow")?;
        let center_z = z.checked_add(8).context("block Z overflow")?;
        let mut possible = false;
        for qy in min_y.wrapping_sub(1).div_euclid(4)..=max_y.div_euclid(4) {
            if env.biome_allowed(
                [center_x.div_euclid(4), qy, center_z.div_euclid(4)],
                BiomeQuery::Column,
            )? {
                possible = true;
                break;
            }
        }
        if !possible {
            return Ok(None);
        }
        let dimensions = match self.kind {
            SimpleStructureKind::DesertPyramid => Some([21, 15, 21]),
            SimpleStructureKind::JungleTemple => Some([12, 10, 15]),
            SimpleStructureKind::SwampHut => Some([7, 7, 9]),
            SimpleStructureKind::BuriedTreasure => None,
        };
        if matches!(
            self.kind,
            SimpleStructureKind::DesertPyramid | SimpleStructureKind::JungleTemple
        ) {
            let [width, _, depth] = dimensions.unwrap();
            let x2 = x.checked_add(width).context("structure X overflow")?;
            let z2 = z.checked_add(depth).context("structure Z overflow")?;
            let mut lowest = i32::MAX;
            for [px, pz] in [[x, z], [x, z2], [x2, z], [x2, z2]] {
                lowest = lowest.min(env.first_occupied_height(px, pz, Heightmap::WorldSurface)?);
            }
            if lowest < env.sea_level() {
                return Ok(None);
            }
        }
        let heightmap = if self.kind == SimpleStructureKind::BuriedTreasure {
            Heightmap::OceanFloor
        } else {
            Heightmap::WorldSurface
        };
        let y = env.first_occupied_height(center_x, center_z, heightmap)?;
        if !env.biome_allowed(
            [
                center_x.div_euclid(4),
                y.div_euclid(4),
                center_z.div_euclid(4),
            ],
            BiomeQuery::Point,
        )? {
            return Ok(None);
        }
        let (orientation, bounds) = if let Some([width, height, depth]) = dimensions {
            let direction = match large_feature(seed, chunk[0], chunk[1])
                .next_int_bound(4)
                .expect("positive bound")
            {
                0 => HorizontalDirection::North,
                1 => HorizontalDirection::East,
                2 => HorizontalDirection::South,
                _ => HorizontalDirection::West,
            };
            let (size_x, size_z) = if matches!(
                direction,
                HorizontalDirection::North | HorizontalDirection::South
            ) {
                (width, depth)
            } else {
                (depth, width)
            };
            let max_x = x.checked_add(size_x - 1).context("piece X overflow")?;
            let max_z = z.checked_add(size_z - 1).context("piece Z overflow")?;
            (
                Some(direction),
                BoundingBox::new([x, 64, z], [max_x, 64 + height - 1, max_z])
                    .map_err(anyhow::Error::msg)?,
            )
        } else {
            let p = [
                x.checked_add(9).context("piece X overflow")?,
                90,
                z.checked_add(9).context("piece Z overflow")?,
            ];
            (None, BoundingBox::new(p, p).map_err(anyhow::Error::msg)?)
        };
        Ok(Some(SimpleStructureStart {
            kind: self.kind,
            chunk,
            generation_point: [center_x, y, center_z],
            orientation,
            bounds,
            terrain_adjustment: self.terrain_adjustment,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Env {
        calls: Vec<(BiomeQuery, [i32; 3])>,
        heights: Vec<(i32, i32, Heightmap)>,
    }
    impl StartEnvironment for Env {
        fn vertical_bounds(&self) -> (i32, i32) {
            (-64, 319)
        }
        fn sea_level(&self) -> i32 {
            63
        }
        fn first_occupied_height(&mut self, x: i32, z: i32, map: Heightmap) -> Result<i32> {
            self.heights.push((x, z, map));
            Ok(75)
        }
        fn biome_allowed(&mut self, quart: [i32; 3], query: BiomeQuery) -> Result<bool> {
            self.calls.push((query, quart));
            Ok(true)
        }
    }
    #[test]
    fn center_prechecks_validate_type_specific_and_common_settings() {
        assert_eq!(
            structure_center_precheck_heightmap(&json!({"type":"shipwreck","is_beached":false}))
                .unwrap(),
            Some(Heightmap::OceanFloor)
        );
        assert_eq!(
            structure_center_precheck_heightmap(&json!({"type":"minecraft:igloo"})).unwrap(),
            Some(Heightmap::WorldSurface)
        );
        assert!(structure_center_precheck_heightmap(&json!({"type":"shipwreck"})).is_err());
        assert!(
            structure_center_precheck_heightmap(
                &json!({"type":"shipwreck","is_beached":false,"terrain_adaptation":"invalid"})
            )
            .is_err()
        );
        assert_eq!(structure_center_precheck_heightmap(&json!({"type":"ocean_ruin","biome_temp":"warm","large_probability":0.3,"cluster_probability":0.9})).unwrap(), Some(Heightmap::OceanFloor));
        assert!(structure_center_precheck_heightmap(&json!({"type":"ocean_ruin","biome_temp":"warm","large_probability":1.1,"cluster_probability":0.9})).is_err());
        assert_eq!(
            structure_center_precheck_heightmap(&json!({"type":"ocean_monument"})).unwrap(),
            Some(Heightmap::OceanFloor)
        );
        assert_eq!(
            structure_center_precheck_heightmap(&json!({"type":"minecraft:village"})).unwrap(),
            None
        );
    }
    #[test]
    fn resolves_generation_biome_and_initial_piece_without_placing_blocks() {
        let structure = SimpleStructure::decode(
            &json!({"type":"minecraft:jungle_temple","terrain_adaptation":"beard_box"}),
        )
        .unwrap();
        let mut env = Env {
            calls: vec![],
            heights: vec![],
        };
        let start = structure.generate(&mut env, 42, [-1, 2]).unwrap().unwrap();
        assert_eq!(start.generation_point, [-8, 75, 40]);
        assert_eq!(
            env.calls,
            vec![
                (BiomeQuery::Column, [-2, -17, 10]),
                (BiomeQuery::Point, [-2, 18, 10])
            ]
        );
        assert_eq!(
            env.heights,
            vec![
                (-16, 32, Heightmap::WorldSurface),
                (-16, 47, Heightmap::WorldSurface),
                (-4, 32, Heightmap::WorldSurface),
                (-4, 47, Heightmap::WorldSurface),
                (-8, 40, Heightmap::WorldSurface)
            ]
        );
        assert_eq!(
            start.terrain_piece().adjustment,
            TerrainAdjustment::BeardBox
        );
        assert!(SimpleStructure::decode(&json!({"type":"minecraft:village"})).is_err());
        assert!(
            SimpleStructure::decode(
                &json!({"type":"minecraft:swamp_hut","terrain_adaptation":"unknown"})
            )
            .is_err()
        );
    }
}
