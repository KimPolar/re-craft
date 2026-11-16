use super::{
    tests::{fixture_chunk_document, session, test_heightmap_tags},
    *,
};
use native_persistence::{
    edit::{BlockChange, ChunkDraft, draft_chunk_blocks},
    nbt::{self, NamedTag, Tag, TagType},
    palette::BlockState as SavedState,
};
use native_world::loaded::SavedBlockEntities;

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
    kind: FallingKind,
    source_id: &str,
    dusted: &str,
    delay: i32,
    falling: bool,
) -> (LoadedSession, ChunkPos, NamedTag, SavedBlockTick) {
    let mut session = session();
    let home = session.player_chunk().unwrap();
    let position = session.origin + IVec3::new(8, 5, 8);
    let brushable = BlockState {
        name: kind.id().into(),
        properties: [("dusted".into(), dusted.into())].into(),
    };
    let floor = BlockState {
        name: if falling {
            "minecraft:air"
        } else {
            "minecraft:stone"
        }
        .into(),
        properties: Default::default(),
    };
    let mut changes = Vec::new();
    for (pos, next) in [(position, brushable), (position - IVec3::Y, floor)] {
        let old = session.world.block(pos.x, pos.y, pos.z).unwrap().clone();
        let id = session.intern_block(next.clone());
        Arc::make_mut(&mut session.world)
            .set_block_id(pos.x, pos.y, pos.z, id)
            .unwrap();
        changes.push(BlockChange {
            x: pos.x,
            y: pos.y,
            z: pos.z,
            expected: saved(&old),
            replacement: saved(&next),
        });
    }
    let tick = SavedBlockTick {
        block: source_id.into(),
        position: position.to_array(),
        delay,
        priority: -2,
    };
    // Construct the original NBT independently of the code under test.
    let tick_tag = Tag::Compound(vec![
        ("i".into(), Tag::String(source_id.into())),
        ("x".into(), Tag::Int(position.x)),
        ("y".into(), Tag::Int(position.y)),
        ("z".into(), Tag::Int(position.z)),
        ("t".into(), Tag::Int(delay)),
        ("p".into(), Tag::Int(-2)),
    ]);
    let entities = Tag::List {
        element_type: TagType::Compound,
        values: vec![Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:brushable_block".into())),
            ("x".into(), Tag::Int(position.x)),
            ("y".into(), Tag::Int(position.y)),
            ("z".into(), Tag::Int(position.z)),
            (
                "LootTable".into(),
                Tag::String("minecraft:archaeology/desert_pyramid".into()),
            ),
            ("LootTableSeed".into(), Tag::Long(19)),
        ])],
    };
    let mut source = draft_chunk_blocks(
        &fixture_chunk_document(home),
        native_persistence::world::ChunkPos {
            x: home.x,
            z: home.z,
        },
        &changes,
    )
    .unwrap()
    .document()
    .clone();
    let Tag::Compound(fields) = &mut source.value else {
        unreachable!()
    };
    fields.push((
        "block_ticks".into(),
        Tag::List {
            element_type: TagType::Compound,
            values: vec![tick_tag],
        },
    ));
    fields.push(("block_entities".into(), entities.clone()));
    let world = Arc::make_mut(&mut session.world);
    world.saved_block_ticks.insert(home, vec![tick.clone()]);
    world.saved_block_entities.insert(
        home,
        SavedBlockEntities::Encoded(
            nbt::write(&NamedTag {
                name: "".into(),
                value: entities,
            })
            .unwrap()
            .into(),
        ),
    );
    session.enqueue_saved_sand(std::slice::from_ref(&tick));
    session.mark_tick_chunk_dirty(position.x, position.z);
    (session, home, source, tick)
}

fn draft(session: &LoadedSession, home: ChunkPos, source: &NamedTag) -> anyhow::Result<ChunkDraft> {
    session.draft_dirty_chunk_with_heightmaps_and_simulated_ticks(
        home,
        source,
        &test_heightmap_tags(),
        |_| anyhow::bail!("tick-only draft must not rebuild heightmaps"),
    )
}

#[test]
fn saved_brushable_ticks_allow_pending_and_consumed_chunk_drafts() {
    for kind in [FallingKind::SuspiciousSand, FallingKind::SuspiciousGravel] {
        for source_id in [kind.id(), kind.id().strip_prefix("minecraft:").unwrap()] {
            for dusted in ["0", "1", "2", "3"] {
                for delay in [0, 7] {
                    let (mut session, home, source, tick) =
                        fixture(kind, source_id, dusted, delay, false);
                    let before = nbt::write(&source).unwrap();
                    if delay == 0 {
                        session.advance_block_ticks();
                        assert!(!session.unresolved_brushable_lifecycle);
                        assert!(session.falling_sand.is_empty());
                    }
                    let result = draft(&session, home, &source).unwrap();
                    assert!(result.matches_source(&source).unwrap());
                    assert_eq!(nbt::write(&source).unwrap(), before);
                    assert_eq!(
                        result.document().value.get("sections"),
                        source.value.get("sections")
                    );
                    assert_eq!(
                        result.document().value.get("block_entities"),
                        source.value.get("block_entities")
                    );
                    assert_eq!(
                        result.document().value.get("unknown_chunk_data"),
                        source.value.get("unknown_chunk_data")
                    );
                    let Some(Tag::List { values, .. }) = result.document().value.get("block_ticks")
                    else {
                        panic!("missing block ticks");
                    };
                    if delay == 0 {
                        assert!(values.is_empty());
                    } else {
                        assert_eq!(values.len(), 1);
                        assert_eq!(values[0].get("i"), Some(&Tag::String(kind.id().into())));
                        assert_eq!(values[0].get("x"), Some(&Tag::Int(tick.position[0])));
                        assert_eq!(values[0].get("y"), Some(&Tag::Int(tick.position[1])));
                        assert_eq!(values[0].get("z"), Some(&Tag::Int(tick.position[2])));
                        assert_eq!(values[0].get("t"), Some(&Tag::Int(delay)));
                        assert_eq!(values[0].get("p"), Some(&Tag::Int(-2)));
                    }
                }
            }
        }
    }
}

#[test]
fn unsupported_brushable_fall_still_blocks_chunk_draft() {
    for kind in [FallingKind::SuspiciousSand, FallingKind::SuspiciousGravel] {
        let (mut session, home, source, _) = fixture(kind, kind.id(), "0", 0, true);
        let before = nbt::write(&source).unwrap();
        session.advance_block_ticks();
        assert!(session.unresolved_brushable_lifecycle);
        assert!(draft(&session, home, &source).is_err());
        assert_eq!(nbt::write(&source).unwrap(), before);
    }
}
