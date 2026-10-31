//! Read saved chunks on a worker; publish completed chunks at session tick boundaries.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use native_persistence::{
    heightmap::{HeightBounds, SpawnHeightmaps},
    world::{ChunkArea, ChunkPos as SavedChunkPos, WorldReader},
};
use native_resources::heightmap_tags::HeightmapTags;
use native_session::{
    GameSession,
    loaded::{LoadedSession, SavedEntityData, SavedPoiData},
};
use native_world::loaded::{ChunkPos, World};

type Defaults = BTreeMap<String, BTreeMap<String, String>>;
type Window = (ChunkPos, u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ActiveWindows {
    primary: Window,
    respawn: Option<Window>,
}

impl ActiveWindows {
    fn contains(self, pos: ChunkPos) -> bool {
        within(pos, self.primary) || self.respawn.is_some_and(|window| within(pos, window))
    }
}

struct Response {
    pos: ChunkPos,
    result: Result<Option<ChunkPayload>, String>,
    stale: bool,
}

#[derive(Clone, Copy)]
struct Request {
    pos: ChunkPos,
    entities_only: bool,
}

struct ChunkPayload {
    world: Option<World>,
    entities: SavedEntityData,
    poi: SavedPoiData,
    heightmaps: Option<SpawnHeightmaps>,
}

pub struct ChunkStreamer {
    requests: Option<SyncSender<Request>>,
    responses: Receiver<Response>,
    window: Arc<Mutex<Option<ActiveWindows>>>,
    stop: Arc<AtomicBool>,
    center: Option<ChunkPos>,
    respawn: Option<Window>,
    radius: u32,
    targets: Vec<ChunkPos>,
    settled_prefix: usize,
    #[cfg(test)]
    skipped_target_checks: u64,
    pending: BTreeSet<ChunkPos>,
    missing: BTreeSet<ChunkPos>,
    failed: BTreeSet<ChunkPos>,
    entity_retry: BTreeMap<ChunkPos, (Instant, u32)>,
    _world_guard: Option<native_persistence::save::WorldSessionGuard>,
}

impl ChunkStreamer {
    pub fn radius(&self) -> u32 {
        self.radius
    }

    pub fn set_radius(&mut self, radius: u32) {
        let radius = radius.max(1);
        if self.radius != radius {
            self.radius = radius;
            // Force the next service call to rebuild the worker window and
            // update the session even when the player has not moved.
            self.center = None;
        }
    }

    pub fn start(
        mut reader: WorldReader,
        defaults: Defaults,
        radius: u32,
        missing: BTreeSet<ChunkPos>,
        heightmap_context: Option<(HeightBounds, Arc<HeightmapTags>)>,
    ) -> Result<Self> {
        let guard = reader.session_guard();
        let mut streamer = Self::with_entity_source(radius, missing, move |pos, entities_only| {
            if entities_only {
                return Ok(Some(ChunkPayload {
                    world: None,
                    entities: read_entity_data(&mut reader, pos),
                    poi: read_poi_data(&mut reader, pos),
                    heightmaps: None,
                }));
            }
            let Some(chunk) = reader.read_chunk(pos.x, pos.z)? else {
                return Ok(None);
            };
            let saved = SavedChunkPos { x: pos.x, z: pos.z };
            let chunk = Arc::new(chunk);
            let area = ChunkArea {
                center: saved,
                radius: 0,
                chunks: BTreeMap::from([(saved, Arc::clone(&chunk))]),
                missing: Vec::new(),
            };
            let world = area.materialize_with_defaults(reader.dimension(), &defaults)?;
            let entities = read_entity_data(&mut reader, pos);
            let poi = read_poi_data(&mut reader, pos);
            let heightmaps = heightmap_context.as_ref().and_then(|(bounds, tags)| {
                match native_session::spawn_heightmaps::prepare(&chunk, &world, *bounds, tags) {
                    Ok(maps) => Some(maps),
                    Err(error) => {
                        eprintln!(
                            "Could not prepare spawn heightmaps for chunk {}, {}: {error:#}",
                            pos.x, pos.z
                        );
                        None
                    }
                }
            });
            Ok(Some(ChunkPayload {
                world: Some(world),
                entities,
                poi,
                heightmaps,
            }))
        })?;
        streamer._world_guard = guard;
        Ok(streamer)
    }

    #[cfg(test)]
    fn with_source(
        radius: u32,
        missing: BTreeSet<ChunkPos>,
        mut read: impl FnMut(ChunkPos) -> Result<Option<World>> + Send + 'static,
    ) -> Result<Self> {
        Self::with_entity_source(radius, missing, move |pos, _| {
            Ok(read(pos)?.map(|world| ChunkPayload {
                world: Some(world),
                entities: SavedEntityData::Unavailable,
                poi: SavedPoiData::Unavailable,
                heightmaps: None,
            }))
        })
    }

    fn with_entity_source(
        radius: u32,
        missing: BTreeSet<ChunkPos>,
        mut read: impl FnMut(ChunkPos, bool) -> Result<Option<ChunkPayload>> + Send + 'static,
    ) -> Result<Self> {
        let (requests, request_rx) = mpsc::sync_channel::<Request>(8);
        let (response_tx, responses) = mpsc::channel();
        let window = Arc::new(Mutex::new(None::<ActiveWindows>));
        let worker_window = Arc::clone(&window);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        thread::Builder::new()
            .name("minecraft-chunk-reader".into())
            .spawn(move || {
                while let Ok(request) = request_rx.recv() {
                    let pos = request.pos;
                    if worker_stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let current = *worker_window
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let stale = current.is_some_and(|windows| !windows.contains(pos));
                    let result = if stale {
                        Ok(None)
                    } else {
                        read(pos, request.entities_only).map_err(|error| format!("{error:#}"))
                    };
                    if response_tx.send(Response { pos, result, stale }).is_err() {
                        break;
                    }
                }
            })
            .context("cannot start the chunk reader thread")?;
        Ok(Self {
            requests: Some(requests),
            responses,
            window,
            stop,
            center: None,
            respawn: None,
            radius: radius.max(1),
            targets: Vec::new(),
            settled_prefix: 0,
            #[cfg(test)]
            skipped_target_checks: 0,
            pending: BTreeSet::new(),
            missing,
            failed: BTreeSet::new(),
            entity_retry: BTreeMap::new(),
            _world_guard: None,
        })
    }

    pub fn service(&mut self, session: &mut LoadedSession) -> Result<()> {
        let center = session
            .player_chunk()
            .context("player is outside the supported block coordinate range")?;
        let moved = self.center != Some(center);
        let respawn = session.requested_respawn_window();
        let windows_changed = moved || self.respawn != respawn;
        let windows = ActiveWindows {
            primary: (center, self.radius),
            respawn,
        };
        if windows_changed {
            self.settled_prefix = 0;
            self.center = Some(center);
            self.respawn = respawn;
            *self
                .window
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(windows);
            let mut seen = BTreeSet::new();
            self.targets = respawn
                .into_iter()
                .flat_map(|(center, radius)| targets(center, radius))
                .chain(targets(center, self.radius))
                .filter(|pos| seen.insert(*pos))
                .collect();
            self.entity_retry.retain(|pos, _| windows.contains(*pos));
            for &pos in &self.targets {
                if matches!(
                    session.saved_entity_chunk(pos),
                    Some(SavedEntityData::Unavailable)
                ) || (session.can_retry_poi_chunk(pos)
                    && matches!(
                        session.saved_poi_chunk(pos),
                        Some(SavedPoiData::Unavailable)
                    ))
                {
                    self.entity_retry
                        .entry(pos)
                        .or_insert((Instant::now() + Duration::from_secs(5), 0));
                }
            }
            if moved {
                session.queue_chunk_window(center, self.radius);
            }
            session.queue_respawn_window(respawn);
            let snapshot = session.snapshot();
            let world = snapshot
                .world
                .loaded()
                .context("streamed session has no loaded world")?;
            for pos in self
                .targets
                .iter()
                .filter(|pos| within(**pos, windows.primary) && self.missing.contains(pos))
            {
                if !world.missing_chunks.contains(pos) {
                    session.queue_missing_chunk(*pos);
                }
            }
        }
        let mut received = false;
        loop {
            match self.responses.try_recv() {
                Ok(response) => {
                    received = true;
                    self.pending.remove(&response.pos);
                    // The player may move while an I/O operation is running.
                    // Do not let an obsolete missing/error result poison a
                    // future visit to that chunk.
                    if response.stale || !windows.contains(response.pos) {
                        continue;
                    }
                    match response.result {
                        Ok(Some(ChunkPayload {
                            world,
                            entities,
                            poi,
                            heightmaps,
                        })) => {
                            if matches!(entities, SavedEntityData::Unavailable)
                                || (session.can_retry_poi_chunk(response.pos)
                                    && matches!(poi, SavedPoiData::Unavailable))
                            {
                                let attempts = self
                                    .entity_retry
                                    .get(&response.pos)
                                    .map_or(0, |(_, attempts)| attempts.saturating_add(1));
                                let delay = 5_u64.saturating_mul(1_u64 << attempts.min(4));
                                self.entity_retry.insert(
                                    response.pos,
                                    (Instant::now() + Duration::from_secs(delay), attempts),
                                );
                            } else {
                                self.entity_retry.remove(&response.pos);
                            }
                            if let Some(world) = world {
                                session.queue_loaded_chunk_with_poi(
                                    response.pos,
                                    world,
                                    entities,
                                    Some(poi),
                                    heightmaps,
                                );
                            } else {
                                session.queue_entity_chunk(response.pos, entities);
                                session.queue_poi_chunk(response.pos, poi);
                            }
                        }
                        Ok(None) => {
                            self.missing.insert(response.pos);
                            if within(response.pos, (center, self.radius)) {
                                session.queue_missing_chunk(response.pos);
                            }
                        }
                        Err(error) => {
                            self.failed.insert(response.pos);
                            eprintln!(
                                "Could not load chunk {}, {}: {error}",
                                response.pos.x, response.pos.z
                            );
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    anyhow::bail!("chunk reader stopped unexpectedly")
                }
            }
        }
        let retry_due = self.entity_retry.iter().any(|(pos, (at, _))| {
            *at <= Instant::now()
                && !self.pending.contains(pos)
                && !self.failed.contains(pos)
                && !self.missing.contains(pos)
        });
        if windows_changed || received || retry_due {
            while let Some(&pos) = self.targets.get(self.settled_prefix) {
                if !self.missing.contains(&pos)
                    && !self.failed.contains(&pos)
                    && !session.has_resident_or_parked_chunk(pos)
                {
                    break;
                }
                self.settled_prefix += 1;
            }
            // Entity retries can become due for already settled chunks.
            // Rescan in the original target order only when a retry is due.
            let start = if retry_due { 0 } else { self.settled_prefix };
            #[cfg(test)]
            {
                self.skipped_target_checks += start as u64;
            }
            for &pos in &self.targets[start..] {
                let retry = self
                    .entity_retry
                    .get(&pos)
                    .is_some_and(|(at, _)| *at <= Instant::now());
                if self.missing.contains(&pos)
                    || self.failed.contains(&pos)
                    || self.pending.contains(&pos)
                    || (session.has_chunk_or_pending(pos) && !retry)
                {
                    continue;
                }
                match self
                    .requests
                    .as_ref()
                    .context("chunk reader is closed")?
                    .try_send(Request {
                        pos,
                        entities_only: retry && session.has_chunk_or_pending(pos),
                    }) {
                    Ok(()) => {
                        self.pending.insert(pos);
                    }
                    Err(TrySendError::Full(_)) => break,
                    Err(TrySendError::Disconnected(_)) => {
                        anyhow::bail!("chunk reader stopped unexpectedly")
                    }
                }
            }
        }
        for error in session.take_stream_errors() {
            eprintln!("Could not attach streamed {error}");
        }
        Ok(())
    }
}

pub(crate) fn read_entity_data(reader: &mut WorldReader, pos: ChunkPos) -> SavedEntityData {
    match reader.read_entity_chunk(pos.x, pos.z) {
        Ok(None) => SavedEntityData::Empty,
        Ok(Some(chunk)) => SavedEntityData::Present(Arc::new(chunk)),
        Err(error) => {
            eprintln!(
                "Could not read entity chunk {}, {}: {error:#}",
                pos.x, pos.z
            );
            SavedEntityData::Unavailable
        }
    }
}

pub(crate) fn read_poi_data(reader: &mut WorldReader, pos: ChunkPos) -> SavedPoiData {
    match reader.read_poi_chunk(pos.x, pos.z) {
        Ok(None) => SavedPoiData::Missing,
        Ok(Some(chunk)) => SavedPoiData::Present(Arc::new(chunk)),
        Err(error) => {
            eprintln!("Could not read POI chunk {}, {}: {error:#}", pos.x, pos.z);
            SavedPoiData::Unavailable
        }
    }
}

impl Drop for ChunkStreamer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.requests.take();
    }
}

fn within(pos: ChunkPos, (center, radius): Window) -> bool {
    (i64::from(pos.x) - i64::from(center.x)).abs() <= i64::from(radius)
        && (i64::from(pos.z) - i64::from(center.z)).abs() <= i64::from(radius)
}

fn targets(center: ChunkPos, radius: u32) -> Vec<ChunkPos> {
    let mut positions = Vec::new();
    let radius = i64::from(radius);
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            let (Ok(x), Ok(z)) = (
                i32::try_from(i64::from(center.x) + dx),
                i32::try_from(i64::from(center.z) + dz),
            ) else {
                continue;
            };
            // A chunk whose blocks cannot be represented by the renderer remains unavailable.
            if x.checked_mul(16).is_none() || z.checked_mul(16).is_none() {
                continue;
            }
            positions.push(ChunkPos { x, z });
        }
    }
    positions.sort_by_key(|pos| {
        let dx = (i64::from(pos.x) - i64::from(center.x)).abs();
        let dz = (i64::from(pos.z) - i64::from(center.z)).abs();
        (dx.max(dz), dx * dx + dz * dz, pos.z, pos.x)
    });
    positions
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{IVec3, Vec3};
    use native_session::{PlayerCommand, PlayerState, loaded::BuildBounds};
    use native_world::loaded::{BLOCK_COUNT, BlockState, Chunk, Section};
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    #[cfg(windows)]
    fn locked_world_fixture() -> (tempfile::TempDir, WorldReader) {
        use flate2::{Compression, write::GzEncoder};
        use native_persistence::{
            nbt::{self, NamedTag, Tag},
            world::DimensionId,
        };
        use std::io::Write;
        let parent =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/stream-lock-tests");
        std::fs::create_dir_all(&parent).unwrap();
        let dir = tempfile::Builder::new()
            .prefix("world-")
            .tempdir_in(parent)
            .unwrap();
        let level = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "Data".into(),
                Tag::Compound(vec![("DataVersion".into(), Tag::Int(5023))]),
            )]),
        };
        let mut output = GzEncoder::new(Vec::new(), Compression::default());
        output.write_all(&nbt::write(&level).unwrap()).unwrap();
        std::fs::write(dir.path().join("level.dat"), output.finish().unwrap()).unwrap();
        let reader =
            WorldReader::open_for_session(dir.path(), DimensionId::parse("overworld").unwrap())
                .unwrap();
        (dir, reader)
    }

    #[cfg(windows)]
    fn wait_for_world_unlock(root: &std::path::Path) {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if native_persistence::save::WorldWriteGuard::acquire(root).is_ok() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "chunk reader kept the world locked after stopping"
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[cfg(windows)]
    #[test]
    fn streamer_retains_the_world_lock_until_it_stops() {
        let (world, reader) = locked_world_fixture();
        let streamer =
            ChunkStreamer::start(reader, BTreeMap::new(), 1, BTreeSet::new(), None).unwrap();
        assert!(native_persistence::save::WorldWriteGuard::acquire(world.path()).is_err());
        drop(streamer);
        wait_for_world_unlock(world.path());
    }

    #[cfg(windows)]
    #[test]
    fn closing_does_not_release_the_lock_while_a_background_read_is_running() {
        let (world, reader) = locked_world_fixture();
        let guard = reader.session_guard().unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let mut streamer = ChunkStreamer::with_source(1, BTreeSet::new(), move |_| {
            let _hold_reader = &reader;
            started_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            Ok(None)
        })
        .unwrap();
        streamer._world_guard = Some(guard);
        let player = PlayerState {
            position: Vec3::new(8.5, 1.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let mut session = LoadedSession::new(
            chunk_world(ChunkPos { x: 0, z: 0 }),
            IVec3::ZERO,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        streamer.service(&mut session).unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        drop(streamer);
        assert!(native_persistence::save::WorldWriteGuard::acquire(world.path()).is_err());
        release_tx.send(()).unwrap();
        wait_for_world_unlock(world.path());
    }

    #[cfg(windows)]
    #[test]
    fn stopped_reader_does_not_unlock_a_still_active_session() {
        let (world, reader) = locked_world_fixture();
        let guard = reader.session_guard().unwrap();
        let mut streamer = ChunkStreamer::with_source(1, BTreeSet::new(), move |_| {
            let _hold_reader = &reader;
            panic!("simulated reader failure");
        })
        .unwrap();
        streamer._world_guard = Some(guard);
        let player = PlayerState {
            position: Vec3::new(8.5, 1.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let mut session = LoadedSession::new(
            chunk_world(ChunkPos { x: 0, z: 0 }),
            IVec3::ZERO,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while streamer.service(&mut session).is_ok() {
            assert!(Instant::now() < deadline, "reader failure was not detected");
            thread::sleep(Duration::from_millis(2));
        }
        assert!(native_persistence::save::WorldWriteGuard::acquire(world.path()).is_err());
        drop(streamer);
        wait_for_world_unlock(world.path());
    }

    #[test]
    fn retries_unavailable_entities_or_poi_for_a_resident_chunk() {
        for retry_poi in [false, true] {
            let home = ChunkPos { x: 0, z: 0 };
            let player = PlayerState {
                position: Vec3::new(8.5, 1.0, 8.5),
                yaw: 0.0,
                pitch: 0.0,
                vertical_velocity: 0.0,
                on_ground: false,
                pose: native_session::PlayerPose::Standing,
            };
            let mut session = LoadedSession::new(
                chunk_world(home),
                IVec3::ZERO,
                BuildBounds {
                    min_y: 0,
                    max_y: 32,
                },
                player,
            )
            .unwrap();
            session
                .set_initial_entity_chunk(
                    home,
                    if retry_poi {
                        SavedEntityData::Empty
                    } else {
                        SavedEntityData::Unavailable
                    },
                )
                .unwrap();
            session
                .set_initial_poi_chunk(
                    home,
                    if retry_poi {
                        SavedPoiData::Unavailable
                    } else {
                        SavedPoiData::Missing
                    },
                )
                .unwrap();
            let mut streamer =
                ChunkStreamer::with_entity_source(1, BTreeSet::new(), move |pos, entities_only| {
                    if pos == home {
                        assert!(entities_only);
                    }
                    Ok((pos == home).then_some(ChunkPayload {
                        world: None,
                        entities: SavedEntityData::Empty,
                        poi: SavedPoiData::Missing,
                        heightmaps: None,
                    }))
                })
                .unwrap();
            streamer.service(&mut session).unwrap();
            assert!(streamer.entity_retry.contains_key(&home));
            streamer.entity_retry.get_mut(&home).unwrap().0 = Instant::now();
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                streamer.service(&mut session).unwrap();
                session.update(Duration::from_millis(50));
                if matches!(
                    session.saved_entity_chunk(home),
                    Some(SavedEntityData::Empty)
                ) && matches!(session.saved_poi_chunk(home), Some(SavedPoiData::Missing))
                {
                    break;
                }
                assert!(Instant::now() < deadline, "entity retry did not complete");
                thread::sleep(Duration::from_millis(2));
            }
            assert!(!streamer.entity_retry.contains_key(&home));
        }
    }

    #[test]
    fn obsolete_missing_and_error_responses_do_not_poison_future_loads() {
        let home = ChunkPos { x: 0, z: 0 };
        let player = PlayerState {
            position: Vec3::new(8.5, 1.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let mut session = LoadedSession::new(
            chunk_world(home),
            IVec3::ZERO,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        let mut streamer = ChunkStreamer::with_source(1, BTreeSet::new(), |_| Ok(None)).unwrap();
        let (sender, receiver) = mpsc::channel();
        streamer.responses = receiver;
        streamer.center = Some(home);
        let missing = ChunkPos { x: 4, z: 0 };
        let failed = ChunkPos { x: 5, z: 0 };
        streamer.pending.extend([missing, failed]);
        sender
            .send(Response {
                pos: missing,
                result: Ok(None),
                stale: false,
            })
            .unwrap();
        sender
            .send(Response {
                pos: failed,
                result: Err("transient".into()),
                stale: false,
            })
            .unwrap();
        streamer.service(&mut session).unwrap();
        assert!(!streamer.missing.contains(&missing));
        assert!(!streamer.failed.contains(&failed));
        assert!(!streamer.pending.contains(&missing));
        assert!(!streamer.pending.contains(&failed));
    }

    fn chunk_world(pos: ChunkPos) -> World {
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let stone = world.intern_state(BlockState {
            name: "minecraft:stone".into(),
            properties: BTreeMap::new(),
        });
        let mut blocks = Box::new([air; BLOCK_COUNT]);
        for z in 0..16 {
            for x in 0..16 {
                blocks[x + z * 16] = stone;
            }
        }
        world.chunks.insert(
            pos,
            Arc::new(Chunk {
                block_light_correct: false,
                position: pos,
                status: Some("minecraft:full".into()),
                sections: BTreeMap::from([(
                    0,
                    Arc::new(Section {
                        blocks: Some(blocks),
                        biomes: None,
                        block_light: None,
                        sky_light: None,
                    }),
                )]),
            }),
        );
        world
    }

    #[test]
    fn changing_radius_without_moving_loads_and_unloads_chunks() {
        let home = ChunkPos { x: 0, z: 0 };
        let far = ChunkPos { x: 2, z: 0 };
        let player = PlayerState {
            position: Vec3::new(8.5, 1.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let mut session = LoadedSession::new(
            chunk_world(home),
            IVec3::ZERO,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        let mut streamer = ChunkStreamer::with_source(1, BTreeSet::new(), move |pos| {
            Ok((pos == far).then(|| chunk_world(pos)))
        })
        .unwrap();
        streamer.service(&mut session).unwrap();
        session.update(Duration::from_millis(50));
        assert!(
            !session
                .snapshot()
                .world
                .loaded()
                .unwrap()
                .chunks
                .contains_key(&far)
        );
        streamer.set_radius(2);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !session
            .snapshot()
            .world
            .loaded()
            .unwrap()
            .chunks
            .contains_key(&far)
        {
            streamer.service(&mut session).unwrap();
            session.update(Duration::from_millis(50));
            assert!(
                Instant::now() < deadline,
                "expanded window did not load the distant chunk"
            );
            thread::sleep(Duration::from_millis(2));
        }
        streamer.set_radius(1);
        streamer.service(&mut session).unwrap();
        session.update(Duration::from_millis(50));
        assert!(
            !session
                .snapshot()
                .world
                .loaded()
                .unwrap()
                .chunks
                .contains_key(&far)
        );
        streamer.set_radius(2);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !session
            .snapshot()
            .world
            .loaded()
            .unwrap()
            .chunks
            .contains_key(&far)
        {
            streamer.service(&mut session).unwrap();
            session.update(Duration::from_millis(50));
            assert!(
                Instant::now() < deadline,
                "expanded window did not reload the distant chunk"
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn settled_prefix_skips_completed_chunks_but_keeps_nearest_first_order() {
        let home = ChunkPos { x: 0, z: 0 };
        let player = PlayerState {
            position: Vec3::new(8.5, 1.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let mut session = LoadedSession::new(
            chunk_world(home),
            IVec3::ZERO,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        let (reads_tx, reads_rx) = mpsc::channel();
        let (permit_tx, permit_rx) = mpsc::channel();
        let mut streamer =
            ChunkStreamer::with_entity_source(6, BTreeSet::new(), move |pos, entities_only| {
                assert!(!entities_only);
                permit_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                reads_tx.send(pos).unwrap();
                Ok(Some(ChunkPayload {
                    world: Some(chunk_world(pos)),
                    entities: SavedEntityData::Empty,
                    poi: SavedPoiData::Missing,
                    heightmaps: None,
                }))
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while session.snapshot().world.loaded().unwrap().chunks.len() < 169 {
            streamer.service(&mut session).unwrap();
            permit_tx.send(()).unwrap();
            session.update(Duration::from_millis(50));
            assert!(
                Instant::now() < deadline,
                "streamer did not fill the window"
            );
            thread::sleep(Duration::from_millis(2));
        }
        let reads: Vec<_> = reads_rx.try_iter().collect();
        let expected: Vec<_> = targets(home, 6)
            .into_iter()
            .filter(|&pos| pos != home)
            .collect();
        assert_eq!(reads, expected, "request order changed");
        eprintln!(
            "skipped {} settled target checks",
            streamer.skipped_target_checks
        );
        assert!(
            streamer.skipped_target_checks > 1_000,
            "settled targets should not be rescanned on every response"
        );
        assert!(streamer.settled_prefix > 100);
    }

    #[test]
    fn delayed_neighbor_is_a_barrier_then_becomes_walkable() {
        let home = ChunkPos { x: 0, z: 0 };
        let neighbor = ChunkPos { x: 1, z: 0 };
        let player = PlayerState {
            position: Vec3::new(15.4, 1.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let mut session = LoadedSession::new(
            chunk_world(home),
            IVec3::ZERO,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let mut streamer = ChunkStreamer::with_source(1, BTreeSet::new(), move |pos| {
            if pos == neighbor {
                started_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                Ok(Some(chunk_world(pos)))
            } else {
                Ok(None)
            }
        })
        .unwrap();
        streamer.service(&mut session).unwrap();
        started_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        session.submit_command(PlayerCommand::Move {
            forward: 0.0,
            strafe: 1.0,
            jump: false,
        });
        for _ in 0..30 {
            streamer.service(&mut session).unwrap();
            session.update(Duration::from_millis(50));
        }
        assert!(session.snapshot().player.position.x <= 15.700_01);
        assert!(
            !session
                .snapshot()
                .world
                .loaded()
                .unwrap()
                .chunks
                .contains_key(&neighbor)
        );
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !session
            .snapshot()
            .world
            .loaded()
            .unwrap()
            .chunks
            .contains_key(&neighbor)
            && Instant::now() < deadline
        {
            streamer.service(&mut session).unwrap();
            session.update(Duration::from_millis(50));
            thread::sleep(Duration::from_millis(2));
        }
        assert!(
            session
                .snapshot()
                .world
                .loaded()
                .unwrap()
                .chunks
                .contains_key(&neighbor)
        );
        for _ in 0..15 {
            streamer.service(&mut session).unwrap();
            session.update(Duration::from_millis(50));
        }
        let snapshot = session.snapshot();
        assert!(snapshot.player.position.x + snapshot.origin.x as f32 > 16.0);
    }

    #[test]
    fn distant_respawn_waits_for_the_background_chunk_reader() {
        use native_persistence::world::{
            DimensionId, PlayerRespawnConfig, RespawnData, SavedWorldBorder,
        };
        use native_resources::{
            packs::{ResourceId, ResourceProvider},
            respawn_environment::RespawnEnvironmentRegistry,
            respawn_tags::RespawnBlockTags,
        };
        use native_session::loaded::SavedRespawnSupport;
        use native_world::loaded::BIOME_COUNT;

        struct Provider;
        impl ResourceProvider for Provider {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                Ok(match path {
                    "data/minecraft/dimension_type/overworld.json" => {
                        Some(br#"{"attributes":{}}"#.to_vec())
                    }
                    "data/minecraft/worldgen/biome/plains.json" => {
                        Some(br#"{"attributes":{}}"#.to_vec())
                    }
                    _ => None,
                })
            }
            fn read_data_stack(&mut self, _: &str) -> Result<Vec<Vec<u8>>> {
                Ok(vec![br#"{"values":[]}"#.to_vec()])
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                Ok(match folder {
                    "worldgen/biome" => vec![ResourceId::parse("minecraft:plains")?],
                    "timeline" => vec![],
                    _ => unreachable!(),
                })
            }
        }
        for (immediate, edge) in [(false, false), (true, false), (false, true), (true, true)] {
            let home = ChunkPos { x: 0, z: 0 };
            let remote = ChunkPos {
                x: 1_000_000,
                z: -1_000_000,
            };
            let local = if edge { 15 } else { 8 };
            let spawn = IVec3::new(remote.x * 16 + local, 1, remote.z * 16 + local);
            let mut distant = chunk_world(remote);
            let bed = distant.intern_state(BlockState {
                name: "minecraft:red_bed".into(),
                properties: BTreeMap::from([
                    ("facing".into(), "north".into()),
                    ("part".into(), "head".into()),
                    ("occupied".into(), "false".into()),
                ]),
            });
            distant
                .set_block_id(spawn.x, spawn.y, spawn.z, bed)
                .unwrap();
            let biome = distant.intern_biome("minecraft:plains".into());
            let chunk = distant.chunks.get_mut(&remote).unwrap();
            Arc::make_mut(Arc::make_mut(chunk).sections.get_mut(&0).unwrap()).biomes =
                Some(Box::new([biome; BIOME_COUNT]));
            let player = PlayerState {
                position: Vec3::new(8.5, 20.0, 8.5),
                yaw: 0.0,
                pitch: 0.0,
                vertical_velocity: 0.0,
                on_ground: false,
                pose: native_session::PlayerPose::Standing,
            };
            let mut session = LoadedSession::new(
                chunk_world(home),
                IVec3::ZERO,
                BuildBounds {
                    min_y: 0,
                    max_y: 32,
                },
                player,
            )
            .unwrap();
            let ids = [
                "air",
                "stone",
                "grass_block",
                "dirt",
                "stone_bricks",
                "red_bed",
            ]
            .into_iter()
            .map(|id| format!("minecraft:{id}"))
            .collect();
            session
                .set_saved_respawn_support(SavedRespawnSupport {
                    config: PlayerRespawnConfig {
                        data: RespawnData {
                            dimension: DimensionId::parse("overworld").unwrap(),
                            position: spawn.to_array(),
                            yaw: 0.0,
                            pitch: 0.0,
                        },
                        forced: false,
                    },
                    environment: Arc::new(
                        RespawnEnvironmentRegistry::load(&mut Provider, "minecraft:overworld")
                            .unwrap(),
                    ),
                    tags: Arc::new(RespawnBlockTags::load(&mut Provider, ids).unwrap()),
                    border: SavedWorldBorder::default(),
                    max_health: 20.0,
                })
                .unwrap();
            session.set_player_vitals(native_session::survival::PlayerVitals {
                health: 1.0,
                ..Default::default()
            });
            session.set_death_rules(native_session::DeathRules {
                immediate_respawn: immediate,
                ..Default::default()
            });
            let (started_tx, started_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let mut streamer = ChunkStreamer::with_source(1, BTreeSet::new(), move |pos| {
                if pos == remote {
                    started_tx.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    Ok(Some(distant.clone()))
                } else if within(pos, (remote, 1)) {
                    Ok(Some(chunk_world(pos)))
                } else {
                    Ok(None)
                }
            })
            .unwrap();
            for _ in 0..100 {
                streamer.service(&mut session).unwrap();
                session.update(Duration::from_millis(50));
                if session.snapshot().death.is_some() {
                    break;
                }
            }
            assert!(
                session.snapshot().death.is_some(),
                "player must die from the fall"
            );
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                streamer.service(&mut session).unwrap();
                match started_rx.try_recv() {
                    Ok(()) => break,
                    Err(mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    error => panic!("respawn chunk reader did not start: {error:?}"),
                }
            }
            assert!(!session.snapshot().death.unwrap().can_respawn);
            release_tx.send(()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while session
                .snapshot()
                .death
                .is_some_and(|death| !death.can_respawn)
                && Instant::now() < deadline
            {
                streamer.service(&mut session).unwrap();
                session.update(Duration::from_millis(50));
                thread::sleep(Duration::from_millis(2));
            }
            let before = session.snapshot();
            if !immediate {
                assert!(
                    before.death.unwrap().can_respawn,
                    "immediate={immediate} edge={edge}"
                );
                assert!(!before.world.loaded().unwrap().chunks.contains_key(&remote));
                session.submit_command(PlayerCommand::Respawn);
                session.update(Duration::from_millis(50));
            }
            let after = session.snapshot();
            assert!(after.death.is_none(), "immediate={immediate} edge={edge}");
            let absolute = after.player.position.as_dvec3() + after.origin.as_dvec3();
            assert!((absolute.x - f64::from(spawn.x)).abs() < 4.0);
            assert!((absolute.z - f64::from(spawn.z)).abs() < 4.0);
            assert!(after.world.loaded().unwrap().chunks.contains_key(&remote));
        }
    }

    #[test]
    fn distant_world_spawn_waits_for_streamed_heightmaps_and_entities() {
        use native_persistence::{
            nbt::{NamedTag, Tag},
            world::{DimensionId, LoadedChunk, RespawnData, SavedWorldBorder},
        };
        use native_resources::packs::ResourceProvider;
        use native_session::loaded::WorldRespawnSupport;

        struct Provider;
        impl ResourceProvider for Provider {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data_stack(&mut self, _: &str) -> Result<Vec<Vec<u8>>> {
                Ok(vec![br#"{"values":["minecraft:stone","minecraft:dirt","minecraft:grass_block","minecraft:stone_bricks"]}"#.to_vec()])
            }
        }
        for (immediate, edge, moving, outside) in [
            (false, false, false, false),
            (true, false, false, false),
            (false, true, false, false),
            (true, true, false, false),
            (false, false, true, false),
            (true, false, true, false),
            (false, true, true, false),
            (true, true, true, false),
            (false, false, true, true),
            (true, false, true, true),
        ] {
            let home = ChunkPos { x: 0, z: 0 };
            let remote = ChunkPos {
                x: 1_000_000,
                z: -1_000_000,
            };
            let local = if edge { 15 } else { 8 };
            let spawn = IVec3::new(
                remote.x * 16 + if outside { 31 } else { local },
                if moving { 31 } else { 5 },
                remote.z * 16 + local,
            );
            let player = PlayerState {
                position: Vec3::new(8.5, 20.0, 8.5),
                yaw: 0.0,
                pitch: 0.0,
                vertical_velocity: 0.0,
                on_ground: false,
                pose: native_session::PlayerPose::Standing,
            };
            let mut session = LoadedSession::new(
                chunk_world(home),
                IVec3::ZERO,
                BuildBounds {
                    min_y: 0,
                    max_y: 32,
                },
                player,
            )
            .unwrap();
            let ids = ["air", "stone", "dirt", "grass_block", "stone_bricks"]
                .into_iter()
                .map(|name| format!("minecraft:{name}"))
                .collect();
            let tags = Arc::new(HeightmapTags::load(&mut Provider, &ids).unwrap());
            session.set_spawn_heightmap_tags(Arc::clone(&tags)).unwrap();
            session.set_saved_respawn_absent().unwrap();
            session
                .set_world_respawn_support(WorldRespawnSupport {
                    spawn: RespawnData {
                        dimension: DimensionId::parse("overworld").unwrap(),
                        position: spawn.to_array(),
                        yaw: 25.0,
                        pitch: 10.0,
                    },
                    border: if outside {
                        SavedWorldBorder {
                            center_x: f64::from(remote.x * 16 + 8),
                            center_z: f64::from(remote.z * 16 + 8),
                            size: 16.0,
                            lerp_target: 8.0,
                            lerp_time: 100_000,
                            ..Default::default()
                        }
                    } else if moving {
                        SavedWorldBorder {
                            lerp_target: 59_999_000.0,
                            lerp_time: 100_000,
                            ..Default::default()
                        }
                    } else {
                        SavedWorldBorder::default()
                    },
                    max_health: 20.0,
                    radius: 0,
                    adventure: moving,
                    ceiling_spawn_height: None,
                })
                .unwrap();
            session.set_player_vitals(native_session::survival::PlayerVitals {
                health: 1.0,
                ..Default::default()
            });
            session.set_death_rules(native_session::DeathRules {
                immediate_respawn: immediate,
                ..Default::default()
            });
            let (started_tx, started_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let mut streamer =
                ChunkStreamer::with_entity_source(1, BTreeSet::new(), move |pos, _| {
                    if pos == remote {
                        started_tx.send(()).unwrap();
                        release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    }
                    if !within(pos, (remote, 1)) {
                        return Ok(None);
                    }
                    let world = chunk_world(pos);
                    let saved = LoadedChunk {
                        position: SavedChunkPos { x: pos.x, z: pos.z },
                        data_version: 5023,
                        status: Some("minecraft:full".into()),
                        document: Arc::new(NamedTag {
                            name: "".into(),
                            value: Tag::Compound(vec![]),
                        }),
                        sections: Vec::new(),
                    };
                    let maps = native_session::spawn_heightmaps::prepare(
                        &saved,
                        &world,
                        HeightBounds {
                            min_y: 0,
                            max_y: 32,
                        },
                        &tags,
                    )?;
                    Ok(Some(ChunkPayload {
                        world: Some(world),
                        entities: SavedEntityData::Empty,
                        poi: SavedPoiData::Missing,
                        heightmaps: Some(maps),
                    }))
                })
                .unwrap();
            for _ in 0..100 {
                streamer.service(&mut session).unwrap();
                session.update(Duration::from_millis(50));
                if session.snapshot().death.is_some() {
                    break;
                }
            }
            assert!(session.snapshot().death.is_some());
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                streamer.service(&mut session).unwrap();
                match started_rx.try_recv() {
                    Ok(()) => break,
                    Err(mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(2))
                    }
                    error => panic!("world spawn reader did not start: {error:?}"),
                }
            }
            assert!(!session.snapshot().death.unwrap().can_respawn);
            for _ in 0..12 {
                streamer.service(&mut session).unwrap();
                session.update(Duration::from_millis(50));
            }
            let waiting = session.snapshot().death.unwrap();
            assert!(!waiting.can_respawn);
            assert_eq!(waiting.respawn_pending, immediate);
            release_tx.send(()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while session
                .snapshot()
                .death
                .is_some_and(|death| !death.can_respawn)
                && Instant::now() < deadline
            {
                streamer.service(&mut session).unwrap();
                session.update(Duration::from_millis(50));
                thread::sleep(Duration::from_millis(2));
            }
            if !immediate {
                assert!(session.snapshot().death.unwrap().can_respawn, "edge={edge}");
                assert!(
                    !session
                        .snapshot()
                        .world
                        .loaded()
                        .unwrap()
                        .chunks
                        .contains_key(&remote)
                );
                session.submit_command(PlayerCommand::Respawn);
                session.update(Duration::from_millis(50));
                let deadline = Instant::now() + Duration::from_secs(5);
                while session.snapshot().death.is_some() && Instant::now() < deadline {
                    streamer.service(&mut session).unwrap();
                    session.update(Duration::from_millis(50));
                    thread::sleep(Duration::from_millis(2));
                }
            }
            let after = session.snapshot();
            assert!(
                after.death.is_none(),
                "immediate={immediate}, edge={edge}, moving={moving}, outside={outside}"
            );
            let absolute = after.player.position.as_dvec3() + after.origin.as_dvec3();
            assert_eq!(
                absolute,
                glam::DVec3::new(
                    f64::from(if outside { remote.x * 16 + 8 } else { spawn.x }) + 0.5,
                    1.0,
                    f64::from(spawn.z) + 0.5,
                )
            );
            assert!(after.world.loaded().unwrap().chunks.contains_key(&remote));
        }
    }

    #[test]
    fn targets_are_nearest_first_and_coordinate_overflow_is_excluded() {
        let center = ChunkPos { x: -1, z: 1 };
        let positions = targets(center, 1);
        assert_eq!(positions.len(), 9);
        assert_eq!(positions[0], center);
        assert!(positions.iter().all(|pos| within(*pos, (center, 1))));
        assert!(targets(ChunkPos { x: i32::MAX, z: 0 }, 1).is_empty());
    }

    #[test]
    fn reads_neighbor_from_anvil_without_modifying_world_files() {
        use flate2::{
            Compression,
            write::{GzEncoder, ZlibEncoder},
        };
        use native_persistence::{
            nbt::{self, NamedTag, Tag, TagType},
            world::DimensionId,
        };
        use std::io::Write;

        struct HeightTagsProvider;
        impl native_resources::packs::ResourceProvider for HeightTagsProvider {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data_stack(&mut self, _: &str) -> Result<Vec<Vec<u8>>> {
                Ok(vec![br#"{"values":["minecraft:stone","minecraft:dirt","minecraft:grass_block","minecraft:stone_bricks"]}"#.to_vec()])
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let level = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "Data".into(),
                Tag::Compound(vec![("DataVersion".into(), Tag::Int(5023))]),
            )]),
        };
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&nbt::write(&level).unwrap()).unwrap();
        let level_bytes = gzip.finish().unwrap();
        std::fs::write(temp.path().join("level.dat"), &level_bytes).unwrap();
        let mut region = vec![0u8; 4 * 4096];
        for x in 0..=1 {
            let (palette, floor_index, air_index) = if x == 0 {
                (vec!["minecraft:air", "minecraft:stone"], 1u64, 0u64)
            } else {
                (vec!["minecraft:stone", "minecraft:air"], 0u64, 1u64)
            };
            let words: Vec<i64> = (0..256)
                .map(|word| {
                    let value = if word < 16 { floor_index } else { air_index };
                    (0..16).fold(0u64, |bits, offset| bits | (value << (offset * 4))) as i64
                })
                .collect();
            let section = Tag::Compound(vec![
                ("Y".into(), Tag::Byte(0)),
                (
                    "block_states".into(),
                    Tag::Compound(vec![
                        (
                            "palette".into(),
                            Tag::List {
                                element_type: TagType::String,
                                values: palette.iter().map(|s| Tag::String((*s).into())).collect(),
                            },
                        ),
                        ("data".into(), Tag::LongArray(words)),
                    ]),
                ),
            ]);
            let chunk = NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![
                    ("DataVersion".into(), Tag::Int(5023)),
                    ("xPos".into(), Tag::Int(x)),
                    ("zPos".into(), Tag::Int(0)),
                    ("Status".into(), Tag::String("minecraft:full".into())),
                    (
                        "sections".into(),
                        Tag::List {
                            element_type: TagType::Compound,
                            values: vec![section],
                        },
                    ),
                ]),
            };
            let mut zlib = ZlibEncoder::new(Vec::new(), Compression::default());
            zlib.write_all(&nbt::write(&chunk).unwrap()).unwrap();
            let compressed = zlib.finish().unwrap();
            assert!(compressed.len() + 5 <= 4096);
            let sector = x as usize + 2;
            region[x as usize * 4..x as usize * 4 + 4]
                .copy_from_slice(&(((sector as u32) << 8) | 1).to_be_bytes());
            let base = sector * 4096;
            region[base..base + 4].copy_from_slice(&((compressed.len() + 1) as u32).to_be_bytes());
            region[base + 4] = 2;
            region[base + 5..base + 5 + compressed.len()].copy_from_slice(&compressed);
        }
        let region_dir = temp.path().join("dimensions/minecraft/overworld/region");
        std::fs::create_dir_all(&region_dir).unwrap();
        let region_path = region_dir.join("r.0.0.mca");
        std::fs::write(&region_path, &region).unwrap();
        let entity_document = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("Position".into(), Tag::IntArray(vec![1, 0])),
                (
                    "Entities".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![Tag::Compound(vec![
                            ("id".into(), Tag::String("minecraft:pig".into())),
                            ("unknown".into(), Tag::Int(42)),
                        ])],
                    },
                ),
            ]),
        };
        let mut zlib = ZlibEncoder::new(Vec::new(), Compression::default());
        zlib.write_all(&nbt::write(&entity_document).unwrap())
            .unwrap();
        let compressed = zlib.finish().unwrap();
        assert!(compressed.len() + 5 <= 4096);
        let mut entity_region = vec![0u8; 3 * 4096];
        entity_region[4..8].copy_from_slice(&((2u32 << 8) | 1).to_be_bytes());
        entity_region[8192..8196].copy_from_slice(&((compressed.len() + 1) as u32).to_be_bytes());
        entity_region[8196] = 2;
        entity_region[8197..8197 + compressed.len()].copy_from_slice(&compressed);
        let entity_dir = temp.path().join("dimensions/minecraft/overworld/entities");
        std::fs::create_dir_all(&entity_dir).unwrap();
        let entity_path = entity_dir.join("r.0.0.mca");
        std::fs::write(&entity_path, &entity_region).unwrap();
        let poi_document = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                (
                    "Sections".into(),
                    Tag::Compound(vec![(
                        "0".into(),
                        Tag::Compound(vec![
                            ("Valid".into(), Tag::Byte(1)),
                            (
                                "Records".into(),
                                Tag::List {
                                    element_type: TagType::Compound,
                                    values: vec![Tag::Compound(vec![
                                        ("pos".into(), Tag::IntArray(vec![16, 1, 0])),
                                        ("type".into(), Tag::String("minecraft:home".into())),
                                        ("free_tickets".into(), Tag::Int(0)),
                                    ])],
                                },
                            ),
                        ]),
                    )]),
                ),
            ]),
        };
        let mut zlib = ZlibEncoder::new(Vec::new(), Compression::default());
        zlib.write_all(&nbt::write(&poi_document).unwrap()).unwrap();
        let compressed_poi = zlib.finish().unwrap();
        let mut poi_region = vec![0u8; 3 * 4096];
        poi_region[4..8].copy_from_slice(&((2u32 << 8) | 1).to_be_bytes());
        poi_region[8192..8196].copy_from_slice(&((compressed_poi.len() + 1) as u32).to_be_bytes());
        poi_region[8196] = 2;
        poi_region[8197..8197 + compressed_poi.len()].copy_from_slice(&compressed_poi);
        let poi_dir = temp.path().join("dimensions/minecraft/overworld/poi");
        std::fs::create_dir_all(&poi_dir).unwrap();
        let poi_path = poi_dir.join("r.0.0.mca");
        std::fs::write(&poi_path, &poi_region).unwrap();
        let dimension = DimensionId::parse("minecraft:overworld").unwrap();
        let mut reader = WorldReader::open(temp.path(), dimension).unwrap();
        let area = reader.read_area(SavedChunkPos { x: 0, z: 0 }, 0).unwrap();
        let world = area.materialize(reader.dimension()).unwrap();
        let player = PlayerState {
            position: Vec3::new(15.4, 1.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let mut session = LoadedSession::new(
            world,
            IVec3::ZERO,
            BuildBounds {
                min_y: 0,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        let registry = ["air", "stone", "dirt", "grass_block", "stone_bricks"]
            .into_iter()
            .map(|name| format!("minecraft:{name}"))
            .collect();
        let tags = Arc::new(HeightmapTags::load(&mut HeightTagsProvider, &registry).unwrap());
        session.set_spawn_heightmap_tags(Arc::clone(&tags)).unwrap();
        session
            .prepare_initial_spawn_heightmaps(&area.chunks[&SavedChunkPos { x: 0, z: 0 }])
            .unwrap();
        assert_eq!(session.spawn_column_heights(0, 0).unwrap().world_surface, 0);
        assert!(session.spawn_column_heights(16, 0).is_err());
        let mut streamer = ChunkStreamer::start(
            reader,
            BTreeMap::new(),
            0,
            BTreeSet::new(),
            Some((
                HeightBounds {
                    min_y: 0,
                    max_y: 32,
                },
                tags,
            )),
        )
        .unwrap();
        session.submit_command(PlayerCommand::Move {
            forward: 0.0,
            strafe: 1.0,
            jump: false,
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while {
            let snapshot = session.snapshot();
            snapshot.player.position.x + snapshot.origin.x as f32 <= 16.0
                && Instant::now() < deadline
        } {
            streamer.service(&mut session).unwrap();
            session.update(Duration::from_millis(50));
            thread::sleep(Duration::from_millis(2));
        }
        let snapshot = session.snapshot();
        assert!(snapshot.player.position.x + snapshot.origin.x as f32 > 16.0);
        assert!(
            snapshot
                .world
                .loaded()
                .unwrap()
                .chunks
                .contains_key(&ChunkPos { x: 1, z: 0 })
        );
        let Some(SavedEntityData::Present(entities)) =
            session.saved_entity_chunk(ChunkPos { x: 1, z: 0 })
        else {
            panic!("streamed entity data was not preserved");
        };
        assert_eq!(*entities.document, entity_document);
        let Some(SavedPoiData::Present(poi)) = session.saved_poi_chunk(ChunkPos { x: 1, z: 0 })
        else {
            panic!("streamed POI data was not preserved")
        };
        assert_eq!(*poi.document, poi_document);
        assert_eq!(
            session.spawn_column_heights(16, 0).unwrap(),
            native_session::world_spawn::SpawnColumnHeights {
                motion_blocking: 0,
                world_surface: 0,
                ocean_floor: 0,
            }
        );
        assert_eq!(
            std::fs::read(temp.path().join("level.dat")).unwrap(),
            level_bytes
        );
        assert_eq!(std::fs::read(region_path).unwrap(), region);
        assert_eq!(std::fs::read(entity_path).unwrap(), entity_region);
        assert_eq!(std::fs::read(poi_path).unwrap(), poi_region);

        // A missing region really is empty, but a truncated region must not
        // become a false negative for future entity collision checks.
        let bad_path = entity_dir.join("r.1.0.mca");
        std::fs::write(&bad_path, [1, 2, 3]).unwrap();
        let mut reader = WorldReader::open(
            temp.path(),
            DimensionId::parse("minecraft:overworld").unwrap(),
        )
        .unwrap();
        assert!(matches!(
            read_entity_data(&mut reader, ChunkPos { x: 64, z: 0 }),
            SavedEntityData::Empty
        ));
        assert!(matches!(
            read_entity_data(&mut reader, ChunkPos { x: 32, z: 0 }),
            SavedEntityData::Unavailable
        ));
        assert!(reader.read_chunk(1, 0).unwrap().is_some());
        assert_eq!(std::fs::read(bad_path).unwrap(), [1, 2, 3]);
        let bad_poi = poi_dir.join("r.1.0.mca");
        std::fs::write(&bad_poi, [1, 2, 3]).unwrap();
        assert!(matches!(
            read_poi_data(&mut reader, ChunkPos { x: 64, z: 0 }),
            SavedPoiData::Missing
        ));
        assert!(matches!(
            read_poi_data(&mut reader, ChunkPos { x: 32, z: 0 }),
            SavedPoiData::Unavailable
        ));
        assert_eq!(std::fs::read(bad_poi).unwrap(), [1, 2, 3]);
    }
}
