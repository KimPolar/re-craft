//! In-memory POI changes. A draft is not a complete or writable world save.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail, ensure};
use native_world::{loaded::BlockState, poi};

use crate::{
    nbt::{JavaString, NamedTag, Tag, TagType},
    world::{ChunkPos, SavedPoiChunk, SavedPoiRecord, TARGET_DATA_VERSION, decode_poi_chunk},
};

pub struct PoiBlockChange {
    pub position: [i32; 3],
    pub expected: BlockState,
    pub replacement: BlockState,
}

/// A section reconciliation result. It cannot be written as a world save.
pub struct PoiRefreshDraft {
    pub source: Option<NamedTag>,
    pub document: Option<NamedTag>,
    pub discarded_positions: BTreeSet<[i32; 3]>,
    pub changed: bool,
}

/// Reconcile a missing or invalid POI section with a complete loaded block section.
/// A valid existing section is left untouched, as in Java 26.3. For invalid
/// sections, Java retains any previous record at the same position, even if its
/// POI type differs from the current block. The returned draft is not writable.
/// Callers must validate the dimension and provide registry checks for states
/// and persisted POI types; missing block data is never assumed to be air.
pub fn refresh_section(
    source: Option<&SavedPoiChunk>,
    position: ChunkPos,
    section_y: i8,
    world: &native_world::loaded::World,
    mut validate_state: impl FnMut(&BlockState) -> Result<()>,
    mut validate_type: impl FnMut(&JavaString) -> Result<()>,
) -> Result<PoiRefreshDraft> {
    let original = source.map(|source| (*source.document).clone());
    let decoded = match source {
        Some(source) => {
            ensure!(
                source.position == position,
                "POI source belongs to another chunk"
            );
            Some(decode_poi_chunk(original.clone().unwrap(), position)?)
        }
        None => None,
    };
    let previous = decoded
        .as_ref()
        .and_then(|chunk| chunk.sections.get(&i32::from(section_y)));
    if let Some(section) = previous {
        for record in &section.records {
            validate_type(&record.type_id)?;
        }
    }
    if previous.is_some_and(|section| section.valid) {
        return Ok(PoiRefreshDraft {
            source: original.clone(),
            document: original,
            discarded_positions: BTreeSet::new(),
            changed: false,
        });
    }
    let chunk = world
        .chunks
        .get(&native_world::loaded::ChunkPos {
            x: position.x,
            z: position.z,
        })
        .context("POI refresh terrain chunk is missing")?;
    let blocks = chunk
        .sections
        .get(&section_y)
        .and_then(|section| section.blocks.as_ref())
        .context("POI refresh block section is incomplete")?;
    let base_x = position.x.checked_mul(16).context("POI chunk X overflow")?;
    let base_z = position.z.checked_mul(16).context("POI chunk Z overflow")?;
    let base_y = i32::from(section_y) * 16;
    let mut cached = BTreeMap::new();
    let mut discovered = Vec::new();
    for (index, id) in blocks.iter().copied().enumerate() {
        let kind = if let Some(kind) = cached.get(&id) {
            *kind
        } else {
            let state = world
                .state(id)
                .context("POI refresh block state is unavailable")?;
            validate_state(state)?;
            let kind = poi::for_state(state);
            cached.insert(id, kind);
            kind
        };
        if let Some(kind) = kind {
            let x = base_x
                .checked_add((index % 16) as i32)
                .context("POI block X overflow")?;
            let z = base_z
                .checked_add(((index / 16) % 16) as i32)
                .context("POI block Z overflow")?;
            let y = base_y + (index / 256) as i32;
            discovered.push(([x, y, z], kind));
        }
    }
    if previous.is_none() && discovered.is_empty() {
        return Ok(PoiRefreshDraft {
            source: original.clone(),
            document: original,
            discarded_positions: BTreeSet::new(),
            changed: false,
        });
    }
    let mut document = original.clone().unwrap_or_else(|| NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![
            ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
            ("Sections".into(), Tag::Compound(vec![])),
        ]),
    });
    let Tag::Compound(root) = &mut document.value else {
        unreachable!()
    };
    let Tag::Compound(sections) = &mut root
        .iter_mut()
        .find(|(name, _)| name.matches("Sections"))
        .unwrap()
        .1
    else {
        unreachable!()
    };
    let name = section_y.to_string();
    let section = match sections.iter().position(|(key, _)| key.matches(&name)) {
        Some(index) => &mut sections[index].1,
        None => {
            sections.push((name.as_str().into(), Tag::Compound(vec![])));
            &mut sections.last_mut().unwrap().1
        }
    };
    let Tag::Compound(fields) = section else {
        unreachable!()
    };
    let mut old = BTreeMap::new();
    if let Some(previous) = previous {
        let Tag::List { values, .. } = &fields
            .iter()
            .find(|(key, _)| key.matches("Records"))
            .unwrap()
            .1
        else {
            unreachable!()
        };
        for (record, raw) in previous.records.iter().zip(values) {
            old.insert(record.position, raw.clone());
        }
    }
    let mut records = Vec::with_capacity(discovered.len());
    for (position, kind) in discovered {
        records.push(old.remove(&position).unwrap_or_else(|| {
            Tag::Compound(vec![
                ("pos".into(), Tag::IntArray(position.to_vec())),
                ("type".into(), Tag::String(kind.id.into())),
                ("free_tickets".into(), Tag::Int(i32::from(kind.max_tickets))),
            ])
        }));
    }
    let discarded_positions = old.into_keys().collect();
    let replacement = Tag::List {
        element_type: if records.is_empty() {
            TagType::End
        } else {
            TagType::Compound
        },
        values: records,
    };
    if let Some((_, value)) = fields.iter_mut().find(|(key, _)| key.matches("Records")) {
        *value = replacement;
    } else {
        fields.push(("Records".into(), replacement));
    }
    if let Some((_, value)) = fields.iter_mut().find(|(key, _)| key.matches("Valid")) {
        *value = Tag::Byte(1);
    } else {
        fields.push(("Valid".into(), Tag::Byte(1)));
    }
    decode_poi_chunk(document.clone(), position)?;
    Ok(PoiRefreshDraft {
        source: original,
        document: Some(document),
        discarded_positions,
        changed: true,
    })
}

/// This document is an incomplete draft; it is not a write authorization.
pub struct PoiDraft {
    source: NamedTag,
    document: NamedTag,
    position: ChunkPos,
    changed_positions: BTreeSet<[i32; 3]>,
}

impl PoiDraft {
    pub fn source(&self) -> &NamedTag {
        &self.source
    }
    pub fn document(&self) -> &NamedTag {
        &self.document
    }
    pub fn position(&self) -> ChunkPos {
        self.position
    }
    pub fn changed_positions(&self) -> &BTreeSet<[i32; 3]> {
        &self.changed_positions
    }
}

/// Stage known block transitions against an existing valid POI section.
///
/// `validate_state` must reject unregistered or noncanonical block states.
/// This does not validate terrain against `expected`, simulate villager ticket
/// changes, rebuild missing or invalid sections, or permit an on-disk write.
pub fn draft_block_changes(
    source: &SavedPoiChunk,
    changes: &[PoiBlockChange],
    mut validate_state: impl FnMut(&BlockState) -> Result<()>,
) -> Result<PoiDraft> {
    let original = (*source.document).clone();
    let mut decoded = decode_poi_chunk(original.clone(), source.position)?;
    let mut document = original.clone();
    let mut visited = BTreeSet::new();
    let mut changed_positions = BTreeSet::new();
    for change in changes {
        let [x, y, z] = change.position;
        ensure!(
            x.div_euclid(16) == source.position.x && z.div_euclid(16) == source.position.z,
            "POI change is outside its chunk"
        );
        ensure!(
            visited.insert(change.position),
            "duplicate POI block change"
        );
        validate_state(&change.expected)?;
        validate_state(&change.replacement)?;
        let Some((before, after)) = poi::change(&change.expected, &change.replacement) else {
            continue;
        };
        let section_y = y.div_euclid(16);
        let section = decoded
            .sections
            .get_mut(&section_y)
            .context("POI section is missing and must be rebuilt")?;
        ensure!(section.valid, "POI section is invalid and must be rebuilt");
        let index = section
            .records
            .iter()
            .position(|record| record.position == change.position);
        match (before, index) {
            (None, None) => {}
            (Some(kind), Some(index)) if section.records[index].type_id.matches(kind.id) => {}
            _ => bail!("existing POI does not match the expected block state"),
        }
        let Tag::Compound(root) = &mut document.value else {
            unreachable!()
        };
        let Tag::Compound(sections) = &mut root
            .iter_mut()
            .find(|(key, _)| key.matches("Sections"))
            .unwrap()
            .1
        else {
            unreachable!()
        };
        let section_name = section_y.to_string();
        let Tag::Compound(raw_section) = &mut sections
            .iter_mut()
            .find(|(key, _)| key.matches(&section_name))
            .unwrap()
            .1
        else {
            unreachable!()
        };
        let Tag::List {
            element_type,
            values,
        } = &mut raw_section
            .iter_mut()
            .find(|(key, _)| key.matches("Records"))
            .unwrap()
            .1
        else {
            unreachable!()
        };
        if let Some(index) = index {
            values.remove(index);
            section.records.remove(index);
        }
        if let Some(kind) = after {
            *element_type = TagType::Compound;
            values.push(Tag::Compound(vec![
                ("pos".into(), Tag::IntArray(change.position.to_vec())),
                ("type".into(), Tag::String(kind.id.into())),
                ("free_tickets".into(), Tag::Int(i32::from(kind.max_tickets))),
            ]));
            section.records.push(SavedPoiRecord {
                position: change.position,
                type_id: kind.id.into(),
                free_tickets: i32::from(kind.max_tickets),
            });
        }
        changed_positions.insert(change.position);
    }
    // Revalidate the assembled document before exposing it to callers.
    decode_poi_chunk(document.clone(), source.position)?;
    Ok(PoiDraft {
        source: original,
        document,
        position: source.position,
        changed_positions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{nbt::TagType, world::TARGET_DATA_VERSION};
    use native_world::loaded::{self, BLOCK_COUNT};
    use std::sync::Arc;

    fn state(id: &str, properties: &[(&str, &str)]) -> BlockState {
        BlockState {
            name: format!("minecraft:{id}"),
            properties: properties
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
        }
    }
    fn source(valid: bool) -> SavedPoiChunk {
        let record = |x, kind: &str, tickets| {
            Tag::Compound(vec![
                ("pos".into(), Tag::IntArray(vec![x, 4, 0])),
                ("type".into(), Tag::String(kind.into())),
                ("free_tickets".into(), Tag::Int(tickets)),
                ("future".into(), Tag::String("retained".into())),
            ])
        };
        decode_poi_chunk(
            NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![
                    ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                    ("unrecognized".into(), Tag::Int(77)),
                    (
                        "Sections".into(),
                        Tag::Compound(vec![(
                            "0".into(),
                            Tag::Compound(vec![
                                ("Valid".into(), Tag::Byte(i8::from(valid))),
                                ("future_section".into(), Tag::Int(88)),
                                (
                                    "Records".into(),
                                    Tag::List {
                                        element_type: TagType::Compound,
                                        values: vec![
                                            record(0, "minecraft:home", 0),
                                            record(1, "minecraft:farmer", 1),
                                        ],
                                    },
                                ),
                            ]),
                        )]),
                    ),
                ]),
            },
            ChunkPos { x: 0, z: 0 },
        )
        .unwrap()
    }
    fn edit(x: i32, old: BlockState, new: BlockState) -> PoiBlockChange {
        PoiBlockChange {
            position: [x, 4, 0],
            expected: old,
            replacement: new,
        }
    }
    #[test]
    fn changes_preserve_other_records_and_fail_atomically() {
        let source = source(true);
        let before = (*source.document).clone();
        let head = state("red_bed", &[("part", "head")]);
        let air = state("air", &[]);
        let farmer = state("composter", &[]);
        let bell = state("bell", &[]);
        let changes = [
            edit(0, head.clone(), air.clone()),
            edit(2, air.clone(), bell.clone()),
        ];
        let draft = draft_block_changes(&source, &changes, |_| Ok(())).unwrap();
        assert_eq!(*source.document, before);
        assert_eq!(draft.source(), &before);
        assert_eq!(draft.position(), source.position);
        assert_eq!(draft.changed_positions().len(), 2);
        let result = decode_poi_chunk(draft.document().clone(), source.position).unwrap();
        let records = &result.sections[&0].records;
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].position, [1, 4, 0]);
        assert_eq!(records[0].free_tickets, 1);
        assert_eq!(records[1].position, [2, 4, 0]);
        assert_eq!(records[1].free_tickets, 32);
        assert!(records[1].type_id.matches("minecraft:meeting"));
        let raw = draft
            .document()
            .value
            .get("Sections")
            .unwrap()
            .get("0")
            .unwrap();
        assert_eq!(raw.get("future_section"), Some(&Tag::Int(88)));
        let Some(Tag::List { values, .. }) = raw.get("Records") else {
            panic!()
        };
        assert_eq!(
            values[0].get("future"),
            Some(&Tag::String("retained".into()))
        );
        assert!(draft_block_changes(&source, &[edit(0, farmer, air.clone())], |_| Ok(())).is_err());
        assert!(
            draft_block_changes(
                &source,
                &[
                    edit(0, head.clone(), air.clone()),
                    edit(0, head.clone(), bell.clone())
                ],
                |_| Ok(())
            )
            .is_err()
        );
        assert!(
            draft_block_changes(
                &source,
                &[edit(3, air.clone(), bell.clone())],
                |_| anyhow::bail!("unknown state")
            )
            .is_err()
        );
        assert!(
            draft_block_changes(&source, &[edit(3, air.clone(), bell.clone())], |_| Ok(())).is_ok()
        );
        assert!(
            draft_block_changes(&self::source(false), &[edit(3, air, bell)], |_| Ok(())).is_err()
        );
        assert_eq!(*source.document, before);
    }

    #[test]
    fn unchanged_type_keeps_poi_tickets_and_original_bytes() {
        let source = source(true);
        let old = state("red_bed", &[("part", "head"), ("occupied", "false")]);
        let new = state("red_bed", &[("part", "head"), ("occupied", "true")]);
        let draft = draft_block_changes(&source, &[edit(0, old, new)], |_| Ok(())).unwrap();
        assert!(draft.changed_positions().is_empty());
        assert_eq!(draft.document(), &*source.document);
        assert_eq!(source.sections[&0].records[0].free_tickets, 0);
    }

    fn terrain(pois: &[(usize, &str)]) -> loaded::World {
        let mut world = loaded::World::new("minecraft:overworld");
        let air = world.intern_state(state("air", &[]));
        let mut blocks = Box::new([air; BLOCK_COUNT]);
        for (x, name) in pois {
            let id = world.intern_state(state(name, &[]));
            blocks[x + 4 * 256] = id;
        }
        let position = loaded::ChunkPos { x: 0, z: 0 };
        world.chunks.insert(
            position,
            Arc::new(loaded::Chunk {
                position,
                status: Some("minecraft:full".into()),
                block_light_correct: false,
                sections: BTreeMap::from([(
                    0,
                    Arc::new(loaded::Section {
                        blocks: Some(blocks),
                        biomes: None,
                        block_light: None,
                        sky_light: None,
                    }),
                )]),
            }),
        );
        world
    }

    #[test]
    fn refresh_matches_java_position_retention_and_validity_rules() {
        let original = source(false);
        let original_document = (*original.document).clone();
        let world = terrain(&[(0, "bell"), (2, "bell")]);
        let mut validations = 0;
        let draft = refresh_section(
            Some(&original),
            original.position,
            0,
            &world,
            |_| {
                validations += 1;
                Ok(())
            },
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(validations, 2, "each distinct state is validated once");
        assert!(draft.changed);
        assert_eq!(draft.discarded_positions, BTreeSet::from([[1, 4, 0]]));
        assert_eq!(draft.source.as_ref(), Some(&*original.document));
        let decoded = decode_poi_chunk(draft.document.unwrap(), original.position).unwrap();
        let section = &decoded.sections[&0];
        assert!(section.valid);
        assert_eq!(section.records.len(), 2);
        assert!(
            section.records[0].type_id.matches("minecraft:home"),
            "Java retains the old type at this position"
        );
        assert_eq!(section.records[0].free_tickets, 0);
        assert!(section.records[1].type_id.matches("minecraft:meeting"));
        assert_eq!(section.records[1].free_tickets, 32);
        let valid = source(true);
        let draft = refresh_section(
            Some(&valid),
            valid.position,
            0,
            &loaded::World::default(),
            |_| unreachable!(),
            |_| Ok(()),
        )
        .unwrap();
        assert!(!draft.changed);
        assert_eq!(draft.document.as_ref(), Some(&*valid.document));
        assert!(
            refresh_section(
                Some(&valid),
                valid.position,
                0,
                &loaded::World::default(),
                |_| unreachable!(),
                |_| anyhow::bail!("unknown type")
            )
            .is_err()
        );
        assert!(
            refresh_section(
                Some(&original),
                original.position,
                0,
                &world,
                |_| Ok(()),
                |_| anyhow::bail!("unrecognized POI type")
            )
            .is_err()
        );
        assert!(
            refresh_section(
                Some(&original),
                original.position,
                0,
                &world,
                |_| anyhow::bail!("unknown block"),
                |_| Ok(())
            )
            .is_err()
        );
        assert!(
            refresh_section(
                Some(&original),
                original.position,
                1,
                &world,
                |_| Ok(()),
                |_| Ok(())
            )
            .is_err()
        );
        assert_eq!(*original.document, original_document);
    }

    #[test]
    fn missing_section_is_only_created_when_blocks_contain_a_poi() {
        let pos = ChunkPos { x: 0, z: 0 };
        let empty =
            refresh_section(None, pos, 0, &terrain(&[]), |_| Ok(()), |_| unreachable!()).unwrap();
        assert!(!empty.changed);
        assert!(empty.document.is_none());
        let created = refresh_section(
            None,
            pos,
            0,
            &terrain(&[(2, "bell")]),
            |_| Ok(()),
            |_| unreachable!(),
        )
        .unwrap();
        assert!(created.changed);
        let section = decode_poi_chunk(created.document.unwrap(), pos)
            .unwrap()
            .sections
            .remove(&0)
            .unwrap();
        assert!(section.valid);
        assert_eq!(section.records[0].position, [2, 4, 0]);
        assert_eq!(section.records[0].free_tickets, 32);
    }
}
