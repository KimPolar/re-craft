//! Seeded 26.3 terrain noise, including height-smeared Perlin octaves.
use crate::perlin::{NoiseRandom, PerlinNoise};

#[derive(Debug)]
struct Layer {
    noise: PerlinNoise,
    frequency: f64,
    amplitude: f32,
    smear: f64,
}

#[derive(Debug)]
pub struct BlendedNoise {
    minimum: Vec<Layer>,
    maximum: Vec<Layer>,
    main: Vec<Layer>,
    xz_scale: f64,
    y_scale: f64,
    main_xz_scale: f64,
    main_y_scale: f64,
    bound: f32,
}

impl BlendedNoise {
    pub fn new(
        random: &mut impl NoiseRandom,
        xz_scale: f64,
        y_scale: f64,
        xz_factor: f64,
        y_factor: f64,
        smear_multiplier: f64,
    ) -> Self {
        let xz_scale = 684.412 * xz_scale;
        let y_scale = 684.412 * y_scale;
        let smear = y_scale * smear_multiplier;
        let make_stack = |random: &mut _, count: usize, smear: f64, factor: f64| {
            let mut frequency = 1.0;
            let mut amplitude = factor / (2.0_f64.powi(count as i32) - 1.0);
            let mut layers = Vec::with_capacity(count);
            for _ in 0..count {
                layers.push(Layer {
                    noise: PerlinNoise::new(random),
                    frequency,
                    amplitude: amplitude as f32,
                    smear: smear * frequency,
                });
                frequency /= 2.0;
                amplitude *= 2.0;
            }
            layers
        };
        let minimum = make_stack(random, 16, smear, 0.9999847412109375);
        let maximum = make_stack(random, 16, smear, 0.9999847412109375);
        let main = make_stack(random, 8, smear / y_factor, 12.75);
        let mut bound = 0.0_f32;
        for layer in &minimum {
            bound += ((layer.smear.abs() + 2.0) as f32) * layer.amplitude;
        }
        Self {
            minimum,
            maximum,
            main,
            xz_scale,
            y_scale,
            main_xz_scale: xz_scale / xz_factor,
            main_y_scale: y_scale / y_factor,
            bound,
        }
    }

    pub fn range(&self) -> (f32, f32) {
        (-self.bound, self.bound)
    }

    pub fn sample(&self, x: i32, y: i32, z: i32, volume: bool) -> f32 {
        let stack = |layers: &[Layer], xz_scale: f64, y_scale: f64| {
            let mut sum = 0.0_f32;
            for layer in layers {
                let (x, y, z) = if volume {
                    (
                        f64::from(x) * (xz_scale * layer.frequency),
                        f64::from(y) * (y_scale * layer.frequency),
                        f64::from(z) * (xz_scale * layer.frequency),
                    )
                } else {
                    (
                        (f64::from(x) * xz_scale) * layer.frequency,
                        (f64::from(y) * y_scale) * layer.frequency,
                        (f64::from(z) * xz_scale) * layer.frequency,
                    )
                };
                sum += layer.amplitude * layer.noise.sample_smeared(x, y, z, layer.smear, volume);
            }
            sum
        };
        let alpha =
            (stack(&self.main, self.main_xz_scale, self.main_y_scale) + 0.5).clamp(0.0, 1.0);
        if alpha == 0.0 {
            return stack(&self.minimum, self.xz_scale, self.y_scale);
        }
        if alpha == 1.0 {
            return stack(&self.maximum, self.xz_scale, self.y_scale);
        }
        let first = stack(&self.minimum, self.xz_scale, self.y_scale);
        let second = stack(&self.maximum, self.xz_scale, self.y_scale);
        first + alpha * (second - first)
    }
}
