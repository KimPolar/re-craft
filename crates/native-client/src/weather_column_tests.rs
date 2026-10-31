//! Compare weather-column animation to the installed Java client.
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn precipitation_columns_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: serde_json::Value =
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
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let source = temp.path().join("WeatherColumnOracle.java");
    let output = temp.path().join("columns.txt");
    std::fs::write(
        &source,
        include_str!("../tests/java/WeatherColumnOracle.java"),
    )
    .unwrap();
    for command in [
        Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source),
        Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("WeatherColumnOracle")
            .arg(&output),
    ] {
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "oracle failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let text = std::fs::read_to_string(output).unwrap();
    let mut count = 0;
    for line in text.lines() {
        let values: Vec<i64> = line
            .split_whitespace()
            .map(|value| value.parse().unwrap())
            .collect();
        assert_eq!(values.len(), 10);
        let x = values[0] as i32;
        let z = values[1] as i32;
        let partial = f32::from_bits(values[3] as i32 as u32);
        let light = [values[4] as u8, values[5] as u8];
        let actual = if values[6] == 0 {
            native_core::weather_columns::rain(x, z, values[2], partial, light)
        } else {
            native_core::weather_columns::snow(x, z, values[2], partial, light)
        };
        assert_eq!(actual.u.to_bits(), values[7] as i32 as u32, "u: {line}");
        assert_eq!(actual.v.to_bits(), values[8] as i32 as u32, "v: {line}");
        assert_eq!(
            i64::from((u32::from(actual.block_light) << 4) | (u32::from(actual.sky_light) << 20)),
            values[9],
            "light: {line}"
        );
        count += 1;
    }
    assert_eq!(count, 12000);
}
