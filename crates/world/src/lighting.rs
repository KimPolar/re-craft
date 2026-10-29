//! CPU representation of the 26.3 light map. The two light channels remain separate.

#[derive(Clone, Debug, PartialEq)]
pub struct LightmapSettings {
    pub sky_factor: f32,
    pub block_factor: f32,
    pub night_vision_factor: f32,
    pub darkness_scale: f32,
    pub boss_darkening: f32,
    pub brightness: f32,
    pub block_tint: [f32; 3],
    pub sky_color: [f32; 3],
    pub ambient_color: [f32; 3],
    pub night_vision_color: [f32; 3],
}

impl Default for LightmapSettings {
    fn default() -> Self {
        Self {
            sky_factor: 1.0,
            block_factor: 1.4,
            night_vision_factor: 0.0,
            darkness_scale: 0.0,
            boss_darkening: 0.0,
            brightness: 0.0,
            block_tint: [1.0, 216.0 / 255.0, 140.0 / 255.0],
            sky_color: [1.0; 3],
            ambient_color: [10.0 / 255.0; 3],
            night_vision_color: [0.6; 3],
        }
    }
}

impl LightmapSettings {
    /// Build a lookup once when lighting inputs change, indexed by sky then block level.
    pub fn table(&self) -> [[[f32; 3]; 16]; 16] {
        std::array::from_fn(|sky| std::array::from_fn(|block| self.sample(block as u8, sky as u8)))
    }

    /// Calculates the light map's RGB entry for integer light levels from 0 to 15.
    pub fn sample(&self, block: u8, sky: u8) -> [f32; 3] {
        let level = |n: u8| f32::from(n.min(15)) / 15.0;
        let brightness = |n: f32| n / (4.0 - 3.0 * n);
        let block_level = level(block);
        let sky_level = level(sky);
        let block_strength = brightness(block_level) * self.block_factor;
        let sky_strength = brightness(sky_level) * self.sky_factor;
        let whiteness = 0.9 * (2.0 * block_level - 1.0).powi(2);
        let mut color = std::array::from_fn(|index| {
            let ambient = self.ambient_color[index]
                .max(self.night_vision_color[index] * self.night_vision_factor);
            let block_color = self.block_tint[index] * (1.0 - whiteness) + whiteness;
            let shade = if index == 0 { 0.7 } else { 0.6 };
            let combined =
                ambient + self.sky_color[index] * sky_strength + block_color * block_strength;
            (combined * (1.0 - self.boss_darkening + shade * self.boss_darkening)
                - self.darkness_scale)
                .clamp(0.0, 1.0)
        });
        let max = color.into_iter().fold(0.0f32, f32::max);
        if max > 0.0 {
            let inverted = 1.0 - max;
            let gamma_scale = (1.0 - inverted.powi(4)) / max;
            for value in &mut color {
                *value *= 1.0 - self.brightness + self.brightness * gamma_scale;
            }
        }
        color
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_and_sky_are_distinct_and_black_stays_finite() {
        let settings = LightmapSettings::default();
        let torch = settings.sample(8, 0);
        let sky = settings.sample(0, 8);
        assert!(torch[0] > torch[1] && torch[1] > torch[2]);
        assert_eq!(sky[0], sky[1]);
        assert_eq!(sky[1], sky[2]);
        let dark = LightmapSettings {
            ambient_color: [0.0; 3],
            brightness: 1.0,
            ..settings
        };
        assert_eq!(dark.sample(0, 0), [0.0; 3]);
    }
}
