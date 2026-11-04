//! Chunk-generation creature positions and entity-type spawn boxes.
use anyhow::{Context, Result, ensure};

/// Unscaled 26.3 entity-type dimensions and its spawn-dimension multiplier.
/// These values must come from the verified entity type, not the mob's later
/// instance dimensions or a resource pack.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CreatureSpawnDimensions {
    pub width: f32,
    pub height: f32,
    pub spawn_scale: f32,
}

/// Built-in 26.3 type dimensions verified against the installed client.
/// Other types must be resolved separately before their collision is tested.
pub fn known_creature_spawn_dimensions(
    entity: &crate::packs::ResourceId,
) -> Option<CreatureSpawnDimensions> {
    if entity.namespace != "minecraft" {
        return None;
    }
    let (width, height, spawn_scale) = match entity.path.as_str() {
        "pig" => (0.9, 0.9, 1.0),
        "cow" => (0.9, 1.4, 1.0),
        "sheep" | "goat" => (0.9, 1.3, 1.0),
        "chicken" => (0.4, 0.7, 1.0),
        "horse" => (1.3964844, 1.6, 1.0),
        "donkey" => (1.3964844, 1.5, 1.0),
        "rabbit" => (0.49, 0.6, 1.0),
        "slime" | "magma_cube" => (0.52, 0.52, 4.0),
        "sulfur_cube" => (0.49, 0.49, 2.0),
        _ => return None,
    };
    Some(CreatureSpawnDimensions {
        width,
        height,
        spawn_scale,
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CreatureSpawnGeometry {
    pub position: [f64; 3],
    pub min: [f64; 3],
    pub max: [f64; 3],
}

/// Java clamps the integer candidate with the unscaled type width, then
/// multiplies spawn dimensions as floats before widening them to doubles.
pub fn creature_spawn_geometry(
    chunk_x: i32,
    chunk_z: i32,
    x: i32,
    y: i32,
    z: i32,
    dimensions: CreatureSpawnDimensions,
) -> Result<CreatureSpawnGeometry> {
    let CreatureSpawnDimensions {
        width,
        height,
        spawn_scale,
    } = dimensions;
    ensure!(
        width.is_finite()
            && width >= 0.0
            && height.is_finite()
            && height >= 0.0
            && spawn_scale.is_finite()
            && spawn_scale >= 0.0,
        "invalid entity spawn dimensions"
    );
    let min_x = chunk_x.checked_mul(16).context("spawn chunk X overflow")?;
    let min_z = chunk_z.checked_mul(16).context("spawn chunk Z overflow")?;
    let max_x = min_x.checked_add(16).context("spawn chunk X overflow")?;
    let max_z = min_z.checked_add(16).context("spawn chunk Z overflow")?;
    ensure!(
        (min_x..max_x).contains(&x) && (min_z..max_z).contains(&z),
        "spawn candidate is outside its chunk"
    );
    // Mth.clamp checks the lower bound first even when min exceeds max.
    let clamp = |value: f64, min: f64, max: f64| if value < min { min } else { value.min(max) };
    let width64 = f64::from(width);
    let px = clamp(
        f64::from(x),
        f64::from(min_x) + width64,
        f64::from(max_x) - width64,
    );
    let pz = clamp(
        f64::from(z),
        f64::from(min_z) + width64,
        f64::from(max_z) - width64,
    );
    let half = f64::from((spawn_scale * width) / 2.0_f32);
    let tall = f64::from(spawn_scale * height);
    ensure!(
        half.is_finite() && tall.is_finite(),
        "entity spawn dimensions overflow"
    );
    let py = f64::from(y);
    Ok(CreatureSpawnGeometry {
        position: [px, py, pz],
        min: [px - half, py, pz - half],
        max: [px + half, py + tall, pz + half],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clamps_negative_chunk_using_unscaled_width() {
        let dims = CreatureSpawnDimensions {
            width: 0.9,
            height: 1.4,
            spawn_scale: 0.75,
        };
        let geometry = creature_spawn_geometry(-1, 2, -16, 64, 47, dims).unwrap();
        assert_eq!(geometry.position, [-16.0 + f64::from(0.9_f32), 64.0, 47.0]);
        assert_eq!(
            geometry.min[0],
            geometry.position[0] - f64::from((0.75_f32 * 0.9) / 2.0)
        );
        assert!(creature_spawn_geometry(-1, 2, 0, 64, 47, dims).is_err());
        assert!(creature_spawn_geometry(i32::MAX, 2, 0, 64, 47, dims).is_err());
        assert!(
            known_creature_spawn_dimensions(
                &crate::packs::ResourceId::parse("example:pig").unwrap()
            )
            .is_none()
        );
        assert!(
            known_creature_spawn_dimensions(
                &crate::packs::ResourceId::parse("minecraft:unknown").unwrap()
            )
            .is_none()
        );
    }
}
