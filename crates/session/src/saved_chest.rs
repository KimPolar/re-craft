//! Read saved single-chest inventories while retaining the original NBT.
//! Container interaction and loot-table generation are not implemented here.

use crate::inventory::ItemStack;
use anyhow::{Context, Result, bail, ensure};
use glam::IVec3;
use native_persistence::nbt::{self, Tag, TagType};
use native_resources::packs::ResourceId;
use native_world::loaded::{ChunkPos, SavedBlockEntities};
use std::{collections::BTreeSet, sync::Arc};

#[derive(Clone, Debug)]
pub struct SavedChest {
    pub position: IVec3,
    pub slots: [Option<ItemStack>; 27],
    original_slots: [Option<ItemStack>; 27],
    source: Arc<Tag>,
}

impl SavedChest {
    pub fn source(&self) -> &Tag {
        &self.source
    }

    /// Prepare a chest entity in memory. The source, player, chunk and all
    /// other affected world data must be checked and committed together before
    /// this can be used for a world save.
    pub fn draft_inventory(&self, slots: &[Option<ItemStack>; 27]) -> Result<Tag> {
        if slots == &self.original_slots {
            return Ok((*self.source).clone());
        }
        let original_items = match unique(&self.source, "Items")? {
            Some(Tag::List { values, .. }) => values.as_slice(),
            None => &[],
            _ => bail!("chest Items is not a list"),
        };
        let mut source_by_slot: [Option<&Tag>; 27] = [None; 27];
        for item in original_items {
            let Some(Tag::Byte(slot)) = unique(item, "Slot")? else {
                bail!("chest item slot is invalid")
            };
            let index = *slot as u8 as usize;
            ensure!(
                index < 27 && source_by_slot[index].is_none(),
                "invalid or duplicate chest item slot"
            );
            source_by_slot[index] = Some(item);
            if slots[index] != self.original_slots[index] {
                ensure!(
                    self.original_slots[index]
                        .as_ref()
                        .is_none_or(|item| item.max_count().is_some()),
                    "an unresolved item would be lost or moved"
                );
                crate::player_data::validate_original_stack(item, true)?;
            }
        }
        let mut values = Vec::new();
        for (index, item) in slots.iter().enumerate() {
            let Some(item) = item else { continue };
            ensure!(
                (1..=99).contains(&item.count()),
                "item count is outside the saved codec range"
            );
            ResourceId::parse(item.id()).context("invalid saved item id")?;
            ensure!(
                matches!(item.components(), Tag::Compound(_)),
                "item components are not a compound"
            );
            if item.max_count().is_none() {
                ensure!(
                    self.original_slots[index].as_ref() == Some(item),
                    "an unresolved item was created or modified"
                );
            } else {
                ensure!(
                    crate::equippable::saved_inventory_max(item.id(), item.components())
                        == item.max_count(),
                    "item stack limit cannot be reconstructed from its saved components"
                );
            }
            if self.original_slots[index].as_ref() == Some(item) {
                values.push(
                    source_by_slot[index]
                        .context("unchanged item is missing from the source")?
                        .clone(),
                );
            } else {
                values.push(crate::player_data::encode_stack(item, Some(index as u8)));
            }
        }
        let mut document = (*self.source).clone();
        let Tag::Compound(fields) = &mut document else {
            unreachable!()
        };
        let items = Tag::List {
            element_type: TagType::Compound,
            values,
        };
        if let Some((_, original)) = fields.iter_mut().find(|(name, _)| name.matches("Items")) {
            *original = items;
        } else {
            fields.push(("Items".into(), items));
        }
        Ok(document)
    }
}

fn unique<'a>(tag: &'a Tag, name: &str) -> Result<Option<&'a Tag>> {
    let Tag::Compound(fields) = tag else {
        bail!("block entity or item is not a compound")
    };
    let mut matches = fields.iter().filter(|(key, _)| key.matches(name));
    let value = matches.next().map(|(_, value)| value);
    ensure!(matches.next().is_none(), "duplicate {name} fields");
    Ok(value)
}

/// An unavailable chunk, malformed coordinates, pending loot, or an
/// unrecognized item is not interpreted as an empty container.
pub fn read(
    payload: &SavedBlockEntities,
    chunk: ChunkPos,
    position: IVec3,
) -> Result<Option<SavedChest>> {
    ensure!(
        position.x.div_euclid(16) == chunk.x && position.z.div_euclid(16) == chunk.z,
        "chest is outside the chunk"
    );
    let bytes = match payload {
        SavedBlockEntities::Empty => return Ok(None),
        SavedBlockEntities::Unavailable => bail!("block entity data is unavailable"),
        SavedBlockEntities::Encoded(bytes) => bytes,
    };
    let document = nbt::read(bytes)?;
    let Tag::List {
        element_type: TagType::Compound,
        values,
    } = document.value
    else {
        bail!("block entities are not a compound list")
    };
    let mut positions = BTreeSet::new();
    let mut found = None;
    for entity in values {
        let coord = |name| -> Result<i32> {
            match unique(&entity, name)? {
                Some(Tag::Int(value)) => Ok(*value),
                _ => bail!("invalid block entity {name}"),
            }
        };
        let p = IVec3::new(coord("x")?, coord("y")?, coord("z")?);
        ensure!(
            p.x.div_euclid(16) == chunk.x && p.z.div_euclid(16) == chunk.z,
            "block entity outside chunk"
        );
        ensure!(
            positions.insert(p.to_array()),
            "duplicate block entity position"
        );
        if p == position {
            found = Some(entity);
        }
    }
    let Some(entity) = found else { return Ok(None) };
    ensure!(
        matches!(unique(&entity, "id")?, Some(Tag::String(id)) if id.matches("minecraft:chest") || id.matches("chest") || id.matches("minecraft:trapped_chest") || id.matches("trapped_chest")),
        "saved block entity is not a chest"
    );
    ensure!(
        unique(&entity, "LootTable")?.is_none(),
        "chest loot table must be generated first"
    );
    ensure!(
        unique(&entity, "lock")?.is_none() && unique(&entity, "Lock")?.is_none(),
        "locked chest interaction is unsupported"
    );
    let mut slots: [Option<ItemStack>; 27] = std::array::from_fn(|_| None);
    if let Some(items) = unique(&entity, "Items")? {
        let Tag::List {
            element_type,
            values,
        } = items
        else {
            bail!("chest Items is not a list")
        };
        ensure!(
            *element_type == TagType::Compound || values.is_empty(),
            "chest Items is not a compound list"
        );
        for item in values {
            for key in ["Slot", "id", "count", "components"] {
                unique(item, key)?;
            }
            let Some(Tag::Byte(slot)) = unique(item, "Slot")? else {
                bail!("chest item slot is invalid")
            };
            let index = *slot as u8 as usize;
            ensure!(index < 27, "chest item slot is outside the container");
            ensure!(slots[index].is_none(), "duplicate chest item slot");
            slots[index] =
                Some(crate::player_data::read_stack(item).context("invalid chest item")?);
        }
    }
    Ok(Some(SavedChest {
        position,
        original_slots: slots.clone(),
        slots,
        source: Arc::new(entity),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_persistence::nbt::NamedTag;
    fn chest() -> Tag {
        Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:chest".into())),
            ("x".into(), Tag::Int(-1)),
            ("y".into(), Tag::Int(64)),
            ("z".into(), Tag::Int(32)),
            (
                "future".into(),
                Tag::Compound(vec![("preserve".into(), Tag::Int(9))]),
            ),
            (
                "Items".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![
                        Tag::Compound(vec![
                            ("Slot".into(), Tag::Byte(0)),
                            ("id".into(), Tag::String("minecraft:diamond".into())),
                            ("count".into(), Tag::Int(3)),
                        ]),
                        Tag::Compound(vec![
                            ("Slot".into(), Tag::Byte(26)),
                            ("id".into(), Tag::String("example:unknown".into())),
                            ("count".into(), Tag::Int(2)),
                            (
                                "components".into(),
                                Tag::Compound(vec![("example:data".into(), Tag::Int(3))]),
                            ),
                        ]),
                    ],
                },
            ),
        ])
    }
    fn payload(entries: Vec<Tag>) -> SavedBlockEntities {
        let bytes = nbt::write(&NamedTag {
            name: "".into(),
            value: Tag::List {
                element_type: TagType::Compound,
                values: entries,
            },
        })
        .unwrap();
        SavedBlockEntities::Encoded(bytes.into())
    }
    #[test]
    fn reads_inventory_and_preserves_unknown_data() {
        let original = chest();
        let saved = read(
            &payload(vec![original.clone()]),
            ChunkPos { x: -1, z: 2 },
            IVec3::new(-1, 64, 32),
        )
        .unwrap()
        .unwrap();
        assert_eq!(saved.source(), &original);
        assert_eq!(
            (
                saved.slots[0].as_ref().unwrap().id(),
                saved.slots[0].as_ref().unwrap().count()
            ),
            ("minecraft:diamond", 3)
        );
        let unknown = saved.slots[26].as_ref().unwrap();
        assert_eq!(unknown.id(), "example:unknown");
        assert!(unknown.max_count().is_none());
        assert!(saved.slots[1..26].iter().all(Option::is_none));
    }
    #[test]
    fn drafts_changed_slots_preserving_other_fields_and_unchanged_items() {
        let mut original = chest();
        let Tag::Compound(fields) = &mut original else {
            unreachable!()
        };
        let Tag::List { values, .. } = &mut fields
            .iter_mut()
            .find(|(key, _)| key.matches("Items"))
            .unwrap()
            .1
        else {
            unreachable!()
        };
        let Tag::Compound(unknown_fields) = &mut values[1] else {
            unreachable!()
        };
        unknown_fields.push(("future_item_field".into(), Tag::Int(44)));
        let unchanged_unknown = values[1].clone();
        let saved = read(
            &payload(vec![original.clone()]),
            ChunkPos { x: -1, z: 2 },
            IVec3::new(-1, 64, 32),
        )
        .unwrap()
        .unwrap();
        assert_eq!(saved.draft_inventory(&saved.slots).unwrap(), original);
        let mut changed = saved.slots.clone();
        changed[5] = changed[0].take();
        let draft = saved.draft_inventory(&changed).unwrap();
        assert_eq!(draft.get("future"), original.get("future"));
        let Tag::List { values, .. } = draft.get("Items").unwrap() else {
            unreachable!()
        };
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].get("Slot"), Some(&Tag::Byte(5)));
        assert_eq!(values[1], unchanged_unknown);
        let reread = read(
            &payload(vec![draft]),
            ChunkPos { x: -1, z: 2 },
            IVec3::new(-1, 64, 32),
        )
        .unwrap()
        .unwrap();
        assert_eq!(reread.slots, changed);
        changed[26] = None;
        assert!(saved.draft_inventory(&changed).is_err());
        changed[26] = saved.slots[26].clone();
        changed[4] =
            Some(ItemStack::unresolved("example:new".into(), 1, Tag::Compound(vec![])).unwrap());
        assert!(saved.draft_inventory(&changed).is_err());
    }

    #[test]
    fn rejects_changes_that_would_discard_extra_item_fields_or_invalid_stacks() {
        let mut original = chest();
        let Tag::Compound(fields) = &mut original else {
            unreachable!()
        };
        let Tag::List { values, .. } = &mut fields
            .iter_mut()
            .find(|(key, _)| key.matches("Items"))
            .unwrap()
            .1
        else {
            unreachable!()
        };
        let Tag::Compound(item) = &mut values[0] else {
            unreachable!()
        };
        item.push(("future_item_field".into(), Tag::Int(44)));
        let saved = read(
            &payload(vec![original]),
            ChunkPos { x: -1, z: 2 },
            IVec3::new(-1, 64, 32),
        )
        .unwrap()
        .unwrap();
        let mut changed = saved.slots.clone();
        changed[5] = changed[0].take();
        assert!(saved.draft_inventory(&changed).is_err());
        changed = saved.slots.clone();
        changed[5] = ItemStack::new("minecraft:diamond".into(), 100, 64, Tag::Compound(vec![]));
        assert!(saved.draft_inventory(&changed).is_err());
        changed[5] = ItemStack::new("minecraft:diamond".into(), 1, 1, Tag::Compound(vec![]));
        assert!(saved.draft_inventory(&changed).is_err());
    }
    #[test]
    fn rejects_unavailable_loot_locks_and_ambiguous_slots() {
        let position = IVec3::new(-1, 64, 32);
        let chunk = ChunkPos { x: -1, z: 2 };
        assert!(read(&SavedBlockEntities::Unavailable, chunk, position).is_err());
        assert!(
            read(&SavedBlockEntities::Empty, chunk, position)
                .unwrap()
                .is_none()
        );
        let mut loot = chest();
        let Tag::Compound(fields) = &mut loot else {
            unreachable!()
        };
        fields.push((
            "LootTable".into(),
            Tag::String("minecraft:chests/simple_dungeon".into()),
        ));
        assert!(read(&payload(vec![loot]), chunk, position).is_err());
        let mut locked = chest();
        let Tag::Compound(fields) = &mut locked else {
            unreachable!()
        };
        fields.push(("lock".into(), Tag::Compound(vec![])));
        assert!(read(&payload(vec![locked]), chunk, position).is_err());
        assert!(read(&payload(vec![chest(), chest()]), chunk, position).is_err());
        let mut duplicate = chest();
        let Tag::Compound(fields) = &mut duplicate else {
            unreachable!()
        };
        let Tag::List { values, .. } = &mut fields
            .iter_mut()
            .find(|(name, _)| name.matches("Items"))
            .unwrap()
            .1
        else {
            unreachable!()
        };
        values.push(values[0].clone());
        assert!(read(&payload(vec![duplicate]), chunk, position).is_err());
    }
}
