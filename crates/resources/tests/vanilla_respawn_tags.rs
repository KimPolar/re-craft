//! Compare installed block identities and respawn tags with Java's tag loader.

use native_resources::{
    datapacks::DataPackSettings, packs::ResourceStack, respawn_tags::RespawnBlockTags,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

fn classpath(game: &Path, classes: &Path) -> OsString {
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    let mut paths = vec![version.join("26.3.jar"), classes.to_path_buf()];
    for library in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    std::env::join_paths(paths).unwrap()
}

fn run(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "Java respawn tag oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn block_registry_and_respawn_tags_match_java_with_data_pack_overrides() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let world = temp.path().join("world");
    let pack = world.join("datapacks/respawn");
    let tags_dir = pack.join("data/minecraft/tags/block");
    std::fs::create_dir_all(&tags_dir).unwrap();
    std::fs::write(
        pack.join("pack.mcmeta"),
        br#"{"pack":{"description":"respawn test","min_format":121,"max_format":121}}"#,
    )
    .unwrap();
    for (name, data) in [
        ("climbable", br##"{"replace":true,"values":["minecraft:stone","#minecraft:planks",{"id":"minecraft:absent_for_test","required":false}]}"##.as_slice()),
        ("invalid_spawn_inside", br#"{"replace":true,"values":["minecraft:air","minecraft:stone"]}"#.as_slice()),
        ("default_immune_to", br#"{"replace":true,"values":["minecraft:lava","minecraft:cactus","minecraft:soul_campfire"]}"#.as_slice()),
        ("fire", br#"{"replace":true,"values":["minecraft:stone","minecraft:magma_block","minecraft:campfire"]}"#.as_slice()),
    ] { std::fs::write(tags_dir.join(format!("{name}.json")), data).unwrap(); }
    let source = temp.path().join("RespawnTagsOracle.java");
    let output = temp.path().join("result.json");
    std::fs::write(&source, include_str!("java/RespawnTagsOracle.java")).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java"))
        .arg("-cp")
        .arg(&cp)
        .arg("RespawnTagsOracle")
        .arg(world.join("datapacks"))
        .arg(&output));
    let scenarios: Vec<Vec<Value>> =
        serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(scenarios.len(), 2);
    for (index, rows) in scenarios.iter().enumerate() {
        let mut resources = ResourceStack::open(&game).unwrap();
        let registry = resources.vanilla_block_ids().unwrap();
        let expected_ids: BTreeSet<_> = rows
            .iter()
            .map(|row| row["id"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(registry, expected_ids, "installed 26.3 block registry");
        let settings = DataPackSettings {
            enabled: if index == 0 {
                vec!["vanilla".into()]
            } else {
                vec!["vanilla".into(), "file/respawn".into()]
            },
            disabled: if index == 0 {
                vec!["file/respawn".into()]
            } else {
                Vec::new()
            },
            enabled_features: vec!["minecraft:vanilla".into()],
        };
        assert!(
            resources
                .configure_world_data(&world, &settings)
                .unwrap()
                .is_empty()
        );
        let tags = RespawnBlockTags::load(&mut resources, registry).unwrap();
        for row in rows {
            let id = row["id"].as_str().unwrap();
            let mut properties = BTreeMap::new();
            if let Some(lit) = row["lit"].as_str() {
                properties.insert("lit".into(), lit.into());
            }
            let facts = tags.for_block(id, &properties).unwrap();
            assert_eq!(
                facts.dangerous,
                row["dangerous"].as_bool().unwrap(),
                "{id} {properties:?} scenario {index}"
            );
            assert_eq!(
                facts.climbable,
                row["climbable"].as_bool().unwrap(),
                "{id} scenario {index}"
            );
            assert_eq!(
                facts.invalid_inside,
                row["invalid"].as_bool().unwrap(),
                "{id} scenario {index}"
            );
        }
    }
}
