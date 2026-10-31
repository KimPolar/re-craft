//! Check real overworld chunk-biome integration against Java 26.3.
use anyhow::{Context, Result};
use native_resources::{
    biome_source::ClimateVolume,
    density::DensityEvaluator,
    noise_router::NoiseRouter,
    packs::{ResourceProvider, ResourceStack},
    surface_pass::SurfaceBlockKind,
};
use native_session::worldgen::TerrainGenerator;
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command, sync::Arc};
fn run(command: &mut Command) -> String {
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "Java biome oracle failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn real_overworld_biome_chunks_and_surface_pipeline() {
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
    for library in manifest["libraries"].as_array().unwrap() {
        if let Some(p) = library["downloads"]["artifact"]["path"].as_str() {
            let p = game.join("libraries").join(p);
            if p.is_file() {
                paths.push(p);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("BiomeSourceOracle.java");
    std::fs::write(
        &source,
        include_str!("../../resources/tests/java/BiomeSourceOracle.java"),
    )
    .unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let mut resources = ResourceStack::open(&game).unwrap();
    let preset: Value = serde_json::from_slice(
        &resources
            .read_builtin_data("data/minecraft/worldgen/world_preset/normal.json")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let definition = &preset["dimensions"]["minecraft:overworld"]["generator"];
    let seed = 2345;
    let mut generator =
        TerrainGenerator::without_terrain_adjustments(&mut resources, definition, seed, -64, 384)
            .unwrap();
    let router =
        NoiseRouter::load_without_terrain_adjustments(&mut resources, "minecraft:overworld", seed)
            .unwrap();
    let mut evaluator = DensityEvaluator::default();
    let mut targets = Vec::new();
    let mut expected = Vec::new();
    let mut center = None;
    for chunk in [[-17, 32], [0, 0]] {
        let generated = Arc::new(generator.generate_biomes(chunk, &mut evaluator).unwrap());
        let min = [chunk[0] * 4, -16, chunk[1] * 4];
        let volume = ClimateVolume::sample(&router, &mut evaluator, min, [4, 96, 4]).unwrap();
        for section in 0..24 {
            for x in 0..4 {
                for y in 0..4 {
                    for z in 0..4 {
                        let py = section * 4 + y;
                        targets.push(volume.get([min[0] + x, min[1] + py, min[2] + z]).unwrap());
                        let index = (z as usize * 4 + x as usize) * 96 + py as usize;
                        let id = &generated.values()[index];
                        expected.push(format!("{}:{}", id.namespace, id.path));
                    }
                }
            }
        }
        if chunk == [0, 0] {
            center = Some(generated);
        }
    }
    let input = temp.path().join("biomes.json");
    std::fs::write(
        &input,
        serde_json::to_vec(&json!({"groups":[{"overworld":true}],"targets":targets})).unwrap(),
    )
    .unwrap();
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("BiomeSourceOracle")
        .arg(&input));
    let actual: Vec<_> = output
        .lines()
        .filter_map(|line| line.strip_prefix("P "))
        .collect();
    assert_eq!(actual.len(), 2 * 16 * 96);
    assert_eq!(
        actual,
        expected.iter().map(String::as_str).collect::<Vec<_>>()
    );
    let mut neighbors = generator.biome_neighborhood().unwrap();
    neighbors.insert(center.unwrap()).unwrap();
    for z in -1..=1 {
        for x in -1..=1 {
            if [x, z] != [0, 0] {
                neighbors
                    .insert(Arc::new(
                        generator.generate_biomes([x, z], &mut evaluator).unwrap(),
                    ))
                    .unwrap();
            }
        }
    }
    let classify = |state: &Value| -> Result<SurfaceBlockKind> {
        let id = state
            .as_str()
            .or_else(|| state.get("id").and_then(Value::as_str))
            .context("block has no id")?;
        Ok(match id {
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" => SurfaceBlockKind::Air,
            "minecraft:water" | "minecraft:lava" => SurfaceBlockKind::Fluid,
            _ => SurfaceBlockKind::Solid,
        })
    };
    let result = generator
        .generate_surface(&mut resources, &mut evaluator, [0, 0], &neighbors, classify)
        .unwrap();
    assert_eq!(result.states.len(), 16 * 16 * 384);
    assert!(
        result
            .states
            .iter()
            .any(|s| s.as_str() != Some("minecraft:air"))
    );
}
