use native_persistence::nbt::{self, Tag, TagType};
use native_resources::packs::ResourceId;
use native_session::worldgen::saved_structure_starts::{
    RestoredStructurePiece, RestoredStructureStart, restore_saved_structure_starts,
};
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
        let ["T", _, _, _, filename] = fields.as_slice() else {
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
        count += 1;
    }
    assert_eq!(count, 32);
}
