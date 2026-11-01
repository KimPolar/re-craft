//! Chunk-local 26.3 carver geometry and carving masks.
//! Aquifer application and world integration are separate stages.
use anyhow::{Context, Result, ensure};

pub mod apply;
pub mod canyon;
pub mod cave;
pub mod schedule;
pub mod tags;

fn source_random(
    seed: i64,
    index: i32,
    source: [i32; 2],
    probability: f32,
) -> Option<native_core::random::LegacyRandom> {
    use native_core::random::LegacyRandom;
    let seed = seed.wrapping_add(i64::from(index));
    let mut random = LegacyRandom::new(seed);
    let a = random.next_long();
    let b = random.next_long();
    let mixed = i64::from(source[0]).wrapping_mul(a) ^ i64::from(source[1]).wrapping_mul(b) ^ seed;
    let mut random = LegacyRandom::new(mixed);
    (random.next_float() <= probability).then_some(random)
}

/// Bit order matches Java: local X, local Z, then Y.
pub struct CarvingMask {
    min_y: i32,
    max_y: i32,
    height: usize,
    bits: Vec<u64>,
}
impl CarvingMask {
    pub fn new(min_y: i32, max_y: i32) -> Result<Self> {
        let height = i64::from(max_y) - i64::from(min_y) + 1;
        ensure!((1..=4096).contains(&height), "invalid carving height range");
        let height = height as usize;
        Ok(Self {
            min_y,
            max_y,
            height,
            bits: vec![0; (height * 256).div_ceil(64)],
        })
    }
    pub fn min_y(&self) -> i32 {
        self.min_y
    }
    pub fn max_y(&self) -> i32 {
        self.max_y
    }
    pub fn is_empty(&self) -> bool {
        self.bits.iter().all(|word| *word == 0)
    }
    pub fn contains(&self, x: u8, y: i32, z: u8) -> bool {
        if x >= 16 || z >= 16 || y < self.min_y || y > self.max_y {
            return false;
        }
        let index = (usize::from(x) * 16 + usize::from(z)) * self.height
            + (i64::from(y) - i64::from(self.min_y)) as usize;
        self.bits[index / 64] & (1u64 << (index % 64)) != 0
    }
    fn carve(&mut self, x: u8, y: i32, z: u8) {
        let index = (usize::from(x) * 16 + usize::from(z)) * self.height
            + (i64::from(y) - i64::from(self.min_y)) as usize;
        self.bits[index / 64] |= 1u64 << (index % 64);
    }
    /// Contiguous set-bit runs in Java's X, Z, ascending-Y visitation order.
    pub fn segments(&self) -> Vec<(u8, u8, i32, i32)> {
        let mut result = Vec::new();
        for x in 0..16u8 {
            for z in 0..16u8 {
                let mut y = self.min_y;
                loop {
                    while y <= self.max_y && !self.contains(x, y, z) {
                        if y == self.max_y {
                            break;
                        }
                        y += 1;
                    }
                    if !self.contains(x, y, z) {
                        break;
                    }
                    let start = y;
                    while y < self.max_y && self.contains(x, y + 1, z) {
                        y += 1;
                    }
                    result.push((x, z, start, y));
                    if y == self.max_y {
                        break;
                    }
                    y += 1;
                }
            }
        }
        result
    }
}

fn floor_java(value: f64) -> i32 {
    let integral = value as i32;
    if value < f64::from(integral) {
        integral.wrapping_sub(1)
    } else {
        integral
    }
}

/// Java's reach test for a tunnel step. The radius addition is done in float.
pub fn can_reach(
    chunk: [i32; 2],
    x: f64,
    z: f64,
    current: i32,
    total: i32,
    thickness: f32,
) -> bool {
    let mid_x = chunk[0].wrapping_mul(16).wrapping_add(8);
    let mid_z = chunk[1].wrapping_mul(16).wrapping_add(8);
    let dx = x - f64::from(mid_x);
    let dz = z - f64::from(mid_z);
    let remaining = f64::from(total.wrapping_sub(current));
    let radius = f64::from(thickness + 2.0_f32 + 16.0_f32);
    dx * dx + dz * dz - remaining * remaining <= radius * radius
}

/// Marks an ellipsoid with the caller's carver-specific skip predicate.
/// The mask contains no block changes; applying it requires the aquifer and
/// the active block registry.
pub fn carve_ellipsoid(
    mask: &mut CarvingMask,
    chunk: [i32; 2],
    center: [f64; 3],
    horizontal: f64,
    vertical: f64,
    mut skip: impl FnMut(f64, f64, f64, i32) -> bool,
) -> Result<()> {
    ensure!(
        center.iter().all(|v| v.is_finite()) && horizontal.is_finite() && vertical.is_finite(),
        "invalid carver ellipsoid"
    );
    let origin_x = chunk[0]
        .checked_mul(16)
        .context("carver chunk X overflow")?;
    let origin_z = chunk[1]
        .checked_mul(16)
        .context("carver chunk Z overflow")?;
    let [x, y, z] = center;
    let max_delta = 16.0 + horizontal * 2.0;
    if (x - f64::from(origin_x + 8)).abs() > max_delta
        || (z - f64::from(origin_z + 8)).abs() > max_delta
    {
        return Ok(());
    }
    let min_x = floor_java(x - horizontal)
        .wrapping_sub(origin_x)
        .wrapping_sub(1)
        .max(0);
    let max_x = floor_java(x + horizontal).wrapping_sub(origin_x).min(15);
    let min_z = floor_java(z - horizontal)
        .wrapping_sub(origin_z)
        .wrapping_sub(1)
        .max(0);
    let max_z = floor_java(z + horizontal).wrapping_sub(origin_z).min(15);
    let min_y = floor_java(y - vertical).wrapping_sub(1).max(mask.min_y);
    let max_y = floor_java(y + vertical).wrapping_add(1).min(mask.max_y);
    if min_x > max_x || min_z > max_z || min_y >= max_y {
        return Ok(());
    }
    for lx in min_x..=max_x {
        let xd = (f64::from(origin_x + lx) + 0.5 - x) / horizontal;
        for lz in min_z..=max_z {
            let zd = (f64::from(origin_z + lz) + 0.5 - z) / horizontal;
            if xd * xd + zd * zd >= 1.0 {
                continue;
            }
            for by in (min_y + 1..=max_y).rev() {
                let yd = (f64::from(by) - 0.5 - y) / vertical;
                if !skip(xd, yd, zd, by) {
                    mask.carve(lx as u8, by, lz as u8);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mask_preserves_column_segments_and_validates_bounds() {
        let mut mask = CarvingMask::new(-2, 3).unwrap();
        for (x, y, z) in [(1, -1, 2), (1, 0, 2), (1, 2, 2), (0, 3, 15)] {
            mask.carve(x, y, z);
        }
        assert_eq!(
            mask.segments(),
            vec![(0, 15, 3, 3), (1, 2, -1, 0), (1, 2, 2, 2)]
        );
        assert!(!mask.contains(16, 0, 2));
        assert!(CarvingMask::new(4, 3).is_err());
    }
}
