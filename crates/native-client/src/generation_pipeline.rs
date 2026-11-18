//! Nonblocking connection between initial generation and private world staging.
use std::{collections::BTreeSet, path::PathBuf};

use anyhow::{Result, ensure};
use native_persistence::world::SavedWorldBorder;
use native_world::loaded::ChunkPos;

use crate::{
    generation_staging::{
        InitialWorldStagingWorker, StagedWorld, StagingProgress, StagingResult, StagingStartError,
    },
    generation_worker::initial_extension::InitialExtensionResources,
    generation_worker::{GenerationProgress, GenerationReady, GenerationResult, GenerationWorker},
    world_creation::FrozenWorldCreation,
};

#[cfg(all(test, target_os = "windows"))]
#[path = "generation_pipeline_extension_tests.rs"]
mod extension_recovery_tests;

pub struct InitialWorldOptions {
    /// Existing directory in which the private staging tree is created.
    pub staging_parent: PathBuf,
    pub border: SavedWorldBorder,
    pub player_random_seed: i64,
    pub ceiling_spawn_height: Option<i32>,
    pub mesh_revision: u64,
    pub biome_blend: u8,
    pub last_played_ms: i64,
    pub region_timestamp: u32,
}

#[derive(Clone, Debug)]
pub enum InitialWorldProgress {
    Generating(GenerationProgress),
    Staging(StagingProgress),
    Finished,
}

pub enum InitialWorldResult {
    /// An isolated inspection stage, not a published or playable world.
    Ready {
        generation: GenerationReady,
        world: StagedWorld,
        /// Coverage of the player's initial 3×3 load area. This report does
        /// not establish full world completeness or authorize publication.
        player_area: Result<InitialPlayerArea>,
    },
    Cancelled {
        /// Available when generation completed before cancellation.
        generation: Option<GenerationReady>,
    },
    Failed {
        error: anyhow::Error,
        /// Staging or extension start failures retain their input for retry.
        generation: Option<GenerationReady>,
    },
}

/// Chunks needed for the initial player load that are absent or still have
/// deferred postprocessing. Coordinates are in the generated dimension.
#[derive(Debug, Eq, PartialEq)]
pub struct InitialPlayerArea {
    pub center: native_world::loaded::ChunkPos,
    pub missing: Vec<native_world::loaded::ChunkPos>,
    pub deferred: Vec<native_world::loaded::ChunkPos>,
}

fn initial_player_area(generation: &GenerationReady) -> Result<InitialPlayerArea> {
    use anyhow::Context;
    let spawn = generation
        .player_spawn
        .context("initial player spawn missing")?;
    let processed = generation
        .postprocessed
        .as_ref()
        .context("initial postprocessing missing")?;
    classify_initial_player_area(
        spawn,
        processed
            .region
            .ticks()
            .registration_order()
            .iter()
            .copied(),
        &processed.region.chunks,
        processed.region.deferred_chunks(),
    )
}

pub(crate) fn classify_initial_player_area(
    spawn: glam::IVec3,
    registered_positions: impl IntoIterator<Item = native_world::loaded::ChunkPos>,
    completed: &std::collections::BTreeSet<native_world::loaded::ChunkPos>,
    deferred_positions: impl IntoIterator<Item = native_world::loaded::ChunkPos>,
) -> Result<InitialPlayerArea> {
    use anyhow::{Context, ensure};
    use native_world::loaded::ChunkPos;
    let center = ChunkPos {
        x: spawn.x.div_euclid(16),
        z: spawn.z.div_euclid(16),
    };
    let min_x = center.x.checked_sub(1).context("initial area X overflow")?;
    let max_x = center.x.checked_add(1).context("initial area X overflow")?;
    let min_z = center.z.checked_sub(1).context("initial area Z overflow")?;
    let max_z = center.z.checked_add(1).context("initial area Z overflow")?;
    let mut registered = std::collections::BTreeSet::new();
    for position in registered_positions {
        if (min_x..=max_x).contains(&position.x) && (min_z..=max_z).contains(&position.z) {
            ensure!(
                registered.insert(position),
                "duplicate registered initial chunk"
            );
        }
    }
    let deferred_chunks: std::collections::BTreeSet<_> = deferred_positions
        .into_iter()
        .filter(|position| {
            (min_x..=max_x).contains(&position.x) && (min_z..=max_z).contains(&position.z)
        })
        .collect();
    let mut missing = Vec::new();
    let mut deferred = Vec::new();
    for z in min_z..=max_z {
        for x in min_x..=max_x {
            let position = ChunkPos { x, z };
            if !registered.contains(&position) {
                ensure!(
                    !completed.contains(&position) && !deferred_chunks.contains(&position),
                    "unregistered initial chunk has postprocessing state"
                );
                missing.push(position);
            } else if !completed.contains(&position) {
                ensure!(
                    deferred_chunks.contains(&position),
                    "registered initial chunk has no postprocessing state"
                );
                deferred.push(position);
            } else {
                ensure!(
                    !deferred_chunks.contains(&position),
                    "completed initial chunk is also deferred"
                );
            }
        }
    }
    Ok(InitialPlayerArea {
        center,
        missing,
        deferred,
    })
}

enum State {
    Generating(GenerationWorker),
    Staging(InitialWorldStagingWorker),
    Finished,
}

pub struct InitialWorldPipeline {
    state: State,
    options: InitialWorldOptions,
    structures: bool,
    extension: Option<(BTreeSet<ChunkPos>, InitialExtensionResources)>,
    cancelled: bool,
}

impl InitialWorldPipeline {
    /// Freeze settings before this call. The returned stage is never published.
    pub fn start(frozen: FrozenWorldCreation, options: InitialWorldOptions) -> Result<Self> {
        Self::start_with_additional_targets(frozen, options, BTreeSet::new())
    }

    /// Complete explicit additional chunks while the original generation
    /// context is still available. The resulting stage is still private.
    pub(crate) fn start_with_additional_targets(
        frozen: FrozenWorldCreation,
        options: InitialWorldOptions,
        targets: BTreeSet<ChunkPos>,
    ) -> Result<Self> {
        let structures = frozen.generation.structures_enabled;
        let extension = if targets.is_empty() {
            None
        } else {
            ensure!(
                structures,
                "additional initial chunks require structure-aware generation"
            );
            Some((
                targets,
                InitialExtensionResources {
                    minecraft_dir: frozen.generation.minecraft_dir.clone(),
                    preset: frozen.generation.preset.clone(),
                    dimension: frozen.generation.dimension.clone(),
                    selection: frozen
                        .generation
                        .data_packs
                        .as_ref()
                        .map(|(_, selection)| selection.clone()),
                },
            ))
        };
        let worker = GenerationWorker::start_frozen_initial_world(
            frozen,
            options.border,
            options.player_random_seed,
            options.ceiling_spawn_height,
            options.mesh_revision,
            options.biome_blend,
        )?;
        Ok(Self {
            state: State::Generating(worker),
            options,
            structures,
            extension,
            cancelled: false,
        })
    }

    /// Request cancellation without waiting for a worker or a file operation.
    pub fn cancel(&mut self) {
        self.cancelled = true;
        match &self.state {
            State::Generating(worker) => worker.cancel(),
            State::Staging(worker) => worker.cancel(),
            State::Finished => {}
        }
    }

    pub fn progress(&self) -> InitialWorldProgress {
        match &self.state {
            State::Generating(worker) => InitialWorldProgress::Generating(worker.progress()),
            State::Staging(worker) => InitialWorldProgress::Staging(worker.progress()),
            State::Finished => InitialWorldProgress::Finished,
        }
    }

    /// Retry retained generation with its original remaining work and settings.
    /// A new parent can be supplied after a directory or permissions error.
    pub fn retry_staging(
        &mut self,
        generation: GenerationReady,
        parent: PathBuf,
    ) -> std::result::Result<(), StagingStartError> {
        if !matches!(self.state, State::Finished) {
            return Err(StagingStartError {
                generation,
                error: anyhow::anyhow!("initial world pipeline is still running"),
            });
        }
        self.start_remaining_work(generation, parent)
    }

    fn start_remaining_work(
        &mut self,
        generation: GenerationReady,
        parent: PathBuf,
    ) -> std::result::Result<(), StagingStartError> {
        if let Some((targets, resources)) = &self.extension {
            let worker = GenerationWorker::extend_postprocessed_initial(
                generation,
                targets.clone(),
                self.options.player_random_seed,
                resources.clone(),
            )
            .map_err(|failure| StagingStartError {
                generation: failure.generation,
                error: failure
                    .error
                    .context("cannot start initial world extension"),
            })?;
            self.extension = None;
            self.options.staging_parent = parent;
            self.cancelled = false;
            self.state = State::Generating(worker);
        } else {
            let worker = InitialWorldStagingWorker::start_recoverable(
                generation,
                parent.clone(),
                self.structures,
                self.options.last_played_ms,
                self.options.region_timestamp,
            )
            .map_err(|failure| StagingStartError {
                generation: failure.generation,
                error: failure.error.context("cannot start initial world staging"),
            })?;
            self.options.staging_parent = parent;
            self.cancelled = false;
            self.state = State::Staging(worker);
        }
        Ok(())
    }

    /// Poll once without waiting. A terminal result is returned at most once.
    pub fn poll(&mut self) -> Option<InitialWorldResult> {
        match &self.state {
            State::Generating(worker) => match worker.try_result() {
                Ok(None) => None,
                Ok(Some(GenerationResult::Ready(generation))) if !self.cancelled => {
                    match self.start_remaining_work(generation, self.options.staging_parent.clone())
                    {
                        Ok(()) => None,
                        Err(failure) => {
                            self.state = State::Finished;
                            Some(InitialWorldResult::Failed {
                                error: failure.error,
                                generation: Some(failure.generation),
                            })
                        }
                    }
                }
                Ok(Some(GenerationResult::Ready(generation))) => {
                    self.state = State::Finished;
                    Some(InitialWorldResult::Cancelled {
                        generation: Some(generation),
                    })
                }
                Ok(Some(GenerationResult::Cancelled)) => {
                    self.state = State::Finished;
                    Some(InitialWorldResult::Cancelled { generation: None })
                }
                Ok(Some(GenerationResult::Failed(error))) | Err(error) => {
                    self.state = State::Finished;
                    Some(InitialWorldResult::Failed {
                        error: error.context("initial world generation failed"),
                        generation: None,
                    })
                }
            },
            State::Staging(worker) => match worker.try_result() {
                Ok(None) => None,
                Ok(Some(StagingResult::Ready { generation, world })) if !self.cancelled => {
                    self.state = State::Finished;
                    let player_area = initial_player_area(&generation);
                    Some(InitialWorldResult::Ready {
                        generation,
                        world,
                        player_area,
                    })
                }
                Ok(Some(
                    StagingResult::Ready { generation, .. } | StagingResult::Cancelled(generation),
                )) => {
                    self.state = State::Finished;
                    Some(InitialWorldResult::Cancelled {
                        generation: Some(generation),
                    })
                }
                Ok(Some(StagingResult::Failed { generation, error })) => {
                    self.state = State::Finished;
                    Some(InitialWorldResult::Failed {
                        error: error.context("initial world staging failed"),
                        generation: Some(generation),
                    })
                }
                Err(error) => {
                    self.state = State::Finished;
                    Some(InitialWorldResult::Failed {
                        error: error.context("initial world staging worker failed"),
                        generation: None,
                    })
                }
            },
            State::Finished => None,
        }
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use anyhow::{Context, ensure};
    use std::time::{Duration, Instant};

    fn options(parent: PathBuf) -> InitialWorldOptions {
        InitialWorldOptions {
            staging_parent: parent,
            border: SavedWorldBorder::default(),
            player_random_seed: 1234,
            ceiling_spawn_height: None,
            mesh_revision: 1,
            biome_blend: 0,
            last_played_ms: 123,
            region_timestamp: 1,
        }
    }

    fn wait(pipeline: &mut InitialWorldPipeline) -> InitialWorldResult {
        let started = Instant::now();
        let deadline = started + Duration::from_secs(900);
        let mut next_report = started + Duration::from_secs(30);
        loop {
            if let Some(result) = pipeline.poll() {
                return result;
            }
            let now = Instant::now();
            if now >= next_report {
                eprintln!(
                    "initial world after {:?}: {:?}",
                    now - started,
                    pipeline.progress()
                );
                next_report = now + Duration::from_secs(30);
            }
            assert!(now < deadline, "initial world pipeline timed out");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn copy_staged_tree(source: &std::path::Path, destination: &std::path::Path) -> Result<()> {
        std::fs::create_dir(destination)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            let metadata = entry.file_type()?;
            let target = destination.join(entry.file_name());
            if metadata.is_dir() {
                copy_staged_tree(&entry.path(), &target)?;
            } else if metadata.is_file() {
                std::fs::copy(entry.path(), target)?;
            } else {
                anyhow::bail!("unexpected staged world entry");
            }
        }
        Ok(())
    }

    /// Compare a staged overworld chunk's unzoomed quart biomes with the
    /// installed Java generator, independently of the Rust generation draft.
    fn compare_staged_biomes_with_java(
        game: &std::path::Path,
        parent: &std::path::Path,
        seed: i64,
        target: ChunkPos,
        chunk: &native_persistence::world::LoadedChunk,
    ) -> Result<()> {
        use std::process::Command;

        let oracle = tempfile::tempdir_in(parent)?;
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
        let compared = Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(classpath)
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
        let mut bounds = header.split_whitespace();
        let min_y: i32 = bounds
            .next()
            .context("minimum biome height missing")?
            .parse()?;
        let height: i32 = bounds.next().context("biome height missing")?.parse()?;
        ensure!(
            height > 0 && height % 4 == 0 && bounds.next().is_none(),
            "invalid Java biome height"
        );
        let origin_x = target.x.checked_mul(16).context("biome X overflow")?;
        let origin_z = target.z.checked_mul(16).context("biome Z overflow")?;
        let mut columns = BTreeSet::new();
        for row in output.lines().filter_map(|line| line.strip_prefix("B ")) {
            let mut fields = row.split_whitespace();
            let x: i32 = fields.next().context("biome X is missing")?.parse()?;
            let z: i32 = fields.next().context("biome Z is missing")?.parse()?;
            ensure!(
                (0..4).contains(&x) && (0..4).contains(&z) && columns.insert((x, z)),
                "invalid biome column"
            );
            for y in 0..height / 4 {
                let expected = fields.next().context("Java biome value is missing")?;
                let actual = chunk
                    .biome(origin_x + x * 4, min_y + y * 4, origin_z + z * 4)
                    .context("staged biome value is missing")?;
                ensure!(
                    actual.matches(expected),
                    "staged biome differs at local quart ({x}, {y}, {z})"
                );
            }
            ensure!(fields.next().is_none(), "extra Java biome values");
        }
        ensure!(columns.len() == 16, "Java biome columns are incomplete");
        eprintln!(
            "Compared {} staged biome samples with Java for {target:?}",
            16 * height / 4
        );
        Ok(())
    }

    #[test]
    fn rejects_inconsistent_settings_before_starting_generation() -> Result<()> {
        let parent = tempfile::tempdir()?;
        let settings = crate::world_creation::WorldCreationSettings::new("Test".into());
        let mut frozen = settings.freeze(parent.path().to_path_buf(), None);
        frozen.level.session.seed = frozen.generation.seed.wrapping_add(1);
        let error = InitialWorldPipeline::start(frozen, options(parent.path().to_path_buf()))
            .err()
            .context("inconsistent settings were accepted")?;
        assert!(format!("{error:#}").contains("session seed differs"));
        assert_eq!(std::fs::read_dir(parent.path())?.count(), 0);
        Ok(())
    }

    #[test]
    fn rejects_unstructured_extension_before_starting_generation() -> Result<()> {
        let parent = tempfile::tempdir()?;
        let mut settings = crate::world_creation::WorldCreationSettings::new("Test".into());
        settings.generate_structures = false;
        let frozen = settings.freeze(parent.path().to_path_buf(), None);
        let error = InitialWorldPipeline::start_with_additional_targets(
            frozen,
            options(parent.path().to_path_buf()),
            BTreeSet::from([ChunkPos { x: 28, z: 24 }]),
        )
        .err()
        .context("unstructured extension was accepted")?;
        assert!(format!("{error:#}").contains("structure-aware generation"));
        assert_eq!(std::fs::read_dir(parent.path())?.count(), 0);
        Ok(())
    }

    #[test]
    fn player_area_reports_missing_and_deferred_chunks() -> Result<()> {
        use native_world::loaded::ChunkPos;
        use std::collections::BTreeSet;
        let center = ChunkPos { x: -1, z: -2 };
        let neighbor = ChunkPos { x: -2, z: -2 };
        let complete = BTreeSet::from([center]);
        let area = classify_initial_player_area(
            glam::IVec3::new(-1, 64, -17),
            [center, neighbor],
            &complete,
            [neighbor],
        )?;
        assert_eq!(area.center, center);
        assert_eq!(area.deferred, vec![neighbor]);
        assert_eq!(area.missing.len(), 7);
        assert!(!area.missing.contains(&center));
        assert!(!area.missing.contains(&neighbor));
        assert!(
            classify_initial_player_area(
                glam::IVec3::ZERO,
                [ChunkPos { x: 0, z: 0 }; 2],
                &BTreeSet::new(),
                [],
            )
            .is_err()
        );
        assert!(
            classify_initial_player_area(
                glam::IVec3::ZERO,
                [ChunkPos { x: 0, z: 0 }],
                &BTreeSet::new(),
                [],
            )
            .is_err()
        );
        assert!(
            classify_initial_player_area(
                glam::IVec3::ZERO,
                [],
                &BTreeSet::from([ChunkPos { x: 0, z: 0 }]),
                [],
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and trial chamber block placement"]
    fn seed_one_trial_chamber_chunk_reaches_light_stage() -> Result<()> {
        use glam::IVec3;
        use native_world::loaded::ChunkPos;
        use std::collections::BTreeSet;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut settings = crate::world_creation::WorldCreationSettings::new("Test".into());
        settings.seed = crate::chat_input::ChatInput::field("1");
        settings.generate_structures = true;
        let frozen = settings.freeze(game, None);
        let target = ChunkPos { x: 18, z: 14 };
        let worker = GenerationWorker::start_with_structures(
            move || crate::fresh_generation::prepare_with_structures(frozen.generation),
            BTreeSet::from([target]),
            1,
            0,
            IVec3::new(target.x * 16, 0, target.z * 16),
        )?;
        let started = Instant::now();
        let mut next_report = started + Duration::from_secs(30);
        loop {
            match worker.try_result()? {
                Some(GenerationResult::Ready(ready)) => {
                    ensure!(
                        ready
                            .mesh
                            .targets
                            .as_ref()
                            .is_some_and(|targets| targets.contains(&target)),
                        "trial chunk missing"
                    );
                    break;
                }
                Some(GenerationResult::Failed(error)) => {
                    return Err(error.context(format!("trial generation: {:?}", worker.progress())));
                }
                Some(GenerationResult::Cancelled) => anyhow::bail!("trial generation cancelled"),
                None => {}
            }
            let now = Instant::now();
            if now >= next_report {
                eprintln!(
                    "trial generation after {:?}: {:?}",
                    now - started,
                    worker.progress()
                );
                next_report = now + Duration::from_secs(30);
            }
            ensure!(
                now.duration_since(started) < Duration::from_secs(900),
                "trial generation timed out"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires Minecraft 26.3; structured world generation is incomplete"]
    fn structured_seed_one_reaches_initial_world_stage() -> Result<()> {
        structured_seed_reaches_initial_world_stage(1, false, BTreeSet::new(), false)
    }

    #[test]
    #[ignore = "requires Minecraft 26.3; structured world generation is incomplete"]
    fn structured_seed_negative_one_reaches_initial_world_stage() -> Result<()> {
        structured_seed_reaches_initial_world_stage(-1, false, BTreeSet::new(), false)
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and complete generated-world play-save dependencies"]
    fn generated_world_ticked_player_save_checks_dependencies() -> Result<()> {
        structured_seed_reaches_initial_world_stage(-1, true, BTreeSet::new(), false)
    }

    #[test]
    #[ignore = "requires Minecraft 26.3, Java 25 and additional structured chunk generation"]
    fn generated_world_extends_requested_neighbor_before_staging() -> Result<()> {
        structured_seed_reaches_initial_world_stage(
            -1,
            false,
            BTreeSet::from([ChunkPos { x: 28, z: 24 }]),
            false,
        )
    }

    #[test]
    #[ignore = "requires Minecraft 26.3, Java 25 and complete bubble-column save tracking"]
    fn generated_world_with_extended_neighbor_saves_after_play() -> Result<()> {
        structured_seed_reaches_initial_world_stage(
            -1,
            true,
            BTreeSet::from([ChunkPos { x: 28, z: 24 }]),
            true,
        )
    }

    fn structured_seed_reaches_initial_world_stage(
        seed: i64,
        play_save: bool,
        required_targets: BTreeSet<ChunkPos>,
        require_save: bool,
    ) -> Result<()> {
        let game = match std::env::var_os("MINECRAFT_DIR") {
            Some(path) => PathBuf::from(path),
            None => native_resources::default_minecraft_dir()?,
        };
        // Some Windows sandboxes disallow the no-replace rename used by world
        // publication in the system temporary directory. Use the same
        // workspace-local test root as the persistence publication tests.
        let test_root =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/region-test-temp");
        std::fs::create_dir_all(&test_root)?;
        let parent = tempfile::Builder::new()
            .prefix("initial-world-")
            .tempdir_in(test_root)?;
        let mut settings = crate::world_creation::WorldCreationSettings::new("Test".into());
        settings.seed = crate::chat_input::ChatInput::field(&seed.to_string());
        settings.generate_structures = true;
        let mut pipeline = InitialWorldPipeline::start_with_additional_targets(
            settings.freeze(game.clone(), None),
            options(parent.path().to_path_buf()),
            required_targets.clone(),
        )?;
        match wait(&mut pipeline) {
            InitialWorldResult::Ready {
                generation,
                world,
                player_area,
            } => {
                let player_area = player_area?;
                assert!(
                    player_area.missing.is_empty(),
                    "initial player area has missing chunks: {:?}",
                    player_area.missing
                );
                assert!(
                    player_area.deferred.is_empty(),
                    "initial player area has deferred chunks: {:?}",
                    player_area.deferred
                );
                let StagedWorld::Structured(staged_world) = &world else {
                    anyhow::bail!("structured generation returned an unstructured stage");
                };
                let (path, uuid) = (staged_world.path(), staged_world.initial.uuid);
                let mut reader = native_persistence::world::WorldReader::open(
                    path,
                    native_persistence::world::DimensionId::parse("overworld")?,
                )?;
                assert_eq!(reader.saved_world_seed()?, Some(seed));
                assert_eq!(reader.singleplayer_uuid_words()?, Some(uuid));
                assert_eq!(
                    reader
                        .read_player()?
                        .context("staged player missing")?
                        .value
                        .get("UUID"),
                    Some(&native_persistence::nbt::Tag::IntArray(uuid.to_vec()))
                );
                let processed = generation
                    .postprocessed
                    .as_ref()
                    .context("postprocessed generation missing")?;
                assert!(!processed.chunks.is_empty());
                for target in &required_targets {
                    assert!(
                        processed.region.chunks.contains(target),
                        "requested chunk {target:?} was not postprocessed"
                    );
                    let chunk = reader
                        .read_chunk(target.x, target.z)?
                        .context("requested chunk was not saved")?;
                    assert!(chunk.is_full(), "requested chunk {target:?} is not full");
                    compare_staged_biomes_with_java(&game, parent.path(), seed, *target, &chunk)?;
                }
                let mut structure_starts = 0_usize;
                let mut structure_references = 0_usize;
                for (position, expected) in &processed.chunks {
                    let saved = reader
                        .read_chunk(position.x, position.z)?
                        .context("staged terrain chunk missing")?;
                    assert!(saved.is_full());
                    for field in [
                        "sections",
                        "Heightmaps",
                        "block_entities",
                        "block_ticks",
                        "fluid_ticks",
                        "PostProcessing",
                        "structures",
                    ] {
                        assert_eq!(
                            saved.document.value.get(field),
                            expected.chunk.value.get(field),
                            "chunk {position:?}: {field}"
                        );
                    }
                    let structures = saved
                        .document
                        .value
                        .get("structures")
                        .context("staged structure data missing")?;
                    let Some(native_persistence::nbt::Tag::Compound(starts)) =
                        structures.get("starts")
                    else {
                        anyhow::bail!("staged structure starts are invalid at {position:?}");
                    };
                    let Some(native_persistence::nbt::Tag::Compound(references)) =
                        structures.get("References")
                    else {
                        anyhow::bail!("staged structure references are invalid at {position:?}");
                    };
                    structure_starts += starts.len();
                    structure_references += references.len();
                    let records = expected
                        .entity_records
                        .as_ref()
                        .context("entity coverage missing")?;
                    let saved_entities = reader.read_entity_chunk(position.x, position.z)?;
                    if records.is_empty() {
                        assert!(
                            saved_entities.is_none(),
                            "empty entity chunk unexpectedly stored at {position:?}"
                        );
                    } else {
                        let expected_entities = expected
                            .entity_chunk
                            .as_ref()
                            .context("entity draft missing")?;
                        assert_eq!(
                            saved_entities
                                .context("staged entities missing")?
                                .document
                                .as_ref(),
                            expected_entities
                        );
                    }
                }
                eprintln!(
                    "seed {seed}: checked {} staged chunks, {structure_starts} structure starts and {structure_references} reference entries",
                    processed.chunks.len()
                );
                drop(reader);
                // The normal session loader creates a lock file; inspect a copy
                // so that this test does not change the sealed private stage.
                let reopened_path = parent.path().join("reopened-world");
                copy_staged_tree(path, &reopened_path)?;
                let prepared = crate::prepare_existing_world(
                    &reopened_path,
                    &game,
                    &[],
                    crate::ViewOptions {
                        dimension: None,
                        chunk: None,
                        radius: 1,
                        smoke_test: false,
                        block_report: None,
                        play: true,
                        video: crate::menu_settings::VideoSettings::default(),
                    },
                    None,
                )?;
                let crate::PreparedWorld::Play(reopened) = prepared else {
                    anyhow::bail!("staged world did not reopen as a play session");
                };
                use native_session::GameSession;
                let before = staged_world.initial.session.snapshot();
                let after = reopened.session.snapshot();
                let before_position = before.origin.as_vec3() + before.player.position;
                let after_position = after.origin.as_vec3() + after.player.position;
                assert!((before_position - after_position).length() < 0.0001);
                assert_eq!(before.game_time, after.game_time);
                assert_eq!(before.game_mode, after.game_mode);
                let reopened_world = after
                    .world
                    .loaded()
                    .context("reopened terrain is missing")?;
                assert!(!reopened_world.chunks.is_empty());
                for (_, state) in reopened_world.states() {
                    let positioned = || {
                        native_world::collision::positioned_speleothem_shape(
                            state,
                            glam::IVec3::ZERO,
                        )
                        .is_some()
                    };
                    let collision =
                        native_world::collision::block_shape(state).is_none() && !positioned();
                    let outline = native_world::collision::block_outline(state).is_none()
                        && !positioned()
                        && native_world::collision::positioned_flower_outline(
                            state,
                            glam::IVec3::ZERO,
                        )
                        .is_none();
                    if collision || outline {
                        eprintln!(
                            "unsupported shape: collision={collision} outline={outline} {state:?}"
                        );
                    }
                }
                assert_eq!(reopened.session.unsupported_states(), 0);
                assert_eq!(reopened.session.unsupported_outlines(), 0);
                assert_eq!(
                    player_area
                        .missing
                        .iter()
                        .copied()
                        .collect::<std::collections::BTreeSet<_>>(),
                    reopened_world.missing_chunks,
                );
                drop(reopened);

                // Publish only into the temporary test directory. Keep the
                // reservation's lock throughout the new-world handoff.
                let (initial, stage) = world.into_parts();
                assert_eq!(initial.uuid, uuid);
                let published = stage.publish(parent.path(), "Test", "New World")?;
                let published_path = published.path.clone();
                let prepared = crate::prepare_world_with_session(
                    &published_path,
                    &game,
                    &[],
                    crate::ViewOptions {
                        dimension: None,
                        chunk: None,
                        radius: 1,
                        smoke_test: false,
                        block_report: None,
                        play: true,
                        video: crate::menu_settings::VideoSettings::default(),
                    },
                    None,
                    Some(published.session().clone()),
                )?;
                let crate::PreparedWorld::Play(reopened) = prepared else {
                    anyhow::bail!("published world did not reopen as a play session");
                };
                let reopened_snapshot = reopened.session.snapshot();
                let reopened_position =
                    reopened_snapshot.origin.as_vec3() + reopened_snapshot.player.position;
                assert!((before_position - reopened_position).length() < 0.0001);
                let session_lock = published.session().clone();
                drop(published);
                assert!(
                    native_persistence::save::WorldWriteGuard::acquire(&published_path).is_err()
                );
                if play_save {
                    roundtrip_ticked_player(
                        &published_path,
                        &game,
                        session_lock,
                        *reopened,
                        &required_targets,
                        require_save,
                    )?;
                } else {
                    drop(reopened);
                }
            }
            InitialWorldResult::Failed { error, .. } => return Err(error),
            InitialWorldResult::Cancelled { .. } => {
                anyhow::bail!("initial generation was cancelled")
            }
        }
        Ok(())
    }

    /// Exercise a real generated world's prepared session and partial save
    /// transaction in a private test directory. This does not authorize saving
    /// arbitrary gameplay: only the fields checked below are covered.
    fn roundtrip_ticked_player(
        world_path: &std::path::Path,
        game: &std::path::Path,
        lock: native_persistence::save::WorldSessionGuard,
        prepared: crate::PreparedPlayWorld,
        required_targets: &BTreeSet<ChunkPos>,
        require_save: bool,
    ) -> Result<()> {
        use native_persistence::world::{DimensionId, WorldReader};
        use native_session::{GameSession, PlayerCommand};
        use native_world::{fluid::FluidState, loaded::BlockState};

        let crate::PreparedPlayWorld {
            mut session,
            mut resources,
            mut streamer,
            ..
        } = prepared;
        let mut reader = WorldReader::open_with_session(
            world_path,
            DimensionId::parse("overworld")?,
            lock.clone(),
        )?;
        let initial_level = std::fs::read(world_path.join("level.dat"))?;
        let initial_player = native_persistence::nbt::write(
            &reader.read_player()?.context("initial player missing")?,
        )?;
        session.set_random_tick_speed(0);
        session.submit_command(PlayerCommand::Look {
            yaw: 17.0,
            pitch: -6.0,
        });
        session.update(Duration::from_millis(50));
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut next_tick = Instant::now() + Duration::from_millis(50);
        let unavailable = loop {
            let required = session.required_shape_update_chunks();
            if required.is_empty() {
                break Vec::new();
            }
            let waiting: Vec<_> = required
                .iter()
                .copied()
                .filter(|pos| !session.has_resident_or_parked_chunk(*pos))
                .collect();
            let mut absent = Vec::new();
            for position in &waiting {
                if !reader
                    .read_chunk(position.x, position.z)?
                    .is_some_and(|chunk| chunk.is_full())
                {
                    absent.push(*position);
                }
            }
            if !waiting.is_empty() && absent.len() == waiting.len() {
                break absent;
            }
            ensure!(
                Instant::now() < deadline,
                "saved shape-update neighbors did not settle: {required:?}"
            );
            streamer.service(&mut session)?;
            if Instant::now() >= next_tick {
                session.update(Duration::from_millis(50));
                next_tick += Duration::from_millis(50);
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        drop(streamer);
        let expected = session.snapshot();
        let mut frozen = session
            .freeze_partial_save()
            .map_err(|error| error.into_parts().1)
            .context("cannot freeze the ticked generated session")?;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let lighting = frozen.poll_lighting();
            if lighting.is_settled() {
                break;
            }
            ensure!(
                Instant::now() < deadline,
                "frozen light did not settle: {lighting:?}, missing sky chunks: {:?}",
                frozen.session().missing_sky_chunks()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let sources =
            reader.capture_session_save_sources(frozen.source_positions().iter().copied())?;
        let ids = resources.vanilla_block_ids()?;
        let tags = native_resources::heightmap_tags::HeightmapTags::load(&mut resources, &ids)?;
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
                "unsupported saved block property"
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
            |_| anyhow::bail!("unexpected POI change in player round trip"),
            1,
        );
        if !unavailable.is_empty() {
            ensure!(
                unavailable
                    .iter()
                    .all(|pos| !required_targets.contains(pos)),
                "requested shape dependency remained unavailable: {unavailable:?}"
            );
            let error = plan
                .err()
                .context("save accepted missing shape dependencies")?;
            let message = format!("{error:#}");
            ensure!(
                [
                    "fence shape updates",
                    "lichen shape updates",
                    "stair shape updates"
                ]
                .iter()
                .any(|kind| message.contains(kind)),
                "save rejected for an unexpected reason: {error:#}"
            );
            assert_eq!(std::fs::read(world_path.join("level.dat"))?, initial_level);
            assert_eq!(
                native_persistence::nbt::write(&reader.read_player()?.context("player missing")?)?,
                initial_player
            );
            eprintln!("Save correctly refused for ungenerated shape dependencies: {unavailable:?}");
            ensure!(
                !require_save,
                "play-save requires complete shape dependencies"
            );
            return Ok(());
        }
        let plan = plan.context("cannot draft the ticked generated session")?;
        let mut expected_edits: std::collections::BTreeMap<_, _> = expected
            .world
            .loaded()
            .context("saved session terrain is missing")?
            .block_edits()
            .map(|edit| (edit.position, edit.clone()))
            .collect();
        let mut written_edits = Vec::new();
        let dimension = DimensionId::parse("overworld")?;
        drop(reader);
        let mut writer = lock.acquire_write()?;
        writer.write_plan(plan, |actual_dimension, change| {
            ensure!(
                *actual_dimension == dimension,
                "block edit is in another dimension"
            );
            let position = native_world::loaded::BlockPosition {
                x: change.x,
                y: change.y,
                z: change.z,
            };
            let expected = expected_edits
                .remove(&position)
                .context("block edit was not recorded by the session")?;
            ensure!(
                expected.original.as_ref() == Some(&resolve(&change.expected)?),
                "block edit source changed"
            );
            ensure!(
                expected.replacement == resolve(&change.replacement)?,
                "block edit result changed"
            );
            written_edits.push((position, expected.replacement));
            Ok(())
        })?;
        ensure!(
            expected_edits.is_empty(),
            "session block edits were not included in the save"
        );
        drop(writer);
        let mut reader = WorldReader::open_with_session(
            world_path,
            DimensionId::parse("overworld")?,
            lock.clone(),
        )?;
        let player = reader.read_player()?.context("saved player missing")?;
        let camera = crate::saved_camera(&player)?.context("saved player camera missing")?;
        assert_eq!(camera.yaw, expected.player.yaw.to_degrees() - 180.0);
        assert_eq!(camera.pitch, -expected.player.pitch.to_degrees());
        assert_eq!(reader.game_time()?, expected.game_time as i64);
        for (position, expected_state) in &written_edits {
            let chunk = reader
                .read_chunk(position.x.div_euclid(16), position.z.div_euclid(16))?
                .context("saved edited chunk missing")?;
            let actual = chunk
                .block(position.x, position.y, position.z)
                .context("saved edited block missing")?;
            ensure!(
                resolve(actual)? == *expected_state,
                "saved block state changed at {position:?}"
            );
        }
        eprintln!("Verified {} saved block edits", written_edits.len());
        drop(reader);
        drop(frozen);
        let reopened = crate::prepare_world_with_session(
            world_path,
            game,
            &[],
            crate::ViewOptions {
                dimension: None,
                chunk: None,
                radius: 1,
                smoke_test: false,
                block_report: None,
                play: true,
                video: crate::menu_settings::VideoSettings::default(),
            },
            None,
            Some(lock),
        )?;
        let crate::PreparedWorld::Play(reopened) = reopened else {
            anyhow::bail!("saved generated world did not reopen as a play session");
        };
        let actual = reopened.session.snapshot();
        assert!((actual.player.yaw - expected.player.yaw).abs() < 0.00001);
        assert!((actual.player.pitch - expected.player.pitch).abs() < 0.00001);
        assert_eq!(actual.game_time, expected.game_time);
        let expected_pos = expected.origin.as_vec3() + expected.player.position;
        let actual_pos = actual.origin.as_vec3() + actual.player.position;
        ensure!(
            (expected_pos - actual_pos).length() < 0.0001,
            "saved player position changed"
        );
        Ok(())
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and complete initial neighbor postprocessing"]
    fn generated_world_can_cancel_staging_retry_and_report_failure() -> Result<()> {
        let game =
            native_resources::default_minecraft_dir().context("Minecraft directory missing")?;
        let parent = tempfile::tempdir()?;
        let mut settings = crate::world_creation::WorldCreationSettings::new("Test".into());
        settings.seed = crate::chat_input::ChatInput::field("42");
        settings.generate_structures = true;
        let mut pipeline = InitialWorldPipeline::start(
            settings.freeze(game, None),
            options(parent.path().to_path_buf()),
        )?;
        let deadline = Instant::now() + Duration::from_secs(900);
        loop {
            match pipeline.poll() {
                None => {}
                Some(InitialWorldResult::Failed { error, .. }) => return Err(error),
                Some(_) => anyhow::bail!("generation ended before staging"),
            }
            if matches!(pipeline.progress(), InitialWorldProgress::Staging(_)) {
                break;
            }
            assert!(Instant::now() < deadline, "generation timed out");
            std::thread::sleep(Duration::from_millis(20));
        }
        pipeline.cancel();
        let generation = match wait(&mut pipeline) {
            InitialWorldResult::Cancelled {
                generation: Some(generation),
            } => generation,
            InitialWorldResult::Failed { error, .. } => return Err(error),
            _ => anyhow::bail!("staging cancellation did not retain generation"),
        };
        assert!(pipeline.poll().is_none());
        assert_eq!(std::fs::read_dir(parent.path())?.count(), 0);

        pipeline
            .retry_staging(generation, parent.path().to_path_buf())
            .map_err(|failure| failure.error)?;
        let (generation, world) = match wait(&mut pipeline) {
            InitialWorldResult::Ready {
                generation, world, ..
            } => (generation, world),
            InitialWorldResult::Failed { error, .. } => return Err(error),
            _ => anyhow::bail!("staging retry was cancelled"),
        };
        let stage_path = match &world {
            StagedWorld::Structured(world) => world.path().to_path_buf(),
            StagedWorld::Regular(_) => anyhow::bail!("wrong stage type"),
        };
        assert!(stage_path.join("level.dat").is_file());
        drop(world);
        assert!(!stage_path.exists());
        assert!(matches!(
            pipeline.progress(),
            InitialWorldProgress::Finished
        ));
        assert!(pipeline.poll().is_none());

        let missing = parent.path().join("missing");
        pipeline
            .retry_staging(generation, missing)
            .map_err(|failure| failure.error)?;
        let generation = match wait(&mut pipeline) {
            InitialWorldResult::Failed {
                generation: Some(generation),
                ..
            } => generation,
            _ => anyhow::bail!("staging failure did not retain generation"),
        };
        assert!(pipeline.poll().is_none());
        pipeline
            .retry_staging(generation, parent.path().to_path_buf())
            .map_err(|failure| failure.error)?;
        match wait(&mut pipeline) {
            InitialWorldResult::Ready { world, .. } => drop(world),
            InitialWorldResult::Failed { error, .. } => return Err(error),
            _ => anyhow::bail!("staging retry after failure was cancelled"),
        }
        assert_eq!(std::fs::read_dir(parent.path())?.count(), 0);
        Ok(())
    }
}
