//! Deferred barrel contents produced by structure templates.
use anyhow::{Context, Result, bail, ensure};
use native_persistence::nbt::{Tag, TagType};
use native_resources::packs::ResourceId;

#[derive(Clone, Debug)]
pub struct GeneratedBarrel {
    pub position: [i32; 3],
    pub state: native_world::loaded::BlockState,
    loot: Option<(String, i64)>,
}

impl GeneratedBarrel {
    pub fn from_template(
        position: [i32; 3],
        state: native_world::loaded::BlockState,
        nbt: &Tag,
        seed: i64,
    ) -> Result<Self> {
        ensure!(
            native_world::collision::barrel_state(&state),
            "invalid barrel state"
        );
        let Tag::Compound(fields) = nbt else {
            bail!("barrel data is not a compound")
        };
        let mut keys = std::collections::BTreeSet::new();
        ensure!(
            fields
                .iter()
                .all(|(key, _)| ["id", "LootTable", "LootTableSeed", "Items"]
                    .iter()
                    .any(|name| key.matches(name))
                    && keys.insert(key.0.clone())),
            "unsupported barrel data"
        );
        ensure!(
            matches!(nbt.get("id"), Some(Tag::String(id)) if id.matches("minecraft:barrel")),
            "invalid barrel entity id"
        );
        ensure!(
            matches!(nbt.get("LootTableSeed"), None | Some(Tag::Long(_))),
            "invalid barrel loot seed"
        );
        let loot = match nbt.get("LootTable") {
            Some(Tag::String(table)) => {
                ensure!(
                    nbt.get("Items").is_none(),
                    "barrel loot and inventory cannot both be preserved"
                );
                let table = String::from_utf16(&table.0).context("invalid barrel loot table")?;
                ResourceId::parse(&table)?;
                Some((table, seed))
            }
            None => {
                let empty = match nbt.get("Items") {
                    None => true,
                    Some(Tag::List {
                        element_type: TagType::End | TagType::Compound,
                        values,
                    }) => values.is_empty(),
                    _ => false,
                };
                ensure!(empty, "nonempty barrel inventory is not yet supported");
                None
            }
            _ => bail!("invalid barrel loot table"),
        };
        Ok(Self {
            position,
            state,
            loot,
        })
    }

    pub fn block_entity(&self) -> Tag {
        let [x, y, z] = self.position;
        let mut fields = vec![
            ("id".into(), Tag::String("minecraft:barrel".into())),
            ("x".into(), Tag::Int(x)),
            ("y".into(), Tag::Int(y)),
            ("z".into(), Tag::Int(z)),
            ("components".into(), Tag::Compound(vec![])),
        ];
        if let Some((table, seed)) = &self.loot {
            fields.push(("LootTable".into(), Tag::String(table.as_str().into())));
            if *seed != 0 {
                fields.push(("LootTableSeed".into(), Tag::Long(*seed)));
            }
        } else {
            fields.push((
                "Items".into(),
                Tag::List {
                    element_type: TagType::End,
                    values: vec![],
                },
            ));
        }
        Tag::Compound(fields)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn state() -> native_world::loaded::BlockState {
        native_world::loaded::BlockState {
            name: "minecraft:barrel".into(),
            properties: BTreeMap::from([
                ("facing".into(), "up".into()),
                ("open".into(), "false".into()),
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
    fn rejects_unknown_barrel_data() {
        let nbt = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:barrel".into())),
            ("unknown".into(), Tag::Int(1)),
        ]);
        assert!(GeneratedBarrel::from_template([3, 7, -2], state(), &nbt, 0).is_err());
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
        let source = temp.path().join("JigsawBarrelOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/JigsawBarrelOracle.java"),
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
            .arg("JigsawBarrelOracle")
            .arg(version.join("26.3.jar"))
            .arg(temp.path())
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        for index in 0..3 {
            let input = native_persistence::nbt::read(&std::fs::read(
                temp.path().join(format!("input-{index}.nbt")),
            )?)?
            .value;
            for (seed_index, seed) in [0, 0x123456789abcdef, -1].into_iter().enumerate() {
                let mut expected = native_persistence::nbt::read(&std::fs::read(
                    temp.path().join(format!("saved-{index}-{seed_index}.nbt")),
                )?)?
                .value;
                let mut actual = GeneratedBarrel::from_template([3, 7, -2], state(), &input, seed)?
                    .block_entity();
                canonicalize(&mut expected);
                canonicalize(&mut actual);
                assert_eq!(actual, expected, "barrel {index}, seed {seed}");
            }
        }
        Ok(())
    }
}
