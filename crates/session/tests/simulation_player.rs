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
    for round in 0..15 {
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
            _ => unreachable!(),
        }
        append_snapshot(&mut player, round, &mut expected);
    }
    let actual: Vec<_> = stdout
        .lines()
        .map(|line| line.split_once("[STDOUT]: ").map_or(line, |(_, line)| line))
        .filter(|line| {
            line.starts_with("ACTIVE ") || line.starts_with("TICK ") || line.starts_with("BLOCK ")
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
}
