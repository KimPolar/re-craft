//! Read saved single-chest inventories while retaining the original NBT.
//! Container interaction and loot-table generation are not implemented here.

use crate::inventory::ItemStack;
use anyhow::{Context, Result, bail, ensure};
use glam::IVec3;
use native_persistence::nbt::{self, Tag, TagType};
use native_world::loaded::{ChunkPos, SavedBlockEntities};
use std::{collections::BTreeSet, sync::Arc};

#[derive(Clone, Debug)]
pub struct SavedChest {
    pub position: IVec3,
    pub slots: [Option<ItemStack>; 27],
    source: Arc<Tag>,
}

impl SavedChest {
    pub fn source(&self) -> &Tag {
        &self.source
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
        matches!(unique(&entity, "id")?, Some(Tag::String(id)) if id.matches("minecraft:chest") || id.matches("chest")),
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
