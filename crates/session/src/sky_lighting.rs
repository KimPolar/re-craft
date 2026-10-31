//! Background sky-light work over immutable session snapshots.
use native_world::{
    block_light::LightBounds,
    loaded::{BlockPosition, BlockStateId, Chunk, ChunkPos, Section, World},
    sky_light::{SkyColumn, SkySourceScanner, sky_change_for_edit},
    sky_propagation::{SkyChange, relight_changes},
    sky_storage::restore_missing_for_changes,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        mpsc::{self, Receiver, TryRecvError},
    },
};

type SkySources = BTreeMap<(i32, i32), SkyColumn>;
type AppliedLighting = (
    BTreeSet<ChunkPos>,
    SkySources,
    BTreeMap<ChunkPos, Arc<Chunk>>,
);

#[derive(Clone, Copy)]
struct Edit {
    position: BlockPosition,
    old: Option<BlockStateId>,
    new: BlockStateId,
}

struct Computed {
    world: World,
    changed: BTreeSet<ChunkPos>,
    sources: SkySources,
}

struct Reply {
    generation: u64,
    result: Option<Computed>,
}

#[derive(Default)]
pub(crate) struct SkyLighting {
    enabled: bool,
    configured: bool,
    generation: u64,
    failed_generation: Option<u64>,
    retry_delay: u8,
    worker: Option<(u64, Receiver<Reply>)>,
    pending: Vec<Edit>,
    // The original section contents are needed to reproduce Java's initial
    // source scan for columns first encountered after another column changed.
    originals: BTreeMap<ChunkPos, Arc<Chunk>>,
    sources: SkySources,
    applied_originals: BTreeMap<ChunkPos, Arc<Chunk>>,
}

impl SkyLighting {
    pub(crate) fn capability(&self) -> Option<bool> {
        self.configured.then_some(self.enabled)
    }
    pub(crate) fn is_current(&self) -> bool {
        self.enabled && self.pending.is_empty()
    }

    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.configured = true;
        self.invalidate();
    }

    pub(crate) fn invalidate(&mut self) {
        if self.enabled {
            self.generation = self.generation.wrapping_add(1);
        }
    }

    pub(crate) fn streaming_changed(&mut self) {
        self.invalidate();
        self.retry_delay = 0;
    }

    pub(crate) fn record(
        &mut self,
        position: BlockPosition,
        old: Option<BlockStateId>,
        new: BlockStateId,
        original: &Arc<Chunk>,
    ) {
        if !self.enabled {
            return;
        }
        self.originals
            .entry(position.chunk())
            .or_insert_with(|| Arc::clone(original));
        self.pending.push(Edit { position, old, new });
        self.invalidate();
    }

    /// An isolated edit with unchanged sky behavior can often update its
    /// already-known source immediately without launching a worker. Still
    /// retain it if Java's boundary rules actually changed the source.
    pub(crate) fn discard_trivial(&mut self, world: &World, position: BlockPosition) {
        if self.worker.is_some() || self.pending.len() != 1 {
            return;
        }
        let edit = self.pending[0];
        if edit.position != position {
            return;
        }
        let Some(new) = world.state(edit.new) else {
            return;
        };
        if sky_change_for_edit(edit.old.and_then(|old| world.state(old)), new) {
            return;
        }
        let Some(mut column) = self.sources.get(&(position.x, position.z)).copied() else {
            return;
        };
        if column.update(world, position.y) == Some(false) {
            self.sources.insert((position.x, position.z), column);
            self.pending.clear();
        }
    }

    /// Poll results and start at most one background job. Any block or stream
    /// change while it runs invalidates the result; the edits stay pending.
    pub(crate) fn advance(
        &mut self,
        world: &mut Arc<World>,
        bounds: Option<LightBounds>,
    ) -> BTreeSet<ChunkPos> {
        let mut changed = BTreeSet::new();
        self.retry_delay = self.retry_delay.saturating_sub(1);
        if let Some((worker_generation, receiver)) = &self.worker {
            let worker_generation = *worker_generation;
            match receiver.try_recv() {
                Ok(reply) => {
                    self.worker = None;
                    if self.enabled && reply.generation == self.generation {
                        match reply.result.and_then(|result| apply(world, result)) {
                            Some((updated, sources, originals)) => {
                                self.pending.clear();
                                self.sources = sources;
                                self.failed_generation = None;
                                self.retry_delay = 0;
                                self.applied_originals.extend(originals);
                                changed = updated;
                            }
                            None => {
                                self.failed_generation = Some(self.generation);
                                self.retry_delay = 20;
                            }
                        }
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    self.worker = None;
                    if worker_generation == self.generation {
                        self.failed_generation = Some(self.generation);
                        self.retry_delay = 20;
                    }
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if !self.enabled
            || self.pending.is_empty()
            || self.worker.is_some()
            || self.failed_generation == Some(self.generation)
            || self.retry_delay != 0
        {
            return changed;
        }
        let Some(bounds) = bounds else {
            self.failed_generation = Some(self.generation);
            self.retry_delay = 20;
            return changed;
        };
        let generation = self.generation;
        let snapshot = Arc::clone(world);
        let edits = self.pending.clone();
        let originals = self.originals.clone();
        let sources = self.sources.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        let spawned = std::thread::Builder::new()
            .name("sky-light".into())
            .spawn(move || {
                let result = compute(&snapshot, bounds, &edits, &originals, sources);
                let _ = tx.send(Reply { generation, result });
            });
        if spawned.is_ok() {
            self.worker = Some((generation, rx));
        } else {
            self.failed_generation = Some(generation);
            self.retry_delay = 20;
        }
        changed
    }

    pub(crate) fn settled(&self) -> bool {
        self.pending.is_empty() && self.worker.is_none()
    }

    pub(crate) fn take_applied_originals(&mut self) -> BTreeMap<ChunkPos, Arc<Chunk>> {
        std::mem::take(&mut self.applied_originals)
    }

    #[cfg(test)]
    pub(crate) fn wait_for_worker(&mut self) {
        let (generation, receiver) = self.worker.take().expect("worker started");
        let reply = receiver
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("worker completed");
        let (tx, rx) = mpsc::sync_channel(1);
        assert!(tx.send(reply).is_ok());
        self.worker = Some((generation, rx));
    }
}

fn source_for(
    sources: &mut SkySources,
    scanner: &mut SkySourceScanner<'_>,
    x: i32,
    z: i32,
) -> Option<SkyColumn> {
    if let Some(column) = sources.get(&(x, z)) {
        return Some(*column);
    }
    let column = scanner.load(x, z)?;
    sources.insert((x, z), column);
    Some(column)
}

fn compute(
    snapshot: &World,
    bounds: LightBounds,
    edits: &[Edit],
    originals: &BTreeMap<ChunkPos, Arc<Chunk>>,
    mut sources: SkySources,
) -> Option<Computed> {
    let edited_columns: BTreeSet<_> = sources
        .keys()
        .copied()
        .chain(edits.iter().map(|edit| (edit.position.x, edit.position.z)))
        .collect();
    let mut work = snapshot.clone();
    // Reverse the pending edits to restore the world corresponding to the
    // stored sky arrays. Unknown original block data is never guessed.
    for edit in edits.iter().rev() {
        let old = edit.old?;
        let p = edit.position;
        if work.block_id(p.x, p.y, p.z)? != edit.new || work.state(old).is_none() {
            return None;
        }
        work.set_block_id(p.x, p.y, p.z, old).ok()?;
    }
    let mut baseline = work.clone();
    let mut baseline_sources = sources.clone();
    let mut historical = work.clone();
    for (position, original) in originals {
        if let Some(chunk) = historical.chunks.get_mut(position) {
            *chunk = Arc::clone(original);
        }
    }
    let mut scanner = SkySourceScanner::new(&historical, bounds);
    let mut changes = Vec::new();
    for edit in edits {
        let p = edit.position;
        let old = edit.old?;
        if work.block_id(p.x, p.y, p.z)? != old {
            return None;
        }
        let affects_sky = sky_change_for_edit(work.state(old), work.state(edit.new)?);
        let mut column = source_for(&mut sources, &mut scanner, p.x, p.z)?;
        let previous_source = column.lowest_source_y();
        work.set_block_id(p.x, p.y, p.z, edit.new).ok()?;
        column.update(&work, p.y)?;
        let current_source = column.lowest_source_y();
        sources.insert((p.x, p.z), column);
        if previous_source != current_source || affects_sky {
            changes.push(SkyChange {
                position: p,
                previous_source,
                current_source,
            });
        }
    }
    let restored =
        restore_missing_for_changes(&mut baseline, &historical, bounds, &changes, |x, z| {
            source_for(&mut baseline_sources, &mut scanner, x, z).map(SkyColumn::lowest_source_y)
        });
    for pos in &restored {
        let original = baseline.chunks.get(pos)?;
        let target = Arc::make_mut(work.chunks.get_mut(pos)?);
        for (&y, section) in &original.sections {
            let Some(light) = section.sky_light.as_ref() else {
                continue;
            };
            let target = target.sections.entry(y).or_insert_with(|| {
                Arc::new(Section {
                    blocks: None,
                    biomes: None,
                    block_light: None,
                    sky_light: None,
                })
            });
            if target.sky_light.is_none() {
                Arc::make_mut(target).sky_light = Some(light.clone());
            }
        }
    }
    let mut changed = relight_changes(&mut work, bounds, &changes, |x, z| {
        source_for(&mut sources, &mut scanner, x, z).map(SkyColumn::lowest_source_y)
    })?;
    changed.extend(restored);
    sources.retain(|position, _| edited_columns.contains(position));
    Some(Computed {
        world: work,
        changed,
        sources,
    })
}

fn apply(world: &mut Arc<World>, computed: Computed) -> Option<AppliedLighting> {
    let mut updates = Vec::new();
    let mut originals = BTreeMap::new();
    for pos in &computed.changed {
        let current = world.chunks.get(pos)?;
        originals.insert(*pos, Arc::clone(current));
        let calculated = computed.world.chunks.get(pos)?;
        for (&y, section) in &calculated.sections {
            match current.sections.get(&y) {
                Some(destination) if destination.sky_light != section.sky_light => {
                    updates.push((*pos, y, section.sky_light.clone()))
                }
                Some(_) => {}
                None if section.blocks.is_none()
                    && section.biomes.is_none()
                    && section.block_light.is_none()
                    && section.sky_light.is_some() =>
                {
                    updates.push((*pos, y, section.sky_light.clone()))
                }
                None => return None,
            }
        }
    }
    let world = Arc::make_mut(world);
    let mut changed = BTreeSet::new();
    for (pos, y, light) in updates {
        let chunk = Arc::make_mut(world.chunks.get_mut(&pos)?);
        let section = chunk.sections.entry(y).or_insert_with(|| {
            Arc::new(Section {
                blocks: None,
                biomes: None,
                block_light: None,
                sky_light: None,
            })
        });
        let section = Arc::make_mut(section);
        section.sky_light = light;
        changed.insert(pos);
    }
    Some((changed, computed.sources, originals))
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_world::loaded::{BLOCK_COUNT, BlockState, LIGHT_BYTES, Section};

    fn state(name: &str, properties: &[(&str, &str)]) -> BlockState {
        BlockState {
            name: format!("minecraft:{name}"),
            properties: properties
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
        }
    }
    fn fixture() -> Arc<World> {
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(state("air", &[]));
        for x in -1..=2 {
            for z in -1..=2 {
                let pos = ChunkPos { x, z };
                let sections = (-1..=2)
                    .map(|y| {
                        (
                            y,
                            Arc::new(Section {
                                blocks: (0..=1).contains(&y).then(|| Box::new([air; BLOCK_COUNT])),
                                biomes: None,
                                block_light: Some(Box::new([0; LIGHT_BYTES])),
                                sky_light: Some(Box::new([0xff; LIGHT_BYTES])),
                            }),
                        )
                    })
                    .collect();
                world.chunks.insert(
                    pos,
                    Arc::new(Chunk {
                        position: pos,
                        status: Some("minecraft:full".into()),
                        block_light_correct: true,
                        sections,
                    }),
                );
            }
        }
        Arc::new(world)
    }
    fn edit(sky: &mut SkyLighting, world: &mut Arc<World>, p: BlockPosition, new: BlockState) {
        let old = world.block_id(p.x, p.y, p.z);
        let id = Arc::make_mut(world).intern_state(new);
        let original = Arc::clone(world.chunks.get(&p.chunk()).unwrap());
        sky.record(p, old, id, &original);
        let world = Arc::make_mut(world);
        world.set_block_id(p.x, p.y, p.z, id).unwrap();
        sky.discard_trivial(world, p);
    }
    fn finish(
        sky: &mut SkyLighting,
        world: &mut Arc<World>,
        bounds: LightBounds,
    ) -> BTreeSet<ChunkPos> {
        if sky.worker.is_none() {
            sky.advance(world, Some(bounds));
        }
        sky.wait_for_worker();
        sky.advance(world, Some(bounds))
    }
    #[test]
    fn worker_applies_sky_to_current_world_and_preserves_snapshot() {
        let bounds = LightBounds::new(0, 32).unwrap();
        let mut world = fixture();
        let mut sky = SkyLighting::default();
        sky.set_enabled(true);
        edit(
            &mut sky,
            &mut world,
            BlockPosition {
                x: 15,
                y: 15,
                z: 15,
            },
            state("stone", &[]),
        );
        let before = Arc::clone(&world);
        let changed = finish(&mut sky, &mut world, bounds);
        assert!(sky.pending.is_empty());
        assert!(sky.settled());
        let originals = sky.take_applied_originals();
        assert!(changed.iter().all(|pos| originals.contains_key(pos)));
        assert_eq!(
            originals[&ChunkPos { x: 0, z: 0 }].sections[&0]
                .sky_light
                .as_ref()
                .unwrap()[0],
            0xff
        );
        assert!(sky.take_applied_originals().is_empty());
        assert!(changed.contains(&ChunkPos { x: 0, z: 0 }));
        assert_eq!(world.light(15, 14, 15, true), Some(14));
        assert_eq!(world.light(15, 14, 15, false), Some(0));
        assert_eq!(before.light(15, 14, 15, true), Some(15));
    }
    #[test]
    fn worker_restores_proven_absent_padding_before_relighting() {
        let bounds = LightBounds::new(0, 32).unwrap();
        let mut world = fixture();
        for x in 0..=1 {
            for z in 0..=1 {
                let chunk = Arc::make_mut(
                    Arc::make_mut(&mut world)
                        .chunks
                        .get_mut(&ChunkPos { x, z })
                        .unwrap(),
                );
                chunk.sections.remove(&-1);
                chunk.sections.remove(&2);
            }
        }
        let mut sky = SkyLighting::default();
        sky.set_enabled(true);
        edit(
            &mut sky,
            &mut world,
            BlockPosition {
                x: 15,
                y: 31,
                z: 15,
            },
            state("stone", &[]),
        );
        let before = Arc::clone(&world);
        let changed = finish(&mut sky, &mut world, bounds);
        assert!(!changed.is_empty());
        assert!(sky.pending.is_empty());
        assert_eq!(before.light(15, -1, 15, true), None);
        assert_eq!(world.light(15, -1, 15, true), Some(14));
        assert_eq!(world.light(15, 32, 15, true), Some(15));
    }
    #[test]
    fn skyless_dimensions_do_not_enqueue_work() {
        let bounds = LightBounds::new(0, 32).unwrap();
        let mut world = fixture();
        let mut sky = SkyLighting::default();
        sky.set_enabled(false);
        edit(
            &mut sky,
            &mut world,
            BlockPosition {
                x: 15,
                y: 15,
                z: 15,
            },
            state("stone", &[]),
        );
        assert!(sky.advance(&mut world, Some(bounds)).is_empty());
        assert!(sky.worker.is_none());
        assert!(sky.pending.is_empty());
        assert_eq!(world.light(15, 14, 15, true), Some(15));
    }
    #[test]
    fn known_sky_neutral_edit_does_not_start_a_worker() {
        let bounds = LightBounds::new(0, 32).unwrap();
        let mut world = fixture();
        let mut sky = SkyLighting::default();
        sky.set_enabled(true);
        let p = BlockPosition {
            x: 15,
            y: 15,
            z: 15,
        };
        edit(&mut sky, &mut world, p, state("torch", &[]));
        finish(&mut sky, &mut world, bounds);
        assert!(sky.pending.is_empty());
        edit(&mut sky, &mut world, p, state("air", &[]));
        assert!(sky.pending.is_empty());
        sky.advance(&mut world, Some(bounds));
        assert!(sky.worker.is_none());
    }
    #[test]
    fn sky_neutral_edit_keeps_work_when_java_source_boundary_changes() {
        let bounds = LightBounds::new(0, 32).unwrap();
        let mut world = fixture();
        let slab = Arc::make_mut(&mut world).intern_state(state(
            "stone_slab",
            &[("type", "bottom"), ("waterlogged", "false")],
        ));
        Arc::make_mut(&mut world)
            .set_block_id(15, 0, 15, slab)
            .unwrap();
        let mut sky = SkyLighting::default();
        sky.set_enabled(true);
        sky.sources
            .insert((15, 15), SkyColumn::load(&world, bounds, 15, 15).unwrap());
        edit(
            &mut sky,
            &mut world,
            BlockPosition { x: 15, y: 0, z: 15 },
            state("oak_slab", &[("type", "bottom"), ("waterlogged", "false")]),
        );
        assert_eq!(sky.pending.len(), 1);
    }
    #[test]
    fn stale_worker_result_is_discarded_and_both_edits_are_retried() {
        let bounds = LightBounds::new(0, 32).unwrap();
        let mut world = fixture();
        let mut sky = SkyLighting::default();
        sky.set_enabled(true);
        edit(
            &mut sky,
            &mut world,
            BlockPosition {
                x: 14,
                y: 15,
                z: 15,
            },
            state("stone", &[]),
        );
        sky.advance(&mut world, Some(bounds));
        assert!(sky.worker.is_some());
        edit(
            &mut sky,
            &mut world,
            BlockPosition {
                x: 16,
                y: 15,
                z: 15,
            },
            state("stone", &[]),
        );
        assert!(finish(&mut sky, &mut world, bounds).is_empty());
        assert_eq!(sky.pending.len(), 2);
        assert_eq!(world.light(14, 14, 15, true), Some(15));
        let changed = finish(&mut sky, &mut world, bounds);
        assert!(!changed.is_empty());
        assert!(sky.pending.is_empty());
        assert_eq!(world.light(14, 14, 15, true), Some(14));
        assert_eq!(world.light(16, 14, 15, true), Some(14));
    }
    #[test]
    fn missing_light_waits_until_data_changes() {
        let bounds = LightBounds::new(0, 32).unwrap();
        let mut world = fixture();
        let mut sky = SkyLighting::default();
        sky.set_enabled(true);
        let chunk = Arc::make_mut(
            Arc::make_mut(&mut world)
                .chunks
                .get_mut(&ChunkPos { x: 0, z: 0 })
                .unwrap(),
        );
        chunk.block_light_correct = false;
        Arc::make_mut(chunk.sections.get_mut(&0).unwrap()).sky_light = None;
        edit(
            &mut sky,
            &mut world,
            BlockPosition {
                x: 15,
                y: 15,
                z: 15,
            },
            state("stone", &[]),
        );
        assert!(finish(&mut sky, &mut world, bounds).is_empty());
        assert_eq!(sky.pending.len(), 1);
        sky.advance(&mut world, Some(bounds));
        assert!(sky.worker.is_none());
        edit(
            &mut sky,
            &mut world,
            BlockPosition {
                x: 16,
                y: 15,
                z: 15,
            },
            state("stone", &[]),
        );
        sky.advance(&mut world, Some(bounds));
        assert!(
            sky.worker.is_none(),
            "new edits do not retry failed work every tick"
        );
        let chunk = Arc::make_mut(
            Arc::make_mut(&mut world)
                .chunks
                .get_mut(&ChunkPos { x: 0, z: 0 })
                .unwrap(),
        );
        chunk.block_light_correct = true;
        sky.streaming_changed();
        assert!(!finish(&mut sky, &mut world, bounds).is_empty());
        assert_eq!(world.light(15, 14, 15, true), Some(14));
        assert_eq!(world.light(16, 14, 15, true), Some(14));
    }
    #[test]
    fn later_column_uses_original_section_air_status() {
        let bounds = LightBounds::new(0, 32).unwrap();
        let mut world = fixture();
        let slab = Arc::make_mut(&mut world).intern_state(state(
            "stone_slab",
            &[("type", "bottom"), ("waterlogged", "false")],
        ));
        Arc::make_mut(&mut world)
            .set_block_id(15, 16, 15, slab)
            .unwrap();
        let mut sky = SkyLighting::default();
        sky.set_enabled(true);
        edit(
            &mut sky,
            &mut world,
            BlockPosition { x: 14, y: 0, z: 15 },
            state("torch", &[]),
        );
        finish(&mut sky, &mut world, bounds);
        assert!(sky.pending.is_empty());
        edit(
            &mut sky,
            &mut world,
            BlockPosition { x: 15, y: 8, z: 15 },
            state("stone", &[]),
        );
        finish(&mut sky, &mut world, bounds);
        assert!(sky.pending.is_empty());
        assert_eq!(sky.sources[&(15, 15)].lowest_source_y(), 9);
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn background_session_changes_match_java_sky_engine() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft")
            });
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("SkyLightEngineOracle.java");
        std::fs::write(
            &source,
            include_str!("../../world/tests/java/SkyLightEngineOracle.java"),
        )
        .unwrap();
        let run = |cmd: &mut Command| {
            let out = cmd.output().unwrap();
            assert!(
                out.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-encoding")
            .arg("UTF-8")
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let mut actions = Vec::new();
        for x in 13..=17 {
            for z in 13..=17 {
                actions.push((x, 24, z, "stone", true));
            }
        }
        actions.last_mut().unwrap().4 = false;
        actions.extend([
            (14, 24, 15, "air", true),
            (16, 24, 15, "air", false),
            (14, 24, 15, "water", true),
            (16, 24, 15, "slab_bottom", false),
            (15, 24, 15, "air", true),
            (15, 24, 15, "stone", true),
            (15, 24, 15, "air", false),
            (14, 24, 15, "air", true),
            (16, 24, 15, "air", false),
        ]);
        let entries: Vec<_> = actions.iter().map(|(x,y,z,name,defer)| serde_json::json!({"x":x,"y":y,"z":z,"state":name,"defer":defer})).collect();
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(&input, serde_json::to_vec(&entries).unwrap()).unwrap();
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("SkyLightEngineOracle")
            .arg(&input)
            .arg(&output));
        let expected: Vec<Vec<u8>> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(
            expected.len(),
            actions.iter().filter(|action| !action.4).count()
        );
        let mut expected = expected.into_iter();
        let mut world = fixture();
        let mut sky = SkyLighting::default();
        sky.set_enabled(true);
        let bounds = LightBounds::new(0, 32).unwrap();
        for (x, y, z, name, defer) in actions {
            let block = match name {
                "water" => state("water", &[("level", "0")]),
                "slab_bottom" => state(
                    "stone_slab",
                    &[("type", "bottom"), ("waterlogged", "false")],
                ),
                _ => state(name, &[]),
            };
            edit(&mut sky, &mut world, BlockPosition { x, y, z }, block);
            if defer {
                continue;
            }
            finish(&mut sky, &mut world, bounds);
            assert!(sky.pending.is_empty());
            let expected = expected.next().unwrap();
            for y in -15..47 {
                for z in 0..31 {
                    for x in 0..31 {
                        let index = ((y + 15) * 31 * 31 + z * 31 + x) as usize;
                        assert_eq!(
                            world.light(x, y, z, true).unwrap(),
                            expected[index],
                            "after {name}, at {x},{y},{z}"
                        );
                    }
                }
            }
        }
        assert!(expected.next().is_none());
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn worker_restores_java_omitted_layers_before_edit() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft")
            });
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("SkyStorageOracle.java");
        std::fs::write(
            &source,
            include_str!("../../world/tests/java/SkyStorageOracle.java"),
        )
        .unwrap();
        let run = |cmd: &mut Command| {
            let out = cmd.output().unwrap();
            assert!(
                out.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-encoding")
            .arg("UTF-8")
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let output = temp.path().join("storage.json");
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("SkyStorageOracle")
            .arg(&output));
        let cases: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(cases.len(), 2);
        for (roof, case) in [false, true].into_iter().zip(cases) {
            let mut world = World::new("minecraft:overworld");
            let air = world.intern_state(state("air", &[]));
            let stone = world.intern_state(state("stone", &[]));
            for x in -2..=2 {
                for z in -2..=2 {
                    let pos = ChunkPos { x, z };
                    let sections = (0..=3)
                        .map(|y| {
                            let mut blocks = Box::new([air; BLOCK_COUNT]);
                            if roof && y == 1 && (-1..=1).contains(&x) && (-1..=1).contains(&z) {
                                for x in 0..16 {
                                    for z in 0..16 {
                                        blocks[x + z * 16 + 15 * 256] = stone;
                                    }
                                }
                            }
                            (
                                y,
                                Arc::new(Section {
                                    blocks: Some(blocks),
                                    biomes: None,
                                    block_light: None,
                                    sky_light: None,
                                }),
                            )
                        })
                        .collect();
                    world.chunks.insert(
                        pos,
                        Arc::new(Chunk {
                            position: pos,
                            status: Some("minecraft:full".into()),
                            block_light_correct: true,
                            sections,
                        }),
                    );
                }
            }
            for row in case["layers"].as_array().unwrap() {
                let pos = ChunkPos {
                    x: row["x"].as_i64().unwrap() as i32,
                    z: row["z"].as_i64().unwrap() as i32,
                };
                let sy = row["y"].as_i64().unwrap() as i8;
                let Some(values) = row["saved"].as_array() else {
                    continue;
                };
                let bytes: Vec<u8> = values.iter().map(|v| v.as_u64().unwrap() as u8).collect();
                let data: Box<[u8; LIGHT_BYTES]> = bytes.into_boxed_slice().try_into().unwrap();
                let chunk = Arc::make_mut(world.chunks.get_mut(&pos).unwrap());
                let section = chunk.sections.entry(sy).or_insert_with(|| {
                    Arc::new(Section {
                        blocks: None,
                        biomes: None,
                        block_light: None,
                        sky_light: None,
                    })
                });
                Arc::make_mut(section).sky_light = Some(data);
            }
            let mut world = Arc::new(world);
            let before = Arc::clone(&world);
            let mut sky = SkyLighting::default();
            sky.set_enabled(true);
            edit(
                &mut sky,
                &mut world,
                BlockPosition { x: 8, y: 63, z: 8 },
                state("stone", &[]),
            );
            finish(&mut sky, &mut world, LightBounds::new(0, 64).unwrap());
            assert!(sky.pending.is_empty(), "roof={roof}");
            let after = case["after"].as_array().unwrap();
            let mut compared = 0;
            for y in -16..80 {
                for z in 0..16 {
                    for x in 0..16 {
                        if let Some(value) = world.light(x, y, z, true) {
                            let index = ((y + 16) * 256 + z * 16 + x) as usize;
                            assert_eq!(
                                u64::from(value),
                                after[index].as_u64().unwrap(),
                                "roof={roof} at {x},{y},{z}"
                            );
                            compared += 1;
                        }
                    }
                }
            }
            assert!(
                compared >= 16 * 16 * 62,
                "insufficient reconstructed sample for roof={roof}"
            );
            assert_eq!(before.block(8, 63, 8).unwrap().name, "minecraft:air");
        }
    }
}
