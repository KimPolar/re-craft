//! World-seeded density noise and cell-volume checks against Java 26.3.
use native_resources::packs::ResourceId;
use native_resources::{
    density::{DensityCompiler, DensityEvaluator, WorldgenRandom},
    noise_router::{NoiseRouter, RouterField},
    noise_stage::{NoiseMaterial, NoiseStageGenerator, select_without_aquifer},
    packs::{ResourceProvider, ResourceStack},
    surface_rules::{
        SurfaceDensitySampler, SurfaceInputs, SurfacePosition, SurfaceRandom, SurfaceRandomFactory,
        SurfaceRuleCompiler,
    },
};
use serde_json::{Value, json};
use std::sync::Arc;
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
struct OreInputs<'a> {
    density: SurfaceDensitySampler<'a, Data>,
    random: SurfaceRandomFactory,
}
impl SurfaceInputs for OreInputs<'_> {
    fn biome_matches(&mut self, _: &Value, _: &SurfacePosition) -> anyhow::Result<bool> {
        anyhow::bail!("unexpected biome")
    }
    fn cold_enough_to_snow(&mut self, _: &SurfacePosition) -> anyhow::Result<bool> {
        anyhow::bail!("unexpected temperature")
    }
    fn noise(&mut self, _: &ResourceId, _: bool, _: &SurfacePosition) -> anyhow::Result<f64> {
        anyhow::bail!("unexpected noise")
    }
    fn positional_random(&mut self, _: &ResourceId, _: &SurfacePosition) -> anyhow::Result<f32> {
        anyhow::bail!("unexpected gradient random")
    }
    fn ore_random(&mut self, p: &SurfacePosition) -> anyhow::Result<SurfaceRandom> {
        Ok(self
            .random
            .at(&ResourceId::parse("minecraft:ore")?, p.x, p.y, p.z))
    }
    fn density(&mut self, v: &Value, in_chunk: bool, p: &SurfacePosition) -> anyhow::Result<f32> {
        self.density.sample(v, in_chunk, p.x, p.y, p.z)
    }
    fn special_rule(
        &mut self,
        _: &str,
        _: &Value,
        _: &SurfacePosition,
    ) -> anyhow::Result<Option<Arc<Value>>> {
        anyhow::bail!("unexpected special rule")
    }
}
fn run(command: &mut Command) -> String {
    let output = command.output().expect("cannot run Java 25");
    assert!(
        output.status.success(),
        "world density oracle failed:\n{}",
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
            let mut noise_name = None;
            for (key, child) in object.iter_mut() {
                if key == "type" {
                    continue;
                }
                if key == "noise" {
                    let name = child.as_str().unwrap().to_owned();
                    noise_name = Some(name.clone());
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
            if let Some(name) = noise_name {
                object.insert("_oracle_noise_id".into(), Value::String(name));
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
        for (xz_scale, y_scale, shifted) in [
            (0.0, 0.0, false),
            (0.0, 0.021, false),
            (0.013, 0.0, false),
            (0.0, 0.0, true),
        ] {
            let mut rust_noise =
                json!({"type":"noise","noise":id,"xz_scale":xz_scale,"y_scale":y_scale});
            if shifted {
                rust_noise["shift_x"] = gradient.clone();
            }
            let mut java_noise = rust_noise.clone();
            java_noise["noise"] = definition.clone();
            fixtures.push(json!({"id":id,"rust_expression":{"type":"interpolated","input":rust_noise,"cell_size_xz":4,"cell_size_y":8},"java_expression":{"type":"interpolated","input":java_noise,"cell_size_xz":4,"cell_size_y":8}}));
        }
        for axis in ["x", "y", "z"] {
            let rust_slice = json!({"type":"slice", "axis":axis, "coordinate":13, "input":{"type":"noise","noise":id,"xz_scale":0.013,"y_scale":0.021}});
            let java_slice = json!({"type":"slice", "axis":axis, "coordinate":13, "input":{"type":"noise","noise":definition,"xz_scale":0.013,"y_scale":0.021}});
            fixtures
                .push(json!({"id":id,"rust_expression":rust_slice,"java_expression":java_slice}));
            fixtures.push(json!({"id":id,"rust_expression":{"type":"interpolated","input":rust_slice,"cell_size_xz":4,"cell_size_y":8},"java_expression":{"type":"interpolated","input":java_slice,"cell_size_xz":4,"cell_size_y":8}}));
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
        "minecraft:overworld/caves/spaghetti_2d",
        "minecraft:overworld/caves/entrances",
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
        "minecraft:overworld/sloped_cheese",
        "minecraft:overworld/preliminary_surface_level",
        "minecraft:overworld/chunk_surface_level",
        "minecraft:overworld/final_density",
        "minecraft:overworld_large_biomes/offset",
        "minecraft:overworld_large_biomes/factor",
        "minecraft:overworld_large_biomes/jaggedness",
        "minecraft:overworld_large_biomes/depth",
        "minecraft:overworld_large_biomes/sloped_cheese",
        "minecraft:overworld_large_biomes/preliminary_surface_level",
        "minecraft:overworld_large_biomes/chunk_surface_level",
        "minecraft:overworld_large_biomes/final_density",
        "minecraft:overworld_amplified/offset",
        "minecraft:overworld_amplified/factor",
        "minecraft:overworld_amplified/jaggedness",
        "minecraft:overworld_amplified/depth",
        "minecraft:overworld_amplified/sloped_cheese",
        "minecraft:overworld_amplified/preliminary_surface_level",
        "minecraft:overworld_amplified/chunk_surface_level",
        "minecraft:overworld_amplified/final_density",
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
        fixtures.push(json!({"id":noise_ids.into_iter().next().unwrap(),"noises":noises,"unblended":true,"no_structures":true,"rust_expression":id,"java_expression":java_expression}));
    }
    let mut noise_ids = std::collections::BTreeSet::new();
    let java_islands = inline_installed(
        json!("minecraft:end/islands"),
        &mut sources,
        &mut data,
        &mut noise_ids,
    );
    assert!(noise_ids.is_empty());
    let outer = json!({"type":"end_outer_islands"});
    fixtures.push(json!({"id":"minecraft:offset","end_fixture":true,"rust_expression":outer,"java_expression":outer}));
    fixtures.push(json!({"id":"minecraft:offset","end_fixture":true,"rust_expression":"minecraft:end/islands","java_expression":java_islands}));
    fixtures.push(json!({"id":"minecraft:offset","end_fixture":true,"rust_expression":{"type":"interpolated","input":outer,"cell_size_xz":4,"cell_size_y":8},"java_expression":{"type":"interpolated","input":outer,"cell_size_xz":4,"cell_size_y":8}}));
    for id in [
        "minecraft:overworld/base_3d_noise",
        "minecraft:nether/base_3d_noise",
        "minecraft:end/base_3d_noise",
        "minecraft:end/sloped_cheese",
    ] {
        let mut names = std::collections::BTreeSet::new();
        let java = inline_installed(json!(id), &mut sources, &mut data, &mut names);
        assert!(names.is_empty());
        fixtures
            .push(json!({"id":"minecraft:terrain","rust_expression":id,"java_expression":java}));
        fixtures.push(json!({"id":"minecraft:terrain","rust_expression":{"type":"interpolated","input":id,"cell_size_xz":4,"cell_size_y":8},"java_expression":{"type":"interpolated","input":java,"cell_size_xz":4,"cell_size_y":8}}));
    }
    for params in [
        [0.001, 0.001, 1000.0, 1000.0, 1.0],
        [1.3, 0.7, 2.5, 3.0, 4.0],
        [1000.0, 1000.0, 0.001, 0.001, 8.0],
    ] {
        let value = json!({"type":"old_blended_noise","xz_scale":params[0],"y_scale":params[1],"xz_factor":params[2],"y_factor":params[3],"smear_scale_multiplier":params[4]});
        fixtures.push(
            json!({"id":"minecraft:terrain","rust_expression":value,"java_expression":value}),
        );
    }
    let gx = json!({"type":"gradient","axis":"x","from_coordinate":-32,"to_coordinate":32,"from_value":-1.0,"to_value":1.0});
    let gy = json!({"type":"gradient","axis":"y","from_coordinate":-64,"to_coordinate":64,"from_value":1.0,"to_value":-1.0});
    let gz = json!({"type":"gradient","axis":"z","from_coordinate":-32,"to_coordinate":32,"from_value":0.5,"to_value":-0.5});
    let shape = json!({"type":"add","left":{"type":"mul","left":gx,"right":gy},"right":gz});
    for (min, size, xz, y) in [
        ([-8, -8, 0], [9, 9, 5], 4, 8),
        ([-17, -5, 31], [5, 9, 5], 3, 5),
        ([0, 0, 0], [1, 1, 1], 4, 8),
        ([2, 3, 5], [1, 1, 1], 4, 8),
        ([0, 0, 0], [4, 8, 4], 1, 1),
        ([i32::MIN, -5, i32::MAX - 8], [5, 9, 5], 3, 5),
        ([0, i32::MIN, 0], [3, 6, 3], 3, 5),
    ] {
        let expr = json!({"type":"interpolated","input":shape,"cell_size_xz":xz,"cell_size_y":y});
        fixtures.push(json!({"id":"minecraft:terrain","rust_expression":expr,"java_expression":expr,"volume_min":min,"volume_size":size}));
    }
    for name in ["overworld", "nether", "end"] {
        let path = format!("data/minecraft/worldgen/noise_settings/{name}.json");
        let bytes = sources.read_builtin_data(&path).unwrap().unwrap();
        let settings: Value = serde_json::from_slice(&bytes).unwrap();
        data.0.insert(path, bytes);
        let mode = if settings["legacy_random_source"].as_bool().unwrap() {
            0
        } else {
            1
        };
        for field in RouterField::ALL {
            let expression = settings["noise_router"][field.name()].clone();
            let mut noise_ids = std::collections::BTreeSet::new();
            let java =
                inline_installed(expression.clone(), &mut sources, &mut data, &mut noise_ids);
            let mut noises = serde_json::Map::new();
            for id in &noise_ids {
                let path = format!(
                    "data/minecraft/worldgen/noise/{}.json",
                    id.strip_prefix("minecraft:").unwrap()
                );
                noises.insert(
                    id.clone(),
                    serde_json::from_slice(data.0.get(&path).unwrap()).unwrap(),
                );
            }
            let mut fixture = json!({"id":"minecraft:terrain","noises":noises,"router_settings":format!("minecraft:{name}"),"router_field":field.name(),"router_mode":mode,"rust_expression":expression,"java_expression":java});
            if field.name() == "final_density" {
                fixture["volume_min"] = json!([-17, -5, 31]);
                fixture["volume_size"] = json!([5, 9, 5]);
                fixture["chunk_min_y"] = settings["noise"]["min_y"].clone();
                fixture["chunk_height"] = settings["noise"]["height"].clone();
                if name != "overworld" {
                    fixture["simple_noise"] = json!(name);
                } else {
                    let mut aquifers = serde_json::Map::new();
                    for (key, expression) in settings["aquifers"].as_object().unwrap() {
                        aquifers.insert(
                            key.clone(),
                            inline_installed(
                                expression.clone(),
                                &mut sources,
                                &mut data,
                                &mut noise_ids,
                            ),
                        );
                    }
                    let mut noises = serde_json::Map::new();
                    for id in &noise_ids {
                        let path = format!(
                            "data/minecraft/worldgen/noise/{}.json",
                            id.strip_prefix("minecraft:").unwrap()
                        );
                        noises.insert(
                            id.clone(),
                            serde_json::from_slice(data.0.get(&path).unwrap()).unwrap(),
                        );
                    }
                    fixture["noises"] = Value::Object(noises);
                    fixture["java_aquifers"] = Value::Object(aquifers);
                }
            }
            fixtures.push(fixture);
        }
    }
    for ore in ["copper", "iron"] {
        let path = format!("data/minecraft/worldgen/material_rule/overworld/{ore}_ore_vein.json");
        let original: Value =
            serde_json::from_slice(&sources.read_builtin_data(&path).unwrap().unwrap()).unwrap();
        let mut names = std::collections::BTreeSet::new();
        let mut functions = Vec::new();
        for field in ["density", "richness", "filler_gap"] {
            functions.push(inline_installed(
                original[field].clone(),
                &mut sources,
                &mut data,
                &mut names,
            ));
        }
        let mut noises = serde_json::Map::new();
        for id in &names {
            let path = format!(
                "data/minecraft/worldgen/noise/{}.json",
                id.strip_prefix("minecraft:").unwrap()
            );
            noises.insert(
                id.clone(),
                serde_json::from_slice(data.0.get(&path).unwrap()).unwrap(),
            );
        }
        let mins = if ore == "copper" {
            json!({"0":[-1024,0,-704],"2345":[-1024,0,592]})
        } else {
            json!({"0":[-1008,-60,-928],"2345":[-1024,-60,-880]})
        };
        fixtures.push(json!({"id":"minecraft:terrain","noises":noises,"router_mode":1,
            "rust_expression":original["density"],"java_expression":functions[0],
            "java_ore_functions":functions,"rust_ore_rule":original,"ore_min_by_seed":mins,"ore_size":[16,64,16],
            "surface_density":true,"volume_min":[-17,-60,31],"volume_size":[5,9,5]}));
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
    let mut end_samples = 0;
    let mut volume_samples = 0;
    let mut volume_values = None;
    let mut chunk_values = None;
    let mut chunk_index = 0;
    let mut chunk_samples = 0;
    let mut material_chunk = None;
    let mut material_index = 0;
    let mut material_samples = 0;
    let mut fluid_cases = 0;
    let mut ore_values = None;
    let mut ore_index = 0;
    let mut ore_samples = 0;
    let mut ore_nonempty = 0;
    let mut ore_hits = BTreeMap::new();
    let mut surface_density_values = None;
    let mut surface_density_samples = 0;
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
                let function = if let Some(name) = fixtures[index]["router_settings"].as_str() {
                    let router =
                        NoiseRouter::load_without_terrain_adjustments(&mut data, name, seed)
                            .unwrap();
                    assert_eq!(router.random, algorithm);
                    let field = RouterField::ALL
                        .into_iter()
                        .find(|field| {
                            field.name() == fixtures[index]["router_field"].as_str().unwrap()
                        })
                        .unwrap();
                    let all = router.sample_all(&mut evaluator, -17, -64, 49);
                    assert_eq!(
                        all[field as usize].to_bits(),
                        router.sample(&mut evaluator, field, -17, -64, 49).to_bits(),
                        "shared router sampling {name} {}",
                        field.name()
                    );
                    std::sync::Arc::clone(router.function(field))
                } else {
                    let mut compiler = DensityCompiler::with_world_seed(&mut data, seed, algorithm);
                    if fixtures[index]["unblended"].as_bool() == Some(true) {
                        compiler = compiler.without_old_chunk_blending();
                    }
                    if fixtures[index]["no_structures"].as_bool() == Some(true) {
                        compiler = compiler.without_structure_terrain_adjustment();
                    }
                    compiler
                        .compile(&fixtures[index]["rust_expression"])
                        .unwrap()
                };
                let (min, max) = function.range();
                assert_eq!(
                    (min.to_bits(), max.to_bits()),
                    (expected_min, expected_max),
                    "range fixture {index}"
                );
                compiled = Some((index, mode, seed, function));
                volume_values = None;
                chunk_values = None;
                chunk_index = 0;
                material_chunk = None;
                material_index = 0;
                ore_values = None;
                ore_index = 0;
                surface_density_values = None;
                groups += 1;
            }
            Some("P") | Some("Q") => {
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
                if fields[0] == "P" {
                    samples += 1;
                } else {
                    end_samples += 1;
                }
            }
            Some("V") => {
                let (index, mode, seed, function) = compiled.as_ref().unwrap();
                let min =
                    [0, 1, 2].map(|i| fixtures[*index]["volume_min"][i].as_i64().unwrap() as i32);
                let size =
                    [0, 1, 2].map(|i| fixtures[*index]["volume_size"][i].as_u64().unwrap() as u32);
                let values = volume_values.get_or_insert_with(|| {
                    evaluator.sample_block_volume(function, min, size).unwrap()
                });
                let x: usize = fields[1].parse().unwrap();
                let y: usize = fields[2].parse().unwrap();
                let z: usize = fields[3].parse().unwrap();
                let expected = fields[4].parse::<i32>().unwrap() as u32;
                assert_eq!(
                    values[(z * size[0] as usize + x) * size[1] as usize + y].to_bits(),
                    expected,
                    "volume fixture {index} mode {mode} seed {seed} at {x},{y},{z}"
                );
                if fixtures[*index].get("surface_density").is_some() {
                    let surface = surface_density_values.get_or_insert_with(|| {
                        let algorithm = if *mode == 0 {
                            WorldgenRandom::Legacy
                        } else {
                            WorldgenRandom::Xoroshiro
                        };
                        let mut density = SurfaceDensitySampler::without_terrain_adjustments(
                            &mut data, *seed, algorithm, min, size,
                        )
                        .unwrap();
                        let mut values = Vec::new();
                        for z in 0..size[2] {
                            for x in 0..size[0] {
                                for y in 0..size[1] {
                                    values.push(
                                        density
                                            .sample(
                                                &fixtures[*index]["rust_expression"],
                                                true,
                                                min[0] + x as i32,
                                                min[1] + y as i32,
                                                min[2] + z as i32,
                                            )
                                            .unwrap(),
                                    );
                                }
                            }
                        }
                        values
                    });
                    assert_eq!(
                        surface[(z * size[0] as usize + x) * size[1] as usize + y].to_bits(),
                        expected,
                        "material density fixture {index} at {x},{y},{z}"
                    );
                    surface_density_samples += 1;
                }
                volume_samples += 1;
            }
            Some("C") => {
                let (index, mode, seed, _) = compiled.as_ref().unwrap();
                let name = fixtures[*index]["router_settings"].as_str().unwrap();
                let chunk = chunk_values.get_or_insert_with(|| {
                    let router =
                        NoiseRouter::load_without_terrain_adjustments(&mut data, name, *seed)
                            .unwrap();
                    router
                        .sample_chunk_density(&mut evaluator, 90, -60)
                        .unwrap()
                });
                assert_eq!(
                    chunk.values()[chunk_index].to_bits(),
                    fields[1].parse::<i32>().unwrap() as u32,
                    "chunk {name} mode {mode} seed {seed} index {chunk_index}"
                );
                chunk_index += 1;
                chunk_samples += 1;
            }
            Some("F") => {
                let sea: i32 = fields[1].parse().unwrap();
                let y: i32 = fields[2].parse().unwrap();
                let density = f32::from_bits(fields[3].parse::<i32>().unwrap() as u32);
                let expected: i32 = fields[4].parse().unwrap();
                let actual = match select_without_aquifer(density, y, sea) {
                    NoiseMaterial::DefaultBlock => 0,
                    NoiseMaterial::DefaultFluid => 1,
                    NoiseMaterial::Lava => 2,
                    NoiseMaterial::Air => 3,
                };
                assert_eq!(actual, expected, "sea {sea} y {y} density {density}");
                fluid_cases += 1;
            }
            Some("M") => {
                let (index, mode, seed, _) = compiled.as_ref().unwrap();
                let name = fixtures[*index]["router_settings"].as_str().unwrap();
                let chunk = material_chunk.get_or_insert_with(|| {
                    let generator = NoiseStageGenerator::load_without_terrain_adjustments(
                        &mut data, name, *seed,
                    )
                    .unwrap();
                    generator.generate(&mut evaluator, 90, -60).unwrap()
                });
                let actual = match chunk.materials()[material_index] {
                    NoiseMaterial::DefaultBlock => 0,
                    NoiseMaterial::DefaultFluid => {
                        if name == "minecraft:the_end" || name == "minecraft:end" {
                            3
                        } else if name == "minecraft:overworld" {
                            1
                        } else {
                            2
                        }
                    }
                    NoiseMaterial::Lava => 2,
                    NoiseMaterial::Air => 3,
                };
                assert_eq!(
                    actual,
                    fields[1].parse::<i32>().unwrap(),
                    "material {name} mode {mode} seed {seed} index {material_index}"
                );
                assert_eq!(
                    chunk.fluid_updates()[material_index],
                    fields[2] == "1",
                    "fluid update {name} index {material_index}"
                );
                material_index += 1;
                material_samples += 1;
            }
            Some("O") => {
                let (index, mode, seed, _) = compiled.as_ref().unwrap();
                let fixture = &fixtures[*index];
                let values = ore_values.get_or_insert_with(|| {
                    let mut compiler = SurfaceRuleCompiler::new(&mut data);
                    let rule = compiler.compile(&fixture["rust_ore_rule"]).unwrap();
                    drop(compiler);
                    let min = [0, 1, 2].map(|i| {
                        fixture["ore_min_by_seed"][seed.to_string()][i]
                            .as_i64()
                            .unwrap() as i32
                    });
                    let size = [0, 1, 2].map(|i| fixture["ore_size"][i].as_u64().unwrap() as u32);
                    let algorithm = if *mode == 0 {
                        WorldgenRandom::Legacy
                    } else {
                        WorldgenRandom::Xoroshiro
                    };
                    let density = SurfaceDensitySampler::without_terrain_adjustments(
                        &mut data, *seed, algorithm, min, size,
                    )
                    .unwrap();
                    let mut inputs = OreInputs {
                        density,
                        random: SurfaceRandomFactory::new(*seed, algorithm),
                    };
                    let mut result = Vec::new();
                    for z in 0..size[2] {
                        for x in 0..size[0] {
                            for y in 0..size[1] {
                                let p = SurfacePosition {
                                    x: min[0] + x as i32,
                                    y: min[1] + y as i32,
                                    z: min[2] + z as i32,
                                    min_y: -64,
                                    height: 384,
                                    sea_level: 63,
                                    stone_above: 0,
                                    stone_below: 0,
                                    surface_depth: 0,
                                    surface_secondary: 0.0,
                                    water_height: i32::MIN,
                                    min_surface_level: 0,
                                    gradient_x: 0,
                                    gradient_z: 0,
                                };
                                let block = rule.evaluate(&p, &mut inputs).unwrap();
                                let code = match block.as_deref() {
                                    None => 0,
                                    Some(v) if *v == fixture["rust_ore_rule"]["ore_block"] => 1,
                                    Some(v) if *v == fixture["rust_ore_rule"]["raw_ore_block"] => 2,
                                    Some(v) if *v == fixture["rust_ore_rule"]["filler_block"] => 3,
                                    _ => panic!("unexpected ore block"),
                                };
                                result.push(code);
                            }
                        }
                    }
                    result
                });
                let actual = values[ore_index];
                let expected: i32 = fields[1].parse().unwrap();
                assert_eq!(
                    actual, expected,
                    "ore fixture {index} seed {seed} index {ore_index}"
                );
                ore_nonempty += usize::from(actual != 0);
                *ore_hits.entry((*index, *seed)).or_insert(0usize) += usize::from(actual != 0);
                ore_index += 1;
                ore_samples += 1;
            }
            _ => {}
        }
    }
    assert_eq!(
        groups,
        fixtures
            .iter()
            .map(|fixture| if fixture.get("router_mode").is_some() {
                5
            } else {
                10
            })
            .sum::<usize>()
    );
    assert_eq!(samples, groups * 24);
    assert_eq!(end_samples, 3 * 10 * 80);
    let expected_volume_samples: usize = fixtures
        .iter()
        .filter_map(|fixture| {
            let size = fixture.get("volume_size")?.as_array()?;
            let groups = if fixture.get("router_mode").is_some() {
                5
            } else {
                10
            };
            Some(
                groups
                    * size
                        .iter()
                        .map(|n| n.as_u64().unwrap() as usize)
                        .product::<usize>(),
            )
        })
        .sum();
    assert_eq!(volume_samples, expected_volume_samples);
    assert_eq!(chunk_samples, 16 * 16 * (384 + 128 + 128));
    assert_eq!(fluid_cases, 7 * 16 * 7);
    assert_eq!(material_samples, 16 * 16 * (384 + 2 * 128));
    assert_eq!(ore_samples, 2 * 2 * 16 * 64 * 16);
    assert_eq!(surface_density_samples, 2 * 5 * 5 * 9 * 5);
    assert!(ore_nonempty > 0, "ore fixtures must exercise filled veins");
    assert_eq!(ore_hits.len(), 4);
    assert!(
        ore_hits.values().all(|hits| *hits > 0),
        "every ore and seed must exercise filled veins: {ore_hits:?}"
    );
}
