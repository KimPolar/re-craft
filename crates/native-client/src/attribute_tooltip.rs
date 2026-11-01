//! Item attribute modifier tooltip lines.

use crate::{
    menu_assets::MenuAssets,
    text_component::{Span, Style},
};
use native_persistence::nbt::Tag;
use native_session::inventory::ItemStack;

struct Modifier {
    attribute: String,
    id: String,
    slot: String,
    amount: f64,
    operation: u8,
    hidden: bool,
    override_text: Option<Tag>,
}

fn modifier(attribute: &str, id: &str, slot: &str, amount: f64) -> Modifier {
    Modifier {
        attribute: attribute.into(),
        id: id.into(),
        slot: slot.into(),
        amount,
        operation: 0,
        hidden: false,
        override_text: None,
    }
}

fn default_modifiers(id: &str) -> Vec<Modifier> {
    let Some(id) = id.strip_prefix("minecraft:") else {
        return vec![];
    };
    let (material, kind) = id.split_once('_').unwrap_or((id, ""));
    let tier = match material {
        "wooden" | "golden" => Some(0),
        "stone" | "copper" => Some(1),
        "iron" => Some(2),
        "diamond" => Some(3),
        "netherite" => Some(4),
        _ => None,
    };
    let tool = tier
        .and_then(|tier| {
            let tier_damage = [0.0, 1.0, 2.0, 3.0, 4.0][tier];
            Some(match kind {
                "sword" => (tier_damage + 3.0, f64::from(-2.4_f32)),
                "shovel" => (tier_damage + 1.5, -3.0),
                "pickaxe" => (tier_damage + 1.0, f64::from(-2.8_f32)),
                "axe" => (
                    match material {
                        "wooden" | "golden" => 6.0,
                        "netherite" => 9.0,
                        _ => 8.0,
                    },
                    f64::from(match material {
                        "wooden" | "stone" | "copper" => -3.2_f32,
                        "iron" => -3.1_f32,
                        _ => -3.0_f32,
                    }),
                ),
                "hoe" => (
                    0.0,
                    match tier {
                        0 => -3.0,
                        1 => -2.0,
                        2 => -1.0,
                        _ => 0.0,
                    },
                ),
                "spear" => (
                    tier_damage,
                    match material {
                        "wooden" => -2.4615384340286255,
                        "stone" => -2.666666626930237,
                        "copper" => -2.8235294818878174,
                        "iron" | "golden" => -2.9473683834075928,
                        "diamond" => -3.0476189851760864,
                        _ => -3.13043475151062,
                    },
                ),
                _ => return None,
            })
        })
        .or_else(|| match id {
            "mace" => Some((5.0, f64::from(-3.4_f32))),
            "trident" => Some((8.0, f64::from(-2.9_f32))),
            _ => None,
        });
    if let Some((damage, speed)) = tool {
        return vec![
            modifier("attack_damage", "base_attack_damage", "mainhand", damage),
            modifier("attack_speed", "base_attack_speed", "mainhand", speed),
        ];
    }
    let armor_index = match kind {
        "helmet" => Some((0, "head", "helmet")),
        "chestplate" => Some((1, "chest", "chestplate")),
        "leggings" => Some((2, "legs", "leggings")),
        "boots" => Some((3, "feet", "boots")),
        _ => None,
    };
    let (slot, armor, toughness, knockback, name) = if let Some((index, slot, name)) = armor_index {
        let values = match material {
            "leather" => [1, 3, 2, 1],
            "copper" => [2, 4, 3, 1],
            "chainmail" => [2, 5, 4, 1],
            "iron" => [2, 6, 5, 2],
            "golden" => [2, 5, 3, 1],
            "diamond" | "netherite" => [3, 8, 6, 3],
            "turtle" if kind == "helmet" => [2, 0, 0, 0],
            _ => return vec![],
        };
        (
            slot,
            values[index],
            if material == "diamond" {
                2
            } else if material == "netherite" {
                3
            } else {
                0
            },
            material == "netherite",
            name,
        )
    } else if id == "wolf_armor" {
        ("body", 11, 0, false, "body")
    } else if matches!(kind, "horse_armor" | "nautilus_armor") {
        let value = match material {
            "leather" if kind == "horse_armor" => 3,
            "copper" => 4,
            "iron" => 5,
            "golden" => 7,
            "diamond" => 11,
            "netherite" => 19,
            _ => return vec![],
        };
        (
            "body",
            value,
            if material == "diamond" {
                2
            } else if material == "netherite" {
                3
            } else {
                0
            },
            material == "netherite",
            "body",
        )
    } else {
        return vec![];
    };
    let id = format!("armor.{name}");
    let mut out = vec![
        modifier("armor", &id, slot, f64::from(armor)),
        modifier("armor_toughness", &id, slot, f64::from(toughness)),
    ];
    if knockback {
        out.push(modifier(
            "knockback_resistance",
            &id,
            slot,
            f64::from(0.1_f32),
        ));
    }
    out
}

fn string(tag: &Tag) -> Option<String> {
    let Tag::String(value) = tag else { return None };
    String::from_utf16(&value.0).ok()
}
fn number(tag: &Tag) -> Option<f64> {
    match tag {
        Tag::Byte(v) => Some(f64::from(*v)),
        Tag::Short(v) => Some(f64::from(*v)),
        Tag::Int(v) => Some(f64::from(*v)),
        Tag::Long(v) => Some(*v as f64),
        Tag::Float(v) => Some(f64::from(*v)),
        Tag::Double(v) => Some(*v),
        _ => None,
    }
    .filter(|v| v.is_finite())
}
fn parse_modifier(tag: &Tag) -> Option<Modifier> {
    let attribute = string(tag.get("type")?)?;
    let attribute = if let Some(attribute) = attribute.strip_prefix("minecraft:") {
        attribute.to_owned()
    } else if attribute.contains(':') {
        return None;
    } else {
        attribute
    };
    let id = string(tag.get("id")?)?;
    let id = id.strip_prefix("minecraft:").unwrap_or(&id).to_owned();
    let slot = tag
        .get("slot")
        .and_then(string)
        .unwrap_or_else(|| "any".into());
    let amount = number(tag.get("amount")?)?;
    let operation = match string(tag.get("operation")?)?.as_str() {
        "add_value" => 0,
        "add_multiplied_base" => 1,
        "add_multiplied_total" => 2,
        _ => return None,
    };
    let display = tag.get("display");
    let display_type = display
        .and_then(|v| v.get("type"))
        .and_then(string)
        .unwrap_or_else(|| "default".into());
    let (hidden, override_text) = match display_type.as_str() {
        "default" => (false, None),
        "hidden" => (true, None),
        "override" => (false, Some(display?.get("value")?.clone())),
        _ => return None,
    };
    Some(Modifier {
        attribute,
        id,
        slot,
        amount,
        operation,
        hidden,
        override_text,
    })
}

fn color(attribute: &str, positive: bool) -> [u8; 3] {
    if matches!(
        attribute,
        "gravity" | "scale" | "waypoint_transmit_range" | "waypoint_receive_range"
    ) {
        [170, 170, 170]
    } else if positive ^ matches!(attribute, "burning_time" | "fall_damage_multiplier") {
        [85, 85, 255]
    } else {
        [255, 85, 85]
    }
}

fn line(text: String, color: [u8; 3]) -> Vec<Span> {
    vec![Span {
        text,
        style: Style {
            color: Some(color),
            ..Default::default()
        },
    }]
}
fn decimal(value: f64) -> String {
    let mut output = format!("{value:.2}");
    if output.contains('.') {
        while output.ends_with('0') {
            output.pop();
        }
        if output.ends_with('.') {
            output.pop();
        }
    }
    output
}

pub fn lines(item: &ItemStack, assets: &MenuAssets, base: Option<[f64; 2]>) -> Vec<Vec<Span>> {
    let modifiers = match item.components().get("minecraft:attribute_modifiers") {
        Some(Tag::List { values, .. }) => values.iter().filter_map(parse_modifier).collect(),
        Some(_) => Vec::new(),
        None if item
            .components()
            .get("!minecraft:attribute_modifiers")
            .is_some() =>
        {
            Vec::new()
        }
        None => default_modifiers(item.id()),
    };
    let mut output = Vec::new();
    for slot in [
        "any", "mainhand", "offhand", "hand", "feet", "legs", "chest", "head", "armor", "body",
        "saddle",
    ] {
        let mut header = false;
        for modifier in modifiers
            .iter()
            .filter(|entry| entry.slot == slot && !entry.hidden)
        {
            if !header {
                output.push(vec![]);
                let key = format!("item.modifiers.{slot}");
                output.push(line(assets.translate(&key, &key).into(), [170, 170, 170]));
                header = true;
            }
            if let Some(text) = &modifier.override_text {
                if let Some(spans) = crate::text_component::resolve(text, |key, fallback| {
                    assets.translate(key, fallback).to_owned()
                }) {
                    output.push(spans);
                }
                continue;
            }
            let mut amount = modifier.amount;
            let equals = match modifier.id.as_str() {
                "base_attack_damage" => {
                    if let Some(base) = base {
                        amount += base[0];
                        true
                    } else {
                        false
                    }
                }
                "base_attack_speed" => {
                    if let Some(base) = base {
                        amount += base[1];
                        true
                    } else {
                        false
                    }
                }
                _ => false,
            };
            let displayed = if modifier.operation != 0 {
                amount * 100.0
            } else if modifier.attribute == "knockback_resistance" {
                amount * 10.0
            } else {
                amount
            };
            if !equals && amount == 0.0 {
                continue;
            }
            let category = if equals {
                "equals"
            } else if amount > 0.0 {
                "plus"
            } else {
                "take"
            };
            let key = format!("attribute.modifier.{category}.{}", modifier.operation);
            let attribute = format!("attribute.name.{}", modifier.attribute);
            let formatted = crate::potion_tooltip::formatted(
                assets,
                &key,
                &[
                    &decimal(if amount < 0.0 && !equals {
                        -displayed
                    } else {
                        displayed
                    }),
                    assets.translate(&attribute, &attribute),
                ],
            );
            let text = if equals {
                format!(" {formatted}")
            } else {
                formatted
            };
            output.push(line(
                text,
                if equals {
                    [0, 170, 0]
                } else {
                    color(&modifier.attribute, amount > 0.0)
                },
            ));
        }
    }
    output
}
