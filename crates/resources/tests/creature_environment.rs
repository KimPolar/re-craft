use native_core::random::LegacyRandom;
use native_resources::{
    mob_spawns::{load_biome_spawn_attributes, resolve_creature_spawns},
    packs::{ResourceProvider, ResourceStack},
};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn creature_environment_layers_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let attr = |v: Value| json!({"minecraft:gameplay/creature_world_gen_spawn_probability":v});
    let spawn = |entity: &str| json!({"minecraft:gameplay/natural_mob_spawns":{"spawns_by_category":{"creature":[{"type":entity,"count":2,"weight":1}]},"spawn_costs":{}}});
    let dimensions = vec![
        json!({}),
        attr(json!(0.25)),
        attr(json!({"modifier":"add","argument":0.75})),
        spawn("minecraft:pig"),
    ];
    let spawn_biomes = vec![json!({}), spawn("minecraft:sheep"), attr(json!(0.8))];
    let probability_biomes = vec![
        json!({}),
        attr(json!(0.03)),
        attr(json!({"modifier":"multiply","argument":2.0})),
        attr(json!(-0.0)),
        spawn("minecraft:cow"),
    ];
    let later = vec![
        vec![],
        vec![attr(json!({"modifier":"subtract","argument":0.25}))],
        vec![
            attr(json!({"modifier":"maximum","argument":0.9})),
            attr(json!({"modifier":"minimum","argument":0.4})),
        ],
        vec![
            attr(json!({"modifier":"add","argument":2.0})),
            attr(json!({"modifier":"subtract","argument":1.5})),
        ],
        vec![spawn("minecraft:cow")],
    ];
    let mut cases = Vec::new();
    for dimension in &dimensions {
        for spawn_biome in &spawn_biomes {
            for probability_biome in &probability_biomes {
                for layers in &later {
                    cases.push(json!({"dimension":dimension,"spawn_biome":spawn_biome,"probability_biome":probability_biome,"later":layers}));
                }
            }
        }
    }
    let mut resources = ResourceStack::open(&game).unwrap();
    let biomes = resources.list_data("worldgen/biome").unwrap();
    assert!(!biomes.is_empty());
    for biome in biomes {
        let attributes = load_biome_spawn_attributes(&mut resources, &biome).unwrap();
        let mut selected = serde_json::Map::new();
        for name in [
            "minecraft:gameplay/natural_mob_spawns",
            "minecraft:gameplay/creature_world_gen_spawn_probability",
        ] {
            if let Some(value) = attributes.get(name) {
                selected.insert(name.to_owned(), value.clone());
            }
        }
        cases.push(json!({"dimension":{},"spawn_biome":selected,"probability_biome":selected,"later":[],"biome":format!("{}:{}",biome.namespace,biome.path)}));
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
    let source = temp.path().join("CreatureEnvironmentOracle.java");
    std::fs::write(&source, include_str!("java/CreatureEnvironmentOracle.java")).unwrap();
    let run = |cmd: &mut Command| {
        let result = cmd.output().unwrap();
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
    let input = temp.path().join("cases.json");
    let output = temp.path().join("results.json");
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    run(Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg("CreatureEnvironmentOracle")
        .arg(&input)
        .arg(&output));
    let expected: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(expected.len(), cases.len());
    for (case, expected) in cases.iter().zip(expected) {
        let result = resolve_creature_spawns(
            &case["dimension"],
            &case["spawn_biome"],
            &case["probability_biome"],
            case["later"].as_array().unwrap(),
            |id| (id.namespace == "minecraft").then_some(false),
        )
        .unwrap();
        let mut random = LegacyRandom::new(42);
        let samples: Vec<_> = (0..8)
            .map(|_| match result.spawns.choose(&mut random) {
                Some((id, count)) => json!([format!("{}:{}", id.namespace, id.path), count]),
                None => Value::Null,
            })
            .collect();
        assert_eq!(
            json!({"bits":result.probability.to_bits() as i32,"samples":samples,"next":random.next_int()}),
            expected,
            "{case}"
        );
    }
}
