//! Trial-spawner block entities produced by vanilla structure templates.
use anyhow::{Context, Result, ensure};
use native_persistence::nbt::Tag;

#[derive(Clone, Debug)]
pub struct GeneratedTrialSpawner {
    pub position: [i32; 3],
    pub state: native_world::loaded::BlockState,
    normal_config: String,
    ominous_config: String,
}

impl GeneratedTrialSpawner {
    pub fn from_template(
        position: [i32; 3],
        state: native_world::loaded::BlockState,
        nbt: &Tag,
    ) -> Result<Self> {
        ensure!(
            native_world::collision::trial_spawner_state(&state).is_some(),
            "invalid trial spawner state"
        );
        let Tag::Compound(fields) = nbt else {
            anyhow::bail!("trial spawner data is not a compound")
        };
        ensure!(
            fields.len() == 3
                && ["id", "normal_config", "ominous_config"]
                    .iter()
                    .all(|key| fields.iter().filter(|(name, _)| name.matches(key)).count() == 1),
            "unsupported trial spawner data"
        );
        ensure!(
            matches!(nbt.get("id"), Some(Tag::String(id)) if id.matches("minecraft:trial_spawner")),
            "invalid trial spawner entity id"
        );
        let read_config = |name| -> Result<String> {
            let Some(Tag::String(value)) = nbt.get(name) else {
                anyhow::bail!("invalid trial spawner config {name}")
            };
            String::from_utf16(&value.0).context("invalid trial spawner config encoding")
        };
        let normal_config = read_config("normal_config")?;
        let ominous_config = read_config("ominous_config")?;
        let base = normal_config
            .strip_prefix("minecraft:trial_chamber/")
            .and_then(|id| id.strip_suffix("/normal"));
        ensure!(
            base.is_some_and(|id| matches!(
                id,
                "breeze"
                    | "melee/husk"
                    | "melee/spider"
                    | "melee/zombie"
                    | "ranged/poison_skeleton"
                    | "ranged/skeleton"
                    | "ranged/stray"
                    | "slow_ranged/poison_skeleton"
                    | "slow_ranged/skeleton"
                    | "slow_ranged/stray"
                    | "small_melee/baby_zombie"
                    | "small_melee/cave_spider"
                    | "small_melee/silverfish"
                    | "small_melee/slime"
            )) && ominous_config == format!("minecraft:trial_chamber/{}/ominous", base.unwrap()),
            "unsupported trial spawner configurations"
        );
        Ok(Self {
            position,
            state,
            normal_config,
            ominous_config,
        })
    }

    pub fn block_entity(&self) -> Tag {
        let [x, y, z] = self.position;
        Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:trial_spawner".into())),
            ("x".into(), Tag::Int(x)),
            ("y".into(), Tag::Int(y)),
            ("z".into(), Tag::Int(z)),
            (
                "normal_config".into(),
                Tag::String(self.normal_config.as_str().into()),
            ),
            (
                "ominous_config".into(),
                Tag::String(self.ominous_config.as_str().into()),
            ),
            ("components".into(), Tag::Compound(vec![])),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn state() -> native_world::loaded::BlockState {
        native_world::loaded::BlockState {
            name: "minecraft:trial_spawner".into(),
            properties: BTreeMap::from([
                ("ominous".into(), "false".into()),
                ("trial_spawner_state".into(), "inactive".into()),
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
    fn rejects_unrecognized_config_without_discarding_it() {
        let data = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:trial_spawner".into())),
            ("normal_config".into(), Tag::String("example:custom".into())),
            ("ominous_config".into(), Tag::String("example:other".into())),
        ]);
        assert!(GeneratedTrialSpawner::from_template([3, 7, -2], state(), &data).is_err());
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn trial_template_data_matches_java_serialization() -> Result<()> {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or(native_resources::default_minecraft_dir()?);
        let _verified = native_resources::packs::ResourceStack::open(&game)?;
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
        let source = temp.path().join("JigsawTrialSpawnerOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/JigsawTrialSpawnerOracle.java"),
        )?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compile failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("JigsawTrialSpawnerOracle")
            .arg(version.join("26.3.jar"))
            .arg(temp.path())
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        for index in 0..14 {
            let input = native_persistence::nbt::read(&std::fs::read(
                temp.path().join(format!("input-{index}.nbt")),
            )?)?
            .value;
            let mut expected = native_persistence::nbt::read(&std::fs::read(
                temp.path().join(format!("saved-{index}.nbt")),
            )?)?
            .value;
            let mut actual =
                GeneratedTrialSpawner::from_template([3, 7, -2], state(), &input)?.block_entity();
            canonicalize(&mut expected);
            canonicalize(&mut actual);
            assert_eq!(actual, expected, "trial spawner {index}");
        }
        Ok(())
    }
}
