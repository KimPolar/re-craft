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
