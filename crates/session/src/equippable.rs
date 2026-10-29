//! Player equipment-slot behavior for known vanilla items and saved components.

use native_persistence::nbt::Tag;
use native_resources::{enchantments::EquipmentSlot, packs::ResourceId};

use crate::inventory::{
    ItemStack, ordinary_item_default_durability, ordinary_item_default_max, saved_item_max,
};

/// Known 26.3 item prototypes, independent of a player's component patch.
pub fn vanilla_equipment(id: &str) -> Option<(EquipmentSlot, u32, Option<u32>)> {
    let name = id.strip_prefix("minecraft:")?;
    match name {
        "carved_pumpkin"
        | "skeleton_skull"
        | "wither_skeleton_skull"
        | "player_head"
        | "zombie_head"
        | "creeper_head"
        | "dragon_head"
        | "piglin_head" => {
            return Some((EquipmentSlot::Head, 64, None));
        }
        "turtle_helmet" => return Some((EquipmentSlot::Head, 1, Some(275))),
        "elytra" => return Some((EquipmentSlot::Chest, 1, Some(432))),
        "shield" => return Some((EquipmentSlot::Offhand, 1, Some(336))),
        _ => {}
    }
    let (material, kind) = name.split_once('_')?;
    let multiplier = match material {
        "leather" => 5,
        "golden" => 7,
        "copper" => 11,
        "chainmail" | "iron" => 15,
        "diamond" => 33,
        "netherite" => 37,
        _ => return None,
    };
    let (slot, factor) = match kind {
        "helmet" => (EquipmentSlot::Head, 11),
        "chestplate" => (EquipmentSlot::Chest, 16),
        "leggings" => (EquipmentSlot::Legs, 15),
        "boots" => (EquipmentSlot::Feet, 13),
        _ => return None,
    };
    Some((slot, 1, Some(multiplier * factor)))
}

fn string(tag: &Tag) -> Option<String> {
    let Tag::String(text) = tag else { return None };
    String::from_utf16(&text.0).ok()
}

fn parse_slot(tag: &Tag) -> Option<EquipmentSlot> {
    Some(match string(tag)?.as_str() {
        "head" => EquipmentSlot::Head,
        "chest" => EquipmentSlot::Chest,
        "legs" => EquipmentSlot::Legs,
        "feet" => EquipmentSlot::Feet,
        "offhand" => EquipmentSlot::Offhand,
        "mainhand" => EquipmentSlot::Mainhand,
        _ => return None,
    })
}

/// Infer a stack limit only when the item's ordinary inventory click behavior
/// and all of its saved component shapes are understood.
pub fn saved_inventory_max(id: &str, components: &Tag) -> Option<u32> {
    if let Some(max) = saved_item_max(id, components) {
        return Some(max);
    }
    let default = vanilla_equipment(id)
        .map(|entry| entry.1)
        .or_else(|| ordinary_item_default_max(id))?;
    let Tag::Compound(entries) = components else {
        return None;
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut max = default;
    let mut has_equippable_patch = false;
    for (key, value) in entries {
        let key = String::from_utf16(&key.0).ok()?;
        if !seen.insert(key.clone()) {
            return None;
        }
        match key.as_str() {
            "minecraft:max_stack_size" => {
                max = u32::try_from(value.as_java_int()?)
                    .ok()
                    .filter(|n| (1..=99).contains(n))?;
            }
            "minecraft:damage" if default == 1 && matches!(value, Tag::Int(n) if *n >= 0) => {}
            "minecraft:custom_data"
            | "minecraft:trim"
            | "minecraft:profile"
            | "minecraft:custom_model_data"
                if matches!(value, Tag::Compound(_)) => {}
            "minecraft:repair_cost" | "minecraft:dyed_color" if value.as_java_int().is_some() => {}
            "minecraft:unbreakable" if matches!(value, Tag::Compound(fields) if fields.is_empty()) =>
                {}
            "minecraft:custom_name" | "minecraft:item_name" | "minecraft:lore" => {}
            "minecraft:attribute_modifiers" if matches!(value, Tag::List { .. }) => {}
            "!minecraft:attribute_modifiers" if matches!(value, Tag::Compound(fields) if fields.is_empty()) =>
                {}
            "minecraft:enchantments" if valid_enchantments(value) => {}
            "minecraft:enchantment_glint_override" if matches!(value, Tag::Byte(0 | 1)) => {}
            "!minecraft:enchantment_glint_override" if matches!(value, Tag::Compound(fields) if fields.is_empty()) =>
                {}
            "minecraft:equippable"
                if matches!(value, Tag::Compound(_))
                    && value.get("slot").and_then(parse_slot).is_some() =>
            {
                has_equippable_patch = true;
            }
            "!minecraft:equippable" if matches!(value, Tag::Compound(fields) if fields.is_empty()) =>
            {
                has_equippable_patch = true;
            }
            _ => return None,
        }
    }
    if seen.contains("minecraft:enchantment_glint_override")
        && seen.contains("!minecraft:enchantment_glint_override")
    {
        return None;
    }
    if default == 1 && max != 1 {
        return None;
    }
    if vanilla_equipment(id).is_none()
        && ordinary_item_default_durability(id).is_none()
        && !has_equippable_patch
    {
        return None;
    }
    Some(max)
}

fn valid_enchantments(value: &Tag) -> bool {
    let Tag::Compound(entries) = value else {
        return false;
    };
    let mut ids = std::collections::BTreeSet::new();
    entries.iter().all(|(name, level)| {
        let Ok(name) = String::from_utf16(&name.0) else {
            return false;
        };
        let Ok(id) = ResourceId::parse(&name) else {
            return false;
        };
        ids.insert(id)
            && level
                .as_java_int()
                .is_some_and(|level| (1..=255).contains(&level))
    })
}

/// Preferred slot comes from the effective equippable component; entity
/// restrictions are checked separately for `ArmorSlot.mayPlace`.
pub fn preferred_slot(item: &ItemStack) -> Option<EquipmentSlot> {
    if item.components().get("!minecraft:equippable").is_some() {
        return None;
    }
    if let Some(value) = item.components().get("minecraft:equippable") {
        return value.get("slot").and_then(parse_slot);
    }
    vanilla_equipment(item.id()).map(|entry| entry.0)
}

pub fn player_can_equip(item: &ItemStack, slot: EquipmentSlot) -> bool {
    if preferred_slot(item) != Some(slot) {
        return false;
    }
    let Some(value) = item.components().get("minecraft:equippable") else {
        return true;
    };
    let Some(allowed) = value.get("allowed_entities") else {
        return true;
    };
    let player = |value: &Tag| {
        string(value)
            .and_then(|id| ResourceId::parse(&id).ok())
            .is_some_and(|id| id.namespace == "minecraft" && id.path == "player")
    };
    match allowed {
        Tag::String(_) => player(allowed),
        Tag::List { values, .. } => values.iter().any(player),
        _ => false,
    }
}
