//! Incremental block-position portion of LevelChunk.postProcessGeneration.
//! Run only after features and all required neighbor chunks are ready. The
//! generation world must remain isolated between polls: Java executes the
//! entire pass synchronously before normal world simulation resumes. The
//! caller supplies the actual block, entity and upgrade operations.

use std::sync::Arc;

use anyhow::{Result, bail};
use native_core::postprocessing::PostProcessing;
use native_world::loaded::ChunkPos;

/// Apply `Block.updateFromNeighbourShapes` using a world's real updateShape
/// implementation and its scheduled-tick/random side effects. Reads happen
/// immediately before each callback because earlier callbacks may mutate the
/// world. The returned state is not written by this function.
pub fn update_from_neighbor_shapes<W: native_resources::tree_leaf_update::ShapeEdgeWorld>(
    world: &mut W,
    pos: [i32; 3],
    original: &W::State,
) -> Result<W::State> {
    let mut state = original.clone();
    for direction in [
        [-1, 0, 0],
        [1, 0, 0],
        [0, 0, -1],
        [0, 0, 1],
        [0, -1, 0],
        [0, 1, 0],
    ] {
        let neighbor = [
            pos[0].wrapping_add(direction[0]),
            pos[1].wrapping_add(direction[1]),
            pos[2].wrapping_add(direction[2]),
        ];
        let other = world.block(neighbor)?;
        state = world.update_shape(pos, &state, direction, neighbor, &other)?;
    }
    Ok(state)
}

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

    /// Clear a nonempty section's pending list after its last entry succeeds.
    fn clear_processed_section(&mut self, section: usize) -> Result<()>;
}

/// Processes an immutable ordered list in bounded batches. A failed callback
/// may already have changed the world: the runner is poisoned and the caller
/// must discard that generation attempt rather than retrying a partial entry.
/// The source list is immutable; the corresponding live section is cleared
/// through the callback immediately after its entries succeed. This runner
/// does not complete the remaining Java postprocessing steps.
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
        Ok(self.poll_counted(world, max_positions)?.0)
    }

    fn poll_counted<W: PostprocessingWorld>(
        &mut self,
        world: &mut W,
        max_positions: usize,
    ) -> Result<(bool, usize)> {
        if self.failed {
            bail!("postprocessing attempt already failed");
        }
        self.skip_empty();
        let mut processed = 0;
        for _ in 0..max_positions {
            let Some(section) = self.marks.sections().get(self.section) else {
                return Ok((true, processed));
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
            self.index += 1;
            processed += 1;
            if self.index == section.len() {
                world.clear_processed_section(self.section)?;
            }
            self.failed = false;
            self.skip_empty();
        }
        Ok((self.section == self.marks.sections().len(), processed))
    }
}

/// Remaining operations of Java's chunk postprocessing. The position list
/// must use the pending map's iteration order at the time of the call.
pub trait PostprocessingChunk: PostprocessingWorld {
    fn pending_block_entity_positions(&mut self) -> Result<Vec<[i32; 3]>>;
    fn promote_pending_block_entity(&mut self, pos: [i32; 3]) -> Result<()>;
    fn clear_pending_block_entities(&mut self) -> Result<()>;
    fn apply_upgrade_data(&mut self) -> Result<()>;
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Phase {
    Blocks,
    CaptureEntities,
    PromoteEntities,
    ClearEntities,
    Upgrade,
    Done,
}

/// Complete the ordered post-generation pass in bounded work units. The
/// caller must keep the world isolated until completion. Any callback error
/// poisons this attempt because it may have already changed the world.
pub struct PostprocessingTask {
    blocks: PostprocessingBlocks,
    phase: Phase,
    positions: Vec<[i32; 3]>,
    next_entity: usize,
    failed: bool,
}

impl PostprocessingTask {
    pub fn new(marks: Arc<PostProcessing>, chunk: ChunkPos) -> Self {
        Self {
            blocks: PostprocessingBlocks::new(marks, chunk),
            phase: Phase::Blocks,
            positions: Vec::new(),
            next_entity: 0,
            failed: false,
        }
    }

    pub fn poll<W: PostprocessingChunk>(
        &mut self,
        world: &mut W,
        mut budget: usize,
    ) -> Result<bool> {
        if self.failed {
            bail!("postprocessing attempt already failed");
        }
        while budget > 0 {
            match self.phase {
                Phase::Blocks => {
                    self.failed = true;
                    let (done, used) = self.blocks.poll_counted(world, budget)?;
                    self.failed = false;
                    budget -= used;
                    if !done {
                        return Ok(false);
                    }
                    self.phase = Phase::CaptureEntities;
                }
                Phase::CaptureEntities => {
                    self.failed = true;
                    self.positions = world.pending_block_entity_positions()?;
                    self.failed = false;
                    self.phase = Phase::PromoteEntities;
                    budget -= 1;
                }
                Phase::PromoteEntities if self.next_entity == self.positions.len() => {
                    self.phase = Phase::ClearEntities;
                }
                Phase::PromoteEntities => {
                    self.failed = true;
                    world.promote_pending_block_entity(self.positions[self.next_entity])?;
                    self.failed = false;
                    self.next_entity += 1;
                    budget -= 1;
                }
                Phase::ClearEntities => {
                    self.failed = true;
                    world.clear_pending_block_entities()?;
                    self.failed = false;
                    self.phase = Phase::Upgrade;
                    budget -= 1;
                }
                Phase::Upgrade => {
                    self.failed = true;
                    world.apply_upgrade_data()?;
                    self.failed = false;
                    self.phase = Phase::Done;
                    budget -= 1;
                }
                Phase::Done => return Ok(true),
            }
        }
        Ok(self.phase == Phase::Done)
    }
}
