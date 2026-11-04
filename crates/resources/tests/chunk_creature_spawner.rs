//! Differential tests using Java's real chunk-generation NaturalSpawner.
use native_resources::{
    mob_spawns::{CreatureSpawnLayer, CreatureSpawns},
    worldgen_mob_spawner::{AttemptRejection, ChunkCreatureSpawner, CreatureSpawnProgress},
};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};

fn run(command: &mut Command) {
    let output = command.output().expect("cannot run Java 25");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn creature_generation_attempts_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
        .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let settings = |entries: Value| {
        json!({"minecraft:gameplay/natural_mob_spawns": {
            "modifier":"overlay", "argument":{"spawns_by_category":{"creature":entries}, "spawn_costs":{}}
        }})
    };
    let lists = [
        settings(json!([])),
        settings(json!([{"type":"minecraft:pig","weight":0,"count":2}])),
        settings(json!([{"type":"minecraft:pig","weight":1,"count":0}])),
        settings(json!([{"type":"minecraft:pig","weight":1,"count":-2}])),
        settings(json!([
            {"type":"minecraft:pig","weight":5,"count":2},
            {"type":"minecraft:pig","weight":3,"count":{"type":"minecraft:uniform","min_inclusive":1,"max_inclusive":4}},
            {"type":"minecraft:pig","weight":0,"count":9}
        ])),
    ];
    let mut cases = Vec::new();
    for attributes in lists {
        for seed in [0_i64, 1, 42, -1, i64::MIN, i64::MAX] {
            for (x, z) in [(0, 0), (-13, 17), (29, -7)] {
                for probability in [0.0, 0.1, 0.75] {
                    for no_collision in [false, true] {
                        cases.push(json!({"attributes":attributes,"seed":seed,"x":x,"z":z,
                            "probability":probability,"enabled":true,"no_collision":no_collision}));
                    }
                }
            }
        }
    }
    cases.push(
        json!({"attributes":settings(json!([{"type":"minecraft:pig","weight":1,"count":2}])),
        "seed":42,"x":0,"z":0,"probability":0.75,"enabled":false,"no_collision":false}),
    );
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
    let source = temp.path().join("ChunkCreatureSpawnOracle.java");
    std::fs::write(&source, include_str!("java/ChunkCreatureSpawnOracle.java")).unwrap();
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
        .arg("ChunkCreatureSpawnOracle")
        .arg(&input)
        .arg(&output));
    let results: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(results.len(), cases.len());
    let mut observed = [0_usize; 2];
    for (case, expected) in cases.iter().zip(results) {
        let mut spawns = CreatureSpawns::default();
        if let Some(layer) =
            CreatureSpawnLayer::from_attributes(&case["attributes"], |_| Some(false)).unwrap()
        {
            spawns.apply(layer);
        }
        let mut planner = ChunkCreatureSpawner::new(
            case["seed"].as_i64().unwrap(),
            case["x"].as_i64().unwrap() as i32,
            case["z"].as_i64().unwrap() as i32,
            case["enabled"].as_bool().unwrap(),
            case["probability"].as_f64().unwrap() as f32,
            spawns,
        )
        .unwrap();
        let mut attempts = Vec::new();
        loop {
            match planner.next(1).unwrap() {
                CreatureSpawnProgress::Pending => continue,
                CreatureSpawnProgress::Finished => break,
                CreatureSpawnProgress::Attempt(attempt) => {
                    assert_eq!(
                        planner.next(0).unwrap(),
                        CreatureSpawnProgress::Attempt(attempt.clone())
                    );
                    attempts.push(json!([attempt.x, attempt.z]));
                    let reason = if case["no_collision"].as_bool().unwrap() {
                        AttemptRejection::CollisionRuleOrConstruction
                    } else {
                        AttemptRejection::Placement
                    };
                    planner.reject(reason).unwrap();
                }
            }
            assert!(attempts.len() < 100_000, "unexpected attempt count");
        }
        let mut random = planner.into_random().unwrap();
        if !attempts.is_empty() {
            observed[usize::from(case["no_collision"].as_bool().unwrap())] += 1;
        }
        let collision_calls = if case["no_collision"].as_bool().unwrap() {
            attempts.len()
        } else {
            0
        };
        assert_eq!(
            json!({"attempts": attempts, "collision_calls":collision_calls, "next": random.next_int()}),
            expected,
            "case {case}"
        );
    }
    assert!(
        observed.into_iter().all(|count| count > 0),
        "both rejection paths must execute"
    );
}
