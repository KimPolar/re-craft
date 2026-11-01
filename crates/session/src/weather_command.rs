//! The Java 26.3 `/weather` command and its shared weather data updates.

use native_core::random::LegacyRandom;
use native_persistence::world::SavedWeather;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WeatherKind {
    Clear,
    Rain,
    Thunder,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WeatherCommand {
    pub kind: WeatherKind,
    /// None samples the source dimension's level random at execution time.
    pub duration: Option<i32>,
}

pub fn parse(input: &str) -> Option<WeatherCommand> {
    let parts: Vec<_> = input.split(' ').collect();
    let (kind, duration) = match parts.as_slice() {
        ["/weather", kind] => (*kind, None),
        ["/weather", kind, duration] => (*kind, Some(crate::time_command::duration(duration, 1)?)),
        _ => return None,
    };
    let kind = match kind {
        "clear" => WeatherKind::Clear,
        "rain" => WeatherKind::Rain,
        "thunder" => WeatherKind::Thunder,
        _ => return None,
    };
    Some(WeatherCommand { kind, duration })
}

/// Update shared WeatherData exactly as MinecraftServer.setWeatherParameters does.
pub fn apply(data: &mut SavedWeather, command: WeatherCommand, random: &mut LegacyRandom) -> bool {
    if command.duration.is_some_and(|duration| duration < 1) {
        return false;
    }
    let duration = command.duration.unwrap_or_else(|| {
        let (min, max) = match command.kind {
            WeatherKind::Clear => (12000, 180000),
            WeatherKind::Rain => (12000, 24000),
            WeatherKind::Thunder => (3600, 15600),
        };
        min + random
            .next_int_bound(max - min + 1)
            .expect("positive weather range")
    });
    let (clear, weather, raining, thundering) = match command.kind {
        WeatherKind::Clear => (duration, 0, false, false),
        WeatherKind::Rain => (0, duration, true, false),
        WeatherKind::Thunder => (0, duration, true, true),
    };
    *data = SavedWeather {
        clear_weather_time: clear,
        rain_time: weather,
        thunder_time: weather,
        raining,
        thundering,
    };
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_weather_modes_and_brigadier_time_arguments() {
        for input in [
            "/weather clear",
            "/weather rain 1",
            "/weather thunder .5s",
            "/weather clear 1d",
        ] {
            assert!(parse(input).is_some(), "{input}");
        }
        for input in [
            "/weather",
            "/weather snow",
            "/weather clear 0",
            "/weather clear -1",
            "/weather rain 1e2",
            "/weather thunder 1 extra",
        ] {
            assert!(parse(input).is_none(), "{input}");
        }
    }
}
