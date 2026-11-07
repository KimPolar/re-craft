use anyhow::{Context, Result, ensure};
use native_core::random::LegacyRandom;
use native_resources::{
    shipwreck::{self, HeightWorld, MarkerWorld, Piece},
    structure_start::Heightmap,
    structure_transform::Rotation,
};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

struct Heights {
    case: i32,
    beached: bool,
    calls: i32,
    hash: i32,
}
impl HeightWorld for Heights {
    fn max_y(&self) -> i32 {
        319
    }
    fn height(&mut self, map: Heightmap, x: i32, z: i32) -> Result<i32> {
        assert_eq!(
            map,
            if self.beached {
                Heightmap::WorldSurface
            } else {
                Heightmap::OceanFloor
            }
        );
        self.calls += 1;
        self.hash = self
            .hash
            .wrapping_mul(31)
            .wrapping_add(x)
            .wrapping_mul(31)
            .wrapping_add(z);
        Ok(-80 + (x * 31 + z * 17 + self.case * 7).rem_euclid(201))
    }
}
#[derive(Default)]
struct Marker {
    kind: i32,
    calls: i32,
    table: Option<String>,
    seed: i64,
}
impl MarkerWorld for Marker {
    fn is_randomizable_container(&mut self, pos: [i32; 3]) -> Result<bool> {
        assert_eq!(pos, [-7, 55, 21]);
        self.calls += 1;
        Ok(self.kind == 2)
    }
    fn set_loot_table(&mut self, pos: [i32; 3], table: &str, seed: i64) -> Result<()> {
        assert_eq!(pos, [-7, 55, 21]);
        self.table = Some(table.into());
        self.seed = seed;
        Ok(())
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn shipwreck_alignment_and_markers_match_java() -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
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
    let cp = std::env::join_paths(paths)?;
    let source = temp.path().join("ShipwreckBehaviorOracle.java");
    std::fs::write(&source, include_str!("java/ShipwreckBehaviorOracle.java"))?;
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
        .arg("ShipwreckBehaviorOracle")
        .output()?;
    ensure!(
        output.status.success(),
        "Java oracle: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout)?;
    let mut counts = [0, 0];
    for line in text.lines() {
        let row: Vec<_> = line.split_whitespace().collect();
        if row.first() == Some(&"H") {
            let i: i32 = row[1].parse()?;
            let beached = i & 1 != 0;
            let mut adjusted = i % 7 == 0;
            let mut piece = Piece {
                template: "minecraft:shipwreck/with_mast".into(),
                position: [-200 + i, 90, 300 - i * 2],
                rotation: [
                    Rotation::None,
                    Rotation::Clockwise90,
                    Rotation::Clockwise180,
                    Rotation::Counterclockwise90,
                ][i as usize % 4],
                beached,
            };
            let mut world = Heights {
                case: i,
                beached,
                calls: 0,
                hash: 0,
            };
            let mut random = LegacyRandom::new(i64::from(i) * 987654321 - 123456789);
            for _ in 0..2 {
                shipwreck::align_during_decoration(
                    &mut piece,
                    [i % 35, (i * 7) % 35, (i * 11) % 35],
                    &mut adjusted,
                    &mut world,
                    || random.next_int_bound(3).unwrap(),
                )?;
            }
            assert_eq!(piece.position[1], row[2].parse::<i32>()?, "{line}");
            assert_eq!(adjusted, row[3].parse::<bool>()?, "{line}");
            assert_eq!(world.calls, row[4].parse::<i32>()?, "{line}");
            assert_eq!(world.hash, row[5].parse::<i32>()?, "{line}");
            assert_eq!(random.next_int(), row[6].parse::<i32>()?, "{line}");
            counts[0] += 1;
        } else if row.first() == Some(&"M") {
            let i: usize = row[1].parse()?;
            let kind: i32 = row[2].parse()?;
            let mut world = Marker {
                kind,
                ..Default::default()
            };
            let mut random = LegacyRandom::new(-456789123 + i as i64 * 89 + i64::from(kind));
            shipwreck::handle_data_marker(
                &mut world,
                &mut random,
                [
                    "map_chest",
                    "treasure_chest",
                    "supply_chest",
                    "unknown",
                    "MAP_CHEST",
                ][i],
                [-7, 56, 21],
            )?;
            assert_eq!(world.calls, row[3].parse::<i32>()?, "{line}");
            assert_eq!(world.table.as_deref().unwrap_or("-"), row[4], "{line}");
            assert_eq!(world.seed, row[5].parse::<i64>()?, "{line}");
            assert_eq!(random.next_long(), row[6].parse::<i64>()?, "{line}");
            counts[1] += 1;
        }
    }
    assert_eq!(counts, [256, 15]);
    Ok(())
}
