//! Read a saved banner for pick block without mutating the source chunk.

use anyhow::{Result, bail, ensure};
use glam::IVec3;
use native_persistence::nbt::{self, Tag, TagType};
use native_world::loaded::{ChunkPos, SavedBlockEntities};
use std::collections::BTreeSet;

use crate::inventory::{ItemStack, saved_item_max};

/// Convert a resolved banner item to the fields persisted by its block entity.
pub(crate) fn placed(position: IVec3, item: &ItemStack) -> Result<Tag> {
    ensure!(
        crate::inventory::is_banner_item(item.id())
            && item.max_count().is_some()
            && saved_item_max(item.id(), item.components()) == item.max_count(),
        "unsupported banner item"
    );
    let Tag::Compound(entries) = item.components() else {
        bail!("invalid banner components")
    };
    let mut fields = vec![
        ("id".into(), Tag::String("minecraft:banner".into())),
        ("x".into(), Tag::Int(position.x)),
        ("y".into(), Tag::Int(position.y)),
        ("z".into(), Tag::Int(position.z)),
    ];
    let mut other = Vec::new();
    for (name, value) in entries {
        if name.matches("minecraft:banner_patterns") {
            if let Tag::List { values, .. } = value
                && !values.is_empty()
            {
                fields.push(("patterns".into(), value.clone()));
            }
        } else if name.matches("minecraft:custom_name") {
            fields.push(("CustomName".into(), value.clone()));
        } else if !name.0.first().is_some_and(|c| *c == u16::from(b'!')) {
            other.push((name.clone(), value.clone()));
        }
    }
    fields.push(("components".into(), Tag::Compound(other)));
    Ok(Tag::Compound(fields))
}

/// Build a renderable session view without modifying the original chunk bytes.
pub(crate) fn append(
    payload: &SavedBlockEntities,
    chunk: ChunkPos,
    entity: &Tag,
) -> Result<SavedBlockEntities> {
    let coordinate = |value: &Tag, key| -> Result<i32> {
        match unique(value, key)? {
            Some(Tag::Int(v)) => Ok(*v),
            _ => bail!("invalid block entity coordinate"),
        }
    };
    let target = IVec3::new(
        coordinate(entity, "x")?,
        coordinate(entity, "y")?,
        coordinate(entity, "z")?,
    );
    ensure!(
        target.x.div_euclid(16) == chunk.x && target.z.div_euclid(16) == chunk.z,
        "block entity outside chunk"
    );
    let mut values = match payload {
        SavedBlockEntities::Unavailable => bail!("block entities unavailable"),
        SavedBlockEntities::Empty => Vec::new(),
        SavedBlockEntities::Encoded(bytes) => match nbt::read(bytes)?.value {
            Tag::List {
                element_type: TagType::Compound,
                values,
            } => values,
            _ => bail!("invalid block entity list"),
        },
    };
    let mut seen = BTreeSet::new();
    for entry in &values {
        ensure!(
            matches!(unique(entry,"id")?, Some(Tag::String(id)) if !id.0.is_empty()),
            "invalid block entity id"
        );
        let p = IVec3::new(
            coordinate(entry, "x")?,
            coordinate(entry, "y")?,
            coordinate(entry, "z")?,
        );
        ensure!(
            p.x.div_euclid(16) == chunk.x
                && p.z.div_euclid(16) == chunk.z
                && seen.insert(p.to_array())
                && p != target,
            "occupied or invalid block entity position"
        );
    }
    values.push(entity.clone());
    let bytes = nbt::write(&nbt::NamedTag {
        name: "".into(),
        value: Tag::List {
            element_type: TagType::Compound,
            values,
        },
    })?;
    Ok(SavedBlockEntities::Encoded(bytes.into()))
}

fn unique<'a>(tag: &'a Tag, key: &str) -> Result<Option<&'a Tag>> {
    let Tag::Compound(fields) = tag else {
        bail!("block entity is not a compound")
    };
    let mut matches = fields.iter().filter(|(name, _)| name.matches(key));
    let value = matches.next().map(|(_, value)| value);
    ensure!(matches.next().is_none(), "duplicate {key}");
    Ok(value)
}

pub(crate) fn find(
    payload: &SavedBlockEntities,
    chunk: ChunkPos,
    position: IVec3,
) -> Result<Option<Tag>> {
    ensure!(
        position.x.div_euclid(16) == chunk.x && position.z.div_euclid(16) == chunk.z,
        "banner outside chunk"
    );
    let mut found = None;
    match payload {
        SavedBlockEntities::Unavailable => bail!("block entity data unavailable"),
        SavedBlockEntities::Empty => {}
        SavedBlockEntities::Encoded(bytes) => {
            let document = nbt::read(bytes)?;
            let Tag::List {
                element_type: TagType::Compound,
                values,
            } = document.value
            else {
                bail!("invalid block entities list")
            };
            let mut seen = BTreeSet::new();
            for entity in values {
                let coordinate = |key| -> Result<i32> {
                    match unique(&entity, key)? {
                        Some(Tag::Int(value)) => Ok(*value),
                        _ => bail!("invalid block entity coordinate"),
                    }
                };
                let p = IVec3::new(coordinate("x")?, coordinate("y")?, coordinate("z")?);
                ensure!(
                    p.x.div_euclid(16) == chunk.x
                        && p.z.div_euclid(16) == chunk.z
                        && seen.insert(p.to_array()),
                    "invalid or duplicate block entity position"
                );
                if p == position {
                    found = Some(entity);
                }
            }
        }
    }
    if let Some(entity) = &found {
        ensure!(
            matches!(unique(entity, "id")?, Some(Tag::String(id)) if id.matches("minecraft:banner") || id.matches("banner")),
            "not a banner block entity"
        );
    }
    Ok(found)
}

pub(crate) fn collected_components(entity: Option<&Tag>) -> Result<Tag> {
    let mut components = Vec::new();
    if let Some(entity) = entity {
        if let Some(value) = unique(entity, "components")? {
            let Tag::Compound(entries) = value else {
                bail!("invalid block entity components")
            };
            // The banner's implicit components override its stored component map.
            components.extend(
                entries
                    .iter()
                    .filter(|(key, _)| {
                        !key.matches("minecraft:banner_patterns")
                            && !key.matches("minecraft:custom_name")
                    })
                    .cloned(),
            );
        }
        let patterns = unique(entity, "patterns")?.cloned().unwrap_or(Tag::List {
            element_type: TagType::Compound,
            values: Vec::new(),
        });
        components.push(("minecraft:banner_patterns".into(), patterns));
        if let Some(name) = unique(entity, "CustomName")? {
            components.push(("minecraft:custom_name".into(), name.clone()));
        }
    }
    Ok(Tag::Compound(components))
}

pub(crate) fn pick_item(
    payload: &SavedBlockEntities,
    chunk: ChunkPos,
    position: IVec3,
    id: String,
) -> Result<ItemStack> {
    let entity = find(payload, chunk, position)?;
    let components = collected_components(entity.as_ref())?;
    let max = saved_item_max(&id, &components)
        .ok_or_else(|| anyhow::anyhow!("unsupported banner item components"))?;
    ItemStack::new(id, 1, max, components).ok_or_else(|| anyhow::anyhow!("invalid banner item"))
}

pub(crate) fn remove(
    payload: &SavedBlockEntities,
    chunk: ChunkPos,
    position: IVec3,
) -> Result<(SavedBlockEntities, Tag)> {
    let entity = find(payload, chunk, position)?
        .ok_or_else(|| anyhow::anyhow!("banner entity is missing"))?;
    let SavedBlockEntities::Encoded(bytes) = payload else {
        bail!("banner entity data is unavailable")
    };
    let mut document = nbt::read(bytes)?;
    let Tag::List { values, .. } = &mut document.value else {
        bail!("invalid block entity list")
    };
    let index = values
        .iter()
        .position(|value| value == &entity)
        .ok_or_else(|| anyhow::anyhow!("banner entity disappeared"))?;
    values.remove(index);
    let replacement = if values.is_empty() {
        SavedBlockEntities::Empty
    } else {
        SavedBlockEntities::Encoded(nbt::write(&document)?.into())
    };
    Ok((replacement, entity))
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_persistence::nbt::NamedTag;

    fn payload(entities: Vec<Tag>) -> SavedBlockEntities {
        SavedBlockEntities::Encoded(
            nbt::write(&NamedTag {
                name: "".into(),
                value: Tag::List {
                    element_type: TagType::Compound,
                    values: entities,
                },
            })
            .unwrap()
            .into(),
        )
    }

    #[test]
    fn malformed_or_unavailable_entities_do_not_produce_items() {
        let chunk = ChunkPos { x: 0, z: 0 };
        let position = IVec3::ZERO;
        let id = "minecraft:white_banner".to_owned();
        assert!(
            pick_item(
                &SavedBlockEntities::Unavailable,
                chunk,
                position,
                id.clone()
            )
            .is_err()
        );
        let entity = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:banner".into())),
            ("x".into(), Tag::Int(0)),
            ("y".into(), Tag::Int(0)),
            ("z".into(), Tag::Int(0)),
            (
                "components".into(),
                Tag::Compound(vec![("minecraft:unknown".into(), Tag::Int(3))]),
            ),
        ]);
        assert!(pick_item(&payload(vec![entity.clone()]), chunk, position, id.clone()).is_err());
        assert!(
            pick_item(
                &payload(vec![entity.clone(), entity]),
                chunk,
                position,
                id.clone()
            )
            .is_err()
        );
        assert!(pick_item(&SavedBlockEntities::Empty, chunk, position, id).is_ok());
    }

    #[test]
    fn placement_rejects_unavailable_occupied_and_malformed_sources() {
        let chunk = ChunkPos { x: 0, z: 0 };
        let pos = IVec3::ZERO;
        let item = ItemStack::new(
            "minecraft:white_banner".into(),
            1,
            16,
            Tag::Compound(vec![]),
        )
        .unwrap();
        let entity = placed(pos, &item).unwrap();
        assert!(append(&SavedBlockEntities::Unavailable, chunk, &entity).is_err());
        assert!(append(&payload(vec![entity.clone()]), chunk, &entity).is_err());
        let invalid = Tag::Compound(vec![
            ("x".into(), Tag::Int(1)),
            ("y".into(), Tag::Int(0)),
            ("z".into(), Tag::Int(0)),
        ]);
        assert!(append(&payload(vec![invalid]), chunk, &entity).is_err());
        let visible = append(&SavedBlockEntities::Empty, chunk, &entity).unwrap();
        assert!(pick_item(&visible, chunk, pos, "minecraft:white_banner".into()).is_ok());
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn banner_loot_component_filters_match_java() {
        use native_resources::{loot::BlockLootTables, packs::ResourceProvider};
        use native_world::loaded::BlockState;
        use std::{collections::BTreeMap, path::PathBuf, process::Command};
        struct Source(Vec<u8>);
        impl ResourceProvider for Source {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                Ok(
                    (path == "data/minecraft/loot_table/blocks/white_banner.json")
                        .then(|| self.0.clone()),
                )
            }
        }
        let filters = [
            serde_json::json!({"include":["minecraft:banner_patterns","minecraft:custom_name"]}),
            serde_json::json!({"include":["minecraft:banner_patterns","minecraft:custom_name"],"exclude":["minecraft:banner_patterns"]}),
            serde_json::json!({"exclude":["minecraft:custom_data"]}),
            serde_json::json!({"include":["minecraft:custom_data"]}),
        ];
        let tables:Vec<_>=filters.into_iter().map(|filter| {let mut modifier=filter.as_object().unwrap().clone();modifier.insert("type".into(),serde_json::json!("minecraft:copy_components"));modifier.insert("source".into(),serde_json::json!("block_entity"));serde_json::json!({"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:white_banner","modifier":modifier}]}]})}).collect();
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for lib in metadata["libraries"].as_array().unwrap() {
            if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("BannerLootOracle.java");
        let input = temp.path().join("tables.json");
        let output = temp.path().join("items.nbt");
        std::fs::write(&source, include_str!("../tests/java/BannerLootOracle.java")).unwrap();
        std::fs::write(&input, serde_json::to_vec(&tables).unwrap()).unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("BannerLootOracle")
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
        let expected = nbt::read(&std::fs::read(output).unwrap()).unwrap();
        let block = BlockState {
            name: "minecraft:white_banner".into(),
            properties: BTreeMap::from([("rotation".into(), "0".into())]),
        };
        let components = Tag::Compound(vec![
            (
                "minecraft:banner_patterns".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![Tag::Compound(vec![
                        ("pattern".into(), Tag::String("minecraft:cross".into())),
                        ("color".into(), Tag::String("blue".into())),
                    ])],
                },
            ),
            ("minecraft:custom_name".into(), Tag::String("Named".into())),
            (
                "minecraft:custom_data".into(),
                Tag::Compound(vec![("future".into(), Tag::Int(42))]),
            ),
        ]);
        let items = crate::items::ItemEntities::default();
        for (index, table) in tables.iter().enumerate() {
            let loot =
                BlockLootTables::load(&mut Source(serde_json::to_vec(table).unwrap())).unwrap();
            let (drops, prepared, _) = items
                .prepare_block_entity_drops(
                    "minecraft:white_banner",
                    &block,
                    None,
                    true,
                    &components,
                    Some(&loot),
                )
                .unwrap();
            assert_eq!(drops.len(), 1);
            let actual = &prepared[0];
            let expected = expected
                .value
                .get(&format!("case{index}"))
                .unwrap()
                .get("components")
                .unwrap();
            for key in [
                "minecraft:banner_patterns",
                "minecraft:custom_name",
                "minecraft:custom_data",
            ] {
                match (actual.get(key), expected.get(key)) {
                    (Some(actual), Some(expected)) => assert!(
                        crate::inventory::equal_tag(actual, expected),
                        "case {index} {key}"
                    ),
                    (None, None) => {}
                    (actual, expected) => panic!("case {index} {key}: {actual:?} vs {expected:?}"),
                }
            }
        }
    }

    #[test]
    #[ignore = "requires installed Minecraft 26.3 assets"]
    fn installed_banner_loot_preserves_vanilla_name_and_patterns() {
        use native_resources::{loot::BlockLootTables, packs::ResourceStack};
        use native_world::loaded::BlockState;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let loot = BlockLootTables::load(&mut ResourceStack::open(&game).unwrap()).unwrap();
        let components = Tag::Compound(vec![
            (
                "minecraft:banner_patterns".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![Tag::Compound(vec![
                        ("pattern".into(), Tag::String("minecraft:cross".into())),
                        ("color".into(), Tag::String("blue".into())),
                    ])],
                },
            ),
            ("minecraft:custom_name".into(), Tag::String("Named".into())),
            (
                "minecraft:custom_data".into(),
                Tag::Compound(vec![("future".into(), Tag::Int(42))]),
            ),
        ]);
        let items = crate::items::ItemEntities::default();
        for color in [
            "white",
            "orange",
            "magenta",
            "light_blue",
            "yellow",
            "lime",
            "pink",
            "gray",
            "light_gray",
            "cyan",
            "purple",
            "blue",
            "brown",
            "green",
            "red",
            "black",
        ] {
            for wall in [false, true] {
                let item = format!("minecraft:{color}_banner");
                crate::items::ItemEntities::validate_block_entity_drops(
                    &item,
                    &components,
                    Some(&loot),
                )
                .unwrap();
                let block = BlockState {
                    name: format!(
                        "minecraft:{color}_{}banner",
                        if wall { "wall_" } else { "" }
                    ),
                    properties: if wall {
                        [("facing".into(), "north".into())].into()
                    } else {
                        [("rotation".into(), "0".into())].into()
                    },
                };
                let (drops, prepared, _) = items
                    .prepare_block_entity_drops(&item, &block, None, true, &components, Some(&loot))
                    .unwrap();
                assert_eq!(drops.len(), 1);
                assert_eq!(
                    format!("{}:{}", drops[0].item.namespace, drops[0].item.path),
                    item
                );
                assert_eq!(
                    prepared[0].get("minecraft:banner_patterns"),
                    components.get("minecraft:banner_patterns")
                );
                assert_eq!(
                    prepared[0].get("minecraft:custom_name"),
                    components.get("minecraft:custom_name")
                );
                assert!(prepared[0].get("minecraft:custom_data").is_none());
            }
        }
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn saved_banner_picks_and_shapes_match_java() {
        use native_world::{
            collision::{banner_item_id, block_outline, block_shape},
            loaded::BlockState,
        };
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for entry in metadata["libraries"].as_array().unwrap() {
            if let Some(path) = entry["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("BannerPickOracle.java");
        let nbt_path = temp.path().join("banners.nbt");
        let json_path = temp.path().join("banners.json");
        std::fs::write(&source, include_str!("../tests/java/BannerPickOracle.java")).unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("BannerPickOracle")
                .arg(&nbt_path)
                .arg(&json_path),
        ] {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let document = nbt::read(&std::fs::read(nbt_path).unwrap()).unwrap();
        let rows: serde_json::Value =
            serde_json::from_slice(&std::fs::read(json_path).unwrap()).unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 80);
        for row in rows.as_array().unwrap() {
            let color = row["color"].as_str().unwrap();
            let face = row["face"].as_str().unwrap();
            let entity = document
                .value
                .get(&format!("{color}_{face}"))
                .unwrap()
                .clone();
            let block = BlockState {
                name: format!(
                    "minecraft:{color}_{}banner",
                    if face == "standing" { "" } else { "wall_" }
                ),
                properties: if face == "standing" {
                    [("rotation".into(), "0".into())].into()
                } else {
                    [("facing".into(), face.into())].into()
                },
            };
            let id = banner_item_id(&block).unwrap();
            let item = pick_item(
                &payload(vec![entity.clone()]),
                ChunkPos { x: 0, z: 0 },
                IVec3::ZERO,
                id,
            )
            .unwrap();
            assert_eq!(item.id(), row["id"].as_str().unwrap());
            assert_eq!(item.max_count(), Some(row["max"].as_u64().unwrap() as u32));
            assert_eq!(
                item.components().get("minecraft:banner_patterns"),
                entity.get("patterns")
            );
            assert_eq!(
                item.components().get("minecraft:custom_name"),
                entity.get("CustomName")
            );
            let placed = placed(IVec3::ZERO, &item).unwrap();
            for key in ["id", "x", "y", "z", "patterns", "CustomName", "components"] {
                assert_eq!(placed.get(key), entity.get(key), "{key}: {row}");
            }
            let Some(Tag::List { values, .. }) = item.components().get("minecraft:banner_patterns")
            else {
                panic!()
            };
            let expected_patterns = row["patterns"].as_array().unwrap();
            assert_eq!(values.len(), expected_patterns.len());
            for (layer, expected) in values.iter().zip(expected_patterns) {
                assert!(
                    matches!(layer.get("pattern"), Some(Tag::String(value)) if value.matches(expected["pattern"].as_str().unwrap()))
                );
                assert!(
                    matches!(layer.get("color"), Some(Tag::String(value)) if value.matches(expected["color"].as_str().unwrap()))
                );
            }
            let expected = row["outline"][0].as_array().unwrap();
            let shape = block_outline(&block).unwrap();
            assert_eq!(shape.len(), 1);
            let actual = [
                shape[0].min.x,
                shape[0].min.y,
                shape[0].min.z,
                shape[0].max.x,
                shape[0].max.y,
                shape[0].max.z,
            ];
            for (a, b) in actual.into_iter().zip(expected) {
                assert_eq!(f64::from(a), b.as_f64().unwrap(), "{row}");
            }
            assert_eq!(
                block_shape(&block).unwrap().is_empty(),
                row["collision_empty"].as_bool().unwrap()
            );
        }
    }
}
