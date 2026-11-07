use native_core::random::LegacyRandom;
use native_resources::{shipwreck, structure_transform::Rotation};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn shipwreck_selection_and_bounds_match_java() -> anyhow::Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir()?;
    let version = game.join("versions/26.3");
    let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    anyhow::ensure!(manifest["id"] == "26.3", "incorrect game version");
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
    let source = temp.path().join("ShipwreckSelectionOracle.java");
    std::fs::write(&source, include_str!("java/ShipwreckSelectionOracle.java"))?;
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
        .arg("ShipwreckSelectionOracle")
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "Java failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout)?;
    let mut count = 0;
    for line in text.lines().filter(|line| line.starts_with("C ")) {
        let row: Vec<_> = line.split_whitespace().collect();
        let mode: i32 = row[1].parse()?;
        let seed: i32 = row[2].parse()?;
        let beached: bool = row[3].parse()?;
        let chunk = [seed * 19 - 4000, 3000 - seed * 13];
        let mut random = LegacyRandom::new(i64::from(seed));
        if mode == 1 {
            let a = random.next_long();
            let b = random.next_long();
            random = LegacyRandom::new(
                i64::from(chunk[0]).wrapping_mul(a)
                    ^ i64::from(chunk[1]).wrapping_mul(b)
                    ^ i64::from(seed),
            );
        }
        let origin = if mode == 1 {
            [chunk[0] * 16, 90, chunk[1] * 16]
        } else {
            [chunk[0], 90, chunk[1]]
        };
        let piece = shipwreck::select_piece(&mut random, origin, beached);
        if mode == 1 {
            assert_eq!(
                piece,
                shipwreck::select_for_chunk(i64::from(seed), chunk, beached)?,
                "{line}"
            );
        }
        assert_eq!(piece.template, row[4], "{line}");
        assert_eq!(
            piece.rotation,
            match row[5] {
                "0" => Rotation::None,
                "1" => Rotation::Clockwise90,
                "2" => Rotation::Clockwise180,
                "3" => Rotation::Counterclockwise90,
                _ => anyhow::bail!("invalid rotation"),
            },
            "{line}"
        );
        let size = [1 + seed % 41, 1 + (seed * 7) % 41, 1 + (seed * 13) % 41];
        let bounds = piece.bounds(size)?;
        assert_eq!(
            [
                bounds.min[0],
                bounds.min[1],
                bounds.min[2],
                bounds.max[0],
                bounds.max[1],
                bounds.max[2]
            ],
            [
                row[6].parse::<i32>()?,
                row[7].parse::<i32>()?,
                row[8].parse::<i32>()?,
                row[9].parse::<i32>()?,
                row[10].parse::<i32>()?,
                row[11].parse::<i32>()?
            ],
            "{line}"
        );
        assert_eq!(
            shipwreck::too_big_for_region(size),
            row[12].parse::<bool>()?,
            "{line}"
        );
        let y = if beached {
            shipwreck::beached_height(-20, size[1], random.next_int_bound(3).unwrap())?
        } else {
            0
        };
        assert_eq!(y, row[13].parse::<i32>()?, "{line}");
        assert_eq!(random.next_int(), row[14].parse::<i32>()?, "{line}");
        count += 1;
    }
    assert_eq!(count, 2 * 512 * 2);
    Ok(())
}
