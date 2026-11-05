//! Compare ocean ruin floor adjustment with the installed Java 26.3 client.
use anyhow::{Context, Result, ensure};
use native_resources::ocean_ruin::{FloorWorld, Piece, Rotation, floor_position};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

struct Floor {
    initial_y: i32,
    case: i32,
}
impl FloorWorld for Floor {
    fn min_y(&self) -> i32 {
        -64
    }
    fn ocean_floor_height(&mut self, _: i32, _: i32) -> Result<i32> {
        Ok(self.initial_y)
    }
    fn floor_scan_passable(&mut self, [x, y, z]: [i32; 3]) -> Result<bool> {
        let bottom = self.initial_y - 1 - (x * 31 + z * 17 + self.case * 7).rem_euclid(13);
        if y <= bottom {
            return Ok(false);
        }
        Ok((x * 17 + z * 31 + y + self.case).rem_euclid(7) != 4)
    }
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn floor_adjustment_matches_java() -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir()?;
    let version = game.join("versions/26.3");
    let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    ensure!(manifest["id"] == "26.3", "wrong version");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"]
        .as_array()
        .context("libraries missing")?
    {
        if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let classpath = std::env::join_paths(paths)?;
    let source = temp.path().join("OceanRuinFloorOracle.java");
    std::fs::write(&source, include_str!("java/OceanRuinFloorOracle.java"))?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(&source)
        .output()?;
    ensure!(
        compiled.status.success(),
        "Java compile failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let result = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(classpath)
        .arg("OceanRuinFloorOracle")
        .output()?;
    ensure!(
        result.status.success(),
        "Java oracle failed: {}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let output = String::from_utf8(result.stdout)?;
    let rows: Vec<_> = output
        .lines()
        .filter(|line| line.starts_with("H\t"))
        .collect();
    ensure!(rows.len() == 512, "wrong oracle case count");
    let heights = [-63, -20, 1, 56, 64, 521, 600];
    let rotations = [
        Rotation::None,
        Rotation::Clockwise90,
        Rotation::Clockwise180,
        Rotation::Counterclockwise90,
    ];
    let mut adjusted = 0;
    for (i, row) in rows.iter().enumerate() {
        let fields: Vec<_> = row.split('\t').collect();
        assert_eq!(fields[1].parse::<usize>()?, i);
        let expected: i32 = fields[2].parse()?;
        let initial_y = heights[i % heights.len()];
        let piece = Piece {
            template: "minecraft:test".into(),
            position: [-123 + i as i32 * 7, 90, 211 - i as i32 * 11],
            rotation: rotations[i % 4],
            integrity: 1.0,
            large: false,
        };
        let mut world = Floor {
            initial_y,
            case: i as i32,
        };
        let result = floor_position(
            &piece,
            [(i % 9 + 1) as i32, 1, ((i / 9) % 9 + 1) as i32],
            &mut world,
        )?;
        assert_eq!(
            result,
            [piece.position[0], expected, piece.position[2]],
            "case {i}"
        );
        adjusted += usize::from(result[1] != initial_y);
    }
    assert!(adjusted > 0);
    println!("verified 512 floor adjustments, including {adjusted} lowered origins");
    Ok(())
}
