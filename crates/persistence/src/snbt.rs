//! Conservative parser for the SNBT values emitted by Java's NBT printer.
//! Unsupported syntax is rejected instead of being interpreted as different data.

use crate::nbt::{JavaString, Tag, TagType};
use anyhow::{Context, Result, bail, ensure};

pub fn parse_compound(input: &JavaString) -> Result<Tag> {
    let text = String::from_utf16(&input.0).context("SNBT has invalid UTF-16")?;
    ensure!(text.len() <= 1_048_576, "SNBT is too large");
    let mut parser = Parser {
        text: &text,
        offset: 0,
        nodes: 0,
    };
    let tag = parser.value(0)?;
    parser.space();
    ensure!(parser.offset == text.len(), "trailing SNBT input");
    ensure!(
        matches!(tag, Tag::Compound(_)),
        "SNBT root must be a compound"
    );
    Ok(tag)
}

struct Parser<'a> {
    text: &'a str,
    offset: usize,
    nodes: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.text[self.offset..].chars().next()
    }
    fn take(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.offset += c.len_utf8();
        Some(c)
    }
    fn space(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.take();
        }
    }
    fn expect(&mut self, c: char) -> Result<()> {
        self.space();
        ensure!(self.take() == Some(c), "expected {c}");
        Ok(())
    }
    fn token(&mut self) -> Result<String> {
        self.space();
        if matches!(self.peek(), Some('"' | '\'')) {
            let quote = self.take().unwrap();
            let mut result = String::new();
            loop {
                let c = self.take().context("unterminated SNBT string")?;
                if c == quote {
                    return Ok(result);
                }
                if c == '\\' {
                    let next = self.take().context("unterminated escape")?;
                    ensure!(
                        next == quote || next == '\\',
                        "unsupported SNBT string escape"
                    );
                    result.push(next);
                } else {
                    result.push(c);
                }
            }
        }
        let start = self.offset;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '.'))
        {
            self.take();
        }
        ensure!(start != self.offset, "expected SNBT token");
        Ok(self.text[start..self.offset].to_owned())
    }
    fn value(&mut self, depth: usize) -> Result<Tag> {
        ensure!(depth <= 128, "SNBT nesting too deep");
        self.nodes += 1;
        ensure!(self.nodes <= 100_000, "too many SNBT values");
        self.space();
        match self.peek() {
            Some('{') => self.compound(depth),
            Some('[') => self.list(depth),
            Some('"' | '\'') => Ok(Tag::String(self.token()?.as_str().into())),
            Some(_) => {
                let token = self.token()?;
                let lower = token.to_ascii_lowercase();
                if lower == "true" {
                    return Ok(Tag::Byte(1));
                }
                if lower == "false" {
                    return Ok(Tag::Byte(0));
                }
                ensure!(
                    !matches!(
                        lower.as_str(),
                        "nan" | "nanf" | "nand" | "infinity" | "infinityf" | "infinityd"
                    ),
                    "unsupported nonfinite SNBT value"
                );
                if token.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '-' | '+' | '.')) {
                    let suffix = lower.chars().last().unwrap();
                    let number = if matches!(suffix, 'b' | 's' | 'l' | 'f' | 'd') {
                        &token[..token.len() - 1]
                    } else {
                        &token
                    };
                    return Ok(match suffix {
                        'b' => Tag::Byte(number.parse().context("invalid SNBT byte")?),
                        's' => Tag::Short(number.parse().context("invalid SNBT short")?),
                        'l' => Tag::Long(number.parse().context("invalid SNBT long")?),
                        'f' => {
                            let value: f32 = number.parse().context("invalid SNBT float")?;
                            ensure!(value.is_finite(), "nonfinite SNBT float");
                            Tag::Float(value)
                        }
                        'd' => {
                            let value: f64 = number.parse().context("invalid SNBT double")?;
                            ensure!(value.is_finite(), "nonfinite SNBT double");
                            Tag::Double(value)
                        }
                        _ if number.contains(['.', 'e', 'E']) => {
                            let value: f64 = number.parse().context("invalid SNBT double")?;
                            ensure!(value.is_finite(), "nonfinite SNBT double");
                            Tag::Double(value)
                        }
                        _ => Tag::Int(number.parse().context("invalid SNBT int")?),
                    });
                }
                Ok(Tag::String(token.as_str().into()))
            }
            None => bail!("missing SNBT value"),
        }
    }
    fn compound(&mut self, depth: usize) -> Result<Tag> {
        self.expect('{')?;
        let mut values = Vec::new();
        self.space();
        if self.peek() == Some('}') {
            self.take();
            return Ok(Tag::Compound(values));
        }
        loop {
            let key: JavaString = self.token()?.as_str().into();
            self.expect(':')?;
            let value = self.value(depth + 1)?;
            if let Some(existing) = values.iter_mut().find(|(name, _)| *name == key) {
                existing.1 = value;
            } else {
                values.push((key, value));
            }
            self.space();
            match self.take() {
                Some('}') => break,
                Some(',') => {}
                _ => bail!("expected comma or closing brace"),
            }
        }
        Ok(Tag::Compound(values))
    }
    fn list(&mut self, depth: usize) -> Result<Tag> {
        self.expect('[')?;
        self.space();
        let array = if self.text[self.offset..].starts_with("B;") {
            Some('B')
        } else if self.text[self.offset..].starts_with("I;") {
            Some('I')
        } else if self.text[self.offset..].starts_with("L;") {
            Some('L')
        } else {
            None
        };
        if array.is_some() {
            self.offset += 2;
        }
        let mut values = Vec::new();
        self.space();
        if self.peek() == Some(']') {
            self.take();
        } else {
            loop {
                values.push(self.value(depth + 1)?);
                self.space();
                match self.take() {
                    Some(']') => break,
                    Some(',') => {}
                    _ => bail!("expected comma or closing bracket"),
                }
            }
        }
        Ok(match array {
            Some('B') => Tag::ByteArray(
                values
                    .into_iter()
                    .map(|v| match v {
                        Tag::Byte(n) => Ok(n as u8),
                        _ => bail!("invalid SNBT byte array"),
                    })
                    .collect::<Result<_>>()?,
            ),
            Some('I') => Tag::IntArray(
                values
                    .into_iter()
                    .map(|v| match v {
                        Tag::Int(n) => Ok(n),
                        _ => bail!("invalid SNBT int array"),
                    })
                    .collect::<Result<_>>()?,
            ),
            Some('L') => Tag::LongArray(
                values
                    .into_iter()
                    .map(|v| match v {
                        Tag::Long(n) => Ok(n),
                        _ => bail!("invalid SNBT long array"),
                    })
                    .collect::<Result<_>>()?,
            ),
            Some(_) => unreachable!(),
            None => {
                let element_type = values.first().map_or(TagType::End, Tag::tag_type);
                ensure!(
                    values.iter().all(|v| v.tag_type() == element_type),
                    "unsupported mixed SNBT list"
                );
                Tag::List {
                    element_type,
                    values,
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canonical_nested_values_and_arrays() {
        let value = parse_compound(&r#"{byte:-3b,short:4s,int:5,long:6L,float:1.5f,double:2.25d,text:"hello\\\" world",bytes:[B;-1b,2b],ints:[I;1,2],longs:[L;3L,4L],list:[{v:1},{v:2}]}"#.into()).unwrap();
        assert_eq!(value.get("byte"), Some(&Tag::Byte(-3)));
        assert_eq!(value.get("short"), Some(&Tag::Short(4)));
        assert_eq!(value.get("int"), Some(&Tag::Int(5)));
        assert_eq!(value.get("long"), Some(&Tag::Long(6)));
        assert_eq!(value.get("float"), Some(&Tag::Float(1.5)));
        assert_eq!(value.get("double"), Some(&Tag::Double(2.25)));
        assert_eq!(
            value.get("text"),
            Some(&Tag::String("hello\\\" world".into()))
        );
        assert_eq!(value.get("bytes"), Some(&Tag::ByteArray(vec![255, 2])));
        assert_eq!(value.get("ints"), Some(&Tag::IntArray(vec![1, 2])));
        assert_eq!(value.get("longs"), Some(&Tag::LongArray(vec![3, 4])));
        assert!(matches!(value.get("list"), Some(Tag::List { values, .. }) if values.len() == 2));
    }

    #[test]
    fn rejects_unsupported_or_incomplete_input() {
        for text in [
            "{n:NaNf}",
            "{n:1b} trailing",
            "{x:[1,2b]}",
            "{x:[B;1]}",
            "{x:1,}",
            "{x:\"\\n\"}",
            "[1]",
        ] {
            assert!(parse_compound(&text.into()).is_err(), "{text}");
        }
    }
}
