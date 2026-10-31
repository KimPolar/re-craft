//! Two-dimensional simplex noise used by Java 26.3 biome colors.

use crate::random::LegacyRandom;

/// Java's simplex permutation and coordinate offsets, initialized from its
/// legacy random source. Even zero offsets consume three random doubles.
pub struct Simplex2D {
    permutation: [u8; 256],
    offset_x: f64,
    offset_y: f64,
}

impl Simplex2D {
    pub fn new(seed: i64, zero_offsets: bool) -> Self {
        let mut random = LegacyRandom::new(seed);
        Self::from_random(&mut random, zero_offsets)
    }

    /// Construct consecutive noise layers from the same Java random source.
    pub fn from_random(random: &mut LegacyRandom, zero_offsets: bool) -> Self {
        let scale = if zero_offsets { 0.0 } else { 256.0 };
        let offset_x = random.next_double() * scale;
        let offset_y = random.next_double() * scale;
        let _offset_z = random.next_double() * scale;
        let mut permutation = std::array::from_fn(|index| index as u8);
        for i in 0..256 {
            let other = i + random.next_int_bound((256 - i) as i32).unwrap() as usize;
            permutation.swap(i, other);
        }
        Self {
            permutation,
            offset_x,
            offset_y,
        }
    }

    fn permute(&self, value: i32) -> i32 {
        i32::from(self.permutation[(value & 255) as usize])
    }

    /// Java computes the simplex geometry in doubles and returns a float.
    pub fn sample(&self, x: f64, y: f64) -> f32 {
        const GRADIENTS: [(f64, f64); 12] = [
            (1.0, 1.0),
            (-1.0, 1.0),
            (1.0, -1.0),
            (-1.0, -1.0),
            (1.0, 0.0),
            (-1.0, 0.0),
            (1.0, 0.0),
            (-1.0, 0.0),
            (0.0, 1.0),
            (0.0, -1.0),
            (0.0, 1.0),
            (0.0, -1.0),
        ];
        let sqrt3 = 3.0_f64.sqrt();
        let f2 = 0.5 * (sqrt3 - 1.0);
        let g2 = (3.0 - sqrt3) / 6.0;
        let x = x + self.offset_x;
        let y = y + self.offset_y;
        let skew = (x + y) * f2;
        let i = (x + skew).floor() as i32;
        let j = (y + skew).floor() as i32;
        let unskew = f64::from(i.wrapping_add(j)) * g2;
        let x0 = x - (f64::from(i) - unskew);
        let y0 = y - (f64::from(j) - unskew);
        let (i1, j1) = if x0 > y0 { (1, 0) } else { (0, 1) };
        let x1 = x0 - f64::from(i1) + g2;
        let y1 = y0 - f64::from(j1) + g2;
        let x2 = x0 - 1.0 + 2.0 * g2;
        let y2 = y0 - 1.0 + 2.0 * g2;
        let ii = i & 255;
        let jj = j & 255;
        let grad = |i, j| (self.permute(i + self.permute(j)) % 12) as usize;
        let corner = |gradient: usize, x: f64, y: f64| {
            let mut t = 0.5 - x * x - y * y - 0.0;
            if t < 0.0 {
                return 0.0;
            }
            t *= t;
            let (gx, gy) = GRADIENTS[gradient];
            t * t * (gx * x + gy * y + 0.0)
        };
        (70.0
            * (corner(grad(ii, jj), x0, y0)
                + corner(grad(ii + i1, jj + j1), x1, y1)
                + corner(grad(ii + 1, jj + 1), x2, y2))) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::Simplex2D;

    #[test]
    fn legacy_swamp_noise_matches_java_reference_bits() {
        let noise = Simplex2D::new(2345, true);
        for (x, z, bits) in [
            (-128, -8, -1088921235_i32),
            (-127, -7, -1089186037_i32),
            (-126, -6, -1089734021_i32),
        ] {
            assert_eq!(
                noise
                    .sample(f64::from(x) * 0.0225, f64::from(z) * 0.0225)
                    .to_bits(),
                bits as u32
            );
        }
        assert_eq!(noise.sample(0.0, 0.0), 0.0);
    }
}
