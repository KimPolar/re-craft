//! Vanilla block-entity data for sulfur generated in protochunks.
use anyhow::{Result, ensure};
use native_persistence::nbt::Tag;
use native_world::loaded::BlockState;

pub(super) fn is_valid_state(state: &BlockState) -> bool {
    state.name == "minecraft:potent_sulfur"
        && state.properties.len() == 1
        && matches!(
            state
                .properties
                .get("potent_sulfur_state")
                .map(String::as_str),
            Some("dry" | "wet" | "dormant" | "erupting" | "continuous")
        )
}

/// The default entity produced when a protochunk placeholder is materialized.
#[derive(Clone, Debug)]
pub struct GeneratedPotentSulfur {
    pub position: [i32; 3],
    pub state: BlockState,
}

impl GeneratedPotentSulfur {
    pub fn new(position: [i32; 3], state: BlockState) -> Result<Self> {
        ensure!(
            is_valid_state(&state),
            "invalid generated potent sulfur state"
        );
        Ok(Self { position, state })
    }

    pub fn block_entity(&self) -> Tag {
        let [x, y, z] = self.position;
        Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:potent_sulfur".into())),
            ("x".into(), Tag::Int(x)),
            ("y".into(), Tag::Int(y)),
            ("z".into(), Tag::Int(z)),
            ("countdown".into(), Tag::Int(-1)),
            ("components".into(), Tag::Compound(vec![])),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::{BTreeMap, BTreeSet},
        path::PathBuf,
        process::Command,
    };

    fn state(value: &str) -> BlockState {
        BlockState {
            name: "minecraft:potent_sulfur".into(),
            properties: BTreeMap::from([("potent_sulfur_state".into(), value.into())]),
        }
    }

    #[test]
    fn generated_entity_validates_complete_state() {
        for value in ["dry", "wet", "dormant", "erupting", "continuous"] {
            let entity = GeneratedPotentSulfur::new([-17, -4, 23], state(value)).unwrap();
            assert_eq!(entity.block_entity().get("countdown"), Some(&Tag::Int(-1)));
        }
        assert!(GeneratedPotentSulfur::new([0; 3], state("invalid")).is_err());
        let mut malformed = state("wet");
        malformed.properties.insert("extra".into(), "true".into());
        assert!(GeneratedPotentSulfur::new([0; 3], malformed).is_err());
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn potent_sulfur_entity_and_state_match_java() -> Result<()> {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game)?;
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        ensure!(manifest["id"] == "26.3", "wrong Minecraft version");
        let temp = tempfile::tempdir()?;
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_owned()];
        for library in manifest["libraries"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing libraries"))?
        {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths)?;
        let source = temp.path().join("PotentSulfurOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/PotentSulfurOracle.java"),
        )?;
        let compile = Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source)
            .output()?;
        ensure!(
            compile.status.success(),
            "Java compilation failed: {}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("PotentSulfurOracle")
            .arg(temp.path())
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut seen = BTreeSet::new();
        for line in String::from_utf8(output.stdout)?.lines() {
            let words: Vec<_> = line.split_whitespace().collect();
            if let [
                "P",
                index,
                variant,
                survives,
                solid,
                liquid,
                has_entity,
                poi,
                post,
            ] = words.as_slice()
            {
                assert_eq!(
                    (*survives, *solid, *liquid, *has_entity, *poi, *post),
                    ("true", "true", "false", "true", "false", "null"),
                    "{line}"
                );
                let index: usize = index.parse()?;
                let entity = GeneratedPotentSulfur::new([index as i32, 8, 0], state(variant))?;
                let actual = entity.block_entity();
                let expected = native_persistence::nbt::read(&std::fs::read(
                    temp.path().join(format!("{index}.nbt")),
                )?)?
                .value;
                let (Tag::Compound(actual_fields), Tag::Compound(expected_fields)) =
                    (&actual, &expected)
                else {
                    anyhow::bail!("invalid entity compound")
                };
                let names =
                    |fields: &Vec<(native_persistence::nbt::JavaString, Tag)>| -> BTreeSet<String> {
                        fields
                            .iter()
                            .map(|(name, _)| name.to_string_lossy())
                            .collect()
                    };
                let fields = names(actual_fields);
                assert_eq!(fields, names(expected_fields), "{line}");
                for field in fields {
                    assert_eq!(
                        actual.get(&field),
                        expected.get(&field),
                        "{line} field {field}"
                    );
                }
                seen.insert((*variant).to_owned());
            }
        }
        assert_eq!(
            seen,
            ["dry", "wet", "dormant", "erupting", "continuous"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        );
        Ok(())
    }
}
