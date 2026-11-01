//! Distinguish entities known never to collide from ones needing simulation.

use anyhow::{Context, Result, bail, ensure};
use native_persistence::nbt::{Tag, TagType};
use native_resources::{entity_collision::EntityCollisionKind, packs::ResourceId};

pub(crate) fn require_noncolliding(
    entities: &[Tag],
    mut kind: impl FnMut(&str) -> Option<EntityCollisionKind>,
) -> Result<()> {
    let mut pending: Vec<_> = entities.iter().collect();
    while let Some(entity) = pending.pop() {
        ensure!(
            matches!(entity, Tag::Compound(_)),
            "saved entity is not a compound"
        );
        let Some(Tag::String(id)) = entity.get("id") else {
            bail!("saved entity type is missing")
        };
        let id = String::from_utf16(&id.0).context("saved entity type is invalid UTF-16")?;
        let id = ResourceId::parse(&id)?;
        let id = format!("{}:{}", id.namespace, id.path);
        match kind(&id).with_context(|| format!("unknown saved entity type {id}"))? {
            EntityCollisionKind::Never => {}
            EntityCollisionKind::RequiresEntityState => {
                bail!("collision behavior of {id} requires a simulated entity")
            }
        }
        match entity.get("Passengers") {
            None => {}
            Some(Tag::List {
                element_type: TagType::Compound,
                values,
            }) => pending.extend(values),
            Some(Tag::List {
                element_type: TagType::End,
                values,
            }) if values.is_empty() => {}
            _ => bail!("entity passengers are not a compound list"),
        }
    }
    Ok(())
}

/// A non-colliding entity may still block block placement (living mobs do).
/// Only types whose vanilla constructors leave `blocksBuilding` false are
/// accepted here, including every entity in their passenger trees.
pub(crate) fn require_nonblocking_building(entities: &[Tag]) -> Result<()> {
    let mut pending: Vec<_> = entities.iter().collect();
    while let Some(entity) = pending.pop() {
        let Tag::Compound(fields) = entity else {
            bail!("saved entity is not a compound")
        };
        ensure!(
            fields.iter().filter(|(name, _)| name.matches("id")).count() == 1,
            "entity type is missing or duplicated"
        );
        ensure!(
            fields
                .iter()
                .filter(|(name, _)| name.matches("Passengers"))
                .count()
                <= 1,
            "entity passengers are duplicated"
        );
        let Some(Tag::String(id)) = entity.get("id") else {
            bail!("saved entity type is missing")
        };
        let id = ResourceId::parse(
            &String::from_utf16(&id.0).context("entity type is invalid UTF-16")?,
        )?;
        ensure!(
            id.namespace == "minecraft" && matches!(id.path.as_str(), "item" | "experience_orb"),
            "entity may obstruct block placement"
        );
        match entity.get("Passengers") {
            None => {}
            Some(Tag::List {
                element_type: TagType::Compound,
                values,
            }) => pending.extend(values),
            Some(Tag::List {
                element_type: TagType::End,
                values,
            }) if values.is_empty() => {}
            _ => bail!("entity passengers are not a compound list"),
        }
    }
    Ok(())
}

pub(crate) fn require_nonblocking_building_chunk(
    chunk: &native_persistence::world::SavedEntityChunk,
) -> Result<()> {
    let Tag::Compound(fields) = &chunk.document.value else {
        bail!("entity chunk is not a compound")
    };
    ensure!(
        fields
            .iter()
            .filter(|(name, _)| name.matches("Entities"))
            .count()
            <= 1,
        "entity list is duplicated"
    );
    match chunk.document.value.get("Entities") {
        None => Ok(()),
        Some(Tag::List {
            element_type: TagType::Compound,
            values,
        }) => require_nonblocking_building(values),
        Some(Tag::List {
            element_type: TagType::End,
            values,
        }) if values.is_empty() => Ok(()),
        _ => bail!("entity chunk list is invalid"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entity(id: &str, passengers: Vec<Tag>) -> Tag {
        Tag::Compound(vec![
            ("id".into(), Tag::String(id.into())),
            (
                "Passengers".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: passengers,
                },
            ),
        ])
    }
    fn kind(id: &str) -> Option<EntityCollisionKind> {
        match id {
            "minecraft:pig" | "minecraft:item" => Some(EntityCollisionKind::Never),
            "minecraft:oak_boat" => Some(EntityCollisionKind::RequiresEntityState),
            _ => None,
        }
    }
    #[test]
    fn passenger_trees_and_unknown_types_are_never_assumed_safe() {
        let safe = entity("pig", vec![entity("minecraft:item", vec![])]);
        assert!(require_noncolliding(&[safe], kind).is_ok());
        let boat = entity("pig", vec![entity("minecraft:oak_boat", vec![])]);
        assert!(require_noncolliding(&[boat], kind).is_err());
        assert!(require_noncolliding(&[entity("example:unknown", vec![])], kind).is_err());
        assert!(require_noncolliding(&[Tag::Compound(vec![])], kind).is_err());
        let malformed = Tag::Compound(vec![
            ("id".into(), Tag::String("pig".into())),
            ("Passengers".into(), Tag::Int(0)),
        ]);
        assert!(require_noncolliding(&[malformed], kind).is_err());
    }

    #[test]
    fn building_obstruction_requires_all_passengers_to_be_known_nonblocking() {
        assert!(
            require_nonblocking_building(&[entity(
                "minecraft:item",
                vec![entity("experience_orb", vec![])]
            )])
            .is_ok()
        );
        assert!(
            require_nonblocking_building(&[entity("item", vec![entity("minecraft:pig", vec![])])])
                .is_err()
        );
        assert!(require_nonblocking_building(&[entity("minecraft:pig", vec![])]).is_err());
        assert!(require_nonblocking_building(&[entity("example:unknown", vec![])]).is_err());
        assert!(require_nonblocking_building(&[Tag::Compound(vec![])]).is_err());
        let duplicate_id = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:item".into())),
            ("id".into(), Tag::String("minecraft:pig".into())),
        ]);
        assert!(require_nonblocking_building(&[duplicate_id]).is_err());
        let duplicate_passengers = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:item".into())),
            (
                "Passengers".into(),
                Tag::List {
                    element_type: TagType::End,
                    values: vec![],
                },
            ),
            (
                "Passengers".into(),
                Tag::List {
                    element_type: TagType::End,
                    values: vec![],
                },
            ),
        ]);
        assert!(require_nonblocking_building(&[duplicate_passengers]).is_err());
    }

    #[test]
    fn malformed_or_ambiguous_entity_lists_are_not_considered_empty() {
        use native_persistence::{
            nbt::NamedTag,
            world::{ChunkPos, SavedEntityChunk},
        };
        use std::sync::Arc;
        let chunk = |value| SavedEntityChunk {
            position: ChunkPos { x: 0, z: 0 },
            document: Arc::new(NamedTag {
                name: "".into(),
                value,
            }),
        };
        assert!(require_nonblocking_building_chunk(&chunk(Tag::Int(0))).is_err());
        assert!(
            require_nonblocking_building_chunk(&chunk(Tag::Compound(vec![(
                "Entities".into(),
                Tag::Int(0)
            )])))
            .is_err()
        );
        let empty = Tag::List {
            element_type: TagType::End,
            values: vec![],
        };
        assert!(
            require_nonblocking_building_chunk(&chunk(Tag::Compound(vec![
                ("Entities".into(), empty.clone()),
                ("Entities".into(), empty)
            ])))
            .is_err()
        );
        assert!(require_nonblocking_building_chunk(&chunk(Tag::Compound(vec![]))).is_ok());
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn vanilla_loaded_entity_building_flags() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in metadata["libraries"].as_array().unwrap() {
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
        let source = temp.path().join("PlacementEntityOracle.java");
        let output = temp.path().join("output.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/PlacementEntityOracle.java"),
        )
        .unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("PlacementEntityOracle")
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
        let actual: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(
            actual,
            serde_json::json!({"item":false,"experience_orb":false,"armor_stand":true,"loaded_falling_block":false})
        );
    }
}
