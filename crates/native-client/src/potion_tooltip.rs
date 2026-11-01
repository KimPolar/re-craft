//! The vanilla potion effects currently supported by inventory tooltips.

use crate::menu_assets::MenuAssets;
use crate::text_component::{Part, Span, Style};
use native_persistence::nbt::Tag;
use native_session::inventory::ItemStack;

#[derive(Clone, Copy)]
struct Effect {
    id: &'static str,
    duration: i32,
    amplifier: u8,
}

fn effects(id: &str) -> Option<Vec<Effect>> {
    let (variant, name) = if let Some(name) = id.strip_prefix("long_") {
        (1, name)
    } else if let Some(name) = id.strip_prefix("strong_") {
        (2, name)
    } else {
        (0, id)
    };
    let effect = |id, durations: [i32; 3], amplifier| {
        let duration = durations[variant];
        (duration >= 0).then_some(Effect {
            id,
            duration,
            amplifier,
        })
    };
    let amplifier = u8::from(variant == 2);
    let one = match name {
        "water" | "mundane" | "thick" | "awkward" if variant == 0 => return Some(vec![]),
        "night_vision" => effect("night_vision", [3600, 9600, -1], amplifier),
        "invisibility" => effect("invisibility", [3600, 9600, -1], amplifier),
        "leaping" => effect("jump_boost", [3600, 9600, 1800], amplifier),
        "fire_resistance" => effect("fire_resistance", [3600, 9600, -1], amplifier),
        "swiftness" => effect("speed", [3600, 9600, 1800], amplifier),
        "slowness" => effect(
            "slowness",
            [1800, 4800, 400],
            if variant == 2 { 3 } else { 0 },
        ),
        "turtle_master" => {
            let duration = [400, 800, 400][variant];
            return Some(vec![
                Effect {
                    id: "slowness",
                    duration,
                    amplifier: if variant == 2 { 5 } else { 3 },
                },
                Effect {
                    id: "resistance",
                    duration,
                    amplifier: if variant == 2 { 3 } else { 2 },
                },
            ]);
        }
        "water_breathing" => effect("water_breathing", [3600, 9600, -1], amplifier),
        "healing" => effect("instant_health", [1, -1, 1], amplifier),
        "harming" => effect("instant_damage", [1, -1, 1], amplifier),
        "poison" => effect("poison", [900, 1800, 432], amplifier),
        "regeneration" => effect("regeneration", [900, 1800, 450], amplifier),
        "strength" => effect("strength", [3600, 9600, 1800], amplifier),
        "weakness" => effect("weakness", [1800, 4800, -1], amplifier),
        "luck" => effect("luck", [6000, -1, -1], amplifier),
        "slow_falling" => effect("slow_falling", [1800, 4800, -1], amplifier),
        "wind_charged" => effect("wind_charged", [3600, -1, -1], amplifier),
        "weaving" => effect("weaving", [3600, -1, -1], amplifier),
        "oozing" => effect("oozing", [3600, -1, -1], amplifier),
        "infested" => effect("infested", [3600, -1, -1], amplifier),
        _ => None,
    }?;
    Some(vec![one])
}

fn effect_id(id: &str) -> Option<&'static str> {
    Some(match id.strip_prefix("minecraft:").unwrap_or(id) {
        "speed" => "speed",
        "slowness" => "slowness",
        "haste" => "haste",
        "mining_fatigue" => "mining_fatigue",
        "strength" => "strength",
        "instant_health" => "instant_health",
        "instant_damage" => "instant_damage",
        "jump_boost" => "jump_boost",
        "nausea" => "nausea",
        "regeneration" => "regeneration",
        "resistance" => "resistance",
        "fire_resistance" => "fire_resistance",
        "water_breathing" => "water_breathing",
        "invisibility" => "invisibility",
        "blindness" => "blindness",
        "night_vision" => "night_vision",
        "hunger" => "hunger",
        "weakness" => "weakness",
        "poison" => "poison",
        "wither" => "wither",
        "health_boost" => "health_boost",
        "absorption" => "absorption",
        "saturation" => "saturation",
        "glowing" => "glowing",
        "levitation" => "levitation",
        "luck" => "luck",
        "unluck" => "unluck",
        "slow_falling" => "slow_falling",
        "conduit_power" => "conduit_power",
        "dolphins_grace" => "dolphins_grace",
        "bad_omen" => "bad_omen",
        "hero_of_the_village" => "hero_of_the_village",
        "darkness" => "darkness",
        "trial_omen" => "trial_omen",
        "raid_omen" => "raid_omen",
        "wind_charged" => "wind_charged",
        "weaving" => "weaving",
        "oozing" => "oozing",
        "infested" => "infested",
        "breath_of_the_nautilus" => "breath_of_the_nautilus",
        _ => return None,
    })
}

fn custom_effect(tag: &Tag) -> Option<Effect> {
    let Tag::String(id) = tag.get("id")? else {
        return None;
    };
    let id = String::from_utf16(&id.0).ok()?;
    let duration = match tag.get("duration") {
        Some(value) => value.as_java_int()?,
        None => 0,
    };
    // The codec stores the amplifier as a signed byte and reads it unsigned.
    let amplifier = match tag.get("amplifier") {
        Some(value) => value.as_java_int()? as u8,
        None => 0,
    };
    Some(Effect {
        id: effect_id(&id)?,
        duration,
        amplifier,
    })
}

fn formatted(assets: &MenuAssets, key: &str, args: &[&str]) -> String {
    let template = assets.translate(key, key);
    let Some(parts) = crate::text_component::format_parts(template) else {
        return template.into();
    };
    let mut out = String::new();
    for part in parts {
        match part {
            Part::Text(text) => out.push_str(&text),
            Part::Argument(index) => match args.get(index) {
                Some(value) => out.push_str(value),
                None => return template.into(),
            },
        }
    }
    out
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

/// None means the active potion definition is unknown, not that it has no effects.
pub fn lines(item: &ItemStack, assets: &MenuAssets) -> Option<Vec<Vec<Span>>> {
    let contents = item.components().get("minecraft:potion_contents");
    let potion_item = matches!(
        item.id(),
        "minecraft:potion"
            | "minecraft:splash_potion"
            | "minecraft:lingering_potion"
            | "minecraft:tipped_arrow"
    );
    if contents.is_none()
        && (!potion_item
            || item
                .components()
                .get("!minecraft:potion_contents")
                .is_some())
    {
        return None;
    }
    let potion = contents.and_then(|contents| contents.get("potion"));
    let mut effects = match potion {
        None => vec![],
        Some(Tag::String(id)) => {
            let id = String::from_utf16(&id.0).ok()?;
            effects(id.strip_prefix("minecraft:").unwrap_or(&id))?
        }
        _ => return None,
    };
    if let Some(custom) = contents.and_then(|contents| contents.get("custom_effects")) {
        let Tag::List { values, .. } = custom else {
            return None;
        };
        for value in values {
            effects.push(custom_effect(value)?);
        }
    }
    if effects.is_empty() {
        return Some(vec![line(
            assets.translate("effect.none", "effect.none").into(),
            [170, 170, 170],
        )]);
    }
    let default_scale = match item.id() {
        "minecraft:lingering_potion" => 0.25,
        "minecraft:tipped_arrow" => 0.125,
        _ => 1.0,
    };
    let scale = match item.components().get("minecraft:potion_duration_scale") {
        Some(Tag::Float(value)) => *value,
        Some(Tag::Double(value)) => *value as f32,
        Some(value) => value.as_java_int().map(|value| value as f32)?,
        None if item
            .components()
            .get("!minecraft:potion_duration_scale")
            .is_some() =>
        {
            1.0
        }
        None => default_scale,
    };
    let mut lines = Vec::new();
    let mut attributes = Vec::new();
    for effect in effects {
        let harmful = matches!(
            effect.id,
            "slowness"
                | "instant_damage"
                | "poison"
                | "weakness"
                | "wind_charged"
                | "weaving"
                | "oozing"
                | "infested"
                | "mining_fatigue"
                | "nausea"
                | "blindness"
                | "hunger"
                | "wither"
                | "levitation"
                | "unluck"
                | "darkness"
        );
        let color = if harmful {
            [255, 85, 85]
        } else {
            [85, 85, 255]
        };
        let key = format!("effect.minecraft.{}", effect.id);
        let mut name = assets.translate(&key, &key).to_owned();
        if effect.amplifier > 0 {
            let key = format!("potion.potency.{}", effect.amplifier);
            name = formatted(
                assets,
                "potion.withAmplifier",
                &[&name, assets.translate(&key, &key)],
            );
        }
        if effect.duration > 20 || effect.duration == -1 {
            let duration = if effect.duration == -1 {
                assets
                    .translate("effect.duration.infinite", "effect.duration.infinite")
                    .to_owned()
            } else {
                let seconds = ((effect.duration as f32 * scale).floor() as i32).max(0) / 20;
                if seconds >= 3600 {
                    format!(
                        "{:02}:{:02}:{:02}",
                        seconds / 3600,
                        seconds / 60 % 60,
                        seconds % 60
                    )
                } else {
                    format!("{:02}:{:02}", seconds / 60, seconds % 60)
                }
            };
            name = formatted(assets, "potion.withDuration", &[&name, &duration]);
        }
        lines.push(line(name, color));
        let level = i32::from(effect.amplifier) + 1;
        let attribute = match effect.id {
            "speed" => Some(("movement_speed", 20 * level, true)),
            "slowness" => Some(("movement_speed", -15 * level, true)),
            "haste" => Some(("attack_speed", 10 * level, true)),
            "mining_fatigue" => Some(("attack_speed", -10 * level, true)),
            "jump_boost" => Some(("safe_fall_distance", level, false)),
            "strength" => Some(("attack_damage", 3 * level, false)),
            "weakness" => Some(("attack_damage", -4 * level, false)),
            "luck" => Some(("luck", level, false)),
            "unluck" => Some(("luck", -level, false)),
            "health_boost" => Some(("max_health", 4 * level, false)),
            "absorption" => Some(("max_absorption", 4 * level, false)),
            "invisibility" => Some(("waypoint_transmit_range", -100 * level, true)),
            _ => None,
        };
        if let Some((attribute, amount, percent)) = attribute {
            let key = format!("attribute.name.{attribute}");
            let translation = format!(
                "attribute.modifier.{}.{}",
                if amount < 0 { "take" } else { "plus" },
                u8::from(percent)
            );
            let negative = amount < 0;
            let amount = amount.abs().to_string();
            attributes.push(line(
                formatted(
                    assets,
                    &translation,
                    &[&amount, assets.translate(&key, &key)],
                ),
                if negative {
                    [255, 85, 85]
                } else {
                    [85, 85, 255]
                },
            ));
        }
    }
    if !attributes.is_empty() {
        lines.push(vec![]);
        lines.push(line(
            assets
                .translate("potion.whenDrank", "potion.whenDrank")
                .into(),
            [170, 0, 170],
        ));
        lines.extend(attributes);
    }
    Some(lines)
}
