//! Fluid registry identity for all built-in Java 26.3 block states.

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn fluid_identity_matches_every_vanilla_block_state() {
    use native_world::{
        fluid::{FluidKind, FluidState},
        loaded::BlockState,
    };
    use std::{path::PathBuf, process::Command};
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
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
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let source = temp.path().join("FluidStateOracle.java");
    let output = temp.path().join("states.json");
    std::fs::write(&source, include_str!("java/FluidStateOracle.java")).unwrap();
    for command in [
        Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source),
        Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("FluidStateOracle")
            .arg(&output),
    ] {
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "Java fluid state oracle failed:\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let rows: Vec<serde_json::Value> =
        serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    let mut failures = Vec::new();
    for row in &rows {
        let state = BlockState {
            name: row["name"].as_str().unwrap().to_owned(),
            properties: serde_json::from_value(row["properties"].clone()).unwrap(),
        };
        let actual = match FluidState::from_block(&state).unwrap() {
            Some(FluidState {
                kind: FluidKind::Water,
                source: true,
                ..
            }) => "minecraft:water",
            Some(FluidState {
                kind: FluidKind::Water,
                source: false,
                ..
            }) => "minecraft:flowing_water",
            Some(FluidState {
                kind: FluidKind::Lava,
                source: true,
                ..
            }) => "minecraft:lava",
            Some(FluidState {
                kind: FluidKind::Lava,
                source: false,
                ..
            }) => "minecraft:flowing_lava",
            None => "minecraft:empty",
        };
        if actual != row["fluid"].as_str().unwrap() {
            failures.push(format!(
                "{state:?}: expected {}, got {actual}",
                row["fluid"]
            ));
        }
    }
    assert!(rows.len() > 10000);
    assert!(
        failures.is_empty(),
        "{} mismatches; first samples:\n{}",
        failures.len(),
        failures
            .iter()
            .take(30)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
