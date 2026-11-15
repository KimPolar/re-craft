//! Opt-in round trip through the installed Java 26.3 codecs in a temporary world.
use super::*;
use native_persistence::world::ChunkPos;
use std::{path::PathBuf, process::Command, time::Duration};

/// Java stores compounds as maps and may write their keys in a different order.
/// Lists retain their order and numeric values are compared bit-for-bit.
fn same_nbt(a: &native_persistence::nbt::Tag, b: &native_persistence::nbt::Tag) -> bool {
    use native_persistence::nbt::Tag;
    match (a, b) {
        (Tag::Compound(a), Tag::Compound(b)) => {
            a.len() == b.len()
                && a.iter().enumerate().all(|(index, (key, value))| {
                    !a[..index].iter().any(|(other, _)| other == key)
                        && b.iter().filter(|(other, _)| other == key).count() == 1
                        && b.iter()
                            .find(|(other, _)| other == key)
                            .is_some_and(|(_, other)| same_nbt(value, other))
                })
        }
        (
            Tag::List {
                element_type: a_type,
                values: a,
            },
            Tag::List {
                element_type: b_type,
                values: b,
            },
        ) => a_type == b_type && a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same_nbt(a, b)),
        (Tag::Float(a), Tag::Float(b)) => a.to_bits() == b.to_bits(),
        (Tag::Double(a), Tag::Double(b)) => a.to_bits() == b.to_bits(),
        _ => a == b,
    }
}

fn same_optional_nbt(
    a: Option<&native_persistence::nbt::Tag>,
    b: Option<&native_persistence::nbt::Tag>,
) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => same_nbt(a, b),
        _ => false,
    }
}

/// Section palettes may be encoded differently after a Java save. Compare the
/// remaining fields without silently discarding unknown chunk metadata.
fn chunk_metadata(value: &native_persistence::nbt::Tag) -> Option<native_persistence::nbt::Tag> {
    use native_persistence::nbt::Tag;
    match value {
        Tag::Compound(entries) => Some(Tag::Compound(
            entries
                .iter()
                .filter(|(name, _)| !name.matches("sections"))
                .cloned()
                .collect(),
        )),
        _ => None,
    }
}

fn same_chunk_metadata(a: &native_persistence::nbt::Tag, b: &native_persistence::nbt::Tag) -> bool {
    match (chunk_metadata(a), chunk_metadata(b)) {
        (Some(a), Some(b)) => same_nbt(&a, &b),
        _ => false,
    }
}

#[test]
fn nbt_comparison_ignores_only_compound_key_order() {
    use native_persistence::nbt::{Tag, TagType};
    let a = Tag::Compound(vec![
        ("a".into(), Tag::Float(-0.0)),
        (
            "b".into(),
            Tag::List {
                element_type: TagType::Int,
                values: vec![Tag::Int(1), Tag::Int(2)],
            },
        ),
    ]);
    let reversed = Tag::Compound(vec![
        (
            "b".into(),
            Tag::List {
                element_type: TagType::Int,
                values: vec![Tag::Int(1), Tag::Int(2)],
            },
        ),
        ("a".into(), Tag::Float(-0.0)),
    ]);
    let reordered_list = Tag::Compound(vec![
        (
            "b".into(),
            Tag::List {
                element_type: TagType::Int,
                values: vec![Tag::Int(2), Tag::Int(1)],
            },
        ),
        ("a".into(), Tag::Float(-0.0)),
    ]);
    let duplicate = Tag::Compound(vec![
        ("a".into(), Tag::Float(-0.0)),
        ("a".into(), Tag::Float(-0.0)),
    ]);
    assert!(same_nbt(&a, &reversed));
    assert!(!same_nbt(&a, &reordered_list));
    assert!(!same_nbt(&duplicate, &duplicate));
    assert!(!same_nbt(&Tag::Float(0.0), &Tag::Float(-0.0)));
    assert!(same_optional_nbt(None, None));
    assert!(!same_optional_nbt(Some(&a), None));
    assert!(same_optional_nbt(Some(&a), Some(&reversed)));
    let with_sections = Tag::Compound(vec![
        ("sections".into(), Tag::Int(1)),
        ("future_field".into(), Tag::Long(42)),
    ]);
    let reencoded_sections = Tag::Compound(vec![
        ("sections".into(), Tag::Int(2)),
        ("future_field".into(), Tag::Long(42)),
    ]);
    let lost_field = Tag::Compound(vec![("sections".into(), Tag::Int(2))]);
    assert!(same_chunk_metadata(&with_sections, &reencoded_sections));
    assert!(!same_chunk_metadata(&with_sections, &lost_field));
}

/// Enumerate actual region entries instead of assuming the requested chunks
/// are the only chunks written by initial postprocessing.
fn saved_terrain_positions(directory: &std::path::Path) -> Result<BTreeSet<ChunkPos>> {
    let positions = saved_region_positions(directory)?;
    ensure!(!positions.is_empty(), "no saved terrain chunks");
    Ok(positions)
}

fn saved_region_positions(directory: &std::path::Path) -> Result<BTreeSet<ChunkPos>> {
    let mut positions = BTreeSet::new();
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(positions),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_str().context("non-Unicode region filename")?;
        if !name.ends_with(".mca") {
            continue;
        }
        let mut region = native_persistence::anvil::RegionReader::open(&entry.path())?;
        for (x, z) in region.chunk_positions()? {
            ensure!(
                positions.insert(ChunkPos { x, z }),
                "duplicate region entry {x}, {z}"
            );
        }
    }
    Ok(positions)
}

#[test]
fn enumerates_actual_region_entries_including_negative_coordinates() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut header = [0u8; 4 * 4096];
    header[0..4].copy_from_slice(&[0, 0, 2, 1]);
    header[4092..4096].copy_from_slice(&[0, 0, 3, 1]);
    std::fs::write(dir.path().join("r.-1.0.mca"), header)?;
    assert_eq!(
        saved_terrain_positions(dir.path())?,
        BTreeSet::from([ChunkPos { x: -32, z: 0 }, ChunkPos { x: -1, z: 31 }])
    );
    header[0..4].copy_from_slice(&[0, 0, 0, 1]);
    std::fs::write(dir.path().join("r.-1.0.mca"), header)?;
    assert!(saved_terrain_positions(dir.path()).is_err());
    assert!(saved_region_positions(&dir.path().join("missing"))?.is_empty());
    Ok(())
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn new_player_round_trips_through_java_player() -> Result<()> {
    use native_session::{
        GameMode,
        inventory::PlayerInventory,
        player_data::{PlayerSaveState, SavedPlayerMotion, draft_new_player_state},
    };
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let temp = tempfile::tempdir()?;
    let version = game.join("versions/26.3");
    let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for library in manifest["libraries"]
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
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let source = temp.path().join("GeneratedWorldJavaOracle.java");
    std::fs::write(&source, include_str!("GeneratedWorldJavaOracle.java"))?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&classpath)
        .arg(&source)
        .output()?;
    ensure!(
        compiled.status.success(),
        "Java compilation failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let state = PlayerSaveState {
        position: glam::DVec3::new(-17.25, 64.5, 99.75),
        motion: SavedPlayerMotion {
            velocity: glam::DVec3::ZERO,
            on_ground: true,
            fall_distance: 0.0,
        },
        yaw: 91.5,
        pitch: -20.0,
        dimension: "minecraft:overworld".into(),
        game_mode: GameMode::Survival,
        vitals: Default::default(),
        experience: Default::default(),
        score: 0,
    };
    let player = draft_new_player_state(
        [1, -2, 3, 4],
        55,
        &state,
        &PlayerInventory::default(),
        false,
    )?;
    let player_file = temp.path().join("player.nbt");
    std::fs::write(
        &player_file,
        native_persistence::nbt::write(player.document())?,
    )?;
    let checked = Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&classpath)
        .arg("GeneratedWorldJavaOracle")
        .arg("--player")
        .arg(&player_file)
        .output()?;
    ensure!(
        checked.status.success(),
        "Java player check failed: {} {}",
        String::from_utf8_lossy(&checked.stdout),
        String::from_utf8_lossy(&checked.stderr)
    );
    ensure!(
        String::from_utf8_lossy(&checked.stdout)
            .lines()
            .any(|line| line == "PLAYER_OK"),
        "Java player check returned no result"
    );
    Ok(())
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK; generates a temporary world"]
fn generated_world_spawn_chunk_round_trips_through_java() -> Result<()> {
    generated_world_round_trips_through_java(-1, GenerationCheck::RoundTrip)
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK; generates a temporary world"]
fn generated_world_seed_one_round_trips_through_java() -> Result<()> {
    generated_world_round_trips_through_java(1, GenerationCheck::RoundTrip)
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK; currently fails on unsettled live light"]
fn generated_world_seed_one_captures_live_inputs_for_missing_shape_chunk() -> Result<()> {
    generated_world_round_trips_through_java(1, GenerationCheck::LiveInputs)
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK; generates a temporary world"]
fn generated_world_seed_one_stages_dirty_terrain() -> Result<()> {
    generated_world_round_trips_through_java(1, GenerationCheck::StagedTerrain)
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK; generates a temporary world"]
fn generated_world_seed_one_runs_missing_chunk_generation_workers() -> Result<()> {
    generated_world_round_trips_through_java(1, GenerationCheck::GenerationWorkers)
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK; generates a temporary world"]
fn generated_world_seed_42_round_trips_through_java() -> Result<()> {
    generated_world_round_trips_through_java(42, GenerationCheck::RoundTrip)
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum GenerationCheck {
    RoundTrip,
    LiveInputs,
    StagedTerrain,
    GenerationWorkers,
}

fn prepare_missing_generation(
    targets: crate::chunk_stream::MissingGenerationTargets,
    streamer: &crate::chunk_stream::ChunkStreamer,
    session: &native_session::loaded::LoadedSession,
    world_path: &std::path::Path,
    dimension: &native_persistence::world::DimensionId,
    guard: &native_persistence::save::WorldSessionGuard,
    game: &std::path::Path,
) -> Result<crate::chunk_stream::PreparedMissingGeneration> {
    let reader = native_persistence::world::WorldReader::open_with_session(
        world_path,
        dimension.clone(),
        guard.clone(),
    )?;
    let mut capture = crate::chunk_stream::MissingGenerationCaptureWorker::start(
        targets, streamer, session, reader,
    )?;
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    let captured = loop {
        if let Some(result) = capture.try_result(streamer, session)? {
            break result;
        }
        ensure!(
            std::time::Instant::now() < deadline,
            "generation capture timed out"
        );
        std::thread::sleep(Duration::from_millis(2));
    };
    let mut prepare = crate::chunk_stream::MissingGenerationPrepareWorker::start(
        captured,
        streamer,
        session,
        game.to_path_buf(),
    )
    .map_err(|failure| failure.error)?;
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    loop {
        if let Some(result) = prepare.try_result(streamer, session)? {
            return Ok(result);
        }
        ensure!(
            std::time::Instant::now() < deadline,
            "generation preparation timed out"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn generated_world_round_trips_through_java(seed: i64, check: GenerationCheck) -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let border_data = native_persistence::world::SavedWorldBorder::default();
    let mut settings = crate::world_creation::WorldCreationSettings::new("Test".into());
    settings.seed = crate::chat_input::ChatInput::field(&seed.to_string());
    settings.generate_structures = true;
    let worker = GenerationWorker::start_structured_initial_world(
        settings.freeze(game.clone(), None),
        StructuredInitialOptions {
            border: native_session::respawn::RespawnBorder::from_saved(border_data, 0)?,
            spawn_mobs: true,
            advance_time: true,
            water_source_conversion: true,
            player: InitialPlayerSpawnOptions {
                radius: 10,
                adventure: false,
                random_seed: 1234,
                saved_border: Some(border_data),
            },
            ceiling_spawn_height: None,
            initial_bonus_seed: None,
        },
        1,
        0,
    )?;
    let deadline = std::time::Instant::now() + Duration::from_secs(900);
    let mut ready = loop {
        match worker.result.recv_timeout(Duration::from_secs(20)) {
            Ok(GenerationResult::Ready(ready)) => break ready,
            Ok(GenerationResult::Failed(error)) => {
                return Err(error.context(format!("generation: {:?}", worker.progress())));
            }
            Ok(GenerationResult::Cancelled) => anyhow::bail!("world generation cancelled"),
            Err(mpsc::RecvTimeoutError::Timeout) if std::time::Instant::now() < deadline => {
                eprintln!("generation: {:?}", worker.progress())
            }
            Err(error) => return Err(error.into()),
        }
    };
    let world_spawn = ready.suggested_spawn.context("world spawn missing")?;
    let player_spawn = ready.player_spawn.context("player spawn missing")?;
    let session_targets = ready
        .mesh
        .targets
        .as_ref()
        .context("session targets missing")?
        .clone();
    let generated_regions = ready.batch.prepare_converted_structure_region_drafts(
        &ready
            .postprocessed
            .as_ref()
            .context("postprocessed region missing")?
            .region,
        1,
    )?;
    let directory = tempfile::tempdir()?;
    let mut stage = ready.stage_structure_initial_world(directory.path(), 123, 1)?;
    let dimension = native_persistence::world::DimensionId::parse("minecraft:overworld")?;
    fn copy_metadata(source: &std::path::Path, target: &std::path::Path) -> Result<()> {
        std::fs::create_dir_all(target)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            let destination = target.join(entry.file_name());
            if kind.is_dir() {
                copy_metadata(&entry.path(), &destination)?;
            } else {
                ensure!(kind.is_file(), "unexpected metadata link");
                std::fs::copy(entry.path(), destination)?;
            }
        }
        Ok(())
    }
    let workspace_temp =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/region-test-temp");
    std::fs::create_dir_all(&workspace_temp)?;
    let transaction_world = tempfile::Builder::new()
        .prefix("generated-transaction-")
        .tempdir_in(workspace_temp)?;
    std::fs::copy(
        stage.path().join("level.dat"),
        transaction_world.path().join("level.dat"),
    )?;
    for name in ["data", "players"] {
        copy_metadata(
            &stage.path().join(name),
            &transaction_world.path().join(name),
        )?;
    }
    std::fs::create_dir_all(
        transaction_world
            .path()
            .join("dimensions/minecraft/overworld"),
    )?;
    let mut save = native_persistence::save::WorldSavePlan::new();
    save.add_generated_regions(dimension.clone(), generated_regions.regions.into_values())?;
    ensure!(
        save.chunk_count() == generated_regions.chunk_count,
        "generated chunk count differs"
    );
    let mut guard = native_persistence::save::WorldWriteGuard::acquire(transaction_world.path())?;
    guard.write_plan(save, |_, _| Ok(()))?;
    drop(guard);
    let position = native_persistence::world::ChunkPos {
        x: player_spawn.x.div_euclid(16),
        z: player_spawn.z.div_euclid(16),
    };
    let positions =
        saved_terrain_positions(&stage.path().join("dimensions/minecraft/overworld/region"))?;
    let sidecars = stage.path().join("dimensions/minecraft/overworld");
    let entity_positions = saved_region_positions(&sidecars.join("entities"))?;
    let poi_positions = saved_region_positions(&sidecars.join("poi"))?;
    let transaction_sidecars = transaction_world
        .path()
        .join("dimensions/minecraft/overworld");
    ensure!(
        saved_terrain_positions(&transaction_sidecars.join("region"))? == positions,
        "transaction terrain positions differ"
    );
    ensure!(
        saved_region_positions(&transaction_sidecars.join("entities"))? == entity_positions,
        "transaction entity positions differ"
    );
    ensure!(
        saved_region_positions(&transaction_sidecars.join("poi"))? == poi_positions,
        "transaction POI positions differ"
    );
    ensure!(
        entity_positions.is_subset(&positions),
        "entity chunk has no saved terrain"
    );
    ensure!(
        poi_positions.is_subset(&positions),
        "POI chunk has no saved terrain"
    );
    ensure!(positions.contains(&position), "player chunk is not saved");
    let mut reader = native_persistence::world::WorldReader::open(stage.path(), dimension.clone())?;
    let mut resources = native_resources::packs::ResourceStack::open(&game)?;
    let defaults = crate::vanilla_world_block_defaults(&mut resources)?;
    let materialize = |position, chunk: native_persistence::world::LoadedChunk| {
        native_persistence::world::ChunkArea {
            center: position,
            radius: 0,
            chunks: BTreeMap::from([(position, Arc::new(chunk))]),
            missing: Vec::new(),
        }
        .materialize_with_defaults(&dimension, &defaults)
    };
    let mut session_entity_chunks = 0usize;
    for &pos in session_targets.iter() {
        let staged_entities = reader.read_entity_chunk(pos.x, pos.z)?;
        match (
            stage.initial.session.saved_entity_chunk(pos),
            staged_entities,
        ) {
            (Some(native_session::loaded::SavedEntityData::Empty), None) => {}
            (Some(native_session::loaded::SavedEntityData::Present(session)), Some(saved)) => {
                assert_eq!(
                    session.document, saved.document,
                    "initial session entities {pos:?}"
                );
                session_entity_chunks += 1;
            }
            _ => anyhow::bail!("initial session entity data differs from staged chunk {pos:?}"),
        }
        let staged_poi = reader.read_poi_chunk(pos.x, pos.z)?;
        match (stage.initial.session.saved_poi_chunk(pos), staged_poi) {
            (Some(native_session::loaded::SavedPoiData::Missing), None) => {}
            (Some(native_session::loaded::SavedPoiData::Present(session)), Some(saved)) => {
                assert_eq!(
                    session.document, saved.document,
                    "initial session POI {pos:?}"
                );
            }
            _ => anyhow::bail!("initial session POI differs from staged chunk {pos:?}"),
        }
    }
    eprintln!("Matched {session_entity_chunks} nonempty entity chunks in the initial session");
    // The initial player area does not necessarily contain generated entities.
    // Import a real nonempty staged chunk before the first tick and check that
    // its entity and POI documents remain intact alongside its terrain.
    let entity_position = *entity_positions
        .iter()
        .next()
        .context("no generated entity chunk")?;
    let session_position = native_world::loaded::ChunkPos {
        x: entity_position.x,
        z: entity_position.z,
    };
    let saved_entities = reader
        .read_entity_chunk(entity_position.x, entity_position.z)?
        .context("generated entity chunk missing")?;
    let saved_entity_document = Arc::clone(&saved_entities.document);
    let session = &mut stage.initial.session;
    if !session.initial_spawn_chunk_loaded(session_position) {
        let chunk = reader
            .read_chunk(entity_position.x, entity_position.z)?
            .context("generated entity terrain missing")?;
        let world = materialize(entity_position, chunk)?;
        let poi = match reader.read_poi_chunk(entity_position.x, entity_position.z)? {
            Some(chunk) => native_session::loaded::SavedPoiData::Present(Arc::new(chunk)),
            None => native_session::loaded::SavedPoiData::Missing,
        };
        let expected_poi = match &poi {
            native_session::loaded::SavedPoiData::Present(chunk) => {
                Some(Arc::clone(&chunk.document))
            }
            native_session::loaded::SavedPoiData::Missing => None,
            native_session::loaded::SavedPoiData::Unavailable => unreachable!(),
        };
        session.import_initial_spawn_chunk(
            session_position,
            world,
            native_session::loaded::SavedEntityData::Present(Arc::new(saved_entities)),
            poi,
            None,
        )?;
        match (session.saved_poi_chunk(session_position), expected_poi) {
            (Some(native_session::loaded::SavedPoiData::Present(chunk)), Some(document)) => {
                ensure!(chunk.document == document, "imported POI differs")
            }
            (Some(native_session::loaded::SavedPoiData::Missing), None) => {}
            _ => anyhow::bail!("imported POI is unavailable"),
        }
    } else {
        match session.saved_entity_chunk(session_position) {
            Some(native_session::loaded::SavedEntityData::Present(chunk)) => {
                ensure!(
                    chunk.document == saved_entity_document,
                    "loaded generated entities differ"
                );
            }
            None | Some(native_session::loaded::SavedEntityData::Unavailable) => {
                session.set_initial_entity_chunk(
                    session_position,
                    native_session::loaded::SavedEntityData::Present(Arc::new(saved_entities)),
                )?;
            }
            Some(native_session::loaded::SavedEntityData::Empty) => {
                anyhow::bail!("generated entities were marked empty")
            }
        }
    }
    match session.saved_entity_chunk(session_position) {
        Some(native_session::loaded::SavedEntityData::Present(chunk)) => ensure!(
            chunk.document == saved_entity_document,
            "imported generated entities differ"
        ),
        _ => anyhow::bail!("generated entities were not retained"),
    }
    eprintln!(
        "Imported generated entity chunk {}, {} before the first tick",
        entity_position.x, entity_position.z
    );
    let mut before_chunks = BTreeMap::new();
    for &pos in &positions {
        let chunk = reader
            .read_chunk(pos.x, pos.z)?
            .with_context(|| format!("saved chunk {pos:?} missing"))?;
        before_chunks.insert(pos, chunk);
    }
    let mut before_entities = BTreeMap::new();
    for &pos in &entity_positions {
        let chunk = reader
            .read_entity_chunk(pos.x, pos.z)?
            .with_context(|| format!("entity chunk {pos:?} missing"))?;
        before_entities.insert(pos, chunk.document);
    }
    let mut before_pois = BTreeMap::new();
    for &pos in &poi_positions {
        let chunk = reader
            .read_poi_chunk(pos.x, pos.z)?
            .with_context(|| format!("POI chunk {pos:?} missing"))?;
        before_pois.insert(pos, chunk.document);
    }
    let player = reader.read_player()?.context("player data missing")?;
    let clocks = reader.world_clocks()?;
    let game_rules = reader.fluid_game_rules()?;
    assert_eq!(
        game_rules,
        native_persistence::world::FluidGameRules::default()
    );
    drop(reader);
    let version = game.join("versions/26.3");
    let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    let oracle = tempfile::tempdir()?;
    let mut paths = vec![version.join("26.3.jar"), oracle.path().to_path_buf()];
    for library in manifest["libraries"]
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
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let source = oracle.path().join("GeneratedWorldJavaOracle.java");
    let position_file = oracle.path().join("chunks.txt");
    std::fs::write(
        &position_file,
        positions
            .iter()
            .map(|pos| format!("{} {}\n", pos.x, pos.z))
            .collect::<String>(),
    )?;
    let entity_file = oracle.path().join("entities.txt");
    let poi_file = oracle.path().join("poi.txt");
    for (path, positions) in [
        (&entity_file, &entity_positions),
        (&poi_file, &poi_positions),
    ] {
        std::fs::write(
            path,
            positions
                .iter()
                .map(|pos| format!("{} {}\n", pos.x, pos.z))
                .collect::<String>(),
        )?;
    }
    std::fs::write(&source, include_str!("GeneratedWorldJavaOracle.java"))?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&classpath)
        .arg(&source)
        .output()?;
    ensure!(
        compiled.status.success(),
        "Java compilation failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let portal_check = Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&classpath)
        .arg("GeneratedWorldJavaOracle")
        .arg("--portal-self-test")
        .output()?;
    ensure!(
        portal_check.status.success()
            && String::from_utf8_lossy(&portal_check.stdout)
                .lines()
                .any(|line| line == "PORTAL_ORACLE_OK"),
        "Java portal oracle self-test failed: {} {}",
        String::from_utf8_lossy(&portal_check.stdout),
        String::from_utf8_lossy(&portal_check.stderr)
    );
    let checked = Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&classpath)
        .arg("GeneratedWorldJavaOracle")
        .arg(transaction_world.path())
        .arg(seed.to_string())
        .args(world_spawn.to_array().map(|v| v.to_string()))
        .args(player_spawn.to_array().map(|v| v.to_string()))
        .arg(&position_file)
        .arg(&entity_file)
        .arg(&poi_file)
        .output()?;
    ensure!(
        checked.status.success(),
        "Java world check failed: {} {}",
        String::from_utf8_lossy(&checked.stdout),
        String::from_utf8_lossy(&checked.stderr)
    );
    ensure!(
        String::from_utf8_lossy(&checked.stdout)
            .lines()
            .any(|line| line
                == format!(
                    "OK {} {} {}",
                    positions.len(),
                    entity_positions.len(),
                    poi_positions.len()
                )),
        "Java world check returned no result"
    );
    let structures_line = String::from_utf8_lossy(&checked.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("STRUCTURES ").map(str::to_owned))
        .context("Java structure count missing")?;
    let structure_counts = structures_line
        .split_whitespace()
        .map(str::parse::<usize>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    ensure!(structure_counts.len() == 4, "invalid Java structure counts");
    let structure_types = String::from_utf8_lossy(&checked.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("STRUCTURE_TYPES ").map(str::to_owned))
        .context("Java structure type report missing")?;
    eprintln!("Saved structure types: {structure_types}");
    eprintln!(
        "Java restored {} portal starts and {} references, {} mineshaft starts and {} references",
        structure_counts[0], structure_counts[1], structure_counts[2], structure_counts[3]
    );
    if seed == -1 {
        ensure!(
            structure_counts[2] > 0 && structure_counts[3] > 0,
            "seed -1 did not exercise a mineshaft start and reference"
        );
    }
    let entity_count: usize = String::from_utf8_lossy(&checked.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("ENTITIES "))
        .context("Java entity count missing")?
        .parse()?;
    if seed == 42 {
        ensure!(
            entity_count > 0,
            "seed 42 did not exercise Java entity loading"
        );
    }
    eprintln!(
        "Java loaded and saved {} entities from {} entity chunks and {} POI chunks",
        entity_count,
        entity_positions.len(),
        poi_positions.len()
    );

    let sidecars = transaction_sidecars;
    let mut reader =
        native_persistence::world::WorldReader::open(transaction_world.path(), dimension.clone())?;
    assert_eq!(reader.read_player()?.as_ref(), Some(&player));
    assert_eq!(reader.world_clocks()?, clocks);
    assert_eq!(reader.fluid_game_rules()?, game_rules);
    assert_eq!(reader.saved_world_seed()?, Some(seed));
    assert_eq!(
        saved_region_positions(&sidecars.join("entities"))?,
        entity_positions
    );
    assert_eq!(
        saved_region_positions(&sidecars.join("poi"))?,
        poi_positions
    );
    for (pos, original) in before_entities {
        let rewritten = reader
            .read_entity_chunk(pos.x, pos.z)?
            .context("rewritten entity chunk missing")?
            .document;
        assert!(
            rewritten.name == original.name && same_nbt(&rewritten.value, &original.value),
            "entity chunk {pos:?} differs"
        );
    }
    for (pos, original) in before_pois {
        let rewritten = reader
            .read_poi_chunk(pos.x, pos.z)?
            .context("rewritten POI chunk missing")?
            .document;
        assert!(
            rewritten.name == original.name && same_nbt(&rewritten.value, &original.value),
            "POI chunk {pos:?} differs"
        );
    }
    assert_eq!(
        saved_terrain_positions(
            &transaction_world
                .path()
                .join("dimensions/minecraft/overworld/region")
        )?,
        positions
    );
    for (position, before) in before_chunks {
        let after = reader
            .read_chunk(position.x, position.z)?
            .with_context(|| format!("rewritten chunk {position:?} missing"))?;
        assert_eq!(before.status, after.status, "chunk {position:?}");
        assert_eq!(
            before.document.name, after.document.name,
            "chunk {position:?} NBT name"
        );
        assert!(
            same_chunk_metadata(&before.document.value, &after.document.value),
            "chunk {position:?} metadata differs between staging and Java round-trip: {:?} / {:?}",
            chunk_metadata(&before.document.value),
            chunk_metadata(&after.document.value)
        );
        for key in [
            "DataVersion",
            "xPos",
            "yPos",
            "zPos",
            "structures",
            "block_entities",
            "block_ticks",
            "fluid_ticks",
            "Heightmaps",
            "PostProcessing",
        ] {
            assert!(
                same_optional_nbt(
                    before.document.value.get(key),
                    after.document.value.get(key)
                ),
                "chunk {position:?} field {key} differs between staging and Java round-trip"
            );
        }
        assert_eq!(
            before.block_light_correct(),
            after.block_light_correct(),
            "chunk {position:?}"
        );
        let before_world = materialize(position, before.clone())?;
        let after_world = materialize(position, after.clone())?;
        for y in -80..336 {
            for z in position.z * 16..position.z * 16 + 16 {
                for x in position.x * 16..position.x * 16 + 16 {
                    assert_eq!(
                        before_world.block(x, y, z),
                        after_world.block(x, y, z),
                        "block {x} {y} {z}"
                    );
                    assert_eq!(
                        before.biome(x, y, z),
                        after.biome(x, y, z),
                        "biome {x} {y} {z}"
                    );
                    for sky in [false, true] {
                        assert_eq!(
                            before.light(x, y, z, sky),
                            after.light(x, y, z, sky),
                            "light {x} {y} {z}, sky={sky}"
                        );
                    }
                }
            }
        }
    }
    if seed == 1 {
        use native_session::{GameSession, PlayerCommand, SystemMessageKind};

        let staged_world_path = stage.path().to_path_buf();
        let stream_reader = native_persistence::world::WorldReader::open_for_session(
            &staged_world_path,
            dimension.clone(),
        )?;
        let stream_guard = stream_reader
            .session_guard()
            .context("streaming world lock missing")?;
        let mut streamer = crate::chunk_stream::ChunkStreamer::start(
            stream_reader,
            defaults.clone(),
            4,
            BTreeSet::new(),
            None,
        )?;
        let session = &mut stage.initial.session;
        let prior_errors = session.take_stream_errors();
        ensure!(
            prior_errors.is_empty(),
            "initial session errors: {prior_errors:?}"
        );
        let initial_tick = session.snapshot().tick;
        let initial_position = native_session::player_data::read_saved_position(&player)?
            .context("initial player position missing")?;
        session.submit_command(PlayerCommand::QueryWorldSeed);
        session.submit_command(PlayerCommand::ToggleInventory);
        session.submit_command(PlayerCommand::Move {
            forward: 1.0,
            strafe: 0.0,
            jump: false,
        });
        session.update(Duration::from_millis(50));
        let opened = session.snapshot();
        assert_eq!(opened.tick, initial_tick + 1);
        assert!(opened.inventory.is_open());
        assert!(opened.system_messages.iter().any(|message| matches!(
            message.kind, SystemMessageKind::WorldSeed(value) if value == seed
        )));
        session.submit_command(PlayerCommand::ToggleInventory);
        session.submit_command(PlayerCommand::Move {
            forward: 0.0,
            strafe: 0.0,
            jump: false,
        });
        for _ in 0..19 {
            session.update(Duration::from_millis(50));
        }
        let after = session.snapshot();
        assert_eq!(after.tick, initial_tick + 20);
        assert!(!after.inventory.is_open());
        assert!(after.player.position.is_finite());
        let errors = session.take_stream_errors();
        ensure!(
            errors.is_empty(),
            "generated session tick errors: {errors:?}"
        );
        let actual_position = after.origin.as_dvec3() + after.player.position.as_dvec3();
        ensure!(
            actual_position != initial_position,
            "player position did not change"
        );
        let waiting: Vec<_> = session.pending_fence_update_chunks().collect();
        let available: Vec<_> = waiting
            .iter()
            .copied()
            .filter(|pos| positions.contains(&ChunkPos { x: pos.x, z: pos.z }))
            .collect();
        ensure!(
            !available.is_empty(),
            "seed 1 did not exercise streaming a saved pending chunk"
        );
        eprintln!("Fence updates await {waiting:?}; saved terrain is available for {available:?}");
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let mut settled_ticks = 0;
        while settled_ticks < 10 {
            ensure!(
                std::time::Instant::now() < deadline,
                "streaming saved shape dependencies timed out"
            );
            streamer.service(session)?;
            session.update(Duration::from_millis(50));
            let errors = session.take_stream_errors();
            ensure!(
                errors.is_empty(),
                "generated session streaming errors: {errors:?}"
            );
            // Updates may expose more dependencies as chunks arrive. Wait for
            // all saved ones, including the fence and source chunks, rather
            // than only the chunks originally listed as waiting.
            let saved_pending = session
                .required_shape_update_chunks()
                .into_iter()
                .any(|pos| {
                    positions.contains(&ChunkPos { x: pos.x, z: pos.z })
                        && !session.has_resident_or_parked_chunk(pos)
                });
            settled_ticks = if saved_pending { 0 } else { settled_ticks + 1 };
            std::thread::sleep(Duration::from_millis(10));
        }
        let missing_dependencies: Vec<_> = session
            .required_shape_update_chunks()
            .into_iter()
            .filter(|pos| !positions.contains(&ChunkPos { x: pos.x, z: pos.z }))
            .collect();
        eprintln!(
            "Fence updates still waiting after streaming: {:?}; dependencies not staged: {missing_dependencies:?}",
            session.pending_fence_update_chunks().collect::<Vec<_>>(),
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let generation_targets = loop {
            streamer.service(session)?;
            let required_missing: Vec<_> = session
                .required_shape_update_chunks()
                .into_iter()
                .filter(|pos| !positions.contains(&ChunkPos { x: pos.x, z: pos.z }))
                .collect();
            if let Some(targets) = streamer.missing_generation_targets(session, usize::MAX) {
                if !required_missing.is_empty() && targets.targets().starts_with(&required_missing)
                {
                    ensure!(
                        targets.is_current(&streamer, session),
                        "generation targets are stale"
                    );
                    eprintln!(
                        "Missing shape dependencies prioritized for generation: {required_missing:?}"
                    );
                    let first = streamer
                        .missing_generation_targets(session, 1)
                        .context("single generation target missing")?;
                    ensure!(
                        first.targets() == [required_missing[0]],
                        "first generation target does not match the deferred update"
                    );
                    break first;
                }
            }
            ensure!(
                std::time::Instant::now() < deadline,
                "missing shape generation targets timed out"
            );
            session.update(Duration::from_millis(50));
            let errors = session.take_stream_errors();
            ensure!(
                errors.is_empty(),
                "generation target streaming errors: {errors:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        if check != GenerationCheck::RoundTrip {
            let prepared = prepare_missing_generation(
                generation_targets,
                &streamer,
                session,
                &staged_world_path,
                &dimension,
                &stream_guard,
                &game,
            )?;
            if check == GenerationCheck::GenerationWorkers {
                use native_session::worldgen::schedule::GenerationStage;

                ensure!(
                    !session.dirty_chunk_positions().is_empty(),
                    "seed 1 fixture has no live dirty chunks"
                );
                let mut prepared = prepared;
                let mut target = *prepared
                    .targets()
                    .first()
                    .context("missing target absent")?;
                let retry_deadline = std::time::Instant::now() + Duration::from_secs(120);
                let mut biomes = loop {
                    match crate::chunk_stream::MissingGenerationBiomeWorker::start(
                        prepared, &streamer, session,
                    ) {
                        Ok(worker) => break worker,
                        Err(failure) => {
                            let Some(pending) = failure.pending() else {
                                return Err(failure.error.context("cannot start biome generation"));
                            };
                            eprintln!("Waiting for live generation input: {pending}");
                            prepared = failure.prepared;
                            ensure!(
                                std::time::Instant::now() < retry_deadline,
                                "pending terrain input did not settle"
                            );
                            session.update(Duration::from_millis(50));
                            let errors = session.take_stream_errors();
                            ensure!(errors.is_empty(), "streaming retry errors: {errors:?}");
                            streamer.service(session)?;
                            if !prepared.is_current(&streamer, session) {
                                crate::generation_staging::discard_async(prepared);
                                let targets = loop {
                                    if let Some(targets) =
                                        streamer.missing_generation_targets(session, 1)
                                        && !targets.targets().is_empty()
                                    {
                                        break targets;
                                    }
                                    ensure!(
                                        std::time::Instant::now() < retry_deadline,
                                        "generation target retry timed out"
                                    );
                                    session.update(Duration::from_millis(50));
                                    let errors = session.take_stream_errors();
                                    ensure!(
                                        errors.is_empty(),
                                        "streaming retry errors: {errors:?}"
                                    );
                                    streamer.service(session)?;
                                    std::thread::sleep(Duration::from_millis(10));
                                };
                                prepared = prepare_missing_generation(
                                    targets,
                                    &streamer,
                                    session,
                                    &staged_world_path,
                                    &dimension,
                                    &stream_guard,
                                    &game,
                                )?;
                                target = *prepared
                                    .targets()
                                    .first()
                                    .context("missing target absent")?;
                            }
                            std::thread::sleep(Duration::from_millis(10));
                        }
                    }
                };
                let deadline = std::time::Instant::now() + Duration::from_secs(120);
                let biomed = loop {
                    if let Some(result) = biomes.try_result(&streamer, session)? {
                        break result;
                    }
                    ensure!(
                        std::time::Instant::now() < deadline,
                        "missing chunk biome generation timed out"
                    );
                    std::thread::sleep(Duration::from_millis(2));
                };
                ensure!(biomed.targets() == [target], "biome targets changed");
                let mut terrain = crate::chunk_stream::MissingGenerationTerrainWorker::start(
                    biomed,
                    &streamer,
                    session,
                    usize::MAX,
                    BTreeMap::new(),
                )
                .map_err(|failure| failure.error)?;
                let deadline = std::time::Instant::now() + Duration::from_secs(120);
                let generated = loop {
                    if let Some(result) = terrain.try_result(&streamer, session)? {
                        break result;
                    }
                    ensure!(
                        std::time::Instant::now() < deadline,
                        "missing chunk terrain generation timed out"
                    );
                    std::thread::sleep(Duration::from_millis(2));
                };
                ensure!(
                    generated.generated().first() == Some(&target),
                    "target terrain was not generated"
                );
                ensure!(generated.remaining() == 0, "terrain requirements remain");
                eprintln!(
                    "Generated {} terrain chunks and restored {} saved terrain chunks",
                    generated.generated().len(),
                    generated.saved().len()
                );
                let saved_positions: BTreeSet<_> = generated
                    .saved()
                    .iter()
                    .map(|(position, _)| *position)
                    .collect();
                let completed_saved = generated.saved().iter().find_map(|(position, stage)| {
                    (*stage >= GenerationStage::Features).then_some(*position)
                });
                let failure = match crate::chunk_stream::MissingGenerationAssemblyWorker::start(
                    generated, &streamer, session,
                ) {
                    Ok(_) => anyhow::bail!("dirty live terrain was accepted for assembly"),
                    Err(failure) => failure,
                };
                eprintln!("Live assembly remains gated: {:#}", failure.error);
                let mut assembly =
                    crate::chunk_stream::MissingGenerationStagedAssemblyWorker::start(
                        failure.terrain,
                        &streamer,
                        session,
                    )
                    .map_err(|failure| failure.error)?;
                let deadline = std::time::Instant::now() + Duration::from_secs(120);
                let staged = loop {
                    if let Some(result) = assembly.try_result(&streamer, session)? {
                        break result;
                    }
                    ensure!(
                        std::time::Instant::now() < deadline,
                        "staged live terrain assembly timed out"
                    );
                    std::thread::sleep(Duration::from_millis(2));
                };
                ensure!(
                    staged.is_current(&streamer, session),
                    "staged live terrain is stale"
                );
                ensure!(
                    staged.completed_stage(target) == Some(GenerationStage::Terrain),
                    "target did not reach terrain stage"
                );
                let staged_world = staged.snapshot();
                let snapshot = session.snapshot();
                let live_world = snapshot.world.loaded().context("loaded world missing")?;
                let mut edit_count = 0;
                for edit in live_world.block_edits() {
                    if !saved_positions.contains(&edit.position.chunk()) {
                        continue;
                    }
                    let pos = edit.position;
                    ensure!(
                        staged_world.block(pos.x, pos.y, pos.z) == Some(&edit.replacement),
                        "staged terrain differs from live block at {pos:?}"
                    );
                    ensure!(
                        staged_world.light(pos.x, pos.y, pos.z, false).is_none()
                            && staged_world.light(pos.x, pos.y, pos.z, true).is_none(),
                        "unverified lighting leaked into staged terrain at {pos:?}"
                    );
                    edit_count += 1;
                }
                ensure!(
                    edit_count > 0,
                    "fixture has no live edits in saved terrain inputs"
                );
                use native_persistence::heightmap::{
                    HeightBounds, SavedGenerationHeightmapKind as SavedKind,
                    SavedGenerationHeightmaps, SpawnHeightmapKind as LiveKind,
                };
                let mut height_count = 0;
                for position in &saved_positions {
                    let Some(live_maps) = session.spawn_heightmaps_for_chunk(*position) else {
                        continue;
                    };
                    let document = native_persistence::nbt::NamedTag {
                        name: "".into(),
                        value: native_persistence::nbt::Tag::Compound(vec![(
                            "Heightmaps".into(),
                            staged.heightmaps(*position)?,
                        )]),
                    };
                    let staged_maps = SavedGenerationHeightmaps::read(
                        &document,
                        HeightBounds {
                            min_y: -64,
                            max_y: 320,
                        },
                    )?;
                    for (saved_kind, live_kind) in [
                        (SavedKind::WorldSurface, LiveKind::WorldSurface),
                        (SavedKind::OceanFloor, LiveKind::OceanFloor),
                        (SavedKind::MotionBlocking, LiveKind::MotionBlocking),
                    ] {
                        for z in 0..16 {
                            for x in 0..16 {
                                let staged_height = staged_maps
                                    .first_available(saved_kind, x, z)
                                    .map(|height| height - 1);
                                let live_height = live_maps.highest(live_kind, x, z);
                                if staged_height.is_some() && live_height.is_some() {
                                    ensure!(
                                        staged_height == live_height,
                                        "heightmap differs at {position:?} ({x}, {z}): {saved_kind:?}"
                                    );
                                    height_count += 1;
                                }
                            }
                        }
                    }
                }
                eprintln!(
                    "Staged {edit_count} live block edits and compared {height_count} live heights"
                );
                let biome = staged
                    .biome_chunk(target)
                    .context("target biome volume is missing")?;
                let biome_source = oracle.path().join("GeneratedMissingBiomeOracle.java");
                std::fs::write(
                    &biome_source,
                    include_str!("GeneratedMissingBiomeOracle.java"),
                )?;
                let compiled = Command::new(java.join("javac.exe"))
                    .arg("-classpath")
                    .arg(&classpath)
                    .arg(&biome_source)
                    .output()?;
                ensure!(
                    compiled.status.success(),
                    "Java biome oracle compilation failed: {}",
                    String::from_utf8_lossy(&compiled.stderr)
                );
                let compared = Command::new(java.join("java.exe"))
                    .arg("-classpath")
                    .arg(&classpath)
                    .arg("GeneratedMissingBiomeOracle")
                    .args([seed.to_string(), target.x.to_string(), target.z.to_string()])
                    .output()?;
                ensure!(
                    compared.status.success(),
                    "Java biome oracle failed: {} {}",
                    String::from_utf8_lossy(&compared.stdout),
                    String::from_utf8_lossy(&compared.stderr)
                );
                let output = std::str::from_utf8(&compared.stdout)?;
                let header = output
                    .lines()
                    .find_map(|line| line.strip_prefix("V "))
                    .context("Java biome height is missing")?;
                ensure!(
                    header == format!("{} {}", biome.min_y(), biome.height()),
                    "Java biome height differs"
                );
                let quart_height = usize::try_from(biome.height() / 4)?;
                let mut columns = BTreeSet::new();
                for row in output.lines().filter_map(|line| line.strip_prefix("B ")) {
                    let mut fields = row.split_whitespace();
                    let x: usize = fields.next().context("biome X is missing")?.parse()?;
                    let z: usize = fields.next().context("biome Z is missing")?.parse()?;
                    ensure!(
                        x < 4 && z < 4 && columns.insert((x, z)),
                        "invalid biome column"
                    );
                    for y in 0..quart_height {
                        let expected = fields.next().context("Java biome value is missing")?;
                        let actual = &biome.values()[(z * 4 + x) * quart_height + y];
                        ensure!(
                            *actual == native_resources::packs::ResourceId::parse(expected)?,
                            "generated biome differs at local quart ({x}, {y}, {z})"
                        );
                    }
                    ensure!(fields.next().is_none(), "extra Java biome values");
                }
                ensure!(columns.len() == 16, "Java biome columns are incomplete");
                eprintln!(
                    "Generated missing terrain at {target:?}; matched {} Java biome samples",
                    columns.len() * quart_height
                );
                let staged = if let Some(position) = completed_saved {
                    let failure =
                        match crate::chunk_stream::MissingGenerationStagedFeatureWorker::start(
                            staged, &streamer, session, position,
                        ) {
                            Ok(_) => anyhow::bail!("completed saved features were accepted again"),
                            Err(failure) => failure,
                        };
                    ensure!(
                        failure
                            .error
                            .to_string()
                            .contains("already completed features"),
                        "unexpected saved-feature error: {:#}",
                        failure.error
                    );
                    failure.terrain
                } else {
                    staged
                };
                let before_maps = staged.heightmaps(target)?;
                let mut features =
                    crate::chunk_stream::MissingGenerationStagedFeatureWorker::start(
                        staged, &streamer, session, target,
                    )
                    .map_err(|failure| failure.error)?;
                let deadline = std::time::Instant::now() + Duration::from_secs(120);
                let (featured, placements) = loop {
                    if let Some(result) = features.try_result(&streamer, session)? {
                        match result {
                            crate::chunk_stream::StagedMissingFeatureResult::Completed {
                                terrain,
                                position,
                                placements,
                            } => {
                                ensure!(position == target, "feature target changed");
                                break (terrain, placements);
                            }
                            crate::chunk_stream::StagedMissingFeatureResult::Failed {
                                terrain,
                                error,
                            } => {
                                ensure!(
                                    std::sync::Arc::ptr_eq(&staged_world, &terrain.snapshot()),
                                    "failed feature pass changed staged terrain"
                                );
                                ensure!(
                                    terrain.completed_stage(target)
                                        == Some(GenerationStage::Terrain)
                                        && terrain.heightmaps(target)? == before_maps,
                                    "failed feature pass changed staged generation state"
                                );
                                return Err(error.context("staged live features did not complete"));
                            }
                        }
                    }
                    ensure!(
                        std::time::Instant::now() < deadline,
                        "staged live feature generation timed out"
                    );
                    std::thread::sleep(Duration::from_millis(2));
                };
                ensure!(
                    featured.completed_stage(target) == Some(GenerationStage::Features),
                    "target did not reach the feature stage"
                );
                ensure!(
                    !placements.is_empty(),
                    "feature plan was unexpectedly empty"
                );
                eprintln!(
                    "Ran {} staged feature placements for {target:?}; {} reported success",
                    placements.len(),
                    placements.iter().filter(|placed| **placed).count()
                );
                ensure!(
                    featured.feature_order() == [target],
                    "feature order was not retained"
                );
                let mut featured = featured;
                while let Some(position) = featured.ready_features().first().copied() {
                    let before_world = featured.snapshot();
                    let before_order = featured.feature_order().to_vec();
                    let mut worker =
                        crate::chunk_stream::MissingGenerationStagedFeatureWorker::start(
                            featured, &streamer, session, position,
                        )
                        .map_err(|failure| failure.error)?;
                    let deadline = std::time::Instant::now() + Duration::from_secs(120);
                    featured = loop {
                        if let Some(result) = worker.try_result(&streamer, session)? {
                            match result {
                                crate::chunk_stream::StagedMissingFeatureResult::Completed {
                                    terrain,
                                    position: completed,
                                    ..
                                } => {
                                    ensure!(completed == position, "feature target changed");
                                    ensure!(
                                        terrain.feature_order().last() == Some(&position),
                                        "completed feature order was not retained"
                                    );
                                    break terrain;
                                }
                                crate::chunk_stream::StagedMissingFeatureResult::Failed {
                                    terrain,
                                    error,
                                } => {
                                    ensure!(
                                        std::sync::Arc::ptr_eq(&before_world, &terrain.snapshot())
                                            && terrain.feature_order() == before_order,
                                        "failed feature pass changed staged terrain or order"
                                    );
                                    return Err(error.context(format!(
                                        "staged neighboring features did not complete at {position:?}"
                                    )));
                                }
                            }
                        }
                        ensure!(
                            std::time::Instant::now() < deadline,
                            "neighbor feature generation timed out"
                        );
                        std::thread::sleep(Duration::from_millis(2));
                    };
                }
                ensure!(
                    featured.unfinished_features().is_empty(),
                    "feature centers remain blocked: {:?}",
                    featured.unfinished_features()
                );
                eprintln!(
                    "Prepared {} feature centers in recorded test order",
                    featured.feature_order().len()
                );
                return Ok(());
            }
            let (schedule, generator, restored, observed) =
                prepared.into_checked_parts(&streamer, session)?;
            if check == GenerationCheck::StagedTerrain {
                ensure!(
                    !session.dirty_chunk_positions().is_empty(),
                    "seed 1 fixture has no live dirty chunks"
                );
                ensure!(
                    generator.capture_live_terrain(session, &restored).is_err(),
                    "seed 1 fixture did not exercise the strict live guard"
                );
                let stage = generator
                    .capture_live_terrain_stage(session, &restored, &schedule)
                    .context("cannot stage seed 1 live terrain")?;
                generator.verify_live_terrain_stage(session, &stage)?;
                ensure!(
                    observed.is_current(&streamer, session),
                    "generation observation changed"
                );
                let live = session.snapshot();
                let world = live
                    .world
                    .loaded()
                    .context("loaded world snapshot missing")?;
                let edited = world
                    .block_edits()
                    .find(|edit| {
                        stage
                            .block(edit.position.x, edit.position.y, edit.position.z)
                            .is_some()
                            && world
                                .block(edit.position.x, edit.position.y, edit.position.z)
                                .is_some()
                    })
                    .context("no live block edit was included in staged terrain")?;
                ensure!(
                    stage.block(edited.position.x, edited.position.y, edited.position.z)
                        == Some(&edited.replacement),
                    "staged terrain does not include the live block edit"
                );
                let biome = world
                    .biome(edited.position.x, edited.position.y, edited.position.z)
                    .context("edited block biome is unavailable")?;
                ensure!(
                    stage.biome(edited.position.x, edited.position.y, edited.position.z)
                        == Some(biome),
                    "staged terrain biome differs from the live world"
                );
                eprintln!(
                    "Staged dirty chunks: {:?}; missing sky chunks: {:?}",
                    session.dirty_chunk_positions(),
                    session.missing_sky_chunks()
                );
                return Ok(());
            }
            let deadline = std::time::Instant::now() + Duration::from_secs(15);
            let live_inputs = loop {
                match generator.capture_live_inputs_with_deferred_shapes(session, &restored) {
                    Ok(inputs) => break inputs,
                    Err(error)
                        if error.to_string().contains("sky lighting is not settled")
                            && std::time::Instant::now() < deadline =>
                    {
                        streamer.service(session)?;
                        session.update(Duration::from_millis(50));
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(error) => {
                        eprintln!(
                            "Dirty chunks during generation: {:?}",
                            session.dirty_chunk_positions()
                        );
                        eprintln!(
                            "Missing sky-light chunks during generation: {:?}",
                            session.missing_sky_chunks()
                        );
                        return Err(error.context("cannot bind seed 1 live generation inputs"));
                    }
                }
            };
            ensure!(
                observed.is_current(&streamer, session),
                "seed 1 generation observation changed while lighting settled"
            );
            generator.verify_live_inputs(session, &live_inputs)?;
        }
        match session.draft_new_player_state() {
            Ok(draft) => {
                let ticked_player_file = oracle.path().join("ticked-player.nbt");
                std::fs::write(
                    &ticked_player_file,
                    native_persistence::nbt::write(draft.document())?,
                )?;
                let checked = Command::new(java.join("java.exe"))
                    .arg("-classpath")
                    .arg(&classpath)
                    .arg("GeneratedWorldJavaOracle")
                    .arg("--player")
                    .arg(&ticked_player_file)
                    .output()?;
                ensure!(
                    checked.status.success()
                        && String::from_utf8_lossy(&checked.stdout)
                            .lines()
                            .any(|line| line == "PLAYER_OK"),
                    "Java ticked player check failed: {} {}",
                    String::from_utf8_lossy(&checked.stdout),
                    String::from_utf8_lossy(&checked.stderr)
                );
                eprintln!("Java round-tripped the ticked generated player");
            }
            Err(error) => {
                let message = error.to_string();
                ensure!(
                    message.contains("fence shape updates")
                        && message.contains("unresolved: false")
                        && message.contains("waiting for chunks: [ChunkPos"),
                    "unexpected generated player save refusal: {error:#}"
                );
                ensure!(
                    !missing_dependencies.is_empty(),
                    "player save refused after all saved shape dependencies were loaded: {error:#}"
                );
                eprintln!("Ticked player save correctly refused: {error:#}");
            }
        }
        eprintln!(
            "Generated seed 1 session advanced at least 20 ticks and handled movement, inventory and seed commands"
        );
    }
    eprintln!(
        "Java round-tripped {} terrain, {} entity and {} POI chunks; loaded and saved {} entities",
        positions.len(),
        entity_positions.len(),
        poi_positions.len(),
        entity_count
    );
    Ok(())
}
