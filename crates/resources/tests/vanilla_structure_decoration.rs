//! Compare structure registry decoration order and random seeds with Java 26.3.
use native_resources::{packs::ResourceStack, structure_decoration::StructureDecorationSchedule};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

fn run(command: &mut Command) -> String {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "Java oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn structure_decoration_registry_order_and_random_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let classpath = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("StructureDecorationOracle.java");
    std::fs::write(&source, include_str!("java/StructureDecorationOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&classpath)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&classpath)
        .arg("StructureDecorationOracle"));
    let mut resources = ResourceStack::open(&game).unwrap();
    let schedule = StructureDecorationSchedule::load(&mut resources).unwrap();
    let mut seen = 0;
    for line in output.lines().filter(|line| line.starts_with("S ")) {
        let parts: Vec<_> = line.split_whitespace().collect();
        let step: usize = parts[1].parse().unwrap();
        let index: usize = parts[2].parse().unwrap();
        let entry = &schedule.steps()[step][index];
        assert_eq!(
            format!("{}:{}", entry.id.namespace, entry.id.path),
            parts[3]
        );
        assert_eq!((entry.step, entry.index), (step as i32, index as i32));
        let mut random = entry.random(parts[4].parse().unwrap());
        for expected in &parts[5..8] {
            assert_eq!(
                random.next_long(),
                expected.parse::<i64>().unwrap(),
                "{line}"
            );
        }
        seen += 1;
    }
    assert_eq!(
        seen,
        schedule.steps().iter().map(Vec::len).sum::<usize>() * 5
    );
    assert!(seen > 0);
}
