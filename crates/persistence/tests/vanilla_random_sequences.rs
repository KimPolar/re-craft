//! The vanilla 26.3 codec must accept the staged random-sequence document.
use native_persistence::{nbt, random_sequences::RandomSequences};
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
fn vanilla_reads_staged_random_sequence_and_continues_its_state() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
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
    let source = temp.path().join("RandomSequencesNbtOracle.java");
    std::fs::write(&source, include_str!("java/RandomSequencesNbtOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    let name = "minecraft:chests/buried_treasure";
    let seed = 987654321_i64;
    let mut sequences = RandomSequences::read(None).unwrap();
    let mut random = sequences.get(name, seed).unwrap();
    for _ in 0..13 {
        random.next_long();
    }
    sequences.set(name, &random).unwrap();
    let path = temp.path().join("random_sequences.nbt");
    std::fs::write(&path, nbt::write(&sequences.draft().unwrap()).unwrap()).unwrap();
    let result = run(Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg("RandomSequencesNbtOracle")
        .arg(path)
        .arg(name)
        .arg(seed.to_string()));
    let java: Vec<i64> = result
        .lines()
        .find_map(|line| line.strip_prefix("R "))
        .unwrap()
        .split_whitespace()
        .map(|value| value.parse().unwrap())
        .collect();
    assert_eq!(java, (0..3).map(|_| random.next_long()).collect::<Vec<_>>());
}
