use super::{
    tests::{fixture_chunk_document, session, test_heightmap_tags},
    *,
};
use native_persistence::{edit::BlockChange, nbt::NamedTag, palette::BlockState as SavedState};

fn state(name: &str, properties: &[(&str, &str)]) -> BlockState {
    BlockState {
        name: name.into(),
        properties: properties
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect(),
    }
}

fn saved(state: &BlockState) -> SavedState {
    SavedState {
        name: state.name.as_str().into(),
        properties: state
            .properties
            .iter()
            .map(|(key, value)| (key.as_str().into(), value.as_str().into()))
            .collect(),
    }
}

fn fixture(
    center: BlockState,
    kind: BubbleTickKind,
    with_tags: bool,
) -> (LoadedSession, ChunkPos, Vec<SavedBlockTick>, NamedTag) {
    let mut session = session();
    let home = session.player_chunk().unwrap();
    let relative = IVec3::new(8, 5, 8);
    let absolute = session.origin + relative;
    let support = state("minecraft:magma_block", &[]);
    let mut changes = Vec::new();
    for (position, next) in [(absolute - IVec3::Y, support), (absolute, center)] {
        let old = session
            .world
            .block(position.x, position.y, position.z)
            .unwrap()
            .clone();
        let id = session.intern_block(next.clone());
        Arc::make_mut(&mut session.world)
            .set_block_id(position.x, position.y, position.z, id)
            .unwrap();
        changes.push(BlockChange {
            x: position.x,
            y: position.y,
            z: position.z,
            expected: saved(&old),
            replacement: saved(&next),
        });
    }
    if with_tags {
        session.set_bubble_column_tags(Arc::new(
            native_resources::fluid_tags::BubbleColumnTags::new(
                BTreeSet::from(["minecraft:magma_block".into()]),
                BTreeSet::new(),
            ),
        ));
    } else {
        session.bubble_column_tags = None;
    }
    let ticks = vec![
        SavedBlockTick {
            block: kind.id().into(),
            position: absolute.to_array(),
            delay: 0,
            priority: -1,
        },
        SavedBlockTick {
            block: "minecraft:sand".into(),
            position: absolute.to_array(),
            delay: 20,
            priority: 0,
        },
        SavedBlockTick {
            block: kind.id().into(),
            position: absolute.to_array(),
            delay: 10,
            priority: 1,
        },
    ];
    let mut source = native_persistence::edit::draft_chunk_blocks(
        &fixture_chunk_document(home),
        native_persistence::world::ChunkPos {
            x: home.x,
            z: home.z,
        },
        &changes,
    )
    .unwrap();
    source.replace_block_ticks(&ticks).unwrap();
    let source = source.document().clone();
    Arc::make_mut(&mut session.world)
        .saved_block_ticks
        .insert(home, ticks.clone());
    session.enqueue_saved_sand(&ticks);
    session.enqueue_saved_bubble(&ticks);
    (session, home, ticks, source)
}

#[test]
fn unchanged_bubble_callbacks_preserve_other_ticks_and_allow_a_chunk_draft() {
    for (center, kind) in [
        (
            state("minecraft:bubble_column", &[("drag", "true")]),
            BubbleTickKind::Column,
        ),
        (
            state("minecraft:water", &[("level", "1")]),
            BubbleTickKind::Water,
        ),
    ] {
        let (mut session, home, ticks, source) = fixture(center, kind, true);
        let before_world = Arc::clone(&session.world);
        let before_fluid = session.draft_fluid_ticks(home).unwrap();
        let before_random = session.level_random.state();
        session.advance_block_ticks();
        assert!(!session.unresolved_bubble_tick);
        assert!(Arc::ptr_eq(&before_world, &session.world));
        assert_eq!(session.level_random.state(), before_random);
        assert_eq!(session.draft_fluid_ticks(home).unwrap(), before_fluid);
        assert_eq!(session.draft_block_ticks(home).unwrap(), ticks[1..]);
        let draft = session
            .draft_dirty_chunk_with_heightmaps_and_simulated_ticks(
                home,
                &source,
                &test_heightmap_tags(),
                |_| anyhow::bail!("tick-only draft must not rebuild heightmaps"),
            )
            .unwrap();
        let mut expected = native_persistence::edit::draft_chunk_blocks(
            &source,
            native_persistence::world::ChunkPos {
                x: home.x,
                z: home.z,
            },
            &[],
        )
        .unwrap();
        expected.replace_block_ticks(&ticks[1..]).unwrap();
        assert_eq!(
            draft.document().value.get("block_ticks"),
            expected.document().value.get("block_ticks")
        );
        assert!(draft.matches_source(&source).unwrap());
    }
}

fn effect_fixture() -> (LoadedSession, ChunkPos, NamedTag) {
    let water = state("minecraft:water", &[("level", "0")]);
    let (mut session, home, _, source) = fixture(water.clone(), BubbleTickKind::Water, true);
    let center = session.origin + IVec3::new(8, 5, 8);
    let side = center + IVec3::X;
    let additions = [
        (center + IVec3::Y, water),
        (side, state("minecraft:bubble_column", &[("drag", "false")])),
    ];
    let mut changes = Vec::new();
    for (position, next) in additions {
        let old = session
            .world
            .block(position.x, position.y, position.z)
            .unwrap()
            .clone();
        let id = session.intern_block(next.clone());
        Arc::make_mut(&mut session.world)
            .set_block_id(position.x, position.y, position.z, id)
            .unwrap();
        changes.push(BlockChange {
            x: position.x,
            y: position.y,
            z: position.z,
            expected: saved(&old),
            replacement: saved(&next),
        });
    }
    let fluids = vec![
        SavedFluidTick {
            fluid: "minecraft:water".into(),
            position: side.to_array(),
            delay: 13,
            priority: -2,
        },
        SavedFluidTick {
            fluid: "minecraft:flowing_water".into(),
            position: (center - IVec3::X).to_array(),
            delay: 15,
            priority: 1,
        },
    ];
    let mut initial = native_persistence::edit::draft_chunk_blocks(
        &source,
        native_persistence::world::ChunkPos {
            x: home.x,
            z: home.z,
        },
        &changes,
    )
    .unwrap();
    initial.replace_fluid_ticks(&fluids).unwrap();
    Arc::make_mut(&mut session.world)
        .saved_fluid_ticks
        .insert(home, fluids.clone());
    session.enqueue_saved_fluids(home, &fluids);
    (session, home, initial.document().clone())
}

fn bubble_heightmap_tags() -> HeightmapTags {
    struct Provider;
    impl native_resources::packs::ResourceProvider for Provider {
        fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data_stack(&mut self, _: &str) -> anyhow::Result<Vec<Vec<u8>>> {
            Ok(vec![
                br#"{"values":["minecraft:stone","minecraft:magma_block"]}"#.to_vec(),
            ])
        }
    }
    let registry = ["air", "stone", "magma_block", "water", "bubble_column"]
        .into_iter()
        .map(|name| format!("minecraft:{name}"))
        .collect();
    HeightmapTags::load(&mut Provider, &registry).unwrap()
}

fn bubble_air_and_fluid(state: &SavedState) -> anyhow::Result<(bool, bool)> {
    Ok((
        state.name.matches("minecraft:air"),
        state.name.matches("minecraft:water") || state.name.matches("minecraft:bubble_column"),
    ))
}

#[test]
fn changed_bubble_callbacks_save_blocks_and_scheduled_ticks_together() {
    let (mut session, home, source) = effect_fixture();
    let center = session.origin + IVec3::new(8, 5, 8);
    let side = center + IVec3::X;
    let before_random = session.level_random.state();
    session.advance_block_ticks();
    assert!(!session.unresolved_bubble_tick);
    assert_eq!(session.level_random.state(), before_random);
    let column = state("minecraft:bubble_column", &[("drag", "true")]);
    for position in [center, center + IVec3::Y] {
        assert_eq!(
            session.world.block(position.x, position.y, position.z),
            Some(&column)
        );
    }
    let blocks = session.draft_block_ticks(home).unwrap();
    assert_eq!(
        blocks,
        vec![
            SavedBlockTick {
                block: "minecraft:sand".into(),
                position: center.to_array(),
                delay: 20,
                priority: 0,
            },
            SavedBlockTick {
                block: "minecraft:water".into(),
                position: center.to_array(),
                delay: 10,
                priority: 1,
            },
            SavedBlockTick {
                block: "minecraft:bubble_column".into(),
                position: side.to_array(),
                delay: 5,
                priority: 0,
            },
        ]
    );
    let fluids = session.draft_fluid_ticks(home).unwrap();
    assert_eq!(
        fluids,
        vec![
            SavedFluidTick {
                fluid: "minecraft:water".into(),
                position: side.to_array(),
                delay: 13,
                priority: -2,
            },
            SavedFluidTick {
                fluid: "minecraft:flowing_water".into(),
                position: (center - IVec3::X).to_array(),
                delay: 15,
                priority: 1,
            },
            SavedFluidTick {
                fluid: "minecraft:water".into(),
                position: (center + IVec3::Y).to_array(),
                delay: 5,
                priority: 0,
            },
            SavedFluidTick {
                fluid: "minecraft:water".into(),
                position: center.to_array(),
                delay: 5,
                priority: 0,
            },
        ]
    );
    let draft = session
        .draft_dirty_chunk_with_heightmaps_and_simulated_ticks(
            home,
            &source,
            &bubble_heightmap_tags(),
            bubble_air_and_fluid,
        )
        .unwrap();
    let water = state("minecraft:water", &[("level", "0")]);
    let changes: Vec<_> = [center, center + IVec3::Y]
        .into_iter()
        .map(|position| BlockChange {
            x: position.x,
            y: position.y,
            z: position.z,
            expected: saved(&water),
            replacement: saved(&column),
        })
        .collect();
    let mut expected = native_persistence::edit::draft_chunk_blocks(
        &source,
        native_persistence::world::ChunkPos {
            x: home.x,
            z: home.z,
        },
        &changes,
    )
    .unwrap();
    expected.replace_block_ticks(&blocks).unwrap();
    expected.replace_fluid_ticks(&fluids).unwrap();
    for field in [
        "sections",
        "block_ticks",
        "fluid_ticks",
        "unknown_chunk_data",
    ] {
        assert_eq!(
            draft.document().value.get(field),
            expected.document().value.get(field),
            "{field}"
        );
    }
    assert!(draft.matches_source(&source).unwrap());
}

#[test]
fn bubble_tick_in_a_nonfull_chunk_remains_deferred_without_losing_its_schedule() {
    let (mut session, home, _) = effect_fixture();
    let world = Arc::make_mut(&mut session.world);
    Arc::make_mut(world.chunks.get_mut(&home).unwrap()).status = Some("minecraft:features".into());
    let before = Arc::clone(&session.world);
    let blocks = session.draft_block_ticks(home).unwrap();
    let fluids = session.draft_fluid_ticks(home).unwrap();
    session.advance_block_ticks();
    assert!(!session.unresolved_bubble_tick);
    assert_eq!(session.bubble_deferred.len(), 1);
    assert!(Arc::ptr_eq(&before, &session.world));
    assert_eq!(session.draft_block_ticks(home).unwrap(), blocks);
    assert_eq!(session.draft_fluid_ticks(home).unwrap(), fluids);
}

fn shape_fixture() -> (LoadedSession, ChunkPos, NamedTag) {
    let column = state("minecraft:bubble_column", &[("drag", "true")]);
    let (mut session, home, _, source) = fixture(column, BubbleTickKind::Column, true);
    session.bubble_ticks.clear();
    session.bubble_pending.clear();
    session.sand_ticks.clear();
    session.sand_pending.clear();
    Arc::make_mut(&mut session.world)
        .saved_block_ticks
        .insert(home, Vec::new());
    let mut initial = native_persistence::edit::draft_chunk_blocks(
        &source,
        native_persistence::world::ChunkPos {
            x: home.x,
            z: home.z,
        },
        &[],
    )
    .unwrap();
    initial.replace_block_ticks(&[]).unwrap();
    (session, home, initial.document().clone())
}

#[test]
fn bubble_shape_update_preserves_the_block_and_both_tick_queues() {
    let (mut session, home, source) = shape_fixture();
    let relative = IVec3::new(8, 5, 8);
    let center = session.origin + relative;
    let above = center + IVec3::Y;
    let water = state("minecraft:water", &[("level", "0")]);
    let water_id = session.intern_block(water.clone());
    assert!(session.set(relative + IVec3::Y, water_id));
    assert!(!session.unresolved_bubble_tick);
    let blocks = session.draft_block_ticks(home).unwrap();
    assert_eq!(
        blocks,
        vec![SavedBlockTick {
            block: "minecraft:bubble_column".into(),
            position: center.to_array(),
            delay: 5,
            priority: 0,
        }]
    );
    let fluids = session.draft_fluid_ticks(home).unwrap();
    assert_eq!(
        fluids,
        [above, center]
            .into_iter()
            .map(|position| SavedFluidTick {
                fluid: "minecraft:water".into(),
                position: position.to_array(),
                delay: 5,
                priority: 0,
            })
            .collect::<Vec<_>>()
    );
    let draft = session
        .draft_dirty_chunk_with_heightmaps_and_simulated_ticks(
            home,
            &source,
            &bubble_heightmap_tags(),
            bubble_air_and_fluid,
        )
        .unwrap();
    let mut expected = native_persistence::edit::draft_chunk_blocks(
        &source,
        native_persistence::world::ChunkPos {
            x: home.x,
            z: home.z,
        },
        &[BlockChange {
            x: above.x,
            y: above.y,
            z: above.z,
            expected: saved(&state("minecraft:air", &[])),
            replacement: saved(&water),
        }],
    )
    .unwrap();
    expected.replace_block_ticks(&blocks).unwrap();
    expected.replace_fluid_ticks(&fluids).unwrap();
    for field in [
        "sections",
        "block_ticks",
        "fluid_ticks",
        "unknown_chunk_data",
    ] {
        assert_eq!(
            draft.document().value.get(field),
            expected.document().value.get(field),
            "{field}"
        );
    }
    assert!(draft.matches_source(&source).unwrap());
}

#[test]
fn bubble_shape_update_blocks_saving_when_its_fluid_or_tags_are_missing() {
    for missing_tags in [false, true] {
        let (mut session, home, _) = shape_fixture();
        let relative = IVec3::new(8, 5, 8);
        if missing_tags {
            session.bubble_column_tags = None;
        } else {
            let Cell::State(id) = cell(
                &session.world,
                session.origin,
                session.bounds,
                relative,
                session.states[0],
            ) else {
                panic!("bubble column missing from fixture")
            };
            session.fluids[id.0 as usize] = None;
        }
        let water = session.intern_block(state("minecraft:water", &[("level", "0")]));
        assert!(session.set(relative + IVec3::Y, water));
        assert!(session.unresolved_bubble_tick);
        assert!(session.draft_block_ticks(home).is_err());
    }
}

#[test]
fn bubble_shape_update_across_chunk_boundary_preserves_both_chunks_ticks() {
    let (mut session, home, source) = shape_fixture();
    let neighbor = ChunkPos {
        x: home.x + 1,
        z: home.z,
    };
    let clean = super::tests::session();
    let mut neighbor_chunk = (**clean.world.chunks.get(&home).unwrap()).clone();
    neighbor_chunk.position = neighbor;
    let world = Arc::make_mut(&mut session.world);
    world.chunks.insert(neighbor, Arc::new(neighbor_chunk));
    world.saved_block_ticks.insert(neighbor, Vec::new());
    world.saved_fluid_ticks.insert(neighbor, Vec::new());
    session.fluid_chunks_seen.insert(neighbor);

    let bubble_pos = session.origin + IVec3::new(15, 5, 8);
    let water_pos = bubble_pos + IVec3::X;
    let bubble = state("minecraft:bubble_column", &[("drag", "true")]);
    let bubble_id = session.intern_block(bubble.clone());
    Arc::make_mut(&mut session.world)
        .set_block_id(bubble_pos.x, bubble_pos.y, bubble_pos.z, bubble_id)
        .unwrap();
    let home_source = native_persistence::edit::draft_chunk_blocks(
        &source,
        native_persistence::world::ChunkPos {
            x: home.x,
            z: home.z,
        },
        &[BlockChange {
            x: bubble_pos.x,
            y: bubble_pos.y,
            z: bubble_pos.z,
            expected: saved(&state("minecraft:air", &[])),
            replacement: saved(&bubble),
        }],
    )
    .unwrap()
    .document()
    .clone();
    let neighbor_source = fixture_chunk_document(neighbor);
    let water = state("minecraft:water", &[("level", "0")]);
    let water_id = session.intern_block(water.clone());
    assert!(session.set(IVec3::new(16, 5, 8), water_id));
    assert!(!session.unresolved_bubble_tick);
    assert!(session.dirty_chunk_positions().contains(&home));
    assert!(session.dirty_chunk_positions().contains(&neighbor));

    let blocks = session.draft_block_ticks(home).unwrap();
    assert_eq!(
        blocks,
        vec![SavedBlockTick {
            block: "minecraft:bubble_column".into(),
            position: bubble_pos.to_array(),
            delay: 5,
            priority: 0,
        }]
    );
    let fluid = |position: IVec3| SavedFluidTick {
        fluid: "minecraft:water".into(),
        position: position.to_array(),
        delay: 5,
        priority: 0,
    };
    let home_fluids = session.draft_fluid_ticks(home).unwrap();
    let neighbor_fluids = session.draft_fluid_ticks(neighbor).unwrap();
    assert_eq!(home_fluids, vec![fluid(bubble_pos)]);
    assert_eq!(neighbor_fluids, vec![fluid(water_pos)]);
    assert!(session.draft_block_ticks(neighbor).unwrap().is_empty());

    let tags = bubble_heightmap_tags();
    let home_draft = session
        .draft_dirty_chunk_with_heightmaps_and_simulated_ticks(
            home,
            &home_source,
            &tags,
            bubble_air_and_fluid,
        )
        .unwrap();
    let neighbor_draft = session
        .draft_dirty_chunk_with_heightmaps_and_simulated_ticks(
            neighbor,
            &neighbor_source,
            &tags,
            bubble_air_and_fluid,
        )
        .unwrap();
    let mut expected_home = native_persistence::edit::draft_chunk_blocks(
        &home_source,
        native_persistence::world::ChunkPos {
            x: home.x,
            z: home.z,
        },
        &[],
    )
    .unwrap();
    expected_home.replace_block_ticks(&blocks).unwrap();
    expected_home.replace_fluid_ticks(&home_fluids).unwrap();
    let mut expected_neighbor = native_persistence::edit::draft_chunk_blocks(
        &neighbor_source,
        native_persistence::world::ChunkPos {
            x: neighbor.x,
            z: neighbor.z,
        },
        &[BlockChange {
            x: water_pos.x,
            y: water_pos.y,
            z: water_pos.z,
            expected: saved(&state("minecraft:air", &[])),
            replacement: saved(&water),
        }],
    )
    .unwrap();
    expected_neighbor.replace_block_ticks(&[]).unwrap();
    expected_neighbor
        .replace_fluid_ticks(&neighbor_fluids)
        .unwrap();
    for (actual, expected) in [
        (&home_draft, &expected_home),
        (&neighbor_draft, &expected_neighbor),
    ] {
        for field in [
            "sections",
            "block_ticks",
            "fluid_ticks",
            "unknown_chunk_data",
        ] {
            assert_eq!(
                actual.document().value.get(field),
                expected.document().value.get(field),
                "{field}"
            );
        }
    }
}

fn active_fluid_fixture() -> (LoadedSession, ChunkPos, NamedTag) {
    use native_persistence::nbt::{Tag, TagType};

    let mut session = session();
    session.sky_lighting.set_enabled(false);
    session.set_bubble_column_tags(Arc::new(
        native_resources::fluid_tags::BubbleColumnTags::new(
            BTreeSet::from(["minecraft:stone".into()]),
            BTreeSet::new(),
        ),
    ));
    let home = session.player_chunk().unwrap();
    let center = IVec3::new(8, 10, 8);
    let north = center + IVec3::NEG_Z;
    let stone = session.intern_block(state("minecraft:stone", &[]));
    let bubble = session.intern_block(state("minecraft:bubble_column", &[("drag", "true")]));
    let air = session.states[0];
    let world = Arc::make_mut(&mut session.world);
    let chunk = Arc::make_mut(world.chunks.get_mut(&home).unwrap());
    let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
    let blocks = section.blocks.as_mut().unwrap();
    blocks.fill(stone);
    let index = |pos: IVec3| (pos.x + pos.z * 16 + pos.y * 256) as usize;
    blocks[index(center)] = bubble;
    blocks[index(north)] = air;
    let original = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![
            ("DataVersion".into(), Tag::Int(5023)),
            ("xPos".into(), Tag::Int(home.x)),
            ("zPos".into(), Tag::Int(home.z)),
            ("Status".into(), Tag::String("minecraft:full".into())),
            ("unknown_chunk_data".into(), Tag::Long(42)),
            (
                "sections".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![Tag::Compound(vec![
                        ("Y".into(), Tag::Byte(0)),
                        (
                            "block_states".into(),
                            Tag::Compound(vec![(
                                "palette".into(),
                                Tag::List {
                                    element_type: TagType::String,
                                    values: vec![Tag::String("minecraft:stone".into())],
                                },
                            )]),
                        ),
                    ])],
                },
            ),
        ]),
    };
    let stone_state = state("minecraft:stone", &[]);
    let mut initial = native_persistence::edit::draft_chunk_blocks(
        &original,
        native_persistence::world::ChunkPos {
            x: home.x,
            z: home.z,
        },
        &[
            BlockChange {
                x: session.origin.x + center.x,
                y: session.origin.y + center.y,
                z: session.origin.z + center.z,
                expected: saved(&stone_state),
                replacement: saved(&state("minecraft:bubble_column", &[("drag", "true")])),
            },
            BlockChange {
                x: session.origin.x + north.x,
                y: session.origin.y + north.y,
                z: session.origin.z + north.z,
                expected: saved(&stone_state),
                replacement: saved(&state("minecraft:air", &[])),
            },
        ],
    )
    .unwrap();
    let ticks = vec![SavedFluidTick {
        fluid: "minecraft:water".into(),
        position: (session.origin + center).to_array(),
        delay: 0,
        priority: 0,
    }];
    initial.replace_fluid_ticks(&ticks).unwrap();
    Arc::make_mut(&mut session.world)
        .saved_fluid_ticks
        .insert(home, ticks.clone());
    session.enqueue_saved_fluids(home, &ticks);
    session.level_random = native_core::random::LegacyRandom::new(1);
    (session, home, initial.document().clone())
}

#[test]
fn active_bubble_fluid_tick_saves_and_resumes_its_water_spread() {
    use native_persistence::world::{ChunkArea, DimensionId, LoadedChunk};

    let (mut session, home, source) = active_fluid_fixture();
    let center = session.origin + IVec3::new(8, 10, 8);
    let north = center + IVec3::NEG_Z;
    let random = session.level_random.state();
    session.advance_fluid_ticks();
    assert!(!session.unresolved_bubble_tick);
    assert_eq!(session.level_random.state(), random);
    let flowing = state("minecraft:water", &[("level", "1")]);
    assert_eq!(
        session.world.block(north.x, north.y, north.z),
        Some(&flowing)
    );
    assert!(session.draft_block_ticks(home).unwrap().is_empty());
    let fluids = vec![
        SavedFluidTick {
            fluid: "minecraft:flowing_water".into(),
            position: north.to_array(),
            delay: 5,
            priority: 0,
        },
        SavedFluidTick {
            fluid: "minecraft:water".into(),
            position: center.to_array(),
            delay: 5,
            priority: 0,
        },
    ];
    assert_eq!(session.draft_fluid_ticks(home).unwrap(), fluids);
    let draft = session
        .draft_dirty_chunk_with_heightmaps_and_simulated_ticks(
            home,
            &source,
            &bubble_heightmap_tags(),
            bubble_air_and_fluid,
        )
        .unwrap();
    let mut expected = native_persistence::edit::draft_chunk_blocks(
        &source,
        native_persistence::world::ChunkPos {
            x: home.x,
            z: home.z,
        },
        &[BlockChange {
            x: north.x,
            y: north.y,
            z: north.z,
            expected: saved(&state("minecraft:air", &[])),
            replacement: saved(&flowing),
        }],
    )
    .unwrap();
    expected.replace_block_ticks(&[]).unwrap();
    expected.replace_fluid_ticks(&fluids).unwrap();
    for field in [
        "sections",
        "block_ticks",
        "fluid_ticks",
        "unknown_chunk_data",
    ] {
        assert_eq!(
            draft.document().value.get(field),
            expected.document().value.get(field)
        );
    }
    assert!(draft.matches_source(&source).unwrap());

    let document =
        native_persistence::nbt::read(&native_persistence::nbt::write(draft.document()).unwrap())
            .unwrap();
    let position = native_persistence::world::ChunkPos {
        x: home.x,
        z: home.z,
    };
    let chunk = LoadedChunk {
        position,
        data_version: 5023,
        status: Some("minecraft:full".into()),
        sections: native_persistence::palette::decode_sections(&document.value).unwrap(),
        document: Arc::new(document),
    };
    let area = ChunkArea {
        center: position,
        radius: 0,
        chunks: BTreeMap::from([(position, Arc::new(chunk))]),
        missing: Vec::new(),
    };
    let world = area
        .materialize(&DimensionId::parse("overworld").unwrap())
        .unwrap();
    let mut reopened = LoadedSession::new_with_initial_level_random(
        world,
        session.origin,
        session.bounds,
        session.player,
        session.level_random.clone(),
    )
    .unwrap();
    reopened.sky_lighting.set_enabled(false);
    reopened.set_bubble_column_tags(session.bubble_column_tags.clone().unwrap());
    reopened.set_farmland_water_tag(Arc::new(BTreeSet::from([
        "minecraft:water".into(),
        "minecraft:flowing_water".into(),
    ])));
    reopened.set_lava_fluid_tag(Arc::new(BTreeSet::from([
        "minecraft:lava".into(),
        "minecraft:flowing_lava".into(),
    ])));
    assert_eq!(reopened.draft_fluid_ticks(home).unwrap(), fluids);
    assert_eq!(
        reopened.world.block(north.x, north.y, north.z),
        Some(&flowing)
    );
    session.tick = 5;
    reopened.tick = 5;
    session.advance_fluid_ticks();
    reopened.advance_fluid_ticks();
    assert_eq!(
        session.draft_fluid_ticks(home).unwrap(),
        reopened.draft_fluid_ticks(home).unwrap()
    );
    assert_eq!(
        session.draft_block_ticks(home).unwrap(),
        reopened.draft_block_ticks(home).unwrap()
    );
    for y in 0..16 {
        for z in 0..16 {
            for x in 0..16 {
                let pos = session.origin + IVec3::new(x, y, z);
                assert_eq!(
                    session.world.block(pos.x, pos.y, pos.z),
                    reopened.world.block(pos.x, pos.y, pos.z)
                );
            }
        }
    }
}

#[test]
fn active_bubble_fluid_spread_across_chunks_saves_both_tick_queues() {
    use native_persistence::nbt::Tag;

    let (mut session, home, source) = active_fluid_fixture();
    let neighbor = ChunkPos {
        x: home.x + 1,
        z: home.z,
    };
    let mut adjacent = (**session.world.chunks.get(&home).unwrap()).clone();
    adjacent.position = neighbor;
    let world = Arc::make_mut(&mut session.world);
    world.chunks.insert(neighbor, Arc::new(adjacent));
    world.saved_block_ticks.insert(neighbor, Vec::new());
    world.saved_fluid_ticks.insert(home, Vec::new());
    world.saved_fluid_ticks.insert(neighbor, Vec::new());
    session.fluid_chunks_seen.insert(neighbor);
    session.fluid_ticks.clear();
    session.fluid_pending.clear();

    let relative = IVec3::new(15, 10, 8);
    let center = session.origin + relative;
    let east = center + IVec3::X;
    let bubble = state("minecraft:bubble_column", &[("drag", "true")]);
    let stone = state("minecraft:stone", &[]);
    let air = state("minecraft:air", &[]);
    let bubble_id = session.intern_block(bubble.clone());
    let world = Arc::make_mut(&mut session.world);
    world
        .set_block_id(center.x, center.y, center.z, bubble_id)
        .unwrap();
    world
        .set_block_id(east.x, east.y, east.z, session.states[0])
        .unwrap();
    let mut home_initial = native_persistence::edit::draft_chunk_blocks(
        &source,
        native_persistence::world::ChunkPos {
            x: home.x,
            z: home.z,
        },
        &[BlockChange {
            x: center.x,
            y: center.y,
            z: center.z,
            expected: saved(&stone),
            replacement: saved(&bubble),
        }],
    )
    .unwrap();
    home_initial.replace_fluid_ticks(&[]).unwrap();
    let home_source = home_initial.document().clone();
    let mut neighbor_original = source.clone();
    let Tag::Compound(fields) = &mut neighbor_original.value else {
        panic!("chunk root is not a compound")
    };
    fields
        .iter_mut()
        .find(|(name, _)| name.matches("xPos"))
        .unwrap()
        .1 = Tag::Int(neighbor.x);
    fields.retain(|(name, _)| !name.matches("fluid_ticks"));
    let mut neighbor_initial = native_persistence::edit::draft_chunk_blocks(
        &neighbor_original,
        native_persistence::world::ChunkPos {
            x: neighbor.x,
            z: neighbor.z,
        },
        &[BlockChange {
            x: east.x,
            y: east.y,
            z: east.z,
            expected: saved(&stone),
            replacement: saved(&air),
        }],
    )
    .unwrap();
    neighbor_initial.replace_fluid_ticks(&[]).unwrap();
    let neighbor_source = neighbor_initial.document().clone();

    session.schedule_fluid(relative, session.simulated_fluid(bubble_id).unwrap(), 0);
    session.advance_fluid_ticks();
    assert!(!session.unresolved_bubble_tick);
    let flowing = state("minecraft:water", &[("level", "1")]);
    assert_eq!(session.world.block(east.x, east.y, east.z), Some(&flowing));
    let tick = |fluid: &str, position: IVec3| SavedFluidTick {
        fluid: fluid.into(),
        position: position.to_array(),
        delay: 5,
        priority: 0,
    };
    let home_fluids = vec![tick("minecraft:water", center)];
    let neighbor_fluids = vec![tick("minecraft:flowing_water", east)];
    assert_eq!(session.draft_fluid_ticks(home).unwrap(), home_fluids);
    assert_eq!(
        session.draft_fluid_ticks(neighbor).unwrap(),
        neighbor_fluids
    );
    let tags = bubble_heightmap_tags();
    let home_draft = session
        .draft_dirty_chunk_with_heightmaps_and_simulated_ticks(
            home,
            &home_source,
            &tags,
            bubble_air_and_fluid,
        )
        .unwrap();
    let neighbor_draft = session
        .draft_dirty_chunk_with_heightmaps_and_simulated_ticks(
            neighbor,
            &neighbor_source,
            &tags,
            bubble_air_and_fluid,
        )
        .unwrap();
    let mut expected_home = native_persistence::edit::draft_chunk_blocks(
        &home_source,
        native_persistence::world::ChunkPos {
            x: home.x,
            z: home.z,
        },
        &[],
    )
    .unwrap();
    expected_home.replace_block_ticks(&[]).unwrap();
    expected_home.replace_fluid_ticks(&home_fluids).unwrap();
    let mut expected_neighbor = native_persistence::edit::draft_chunk_blocks(
        &neighbor_source,
        native_persistence::world::ChunkPos {
            x: neighbor.x,
            z: neighbor.z,
        },
        &[BlockChange {
            x: east.x,
            y: east.y,
            z: east.z,
            expected: saved(&air),
            replacement: saved(&flowing),
        }],
    )
    .unwrap();
    expected_neighbor.replace_block_ticks(&[]).unwrap();
    expected_neighbor
        .replace_fluid_ticks(&neighbor_fluids)
        .unwrap();
    for (actual, expected) in [
        (&home_draft, &expected_home),
        (&neighbor_draft, &expected_neighbor),
    ] {
        for field in [
            "sections",
            "block_ticks",
            "fluid_ticks",
            "unknown_chunk_data",
        ] {
            assert_eq!(
                actual.document().value.get(field),
                expected.document().value.get(field),
                "{field}"
            );
        }
    }
}

#[test]
fn active_bubble_fluid_tick_with_unknown_neighbors_still_blocks_saving() {
    for unknown in [
        state("minecraft:oak_planks", &[]),
        state("minecraft:deepslate", &[]),
        state("minecraft:deepslate", &[("axis", "invalid")]),
        state("minecraft:water", &[]),
        state("minecraft:water", &[("custom", "0")]),
        state("minecraft:water", &[("level", "01")]),
        state("minecraft:water", &[("level", "+1")]),
        state("minecraft:water", &[("level", "invalid")]),
        state("minecraft:bubble_column", &[("drag", "invalid")]),
    ] {
        let (mut session, home, _) = active_fluid_fixture();
        let position = session.origin + IVec3::new(9, 10, 8);
        let id = session.intern_block(unknown.clone());
        Arc::make_mut(&mut session.world)
            .set_block_id(position.x, position.y, position.z, id)
            .unwrap();
        session.advance_fluid_ticks();
        assert!(session.unresolved_bubble_tick, "{unknown:?}");
        assert!(session.draft_block_ticks(home).is_err());
    }
}

#[test]
fn active_bubble_fluid_tick_at_a_missing_chunk_still_blocks_saving() {
    let (mut session, home, _) = active_fluid_fixture();
    session.fluid_ticks.clear();
    session.fluid_pending.clear();
    let relative = IVec3::new(15, 10, 8);
    let absolute = session.origin + relative;
    let bubble = session.intern_block(state("minecraft:bubble_column", &[("drag", "true")]));
    Arc::make_mut(&mut session.world)
        .set_block_id(absolute.x, absolute.y, absolute.z, bubble)
        .unwrap();
    let water = session.simulated_fluid(bubble).unwrap();
    session.schedule_fluid(relative, water, 0);
    session.advance_fluid_ticks();
    assert!(session.unresolved_bubble_tick);
    assert!(session.draft_block_ticks(home).is_err());
}

#[test]
fn active_bubble_fluid_tick_without_bubble_tags_still_blocks_saving() {
    let (mut session, home, _) = active_fluid_fixture();
    session.bubble_column_tags = None;
    session.advance_fluid_ticks();
    assert!(session.unresolved_bubble_tick);
    assert!(session.draft_block_ticks(home).is_err());
}

#[test]
fn active_bubble_fluid_tick_requires_compatible_water_and_lava_tags() {
    let standard_water =
        BTreeSet::from(["minecraft:water".into(), "minecraft:flowing_water".into()]);
    let standard_lava = BTreeSet::from(["minecraft:lava".into(), "minecraft:flowing_lava".into()]);
    let cases = [
        (None, Some(standard_lava.clone())),
        (Some(BTreeSet::new()), Some(standard_lava.clone())),
        (
            Some(BTreeSet::from(["minecraft:water".into()])),
            Some(standard_lava.clone()),
        ),
        (
            Some(BTreeSet::from(["minecraft:flowing_water".into()])),
            Some(standard_lava.clone()),
        ),
        (Some(standard_water.clone()), None),
        (
            Some(standard_water.clone()),
            Some(BTreeSet::from(["minecraft:water".into()])),
        ),
        (
            Some(standard_water),
            Some(BTreeSet::from(["minecraft:flowing_water".into()])),
        ),
    ];
    for (water, lava) in cases {
        let (mut session, home, _) = active_fluid_fixture();
        match water {
            Some(tag) => session.set_farmland_water_tag(Arc::new(tag)),
            None => session.farmland_water_tag = None,
        }
        match lava {
            Some(tag) => session.set_lava_fluid_tag(Arc::new(tag)),
            None => session.lava_fluid_tag = None,
        }
        session.advance_fluid_ticks();
        assert!(session.unresolved_bubble_tick);
        assert!(session.draft_block_ticks(home).is_err());
    }
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn active_bubble_fluid_draft_round_trips_in_java() -> anyhow::Result<()> {
    use anyhow::{Context, ensure};
    use std::{path::PathBuf, process::Command};

    let (mut session, home, source) = active_fluid_fixture();
    let center = session.origin + IVec3::new(8, 10, 8);
    session.advance_fluid_ticks();
    let draft = session.draft_dirty_chunk_with_heightmaps_and_simulated_ticks(
        home,
        &source,
        &bubble_heightmap_tags(),
        bubble_air_and_fluid,
    )?;
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let version = game.join("versions/26.3");
    let metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    ensure!(metadata["id"] == "26.3", "wrong Minecraft version");
    let temp = tempfile::tempdir()?;
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for library in metadata["libraries"]
        .as_array()
        .context("libraries missing")?
    {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let classpath = std::env::join_paths(paths)?;
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let oracle = temp.path().join("BubbleFluidSaveOracle.java");
    let nbt = temp.path().join("chunk.nbt");
    std::fs::write(
        &oracle,
        include_str!("../tests/java/BubbleFluidSaveOracle.java"),
    )?;
    std::fs::write(&nbt, native_persistence::nbt::write(draft.document())?)?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(oracle)
        .output()?;
    ensure!(
        compiled.status.success(),
        "Java chunk oracle did not compile: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(classpath)
        .arg("BubbleFluidSaveOracle")
        .arg(nbt)
        .args(center.to_array().map(|coordinate| coordinate.to_string()))
        .output()?;
    ensure!(
        output.status.success(),
        "Java chunk comparison failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[test]
fn stale_fluid_ticks_on_a_bubble_column_are_consumed_and_saved() {
    for drag in ["true", "false"] {
        let (mut session, home, _, source) = fixture(
            state("minecraft:bubble_column", &[("drag", drag)]),
            BubbleTickKind::Column,
            true,
        );
        let center = session.origin + IVec3::new(8, 5, 8);
        let fluids: Vec<_> = [
            ("minecraft:flowing_water", 0, -2),
            ("minecraft:lava", 0, 0),
            ("minecraft:flowing_lava", 0, 1),
            ("minecraft:water", 20, -1),
        ]
        .into_iter()
        .map(|(fluid, delay, priority)| SavedFluidTick {
            fluid: fluid.into(),
            position: center.to_array(),
            delay,
            priority,
        })
        .collect();
        let mut initial = native_persistence::edit::draft_chunk_blocks(
            &source,
            native_persistence::world::ChunkPos {
                x: home.x,
                z: home.z,
            },
            &[],
        )
        .unwrap();
        initial.replace_fluid_ticks(&fluids).unwrap();
        let source = initial.document().clone();
        Arc::make_mut(&mut session.world)
            .saved_fluid_ticks
            .insert(home, fluids.clone());
        session.enqueue_saved_fluids(home, &fluids);
        let before = Arc::clone(&session.world);
        let random = session.level_random.state();
        session.advance_fluid_ticks();
        assert!(Arc::ptr_eq(&before, &session.world));
        assert_eq!(session.level_random.state(), random);
        assert!(!session.unresolved_bubble_tick);
        assert_eq!(session.draft_fluid_ticks(home).unwrap(), fluids[3..]);
        let draft = session
            .draft_dirty_chunk_with_heightmaps_and_simulated_ticks(
                home,
                &source,
                &test_heightmap_tags(),
                |_| anyhow::bail!("tick-only draft must not rebuild heightmaps"),
            )
            .unwrap();
        let mut expected = native_persistence::edit::draft_chunk_blocks(
            &source,
            native_persistence::world::ChunkPos {
                x: home.x,
                z: home.z,
            },
            &[],
        )
        .unwrap();
        expected.replace_fluid_ticks(&fluids[3..]).unwrap();
        assert_eq!(
            draft.document().value.get("fluid_ticks"),
            expected.document().value.get("fluid_ticks")
        );
        assert!(draft.matches_source(&source).unwrap());
    }
}

#[test]
fn stale_bubble_fluid_cannot_be_guessed_when_its_state_is_unavailable() {
    let (mut session, home, _, _) = fixture(
        state("minecraft:bubble_column", &[("drag", "true")]),
        BubbleTickKind::Column,
        true,
    );
    let relative = IVec3::new(8, 5, 8);
    let Cell::State(id) = cell(
        &session.world,
        session.origin,
        session.bounds,
        relative,
        session.states[0],
    ) else {
        panic!("bubble column missing from fixture")
    };
    session.fluids[id.0 as usize] = None;
    session.enqueue_saved_fluids(
        home,
        &[SavedFluidTick {
            fluid: "minecraft:flowing_water".into(),
            position: (session.origin + relative).to_array(),
            delay: 0,
            priority: 0,
        }],
    );
    session.advance_fluid_ticks();
    assert!(session.unresolved_bubble_tick);
    assert!(session.draft_block_ticks(home).is_err());
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn stale_bubble_fluid_ticks_are_skipped_by_java_server() -> anyhow::Result<()> {
    use anyhow::{Context, ensure};
    use std::{path::PathBuf, process::Command};

    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let version = game.join("versions/26.3");
    let metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    ensure!(metadata["id"] == "26.3", "wrong Minecraft version");
    let temp = tempfile::tempdir()?;
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for library in metadata["libraries"]
        .as_array()
        .context("libraries missing")?
    {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let classpath = std::env::join_paths(paths)?;
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let oracle = temp.path().join("BubbleStaleFluidTickOracle.java");
    std::fs::write(
        &oracle,
        include_str!("../tests/java/BubbleStaleFluidTickOracle.java"),
    )?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(oracle)
        .output()?;
    ensure!(
        compiled.status.success(),
        "Java fluid oracle did not compile: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(classpath)
        .arg("BubbleStaleFluidTickOracle")
        .output()?;
    ensure!(
        output.status.success(),
        "Java fluid comparison failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn changed_bubble_callback_saves_both_tick_queues_for_java() -> anyhow::Result<()> {
    use anyhow::{Context, ensure};
    use std::{path::PathBuf, process::Command};

    let (mut session, home, source) = effect_fixture();
    let center = session.origin + IVec3::new(8, 5, 8);
    session.advance_block_ticks();
    let draft = session.draft_dirty_chunk_with_heightmaps_and_simulated_ticks(
        home,
        &source,
        &bubble_heightmap_tags(),
        bubble_air_and_fluid,
    )?;
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let version = game.join("versions/26.3");
    let metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    ensure!(metadata["id"] == "26.3", "wrong Minecraft version");
    let temp = tempfile::tempdir()?;
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for library in metadata["libraries"]
        .as_array()
        .context("libraries missing")?
    {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let classpath = std::env::join_paths(paths)?;
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let oracle = temp.path().join("BubbleEffectSavedTickOracle.java");
    let nbt = temp.path().join("chunk.nbt");
    std::fs::write(
        &oracle,
        include_str!("../tests/java/BubbleEffectSavedTickOracle.java"),
    )?;
    std::fs::write(&nbt, native_persistence::nbt::write(draft.document())?)?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(oracle)
        .output()?;
    ensure!(
        compiled.status.success(),
        "Java tick oracle did not compile: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(classpath)
        .arg("BubbleEffectSavedTickOracle")
        .arg(nbt)
        .args(center.to_array().map(|coordinate| coordinate.to_string()))
        .output()?;
    ensure!(
        output.status.success(),
        "Java tick comparison failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[test]
fn unknown_bubble_callbacks_still_block_saving() {
    let column = state("minecraft:bubble_column", &[("drag", "false")]);
    let (mut session, home, _, _) = fixture(column, BubbleTickKind::Column, false);
    session.advance_block_ticks();
    assert!(session.unresolved_bubble_tick);
    assert!(session.draft_block_ticks(home).is_err());
    let (mut session, home, _, _) = fixture(
        state("minecraft:bubble_column", &[("drag", "true")]),
        BubbleTickKind::Column,
        true,
    );
    session.unresolved_bubble_tick = true;
    session.advance_block_ticks();
    assert!(session.unresolved_bubble_tick);
    assert!(session.draft_block_ticks(home).is_err());

    let (mut malformed, home, _, _) = fixture(
        state("minecraft:water", &[("level", "invalid")]),
        BubbleTickKind::Water,
        true,
    );
    let before = Arc::clone(&malformed.world);
    malformed.advance_block_ticks();
    assert!(malformed.unresolved_bubble_tick);
    assert_eq!(malformed.bubble_deferred.len(), 1);
    assert!(Arc::ptr_eq(&before, &malformed.world));
    assert!(malformed.draft_block_ticks(home).is_err());
}
