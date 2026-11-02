use native_world::{
    block_light::LightBounds,
    generation_block_light::{
        generated_block_light_from_active_chunks, initialize_generated_block_light,
    },
    generation_light::{VisibleLayers, VisibleLightSnapshot},
    generation_sky_light::{
        generated_sky_light_from_active_chunks, generated_sky_light_with_initialization_history,
        initialize_generated_sky_light,
    },
    loaded::{BLOCK_COUNT, BlockState, Chunk, ChunkPos, Section, World},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    process::Command,
    sync::Arc,
};
fn state(name: &str) -> BlockState {
    let (block, properties) = match name {
        "air" => ("air", vec![]),
        "stone" => ("stone", vec![]),
        "glass" => ("glass", vec![]),
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

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn generated_sky_light_matches_java_initial_engine() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
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
    let source = temp.path().join("GeneratedSkyLightOracle.java");
    std::fs::write(&source, include_str!("java/GeneratedSkyLightOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let mut entries = Vec::new();
    let mut world = fixture();
    let mut add = |x, y, z, name| {
        entries.push(serde_json::json!({"x":x,"y":y,"z":z,"state":name}));
        let id = world.intern_state(state(name));
        world.set_block_id(x, y, z, id).unwrap();
    };
    for z in -16..32 {
        for x in -16..32 {
            if (x, z) != (5, 5) && (x, z) != (16, 8) {
                add(x, 20, z, "stone");
            }
        }
    }
    for (x, y, z, name) in [
        (5, 19, 5, "water"),
        (5, 18, 5, "water"),
        (8, 19, 8, "slab_bottom"),
        (9, 19, 8, "slab_top"),
        (9, 18, 8, "slab_bottom"),
        (16, 19, 8, "glass"),
        (0, 8, 0, "stone"),
        (15, 2, 15, "stone"),
    ] {
        add(x, y, z, name);
    }
    let input = temp.path().join("input.json");
    let output = temp.path().join("output.json");
    std::fs::write(&input, serde_json::to_vec(&entries).unwrap()).unwrap();
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("GeneratedSkyLightOracle")
        .arg(&input)
        .arg(&output));
    let expected: Vec<u8> = serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
    let actual = initialize_generated_sky_light(
        &world,
        LightBounds::new(0, 32).unwrap(),
        ChunkPos { x: 0, z: 0 },
    )
    .unwrap();
    assert_eq!(expected.len(), 64 * 16 * 16);
    for y in -16..48 {
        for z in 0..16 {
            for x in 0..16 {
                let index = ((y + 16) * 256 + z * 16 + x) as usize;
                assert_eq!(
                    value(&actual, x as usize, y, z as usize),
                    expected[index],
                    "{x},{y},{z}"
                );
            }
        }
    }
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("GeneratedSkyLightOracle")
        .arg(&input)
        .arg(&output)
        .arg("staged")
        .arg("1,0,-1"));
    let expected: Vec<Vec<u8>> = serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
    let mut statuses = Vec::new();
    let mut covered = Vec::new();
    for z in -1..=1 {
        for x in -1..=1 {
            let p = ChunkPos { x, z };
            covered.push(p);
            for y in 0..2 {
                statuses.push((p, y));
            }
        }
    }
    let storage =
        VisibleLightSnapshot::fresh_after_section_updates(true, true, statuses, covered).unwrap();
    let center = ChunkPos { x: 0, z: 0 };
    let bounds = LightBounds::new(0, 32).unwrap();
    let mut active = BTreeSet::new();
    assert_eq!(expected.len(), 4);
    for stage in 0..4 {
        if stage > 0 {
            active.insert(ChunkPos {
                x: [1, 0, -1][stage - 1],
                z: 0,
            });
        }
        let actual =
            generated_sky_light_from_active_chunks(&world, bounds, center, &active, &storage)
                .unwrap();
        let mut differences = Vec::new();
        for y in -16..48 {
            for z in 0..16 {
                for x in 0..16 {
                    let i = ((y + 16) * 256 + z * 16 + x) as usize;
                    let found = value(&actual, x as usize, y, z as usize);
                    if found != expected[stage][i] && differences.len() < 30 {
                        differences.push((x, y, z, found, expected[stage][i]));
                    }
                }
            }
        }
        assert!(
            differences.is_empty(),
            "stage {stage} differences {differences:?}"
        );
    }
    let mut terrain = fixture();
    let mut varied = Vec::new();
    for z in -16i32..32 {
        for x in -16i32..32 {
            let y = 8 + (x * 3 + z * 5).rem_euclid(9);
            let kind = (x * 11 - z * 7).rem_euclid(7);
            let blocks: &[(i32, &str)] = match kind {
                0 => &[],
                1 => &[(0, "water")],
                2 => &[(0, "slab_bottom")],
                3 => &[(0, "slab_top")],
                4 => &[(0, "stone"), (1, "water")],
                _ => &[(0, "stone")],
            };
            for &(offset, name) in blocks {
                varied.push(serde_json::json!({"x":x,"y":y+offset,"z":z,"state":name}));
                let id = terrain.intern_state(state(name));
                terrain.set_block_id(x, y + offset, z, id).unwrap();
            }
        }
    }
    std::fs::write(&input, serde_json::to_vec(&varied).unwrap()).unwrap();
    for (order, java_order) in [([0, 1, -1], "0,1,-1"), ([1, 0, -1], "1,0,-1")] {
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("GeneratedSkyLightOracle")
            .arg(&input)
            .arg(&output)
            .arg("staged")
            .arg(java_order));
        let expected: Vec<Vec<u8>> =
            serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
        let mut active = BTreeSet::new();
        for stage in 0..4 {
            if stage > 0 {
                active.insert(ChunkPos {
                    x: order[stage - 1],
                    z: 0,
                });
            }
            let actual =
                generated_sky_light_from_active_chunks(&terrain, bounds, center, &active, &storage)
                    .unwrap();
            let mut differences = Vec::new();
            for y in -16..48 {
                for z in 0..16 {
                    for x in 0..16 {
                        let index = ((y + 16) * 256 + z * 16 + x) as usize;
                        let found = value(&actual, x as usize, y, z as usize);
                        if found != expected[stage][index] && differences.len() < 30 {
                            differences.push((x, y, z, found, expected[stage][index]));
                        }
                    }
                }
            }
            assert!(
                differences.is_empty(),
                "varied order {java_order} stage {stage} differences {differences:?}"
            );
        }
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
fn staged_block_sources_match_java_engine() {
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
    let source = temp.path().join("InitialBlockLightStagesOracle.java");
    std::fs::write(
        &source,
        include_str!("java/InitialBlockLightStagesOracle.java"),
    )
    .unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = temp.path().join("output.json");
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("InitialBlockLightStagesOracle")
        .arg(&output));
    let expected: Vec<Vec<u8>> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(expected.len(), 4);
    let mut world = fixture();
    for (x, y, z, name) in [
        (3, 8, 8, "torch"),
        (16, 8, 8, "torch"),
        (-1, 8, 5, "torch"),
        (14, 30, 14, "torch"),
        (4, 8, 8, "stone"),
        (0, 8, 6, "stone"),
        (15, 8, 7, "stone"),
    ] {
        let id = world.intern_state(state(name));
        world.set_block_id(x, y, z, id).unwrap();
    }
    let mut statuses = Vec::new();
    let mut covered = Vec::new();
    for z in -1..=1 {
        for x in -1..=1 {
            let pos = ChunkPos { x, z };
            covered.push(pos);
            for sy in 0..2 {
                statuses.push((pos, sy));
            }
        }
    }
    let storage =
        VisibleLightSnapshot::fresh_after_section_updates(true, false, statuses, covered).unwrap();
    let bounds = LightBounds::new(0, 32).unwrap();
    let center = ChunkPos { x: 0, z: 0 };
    let mut active = BTreeSet::new();
    for stage in 0..4 {
        if stage > 0 {
            active.insert(ChunkPos {
                x: [0, 1, -1][stage - 1],
                z: 0,
            });
        }
        let actual =
            generated_block_light_from_active_chunks(&world, bounds, center, &active, &storage)
                .unwrap();
        assert_eq!(expected[stage].len(), 64 * 256);
        for y in -16..48 {
            for z in 0..16 {
                for x in 0..16 {
                    let i = ((y + 16) * 256 + z * 16 + x) as usize;
                    assert_eq!(
                        value(&actual, x as usize, y, z as usize),
                        expected[stage][i],
                        "stage {stage} at {x},{y},{z}"
                    );
                }
            }
        }
        if stage == 1 {
            let full = initialize_generated_block_light(&world, bounds, center).unwrap();
            assert_ne!(
                value(&actual, 15, 8, 8),
                value(&full, 15, 8, 8),
                "unactivated neighbor must not emit yet"
            );
        }
    }
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn partially_initialized_sky_matches_java_engine() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
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
    let source = temp.path().join("SparseSkyLightStagesOracle.java");
    std::fs::write(
        &source,
        include_str!("java/SparseSkyLightStagesOracle.java"),
    )
    .unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(source));
    let mut world = World::new("minecraft:overworld");
    let air = world.intern_state(state("air"));
    let stone = world.intern_state(state("stone"));
    let mut covered = BTreeSet::new();
    for z in -1..=1 {
        for x in -1..=1 {
            let pos = ChunkPos { x, z };
            covered.insert(pos);
            let sections = (0..6)
                .map(|y| {
                    (
                        y,
                        Arc::new(Section {
                            blocks: Some(Box::new([air; BLOCK_COUNT])),
                            biomes: None,
                            block_light: None,
                            sky_light: None,
                        }),
                    )
                })
                .collect();
            world.chunks.insert(
                pos,
                Arc::new(Chunk {
                    position: pos,
                    status: Some("minecraft:terrain".into()),
                    block_light_correct: false,
                    sections,
                }),
            );
        }
    }
    let mut entries = Vec::new();
    for z in -16..32 {
        for x in -16..32 {
            for y in [4, 80] {
                if y == 80 && ((x == 5 && z == 5) || (x == 16 && z == 8)) {
                    continue;
                }
                world.set_block_id(x, y, z, stone).unwrap();
                entries.push(serde_json::json!({"x":x,"y":y,"z":z,"state":"stone"}));
            }
        }
    }
    let input = temp.path().join("input.json");
    let output = temp.path().join("output.json");
    std::fs::write(&input, serde_json::to_vec(&entries).unwrap()).unwrap();
    let center = ChunkPos { x: 0, z: 0 };
    let east = ChunkPos { x: 1, z: 0 };
    for (names, registered_names, order, initialized, registered) in [
        (
            "0:0",
            "0:0",
            "0",
            BTreeSet::from([center]),
            BTreeSet::from([center]),
        ),
        (
            "1:0",
            "1:0",
            "1",
            BTreeSet::from([east]),
            BTreeSet::from([east]),
        ),
        (
            "0:0,1:0",
            "0:0,1:0",
            "1,0",
            BTreeSet::from([center, east]),
            BTreeSet::from([center, east]),
        ),
        (
            "0:0",
            "0:0,1:0",
            "0",
            BTreeSet::from([center]),
            BTreeSet::from([center, east]),
        ),
    ] {
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("SparseSkyLightStagesOracle")
            .arg(&input)
            .arg(&output)
            .arg(order)
            .arg(names)
            .arg(registered_names));
        let expected: Vec<Vec<u8>> =
            serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
        let storage = VisibleLightSnapshot::fresh_after_chunk_initialization(
            &world,
            0..6,
            registered.iter().copied(),
            covered.iter().copied(),
            true,
            true,
            |s| Some(s.name == "minecraft:air"),
        )
        .unwrap();
        let mut active = BTreeSet::new();
        for (stage, wanted) in expected.iter().enumerate() {
            if stage > 0 {
                active.insert(ChunkPos {
                    x: order.split(',').nth(stage - 1).unwrap().parse().unwrap(),
                    z: 0,
                });
            }
            let actual = generated_sky_light_with_initialization_history(
                &world,
                LightBounds::new(0, 96).unwrap(),
                center,
                &active,
                &storage,
                &registered,
                &initialized,
            )
            .unwrap();
            for y in -16..112 {
                for z in 0..16 {
                    for x in 0..16 {
                        let i = ((y + 16) * 256 + z * 16 + x) as usize;
                        assert_eq!(
                            value(&actual, x as usize, y, z as usize),
                            wanted[i],
                            "{names} stage {stage} at {x},{y},{z}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn sparse_staged_sky_matches_java_engine() {
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
    let source = temp.path().join("SparseSkyLightStagesOracle.java");
    std::fs::write(
        &source,
        include_str!("java/SparseSkyLightStagesOracle.java"),
    )
    .unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let input = temp.path().join("input.json");
    let output = temp.path().join("output.json");
    for mode in 0..3 {
        let roof = mode != 0;
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(state("air"));
        let mut covered = Vec::new();
        for z in -1..=1 {
            for x in -1..=1 {
                let pos = ChunkPos { x, z };
                covered.push(pos);
                let mut sections = BTreeMap::new();
                for y in 0..6 {
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
                        status: Some("minecraft:features".into()),
                        block_light_correct: false,
                        sections,
                    }),
                );
            }
        }
        let mut entries = Vec::new();
        for z in -16..32 {
            for x in -16..32 {
                let mut add = |y, name| {
                    entries.push(serde_json::json!({"x":x,"y":y,"z":z,"state":name}));
                    let id = world.intern_state(state(name));
                    world.set_block_id(x, y, z, id).unwrap();
                };
                add(4, "stone");
                if roof && (x, z) != (5, 5) && (x, z) != (16, 8) {
                    add(80, "stone");
                }
                if roof && (x, z) == (5, 5) {
                    add(79, "water");
                }
                if mode == 2 && (x, z) == (16, 8) {
                    add(40, "stone");
                }
            }
        }
        std::fs::write(&input, serde_json::to_vec(&entries).unwrap()).unwrap();
        let storage = VisibleLightSnapshot::fresh_after_chunk_initialization(
            &world,
            0..6,
            covered.iter().copied(),
            covered.iter().copied(),
            true,
            true,
            |s| Some(s.name == "minecraft:air"),
        )
        .unwrap();
        let center = ChunkPos { x: 0, z: 0 };
        let bounds = LightBounds::new(0, 96).unwrap();
        let orders: &[(&str, [i32; 3])] = if mode == 2 {
            &[
                ("0,1,-1", [0, 1, -1]),
                ("1,0,-1", [1, 0, -1]),
                ("-1,1,0", [-1, 1, 0]),
            ]
        } else {
            &[("0,1,-1", [0, 1, -1])]
        };
        for &(order_name, order) in orders {
            run(Command::new(java.join("java.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg("SparseSkyLightStagesOracle")
                .arg(&input)
                .arg(&output)
                .arg(order_name));
            let expected: Vec<Vec<u8>> =
                serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
            let mut active = BTreeSet::new();
            for stage in 0..4 {
                if stage > 0 {
                    active.insert(ChunkPos {
                        x: order[stage - 1],
                        z: 0,
                    });
                }
                let actual = generated_sky_light_from_active_chunks(
                    &world, bounds, center, &active, &storage,
                )
                .expect("sparse sections should be supported");
                let mut differences = Vec::new();
                for y in -16..112 {
                    for z in 0..16 {
                        for x in 0..16 {
                            let i = ((y + 16) * 256 + z * 16 + x) as usize;
                            let found = value(&actual, x as usize, y, z as usize);
                            if found != expected[stage][i] && differences.len() < 30 {
                                differences.push((x, y, z, found, expected[stage][i]));
                            }
                        }
                    }
                }
                assert!(
                    differences.is_empty(),
                    "mode {mode} order {order_name} stage {stage}: {differences:?}"
                );
            }
        }
    }
}

#[test]
fn generated_sky_handles_open_columns_and_incomplete_inputs() {
    let mut world = fixture();
    let center = ChunkPos { x: 0, z: 0 };
    let bounds = LightBounds::new(0, 32).unwrap();
    let covered = (-1..=1).flat_map(|z| (-1..=1).map(move |x| ChunkPos { x, z }));
    let sparse =
        VisibleLightSnapshot::fresh_after_section_updates(true, true, [], covered).unwrap();
    let empty =
        generated_sky_light_from_active_chunks(&world, bounds, center, &BTreeSet::new(), &sparse)
            .unwrap();
    assert!(empty.values().all(|layer| layer.iter().all(|&b| b == 0xff)));
    let unknown = VisibleLightSnapshot::new(true, true);
    assert!(
        generated_sky_light_from_active_chunks(&world, bounds, center, &BTreeSet::new(), &unknown)
            .is_none()
    );
    let sky = initialize_generated_sky_light(&world, bounds, center).unwrap();
    assert!(sky.values().all(|layer| layer.iter().all(|&b| b == 0xff)));
    let stone = world.intern_state(state("stone"));
    world.set_block_id(8, 15, 8, stone).unwrap();
    assert!(
        generated_sky_light_from_active_chunks(&world, bounds, center, &BTreeSet::new(), &sparse)
            .is_none()
    );
    let sky = initialize_generated_sky_light(&world, bounds, center).unwrap();
    assert_eq!(value(&sky, 8, 16, 8), 15);
    assert_eq!(value(&sky, 8, 15, 8), 0);
    assert_eq!(value(&sky, 8, 14, 8), 14);
    let unknown = world.intern_state(BlockState {
        name: "example:unknown".into(),
        properties: Default::default(),
    });
    world.set_block_id(1, 10, 1, unknown).unwrap();
    assert!(initialize_generated_sky_light(&world, bounds, center).is_none());
    let air = world.intern_state(state("air"));
    world.set_block_id(1, 10, 1, air).unwrap();
    world.chunks.remove(&ChunkPos { x: -1, z: 0 });
    assert!(initialize_generated_sky_light(&world, bounds, center).is_none());
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
