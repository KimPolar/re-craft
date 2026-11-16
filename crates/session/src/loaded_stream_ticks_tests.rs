use super::{
    tests::{fixture_chunk_document, session, test_heightmap_tags},
    *,
};
use native_persistence::{
    nbt::{NamedTag, Tag},
    world::{ChunkArea, DimensionId, LoadedChunk},
};
use native_world::loaded::{SavedBlockEntities, SavedChunkActivation};

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

fn ticked_source(position: ChunkPos) -> (World, NamedTag) {
    let saved_pos = native_persistence::world::ChunkPos {
        x: position.x,
        z: position.z,
    };
    let original = fixture_chunk_document(position);
    let mut draft =
        native_persistence::edit::draft_chunk_blocks(&original, saved_pos, &[]).unwrap();
    let block = [position.x * 16 + 1, 2, position.z * 16 + 1];
    draft
        .replace_block_ticks(&[SavedBlockTick {
            block: "minecraft:sand".into(),
            position: block,
            delay: 20,
            priority: -1,
        }])
        .unwrap();
    draft
        .replace_fluid_ticks(&[SavedFluidTick {
            fluid: "minecraft:water".into(),
            position: block,
            delay: 30,
            priority: 1,
        }])
        .unwrap();
    let original = draft.document().clone();
    (materialize_source(position, &original), original)
}

fn materialize_source(position: ChunkPos, document: &NamedTag) -> World {
    let saved_pos = native_persistence::world::ChunkPos {
        x: position.x,
        z: position.z,
    };
    let sections = native_persistence::palette::decode_sections(&document.value).unwrap();
    let area = ChunkArea {
        center: saved_pos,
        radius: 0,
        chunks: BTreeMap::from([(
            saved_pos,
            Arc::new(LoadedChunk {
                position: saved_pos,
                data_version: 5023,
                status: Some("minecraft:full".into()),
                document: Arc::new(document.clone()),
                sections,
            }),
        )]),
        missing: vec![],
    };
    area.materialize(&DimensionId::parse("overworld").unwrap())
        .unwrap()
}

fn queue_tracked_source(session: &mut LoadedSession, pos: ChunkPos, source: World) {
    session.queue_streamed_chunk(
        pos,
        Some(source),
        Some(SavedEntityData::Empty),
        Some(SavedPoiData::Missing),
        None,
        None,
    );
}

fn tracked_tick_session() -> (LoadedSession, ChunkPos, ChunkPos, NamedTag) {
    let mut session = session();
    let home = session.player_chunk().unwrap();
    let neighbor = ChunkPos {
        x: home.x + 1,
        z: home.z,
    };
    let world = Arc::make_mut(&mut session.world);
    world.remember_saved_chunk_source_with_activation(home, [17; 32], SavedChunkActivation::Empty);
    world
        .saved_block_entities
        .insert(home, SavedBlockEntities::Empty);
    session.require_saved_chunk_sources().unwrap();
    configure_index(&mut session, true);
    let (source, original) = ticked_source(neighbor);
    session.queue_chunk_window(home, 1);
    queue_tracked_source(&mut session, neighbor, source);
    session.apply_streaming();
    assert!(session.saved_chunk_preparation_verified(neighbor));
    assert!(session.generated_block_order.is_some());
    assert!(session.generated_fluid_order.is_some());
    session.tick = 3;
    session.mark_advanced_saved_tick_chunks_dirty();
    (session, home, neighbor, original)
}

#[test]
fn streamed_pending_ticks_keep_delays_across_unload_and_reload() {
    for indexed in [false, true] {
        let mut session = session();
        let home = session.player_chunk().unwrap();
        let neighbor = ChunkPos {
            x: home.x + 1,
            z: home.z,
        };
        let world = Arc::make_mut(&mut session.world);
        world.remember_saved_chunk_source_with_activation(
            home,
            [17; 32],
            SavedChunkActivation::Empty,
        );
        world
            .saved_block_entities
            .insert(home, SavedBlockEntities::Empty);
        session.require_saved_chunk_sources().unwrap();
        configure_index(&mut session, indexed);
        let (_, original) = ticked_source(neighbor);
        let clean = materialize_source(neighbor, &original)
            .saved_chunk_activation(neighbor)
            .unwrap()
            .clone();
        let mut marked = original.clone();
        let saved = [neighbor.x * 16 + 1, 2, neighbor.z * 16 + 1];
        let new = [neighbor.x * 16 + 2, 2, neighbor.z * 16 + 1];
        let mut postprocessing = native_core::postprocessing::PostProcessing::new(0, 32).unwrap();
        assert!(postprocessing.mark(saved[0], saved[1], saved[2]));
        let Tag::Compound(fields) = &mut marked.value else {
            panic!("expected chunk compound")
        };
        fields.push((
            "PostProcessing".into(),
            native_persistence::postprocessing::encode(&postprocessing),
        ));
        session.tick = 5;
        session.queue_chunk_window(home, 1);
        queue_tracked_source(
            &mut session,
            neighbor,
            materialize_source(neighbor, &marked),
        );
        session.apply_streaming();
        assert!(!session.saved_chunk_preparation_verified(neighbor));
        assert_eq!(
            session.pending_saved_tick_activation[&neighbor].registered_at,
            5
        );
        session.tick = 15;
        let saved_relative = IVec3::from_array(saved) - session.origin;
        let new_relative = IVec3::from_array(new) - session.origin;
        session.schedule_sand(saved_relative, FallingKind::Sand);
        session.schedule_sand(new_relative, FallingKind::Sand);
        let water = FluidState {
            kind: FluidKind::Water,
            amount: 8,
            source: true,
            falling: false,
        };
        session.schedule_fluid(saved_relative, water, 3);
        session.schedule_fluid(new_relative, water, 3);
        session.queue_chunk_window(home, 0);
        session.apply_streaming();
        assert!(session.parked_chunks.contains_key(&neighbor));
        session.tick = 20;
        session.queue_chunk_window(home, 1);
        session.apply_streaming();
        assert_eq!(
            session.pending_saved_tick_activation[&neighbor].registered_at,
            5
        );
        session.tick = 25;
        Arc::make_mut(&mut session.world)
            .remember_saved_chunk_source_with_activation(neighbor, [18; 32], clean);
        session.inspect_saved_chunk_preparation();
        assert!(session.saved_chunk_preparation_verified(neighbor));
        assert!(session.validate_saved_chunk_preparation().is_err());
        let blocks: Vec<_> = session
            .sand_ticks
            .iter()
            .map(|(order, key)| (order.0, key.x))
            .collect();
        let fluids: Vec<_> = session
            .fluid_ticks
            .iter()
            .map(|(order, tick)| (order.0, tick.key.x))
            .collect();
        assert_eq!(blocks, [(17, new[0]), (45, saved[0])], "indexed={indexed}");
        assert_eq!(fluids, [(18, new[0]), (55, saved[0])], "indexed={indexed}");
        assert_eq!(session.generated_block_order.is_some(), indexed);
        assert_eq!(session.generated_fluid_order.is_some(), indexed);
    }
}

#[test]
fn tick_only_saved_chunks_release_terrain_and_restore_their_schedules() {
    let (mut session, home, neighbor, original) = tracked_tick_session();
    let weak = Arc::downgrade(&session.world.chunks[&neighbor]);
    let block_ticks = session.draft_block_ticks(neighbor).unwrap();
    let fluid_ticks = session.draft_fluid_ticks(neighbor).unwrap();
    let source = session.world.saved_chunk_source(neighbor).unwrap();
    session.queue_chunk_window(home, 0);
    session.apply_streaming();
    assert!(!session.has_resident_or_parked_chunk(neighbor));
    assert!(
        weak.upgrade().is_none(),
        "unloaded terrain remained allocated"
    );
    assert_eq!(session.world.saved_chunk_source(neighbor), Some(source));
    assert_eq!(session.draft_block_ticks(neighbor).unwrap(), block_ticks);
    assert_eq!(session.draft_fluid_ticks(neighbor).unwrap(), fluid_ticks);
    for order in [
        &session.generated_block_order,
        &session.generated_fluid_order,
    ]
    .into_iter()
    .flatten()
    {
        assert!(order.parked.contains_key(&neighbor));
    }
    let draft = session
        .draft_dirty_chunk_with_heightmaps_and_simulated_ticks(
            neighbor,
            &original,
            &test_heightmap_tags(),
            |_| anyhow::bail!("tick-only draft rebuilt heightmaps"),
        )
        .unwrap();
    assert!(draft.matches_source(&original).unwrap());
    assert_eq!(
        draft.document().value.get("sections"),
        original.value.get("sections")
    );
    assert_eq!(
        draft.document().value.get("unknown_chunk_data"),
        original.value.get("unknown_chunk_data")
    );

    let mut stale = original.clone();
    let native_persistence::nbt::Tag::Compound(fields) = &mut stale.value else {
        unreachable!()
    };
    fields.push((
        "new_unknown_data".into(),
        native_persistence::nbt::Tag::Int(1),
    ));
    assert!(
        session
            .draft_dirty_chunk_with_heightmaps_and_simulated_ticks(
                neighbor,
                &stale,
                &test_heightmap_tags(),
                |_| Ok((true, false)),
            )
            .is_err()
    );
    session.queue_chunk_window(home, 1);
    queue_tracked_source(&mut session, neighbor, materialize_source(neighbor, &stale));
    session.apply_streaming();
    assert!(!session.has_resident_or_parked_chunk(neighbor));
    assert_eq!(session.world.saved_chunk_source(neighbor), Some(source));
    let mut spoofed = materialize_source(neighbor, &original);
    let stone = spoofed.intern_state(state("minecraft:stone", &[]));
    spoofed
        .set_block_id(neighbor.x * 16 + 1, 2, neighbor.z * 16 + 1, stone)
        .unwrap();
    assert_eq!(spoofed.saved_chunk_source(neighbor), Some(source));
    queue_tracked_source(&mut session, neighbor, spoofed);
    session.apply_streaming();
    assert!(!session.has_resident_or_parked_chunk(neighbor));
    assert_eq!(session.world.saved_chunk_source(neighbor), Some(source));
    let mut wrong_ticks = materialize_source(neighbor, &original);
    wrong_ticks.saved_block_ticks.get_mut(&neighbor).unwrap()[0].delay += 1;
    queue_tracked_source(&mut session, neighbor, wrong_ticks);
    session.apply_streaming();
    assert!(!session.has_resident_or_parked_chunk(neighbor));
    for (entities, poi) in [
        (SavedEntityData::Unavailable, SavedPoiData::Missing),
        (SavedEntityData::Empty, SavedPoiData::Unavailable),
    ] {
        session.queue_streamed_chunk(
            neighbor,
            Some(materialize_source(neighbor, &original)),
            Some(entities),
            Some(poi),
            None,
            None,
        );
        session.apply_streaming();
        assert!(!session.has_resident_or_parked_chunk(neighbor));
    }
    queue_tracked_source(
        &mut session,
        neighbor,
        materialize_source(neighbor, &original),
    );
    session.apply_streaming();
    assert!(session.world.chunks.contains_key(&neighbor));
    assert!(!session.released_tick_terrain.contains_key(&neighbor));
    assert_eq!(session.draft_block_ticks(neighbor).unwrap(), block_ticks);
    assert_eq!(session.draft_fluid_ticks(neighbor).unwrap(), fluid_ticks);
    assert_eq!(session.sand_ticks.len(), 1);
    assert_eq!(session.fluid_ticks.len(), 1);
    for order in [
        &session.generated_block_order,
        &session.generated_fluid_order,
    ]
    .into_iter()
    .flatten()
    {
        assert!(!order.parked.contains_key(&neighbor));
        assert!(order.queue.container(neighbor).is_some());
    }
}

#[test]
fn bulk_tick_only_unload_is_bounded_and_eventually_releases_every_chunk() {
    let (mut session, home, first, _) = tracked_tick_session();
    session.queue_chunk_window(home, 16);
    session.apply_streaming();

    let mut positions = vec![first];
    for offset in 2..=8 {
        let pos = ChunkPos {
            x: home.x + offset,
            z: home.z,
        };
        let (source, _) = ticked_source(pos);
        queue_tracked_source(&mut session, pos, source);
        session.apply_streaming();
        session.mark_advanced_saved_tick_chunks_dirty();
        positions.push(pos);
    }
    let weak_chunks: Vec<_> = positions
        .iter()
        .map(|pos| Arc::downgrade(&session.world.chunks[pos]))
        .collect();
    let block_ticks: Vec<_> = positions
        .iter()
        .map(|pos| session.draft_block_ticks(*pos).unwrap())
        .collect();
    let fluid_ticks: Vec<_> = positions
        .iter()
        .map(|pos| session.draft_fluid_ticks(*pos).unwrap())
        .collect();
    session.queue_chunk_window(
        ChunkPos {
            x: home.x + 50,
            z: home.z,
        },
        0,
    );
    let mut previous = 0;
    for _ in 0..positions.len() {
        session.apply_streaming();
        let released = positions
            .iter()
            .filter(|pos| session.released_tick_terrain.contains_key(pos))
            .count();
        assert!(released > previous);
        assert!(released - previous <= RELEASED_TICK_TERRAIN_BUDGET);
        for (index, pos) in positions.iter().enumerate() {
            assert_eq!(session.draft_block_ticks(*pos).unwrap(), block_ticks[index]);
            assert_eq!(session.draft_fluid_ticks(*pos).unwrap(), fluid_ticks[index]);
        }
        previous = released;
        if released == positions.len() {
            break;
        }
    }
    assert_eq!(previous, positions.len());
    assert!(weak_chunks.iter().all(|chunk| chunk.upgrade().is_none()));
    assert_eq!(session.sand_ticks.len(), positions.len());
    assert_eq!(session.fluid_ticks.len(), positions.len());
}

#[test]
fn tick_only_unloads_do_not_keep_visited_terrain_allocated() {
    let (mut session, home, first, _) = tracked_tick_session();
    let mut previous = first;
    let mut released = Vec::new();
    for offset in 2..=33 {
        let weak = Arc::downgrade(&session.world.chunks[&previous]);
        let next = ChunkPos {
            x: home.x + offset,
            z: home.z,
        };
        let (source, _) = ticked_source(next);
        session.queue_chunk_window(next, 0);
        queue_tracked_source(&mut session, next, source);
        session.apply_streaming();
        assert!(weak.upgrade().is_none(), "visited chunk remained allocated");
        released.push(previous);
        assert_eq!(session.released_tick_terrain.len(), released.len());
        assert_eq!(session.world.chunks.len(), 1);
        assert_eq!(session.sand_ticks.len(), offset as usize);
        assert_eq!(session.fluid_ticks.len(), offset as usize);
        session.tick += 1;
        session.mark_advanced_saved_tick_chunks_dirty();
        previous = next;
    }
    assert!(
        released
            .iter()
            .all(|pos| session.world.saved_chunk_source(*pos).is_some())
    );
}

#[test]
fn released_ticks_run_in_the_same_order_as_retained_chunks() {
    let (mut released, home, neighbor, original) = tracked_tick_session();
    let (mut retained, _, _, _) = tracked_tick_session();
    retained
        .original_light_chunks
        .insert(neighbor, Arc::clone(&retained.world.chunks[&neighbor]));
    for session in [&mut released, &mut retained] {
        session.queue_chunk_window(home, 0);
        session.apply_streaming();
    }
    assert!(!released.has_resident_or_parked_chunk(neighbor));
    assert!(retained.parked_chunks.contains_key(&neighbor));
    for session in [&mut released, &mut retained] {
        session.tick = 19;
        session.queue_chunk_window(home, 1);
        if !session.has_resident_or_parked_chunk(neighbor) {
            queue_tracked_source(session, neighbor, materialize_source(neighbor, &original));
        }
        session.apply_streaming();
        assert_eq!(session.draft_block_ticks(neighbor).unwrap()[0].delay, 1);
        session.tick = 20;
        session.advance_block_ticks();
        assert!(session.sand_ticks.is_empty());
        session.tick = 30;
    }
    let collect = |session: &mut LoadedSession| {
        session
            .collect_due_fluid_ticks()
            .into_iter()
            .map(|(order, tick)| (order, tick.key))
            .collect::<Vec<_>>()
    };
    let actual = collect(&mut released);
    assert_eq!(actual.len(), 1);
    assert_eq!(actual, collect(&mut retained));
}

#[test]
fn tick_only_unload_keeps_terrain_with_other_unsaved_effects() {
    for case in ["blocks", "light", "entity", "poi", "unindexed", "shape"] {
        let (mut session, home, neighbor, _) = tracked_tick_session();
        match case {
            "blocks" => {
                session.dirty_chunks.insert(neighbor);
            }
            "light" => {
                session
                    .original_light_chunks
                    .insert(neighbor, Arc::clone(&session.world.chunks[&neighbor]));
            }
            "entity" => {
                session
                    .entity_chunks
                    .insert(neighbor, SavedEntityData::Unavailable);
            }
            "poi" => {
                session
                    .poi_chunks
                    .insert(neighbor, SavedPoiData::Unavailable);
            }
            "unindexed" => {
                session.generated_block_order = None;
            }
            "shape" => {
                let pos = BlockPosition {
                    x: neighbor.x * 16 + 1,
                    y: 2,
                    z: neighbor.z * 16 + 1,
                };
                let missing = BlockPosition {
                    x: pos.x + 16,
                    ..pos
                };
                session
                    .pending_lichen_updates
                    .push_back((pos, missing, session.states[0]));
            }
            _ => unreachable!(),
        }
        session.queue_chunk_window(home, 0);
        session.apply_streaming();
        assert!(
            session.parked_chunks.contains_key(&neighbor),
            "released {case} terrain"
        );
        assert!(!session.released_tick_terrain.contains_key(&neighbor));
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
        Arc::make_mut(&mut session.generated_block_order.as_mut().unwrap().queue)
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
