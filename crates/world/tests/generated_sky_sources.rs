use native_world::{
    block_light::LightBounds,
    loaded::{BLOCK_COUNT, BlockState, Chunk, ChunkPos, Section, World},
    sky_light::{UNBOUNDED_SKY_SOURCE, generated_chunk_sky_issue, generated_chunk_sky_sources},
};
use std::{collections::BTreeMap, path::PathBuf, process::Command, sync::Arc};

fn state(name: &str) -> BlockState {
    let (name, properties) = match name {
        "air" => ("air", vec![]),
        "stone" => ("stone", vec![]),
        "glass" => ("glass", vec![]),
        "water" => ("water", vec![("level", "0")]),
        "leaf_litter" => (
            "leaf_litter",
            vec![("facing", "north"), ("segment_amount", "2")],
        ),
        "slab_bottom" => (
            "stone_slab",
            vec![("type", "bottom"), ("waterlogged", "false")],
        ),
        "slab_top" => (
            "stone_slab",
            vec![("type", "top"), ("waterlogged", "false")],
        ),
        "snow_7" => ("snow", vec![("layers", "7")]),
        _ => panic!("unknown fixture state"),
    };
    BlockState {
        name: format!("minecraft:{name}"),
        properties: properties
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect(),
    }
}
fn fixture() -> (World, ChunkPos) {
    let mut world = World::new("minecraft:overworld");
    let air = world.intern_state(state("air"));
    let pos = ChunkPos { x: -1, z: 2 };
    let mut sections = BTreeMap::new();
    for y in -1..=0 {
        sections.insert(
            y,
            Arc::new(Section {
                blocks: Some(Box::new([air; BLOCK_COUNT])),
                biomes: None,
                block_light: None,
                sky_light: None,
            }),
        );
    }
    world.chunks.insert(
        pos,
        Arc::new(Chunk {
            position: pos,
            status: Some("minecraft:terrain".into()),
            block_light_correct: false,
            sections,
        }),
    );
    (world, pos)
}
fn run(command: &mut Command) {
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn generated_sources_reject_incomplete_data_without_mutation() {
    let (mut world, pos) = fixture();
    let bounds = LightBounds::new(-16, 16).unwrap();
    let old = world.chunks[&pos].clone();
    assert!(
        generated_chunk_sky_sources(&world, bounds, pos)
            .unwrap()
            .iter()
            .all(|&v| v == UNBOUNDED_SKY_SOURCE)
    );
    assert!(Arc::ptr_eq(&old, &world.chunks[&pos]));
    Arc::make_mut(world.chunks.get_mut(&pos).unwrap())
        .sections
        .remove(&-1);
    assert!(generated_chunk_sky_sources(&world, bounds, pos).is_none());
    assert!(generated_chunk_sky_issue(&world, bounds, pos).contains("missing section -1"));
    let (mut world, pos) = fixture();
    let unknown = world.intern_state(BlockState {
        name: "example:unknown".into(),
        properties: Default::default(),
    });
    world.set_block_id(-16, 15, 32, unknown).unwrap();
    assert!(generated_chunk_sky_sources(&world, bounds, pos).is_none());
    let issue = generated_chunk_sky_issue(&world, bounds, pos);
    assert!(issue.contains("example:unknown"), "{issue}");
    assert!(issue.contains("[-16, 15, 32]"), "{issue}");
}

#[test]
fn leaf_litter_transmits_generated_sky_light() {
    let (mut world, pos) = fixture();
    let leaf = world.intern_state(state("leaf_litter"));
    let stone = world.intern_state(state("stone"));
    world.set_block_id(-16, 5, 32, leaf).unwrap();
    world.set_block_id(-16, 4, 32, stone).unwrap();
    let sources =
        generated_chunk_sky_sources(&world, LightBounds::new(-16, 16).unwrap(), pos).unwrap();
    assert_eq!(sources[0], 5);
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn generated_sources_match_java_for_every_column() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
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
                paths.push(p)
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("SkyColumnOracle.java");
    std::fs::write(&source, include_str!("java/SkyColumnOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let (mut world, pos) = fixture();
    let mut entries = Vec::new();
    for z in 0..16 {
        for x in 0..16 {
            let blocks: &[(i32, &str)] = match (x + 2 * z) % 5 {
                0 => &[(7, "water"), (-3, "stone")],
                1 => &[(0, "slab_bottom"), (-1, "slab_top")],
                2 => &[(15, "snow_7"), (-2, "glass")],
                3 => &[(-16, "stone")],
                _ => &[],
            };
            for &(y, name) in blocks {
                entries.push(serde_json::json!({"x":x,"y":y,"z":z,"state":name}));
                let id = world.intern_state(state(name));
                world
                    .set_block_id(pos.x * 16 + x, y, pos.z * 16 + z, id)
                    .unwrap();
            }
        }
    }
    let input = temp.path().join("input.json");
    let output = temp.path().join("output.json");
    std::fs::write(&input, serde_json::to_vec(&entries).unwrap()).unwrap();
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("SkyColumnOracle")
        .arg(&input)
        .arg(&output)
        .arg("initial_all")
        .arg("-16")
        .arg("32"));
    let expected: Vec<i32> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    let actual =
        generated_chunk_sky_sources(&world, LightBounds::new(-16, 16).unwrap(), pos).unwrap();
    assert_eq!(&actual[..], expected.as_slice());
}
