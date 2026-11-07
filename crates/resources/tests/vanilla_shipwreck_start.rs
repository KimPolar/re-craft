use anyhow::{Context, Result, ensure};
use native_core::random::LegacyRandom;
use native_resources::{
    shipwreck,
    structure_start::{BiomeQuery, Heightmap, StartEnvironment},
    structure_transform::Rotation,
};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

struct Environment {
    calls: i32,
    hash: i32,
}
impl StartEnvironment for Environment {
    fn vertical_bounds(&self) -> (i32, i32) {
        (-64, 319)
    }
    fn sea_level(&self) -> i32 {
        63
    }
    fn first_occupied_height(&mut self, x: i32, z: i32, map: Heightmap) -> Result<i32> {
        self.calls += 1;
        self.hash = self
            .hash
            .wrapping_mul(31)
            .wrapping_add(x)
            .wrapping_mul(31)
            .wrapping_add(z)
            .wrapping_mul(31)
            .wrapping_add(if map == Heightmap::WorldSurface { 1 } else { 2 });
        Ok(-80 + (x * 13 + z * 7).rem_euclid(201)
            - if map == Heightmap::OceanFloor { 10 } else { 0 })
    }
    fn biome_allowed(&mut self, _: [i32; 3], _: BiomeQuery) -> Result<bool> {
        Ok(true)
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn shipwreck_start_alignment_matches_java() -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir()?;
    let version = game.join("versions/26.3");
    let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    ensure!(manifest["id"] == "26.3", "wrong version");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"]
        .as_array()
        .context("missing libraries")?
    {
        if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths)?;
    let source = temp.path().join("ShipwreckStartOracle.java");
    std::fs::write(&source, include_str!("java/ShipwreckStartOracle.java"))?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source)
        .output()?;
    ensure!(
        compiled.status.success(),
        "Java compilation: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(cp)
        .arg("ShipwreckStartOracle")
        .output()?;
    ensure!(
        output.status.success(),
        "Java oracle: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout)?;
    let mut count = 0;
    for line in text.lines().filter(|l| l.starts_with("S ")) {
        let row: Vec<_> = line.split_whitespace().collect();
        let i: i32 = row[1].parse()?;
        let beached: bool = row[2].parse()?;
        let chunk = [i * 17 - 251, 193 - i * 13];
        let seed = i64::from(i) * 987654321 - 123456789;
        let mut env = Environment { calls: 0, hash: 0 };
        native_resources::structure_start::center_generation_point(
            &mut env,
            chunk,
            if beached {
                Heightmap::WorldSurface
            } else {
                Heightmap::OceanFloor
            },
        )?
        .unwrap();
        let mut random = LegacyRandom::new(seed);
        let a = random.next_long();
        let b = random.next_long();
        random = LegacyRandom::new(
            i64::from(chunk[0]).wrapping_mul(a) ^ i64::from(chunk[1]).wrapping_mul(b) ^ seed,
        );
        let mut piece =
            shipwreck::select_piece(&mut random, [chunk[0] * 16, 90, chunk[1] * 16], beached);
        let (bounds, adjusted) = shipwreck::align_during_start(
            &mut piece,
            [1 + i % 40, 1 + (i * 7) % 40, 1 + (i * 11) % 40],
            &mut env,
            &mut random,
        )?;
        assert_eq!(piece.template, row[3], "{line}");
        assert_eq!(
            piece.rotation,
            [
                Rotation::None,
                Rotation::Clockwise90,
                Rotation::Clockwise180,
                Rotation::Counterclockwise90
            ][row[4].parse::<usize>()?],
            "{line}"
        );
        assert_eq!(piece.position[1], row[5].parse::<i32>()?, "{line}");
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
        assert_eq!(adjusted, row[12].parse::<bool>()?, "{line}");
        assert_eq!(env.calls, row[13].parse::<i32>()?, "{line}");
        assert_eq!(env.hash, row[14].parse::<i32>()?, "{line}");
        assert_eq!(random.next_int(), row[15].parse::<i32>()?, "{line}");
        count += 1;
    }
    assert_eq!(count, 512);
    Ok(())
}
