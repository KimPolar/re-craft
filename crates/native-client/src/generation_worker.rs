//! Prepare a fixed fresh-generation batch away from the window thread.
use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
};

use anyhow::{Context, Result, ensure};
use glam::IVec3;
use native_resources::packs::ResourceProvider;
use native_session::worldgen::{CarverBlockState, FreshTerrainBatch, schedule::GenerationStage};
use native_world::loaded::{BlockState, ChunkPos};
use serde_json::Value;

use crate::mesh_worker::MeshJob;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GenerationPhase {
    Planning,
    Generating,
    Lighting,
    PreparingMesh,
    Ready,
    Cancelled,
    Failed,
}

#[derive(Clone, Copy, Debug)]
pub struct GenerationProgress {
    pub phase: GenerationPhase,
    pub completed_steps: u64,
    pub last_step: Option<(ChunkPos, GenerationStage)>,
}

pub struct GenerationReady {
    pub batch: FreshTerrainBatch,
    pub mesh: MeshJob,
}

impl GenerationReady {
    /// Construct a transient session from fully lit targets. The caller must
    /// choose and validate the player spawn, and keep the batch for generation;
    /// this does not create or save a Java world.
    pub fn start_session(
        &mut self,
        player: native_session::PlayerState,
    ) -> Result<native_session::loaded::LoadedSession> {
        use native_session::{
            inventory::PlayerInventory,
            loaded::{BuildBounds, LoadedSession},
        };
        let targets = self
            .mesh
            .targets
            .as_ref()
            .context("generated session requires light-complete targets")?;
        ensure!(!targets.is_empty(), "generated session has no targets");
        ensure!(
            self.batch.is_complete(),
            "generated session has pending generation work"
        );
        let current = self.batch.lit_render_snapshot(targets.iter().copied())?;
        ensure!(
            Arc::ptr_eq(&current, &self.mesh.world),
            "generated session mesh snapshot is stale"
        );
        let (min_y, max_y) = self.batch.build_bounds()?;
        ensure!(
            player.position.is_finite(),
            "invalid generated player position"
        );
        let absolute = player.position.as_dvec3() + self.mesh.origin.as_dvec3();
        ensure!(
            absolute.x >= f64::from(i32::MIN)
                && absolute.x < f64::from(i32::MAX)
                && absolute.z >= f64::from(i32::MIN)
                && absolute.z < f64::from(i32::MAX)
                && absolute.y >= f64::from(min_y)
                && absolute.y <= f64::from(max_y),
            "generated player position is outside supported bounds"
        );
        let player_chunk = ChunkPos {
            x: (absolute.x.floor() as i32).div_euclid(16),
            z: (absolute.z.floor() as i32).div_euclid(16),
        };
        ensure!(
            targets.contains(&player_chunk),
            "generated player is outside light-complete targets"
        );
        let mut world = (*self.mesh.world).clone();
        world.chunks.retain(|pos, _| targets.contains(pos));
        ensure!(
            world.chunks.len() == targets.len(),
            "generated session target is missing"
        );
        for chunk in world.chunks.values_mut() {
            for sy in min_y.div_euclid(16) - 1..=max_y.div_euclid(16) {
                let section = chunk
                    .sections
                    .get(&i8::try_from(sy)?)
                    .context("generated session light section is missing")?;
                ensure!(
                    section.block_light.is_some() && section.sky_light.is_some(),
                    "generated session light data is missing"
                );
                if sy >= min_y.div_euclid(16) && sy < max_y.div_euclid(16) {
                    ensure!(
                        section.blocks.is_some(),
                        "generated session blocks are missing"
                    );
                }
            }
            Arc::make_mut(chunk).block_light_correct = true;
        }
        let mut session = LoadedSession::new(
            world,
            self.mesh.origin,
            BuildBounds { min_y, max_y },
            player,
        )
        .map_err(anyhow::Error::msg)?;
        session.set_player_inventory(PlayerInventory::default());
        Ok(session)
    }
}

pub enum GenerationResult {
    Ready(GenerationReady),
    Cancelled,
    Failed(anyhow::Error),
}

pub struct GenerationWorker {
    cancelled: Arc<AtomicBool>,
    progress: Arc<Mutex<GenerationProgress>>,
    result: Receiver<GenerationResult>,
}

impl GenerationWorker {
    /// Construct the batch and its resources on the worker thread. They must
    /// use the same immutable pack stack and have decoration/light configured.
    /// This finite batch does not save or start play.
    pub fn start<P, C, R, F>(
        prepare: F,
        targets: BTreeSet<ChunkPos>,
        revision: u64,
        biome_blend: u8,
        origin: IVec3,
    ) -> Result<Self>
    where
        F: FnOnce() -> Result<(FreshTerrainBatch, P, C, R)> + Send + 'static,
        P: ResourceProvider,
        C: FnMut(&Value) -> Result<CarverBlockState>,
        R: FnMut(&Value) -> Result<BlockState>,
    {
        Self::spawn(move |cancelled, progress| {
            ensure!(!targets.is_empty(), "no generation targets");
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let mut initialized = BTreeSet::new();
            for pos in &targets {
                for dz in -1i32..=1 {
                    for dx in -1i32..=1 {
                        initialized.insert(ChunkPos {
                            x: pos.x.checked_add(dx).context("generation X overflow")?,
                            z: pos.z.checked_add(dz).context("generation Z overflow")?,
                        });
                    }
                }
            }
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let (mut batch, mut provider, mut classify, mut resolve) = prepare()?;
            ensure!(
                batch.is_complete() && batch.schedule().requested_chunks() == 0,
                "generation batch must be fresh"
            );
            for pos in initialized {
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                batch.request_light_initialization(pos)?;
            }
            set_phase(progress, GenerationPhase::Generating);
            while let Some((pos, stage)) = {
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                generation_step(&mut batch, &mut provider, &mut classify, &mut resolve)?
            } {
                note_step(progress, pos, stage);
            }
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            batch.seal_for_light_propagation()?;
            set_phase(progress, GenerationPhase::Lighting);
            for &pos in &targets {
                batch.request_light(pos)?;
            }
            while let Some((pos, stage)) = {
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                generation_step(&mut batch, &mut provider, &mut classify, &mut resolve)?
            } {
                note_step(progress, pos, stage);
            }
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            set_phase(progress, GenerationPhase::PreparingMesh);
            let mesh = MeshJob::for_generated(&mut batch, targets, revision, biome_blend, origin)?;
            Ok(Some(GenerationReady { batch, mesh }))
        })
    }

    fn spawn(
        task: impl FnOnce(&AtomicBool, &Mutex<GenerationProgress>) -> Result<Option<GenerationReady>>
        + Send
        + 'static,
    ) -> Result<Self> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Mutex::new(GenerationProgress {
            phase: GenerationPhase::Planning,
            completed_steps: 0,
            last_step: None,
        }));
        let worker_cancelled = Arc::clone(&cancelled);
        let worker_progress = Arc::clone(&progress);
        let (sender, result) = mpsc::channel();
        thread::Builder::new()
            .name("minecraft-world-generator".into())
            .spawn(move || {
                let outcome = match task(&worker_cancelled, &worker_progress) {
                    Ok(Some(ready)) if !worker_cancelled.load(Ordering::Relaxed) => {
                        GenerationResult::Ready(ready)
                    }
                    Ok(_) => GenerationResult::Cancelled,
                    Err(error) => GenerationResult::Failed(error),
                };
                let phase = match &outcome {
                    GenerationResult::Ready(_) => GenerationPhase::Ready,
                    GenerationResult::Cancelled => GenerationPhase::Cancelled,
                    GenerationResult::Failed(_) => GenerationPhase::Failed,
                };
                set_phase(&worker_progress, phase);
                let _ = sender.send(outcome);
            })
            .context("cannot start world generation worker")?;
        Ok(Self {
            cancelled,
            progress,
            result,
        })
    }

    /// Cancellation is cooperative between steps; resource setup, an in-flight
    /// chunk calculation or light snapshot is allowed to finish first.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
    pub fn progress(&self) -> GenerationProgress {
        *self.progress.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn try_result(&self) -> Result<Option<GenerationResult>> {
        match self.result.try_recv() {
            Ok(result) => Ok(Some(result)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                anyhow::bail!("world generation worker stopped unexpectedly")
            }
        }
    }
}

fn generation_step(
    batch: &mut FreshTerrainBatch,
    provider: &mut impl ResourceProvider,
    classify: &mut impl FnMut(&Value) -> Result<CarverBlockState>,
    resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
) -> Result<Option<(ChunkPos, GenerationStage)>> {
    let next = batch.schedule().next_ready();
    batch
        .step(provider, classify, resolve)
        .with_context(|| format!("generation step {next:?} failed"))
}

impl Drop for GenerationWorker {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn set_phase(progress: &Mutex<GenerationProgress>, phase: GenerationPhase) {
    progress.lock().unwrap_or_else(|e| e.into_inner()).phase = phase;
}
fn note_step(progress: &Mutex<GenerationProgress>, pos: ChunkPos, stage: GenerationStage) {
    let mut progress = progress.lock().unwrap_or_else(|e| e.into_inner());
    progress.completed_steps = progress.completed_steps.saturating_add(1);
    progress.last_step = Some((pos, stage));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn cancellation_and_errors_report_terminal_state() {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = GenerationWorker::spawn(move |cancelled, progress| {
            set_phase(progress, GenerationPhase::Generating);
            note_step(progress, ChunkPos { x: 0, z: 0 }, GenerationStage::Biomes);
            entered_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            assert!(cancelled.load(Ordering::Relaxed));
            Ok(None)
        })
        .unwrap();
        entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(worker.progress().phase, GenerationPhase::Generating);
        assert_eq!(worker.progress().completed_steps, 1);
        worker.cancel();
        release_tx.send(()).unwrap();
        assert!(matches!(
            worker.result.recv_timeout(Duration::from_secs(3)).unwrap(),
            GenerationResult::Cancelled
        ));
        assert_eq!(worker.progress().phase, GenerationPhase::Cancelled);
        let worker = GenerationWorker::spawn(|_, _| anyhow::bail!("fixture failure")).unwrap();
        let GenerationResult::Failed(error) =
            worker.result.recv_timeout(Duration::from_secs(3)).unwrap()
        else {
            panic!("expected failure")
        };
        assert!(error.to_string().contains("fixture failure"));
        assert_eq!(worker.progress().phase, GenerationPhase::Failed);
    }

    #[test]
    fn invalid_targets_fail_before_resource_setup() {
        struct UnusedProvider;
        impl ResourceProvider for UnusedProvider {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
        }
        type Prepared = (
            FreshTerrainBatch,
            UnusedProvider,
            fn(&Value) -> Result<CarverBlockState>,
            fn(&Value) -> Result<BlockState>,
        );
        fn never_prepare() -> Result<Prepared> {
            panic!("resource setup must not run")
        }
        for targets in [
            BTreeSet::new(),
            BTreeSet::from([ChunkPos { x: i32::MAX, z: 0 }]),
        ] {
            let worker =
                GenerationWorker::start(never_prepare, targets, 0, 0, IVec3::ZERO).unwrap();
            let GenerationResult::Failed(_) =
                worker.result.recv_timeout(Duration::from_secs(3)).unwrap()
            else {
                panic!("expected invalid targets to fail")
            };
            assert_eq!(worker.progress().phase, GenerationPhase::Failed);
        }
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and a full generation batch"]
    fn installed_overworld_generates_a_lit_mesh_job() -> Result<()> {
        installed_overworld_generation(false)
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and a full generation batch"]
    fn installed_overworld_expands_to_adjacent_lit_mesh() -> Result<()> {
        installed_overworld_generation(true)
    }

    fn installed_overworld_generation(expand: bool) -> Result<()> {
        use crate::fresh_generation::{FreshGenerationSettings, prepare};

        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mesh_game = game.clone();
        let targets = BTreeSet::from([ChunkPos { x: 0, z: 0 }]);
        let worker = GenerationWorker::start(
            move || {
                Ok(prepare(FreshGenerationSettings {
                    minecraft_dir: game,
                    preset: "minecraft:normal".into(),
                    dimension: "minecraft:overworld".into(),
                    seed: 42,
                    structures_enabled: false,
                    data_packs: None,
                })?
                .into_worker_inputs())
            },
            targets.clone(),
            7,
            0,
            IVec3::ZERO,
        )?;
        let result = worker.result.recv_timeout(Duration::from_secs(300))?;
        let mut generated = match result {
            GenerationResult::Ready(generated) => generated,
            GenerationResult::Failed(error) => {
                return Err(error.context(format!("generation progress: {:?}", worker.progress())));
            }
            GenerationResult::Cancelled => anyhow::bail!("generation was cancelled"),
        };
        assert_eq!(worker.progress().phase, GenerationPhase::Ready);
        assert_eq!(generated.mesh.targets.as_deref(), Some(&targets));
        assert_eq!(generated.mesh.revision, 7);
        assert!(
            generated
                .mesh
                .world
                .chunks
                .contains_key(&ChunkPos { x: 0, z: 0 })
        );
        {
            use native_session::{GameSession, PlayerPose, PlayerState};
            let player = PlayerState {
                position: glam::Vec3::new(8.5, 150.0, 8.5),
                yaw: 0.0,
                pitch: 0.0,
                vertical_velocity: 0.0,
                on_ground: false,
                pose: PlayerPose::Standing,
            };
            let session = generated.start_session(player)?;
            let snapshot = session.snapshot();
            let world = snapshot
                .world
                .loaded()
                .context("generated session world missing")?;
            assert_eq!(world.chunks.len(), 1);
            assert!(world.chunks[&ChunkPos { x: 0, z: 0 }].block_light_correct);
        }
        let mesher = crate::mesh_worker::MeshWorker::start(
            native_resources::packs::ResourceStack::open(&mesh_game)?,
        )?;
        mesher.submit(generated.mesh);
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        let mesh = loop {
            if let Some(result) = mesher.try_result()? {
                break result;
            }
            anyhow::ensure!(
                std::time::Instant::now() < deadline,
                "generated mesh timed out"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(mesh.revision, 7);
        assert_eq!(mesh.targets.as_deref(), Some(&targets));
        let scene = mesh.scene?;
        assert_eq!(scene.chunks.len(), 1);
        assert!(scene.chunks.contains_key(&ChunkPos { x: 0, z: 0 }));
        assert!(scene.opaque_len() > 0, "generated terrain mesh is empty");
        if !expand {
            return Ok(());
        }

        // Continue the same generated batch toward the immediately adjacent
        // chunk. Reopen the same vanilla pack stack for the test driver.
        let mut batch = generated.batch;
        let (_, mut provider, mut classify, mut resolve) = prepare(FreshGenerationSettings {
            minecraft_dir: mesh_game,
            preset: "minecraft:normal".into(),
            dimension: "minecraft:overworld".into(),
            seed: 42,
            structures_enabled: false,
            data_packs: None,
        })?
        .into_worker_inputs();
        let east = ChunkPos { x: 1, z: 0 };
        let edge: Vec<_> = (-1..=1).map(|z| ChunkPos { x: 2, z }).collect();
        for &pos in &edge {
            batch.request_features(pos)?;
        }
        while generation_step(&mut batch, &mut provider, &mut classify, &mut resolve)?.is_some() {}
        for &pos in &edge {
            batch.request_light_initialization(pos)?;
        }
        while generation_step(&mut batch, &mut provider, &mut classify, &mut resolve)?.is_some() {}
        batch.request_light(east)?;
        while generation_step(&mut batch, &mut provider, &mut classify, &mut resolve)?.is_some() {}
        let expanded_targets = BTreeSet::from([ChunkPos { x: 0, z: 0 }, east]);
        let job = MeshJob::for_generated(&mut batch, expanded_targets.clone(), 8, 0, IVec3::ZERO)?;
        mesher.submit(job);
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        let mesh = loop {
            if let Some(result) = mesher.try_result()? {
                break result;
            }
            anyhow::ensure!(
                std::time::Instant::now() < deadline,
                "expanded mesh timed out"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(mesh.revision, 8);
        assert_eq!(mesh.targets.as_deref(), Some(&expanded_targets));
        let scene = mesh.scene?;
        assert_eq!(scene.chunks.len(), 2);
        assert!(scene.chunks.contains_key(&east));
        assert!(scene.opaque_len() > 0);
        Ok(())
    }
}
