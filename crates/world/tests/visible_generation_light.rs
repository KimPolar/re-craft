use native_world::{
    generation_light::{VisibleLayers, VisibleLightColumn, VisibleLightSnapshot},
    loaded::{BLOCK_COUNT, BlockState, Chunk, ChunkPos, LIGHT_BYTES, Section, World},
};
use std::{collections::BTreeMap, path::PathBuf, process::Command, sync::Arc};
#[derive(Default)]
struct Column {
    block: VisibleLayers,
    sky: VisibleLayers,
}
fn run(c: &mut Command) -> String {
    let o = c.output().unwrap();
    assert!(
        o.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&o.stderr),
        String::from_utf8_lossy(&o.stdout)
    );
    String::from_utf8(o.stdout).unwrap()
}
fn initialized_chunk_fixture() -> World {
    let mut world = World::new("minecraft:overworld");
    let make = |name: &str| BlockState {
        name: format!("minecraft:{name}"),
        properties: Default::default(),
    };
    let air = world.intern_state(make("air"));
    for x in [-1, 0] {
        let position = ChunkPos { x, z: 0 };
        let mut sections = BTreeMap::new();
        for sy in -1..=2 {
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
            position,
            Arc::new(Chunk {
                position,
                status: Some("minecraft:features".into()),
                block_light_correct: false,
                sections,
            }),
        );
    }
    for (x, y, z, name) in [
        (0, -16, 0, "cave_air"),
        (0, 0, 0, "void_air"),
        (0, 16, 0, "stone"),
        (0, 32, 0, "water"),
        (-16, -16, 0, "torch"),
    ] {
        let id = world.intern_state(make(name));
        world.set_block_id(x, y, z, id).unwrap();
    }
    world
}

#[test]
fn chunk_capture_rejects_missing_and_unknown_section_data() {
    let center = ChunkPos { x: 0, z: 0 };
    let capture = |world: &World| {
        VisibleLightSnapshot::fresh_after_chunk_initialization(
            world,
            -1..3,
            [center],
            [center],
            true,
            true,
            |state| match state.name.as_str() {
                "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" => Some(true),
                "minecraft:stone" | "minecraft:water" => Some(false),
                _ => None,
            },
        )
    };
    let mut world = initialized_chunk_fixture();
    let light = capture(&world).unwrap();
    assert_eq!(light.sky_light([0, 48, 0]), Some(0));
    assert_eq!(light.sky_light([0, 64, 0]), Some(15));
    Arc::make_mut(world.chunks.get_mut(&center).unwrap())
        .sections
        .remove(&2);
    assert!(capture(&world).is_none());
    let mut world = initialized_chunk_fixture();
    let unknown = world.intern_state(BlockState {
        name: "example:unknown".into(),
        properties: Default::default(),
    });
    world.set_block_id(0, -16, 0, unknown).unwrap();
    assert!(capture(&world).is_none());
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn visible_generation_light_matches_java_engine_storage() {
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
    let source = temp.path().join("VisibleGenerationLightOracle.java");
    std::fs::write(
        &source,
        include_str!("java/VisibleGenerationLightOracle.java"),
    )
    .unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let actual = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("VisibleGenerationLightOracle"));
    let mut columns: BTreeMap<ChunkPos, Column> = BTreeMap::new();
    let mut snapshot = None;
    let mut fresh = None;
    let mut channels = (false, false);
    let mut stage = 0;
    let mut cases = 0;
    let mut nonzero_blocks = false;
    let mut nontrivial_sky = false;
    for line in actual.lines() {
        let a = line.split_whitespace().collect::<Vec<_>>();
        match a.first().copied() {
            Some("H") => {
                stage += 1;
                let mode: u8 = a[1].parse().unwrap();
                channels = (a[2].parse().unwrap(), a[3].parse().unwrap());
                snapshot = None;
                columns.clear();
                for x in -1..=1 {
                    for z in -1..=1 {
                        columns.insert(ChunkPos { x, z }, Column::default());
                    }
                }
                let mut statuses = Vec::new();
                if mode > 0 {
                    statuses.push((ChunkPos { x: 0, z: 0 }, 0));
                }
                if mode == 4 {
                    statuses.extend([(ChunkPos { x: -1, z: 1 }, -1), (ChunkPos { x: 1, z: 0 }, 1)]);
                }
                if mode == 5 {
                    statuses.extend([(ChunkPos { x: 0, z: 0 }, 2), (ChunkPos { x: 1, z: -1 }, -2)]);
                }
                fresh = if matches!(mode, 0 | 1 | 4 | 5) {
                    Some(
                        VisibleLightSnapshot::fresh_after_section_updates(
                            channels.0,
                            channels.1,
                            statuses,
                            columns.keys().copied(),
                        )
                        .unwrap(),
                    )
                } else if mode == 6 {
                    Some(
                        VisibleLightSnapshot::fresh_after_chunk_initialization(
                            &initialized_chunk_fixture(),
                            -1..3,
                            [ChunkPos { x: 0, z: 0 }, ChunkPos { x: -1, z: 0 }],
                            columns.keys().copied(),
                            channels.0,
                            channels.1,
                            |s| {
                                Some(matches!(
                                    s.name.as_str(),
                                    "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                                ))
                            },
                        )
                        .unwrap(),
                    )
                } else {
                    None
                };
            }
            Some("L") => {
                let pos = ChunkPos {
                    x: a[2].parse().unwrap(),
                    z: a[3].parse().unwrap(),
                };
                let sy = a[4].parse().unwrap();
                let hex = a[5].as_bytes();
                assert_eq!(hex.len(), LIGHT_BYTES * 2);
                let mut data = [0u8; LIGHT_BYTES];
                for (i, pair) in hex.chunks_exact(2).enumerate() {
                    let s = std::str::from_utf8(pair).unwrap();
                    data[i] = u8::from_str_radix(s, 16).unwrap();
                }
                let column = columns.get_mut(&pos).unwrap();
                if a[1] == "B" {
                    column.block.insert(sy, Arc::new(data));
                } else {
                    column.sky.insert(sy, Arc::new(data));
                }
            }
            Some("Q") => {
                let light = snapshot.get_or_insert_with(|| {
                    let mut light = VisibleLightSnapshot::new(channels.0, channels.1);
                    for (pos, column) in &columns {
                        light.insert_complete_column(
                            *pos,
                            VisibleLightColumn::new(column.block.clone(), column.sky.clone()),
                        );
                    }
                    light
                });
                let p = [
                    a[1].parse().unwrap(),
                    a[2].parse().unwrap(),
                    a[3].parse().unwrap(),
                ];
                let b = a[4].parse::<u8>().unwrap();
                let s = a[5].parse::<u8>().unwrap();
                assert_eq!(light.block_light(p), Some(b), "stage {stage} {p:?}");
                assert_eq!(light.sky_light(p), Some(s), "stage {stage} {p:?}");
                if let Some(fresh) = &fresh {
                    assert_eq!(fresh.block_light(p), Some(b), "fresh stage {stage} {p:?}");
                    assert_eq!(fresh.sky_light(p), Some(s), "fresh stage {stage} {p:?}");
                }
                assert_eq!(
                    light.raw_brightness(p, 0),
                    Some(a[6].parse().unwrap()),
                    "stage {stage} {p:?}"
                );
                assert_eq!(
                    light.raw_brightness(p, 5),
                    Some(a[7].parse().unwrap()),
                    "stage {stage} {p:?}"
                );
                nonzero_blocks |= b > 0;
                nontrivial_sky |= s > 0 && s < 15;
                cases += 1;
            }
            _ => {}
        }
    }
    assert_eq!(stage, 28);
    assert_eq!(cases, 28 * 7 * 4 * 10);
    assert!(nonzero_blocks && nontrivial_sky);
}
