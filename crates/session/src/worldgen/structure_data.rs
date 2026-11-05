//! Serialize supported fresh structure data without authorizing a world save.
use anyhow::{Context, Result, ensure};
use native_persistence::nbt::{Tag, TagType};
use native_resources::structure_start::SimpleStructureKind;
use native_world::loaded::ChunkPos;
use std::collections::{BTreeMap, BTreeSet};

use super::{
    structure_batch::SimpleStarts,
    structure_references::{
        OrderedStructureReferences, StructureStartBounds, collect_ordered_structure_references,
    },
};

/// Verify that the saved reference index exactly reflects all starts in the
/// reference radius. Return source chunks that a later save plan must retain.
/// This is evidence for a save plan, not authorization to write a world.
pub(super) fn referenced_start_chunks(
    pos: ChunkPos,
    all_starts: &BTreeMap<ChunkPos, SimpleStarts>,
    references: &OrderedStructureReferences,
) -> Result<BTreeSet<ChunkPos>> {
    let recomputed = collect_ordered_structure_references(pos, |source| {
        all_starts
            .get(&source)
            .with_context(|| format!("structure starts are missing at {}, {}", source.x, source.z))?
            .iter()
            .map(|(id, start)| {
                ensure!(
                    start.chunk == [source.x, source.z],
                    "structure start belongs to another chunk"
                );
                StructureStartBounds::from_simple(id.clone(), start)
            })
            .collect()
    })?;
    ensure!(
        recomputed.membership() == references.membership(),
        "structure references differ from generated starts"
    );
    let mut required = BTreeSet::new();
    for (id, sources) in references.membership() {
        ensure!(
            references.order(id) == recomputed.order(id),
            "structure reference order differs from generated starts"
        );
        for &packed in sources {
            let bits = packed as u64;
            let source = ChunkPos {
                x: bits as u32 as i32,
                z: (bits >> 32) as u32 as i32,
            };
            let start = all_starts
                .get(&source)
                .and_then(|starts| starts.get(id))
                .context("referenced structure start is missing")?;
            validate_buried_treasure_start(source, start)?;
            required.insert(source);
        }
    }
    Ok(required)
}

fn validate_buried_treasure_start(
    pos: ChunkPos,
    start: &native_resources::structure_start::SimpleStructureStart,
) -> Result<()> {
    ensure!(
        start.kind == SimpleStructureKind::BuriedTreasure,
        "unsupported structure start serialization"
    );
    ensure!(
        start.chunk == [pos.x, pos.z],
        "structure start belongs to another chunk"
    );
    ensure!(
        start.orientation.is_none(),
        "buried treasure has an unexpected orientation"
    );
    ensure!(
        start.bounds.min == start.bounds.max,
        "buried treasure has an invalid bounding box"
    );
    let x = pos
        .x
        .checked_mul(16)
        .and_then(|v| v.checked_add(9))
        .context("structure X overflow")?;
    let z = pos
        .z
        .checked_mul(16)
        .and_then(|v| v.checked_add(9))
        .context("structure Z overflow")?;
    ensure!(
        start.bounds.min[0] == x && start.bounds.min[2] == z,
        "buried treasure has an invalid position"
    );
    Ok(())
}

/// Serialize fresh starts and ordered references belonging to one chunk.
/// Only buried treasure is supported; existing-world data must not use this.
pub(super) fn pack_structure_data(
    pos: ChunkPos,
    starts: &SimpleStarts,
    references: &OrderedStructureReferences,
) -> Result<Tag> {
    let mut packed_starts = Vec::with_capacity(starts.len());
    for (id, start) in starts {
        validate_buried_treasure_start(pos, start)?;
        let name = format!("{}:{}", id.namespace, id.path);
        let p = start.bounds.min;
        let piece = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:btp".into())),
            (
                "BB".into(),
                Tag::IntArray(vec![p[0], p[1], p[2], p[0], p[1], p[2]]),
            ),
            ("O".into(), Tag::Int(-1)),
            ("GD".into(), Tag::Int(0)),
        ]);
        let value = Tag::Compound(vec![
            ("id".into(), Tag::String(name.as_str().into())),
            ("ChunkX".into(), Tag::Int(pos.x)),
            ("ChunkZ".into(), Tag::Int(pos.z)),
            ("references".into(), Tag::Int(0)),
            (
                "Children".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![piece],
                },
            ),
        ]);
        packed_starts.push((name.as_str().into(), value));
    }
    let mut packed_references = Vec::with_capacity(references.membership().len());
    for (id, members) in references.membership() {
        let order = references
            .order(id)
            .context("structure reference order is missing")?;
        ensure!(
            order.iter().copied().collect::<BTreeSet<_>>() == *members
                && order.len() == members.len(),
            "structure reference order differs from membership"
        );
        let name = format!("{}:{}", id.namespace, id.path);
        packed_references.push((name.as_str().into(), Tag::LongArray(order.to_vec())));
    }
    Ok(Tag::Compound(vec![
        ("starts".into(), Tag::Compound(packed_starts)),
        ("References".into(), Tag::Compound(packed_references)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::super::structure_references::{
        StructureStartBounds, collect_ordered_structure_references,
    };
    use super::*;
    use native_core::beardifier::{BoundingBox, TerrainAdjustment};
    use native_resources::{packs::ResourceId, structure_start::SimpleStructureStart};

    fn fixture(pos: ChunkPos, y: i32, present: bool) -> (SimpleStarts, OrderedStructureReferences) {
        let id = ResourceId::parse("example:treasure").unwrap();
        let p = [pos.x * 16 + 9, y, pos.z * 16 + 9];
        let start = SimpleStructureStart {
            kind: SimpleStructureKind::BuriedTreasure,
            chunk: [pos.x, pos.z],
            generation_point: [pos.x * 16 + 8, 0, pos.z * 16 + 8],
            orientation: None,
            bounds: BoundingBox::new(p, p).unwrap(),
            terrain_adjustment: TerrainAdjustment::None,
        };
        let sources = [(-1, -1), (0, 0), (1, -1)];
        let references = collect_ordered_structure_references(pos, |source| {
            if present && sources.contains(&(source.x - pos.x, source.z - pos.z)) {
                Ok(vec![StructureStartBounds {
                    structure: id.clone(),
                    bounds: BoundingBox::new(p, p).unwrap(),
                }])
            } else {
                Ok(vec![])
            }
        })
        .unwrap();
        (
            if present {
                [(id, start)].into_iter().collect()
            } else {
                SimpleStarts::new()
            },
            references,
        )
    }

    #[test]
    fn rejects_unrepresentable_structure_starts() {
        let pos = ChunkPos { x: -17, z: 23 };
        let (mut starts, references) = fixture(pos, -27, true);
        let id = ResourceId::parse("example:treasure").unwrap();
        assert!(pack_structure_data(pos, &starts, &references).is_ok());
        starts.get_mut(&id).unwrap().chunk[0] += 1;
        assert!(pack_structure_data(pos, &starts, &references).is_err());
        starts.get_mut(&id).unwrap().chunk[0] -= 1;
        starts.get_mut(&id).unwrap().kind = SimpleStructureKind::SwampHut;
        assert!(pack_structure_data(pos, &starts, &references).is_err());
        starts.get_mut(&id).unwrap().kind = SimpleStructureKind::BuriedTreasure;
        starts.get_mut(&id).unwrap().bounds.max[1] += 1;
        assert!(pack_structure_data(pos, &starts, &references).is_err());
    }

    #[test]
    fn referenced_sources_require_complete_consistent_generated_starts() {
        let pos = ChunkPos { x: -17, z: 23 };
        let id = ResourceId::parse("example:treasure").unwrap();
        let (start, _) = fixture(pos, -27, true);
        let mut all = BTreeMap::new();
        for x in pos.x - 8..=pos.x + 8 {
            for z in pos.z - 8..=pos.z + 8 {
                all.insert(ChunkPos { x, z }, SimpleStarts::new());
            }
        }
        all.insert(pos, start);
        let references = collect_ordered_structure_references(pos, |source| {
            all[&source]
                .iter()
                .map(|(id, start)| StructureStartBounds::from_simple(id.clone(), start))
                .collect()
        })
        .unwrap();
        assert_eq!(
            referenced_start_chunks(pos, &all, &references).unwrap(),
            BTreeSet::from([pos])
        );
        let mut missing = all.clone();
        missing.remove(&ChunkPos {
            x: pos.x - 8,
            z: pos.z + 8,
        });
        assert!(referenced_start_chunks(pos, &missing, &references).is_err());
        let mut missing_start = all.clone();
        missing_start.get_mut(&pos).unwrap().clear();
        assert!(referenced_start_chunks(pos, &missing_start, &references).is_err());
        let empty_refs = collect_ordered_structure_references(pos, |_| Ok(vec![])).unwrap();
        assert!(referenced_start_chunks(pos, &all, &empty_refs).is_err());
        assert!(
            referenced_start_chunks(pos, &missing_start, &empty_refs)
                .unwrap()
                .is_empty()
        );
        let mut wrong_kind = all.clone();
        wrong_kind.get_mut(&pos).unwrap().get_mut(&id).unwrap().kind =
            SimpleStructureKind::SwampHut;
        assert!(referenced_start_chunks(pos, &wrong_kind, &references).is_err());
        let mut wrong_position = all.clone();
        wrong_position
            .get_mut(&pos)
            .unwrap()
            .get_mut(&id)
            .unwrap()
            .bounds
            .min[1] -= 1;
        assert!(referenced_start_chunks(pos, &wrong_position, &references).is_err());
    }

    fn canonical(tag: Tag) -> Tag {
        match tag {
            Tag::Compound(fields) => {
                let mut fields: Vec<_> = fields
                    .into_iter()
                    .map(|(key, value)| (key, canonical(value)))
                    .collect();
                fields.sort_by(|(a, _), (b, _)| a.0.cmp(&b.0));
                Tag::Compound(fields)
            }
            Tag::List {
                element_type,
                values,
            } => Tag::List {
                element_type,
                values: values.into_iter().map(canonical).collect(),
            },
            other => other,
        }
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn structure_data_matches_java_chunk_serializer() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
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
        let source = temp.path().join("StructureDataOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/StructureDataOracle.java"),
        )
        .unwrap();
        let run = |command: &mut Command| {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("StructureDataOracle")
            .arg(temp.path()));
        for (index, pos, y, present) in [
            (0, ChunkPos { x: 0, z: 0 }, 90, false),
            (1, ChunkPos { x: 0, z: 0 }, 90, true),
            (2, ChunkPos { x: -17, z: 23 }, -27, true),
        ] {
            let (starts, references) = fixture(pos, y, present);
            let actual = pack_structure_data(pos, &starts, &references).unwrap();
            let document = native_persistence::nbt::NamedTag {
                name: "".into(),
                value: actual.clone(),
            };
            std::fs::write(
                temp.path().join(format!("structure-rust-{index}.nbt")),
                native_persistence::nbt::write(&document).unwrap(),
            )
            .unwrap();
            let expected = native_persistence::nbt::read(
                &std::fs::read(temp.path().join(format!("structure-{index}.nbt"))).unwrap(),
            )
            .unwrap()
            .value;
            assert_eq!(canonical(actual), canonical(expected), "case {index}");
        }
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("StructureDataOracle")
            .arg(temp.path())
            .arg("verify"));
    }
}
