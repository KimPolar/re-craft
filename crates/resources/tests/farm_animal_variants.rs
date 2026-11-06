use std::{collections::BTreeMap, path::PathBuf, process::Command};

use anyhow::Result;
use native_resources::{
    animal_variants::{
        FarmAnimal, FarmAnimalSpawnVariants, FarmAnimalVariants, ZombieNautilusSpawnVariants,
    },
    density::WorldgenRandom,
    packs::{ResourceId, ResourceProvider, ResourceStack},
    worldgen_region_random::{AnimalGroupData, RegionRandom},
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

struct UnorderedRegistry(Vec<ResourceId>);

impl ResourceProvider for UnorderedRegistry {
    fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn list_data(&mut self, _: &str) -> Result<Vec<ResourceId>> {
        Ok(self.0.clone())
    }
}

fn synthetic_registry() -> UnorderedRegistry {
    UnorderedRegistry(
        [
            "z:a",
            "a:b",
            "a:a",
            "z:a-foo",
            "a:a/child",
            "a:a_foo",
            "a:a.foo",
            "a:a-foo",
        ]
        .iter()
        .map(|name| id(name))
        .collect(),
    )
}

#[test]
fn registry_order_sorts_json_path_before_namespace() {
    let mut provider = synthetic_registry();
    let expected: Vec<_> = [
        "a:a-foo",
        "z:a-foo",
        "a:a.foo",
        "a:a",
        "z:a",
        "a:a/child",
        "a:a_foo",
        "a:b",
    ]
    .iter()
    .map(|name| id(name))
    .collect();
    assert_eq!(
        provider.list_registry_data("pig_variant").unwrap(),
        expected
    );
    provider.0.push(id("a:a"));
    assert!(provider.list_registry_data("pig_variant").is_err());
}

#[test]
fn animal_initialization_preserves_random_and_group_on_unknown_biome() {
    let mut data = Data::default();
    data.0.insert(
        "data/minecraft/worldgen/biome/plains.json".into(),
        b"{}".to_vec(),
    );
    data.0.insert(
        "data/minecraft/pig_variant/temperate.json".into(),
        br#"{"spawn_conditions":[{"priority":0}]}"#.to_vec(),
    );
    assert!(FarmAnimalSpawnVariants::load_active(&mut data, FarmAnimal::Pig).is_err());
    data.0.insert(
        "data/minecraft/pig_sound_variant/classic.json".into(),
        b"{}".to_vec(),
    );
    let variants = FarmAnimalSpawnVariants::load_active(&mut data, FarmAnimal::Pig).unwrap();
    let mut random = RegionRandom::new(5, WorldgenRandom::Xoroshiro, 0, 0);
    let mut expected_random = random.clone();
    let mut group = AnimalGroupData::default();
    let expected_group = group;
    assert!(
        variants
            .finalize(&id("desert"), &mut random, &mut group, false)
            .is_err()
    );
    assert_eq!(group, expected_group);
    assert_eq!(random.next_long(), expected_random.next_long());
}

#[test]
fn zombie_nautilus_selection_uses_pack_variants_and_preserves_random_on_error() {
    let mut data = Data::default();
    data.0.insert(
        "data/minecraft/worldgen/biome/plains.json".into(),
        b"{}".to_vec(),
    );
    data.0.insert("data/minecraft/zombie_nautilus_variant/custom.json".into(),
        br#"{"spawn_conditions":[{"priority":1,"condition":{"type":"minecraft:biome","biomes":"minecraft:plains"}}]}"#.to_vec());
    let variants = ZombieNautilusSpawnVariants::load_active(&mut data).unwrap();
    for seed in 0..32 {
        let mut random = RegionRandom::new(seed, WorldgenRandom::Legacy, 0, 0);
        let init = variants
            .finalize_structure_spawn(&id("plains"), &mut random)
            .unwrap();
        assert_eq!(init.variant, id("custom"));
        assert!((2400..=3600).contains(&init.attack_cooldown));
        assert!(!init.animal.set_baby_age);
        let mut before = random.clone();
        assert!(
            variants
                .finalize_structure_spawn(&id("desert"), &mut random)
                .is_err()
        );
        assert_eq!(random.next_long(), before.next_long());
    }
}

#[test]
fn construction_default_prefers_temperate_then_first_registry_entry() {
    let mut data = Data::default();
    data.0.insert(
        "data/minecraft/worldgen/biome/plains.json".into(),
        b"{}".to_vec(),
    );
    data.0.insert(
        "data/minecraft/pig_sound_variant/classic.json".into(),
        b"{}".to_vec(),
    );
    data.0
        .insert("data/minecraft/pig_variant/z.json".into(), b"{}".to_vec());
    data.0
        .insert("data/minecraft/pig_variant/a.json".into(), b"{}".to_vec());
    let fallback = FarmAnimalSpawnVariants::load_active(&mut data, FarmAnimal::Pig).unwrap();
    assert_eq!(fallback.default_variant(), &id("a"));
    data.0.insert(
        "data/minecraft/pig_variant/temperate.json".into(),
        b"{}".to_vec(),
    );
    let temperate = FarmAnimalSpawnVariants::load_active(&mut data, FarmAnimal::Pig).unwrap();
    assert_eq!(temperate.default_variant(), &id("temperate"));
}

#[test]
fn world_conditions_select_animal_and_nautilus_variants_transactionally() {
    use native_resources::animal_variants::{SpawnVariantContext, ZombieNautilusSpawnVariants};
    use std::collections::BTreeSet;
    let mut data = Data::default();
    data.0.insert(
        "data/minecraft/worldgen/biome/ocean.json".into(),
        b"{}".to_vec(),
    );
    data.0.insert(
        "data/minecraft/worldgen/structure/shipwreck.json".into(),
        b"{}".to_vec(),
    );
    data.0.insert(
        "data/minecraft/pig_sound_variant/classic.json".into(),
        b"{}".to_vec(),
    );
    for registry in ["pig_variant", "zombie_nautilus_variant"] {
        for (name, conditions) in [
            ("temperate", json!([{"priority":0}])),
            (
                "moon",
                json!([{"priority":1,"condition":{"type":"minecraft:moon_brightness","range":{"min":0.9}}}]),
            ),
            (
                "ship",
                json!([{"priority":2,"condition":{"type":"minecraft:structure","structures":"minecraft:shipwreck"}}]),
            ),
        ] {
            data.0.insert(
                format!("data/minecraft/{registry}/{name}.json"),
                serde_json::to_vec(&json!({"spawn_conditions":conditions})).unwrap(),
            );
        }
    }
    let animal = FarmAnimalSpawnVariants::load_active(&mut data, FarmAnimal::Pig).unwrap();
    let nautilus = ZombieNautilusSpawnVariants::load_active(&mut data).unwrap();
    let biome = id("ocean");
    let none = BTreeSet::new();
    let ship = BTreeSet::from([id("shipwreck")]);
    for (phase, structures, expected) in [
        (4, &none, "temperate"),
        (0, &none, "moon"),
        (0, &ship, "ship"),
    ] {
        for algorithm in [WorldgenRandom::Legacy, WorldgenRandom::Xoroshiro] {
            let context = SpawnVariantContext {
                biome: Some(&biome),
                moon_phase: Some(phase),
                structures_with_piece: Some(structures),
            };
            let mut random = RegionRandom::new(42, algorithm, 0, 0);
            let mut group = AnimalGroupData::default();
            assert_eq!(
                animal
                    .finalize_in_context(&context, &mut random, &mut group, false)
                    .unwrap()
                    .variant,
                Some(id(expected))
            );
            let mut random = RegionRandom::new(42, algorithm, 0, 0);
            assert_eq!(
                nautilus
                    .finalize_structure_spawn_in_context(&context, &mut random)
                    .unwrap()
                    .variant,
                id(expected)
            );
        }
    }
    let incomplete = SpawnVariantContext {
        biome: Some(&biome),
        moon_phase: None,
        structures_with_piece: None,
    };
    let mut random = RegionRandom::new(42, WorldgenRandom::Legacy, 0, 0);
    let mut original = random.clone();
    let mut group = AnimalGroupData::default();
    let old_group = group;
    assert!(
        animal
            .finalize_in_context(&incomplete, &mut random, &mut group, false)
            .is_err()
    );
    assert_eq!(group, old_group);
    assert_eq!(random.next_long(), original.next_long());
    let mut random = RegionRandom::new(42, WorldgenRandom::Legacy, 0, 0);
    let mut original = random.clone();
    assert!(
        nautilus
            .finalize_structure_spawn_in_context(&incomplete, &mut random)
            .is_err()
    );
    assert_eq!(random.next_long(), original.next_long());
}

#[test]
fn selectors_query_only_conditions_reached_at_the_winning_priority() {
    use native_resources::animal_variants::SpawnVariantEnvironment;
    use std::collections::BTreeSet;
    struct Environment {
        biome: ResourceId,
        structures: BTreeSet<ResourceId>,
        structure_queries: usize,
        moon_queries: usize,
    }
    impl SpawnVariantEnvironment for Environment {
        fn biome(&self) -> Result<&ResourceId> {
            Ok(&self.biome)
        }
        fn moon_phase(&mut self) -> Result<u8> {
            self.moon_queries += 1;
            anyhow::bail!("clock unavailable")
        }
        fn structures_with_piece(&mut self) -> Result<&BTreeSet<ResourceId>> {
            self.structure_queries += 1;
            Ok(&self.structures)
        }
    }
    let mut data = Data::default();
    data.0.insert(
        "data/minecraft/worldgen/biome/ocean.json".into(),
        b"{}".to_vec(),
    );
    data.0.insert(
        "data/minecraft/worldgen/structure/shipwreck.json".into(),
        b"{}".to_vec(),
    );
    data.0.insert(
        "data/minecraft/pig_sound_variant/classic.json".into(),
        b"{}".to_vec(),
    );
    for (name, condition) in [
        (
            "ship",
            json!({"priority":2,"condition":{"type":"minecraft:structure","structures":"minecraft:shipwreck"}}),
        ),
        (
            "moon",
            json!({"priority":1,"condition":{"type":"minecraft:moon_brightness","range":{"min":0.9}}}),
        ),
    ] {
        data.0.insert(
            format!("data/minecraft/pig_variant/{name}.json"),
            serde_json::to_vec(&json!({"spawn_conditions":[condition]})).unwrap(),
        );
    }
    let variants = FarmAnimalSpawnVariants::load_active(&mut data, FarmAnimal::Pig).unwrap();
    let mut env = Environment {
        biome: id("ocean"),
        structures: BTreeSet::from([id("shipwreck")]),
        structure_queries: 0,
        moon_queries: 0,
    };
    let mut random = RegionRandom::new(17, WorldgenRandom::Legacy, 0, 0);
    let mut group = AnimalGroupData::default();
    assert_eq!(
        variants
            .finalize_in_environment(&mut env, &mut random, &mut group, false)
            .unwrap()
            .variant,
        Some(id("ship"))
    );
    assert_eq!((env.structure_queries, env.moon_queries), (1, 0));
    env.structures.clear();
    let mut expected_random = random.clone();
    let expected_group = group;
    assert!(
        variants
            .finalize_in_environment(&mut env, &mut random, &mut group, false)
            .is_err()
    );
    assert_eq!((env.structure_queries, env.moon_queries), (2, 1));
    assert_eq!(group, expected_group);
    assert_eq!(random.next_long(), expected_random.next_long());
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
                    for size in [0, 2] {
                        for existing in [false, true] {
                            cases.push(json!({"animal":animal, "biome":biome, "legacy":legacy, "seed":seed, "size":size, "existing":existing}));
                        }
                    }
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
    let mut synthetic_data = Data::default();
    synthetic_data.0.insert(
        "data/minecraft/worldgen/biome/ocean.json".into(),
        b"{}".to_vec(),
    );
    synthetic_data.0.insert(
        "data/minecraft/worldgen/structure/shipwreck.json".into(),
        b"{}".to_vec(),
    );
    for (name, conditions) in [
        ("temperate", json!([{"priority":0}])),
        (
            "moon",
            json!([{"priority":1,"condition":{"type":"minecraft:moon_brightness","range":{"min":0.9}}}]),
        ),
        (
            "ship",
            json!([{"priority":2,"condition":{"type":"minecraft:structure","structures":"minecraft:shipwreck"}}]),
        ),
    ] {
        synthetic_data.0.insert(
            format!("data/minecraft/pig_variant/{name}.json"),
            serde_json::to_vec(&json!({"spawn_conditions":conditions})).unwrap(),
        );
    }
    let table = FarmAnimalVariants::load_active(&mut synthetic_data, FarmAnimal::Pig).unwrap();
    let biome = id("ocean");
    let no_structures = std::collections::BTreeSet::new();
    let shipwreck = std::collections::BTreeSet::from([id("shipwreck")]);
    let environment_cases = expected["environment_cases"].as_array().unwrap();
    assert_eq!(environment_cases.len(), 192);
    for case in environment_cases {
        let structures = if case["inside"].as_bool().unwrap() {
            &shipwreck
        } else {
            &no_structures
        };
        let context = native_resources::animal_variants::SpawnVariantContext {
            biome: Some(&biome),
            moon_phase: Some(case["phase"].as_u64().unwrap() as u8),
            structures_with_piece: Some(structures),
        };
        let algorithm = if case["legacy"].as_bool().unwrap() {
            WorldgenRandom::Legacy
        } else {
            WorldgenRandom::Xoroshiro
        };
        let mut random = RegionRandom::new(case["seed"].as_i64().unwrap(), algorithm, 0, 0);
        assert_eq!(
            table.choose_in_context(&context, &mut random).unwrap(),
            Some(&id(case["selected"].as_str().unwrap()))
        );
        assert_eq!(random.next_long(), case["next"].as_i64().unwrap());
    }
    let mut provider = ResourceStack::open(&game).unwrap();
    let java_order: Vec<_> = expected["synthetic_order"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| id(value.as_str().unwrap()))
        .collect();
    assert_eq!(
        synthetic_registry()
            .list_registry_data("pig_variant")
            .unwrap(),
        java_order
    );
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
        assert_eq!(
            provider
                .list_registry_data(&format!("{name}_variant"))
                .unwrap(),
            order
        );
        let table = FarmAnimalSpawnVariants::load_active(&mut provider, animal).unwrap();
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
            let mut group = AnimalGroupData {
                size: case["size"].as_i64().unwrap() as i32,
                ..Default::default()
            };
            let existing = case["existing"].as_bool().unwrap();
            let actual = table
                .finalize(
                    &id(case["biome"].as_str().unwrap()),
                    &mut random,
                    &mut group,
                    existing,
                )
                .unwrap();
            let selected = actual
                .variant
                .as_ref()
                .map(|id| format!("{}:{}", id.namespace, id.path));
            assert_eq!(
                selected.as_deref(),
                expected["selected"].as_str(),
                "case {case}"
            );
            assert_eq!(
                format!(
                    "{}:{}",
                    actual.sound_variant.namespace, actual.sound_variant.path
                ),
                expected["sound"].as_str().unwrap(),
                "case {case}"
            );
            assert_eq!(
                actual.animal.set_baby_age,
                expected["baby"].as_bool().unwrap(),
                "case {case}"
            );
            assert_eq!(
                actual.animal.left_handed,
                expected["left"].as_bool().unwrap(),
                "case {case}"
            );
            assert_eq!(
                group.size as i64,
                expected["size"].as_i64().unwrap(),
                "case {case}"
            );
            if existing {
                assert_eq!(actual.animal.follow_range_bonus, None);
            } else {
                assert_eq!(
                    actual.animal.follow_range_bonus.unwrap().to_bits() as i64,
                    expected["bonus_bits"].as_i64().unwrap(),
                    "case {case}"
                );
            }
            assert_eq!(
                random.next_long(),
                expected["next"].as_i64().unwrap(),
                "case {case}"
            );
        }
    }
}
