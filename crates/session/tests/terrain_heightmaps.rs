//! World-generation heightmaps from real 26.3 terrain before decoration.
use std::{collections::BTreeMap, path::PathBuf, process::Command, sync::Arc};

use anyhow::{Context, Result};
use native_resources::{
    carver::{schedule::CarverCache, tags::CarverBlockTags},
    density::DensityEvaluator,
    heightmap_tags::HeightmapTags,
    packs::{ResourceProvider, ResourceStack},
};
use native_session::worldgen::{
    CarvedTerrainWorld, CarverBlockState, TerrainGenerator, materialize_carved_terrain,
    resolve_generated_terrain_state,
};
use native_world::loaded::ChunkPos;
use serde_json::Value;

fn run(command: &mut Command) -> String {
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn terrain_stage_heightmaps_match_java_before_features() -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir()?;
    let version = game.join("versions/26.3");
    let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
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
    let cp = std::env::join_paths(paths)?;
    let source = temp.path().join("TerrainChunkOracle.java");
    std::fs::write(&source, include_str!("java/TerrainChunkOracle.java"))?;
    run(Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(source));
    let mut resources = ResourceStack::open(&game)?;
    let blocks = resources.vanilla_block_ids()?;
    let tags = HeightmapTags::load(&mut resources, &blocks)?;
    let mut flat_tags = String::new();
    for block in &blocks {
        let flags = tags.for_block(block)?;
        flat_tags.push_str(&format!(
            "{block} {} {}\n",
            u8::from(flags.blocks_motion),
            u8::from(flags.blocks_motion_no_leaves)
        ));
    }
    let tag_file = temp.path().join("heightmap-tags.txt");
    std::fs::write(&tag_file, flat_tags)?;
    let output = run(Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg("-Drecraft.heightmaps=true")
        .arg(format!("-Drecraft.heightmapTags={}", tag_file.display()))
        .arg("TerrainChunkOracle")
        .args([
            "overworld",
            "2345",
            "0",
            "0",
            "overworld",
            "12345",
            "-100",
            "17",
            "nether",
            "42",
            "0",
            "0",
            "end",
            "42",
            "0",
            "0",
        ]));
    let mut stages = Vec::<BTreeMap<String, Vec<i32>>>::new();
    for line in output.lines() {
        if line.starts_with("S ") {
            stages.push(BTreeMap::new());
        }
        if let Some(line) = line.strip_prefix("H ") {
            let mut words = line.split_whitespace();
            let name = words.next().unwrap().to_owned();
            let values = words.map(|word| word.parse().unwrap()).collect();
            assert!(stages.last_mut().unwrap().insert(name, values).is_none());
        }
    }
    assert_eq!(stages.len(), 12);
    let preset: Value = serde_json::from_slice(
        &resources
            .read_builtin_data("data/minecraft/worldgen/world_preset/normal.json")?
            .context("missing normal preset")?,
    )?;
    let registry = resources.vanilla_generated_block_registry()?;
    let carver_tags = CarverBlockTags::load(&mut resources, &blocks)?;
    for (index, (dimension, seed, pos, min_y, height)) in [
        ("minecraft:overworld", 2345, [0, 0], -64, 384),
        ("minecraft:overworld", 12345, [-100, 17], -64, 384),
        ("minecraft:the_nether", 42, [0, 0], 0, 256),
        ("minecraft:the_end", 42, [0, 0], 0, 256),
    ]
    .into_iter()
    .enumerate()
    {
        let mut generator = TerrainGenerator::without_terrain_adjustments(
            &mut resources,
            &preset["dimensions"][dimension]["generator"],
            seed,
            min_y,
            height,
        )?;
        let mut evaluator = DensityEvaluator::default();
        let mut neighborhood = generator.biome_neighborhood()?;
        for x in pos[0] - 1..=pos[0] + 1 {
            for z in pos[1] - 1..=pos[1] + 1 {
                neighborhood
                    .insert(Arc::new(generator.generate_biomes([x, z], &mut evaluator)?))?;
            }
        }
        let chunk = generator.generate_carved_surface(
            &mut resources,
            &mut evaluator,
            &mut CarverCache::default(),
            pos,
            &neighborhood,
            &carver_tags,
            |value| {
                let state = resolve_generated_terrain_state(value, &registry)?;
                let is_air = matches!(
                    state.name.as_str(),
                    "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                );
                let has_fluid = native_world::fluid::FluidState::from_block(&state)
                    .map_err(anyhow::Error::msg)?
                    .is_some();
                Ok(CarverBlockState { is_air, has_fluid })
            },
        )?;
        let materialized = materialize_carved_terrain(
            chunk,
            neighborhood.chunk(pos).context("missing biome")?,
            dimension,
            |value| resolve_generated_terrain_state(value, &registry),
        )?;
        let mut terrain = CarvedTerrainWorld::new(materialized);
        let chunk_pos = ChunkPos {
            x: pos[0],
            z: pos[1],
        };
        let start = std::time::Instant::now();
        terrain.initialize_worldgen_heightmaps(chunk_pos, &tags)?;
        eprintln!("heightmap scan {dimension} {pos:?}: {:?}", start.elapsed());
        let (surface, ocean) = terrain
            .worldgen_heightmaps(chunk_pos)
            .context("missing maps")?;
        let expected = &stages[index * 3 + 2];
        assert_eq!(
            Some(surface.as_slice()),
            expected.get("WORLD_SURFACE_WG").map(Vec::as_slice),
            "{dimension} {seed} {pos:?}"
        );
        assert_eq!(
            Some(ocean.as_slice()),
            expected.get("OCEAN_FLOOR_WG").map(Vec::as_slice),
            "{dimension} {seed} {pos:?}"
        );
    }
    Ok(())
}
