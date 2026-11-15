use native_persistence::chunk_tickets::TicketKind;
use native_session::simulation_tickets::{ChunkSaveReadiness, SimulationTickets};
use native_world::loaded::ChunkPos;

#[test]
fn timed_tickets_pause_refresh_and_expire_at_the_java_boundary() {
    let pos = ChunkPos { x: 0, z: 0 };
    let forced = ChunkPos { x: 100, z: 100 };
    let mut store = SimulationTickets::default();
    assert!(store.add_ticket(pos, TicketKind::Portal, 31));
    assert!(store.add_ticket(pos, TicketKind::Unknown, 31));
    assert!(store.add_ticket(forced, TicketKind::Forced, 31));
    store.run_updates();
    assert!(store.tracker().in_entity_ticking_range(pos));

    assert_eq!(store.purge_stale(|_| ChunkSaveReadiness::Pending), 0);
    assert_eq!(store.purge_stale(|_| ChunkSaveReadiness::Pending), 1);
    let portal = store
        .tickets()
        .find(|t| t.kind == TicketKind::Portal)
        .unwrap();
    assert_eq!(portal.ticks_left, 300);

    assert_eq!(store.purge_stale(|_| ChunkSaveReadiness::Absent), 0);
    assert_eq!(
        store
            .tickets()
            .find(|t| t.kind == TicketKind::Portal)
            .unwrap()
            .ticks_left,
        299
    );
    assert!(!store.add_ticket(pos, TicketKind::Portal, 31));
    assert_eq!(
        store
            .tickets()
            .find(|t| t.kind == TicketKind::Portal)
            .unwrap()
            .ticks_left,
        300
    );
    for _ in 0..300 {
        assert_eq!(store.purge_stale(|_| ChunkSaveReadiness::Ready), 0);
    }
    assert_eq!(
        store
            .tickets()
            .find(|t| t.kind == TicketKind::Portal)
            .unwrap()
            .ticks_left,
        0
    );
    assert_eq!(store.purge_stale(|_| ChunkSaveReadiness::Ready), 1);
    store.run_updates();
    assert!(!store.tracker().in_entity_ticking_range(pos));
    assert!(store.tracker().in_entity_ticking_range(forced));
    assert_eq!(store.tickets().count(), 1);
}

#[test]
fn replacing_levels_coalesces_duplicate_tickets() {
    let pos = ChunkPos { x: 3, z: -8 };
    let mut store = SimulationTickets::default();
    assert!(store.add_ticket(pos, TicketKind::PlayerSimulation, 24));
    assert!(store.add_ticket(pos, TicketKind::PlayerSimulation, 25));
    store.run_updates();
    assert_eq!(store.tracker().level(pos), 24);
    store.replace_level(TicketKind::PlayerSimulation, 27);
    store.run_updates();
    assert_eq!(store.tickets().count(), 1);
    assert_eq!(store.tracker().level(pos), 27);
    assert!(store.remove_ticket(pos, TicketKind::PlayerSimulation, 27));
    assert!(!store.remove_ticket(pos, TicketKind::PlayerSimulation, 27));
    store.run_updates();
    assert!(!store.tracker().in_entity_ticking_range(pos));
}

#[cfg(windows)]
#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn ticket_lifecycle_matches_java_26_3() {
    use std::{path::PathBuf, process::Command};

    use native_persistence::{chunk_tickets::SavedChunkTickets, nbt};

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
    let source = temp.path().join("SimulationTicketLifecycleOracle.java");
    std::fs::write(
        &source,
        include_str!("java/SimulationTicketLifecycleOracle.java"),
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
        .arg("SimulationTicketLifecycleOracle")
        .arg(&file)
        .output()
        .unwrap();
    assert!(
        actual.status.success(),
        "{}",
        String::from_utf8_lossy(&actual.stderr)
    );
    let document = nbt::read(&std::fs::read(file).unwrap()).unwrap();
    let saved = SavedChunkTickets::read(Some(document)).unwrap();
    assert!(saved.entries().len() > 70);
    let mut store = SimulationTickets::from_saved(&saved);
    let mut expected = Vec::new();
    store.run_updates();
    append_snapshot(&store, -1, &mut expected);
    let portal_pos = ChunkPos { x: 3, z: -8 };
    let zero = ChunkPos { x: 0, z: 0 };
    for round in 0_i32..12 {
        match round {
            2 | 4 => {
                store.add_ticket(portal_pos, TicketKind::Portal, 29);
            }
            3 => store.replace_level(TicketKind::PlayerSimulation, 27),
            5 => {
                store.remove_ticket(zero, TicketKind::Forced, 31);
            }
            _ => {}
        }
        store.purge_stale(|pos| {
            match pos
                .x
                .wrapping_mul(17)
                .wrapping_add(pos.z.wrapping_mul(31))
                .wrapping_add(round)
                .rem_euclid(3)
            {
                0 => ChunkSaveReadiness::Pending,
                1 => ChunkSaveReadiness::Ready,
                _ => ChunkSaveReadiness::Absent,
            }
        });
        store.run_updates();
        append_snapshot(&store, round, &mut expected);
    }
    let stdout = String::from_utf8(actual.stdout).unwrap();
    let lines: Vec<_> = stdout
        .lines()
        .map(|line| line.split_once("[STDOUT]: ").map_or(line, |(_, line)| line))
        .filter(|line| line.starts_with("ACTIVE ") || line.starts_with("TICK "))
        .collect();
    assert_eq!(lines.len(), expected.len(), "Java trace count differs");
    for (actual, expected) in lines.iter().zip(&expected) {
        if *actual != expected {
            let actual_parts: Vec<_> = actual.split_whitespace().collect();
            let expected_parts: Vec<_> = expected.split_whitespace().collect();
            let at = actual_parts
                .iter()
                .zip(&expected_parts)
                .position(|(a, b)| a != b)
                .unwrap_or(actual_parts.len().min(expected_parts.len()));
            panic!(
                "{} trace differs at element {}: Java {:?}, Rust {:?}; lengths {} vs {}",
                actual_parts.first().unwrap_or(&""),
                at,
                actual_parts.get(at),
                expected_parts.get(at),
                actual_parts.len(),
                expected_parts.len()
            );
        }
    }
}

#[cfg(windows)]
fn append_snapshot(store: &SimulationTickets, round: i32, result: &mut Vec<String>) {
    let mut active = format!("ACTIVE {round}");
    for ticket in store.tickets() {
        active.push_str(&format!(
            " {},{},{},{},{}",
            ticket.position.x,
            ticket.position.z,
            ticket.kind.identifier(),
            ticket.level,
            ticket.ticks_left
        ));
    }
    result.push(active);
    let mut order = format!("TICK {round}");
    for pos in store.tracker().entity_ticking_order() {
        order.push_str(&format!(
            " {},{},{}",
            pos.x,
            pos.z,
            store.tracker().level(pos)
        ));
    }
    result.push(order);
}
