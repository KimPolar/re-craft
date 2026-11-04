//! Compare real 26.3 SpawnPlacements with the resumable Rust inputs.
use anyhow::Result;
use glam::IVec3;
use native_core::random::LegacyRandom;
use native_resources::{
    packs::{ResourceId, ResourceStack},
    worldgen_mob_rules::{
        GroundCreatureSpawnWorld, GroundSpawnTag, GroundSpawnTags, check_ground_creature_spawn,
    },
};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3"]
fn vanilla_ground_tags_resolve_from_installed_packs() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let mut resources = ResourceStack::open(&game).unwrap();
    let registry = resources.vanilla_block_ids().unwrap();
    let tags = GroundSpawnTags::load(&mut resources, &registry).unwrap();
    assert!(tags.contains(GroundSpawnTag::Animals, "minecraft:grass_block"));
    assert!(tags.contains(GroundSpawnTag::Goats, "minecraft:grass_block"));
    assert!(tags.contains(GroundSpawnTag::Rabbits, "minecraft:grass_block"));
    assert!(tags.contains(GroundSpawnTag::Goats, "minecraft:stone"));
    assert!(!tags.contains(GroundSpawnTag::Animals, "minecraft:stone"));
    assert!(tags.contains(GroundSpawnTag::Rabbits, "minecraft:sand"));
}

struct World {
    pos: IVec3,
    light: u8,
    mask: u8,
    calls: Vec<&'static str>,
}
impl GroundCreatureSpawnWorld for World {
    fn raw_brightness(&mut self, position: IVec3) -> Result<u8> {
        assert_eq!(position, self.pos);
        self.calls.push("light");
        Ok(self.light)
    }
    fn tagged_ground(&mut self, position: IVec3, tag: GroundSpawnTag) -> Result<bool> {
        assert_eq!(position, self.pos - IVec3::Y);
        self.calls.push("ground");
        Ok(self.mask
            & match tag {
                GroundSpawnTag::Animals => 1,
                GroundSpawnTag::Goats => 2,
                GroundSpawnTag::Rabbits => 4,
            }
            != 0)
    }
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn ground_rules_match_java_results_call_order_and_random() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
        .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let mut cases = Vec::new();
    for entity in [
        "pig", "cow", "sheep", "chicken", "horse", "donkey", "goat", "rabbit",
    ] {
        for mask in 0..8 {
            for light in 0..=15 {
                for position in [[0, 64, 0], [-17, -16, 31]] {
                    for peaceful in [false, true] {
                        cases.push(json!({"entity":format!("minecraft:{entity}"),"mask":mask,"light":light,"position":position,"seed":42,"peaceful":peaceful}));
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
    let source = temp.path().join("ChunkCreatureRulesOracle.java");
    std::fs::write(&source, include_str!("java/ChunkCreatureRulesOracle.java")).unwrap();
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
        .arg("ChunkCreatureRulesOracle")
        .arg(&input)
        .arg(&output));
    let results: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(results.len(), cases.len());
    for (case, expected) in cases.iter().zip(results) {
        let coords = case["position"].as_array().unwrap();
        let pos = IVec3::new(
            coords[0].as_i64().unwrap() as i32,
            coords[1].as_i64().unwrap() as i32,
            coords[2].as_i64().unwrap() as i32,
        );
        let mut world = World {
            pos,
            light: case["light"].as_u64().unwrap() as u8,
            mask: case["mask"].as_u64().unwrap() as u8,
            calls: Vec::new(),
        };
        let entity = ResourceId::parse(case["entity"].as_str().unwrap()).unwrap();
        let result = check_ground_creature_spawn(&entity, pos, &mut world).unwrap();
        let mut random = LegacyRandom::new(case["seed"].as_i64().unwrap());
        assert_eq!(
            json!({"result":result,"calls":world.calls,"next":random.next_int()}),
            expected,
            "case {case}"
        );
    }
}
