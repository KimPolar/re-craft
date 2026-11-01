//! Supported 26.3 time commands and Brigadier's float-based time argument.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeCommand {
    QueryGameTime,
    QueryClock,
    Set(i32),
    SetMarker(TimeMarker),
    Add(i32),
    Pause,
    Resume,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeMarker {
    Day,
    Noon,
    Night,
    Midnight,
}
impl TimeMarker {
    pub fn name(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Noon => "noon",
            Self::Night => "night",
            Self::Midnight => "midnight",
        }
    }
    fn parse(name: &str) -> Option<Self> {
        match name
            .strip_prefix("minecraft:")
            .or_else(|| name.strip_prefix(':'))
            .unwrap_or(name)
        {
            "day" => Some(Self::Day),
            "noon" => Some(Self::Noon),
            "night" => Some(Self::Night),
            "midnight" => Some(Self::Midnight),
            _ => None,
        }
    }
}

/// Parse only the currently supported default-clock forms.
pub fn parse(input: &str) -> Option<TimeCommand> {
    let parts: Vec<_> = input.split(' ').collect();
    match parts.as_slice() {
        ["/time", "query", "gametime"] => Some(TimeCommand::QueryGameTime),
        ["/time", "query", "time"] => Some(TimeCommand::QueryClock),
        ["/time", "set", value] => duration(value, 0)
            .map(TimeCommand::Set)
            .or_else(|| TimeMarker::parse(value).map(TimeCommand::SetMarker)),
        ["/time", "add", value] => duration(value, i32::MIN).map(TimeCommand::Add),
        ["/time", "pause"] => Some(TimeCommand::Pause),
        ["/time", "resume"] => Some(TimeCommand::Resume),
        _ => None,
    }
}

pub fn duration(input: &str, minimum: i32) -> Option<i32> {
    let end = input
        .bytes()
        .take_while(|b| b.is_ascii_digit() || *b == b'-' || *b == b'.')
        .count();
    let number: f32 = input[..end].parse().ok()?;
    let factor = match &input[end..] {
        "" | "t" => 1.0_f32,
        "s" => 20.0,
        "d" => 24000.0,
        _ => return None,
    };
    // Math.round(float) rounds ties toward positive infinity and saturates at i32 limits.
    let ticks = ((number * factor) as f64 + 0.5).floor() as i32;
    (ticks >= minimum).then_some(ticks)
}

/// Return the next occurrence, or `None` when the clock is already on it.
pub fn resolve_marker(
    marker: native_resources::world_clock::TimeMarker,
    current: i64,
) -> Option<i64> {
    match marker.period {
        None => (current != i64::from(marker.ticks)).then_some(i64::from(marker.ticks)),
        Some(period) if period <= 0 => None,
        Some(period) => {
            let offset = current % i64::from(period);
            let distance = i64::from(marker.ticks).wrapping_sub(offset);
            if distance == 0 {
                return None;
            }
            let distance = if distance > 0 {
                distance
            } else {
                i64::from(period).wrapping_add(distance)
            };
            Some(current.wrapping_add(distance))
        }
    }
}

pub fn set_clock(clock: &mut native_persistence::world::SavedClock, ticks: i32) -> bool {
    if clock.total_ticks == i64::from(ticks) {
        return false;
    }
    clock.total_ticks = i64::from(ticks);
    clock.partial_tick = 0.0;
    true
}

pub fn add_clock(clock: &mut native_persistence::world::SavedClock, ticks: i32) {
    clock.total_ticks = clock.total_ticks.wrapping_add(i64::from(ticks)).max(0);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_units_rounding_and_supported_syntax() {
        for (text, min, expected) in [
            ("1d", 0, Some(24000)),
            ("1.5s", 0, Some(30)),
            ("-0.5", 0, Some(0)),
            ("-0.6", 0, None),
            ("-1s", i32::MIN, Some(-20)),
            ("1e3", 0, None),
            ("+1", 0, None),
            ("1day", 0, None),
            ("1.2.3", 0, None),
        ] {
            assert_eq!(duration(text, min), expected, "{text}");
        }
        assert_eq!(parse("/time set 1d"), Some(TimeCommand::Set(24000)));
        assert_eq!(
            parse("/time query gametime"),
            Some(TimeCommand::QueryGameTime)
        );
        assert_eq!(parse("/time query time"), Some(TimeCommand::QueryClock));
        assert_eq!(
            parse("/time set day"),
            Some(TimeCommand::SetMarker(TimeMarker::Day))
        );
        assert_eq!(
            parse("/time set minecraft:night"),
            Some(TimeCommand::SetMarker(TimeMarker::Night))
        );
        assert_eq!(parse("/time add 1 extra"), None);
    }
}
