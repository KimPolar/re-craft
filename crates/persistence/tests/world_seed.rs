//! Differential check against the installed Java Edition seed parser.
use std::{path::PathBuf, process::Command};

use native_core::world_seed::{RandomWorldSeed, parse_seed};

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn create_world_seed_text_matches_java_26_3() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME")
        .or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let mut classpath = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for library in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                classpath.push(path);
            }
        }
    }
    let classpath = std::env::join_paths(classpath).unwrap();
    let source = temp.path().join("WorldSeedOracle.java");
    std::fs::write(&source, include_str!("java/WorldSeedOracle.java")).unwrap();
    let random_source = temp.path().join("RandomWorldSeedOracle.java");
    std::fs::write(
        &random_source,
        include_str!("java/RandomWorldSeedOracle.java"),
    )
    .unwrap();
    let compile = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(source)
        .arg(random_source)
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );

    let mut cases: Vec<String> = [
        "",
        " \t\0\r\n",
        "0",
        "-0",
        "+0",
        "+",
        "-",
        "--1",
        " 42 ",
        "9223372036854775807",
        "9223372036854775808",
        "-9223372036854775808",
        "-9223372036854775809",
        "000000000000000000000000001",
        "１２٣",
        "😀",
        "𝟡",
        "\u{a0}",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    for code in 0..=0xffff {
        if let Some(c) = char::from_u32(code) {
            cases.push(c.to_string());
            cases.push(format!("1{c}2"));
            cases.push(format!(" -{c}9 "));
        }
    }
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(cases.len() as u32).to_be_bytes());
    for case in &cases {
        let units: Vec<_> = case.encode_utf16().collect();
        bytes.extend_from_slice(&(units.len() as u32).to_be_bytes());
        for unit in units {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
    }
    let input = temp.path().join("input.bin");
    let output = temp.path().join("output.bin");
    std::fs::write(&input, bytes).unwrap();
    let run = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg("WorldSeedOracle")
        .arg(&input)
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let results = std::fs::read(&output).unwrap();
    assert_eq!(results.len(), cases.len() * 9);
    for (case, row) in cases.iter().zip(results.chunks_exact(9)) {
        let value = i64::from_be_bytes(row[1..].try_into().unwrap());
        let expected = (row[0] != 0).then_some(value);
        assert_eq!(parse_seed(case), expected, "seed input {case:?}");
    }

    let clocks: Vec<i64> = (0..4096i64)
        .map(|i| i.wrapping_mul(6364136223846793005).wrapping_add(i64::MIN))
        .collect();
    let mut bytes = Vec::with_capacity(4 + clocks.len() * 8);
    bytes.extend_from_slice(&(clocks.len() as u32).to_be_bytes());
    for clock in &clocks {
        bytes.extend_from_slice(&clock.to_be_bytes());
    }
    std::fs::write(&input, bytes).unwrap();
    let run = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(classpath)
        .arg("RandomWorldSeedOracle")
        .arg(&input)
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let results = std::fs::read(output).unwrap();
    assert_eq!(results.len(), clocks.len() * 8);
    let generator = RandomWorldSeed::new();
    for (clock, row) in clocks.into_iter().zip(results.chunks_exact(8)) {
        let expected = i64::from_be_bytes(row.try_into().unwrap());
        assert_eq!(
            generator.next_with_clock(|| clock),
            expected,
            "clock {clock}"
        );
    }
}
