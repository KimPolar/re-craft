use super::{tests::session, *};

fn source_at(session: &LoadedSession, position: ChunkPos) -> World {
    let mut source = session.world.as_ref().clone();
    let template = source.chunks.values().next().unwrap();
    let mut chunk = template.as_ref().clone();
    chunk.position = position;
    source.chunks.clear();
    source.chunks.insert(position, Arc::new(chunk));
    source.saved_block_ticks.clear();
    source.saved_fluid_ticks.clear();
    source
}

fn configure_index(session: &mut LoadedSession, indexed: bool) {
    session.set_initial_game_time(100).unwrap();
    if indexed {
        let registration = session.world.chunks.keys().copied().collect::<Vec<_>>();
        assert_eq!(
            session.install_saved_tick_orders(&registration).unwrap(),
            (true, true)
        );
    }
}

#[test]
fn clean_streamed_chunk_reregisters_saved_ticks_after_eviction() {
    for indexed in [false, true] {
        let mut session = session();
        configure_index(&mut session, indexed);
        let home = session.player_chunk().unwrap();
        let neighbor = ChunkPos {
            x: home.x + 1,
            z: home.z,
        };
        let source = source_at(&session, neighbor);
        session.queue_chunk_window(home, 1);
        session.queue_loaded_chunk(neighbor, source);
        session.apply_streaming();
        assert!(session.fluid_chunks_seen.contains(&neighbor));
        assert!(!session.has_unsaved_chunk_changes(neighbor));

        session.queue_chunk_window(home, 0);
        session.apply_streaming();
        assert!(!session.has_resident_or_parked_chunk(neighbor));
        assert!(!session.fluid_chunks_seen.contains(&neighbor));
        for order in [
            &session.generated_block_order,
            &session.generated_fluid_order,
        ]
        .into_iter()
        .flatten()
        {
            assert!(!order.parked.contains_key(&neighbor));
            assert!(order.queue.container(neighbor).is_none());
        }

        let mut source = source_at(&session, neighbor);
        let position = [neighbor.x * 16 + 1, 2, neighbor.z * 16 + 1];
        source.saved_block_ticks.insert(
            neighbor,
            vec![SavedBlockTick {
                block: "minecraft:sand".into(),
                position,
                delay: 5,
                priority: 0,
            }],
        );
        source.saved_fluid_ticks.insert(
            neighbor,
            vec![SavedFluidTick {
                fluid: "minecraft:water".into(),
                position,
                delay: 5,
                priority: 0,
            }],
        );
        // The chunk may be revisited many ticks after its original unload.
        session.tick = 17;
        session.queue_chunk_window(home, 1);
        session.queue_loaded_chunk(neighbor, source);
        session.apply_streaming();
        assert!(session.fluid_chunks_seen.contains(&neighbor));
        assert_eq!(session.sand_ticks.len(), 1);
        assert_eq!(session.fluid_ticks.len(), 1);
        assert_eq!(session.generated_block_order.is_some(), indexed);
        assert_eq!(session.generated_fluid_order.is_some(), indexed);
        session.queue_chunk_window(home, 0);
        session.apply_streaming();
        assert!(session.parked_chunks.contains_key(&neighbor));
        assert!(session.fluid_chunks_seen.contains(&neighbor));
        for order in [
            &session.generated_block_order,
            &session.generated_fluid_order,
        ]
        .into_iter()
        .flatten()
        {
            assert!(order.parked.contains_key(&neighbor));
        }
        session.queue_chunk_window(home, 1);
        session.apply_streaming();
        assert_eq!(session.sand_ticks.len(), 1);
        assert_eq!(session.fluid_ticks.len(), 1);
        session.tick = 21;
        assert!(session.collect_due_fluid_ticks().is_empty());
        assert_eq!(session.sand_ticks.len(), 1);
        session.tick = 22;
        assert_eq!(session.collect_due_fluid_ticks().len(), 1);
        session.advance_block_ticks();
        assert!(session.sand_ticks.is_empty());
    }
}

#[test]
fn clean_parked_chunk_releases_tick_containers_when_no_longer_needed() {
    for indexed in [false, true] {
        let mut session = session();
        configure_index(&mut session, indexed);
        let home = session.player_chunk().unwrap();
        let neighbor = ChunkPos {
            x: home.x + 1,
            z: home.z,
        };
        let source = source_at(&session, neighbor);
        session.queue_chunk_window(home, 1);
        session.queue_loaded_chunk(neighbor, source);
        session.apply_streaming();
        session.queue_respawn_window(Some((neighbor, 0)));
        session.queue_chunk_window(home, 0);
        session.apply_streaming();
        assert!(session.parked_chunks.contains_key(&neighbor));
        assert!(session.fluid_chunks_seen.contains(&neighbor));
        session.queue_respawn_window(None);
        session.apply_streaming();
        assert!(!session.has_resident_or_parked_chunk(neighbor));
        assert!(!session.fluid_chunks_seen.contains(&neighbor));
        for order in [
            &session.generated_block_order,
            &session.generated_fluid_order,
        ]
        .into_iter()
        .flatten()
        {
            assert!(!order.parked.contains_key(&neighbor));
            assert!(order.queue.container(neighbor).is_none());
        }
    }
}

#[test]
fn nonempty_index_container_is_not_forgotten_if_dirty_tracking_is_missing() {
    use crate::worldgen::chunk_ticks::{ScheduleOutcome, ScheduledTick};

    let mut session = session();
    configure_index(&mut session, true);
    let home = session.player_chunk().unwrap();
    let neighbor = ChunkPos {
        x: home.x + 1,
        z: home.z,
    };
    let source = source_at(&session, neighbor);
    session.queue_chunk_window(home, 1);
    session.queue_loaded_chunk(neighbor, source);
    session.apply_streaming();
    let position = [neighbor.x * 16 + 1, 2, neighbor.z * 16 + 1];
    assert_eq!(
        session
            .generated_block_order
            .as_mut()
            .unwrap()
            .queue
            .schedule(ScheduledTick {
                kind: "minecraft:sand".into(),
                position,
                trigger: 105,
                priority: 0,
                sub_tick: 0,
            })
            .unwrap(),
        ScheduleOutcome::Added
    );
    assert!(!session.has_unsaved_chunk_changes(neighbor));
    session.queue_chunk_window(home, 0);
    session.apply_streaming();
    assert!(session.parked_chunks.contains_key(&neighbor));
    assert!(session.fluid_chunks_seen.contains(&neighbor));
    assert!(
        session.generated_block_order.as_ref().unwrap().parked[&neighbor]
            .contains("minecraft:sand", position)
            .unwrap()
    );
}

#[test]
fn clean_chunk_streaming_does_not_retain_visited_tick_containers() {
    for indexed in [false, true] {
        let mut session = session();
        configure_index(&mut session, indexed);
        let home = session.player_chunk().unwrap();
        let source = source_at(&session, home);
        let template = Arc::clone(&source.chunks[&home]);
        for x in 1..=64 {
            let position = ChunkPos {
                x: home.x + x,
                z: home.z,
            };
            let mut next = source.clone();
            next.chunks.clear();
            let mut chunk = template.as_ref().clone();
            chunk.position = position;
            next.chunks.insert(position, Arc::new(chunk));
            session.queue_chunk_window(position, 0);
            session.queue_loaded_chunk(position, next);
            session.apply_streaming();
            assert_eq!(session.fluid_chunks_seen, BTreeSet::from([position]));
            for order in [
                &session.generated_block_order,
                &session.generated_fluid_order,
            ]
            .into_iter()
            .flatten()
            {
                assert!(order.parked.is_empty());
                assert!(order.queue.container(position).is_some());
            }
        }
    }
}
