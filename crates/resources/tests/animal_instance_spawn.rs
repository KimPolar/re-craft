//! Compare post-construction animal rules with Java 26.3.
use anyhow::Result;
use glam::IVec3;
use native_resources::{
    packs::ResourceId,
    worldgen_mob_rules::{AnimalInstanceSpawnWorld, check_animal_instance_spawn},
};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};

struct World {
    pos: IVec3,
    grass: bool,
    light: u8,
    ambient: f32,
    calls: Vec<&'static str>,
}
impl AnimalInstanceSpawnWorld for World {
    fn is_grass_block(&mut self, position: IVec3) -> Result<bool> {
        assert_eq!(position, self.pos - IVec3::Y);
        self.calls.push("ground");
        Ok(self.grass)
    }
    fn raw_brightness(&mut self, position: IVec3) -> Result<u8> {
        assert_eq!(position, self.pos);
        self.calls.push("light");
        Ok(self.light)
    }
    fn ambient_light(&mut self) -> Result<f32> {
        self.calls.push("ambient");
        Ok(self.ambient)
    }
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn constructed_animal_rules_match_java_including_world_edges() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
        .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let mut cases = Vec::new();
    for entity in ["pig", "cow", "sheep", "chicken", "horse", "goat", "rabbit"] {
        for grass in [false, true] {
            for light in 0..=15 {
                for ambient in [0.0, 0.1, 0.2, 0.25, 0.5, 1.0] {
                    for x in [-30_000_001, -30_000_000, 29_999_999, 30_000_000] {
                        cases.push(json!({"entity":entity,"grass":grass,"light":light,"ambient":ambient,"position":[x,64,0]}));
                    }
                }
            }
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for library in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("AnimalInstanceSpawnOracle.java");
    std::fs::write(&source, include_str!("java/AnimalInstanceSpawnOracle.java")).unwrap();
    let run = |command: &mut Command| {
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run(Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    let input = temp.path().join("input.json");
    let output = temp.path().join("output.json");
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    run(Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg("AnimalInstanceSpawnOracle")
        .arg(&input)
        .arg(&output));
    let results: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(results.len(), cases.len());
    for (case, expected) in cases.iter().zip(results) {
        let pos = IVec3::new(case["position"][0].as_i64().unwrap() as i32, 64, 0);
        let mut world = World {
            pos,
            grass: case["grass"].as_bool().unwrap(),
            light: case["light"].as_u64().unwrap() as u8,
            ambient: case["ambient"].as_f64().unwrap() as f32,
            calls: Vec::new(),
        };
        let id =
            ResourceId::parse(&format!("minecraft:{}", case["entity"].as_str().unwrap())).unwrap();
        let actual = check_animal_instance_spawn(&id, pos, &mut world).unwrap();
        assert_eq!(
            json!({"result":actual,"calls":world.calls}),
            expected,
            "case {case}"
        );
    }
}
