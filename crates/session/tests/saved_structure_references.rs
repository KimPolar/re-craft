use native_persistence::nbt::{self, NamedTag, Tag, TagType};
use native_resources::packs::ResourceId;
use native_session::worldgen::structure_references::{
    StructureStartBounds, collect_ordered_structure_references,
    collect_ordered_structure_references_with_saved, restore_saved_structure_references,
};
use native_world::loaded::ChunkPos;
use std::{path::PathBuf, process::Command};

fn run(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn packed(x: i32, z: i32) -> i64 {
    (((z as u32 as u64) << 32) | x as u32 as u64) as i64
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn saved_structure_reference_order_matches_java_26_3() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: serde_json::Value =
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
    let cp = std::env::join_paths(paths).unwrap();
    let oracle = temp.path().join("SavedStructureReferencesOracle.java");
    std::fs::write(
        &oracle,
        include_str!("java/SavedStructureReferencesOracle.java"),
    )
    .unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(&oracle));
    let ids = [
        ResourceId::parse("minecraft:buried_treasure").unwrap(),
        ResourceId::parse("minecraft:swamp_hut").unwrap(),
    ];
    let mut cases = Vec::new();
    for target in [
        ChunkPos { x: 0, z: 0 },
        ChunkPos { x: -7, z: 13 },
        ChunkPos {
            x: i32::MAX,
            z: i32::MIN,
        },
    ] {
        for count in [0usize, 1, 2, 12, 24, 35, 100, 289, 360] {
            let mut values = Vec::new();
            for i in 0..count {
                let n = (i * 137) % 289;
                let dx = (n % 17) as i32 - 8;
                let dz = (n / 17) as i32 - 8;
                values.push(packed(target.x.wrapping_add(dx), target.z.wrapping_add(dz)));
            }
            values.extend([
                packed(target.x.wrapping_add(9), target.z),
                packed(target.x.wrapping_add(i32::MIN), target.z),
            ]);
            let references = Tag::Compound(vec![
                (
                    "minecraft:buried_treasure".into(),
                    Tag::LongArray(values.clone()),
                ),
                (
                    "minecraft:swamp_hut".into(),
                    if count == 12 {
                        Tag::Int(7)
                    } else {
                        Tag::LongArray(values.into_iter().rev().collect())
                    },
                ),
                ("example:unknown".into(), Tag::LongArray(vec![0])),
            ]);
            let decoded = restore_saved_structure_references(target, Some(&references), |id| {
                Ok(ids.contains(id))
            })
            .unwrap();
            assert!(!decoded.discarded.is_empty());
            let expected = Tag::Compound(
                ids.iter()
                    .filter_map(|id| {
                        let key = format!("{}:{}", id.namespace, id.path);
                        Some((
                            key.as_str().into(),
                            Tag::LongArray(decoded.references.order(id)?.to_vec()),
                        ))
                    })
                    .collect(),
            );
            cases.push(Tag::Compound(vec![
                ("x".into(), Tag::Int(target.x)),
                ("z".into(), Tag::Int(target.z)),
                (
                    "structures".into(),
                    Tag::Compound(vec![("References".into(), references)]),
                ),
                ("expected".into(), expected),
            ]));
        }
    }
    // Encode generated sets in the same form used by chunk drafts, then let
    // Java rebuild each fastutil set from those arrays.
    let target = ChunkPos { x: -7, z: 13 };
    let bounds = native_core::beardifier::BoundingBox::new([-112, 0, 208], [-97, 15, 223]).unwrap();
    for count in [1usize, 2, 12, 24, 100, 289] {
        let generated = collect_ordered_structure_references(target, |source| {
            let dx = source.x - target.x + 8;
            let dz = source.z - target.z + 8;
            Ok(if (dx * 17 + dz) < count as i32 {
                vec![StructureStartBounds {
                    structure: ids[0].clone(),
                    bounds,
                }]
            } else {
                vec![]
            })
        })
        .unwrap();
        let references = generated.to_tag();
        let decoded =
            restore_saved_structure_references(
                target,
                Some(&references),
                |id| Ok(ids.contains(id)),
            )
            .unwrap();
        let expected = Tag::Compound(vec![(
            "minecraft:buried_treasure".into(),
            Tag::LongArray(decoded.references.order(&ids[0]).unwrap().to_vec()),
        )]);
        cases.push(Tag::Compound(vec![
            ("x".into(), Tag::Int(target.x)),
            ("z".into(), Tag::Int(target.z)),
            (
                "structures".into(),
                Tag::Compound(vec![("References".into(), references)]),
            ),
            ("expected".into(), expected),
        ]));
    }
    // Reuse the sets created by Java's saved-chunk decoder before inserting
    // new starts, including duplicate entries and fastutil rehash boundaries.
    for target in [ChunkPos { x: 0, z: 0 }, ChunkPos { x: -7, z: 13 }] {
        let bounds = native_core::beardifier::BoundingBox::new(
            [target.x * 16, 0, target.z * 16],
            [target.x * 16 + 15, 15, target.z * 16 + 15],
        )
        .unwrap();
        for count in [0usize, 1, 2, 12, 24, 100, 289] {
            let values: Vec<_> = (0..count)
                .map(|i| {
                    let n = (i * 137) % 289;
                    packed(
                        target.x + (n / 17) as i32 - 8,
                        target.z + (n % 17) as i32 - 8,
                    )
                })
                .collect();
            let original = Tag::Compound(vec![
                ("minecraft:buried_treasure".into(), Tag::LongArray(values)),
                ("minecraft:swamp_hut".into(), Tag::LongArray(vec![])),
            ]);
            let saved = restore_saved_structure_references(target, Some(&original), |id| {
                Ok(ids.contains(id))
            })
            .unwrap();
            assert!(saved.discarded.is_empty());
            let mut additions = Vec::new();
            let merged =
                collect_ordered_structure_references_with_saved(target, &saved, |source| {
                    let dx = source.x - target.x + 8;
                    let dz = source.z - target.z + 8;
                    let mut starts = Vec::new();
                    for (index, divisor) in [(0usize, 3), (1usize, 11)] {
                        if (dx * 17 + dz) % divisor == 0 {
                            additions.push(Tag::Compound(vec![
                                (
                                    "id".into(),
                                    Tag::String(
                                        format!("{}:{}", ids[index].namespace, ids[index].path)
                                            .as_str()
                                            .into(),
                                    ),
                                ),
                                ("value".into(), Tag::Long(packed(source.x, source.z))),
                            ]));
                            starts.push(StructureStartBounds {
                                structure: ids[index].clone(),
                                bounds,
                            });
                        }
                    }
                    Ok(starts)
                })
                .unwrap();
            cases.push(Tag::Compound(vec![
                ("x".into(), Tag::Int(target.x)),
                ("z".into(), Tag::Int(target.z)),
                (
                    "structures".into(),
                    Tag::Compound(vec![("References".into(), original)]),
                ),
                (
                    "additions".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: additions,
                    },
                ),
                ("expected".into(), merged.to_tag()),
            ]));
        }
    }
    let invalid = Tag::Int(7);
    let decoded =
        restore_saved_structure_references(ChunkPos { x: 0, z: 0 }, Some(&invalid), |_| Ok(true))
            .unwrap();
    assert!(!decoded.discarded.is_empty());
    assert!(decoded.references.membership().is_empty());
    cases.push(Tag::Compound(vec![
        ("x".into(), Tag::Int(0)),
        ("z".into(), Tag::Int(0)),
        (
            "structures".into(),
            Tag::Compound(vec![("References".into(), invalid)]),
        ),
        ("expected".into(), Tag::Compound(vec![])),
    ]));
    let document = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![(
            "cases".into(),
            Tag::List {
                element_type: TagType::Compound,
                values: cases,
            },
        )]),
    };
    let input = temp.path().join("cases.nbt");
    std::fs::write(&input, nbt::write(&document).unwrap()).unwrap();
    run(Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg("SavedStructureReferencesOracle")
        .arg(input));
}
