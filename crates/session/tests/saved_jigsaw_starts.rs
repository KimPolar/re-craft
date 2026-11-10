use native_persistence::nbt::{self, Tag, TagType};
use native_resources::packs::ResourceId;
use native_session::worldgen::saved_structure_starts::{
    RestoredStructurePiece, RestoredStructureStart, restore_saved_structure_starts,
    saved_beardifier_for_chunk,
};
use native_session::worldgen::structure_references::restore_saved_structure_references;
use native_world::loaded::ChunkPos;
use std::{path::PathBuf, process::Command};

fn run(command: &mut Command) -> String {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
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
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn restored_jigsaw_starts_use_java_piece_data() {
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
    let oracle = temp.path().join("JigsawPieceOracle.java");
    std::fs::write(
        &oracle,
        include_str!("../../persistence/tests/java/JigsawPieceOracle.java"),
    )
    .unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(&oracle));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg("JigsawPieceOracle")
        .arg(temp.path()));
    let id = ResourceId::parse("example:jigsaw").unwrap();
    let mut count = 0;
    for line in output.lines() {
        let fields: Vec<_> = line.split('\t').collect();
        let ["T", kind, _, _, filename] = fields.as_slice() else {
            continue;
        };
        let piece = nbt::read(&std::fs::read(temp.path().join(filename)).unwrap())
            .unwrap()
            .value;
        let start = Tag::Compound(vec![
            ("id".into(), Tag::String("example:jigsaw".into())),
            ("ChunkX".into(), Tag::Int(0)),
            ("ChunkZ".into(), Tag::Int(0)),
            ("references".into(), Tag::Int(0)),
            (
                "Children".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![piece.clone()],
                },
            ),
        ]);
        let data = Tag::Compound(vec![("example:jigsaw".into(), start)]);
        let restored = restore_saved_structure_starts(ChunkPos { x: 0, z: 0 }, Some(&data), |_| {
            Ok(Some(
                serde_json::json!({"type":"minecraft:jigsaw","terrain_adaptation":"beard_thin"}),
            ))
        })
        .unwrap();
        let RestoredStructureStart::Present { pieces, .. } = &restored[&id] else {
            panic!("missing start")
        };
        let RestoredStructurePiece::Jigsaw(saved) = &pieces[0] else {
            panic!("wrong piece")
        };
        assert_eq!(canonical(saved.to_tag().unwrap()), canonical(piece));
        let bounds = restored[&id].reference_bounds(&id).unwrap().unwrap().bounds;
        assert_eq!(bounds.min, [-14, -10, -15]);
        assert_eq!(bounds.max, [17, 20, 21]);
        let terrain = restored[&id].terrain_pieces().unwrap();
        assert_eq!(terrain.len(), 1);
        assert_eq!(terrain[0].ground_delta, 4);
        assert_eq!(
            terrain[0]
                .junctions
                .iter()
                .map(|j| j.position)
                .collect::<Vec<_>>(),
            vec![[12, -5, 7], [-4, 60, 2]]
        );
        let projection = if *kind == "0" || *kind == "1" {
            native_core::beardifier::PieceProjection::Rigid
        } else {
            native_core::beardifier::PieceProjection::TerrainMatching
        };
        assert_eq!(terrain[0].projection, projection);
        let target = ChunkPos { x: 0, z: 0 };
        let refs = Tag::Compound(vec![("example:jigsaw".into(), Tag::LongArray(vec![0]))]);
        let refs = restore_saved_structure_references(target, Some(&refs), |_| Ok(true)).unwrap();
        let starts = std::collections::BTreeMap::from([(target, restored)]);
        let beardifier =
            saved_beardifier_for_chunk(target, &refs, std::slice::from_ref(&id), &starts).unwrap();
        let expected = native_core::beardifier::Beardifier::for_chunk(
            [0, 0],
            &[native_core::beardifier::StructurePiece {
                bounds: native_core::beardifier::BoundingBox::new([-2, 2, -3], [5, 8, 9]).unwrap(),
                adjustment: native_core::beardifier::TerrainAdjustment::BeardThin,
                ground_delta: 4,
                projection,
                junctions: vec![
                    native_core::beardifier::Junction {
                        position: [12, -5, 7],
                    },
                    native_core::beardifier::Junction {
                        position: [-4, 60, 2],
                    },
                ],
            }],
        )
        .unwrap();
        assert_eq!(beardifier.affected_bounds(), expected.affected_bounds());
        for position in [[0, 0, 0], [12, -5, 7], [-4, 60, 2], [3, 5, 3]] {
            assert_eq!(
                beardifier
                    .sample(position[0], position[1], position[2])
                    .to_bits(),
                expected
                    .sample(position[0], position[1], position[2])
                    .to_bits()
            );
        }
        assert!(saved_beardifier_for_chunk(target, &refs, &[], &starts).is_err());
        assert!(
            saved_beardifier_for_chunk(
                target,
                &refs,
                std::slice::from_ref(&id),
                &Default::default()
            )
            .is_err()
        );
        count += 1;
    }
    assert_eq!(count, 32);
}
