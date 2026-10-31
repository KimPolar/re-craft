//! Compare data-pack block-tag changes with Java's default tool components.

use native_persistence::nbt::Tag;
use native_session::{
    inventory::ItemStack,
    mining::{self, Material},
};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn removed_tool_component_matches_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let source = temp.path().join("RemovedToolOracle.java");
    let output = temp.path().join("removed.bin");
    std::fs::write(&source, include_str!("java/RemovedToolOracle.java")).unwrap();
    for command in [
        Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source),
        Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("RemovedToolOracle")
            .arg(&output),
    ] {
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "Java oracle failed:\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let bytes = std::fs::read(output).unwrap();
    assert_eq!(bytes.len(), 3 * 3 * 5);
    let mut offset = 0;
    for id in ["diamond_sword", "iron_pickaxe", "shears"] {
        let components = Tag::Compound(vec![("!minecraft:tool".into(), Tag::Compound(vec![]))]);
        let id = format!("minecraft:{id}");
        let max = native_session::inventory::saved_item_max(&id, &components).unwrap();
        let item = ItemStack::new(id.clone(), 1, max, components).unwrap();
        for block in ["minecraft:stone", "minecraft:dirt", "minecraft:oak_stairs"] {
            let properties =
                mining::mining_properties(Material::from_block_name(block).unwrap(), Some(&item))
                    .unwrap();
            assert_eq!(
                properties.speed.to_bits(),
                u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()),
                "{id} {block}"
            );
            assert_eq!(
                properties.correct_for_drops,
                bytes[offset + 4] != 0,
                "{id} {block}"
            );
            assert_eq!(properties.damage_per_block, 0);
            assert!(properties.can_destroy_in_creative);
            offset += 5;
        }
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn default_tool_rules_match_java_with_changed_block_tags() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let source = temp.path().join("MiningTagsOracle.java");
    let output = temp.path().join("tags.bin");
    std::fs::write(&source, include_str!("java/MiningTagsOracle.java")).unwrap();
    for command in [
        Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source),
        Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("MiningTagsOracle")
            .arg(&output),
    ] {
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "mining tags oracle failed:\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let bytes = std::fs::read(output).unwrap();
    assert_eq!(bytes.len(), 2048 * (2 + 2 * 36 * 7));
    let mut tools = Vec::new();
    for tier in [
        "wooden",
        "stone",
        "copper",
        "iron",
        "golden",
        "diamond",
        "netherite",
    ] {
        for kind in ["sword", "shovel", "pickaxe", "axe", "hoe"] {
            tools.push(format!("minecraft:{tier}_{kind}"));
        }
    }
    tools.push("minecraft:shears".into());
    let blocks = [
        Material::from_block_name("minecraft:oak_stairs").unwrap(),
        Material::from_block_name("minecraft:stone_stairs").unwrap(),
    ];
    let mut offset = 0;
    for _ in 0..2048 {
        let flags = u16::from_be_bytes(bytes[offset..offset + 2].try_into().unwrap());
        offset += 2;
        for material in blocks {
            for tool in &tools {
                let speed = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap());
                let correct = bytes[offset + 4] != 0;
                let damage = u32::from(bytes[offset + 5]);
                let creative = bytes[offset + 6] != 0;
                offset += 7;
                let stack = ItemStack::new(tool.clone(), 1, 1, Tag::Compound(vec![])).unwrap();
                let actual =
                    mining::mining_properties_with_tags(material, Some(&stack), flags).unwrap();
                assert_eq!(
                    actual.speed.to_bits(),
                    speed,
                    "{material:?} {tool} flags={flags:04x}"
                );
                assert_eq!(
                    actual.correct_for_drops, correct,
                    "{material:?} {tool} flags={flags:04x}"
                );
                assert_eq!(actual.damage_per_block, damage, "{tool}");
                assert_eq!(actual.can_destroy_in_creative, creative, "{tool}");
            }
        }
    }
    eprintln!("Compared 147,456 tool/block/tag combinations with Java 26.3");
}

#[test]
#[ignore = "requires Minecraft 26.3 assets"]
fn installed_tags_match_verified_default_stair_rules() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
    let registry = resources.vanilla_block_ids().unwrap();
    let tags = native_resources::mining_tags::MiningTags::load(&mut resources, &registry).unwrap();
    let mut tools = Vec::new();
    for tier in [
        "wooden",
        "stone",
        "copper",
        "iron",
        "golden",
        "diamond",
        "netherite",
    ] {
        for kind in ["sword", "shovel", "pickaxe", "axe", "hoe"] {
            tools.push(format!("minecraft:{tier}_{kind}"));
        }
    }
    tools.push("minecraft:shears".into());
    for name in native_core::stairs::ITEMS {
        let id = format!("minecraft:{name}");
        let material = Material::from_block_name(&id).unwrap();
        let flags = tags.for_block(&id).unwrap();
        for tool in &tools {
            let item = ItemStack::new(tool.clone(), 1, 1, Tag::Compound(vec![])).unwrap();
            assert_eq!(
                mining::mining_properties_with_tags(material, Some(&item), flags),
                mining::mining_properties(material, Some(&item)),
                "{id} {tool}"
            );
        }
    }
}
