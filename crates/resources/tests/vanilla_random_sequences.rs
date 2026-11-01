//! Compare named world random streams and their serialized state with 26.3.
use native_core::random::XoroshiroRandom;
use serde_json::Value;
use std::{path::PathBuf, process::Command};

fn run(command: &mut Command) -> String {
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "Java oracle failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}

#[test]
#[cfg(target_os = "windows")]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn named_sequence_initialization_and_state_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let source = temp.path().join("RandomSequencesOracle.java");
    std::fs::write(&source, include_str!("java/RandomSequencesOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg("RandomSequencesOracle"));
    assert_eq!(
        output.lines().find(|line| line.starts_with("N ")).unwrap(),
        "N {salt:0,sequences:{\"minecraft:chests/buried_treasure\":{source:[L;6278470538014359782L,7328044907969488672L]}}}"
    );
    let mut lines = output
        .lines()
        .filter(|line| line.starts_with("C ") || line.starts_with("R ") || line.starts_with("J "));
    let mut count = 0;
    while let Some(header) = lines.next() {
        let fields: Vec<_> = header.split_whitespace().collect();
        assert_eq!(fields[0], "C");
        let seed = fields[1].parse().unwrap();
        let salt = fields[2].parse().unwrap();
        let include_world = fields[3].parse::<bool>().unwrap();
        let include_id = fields[4].parse::<bool>().unwrap();
        let id = fields[5];
        let expected: Vec<i64> = lines
            .next()
            .unwrap()
            .strip_prefix("R ")
            .unwrap()
            .split_whitespace()
            .map(|value| value.parse().unwrap())
            .collect();
        let mut random = XoroshiroRandom::for_world_sequence(
            seed,
            salt,
            include_world,
            include_id.then_some(id),
        );
        let actual: Vec<_> = (0..4).map(|_| random.next_long()).collect();
        assert_eq!(
            actual, expected,
            "seed={seed}, salt={salt}, world={include_world}, id={id}, include_id={include_id}"
        );
        let json: Value =
            serde_json::from_str(lines.next().unwrap().strip_prefix("J ").unwrap()).unwrap();
        let source = json["sequences"][id]["source"].as_array().unwrap();
        assert_eq!(source.len(), 2);
        assert_eq!(
            random.state(),
            (
                source[0].as_i64().unwrap() as u64,
                source[1].as_i64().unwrap() as u64
            )
        );
        count += 1;
    }
    assert_eq!(count, 192);
}
