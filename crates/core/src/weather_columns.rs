//! Animated 26.3 precipitation-column offsets and geometry.
//!
//! These functions do not select columns or access world state. Callers must
//! supply the verified biome, heightmap and light data for each column.

use crate::random::LegacyRandom;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColumnAnimation {
    pub u: f32,
    pub v: f32,
    pub block_light: u8,
    pub sky_light: u8,
}

fn coordinate_seed(x: i32, z: i32) -> i32 {
    let x = x
        .wrapping_mul(x)
        .wrapping_mul(3121)
        .wrapping_add(x.wrapping_mul(45_238_971));
    let z = z
        .wrapping_mul(z)
        .wrapping_mul(418_711)
        .wrapping_add(z.wrapping_mul(13_761));
    x ^ z
}

fn gaussian_pair(random: &mut LegacyRandom) -> (f64, f64) {
    loop {
        let a = 2.0 * random.next_double() - 1.0;
        let b = 2.0 * random.next_double() - 1.0;
        let r = a * a + b * b;
        if r < 1.0 && r != 0.0 {
            let scale = (-2.0 * r.ln() / r).sqrt();
            return (a * scale, b * scale);
        }
    }
}

/// Create the animated texture offsets for a rain column.
pub fn rain(x: i32, z: i32, game_time: i64, partial: f32, light: [u8; 2]) -> ColumnAnimation {
    let mut random = LegacyRandom::new(i64::from(coordinate_seed(x, z)));
    let hash = x
        .wrapping_mul(x)
        .wrapping_mul(3121)
        .wrapping_add(x.wrapping_mul(45_238_971))
        .wrapping_add(z.wrapping_mul(z).wrapping_mul(418_711))
        .wrapping_add(z.wrapping_mul(13_761))
        & 255;
    let time = (game_time & 131_071) as i32;
    let speed = 3.0_f32 + random.next_float();
    let v = (-((time + hash) as f32 + partial) / 32.0 * speed) % 32.0;
    ColumnAnimation {
        u: 0.0,
        v,
        block_light: light[0],
        sky_light: light[1],
    }
}

/// Create the animated texture offsets and brightened light for snow.
pub fn snow(x: i32, z: i32, game_time: i64, partial: f32, light: [u8; 2]) -> ColumnAnimation {
    let mut random = LegacyRandom::new(i64::from(coordinate_seed(x, z)));
    let time = (game_time & 131_071) as f32 + partial;
    let first = random.next_double();
    let (a, b) = gaussian_pair(&mut random);
    let u = (first + f64::from(time * 0.01 * a as f32)) as f32;
    let second = random.next_double();
    let drift = (second + f64::from(time * b as f32 * 0.001)) as f32;
    let v = -((game_time & 511) as f32 + partial) / 512.0 + drift;
    ColumnAnimation {
        u,
        v,
        block_light: (light[0].min(15) * 3 + 15) / 4,
        sky_light: (light[1].min(15) * 3 + 15) / 4,
    }
}

/// Java's square of columns, clamped above the motion-blocking heightmap.
pub fn vertical_range(camera_y: i32, height: i32, radius: i32) -> Option<(i32, i32)> {
    if !(3..=10).contains(&radius) {
        return None;
    }
    let bottom = camera_y.checked_sub(radius)?.max(height);
    let top = camera_y.checked_add(radius)?.max(height);
    (top > bottom).then_some((bottom, top))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn range_stops_at_roofs_and_snow_brightens_both_channels() {
        assert_eq!(vertical_range(70, 64, 5), Some((65, 75)));
        assert_eq!(vertical_range(70, 71, 5), Some((71, 75)));
        assert_eq!(vertical_range(70, 75, 5), None);
        assert_eq!(vertical_range(70, 64, 0), None);
        let snow = snow(-9, 12, 123, 0.5, [0, 15]);
        assert_eq!([snow.block_light, snow.sky_light], [3, 15]);
        assert_eq!(rain(-9, 12, 123, 0.5, [0, 15]).u, 0.0);
    }
}
