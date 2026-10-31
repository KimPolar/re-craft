//! Java Edition NBT, including Java's modified UTF-8 strings.

use std::io::{Cursor, Read, Write};

use anyhow::{Context, Result, bail, ensure};
use flate2::read::{GzDecoder, ZlibDecoder};

pub const MAX_NBT_BYTES: usize = 64 * 1024 * 1024;
const MAX_DEPTH: usize = 512;
const MAX_NODES: usize = 1_000_000;

/// Java strings retain UTF-16 code units, including unpaired surrogates.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JavaString(pub Vec<u16>);

impl JavaString {
    pub fn matches(&self, text: &str) -> bool {
        self.0.iter().copied().eq(text.encode_utf16())
    }

    pub fn to_string_lossy(&self) -> String {
        String::from_utf16_lossy(&self.0)
    }
}

impl From<&str> for JavaString {
    fn from(value: &str) -> Self {
        Self(value.encode_utf16().collect())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum TagType {
    End = 0,
    Byte = 1,
    Short = 2,
    Int = 3,
    Long = 4,
    Float = 5,
    Double = 6,
    ByteArray = 7,
    String = 8,
    List = 9,
    Compound = 10,
    IntArray = 11,
    LongArray = 12,
}

impl TryFrom<u8> for TagType {
    type Error = anyhow::Error;

    fn try_from(value: u8) -> Result<Self> {
        Ok(match value {
            0 => Self::End,
            1 => Self::Byte,
            2 => Self::Short,
            3 => Self::Int,
            4 => Self::Long,
            5 => Self::Float,
            6 => Self::Double,
            7 => Self::ByteArray,
            8 => Self::String,
            9 => Self::List,
            10 => Self::Compound,
            11 => Self::IntArray,
            12 => Self::LongArray,
            _ => bail!("unknown NBT tag type {value}"),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tag {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<u8>),
    String(JavaString),
    List {
        element_type: TagType,
        values: Vec<Tag>,
    },
    Compound(Vec<(JavaString, Tag)>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl Tag {
    /// Match `Number.longValue()` for Java dynamic numeric values.
    pub fn as_java_long(&self) -> Option<i64> {
        match self {
            Self::Byte(value) => Some(i64::from(*value)),
            Self::Short(value) => Some(i64::from(*value)),
            Self::Int(value) => Some(i64::from(*value)),
            Self::Long(value) => Some(*value),
            Self::Float(value) => Some(*value as i64),
            Self::Double(value) => Some(*value as i64),
            _ => None,
        }
    }

    /// Match `Number.intValue()` for numeric NBT tags used by Java codecs.
    pub fn as_java_int(&self) -> Option<i32> {
        match self {
            Self::Byte(value) => Some(i32::from(*value)),
            Self::Short(value) => Some(i32::from(*value)),
            Self::Int(value) => Some(*value),
            Self::Long(value) => Some(*value as i32),
            Self::Float(value) => Some(*value as i32),
            Self::Double(value) => Some(*value as i32),
            _ => None,
        }
    }

    pub fn tag_type(&self) -> TagType {
        match self {
            Self::Byte(_) => TagType::Byte,
            Self::Short(_) => TagType::Short,
            Self::Int(_) => TagType::Int,
            Self::Long(_) => TagType::Long,
            Self::Float(_) => TagType::Float,
            Self::Double(_) => TagType::Double,
            Self::ByteArray(_) => TagType::ByteArray,
            Self::String(_) => TagType::String,
            Self::List { .. } => TagType::List,
            Self::Compound(_) => TagType::Compound,
            Self::IntArray(_) => TagType::IntArray,
            Self::LongArray(_) => TagType::LongArray,
        }
    }

    /// Java's compound reader keeps the last value for duplicate names.
    pub fn get(&self, name: &str) -> Option<&Tag> {
        match self {
            Self::Compound(entries) => entries
                .iter()
                .rev()
                .find(|(key, _)| key.matches(name))
                .map(|(_, value)| value),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NamedTag {
    pub name: JavaString,
    pub value: Tag,
}

pub fn read(bytes: &[u8]) -> Result<NamedTag> {
    ensure!(
        bytes.len() <= MAX_NBT_BYTES,
        "NBT exceeds the {MAX_NBT_BYTES}-byte limit"
    );
    let mut parser = Parser {
        bytes,
        position: 0,
        nodes: 0,
    };
    let kind = TagType::try_from(parser.byte()?)?;
    ensure!(kind != TagType::End, "NBT root cannot be an end tag");
    let name = parser.string()?;
    let value = parser.payload(kind, 0)?;
    ensure!(
        parser.position == bytes.len(),
        "trailing bytes after NBT root"
    );
    Ok(NamedTag { name, value })
}

pub fn read_gzip(reader: impl Read) -> Result<NamedTag> {
    read(&decompress_limited(GzDecoder::new(reader))?)
}

pub fn read_zlib(reader: impl Read) -> Result<NamedTag> {
    read(&decompress_limited(ZlibDecoder::new(reader))?)
}

pub fn decompress_limited(reader: impl Read) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_NBT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .context("cannot decompress NBT")?;
    ensure!(
        bytes.len() <= MAX_NBT_BYTES,
        "decompressed NBT exceeds the {MAX_NBT_BYTES}-byte limit"
    );
    Ok(bytes)
}

pub fn write(root: &NamedTag) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    bytes.push(root.value.tag_type() as u8);
    write_string(&mut bytes, &root.name)?;
    write_payload(&mut bytes, &root.value, 0)?;
    ensure!(
        bytes.len() <= MAX_NBT_BYTES,
        "serialized NBT exceeds the {MAX_NBT_BYTES}-byte limit"
    );
    Ok(bytes)
}

struct Parser<'a> {
    bytes: &'a [u8],
    position: usize,
    nodes: usize,
}

impl<'a> Parser<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(length)
            .context("NBT offset overflow")?;
        let value = self
            .bytes
            .get(self.position..end)
            .context("truncated NBT")?;
        self.position = end;
        Ok(value)
    }

    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn short(&mut self) -> Result<i16> {
        Ok(i16::from_be_bytes(self.take(2)?.try_into()?))
    }
    fn int(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into()?))
    }
    fn long(&mut self) -> Result<i64> {
        Ok(i64::from_be_bytes(self.take(8)?.try_into()?))
    }

    fn length(&mut self, bytes_per_element: usize) -> Result<usize> {
        let length = self.int()?;
        ensure!(length >= 0, "negative NBT collection length");
        let length = length as usize;
        let required = length
            .checked_mul(bytes_per_element)
            .context("NBT collection length overflow")?;
        ensure!(
            required <= self.bytes.len() - self.position,
            "NBT collection exceeds remaining input"
        );
        Ok(length)
    }

    fn string(&mut self) -> Result<JavaString> {
        let length = u16::from_be_bytes(self.take(2)?.try_into()?) as usize;
        let bytes = self.take(length)?;
        let mut units = Vec::with_capacity(length);
        let mut i = 0;
        while i < bytes.len() {
            let first = bytes[i];
            if first & 0x80 == 0 {
                units.push(u16::from(first));
                i += 1;
            } else if first & 0xe0 == 0xc0 {
                ensure!(
                    i + 1 < bytes.len() && bytes[i + 1] & 0xc0 == 0x80,
                    "invalid modified UTF-8"
                );
                units.push((u16::from(first & 0x1f) << 6) | u16::from(bytes[i + 1] & 0x3f));
                i += 2;
            } else if first & 0xf0 == 0xe0 {
                ensure!(
                    i + 2 < bytes.len()
                        && bytes[i + 1] & 0xc0 == 0x80
                        && bytes[i + 2] & 0xc0 == 0x80,
                    "invalid modified UTF-8"
                );
                units.push(
                    (u16::from(first & 0x0f) << 12)
                        | (u16::from(bytes[i + 1] & 0x3f) << 6)
                        | u16::from(bytes[i + 2] & 0x3f),
                );
                i += 3;
            } else {
                bail!("invalid modified UTF-8");
            }
        }
        Ok(JavaString(units))
    }

    fn payload(&mut self, kind: TagType, depth: usize) -> Result<Tag> {
        ensure!(depth <= MAX_DEPTH, "NBT nesting exceeds {MAX_DEPTH}");
        self.nodes += 1;
        ensure!(self.nodes <= MAX_NODES, "NBT contains too many tags");
        Ok(match kind {
            TagType::End => bail!("end tag is not a value"),
            TagType::Byte => Tag::Byte(self.byte()? as i8),
            TagType::Short => Tag::Short(self.short()?),
            TagType::Int => Tag::Int(self.int()?),
            TagType::Long => Tag::Long(self.long()?),
            TagType::Float => Tag::Float(f32::from_bits(self.int()? as u32)),
            TagType::Double => Tag::Double(f64::from_bits(self.long()? as u64)),
            TagType::ByteArray => {
                let len = self.length(1)?;
                Tag::ByteArray(self.take(len)?.to_vec())
            }
            TagType::String => Tag::String(self.string()?),
            TagType::List => {
                let element_type = TagType::try_from(self.byte()?)?;
                let length = self.length(1)?;
                ensure!(
                    length == 0 || element_type != TagType::End,
                    "nonempty NBT list has end element type"
                );
                ensure!(
                    length <= MAX_NODES - self.nodes,
                    "NBT list contains too many tags"
                );
                let mut values = Vec::with_capacity(length);
                for _ in 0..length {
                    values.push(self.payload(element_type, depth + 1)?);
                }
                Tag::List {
                    element_type,
                    values,
                }
            }
            TagType::Compound => {
                let mut entries = Vec::new();
                loop {
                    let child = TagType::try_from(self.byte()?)?;
                    if child == TagType::End {
                        break;
                    }
                    let name = self.string()?;
                    entries.push((name, self.payload(child, depth + 1)?));
                }
                Tag::Compound(entries)
            }
            TagType::IntArray => {
                let length = self.length(4)?;
                let mut values = Vec::with_capacity(length);
                for _ in 0..length {
                    values.push(self.int()?);
                }
                Tag::IntArray(values)
            }
            TagType::LongArray => {
                let length = self.length(8)?;
                let mut values = Vec::with_capacity(length);
                for _ in 0..length {
                    values.push(self.long()?);
                }
                Tag::LongArray(values)
            }
        })
    }
}

fn write_string(output: &mut Vec<u8>, value: &JavaString) -> Result<()> {
    let mut encoded = Vec::new();
    for &unit in &value.0 {
        if (1..=0x7f).contains(&unit) {
            encoded.push(unit as u8);
        } else if unit <= 0x7ff {
            encoded.push((0xc0 | (unit >> 6)) as u8);
            encoded.push((0x80 | (unit & 0x3f)) as u8);
        } else {
            encoded.push((0xe0 | (unit >> 12)) as u8);
            encoded.push((0x80 | ((unit >> 6) & 0x3f)) as u8);
            encoded.push((0x80 | (unit & 0x3f)) as u8);
        }
    }
    let length =
        u16::try_from(encoded.len()).context("Java NBT string exceeds 65535 encoded bytes")?;
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(&encoded);
    Ok(())
}

fn write_length(output: &mut Vec<u8>, length: usize) -> Result<()> {
    let length = i32::try_from(length).context("NBT collection exceeds signed 32-bit length")?;
    output.extend_from_slice(&length.to_be_bytes());
    Ok(())
}

fn write_payload(output: &mut Vec<u8>, value: &Tag, depth: usize) -> Result<()> {
    ensure!(depth <= MAX_DEPTH, "NBT nesting exceeds {MAX_DEPTH}");
    match value {
        Tag::Byte(value) => output.push(*value as u8),
        Tag::Short(value) => output.extend_from_slice(&value.to_be_bytes()),
        Tag::Int(value) => output.extend_from_slice(&value.to_be_bytes()),
        Tag::Long(value) => output.extend_from_slice(&value.to_be_bytes()),
        Tag::Float(value) => output.extend_from_slice(&value.to_bits().to_be_bytes()),
        Tag::Double(value) => output.extend_from_slice(&value.to_bits().to_be_bytes()),
        Tag::ByteArray(values) => {
            write_length(output, values.len())?;
            output.extend_from_slice(values);
        }
        Tag::String(value) => write_string(output, value)?,
        Tag::List {
            element_type,
            values,
        } => {
            ensure!(
                values.is_empty() || *element_type != TagType::End,
                "nonempty NBT list has end element type"
            );
            ensure!(
                values.iter().all(|value| value.tag_type() == *element_type),
                "NBT list has mixed element types"
            );
            output.push(*element_type as u8);
            write_length(output, values.len())?;
            for value in values {
                write_payload(output, value, depth + 1)?;
            }
        }
        Tag::Compound(entries) => {
            for (name, value) in entries {
                output.push(value.tag_type() as u8);
                write_string(output, name)?;
                write_payload(output, value, depth + 1)?;
            }
            output.push(TagType::End as u8);
        }
        Tag::IntArray(values) => {
            write_length(output, values.len())?;
            for value in values {
                output.write_all(&value.to_be_bytes())?;
            }
        }
        Tag::LongArray(values) => {
            write_length(output, values.len())?;
            for value in values {
                output.write_all(&value.to_be_bytes())?;
            }
        }
    }
    Ok(())
}

pub fn decode_payload(compression: u8, bytes: &[u8]) -> Result<NamedTag> {
    read(&decompress_payload(compression, bytes)?)
}

pub(crate) fn decompress_payload(compression: u8, bytes: &[u8]) -> Result<Vec<u8>> {
    match compression {
        1 => decompress_limited(GzDecoder::new(Cursor::new(bytes))),
        2 => decompress_limited(ZlibDecoder::new(Cursor::new(bytes))),
        3 => {
            ensure!(
                bytes.len() <= MAX_NBT_BYTES,
                "NBT exceeds the {MAX_NBT_BYTES}-byte limit"
            );
            Ok(bytes.to_vec())
        }
        4 => {
            validate_java_lz4(bytes)?;
            let reader = lz4_java_wrc::Lz4BlockInput::new(Cursor::new(bytes));
            decompress_limited(reader)
        }
        _ => bail!("unsupported Anvil compression type {compression}"),
    }
}

// Validate block lengths before handing bytes to the decoder: a corrupt header must not
// cause a large allocation before the decompressed-stream limit is enforced.
fn validate_java_lz4(bytes: &[u8]) -> Result<()> {
    const HEADER: usize = 21;
    let mut offset = 0usize;
    let mut total = 0usize;
    loop {
        let end = offset
            .checked_add(HEADER)
            .context("LZ4 header offset overflow")?;
        let header = bytes
            .get(offset..end)
            .context("truncated LZ4 block header")?;
        ensure!(&header[..8] == b"LZ4Block", "invalid Java LZ4 block magic");
        let compressed = u32::from_le_bytes(header[9..13].try_into()?) as usize;
        let uncompressed = u32::from_le_bytes(header[13..17].try_into()?) as usize;
        if compressed == 0 && uncompressed == 0 {
            ensure!(
                header[17..21] == [0; 4] && end == bytes.len(),
                "invalid Java LZ4 end marker"
            );
            return Ok(());
        }
        ensure!(
            compressed > 0 && uncompressed > 0,
            "invalid Java LZ4 block lengths"
        );
        total = total
            .checked_add(uncompressed)
            .context("LZ4 output size overflow")?;
        ensure!(
            total <= MAX_NBT_BYTES,
            "decompressed NBT exceeds the {MAX_NBT_BYTES}-byte limit"
        );
        offset = end
            .checked_add(compressed)
            .context("LZ4 block size overflow")?;
        ensure!(offset <= bytes.len(), "truncated Java LZ4 block");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_types_round_trip_including_java_utf16_and_empty_list_type() {
        let root = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("byte".into(), Tag::Byte(-5)),
                ("short".into(), Tag::Short(-1234)),
                ("int".into(), Tag::Int(-42)),
                ("long".into(), Tag::Long(i64::MIN)),
                ("float".into(), Tag::Float(0.25)),
                ("double".into(), Tag::Double(-42.5)),
                ("bytes".into(), Tag::ByteArray(vec![0, 128, 255])),
                (
                    "text".into(),
                    Tag::String(JavaString(vec![0, 0x0041, 0xd83d, 0xde00, 0xd800])),
                ),
                (
                    "list".into(),
                    Tag::List {
                        element_type: TagType::Int,
                        values: vec![Tag::Int(1), Tag::Int(2)],
                    },
                ),
                (
                    "empty".into(),
                    Tag::List {
                        element_type: TagType::String,
                        values: vec![],
                    },
                ),
                ("ints".into(), Tag::IntArray(vec![i32::MIN, 0, i32::MAX])),
                ("longs".into(), Tag::LongArray(vec![i64::MIN, 0, i64::MAX])),
            ]),
        };
        let bytes = write(&root).unwrap();
        assert_eq!(read(&bytes).unwrap(), root);
        assert!(bytes.windows(2).any(|window| window == [0xc0, 0x80]));
    }

    #[test]
    fn truncated_and_invalid_lengths_fail() {
        assert!(read(&[10, 0, 0, 3]).is_err());
        assert!(read(&[7, 0, 0, 0xff, 0xff, 0xff, 0xff]).is_err());
        assert!(read(&[9, 0, 0, 0, 0, 0, 0, 1]).is_err());
    }

    #[test]
    fn mixed_list_cannot_be_written() {
        let root = NamedTag {
            name: "".into(),
            value: Tag::List {
                element_type: TagType::Int,
                values: vec![Tag::String("x".into())],
            },
        };
        assert!(write(&root).is_err());
    }

    #[test]
    fn corrupt_lz4_header_is_rejected_before_allocation() {
        let mut header = b"LZ4Block".to_vec();
        header.push(0x20);
        header.extend_from_slice(&u32::MAX.to_le_bytes());
        header.extend_from_slice(&16u32.to_le_bytes());
        header.extend_from_slice(&0u32.to_le_bytes());
        assert!(decode_payload(4, &header).is_err());
    }
}
