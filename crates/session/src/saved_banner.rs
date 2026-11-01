//! Read a saved banner for pick block without mutating the source chunk.

use anyhow::{Result, bail, ensure};
use glam::IVec3;
use native_persistence::nbt::{self, Tag, TagType};
use native_world::loaded::{ChunkPos, SavedBlockEntities};
use std::collections::BTreeSet;

use crate::inventory::{ItemStack, saved_item_max};

fn unique<'a>(tag: &'a Tag, key: &str) -> Result<Option<&'a Tag>> {
    let Tag::Compound(fields) = tag else {
        bail!("block entity is not a compound")
    };
    let mut matches = fields.iter().filter(|(name, _)| name.matches(key));
    let value = matches.next().map(|(_, value)| value);
    ensure!(matches.next().is_none(), "duplicate {key}");
    Ok(value)
}

pub(crate) fn pick_item(
    payload: &SavedBlockEntities,
    chunk: ChunkPos,
    position: IVec3,
    id: String,
) -> Result<ItemStack> {
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
    let mut components = Vec::new();
    if let Some(entity) = found {
        ensure!(
            matches!(unique(&entity, "id")?, Some(Tag::String(id)) if id.matches("minecraft:banner") || id.matches("banner")),
            "not a banner block entity"
        );
        if let Some(value) = unique(&entity, "components")? {
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
        if let Some(patterns) = unique(&entity, "patterns")? {
            components.push(("minecraft:banner_patterns".into(), patterns.clone()));
        }
        if let Some(name) = unique(&entity, "CustomName")? {
            components.push(("minecraft:custom_name".into(), name.clone()));
        }
    }
    let components = Tag::Compound(components);
    let max = saved_item_max(&id, &components)
        .ok_or_else(|| anyhow::anyhow!("unsupported banner item components"))?;
    ItemStack::new(id, 1, max, components).ok_or_else(|| anyhow::anyhow!("invalid banner item"))
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
