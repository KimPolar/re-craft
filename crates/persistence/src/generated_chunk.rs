//! Assemble a fresh SPAWN-stage protochunk in memory, without saving a world.
use std::collections::BTreeSet;

use anyhow::{Context, Result, bail, ensure};
use native_resources::packs::ResourceId;
use native_world::loaded::ChunkPos;

use crate::{
    nbt::{self, NamedTag, Tag, TagType},
    palette, postprocessing,
    world::TARGET_DATA_VERSION,
};

/// Components generated against one unchanged registry and data-pack stack.
/// The caller must supply every feature-generated protochunk entity. This
/// structure does not establish that a chunk or world is safe to save.
pub struct SpawnChunkComponents {
    pub position: ChunkPos,
    pub min_y: i32,
    pub height: i32,
    pub last_update: i64,
    pub sections: Tag,
    pub heightmaps: Tag,
    pub block_ticks: Tag,
    pub fluid_ticks: Tag,
    pub postprocessing: Tag,
    pub block_entities: Tag,
    pub entities: Vec<Tag>,
}

/// Complete LevelChunk components prepared from a single generation pass.
/// Entity-region data and world-level save validation remain separate.
pub struct FullChunkComponents {
    pub position: ChunkPos,
    pub min_y: i32,
    pub height: i32,
    pub last_update: i64,
    pub sections: Tag,
    pub heightmaps: Tag,
    pub block_ticks: Tag,
    pub fluid_ticks: Tag,
    pub block_entities: Tag,
}

/// Assemble the entity-region document for already serialized entity records.
/// A caller must separately establish record completeness, ownership and the
/// correct empty-chunk storage action before saving.
pub fn assemble_entity_chunk(position: ChunkPos, records: &[Tag]) -> Result<NamedTag> {
    let entities = compound_list(
        Tag::List {
            element_type: TagType::Compound,
            values: records.to_vec(),
        },
        "entities",
    )?;
    let document = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![
            ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
            ("Entities".into(), entities),
            (
                "Position".into(),
                Tag::IntArray(vec![position.x, position.z]),
            ),
        ]),
    };
    crate::world::decode_entity_chunk(
        document.clone(),
        crate::world::ChunkPos {
            x: position.x,
            z: position.z,
        },
    )?;
    nbt::write(&document)?;
    Ok(document)
}

fn compound_list(tag: Tag, label: &str) -> Result<Tag> {
    let Tag::List {
        element_type,
        values,
    } = tag
    else {
        bail!("{label} must be a list")
    };
    ensure!(
        (values.is_empty() && matches!(element_type, TagType::End | TagType::Compound))
            || element_type == TagType::Compound,
        "{label} must contain compounds"
    );
    ensure!(
        values.iter().all(|item| matches!(item, Tag::Compound(_))),
        "{label} contains a non-compound"
    );
    Ok(Tag::List {
        element_type: if values.is_empty() {
            TagType::End
        } else {
            TagType::Compound
        },
        values,
    })
}

fn validate_positioned(
    tag: &Tag,
    label: &str,
    position: ChunkPos,
    bounds: Option<(i32, i32)>,
    full: bool,
) -> Result<()> {
    let Tag::List { values, .. } = tag else {
        unreachable!("validated compound list")
    };
    let mut seen = BTreeSet::new();
    for value in values {
        let Tag::Compound(fields) = value else {
            unreachable!("validated list element")
        };
        let one = |name: &str| -> Result<&Tag> {
            let mut entries = fields
                .iter()
                .filter(|(key, _)| key.matches(name))
                .map(|(_, value)| value);
            let entry = entries
                .next()
                .with_context(|| format!("{label} is missing {name}"))?;
            ensure!(entries.next().is_none(), "{label} has duplicate {name}");
            Ok(entry)
        };
        let coordinate = |key: &str| -> Result<i32> {
            match one(key)? {
                Tag::Int(value) => Ok(*value),
                _ => bail!("{label} has invalid {key}"),
            }
        };
        let p = [coordinate("x")?, coordinate("y")?, coordinate("z")?];
        ensure!(
            p[0].div_euclid(16) == position.x && p[2].div_euclid(16) == position.z,
            "{label} lies outside its chunk"
        );
        if let Some((min_y, max_y)) = bounds {
            ensure!(
                (min_y..max_y).contains(&p[1]),
                "{label} lies outside build height"
            );
        }
        let id_field = if bounds.is_some() { "id" } else { "i" };
        let Tag::String(id) = one(id_field)? else {
            bail!("{label} has invalid identifier")
        };
        let id = id.to_string_lossy();
        ResourceId::parse(&id)?;
        if bounds.is_some() {
            ensure!(
                seen.insert((p, String::new())),
                "duplicate block entity position"
            );
        } else {
            ensure!(full || seen.insert((p, id)), "duplicate generated tick");
            ensure!(
                (if full {
                    matches!(one("t")?, Tag::Int(_))
                } else {
                    matches!(one("t")?, Tag::Int(0))
                }) && matches!(one("p")?, Tag::Int(priority) if (-3..=3).contains(priority)),
                "invalid generated tick timing"
            );
        }
    }
    Ok(())
}

/// Assemble only the metadata and previously prepared components for a fresh
/// chunk with structures disabled. Lighting is not declared fully correct.
pub fn assemble_spawn_chunk(parts: SpawnChunkComponents) -> Result<NamedTag> {
    assemble_chunk(parts, false)
}

/// Assemble an in-memory FULL chunk without writing any world files. The
/// caller must independently establish lighting, entity and structure coverage.
pub fn assemble_full_chunk(parts: FullChunkComponents) -> Result<NamedTag> {
    let marks = native_core::postprocessing::PostProcessing::new(parts.min_y, parts.height)
        .context("invalid generated postprocessing bounds")?;
    assemble_full_chunk_with_postprocessing(parts, postprocessing::encode(&marks))
}

/// Assemble a FULL chunk whose post-generation callbacks have not all run.
/// The caller must capture the corresponding live blocks, ticks, block entities
/// and lighting at the same point; this alone does not authorize a world save.
pub fn assemble_full_chunk_with_postprocessing(
    parts: FullChunkComponents,
    postprocessing: Tag,
) -> Result<NamedTag> {
    assemble_chunk(
        SpawnChunkComponents {
            position: parts.position,
            min_y: parts.min_y,
            height: parts.height,
            last_update: parts.last_update,
            sections: parts.sections,
            heightmaps: parts.heightmaps,
            block_ticks: parts.block_ticks,
            fluid_ticks: parts.fluid_ticks,
            postprocessing,
            block_entities: parts.block_entities,
            entities: Vec::new(),
        },
        true,
    )
}

fn assemble_chunk(parts: SpawnChunkComponents, full: bool) -> Result<NamedTag> {
    let SpawnChunkComponents {
        position,
        min_y,
        height,
        last_update,
        sections,
        heightmaps,
        block_ticks,
        fluid_ticks,
        postprocessing,
        block_entities,
        entities,
    } = parts;
    ensure!(
        height > 0 && height % 16 == 0 && min_y.rem_euclid(16) == 0,
        "invalid generated chunk height"
    );
    let max_y = min_y
        .checked_add(height)
        .context("generated height overflow")?;
    ensure!(
        min_y / 16 >= i32::from(i8::MIN) && max_y / 16 - 1 <= i32::from(i8::MAX),
        "generated chunk height is unsupported"
    );
    let sections = compound_list(sections, "sections")?;
    let decoded =
        palette::decode_sections(&Tag::Compound(vec![("sections".into(), sections.clone())]))?;
    let first = min_y / 16;
    let end = max_y / 16;
    for sy in first..end {
        let section = decoded
            .iter()
            .find(|s| i32::from(s.y) == sy)
            .context("generated build section is missing")?;
        ensure!(
            section.blocks.is_some() && section.biomes.is_some(),
            "generated build section is incomplete"
        );
    }
    for section in &decoded {
        if !(first..end).contains(&i32::from(section.y)) {
            ensure!(
                section.blocks.is_none() && section.biomes.is_none(),
                "build data outside generation height"
            );
        }
    }
    let Tag::Compound(maps) = &heightmaps else {
        bail!("generated heightmaps must be a compound")
    };
    let mut names = BTreeSet::new();
    let bits = 32 - (height as u32).leading_zeros();
    let words = 256usize.div_ceil(64 / bits as usize);
    for (name, data) in maps {
        let key = name.to_string_lossy();
        ensure!(
            [
                "WORLD_SURFACE",
                "OCEAN_FLOOR",
                "MOTION_BLOCKING",
                "MOTION_BLOCKING_NO_LEAVES",
                "WORLD_SURFACE_WG",
                "OCEAN_FLOOR_WG"
            ]
            .contains(&key.as_str())
                && names.insert(key),
            "unknown or duplicate generated heightmap"
        );
        let Tag::LongArray(values) = data else {
            bail!("invalid generated heightmap storage")
        };
        ensure!(values.len() == words, "invalid generated heightmap storage");
        let per_word = 64 / bits as usize;
        let mask = (1u64 << bits) - 1;
        for index in 0..256 {
            let relative =
                ((values[index / per_word] as u64) >> ((index % per_word) * bits as usize)) & mask;
            ensure!(
                relative <= height as u64,
                "generated heightmap value is outside its bounds"
            );
        }
    }
    if full {
        ensure!(
            names
                == [
                    "WORLD_SURFACE",
                    "OCEAN_FLOOR",
                    "MOTION_BLOCKING",
                    "MOTION_BLOCKING_NO_LEAVES"
                ]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            "FULL chunk must contain exactly four heightmaps"
        );
    }
    postprocessing::decode(&postprocessing, min_y, height)?;
    let block_ticks = compound_list(block_ticks, "block ticks")?;
    let fluid_ticks = compound_list(fluid_ticks, "fluid ticks")?;
    let block_entities = compound_list(block_entities, "block entities")?;
    validate_positioned(&block_ticks, "block tick", position, None, full)?;
    validate_positioned(&fluid_ticks, "fluid tick", position, None, full)?;
    validate_positioned(
        &block_entities,
        "block entity",
        position,
        Some((min_y, max_y)),
        full,
    )?;
    let entities = compound_list(
        Tag::List {
            element_type: TagType::Compound,
            values: entities,
        },
        "entities",
    )?;
    let mut fields = vec![
        ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
        ("xPos".into(), Tag::Int(position.x)),
        ("yPos".into(), Tag::Int(first)),
        ("zPos".into(), Tag::Int(position.z)),
        ("LastUpdate".into(), Tag::Long(last_update)),
        ("InhabitedTime".into(), Tag::Long(0)),
        (
            "Status".into(),
            Tag::String(
                if full {
                    "minecraft:full"
                } else {
                    "minecraft:spawn"
                }
                .into(),
            ),
        ),
        ("sections".into(), sections),
        ("block_entities".into(), block_entities),
    ];
    if full {
        fields.push(("isLightOn".into(), Tag::Byte(1)));
    } else {
        fields.push(("entities".into(), entities));
    }
    fields.extend(vec![
        ("block_ticks".into(), block_ticks),
        ("fluid_ticks".into(), fluid_ticks),
        ("PostProcessing".into(), postprocessing),
        ("Heightmaps".into(), heightmaps),
        (
            "structures".into(),
            Tag::Compound(vec![
                ("starts".into(), Tag::Compound(vec![])),
                ("References".into(), Tag::Compound(vec![])),
            ]),
        ),
    ]);
    let result = NamedTag {
        name: "".into(),
        value: Tag::Compound(fields),
    };
    // Check NBT representability and size before exposing the draft.
    nbt::write(&result)?;
    Ok(result)
}
