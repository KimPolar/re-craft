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
    let mut random = DecorationRandom::new(0);
    let decoration = random.set_decoration_seed(seed, -32, 48);
    random.set_feature_seed(decoration, 7, 3);
    let mut env = Environment { allow };
    let mut iter = feature.positions([-32, 0, 48], -64, 384).unwrap();
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
        json!({"feature":"minecraft:ore_dirt","placement":[]}),
    ];
    let mut resources = ResourceStack::open(&game).unwrap();
    for name in ["ore_diamond", "ore_dirt", "ore_coal_upper"] {
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
        for seed in [0, 1, -1, 987654321] {
            expected.push(format!("C {index} {seed}"));
            expected.extend(collect(&feature, seed, true));
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
