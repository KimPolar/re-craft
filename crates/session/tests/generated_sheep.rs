use native_persistence::nbt::{NamedTag, write};
use native_resources::{
    density::WorldgenRandom,
    worldgen_region_random::{AnimalGroupData, RegionRandom, finalize_sheep},
};
use native_session::worldgen::draft_fresh_sheep;
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn fresh_sheep_drafts_match_java_entities() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
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
    let source = temp.path().join("GeneratedSheepOracle.java");
    std::fs::write(&source, include_str!("java/GeneratedSheepOracle.java")).unwrap();
    let run = |command: &mut Command| {
        let result = command.output().unwrap();
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
        .arg(&source));
    let mut cases = Vec::new();
    for legacy in [false, true] {
        for seed in [0_i64, 1, 42, -1, i64::MIN, i64::MAX] {
            for (warm, cold) in [(false, false), (true, false), (false, true), (true, true)] {
                for chance in [0.05_f32, 1.0] {
                    let algorithm = if legacy {
                        WorldgenRandom::Legacy
                    } else {
                        WorldgenRandom::Xoroshiro
                    };
                    let mut random = RegionRandom::new(seed, algorithm, -13, 17);
                    let mut group = AnimalGroupData {
                        baby_spawn_chance: chance,
                        ..Default::default()
                    };
                    let mut documents = Vec::new();
                    for index in 0..3 {
                        let init = finalize_sheep(&mut random, &mut group, false, warm, cold);
                        let tag =
                            draft_fresh_sheep([0, 1, 0, 2], [3.5, 10.0, -2.5], 22.5, init).unwrap();
                        let path = temp
                            .path()
                            .join(format!("sheep-{}-{index}.nbt", cases.len()));
                        std::fs::write(
                            &path,
                            write(&NamedTag {
                                name: "".into(),
                                value: tag,
                            })
                            .unwrap(),
                        )
                        .unwrap();
                        documents.push(path);
                    }
                    cases.push(json!({"legacy":legacy,"seed":seed,"x":-13,"z":17,"warm":warm,"cold":cold,"chance":chance,"paths":documents}));
                }
            }
        }
    }
    let pink_seed = (0_i64..100_000)
        .find(|&seed| {
            let mut random = RegionRandom::new(seed, WorldgenRandom::Legacy, -13, 17);
            let mut group = AnimalGroupData::default();
            finalize_sheep(&mut random, &mut group, false, false, false).color
                == native_resources::worldgen_region_random::SheepSpawnColor::Pink
        })
        .unwrap();
    let mut random = RegionRandom::new(pink_seed, WorldgenRandom::Legacy, -13, 17);
    let mut group = AnimalGroupData::default();
    let init = finalize_sheep(&mut random, &mut group, false, false, false);
    let path = temp.path().join("pink-sheep.nbt");
    std::fs::write(
        &path,
        write(&NamedTag {
            name: "".into(),
            value: draft_fresh_sheep([0, 1, 0, 2], [3.5, 10.0, -2.5], 22.5, init).unwrap(),
        })
        .unwrap(),
    )
    .unwrap();
    cases.push(json!({"legacy":true,"seed":pink_seed,"x":-13,"z":17,"warm":false,"cold":false,"chance":0.05,"paths":[path]}));
    let input = temp.path().join("cases.json");
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    run(Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg("GeneratedSheepOracle")
        .arg(input));
}
