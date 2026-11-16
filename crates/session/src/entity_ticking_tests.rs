use super::*;

fn position() -> ChunkPos {
    ChunkPos { x: 0, z: 0 }
}

fn future_id(index: usize) -> FullFutureId {
    FullFutureId(index as u64 + 1)
}

fn label(state: TickingFutureState) -> &'static str {
    match state {
        TickingFutureState::Waiting => "waiting",
        TickingFutureState::Available => "available",
        TickingFutureState::Unavailable => "unavailable",
        TickingFutureState::Exceptional => "exception",
    }
}

fn event_labels(queue: &mut TickingPreparations) -> Vec<String> {
    let mut result = Vec::new();
    while let Some(event) = queue.next_executor_event() {
        match event {
            TickingExecutorEvent::BlockQueued(_) => result.push("block-queued".into()),
            TickingExecutorEvent::EntityListener { listener, state } => {
                if state != TickingFutureState::Exceptional {
                    result.push(format!(
                        "listener:{}",
                        match listener.0 {
                            0 => "A",
                            1 => "B",
                            2 => "C",
                            3 => "D",
                            other => panic!("unexpected listener {other}"),
                        }
                    ));
                }
            }
        }
    }
    result
}

fn basic_trace(name: &str) -> Result<String> {
    let mut queue = TickingPreparations::with_entity_tracking();
    let dependencies = entity_ticking_dependencies(position());
    let mut requests = Vec::new();
    let id_for = |index| {
        future_id(if name == "shared" && index == 22 {
            2
        } else {
            index
        })
    };
    queue.start_entity(EntityRequestId(1), position(), |pos| {
        let index = dependencies.iter().position(|&entry| entry == pos).unwrap();
        if name == "missing" && index == 1 {
            return None;
        }
        requests.push(format!("request:{},{}", pos.x, pos.z));
        Some(FullFuture {
            id: id_for(index),
            result: (name == "ready").then_some(FullFutureResult::Full),
        })
    })?;
    queue.register_entity_listener(EntityRequestId(1), EntityListenerId(0))?;
    queue.register_entity_listener(EntityRequestId(1), EntityListenerId(1))?;
    queue.register_entity_listener(EntityRequestId(1), EntityListenerId(2))?;
    queue.register_entity_listener(EntityRequestId(1), EntityListenerId(3))?;
    let initial = queue.entity_state(EntityRequestId(1)).unwrap();
    if name != "missing" && name != "ready" {
        for index in 0..24 {
            let result = match (name, index) {
                ("unavailable", 2) => FullFutureResult::Unavailable,
                ("exception", 2) => FullFutureResult::Exceptional,
                _ => FullFutureResult::Full,
            };
            queue.complete(id_for(index), result)?;
        }
    }
    let before_last = queue.entity_state(EntityRequestId(1)).unwrap();
    let cancelled = name == "cancel" && queue.cancel_entity(EntityRequestId(1))?;
    if name != "missing" && name != "ready" {
        queue.complete(id_for(24), FullFutureResult::Full)?;
    }
    let final_state = queue.entity_state(EntityRequestId(1)).unwrap();
    Ok(format!(
        "basic:{name}|{}|{}|{}|{cancelled}|{}|{}",
        requests.join(";"),
        label(initial),
        label(before_last),
        label(final_state),
        event_labels(&mut queue).join(";")
    ))
}

fn overlap_trace(entity_first: bool, shared_index: usize, duplicate: bool) -> Result<String> {
    let mut queue = TickingPreparations::with_entity_tracking();
    let dependencies = entity_ticking_dependencies(position());
    let id_for = |index| {
        future_id(if duplicate && index == 0 {
            shared_index
        } else {
            index
        })
    };
    let request = |pos| {
        let index = dependencies.iter().position(|&entry| entry == pos).unwrap();
        Some(FullFuture {
            id: id_for(index),
            result: None,
        })
    };
    if entity_first {
        queue.start_entity(EntityRequestId(1), position(), request)?;
        queue.start(TickingRequestId(1), position(), request)?;
    } else {
        queue.start(TickingRequestId(1), position(), request)?;
        queue.start_entity(EntityRequestId(1), position(), request)?;
    }
    queue.register_entity_listener(EntityRequestId(1), EntityListenerId(0))?;
    queue.register_entity_listener(EntityRequestId(1), EntityListenerId(1))?;
    for index in 0..25 {
        if id_for(index) != id_for(shared_index) {
            queue.complete(id_for(index), FullFutureResult::Full)?;
        }
    }
    let before = format!(
        "{},{}",
        label(queue.entity_state(EntityRequestId(1)).unwrap()),
        label(queue.state(TickingRequestId(1)).unwrap())
    );
    queue.complete(id_for(shared_index), FullFutureResult::Full)?;
    let after = format!(
        "{},{}",
        label(queue.entity_state(EntityRequestId(1)).unwrap()),
        label(queue.state(TickingRequestId(1)).unwrap())
    );
    Ok(format!(
        "overlap:{entity_first}:{shared_index}:{duplicate}|{before}|{after}|{}",
        event_labels(&mut queue).join(";")
    ))
}

#[test]
fn entity_holder_demotes_and_releases_pending_future_in_reverse_order() -> Result<()> {
    let mut queue = TickingPreparations::with_entity_tracking();
    let dependencies = entity_ticking_dependencies(position());
    let mut holder = EntityTickingHolder::new(position(), 31, &queue);
    assert!(holder.needs_new_entity_request());
    assert_eq!(
        holder.update(&mut queue, Some(EntityRequestId(3)), |pos| Some(
            FullFuture {
                id: future_id(dependencies.iter().position(|&entry| entry == pos).unwrap()),
                result: None,
            }
        ))?,
        EntityTickingTransition::Promoted(EntityRequestId(3))
    );
    holder.register_listener(&mut queue, EntityListenerId(0))?;
    holder.register_listener(&mut queue, EntityListenerId(1))?;
    assert!(queue.retire_entity(EntityRequestId(3)).is_err());
    assert!(queue.forget_full_future(future_id(0)).is_err());
    holder.set_ticket_level(32);
    assert_eq!(
        holder.update(&mut queue, None, |_| unreachable!())?,
        EntityTickingTransition::Demoted {
            previous: EntityRequestId(3),
            completed_as_unavailable: true,
        }
    );
    assert_eq!(holder.state(&queue)?, TickingFutureState::Unavailable);
    assert_eq!(event_labels(&mut queue), ["listener:B", "listener:A"]);
    assert!(queue.retire_entity(EntityRequestId(3))?);
    assert!(queue.forget_full_future(future_id(0))?);
    Ok(())
}

#[test]
fn entity_holder_rejects_foreign_queue_and_duplicate_listener() -> Result<()> {
    let mut first = TickingPreparations::with_entity_tracking();
    let mut other = TickingPreparations::with_entity_tracking();
    let holder = EntityTickingHolder::new(position(), 32, &first);
    assert!(holder.state(&other).is_err());
    assert!(
        holder
            .register_listener(&mut other, EntityListenerId(0))
            .is_err()
    );
    holder.register_listener(&mut first, EntityListenerId(0))?;
    assert!(
        holder
            .register_listener(&mut first, EntityListenerId(0))
            .is_err()
    );
    assert_eq!(event_labels(&mut first), ["listener:A"]);
    holder.register_listener(&mut first, EntityListenerId(0))?;
    assert_eq!(event_labels(&mut first), ["listener:A"]);
    Ok(())
}

#[test]
fn entity_dependency_order_wraps_at_java_integer_boundaries() {
    let dependencies = entity_ticking_dependencies(ChunkPos {
        x: i32::MAX,
        z: i32::MIN,
    });
    assert_eq!(
        dependencies[0],
        ChunkPos {
            x: i32::MAX - 2,
            z: i32::MAX - 1
        }
    );
    assert_eq!(
        dependencies[12],
        ChunkPos {
            x: i32::MAX,
            z: i32::MIN
        }
    );
    assert_eq!(
        dependencies[24],
        ChunkPos {
            x: i32::MIN + 1,
            z: i32::MIN + 2
        }
    );
}

fn pump_player(
    player: &mut PlayerSimulation,
    changes: &mut Vec<crate::simulation_player::PlayerLoadingTicketChange>,
) {
    for _ in 0..100 {
        while player.step_player_loading_dispatcher() {}
        if !player.step_player_loading_main(|event| changes.push(event)) {
            break;
        }
    }
    while player.step_player_loading_dispatcher() {}
}

fn position_id(pos: ChunkPos) -> FullFutureId {
    FullFutureId(u64::from(pos.x as u32) | (u64::from(pos.z as u32) << 32))
}

#[test]
fn player_loading_links_wait_for_the_matching_holder_future() -> Result<()> {
    use crate::simulation_player::PlayerSection;
    use crate::simulation_tickets::SimulationTickets;
    use native_persistence::chunk_tickets::TicketKind;

    for exceptional in [false, true] {
        let mut player = PlayerSimulation::new(SimulationTickets::with_loading_tracker(), 2);
        player.set_loading_view_distance(2);
        player.observe_player(PlayerSection { x: 0, y: 0, z: 0 }, false, true);
        player.run_updates();
        let mut changes = Vec::new();
        pump_player(&mut player, &mut changes);
        assert_eq!(changes.len(), 4);
        let positions: Vec<_> = player
            .tickets()
            .tickets()
            .filter(|ticket| ticket.kind == TicketKind::PlayerLoading)
            .map(|ticket| ChunkPos {
                x: ticket.position.x,
                z: ticket.position.z,
            })
            .collect();
        assert_eq!(positions.len(), 4);
        assert!(positions.contains(&position()));

        let mut queue = TickingPreparations::with_entity_tracking();
        let mut links = EntityLoadingLinks::new(&queue)?;
        let mut holders = BTreeMap::new();
        for (index, &pos) in positions.iter().enumerate() {
            let mut holder = EntityTickingHolder::new(pos, 31, &queue);
            holder.update(
                &mut queue,
                Some(EntityRequestId(index as u64)),
                |neighbor| {
                    Some(FullFuture {
                        id: position_id(neighbor),
                        result: None,
                    })
                },
            )?;
            holders.insert(pos, holder);
        }
        player.register_player_loading_releases(|token| {
            let holder = holders.get(&token.position()).unwrap();
            links.register(&mut queue, holder, token)
        })?;
        assert_eq!(links.pending_len(), 4);

        let dependencies = entity_ticking_dependencies(position());
        for (index, &pos) in dependencies.iter().enumerate().take(24) {
            queue.complete(
                position_id(pos),
                if exceptional && index == 2 {
                    FullFutureResult::Exceptional
                } else {
                    FullFutureResult::Full
                },
            )?;
        }
        assert_eq!(queue.next_executor_event(), None);
        pump_player(&mut player, &mut changes);
        assert_eq!(changes.len(), 4);
        queue.complete(position_id(dependencies[24]), FullFutureResult::Full)?;
        let Some(TickingExecutorEvent::EntityListener { listener, state }) =
            queue.next_executor_event()
        else {
            panic!("expected the center holder's completion");
        };
        let mut other_player = PlayerSimulation::new(SimulationTickets::with_loading_tracker(), 2);
        assert!(
            links
                .deliver(&queue, &mut other_player, listener, state)
                .is_err()
        );
        assert_eq!(links.pending_len(), 4);
        assert!(
            links
                .deliver(
                    &TickingPreparations::with_entity_tracking(),
                    &mut player,
                    listener,
                    state
                )
                .is_err()
        );
        links.deliver(&queue, &mut player, listener, state)?;
        assert_eq!(links.pending_len(), 3);
        assert!(links.deliver(&queue, &mut player, listener, state).is_err());
        pump_player(&mut player, &mut changes);
        assert_eq!(changes.len(), if exceptional { 4 } else { 5 });
    }
    Ok(())
}

#[test]
#[cfg(target_os = "windows")]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn entity_ticking_matches_java_26_3() -> Result<()> {
    let expected = crate::worldgen::postprocessing_java_results(
        "EntityTickingFutureOracle",
        include_str!("../tests/java/EntityTickingFutureOracle.java"),
        &[],
    );
    let mut actual = Vec::new();
    for name in [
        "missing",
        "ready",
        "success",
        "unavailable",
        "exception",
        "cancel",
        "shared",
    ] {
        actual.push(basic_trace(name)?);
    }
    for index in [6, 7, 8, 11, 12, 13, 16, 17, 18] {
        for first in [false, true] {
            for duplicate in [false, true] {
                actual.push(overlap_trace(first, index, duplicate)?);
            }
        }
    }
    assert_eq!(actual, expected);
    Ok(())
}
