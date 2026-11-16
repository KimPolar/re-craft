use super::*;
use native_persistence::{
    nbt::{NamedTag, Tag, TagType},
    world::{ChunkArea, DimensionId, LoadedChunk, TARGET_DATA_VERSION},
};
use native_world::loaded::{SavedBlockEntities, SavedChunkActivation};

fn streamed_chunk(pos: ChunkPos, post: Tag) -> World {
    streamed_chunk_with_ticks(pos, post, vec![])
}

fn streamed_chunk_with_ticks(
    pos: ChunkPos,
    post: Tag,
    ticks: Vec<(native_persistence::nbt::JavaString, Tag)>,
) -> World {
    let saved_pos = native_persistence::world::ChunkPos { x: pos.x, z: pos.z };
    let mut fields = vec![
        ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
        ("xPos".into(), Tag::Int(pos.x)),
        ("zPos".into(), Tag::Int(pos.z)),
        ("Status".into(), Tag::String("minecraft:full".into())),
        ("PostProcessing".into(), post),
    ];
    fields.extend(ticks);
    let document = NamedTag {
        name: "".into(),
        value: Tag::Compound(fields),
    };
    let area = ChunkArea {
        center: saved_pos,
        radius: 0,
        chunks: BTreeMap::from([(
            saved_pos,
            Arc::new(LoadedChunk {
                position: saved_pos,
                data_version: TARGET_DATA_VERSION,
                status: Some("minecraft:full".into()),
                document: Arc::new(document),
                sections: vec![],
            }),
        )]),
        missing: vec![],
    };
    area.materialize(&DimensionId::parse("overworld").unwrap())
        .unwrap()
}

fn captured_ticks(world: &World, pos: ChunkPos) -> SavedChunkActivation {
    let blocks = world
        .saved_block_ticks
        .get(&pos)
        .map_or(&[][..], Vec::as_slice);
    let fluids = world
        .saved_fluid_ticks
        .get(&pos)
        .map_or(&[][..], Vec::as_slice);
    let document = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![
            (
                "block_ticks".into(),
                native_persistence::generated_ticks::encode_level_block_ticks(pos, blocks, |_| {
                    true
                })
                .unwrap(),
            ),
            (
                "fluid_ticks".into(),
                native_persistence::generated_ticks::encode_level_fluid_ticks(pos, fluids).unwrap(),
            ),
        ]),
    };
    SavedChunkActivation::Encoded(native_persistence::nbt::write(&document).unwrap().into())
}

#[test]
fn streamed_pending_work_blocks_save_even_after_clean_chunk_is_evicted() {
    let mut session = super::tests::session();
    let home = session.player_chunk().unwrap();
    let neighbor = ChunkPos {
        x: home.x + 1,
        z: home.z,
    };
    let world = Arc::make_mut(&mut session.world);
    world.remember_saved_chunk_source_with_activation(home, [1; 32], SavedChunkActivation::Empty);
    world
        .saved_block_entities
        .insert(home, SavedBlockEntities::Empty);
    session.require_saved_chunk_sources().unwrap();
    session.queue_chunk_window(home, 1);
    session.update(TICK);
    assert!(session.validate_saved_chunk_preparation().is_ok());
    assert_eq!(session.checked_chunk_preparation.get(&home), Some(&[1; 32]));

    let pending = Tag::List {
        element_type: TagType::List,
        values: vec![
            Tag::List {
                element_type: TagType::Short,
                values: vec![Tag::Short(0)],
            },
            Tag::List {
                element_type: TagType::End,
                values: vec![],
            },
        ],
    };
    session.queue_streamed_chunk(
        neighbor,
        Some(streamed_chunk(neighbor, pending)),
        Some(SavedEntityData::Empty),
        Some(SavedPoiData::Missing),
        None,
        None,
    );
    session.update(TICK);
    assert!(session.world.chunks.contains_key(&neighbor));
    let error = session
        .validate_saved_chunk_preparation()
        .unwrap_err()
        .to_string();
    assert!(error.contains("postprocessing"), "{error}");
    assert!(
        session
            .draft_block_ticks(home)
            .unwrap_err()
            .to_string()
            .contains("postprocessing")
    );
    assert!(session.validate_generation_block_readiness().is_err());

    let later = ChunkPos {
        x: home.x,
        z: home.z + 1,
    };
    let empty = Tag::List {
        element_type: TagType::List,
        values: vec![
            Tag::List {
                element_type: TagType::End,
                values: vec![],
            },
            Tag::List {
                element_type: TagType::End,
                values: vec![],
            },
        ],
    };
    session.queue_streamed_chunk(
        later,
        Some(streamed_chunk(later, empty)),
        Some(SavedEntityData::Empty),
        Some(SavedPoiData::Missing),
        None,
        None,
    );
    session.update(TICK);
    assert!(session.saved_chunk_preparation_verified(later));
    assert!(session.validate_saved_chunk_preparation().is_err());

    session.queue_chunk_window(home, 0);
    session.update(TICK);
    assert!(!session.world.chunks.contains_key(&neighbor));
    assert!(session.world.saved_chunk_source(neighbor).is_none());
    assert!(session.validate_saved_chunk_preparation().is_err());
}

#[test]
fn streamed_unsupported_or_dropped_ticks_block_the_session_save() {
    for (field, malformed) in [("block_ticks", false), ("fluid_ticks", true)] {
        let mut session = super::tests::session();
        let home = session.player_chunk().unwrap();
        let neighbor = ChunkPos {
            x: home.x + 1,
            z: home.z,
        };
        let world = Arc::make_mut(&mut session.world);
        world.remember_saved_chunk_source_with_activation(
            home,
            [1; 32],
            SavedChunkActivation::Empty,
        );
        world
            .saved_block_entities
            .insert(home, SavedBlockEntities::Empty);
        session.require_saved_chunk_sources().unwrap();
        session.queue_chunk_window(home, 1);
        session.update(TICK);
        assert!(session.saved_chunk_preparation_verified(home));

        let mut fields = vec![
            (
                "i".into(),
                Tag::String(
                    if malformed {
                        "minecraft:water"
                    } else {
                        "minecraft:observer"
                    }
                    .into(),
                ),
            ),
            ("x".into(), Tag::Int(neighbor.x * 16)),
            ("y".into(), Tag::Int(1)),
            ("z".into(), Tag::Int(neighbor.z * 16)),
            ("p".into(), Tag::Int(0)),
        ];
        if !malformed {
            fields.push(("t".into(), Tag::Int(0)));
        }
        let raw_ticks = Tag::List {
            element_type: TagType::Compound,
            values: vec![Tag::Compound(fields)],
        };
        let empty_post = Tag::List {
            element_type: TagType::End,
            values: vec![],
        };
        let incoming =
            streamed_chunk_with_ticks(neighbor, empty_post, vec![(field.into(), raw_ticks)]);
        if malformed {
            assert!(!incoming.saved_fluid_ticks.contains_key(&neighbor));
        } else {
            assert_eq!(incoming.saved_block_ticks[&neighbor].len(), 1);
        }
        session.queue_streamed_chunk(
            neighbor,
            Some(incoming),
            Some(SavedEntityData::Empty),
            Some(SavedPoiData::Missing),
            None,
            None,
        );
        session.update(TICK);
        assert!(!session.saved_chunk_preparation_verified(neighbor));
        assert!(session.saved_chunk_preparation_verified(home));
        let error = session
            .validate_saved_chunk_preparation()
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(if malformed {
                "fluid ticks"
            } else {
                "block ticks"
            }),
            "{error}"
        );
        assert!(session.draft_block_ticks(home).is_err());

        session.queue_chunk_window(home, 0);
        session.update(TICK);
        assert!(!session.world.chunks.contains_key(&neighbor));
        assert!(session.validate_saved_chunk_preparation().is_err());
    }
}

#[test]
fn uncaptured_preparation_is_rejected_only_after_a_tick() {
    let mut session = super::tests::session();
    let home = session.player_chunk().unwrap();
    Arc::make_mut(&mut session.world).remember_saved_chunk_source(home, [2; 32]);
    session.require_saved_chunk_sources().unwrap();
    assert!(session.validate_saved_chunk_preparation().is_ok());
    session.update(TICK);
    let error = session
        .validate_saved_chunk_preparation()
        .unwrap_err()
        .to_string();
    assert!(error.contains("was not captured"), "{error}");
}

#[test]
fn intermediate_terrain_can_be_captured_with_pending_saved_preparation() {
    let mut session = super::tests::session();
    let home = session.player_chunk().unwrap();
    let pending = ChunkPos {
        x: home.x + 1,
        z: home.z,
    };
    let world = Arc::make_mut(&mut session.world);
    world.remember_saved_chunk_source_with_activation(home, [1; 32], SavedChunkActivation::Empty);
    world
        .saved_block_entities
        .insert(home, SavedBlockEntities::Empty);
    session.require_saved_chunk_sources().unwrap();
    session.queue_chunk_window(home, 1);
    session.queue_streamed_chunk(
        pending,
        Some(streamed_chunk(
            pending,
            Tag::List {
                element_type: TagType::List,
                values: vec![
                    Tag::List {
                        element_type: TagType::Short,
                        values: vec![Tag::Short(0)],
                    },
                    Tag::List {
                        element_type: TagType::End,
                        values: vec![],
                    },
                ],
            },
        )),
        Some(SavedEntityData::Empty),
        Some(SavedPoiData::Missing),
        None,
        None,
    );
    session.update(TICK);
    assert!(!session.saved_chunk_preparation_verified(pending));
    assert!(session.validate_saved_chunk_preparation().is_err());
    let source = Arc::clone(&session.world);
    let stage = session
        .capture_generation_terrain_stage(Arc::clone(&source), [home, pending])
        .expect("unchanged block and biome inputs should remain available");
    session.verify_generation_terrain_stage(&stage).unwrap();
    assert!(
        session
            .capture_generation_terrain(&source, [home, pending])
            .is_err()
    );
    assert!(session.validate_saved_chunk_preparation().is_err());

    session.unresolved_other_random_tick = true;
    assert!(session.verify_generation_terrain_stage(&stage).is_err());
    assert!(
        session
            .capture_generation_terrain_stage(Arc::clone(&source), [home, pending])
            .is_err()
    );
    session.unresolved_other_random_tick = false;
    session.unresolved_brushable_lifecycle = true;
    assert!(session.verify_generation_terrain_stage(&stage).is_err());
    session.unresolved_brushable_lifecycle = false;

    Arc::make_mut(&mut session.world).remember_saved_chunk_source(pending, [9; 32]);
    assert!(session.verify_generation_terrain_stage(&stage).is_err());
}

#[test]
fn pending_saved_preparation_defers_ticks_without_stopping_other_chunks() {
    for indexed in [false, true] {
        let template = super::tests::session();
        let home = template.player_chunk().unwrap();
        // Sort the unresolved chunk before the valid chunk so inspection must
        // continue after reporting the first failure.
        let pending = ChunkPos {
            x: home.x - 1,
            z: home.z,
        };
        let mut world = template.world.as_ref().clone();
        let ore = world.intern_state(state("minecraft:redstone_ore", &[("lit", "true")]));
        let sand = world.intern_state(state("minecraft:sand", &[]));
        for position in [pending, home] {
            let mut chunk = world.chunks[&home].as_ref().clone();
            chunk.position = position;
            let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
            section.blocks = Some(Box::new([ore; BLOCK_COUNT]));
            world.chunks.insert(position, Arc::new(chunk));
            let target = [position.x * 16 + 3, 2, position.z * 16 + 4];
            world
                .set_block_id(target[0], target[1], target[2], sand)
                .unwrap();
            world.saved_block_ticks.insert(
                position,
                vec![SavedBlockTick {
                    block: "minecraft:sand".into(),
                    position: target,
                    delay: 0,
                    priority: 0,
                }],
            );
            world.saved_fluid_ticks.insert(
                position,
                vec![SavedFluidTick {
                    fluid: "minecraft:water".into(),
                    position: target,
                    delay: 0,
                    priority: 0,
                }],
            );
            world
                .saved_block_entities
                .insert(position, SavedBlockEntities::Empty);
        }
        let mut marks = native_core::postprocessing::PostProcessing::new(0, 32).unwrap();
        assert!(marks.mark(pending.x * 16, 1, pending.z * 16));
        let document = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "PostProcessing".into(),
                native_persistence::postprocessing::encode(&marks),
            )]),
        };
        let activation = SavedChunkActivation::Encoded(
            native_persistence::nbt::write(&document).unwrap().into(),
        );
        world.remember_saved_chunk_source_with_activation(pending, [1; 32], activation);
        let home_ticks = captured_ticks(&world, home);
        let pending_ticks = captured_ticks(&world, pending);
        world.remember_saved_chunk_source_with_activation(home, [2; 32], home_ticks);
        let mut session =
            LoadedSession::new(world, template.origin, template.bounds, template.player).unwrap();
        session.set_initial_game_time(42).unwrap();
        if indexed {
            assert_eq!(
                session.install_saved_tick_orders(&[pending, home]).unwrap(),
                (true, true)
            );
        }
        session.require_saved_chunk_sources().unwrap();
        assert!(!session.simulation_allows_block_ticks(pending));
        assert!(!session.simulation_allows_block_ticks(home));
        session.inspect_saved_chunk_preparation();
        assert!(!session.simulation_allows_block_ticks(pending));
        assert!(session.simulation_allows_block_ticks(home));
        assert!(session.validate_saved_chunk_preparation().is_err());

        let fluids = session.collect_due_fluid_ticks();
        assert_eq!(fluids.len(), 1);
        assert_eq!(fluids[0].1.key.x.div_euclid(16), home.x);
        session.advance_block_ticks();
        assert_eq!(session.sand_ticks.len(), 1);
        assert_eq!(
            session
                .sand_ticks
                .first_key_value()
                .unwrap()
                .1
                .x
                .div_euclid(16),
            pending.x
        );
        session.set_random_tick_speed(1);
        session.refresh_random_tick_sections();
        assert_eq!(session.active_random_tick_sections.len(), 1);
        assert_eq!(session.active_random_tick_sections[0].0, home);
        let mut expected_random = native_core::random::BlockRandomPosition::new(123);
        expected_random.next_section_offset();
        session.random_block_position = native_core::random::BlockRandomPosition::new(123);
        session.advance_random_block_ticks();
        assert_eq!(
            session.random_block_position.next_section_offset(),
            expected_random.next_section_offset()
        );

        // Simulate replacing the source with a separately verified clean one.
        // The earlier unresolved effects still prevent saving this session.
        Arc::make_mut(&mut session.world).remember_saved_chunk_source_with_activation(
            pending,
            [3; 32],
            pending_ticks,
        );
        session.inspect_saved_chunk_preparation();
        assert!(session.simulation_allows_block_ticks(pending));
        assert!(session.validate_saved_chunk_preparation().is_err());
        session.refresh_random_tick_sections();
        assert_eq!(session.active_random_tick_sections.len(), 2);
        assert_eq!(session.collect_due_fluid_ticks().len(), 1);
        session.advance_block_ticks();
        assert!(session.sand_ticks.is_empty());
    }
}

#[test]
fn saved_ticks_keep_their_delay_while_postprocessing_is_pending() {
    for indexed in [false, true] {
        let template = super::tests::session();
        let home = template.player_chunk().unwrap();
        let pending = ChunkPos {
            x: home.x + 1,
            z: home.z,
        };
        let mut world = template.world.as_ref().clone();
        let mut chunk = world.chunks[&home].as_ref().clone();
        chunk.position = pending;
        world.chunks.insert(pending, Arc::new(chunk));
        let sand = world.intern_state(state("minecraft:sand", &[]));
        let saved = [pending.x * 16 + 3, 2, pending.z * 16 + 4];
        let new = [pending.x * 16 + 4, 2, pending.z * 16 + 4];
        world
            .set_block_id(saved[0], saved[1], saved[2], sand)
            .unwrap();
        world.set_block_id(new[0], new[1], new[2], sand).unwrap();
        world.saved_block_ticks.insert(
            pending,
            [4, 6]
                .into_iter()
                .map(|delay| SavedBlockTick {
                    block: "minecraft:sand".into(),
                    position: saved,
                    delay,
                    priority: 0,
                })
                .collect(),
        );
        world.saved_fluid_ticks.insert(
            pending,
            vec![SavedFluidTick {
                fluid: "minecraft:water".into(),
                position: saved,
                delay: 4,
                priority: 0,
            }],
        );
        for pos in [home, pending] {
            world
                .saved_block_entities
                .insert(pos, SavedBlockEntities::Empty);
        }
        world.remember_saved_chunk_source_with_activation(
            home,
            [1; 32],
            captured_ticks(&world, home),
        );
        let clean = captured_ticks(&world, pending);
        let mut marks = native_core::postprocessing::PostProcessing::new(0, 32).unwrap();
        assert!(marks.mark(saved[0], saved[1], saved[2]));
        let document = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                (
                    "PostProcessing".into(),
                    native_persistence::postprocessing::encode(&marks),
                ),
                (
                    "block_ticks".into(),
                    native_persistence::generated_ticks::encode_level_block_ticks(
                        pending,
                        &world.saved_block_ticks[&pending],
                        |_| true,
                    )
                    .unwrap(),
                ),
                (
                    "fluid_ticks".into(),
                    native_persistence::generated_ticks::encode_level_fluid_ticks(
                        pending,
                        &world.saved_fluid_ticks[&pending],
                    )
                    .unwrap(),
                ),
            ]),
        };
        world.remember_saved_chunk_source_with_activation(
            pending,
            [2; 32],
            SavedChunkActivation::Encoded(
                native_persistence::nbt::write(&document).unwrap().into(),
            ),
        );
        let mut session =
            LoadedSession::new(world, template.origin, template.bounds, template.player).unwrap();
        session.require_saved_chunk_sources().unwrap();
        session.set_initial_game_time(42).unwrap();
        if indexed {
            assert_eq!(
                session.install_saved_tick_orders(&[home, pending]).unwrap(),
                (true, true)
            );
        }
        session.inspect_saved_chunk_preparation();
        assert!(!session.saved_chunk_preparation_verified(pending));
        session.tick = 10;
        session.mark_advanced_saved_tick_chunks_dirty();
        assert!(session.tick_chunks_needing_dirty.contains(&pending));
        assert!(!session.tick_dirty_chunks.contains(&pending));
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
        assert_eq!(session.sand_ticks.len(), 3);
        assert_eq!(session.fluid_ticks.len(), 2);
        assert!(session.collect_due_fluid_ticks().is_empty());
        session.tick = 20;
        let old_blocks: Vec<_> = session
            .sand_ticks
            .iter()
            .map(|(order, key)| (*order, *key))
            .collect();
        let old_fluids: Vec<_> = session
            .fluid_ticks
            .iter()
            .map(|(order, tick)| (*order, tick.key))
            .collect();
        let changed = {
            let world = Arc::make_mut(&mut session.world);
            world.saved_block_ticks.get_mut(&pending).unwrap()[0].delay += 1;
            captured_ticks(world, pending)
        };
        Arc::make_mut(&mut session.world)
            .remember_saved_chunk_source_with_activation(pending, [4; 32], changed);
        session.inspect_saved_chunk_preparation();
        assert!(!session.saved_chunk_preparation_verified(pending));
        assert_eq!(
            old_blocks,
            session
                .sand_ticks
                .iter()
                .map(|(order, key)| (*order, *key))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            old_fluids,
            session
                .fluid_ticks
                .iter()
                .map(|(order, tick)| (*order, tick.key))
                .collect::<Vec<_>>()
        );
        Arc::make_mut(&mut session.world)
            .saved_block_ticks
            .get_mut(&pending)
            .unwrap()[0]
            .delay -= 1;
        let saved_fluid_order = session
            .fluid_ticks
            .iter()
            .find_map(|(order, tick)| (tick.key.x == saved[0]).then_some(*order))
            .unwrap();
        let saved_fluid = session.fluid_ticks.remove(&saved_fluid_order).unwrap();
        Arc::make_mut(&mut session.world).remember_saved_chunk_source_with_activation(
            pending,
            [5; 32],
            clean.clone(),
        );
        session.inspect_saved_chunk_preparation();
        assert!(!session.saved_chunk_preparation_verified(pending));
        assert_eq!(
            old_blocks,
            session
                .sand_ticks
                .iter()
                .map(|(order, key)| (*order, *key))
                .collect::<Vec<_>>()
        );
        if indexed {
            assert!(
                session
                    .generated_block_order
                    .as_ref()
                    .unwrap()
                    .queue
                    .container(pending)
                    .unwrap()
                    .pending_saved()
                    .is_some()
            );
        }
        session.fluid_ticks.insert(saved_fluid_order, saved_fluid);
        // This substitutes a separately confirmed clean source only to test
        // tick timing. It is not proof of actual holder readiness or safe save.
        Arc::make_mut(&mut session.world)
            .remember_saved_chunk_source_with_activation(pending, [3; 32], clean);
        session.inspect_saved_chunk_preparation();
        assert!(session.saved_chunk_preparation_verified(pending));
        assert!(session.validate_saved_chunk_preparation().is_err());
        let block_times: Vec<_> = session
            .sand_ticks
            .iter()
            .map(|(order, key)| (order.0, key.x))
            .collect();
        assert_eq!(
            block_times,
            [(12, new[0]), (24, saved[0]), (26, saved[0])],
            "indexed={indexed}"
        );
        let fluid_times: Vec<_> = session
            .fluid_ticks
            .iter()
            .map(|(order, tick)| (order.0, tick.key.x))
            .collect();
        assert_eq!(
            fluid_times,
            [(13, new[0]), (24, saved[0])],
            "indexed={indexed}"
        );
        let trace: Vec<_> = block_times
            .iter()
            .map(|(due, x)| format!("B {due} {}", x.rem_euclid(16)))
            .chain(
                fluid_times
                    .iter()
                    .map(|(due, x)| format!("F {due} {}", x.rem_euclid(16))),
            )
            .collect();
        assert_eq!(trace, expected_pending_tick_trace());
        if indexed {
            let first = session.collect_generated_block_ticks().unwrap();
            assert_eq!(first.len(), 1);
            assert_eq!(first[0].1.identity().1, new);
        }
        let first = session.collect_due_fluid_ticks();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].1.key.x, new[0]);
        session.tick = 23;
        assert!(session.collect_due_fluid_ticks().is_empty());
        session.tick = 24;
        let next = session.collect_due_fluid_ticks();
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].1.key.x, saved[0]);
        if indexed {
            let next = session.collect_generated_block_ticks().unwrap();
            assert_eq!(next.len(), 1);
            assert_eq!(next[0].1.identity().1, saved);
            session.tick = 26;
            let next = session.collect_generated_block_ticks().unwrap();
            assert_eq!(next.len(), 1);
            assert_eq!(next[0].1.identity().1, saved);
        }
    }
}

fn expected_pending_tick_trace() -> Vec<String> {
    ["B 12 4", "B 24 3", "B 26 3", "F 13 4", "F 24 3"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}

#[test]
fn pending_activation_retimes_each_supported_block_tick_queue() {
    let kinds = [
        "minecraft:air",
        "minecraft:sand",
        "minecraft:oak_button",
        "minecraft:farmland",
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
        let mut world = template.world.as_ref().clone();
        let ticks: Vec<_> = kinds
            .iter()
            .enumerate()
            .map(|(index, kind)| SavedBlockTick {
                block: (*kind).into(),
                position: [home.x * 16 + index as i32 + 1, 2, home.z * 16 + 1],
                delay: 3,
                priority: 0,
            })
            .collect();
        world.saved_block_ticks.insert(home, ticks.clone());
        let mut session =
            LoadedSession::new(world, template.origin, template.bounds, template.player).unwrap();
        session.pending_saved_tick_activation.insert(
            home,
            PendingSavedTickActivation {
                registered_at: 0,
                blocks: ticks,
                fluids: vec![],
            },
        );
        session.set_initial_game_time(42).unwrap();
        if indexed {
            assert_eq!(
                session.install_saved_tick_orders(&[home]).unwrap(),
                (true, true)
            );
        }
        session.tick = 10;
        session.activate_pending_saved_ticks(home).unwrap();
        assert!(!session.pending_saved_tick_activation.contains_key(&home));
        let times = [
            session.air_ticks.first_key_value().unwrap().0.0,
            session.sand_ticks.first_key_value().unwrap().0.0,
            session.button_ticks.first_key_value().unwrap().0.0,
            session.soil_ticks.first_key_value().unwrap().0.0,
            session.lamp_ticks.first_key_value().unwrap().0.0,
            session.ice_ticks.first_key_value().unwrap().0.0,
            session.sensor_ticks.first_key_value().unwrap().0.0,
            session.bubble_ticks.first_key_value().unwrap().0.0,
            session.leaf_ticks.first_key_value().unwrap().0.0,
            session.speleothem_ticks.first_key_value().unwrap().0.0,
        ];
        assert_eq!(times, [13; 10], "indexed={indexed}");
        if indexed {
            let container = session
                .generated_block_order
                .as_ref()
                .unwrap()
                .queue
                .container(home)
                .unwrap();
            assert!(container.pending_saved().is_none());
            assert_eq!(container.peek().unwrap().trigger, 55);
        }
    }
}

#[test]
fn source_tracking_preserves_already_activated_generated_ticks() {
    use crate::worldgen::chunk_ticks::{
        ChunkTicks, ConvertedChunkTicks, RegionTicks, SavedTick, TickRegistry,
    };
    let mut session = super::tests::session();
    let home = session.player_chunk().unwrap();
    let world = Arc::make_mut(&mut session.world);
    world.remember_saved_chunk_source_with_activation(home, [7; 32], SavedChunkActivation::Empty);
    world
        .saved_block_entities
        .insert(home, SavedBlockEntities::Empty);
    let region = RegionTicks::new(
        BTreeMap::from([(
            home,
            ConvertedChunkTicks {
                blocks: ChunkTicks::from_saved(
                    home,
                    vec![SavedTick {
                        kind: "minecraft:sand".into(),
                        position: [home.x * 16 + 1, 2, home.z * 16 + 1],
                        delay: 3,
                        priority: 0,
                    }],
                )
                .unwrap(),
                fluids: ChunkTicks::from_saved(home, vec![]).unwrap(),
            },
        )]),
        42,
        7,
    )
    .unwrap();
    let snapshot = region.activated_snapshot();
    session.install_generated_ticks(&snapshot).unwrap();
    session
        .install_generated_block_order(
            &snapshot,
            region
                .captured_indexed_queue(TickRegistry::Block, &[home])
                .unwrap(),
        )
        .unwrap();
    session
        .install_generated_fluid_order(
            &snapshot,
            region
                .captured_indexed_queue(TickRegistry::Fluid, &[home])
                .unwrap(),
        )
        .unwrap();
    session.require_saved_chunk_sources().unwrap();
    assert!(session.pending_saved_tick_activation.is_empty());
    assert!(session.generated_block_order.is_some());
    assert!(session.generated_fluid_order.is_some());
    session.inspect_saved_chunk_preparation();
    assert!(session.saved_chunk_preparation_verified(home));
    assert_eq!(session.sand_ticks.first_key_value().unwrap().0.0, 3);
    assert_eq!(session.next_sub_tick, 7);
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn saved_pending_tick_timing_matches_java_26_3() {
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
    let version = game.join("versions/26.3");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let temp = tempfile::tempdir().unwrap();
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for library in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let classpath = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("SavedPendingTickOracle.java");
    std::fs::write(
        &source,
        include_str!("../tests/java/SavedPendingTickOracle.java"),
    )
    .unwrap();
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(source)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(classpath)
        .arg("SavedPendingTickOracle")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let actual = String::from_utf8(output.stdout).unwrap();
    let actual: Vec<_> = actual
        .lines()
        .filter(|line| line.starts_with("B ") || line.starts_with("F "))
        .map(str::to_owned)
        .collect();
    assert_eq!(actual, expected_pending_tick_trace());
}
