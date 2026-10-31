//! Shared Java 26.3 weather timers and per-level rain and thunder intensity.

use native_core::random::LegacyRandom;
use native_persistence::world::SavedWeather;

#[derive(Clone, Debug, PartialEq)]
pub struct Weather {
    pub data: SavedWeather,
    pub rain_level: f32,
    pub thunder_level: f32,
}

impl Weather {
    /// Mirrors ServerLevel.prepareWeather on initial load.
    pub fn new(data: SavedWeather) -> Self {
        Self {
            rain_level: if data.raining { 1.0 } else { 0.0 },
            thunder_level: if data.raining && data.thundering {
                1.0
            } else {
                0.0
            },
            data,
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
}
