//! Compare material-rule control flow and conditions with Java 26.3.
use native_core::beardifier::{Beardifier, BoundingBox, RigidPiece, TerrainAdjustment};
use native_resources::{
    density::WorldgenRandom,
    packs::{ResourceId, ResourceProvider, ResourceStack},
    surface_rules::{
        SurfaceDensitySampler, SurfaceInputs, SurfacePosition, SurfaceRandom, SurfaceRandomFactory,
        SurfaceRuleCompiler,
    },
};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command, sync::Arc};

struct Empty;
impl ResourceProvider for Empty {
    fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(None)
    }
}
struct Inputs<'a>(SurfaceRandomFactory, SurfaceDensitySampler<'a, Empty>);
impl SurfaceInputs for Inputs<'_> {
    fn biome_matches(&mut self, _: &Value, _: &SurfacePosition) -> anyhow::Result<bool> {
        anyhow::bail!("unexpected biome")
    }
    fn cold_enough_to_snow(&mut self, _: &SurfacePosition) -> anyhow::Result<bool> {
        anyhow::bail!("unexpected temperature")
    }
    fn noise(&mut self, _: &ResourceId, _: bool, p: &SurfacePosition) -> anyhow::Result<f64> {
        Ok(f64::from(p.x % 5 - 2) * 0.5)
    }
    fn positional_random(&mut self, name: &ResourceId, p: &SurfacePosition) -> anyhow::Result<f32> {
        Ok(self.0.at(name, p.x, p.y, p.z).next_float())
    }
    fn ore_random(&mut self, p: &SurfacePosition) -> anyhow::Result<SurfaceRandom> {
        Ok(self
            .0
            .at(&ResourceId::parse("minecraft:ore")?, p.x, p.y, p.z))
    }
    fn density(
        &mut self,
        value: &Value,
        in_chunk: bool,
        p: &SurfacePosition,
    ) -> anyhow::Result<f32> {
        self.1.sample(value, in_chunk, p.x, p.y, p.z)
    }
    fn bandlands(&mut self, _: &SurfacePosition) -> anyhow::Result<Arc<Value>> {
        anyhow::bail!("unexpected bandlands")
    }
}
fn run(c: &mut Command) -> String {
    let out = c.output().expect("cannot run Java 25");
    assert!(
        out.status.success(),
        "surface oracle failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
fn position(n: i32) -> SurfacePosition {
    SurfacePosition {
        x: n,
        y: (n * 37) % 211 - 100,
        z: -n,
        min_y: -64,
        height: 384,
        sea_level: 63,
        stone_above: (n * 7) % 11,
        stone_below: (n * 13) % 11,
        surface_depth: (n * 5) % 9 - 2,
        surface_secondary: f64::from((n * 7) % 25 - 12) / 8.0,
        water_height: if n % 5 == 0 {
            i32::MIN
        } else {
            (n * 17) % 181 - 90
        },
        min_surface_level: (n * 19) % 201 - 100,
        gradient_x: (n * 3) % 13 - 6,
        gradient_z: (n * 11) % 13 - 6,
    }
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn material_conditions_and_control_flow_match_java() {
    let conditions = vec![
        json!({"type":"y_above","anchor":{"absolute":13},"surface_depth_multiplier":-1,"add_stone_depth":true}),
        json!({"type":"y_above","anchor":{"above_bottom":20},"surface_depth_multiplier":2,"add_stone_depth":false}),
        json!({"type":"y_above","anchor":{"below_top":310},"surface_depth_multiplier":0,"add_stone_depth":true}),
        json!({"type":"y_above","anchor":{"relative_to_sea_level":-30},"surface_depth_multiplier":1,"add_stone_depth":false}),
        json!({"type":"water","offset":-6,"surface_depth_multiplier":-1,"add_stone_depth":true}),
        json!({"type":"water","offset":0,"surface_depth_multiplier":2,"add_stone_depth":false}),
        json!({"type":"stone_depth","offset":0,"add_surface_depth":true,"secondary_depth_range":6,"surface_type":"floor"}),
        json!({"type":"stone_depth","offset":-2,"add_surface_depth":false,"secondary_depth_range":30,"surface_type":"ceiling"}),
        json!({"type":"hole"}),
        json!({"type":"steep"}),
        json!({"type":"above_preliminary_surface"}),
        json!({"type":"not","invert":{"type":"hole"}}),
        json!({"type":"noise_threshold","noise":"minecraft:patch","min_threshold":-0.5,"max_threshold":0.5}),
        json!({"type":"noise_threshold","noise":"minecraft:patch","min_threshold":0.0,"max_threshold":1.0,"is_3d":true}),
        json!({"type":"vertical_gradient","random_name":"minecraft:bedrock_floor","true_at_and_below":{"absolute":-60},"false_at_and_above":{"absolute":40}}),
    ];
    let mut fixtures:Vec<Value>=conditions.iter().map(|condition|json!({"type":"sequence","sequence":[
        {"type":"condition","if_true":condition,"then_run":{"type":"block","result_state":"minecraft:stone"}},
        {"type":"block","result_state":"minecraft:dirt"}]})).collect();
    fixtures.push(json!({"type":"sequence","sequence":[
        {"type":"condition","if_true":conditions[0],"then_run":{"type":"condition","if_true":conditions[4],"then_run":{"type":"block","result_state":"minecraft:grass_block"}}},
        {"type":"condition","if_true":conditions[6],"then_run":{"type":"block","result_state":"minecraft:stone"}},
        {"type":"block","result_state":"minecraft:dirt"}]}));
    for (density, richness, gap, raw) in [
        (-1.0, 0.5, -0.2, 0.5),
        (0.0, 0.5, -0.2, 0.5),
        (0.1, 0.5, -0.2, 0.5),
        (0.5, 0.5, -0.2, 0.5),
        (1.0, 0.0, -0.2, 0.5),
        (1.0, 1.0, 0.0, 0.5),
        (1.0, 1.0, -0.2, 0.0),
        (1.0, 1.0, -0.2, 1.0),
        (1.0, 0.4, -0.2, 0.2),
    ] {
        fixtures.push(json!({"type":"ore_vein","ore_block":"minecraft:stone","raw_ore_block":"minecraft:dirt",
            "filler_block":"minecraft:grass_block","raw_ore_chance":raw,"density":density,"richness":richness,"filler_gap":gap}));
    }
    fixtures.push(json!({"type":"ore_vein","ore_block":"minecraft:stone","raw_ore_block":"minecraft:dirt",
        "filler_block":"minecraft:grass_block","raw_ore_chance":0.5,"density":{"type":"minecraft:beardifier"},"richness":0.6,"filler_gap":-0.2}));
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("SurfaceRulesOracle.java");
    std::fs::write(&source, include_str!("java/SurfaceRulesOracle.java")).unwrap();
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
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let input = temp.path().join("surface.json");
    std::fs::write(&input, serde_json::to_vec(&fixtures).unwrap()).unwrap();
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("SurfaceRulesOracle")
        .arg(&input));
    let mut data = Empty;
    let mut compiler = SurfaceRuleCompiler::new(&mut data);
    let mut current = None;
    let bounds = BoundingBox::new([0, -100, -30], [30, 100, 0]).unwrap();
    let structure = Arc::new(Beardifier::new(
        vec![RigidPiece {
            bounds,
            adjustment: TerrainAdjustment::Encapsulate,
            ground_delta: 0,
        }],
        vec![],
        Some(BoundingBox::new([-24, -124, -54], [54, 124, 24]).unwrap()),
    ));
    let mut density_data = Empty;
    let density = SurfaceDensitySampler::with_structure_context(
        &mut density_data,
        12345,
        WorldgenRandom::Xoroshiro,
        [0, -64, 0],
        [16, 128, 16],
        Some(structure),
    )
    .unwrap();
    let mut inputs = Inputs(
        SurfaceRandomFactory::new(12345, WorldgenRandom::Xoroshiro),
        density,
    );
    let mut groups = 0;
    let mut samples = 0;
    let mut random_samples = 0;
    let mut random_factories = std::collections::BTreeMap::new();
    for line in output.lines() {
        let f: Vec<_> = line.split_whitespace().collect();
        match f.first().copied() {
            Some("S") => {
                let i: usize = f[1].parse().unwrap();
                current = Some(compiler.compile(&fixtures[i]).unwrap());
                groups += 1;
            }
            Some("P") => {
                let n: i32 = f[1].parse().unwrap();
                let state = current
                    .as_ref()
                    .unwrap()
                    .evaluate(&position(n), &mut inputs)
                    .unwrap();
                let code = match state.as_deref().and_then(Value::as_str) {
                    None => 0,
                    Some("minecraft:stone") => 1,
                    Some("minecraft:dirt") => 2,
                    Some("minecraft:grass_block") => 3,
                    other => panic!("unexpected {other:?}"),
                };
                assert_eq!(
                    code,
                    f[2].parse::<i32>().unwrap(),
                    "fixture {} case {n}",
                    groups - 1
                );
                samples += 1;
            }
            Some("R") => {
                let mode: i32 = f[1].parse().unwrap();
                let seed: i64 = f[2].parse().unwrap();
                let name = ResourceId::parse(f[3]).unwrap();
                let coordinate = |i: usize| f[i].parse::<i32>().unwrap();
                let factory = random_factories.entry((mode, seed)).or_insert_with(|| {
                    SurfaceRandomFactory::new(
                        seed,
                        if mode == 0 {
                            WorldgenRandom::Legacy
                        } else {
                            WorldgenRandom::Xoroshiro
                        },
                    )
                });
                let mut r = factory.at(&name, coordinate(4), coordinate(5), coordinate(6));
                for i in 0..3 {
                    assert_eq!(
                        r.next_float().to_bits(),
                        coordinate(7 + i) as u32,
                        "named random {line}"
                    );
                }
                random_samples += 1;
            }
            _ => {}
        }
    }
    assert_eq!(groups, fixtures.len());
    assert_eq!(samples, fixtures.len() * 512);
    assert_eq!(random_samples, 2 * 5 * 2 * 7);
    let mut resources = ResourceStack::open(&game).unwrap();
    let mut compiler = SurfaceRuleCompiler::new(&mut resources);
    for name in ["minecraft:end", "minecraft:nether", "minecraft:overworld"] {
        compiler
            .compile(&json!(name))
            .unwrap_or_else(|e| panic!("{name}: {e:#}"));
    }
}
