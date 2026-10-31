//! Exercise the actual 26.3 End dimension and its taller dimension bounds.
use anyhow::{Context, Result};
use native_resources::{
    biome_source::BiomeSource,
    density::DensityEvaluator,
    noise_router::NoiseRouter,
    packs::{ResourceProvider, ResourceStack},
    surface_pass::SurfaceBlockKind,
};
use native_session::worldgen::TerrainGenerator;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};

#[test]
#[ignore = "requires Minecraft Java Edition 26.3"]
fn end_dimension_biomes_and_surface_use_full_height() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let mut resources = ResourceStack::open(&game).unwrap();
    let preset: Value = serde_json::from_slice(
        &resources
            .read_builtin_data("data/minecraft/worldgen/world_preset/normal.json")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let dimension: Value = serde_json::from_slice(
        &resources
            .read_builtin_data("data/minecraft/dimension_type/the_end.json")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let min = dimension["min_y"].as_i64().unwrap() as i32;
    let height = dimension["height"].as_i64().unwrap() as i32;
    assert_eq!((min, height), (0, 256));
    let definition = &preset["dimensions"]["minecraft:the_end"]["generator"];
    let mut generator = TerrainGenerator::without_terrain_adjustments(
        &mut resources,
        definition,
        2345,
        min,
        height,
    )
    .unwrap();
    let router =
        NoiseRouter::load_without_terrain_adjustments(&mut resources, "minecraft:end", 2345)
            .unwrap();
    assert_eq!((router.min_y, router.height), (0, 128));
    let mut source =
        BiomeSource::load(&mut resources, &json!({"type":"minecraft:the_end"})).unwrap();
    let mut evaluator = DensityEvaluator::default();
    for chunk in [[0, 0], [64, 0], [65, 0], [128, -77]] {
        let generated = generator.generate_biomes(chunk, &mut evaluator).unwrap();
        assert_eq!(generated.values().len(), 16 * 64);
        for y in 0..64 {
            for (x, z) in [(0, 0), (3, 0), (0, 3), (3, 3)] {
                let expected = source.sample_quart(
                    &router,
                    &mut evaluator,
                    [chunk[0] * 4 + x, y, chunk[1] * 4 + z],
                );
                let index = (z as usize * 4 + x as usize) * 64 + y as usize;
                assert_eq!(
                    &generated.values()[index],
                    expected,
                    "chunk {chunk:?} y {y}"
                );
            }
        }
    }
    let mut neighbors = generator.biome_neighborhood().unwrap();
    for z in -1..=1 {
        for x in -1..=1 {
            neighbors
                .insert(Arc::new(
                    generator.generate_biomes([x, z], &mut evaluator).unwrap(),
                ))
                .unwrap();
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
    let surface = generator
        .generate_surface(&mut resources, &mut evaluator, [0, 0], &neighbors, classify)
        .unwrap();
    assert_eq!((surface.min_y, surface.height), (0, 256));
    for column in surface.states.chunks_exact(256) {
        assert!(
            column[128..]
                .iter()
                .all(|block| block.as_str() == Some("minecraft:air"))
        );
    }
}
