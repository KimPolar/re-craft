//! Compare spawn position control flow with the installed Java 26.3 methods.
use anyhow::{Context, Result};
use glam::IVec3;
use native_resources::mob_spawn_placement::{
    MobPlacementWorld, TopMobSpawnPosition, on_ground_spawn_position_ok,
};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};

struct JavaFacts<'a> {
    case: &'a Value,
    result: &'a Value,
}
impl JavaFacts<'_> {
    fn fact(&self, pos: IVec3, name: &str) -> Result<bool> {
        self.result["facts"]
            .get(pos.y.to_string())
            .and_then(|v| v.get(name))
            .and_then(Value::as_bool)
            .context("missing test fact")
    }
}
impl MobPlacementWorld for JavaFacts<'_> {
    fn height(&mut self, _: i32, _: i32) -> Result<i32> {
        Ok(self.case["height"].as_i64().unwrap() as i32)
    }
    fn is_air(&mut self, pos: IVec3) -> Result<bool> {
        self.fact(pos, "air")
    }
    fn pathfindable_land(&mut self, pos: IVec3) -> Result<bool> {
        self.fact(pos, "path")
    }
    fn within_border(&mut self, _: IVec3) -> Result<bool> {
        Ok(self.result["within"].as_bool().unwrap())
    }
    fn valid_spawn_support(&mut self, pos: IVec3) -> Result<bool> {
        self.fact(pos, "support")
    }
    fn full_collision(&mut self, pos: IVec3) -> Result<bool> {
        self.fact(pos, "full")
    }
    fn signal_source(&mut self, pos: IVec3) -> Result<bool> {
        self.fact(pos, "signal")
    }
    fn fluid_empty(&mut self, pos: IVec3) -> Result<bool> {
        self.fact(pos, "fluid_empty")
    }
    fn prevents_mob_spawning(&mut self, pos: IVec3) -> Result<bool> {
        self.fact(pos, "prevents")
    }
    fn dangerous_to_entity(&mut self, pos: IVec3) -> Result<bool> {
        self.fact(pos, "dangerous")
    }
}
fn run(command: &mut Command) {
    let result = command.output().expect("cannot run Java 25");
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn top_and_on_ground_placement_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
        .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let mut cases = Vec::new();
    for support in [
        "stone",
        "dirt",
        "grass_block",
        "sand",
        "glass",
        "oak_slab",
        "oak_leaves",
        "magma_block",
        "water",
        "air",
    ] {
        for feet in [
            "air",
            "stone",
            "water",
            "cactus",
            "short_grass",
            "cobweb",
            "redstone_block",
            "torch",
        ] {
            for x in [0, 20] {
                cases.push(
                    json!({"height":8,"min_y":0,"ceiling":false,"x":x,"z":0,"border_size":20.0,
                    "blocks":{"7":format!("minecraft:{support}"),"8":format!("minecraft:{feet}")}}),
                );
            }
        }
    }
    for floor in ["stone", "air", "water", "oak_slab", "grass_block"] {
        cases.push(json!({"height":12,"min_y":0,"ceiling":true,"x":-3,"z":4,"border_size":100.0,
            "blocks":{"11":"minecraft:stone","10":"minecraft:stone","7":format!("minecraft:{floor}")}}));
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
    let source = temp.path().join("MobPlacementOracle.java");
    std::fs::write(
        &source,
        include_str!("../../resources/tests/java/MobPlacementOracle.java"),
    )
    .unwrap();
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
        .arg("MobPlacementOracle")
        .arg(&input)
        .arg(&output));
    let results: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(results.len(), cases.len());
    let mut checked_facts = 0;
    for (case, expected) in cases.iter().zip(&results) {
        for y in -2..=20 {
            let name = case["blocks"]
                .get(y.to_string())
                .and_then(Value::as_str)
                .unwrap_or("minecraft:air");
            let mut properties = std::collections::BTreeMap::new();
            if name == "minecraft:grass_block" {
                properties.insert("snowy".into(), "false".into());
            }
            let state = native_world::loaded::BlockState {
                name: name.into(),
                properties,
            };
            if let Some(facts) =
                native_world::mob_spawn_blocks::common_mob_spawn_block_facts(&state)
            {
                let actual = &expected["facts"][y.to_string()];
                assert_eq!(actual["universal"], true, "{name}");
                assert_eq!(
                    json!({"air":facts.air,"path":facts.pathfindable_land,
                    "support":facts.valid_support,"full":facts.full_collision,
                    "signal":facts.signal_source,"fluid_empty":facts.fluid_empty,
                    "dangerous":facts.dangerous}),
                    json!({"air":actual["air"],"path":actual["path"],"support":actual["support"],
                        "full":actual["full"],"signal":actual["signal"],
                        "fluid_empty":actual["fluid_empty"],"dangerous":actual["dangerous"]}),
                    "{name}"
                );
                checked_facts += 1;
            }
        }
        for budget in [1, 2, 16] {
            let x = case["x"].as_i64().unwrap() as i32;
            let z = case["z"].as_i64().unwrap() as i32;
            let mut world = JavaFacts {
                case,
                result: expected,
            };
            let mut search = TopMobSpawnPosition::new(
                x,
                z,
                case["min_y"].as_i64().unwrap() as i32,
                case["ceiling"].as_bool().unwrap(),
            );
            let top = loop {
                if let Some(pos) = search.poll(&mut world, budget).unwrap() {
                    break pos;
                }
            };
            assert_eq!(
                top.y,
                expected["top_y"].as_i64().unwrap() as i32,
                "case {case}"
            );
            assert_eq!(
                on_ground_spawn_position_ok(&mut world, top).unwrap(),
                expected["ok"].as_bool().unwrap(),
                "case {case}"
            );
        }
    }
    assert!(checked_facts > 100);
}
