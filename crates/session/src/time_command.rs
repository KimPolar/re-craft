//! Supported 26.3 time commands and Brigadier's float-based time argument.

use native_resources::packs::ResourceId;

/// Parse command identifiers without using them as filesystem paths.
pub fn parse_identifier(input: &str) -> Option<ResourceId> {
    let (namespace, path) = input.split_once(':').unwrap_or(("minecraft", input));
    let namespace = if namespace.is_empty() {
        "minecraft"
    } else {
        namespace
    };
    if !namespace
        .bytes()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_.-".contains(&c))
        || !path
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_./-".contains(&c))
    {
        return None;
    }
    Some(ResourceId {
        namespace: namespace.into(),
        path: path.into(),
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeCommand {
    pub clock: Option<ResourceId>,
    pub action: TimeAction,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimeAction {
    QueryGameTime,
    QueryClock,
    QueryTimeline {
        timeline: ResourceId,
        repetitions: bool,
    },
    Set(i32),
    SetMarker(ResourceId),
    Add(i32),
    Pause,
    Resume,
    SetRate(u32),
}

/// Parse default-clock and `of <clock>` forms, retaining data-pack identifiers.
pub fn parse(input: &str) -> Option<TimeCommand> {
    let parts: Vec<_> = input.split(' ').collect();
    let (clock, args) = match parts.as_slice() {
        ["/time", "of", name, rest @ ..] if !name.is_empty() => {
            (Some(parse_identifier(name)?), rest)
        }
        ["/time", rest @ ..] => (None, rest),
        _ => return None,
    };
    let action = match args {
        ["query", "gametime"] if clock.is_none() => TimeAction::QueryGameTime,
        ["query", "time"] => TimeAction::QueryClock,
        ["query", name] if !name.is_empty() => TimeAction::QueryTimeline {
            timeline: parse_identifier(name)?,
            repetitions: false,
        },
        ["query", name, "repetition"] if !name.is_empty() => TimeAction::QueryTimeline {
            timeline: parse_identifier(name)?,
            repetitions: true,
        },
        ["set", value] if !value.is_empty() => duration(value, 0)
            .map(TimeAction::Set)
            .or_else(|| parse_identifier(value).map(TimeAction::SetMarker))?,
        ["add", value] => TimeAction::Add(duration(value, i32::MIN)?),
        ["pause"] => TimeAction::Pause,
        ["resume"] => TimeAction::Resume,
        ["rate", value] => TimeAction::SetRate(rate(value)?.to_bits()),
        _ => return None,
    };
    Some(TimeCommand { clock, action })
}

pub fn rate(input: &str) -> Option<f32> {
    if input.is_empty()
        || !input
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'-' || b == b'.')
    {
        return None;
    }
    let value: f32 = input.parse().ok()?;
    (0.00001..=1000.0).contains(&value).then_some(value)
}

/// Java integer division truncates towards zero and casts period counts to int.
pub fn timeline_value(ticks: i64, period: Option<i32>, repetitions: bool) -> i64 {
    match (period, repetitions) {
        (Some(period), true) => i64::from((ticks / i64::from(period)) as i32),
        (Some(period), false) => ticks % i64::from(period),
        (None, true) => 0,
        (None, false) => ticks,
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
        assert_eq!(
            parse("/time set 1d").unwrap().action,
            TimeAction::Set(24000)
        );
        assert_eq!(
            parse("/time query gametime").unwrap().action,
            TimeAction::QueryGameTime
        );
        assert_eq!(
            parse("/time query time").unwrap().action,
            TimeAction::QueryClock
        );
        assert_eq!(
            parse("/time set day").unwrap().action,
            TimeAction::SetMarker(ResourceId::parse("minecraft:day").unwrap())
        );
        assert_eq!(
            parse("/time set minecraft:night").unwrap().action,
            TimeAction::SetMarker(ResourceId::parse("minecraft:night").unwrap())
        );
        let custom = parse("/time of example:clock set example:wake").unwrap();
        assert_eq!(
            custom.clock,
            Some(ResourceId::parse("example:clock").unwrap())
        );
        assert_eq!(
            custom.action,
            TimeAction::SetMarker(ResourceId::parse("example:wake").unwrap())
        );
        assert!(matches!(
            parse("/time of example:clock query gametime")
                .unwrap()
                .action,
            TimeAction::QueryTimeline { .. }
        ));
        assert_eq!(parse_identifier("example:").unwrap().path, "");
        assert_eq!(parse_identifier("example:a/../b").unwrap().path, "a/../b");
        assert_eq!(parse("/time set "), None);
        assert_eq!(parse("/time add 1 extra"), None);
        for (input, valid) in [
            ("0.00001", true),
            ("1000", true),
            (".5", true),
            ("0", false),
            ("1000.1", false),
            ("+1", false),
            ("1e2", false),
            ("1.2.3", false),
        ] {
            assert_eq!(rate(input).is_some(), valid, "{input}");
        }
        assert!(matches!(
            parse("/time query example:timeline repetition")
                .unwrap()
                .action,
            TimeAction::QueryTimeline {
                repetitions: true,
                ..
            }
        ));
        assert_eq!(parse("/time query timeline example:timeline"), None);
    }
}
