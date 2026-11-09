//! Nonblocking connection between initial generation and private world staging.
use std::path::PathBuf;

use anyhow::Result;
use native_persistence::world::SavedWorldBorder;

use crate::{
    generation_staging::{InitialWorldStagingWorker, StagedWorld, StagingProgress, StagingResult},
    generation_worker::{GenerationProgress, GenerationReady, GenerationResult, GenerationWorker},
    world_creation::FrozenWorldCreation,
};

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
    },
    Cancelled {
        /// Available when generation completed before cancellation.
        generation: Option<GenerationReady>,
    },
    Failed {
        error: anyhow::Error,
        /// Staging failures retain their input so a caller can retry.
        generation: Option<GenerationReady>,
    },
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
    cancelled: bool,
}

impl InitialWorldPipeline {
    /// Freeze settings before this call. The returned stage is never published.
    pub fn start(frozen: FrozenWorldCreation, options: InitialWorldOptions) -> Result<Self> {
        let structures = frozen.generation.structures_enabled;
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

    /// Poll once without waiting. A terminal result is returned at most once.
    pub fn poll(&mut self) -> Option<InitialWorldResult> {
        match &self.state {
            State::Generating(worker) => match worker.try_result() {
                Ok(None) => None,
                Ok(Some(GenerationResult::Ready(generation))) if !self.cancelled => {
                    match InitialWorldStagingWorker::start(
                        generation,
                        self.options.staging_parent.clone(),
                        self.structures,
                        self.options.last_played_ms,
                        self.options.region_timestamp,
                    ) {
                        Ok(worker) => {
                            self.state = State::Staging(worker);
                            None
                        }
                        Err(error) => {
                            self.state = State::Finished;
                            Some(InitialWorldResult::Failed {
                                error: error.context("cannot start initial world staging"),
                                generation: None,
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
                    Some(InitialWorldResult::Ready { generation, world })
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
    use anyhow::Context;
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
        let deadline = Instant::now() + Duration::from_secs(900);
        loop {
            if let Some(result) = pipeline.poll() {
                return result;
            }
            assert!(
                Instant::now() < deadline,
                "initial world pipeline timed out"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
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

        let worker = InitialWorldStagingWorker::start(
            generation,
            parent.path().to_path_buf(),
            true,
            123,
            1,
        )?;
        let mut retry = InitialWorldPipeline {
            state: State::Staging(worker),
            options: options(parent.path().to_path_buf()),
            structures: true,
            cancelled: false,
        };
        let (generation, world) = match wait(&mut retry) {
            InitialWorldResult::Ready { generation, world } => (generation, world),
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
        assert!(matches!(retry.progress(), InitialWorldProgress::Finished));
        assert!(retry.poll().is_none());

        let missing = parent.path().join("missing");
        let worker = InitialWorldStagingWorker::start(generation, missing.clone(), true, 123, 1)?;
        let mut failed = InitialWorldPipeline {
            state: State::Staging(worker),
            options: options(missing),
            structures: true,
            cancelled: false,
        };
        assert!(matches!(
            wait(&mut failed),
            InitialWorldResult::Failed {
                generation: Some(_),
                ..
            }
        ));
        assert!(failed.poll().is_none());
        Ok(())
    }
}
