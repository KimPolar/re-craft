//! Compare spreading-soil ticks with the installed Java 26.3 implementation.

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn spreading_soil_random_ticks_match_java() {
    use anyhow::Result;
    use glam::IVec3;
    use native_core::random::LegacyRandom;
    use native_session::spreading_soil::{SpreadingWorld, random_tick};
    use native_world::{
        fluid::{FluidKind, FluidState},
        loaded::BlockState,
    };
    use serde_json::{Value, json};
    use std::{collections::HashMap, path::PathBuf, process::Command};

    fn state(name: &str, properties: &[(&str, &str)]) -> BlockState {
        BlockState {
            name: format!("minecraft:{name}"),
            properties: properties
                .iter()
                .map(|(key, value)| ((*key).into(), (*value).into()))
                .collect(),
        }
    }
    fn cover(index: i64) -> BlockState {
        match index {
            0 => state("air", &[]),
            1 => state("snow", &[("layers", "1")]),
            2 => state("water", &[("level", "0")]),
            3 => state("water", &[("level", "1")]),
            4 => state("stone", &[]),
            5 => state("glass", &[]),
            6 => state("oak_slab", &[("type", "top"), ("waterlogged", "false")]),
            7 => state("oak_slab", &[("type", "bottom"), ("waterlogged", "false")]),
            8 => state("snow", &[("layers", "8")]),
            9 => state("oak_slab", &[("type", "bottom"), ("waterlogged", "true")]),
            _ => panic!("unknown cover"),
        }
    }
    struct World {
        blocks: HashMap<IVec3, BlockState>,
        writes: Vec<Value>,
        brightness: u8,
        tag: i64,
        consume_random: bool,
    }
    impl SpreadingWorld for World {
        fn has_dirt(&mut self) -> Result<bool> {
            Ok(true)
        }
        fn block(&mut self, pos: IVec3) -> Result<BlockState> {
            Ok(self
                .blocks
                .get(&pos)
                .cloned()
                .unwrap_or_else(|| state("air", &[])))
        }
        fn brightness(&mut self, _: IVec3) -> Result<u8> {
            Ok(self.brightness)
        }
        fn water_tagged(&mut self, block: &BlockState) -> Result<bool> {
            let fluid = FluidState::from_block(block).map_err(anyhow::Error::msg)?;
            Ok(match self.tag {
                0 => fluid.is_some_and(|fluid| fluid.kind == FluidKind::Water),
                1 => fluid.is_none(),
                _ => false,
            })
        }
        fn snowy(&mut self, block: &BlockState) -> Result<bool> {
            Ok(matches!(
                block.name.as_str(),
                "minecraft:snow" | "minecraft:snow_block"
            ))
        }
        fn set_block(
            &mut self,
            pos: IVec3,
            block: BlockState,
            random: &mut LegacyRandom,
        ) -> Result<()> {
            self.writes.push(json!([
                pos.x,
                pos.y,
                pos.z,
                block.name,
                block
                    .properties
                    .get("snowy")
                    .is_some_and(|value| value == "true")
            ]));
            self.blocks.insert(pos, block);
            if self.consume_random {
                random.next_int();
            }
            Ok(())
        }
    }

    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for library in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let classpath = std::env::join_paths(paths).unwrap();
    let java = std::env::var_os("MINECRAFT_JAVA_HOME")
        .or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let source = temp.path().join("SpreadingSnowyOracle.java");
    let output = temp.path().join("spreading.json");
    std::fs::write(&source, include_str!("java/SpreadingSnowyOracle.java")).unwrap();
    for command in [
        Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source),
        Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("SpreadingSnowyOracle")
            .arg(&output),
    ] {
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "Java soil oracle failed:\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(rows.len(), 3 * 2 * 10 * 4 * 32 * 2 * 2);
    let mut changes = 0;
    for row in rows {
        let seed = row["seed"].as_i64().unwrap();
        let mut world = World {
            blocks: HashMap::new(),
            writes: Vec::new(),
            brightness: row["brightness"].as_u64().unwrap() as u8,
            tag: row["tag"].as_i64().unwrap(),
            consume_random: row["consume"].as_bool().unwrap(),
        };
        for x in -1..=1 {
            for z in -1..=1 {
                world.blocks.insert(IVec3::new(x, 7, z), state("dirt", &[]));
                if x != 0 || z != 0 {
                    world.blocks.insert(
                        IVec3::new(x, 8, z),
                        cover((i64::from((x + 1) * 3 + z + 1) + seed).rem_euclid(5)),
                    );
                }
            }
        }
        let source = state(
            if row["kind"] == 0 {
                "grass_block"
            } else {
                "mycelium"
            },
            &[(
                "snowy",
                if row["snowy"].as_bool().unwrap() {
                    "true"
                } else {
                    "false"
                },
            )],
        );
        let origin = IVec3::new(0, 8, 0);
        world.blocks.insert(origin, source.clone());
        world
            .blocks
            .insert(origin + IVec3::Y, cover(row["cover"].as_i64().unwrap()));
        let mut random = LegacyRandom::new(seed);
        random_tick(&mut world, origin, &source, &mut random).unwrap();
        assert_eq!(
            world.writes,
            row["writes"].as_array().unwrap().clone(),
            "{row}"
        );
        assert_eq!(
            i64::from(random.next_int()),
            row["next"].as_i64().unwrap(),
            "{row}"
        );
        changes += world.writes.len();
    }
    assert!(
        changes > 0,
        "Java scenarios did not exercise any block changes"
    );
    eprintln!("Compared 30,720 spreading-soil ticks and {changes} writes with Java 26.3");
}
