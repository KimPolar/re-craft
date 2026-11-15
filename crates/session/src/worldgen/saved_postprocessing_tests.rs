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
    let defaults = BTreeMap::new();
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
                ids.contains(&state.name) && state.properties.is_empty(),
                "unknown fixture state"
            );
            Ok(())
        },
    )
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
