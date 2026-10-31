//! Compare farmland random-tick decisions with the installed Java client.

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn farmland_random_ticks_match_java() {
    use native_session::farmland::{RandomTick, random_tick};
    use std::{path::PathBuf, process::Command};

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
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let source = temp.path().join("FarmlandRandomOracle.java");
    let output = temp.path().join("farmland.json");
    std::fs::write(&source, include_str!("java/FarmlandRandomOracle.java")).unwrap();
    for command in [
        Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source),
        Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("FarmlandRandomOracle")
            .arg(&output),
    ] {
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "Java farmland oracle failed:\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let rows: Vec<Vec<serde_json::Value>> =
        serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(rows.len(), 5376);
    for row in rows {
        let moisture = row[0].as_u64().unwrap() as u8;
        let x = row[1].as_i64().unwrap();
        let y = row[2].as_i64().unwrap();
        let z = row[3].as_i64().unwrap();
        let fluid = row[4].as_i64().unwrap();
        let in_range = (-4..=4).contains(&x) && (0..=1).contains(&y) && (-4..=4).contains(&z);
        let tag = row[9].as_i64().unwrap();
        let water = tag == 3
            || in_range
                && match tag {
                    0 => fluid != 3,
                    1 => fluid == 0 || fluid == 2,
                    2 => fluid == 1,
                    4 => fluid == 3,
                    _ => false,
                };
        let rain = row[5].as_bool().unwrap();
        let maintain = row[6].as_bool().unwrap();
        let action = random_tick(moisture, Some(water), Some(rain), Some(maintain)).unwrap();
        let actual = match action {
            RandomTick::Unchanged => i64::from(moisture),
            RandomTick::Moisture(value) => i64::from(value),
            RandomTick::Dirt => -1,
        };
        assert_eq!(actual, row[7].as_i64().unwrap(), "{row:?}");
        assert_eq!(
            row[8].as_i64().unwrap(),
            if matches!(action, RandomTick::Moisture(_)) {
                2
            } else {
                -1
            },
            "{row:?}"
        );
    }
}
