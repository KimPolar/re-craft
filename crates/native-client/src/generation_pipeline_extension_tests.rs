//! Exercise retry across the real initial-generation/extension boundary.
use super::*;
use anyhow::{Context, Result, ensure};
use std::time::{Duration, Instant};

#[test]
#[ignore = "requires Minecraft 26.3 and long-running structured world generation"]
fn cancelled_initial_world_retries_its_pending_extension() -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let test_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/region-test-temp");
    std::fs::create_dir_all(&test_root)?;
    let parent = tempfile::Builder::new()
        .prefix("extension-retry-")
        .tempdir_in(test_root)?;
    let mut settings = crate::world_creation::WorldCreationSettings::new("Test".into());
    settings.seed = crate::chat_input::ChatInput::field("-1");
    settings.generate_structures = true;
    let target = ChunkPos { x: 30, z: 24 };
    let options = InitialWorldOptions {
        staging_parent: parent.path().to_path_buf(),
        border: SavedWorldBorder::default(),
        player_random_seed: 1234,
        ceiling_spawn_height: None,
        mesh_revision: 1,
        biome_blend: 0,
        last_played_ms: 123,
        region_timestamp: 1,
    };
    let mut pipeline = InitialWorldPipeline::start_with_additional_targets(
        settings.freeze(game, None),
        options,
        BTreeSet::from([target]),
    )?;

    // Wait without polling the result so cancellation lands after generation
    // completed but before the pipeline can start its additional targets.
    let deadline = Instant::now() + Duration::from_secs(900);
    let mut report = Instant::now() + Duration::from_secs(30);
    loop {
        let progress = pipeline.progress();
        match progress {
            InitialWorldProgress::Generating(progress)
                if progress.phase == crate::generation_worker::GenerationPhase::Ready =>
            {
                break;
            }
            InitialWorldProgress::Generating(progress)
                if matches!(
                    progress.phase,
                    crate::generation_worker::GenerationPhase::Failed
                        | crate::generation_worker::GenerationPhase::Cancelled
                ) =>
            {
                anyhow::bail!("initial generation did not complete: {progress:?}")
            }
            InitialWorldProgress::Generating(_) => {}
            _ => anyhow::bail!("pipeline left generation before retry test"),
        }
        let now = Instant::now();
        ensure!(now < deadline, "initial generation timed out");
        if now >= report {
            eprintln!("extension retry: {progress:?}");
            report = now + Duration::from_secs(30);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    pipeline.cancel();
    let generation = loop {
        match pipeline.poll() {
            Some(InitialWorldResult::Cancelled {
                generation: Some(generation),
            }) => break generation,
            Some(InitialWorldResult::Failed { error, .. }) => return Err(error),
            Some(_) => anyhow::bail!("cancellation lost the completed generation"),
            None => {}
        }
        ensure!(Instant::now() < deadline, "cancellation timed out");
        std::thread::sleep(Duration::from_millis(20));
    };
    let spawn = generation.player_spawn.context("initial spawn missing")?;
    ensure!(
        ChunkPos {
            x: spawn.x.div_euclid(16),
            z: spawn.z.div_euclid(16),
        } == target,
        "unexpected spawn for retry fixture"
    );

    // A rejected extension must return the unchanged generation and keep the
    // extension pending, so that the caller can correct inputs and retry.
    let pending = pipeline.extension.as_mut().context("extension was lost")?;
    let original_targets = std::mem::take(&mut pending.0);
    let failure = match pipeline.retry_staging(generation, parent.path().to_path_buf()) {
        Ok(()) => anyhow::bail!("empty extension unexpectedly started"),
        Err(failure) => failure,
    };
    assert!(format!("{:#}", failure.error).contains("no additional initial generation targets"));
    assert!(matches!(
        pipeline.progress(),
        InitialWorldProgress::Finished
    ));
    pipeline.extension.as_mut().context("extension was lost")?.0 = original_targets;
    pipeline
        .retry_staging(failure.generation, parent.path().to_path_buf())
        .map_err(|failure| failure.error)?;
    assert!(matches!(
        pipeline.progress(),
        InitialWorldProgress::Generating(_)
    ));

    let deadline = Instant::now() + Duration::from_secs(900);
    let mut report = Instant::now() + Duration::from_secs(30);
    loop {
        match pipeline.poll() {
            Some(InitialWorldResult::Ready {
                generation,
                world,
                player_area,
            }) => {
                let area = player_area?;
                assert!(area.missing.is_empty() && area.deferred.is_empty());
                assert!(
                    generation
                        .postprocessed
                        .as_ref()
                        .context("postprocessing missing")?
                        .region
                        .chunks
                        .contains(&target)
                );
                let path = match &world {
                    StagedWorld::Structured(stage) => stage.path(),
                    StagedWorld::Regular(_) => anyhow::bail!("wrong staged world type"),
                };
                assert!(path.join("level.dat").is_file());
                drop(world);
                break;
            }
            Some(InitialWorldResult::Failed { error, .. }) => return Err(error),
            Some(_) => anyhow::bail!("extension retry was cancelled"),
            None => {}
        }
        let now = Instant::now();
        ensure!(now < deadline, "extension retry timed out");
        if now >= report {
            eprintln!("extension retry: {:?}", pipeline.progress());
            report = now + Duration::from_secs(30);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(std::fs::read_dir(parent.path())?.next().is_none());
    Ok(())
}
