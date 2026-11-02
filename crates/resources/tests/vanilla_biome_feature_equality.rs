use native_resources::{
    packs::{ResourceProvider, ResourceStack},
    placed_feature::PlacedFeature,
};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

fn run(command: &mut Command) -> String {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "Java biome feature oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn height_provider_identity_controls_java_biome_membership() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"].as_array().unwrap() {
        if let Some(p) = lib["downloads"]["artifact"]["path"].as_str() {
            let p = game.join("libraries").join(p);
            if p.is_file() {
                paths.push(p);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("BiomeFeatureEqualityOracle.java");
    std::fs::write(
        &source,
        include_str!("java/BiomeFeatureEqualityOracle.java"),
    )
    .unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("BiomeFeatureEqualityOracle"));
    let lines: Vec<_> = output
        .lines()
        .filter(|line| {
            line.starts_with("uniform ")
                || line.starts_with("trapezoid ")
                || line.starts_with("none ")
        })
        .collect();
    assert_eq!(
        lines,
        [
            "uniform true false",
            "trapezoid true false",
            "none true true"
        ]
    );

    let mut resources = ResourceStack::open(&game).unwrap();
    let ids = resources.list_data("worldgen/placed_feature").unwrap();
    let mut count = 0;
    for id in ids
        .into_iter()
        .filter(|id| id.namespace == "minecraft" && id.path.starts_with("ore_"))
    {
        let placed = PlacedFeature::load(&mut resources, &id)
            .unwrap_or_else(|e| panic!("{}:{}: {e:#}", id.namespace, id.path));
        assert!(
            placed.has_independent_height_provider(),
            "ore {} has no independent height provider",
            id.path
        );
        count += 1;
    }
    assert_eq!(count, 40);
}
