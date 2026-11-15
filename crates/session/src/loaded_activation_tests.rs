use super::*;
use native_persistence::{
    nbt::{NamedTag, Tag, TagType},
    world::{ChunkArea, DimensionId, LoadedChunk, TARGET_DATA_VERSION},
};
use native_world::loaded::{SavedBlockEntities, SavedChunkActivation};

fn streamed_chunk(pos: ChunkPos, post: Tag) -> World {
    let saved_pos = native_persistence::world::ChunkPos { x: pos.x, z: pos.z };
    let document = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![
            ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
            ("xPos".into(), Tag::Int(pos.x)),
            ("zPos".into(), Tag::Int(pos.z)),
            ("Status".into(), Tag::String("minecraft:full".into())),
            ("PostProcessing".into(), post),
        ]),
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

    session.queue_chunk_window(home, 0);
    session.update(TICK);
    assert!(!session.world.chunks.contains_key(&neighbor));
    assert!(session.world.saved_chunk_source(neighbor).is_none());
    assert!(session.validate_saved_chunk_preparation().is_err());
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
