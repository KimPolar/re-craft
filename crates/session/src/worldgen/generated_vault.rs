//! Verified trial-chamber vault block-entity data before world publication.
use anyhow::{Result, ensure};
use native_persistence::nbt::Tag;

/// The two vanilla template configurations whose save format is verified.
#[derive(Clone, Debug)]
pub struct GeneratedVault {
    pub position: [i32; 3],
    pub state: native_world::loaded::BlockState,
    ominous_key: bool,
}

impl GeneratedVault {
    pub fn from_template(
        position: [i32; 3],
        state: native_world::loaded::BlockState,
        nbt: &Tag,
    ) -> Result<Self> {
        ensure!(
            native_world::collision::vault_state(&state).is_some(),
            "invalid vault state"
        );
        let config = nbt
            .get("config")
            .ok_or_else(|| anyhow::anyhow!("vault configuration missing"))?;
        let key = config
            .get("key_item")
            .ok_or_else(|| anyhow::anyhow!("vault key missing"))?;
        fn fields_exact(tag: &Tag, expected: &[&str]) -> bool {
            let Tag::Compound(fields) = tag else {
                return false;
            };
            fields.len() == expected.len()
                && expected
                    .iter()
                    .all(|name| fields.iter().filter(|(key, _)| key.matches(name)).count() == 1)
        }
        ensure!(
            fields_exact(nbt, &["id", "config"]),
            "unsupported vault template fields"
        );
        ensure!(
            fields_exact(config, &["key_item", "loot_table"]),
            "unsupported vault configuration fields"
        );
        ensure!(
            fields_exact(key, &["id", "count"]),
            "unsupported vault key fields"
        );
        ensure!(
            matches!(nbt.get("id"), Some(Tag::String(id)) if id.matches("minecraft:vault")),
            "invalid vault entity id"
        );
        ensure!(
            key.get("count") == Some(&Tag::Int(1)),
            "unsupported vault key count"
        );
        let normal = matches!(key.get("id"), Some(Tag::String(id)) if id.matches("minecraft:trial_key"))
            && matches!(config.get("loot_table"), Some(Tag::String(id)) if id.matches("minecraft:chests/trial_chambers/reward"));
        let ominous = matches!(key.get("id"), Some(Tag::String(id)) if id.matches("minecraft:ominous_trial_key"))
            && matches!(config.get("loot_table"), Some(Tag::String(id)) if id.matches("minecraft:chests/trial_chambers/reward_ominous"));
        ensure!(normal || ominous, "unsupported vault key or loot table");
        Ok(Self {
            position,
            state,
            ominous_key: ominous,
        })
    }

    pub fn block_entity(&self) -> Tag {
        let [x, y, z] = self.position;
        let key_id = if self.ominous_key {
            "minecraft:ominous_trial_key"
        } else {
            "minecraft:trial_key"
        };
        let mut config = vec![(
            "key_item".into(),
            Tag::Compound(vec![
                ("id".into(), Tag::String(key_id.into())),
                ("count".into(), Tag::Int(1)),
            ]),
        )];
        if self.ominous_key {
            config.push((
                "loot_table".into(),
                Tag::String("minecraft:chests/trial_chambers/reward_ominous".into()),
            ));
        }
        Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:vault".into())),
            ("x".into(), Tag::Int(x)),
            ("y".into(), Tag::Int(y)),
            ("z".into(), Tag::Int(z)),
            ("config".into(), Tag::Compound(config)),
            ("server_data".into(), Tag::Compound(vec![])),
            ("shared_data".into(), Tag::Compound(vec![])),
            ("components".into(), Tag::Compound(vec![])),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn template(ominous: bool) -> Tag {
        let key = if ominous {
            "minecraft:ominous_trial_key"
        } else {
            "minecraft:trial_key"
        };
        let loot = if ominous {
            "minecraft:chests/trial_chambers/reward_ominous"
        } else {
            "minecraft:chests/trial_chambers/reward"
        };
        Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:vault".into())),
            (
                "config".into(),
                Tag::Compound(vec![
                    (
                        "key_item".into(),
                        Tag::Compound(vec![
                            ("id".into(), Tag::String(key.into())),
                            ("count".into(), Tag::Int(1)),
                        ]),
                    ),
                    ("loot_table".into(), Tag::String(loot.into())),
                ]),
            ),
        ])
    }

    fn state() -> native_world::loaded::BlockState {
        native_world::loaded::BlockState {
            name: "minecraft:vault".into(),
            properties: BTreeMap::from([
                ("facing".into(), "north".into()),
                ("ominous".into(), "false".into()),
                ("vault_state".into(), "inactive".into()),
            ]),
        }
    }

    fn canonicalize(tag: &mut Tag) {
        match tag {
            Tag::Compound(fields) => {
                for (_, value) in fields.iter_mut() {
                    canonicalize(value);
                }
                fields.sort_by(|a, b| a.0.0.cmp(&b.0.0));
            }
            Tag::List { values, .. } => values.iter_mut().for_each(canonicalize),
            _ => {}
        }
    }

    #[test]
    fn rejects_unrecognized_vault_data_without_dropping_it() {
        let mut input = template(false);
        if let Tag::Compound(fields) = &mut input {
            fields.push(("extra".into(), Tag::Int(42)));
        }
        assert!(GeneratedVault::from_template([3, 7, -2], state(), &input).is_err());
        let mut invalid_state = state();
        invalid_state
            .properties
            .insert("ominous".into(), "unknown".into());
        assert!(
            GeneratedVault::from_template([3, 7, -2], invalid_state, &template(false)).is_err()
        );
        let mut input = template(true);
        if let Tag::Compound(fields) = &mut input {
            fields.push(("id".into(), Tag::String("minecraft:vault".into())));
        }
        assert!(GeneratedVault::from_template([3, 7, -2], state(), &input).is_err());
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn template_vault_data_matches_java_serialization() -> Result<()> {
        use anyhow::{Context, ensure};
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or(native_resources::default_minecraft_dir()?);
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for lib in manifest["libraries"]
            .as_array()
            .context("libraries missing")?
        {
            if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths)?;
        let source = temp.path().join("JigsawVaultOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/JigsawVaultOracle.java"),
        )?;
        let compile = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source)
            .output()?;
        ensure!(
            compile.status.success(),
            "Java compile failed: {}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let run = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("JigsawVaultOracle")
            .arg(version.join("26.3.jar"))
            .arg(temp.path())
            .output()?;
        ensure!(
            run.status.success(),
            "Java oracle failed: {}\n{}",
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        );
        for (ominous, name) in [(false, "normal.nbt"), (true, "ominous.nbt")] {
            let mut expected =
                native_persistence::nbt::read(&std::fs::read(temp.path().join(name))?)?.value;
            let mut actual =
                GeneratedVault::from_template([3, 7, -2], state(), &template(ominous))?
                    .block_entity();
            canonicalize(&mut expected);
            canonicalize(&mut actual);
            assert_eq!(actual, expected, "{name}");
        }
        Ok(())
    }
}
