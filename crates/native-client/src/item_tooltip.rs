//! Item tooltip text and component styling for the inventory.

use crate::menu_assets::MenuAssets;
use crate::text_component::{Span, Style};
use native_persistence::nbt::Tag;
use native_resources::item_registry::ItemRarity;
use native_session::inventory::ItemStack;

fn text(value: &Tag) -> Option<String> {
    if let Tag::String(value) = value {
        String::from_utf16(&value.0).ok()
    } else {
        None
    }
}

fn hidden(display: Option<&Tag>, component: &str) -> bool {
    display
        .and_then(|value| value.get("hidden_components"))
        .and_then(|value| {
            if let Tag::List { values, .. } = value {
                Some(values)
            } else {
                None
            }
        })
        .is_some_and(|values| {
            values.iter().any(|value| {
                text(value).is_some_and(|id| {
                    id.strip_prefix("minecraft:").unwrap_or(&id)
                        == component.strip_prefix("minecraft:").unwrap_or(component)
                })
            })
        })
}

fn rarity(item: &ItemStack, assets: &MenuAssets) -> ItemRarity {
    let components = item.components();
    let default = if components.get("!minecraft:rarity").is_some() {
        ItemRarity::Common
    } else {
        assets
            .item_registry
            .as_ref()
            .and_then(|registry| registry.default_rarity(item.id()))
            .unwrap_or(ItemRarity::Common)
    };
    let mut rarity = components
        .get("minecraft:rarity")
        .and_then(text)
        .and_then(|value| match value.as_str() {
            "common" => Some(ItemRarity::Common),
            "uncommon" => Some(ItemRarity::Uncommon),
            "rare" => Some(ItemRarity::Rare),
            "epic" => Some(ItemRarity::Epic),
            _ => None,
        })
        .unwrap_or(default);
    let enchanted = matches!(components.get("minecraft:enchantments"),Some(Tag::Compound(values)) if !values.is_empty());
    if enchanted {
        rarity = match rarity {
            ItemRarity::Common | ItemRarity::Uncommon => ItemRarity::Rare,
            ItemRarity::Rare => ItemRarity::Epic,
            ItemRarity::Epic => ItemRarity::Epic,
        };
    }
    rarity
}

fn potion_name(item: &ItemStack) -> Option<String> {
    let prefix = match item.id() {
        "minecraft:potion" => "item.minecraft.potion.effect.",
        "minecraft:splash_potion" => "item.minecraft.splash_potion.effect.",
        "minecraft:lingering_potion" => "item.minecraft.lingering_potion.effect.",
        "minecraft:tipped_arrow" => "item.minecraft.tipped_arrow.effect.",
        _ => return None,
    };
    let contents = item.components().get("minecraft:potion_contents");
    let value = contents
        .and_then(|contents| contents.get("custom_name"))
        .and_then(text)
        .or_else(|| {
            contents
                .and_then(|contents| contents.get("potion"))
                .and_then(text)
                .map(|id| {
                    let path = id.rsplit(':').next().unwrap_or(&id);
                    path.strip_prefix("long_")
                        .or_else(|| path.strip_prefix("strong_"))
                        .unwrap_or(path)
                        .to_owned()
                })
        })
        .unwrap_or_else(|| "empty".to_owned());
    Some(format!("{prefix}{value}"))
}

/// Supported title and lore lines. Additional item-specific detail lines are
/// still being implemented and must not be interpreted as complete tooltips.
pub fn lines(
    item: &ItemStack,
    assets: &MenuAssets,
    enchantments: &crate::enchantment_tooltip::Registry,
    attack_base: Option<[f64; 2]>,
) -> Vec<Vec<Span>> {
    let components = item.components();
    let display = components.get("minecraft:tooltip_display");
    if display
        .and_then(|tag| tag.get("hide_tooltip"))
        .and_then(Tag::as_java_int)
        .is_some_and(|v| v != 0)
    {
        return Vec::new();
    }
    let color = match rarity(item, assets) {
        ItemRarity::Common => [255, 255, 255],
        ItemRarity::Uncommon => [255, 255, 85],
        ItemRarity::Rare => [85, 255, 255],
        ItemRarity::Epic => [255, 85, 255],
    };
    let custom = components.get("minecraft:custom_name");
    let base = Style {
        color: Some(color),
        italic: custom.is_some(),
        ..Default::default()
    };
    let translate = |key: &str, fallback: &str| assets.translate(key, fallback).to_owned();
    let title = if let Some(custom) = custom {
        crate::text_component::resolve_with_style(custom, base, translate).unwrap_or_default()
    } else if let Some(key) = potion_name(item) {
        vec![Span {
            text: assets.translate(&key, &key).to_owned(),
            style: base,
        }]
    } else if let Some(name) = components.get("minecraft:item_name") {
        crate::text_component::resolve_with_style(name, base, translate).unwrap_or_default()
    } else if components.get("!minecraft:item_name").is_some() {
        Vec::new()
    } else {
        let key = assets
            .item_registry
            .as_ref()
            .and_then(|registry| registry.description_key(item.id()))
            .unwrap_or_else(|| format!("item.{}", item.id().replace(':', ".")));
        vec![Span {
            text: assets.translate(&key, &key).to_owned(),
            style: base,
        }]
    };
    let mut lines = vec![title];
    if !hidden(display, "minecraft:potion_contents")
        && let Some(potion_lines) = crate::potion_tooltip::lines(item, assets)
    {
        lines.extend(potion_lines);
    }
    for component in ["minecraft:stored_enchantments", "minecraft:enchantments"] {
        if !hidden(display, component)
            && let Some(value) = components.get(component)
        {
            lines.extend(enchantments.lines(value, assets));
        }
    }
    if !hidden(display, "minecraft:lore")
        && let Some(Tag::List { values, .. }) = components.get("minecraft:lore")
    {
        let lore_style = Style {
            color: Some([170, 0, 170]),
            italic: true,
            ..Default::default()
        };
        for value in values.iter().take(256) {
            if let Some(line) =
                crate::text_component::resolve_with_style(value, lore_style, translate)
            {
                lines.push(line);
            }
        }
    }
    if !hidden(display, "minecraft:attribute_modifiers") {
        lines.extend(crate::attribute_tooltip::lines(item, assets, attack_base));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_persistence::nbt::TagType;
    use std::{path::PathBuf, process::Command};

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn item_description_keys_match_java_registry() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("ItemTooltipOracle.java");
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/ItemTooltipOracle.java"),
        )
        .unwrap();
        std::fs::write(&input, b"[{\"all\":true}]").unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("ItemTooltipOracle")
                .arg(&input)
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let registry = native_resources::packs::ResourceStack::open(&game)
            .unwrap()
            .vanilla_item_registry()
            .unwrap();
        assert_eq!(rows.len(), registry.iter().count());
        let mismatches: Vec<_> = rows
            .iter()
            .filter_map(|row| {
                let id = row["id"].as_str().unwrap();
                let actual = registry.description_key(id);
                (actual.as_deref() != row["description"].as_str())
                    .then(|| format!("{id}: {actual:?} != {}", row["description"]))
            })
            .collect();
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
        let rarity_mismatches: Vec<_> = rows
            .iter()
            .filter_map(|row| {
                let id = row["id"].as_str().unwrap();
                let actual = registry
                    .default_rarity(id)
                    .map(|value| format!("{value:?}").to_ascii_uppercase());
                (actual.as_deref() != row["rarity"].as_str())
                    .then(|| format!("{id}: {actual:?} != {}", row["rarity"]))
            })
            .collect();
        assert!(
            rarity_mismatches.is_empty(),
            "{}",
            rarity_mismatches.join("\n")
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn item_titles_and_lore_match_java_components() {
        use native_resources::packs::ResourceProvider;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("ItemTooltipOracle.java");
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/ItemTooltipOracle.java"),
        )
        .unwrap();
        let mut cases = Vec::new();
        for id in [
            "stone",
            "apple",
            "diamond_sword",
            "nether_star",
            "sniffer_egg",
            "light",
            "resin_clump",
            "wheat",
            "nether_wart",
        ] {
            cases.push(serde_json::json!({"id":format!("minecraft:{id}")}));
            for rarity in ["COMMON", "UNCOMMON", "RARE", "EPIC"] {
                cases.push(serde_json::json!({"id":format!("minecraft:{id}"),"custom":"Named Item","rarity":rarity}));
                cases.push(serde_json::json!({"id":format!("minecraft:{id}"),"item_name":"Prototype Name","rarity":rarity}));
            }
        }
        cases.push(serde_json::json!({"id":"minecraft:apple","lore":["First line","Second line"]}));
        cases.push(serde_json::json!({"id":"minecraft:apple","custom":"Named","lore":["Lore"]}));
        cases.push(serde_json::json!({"id":"minecraft:stone","hide":true}));
        for id in [
            "potion",
            "splash_potion",
            "lingering_potion",
            "tipped_arrow",
        ] {
            cases.push(serde_json::json!({"id":format!("minecraft:{id}")}));
            cases.push(
                serde_json::json!({"id":format!("minecraft:{id}"),"item_name":"Prototype Name"}),
            );
        }
        for id in [
            "potion",
            "splash_potion",
            "lingering_potion",
            "tipped_arrow",
            "apple",
        ] {
            for potion in [
                "water",
                "mundane",
                "thick",
                "awkward",
                "night_vision",
                "long_night_vision",
                "invisibility",
                "long_invisibility",
                "leaping",
                "long_leaping",
                "strong_leaping",
                "fire_resistance",
                "long_fire_resistance",
                "swiftness",
                "long_swiftness",
                "strong_swiftness",
                "slowness",
                "long_slowness",
                "strong_slowness",
                "turtle_master",
                "long_turtle_master",
                "strong_turtle_master",
                "water_breathing",
                "long_water_breathing",
                "healing",
                "strong_healing",
                "harming",
                "strong_harming",
                "poison",
                "long_poison",
                "strong_poison",
                "regeneration",
                "long_regeneration",
                "strong_regeneration",
                "strength",
                "long_strength",
                "strong_strength",
                "weakness",
                "long_weakness",
                "luck",
                "slow_falling",
                "long_slow_falling",
                "wind_charged",
                "weaving",
                "oozing",
                "infested",
            ] {
                cases.push(serde_json::json!({"id":format!("minecraft:{id}"),"potion":potion}));
            }
        }
        for id in [
            "potion",
            "splash_potion",
            "lingering_potion",
            "tipped_arrow",
        ] {
            for scale in [0.0, 0.125, 0.5, 1.5, 2.0] {
                cases.push(serde_json::json!({"id":format!("minecraft:{id}"),"potion":"strong_swiftness","scale":scale}));
            }
        }
        cases.push(
            serde_json::json!({"id":"minecraft:potion","potion":"swiftness","lore":["Lore"]}),
        );
        cases.push(
            serde_json::json!({"id":"minecraft:potion","potion":"swiftness","hide_potion":true}),
        );
        cases.push(serde_json::json!({"id":"minecraft:potion","hide_potion":true}));
        for effect in [
            "speed",
            "slowness",
            "haste",
            "mining_fatigue",
            "strength",
            "instant_health",
            "instant_damage",
            "jump_boost",
            "nausea",
            "regeneration",
            "resistance",
            "fire_resistance",
            "water_breathing",
            "invisibility",
            "blindness",
            "night_vision",
            "hunger",
            "weakness",
            "poison",
            "wither",
            "health_boost",
            "absorption",
            "saturation",
            "glowing",
            "levitation",
            "luck",
            "unluck",
            "slow_falling",
            "conduit_power",
            "dolphins_grace",
            "bad_omen",
            "hero_of_the_village",
            "darkness",
            "trial_omen",
            "raid_omen",
            "wind_charged",
            "weaving",
            "oozing",
            "infested",
            "breath_of_the_nautilus",
        ] {
            for (amplifier, duration) in
                [(0, -1), (1, 0), (5, 20), (9, 21), (10, 1801), (255, 72001)]
            {
                for id in [
                    "potion",
                    "splash_potion",
                    "lingering_potion",
                    "tipped_arrow",
                    "apple",
                ] {
                    cases.push(serde_json::json!({"id":format!("minecraft:{id}"),"custom_effects":[{"id":effect,"amplifier":amplifier,"duration":duration}]}));
                }
            }
        }
        cases.push(serde_json::json!({"id":"minecraft:potion","potion":"swiftness","custom_effects":[{"id":"speed","duration":-1,"amplifier":255},{"id":"weakness","duration":40,"amplifier":1,"ambient":true,"show_particles":false,"show_icon":false}],"lore":["Lore"]}));
        cases.push(serde_json::json!({"id":"minecraft:apple","custom_effects":[]}));
        cases.push(serde_json::json!({"id":"minecraft:potion","custom_effects":[{"id":"haste","duration":100}],"hide_potion":true}));
        cases.push(serde_json::json!({"id":"minecraft:potion","custom_effects":[{"id":"haste"}]}));
        for id in [
            "potion",
            "splash_potion",
            "lingering_potion",
            "tipped_arrow",
        ] {
            for scale in [0.0, 0.125, 0.5, 1.5, 2.0] {
                cases.push(serde_json::json!({"id":format!("minecraft:{id}"),"scale":scale,"custom_effects":[{"id":"haste","duration":1801,"amplifier":1}]}));
            }
        }
        let tag = native_resources::packs::ResourceStack::open(&game)
            .unwrap()
            .read_builtin_data("data/minecraft/tags/enchantment/tooltip_order.json")
            .unwrap()
            .unwrap();
        let tag: serde_json::Value = serde_json::from_slice(&tag).unwrap();
        for enchantment in tag["values"].as_array().unwrap() {
            let enchantment = enchantment.as_str().unwrap();
            for level in [1, 2, 5, 255] {
                for item in ["apple", "enchanted_book"] {
                    for component in ["enchantments", "stored_enchantments"] {
                        let mut values = serde_json::Map::new();
                        values.insert(enchantment.to_owned(), serde_json::json!(level));
                        let mut case = serde_json::json!({"id":format!("minecraft:{item}")});
                        case[component] = values.into();
                        cases.push(case);
                    }
                }
            }
        }
        cases.push(serde_json::json!({"id":"minecraft:apple","potion":"swiftness","enchantments":{"minecraft:mending":1,"minecraft:sharpness":5,"minecraft:binding_curse":1},"stored_enchantments":{"minecraft:vanishing_curse":1,"minecraft:efficiency":3},"lore":["Lore"]}));
        cases.push(serde_json::json!({"id":"minecraft:apple","enchantments":{"minecraft:sharpness":5},"hide_enchantments":true}));
        cases.push(serde_json::json!({"id":"minecraft:enchanted_book","stored_enchantments":{"minecraft:binding_curse":1},"hide_stored":true}));
        let registry = native_resources::packs::ResourceStack::open(&game)
            .unwrap()
            .vanilla_item_registry()
            .unwrap();
        for (id, _) in registry.iter() {
            cases.push(serde_json::json!({"id":id,"attributes_only":true}));
            cases.push(serde_json::json!({"id":id,"attributes_only":true,"player":true}));
        }
        for (damage, speed) in [(0.0, 0.0), (3.5, 7.0), (-5.0, 1.0)] {
            for id in ["wooden_hoe", "diamond_sword", "netherite_spear"] {
                cases.push(serde_json::json!({"id":format!("minecraft:{id}"),"attributes_only":true,"player":true,"damage_base":damage,"speed_base":speed}));
            }
        }
        let custom_attributes = serde_json::json!([
            {"type":"minecraft:movement_speed","id":"example:swift","amount":0.25,"operation":"add_multiplied_total","slot":"feet"},
            {"type":"minecraft:burning_time","id":"example:burn","amount":-0.15,"operation":"add_multiplied_base","slot":"head"},
            {"type":"minecraft:gravity","id":"example:gravity","amount":0.3,"operation":"add_value","slot":"any"},
            {"type":"minecraft:attack_damage","id":"minecraft:base_attack_damage","amount":2.5,"operation":"add_value","slot":"mainhand"},
            {"type":"minecraft:armor","id":"example:concealed","amount":3,"operation":"add_value","slot":"chest","display":{"type":"hidden"}}
            ,{"type":"minecraft:armor","id":"example:custom","amount":0,"operation":"add_value","slot":"legs","display":{"type":"override","value":{"text":"Custom modifier","color":"gold"}}}
        ]);
        for id in ["apple", "diamond_sword"] {
            for player in [false, true] {
                cases.push(serde_json::json!({"id":format!("minecraft:{id}"),"attributes_only":true,"player":player,"attributes":custom_attributes}));
            }
        }
        cases.push(serde_json::json!({"id":"minecraft:diamond_sword","attributes_only":true,"player":true,"remove_attributes":true}));
        cases.push(serde_json::json!({"id":"minecraft:diamond_sword","attributes_only":true,"player":true,"hide_attributes":true}));
        for amount in [
            -1234.555, -1.675, -0.005, -0.0, 0.0, 0.005, 0.015, 1.005, 1.675, 1234.555,
        ] {
            for operation in ["add_value", "add_multiplied_base", "add_multiplied_total"] {
                for attribute in ["gravity", "movement_speed"] {
                    cases.push(serde_json::json!({"id":"minecraft:apple","attributes_only":true,"player":true,"attributes":[{"type":format!("minecraft:{attribute}"),"id":"example:rounding","amount":amount,"operation":operation,"slot":"mainhand"}]}));
                }
            }
        }
        std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("ItemTooltipOracle")
                .arg(&input)
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let expected: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let mut assets = MenuAssets::load(&game, &[]).unwrap();
        let english = native_resources::packs::ResourceStack::open(&game)
            .unwrap()
            .read_asset("assets/minecraft/lang/en_us.json")
            .unwrap()
            .unwrap();
        assets.language = serde_json::from_slice(&english).unwrap();
        for (case, expected) in cases.iter().zip(expected) {
            let mut fields = Vec::new();
            for (json, component) in [
                ("custom", "minecraft:custom_name"),
                ("item_name", "minecraft:item_name"),
            ] {
                if let Some(value) = case[json].as_str() {
                    fields.push((component.into(), Tag::String(value.into())));
                }
            }
            if let Some(value) = case["rarity"].as_str() {
                fields.push((
                    "minecraft:rarity".into(),
                    Tag::String(value.to_ascii_lowercase().as_str().into()),
                ));
            }
            if case["hide"].as_bool() == Some(true) {
                fields.push((
                    "minecraft:tooltip_display".into(),
                    Tag::Compound(vec![("hide_tooltip".into(), Tag::Byte(1))]),
                ));
            }
            if case["hide_potion"].as_bool() == Some(true) {
                fields.push((
                    "minecraft:tooltip_display".into(),
                    Tag::Compound(vec![(
                        "hidden_components".into(),
                        Tag::List {
                            element_type: TagType::String,
                            values: vec![Tag::String("minecraft:potion_contents".into())],
                        },
                    )]),
                ));
            }
            for (flag, component) in [
                ("hide_enchantments", "minecraft:enchantments"),
                ("hide_stored", "minecraft:stored_enchantments"),
                ("hide_attributes", "minecraft:attribute_modifiers"),
            ] {
                if case[flag].as_bool() == Some(true) {
                    fields.push((
                        "minecraft:tooltip_display".into(),
                        Tag::Compound(vec![(
                            "hidden_components".into(),
                            Tag::List {
                                element_type: TagType::String,
                                values: vec![Tag::String(component.into())],
                            },
                        )]),
                    ));
                }
            }
            for (key, component) in [
                ("enchantments", "minecraft:enchantments"),
                ("stored_enchantments", "minecraft:stored_enchantments"),
            ] {
                if let Some(values) = case[key].as_object() {
                    fields.push((
                        component.into(),
                        Tag::Compound(
                            values
                                .iter()
                                .map(|(id, level)| {
                                    (id.as_str().into(), Tag::Int(level.as_i64().unwrap() as i32))
                                })
                                .collect(),
                        ),
                    ));
                }
            }
            if let Some(attributes) = case.get("attributes") {
                fields.push((
                    "minecraft:attribute_modifiers".into(),
                    crate::enchantment_tooltip::component(attributes).unwrap(),
                ));
            }
            if case["remove_attributes"].as_bool() == Some(true) {
                fields.push((
                    "!minecraft:attribute_modifiers".into(),
                    Tag::Compound(vec![]),
                ));
            }
            if let Some(scale) = case["scale"].as_f64() {
                fields.push((
                    "minecraft:potion_duration_scale".into(),
                    Tag::Float(scale as f32),
                ));
            }
            if let Some(lore) = case["lore"].as_array() {
                fields.push((
                    "minecraft:lore".into(),
                    Tag::List {
                        element_type: TagType::String,
                        values: lore
                            .iter()
                            .map(|v| Tag::String(v.as_str().unwrap().into()))
                            .collect(),
                    },
                ));
            }
            if case["potion"].is_string() || case["custom_effects"].is_array() {
                let mut contents = Vec::new();
                if let Some(potion) = case["potion"].as_str() {
                    contents.push((
                        "potion".into(),
                        Tag::String(format!("minecraft:{potion}").as_str().into()),
                    ));
                }
                if let Some(effects) = case["custom_effects"].as_array() {
                    let values = effects
                        .iter()
                        .map(|effect| {
                            let mut fields = vec![(
                                "id".into(),
                                Tag::String(
                                    format!("minecraft:{}", effect["id"].as_str().unwrap())
                                        .as_str()
                                        .into(),
                                ),
                            )];
                            if let Some(duration) = effect["duration"].as_i64() {
                                fields.push(("duration".into(), Tag::Int(duration as i32)));
                            }
                            if let Some(amplifier) = effect["amplifier"].as_u64() {
                                fields.push(("amplifier".into(), Tag::Byte(amplifier as u8 as i8)));
                            }
                            for flag in ["ambient", "show_particles", "show_icon"] {
                                if let Some(value) = effect[flag].as_bool() {
                                    fields.push((flag.into(), Tag::Byte(i8::from(value))));
                                }
                            }
                            Tag::Compound(fields)
                        })
                        .collect();
                    contents.push((
                        "custom_effects".into(),
                        Tag::List {
                            element_type: TagType::Compound,
                            values,
                        },
                    ));
                }
                fields.push(("minecraft:potion_contents".into(), Tag::Compound(contents)));
            }
            let item = ItemStack::new(
                case["id"].as_str().unwrap().into(),
                1,
                64,
                Tag::Compound(fields),
            )
            .unwrap();
            let attribute_case = case["attributes_only"].as_bool() == Some(true);
            let player = case["player"].as_bool() == Some(true);
            let attack_base = player.then(|| {
                [
                    case["damage_base"].as_f64().unwrap_or(1.0),
                    case["speed_base"].as_f64().unwrap_or(4.0),
                ]
            });
            let actual = if attribute_case {
                if case["hide_attributes"].as_bool() == Some(true) {
                    vec![]
                } else {
                    crate::attribute_tooltip::lines(&item, &assets, attack_base)
                }
            } else {
                lines(&item, &assets, &assets.enchantments, attack_base)
            };
            let expected_lines = expected["spans"].as_array().unwrap();
            if case["hide"].as_bool() == Some(true) {
                assert!(actual.is_empty() && expected_lines.is_empty(), "{case}");
                continue;
            }
            let potion_case = attribute_case
                || case["potion"].is_string()
                || case["enchantments"].is_object()
                || case["stored_enchantments"].is_object()
                || case["custom_effects"].is_array()
                || matches!(
                    case["id"].as_str(),
                    Some(
                        "minecraft:potion"
                            | "minecraft:splash_potion"
                            | "minecraft:lingering_potion"
                            | "minecraft:tipped_arrow"
                    )
                );
            let count = if case["lore"].is_array() || potion_case {
                actual.len()
            } else {
                1
            };
            if potion_case
                && !case["enchantments"].is_object()
                && !case["stored_enchantments"].is_object()
            {
                assert_eq!(actual.len(), expected_lines.len(), "{case} {expected}");
            }
            assert!(expected_lines.len() >= count, "{case}");
            for index in 0..count {
                let actual_chars: Vec<_> = actual[index]
                    .iter()
                    .flat_map(|span| {
                        span.text.chars().map(move |ch| {
                            let color = span.style.color.unwrap_or([255; 3]);
                            (
                                ch,
                                u32::from(color[0]) << 16
                                    | u32::from(color[1]) << 8
                                    | u32::from(color[2]),
                                span.style.italic,
                            )
                        })
                    })
                    .collect();
                let expected_chars: Vec<_> = expected_lines[index]
                    .as_array()
                    .unwrap()
                    .iter()
                    .flat_map(|span| {
                        let color = span["color"].as_u64().unwrap() as u32;
                        let italic = span["italic"].as_bool().unwrap();
                        span["text"]
                            .as_str()
                            .unwrap()
                            .chars()
                            .map(move |ch| (ch, color, italic))
                    })
                    .collect();
                assert_eq!(actual_chars, expected_chars, "{case} {expected}");
            }
        }
    }
}
