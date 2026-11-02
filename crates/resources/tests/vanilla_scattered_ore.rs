use anyhow::{Context, Result};
use native_core::random::DecorationRandom;
use native_resources::{
    ore_feature::{OreBlockTags, OreVein, OreVeinWorld, OreWorld, ScatteredOre},
    packs::{ResourceId, ResourceProvider, ResourceStack},
    placed_feature::{PlacedFeature, PlacementEnvironment},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    process::Command,
};

#[derive(Default)]
struct World {
    blocks: BTreeMap<[i32; 3], String>,
    writes: Vec<String>,
    ground: bool,
    restrict: bool,
}
impl OreVeinWorld for World {
    fn ocean_floor_height(&mut self, _: i32, _: i32) -> Result<i32> {
        Ok(if self.ground { 20 } else { -100 })
    }
    fn outside_build_height(&self, y: i32) -> bool {
        !(-16..16).contains(&y)
    }
    fn ensure_can_write(&mut self, p: [i32; 3]) -> Result<bool> {
        Ok(!self.restrict || (p[0] + p[2]) & 31 != 0)
    }
    fn section_block(&mut self, p: [i32; 3]) -> Result<Option<String>> {
        Ok(Some(self.block(p)?))
    }
    fn section_neighbor(&mut self, p: [i32; 3]) -> Result<String> {
        if self.outside_build_height(p[1]) {
            Ok("minecraft:air".into())
        } else {
            self.block(p)
        }
    }
    fn set_section_block(&mut self, p: [i32; 3], s: &String) -> Result<()> {
        self.writes
            .push(format!("W {} {} {} {s}", p[0], p[1], p[2]));
        self.blocks.insert(p, s.clone());
        Ok(())
    }
}
struct Environment;
impl PlacementEnvironment for Environment {
    fn min_y(&self) -> i32 {
        -64
    }
    fn heightmap(&mut self, _: &str, _: i32, _: i32) -> Result<i32> {
        unreachable!()
    }
    fn biome_allows(&mut self, _: [i32; 3]) -> Result<bool> {
        unreachable!()
    }
}
fn initial(p: [i32; 3]) -> String {
    let key = p[0]
        .wrapping_mul(31)
        .wrapping_add(p[1].wrapping_mul(17))
        .wrapping_add(p[2].wrapping_mul(13));
    if key & 7 == 0 {
        "minecraft:air"
    } else if p[1] < 0 {
        "minecraft:deepslate"
    } else {
        "minecraft:stone"
    }
    .to_owned()
}
impl OreWorld for World {
    type State = String;
    fn block(&mut self, p: [i32; 3]) -> Result<String> {
        Ok(self.blocks.get(&p).cloned().unwrap_or_else(|| initial(p)))
    }
    fn block_is(&mut self, s: &String, id: &ResourceId) -> Result<bool> {
        Ok(*s == format!("{}:{}", id.namespace, id.path))
    }
    fn in_tag(&mut self, s: &String, tag: &ResourceId) -> Result<bool> {
        Ok(tag.namespace == "minecraft"
            && tag.path == "base_stone_overworld"
            && matches!(s.as_str(), "minecraft:stone" | "minecraft:deepslate"))
    }
    fn is_air(&mut self, s: &String) -> Result<bool> {
        Ok(s == "minecraft:air")
    }
    fn set_block(&mut self, p: [i32; 3], s: &String, flags: i32) -> Result<bool> {
        self.writes
            .push(format!("W {} {} {} {s} {flags}", p[0], p[1], p[2]));
        self.blocks.insert(p, s.clone());
        Ok(true)
    }
}
fn resolve(v: &Value) -> Result<String> {
    Ok(v.as_str().context("fixture requires an ID")?.to_owned())
}
fn definition(size: i32, chance: f32) -> Value {
    json!({"type":"minecraft:scattered_ore","size":size,"discard_chance_on_air_exposure":chance,"targets":[
        {"state":"minecraft:ancient_debris","target":{"predicate_type":"minecraft:all_of","rules":[{"predicate_type":"minecraft:tag_match","tag":"minecraft:base_stone_overworld"},{"predicate_type":"minecraft:height_match","min_inclusive":-2,"max_inclusive":2},{"predicate_type":"minecraft:not","rule":{"predicate_type":"minecraft:random_block_match","block":"minecraft:stone","probability":0.25}}]}},
        {"state":"minecraft:gold_ore","target":{"predicate_type":"minecraft:any_of","rules":[{"predicate_type":"minecraft:block_match","block":"minecraft:deepslate"},{"predicate_type":"minecraft:random_block_match","block":"minecraft:stone","probability":0.5}]}}
    ]})
}
#[test]
fn validates_feature_settings() {
    assert!(ScatteredOre::decode(&definition(65, 0.0), &mut resolve).is_err());
    assert!(ScatteredOre::decode(&definition(3, 1.1), &mut resolve).is_err());
    let mut bad = definition(3, 0.0);
    bad["targets"][0]["target"]["predicate_type"] = json!("example:unknown");
    assert!(ScatteredOre::decode(&bad, &mut resolve).is_err());
    let feature = ScatteredOre::decode(&definition(0, 1.0), &mut resolve).unwrap();
    let mut world = World::default();
    assert!(
        feature
            .place(&mut world, &mut DecorationRandom::new(1), [0, 0, 0])
            .unwrap()
    );
    assert!(world.writes.is_empty());
}
#[test]
fn replacement_tags_follow_active_pack_layers() {
    struct Data(BTreeMap<String, Vec<Vec<u8>>>);
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data_stack(&mut self, path: &str) -> Result<Vec<Vec<u8>>> {
            Ok(self.0.get(path).cloned().unwrap_or_default())
        }
    }
    let mut data = Data(BTreeMap::from([
        (
            "data/minecraft/tags/block/base_stone_overworld.json".into(),
            vec![
                br#"{"values":["minecraft:stone"]}"#.to_vec(),
                br##"{"replace":true,"values":["#example:rocks"]}"##.to_vec(),
            ],
        ),
        (
            "data/example/tags/block/rocks.json".into(),
            vec![br#"{"values":["minecraft:deepslate"]}"#.to_vec()],
        ),
    ]));
    let registry: BTreeSet<String> = ["minecraft:stone", "minecraft:deepslate"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let feature = ScatteredOre::decode(&definition(3, 1.0), &mut resolve).unwrap();
    let tags = OreBlockTags::load(&mut data, &registry, &feature.required_tags()).unwrap();
    let tag = ResourceId::parse("minecraft:base_stone_overworld").unwrap();
    assert!(!tags.contains(&tag, "minecraft:stone").unwrap());
    assert!(tags.contains(&tag, "minecraft:deepslate").unwrap());
    assert!(tags.contains(&tag, "example:unknown").is_err());
}
fn run(command: &mut Command) -> String {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "Java ore oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn scattered_ores_and_randomness_match_java() {
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
    let source = temp.path().join("ScatteredOreOracle.java");
    std::fs::write(&source, include_str!("java/ScatteredOreOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("ScatteredOreOracle"));
    let mut expected = Vec::new();
    for size in [0, 1, 3, 12, 64] {
        for chance in [0.0f32, 0.5, 1.0] {
            let feature = ScatteredOre::decode(&definition(size, chance), &mut resolve).unwrap();
            for seed in 0..16i64 {
                expected.push(format!("C {size} {chance:.1} {seed}"));
                let mut random = DecorationRandom::new(0);
                let deco = random.set_decoration_seed(seed, -32, 48);
                random.set_feature_seed(deco, 7, 3);
                let mut world = World::default();
                let result = feature
                    .place(&mut world, &mut random, [-16, 0, 32])
                    .unwrap();
                expected.extend(world.writes);
                expected.push(format!("R {result} {}", random.next_long()));
            }
        }
    }
    let placement=PlacedFeature::decode(&json!({"feature":"minecraft:ore_ancient_debris_large","placement":[
        {"type":"minecraft:count","count":4},{"type":"minecraft:in_square"},
        {"type":"minecraft:height_range","height":{"type":"minecraft:uniform","min_inclusive":{"absolute":-5},"max_inclusive":{"absolute":5}}}
    ]})).unwrap();
    let ore = ScatteredOre::decode(&definition(12, 0.5), &mut resolve).unwrap();
    for seed in 0..16i64 {
        expected.push(format!("I {seed}"));
        let mut random = DecorationRandom::new(0);
        let deco = random.set_decoration_seed(seed, -32, 48);
        random.set_feature_seed(deco, 7, 3);
        let mut world = World::default();
        let mut positions = placement.positions([-32, 0, 48], -64, 384).unwrap();
        while let Some(pos) = positions.next(&mut random, &mut Environment).unwrap() {
            ore.place(&mut world, &mut random, pos).unwrap();
        }
        expected.extend(world.writes);
        expected.push(format!("T {}", random.next_long()));
    }
    let actual: Vec<_> = output
        .lines()
        .filter(|line| {
            line.starts_with("C ")
                || line.starts_with("W ")
                || line.starts_with("R ")
                || line.starts_with("I ")
                || line.starts_with("T ")
        })
        .collect();
    assert_eq!(
        actual,
        expected.iter().map(String::as_str).collect::<Vec<_>>()
    );
    let mut resources = ResourceStack::open(&game).unwrap();
    let generated_registry = resources.vanilla_generated_block_registry().unwrap();
    let registry: BTreeSet<String> = output
        .lines()
        .filter_map(|line| line.strip_prefix("B "))
        .map(str::to_owned)
        .collect();
    assert!(registry.len() > 1000);
    for name in ["ore_ancient_debris_large", "ore_ancient_debris_small"] {
        let feature = ScatteredOre::load(
            &mut resources,
            &ResourceId::parse(&format!("minecraft:{name}")).unwrap(),
            &mut resolve,
        )
        .unwrap();
        ScatteredOre::load(
            &mut resources,
            &ResourceId::parse(&format!("minecraft:{name}")).unwrap(),
            &mut |state| generated_registry.resolve(state),
        )
        .unwrap();
        let tags = OreBlockTags::load(&mut resources, &registry, &feature.required_tags()).unwrap();
        let tag = ResourceId::parse("minecraft:base_stone_nether").unwrap();
        for block in [
            "minecraft:netherrack",
            "minecraft:basalt",
            "minecraft:blackstone",
        ] {
            assert!(tags.contains(&tag, block).unwrap());
        }
        assert!(!tags.contains(&tag, "minecraft:stone").unwrap());
    }
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn ore_veins_and_randomness_match_java() {
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
    let source = temp.path().join("OreVeinOracle.java");
    std::fs::write(&source, include_str!("java/OreVeinOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("OreVeinOracle"));
    let mut expected = Vec::new();
    for size in [0, 1, 3, 9, 17, 33, 64] {
        for chance in [0.0f32, 0.5, 1.0] {
            let mut value = definition(size, chance);
            value["type"] = json!("minecraft:ore");
            value["targets"][0]["state"] = json!("minecraft:diamond_ore");
            let feature = OreVein::decode(&value, &mut resolve).unwrap();
            for seed in 0..8i64 {
                expected.push(format!("C {size} {chance:.1} {seed}"));
                let mut random = DecorationRandom::new(0);
                let deco = random.set_decoration_seed(seed, -32, 48);
                random.set_feature_seed(deco, 7, 3);
                let mut world = World {
                    ground: seed % 4 != 0,
                    restrict: seed % 3 == 0,
                    ..Default::default()
                };
                let result = feature
                    .place(
                        &mut world,
                        &mut random,
                        [-16, if seed % 2 == 0 { -1 } else { 3 }, 32],
                    )
                    .unwrap();
                expected.extend(world.writes);
                expected.push(format!("R {result} {}", random.next_long()));
            }
        }
    }
    let placement=PlacedFeature::decode(&json!({"feature":"minecraft:ore_diamond_small","placement":[
        {"type":"minecraft:count","count":4},{"type":"minecraft:in_square"},
        {"type":"minecraft:height_range","height":{"type":"minecraft:uniform","min_inclusive":{"absolute":-5},"max_inclusive":{"absolute":5}}}
    ]})).unwrap();
    let mut value = definition(9, 0.5);
    value["type"] = json!("minecraft:ore");
    value["targets"][0]["state"] = json!("minecraft:diamond_ore");
    let feature = OreVein::decode(&value, &mut resolve).unwrap();
    for seed in 0..8i64 {
        expected.push(format!("I {seed}"));
        let mut random = DecorationRandom::new(0);
        let deco = random.set_decoration_seed(seed, -32, 48);
        random.set_feature_seed(deco, 7, 3);
        let mut world = World {
            ground: true,
            ..Default::default()
        };
        let mut positions = placement.positions([-32, 0, 48], -16, 32).unwrap();
        while let Some(pos) = positions.next(&mut random, &mut Environment).unwrap() {
            feature.place(&mut world, &mut random, pos).unwrap();
        }
        expected.extend(world.writes);
        expected.push(format!("T {}", random.next_long()));
    }
    let actual: Vec<_> = output
        .lines()
        .filter(|line| {
            line.starts_with("C ")
                || line.starts_with("W ")
                || line.starts_with("R ")
                || line.starts_with("I ")
                || line.starts_with("T ")
        })
        .collect();
    assert_eq!(actual.len(), expected.len(), "different number of records");
    for (index, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
        assert_eq!(*actual, expected, "ore record {index}");
    }
    let mut resources = ResourceStack::open(&game).unwrap();
    let generated_registry = resources.vanilla_generated_block_registry().unwrap();
    for name in ["ore_diamond_small", "ore_iron", "ore_coal", "ore_dirt"] {
        OreVein::load(
            &mut resources,
            &ResourceId::parse(&format!("minecraft:{name}")).unwrap(),
            &mut |state| generated_registry.resolve(state),
        )
        .unwrap();
    }
}
