//! Isolated postprocessing over the actual generated block snapshot.
//!
//! More block and fluid callbacks are needed before this can produce a
//! playable or saveable chunk. Unknown callbacks fail the entire attempt.
use std::{collections::BTreeSet, sync::Arc};

use anyhow::{Context, Result, bail, ensure};
use native_resources::tree_leaf_update::ShapeEdgeWorld;
use native_world::loaded::{BlockState, ChunkPos, World};

use super::{
    chunk_ticks::{RegionTicks, ScheduleOutcome, TickRegistry},
    postprocessing::{
        ConvertedPostprocessingRegion, PostprocessingChunk, PostprocessingWorld,
        update_from_neighbor_shapes,
    },
};

/// An isolated attempt. The source snapshot is immutable, and no ticks are
/// returned if a callback fails or if any chunk remains unfinished.
pub struct GeneratedPostprocessing {
    world: Arc<World>,
    min_y: i32,
    max_y: i32,
    region: ConvertedPostprocessingRegion,
    completed: BTreeSet<ChunkPos>,
    active: Option<ChunkPos>,
    failed: bool,
}

/// Completed callbacks for supported blocks only. Other conversion, lighting,
/// entity and world-save requirements must still be validated.
pub struct PostprocessedGeneratedRegion {
    pub world: Arc<World>,
    pub ticks: RegionTicks,
    pub chunks: BTreeSet<ChunkPos>,
}

impl GeneratedPostprocessing {
    pub(super) fn new(
        world: Arc<World>,
        min_y: i32,
        max_y: i32,
        region: ConvertedPostprocessingRegion,
    ) -> Self {
        Self {
            world,
            min_y,
            max_y,
            region,
            completed: BTreeSet::new(),
            active: None,
            failed: false,
        }
    }

    /// Run a bounded number of callbacks without interleaving another chunk.
    /// Java runs each chunk's pass synchronously against its live neighbors.
    pub fn poll(&mut self, pos: ChunkPos, budget: usize) -> Result<bool> {
        ensure!(!self.failed, "generated postprocessing attempt failed");
        ensure!(
            !self.completed.contains(&pos),
            "chunk was already processed"
        );
        ensure!(
            self.active.is_none_or(|active| active == pos),
            "another chunk is being processed"
        );
        let task = self
            .region
            .tasks
            .get_mut(&pos)
            .context("chunk is not in the converted region")?;
        self.active = Some(pos);
        self.failed = true;
        let mut view = GeneratedBlockView {
            world: &self.world,
            min_y: self.min_y,
            max_y: self.max_y,
            ticks: &mut self.region.ticks,
        };
        let done = task.poll(&mut view, budget)?;
        if done {
            self.completed.insert(pos);
            self.active = None;
        }
        self.failed = false;
        Ok(done)
    }

    pub fn finish(self) -> Result<PostprocessedGeneratedRegion> {
        ensure!(!self.failed, "generated postprocessing attempt failed");
        ensure!(
            self.completed.len() == self.region.tasks.len(),
            "generated postprocessing is incomplete"
        );
        Ok(PostprocessedGeneratedRegion {
            world: self.world,
            ticks: self.region.ticks,
            chunks: self.completed,
        })
    }
}

struct GeneratedBlockView<'a> {
    world: &'a World,
    min_y: i32,
    max_y: i32,
    ticks: &'a mut RegionTicks,
}

fn supported_dry_block(state: &BlockState) -> Result<bool> {
    ensure!(
        state.properties.is_empty(),
        "unsupported generated postprocessing block state"
    );
    match state.name.as_str() {
        "minecraft:sand" | "minecraft:red_sand" | "minecraft:gravel" => Ok(true),
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" | "minecraft:stone"
        | "minecraft:dirt" => Ok(false),
        _ => bail!("unsupported generated postprocessing block {}", state.name),
    }
}

impl GeneratedBlockView<'_> {
    fn read(&self, pos: [i32; 3]) -> Result<BlockState> {
        if !(self.min_y..self.max_y).contains(&pos[1]) {
            return Ok(BlockState {
                name: "minecraft:void_air".into(),
                properties: Default::default(),
            });
        }
        let chunk = ChunkPos {
            x: pos[0].div_euclid(16),
            z: pos[2].div_euclid(16),
        };
        ensure!(
            self.ticks.chunk(chunk).is_some(),
            "postprocessing neighbor is not converted"
        );
        self.world
            .block(pos[0], pos[1], pos[2])
            .cloned()
            .context("generated postprocessing block is missing")
    }
}

impl ShapeEdgeWorld for GeneratedBlockView<'_> {
    type State = BlockState;

    fn block(&mut self, pos: [i32; 3]) -> Result<BlockState> {
        self.read(pos)
    }

    fn update_shape(
        &mut self,
        pos: [i32; 3],
        state: &BlockState,
        _: [i32; 3],
        _: [i32; 3],
        _: &BlockState,
    ) -> Result<BlockState> {
        if supported_dry_block(state)? {
            ensure!(
                self.ticks
                    .schedule(TickRegistry::Block, &state.name, pos, 2, 0)?
                    != ScheduleOutcome::MissingChunk,
                "falling block tick container is missing"
            );
        }
        Ok(state.clone())
    }

    fn set_block(&mut self, _: [i32; 3], _: &BlockState, _: i32) -> Result<()> {
        bail!("generated postprocessing block writes are not implemented")
    }
}

impl PostprocessingWorld for GeneratedBlockView<'_> {
    type State = BlockState;
    type Fluid = ();

    fn block(&mut self, pos: [i32; 3]) -> Result<BlockState> {
        self.read(pos)
    }
    fn fluid(&mut self, state: &BlockState) -> Result<Option<()>> {
        supported_dry_block(state)?;
        Ok(None)
    }
    fn tick_fluid(&mut self, _: [i32; 3], _: &BlockState, _: &()) -> Result<()> {
        bail!("generated fluid postprocessing is not implemented")
    }
    fn is_liquid_block(&mut self, _: &BlockState) -> Result<bool> {
        Ok(false)
    }
    fn tick_liquid_block(&mut self, _: [i32; 3], _: &BlockState) -> Result<()> {
        bail!("generated liquid postprocessing is not implemented")
    }
    fn update_neighbor_shapes(&mut self, pos: [i32; 3], state: &BlockState) -> Result<BlockState> {
        update_from_neighbor_shapes(self, pos, state)
    }
    fn set_block(&mut self, _: [i32; 3], _: &BlockState, _: i32) -> Result<()> {
        bail!("generated postprocessing block writes are not implemented")
    }
    fn clear_processed_section(&mut self, _: usize) -> Result<()> {
        Ok(())
    }
}

impl PostprocessingChunk for GeneratedBlockView<'_> {
    // Fresh generation tracks its materialized block entities separately; the
    // caller validates the generated entity data before constructing this view.
    fn pending_block_entity_positions(&mut self) -> Result<Vec<[i32; 3]>> {
        Ok(Vec::new())
    }
    fn promote_pending_block_entity(&mut self, _: [i32; 3]) -> Result<()> {
        bail!("unexpected pending generated block entity")
    }
    fn clear_pending_block_entities(&mut self) -> Result<()> {
        Ok(())
    }
    fn apply_upgrade_data(&mut self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::{
        chunk_ticks::{ChunkTicks, ConvertedChunkTicks, SavedTick},
        postprocessing::PostprocessingTask,
    };
    use native_core::postprocessing::PostProcessing;
    use native_world::loaded::{BLOCK_COUNT, Chunk, Section};
    use std::collections::BTreeMap;

    fn fixture(unsupported: bool) -> (GeneratedPostprocessing, Arc<World>, ChunkPos) {
        let pos = ChunkPos { x: 0, z: 0 };
        let mut world = World::new("minecraft:overworld");
        let state = |name: &str| BlockState {
            name: format!("minecraft:{name}"),
            properties: BTreeMap::new(),
        };
        let stone = world.intern_state(state("stone"));
        let sand = world.intern_state(state("sand"));
        let red = world.intern_state(state("red_sand"));
        let gravel = world.intern_state(state("gravel"));
        let unknown = world.intern_state(state("oak_log"));
        let mut blocks = Box::new([stone; BLOCK_COUNT]);
        let index = |x: usize| x + 8 * 16 + 8 * 256;
        blocks[index(8)] = sand;
        blocks[index(9)] = if unsupported { unknown } else { red };
        blocks[index(10)] = gravel;
        world.chunks.insert(
            pos,
            Arc::new(Chunk {
                position: pos,
                status: Some("minecraft:spawn".into()),
                block_light_correct: false,
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
        let world = Arc::new(world);
        let mut marks = PostProcessing::new(0, 16).unwrap();
        for x in [8, 8, 9, 10] {
            marks.mark(x, 8, 8);
        }
        let task = PostprocessingTask::new(Arc::new(marks), pos);
        let ticks = ConvertedChunkTicks {
            blocks: ChunkTicks::from_saved(
                pos,
                vec![SavedTick {
                    kind: "minecraft:sand".into(),
                    position: [8, 8, 8],
                    delay: 0,
                    priority: 0,
                }],
            )
            .unwrap(),
            fluids: ChunkTicks::from_saved(pos, vec![]).unwrap(),
        };
        let region = ConvertedPostprocessingRegion {
            tasks: BTreeMap::from([(pos, task)]),
            ticks: RegionTicks::new(BTreeMap::from([(pos, ticks)]), 100, 2).unwrap(),
        };
        (
            GeneratedPostprocessing::new(Arc::clone(&world), 0, 16, region),
            world,
            pos,
        )
    }

    fn result_trace(budget: usize) -> Vec<String> {
        let (mut stage, source, pos) = fixture(false);
        while !stage.poll(pos, budget).unwrap() {}
        let result = stage.finish().unwrap();
        assert!(Arc::ptr_eq(&source, &result.world));
        let mut trace = Vec::new();
        for tick in result.ticks.chunk(pos).unwrap().blocks.pack(100) {
            trace.push(format!(
                "{} {},{},{} {} {}",
                tick.kind,
                tick.position[0],
                tick.position[1],
                tick.position[2],
                tick.delay,
                tick.priority
            ));
        }
        trace.push(format!("counter {}", result.ticks.next_sub_tick()));
        trace
    }

    #[test]
    fn generated_falling_blocks_schedule_real_ticks_and_fail_atomically() {
        let expected = vec![
            "minecraft:sand 8,8,8 0 0",
            "minecraft:red_sand 9,8,8 2 0",
            "minecraft:gravel 10,8,8 2 0",
            "counter 26",
        ];
        assert_eq!(result_trace(1), expected);
        assert_eq!(result_trace(100), expected);
        let (mut stage, source, pos) = fixture(true);
        assert!(stage.poll(pos, 3).is_err());
        assert!(stage.poll(pos, 1).is_err());
        assert!(stage.finish().is_err());
        assert_eq!(source.block(9, 8, 8).unwrap().name, "minecraft:oak_log");
        assert!(source.saved_block_ticks.is_empty());
        let (stage, _, _) = fixture(false);
        assert!(stage.finish().is_err());
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn generated_falling_postprocessing_matches_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
            .join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        assert_eq!(manifest["id"], "26.3");
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let classpath = std::env::join_paths(paths).unwrap();
        let source = temp
            .path()
            .join("GeneratedFallingPostprocessingOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/GeneratedFallingPostprocessingOracle.java"),
        )
        .unwrap();
        let compile = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(source)
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let run = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(classpath)
            .arg("GeneratedFallingPostprocessingOracle")
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        let output = String::from_utf8(run.stdout).unwrap();
        let lines: Vec<_> = output.lines().collect();
        let start = lines
            .iter()
            .position(|line| *line == "BEGIN_RESULT")
            .unwrap();
        let end = lines.iter().position(|line| *line == "END_RESULT").unwrap();
        assert_eq!(
            &lines[start + 1..end],
            result_trace(1)
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );
    }
}
