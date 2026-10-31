//! Compare four-block climate volume sampling, including interpolation, to Java.
use native_resources::{
    density::{DensityCompiler, DensityEvaluator, WorldgenRandom},
    noise_router::NoiseRouter,
    packs::{ResourceProvider, ResourceStack},
};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};
fn run(command: &mut Command) -> String {
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "Java density oracle failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn stepped_volumes_match_vanilla() {
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
                paths.push(p);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("SteppedDensityOracle.java");
    std::fs::write(&source, include_str!("java/SteppedDensityOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let mut data = ResourceStack::open(&game).unwrap();
    let noise: Value = serde_json::from_slice(
        &data
            .read_builtin_data("data/minecraft/worldgen/noise/surface.json")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let gradient = |axis| json!({"type":"gradient","axis":axis,"from_coordinate":-32,"to_coordinate":32,"from_value":-0.7,"to_value":0.9});
    let mut rust = Vec::new();
    let mut java_fixtures = Vec::new();
    for y_scale in [0.0, 0.017] {
        let r =
            json!({"type":"noise","noise":"minecraft:surface","xz_scale":0.0375,"y_scale":y_scale});
        let mut j = r.clone();
        j["noise"] = noise.clone();
        for cell in [(4, 8), (8, 4)] {
            let wrap = |v| json!({"type":"interpolated","input":v,"cell_size_xz":cell.0,"cell_size_y":cell.1});
            rust.push(wrap(r.clone()));
            java_fixtures.push(wrap(j.clone()));
        }
        rust.push(r);
        java_fixtures.push(j);
    }
    let polynomial = json!({"type":"mul","left":gradient("x"),"right":{"type":"add","left":gradient("y"),"right":gradient("z")}});
    for input in [polynomial, gradient("y")] {
        let expression =
            json!({"type":"interpolated","input":input,"cell_size_xz":4,"cell_size_y":8});
        rust.push(expression.clone());
        java_fixtures.push(expression);
    }
    let volumes = vec![
        json!({"min":[0,-16,0],"size":[4,5,4],"step":[4,4,4]}),
        json!({"min":[-13,-17,9],"size":[4,5,3],"step":[4,4,4]}),
        json!({"min":[8,0,-12],"size":[4,1,5],"step":[4,1,4]}),
        json!({"min":[-8,-16,16],"size":[3,4,3],"step":[4,8,4]}),
        json!({"min":[1,3,-5],"size":[4,5,3],"step":[2,3,4]}),
        json!({"min":[1,8,-5],"size":[1,4,1],"step":[4,4,4]}),
    ];
    let input = temp.path().join("cases.json");
    std::fs::write(
        &input,
        serde_json::to_vec(&json!({"fixtures":java_fixtures,"volumes":volumes})).unwrap(),
    )
    .unwrap();
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("SteppedDensityOracle")
        .arg(&input));
    let mut expected = Vec::<Vec<u32>>::new();
    for line in output.lines() {
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("S") => expected.push(Vec::new()),
            Some("P") => expected
                .last_mut()
                .unwrap()
                .push(parts.next().unwrap().parse::<i32>().unwrap() as u32),
            _ => {}
        }
    }
    assert_eq!(expected.len(), 2 * rust.len() * volumes.len());
    let mut group = 0;
    for algorithm in [WorldgenRandom::Legacy, WorldgenRandom::Xoroshiro] {
        let mut compiler = DensityCompiler::with_world_seed(&mut data, 2345, algorithm);
        let functions: Vec<_> = rust.iter().map(|v| compiler.compile(v).unwrap()).collect();
        let mut evaluator = DensityEvaluator::default();
        for (fixture, function) in functions.iter().enumerate() {
            for (vi, v) in volumes.iter().enumerate() {
                let min = [0, 1, 2].map(|i| v["min"][i].as_i64().unwrap() as i32);
                let size = [0, 1, 2].map(|i| v["size"][i].as_u64().unwrap() as u32);
                let step = [0, 1, 2].map(|i| v["step"][i].as_i64().unwrap() as i32);
                let actual = evaluator
                    .sample_stepped_volume(function, min, size, step)
                    .unwrap();
                assert_eq!(
                    actual.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                    expected[group],
                    "fixture {fixture} volume {vi} mode {algorithm:?}"
                );
                group += 1;
            }
        }
    }
    for (mode, algorithm) in [WorldgenRandom::Legacy, WorldgenRandom::Xoroshiro]
        .into_iter()
        .enumerate()
    {
        let definition = json!({"noise":{"min_y":-16,"height":32},"legacy_random_source":matches!(algorithm,WorldgenRandom::Legacy),
            "noise_router":{"final_density":0,"temperature":rust[0],"vegetation":rust[1],"continents":rust[2],
                "erosion":rust[3],"depth":rust[4],"ridges":rust[5],"chunk_surface_level":0}});
        let router = NoiseRouter::from_json_without_terrain_adjustments(
            &mut data,
            &serde_json::to_vec(&definition).unwrap(),
            2345,
        )
        .unwrap();
        let mut evaluator = DensityEvaluator::default();
        let actual = router
            .sample_climate_volume(&mut evaluator, [0, -4, 0], [4, 5, 4])
            .unwrap();
        for (index, sample) in actual.iter().enumerate() {
            for field in 0..6 {
                let java_group = mode * rust.len() * volumes.len() + field * volumes.len();
                assert_eq!(
                    sample[field].to_bits(),
                    expected[java_group][index],
                    "climate field {field} index {index}"
                );
            }
        }
    }
}
