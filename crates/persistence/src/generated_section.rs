//! Encode generated section data, without claiming a chunk is ready to save.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use native_resources::packs::ResourceId;
use native_world::loaded::{BiomeId, BlockState, BlockStateId, ChunkPos, World};

use crate::{
    nbt::{Tag, TagType},
    palette,
};

/// Build only the `sections` field of a fresh chunk. This does not serialize
/// ticks, block entities, heightmaps, structures, entities, or chunk status.
/// Inputs must use the same immutable registry and biome data-pack stack that
/// generated the chunk. Every build-height section must be complete.
pub fn encode_generated_sections(
    world: &World,
    pos: ChunkPos,
    min_y: i32,
    height: i32,
    has_skylight: bool,
    mut valid_block: impl FnMut(&BlockState) -> bool,
    mut known_biome: impl FnMut(&str) -> bool,
) -> Result<Tag> {
    ensure!(
        height > 0 && height % 16 == 0 && min_y % 16 == 0,
        "invalid generation height"
    );
    let end = min_y
        .checked_add(height)
        .context("generation height overflow")?;
    let first = min_y.div_euclid(16);
    let last = end.div_euclid(16);
    ensure!(
        first >= i32::from(i8::MIN) && last - 1 <= i32::from(i8::MAX),
        "section height outside NBT byte range"
    );
    let chunk = world
        .chunks
        .get(&pos)
        .context("generated chunk is missing")?;
    ensure!(chunk.position == pos, "generated chunk position differs");
    for y in first..last {
        let section = chunk
            .sections
            .get(&(y as i8))
            .context("generated build section is missing")?;
        ensure!(
            section.blocks.is_some() && section.biomes.is_some(),
            "generated section data is incomplete"
        );
    }
    let mut sections = Vec::with_capacity(chunk.sections.len());
    for (&y, section) in &chunk.sections {
        let build = (first..last).contains(&i32::from(y));
        ensure!(
            build || (section.blocks.is_none() && section.biomes.is_none()),
            "block or biome data outside generation height"
        );
        if !build && section.block_light.is_none() && (!has_skylight || section.sky_light.is_none())
        {
            continue;
        }
        let mut fields = vec![("Y".into(), Tag::Byte(y))];
        if let Some(blocks) = &section.blocks {
            let mut lookup = BTreeMap::<BlockStateId, u32>::new();
            let mut values = Vec::new();
            let mut indices = Vec::with_capacity(blocks.len());
            for &id in blocks.iter() {
                let index = if let Some(index) = lookup.get(&id) {
                    *index
                } else {
                    let state = world
                        .state(id)
                        .context("unknown generated block-state ID")?;
                    ResourceId::parse(&state.name)?;
                    ensure!(
                        valid_block(state),
                        "unknown generated block state {}",
                        state.name
                    );
                    let mut state_fields =
                        vec![("id".into(), Tag::String(state.name.as_str().into()))];
                    if !state.properties.is_empty() {
                        state_fields.push((
                            "properties".into(),
                            Tag::Compound(
                                state
                                    .properties
                                    .iter()
                                    .map(|(key, value)| {
                                        (key.as_str().into(), Tag::String(value.as_str().into()))
                                    })
                                    .collect(),
                            ),
                        ));
                    }
                    let index = u32::try_from(values.len())?;
                    values.push(Tag::Compound(state_fields));
                    lookup.insert(id, index);
                    index
                };
                indices.push(index);
            }
            let mut storage = vec![(
                "palette".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values,
                },
            )];
            if let Some(data) = palette::encode_indices(lookup.len(), &indices, 4)? {
                storage.push(("data".into(), Tag::LongArray(data)));
            }
            fields.push(("block_states".into(), Tag::Compound(storage)));
        }
        if let Some(biomes) = &section.biomes {
            let mut lookup = BTreeMap::<BiomeId, u32>::new();
            let mut values = Vec::new();
            let mut indices = Vec::with_capacity(biomes.len());
            for &id in biomes.iter() {
                let index = if let Some(index) = lookup.get(&id) {
                    *index
                } else {
                    let name = world
                        .biome_by_id(id)
                        .context("unknown generated biome ID")?;
                    ResourceId::parse(name)?;
                    ensure!(known_biome(name), "unknown generated biome {name}");
                    let index = u32::try_from(values.len())?;
                    values.push(Tag::String(name.into()));
                    lookup.insert(id, index);
                    index
                };
                indices.push(index);
            }
            let mut storage = vec![(
                "palette".into(),
                Tag::List {
                    element_type: TagType::String,
                    values,
                },
            )];
            if let Some(data) = palette::encode_indices(lookup.len(), &indices, 1)? {
                storage.push(("data".into(), Tag::LongArray(data)));
            }
            fields.push(("biomes".into(), Tag::Compound(storage)));
        }
        if let Some(light) = &section.block_light {
            fields.push(("BlockLight".into(), Tag::ByteArray(light.to_vec())));
        }
        if has_skylight && let Some(light) = &section.sky_light {
            fields.push(("SkyLight".into(), Tag::ByteArray(light.to_vec())));
        }
        sections.push(Tag::Compound(fields));
    }
    Ok(Tag::List {
        element_type: TagType::Compound,
        values: sections,
    })
}

/// Merge generated block storage into captured biome-stage sections without
/// replacing their biome palettes, lighting, or unrecognized section fields.
/// This returns one intermediate NBT component, not a writable chunk draft.
pub fn merge_biome_stage_blocks(source: &Tag, generated: &Tag) -> Result<Tag> {
    let wrap = |sections: &Tag| Tag::Compound(vec![("sections".into(), sections.clone())]);
    let source_container = wrap(source);
    let generated_container = wrap(generated);
    palette::validate_26_3_block_states(&source_container)?;
    palette::validate_26_3_block_states(&generated_container)?;
    let original = palette::decode_sections(&source_container)?;
    let replacement = palette::decode_sections(&generated_container)?;
    ensure!(!replacement.is_empty(), "no generated sections");
    let original: BTreeMap<_, _> = original
        .into_iter()
        .map(|section| (section.y, section))
        .collect();
    for section in &replacement {
        let saved = original
            .get(&section.y)
            .context("generated section has no saved biome section")?;
        if let Some(blocks) = &saved.blocks {
            ensure!(
                blocks
                    .indices
                    .iter()
                    .all(
                        |&index| blocks.states.get(index as usize).is_some_and(|state| {
                            state.name.matches("minecraft:air") && state.properties.is_empty()
                        })
                    ),
                "saved biome section contains blocks"
            );
        }
        ensure!(section.blocks.is_some(), "generated section has no blocks");
        let old = saved
            .biomes
            .as_ref()
            .context("saved section has no biomes")?;
        let new = section
            .biomes
            .as_ref()
            .context("generated section has no biomes")?;
        ensure!(
            old.indices.len() == new.indices.len(),
            "generated biome volume differs"
        );
        for (&old_index, &new_index) in old.indices.iter().zip(&new.indices) {
            let old_name = old
                .names
                .get(old_index as usize)
                .context("invalid saved biome index")?;
            let new_name = new
                .names
                .get(new_index as usize)
                .context("invalid generated biome index")?;
            let old_id = ResourceId::parse(
                &String::from_utf16(&old_name.0).context("invalid saved biome ID")?,
            )?;
            let new_id = ResourceId::parse(
                &String::from_utf16(&new_name.0).context("invalid generated biome ID")?,
            )?;
            ensure!(old_id == new_id, "generated biome differs from saved biome");
        }
    }
    let Tag::List {
        element_type: TagType::Compound,
        values: generated_values,
    } = generated
    else {
        anyhow::bail!("generated sections must be a compound list")
    };
    let Tag::List {
        element_type: TagType::Compound,
        values: source_values,
    } = source
    else {
        anyhow::bail!("saved sections must be a compound list")
    };
    let fields = |section: &Tag| -> Result<(i8, Tag)> {
        let Tag::Compound(fields) = section else {
            anyhow::bail!("invalid generated section")
        };
        let mut ys = fields.iter().filter(|(key, _)| key.matches("Y"));
        let Some((_, Tag::Byte(y))) = ys.next() else {
            anyhow::bail!("missing generated section Y")
        };
        ensure!(ys.next().is_none(), "duplicate generated section Y");
        let mut block_fields = fields.iter().filter(|(key, _)| key.matches("block_states"));
        let blocks = block_fields
            .next()
            .context("generated block states missing")?
            .1
            .clone();
        ensure!(
            block_fields.next().is_none(),
            "duplicate generated block states"
        );
        ensure!(
            fields
                .iter()
                .filter(|(key, _)| key.matches("biomes"))
                .count()
                == 1,
            "invalid generated biomes"
        );
        Ok((*y, blocks))
    };
    let mut replacements = BTreeMap::new();
    for section in generated_values {
        let (y, blocks) = fields(section)?;
        ensure!(
            replacements.insert(y, blocks).is_none(),
            "duplicate generated section"
        );
    }
    let mut seen = BTreeSet::new();
    let mut output = source_values.clone();
    for section in &mut output {
        let Tag::Compound(fields) = section else {
            anyhow::bail!("invalid saved section")
        };
        let mut ys = fields.iter().filter(|(key, _)| key.matches("Y"));
        let Some((_, Tag::Byte(y))) = ys.next() else {
            anyhow::bail!("missing saved section Y")
        };
        ensure!(
            ys.next().is_none() && seen.insert(*y),
            "duplicate saved section Y"
        );
        let y = *y;
        ensure!(
            fields
                .iter()
                .filter(|(key, _)| key.matches("biomes"))
                .count()
                <= 1,
            "duplicate saved biomes"
        );
        let positions: Vec<_> = fields
            .iter()
            .enumerate()
            .filter(|(_, (key, _))| key.matches("block_states"))
            .map(|(index, _)| index)
            .collect();
        ensure!(positions.len() <= 1, "duplicate saved block states");
        if let Some(blocks) = replacements.remove(&y) {
            if let Some(&index) = positions.first() {
                fields[index].1 = blocks;
            } else {
                fields.push(("block_states".into(), blocks));
            }
        }
    }
    ensure!(
        replacements.is_empty(),
        "generated sections are not covered by the saved chunk"
    );
    Ok(Tag::List {
        element_type: TagType::Compound,
        values: output,
    })
}

/// Attach a complete visible column to intermediate sections while preserving
/// every other section field. This neither advances status nor permits saving.
/// Existing light is rejected: it cannot be merged with newly propagated data.
pub fn merge_generated_light_sections(
    source: &Tag,
    light: &native_world::generation_light::VisibleLightSnapshot,
    position: ChunkPos,
    min_y: i32,
    height: i32,
    has_skylight: bool,
) -> Result<Tag> {
    ensure!(
        height > 0 && height % 16 == 0 && min_y % 16 == 0,
        "invalid light height"
    );
    ensure!(
        light.has_block_channel() && light.has_sky_channel() == has_skylight,
        "light channels differ from dimension"
    );
    let end = min_y.checked_add(height).context("light height overflow")?;
    let min_section = min_y
        .div_euclid(16)
        .checked_sub(1)
        .context("light section underflow")?;
    let max_section = end.div_euclid(16);
    ensure!(
        i8::try_from(min_section).is_ok() && i8::try_from(max_section).is_ok(),
        "light section outside NBT byte range"
    );
    let column = light
        .complete_column(position)
        .context("light column is missing")?;
    ensure!(
        column
            .block_layers()
            .keys()
            .chain(column.sky_layers().keys())
            .all(|y| (min_section..=max_section).contains(y)),
        "light layer outside generation bounds"
    );
    ensure!(
        has_skylight || column.sky_layers().is_empty(),
        "unexpected sky light"
    );
    let Tag::List {
        element_type: TagType::Compound,
        values,
    } = source
    else {
        anyhow::bail!("saved sections must be a compound list")
    };
    let mut output = values.clone();
    let mut indexes = BTreeMap::new();
    for (index, section) in output.iter().enumerate() {
        let Tag::Compound(fields) = section else {
            anyhow::bail!("invalid saved section")
        };
        let mut ys = fields.iter().filter(|(key, _)| key.matches("Y"));
        let Some((_, Tag::Byte(y))) = ys.next() else {
            anyhow::bail!("saved section Y missing")
        };
        ensure!(
            ys.next().is_none() && indexes.insert(i32::from(*y), index).is_none(),
            "duplicate saved section Y"
        );
        ensure!(
            fields
                .iter()
                .all(|(key, _)| !key.matches("BlockLight") && !key.matches("SkyLight")),
            "saved light cannot be merged"
        );
    }
    let ys: BTreeSet<_> = column
        .block_layers()
        .keys()
        .chain(column.sky_layers().keys())
        .copied()
        .collect();
    for y in ys {
        let index = if let Some(&index) = indexes.get(&y) {
            index
        } else {
            let index = output.len();
            output.push(Tag::Compound(vec![(
                "Y".into(),
                Tag::Byte(i8::try_from(y)?),
            )]));
            index
        };
        let Tag::Compound(fields) = &mut output[index] else {
            unreachable!()
        };
        if let Some(layer) = column.block_layers().get(&y) {
            fields.push(("BlockLight".into(), Tag::ByteArray(layer.as_ref().to_vec())));
        }
        if let Some(layer) = column.sky_layers().get(&y) {
            fields.push(("SkyLight".into(), Tag::ByteArray(layer.as_ref().to_vec())));
        }
    }
    Ok(Tag::List {
        element_type: TagType::Compound,
        values: output,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(y: i8, block: Option<&str>, biome: &str) -> Tag {
        let mut fields = vec![
            ("Y".into(), Tag::Byte(y)),
            (
                "biomes".into(),
                Tag::Compound(vec![(
                    "palette".into(),
                    Tag::List {
                        element_type: TagType::String,
                        values: vec![Tag::String(biome.into())],
                    },
                )]),
            ),
            ("custom".into(), Tag::Int(7)),
        ];
        if let Some(block) = block {
            fields.push((
                "block_states".into(),
                Tag::Compound(vec![(
                    "palette".into(),
                    Tag::List {
                        element_type: TagType::String,
                        values: vec![Tag::String(block.into())],
                    },
                )]),
            ));
        }
        Tag::Compound(fields)
    }
    fn list(values: Vec<Tag>) -> Tag {
        Tag::List {
            element_type: TagType::Compound,
            values,
        }
    }

    #[test]
    fn light_merge_preserves_sections_and_rejects_ambiguous_input() {
        use native_world::generation_light::{
            VisibleLayers, VisibleLightColumn, VisibleLightSnapshot,
        };
        use std::sync::Arc;
        let pos = ChunkPos { x: 3, z: -1 };
        let source = list(vec![section(
            0,
            Some("minecraft:stone"),
            "minecraft:plains",
        )]);
        let mut block = VisibleLayers::new();
        block.insert(0, Arc::new([0x12; native_world::loaded::LIGHT_BYTES]));
        let mut sky = VisibleLayers::new();
        sky.insert(1, Arc::new([0x34; native_world::loaded::LIGHT_BYTES]));
        let mut light = VisibleLightSnapshot::new(true, true);
        light.insert_complete_column(pos, VisibleLightColumn::new(block, sky));
        let merged = merge_generated_light_sections(&source, &light, pos, 0, 16, true).unwrap();
        let Tag::List { values, .. } = &merged else {
            panic!()
        };
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].get("custom"), Some(&Tag::Int(7)));
        assert_eq!(
            values[0].get("BlockLight"),
            Some(&Tag::ByteArray(vec![
                0x12;
                native_world::loaded::LIGHT_BYTES
            ]))
        );
        assert_eq!(values[1].get("Y"), Some(&Tag::Byte(1)));
        assert_eq!(
            values[1].get("SkyLight"),
            Some(&Tag::ByteArray(vec![
                0x34;
                native_world::loaded::LIGHT_BYTES
            ]))
        );
        assert!(merge_generated_light_sections(&merged, &light, pos, 0, 16, true).is_err());
        assert!(
            merge_generated_light_sections(&source, &light, ChunkPos { x: 0, z: 0 }, 0, 16, true)
                .is_err()
        );
        assert!(merge_generated_light_sections(&source, &light, pos, 0, 16, false).is_err());
        assert!(
            merge_generated_light_sections(
                &list(vec![section(0, None, "plains"), section(0, None, "plains")]),
                &light,
                pos,
                0,
                16,
                true
            )
            .is_err()
        );
        let mut outside = VisibleLayers::new();
        outside.insert(2, Arc::new([0; native_world::loaded::LIGHT_BYTES]));
        let mut invalid = VisibleLightSnapshot::new(true, true);
        invalid.insert_complete_column(pos, VisibleLightColumn::new(outside, VisibleLayers::new()));
        assert!(merge_generated_light_sections(&source, &invalid, pos, 0, 16, true).is_err());
    }

    #[test]
    fn biome_section_merge_preserves_metadata_and_rejects_lossy_inputs() {
        let source = list(vec![
            section(-1, None, "plains"),
            section(0, Some("minecraft:air"), "minecraft:forest"),
        ]);
        let generated = list(vec![
            section(-1, Some("minecraft:stone"), "minecraft:plains"),
            section(0, Some("minecraft:dirt"), "forest"),
        ]);
        let merged = merge_biome_stage_blocks(&source, &generated).unwrap();
        let Tag::List { values, .. } = &merged else {
            panic!()
        };
        assert_eq!(values[0].get("custom"), Some(&Tag::Int(7)));
        let Tag::List {
            values: source_values,
            ..
        } = &source
        else {
            panic!()
        };
        assert_eq!(values[0].get("biomes"), source_values[0].get("biomes"));
        assert!(source_values[0].get("block_states").is_none());
        let wrapped = Tag::Compound(vec![("sections".into(), merged)]);
        let decoded = palette::decode_sections(&wrapped).unwrap();
        assert!(
            decoded[0]
                .blocks
                .as_ref()
                .unwrap()
                .block(0, 0, 0)
                .unwrap()
                .name
                .matches("minecraft:stone")
        );
        assert!(
            decoded[1]
                .blocks
                .as_ref()
                .unwrap()
                .block(15, 15, 15)
                .unwrap()
                .name
                .matches("minecraft:dirt")
        );
        assert!(
            merge_biome_stage_blocks(
                &list(vec![section(-1, Some("minecraft:stone"), "plains")]),
                &list(vec![section(-1, Some("minecraft:dirt"), "plains")])
            )
            .is_err()
        );
        assert!(
            merge_biome_stage_blocks(
                &source,
                &list(vec![section(-1, Some("minecraft:stone"), "desert")])
            )
            .is_err()
        );
        assert!(
            merge_biome_stage_blocks(
                &source,
                &list(vec![section(1, Some("minecraft:stone"), "plains")])
            )
            .is_err()
        );
        assert!(
            merge_biome_stage_blocks(
                &list(vec![
                    section(-1, None, "plains"),
                    section(-1, None, "plains")
                ]),
                &generated
            )
            .is_err()
        );
        let mut ambiguous = section(-1, Some("minecraft:air"), "plains");
        let Tag::Compound(fields) = &mut ambiguous else {
            unreachable!()
        };
        let Tag::Compound(states) = &mut fields
            .iter_mut()
            .find(|(key, _)| key.matches("block_states"))
            .unwrap()
            .1
        else {
            unreachable!()
        };
        states.push((
            "palette".into(),
            Tag::List {
                element_type: TagType::String,
                values: vec![Tag::String("minecraft:stone".into())],
            },
        ));
        assert!(merge_biome_stage_blocks(&list(vec![ambiguous]), &generated).is_err());
    }
}
