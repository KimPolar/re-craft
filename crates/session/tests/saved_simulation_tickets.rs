#[cfg(windows)]
#[test]
fn captured_ticket_activation_checks_world_dimension_and_epoch() {
    use std::io::Write;

    use flate2::{Compression, write::GzEncoder};
    use native_persistence::{
        nbt::{self, NamedTag, Tag, TagType},
        world::{DimensionId, TARGET_DATA_VERSION, WorldReader},
    };
    use native_session::simulation_chunks::SimulationChunks;
    use native_session::simulation_tickets::SimulationTickets;
    use native_world::loaded::ChunkPos;

    let world = tempfile::tempdir().unwrap();
    let level = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![(
            "Data".into(),
            Tag::Compound(vec![("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION))]),
        )]),
    };
    let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
    gzip.write_all(&nbt::write(&level).unwrap()).unwrap();
    std::fs::write(world.path().join("level.dat"), gzip.finish().unwrap()).unwrap();
    let path = world
        .path()
        .join("dimensions/minecraft/overworld/data/minecraft/chunk_tickets.dat");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let saved = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![
            ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
            (
                "data".into(),
                Tag::Compound(vec![(
                    "tickets".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![Tag::Compound(vec![
                            ("chunk_pos".into(), Tag::IntArray(vec![-3, 4])),
                            ("type".into(), Tag::String("minecraft:forced".into())),
                            ("level".into(), Tag::Int(31)),
                        ])],
                    },
                )]),
            ),
        ]),
    };
    std::fs::write(&path, nbt::write(&saved).unwrap()).unwrap();
    let dimension = DimensionId::parse("overworld").unwrap();
    let reader = WorldReader::open_for_session(world.path(), dimension.clone()).unwrap();
    let guard = reader.session_guard().unwrap();
    let capture = reader.capture_chunk_tickets().unwrap();
    let mut live_tickets = SimulationTickets::from_capture(&capture, &guard, &dimension).unwrap();
    let mut loading =
        SimulationTickets::from_capture_with_loading(&capture, &guard, &dimension).unwrap();
    assert!(loading.loading_tracker().unwrap().has_pending_updates());
    loading.run_loading_updates(usize::MAX, |_| {}).unwrap();
    assert_eq!(
        loading
            .loading_tracker()
            .unwrap()
            .level(ChunkPos { x: -3, z: 4 }),
        31
    );
    assert_eq!(
        loading
            .loading_tracker()
            .unwrap()
            .level(ChunkPos { x: -2, z: 4 }),
        32
    );
    assert!(
        SimulationTickets::from_capture_with_loading(
            &capture,
            &guard,
            &DimensionId::parse("the_nether").unwrap()
        )
        .is_err()
    );
    live_tickets.run_updates();
    assert!(
        live_tickets
            .tracker()
            .in_entity_ticking_range(ChunkPos { x: -3, z: 4 })
    );
    assert!(
        SimulationTickets::from_capture(
            &capture,
            &guard,
            &DimensionId::parse("the_nether").unwrap()
        )
        .is_err()
    );
    let mut tracker = SimulationChunks::default();
    tracker
        .activate_captured_tickets(&capture, &guard, &dimension)
        .unwrap();
    tracker.run_updates();
    assert!(tracker.in_entity_ticking_range(ChunkPos { x: -3, z: 4 }));
    let before = tracker.entity_ticking_order().collect::<Vec<_>>();
    assert!(
        tracker
            .activate_captured_tickets(&capture, &guard, &DimensionId::parse("the_nether").unwrap())
            .is_err()
    );
    assert_eq!(tracker.entity_ticking_order().collect::<Vec<_>>(), before);
    drop(guard.acquire_write().unwrap());
    assert!(SimulationTickets::from_capture(&capture, &guard, &dimension).is_err());
    assert!(SimulationTickets::from_capture_with_loading(&capture, &guard, &dimension).is_err());
    assert!(
        tracker
            .activate_captured_tickets(&capture, &guard, &dimension)
            .is_err()
    );
    assert_eq!(tracker.entity_ticking_order().collect::<Vec<_>>(), before);
}

#[cfg(windows)]
#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn saved_ticket_activation_matches_java_26_3() {
    use std::{path::PathBuf, process::Command};

    use native_persistence::{chunk_tickets::SavedChunkTickets, nbt};
    use native_session::simulation_chunks::SimulationChunks;
    use native_world::loaded::ChunkPos;

    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME")
        .or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join(
                "AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon",
            )
        })
        .join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for library in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("SavedSimulationTicketsOracle.java");
    std::fs::write(
        &source,
        include_str!("java/SavedSimulationTicketsOracle.java"),
    )
    .unwrap();
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(source)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let file = temp.path().join("chunk_tickets.dat");
    let actual = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(cp)
        .arg("SavedSimulationTicketsOracle")
        .arg(&file)
        .output()
        .unwrap();
    assert!(
        actual.status.success(),
        "{}",
        String::from_utf8_lossy(&actual.stderr)
    );
    let document = nbt::read(&std::fs::read(file).unwrap()).unwrap();
    let saved = SavedChunkTickets::read(Some(document.clone())).unwrap();
    assert_eq!(saved.source(), Some(&document));
    assert!(saved.entries().len() > 90);
    let mut activation = String::from("ACT");
    for ticket in saved.activation_order() {
        activation.push_str(&format!(
            " {},{},{},{},{}",
            ticket.position.x,
            ticket.position.z,
            ticket.kind.identifier(),
            ticket.level,
            ticket.ticks_left
        ));
    }
    let mut tracker = SimulationChunks::default();
    tracker.activate_saved_tickets(&saved);
    tracker.add_ticket(
        ChunkPos { x: 2, z: -5 },
        native_persistence::chunk_tickets::TicketKind::PlayerSimulation.simulation_key(),
        24,
    );
    tracker.run_updates();
    let mut order = String::from("TICK");
    for pos in tracker.entity_ticking_order() {
        order.push_str(&format!(" {},{},{}", pos.x, pos.z, tracker.level(pos)));
    }
    let stdout = String::from_utf8(actual.stdout).unwrap();
    let lines: Vec<_> = stdout
        .lines()
        .map(|line| line.split_once("[STDOUT]: ").map_or(line, |(_, line)| line))
        .filter(|line| line.starts_with("ACT") || line.starts_with("TICK"))
        .collect();
    assert_eq!(lines, [activation.as_str(), order.as_str()]);
}
