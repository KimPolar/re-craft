//! Incremental block-position portion of LevelChunk.postProcessGeneration.
//! Run only after features and all required neighbor chunks are ready. The
//! generation world must remain isolated between polls: Java executes the
//! entire pass synchronously before normal world simulation resumes. Block
//! entity promotion and upgrade data are separate steps and are not handled here.

use std::sync::Arc;

use anyhow::{Result, bail};
use native_core::postprocessing::PostProcessing;
use native_world::loaded::ChunkPos;

/// Access to a generation world with its real block/fluid and neighbor rules.
/// State equality must be registry identity, as Java compares canonical states.
pub trait PostprocessingWorld {
    type State: Clone + Eq;
    type Fluid;

    fn block(&mut self, pos: [i32; 3]) -> Result<Self::State>;
    fn fluid(&mut self, state: &Self::State) -> Result<Option<Self::Fluid>>;
    fn tick_fluid(&mut self, pos: [i32; 3], state: &Self::State, fluid: &Self::Fluid)
    -> Result<()>;
    fn is_liquid_block(&mut self, state: &Self::State) -> Result<bool>;
    fn tick_liquid_block(&mut self, pos: [i32; 3], state: &Self::State) -> Result<()>;
    fn update_neighbor_shapes(&mut self, pos: [i32; 3], state: &Self::State)
    -> Result<Self::State>;
    fn set_block(&mut self, pos: [i32; 3], state: &Self::State, flags: i32) -> Result<()>;
}

/// Processes an immutable ordered list in bounded batches. A failed callback
/// may already have changed the world: the runner is poisoned and the caller
/// must discard that generation attempt rather than retrying a partial entry.
/// The source list is immutable; callers may clear the corresponding pending
/// data only after this pass and the remaining Java postprocessing steps succeed.
pub struct PostprocessingBlocks {
    marks: Arc<PostProcessing>,
    chunk: ChunkPos,
    section: usize,
    index: usize,
    failed: bool,
}

impl PostprocessingBlocks {
    pub fn new(marks: Arc<PostProcessing>, chunk: ChunkPos) -> Self {
        Self {
            marks,
            chunk,
            section: 0,
            index: 0,
            failed: false,
        }
    }

    fn skip_empty(&mut self) {
        while let Some(section) = self.marks.sections().get(self.section) {
            if self.index < section.len() {
                break;
            }
            self.section += 1;
            self.index = 0;
        }
    }

    /// Returns true when all marked positions have been processed. Java uses
    /// the original state for both operations even if the fluid tick changes it.
    pub fn poll<W: PostprocessingWorld>(
        &mut self,
        world: &mut W,
        max_positions: usize,
    ) -> Result<bool> {
        if self.failed {
            bail!("postprocessing attempt already failed");
        }
        self.skip_empty();
        for _ in 0..max_positions {
            let Some(section) = self.marks.sections().get(self.section) else {
                return Ok(true);
            };
            let packed = section[self.index];
            let pos = self
                .marks
                .unpack(self.section, packed, self.chunk.x, self.chunk.z)
                .expect("section was checked");
            // Mark failed before any callback, including unwinding through it.
            self.failed = true;
            let original = world.block(pos)?;
            if let Some(fluid) = world.fluid(&original)? {
                world.tick_fluid(pos, &original, &fluid)?;
            }
            if world.is_liquid_block(&original)? {
                world.tick_liquid_block(pos, &original)?;
            } else {
                let updated = world.update_neighbor_shapes(pos, &original)?;
                if updated != original {
                    world.set_block(pos, &updated, 276)?;
                }
            }
            self.failed = false;
            self.index += 1;
            self.skip_empty();
        }
        Ok(self.section == self.marks.sections().len())
    }
}
