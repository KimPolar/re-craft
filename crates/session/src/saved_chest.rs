//! Read saved chest halves and stage inventory changes while retaining NBT.
//! Loot-table generation and coordinated world saving are not implemented here.

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

/// Validate item-carried contents without recursively resolving item prototypes.
fn container_entries(container: &Tag) -> Result<Vec<(usize, &Tag)>> {
    let Tag::List {
        element_type,
        values,
    } = container
    else {
        bail!("container component is not a list")
    };
    ensure!(
        *element_type == TagType::Compound || values.is_empty(),
        "container entries are not compounds"
    );
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for entry in values {
        let Tag::Compound(fields) = entry else {
            bail!("container entry is not a compound")
        };
        ensure!(
            fields
                .iter()
                .all(|(name, _)| name.matches("slot") || name.matches("item")),
            "unknown container entry field"
        );
        let index = match unique(entry, "slot")? {
            Some(Tag::Int(value)) if (0..=255).contains(value) => *value as usize,
            _ => bail!("invalid container slot"),
        };
        ensure!(seen.insert(index), "duplicate container slot");
        let item = unique(entry, "item")?.context("container item is missing")?;
        let Tag::Compound(fields) = item else {
            bail!("container item is not a compound")
        };
        ensure!(
            fields.iter().all(|(name, _)| ["id", "count", "components"]
                .iter()
                .any(|key| name.matches(key))),
            "unknown container item field"
        );
        let Some(Tag::String(id)) = unique(item, "id")? else {
            bail!("container item id is missing")
        };
        ResourceId::parse(&String::from_utf16(&id.0).context("invalid item id")?)?;
        ensure!(
            matches!(unique(item, "count")?, None | Some(Tag::Int(1..=99))),
            "invalid container item count"
        );
        ensure!(
            matches!(unique(item, "components")?, None | Some(Tag::Compound(_))),
            "invalid container item components"
        );
        result.push((index, item));
    }
    Ok(result)
}

pub(crate) fn placed_item_components(components: &Tag) -> Result<(Option<&Tag>, Option<&Tag>)> {
    let Tag::Compound(fields) = components else {
        bail!("item components are not a compound")
    };
    let mut name = None;
    let mut container = None;
    for (key, value) in fields {
        if key.matches("minecraft:custom_name")
            && name.is_none()
            && matches!(value, Tag::String(_) | Tag::Compound(_) | Tag::List { .. })
        {
            name = Some(value);
        } else if key.matches("minecraft:container") && container.is_none() {
            container_entries(value)?;
            container = Some(value);
        } else {
            bail!("unsupported or duplicate chest item component")
        }
    }
    Ok((name, container))
}

// NbtUtils.compareNbt(pattern, value, true): compounds are subsets and
// nonempty lists match without regard to order (but cannot be longer).
fn matches_partial_nbt(pattern: &Tag, value: &Tag) -> bool {
    match (pattern, value) {
        (Tag::Compound(expected), Tag::Compound(actual)) => {
            expected.iter().enumerate().all(|(i, (key, wanted))| {
                expected[i + 1..].iter().any(|(later, _)| later == key)
                    || actual
                        .iter()
                        .rev()
                        .find(|(name, _)| name == key)
                        .is_some_and(|(_, got)| matches_partial_nbt(wanted, got))
            })
        }
        (
            Tag::List {
                values: expected, ..
            },
            Tag::List { values: actual, .. },
        ) => {
            if expected.is_empty() {
                actual.is_empty()
            } else {
                expected.len() <= actual.len()
                    && expected
                        .iter()
                        .all(|wanted| actual.iter().any(|got| matches_partial_nbt(wanted, got)))
            }
        }
        _ => crate::inventory::equal_tag(pattern, value),
    }
}

impl SavedChest {
    pub(crate) fn placed(
        position: IVec3,
        id: &str,
        custom_name: Option<Tag>,
        container: Option<&Tag>,
    ) -> Result<Self> {
        let mut slots: [Option<ItemStack>; 27] = std::array::from_fn(|_| None);
        let mut items = Vec::new();
        if let Some(container) = container {
            for (index, item) in container_entries(container)? {
                let decoded = crate::player_data::read_stack(item)?;
                if index < slots.len() {
                    slots[index] = Some(decoded);
                    let Tag::Compound(mut fields) = item.clone() else {
                        unreachable!()
                    };
                    fields.push(("Slot".into(), Tag::Byte(index as i8)));
                    items.push(Tag::Compound(fields));
                }
            }
        }
        let mut fields = vec![
            ("id".into(), Tag::String(id.into())),
            ("x".into(), Tag::Int(position.x)),
            ("y".into(), Tag::Int(position.y)),
            ("z".into(), Tag::Int(position.z)),
        ];
        if let Some(name) = custom_name {
            fields.push(("CustomName".into(), name));
        }
        if !items.is_empty() {
            fields.push((
                "Items".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: items,
                },
            ));
        }
        Ok(Self {
            position,
            slots: slots.clone(),
            original_slots: slots,
            source: Arc::new(Tag::Compound(fields)),
        })
    }
    pub fn source(&self) -> &Tag {
        &self.source
    }

    /// Preserve the original text component, including forms not yet rendered.
    pub fn custom_name(&self) -> Option<&Tag> {
        self.source.get("CustomName")
    }

    /// Interpret the supported subset of 26.3's lock item predicate.
    /// Unknown predicates remain unavailable rather than being treated as open.
    pub(crate) fn can_unlock(
        &self,
        item: Option<&ItemStack>,
        spectator: bool,
        tags: Option<&native_resources::recipes::ItemTags>,
        registry: Option<&native_resources::item_registry::ItemRegistry>,
    ) -> Result<bool> {
        if spectator {
            return Ok(true);
        }
        let Some(lock) = unique(&self.source, "lock")? else {
            return Ok(true);
        };
        let Tag::Compound(fields) = lock else {
            bail!("unsupported lock predicate")
        };
        ensure!(
            fields.iter().all(|(name, _)| name.matches("items")
                || name.matches("count")
                || name.matches("components")
                || name.matches("predicates")),
            "unsupported lock predicate field"
        );
        if let Some(items) = unique(lock, "items")? {
            if let Tag::String(value) = items {
                let name = String::from_utf16(&value.0).context("invalid lock item identifier")?;
                if let Some(tag) = name.strip_prefix('#') {
                    let tag = ResourceId::parse(tag)?;
                    let tags = tags.context("active item tags are unavailable")?;
                    let registry = registry.context("active item registry is unavailable")?;
                    let held = item.map_or("minecraft:air", ItemStack::id);
                    if !tags
                        .checked_contains(&tag, held, |id| registry.id(id).is_some())
                        .context("item tag is unavailable or invalid")?
                    {
                        return Ok(false);
                    }
                } else {
                    let id = ResourceId::parse(&name)?;
                    let id = format!("{}:{}", id.namespace, id.path);
                    if !item.map_or(id == "minecraft:air", |item| item.id() == id) {
                        return Ok(false);
                    }
                }
            } else {
                let choices = match items {
                    Tag::List {
                        element_type: TagType::String,
                        values,
                    } => values.as_slice(),
                    _ => bail!("unsupported lock item set"),
                };
                let mut matched = false;
                for choice in choices {
                    let Tag::String(id) = choice else {
                        bail!("invalid lock item")
                    };
                    let id = String::from_utf16(&id.0).context("invalid lock item identifier")?;
                    ensure!(!id.starts_with('#'), "item tag locks are unsupported");
                    let id = ResourceId::parse(&id)?;
                    let id = format!("{}:{}", id.namespace, id.path);
                    matched |= item.map_or(id == "minecraft:air", |item| item.id() == id);
                }
                if !matched {
                    return Ok(false);
                }
            }
        }
        let count = item.map_or(0, |stack| stack.count() as i64);
        if let Some(range) = unique(lock, "count")? {
            let bound = |value: &Tag| -> Result<i64> {
                value
                    .as_java_int()
                    .map(i64::from)
                    .context("unsupported lock count")
            };
            let (min, max) = if let Tag::Compound(fields) = range {
                ensure!(
                    fields
                        .iter()
                        .all(|(name, _)| name.matches("min") || name.matches("max")),
                    "unsupported lock count field"
                );
                (
                    unique(range, "min")?.map(bound).transpose()?,
                    unique(range, "max")?.map(bound).transpose()?,
                )
            } else {
                let value = bound(range)?;
                (Some(value), Some(value))
            };
            ensure!(
                min.zip(max).is_none_or(|(min, max)| min <= max),
                "invalid lock count range"
            );
            if min.is_some_and(|min| count < min) || max.is_some_and(|max| count > max) {
                return Ok(false);
            }
        }
        let component = |name: &str| -> Result<Option<&Tag>> {
            let Some(components) = unique(lock, name)? else {
                return Ok(None);
            };
            let Tag::Compound(fields) = components else {
                bail!("invalid lock {name}")
            };
            ensure!(
                fields
                    .iter()
                    .all(|(key, _)| key.matches("minecraft:custom_data")),
                "unsupported lock {name}"
            );
            let expected = unique(components, "minecraft:custom_data")?;
            Ok(expected)
        };
        let exact = component("components")?;
        ensure!(
            exact.is_none_or(|value| matches!(value, Tag::Compound(_))),
            "invalid exact custom data"
        );
        let partial = component("predicates")?
            .map(|value| -> Result<std::borrow::Cow<'_, Tag>> {
                match value {
                    Tag::Compound(_) => Ok(std::borrow::Cow::Borrowed(value)),
                    Tag::String(text) => Ok(std::borrow::Cow::Owned(
                        native_persistence::snbt::parse_compound(text)?,
                    )),
                    _ => bail!("invalid custom data predicate"),
                }
            })
            .transpose()?;
        if exact.is_some() || partial.is_some() {
            // Verified built-in prototypes have no custom_data; a patch can add
            // or remove it. An unrecognized item has an unknown prototype.
            let held = if let Some(item) = item {
                let registry = registry.context("item registry is unavailable")?;
                ensure!(registry.id(item.id()).is_some(), "unknown item prototype");
                let value = unique(item.components(), "minecraft:custom_data")?;
                let removed = unique(item.components(), "!minecraft:custom_data")?;
                ensure!(
                    value.is_none() || removed.is_none(),
                    "conflicting custom data patch"
                );
                ensure!(
                    value.is_none_or(|value| matches!(value, Tag::Compound(_))),
                    "invalid item custom data"
                );
                value
            } else {
                None
            };
            if let Some(expected) = exact {
                if !held.is_some_and(|value| crate::inventory::equal_tag(expected, value)) {
                    return Ok(false);
                }
            }
            if let Some(expected) = partial {
                let empty = Tag::Compound(vec![]);
                if !matches_partial_nbt(&expected, held.unwrap_or(&empty)) {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    pub(crate) fn inventory_changed(&self) -> bool {
        self.slots != self.original_slots
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
    unique(&entity, "CustomName")?;
    ensure!(
        unique(&entity, "LootTable")?.is_none(),
        "chest loot table must be generated first"
    );
    unique(&entity, "lock")?;
    unique(&entity, "Lock")?;
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

    #[test]
    fn partial_custom_data_uses_subset_and_unordered_list_matching() {
        let parse = |text: &str| native_persistence::snbt::parse_compound(&text.into()).unwrap();
        let actual = parse("{nested:{a:1,b:2},list:[2,1],other:1}");
        assert!(matches_partial_nbt(
            &parse("{nested:{a:1},list:[1,2]}"),
            &actual
        ));
        assert!(matches_partial_nbt(&parse("{list:[1,1]}"), &actual));
        assert!(!matches_partial_nbt(&parse("{list:[1,1,1]}"), &actual));
        assert!(!matches_partial_nbt(&parse("{list:[]}"), &actual));
        assert!(!matches_partial_nbt(&parse("{nested:{a:1b}}"), &actual));
        assert!(matches_partial_nbt(&parse("{}"), &actual));
    }
    use native_persistence::nbt::NamedTag;
    #[test]
    fn placed_container_rejects_ambiguous_or_unpreservable_items() {
        let item = Tag::Compound(vec![("id".into(), Tag::String("minecraft:stone".into()))]);
        let entry = |slot: i32| {
            Tag::Compound(vec![
                ("slot".into(), Tag::Int(slot)),
                ("item".into(), item.clone()),
            ])
        };
        let component = |values| Tag::List {
            element_type: TagType::Compound,
            values,
        };
        for invalid in [
            component(vec![entry(4), entry(4)]),
            component(vec![entry(-1)]),
            component(vec![entry(256)]),
            component(vec![Tag::Compound(vec![
                ("slot".into(), Tag::Int(4)),
                ("item".into(), item.clone()),
                ("future".into(), Tag::Int(1)),
            ])]),
            component(vec![Tag::Compound(vec![
                ("slot".into(), Tag::Int(4)),
                (
                    "item".into(),
                    Tag::Compound(vec![
                        ("id".into(), Tag::String("minecraft:stone".into())),
                        ("future".into(), Tag::Int(1)),
                    ]),
                ),
            ])]),
        ] {
            assert!(
                SavedChest::placed(IVec3::ZERO, "minecraft:chest", None, Some(&invalid)).is_err()
            );
        }
        let outside = SavedChest::placed(
            IVec3::ZERO,
            "minecraft:chest",
            None,
            Some(&component(vec![entry(255)])),
        )
        .unwrap();
        assert!(outside.slots.iter().all(Option::is_none));
    }
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
    fn rejects_unavailable_loot_and_ambiguous_slots() {
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
        let locked = read(&payload(vec![locked]), chunk, position)
            .unwrap()
            .unwrap();
        assert!(locked.can_unlock(None, false, None, None).unwrap());
        let mut unsupported = chest();
        let Tag::Compound(fields) = &mut unsupported else {
            unreachable!()
        };
        fields.push((
            "lock".into(),
            Tag::Compound(vec![(
                "components".into(),
                Tag::Compound(vec![("minecraft:damage".into(), Tag::Int(1))]),
            )]),
        ));
        let unsupported = read(&payload(vec![unsupported]), chunk, position)
            .unwrap()
            .unwrap();
        assert!(unsupported.can_unlock(None, false, None, None).is_err());
        assert!(unsupported.can_unlock(None, true, None, None).unwrap());
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

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn lock_item_and_count_predicates_match_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let registry = resources.vanilla_item_registry().unwrap();
        let recipes = native_resources::recipes::CraftingRecipes::load(&mut resources).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let classpath = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("ChestLockOracle.java");
        let output = temp.path().join("locks.nbt");
        std::fs::write(&source, include_str!("../tests/java/ChestLockOracle.java")).unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&classpath)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&classpath)
                .arg("ChestLockOracle")
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "oracle failed: {}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let oracle = nbt::read(&std::fs::read(output).unwrap()).unwrap();
        let Tag::List { values, .. } = oracle.value.get("cases").unwrap() else {
            panic!()
        };
        let stack =
            |id: &str, count| ItemStack::new(id.into(), count, 64, Tag::Compound(vec![])).unwrap();
        let hands = [
            None,
            Some(stack("minecraft:diamond", 1)),
            Some(stack("minecraft:diamond", 3)),
            Some(stack("minecraft:stone", 2)),
            Some(stack("minecraft:oak_planks", 1)),
        ];
        for (case, row) in values.iter().enumerate() {
            let mut original = chest();
            let Tag::Compound(fields) = &mut original else {
                panic!()
            };
            fields.push(("lock".into(), row.get("lock").unwrap().clone()));
            let saved = read(
                &payload(vec![original]),
                ChunkPos { x: -1, z: 2 },
                IVec3::new(-1, 64, 32),
            )
            .unwrap()
            .unwrap();
            let Tag::ByteArray(expected) = row.get("answers").unwrap() else {
                panic!()
            };
            for (hand, &answer) in hands.iter().zip(expected) {
                assert_eq!(
                    saved
                        .can_unlock(hand.as_ref(), false, Some(&recipes.tags), Some(&registry))
                        .unwrap_or_else(|error| panic!(
                            "case {case}, lock {:?}: {error:#}",
                            row.get("lock")
                        )),
                    answer != 0,
                    "case {case} hand {hand:?}"
                );
                assert!(
                    saved
                        .can_unlock(hand.as_ref(), true, Some(&recipes.tags), Some(&registry))
                        .unwrap()
                );
            }
        }
        let Tag::List { values: stacks, .. } = oracle.value.get("component_stacks").unwrap() else {
            panic!()
        };
        let mut component_hands = vec![None];
        component_hands.extend(
            stacks
                .iter()
                .map(|stack| Some(crate::player_data::read_stack(stack).unwrap())),
        );
        let Tag::List { values: rows, .. } = oracle.value.get("component_cases").unwrap() else {
            panic!()
        };
        for (case, row) in rows.iter().enumerate() {
            let mut original = chest();
            let Tag::Compound(fields) = &mut original else {
                panic!()
            };
            fields.push(("lock".into(), row.get("lock").unwrap().clone()));
            let saved = read(
                &payload(vec![original]),
                ChunkPos { x: -1, z: 2 },
                IVec3::new(-1, 64, 32),
            )
            .unwrap()
            .unwrap();
            let Tag::ByteArray(expected) = row.get("answers").unwrap() else {
                panic!()
            };
            for (hand, &answer) in component_hands.iter().zip(expected) {
                assert_eq!(
                    saved
                        .can_unlock(hand.as_ref(), false, Some(&recipes.tags), Some(&registry))
                        .unwrap_or_else(|error| panic!(
                            "component case {case}, lock {:?}: {error:#}",
                            row.get("lock")
                        )),
                    answer != 0,
                    "component case {case} hand {hand:?}, lock {:?}",
                    row.get("lock")
                );
            }
        }
    }
}
