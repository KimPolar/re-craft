#![cfg(windows)]

use super::*;
use flate2::{Compression, write::GzEncoder};
use native_persistence::{
    nbt::{self, NamedTag, Tag, TagType},
    world::{DimensionId, WorldReader},
};
use std::io::Write;

#[test]
fn locked_saved_tickets_drive_live_player_dependencies_and_reject_stale_capture() {
    let parent = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/session-save-test-temp");
    std::fs::create_dir_all(&parent).unwrap();
    let dir = tempfile::Builder::new()
        .prefix("simulation-player-")
        .tempdir_in(parent)
        .unwrap();
    let level = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![(
            "Data".into(),
            Tag::Compound(vec![("DataVersion".into(), Tag::Int(5023))]),
        )]),
    };
    let mut compressed = GzEncoder::new(Vec::new(), Compression::default());
    compressed.write_all(&nbt::write(&level).unwrap()).unwrap();
    std::fs::write(dir.path().join("level.dat"), compressed.finish().unwrap()).unwrap();
    let forced = ChunkPos { x: 12, z: -8 };
    let tickets = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![
            ("DataVersion".into(), Tag::Int(5023)),
            (
                "data".into(),
                Tag::Compound(vec![(
                    "tickets".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![Tag::Compound(vec![
                            ("chunk_pos".into(), Tag::IntArray(vec![forced.x, forced.z])),
                            ("type".into(), Tag::String("minecraft:forced".into())),
                            ("level".into(), Tag::Int(31)),
                            ("ticks_left".into(), Tag::Long(0)),
                        ])],
                    },
                )]),
            ),
        ]),
    };
    let path = dir
        .path()
        .join("dimensions/minecraft/overworld/data/minecraft/chunk_tickets.dat");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, nbt::write(&tickets).unwrap()).unwrap();
    let reader =
        WorldReader::open_for_session(dir.path(), DimensionId::parse("overworld").unwrap())
            .unwrap();
    let guard = reader.session_guard().unwrap();
    let stale = reader.capture_chunk_tickets().unwrap();
    let mut session = super::tests::session();
    session.bind_saved_world_session(guard.clone()).unwrap();
    drop(guard.acquire_write().unwrap());
    assert!(
        session
            .install_player_simulation(&stale, 5, 2, false)
            .is_err()
    );
    assert!(session.simulation_dependencies().is_empty());

    let captured = reader.capture_chunk_tickets().unwrap();
    session
        .install_player_simulation(&captured, 5, 2, false)
        .unwrap();
    let home = session.player_chunk().unwrap();
    let halo = ChunkPos {
        x: home.x + 4,
        z: home.z,
    };
    let farther = ChunkPos {
        x: home.x + 6,
        z: home.z,
    };
    assert!(session.simulation_dependencies().contains(&halo));
    assert!(session.simulation_dependencies().contains(&forced));
    assert!(!session.simulation_dependencies().contains(&farther));
    session.set_simulation_view_distance(4);
    assert!(session.simulation_dependencies().contains(&farther));
    session.set_simulation_view_distance(2);
    assert!(!session.simulation_dependencies().contains(&farther));
    let stable = Arc::clone(session.simulation_dependencies());
    session.motion.position.y = 17.0;
    session.update_player_simulation();
    assert!(Arc::ptr_eq(&stable, session.simulation_dependencies()));
    session.motion.position.y = 1.0;
    session.update(TICK);
    assert!(Arc::ptr_eq(&stable, session.simulation_dependencies()));
    session.set_game_mode(GameMode::Spectator);
    session.update(TICK);
    assert!(!session.simulation_dependencies().contains(&halo));
    assert!(session.simulation_dependencies().contains(&forced));
}

#[test]
fn unadvanced_timed_tickets_block_saving_and_generation_after_a_tick() {
    use crate::simulation_player::PlayerSimulation;
    use crate::simulation_tickets::SimulationTickets;
    use native_persistence::chunk_tickets::TicketKind;

    let mut session = super::tests::session();
    let mut tickets = SimulationTickets::default();
    tickets.add_ticket(ChunkPos { x: 0, z: 0 }, TicketKind::Portal, 31);
    session.player_simulation = Some(PlayerSimulationState {
        player: PlayerSimulation::new(tickets, 12),
        spectators_generate_chunks: true,
        view_distance: 12,
    });
    assert!(session.validate_simulation_ticket_progress().is_ok());
    session.tick = 1;
    assert!(session.validate_simulation_ticket_progress().is_err());
    assert!(session.validate_generation_block_readiness().is_err());
    assert!(
        session
            .draft_block_ticks(session.player_chunk().unwrap())
            .is_err()
    );
}

#[test]
fn player_ticket_changes_are_independent_of_frame_intervals() {
    use crate::simulation_player::PlayerSimulation;
    use crate::simulation_tickets::SimulationTickets;

    fn start() -> LoadedSession {
        let mut session = super::tests::session();
        session.set_game_mode(GameMode::Spectator);
        session.motion.position = DVec3::new(8.5, 2.0, 15.5);
        session.player.position = session.motion.position.as_vec3();
        let mut player = PlayerSimulation::new(SimulationTickets::default(), 2);
        player.observe_player(session.player_section().unwrap(), true, true);
        player.run_updates();
        session.simulation_dependencies = Arc::new(player.full_tick_dependencies(2).unwrap());
        session.player_simulation = Some(PlayerSimulationState {
            player,
            spectators_generate_chunks: true,
            view_distance: 2,
        });
        session.submit_command(PlayerCommand::Move {
            forward: 1.0,
            strafe: 0.0,
            jump: false,
        });
        session
    }

    let mut fixed = start();
    let mut fine = start();
    let initial = fixed.player_chunk().unwrap();
    for _ in 0..40 {
        fixed.update(Duration::from_millis(50));
    }
    for _ in 0..200 {
        fine.update(Duration::from_millis(10));
    }
    assert_ne!(fixed.player_chunk().unwrap(), initial);
    assert_eq!(fixed.player_chunk(), fine.player_chunk());
    assert_eq!(fixed.motion.position, fine.motion.position);
    assert_eq!(
        fixed.simulation_dependencies(),
        fine.simulation_dependencies()
    );
    let order = |session: &LoadedSession| {
        session
            .player_simulation
            .as_ref()
            .unwrap()
            .player
            .tracker()
            .entity_ticking_order()
            .collect::<Vec<_>>()
    };
    assert_eq!(order(&fixed), order(&fine));
}
