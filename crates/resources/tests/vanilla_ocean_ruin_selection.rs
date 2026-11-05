use native_core::random::LegacyRandom;
use native_resources::ocean_ruin::{
    Rotation, Settings, Temperature, select_for_chunk, select_pieces,
};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn ocean_ruin_piece_selection_matches_java() -> anyhow::Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir()?;
    let version = game.join("versions/26.3");
    let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for library in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let classpath = std::env::join_paths(paths)?;
    let source = temp.path().join("OceanRuinSelectionOracle.java");
    std::fs::write(&source, include_str!("java/OceanRuinSelectionOracle.java"))?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(&source)
        .output()?;
    anyhow::ensure!(
        compiled.status.success(),
        "Java compile failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(classpath)
        .arg("OceanRuinSelectionOracle")
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "Java oracle failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout)?;
    let mut lines = text
        .lines()
        .filter(|line| line.starts_with("C ") || line.starts_with("P "));
    let mut count = 0;
    while let Some(header) = lines.next() {
        let row: Vec<_> = header.split_whitespace().collect();
        assert_eq!(row[0], "C", "{header}");
        let mode: i32 = row[2].parse()?;
        let seed: i32 = row[3].parse()?;
        let temperature = match row[4] {
            "WARM" => Temperature::Warm,
            "COLD" => Temperature::Cold,
            _ => anyhow::bail!("unexpected type"),
        };
        let mut random = LegacyRandom::new(i64::from(seed));
        let chunk = [seed * 19 - 1100, 701 - seed * 13];
        if mode == 1 {
            let a = random.next_long();
            let b = random.next_long();
            random = LegacyRandom::new(
                i64::from(chunk[0]).wrapping_mul(a)
                    ^ i64::from(chunk[1]).wrapping_mul(b)
                    ^ i64::from(seed),
            );
        }
        let settings = Settings {
            temperature,
            large_probability: row[5].parse()?,
            cluster_probability: row[6].parse()?,
        };
        let origin = if mode == 1 {
            [chunk[0] * 16, 90, chunk[1] * 16]
        } else {
            [chunk[0], 90, chunk[1]]
        };
        let pieces = select_pieces(
            &mut random,
            origin,
            temperature,
            settings.large_probability,
            settings.cluster_probability,
        )?;
        if mode == 1 {
            assert_eq!(
                pieces,
                select_for_chunk(i64::from(seed), chunk, settings)?,
                "{header}"
            );
        }
        assert_eq!(pieces.len(), row[7].parse::<usize>()?, "{header}");
        assert_eq!(
            random.next_int(),
            row[8].parse::<i32>()?,
            "random state: {header}"
        );
        for piece in pieces {
            let line = lines.next().expect("missing Java piece");
            let row: Vec<_> = line.split_whitespace().collect();
            assert_eq!(row[0], "P", "{line}");
            assert_eq!(piece.template, row[1], "{header}");
            assert_eq!(
                piece.position,
                [
                    row[2].parse::<i32>()?,
                    row[3].parse::<i32>()?,
                    row[4].parse::<i32>()?
                ],
                "{header}"
            );
            let rotation = match row[5] {
                "0" => Rotation::None,
                "1" => Rotation::Clockwise90,
                "2" => Rotation::Clockwise180,
                "3" => Rotation::Counterclockwise90,
                _ => anyhow::bail!("invalid rotation"),
            };
            assert_eq!(piece.rotation, rotation, "{header}");
            assert_eq!(
                piece.integrity.to_bits() as i32,
                row[6].parse::<i32>()?,
                "{header}"
            );
            assert_eq!(piece.large, row[7].parse::<bool>()?, "{header}");
        }
        count += 1;
    }
    assert_eq!(count, 2 * 128 * 2 * 3 * 3);
    Ok(())
}
