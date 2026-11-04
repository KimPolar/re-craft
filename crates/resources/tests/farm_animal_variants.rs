use std::{collections::BTreeMap, path::PathBuf, process::Command};

use anyhow::Result;
use native_resources::{
    animal_variants::{FarmAnimal, FarmAnimalVariants},
    density::WorldgenRandom,
    packs::{ResourceId, ResourceProvider, ResourceStack},
    worldgen_region_random::RegionRandom,
};
use serde_json::{Value, json};

#[derive(Default)]
struct Data(BTreeMap<String, Vec<u8>>);

impl ResourceProvider for Data {
    fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.0.get(path).cloned())
    }
    fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
        let prefix = format!("data/minecraft/{folder}/");
        self.0
            .keys()
            .filter_map(|path| path.strip_prefix(&prefix)?.strip_suffix(".json"))
            .map(ResourceId::parse)
            .collect()
    }
}

fn id(s: &str) -> ResourceId {
    ResourceId::parse(s).unwrap()
}

#[test]
fn selectors_preserve_order_duplicates_and_reject_unknown_conditions() {
    let mut data = Data::default();
    for biome in ["plains", "desert"] {
        data.0.insert(
            format!("data/minecraft/worldgen/biome/{biome}.json"),
            b"{}".to_vec(),
        );
    }
    data.0.insert("data/minecraft/pig_variant/a.json".into(), serde_json::to_vec(&json!({
        "spawn_conditions":[{"priority":0},{"priority":2,"condition":{"type":"minecraft:biome","biomes":["minecraft:plains"]}},{"priority":2}]
    })).unwrap());
    data.0.insert("data/minecraft/pig_variant/b.json".into(), serde_json::to_vec(&json!({
        "spawn_conditions":[{"priority":2,"condition":{"type":"minecraft:biome","biomes":"minecraft:plains"}}]
    })).unwrap());
    let ordered = [id("b"), id("a")];
    let table = FarmAnimalVariants::load(&mut data, FarmAnimal::Pig, &ordered).unwrap();
    for seed in 0..100 {
        let mut actual = RegionRandom::new(seed, WorldgenRandom::Legacy, 0, 0);
        let mut reference = actual.clone();
        let expected =
            [&ordered[0], &ordered[1], &ordered[1]][reference.next_int_bound(3).unwrap() as usize];
        assert_eq!(
            table.choose(&id("plains"), &mut actual).unwrap(),
            Some(expected)
        );
        assert_eq!(actual.next_long(), reference.next_long());
    }
    let mut random = RegionRandom::new(42, WorldgenRandom::Legacy, 0, 0);
    let mut reference = random.clone();
    assert_eq!(
        table.choose(&id("desert"), &mut random).unwrap(),
        Some(&ordered[1])
    );
    reference.next_int_bound(1).unwrap();
    assert_eq!(random.next_long(), reference.next_long());
    assert!(table.choose(&id("unknown"), &mut random).is_err());
    assert!(FarmAnimalVariants::load(&mut data, FarmAnimal::Pig, &ordered[..1]).is_err());
    assert!(FarmAnimalVariants::load(&mut data, FarmAnimal::Pig, &[id("a"), id("a")]).is_err());
    data.0.insert("data/minecraft/pig_variant/b.json".into(), br#"{"spawn_conditions":[{"priority":1,"condition":{"type":"minecraft:moon_brightness"}}]}"#.to_vec());
    assert!(FarmAnimalVariants::load(&mut data, FarmAnimal::Pig, &ordered).is_err());
}

#[test]
fn tags_follow_data_pack_layers_and_missing_matches_do_not_consume_randomness() {
    struct Layered(Data);
    impl ResourceProvider for Layered {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            self.0.read_data(path)
        }
        fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
            self.0.list_data(folder)
        }
        fn read_data_stack(&mut self, path: &str) -> Result<Vec<Vec<u8>>> {
            if path == "data/minecraft/tags/worldgen/biome/variants.json" {
                Ok(vec![
                    br#"{"values":["minecraft:desert"]}"#.to_vec(),
                    br#"{"replace":true,"values":["minecraft:plains"]}"#.to_vec(),
                ])
            } else {
                Ok(Vec::new())
            }
        }
    }
    let mut data = Data::default();
    for biome in ["plains", "desert"] {
        data.0.insert(
            format!("data/minecraft/worldgen/biome/{biome}.json"),
            b"{}".to_vec(),
        );
    }
    data.0.insert("data/minecraft/cow_variant/custom.json".into(),
        br##"{"spawn_conditions":[{"priority":-2147483648,"condition":{"type":"minecraft:biome","biomes":"#minecraft:variants"}}]}"##.to_vec());
    let mut provider = Layered(data);
    let table = FarmAnimalVariants::load(&mut provider, FarmAnimal::Cow, &[id("custom")]).unwrap();
    let mut random = RegionRandom::new(7, WorldgenRandom::Xoroshiro, -1, 1);
    let mut untouched = random.clone();
    assert_eq!(table.choose(&id("desert"), &mut random).unwrap(), None);
    assert_eq!(random.next_long(), untouched.next_long());
    assert_eq!(
        table.choose(&id("plains"), &mut random).unwrap(),
        Some(&id("custom"))
    );
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn installed_variants_match_java_selection_and_random_state() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
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
    let source = temp.path().join("FarmAnimalVariantOracle.java");
    std::fs::write(&source, include_str!("java/FarmAnimalVariantOracle.java")).unwrap();
    let run = |command: &mut Command| {
        let output = command.output().unwrap();
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
        .arg(source));
    let mut cases = Vec::new();
    for animal in ["pig", "cow", "chicken"] {
        for biome in [
            "minecraft:plains",
            "minecraft:desert",
            "minecraft:snowy_plains",
            "minecraft:jungle",
            "minecraft:taiga",
        ] {
            for legacy in [true, false] {
                for seed in [0_i64, 1, -1, 42, i64::MIN, i64::MAX] {
                    cases.push(
                        json!({"animal":animal, "biome":biome, "legacy":legacy, "seed":seed}),
                    );
                }
            }
        }
    }
    let input = temp.path().join("cases.json");
    let output = temp.path().join("output.json");
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    run(Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg("FarmAnimalVariantOracle")
        .arg(input)
        .arg(&output));
    let expected: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    let mut provider = ResourceStack::open(&game).unwrap();
    for (name, animal) in [
        ("pig", FarmAnimal::Pig),
        ("cow", FarmAnimal::Cow),
        ("chicken", FarmAnimal::Chicken),
    ] {
        let order: Vec<_> = expected["order"][name]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| id(v.as_str().unwrap()))
            .collect();
        let table = FarmAnimalVariants::load(&mut provider, animal, &order).unwrap();
        for (case, expected) in cases.iter().zip(expected["results"].as_array().unwrap()) {
            if case["animal"] != name {
                continue;
            }
            let algorithm = if case["legacy"].as_bool().unwrap() {
                WorldgenRandom::Legacy
            } else {
                WorldgenRandom::Xoroshiro
            };
            let mut random = RegionRandom::new(case["seed"].as_i64().unwrap(), algorithm, -13, 17);
            let selected = table
                .choose(&id(case["biome"].as_str().unwrap()), &mut random)
                .unwrap()
                .map(|id| format!("{}:{}", id.namespace, id.path));
            assert_eq!(
                selected.as_deref(),
                expected["selected"].as_str(),
                "case {case}"
            );
            assert_eq!(
                random.next_long(),
                expected["next"].as_i64().unwrap(),
                "case {case}"
            );
        }
    }
}
