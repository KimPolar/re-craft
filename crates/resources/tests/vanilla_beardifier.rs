//! Compare 26.3 structure terrain contributions and sampled volumes.
use native_core::beardifier::{Beardifier, BoundingBox, Junction, RigidPiece, TerrainAdjustment};
use native_resources::{
    density::{DensityCompiler, DensityEvaluator},
    noise_router::{NoiseRouter, RouterField},
    packs::{ResourceProvider, ResourceStack},
};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command, sync::Arc};

struct EmptyProvider;
impl ResourceProvider for EmptyProvider {
    fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(None)
    }
}
fn run(command: &mut Command) -> String {
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "Java beardifier oracle failed: {}\n{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout[out.stdout.len().saturating_sub(2500)..])
    );
    String::from_utf8(out.stdout).unwrap()
}
fn vec3(v: &Value) -> [i32; 3] {
    [0, 1, 2].map(|i| v[i].as_i64().unwrap() as i32)
}
fn bounds(v: &Value) -> BoundingBox {
    BoundingBox::new(vec3(&v["min"]), vec3(&v["max"])).unwrap()
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn structure_terrain_samples_match_java_bits() {
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
    let source = temp.path().join("BeardifierOracle.java");
    std::fs::write(&source, include_str!("java/BeardifierOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let mut state = 39_281_321_871u64;
    let mut random = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 32) as u32
    };
    let mut groups = Vec::new();
    let kinds = ["NONE", "BURY", "BEARD_THIN", "BEARD_BOX", "ENCAPSULATE"];
    for g in 0..25 {
        let mut pieces = Vec::new();
        for p in 0..(1 + g % 5) {
            let x = (random() % 31) as i32 - 15;
            let y = (random() % 21) as i32 - 10;
            let z = (random() % 31) as i32 - 15;
            pieces.push(json!({"box":{"min":[x,y,z],"max":[x+(random()%10) as i32,y+(random()%12) as i32,z+(random()%10) as i32]},"adjustment":kinds[(g+p)%5],"delta":(random()%9) as i32-4}));
        }
        let junctions: Vec<_> = (0..g % 4)
            .map(|_| {
                json!([
                    (random() % 31) as i32 - 15,
                    (random() % 21) as i32 - 10,
                    (random() % 31) as i32 - 15
                ])
            })
            .collect();
        let affected = if g == 0 {
            Value::Null
        } else {
            json!({"min":[-24,-20,-24],"max":[24,20,24]})
        };
        let mut points = Vec::new();
        for _ in 0..320 {
            points.push(json!([
                (random() % 75) as i32 - 37,
                (random() % 65) as i32 - 32,
                (random() % 75) as i32 - 37
            ]));
        }
        points.extend([
            json!([-24, -20, -24]),
            json!([24, 20, 24]),
            json!([-25, 0, 0]),
            json!([25, 0, 0]),
        ]);
        let mut volumes = Vec::new();
        for _ in 0..12 {
            volumes.push(json!({"min":[(random()%61) as i32-30,(random()%51) as i32-25,(random()%61) as i32-30],"size":[1+random()%5,1+random()%6,1+random()%5],"step":[1+random()%4,1+random()%4,1+random()%4]}));
        }
        groups.push(json!({"pieces":pieces,"junctions":junctions,"affected":affected,"points":points,"volumes":volumes}));
    }
    let input = temp.path().join("cases.json");
    std::fs::write(&input, serde_json::to_vec(&groups).unwrap()).unwrap();
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("BeardifierOracle")
        .arg(&input));
    let mut lines = output.lines().filter(|s| {
        s == &"S"
            || ["P ", "V ", "Q ", "W "]
                .iter()
                .any(|prefix| s.starts_with(prefix))
    });
    let mut sample_count = 0;
    let mut provider = EmptyProvider;
    let function = DensityCompiler::new(&mut provider)
        .with_structure_terrain_context()
        .compile(
            &json!({"type":"minecraft:add","left":-0.25,"right":{"type":"minecraft:beardifier"}}),
        )
        .unwrap();
    let mut evaluator = DensityEvaluator::default();
    for (g, case) in groups.iter().enumerate() {
        assert_eq!(lines.next(), Some("S"));
        let pieces = case["pieces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| RigidPiece {
                bounds: bounds(&p["box"]),
                adjustment: match p["adjustment"].as_str().unwrap() {
                    "NONE" => TerrainAdjustment::None,
                    "BURY" => TerrainAdjustment::Bury,
                    "BEARD_THIN" => TerrainAdjustment::BeardThin,
                    "BEARD_BOX" => TerrainAdjustment::BeardBox,
                    _ => TerrainAdjustment::Encapsulate,
                },
                ground_delta: p["delta"].as_i64().unwrap() as i32,
            })
            .collect();
        let junctions = case["junctions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| Junction { position: vec3(v) })
            .collect();
        let affected = (!case["affected"].is_null()).then(|| bounds(&case["affected"]));
        let generator = Beardifier::new(pieces, junctions, affected);
        for (i, p) in case["points"].as_array().unwrap().iter().enumerate() {
            let p = vec3(p);
            let expected = lines
                .next()
                .unwrap()
                .strip_prefix("P ")
                .unwrap()
                .parse::<i32>()
                .unwrap() as u32;
            assert_eq!(
                generator.sample(p[0], p[1], p[2]).to_bits(),
                expected,
                "case {g} point {i} {p:?}"
            );
            sample_count += 1;
        }
        for (i, v) in case["volumes"].as_array().unwrap().iter().enumerate() {
            let min = vec3(&v["min"]);
            let sz = vec3(&v["size"]).map(|n| n as u32);
            let step = vec3(&v["step"]);
            let values = generator.sample_volume(min, sz, step).unwrap();
            let expected: Vec<_> = lines
                .next()
                .unwrap()
                .split_whitespace()
                .skip(1)
                .map(|s| s.parse::<i32>().unwrap() as u32)
                .collect();
            assert_eq!(
                values.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                expected,
                "case {g} volume {i} {min:?} {sz:?} {step:?}"
            );
            sample_count += values.len();
        }
        evaluator.replace_structure_terrain_context(Some(Arc::new(generator)));
        for (i, p) in case["points"].as_array().unwrap().iter().enumerate() {
            let p = vec3(p);
            let expected = lines
                .next()
                .unwrap()
                .strip_prefix("Q ")
                .unwrap()
                .parse::<i32>()
                .unwrap() as u32;
            assert_eq!(
                evaluator.sample(&function, p[0], p[1], p[2]).to_bits(),
                expected,
                "density case {g} point {i} {p:?}"
            );
        }
        for (i, v) in case["volumes"].as_array().unwrap().iter().enumerate() {
            let min = vec3(&v["min"]);
            let size = vec3(&v["size"]).map(|n| n as u32);
            let step = vec3(&v["step"]);
            let expected: Vec<u32> = lines
                .next()
                .unwrap()
                .split_whitespace()
                .skip(1)
                .map(|s| s.parse::<i32>().unwrap() as u32)
                .collect();
            let actual = evaluator
                .sample_stepped_volume(&function, min, size, step)
                .unwrap();
            assert_eq!(
                actual.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                expected,
                "density case {g} volume {i} {min:?} {size:?} {step:?}"
            );
        }
    }
    assert!(sample_count > 15_000);
    assert!(lines.next().is_none());

    // Exercise the installed, reference-heavy overworld graph, compiled only once
    // for all structure contexts rather than once per chunk.
    let mut resources = ResourceStack::open(&game).unwrap();
    let settings = resources
        .read_builtin_data("data/minecraft/worldgen/noise_settings/overworld.json")
        .unwrap()
        .unwrap();
    let adjusted =
        NoiseRouter::from_json_with_structure_context(&mut resources, &settings, 12345).unwrap();
    let plain =
        NoiseRouter::from_json_without_terrain_adjustments(&mut resources, &settings, 12345)
            .unwrap();
    let bounds = BoundingBox::new([0, 60, 0], [3, 65, 3]).unwrap();
    let structure = Arc::new(Beardifier::new(
        vec![RigidPiece {
            bounds,
            adjustment: TerrainAdjustment::Bury,
            ground_delta: 0,
        }],
        vec![],
        Some(BoundingBox::new([-24, 36, -24], [27, 89, 27]).unwrap()),
    ));
    let mut plain_eval = DensityEvaluator::default();
    let mut adjusted_eval = DensityEvaluator::default();
    let min = [0, 58, 0];
    let size = [4, 8, 4];
    let base = plain
        .sample_block_volume(&mut plain_eval, RouterField::FinalDensity, min, size)
        .unwrap();
    let empty = adjusted
        .sample_block_volume(&mut adjusted_eval, RouterField::FinalDensity, min, size)
        .unwrap();
    assert_eq!(
        base.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        empty.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    );
    adjusted_eval.replace_structure_terrain_context(Some(Arc::clone(&structure)));
    let actual = adjusted
        .sample_block_volume(&mut adjusted_eval, RouterField::FinalDensity, min, size)
        .unwrap();
    let contribution = structure.sample_volume(min, size, [1, 1, 1]).unwrap();
    assert!(contribution.iter().any(|v| *v != 0.0));
    for ((base, contribution), actual) in base.iter().zip(contribution).zip(actual) {
        assert_eq!((*base + contribution).to_bits(), actual.to_bits());
    }
}
