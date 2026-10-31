//! Shared Java 26.3 weather timers and per-level rain and thunder intensity.

use native_core::random::LegacyRandom;
use native_persistence::heightmap::SpawnHeightmaps;
use native_persistence::world::SavedWeather;
use native_resources::biome_climate::BiomeClimate;
use native_world::loaded::ChunkPos;
use std::{collections::BTreeMap, sync::Arc};

/// Immutable weather inputs; each heightmap is shared independently across ticks.
#[derive(Clone)]
pub struct WeatherTerrain {
    pub heightmaps: Arc<BTreeMap<ChunkPos, Arc<SpawnHeightmaps>>>,
    pub climates: Arc<BTreeMap<String, BiomeClimate>>,
    pub sea_level: Option<i32>,
    pub min_y: i32,
    pub max_y: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Weather {
    pub data: SavedWeather,
    pub rain_level: f32,
    pub thunder_level: f32,
    previous_rain_level: f32,
    previous_thunder_level: f32,
}

/// Tick endpoints shared with the renderer; interpolation never mutates simulation.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WeatherVisual {
    pub previous_rain: f32,
    pub rain: f32,
    pub previous_thunder: f32,
    pub thunder: f32,
}

impl WeatherVisual {
    pub fn rain_at(self, partial: f32) -> f32 {
        self.previous_rain + partial.clamp(0.0, 1.0) * (self.rain - self.previous_rain)
    }

    /// Level.getThunderLevel interpolates thunder and multiplies it by rain.
    pub fn thunder_at(self, partial: f32) -> f32 {
        (self.previous_thunder + partial.clamp(0.0, 1.0) * (self.thunder - self.previous_thunder))
            * self.rain_at(partial)
    }
}

impl Weather {
    /// Mirrors ServerLevel.prepareWeather on initial load.
    pub fn new(data: SavedWeather) -> Self {
        let rain = if data.raining { 1.0 } else { 0.0 };
        let thunder = if data.raining && data.thundering {
            1.0
        } else {
            0.0
        };
        Self {
            rain_level: rain,
            thunder_level: thunder,
            previous_rain_level: rain,
            previous_thunder_level: thunder,
            data,
        }
    }

    pub fn visual(&self) -> WeatherVisual {
        WeatherVisual {
            previous_rain: self.previous_rain_level,
            rain: self.rain_level,
            previous_thunder: self.previous_thunder_level,
            thunder: self.thunder_level,
        }
    }

    pub fn tick(
        &mut self,
        can_have_weather: bool,
        advance_weather: bool,
        random: &mut LegacyRandom,
    ) {
        if !can_have_weather {
            return;
        }
        self.previous_rain_level = self.rain_level;
        self.previous_thunder_level = self.thunder_level;
        if advance_weather {
            let data = &mut self.data;
            if data.clear_weather_time > 0 {
                data.clear_weather_time -= 1;
                data.thunder_time = if data.thundering { 0 } else { 1 };
                data.rain_time = if data.raining { 0 } else { 1 };
                data.thundering = false;
                data.raining = false;
            } else {
                if data.thunder_time > 0 {
                    data.thunder_time -= 1;
                    if data.thunder_time == 0 {
                        data.thundering = !data.thundering;
                    }
                } else {
                    let (min, max) = if data.thundering {
                        (3600, 15600)
                    } else {
                        (12000, 180000)
                    };
                    data.thunder_time = min
                        + random
                            .next_int_bound(max - min + 1)
                            .expect("valid weather range");
                }
                if data.rain_time > 0 {
                    data.rain_time -= 1;
                    if data.rain_time == 0 {
                        data.raining = !data.raining;
                    }
                } else {
                    let (min, max) = if data.raining {
                        (12000, 24000)
                    } else {
                        (12000, 180000)
                    };
                    data.rain_time = min
                        + random
                            .next_int_bound(max - min + 1)
                            .expect("valid weather range");
                }
            }
        }
        self.thunder_level = (self.thunder_level
            + if self.data.thundering {
                0.01_f32
            } else {
                -0.01_f32
            })
        .clamp(0.0, 1.0);
        self.rain_level = (self.rain_level
            + if self.data.raining {
                0.01_f32
            } else {
                -0.01_f32
            })
        .clamp(0.0, 1.0);
    }

    pub fn is_raining(&self, can_have_weather: bool) -> bool {
        can_have_weather && f64::from(self.rain_level) > 0.2
    }

    pub fn is_thundering(&self, can_have_weather: bool) -> bool {
        can_have_weather && f64::from(self.thunder_level * self.rain_level) > 0.9
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn weather_freeze_keeps_timers_but_updates_intensity() {
        let data = SavedWeather {
            rain_time: 1,
            ..SavedWeather::default()
        };
        let mut weather = Weather::new(data);
        let mut random = LegacyRandom::new(1);
        weather.data.raining = true;
        weather.tick(true, false, &mut random);
        assert_eq!(weather.data.rain_time, 1);
        assert_eq!(weather.rain_level, 0.01);
        weather.tick(false, true, &mut random);
        assert_eq!(weather.rain_level, 0.01);
        assert_eq!(weather.data.rain_time, 1);
    }

    #[test]
    fn visual_endpoints_interpolate_rain_and_thunder_independently() {
        let mut weather = Weather::new(SavedWeather::default());
        weather.data.raining = true;
        weather.data.thundering = true;
        let mut random = LegacyRandom::new(1);
        weather.tick(true, false, &mut random);
        let visual = weather.visual();
        assert_eq!(visual.rain_at(0.0), 0.0);
        assert_eq!(visual.rain_at(0.5), 0.005);
        assert_eq!(visual.rain_at(1.0), 0.01);
        assert_eq!(visual.thunder_at(0.5), 0.005_f32 * 0.005_f32);
        weather.tick(true, false, &mut random);
        assert_eq!(weather.visual().previous_rain, 0.01);
        assert_eq!(visual.rain, 0.01, "published snapshot stays unchanged");
    }
}
