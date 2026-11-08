//! Compare crop growth and random consumption against the installed client.

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn crop_growth_matches_java() {
    use native_core::random::LegacyRandom;
    use native_session::crop::{Crop, Soil, growth_speed, random_tick};
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
    let java = std::env::var_os("MINECRAFT_JAVA_HOME")
        .or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let source = temp.path().join("CropRandomOracle.java");
    let output = temp.path().join("crop.json");
    std::fs::write(&source, include_str!("java/CropRandomOracle.java")).unwrap();
    for command in [
        Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source),
        Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("CropRandomOracle")
            .arg(&output),
    ] {
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "Java crop oracle failed:\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let rows: Vec<Vec<i64>> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(rows.len(), 8400);
    let soils = [
        [0, 0, 0, 0, 0, 0, 0, 0, 0],
        [1, 1, 1, 1, 1, 1, 1, 1, 1],
        [2, 2, 2, 2, 2, 2, 2, 2, 2],
        [0, 1, 2, 1, 2, 0, 2, 0, 1],
        [3, 0, 1, 0, 3, 2, 1, 2, 3],
    ];
    for row in rows {
        let crop = [Crop::Wheat, Crop::Carrots, Crop::Potatoes, Crop::Beetroots][row[0] as usize];
        let soil = soils[row[1] as usize].map(|value| match value {
            0 => Soil::Other,
            2 => Soil::Wet,
            _ => Soil::Dry,
        });
        let neighbors = std::array::from_fn(|i| (row[2] & (1 << i)) != 0);
        assert_eq!(
            growth_speed(soil, neighbors).to_bits(),
            row[6] as u32,
            "{row:?}"
        );
        let mut random = LegacyRandom::new(row[5]);
        let age = random_tick(
            crop,
            row[4] as u8,
            row[3] as u8,
            soil,
            neighbors,
            &mut random,
        );
        assert_eq!(i64::from(age), row[7], "{row:?}");
        assert_eq!(i64::from(random.next_int()), row[8], "random after {row:?}");
    }
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn crop_survival_and_bonemeal_match_java() {
    use native_core::random::LegacyRandom;
    use native_session::crop::{Crop, bonemeal, can_survive};
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
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let source = temp.path().join("CropUseOracle.java");
    let output = temp.path().join("crop-use.json");
    std::fs::write(&source, include_str!("java/CropUseOracle.java")).unwrap();
    for command in [
        Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source),
        Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("CropUseOracle")
            .arg(&output),
    ] {
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "Java crop use oracle failed:\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let output: serde_json::Value =
        serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    let survival = output["survival"].as_array().unwrap();
    assert_eq!(survival.len(), 40);
    for row in survival {
        assert_eq!(
            can_survive(
                Some(row[2].as_u64().unwrap() as u8),
                Some(row[1].as_bool().unwrap())
            ),
            row[3].as_bool(),
            "{row}"
        );
    }
    let shapes = output["shape"].as_array().unwrap();
    assert_eq!(shapes.len(), 240);
    for row in shapes {
        let survives = can_survive(
            Some(row[2].as_u64().unwrap() as u8),
            Some(row[1].as_bool().unwrap()),
        )
        .unwrap();
        assert_eq!(!survives, row[4].as_bool().unwrap(), "{row}");
    }
    let mining = output["mining"].as_array().unwrap();
    assert_eq!(mining.len(), 4);
    for row in mining {
        assert_eq!(
            f64::from(native_session::mining::Material::Crop.hardness()),
            row[1].as_f64().unwrap(),
            "{row}"
        );
        assert_eq!(
            native_session::mining::Material::Crop.needs_tool(),
            row[2].as_bool().unwrap(),
            "{row}"
        );
    }
    let rows = output["bonemeal"].as_array().unwrap();
    assert_eq!(rows.len(), 168);
    for row in rows {
        let crop = [Crop::Wheat, Crop::Carrots, Crop::Potatoes, Crop::Beetroots]
            [row[0].as_u64().unwrap() as usize];
        let age = row[1].as_u64().unwrap() as u8;
        let mut random = LegacyRandom::new(row[2].as_i64().unwrap());
        let next = bonemeal(crop, age, &mut random);
        assert_eq!(next.is_some(), row[3].as_bool().unwrap(), "{row}");
        assert_eq!(
            u64::from(next.unwrap_or(age)),
            row[4].as_u64().unwrap(),
            "{row}"
        );
        assert_eq!(
            i64::from(random.next_int()),
            row[5].as_i64().unwrap(),
            "{row}"
        );
    }
    assert_eq!(can_survive(Some(7), None), Some(false));
    assert_eq!(can_survive(Some(8), None), None);
    assert_eq!(can_survive(None, Some(false)), Some(false));
}
