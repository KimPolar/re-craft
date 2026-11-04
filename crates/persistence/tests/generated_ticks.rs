use native_persistence::{
    generated_ticks::{
        encode_block_ticks, encode_fluid_ticks, encode_level_block_ticks, encode_level_fluid_ticks,
    },
    nbt::{self, NamedTag, Tag},
};
use native_world::loaded::{ChunkPos, SavedBlockTick, SavedFluidTick};
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn generated_ticks_match_java_protochunk_packing() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
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
    let source = temp.path().join("GeneratedTicksOracle.java");
    std::fs::write(&source, include_str!("java/GeneratedTicksOracle.java")).unwrap();
    let run = |cmd: &mut Command| {
        let result = cmd.output().unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    };
    run(Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(source));
    let pos = ChunkPos { x: -2, z: 3 };
    let blocks = [
        SavedBlockTick {
            block: "air".into(),
            position: [-32, -65, 63],
            delay: 0,
            priority: -1,
        },
        SavedBlockTick {
            block: "minecraft:sand".into(),
            position: [-31, 15, 48],
            delay: 0,
            priority: -3,
        },
        SavedBlockTick {
            block: "water".into(),
            position: [-31, 15, 48],
            delay: 0,
            priority: 3,
        },
        SavedBlockTick {
            block: "minecraft:sand".into(),
            position: [-17, 256, 50],
            delay: 0,
            priority: 0,
        },
        SavedBlockTick {
            block: "minecraft:dirt".into(),
            position: [-32, 16, 48],
            delay: 0,
            priority: 1,
        },
    ];
    let fluids = [
        SavedFluidTick {
            fluid: "water".into(),
            position: [-30, 1, 52],
            delay: 0,
            priority: 2,
        },
        SavedFluidTick {
            fluid: "lava".into(),
            position: [-30, 1, 52],
            delay: 0,
            priority: -2,
        },
        SavedFluidTick {
            fluid: "flowing_water".into(),
            position: [-17, 320, 63],
            delay: 0,
            priority: 0,
        },
    ];
    let write = |path: &std::path::Path, blocks: &[SavedBlockTick], fluids: &[SavedFluidTick]| {
        let document = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                (
                    "block_ticks".into(),
                    encode_block_ticks(pos, blocks, |id| {
                        matches!(
                            id,
                            "minecraft:air"
                                | "minecraft:sand"
                                | "minecraft:water"
                                | "minecraft:dirt"
                        )
                    })
                    .unwrap(),
                ),
                (
                    "fluid_ticks".into(),
                    encode_fluid_ticks(pos, fluids).unwrap(),
                ),
            ]),
        };
        std::fs::write(path, nbt::write(&document).unwrap()).unwrap();
    };
    let input = temp.path().join("ticks.nbt");
    let empty = temp.path().join("empty.nbt");
    let level = temp.path().join("level.nbt");
    write(&input, &blocks, &fluids);
    write(&empty, &[], &[]);
    let level_blocks = [
        SavedBlockTick {
            block: "sand".into(),
            position: [-31, 15, 48],
            delay: -3,
            priority: -1,
        },
        SavedBlockTick {
            block: "sand".into(),
            position: [-31, 15, 48],
            delay: 9,
            priority: 3,
        },
        SavedBlockTick {
            block: "water".into(),
            position: [-30, 15, 48],
            delay: 0,
            priority: 0,
        },
        SavedBlockTick {
            block: "dirt".into(),
            position: [-17, 0, 48],
            delay: 5,
            priority: -2,
        },
    ];
    let level_fluids = [
        SavedFluidTick {
            fluid: "water".into(),
            position: [-30, 1, 52],
            delay: -2,
            priority: 0,
        },
        SavedFluidTick {
            fluid: "flowing_lava".into(),
            position: [-17, 0, 48],
            delay: 16,
            priority: 1,
        },
    ];
    let doc = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![
            (
                "block_ticks".into(),
                encode_level_block_ticks(pos, &level_blocks, |id| {
                    matches!(id, "minecraft:sand" | "minecraft:water" | "minecraft:dirt")
                })
                .unwrap(),
            ),
            (
                "fluid_ticks".into(),
                encode_level_fluid_ticks(pos, &level_fluids).unwrap(),
            ),
        ]),
    };
    std::fs::write(&level, nbt::write(&doc).unwrap()).unwrap();
    run(Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(cp)
        .arg("GeneratedTicksOracle")
        .arg(input)
        .arg(empty)
        .arg(level));
}
