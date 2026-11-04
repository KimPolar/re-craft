//! Compare creature spawn layers and weighted sampling with Java 26.3.
use native_core::random::LegacyRandom;
use native_resources::{
    mob_spawns::{CreatureSpawnLayer, CreatureSpawns},
    packs::{ResourceProvider, ResourceStack},
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
fn creature_layers_and_weighted_samples_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
        .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let mut resources = ResourceStack::open(&game).unwrap();
    let registry = resources.vanilla_entity_category_registry().unwrap();
    let entry = |name, weight, count: Value| json!({"type":name,"weight":weight,"count":count});
    let settings = |creature: Option<Value>| {
        let mut categories = json!({});
        if let Some(creature) = creature {
            categories["creature"] = creature;
        }
        json!({"spawns_by_category":categories,"spawn_costs":{}})
    };
    let attributes = |value: Value| json!({"minecraft:gameplay/natural_mob_spawns":value});
    let base = attributes(settings(Some(json!([
        entry("minecraft:pig", 3, json!(4)),
        entry(
            "minecraft:sheep",
            61,
            json!({"type":"minecraft:uniform","min_inclusive":2,"max_inclusive":6})
        ),
        entry("minecraft:item", 0, json!(1))
    ]))));
    let overlays = vec![
        vec![],
        vec![base.clone()],
        vec![
            base.clone(),
            attributes(json!({"modifier":"overlay","argument":settings(None)})),
        ],
        vec![
            base.clone(),
            attributes(json!({"modifier":"overlay","argument":settings(Some(json!([])))})),
        ],
        vec![base.clone(), attributes(settings(None))],
        vec![attributes(settings(Some(json!([entry(
            "minecraft:item",
            1,
            json!(2)
        )]))))],
        vec![attributes(settings(Some(json!([entry(
            "minecraft:pig",
            0,
            json!(1)
        )]))))],
        vec![attributes(settings(Some(json!([
            entry(
                "minecraft:pig",
                31,
                json!({"type":"minecraft:clamped","source":{"type":"minecraft:uniform","min_inclusive":-4,"max_inclusive":7},"min_inclusive":0,"max_inclusive":3})
            ),
            entry(
                "minecraft:sheep",
                32,
                json!({"type":"minecraft:weighted_list","distribution":[{"data":1,"weight":1},{"data":{"type":"minecraft:uniform","min_inclusive":2,"max_inclusive":5},"weight":3}]})
            )
        ]))))],
        vec![attributes(settings(Some(json!([
            entry(
                "minecraft:cow",
                1,
                json!({"type":"minecraft:biased_to_bottom","min_inclusive":1,"max_inclusive":5})
            ),
            entry(
                "minecraft:pig",
                2,
                json!({"type":"minecraft:trapezoid","min":-3,"max":3,"plateau":0})
            )
        ]))))],
    ];
    let mut cases = Vec::new();
    for layers in overlays {
        for seed in [0_i64, 42, -1, i64::MIN, i64::MAX] {
            cases.push(json!({"layers": layers, "seed":seed}));
        }
    }
    for biome in resources.list_data("worldgen/biome").unwrap() {
        let path = format!(
            "data/{}/worldgen/biome/{}.json",
            biome.namespace, biome.path
        );
        let data: Value =
            serde_json::from_slice(&resources.read_data(&path).unwrap().unwrap()).unwrap();
        let mut layer = serde_json::Map::new();
        if let Some(value) = data
            .get("attributes")
            .and_then(|v| v.get("minecraft:gameplay/natural_mob_spawns"))
        {
            layer.insert(
                "minecraft:gameplay/natural_mob_spawns".into(),
                value.clone(),
            );
        }
        cases.push(json!({"layers":[layer],"seed":42,"name":path,"biome":format!("{}:{}", biome.namespace, biome.path)}));
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
    let source = temp.path().join("MobSpawnLayerOracle.java");
    std::fs::write(&source, include_str!("java/MobSpawnLayerOracle.java")).unwrap();
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
        .arg("MobSpawnLayerOracle")
        .arg(&input)
        .arg(&output));
    let results: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(results.len(), cases.len());
    for (case, expected) in cases.iter().zip(results) {
        let mut spawns = CreatureSpawns::default();
        for layer in case["layers"].as_array().unwrap() {
            if let Some(layer) =
                CreatureSpawnLayer::from_attributes(layer, |id| registry.is_misc(id)).unwrap()
            {
                spawns.apply(layer);
            }
        }
        let mut random = LegacyRandom::new(case["seed"].as_i64().unwrap());
        let samples: Vec<_> = (0..24)
            .map(|_| match spawns.choose(&mut random) {
                Some((id, count)) => json!([format!("{}:{}", id.namespace, id.path), count]),
                None => Value::Null,
            })
            .collect();
        assert_eq!(
            json!({"samples": samples, "next": random.next_int()}),
            expected,
            "case {case}"
        );
        if let Some(biome) = case.get("biome").and_then(Value::as_str) {
            let id = native_resources::packs::ResourceId::parse(biome).unwrap();
            let mut loaded = CreatureSpawns::default();
            if let Some(layer) =
                CreatureSpawnLayer::load_biome(&mut resources, &id, |id| registry.is_misc(id))
                    .unwrap()
            {
                loaded.apply(layer);
            }
            let mut random = LegacyRandom::new(42);
            let samples: Vec<_> = (0..24)
                .map(|_| match loaded.choose(&mut random) {
                    Some((id, count)) => json!([format!("{}:{}", id.namespace, id.path), count]),
                    None => Value::Null,
                })
                .collect();
            assert_eq!(
                json!({"samples": samples, "next": random.next_int()}),
                expected,
                "loaded biome {biome}"
            );
        }
    }
}
