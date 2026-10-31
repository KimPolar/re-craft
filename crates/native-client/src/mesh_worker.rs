//! Build a world mesh from immutable snapshots without blocking the game loop.

use crate::preview::{IncrementalPreview, PreviewScene};
use anyhow::{Context, Result};
use glam::IVec3;
use native_resources::packs::ResourceStack;
use native_world::loaded::World;
use std::{
    sync::{
        Arc, Condvar, Mutex,
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
};

pub struct MeshJob {
    pub revision: u64,
    pub biome_blend: u8,
    pub origin: IVec3,
    pub world: Arc<World>,
}

pub struct MeshResult {
    pub revision: u64,
    pub biome_blend: u8,
    pub scene: Result<PreviewScene>,
}

#[derive(Default)]
struct Queue {
    pending: Option<MeshJob>,
    stopped: bool,
}

pub struct MeshWorker {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    results: Receiver<MeshResult>,
}

impl MeshWorker {
    pub fn start(mut resources: ResourceStack) -> Result<Self> {
        let mut cache = IncrementalPreview::default();
        Self::with_builder(move |job| {
            cache.prepare_with_blend(
                &job.world,
                &mut resources,
                job.origin,
                4096,
                job.biome_blend,
            )
        })
    }

    fn with_builder(
        mut build: impl FnMut(&MeshJob) -> Result<PreviewScene> + Send + 'static,
    ) -> Result<Self> {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let worker_queue = Arc::clone(&queue);
        let (sender, results) = mpsc::channel();
        thread::Builder::new()
            .name("minecraft-mesh-builder".into())
            .spawn(move || {
                loop {
                    let job = {
                        let (lock, wake) = &*worker_queue;
                        let mut queue =
                            lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                        while queue.pending.is_none() && !queue.stopped {
                            queue = wake
                                .wait(queue)
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                        }
                        if queue.stopped {
                            break;
                        }
                        queue.pending.take().expect("pending mesh job was checked")
                    };
                    let revision = job.revision;
                    let biome_blend = job.biome_blend;
                    let scene = build(&job);
                    if sender
                        .send(MeshResult {
                            revision,
                            biome_blend,
                            scene,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .context("cannot start the world mesh worker")?;
        Ok(Self { queue, results })
    }

    /// At most one unstarted job is retained; newer snapshots replace it.
    pub fn submit(&self, job: MeshJob) {
        let (lock, wake) = &*self.queue;
        let mut queue = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        queue.pending = Some(job);
        wake.notify_one();
    }

    pub fn try_result(&self) -> Result<Option<MeshResult>> {
        match self.results.try_recv() {
            Ok(result) => Ok(Some(result)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                anyhow::bail!("world mesh worker stopped unexpectedly")
            }
        }
    }
}

impl Drop for MeshWorker {
    fn drop(&mut self) {
        let (lock, wake) = &*self.queue;
        let mut queue = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        queue.stopped = true;
        queue.pending = None;
        wake.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};

    fn job(revision: u64) -> MeshJob {
        MeshJob {
            revision,
            biome_blend: 0,
            origin: IVec3::ZERO,
            world: Arc::new(World::new("minecraft:overworld")),
        }
    }

    #[test]
    fn pending_mesh_jobs_are_coalesced_without_blocking_submit() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = MeshWorker::with_builder(move |job| {
            started_tx.send(job.revision).unwrap();
            if job.revision == 1 {
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            anyhow::bail!("test build {}", job.revision)
        })
        .unwrap();
        worker.submit(job(1));
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(3)).unwrap(), 1);
        worker.submit(job(2));
        worker.submit(job(3));
        release_tx.send(()).unwrap();
        assert_eq!(
            worker
                .results
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .revision,
            1
        );
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(3)).unwrap(), 3);
        assert_eq!(
            worker
                .results
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .revision,
            3
        );
        assert!(started_rx.try_recv().is_err());
    }

    #[test]
    fn pending_settings_for_the_same_world_revision_keep_the_latest_blend() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = MeshWorker::with_builder(move |job| {
            started_tx.send(job.biome_blend).unwrap();
            if job.biome_blend == 2 {
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            anyhow::bail!("test build")
        })
        .unwrap();
        let mut first = job(1);
        first.biome_blend = 2;
        worker.submit(first);
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(3)).unwrap(), 2);
        let mut next = job(1);
        next.biome_blend = 3;
        worker.submit(next);
        let mut latest = job(1);
        latest.biome_blend = 7;
        worker.submit(latest);
        release_tx.send(()).unwrap();
        assert_eq!(
            worker
                .results
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .biome_blend,
            2
        );
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(3)).unwrap(), 7);
        let result = worker.results.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!((result.revision, result.biome_blend), (1, 7));
        assert!(started_rx.try_recv().is_err());
    }
}
