use native_persistence::nbt::{self, NamedTag, Tag, TagType};
use native_resources::packs::ResourceId;
use native_session::worldgen::structure_references::restore_saved_structure_references;
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
