//! Compare simple structure generation stubs and piece bounds with Java 26.3.
use native_resources::structure_start::{
    BiomeQuery, Heightmap, HorizontalDirection, SimpleStructure, SimpleStructureKind,
    StartEnvironment,
};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};

fn run(command: &mut Command) -> String {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "Java structure oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
struct Environment {
    mode: i32,
    checks: usize,
}
impl StartEnvironment for Environment {
    fn vertical_bounds(&self) -> (i32, i32) {
        (-64, 319)
    }
    fn sea_level(&self) -> i32 {
        63
    }
    fn first_occupied_height(&mut self, x: i32, z: i32, map: Heightmap) -> anyhow::Result<i32> {
        let base = if self.mode == 1 { 60 } else { 75 };
        Ok(
            base + (x.wrapping_mul(13).wrapping_add(z.wrapping_mul(7))).rem_euclid(11)
                - 5
                - if map == Heightmap::OceanFloor { 10 } else { 0 },
        )
    }
    fn biome_allowed(&mut self, _: [i32; 3], _: BiomeQuery) -> anyhow::Result<bool> {
        self.checks += 1;
        Ok(self.mode != 2 && (self.mode != 3 || self.checks == 1))
    }
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn simple_starts_match_java_generation_stubs() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path)
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("SimpleStructureStartOracle.java");
    std::fs::write(
        &source,
        include_str!("java/SimpleStructureStartOracle.java"),
    )
    .unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("SimpleStructureStartOracle"));
    let types = [
        ("desert_pyramid", SimpleStructureKind::DesertPyramid),
        ("jungle_temple", SimpleStructureKind::JungleTemple),
        ("swamp_hut", SimpleStructureKind::SwampHut),
        ("buried_treasure", SimpleStructureKind::BuriedTreasure),
    ];
    let mut count = 0;
    for line in output.lines().filter(|s| s.starts_with("S ")) {
        let words: Vec<_> = line.split_whitespace().collect();
        let k: usize = words[1].parse().unwrap();
        let seed: i64 = words[2].parse().unwrap();
        let x: i32 = words[3].parse().unwrap();
        let z: i32 = words[4].parse().unwrap();
        let mode: i32 = words[5].parse().unwrap();
        let structure =
            SimpleStructure::decode(&json!({"type":format!("minecraft:{}",types[k].0)})).unwrap();
        let actual = structure
            .generate(&mut Environment { mode, checks: 0 }, seed, [x, z])
            .unwrap();
        if words[6] == "X" {
            assert!(actual.is_none(), "{line}");
        } else {
            let actual = actual.unwrap_or_else(|| panic!("expected start: {line}"));
            assert_eq!(actual.kind, types[k].1);
            let number = |i: usize| words[i].parse::<i32>().unwrap();
            assert_eq!(
                actual.generation_point,
                [number(7), number(8), number(9)],
                "{line}"
            );
            let orientation = match words[10] {
                "NORTH" => Some(HorizontalDirection::North),
                "EAST" => Some(HorizontalDirection::East),
                "SOUTH" => Some(HorizontalDirection::South),
                "WEST" => Some(HorizontalDirection::West),
                "NONE" => None,
                _ => panic!("{line}"),
            };
            assert_eq!(actual.orientation, orientation, "{line}");
            assert_eq!(
                actual.bounds.min,
                [number(11), number(12), number(13)],
                "{line}"
            );
            assert_eq!(
                actual.bounds.max,
                [number(14), number(15), number(16)],
                "{line}"
            );
        }
        count += 1;
    }
    assert_eq!(count, 4 * 6 * 31 * 4);
}
