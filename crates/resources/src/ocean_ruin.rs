//! Java 26.3 ocean-ruin template selection before block placement.
use anyhow::{Result, ensure};
use native_core::random::LegacyRandom;
use serde_json::Value;

use crate::{packs::ResourceId, structure_placement::large_feature};

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Rotation {
    None,
    Clockwise90,
    Clockwise180,
    Counterclockwise90,
}

impl Rotation {
    fn random(random: &mut LegacyRandom) -> Self {
        match random.next_int_bound(4).unwrap() {
            0 => Self::None,
            1 => Self::Clockwise90,
            2 => Self::Clockwise180,
            _ => Self::Counterclockwise90,
        }
    }
    fn transform(self, [x, z]: [i32; 2]) -> [i32; 2] {
        match self {
            Self::None => [x, z],
            Self::Clockwise90 => [z.wrapping_neg(), x],
            Self::Clockwise180 => [x.wrapping_neg(), z.wrapping_neg()],
            Self::Counterclockwise90 => [z, x.wrapping_neg()],
        }
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
    let offset = rotation.transform([15, 15]);
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
        let offset = rotation.transform([5, 6]);
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
    #[test]
    fn rejects_invalid_probabilities_without_consuming_randomness() {
        let mut random = LegacyRandom::new(42);
        let before = random.state();
        assert!(select_pieces(&mut random, [0, 90, 0], Temperature::Warm, f32::NAN, 0.5).is_err());
        assert!(select_pieces(&mut random, [0, 90, 0], Temperature::Warm, 0.5, 1.1).is_err());
        assert_eq!(random.state(), before);
    }
}
