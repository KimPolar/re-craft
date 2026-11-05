//! Compare reference discovery with Java 26.3's ChunkGenerator.createReferences.
use std::{collections::BTreeMap, path::PathBuf, process::Command};

use native_core::beardifier::BoundingBox;
use native_resources::packs::ResourceId;
use native_session::worldgen::structure_references::{
    StructureStartBounds, collect_structure_references,
};
use native_world::loaded::ChunkPos;
use serde_json::{Value, json};

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
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn generated_structure_references_match_java() {
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
    let source = temp.path().join("StructureReferencesOracle.java");
    std::fs::write(&source, include_str!("java/StructureReferencesOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&classpath)
        .arg(&source));

    let mut cases = Vec::new();
    let mut expected_inputs = Vec::new();
    for target in [
        ChunkPos { x: 0, z: 0 },
        ChunkPos { x: -17, z: 23 },
        ChunkPos { x: 300, z: -100 },
    ] {
        let mut starts: BTreeMap<ChunkPos, Vec<StructureStartBounds>> = BTreeMap::new();
        let mut records = Vec::new();
        for dx in -9i32..=9 {
            for dz in -9i32..=9 {
                if (dx * 19 + dz * 7).rem_euclid(5) != 0 {
                    continue;
                }
                let source = ChunkPos {
                    x: target.x + dx,
                    z: target.z + dz,
                };
                let id = if (dx + dz).rem_euclid(2) == 0 {
                    "minecraft:buried_treasure"
                } else {
                    "minecraft:desert_pyramid"
                };
                // Exercise inclusive edges, non-overlap, Y independence, and the eight-chunk limit.
                let edge_x = target.x * 16 + (dx * 3 + dz).rem_euclid(20) - 2;
                let edge_z = target.z * 16 + (dz * 5 - dx).rem_euclid(20) - 2;
                let bounds =
                    BoundingBox::new([edge_x, -300, edge_z], [edge_x + 1, -290, edge_z + 1])
                        .unwrap();
                starts
                    .entry(source)
                    .or_default()
                    .push(StructureStartBounds {
                        structure: ResourceId::parse(id).unwrap(),
                        bounds,
                    });
                records.push(json!({"x":source.x,"z":source.z,"id":id,"bounds":[bounds.min[0],bounds.min[1],bounds.min[2],bounds.max[0],bounds.max[1],bounds.max[2]]}));
            }
        }
        cases.push(json!({"x":target.x,"z":target.z,"starts":records}));
        expected_inputs.push((target, starts));
    }
    let input = temp.path().join("cases.json");
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&classpath)
        .arg("StructureReferencesOracle")
        .arg(input));
    let mut groups = Vec::new();
    let mut java_refs = BTreeMap::<ResourceId, std::collections::BTreeSet<i64>>::new();
    for line in output.lines() {
        let parts: Vec<_> = line.split_whitespace().collect();
        if parts.first() == Some(&"REF") {
            java_refs
                .entry(ResourceId::parse(parts[1]).unwrap())
                .or_default()
                .insert(parts[2].parse().unwrap());
        } else if parts.first() == Some(&"END") {
            groups.push(std::mem::take(&mut java_refs));
        }
    }
    assert_eq!(groups.len(), expected_inputs.len());
    for ((target, starts), expected) in expected_inputs.into_iter().zip(groups) {
        assert!(!expected.is_empty());
        let actual = collect_structure_references(target, |source| {
            Ok(starts.get(&source).cloned().unwrap_or_default())
        })
        .unwrap();
        assert_eq!(actual, expected, "target {target:?}");
    }
}
