//! In-memory POI changes. A draft is not a complete or writable world save.

use std::collections::BTreeSet;

use anyhow::{Context, Result, bail, ensure};
use native_world::{loaded::BlockState, poi};

use crate::{
    nbt::{NamedTag, Tag, TagType},
    world::{ChunkPos, SavedPoiChunk, SavedPoiRecord, decode_poi_chunk},
};

pub struct PoiBlockChange {
    pub position: [i32; 3],
    pub expected: BlockState,
    pub replacement: BlockState,
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
}
