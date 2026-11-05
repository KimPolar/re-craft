//! Deferred loot in freshly generated archaeology block entities.
use anyhow::{Context, Result, ensure};
use native_persistence::nbt::Tag;
use native_resources::packs::ResourceId;
use native_world::loaded::BlockState;

use super::ocean_ruin::ArchaeologyReplacement;

/// A materialized brushable block entity created from a structure processor.
/// Unknown input fields are rejected until their load semantics are modeled.
#[derive(Clone, Debug)]
pub struct GeneratedBrushable {
    pub position: [i32; 3],
    pub state: BlockState,
    loot_table: ResourceId,
    loot_seed: i64,
}

impl GeneratedBrushable {
    pub fn from_replacement(
        position: [i32; 3],
        replacement: &ArchaeologyReplacement,
    ) -> Result<Self> {
        ensure!(
            matches!(
                replacement.state.name.as_str(),
                "minecraft:suspicious_sand" | "minecraft:suspicious_gravel"
            ) && replacement.state.properties.len() == 1
                && replacement
                    .state
                    .properties
                    .get("dusted")
                    .map(String::as_str)
                    == Some("0"),
            "unsupported generated brushable state"
        );
        let Tag::Compound(entries) = &replacement.nbt else {
            anyhow::bail!("brushable data is not a compound")
        };
        ensure!(entries.len() == 2, "unsupported generated brushable data");
        let mut table = None;
        let mut seed = None;
        for (name, value) in entries {
            if name.matches("LootTable") && table.is_none() {
                let Tag::String(id) = value else {
                    anyhow::bail!("invalid brushable loot table")
                };
                table = Some(ResourceId::parse(
                    &String::from_utf16(&id.0).context("invalid brushable loot table")?,
                )?);
            } else if name.matches("LootTableSeed") && seed.is_none() {
                let Tag::Long(value) = value else {
                    anyhow::bail!("invalid brushable loot seed")
                };
                seed = Some(*value);
            } else {
                anyhow::bail!("unsupported generated brushable data")
            }
        }
        Ok(Self {
            position,
            state: replacement.state.clone(),
            loot_table: table.context("missing brushable loot table")?,
            loot_seed: seed.context("missing brushable loot seed")?,
        })
    }

    pub fn block_entity(&self) -> Tag {
        let [x, y, z] = self.position;
        let loot_table = format!("{}:{}", self.loot_table.namespace, self.loot_table.path);
        let mut fields = vec![
            ("id".into(), Tag::String("minecraft:brushable_block".into())),
            ("x".into(), Tag::Int(x)),
            ("y".into(), Tag::Int(y)),
            ("z".into(), Tag::Int(z)),
            ("LootTable".into(), Tag::String(loot_table.as_str().into())),
        ];
        if self.loot_seed != 0 {
            fields.push(("LootTableSeed".into(), Tag::Long(self.loot_seed)));
        }
        fields.push(("components".into(), Tag::Compound(vec![])));
        Tag::Compound(fields)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn replacement(sand: bool, seed: i64) -> ArchaeologyReplacement {
        ArchaeologyReplacement {
            state: BlockState {
                name: if sand {
                    "minecraft:suspicious_sand"
                } else {
                    "minecraft:suspicious_gravel"
                }
                .into(),
                properties: BTreeMap::from([("dusted".into(), "0".into())]),
            },
            nbt: Tag::Compound(vec![
                (
                    "LootTable".into(),
                    Tag::String("minecraft:archaeology/ocean_ruin_warm".into()),
                ),
                ("LootTableSeed".into(), Tag::Long(seed)),
            ]),
        }
    }

    #[test]
    fn preserves_deferred_loot_and_rejects_unmodeled_data() {
        let mut input = replacement(true, 0);
        let result = GeneratedBrushable::from_replacement([-1, 4, 16], &input)
            .unwrap()
            .block_entity();
        assert_eq!(result.get("LootTableSeed"), None);
        assert_eq!(result.get("x"), Some(&Tag::Int(-1)));
        assert!(
            matches!(result.get("LootTable"), Some(Tag::String(id)) if id.matches("minecraft:archaeology/ocean_ruin_warm"))
        );
        let Tag::Compound(fields) = &mut input.nbt else {
            unreachable!()
        };
        fields.push(("unknown".into(), Tag::Int(1)));
        assert!(GeneratedBrushable::from_replacement([0; 3], &input).is_err());
        let mut input = replacement(false, -7);
        input.state.properties.insert("dusted".into(), "1".into());
        assert!(GeneratedBrushable::from_replacement([0; 3], &input).is_err());
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn brushable_block_entity_matches_java() -> Result<()> {
        use native_persistence::nbt::{self, NamedTag};
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        ensure!(manifest["id"] == "26.3", "wrong version");
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for lib in manifest["libraries"]
            .as_array()
            .context("missing libraries")?
        {
            if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths)?;
        let source = temp.path().join("BrushableBlockEntityOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/BrushableBlockEntityOracle.java"),
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
        let mut cases = Vec::new();
        for (i, seed) in [0, 1, -1, i64::MIN, i64::MAX].into_iter().enumerate() {
            for sand in [false, true] {
                let position = [i as i32 - 17, -32 + i as i32, 20 - i as i32];
                let value =
                    GeneratedBrushable::from_replacement(position, &replacement(sand, seed))?
                        .block_entity();
                let path = temp.path().join(format!("brushable-{i}-{sand}.nbt"));
                std::fs::write(
                    &path,
                    nbt::write(&NamedTag {
                        name: "".into(),
                        value,
                    })?,
                )?;
                cases.push(serde_json::json!({"x":position[0],"y":position[1],"z":position[2],"sand":sand,"table":"minecraft:archaeology/ocean_ruin_warm","seed":seed,"path":path}));
            }
        }
        let input = temp.path().join("cases.json");
        std::fs::write(&input, serde_json::to_vec(&cases)?)?;
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(cp)
            .arg("BrushableBlockEntityOracle")
            .arg(input)
            .output()?;
        ensure!(
            output.status.success(),
            "Java NBT comparison failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        ensure!(
            String::from_utf8(output.stdout)?
                .lines()
                .any(|line| line == format!("OK\t{}", cases.len())),
            "missing oracle result"
        );
        Ok(())
    }
}
