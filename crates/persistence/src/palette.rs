//! Decoding 26.3 chunk block-state palettes. No registry entries are invented.

use std::collections::BTreeSet;

use anyhow::{Context, Result, bail, ensure};

use crate::nbt::{JavaString, Tag, TagType};

pub const BLOCKS_PER_SECTION: usize = 16 * 16 * 16;
pub const BIOMES_PER_SECTION: usize = 4 * 4 * 4;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlockState {
    pub name: JavaString,
    /// An empty list represents the registry's default state, which can have properties.
    pub properties: Vec<(JavaString, JavaString)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlockSection {
    pub y: i8,
    pub palette: Vec<BlockState>,
    /// Vanilla order: x changes fastest, followed by z, then y.
    pub indices: Vec<u32>,
    pub biomes: Option<BiomePalette>,
    pub block_light: Option<Vec<u8>>,
    pub sky_light: Option<Vec<u8>>,
}

/// A stored section may contain only lighting or biome data and no block states.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChunkSection {
    pub y: i8,
    pub blocks: Option<BlockPalette>,
    pub biomes: Option<BiomePalette>,
    pub block_light: Option<Vec<u8>>,
    pub sky_light: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlockPalette {
    pub states: Vec<BlockState>,
    pub indices: Vec<u32>,
}

impl BlockPalette {
    pub fn block(&self, x: usize, y: usize, z: usize) -> Option<&BlockState> {
        if x >= 16 || y >= 16 || z >= 16 {
            return None;
        }
        self.states
            .get(*self.indices.get(x + z * 16 + y * 256)? as usize)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BiomePalette {
    pub names: Vec<JavaString>,
    pub indices: Vec<u32>,
}

impl BiomePalette {
    pub fn biome(&self, x: usize, y: usize, z: usize) -> Option<&JavaString> {
        if x >= 4 || y >= 4 || z >= 4 {
            return None;
        }
        self.names
            .get(*self.indices.get(x + z * 4 + y * 16)? as usize)
    }
}

impl BlockSection {
    pub fn block(&self, x: usize, y: usize, z: usize) -> Option<&BlockState> {
        if x >= 16 || y >= 16 || z >= 16 {
            return None;
        }
        let index = x + z * 16 + y * 256;
        self.palette.get(*self.indices.get(index)? as usize)
    }
}

/// Decodes the non-crossing 64-bit packing used by modern Java Edition sections.
pub fn decode_indices(
    palette_len: usize,
    data: Option<&[i64]>,
    count: usize,
    min_bits: u32,
) -> Result<Vec<u32>> {
    ensure!(
        palette_len > 0 && palette_len <= count,
        "invalid palette length {palette_len}"
    );
    if palette_len == 1 && data.is_none() {
        return Ok(vec![0; count]);
    }
    let bits = (usize::BITS - (palette_len - 1).leading_zeros()).max(min_bits);
    ensure!(
        (1..=32).contains(&bits),
        "unsupported palette bit width {bits}"
    );
    let per_long = 64usize / bits as usize;
    let expected = count.div_ceil(per_long);
    let data = data.context("multi-entry palette is missing packed data")?;
    ensure!(
        data.len() == expected,
        "packed palette has {} longs; expected {expected}",
        data.len()
    );
    let mask = (1u64 << bits) - 1;
    let mut indices = Vec::with_capacity(count);
    for i in 0..count {
        let word = data[i / per_long] as u64;
        let index = ((word >> ((i % per_long) * bits as usize)) & mask) as u32;
        ensure!(
            (index as usize) < palette_len,
            "palette index {index} is out of bounds"
        );
        indices.push(index);
    }
    Ok(indices)
}

/// Encode non-crossing 64-bit palette words. A singleton palette needs no data array.
pub fn encode_indices(
    palette_len: usize,
    indices: &[u32],
    min_bits: u32,
) -> Result<Option<Vec<i64>>> {
    ensure!(
        palette_len > 0 && palette_len <= indices.len(),
        "invalid palette length {palette_len}"
    );
    ensure!(
        indices.iter().all(|value| (*value as usize) < palette_len),
        "palette index is out of bounds"
    );
    if palette_len == 1 {
        return Ok(None);
    }
    let bits = (usize::BITS - (palette_len - 1).leading_zeros()).max(min_bits);
    ensure!(
        (1..=32).contains(&bits),
        "unsupported palette bit width {bits}"
    );
    let per_long = 64usize / bits as usize;
    let mut words = vec![0u64; indices.len().div_ceil(per_long)];
    for (index, value) in indices.iter().enumerate() {
        words[index / per_long] |= u64::from(*value) << ((index % per_long) * bits as usize);
    }
    Ok(Some(words.into_iter().map(|word| word as i64).collect()))
}

/// Java 26.3 stores default states as strings and non-default states as maps.
/// Mixed NBT lists wrap a non-compound item in a compound with the empty key.
pub(crate) fn unwrap_palette_entry(value: &Tag) -> &Tag {
    if let Tag::Compound(entries) = value
        && entries.len() == 1
        && entries[0].0.0.is_empty()
    {
        return &entries[0].1;
    }
    value
}

fn decode_block_state(value: &Tag, section_y: i8) -> Result<BlockState> {
    let value = unwrap_palette_entry(value);
    if let Tag::String(name) = value {
        return Ok(BlockState {
            name: name.clone(),
            properties: Vec::new(),
        });
    }
    let (name_key, properties_key) = if value.get("id").is_some() {
        ("id", "properties")
    } else {
        // Kept for read-only inspection of legacy-shaped synthetic data.
        ("Name", "Properties")
    };
    let name = match value.get(name_key) {
        Some(Tag::String(value)) => value.clone(),
        _ => bail!("section {section_y} contains a block state without an identifier"),
    };
    let properties = match value.get(properties_key) {
        None => Vec::new(),
        Some(Tag::Compound(values)) => values
            .iter()
            .map(|(key, value)| match value {
                Tag::String(value) => Ok((key.clone(), value.clone())),
                _ => bail!("block-state property is not a string"),
            })
            .collect::<Result<Vec<_>>>()?,
        _ => bail!("block-state properties are not a compound"),
    };
    Ok(BlockState { name, properties })
}

/// Reject ambiguous or legacy-shaped block states before a 26.3 chunk is written.
/// Read-only inspection remains permissive so older synthetic data can be examined.
pub(crate) fn validate_26_3_block_states(chunk: &Tag) -> Result<()> {
    fn unique_field<'a>(tag: &'a Tag, name: &str) -> Result<Option<&'a Tag>> {
        let Tag::Compound(entries) = tag else {
            bail!("{name} parent is not a compound");
        };
        let mut matching = entries.iter().filter(|(key, _)| key.matches(name));
        let value = matching.next().map(|(_, value)| value);
        ensure!(matching.next().is_none(), "duplicate {name} tags");
        Ok(value)
    }

    let sections = match unique_field(chunk, "sections")? {
        None => return Ok(()),
        Some(Tag::List { values, .. }) if values.is_empty() => return Ok(()),
        Some(Tag::List {
            element_type: TagType::Compound,
            values,
        }) => values,
        _ => bail!("chunk sections must be a compound list"),
    };
    for section in sections {
        unique_field(section, "Y")?;
        unique_field(section, "BlockLight")?;
        unique_field(section, "SkyLight")?;
        if let Some(biomes) = unique_field(section, "biomes")? {
            unique_field(biomes, "palette")?;
            unique_field(biomes, "data")?;
        }
        let Some(states) = unique_field(section, "block_states")? else {
            continue;
        };
        unique_field(states, "data")?;
        let (element_type, entries) = match unique_field(states, "palette")? {
            Some(Tag::List {
                element_type: element_type @ (TagType::String | TagType::Compound),
                values,
            }) => (*element_type, values),
            _ => bail!("block-state palette is not a supported list"),
        };
        for entry in entries {
            ensure!(entry.tag_type() == element_type, "mixed NBT list types");
            match unwrap_palette_entry(entry) {
                Tag::String(name) => ensure!(!name.0.is_empty(), "block state name is empty"),
                value @ Tag::Compound(_) => {
                    ensure!(
                        value.get("Name").is_none() && value.get("Properties").is_none(),
                        "legacy block-state format cannot be written as 26.3"
                    );
                    ensure!(
                        matches!(unique_field(value, "id")?, Some(Tag::String(name)) if !name.0.is_empty()),
                        "block state has no valid id"
                    );
                    match unique_field(value, "properties")? {
                        None => {}
                        Some(Tag::Compound(properties)) => {
                            let mut names = BTreeSet::new();
                            for (name, property) in properties {
                                ensure!(names.insert(&name.0), "duplicate block-state property");
                                ensure!(
                                    matches!(property, Tag::String(_)),
                                    "block-state property is not a string"
                                );
                            }
                        }
                        _ => bail!("block-state properties are not a compound"),
                    }
                }
                _ => bail!("invalid block-state palette entry"),
            }
        }
    }
    Ok(())
}

/// Decode every stored section, including sections that contain only light or biomes.
pub fn decode_sections(chunk: &Tag) -> Result<Vec<ChunkSection>> {
    let sections = match chunk.get("sections") {
        Some(Tag::List { values, .. }) if values.is_empty() => return Ok(Vec::new()),
        Some(Tag::List {
            element_type: TagType::Compound,
            values,
        }) => values,
        None => return Ok(Vec::new()),
        _ => bail!("chunk sections must be a compound list"),
    };
    let mut result = Vec::new();
    let mut section_ys = BTreeSet::new();
    for section in sections {
        let y = match section.get("Y") {
            Some(Tag::Byte(y)) => *y,
            _ => bail!("chunk section is missing byte Y"),
        };
        ensure!(section_ys.insert(y), "duplicate chunk section Y {y}");
        let blocks = match section.get("block_states") {
            None => None,
            Some(states) => {
                let palette = match states.get("palette") {
                    Some(Tag::List {
                        element_type: TagType::Compound | TagType::String,
                        values,
                    }) => values,
                    _ => bail!("section {y} has no block-state palette"),
                };
                let block_states = palette
                    .iter()
                    .map(|value| decode_block_state(value, y))
                    .collect::<Result<Vec<_>>>()?;
                let data = match states.get("data") {
                    Some(Tag::LongArray(value)) => Some(value.as_slice()),
                    None => None,
                    _ => bail!("section {y} block-state data is not a long array"),
                };
                let indices = decode_indices(block_states.len(), data, BLOCKS_PER_SECTION, 4)
                    .with_context(|| format!("invalid block-state storage in section {y}"))?;
                Some(BlockPalette {
                    states: block_states,
                    indices,
                })
            }
        };
        let biomes = match section.get("biomes") {
            None => None,
            Some(biomes) => {
                let names = match biomes.get("palette") {
                    Some(Tag::List {
                        element_type: TagType::String,
                        values,
                    }) => values
                        .iter()
                        .map(|value| match value {
                            Tag::String(value) => Ok(value.clone()),
                            _ => bail!("biome palette contains a non-string value"),
                        })
                        .collect::<Result<Vec<_>>>()?,
                    _ => bail!("section {y} has no biome palette"),
                };
                let data = match biomes.get("data") {
                    Some(Tag::LongArray(value)) => Some(value.as_slice()),
                    None => None,
                    _ => bail!("section {y} biome data is not a long array"),
                };
                let indices = decode_indices(names.len(), data, BIOMES_PER_SECTION, 1)
                    .with_context(|| format!("invalid biome storage in section {y}"))?;
                Some(BiomePalette { names, indices })
            }
        };
        let read_light = |name| -> Result<Option<Vec<u8>>> {
            match section.get(name) {
                None => Ok(None),
                Some(Tag::ByteArray(data)) if data.len() == 2048 => Ok(Some(data.clone())),
                _ => bail!("section {y} {name} must be a 2048-byte array"),
            }
        };
        result.push(ChunkSection {
            y,
            blocks,
            biomes,
            block_light: read_light("BlockLight")?,
            sky_light: read_light("SkyLight")?,
        });
    }
    Ok(result)
}

/// Compatibility helper for callers that only need sections with block states.
pub fn decode_block_sections(chunk: &Tag) -> Result<Vec<BlockSection>> {
    Ok(decode_sections(chunk)?
        .into_iter()
        .filter_map(|section| {
            section.blocks.map(|blocks| BlockSection {
                y: section.y,
                palette: blocks.states,
                indices: blocks.indices,
                biomes: section.biomes,
                block_light: section.block_light,
                sky_light: section.sky_light,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(values: &[u32], bits: usize) -> Vec<i64> {
        let per_long = 64 / bits;
        let mut words = vec![0u64; values.len().div_ceil(per_long)];
        for (index, value) in values.iter().enumerate() {
            words[index / per_long] |= u64::from(*value) << ((index % per_long) * bits);
        }
        words.into_iter().map(|word| word as i64).collect()
    }

    #[test]
    fn palette_widths_and_word_boundaries_round_trip() {
        for palette_len in [2usize, 16, 17, 32, 33, 256, 4096] {
            let bits = ((usize::BITS - (palette_len - 1).leading_zeros()) as usize).max(4);
            let expected: Vec<u32> = (0..BLOCKS_PER_SECTION)
                .map(|i| (i % palette_len) as u32)
                .collect();
            let packed = pack(&expected, bits);
            assert_eq!(
                encode_indices(palette_len, &expected, 4).unwrap(),
                Some(packed.clone())
            );
            assert_eq!(
                decode_indices(palette_len, Some(&packed), BLOCKS_PER_SECTION, 4).unwrap(),
                expected
            );
        }
        assert_eq!(
            decode_indices(1, None, BLOCKS_PER_SECTION, 4).unwrap(),
            vec![0; BLOCKS_PER_SECTION]
        );
        assert_eq!(
            encode_indices(1, &vec![0; BLOCKS_PER_SECTION], 4).unwrap(),
            None
        );
    }

    #[test]
    fn invalid_palette_data_is_not_silently_accepted() {
        assert!(decode_indices(2, None, BLOCKS_PER_SECTION, 4).is_err());
        let mut packed = vec![0i64; BLOCKS_PER_SECTION / 16];
        packed[0] = 0xf;
        assert!(decode_indices(2, Some(&packed), BLOCKS_PER_SECTION, 4).is_err());
        assert!(decode_indices(0, None, BLOCKS_PER_SECTION, 4).is_err());
    }
}
