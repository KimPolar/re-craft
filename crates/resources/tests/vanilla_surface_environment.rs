//! Exercise the complete seeded inputs with the installed 26.3 data registry.
use native_resources::{
    default_minecraft_dir,
    density::WorldgenRandom,
    packs::{ResourceId, ResourceStack},
    surface_rules::{SeededSurfaceInputs, SurfacePosition, SurfaceRuleCompiler},
};
use serde_json::json;
use std::path::PathBuf;

#[test]
#[ignore = "requires Minecraft Java Edition 26.3"]
fn seeded_context_loads_vanilla_rules_and_evaluates_biomes() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| default_minecraft_dir().unwrap());
    let mut resources = ResourceStack::open(&game).unwrap();
    let mut compiler = SurfaceRuleCompiler::new(&mut resources);
    let overworld = compiler.compile(&json!("minecraft:overworld")).unwrap();
    let test_rule = compiler
        .compile(&json!({
            "type":"condition",
            "if_true":{"type":"biome","biome_is":["minecraft:snowy_plains"]},
            "then_run":{"type":"condition","if_true":{"type":"temperature"},
                        "then_run":{"type":"block","result_state":"minecraft:snow_block"}}
        }))
        .unwrap();
    drop(compiler);
    // Actual vanilla selectors and all seeded dependencies must load together.
    drop(
        SeededSurfaceInputs::without_terrain_adjustments(
            &mut resources,
            &overworld,
            42,
            WorldgenRandom::Xoroshiro,
            [0, -64, 0],
            [16, 384, 16],
        )
        .unwrap(),
    );
    let mut inputs = SeededSurfaceInputs::without_terrain_adjustments(
        &mut resources,
        &test_rule,
        42,
        WorldgenRandom::Xoroshiro,
        [0, -64, 0],
        [16, 384, 16],
    )
    .unwrap();
    let p = SurfacePosition {
        x: 0,
        y: 63,
        z: 0,
        min_y: -64,
        height: 384,
        sea_level: 63,
        stone_above: 1,
        stone_below: 1,
        surface_depth: 0,
        surface_secondary: 0.0,
        water_height: i32::MIN,
        min_surface_level: 0,
        gradient_x: 0,
        gradient_z: 0,
    };
    assert!(test_rule.evaluate(&p, &mut inputs).is_err());
    inputs
        .set_biome(ResourceId::parse("minecraft:snowy_plains").unwrap())
        .unwrap();
    assert!(test_rule.evaluate(&p, &mut inputs).unwrap().is_some());
    inputs
        .set_biome(ResourceId::parse("minecraft:plains").unwrap())
        .unwrap();
    assert!(test_rule.evaluate(&p, &mut inputs).unwrap().is_none());
    assert!(
        inputs
            .set_biome(ResourceId::parse("example:missing").unwrap())
            .is_err()
    );
}
