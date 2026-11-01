//! Verify mask application against Java's real NoiseBasedChunkGenerator method.
use anyhow::Result;
use native_resources::{
    carver::{
        CarvingMask,
        apply::{AquiferReplacement, CarvedState, CarvingInputs, apply_mask},
        carve_ellipsoid,
    },
    surface_pass::SurfaceStageChunk,
};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command, sync::Arc};
fn run(command: &mut Command) -> String {
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "Java apply oracle failed: {}\n{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout[out.stdout.len().saturating_sub(2000)..])
    );
    String::from_utf8(out.stdout).unwrap()
}
fn state(name: &str, has_fluid: bool) -> CarvedState {
    CarvedState {
        state: Arc::new(json!(name)),
        has_fluid,
    }
}
#[derive(Default)]
struct Inputs(Vec<(i32, bool)>);
impl CarvingInputs for Inputs {
    fn may_carve(&mut self, _: u8, y: i32, _: u8) -> Result<bool> {
        Ok(y != 5)
    }
    fn uncarvable(&mut self, v: &Value) -> Result<bool> {
        Ok(v.as_str() == Some("minecraft:bedrock"))
    }
    fn aquifer(&mut self, _: i32, y: i32, _: i32) -> Result<Option<AquiferReplacement>> {
        if y == 4 {
            return Ok(None);
        }
        let fluid = y == 7;
        Ok(Some(AquiferReplacement {
            block: state(
                if fluid {
                    "minecraft:water"
                } else {
                    "minecraft:cave_air"
                },
                fluid,
            ),
            schedule_fluid_update: true,
        }))
    }
    fn top_material(
        &mut self,
        _: &SurfaceStageChunk,
        _: i32,
        y: i32,
        _: i32,
        fluid: bool,
    ) -> Result<Option<CarvedState>> {
        self.0.push((y, fluid));
        Ok(Some(state(
            if fluid {
                "minecraft:water"
            } else {
                "minecraft:grass_block"
            },
            fluid,
        )))
    }
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn carving_application_matches_java() {
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
    let source = temp.path().join("CarverApplyOracle.java");
    std::fs::write(&source, include_str!("java/CarverApplyOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("CarverApplyOracle"));
    let mut chunk = SurfaceStageChunk {
        chunk_position: [0, 0],
        min_y: 0,
        height: 16,
        states: vec![Arc::new(json!("minecraft:stone")); 4096],
        fluid_updates: vec![false; 4096],
    };
    for (y, name) in [
        (2, "minecraft:dirt"),
        (6, "minecraft:dirt"),
        (7, "minecraft:grass_block"),
        (8, "minecraft:bedrock"),
        (10, "minecraft:dirt"),
    ] {
        chunk.states[y] = Arc::new(json!(name));
    }
    let mut mask = CarvingMask::new(1, 14).unwrap();
    carve_ellipsoid(
        &mut mask,
        [0, 0],
        [0.5, 8.0, 0.5],
        0.5,
        10.0,
        |_, _, _, y| ![3, 4, 5, 7, 8, 11].contains(&y),
    )
    .unwrap();
    let mut inputs = Inputs::default();
    apply_mask(&mut chunk, &mask, &mut inputs).unwrap();
    let expected: Vec<String> = inputs
        .0
        .iter()
        .map(|(y, fluid)| format!("T {y} {fluid}"))
        .chain((0..16).map(|y| {
            format!(
                "P {y} {} {}",
                chunk.states[y].as_str().unwrap(),
                chunk.fluid_updates[y]
            )
        }))
        .collect();
    let actual: Vec<_> = output
        .lines()
        .filter(|line| line.starts_with("T ") || line.starts_with("P "))
        .collect();
    assert_eq!(
        actual,
        expected.iter().map(String::as_str).collect::<Vec<_>>()
    );
}
