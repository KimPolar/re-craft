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
}

#[derive(Default)]
pub struct Registry {
    definitions: HashMap<String, Definition>,
    order: Vec<String>,
    curses: HashSet<String>,
}

fn component(value: &serde_json::Value) -> Option<Tag> {
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
            br#"{"description":{"text":"Warp","color":"yellow"},"max_level":3}"#,
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
        let assets = MenuAssets::load(&game, &[]).unwrap();
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
    }
}
