use super::*;
use native_core::postprocessing::PostProcessing;
use native_persistence::{
    heightmap::{GeneratedHeightmaps, encode_generated_heightmaps},
    nbt::TagType,
    palette,
    world::{ChunkPos as SavedChunkPos, LoadedChunk},
};

struct Tags;
impl ResourceProvider for Tags {
    fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }

    fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        if path.ends_with("blocks_motion_in_heightmap.json")
            || path.ends_with("blocks_motion_in_heightmap_no_leaves.json")
        {
            Ok(Some(
                br#"{"values":["minecraft:stone","minecraft:sand","minecraft:red_sand","minecraft:gravel"]}"#.to_vec(),
            ))
        } else {
            Ok(None)
        }
    }
}

fn fixture() -> Result<(ChunkArea, BTreeSet<String>)> {
    let pos = SavedChunkPos { x: 0, z: 0 };
    let names = ["stone", "sand", "red_sand", "gravel"];
    let ids = names
        .iter()
        .map(|name| format!("minecraft:{name}"))
        .collect();
    let mut indices = vec![0; 4096];
    for (x, id) in [(8, 1), (9, 2), (10, 3)] {
        indices[x + 8 * 16 + 8 * 256] = id;
    }
    let data = palette::encode_indices(names.len(), &indices, 4)?.unwrap();
    let mut marks = PostProcessing::new(0, 16).unwrap();
    for x in [8, 8, 9, 10] {
        assert!(marks.mark(x, 8, 8));
    }
    let surface = [16; 256];
    let bounds = HeightBounds {
        min_y: 0,
        max_y: 16,
    };
    let maps = encode_generated_heightmaps(
        bounds,
        &GeneratedHeightmaps {
            world_surface: Some(&surface),
            ocean_floor: Some(&surface),
            motion_blocking: Some(&surface),
            motion_blocking_no_leaves: Some(&surface),
            ..Default::default()
        },
    )?;
    let tick = Tag::Compound(vec![
        ("i".into(), Tag::String("minecraft:sand".into())),
        ("x".into(), Tag::Int(8)),
        ("y".into(), Tag::Int(8)),
        ("z".into(), Tag::Int(8)),
        ("t".into(), Tag::Int(0)),
        ("p".into(), Tag::Int(0)),
    ]);
    let document = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![
            ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
            ("xPos".into(), Tag::Int(pos.x)),
            ("zPos".into(), Tag::Int(pos.z)),
            ("Status".into(), Tag::String("minecraft:full".into())),
            ("isLightOn".into(), Tag::Byte(1)),
            (
                "sections".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![Tag::Compound(vec![
                        ("Y".into(), Tag::Byte(0)),
                        (
                            "block_states".into(),
                            Tag::Compound(vec![
                                (
                                    "palette".into(),
                                    Tag::List {
                                        element_type: TagType::String,
                                        values: names
                                            .iter()
                                            .map(|name| {
                                                Tag::String(
                                                    format!("minecraft:{name}").as_str().into(),
                                                )
                                            })
                                            .collect(),
                                    },
                                ),
                                ("data".into(), Tag::LongArray(data)),
                            ]),
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
                    ])],
                },
            ),
            ("Heightmaps".into(), maps),
            (
                "PostProcessing".into(),
                native_persistence::postprocessing::encode(&marks),
            ),
            (
                "block_ticks".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![tick],
                },
            ),
            (
                "fluid_ticks".into(),
                Tag::List {
                    element_type: TagType::End,
                    values: vec![],
                },
            ),
        ]),
    };
    let sections = palette::decode_sections(&document.value)?;
    let chunk = LoadedChunk {
        position: pos,
        data_version: TARGET_DATA_VERSION,
        status: Some("minecraft:full".into()),
        document: Arc::new(document),
        sections,
    };
    Ok((
        ChunkArea {
            center: pos,
            radius: 0,
            chunks: BTreeMap::from([(pos, Arc::new(chunk))]),
            missing: vec![],
        },
        ids,
    ))
}

fn begin(area: &ChunkArea, ids: &BTreeSet<String>) -> Result<GeneratedPostprocessing> {
    begin_with_registration(area, ids, &[ChunkPos { x: 0, z: 0 }])
}

fn begin_with_registration(
    area: &ChunkArea,
    ids: &BTreeSet<String>,
    registration: &[ChunkPos],
) -> Result<GeneratedPostprocessing> {
    let dimension = DimensionId::parse("overworld")?;
    let defaults = BTreeMap::from([
        (
            "minecraft:chest".into(),
            BTreeMap::from([
                ("facing".into(), "north".into()),
                ("type".into(), "single".into()),
                ("waterlogged".into(), "false".into()),
            ]),
        ),
        (
            "minecraft:potent_sulfur".into(),
            BTreeMap::from([("potent_sulfur_state".into(), "dry".into())]),
        ),
        (
            "minecraft:suspicious_sand".into(),
            BTreeMap::from([("dusted".into(), "0".into())]),
        ),
        (
            "minecraft:suspicious_gravel".into(),
            BTreeMap::from([("dusted".into(), "0".into())]),
        ),
    ]);
    begin_saved_postprocessing(
        &mut Tags,
        SavedPostprocessingInputs {
            area,
            dimension: &dimension,
            defaults: &defaults,
            block_ids: ids,
            bounds: BuildBounds {
                min_y: 0,
                max_y: 16,
            },
            registration,
            game_time: 100,
            next_sub_tick: 2,
            light: None,
        },
        |state| {
            ensure!(
                ids.contains(&state.name)
                    && state.properties == defaults.get(&state.name).cloned().unwrap_or_default(),
                "unknown fixture state"
            );
            Ok(())
        },
    )
}

fn ticking_fixture() -> Result<(ChunkArea, BTreeSet<String>, Vec<ChunkPos>)> {
    let (mut area, ids) = fixture()?;
    let center = ChunkPos { x: 0, z: 0 };
    let registration = crate::ticking_preparation::ticking_dependencies(center).to_vec();
    for &position in registration.iter().filter(|&&position| position != center) {
        let mut chunk = area.chunks[&area.center].as_ref().clone();
        chunk.position = SavedChunkPos {
            x: position.x,
            z: position.z,
        };
        let document = Arc::make_mut(&mut chunk.document);
        let Tag::Compound(fields) = &mut document.value else {
            unreachable!()
        };
        fields
            .iter_mut()
            .find(|(name, _)| name.matches("xPos"))
            .unwrap()
            .1 = Tag::Int(position.x);
        fields
            .iter_mut()
            .find(|(name, _)| name.matches("zPos"))
            .unwrap()
            .1 = Tag::Int(position.z);
        fields
            .iter_mut()
            .find(|(name, _)| name.matches("PostProcessing"))
            .unwrap()
            .1 = native_persistence::postprocessing::encode(&PostProcessing::new(0, 16).unwrap());
        fields
            .iter_mut()
            .find(|(name, _)| name.matches("block_ticks"))
            .unwrap()
            .1 = Tag::List {
            element_type: TagType::Compound,
            values: vec![Tag::Compound(vec![
                ("i".into(), Tag::String("minecraft:sand".into())),
                ("x".into(), Tag::Int(position.x * 16 + 8)),
                ("y".into(), Tag::Int(8)),
                ("z".into(), Tag::Int(position.z * 16 + 8)),
                ("t".into(), Tag::Int(7)),
                ("p".into(), Tag::Int(0)),
            ])],
        };
        area.chunks.insert(chunk.position, Arc::new(chunk));
    }
    Ok((area, ids, registration))
}

fn overlapping_ticking_fixture() -> Result<(ChunkArea, BTreeSet<String>, Vec<ChunkPos>)> {
    let (mut area, ids, _) = ticking_fixture()?;
    let marks = area.chunks[&area.center]
        .document
        .value
        .get("PostProcessing")
        .unwrap()
        .clone();
    for z in -1..=1 {
        let mut chunk = area.chunks[&SavedChunkPos { x: 1, z }].as_ref().clone();
        chunk.position = SavedChunkPos { x: 2, z };
        let document = Arc::make_mut(&mut chunk.document);
        let Tag::Compound(fields) = &mut document.value else {
            unreachable!()
        };
        fields
            .iter_mut()
            .find(|(name, _)| name.matches("xPos"))
            .unwrap()
            .1 = Tag::Int(2);
        fields
            .iter_mut()
            .find(|(name, _)| name.matches("block_ticks"))
            .unwrap()
            .1 = Tag::List {
            element_type: TagType::Compound,
            values: vec![Tag::Compound(vec![
                ("i".into(), Tag::String("minecraft:sand".into())),
                ("x".into(), Tag::Int(40)),
                ("y".into(), Tag::Int(8)),
                ("z".into(), Tag::Int(z * 16 + 8)),
                ("t".into(), Tag::Int(7)),
                ("p".into(), Tag::Int(0)),
            ])],
        };
        area.chunks.insert(chunk.position, Arc::new(chunk));
    }
    let second = Arc::make_mut(area.chunks.get_mut(&SavedChunkPos { x: 1, z: 0 }).unwrap());
    let Tag::Compound(fields) = &mut Arc::make_mut(&mut second.document).value else {
        unreachable!()
    };
    fields
        .iter_mut()
        .find(|(name, _)| name.matches("PostProcessing"))
        .unwrap()
        .1 = marks;
    let registration = (-1..=1)
        .flat_map(|z| (-1..=2).map(move |x| ChunkPos { x, z }))
        .collect();
    Ok((area, ids, registration))
}

fn begin_ticking_batch(
    area: &ChunkArea,
    ids: &BTreeSet<String>,
    registration: &[ChunkPos],
) -> Result<SavedTickingBatch> {
    SavedTickingBatch::begin(
        &mut Tags,
        SavedPostprocessingInputs {
            area,
            dimension: &DimensionId::parse("overworld")?,
            defaults: &BTreeMap::new(),
            block_ids: ids,
            bounds: BuildBounds {
                min_y: 0,
                max_y: 16,
            },
            registration,
            game_time: 500,
            next_sub_tick: 2,
            light: None,
        },
        |state| {
            ensure!(
                ids.contains(&state.name) && state.properties.is_empty(),
                "unknown fixture state"
            );
            Ok(())
        },
        |_| Ok(()),
    )
}

fn water_ticking_fixture() -> Result<(ChunkArea, BTreeSet<String>, Vec<ChunkPos>)> {
    let (mut area, mut ids, registration) = overlapping_ticking_fixture()?;
    ids.extend(["minecraft:air".into(), "minecraft:water".into()]);
    for (chunk_x, local_x, name) in [(0, 15, "minecraft:water"), (1, 0, "minecraft:air")] {
        let pos = SavedChunkPos { x: chunk_x, z: 0 };
        let chunk = Arc::make_mut(area.chunks.get_mut(&pos).unwrap());
        let document = Arc::make_mut(&mut chunk.document);
        let Tag::Compound(fields) = &mut document.value else {
            unreachable!()
        };
        let mut marks = PostProcessing::new(0, 16).unwrap();
        marks.mark(chunk_x * 16 + local_x, 8, 8);
        fields
            .iter_mut()
            .find(|(key, _)| key.matches("PostProcessing"))
            .unwrap()
            .1 = native_persistence::postprocessing::encode(&marks);
        fields
            .iter_mut()
            .find(|(key, _)| key.matches("block_ticks"))
            .unwrap()
            .1 = Tag::List {
            element_type: TagType::End,
            values: vec![],
        };
        let Tag::List {
            values: sections, ..
        } = &mut fields
            .iter_mut()
            .find(|(key, _)| key.matches("sections"))
            .unwrap()
            .1
        else {
            unreachable!()
        };
        let Tag::Compound(section) = &mut sections[0] else {
            unreachable!()
        };
        let mut indices = vec![0; 4096];
        indices[(local_x + 8 * 16 + 8 * 256) as usize] = 1;
        let data = palette::encode_indices(2, &indices, 4)?.unwrap();
        section
            .iter_mut()
            .find(|(key, _)| key.matches("block_states"))
            .unwrap()
            .1 = Tag::Compound(vec![
            (
                "palette".into(),
                Tag::List {
                    element_type: TagType::String,
                    values: vec![
                        Tag::String("minecraft:stone".into()),
                        Tag::String(name.into()),
                    ],
                },
            ),
            ("data".into(), Tag::LongArray(data)),
        ]);
        chunk.sections = palette::decode_sections(&document.value)?;
    }
    Ok((area, ids, registration))
}

fn begin_water_ticking_batch(
    area: &ChunkArea,
    ids: &BTreeSet<String>,
    registration: &[ChunkPos],
) -> Result<SavedTickingBatch> {
    let defaults = BTreeMap::from([(
        "minecraft:water".into(),
        BTreeMap::from([("level".into(), "0".into())]),
    )]);
    SavedTickingBatch::begin(
        &mut Tags,
        SavedPostprocessingInputs {
            area,
            dimension: &DimensionId::parse("overworld")?,
            defaults: &defaults,
            block_ids: ids,
            bounds: BuildBounds {
                min_y: 0,
                max_y: 16,
            },
            registration,
            game_time: 100,
            next_sub_tick: 2,
            light: None,
        },
        |state| {
            ensure!(
                ids.contains(&state.name)
                    && state.properties == defaults.get(&state.name).cloned().unwrap_or_default(),
                "unknown fixture state"
            );
            Ok(())
        },
        |stage| {
            stage.set_water_source_conversion(true)?;
            stage.set_fluid_tags(
                Some(Arc::new(BTreeSet::from([
                    "minecraft:air".into(),
                    "minecraft:water".into(),
                ]))),
                Some(Arc::new(BTreeSet::from([
                    "minecraft:lava".into(),
                    "minecraft:flowing_lava".into(),
                ]))),
            );
            stage.set_water_fluid_tags(Some(Arc::new(BTreeSet::from([
                "minecraft:water".into(),
                "minecraft:flowing_water".into(),
            ]))));
            stage.set_bubble_tags(Some(Arc::new(
                native_resources::fluid_tags::BubbleColumnTags::new(
                    BTreeSet::new(),
                    BTreeSet::new(),
                ),
            )));
            Ok(())
        },
    )
}

fn water_ticking_trace() -> Result<Vec<String>> {
    let (area, ids, registration) = water_ticking_fixture()?;
    let a = ChunkPos { x: 0, z: 0 };
    let b = ChunkPos { x: 1, z: 0 };
    let mut batch = begin_water_ticking_batch(&area, &ids, &registration)?;
    let mut first = ready_future(a, &registration)?;
    let mut second = ready_future(b, &registration)?;
    assert!(batch.dispatch(&mut first, 100)?);
    let after_first = batch
        .stage
        .world
        .block(16, 8, 8)
        .context("missing neighbor block")?;
    let mut trace = vec![format!(
        "first {} {}",
        after_first.name,
        after_first
            .properties
            .get("level")
            .map_or("-", String::as_str)
    )];
    assert!(batch.dispatch(&mut second, 100)?);
    let result = batch.finish()?;
    let after_second = result
        .region
        .world
        .block(16, 8, 8)
        .context("missing neighbor block")?;
    trace.push(format!(
        "second {} {}",
        after_second.name,
        after_second
            .properties
            .get("level")
            .map_or("-", String::as_str)
    ));
    for (label, pos) in [("a", a), ("b", b)] {
        for tick in result.fluid_ticks.container(pos).unwrap().pack(100) {
            trace.push(format!(
                "{label} {} {},{},{} {} {}",
                tick.kind,
                tick.position[0],
                tick.position[1],
                tick.position[2],
                tick.delay,
                tick.priority
            ));
        }
    }
    trace.push(format!("counter {}", result.region.ticks().next_sub_tick()));
    Ok(trace)
}

#[test]
fn overlapping_ticking_callbacks_read_previous_neighbor_edits() -> Result<()> {
    let trace = water_ticking_trace()?;
    assert!(trace.contains(&"first minecraft:water 1".to_owned()));
    assert!(trace.contains(&"second minecraft:water 1".to_owned()));
    assert!(
        trace
            .iter()
            .any(|line| line.starts_with("b minecraft:flowing_water 16,8,8 "))
    );
    Ok(())
}

#[test]
#[cfg(target_os = "windows")]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn overlapping_ticking_callbacks_preserve_water_updates_like_java() -> Result<()> {
    let java = super::super::postprocessing_world::tests::java_results_with_args(
        "GeneratedWaterSpreadPostprocessingOracle",
        include_str!("../../tests/java/GeneratedWaterSpreadPostprocessingOracle.java"),
        &["edge_ticking_sequence"],
    );
    assert_eq!(water_ticking_trace()?, java);
    Ok(())
}

fn ready_future(center: ChunkPos, registration: &[ChunkPos]) -> Result<TickingPreparation> {
    use crate::ticking_preparation::{FullFuture, FullFutureId, FullFutureResult};
    TickingPreparation::start(center, |position| {
        Some(FullFuture {
            id: FullFutureId(registration.iter().position(|&pos| pos == position)? as u64),
            result: Some(FullFutureResult::Full),
        })
    })
}

#[test]
fn overlapping_saved_ticking_callbacks_keep_shared_world_and_tick_history() -> Result<()> {
    let (area, ids, registration) = overlapping_ticking_fixture()?;
    let mut batch = begin_ticking_batch(&area, &ids, &registration)?;
    let first = ChunkPos { x: 0, z: 0 };
    let second = ChunkPos { x: 1, z: 0 };
    let mut a = ready_future(first, &registration)?;
    let mut b = ready_future(second, &registration)?;
    assert!(batch.dispatch(&mut a, 500)?);
    assert!(batch.dispatch(&mut b, 500)?);
    let mut again = ready_future(first, &registration)?;
    assert!(batch.dispatch(&mut again, 500)?);
    assert_eq!(a.state(), TickingFutureState::Available);
    assert_eq!(b.state(), TickingFutureState::Available);
    assert_eq!(again.state(), TickingFutureState::Available);
    let result = batch.finish()?;
    assert_eq!(result.region.chunks, BTreeSet::from([first, second]));
    for pos in [first, second] {
        let ticks = result.block_ticks.container(pos).unwrap();
        assert_eq!(ticks.pack(500).len(), 3);
        assert_eq!(ticks.pack(1_000)[0].delay, ticks.pack(500)[0].delay - 500);
        assert_eq!(
            result.region.ticks().chunk(pos).unwrap().blocks.pack(500),
            ticks.pack(500)
        );
    }
    let neighbor = result
        .block_ticks
        .container(ChunkPos { x: -1, z: -1 })
        .unwrap();
    assert_eq!(neighbor.pack(500)[0].delay, 7);
    assert_eq!(neighbor.pack(1_000)[0].delay, 7);
    assert_eq!(result.region.ticks().next_sub_tick(), 50);
    Ok(())
}

#[test]
fn saved_ticking_batch_skips_cancelled_and_rejects_incomplete_neighbors() -> Result<()> {
    let (area, ids, registration) = ticking_fixture()?;
    let mut batch = begin_ticking_batch(&area, &ids, &registration)?;
    let center = ChunkPos { x: 0, z: 0 };
    let mut cancelled = ready_future(center, &registration)?;
    assert!(cancelled.cancel());
    assert!(batch.dispatch(&mut cancelled, 500)?);
    assert_eq!(cancelled.state(), TickingFutureState::Unavailable);
    let mut ready = ready_future(center, &registration)?;
    assert!(batch.dispatch(&mut ready, 500)?);
    let result = batch.finish()?;
    assert_eq!(result.region.chunks, BTreeSet::from([center]));

    let mut batch = begin_ticking_batch(&area, &ids, &registration)?;
    let source = native_persistence::nbt::write(&area.chunks[&area.center].document)?;
    let mut ready = ready_future(center, &registration)?;
    assert!(batch.dispatch(&mut ready, 500)?);
    let other = ChunkPos { x: 1, z: 0 };
    let claimed = ticking_dependencies(other);
    let mut future = ready_future(other, &claimed)?;
    assert!(batch.dispatch(&mut future, 500).is_err());
    assert_eq!(future.state(), TickingFutureState::Exceptional);
    assert!(batch.finish().is_err());
    assert_eq!(
        native_persistence::nbt::write(&area.chunks[&area.center].document)?,
        source
    );
    Ok(())
}

#[test]
fn saved_ticking_batch_rejects_level_time_changes_before_dispatch() -> Result<()> {
    let (area, ids, registration) = ticking_fixture()?;
    let mut batch = begin_ticking_batch(&area, &ids, &registration)?;
    let center = ChunkPos { x: 0, z: 0 };
    let source = native_persistence::nbt::write(&area.chunks[&area.center].document)?;
    let mut first = ready_future(center, &registration)?;
    assert!(batch.dispatch(&mut first, 500)?);
    let mut next = ready_future(center, &registration)?;
    assert!(batch.dispatch(&mut next, 501).is_err());
    assert_eq!(next.state(), TickingFutureState::Waiting);
    assert!(next.has_queued_callback());
    assert!(batch.finish().is_err());
    assert_eq!(
        native_persistence::nbt::write(&area.chunks[&area.center].document)?,
        source
    );
    Ok(())
}

fn dispatch_fixture(
    future: &mut TickingPreparation,
    area: &ChunkArea,
    ids: &BTreeSet<String>,
    registration: &[ChunkPos],
    game_time: i64,
) -> Result<Option<SavedTickingResult>> {
    let dimension = DimensionId::parse("overworld")?;
    let defaults = BTreeMap::new();
    dispatch_saved_ticking(
        future,
        &mut Tags,
        SavedPostprocessingInputs {
            area,
            dimension: &dimension,
            defaults: &defaults,
            block_ids: ids,
            bounds: BuildBounds {
                min_y: 0,
                max_y: 16,
            },
            registration,
            game_time,
            next_sub_tick: 2,
            light: None,
        },
        |state| {
            ensure!(
                ids.contains(&state.name) && state.properties.is_empty(),
                "unknown fixture state"
            );
            Ok(())
        },
        |_| Ok(()),
    )
}

fn completed_ticking_fixture() -> Result<(ChunkArea, SavedTickingResult)> {
    use crate::ticking_preparation::{FullFuture, FullFutureId, FullFutureResult};
    let (area, ids, registration) = ticking_fixture()?;
    let center = ChunkPos { x: 0, z: 0 };
    let mut future = TickingPreparation::start(center, |position| {
        Some(FullFuture {
            id: FullFutureId(registration.iter().position(|&pos| pos == position)? as u64),
            result: Some(FullFutureResult::Full),
        })
    })?;
    let result = dispatch_fixture(&mut future, &area, &ids, &registration, 500)?
        .context("saved ticking callback did not complete")?;
    Ok((area, result))
}

fn live_ticking_fixture() -> Result<(
    ChunkArea,
    BTreeSet<String>,
    Vec<ChunkPos>,
    crate::loaded::LoadedSession,
    Arc<native_world::loaded::World>,
    GenerationTerrainStage,
)> {
    use glam::{IVec3, Vec3};
    let (area, mut ids, registration) = ticking_fixture()?;
    ids.insert("minecraft:air".into());
    let dimension = DimensionId::parse("overworld")?;
    let source = Arc::new(area.materialize(&dimension)?);
    let bounds = BuildBounds {
        min_y: 0,
        max_y: 16,
    };
    let mut session = crate::loaded::LoadedSession::new(
        source.as_ref().clone(),
        IVec3::ZERO,
        bounds,
        crate::PlayerState {
            position: Vec3::new(8.5, 10.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: crate::PlayerPose::Standing,
        },
    )
    .map_err(anyhow::Error::msg)?;
    assert!(session.set_block_for_postprocessing_test(
        IVec3::new(8, 8, 8),
        BlockState {
            name: "minecraft:red_sand".into(),
            properties: BTreeMap::new(),
        },
    ));
    let terrain = session
        .capture_generation_terrain_stage(Arc::clone(&source), registration.iter().copied())?;
    Ok((area, ids, registration, session, source, terrain))
}

fn classify_live_fixture(
    state: &native_persistence::palette::BlockState,
) -> Result<native_persistence::heightmap::BlockHeightFlags> {
    ensure!(
        [
            "minecraft:air",
            "minecraft:stone",
            "minecraft:sand",
            "minecraft:red_sand",
            "minecraft:gravel"
        ]
        .iter()
        .any(|name| state.name.matches(name)),
        "unknown fixture heightmap state"
    );
    let is_air = state.name.matches("minecraft:air");
    Ok(native_persistence::heightmap::BlockHeightFlags {
        is_air,
        blocks_motion: !is_air,
        blocks_motion_no_leaves: !is_air,
        has_fluid: false,
    })
}

fn begin_live_fixture(
    area: &ChunkArea,
    ids: &BTreeSet<String>,
    registration: &[ChunkPos],
    terrain: &GenerationTerrainStage,
) -> Result<SavedTickingBatch> {
    SavedTickingBatch::begin_with_live_terrain(
        &mut Tags,
        SavedPostprocessingInputs {
            area,
            dimension: &DimensionId::parse("overworld")?,
            defaults: &BTreeMap::new(),
            block_ids: ids,
            bounds: BuildBounds {
                min_y: 0,
                max_y: 16,
            },
            registration,
            game_time: 100,
            next_sub_tick: 2,
            light: None,
        },
        terrain,
        |state| {
            ensure!(
                ids.contains(&state.name) && state.properties.is_empty(),
                "unknown fixture state"
            );
            Ok(())
        },
        |_| Ok(()),
    )
}

fn live_session_tick_fixture() -> Result<(
    ChunkArea,
    BTreeSet<String>,
    Vec<ChunkPos>,
    crate::loaded::LoadedSession,
    GenerationTerrainStage,
    SavedTickQueueCapture,
)> {
    let (area, ids, registration, mut session, _, terrain) = live_ticking_fixture()?;
    session
        .set_initial_game_time(100)
        .map_err(anyhow::Error::msg)?;
    session.require_saved_chunk_sources()?;
    ensure!(
        session.install_saved_tick_orders(&registration)? == (true, true),
        "fixture tick indexes were not installed"
    );
    session.schedule_sand_for_postprocessing_test(
        glam::IVec3::new(8, 8, 8),
        crate::falling_block::FallingKind::RedSand,
    );
    let ticks = session.capture_saved_tick_queues()?;
    Ok((area, ids, registration, session, terrain, ticks))
}

fn begin_live_session_fixture(
    area: &ChunkArea,
    ids: &BTreeSet<String>,
    terrain: &GenerationTerrainStage,
    ticks: &SavedTickQueueCapture,
) -> Result<SavedTickingBatch> {
    SavedTickingBatch::begin_with_live_session(
        &mut Tags,
        SavedPostprocessingInputs {
            area,
            dimension: &DimensionId::parse("overworld")?,
            defaults: &BTreeMap::new(),
            block_ids: ids,
            bounds: BuildBounds {
                min_y: 0,
                max_y: 16,
            },
            registration: ticks.registration_order(),
            game_time: ticks.game_time(),
            next_sub_tick: ticks.next_sub_tick(),
            light: None,
        },
        terrain,
        ticks,
        |state| {
            ensure!(
                ids.contains(&state.name) && state.properties.is_empty(),
                "unknown fixture state"
            );
            Ok(())
        },
        |_| Ok(()),
    )
}

#[test]
fn saved_ticking_preserves_live_schedules_and_detects_stale_queue_capture() -> Result<()> {
    let (area, ids, registration, mut session, terrain, capture) = live_session_tick_fixture()?;
    session.verify_saved_tick_queues(&capture)?;
    session.verify_generation_terrain_stage(&terrain)?;
    assert_eq!(capture.registration_order(), registration);
    assert_eq!(capture.next_sub_tick(), 1);
    let center = ChunkPos { x: 0, z: 0 };
    let mut batch = begin_live_session_fixture(&area, &ids, &terrain, &capture)?;
    assert!(batch.dispatch(&mut ready_future(center, &registration)?, 100)?);
    let result = batch.finish()?;
    let center_ticks = result.block_ticks.container(center).unwrap();
    let reds: Vec<_> = center_ticks
        .activated(100)
        .into_iter()
        .filter(|tick| tick.kind == "minecraft:red_sand" && tick.position == [8, 8, 8])
        .collect();
    assert_eq!(reds.len(), 1);
    assert_eq!((reds[0].trigger, reds[0].sub_tick), (102, 0));
    let neighbor = result.block_ticks.container(registration[0]).unwrap();
    assert_eq!(neighbor.pack(100)[0].delay, 7);
    assert_eq!(neighbor.pack(200)[0].delay, -93);
    session.verify_saved_tick_queues(&capture)?;

    // A duplicate scheduling request still consumes a sub-tick counter and
    // invalidates the capture, while the isolated result keeps the old queue.
    session.schedule_sand_for_postprocessing_test(
        glam::IVec3::new(8, 8, 8),
        crate::falling_block::FallingKind::RedSand,
    );
    assert!(session.verify_saved_tick_queues(&capture).is_err());
    assert_eq!(
        center_ticks
            .activated(100)
            .into_iter()
            .filter(|tick| { tick.kind == "minecraft:red_sand" && tick.position == [8, 8, 8] })
            .count(),
        1
    );

    let (_, _, _, _, other_terrain, other_capture) = live_session_tick_fixture()?;
    assert!(begin_live_session_fixture(&area, &ids, &other_terrain, &capture).is_err());
    assert!(session.verify_saved_tick_queues(&other_capture).is_err());
    Ok(())
}

#[test]
fn saved_ticking_rejects_marks_after_saved_ticks_activate() -> Result<()> {
    let (area, ids, _, mut session, terrain, _) = live_session_tick_fixture()?;
    session.activate_saved_ticks_for_postprocessing_test(ChunkPos { x: 0, z: 0 })?;
    let capture = session.capture_saved_tick_queues()?;
    let error = begin_live_session_fixture(&area, &ids, &terrain, &capture)
        .err()
        .context("activated ticks with unfinished postprocessing were accepted")?;
    assert!(error.to_string().contains("not run before ticks activated"));
    Ok(())
}

fn live_callback_fixture(
    mark_x: i32,
) -> Result<(
    tempfile::TempDir,
    ChunkArea,
    BTreeSet<String>,
    Vec<ChunkPos>,
    crate::loaded::LoadedSession,
    GenerationTerrainStage,
    SavedTickQueueCapture,
)> {
    use flate2::{Compression, write::GzEncoder};
    use glam::{IVec3, Vec3};
    use std::io::Write;

    let (mut area, mut ids, registration) = ticking_fixture()?;
    ids.insert("minecraft:air".into());
    let chunk = Arc::make_mut(area.chunks.get_mut(&area.center).unwrap());
    let Tag::Compound(fields) = &mut Arc::make_mut(&mut chunk.document).value else {
        unreachable!()
    };
    let mut marks = PostProcessing::new(0, 16).unwrap();
    assert!(marks.mark(mark_x, 8, 8));
    fields
        .iter_mut()
        .find(|(name, _)| name.matches("PostProcessing"))
        .unwrap()
        .1 = native_persistence::postprocessing::encode(&marks);
    let dimension = DimensionId::parse("overworld")?;
    let source = Arc::new(area.materialize(&dimension)?);
    let mut session = crate::loaded::LoadedSession::new(
        source.as_ref().clone(),
        IVec3::ZERO,
        BuildBounds {
            min_y: 0,
            max_y: 16,
        },
        crate::PlayerState {
            position: Vec3::new(8.5, 10.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: crate::PlayerPose::Standing,
        },
    )
    .map_err(anyhow::Error::msg)?;
    let root = tempfile::tempdir()?;
    let level = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![(
            "Data".into(),
            Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                ("Time".into(), Tag::Long(100)),
            ]),
        )]),
    };
    let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
    gzip.write_all(&native_persistence::nbt::write(&level)?)?;
    std::fs::write(root.path().join("level.dat"), gzip.finish()?)?;
    let reader = native_persistence::world::WorldReader::open_for_session(root.path(), dimension)?;
    let packs = reader.capture_world_data_packs()?;
    session.bind_saved_world_session(reader.session_guard().context("missing session lock")?)?;
    session.bind_world_data_packs(packs)?;
    session
        .set_initial_game_time(100)
        .map_err(anyhow::Error::msg)?;
    ensure!(
        session.install_saved_tick_orders(&registration)? == (true, true),
        "tick indexes not installed"
    );
    let terrain = session.capture_generation_terrain_stage(source, registration.iter().copied())?;
    let ticks = session.capture_saved_tick_queues()?;
    Ok((root, area, ids, registration, session, terrain, ticks))
}

fn begin_no_effect_fixture(
    area: &ChunkArea,
    ids: &BTreeSet<String>,
    terrain: &GenerationTerrainStage,
    ticks: &SavedTickQueueCapture,
) -> Result<SavedTickingBatch> {
    let mut batch = begin_live_session_fixture(area, ids, terrain, ticks)?;
    batch.stage.set_water_source_conversion(true)?;
    batch
        .stage
        .set_lava_rules(super::super::postprocessing_world::GeneratedLavaRules {
            fast_lava: false,
            source_conversion: false,
        })?;
    // This synthetic provider has no ResourceStack; production can only bind
    // the proof through begin_with_bound_live_session.
    ensure!(ticks.data_packs().is_some(), "fixture packs are missing");
    batch.live.as_mut().unwrap().packs_bound = true;
    Ok(batch)
}

#[test]
#[cfg(target_os = "windows")]
fn live_no_effect_callback_activates_only_the_original_holder() -> Result<()> {
    use crate::ticking_preparation::TickingFutureState;
    let (_root, area, ids, registration, mut session, terrain, ticks) = live_callback_fixture(7)?;
    let center = ChunkPos { x: 0, z: 0 };
    let mut original = ready_future(center, &registration)?;
    let mut worker = original.fork_queued_callback()?;
    let mut batch = begin_no_effect_fixture(&area, &ids, &terrain, &ticks)?;
    assert!(batch.dispatch(&mut worker, 100)?);
    let result = batch.finish()?;
    assert_eq!(original.state(), TickingFutureState::Waiting);
    let mut unrelated = ready_future(center, &registration)?;
    assert!(
        session
            .apply_no_effect_saved_ticking(&mut unrelated, &terrain, &result)
            .is_err()
    );
    assert_eq!(original.state(), TickingFutureState::Waiting);
    session.apply_no_effect_saved_ticking(&mut original, &terrain, &result)?;
    assert_eq!(original.state(), TickingFutureState::Available);
    assert!(session.verify_saved_tick_queues(&ticks).is_err());
    let after = session
        .capture_saved_tick_queues()?
        .isolated_region_ticks()?;
    assert!(
        after
            .chunk(center)
            .unwrap()
            .blocks
            .pending_saved()
            .is_none()
    );
    assert!(session.dirty_chunk_positions().contains(&center));
    assert!(
        session
            .draft_block_ticks(center)
            .unwrap_err()
            .to_string()
            .contains("postprocessing")
    );
    assert!(
        session
            .apply_no_effect_saved_ticking(&mut original, &terrain, &result)
            .is_err()
    );
    Ok(())
}

#[test]
#[cfg(target_os = "windows")]
fn live_no_effect_callback_survives_tick_and_unrelated_queue_changes() -> Result<()> {
    use crate::{GameSession, ticking_preparation::TickingFutureState};
    use std::time::Duration;

    for elapsed in [vec![150], vec![5, 10, 35, 100]] {
        let (_root, area, ids, registration, mut session, terrain, ticks) =
            live_callback_fixture(7)?;
        let center = ChunkPos { x: 0, z: 0 };
        let neighbor = ChunkPos { x: 1, z: 0 };
        let mut original = ready_future(center, &registration)?;
        let mut worker = original.fork_queued_callback()?;
        let mut batch = begin_no_effect_fixture(&area, &ids, &terrain, &ticks)?;
        assert!(batch.dispatch(&mut worker, 100)?);
        let result = batch.finish()?;
        for millis in elapsed {
            session.update(Duration::from_millis(millis));
        }
        assert_eq!(session.snapshot().game_time, 103);
        session.schedule_sand_for_postprocessing_test(
            glam::IVec3::new(25, 8, 8),
            crate::falling_block::FallingKind::RedSand,
        );
        session.schedule_sand_for_postprocessing_test(
            glam::IVec3::new(9, 8, 8),
            crate::falling_block::FallingKind::RedSand,
        );
        assert!(session.verify_saved_tick_queues(&ticks).is_err());
        session.apply_no_effect_saved_ticking(&mut original, &terrain, &result)?;
        assert_eq!(original.state(), TickingFutureState::Available);
        let current = session.capture_saved_tick_queues()?;
        assert_eq!(current.game_time(), 103);
        assert_eq!(current.next_sub_tick(), 2);
        let region = current.isolated_region_ticks()?;
        let center_ticks = region.chunk(center).unwrap().blocks.activated(103);
        assert!(center_ticks.iter().any(|tick| {
            tick.kind == "minecraft:sand"
                && tick.position == [8, 8, 8]
                && tick.trigger == 103
                && tick.sub_tick == -1
        }));
        assert!(center_ticks.iter().any(|tick| {
            tick.kind == "minecraft:red_sand"
                && tick.position == [9, 8, 8]
                && tick.trigger == 105
                && tick.sub_tick == 1
        }));
        let neighbor_ticks = region.chunk(neighbor).unwrap().blocks.activated(103);
        assert!(neighbor_ticks.iter().any(|tick| {
            tick.kind == "minecraft:red_sand"
                && tick.position == [25, 8, 8]
                && tick.trigger == 105
                && tick.sub_tick == 0
        }));
    }
    Ok(())
}

#[test]
#[cfg(target_os = "windows")]
fn live_no_effect_callback_rejects_ticks_activated_while_working() -> Result<()> {
    let (_root, area, ids, registration, mut session, terrain, ticks) = live_callback_fixture(7)?;
    let center = ChunkPos { x: 0, z: 0 };
    let mut original = ready_future(center, &registration)?;
    let mut worker = original.fork_queued_callback()?;
    let mut batch = begin_no_effect_fixture(&area, &ids, &terrain, &ticks)?;
    assert!(batch.dispatch(&mut worker, 100)?);
    let result = batch.finish()?;
    session.activate_saved_ticks_for_postprocessing_test(center)?;
    assert!(
        session
            .apply_no_effect_saved_ticking(&mut original, &terrain, &result)
            .is_err()
    );
    assert_eq!(original.state(), TickingFutureState::Waiting);
    Ok(())
}

#[test]
#[cfg(target_os = "windows")]
fn live_no_effect_callback_rejects_modified_result_and_changed_terrain() -> Result<()> {
    use super::super::chunk_ticks::ScheduledTick;
    use crate::ticking_preparation::TickingFutureState;
    let (_root, area, ids, registration, mut session, terrain, ticks) = live_callback_fixture(7)?;
    let center = ChunkPos { x: 0, z: 0 };
    let mut original = ready_future(center, &registration)?;
    let mut worker = original.fork_queued_callback()?;
    let mut batch = begin_no_effect_fixture(&area, &ids, &terrain, &ticks)?;
    assert!(batch.dispatch(&mut worker, 100)?);
    let mut result = batch.finish()?;
    Arc::make_mut(&mut result.block_ticks).schedule(ScheduledTick {
        kind: "minecraft:gravel".into(),
        position: [1, 8, 1],
        trigger: 102,
        priority: 0,
        sub_tick: 0,
    })?;
    assert!(
        session
            .apply_no_effect_saved_ticking(&mut original, &terrain, &result)
            .is_err()
    );
    assert_eq!(original.state(), TickingFutureState::Waiting);
    session.verify_saved_tick_queues(&ticks)?;
    let mut worker = original.fork_queued_callback()?;
    let mut batch = begin_no_effect_fixture(&area, &ids, &terrain, &ticks)?;
    assert!(batch.dispatch(&mut worker, 100)?);
    let result = batch.finish()?;
    assert!(session.set_block_for_postprocessing_test(
        glam::IVec3::new(7, 8, 8),
        BlockState {
            name: "minecraft:red_sand".into(),
            properties: BTreeMap::new(),
        }
    ));
    assert!(
        session
            .apply_no_effect_saved_ticking(&mut original, &terrain, &result)
            .is_err()
    );
    assert_eq!(original.state(), TickingFutureState::Waiting);
    Ok(())
}

#[test]
#[cfg(target_os = "windows")]
fn live_callback_rejects_new_scheduling_until_it_is_reconciled() -> Result<()> {
    let (_root, area, ids, registration, mut session, terrain, ticks) = live_callback_fixture(8)?;
    let center = ChunkPos { x: 0, z: 0 };
    let mut original = ready_future(center, &registration)?;
    let mut worker = original.fork_queued_callback()?;
    let mut batch = begin_no_effect_fixture(&area, &ids, &terrain, &ticks)?;
    assert!(batch.dispatch(&mut worker, 100)?);
    let result = batch.finish()?;
    assert!(result.region.ticks().next_sub_tick() > ticks.next_sub_tick());
    assert!(
        session
            .apply_no_effect_saved_ticking(&mut original, &terrain, &result)
            .is_err()
    );
    assert_eq!(original.state(), TickingFutureState::Waiting);
    session.verify_saved_tick_queues(&ticks)?;
    Ok(())
}

#[test]
#[cfg(target_os = "windows")]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn live_no_effect_callback_matches_java_saved_ticks() -> Result<()> {
    let (_root, area, ids, registration, mut session, terrain, ticks) = live_callback_fixture(7)?;
    let directory = tempfile::tempdir()?;
    let input = directory.path().join("chunk.nbt");
    let output = directory.path().join("postprocessed.nbt");
    std::fs::write(
        &input,
        native_persistence::nbt::write(&area.chunks[&area.center].document)?,
    )?;
    let java = super::super::postprocessing_world::tests::java_results_with_args(
        "SavedFullPostprocessingOracle",
        include_str!("../../tests/java/SavedFullPostprocessingOracle.java"),
        &[
            input.to_str().context("invalid NBT input path")?,
            output.to_str().context("invalid NBT output path")?,
            "unpack",
        ],
    );
    let center = ChunkPos { x: 0, z: 0 };
    let mut original = ready_future(center, &registration)?;
    let mut worker = original.fork_queued_callback()?;
    let mut batch = begin_no_effect_fixture(&area, &ids, &terrain, &ticks)?;
    assert!(batch.dispatch(&mut worker, 100)?);
    let result = batch.finish()?;
    let mut actual = result
        .block_ticks
        .container(center)
        .unwrap()
        .pack(100)
        .into_iter()
        .map(|tick| {
            format!(
                "{} {},{},{} {} {}",
                tick.kind,
                tick.position[0],
                tick.position[1],
                tick.position[2],
                tick.delay,
                tick.priority
            )
        })
        .collect::<Vec<_>>();
    actual.push(format!("counter {}", result.region.ticks().next_sub_tick()));
    assert_eq!(actual, java);
    session.apply_no_effect_saved_ticking(&mut original, &terrain, &result)?;
    assert_eq!(original.state(), TickingFutureState::Available);
    Ok(())
}

#[test]
#[cfg(target_os = "windows")]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn delayed_live_no_effect_callback_matches_java_saved_ticks() -> Result<()> {
    use crate::GameSession;
    use std::time::Duration;

    let (_root, area, ids, registration, mut session, terrain, ticks) = live_callback_fixture(7)?;
    let directory = tempfile::tempdir()?;
    let input = directory.path().join("chunk.nbt");
    let output = directory.path().join("postprocessed.nbt");
    std::fs::write(
        &input,
        native_persistence::nbt::write(&area.chunks[&area.center].document)?,
    )?;
    let java = super::super::postprocessing_world::tests::java_results_with_args(
        "SavedFullPostprocessingOracle",
        include_str!("../../tests/java/SavedFullPostprocessingOracle.java"),
        &[
            input.to_str().context("invalid NBT input path")?,
            output.to_str().context("invalid NBT output path")?,
            "unpack-with-live-tick",
            "103",
            "1",
        ],
    );
    let center = ChunkPos { x: 0, z: 0 };
    let mut original = ready_future(center, &registration)?;
    let mut worker = original.fork_queued_callback()?;
    let mut batch = begin_no_effect_fixture(&area, &ids, &terrain, &ticks)?;
    assert!(batch.dispatch(&mut worker, 100)?);
    let result = batch.finish()?;
    session.update(Duration::from_millis(150));
    session.schedule_sand_for_postprocessing_test(
        glam::IVec3::new(25, 8, 8),
        crate::falling_block::FallingKind::RedSand,
    );
    session.schedule_sand_for_postprocessing_test(
        glam::IVec3::new(9, 8, 8),
        crate::falling_block::FallingKind::RedSand,
    );
    session.apply_no_effect_saved_ticking(&mut original, &terrain, &result)?;
    let current = session.capture_saved_tick_queues()?;
    let region = current.isolated_region_ticks()?;
    let mut actual = region
        .chunk(center)
        .unwrap()
        .blocks
        .pack(103)
        .into_iter()
        .map(|tick| {
            format!(
                "{} {},{},{} {} {}",
                tick.kind,
                tick.position[0],
                tick.position[1],
                tick.position[2],
                tick.delay,
                tick.priority
            )
        })
        .collect::<Vec<_>>();
    actual.push(format!("counter {}", current.next_sub_tick()));
    assert_eq!(actual, java);
    Ok(())
}

#[test]
#[cfg(target_os = "windows")]
#[ignore = "requires installed Minecraft Java Edition 26.3"]
fn live_callback_binds_the_actual_session_data_packs() -> Result<()> {
    let (root, area, _, registration, mut session, terrain, ticks) = live_callback_fixture(7)?;
    let game =
        native_resources::default_minecraft_dir().context("Minecraft directory unavailable")?;
    let packs = ticks.data_packs().context("fixture packs missing")?;
    let selection = packs.selection();
    let settings = native_resources::datapacks::DataPackSettings {
        enabled: selection.enabled.clone(),
        disabled: selection.disabled.clone(),
        enabled_features: selection.enabled_features.clone(),
    };
    let mut provider = native_resources::packs::ResourceStack::open(&game)?;
    let ids = provider.vanilla_block_ids()?;
    let registry = provider.vanilla_generated_block_registry()?;
    let defaults = registry.default_states();
    let begin = |provider: &mut native_resources::packs::ResourceStack| {
        SavedTickingBatch::begin_with_bound_live_session(
            provider,
            SavedPostprocessingInputs {
                area: &area,
                dimension: &DimensionId::parse("overworld")?,
                defaults: &defaults,
                block_ids: &ids,
                bounds: BuildBounds {
                    min_y: 0,
                    max_y: 16,
                },
                registration: ticks.registration_order(),
                game_time: ticks.game_time(),
                next_sub_tick: ticks.next_sub_tick(),
                light: None,
            },
            &terrain,
            &ticks,
            |state| {
                ensure!(
                    registry.is_canonical_state(&state.name, &state.properties),
                    "unknown block state"
                );
                Ok(())
            },
            true,
            super::super::postprocessing_world::GeneratedLavaRules {
                fast_lava: false,
                source_conversion: false,
            },
        )
    };
    let other =
        Arc::new(native_resources::datapack_snapshot::DataPackSnapshot::capture(root.path())?);
    assert!(
        provider
            .configure_snapshot_data(other, &settings)?
            .is_empty()
    );
    assert!(begin(&mut provider).is_err());
    let mut different_selection = settings.clone();
    different_selection
        .disabled
        .push("file/disabled.zip".into());
    provider.configure_snapshot_data(Arc::clone(packs.packs()), &different_selection)?;
    assert!(begin(&mut provider).is_err());
    assert!(
        provider
            .configure_snapshot_data(Arc::clone(packs.packs()), &settings)?
            .is_empty()
    );
    let mut batch = begin(&mut provider)?;
    let center = ChunkPos { x: 0, z: 0 };
    let mut original = ready_future(center, &registration)?;
    let mut worker = original.fork_queued_callback()?;
    assert!(batch.dispatch(&mut worker, 100)?);
    let result = batch.finish()?;
    session.apply_no_effect_saved_ticking(&mut original, &terrain, &result)?;
    assert_eq!(original.state(), TickingFutureState::Available);
    Ok(())
}

#[test]
#[cfg(target_os = "windows")]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn saved_ticking_with_live_schedules_matches_java() -> Result<()> {
    let (area, ids, registration, session, terrain, capture) = live_session_tick_fixture()?;
    session.verify_saved_tick_queues(&capture)?;
    session.verify_generation_terrain_stage(&terrain)?;
    let center = ChunkPos { x: 0, z: 0 };
    let draft = terrain
        .saved_edited_chunk_draft(center, &area.chunks[&area.center], classify_live_fixture)?
        .context("live edit draft is missing")?;
    let directory = tempfile::tempdir()?;
    let input = directory.path().join("live.nbt");
    let output = directory.path().join("postprocessed.nbt");
    std::fs::write(&input, native_persistence::nbt::write(draft.document())?)?;
    let expected = super::super::postprocessing_world::tests::java_results_with_args(
        "SavedFullPostprocessingOracle",
        include_str!("../../tests/java/SavedFullPostprocessingOracle.java"),
        &[
            input.to_str().context("invalid NBT input path")?,
            output.to_str().context("invalid NBT output path")?,
            "live-tick",
        ],
    );
    let mut batch = begin_live_session_fixture(&area, &ids, &terrain, &capture)?;
    assert!(batch.dispatch(&mut ready_future(center, &registration)?, 100)?);
    let result = batch.finish()?;
    let mut actual = result
        .block_ticks
        .container(center)
        .unwrap()
        .pack(100)
        .into_iter()
        .map(|tick| {
            format!(
                "{} {},{},{} {} {}",
                tick.kind,
                tick.position[0],
                tick.position[1],
                tick.position[2],
                tick.delay,
                tick.priority
            )
        })
        .collect::<Vec<_>>();
    actual.push(format!("counter {}", result.region.ticks().next_sub_tick()));
    assert_eq!(actual, expected);
    Ok(())
}

#[test]
fn saved_ticking_uses_live_blocks_and_rejects_stale_terrain() -> Result<()> {
    use crate::ticking_preparation::{FullFuture, FullFutureId, FullFutureResult};
    let (area, ids, registration, mut session, source, terrain) = live_ticking_fixture()?;
    let center = ChunkPos { x: 0, z: 0 };
    session.verify_generation_terrain_stage(&terrain)?;
    let mut future = TickingPreparation::start(center, |position| {
        Some(FullFuture {
            id: FullFutureId(registration.iter().position(|&pos| pos == position)? as u64),
            result: Some(FullFutureResult::Full),
        })
    })?;
    let mut batch = begin_live_fixture(&area, &ids, &registration, &terrain)?;
    assert!(batch.dispatch(&mut future, 100)?);
    let result = batch.finish()?;
    assert_eq!(source.block(8, 8, 8).unwrap().name, "minecraft:sand");
    assert_eq!(
        result.region.world.block(8, 8, 8).unwrap().name,
        "minecraft:red_sand"
    );
    let ticks = result.block_ticks.container(center).unwrap().pack(100);
    assert!(ticks.iter().any(|tick| tick.kind == "minecraft:sand"
        && tick.position == [8, 8, 8]
        && tick.delay == 0));
    assert!(ticks.iter().any(|tick| tick.kind == "minecraft:red_sand"
        && tick.position == [8, 8, 8]
        && tick.delay == 2));
    assert_eq!(
        native_persistence::nbt::fingerprint(&area.chunks[&area.center].document)?,
        source.saved_chunk_source(center).unwrap()
    );
    assert!(
        terrain
            .saved_postprocessing_world(
                &source,
                BuildBounds {
                    min_y: 0,
                    max_y: 16,
                },
                |state| {
                    ensure!(state.name != "minecraft:red_sand", "unsupported live state");
                    Ok(())
                },
            )
            .is_err()
    );

    struct NoMotionTags;
    impl ResourceProvider for NoMotionTags {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok((path.ends_with("blocks_motion_in_heightmap.json")
                || path.ends_with("blocks_motion_in_heightmap_no_leaves.json"))
            .then(|| br#"{"values":[]}"#.to_vec()))
        }
    }
    let incompatible = SavedTickingBatch::begin_with_live_terrain(
        &mut NoMotionTags,
        SavedPostprocessingInputs {
            area: &area,
            dimension: &DimensionId::parse("overworld")?,
            defaults: &BTreeMap::new(),
            block_ids: &ids,
            bounds: BuildBounds {
                min_y: 0,
                max_y: 16,
            },
            registration: &registration,
            game_time: 100,
            next_sub_tick: 2,
            light: None,
        },
        &terrain,
        |state| {
            ensure!(ids.contains(&state.name), "unknown fixture state");
            Ok(())
        },
        |_| Ok(()),
    );
    let Err(error) = incompatible else {
        panic!("incompatible heightmap tags were accepted");
    };
    assert!(format!("{error:#}").contains("does not match source blocks"));

    let mut changed = ChunkArea {
        center: area.center,
        radius: area.radius,
        chunks: area.chunks.clone(),
        missing: area.missing.clone(),
    };
    let neighbor = SavedChunkPos { x: -1, z: -1 };
    let chunk = Arc::make_mut(changed.chunks.get_mut(&neighbor).unwrap());
    let Tag::Compound(fields) = &mut Arc::make_mut(&mut chunk.document).value else {
        unreachable!()
    };
    fields.push(("changed".into(), Tag::Byte(1)));
    assert!(begin_live_fixture(&changed, &ids, &registration, &terrain).is_err());

    let mut forged = source.as_ref().clone();
    let red = forged.intern_state(BlockState {
        name: "minecraft:red_sand".into(),
        properties: BTreeMap::new(),
    });
    let chunk = Arc::make_mut(forged.chunks.get_mut(&center).unwrap());
    Arc::make_mut(chunk.sections.get_mut(&0).unwrap())
        .blocks
        .as_mut()
        .unwrap()[0] = red;
    assert_eq!(
        forged.saved_chunk_source(center),
        source.saved_chunk_source(center)
    );
    assert!(
        terrain
            .saved_postprocessing_world(
                &forged,
                BuildBounds {
                    min_y: 0,
                    max_y: 16,
                },
                |_| Ok(()),
            )
            .is_err()
    );

    assert!(session.set_block_for_postprocessing_test(
        glam::IVec3::new(7, 8, 8),
        BlockState {
            name: "minecraft:red_sand".into(),
            properties: BTreeMap::new()
        },
    ));
    assert!(session.verify_generation_terrain_stage(&terrain).is_err());
    Ok(())
}

#[test]
#[cfg(target_os = "windows")]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn saved_postprocessing_with_live_blocks_matches_java() -> Result<()> {
    let (area, ids, registration, session, _, terrain) = live_ticking_fixture()?;
    session.verify_generation_terrain_stage(&terrain)?;
    let center = ChunkPos { x: 0, z: 0 };
    let source = &area.chunks[&area.center];
    let draft = terrain
        .saved_edited_chunk_draft(center, source, classify_live_fixture)?
        .unwrap();
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("live.nbt");
    std::fs::write(&path, native_persistence::nbt::write(draft.document())?)?;
    let expected = super::super::postprocessing_world::tests::java_results_with_args(
        "SavedFullPostprocessingOracle",
        include_str!("../../tests/java/SavedFullPostprocessingOracle.java"),
        &[path.to_str().context("invalid NBT path")?],
    );
    let dimension = DimensionId::parse("overworld")?;
    let mut stage = begin_saved_postprocessing_with_live_terrain(
        &mut Tags,
        SavedPostprocessingInputs {
            area: &area,
            dimension: &dimension,
            defaults: &BTreeMap::new(),
            block_ids: &ids,
            bounds: BuildBounds {
                min_y: 0,
                max_y: 16,
            },
            registration: &registration,
            game_time: 100,
            next_sub_tick: 2,
            light: None,
        },
        &terrain,
        |state| {
            ensure!(
                ids.contains(&state.name) && state.properties.is_empty(),
                "unknown fixture state"
            );
            Ok(())
        },
    )?;
    stage.select_initial_targets(BTreeSet::from([center]))?;
    while !stage.poll(center, 256)? {}
    let result = stage.finish()?;
    let mut actual = result
        .ticks()
        .chunk(center)
        .unwrap()
        .blocks
        .pack(100)
        .into_iter()
        .map(|tick| {
            format!(
                "{} {},{},{} {} {}",
                tick.kind,
                tick.position[0],
                tick.position[1],
                tick.position[2],
                tick.delay,
                tick.priority
            )
        })
        .collect::<Vec<_>>();
    actual.push(format!("counter {}", result.ticks().next_sub_tick()));
    assert_eq!(actual, expected);
    Ok(())
}

#[test]
fn completed_saved_marks_draft_checks_sources_and_keeps_other_chunks_untouched() -> Result<()> {
    let (area, mut result) = completed_ticking_fixture()?;
    let center = ChunkPos { x: 0, z: 0 };
    let neighbor = ChunkPos { x: -1, z: -1 };
    let original = native_persistence::nbt::write(&area.chunks[&area.center].document)?;
    let drafts = result.draft_completed_postprocessing_marks(&area, [center])?;
    let draft = &drafts[&center];
    let empty = PostProcessing::new(0, 16).unwrap();
    assert_eq!(
        draft.document().value.get("PostProcessing"),
        Some(&native_persistence::postprocessing::encode(&empty))
    );
    for name in ["sections", "Heightmaps", "block_ticks", "fluid_ticks"] {
        assert_eq!(
            draft.document().value.get(name),
            area.chunks[&area.center].document.value.get(name)
        );
    }
    assert!(draft.matches_source(&area.chunks[&area.center].document)?);
    assert_eq!(
        native_persistence::nbt::write(&area.chunks[&area.center].document)?,
        original
    );
    assert!(
        result
            .draft_completed_postprocessing_marks(&area, [neighbor])
            .is_err()
    );
    assert!(
        result
            .draft_completed_postprocessing_marks(&area, [center, center])
            .is_err()
    );
    assert!(
        result
            .draft_completed_postprocessing_marks(&area, [])
            .is_err()
    );

    let mut changed = ChunkArea {
        center: area.center,
        radius: area.radius,
        chunks: area.chunks.clone(),
        missing: area.missing.clone(),
    };
    let position = SavedChunkPos {
        x: neighbor.x,
        z: neighbor.z,
    };
    let chunk = Arc::make_mut(changed.chunks.get_mut(&position).unwrap());
    let Tag::Compound(fields) = &mut Arc::make_mut(&mut chunk.document).value else {
        unreachable!()
    };
    fields.push(("unrelated_but_changed".into(), Tag::Int(1)));
    assert!(
        result
            .draft_completed_postprocessing_marks(&changed, [center])
            .is_err()
    );
    changed.chunks.remove(&position);
    assert!(
        result
            .draft_completed_postprocessing_marks(&changed, [center])
            .is_err()
    );

    result.region.chunks.insert(neighbor);
    assert!(
        result
            .draft_completed_postprocessing_marks(&area, [center])
            .is_err()
    );
    result.region.chunks.remove(&neighbor);
    result
        .region
        .light_checks
        .push(native_world::loaded::BlockPosition { x: 0, y: 8, z: 0 });
    assert!(
        result
            .draft_completed_postprocessing_marks(&area, [center])
            .is_err()
    );
    Ok(())
}

#[test]
#[cfg(target_os = "windows")]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn completed_saved_marks_draft_matches_java_serialization() -> Result<()> {
    let (area, result) = completed_ticking_fixture()?;
    let directory = tempfile::tempdir()?;
    let input = directory.path().join("before.nbt");
    let output = directory.path().join("after.nbt");
    std::fs::write(
        &input,
        native_persistence::nbt::write(&area.chunks[&area.center].document)?,
    )?;
    super::super::postprocessing_world::tests::java_results_with_args(
        "SavedFullPostprocessingOracle",
        include_str!("../../tests/java/SavedFullPostprocessingOracle.java"),
        &[
            input.to_str().context("invalid input path")?,
            output.to_str().context("invalid output path")?,
        ],
    );
    let java = native_persistence::nbt::read(&std::fs::read(output)?)?;
    let center = ChunkPos { x: 0, z: 0 };
    let drafts = result.draft_completed_postprocessing_marks(&area, [center])?;
    assert_eq!(
        drafts[&center].document().value.get("PostProcessing"),
        java.value.get("PostProcessing")
    );
    Ok(())
}

#[test]
fn saved_ticking_dispatch_postprocesses_then_unpacks_only_the_ready_center() -> Result<()> {
    use crate::ticking_preparation::{FullFuture, FullFutureId, FullFutureResult};
    let (area, ids, registration) = ticking_fixture()?;
    let center = ChunkPos { x: 0, z: 0 };
    let mut future = TickingPreparation::start(center, |position| {
        Some(FullFuture {
            id: FullFutureId(
                registration
                    .iter()
                    .position(|&entry| entry == position)
                    .unwrap() as u64,
            ),
            result: None,
        })
    })?;
    for index in 0..8 {
        future.complete(FullFutureId(index), FullFutureResult::Full)?;
    }
    assert!(dispatch_fixture(&mut future, &area, &ids, &registration, 100)?.is_none());
    future.complete(FullFutureId(8), FullFutureResult::Full)?;
    let completed = dispatch_fixture(&mut future, &area, &ids, &registration, 500)?
        .context("queued ticking future was not dispatched")?;
    assert_eq!(future.state(), TickingFutureState::Available);
    assert_eq!(completed.region.chunks, BTreeSet::from([center]));
    assert_eq!(completed.region.deferred_chunks().count(), 8);
    let ticks = completed.block_ticks.container(center).unwrap().pack(500);
    assert_eq!(
        ticks
            .iter()
            .map(|tick| (tick.kind.as_str(), tick.delay))
            .collect::<Vec<_>>(),
        [
            ("minecraft:sand", 0),
            ("minecraft:red_sand", 2),
            ("minecraft:gravel", 2)
        ]
    );
    let stored = &completed.region.ticks().chunk(center).unwrap().blocks;
    assert_eq!(stored.pack(500), ticks);
    assert_eq!(stored.pack(1_000)[0].delay, -500);
    let neighbor = completed.block_ticks.container(registration[0]).unwrap();
    assert_eq!(neighbor.pack(500)[0].delay, 7);
    assert_eq!(neighbor.pack(1_000)[0].delay, 7);
    assert!(dispatch_fixture(&mut future, &area, &ids, &registration, 501)?.is_none());
    Ok(())
}

#[test]
fn saved_ticking_dispatch_rejects_missing_inputs_and_skips_cancelled_work() -> Result<()> {
    use crate::ticking_preparation::{FullFuture, FullFutureId, FullFutureResult};
    let (area, ids) = fixture()?;
    let center = ChunkPos { x: 0, z: 0 };
    let registration = crate::ticking_preparation::ticking_dependencies(center);
    let start = || {
        TickingPreparation::start(center, |position| {
            Some(FullFuture {
                id: FullFutureId(
                    registration
                        .iter()
                        .position(|&entry| entry == position)
                        .unwrap() as u64,
                ),
                result: Some(FullFutureResult::Full),
            })
        })
    };
    let original = native_persistence::nbt::write(&area.chunks[&area.center].document)?;
    let mut missing = start()?;
    assert!(dispatch_fixture(&mut missing, &area, &ids, &[center], 100).is_err());
    assert_eq!(missing.state(), TickingFutureState::Exceptional);
    let mut cancelled = start()?;
    assert!(cancelled.cancel());
    assert!(dispatch_fixture(&mut cancelled, &area, &ids, &[center], 100)?.is_none());
    assert_eq!(cancelled.state(), TickingFutureState::Unavailable);
    assert_eq!(
        native_persistence::nbt::write(&area.chunks[&area.center].document)?,
        original
    );
    Ok(())
}

#[test]
fn saved_ticking_shared_unavailable_future_leaves_the_saved_region_untouched() -> Result<()> {
    use crate::ticking_preparation::{FullFuture, FullFutureId, FullFutureResult};

    let (area, ids, registration) = ticking_fixture()?;
    let originals = area
        .chunks
        .iter()
        .map(|(&pos, chunk)| Ok((pos, native_persistence::nbt::write(&chunk.document)?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let center = ChunkPos { x: 0, z: 0 };
    let mut future = TickingPreparation::start(center, |position| {
        let index = registration
            .iter()
            .position(|&pos| pos == position)
            .unwrap();
        Some(FullFuture {
            id: FullFutureId(if index == 7 { 1 } else { index } as u64),
            result: (index != 1 && index != 7).then_some(FullFutureResult::Full),
        })
    })?;
    let mut batch = begin_ticking_batch(&area, &ids, &registration)?;
    assert!(!batch.dispatch(&mut future, 500)?);
    assert!(future.complete(FullFutureId(1), FullFutureResult::Unavailable)?);
    assert!(batch.dispatch(&mut future, 500)?);
    assert_eq!(future.state(), TickingFutureState::Unavailable);
    assert!(batch.finish().is_err());
    for (pos, original) in originals {
        assert_eq!(
            native_persistence::nbt::write(&area.chunks[&pos].document)?,
            original
        );
    }
    Ok(())
}

#[test]
fn saved_ticking_dispatch_runs_from_the_shared_future_queue() -> Result<()> {
    use crate::ticking_preparation::{
        FullFuture, FullFutureId, FullFutureResult, TickingPreparations, TickingRequestId,
    };
    let (area, ids, registration) = ticking_fixture()?;
    let center = ChunkPos { x: 0, z: 0 };
    let request = TickingRequestId(41);
    let mut futures = TickingPreparations::default();
    futures.start(request, center, |position| {
        Some(FullFuture {
            id: FullFutureId(
                registration
                    .iter()
                    .position(|&entry| entry == position)
                    .unwrap() as u64,
            ),
            result: None,
        })
    })?;
    for index in 0..8 {
        futures.complete(FullFutureId(index), FullFutureResult::Full)?;
    }
    assert_eq!(futures.queued_len(), 0);
    futures.complete(FullFutureId(8), FullFutureResult::Full)?;
    let (dispatched, result) = futures
        .dispatch_next(|future| dispatch_fixture(future, &area, &ids, &registration, 500))?
        .context("ticking callback was not queued")?;
    assert_eq!(dispatched, request);
    assert_eq!(
        result
            .context("ticking callback did not produce a region")?
            .region
            .chunks,
        BTreeSet::from([center])
    );
    assert_eq!(futures.state(request), Some(TickingFutureState::Available));
    assert!(futures.retire(request)?);
    for index in 0..9 {
        assert!(futures.forget_full_future(FullFutureId(index))?);
    }
    Ok(())
}

#[test]
#[cfg(target_os = "windows")]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn saved_ticking_dispatch_matches_java_full_future_and_unpacked_ticks() -> Result<()> {
    use crate::ticking_preparation::{FullFuture, FullFutureId, FullFutureResult};
    let (area, ids, registration) = ticking_fixture()?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("chunk.nbt");
    std::fs::write(
        &path,
        native_persistence::nbt::write(&area.chunks[&area.center].document)?,
    )?;
    let java = super::super::postprocessing_world::tests::java_results_with_args(
        "SavedTickingDispatchOracle",
        include_str!("../../tests/java/SavedTickingDispatchOracle.java"),
        &[path.to_str().context("invalid NBT path")?],
    );
    let center = ChunkPos { x: 0, z: 0 };
    let mut future = TickingPreparation::start(center, |position| {
        Some(FullFuture {
            id: FullFutureId(
                registration
                    .iter()
                    .position(|&entry| entry == position)
                    .unwrap() as u64,
            ),
            result: None,
        })
    })?;
    for index in 0..8 {
        future.complete(FullFutureId(index), FullFutureResult::Full)?;
    }
    assert!(dispatch_fixture(&mut future, &area, &ids, &registration, 100)?.is_none());
    let saved_count = match area.chunks[&area.center].document.value.get("block_ticks") {
        Some(Tag::List { values, .. }) => values.len(),
        _ => unreachable!(),
    };
    let neighbor = area.chunks[&SavedChunkPos { x: -1, z: -1 }]
        .document
        .value
        .get("block_ticks")
        .unwrap();
    let Tag::List { values, .. } = neighbor else {
        unreachable!()
    };
    let Some(Tag::Int(neighbor_delay)) = values[0].get("t") else {
        unreachable!()
    };
    let mut actual = vec![format!(
        "before {} {saved_count} {neighbor_delay}",
        future.state() != TickingFutureState::Waiting
    )];
    future.complete(FullFutureId(8), FullFutureResult::Full)?;
    actual.push(format!(
        "queued {} {saved_count} {}",
        future.state() != TickingFutureState::Waiting,
        usize::from(future.has_queued_callback())
    ));
    let result = dispatch_fixture(&mut future, &area, &ids, &registration, 500)?.unwrap();
    actual.push(format!(
        "success {}",
        future.state() == TickingFutureState::Available
    ));
    for tick in result.block_ticks.container(center).unwrap().pack(500) {
        actual.push(format!(
            "{} {},{},{} {} {}",
            tick.kind,
            tick.position[0],
            tick.position[1],
            tick.position[2],
            tick.delay,
            tick.priority
        ));
    }
    let neighbor_ticks = result.block_ticks.container(registration[0]).unwrap();
    actual.push(format!(
        "neighbor {} {}",
        neighbor_ticks.pack(500)[0].delay,
        neighbor_ticks.pack(1_000)[0].delay
    ));
    actual.push(format!("counter {}", result.region.ticks().next_sub_tick()));
    assert_eq!(actual, java);
    Ok(())
}

#[test]
#[cfg(target_os = "windows")]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn overlapping_saved_ticking_callbacks_match_java() -> Result<()> {
    use crate::ticking_preparation::{
        FullFuture, FullFutureId, FullFutureResult, TickingPreparations, TickingRequestId,
    };
    let (area, ids, registration) = overlapping_ticking_fixture()?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("chunk.nbt");
    std::fs::write(
        &path,
        native_persistence::nbt::write(&area.chunks[&area.center].document)?,
    )?;
    let java = super::super::postprocessing_world::tests::java_results_with_args(
        "SavedTickingDispatchOracle",
        include_str!("../../tests/java/SavedTickingDispatchOracle.java"),
        &[path.to_str().context("invalid NBT path")?, "overlap"],
    );
    let a = ChunkPos { x: 0, z: 0 };
    let b = ChunkPos { x: 1, z: 0 };
    let future_id =
        |pos| FullFutureId(registration.iter().position(|&entry| entry == pos).unwrap() as u64);
    let mut futures = TickingPreparations::default();
    for (id, center) in [(TickingRequestId(0), a), (TickingRequestId(1), b)] {
        futures.start(id, center, |pos| {
            Some(FullFuture {
                id: future_id(pos),
                result: None,
            })
        })?;
    }
    let mut batch = begin_ticking_batch(&area, &ids, &registration)?;
    for z in -1..=1 {
        for x in -1..=1 {
            futures.complete(future_id(ChunkPos { x, z }), FullFutureResult::Full)?;
        }
    }
    assert_eq!(
        futures.dispatch_next(|future| batch.dispatch(future, 500))?,
        Some((TickingRequestId(0), true))
    );
    for z in -1..=1 {
        futures.complete(future_id(ChunkPos { x: 2, z }), FullFutureResult::Full)?;
    }
    assert_eq!(
        futures.dispatch_next(|future| batch.dispatch(future, 500))?,
        Some((TickingRequestId(1), true))
    );
    futures.start(TickingRequestId(2), a, |pos| {
        Some(FullFuture {
            id: future_id(pos),
            result: Some(FullFutureResult::Full),
        })
    })?;
    assert_eq!(
        futures.dispatch_next(|future| batch.dispatch(future, 500))?,
        Some((TickingRequestId(2), true))
    );
    let mut result = batch.finish()?;
    let mut actual = vec![
        format!(
            "first {}",
            futures.state(TickingRequestId(0)) == Some(TickingFutureState::Available)
        ),
        format!(
            "second {}",
            futures.state(TickingRequestId(1)) == Some(TickingFutureState::Available)
        ),
        format!(
            "again {}",
            futures.state(TickingRequestId(2)) == Some(TickingFutureState::Available)
        ),
    ];
    for (label, pos) in [("a", a), ("b", b)] {
        for tick in result.block_ticks.container(pos).unwrap().pack(500) {
            actual.push(format!(
                "{label} {} {},{},{} {} {}",
                tick.kind,
                tick.position[0],
                tick.position[1],
                tick.position[2],
                tick.delay,
                tick.priority
            ));
        }
    }
    let neighbor = result
        .block_ticks
        .container(ChunkPos { x: -1, z: -1 })
        .unwrap();
    actual.push(format!(
        "neighbor {} {}",
        neighbor.pack(500)[0].delay,
        neighbor.pack(1_000)[0].delay
    ));
    actual.push(format!("counter {}", result.region.ticks().next_sub_tick()));
    for tick in Arc::make_mut(&mut result.block_ticks).collect_due(502, 65_536, |_| true) {
        actual.push(format!(
            "run {} {},{},{}",
            tick.kind, tick.position[0], tick.position[1], tick.position[2]
        ));
    }
    assert_eq!(actual, java);
    Ok(())
}

fn entity_fixture() -> Result<(ChunkArea, BTreeSet<String>)> {
    let (mut area, mut ids) = fixture()?;
    ids.extend(["minecraft:chest".into(), "minecraft:potent_sulfur".into()]);
    let chunk = Arc::make_mut(area.chunks.get_mut(&area.center).unwrap());
    let document = Arc::make_mut(&mut chunk.document);
    let Tag::Compound(fields) = &mut document.value else {
        unreachable!()
    };
    fn get<'a>(
        fields: &'a mut [(native_persistence::nbt::JavaString, Tag)],
        key: &str,
    ) -> &'a mut (native_persistence::nbt::JavaString, Tag) {
        fields
            .iter_mut()
            .find(|(name, _)| name.matches(key))
            .unwrap()
    }
    get(fields, "PostProcessing").1 =
        native_persistence::postprocessing::encode(&PostProcessing::new(0, 16).unwrap());
    get(fields, "block_ticks").1 = Tag::List {
        element_type: TagType::End,
        values: vec![],
    };
    let Tag::List {
        values: sections, ..
    } = &mut get(fields, "sections").1
    else {
        unreachable!()
    };
    let Tag::Compound(section) = &mut sections[0] else {
        unreachable!()
    };
    let Tag::Compound(blocks) = &mut get(section, "block_states").1 else {
        unreachable!()
    };
    let mut indices = vec![0; 4096];
    let mut entities = Vec::new();
    for i in 0..27 {
        let pos = [i % 16, 8, i / 16];
        let sulfur = i >= 24;
        indices[(pos[0] + pos[2] * 16 + pos[1] * 256) as usize] = if sulfur { 2 } else { 1 };
        let mut tag = if i % 3 == 1 {
            Tag::Compound(vec![
                ("id".into(), Tag::String("DUMMY".into())),
                ("x".into(), Tag::Int(pos[0])),
                ("y".into(), Tag::Int(pos[1])),
                ("z".into(), Tag::Int(pos[2])),
            ])
        } else if sulfur {
            super::super::GeneratedPotentSulfur::new(
                pos,
                BlockState {
                    name: "minecraft:potent_sulfur".into(),
                    properties: BTreeMap::from([("potent_sulfur_state".into(), "dry".into())]),
                },
            )?
            .block_entity()
        } else if i % 4 == 0 {
            super::super::GeneratedLootChest::new(
                pos,
                native_resources::structure_start::HorizontalDirection::North,
                "minecraft:chests/buried_treasure",
                -(i as i64),
            )?
            .block_entity()
        } else {
            super::super::GeneratedEmptyChest {
                position: pos,
                state: BlockState {
                    name: "minecraft:chest".into(),
                    properties: BTreeMap::from([
                        ("facing".into(), "north".into()),
                        ("type".into(), "single".into()),
                        ("waterlogged".into(), "false".into()),
                    ]),
                },
            }
            .block_entity()
        };
        let Tag::Compound(ref mut contents) = tag else {
            unreachable!()
        };
        if i % 3 != 0 || i % 2 == 0 {
            contents.push(("keepPacked".into(), Tag::Byte(i8::from(i % 3 != 0))));
        }
        entities.push(tag);
    }
    get(blocks, "palette").1 = Tag::List {
        element_type: TagType::String,
        values: [
            "minecraft:stone",
            "minecraft:chest",
            "minecraft:potent_sulfur",
        ]
        .into_iter()
        .map(|name| Tag::String(name.into()))
        .collect(),
    };
    get(blocks, "data").1 = Tag::LongArray(palette::encode_indices(3, &indices, 4)?.unwrap());
    fields.push((
        "block_entities".into(),
        Tag::List {
            element_type: TagType::Compound,
            values: entities,
        },
    ));
    chunk.sections = palette::decode_sections(&document.value)?;
    Ok((area, ids))
}

fn brushable_entity_fixture() -> Result<(ChunkArea, BTreeSet<String>)> {
    let (mut area, mut ids) = fixture()?;
    ids.extend([
        "minecraft:suspicious_sand".into(),
        "minecraft:suspicious_gravel".into(),
    ]);
    let chunk = Arc::make_mut(area.chunks.get_mut(&area.center).unwrap());
    let document = Arc::make_mut(&mut chunk.document);
    let Tag::Compound(fields) = &mut document.value else {
        unreachable!()
    };
    let find = |fields: &mut Vec<(native_persistence::nbt::JavaString, Tag)>, key: &str| {
        fields
            .iter()
            .position(|(name, _)| name.matches(key))
            .unwrap()
    };
    let post = find(fields, "PostProcessing");
    fields[post].1 =
        native_persistence::postprocessing::encode(&PostProcessing::new(0, 16).unwrap());
    let ticks = find(fields, "block_ticks");
    fields[ticks].1 = Tag::List {
        element_type: TagType::End,
        values: vec![],
    };
    let sections = find(fields, "sections");
    let Tag::List { values, .. } = &mut fields[sections].1 else {
        unreachable!()
    };
    let Tag::Compound(section) = &mut values[0] else {
        unreachable!()
    };
    let index = find(section, "block_states");
    let Tag::Compound(blocks) = &mut section[index].1 else {
        unreachable!()
    };
    let mut indices = vec![0; 4096];
    let mut entities = Vec::new();
    for i in 0..8 {
        let position = [i, 8, 0];
        let sand = i < 4;
        indices[(i + 8 * 256) as usize] = if sand { 1 } else { 2 };
        let state = BlockState {
            name: if sand {
                "minecraft:suspicious_sand"
            } else {
                "minecraft:suspicious_gravel"
            }
            .into(),
            properties: BTreeMap::from([("dusted".into(), "0".into())]),
        };
        let mut entity = match i % 4 {
            0 => Tag::Compound(vec![
                ("id".into(), Tag::String("DUMMY".into())),
                ("x".into(), Tag::Int(position[0])),
                ("y".into(), Tag::Int(position[1])),
                ("z".into(), Tag::Int(position[2])),
            ]),
            1 => super::super::GeneratedBrushable::empty_block_entity(position, &state)?,
            _ => super::super::GeneratedBrushable::from_saved_loot(
                position,
                state,
                "minecraft:archaeology/ocean_ruin_cold",
                if i % 4 == 2 { 0 } else { -37 },
            )?
            .block_entity(),
        };
        let Tag::Compound(contents) = &mut entity else {
            unreachable!()
        };
        contents.push(("keepPacked".into(), Tag::Byte(i8::from(i % 4 != 1))));
        entities.push(entity);
    }
    let palette = find(blocks, "palette");
    blocks[palette].1 = Tag::List {
        element_type: TagType::String,
        values: [
            "minecraft:stone",
            "minecraft:suspicious_sand",
            "minecraft:suspicious_gravel",
        ]
        .into_iter()
        .map(|name| Tag::String(name.into()))
        .collect(),
    };
    let data = find(blocks, "data");
    blocks[data].1 = Tag::LongArray(palette::encode_indices(3, &indices, 4)?.unwrap());
    fields.push((
        "block_entities".into(),
        Tag::List {
            element_type: TagType::Compound,
            values: entities,
        },
    ));
    chunk.sections = palette::decode_sections(&document.value)?;
    Ok((area, ids))
}

fn entity_output(area: &ChunkArea, ids: &BTreeSet<String>) -> Result<Tag> {
    let original = native_persistence::nbt::write(&area.chunks[&area.center].document)?;
    let mut stage = begin(area, ids)?;
    let pos = ChunkPos { x: 0, z: 0 };
    while !stage.poll(pos, 1)? {}
    let result = stage.finish()?.block_entity_data(pos)?;
    assert_eq!(
        native_persistence::nbt::write(&area.chunks[&area.center].document)?,
        original
    );
    Ok(result)
}

#[test]
fn saved_brushable_entities_preserve_deferred_loot_and_reject_unknown_contents() -> Result<()> {
    let (area, ids) = brushable_entity_fixture()?;
    let Tag::List { values, .. } = entity_output(&area, &ids)? else {
        unreachable!()
    };
    assert_eq!(values.len(), 8);
    for tag in &values {
        assert!(
            matches!(tag.get("id"), Some(Tag::String(id)) if id.matches("minecraft:brushable_block"))
        );
        assert_eq!(tag.get("keepPacked"), Some(&Tag::Byte(0)));
        let Some(Tag::Int(x)) = tag.get("x") else {
            unreachable!()
        };
        match *x % 4 {
            0 | 1 => assert!(tag.get("LootTable").is_none()),
            2 => {
                assert!(tag.get("LootTable").is_some());
                assert!(tag.get("LootTableSeed").is_none());
            }
            3 => assert_eq!(tag.get("LootTableSeed"), Some(&Tag::Long(-37))),
            _ => unreachable!(),
        }
    }
    let mut damaged = ChunkArea {
        center: area.center,
        radius: area.radius,
        chunks: area.chunks.clone(),
        missing: area.missing.clone(),
    };
    let chunk = Arc::make_mut(damaged.chunks.get_mut(&damaged.center).unwrap());
    let document = Arc::make_mut(&mut chunk.document);
    let Tag::Compound(fields) = &mut document.value else {
        unreachable!()
    };
    let Tag::List { values, .. } = &mut fields
        .iter_mut()
        .find(|(name, _)| name.matches("block_entities"))
        .unwrap()
        .1
    else {
        unreachable!()
    };
    let Tag::Compound(entity) = &mut values[2] else {
        unreachable!()
    };
    entity.push(("unknown".into(), Tag::Int(1)));
    assert!(begin(&damaged, &ids).is_err());
    Ok(())
}

fn deferred_entity_output(area: &ChunkArea, ids: &BTreeSet<String>) -> Result<(Tag, Tag)> {
    let mut expanded = ChunkArea {
        center: area.center,
        radius: area.radius,
        chunks: area.chunks.clone(),
        missing: area.missing.clone(),
    };
    let mut neighbor = fixture()?.0.chunks[&SavedChunkPos { x: 0, z: 0 }]
        .as_ref()
        .clone();
    neighbor.position = SavedChunkPos { x: 1, z: 0 };
    let document = Arc::make_mut(&mut neighbor.document);
    let Tag::Compound(fields) = &mut document.value else {
        unreachable!()
    };
    fields
        .iter_mut()
        .find(|(name, _)| name.matches("xPos"))
        .unwrap()
        .1 = Tag::Int(1);
    fields
        .iter_mut()
        .find(|(name, _)| name.matches("PostProcessing"))
        .unwrap()
        .1 = native_persistence::postprocessing::encode(&PostProcessing::new(0, 16).unwrap());
    fields
        .iter_mut()
        .find(|(name, _)| name.matches("block_ticks"))
        .unwrap()
        .1 = Tag::List {
        element_type: TagType::End,
        values: vec![],
    };
    expanded
        .chunks
        .insert(neighbor.position, Arc::new(neighbor));
    let home = ChunkPos { x: 0, z: 0 };
    let east = ChunkPos { x: 1, z: 0 };
    let mut stage = begin_with_registration(&expanded, ids, &[home, east])?;
    stage.select_initial_targets(BTreeSet::from([east]))?;
    let saved = stage.capture_deferred_block_entities(home)?;
    stage.request_converted_chunk(home)?;
    while !stage.poll(east, 1)? {}
    while !stage.poll(home, 1)? {}
    let after = stage.finish()?.block_entity_data(home)?;
    Ok((saved, after))
}

#[test]
fn saved_full_postprocessing_restores_supported_block_entities_without_mutating_source()
-> Result<()> {
    let (area, ids) = entity_fixture()?;
    let Tag::List { values, .. } = entity_output(&area, &ids)? else {
        unreachable!()
    };
    assert_eq!(values.len(), 27);
    let mut positions = BTreeSet::new();
    for tag in values {
        let (Some(Tag::Int(x)), Some(Tag::Int(y)), Some(Tag::Int(z))) =
            (tag.get("x"), tag.get("y"), tag.get("z"))
        else {
            unreachable!()
        };
        assert!(positions.insert((*x, *y, *z)));
        assert_eq!(tag.get("keepPacked"), Some(&Tag::Byte(0)));
        assert!(matches!(tag.get("id"), Some(Tag::String(id))
            if id.matches(if x + z * 16 < 24 { "minecraft:chest" } else { "minecraft:potent_sulfur" })));
    }
    Ok(())
}

#[test]
fn deferred_saved_entities_can_be_captured_then_postprocessed() -> Result<()> {
    let (area, ids) = entity_fixture()?;
    let (saved, after) = deferred_entity_output(&area, &ids)?;
    for tag in [saved, after] {
        let Tag::List { values, .. } = tag else {
            unreachable!()
        };
        assert_eq!(values.len(), 27);
        assert!(
            values
                .iter()
                .all(|tag| tag.get("keepPacked") == Some(&Tag::Byte(0)))
        );
    }
    Ok(())
}

#[test]
fn saved_loot_chests_keep_table_and_seed_during_promotion() -> Result<()> {
    let (area, ids) = entity_fixture()?;
    let (deferred, after) = deferred_entity_output(&area, &ids)?;
    for data in [deferred, after, entity_output(&area, &ids)?] {
        let Tag::List { values, .. } = data else {
            unreachable!()
        };
        for index in [0, 8, 12, 20] {
            let chest = values
                .iter()
                .find(|tag| {
                    tag.get("x") == Some(&Tag::Int(index % 16))
                        && tag.get("y") == Some(&Tag::Int(8))
                        && tag.get("z") == Some(&Tag::Int(index / 16))
                })
                .unwrap();
            assert_eq!(
                chest.get("LootTable"),
                Some(&Tag::String("minecraft:chests/buried_treasure".into()))
            );
            assert!(chest.get("Items").is_none());
            assert_eq!(
                chest.get("LootTableSeed"),
                (index != 0).then_some(&Tag::Long(-(index as i64)))
            );
            assert_eq!(chest.get("keepPacked"), Some(&Tag::Byte(0)));
        }
    }
    Ok(())
}

#[test]
fn saved_full_postprocessing_rejects_ambiguous_or_unpreservable_entities() -> Result<()> {
    for case in 0..10 {
        let (mut area, ids) = entity_fixture()?;
        let chunk = Arc::make_mut(area.chunks.get_mut(&area.center).unwrap());
        let document = Arc::make_mut(&mut chunk.document);
        let Tag::Compound(fields) = &mut document.value else {
            unreachable!()
        };
        let Tag::List { values, .. } = &mut fields
            .iter_mut()
            .find(|(name, _)| name.matches("block_entities"))
            .unwrap()
            .1
        else {
            unreachable!()
        };
        match case {
            0 => values.push(values[0].clone()),
            1 => {
                let Tag::Compound(fields) = &mut values[0] else {
                    unreachable!()
                };
                fields.push(("x".into(), Tag::Int(0)));
            }
            2 => {
                let Tag::Compound(fields) = &mut values[1] else {
                    unreachable!()
                };
                fields.push(("unknown".into(), Tag::Int(1)));
            }
            3 => {
                let Tag::Compound(fields) = &mut values[0] else {
                    unreachable!()
                };
                fields
                    .iter_mut()
                    .find(|(name, _)| name.matches("x"))
                    .unwrap()
                    .1 = Tag::Int(16);
            }
            4 => {
                let Tag::Compound(fields) = &mut values[0] else {
                    unreachable!()
                };
                fields
                    .iter_mut()
                    .find(|(name, _)| name.matches("keepPacked"))
                    .unwrap()
                    .1 = Tag::Int(0);
            }
            5 => {
                let Tag::Compound(fields) = &mut values[24] else {
                    unreachable!()
                };
                fields
                    .iter_mut()
                    .find(|(name, _)| name.matches("countdown"))
                    .unwrap()
                    .1 = Tag::Int(10);
            }
            6 => {
                let Tag::Compound(fields) = &mut values[8] else {
                    unreachable!()
                };
                fields
                    .iter_mut()
                    .find(|(name, _)| name.matches("LootTableSeed"))
                    .unwrap()
                    .1 = Tag::Int(-8);
            }
            7 => {
                let Tag::Compound(fields) = &mut values[0] else {
                    unreachable!()
                };
                fields.push(("LootTableSeed".into(), Tag::Long(0)));
            }
            8 => {
                let Tag::Compound(fields) = &mut values[0] else {
                    unreachable!()
                };
                fields
                    .iter_mut()
                    .find(|(name, _)| name.matches("LootTable"))
                    .unwrap()
                    .1 = Tag::String("INVALID TABLE".into());
            }
            9 => {
                let Tag::Compound(fields) = &mut values[0] else {
                    unreachable!()
                };
                fields.push((
                    "Items".into(),
                    Tag::List {
                        element_type: TagType::End,
                        values: vec![],
                    },
                ));
            }
            _ => unreachable!(),
        }
        let before = native_persistence::nbt::write(document)?;
        assert!(
            begin(&area, &ids).is_err(),
            "case {case} unexpectedly succeeded"
        );
        assert_eq!(
            native_persistence::nbt::write(&area.chunks[&area.center].document)?,
            before
        );
    }
    Ok(())
}

#[test]
#[cfg(target_os = "windows")]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn saved_brushable_entities_match_java_deserialization_and_postprocessing() -> Result<()> {
    let (area, ids) = brushable_entity_fixture()?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("chunk.nbt");
    std::fs::write(
        &path,
        native_persistence::nbt::write(&area.chunks[&area.center].document)?,
    )?;
    let java = super::super::postprocessing_world::tests::java_results_with_args(
        "SavedFullBlockEntityOracle",
        include_str!("../../tests/java/SavedFullBlockEntityOracle.java"),
        &[
            path.to_str().context("invalid NBT path")?,
            directory.path().to_str().context("invalid output path")?,
            "8",
        ],
    );
    let (saved, after) = deferred_entity_output(&area, &ids)?;
    let mut actual = Vec::new();
    for (label, data) in [
        ("S", saved),
        ("T", after),
        ("O", entity_output(&area, &ids)?),
    ] {
        let Tag::List { values, .. } = data else {
            unreachable!()
        };
        let mut line = label.to_owned();
        for tag in values {
            let (Some(Tag::Int(x)), Some(Tag::Int(y)), Some(Tag::Int(z))) =
                (tag.get("x"), tag.get("y"), tag.get("z"))
            else {
                unreachable!()
            };
            line.push_str(&format!(" {x},{y},{z}"));
            let expected = native_persistence::nbt::read(&std::fs::read(
                directory.path().join(format!("{label}-{x}-{y}-{z}.nbt")),
            )?)?
            .value;
            assert!(
                crate::inventory::equal_tag(&tag, &expected),
                "different NBT at {x},{y},{z}: {tag:?} vs {expected:?}"
            );
        }
        actual.push(line);
    }
    assert_eq!(actual, java);
    Ok(())
}

#[test]
#[cfg(target_os = "windows")]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn saved_full_block_entities_match_java_deserialization_and_postprocessing() -> Result<()> {
    let (area, ids) = entity_fixture()?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("chunk.nbt");
    std::fs::write(
        &path,
        native_persistence::nbt::write(&area.chunks[&area.center].document)?,
    )?;
    let java = super::super::postprocessing_world::tests::java_results_with_args(
        "SavedFullBlockEntityOracle",
        include_str!("../../tests/java/SavedFullBlockEntityOracle.java"),
        &[
            path.to_str().context("invalid NBT path")?,
            directory.path().to_str().context("invalid output path")?,
        ],
    );
    let (saved, after) = deferred_entity_output(&area, &ids)?;
    let mut actual = Vec::new();
    for (label, data) in [
        ("S", saved),
        ("T", after),
        ("O", entity_output(&area, &ids)?),
    ] {
        let Tag::List { values, .. } = data else {
            unreachable!()
        };
        let mut line = label.to_owned();
        for tag in values {
            let (Some(Tag::Int(x)), Some(Tag::Int(y)), Some(Tag::Int(z))) =
                (tag.get("x"), tag.get("y"), tag.get("z"))
            else {
                unreachable!()
            };
            line.push_str(&format!(" {x},{y},{z}"));
            let expected = native_persistence::nbt::read(&std::fs::read(
                directory.path().join(format!("{label}-{x}-{y}-{z}.nbt")),
            )?)?
            .value;
            assert!(
                crate::inventory::equal_tag(&tag, &expected),
                "different NBT at {x},{y},{z}: {tag:?} vs {expected:?}"
            );
        }
        actual.push(line);
    }
    assert_eq!(actual, java);
    Ok(())
}

fn trace() -> Result<Vec<String>> {
    let (area, ids) = fixture()?;
    let original = native_persistence::nbt::write(&area.chunks[&area.center].document)?;
    let mut stage = begin(&area, &ids)?;
    let pos = ChunkPos { x: 0, z: 0 };
    while !stage.poll(pos, 1)? {}
    let result = stage.finish()?;
    let chunk = result.ticks().chunk(pos).unwrap();
    let mut trace: Vec<_> = chunk
        .blocks
        .pack(100)
        .iter()
        .map(|tick| {
            format!(
                "{} {},{},{} {} {}",
                tick.kind,
                tick.position[0],
                tick.position[1],
                tick.position[2],
                tick.delay,
                tick.priority
            )
        })
        .collect();
    trace.push(format!("counter {}", result.ticks().next_sub_tick()));
    assert_eq!(
        native_persistence::nbt::write(&area.chunks[&area.center].document)?,
        original
    );
    Ok(trace)
}

#[test]
fn saved_full_postprocessing_keeps_marks_and_pending_ticks_in_order() -> Result<()> {
    assert_eq!(
        trace()?,
        [
            "minecraft:sand 8,8,8 0 0",
            "minecraft:red_sand 9,8,8 2 0",
            "minecraft:gravel 10,8,8 2 0",
            "counter 26",
        ]
    );
    Ok(())
}

#[test]
fn saved_full_postprocessing_rejects_unavailable_or_changed_inputs() -> Result<()> {
    let (area, ids) = fixture()?;
    let mut different_sections = fixture()?.0;
    Arc::make_mut(
        different_sections
            .chunks
            .get_mut(&different_sections.center)
            .unwrap(),
    )
    .sections
    .clear();
    assert!(begin(&different_sections, &ids).is_err());

    let mut packed = fixture()?.0;
    let chunk = Arc::make_mut(packed.chunks.get_mut(&packed.center).unwrap());
    let document = Arc::make_mut(&mut chunk.document);
    let Tag::Compound(fields) = &mut document.value else {
        unreachable!()
    };
    fields.push((
        "block_entities".into(),
        Tag::List {
            element_type: TagType::Compound,
            values: vec![Tag::Compound(vec![("keepPacked".into(), Tag::Byte(1))])],
        },
    ));
    assert!(begin(&packed, &ids).is_err());

    let mut duplicate_ticks = fixture()?.0;
    let chunk = Arc::make_mut(
        duplicate_ticks
            .chunks
            .get_mut(&duplicate_ticks.center)
            .unwrap(),
    );
    let document = Arc::make_mut(&mut chunk.document);
    let Tag::Compound(fields) = &mut document.value else {
        unreachable!()
    };
    fields.push(("block_ticks".into(), Tag::Int(0)));
    assert!(begin(&duplicate_ticks, &ids).is_err());
    assert!(begin(&area, &ids).is_ok());
    Ok(())
}

#[test]
fn missing_saved_neighbor_discards_partial_work_and_can_be_retried() -> Result<()> {
    use super::super::PostprocessingAttempt;
    let (mut area, ids) = fixture()?;
    let center = ChunkPos { x: 0, z: 0 };
    let neighbor = ChunkPos { x: 1, z: 0 };
    let saved_center = area.center;
    let chunk = Arc::make_mut(area.chunks.get_mut(&saved_center).unwrap());
    let document = Arc::make_mut(&mut chunk.document);
    let Tag::Compound(fields) = &mut document.value else {
        unreachable!()
    };
    let mut marks = PostProcessing::new(0, 16).unwrap();
    assert!(marks.mark(9, 8, 8));
    assert!(marks.mark(15, 8, 8));
    let post = fields
        .iter_mut()
        .find(|(name, _)| name.matches("PostProcessing"))
        .unwrap();
    post.1 = native_persistence::postprocessing::encode(&marks);
    let original = native_persistence::nbt::write(document)?;

    let mut incomplete = begin(&area, &ids)?;
    assert_eq!(
        incomplete.try_poll(center, 256)?,
        PostprocessingAttempt::NeedsChunk(neighbor)
    );
    assert_eq!(
        incomplete.try_poll(center, 256)?,
        PostprocessingAttempt::NeedsChunk(neighbor)
    );
    assert!(incomplete.finish().is_err());
    assert_eq!(
        native_persistence::nbt::write(&area.chunks[&saved_center].document)?,
        original
    );

    let mut saved_neighbor = area.chunks[&saved_center].as_ref().clone();
    saved_neighbor.position = SavedChunkPos { x: 1, z: 0 };
    let document = Arc::make_mut(&mut saved_neighbor.document);
    let Tag::Compound(fields) = &mut document.value else {
        unreachable!()
    };
    fields
        .iter_mut()
        .find(|(name, _)| name.matches("xPos"))
        .unwrap()
        .1 = Tag::Int(1);
    fields
        .iter_mut()
        .find(|(name, _)| name.matches("PostProcessing"))
        .unwrap()
        .1 = native_persistence::postprocessing::encode(&PostProcessing::new(0, 16).unwrap());
    fields
        .iter_mut()
        .find(|(name, _)| name.matches("block_ticks"))
        .unwrap()
        .1 = Tag::List {
        element_type: TagType::End,
        values: vec![],
    };
    area.chunks
        .insert(saved_neighbor.position, Arc::new(saved_neighbor));
    let mut expanded = begin_with_registration(&area, &ids, &[center, neighbor])?;
    expanded.select_initial_targets(BTreeSet::from([center]))?;
    assert_eq!(
        expanded.try_poll(center, 256)?,
        PostprocessingAttempt::Progress { complete: true }
    );
    let result = expanded.finish()?;
    let ticks = result.ticks().chunk(center).unwrap().blocks.pack(100);
    assert_eq!(ticks.len(), 2);
    assert_eq!(ticks[0].kind, "minecraft:sand");
    assert_eq!(ticks[1].kind, "minecraft:red_sand");
    assert_eq!(ticks[1].position, [9, 8, 8]);
    assert_eq!(ticks[1].delay, 2);
    assert_eq!(
        native_persistence::nbt::write(&area.chunks[&saved_center].document)?,
        original
    );
    Ok(())
}

#[test]
#[cfg(target_os = "windows")]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn saved_full_postprocessing_matches_java_falling_tick_order() -> Result<()> {
    let (area, _) = fixture()?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("chunk.nbt");
    std::fs::write(
        &path,
        native_persistence::nbt::write(&area.chunks[&area.center].document)?,
    )?;
    let expected = super::super::postprocessing_world::tests::java_results_with_args(
        "SavedFullPostprocessingOracle",
        include_str!("../../tests/java/SavedFullPostprocessingOracle.java"),
        &[path.to_str().context("invalid temporary NBT path")?],
    );
    assert_eq!(trace()?, expected);
    Ok(())
}
