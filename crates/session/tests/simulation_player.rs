use native_persistence::chunk_tickets::TicketKind;
use native_session::{
    simulation_player::{PlayerSection, PlayerSimulation},
    simulation_tickets::SimulationTickets,
};
use native_world::loaded::ChunkPos;

#[test]
fn saved_forced_tickets_and_player_registration_coexist() {
    let mut tickets = SimulationTickets::default();
    let forced = ChunkPos { x: 0, z: 0 };
    tickets.add_ticket(forced, TicketKind::Forced, 31);
    let mut player = PlayerSimulation::new(tickets, 12);
    player.observe_player(PlayerSection { x: 0, y: 4, z: 0 }, false, true);
    player.run_updates();
    assert_eq!(player.tracker().level(forced), 19);
    player.observe_player(PlayerSection { x: 1, y: 4, z: 0 }, true, false);
    player.run_updates();
    assert_eq!(player.tracker().level(forced), 31);
    assert_eq!(player.tickets().tickets().count(), 1);
}

#[test]
fn natural_spawn_updates_even_when_a_stronger_forced_ticket_masks_the_player() {
    let mut tickets = SimulationTickets::default();
    tickets.add_ticket(ChunkPos { x: 0, z: 0 }, TicketKind::Forced, 0);
    let mut player = PlayerSimulation::new(tickets, 12);
    player.run_updates();
    player.observe_player(PlayerSection { x: 0, y: 4, z: 0 }, false, true);
    assert!(!player.tracker().has_pending_updates());
    assert!(player.has_pending_updates());
    assert!(player.natural_spawn_candidates().is_none());
    player.run_updates();
    assert_eq!(player.natural_spawn_candidates().unwrap().count(), 289);
    player.observe_player(PlayerSection { x: 1, y: 4, z: 0 }, false, true);
    assert!(!player.tracker().has_pending_updates());
    assert!(player.has_pending_updates());
    player.run_updates();
    let candidates: Vec<_> = player.natural_spawn_candidates().unwrap().collect();
    assert_eq!(candidates.len(), 289);
    assert!(candidates.contains(&ChunkPos { x: 9, z: 0 }));
    assert!(!candidates.contains(&ChunkPos { x: -8, z: 0 }));
}

#[test]
#[ignore = "manual timing of simulation ticket movement"]
fn measure_player_simulation_chunk_movement() {
    use std::time::Instant;

    let mut tickets = SimulationTickets::default();
    tickets.add_ticket(ChunkPos { x: 100, z: 100 }, TicketKind::Forced, 31);
    let mut player = PlayerSimulation::new(tickets, 12);
    player.observe_player(PlayerSection { x: 0, y: 4, z: 0 }, false, true);
    player.run_updates();
    let mut times = Vec::new();
    for x in 1..=75 {
        let started = Instant::now();
        player.observe_player(PlayerSection { x, y: 4, z: 0 }, false, true);
        player.run_updates();
        let dependencies = player.full_tick_dependencies(12).unwrap();
        assert!(dependencies.contains(&ChunkPos { x, z: 0 }));
        if x > 15 {
            times.push(started.elapsed());
        }
    }
    times.sort_unstable();
    eprintln!(
        "60 chunk moves with simulation/render distance 12: median {:.3} ms, p95 {:.3} ms",
        times[times.len() / 2].as_secs_f64() * 1_000.0,
        times[times.len() * 95 / 100].as_secs_f64() * 1_000.0,
    );
}

#[cfg(windows)]
fn run_java_oracle(class: &str, java_source: &str) -> String {
    use std::{path::PathBuf, process::Command};

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
    let source = temp.path().join(format!("{class}.java"));
    std::fs::write(&source, java_source).unwrap();
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
    let result = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(cp)
        .arg(class)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}

#[cfg(windows)]
#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn ticking_chunk_activation_runs_postprocessing_before_starting_ticks() {
    let stdout = run_java_oracle(
        "SavedChunkActivationOracle",
        include_str!("java/SavedChunkActivationOracle.java"),
    );
    let lines: Vec<_> = stdout
        .lines()
        .filter(|line| line.starts_with("ORDER ") || line.starts_with("FAILED "))
        .collect();
    assert_eq!(
        lines,
        [
            "ORDER postprocess start-ticking send-future",
            "FAILED postprocess"
        ]
    );
}

#[cfg(windows)]
#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn player_simulation_lifecycle_matches_java_26_3() {
    let stdout = run_java_oracle(
        "PlayerSimulationOracle",
        include_str!("java/PlayerSimulationOracle.java"),
    );
    let mut store = SimulationTickets::default();
    store.add_ticket(ChunkPos { x: 0, z: 0 }, TicketKind::Forced, 31);
    store.add_ticket(ChunkPos { x: 8, z: -2 }, TicketKind::Portal, 30);
    let mut player = PlayerSimulation::new(store, 12);
    let mut expected = Vec::new();
    append_snapshot(&mut player, -1, &mut expected);
    for round in 0..20 {
        let section = |x, y, z| PlayerSection { x, y, z };
        match round {
            0 | 1 => player.observe_player(section(0, 4, 0), false, true),
            2 => player.observe_player(section(0, 5, 0), false, true),
            3 => player.observe_player(section(1, 5, 0), false, true),
            4 => player.observe_player(section(1, 5, 0), true, true),
            5 => player.observe_player(section(1, 5, 0), true, false),
            6 => player.observe_player(section(-3, -5, 7), true, false),
            7 => player.observe_player(section(-3, -5, 7), true, true),
            8 => player.set_distance(5),
            9 => player.observe_player(section(-9, 3, -6), false, true),
            10 => {
                player.add_ticket(ChunkPos { x: -9, z: -6 }, TicketKind::Forced, 31);
            }
            11 => player.set_distance(16),
            12 => player.observe_player(section(-9, 3, -6), true, true),
            13 => player.remove_player(),
            14 => player.observe_player(section(0, 4, 0), false, true),
            15 => {
                player.observe_player(section(3, 4, 2), false, true);
                player.observe_player(section(4, 4, 2), false, true);
                player.observe_player(section(-4, 4, -2), false, true);
            }
            16 => {
                player.observe_player(section(-4, 6, -2), false, true);
                player.observe_player(section(-4, -2, -2), false, true);
            }
            17 => {
                player.observe_player(section(-4, -2, -2), true, false);
                player.observe_player(section(-4, -2, -2), false, true);
            }
            18 => player.observe_player(section(1875000, 0, -1875000), false, true),
            19 => player.observe_player(section(0, 4, 0), false, true),
            _ => unreachable!(),
        }
        append_snapshot(&mut player, round, &mut expected);
    }
    let actual: Vec<_> = stdout
        .lines()
        .map(|line| line.split_once("[STDOUT]: ").map_or(line, |(_, line)| line))
        .filter(|line| {
            line.starts_with("ACTIVE ")
                || line.starts_with("TICK ")
                || line.starts_with("BLOCK ")
                || line.starts_with("SPAWN ")
        })
        .collect();
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(&expected) {
        if *actual != expected {
            let actual: Vec<_> = actual.split_whitespace().collect();
            let expected: Vec<_> = expected.split_whitespace().collect();
            let index = actual
                .iter()
                .zip(&expected)
                .position(|(a, b)| a != b)
                .unwrap_or(actual.len().min(expected.len()));
            panic!(
                "{} {} differs at {}: Java {:?}, Rust {:?}; lengths {} and {}",
                actual.first().unwrap_or(&""),
                actual.get(1).unwrap_or(&""),
                index,
                actual.get(index),
                expected.get(index),
                actual.len(),
                expected.len()
            );
        }
    }
}

#[cfg(windows)]
#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn spawning_collection_and_shuffle_match_java_26_3() {
    let stdout = run_java_oracle(
        "SpawnChunkOracle",
        include_str!("java/SpawnChunkOracle.java"),
    );
    let cases = [
        (0, 0, 8.0, 8.0, false, true, 0),
        (-1, -1, -0.5, -15.5, false, true, 123456789),
        (3, -4, 63.999, -63.999, false, true, -1),
        (0, 0, 8.0, 8.0, true, true, 42),
        (0, 0, 8.0, 8.0, true, false, 42),
        (1875000, -1875000, 30000008.0, -29999992.0, false, true, 1),
    ];
    let mut expected = Vec::new();
    for (index, (x, z, px, pz, spectator, spectator_loads, seed)) in cases.into_iter().enumerate() {
        let mut player = PlayerSimulation::new(SimulationTickets::default(), 12);
        player.observe_player(PlayerSection { x, y: 4, z }, spectator, spectator_loads);
        player.run_updates();
        let mut chunks = player
            .collect_spawning_chunks(px, pz, spectator, |pos| {
                pos.x
                    .wrapping_mul(31)
                    .wrapping_add(pos.z.wrapping_mul(17))
                    .wrapping_add(index as i32)
                    .rem_euclid(7)
                    >= 3
            })
            .unwrap();
        let mut collected = format!("COLLECT {index}");
        for pos in &chunks {
            collected.push_str(&format!(" {},{}", pos.x, pos.z));
        }
        expected.push(collected);
        let mut random = native_core::random::LegacyRandom::new(seed);
        PlayerSimulation::shuffle_spawning_chunks(&mut chunks, &mut random);
        let mut shuffled = format!("SHUFFLE {index}");
        for pos in &chunks {
            shuffled.push_str(&format!(" {},{}", pos.x, pos.z));
        }
        shuffled.push_str(&format!(" next={}", random.next_int()));
        expected.push(shuffled);
    }
    let actual: Vec<_> = stdout
        .lines()
        .map(|line| line.split_once("[STDOUT]: ").map_or(line, |(_, line)| line))
        .filter(|line| line.starts_with("COLLECT ") || line.starts_with("SHUFFLE "))
        .collect();
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(&expected) {
        if *actual != expected {
            let actual: Vec<_> = actual.split_whitespace().collect();
            let expected: Vec<_> = expected.split_whitespace().collect();
            let index = actual
                .iter()
                .zip(&expected)
                .position(|(a, b)| a != b)
                .unwrap_or(actual.len().min(expected.len()));
            panic!(
                "{} {} differs at {}: Java {:?}, Rust {:?}; lengths {} and {}",
                actual.first().unwrap_or(&""),
                actual.get(1).unwrap_or(&""),
                index,
                actual.get(index),
                expected.get(index),
                actual.len(),
                expected.len()
            );
        }
    }
}

#[cfg(windows)]
#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn scheduled_tick_simulation_filter_matches_java_26_3() {
    use native_session::worldgen::chunk_ticks::{ChunkTicks, IndexedRegionQueue, SavedTick};

    fn prepare(kind: &str) -> IndexedRegionQueue {
        let mut queue = IndexedRegionQueue::default();
        for x in -2..=2 {
            let pos = ChunkPos { x, z: 0 };
            let ticks = ChunkTicks::from_saved(
                pos,
                vec![SavedTick {
                    kind: kind.into(),
                    position: [x * 16 + 4, 2, 4],
                    delay: 0,
                    priority: 0,
                }],
            )
            .unwrap();
            queue.add_container(pos, ticks).unwrap();
            queue.unpack_container(pos, 42).unwrap();
        }
        queue
    }

    fn emit(
        output: &mut Vec<String>,
        name: &str,
        step: i32,
        time: i64,
        queue: &mut IndexedRegionQueue,
        player: &PlayerSimulation,
    ) {
        let mut line = format!("{name} {step}");
        for tick in queue.collect_due(time, 65_536, |pos| {
            player.tracker().in_block_ticking_range(pos)
        }) {
            line.push_str(&format!(" {}", tick.position[0].div_euclid(16)));
        }
        output.push(line);
    }

    let stdout = run_java_oracle(
        "SimulationScheduledTicksOracle",
        include_str!("java/SimulationScheduledTicksOracle.java"),
    );
    let mut player = PlayerSimulation::new(SimulationTickets::default(), 0);
    player.observe_player(PlayerSection { x: 0, y: 4, z: 0 }, false, true);
    player.run_updates();
    let mut blocks = prepare("minecraft:sand");
    let mut fluids = prepare("minecraft:water");
    let mut expected = Vec::new();
    emit(&mut expected, "BLOCK", 0, 42, &mut blocks, &player);
    emit(&mut expected, "FLUID", 0, 42, &mut fluids, &player);
    player.observe_player(PlayerSection { x: 1, y: 4, z: 0 }, false, true);
    player.run_updates();
    emit(&mut expected, "BLOCK", 1, 43, &mut blocks, &player);
    emit(&mut expected, "FLUID", 1, 43, &mut fluids, &player);
    player.remove_player();
    player.add_ticket(ChunkPos { x: -2, z: 0 }, TicketKind::Forced, 31);
    player.run_updates();
    emit(&mut expected, "BLOCK", 2, 44, &mut blocks, &player);
    emit(&mut expected, "FLUID", 2, 44, &mut fluids, &player);
    let actual: Vec<_> = stdout
        .lines()
        .map(|line| line.split_once("[STDOUT]: ").map_or(line, |(_, line)| line))
        .filter(|line| line.starts_with("BLOCK ") || line.starts_with("FLUID "))
        .collect();
    assert_eq!(actual, expected);
}

#[cfg(windows)]
#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn loading_and_simulation_dependencies_match_java_26_3() {
    let stdout = run_java_oracle(
        "LoadingTickDependenciesOracle",
        include_str!("java/LoadingTickDependenciesOracle.java"),
    );
    let cases = [
        (12, 2, 0, 0, true, false, false),
        (5, 12, -4, 3, true, true, false),
        (12, 2, 0, 0, false, true, false),
        (12, 12, 0, 0, true, true, true),
        (12, 2, 0, 0, false, false, true),
        (12, 2, 0, 0, false, true, false),
    ];
    let mut expected = Vec::new();
    for (index, (distance, view, x, z, active, forced, saved_simulation)) in
        cases.into_iter().enumerate()
    {
        let mut player = PlayerSimulation::new(SimulationTickets::default(), distance);
        if active {
            player.observe_player(PlayerSection { x, y: 0, z }, false, true);
        }
        if forced {
            player.add_ticket(
                ChunkPos { x: 40, z: -9 },
                TicketKind::Forced,
                if index == 5 { 29 } else { 31 },
            );
        }
        if saved_simulation {
            player.add_ticket(ChunkPos { x: -40, z: 9 }, TicketKind::PlayerSimulation, 31);
        }
        if index == 4 {
            player.add_ticket(ChunkPos { x: -40, z: 9 }, TicketKind::Unknown, 31);
        }
        player.run_updates();
        let mut line = format!("DEPENDENCIES {index}");
        for position in player.full_tick_dependencies(view).unwrap() {
            line.push_str(&format!(" {},{}", position.x, position.z));
        }
        expected.push(line);
    }
    let actual: Vec<_> = stdout
        .lines()
        .map(|line| line.split_once("[STDOUT]: ").map_or(line, |(_, line)| line))
        .filter(|line| line.starts_with("DEPENDENCIES "))
        .collect();
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(&expected) {
        if *actual != expected {
            let actual: Vec<_> = actual.split_whitespace().collect();
            let expected: Vec<_> = expected.split_whitespace().collect();
            let index = actual
                .iter()
                .zip(&expected)
                .position(|(a, b)| a != b)
                .unwrap_or(actual.len().min(expected.len()));
            panic!(
                "loading case {} differs at {}: Java {:?}, Rust {:?}; lengths {} and {}",
                actual.get(1).unwrap_or(&""),
                index,
                actual.get(index),
                expected.get(index),
                actual.len(),
                expected.len()
            );
        }
    }
}

#[cfg(windows)]
fn append_snapshot(player: &mut PlayerSimulation, round: i32, output: &mut Vec<String>) {
    player.run_updates();
    let mut active = format!("ACTIVE {round}");
    for ticket in player.tickets().tickets() {
        active.push_str(&format!(
            " {},{},{},{},{}",
            ticket.position.x,
            ticket.position.z,
            ticket.kind.identifier(),
            ticket.level,
            ticket.ticks_left
        ));
    }
    output.push(active);
    let mut order = format!("TICK {round}");
    for pos in player.tracker().entity_ticking_order() {
        order.push_str(&format!(
            " {},{},{}",
            pos.x,
            pos.z,
            player.tracker().level(pos)
        ));
    }
    output.push(order);
    let mut block_order = format!("BLOCK {round}");
    for pos in player.tracker().block_ticking_order() {
        block_order.push_str(&format!(
            " {},{},{}",
            pos.x,
            pos.z,
            player.tracker().level(pos)
        ));
    }
    output.push(block_order);
    let mut spawn = format!("SPAWN {round}");
    for pos in player.natural_spawn_candidates().unwrap() {
        spawn.push_str(&format!(" {},{}", pos.x, pos.z));
    }
    output.push(spawn);
}
