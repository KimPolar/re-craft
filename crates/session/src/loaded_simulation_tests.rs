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

#[test]
fn live_spawn_selection_uses_absolute_position_and_tracks_masked_player_moves() {
    use crate::simulation_player::PlayerSimulation;
    use crate::simulation_tickets::SimulationTickets;
    use native_persistence::chunk_tickets::TicketKind;

    let mut session = super::tests::session();
    session.origin = IVec3::new(160, 0, -160);
    session.motion.position = DVec3::new(8.0, 2.0, 8.0);
    session.set_game_mode(GameMode::Survival);
    let home = session.player_chunk().unwrap();
    let near = ChunkPos {
        x: home.x + 1,
        z: home.z,
    };
    let boundary = ChunkPos {
        x: home.x + 8,
        z: home.z,
    };
    let ready = BTreeSet::from([home, near, boundary]);
    assert!(session.collect_spawning_chunks(&ready).is_none());
    let mut tickets = SimulationTickets::default();
    tickets.add_ticket(home, TicketKind::Forced, 0);
    let mut player = PlayerSimulation::new(tickets, 12);
    player.observe_player(session.player_section().unwrap(), false, true);
    player.run_updates();
    session.player_simulation = Some(PlayerSimulationState {
        player,
        spectators_generate_chunks: true,
        view_distance: 12,
    });
    assert_eq!(
        session
            .collect_spawning_chunks(&ready)
            .unwrap()
            .into_iter()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([home, near])
    );
    session.set_game_mode(GameMode::Spectator);
    assert!(session.collect_spawning_chunks(&ready).unwrap().is_empty());
    session.set_game_mode(GameMode::Survival);
    session.motion.position.x += 16.0;
    session.update_player_simulation();
    let state = session.player_simulation.as_ref().unwrap();
    assert!(!state.player.has_pending_updates());
    let candidates: BTreeSet<_> = state.player.natural_spawn_candidates().unwrap().collect();
    assert!(candidates.contains(&ChunkPos {
        x: home.x + 9,
        z: home.z
    }));
    assert!(!candidates.contains(&ChunkPos {
        x: home.x - 8,
        z: home.z
    }));
}

#[test]
fn scheduled_blocks_and_fluids_wait_for_simulation_tickets() {
    use crate::simulation_player::PlayerSimulation;
    use crate::simulation_tickets::SimulationTickets;

    for indexed in [false, true] {
        for direction in [-1, 1] {
            for force in [false, true] {
                let template = super::tests::session();
                let home = template.player_chunk().unwrap();
                let ring = ChunkPos {
                    x: home.x + direction,
                    z: home.z,
                };
                let far = ChunkPos {
                    x: home.x + direction * 2,
                    z: home.z,
                };
                let mut world = template.world.as_ref().clone();
                for pos in [ring, far] {
                    let mut chunk = world.chunks[&home].as_ref().clone();
                    chunk.position = pos;
                    world.chunks.insert(pos, Arc::new(chunk));
                }
                let sand = world.intern_state(state("minecraft:sand", &[]));
                for pos in [home, ring, far] {
                    let position = [pos.x * 16 + 3, 2, pos.z * 16 + 4];
                    world
                        .set_block_id(position[0], position[1], position[2], sand)
                        .unwrap();
                    world.saved_block_ticks.insert(
                        pos,
                        vec![SavedBlockTick {
                            block: "minecraft:sand".into(),
                            position,
                            delay: 0,
                            priority: 0,
                        }],
                    );
                    world.saved_fluid_ticks.insert(
                        pos,
                        vec![SavedFluidTick {
                            fluid: "minecraft:water".into(),
                            position: [pos.x * 16 + 4, 2, pos.z * 16 + 4],
                            delay: 0,
                            priority: 0,
                        }],
                    );
                }
                let registration: Vec<_> = world.chunks.keys().copied().collect();
                let mut session =
                    LoadedSession::new(world, template.origin, template.bounds, template.player)
                        .unwrap();
                session.set_falling_fire_tag(Arc::new(BTreeSet::new()));
                session.set_initial_game_time(42).unwrap();
                if indexed {
                    assert_eq!(
                        session.install_saved_tick_orders(&registration).unwrap(),
                        (true, true)
                    );
                }
                let mut player = PlayerSimulation::new(SimulationTickets::default(), 0);
                player.observe_player(session.player_section().unwrap(), false, true);
                player.run_updates();
                session.player_simulation = Some(PlayerSimulationState {
                    player,
                    spectators_generate_chunks: true,
                    view_distance: 2,
                });
                assert!(session.simulation_allows_block_ticks(home));
                assert!(session.simulation_allows_block_ticks(ring));
                assert!(!session.simulation_allows_block_ticks(far));

                let collected: BTreeSet<_> = session
                    .collect_due_fluid_ticks()
                    .into_iter()
                    .map(|(_, tick)| ChunkPos {
                        x: tick.key.x.div_euclid(16),
                        z: tick.key.z.div_euclid(16),
                    })
                    .collect();
                assert_eq!(collected, BTreeSet::from([home, ring]));
                assert_eq!(session.fluid_ticks.len(), 1);
                session.advance_block_ticks();
                assert_eq!(session.falling_sand.len(), 2);
                assert_eq!(session.sand_ticks.len(), 1);
                let far_absolute = IVec3::new(far.x * 16 + 3, 2, far.z * 16 + 4);
                assert_eq!(
                    session
                        .world
                        .block(far_absolute.x, far_absolute.y, far_absolute.z)
                        .unwrap()
                        .name,
                    "minecraft:sand"
                );
                session.schedule_sand(far_absolute - session.origin, FallingKind::Sand);
                assert_eq!(
                    session.sand_ticks.len(),
                    1,
                    "paused tick must reject duplicates"
                );

                let block_order = *session.sand_ticks.first_key_value().unwrap().0;
                let fluid_order = *session.fluid_ticks.first_key_value().unwrap().0;
                for _ in 0..3 {
                    session.tick += 1;
                    session.game_time += 1;
                    assert!(session.collect_due_fluid_ticks().is_empty());
                    session.advance_block_ticks();
                    assert_eq!(
                        *session.sand_ticks.first_key_value().unwrap().0,
                        block_order
                    );
                    assert_eq!(
                        *session.fluid_ticks.first_key_value().unwrap().0,
                        fluid_order
                    );
                }

                let revision = session.revision;
                if force {
                    let state = session.player_simulation.as_mut().unwrap();
                    state.player.add_ticket(
                        far,
                        native_persistence::chunk_tickets::TicketKind::Forced,
                        31,
                    );
                    state.player.run_updates();
                } else {
                    session.motion.position.x += f64::from(direction * 16);
                    session.update_player_simulation();
                }
                assert_eq!(session.revision, revision);
                assert!(session.simulation_allows_block_ticks(far));
                let fluid = session.collect_due_fluid_ticks();
                assert_eq!(fluid.len(), 1);
                assert_eq!(fluid[0].1.key.x.div_euclid(16), far.x);
                session.advance_block_ticks();
                assert_eq!(session.falling_sand.len(), 3);
                assert!(session.sand_ticks.is_empty());
                assert!(session.fluid_ticks.is_empty());
            }
        }
    }
}

#[test]
fn all_supported_block_tick_kinds_survive_an_inactive_chunk() {
    use crate::simulation_player::PlayerSimulation;
    use crate::simulation_tickets::SimulationTickets;
    use native_persistence::chunk_tickets::TicketKind;

    let kinds = [
        "minecraft:air",
        "minecraft:sand",
        "minecraft:stone_button",
        "minecraft:dirt_path",
        "minecraft:redstone_lamp",
        "minecraft:frosted_ice",
        "minecraft:sculk_sensor",
        "minecraft:water",
        "minecraft:oak_leaves",
        "minecraft:pointed_dripstone",
    ];
    for indexed in [false, true] {
        let template = super::tests::session();
        let home = template.player_chunk().unwrap();
        let far = ChunkPos {
            x: home.x + 2,
            z: home.z,
        };
        let mut world = template.world.as_ref().clone();
        let mut chunk = world.chunks[&home].as_ref().clone();
        chunk.position = far;
        world.chunks.insert(far, Arc::new(chunk));
        world.saved_block_ticks.insert(
            far,
            kinds
                .iter()
                .enumerate()
                .map(|(i, kind)| SavedBlockTick {
                    block: (*kind).into(),
                    position: [far.x * 16 + 3 + i as i32, 2, far.z * 16 + 4],
                    delay: 0,
                    priority: 0,
                })
                .collect(),
        );
        let registration: Vec<_> = world.chunks.keys().copied().collect();
        let mut session =
            LoadedSession::new(world, template.origin, template.bounds, template.player).unwrap();
        session.set_initial_game_time(42).unwrap();
        if indexed {
            assert_eq!(
                session.install_saved_tick_orders(&registration).unwrap(),
                (true, true)
            );
        }
        let mut player = PlayerSimulation::new(SimulationTickets::default(), 0);
        player.observe_player(session.player_section().unwrap(), false, true);
        player.run_updates();
        session.player_simulation = Some(PlayerSimulationState {
            player,
            spectators_generate_chunks: true,
            view_distance: 2,
        });
        let sizes = |session: &LoadedSession| {
            [
                session.air_ticks.len() + session.air_deferred.len(),
                session.sand_ticks.len() + session.sand_deferred.len(),
                session.button_ticks.len() + session.button_deferred.len(),
                session.soil_ticks.len() + session.soil_deferred.len(),
                session.lamp_ticks.len() + session.lamp_deferred.len(),
                session.ice_ticks.len() + session.ice_deferred.len(),
                session.sensor_ticks.len() + session.sensor_deferred.len(),
                session.bubble_ticks.len() + session.bubble_deferred.len(),
                session.leaf_ticks.len() + session.leaf_deferred.len(),
                session.speleothem_ticks.len() + session.speleothem_deferred.len(),
            ]
        };
        assert_eq!(sizes(&session), [1; 10]);
        session.advance_block_ticks();
        assert_eq!(sizes(&session), [1; 10]);
        let state = session.player_simulation.as_mut().unwrap();
        state.player.add_ticket(far, TicketKind::Forced, 31);
        state.player.run_updates();
        session.advance_block_ticks();
        assert_eq!(sizes(&session), [0; 10]);
    }
}

#[test]
fn scheduled_ticks_resume_consistently_across_frame_rates() {
    use crate::simulation_player::PlayerSimulation;
    use crate::simulation_tickets::SimulationTickets;

    fn start() -> (LoadedSession, ChunkPos, IVec3) {
        let template = super::tests::session();
        let home = template.player_chunk().unwrap();
        let ring = ChunkPos {
            x: home.x,
            z: home.z + 1,
        };
        let far = ChunkPos {
            x: home.x,
            z: home.z + 2,
        };
        let mut world = template.world.as_ref().clone();
        for pos in [ring, far] {
            let mut chunk = world.chunks[&home].as_ref().clone();
            chunk.position = pos;
            world.chunks.insert(pos, Arc::new(chunk));
        }
        let target = IVec3::new(far.x * 16 + 3, 2, far.z * 16 + 4);
        let sand = world.intern_state(state("minecraft:sand", &[]));
        world
            .set_block_id(target.x, target.y, target.z, sand)
            .unwrap();
        world.saved_block_ticks.insert(
            far,
            vec![SavedBlockTick {
                block: "minecraft:sand".into(),
                position: target.to_array(),
                delay: 0,
                priority: 0,
            }],
        );
        let mut session =
            LoadedSession::new(world, template.origin, template.bounds, template.player).unwrap();
        session.set_falling_fire_tag(Arc::new(BTreeSet::new()));
        session.set_random_tick_speed(0);
        session.set_game_mode(GameMode::Spectator);
        session.motion.position = DVec3::new(8.5, 2.0, 15.5);
        session.player.position = session.motion.position.as_vec3();
        let mut player = PlayerSimulation::new(SimulationTickets::default(), 0);
        player.observe_player(session.player_section().unwrap(), true, true);
        player.run_updates();
        session.player_simulation = Some(PlayerSimulationState {
            player,
            spectators_generate_chunks: true,
            view_distance: 2,
        });
        session.submit_command(PlayerCommand::Move {
            forward: -1.0,
            strafe: 0.0,
            jump: false,
        });
        (session, home, target)
    }

    let (mut fixed, home, target) = start();
    let (mut fine, _, _) = start();
    for _ in 0..40 {
        fixed.update(Duration::from_millis(50));
    }
    for _ in 0..200 {
        fine.update(Duration::from_millis(10));
    }
    assert!(fixed.player_chunk().unwrap().z > home.z);
    assert_eq!(fixed.player_chunk(), fine.player_chunk());
    assert_eq!(fixed.motion.position, fine.motion.position);
    assert_eq!(fixed.sand_ticks, fine.sand_ticks);
    assert_eq!(fixed.sand_deferred, fine.sand_deferred);
    assert_eq!(fixed.falling_sand.len(), fine.falling_sand.len());
    let state = |session: &LoadedSession, position: IVec3| {
        session
            .world
            .block(position.x, position.y, position.z)
            .unwrap()
            .name
            .clone()
    };
    assert_eq!(state(&fixed, target), "minecraft:air");
    assert_eq!(state(&fixed, target), state(&fine, target));
    assert_eq!(
        state(&fixed, target - IVec3::Y),
        state(&fine, target - IVec3::Y)
    );
}
