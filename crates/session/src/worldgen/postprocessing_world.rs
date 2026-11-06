//! Isolated postprocessing over the actual generated block snapshot.
//!
//! More block and fluid callbacks are needed before this can produce a
//! playable or saveable chunk. Unknown callbacks fail the entire attempt.
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use native_resources::tree_leaf_update::ShapeEdgeWorld;
use native_world::{
    generation_light::VisibleLightSnapshot,
    loaded::{BlockPosition, BlockState, ChunkPos, World},
};

use super::{
    chunk_ticks::{RegionTicks, ScheduleOutcome, TickRegistry},
    entity_order::{LivePositions, PendingPositions},
    postprocessing::{
        ConvertedPostprocessingRegion, PostprocessingChunk, PostprocessingWorld,
        update_from_neighbor_shapes,
    },
};

/// Block entities owned by the converted LevelChunk, isolated from the protochunk.
#[derive(Clone, Default)]
pub(super) struct ConvertedBlockEntities {
    records: BTreeMap<[i32; 3], native_persistence::nbt::Tag>,
    pending: PendingPositions,
    live: LivePositions,
}

impl ConvertedBlockEntities {
    pub(super) fn from_proto(
        data: native_persistence::nbt::Tag,
        live: Option<&LivePositions>,
        pending: Option<&PendingPositions>,
    ) -> Result<Self> {
        use native_persistence::nbt::Tag;
        let Tag::List { values, .. } = data else {
            bail!("invalid generated block entity list")
        };
        let mut records = BTreeMap::new();
        let mut pending_keys = BTreeSet::new();
        for tag in values {
            let number = |name| match tag.get(name) {
                Some(Tag::Int(value)) => Ok(*value),
                _ => bail!("generated block entity coordinate is missing"),
            };
            let pos = [number("x")?, number("y")?, number("z")?];
            if matches!(tag.get("id"), Some(Tag::String(id)) if id.matches("DUMMY")) {
                pending_keys.insert(pos);
            }
            ensure!(
                records.insert(pos, tag).is_none(),
                "duplicate generated block entity"
            );
        }
        ensure!(
            pending.map_or(pending_keys.is_empty(), |history| history.members()
                == &pending_keys),
            "pending block entities do not match history"
        );
        let pending = pending
            .map(PendingPositions::converted_copy)
            .transpose()?
            .unwrap_or_default();
        let live = live.map(LivePositions::converted_copy).unwrap_or_default();
        let live_keys: BTreeSet<_> = live.saving_order()?.into_iter().collect();
        ensure!(
            live_keys.len() + pending_keys.len() == records.len()
                && live_keys
                    .iter()
                    .all(|pos| records.contains_key(pos) && !pending_keys.contains(pos)),
            "materialized block entities do not match history"
        );
        Ok(Self {
            records,
            pending,
            live,
        })
    }

    fn saved(&self) -> Result<native_persistence::nbt::Tag> {
        use native_persistence::nbt::{Tag, TagType};
        ensure!(
            self.pending.members().is_empty(),
            "postprocessed block entities remain pending"
        );
        let values = self
            .live
            .saving_order()?
            .into_iter()
            .map(|pos| {
                let mut data = self
                    .records
                    .get(&pos)
                    .context("postprocessed block entity is missing")?
                    .clone();
                let Tag::Compound(fields) = &mut data else {
                    bail!("invalid postprocessed block entity")
                };
                ensure!(
                    !fields.iter().any(|(name, _)| name.matches("keepPacked")),
                    "duplicate keepPacked field"
                );
                fields.push(("keepPacked".into(), Tag::Byte(0)));
                Ok(data)
            })
            .collect::<Result<Vec<_>>>()?;
        ensure!(
            values.len() == self.records.len(),
            "postprocessed block entity order is incomplete"
        );
        Ok(Tag::List {
            element_type: TagType::Compound,
            values,
        })
    }
}

/// An isolated attempt. The source snapshot is immutable, and no ticks are
/// returned if a callback fails or if any chunk remains unfinished.
#[derive(Clone)]
pub struct GeneratedPostprocessing {
    pub(super) world: Arc<World>,
    pub(super) source: Arc<World>,
    pub(super) batch_revision: Option<u64>,
    min_y: i32,
    max_y: i32,
    pub(super) region: ConvertedPostprocessingRegion,
    completed: BTreeSet<ChunkPos>,
    active: Option<ChunkPos>,
    pub(super) failed: bool,
    pub(super) light: Option<VisibleLightSnapshot>,
    pub(super) light_checkpoint: Option<PostprocessingLightCheckpoint>,
    mushroom_tags: Option<Arc<native_resources::ore_feature::OreBlockTags>>,
    leaf_tags: Option<Arc<native_resources::ore_feature::OreBlockTags>>,
    fence_tags: Option<Arc<native_resources::ore_feature::OreBlockTags>>,
    bubble_tags: Option<Arc<native_resources::fluid_tags::BubbleColumnTags>>,
    washed_blocks: Option<Arc<BTreeSet<String>>>,
    lava_fluids: Option<Arc<BTreeSet<String>>>,
    water_source_conversion: Option<bool>,
    edits: Vec<BlockPosition>,
    read_chunks: RefCell<BTreeSet<ChunkPos>>,
    light_reads: RefCell<BTreeSet<ChunkPos>>,
    pub(super) light_checks: Vec<BlockPosition>,
    heightmaps: BTreeMap<ChunkPos, native_persistence::heightmap::FullHeightmaps>,
    heightmap_tags: Option<Arc<native_resources::heightmap_tags::HeightmapTags>>,
    block_entities: BTreeMap<ChunkPos, ConvertedBlockEntities>,
    block_entity_edits: BTreeMap<BlockPosition, (BlockState, BlockState)>,
    bonus_chest_attempted: bool,
}

/// A bonus attempt either completed or needs another converted chunk. A
/// missing chunk leaves both the postprocessing stage and RNG untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitialBonusChestAttempt {
    Complete(Option<[i32; 3]>),
    NeedsChunk(ChunkPos),
}

/// Result of an isolated bounded postprocessing attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostprocessingAttempt {
    Progress { complete: bool },
    NeedsChunk(ChunkPos),
}

#[derive(Debug)]
struct MissingPostprocessingChunk(ChunkPos);

impl std::fmt::Display for MissingPostprocessingChunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "postprocessing neighbor is not converted: {}, {}",
            self.0.x, self.0.z
        )
    }
}

impl std::error::Error for MissingPostprocessingChunk {}

/// Completed callbacks for supported blocks only. Other conversion, lighting,
/// entity and world-save requirements must still be validated.
pub struct PostprocessedGeneratedRegion {
    pub world: Arc<World>,
    pub(super) ticks: RegionTicks,
    pub chunks: BTreeSet<ChunkPos>,
    /// Ordered block changes performed during postprocessing.
    pub edits: Vec<BlockPosition>,
    pub(super) read_chunks: BTreeSet<ChunkPos>,
    pub(super) sealed_read_chunks: BTreeSet<ChunkPos>,
    pub(super) light_reads: BTreeSet<ChunkPos>,
    pub(super) sealed_light_reads: BTreeSet<ChunkPos>,
    /// These light checks must be processed before publishing the result.
    pub light_checks: Vec<BlockPosition>,
    /// Updated FULL maps; other chunk and world validation remains necessary.
    pub(super) heightmaps: BTreeMap<ChunkPos, native_persistence::heightmap::FullHeightmaps>,
    pub(super) source: Arc<World>,
    pub(super) sealed_world: Arc<World>,
    pub(super) sealed_edits: Vec<BlockPosition>,
    pub(super) sealed_checks: Vec<BlockPosition>,
    pub(super) sealed_chunks: BTreeSet<ChunkPos>,
    pub(super) batch_revision: Option<u64>,
    pub(super) min_y: i32,
    pub(super) max_y: i32,
    pub(super) light_checkpoint: Option<PostprocessingLightCheckpoint>,
    water_source_conversion: Option<bool>,
    block_entities: BTreeMap<ChunkPos, ConvertedBlockEntities>,
    block_entity_edits: BTreeMap<BlockPosition, (BlockState, BlockState)>,
    bonus_chest_attempted: bool,
}

/// A completed explicit light drain and the world revision it covered.
/// Positions before `drained_checks` must not be applied again.
#[derive(Clone)]
pub(super) struct PostprocessingLightCheckpoint {
    pub(super) world: Arc<World>,
    pub(super) visible: VisibleLightSnapshot,
    pub(super) sky_sources: BTreeMap<ChunkPos, Arc<[i32; 256]>>,
    pub(super) drained_checks: usize,
}

impl PostprocessedGeneratedRegion {
    pub(super) fn permits_block_entity_edit(
        &self,
        pos: BlockPosition,
        old: &BlockState,
        new: &BlockState,
    ) -> bool {
        self.block_entity_edits
            .get(&pos)
            .is_some_and(|(before, after)| before == old && after == new)
    }
    /// Block entities after this chunk's post-generation promotions and writes.
    pub fn block_entity_data(&self, pos: ChunkPos) -> Result<native_persistence::nbt::Tag> {
        self.block_entities
            .get(&pos)
            .context("converted block entities are missing")?
            .saved()
    }
    /// Read the completed tick containers without changing their snapshot.
    pub fn ticks(&self) -> &RegionTicks {
        &self.ticks
    }

    /// Read the heightmaps produced by the same postprocessing pass.
    pub fn heightmaps(&self) -> &BTreeMap<ChunkPos, native_persistence::heightmap::FullHeightmaps> {
        &self.heightmaps
    }

    /// The explicit rule used for this pass, if supplied by its caller.
    pub fn water_source_conversion(&self) -> Option<bool> {
        self.water_source_conversion
    }
}

/// Apply one explicitly chosen light-engine drain to a complete input
/// revision. Source heights already include all edits affecting direct sky.
pub(super) fn drain_postprocessing_light(
    world: &World,
    bounds: native_world::block_light::LightBounds,
    before: &VisibleLightSnapshot,
    active: &BTreeSet<ChunkPos>,
    sources: &BTreeMap<ChunkPos, Arc<[i32; 256]>>,
    checks: &[[i32; 3]],
) -> Result<VisibleLightSnapshot> {
    let block = native_world::generation_block_light::check_generated_block_light(
        world, bounds, before, active, checks,
    )
    .context("postprocessing block-light inputs are incomplete or unsupported")?;
    if before.has_sky_channel() {
        native_world::generation_sky_light::check_generated_sky_blocks(
            world, bounds, &block, active, checks, sources,
        )
        .context("postprocessing sky-light inputs are incomplete or unsupported")
    } else {
        Ok(block)
    }
}

impl GeneratedPostprocessing {
    /// Try a bounded pass without exposing partial callbacks on failure.
    /// The caller must preserve Java's callback order when supplying a missing
    /// chunk; merely processing that chunk first is not always equivalent.
    pub fn try_poll(&mut self, pos: ChunkPos, budget: usize) -> Result<PostprocessingAttempt> {
        let mut attempt = self.clone();
        match attempt.poll(pos, budget) {
            Ok(complete) => {
                *self = attempt;
                Ok(PostprocessingAttempt::Progress { complete })
            }
            Err(error) => match error.downcast_ref::<MissingPostprocessingChunk>() {
                Some(missing) => Ok(PostprocessingAttempt::NeedsChunk(missing.0)),
                None => Err(error),
            },
        }
    }

    /// Attempt placement on an isolated fork. Missing neighbors are reported
    /// without publishing partial blocks, ticks, block entities or RNG changes.
    pub fn try_place_initial_bonus_chest<
        R: native_resources::bonus_chest::BonusChestRandom + Clone,
    >(
        &mut self,
        feature: &native_resources::bonus_chest::BonusChestFeature,
        random: &mut R,
        origin: [i32; 3],
    ) -> Result<InitialBonusChestAttempt> {
        let mut attempt = self.clone();
        let mut trial_random = random.clone();
        match attempt.place_initial_bonus_chest(feature, &mut trial_random, origin) {
            Ok(pos) => {
                *self = attempt;
                *random = trial_random;
                Ok(InitialBonusChestAttempt::Complete(pos))
            }
            Err(error) => {
                if let Some(missing) = error.downcast_ref::<MissingPostprocessingChunk>() {
                    Ok(InitialBonusChestAttempt::NeedsChunk(missing.0))
                } else {
                    Err(error)
                }
            }
        }
    }

    pub(super) fn new(
        world: Arc<World>,
        min_y: i32,
        max_y: i32,
        region: ConvertedPostprocessingRegion,
        light: Option<VisibleLightSnapshot>,
        mushroom_tags: Option<Arc<native_resources::ore_feature::OreBlockTags>>,
        heightmaps: BTreeMap<ChunkPos, native_persistence::heightmap::FullHeightmaps>,
        heightmap_tags: Option<Arc<native_resources::heightmap_tags::HeightmapTags>>,
    ) -> Self {
        let block_entities = region
            .tasks
            .keys()
            .copied()
            .map(|pos| (pos, ConvertedBlockEntities::default()))
            .collect();
        Self {
            source: Arc::clone(&world),
            world,
            batch_revision: None,
            min_y,
            max_y,
            region,
            completed: BTreeSet::new(),
            active: None,
            failed: false,
            light,
            light_checkpoint: None,
            mushroom_tags,
            leaf_tags: None,
            fence_tags: None,
            bubble_tags: None,
            washed_blocks: None,
            lava_fluids: None,
            water_source_conversion: None,
            edits: Vec::new(),
            read_chunks: RefCell::new(BTreeSet::new()),
            light_reads: RefCell::new(BTreeSet::new()),
            light_checks: Vec::new(),
            heightmaps,
            heightmap_tags,
            block_entities,
            block_entity_edits: BTreeMap::new(),
            bonus_chest_attempted: false,
        }
    }

    pub(super) fn set_block_entities(
        &mut self,
        entities: BTreeMap<ChunkPos, ConvertedBlockEntities>,
    ) -> Result<()> {
        ensure!(
            entities.keys().eq(self.region.tasks.keys()),
            "converted block entity chunk set differs"
        );
        self.block_entities = entities;
        Ok(())
    }

    pub(super) fn bind_batch_revision(&mut self, revision: u64) {
        self.batch_revision = Some(revision);
    }

    /// Restore the already completed chunks without running their callbacks a
    /// second time. The caller validates the source and supplies its light.
    pub(super) fn from_completed(
        completed: &PostprocessedGeneratedRegion,
        light: Option<VisibleLightSnapshot>,
        mushroom_tags: Option<Arc<native_resources::ore_feature::OreBlockTags>>,
        heightmap_tags: Arc<native_resources::heightmap_tags::HeightmapTags>,
    ) -> Result<Self> {
        let height = completed
            .max_y
            .checked_sub(completed.min_y)
            .context("invalid postprocessing height")?;
        let marks = Arc::new(
            native_core::postprocessing::PostProcessing::new(completed.min_y, height)
                .context("unsupported postprocessing height")?,
        );
        let tasks = completed
            .chunks
            .iter()
            .copied()
            .map(|pos| {
                (
                    pos,
                    super::postprocessing::PostprocessingTask::new(Arc::clone(&marks), pos),
                )
            })
            .collect();
        let region = ConvertedPostprocessingRegion {
            tasks,
            ticks: completed.ticks.clone(),
        };
        let mut stage = Self::new(
            Arc::clone(&completed.world),
            completed.min_y,
            completed.max_y,
            region,
            light,
            mushroom_tags,
            completed.heightmaps.clone(),
            Some(heightmap_tags),
        );
        stage.source = Arc::clone(&completed.source);
        stage.batch_revision = completed.batch_revision;
        stage.completed = completed.chunks.clone();
        stage.light_checkpoint = completed.light_checkpoint.clone();
        stage.water_source_conversion = completed.water_source_conversion;
        stage.edits = completed.edits.clone();
        stage.read_chunks = RefCell::new(completed.read_chunks.clone());
        stage.light_reads = RefCell::new(completed.light_reads.clone());
        stage.light_checks = completed.light_checks.clone();
        stage.block_entities = completed.block_entities.clone();
        stage.block_entity_edits = completed.block_entity_edits.clone();
        stage.bonus_chest_attempted = completed.bonus_chest_attempted;
        Ok(stage)
    }

    pub(super) fn check_registration(&self, pos: ChunkPos) -> Result<()> {
        ensure!(
            !self.failed
                && self.active.is_none()
                && self.completed.len() == self.region.tasks.len(),
            "finish existing postprocessing before registering a chunk"
        );
        ensure!(
            !self.region.tasks.contains_key(&pos)
                && !self.heightmaps.contains_key(&pos)
                && !self.block_entities.contains_key(&pos)
                && self.region.ticks.chunk(pos).is_none(),
            "postprocessing chunk is already registered"
        );
        Ok(())
    }

    pub(super) fn register_converted_chunk(
        &mut self,
        pos: ChunkPos,
        converted: super::postprocessing::ConvertedChunkPostprocessing,
        entities: ConvertedBlockEntities,
        heightmaps: native_persistence::heightmap::FullHeightmaps,
        light: Option<VisibleLightSnapshot>,
    ) -> Result<()> {
        self.check_registration(pos)?;
        self.region.ticks.register_chunk(pos, converted.ticks)?;
        self.region.tasks.insert(pos, converted.task);
        self.block_entities.insert(pos, entities);
        self.heightmaps.insert(pos, heightmaps);
        if let Some(light) = light {
            self.light = Some(light);
        }
        Ok(())
    }

    /// `before` must include the last checkpoint plus every newly needed
    /// column; callers are responsible for validating its batch revision.
    pub(super) fn drain_recorded_light(
        &mut self,
        before: VisibleLightSnapshot,
        active: &BTreeSet<ChunkPos>,
        initial_sources: &BTreeMap<ChunkPos, Arc<[i32; 256]>>,
    ) -> Result<bool> {
        ensure!(!self.failed, "generated postprocessing attempt failed");
        let checkpoint = self.light_checkpoint.as_ref();
        let drained = checkpoint.map_or(0, |saved| saved.drained_checks);
        let pending = self
            .light_checks
            .get(drained..)
            .context("invalid postprocessing light checkpoint")?;
        if pending.is_empty() {
            return Ok(false);
        }
        let bounds = native_world::block_light::LightBounds::new(self.min_y, self.max_y)
            .context("unsupported postprocessing light bounds")?;
        let previous_world = checkpoint.map_or(self.source.as_ref(), |saved| saved.world.as_ref());
        let previous_sources = checkpoint.map_or(initial_sources, |saved| &saved.sky_sources);
        let updated_sources = if before.has_sky_channel() {
            super::terrain_batch::updated_postprocessing_sky_sources(
                previous_world,
                &self.world,
                pending,
                bounds,
                previous_sources,
            )?
        } else {
            None
        };
        let sources = updated_sources.unwrap_or_else(|| previous_sources.clone());
        let checks: Vec<_> = pending.iter().map(|pos| [pos.x, pos.y, pos.z]).collect();
        let visible =
            drain_postprocessing_light(&self.world, bounds, &before, active, &sources, &checks)?;
        self.light = Some(visible.clone());
        self.light_checkpoint = Some(PostprocessingLightCheckpoint {
            world: Arc::clone(&self.world),
            visible,
            sky_sources: sources,
            drained_checks: self.light_checks.len(),
        });
        Ok(true)
    }

    pub(super) fn set_leaf_tags(
        &mut self,
        tags: Option<Arc<native_resources::ore_feature::OreBlockTags>>,
    ) {
        self.leaf_tags = tags;
    }

    pub(super) fn set_fence_tags(
        &mut self,
        tags: Option<Arc<native_resources::ore_feature::OreBlockTags>>,
    ) {
        self.fence_tags = tags;
    }

    pub(super) fn set_bubble_tags(
        &mut self,
        tags: Option<Arc<native_resources::fluid_tags::BubbleColumnTags>>,
    ) {
        self.bubble_tags = tags;
    }

    pub(super) fn set_fluid_tags(
        &mut self,
        washed: Option<Arc<BTreeSet<String>>>,
        lava: Option<Arc<BTreeSet<String>>>,
    ) {
        self.washed_blocks = washed;
        self.lava_fluids = lava;
    }

    /// Supply the level's actual gamerule before executing any chunk. The
    /// caller must keep it fixed while this isolated generation pass runs.
    pub fn set_water_source_conversion(&mut self, enabled: bool) -> Result<()> {
        ensure!(
            !self.failed && self.active.is_none() && self.completed.is_empty(),
            "water source conversion must be set before postprocessing begins"
        );
        self.water_source_conversion = Some(enabled);
        Ok(())
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
            chunk: pos,
            block_entities: self
                .block_entities
                .get_mut(&pos)
                .context("converted block entities are missing")?,
            block_entity_edits: &mut self.block_entity_edits,
            world: &mut self.world,
            read_chunks: &self.read_chunks,
            light_reads: &self.light_reads,
            min_y: self.min_y,
            max_y: self.max_y,
            ticks: &mut self.region.ticks,
            light: self.light.as_ref(),
            mushroom_tags: self.mushroom_tags.as_deref(),
            leaf_tags: self.leaf_tags.as_deref(),
            fence_tags: self.fence_tags.as_deref(),
            bubble_tags: self.bubble_tags.as_deref(),
            washed_blocks: self.washed_blocks.as_deref(),
            lava_fluids: self.lava_fluids.as_deref(),
            water_source_conversion: self.water_source_conversion,
            edits: &mut self.edits,
            light_checks: &mut self.light_checks,
            heightmaps: &mut self.heightmaps,
            heightmap_tags: self.heightmap_tags.as_deref(),
        };
        let done = task.poll(&mut view, budget)?;
        if done {
            self.completed.insert(pos);
            self.active = None;
        }
        self.failed = false;
        Ok(done)
    }

    /// Place the initial bonus chest after all selected LevelChunks finish
    /// postprocessing. The isolated stage is discarded on an unsupported write.
    /// The caller controls the server-level RNG and must invoke this before the
    /// first player-spawn collision search. Its clone must have independent
    /// state so an unsuccessful attempt cannot advance the original stream.
    pub fn place_initial_bonus_chest<R: native_resources::bonus_chest::BonusChestRandom + Clone>(
        &mut self,
        feature: &native_resources::bonus_chest::BonusChestFeature,
        random: &mut R,
        origin: [i32; 3],
    ) -> Result<Option<[i32; 3]>> {
        ensure!(
            !self.failed
                && self.active.is_none()
                && self.completed.len() == self.region.tasks.len(),
            "complete postprocessing before the bonus chest"
        );
        ensure!(
            !self.bonus_chest_attempted,
            "bonus chest was already attempted"
        );
        let pos = ChunkPos {
            x: origin[0].div_euclid(16),
            z: origin[2].div_euclid(16),
        };
        ensure!(
            self.completed.contains(&pos),
            "bonus chest spawn chunk is not postprocessed"
        );
        let mut staged_random = random.clone();
        self.failed = true;
        let mut view = GeneratedBlockView {
            chunk: pos,
            block_entities: self
                .block_entities
                .get_mut(&pos)
                .context("converted block entities are missing")?,
            block_entity_edits: &mut self.block_entity_edits,
            world: &mut self.world,
            read_chunks: &self.read_chunks,
            light_reads: &self.light_reads,
            min_y: self.min_y,
            max_y: self.max_y,
            ticks: &mut self.region.ticks,
            light: self.light.as_ref(),
            mushroom_tags: self.mushroom_tags.as_deref(),
            leaf_tags: self.leaf_tags.as_deref(),
            fence_tags: self.fence_tags.as_deref(),
            bubble_tags: self.bubble_tags.as_deref(),
            washed_blocks: self.washed_blocks.as_deref(),
            lava_fluids: self.lava_fluids.as_deref(),
            water_source_conversion: self.water_source_conversion,
            edits: &mut self.edits,
            light_checks: &mut self.light_checks,
            heightmaps: &mut self.heightmaps,
            heightmap_tags: self.heightmap_tags.as_deref(),
        };
        let placed = feature.place(&mut view, &mut staged_random, origin)?;
        *random = staged_random;
        self.bonus_chest_attempted = true;
        self.failed = false;
        Ok(placed)
    }

    pub fn finish(self) -> Result<PostprocessedGeneratedRegion> {
        ensure!(!self.failed, "generated postprocessing attempt failed");
        ensure!(
            self.completed.len() == self.region.tasks.len(),
            "generated postprocessing is incomplete"
        );
        let read_chunks = self.read_chunks.into_inner();
        let light_reads = self.light_reads.into_inner();
        Ok(PostprocessedGeneratedRegion {
            sealed_world: Arc::clone(&self.world),
            world: self.world,
            ticks: self.region.ticks,
            sealed_chunks: self.completed.clone(),
            chunks: self.completed,
            sealed_edits: self.edits.clone(),
            edits: self.edits,
            sealed_read_chunks: read_chunks.clone(),
            read_chunks,
            sealed_light_reads: light_reads.clone(),
            light_reads,
            sealed_checks: self.light_checks.clone(),
            light_checks: self.light_checks,
            heightmaps: self.heightmaps,
            source: self.source,
            batch_revision: self.batch_revision,
            min_y: self.min_y,
            max_y: self.max_y,
            light_checkpoint: self.light_checkpoint,
            water_source_conversion: self.water_source_conversion,
            block_entities: self.block_entities,
            block_entity_edits: self.block_entity_edits,
            bonus_chest_attempted: self.bonus_chest_attempted,
        })
    }
}

struct GeneratedBlockView<'a> {
    chunk: ChunkPos,
    block_entities: &'a mut ConvertedBlockEntities,
    block_entity_edits: &'a mut BTreeMap<BlockPosition, (BlockState, BlockState)>,
    world: &'a mut Arc<World>,
    read_chunks: &'a RefCell<BTreeSet<ChunkPos>>,
    light_reads: &'a RefCell<BTreeSet<ChunkPos>>,
    min_y: i32,
    max_y: i32,
    ticks: &'a mut RegionTicks,
    light: Option<&'a VisibleLightSnapshot>,
    mushroom_tags: Option<&'a native_resources::ore_feature::OreBlockTags>,
    leaf_tags: Option<&'a native_resources::ore_feature::OreBlockTags>,
    fence_tags: Option<&'a native_resources::ore_feature::OreBlockTags>,
    bubble_tags: Option<&'a native_resources::fluid_tags::BubbleColumnTags>,
    washed_blocks: Option<&'a BTreeSet<String>>,
    lava_fluids: Option<&'a BTreeSet<String>>,
    water_source_conversion: Option<bool>,
    edits: &'a mut Vec<BlockPosition>,
    light_checks: &'a mut Vec<BlockPosition>,
    heightmaps: &'a mut BTreeMap<ChunkPos, native_persistence::heightmap::FullHeightmaps>,
    heightmap_tags: Option<&'a native_resources::heightmap_tags::HeightmapTags>,
}

#[derive(Clone, Copy)]
enum DryBlock {
    Plain,
    Falling,
    Mushroom,
    Leaves,
}

// Full cubes whose block states have no properties and whose Java shape
// callbacks are inert. Keep this explicit: other blocks can schedule ticks,
// hold fluids, or expose non-full collision faces.
fn inert_generated_full_cube(name: &str) -> bool {
    matches!(
        name,
        "minecraft:stone"
            | "minecraft:granite"
            | "minecraft:diorite"
            | "minecraft:andesite"
            | "minecraft:tuff"
            | "minecraft:calcite"
            | "minecraft:dripstone_block"
            | "minecraft:cobblestone"
            | "minecraft:mossy_cobblestone"
            | "minecraft:dirt"
            | "minecraft:coarse_dirt"
            | "minecraft:rooted_dirt"
            | "minecraft:clay"
            | "minecraft:bedrock"
            | "minecraft:netherrack"
            | "minecraft:end_stone"
            | "minecraft:blackstone"
            | "minecraft:obsidian"
            | "minecraft:crying_obsidian"
            | "minecraft:coal_ore"
            | "minecraft:iron_ore"
            | "minecraft:copper_ore"
            | "minecraft:gold_ore"
            | "minecraft:lapis_ore"
            | "minecraft:diamond_ore"
            | "minecraft:emerald_ore"
            | "minecraft:deepslate_coal_ore"
            | "minecraft:deepslate_iron_ore"
            | "minecraft:deepslate_copper_ore"
            | "minecraft:deepslate_gold_ore"
            | "minecraft:deepslate_lapis_ore"
            | "minecraft:deepslate_diamond_ore"
            | "minecraft:deepslate_emerald_ore"
            | "minecraft:nether_gold_ore"
            | "minecraft:nether_quartz_ore"
            | "minecraft:ancient_debris"
    )
}

// These vanilla pillars have exactly one axis property, no contained fluid,
// full faces, and no neighbor-shape side effects in Java 26.3.
fn inert_generated_pillar(state: &BlockState) -> bool {
    if state.properties.len() != 1
        || !matches!(
            state.properties.get("axis").map(String::as_str),
            Some("x" | "y" | "z")
        )
    {
        return false;
    }
    let Some(name) = state.name.strip_prefix("minecraft:") else {
        return false;
    };
    if matches!(
        name,
        "deepslate" | "basalt" | "polished_basalt" | "bone_block" | "quartz_pillar"
    ) {
        return true;
    }
    let name = name.strip_prefix("stripped_").unwrap_or(name);
    if name == "bamboo_block" {
        return true;
    }
    if let Some(species) = name
        .strip_suffix("_log")
        .or_else(|| name.strip_suffix("_wood"))
    {
        return matches!(
            species,
            "oak"
                | "spruce"
                | "birch"
                | "jungle"
                | "acacia"
                | "dark_oak"
                | "mangrove"
                | "cherry"
                | "pale_oak"
        );
    }
    name.strip_suffix("_stem")
        .or_else(|| name.strip_suffix("_hyphae"))
        .is_some_and(|species| matches!(species, "crimson" | "warped"))
}

fn leaf_distance(state: &BlockState) -> Result<Option<u8>> {
    if !matches!(
        state.name.as_str(),
        "minecraft:oak_leaves"
            | "minecraft:spruce_leaves"
            | "minecraft:birch_leaves"
            | "minecraft:jungle_leaves"
            | "minecraft:acacia_leaves"
            | "minecraft:dark_oak_leaves"
            | "minecraft:mangrove_leaves"
            | "minecraft:cherry_leaves"
            | "minecraft:pale_oak_leaves"
            | "minecraft:azalea_leaves"
            | "minecraft:flowering_azalea_leaves"
    ) {
        return Ok(None);
    }
    ensure!(
        state.properties.len() == 3
            && matches!(
                state.properties.get("persistent").map(String::as_str),
                Some("true" | "false")
            )
            && matches!(
                state.properties.get("waterlogged").map(String::as_str),
                Some("true" | "false")
            ),
        "invalid generated leaf state"
    );
    let raw = state
        .properties
        .get("distance")
        .context("missing leaf distance")?;
    let distance = raw
        .parse::<u8>()
        .ok()
        .filter(|distance| (1..=7).contains(distance) && distance.to_string() == *raw)
        .context("invalid leaf distance")?;
    Ok(Some(distance))
}

fn supported_postprocessing_block(state: &BlockState) -> Result<DryBlock> {
    if leaf_distance(state)?.is_some() {
        return Ok(DryBlock::Leaves);
    }
    if inert_generated_pillar(state) {
        return Ok(DryBlock::Plain);
    }
    ensure!(
        state.properties.is_empty(),
        "unsupported generated postprocessing block state: {state:?}"
    );
    match state.name.as_str() {
        "minecraft:sand" | "minecraft:red_sand" | "minecraft:gravel" => Ok(DryBlock::Falling),
        "minecraft:brown_mushroom" | "minecraft:red_mushroom" => Ok(DryBlock::Mushroom),
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" => Ok(DryBlock::Plain),
        name if inert_generated_full_cube(name) => Ok(DryBlock::Plain),
        _ => bail!("unsupported generated postprocessing block {}", state.name),
    }
}

fn liquid_state(state: &BlockState) -> Result<Option<native_world::fluid::FluidState>> {
    if !matches!(state.name.as_str(), "minecraft:water" | "minecraft:lava") {
        return Ok(None);
    }
    let raw = state
        .properties
        .get("level")
        .context("liquid level missing")?;
    ensure!(
        state.properties.len() == 1
            && raw
                .parse::<u8>()
                .ok()
                .is_some_and(|level| level <= 15 && level.to_string() == *raw),
        "invalid generated liquid state"
    );
    native_world::fluid::FluidState::from_block(state).map_err(anyhow::Error::msg)
}

fn bonus_water_shape_schedules(state: &BlockState) -> Result<bool> {
    Ok(liquid_state(state)?
        .is_some_and(|fluid| fluid.kind == native_world::fluid::FluidKind::Water && fluid.source))
}

fn bonus_falling_shape_schedules(state: &BlockState) -> bool {
    matches!(
        state.name.as_str(),
        "minecraft:sand" | "minecraft:red_sand" | "minecraft:gravel"
    ) && state.properties.is_empty()
}

impl GeneratedBlockView<'_> {
    fn fence_connection(
        &self,
        state: &BlockState,
        neighbor: &BlockState,
        direction: [i32; 3],
    ) -> Result<bool> {
        use native_resources::packs::ResourceId;
        let tags = self
            .fence_tags
            .context("fence connection tags unavailable")?;
        let fences = ResourceId::parse("minecraft:fences")?;
        let wooden = ResourceId::parse("minecraft:wooden_fences")?;
        let shulkers = ResourceId::parse("minecraft:shulker_boxes")?;
        if tags.contains(&fences, &neighbor.name)?
            && tags.contains(&wooden, &neighbor.name)? == tags.contains(&wooden, &state.name)?
        {
            return Ok(true);
        }
        let gate = neighbor
            .name
            .strip_prefix("minecraft:")
            .and_then(|name| name.strip_suffix("_fence_gate"))
            .is_some_and(|family| {
                native_core::wood::Planks::ALL.into_iter().any(|wood| {
                    wood.id()
                        .strip_prefix("minecraft:")
                        .and_then(|id| id.strip_suffix("_planks"))
                        == Some(family)
                })
            });
        if gate {
            ensure!(
                neighbor.properties.len() == 4
                    && ["in_wall", "open", "powered"].iter().all(|key| matches!(
                        neighbor.properties.get(*key).map(String::as_str),
                        Some("true" | "false")
                    )),
                "invalid fence gate state"
            );
            let gate_x = match neighbor.properties.get("facing").map(String::as_str) {
                Some("east" | "west") => true,
                Some("north" | "south") => false,
                _ => anyhow::bail!("invalid fence gate facing"),
            };
            return Ok(gate_x != (direction[0] != 0));
        }
        let except = leaf_distance(neighbor)?.is_some()
            || matches!(
                neighbor.name.as_str(),
                "minecraft:barrier"
                    | "minecraft:carved_pumpkin"
                    | "minecraft:jack_o_lantern"
                    | "minecraft:melon"
                    | "minecraft:pumpkin"
            )
            || tags.contains(&shulkers, &neighbor.name)?;
        if except {
            return Ok(false);
        }
        let face = glam::IVec3::new(-direction[0], -direction[1], -direction[2]);
        native_world::collision::button_support_face(neighbor, face)
            .with_context(|| format!("unsupported fence neighbor support: {}", neighbor.name))
    }

    fn slab(&self, state: &BlockState) -> Option<(bool, bool)> {
        if !state.name.starts_with("minecraft:")
            || !state.name.ends_with("_slab")
            || self.heightmap_tags?.for_block(&state.name).is_err()
            || native_world::collision::slab_stair_solid(state) != Some(true)
        {
            return None;
        }
        let top = match state.properties.get("type")?.as_str() {
            "top" => true,
            "bottom" => false,
            _ => return None,
        };
        Some((top, state.properties.get("waterlogged")? == "true"))
    }

    fn stair(&self, state: &BlockState) -> Option<(u8, bool)> {
        if !state.name.starts_with("minecraft:")
            || !state.name.ends_with("_stairs")
            || self.heightmap_tags?.for_block(&state.name).is_err()
            || native_world::collision::slab_stair_solid(state) != Some(true)
        {
            return None;
        }
        let shape = native_world::collision::block_shape(state)?;
        Some((
            native_world::fluid_sim::octants(&shape)?,
            state.properties.get("waterlogged")? == "true",
        ))
    }

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
        self.read_chunks.borrow_mut().insert(chunk);
        if self.ticks.chunk(chunk).is_none() {
            return Err(MissingPostprocessingChunk(chunk).into());
        }
        self.world
            .block(pos[0], pos[1], pos[2])
            .cloned()
            .context("generated postprocessing block is missing")
    }

    fn fluid_cell(&self, pos: glam::IVec3) -> native_world::fluid_sim::Cell {
        use native_world::fluid_sim::Cell;
        let Ok(state) = self.read(pos.to_array()) else {
            return Cell::Unknown;
        };
        if native_world::collision::fence_waterlogged(&state) == Some(true) {
            return Cell::FenceWater(native_world::fluid::FluidState {
                kind: native_world::fluid::FluidKind::Water,
                amount: 8,
                source: true,
                falling: false,
            });
        }
        if super::ore::valid_lichen(&state) && state.properties["waterlogged"] == "true" {
            return Cell::FenceWater(native_world::fluid::FluidState {
                kind: native_world::fluid::FluidKind::Water,
                amount: 8,
                source: true,
                falling: false,
            });
        }
        if let Some((top, waterlogged)) = self.slab(&state) {
            return Cell::Slab { top, waterlogged };
        }
        if let Some((voxels, waterlogged)) = self.stair(&state) {
            return Cell::Stairs {
                voxels,
                waterlogged,
            };
        }
        if leaf_distance(&state).ok().flatten().is_some()
            && state
                .properties
                .get("waterlogged")
                .is_some_and(|value| value == "true")
        {
            return Cell::GrateWater(native_world::fluid::FluidState {
                kind: native_world::fluid::FluidKind::Water,
                amount: 8,
                source: true,
                falling: false,
            });
        }
        if matches!(state.name.as_str(), "minecraft:water" | "minecraft:lava") {
            return liquid_state(&state)
                .ok()
                .flatten()
                .map_or(Cell::Unknown, Cell::Fluid);
        }
        if inert_generated_pillar(&state) {
            return Cell::Solid;
        }
        if !state.properties.is_empty() {
            return Cell::Unknown;
        }
        match state.name.as_str() {
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" => Cell::Air,
            "minecraft:sand" | "minecraft:red_sand" | "minecraft:gravel" => Cell::Solid,
            name if inert_generated_full_cube(name) => Cell::Solid,
            _ => Cell::Unknown,
        }
    }

    fn fluid_cell_for_spread(&self, pos: glam::IVec3) -> native_world::fluid_sim::Cell {
        use native_world::{fluid::FluidKind, fluid_sim::Cell};
        let cell = self.fluid_cell(pos);
        match cell {
            Cell::Air | Cell::Fluid(_) => {
                let Ok(state) = self.read(pos.to_array()) else {
                    return Cell::Unknown;
                };
                if !self
                    .washed_blocks
                    .is_some_and(|tags| tags.contains(&state.name))
                {
                    return Cell::Unknown;
                }
                if matches!(cell, Cell::Fluid(fluid) if fluid.kind != FluidKind::Water) {
                    return Cell::Unknown;
                }
                cell
            }
            _ => cell,
        }
    }

    fn tick_generated_fluid(
        &mut self,
        pos: [i32; 3],
        fluid: native_world::fluid::FluidState,
    ) -> Result<()> {
        use native_world::{
            fluid::FluidKind,
            fluid_sim::{Cell, Settings, try_tick},
        };
        // A source enclosed on the bottom and all four sides cannot flow,
        // regardless of gamerules or the dimension's lava speed.
        if fluid.source
            && pos
                .iter()
                .all(|value| value.checked_sub(6).is_some() && value.checked_add(6).is_some())
            && self
                .fluid_cell(glam::IVec3::from_array(pos))
                .contained_fluid()
                == Some(fluid)
            && [[0, -1, 0], [-1, 0, 0], [1, 0, 0], [0, 0, -1], [0, 0, 1]]
                .into_iter()
                .all(|delta| {
                    self.fluid_cell(glam::IVec3::new(
                        pos[0] + delta[0],
                        pos[1] + delta[1],
                        pos[2] + delta[2],
                    )) == Cell::Solid
                })
        {
            return Ok(());
        }
        ensure!(
            !super::ore::valid_lichen(&self.read(pos)?),
            "waterlogged lichen fluid spread is not yet supported"
        );
        // Unknown rules must yield the same plan for both possible values.
        // Water's delay is fixed and never samples lava's delay RNG.
        let mut planned = None;
        for converts in [false, true] {
            if fluid.kind == FluidKind::Water
                && self
                    .water_source_conversion
                    .is_some_and(|rule| rule != converts)
            {
                continue;
            }
            for fast_lava in [false, true] {
                if fluid.kind == FluidKind::Water && fast_lava {
                    continue;
                }
                let settings = Settings {
                    water_source_conversion: converts,
                    lava_source_conversion: converts,
                    fast_lava,
                };
                let changes = try_tick(
                    &|p| self.fluid_cell(p),
                    glam::IVec3::from_array(pos),
                    fluid,
                    settings,
                )
                .map_err(|error| self.fluid_incomplete_error(error, "generated fluid tick"))?;
                if let Some(previous) = &planned {
                    ensure!(
                        *previous == changes,
                        "generated fluid tick depends on unavailable settings"
                    );
                } else {
                    planned = Some(changes);
                }
            }
        }
        let planned = planned.context("fluid settings were not evaluated")?;
        if planned.is_empty() {
            return Ok(());
        }
        use native_world::fluid_sim::ChangedBlock;
        ensure!(
            fluid.kind == FluidKind::Water,
            "generated lava callbacks are unsupported"
        );
        let origin = glam::IVec3::from_array(pos);
        for converts in [false, true] {
            if self
                .water_source_conversion
                .is_some_and(|rule| rule != converts)
            {
                continue;
            }
            let settings = Settings {
                water_source_conversion: converts,
                ..Settings::default()
            };
            let strict = try_tick(&|p| self.fluid_cell_for_spread(p), origin, fluid, settings)
                .map_err(|error| self.fluid_incomplete_error(error, "generated water path"))?;
            ensure!(
                strict == planned,
                "water path differs under active replacement tags"
            );
        }
        for change in &planned {
            let direction = change.position - origin;
            if direction == glam::IVec3::ZERO {
                ensure!(
                    !fluid.source
                        && (matches!(change.block, ChangedBlock::Air)
                            || matches!(change.block, ChangedBlock::Fluid(next)
                            if next.kind == FluidKind::Water && (1..=8).contains(&next.amount)
                                && (!next.source || (next.amount == 8 && !next.falling)))),
                    "unsupported generated water origin update"
                );
                continue;
            }
            let valid_direction = direction == glam::IVec3::NEG_Y
                || [
                    glam::IVec3::NEG_X,
                    glam::IVec3::X,
                    glam::IVec3::NEG_Z,
                    glam::IVec3::Z,
                ]
                .contains(&direction);
            let valid_target = match change.block {
                ChangedBlock::Fluid(next) => {
                    matches!(self.fluid_cell_for_spread(change.position), Cell::Air)
                        && next.kind == FluidKind::Water
                        && ((direction == glam::IVec3::NEG_Y
                            && !next.source
                            && next.falling
                            && next.amount == 8)
                            || (direction != glam::IVec3::NEG_Y
                                && !next.falling
                                && ((next.source && next.amount == 8)
                                    || (!next.source && (1..=7).contains(&next.amount)))))
                }
                ChangedBlock::WaterloggedSlab { top } => matches!(
                    self.fluid_cell_for_spread(change.position),
                    Cell::Slab { top: actual, waterlogged: false } if actual == top
                ),
                ChangedBlock::WaterloggedStairs { voxels } => matches!(
                    self.fluid_cell_for_spread(change.position),
                    Cell::Stairs { voxels: actual, waterlogged: false } if actual == voxels
                ),
                _ => false,
            };
            ensure!(
                valid_direction && valid_target,
                "generated fluid tick requires unsupported block callbacks"
            );
        }
        let settings = Settings {
            water_source_conversion: self.water_source_conversion.unwrap_or(true),
            ..Settings::default()
        };
        let mut live = GeneratedWaterTick {
            view: self,
            origin: glam::IVec3::from_array(pos),
        };
        let result = native_world::fluid_sim::tick_live(
            &mut live,
            glam::IVec3::from_array(pos),
            fluid,
            settings,
        );
        match result {
            Ok(()) => {}
            Err(native_world::fluid_sim::LiveTickError::World(error)) => {
                return Err(error.context("generated water tick failed"));
            }
            Err(native_world::fluid_sim::LiveTickError::Incomplete(error)) => {
                return Err(live
                    .view
                    .fluid_incomplete_error(error, "generated water tick"));
            }
        }
        Ok(())
    }

    fn fluid_incomplete_error(
        &self,
        error: native_world::fluid_sim::IncompleteTick,
        context: &str,
    ) -> anyhow::Error {
        if let native_world::fluid_sim::IncompleteTick::Unknown(pos) = error {
            if let Err(cause) = self.read(pos.to_array()) {
                return cause.context(context.to_owned());
            }
        }
        anyhow::anyhow!("{context} is incomplete: {error:?}")
    }

    fn schedule_water(&mut self, pos: [i32; 3], state: &BlockState) -> Result<()> {
        let fluid = liquid_state(state)?.context("water callback has no fluid")?;
        ensure!(
            fluid.kind == native_world::fluid::FluidKind::Water,
            "unsupported fluid callback"
        );
        let id = if fluid.source {
            "minecraft:water"
        } else {
            "minecraft:flowing_water"
        };
        ensure!(
            self.ticks.schedule(TickRegistry::Fluid, id, pos, 5, 0)?
                != ScheduleOutcome::MissingChunk,
            "water tick container is missing"
        );
        Ok(())
    }

    fn schedule_water_bubble(&mut self, pos: [i32; 3], state: &BlockState) -> Result<()> {
        let fluid = liquid_state(state)?.context("water callback has no fluid")?;
        let tags = self.bubble_tags.context("bubble-column tags unavailable")?;
        if fluid.source && tags.can_occupy_fluid("minecraft:water") {
            let support = self.read([pos[0], pos[1].wrapping_sub(1), pos[2]])?;
            if tags.enables(&support.name) {
                ensure!(
                    self.ticks
                        .schedule(TickRegistry::Block, "minecraft:water", pos, 20, 0)?
                        != ScheduleOutcome::MissingChunk,
                    "bubble tick container is missing"
                );
            }
        }
        Ok(())
    }

    fn spread_water(
        &mut self,
        target: glam::IVec3,
        fluid: native_world::fluid::FluidState,
    ) -> Result<()> {
        use native_world::fluid_sim::Cell;
        let old = self.read(target.to_array())?;
        if matches!(
            self.fluid_cell(target),
            Cell::Slab {
                waterlogged: false,
                ..
            } | Cell::Stairs {
                waterlogged: false,
                ..
            }
        ) {
            ensure!(
                fluid.source && !fluid.falling,
                "block requires source water"
            );
            let mut next = old.clone();
            next.properties.insert("waterlogged".into(), "true".into());
            self.apply_water_change(target, &old, &next)?;
            self.schedule_contained_water(target.to_array())?;
            return Ok(());
        }
        ensure!(
            matches!(self.fluid_cell(target), Cell::Air),
            "water target is not air"
        );
        ensure!(
            self.washed_blocks
                .context("fluid replacement tags unavailable")?
                .contains(&old.name),
            "water target is not tagged as washable"
        );
        let next = BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([(
                "level".into(),
                (8 - fluid.amount + if fluid.falling { 8 } else { 0 }).to_string(),
            )]),
        };
        self.apply_water_change(target, &old, &next)
    }

    fn schedule_contained_water(&mut self, pos: [i32; 3]) -> Result<()> {
        ensure!(
            self.ticks
                .schedule(TickRegistry::Fluid, "minecraft:water", pos, 5, 0)?
                != ScheduleOutcome::MissingChunk,
            "contained water tick container is missing"
        );
        Ok(())
    }

    fn apply_water_change(
        &mut self,
        target: glam::IVec3,
        old: &BlockState,
        next: &BlockState,
    ) -> Result<()> {
        use native_world::fluid_sim::Cell;
        let lava_tags = self.lava_fluids.context("lava fluid tags unavailable")?;
        ensure!(
            !lava_tags.contains("minecraft:water")
                && !lava_tags.contains("minecraft:flowing_water"),
            "water uses unsupported lava callbacks"
        );
        let next_fluid = liquid_state(next)?;
        let next_slab = self.slab(next);
        let next_stair = self.stair(next);
        let contained_source =
            next_slab.is_some_and(|(_, wet)| wet) || next_stair.is_some_and(|(_, wet)| wet);
        ensure!(
            next_fluid.is_some_and(|fluid| fluid.kind == native_world::fluid::FluidKind::Water)
                || (next.name == "minecraft:air" && next.properties.is_empty())
                || contained_source,
            "unsupported water replacement state"
        );
        let old_air = matches!(
            old.name.as_str(),
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
        );
        ensure!(
            old_air
                || liquid_state(old)?
                    .is_some_and(|fluid| fluid.kind == native_world::fluid::FluidKind::Water)
                || (self.slab(old).is_some_and(|(_, wet)| !wet)
                    && next_slab.is_some_and(|(_, wet)| wet))
                || (self.stair(old).is_some_and(|(_, wet)| !wet)
                    && next_stair.is_some_and(|(_, wet)| wet)),
            "unsupported original water state"
        );
        let next_air = next.name == "minecraft:air";
        let chunk_pos = ChunkPos {
            x: target.x.div_euclid(16),
            z: target.z.div_euclid(16),
        };
        let section_y =
            i8::try_from(target.y.div_euclid(16)).context("water section out of range")?;
        let blocks = self
            .world
            .chunks
            .get(&chunk_pos)
            .and_then(|chunk| chunk.sections.get(&section_y))
            .and_then(|section| section.blocks.as_ref())
            .context("water section unavailable")?;
        if old_air != next_air {
            let mut nonempty_count = 0;
            for id in blocks.iter() {
                let state = self
                    .world
                    .state(*id)
                    .context("water section state missing")?;
                if !matches!(
                    state.name.as_str(),
                    "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                ) {
                    nonempty_count += 1;
                    if nonempty_count >= if old_air { 1 } else { 2 } {
                        break;
                    }
                }
            }
            ensure!(
                nonempty_count >= if old_air { 1 } else { 2 },
                "water change requires light section allocation update"
            );
        }
        let directions = [
            [-1, 0, 0],
            [1, 0, 0],
            [0, -1, 0],
            [0, 1, 0],
            [0, 0, -1],
            [0, 0, 1],
        ];
        for d in directions {
            let p = target + glam::IVec3::from_array(d);
            let state = self.read(p.to_array())?;
            ensure!(
                matches!(
                    self.fluid_cell(p),
                    Cell::Air
                        | Cell::Solid
                        | Cell::Fluid(_)
                        | Cell::FenceWater(_)
                        | Cell::Slab { .. }
                        | Cell::Stairs { .. }
                ),
                "unsupported water neighbor callback"
            );
            if let Some(fluid) = liquid_state(&state)? {
                ensure!(
                    fluid.kind == native_world::fluid::FluidKind::Water,
                    "water touches unsupported liquid"
                );
            }
        }
        self.write_postprocessed_state(target.to_array(), old, next)?;
        if let Some(fluid) = next_fluid {
            self.schedule_water(target.to_array(), next)?;
            if fluid.source {
                self.schedule_water_bubble(target.to_array(), next)?;
            }
        }
        // Redstone neighbor callbacks precede shape updates.
        for d in directions {
            let p = target + glam::IVec3::from_array(d);
            let state = self.read(p.to_array())?;
            if liquid_state(&state)?.is_some() {
                self.schedule_water(p.to_array(), &state)?;
                self.schedule_water_bubble(p.to_array(), &state)?;
            }
        }
        for d in [
            [-1, 0, 0],
            [1, 0, 0],
            [0, 0, -1],
            [0, 0, 1],
            [0, -1, 0],
            [0, 1, 0],
        ] {
            let p = target + glam::IVec3::from_array(d);
            let state = self.read(p.to_array())?;
            if let Some(neighbor_fluid) = liquid_state(&state)? {
                if neighbor_fluid.source
                    || contained_source
                    || next_fluid.is_some_and(|fluid| fluid.source)
                {
                    self.schedule_water(p.to_array(), &state)?;
                }
                if neighbor_fluid.source {
                    let tags = self.bubble_tags.context("bubble-column tags unavailable")?;
                    if d == [0, 1, 0]
                        && tags.can_occupy_fluid("minecraft:water")
                        && tags.enables(&next.name)
                    {
                        ensure!(
                            self.ticks.schedule(
                                TickRegistry::Block,
                                "minecraft:water",
                                p.to_array(),
                                20,
                                0
                            )? != ScheduleOutcome::MissingChunk,
                            "bubble tick container is missing"
                        );
                    }
                }
            } else if native_world::collision::fence_waterlogged(&state) == Some(true) {
                let direction = [-d[0], -d[1], -d[2]];
                let updated =
                    self.update_shape(p.to_array(), &state, direction, target.to_array(), next)?;
                if updated != state {
                    self.write_postprocessed_state(p.to_array(), &state, &updated)?;
                    self.update_fence_shape_neighbors(p.to_array(), &updated, 512)?;
                }
            } else if self.slab(&state).is_some_and(|(_, wet)| wet)
                || self.stair(&state).is_some_and(|(_, wet)| wet)
            {
                self.schedule_contained_water(p.to_array())?;
            } else if matches!(
                state.name.as_str(),
                "minecraft:sand" | "minecraft:red_sand" | "minecraft:gravel"
            ) {
                ensure!(
                    self.ticks
                        .schedule(TickRegistry::Block, &state.name, p.to_array(), 2, 0)?
                        != ScheduleOutcome::MissingChunk,
                    "falling tick container is missing"
                );
            }
        }
        Ok(())
    }

    /// A shape-update write has Java's flag 2: shape notifications, but no
    /// redstone neighbor notifications. Each nested write runs before the
    /// remaining shape callbacks of the enclosing update.
    fn update_fence_shape_neighbors(
        &mut self,
        pos: [i32; 3],
        fence: &BlockState,
        depth: usize,
    ) -> Result<()> {
        use native_world::fluid_sim::Cell;
        ensure!(depth > 0, "fence shape update depth exceeded");
        for d in [
            [-1, 0, 0],
            [1, 0, 0],
            [0, 0, -1],
            [0, 0, 1],
            [0, -1, 0],
            [0, 1, 0],
        ] {
            let neighbor = [
                pos[0].wrapping_add(d[0]),
                pos[1].wrapping_add(d[1]),
                pos[2].wrapping_add(d[2]),
            ];
            let state = self.read(neighbor)?;
            let cell = self.fluid_cell(glam::IVec3::from_array(neighbor));
            ensure!(
                matches!(
                    cell,
                    Cell::Air
                        | Cell::Solid
                        | Cell::Fluid(_)
                        | Cell::FenceWater(_)
                        | Cell::Slab { .. }
                        | Cell::Stairs { .. }
                ),
                "unsupported fence neighbor shape callback: {}",
                state.name
            );
            if let Some(fluid) = liquid_state(&state)? {
                ensure!(
                    fluid.kind == native_world::fluid::FluidKind::Water,
                    "fence touches unsupported liquid"
                );
                self.schedule_water(neighbor, &state)?;
                if fluid.source && d == [0, 1, 0] {
                    let tags = self.bubble_tags.context("bubble-column tags unavailable")?;
                    if tags.can_occupy_fluid("minecraft:water") && tags.enables(&fence.name) {
                        ensure!(
                            self.ticks.schedule(
                                TickRegistry::Block,
                                "minecraft:water",
                                neighbor,
                                20,
                                0
                            )? != ScheduleOutcome::MissingChunk,
                            "bubble tick container is missing"
                        );
                    }
                }
            } else if native_world::collision::fence_waterlogged(&state) == Some(true) {
                let next =
                    self.update_shape(neighbor, &state, [-d[0], -d[1], -d[2]], pos, fence)?;
                if next != state {
                    self.write_postprocessed_state(neighbor, &state, &next)?;
                    self.update_fence_shape_neighbors(neighbor, &next, depth - 1)?;
                }
            } else if self.slab(&state).is_some_and(|(_, wet)| wet)
                || self.stair(&state).is_some_and(|(_, wet)| wet)
            {
                self.schedule_contained_water(neighbor)?;
            } else if matches!(
                state.name.as_str(),
                "minecraft:sand" | "minecraft:red_sand" | "minecraft:gravel"
            ) {
                ensure!(
                    self.ticks
                        .schedule(TickRegistry::Block, &state.name, neighbor, 2, 0)?
                        != ScheduleOutcome::MissingChunk,
                    "falling tick container is missing"
                );
            }
        }
        Ok(())
    }

    fn mushroom_survives(&self, pos: [i32; 3]) -> Result<bool> {
        let below = self.read([pos[0], pos[1].wrapping_sub(1), pos[2]])?;
        static TAG: std::sync::OnceLock<native_resources::packs::ResourceId> =
            std::sync::OnceLock::new();
        let tag = TAG.get_or_init(|| {
            native_resources::packs::ResourceId::parse(
                "minecraft:overrides_mushroom_light_requirement",
            )
            .expect("static tag identifier")
        });
        if self
            .mushroom_tags
            .context("mushroom support tags unavailable")?
            .contains(tag, &below.name)?
        {
            return Ok(true);
        }
        self.light_reads.borrow_mut().insert(ChunkPos {
            x: pos[0].div_euclid(16),
            z: pos[2].div_euclid(16),
        });
        let brightness = self
            .light
            .context("mushroom lighting unavailable")?
            .raw_brightness(pos, 0)
            .context("mushroom light column unavailable")?;
        if brightness >= 13 {
            return Ok(false);
        }
        if matches!(
            below.name.as_str(),
            "minecraft:grass_block" | "minecraft:podzol" | "minecraft:mycelium"
        ) {
            ensure!(
                below.properties.len() == 1
                    && matches!(
                        below.properties.get("snowy").map(String::as_str),
                        Some("true" | "false")
                    ),
                "invalid mushroom support state"
            );
            return Ok(true);
        }
        if matches!(
            below.name.as_str(),
            "minecraft:crimson_nylium" | "minecraft:warped_nylium"
        ) {
            ensure!(
                below.properties.is_empty(),
                "invalid mushroom support state"
            );
            return Ok(true);
        }
        Ok(super::ore::monster_block_flags(&below)?.1)
    }

    fn replace_postprocessed_shape(
        &mut self,
        pos: [i32; 3],
        next: &BlockState,
        flags: i32,
    ) -> Result<()> {
        let old = self.read(pos)?;
        if super::ore::valid_lichen(&old) {
            let faces = ["down", "up", "north", "south", "west", "east"];
            let valid = if super::ore::valid_lichen(next) {
                old.properties["waterlogged"] == next.properties["waterlogged"]
                    && faces.iter().all(|face| {
                        old.properties[*face] == "true" || next.properties[*face] == "false"
                    })
                    && faces.iter().any(|face| next.properties[*face] == "true")
            } else {
                next.name == "minecraft:air" && next.properties.is_empty()
            };
            ensure!(flags == 276 && valid, "unsupported generated lichen write");
            return self.write_postprocessed_state(pos, &old, next);
        }
        if native_world::collision::fence_waterlogged(&old).is_some()
            && native_world::collision::fence_waterlogged(next).is_some()
        {
            let mut expected = old.clone();
            for side in ["north", "east", "south", "west"] {
                expected
                    .properties
                    .insert(side.into(), next.properties[side].clone());
            }
            ensure!(
                flags == 276 && *next == expected,
                "unsupported generated fence write"
            );
            return self.write_postprocessed_state(pos, &old, next);
        }
        if self.stair(&old).is_some() && self.stair(next).is_some() {
            let mut expected = old.clone();
            expected
                .properties
                .insert("shape".into(), next.properties["shape"].clone());
            ensure!(
                flags == 276 && *next == expected,
                "unsupported generated stair write"
            );
            // LevelChunk queues lighting checks. The batch drains them after
            // postprocessing at an explicitly chosen light-engine boundary.
            return self.write_postprocessed_state(pos, &old, next);
        }
        ensure!(
            flags == 276 && next.name == "minecraft:air" && next.properties.is_empty(),
            "unsupported generated postprocessing write"
        );
        ensure!(
            matches!(supported_postprocessing_block(&old)?, DryBlock::Mushroom),
            "postprocessing source is not a mushroom"
        );
        let same_light = native_world::block_light::same_light_behavior(&old, next)
            .context("mushroom light behavior unavailable")?;
        ensure!(
            native_world::sky_light::same_sky_behavior(&old, next) == Some(true),
            "mushroom removal changes direct sky sources"
        );
        if !same_light {
            let change = native_world::block_light::light_change(&old, next)
                .context("mushroom light change unavailable")?;
            ensure!(
                change.radius() == 0,
                "mushroom light change requires propagation during postprocessing"
            );
        }
        self.write_postprocessed_state(pos, &old, next)
    }

    fn write_postprocessed_state(
        &mut self,
        pos: [i32; 3],
        old: &BlockState,
        next: &BlockState,
    ) -> Result<()> {
        let same_light = native_world::block_light::same_light_behavior(old, next)
            .context("postprocessing light behavior unavailable")?;
        let world = Arc::make_mut(self.world);
        let id = world.intern_state(next.clone());
        ensure!(
            world
                .set_block_id(pos[0], pos[1], pos[2], id)
                .map_err(|error| anyhow::anyhow!("cannot update generated block: {error:?}"))?,
            "generated block write was unchanged"
        );
        let tags = self
            .heightmap_tags
            .context("generated heightmap tags unavailable")?;
        let chunk = ChunkPos {
            x: pos[0].div_euclid(16),
            z: pos[2].div_euclid(16),
        };
        let maps = self
            .heightmaps
            .get_mut(&chunk)
            .context("generated FULL heightmaps unavailable")?;
        let base_x = pos[0] - pos[0].rem_euclid(16);
        let base_z = pos[2] - pos[2].rem_euclid(16);
        maps.update_after_change(
            pos[0],
            pos[1],
            pos[2],
            crate::spawn_heightmaps::classify(next, tags)?,
            |x, y, z| {
                let block = world
                    .block(base_x + x, y, base_z + z)
                    .context("heightmap scan block unavailable")?;
                crate::spawn_heightmaps::classify(block, tags)
            },
        )?;
        let position = BlockPosition {
            x: pos[0],
            y: pos[1],
            z: pos[2],
        };
        self.edits.push(position);
        if !same_light {
            self.light_checks.push(position);
        }
        Ok(())
    }
}

struct GeneratedWaterTick<'a, 'b> {
    view: &'a mut GeneratedBlockView<'b>,
    origin: glam::IVec3,
}

impl native_world::fluid_sim::LiveFluidWorld for GeneratedWaterTick<'_, '_> {
    type Error = anyhow::Error;

    fn cell(&self, pos: glam::IVec3) -> native_world::fluid_sim::Cell {
        self.view.fluid_cell_for_spread(pos)
    }

    fn delay(
        &mut self,
        _: glam::IVec3,
        old: native_world::fluid::FluidState,
        _: Option<native_world::fluid::FluidState>,
    ) -> Result<u64> {
        ensure!(
            old.kind == native_world::fluid::FluidKind::Water,
            "unsupported fluid delay"
        );
        Ok(5)
    }

    fn update_origin(
        &mut self,
        pos: glam::IVec3,
        next: Option<native_world::fluid::FluidState>,
    ) -> Result<()> {
        ensure!(pos == self.origin, "water origin changed unexpectedly");
        let old = self.view.read(pos.to_array())?;
        ensure!(
            liquid_state(&old)?.is_some_and(|fluid| fluid.kind
                == native_world::fluid::FluidKind::Water
                && !fluid.source),
            "water origin is no longer flowing"
        );
        let replacement = if let Some(fluid) = next {
            ensure!(
                fluid.kind == native_world::fluid::FluidKind::Water
                    && (1..=8).contains(&fluid.amount)
                    && (!fluid.source || (fluid.amount == 8 && !fluid.falling)),
                "invalid recalculated water"
            );
            BlockState {
                name: "minecraft:water".into(),
                properties: BTreeMap::from([(
                    "level".into(),
                    (8 - fluid.amount + if fluid.falling { 8 } else { 0 }).to_string(),
                )]),
            }
        } else {
            BlockState {
                name: "minecraft:air".into(),
                properties: BTreeMap::new(),
            }
        };
        self.view.apply_water_change(pos, &old, &replacement)
    }

    fn schedule(
        &mut self,
        pos: glam::IVec3,
        fluid: native_world::fluid::FluidState,
        delay: u64,
    ) -> Result<()> {
        ensure!(
            pos == self.origin && delay == 5,
            "unsupported flowing water schedule"
        );
        let current = self.view.read(pos.to_array())?;
        ensure!(
            liquid_state(&current)? == Some(fluid),
            "flowing water changed before scheduling"
        );
        self.view.schedule_water(pos.to_array(), &current)
    }

    fn spread_to(
        &mut self,
        pos: glam::IVec3,
        old: native_world::fluid_sim::Cell,
        fluid: native_world::fluid::FluidState,
        direction: glam::IVec3,
    ) -> Result<()> {
        use native_world::{fluid::FluidKind, fluid_sim::Cell};
        let horizontal = [
            glam::IVec3::NEG_X,
            glam::IVec3::X,
            glam::IVec3::NEG_Z,
            glam::IVec3::Z,
        ]
        .contains(&direction);
        ensure!(
            (old == Cell::Air
                || matches!(
                    old,
                    Cell::Slab {
                        waterlogged: false,
                        ..
                    } | Cell::Stairs {
                        waterlogged: false,
                        ..
                    }
                ) && fluid.source)
                && pos == self.origin + direction
                && fluid.kind == FluidKind::Water
                && ((direction == glam::IVec3::NEG_Y
                    && !fluid.source
                    && fluid.falling
                    && fluid.amount == 8)
                    || (horizontal
                        && !fluid.falling
                        && ((fluid.source && fluid.amount == 8)
                            || (!fluid.source && (1..=7).contains(&fluid.amount))))),
            "unsupported generated water spread"
        );
        self.view.spread_water(pos, fluid)
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
        direction: [i32; 3],
        _: [i32; 3],
        neighbor: &BlockState,
    ) -> Result<BlockState> {
        if super::ore::valid_lichen(state) {
            if state.properties["waterlogged"] == "true" {
                self.schedule_contained_water(pos)?;
            }
            let faces = ["down", "up", "north", "south", "west", "east"];
            if !faces.iter().any(|face| state.properties[*face] == "true") {
                return Ok(BlockState {
                    name: "minecraft:air".into(),
                    properties: Default::default(),
                });
            }
            let face = match direction {
                [-1, 0, 0] => "west",
                [1, 0, 0] => "east",
                [0, 0, -1] => "north",
                [0, 0, 1] => "south",
                [0, -1, 0] => "down",
                [0, 1, 0] => "up",
                _ => anyhow::bail!("invalid lichen update direction"),
            };
            if state.properties[face] == "false" || super::ore::multiface_support(neighbor)? {
                return Ok(state.clone());
            }
            let mut updated = state.clone();
            updated.properties.insert(face.into(), "false".into());
            return if faces.iter().any(|face| updated.properties[*face] == "true") {
                Ok(updated)
            } else {
                Ok(BlockState {
                    name: "minecraft:air".into(),
                    properties: Default::default(),
                })
            };
        }
        if let Some(waterlogged) = native_world::collision::fence_waterlogged(state) {
            if waterlogged {
                self.schedule_contained_water(pos)?;
            }
            let side = match direction {
                [-1, 0, 0] => Some("west"),
                [1, 0, 0] => Some("east"),
                [0, 0, -1] => Some("north"),
                [0, 0, 1] => Some("south"),
                [0, -1, 0] | [0, 1, 0] => None,
                _ => anyhow::bail!("invalid fence update direction"),
            };
            if let Some(side) = side {
                let mut updated = state.clone();
                updated.properties.insert(
                    side.into(),
                    self.fence_connection(state, neighbor, direction)?
                        .to_string(),
                );
                return Ok(updated);
            }
            return Ok(state.clone());
        }
        if let Some((_, waterlogged)) = self.slab(state) {
            if waterlogged {
                self.schedule_contained_water(pos)?;
            }
            return Ok(state.clone());
        }
        if let Some((_, waterlogged)) = self.stair(state) {
            if waterlogged {
                self.schedule_contained_water(pos)?;
            }
            if direction[1] != 0 {
                return Ok(state.clone());
            }
            let stair = native_world::stairs::Stair::from_state(state)
                .context("invalid generated stair")?;
            let base = glam::IVec3::from_array(pos);
            let shape = native_world::stairs::shape(stair, |offset| {
                let Some(x) = base.x.checked_add(offset.x) else {
                    return native_world::stairs::Neighbor::Unknown;
                };
                let Some(z) = base.z.checked_add(offset.z) else {
                    return native_world::stairs::Neighbor::Unknown;
                };
                let Ok(other) = self.read([x, base.y, z]) else {
                    return native_world::stairs::Neighbor::Unknown;
                };
                if let Some((_, _)) = self.stair(&other) {
                    native_world::stairs::Stair::from_state(&other).map_or(
                        native_world::stairs::Neighbor::Unknown,
                        native_world::stairs::Neighbor::Stair,
                    )
                } else if other.name.ends_with("_stairs") {
                    native_world::stairs::Neighbor::Unknown
                } else {
                    native_world::stairs::Neighbor::Other
                }
            })
            .context("stair shape needs unavailable neighbors")?;
            let mut updated = state.clone();
            updated.properties.insert("shape".into(), shape.into());
            return Ok(updated);
        }
        match supported_postprocessing_block(state)? {
            DryBlock::Falling => {
                ensure!(
                    self.ticks
                        .schedule(TickRegistry::Block, &state.name, pos, 2, 0)?
                        != ScheduleOutcome::MissingChunk,
                    "falling block tick container is missing"
                );
            }
            DryBlock::Mushroom if !self.mushroom_survives(pos)? => {
                return Ok(BlockState {
                    name: "minecraft:air".into(),
                    properties: Default::default(),
                });
            }
            DryBlock::Leaves => {
                if state.properties["waterlogged"] == "true" {
                    self.schedule_contained_water(pos)?;
                }
                static TAG: std::sync::OnceLock<native_resources::packs::ResourceId> =
                    std::sync::OnceLock::new();
                let tag = TAG.get_or_init(|| {
                    native_resources::packs::ResourceId::parse(
                        "minecraft:prevents_nearby_leaf_decay",
                    )
                    .expect("static tag identifier")
                });
                let tagged = self
                    .leaf_tags
                    .context("leaf support tags unavailable")?
                    .contains(tag, &neighbor.name)?;
                // A non-tagged neighbor's leaf distance is at least one;
                // only a tagged neighbor can produce distance zero.
                if !tagged || state.properties["distance"] != "1" {
                    ensure!(
                        self.ticks
                            .schedule(TickRegistry::Block, &state.name, pos, 1, 0)?
                            != ScheduleOutcome::MissingChunk,
                        "leaf tick container is missing"
                    );
                }
            }
            DryBlock::Plain | DryBlock::Mushroom => {}
        }
        Ok(state.clone())
    }

    fn set_block(&mut self, pos: [i32; 3], state: &BlockState, flags: i32) -> Result<()> {
        self.replace_postprocessed_shape(pos, state, flags)
    }
}

impl PostprocessingWorld for GeneratedBlockView<'_> {
    type State = BlockState;
    type Fluid = native_world::fluid::FluidState;

    fn block(&mut self, pos: [i32; 3]) -> Result<BlockState> {
        self.read(pos)
    }
    fn fluid(&mut self, state: &BlockState) -> Result<Option<Self::Fluid>> {
        if super::ore::valid_lichen(state) {
            return Ok((state.properties["waterlogged"] == "true").then_some(
                native_world::fluid::FluidState {
                    kind: native_world::fluid::FluidKind::Water,
                    amount: 8,
                    source: true,
                    falling: false,
                },
            ));
        }
        if let Some(fluid) = liquid_state(state)? {
            return Ok(Some(fluid));
        }
        if let Some(waterlogged) = native_world::collision::fence_waterlogged(state) {
            return Ok(waterlogged.then_some(native_world::fluid::FluidState {
                kind: native_world::fluid::FluidKind::Water,
                amount: 8,
                source: true,
                falling: false,
            }));
        }
        if let Some((top, waterlogged)) = self.slab(state) {
            return Ok(native_world::fluid_sim::Cell::Slab { top, waterlogged }.contained_fluid());
        }
        if let Some((voxels, waterlogged)) = self.stair(state) {
            return Ok(native_world::fluid_sim::Cell::Stairs {
                voxels,
                waterlogged,
            }
            .contained_fluid());
        }
        if leaf_distance(state)?.is_some() && state.properties["waterlogged"] == "true" {
            return Ok(Some(native_world::fluid::FluidState {
                kind: native_world::fluid::FluidKind::Water,
                amount: 8,
                source: true,
                falling: false,
            }));
        }
        supported_postprocessing_block(state)?;
        Ok(None)
    }
    fn tick_fluid(
        &mut self,
        pos: [i32; 3],
        _state: &BlockState,
        fluid: &Self::Fluid,
    ) -> Result<()> {
        self.tick_generated_fluid(pos, *fluid)
    }
    fn is_liquid_block(&mut self, state: &BlockState) -> Result<bool> {
        Ok(liquid_state(state)?.is_some())
    }
    fn tick_liquid_block(&mut self, pos: [i32; 3], state: &BlockState) -> Result<()> {
        let tags = self.bubble_tags.context("bubble-column tags unavailable")?;
        crate::bubble_column::tick_liquid_block(self, glam::IVec3::from_array(pos), state, tags)
    }
    fn update_neighbor_shapes(&mut self, pos: [i32; 3], state: &BlockState) -> Result<BlockState> {
        update_from_neighbor_shapes(self, pos, state)
    }
    fn set_block(&mut self, pos: [i32; 3], state: &BlockState, flags: i32) -> Result<()> {
        self.replace_postprocessed_shape(pos, state, flags)
    }
    fn clear_processed_section(&mut self, _: usize) -> Result<()> {
        Ok(())
    }
}

impl crate::bubble_column::ColumnWorld for GeneratedBlockView<'_> {
    fn block(&mut self, pos: glam::IVec3) -> Result<BlockState> {
        self.read(pos.to_array())
    }

    fn set_block(&mut self, pos: glam::IVec3, state: &BlockState) -> Result<bool> {
        ensure!(
            self.read(pos.to_array())? == *state,
            "bubble-column update requires block callbacks"
        );
        Ok(false)
    }
}

impl PostprocessingChunk for GeneratedBlockView<'_> {
    fn pending_block_entity_positions(&mut self) -> Result<Vec<[i32; 3]>> {
        self.block_entities.pending.positions()
    }
    fn promote_pending_block_entity(&mut self, pos: [i32; 3]) -> Result<()> {
        ensure!(
            pos[0].div_euclid(16) == self.chunk.x && pos[2].div_euclid(16) == self.chunk.z,
            "pending block entity belongs to another chunk"
        );
        ensure!(
            self.block_entities.pending.members().contains(&pos),
            "pending block entity is missing"
        );
        let state = self.read(pos)?;
        ensure!(
            state.name == "minecraft:chest",
            "unsupported pending generated block entity"
        );
        let entity = super::GeneratedEmptyChest {
            position: pos,
            state,
        };
        self.block_entities.pending.remove(pos);
        self.block_entities
            .records
            .insert(pos, entity.block_entity());
        self.block_entities.live.insert(pos);
        Ok(())
    }
    fn clear_pending_block_entities(&mut self) -> Result<()> {
        ensure!(
            self.block_entities.pending.members().is_empty(),
            "pending generated block entities remain"
        );
        Ok(())
    }
    fn apply_upgrade_data(&mut self) -> Result<()> {
        Ok(())
    }
}

impl native_resources::bonus_chest::BonusChestWorld for GeneratedBlockView<'_> {
    fn motion_blocking_no_leaves(&mut self, x: i32, z: i32) -> Result<[i32; 3]> {
        let chunk = ChunkPos {
            x: x.div_euclid(16),
            z: z.div_euclid(16),
        };
        let map = self
            .heightmaps
            .get(&chunk)
            .context("bonus chest heightmap is missing")?;
        Ok([x, map.motion_blocking_no_leaves(x, z), z])
    }

    fn is_empty_block(&mut self, pos: [i32; 3]) -> Result<bool> {
        let state = self.read(pos)?;
        Ok(matches!(
            state.name.as_str(),
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
        ))
    }

    fn collision_shape_empty(&mut self, pos: [i32; 3]) -> Result<bool> {
        let state = self.read(pos)?;
        Ok(native_world::collision::block_shape(&state)
            .context("bonus chest collision shape is unavailable")?
            .is_empty())
    }

    fn set_chest(&mut self, pos: [i32; 3]) -> Result<()> {
        ensure!(
            pos[0].div_euclid(16) == self.chunk.x && pos[2].div_euclid(16) == self.chunk.z,
            "bonus chest is outside its spawn chunk"
        );
        let old = self.read(pos)?;
        ensure!(
            matches!(old.name.as_str(), "minecraft:air" | "minecraft:cave_air"),
            "bonus chest replacement effects are unsupported"
        );
        ensure!(
            !self.block_entities.records.contains_key(&pos),
            "bonus chest entity already exists"
        );
        let state = BlockState {
            name: "minecraft:chest".into(),
            properties: BTreeMap::from([
                ("facing".into(), "north".into()),
                ("type".into(), "single".into()),
                ("waterlogged".into(), "false".into()),
            ]),
        };
        self.check_bonus_neighbors(pos, false)?;
        self.write_postprocessed_state(pos, &old, &state)?;
        let entity = super::GeneratedEmptyChest {
            position: pos,
            state: state.clone(),
        };
        self.block_entities
            .records
            .insert(pos, entity.block_entity());
        self.block_entities.live.insert(pos);
        self.block_entity_edits.insert(
            BlockPosition {
                x: pos[0],
                y: pos[1],
                z: pos[2],
            },
            (old, state),
        );
        self.update_bonus_neighbors(pos)?;
        Ok(())
    }

    fn has_randomizable_container(&mut self, pos: [i32; 3]) -> Result<bool> {
        Ok(self.block_entities.records.contains_key(&pos))
    }

    fn set_bonus_chest_loot(&mut self, pos: [i32; 3], seed: i64) -> Result<()> {
        ensure!(
            self.block_entities.records.contains_key(&pos),
            "bonus chest entity is missing"
        );
        let chest = super::GeneratedLootChest::new(
            pos,
            native_resources::structure_start::HorizontalDirection::North,
            native_resources::bonus_chest::BONUS_CHEST_LOOT_TABLE,
            seed,
        )?;
        self.block_entities
            .records
            .insert(pos, chest.block_entity());
        Ok(())
    }

    fn torch_can_survive(&mut self, pos: [i32; 3]) -> Result<bool> {
        let below = self.read([pos[0], pos[1].wrapping_sub(1), pos[2]])?;
        if matches!(
            below.name.as_str(),
            "minecraft:air"
                | "minecraft:cave_air"
                | "minecraft:void_air"
                | "minecraft:water"
                | "minecraft:lava"
        ) {
            return Ok(false);
        }
        ensure!(
            native_world::collision::button_support_face(&below, glam::IVec3::Y) == Some(true),
            "bonus torch center support is unavailable"
        );
        Ok(true)
    }

    fn set_torch(&mut self, pos: [i32; 3]) -> Result<()> {
        let old = self.read(pos)?;
        let torch = BlockState {
            name: "minecraft:torch".into(),
            properties: BTreeMap::new(),
        };
        if old == torch {
            return Ok(());
        }
        ensure!(
            matches!(old.name.as_str(), "minecraft:air" | "minecraft:cave_air"),
            "bonus torch replacement effects are unsupported"
        );
        self.check_bonus_neighbors(pos, true)?;
        self.write_postprocessed_state(pos, &old, &torch)?;
        self.update_bonus_neighbors(pos)
    }
}

impl GeneratedBlockView<'_> {
    /// Flags=2 still updates neighboring shapes. Admit only callbacks that
    /// are modeled for these specific changes.
    fn check_bonus_neighbors(&self, pos: [i32; 3], torch: bool) -> Result<()> {
        for [dx, dy, dz] in [
            [-1i32, 0i32, 0i32],
            [1, 0, 0],
            [0, 0, -1],
            [0, 0, 1],
            [0, -1, 0],
            [0, 1, 0],
        ] {
            let neighbor = [
                pos[0].wrapping_add(dx),
                pos[1].wrapping_add(dy),
                pos[2].wrapping_add(dz),
            ];
            let state = self.read(neighbor)?;
            let inert = matches!(
                state.name.as_str(),
                "minecraft:air"
                    | "minecraft:cave_air"
                    | "minecraft:void_air"
                    | "minecraft:stone"
                    | "minecraft:dirt"
                    | "minecraft:coarse_dirt"
                    | "minecraft:rooted_dirt"
                    | "minecraft:cobblestone"
            ) && state.properties.is_empty();
            let same_height_torch =
                torch && dy == 0 && state.name == "minecraft:torch" && state.properties.is_empty();
            let same_height_new_chest = torch
                && dy == 0
                && self.block_entity_edits.contains_key(&BlockPosition {
                    x: neighbor[0],
                    y: neighbor[1],
                    z: neighbor[2],
                })
                && state.name == "minecraft:chest";
            let water = liquid_state(&state)?
                .is_some_and(|fluid| fluid.kind == native_world::fluid::FluidKind::Water);
            ensure!(
                inert
                    || same_height_torch
                    || same_height_new_chest
                    || water
                    || bonus_falling_shape_schedules(&state)
                    || snowy_bonus_neighbor_inert(&state, dy)
                    || snowy_bonus_neighbor_changes(&state, dy),
                "bonus chest neighbor shape callback is unsupported: {}",
                state.name
            );
        }
        Ok(())
    }

    /// Apply shape callbacks in Java's neighbor-update order. Water sources
    /// and falling blocks schedule ticks; flowing water remains unchanged.
    fn update_bonus_neighbors(&mut self, pos: [i32; 3]) -> Result<()> {
        for [dx, dy, dz] in [
            [-1i32, 0i32, 0i32],
            [1, 0, 0],
            [0, 0, -1],
            [0, 0, 1],
            [0, -1, 0],
            [0, 1, 0],
        ] {
            let neighbor = [
                pos[0].wrapping_add(dx),
                pos[1].wrapping_add(dy),
                pos[2].wrapping_add(dz),
            ];
            let state = self.read(neighbor)?;
            if bonus_water_shape_schedules(&state)? {
                self.schedule_water(neighbor, &state)?;
            } else if bonus_falling_shape_schedules(&state) {
                ensure!(
                    self.ticks
                        .schedule(TickRegistry::Block, &state.name, neighbor, 2, 0)?
                        != ScheduleOutcome::MissingChunk,
                    "falling tick container is missing"
                );
            }
            if dy == -1 {
                self.update_snowy_bonus_support(pos)?;
            }
        }
        Ok(())
    }

    /// The placement's lower neighbor sees the new block on its upper face.
    /// A snowy surface changes to false and performs its own shape updates.
    fn update_snowy_bonus_support(&mut self, pos: [i32; 3]) -> Result<()> {
        let below = [pos[0], pos[1].wrapping_sub(1), pos[2]];
        let old = self.read(below)?;
        if !snowy_bonus_neighbor_changes(&old, -1) {
            return Ok(());
        }
        for [dx, dy, dz] in [
            [-1i32, 0i32, 0i32],
            [1, 0, 0],
            [0, 0, -1],
            [0, 0, 1],
            [0, -1, 0],
            [0, 1, 0],
        ] {
            let p = [
                below[0].wrapping_add(dx),
                below[1].wrapping_add(dy),
                below[2].wrapping_add(dz),
            ];
            let state = self.read(p)?;
            let inert = matches!(
                state.name.as_str(),
                "minecraft:air"
                    | "minecraft:cave_air"
                    | "minecraft:void_air"
                    | "minecraft:stone"
                    | "minecraft:dirt"
                    | "minecraft:coarse_dirt"
                    | "minecraft:rooted_dirt"
                    | "minecraft:cobblestone"
            ) && state.properties.is_empty();
            let placed_above = p == pos
                && dy == 1
                && matches!(state.name.as_str(), "minecraft:chest" | "minecraft:torch");
            ensure!(
                inert || placed_above || snowy_bonus_neighbor_inert(&state, dy),
                "snowy bonus support neighbor callback is unsupported: {}",
                state.name
            );
        }
        let mut next = old.clone();
        next.properties.insert("snowy".into(), "false".into());
        self.write_postprocessed_state(below, &old, &next)
    }
}

/// SnowyBlock only changes its snowy flag when its upper neighbor changes.
/// Both bonus chest and torch are non-snow blocks.
fn snowy_bonus_neighbor_inert(state: &BlockState, dy: i32) -> bool {
    if !matches!(
        state.name.as_str(),
        "minecraft:grass_block" | "minecraft:podzol" | "minecraft:mycelium"
    ) || state.properties.len() != 1
    {
        return false;
    }
    match state.properties.get("snowy").map(String::as_str) {
        Some("false") => true,
        Some("true") => dy != -1,
        _ => false,
    }
}

fn snowy_bonus_neighbor_changes(state: &BlockState, dy: i32) -> bool {
    dy == -1
        && matches!(
            state.name.as_str(),
            "minecraft:grass_block" | "minecraft:podzol" | "minecraft:mycelium"
        )
        && state.properties.len() == 1
        && state
            .properties
            .get("snowy")
            .is_some_and(|value| value == "true")
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

    fn fixture_maps(surface: i32) -> native_persistence::heightmap::FullHeightmaps {
        use native_persistence::heightmap::{
            FullHeightmaps, GeneratedHeightmaps, HeightBounds, encode_generated_heightmaps,
            generated_full_heightmaps,
        };
        let bounds = HeightBounds {
            min_y: 0,
            max_y: 16,
        };
        let mut heights = [0; 256];
        heights[8 + 8 * 16] = surface;
        let source = encode_generated_heightmaps(
            bounds,
            &GeneratedHeightmaps {
                world_surface: Some(&heights),
                ..Default::default()
            },
        )
        .unwrap();
        FullHeightmaps::from_converted(bounds, &generated_full_heightmaps(bounds, &source).unwrap())
            .unwrap()
    }

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
            GeneratedPostprocessing::new(
                Arc::clone(&world),
                0,
                16,
                region,
                None,
                None,
                BTreeMap::from([(pos, fixture_maps(0))]),
                None,
            ),
            world,
            pos,
        )
    }

    fn fence_postprocessing_trace(mode: &str) -> String {
        use native_resources::{
            ore_feature::OreBlockTags,
            packs::{ResourceId, ResourceProvider},
        };
        struct Tags(bool);
        impl ResourceProvider for Tags {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                let value = match path {
                    "data/minecraft/tags/block/fences.json" => {
                        if self.0 {
                            r#"{"values":["minecraft:oak_fence","minecraft:nether_brick_fence","minecraft:air"]}"#
                        } else {
                            r#"{"values":["minecraft:oak_fence","minecraft:nether_brick_fence"]}"#
                        }
                    }
                    "data/minecraft/tags/block/wooden_fences.json" => {
                        if self.0 {
                            r#"{"values":["minecraft:oak_fence","minecraft:air"]}"#
                        } else {
                            r#"{"values":["minecraft:oak_fence"]}"#
                        }
                    }
                    "data/minecraft/tags/block/shulker_boxes.json" => r#"{"values":[]}"#,
                    "data/minecraft/tags/block/blocks_motion_in_heightmap.json"
                    | "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json" => {
                        r#"{"values":["minecraft:stone","minecraft:spruce_planks","minecraft:oak_log","minecraft:oak_fence","minecraft:nether_brick_fence","minecraft:oak_fence_gate"]}"#
                    }
                    _ => return Ok(None),
                };
                Ok(Some(value.as_bytes().to_vec()))
            }
        }
        let registry = [
            "air",
            "stone",
            "oak_fence",
            "nether_brick_fence",
            "oak_fence_gate",
            "spruce_planks",
            "oak_log",
        ]
        .into_iter()
        .map(|name| format!("minecraft:{name}"))
        .collect();
        let ids = [
            "minecraft:fences",
            "minecraft:wooden_fences",
            "minecraft:shulker_boxes",
        ]
        .into_iter()
        .map(|name| ResourceId::parse(name).unwrap())
        .collect();
        let (mut stage, _, chunk) = fixture(false);
        stage.set_fence_tags(Some(Arc::new(
            OreBlockTags::load(&mut Tags(mode == "tag_air"), &registry, &ids).unwrap(),
        )));
        stage.heightmap_tags = Some(Arc::new(
            native_resources::heightmap_tags::HeightmapTags::load(&mut Tags(false), &registry)
                .unwrap(),
        ));
        let world = Arc::make_mut(&mut stage.world);
        let stone = world.block_id(0, 0, 0).unwrap();
        Arc::make_mut(
            Arc::make_mut(world.chunks.get_mut(&chunk).unwrap())
                .sections
                .get_mut(&0)
                .unwrap(),
        )
        .blocks
        .as_mut()
        .unwrap()
        .fill(stone);
        let fence = |name: &str, wet: bool| BlockState {
            name: format!("minecraft:{name}"),
            properties: [
                ("north", "false"),
                ("east", "false"),
                ("south", "false"),
                ("west", "false"),
                ("waterlogged", if wet { "true" } else { "false" }),
            ]
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect(),
        };
        let wet = mode == "wet_enclosed";
        let id = world.intern_state(fence("oak_fence", wet));
        world.set_block_id(8, 8, 8, id).unwrap();
        if !wet {
            let air = world.intern_state(BlockState {
                name: "minecraft:air".into(),
                properties: BTreeMap::new(),
            });
            let planks = world.intern_state(BlockState {
                name: "minecraft:spruce_planks".into(),
                properties: BTreeMap::new(),
            });
            let log = world.intern_state(BlockState {
                name: "minecraft:oak_log".into(),
                properties: BTreeMap::from([("axis".into(), "y".into())]),
            });
            world.set_block_id(7, 8, 8, air).unwrap();
            world.set_block_id(9, 8, 8, planks).unwrap();
            world.set_block_id(8, 8, 7, log).unwrap();
            world.set_block_id(8, 8, 9, air).unwrap();
            if mode == "same_fence" || mode == "other_fence" {
                let name = if mode == "same_fence" {
                    "oak_fence"
                } else {
                    "nether_brick_fence"
                };
                let id = world.intern_state(fence(name, false));
                world.set_block_id(7, 8, 8, id).unwrap();
            }
            if mode.starts_with("gate_") {
                let gate = world.intern_state(BlockState {
                    name: "minecraft:oak_fence_gate".into(),
                    properties: [
                        (
                            "facing",
                            if mode == "gate_north" {
                                "north"
                            } else {
                                "east"
                            },
                        ),
                        ("in_wall", "false"),
                        ("open", "false"),
                        ("powered", "false"),
                    ]
                    .into_iter()
                    .map(|(k, v)| (k.into(), v.into()))
                    .collect(),
                });
                world.set_block_id(7, 8, 8, gate).unwrap();
            }
        }
        stage.source = Arc::clone(&stage.world);
        let mut marks = PostProcessing::new(0, 16).unwrap();
        marks.mark(8, 8, 8);
        stage
            .region
            .tasks
            .insert(chunk, PostprocessingTask::new(Arc::new(marks), chunk));
        stage.region.ticks = RegionTicks::new(
            BTreeMap::from([(
                chunk,
                ConvertedChunkTicks {
                    blocks: ChunkTicks::from_saved(chunk, vec![]).unwrap(),
                    fluids: ChunkTicks::from_saved(chunk, vec![]).unwrap(),
                },
            )]),
            100,
            0,
        )
        .unwrap();
        while !stage.poll(chunk, 1).unwrap() {}
        let result = stage.finish().unwrap();
        let state = result.world.block(8, 8, 8).unwrap();
        let ticks = result.ticks.chunk(chunk).unwrap();
        let blocks = ticks.blocks.pack(100);
        let fluids = ticks.fluids.pack(100);
        format!(
            "F {mode} {} {} {} {} {} {} {} {} {} {} {}",
            state.properties["north"],
            state.properties["east"],
            state.properties["south"],
            state.properties["west"],
            state.properties["waterlogged"],
            blocks.len(),
            fluids.len(),
            fluids.first().map_or(-1, |tick| tick.delay),
            result.ticks.activated_snapshot().next_sub_tick,
            result.edits.len(),
            result.light_checks.len()
        )
    }

    fn lichen_postprocessing_trace(mode: &str) -> String {
        use native_resources::packs::ResourceProvider;
        struct Tags;
        impl ResourceProvider for Tags {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                match path {
                    "data/minecraft/tags/block/blocks_motion_in_heightmap.json"
                    | "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json" => {
                        Ok(Some(br#"{"values":["minecraft:stone"]}"#.to_vec()))
                    }
                    _ => Ok(None),
                }
            }
        }
        let registry = ["air", "stone", "glow_lichen"]
            .into_iter()
            .map(|name| format!("minecraft:{name}"))
            .collect();
        let (mut stage, _, chunk) = fixture(false);
        stage.heightmap_tags = Some(Arc::new(
            native_resources::heightmap_tags::HeightmapTags::load(&mut Tags, &registry).unwrap(),
        ));
        let world = Arc::make_mut(&mut stage.world);
        let stone = world.block_id(0, 0, 0).unwrap();
        Arc::make_mut(
            Arc::make_mut(world.chunks.get_mut(&chunk).unwrap())
                .sections
                .get_mut(&0)
                .unwrap(),
        )
        .blocks
        .as_mut()
        .unwrap()
        .fill(stone);
        let faces = ["down", "up", "north", "south", "west", "east"];
        let mut lichen = BlockState {
            name: "minecraft:glow_lichen".into(),
            properties: BTreeMap::new(),
        };
        for face in faces {
            let present = mode == "all"
                || mode.starts_with("lose_")
                || (face == "west" && mode != "empty")
                || (face == "east" && mode == "lost_one");
            lichen.properties.insert(face.into(), present.to_string());
        }
        lichen
            .properties
            .insert("waterlogged".into(), (mode == "wet").to_string());
        let id = world.intern_state(lichen);
        world.set_block_id(8, 8, 8, id).unwrap();
        if mode.starts_with("lost_") {
            let air = world.intern_state(BlockState {
                name: "minecraft:air".into(),
                properties: BTreeMap::new(),
            });
            world.set_block_id(7, 8, 8, air).unwrap();
        }
        if let Some(face) = mode.strip_prefix("lose_") {
            let air = world.intern_state(BlockState {
                name: "minecraft:air".into(),
                properties: BTreeMap::new(),
            });
            let pos = match face {
                "down" => [8, 7, 8],
                "up" => [8, 9, 8],
                "north" => [8, 8, 7],
                "south" => [8, 8, 9],
                "west" => [7, 8, 8],
                "east" => [9, 8, 8],
                _ => panic!("invalid test face"),
            };
            world.set_block_id(pos[0], pos[1], pos[2], air).unwrap();
        }
        stage.source = Arc::clone(&stage.world);
        let mut marks = PostProcessing::new(0, 16).unwrap();
        marks.mark(8, 8, 8);
        stage
            .region
            .tasks
            .insert(chunk, PostprocessingTask::new(Arc::new(marks), chunk));
        stage.region.ticks = RegionTicks::new(
            BTreeMap::from([(
                chunk,
                ConvertedChunkTicks {
                    blocks: ChunkTicks::from_saved(chunk, vec![]).unwrap(),
                    fluids: ChunkTicks::from_saved(chunk, vec![]).unwrap(),
                },
            )]),
            100,
            0,
        )
        .unwrap();
        while !stage.poll(chunk, 1).unwrap() {}
        let result = stage.finish().unwrap();
        let block = result.world.block(8, 8, 8).unwrap();
        let mask = if block.name == "minecraft:glow_lichen" {
            faces
                .into_iter()
                .map(|face| {
                    if block.properties[face] == "true" {
                        '1'
                    } else {
                        '0'
                    }
                })
                .collect::<String>()
        } else {
            "-".into()
        };
        let ticks = result.ticks.chunk(chunk).unwrap();
        let blocks = ticks.blocks.pack(100);
        let fluids = ticks.fluids.pack(100);
        format!(
            "L {mode} {} {mask} {} {} {} {} {} {}",
            block.name,
            blocks.len(),
            fluids.len(),
            fluids.first().map_or(-1, |tick| tick.delay),
            result.ticks.activated_snapshot().next_sub_tick,
            result.edits.len(),
            result.light_checks.len()
        )
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn generated_lichen_postprocessing_matches_java() {
        let actual = java_results(
            "GeneratedLichenPostprocessingOracle",
            include_str!("../../tests/java/GeneratedLichenPostprocessingOracle.java"),
        );
        let expected: Vec<_> = [
            "supported",
            "lost_west",
            "lost_one",
            "empty",
            "wet",
            "all",
            "lose_down",
            "lose_up",
            "lose_north",
            "lose_south",
            "lose_west",
            "lose_east",
        ]
        .into_iter()
        .map(lichen_postprocessing_trace)
        .collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn generated_lichen_updates_faces_and_schedules_water() {
        for (mode, expected) in [
            ("supported", "minecraft:glow_lichen 000010 0 0 -1 0 0 0"),
            ("lost_west", "minecraft:air - 0 0 -1 0 1 1"),
            ("lost_one", "minecraft:glow_lichen 000001 0 0 -1 0 1 0"),
            ("empty", "minecraft:air - 0 0 -1 0 1 0"),
            ("wet", "minecraft:glow_lichen 000010 0 1 5 6 0 0"),
        ] {
            assert_eq!(
                lichen_postprocessing_trace(mode),
                format!("L {mode} {expected}")
            );
        }
    }

    #[test]
    fn fence_postprocessing_updates_connections_and_water_ticks() {
        for (mode, expected) in [
            ("mixed", "true true false false false 0 0 -1 0 1 0"),
            ("same_fence", "true true false true false 0 0 -1 0 1 0"),
            ("other_fence", "true true false false false 0 0 -1 0 1 0"),
            ("tag_air", "true true true true false 0 0 -1 0 1 0"),
            ("wet_enclosed", "true true true true true 0 1 5 6 1 0"),
            ("gate_north", "true true false true false 0 0 -1 0 1 0"),
            ("gate_east", "true true false false false 0 0 -1 0 1 0"),
        ] {
            assert_eq!(
                fence_postprocessing_trace(mode),
                format!("F {mode} {expected}")
            );
        }
    }

    #[test]
    fn transactional_poll_reports_missing_fluid_neighbor_without_partial_ticks() {
        let (mut stage, _, chunk) = fixture(false);
        let world = Arc::make_mut(&mut stage.world);
        let water = world.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "0".into())]),
        });
        world.set_block_id(15, 8, 8, water).unwrap();
        stage.source = Arc::clone(&stage.world);
        let mut marks = PostProcessing::new(0, 16).unwrap();
        marks.mark(8, 8, 8);
        marks.mark(15, 8, 8);
        stage
            .region
            .tasks
            .insert(chunk, PostprocessingTask::new(Arc::new(marks), chunk));
        let before_world = Arc::clone(&stage.world);
        let before_ticks = stage.region.ticks.activated_snapshot();
        let before_reads = stage.read_chunks.borrow().clone();
        for _ in 0..2 {
            assert_eq!(
                stage.try_poll(chunk, 10).unwrap(),
                PostprocessingAttempt::NeedsChunk(ChunkPos { x: 1, z: 0 })
            );
            assert!(Arc::ptr_eq(&stage.world, &before_world));
            assert_eq!(stage.region.ticks.activated_snapshot(), before_ticks);
            assert_eq!(*stage.read_chunks.borrow(), before_reads);
            assert!(stage.edits.is_empty() && stage.light_checks.is_empty());
            assert!(!stage.failed && stage.active.is_none());
        }
        assert_eq!(
            stage.try_poll(chunk, 1).unwrap(),
            PostprocessingAttempt::Progress { complete: false }
        );
        assert_ne!(stage.region.ticks.activated_snapshot(), before_ticks);
    }

    #[test]
    fn transactional_poll_does_not_convert_unsupported_blocks_to_missing_chunks() {
        let (mut stage, _, chunk) = fixture(true);
        let before_world = Arc::clone(&stage.world);
        let before_ticks = stage.region.ticks.activated_snapshot();
        let error = stage.try_poll(chunk, 10).unwrap_err();
        assert!(error.downcast_ref::<MissingPostprocessingChunk>().is_none());
        assert!(Arc::ptr_eq(&stage.world, &before_world));
        assert_eq!(stage.region.ticks.activated_snapshot(), before_ticks);
        assert!(!stage.failed && stage.active.is_none());
    }

    fn inert_cube_postprocessing_trace() -> Vec<String> {
        let names = [
            "stone",
            "granite",
            "diorite",
            "andesite",
            "tuff",
            "calcite",
            "dripstone_block",
            "cobblestone",
            "mossy_cobblestone",
            "dirt",
            "coarse_dirt",
            "rooted_dirt",
            "clay",
            "bedrock",
            "netherrack",
            "end_stone",
            "blackstone",
            "obsidian",
            "crying_obsidian",
            "coal_ore",
            "iron_ore",
            "copper_ore",
            "gold_ore",
            "lapis_ore",
            "diamond_ore",
            "emerald_ore",
            "deepslate_coal_ore",
            "deepslate_iron_ore",
            "deepslate_copper_ore",
            "deepslate_gold_ore",
            "deepslate_lapis_ore",
            "deepslate_diamond_ore",
            "deepslate_emerald_ore",
            "nether_gold_ore",
            "nether_quartz_ore",
            "ancient_debris",
        ];
        let mut trace = Vec::new();
        for name in names {
            for adjacent in ["stone", "air", "water"] {
                let (mut stage, _, chunk) = fixture(false);
                let world = Arc::make_mut(&mut stage.world);
                let id = world.intern_state(BlockState {
                    name: format!("minecraft:{name}"),
                    properties: BTreeMap::new(),
                });
                let neighbor = world.intern_state(BlockState {
                    name: format!("minecraft:{adjacent}"),
                    properties: if adjacent == "water" {
                        BTreeMap::from([("level".into(), "0".into())])
                    } else {
                        BTreeMap::new()
                    },
                });
                world.set_block_id(8, 8, 8, id).unwrap();
                for [x, y, z] in [
                    [7, 8, 8],
                    [9, 8, 8],
                    [8, 7, 8],
                    [8, 9, 8],
                    [8, 8, 7],
                    [8, 8, 9],
                ] {
                    world.set_block_id(x, y, z, neighbor).unwrap();
                }
                stage.source = Arc::clone(&stage.world);
                let mut marks = PostProcessing::new(0, 16).unwrap();
                marks.mark(8, 8, 8);
                stage
                    .region
                    .tasks
                    .insert(chunk, PostprocessingTask::new(Arc::new(marks), chunk));
                stage.region.ticks = RegionTicks::new(
                    BTreeMap::from([(
                        chunk,
                        ConvertedChunkTicks {
                            blocks: ChunkTicks::from_saved(chunk, vec![]).unwrap(),
                            fluids: ChunkTicks::from_saved(chunk, vec![]).unwrap(),
                        },
                    )]),
                    100,
                    0,
                )
                .unwrap();
                let before_ticks = stage.region.ticks.activated_snapshot();
                let before_world = Arc::clone(&stage.world);
                while !stage.poll(chunk, 1).unwrap() {}
                let result = stage.finish().unwrap();
                assert!(Arc::ptr_eq(&before_world, &result.world));
                assert_eq!(before_ticks, result.ticks.activated_snapshot());
                assert!(result.edits.is_empty());
                assert!(inert_generated_full_cube(&format!("minecraft:{name}")));
                trace.push(format!(
                    "{name} {adjacent} 0 0 0 {}",
                    native_core::random::LegacyRandom::new(42).next_long()
                ));
            }
        }
        trace
    }

    fn inert_pillar_postprocessing_trace() -> Vec<String> {
        let mut names = Vec::new();
        for species in [
            "oak", "spruce", "birch", "jungle", "acacia", "dark_oak", "mangrove", "cherry",
            "pale_oak",
        ] {
            for suffix in ["_log", "_wood"] {
                for prefix in ["", "stripped_"] {
                    names.push(format!("{prefix}{species}{suffix}"));
                }
            }
        }
        for species in ["crimson", "warped"] {
            for suffix in ["_stem", "_hyphae"] {
                for prefix in ["", "stripped_"] {
                    names.push(format!("{prefix}{species}{suffix}"));
                }
            }
        }
        names.extend(
            [
                "bamboo_block",
                "stripped_bamboo_block",
                "deepslate",
                "basalt",
                "polished_basalt",
                "bone_block",
                "quartz_pillar",
            ]
            .into_iter()
            .map(str::to_owned),
        );
        let mut trace = Vec::new();
        for name in names {
            for axis in ["x", "y", "z"] {
                for adjacent in ["stone", "air", "water"] {
                    let (mut stage, _, chunk) = fixture(false);
                    let world = Arc::make_mut(&mut stage.world);
                    let pillar = BlockState {
                        name: format!("minecraft:{name}"),
                        properties: BTreeMap::from([("axis".into(), axis.into())]),
                    };
                    assert!(inert_generated_pillar(&pillar));
                    let id = world.intern_state(pillar);
                    let neighbor = world.intern_state(BlockState {
                        name: format!("minecraft:{adjacent}"),
                        properties: if adjacent == "water" {
                            BTreeMap::from([("level".into(), "0".into())])
                        } else {
                            BTreeMap::new()
                        },
                    });
                    world.set_block_id(8, 8, 8, id).unwrap();
                    for [x, y, z] in [
                        [7, 8, 8],
                        [9, 8, 8],
                        [8, 7, 8],
                        [8, 9, 8],
                        [8, 8, 7],
                        [8, 8, 9],
                    ] {
                        world.set_block_id(x, y, z, neighbor).unwrap();
                    }
                    stage.source = Arc::clone(&stage.world);
                    let mut marks = PostProcessing::new(0, 16).unwrap();
                    marks.mark(8, 8, 8);
                    stage
                        .region
                        .tasks
                        .insert(chunk, PostprocessingTask::new(Arc::new(marks), chunk));
                    stage.region.ticks = RegionTicks::new(
                        BTreeMap::from([(
                            chunk,
                            ConvertedChunkTicks {
                                blocks: ChunkTicks::from_saved(chunk, vec![]).unwrap(),
                                fluids: ChunkTicks::from_saved(chunk, vec![]).unwrap(),
                            },
                        )]),
                        100,
                        0,
                    )
                    .unwrap();
                    let before_ticks = stage.region.ticks.activated_snapshot();
                    let before_world = Arc::clone(&stage.world);
                    while !stage.poll(chunk, 1).unwrap() {}
                    let result = stage.finish().unwrap();
                    assert!(Arc::ptr_eq(&before_world, &result.world));
                    assert_eq!(before_ticks, result.ticks.activated_snapshot());
                    assert!(result.edits.is_empty());
                    trace.push(format!(
                        "P {name} {axis} {adjacent} 0 0 0 {}",
                        native_core::random::LegacyRandom::new(42).next_long()
                    ));
                }
            }
        }
        trace
    }

    #[test]
    fn natural_pillars_preserve_axis_and_reject_invalid_states() {
        assert_eq!(inert_pillar_postprocessing_trace().len(), 459);
        for (name, props) in [
            ("minecraft:oak_log", vec![]),
            ("minecraft:oak_log", vec![("axis", "invalid")]),
            ("minecraft:oak_log", vec![("axis", "x"), ("extra", "true")]),
            ("minecraft:crimson_log", vec![("axis", "x")]),
            ("example:oak_log", vec![("axis", "y")]),
        ] {
            let state = BlockState {
                name: name.into(),
                properties: props
                    .into_iter()
                    .map(|(key, value)| (key.into(), value.into()))
                    .collect(),
            };
            assert!(!inert_generated_pillar(&state), "{state:?}");
            assert!(supported_postprocessing_block(&state).is_err());
        }
    }

    #[test]
    fn inert_natural_cubes_complete_postprocessing_without_side_effects() {
        assert_eq!(inert_cube_postprocessing_trace().len(), 108);
        assert!(!inert_generated_full_cube("minecraft:oak_log"));
        let invalid = BlockState {
            name: "minecraft:stone".into(),
            properties: BTreeMap::from([("extra".into(), "true".into())]),
        };
        assert!(supported_postprocessing_block(&invalid).is_err());
    }

    #[test]
    fn natural_cubes_contain_generated_water_without_changing_ticks() {
        for (material, axis) in [
            ("diamond_ore", None),
            ("nether_quartz_ore", None),
            ("obsidian", None),
            ("deepslate", Some("y")),
            ("oak_log", Some("x")),
            ("warped_hyphae", Some("z")),
        ] {
            let (mut stage, _, chunk) = fixture(false);
            let world = Arc::make_mut(&mut stage.world);
            let solid = world.intern_state(BlockState {
                name: format!("minecraft:{material}"),
                properties: axis.map_or_else(BTreeMap::new, |axis| {
                    BTreeMap::from([("axis".into(), axis.into())])
                }),
            });
            let water = world.intern_state(BlockState {
                name: "minecraft:water".into(),
                properties: BTreeMap::from([("level".into(), "0".into())]),
            });
            for [x, y, z] in [
                [7, 8, 8],
                [9, 8, 8],
                [8, 7, 8],
                [8, 9, 8],
                [8, 8, 7],
                [8, 8, 9],
            ] {
                world.set_block_id(x, y, z, solid).unwrap();
            }
            world.set_block_id(8, 8, 8, water).unwrap();
            stage.source = Arc::clone(&stage.world);
            stage.set_bubble_tags(Some(Arc::new(
                native_resources::fluid_tags::BubbleColumnTags::with_fluids(
                    BTreeSet::new(),
                    BTreeSet::new(),
                    BTreeSet::from(["minecraft:water".into()]),
                ),
            )));
            let mut marks = PostProcessing::new(0, 16).unwrap();
            marks.mark(8, 8, 8);
            stage
                .region
                .tasks
                .insert(chunk, PostprocessingTask::new(Arc::new(marks), chunk));
            let before = stage.region.ticks.activated_snapshot();
            let original = Arc::clone(&stage.world);
            while !stage.poll(chunk, 1).unwrap() {}
            let result = stage.finish().unwrap();
            assert!(Arc::ptr_eq(&original, &result.world), "{material}");
            assert_eq!(before, result.ticks.activated_snapshot(), "{material}");
        }
    }

    fn bonus_chest_fixture() -> (GeneratedPostprocessing, Arc<World>) {
        use native_persistence::heightmap::{
            FullHeightmaps, GeneratedHeightmaps, HeightBounds, encode_generated_heightmaps,
            generated_full_heightmaps,
        };
        use native_resources::{heightmap_tags::HeightmapTags, packs::ResourceProvider};
        struct Tags;
        impl ResourceProvider for Tags {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path.ends_with("blocks_motion_in_heightmap.json")
                    || path.ends_with("blocks_motion_in_heightmap_no_leaves.json")
                {
                    Ok(Some(
                        br#"{"values":["minecraft:stone","minecraft:chest","minecraft:grass_block","minecraft:podzol","minecraft:mycelium","minecraft:sand","minecraft:red_sand","minecraft:gravel"]}"#.to_vec(),
                    ))
                } else {
                    Ok(None)
                }
            }
        }
        let registry = [
            "air",
            "cave_air",
            "void_air",
            "stone",
            "chest",
            "torch",
            "grass_block",
            "podzol",
            "mycelium",
            "sand",
            "red_sand",
            "gravel",
        ]
        .into_iter()
        .map(|name| format!("minecraft:{name}"))
        .collect();
        let tags = Arc::new(HeightmapTags::load(&mut Tags, &registry).unwrap());
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let stone = world.intern_state(BlockState {
            name: "minecraft:stone".into(),
            properties: BTreeMap::new(),
        });
        let mut tasks = BTreeMap::new();
        let mut ticks = BTreeMap::new();
        let mut maps = BTreeMap::new();
        let bounds = HeightBounds {
            min_y: 0,
            max_y: 16,
        };
        let heights = [8; 256];
        let raw = encode_generated_heightmaps(
            bounds,
            &GeneratedHeightmaps {
                world_surface: Some(&heights),
                ocean_floor: Some(&heights),
                motion_blocking: Some(&heights),
                motion_blocking_no_leaves: Some(&heights),
                ..Default::default()
            },
        )
        .unwrap();
        let full = generated_full_heightmaps(bounds, &raw).unwrap();
        for z in -1..=1 {
            for x in -1..=1 {
                let pos = ChunkPos { x, z };
                let mut blocks = Box::new([air; BLOCK_COUNT]);
                for y in 0..8 {
                    blocks[y * 256..(y + 1) * 256].fill(stone);
                }
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
                let marks = PostProcessing::new(0, 16).unwrap();
                tasks.insert(pos, PostprocessingTask::new(Arc::new(marks), pos));
                ticks.insert(
                    pos,
                    ConvertedChunkTicks {
                        blocks: ChunkTicks::from_saved(pos, vec![]).unwrap(),
                        fluids: ChunkTicks::from_saved(pos, vec![]).unwrap(),
                    },
                );
                maps.insert(pos, FullHeightmaps::from_converted(bounds, &full).unwrap());
            }
        }
        let world = Arc::new(world);
        let region = ConvertedPostprocessingRegion {
            tasks,
            ticks: RegionTicks::new(ticks, 0, 0).unwrap(),
        };
        (
            GeneratedPostprocessing::new(
                Arc::clone(&world),
                0,
                16,
                region,
                None,
                None,
                maps,
                Some(tags),
            ),
            world,
        )
    }

    #[test]
    fn adding_converted_chunk_retains_bonus_and_prior_tick_history() {
        use super::super::chunk_ticks::{ScheduleOutcome, TickRegistry};
        use native_core::random::LegacyRandom;

        let (fixture, world) = bonus_chest_fixture();
        let first = ChunkPos { x: 0, z: 0 };
        let second = ChunkPos { x: 1, z: 0 };
        let empty_ticks = |pos| ConvertedChunkTicks {
            blocks: ChunkTicks::from_saved(pos, vec![]).unwrap(),
            fluids: ChunkTicks::from_saved(pos, vec![]).unwrap(),
        };
        let task =
            |pos| PostprocessingTask::new(Arc::new(PostProcessing::new(0, 16).unwrap()), pos);
        let first_maps = fixture.heightmaps[&first].clone();
        let second_maps = fixture.heightmaps[&second].clone();
        let mut stage = GeneratedPostprocessing::new(
            Arc::clone(&world),
            0,
            16,
            ConvertedPostprocessingRegion {
                tasks: BTreeMap::from([(first, task(first))]),
                ticks: RegionTicks::new(BTreeMap::from([(first, empty_ticks(first))]), 100, 7)
                    .unwrap(),
            },
            None,
            None,
            BTreeMap::from([(first, first_maps)]),
            fixture.heightmap_tags.clone(),
        );
        stage.set_water_source_conversion(false).unwrap();
        assert!(
            stage
                .register_converted_chunk(
                    second,
                    super::super::postprocessing::ConvertedChunkPostprocessing {
                        task: task(second),
                        ticks: empty_ticks(second)
                    },
                    ConvertedBlockEntities::default(),
                    second_maps.clone(),
                    None,
                )
                .is_err()
        );
        while !stage.poll(first, 20).unwrap() {}
        assert_eq!(
            stage
                .region
                .ticks
                .schedule(TickRegistry::Block, "sand", [2, 8, 2], 1, 0)
                .unwrap(),
            ScheduleOutcome::Added
        );
        let mut random = LegacyRandom::new(1);
        let chest = stage
            .place_initial_bonus_chest(
                &native_resources::bonus_chest::BonusChestFeature,
                &mut random,
                [0, 8, 0],
            )
            .unwrap()
            .unwrap();
        let state = random.state();
        stage
            .register_converted_chunk(
                second,
                super::super::postprocessing::ConvertedChunkPostprocessing {
                    task: task(second),
                    ticks: empty_ticks(second),
                },
                ConvertedBlockEntities::default(),
                second_maps,
                None,
            )
            .unwrap();
        assert_eq!(stage.region.ticks.registration_order(), &[first, second]);
        assert_eq!(stage.region.ticks.next_sub_tick(), 8);
        assert_eq!(
            stage
                .region
                .ticks
                .schedule(TickRegistry::Fluid, "water", [16, 8, 0], 2, 0)
                .unwrap(),
            ScheduleOutcome::Added
        );
        while !stage.poll(second, 20).unwrap() {}
        let result = stage.finish().unwrap();
        assert_eq!(random.state(), state);
        assert_eq!(
            result
                .world
                .block(chest[0], chest[1], chest[2])
                .unwrap()
                .name,
            "minecraft:chest"
        );
        assert_eq!(result.ticks().next_sub_tick(), 9);
        assert_eq!(
            result
                .ticks()
                .chunk(first)
                .unwrap()
                .blocks
                .peek()
                .unwrap()
                .sub_tick,
            7
        );
        assert_eq!(
            result
                .ticks()
                .chunk(second)
                .unwrap()
                .fluids
                .peek()
                .unwrap()
                .sub_tick,
            8
        );
        assert!(
            matches!(result.block_entity_data(first).unwrap(), native_persistence::nbt::Tag::List { values, .. } if values.len() == 1)
        );
        let original_entities = result.block_entity_data(first).unwrap();
        let mut resumed = GeneratedPostprocessing::from_completed(
            &result,
            None,
            None,
            fixture.heightmap_tags.clone().unwrap(),
        )
        .unwrap();
        assert!(resumed.poll(first, 20).is_err());
        assert_eq!(resumed.water_source_conversion, Some(false));
        let random_state = random.state();
        assert!(
            resumed
                .place_initial_bonus_chest(
                    &native_resources::bonus_chest::BonusChestFeature,
                    &mut random,
                    [0, 8, 0],
                )
                .is_err()
        );
        assert_eq!(random.state(), random_state);
        let third = ChunkPos { x: 0, z: -1 };
        resumed
            .register_converted_chunk(
                third,
                super::super::postprocessing::ConvertedChunkPostprocessing {
                    task: task(third),
                    ticks: empty_ticks(third),
                },
                ConvertedBlockEntities::default(),
                fixture.heightmaps[&third].clone(),
                None,
            )
            .unwrap();
        while !resumed.poll(third, 20).unwrap() {}
        let extended = resumed.finish().unwrap();
        assert!(result.read_chunks.is_subset(&extended.read_chunks));
        assert_eq!(extended.read_chunks, extended.sealed_read_chunks);
        assert_eq!(
            extended.ticks().registration_order(),
            &[first, second, third]
        );
        assert_eq!(extended.ticks().next_sub_tick(), 9);
        assert_eq!(
            extended
                .ticks()
                .chunk(first)
                .unwrap()
                .blocks
                .peek()
                .unwrap()
                .sub_tick,
            7
        );
        assert_eq!(
            extended
                .ticks()
                .chunk(second)
                .unwrap()
                .fluids
                .peek()
                .unwrap()
                .sub_tick,
            8
        );
        assert_eq!(
            extended.block_entity_data(first).unwrap(),
            original_entities
        );
        assert_eq!(
            extended
                .world
                .block(chest[0], chest[1], chest[2])
                .unwrap()
                .name,
            "minecraft:chest"
        );
        assert_eq!(result.ticks().registration_order(), &[first, second]);
        assert!(!result.chunks.contains(&third));
    }

    fn bonus_chest_snowy_surface(name: &str, snowy: bool) -> (GeneratedPostprocessing, Arc<World>) {
        let (mut stage, _) = bonus_chest_fixture();
        let world = Arc::make_mut(&mut stage.world);
        let id = world.intern_state(BlockState {
            name: format!("minecraft:{name}"),
            properties: BTreeMap::from([("snowy".into(), snowy.to_string())]),
        });
        for z in -16..32 {
            for x in -16..32 {
                world.set_block_id(x, 7, z, id).unwrap();
            }
        }
        stage.source = Arc::clone(&stage.world);
        let source = Arc::clone(&stage.source);
        (stage, source)
    }

    #[test]
    fn bonus_chest_supports_unsnowed_natural_surfaces() {
        use native_core::random::LegacyRandom;
        for name in ["grass_block", "podzol", "mycelium"] {
            let (mut stage, source) = bonus_chest_snowy_surface(name, false);
            let chunks: Vec<_> = stage.region.tasks.keys().copied().collect();
            for chunk in chunks {
                while !stage.poll(chunk, 10).unwrap() {}
            }
            let mut random = LegacyRandom::new(42);
            let pos = stage
                .place_initial_bonus_chest(
                    &native_resources::bonus_chest::BonusChestFeature,
                    &mut random,
                    [0, 8, 0],
                )
                .unwrap()
                .unwrap();
            let result = stage.finish().unwrap();
            assert_eq!(
                result.world.block(pos[0], 7, pos[2]),
                source.block(pos[0], 7, pos[2])
            );
            assert_eq!(
                result.world.block(pos[0], 8, pos[2]).unwrap().name,
                "minecraft:chest"
            );
            assert_eq!(result.edits.len(), 5);
        }
    }

    #[test]
    fn snowy_neighbor_identifies_unknown_state_and_required_update() {
        for name in ["grass_block", "podzol", "mycelium"] {
            let mut state = BlockState {
                name: format!("minecraft:{name}"),
                properties: BTreeMap::from([("snowy".into(), "true".into())]),
            };
            assert!(!snowy_bonus_neighbor_inert(&state, -1));
            for dy in [0, 1] {
                assert!(snowy_bonus_neighbor_inert(&state, dy));
            }
            state.properties.insert("snowy".into(), "false".into());
            assert!(snowy_bonus_neighbor_inert(&state, -1));
            state.properties.insert("snowy".into(), "invalid".into());
            assert!(!snowy_bonus_neighbor_inert(&state, 0));
        }
    }

    #[test]
    fn bonus_chest_updates_snowy_support_without_changing_source() {
        use native_core::random::LegacyRandom;
        for name in ["grass_block", "podzol", "mycelium"] {
            let (mut stage, source) = bonus_chest_snowy_surface(name, true);
            let chunks: Vec<_> = stage.region.tasks.keys().copied().collect();
            for chunk in chunks {
                while !stage.poll(chunk, 10).unwrap() {}
            }
            let mut random = LegacyRandom::new(42);
            let p = stage
                .place_initial_bonus_chest(
                    &native_resources::bonus_chest::BonusChestFeature,
                    &mut random,
                    [0, 8, 0],
                )
                .unwrap()
                .unwrap();
            let result = stage.finish().unwrap();
            assert_eq!(result.edits.len(), 10);
            for [dx, dz] in [[0, 0], [0, -1], [1, 0], [0, 1], [-1, 0]] {
                let [x, z] = [p[0] + dx, p[2] + dz];
                assert_eq!(
                    source
                        .block(x, 7, z)
                        .unwrap()
                        .properties
                        .get("snowy")
                        .map(String::as_str),
                    Some("true")
                );
                assert_eq!(
                    result
                        .world
                        .block(x, 7, z)
                        .unwrap()
                        .properties
                        .get("snowy")
                        .map(String::as_str),
                    Some("false")
                );
            }
        }
    }

    #[test]
    fn bonus_chest_updates_isolated_generated_world_and_records() {
        use native_core::random::LegacyRandom;
        use native_persistence::nbt::Tag;
        let (mut stage, original) = bonus_chest_fixture();
        let mut random = LegacyRandom::new(42);
        let before = random.state();
        assert!(
            stage
                .place_initial_bonus_chest(
                    &native_resources::bonus_chest::BonusChestFeature,
                    &mut random,
                    [0, 8, 0]
                )
                .is_err()
        );
        assert_eq!(random.state(), before);
        let chunks: Vec<_> = stage.region.tasks.keys().copied().collect();
        for chunk in chunks {
            while !stage.poll(chunk, 10).unwrap() {}
        }
        let p = stage
            .place_initial_bonus_chest(
                &native_resources::bonus_chest::BonusChestFeature,
                &mut random,
                [0, 8, 0],
            )
            .unwrap()
            .unwrap();
        assert_eq!(p[1], 8);
        assert_eq!(
            original.block(p[0], p[1], p[2]).unwrap().name,
            "minecraft:air"
        );
        assert!(
            stage
                .place_initial_bonus_chest(
                    &native_resources::bonus_chest::BonusChestFeature,
                    &mut random,
                    [0, 8, 0]
                )
                .is_err()
        );
        let processed = stage.finish().unwrap();
        assert!(!processed.read_chunks.is_empty());
        assert_eq!(processed.read_chunks, processed.sealed_read_chunks);
        assert_eq!(
            processed.world.block(p[0], p[1], p[2]).unwrap().name,
            "minecraft:chest"
        );
        for [dx, dz] in [[0, -1], [1, 0], [0, 1], [-1, 0]] {
            assert_eq!(
                processed
                    .world
                    .block(p[0] + dx, p[1], p[2] + dz)
                    .unwrap()
                    .name,
                "minecraft:torch"
            );
        }
        let chunk = ChunkPos { x: 0, z: 0 };
        assert_eq!(
            processed.heightmaps[&chunk].motion_blocking_no_leaves(p[0], p[2]),
            9
        );
        assert_eq!(processed.edits.len(), 5);
        assert!(processed.light_checks.len() >= 4);
        let Tag::List { values, .. } = processed.block_entity_data(chunk).unwrap() else {
            panic!("missing entities")
        };
        assert_eq!(values.len(), 1);
        assert_eq!(
            values[0].get("LootTable"),
            Some(&Tag::String("minecraft:chests/spawn_bonus_chest".into()))
        );
        assert!(matches!(values[0].get("LootTableSeed"), Some(Tag::Long(_))));
        assert_eq!(values[0].get("keepPacked"), Some(&Tag::Byte(0)));
    }

    #[test]
    fn bonus_chest_missing_neighbor_can_retry_without_partial_effects() {
        use native_core::random::LegacyRandom;
        let center = ChunkPos { x: 0, z: 0 };
        let east = ChunkPos { x: 1, z: 0 };
        let (mut stage, source) = bonus_chest_fixture();
        let center_task = stage.region.tasks.remove(&center).unwrap();
        let east_task = stage.region.tasks.remove(&east).unwrap();
        let center_ticks = stage.region.ticks.chunk(center).unwrap().clone();
        let east_ticks = stage.region.ticks.chunk(east).unwrap().clone();
        let east_entities = stage.block_entities.remove(&east).unwrap();
        let east_maps = stage.heightmaps.remove(&east).unwrap();
        stage.region.tasks = BTreeMap::from([(center, center_task)]);
        stage.region.ticks =
            RegionTicks::new(BTreeMap::from([(center, center_ticks)]), 0, 0).unwrap();
        stage.block_entities.retain(|pos, _| *pos == center);
        stage.heightmaps.retain(|pos, _| *pos == center);
        while !stage.poll(center, 10).unwrap() {}
        let before_world = Arc::clone(&stage.world);
        let before_ticks = stage.region.ticks.activated_snapshot();
        let before_reads = stage.read_chunks.borrow().clone();
        let mut random = LegacyRandom::new(4);
        let before_random = random.state();
        assert_eq!(
            stage
                .try_place_initial_bonus_chest(
                    &native_resources::bonus_chest::BonusChestFeature,
                    &mut random,
                    [0, 8, 0],
                )
                .unwrap(),
            InitialBonusChestAttempt::NeedsChunk(east)
        );
        assert!(Arc::ptr_eq(&before_world, &stage.world));
        assert_eq!(stage.region.ticks.activated_snapshot(), before_ticks);
        assert_eq!(*stage.read_chunks.borrow(), before_reads);
        assert!(stage.edits.is_empty());
        assert!(!stage.bonus_chest_attempted && !stage.failed);
        assert_eq!(random.state(), before_random);
        assert_eq!(source.block(14, 8, 7).unwrap().name, "minecraft:air");

        stage
            .register_converted_chunk(
                east,
                super::super::postprocessing::ConvertedChunkPostprocessing {
                    task: east_task,
                    ticks: east_ticks,
                },
                east_entities,
                east_maps,
                None,
            )
            .unwrap();
        while !stage.poll(east, 10).unwrap() {}
        assert_eq!(
            stage
                .try_place_initial_bonus_chest(
                    &native_resources::bonus_chest::BonusChestFeature,
                    &mut random,
                    [0, 8, 0],
                )
                .unwrap(),
            InitialBonusChestAttempt::Complete(Some([14, 8, 7]))
        );
        let resumed = stage.finish().unwrap();
        let (mut baseline, _) = bonus_chest_fixture();
        let chunks: Vec<_> = baseline.region.tasks.keys().copied().collect();
        for chunk in chunks {
            while !baseline.poll(chunk, 10).unwrap() {}
        }
        let mut baseline_random = LegacyRandom::new(4);
        assert_eq!(
            baseline
                .place_initial_bonus_chest(
                    &native_resources::bonus_chest::BonusChestFeature,
                    &mut baseline_random,
                    [0, 8, 0],
                )
                .unwrap(),
            Some([14, 8, 7])
        );
        let baseline = baseline.finish().unwrap();
        assert_eq!(random.state(), baseline_random.state());
        assert_eq!(resumed.edits, baseline.edits);
        assert_eq!(resumed.light_checks, baseline.light_checks);
        assert_eq!(
            resumed.block_entity_data(center).unwrap(),
            baseline.block_entity_data(center).unwrap()
        );
        let resumed_ticks = resumed.ticks.activated_snapshot();
        let baseline_ticks = baseline.ticks.activated_snapshot();
        assert_eq!(resumed_ticks.next_sub_tick, baseline_ticks.next_sub_tick);
        assert_eq!(
            resumed_ticks.chunks[&center],
            baseline_ticks.chunks[&center]
        );
        assert_eq!(resumed_ticks.chunks[&east], baseline_ticks.chunks[&east]);
        for edit in &resumed.edits {
            assert_eq!(
                resumed.world.block(edit.x, edit.y, edit.z),
                baseline.world.block(edit.x, edit.y, edit.z)
            );
        }
    }

    #[test]
    fn bonus_chest_schedules_adjacent_source_water_and_keeps_flowing_water_inert() {
        use native_core::random::LegacyRandom;
        let run = |water_level: Option<u8>| {
            let (mut stage, _) = bonus_chest_fixture();
            let chunks: Vec<_> = stage.region.tasks.keys().copied().collect();
            if let Some(level) = water_level {
                // Seed 42 selects the same position in the unchanged flat fixture.
                let (mut baseline, _) = bonus_chest_fixture();
                for &chunk in &chunks {
                    while !baseline.poll(chunk, 10).unwrap() {}
                }
                let pos = baseline
                    .place_initial_bonus_chest(
                        &native_resources::bonus_chest::BonusChestFeature,
                        &mut LegacyRandom::new(42),
                        [0, 8, 0],
                    )
                    .unwrap()
                    .unwrap();
                let world = Arc::make_mut(&mut stage.world);
                let water = world.intern_state(BlockState {
                    name: "minecraft:water".into(),
                    properties: BTreeMap::from([("level".into(), level.to_string())]),
                });
                world
                    .set_block_id(pos[0], pos[1] + 1, pos[2], water)
                    .unwrap();
                stage.source = Arc::clone(&stage.world);
            }
            for &chunk in &chunks {
                while !stage.poll(chunk, 10).unwrap() {}
            }
            let mut random = LegacyRandom::new(42);
            let chest = stage
                .place_initial_bonus_chest(
                    &native_resources::bonus_chest::BonusChestFeature,
                    &mut random,
                    [0, 8, 0],
                )
                .unwrap()
                .unwrap();
            let result = stage.finish().unwrap();
            (chest, random.state(), result.ticks.activated_snapshot())
        };
        let (pos, original_random, _) = run(None);
        for level in [0, 1, 8, 15] {
            let (chest, random, ticks) = run(Some(level));
            assert_eq!(chest, pos);
            assert_eq!(random, original_random);
            let fluid_ticks: Vec<_> = ticks
                .chunks
                .values()
                .flat_map(|chunk| &chunk.fluids)
                .collect();
            if level == 0 {
                assert_eq!(fluid_ticks.len(), 1);
                assert_eq!(fluid_ticks[0].kind, "minecraft:water");
                assert_eq!(fluid_ticks[0].position, [pos[0], pos[1] + 1, pos[2]]);
                assert_eq!(fluid_ticks[0].trigger, 5);
            } else {
                assert!(fluid_ticks.is_empty());
            }
        }
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn bonus_water_neighbor_callbacks_match_java() {
        use native_core::random::LegacyRandom;
        let rows = java_results(
            "BonusWaterNeighborOracle",
            include_str!("../../tests/java/BonusWaterNeighborOracle.java"),
        );
        assert_eq!(rows.len(), 16 * 6 * 2);
        for row in rows {
            let fields: Vec<_> = row.split_whitespace().collect();
            assert_eq!(fields.len(), 6, "{row}");
            let level: u8 = fields[0].parse().unwrap();
            let state = BlockState {
                name: "minecraft:water".into(),
                properties: BTreeMap::from([("level".into(), level.to_string())]),
            };
            assert_eq!(fields[3], "true", "{row}");
            let expected_tick = if bonus_water_shape_schedules(&state).unwrap() {
                "minecraft:water:5"
            } else {
                "none"
            };
            assert_eq!(fields[4], expected_tick, "{row}");
            assert_eq!(
                fields[5].parse::<i64>().unwrap(),
                LegacyRandom::new(42).next_long(),
                "{row}"
            );
        }
    }

    #[test]
    fn bonus_chest_schedules_falling_neighbors_without_replacing_existing_ticks() {
        use native_core::random::LegacyRandom;
        let (mut baseline, _) = bonus_chest_fixture();
        let chunks: Vec<_> = baseline.region.tasks.keys().copied().collect();
        for chunk in chunks {
            while !baseline.poll(chunk, 10).unwrap() {}
        }
        let expected_chest = baseline
            .place_initial_bonus_chest(
                &native_resources::bonus_chest::BonusChestFeature,
                &mut LegacyRandom::new(42),
                [0, 8, 0],
            )
            .unwrap()
            .unwrap();
        for block in ["minecraft:sand", "minecraft:red_sand", "minecraft:gravel"] {
            let (mut stage, _) = bonus_chest_fixture();
            let world = Arc::make_mut(&mut stage.world);
            let falling = world.intern_state(BlockState {
                name: block.into(),
                properties: BTreeMap::new(),
            });
            for z in -16..32 {
                for x in -16..32 {
                    world.set_block_id(x, 7, z, falling).unwrap();
                }
            }
            let water = world.intern_state(BlockState {
                name: "minecraft:water".into(),
                properties: BTreeMap::from([("level".into(), "0".into())]),
            });
            world
                .set_block_id(expected_chest[0], 9, expected_chest[2], water)
                .unwrap();
            stage.source = Arc::clone(&stage.world);
            let chunks: Vec<_> = stage.region.tasks.keys().copied().collect();
            for chunk in chunks {
                while !stage.poll(chunk, 10).unwrap() {}
            }
            let below_chest = [expected_chest[0], 7, expected_chest[2]];
            assert_eq!(
                stage
                    .region
                    .ticks
                    .schedule(TickRegistry::Block, block, below_chest, 9, 0)
                    .unwrap(),
                ScheduleOutcome::Added
            );
            let mut random = LegacyRandom::new(42);
            let pos = stage
                .place_initial_bonus_chest(
                    &native_resources::bonus_chest::BonusChestFeature,
                    &mut random,
                    [0, 8, 0],
                )
                .unwrap()
                .unwrap();
            assert_eq!(pos, expected_chest);
            let result = stage.finish().unwrap();
            let ticks = result.ticks.activated_snapshot();
            let scheduled: Vec<_> = ticks
                .chunks
                .values()
                .flat_map(|chunk| &chunk.blocks)
                .collect();
            let expected: Vec<_> = [[0, 0], [0, -1], [1, 0], [0, 1], [-1, 0]]
                .into_iter()
                .map(|[dx, dz]| [pos[0] + dx, 7, pos[2] + dz])
                .collect();
            assert_eq!(scheduled.len(), expected.len());
            for (sub_tick, target) in expected.into_iter().enumerate() {
                let tick = scheduled
                    .iter()
                    .find(|tick| tick.position == target)
                    .unwrap();
                assert_eq!(tick.kind, block);
                assert_eq!(tick.trigger, if sub_tick == 0 { 9 } else { 2 });
                assert_eq!(
                    tick.sub_tick,
                    if sub_tick == 0 {
                        0
                    } else {
                        sub_tick as i64 + 2
                    }
                );
            }
            let fluid_ticks: Vec<_> = ticks
                .chunks
                .values()
                .flat_map(|chunk| &chunk.fluids)
                .collect();
            assert_eq!(fluid_ticks.len(), 1);
            assert_eq!(fluid_ticks[0].position, [pos[0], 9, pos[2]]);
            assert_eq!(fluid_ticks[0].kind, "minecraft:water");
            assert_eq!(fluid_ticks[0].trigger, 5);
            assert_eq!(fluid_ticks[0].sub_tick, 2);
            assert_eq!(ticks.next_sub_tick, 7);
        }
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn bonus_falling_neighbor_callbacks_match_java() {
        use native_core::random::LegacyRandom;
        let rows = java_results(
            "BonusFallingNeighborOracle",
            include_str!("../../tests/java/BonusFallingNeighborOracle.java"),
        );
        assert_eq!(rows.len(), 3 * 6 * 2);
        for row in rows {
            let fields: Vec<_> = row.split_whitespace().collect();
            assert_eq!(fields.len(), 6, "{row}");
            let state = BlockState {
                name: fields[0].into(),
                properties: BTreeMap::new(),
            };
            assert!(bonus_falling_shape_schedules(&state), "{row}");
            assert_eq!(fields[3], "true", "{row}");
            assert_eq!(fields[4], format!("{}:2", fields[0]), "{row}");
            assert_eq!(
                fields[5].parse::<i64>().unwrap(),
                LegacyRandom::new(42).next_long(),
                "{row}"
            );
        }
    }

    #[test]
    fn bonus_chest_rejects_unmodeled_neighbor_effects_without_advancing_rng() {
        use native_core::random::LegacyRandom;
        let (mut stage, _) = bonus_chest_fixture();
        let world = Arc::make_mut(&mut stage.world);
        let unsupported = world.intern_state(BlockState {
            name: "minecraft:bedrock".into(),
            properties: BTreeMap::new(),
        });
        for z in -16..32 {
            for x in -16..32 {
                world.set_block_id(x, 7, z, unsupported).unwrap();
            }
        }
        stage.source = Arc::clone(&stage.world);
        let source = Arc::clone(&stage.source);
        let chunks: Vec<_> = stage.region.tasks.keys().copied().collect();
        for chunk in chunks {
            while !stage.poll(chunk, 10).unwrap() {}
        }
        let mut random = LegacyRandom::new(42);
        let state = random.state();
        let error = stage
            .try_place_initial_bonus_chest(
                &native_resources::bonus_chest::BonusChestFeature,
                &mut random,
                [0, 8, 0],
            )
            .unwrap_err();
        assert!(format!("{error:#}").contains("neighbor shape callback"));
        assert_eq!(random.state(), state);
        assert!(!stage.failed);
        assert!(Arc::ptr_eq(&stage.world, &source));
        let error = stage
            .place_initial_bonus_chest(
                &native_resources::bonus_chest::BonusChestFeature,
                &mut random,
                [0, 8, 0],
            )
            .unwrap_err();
        assert!(format!("{error:#}").contains("neighbor shape callback"));
        assert_eq!(random.state(), state);
        assert_eq!(source.block(0, 7, 0).unwrap().name, "minecraft:bedrock");
        assert!(stage.finish().is_err());
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn generated_bonus_chest_matches_java_flat_world() {
        use native_core::random::LegacyRandom;
        use native_persistence::nbt::Tag;
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_owned()];
        for lib in manifest["libraries"].as_array().unwrap() {
            if let Some(p) = lib["downloads"]["artifact"]["path"].as_str() {
                let p = game.join("libraries").join(p);
                if p.is_file() {
                    paths.push(p);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("BonusChestOracle.java");
        std::fs::write(
            &source,
            include_str!("../../../resources/tests/java/BonusChestOracle.java"),
        )
        .unwrap();
        let run = |cmd: &mut Command| {
            let out = cmd.output().unwrap();
            assert!(
                out.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&out.stderr),
                String::from_utf8_lossy(&out.stdout)
            );
            String::from_utf8(out.stdout).unwrap()
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source));
        let snowy_source = temp.path().join("BonusChestSnowyOracle.java");
        std::fs::write(
            &snowy_source,
            include_str!("../../tests/java/BonusChestSnowyOracle.java"),
        )
        .unwrap();
        run(Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&snowy_source));
        let snowy_output = run(Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("BonusChestSnowyOracle"));
        let rows: Vec<_> = snowy_output
            .lines()
            .filter(|line| line.starts_with("S "))
            .collect();
        assert_eq!(rows.len(), 72);
        for line in rows {
            let fields: Vec<_> = line.split_whitespace().collect();
            let snowy: bool = fields[2].parse().unwrap();
            let state = BlockState {
                name: fields[1].into(),
                properties: BTreeMap::from([("snowy".into(), snowy.to_string())]),
            };
            let dy = match fields[3] {
                "up" => -1,
                "down" => 1,
                _ => 0,
            };
            assert_eq!(
                snowy_bonus_neighbor_inert(&state, dy),
                fields[5].parse::<bool>().unwrap(),
                "{line}"
            );
            assert_eq!(
                native_world::collision::button_support_face(&state, glam::IVec3::Y),
                Some(fields[6].parse::<bool>().unwrap()),
                "{line}"
            );
            assert_eq!(
                LegacyRandom::new(42).next_long(),
                fields[7].parse::<i64>().unwrap(),
                "{line}"
            );
        }
        for (arg, name, mode, snowy) in [
            ("flat", None, 4, false),
            ("flat_grass", Some("grass_block"), 5, false),
            ("flat_podzol", Some("podzol"), 6, false),
            ("flat_mycelium", Some("mycelium"), 7, false),
            ("flat_snowy_grass", Some("grass_block"), 8, true),
            ("flat_snowy_podzol", Some("podzol"), 9, true),
            ("flat_snowy_mycelium", Some("mycelium"), 10, true),
        ] {
            let output = run(Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("BonusChestOracle")
                .arg(arg));
            if snowy {
                let shape_next = output
                    .lines()
                    .find_map(|line| line.strip_prefix("T "))
                    .unwrap()
                    .parse::<i64>()
                    .unwrap();
                assert_eq!(
                    shape_next,
                    LegacyRandom::new(13).next_long(),
                    "shape callbacks consumed random state"
                );
            }
            let prefix = format!("C 0 42 {mode} true true ");
            let line = output
                .lines()
                .find(|line| line.starts_with(&prefix))
                .unwrap();
            let (_, tail) = line.split_once(" W ").unwrap();
            let (expected_writes, tail) = tail.split_once(" L ").unwrap();
            let (expected_loot, expected_next) = tail.split_once(" R ").unwrap();
            let (mut stage, _) = name.map_or_else(bonus_chest_fixture, |name| {
                bonus_chest_snowy_surface(name, snowy)
            });
            let chunks: Vec<_> = stage.region.tasks.keys().copied().collect();
            for chunk in chunks {
                while !stage.poll(chunk, 10).unwrap() {}
            }
            let mut random = LegacyRandom::new(42);
            let pos = stage
                .place_initial_bonus_chest(
                    &native_resources::bonus_chest::BonusChestFeature,
                    &mut random,
                    [0, 8, 0],
                )
                .unwrap()
                .unwrap();
            let result = stage.finish().unwrap();
            if snowy {
                let expected_snowy: Vec<bool> = output
                    .lines()
                    .find_map(|line| line.strip_prefix("G "))
                    .unwrap()
                    .split_whitespace()
                    .map(|value| value.parse().unwrap())
                    .collect();
                let actual_snowy: Vec<bool> = [[0, 0], [0, -1], [1, 0], [0, 1], [-1, 0]]
                    .into_iter()
                    .map(|[dx, dz]| {
                        result
                            .world
                            .block(pos[0] + dx, pos[1] - 1, pos[2] + dz)
                            .unwrap()
                            .properties["snowy"]
                            .parse()
                            .unwrap()
                    })
                    .collect();
                assert_eq!(
                    actual_snowy, expected_snowy,
                    "snowy support state for {arg}"
                );
            }
            let writes = result
                .edits
                .iter()
                .map(|p| {
                    let state = result.world.block(p.x, p.y, p.z).unwrap();
                    format!(
                        "{},{},{},{}",
                        p.x,
                        p.y,
                        p.z,
                        state.name.strip_prefix("minecraft:").unwrap()
                    )
                })
                .collect::<Vec<_>>()
                .join(";");
            assert_eq!(writes, expected_writes);
            let Tag::List { values, .. } =
                result.block_entity_data(ChunkPos { x: 0, z: 0 }).unwrap()
            else {
                panic!("missing block entity")
            };
            let seed = match values[0].get("LootTableSeed") {
                Some(Tag::Long(seed)) => *seed,
                None => 0,
                _ => panic!("invalid loot seed"),
            };
            assert_eq!(
                format!("{},{},{},{seed}", pos[0], pos[1], pos[2]),
                expected_loot
            );
            assert_eq!(random.next_long(), expected_next.parse::<i64>().unwrap());
        }
    }

    #[test]
    fn postprocessing_promotes_pending_chest_without_mutating_source() {
        use native_persistence::nbt::{Tag, TagType};
        let (mut stage, _, chunk) = fixture(false);
        let position = [2, 8, 2];
        let chest = BlockState {
            name: "minecraft:chest".into(),
            properties: BTreeMap::from([
                ("facing".into(), "west".into()),
                ("type".into(), "left".into()),
                ("waterlogged".into(), "true".into()),
            ]),
        };
        let world = Arc::make_mut(&mut stage.world);
        let id = world.intern_state(chest.clone());
        assert!(
            world
                .set_block_id(position[0], position[1], position[2], id)
                .unwrap()
        );
        stage.source = Arc::clone(&stage.world);
        let source = Arc::clone(&stage.source);
        let mut pending = PendingPositions::default();
        pending.insert(position);
        let dummy = Tag::Compound(vec![
            ("id".into(), Tag::String("DUMMY".into())),
            ("x".into(), Tag::Int(position[0])),
            ("y".into(), Tag::Int(position[1])),
            ("z".into(), Tag::Int(position[2])),
        ]);
        let input = Tag::List {
            element_type: TagType::Compound,
            values: vec![dummy.clone()],
        };
        assert!(ConvertedBlockEntities::from_proto(input.clone(), None, None).is_err());
        let converted = ConvertedBlockEntities::from_proto(input, None, Some(&pending)).unwrap();
        stage
            .set_block_entities(BTreeMap::from([(chunk, converted)]))
            .unwrap();
        while !stage.poll(chunk, 1).unwrap() {}
        let result = stage.finish().unwrap();
        assert!(Arc::ptr_eq(&source, &result.world));
        assert_eq!(
            source.block(position[0], position[1], position[2]),
            Some(&chest)
        );
        let Tag::List { values, .. } = result.block_entity_data(chunk).unwrap() else {
            panic!("missing entities")
        };
        assert_eq!(values.len(), 1);
        assert_eq!(
            values[0].get("id"),
            Some(&Tag::String("minecraft:chest".into()))
        );
        assert_eq!(values[0].get("keepPacked"), Some(&Tag::Byte(0)));
        assert!(
            matches!(values[0].get("Items"), Some(Tag::List { values, .. }) if values.is_empty())
        );
        assert_eq!(dummy.get("id"), Some(&Tag::String("DUMMY".into())));
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn pending_chests_promote_like_java_level_chunk() {
        use native_persistence::nbt::{self, Tag, TagType};
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
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
        let source = temp
            .path()
            .join("GeneratedPendingPostprocessingOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/GeneratedPendingPostprocessingOracle.java"),
        )
        .unwrap();
        let run = |command: &mut Command| {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            );
            String::from_utf8(output.stdout).unwrap()
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source));
        let output = run(Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("GeneratedPendingPostprocessingOracle")
            .arg(temp.path()));
        let parse = |prefix| {
            output
                .lines()
                .find_map(|line| line.strip_prefix(prefix))
                .unwrap()
                .split_whitespace()
                .map(|p| {
                    let values: Vec<i32> = p.split(',').map(|n| n.parse().unwrap()).collect();
                    [values[0], values[1], values[2]]
                })
                .collect::<Vec<_>>()
        };
        let expected_pending = parse("P ");
        let expected_order = parse("O ");
        assert_eq!(expected_pending.len(), 12);
        assert_eq!(expected_order.len(), 24);
        for budget in [1, 100] {
            let (mut stage, _, chunk) = fixture(false);
            let state = BlockState {
                name: "minecraft:chest".into(),
                properties: BTreeMap::from([
                    ("facing".into(), "north".into()),
                    ("type".into(), "single".into()),
                    ("waterlogged".into(), "false".into()),
                ]),
            };
            let world = Arc::make_mut(&mut stage.world);
            let id = world.intern_state(state.clone());
            for i in 0..24 {
                assert!(world.set_block_id(i % 16, 8, i / 16, id).unwrap());
            }
            stage.source = Arc::clone(&stage.world);
            let mut pending = PendingPositions::default();
            let mut live = LivePositions::default();
            let mut values = Vec::new();
            for i in 0..24 {
                pending.insert([i % 16, 8, i / 16]);
            }
            for i in (0..24).step_by(2) {
                let p = [i % 16, 8, i / 16];
                pending.remove(p);
                live.insert(p);
            }
            for i in 0..24 {
                let p = [i % 16, 8, i / 16];
                if i % 2 == 0 {
                    values.push(
                        super::super::GeneratedEmptyChest {
                            position: p,
                            state: state.clone(),
                        }
                        .block_entity(),
                    );
                } else {
                    values.push(Tag::Compound(vec![
                        ("id".into(), Tag::String("DUMMY".into())),
                        ("x".into(), Tag::Int(p[0])),
                        ("y".into(), Tag::Int(p[1])),
                        ("z".into(), Tag::Int(p[2])),
                    ]));
                }
            }
            let converted = ConvertedBlockEntities::from_proto(
                Tag::List {
                    element_type: TagType::Compound,
                    values,
                },
                Some(&live),
                Some(&pending),
            )
            .unwrap();
            assert_eq!(converted.pending.positions().unwrap(), expected_pending);
            stage
                .set_block_entities(BTreeMap::from([(chunk, converted)]))
                .unwrap();
            while !stage.poll(chunk, budget).unwrap() {}
            let result = stage.finish().unwrap();
            let Tag::List { values, .. } = result.block_entity_data(chunk).unwrap() else {
                panic!("missing entity list")
            };
            let coordinates = |tag: &Tag| {
                let number = |name| match tag.get(name) {
                    Some(Tag::Int(value)) => *value,
                    _ => panic!("missing position"),
                };
                [number("x"), number("y"), number("z")]
            };
            assert_eq!(
                values.iter().map(coordinates).collect::<Vec<_>>(),
                expected_order
            );
            for actual in &values {
                let p = coordinates(actual);
                let expected = nbt::read(
                    &std::fs::read(temp.path().join(format!("{}.nbt", p[0] + p[2] * 16))).unwrap(),
                )
                .unwrap()
                .value;
                let (Tag::Compound(a), Tag::Compound(b)) = (actual, &expected) else {
                    panic!("invalid NBT")
                };
                let names_a: BTreeSet<_> =
                    a.iter().map(|(name, _)| name.to_string_lossy()).collect();
                let names_b: BTreeSet<_> =
                    b.iter().map(|(name, _)| name.to_string_lossy()).collect();
                assert_eq!(names_a, names_b, "fields at {p:?}");
                for field in names_a {
                    assert_eq!(
                        actual.get(&field),
                        expected.get(&field),
                        "field {field} at {p:?}"
                    );
                }
            }
        }
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

    fn mushroom_case(mushroom: &str, support: &str, tagged: bool, brightness: u8) -> String {
        use native_resources::{
            ore_feature::OreBlockTags,
            packs::{ResourceId, ResourceProvider},
        };
        use native_world::generation_light::VisibleLightColumn;
        struct TagSource {
            support: String,
            tagged: bool,
        }
        impl ResourceProvider for TagSource {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if matches!(
                    path,
                    "data/minecraft/tags/block/blocks_motion_in_heightmap.json"
                        | "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json"
                ) {
                    return Ok(Some(br#"{"values":[]}"#.to_vec()));
                }
                if path != "data/minecraft/tags/block/overrides_mushroom_light_requirement.json" {
                    return Ok(None);
                }
                Ok(Some(serde_json::to_vec(
                    &serde_json::json!({"values": if self.tagged { vec![self.support.clone()] } else { vec![] }}),
                )?))
            }
        }
        let pos = ChunkPos { x: 0, z: 0 };
        let id = |name: &str| format!("minecraft:{name}");
        let registry = BTreeSet::from([id("air"), id("void_air"), id(mushroom), id(support)]);
        let tags = OreBlockTags::load(
            &mut TagSource {
                support: id(support),
                tagged,
            },
            &registry,
            &BTreeSet::from([
                ResourceId::parse("minecraft:overrides_mushroom_light_requirement").unwrap(),
            ]),
        )
        .unwrap();
        let heightmap_tags = native_resources::heightmap_tags::HeightmapTags::load(
            &mut TagSource {
                support: id(support),
                tagged,
            },
            &registry,
        )
        .unwrap();
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: id("air"),
            properties: BTreeMap::new(),
        });
        let plant = world.intern_state(BlockState {
            name: id(mushroom),
            properties: BTreeMap::new(),
        });
        let properties = match support {
            "grass_block" | "podzol" | "mycelium" => {
                BTreeMap::from([("snowy".into(), "false".into())])
            }
            "water" => BTreeMap::from([("level".into(), "0".into())]),
            "chest" => BTreeMap::from([
                ("facing".into(), "north".into()),
                ("type".into(), "single".into()),
                ("waterlogged".into(), "false".into()),
            ]),
            _ => BTreeMap::new(),
        };
        let support_id = world.intern_state(BlockState {
            name: id(support),
            properties,
        });
        let mut blocks = Box::new([air; BLOCK_COUNT]);
        blocks[8 + 8 * 16 + 8 * 256] = plant;
        blocks[8 + 8 * 16 + 7 * 256] = support_id;
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
        let source = Arc::new(world);
        let mut light = VisibleLightSnapshot::new(true, false);
        light.insert_complete_column(
            pos,
            VisibleLightColumn::new(
                BTreeMap::from([(
                    0,
                    Arc::new([brightness | brightness << 4; native_world::loaded::LIGHT_BYTES]),
                )]),
                BTreeMap::new(),
            ),
        );
        let mut marks = PostProcessing::new(0, 16).unwrap();
        marks.mark(8, 8, 8);
        let region = ConvertedPostprocessingRegion {
            tasks: BTreeMap::from([(pos, PostprocessingTask::new(Arc::new(marks), pos))]),
            ticks: RegionTicks::new(
                BTreeMap::from([(
                    pos,
                    ConvertedChunkTicks {
                        blocks: ChunkTicks::from_saved(pos, vec![]).unwrap(),
                        fluids: ChunkTicks::from_saved(pos, vec![]).unwrap(),
                    },
                )]),
                100,
                0,
            )
            .unwrap(),
        };
        let mut stage = GeneratedPostprocessing::new(
            Arc::clone(&source),
            0,
            16,
            region,
            Some(light),
            Some(Arc::new(tags)),
            BTreeMap::from([(pos, fixture_maps(9))]),
            Some(Arc::new(heightmap_tags)),
        );
        while !stage.poll(pos, 1).unwrap() {}
        let result = stage.finish().unwrap();
        assert_eq!(
            result.light_reads,
            if tagged {
                BTreeSet::new()
            } else {
                BTreeSet::from([pos])
            }
        );
        assert_eq!(result.light_reads, result.sealed_light_reads);
        assert_eq!(source.block(8, 8, 8).unwrap().name, id(mushroom));
        let state = &result.world.block(8, 8, 8).unwrap().name;
        let removed = state == "minecraft:air";
        assert_eq!(result.edits.len(), usize::from(removed));
        assert_eq!(
            result.light_checks.len(),
            usize::from(removed && mushroom == "brown_mushroom")
        );
        let maps = result.heightmaps[&pos].to_tag().unwrap();
        let document = native_persistence::nbt::NamedTag {
            name: "".into(),
            value: native_persistence::nbt::Tag::Compound(vec![("Heightmaps".into(), maps)]),
        };
        let heights = native_persistence::heightmap::SpawnHeightmaps::read(
            &document,
            native_persistence::heightmap::HeightBounds {
                min_y: 0,
                max_y: 16,
            },
        )
        .unwrap();
        assert_eq!(
            heights
                .highest(
                    native_persistence::heightmap::SpawnHeightmapKind::WorldSurface,
                    8,
                    8
                )
                .unwrap()
                + 1,
            if removed {
                if support == "air" { 0 } else { 8 }
            } else {
                9
            }
        );
        format!(
            "{} {} {} {} {} {} {}",
            id(mushroom),
            id(support),
            tagged,
            brightness,
            state,
            usize::from(removed),
            if removed { 276 } else { -1 }
        )
    }

    #[test]
    fn mushroom_removal_preserves_source_and_records_light_and_heightmap_work() {
        assert_eq!(
            mushroom_case("brown_mushroom", "stone", false, 12),
            "minecraft:brown_mushroom minecraft:stone false 12 minecraft:brown_mushroom 0 -1"
        );
        assert_eq!(
            mushroom_case("brown_mushroom", "stone", false, 13),
            "minecraft:brown_mushroom minecraft:stone false 13 minecraft:air 1 276"
        );
        assert_eq!(
            mushroom_case("red_mushroom", "stone", true, 15),
            "minecraft:red_mushroom minecraft:stone true 15 minecraft:red_mushroom 0 -1"
        );
        assert_eq!(
            mushroom_case("red_mushroom", "air", false, 0),
            "minecraft:red_mushroom minecraft:air false 0 minecraft:air 1 276"
        );
    }

    fn leaf_case(name: &str, distance: u8, tagged: bool, budget: usize) -> String {
        use native_resources::{
            ore_feature::OreBlockTags,
            packs::{ResourceId, ResourceProvider},
        };
        struct TagSource(bool);
        impl ResourceProvider for TagSource {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/minecraft/tags/block/prevents_nearby_leaf_decay.json" {
                    return Ok(Some(serde_json::to_vec(&serde_json::json!({
                        "values": if self.0 { vec!["minecraft:stone"] } else { vec![] }
                    }))?));
                }
                Ok(None)
            }
        }
        let (mut stage, _, pos) = fixture(false);
        let world = Arc::make_mut(&mut stage.world);
        let stone = world.block_id(0, 0, 0).unwrap();
        let leaf = world.intern_state(BlockState {
            name: format!("minecraft:{name}"),
            properties: BTreeMap::from([
                ("distance".into(), distance.to_string()),
                ("persistent".into(), "false".into()),
                ("waterlogged".into(), "false".into()),
            ]),
        });
        let section = Arc::make_mut(
            Arc::make_mut(world.chunks.get_mut(&pos).unwrap())
                .sections
                .get_mut(&0)
                .unwrap(),
        );
        section.blocks.as_mut().unwrap().fill(stone);
        world.set_block_id(8, 8, 8, leaf).unwrap();
        stage.source = Arc::clone(&stage.world);
        let mut marks = PostProcessing::new(0, 16).unwrap();
        marks.mark(8, 8, 8);
        marks.mark(8, 8, 8);
        stage.region.tasks = BTreeMap::from([(pos, PostprocessingTask::new(Arc::new(marks), pos))]);
        stage.region.ticks = RegionTicks::new(
            BTreeMap::from([(
                pos,
                ConvertedChunkTicks {
                    blocks: ChunkTicks::from_saved(pos, vec![]).unwrap(),
                    fluids: ChunkTicks::from_saved(pos, vec![]).unwrap(),
                },
            )]),
            100,
            2,
        )
        .unwrap();
        let tag = ResourceId::parse("minecraft:prevents_nearby_leaf_decay").unwrap();
        let registry = BTreeSet::from(["minecraft:stone".to_owned(), format!("minecraft:{name}")]);
        stage.set_leaf_tags(Some(Arc::new(
            OreBlockTags::load(&mut TagSource(tagged), &registry, &BTreeSet::from([tag])).unwrap(),
        )));
        while !stage.poll(pos, budget).unwrap() {}
        let result = stage.finish().unwrap();
        let ticks = result.ticks.chunk(pos).unwrap().blocks.pack(100);
        assert!(result.edits.is_empty());
        assert_eq!(
            result.world.block(8, 8, 8).unwrap().name,
            format!("minecraft:{name}")
        );
        format!(
            "minecraft:{name} {distance} {tagged} {} {} {}",
            ticks.len(),
            ticks.first().map_or(-1, |tick| tick.delay),
            result.ticks.next_sub_tick()
        )
    }

    #[test]
    fn leaf_postprocessing_schedules_once_and_accounts_for_each_attempt() {
        assert_eq!(
            leaf_case("oak_leaves", 1, true, 1),
            "minecraft:oak_leaves 1 true 0 -1 2"
        );
        assert_eq!(
            leaf_case("oak_leaves", 1, false, 1),
            "minecraft:oak_leaves 1 false 1 1 14"
        );
        assert_eq!(
            leaf_case("cherry_leaves", 7, true, 100),
            "minecraft:cherry_leaves 7 true 1 1 14"
        );
    }

    #[test]
    fn leaves_reject_missing_tags_and_invalid_states() {
        let state = |distance: &str, waterlogged: &str| BlockState {
            name: "minecraft:oak_leaves".into(),
            properties: BTreeMap::from([
                ("distance".into(), distance.into()),
                ("persistent".into(), "true".into()),
                ("waterlogged".into(), waterlogged.into()),
            ]),
        };
        assert!(matches!(
            supported_postprocessing_block(&state("1", "true")),
            Ok(DryBlock::Leaves)
        ));
        assert!(supported_postprocessing_block(&state("01", "false")).is_err());
        assert!(supported_postprocessing_block(&state("8", "false")).is_err());
        let (mut stage, _, pos) = fixture(false);
        let world = Arc::make_mut(&mut stage.world);
        let leaf = world.intern_state(state("1", "false"));
        world.set_block_id(8, 8, 8, leaf).unwrap();
        let before = Arc::clone(&stage.world);
        assert!(stage.poll(pos, 1).is_err());
        assert!(stage.finish().is_err());
        assert_eq!(before.block(8, 8, 8).unwrap().name, "minecraft:oak_leaves");
    }

    fn contained_liquid_case(name: &str, level: u8, occupy: bool) -> String {
        let (mut stage, _, chunk) = fixture(false);
        let world = Arc::make_mut(&mut stage.world);
        let stone = world.block_id(0, 0, 0).unwrap();
        let liquid = world.intern_state(BlockState {
            name: format!("minecraft:{name}"),
            properties: BTreeMap::from([("level".into(), level.to_string())]),
        });
        let section = Arc::make_mut(
            Arc::make_mut(world.chunks.get_mut(&chunk).unwrap())
                .sections
                .get_mut(&0)
                .unwrap(),
        );
        section.blocks.as_mut().unwrap().fill(stone);
        world.set_block_id(8, 8, 8, liquid).unwrap();
        stage.source = Arc::clone(&stage.world);
        let source = Arc::clone(&stage.world);
        let mut marks = PostProcessing::new(0, 16).unwrap();
        marks.mark(8, 8, 8);
        stage.region.tasks =
            BTreeMap::from([(chunk, PostprocessingTask::new(Arc::new(marks), chunk))]);
        stage.set_bubble_tags(Some(Arc::new(
            native_resources::fluid_tags::BubbleColumnTags::with_fluids(
                BTreeSet::new(),
                BTreeSet::new(),
                if occupy {
                    BTreeSet::from([format!("minecraft:{name}")])
                } else {
                    BTreeSet::new()
                },
            ),
        )));
        while !stage.poll(chunk, 1).unwrap() {}
        let result = stage.finish().unwrap();
        assert!(Arc::ptr_eq(&source, &result.world));
        assert!(result.edits.is_empty());
        format!(
            "minecraft:{name} {level} {occupy} {}",
            result.world.block(8, 8, 8).unwrap().name
        )
    }

    #[test]
    fn enclosed_liquid_ticks_preserve_world_and_reject_flow_or_bubbles() {
        assert_eq!(
            contained_liquid_case("water", 0, true),
            "minecraft:water 0 true minecraft:water"
        );
        assert_eq!(
            contained_liquid_case("lava", 0, false),
            "minecraft:lava 0 false minecraft:lava"
        );
        let (mut stage, source, pos) = fixture(false);
        let world = Arc::make_mut(&mut stage.world);
        let water = world.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "0".into())]),
        });
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        world.set_block_id(8, 8, 8, water).unwrap();
        world.set_block_id(8, 7, 8, air).unwrap();
        let before = Arc::clone(&stage.world);
        assert!(stage.poll(pos, 1).is_err());
        assert!(stage.finish().is_err());
        assert_eq!(before.block(8, 7, 8).unwrap().name, "minecraft:air");
        assert_eq!(source.block(8, 8, 8).unwrap().name, "minecraft:sand");

        let (mut stage, _, pos) = fixture(false);
        let world = Arc::make_mut(&mut stage.world);
        let stone = world.block_id(0, 0, 0).unwrap();
        let water = world.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "0".into())]),
        });
        Arc::make_mut(
            Arc::make_mut(world.chunks.get_mut(&pos).unwrap())
                .sections
                .get_mut(&0)
                .unwrap(),
        )
        .blocks
        .as_mut()
        .unwrap()
        .fill(stone);
        world.set_block_id(8, 8, 8, water).unwrap();
        let before = Arc::clone(&stage.world);
        stage.set_bubble_tags(Some(Arc::new(
            native_resources::fluid_tags::BubbleColumnTags::new(
                BTreeSet::from(["minecraft:stone".into()]),
                BTreeSet::new(),
            ),
        )));
        assert!(stage.poll(pos, 1).is_err());
        assert!(stage.finish().is_err());
        assert_eq!(before.block(8, 8, 8).unwrap().name, "minecraft:water");
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn contained_liquid_postprocessing_matches_java() {
        let actual = java_results(
            "GeneratedContainedLiquidPostprocessingOracle",
            include_str!("../../tests/java/GeneratedContainedLiquidPostprocessingOracle.java"),
        );
        let mut expected = Vec::new();
        for liquid in ["water", "lava"] {
            for occupy in [false, true] {
                for fast in [false, true] {
                    expected.push(format!(
                        "{} {fast}",
                        contained_liquid_case(liquid, 0, occupy)
                    ));
                }
            }
        }
        assert_eq!(actual, expected);
    }

    fn water_spread_trace(case: &str) -> Vec<String> {
        use native_resources::{heightmap_tags::HeightmapTags, packs::ResourceProvider};
        struct Tags;
        impl ResourceProvider for Tags {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path.ends_with("blocks_motion_in_heightmap.json")
                    || path.ends_with("blocks_motion_in_heightmap_no_leaves.json")
                {
                    return Ok(Some(
                        br#"{"values":["minecraft:stone","minecraft:sand","minecraft:stone_slab","minecraft:oak_slab","minecraft:oak_stairs","minecraft:oak_fence"]}"#.to_vec(),
                    ));
                }
                if path == "data/minecraft/tags/block/prevents_nearby_leaf_decay.json" {
                    return Ok(Some(br#"{"values":[]}"#.to_vec()));
                }
                if matches!(
                    path,
                    "data/minecraft/tags/block/fences.json"
                        | "data/minecraft/tags/block/wooden_fences.json"
                ) {
                    return Ok(Some(br#"{"values":["minecraft:oak_fence"]}"#.to_vec()));
                }
                if path == "data/minecraft/tags/block/shulker_boxes.json" {
                    return Ok(Some(br#"{"values":[]}"#.to_vec()));
                }
                Ok(None)
            }
        }
        let (mut stage, _, chunk) = fixture(false);
        let world = Arc::make_mut(&mut stage.world);
        let stone = world.block_id(0, 0, 0).unwrap();
        let water = world.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "0".into())]),
        });
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let sand = world.intern_state(BlockState {
            name: "minecraft:sand".into(),
            properties: BTreeMap::new(),
        });
        let dirt = world.intern_state(BlockState {
            name: "minecraft:dirt".into(),
            properties: BTreeMap::new(),
        });
        Arc::make_mut(
            Arc::make_mut(world.chunks.get_mut(&chunk).unwrap())
                .sections
                .get_mut(&0)
                .unwrap(),
        )
        .blocks
        .as_mut()
        .unwrap()
        .fill(stone);
        let origin_x = if case == "edge" { 15 } else { 8 };
        let east_chunk = ChunkPos { x: 1, z: 0 };
        if case == "edge" {
            let mut east = world.chunks[&chunk].as_ref().clone();
            east.position = east_chunk;
            world.chunks.insert(east_chunk, Arc::new(east));
        }
        world.set_block_id(origin_x, 8, 8, water).unwrap();
        match case {
            "down" | "down_sand" | "down_wet_slab" => {
                world.set_block_id(8, 7, 8, air).unwrap();
                if case == "down_sand" {
                    world.set_block_id(9, 7, 8, sand).unwrap();
                }
                if case == "down_wet_slab" {
                    let slab = world.intern_state(BlockState {
                        name: "minecraft:stone_slab".into(),
                        properties: BTreeMap::from([
                            ("type".into(), "bottom".into()),
                            ("waterlogged".into(), "true".into()),
                        ]),
                    });
                    world.set_block_id(9, 7, 8, slab).unwrap();
                }
            }
            "sides" => {
                for [x, y, z] in [[8, 8, 7], [8, 8, 9], [7, 8, 8], [9, 8, 8]] {
                    world.set_block_id(x, y, z, air).unwrap();
                }
            }
            "mixed" => {
                world.set_block_id(8, 7, 8, air).unwrap();
                for [x, y, z] in [[8, 8, 7], [8, 8, 9], [7, 8, 8]] {
                    world.set_block_id(x, y, z, water).unwrap();
                }
                world.set_block_id(9, 8, 8, air).unwrap();
            }
            "edge" => {
                world.set_block_id(16, 8, 8, air).unwrap();
            }
            "convert_on" | "convert_off" | "convert_bubble" => {
                world.set_block_id(9, 8, 8, air).unwrap();
                world.set_block_id(9, 8, 7, water).unwrap();
                if case == "convert_bubble" {
                    world.set_block_id(9, 7, 8, dirt).unwrap();
                }
            }
            "slab_bottom" | "slab_top" | "slab_bottom_off" | "slab_oak" => {
                let slab = world.intern_state(BlockState {
                    name: if case == "slab_oak" {
                        "minecraft:oak_slab"
                    } else {
                        "minecraft:stone_slab"
                    }
                    .into(),
                    properties: BTreeMap::from([
                        (
                            "type".into(),
                            if case == "slab_top" { "top" } else { "bottom" }.into(),
                        ),
                        ("waterlogged".into(), "false".into()),
                    ]),
                });
                world.set_block_id(9, 8, 8, slab).unwrap();
                world.set_block_id(9, 8, 7, water).unwrap();
            }
            "stair_bottom" | "stair_top" | "stair_bottom_off" => {
                let stair = world.intern_state(BlockState {
                    name: "minecraft:oak_stairs".into(),
                    properties: BTreeMap::from([
                        ("facing".into(), "north".into()),
                        (
                            "half".into(),
                            if case == "stair_top" { "top" } else { "bottom" }.into(),
                        ),
                        ("shape".into(), "straight".into()),
                        ("waterlogged".into(), "false".into()),
                    ]),
                });
                world.set_block_id(9, 8, 8, stair).unwrap();
                world.set_block_id(9, 8, 7, water).unwrap();
            }
            "marked_stair_dry" | "marked_stair_wet" | "marked_stair_side" | "marked_stair_down" => {
                let stair = world.intern_state(BlockState {
                    name: "minecraft:oak_stairs".into(),
                    properties: BTreeMap::from([
                        ("facing".into(), "north".into()),
                        (
                            "half".into(),
                            if case == "marked_stair_down" {
                                "top"
                            } else {
                                "bottom"
                            }
                            .into(),
                        ),
                        ("shape".into(), "straight".into()),
                        (
                            "waterlogged".into(),
                            (case != "marked_stair_dry").to_string(),
                        ),
                    ]),
                });
                world.set_block_id(8, 8, 8, stair).unwrap();
                if case == "marked_stair_side" {
                    world.set_block_id(9, 8, 8, air).unwrap();
                }
                if case == "marked_stair_down" {
                    world.set_block_id(8, 7, 8, air).unwrap();
                }
            }
            case if case.starts_with("marked_stair_corner_") => {
                let parts: Vec<_> = case["marked_stair_corner_".len()..].split('_').collect();
                assert_eq!(parts.len(), 5);
                let make_stair = |facing: &str, wet: bool| BlockState {
                    name: "minecraft:oak_stairs".into(),
                    properties: BTreeMap::from([
                        ("facing".into(), facing.into()),
                        ("half".into(), parts[2].into()),
                        ("shape".into(), "straight".into()),
                        ("waterlogged".into(), wet.to_string()),
                    ]),
                };
                let center = world.intern_state(make_stair(parts[0], parts[3] == "true"));
                let neighbor = world.intern_state(make_stair(parts[1], false));
                world.set_block_id(8, 8, 8, center).unwrap();
                let (dx, dz) = match parts[0] {
                    "north" => (0, -1),
                    "south" => (0, 1),
                    "west" => (-1, 0),
                    "east" => (1, 0),
                    _ => panic!("invalid facing"),
                };
                let sign = if parts[4] == "front" { 1 } else { -1 };
                world
                    .set_block_id(8 + sign * dx, 8, 8 + sign * dz, neighbor)
                    .unwrap();
            }
            case if case.starts_with("marked_stair_variant_") => {
                let parts: Vec<_> = case["marked_stair_variant_".len()..].split('_').collect();
                assert_eq!(parts.len(), 3);
                let stair = world.intern_state(BlockState {
                    name: "minecraft:oak_stairs".into(),
                    properties: BTreeMap::from([
                        ("facing".into(), parts[0].into()),
                        ("half".into(), parts[1].into()),
                        ("shape".into(), "straight".into()),
                        ("waterlogged".into(), parts[2].into()),
                    ]),
                });
                world.set_block_id(8, 8, 8, stair).unwrap();
            }
            "marked_slab_dry" | "marked_slab_wet" | "marked_slab_side" | "marked_slab_down" => {
                let slab = world.intern_state(BlockState {
                    name: "minecraft:stone_slab".into(),
                    properties: BTreeMap::from([
                        (
                            "type".into(),
                            if case == "marked_slab_down" {
                                "top"
                            } else {
                                "bottom"
                            }
                            .into(),
                        ),
                        (
                            "waterlogged".into(),
                            if case == "marked_slab_dry" {
                                "false"
                            } else {
                                "true"
                            }
                            .into(),
                        ),
                    ]),
                });
                world.set_block_id(8, 8, 8, slab).unwrap();
                if case == "marked_slab_side" {
                    world.set_block_id(9, 8, 8, air).unwrap();
                }
                if case == "marked_slab_down" {
                    world.set_block_id(8, 7, 8, air).unwrap();
                }
            }
            "marked_fence_wet"
            | "marked_fence_side"
            | "marked_fence_down"
            | "marked_fence_disconnect"
            | "marked_fence_disconnect_sand"
            | "marked_fence_disconnect_water"
            | "marked_fence_disconnect_neighbor" => {
                let fence = world.intern_state(BlockState {
                    name: "minecraft:oak_fence".into(),
                    properties: ["north", "east", "south", "west", "waterlogged"]
                        .into_iter()
                        .map(|key| {
                            (
                                key.into(),
                                (key == "waterlogged"
                                    || (key == "east"
                                        && case.starts_with("marked_fence_disconnect")))
                                .to_string(),
                            )
                        })
                        .collect(),
                });
                world.set_block_id(8, 8, 8, fence).unwrap();
                if case == "marked_fence_side" || case.starts_with("marked_fence_disconnect") {
                    world.set_block_id(9, 8, 8, air).unwrap();
                }
                if case == "marked_fence_down" {
                    world.set_block_id(8, 7, 8, air).unwrap();
                }
                if case == "marked_fence_disconnect_sand" {
                    world.set_block_id(8, 8, 7, sand).unwrap();
                }
                if case == "marked_fence_disconnect_water" {
                    world.set_block_id(8, 8, 7, water).unwrap();
                }
                if case == "marked_fence_disconnect_neighbor" {
                    let neighbor = world.intern_state(BlockState {
                        name: "minecraft:oak_fence".into(),
                        properties: ["north", "east", "south", "west", "waterlogged"]
                            .into_iter()
                            .map(|key| (key.into(), (key == "waterlogged").to_string()))
                            .collect(),
                    });
                    world.set_block_id(8, 8, 7, neighbor).unwrap();
                }
            }
            case if case.starts_with("marked_leaf_") => {
                let leaf = world.intern_state(BlockState {
                    name: format!(
                        "minecraft:{}",
                        case.strip_prefix("marked_leaf_type_")
                            .unwrap_or("oak_leaves")
                    ),
                    properties: BTreeMap::from([
                        ("distance".into(), "7".into()),
                        ("persistent".into(), "false".into()),
                        (
                            "waterlogged".into(),
                            (case != "marked_leaf_dry").to_string(),
                        ),
                    ]),
                });
                world.set_block_id(8, 8, 8, leaf).unwrap();
                if case == "marked_leaf_side" {
                    world.set_block_id(9, 8, 8, air).unwrap();
                }
                if case == "marked_leaf_down" {
                    world.set_block_id(8, 7, 8, air).unwrap();
                }
            }
            "flow_drain" | "flow_drain_sand" => {
                let flowing = world.intern_state(BlockState {
                    name: "minecraft:water".into(),
                    properties: BTreeMap::from([("level".into(), "1".into())]),
                });
                world.set_block_id(8, 8, 8, flowing).unwrap();
                if case == "flow_drain_sand" {
                    world.set_block_id(9, 8, 8, sand).unwrap();
                }
            }
            "flow_weaken" | "flow_falling" | "flow_down" | "flow_sides" | "origin_source_on"
            | "origin_source_off" => {
                let flowing = world.intern_state(BlockState {
                    name: "minecraft:water".into(),
                    properties: BTreeMap::from([("level".into(), "4".into())]),
                });
                world.set_block_id(8, 8, 8, flowing).unwrap();
                if case == "flow_falling" {
                    world.set_block_id(8, 9, 8, water).unwrap();
                } else {
                    world.set_block_id(7, 8, 8, water).unwrap();
                    if case == "flow_down" {
                        world.set_block_id(8, 7, 8, air).unwrap();
                    }
                    if case == "flow_sides" {
                        for [x, y, z] in [[8, 8, 7], [8, 8, 9], [9, 8, 8]] {
                            world.set_block_id(x, y, z, air).unwrap();
                        }
                    }
                    if case.starts_with("origin_source_") {
                        world.set_block_id(8, 8, 7, water).unwrap();
                    }
                }
            }
            _ => panic!("unknown water fixture"),
        }
        stage.source = Arc::clone(&stage.world);
        let source = Arc::clone(&stage.world);
        let mut marks = PostProcessing::new(0, 16).unwrap();
        marks.mark(origin_x, 8, 8);
        stage.region.tasks =
            BTreeMap::from([(chunk, PostprocessingTask::new(Arc::new(marks), chunk))]);
        let mut chunks = BTreeMap::from([(
            chunk,
            ConvertedChunkTicks {
                blocks: ChunkTicks::from_saved(chunk, vec![]).unwrap(),
                fluids: ChunkTicks::from_saved(chunk, vec![]).unwrap(),
            },
        )]);
        if case == "edge" {
            let empty = PostProcessing::new(0, 16).unwrap();
            stage.region.tasks.insert(
                east_chunk,
                PostprocessingTask::new(Arc::new(empty), east_chunk),
            );
            stage
                .block_entities
                .insert(east_chunk, ConvertedBlockEntities::default());
            chunks.insert(
                east_chunk,
                ConvertedChunkTicks {
                    blocks: ChunkTicks::from_saved(east_chunk, vec![]).unwrap(),
                    fluids: ChunkTicks::from_saved(east_chunk, vec![]).unwrap(),
                },
            );
        }
        stage.region.ticks = RegionTicks::new(chunks, 100, 2).unwrap();
        let mut registry: BTreeSet<String> = [
            "air",
            "water",
            "stone",
            "sand",
            "dirt",
            "stone_slab",
            "oak_slab",
            "oak_stairs",
            "oak_leaves",
            "oak_fence",
        ]
        .into_iter()
        .map(|name| format!("minecraft:{name}"))
        .collect();
        if let Some(name) = case.strip_prefix("marked_leaf_type_") {
            registry.insert(format!("minecraft:{name}"));
        }
        stage.heightmap_tags = Some(Arc::new(HeightmapTags::load(&mut Tags, &registry).unwrap()));
        let leaf_tag =
            native_resources::packs::ResourceId::parse("minecraft:prevents_nearby_leaf_decay")
                .unwrap();
        stage.set_leaf_tags(Some(Arc::new(
            native_resources::ore_feature::OreBlockTags::load(
                &mut Tags,
                &registry,
                &BTreeSet::from([leaf_tag]),
            )
            .unwrap(),
        )));
        let fence_tags = [
            "minecraft:fences",
            "minecraft:wooden_fences",
            "minecraft:shulker_boxes",
        ]
        .into_iter()
        .map(|id| native_resources::packs::ResourceId::parse(id).unwrap())
        .collect();
        stage.set_fence_tags(Some(Arc::new(
            native_resources::ore_feature::OreBlockTags::load(&mut Tags, &registry, &fence_tags)
                .unwrap(),
        )));
        stage.heightmaps.insert(chunk, fixture_maps(16));
        if case == "edge" {
            stage.heightmaps.insert(east_chunk, fixture_maps(16));
        }
        stage.set_bubble_tags(Some(Arc::new(
            native_resources::fluid_tags::BubbleColumnTags::new(
                if case == "convert_bubble" {
                    BTreeSet::from(["minecraft:dirt".into()])
                } else {
                    BTreeSet::new()
                },
                BTreeSet::new(),
            ),
        )));
        stage.set_fluid_tags(
            Some(Arc::new(BTreeSet::from([
                "minecraft:air".into(),
                "minecraft:water".into(),
            ]))),
            Some(Arc::new(BTreeSet::from([
                "minecraft:lava".into(),
                "minecraft:flowing_lava".into(),
            ]))),
        );
        if matches!(
            case,
            "convert_on"
                | "convert_off"
                | "convert_bubble"
                | "origin_source_on"
                | "origin_source_off"
                | "slab_bottom"
                | "slab_top"
                | "slab_bottom_off"
                | "slab_oak"
                | "stair_bottom"
                | "stair_top"
                | "stair_bottom_off"
        ) {
            stage
                .set_water_source_conversion(!matches!(
                    case,
                    "convert_off" | "origin_source_off" | "slab_bottom_off" | "stair_bottom_off"
                ))
                .unwrap();
        }
        while !stage.poll(chunk, 1).unwrap() {}
        if case == "edge" {
            while !stage.poll(east_chunk, 1).unwrap() {}
        }
        let result = stage.finish().unwrap();
        assert_eq!(
            result.water_source_conversion(),
            match case {
                "convert_off" | "origin_source_off" | "slab_bottom_off" | "stair_bottom_off" =>
                    Some(false),
                "convert_on" | "convert_bubble" | "origin_source_on" | "slab_bottom"
                | "slab_top" | "slab_oak" | "stair_bottom" | "stair_top" => Some(true),
                _ => None,
            }
        );
        let expected_checks: Vec<_> = result
            .edits
            .iter()
            .copied()
            .filter(|p| {
                let old = source.block(p.x, p.y, p.z).unwrap();
                let new = result.world.block(p.x, p.y, p.z).unwrap();
                !native_world::block_light::same_light_behavior(old, new).unwrap()
            })
            .collect();
        assert_eq!(result.light_checks, expected_checks);
        let mut trace = vec![format!("case {case}")];
        for [x, y, z] in [
            [origin_x, 8, 8],
            [origin_x, 7, 8],
            [origin_x, 8, 7],
            [origin_x, 8, 9],
            [origin_x - 1, 8, 8],
            [origin_x + 1, 8, 8],
        ] {
            let state = result.world.block(x, y, z).unwrap();
            trace.push(format!(
                "state {x},{y},{z} {} {}",
                state.name,
                state
                    .properties
                    .get("level")
                    .or_else(|| state.properties.get("waterlogged"))
                    .map_or("-", String::as_str)
            ));
            if state.name == "minecraft:oak_stairs" {
                trace.push(format!(
                    "stair {x},{y},{z} {} {} {}",
                    state.properties["facing"], state.properties["half"], state.properties["shape"]
                ));
            }
            if state.name == "minecraft:oak_fence" {
                trace.push(format!(
                    "fence {x},{y},{z} {} {} {} {}",
                    state.properties["north"],
                    state.properties["east"],
                    state.properties["south"],
                    state.properties["west"]
                ));
            }
        }
        if case.starts_with("marked_stair_corner_") {
            for check in &result.light_checks {
                trace.push(format!("light {},{},{}", check.x, check.y, check.z));
            }
        }
        for change in &result.edits {
            assert_ne!(
                source.block(change.x, change.y, change.z),
                result.world.block(change.x, change.y, change.z)
            );
        }
        for kind in [TickRegistry::Block, TickRegistry::Fluid] {
            for tick_chunk in if case == "edge" {
                vec![chunk, east_chunk]
            } else {
                vec![chunk]
            } {
                let ticks = result.ticks.chunk(tick_chunk).unwrap();
                let (label, saved) = match kind {
                    TickRegistry::Block => ("block", ticks.blocks.pack(100)),
                    TickRegistry::Fluid => ("fluid", ticks.fluids.pack(100)),
                };
                for tick in saved {
                    trace.push(format!(
                        "{label} {} {},{},{} {}",
                        tick.kind, tick.position[0], tick.position[1], tick.position[2], tick.delay
                    ));
                }
            }
        }
        trace.push(format!("counter {}", result.ticks.next_sub_tick()));
        trace
    }

    #[test]
    fn downward_water_spread_updates_world_ticks_and_light_checks() {
        let plain = water_spread_trace("down");
        assert!(
            plain
                .iter()
                .any(|line| line == "fluid minecraft:flowing_water 8,7,8 5")
        );
        assert!(
            plain
                .iter()
                .any(|line| line == "fluid minecraft:water 8,8,8 5")
        );
        let sand = water_spread_trace("down_sand");
        assert!(
            sand.iter()
                .any(|line| line == "block minecraft:sand 9,7,8 2")
        );
    }

    #[test]
    fn water_spreads_to_sides_and_after_a_downward_write() {
        let sides = water_spread_trace("sides");
        for p in ["8,8,7", "8,8,9", "7,8,8", "9,8,8"] {
            assert!(
                sides
                    .iter()
                    .any(|line| line == &format!("state {p} minecraft:water 1"))
            );
        }
        let mixed = water_spread_trace("mixed");
        assert!(
            mixed
                .iter()
                .any(|line| line == "state 8,7,8 minecraft:water 8")
        );
        assert!(
            mixed
                .iter()
                .any(|line| line == "state 9,8,8 minecraft:water 1")
        );
        let edge = water_spread_trace("edge");
        assert!(
            edge.iter()
                .any(|line| line == "state 16,8,8 minecraft:water 1")
        );
        assert!(
            edge.iter()
                .any(|line| line == "fluid minecraft:flowing_water 16,8,8 5")
        );
        let source = water_spread_trace("convert_on");
        assert!(
            source
                .iter()
                .any(|line| line == "state 9,8,8 minecraft:water 0")
        );
        assert!(
            source
                .iter()
                .any(|line| line == "fluid minecraft:water 9,8,8 5")
        );
        let flow = water_spread_trace("convert_off");
        assert!(
            flow.iter()
                .any(|line| line == "state 9,8,8 minecraft:water 1")
        );
        assert!(
            flow.iter()
                .any(|line| line == "fluid minecraft:flowing_water 9,8,8 5")
        );
        let bubble = water_spread_trace("convert_bubble");
        assert!(
            bubble
                .iter()
                .any(|line| line == "block minecraft:water 9,8,8 20")
        );
    }

    #[test]
    fn water_spread_rejects_unverified_datapack_replacement_tags() {
        let (mut stage, _, chunk) = fixture(false);
        let world = Arc::make_mut(&mut stage.world);
        let stone = world.block_id(0, 0, 0).unwrap();
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let water = world.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "0".into())]),
        });
        Arc::make_mut(
            Arc::make_mut(world.chunks.get_mut(&chunk).unwrap())
                .sections
                .get_mut(&0)
                .unwrap(),
        )
        .blocks
        .as_mut()
        .unwrap()
        .fill(stone);
        world.set_block_id(8, 8, 8, water).unwrap();
        world.set_block_id(8, 7, 8, air).unwrap();
        let original = Arc::clone(&stage.world);
        stage.set_fluid_tags(
            Some(Arc::new(BTreeSet::from(["minecraft:air".into()]))),
            Some(Arc::new(BTreeSet::from(["minecraft:lava".into()]))),
        );
        let error = stage.poll(chunk, 1).unwrap_err();
        assert!(format!("{error:#}").contains("generated water path is incomplete"));
        assert!(stage.finish().is_err());
        assert_eq!(original.block(8, 7, 8).unwrap().name, "minecraft:air");
    }

    #[test]
    fn water_spread_rejects_unknown_source_conversion_rule() {
        let (mut stage, _, chunk) = fixture(false);
        let world = Arc::make_mut(&mut stage.world);
        let stone = world.block_id(0, 0, 0).unwrap();
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let water = world.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "0".into())]),
        });
        Arc::make_mut(
            Arc::make_mut(world.chunks.get_mut(&chunk).unwrap())
                .sections
                .get_mut(&0)
                .unwrap(),
        )
        .blocks
        .as_mut()
        .unwrap()
        .fill(stone);
        world.set_block_id(8, 8, 8, water).unwrap();
        world.set_block_id(9, 8, 8, air).unwrap();
        world.set_block_id(9, 8, 7, water).unwrap();
        let original = Arc::clone(&stage.world);
        let error = stage.poll(chunk, 1).unwrap_err();
        assert!(format!("{error:#}").contains("depends on unavailable settings"));
        assert!(stage.finish().is_err());
        assert_eq!(original.block(9, 8, 8).unwrap().name, "minecraft:air");
    }

    #[test]
    fn source_conversion_rule_is_fixed_before_chunk_processing() {
        let (mut stage, _, pos) = fixture(false);
        stage.set_water_source_conversion(false).unwrap();
        assert!(!stage.poll(pos, 0).unwrap());
        assert!(stage.set_water_source_conversion(true).is_err());
    }

    #[test]
    fn water_fills_slabs_only_when_a_source_forms() {
        let bottom = water_spread_trace("slab_bottom");
        assert!(
            bottom
                .iter()
                .any(|line| line == "state 9,8,8 minecraft:stone_slab true")
        );
        assert!(
            bottom
                .iter()
                .any(|line| line == "fluid minecraft:water 9,8,8 5")
        );
        let disabled = water_spread_trace("slab_bottom_off");
        assert!(
            disabled
                .iter()
                .any(|line| line == "state 9,8,8 minecraft:stone_slab false")
        );
        let oak = water_spread_trace("slab_oak");
        assert!(
            oak.iter()
                .any(|line| line == "state 9,8,8 minecraft:oak_slab true")
        );
        let wet_neighbor = water_spread_trace("down_wet_slab");
        assert!(
            wet_neighbor
                .iter()
                .any(|line| line == "fluid minecraft:water 9,7,8 5")
        );
    }

    #[test]
    fn marked_waterlogged_slabs_tick_and_update_neighbor_shapes() {
        let dry = water_spread_trace("marked_slab_dry");
        assert!(!dry.iter().any(|line| line.starts_with("fluid ")));
        let wet = water_spread_trace("marked_slab_wet");
        assert!(
            wet.iter()
                .any(|line| line == "fluid minecraft:water 8,8,8 5")
        );
        let side = water_spread_trace("marked_slab_side");
        assert!(
            side.iter()
                .any(|line| line == "state 9,8,8 minecraft:water 1")
        );
        let down = water_spread_trace("marked_slab_down");
        assert!(
            down.iter()
                .any(|line| line == "state 8,7,8 minecraft:water 8")
        );
    }

    #[test]
    fn marked_waterlogged_stairs_and_water_spread_preserve_stair_state() {
        for case in [
            "stair_bottom",
            "stair_top",
            "stair_bottom_off",
            "marked_stair_dry",
            "marked_stair_wet",
            "marked_stair_side",
            "marked_stair_down",
        ] {
            let trace = water_spread_trace(case);
            assert!(
                trace
                    .iter()
                    .any(|line| line.contains("minecraft:oak_stairs")),
                "{case}"
            );
            if case == "marked_stair_wet" {
                assert!(
                    trace
                        .iter()
                        .any(|line| line == "fluid minecraft:water 8,8,8 5")
                );
            }
        }
    }

    #[test]
    fn marked_waterlogged_fences_spread_water_and_preserve_the_fence() {
        for (case, expected) in [
            ("marked_fence_wet", None),
            ("marked_fence_side", Some("state 9,8,8 minecraft:water 1")),
            ("marked_fence_down", Some("state 8,7,8 minecraft:water 8")),
            (
                "marked_fence_disconnect",
                Some("state 9,8,8 minecraft:water 1"),
            ),
            (
                "marked_fence_disconnect_sand",
                Some("state 9,8,8 minecraft:water 1"),
            ),
            (
                "marked_fence_disconnect_water",
                Some("state 9,8,8 minecraft:water 1"),
            ),
            (
                "marked_fence_disconnect_neighbor",
                Some("state 9,8,8 minecraft:water 1"),
            ),
        ] {
            let trace = water_spread_trace(case);
            assert!(
                trace
                    .iter()
                    .any(|line| line == "state 8,8,8 minecraft:oak_fence true")
            );
            if let Some(expected) = expected {
                assert!(
                    trace.iter().any(|line| line == expected),
                    "{case}: {trace:?}"
                );
            }
            if case.starts_with("marked_fence_disconnect") {
                let north = case != "marked_fence_disconnect_water";
                assert!(
                    trace
                        .iter()
                        .any(|line| line == &format!("fence 8,8,8 {north} false true true")),
                    "{case}: {trace:?}"
                );
            }
            if case == "marked_fence_disconnect_sand" {
                assert!(
                    trace
                        .iter()
                        .any(|line| line == "block minecraft:sand 8,8,7 2")
                );
            }
            if case == "marked_fence_disconnect_neighbor" {
                assert!(
                    trace
                        .iter()
                        .any(|line| line == "fence 8,8,7 false false true false")
                );
            }
        }
    }

    #[test]
    fn postprocessed_stair_corners_update_shape_and_queue_light() {
        let outer = water_spread_trace("marked_stair_corner_north_west_bottom_false_front");
        assert!(
            outer
                .iter()
                .any(|line| line == "stair 8,8,8 north bottom outer_left")
        );
        assert!(outer.iter().any(|line| line == "light 8,8,8"));
        let inner = water_spread_trace("marked_stair_corner_north_west_bottom_false_back");
        assert!(
            inner
                .iter()
                .any(|line| line == "stair 8,8,8 north bottom inner_left")
        );
        assert!(inner.iter().any(|line| line == "light 8,8,8"));
    }

    #[test]
    fn marked_waterlogged_leaves_schedule_fluid_and_leaf_ticks_without_spreading() {
        let dry = water_spread_trace("marked_leaf_dry");
        assert!(!dry.iter().any(|line| line.starts_with("fluid ")));
        assert!(
            dry.iter()
                .any(|line| line == "block minecraft:oak_leaves 8,8,8 1")
        );
        assert_eq!(dry.last().unwrap(), "counter 8");
        for case in ["marked_leaf_wet", "marked_leaf_side", "marked_leaf_down"] {
            let trace = water_spread_trace(case);
            assert!(
                trace
                    .iter()
                    .any(|line| line == "fluid minecraft:water 8,8,8 5")
            );
            assert!(
                trace
                    .iter()
                    .any(|line| line == "block minecraft:oak_leaves 8,8,8 1")
            );
            assert_eq!(trace.last().unwrap(), "counter 14");
            if case == "marked_leaf_side" {
                assert!(
                    trace
                        .iter()
                        .any(|line| line == "state 9,8,8 minecraft:air -")
                );
            }
            if case == "marked_leaf_down" {
                assert!(
                    trace
                        .iter()
                        .any(|line| line == "state 8,7,8 minecraft:air -")
                );
            }
        }
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn water_source_postprocessing_matches_java() {
        let actual = java_results(
            "GeneratedWaterSpreadPostprocessingOracle",
            include_str!("../../tests/java/GeneratedWaterSpreadPostprocessingOracle.java"),
        );
        let mut expected = Vec::new();
        for case in [
            "down",
            "down_sand",
            "sides",
            "mixed",
            "edge",
            "convert_off",
            "convert_on",
            "convert_bubble",
            "flow_drain",
            "flow_drain_sand",
            "flow_weaken",
            "flow_falling",
            "flow_down",
            "flow_sides",
            "origin_source_off",
            "origin_source_on",
            "slab_bottom",
            "slab_top",
            "slab_bottom_off",
            "slab_oak",
            "down_wet_slab",
            "marked_slab_dry",
            "marked_slab_wet",
            "marked_slab_side",
            "marked_slab_down",
            "stair_bottom",
            "stair_top",
            "stair_bottom_off",
            "marked_stair_dry",
            "marked_stair_wet",
            "marked_stair_side",
            "marked_stair_down",
            "marked_leaf_dry",
            "marked_leaf_wet",
            "marked_leaf_side",
            "marked_leaf_down",
            "marked_fence_wet",
            "marked_fence_side",
            "marked_fence_down",
            "marked_fence_disconnect",
            "marked_fence_disconnect_sand",
            "marked_fence_disconnect_water",
            "marked_fence_disconnect_neighbor",
        ] {
            expected.extend(water_spread_trace(case));
        }
        for name in [
            "oak_leaves",
            "spruce_leaves",
            "birch_leaves",
            "jungle_leaves",
            "acacia_leaves",
            "dark_oak_leaves",
            "mangrove_leaves",
            "cherry_leaves",
            "pale_oak_leaves",
            "azalea_leaves",
            "flowering_azalea_leaves",
        ] {
            expected.extend(water_spread_trace(&format!("marked_leaf_type_{name}")));
        }
        for facing in ["north", "south", "west", "east"] {
            for half in ["bottom", "top"] {
                for wet in [false, true] {
                    expected.extend(water_spread_trace(&format!(
                        "marked_stair_variant_{facing}_{half}_{wet}"
                    )));
                }
            }
        }
        for facing in ["north", "south", "west", "east"] {
            for neighbor in ["north", "south", "west", "east"] {
                for half in ["bottom", "top"] {
                    for wet in [false, true] {
                        for side in ["front", "back"] {
                            expected.extend(water_spread_trace(&format!(
                                "marked_stair_corner_{facing}_{neighbor}_{half}_{wet}_{side}"
                            )));
                        }
                    }
                }
            }
        }
        assert_eq!(actual, expected);
    }

    #[test]
    fn flowing_water_recalculates_origin_and_schedules_callbacks() {
        let drained = water_spread_trace("flow_drain");
        assert!(
            drained
                .iter()
                .any(|line| line == "state 8,8,8 minecraft:air -")
        );
        let sand = water_spread_trace("flow_drain_sand");
        assert!(
            sand.iter()
                .any(|line| line == "block minecraft:sand 9,8,8 2")
        );
        let weaker = water_spread_trace("flow_weaken");
        assert!(
            weaker
                .iter()
                .any(|line| line == "state 8,8,8 minecraft:water 1")
        );
        let falling = water_spread_trace("flow_falling");
        assert!(
            falling
                .iter()
                .any(|line| line == "state 8,8,8 minecraft:water 8")
        );
        let down = water_spread_trace("flow_down");
        assert!(
            down.iter()
                .any(|line| line == "state 8,7,8 minecraft:water 8")
        );
        let sides = water_spread_trace("flow_sides");
        for pos in ["8,8,7", "8,8,9", "9,8,8"] {
            assert!(
                sides
                    .iter()
                    .any(|line| line == &format!("state {pos} minecraft:water 2"))
            );
        }
        let source = water_spread_trace("origin_source_on");
        assert!(
            source
                .iter()
                .any(|line| line == "state 8,8,8 minecraft:water 0")
        );
        let flowing = water_spread_trace("origin_source_off");
        assert!(
            flowing
                .iter()
                .any(|line| line == "state 8,8,8 minecraft:water 1")
        );
    }

    #[test]
    fn flowing_water_rejects_unavailable_rules_and_section_deallocation() {
        let (mut stage, _, chunk) = fixture(false);
        let world = Arc::make_mut(&mut stage.world);
        let stone = world.block_id(0, 0, 0).unwrap();
        let source = world.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "0".into())]),
        });
        let flowing = world.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "4".into())]),
        });
        Arc::make_mut(
            Arc::make_mut(world.chunks.get_mut(&chunk).unwrap())
                .sections
                .get_mut(&0)
                .unwrap(),
        )
        .blocks
        .as_mut()
        .unwrap()
        .fill(stone);
        world.set_block_id(8, 8, 8, flowing).unwrap();
        world.set_block_id(7, 8, 8, source).unwrap();
        world.set_block_id(8, 8, 7, source).unwrap();
        let original = Arc::clone(&stage.world);
        let error = stage.poll(chunk, 1).unwrap_err();
        assert!(format!("{error:#}").contains("depends on unavailable settings"));
        assert!(stage.finish().is_err());
        assert_eq!(original.block(8, 8, 8).unwrap().properties["level"], "4");

        let (mut stage, _, chunk) = fixture(false);
        let world = Arc::make_mut(&mut stage.world);
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let flowing = world.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "1".into())]),
        });
        Arc::make_mut(
            Arc::make_mut(world.chunks.get_mut(&chunk).unwrap())
                .sections
                .get_mut(&0)
                .unwrap(),
        )
        .blocks
        .as_mut()
        .unwrap()
        .fill(air);
        world.set_block_id(8, 8, 8, flowing).unwrap();
        let original = Arc::clone(&stage.world);
        stage.set_fluid_tags(
            Some(Arc::new(BTreeSet::from([
                "minecraft:air".into(),
                "minecraft:water".into(),
            ]))),
            Some(Arc::new(BTreeSet::from(["minecraft:lava".into()]))),
        );
        let error = stage.poll(chunk, 1).unwrap_err();
        assert!(format!("{error:#}").contains("light section allocation update"));
        assert!(stage.finish().is_err());
        assert_eq!(original.block(8, 8, 8).unwrap().properties["level"], "1");
    }

    #[test]
    fn water_edits_refresh_direct_sky_sources_without_touching_the_input() {
        use native_world::{block_light::LightBounds, sky_light::generated_chunk_sky_sources};
        let (mut stage, _, pos) = fixture(false);
        let world = Arc::make_mut(&mut stage.world);
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let water = world.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "8".into())]),
        });
        Arc::make_mut(
            Arc::make_mut(world.chunks.get_mut(&pos).unwrap())
                .sections
                .get_mut(&0)
                .unwrap(),
        )
        .blocks
        .as_mut()
        .unwrap()
        .fill(air);
        let before = Arc::clone(&stage.world);
        let bounds = LightBounds::new(0, 16).unwrap();
        let original_heights =
            Arc::from(generated_chunk_sky_sources(&before, bounds, pos).unwrap());
        let original = BTreeMap::from([(pos, original_heights)]);
        Arc::make_mut(&mut stage.world)
            .set_block_id(8, 7, 8, water)
            .unwrap();
        let checks = [BlockPosition { x: 8, y: 7, z: 8 }];
        let updated = super::super::terrain_batch::updated_postprocessing_sky_sources(
            &before,
            &stage.world,
            &checks,
            bounds,
            &original,
        )
        .unwrap()
        .unwrap();
        let expected = generated_chunk_sky_sources(&stage.world, bounds, pos).unwrap();
        assert_eq!(updated[&pos].as_ref(), expected.as_ref());
        assert_ne!(updated[&pos][8 + 8 * 16], original[&pos][8 + 8 * 16]);
        assert!(
            super::super::terrain_batch::updated_postprocessing_sky_sources(
                &before,
                &stage.world,
                &checks,
                bounds,
                &BTreeMap::new(),
            )
            .is_err()
        );
        assert_eq!(before.block(8, 7, 8).unwrap().name, "minecraft:air");
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn generated_leaf_postprocessing_matches_java() {
        let actual = java_results(
            "GeneratedLeafPostprocessingOracle",
            include_str!("../../tests/java/GeneratedLeafPostprocessingOracle.java"),
        );
        let mut expected = Vec::new();
        for name in [
            "oak_leaves",
            "spruce_leaves",
            "birch_leaves",
            "jungle_leaves",
            "acacia_leaves",
            "dark_oak_leaves",
            "mangrove_leaves",
            "cherry_leaves",
            "pale_oak_leaves",
            "azalea_leaves",
            "flowering_azalea_leaves",
        ] {
            for distance in [1, 4, 7] {
                for tagged in [false, true] {
                    expected.push(leaf_case(name, distance, tagged, 1));
                }
            }
        }
        assert_eq!(actual, expected);
    }

    #[test]
    fn drained_mushroom_light_preserves_prior_revision_and_requires_both_channels() {
        use native_world::{
            block_light::LightBounds, generation_light::VisibleLightColumn, loaded::LIGHT_BYTES,
        };
        let pos = ChunkPos { x: 0, z: 0 };
        let p = [8, 8, 8];
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        world.chunks.insert(
            pos,
            Arc::new(Chunk {
                position: pos,
                status: Some("minecraft:spawn".into()),
                block_light_correct: false,
                sections: BTreeMap::from([(
                    0,
                    Arc::new(Section {
                        blocks: Some(Box::new([air; BLOCK_COUNT])),
                        biomes: None,
                        block_light: None,
                        sky_light: None,
                    }),
                )]),
            }),
        );
        let mut layer = [0; LIGHT_BYTES];
        let index = 8 + 8 * 16 + 8 * 256;
        layer[index / 2] = 1;
        let column =
            VisibleLightColumn::new(BTreeMap::from([(0, Arc::new(layer))]), BTreeMap::new());
        let mut before = VisibleLightSnapshot::new(true, false);
        before.insert_complete_column(pos, column.clone());
        let bounds = LightBounds::new(0, 16).unwrap();
        let active = BTreeSet::from([pos]);
        let after =
            drain_postprocessing_light(&world, bounds, &before, &active, &BTreeMap::new(), &[p, p])
                .unwrap();
        assert_eq!(before.block_light(p), Some(1));
        assert_eq!(after.block_light(p), Some(0));
        assert_eq!(after.block_section_allocated(pos, 0), Some(true));

        let mut both = VisibleLightSnapshot::new(true, true);
        both.insert_complete_column(
            pos,
            VisibleLightColumn::new(
                BTreeMap::from([(0, Arc::new(layer))]),
                BTreeMap::from([(0, Arc::new(layer))]),
            ),
        );
        let after_both = drain_postprocessing_light(
            &world,
            bounds,
            &both,
            &BTreeSet::new(),
            &BTreeMap::new(),
            &[p],
        )
        .unwrap();
        assert_eq!(after_both.block_light(p), Some(0));
        assert_eq!(after_both.sky_light(p), Some(0));
        assert_eq!(both.block_light(p), Some(1));
        assert_eq!(both.sky_light(p), Some(1));

        let mut missing_sky = VisibleLightSnapshot::new(true, true);
        missing_sky.insert_complete_column(pos, column);
        assert!(
            drain_postprocessing_light(
                &world,
                bounds,
                &missing_sky,
                &active,
                &BTreeMap::new(),
                &[p],
            )
            .is_err()
        );
        assert_eq!(missing_sky.block_light(p), Some(1));
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

    fn java_results(class_name: &str, java_source: &str) -> Vec<String> {
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
        let source = temp.path().join(format!("{class_name}.java"));
        std::fs::write(&source, java_source).unwrap();
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
            .arg(class_name)
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
        lines[start + 1..end]
            .iter()
            .map(|line| (*line).to_owned())
            .collect()
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn generated_fence_postprocessing_matches_java() {
        let actual = java_results(
            "GeneratedFencePostprocessingOracle",
            include_str!("../../tests/java/GeneratedFencePostprocessingOracle.java"),
        );
        let expected: Vec<_> = [
            "mixed",
            "same_fence",
            "other_fence",
            "tag_air",
            "wet_enclosed",
            "gate_north",
            "gate_east",
        ]
        .into_iter()
        .map(fence_postprocessing_trace)
        .collect();
        assert_eq!(actual, expected);
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn inert_natural_cubes_match_java_postprocessing() {
        let actual = java_results(
            "GeneratedInertPostprocessingOracle",
            include_str!("../../tests/java/GeneratedInertPostprocessingOracle.java"),
        );
        let mut expected = inert_cube_postprocessing_trace();
        expected.extend(inert_pillar_postprocessing_trace());
        assert_eq!(actual, expected);
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn generated_falling_postprocessing_matches_java() {
        let actual = java_results(
            "GeneratedFallingPostprocessingOracle",
            include_str!("../../tests/java/GeneratedFallingPostprocessingOracle.java"),
        );
        assert_eq!(actual, result_trace(1));
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn generated_mushroom_postprocessing_matches_java() {
        let actual = java_results(
            "GeneratedMushroomPostprocessingOracle",
            include_str!("../../tests/java/GeneratedMushroomPostprocessingOracle.java"),
        );
        let mut expected = Vec::new();
        for mushroom in ["brown_mushroom", "red_mushroom"] {
            for support in [
                "stone",
                "dirt",
                "grass_block",
                "podzol",
                "mycelium",
                "crimson_nylium",
                "warped_nylium",
                "water",
                "air",
                "chest",
                "spawner",
                "deepslate",
            ] {
                for tagged in [false, true] {
                    for brightness in [0, 12, 13, 15] {
                        expected.push(mushroom_case(mushroom, support, tagged, brightness));
                    }
                }
            }
        }
        assert_eq!(actual, expected);
    }
}
