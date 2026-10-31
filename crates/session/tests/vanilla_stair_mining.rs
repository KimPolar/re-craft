//! Compare every vanilla stair's default mining behavior with Java 26.3.

use native_persistence::nbt::Tag;
use native_session::{
    inventory::ItemStack,
    mining::{self, Material},
};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn stair_mining_matches_vanilla_tools() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
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
    let cp = std::env::join_paths(paths).unwrap();
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let source = temp.path().join("StairMiningOracle.java");
    let output = temp.path().join("mining.json");
    std::fs::write(&source, include_str!("java/StairMiningOracle.java")).unwrap();
    for command in [
        Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source),
        Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("StairMiningOracle")
            .arg(&output)
            .arg(version.join("26.3.jar")),
    ] {
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "stair mining oracle failed:\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(rows.len(), 97);
    let mut tools = Vec::new();
    for material in [
        "wooden",
        "stone",
        "copper",
        "iron",
        "golden",
        "diamond",
        "netherite",
    ] {
        for kind in ["sword", "shovel", "pickaxe", "axe", "hoe"] {
            tools.push(format!("minecraft:{material}_{kind}"));
        }
    }
    tools.push("minecraft:shears".into());
    for row in &rows {
        let id = row["id"].as_str().unwrap();
        let material = Material::from_block_name(id).unwrap();
        assert_eq!(
            material.hardness().to_bits(),
            (row["hardness"].as_f64().unwrap() as f32).to_bits(),
            "{id}"
        );
        assert_eq!(
            material.needs_tool(),
            row["needs_tool"].as_bool().unwrap(),
            "{id}"
        );
        for (tool, expected) in tools.iter().zip(row["tools"].as_array().unwrap()) {
            let item = ItemStack::new(tool.clone(), 1, 1, Tag::Compound(vec![])).unwrap();
            let props = mining::mining_properties(material, Some(&item)).unwrap();
            assert_eq!(
                props.speed.to_bits(),
                (expected[0].as_f64().unwrap() as f32).to_bits(),
                "{id} {tool}"
            );
            assert_eq!(
                props.correct_for_drops,
                expected[1].as_bool().unwrap(),
                "{id} {tool}"
            );
            assert_eq!(
                props.damage_per_block,
                expected[2].as_u64().unwrap() as u32,
                "{id} {tool}"
            );
            assert_eq!(
                props.can_destroy_in_creative,
                expected[3].as_bool().unwrap(),
                "{id} {tool}"
            );
            assert_eq!(
                native_session::inventory::ordinary_item_default_durability(tool),
                Some(expected[4].as_u64().unwrap() as u32),
                "{tool}"
            );
            let mut n = 0;
            for ground in [false, true] {
                for water in [false, true] {
                    let actual = mining::tool_progress(material, Some(&item), ground, water)
                        .unwrap()
                        .to_bits();
                    assert_eq!(
                        actual,
                        expected[5][n].as_i64().unwrap() as i32 as u32,
                        "{id} {tool} ground={ground} water={water}"
                    );
                    n += 1;
                }
            }
        }
    }
    eprintln!(
        "Compared 97 stair materials, 36 tools and 13,968 progress conditions with Java 26.3"
    );
}
