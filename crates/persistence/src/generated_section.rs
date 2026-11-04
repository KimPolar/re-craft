//! Encode generated section data, without claiming a chunk is ready to save.
use std::collections::BTreeMap;

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
