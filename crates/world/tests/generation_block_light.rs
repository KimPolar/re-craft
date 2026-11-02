use native_world::{
    block_light::LightBounds,
    generation_block_light::initialize_generated_block_light,
    generation_light::VisibleLayers,
    loaded::{BLOCK_COUNT, BlockState, Chunk, ChunkPos, Section, World},
};
use std::{collections::BTreeMap, path::PathBuf, process::Command, sync::Arc};
fn state(name: &str) -> BlockState {
    let (block, properties) = match name {
        "air" => ("air", vec![]),
        "stone" => ("stone", vec![]),
        "torch" => ("torch", vec![]),
        "firefly_bush" => ("firefly_bush", vec![]),
        "water" => ("water", vec![("level", "0")]),
        "slab_bottom" => (
            "stone_slab",
            vec![("type", "bottom"), ("waterlogged", "false")],
        ),
        "slab_top" => (
            "stone_slab",
            vec![("type", "top"), ("waterlogged", "false")],
        ),
        "lichen_north" => (
            "glow_lichen",
            vec![
                ("down", "false"),
                ("up", "false"),
                ("north", "true"),
                ("south", "false"),
                ("west", "false"),
                ("east", "false"),
                ("waterlogged", "false"),
            ],
        ),
        "lichen_wet" => (
            "glow_lichen",
            vec![
                ("down", "false"),
                ("up", "false"),
                ("north", "true"),
                ("south", "false"),
                ("west", "false"),
                ("east", "false"),
                ("waterlogged", "true"),
            ],
        ),
        "small_bud" => (
            "small_amethyst_bud",
            vec![("facing", "up"), ("waterlogged", "false")],
        ),
        "cluster_wet" => (
            "amethyst_cluster",
            vec![("facing", "up"), ("waterlogged", "true")],
        ),
        _ => panic!("unknown fixture block"),
    };
    BlockState {
        name: format!("minecraft:{block}"),
        properties: properties
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect(),
    }
}
fn fixture() -> World {
    let mut world = World::new("minecraft:overworld");
    let air = world.intern_state(state("air"));
    for z in -1..=1 {
        for x in -1..=1 {
            let pos = ChunkPos { x, z };
            let mut sections = BTreeMap::new();
            for sy in 0..2 {
                sections.insert(
                    sy,
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
                    status: Some("minecraft:features".into()),
                    block_light_correct: false,
                    sections,
                }),
            );
        }
    }
    world
}
fn value(layers: &VisibleLayers, x: usize, y: i32, z: usize) -> u8 {
    let l = &layers[&y.div_euclid(16)];
    let i = x + z * 16 + y.rem_euclid(16) as usize * 256;
    let b = l[i / 2];
    if i % 2 == 0 { b & 15 } else { b >> 4 }
}
fn run(c: &mut Command) {
    let o = c.output().unwrap();
    assert!(
        o.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn generated_block_light_matches_java_from_complete_halo() {
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
    let source = temp.path().join("BlockLightEngineOracle.java");
    std::fs::write(&source, include_str!("java/BlockLightEngineOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let initial = [
        (-1, 8, 8, "torch"),
        (31, 8, 15, "torch"),
        (8, 30, 8, "torch"),
        (15, 8, 15, "lichen_north"),
        (0, 0, 0, "lichen_wet"),
        (16, 16, 8, "cluster_wet"),
        (8, 16, 8, "firefly_bush"),
        (9, 16, 8, "small_bud"),
        (1, 8, 8, "stone"),
        (0, 8, 7, "water"),
        (7, 30, 8, "slab_bottom"),
        (8, 29, 8, "slab_top"),
    ];
    let entries = initial
        .iter()
        .map(|(x, y, z, s)| serde_json::json!({"x":x,"y":y,"z":z,"state":s}))
        .collect::<Vec<_>>();
    let input = temp.path().join("input.json");
    let output = temp.path().join("output.json");
    std::fs::write(&input,serde_json::to_vec(&serde_json::json!({"min_y":0,"height":32,"sample_min_y":-16,"sample_height":64,"initial":entries,"actions":[{"x":30,"y":0,"z":30,"state":"air"}]})).unwrap()).unwrap();
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("BlockLightEngineOracle")
        .arg(&input)
        .arg(&output));
    let expected: Vec<Vec<u8>> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(expected.len(), 1);
    let mut world = fixture();
    for (x, y, z, name) in initial {
        let id = world.intern_state(state(name));
        world.set_block_id(x, y, z, id).unwrap();
    }
    let layers = initialize_generated_block_light(
        &world,
        LightBounds::new(0, 32).unwrap(),
        ChunkPos { x: 0, z: 0 },
    )
    .unwrap();
    for y in -16..48 {
        for z in 0..16 {
            for x in 0..16 {
                let index = ((y + 16) * 31 * 31 + z * 31 + x) as usize;
                assert_eq!(
                    value(&layers, x as usize, y, z as usize),
                    expected[0][index],
                    "{x},{y},{z}"
                );
            }
        }
    }
}
