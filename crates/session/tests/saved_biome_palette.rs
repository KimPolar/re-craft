use native_persistence::{
    nbt::{self, NamedTag, Tag, TagType},
    palette,
    world::{ChunkPos, LoadedChunk, TARGET_DATA_VERSION},
};
use native_session::worldgen::QuartBiomeChunk;
use std::{path::PathBuf, process::Command, sync::Arc};

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

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn saved_biome_palettes_match_java_26_3() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: serde_json::Value =
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
    let source = temp.path().join("SavedBiomePaletteOracle.java");
    std::fs::write(&source, include_str!("java/SavedBiomePaletteOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));

    let biome_names = [
        "minecraft:plains",
        "minecraft:forest",
        "minecraft:desert",
        "minecraft:swamp",
        "minecraft:ocean",
    ];
    let mut sections = Vec::new();
    let mut files = Vec::new();
    for (i, len) in [1, 3, 5].into_iter().enumerate() {
        let indices: Vec<u32> = (0..64).map(|n| ((n * 7 + i * 3) % len) as u32).collect();
        let mut fields = vec![(
            "palette".into(),
            Tag::List {
                element_type: TagType::String,
                values: biome_names[..len]
                    .iter()
                    .map(|name| Tag::String((*name).into()))
                    .collect(),
            },
        )];
        if let Some(data) = palette::encode_indices(len, &indices, 1).unwrap() {
            fields.push(("data".into(), Tag::LongArray(data)));
        }
        let biome = Tag::Compound(fields);
        let path = temp.path().join(format!("section-{i}.nbt"));
        std::fs::write(
            &path,
            nbt::write(&NamedTag {
                name: "".into(),
                value: biome.clone(),
            })
            .unwrap(),
        )
        .unwrap();
        files.push(path);
        sections.push(Tag::Compound(vec![
            ("Y".into(), Tag::Byte(i as i8 - 1)),
            ("biomes".into(), biome),
        ]));
    }
    let document = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![(
            "sections".into(),
            Tag::List {
                element_type: TagType::Compound,
                values: sections,
            },
        )]),
    };
    let saved = LoadedChunk {
        position: ChunkPos { x: -2, z: 3 },
        data_version: TARGET_DATA_VERSION,
        status: Some("minecraft:full".into()),
        sections: palette::decode_sections(&document.value).unwrap(),
        document: Arc::new(document),
    };
    let restored = QuartBiomeChunk::from_saved_chunk(&saved, 42, -16, 48, |_| Ok(())).unwrap();
    let mut command = Command::new(java.join("java.exe"));
    command
        .arg("-classpath")
        .arg(&cp)
        .arg("SavedBiomePaletteOracle")
        .args(files);
    let java = run(&mut command);
    let mut expected = Vec::new();
    for section in 0..3 {
        for y in 0..4 {
            for z in 0..4 {
                for x in 0..4 {
                    let id = &restored.values()[(z * 4 + x) * 12 + section * 4 + y];
                    expected.push(format!("{}:{}", id.namespace, id.path));
                }
            }
        }
    }
    assert_eq!(java.lines().collect::<Vec<_>>(), expected);
}
