//! World-seeded density noise and cell-volume checks against Java 26.3.
use native_resources::{
    density::{DensityCompiler, DensityEvaluator, WorldgenRandom},
    packs::{ResourceProvider, ResourceStack},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, process::Command};

struct Data(BTreeMap<String, Vec<u8>>);
impl ResourceProvider for Data {
    fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn read_data(&mut self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(self.0.get(path).cloned())
    }
}
fn run(command: &mut Command) -> String {
    let output = command.output().expect("cannot run Java 25");
    assert!(
        output.status.success(),
        "world density oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn inline_installed(
    value: Value,
    sources: &mut ResourceStack,
    data: &mut Data,
    noise_ids: &mut std::collections::BTreeSet<String>,
) -> Value {
    match value {
        Value::String(name) if name.starts_with("minecraft:") => {
            let path = format!(
                "data/minecraft/worldgen/density_function/{}.json",
                name.strip_prefix("minecraft:").unwrap()
            );
            let bytes = sources.read_builtin_data(&path).unwrap().unwrap();
            let json = serde_json::from_slice(&bytes).unwrap();
            data.0.insert(path, bytes);
            inline_installed(json, sources, data, noise_ids)
        }
        Value::Object(mut object) => {
            for (key, child) in object.iter_mut() {
                if key == "type" {
                    continue;
                }
                if key == "noise" {
                    let name = child.as_str().unwrap().to_owned();
                    noise_ids.insert(name.clone());
                    let path = format!(
                        "data/minecraft/worldgen/noise/{}.json",
                        name.strip_prefix("minecraft:").unwrap()
                    );
                    let bytes = sources.read_builtin_data(&path).unwrap().unwrap();
                    *child = serde_json::from_slice(&bytes).unwrap();
                    data.0.insert(path, bytes);
                } else {
                    *child = inline_installed(child.take(), sources, data, noise_ids);
                }
            }
            Value::Object(object)
        }
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| inline_installed(item, sources, data, noise_ids))
                .collect(),
        ),
        other => other,
    }
}

fn find_spline(value: &Value) -> Option<Value> {
    match value {
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("minecraft:spline") {
                return Some(value.clone());
            }
            object.values().find_map(find_spline)
        }
        Value::Array(items) => items.iter().find_map(find_spline),
        _ => None,
    }
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn seeded_noise_functions_and_interpolation_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("WorldDensityNoiseOracle.java");
    std::fs::write(&source, include_str!("java/WorldDensityNoiseOracle.java")).unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let mut sources = ResourceStack::open(&game).unwrap();
    let mut data = Data(BTreeMap::new());
    let mut fixtures = Vec::new();
    for id in [
        "minecraft:temperature",
        "minecraft:continentalness",
        "minecraft:offset",
        "minecraft:nether/temperature",
        "minecraft:nether/vegetation",
        "example:custom",
    ] {
        let (namespace, path) = id.split_once(':').unwrap();
        let resource_path = format!("data/{namespace}/worldgen/noise/{path}.json");
        let bytes = if namespace == "minecraft" {
            sources.read_builtin_data(&resource_path).unwrap().unwrap()
        } else {
            br#"{"base_octave":-4,"octave_count":4,"amplitude_modifiers":[0,1,0,2]}"#.to_vec()
        };
        let definition: Value = serde_json::from_slice(&bytes).unwrap();
        data.0.insert(resource_path, bytes);
        let gradient = json!({"type":"gradient","axis":"y","from_coordinate":-64,"to_coordinate":320,"from_value":1.5,"to_value":-1.5});
        for variant in 0..8 {
            let mut rust_noise =
                json!({"type":"noise","noise":id,"xz_scale":0.013,"y_scale":0.021});
            if variant == 1 || variant == 3 {
                rust_noise["shift_x"] = gradient.clone();
                rust_noise["shift_z"] = json!(-0.375);
            }
            if variant == 2 || variant == 4 {
                rust_noise["shift_x"] = json!(0.125);
                rust_noise["shift_y"] = gradient.clone();
                rust_noise["shift_z"] = json!(-0.25);
            }
            if variant >= 6 {
                rust_noise["shift_x"] =
                    json!({"type":"noise", "noise":id, "xz_scale":0.25,"y_scale":0.0});
            }
            let mut java_noise = rust_noise.clone();
            java_noise["noise"] = definition.clone();
            if variant >= 6 {
                java_noise["shift_x"]["noise"] = definition.clone();
            }
            let (rust_expression, java_expression) = if (3..=5).contains(&variant) || variant == 7 {
                (
                    json!({"type":"interpolated","input":rust_noise,"cell_size_xz":4,"cell_size_y":8}),
                    json!({"type":"interpolated","input":java_noise,"cell_size_xz":4,"cell_size_y":8}),
                )
            } else {
                (rust_noise, java_noise)
            };
            fixtures.push(json!({"id":id,"rust_expression":rust_expression,"java_expression":java_expression}));
        }
        for kind in ["shift_a", "shift_b", "shift"] {
            let rust_shift = json!({"type":kind,"noise":id});
            let java_shift = json!({"type":kind,"noise":definition});
            fixtures
                .push(json!({"id":id,"rust_expression":rust_shift,"java_expression":java_shift}));
            fixtures.push(json!({"id":id,"rust_expression":{"type":"interpolated","input":rust_shift,"cell_size_xz":4,"cell_size_y":8},"java_expression":{"type":"interpolated","input":java_shift,"cell_size_xz":4,"cell_size_y":8}}));
        }
    }
    for id in [
        "minecraft:overworld/ore_vein/toggle",
        "minecraft:overworld/ore_vein/richness",
        "minecraft:overworld/ore_vein/gap",
        "minecraft:overworld/caves/spaghetti_2d_thickness_modulator",
        "minecraft:shift_x",
        "minecraft:shift_z",
        "minecraft:overworld/temperature",
        "minecraft:overworld/vegetation",
        "minecraft:overworld/continents",
        "minecraft:overworld/erosion",
        "minecraft:overworld/ridges",
        "minecraft:overworld/ridges_folded",
        "minecraft:overworld_large_biomes/temperature",
        "minecraft:overworld_large_biomes/vegetation",
        "minecraft:overworld_large_biomes/continents",
        "minecraft:overworld_large_biomes/erosion",
    ] {
        let mut noise_ids = std::collections::BTreeSet::new();
        let java_expression = inline_installed(json!(id), &mut sources, &mut data, &mut noise_ids);
        assert!(!noise_ids.is_empty(), "fixture {id} must use a named noise");
        let mut noises = serde_json::Map::new();
        for name in &noise_ids {
            let path = format!(
                "data/minecraft/worldgen/noise/{}.json",
                name.strip_prefix("minecraft:").unwrap()
            );
            noises.insert(
                name.clone(),
                serde_json::from_slice(data.0.get(&path).unwrap()).unwrap(),
            );
        }
        fixtures.push(json!({"id":noise_ids.into_iter().next().unwrap(),"noises":noises,"rust_expression":id,"java_expression":java_expression}));
    }
    for id in [
        "overworld/offset",
        "overworld/factor",
        "overworld/jaggedness",
        "overworld_large_biomes/offset",
        "overworld_large_biomes/factor",
        "overworld_large_biomes/jaggedness",
        "overworld_amplified/offset",
        "overworld_amplified/factor",
        "overworld_amplified/jaggedness",
    ] {
        let path = format!("data/minecraft/worldgen/density_function/{id}.json");
        let bytes = sources.read_builtin_data(&path).unwrap().unwrap();
        let document: Value = serde_json::from_slice(&bytes).unwrap();
        let rust_expression = find_spline(&document).expect("missing installed spline");
        let mut noise_ids = std::collections::BTreeSet::new();
        let java_expression = inline_installed(
            rust_expression.clone(),
            &mut sources,
            &mut data,
            &mut noise_ids,
        );
        let mut noises = serde_json::Map::new();
        for name in &noise_ids {
            let path = format!(
                "data/minecraft/worldgen/noise/{}.json",
                name.strip_prefix("minecraft:").unwrap()
            );
            noises.insert(
                name.clone(),
                serde_json::from_slice(data.0.get(&path).unwrap()).unwrap(),
            );
        }
        fixtures.push(json!({"id":noise_ids.into_iter().next().unwrap(),"noises":noises,"rust_expression":rust_expression,"java_expression":java_expression}));
    }
    for id in [
        "minecraft:overworld/offset",
        "minecraft:overworld/factor",
        "minecraft:overworld/jaggedness",
        "minecraft:overworld/depth",
        "minecraft:overworld_large_biomes/offset",
        "minecraft:overworld_large_biomes/factor",
        "minecraft:overworld_large_biomes/jaggedness",
        "minecraft:overworld_large_biomes/depth",
        "minecraft:overworld_amplified/offset",
        "minecraft:overworld_amplified/factor",
        "minecraft:overworld_amplified/jaggedness",
        "minecraft:overworld_amplified/depth",
    ] {
        let mut noise_ids = std::collections::BTreeSet::new();
        let java_expression = inline_installed(json!(id), &mut sources, &mut data, &mut noise_ids);
        let mut noises = serde_json::Map::new();
        for name in &noise_ids {
            let path = format!(
                "data/minecraft/worldgen/noise/{}.json",
                name.strip_prefix("minecraft:").unwrap()
            );
            noises.insert(
                name.clone(),
                serde_json::from_slice(data.0.get(&path).unwrap()).unwrap(),
            );
        }
        fixtures.push(json!({"id":noise_ids.into_iter().next().unwrap(),"noises":noises,"unblended":true,"rust_expression":id,"java_expression":java_expression}));
    }
    let input = temp.path().join("fixtures.json");
    std::fs::write(&input, serde_json::to_vec(&fixtures).unwrap()).unwrap();
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("WorldDensityNoiseOracle")
        .arg(input));
    let mut compiled = None;
    let mut evaluator = DensityEvaluator::default();
    let mut groups = 0;
    let mut samples = 0;
    for line in output.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        match fields.first().copied() {
            Some("S") => {
                let index: usize = fields[1].parse().unwrap();
                let mode: i32 = fields[2].parse().unwrap();
                let seed: i64 = fields[3].parse().unwrap();
                let expected_min = fields[4].parse::<i32>().unwrap() as u32;
                let expected_max = fields[5].parse::<i32>().unwrap() as u32;
                let algorithm = if mode == 0 {
                    WorldgenRandom::Legacy
                } else {
                    WorldgenRandom::Xoroshiro
                };
                let mut compiler = DensityCompiler::with_world_seed(&mut data, seed, algorithm);
                if fixtures[index]["unblended"].as_bool() == Some(true) {
                    compiler = compiler.without_old_chunk_blending();
                }
                let function = compiler
                    .compile(&fixtures[index]["rust_expression"])
                    .unwrap();
                let (min, max) = function.range();
                assert_eq!(
                    (min.to_bits(), max.to_bits()),
                    (expected_min, expected_max),
                    "range fixture {index}"
                );
                compiled = Some((index, mode, seed, function));
                groups += 1;
            }
            Some("P") => {
                let x = fields[1].parse::<i32>().unwrap();
                let y = fields[2].parse::<i32>().unwrap();
                let z = fields[3].parse::<i32>().unwrap();
                let expected = fields[4].parse::<i32>().unwrap() as u32;
                let (index, mode, seed, function) = compiled.as_ref().unwrap();
                assert_eq!(
                    function.sample(x, y, z).to_bits(),
                    expected,
                    "fixture {index} mode {mode} seed {seed} at {x} {y} {z}: {}",
                    fixtures[*index]["rust_expression"]
                );
                assert_eq!(
                    evaluator.sample(function, x, y, z).to_bits(),
                    expected,
                    "reused evaluator fixture {index}"
                );
                samples += 1;
            }
            _ => {}
        }
    }
    assert_eq!(groups, fixtures.len() * 10);
    assert_eq!(samples, groups * 12);
}
