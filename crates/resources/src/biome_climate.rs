//! The 26.3 biome climate rules used to distinguish rain and snow.

use std::{collections::BTreeMap, sync::OnceLock};

use anyhow::{Context, Result, bail, ensure};
use native_core::{random::LegacyRandom, simplex::Simplex2D};
use serde_json::Value;

use crate::packs::{ResourceId, ResourceProvider};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Precipitation {
    None,
    Rain,
    Snow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TemperatureModifier {
    None,
    Frozen,
}

#[derive(Clone, Copy, Debug)]
pub struct BiomeClimate {
    pub has_precipitation: bool,
    pub temperature: f32,
    pub modifier: TemperatureModifier,
}

/// Invalid individual entries remain unavailable instead of receiving a guess.
pub fn load_world_climates(
    source: &mut impl ResourceProvider,
) -> Result<BTreeMap<String, BiomeClimate>> {
    let mut result = BTreeMap::new();
    for id in source.list_data("worldgen/biome")? {
        let name = format!("{}:{}", id.namespace, id.path);
        if let Ok(Some(climate)) = BiomeClimate::load_world(source, &name) {
            result.insert(name, climate);
        }
    }
    Ok(result)
}

impl BiomeClimate {
    /// Resolve from the world's active data packs, including overrides.
    pub fn load_world(source: &mut impl ResourceProvider, id: &str) -> Result<Option<Self>> {
        let id = ResourceId::parse(id)?;
        let path = format!("data/{}/worldgen/biome/{}.json", id.namespace, id.path);
        source
            .read_data(&path)?
            .map(|data| Self::from_json(&data))
            .transpose()
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        let value: Value = serde_json::from_slice(bytes).context("invalid biome JSON")?;
        let has_precipitation = value
            .get("has_precipitation")
            .and_then(Value::as_bool)
            .context("missing or invalid has_precipitation")?;
        let temperature = value
            .get("temperature")
            .and_then(Value::as_f64)
            .context("missing or invalid temperature")? as f32;
        ensure!(temperature.is_finite(), "invalid biome temperature");
        let modifier = match value.get("temperature_modifier") {
            None => TemperatureModifier::None,
            Some(Value::String(name)) if name == "none" => TemperatureModifier::None,
            Some(Value::String(name)) if name == "frozen" => TemperatureModifier::Frozen,
            _ => bail!("invalid or unsupported temperature_modifier"),
        };
        Ok(Self {
            has_precipitation,
            temperature,
            modifier,
        })
    }

    /// The exact float evaluation order of Biome.getHeightAdjustedTemperature.
    pub fn temperature_at(self, x: i32, y: i32, z: i32, sea_level: i32) -> f32 {
        let noise = climate_noise();
        let mut temperature = self.temperature;
        if self.modifier == TemperatureModifier::Frozen {
            let x = f64::from(x);
            let z = f64::from(z);
            let frozen = f64::from(noise.frozen(x * 0.05, z * 0.05) * 7.0);
            let info = f64::from(noise.info.sample(x * 0.2, z * 0.2));
            if frozen + info < 0.3 && f64::from(noise.info.sample(x * 0.09, z * 0.09)) < 0.8 {
                temperature = 0.2;
            }
        }
        let threshold = sea_level.wrapping_add(17);
        if y > threshold {
            let x = f64::from(x as f32 / 8.0);
            let z = f64::from(z as f32 / 8.0);
            let noise = noise.temperature.sample(x, z) * 8.0;
            temperature -= (noise + y as f32 - threshold as f32) * 0.05 / 40.0;
        }
        temperature
    }

    /// Surface rules use this even in biomes without precipitation.
    pub fn cold_enough_to_snow(self, x: i32, y: i32, z: i32, sea_level: i32) -> bool {
        self.temperature_at(x, y, z, sea_level) < 0.15
    }

    pub fn precipitation_at(self, x: i32, y: i32, z: i32, sea_level: i32) -> Precipitation {
        if !self.has_precipitation {
            Precipitation::None
        } else if self.cold_enough_to_snow(x, y, z, sea_level) {
            Precipitation::Snow
        } else {
            Precipitation::Rain
        }
    }
}

struct ClimateNoise {
    temperature: Simplex2D,
    info: Simplex2D,
    frozen: [Simplex2D; 3],
}

impl ClimateNoise {
    fn frozen(&self, x: f64, z: f64) -> f32 {
        let mut result = 0.0_f32;
        for (layer, (frequency, amplitude)) in self.frozen.iter().zip([
            (1.0, 0.14285715_f32),
            (0.5, 0.2857143_f32),
            (0.25, 0.5714286_f32),
        ]) {
            result += amplitude * layer.sample(x * frequency, z * frequency);
        }
        result
    }
}

fn climate_noise() -> &'static ClimateNoise {
    static NOISE: OnceLock<ClimateNoise> = OnceLock::new();
    NOISE.get_or_init(|| {
        let mut random = LegacyRandom::new(3456);
        ClimateNoise {
            temperature: Simplex2D::new(1234, true),
            info: Simplex2D::new(2345, true),
            frozen: std::array::from_fn(|_| Simplex2D::from_random(&mut random, true)),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_climate_conservatively() {
        let biome =
            BiomeClimate::from_json(br#"{"has_precipitation":true,"temperature":0.15}"#).unwrap();
        assert_eq!(biome.precipitation_at(0, 63, 0, 63), Precipitation::Rain);
        assert!(BiomeClimate::from_json(br#"{"temperature":0.5}"#).is_err());
        assert!(
            BiomeClimate::from_json(
                br#"{"has_precipitation":true,"temperature":0.5,"temperature_modifier":"future"}"#
            )
            .is_err()
        );
    }
}
