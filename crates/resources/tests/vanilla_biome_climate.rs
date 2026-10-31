//! Differential checks against the installed Java 26.3 biome implementation.
use native_resources::{
    biome_climate::{BiomeClimate, Precipitation, TemperatureModifier},
    default_minecraft_dir,
};
use serde_json::Value;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

fn run(command: &mut Command) -> String {
    let output = command.output().expect("cannot run Java 25");
    assert!(
        output.status.success(),
        "Java climate oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn classpath(game: &Path, classes: &Path) -> OsString {
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
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

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn temperature_and_precipitation_match_vanilla() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("BiomePrecipitationOracle.java");
    std::fs::write(&source, include_str!("java/BiomePrecipitationOracle.java")).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("BiomePrecipitationOracle"));
    let mut count = 0;
    for line in output.lines() {
        let values: Vec<_> = line.split_whitespace().collect();
        assert_eq!(values.len(), 10, "unexpected output: {line}");
        let x: i32 = values[0].parse().unwrap();
        let y: i32 = values[1].parse().unwrap();
        let z: i32 = values[2].parse().unwrap();
        let sea: i32 = values[3].parse().unwrap();
        let climate = BiomeClimate {
            has_precipitation: values[5].parse().unwrap(),
            temperature: f32::from_bits(values[6].parse::<i32>().unwrap() as u32),
            modifier: if values[4] == "true" {
                TemperatureModifier::Frozen
            } else {
                TemperatureModifier::None
            },
        };
        let temperature = climate.temperature_at(x, y, z, sea);
        assert_eq!(
            temperature.to_bits(),
            values[7].parse::<i32>().unwrap() as u32,
            "temperature: {line}"
        );
        let precipitation = match values[8] {
            "NONE" => Precipitation::None,
            "RAIN" => Precipitation::Rain,
            "SNOW" => Precipitation::Snow,
            other => panic!("unknown precipitation {other}"),
        };
        assert_eq!(
            climate.precipitation_at(x, y, z, sea),
            precipitation,
            "precipitation: {line}"
        );
        assert_eq!(
            climate.cold_enough_to_snow(x, y, z, sea),
            values[9].parse::<bool>().unwrap(),
            "snow threshold: {line}"
        );
        count += 1;
    }
    assert_eq!(count, 12000);
}
