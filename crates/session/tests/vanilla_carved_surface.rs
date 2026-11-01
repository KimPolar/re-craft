//! Compare the wired carver pass and real top-material evaluation with Java.
use anyhow::{Result, ensure};
use native_resources::{
    carver::{CarvingMask, carve_ellipsoid, schedule::CarverCache, tags::CarverBlockTags},
    density::DensityEvaluator,
    packs::{ResourceId, ResourceProvider, ResourceStack},
    surface_pass::{SurfaceBlockKind, SurfaceStageChunk},
};
use native_session::worldgen::{CarverBlockState, TerrainGenerator};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::PathBuf, process::Command, sync::Arc};
struct Data;
impl ResourceProvider for Data {
    fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        Ok(if path == "data/minecraft/worldgen/biome/plains.json" {
            Some(br#"{"has_precipitation":true,"temperature":0.8,"carvers":[]}"#.to_vec())
        } else if path.starts_with("data/minecraft/worldgen/noise/") {
            Some(br#"{"base_octave":0,"octave_count":1,"amplitude_modifiers":[0]}"#.to_vec())
        } else if path == "data/minecraft/tags/block/uncarvable.json" {
            Some(br#"{"values":["minecraft:bedrock"]}"#.to_vec())
        } else {
            None
        })
    }
    fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
        ensure!(folder == "worldgen/biome", "unexpected registry");
        Ok(vec![ResourceId::parse("minecraft:plains")?])
    }
}
fn run(command: &mut Command) -> String {
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "Java carved surface oracle failed: {}\n{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout[out.stdout.len().saturating_sub(2500)..])
    );
    String::from_utf8(out.stdout).unwrap()
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn aquifer_and_surface_repair_match_java() {
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
    let source = temp.path().join("CarvedSurfaceOracle.java");
    std::fs::write(&source, include_str!("java/CarvedSurfaceOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("CarvedSurfaceOracle"));
    let rule = json!({"type":"sequence","sequence":[
      {"type":"condition","if_true":{"type":"not","invert":{"type":"water","offset":0,"surface_depth_multiplier":0,"add_stone_depth":false}},"then_run":{"type":"block","result_state":"minecraft:clay"}},
      {"type":"condition","if_true":{"type":"steep"},"then_run":{"type":"block","result_state":"minecraft:podzol"}},
      {"type":"condition","if_true":{"type":"above_preliminary_surface"},"then_run":{"type":"block","result_state":"minecraft:grass_block"}},
      {"type":"block","result_state":"minecraft:stone"}]});
    let definition = json!({"type":"noise","biome_source":{"type":"fixed","biome":"minecraft:plains"},"settings":{
      "noise":{"min_y":0,"height":16},"legacy_random_source":false,"sea_level":8,"default_block":"minecraft:stone","default_fluid":"minecraft:water","material_rule":rule,
      "noise_router":{"final_density":1,"temperature":0,"vegetation":0,"continents":0,"erosion":0,"depth":0,"ridges":0,"chunk_surface_level":12}}});
    let registry: BTreeSet<_> = [
        "air",
        "stone",
        "water",
        "lava",
        "bedrock",
        "dirt",
        "grass_block",
        "clay",
        "podzol",
    ]
    .into_iter()
    .map(|s| format!("minecraft:{s}"))
    .collect();
    let mut data = Data;
    let tags = CarverBlockTags::load(&mut data, &registry).unwrap();
    let mut generator =
        TerrainGenerator::without_terrain_adjustments(&mut data, &definition, 42, 0, 16).unwrap();
    let mut chunk = SurfaceStageChunk {
        chunk_position: [0, 0],
        min_y: 0,
        height: 16,
        states: vec![Arc::new(json!("minecraft:air")); 4096],
        fluid_updates: vec![false; 4096],
    };
    let idx = |x: usize, y: usize, z: usize| (z * 16 + x) * 16 + y;
    for z in 0..16 {
        for x in 0..16 {
            for y in 0..=3 {
                chunk.states[idx(x, y, z)] = Arc::new(json!("minecraft:stone"));
            }
        }
    }
    for y in 4..=15 {
        chunk.states[idx(3, y, 4)] = Arc::new(json!("minecraft:stone"));
    }
    let positions = [(4, 12, 4), (6, 10, 4), (4, 7, 6)];
    let mut mask = CarvingMask::new(1, 14).unwrap();
    for (x, y, z) in positions {
        chunk.states[idx(x, y, z)] = Arc::new(json!("minecraft:grass_block"));
        chunk.states[idx(x, y - 1, z)] = Arc::new(json!("minecraft:dirt"));
        carve_ellipsoid(
            &mut mask,
            [0, 0],
            [x as f64 + 0.5, y as f64, z as f64 + 0.5],
            0.5,
            1.0,
            |_, _, _, by| by != y as i32,
        )
        .unwrap();
    }
    generator
        .apply_carving_to_surface(
            &mut data,
            &mut DensityEvaluator::default(),
            &mut chunk,
            &mask,
            &tags,
            |state| {
                let id = state.as_str().unwrap();
                Ok(CarverBlockState {
                    is_air: id == "minecraft:air",
                    has_fluid: id == "minecraft:water" || id == "minecraft:lava",
                })
            },
        )
        .unwrap();
    let mut expected = Vec::new();
    for (x, y, z) in positions {
        for by in [y, y - 1] {
            let i = idx(x, by, z);
            expected.push(format!(
                "P {x} {by} {z} {} {}",
                chunk.states[i].as_str().unwrap(),
                chunk.fluid_updates[i]
            ));
        }
    }
    let actual: Vec<_> = output
        .lines()
        .filter(|line| line.starts_with("P "))
        .collect();
    assert_eq!(
        actual,
        expected.iter().map(String::as_str).collect::<Vec<_>>()
    );

    // Exercise the wired pass with the installed overworld data and the real
    // complete block registry; this is a smoke test, not a full chunk oracle.
    let registry: BTreeSet<String> = output
        .lines()
        .filter_map(|line| line.strip_prefix("R "))
        .map(str::to_owned)
        .collect();
    assert!(registry.len() > 1000);
    let mut resources = ResourceStack::open(&game).unwrap();
    let preset: Value = serde_json::from_slice(
        &resources
            .read_builtin_data("data/minecraft/worldgen/world_preset/normal.json")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let mut generator = TerrainGenerator::without_terrain_adjustments(
        &mut resources,
        &preset["dimensions"]["minecraft:overworld"]["generator"],
        2345,
        -64,
        384,
    )
    .unwrap();
    let tags = CarverBlockTags::load(&mut resources, &registry).unwrap();
    let mut evaluator = DensityEvaluator::default();
    let mut neighborhood = generator.biome_neighborhood().unwrap();
    for x in -1..=1 {
        for z in -1..=1 {
            neighborhood
                .insert(Arc::new(
                    generator.generate_biomes([x, z], &mut evaluator).unwrap(),
                ))
                .unwrap();
        }
    }
    let classify = |state: &Value| -> Result<CarverBlockState> {
        let id = state
            .as_str()
            .or_else(|| state.get("id").and_then(Value::as_str))
            .ok_or_else(|| anyhow::anyhow!("missing state ID"))?;
        let waterlogged = state
            .get("properties")
            .and_then(|p| p.get("waterlogged"))
            .and_then(Value::as_str)
            == Some("true");
        Ok(CarverBlockState {
            is_air: matches!(
                id,
                "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
            ),
            has_fluid: matches!(id, "minecraft:water" | "minecraft:lava") || waterlogged,
        })
    };
    let mut chunk = generator
        .generate_surface(&mut resources, &mut evaluator, [0, 0], &neighborhood, |s| {
            let f = classify(s)?;
            Ok(if f.is_air {
                SurfaceBlockKind::Air
            } else if f.has_fluid {
                SurfaceBlockKind::Fluid
            } else {
                SurfaceBlockKind::Solid
            })
        })
        .unwrap();
    let before = chunk.states.clone();
    let mut cache = CarverCache::default();
    let mask = generator
        .generate_carving_mask(&mut resources, &mut evaluator, &mut cache, [0, 0])
        .unwrap();
    assert!(!mask.is_empty());
    generator
        .apply_carving_to_surface(
            &mut resources,
            &mut evaluator,
            &mut chunk,
            &mask,
            &tags,
            classify,
        )
        .unwrap();
    assert!(
        before.iter().zip(&chunk.states).any(|(a, b)| a != b),
        "expected carvers to modify the chunk"
    );
}
