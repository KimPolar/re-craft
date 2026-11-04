//! Compare sheep color and common animal finalization against installed 26.3.
use native_resources::{
    density::WorldgenRandom,
    worldgen_region_random::{AnimalGroupData, RegionRandom, finalize_sheep},
};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn region_random_and_sheep_finalization_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
        .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let mut cases = Vec::new();
    for legacy in [false, true] {
        for seed in [0_i64, 1, 42, -1, i64::MIN, i64::MAX] {
            for (x, z) in [(0, 0), (-13, 17), (29, -7), (i32::MIN, i32::MAX)] {
                for (size, baby, chance) in [
                    (0, true, 0.05),
                    (1, true, 0.05),
                    (3, true, 1.0),
                    (2, false, 0.5),
                    (2, true, 0.0),
                ] {
                    for existing in [false, true] {
                        for (warm, cold) in
                            [(false, false), (true, false), (false, true), (true, true)]
                        {
                            cases.push(json!({"legacy":legacy,"seed":seed,"x":x,"z":z,"size":size,"baby":baby,"chance":chance,"existing":existing,"warm":warm,"cold":cold}));
                        }
                    }
                }
            }
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
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
    let source = temp.path().join("RegionAnimalOracle.java");
    std::fs::write(&source, include_str!("java/RegionAnimalOracle.java")).unwrap();
    let run = |command: &mut Command| {
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    };
    run(Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    let input = temp.path().join("input.json");
    let output = temp.path().join("output.json");
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    run(Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg("RegionAnimalOracle")
        .arg(&input)
        .arg(&output));
    let results: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(cases.len(), results.len());
    for (case, expected) in cases.iter().zip(results) {
        let legacy = case["legacy"].as_bool().unwrap();
        let existing = case["existing"].as_bool().unwrap();
        let mut random = RegionRandom::new(
            case["seed"].as_i64().unwrap(),
            if legacy {
                WorldgenRandom::Legacy
            } else {
                WorldgenRandom::Xoroshiro
            },
            case["x"].as_i64().unwrap() as i32,
            case["z"].as_i64().unwrap() as i32,
        );
        let mut group = AnimalGroupData {
            size: case["size"].as_i64().unwrap() as i32,
            should_spawn_baby: case["baby"].as_bool().unwrap(),
            baby_spawn_chance: case["chance"].as_f64().unwrap() as f32,
        };
        let mut animals = Vec::new();
        for _ in 0..3 {
            let sheep = finalize_sheep(
                &mut random,
                &mut group,
                existing,
                case["warm"].as_bool().unwrap(),
                case["cold"].as_bool().unwrap(),
            );
            let actual = sheep.animal;
            animals.push(json!({"color":sheep.color.as_str(), "age":if actual.set_baby_age { -24000 } else { 123 }, "size":group.size,
                "left":actual.left_handed, "modifier_bits":actual.follow_range_bonus.unwrap_or(0.25).to_bits() as i64,
                "permanent":!existing}));
        }
        assert_eq!(
            json!({"animals":animals,"next":random.next_long()}),
            expected,
            "{case}"
        );
    }
}
