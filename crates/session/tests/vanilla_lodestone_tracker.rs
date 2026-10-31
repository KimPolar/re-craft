//! Compare saved lodestone compass tracker components with the Java codec.

use glam::IVec3;
use native_resources::packs::ResourceId;
use native_session::lodestone::Tracker;
use serde_json::Value;
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn tracker_components_match_java_codec() {
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
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let source = temp.path().join("LodestoneTrackerOracle.java");
    let output = temp.path().join("trackers.nbt");
    std::fs::write(&source, include_str!("java/LodestoneTrackerOracle.java")).unwrap();
    for command in [
        Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source),
        Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("LodestoneTrackerOracle")
            .arg(&output),
    ] {
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "oracle failed: {}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let java = native_persistence::nbt::read(&std::fs::read(output).unwrap())
        .unwrap()
        .value;
    assert_eq!(
        native_session::inventory::ordinary_item_default_max("minecraft:compass"),
        java.get("compass_max")
            .and_then(|v| v.as_java_int())
            .map(|v| v as u32)
    );
    let mut second = Tracker::linked(
        ResourceId::parse("minecraft:the_nether").unwrap(),
        IVec3::new(0, -64, 0),
    );
    second.tracked = false;
    let cases = [
        Tracker::linked(
            ResourceId::parse("minecraft:overworld").unwrap(),
            IVec3::new(-123, 64, 456),
        ),
        second,
        Tracker::linked(
            ResourceId::parse("example:deep/cave").unwrap(),
            IVec3::new(30000000, 319, -30000000),
        ),
        Tracker {
            target: None,
            tracked: true,
        },
        Tracker {
            target: None,
            tracked: false,
        },
    ];
    for (i, expected) in cases.iter().enumerate() {
        let tag = java.get(&format!("case{i}")).unwrap();
        assert_eq!(Tracker::parse(tag).as_ref(), Some(expected), "case {i}");
        assert_eq!(
            Tracker::parse(&expected.to_nbt()),
            Tracker::parse(tag),
            "case {i} round trip"
        );
        // NBT compounds are unordered; compare their nested values separately.
        let encoded = expected.to_nbt();
        assert_eq!(encoded.get("tracked"), tag.get("tracked"), "case {i}");
        for field in ["dimension", "pos"] {
            assert_eq!(
                encoded.get("target").and_then(|t| t.get(field)),
                tag.get("target").and_then(|t| t.get(field)),
                "case {i} {field}"
            );
        }
    }
}
