use super::{
    tests::{fixture_chunk_document, session, test_heightmap_tags},
    *,
};

#[test]
fn pending_leaf_ticks_survive_dirty_chunk_draft_with_cross_kind_order() {
    let mut session = session();
    let home = session.player_chunk().unwrap();
    let position = (session.origin + IVec3::new(8, 5, 8)).to_array();
    let ticks = vec![
        SavedBlockTick {
            block: "minecraft:oak_leaves".into(),
            position,
            delay: 20,
            priority: -1,
        },
        SavedBlockTick {
            block: "minecraft:sand".into(),
            position,
            delay: 3,
            priority: 0,
        },
        SavedBlockTick {
            block: "minecraft:oak_leaves".into(),
            position,
            delay: 5,
            priority: 1,
        },
        SavedBlockTick {
            block: "minecraft:water".into(),
            position,
            delay: 6,
            priority: -2,
        },
        SavedBlockTick {
            block: "minecraft:cherry_leaves".into(),
            position,
            delay: 7,
            priority: 2,
        },
    ];
    let mut source = native_persistence::edit::draft_chunk_blocks(
        &fixture_chunk_document(home),
        native_persistence::world::ChunkPos {
            x: home.x,
            z: home.z,
        },
        &[],
    )
    .unwrap();
    source.replace_block_ticks(&ticks).unwrap();
    let source = source.document().clone();
    Arc::make_mut(&mut session.world)
        .saved_block_ticks
        .insert(home, ticks.clone());
    session.enqueue_saved_sand(&ticks);
    session.enqueue_saved_leaves(&ticks);
    session.enqueue_saved_bubble(&ticks);
    assert_eq!(session.draft_block_ticks(home).unwrap(), ticks);
    session.mark_tick_chunk_dirty(home.x * 16, home.z * 16);
    let draft = session
        .draft_dirty_chunk_with_heightmaps_and_simulated_ticks(
            home,
            &source,
            &test_heightmap_tags(),
            |_| anyhow::bail!("tick-only draft must not rebuild heightmaps"),
        )
        .unwrap();
    assert!(draft.matches_source(&source).unwrap());
    assert_eq!(
        draft.document().value.get("block_ticks"),
        source.value.get("block_ticks")
    );
}
