//! Java's seeded block-to-quart biome lookup.

use glam::IVec3;
use sha2::{Digest, Sha256};

/// Java hashes the little-endian world seed and reads the first eight bytes little-endian.
pub fn obfuscate_seed(seed: i64) -> i64 {
    let digest = Sha256::digest(seed.to_le_bytes());
    i64::from_le_bytes(digest[..8].try_into().expect("SHA-256 has 32 bytes"))
}

fn next(seed: i64, salt: i64) -> i64 {
    seed.wrapping_mul(
        seed.wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407),
    )
    .wrapping_add(salt)
}

fn fiddle(seed: i64) -> f64 {
    (((seed >> 24).rem_euclid(1024) as f64 / 1024.0) - 0.5) * 0.9
}

fn distance(seed: i64, pos: IVec3, fraction: [f64; 3]) -> f64 {
    let mut state = seed;
    for salt in [pos.x, pos.y, pos.z, pos.x, pos.y, pos.z] {
        state = next(state, i64::from(salt));
    }
    let x = fiddle(state);
    state = next(state, seed);
    let y = fiddle(state);
    state = next(state, seed);
    let z = fiddle(state);
    let dx = fraction[0] + x;
    let dy = fraction[1] + y;
    let dz = fraction[2] + z;
    dz * dz + dy * dy + dx * dx
}

/// Select the noise-biome quart coordinate for a block, as BiomeManager does.
pub fn quart_position(zoom_seed: i64, block: IVec3) -> IVec3 {
    let shifted = [
        block.x.wrapping_sub(2),
        block.y.wrapping_sub(2),
        block.z.wrapping_sub(2),
    ];
    let base = IVec3::new(shifted[0] >> 2, shifted[1] >> 2, shifted[2] >> 2);
    let fraction = shifted.map(|value| f64::from(value & 3) / 4.0);
    let mut best = f64::INFINITY;
    let mut selected = base;
    for index in 0..8 {
        let delta = IVec3::new((index >> 2) & 1, (index >> 1) & 1, index & 1);
        let sample = base + delta;
        let d = distance(
            zoom_seed,
            sample,
            [
                fraction[0] - f64::from(delta.x),
                fraction[1] - f64::from(delta.y),
                fraction[2] - f64::from(delta.z),
            ],
        );
        if d < best {
            best = d;
            selected = sample;
        }
    }
    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_selection_is_one_of_the_eight_neighboring_quarts() {
        for seed in [0, 1, -1, i64::MIN, i64::MAX] {
            for position in [
                IVec3::ZERO,
                IVec3::new(-17, 64, 31),
                IVec3::new(i32::MIN, i32::MAX, 0),
            ] {
                let shifted = position.to_array().map(|n| n.wrapping_sub(2) >> 2);
                let selected = quart_position(obfuscate_seed(seed), position).to_array();
                for axis in 0..3 {
                    assert!((shifted[axis]..=shifted[axis] + 1).contains(&selected[axis]));
                }
            }
        }
    }
}
