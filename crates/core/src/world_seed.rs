//! Java Edition's create-world seed text parsing.

use std::{
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use crate::random::LegacyRandom;

const INITIAL_UNIQUIFIER: u64 = 8_682_522_807_148_012;
const UNIQUIFIER_MULTIPLIER: u64 = 1_181_783_497_276_652_981;

/// Java's seed uniquifier and legacy random sequence. The time source is
/// supplied separately because Java's `nanoTime` has an arbitrary origin.
pub struct RandomWorldSeed {
    uniquifier: AtomicU64,
}

impl Default for RandomWorldSeed {
    fn default() -> Self {
        Self::new()
    }
}

impl RandomWorldSeed {
    pub const fn new() -> Self {
        Self {
            uniquifier: AtomicU64::new(INITIAL_UNIQUIFIER),
        }
    }

    /// Read the clock after advancing the atomic, as RandomSupport does.
    pub fn next_with_clock(&self, nano_time: impl FnOnce() -> i64) -> i64 {
        let old = self
            .uniquifier
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
                Some(value.wrapping_mul(UNIQUIFIER_MULTIPLIER))
            })
            .expect("the uniquifier update always succeeds");
        let unique = old.wrapping_mul(UNIQUIFIER_MULTIPLIER);
        LegacyRandom::new((unique ^ nano_time() as u64) as i64).next_long()
    }
}

fn nano_time() -> i64 {
    static START: OnceLock<(Instant, u64)> = OnceLock::new();
    let (instant, base) = START.get_or_init(|| {
        let instant = Instant::now();
        let base = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or_else(|error| (error.duration().as_nanos() as u64).wrapping_neg());
        (instant, base)
    });
    base.wrapping_add(instant.elapsed().as_nanos() as u64) as i64
}

/// Select a seed at world creation. Java evaluates a random fallback even
/// for nonempty inputs because `OptionalLong.orElse` evaluates its argument.
pub fn resolve_seed(input: &str) -> i64 {
    static GENERATOR: RandomWorldSeed = RandomWorldSeed::new();
    let fallback = GENERATOR.next_with_clock(nano_time);
    parse_seed(input).unwrap_or(fallback)
}

// Decimal digit ranges recognized by Java 25's Character.digit(char, 10).
// The parser works on UTF-16 code units, just like Long.parseLong(String).
const DECIMAL_ZEROES: [u16; 37] = [
    0x0030, 0x0660, 0x06f0, 0x07c0, 0x0966, 0x09e6, 0x0a66, 0x0ae6, 0x0b66, 0x0be6, 0x0c66, 0x0ce6,
    0x0d66, 0x0de6, 0x0e50, 0x0ed0, 0x0f20, 0x1040, 0x1090, 0x17e0, 0x1810, 0x1946, 0x19d0, 0x1a80,
    0x1a90, 0x1b50, 0x1bb0, 0x1c40, 0x1c50, 0xa620, 0xa8d0, 0xa900, 0xa9d0, 0xa9f0, 0xaa50, 0xabf0,
    0xff10,
];

fn digit(unit: u16) -> Option<i64> {
    let index = DECIMAL_ZEROES.partition_point(|&start| start <= unit);
    let start = *DECIMAL_ZEROES.get(index.checked_sub(1)?)?;
    let value = unit - start;
    (value < 10).then_some(i64::from(value))
}

fn parse_java_long(text: &str) -> Option<i64> {
    let mut units = text.encode_utf16().peekable();
    let negative = match units.peek().copied() {
        Some(0x2d) => {
            units.next();
            true
        }
        Some(0x2b) => {
            units.next();
            false
        }
        _ => false,
    };
    let mut value = 0i64;
    let mut any = false;
    for unit in units {
        any = true;
        value = value.checked_mul(10)?.checked_sub(digit(unit)?)?;
    }
    if !any {
        return None;
    }
    if negative {
        Some(value)
    } else {
        value.checked_neg()
    }
}

/// Return `None` when Java would choose a random seed for a blank input.
/// Otherwise return the parsed long or Java's signed UTF-16 string hash.
pub fn parse_seed(input: &str) -> Option<i64> {
    let trimmed = input.trim_matches(|c| c <= '\u{20}');
    if trimmed.is_empty() {
        return None;
    }
    Some(parse_java_long(trimmed).unwrap_or_else(|| {
        let hash = trimmed.encode_utf16().fold(0i32, |hash, unit| {
            hash.wrapping_mul(31).wrapping_add(i32::from(unit))
        });
        i64::from(hash)
    }))
}

#[cfg(test)]
mod tests {
    use super::{RandomWorldSeed, parse_seed, resolve_seed};

    #[test]
    fn parses_java_seed_inputs_and_falls_back_to_utf16_hash() {
        assert_eq!(parse_seed(" \t\0"), None);
        assert_eq!(parse_seed(" -9223372036854775808 "), Some(i64::MIN));
        assert_eq!(parse_seed("+9223372036854775807"), Some(i64::MAX));
        assert_eq!(parse_seed("１２٣"), Some(123));
        assert_eq!(parse_seed("abc"), Some(96354));
        assert_eq!(parse_seed("😀"), Some(1_772_899));
        assert_ne!(parse_seed("\u{a0}"), None);
        assert_ne!(parse_seed("9223372036854775808"), Some(i64::MAX));
    }

    #[test]
    fn random_seed_counter_is_shared_safely_and_explicit_inputs_win() {
        use std::{collections::BTreeSet, sync::Arc};
        assert_eq!(resolve_seed(" -42 "), -42);
        assert_eq!(resolve_seed("abc"), 96354);
        let generator = Arc::new(RandomWorldSeed::new());
        let values = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..64)
                .map(|_| {
                    let generator = Arc::clone(&generator);
                    scope.spawn(move || generator.next_with_clock(|| 123456789))
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<BTreeSet<_>>()
        });
        assert_eq!(values.len(), 64);
    }
}
