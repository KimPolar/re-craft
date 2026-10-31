//! Compare user-selected aquifer fluid states against Java 26.3.
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
    let output = command.output().expect("cannot run Java 25");
    assert!(
        output.status.success(),
        "aquifer oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn default_states_match_java_aquifers() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("AquiferDefaultStatesOracle.java");
    std::fs::write(
        &source,
        include_str!("java/AquiferDefaultStatesOracle.java"),
    )
    .unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
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
    let cp = std::env::join_paths(paths).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("AquiferDefaultStatesOracle"));
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
    let mut chunk = None;
    let mut groups = 0;
    let mut samples = 0;
    for line in output.lines() {
        let f: Vec<_> = line.split_whitespace().collect();
        match f.first().copied() {
            Some("S") => {
                let s: usize = f[1].parse().unwrap();
                let settings = json!({"noise":{"min_y":-64,"height":128},"legacy_random_source":true,
                    "default_block":"minecraft:stone","default_fluid":states[s],"sea_level":32,
                    "noise_router":{"final_density":-1,"temperature":0,"vegetation":0,"continents":0,"erosion":0,"depth":0,"ridges":0,"chunk_surface_level":0},
                    "aquifers":{"barrier":0,"fluid_level_floodedness":0.3,"fluid_level_spread":0.25,"lava":0.6,"exclusion":0,"surface_level":20}});
                let generator = NoiseStageGenerator::from_json_without_terrain_adjustments(
                    &mut Data,
                    &serde_json::to_vec(&settings).unwrap(),
                    12345,
                )
                .unwrap();
                let generated = generator
                    .generate(&mut DensityEvaluator::default(), -1, 2)
                    .unwrap();
                assert_eq!(generated.default_fluid(), &states[s]);
                chunk = Some(generated);
                groups += 1;
            }
            Some("P") => {
                let n = |i: usize| f[i].parse::<i32>().unwrap();
                let (x, y, z) = (n(1) as u8, n(2), n(3) as u8);
                let chunk = chunk.as_ref().unwrap();
                let code = match chunk.get(x, y, z).unwrap() {
                    NoiseMaterial::DefaultBlock => 0,
                    NoiseMaterial::DefaultFluid => 1,
                    NoiseMaterial::Lava => 2,
                    NoiseMaterial::Air => 3,
                };
                let index = (usize::from(z) * 16 + usize::from(x)) * 128 + (y + 64) as usize;
                assert_eq!(
                    (code, chunk.fluid_updates()[index]),
                    (n(4), n(5) != 0),
                    "state {} case {line}",
                    groups - 1
                );
                samples += 1;
            }
            _ => {}
        }
    }
    assert_eq!(groups, states.len());
    assert_eq!(samples, states.len() * 2048);
}
