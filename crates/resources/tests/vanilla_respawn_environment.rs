//! Compare respawn environment layers against the installed 26.3 codecs.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use native_resources::{
    packs::{ResourceProvider, ResourceStack},
    respawn_environment::{
        BedCondition, BedRule, RespawnEnvironment, RespawnEnvironmentLayer,
        RespawnEnvironmentRegistry,
    },
};
use serde_json::{Value, json};

fn classpath(game: &Path, classes: &Path) -> OsString {
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    let mut paths = vec![version.join("26.3.jar"), classes.to_path_buf()];
    for library in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    std::env::join_paths(paths).unwrap()
}

fn run(command: &mut Command) {
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "Java environment oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

fn relevant(value: &Value) -> Value {
    let mut result = serde_json::Map::new();
    if let Some(attributes) = value.get("attributes").and_then(Value::as_object) {
        for (name, entry) in attributes {
            if matches!(
                name.as_str(),
                "minecraft:gameplay/bed_rule"
                    | "minecraft:gameplay/straw_bed_rule"
                    | "minecraft:gameplay/respawn_anchor_works"
            ) {
                result.insert(name.clone(), entry.clone());
            }
        }
    }
    Value::Object(result)
}

fn bed_json(bed: BedRule) -> Value {
    let condition = |value| match value {
        BedCondition::Always => "always",
        BedCondition::WhenDark => "when_dark",
        BedCondition::Never => "never",
    };
    json!({"can_sleep": condition(bed.can_sleep), "can_set_spawn": condition(bed.can_set_spawn),
        "destroy_on_use": bed.destroy_on_use, "destroy_on_leave": bed.destroy_on_leave})
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn respawn_environment_layers_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let mut resources = ResourceStack::open(&game).unwrap();
    let mut definitions = |folder: &str| -> Vec<Value> {
        resources
            .list_data(folder)
            .unwrap()
            .into_iter()
            .map(|id| {
                let path = format!("data/{}/{folder}/{}.json", id.namespace, id.path);
                let value: Value =
                    serde_json::from_slice(&resources.read_data(&path).unwrap().unwrap()).unwrap();
                relevant(&value)
            })
            .collect()
    };
    let dimensions = definitions("dimension_type");
    let biomes = definitions("worldgen/biome");
    assert!(!dimensions.is_empty() && !biomes.is_empty());
    let mut cases = vec![vec![]];
    cases.push(vec![json!({"minecraft:gameplay/bed_rule": {
        "can_sleep":"always", "can_set_spawn":"never", "modifier":"unrelated"
    }})]);
    for dimension in &dimensions {
        for biome in &biomes {
            cases.push(vec![dimension.clone(), biome.clone()]);
        }
    }
    for base in [false, true] {
        for modifier in ["override", "and", "nand", "or", "nor", "xor", "xnor"] {
            for argument in [false, true] {
                cases.push(vec![json!({"minecraft:gameplay/respawn_anchor_works":base}),
                    json!({"minecraft:gameplay/respawn_anchor_works":{"modifier":modifier,"argument":argument}})]);
            }
        }
    }
    for sleep in ["always", "when_dark", "never"] {
        for spawn in ["always", "when_dark", "never"] {
            for destroy in [false, true] {
                let rule = json!({"can_sleep":sleep,"can_set_spawn":spawn,"destroy_on_use":destroy,"destroy_on_leave":!destroy});
                cases.push(vec![json!({"minecraft:gameplay/bed_rule":rule,
                    "minecraft:gameplay/straw_bed_rule":{"modifier":"override","argument":{"can_sleep":spawn,"can_set_spawn":sleep}}})]);
            }
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("RespawnEnvironmentOracle.java");
    let input = temp.path().join("cases.json");
    let output = temp.path().join("output.json");
    std::fs::write(&source, include_str!("java/RespawnEnvironmentOracle.java")).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java"))
        .arg("-cp")
        .arg(&cp)
        .arg("RespawnEnvironmentOracle")
        .arg(&input)
        .arg(&output));
    let expected: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(cases.len(), expected.len());
    for (layers, expected) in cases.iter().zip(expected) {
        let mut actual = RespawnEnvironment::default();
        for layer in layers {
            actual.apply(RespawnEnvironmentLayer::from_attributes(layer).unwrap());
        }
        assert_eq!(
            json!({"bed":bed_json(actual.bed),"straw_bed":bed_json(actual.straw_bed),
            "anchor":actual.anchor_works,"fast_lava":false}),
            expected,
            "layers {layers:?}"
        );
    }
    eprintln!("compared {} environment layer cases", cases.len());
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn dimension_fast_lava_matches_java_attribute_codec() {
    use native_resources::fluid_environment::fast_lava_from_attributes;
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let mut resources = ResourceStack::open(&game).unwrap();
    let mut cases = vec![
        json!({}),
        json!({"minecraft:gameplay/fast_lava":true}),
        json!({"minecraft:gameplay/fast_lava":false}),
    ];
    for id in resources.list_data("dimension_type").unwrap() {
        let path = format!("data/{}/dimension_type/{}.json", id.namespace, id.path);
        let value: Value =
            serde_json::from_slice(&resources.read_data(&path).unwrap().unwrap()).unwrap();
        let mut attributes = serde_json::Map::new();
        if let Some(fast) = value
            .get("attributes")
            .and_then(|v| v.get("minecraft:gameplay/fast_lava"))
        {
            attributes.insert("minecraft:gameplay/fast_lava".into(), fast.clone());
        }
        let type_name = format!("{}:{}", id.namespace, id.path);
        assert_eq!(
            native_resources::fluid_environment::load_fast_lava(&mut resources, &type_name)
                .unwrap(),
            fast_lava_from_attributes(&Value::Object(attributes.clone())).unwrap(),
            "{type_name}"
        );
        cases.push(Value::Object(attributes));
    }
    for modifier in ["override", "and", "nand", "or", "nor", "xor", "xnor"] {
        for argument in [false, true] {
            cases.push(
                json!({"minecraft:gameplay/fast_lava":{"modifier":modifier,"argument":argument}}),
            );
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("RespawnEnvironmentOracle.java");
    let input = temp.path().join("cases.json");
    let output = temp.path().join("output.json");
    std::fs::write(&source, include_str!("java/RespawnEnvironmentOracle.java")).unwrap();
    std::fs::write(
        &input,
        serde_json::to_vec(&cases.iter().map(|v| vec![v]).collect::<Vec<_>>()).unwrap(),
    )
    .unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java"))
        .arg("-cp")
        .arg(&cp)
        .arg("RespawnEnvironmentOracle")
        .arg(&input)
        .arg(&output));
    let expected: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(cases.len(), expected.len());
    for (attributes, expected) in cases.iter().zip(expected) {
        assert_eq!(
            fast_lava_from_attributes(attributes).unwrap(),
            expected["fast_lava"].as_bool().unwrap(),
            "{attributes}"
        );
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn fast_lava_timeline_matches_java_track_sampler() {
    use native_resources::{fluid_environment::load_fast_lava_environment, packs::ResourceId};
    struct Data {
        dimension: Value,
        timeline: Value,
    }
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
            Ok(match path {
                "data/example/dimension_type/test.json" => {
                    Some(serde_json::to_vec(&self.dimension)?)
                }
                "data/example/timeline/test.json" => Some(serde_json::to_vec(&self.timeline)?),
                _ => None,
            })
        }
    }
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let ticks = [
        -41,
        -20,
        -19,
        -1,
        0,
        1,
        4,
        5,
        6,
        8,
        9,
        10,
        19,
        20,
        21,
        39,
        40,
        i64::MAX,
        i64::MIN,
    ];
    let mut cases = Vec::new();
    for base in [false, true] {
        for ease in ["linear", "constant"] {
            for modifier in ["override", "and", "nand", "or", "nor", "xor", "xnor"] {
                for looping in [false, true] {
                    let track = json!({"modifier":modifier,"ease":ease,"keyframes":[
                        {"ticks":0,"value":false},{"ticks":5,"value":true},
                        {"ticks":5,"value":false},{"ticks":9,"value":true}]});
                    let mut case = json!({"base":base,"track":track,"ticks":ticks});
                    if looping {
                        case["period"] = json!(20);
                    }
                    cases.push(case);
                }
            }
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("FastLavaTimelineOracle.java");
    let input = temp.path().join("cases.json");
    let output = temp.path().join("output.json");
    std::fs::write(&source, include_str!("java/FastLavaTimelineOracle.java")).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java"))
        .arg("-cp")
        .arg(&cp)
        .arg("FastLavaTimelineOracle")
        .arg(&input)
        .arg(&output));
    let expected: Vec<Vec<bool>> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(expected.len(), cases.len());
    for (case, expected) in cases.iter().zip(expected) {
        let mut timeline = json!({"clock":"example:clock","tracks":{"minecraft:gameplay/fast_lava":case["track"]}});
        if let Some(period) = case.get("period") {
            timeline["period_ticks"] = period.clone();
        }
        let mut provider = Data {
            dimension: json!({"attributes":{"gameplay/fast_lava":case["base"]},"timelines":["example:test"]}),
            timeline,
        };
        let environment = load_fast_lava_environment(&mut provider, "example:test").unwrap();
        for (tick, expected) in ticks.into_iter().zip(expected) {
            assert_eq!(
                environment.sample(|id| {
                    assert_eq!(*id, ResourceId::parse("example:clock").unwrap());
                    Some(tick)
                }),
                Some(expected),
                "{case} at {tick}"
            );
        }
    }
}

#[test]
#[ignore = "requires Minecraft 26.3"]
fn respawn_environment_uses_active_data_pack_priority() {
    use native_resources::datapacks::DataPackSettings;
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let mut resources = ResourceStack::open(&game).unwrap();
    let dimension_path = "data/minecraft/dimension_type/overworld.json";
    let biome_path = "data/minecraft/worldgen/biome/plains.json";
    let mut dimension: Value =
        serde_json::from_slice(&resources.read_data(dimension_path).unwrap().unwrap()).unwrap();
    let mut biome: Value =
        serde_json::from_slice(&resources.read_data(biome_path).unwrap().unwrap()).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let world = temp.path().join("world");
    for (name, anchor) in [("low", false), ("high", true)] {
        let pack = world.join("datapacks").join(name);
        let dim_file = pack.join(dimension_path);
        let biome_file = pack.join(biome_path);
        std::fs::create_dir_all(dim_file.parent().unwrap()).unwrap();
        std::fs::create_dir_all(biome_file.parent().unwrap()).unwrap();
        std::fs::write(
            pack.join("pack.mcmeta"),
            br#"{"pack":{"description":"test","min_format":121,"max_format":121}}"#,
        )
        .unwrap();
        dimension["attributes"]["minecraft:gameplay/respawn_anchor_works"] = json!(anchor);
        dimension["attributes"]["minecraft:gameplay/fast_lava"] = json!(anchor);
        biome["attributes"]["minecraft:gameplay/bed_rule"] =
            json!({"can_sleep":"never","can_set_spawn":if anchor {"never"} else {"always"}});
        std::fs::write(dim_file, serde_json::to_vec(&dimension).unwrap()).unwrap();
        std::fs::write(biome_file, serde_json::to_vec(&biome).unwrap()).unwrap();
        let unused = pack.join("data/example/timeline/unused.json");
        std::fs::create_dir_all(unused.parent().unwrap()).unwrap();
        std::fs::write(unused, br#"{"tracks":{"minecraft:gameplay/fast_lava":{}}}"#).unwrap();
    }
    let settings = |high: bool| DataPackSettings {
        enabled: if high {
            vec!["vanilla".into(), "file/low".into(), "file/high".into()]
        } else {
            vec!["vanilla".into(), "file/low".into()]
        },
        disabled: if high {
            vec![]
        } else {
            vec!["file/high".into()]
        },
        enabled_features: vec!["minecraft:vanilla".into()],
    };
    for high in [false, true] {
        assert!(
            resources
                .configure_world_data(&world, &settings(high))
                .unwrap()
                .is_empty()
        );
        let mut environment = RespawnEnvironment::default();
        environment.apply(
            RespawnEnvironmentLayer::load_dimension_type(&mut resources, "minecraft:overworld")
                .unwrap(),
        );
        environment.apply(
            RespawnEnvironmentLayer::load_biome(&mut resources, "minecraft:plains").unwrap(),
        );
        assert_eq!(environment.anchor_works, high);
        assert_eq!(
            native_resources::fluid_environment::load_fast_lava(
                &mut resources,
                "minecraft:overworld"
            )
            .unwrap(),
            high
        );
        assert_eq!(
            environment.bed.can_set_spawn,
            if high {
                BedCondition::Never
            } else {
                BedCondition::Always
            }
        );
        assert!(RespawnEnvironmentLayer::load_biome(&mut resources, "example:missing").is_err());
    }
}

#[test]
#[ignore = "requires Minecraft 26.3"]
fn fast_lava_checks_only_timelines_linked_by_active_tag() {
    use native_resources::{
        datapacks::DataPackSettings,
        fluid_environment::{load_fast_lava, load_fast_lava_environment},
    };
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let temp = tempfile::tempdir().unwrap();
    let world = temp.path().join("world");
    let pack = world.join("datapacks/custom");
    let timeline = pack.join("data/example/timeline/dynamic.json");
    std::fs::create_dir_all(timeline.parent().unwrap()).unwrap();
    std::fs::write(
        &timeline,
        br#"{"clock":"minecraft:overworld","period_ticks":20,"tracks":{"minecraft:gameplay/fast_lava":{"keyframes":[{"ticks":0,"value":false},{"ticks":5,"value":true}]}}}"#,
    )
    .unwrap();
    std::fs::write(
        pack.join("pack.mcmeta"),
        br#"{"pack":{"description":"test","min_format":121,"max_format":121}}"#,
    )
    .unwrap();
    let tag = pack.join("data/minecraft/tags/timeline/in_overworld.json");
    std::fs::create_dir_all(tag.parent().unwrap()).unwrap();
    let settings = DataPackSettings {
        enabled: vec!["vanilla".into(), "file/custom".into()],
        disabled: vec![],
        enabled_features: vec!["minecraft:vanilla".into()],
    };
    for linked in [false, true] {
        let values = if linked {
            json!(["example:dynamic"])
        } else {
            json!([])
        };
        std::fs::write(
            &tag,
            serde_json::to_vec(&json!({"replace":true,"values":values})).unwrap(),
        )
        .unwrap();
        let mut resources = ResourceStack::open(&game).unwrap();
        assert!(
            resources
                .configure_world_data(&world, &settings)
                .unwrap()
                .is_empty()
        );
        let fast = load_fast_lava(&mut resources, "minecraft:overworld");
        if linked {
            assert!(fast.is_err());
            let environment =
                load_fast_lava_environment(&mut resources, "minecraft:overworld").unwrap();
            assert_eq!(environment.sample(|_| Some(0)), Some(false));
            assert_eq!(environment.sample(|_| Some(5)), Some(true));
        } else {
            assert!(!fast.unwrap());
        }
    }
}

#[test]
#[ignore = "requires Minecraft 26.3"]
fn runtime_respawn_environment_uses_biomes_and_rejects_timeline_changes() {
    use native_resources::datapacks::DataPackSettings;
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let mut resources = ResourceStack::open(&game).unwrap();
    let registry = RespawnEnvironmentRegistry::load(&mut resources, "minecraft:overworld").unwrap();
    let plains = registry.for_biome("minecraft:plains").unwrap();
    assert_eq!(plains.bed.can_set_spawn, BedCondition::Always);
    assert!(!plains.anchor_works);
    assert!(registry.for_biome("example:missing").is_err());

    let temp = tempfile::tempdir().unwrap();
    let pack = temp.path().join("datapacks/timeline_override");
    let path = pack.join("data/minecraft/timeline/day.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        pack.join("pack.mcmeta"),
        br#"{"pack":{"description":"test","min_format":121,"max_format":121}}"#,
    )
    .unwrap();
    let mut timeline: Value = serde_json::from_slice(
        &resources
            .read_data("data/minecraft/timeline/day.json")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    timeline["tracks"]["minecraft:gameplay/respawn_anchor_works"] =
        json!({"keyframes":[{"ticks":0,"value":true}]});
    std::fs::write(path, serde_json::to_vec(&timeline).unwrap()).unwrap();
    resources
        .configure_world_data(
            temp.path(),
            &DataPackSettings {
                enabled: vec!["vanilla".into(), "file/timeline_override".into()],
                disabled: vec![],
                enabled_features: vec!["minecraft:vanilla".into()],
            },
        )
        .unwrap();
    assert!(RespawnEnvironmentRegistry::load(&mut resources, "minecraft:overworld").is_err());
}
