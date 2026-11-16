//! End-to-end save checks against a privately published generated world.
use std::{
    collections::BTreeSet,
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use native_persistence::world::{DimensionId, RegionStore, SavedWorldBorder, WorldReader};
use native_session::{GameSession, PlayerCommand};
use native_world::{
    fluid::FluidState,
    loaded::{BlockPosition, BlockState, ChunkPos},
};

use crate::generation_pipeline::{InitialWorldOptions, InitialWorldPipeline, InitialWorldResult};

/// Keep this private test world separate from the user's actual saves.
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and long-running world generation"]
fn generated_portal_world_preserves_remote_chunks_after_play_save() -> Result<()> {
    verify_generated_portal_play_save(0, false)
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and long-running world generation"]
fn generated_portal_world_saves_after_forty_ticks_without_random_ticks() -> Result<()> {
    verify_generated_portal_play_save(40, false)
}

/// Reuse an already generated private fixture to diagnose the gameplay and
/// save path without regenerating distant chunks on every attempt.
#[test]
#[ignore = "requires a private generated 26.3 test world"]
fn existing_generated_portal_world_saves_after_forty_ticks() -> Result<()> {
    let path = private_play_save_world()?;
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let reader = WorldReader::open_for_session(&path, DimensionId::parse("overworld")?)?;
    ensure!(reader.saved_world_seed()? == Some(-1), "wrong test seed");
    ensure!(
        reader.fluid_game_rules()?.random_tick_speed == 0,
        "test world must have random ticks disabled"
    );
    let lock = reader
        .session_guard()
        .context("test world session lock is missing")?;
    drop(reader);
    verify_portal_play_save(&path, &game, lock, 40)
}

fn private_play_save_world() -> Result<PathBuf> {
    let path = std::env::var_os("RECRAFT_PLAY_SAVE_TEST_WORLD")
        .context("RECRAFT_PLAY_SAVE_TEST_WORLD is not set")?;
    let root = std::fs::canonicalize(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/region-test-temp"),
    )?;
    let path = std::fs::canonicalize(path)?;
    ensure!(
        path != root && path.starts_with(&root),
        "test world is outside the private test directory"
    );
    Ok(path)
}

#[test]
#[ignore = "requires a private generated 26.3 test world"]
fn existing_generated_world_uses_captured_data_packs() -> Result<()> {
    let path = private_play_save_world()?;
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let reader = WorldReader::open_for_session(&path, DimensionId::parse("overworld")?)?;
    let selection = reader.data_pack_settings()?;
    let lock = reader
        .session_guard()
        .context("test world session lock is missing")?;
    drop(reader);
    let options = crate::ViewOptions {
        dimension: None,
        chunk: None,
        radius: 1,
        smoke_test: false,
        block_report: None,
        play: true,
        video: crate::menu_settings::VideoSettings::default(),
    };
    let crate::PreparedWorld::Play(prepared) =
        crate::prepare_world_with_session(&path, &game, &[], options, None, Some(lock))?
    else {
        anyhow::bail!("generated world did not open for play")
    };
    let snapshot = prepared
        .resources
        .data_pack_snapshot()
        .context("session pack snapshot missing")?;
    snapshot.verify()?;
    snapshot.verify_source(&path)?;
    let actual = prepared
        .resources
        .data_pack_snapshot_selection()
        .context("session pack selection missing")?;
    ensure!(
        actual.enabled == selection.enabled
            && actual.disabled == selection.disabled
            && actual.enabled_features == selection.enabled_features,
        "session pack selection changed"
    );
    Ok(())
}

#[test]
#[ignore = "requires the generated seed -1 portal fixture with FULL ticking neighbors"]
fn generated_saved_ticking_worker_preserves_live_callback_and_handles_effects() -> Result<()> {
    use crate::saved_ticking_worker::{SavedTickingRequest, SavedTickingWorker};
    use native_session::{
        loaded::BuildBounds,
        ticking_preparation::{
            FullFuture, FullFutureId, FullFutureResult, TickingFutureState, TickingPreparation,
            TickingPreparations, TickingRequestId, ticking_dependencies,
        },
        worldgen::GeneratedLavaRules,
    };
    use std::sync::Arc;

    let root = std::fs::canonicalize(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/region-test-temp"),
    )?;
    let path = std::fs::canonicalize(
        std::env::var_os("RECRAFT_SAVED_TICKING_TEST_WORLD").context("test world not set")?,
    )?;
    ensure!(
        path != root && path.starts_with(&root),
        "test world is outside the private directory"
    );
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let dimension = DimensionId::parse("overworld")?;
    let mut reader = WorldReader::open_for_session(&path, dimension.clone())?;
    ensure!(reader.saved_world_seed()? == Some(-1), "wrong test seed");
    let lock = reader.session_guard().context("test world lock missing")?;
    let center = ChunkPos { x: 20, z: 20 };
    let options = crate::ViewOptions {
        dimension: None,
        chunk: Some((center.x, center.z)),
        radius: 1,
        smoke_test: false,
        block_report: None,
        play: true,
        video: crate::menu_settings::VideoSettings::default(),
    };
    let crate::PreparedWorld::Play(prepared) =
        crate::prepare_world_with_session(&path, &game, &[], options, None, Some(lock))?
    else {
        anyhow::bail!("generated world did not open for play")
    };
    let crate::PreparedPlayWorld {
        mut session,
        mut resources,
        streamer,
        ..
    } = *prepared;
    drop(streamer);
    let packs = resources
        .data_pack_snapshot()
        .context("session packs missing")?;
    let selection = resources
        .data_pack_snapshot_selection()
        .context("pack selection missing")?
        .clone();
    let defaults = resources
        .vanilla_generated_block_registry()?
        .default_states();
    let rules = reader.fluid_game_rules()?;
    let make_request = |reader: &mut WorldReader| -> Result<SavedTickingRequest> {
        let area = reader.read_area(
            native_persistence::world::ChunkPos {
                x: center.x,
                z: center.z,
            },
            1,
        )?;
        ensure!(
            area.chunks.len() == 9 && area.chunks.values().all(|chunk| chunk.is_full()),
            "FULL neighbors missing"
        );
        let saved_terrain = Arc::new(area.materialize_with_defaults(&dimension, &defaults)?);
        Ok(SavedTickingRequest {
            minecraft_dir: game.clone(),
            dimension: dimension.clone(),
            bounds: BuildBounds {
                min_y: -64,
                max_y: 320,
            },
            area,
            saved_terrain,
            packs: Arc::clone(&packs),
            selection: selection.clone(),
            water_source_conversion: rules.water_source_conversion,
            lava_rules: GeneratedLavaRules {
                fast_lava: false,
                source_conversion: rules.lava_source_conversion,
            },
        })
    };
    let dependencies = ticking_dependencies(center);
    let original = TickingPreparation::start(center, |position| {
        Some(FullFuture {
            id: FullFutureId(dependencies.iter().position(|&entry| entry == position)? as u64),
            result: Some(FullFutureResult::Full),
        })
    })?;
    let before = session.snapshot();
    let before_world = Arc::clone(before.world.loaded().context("live world missing")?);
    let before_chunk = reader
        .read_chunk(center.x, center.z)?
        .context("saved center missing")?;
    let before_nbt = native_persistence::nbt::fingerprint(&before_chunk.document)?;
    let before_sub_tick = session.capture_saved_tick_queues()?.next_sub_tick();

    let mut cancelled = SavedTickingWorker::start(&original, &session, make_request(&mut reader)?)?;
    cancelled.cancel();
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        match cancelled.try_result() {
            Err(error) => {
                ensure!(
                    error.to_string().contains("cancelled"),
                    "unexpected cancellation: {error:#}"
                );
                break;
            }
            Ok(None) => {}
            Ok(Some(_)) => anyhow::bail!("cancelled work returned a result"),
        }
        ensure!(
            Instant::now() < deadline,
            "cancelled saved ticking worker timed out"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(original.state(), TickingFutureState::Waiting);
    assert!(original.has_queued_callback());

    let first = TickingRequestId(1);
    let second = TickingRequestId(2);
    let mut queue = TickingPreparations::default();
    for id in [first, second] {
        queue.start(id, center, |position| {
            Some(FullFuture {
                id: FullFutureId(dependencies.iter().position(|&entry| entry == position)? as u64),
                result: Some(FullFutureResult::Full),
            })
        })?;
    }
    let (expected, fork) = queue
        .fork_next_background()?
        .context("callback not queued")?;
    assert_eq!(expected, first);
    let mut worker = SavedTickingWorker::start(&fork, &session, make_request(&mut reader)?)?;
    let running = queue
        .begin_next_background(&fork)?
        .context("callback not queued")?;
    assert_eq!(running, first);
    assert_eq!(queue.cancel(first), Some(true));
    assert_eq!(queue.cancel(second), Some(true));
    assert!(queue.dispatch_next(|_| Ok(())).is_err());
    assert_eq!(queue.running_id(), Some(first));
    assert_eq!(queue.queued_len(), 1);
    let deadline = Instant::now() + Duration::from_secs(120);
    let completed = loop {
        if let Some(result) = worker.try_result()? {
            break result;
        }
        ensure!(Instant::now() < deadline, "saved ticking worker timed out");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(completed.result().region.chunks, BTreeSet::from([center]));
    let edits = completed.result().region.edits.len();
    let checks = completed.result().region.light_checks.len();
    let after_sub_tick = completed.result().region.ticks().next_sub_tick();
    eprintln!(
        "saved ticking worker prepared {center:?}: {edits} edits, {checks} light checks, sub-tick {before_sub_tick} -> {after_sub_tick}"
    );
    assert_eq!(edits, 0);
    assert_eq!(checks, 0);
    assert_eq!(after_sub_tick.wrapping_sub(before_sub_tick), 12);
    assert!(Arc::ptr_eq(
        session.snapshot().world.loaded().unwrap(),
        &before_world
    ));
    assert_eq!(queue.state(first), Some(TickingFutureState::Unavailable));
    let failure = match queue
        .finish_background(|callback| Ok(completed.apply_no_effect(&mut session, callback)))?
    {
        Some((id, Err(failure))) if id == first => failure,
        _ => anyhow::bail!("scheduled callback was accepted as a no-effect callback"),
    };
    let message = format!("{:#}", failure.error);
    ensure!(
        message.contains("postprocessing tick queues need live reconciliation"),
        "unexpected rejection: {message}"
    );
    assert_eq!(queue.state(first), Some(TickingFutureState::Unavailable));
    assert_eq!(queue.running_id(), Some(first));
    assert!(Arc::ptr_eq(
        session.snapshot().world.loaded().unwrap(),
        &before_world
    ));
    let retry = match queue.finish_background(|callback| {
        Ok(failure.prepared.apply_no_effect(&mut session, callback))
    })? {
        Some((id, Err(retry))) if id == first => retry,
        _ => anyhow::bail!("unchanged callback unexpectedly succeeded on retry"),
    };
    assert_eq!(format!("{:#}", retry.error), message);
    assert_eq!(queue.state(first), Some(TickingFutureState::Unavailable));
    assert_eq!(queue.running_id(), Some(first));
    let applied = queue
        .finish_background(|callback| Ok(retry.prepared.apply_scheduled(&mut session, callback)))?
        .context("running callback missing")?;
    assert_eq!(applied.0, first);
    if let Err(failure) = applied.1 {
        let error = failure.error;
        crate::generation_staging::discard_async(failure.prepared);
        return Err(error.context("could not apply generated saved ticking schedules"));
    }
    eprintln!("applied scheduled saved ticking callback to the live session");
    assert_eq!(queue.state(first), Some(TickingFutureState::Unavailable));
    assert_eq!(queue.running_id(), None);
    let drained = queue
        .dispatch_next(|callback| {
            callback.dispatch(
                |_| anyhow::bail!("cancelled callback postprocessed"),
                |_| anyhow::bail!("cancelled callback unpacked"),
                |_| anyhow::bail!("cancelled callback notified"),
            )
        })?
        .context("second callback missing")?;
    assert_eq!(drained.0, second);
    assert_eq!(queue.state(second), Some(TickingFutureState::Unavailable));
    assert_eq!(queue.queued_len(), 0);
    assert_eq!(
        session.capture_saved_tick_queues()?.next_sub_tick(),
        after_sub_tick
    );
    assert!(Arc::ptr_eq(
        session.snapshot().world.loaded().unwrap(),
        &before_world
    ));
    assert_eq!(
        native_persistence::nbt::fingerprint(
            &reader
                .read_chunk(center.x, center.z)?
                .context("saved center missing")?
                .document
        )?,
        before_nbt
    );
    Ok(())
}

#[test]
#[ignore = "requires a private generated 26.3 test world"]
fn generated_world_prepares_missing_terrain_while_saved_chunks_wait() -> Result<()> {
    use crate::chunk_stream::{
        MissingGenerationAssemblyWorker, MissingGenerationBiomeWorker,
        MissingGenerationCaptureWorker, MissingGenerationPrepareWorker,
        MissingGenerationStagedAssemblyWorker, MissingGenerationStagedFeatureWorker,
        MissingGenerationStagedLightWorker, MissingGenerationTerrainWorker, StagedLightOperation,
        StagedMissingFeatureResult, StagedMissingLightResult,
    };

    let path = private_play_save_world()?;
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let reader = WorldReader::open_for_session(&path, DimensionId::parse("overworld")?)?;
    ensure!(reader.saved_world_seed()? == Some(-1), "wrong test seed");
    let lock = reader
        .session_guard()
        .context("test world session lock is missing")?;
    drop(reader);
    let options = crate::ViewOptions {
        dimension: None,
        chunk: None,
        radius: 1,
        smoke_test: false,
        block_report: None,
        play: true,
        video: crate::menu_settings::VideoSettings::default(),
    };
    let crate::PreparedWorld::Play(prepared) =
        crate::prepare_world_with_session(&path, &game, &[], options, None, Some(lock.clone()))?
    else {
        anyhow::bail!("generated world did not open for play")
    };
    let crate::PreparedPlayWorld {
        mut session,
        mut streamer,
        ..
    } = *prepared;
    let deadline = Instant::now() + Duration::from_secs(20);
    let targets = loop {
        streamer.service(&mut session)?;
        session.update(Duration::from_millis(50));
        streamer.service(&mut session)?;
        let snapshot = session.snapshot();
        let world = std::sync::Arc::clone(snapshot.world.loaded().context("world missing")?);
        let center = session.player_chunk().context("player chunk missing")?;
        let blocked = session
            .capture_generation_terrain(&world, [center])
            .is_err_and(|error| error.to_string().contains("saved chunk preparation"));
        if blocked
            && session.pending_stream_data_positions().is_empty()
            && let Some(targets) = streamer.missing_generation_targets(&session, 1)
            && !targets.targets().is_empty()
        {
            break targets;
        }
        ensure!(
            Instant::now() < deadline,
            "saved preparation or missing target not observed"
        );
        std::thread::sleep(Duration::from_millis(2));
    };
    let target = targets.targets()[0];
    eprintln!("preparing missing chunk {target:?} beside the generated world");
    let reader = WorldReader::open_with_session(&path, DimensionId::parse("overworld")?, lock)?;
    let mut capture = MissingGenerationCaptureWorker::start(targets, &streamer, &session, reader)?;
    let deadline = Instant::now() + Duration::from_secs(60);
    let captured = loop {
        if let Some(result) = capture.try_result(&streamer, &session)? {
            break result;
        }
        ensure!(
            Instant::now() < deadline,
            "missing generation capture timed out"
        );
        std::thread::sleep(Duration::from_millis(2));
    };
    let mut prepare =
        MissingGenerationPrepareWorker::start(captured, &streamer, &session, game.clone())
            .map_err(|failure| failure.error)?;
    let deadline = Instant::now() + Duration::from_secs(90);
    let prepared = loop {
        if let Some(result) = prepare.try_result(&streamer, &session)? {
            break result;
        }
        ensure!(
            Instant::now() < deadline,
            "missing generation setup timed out"
        );
        std::thread::sleep(Duration::from_millis(2));
    };
    let mut biomes = MissingGenerationBiomeWorker::start(prepared, &streamer, &session)
        .map_err(|failure| failure.error)?;
    let deadline = Instant::now() + Duration::from_secs(120);
    let biomed = loop {
        if let Some(result) = biomes.try_result(&streamer, &session)? {
            break result;
        }
        ensure!(
            Instant::now() < deadline,
            "missing biome generation timed out"
        );
        std::thread::sleep(Duration::from_millis(2));
    };
    assert_eq!(biomed.targets(), &[target]);
    let mut terrain = MissingGenerationTerrainWorker::start(
        biomed,
        &streamer,
        &session,
        1,
        std::collections::BTreeMap::new(),
    )
    .map_err(|failure| failure.error)?;
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut generated = loop {
        if let Some(result) = terrain.try_result(&streamer, &session)? {
            break result;
        }
        ensure!(
            Instant::now() < deadline,
            "missing terrain generation timed out"
        );
        std::thread::sleep(Duration::from_millis(2));
    };
    assert_eq!(generated.generated(), &[target]);
    while generated.remaining() > 0 {
        let remaining = generated.remaining();
        eprintln!("preparing {remaining} additional terrain chunks");
        let state = generated.into_checked_state(&streamer, &session)?;
        let mut worker = MissingGenerationTerrainWorker::start(
            state,
            &streamer,
            &session,
            4,
            std::collections::BTreeMap::new(),
        )
        .map_err(|failure| failure.error)?;
        let deadline = Instant::now() + Duration::from_secs(180);
        generated = loop {
            if let Some(result) = worker.try_result(&streamer, &session)? {
                break result;
            }
            ensure!(
                Instant::now() < deadline,
                "neighbor terrain generation timed out"
            );
            std::thread::sleep(Duration::from_millis(2));
        };
        ensure!(
            generated.remaining() < remaining,
            "terrain generation made no progress"
        );
    }
    let rejected = match MissingGenerationAssemblyWorker::start(generated, &streamer, &session) {
        Ok(_) => anyhow::bail!("pending saved preparation unexpectedly passed the complete guard"),
        Err(failure) => failure,
    };
    ensure!(
        format!("{:#}", rejected.error).contains("saved chunk preparation"),
        "unexpected complete-guard error: {:#}",
        rejected.error
    );
    let mut assembly =
        MissingGenerationStagedAssemblyWorker::start(rejected.terrain, &streamer, &session)
            .map_err(|failure| failure.error)?;
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut staged = loop {
        if let Some(result) = assembly.try_result(&streamer, &session)? {
            break result;
        }
        ensure!(
            Instant::now() < deadline,
            "staged terrain assembly timed out"
        );
        std::thread::sleep(Duration::from_millis(2));
    };
    assert!(staged.snapshot().chunks.contains_key(&target));
    let biomes = staged
        .biome_chunk(target)
        .context("generated biomes missing")?;
    compare_private_generated_biomes_with_java(&game, target, &biomes)?;
    let live = session.snapshot();
    let live_world = std::sync::Arc::clone(live.world.loaded().context("world missing")?);
    let unfinished = staged.unfinished_features();
    ensure!(
        unfinished.contains(&target),
        "target features are not pending"
    );
    let mut completed = Vec::new();
    let mut placed = 0;
    while let Some(position) = staged.ready_features().first().copied() {
        let mut worker =
            MissingGenerationStagedFeatureWorker::start(staged, &streamer, &session, position)
                .map_err(|failure| failure.error)?;
        let deadline = Instant::now() + Duration::from_secs(120);
        let result = loop {
            if let Some(result) = worker.try_result(&streamer, &session)? {
                break result;
            }
            ensure!(
                Instant::now() < deadline,
                "staged feature generation timed out"
            );
            std::thread::sleep(Duration::from_millis(2));
        };
        match result {
            StagedMissingFeatureResult::Completed {
                terrain,
                position: finished,
                placements,
            } => {
                assert_eq!(finished, position);
                assert!(!placements.is_empty());
                placed += placements.iter().filter(|&&result| result).count();
                completed.push(position);
                staged = terrain;
                assert_eq!(staged.feature_order(), completed);
                assert_eq!(
                    staged.completed_stage(position),
                    Some(native_session::worldgen::schedule::GenerationStage::Features)
                );
            }
            StagedMissingFeatureResult::Failed { error, .. } => {
                return Err(error.context(format!("staged features failed for {position:?}")));
            }
        }
    }
    assert_eq!(completed.len(), unfinished.len());
    assert!(staged.unfinished_features().is_empty());
    assert!(completed.contains(&target));
    assert!(placed > 0, "no selected feature returned success");
    let mut cart_ids = BTreeSet::new();
    for position in staged.snapshot().chunks.keys() {
        for entity in staged.recorded_generated_entities(*position)? {
            use native_persistence::nbt::Tag;
            if !matches!(entity.get("id"), Some(Tag::String(id)) if id.matches("minecraft:chest_minecart"))
            {
                continue;
            }
            let Some(Tag::IntArray(uuid)) = entity.get("UUID") else {
                anyhow::bail!("generated mineshaft minecart has no UUID")
            };
            let uuid: [i32; 4] = uuid.as_slice().try_into()?;
            ensure!(uuid[1] as u32 & 0xf000 == 0x4000, "wrong UUID version");
            ensure!(
                uuid[2] as u32 & 0xc000_0000 == 0x8000_0000,
                "wrong UUID variant"
            );
            ensure!(cart_ids.insert(uuid), "duplicate generated minecart UUID");
            ensure!(
                matches!(entity.get("LootTable"), Some(Tag::String(id)) if id.matches("minecraft:chests/abandoned_mineshaft")),
                "generated mineshaft minecart has the wrong loot table"
            );
        }
    }
    eprintln!(
        "prepared features for {} chunks; {placed} selected features returned success; {} mineshaft minecarts recorded",
        completed.len(),
        cart_ids.len()
    );
    // A synthetic empty engine exercises worker ownership and scheduling only;
    // it is not an observed live engine or a Java light comparison.
    let failure = match MissingGenerationStagedLightWorker::start(
        staged,
        &streamer,
        &session,
        target,
        StagedLightOperation::Initialize,
    ) {
        Ok(_) => anyhow::bail!("light initialization accepted an unbound engine"),
        Err(failure) => failure,
    };
    ensure!(
        failure.error.to_string().contains("not bound"),
        "unexpected light error"
    );
    let mut staged = failure.terrain;
    let tracker = native_world::generation_light::QueuedSectionLightTracker::fresh(true, true, [])
        .context("synthetic light tracker is unavailable")?;
    staged.bind_observed_light(&streamer, &session, tracker, [], Default::default())?;
    let failure = match MissingGenerationStagedLightWorker::start(
        staged,
        &streamer,
        &session,
        target,
        StagedLightOperation::Propagate,
    ) {
        Ok(_) => anyhow::bail!("uninitialized light propagation was accepted"),
        Err(failure) => failure,
    };
    ensure!(
        failure.error.to_string().contains("incomplete"),
        "unexpected light dependency error"
    );
    let mut worker = MissingGenerationStagedLightWorker::start(
        failure.terrain,
        &streamer,
        &session,
        target,
        StagedLightOperation::Initialize,
    )
    .map_err(|failure| failure.error)?;
    let deadline = Instant::now() + Duration::from_secs(120);
    let staged = loop {
        if let Some(result) = worker.try_result(&streamer, &session)? {
            match result {
                StagedMissingLightResult::Completed {
                    terrain,
                    position,
                    operation,
                    visible,
                } => {
                    assert_eq!(position, target);
                    assert_eq!(operation, StagedLightOperation::Initialize);
                    assert!(visible.is_none());
                    break terrain;
                }
                StagedMissingLightResult::Failed { error, .. } => return Err(error),
            }
        }
        ensure!(
            Instant::now() < deadline,
            "staged light initialization timed out"
        );
        std::thread::sleep(Duration::from_millis(2));
    };
    assert_eq!(
        staged.completed_stage(target),
        Some(native_session::worldgen::schedule::GenerationStage::InitializeLight)
    );
    let failure = match MissingGenerationStagedLightWorker::start(
        staged,
        &streamer,
        &session,
        target,
        StagedLightOperation::Propagate,
    ) {
        Ok(_) => anyhow::bail!("light propagation accepted uninitialized neighbors"),
        Err(failure) => failure,
    };
    ensure!(
        failure.error.to_string().contains("incomplete"),
        "unexpected neighbor light error"
    );
    let staged = failure.terrain;
    let snapshot = session.snapshot();
    let world = std::sync::Arc::clone(snapshot.world.loaded().context("world missing")?);
    assert!(std::sync::Arc::ptr_eq(&live_world, &world));
    assert!(
        session
            .capture_generation_terrain(&world, [target])
            .is_err()
    );
    if let Some(next) = completed.into_iter().find(|&position| {
        position != target && staged.ready_queued_light(position, StagedLightOperation::Initialize)
    }) {
        let mut cancelled = MissingGenerationStagedLightWorker::start(
            staged,
            &streamer,
            &session,
            next,
            StagedLightOperation::Initialize,
        )
        .map_err(|failure| failure.error)?;
        cancelled.cancel();
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            match cancelled.try_result(&streamer, &session) {
                Err(error) => {
                    ensure!(
                        error.to_string().contains("cancelled"),
                        "unexpected cancellation error"
                    );
                    eprintln!("cancelled staged light initialization for {next:?}");
                    break;
                }
                Ok(None) => {}
                Ok(Some(_)) => anyhow::bail!("cancelled light work returned a result"),
            }
            ensure!(
                Instant::now() < deadline,
                "cancelled light work did not finish"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    Ok(())
}

fn compare_private_generated_biomes_with_java(
    game: &std::path::Path,
    target: ChunkPos,
    actual: &native_session::worldgen::QuartBiomeChunk,
) -> Result<()> {
    use std::process::Command;
    let oracle = tempfile::tempdir()?;
    let java = std::env::var_os("MINECRAFT_JAVA_HOME")
        .or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join(
                "AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon",
            )
        })
        .join("bin");
    let version = game.join("versions/26.3");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    ensure!(manifest["id"] == "26.3", "wrong Minecraft version");
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
    let source = oracle.path().join("GeneratedMissingBiomeOracle.java");
    std::fs::write(
        &source,
        include_str!("../tests/java/GeneratedMissingBiomeOracle.java"),
    )?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&classpath)
        .arg(source)
        .output()?;
    ensure!(
        compiled.status.success(),
        "Java biome oracle compilation failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(classpath)
        .arg("GeneratedMissingBiomeOracle")
        .args(["-1".to_owned(), target.x.to_string(), target.z.to_string()])
        .output()?;
    ensure!(
        output.status.success(),
        "Java biome oracle failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let output = std::str::from_utf8(&output.stdout)?;
    let header = output
        .lines()
        .find_map(|line| line.strip_prefix("V "))
        .context("Java biome height missing")?;
    let mut bounds = header.split_whitespace();
    let min_y: i32 = bounds.next().context("minimum height missing")?.parse()?;
    let height: i32 = bounds.next().context("height missing")?.parse()?;
    ensure!(
        height > 0 && height % 4 == 0 && bounds.next().is_none(),
        "invalid Java biome height"
    );
    ensure!(
        actual.chunk_position() == [target.x, target.z]
            && actual.min_y() == min_y
            && actual.height() == height,
        "generated biome bounds differ"
    );
    let mut columns = BTreeSet::new();
    let quart_height = (height / 4) as usize;
    for row in output.lines().filter_map(|line| line.strip_prefix("B ")) {
        let mut fields = row.split_whitespace();
        let x: usize = fields.next().context("biome X missing")?.parse()?;
        let z: usize = fields.next().context("biome Z missing")?.parse()?;
        ensure!(
            x < 4 && z < 4 && columns.insert((x, z)),
            "invalid biome column"
        );
        for y in 0..quart_height {
            let expected = fields.next().context("Java biome missing")?;
            let biome = &actual.values()[(z * 4 + x) * quart_height + y];
            ensure!(
                format!("{}:{}", biome.namespace, biome.path) == expected,
                "biome differs at {target:?}, local ({x}, {y}, {z})"
            );
        }
        ensure!(fields.next().is_none(), "extra Java biome values");
    }
    ensure!(columns.len() == 16, "Java biome columns are incomplete");
    eprintln!(
        "compared {} generated biome samples with Java for {target:?}",
        16 * quart_height
    );
    Ok(())
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and long-running world generation"]
fn generated_world_saved_full_chunk_can_resume_postprocessing() -> Result<()> {
    verify_generated_portal_play_save(0, true)
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and long-running world generation"]
fn generated_world_saved_ticking_future_uses_full_neighbors() -> Result<()> {
    if let Some(path) = std::env::var_os("RECRAFT_SAVED_TICKING_TEST_WORLD") {
        let root = std::fs::canonicalize(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/region-test-temp"),
        )?;
        let path = std::fs::canonicalize(path)?;
        ensure!(
            path.starts_with(&root),
            "test world is outside the private test directory"
        );
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let reader = WorldReader::open_for_session(&path, DimensionId::parse("overworld")?)?;
        let lock = reader
            .session_guard()
            .context("test world session lock is missing")?;
        drop(reader);
        return verify_saved_ticking(&path, &game, lock);
    }
    verify_generated_portal_play_save_inner(0, false, true)
}

fn verify_generated_portal_play_save(
    additional_ticks: u64,
    check_saved_postprocessing: bool,
) -> Result<()> {
    verify_generated_portal_play_save_inner(additional_ticks, check_saved_postprocessing, false)
}

fn verify_generated_portal_play_save_inner(
    additional_ticks: u64,
    check_saved_postprocessing: bool,
    check_saved_ticking: bool,
) -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let test_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/region-test-temp");
    std::fs::create_dir_all(&test_root)?;
    let parent = tempfile::Builder::new()
        .prefix("portal-play-save-")
        .tempdir_in(test_root)?;
    let parent_path = parent.path().to_path_buf();
    let mut settings = crate::world_creation::WorldCreationSettings::new("Test".into());
    settings.seed = crate::chat_input::ChatInput::field("-1");
    if additional_ticks != 0 {
        // Keep this test focused on scheduled fluid and block ticks. Default
        // random ticks still need complete sky-light and neighboring chunks.
        settings.game_rules.random_tick_speed = 0;
    }
    let mut targets = BTreeSet::from([
        ChunkPos { x: 20, z: 19 },
        ChunkPos { x: 21, z: 19 },
        ChunkPos { x: 28, z: 24 },
    ]);
    if check_saved_ticking {
        // Complete the eight surrounding chunks while leaving the center
        // deferred. Unrequested generated neighbors are not necessarily FULL.
        targets.clear();
        for dz in -1..=1 {
            for dx in -1..=1 {
                if dx != 0 || dz != 0 {
                    targets.insert(ChunkPos {
                        x: 20 + dx,
                        z: 20 + dz,
                    });
                }
            }
        }
    }
    let options = InitialWorldOptions {
        staging_parent: parent_path.clone(),
        border: SavedWorldBorder::default(),
        player_random_seed: 1234,
        ceiling_spawn_height: None,
        mesh_revision: 1,
        biome_blend: 0,
        last_played_ms: 123,
        region_timestamp: 1,
    };
    let mut pipeline = InitialWorldPipeline::start_with_additional_targets(
        settings.freeze(game.clone(), None),
        options,
        targets.clone(),
    )?;
    let start = Instant::now();
    let deadline = start + Duration::from_secs(900);
    let mut next_report = start + Duration::from_secs(30);
    let world = loop {
        match pipeline.poll() {
            Some(InitialWorldResult::Ready {
                generation,
                world,
                player_area,
            }) => {
                let area = player_area?;
                ensure!(
                    area.missing.is_empty() && area.deferred.is_empty(),
                    "initial player area is incomplete"
                );
                let processed = generation
                    .postprocessed
                    .as_ref()
                    .context("postprocessing missing")?;
                ensure!(
                    targets.is_subset(&processed.region.chunks),
                    "required chunks were not postprocessed"
                );
                break world;
            }
            Some(InitialWorldResult::Failed { error, .. }) => return Err(error),
            Some(InitialWorldResult::Cancelled { .. }) => {
                anyhow::bail!("initial generation cancelled")
            }
            None => {}
        }
        let now = Instant::now();
        if now >= next_report {
            eprintln!(
                "portal play-save after {:?}: {:?}",
                now - start,
                pipeline.progress()
            );
            next_report = now + Duration::from_secs(30);
        }
        ensure!(now < deadline, "initial world generation timed out");
        std::thread::sleep(Duration::from_millis(20));
    };
    let (_, stage) = world.into_parts();
    let published = stage.publish(&parent_path, "Test", "New World")?;
    let path = published.path.clone();
    let lock = published.session().clone();
    if check_saved_postprocessing {
        return verify_saved_postprocessing(&path, &game, lock);
    }
    if check_saved_ticking {
        let result = verify_saved_ticking(&path, &game, lock);
        if result.is_err() || std::env::var_os("RECRAFT_KEEP_GENERATED_WORLD").is_some() {
            let kept = parent.keep();
            eprintln!("kept private ticking test world: {}", kept.display());
        }
        return result;
    }
    let result = verify_portal_play_save(&path, &game, lock, additional_ticks);
    if result.is_err() && std::env::var_os("RECRAFT_KEEP_FAILED_GENERATED_WORLD").is_some() {
        let kept = parent.keep();
        eprintln!("kept private failed test world: {}", kept.display());
    }
    result
}

fn verify_saved_ticking(
    path: &std::path::Path,
    game: &std::path::Path,
    lock: native_persistence::save::WorldSessionGuard,
) -> Result<()> {
    use native_session::{
        loaded::BuildBounds,
        ticking_preparation::{
            FullFuture, FullFutureId, FullFutureResult, TickingFutureState, TickingPreparation,
            ticking_dependencies,
        },
        worldgen::{
            GeneratedLavaRules,
            saved_postprocessing::{SavedPostprocessingInputs, dispatch_saved_ticking},
        },
    };
    let dimension = DimensionId::parse("overworld")?;
    let mut reader = WorldReader::open_with_session(path, dimension.clone(), lock)?;
    let mut resources = native_resources::packs::ResourceStack::open(game)?;
    let ids = resources.vanilla_block_ids()?;
    let registry = resources.vanilla_generated_block_registry()?;
    let defaults = registry.default_states();
    let game_time = reader.game_time()?;
    let rules = reader.fluid_game_rules()?;
    let mut failures = Vec::new();
    let mut marked = 0;
    let mut incomplete = 0;
    for saved in reader.saved_chunk_positions(RegionStore::Terrain)? {
        let Some(chunk) = reader.read_chunk(saved.x, saved.z)? else {
            continue;
        };
        if !chunk.is_full() {
            continue;
        }
        let Some(post) = chunk.document.value.get("PostProcessing") else {
            continue;
        };
        if matches!(post, native_persistence::nbt::Tag::List { values, .. } if values.is_empty()) {
            continue;
        }
        let marks = native_persistence::postprocessing::decode(post, -64, 384)?;
        if marks.sections().iter().all(Vec::is_empty) {
            continue;
        }
        marked += 1;
        let area = reader.read_area(saved, 1)?;
        if area.chunks.len() != 9 || area.chunks.values().any(|chunk| !chunk.is_full()) {
            incomplete += 1;
            if failures.len() < 8 {
                let full = area.chunks.values().filter(|chunk| chunk.is_full()).count();
                failures.push(format!(
                    "{saved:?}: {full}/9 FULL, {} absent",
                    area.missing.len()
                ));
            }
            continue;
        }
        let pos = ChunkPos {
            x: saved.x,
            z: saved.z,
        };
        let registration = ticking_dependencies(pos);
        let fingerprints = area
            .chunks
            .iter()
            .map(|(position, chunk)| {
                Ok((
                    *position,
                    native_persistence::nbt::fingerprint(&chunk.document)?,
                ))
            })
            .collect::<Result<std::collections::BTreeMap<_, _>>>()?;
        let mut future = TickingPreparation::start(pos, |position| {
            Some(FullFuture {
                id: FullFutureId(
                    registration
                        .iter()
                        .position(|&entry| entry == position)
                        .unwrap() as u64,
                ),
                result: Some(FullFutureResult::Full),
            })
        })?;
        let result = dispatch_saved_ticking(
            &mut future,
            &mut resources,
            SavedPostprocessingInputs {
                area: &area,
                dimension: &dimension,
                defaults: &defaults,
                block_ids: &ids,
                bounds: BuildBounds {
                    min_y: -64,
                    max_y: 320,
                },
                registration: &registration,
                game_time,
                next_sub_tick: 0,
                light: None,
            },
            |state| {
                ensure!(
                    registry.is_canonical_state(&state.name, &state.properties),
                    "unknown block state: {state:?}"
                );
                Ok(())
            },
            |stage| {
                stage.set_water_source_conversion(rules.water_source_conversion)?;
                stage.set_lava_rules(GeneratedLavaRules {
                    fast_lava: false,
                    source_conversion: rules.lava_source_conversion,
                })?;
                Ok(())
            },
        );
        let completed = match result {
            Ok(Some(completed)) => completed,
            Ok(None) => {
                failures.push(format!("{pos:?}: ticking callback did not run"));
                continue;
            }
            Err(error) => {
                failures.push(format!("{pos:?}: {error:#}"));
                continue;
            }
        };
        ensure!(
            future.state() == TickingFutureState::Available,
            "ticking future did not complete"
        );
        ensure!(
            completed.region.chunks == BTreeSet::from([pos]),
            "wrong postprocessed chunk"
        );
        ensure!(
            completed.block_ticks.container(pos).is_some()
                && completed.fluid_ticks.container(pos).is_some(),
            "ticking queues are missing"
        );
        for (position, expected) in fingerprints {
            let source = reader
                .read_chunk(position.x, position.z)?
                .context("saved chunk disappeared")?;
            ensure!(
                native_persistence::nbt::fingerprint(&source.document)? == expected,
                "isolated ticking preparation modified its source"
            );
        }
        eprintln!("prepared generated saved chunk {pos:?} with its 3x3 FULL neighbors");
        return Ok(());
    }
    anyhow::bail!(
        "no generated saved chunk could complete 3x3 ticking preparation ({marked} marked, {incomplete} incomplete): {}",
        failures.join("; ")
    )
}

fn verify_saved_postprocessing(
    path: &std::path::Path,
    game: &std::path::Path,
    lock: native_persistence::save::WorldSessionGuard,
) -> Result<()> {
    use native_session::{
        loaded::BuildBounds,
        worldgen::{
            GeneratedLavaRules, PostprocessingAttempt,
            saved_postprocessing::{SavedPostprocessingInputs, begin_saved_postprocessing},
        },
    };
    let dimension = DimensionId::parse("overworld")?;
    let mut reader = WorldReader::open_with_session(path, dimension.clone(), lock)?;
    let mut resources = native_resources::packs::ResourceStack::open(game)?;
    let ids = resources.vanilla_block_ids()?;
    let registry = resources.vanilla_generated_block_registry()?;
    let defaults = registry.default_states();
    let game_time = reader.game_time()?;
    let rules = reader.fluid_game_rules()?;
    let mut failures = Vec::new();
    for saved_pos in reader.saved_chunk_positions(RegionStore::Terrain)? {
        let Some(chunk) = reader.read_chunk(saved_pos.x, saved_pos.z)? else {
            continue;
        };
        if !chunk.is_full() {
            continue;
        }
        let Some(post) = chunk.document.value.get("PostProcessing") else {
            continue;
        };
        if matches!(post, native_persistence::nbt::Tag::List { values, .. } if values.is_empty()) {
            continue;
        }
        let marks = native_persistence::postprocessing::decode(post, -64, 384)?;
        if marks.sections().iter().all(Vec::is_empty) {
            continue;
        }
        let before = native_persistence::nbt::fingerprint(&chunk.document)?;
        let mut area = reader.read_area(saved_pos, 0)?;
        let pos = ChunkPos {
            x: saved_pos.x,
            z: saved_pos.z,
        };
        let mut registration = vec![pos];
        let mut fingerprints = std::collections::BTreeMap::from([(pos, before)]);
        loop {
            let mut stage = match begin_saved_postprocessing(
                &mut resources,
                SavedPostprocessingInputs {
                    area: &area,
                    dimension: &dimension,
                    defaults: &defaults,
                    block_ids: &ids,
                    bounds: BuildBounds {
                        min_y: -64,
                        max_y: 320,
                    },
                    registration: &registration,
                    game_time,
                    next_sub_tick: 0,
                    light: None,
                },
                |state| {
                    ensure!(
                        registry.is_canonical_state(&state.name, &state.properties),
                        "unknown block state: {state:?}"
                    );
                    Ok(())
                },
            ) {
                Ok(stage) => stage,
                Err(error) => {
                    failures.push(format!("{pos:?}: {error:#}"));
                    break;
                }
            };
            stage.select_initial_targets(BTreeSet::from([pos]))?;
            stage.set_water_source_conversion(rules.water_source_conversion)?;
            stage.set_lava_rules(GeneratedLavaRules {
                fast_lava: false,
                source_conversion: rules.lava_source_conversion,
            })?;
            let outcome = loop {
                match stage.try_poll(pos, 256) {
                    Ok(PostprocessingAttempt::Progress { complete: false }) => {}
                    other => break other,
                }
            };
            match outcome {
                Ok(PostprocessingAttempt::Progress { complete: true }) => {
                    let completed = stage.finish()?;
                    ensure!(
                        completed.chunks.contains(&pos),
                        "target was not postprocessed"
                    );
                    for (source_pos, expected) in &fingerprints {
                        let after = reader
                            .read_chunk(source_pos.x, source_pos.z)?
                            .context("saved chunk disappeared")?;
                        ensure!(
                            native_persistence::nbt::fingerprint(&after.document)? == *expected,
                            "isolated postprocessing changed the source chunk"
                        );
                    }
                    eprintln!("finished saved postprocessing for generated chunk {pos:?}");
                    return Ok(());
                }
                Ok(PostprocessingAttempt::NeedsChunk(missing)) => {
                    if registration.len() >= 32 || registration.contains(&missing) {
                        failures.push(format!("{pos:?}: cannot add neighbor {missing:?}"));
                        break;
                    }
                    let Some(neighbor) = reader.read_chunk(missing.x, missing.z)? else {
                        failures.push(format!("{pos:?}: missing neighbor {missing:?}"));
                        break;
                    };
                    if !neighbor.is_full() {
                        failures.push(format!("{pos:?}: neighbor {missing:?} is not FULL"));
                        break;
                    }
                    let fingerprint = native_persistence::nbt::fingerprint(&neighbor.document)?;
                    let saved = native_persistence::world::ChunkPos {
                        x: missing.x,
                        z: missing.z,
                    };
                    area.chunks.insert(saved, std::sync::Arc::new(neighbor));
                    fingerprints.insert(missing, fingerprint);
                    registration.push(missing);
                }
                Ok(PostprocessingAttempt::Progress { complete: false }) => unreachable!(),
                Err(error) => {
                    failures.push(format!("{pos:?}: {error:#}"));
                    break;
                }
            }
        }
    }
    anyhow::bail!(
        "no generated FULL chunk could resume postprocessing: {}",
        failures.join("; ")
    )
}

fn verify_portal_play_save(
    path: &std::path::Path,
    game: &std::path::Path,
    lock: native_persistence::save::WorldSessionGuard,
    additional_ticks: u64,
) -> Result<()> {
    let options = crate::ViewOptions {
        dimension: None,
        chunk: None,
        radius: 1,
        smoke_test: false,
        block_report: None,
        play: true,
        video: crate::menu_settings::VideoSettings::default(),
    };
    let crate::PreparedWorld::Play(prepared) =
        crate::prepare_world_with_session(&path, &game, &[], options, None, Some(lock.clone()))?
    else {
        anyhow::bail!("generated world did not open for play")
    };
    let crate::PreparedPlayWorld {
        mut session,
        mut resources,
        mut streamer,
        ..
    } = *prepared;
    let mut reader =
        WorldReader::open_with_session(&path, DimensionId::parse("overworld")?, lock.clone())?;
    let portal = reader
        .read_chunk(20, 19)?
        .context("portal start chunk missing")?;
    let Some(native_persistence::nbt::Tag::Compound(starts)) = portal
        .document
        .value
        .get("structures")
        .and_then(|structures| structures.get("starts"))
    else {
        anyhow::bail!("portal structure starts missing")
    };
    ensure!(
        starts.iter().any(|(name, start)| {
            name.to_string_lossy().starts_with("minecraft:ruined_portal")
                && matches!(start.get("Children"), Some(native_persistence::nbt::Tag::List { values, .. }) if !values.is_empty())
        }),
        "generated portal start is missing or has no pieces"
    );
    let portal_before = native_persistence::nbt::write(&portal.document)?;
    let mut remote_entities_before = std::collections::BTreeMap::new();
    // Entity regions can include neighbors outside the postprocessed set.
    for saved in reader.saved_chunk_positions(RegionStore::Entities)? {
        let pos = ChunkPos {
            x: saved.x,
            z: saved.z,
        };
        if !session.has_resident_or_parked_chunk(pos)
            && let Some(chunk) = reader.read_entity_chunk(pos.x, pos.z)?
            && !chunk.entities().is_empty()
        {
            remote_entities_before.insert(pos, native_persistence::nbt::write(&chunk.document)?);
        }
    }
    ensure!(
        !remote_entities_before.is_empty(),
        "no distant entity chunk was available to verify"
    );
    eprintln!(
        "checking {} distant entity chunks",
        remote_entities_before.len()
    );

    // Normal ticks exercise the same world rules as the eventual GUI.
    let before = session.snapshot();
    session.submit_command(PlayerCommand::Look {
        yaw: 0.25,
        pitch: -0.1,
    });
    session.submit_command(PlayerCommand::Move {
        forward: 1.0,
        strafe: 0.0,
        jump: false,
    });
    session.update(Duration::from_millis(50));
    session.submit_command(PlayerCommand::Move {
        forward: 0.0,
        strafe: 0.0,
        jump: false,
    });
    session.update(Duration::from_millis(50));
    for _ in 0..additional_ticks {
        // Keep disk streaming responsive without advancing simulation faster
        // than the game's 20 TPS while waiting for its next tick.
        let next_tick = Instant::now() + Duration::from_millis(50);
        loop {
            streamer.service(&mut session)?;
            let now = Instant::now();
            if now >= next_tick {
                break;
            }
            std::thread::sleep((next_tick - now).min(Duration::from_millis(5)));
        }
        session.update(Duration::from_millis(50));
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut next_tick = Instant::now() + Duration::from_millis(50);
    while !session.required_shape_update_chunks().is_empty() {
        let required = session.required_shape_update_chunks();
        for pos in required
            .iter()
            .filter(|pos| !session.has_resident_or_parked_chunk(**pos))
        {
            ensure!(
                reader
                    .read_chunk(pos.x, pos.z)?
                    .is_some_and(|chunk| chunk.is_full()),
                "required shape dependency is not generated: {pos:?}"
            );
        }
        ensure!(
            Instant::now() < deadline,
            "shape updates did not settle: {required:?}"
        );
        streamer.service(&mut session)?;
        if Instant::now() >= next_tick {
            session.update(Duration::from_millis(50));
            next_tick += Duration::from_millis(50);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(streamer);
    let expected = session.snapshot();
    ensure!(
        expected.tick >= before.tick + 2 + additional_ticks,
        "not all requested gameplay ticks were processed"
    );
    let before_position = before.origin.as_vec3() + before.player.position;
    let expected_position = expected.origin.as_vec3() + expected.player.position;
    ensure!(
        (expected_position - before_position).length() > 0.0001,
        "the player did not move"
    );
    ensure!(
        expected.player.yaw != before.player.yaw || expected.player.pitch != before.player.pitch,
        "the player's look did not change"
    );
    let mut frozen = session
        .freeze_partial_save()
        .map_err(|error| error.into_parts().1)?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while !frozen.poll_lighting().is_settled() {
        ensure!(
            Instant::now() < deadline,
            "saved-world lighting did not settle"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let sources = reader.capture_session_save_sources(frozen.source_positions().iter().copied())?;
    let block_ids = resources.vanilla_block_ids()?;
    let tags = native_resources::heightmap_tags::HeightmapTags::load(&mut resources, &block_ids)?;
    let registry = resources.vanilla_generated_block_registry()?;
    let resolve = |saved: &native_persistence::palette::BlockState| -> Result<BlockState> {
        let name = String::from_utf16(&saved.name.0)?;
        let mut supplied = std::collections::BTreeMap::new();
        for (key, value) in &saved.properties {
            ensure!(
                supplied
                    .insert(String::from_utf16(&key.0)?, String::from_utf16(&value.0)?)
                    .is_none(),
                "duplicate block property"
            );
        }
        let (name, properties) =
            registry.resolve(&serde_json::json!({"id": name, "properties": supplied}))?;
        ensure!(
            supplied
                .iter()
                .all(|(key, value)| properties.get(key) == Some(value)),
            "unsupported block property"
        );
        Ok(BlockState { name, properties })
    };
    let plan = frozen.session().draft_captured_partial_save(
        &sources,
        &tags,
        |saved| {
            let state = resolve(saved)?;
            Ok((
                matches!(
                    state.name.as_str(),
                    "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                ),
                FluidState::from_block(&state)
                    .map_err(anyhow::Error::msg)?
                    .is_some(),
            ))
        },
        &resolve,
        |state| {
            ensure!(
                registry.is_canonical_state(&state.name, &state.properties),
                "unsupported block state"
            );
            Ok(())
        },
        |_| anyhow::bail!("unexpected POI change"),
        1,
    );
    let plan = match plan {
        Ok(plan) => plan,
        Err(error) => {
            if let Err(diagnostic) = report_pending_postprocessing(&mut reader, frozen.session()) {
                eprintln!("could not inspect saved postprocessing: {diagnostic:#}");
            }
            return Err(error.context("cannot draft the played generated world"));
        }
    };
    let mut edits: std::collections::BTreeMap<_, _> = expected
        .world
        .loaded()
        .context("loaded world missing")?
        .block_edits()
        .map(|edit| (edit.position, edit.clone()))
        .collect();
    let mut written_edits = Vec::new();
    drop(reader);
    let mut writer = lock.acquire_write()?;
    writer.write_plan(plan, |dimension, change| {
        ensure!(
            *dimension == DimensionId::parse("overworld")?,
            "wrong dimension"
        );
        let pos = BlockPosition {
            x: change.x,
            y: change.y,
            z: change.z,
        };
        let edit = edits.remove(&pos).context("unexpected saved block edit")?;
        ensure!(
            edit.original.as_ref() == Some(&resolve(&change.expected)?),
            "original block changed"
        );
        ensure!(
            edit.replacement == resolve(&change.replacement)?,
            "saved block changed"
        );
        written_edits.push((pos, edit.replacement));
        Ok(())
    })?;
    ensure!(edits.is_empty(), "saved plan omitted block edits");
    drop(writer);
    drop(frozen);
    let mut reader = WorldReader::open_with_session(&path, DimensionId::parse("overworld")?, lock)?;
    let portal_after = reader.read_chunk(20, 19)?.context("portal chunk lost")?;
    assert_eq!(
        native_persistence::nbt::write(&portal_after.document)?,
        portal_before
    );
    for (pos, before) in remote_entities_before {
        let after = reader
            .read_entity_chunk(pos.x, pos.z)?
            .context("distant entity chunk was lost")?;
        assert_eq!(
            native_persistence::nbt::write(&after.document)?,
            before,
            "distant entity chunk changed at {pos:?}"
        );
    }
    let mut edited_chunks = std::collections::BTreeMap::new();
    for (position, expected_state) in &written_edits {
        let pos = ChunkPos {
            x: position.x.div_euclid(16),
            z: position.z.div_euclid(16),
        };
        if let std::collections::btree_map::Entry::Vacant(entry) = edited_chunks.entry(pos) {
            entry.insert(
                reader
                    .read_chunk(pos.x, pos.z)?
                    .context("saved edited chunk missing")?,
            );
        }
        let actual = edited_chunks[&pos]
            .block(position.x, position.y, position.z)
            .context("saved edited block missing")?;
        ensure!(
            resolve(actual)? == *expected_state,
            "saved block differs at {position:?}"
        );
    }
    eprintln!("reopened {} edited blocks", written_edits.len());
    let player = reader.read_player()?.context("saved player missing")?;
    let camera = crate::saved_camera(&player)?.context("saved player position missing")?;
    for (actual, expected) in camera
        .position
        .into_iter()
        .zip(expected_position.to_array())
    {
        assert!((actual - f64::from(expected)).abs() < 0.0001);
    }
    assert_eq!(camera.yaw, expected.player.yaw.to_degrees() - 180.0);
    assert_eq!(camera.pitch, -expected.player.pitch.to_degrees());
    assert_eq!(reader.game_time()?, expected.game_time as i64);
    Ok(())
}

/// Report actual saved marks and nearby chunk coverage when an end-to-end
/// save fails. This inspects only the private world created by the test.
fn report_pending_postprocessing(
    reader: &mut WorldReader,
    session: &native_session::loaded::LoadedSession,
) -> Result<()> {
    for position in reader.saved_chunk_positions(RegionStore::Terrain)? {
        let position = ChunkPos {
            x: position.x,
            z: position.z,
        };
        if !session.has_resident_or_parked_chunk(position) {
            continue;
        }
        let Some(chunk) = reader.read_chunk(position.x, position.z)? else {
            continue;
        };
        let Some(tag) = chunk.document.value.get("PostProcessing") else {
            continue;
        };
        if matches!(
            tag,
            native_persistence::nbt::Tag::List {
                element_type: native_persistence::nbt::TagType::End
                    | native_persistence::nbt::TagType::List,
                values,
            } if values.is_empty()
        ) {
            continue;
        }
        let marks = native_persistence::postprocessing::decode(tag, -64, 384)?;
        let count: usize = marks.sections().iter().map(Vec::len).sum();
        if count == 0 {
            continue;
        }
        let mut disk_full = 0;
        let mut session_full = 0;
        for dz in -1..=1 {
            for dx in -1..=1 {
                let neighbor = ChunkPos {
                    x: position.x + dx,
                    z: position.z + dz,
                };
                if reader
                    .read_chunk(neighbor.x, neighbor.z)?
                    .is_some_and(|chunk| chunk.is_full())
                {
                    disk_full += 1;
                }
                session_full += usize::from(session.has_resident_or_parked_chunk(neighbor));
            }
        }
        let marks_ref = &marks;
        let samples: Vec<_> = marks
            .sections()
            .iter()
            .enumerate()
            .flat_map(|(section, values)| {
                values.iter().filter_map(move |&packed| {
                    marks_ref.unpack(section, packed, position.x, position.z)
                })
            })
            .take(8)
            .map(|[x, y, z]| {
                let state = chunk
                    .block(x, y, z)
                    .map(|state| state.name.to_string_lossy())
                    .unwrap_or_else(|| "<missing>".into());
                ((x, y, z), state)
            })
            .collect();
        eprintln!(
            "pending postprocessing {position:?}: {count} marks, 3x3 disk FULL={disk_full}, session loaded={session_full}, samples={samples:?}"
        );
    }
    Ok(())
}
