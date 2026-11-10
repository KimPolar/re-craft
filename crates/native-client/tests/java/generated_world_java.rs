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
    generated_world_round_trips_through_java(-1)
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK; generates a temporary world"]
fn generated_world_seed_42_round_trips_through_java() -> Result<()> {
    generated_world_round_trips_through_java(42)
}

fn generated_world_round_trips_through_java(seed: i64) -> Result<()> {
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
    let stage = ready.stage_structure_initial_world(directory.path(), 123, 1)?;
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
    drop(reader);
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
    eprintln!(
        "Java round-tripped {} terrain, {} entity and {} POI chunks; loaded and saved {} entities",
        positions.len(),
        entity_positions.len(),
        poi_positions.len(),
        entity_count
    );
    Ok(())
}
