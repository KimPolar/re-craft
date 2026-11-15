use super::{
    CarvedTerrainWorld, MaterializedTerrain, QueuedGenerationLightEngine, SavedLightLoadOrder,
};
use native_world::{
    generation_light::{LightChannel, QueuedSectionLightTracker},
    loaded::{BLOCK_COUNT, BlockState, Chunk, ChunkPos, LIGHT_BYTES, Section, World},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
    path::PathBuf,
    process::Command,
    sync::Arc,
};

fn fixture(has_sky: bool) -> CarvedTerrainWorld {
    let position = ChunkPos { x: 0, z: 0 };
    let mut world = World::new("minecraft:overworld");
    let state = |name: &str| BlockState {
        name: format!("minecraft:{name}"),
        properties: BTreeMap::new(),
    };
    let air = world.intern_state(state("air"));
    let torch = world.intern_state(state("torch"));
    let mut sections = BTreeMap::new();
    for y in 0..5 {
        let mut blocks = Box::new([air; BLOCK_COUNT]);
        if y == 2 {
            blocks[0] = torch;
        }
        let block_light = match y {
            2 => Some(Box::new(std::array::from_fn(|i| {
                (i as u8).wrapping_mul(37).wrapping_add(11)
            }))),
            4 => Some(Box::new(std::array::from_fn(|i| {
                (i as u8).wrapping_mul(19).wrapping_add(7)
            }))),
            _ => None,
        };
        sections.insert(
            y,
            Arc::new(Section {
                blocks: Some(blocks),
                biomes: None,
                block_light,
                sky_light: (has_sky && y == 2).then(|| Box::new([0xff; LIGHT_BYTES])),
            }),
        );
    }
    world.chunks.insert(
        position,
        Arc::new(Chunk {
            position,
            status: Some("minecraft:full".into()),
            block_light_correct: true,
            sections,
        }),
    );
    CarvedTerrainWorld::new(MaterializedTerrain {
        world,
        position,
        seed: 42,
        min_y: 0,
        height: 80,
        postprocessing: Arc::from([]),
        noise_postprocessing: None,
        surface_postprocessing: None,
        carver_postprocessing: None,
    })
}

fn is_air(state: &BlockState) -> Option<bool> {
    match state.name.as_str() {
        "minecraft:air" => Some(true),
        "minecraft:torch" | "minecraft:stone" => Some(false),
        _ => None,
    }
}

fn compare_capture<'a>(
    lines: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>,
    name: &str,
    tracker: &QueuedSectionLightTracker,
) {
    assert_eq!(lines.next(), Some(format!("H {name}").as_str()));
    let mut seen = BTreeSet::new();
    while lines.peek().is_some_and(|line| line.starts_with("L ")) {
        let parts: Vec<_> = lines.next().unwrap().split(' ').collect();
        let channel = match parts[2] {
            "B" => LightChannel::Block,
            "S" => LightChannel::Sky,
            _ => panic!("unknown channel"),
        };
        let position = ChunkPos {
            x: parts[3].parse().unwrap(),
            z: parts[5].parse().unwrap(),
        };
        let y = parts[4].parse().unwrap();
        assert!(seen.insert((parts[1], channel, position, y)));
        let layer = match parts[1] {
            "V" => tracker.visible_layer(channel, position, y),
            "D" => tracker.data_layer(channel, position, y),
            _ => panic!("unknown layer source"),
        }
        .unwrap()
        .unwrap_or_else(|| panic!("missing Java layer: {name} {} {position:?} {y}", parts[1]));
        let form = layer
            .homogeneous_value()
            .map_or_else(|| "M".to_owned(), |value| format!("H{value}"));
        assert_eq!(
            form, parts[6],
            "{name} {} {channel:?} {position:?} {y}",
            parts[1]
        );
        assert_eq!(parts[7].len(), LIGHT_BYTES * 2);
        let expected: Vec<_> = parts[7]
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect();
        assert_eq!(
            layer.bytes().as_slice(),
            expected,
            "{name} {} {channel:?} {position:?} {y}",
            parts[1]
        );
    }
    let mut expected_count = 0;
    for channel in [LightChannel::Block, LightChannel::Sky] {
        for x in -2..=2 {
            for z in -2..=2 {
                for y in -1..=5 {
                    let position = ChunkPos { x, z };
                    expected_count += usize::from(
                        tracker
                            .visible_layer(channel, position, y)
                            .unwrap()
                            .is_some(),
                    );
                    expected_count +=
                        usize::from(tracker.data_layer(channel, position, y).unwrap().is_some());
                }
            }
        }
    }
    assert_eq!(seen.len(), expected_count, "{name}: extra Rust layer");
}

fn run_java_oracle(main: &str, sources: &[(&str, &str)], input: Option<&str>) -> String {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let java = std::env::var_os("MINECRAFT_JAVA_HOME")
        .or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let version = game.join("versions/26.3");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for library in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let classpath = std::env::join_paths(paths).unwrap();
    let mut compiler = Command::new(java.join("javac.exe"));
    compiler.arg("-cp").arg(&classpath);
    for (name, source) in sources {
        let path = temp.path().join(name);
        std::fs::write(&path, source).unwrap();
        compiler.arg(path);
    }
    let compiled = compiler.output().unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let mut command = Command::new(java.join("java.exe"));
    command.arg("-cp").arg(&classpath).arg(main);
    if let Some(input) = input {
        let path = temp.path().join("events.txt");
        std::fs::write(&path, input).unwrap();
        command.arg(path);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25"]
fn saved_light_streaming_matches_java_for_observed_drains() {
    let cases = [
        (true, SavedLightLoadOrder::SeparateDrains),
        (true, SavedLightLoadOrder::SameDrain),
        (false, SavedLightLoadOrder::SeparateDrains),
        (false, SavedLightLoadOrder::SameDrain),
    ];
    let mut script = String::new();
    for (index, &(has_sky, order)) in cases.iter().enumerate() {
        writeln!(script, "N 1 {}", u8::from(has_sky)).unwrap();
        for phase in ["loaded", "reloaded"] {
            if phase == "reloaded" {
                writeln!(script, "U 0 0\nD\nC {index}_unloaded").unwrap();
            }
            script.push_str("R 0 0 1\nQ B 0 2 0 1\n");
            if has_sky {
                script.push_str("Q S 0 2 0 2\n");
            }
            script.push_str("Q B 0 4 0 3\n");
            if order == SavedLightLoadOrder::SeparateDrains {
                script.push_str("D\n");
            }
            writeln!(
                script,
                "T 0 2 0 1\nD\nE 0 0 1\nR 0 0 0\nD\nC {index}_{phase}"
            )
            .unwrap();
        }
    }
    let output = run_java_oracle(
        "QueuedSectionLightOracle",
        &[(
            "QueuedSectionLightOracle.java",
            include_str!("../../../world/tests/java/QueuedSectionLightOracle.java"),
        )],
        Some(&script),
    );
    let mut lines = output
        .lines()
        .filter(|line| line.starts_with("H ") || line.starts_with("L "))
        .peekable();
    let position = ChunkPos { x: 0, z: 0 };
    for (index, (has_sky, order)) in cases.into_iter().enumerate() {
        let terrain = fixture(has_sky);
        let covered = (-2..=2).flat_map(|x| (-2..=2).map(move |z| ChunkPos { x, z }));
        let tracker = QueuedSectionLightTracker::fresh(true, has_sky, covered).unwrap();
        let mut engine = QueuedGenerationLightEngine::new(
            &terrain,
            tracker,
            [],
            BTreeMap::new(),
            has_sky,
            is_air,
        )
        .unwrap();
        engine
            .load_saved_chunk(&terrain, position, order, is_air)
            .unwrap();
        compare_capture(&mut lines, &format!("{index}_loaded"), engine.tracker());
        engine.unload_chunk(&terrain, position).unwrap();
        compare_capture(&mut lines, &format!("{index}_unloaded"), engine.tracker());
        engine
            .load_saved_chunk(&terrain, position, order, is_air)
            .unwrap();
        compare_capture(&mut lines, &format!("{index}_reloaded"), engine.tracker());
    }
    assert!(lines.next().is_none());
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25"]
fn saved_neighbor_arrival_matches_java_with_active_sky_sources() {
    let output = run_java_oracle(
        "StreamedSavedLightOracle",
        &[
            (
                "QueuedSectionLightOracle.java",
                include_str!("../../../world/tests/java/QueuedSectionLightOracle.java"),
            ),
            (
                "SavedLiveLightOracle.java",
                include_str!("../../../world/tests/java/SavedLiveLightOracle.java"),
            ),
            (
                "StreamedSavedLightOracle.java",
                include_str!("../../tests/java/StreamedSavedLightOracle.java"),
            ),
        ],
        None,
    );
    let mut lines = output
        .lines()
        .filter(|line| line.starts_with("H ") || line.starts_with("L "))
        .peekable();
    let cases = [
        (true, SavedLightLoadOrder::SeparateDrains),
        (true, SavedLightLoadOrder::SameDrain),
        (false, SavedLightLoadOrder::SeparateDrains),
        (false, SavedLightLoadOrder::SameDrain),
    ];
    let center = ChunkPos { x: 0, z: 0 };
    let east = ChunkPos { x: 1, z: 0 };
    for (index, (has_sky, order)) in cases.into_iter().enumerate() {
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let stone = world.intern_state(BlockState {
            name: "minecraft:stone".into(),
            properties: BTreeMap::new(),
        });
        for position in [center, east] {
            let mut sections = BTreeMap::new();
            for y in 0..2 {
                let mut blocks = Box::new([air; BLOCK_COUNT]);
                if y == 0 {
                    blocks[256] = stone;
                }
                sections.insert(
                    y,
                    Arc::new(Section {
                        blocks: Some(blocks),
                        biomes: None,
                        block_light: (y == 0).then(|| {
                            Box::new(std::array::from_fn(|i| {
                                if position == center {
                                    (i as u8).wrapping_mul(37).wrapping_add(11)
                                } else {
                                    (i as u8).wrapping_mul(19).wrapping_add(7)
                                }
                            }))
                        }),
                        sky_light: (has_sky && y == 0).then(|| Box::new([0xff; LIGHT_BYTES])),
                    }),
                );
            }
            sections.insert(
                2,
                Arc::new(Section {
                    blocks: None,
                    biomes: None,
                    block_light: Some(Box::new(std::array::from_fn(|i| {
                        (i as u8).wrapping_mul(19).wrapping_add(7)
                    }))),
                    sky_light: None,
                }),
            );
            world.chunks.insert(
                position,
                Arc::new(Chunk {
                    position,
                    status: Some("minecraft:full".into()),
                    block_light_correct: true,
                    sections,
                }),
            );
        }
        let full = CarvedTerrainWorld::new(MaterializedTerrain {
            world,
            position: center,
            seed: 42,
            min_y: 0,
            height: 32,
            postprocessing: Arc::from([]),
            noise_postprocessing: None,
            surface_postprocessing: None,
            carver_postprocessing: None,
        });
        let mut before = full.clone();
        Arc::make_mut(&mut before.world).chunks.remove(&east);
        let covered = (-2..=2).flat_map(|x| (-2..=2).map(move |z| ChunkPos { x, z }));
        let tracker = QueuedSectionLightTracker::fresh(true, has_sky, covered).unwrap();
        let mut engine = QueuedGenerationLightEngine::new(
            &before,
            tracker,
            [],
            BTreeMap::new(),
            has_sky,
            is_air,
        )
        .unwrap();
        engine
            .load_saved_chunk(&before, center, order, is_air)
            .unwrap();
        compare_capture(&mut lines, &format!("{index}_before"), engine.tracker());
        engine.load_saved_chunk(&full, east, order, is_air).unwrap();
        compare_capture(&mut lines, &format!("{index}_after"), engine.tracker());
    }
    assert!(lines.next().is_none());
}
