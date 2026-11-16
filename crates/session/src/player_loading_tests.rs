use super::*;

fn pump(
    loading: &mut PlayerLoading,
    tickets: &mut SimulationTickets,
    limit: usize,
    events: &mut Vec<PlayerLoadingTicketChange>,
) {
    for _ in 0..limit {
        while loading.step_dispatcher() {}
        if !loading.step_main(tickets, |event| events.push(event)) {
            break;
        }
    }
    while loading.step_dispatcher() {}
}

fn pos(key: u64) -> String {
    let position = unpack(key);
    format!("{},{}", position.x, position.z)
}

fn snapshot(
    label: &str,
    loading: &PlayerLoading,
    tickets: &SimulationTickets,
    events: &mut Vec<PlayerLoadingTicketChange>,
) -> String {
    let changes = events
        .drain(..)
        .map(|event| {
            format!(
                "{}{},{}:{}",
                if event.added { "+" } else { "-" },
                event.position.x,
                event.position.z,
                event.changed
            )
        })
        .collect::<Vec<_>>()
        .join(";");
    let active: BTreeSet<_> = tickets
        .tickets()
        .filter(|ticket| ticket.kind == TicketKind::PlayerLoading)
        .map(|ticket| ChunkPos {
            x: ticket.position.x,
            z: ticket.position.z,
        })
        .collect();
    let positions = |positions: &BTreeSet<ChunkPos>| {
        positions
            .iter()
            .map(|position| pos(pack(*position)))
            .collect::<Vec<_>>()
            .join(";")
    };
    let mut queued = Vec::new();
    for (priority, bucket) in loading.priorities.iter().enumerate() {
        let mut current = bucket.first;
        while let Some(key) = current {
            let node = &bucket.nodes[&key];
            queued.push(format!("{priority}:{}:{}", pos(key), node.tasks));
            current = node.next;
        }
    }
    let releases = loading
        .pending_releases
        .ordered()
        .map(pos)
        .collect::<Vec<_>>()
        .join(";");
    let worker = usize::from(loading.commands.iter().any(|queue| !queue.is_empty()));
    let mut samples = Vec::new();
    for x in -3..=3 {
        for z in -2..=2 {
            let key = pack(ChunkPos { x, z });
            if let Some(level) = loading.queue_levels.get(&key) {
                samples.push(format!("{x},{z}:{level}"));
            }
        }
    }
    format!(
        "SNAP|{label}|{changes}|{}|{}|{}|{releases}|{worker},{}|{}",
        positions(&active),
        positions(&loading.acquired),
        queued.join(";"),
        loading.main.len(),
        samples.join(";")
    )
}

fn oracle_trace() -> Vec<String> {
    let mut loading = PlayerLoading::default();
    let mut tickets = SimulationTickets::with_loading_tracker();
    let mut events = Vec::new();
    let mut output = Vec::new();
    let center = ChunkPos { x: 0, z: 0 };
    loading.set_view_distance(2);
    loading.add_player(center);
    loading.run_updates();
    output.push(snapshot("queued", &loading, &tickets, &mut events));
    pump(&mut loading, &mut tickets, 20, &mut events);
    output.push(snapshot("four", &loading, &tickets, &mut events));
    let mut registrations = Vec::new();
    loading
        .register_releases(&tickets, |token| {
            registrations.push(token);
            Ok(())
        })
        .unwrap();
    let center_release = registrations
        .iter()
        .find(|token| token.position() == center)
        .unwrap();
    loading
        .entity_future_completed(center_release, PlayerLoadingFutureOutcome::Completed)
        .unwrap();
    pump(&mut loading, &mut tickets, 10, &mut events);
    output.push(snapshot("release", &loading, &tickets, &mut events));
    loading.remove_player(center);
    loading.add_player(center);
    loading.run_updates();
    pump(&mut loading, &mut tickets, 10, &mut events);
    output.push(snapshot("vertical", &loading, &tickets, &mut events));
    loading.remove_player(center);
    let moved = ChunkPos { x: 1, z: 0 };
    loading.add_player(moved);
    loading.run_updates();
    pump(&mut loading, &mut tickets, 12, &mut events);
    output.push(snapshot("move", &loading, &tickets, &mut events));
    loading.set_view_distance(1);
    pump(&mut loading, &mut tickets, 20, &mut events);
    output.push(snapshot("view", &loading, &tickets, &mut events));
    loading.remove_player(moved);
    loading.run_updates();
    pump(&mut loading, &mut tickets, 80, &mut events);
    output.push(snapshot("removed", &loading, &tickets, &mut events));
    loading.set_view_distance(2);
    loading.add_player(ChunkPos { x: -2, z: 1 });
    loading.run_updates();
    pump(&mut loading, &mut tickets, 20, &mut events);
    output.push(snapshot("return", &loading, &tickets, &mut events));
    let old_release = registrations
        .iter()
        .find(|token| token.position() == ChunkPos { x: -1, z: 1 })
        .unwrap();
    loading
        .entity_future_completed(old_release, PlayerLoadingFutureOutcome::Completed)
        .unwrap();
    pump(&mut loading, &mut tickets, 10, &mut events);
    output.push(snapshot("late-old", &loading, &tickets, &mut events));

    let mut loading = PlayerLoading::default();
    let mut tickets = SimulationTickets::with_loading_tracker();
    let mut events = Vec::new();
    loading.set_view_distance(2);
    loading.add_player(center);
    loading.run_updates();
    pump(&mut loading, &mut tickets, 1, &mut events);
    output.push(snapshot("race-inflight", &loading, &tickets, &mut events));
    loading.remove_player(center);
    loading.add_player(ChunkPos { x: 20, z: 0 });
    loading.run_updates();
    pump(&mut loading, &mut tickets, 40, &mut events);
    output.push(snapshot("race-moved", &loading, &tickets, &mut events));
    output
}

#[test]
fn player_loading_waits_for_a_real_future_before_admitting_more_chunks() {
    let mut loading = PlayerLoading::default();
    let mut tickets = SimulationTickets::with_loading_tracker();
    let mut events = Vec::new();
    loading.set_view_distance(2);
    loading.add_player(ChunkPos { x: 0, z: 0 });
    loading.run_updates();
    pump(&mut loading, &mut tickets, 100, &mut events);
    assert_eq!(loading.acquired.len(), 4);
    assert_eq!(events.len(), 4);
    let mut candidates = Vec::new();
    loading
        .register_releases(&tickets, |token| {
            candidates.push(token);
            Ok(())
        })
        .unwrap();
    assert_eq!(candidates.len(), 4);
    pump(&mut loading, &mut tickets, 100, &mut events);
    assert_eq!(events.len(), 4);
    let center = candidates
        .iter()
        .find(|token| token.position() == ChunkPos { x: 0, z: 0 })
        .unwrap();
    assert!(
        PlayerLoading::default()
            .entity_future_completed(center, PlayerLoadingFutureOutcome::Completed)
            .is_err()
    );
    let exceptional = candidates
        .iter()
        .find(|token| token.position() != center.position())
        .unwrap();
    loading
        .entity_future_completed(exceptional, PlayerLoadingFutureOutcome::Exceptional)
        .unwrap();
    assert!(
        loading
            .entity_future_completed(exceptional, PlayerLoadingFutureOutcome::Completed)
            .is_err()
    );
    loading
        .entity_future_completed(center, PlayerLoadingFutureOutcome::Completed)
        .unwrap();
    assert!(
        loading
            .entity_future_completed(center, PlayerLoadingFutureOutcome::Completed)
            .is_err()
    );
    pump(&mut loading, &mut tickets, 100, &mut events);
    assert_eq!(events.len(), 5);
    assert_eq!(loading.acquired.len(), 4);
}

#[test]
fn moving_before_main_ticket_application_does_not_load_the_old_center() {
    let mut loading = PlayerLoading::default();
    let mut tickets = SimulationTickets::with_loading_tracker();
    let mut events = Vec::new();
    let first = ChunkPos { x: 0, z: 0 };
    loading.set_view_distance(2);
    loading.add_player(first);
    loading.run_updates();
    while loading.step_dispatcher() {}
    assert!(loading.step_main(&mut tickets, |event| events.push(event)));
    loading.remove_player(first);
    loading.add_player(ChunkPos { x: 20, z: 0 });
    loading.run_updates();
    pump(&mut loading, &mut tickets, 20, &mut events);
    assert!(!tickets.tickets().any(|ticket| {
        ticket.kind == TicketKind::PlayerLoading
            && ticket.position.x == first.x
            && ticket.position.z == first.z
    }));
    assert!(
        !events
            .iter()
            .any(|event| event.added && event.position == first)
    );
}

#[test]
fn release_registration_skips_removed_tickets_and_preserves_failures() {
    let mut loading = PlayerLoading::default();
    let mut tickets = SimulationTickets::with_loading_tracker();
    let present = ChunkPos { x: 0, z: 0 };
    let absent = ChunkPos { x: 1, z: 0 };
    loading.pending_releases.insert(pack(present));
    loading.pending_releases.insert(pack(absent));
    tickets.add_ticket(present, TicketKind::PlayerLoading, 31);
    assert!(
        loading
            .register_releases(&tickets, |_| anyhow::bail!("missing holder"))
            .is_err()
    );
    assert_eq!(loading.pending_releases.ordered().count(), 2);
    let mut registered = Vec::new();
    loading
        .register_releases(&tickets, |token| {
            registered.push(token);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        registered
            .iter()
            .map(PlayerLoadingRelease::position)
            .collect::<Vec<_>>(),
        [present]
    );
    assert_eq!(loading.pending_releases.ordered().count(), 0);
}

#[test]
fn failed_release_registration_keeps_earlier_future_callbacks() {
    let mut loading = PlayerLoading::default();
    let mut tickets = SimulationTickets::with_loading_tracker();
    for position in [ChunkPos { x: 0, z: 0 }, ChunkPos { x: 1, z: 0 }] {
        loading.pending_releases.insert(pack(position));
        tickets.add_ticket(position, TicketKind::PlayerLoading, 31);
    }
    let mut earlier = Vec::new();
    assert!(
        loading
            .register_releases(&tickets, |token| {
                if earlier.is_empty() {
                    earlier.push(token);
                    Ok(())
                } else {
                    anyhow::bail!("second holder is missing")
                }
            })
            .is_err()
    );
    assert_eq!(loading.pending_releases.ordered().count(), 2);
    assert_eq!(loading.registered_releases.len(), 1);
    let mut retried = Vec::new();
    loading
        .register_releases(&tickets, |token| {
            retried.push(token);
            Ok(())
        })
        .unwrap();
    assert_eq!(retried.len(), 2);
    assert_eq!(loading.registered_releases.len(), 3);
    loading
        .entity_future_completed(&earlier[0], PlayerLoadingFutureOutcome::Completed)
        .unwrap();
    assert_eq!(loading.main.len(), 1);
}

#[test]
fn player_simulation_delivers_tickets_to_the_loading_graph() {
    use crate::simulation_player::{PlayerSection, PlayerSimulation};

    fn pump_player(player: &mut PlayerSimulation, events: &mut Vec<PlayerLoadingTicketChange>) {
        for _ in 0..100 {
            while player.step_player_loading_dispatcher() {}
            if !player.step_player_loading_main(|event| events.push(event)) {
                break;
            }
        }
        while player.step_player_loading_dispatcher() {}
    }

    let center = ChunkPos { x: 0, z: 0 };
    let mut player = PlayerSimulation::new(SimulationTickets::with_loading_tracker(), 12);
    let mut events = Vec::new();
    player.set_loading_view_distance(2);
    player.observe_player(PlayerSection { x: 0, y: 4, z: 0 }, false, true);
    player.run_updates();
    pump_player(&mut player, &mut events);
    assert_eq!(events.iter().filter(|event| event.added).count(), 4);
    player.run_loading_updates(usize::MAX, |_| {}).unwrap();
    assert_eq!(
        player.tickets().loading_tracker().unwrap().level(center),
        31
    );
    let mut registered = Vec::new();
    player
        .register_player_loading_releases(|token| {
            registered.push(token);
            Ok(())
        })
        .unwrap();
    assert_eq!(registered.len(), 4);
    let center_release = registered
        .iter()
        .find(|token| token.position() == center)
        .unwrap();
    player
        .player_loading_future_completed(center_release, PlayerLoadingFutureOutcome::Completed)
        .unwrap();
    pump_player(&mut player, &mut events);
    assert_eq!(events.iter().filter(|event| event.added).count(), 5);

    player.observe_player(PlayerSection { x: 20, y: 4, z: 0 }, false, true);
    player.run_updates();
    pump_player(&mut player, &mut events);
    assert!(
        !player
            .tickets()
            .has_ticket_kind(center, TicketKind::PlayerLoading)
    );
    player.run_loading_updates(usize::MAX, |_| {}).unwrap();
    assert_eq!(
        player.tickets().loading_tracker().unwrap().level(center),
        unloaded_level()
    );
}

#[test]
fn simulation_setting_updates_do_not_advance_loading_executor() {
    use crate::simulation_player::{PlayerSection, PlayerSimulation};

    let mut player = PlayerSimulation::new(SimulationTickets::with_loading_tracker(), 12);
    player.set_loading_view_distance(2);
    player.observe_player(PlayerSection { x: 0, y: 0, z: 0 }, false, true);
    player.set_distance(8);
    player.run_simulation_updates();
    assert!(player.has_pending_updates());
    assert!(!player.step_player_loading_dispatcher());
    player.run_updates();
    assert!(player.step_player_loading_dispatcher());
}

#[test]
#[cfg(target_os = "windows")]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn player_loading_dispatch_matches_java_26_3() {
    let expected = crate::worldgen::postprocessing_java_results(
        "PlayerLoadingOracle",
        include_str!("../tests/java/PlayerLoadingOracle.java"),
        &[],
    );
    let actual = oracle_trace();
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected.iter()) {
        let left: Vec<_> = actual.split('|').collect();
        let right: Vec<_> = expected.split('|').collect();
        assert_eq!(left.len(), right.len(), "snapshot format");
        for (index, (observed, expected)) in left.iter().zip(right.iter()).enumerate() {
            assert_eq!(observed, expected, "snapshot {} field {index}", left[1]);
        }
    }
}
