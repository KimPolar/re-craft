use native_persistence::{
    level_data::{NewLevelData, NewWorldDifficulty, NewWorldGameType, draft_new_level_data},
    nbt,
    world::{DimensionId, RespawnData},
};
use native_resources::datapacks::DataPackSettings;
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn new_level_data_matches_java_26_3() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME")
        .or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let temp = tempfile::tempdir().unwrap();
    let packs = DataPackSettings {
        enabled: vec!["vanilla".into(), "file/custom".into()],
        disabled: vec!["file/disabled".into()],
        enabled_features: [
            "minecraft:vanilla",
            "minecraft:trade_rebalance",
            "minecraft:redstone_experiments",
            "minecraft:minecart_improvements",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
    };
    let modes = [
        NewWorldGameType::Survival,
        NewWorldGameType::Creative,
        NewWorldGameType::Adventure,
        NewWorldGameType::Spectator,
    ];
    let difficulties = [
        NewWorldDifficulty::Peaceful,
        NewWorldDifficulty::Easy,
        NewWorldDifficulty::Normal,
        NewWorldDifficulty::Hard,
    ];
    for (mode_index, &game_type) in modes.iter().enumerate() {
        for (difficulty_index, &difficulty) in difficulties.iter().enumerate() {
            for flags in 0..16 {
                for config_index in 0..2 {
                    let spawn = RespawnData {
                        dimension: DimensionId::parse(if mode_index % 2 == 0 {
                            "minecraft:overworld"
                        } else {
                            "minecraft:the_nether"
                        })
                        .unwrap(),
                        position: [-123, 70, 456],
                        yaw: 45.5,
                        pitch: -12.25,
                    };
                    let brands = if flags & 2 != 0 {
                        vec!["vanilla".into(), "custom".into(), "vanilla".into()]
                    } else {
                        vec![]
                    };
                    let input = NewLevelData {
                        name: "  한글 😀  ",
                        game_type,
                        difficulty,
                        hardcore: flags & 1 != 0,
                        difficulty_locked: flags & 2 != 0,
                        allow_commands: flags & 4 != 0,
                        spawn: &spawn,
                        game_time: -123456789,
                        initialized: flags & 8 != 0,
                        last_played_ms: 1234567890123,
                        singleplayer_uuid: (flags & 1 != 0).then_some([
                            0x00112233,
                            0x44556677,
                            0x8899aabbu32 as i32,
                            0xccddeeffu32 as i32,
                        ]),
                        packs: (config_index != 0).then_some(&packs),
                        server_brands: &brands,
                        was_modded: flags & 2 != 0,
                    };
                    let document = draft_new_level_data(&input).unwrap();
                    std::fs::write(
                        temp.path().join(format!(
                            "{mode_index}-{difficulty_index}-{flags}-{config_index}.nbt"
                        )),
                        nbt::write(&document).unwrap(),
                    )
                    .unwrap();
                }
            }
        }
    }
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
    let source = temp.path().join("NewLevelDataOracle.java");
    std::fs::write(&source, include_str!("java/NewLevelDataOracle.java")).unwrap();
    let compile = Command::new(java.join("javac.exe"))
        .args(["-encoding", "UTF-8", "-cp"])
        .arg(&cp)
        .arg(source)
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(cp)
        .arg("NewLevelDataOracle")
        .arg(temp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
