//! Minecraft 26.3 normal noise parameters and seeded Perlin stacks.
use md5::{Digest, Md5};

use crate::{
    perlin::{NoiseRandom, PerlinNoise, PerlinStack},
    random::{LegacyRandom, XoroshiroRandom},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Normalization {
    Disabled,
    Enabled,
    Legacy,
}

#[derive(Clone, Debug)]
pub struct NormalNoiseParameters {
    pub base_amplitude: f64,
    pub base_octave: i32,
    pub octave_count: usize,
    pub normalization: Normalization,
    /// An empty list means 1.0 for every octave.
    pub amplitude_modifiers: Vec<f64>,
}

/// A random source able to fork a named positional factory, as Java does.
pub trait NoiseSeedSource: NoiseRandom {
    type Factory: NoiseSeedFactory<Source = Self>;
    fn fork_noise_factory(&mut self) -> Self::Factory;
}
pub trait NoiseSeedFactory {
    type Source: NoiseRandom;
    fn random_for(&self, name: &str) -> Self::Source;
}

pub struct LegacyNoiseFactory(i64);
impl NoiseSeedSource for LegacyRandom {
    type Factory = LegacyNoiseFactory;
    fn fork_noise_factory(&mut self) -> Self::Factory {
        LegacyNoiseFactory(self.next_long())
    }
}
impl NoiseSeedFactory for LegacyNoiseFactory {
    type Source = LegacyRandom;
    fn random_for(&self, name: &str) -> Self::Source {
        let hash = name.encode_utf16().fold(0i32, |h, unit| {
            h.wrapping_mul(31).wrapping_add(i32::from(unit))
        });
        LegacyRandom::new(i64::from(hash) ^ self.0)
    }
}

pub struct XoroshiroNoiseFactory(u64, u64);
impl NoiseSeedSource for XoroshiroRandom {
    type Factory = XoroshiroNoiseFactory;
    fn fork_noise_factory(&mut self) -> Self::Factory {
        XoroshiroNoiseFactory(self.next_long() as u64, self.next_long() as u64)
    }
}
impl NoiseSeedFactory for XoroshiroNoiseFactory {
    type Source = XoroshiroRandom;
    fn random_for(&self, name: &str) -> Self::Source {
        let digest = Md5::digest(name.as_bytes());
        let low = u64::from_be_bytes(digest[..8].try_into().unwrap());
        let high = u64::from_be_bytes(digest[8..].try_into().unwrap());
        XoroshiroRandom::from_state(low ^ self.0, high ^ self.1)
    }
}

#[derive(Clone, Debug)]
pub struct NormalNoise {
    stack: PerlinStack,
    range: (f32, f32),
}

impl NormalNoiseParameters {
    /// Validate the bounds accepted by the 26.3 data codec.
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.base_amplitude.is_finite()
            || !(f64::from(0.00001f32)..=1_000_000.0).contains(&self.base_amplitude)
        {
            return Err("invalid base amplitude");
        }
        if !(-32..=32).contains(&self.base_octave) {
            return Err("invalid base octave");
        }
        if !(1..=32).contains(&self.octave_count) {
            return Err("invalid octave count");
        }
        if !self.amplitude_modifiers.is_empty()
            && self.amplitude_modifiers.len() != self.octave_count
        {
            return Err("amplitude modifier count must match octave count");
        }
        if self
            .amplitude_modifiers
            .iter()
            .any(|value| !value.is_finite() || !(0.0..=1_000_000.0).contains(value))
        {
            return Err("invalid amplitude modifier");
        }
        Ok(())
    }

    /// Initializes the same named octave sources and advances the parent RNG.
    pub fn create(&self, random: &mut impl NoiseSeedSource) -> Result<NormalNoise, &'static str> {
        let (octaves, normalization, range) = self.prepare()?;
        let first = random.fork_noise_factory();
        let second = random.fork_noise_factory();
        let mut stack = PerlinStack::new();
        for (octave, frequency, amplitude) in octaves {
            let name = format!("octave_{octave}");
            let mut first_random = first.random_for(&name);
            let mut second_random = second.random_for(&name);
            let amplitude = (normalization * amplitude) as f32;
            stack.push(PerlinNoise::new(&mut first_random), frequency, amplitude);
            stack.push(
                PerlinNoise::new(&mut second_random),
                frequency * 1.0181268882175227,
                amplitude,
            );
        }
        Ok(NormalNoise { stack, range })
    }

    fn prepare(&self) -> Result<(Vec<(i32, f64, f64)>, f64, (f32, f32)), &'static str> {
        self.validate()?;
        let mut frequency = 2.0f64.powi(self.base_octave);
        let mut amplitude = self.base_amplitude;
        if self.normalization != Normalization::Disabled {
            let n = self.octave_count as i32;
            amplitude *= 0.5f64.powi(-(n - 1)) / (0.5f64.powi(-n) - 1.0);
        }
        let mut octaves = Vec::with_capacity(self.octave_count);
        for i in 0..self.octave_count {
            let modifier = self.amplitude_modifiers.get(i).copied().unwrap_or(1.0);
            if modifier != 0.0 {
                octaves.push((self.base_octave + i as i32, frequency, amplitude * modifier));
            }
            frequency *= 2.0;
            amplitude *= 0.5;
        }
        // DoubleStream.sum uses compensated summation rather than a simple sum.
        let mut sum = 0.0;
        let mut correction = 0.0;
        let mut variance = 0.0;
        for (_, _, amp) in &octaves {
            let value = amp.abs();
            let adjusted = value - correction;
            let next = sum + adjusted;
            correction = (next - sum) - adjusted;
            sum = next;
            let deviation = 0.2702247831245211 * value;
            variance += deviation * deviation;
        }
        let mut total = sum - correction;
        let deviation = variance.sqrt();
        let mut normalization = if deviation == 0.0 {
            0.0
        } else {
            (total * (1.0 / 3.0)) / (deviation * 2.0f64.sqrt())
        };
        if self.normalization == Normalization::Legacy && normalization != 0.0 {
            let first = self
                .amplitude_modifiers
                .iter()
                .position(|v| *v != 0.0)
                .unwrap_or(0);
            let last = self
                .amplitude_modifiers
                .iter()
                .rposition(|v| *v != 0.0)
                .unwrap_or(self.octave_count - 1);
            let expected = 0.1 * (1.0 + 1.0 / (last - first + 1) as f64);
            let legacy = self.base_amplitude * 0.5 * (1.0 / 3.0) / expected;
            total *= legacy / normalization;
            normalization = legacy;
        }
        let bound = (total * (1.0 / 3.0) * 6.0) as f32;
        Ok((octaves, normalization, (-bound, bound)))
    }

    /// The special sequential initializer used by Nether temperature and vegetation.
    pub fn create_legacy_nether(
        &self,
        random: &mut LegacyRandom,
    ) -> Result<NormalNoise, &'static str> {
        let (_, normalization, range) = self.prepare()?;
        let modifiers = if self.amplitude_modifiers.is_empty() {
            vec![1.0; self.octave_count]
        } else {
            self.amplitude_modifiers.clone()
        };
        let first = legacy_fbm(random, self.base_octave, &modifiers)?;
        let second = legacy_fbm(random, self.base_octave, &modifiers)?;
        let final_amplitude = (normalization * self.base_amplitude) as f32;
        let mut stack = PerlinStack::new();
        stack.append_scaled(first, 1.0, final_amplitude);
        stack.append_scaled(second, 1.0181268882175227, final_amplitude);
        Ok(NormalNoise { stack, range })
    }
}

fn legacy_fbm(
    random: &mut LegacyRandom,
    base_octave: i32,
    modifiers: &[f64],
) -> Result<PerlinStack, &'static str> {
    let count = modifiers.len();
    let zero_index = -base_octave;
    let mut layers = vec![None; count];
    let initial = PerlinNoise::new(random);
    if zero_index >= 0 && (zero_index as usize) < count && modifiers[zero_index as usize] != 0.0 {
        layers[zero_index as usize] = Some(initial);
    }
    for index in (0..zero_index).rev() {
        if (index as usize) < count && modifiers[index as usize] != 0.0 {
            layers[index as usize] = Some(PerlinNoise::new(random));
        } else {
            random.skip_ints(262);
        }
    }
    if layers.iter().filter(|layer| layer.is_some()).count()
        != modifiers.iter().filter(|value| **value != 0.0).count()
    {
        return Err("legacy octave initialization cannot construct the requested layers");
    }
    if zero_index < count as i32 - 1 {
        return Err("positive legacy octaves are unsupported by Java 26.3");
    }
    let mut frequency = 2.0f64.powi(-zero_index);
    let mut amplitude = 2.0f64.powi(count as i32 - 1) / (2.0f64.powi(count as i32) - 1.0);
    let mut stack = PerlinStack::new();
    for (layer, modifier) in layers.into_iter().zip(modifiers) {
        if let Some(noise) = layer {
            stack.push(noise, frequency, (amplitude * modifier) as f32);
        }
        frequency *= 2.0;
        amplitude /= 2.0;
    }
    Ok(stack)
}
impl NormalNoise {
    pub fn sample_scaled_volume(&self, x: i32, y: i32, z: i32, xz_scale: f64, y_scale: f64) -> f32 {
        self.stack.sample_scaled_volume(x, y, z, xz_scale, y_scale)
    }
    pub fn sample(&self, x: f64, y: f64, z: f64) -> f32 {
        self.stack.sample(x, y, z)
    }
    pub fn sample_2d(&self, x: f64, z: f64) -> f32 {
        self.stack.sample_2d(x, z)
    }
    pub fn range(&self) -> (f32, f32) {
        self.range
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_normalization_and_random_forks_match_java_reference() {
        let parameters = NormalNoiseParameters {
            base_amplitude: 1.0,
            base_octave: -4,
            octave_count: 4,
            normalization: Normalization::Legacy,
            amplitude_modifiers: vec![0.0, 1.0, 0.0, 2.0],
        };
        let mut random = LegacyRandom::new(0);
        let noise = parameters.create(&mut random).unwrap();
        assert_eq!(random.next_long(), -6688467811848818630);
        assert_eq!(
            (noise.range().0.to_bits(), noise.range().1.to_bits()),
            (-1084571084i32 as u32, 1062912564)
        );
        assert_eq!(noise.sample(0.0, 0.0, 0.0).to_bits(), 1033087916);
        assert_eq!(noise.sample(-1.5, 64.0, 2.75).to_bits(), 1015982720);
        assert_eq!(noise.sample_2d(-1.5, 2.75).to_bits(), 1044822740);
    }

    #[test]
    fn sequential_nether_octaves_match_java_reference() {
        let parameters = NormalNoiseParameters {
            base_amplitude: 0.9494731054427981,
            base_octave: -7,
            octave_count: 2,
            normalization: Normalization::Enabled,
            amplitude_modifiers: vec![],
        };
        let mut random = LegacyRandom::new(0);
        let noise = parameters.create_legacy_nether(&mut random).unwrap();
        assert_eq!(random.next_long(), 6916802886986171058);
        assert_eq!(noise.sample(0.0, 0.0, 0.0).to_bits(), 1039164812);
        assert_eq!(
            noise.sample(-1.5, 64.0, 2.75).to_bits(),
            -1119361692i32 as u32
        );
        assert_eq!(noise.sample_2d(-1.5, 2.75).to_bits(), 1038596892);
    }
}
