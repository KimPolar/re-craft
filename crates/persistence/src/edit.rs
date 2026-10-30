//! In-memory block-state drafts for existing 26.3 chunks.
//!
//! A draft preserves the source NBT outside the edited block-state storage, but
//! does not automatically update lighting, heightmaps, entities, scheduled ticks
//! or neighbors. Heightmaps can be rebuilt separately; callers must verify other
//! dependent data before passing a draft to the guarded persistence API.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail, ensure};

use crate::{
    nbt::{JavaString, NamedTag, Tag, TagType},
    palette::{self, BLOCKS_PER_SECTION, BlockState},
    world::ChunkPos,
};

#[derive(Clone, Debug)]
pub struct BlockChange {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// Prevent a stale edit from silently replacing a different block.
    pub expected: BlockState,
    pub replacement: BlockState,
}

/// In-memory block-entity edit with an exact original value for stale checks.
#[derive(Clone, Debug)]
pub struct BlockEntityChange {
    pub expected: Tag,
    pub replacement: Tag,
}

/// One saved light layer, with an exact pre-edit source for stale-data checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LightLayerChange {
    pub expected: Option<Box<[u8; native_world::loaded::LIGHT_BYTES]>>,
    pub replacement: Option<Box<[u8; native_world::loaded::LIGHT_BYTES]>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionLightChange {
    pub y: i8,
    pub block: Option<LightLayerChange>,
    pub sky: Option<LightLayerChange>,
}

#[derive(Debug)]
pub struct ChunkDraft {
    source_nbt: Vec<u8>,
    document: NamedTag,
    position: ChunkPos,
    heightmaps_rebuilt: bool,
    changed_block_states: Vec<BlockChange>,
    uncoordinated_block_entities: bool,
    pub changed_blocks: usize,
    pub changed_sections: BTreeSet<i8>,
}

impl ChunkDraft {
    /// This is an incomplete draft and must not yet be saved as a playable chunk.
    pub fn document(&self) -> &NamedTag {
        &self.document
    }

    pub(crate) fn source_nbt(&self) -> &[u8] {
        &self.source_nbt
    }

    pub(crate) fn position(&self) -> ChunkPos {
        self.position
    }

    /// Check the invariants that persistence can verify, then require the
    /// active game registry to approve every changed block state. That check
    /// must reject any light, block-entity, fluid, neighbor, tick, or POI effect
    /// that this draft does not also represent.
    pub(crate) fn validate_for_write(
        &self,
        mut validate_change: impl FnMut(&BlockChange) -> Result<()>,
    ) -> Result<()> {
        ensure!(
            self.changed_blocks == self.changed_block_states.len(),
            "chunk draft change count is inconsistent"
        );
        ensure!(
            !self.uncoordinated_block_entities,
            "block entity edits require a coordinated world save"
        );
        ensure!(
            self.changed_blocks == 0 || self.heightmaps_rebuilt,
            "changed chunk blocks require rebuilt heightmaps"
        );
        self.validate_block_entity_dependencies()?;
        for change in &self.changed_block_states {
            validate_change(change)?;
        }
        Ok(())
    }

    /// Changes at a saved block entity require coordinated state, removal and
    /// drop handling, which the session does not yet provide.
    fn validate_block_entity_dependencies(&self) -> Result<()> {
        let changed: BTreeSet<_> = self
            .changed_block_states
            .iter()
            .map(|change| (change.x, change.y, change.z))
            .collect();
        if changed.is_empty() {
            return Ok(());
        }
        let source = crate::nbt::read(&self.source_nbt)?;
        let Some(list) = unique_field(&source.value, "block_entities")? else {
            return Ok(());
        };
        let Tag::List {
            element_type,
            values,
        } = list
        else {
            bail!("block_entities must be a list")
        };
        ensure!(
            *element_type == TagType::Compound || values.is_empty(),
            "block_entities must be a compound list"
        );
        let mut positions = BTreeSet::new();
        for entity in values {
            let coordinate = |name: &str| -> Result<i32> {
                match unique_field(entity, name)? {
                    Some(Tag::Int(value)) => Ok(*value),
                    _ => bail!("block entity has an invalid {name} coordinate"),
                }
            };
            let position = (coordinate("x")?, coordinate("y")?, coordinate("z")?);
            ensure!(
                position.0.div_euclid(16) == self.position.x
                    && position.2.div_euclid(16) == self.position.z,
                "block entity is outside its chunk"
            );
            ensure!(
                positions.insert(position),
                "duplicate block entity position"
            );
            ensure!(
                !changed.contains(&position),
                "changed block has an uncoordinated block entity"
            );
        }
        Ok(())
    }

    /// Any eventual coordinated write must also compare the raw file bytes.
    pub fn matches_source(&self, current: &NamedTag) -> Result<bool> {
        Ok(crate::nbt::write(current)? == self.source_nbt)
    }

    /// Stage existing block-entity changes atomically. This does not authorize
    /// a durable write: player inventories and other side effects must be
    /// committed in the same transaction first.
    pub fn replace_block_entities(&mut self, changes: &[BlockEntityChange]) -> Result<()> {
        if changes.is_empty() {
            return Ok(());
        }
        let position = |entity: &Tag| -> Result<(i32, i32, i32)> {
            let coordinate = |name| match unique_field(entity, name)? {
                Some(Tag::Int(value)) => Ok(*value),
                _ => bail!("block entity has an invalid {name} coordinate"),
            };
            Ok((coordinate("x")?, coordinate("y")?, coordinate("z")?))
        };
        let mut replacements = BTreeMap::new();
        for change in changes {
            let pos = position(&change.expected)?;
            ensure!(
                position(&change.replacement)? == pos,
                "block entity coordinates changed"
            );
            ensure!(
                pos.0.div_euclid(16) == self.position.x && pos.2.div_euclid(16) == self.position.z,
                "block entity is outside its chunk"
            );
            let id = unique_field(&change.expected, "id")?.context("missing block entity id")?;
            ensure!(
                matches!(id, Tag::String(_))
                    && unique_field(&change.replacement, "id")? == Some(id),
                "block entity id changed"
            );
            ensure!(
                !self
                    .changed_block_states
                    .iter()
                    .any(|block| (block.x, block.y, block.z) == pos),
                "block entity block changed"
            );
            ensure!(
                replacements.insert(pos, change).is_none(),
                "duplicate block entity edit"
            );
        }
        let mut document = self.document.clone();
        let Some(Tag::List {
            element_type,
            values,
        }) = unique_field_mut(&mut document.value, "block_entities")?
        else {
            bail!("chunk block entities are unavailable")
        };
        ensure!(
            *element_type == TagType::Compound,
            "block entities must be a compound list"
        );
        let mut positions = BTreeSet::new();
        for entity in values.iter_mut() {
            let pos = position(entity)?;
            ensure!(
                pos.0.div_euclid(16) == self.position.x && pos.2.div_euclid(16) == self.position.z,
                "block entity is outside its chunk"
            );
            ensure!(positions.insert(pos), "duplicate block entity position");
            if let Some(change) = replacements.remove(&pos) {
                ensure!(
                    *entity == change.expected,
                    "block entity changed before edit"
                );
                *entity = change.replacement.clone();
            }
        }
        ensure!(replacements.is_empty(), "edited block entity is missing");
        self.document = document;
        self.uncoordinated_block_entities = true;
        Ok(())
    }

    /// Apply known layer changes atomically while preserving all other NBT.
    /// The caller must prove that relighting has completed across chunk borders;
    /// this partial draft alone does not authorize a gameplay save.
    pub fn replace_section_lights(&mut self, changes: &[SectionLightChange]) -> Result<()> {
        if changes.is_empty() {
            return Ok(());
        }
        let source = crate::nbt::read(&self.source_nbt)?;
        ensure!(
            matches!(
                unique_field(&source.value, "isLightOn")?,
                Some(Tag::Byte(1))
            ),
            "source light is not known complete"
        );
        let decoded = palette::decode_sections(&source.value)?;
        let mut ys = BTreeSet::new();
        for change in changes {
            ensure!(ys.insert(change.y), "duplicate light section");
            ensure!(
                change.block.is_some() || change.sky.is_some(),
                "empty light section change"
            );
            let old = decoded.iter().find(|section| section.y == change.y);
            for (layer, expected) in [
                (&change.block, old.and_then(|s| s.block_light.as_deref())),
                (&change.sky, old.and_then(|s| s.sky_light.as_deref())),
            ] {
                if let Some(layer) = layer {
                    ensure!(
                        layer.expected.as_deref().map(|data| data.as_slice()) == expected,
                        "source light layer differs"
                    );
                }
            }
        }
        let mut document = self.document.clone();
        if unique_field(&document.value, "sections")?.is_none() {
            let Tag::Compound(fields) = &mut document.value else {
                bail!("chunk must be a compound")
            };
            fields.push((
                "sections".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: Vec::new(),
                },
            ));
        }
        let Some(Tag::List {
            element_type,
            values,
        }) = unique_field_mut(&mut document.value, "sections")?
        else {
            bail!("chunk sections must be a list")
        };
        if values.is_empty() {
            *element_type = TagType::Compound;
        }
        ensure!(
            *element_type == TagType::Compound,
            "chunk sections must be a compound list"
        );
        for change in changes {
            let index = match values.iter().position(
                |section| matches!(section.get("Y"), Some(Tag::Byte(y)) if *y == change.y),
            ) {
                Some(index) => index,
                None => {
                    values.push(Tag::Compound(vec![("Y".into(), Tag::Byte(change.y))]));
                    values.len() - 1
                }
            };
            for (name, layer) in [("BlockLight", &change.block), ("SkyLight", &change.sky)] {
                let Some(layer) = layer else { continue };
                let Tag::Compound(fields) = &mut values[index] else {
                    bail!("section must be a compound")
                };
                ensure!(
                    fields.iter().filter(|(key, _)| key.matches(name)).count() <= 1,
                    "duplicate light layer"
                );
                if let Some(field) = fields.iter().position(|(key, _)| key.matches(name)) {
                    if let Some(value) = &layer.replacement {
                        fields[field].1 = Tag::ByteArray(value.to_vec());
                    } else {
                        fields.remove(field);
                    }
                } else if let Some(value) = &layer.replacement {
                    fields.push((name.into(), Tag::ByteArray(value.to_vec())));
                }
            }
        }
        crate::nbt::write(&document).context("cannot encode light draft")?;
        self.document = document;
        Ok(())
    }

    /// Replace only fully understood falling-block ticks in this draft.
    /// Refuses a source chunk containing other block-tick kinds.
    pub fn replace_falling_block_ticks(
        &mut self,
        ticks: &[native_world::loaded::SavedBlockTick],
    ) -> Result<()> {
        self.replace_block_ticks_inner(ticks, false)
    }

    /// Replace supported falling-block, button, lamp, and soil ticks.
    /// Other chunk data, including lighting and entities, remains unvalidated.
    pub fn replace_block_ticks(
        &mut self,
        ticks: &[native_world::loaded::SavedBlockTick],
    ) -> Result<()> {
        self.replace_block_ticks_inner(ticks, true)
    }

    fn replace_block_ticks_inner(
        &mut self,
        ticks: &[native_world::loaded::SavedBlockTick],
        include_simulated: bool,
    ) -> Result<()> {
        let supported = |name: &str| {
            matches!(
                name,
                "minecraft:sand"
                    | "sand"
                    | "minecraft:red_sand"
                    | "red_sand"
                    | "minecraft:gravel"
                    | "gravel"
            ) || (include_simulated
                && (native_core::button::Button::from_id(name).is_some()
                    || matches!(
                        name,
                        "minecraft:redstone_lamp"
                            | "redstone_lamp"
                            | "minecraft:dirt_path"
                            | "dirt_path"
                            | "minecraft:farmland"
                            | "farmland"
                    )))
        };
        let source = crate::nbt::read(&self.source_nbt)?;
        if let Some(list) = unique_field(&source.value, "block_ticks")? {
            let Tag::List {
                element_type,
                values,
            } = list
            else {
                bail!("block_ticks must be a list")
            };
            ensure!(
                *element_type == TagType::Compound || values.is_empty(),
                "unsupported block tick list type"
            );
            for entry in values {
                let Tag::Compound(fields) = entry else {
                    bail!("block tick must be a compound")
                };
                ensure!(
                    fields.len() == 6,
                    "block tick has unsupported or missing fields"
                );
                for field in ["i", "x", "y", "z", "t", "p"] {
                    ensure!(
                        unique_field(entry, field)?.is_some(),
                        "block tick is missing {field}"
                    );
                }
                let Some(Tag::String(id)) = unique_field(entry, "i")? else {
                    bail!("block tick id must be a string")
                };
                let id = String::from_utf16(&id.0).context("invalid block tick id")?;
                ensure!(supported(&id), "unsupported saved block tick {id}");
                for field in ["x", "y", "z", "t", "p"] {
                    ensure!(
                        unique_field(entry, field)?
                            .and_then(Tag::as_java_int)
                            .is_some(),
                        "block tick {field} must be numeric"
                    );
                }
                let x = unique_field(entry, "x")?
                    .and_then(Tag::as_java_int)
                    .unwrap();
                let z = unique_field(entry, "z")?
                    .and_then(Tag::as_java_int)
                    .unwrap();
                ensure!(
                    x.div_euclid(16) == self.position.x && z.div_euclid(16) == self.position.z,
                    "saved block tick is outside its chunk"
                );
                let priority = unique_field(entry, "p")?
                    .and_then(Tag::as_java_int)
                    .unwrap();
                ensure!(
                    (-3..=3).contains(&priority),
                    "saved block tick priority is out of range"
                );
            }
        }
        let mut entries = Vec::with_capacity(ticks.len());
        for tick in ticks {
            ensure!(
                supported(&tick.block),
                "unsupported block tick {}",
                tick.block
            );
            ensure!(
                tick.position[0].div_euclid(16) == self.position.x
                    && tick.position[2].div_euclid(16) == self.position.z,
                "block tick is outside its chunk"
            );
            ensure!(
                (-3..=3).contains(&tick.priority),
                "block tick priority is out of range"
            );
            entries.push(Tag::Compound(vec![
                ("i".into(), Tag::String(tick.block.as_str().into())),
                ("x".into(), Tag::Int(tick.position[0])),
                ("y".into(), Tag::Int(tick.position[1])),
                ("z".into(), Tag::Int(tick.position[2])),
                ("t".into(), Tag::Int(tick.delay)),
                ("p".into(), Tag::Int(tick.priority)),
            ]));
        }
        let replacement = Tag::List {
            element_type: TagType::Compound,
            values: entries,
        };
        match unique_field_mut(&mut self.document.value, "block_ticks")? {
            Some(field) => *field = replacement,
            None => {
                let Tag::Compound(fields) = &mut self.document.value else {
                    bail!("chunk must be a compound")
                };
                fields.push(("block_ticks".into(), replacement));
            }
        }
        Ok(())
    }

    /// Replace understood fluid ticks in memory; this does not authorize a save.
    pub fn replace_fluid_ticks(
        &mut self,
        ticks: &[native_world::loaded::SavedFluidTick],
    ) -> Result<()> {
        fn supported(name: &str) -> bool {
            matches!(
                name,
                "minecraft:water"
                    | "water"
                    | "minecraft:flowing_water"
                    | "flowing_water"
                    | "minecraft:lava"
                    | "lava"
                    | "minecraft:flowing_lava"
                    | "flowing_lava"
            )
        }
        let source = crate::nbt::read(&self.source_nbt)?;
        if let Some(list) = unique_field(&source.value, "fluid_ticks")? {
            let Tag::List {
                element_type,
                values,
            } = list
            else {
                bail!("fluid_ticks must be a list")
            };
            ensure!(
                *element_type == TagType::Compound || values.is_empty(),
                "unsupported fluid tick list type"
            );
            for entry in values {
                let Tag::Compound(fields) = entry else {
                    bail!("fluid tick must be a compound")
                };
                ensure!(
                    fields.len() == 6,
                    "fluid tick has unsupported or missing fields"
                );
                for field in ["i", "x", "y", "z", "t", "p"] {
                    ensure!(
                        unique_field(entry, field)?.is_some(),
                        "fluid tick is missing {field}"
                    );
                }
                let Some(Tag::String(id)) = unique_field(entry, "i")? else {
                    bail!("fluid tick id must be a string")
                };
                let id = String::from_utf16(&id.0).context("invalid fluid tick id")?;
                ensure!(supported(&id), "unsupported saved fluid tick {id}");
                for field in ["x", "y", "z", "t", "p"] {
                    ensure!(
                        unique_field(entry, field)?
                            .and_then(Tag::as_java_int)
                            .is_some(),
                        "fluid tick {field} must be numeric"
                    );
                }
                let x = unique_field(entry, "x")?
                    .and_then(Tag::as_java_int)
                    .unwrap();
                let z = unique_field(entry, "z")?
                    .and_then(Tag::as_java_int)
                    .unwrap();
                let priority = unique_field(entry, "p")?
                    .and_then(Tag::as_java_int)
                    .unwrap();
                ensure!(
                    x.div_euclid(16) == self.position.x && z.div_euclid(16) == self.position.z,
                    "saved fluid tick is outside its chunk"
                );
                ensure!(
                    (-3..=3).contains(&priority),
                    "saved fluid tick priority is out of range"
                );
            }
        }
        let mut entries = Vec::with_capacity(ticks.len());
        for tick in ticks {
            ensure!(
                supported(&tick.fluid),
                "unsupported fluid tick {}",
                tick.fluid
            );
            ensure!(
                tick.position[0].div_euclid(16) == self.position.x
                    && tick.position[2].div_euclid(16) == self.position.z,
                "fluid tick is outside its chunk"
            );
            ensure!(
                (-3..=3).contains(&tick.priority),
                "fluid tick priority is out of range"
            );
            entries.push(Tag::Compound(vec![
                ("i".into(), Tag::String(tick.fluid.as_str().into())),
                ("x".into(), Tag::Int(tick.position[0])),
                ("y".into(), Tag::Int(tick.position[1])),
                ("z".into(), Tag::Int(tick.position[2])),
                ("t".into(), Tag::Int(tick.delay)),
                ("p".into(), Tag::Int(tick.priority)),
            ]));
        }
        let replacement = Tag::List {
            element_type: TagType::Compound,
            values: entries,
        };
        match unique_field_mut(&mut self.document.value, "fluid_ticks")? {
            Some(field) => *field = replacement,
            None => {
                let Tag::Compound(fields) = &mut self.document.value else {
                    bail!("chunk must be a compound")
                };
                fields.push(("fluid_ticks".into(), replacement));
            }
        }
        Ok(())
    }

    /// Rebuild known heightmaps after validating the source against its blocks.
    /// This does not update lighting, entities, ticks or authorize a file write.
    pub fn rebuild_heightmaps(
        &mut self,
        bounds: crate::heightmap::HeightBounds,
        classify: impl FnMut(&BlockState) -> Result<crate::heightmap::BlockHeightFlags>,
    ) -> Result<()> {
        let original = crate::nbt::read(&self.source_nbt)?;
        self.document = crate::heightmap::rebuild(
            &original,
            &self.document,
            bounds,
            self.heightmaps_rebuilt,
            classify,
        )?;
        self.heightmaps_rebuilt = true;
        Ok(())
    }
}

type StateKey = (Vec<u16>, Vec<(Vec<u16>, Vec<u16>)>);

fn key(state: &BlockState) -> Result<StateKey> {
    ensure!(!state.name.0.is_empty(), "block state name is empty");
    let mut properties: Vec<_> = state
        .properties
        .iter()
        .map(|(name, value)| (name.0.clone(), value.0.clone()))
        .collect();
    properties.sort();
    ensure!(
        properties.windows(2).all(|pair| pair[0].0 != pair[1].0),
        "block state has duplicate properties"
    );
    Ok((state.name.0.clone(), properties))
}

fn state_tag(state: &BlockState) -> Tag {
    if state.properties.is_empty() {
        return Tag::String(state.name.clone());
    }
    Tag::Compound(vec![
        ("id".into(), Tag::String(state.name.clone())),
        (
            "properties".into(),
            Tag::Compound(
                state
                    .properties
                    .iter()
                    .map(|(name, value)| (name.clone(), Tag::String(value.clone())))
                    .collect(),
            ),
        ),
    ])
}

fn append_palette_entry(
    element_type: &mut TagType,
    values: &mut Vec<Tag>,
    value: Tag,
) -> Result<()> {
    match (*element_type, value) {
        (TagType::String, value @ Tag::String(_)) => values.push(value),
        (TagType::String, value @ Tag::Compound(_)) => {
            for entry in values.iter_mut() {
                let original = std::mem::replace(entry, Tag::Compound(Vec::new()));
                *entry = Tag::Compound(vec![("".into(), original)]);
            }
            *element_type = TagType::Compound;
            values.push(value);
        }
        (TagType::Compound, value @ Tag::String(_)) => {
            values.push(Tag::Compound(vec![("".into(), value)]))
        }
        (TagType::Compound, value @ Tag::Compound(_)) => values.push(value),
        _ => bail!("unsupported palette element type"),
    }
    Ok(())
}

fn unique_field<'a>(tag: &'a Tag, name: &str) -> Result<Option<&'a Tag>> {
    let Tag::Compound(entries) = tag else {
        bail!("{name} parent is not a compound")
    };
    let mut matches = entries.iter().filter(|(key, _)| key.matches(name));
    let value = matches.next().map(|(_, value)| value);
    ensure!(
        matches.next().is_none(),
        "duplicate {name} tags are not supported for editing"
    );
    Ok(value)
}

fn unique_field_mut<'a>(tag: &'a mut Tag, name: &str) -> Result<Option<&'a mut Tag>> {
    let Tag::Compound(entries) = tag else {
        bail!("{name} parent is not a compound")
    };
    ensure!(
        entries.iter().filter(|(key, _)| key.matches(name)).count() <= 1,
        "duplicate {name} tags are not supported for editing"
    );
    Ok(entries
        .iter_mut()
        .find(|(key, _)| key.matches(name))
        .map(|(_, value)| value))
}

fn bits(palette_len: usize) -> u32 {
    (usize::BITS - (palette_len - 1).leading_zeros()).max(4)
}

fn unused_bits_are_zero(words: &[i64], bits: u32) -> bool {
    let per_word = 64 / bits as usize;
    words.iter().enumerate().all(|(word_index, value)| {
        let remaining = BLOCKS_PER_SECTION.saturating_sub(word_index * per_word);
        let used_bits = remaining.min(per_word) * bits as usize;
        let mask = if used_bits == 64 {
            u64::MAX
        } else {
            (1u64 << used_bits) - 1
        };
        (*value as u64 & !mask) == 0
    })
}

/// Apply a batch atomically in memory; missing data and stale expected states are errors.
pub fn draft_chunk_blocks(
    original: &NamedTag,
    position: ChunkPos,
    changes: &[BlockChange],
) -> Result<ChunkDraft> {
    for field in ["DataVersion", "xPos", "zPos", "Status", "sections"] {
        unique_field(&original.value, field)?;
    }
    crate::save::validate_chunk(&original.value, position.x, position.z)?;
    let source_nbt = crate::nbt::write(original).context("cannot encode source chunk")?;
    let decoded = palette::decode_sections(&original.value)?;
    let mut grouped: BTreeMap<i8, Vec<&BlockChange>> = BTreeMap::new();
    for change in changes {
        ensure!(
            change.x.div_euclid(16) == position.x && change.z.div_euclid(16) == position.z,
            "block position is outside the chunk"
        );
        let section =
            i8::try_from(change.y.div_euclid(16)).context("block Y is outside section range")?;
        key(&change.expected)?;
        key(&change.replacement)?;
        grouped.entry(section).or_default().push(change);
    }
    let mut document = original.clone();
    let mut changed_blocks = 0;
    let mut changed_sections = BTreeSet::new();
    let mut changed_block_states = Vec::new();
    if grouped.is_empty() {
        return Ok(ChunkDraft {
            source_nbt,
            document,
            position,
            heightmaps_rebuilt: false,
            changed_block_states,
            uncoordinated_block_entities: false,
            changed_blocks,
            changed_sections,
        });
    }
    let sections = match unique_field_mut(&mut document.value, "sections")? {
        Some(Tag::List {
            element_type: TagType::Compound,
            values,
        }) => values,
        _ => bail!("chunk has no editable sections"),
    };
    for (section_y, changes) in grouped {
        let mut decoded_blocks = decoded
            .iter()
            .find(|section| section.y == section_y)
            .and_then(|section| section.blocks.clone())
            .with_context(|| format!("section {section_y} has no block-state data"))?;
        let section = sections
            .iter_mut()
            .find(|section| matches!(section.get("Y"), Some(Tag::Byte(y)) if *y == section_y))
            .with_context(|| format!("missing section {section_y}"))?;
        unique_field(section, "Y")?;
        let state_storage = unique_field_mut(section, "block_states")?
            .context("section is missing block_states")?;
        let raw_data = match unique_field(state_storage, "data")? {
            Some(Tag::LongArray(words)) => Some(words.clone()),
            None => None,
            _ => bail!("block-state data is not a long array"),
        };
        let original_bits = bits(decoded_blocks.states.len());
        let (element_type, values) = match unique_field_mut(state_storage, "palette")? {
            Some(Tag::List {
                element_type,
                values,
            }) if matches!(*element_type, TagType::Compound | TagType::String) => {
                (element_type, values)
            }
            _ => bail!("block-state palette is not a supported list"),
        };
        for value in values.iter() {
            let value = palette::unwrap_palette_entry(value);
            match value {
                Tag::String(_) => {}
                Tag::Compound(_) => {
                    ensure!(
                        value.get("Name").is_none() && value.get("Properties").is_none(),
                        "legacy block-state format cannot be edited as 26.3"
                    );
                    unique_field(value, "id")?;
                    unique_field(value, "properties")?;
                }
                _ => bail!("invalid block-state palette entry"),
            }
        }
        let mut lookup = BTreeMap::new();
        for (index, state) in decoded_blocks.states.iter().enumerate() {
            lookup.entry(key(state)?).or_insert(index as u32);
        }
        let mut section_changes = 0usize;
        for change in changes {
            let index = change.x.rem_euclid(16) as usize
                + change.z.rem_euclid(16) as usize * 16
                + change.y.rem_euclid(16) as usize * 256;
            let current = decoded_blocks.indices[index] as usize;
            ensure!(
                key(&decoded_blocks.states[current])? == key(&change.expected)?,
                "block state changed before edit at ({}, {}, {})",
                change.x,
                change.y,
                change.z
            );
            let replacement = key(&change.replacement)?;
            if key(&decoded_blocks.states[current])? == replacement {
                continue;
            }
            changed_block_states.push(BlockChange {
                expected: decoded_blocks.states[current].clone(),
                ..change.clone()
            });
            let next = if let Some(index) = lookup.get(&replacement) {
                *index
            } else {
                ensure!(
                    values.len() < BLOCKS_PER_SECTION,
                    "block palette has no room for another entry without discarding existing data"
                );
                let index = values.len() as u32;
                append_palette_entry(element_type, values, state_tag(&change.replacement))?;
                decoded_blocks.states.push(change.replacement.clone());
                lookup.insert(replacement, index);
                index
            };
            decoded_blocks.indices[index] = next;
            section_changes += 1;
        }
        if section_changes == 0 {
            continue;
        }
        let new_bits = bits(decoded_blocks.states.len());
        let data = if let Some(mut old) = raw_data {
            if original_bits == new_bits {
                let per_word = 64usize / original_bits as usize;
                let mask = (1u64 << original_bits) - 1;
                for (index, value) in decoded_blocks.indices.iter().enumerate() {
                    let shift = (index % per_word) * original_bits as usize;
                    let word = old[index / per_word] as u64;
                    old[index / per_word] =
                        ((word & !(mask << shift)) | (u64::from(*value) << shift)) as i64;
                }
                old
            } else {
                ensure!(
                    unused_bits_are_zero(&old, original_bits),
                    "cannot repack a section with nonzero unused palette bits"
                );
                palette::encode_indices(decoded_blocks.states.len(), &decoded_blocks.indices, 4)?
                    .context("missing packed block data")?
            }
        } else {
            palette::encode_indices(decoded_blocks.states.len(), &decoded_blocks.indices, 4)?
                .context("missing packed block data")?
        };
        match unique_field_mut(state_storage, "data")? {
            Some(value) => *value = Tag::LongArray(data),
            None => {
                let Tag::Compound(entries) = state_storage else {
                    unreachable!()
                };
                entries.push((JavaString::from("data"), Tag::LongArray(data)));
            }
        }
        changed_blocks += section_changes;
        changed_sections.insert(section_y);
    }
    Ok(ChunkDraft {
        source_nbt,
        document,
        position,
        heightmaps_rebuilt: false,
        changed_block_states,
        uncoordinated_block_entities: false,
        changed_blocks,
        changed_sections,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(name: &str) -> BlockState {
        BlockState {
            name: name.into(),
            properties: Vec::new(),
        }
    }

    #[test]
    fn falling_tick_draft_is_atomic_and_preserves_other_chunk_fields() {
        let pos = ChunkPos { x: -1, z: 2 };
        let original = fixture(1, false);
        let mut draft = draft_chunk_blocks(&original, pos, &[]).unwrap();
        let tick = native_world::loaded::SavedBlockTick {
            block: "minecraft:gravel".into(),
            position: [-15, -60, 33],
            delay: -2,
            priority: -1,
        };
        draft
            .replace_falling_block_ticks(std::slice::from_ref(&tick))
            .unwrap();
        assert!(draft.matches_source(&original).unwrap());
        assert_eq!(
            draft.document().value.get("unknown_chunk"),
            original.value.get("unknown_chunk")
        );
        let Some(Tag::List { values, .. }) = draft.document().value.get("block_ticks") else {
            panic!("missing ticks")
        };
        assert_eq!(values.len(), 1);
        assert_eq!(
            values[0].get("i"),
            Some(&Tag::String("minecraft:gravel".into()))
        );
        assert_eq!(values[0].get("t"), Some(&Tag::Int(-2)));
        let before = crate::nbt::write(draft.document()).unwrap();
        let bad = native_world::loaded::SavedBlockTick {
            position: [0, 0, 0],
            ..tick
        };
        assert!(draft.replace_falling_block_ticks(&[bad]).is_err());
        assert_eq!(crate::nbt::write(draft.document()).unwrap(), before);
    }

    #[test]
    fn all_simulated_button_and_lamp_ticks_round_trip_without_widening_falling_only() {
        let pos = ChunkPos { x: -1, z: 2 };
        let original = fixture(1, false);
        let mut ticks = Vec::new();
        for (index, button) in native_core::button::Button::ALL.into_iter().enumerate() {
            for id in [button.id(), button.id().strip_prefix("minecraft:").unwrap()] {
                ticks.push(native_world::loaded::SavedBlockTick {
                    block: id.into(),
                    position: [-16 + (index as i32 % 16), 64, 33],
                    delay: index as i32 - 8,
                    priority: index as i32 % 7 - 3,
                });
            }
        }
        for id in ["minecraft:redstone_lamp", "redstone_lamp"] {
            ticks.push(native_world::loaded::SavedBlockTick {
                block: id.into(),
                position: [-1, 65, 33],
                delay: 4,
                priority: 0,
            });
        }
        let mut draft = draft_chunk_blocks(&original, pos, &[]).unwrap();
        draft.replace_block_ticks(&ticks).unwrap();
        let source = draft.document().clone();
        let mut next = draft_chunk_blocks(&source, pos, &[]).unwrap();
        next.replace_block_ticks(&ticks).unwrap();
        assert_eq!(
            source.value.get("block_ticks"),
            next.document().value.get("block_ticks")
        );
        let before = crate::nbt::write(next.document()).unwrap();
        let mut unsupported = ticks.clone();
        unsupported.push(native_world::loaded::SavedBlockTick {
            block: "example:oak_button".into(),
            position: [-1, 65, 33],
            delay: 4,
            priority: 0,
        });
        assert!(next.replace_block_ticks(&unsupported).is_err());
        assert_eq!(crate::nbt::write(next.document()).unwrap(), before);
        assert!(next.replace_falling_block_ticks(&[]).is_err());
        assert_eq!(crate::nbt::write(next.document()).unwrap(), before);
    }

    #[test]
    fn light_layer_draft_preserves_other_data_and_rejects_stale_or_ambiguous_sources() {
        let pos = ChunkPos { x: -1, z: 2 };
        let mut original = fixture(1, false);
        let Tag::Compound(fields) = &mut original.value else {
            unreachable!()
        };
        fields.push(("isLightOn".into(), Tag::Byte(1)));
        let mut draft = draft_chunk_blocks(&original, pos, &[]).unwrap();
        let changes = [
            SectionLightChange {
                y: -4,
                block: Some(LightLayerChange {
                    expected: None,
                    replacement: Some(Box::new([0x34; 2048])),
                }),
                sky: None,
            },
            SectionLightChange {
                y: -3,
                block: None,
                sky: Some(LightLayerChange {
                    expected: Some(Box::new([0xff; 2048])),
                    replacement: None,
                }),
            },
            SectionLightChange {
                y: -5,
                block: Some(LightLayerChange {
                    expected: None,
                    replacement: Some(Box::new([0x12; 2048])),
                }),
                sky: Some(LightLayerChange {
                    expected: None,
                    replacement: Some(Box::new([0xab; 2048])),
                }),
            },
        ];
        draft.replace_section_lights(&changes).unwrap();
        assert!(draft.matches_source(&original).unwrap());
        assert_eq!(
            draft.document().value.get("unknown_chunk"),
            original.value.get("unknown_chunk")
        );
        assert_eq!(
            section(draft.document()).get("unknown_section"),
            section(&original).get("unknown_section")
        );
        let decoded = palette::decode_sections(&draft.document().value).unwrap();
        assert_eq!(
            decoded
                .iter()
                .find(|s| s.y == -4)
                .unwrap()
                .block_light
                .as_deref(),
            Some(&[0x34; 2048][..])
        );
        assert!(
            decoded
                .iter()
                .find(|s| s.y == -3)
                .unwrap()
                .sky_light
                .is_none()
        );
        let new = decoded.iter().find(|s| s.y == -5).unwrap();
        assert_eq!(new.block_light.as_deref(), Some(&[0x12; 2048][..]));
        assert_eq!(new.sky_light.as_deref(), Some(&[0xab; 2048][..]));
        let before = crate::nbt::write(draft.document()).unwrap();
        let mut stale = changes.clone();
        stale[1].sky.as_mut().unwrap().expected = Some(Box::new([0; 2048]));
        assert!(draft.replace_section_lights(&stale).is_err());
        assert_eq!(crate::nbt::write(draft.document()).unwrap(), before);
        assert!(
            draft
                .replace_section_lights(&[changes[0].clone(), changes[0].clone()])
                .is_err()
        );
        assert_eq!(crate::nbt::write(draft.document()).unwrap(), before);
        for marker in [None, Some(Tag::Byte(0)), Some(Tag::Int(1))] {
            let mut source = fixture(1, false);
            if let Some(marker) = marker {
                let Tag::Compound(fields) = &mut source.value else {
                    unreachable!()
                };
                fields.push(("isLightOn".into(), marker));
            }
            let mut invalid = draft_chunk_blocks(&source, pos, &[]).unwrap();
            assert!(invalid.replace_section_lights(&changes).is_err());
        }
    }

    #[test]
    fn block_tick_draft_preserves_button_falling_and_soil_order() {
        let pos = ChunkPos { x: -1, z: 2 };
        let original = fixture(1, false);
        let mut draft = draft_chunk_blocks(&original, pos, &[]).unwrap();
        let ticks = [
            native_world::loaded::SavedBlockTick {
                block: "minecraft:stone_button".into(),
                position: [-14, -60, 33],
                delay: 7,
                priority: 0,
            },
            native_world::loaded::SavedBlockTick {
                block: "minecraft:gravel".into(),
                position: [-15, -60, 33],
                delay: 3,
                priority: -1,
            },
            native_world::loaded::SavedBlockTick {
                block: "minecraft:dirt_path".into(),
                position: [-14, -61, 33],
                delay: -2,
                priority: 1,
            },
            native_world::loaded::SavedBlockTick {
                block: "minecraft:farmland".into(),
                position: [-13, -61, 33],
                delay: 5,
                priority: 0,
            },
        ];
        draft.replace_block_ticks(&ticks).unwrap();
        let Some(Tag::List { values, .. }) = draft.document().value.get("block_ticks") else {
            panic!("missing ticks")
        };
        assert_eq!(values.len(), 4);
        assert_eq!(
            values[0].get("i"),
            Some(&Tag::String("minecraft:stone_button".into()))
        );
        assert_eq!(values[0].get("t"), Some(&Tag::Int(7)));
        assert_eq!(
            values[1].get("i"),
            Some(&Tag::String("minecraft:gravel".into()))
        );
        assert_eq!(values[1].get("p"), Some(&Tag::Int(-1)));
        assert_eq!(
            values[2].get("i"),
            Some(&Tag::String("minecraft:dirt_path".into()))
        );
        assert_eq!(values[2].get("t"), Some(&Tag::Int(-2)));
        assert_eq!(
            values[3].get("i"),
            Some(&Tag::String("minecraft:farmland".into()))
        );

        let source_with_button = draft.document().clone();
        let mut full = draft_chunk_blocks(&source_with_button, pos, &[]).unwrap();
        full.replace_block_ticks(&ticks).unwrap();
        let mut falling_only = draft_chunk_blocks(&source_with_button, pos, &[]).unwrap();
        let before = crate::nbt::write(falling_only.document()).unwrap();
        assert!(falling_only.replace_falling_block_ticks(&[]).is_err());
        assert_eq!(crate::nbt::write(falling_only.document()).unwrap(), before);

        let mut soil_only = draft_chunk_blocks(&original, pos, &[]).unwrap();
        soil_only.replace_block_ticks(&ticks[2..]).unwrap();
        let mut falling_only = draft_chunk_blocks(soil_only.document(), pos, &[]).unwrap();
        let before = crate::nbt::write(falling_only.document()).unwrap();
        assert!(falling_only.replace_falling_block_ticks(&[]).is_err());
        assert_eq!(crate::nbt::write(falling_only.document()).unwrap(), before);
    }

    #[test]
    fn falling_tick_draft_rejects_unknown_source_fields_and_ticks() {
        let pos = ChunkPos { x: -1, z: 2 };
        for unknown in [false, true] {
            let mut original = fixture(1, false);
            let Tag::Compound(fields) = &mut original.value else {
                unreachable!()
            };
            let mut tick = vec![
                (
                    "i".into(),
                    Tag::String(
                        if unknown {
                            "minecraft:observer"
                        } else {
                            "minecraft:sand"
                        }
                        .into(),
                    ),
                ),
                ("x".into(), Tag::Int(-15)),
                ("y".into(), Tag::Int(0)),
                ("z".into(), Tag::Int(33)),
                ("t".into(), Tag::Int(2)),
                ("p".into(), Tag::Int(0)),
            ];
            if !unknown {
                tick.push(("extra".into(), Tag::Int(1)));
            }
            fields.push((
                "block_ticks".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![Tag::Compound(tick)],
                },
            ));
            let mut draft = draft_chunk_blocks(&original, pos, &[]).unwrap();
            let before = crate::nbt::write(draft.document()).unwrap();
            assert!(draft.replace_falling_block_ticks(&[]).is_err());
            assert_eq!(crate::nbt::write(draft.document()).unwrap(), before);
        }
    }

    #[test]
    fn fluid_tick_draft_preserves_other_data_and_rejects_unknown_fields_atomically() {
        let pos = ChunkPos { x: -1, z: 2 };
        let mut original = fixture(1, false);
        let Tag::Compound(fields) = &mut original.value else {
            unreachable!()
        };
        fields.push((
            "fluid_ticks".into(),
            Tag::List {
                element_type: TagType::Compound,
                values: vec![Tag::Compound(vec![
                    ("i".into(), Tag::String("minecraft:water".into())),
                    ("x".into(), Tag::Int(-15)),
                    ("y".into(), Tag::Int(-60)),
                    ("z".into(), Tag::Int(33)),
                    ("t".into(), Tag::Int(4)),
                    ("p".into(), Tag::Int(0)),
                ])],
            },
        ));
        let mut draft = draft_chunk_blocks(&original, pos, &[]).unwrap();
        let tick = native_world::loaded::SavedFluidTick {
            fluid: "minecraft:flowing_lava".into(),
            position: [-15, -60, 33],
            delay: -2,
            priority: -1,
        };
        draft
            .replace_fluid_ticks(std::slice::from_ref(&tick))
            .unwrap();
        assert!(draft.matches_source(&original).unwrap());
        assert_eq!(
            draft.document().value.get("unknown_chunk"),
            original.value.get("unknown_chunk")
        );
        let Some(Tag::List { values, .. }) = draft.document().value.get("fluid_ticks") else {
            panic!("missing ticks")
        };
        assert_eq!(
            values[0].get("i"),
            Some(&Tag::String("minecraft:flowing_lava".into()))
        );
        let before = crate::nbt::write(draft.document()).unwrap();
        let bad = native_world::loaded::SavedFluidTick {
            fluid: "example:unknown".into(),
            ..tick
        };
        assert!(draft.replace_fluid_ticks(&[bad]).is_err());
        assert_eq!(crate::nbt::write(draft.document()).unwrap(), before);
        let Tag::Compound(fields) = &mut original.value else {
            unreachable!()
        };
        let Tag::List { values, .. } = &mut fields
            .iter_mut()
            .find(|(name, _)| name.matches("fluid_ticks"))
            .unwrap()
            .1
        else {
            unreachable!()
        };
        let Tag::Compound(entry) = &mut values[0] else {
            unreachable!()
        };
        entry.push(("unknown".into(), Tag::Int(1)));
        let mut draft = draft_chunk_blocks(&original, pos, &[]).unwrap();
        assert!(draft.replace_fluid_ticks(&[]).is_err());
    }

    fn fixture(palette_size: usize, padding_bit: bool) -> NamedTag {
        let mut entries = vec![Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:stone".into())),
            ("unknown_palette_field".into(), Tag::Int(99)),
        ])];
        for index in 1..palette_size {
            entries.push(Tag::Compound(vec![(
                "".into(),
                state_tag(&state(&format!("fixture:block_{index}"))),
            )]));
        }
        let mut block_states = vec![
            (
                "palette".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: entries,
                },
            ),
            ("unknown_storage_field".into(), Tag::Long(123)),
        ];
        if palette_size > 1 {
            let mut data = palette::encode_indices(palette_size, &vec![0; BLOCKS_PER_SECTION], 4)
                .unwrap()
                .unwrap();
            if padding_bit {
                data[0] |= i64::MIN;
            }
            block_states.push(("data".into(), Tag::LongArray(data)));
        }
        NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("xPos".into(), Tag::Int(-1)),
                ("zPos".into(), Tag::Int(2)),
                ("Status".into(), Tag::String("minecraft:full".into())),
                (
                    "sections".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![
                            Tag::Compound(vec![
                                ("Y".into(), Tag::Byte(-4)),
                                ("block_states".into(), Tag::Compound(block_states)),
                                ("unknown_section".into(), Tag::String("keep".into())),
                            ]),
                            Tag::Compound(vec![
                                ("Y".into(), Tag::Byte(-3)),
                                ("SkyLight".into(), Tag::ByteArray(vec![255; 2048])),
                            ]),
                        ],
                    },
                ),
                (
                    "unknown_chunk".into(),
                    Tag::Compound(vec![("keep".into(), Tag::Int(42))]),
                ),
            ]),
        }
    }

    fn edit(x: i32, replacement: &str) -> BlockChange {
        BlockChange {
            x,
            y: -64,
            z: 32,
            expected: state("minecraft:stone"),
            replacement: state(replacement),
        }
    }

    fn section(chunk: &NamedTag) -> &Tag {
        let Tag::List { values, .. } = chunk.value.get("sections").unwrap() else {
            unreachable!()
        };
        &values[0]
    }

    #[test]
    fn block_entity_dependencies_reject_removal_and_ambiguous_positions() {
        let pos = ChunkPos { x: -1, z: 2 };
        let entity = |x: Tag, y: Tag, z: Tag| {
            Tag::Compound(vec![
                ("id".into(), Tag::String("minecraft:chest".into())),
                ("x".into(), x),
                ("y".into(), y),
                ("z".into(), z),
                (
                    "future".into(),
                    Tag::Compound(vec![("keep".into(), Tag::Int(42))]),
                ),
            ])
        };
        let target = entity(Tag::Int(-16), Tag::Int(-64), Tag::Int(32));
        let elsewhere = entity(Tag::Int(-15), Tag::Int(-64), Tag::Int(32));
        let with_entities = |list: Tag| {
            let mut original = fixture(1, false);
            let Tag::Compound(fields) = &mut original.value else {
                unreachable!()
            };
            fields.push(("block_entities".into(), list));
            original
        };
        let list = |values| Tag::List {
            element_type: TagType::Compound,
            values,
        };
        for (entries, allowed) in [
            (list(vec![target.clone()]), false),
            (list(vec![elsewhere.clone()]), true),
            (list(vec![]), true),
            (list(vec![elsewhere.clone(), elsewhere.clone()]), false),
            (
                list(vec![entity(Tag::Int(0), Tag::Int(-64), Tag::Int(32))]),
                false,
            ),
            (
                list(vec![entity(Tag::Byte(-15), Tag::Int(-64), Tag::Int(32))]),
                false,
            ),
            (Tag::String("bad".into()), false),
        ] {
            let source = with_entities(entries);
            let mut draft =
                draft_chunk_blocks(&source, pos, &[edit(-16, "minecraft:dirt")]).unwrap();
            draft.heightmaps_rebuilt = true;
            assert_eq!(
                draft.validate_for_write(|_| Ok(())).is_ok(),
                allowed,
                "{source:?}"
            );
            assert_eq!(
                draft.document().value.get("block_entities"),
                source.value.get("block_entities")
            );
        }
        let mut duplicate = with_entities(list(vec![elsewhere]));
        let Tag::Compound(fields) = &mut duplicate.value else {
            unreachable!()
        };
        fields.push(("block_entities".into(), list(vec![])));
        let mut draft =
            draft_chunk_blocks(&duplicate, pos, &[edit(-16, "minecraft:dirt")]).unwrap();
        draft.heightmaps_rebuilt = true;
        assert!(draft.validate_for_write(|_| Ok(())).is_err());
        let source = with_entities(list(vec![target]));
        let mut change = edit(-16, "minecraft:stone");
        change
            .replacement
            .properties
            .push(("example".into(), "different".into()));
        let mut draft = draft_chunk_blocks(&source, pos, &[change]).unwrap();
        draft.heightmaps_rebuilt = true;
        assert!(draft.validate_for_write(|_| Ok(())).is_err());
    }

    #[test]
    fn staged_block_entities_are_atomic_and_cannot_be_written() {
        let pos = ChunkPos { x: -1, z: 2 };
        let entity = |x| {
            Tag::Compound(vec![
                ("id".into(), Tag::String("minecraft:chest".into())),
                ("x".into(), Tag::Int(x)),
                ("y".into(), Tag::Int(-64)),
                ("z".into(), Tag::Int(32)),
                ("Items".into(), Tag::Int(1)),
                ("future".into(), Tag::Int(42)),
            ])
        };
        let first = entity(-16);
        let other = entity(-15);
        let mut original = fixture(1, false);
        let Tag::Compound(fields) = &mut original.value else {
            unreachable!()
        };
        fields.push((
            "block_entities".into(),
            Tag::List {
                element_type: TagType::Compound,
                values: vec![first.clone(), other.clone()],
            },
        ));
        let mut replacement = first.clone();
        let Tag::Compound(fields) = &mut replacement else {
            unreachable!()
        };
        fields
            .iter_mut()
            .find(|(key, _)| key.matches("Items"))
            .unwrap()
            .1 = Tag::Int(2);
        let valid = BlockEntityChange {
            expected: first.clone(),
            replacement: replacement.clone(),
        };
        let mut draft = draft_chunk_blocks(&original, pos, &[]).unwrap();
        let before = draft.document().clone();
        let missing = BlockEntityChange {
            expected: entity(-14),
            replacement: entity(-14),
        };
        assert!(
            draft
                .replace_block_entities(&[valid.clone(), missing])
                .is_err()
        );
        assert_eq!(draft.document(), &before);
        let stale = BlockEntityChange {
            expected: entity(-15),
            replacement: replacement.clone(),
        };
        assert!(draft.replace_block_entities(&[stale]).is_err());
        draft.replace_block_entities(&[valid]).unwrap();
        assert!(draft.matches_source(&original).unwrap());
        let Some(Tag::List { values, .. }) = draft.document().value.get("block_entities") else {
            panic!("missing entities")
        };
        assert_eq!(values, &[replacement, other]);
        assert!(draft.validate_for_write(|_| Ok(())).is_err());
    }

    #[test]
    fn grows_palette_and_preserves_unrelated_and_unknown_nbt() {
        let original = fixture(1, false);
        let changes: Vec<_> = (0..16)
            .map(|i| edit(-16 + i, &format!("fixture:new_{i}")))
            .collect();
        let draft = draft_chunk_blocks(&original, ChunkPos { x: -1, z: 2 }, &changes).unwrap();
        assert_eq!(draft.changed_blocks, 16);
        assert_eq!(draft.changed_sections, BTreeSet::from([-4]));
        assert_eq!(
            draft.document().value.get("unknown_chunk"),
            original.value.get("unknown_chunk")
        );
        assert_eq!(
            section(draft.document()).get("unknown_section"),
            section(&original).get("unknown_section")
        );
        let Tag::List {
            values: old_sections,
            ..
        } = original.value.get("sections").unwrap()
        else {
            unreachable!()
        };
        let Tag::List {
            values: new_sections,
            ..
        } = draft.document().value.get("sections").unwrap()
        else {
            unreachable!()
        };
        assert_eq!(old_sections[1], new_sections[1]);
        let old_states = section(&original).get("block_states").unwrap();
        let new_states = section(draft.document()).get("block_states").unwrap();
        assert_eq!(
            old_states.get("unknown_storage_field"),
            new_states.get("unknown_storage_field")
        );
        let Tag::List {
            values: old_palette,
            ..
        } = old_states.get("palette").unwrap()
        else {
            unreachable!()
        };
        let Tag::List {
            values: new_palette,
            ..
        } = new_states.get("palette").unwrap()
        else {
            unreachable!()
        };
        assert_eq!(new_palette.len(), 17);
        assert_eq!(new_palette[0], old_palette[0]);
        let decoded = palette::decode_sections(&draft.document().value).unwrap();
        let blocks = decoded[0].blocks.as_ref().unwrap();
        for i in 0..16 {
            assert_eq!(
                blocks.block(i, 0, 0).unwrap().name,
                state(&format!("fixture:new_{i}")).name
            );
        }
        assert_eq!(
            blocks.block(0, 0, 1).unwrap().name,
            state("minecraft:stone").name
        );
        let Tag::LongArray(data) = new_states.get("data").unwrap() else {
            unreachable!()
        };
        assert_eq!(data.len(), BLOCKS_PER_SECTION.div_ceil(12));
    }

    #[test]
    fn stale_missing_and_ambiguous_edits_do_not_change_the_original() {
        let original = fixture(1, false);
        let before = original.clone();
        let mut stale = edit(-15, "minecraft:dirt");
        stale.expected = state("minecraft:air");
        assert!(
            draft_chunk_blocks(
                &original,
                ChunkPos { x: -1, z: 2 },
                &[edit(-16, "minecraft:dirt"), stale]
            )
            .is_err()
        );
        assert!(
            draft_chunk_blocks(
                &original,
                ChunkPos { x: -1, z: 2 },
                &[edit(0, "minecraft:dirt")]
            )
            .is_err()
        );
        let mut missing = edit(-16, "minecraft:dirt");
        missing.y = -48;
        assert!(draft_chunk_blocks(&original, ChunkPos { x: -1, z: 2 }, &[missing]).is_err());
        assert_eq!(original, before);
        let noop = draft_chunk_blocks(
            &original,
            ChunkPos { x: -1, z: 2 },
            &[edit(-16, "minecraft:stone")],
        )
        .unwrap();
        assert_eq!(noop.changed_blocks, 0);
        assert_eq!(noop.document(), &original);
    }

    #[test]
    fn preserves_unused_bits_or_refuses_to_repack_them() {
        let original = fixture(17, true);
        let draft = draft_chunk_blocks(
            &original,
            ChunkPos { x: -1, z: 2 },
            &[edit(-16, "fixture:block_1")],
        )
        .unwrap();
        let Tag::LongArray(data) = section(draft.document())
            .get("block_states")
            .unwrap()
            .get("data")
            .unwrap()
        else {
            unreachable!()
        };
        assert_ne!(data[0] & i64::MIN, 0);
        let cannot_repack = fixture(32, true);
        let error = draft_chunk_blocks(
            &cannot_repack,
            ChunkPos { x: -1, z: 2 },
            &[edit(-16, "fixture:new")],
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("nonzero unused palette bits"));
    }

    #[test]
    fn mixed_default_and_property_states_use_heterogeneous_nbt_wrappers() {
        let mut original = fixture(1, false);
        let sections = unique_field_mut(&mut original.value, "sections")
            .unwrap()
            .unwrap();
        let Tag::List { values, .. } = sections else {
            unreachable!()
        };
        let storage = unique_field_mut(&mut values[0], "block_states")
            .unwrap()
            .unwrap();
        *unique_field_mut(storage, "palette").unwrap().unwrap() = Tag::List {
            element_type: TagType::String,
            values: vec![Tag::String("minecraft:stone".into())],
        };
        let mut replacement = state("minecraft:oak_log");
        replacement.properties.push(("axis".into(), "x".into()));
        let change = BlockChange {
            replacement,
            ..edit(-16, "minecraft:stone")
        };
        let draft = draft_chunk_blocks(&original, ChunkPos { x: -1, z: 2 }, &[change]).unwrap();
        let Tag::List {
            element_type,
            values,
        } = section(draft.document())
            .get("block_states")
            .unwrap()
            .get("palette")
            .unwrap()
        else {
            unreachable!()
        };
        assert_eq!(*element_type, TagType::Compound);
        assert_eq!(
            values[0],
            Tag::Compound(vec![("".into(), Tag::String("minecraft:stone".into()))])
        );
        assert!(
            matches!(values[1].get("id"), Some(Tag::String(name)) if name.matches("minecraft:oak_log"))
        );
        let blocks = palette::decode_sections(&draft.document().value).unwrap();
        assert_eq!(
            blocks[0]
                .blocks
                .as_ref()
                .unwrap()
                .block(0, 0, 0)
                .unwrap()
                .properties,
            vec![("axis".into(), "x".into())]
        );
    }
}
