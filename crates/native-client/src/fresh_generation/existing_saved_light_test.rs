use super::*;
use native_persistence::heightmap::{HeightBounds, SavedGenerationHeightmaps};
use native_resources::surface_pass::SurfaceStageChunk;
use native_session::worldgen::{
    BiomeNeighborhood, CarvedTerrainWorld, QuartBiomeChunk, QueuedGenerationLightEngine,
    SavedLightLoadOrder, materialize_carved_terrain,
    schedule::{GenerationSchedule, GenerationStage},
};
use native_world::{
    generation_light::{LightChannel, QueuedSectionLightTracker},
    loaded::{BIOME_COUNT, BLOCK_COUNT, BlockState, Chunk, ChunkPos, LIGHT_BYTES, Section, World},
};

fn is_air(state: &BlockState) -> Option<bool> {
    match state.name.as_str() {
        "minecraft:air" => Some(true),
        "minecraft:stone" => Some(false),
        _ => None,
    }
}

fn fixture() -> Result<(
    CapturedTerrainBatch,
    CapturedQueuedLight,
    RestoredSavedTerrain,
)> {
    let center = ChunkPos { x: 0, z: 0 };
    let east = ChunkPos { x: 1, z: 0 };
    let base = ChunkPos { x: -1, z: 0 };
    let identity = Arc::new(());
    let plains = ResourceId::parse("minecraft:plains")?;
    let biome = |pos: ChunkPos| {
        QuartBiomeChunk::from_values([pos.x, pos.z], 42, 0, 32, vec![plains.clone(); 128])
            .map(Arc::new)
    };
    let mut biomes = BiomeNeighborhood::new(42, 0, 32)?;
    for position in [base, center, east] {
        biomes.insert(biome(position)?)?;
    }
    let base_biome = biomes.chunk([base.x, base.z]).unwrap();
    let air_value = Arc::new(serde_json::json!("minecraft:air"));
    let generated = materialize_carved_terrain(
        SurfaceStageChunk {
            chunk_position: [base.x, base.z],
            min_y: 0,
            height: 32,
            states: vec![air_value; 32 * 256],
            fluid_updates: vec![false; 32 * 256],
            noise_postprocessing: None,
            surface_postprocessing: None,
            carver_postprocessing: None,
        },
        base_biome,
        "minecraft:overworld",
        |value| {
            Ok(BlockState {
                name: value.as_str().unwrap().into(),
                properties: BTreeMap::new(),
            })
        },
    )?;
    let mut saved_world = World::new("minecraft:overworld");
    let air = saved_world.intern_state(BlockState {
        name: "minecraft:air".into(),
        properties: BTreeMap::new(),
    });
    let stone = saved_world.intern_state(BlockState {
        name: "minecraft:stone".into(),
        properties: BTreeMap::new(),
    });
    let biome_id = saved_world.intern_biome("minecraft:plains".into());
    let mut sources = BTreeMap::new();
    let mut saved_biomes = BTreeMap::new();
    let mut heightmaps = BTreeMap::new();
    let mut postprocessing = BTreeMap::new();
    for position in [center, east] {
        let mut sections = BTreeMap::new();
        for y in 0..2 {
            let mut blocks = Box::new([air; BLOCK_COUNT]);
            if y == 0 {
                blocks[256] = stone;
            }
            sections.insert(
                y,
                Arc::new(Section {
                    blocks: Some(blocks),
                    biomes: Some(Box::new([biome_id; BIOME_COUNT])),
                    block_light: (y == 0).then(|| Box::new([0x33; LIGHT_BYTES])),
                    sky_light: None,
                }),
            );
        }
        saved_world.chunks.insert(
            position,
            Arc::new(Chunk {
                position,
                status: Some("minecraft:full".into()),
                block_light_correct: true,
                sections,
            }),
        );
        let source = Arc::new(NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("Status".into(), Tag::String("minecraft:full".into())),
                ("xPos".into(), Tag::Int(position.x)),
                ("zPos".into(), Tag::Int(position.z)),
            ]),
        });
        let key = native_persistence::world::ChunkPos {
            x: position.x,
            z: position.z,
        };
        heightmaps.insert(
            position,
            SavedGenerationHeightmaps::read(
                &source,
                HeightBounds {
                    min_y: 0,
                    max_y: 32,
                },
            )?,
        );
        sources.insert(key, source);
        saved_biomes.insert(position, biome(position)?);
        postprocessing.insert(position, None);
    }
    let restored = RestoredSavedTerrain {
        identity: Arc::clone(&identity),
        world: Arc::new(saved_world),
        sources,
        biomes: saved_biomes,
        heightmaps,
        postprocessing,
    };
    let mut terrain = CarvedTerrainWorld::new(generated);
    terrain.insert_saved_chunk(&restored.world, center, &restored.heightmaps[&center], None)?;
    let tracker = QueuedSectionLightTracker::fresh(true, false, []).unwrap();
    let mut engine =
        QueuedGenerationLightEngine::new(&terrain, tracker, [], BTreeMap::new(), false, is_air)?;
    engine.load_saved_chunk(&terrain, center, SavedLightLoadOrder::SameDrain, is_air)?;
    let mut schedule = GenerationSchedule::new();
    schedule.request(east, GenerationStage::Terrain)?;
    schedule.restore_imported([
        (center, GenerationStage::Terrain),
        (east, GenerationStage::Biomes),
    ])?;
    let center_key = native_persistence::world::ChunkPos {
        x: center.x,
        z: center.z,
    };
    let base_key = native_persistence::world::ChunkPos {
        x: base.x,
        z: base.z,
    };
    let revision = Arc::new(());
    let batch = CapturedTerrainBatch {
        identity: Arc::clone(&identity),
        schedule,
        biomes,
        terrain,
        sources: BTreeMap::from([
            (base_key, None),
            (center_key, Some(Arc::clone(&restored.sources[&center_key]))),
        ]),
        saved_stages: BTreeMap::from([(center, GenerationStage::Full)]),
        generated_references: BTreeMap::new(),
        generated_starts: BTreeMap::new(),
        treasure_starts: BTreeMap::new(),
        feature_entity_sources: ResumedFeatureEntitySources::default(),
        revision: Arc::clone(&revision),
    };
    let light = CapturedQueuedLight {
        identity,
        revision,
        engine,
    };
    Ok((batch, light, restored))
}

#[test]
fn captured_saved_arrival_commits_terrain_and_light_together() -> Result<()> {
    let (mut batch, mut light, mut restored) = fixture()?;
    let east = ChunkPos { x: 1, z: 0 };
    let key = native_persistence::world::ChunkPos { x: 1, z: 0 };
    let order = SavedLightLoadOrder::SameDrain;
    let before = batch.terrain.snapshot();
    let before_light = light.engine.tracker().snapshot().clone();
    let before_revision = Arc::clone(&batch.revision);

    let identity = Arc::clone(&restored.identity);
    restored.identity = Arc::new(());
    assert!(
        batch
            .import_saved_chunk_with_queued_light(&restored, &mut light, east, order, is_air)
            .is_err()
    );
    restored.identity = identity;
    let original_source = restored.sources.remove(&key).unwrap();
    assert!(
        batch
            .import_saved_chunk_with_queued_light(&restored, &mut light, east, order, is_air)
            .is_err()
    );
    restored.sources.insert(key, original_source);
    let original_biomes = restored.biomes.remove(&east).unwrap();
    assert!(
        batch
            .import_saved_chunk_with_queued_light(&restored, &mut light, east, order, is_air)
            .is_err()
    );
    restored.biomes.insert(east, original_biomes);
    let original_heightmaps = restored.heightmaps.remove(&east).unwrap();
    assert!(
        batch
            .import_saved_chunk_with_queued_light(&restored, &mut light, east, order, is_air)
            .is_err()
    );
    restored.heightmaps.insert(east, original_heightmaps);
    let original_light_revision = Arc::clone(&light.revision);
    light.revision = Arc::new(());
    assert!(
        batch
            .import_saved_chunk_with_queued_light(&restored, &mut light, east, order, is_air)
            .is_err()
    );
    light.revision = original_light_revision;
    assert!(
        batch
            .import_saved_chunk_with_queued_light(&restored, &mut light, east, order, |_| None)
            .is_err()
    );
    assert!(Arc::ptr_eq(&before, &batch.terrain.snapshot()));
    assert!(Arc::ptr_eq(&before_revision, &batch.revision));
    assert!(light.matches_batch(&batch));
    assert_eq!(
        before_light
            .same_complete_column(light.engine.tracker().snapshot(), ChunkPos { x: 0, z: 0 }),
        Some(true)
    );
    assert_eq!(
        batch.schedule.completed(east),
        Some(GenerationStage::Biomes)
    );
    assert!(!batch.sources.contains_key(&key));
    assert!(!light.engine.tracker().active_chunks().contains(&east));

    batch.import_saved_chunk_with_queued_light(&restored, &mut light, east, order, is_air)?;
    assert!(!Arc::ptr_eq(&before_revision, &batch.revision));
    assert!(light.matches_batch(&batch));
    assert!(!before.chunks.contains_key(&east));
    assert_eq!(
        batch.terrain.snapshot().block(16, 1, 0).unwrap().name,
        "minecraft:stone"
    );
    assert_eq!(
        batch.schedule.completed(east),
        Some(GenerationStage::Terrain)
    );
    assert_eq!(batch.saved_stage(east), Some(GenerationStage::Full));
    assert!(Arc::ptr_eq(
        batch.sources[&key].as_ref().unwrap(),
        &restored.sources[&key]
    ));
    assert!(light.engine.tracker().active_chunks().contains(&east));
    assert_eq!(
        light
            .engine
            .tracker()
            .data_layer(LightChannel::Block, east, 0)
            .unwrap()
            .unwrap()
            .bytes()[0],
        0x33
    );
    let after = batch.terrain.snapshot();
    assert!(
        batch
            .import_saved_chunk_with_queued_light(&restored, &mut light, east, order, is_air)
            .is_err()
    );
    assert!(Arc::ptr_eq(&after, &batch.terrain.snapshot()));
    Ok(())
}

#[test]
fn restored_chunk_comparison_rejects_reapplied_block_edits() -> Result<()> {
    use native_world::loaded::BlockPosition;

    let (_, _, restored) = fixture()?;
    let east = ChunkPos { x: 1, z: 0 };
    let mut unchanged = World::new("minecraft:overworld");
    unchanged.import_chunk(&restored.world, east)?;
    assert!(native_session::loaded::matching_terrain_chunks(
        &restored.world,
        &restored.world.chunks[&east],
        &unchanged,
        &unchanged.chunks[&east],
    ));

    let mut edited = World::new("minecraft:overworld");
    edited.record_block_edit(
        BlockPosition { x: 16, y: 1, z: 0 },
        Some(BlockState {
            name: "minecraft:stone".into(),
            properties: BTreeMap::new(),
        }),
        BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        },
    );
    edited.import_chunk(&restored.world, east)?;
    assert_eq!(edited.block(16, 1, 0).unwrap().name, "minecraft:air");
    assert!(!native_session::loaded::matching_terrain_chunks(
        &restored.world,
        &restored.world.chunks[&east],
        &edited,
        &edited.chunks[&east],
    ));
    Ok(())
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3"]
fn captured_generator_streams_restored_saved_light() -> Result<()> {
    use flate2::{Compression, write::GzEncoder};
    use native_persistence::{
        nbt::TagType,
        save::{FreshRegionDraft, FreshRegionKind},
        world::{ChunkPos as SavedPos, WorldReader},
        worldgen_settings::{NewWorldOptions, draft_baked_worldgen_settings},
    };
    use std::io::Write;

    let game = match std::env::var_os("MINECRAFT_DIR") {
        Some(path) => std::path::PathBuf::from(path),
        None => native_resources::default_minecraft_dir()?,
    };
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    let mut resources = ResourceStack::open(&game)?;
    let lighting = GeneratedDimensionLighting::load_preset(
        &mut resources,
        "minecraft:normal",
        "minecraft:overworld",
    )?;
    let settings = draft_baked_worldgen_settings(
        &mut resources,
        "minecraft:normal",
        NewWorldOptions {
            seed: 42,
            generate_structures: false,
            bonus_chest: false,
        },
    )?;
    std::fs::create_dir_all(root.join("data/minecraft"))?;
    std::fs::write(
        root.join("data/minecraft/world_gen_settings.dat"),
        native_persistence::nbt::write(&settings)?,
    )?;
    let level = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![(
            "Data".into(),
            Tag::Compound(vec![("DataVersion".into(), Tag::Int(5023))]),
        )]),
    };
    let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
    gzip.write_all(&native_persistence::nbt::write(&level)?)?;
    std::fs::write(root.join("level.dat"), gzip.finish()?)?;
    let section = |y: i8| {
        Tag::Compound(vec![
            ("Y".into(), Tag::Byte(y)),
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
            (
                "biomes".into(),
                Tag::Compound(vec![(
                    "palette".into(),
                    Tag::List {
                        element_type: TagType::String,
                        values: vec![Tag::String("minecraft:plains".into())],
                    },
                )]),
            ),
        ])
    };
    let mut region = FreshRegionDraft::new(FreshRegionKind::Terrain, 0, 0);
    for x in 0..=1 {
        let saved = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("xPos".into(), Tag::Int(x)),
                ("zPos".into(), Tag::Int(0)),
                ("Status".into(), Tag::String("minecraft:full".into())),
                ("isLightOn".into(), Tag::Byte(1)),
                (
                    "sections".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: (lighting.min_y.div_euclid(16)
                            ..(lighting.min_y + lighting.height).div_euclid(16))
                            .map(|y| section(i8::try_from(y).unwrap()))
                            .collect(),
                    },
                ),
            ]),
        };
        region.add(SavedPos { x, z: 0 }, &saved, 1)?;
    }
    let region_dir = root.join("dimensions/minecraft/overworld/region");
    std::fs::create_dir_all(&region_dir)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .read(true)
        .create_new(true)
        .open(region_dir.join("r.0.0.mca"))?;
    region.write_staged(&mut file)?;
    drop(file);
    let center = ChunkPos { x: 0, z: 0 };
    let east = ChunkPos { x: 1, z: 0 };
    let base = ChunkPos { x: -1, z: 0 };
    let saved_center = SavedPos { x: 0, z: 0 };
    let saved_east = SavedPos { x: 1, z: 0 };
    let saved_base = SavedPos { x: -1, z: 0 };
    let mut reader =
        WorldReader::open_for_session(root, DimensionId::parse("minecraft:overworld")?)?;
    let context = reader.capture_generation_context([saved_base, saved_center, saved_east])?;
    let mut generator = prepare_existing_generator(context, &game)?;
    generator.ensure_terrain_assets()?;
    let mut restored = generator.restore_saved_terrain([saved_center, saved_east])?;
    let mut biomes = BiomeNeighborhood::new(42, lighting.min_y, lighting.height)?;
    let base_biome = Arc::new(QuartBiomeChunk::from_values(
        [base.x, base.z],
        42,
        lighting.min_y,
        lighting.height,
        vec![ResourceId::parse("minecraft:plains")?; (lighting.height / 4 * 16) as usize],
    )?);
    biomes.insert(Arc::clone(&base_biome))?;
    biomes.insert(Arc::clone(&restored.biomes[&center]))?;
    biomes.insert(Arc::clone(&restored.biomes[&east]))?;
    let air = Arc::new(serde_json::json!("minecraft:air"));
    let length = usize::try_from(lighting.height)? * 256;
    let generated = materialize_carved_terrain(
        SurfaceStageChunk {
            chunk_position: [base.x, base.z],
            min_y: lighting.min_y,
            height: lighting.height,
            states: vec![air; length],
            fluid_updates: vec![false; length],
            noise_postprocessing: None,
            surface_postprocessing: None,
            carver_postprocessing: None,
        },
        &base_biome,
        "minecraft:overworld",
        |value| {
            Ok(BlockState {
                name: value.as_str().unwrap().into(),
                properties: BTreeMap::new(),
            })
        },
    )?;
    let mut terrain = CarvedTerrainWorld::new(generated);
    terrain.insert_saved_chunk(&restored.world, center, &restored.heightmaps[&center], None)?;
    let registry = &generator.terrain_assets.as_ref().unwrap().blocks;
    let tracker = QueuedSectionLightTracker::fresh(true, lighting.has_skylight, []).unwrap();
    let mut engine = QueuedGenerationLightEngine::new_with_registry(
        &terrain,
        tracker,
        [],
        BTreeMap::new(),
        lighting.has_skylight,
        registry,
    )?;
    engine.load_saved_chunk_with_registry(
        &terrain,
        center,
        SavedLightLoadOrder::SameDrain,
        registry,
    )?;
    let mut schedule = GenerationSchedule::new();
    schedule.request(east, GenerationStage::Terrain)?;
    schedule.restore_imported([
        (center, GenerationStage::Terrain),
        (east, GenerationStage::Biomes),
    ])?;
    let revision = Arc::new(());
    let batch = CapturedTerrainBatch {
        identity: Arc::clone(&generator.identity),
        schedule,
        biomes,
        terrain,
        sources: BTreeMap::from([
            (saved_base, None),
            (
                saved_center,
                Some(Arc::clone(&restored.sources[&saved_center])),
            ),
        ]),
        saved_stages: BTreeMap::from([(center, GenerationStage::Full)]),
        generated_references: BTreeMap::new(),
        generated_starts: BTreeMap::new(),
        treasure_starts: BTreeMap::new(),
        feature_entity_sources: ResumedFeatureEntitySources::default(),
        revision: Arc::clone(&revision),
    };
    let mut batch = StagedLiveTerrainBatch {
        batch,
        feature_order: Vec::new(),
        mineshaft_starts: BTreeMap::new(),
    };
    let mut light = CapturedQueuedLight {
        identity: Arc::clone(&generator.identity),
        revision,
        engine,
    };
    let original = Arc::clone(&restored.sources[&saved_east]);
    restored
        .sources
        .insert(saved_east, Arc::new((*original).clone()));
    let before = batch.snapshot();
    assert!(
        generator
            .stream_staged_live_saved_chunk_with_queued_light(
                &mut batch,
                &restored,
                &mut light,
                east,
                SavedLightLoadOrder::SeparateDrains,
            )
            .is_err()
    );
    assert!(Arc::ptr_eq(&before, &batch.snapshot()));
    restored.sources.insert(saved_east, original);
    generator.stream_staged_live_saved_chunk_with_queued_light(
        &mut batch,
        &restored,
        &mut light,
        east,
        SavedLightLoadOrder::SeparateDrains,
    )?;
    assert!(light.matches_batch(&batch.batch));
    assert!(light.engine.tracker().active_chunks().contains(&east));
    assert_eq!(batch.batch.saved_stage(east), Some(GenerationStage::Full));
    assert_eq!(batch.completed_stage(east), Some(GenerationStage::Terrain));
    assert_eq!(
        batch.snapshot().block(16, lighting.min_y, 0).unwrap().name,
        "minecraft:stone"
    );
    Ok(())
}
