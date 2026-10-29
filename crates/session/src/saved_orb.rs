//! Read a 26.3 experience orb without modifying its saved entity data.

use crate::{player_data, saved_item};
use anyhow::{Context, Result, bail, ensure};
use glam::DVec3;
use native_persistence::nbt::{NamedTag, Tag, TagType};
use native_resources::packs::ResourceId;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct SavedOrb {
    pub source: Arc<Tag>,
    pub uuid: Option<[i32; 4]>,
    pub position: DVec3,
    pub velocity: DVec3,
    pub on_ground: bool,
    pub no_gravity: bool,
    pub age: i32,
    pub value: i32,
    pub count: i32,
    pub health: i32,
    pub fire_ticks: i16,
    pub invulnerable: bool,
    pub invulnerable_time: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct OrbSaveState {
    pub position: DVec3,
    pub velocity: DVec3,
    pub on_ground: bool,
    pub no_gravity: bool,
    pub age: i32,
    pub value: i32,
    pub count: i32,
    pub health: i32,
    pub fire_ticks: i32,
    pub invulnerable: bool,
    pub invulnerable_time: i32,
}

pub struct OrbDraft {
    source_nbt: Vec<u8>,
    document: NamedTag,
}

impl OrbDraft {
    pub fn document(&self) -> &NamedTag {
        &self.document
    }
    pub fn matches_source(&self, current: &NamedTag) -> Result<bool> {
        Ok(native_persistence::nbt::write(current)? == self.source_nbt)
    }
}

/// A draft of a still-existing orb, not authorization to write a world.
/// Entity creation, deletion, other changes and the world transaction must
/// be coordinated separately.
pub fn draft(original: &NamedTag, state: OrbSaveState) -> Result<OrbDraft> {
    let Tag::Compound(fields) = &original.value else {
        bail!("entity must be a compound");
    };
    for key in [
        "id",
        "UUID",
        "Pos",
        "Motion",
        "OnGround",
        "NoGravity",
        "Age",
        "Value",
        "Count",
        "Health",
        "Fire",
        "Invulnerable",
        "invulnerable_time",
        "Passengers",
    ] {
        ensure!(
            fields.iter().filter(|(name, _)| name.matches(key)).count() <= 1,
            "duplicate entity {key}"
        );
    }
    ensure!(
        original.value.get("Passengers").is_none(),
        "passengers are not simulated"
    );
    let saved = read(&original.value)?.context("entity is not an experience orb")?;
    ensure!(saved.uuid.is_some(), "experience orb has no stable UUID");
    ensure!(saved.value == state.value, "experience orb value changed");
    ensure!(state.count > 0, "experience orb count cannot be saved");
    let age = i16::try_from(state.age).context("experience orb age cannot be saved as a short")?;
    let health =
        i16::try_from(state.health).context("experience orb health cannot be saved as a short")?;
    let fire = i16::try_from(state.fire_ticks)
        .context("experience orb fire cannot be saved as a short")?;
    for key in ["Pos", "Motion"] {
        if let Some(value) = original.value.get(key) {
            ensure!(
                matches!(value, Tag::List { element_type: TagType::Double, values } if values.len() == 3 && values.iter().all(|v| matches!(v, Tag::Double(x) if x.is_finite()))),
                "unsupported {key}"
            );
        }
    }
    for key in ["OnGround", "NoGravity", "Invulnerable"] {
        if let Some(value) = original.value.get(key) {
            ensure!(matches!(value, Tag::Byte(0 | 1)), "unsupported {key}");
        }
    }
    for key in ["Age", "Value", "Health", "Fire"] {
        if let Some(value) = original.value.get(key) {
            ensure!(matches!(value, Tag::Short(_)), "unsupported {key}");
        }
    }
    if let Some(value) = original.value.get("Count") {
        ensure!(matches!(value, Tag::Int(1..)), "unsupported Count");
    }
    if let Some(value) = original.value.get("invulnerable_time") {
        ensure!(
            matches!(value, Tag::Int(_)),
            "unsupported invulnerable_time"
        );
    }
    let valid_position = |v: DVec3| {
        v.is_finite()
            && v.x.abs() <= 30_000_512.0
            && v.z.abs() <= 30_000_512.0
            && v.y.abs() <= 20_000_000.0
    };
    let raw_vector = |key: &str| -> Option<DVec3> {
        let Tag::List { values, .. } = original.value.get(key)? else {
            return None;
        };
        let [Tag::Double(x), Tag::Double(y), Tag::Double(z)] = values.as_slice() else {
            return None;
        };
        Some(DVec3::new(*x, *y, *z))
    };
    ensure!(
        raw_vector("Pos").is_some_and(valid_position) && valid_position(state.position),
        "position would be clamped"
    );
    ensure!(
        raw_vector("Motion").is_none_or(|v| v.abs().max_element() <= 10.0)
            && state.velocity.is_finite()
            && state.velocity.abs().max_element() <= 10.0,
        "velocity would be discarded"
    );
    let source_nbt = native_persistence::nbt::write(original)?;
    let mut document = original.clone();
    let Tag::Compound(fields) = &mut document.value else {
        unreachable!()
    };
    let vector = |v: DVec3| Tag::List {
        element_type: TagType::Double,
        values: v.to_array().into_iter().map(Tag::Double).collect(),
    };
    for (name, value) in [
        ("Pos", vector(state.position)),
        ("Motion", vector(state.velocity)),
        ("OnGround", Tag::Byte(i8::from(state.on_ground))),
        ("NoGravity", Tag::Byte(i8::from(state.no_gravity))),
        ("Age", Tag::Short(age)),
        ("Value", Tag::Short(saved.value as i16)),
        ("Count", Tag::Int(state.count)),
        ("Health", Tag::Short(health)),
        ("Fire", Tag::Short(fire)),
        ("Invulnerable", Tag::Byte(i8::from(state.invulnerable))),
        ("invulnerable_time", Tag::Int(state.invulnerable_time)),
    ] {
        if let Some((_, existing)) = fields.iter_mut().find(|(key, _)| key.matches(name)) {
            *existing = value;
        } else {
            fields.push((name.into(), value));
        }
    }
    Ok(OrbDraft {
        source_nbt,
        document,
    })
}

pub fn read(entity: &Tag) -> Result<Option<SavedOrb>> {
    ensure!(
        matches!(entity, Tag::Compound(_)),
        "entity must be a compound"
    );
    let id = match entity.get("id") {
        Some(Tag::String(value)) => {
            ResourceId::parse(&String::from_utf16(&value.0).context("invalid entity id UTF-16")?)?
        }
        _ => bail!("entity is missing a valid id"),
    };
    if id.namespace != "minecraft" || id.path != "experience_orb" {
        return Ok(None);
    }
    let named = NamedTag {
        name: "".into(),
        value: entity.clone(),
    };
    let position = player_data::read_saved_position(&named)?
        .context("saved experience orb is missing its position")?;
    let motion = player_data::read_saved_motion(&named)?;
    let count = entity
        .get("Count")
        .and_then(player_data::numeric_int)
        .filter(|count| *count > 0)
        .unwrap_or(1);
    Ok(Some(SavedOrb {
        source: Arc::new(named.value),
        uuid: saved_item::optional_uuid(entity, "UUID")?,
        position,
        velocity: motion.velocity,
        on_ground: motion.on_ground,
        no_gravity: entity
            .get("NoGravity")
            .and_then(player_data::numeric_byte)
            .is_some_and(|value| value != 0),
        age: i32::from(saved_item::short(entity.get("Age"), 0)),
        value: i32::from(saved_item::short(entity.get("Value"), 0)),
        count,
        health: i32::from(saved_item::short(entity.get("Health"), 5)),
        fire_ticks: saved_item::short(entity.get("Fire"), 0),
        invulnerable: entity
            .get("Invulnerable")
            .and_then(player_data::numeric_byte)
            .is_some_and(|value| value != 0),
        invulnerable_time: entity
            .get("invulnerable_time")
            .and_then(player_data::numeric_int)
            .unwrap_or(0),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn original() -> NamedTag {
        NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("id".into(), Tag::String("minecraft:experience_orb".into())),
                ("UUID".into(), Tag::IntArray(vec![1, 2, 3, 4])),
                (
                    "Pos".into(),
                    Tag::List {
                        element_type: TagType::Double,
                        values: [1.25, 64.0, -2.5].into_iter().map(Tag::Double).collect(),
                    },
                ),
                ("Age".into(), Tag::Short(12)),
                ("Value".into(), Tag::Short(7)),
                ("Count".into(), Tag::Int(3)),
                ("Health".into(), Tag::Short(5)),
                ("unknown_plugin_field".into(), Tag::Long(987654321)),
            ]),
        }
    }
    fn state() -> OrbSaveState {
        OrbSaveState {
            position: DVec3::new(2.0, 63.0, -2.0),
            velocity: DVec3::new(0.1, -0.2, 0.3),
            on_ground: true,
            no_gravity: false,
            age: 42,
            value: 7,
            count: 5,
            health: 1,
            fire_ticks: 299,
            invulnerable: true,
            invulnerable_time: 9,
        }
    }

    #[test]
    fn draft_preserves_original_and_rejects_unrepresentable_states() {
        let original = original();
        let result = draft(&original, state()).unwrap();
        assert!(result.matches_source(&original).unwrap());
        let saved = read(&result.document().value).unwrap().unwrap();
        assert_eq!(
            (saved.age, saved.value, saved.count, saved.health),
            (42, 7, 5, 1)
        );
        assert_eq!(
            (
                saved.fire_ticks,
                saved.invulnerable,
                saved.invulnerable_time
            ),
            (299, true, 9)
        );
        assert_eq!(saved.position, state().position);
        assert_eq!(saved.velocity, state().velocity);
        assert_eq!(
            saved.source.get("unknown_plugin_field"),
            Some(&Tag::Long(987654321))
        );
        for invalid in [
            OrbSaveState {
                age: 32768,
                ..state()
            },
            OrbSaveState {
                count: 0,
                ..state()
            },
            OrbSaveState {
                value: 3,
                ..state()
            },
            OrbSaveState {
                velocity: DVec3::splat(11.0),
                ..state()
            },
        ] {
            assert!(draft(&original, invalid).is_err());
        }
        let mut changed = original.clone();
        if let Tag::Compound(fields) = &mut changed.value {
            fields.push(("Age".into(), Tag::Short(1)));
        }
        assert!(draft(&changed, state()).is_err());
        assert!(!result.matches_source(&changed).unwrap());
    }

    #[test]
    fn draft_orb_moves_to_known_empty_entity_chunk_without_changing_source() {
        use crate::saved_falling_block::{
            EntityReplacement, draft_entity_chunks_with_optional_sources,
        };
        use native_world::loaded::ChunkPos;
        use std::collections::BTreeMap;

        let home = ChunkPos { x: 0, z: -1 };
        let target = ChunkPos { x: 1, z: -1 };
        let original = original();
        let chunk = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("Position".into(), Tag::IntArray(vec![home.x, home.z])),
                (
                    "Entities".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![original.value.clone()],
                    },
                ),
            ]),
        };
        let movement = draft(
            &original,
            OrbSaveState {
                position: DVec3::new(17.0, 63.0, -2.0),
                ..state()
            },
        )
        .unwrap();
        let replacements = [EntityReplacement::Orb(&movement)];
        assert!(
            draft_entity_chunks_with_optional_sources(
                &BTreeMap::from([(home, Some(&chunk))]),
                &replacements
            )
            .is_err()
        );
        let originals = BTreeMap::from([(home, Some(&chunk)), (target, None)]);
        let drafts = draft_entity_chunks_with_optional_sources(&originals, &replacements).unwrap();
        assert!(drafts[&home].matches_source(&chunk).unwrap());
        assert!(drafts[&target].source_was_absent());
        let Tag::List { values: source, .. } =
            drafts[&home].document().value.get("Entities").unwrap()
        else {
            panic!()
        };
        let Tag::List {
            values: destination,
            ..
        } = drafts[&target].document().value.get("Entities").unwrap()
        else {
            panic!()
        };
        assert!(source.is_empty());
        assert_eq!(destination.len(), 1);
        let saved = read(&destination[0]).unwrap().unwrap();
        assert_eq!(saved.position, DVec3::new(17.0, 63.0, -2.0));
        assert_eq!(
            saved.source.get("unknown_plugin_field"),
            Some(&Tag::Long(987654321))
        );
        let Tag::List {
            values: original_values,
            ..
        } = chunk.value.get("Entities").unwrap()
        else {
            panic!()
        };
        assert_eq!(original_values, &[original.value]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn saved_fields_match_java_orb_value_input() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let classpath = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("SavedOrbOracle.java");
        std::fs::write(&source, include_str!("../tests/java/SavedOrbOracle.java")).unwrap();
        let run = |command: &mut Command| {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "orb oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&classpath)
            .arg(&source));
        let draft_path = temp.path().join("rust-draft.nbt");
        let draft = draft(&original(), state()).unwrap();
        std::fs::write(
            &draft_path,
            native_persistence::nbt::write(draft.document()).unwrap(),
        )
        .unwrap();
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&classpath)
            .arg("SavedOrbOracle")
            .arg(temp.path())
            .arg(draft_path));
        let expected: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(temp.path().join("expected.json")).unwrap())
                .unwrap();
        for (index, row) in expected.iter().enumerate() {
            let data = std::fs::read(temp.path().join(format!("orb-{index}.nbt"))).unwrap();
            let original = native_persistence::nbt::read(&data).unwrap();
            let saved = read(&original.value).unwrap().unwrap();
            for (key, actual) in [
                ("age", saved.age),
                ("value", saved.value),
                ("health", saved.health),
                ("count", saved.count),
            ] {
                assert_eq!(
                    i64::from(actual),
                    row[key].as_i64().unwrap(),
                    "case {index}, {key}"
                );
            }
            assert_eq!(saved.uuid, Some([1, 2, 3, 4]));
            assert_eq!(*saved.source, original.value);
        }
    }
}
