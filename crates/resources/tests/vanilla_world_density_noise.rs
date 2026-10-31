//! World-seeded density noise and cell-volume checks against Java 26.3.
use native_resources::{
    density::{DensityCompiler, WorldgenRandom},
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
    }
    let input = temp.path().join("fixtures.json");
    std::fs::write(&input, serde_json::to_vec(&fixtures).unwrap()).unwrap();
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("WorldDensityNoiseOracle")
        .arg(input));
    let mut compiled = None;
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
                samples += 1;
            }
            _ => {}
        }
    }
    assert_eq!(groups, fixtures.len() * 8);
    assert_eq!(samples, groups * 12);
}
