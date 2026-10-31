//! Differential test against ServerLevel's 26.3 weather tick.

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn weather_timer_and_intensities_match_java() {
    use native_core::random::LegacyRandom;
    use native_persistence::world::SavedWeather;
    use native_session::weather::Weather;
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
    let source = temp.path().join("WeatherOracle.java");
    let output = temp.path().join("weather.json");
    let saved_data = temp.path().join("weather.dat");
    std::fs::write(&source, include_str!("java/WeatherOracle.java")).unwrap();
    for command in [
        Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source),
        Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("WeatherOracle")
            .arg(&output)
            .arg(&saved_data),
    ] {
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "Java weather oracle failed:\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let cases: Vec<serde_json::Value> =
        serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    let saved = native_persistence::nbt::read_gzip(std::fs::File::open(saved_data).unwrap()).unwrap();
    assert_eq!(native_persistence::world::parse_weather_data(&saved).unwrap(), SavedWeather { clear_weather_time: 37, rain_time: 239, thunder_time: 919, raining: true, thundering: false });
    assert_eq!(cases.len(), 240);
    for (case, row) in cases.iter().enumerate() {
        let data = SavedWeather {
            clear_weather_time: row["clear"].as_i64().unwrap() as i32,
            rain_time: row["rain"].as_i64().unwrap() as i32,
            thunder_time: row["thunder"].as_i64().unwrap() as i32,
            raining: row["raining"].as_bool().unwrap(),
            thundering: row["thundering"].as_bool().unwrap(),
        };
        let can = row["can"].as_bool().unwrap();
        let advance = row["advance"].as_bool().unwrap();
        let mut random = LegacyRandom::new(row["seed"].as_i64().unwrap());
        let mut weather = Weather::new(data);
        let trace = row["trace"].as_array().unwrap();
        assert_eq!(trace.len(), 110);
        for (tick, expected) in trace.iter().enumerate() {
            weather.tick(can, advance, &mut random);
            let expected = expected.as_array().unwrap();
            assert_eq!(
                weather.data.clear_weather_time,
                expected[0].as_i64().unwrap() as i32,
                "case {case} tick {tick}"
            );
            assert_eq!(
                weather.data.rain_time,
                expected[1].as_i64().unwrap() as i32,
                "case {case} tick {tick}"
            );
            assert_eq!(
                weather.data.thunder_time,
                expected[2].as_i64().unwrap() as i32,
                "case {case} tick {tick}"
            );
            assert_eq!(
                weather.data.raining,
                expected[3].as_bool().unwrap(),
                "case {case} tick {tick}"
            );
            assert_eq!(
                weather.data.thundering,
                expected[4].as_bool().unwrap(),
                "case {case} tick {tick}"
            );
            assert_eq!(
                weather.rain_level.to_bits(),
                expected[5].as_i64().unwrap() as i32 as u32,
                "case {case} tick {tick}"
            );
            assert_eq!(
                weather.thunder_level.to_bits(),
                expected[6].as_i64().unwrap() as i32 as u32,
                "case {case} tick {tick}"
            );
            assert_eq!(
                weather.is_raining(can),
                expected[7].as_bool().unwrap(),
                "case {case} tick {tick}"
            );
            assert_eq!(
                weather.is_thundering(can),
                expected[8].as_bool().unwrap(),
                "case {case} tick {tick}"
            );
        }
        assert_eq!(
            random.next_int(),
            row["next"].as_i64().unwrap() as i32,
            "random case {case}"
        );
    }
}
