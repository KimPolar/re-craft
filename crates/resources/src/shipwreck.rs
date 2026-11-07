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
    let x = chunk[0].checked_mul(16).context("chunk X overflow")?;
    let z = chunk[1].checked_mul(16).context("chunk Z overflow")?;
    Ok(select_piece(
        &mut large_feature(seed, chunk[0], chunk[1]),
        [x, 90, z],
        beached,
    ))
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
}
