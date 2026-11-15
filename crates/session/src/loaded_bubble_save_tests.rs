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

#[test]
fn changed_or_unknown_bubble_callbacks_still_block_saving() {
    let column = state("minecraft:bubble_column", &[("drag", "false")]);
    for with_tags in [false, true] {
        let (mut session, home, _, _) = fixture(column.clone(), BubbleTickKind::Column, with_tags);
        session.advance_block_ticks();
        assert!(session.unresolved_bubble_tick);
        assert!(session.draft_block_ticks(home).is_err());
    }
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
