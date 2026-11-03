//! Differential test against the installed Java 26.3 generator.
use native_resources::{
    density::{DensityEvaluator, WorldgenRandom},
    packs::{ResourceProvider, ResourceStack},
    spawn_origin::NoiseSpawnTargets,
};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

fn run(command: &mut Command) -> String {
    let result = command.output().expect("cannot run Java 25");
    assert!(
        result.status.success(),
        "Java spawn-origin oracle failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn noise_spawn_origins_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
        .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
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
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("NoiseSpawnOriginOracle.java");
    std::fs::write(&source, include_str!("java/NoiseSpawnOriginOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg("NoiseSpawnOriginOracle"));
    let mut resources = ResourceStack::open(&game).unwrap();
    let mut count = 0;
    for line in output.lines().filter(|line| line.starts_with("P ")) {
        let fields: Vec<_> = line.split_whitespace().collect();
        let dim: usize = fields[1].parse().unwrap();
        let seed: i64 = fields[2].parse().unwrap();
        let expected = [
            fields[3].parse::<i32>().unwrap(),
            fields[4].parse::<i32>().unwrap(),
        ];
        let name = ["overworld", "nether", "end"][dim];
        let path = format!("data/minecraft/worldgen/noise_settings/{name}.json");
        let settings: Value =
            serde_json::from_slice(&resources.read_data(&path).unwrap().unwrap()).unwrap();
        let random = if settings["legacy_random_source"].as_bool().unwrap() {
            WorldgenRandom::Legacy
        } else {
            WorldgenRandom::Xoroshiro
        };
        let targets =
            NoiseSpawnTargets::compile(&mut resources, &settings["spawn_target"], seed, random)
                .unwrap();
        assert_eq!(
            targets.origin_chunk(&mut DensityEvaluator::default()),
            expected,
            "{name}, seed {seed}"
        );
        count += 1;
    }
    assert_eq!(count, 18);
    let mut custom_count = 0;
    for line in output.lines().filter(|line| line.starts_with("C ")) {
        let fields: Vec<_> = line.split_whitespace().collect();
        let seed: i64 = fields[1].parse().unwrap();
        let expected = [
            fields[2].parse::<i32>().unwrap(),
            fields[3].parse::<i32>().unwrap(),
        ];
        let bytes = resources
            .read_data("data/minecraft/worldgen/noise_settings/overworld.json")
            .unwrap()
            .unwrap();
        let settings: Value = serde_json::from_slice(&bytes).unwrap();
        let mut first = settings["spawn_target"][0].as_object().unwrap().clone();
        for range in first.values_mut() {
            *range = serde_json::json!([-0.5, 0.5]);
        }
        let targets = NoiseSpawnTargets::compile(
            &mut resources,
            &Value::Array(vec![Value::Object(first)]),
            seed,
            WorldgenRandom::Xoroshiro,
        )
        .unwrap();
        assert_eq!(
            targets.origin_chunk(&mut DensityEvaluator::default()),
            expected,
            "custom target, seed {seed}"
        );
        custom_count += 1;
    }
    assert_eq!(custom_count, 6);
}
