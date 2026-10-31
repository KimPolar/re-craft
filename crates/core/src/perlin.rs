//! Single-octave gradient noise used by Java 26.3 world generation.
//!
//! Coordinates and random offsets are doubles; local gradient calculations
//! and interpolation use floats, matching 26.3's sampler.
use crate::random::{LegacyRandom, XoroshiroRandom};

/// Random operations consumed while constructing a Perlin octave.
pub trait NoiseRandom {
    fn noise_double(&mut self) -> f64;
    fn noise_bounded_int(&mut self, bound: i32) -> i32;
}

impl NoiseRandom for LegacyRandom {
    fn noise_double(&mut self) -> f64 {
        self.next_double()
    }
    fn noise_bounded_int(&mut self, bound: i32) -> i32 {
        self.next_int_bound(bound).unwrap()
    }
}
impl NoiseRandom for XoroshiroRandom {
    fn noise_double(&mut self) -> f64 {
        self.next_double()
    }
    fn noise_bounded_int(&mut self, bound: i32) -> i32 {
        self.next_int_bound(bound).unwrap()
    }
}

#[derive(Clone, Debug)]
pub struct PerlinNoise {
    permutation: [u8; 256],
    offsets: [f64; 3],
}

/// Ordered 26.3 Perlin layers. A complete `NormalNoise` also chooses the
/// octave seeds, frequencies and normalization; callers must supply them.
#[derive(Clone, Debug, Default)]
pub struct PerlinStack {
    layers: Vec<(PerlinNoise, f64, f32)>,
}

impl PerlinStack {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn push(&mut self, noise: PerlinNoise, frequency: f64, amplitude: f32) {
        self.layers.push((noise, frequency, amplitude));
    }
    pub fn append_scaled(&mut self, stack: PerlinStack, frequency: f64, amplitude: f32) {
        self.layers.extend(stack.layers.into_iter().map(
            |(noise, layer_frequency, layer_amplitude)| {
                (
                    noise,
                    layer_frequency * frequency,
                    layer_amplitude * amplitude,
                )
            },
        ));
    }
    pub fn sample(&self, x: f64, y: f64, z: f64) -> f32 {
        let mut total = 0.0f32;
        for (noise, frequency, amplitude) in &self.layers {
            total += *amplitude * noise.sample(x * frequency, y * frequency, z * frequency);
        }
        total
    }
    pub fn sample_2d(&self, x: f64, z: f64) -> f32 {
        let mut total = 0.0f32;
        for (noise, frequency, amplitude) in &self.layers {
            total += *amplitude * noise.sample_2d(x * frequency, z * frequency);
        }
        total
    }

    /// Matches `NoiseStack.addToVolume`: each layer combines its frequency
    /// with the coordinate scale before multiplying by the block coordinate.
    pub fn sample_scaled_volume(&self, x: i32, y: i32, z: i32, xz_scale: f64, y_scale: f64) -> f32 {
        self.sample_scaled_volume_weighted(x, y, z, xz_scale, y_scale, 1.0)
    }

    pub fn sample_scaled_volume_weighted(
        &self,
        x: i32,
        y: i32,
        z: i32,
        xz_scale: f64,
        y_scale: f64,
        weight: f32,
    ) -> f32 {
        let mut total = 0.0f32;
        for (noise, frequency, amplitude) in &self.layers {
            let xz = xz_scale * frequency;
            let y_scale = y_scale * frequency;
            total += (weight * *amplitude)
                * noise.sample(f64::from(x) * xz, f64::from(y) * y_scale, f64::from(z) * xz);
        }
        total
    }
}

impl PerlinNoise {
    /// Consumes exactly three doubles and 256 bounded draws from the source.
    pub fn new(random: &mut impl NoiseRandom) -> Self {
        let offsets = [
            random.noise_double() * 256.0,
            random.noise_double() * 256.0,
            random.noise_double() * 256.0,
        ];
        let mut permutation = std::array::from_fn(|index| index as u8);
        for index in 0..256 {
            let other = index + random.noise_bounded_int((256 - index) as i32) as usize;
            permutation.swap(index, other);
        }
        Self {
            permutation,
            offsets,
        }
    }

    fn permute(&self, value: i32) -> i32 {
        i32::from(self.permutation[(value & 255) as usize])
    }

    /// Samples Java's two-dimensional entry point (the Y coordinate is zero).
    pub fn sample_2d(&self, x: f64, z: f64) -> f32 {
        self.sample(wrap(x), 0.0, wrap(z))
    }

    /// Samples the underlying octave; this is not yet the complete normal noise stack.
    pub fn sample(&self, x: f64, y: f64, z: f64) -> f32 {
        let x = wrap(x) + self.offsets[0];
        let y = wrap(y) + self.offsets[1];
        let z = wrap(z) + self.offsets[2];
        let ix = java_floor(x);
        let iy = java_floor(y);
        let iz = java_floor(z);
        let fx = (x - f64::from(ix)) as f32;
        let fy = (y - f64::from(iy)) as f32;
        let fz = (z - f64::from(iz)) as f32;
        let px0 = self.permute(ix);
        let px1 = self.permute(ix.wrapping_add(1));
        let p00 = self.permute(px0.wrapping_add(iy));
        let p01 = self.permute(px0.wrapping_add(iy).wrapping_add(1));
        let p10 = self.permute(px1.wrapping_add(iy));
        let p11 = self.permute(px1.wrapping_add(iy).wrapping_add(1));
        let grad = |p: i32, dz: i32, x: f32, y: f32, z: f32| {
            gradient(self.permute(p.wrapping_add(iz).wrapping_add(dz)), x, y, z)
        };
        let x0 = fx;
        let x1 = fx - 1.0;
        let y0 = fy;
        let y1 = fy - 1.0;
        let z0 = fz;
        let z1 = fz - 1.0;
        let (sx, sy, sz) = (smoothstep(fx), smoothstep(fy), smoothstep(fz));
        let plane0 = lerp(
            sy,
            lerp(sx, grad(p00, 0, x0, y0, z0), grad(p10, 0, x1, y0, z0)),
            lerp(sx, grad(p01, 0, x0, y1, z0), grad(p11, 0, x1, y1, z0)),
        );
        let plane1 = lerp(
            sy,
            lerp(sx, grad(p00, 1, x0, y0, z1), grad(p10, 1, x1, y0, z1)),
            lerp(sx, grad(p01, 1, x0, y1, z1), grad(p11, 1, x1, y1, z1)),
        );
        lerp(sz, plane0, plane1)
    }
}

fn java_floor(value: f64) -> i32 {
    let truncated = value as i32;
    if value < f64::from(truncated) {
        truncated.wrapping_sub(1)
    } else {
        truncated
    }
}

fn wrap(value: f64) -> f64 {
    const HALF: f64 = f64::from_bits(16_777_216.0f64.to_bits() - 1);
    const PERIOD: f64 = 33_554_432.0;
    if value >= -HALF && value < HALF {
        value
    } else {
        value - (value / PERIOD + 0.5).floor() * PERIOD
    }
}

fn smoothstep(value: f32) -> f32 {
    value * value * value * (value * (value * 6.0 - 15.0) + 10.0)
}
fn lerp(alpha: f32, first: f32, second: f32) -> f32 {
    first + alpha * (second - first)
}

fn gradient(hash: i32, x: f32, y: f32, z: f32) -> f32 {
    const GRADIENTS: [(f32, f32, f32); 16] = [
        (1.0, 1.0, 0.0),
        (-1.0, 1.0, 0.0),
        (1.0, -1.0, 0.0),
        (-1.0, -1.0, 0.0),
        (1.0, 0.0, 1.0),
        (-1.0, 0.0, 1.0),
        (1.0, 0.0, -1.0),
        (-1.0, 0.0, -1.0),
        (0.0, 1.0, 1.0),
        (0.0, -1.0, 1.0),
        (0.0, 1.0, -1.0),
        (0.0, -1.0, -1.0),
        (1.0, 1.0, 0.0),
        (0.0, -1.0, 1.0),
        (-1.0, 1.0, 0.0),
        (0.0, -1.0, -1.0),
    ];
    let (gx, gy, gz) = GRADIENTS[(hash & 15) as usize];
    gx * x + gy * y + gz * z
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perlin_and_stack_match_java_reference_at_origin() {
        let noise = PerlinNoise::new(&mut LegacyRandom::new(0));
        assert_eq!(
            noise.sample(0.0, 0.0, -0.0).to_bits(),
            -1111233714i32 as u32
        );
        assert_eq!(noise.sample_2d(0.0, -0.0).to_bits(), -1111233714i32 as u32);
        let mut random = LegacyRandom::new(0);
        let mut stack = PerlinStack::new();
        for (frequency, amplitude) in [
            (1.0, 1.0),
            (0.25, -0.2),
            (1.0181268882175227, 0.37),
            (2.0, 0.0),
        ] {
            stack.push(PerlinNoise::new(&mut random), frequency, amplitude);
        }
        assert_eq!(stack.sample(0.0, 0.0, -0.0).to_bits(), 1027044604);
        assert_eq!(stack.sample_2d(0.0, -0.0).to_bits(), 1027044604);
    }
}
