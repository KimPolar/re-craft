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
    native_resources::packs::ResourceStack::open(&game).unwrap();
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
    let rust_data = temp.path().join("rust-weather.dat");
    let rust_level = temp.path().join("rust-level.dat");
    let command_output = temp.path().join("weather-commands.json");
    let shared_output = temp.path().join("weather-shared.json");
    let state = SavedWeather {
        clear_weather_time: 37,
        rain_time: 239,
        thunder_time: 919,
        raining: true,
        thundering: false,
    };
    let document =
        native_persistence::world::WorldReader::update_weather_document(None, state).unwrap();
    {
        use std::io::Write;
        let mut file = flate2::write::GzEncoder::new(
            std::fs::File::create(&rust_data).unwrap(),
            flate2::Compression::default(),
        );
        file.write_all(&native_persistence::nbt::write(&document).unwrap())
            .unwrap();
        file.finish().unwrap();
        let level = native_persistence::nbt::NamedTag {
            name: "".into(),
            value: native_persistence::nbt::Tag::Compound(vec![(
                "Data".into(),
                native_persistence::nbt::Tag::Compound(vec![(
                    "DataVersion".into(),
                    native_persistence::nbt::Tag::Int(5023),
                )]),
            )]),
        };
        let level =
            native_persistence::world::WorldReader::update_level_game_time(&level, i64::MIN + 10)
                .unwrap();
        let mut file = flate2::write::GzEncoder::new(
            std::fs::File::create(&rust_level).unwrap(),
            flate2::Compression::default(),
        );
        file.write_all(&native_persistence::nbt::write(&level).unwrap())
            .unwrap();
        file.finish().unwrap();
    }
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
            .arg(&saved_data)
            .arg(&rust_data)
            .arg(&rust_level)
            .arg(&command_output)
            .arg(&shared_output),
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
    let commands: Vec<serde_json::Value> =
        serde_json::from_slice(&std::fs::read(command_output).unwrap()).unwrap();
    let shared: Vec<serde_json::Value> =
        serde_json::from_slice(&std::fs::read(shared_output).unwrap()).unwrap();
    assert_eq!(shared.len(), 120);
    for case in &shared {
        let data = SavedWeather {
            clear_weather_time: case["clear"].as_i64().unwrap() as i32,
            rain_time: case["rain"].as_i64().unwrap() as i32,
            thunder_time: case["thunder"].as_i64().unwrap() as i32,
            raining: case["raining"].as_bool().unwrap(),
            thundering: case["thundering"].as_bool().unwrap(),
        };
        let current = case["current"].as_i64().unwrap();
        let advance = case["advance"].as_bool().unwrap();
        let mut weather = Weather::new(data);
        let mut randoms = [
            LegacyRandom::new(case["seedA"].as_i64().unwrap()),
            LegacyRandom::new(case["seedB"].as_i64().unwrap()),
            LegacyRandom::new(case["seedC"].as_i64().unwrap()),
        ];
        let level_count = case["levelCount"].as_u64().unwrap() as usize;
        for expected in case["trace"].as_array().unwrap() {
            for (index, random) in randoms.iter_mut().take(level_count).enumerate() {
                if current == index as i64 {
                    weather.tick(true, advance, random);
                } else if advance {
                    Weather::advance_data(&mut weather.data, random);
                }
            }
            let expected = expected.as_array().unwrap();
            assert_eq!(
                weather.data,
                SavedWeather {
                    clear_weather_time: expected[0].as_i64().unwrap() as i32,
                    rain_time: expected[1].as_i64().unwrap() as i32,
                    thunder_time: expected[2].as_i64().unwrap() as i32,
                    raining: expected[3].as_bool().unwrap(),
                    thundering: expected[4].as_bool().unwrap()
                },
                "{case}"
            );
            if current >= 0 {
                assert_eq!(
                    weather.rain_level.to_bits(),
                    expected[5].as_i64().unwrap() as i32 as u32,
                    "{case}"
                );
                assert_eq!(
                    weather.thunder_level.to_bits(),
                    expected[6].as_i64().unwrap() as i32 as u32,
                    "{case}"
                );
            }
        }
        assert_eq!(
            randoms[0].next_int(),
            case["nextA"].as_i64().unwrap() as i32,
            "{case}"
        );
        assert_eq!(
            randoms[1].next_int(),
            case["nextB"].as_i64().unwrap() as i32,
            "{case}"
        );
        assert_eq!(
            randoms[2].next_int(),
            case["nextC"].as_i64().unwrap() as i32,
            "{case}"
        );
    }
    assert_eq!(commands.len(), 72);
    for row in &commands {
        use native_session::weather_command::{WeatherCommand, WeatherKind, apply};
        let mode = row["mode"].as_str().unwrap();
        let kind = match mode {
            "clear" => WeatherKind::Clear,
            "rain" => WeatherKind::Rain,
            "thunder" => WeatherKind::Thunder,
            _ => unreachable!(),
        };
        let duration = row["duration"].as_i64().unwrap() as i32;
        let mut rng = LegacyRandom::new(row["seed"].as_i64().unwrap());
        let mut data = SavedWeather {
            clear_weather_time: 37,
            rain_time: 239,
            thunder_time: 919,
            raining: true,
            thundering: true,
        };
        assert!(apply(
            &mut data,
            WeatherCommand {
                kind,
                duration: (duration != -1).then_some(duration)
            },
            &mut rng
        ));
        assert_eq!(
            data,
            SavedWeather {
                clear_weather_time: row["clear"].as_i64().unwrap() as i32,
                rain_time: row["rain"].as_i64().unwrap() as i32,
                thunder_time: row["thunder"].as_i64().unwrap() as i32,
                raining: row["raining"].as_bool().unwrap(),
                thundering: row["thundering"].as_bool().unwrap()
            },
            "{row}"
        );
        assert_eq!(
            rng.next_int(),
            row["next"].as_i64().unwrap() as i32,
            "{row}"
        );
        assert_eq!(row["returned"].as_i64().unwrap(), i64::from(duration));
        assert_eq!(
            row["message"].as_str().unwrap(),
            format!("commands.weather.set.{mode}")
        );
    }
    let saved =
        native_persistence::nbt::read_gzip(std::fs::File::open(saved_data).unwrap()).unwrap();
    assert_eq!(
        native_persistence::world::parse_weather_data(&saved).unwrap(),
        SavedWeather {
            clear_weather_time: 37,
            rain_time: 239,
            thunder_time: 919,
            raining: true,
            thundering: false
        }
    );
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
