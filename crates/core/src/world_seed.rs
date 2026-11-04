//! Java Edition's create-world seed text parsing.

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
    use super::parse_seed;

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
}
