//! Full stronghold ring positions using the installed 26.3 biome source.
use native_resources::{
    density::DensityEvaluator,
    packs::{ResourceId, ResourceProvider, ResourceStack},
    structure_biomes::StructureBiomes,
    structure_placement::StructureSetCache,
};
use native_session::worldgen::TerrainGenerator;
use serde_json::Value;
use std::{path::PathBuf, process::Command};
fn run(command: &mut Command) -> String {
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "Java stronghold oracle failed: {}\n{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout[out.stdout.len().saturating_sub(2500)..])
    );
    String::from_utf8(out.stdout).unwrap()
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn stronghold_positions_match_vanilla_biomes() {
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
                paths.push(p)
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("VanillaStrongholdsOracle.java");
    std::fs::write(&source, include_str!("java/VanillaStrongholdsOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("VanillaStrongholdsOracle")
        .arg(temp.path())
        .args(["0", "2345"]));
    let mut groups = Vec::<Vec<[i32; 2]>>::new();
    for line in output.lines() {
        let p: Vec<_> = line.split_whitespace().collect();
        match p.first().copied() {
            Some("S") => groups.push(Vec::new()),
            Some("P") => groups
                .last_mut()
                .unwrap()
                .push([p[1].parse().unwrap(), p[2].parse().unwrap()]),
            _ => {}
        }
    }
    assert_eq!(groups.len(), 2);
    let mut resources = ResourceStack::open(&game).unwrap();
    let preset: Value = serde_json::from_slice(
        &resources
            .read_builtin_data("data/minecraft/worldgen/world_preset/normal.json")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let id = ResourceId::parse("minecraft:strongholds").unwrap();
    let rules = StructureBiomes::load(
        &mut resources,
        [ResourceId::parse("minecraft:stronghold").unwrap()],
    )
    .unwrap();
    for (i, seed) in [0, 2345].into_iter().enumerate() {
        let generator = TerrainGenerator::without_terrain_adjustments(
            &mut resources,
            &preset["dimensions"]["minecraft:overworld"]["generator"],
            seed,
            -64,
            384,
        )
        .unwrap();
        let mut cache = StructureSetCache::default();
        let mut evaluator = DensityEvaluator::default();
        assert!(
            generator
                .is_structure_candidate(
                    &mut resources,
                    &mut evaluator,
                    &mut cache,
                    &rules,
                    &id,
                    groups[i][0]
                )
                .unwrap()
        );
        let positions = cache.cached_ring_positions(&id, seed).unwrap();
        assert_eq!(positions.len(), 128);
        assert_eq!(positions, groups[i], "seed {seed}");
        assert!(
            !generator
                .is_structure_candidate(
                    &mut resources,
                    &mut evaluator,
                    &mut cache,
                    &rules,
                    &id,
                    [1_000_000, 1_000_000]
                )
                .unwrap()
        );
    }
}
