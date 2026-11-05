//! Compare reference discovery with Java 26.3's ChunkGenerator.createReferences.
use std::{collections::BTreeMap, path::PathBuf, process::Command};

use native_core::beardifier::{BoundingBox, TerrainAdjustment};
use native_resources::packs::ResourceId;
use native_session::worldgen::structure_references::{
    StructureStartBounds, collect_ordered_structure_references, collect_structure_references,
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
                let (id, adjustment) = match (dx + dz).rem_euclid(5) {
                    0 => ("minecraft:buried_treasure", TerrainAdjustment::None),
                    1 => ("minecraft:stronghold", TerrainAdjustment::Bury),
                    2 => ("minecraft:village_plains", TerrainAdjustment::BeardThin),
                    3 => ("minecraft:ancient_city", TerrainAdjustment::BeardBox),
                    _ => ("minecraft:trial_chambers", TerrainAdjustment::Encapsulate),
                };
                // Exercise inclusive edges, non-overlap, Y independence, and the eight-chunk limit.
                let edge_x = target.x * 16 + (dx * 3 + dz).rem_euclid(50) - 17;
                let edge_z = target.z * 16 + (dz * 5 - dx).rem_euclid(50) - 17;
                let mut pieces = vec![
                    BoundingBox::new([edge_x, -300, edge_z], [edge_x + 1, -290, edge_z + 1])
                        .unwrap(),
                ];
                if (dx - dz).rem_euclid(2) == 0 {
                    pieces.push(
                        BoundingBox::new(
                            [edge_x + 5, -320, edge_z - 7],
                            [edge_x + 7, -310, edge_z - 4],
                        )
                        .unwrap(),
                    );
                }
                starts.entry(source).or_default().push(
                    StructureStartBounds::from_pieces(
                        ResourceId::parse(id).unwrap(),
                        &pieces,
                        adjustment,
                    )
                    .unwrap(),
                );
                let boxes: Vec<_> = pieces
                    .iter()
                    .map(|bounds| {
                        json!([
                            bounds.min[0],
                            bounds.min[1],
                            bounds.min[2],
                            bounds.max[0],
                            bounds.max[1],
                            bounds.max[2]
                        ])
                    })
                    .collect();
                records.push(json!({"x":source.x,"z":source.z,"id":id,"pieces":boxes}));
            }
        }
        cases.push(json!({"x":target.x,"z":target.z,"starts":records}));
        expected_inputs.push((target, starts));
    }
    // More than 24 references triggers fastutil rehashing; include zero,
    // negative packed coordinates, and collisions across the whole radius.
    let target = ChunkPos { x: 0, z: 0 };
    let id = ResourceId::parse("minecraft:buried_treasure").unwrap();
    let mut starts = BTreeMap::new();
    let mut records = Vec::new();
    for x in -8..=8 {
        for z in -8..=8 {
            let source = ChunkPos { x, z };
            let bounds = BoundingBox::new([0, -10, 0], [15, 10, 15]).unwrap();
            starts.insert(
                source,
                vec![
                    StructureStartBounds::from_pieces(
                        id.clone(),
                        &[bounds],
                        TerrainAdjustment::None,
                    )
                    .unwrap(),
                ],
            );
            records.push(
                json!({"x":x,"z":z,"id":"minecraft:buried_treasure","pieces":[[0,-10,0,15,10,15]]}),
            );
        }
    }
    cases.push(json!({"x":0,"z":0,"starts":records}));
    expected_inputs.push((target, starts));
    let input = temp.path().join("cases.json");
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&classpath)
        .arg("StructureReferencesOracle")
        .arg(input));
    let mut groups = Vec::new();
    let mut java_refs = BTreeMap::<ResourceId, std::collections::BTreeSet<i64>>::new();
    let mut ordered_groups = Vec::new();
    let mut java_order = BTreeMap::<ResourceId, Vec<i64>>::new();
    let mut bounding_groups = Vec::new();
    let mut java_bounds = BTreeMap::<(ChunkPos, ResourceId), BoundingBox>::new();
    for line in output.lines() {
        let parts: Vec<_> = line.split_whitespace().collect();
        if parts.first() == Some(&"BOUND") {
            let number = |index: usize| parts[index].parse::<i32>().unwrap();
            let pos = ChunkPos {
                x: number(1),
                z: number(2),
            };
            let id = ResourceId::parse(parts[3]).unwrap();
            let bounds = BoundingBox::new(
                [number(4), number(5), number(6)],
                [number(7), number(8), number(9)],
            )
            .unwrap();
            assert!(java_bounds.insert((pos, id), bounds).is_none());
        } else if parts.first() == Some(&"REF") {
            java_order
                .entry(ResourceId::parse(parts[1]).unwrap())
                .or_default()
                .push(parts[2].parse().unwrap());
            java_refs
                .entry(ResourceId::parse(parts[1]).unwrap())
                .or_default()
                .insert(parts[2].parse().unwrap());
        } else if parts.first() == Some(&"END") {
            groups.push(std::mem::take(&mut java_refs));
            ordered_groups.push(std::mem::take(&mut java_order));
            bounding_groups.push(std::mem::take(&mut java_bounds));
        }
    }
    assert_eq!(groups.len(), expected_inputs.len());
    for name in [
        "minecraft:buried_treasure",
        "minecraft:stronghold",
        "minecraft:village_plains",
        "minecraft:ancient_city",
        "minecraft:trial_chambers",
    ] {
        assert!(
            groups
                .iter()
                .any(|group| group.contains_key(&ResourceId::parse(name).unwrap())),
            "missing fixture for {name}"
        );
    }
    for ((((target, starts), expected), expected_bounds), expected_order) in expected_inputs
        .into_iter()
        .zip(groups)
        .zip(bounding_groups)
        .zip(ordered_groups)
    {
        assert!(!expected.is_empty());
        let actual_bounds: BTreeMap<_, _> = starts
            .iter()
            .flat_map(|(&pos, starts)| {
                starts
                    .iter()
                    .map(move |start| ((pos, start.structure.clone()), start.bounds))
            })
            .collect();
        assert_eq!(
            actual_bounds, expected_bounds,
            "structure bounds at target {target:?}"
        );
        let actual = collect_structure_references(target, |source| {
            Ok(starts.get(&source).cloned().unwrap_or_default())
        })
        .unwrap();
        assert_eq!(actual, expected, "target {target:?}");
        let ordered = collect_ordered_structure_references(target, |source| {
            Ok(starts.get(&source).cloned().unwrap_or_default())
        })
        .unwrap();
        for (id, java) in expected_order {
            assert_eq!(
                ordered.order(&id),
                Some(java.as_slice()),
                "reference order for {id:?} at {target:?}"
            );
        }
    }
}
