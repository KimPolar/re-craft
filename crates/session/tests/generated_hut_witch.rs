use native_persistence::nbt::{NamedTag, write};
use native_resources::{
    density::WorldgenRandom,
    worldgen_region_random::{RegionRandom, finalize_mob_base},
};
use native_session::worldgen::draft_fresh_hut_witch;
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn fresh_hut_witches_match_java_nbt_and_region_random() {
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
    let source = temp.path().join("GeneratedHutWitchOracle.java");
    std::fs::write(&source, include_str!("java/GeneratedHutWitchOracle.java")).unwrap();
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
            let algorithm = if legacy {
                WorldgenRandom::Legacy
            } else {
                WorldgenRandom::Xoroshiro
            };
            let mut random = RegionRandom::new(seed, algorithm, -13, 17);
            let init = finalize_mob_base(&mut random, false);
            let tag = draft_fresh_hut_witch([0, 1, 0, 2], [3, 10, -3], init).unwrap();
            let path = temp.path().join(format!("witch-{}.nbt", cases.len()));
            std::fs::write(
                &path,
                write(&NamedTag {
                    name: "".into(),
                    value: tag,
                })
                .unwrap(),
            )
            .unwrap();
            cases.push(json!({"legacy":legacy,"seed":seed,"path":path,"next":random.next_long()}));
        }
    }
    let input = temp.path().join("cases.json");
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    run(Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg("GeneratedHutWitchOracle")
        .arg(input));
}
