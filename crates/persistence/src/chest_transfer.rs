//! Validate conserved player/chest items before a coordinated partial save.
use crate::{
    edit::CheckedChestInventory,
    nbt::{self, NamedTag, Tag, TagType},
    world::DimensionId,
};
use anyhow::{Context, Result, bail, ensure};
use std::collections::{BTreeMap, BTreeSet};

fn unique<'a>(value: &'a Tag, name: &str) -> Result<Option<&'a Tag>> {
    let Tag::Compound(fields) = value else {
        bail!("inventory parent is not a compound")
    };
    let mut entries = fields.iter().filter(|(key, _)| key.matches(name));
    let result = entries.next().map(|(_, value)| value);
    ensure!(entries.next().is_none(), "duplicate inventory field {name}");
    Ok(result)
}

// Compound order is not significant to NBT equality; reject duplicate names
// rather than guessing which value an unknown component codec would retain.
fn canonical(tag: &Tag) -> Result<Tag> {
    Ok(match tag {
        Tag::Compound(fields) => {
            let mut result = Vec::with_capacity(fields.len());
            for (name, value) in fields {
                result.push((name.clone(), canonical(value)?));
            }
            result.sort_by(|a, b| a.0.0.cmp(&b.0.0));
            ensure!(
                !result.windows(2).any(|pair| pair[0].0 == pair[1].0),
                "duplicate component field"
            );
            Tag::Compound(result)
        }
        Tag::List {
            element_type,
            values,
        } => Tag::List {
            element_type: *element_type,
            values: values.iter().map(canonical).collect::<Result<_>>()?,
        },
        other => other.clone(),
    })
}

fn add_item(
    totals: &mut BTreeMap<Vec<u8>, i64>,
    item: &Tag,
    with_slot: bool,
    direction: i64,
) -> Result<()> {
    let Tag::Compound(fields) = item else {
        bail!("inventory item is not a compound")
    };
    ensure!(
        fields.iter().all(|(name, _)| ["id", "count", "components"]
            .iter()
            .any(|key| name.matches(key))
            || (with_slot && name.matches("Slot"))),
        "unsupported inventory item field"
    );
    let Some(Tag::String(id)) = unique(item, "id")? else {
        bail!("inventory item id is missing")
    };
    let id = String::from_utf16(&id.0).context("invalid inventory item id")?;
    let id = DimensionId::parse(&id).context("invalid inventory item identifier")?;
    ensure!(
        id.namespace != "minecraft" || id.path != "air",
        "empty inventory item"
    );
    let count = match unique(item, "count")? {
        None => 1,
        Some(Tag::Int(count @ 1..=99)) => i64::from(*count),
        _ => bail!("invalid inventory item count"),
    };
    let components = match unique(item, "components")? {
        None => Tag::Compound(Vec::new()),
        Some(value @ Tag::Compound(_)) => canonical(value)?,
        _ => bail!("invalid inventory item components"),
    };
    let id = format!("{}:{}", id.namespace, id.path);
    let key = nbt::write(&NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![
            ("id".into(), Tag::String(id.as_str().into())),
            ("components".into(), components),
        ]),
    })?;
    let value = totals.entry(key).or_default();
    *value = value
        .checked_add(direction * count)
        .context("inventory item total overflow")?;
    Ok(())
}

fn add_list(
    totals: &mut BTreeMap<Vec<u8>, i64>,
    value: Option<&Tag>,
    slots: u8,
    direction: i64,
) -> Result<()> {
    let Some(value) = value else { return Ok(()) };
    let Tag::List {
        element_type,
        values,
    } = value
    else {
        bail!("inventory is not a list")
    };
    ensure!(
        *element_type == TagType::Compound || values.is_empty(),
        "invalid inventory list type"
    );
    let mut seen = BTreeSet::new();
    for item in values {
        let Some(Tag::Byte(slot)) = unique(item, "Slot")? else {
            bail!("invalid inventory slot")
        };
        let slot = *slot as u8;
        ensure!(
            slot < slots && seen.insert(slot),
            "duplicate or unsupported inventory slot"
        );
        add_item(totals, item, true, direction)?;
    }
    Ok(())
}

fn add_player(
    totals: &mut BTreeMap<Vec<u8>, i64>,
    player: &NamedTag,
    direction: i64,
) -> Result<()> {
    add_list(totals, unique(&player.value, "Inventory")?, 36, direction)?;
    if let Some(equipment) = unique(&player.value, "equipment")? {
        for slot in ["head", "chest", "legs", "feet", "offhand"] {
            if let Some(item) = unique(equipment, slot)? {
                add_item(totals, item, false, direction)?;
            }
        }
    }
    Ok(())
}

fn extra_equipment(player: &NamedTag) -> Result<Vec<(nbt::JavaString, Tag)>> {
    let Some(equipment) = unique(&player.value, "equipment")? else {
        return Ok(Vec::new());
    };
    let Tag::Compound(fields) = equipment else {
        bail!("equipment is not a compound")
    };
    let mut seen = BTreeSet::new();
    for (key, _) in fields {
        ensure!(seen.insert(key.0.clone()), "duplicate equipment slot");
    }
    Ok(fields
        .iter()
        .filter(|(key, _)| {
            !["head", "chest", "legs", "feet", "offhand"]
                .iter()
                .any(|slot| key.matches(slot))
        })
        .cloned()
        .collect())
}

pub(crate) fn validate(
    source: &NamedTag,
    replacement: &NamedTag,
    uuid: [i32; 4],
    changes: &[&CheckedChestInventory],
) -> Result<()> {
    for player in [source, replacement] {
        ensure!(
            matches!(unique(&player.value, "UUID")?, Some(Tag::IntArray(id)) if id.as_slice() == uuid),
            "player identity does not match chest transaction"
        );
    }
    ensure!(
        unique(&source.value, "EnderItems")? == unique(&replacement.value, "EnderItems")?,
        "ender chest inventory changed"
    );
    ensure!(
        extra_equipment(source)? == extra_equipment(replacement)?,
        "unrecognized equipment changed"
    );
    let mut totals = BTreeMap::new();
    add_player(&mut totals, source, 1)?;
    add_player(&mut totals, replacement, -1)?;
    for change in changes {
        let original = if let Some(proof) = &change.generated {
            Some(&proof.items)
        } else {
            unique(&change.change.expected, "Items")?
        };
        add_list(&mut totals, original, 27, 1)?;
        add_list(
            &mut totals,
            unique(&change.change.replacement, "Items")?,
            27,
            -1,
        )?;
    }
    ensure!(
        totals.values().all(|count| *count == 0),
        "player and chest items are not conserved"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::BlockEntityChange;

    fn stack(slot: u8, count: i32, components: Tag) -> Tag {
        Tag::Compound(vec![
            ("Slot".into(), Tag::Byte(slot as i8)),
            ("id".into(), Tag::String("minecraft:diamond".into())),
            ("count".into(), Tag::Int(count)),
            ("components".into(), components),
        ])
    }
    fn list(values: Vec<Tag>) -> Tag {
        Tag::List {
            element_type: TagType::Compound,
            values,
        }
    }
    fn player(items: Vec<Tag>) -> NamedTag {
        NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("UUID".into(), Tag::IntArray(vec![0, 0, 0, 1])),
                ("Inventory".into(), list(items)),
            ]),
        }
    }
    fn chest(items: Vec<Tag>) -> Tag {
        Tag::Compound(vec![("Items".into(), list(items))])
    }

    #[test]
    fn transfer_checks_counts_components_identity_and_other_inventories() {
        let components = Tag::Compound(vec![("b".into(), Tag::Int(2)), ("a".into(), Tag::Int(1))]);
        let reordered = Tag::Compound(vec![("a".into(), Tag::Int(1)), ("b".into(), Tag::Int(2))]);
        let source = player(vec![]);
        let replacement = player(vec![stack(4, 2, reordered.clone())]);
        let change = CheckedChestInventory {
            generated: None,
            change: BlockEntityChange {
                expected: chest(vec![stack(3, 5, components.clone())]),
                replacement: chest(vec![stack(7, 3, reordered)]),
            },
        };
        assert!(validate(&source, &replacement, [0, 0, 0, 1], &[&change]).is_ok());
        assert!(
            validate(
                &source,
                &player(vec![stack(4, 3, components.clone())]),
                [0, 0, 0, 1],
                &[&change]
            )
            .is_err()
        );
        assert!(validate(&source, &replacement, [0, 0, 0, 2], &[&change]).is_err());
        let altered = player(vec![stack(
            4,
            2,
            Tag::Compound(vec![("a".into(), Tag::Int(9))]),
        )]);
        assert!(validate(&source, &altered, [0, 0, 0, 1], &[&change]).is_err());
        let duplicate = player(vec![
            stack(4, 1, components.clone()),
            stack(4, 1, components),
        ]);
        assert!(validate(&source, &duplicate, [0, 0, 0, 1], &[&change]).is_err());
        let mut altered = replacement.clone();
        let Tag::Compound(fields) = &mut altered.value else {
            unreachable!()
        };
        fields.push(("EnderItems".into(), list(vec![])));
        assert!(validate(&source, &altered, [0, 0, 0, 1], &[&change]).is_err());
    }
}
