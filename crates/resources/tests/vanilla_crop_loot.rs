//! Compare installed crop loot tables and random consumption with Java 26.3.

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn crop_block_loot_matches_java() {
    use native_core::random::{LegacyRandom, XoroshiroRandom};
    use native_resources::loot::{BasicTool, BlockLootTables};
    use std::{collections::BTreeMap, path::PathBuf, process::Command};

    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let version = game.join("versions/26.3");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for library in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let source = temp.path().join("CropLootOracle.java");
    let output = temp.path().join("crop-loot.json");
    std::fs::write(&source, include_str!("java/CropLootOracle.java")).unwrap();
    for command in [
        Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source),
        Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("CropLootOracle")
            .arg(version.join("26.3.jar"))
            .arg(&output),
    ] {
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "Java crop loot oracle failed:\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
    let tables = BlockLootTables::load(&mut resources).unwrap();
    let rows: Vec<serde_json::Value> =
        serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(rows.len(), 28 * 3 * 2 * 32);
    for row in rows {
        let name =
            ["wheat", "carrots", "potatoes", "beetroots"][row["kind"].as_u64().unwrap() as usize];
        let block = format!("minecraft:{name}");
        let properties = BTreeMap::from([("age".into(), row["age"].as_u64().unwrap().to_string())]);
        let seed = row["seed"].as_i64().unwrap();
        let level = row["level"].as_u64().unwrap() as u32;
        let enchantments = [(
            native_resources::packs::ResourceId::parse("minecraft:fortune").unwrap(),
            level,
        )];
        let tool = BasicTool {
            item: Some("minecraft:diamond_hoe"),
            count: 1,
            enchantments: &enchantments,
        };
        let harvest_tool = if level == 0 {
            BasicTool {
                item: None,
                count: 0,
                enchantments: &[],
            }
        } else {
            tool
        };
        assert!(
            tables.fully_supported_player_block_for_tool(&block, harvest_tool),
            "unsupported player harvest: {row}"
        );
        let (drops, next) = if row["mode"].as_u64().unwrap() == 0 {
            let mut rng = LegacyRandom::new(seed);
            let drops = if level == 0 {
                tables.drops_without_entity_at(&block, Some(&properties), Some(&mut rng))
            } else {
                tables.drops_with_entity_at(&block, &properties, tool, Some(&mut rng))
            }
            .unwrap();
            (drops, rng.next_long())
        } else {
            let mut rng = XoroshiroRandom::new(seed);
            let drops = if level == 0 {
                tables.drops_without_entity_at(&block, Some(&properties), Some(&mut rng))
            } else {
                tables.drops_with_entity_at(&block, &properties, tool, Some(&mut rng))
            }
            .unwrap();
            (drops, rng.next_long())
        };
        let actual: Vec<_> = drops
            .into_iter()
            .map(|drop| {
                serde_json::json!([
                    format!("{}:{}", drop.item.namespace, drop.item.path),
                    drop.count
                ])
            })
            .collect();
        assert_eq!(serde_json::Value::Array(actual), row["drops"], "{row}");
        assert_eq!(next, row["next"].as_i64().unwrap(), "{row}");
    }
}
