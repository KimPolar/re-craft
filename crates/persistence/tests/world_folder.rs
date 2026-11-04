//! Differential check of the 26.3 new-world folder proposal on Windows.
#[cfg(windows)]
#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn folder_proposals_match_java_without_touching_existing_saves() {
    use native_persistence::world_creation::propose_world_folder;
    use std::{path::PathBuf, process::Command};

    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let temp = tempfile::tempdir().unwrap();
    let saves = temp.path().join("saves");
    std::fs::create_dir(&saves).unwrap();
    for name in ["World", "World (2)", "New World", "Mixed Case", "alpha (5)"] {
        std::fs::create_dir(saves.join(name)).unwrap();
    }
    std::fs::write(saves.join("World (1)"), b"keep").unwrap();
    let mut cases: Vec<String> = [
        "",
        "  ",
        "World",
        "world",
        "World (0)",
        "World (01)",
        "World (2)",
        "World (2147483647)",
        "World (2147483648)",
        "Foo ()",
        "New World",
        "Mixed Case",
        "mixed case",
        "alpha (4)",
        "alpha (5)",
        "CON",
        "con",
        "COM",
        "COM1",
        "COM9",
        "COM0",
        "LPT1",
        "LPT9",
        "LPT0",
        "CLOCK$",
        "PRN",
        "NUL",
        "AUX",
        "COM¹",
        "LPT³",
        "name.",
        "...",
        "a/b\\c:*?\"<>|`",
        "한글 😀",
        "\u{a0}",
        "\u{1}bad",
        "name\tpart",
        "x (42)",
        "x (0005)",
        "a (1) (2)",
        "  ..  ",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    for c in 0..=127u32 {
        cases.push(format!("a{}b", char::from_u32(c).unwrap()));
    }
    let input = temp.path().join("input.json");
    let output = temp.path().join("output.json");
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
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
    let classpath = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("WorldFolderOracle.java");
    std::fs::write(&source, include_str!("java/WorldFolderOracle.java")).unwrap();
    let compile = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(source)
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(classpath)
        .arg("WorldFolderOracle")
        .arg(&saves)
        .arg(input)
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let expected: Vec<String> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(expected.len(), cases.len());
    for (name, expected) in cases.iter().zip(expected) {
        assert_eq!(
            propose_world_folder(&saves, name, "New World").unwrap(),
            expected,
            "world name {name:?}"
        );
    }
    assert_eq!(std::fs::read(saves.join("World (1)")).unwrap(), b"keep");
}
