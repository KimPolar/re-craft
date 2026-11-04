use anyhow::Result;
use native_core::random::LegacyRandom;
use native_resources::{
    creature_timeline::CreatureSpawnEnvironment,
    mob_spawns::resolve_creature_spawns,
    packs::{ResourceId, ResourceProvider, ResourceStack},
};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};

struct Data {
    dimension: Value,
    timeline: Value,
}
impl ResourceProvider for Data {
    fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        Ok(match path {
            "data/example/dimension_type/test.json" => Some(serde_json::to_vec(&self.dimension)?),
            "data/example/timeline/test.json" => Some(serde_json::to_vec(&self.timeline)?),
            _ => None,
        })
    }
}

#[test]
fn sampled_timeline_layers_preserve_duplicates_and_require_clocks() {
    let mut data = Data {
        dimension: json!({"timelines":["example:test","example:test"]}),
        timeline: json!({"clock":"example:clock","period_ticks":20,"tracks":{
            "gameplay/creature_world_gen_spawn_probability":{"modifier":"add","keyframes":[{"ticks":0,"value":0.2}]}
        }}),
    };
    let environment =
        CreatureSpawnEnvironment::load(&mut data, "example:test", |_| Some(false)).unwrap();
    assert!(environment.sample_layers(|_| None).is_err());
    let mut clock_reads = 0;
    let layers = environment
        .sample_layers(|_| {
            clock_reads += 1;
            Some(-1)
        })
        .unwrap();
    assert_eq!(clock_reads, 1);
    assert_eq!(layers.len(), 2);
    let result = resolve_creature_spawns(
        environment.dimension_attributes(),
        &json!({}),
        &json!({}),
        &layers,
        |_| Some(false),
    )
    .unwrap();
    assert_eq!(
        result.probability.to_bits(),
        (0.1_f32 + 0.2_f32 + 0.2_f32).to_bits()
    );
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn creature_timeline_tracks_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let mut resources = ResourceStack::open(&game).unwrap();
    for dimension in [
        "minecraft:overworld",
        "minecraft:overworld_caves",
        "minecraft:the_nether",
        "minecraft:the_end",
    ] {
        let environment = CreatureSpawnEnvironment::load(&mut resources, dimension, |id| {
            (id.namespace == "minecraft").then_some(false)
        })
        .unwrap();
        environment.sample_layers(|_| Some(0)).unwrap();
    }
    let ticks = [-41_i64, -20, -1, 0, 1, 5, 9, 10, 11, 15, 19, 20, 21, 41];
    let settings = |name: &str| json!({"spawns_by_category":{"creature":[{"type":name,"weight":1,"count":1}]},"spawn_costs":{}});
    let mut cases = Vec::new();
    for period in [None, Some(20)] {
        for ease in [
            json!("linear"),
            json!("constant"),
            json!({"cubic_bezier":[0.25,0.1,0.25,1.0]}),
            json!({"cubic_bezier":[0.2,1.5,0.8,-0.5]}),
        ] {
            for modifier in ["override", "add", "multiply", "minimum", "maximum"] {
                cases.push(json!({"kind":"probability","period":period,"ticks":ticks,"track":{"modifier":modifier,"ease":ease,"keyframes":[{"ticks":0,"value":0.2},{"ticks":10,"value":0.8},{"ticks":10,"value":0.4},{"ticks":20,"value":0.9}]}}));
            }
            for modifier in ["override", "overlay"] {
                cases.push(json!({"kind":"spawns","period":period,"ticks":ticks,"track":{"modifier":modifier,"ease":ease,"keyframes":[{"ticks":0,"value":settings("minecraft:pig")},{"ticks":10,"value":settings("minecraft:sheep")},{"ticks":20,"value":settings("minecraft:cow")}]}}));
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
    let source = temp.path().join("CreatureTimelineOracle.java");
    std::fs::write(&source, include_str!("java/CreatureTimelineOracle.java")).unwrap();
    let run = |cmd: &mut Command| {
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run(Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    let input = temp.path().join("cases.json");
    let output = temp.path().join("results.json");
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    run(Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg("CreatureTimelineOracle")
        .arg(&input)
        .arg(&output));
    let results: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(results.len(), cases.len());
    for (case, expected) in cases.iter().zip(results) {
        let key = if case["kind"] == "probability" {
            "minecraft:gameplay/creature_world_gen_spawn_probability"
        } else {
            "minecraft:gameplay/natural_mob_spawns"
        };
        let mut timeline = json!({"clock":"example:clock","tracks":{}});
        timeline["tracks"][key] = case["track"].clone();
        if !case["period"].is_null() {
            timeline["period_ticks"] = case["period"].clone();
        }
        let mut data = Data {
            dimension: json!({"timelines":["example:test"]}),
            timeline,
        };
        let env = CreatureSpawnEnvironment::load(&mut data, "example:test", |id| {
            (id.namespace == "minecraft").then_some(false)
        })
        .unwrap();
        assert!(env.sample_layers(|_| None).is_err());
        let actual: Vec<Value> = ticks
            .iter()
            .map(|&tick| {
                let layers = env
                    .sample_layers(|id| {
                        (id == &ResourceId::parse("example:clock").unwrap()).then_some(tick)
                    })
                    .unwrap();
                let settings = resolve_creature_spawns(
                    env.dimension_attributes(),
                    &json!({}),
                    &json!({}),
                    &layers,
                    |_| Some(false),
                )
                .unwrap();
                if case["kind"] == "probability" {
                    json!(settings.probability.to_bits() as i32)
                } else {
                    let mut random = LegacyRandom::new(42);
                    match settings.spawns.choose(&mut random) {
                        Some((id, _)) => json!(format!("{}:{}", id.namespace, id.path)),
                        None => json!("empty"),
                    }
                }
            })
            .collect();
        assert_eq!(json!(actual), expected, "{case}");
    }
}
