//! Compare End biome selection, boundary thresholds and coordinates with Java.
use anyhow::Result;
use native_resources::{
    biome_source::BiomeSource,
    density::DensityEvaluator,
    noise_router::NoiseRouter,
    packs::{ResourceId, ResourceProvider},
};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};
struct Data;
impl ResourceProvider for Data {
    fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn list_data(&mut self, _: &str) -> Result<Vec<ResourceId>> {
        [
            "the_end",
            "end_highlands",
            "end_midlands",
            "small_end_islands",
            "end_barrens",
        ]
        .into_iter()
        .map(ResourceId::parse)
        .collect()
    }
}
fn run(command: &mut Command) -> String {
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "Java End oracle failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn end_biome_source_matches_java() {
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
    let source = temp.path().join("EndBiomeSourceOracle.java");
    std::fs::write(&source, include_str!("java/EndBiomeSourceOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let next_up = |x: f32| {
        f32::from_bits(if x < 0.0 {
            x.to_bits() - 1
        } else {
            x.to_bits() + 1
        })
    };
    let next_down = |x: f32| {
        f32::from_bits(if x < 0.0 {
            x.to_bits() + 1
        } else {
            x.to_bits() - 1
        })
    };
    let mut erosion: Vec<Value> = [
        -0.5_f32,
        next_down(-0.21875),
        -0.21875,
        next_up(-0.21875),
        next_down(-0.0625),
        -0.0625,
        next_up(-0.0625),
        next_down(0.25),
        0.25,
        next_up(0.25),
        0.7,
    ]
    .into_iter()
    .map(|x| json!(x))
    .collect();
    for axis in ["x", "y", "z"] {
        erosion.push(json!({"type":"gradient","axis":axis,"from_coordinate":-1200,"to_coordinate":1200,"from_value":-0.5,"to_value":0.5}));
    }
    let mut points = Vec::new();
    for x in [
        -261,
        -260,
        -259,
        -257,
        -256,
        -255,
        -1,
        0,
        1,
        255,
        256,
        257,
        259,
        260,
        261,
        i32::MIN,
        i32::MAX,
    ] {
        for z in [-261, -257, -256, -1, 0, 1, 255, 256, 257, 261] {
            points.push([x, 13, z]);
        }
    }
    let mut state = 519_751_307_u64;
    for _ in 0..1200 {
        let point = [0, 1, 2].map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((state >> 32) as i32) % 100_000
        });
        points.push(point);
    }
    let input = temp.path().join("cases.json");
    std::fs::write(
        &input,
        serde_json::to_vec(&json!({"erosion":erosion,"points":points})).unwrap(),
    )
    .unwrap();
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("EndBiomeSourceOracle")
        .arg(&input));
    let mut results = Vec::<Vec<String>>::new();
    for line in output.lines() {
        if line == "S" {
            results.push(Vec::new());
        } else if let Some(value) = line.strip_prefix("P ") {
            results.last_mut().unwrap().push(value.to_string());
        }
    }
    assert_eq!(results.len(), erosion.len());
    let mut data = Data;
    let mut source = BiomeSource::load(&mut data, &json!({"type":"minecraft:the_end"})).unwrap();
    assert!(source.select([0.0; 6]).is_err());
    let mut evaluator = DensityEvaluator::default();
    for (i, expression) in erosion.iter().enumerate() {
        let settings = json!({"noise":{"min_y":0,"height":128},"legacy_random_source":false,"noise_router":{
            "final_density":0,"temperature":0,"vegetation":0,"continents":0,"erosion":expression,"depth":0,"ridges":0,"chunk_surface_level":0}});
        let router = NoiseRouter::from_json_without_terrain_adjustments(
            &mut data,
            &serde_json::to_vec(&settings).unwrap(),
            123,
        )
        .unwrap();
        assert_eq!(results[i].len(), points.len());
        for (index, point) in points.iter().enumerate() {
            let id = source.sample_quart(&router, &mut evaluator, *point);
            assert_eq!(
                format!("{}:{}", id.namespace, id.path),
                results[i][index],
                "erosion {i} point {point:?}"
            );
        }
    }
}
