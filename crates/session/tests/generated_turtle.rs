use native_persistence::nbt::{NamedTag, write};
use native_resources::{
    density::WorldgenRandom,
    worldgen_region_random::{AnimalGroupData, RegionRandom, finalize_animal_base},
};
use native_session::worldgen::draft_fresh_turtle;
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn fresh_turtle_drafts_and_random_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME")
        .or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
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
    let source = temp.path().join("GeneratedTurtleOracle.java");
    std::fs::write(&source, include_str!("java/GeneratedTurtleOracle.java")).unwrap();
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
    let mut cases = Vec::new();
    for legacy in [false, true] {
        for seed in [0_i64, 1, 42, -1, i64::MIN, i64::MAX] {
            for chance in [0.0_f32, 0.05, 1.0] {
                for position in [[3.5, 64.0, -2.5], [-0.1, -12.0, 31.9]] {
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
                    for index in 0..4 {
                        let init = finalize_animal_base(&mut random, &mut group, false);
                        let tag = draft_fresh_turtle([0, 1, 0, 2], position, 22.5, init).unwrap();
                        let path = temp
                            .path()
                            .join(format!("turtle-{}-{index}.nbt", cases.len()));
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
                    cases.push(json!({"legacy":legacy,"seed":seed,"chance":chance,"position":position,"paths":documents,"next":random.next_long()}));
                }
            }
        }
    }
    let input = temp.path().join("cases.json");
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    run(Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg("GeneratedTurtleOracle")
        .arg(input));
}
