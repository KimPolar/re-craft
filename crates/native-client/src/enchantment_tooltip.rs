//! Display definitions from the active world's enchantment registry and tags.

use crate::menu_assets::MenuAssets;
use crate::text_component::{Span, Style};
use anyhow::{Context, Result, bail};
use native_persistence::nbt::{Tag, TagType};
use native_resources::packs::{ResourceId, ResourceProvider};
use std::collections::{HashMap, HashSet};

struct Definition {
    description: Tag,
    max_level: i32,
    attributes: Option<native_resources::enchantments::TooltipEnchantment>,
}

#[derive(Default)]
pub struct Registry {
    definitions: HashMap<String, Definition>,
    order: Vec<String>,
    curses: HashSet<String>,
}

pub(crate) fn component(value: &serde_json::Value) -> Option<Tag> {
    Some(match value {
        serde_json::Value::String(v) => Tag::String(v.as_str().into()),
        serde_json::Value::Bool(v) => Tag::Byte(i8::from(*v)),
        serde_json::Value::Number(v) => match v.as_i64().and_then(|n| i32::try_from(n).ok()) {
            Some(n) => Tag::Int(n),
            None => Tag::Double(v.as_f64()?),
        },
        serde_json::Value::Array(values) => Tag::List {
            element_type: TagType::Compound,
            values: values.iter().map(component).collect::<Option<_>>()?,
        },
        serde_json::Value::Object(fields) => Tag::Compound(
            fields
                .iter()
                .map(|(key, value)| Some((key.as_str().into(), component(value)?)))
                .collect::<Option<_>>()?,
        ),
        serde_json::Value::Null => return None,
    })
}

fn qualified(id: &ResourceId) -> String {
    format!("{}:{}", id.namespace, id.path)
}

fn resolve_tag(
    provider: &mut impl ResourceProvider,
    id: &ResourceId,
    definitions: &HashMap<String, Definition>,
    visiting: &mut HashSet<String>,
) -> Result<Vec<String>> {
    let name = qualified(id);
    if !visiting.insert(name.clone()) {
        bail!("cyclic enchantment tag {name}");
    }
    let outcome = (|| {
        let path = format!("data/{}/tags/enchantment/{}.json", id.namespace, id.path);
        let layers = provider.read_data_stack(&path)?;
        if layers.is_empty() {
            visiting.remove(&name);
            bail!("missing enchantment tag {name}");
        }
        let mut entries: Vec<(String, bool)> = Vec::new();
        for bytes in layers {
            let value: serde_json::Value = serde_json::from_slice(&bytes)?;
            if value.get("replace").and_then(|v| v.as_bool()) == Some(true) {
                entries.clear();
            }
            let values = value
                .get("values")
                .and_then(|v| v.as_array())
                .context("missing enchantment tag values")?;
            for value in values {
                let (id, required) = if let Some(id) = value.as_str() {
                    (id, true)
                } else {
                    (
                        value
                            .get("id")
                            .and_then(|v| v.as_str())
                            .context("missing enchantment tag id")?,
                        value
                            .get("required")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(true),
                    )
                };
                entries.push((id.to_owned(), required));
            }
        }
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        for (entry, required) in entries {
            if let Some(child) = entry.strip_prefix('#') {
                match resolve_tag(provider, &ResourceId::parse(child)?, definitions, visiting) {
                    Ok(values) => {
                        for value in values {
                            if seen.insert(value.clone()) {
                                result.push(value);
                            }
                        }
                    }
                    Err(error) if required => return Err(error),
                    Err(_) => {}
                }
            } else {
                let entry = qualified(&ResourceId::parse(&entry)?);
                if definitions.contains_key(&entry) {
                    if seen.insert(entry.clone()) {
                        result.push(entry);
                    }
                } else if required {
                    bail!("unknown enchantment {entry} in {name}");
                }
            }
        }
        Ok(result)
    })();
    visiting.remove(&name);
    outcome
}

impl Registry {
    pub fn load(provider: &mut impl ResourceProvider) -> Result<Self> {
        let mut definitions = HashMap::new();
        for id in provider.list_data("enchantment")? {
            let path = format!("data/{}/enchantment/{}.json", id.namespace, id.path);
            let Some(bytes) = provider.read_data(&path)? else {
                continue;
            };
            let value: serde_json::Value =
                serde_json::from_slice(&bytes).with_context(|| format!("invalid {path}"))?;
            let description = value
                .get("description")
                .and_then(component)
                .with_context(|| format!("invalid description in {path}"))?;
            let max_level = value
                .get("max_level")
                .and_then(|v| v.as_i64())
                .and_then(|v| i32::try_from(v).ok())
                .with_context(|| format!("invalid max_level in {path}"))?;
            definitions.insert(
                qualified(&id),
                Definition {
                    description,
                    max_level,
                    attributes: native_resources::enchantments::TooltipEnchantment::parse(&value)
                        .ok(),
                },
            );
        }
        let load_tag = |provider: &mut _, name| {
            resolve_tag(
                provider,
                &ResourceId::parse(name)?,
                &definitions,
                &mut HashSet::new(),
            )
        };
        let order = load_tag(provider, "minecraft:tooltip_order")?;
        let curses = load_tag(provider, "minecraft:curse")?.into_iter().collect();
        Ok(Self {
            definitions,
            order,
            curses,
        })
    }

    pub fn attribute_modifiers(
        &self,
        item: &native_session::inventory::ItemStack,
    ) -> Vec<(
        String,
        native_resources::enchantments::TooltipAttributeModifier,
    )> {
        let Some(Tag::Compound(entries)) = item.components().get("minecraft:enchantments") else {
            return Vec::new();
        };
        let mut output = Vec::new();
        for (id, value) in entries {
            let Ok(id) = String::from_utf16(&id.0) else {
                continue;
            };
            let Ok(id) = ResourceId::parse(&id) else {
                continue;
            };
            let Some(level) = value
                .as_java_int()
                .and_then(|level| u32::try_from(level).ok())
            else {
                continue;
            };
            let Some(attributes) = self
                .definitions
                .get(&qualified(&id))
                .and_then(|definition| definition.attributes.as_ref())
            else {
                continue;
            };
            let mut seen = HashSet::new();
            for group in attributes.groups() {
                if seen.insert(group)
                    && let Ok(values) = attributes.modifiers_for_group(group, level)
                {
                    output.extend(values.into_iter().map(|value| (group.clone(), value)));
                }
            }
        }
        output
    }

    pub fn lines(&self, value: &Tag, assets: &MenuAssets) -> Vec<Vec<Span>> {
        let Tag::Compound(entries) = value else {
            return Vec::new();
        };
        let mut present = HashMap::new();
        let mut insertion = Vec::new();
        for (id, value) in entries {
            let Ok(id) = String::from_utf16(&id.0) else {
                continue;
            };
            let Ok(id) = ResourceId::parse(&id) else {
                continue;
            };
            let id = qualified(&id);
            let Some(level) = value.as_java_int().filter(|v| (1..=255).contains(v)) else {
                continue;
            };
            if self.definitions.contains_key(&id) {
                if !present.contains_key(&id) {
                    insertion.push(id.clone());
                }
                present.insert(id, level);
            }
        }
        let mut out = Vec::new();
        for id in self.order.iter().chain(insertion.iter()) {
            let Some(level) = present.remove(id) else {
                continue;
            };
            let Some(definition) = self.definitions.get(id) else {
                continue;
            };
            let base = Style {
                color: Some(if self.curses.contains(id) {
                    [255, 85, 85]
                } else {
                    [170, 170, 170]
                }),
                ..Default::default()
            };
            let translate = |key: &str, fallback: &str| assets.translate(key, fallback).to_owned();
            let Some(mut spans) =
                crate::text_component::resolve_with_style(&definition.description, base, translate)
            else {
                continue;
            };
            if level != 1 || definition.max_level != 1 {
                let key = format!("enchantment.level.{level}");
                spans.push(Span {
                    text: format!(" {}", assets.translate(&key, &key)),
                    style: base,
                });
            }
            out.push(spans);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Synthetic;
    impl ResourceProvider for Synthetic {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
            assert_eq!(folder, "enchantment");
            ["example:a", "example:b", "example:c"]
                .into_iter()
                .map(ResourceId::parse)
                .collect()
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(path
                .starts_with("data/example/enchantment/")
                .then(|| br#"{"description":{"text":"Example"},"max_level":1}"#.to_vec()))
        }
        fn read_data_stack(&mut self, path: &str) -> Result<Vec<Vec<u8>>> {
            let layers: Vec<&[u8]> = match path {
                "data/minecraft/tags/enchantment/tooltip_order.json" => vec![br#"{"values":["example:c"]}"#, br##"{"replace":true,"values":["example:b","#example:nested",{"id":"#example:optional","required":false},"example:a"]}"##],
                "data/example/tags/enchantment/nested.json" => vec![br#"{"values":["example:a","example:b"]}"#],
                "data/minecraft/tags/enchantment/curse.json" => vec![br#"{"values":["example:c"]}"#],
                _ => vec![],
            };
            Ok(layers.into_iter().map(<[u8]>::to_vec).collect())
        }
    }

    #[test]
    fn layered_enchantment_tags_keep_order_and_remove_duplicates() {
        let registry = Registry::load(&mut Synthetic).unwrap();
        assert_eq!(registry.order, ["example:b", "example:a"]);
        assert_eq!(registry.curses, HashSet::from(["example:c".to_owned()]));
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn selected_data_pack_controls_names_order_and_curses() {
        use native_resources::{datapacks::DataPackSettings, packs::ResourceStack};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let world = tempfile::tempdir().unwrap();
        let pack = world.path().join("datapacks/custom");
        for dir in [
            "data/minecraft/enchantment",
            "data/example/enchantment",
            "data/minecraft/tags/enchantment",
            "data/example/tags/enchantment",
        ] {
            std::fs::create_dir_all(pack.join(dir)).unwrap();
        }
        std::fs::write(
            pack.join("pack.mcmeta"),
            br#"{"pack":{"description":"tooltip test","min_format":121,"max_format":121}}"#,
        )
        .unwrap();
        std::fs::write(
            pack.join("data/minecraft/enchantment/sharpness.json"),
            br#"{"description":{"text":"Mystic","color":"green"},"max_level":1}"#,
        )
        .unwrap();
        std::fs::write(
            pack.join("data/example/enchantment/warp.json"),
            br#"{"description":{"text":"Warp","color":"yellow"},"supported_items":["minecraft:apple"],"weight":1,"max_level":3,"min_cost":{"base":1,"per_level_above_first":1},"max_cost":{"base":5,"per_level_above_first":1},"anvil_cost":1,"slots":["mainhand","head"],"effects":{"minecraft:attributes":[{"id":"example:power","attribute":"minecraft:attack_damage","amount":{"type":"minecraft:linear","base":1.0,"per_level_above_first":2.0},"operation":"add_value"},{"id":"example:gravity","attribute":"minecraft:gravity","amount":{"type":"minecraft:fraction","numerator":2.0,"denominator":3.0},"operation":"add_multiplied_total"}]}}"#,
        )
        .unwrap();
        std::fs::write(pack.join("data/minecraft/tags/enchantment/tooltip_order.json"), br##"{"replace":true,"values":["example:warp","#example:group",{"id":"#example:missing","required":false},"minecraft:sharpness"]}"##).unwrap();
        std::fs::write(
            pack.join("data/example/tags/enchantment/group.json"),
            br#"{"values":["minecraft:binding_curse","example:warp"]}"#,
        )
        .unwrap();
        std::fs::write(
            pack.join("data/minecraft/tags/enchantment/curse.json"),
            br#"{"replace":true,"values":["example:warp"]}"#,
        )
        .unwrap();
        let mut resources = ResourceStack::open(&game).unwrap();
        let warnings = resources
            .configure_world_data(
                world.path(),
                &DataPackSettings {
                    enabled: vec!["vanilla".into(), "file/custom".into()],
                    disabled: vec![],
                    enabled_features: vec!["minecraft:vanilla".into()],
                },
            )
            .unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let registry = Registry::load(&mut resources).unwrap();
        assert_eq!(
            &registry.order,
            &[
                "example:warp",
                "minecraft:binding_curse",
                "minecraft:sharpness"
            ]
        );
        assert!(registry.curses.contains("example:warp"));
        assert!(!registry.curses.contains("minecraft:binding_curse"));
        let mut assets = MenuAssets::load(&game, &[]).unwrap();
        let english = ResourceStack::open(&game)
            .unwrap()
            .read_asset("assets/minecraft/lang/en_us.json")
            .unwrap()
            .unwrap();
        assets.language = serde_json::from_slice(&english).unwrap();
        let entries = Tag::Compound(vec![
            ("minecraft:sharpness".into(), Tag::Int(1)),
            ("minecraft:binding_curse".into(), Tag::Int(1)),
            ("example:warp".into(), Tag::Int(2)),
        ]);
        let lines = registry.lines(&entries, &assets);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0][0].text, "Warp");
        assert_eq!(lines[0][0].style.color, Some([255, 255, 85]));
        assert_eq!(lines[0][1].style.color, Some([255, 85, 85]));
        assert_eq!(lines[1][0].style.color, Some([170, 170, 170]));
        assert_eq!(lines[2][0].text, "Mystic");
        assert_eq!(lines[2][0].style.color, Some([85, 255, 85]));
        assert_eq!(lines[2].len(), 1);
        let enchanted = native_session::inventory::ItemStack::new(
            "minecraft:apple".into(),
            1,
            64,
            Tag::Compound(vec![(
                "minecraft:enchantments".into(),
                Tag::Compound(vec![("example:warp".into(), Tag::Int(3))]),
            )]),
        )
        .unwrap();
        let values = registry.attribute_modifiers(&enchanted);
        let mainhand: Vec<_> = values
            .iter()
            .filter(|(group, _)| group == "mainhand")
            .map(|(_, value)| value.clone())
            .collect();
        assert_eq!(mainhand.len(), 2);
        assert_eq!(mainhand[0].attribute.path, "attack_damage");
        assert_eq!(mainhand[0].modifier.amount, 5.0);
        assert_eq!(mainhand[1].modifier.amount, f64::from(2.0_f32 / 3.0_f32));
        assert_eq!(
            values
                .iter()
                .filter(|(group, _)| group == "head")
                .map(|(_, value)| value.clone())
                .collect::<Vec<_>>(),
            mainhand
                .iter()
                .cloned()
                .map(|mut modifier| {
                    modifier.modifier.id.path =
                        modifier.modifier.id.path.replace("/mainhand", "/head");
                    modifier
                })
                .collect::<Vec<_>>()
        );
        assert!(!values.iter().any(|(group, _)| group == "armor"));
        let displayed =
            crate::attribute_tooltip::lines(&enchanted, &assets, Some([1.0, 4.0]), &registry);
        assert_eq!(displayed.len(), 8);
        assert_eq!(displayed[2][0].text, "+5 Attack Damage");
        assert_eq!(displayed[3][0].text, "+66.67% Gravity");

        // Load the same synthetic enchantment through Java's real codec and
        // compare the resulting attribute tooltip text and colors.
        let definition: serde_json::Value = serde_json::from_slice(
            &std::fs::read(pack.join("data/example/enchantment/warp.json")).unwrap(),
        )
        .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_owned()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("ItemTooltipOracle.java");
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/ItemTooltipOracle.java"),
        )
        .unwrap();
        let mut cases = Vec::new();
        for id in ["minecraft:apple", "minecraft:diamond_sword"] {
            for level in [1, 2, 3, 255] {
                for player in [false, true] {
                    cases.push(serde_json::json!({"id":id,"attributes_only":true,"player":player,"custom_enchantment":definition,"enchantments":{"example:warp":level}}));
                }
            }
        }
        std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
        for command in [
            std::process::Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            std::process::Command::new(java.join("java.exe"))
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
        for (case, expected) in cases.iter().zip(expected) {
            let level = case["enchantments"]["example:warp"].as_i64().unwrap() as i32;
            let item = native_session::inventory::ItemStack::new(
                case["id"].as_str().unwrap().into(),
                1,
                64,
                Tag::Compound(vec![(
                    "minecraft:enchantments".into(),
                    Tag::Compound(vec![("example:warp".into(), Tag::Int(level))]),
                )]),
            )
            .unwrap();
            let actual = crate::attribute_tooltip::lines(
                &item,
                &assets,
                case["player"].as_bool().unwrap().then_some([1.0, 4.0]),
                &registry,
            );
            let expected = expected["spans"].as_array().unwrap();
            assert_eq!(actual.len(), expected.len(), "{case}");
            for (actual, expected) in actual.iter().zip(expected) {
                let actual: Vec<_> = actual
                    .iter()
                    .flat_map(|span| {
                        span.text
                            .chars()
                            .map(move |ch| (ch, span.style.color.unwrap_or([255, 255, 255])))
                    })
                    .collect();
                let expected: Vec<_> = expected
                    .as_array()
                    .unwrap()
                    .iter()
                    .flat_map(|span| {
                        let rgb = span["color"].as_u64().unwrap() as u32;
                        let color = [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8];
                        span["text"]
                            .as_str()
                            .unwrap()
                            .chars()
                            .map(move |ch| (ch, color))
                    })
                    .collect();
                assert_eq!(actual, expected, "{case}");
            }
        }
    }
}
