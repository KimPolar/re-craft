//! Verified trial-chamber decorated-pot data before world publication.
use anyhow::{Context, Result, ensure};
use native_persistence::nbt::Tag;

#[derive(Clone, Debug)]
pub struct GeneratedDecoratedPot {
    pub position: [i32; 3],
    pub state: native_world::loaded::BlockState,
    sherds: Tag,
    content: PotContent,
}

#[derive(Clone, Copy, Debug)]
enum PotContent {
    Loot { seed: i64 },
    StringItem,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn state() -> native_world::loaded::BlockState {
        native_world::loaded::BlockState {
            name: "minecraft:decorated_pot".into(),
            properties: BTreeMap::from([
                ("cracked".into(), "false".into()),
                ("facing".into(), "north".into()),
                ("waterlogged".into(), "false".into()),
            ]),
        }
    }
    fn template() -> Tag {
        let sherds = ["back", "front", "left", "right"]
            .into_iter()
            .map(|side| {
                (
                    side.into(),
                    Tag::Compound(vec![("id".into(), Tag::String("minecraft:brick".into()))]),
                )
            })
            .collect();
        Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:decorated_pot".into())),
            ("sherds".into(), Tag::Compound(sherds)),
            (
                "LootTable".into(),
                Tag::String("minecraft:pots/trial_chambers/corridor".into()),
            ),
        ])
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
    fn unknown_pot_data_is_rejected_instead_of_discarded() {
        let mut nbt = template();
        if let Tag::Compound(fields) = &mut nbt {
            fields.push(("extra".into(), Tag::Int(1)));
        }
        assert!(GeneratedDecoratedPot::from_template([3, 7, -2], state(), &nbt, 0).is_err());
        let mut bad_state = state();
        bad_state.properties.insert("facing".into(), "up".into());
        assert!(
            GeneratedDecoratedPot::from_template([3, 7, -2], bad_state, &template(), 0).is_err()
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn trial_template_data_matches_java_serialization() -> Result<()> {
        use anyhow::{Context, ensure};
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
        let source = temp.path().join("JigsawDecoratedPotOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/JigsawDecoratedPotOracle.java"),
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
            .arg("JigsawDecoratedPotOracle")
            .arg(version.join("26.3.jar"))
            .arg(temp.path())
            .output()?;
        ensure!(
            run.status.success(),
            "Java oracle failed: {}\n{}",
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        );
        for index in 0..5 {
            let input = native_persistence::nbt::read(&std::fs::read(
                temp.path().join(format!("input-{index}.nbt")),
            )?)?
            .value;
            for (seed_index, seed) in [0, 0x123456789abcdef, -1].into_iter().enumerate() {
                let mut expected = native_persistence::nbt::read(&std::fs::read(
                    temp.path().join(format!("saved-{index}-{seed_index}.nbt")),
                )?)?
                .value;
                let mut actual =
                    GeneratedDecoratedPot::from_template([3, 7, -2], state(), &input, seed)?
                        .block_entity();
                canonicalize(&mut expected);
                canonicalize(&mut actual);
                assert_eq!(actual, expected, "pot {index}, seed {seed}");
            }
        }
        Ok(())
    }
}

fn fields_exact(tag: &Tag, expected: &[&str]) -> bool {
    let Tag::Compound(fields) = tag else {
        return false;
    };
    fields.len() == expected.len()
        && expected
            .iter()
            .all(|name| fields.iter().filter(|(key, _)| key.matches(name)).count() == 1)
}

impl GeneratedDecoratedPot {
    pub fn from_template(
        position: [i32; 3],
        state: native_world::loaded::BlockState,
        nbt: &Tag,
        seed: i64,
    ) -> Result<Self> {
        ensure!(
            native_world::collision::decorated_pot_waterlogged(&state).is_some(),
            "invalid decorated pot state"
        );
        ensure!(
            matches!(nbt.get("id"), Some(Tag::String(id)) if id.matches("minecraft:decorated_pot")),
            "invalid decorated pot entity id"
        );
        let sherds = nbt.get("sherds").context("decorated pot sherds missing")?;
        ensure!(
            fields_exact(sherds, &["back", "front", "left", "right"]),
            "unsupported decorated pot sherds"
        );
        for face in ["back", "front", "left", "right"] {
            let sherd = sherds.get(face).context("decorated pot sherd missing")?;
            ensure!(
                fields_exact(sherd, &["id"]),
                "unsupported decorated pot sherd data"
            );
            ensure!(
                matches!(sherd.get("id"), Some(Tag::String(id)) if ["minecraft:brick", "minecraft:flow_pottery_sherd", "minecraft:guster_pottery_sherd", "minecraft:scrape_pottery_sherd"].iter().any(|value| id.matches(value))),
                "unsupported decorated pot sherd item"
            );
        }
        let content = if fields_exact(nbt, &["id", "sherds", "LootTable"]) {
            ensure!(
                matches!(nbt.get("LootTable"), Some(Tag::String(id)) if id.matches("minecraft:pots/trial_chambers/corridor")),
                "unsupported decorated pot loot table"
            );
            PotContent::Loot { seed }
        } else {
            ensure!(
                fields_exact(nbt, &["id", "sherds", "item"]),
                "unsupported decorated pot entity fields"
            );
            let item = nbt.get("item").context("decorated pot item missing")?;
            ensure!(
                fields_exact(item, &["id", "count"]),
                "unsupported decorated pot item fields"
            );
            ensure!(
                matches!(item.get("id"), Some(Tag::String(id)) if id.matches("minecraft:string"))
                    && item.get("count") == Some(&Tag::Int(3)),
                "unsupported decorated pot item"
            );
            PotContent::StringItem
        };
        Ok(Self {
            position,
            state,
            sherds: sherds.clone(),
            content,
        })
    }

    pub fn block_entity(&self) -> Tag {
        let [x, y, z] = self.position;
        let mut fields = vec![
            ("id".into(), Tag::String("minecraft:decorated_pot".into())),
            ("x".into(), Tag::Int(x)),
            ("y".into(), Tag::Int(y)),
            ("z".into(), Tag::Int(z)),
            ("sherds".into(), self.sherds.clone()),
            ("components".into(), Tag::Compound(vec![])),
        ];
        match self.content {
            PotContent::Loot { seed } => {
                fields.push((
                    "LootTable".into(),
                    Tag::String("minecraft:pots/trial_chambers/corridor".into()),
                ));
                if seed != 0 {
                    fields.push(("LootTableSeed".into(), Tag::Long(seed)));
                }
            }
            PotContent::StringItem => fields.push((
                "item".into(),
                Tag::Compound(vec![
                    ("id".into(), Tag::String("minecraft:string".into())),
                    ("count".into(), Tag::Int(3)),
                ]),
            )),
        }
        Tag::Compound(fields)
    }
}
