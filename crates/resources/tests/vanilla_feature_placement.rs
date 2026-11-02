use anyhow::Result;
use native_core::random::DecorationRandom;
use native_resources::{
    packs::{ResourceId, ResourceProvider, ResourceStack},
    placed_feature::{PlacedFeature, PlacementEnvironment},
};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};

struct Environment {
    allow: bool,
}
impl PlacementEnvironment for Environment {
    fn min_y(&self) -> i32 {
        -64
    }
    fn heightmap(&mut self, _: &str, x: i32, z: i32) -> Result<i32> {
        Ok(if (x ^ z) & 7 == 0 { -64 } else { 70 + (x & 7) })
    }
    fn biome_allows(&mut self, _: [i32; 3]) -> Result<bool> {
        Ok(self.allow)
    }
}
fn collect(feature: &PlacedFeature, seed: i64, allow: bool) -> Vec<String> {
    collect_at(feature, seed, allow, [-32, 0, 48])
}
fn collect_at(feature: &PlacedFeature, seed: i64, allow: bool, origin: [i32; 3]) -> Vec<String> {
    let mut random = DecorationRandom::new(0);
    let decoration = random.set_decoration_seed(seed, -32, 48);
    random.set_feature_seed(decoration, 7, 3);
    let mut env = Environment { allow };
    let mut iter = feature.positions(origin, -64, 384).unwrap();
    let mut output = Vec::new();
    while let Some([x, y, z]) = iter.next(&mut random, &mut env).unwrap() {
        let extra = random.next_int_bound(101).unwrap();
        output.push(format!("P {x} {y} {z} {extra}"));
    }
    output.push(format!("R {}", random.next_long()));
    output
}
#[test]
fn modifiers_reject_unknown_and_filter_missing_height_and_biome() {
    let def = json!({"feature":"minecraft:ore_dirt","placement":[{"type":"minecraft:count","count":8},{"type":"minecraft:in_square"},{"type":"minecraft:heightmap","heightmap":"WORLD_SURFACE_WG"},{"type":"minecraft:biome"}]});
    let feature = PlacedFeature::decode(&def).unwrap();
    assert_eq!(collect(&feature, 42, false).len(), 1);
    let places = collect(&feature, 42, true);
    assert!(places.len() > 1 && places.len() <= 9);
    let at_empty_height = PlacedFeature::decode(&json!({"feature":"minecraft:ore_dirt","placement":[{"type":"minecraft:heightmap","heightmap":"WORLD_SURFACE_WG"}]})).unwrap();
    assert_eq!(collect(&at_empty_height, 42, true).len(), 1);
    assert!(
        PlacedFeature::decode(
            &json!({"feature":"minecraft:ore_dirt","placement":[{"type":"example:custom"}]})
        )
        .is_err()
    );
    assert!(PlacedFeature::decode(&json!({"feature":"minecraft:ore_dirt","placement":[{"type":"minecraft:count","count":4097}]})).is_err());
}

#[test]
fn active_data_override_is_used_and_failed_queries_cannot_resume() {
    struct Data;
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok((path=="data/example/worldgen/placed_feature/test.json").then(||
                serde_json::to_vec(&json!({"feature":"example:replacement","placement":[{"type":"minecraft:in_square"},{"type":"minecraft:heightmap","heightmap":"WORLD_SURFACE_WG"}]})).unwrap()))
        }
    }
    struct Missing;
    impl PlacementEnvironment for Missing {
        fn min_y(&self) -> i32 {
            -64
        }
        fn heightmap(&mut self, _: &str, _: i32, _: i32) -> Result<i32> {
            anyhow::bail!("chunk unavailable")
        }
        fn biome_allows(&mut self, _: [i32; 3]) -> Result<bool> {
            unreachable!()
        }
    }
    let feature =
        PlacedFeature::load(&mut Data, &ResourceId::parse("example:test").unwrap()).unwrap();
    assert_eq!(feature.feature, "example:replacement");
    let mut positions = feature.positions([0, 0, 0], -64, 384).unwrap();
    let mut random = DecorationRandom::new(1);
    assert!(positions.next(&mut random, &mut Missing).is_err());
    assert!(positions.next(&mut random, &mut Missing).is_err());
}
fn run(command: &mut Command) -> String {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "placement oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn modifiers_and_feature_randomness_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"].as_array().unwrap() {
        if let Some(p) = lib["downloads"]["artifact"]["path"].as_str() {
            let p = game.join("libraries").join(p);
            if p.is_file() {
                paths.push(p)
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("FeaturePlacementOracle.java");
    std::fs::write(&source, include_str!("java/FeaturePlacementOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let mut cases = vec![
        json!({"feature":"minecraft:ore_dirt","placement":[{"type":"minecraft:count","count":{"type":"minecraft:uniform","min_inclusive":2,"max_inclusive":5}},{"type":"minecraft:in_square"},{"type":"minecraft:height_range","height":{"type":"minecraft:trapezoid","min_inclusive":{"above_bottom":-20},"max_inclusive":{"above_bottom":80},"plateau":7}},{"type":"minecraft:rarity_filter","chance":2}]}),
        json!({"feature":"minecraft:ore_dirt","placement":[{"type":"minecraft:count","count":{"type":"minecraft:weighted_list","distribution":[{"data":2,"weight":0},{"data":3,"weight":4},{"data":{"type":"minecraft:uniform","min_inclusive":4,"max_inclusive":6},"weight":1}]}},{"type":"minecraft:in_square"},{"type":"minecraft:heightmap","heightmap":"WORLD_SURFACE_WG"},{"type":"minecraft:count","count":2}]}),
        json!({"feature":"minecraft:ore_dirt","placement":[{"type":"minecraft:count","count":3},{"type":"minecraft:height_range","height":{"type":"minecraft:uniform","min_inclusive":{"below_top":0},"max_inclusive":{"absolute":-100}}}]}),
        json!({"feature":"minecraft:ore_dirt","placement":[{"type":"minecraft:count","count":8},{"type":"minecraft:offset","x":{"type":"minecraft:trapezoid","min":-7,"max":7,"plateau":0},"y":{"type":"minecraft:biased_to_bottom","min_inclusive":0,"max_inclusive":3},"z":{"type":"minecraft:trapezoid","min":-4,"max":5,"plateau":2}}]}),
        json!({"feature":"minecraft:ore_dirt","placement":[{"type":"minecraft:count","count":20},{"type":"minecraft:height_range","height":{"type":"minecraft:very_biased_to_bottom","min_inclusive":{"absolute":-64},"max_inclusive":{"absolute":-56},"inner":8}}]}),
        json!({"feature":"minecraft:ore_dirt","placement":[{"type":"minecraft:count","count":20},{"type":"minecraft:height_range","height":{"type":"minecraft:very_biased_to_bottom","min_inclusive":{"absolute":-64},"max_inclusive":{"absolute":-57},"inner":8}}]}),
        json!({"feature":"minecraft:ore_dirt","placement":[{"type":"minecraft:count","count":20},{"type":"minecraft:height_range","height":{"type":"minecraft:very_biased_to_bottom","min_inclusive":{"absolute":-64},"max_inclusive":{"absolute":-40}}}]}),
        json!({"feature":"minecraft:ore_dirt","origin":[5,69,0],"placement":[{"type":"minecraft:surface_relative_threshold_filter","heightmap":"OCEAN_FLOOR_WG","min_inclusive":-10,"max_inclusive":-1}]}),
        json!({"feature":"minecraft:ore_dirt","origin":[0,-2147483648,0],"placement":[{"type":"minecraft:surface_relative_threshold_filter","heightmap":"WORLD_SURFACE_WG"}]}),
        json!({"feature":"minecraft:ore_dirt","origin":[0,2147483647,0],"placement":[{"type":"minecraft:surface_relative_threshold_filter","heightmap":"WORLD_SURFACE_WG","min_inclusive":2147483647}]}),
        json!({"feature":"minecraft:ore_dirt","placement":[]}),
    ];
    let mut resources = ResourceStack::open(&game).unwrap();
    for name in [
        "ore_diamond",
        "ore_dirt",
        "ore_coal_upper",
        "spring_water",
        "spring_lava",
        "underwater_magma",
    ] {
        let id = ResourceId::parse(&format!("minecraft:{name}")).unwrap();
        PlacedFeature::load(&mut resources, &id).unwrap();
        let bytes = resources
            .read_builtin_data(&format!(
                "data/minecraft/worldgen/placed_feature/{name}.json"
            ))
            .unwrap()
            .unwrap();
        let mut value: Value = serde_json::from_slice(&bytes).unwrap();
        value["placement"]
            .as_array_mut()
            .unwrap()
            .retain(|entry| entry["type"] != "minecraft:biome");
        cases.push(value);
    }
    let coords = [
        [0, 0, 0],
        [-32, 0, 48],
        [1234, 9, -4321],
        [-30000000, 0, 30000000],
        [i32::MIN, -63, i32::MAX],
        [100, 1, 200],
        [-10000, 2, -20000],
    ];
    for name in [
        "bamboo",
        "kelp_cold",
        "kelp_warm",
        "warm_ocean_vegetation",
        "flower_cherry",
        "flower_plains",
        "patch_grass_meadow",
        "patch_grass_plain",
        "patch_tall_grass_2",
        "wildflowers_meadow",
    ] {
        let id = ResourceId::parse(&format!("minecraft:{name}")).unwrap();
        if !name.starts_with("kelp_") {
            PlacedFeature::load(&mut resources, &id).unwrap();
        }
        let bytes = resources
            .read_builtin_data(&format!(
                "data/minecraft/worldgen/placed_feature/{name}.json"
            ))
            .unwrap()
            .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        let noise = value["placement"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| {
                matches!(
                    v["type"].as_str(),
                    Some("minecraft:noise_threshold_count" | "minecraft:noise_based_count")
                )
            })
            .unwrap();
        for origin in coords {
            cases.push(json!({"feature":"minecraft:ore_dirt","origin":origin,"placement":[noise,{"type":"minecraft:in_square"},{"type":"minecraft:rarity_filter","chance":2}]}));
        }
    }
    for origin in [[0, 0, 0], [123, 0, -456]] {
        for modifier in [
            json!({"type":"minecraft:noise_based_count","noise_to_count_ratio":10,"noise_factor":0.0}),
            json!({"type":"minecraft:noise_based_count","noise_to_count_ratio":-10,"noise_factor":80.0,"noise_offset":0.4}),
            json!({"type":"minecraft:noise_threshold_count","noise_level":0.0,"below_noise":-2,"above_noise":3}),
        ] {
            cases.push(
                json!({"feature":"minecraft:ore_dirt","origin":origin,"placement":[modifier]}),
            );
        }
    }
    let input = temp.path().join("cases.json");
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("FeaturePlacementOracle")
        .arg(input));
    let mut expected = Vec::new();
    for (index, value) in cases.iter().enumerate() {
        let feature = PlacedFeature::decode(value).unwrap();
        let origin = value
            .get("origin")
            .and_then(Value::as_array)
            .map(|v| {
                [
                    v[0].as_i64().unwrap() as i32,
                    v[1].as_i64().unwrap() as i32,
                    v[2].as_i64().unwrap() as i32,
                ]
            })
            .unwrap_or([-32, 0, 48]);
        for seed in [0, 1, -1, 987654321] {
            expected.push(format!("C {index} {seed}"));
            expected.extend(collect_at(&feature, seed, true, origin));
        }
    }
    let actual: Vec<_> = output
        .lines()
        .filter(|line| line.starts_with("C ") || line.starts_with("P ") || line.starts_with("R "))
        .collect();
    assert_eq!(
        actual,
        expected.iter().map(String::as_str).collect::<Vec<_>>()
    );
}
