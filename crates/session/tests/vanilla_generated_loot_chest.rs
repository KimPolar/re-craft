//! Validate deferred structure-chest NBT against the installed Java 26.3 client.
use native_persistence::nbt::{self, NamedTag, Tag};
use native_resources::structure_start::HorizontalDirection;
use native_session::worldgen::GeneratedLootChest;
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf, process::Command};
fn run(c: &mut Command) -> String {
    let o = c.output().unwrap();
    assert!(
        o.status.success(),
        "Java loot chest oracle failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8(o.stdout).unwrap()
}
fn fields(tag: &Tag) -> BTreeMap<Vec<u16>, Tag> {
    let Tag::Compound(fields) = tag else {
        panic!("expected compound")
    };
    fields
        .iter()
        .map(|(k, v)| (k.0.clone(), v.clone()))
        .collect()
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn generated_deferred_chests_roundtrip_java_nbt() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"].as_array().unwrap() {
        if let Some(p) = lib["downloads"]["artifact"]["path"].as_str() {
            let p = game.join("libraries").join(p);
            if p.is_file() {
                paths.push(p)
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("GeneratedLootChestOracle.java");
    std::fs::write(&source, include_str!("java/GeneratedLootChestOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    assert!(
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("GeneratedLootChestOracle")
            .arg("java")
            .arg(temp.path()))
        .contains("OK")
    );
    for (i, seed) in [0, 1, -1, i64::MIN, i64::MAX].into_iter().enumerate() {
        let pos = [-17 + i as i32, 63 - i as i32, 31 + i as i32];
        let table = if i == 4 {
            "example:custom_table"
        } else {
            "minecraft:chests/buried_treasure"
        };
        let chest = GeneratedLootChest::new(pos, HorizontalDirection::North, table, seed).unwrap();
        let expected =
            nbt::read(&std::fs::read(temp.path().join(format!("java-{i}.nbt"))).unwrap()).unwrap();
        assert_eq!(
            fields(&chest.block_entity()),
            fields(&expected.value),
            "seed={seed}"
        );
        let root = NamedTag {
            name: "".into(),
            value: chest.block_entity(),
        };
        std::fs::write(
            temp.path().join(format!("rust-{i}.nbt")),
            nbt::write(&root).unwrap(),
        )
        .unwrap();
    }
    assert!(
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("GeneratedLootChestOracle")
            .arg("rust")
            .arg(temp.path()))
        .contains("OK")
    );
}
