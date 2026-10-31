//! Exercise the complete seeded inputs with the installed 26.3 data registry.
use native_resources::{
    default_minecraft_dir,
    density::{DensityEvaluator, WorldgenRandom},
    noise_stage::NoiseStageGenerator,
    packs::{ResourceId, ResourceStack},
    surface_pass::{SurfaceBlockKind, apply_surface},
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
    drop(inputs);
    let settings = json!({"noise":{"min_y":0,"height":16},"legacy_random_source":false,
        "default_block":"minecraft:stone","default_fluid":"minecraft:water","sea_level":8,
        "noise_router":{"final_density":1,"temperature":0,"vegetation":0,"continents":0,
        "erosion":0,"depth":0,"ridges":0,"chunk_surface_level":12}});
    let generator = NoiseStageGenerator::from_json_without_terrain_adjustments(
        &mut resources,
        &serde_json::to_vec(&settings).unwrap(),
        42,
    )
    .unwrap();
    let mut evaluator = DensityEvaluator::default();
    let chunk = generator.generate(&mut evaluator, 0, 0).unwrap();
    let preliminary = generator.preliminary_surface(&mut evaluator, 0, 0).unwrap();
    assert_eq!(preliminary, vec![12.0; 256]);
    let mut inputs = SeededSurfaceInputs::without_terrain_adjustments(
        &mut resources,
        &test_rule,
        42,
        generator.random_algorithm(),
        [0, 0, 0],
        [16, 16, 16],
    )
    .unwrap();
    let classify = |state: &serde_json::Value| -> anyhow::Result<SurfaceBlockKind> {
        Ok(match state.as_str().unwrap() {
            "minecraft:air" => SurfaceBlockKind::Air,
            "minecraft:water" | "minecraft:lava" => SurfaceBlockKind::Fluid,
            _ => SurfaceBlockKind::Solid,
        })
    };
    let generated = apply_surface(
        &chunk,
        &test_rule,
        &mut inputs,
        &preliminary,
        generator.sea_level(),
        classify,
        |_, _, _| ResourceId::parse("minecraft:snowy_plains"),
    )
    .unwrap();
    assert_eq!(generated.states.len(), 16 * 16 * 16);
    assert!(
        generated
            .states
            .iter()
            .all(|state| state.as_str() == Some("minecraft:snow_block"))
    );
    assert!(
        apply_surface(
            &chunk,
            &test_rule,
            &mut inputs,
            &preliminary,
            generator.sea_level(),
            classify,
            |_, _, _| ResourceId::parse("minecraft:eroded_badlands")
        )
        .is_ok()
    );
    assert!(
        apply_surface(
            &chunk,
            &test_rule,
            &mut inputs,
            &preliminary,
            generator.sea_level(),
            classify,
            |_, _, _| ResourceId::parse("minecraft:frozen_ocean")
        )
        .is_err()
    );
    drop(inputs);
    let mut compiler = SurfaceRuleCompiler::new(&mut resources);
    let changing_height = compiler.compile(&json!({"type":"sequence","sequence":[
        {"type":"condition","if_true":{"type":"steep"},"then_run":{"type":"block","result_state":"minecraft:snow_block"}},
        {"type":"block","result_state":"minecraft:air"}
    ]})).unwrap();
    drop(compiler);
    let mut inputs = SeededSurfaceInputs::without_terrain_adjustments(
        &mut resources,
        &changing_height,
        42,
        generator.random_algorithm(),
        [0, 0, 0],
        [16, 16, 16],
    )
    .unwrap();
    let generated = apply_surface(
        &chunk,
        &changing_height,
        &mut inputs,
        &preliminary,
        generator.sea_level(),
        classify,
        |_, _, _| ResourceId::parse("minecraft:plains"),
    )
    .unwrap();
    assert_eq!(generated.states[15].as_str(), Some("minecraft:air"));
    assert_eq!(
        generated.states[16 * 16 + 15].as_str(),
        Some("minecraft:snow_block")
    );
    drop(inputs);
    let mut compiler = SurfaceRuleCompiler::new(&mut resources);
    let fluid_rule = compiler
        .compile(&json!({"type":"block","result_state":"minecraft:water"}))
        .unwrap();
    drop(compiler);
    let mut inputs = SeededSurfaceInputs::without_terrain_adjustments(
        &mut resources,
        &fluid_rule,
        42,
        generator.random_algorithm(),
        [0, 0, 0],
        [16, 16, 16],
    )
    .unwrap();
    let fluid = apply_surface(
        &chunk,
        &fluid_rule,
        &mut inputs,
        &preliminary,
        generator.sea_level(),
        classify,
        |_, _, _| ResourceId::parse("minecraft:plains"),
    )
    .unwrap();
    assert!(fluid.fluid_updates.iter().all(|flag| *flag));
}
