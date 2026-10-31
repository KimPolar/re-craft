//! Compare cached aquifer centers with the installed 26.3 implementation.
use native_core::aquifer::AquiferLocator;
use serde_json::Value;
use std::{path::PathBuf, process::Command};

fn run(command: &mut Command) -> String {
    let output = command.output().expect("cannot run Java 25");
    assert!(
        output.status.success(),
        "aquifer oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn centers_match_vanilla_aquifer_cache() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("AquiferCentersOracle.java");
    std::fs::write(&source, include_str!("java/AquiferCentersOracle.java")).unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("AquiferCentersOracle"));
    let mut locator = None;
    let mut groups = 0;
    let mut points = 0;
    let mut ties = 0;
    for line in output.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        match fields.first().copied() {
            Some("S") => {
                let mode: i32 = fields[1].parse().unwrap();
                let seed: i64 = fields[2].parse().unwrap();
                locator = Some(AquiferLocator::new(seed, mode == 0));
                groups += 1;
            }
            Some("P") => {
                let value = |i: usize| fields[i].parse::<i32>().unwrap();
                let (x, y, z) = (value(1), value(2), value(3));
                let base = [(x - 5) >> 4, (y + 1).div_euclid(12), (z - 5) >> 4];
                let locator = locator.as_mut().unwrap();
                let mut candidates = Vec::new();
                let mut i = 4;
                for dx in 0..=1 {
                    for dy in -1..=1 {
                        for dz in 0..=1 {
                            let grid = [base[0] + dx, base[1] + dy, base[2] + dz];
                            let expected = [value(i), value(i + 1), value(i + 2)];
                            assert_eq!(locator.center(grid), expected, "center at {grid:?}");
                            candidates.push(expected);
                            i += 3;
                        }
                    }
                }
                let nearest = locator.nearest(x, y, z);
                for j in 0..4 {
                    assert_eq!(
                        nearest[j].position,
                        candidates[value(i + j) as usize],
                        "nearest at {x},{y},{z}"
                    );
                }
                if nearest
                    .windows(2)
                    .any(|pair| pair[0].distance_squared == pair[1].distance_squared)
                {
                    ties += 1;
                }
                points += 1;
            }
            _ => {}
        }
    }
    assert_eq!(groups, 10);
    assert_eq!(
        points,
        10 * 3 * 64,
        "oracle output: {}",
        output.chars().take(3000).collect::<String>()
    );
    assert!(ties > 0, "fixtures must exercise equal-distance ordering");
}
