//! Carver-density aquifer sampling compared to Java 26.3.
use native_resources::{
    density::DensityEvaluator,
    noise_stage::{NoiseMaterial, NoiseStageGenerator},
    packs::ResourceProvider,
};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};
struct Data;
impl ResourceProvider for Data {
    fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(None)
    }
}
fn run(command: &mut Command) -> String {
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "Java aquifer oracle failed: {}\n{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout[out.stdout.len().saturating_sub(2000)..])
    );
    String::from_utf8(out.stdout).unwrap()
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn zero_density_carving_aquifer_matches_java() {
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
    let source = temp.path().join("AquiferDefaultStatesOracle.java");
    std::fs::write(
        &source,
        include_str!("java/AquiferDefaultStatesOracle.java"),
    )
    .unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("AquiferDefaultStatesOracle")
        .arg("0"));
    let states = [
        json!("minecraft:water"),
        json!({"id":"minecraft:water","properties":{"level":"1"}}),
        json!("minecraft:lava"),
        json!({"id":"minecraft:lava","properties":{"level":"1"}}),
        json!({"id":"minecraft:lava","properties":{"level":"15"}}),
        json!({"id":"minecraft:lava","properties":{"level":"99"}}),
        json!("minecraft:air"),
        json!("minecraft:cave_air"),
        json!("minecraft:void_air"),
        json!("minecraft:stone"),
        json!("minecraft:sand"),
    ];
    let mut groups = Vec::<Vec<(i32, i32, i32, i32, bool)>>::new();
    for line in output.lines() {
        let p: Vec<_> = line.split_whitespace().collect();
        match p.first().copied() {
            Some("S") => groups.push(Vec::new()),
            Some("P") => groups.last_mut().unwrap().push((
                p[1].parse().unwrap(),
                p[2].parse().unwrap(),
                p[3].parse().unwrap(),
                p[4].parse().unwrap(),
                p[5] == "1",
            )),
            _ => {}
        }
    }
    assert_eq!(groups.len(), states.len());
    for (i, fluid) in states.iter().enumerate() {
        let settings = json!({"noise":{"min_y":-64,"height":128},"legacy_random_source":true,
           "default_block":"minecraft:stone","default_fluid":fluid,"sea_level":32,
           "noise_router":{"final_density":-1,"temperature":0,"vegetation":0,"continents":0,"erosion":0,"depth":0,"ridges":0,"chunk_surface_level":0},
           "aquifers":{"barrier":0,"fluid_level_floodedness":0.3,"fluid_level_spread":0.25,"lava":0.6,"exclusion":0,"surface_level":20}});
        let generator = NoiseStageGenerator::from_json_without_terrain_adjustments(
            &mut Data,
            &serde_json::to_vec(&settings).unwrap(),
            12345,
        )
        .unwrap();
        let mut evaluator = DensityEvaluator::default();
        let mut aquifer = generator.carving_aquifer(&mut evaluator, [-1, 2]).unwrap();
        assert_eq!(groups[i].len(), 2048);
        for &(x, y, z, code, updates) in &groups[i] {
            let result = aquifer.sample(-16 + x, y, 32 + z).unwrap();
            let actual = match result.material {
                None => 0,
                Some(NoiseMaterial::DefaultFluid) => 1,
                Some(NoiseMaterial::Lava) => 2,
                Some(NoiseMaterial::Air) => 3,
                Some(NoiseMaterial::DefaultBlock) => panic!("unexpected solid"),
            };
            assert_eq!(
                (actual, result.schedule_fluid_update),
                (code, updates),
                "fluid {i}, ({x},{y},{z})"
            );
        }
        assert!(aquifer.sample(0, 0, 32).is_err());
    }
}
