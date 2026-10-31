//! Compare compass tracker ticks with the actual Java 26.3 tracker.
use glam::IVec3;
use native_resources::packs::ResourceId;
use native_session::lodestone::Tracker;
use serde_json::Value;
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn lodestone_tracker_ticks_match_java() {
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
    let source = temp.path().join("LodestoneTickOracle.java");
    let output = temp.path().join("ticks.json");
    std::fs::write(&source, include_str!("java/LodestoneTickOracle.java")).unwrap();
    for command in [
        Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source),
        Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("LodestoneTickOracle")
            .arg(&output),
    ] {
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "oracle failed:\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(rows.len(), 32);
    let overworld = ResourceId::parse("minecraft:overworld").unwrap();
    let nether = ResourceId::parse("minecraft:the_nether").unwrap();
    for row in rows {
        let b = |name: &str| row[name].as_bool().unwrap();
        let mut tracker = Tracker::linked(overworld.clone(), IVec3::new(-9, 64, 7));
        tracker.tracked = b("tracked");
        if !b("target") {
            tracker.target = None;
        }
        let dimension = if b("same") { &overworld } else { &nether };
        let changed = tracker.update_target(dimension, b("bounds"), Some(b("exists")));
        assert_eq!(changed, b("changed"), "{row}");
        assert_eq!(tracker.target.is_some(), b("after_target"), "{row}");
        assert_eq!(tracker.tracked, b("after_tracked"), "{row}");
        assert_eq!(
            row["calls"].as_u64().unwrap(),
            u64::from(b("tracked") && b("target") && b("same") && b("bounds")),
            "{row}"
        );
    }
}
