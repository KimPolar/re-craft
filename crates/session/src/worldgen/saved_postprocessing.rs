//! Isolated postprocessing inputs restored from saved FULL chunks.
//!
//! The caller must establish Java holder readiness and later reconcile live
//! state, light, entities, POIs, clocks and disk sources before publishing.

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use native_persistence::{
    heightmap::{FullHeightmaps, HeightBounds},
    nbt::{NamedTag, Tag},
    world::{ChunkArea, DimensionId, TARGET_DATA_VERSION},
};
use native_resources::packs::{ResourceId, ResourceProvider};
use native_world::{
    generation_light::VisibleLightSnapshot,
    loaded::{BlockState, ChunkPos, SavedBlockEntities},
};

use crate::{
    loaded::BuildBounds,
    saved_chunk_activation::{SavedTickInputs, read_saved_work},
    ticking_preparation::{TickingFutureState, TickingPreparation, ticking_dependencies},
};

use super::{
    GeneratedPostprocessing,
    captured_postprocessing::CapturedPostprocessingTags,
    chunk_ticks::{
        ChunkTicks, ConvertedChunkTicks, IndexedRegionQueue, RegionTicks, SavedTick, TickRegistry,
    },
    postprocessing::{ConvertedPostprocessingRegion, PostprocessingTask},
    postprocessing_world::{ConvertedBlockEntities, PostprocessedGeneratedRegion},
};

/// Inputs captured from the same world and active resource/data-pack stack.
/// Registration is the actual LevelTicks registration order, not proof that
/// any holder has completed its ticking future.
pub struct SavedPostprocessingInputs<'a> {
    pub area: &'a ChunkArea,
    pub dimension: &'a DimensionId,
    pub defaults: &'a BTreeMap<String, BTreeMap<String, String>>,
    pub block_ids: &'a BTreeSet<String>,
    pub bounds: BuildBounds,
    pub registration: &'a [ChunkPos],
    pub game_time: i64,
    pub next_sub_tick: i64,
    pub light: Option<VisibleLightSnapshot>,
}

/// One completed isolated callback and the LevelTicks indexes after unpacking
/// only the center. Other converted chunks retain their pending saved ticks.
pub struct SavedTickingResult {
    pub region: PostprocessedGeneratedRegion,
    pub block_ticks: IndexedRegionQueue,
    pub fluid_ticks: IndexedRegionQueue,
}

impl SavedTickingResult {
    /// Create marks-only drafts for completed callbacks, using the same saved
    /// inputs as the isolated pass. The drafts cannot be saved: block edits,
    /// ticks, lighting, entities, POIs and live state still need reconciliation.
    /// Run away from the UI thread because every captured source is checked.
    pub fn draft_completed_postprocessing_marks(
        &self,
        area: &ChunkArea,
        positions: impl IntoIterator<Item = ChunkPos>,
    ) -> Result<BTreeMap<ChunkPos, native_persistence::edit::ChunkDraft>> {
        let region = &self.region;
        ensure!(
            Arc::ptr_eq(&region.world, &region.sealed_world)
                && region.edits == region.sealed_edits
                && region.edits.len() == region.edit_states.len()
                && region.read_chunks == region.sealed_read_chunks
                && region.light_reads == region.sealed_light_reads
                && region.light_checks == region.sealed_checks
                && region.chunks == region.sealed_chunks,
            "saved postprocessing result was modified"
        );
        ensure!(
            !region.chunks.is_empty(),
            "no completed saved postprocessing"
        );
        ensure!(
            area.chunks.len() == region.source.chunks.len(),
            "saved postprocessing source chunk set changed"
        );
        for &pos in region.source.chunks.keys() {
            let saved_pos = native_persistence::world::ChunkPos { x: pos.x, z: pos.z };
            let source = area
                .chunks
                .get(&saved_pos)
                .context("saved postprocessing source chunk is missing")?;
            ensure!(
                source.position == saved_pos
                    && region.source.saved_chunk_source(pos)
                        == Some(native_persistence::nbt::fingerprint(&source.document)?),
                "saved postprocessing source changed at {pos:?}"
            );
        }
        let height = region
            .max_y
            .checked_sub(region.min_y)
            .context("invalid postprocessing bounds")?;
        let mut drafts = BTreeMap::new();
        for pos in positions {
            ensure!(region.chunks.contains(&pos), "chunk was not postprocessed");
            ensure!(!drafts.contains_key(&pos), "duplicate postprocessed chunk");
            let saved_pos = native_persistence::world::ChunkPos { x: pos.x, z: pos.z };
            let source = area
                .chunks
                .get(&saved_pos)
                .context("postprocessed source chunk is missing")?;
            let expected = unique_field(&source.document.value, "PostProcessing")?;
            let mut draft =
                native_persistence::edit::draft_chunk_blocks(&source.document, saved_pos, &[])?;
            draft.stage_completed_postprocessing(region.min_y, height, expected)?;
            drafts.insert(pos, draft);
        }
        ensure!(!drafts.is_empty(), "no postprocessed chunks selected");
        Ok(drafts)
    }
}

/// An isolated sequence of holder callbacks at one unchanged level time.
/// Send-sync is treated as pending, so no chunk notification is delivered.
/// Run away from the UI thread; the caller must prevent live interleaving and
/// revalidate every dependency before attaching the result to a session.
pub struct SavedTickingBatch {
    stage: GeneratedPostprocessing,
    available: BTreeSet<ChunkPos>,
    game_time: i64,
    started: bool,
    failed: bool,
}

impl SavedTickingBatch {
    /// Capture all FULL inputs and configure the same environment for every
    /// callback. No holder future has been completed by this operation.
    pub fn begin(
        provider: &mut impl ResourceProvider,
        input: SavedPostprocessingInputs<'_>,
        validate_state: impl FnMut(&BlockState) -> Result<()>,
        configure: impl FnOnce(&mut GeneratedPostprocessing) -> Result<()>,
    ) -> Result<Self> {
        let available = input
            .area
            .chunks
            .keys()
            .map(|pos| ChunkPos { x: pos.x, z: pos.z })
            .collect();
        let game_time = input.game_time;
        let mut stage = begin_saved_postprocessing(provider, input, validate_state)?;
        configure(&mut stage)?;
        Ok(Self {
            stage,
            available,
            game_time,
            started: false,
            failed: false,
        })
    }

    /// Dispatch one queued holder callback without interleaving another one.
    /// Pending saved ticks are unpacked after its postprocessing and become
    /// visible to later callbacks in this same captured level state.
    pub fn dispatch(&mut self, future: &mut TickingPreparation, game_time: i64) -> Result<bool> {
        ensure!(!self.failed, "saved ticking batch has failed");
        if game_time != self.game_time {
            self.failed = true;
            bail!("level time changed during saved ticking preparation");
        }
        let available = &self.available;
        let stage = RefCell::new(&mut self.stage);
        let started = std::cell::Cell::new(self.started);
        let result = future.dispatch(
            |center| {
                ensure!(
                    ticking_dependencies(center)
                        .iter()
                        .all(|position| available.contains(position)),
                    "saved ticking inputs lack a FULL neighbor"
                );
                let mut stage = stage.borrow_mut();
                if stage.postprocessing_completed(center) {
                    return Ok(());
                }
                if started.get() {
                    stage.request_converted_chunk(center)?;
                } else {
                    stage.select_initial_targets(BTreeSet::from([center]))?;
                    started.set(true);
                }
                while !stage.poll(center, 256)? {}
                Ok(())
            },
            |center| stage.borrow_mut().unpack_saved_ticks(center),
            |_| Ok(()),
        );
        self.started = started.get();
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    /// Finish the captured sequence. This alone does not authorize live use.
    pub fn finish(self) -> Result<SavedTickingResult> {
        ensure!(
            !self.failed && self.started,
            "no completed saved ticking callbacks"
        );
        let region = self.stage.finish()?;
        let block_ticks = region
            .ticks()
            .captured_indexed_queue_for(TickRegistry::Block, &[])?;
        let fluid_ticks = region
            .ticks()
            .captured_indexed_queue_for(TickRegistry::Fluid, &[])?;
        Ok(SavedTickingResult {
            region,
            block_ticks,
            fluid_ticks,
        })
    }
}

/// Execute a queued ticking-future callback over the captured FULL source.
/// The caller must sample the game time, registry, light and holder identities
/// at dispatch, prevent interleaving, and later reconcile all live and disk
/// state. Run this away from the UI thread. The send-sync future is treated as
/// pending in this isolated pass; this result cannot authorize live attachment,
/// a send notification, or a save.
pub fn dispatch_saved_ticking(
    future: &mut TickingPreparation,
    provider: &mut impl ResourceProvider,
    input: SavedPostprocessingInputs<'_>,
    mut validate_state: impl FnMut(&BlockState) -> Result<()>,
    mut configure: impl FnMut(&mut GeneratedPostprocessing) -> Result<()>,
) -> Result<Option<SavedTickingResult>> {
    let mut input = Some(input);
    let stage = RefCell::new(None::<GeneratedPostprocessing>);
    let result = RefCell::new(None::<SavedTickingResult>);
    let dispatched = future.dispatch(
        |center| {
            let input = input
                .take()
                .context("saved ticking input was already consumed")?;
            ensure!(
                ticking_dependencies(center).iter().all(|position| {
                    input
                        .area
                        .chunks
                        .contains_key(&native_persistence::world::ChunkPos {
                            x: position.x,
                            z: position.z,
                        })
                }),
                "saved ticking inputs lack a FULL neighbor"
            );
            let mut next = begin_saved_postprocessing(provider, input, &mut validate_state)?;
            next.select_initial_targets(BTreeSet::from([center]))?;
            configure(&mut next)?;
            while !next.poll(center, 256)? {}
            *stage.borrow_mut() = Some(next);
            Ok(())
        },
        |center| {
            stage
                .borrow_mut()
                .as_mut()
                .context("saved ticking postprocessing is missing")?
                .unpack_saved_ticks(center)
        },
        |_| {
            let region = stage
                .borrow_mut()
                .take()
                .context("saved ticking postprocessing is missing")?
                .finish()?;
            let blocks = region
                .ticks()
                .captured_indexed_queue_for(TickRegistry::Block, &[])?;
            let fluids = region
                .ticks()
                .captured_indexed_queue_for(TickRegistry::Fluid, &[])?;
            *result.borrow_mut() = Some(SavedTickingResult {
                region,
                block_ticks: blocks,
                fluid_ticks: fluids,
            });
            Ok(())
        },
    )?;
    if !dispatched || future.state() != TickingFutureState::Available {
        return Ok(None);
    }
    Ok(Some(
        result
            .into_inner()
            .context("saved ticking result is missing")?,
    ))
}

/// Restore an isolated postprocessing stage from original FULL chunk data.
/// Saved tick entries remain pending during callbacks. Reproducible default
/// chests, chests and brushable blocks with deferred loot, and default potent
/// sulfur entities are admitted. Active upgrade data and other contents are
/// rejected. This does not authorize a live commit or save.
pub fn begin_saved_postprocessing(
    provider: &mut impl ResourceProvider,
    input: SavedPostprocessingInputs<'_>,
    mut validate_state: impl FnMut(&BlockState) -> Result<()>,
) -> Result<GeneratedPostprocessing> {
    ensure!(
        !input.area.chunks.is_empty(),
        "no saved postprocessing chunks"
    );
    let height = input
        .bounds
        .max_y
        .checked_sub(input.bounds.min_y)
        .context("invalid saved dimension height")?;
    ensure!(
        native_core::postprocessing::PostProcessing::new(input.bounds.min_y, height).is_some(),
        "unsupported saved postprocessing bounds"
    );
    for (position, chunk) in &input.area.chunks {
        ensure!(
            chunk.position == *position
                && chunk.data_version == TARGET_DATA_VERSION
                && chunk.is_full(),
            "saved chunk identity, version or status differs at {position:?}"
        );
        validate_document(&chunk.document, *position)?;
        ensure!(
            native_persistence::palette::decode_sections(&chunk.document.value)? == chunk.sections,
            "saved chunk sections differ from the original document at {position:?}"
        );
        for section in &chunk.sections {
            if let Some(blocks) = &section.blocks {
                for state in &blocks.states {
                    let mut seen = BTreeSet::new();
                    for (name, _) in &state.properties {
                        ensure!(seen.insert(&name.0), "duplicate saved block property");
                    }
                }
            }
        }
    }
    let world = Arc::new(
        input
            .area
            .materialize_with_defaults(input.dimension, input.defaults)?,
    );
    let mut checked_states = BTreeSet::new();
    for chunk in world.chunks.values() {
        for section in chunk.sections.values() {
            if let Some(blocks) = &section.blocks {
                for &id in blocks.iter() {
                    if checked_states.insert(id) {
                        validate_state(world.state(id).context("saved block state is missing")?)?;
                    }
                }
            }
        }
    }

    let mut tasks = BTreeMap::new();
    let mut containers = BTreeMap::new();
    let mut heightmaps = BTreeMap::new();
    let mut block_entities = BTreeMap::new();
    for (position, source) in &input.area.chunks {
        let pos = ChunkPos {
            x: position.x,
            z: position.z,
        };
        let activation = world
            .saved_chunk_activation(pos)
            .context("saved preparation was not captured")?;
        let entities = world
            .saved_block_entities
            .get(&pos)
            .context("saved block entities were not captured")?;
        block_entities.insert(
            pos,
            ConvertedBlockEntities::from_saved(entities, &world, pos, input.bounds)?,
        );
        let blocks = world
            .saved_block_ticks
            .get(&pos)
            .map_or(&[][..], Vec::as_slice);
        let fluids = world
            .saved_fluid_ticks
            .get(&pos)
            .map_or(&[][..], Vec::as_slice);
        let marks = read_saved_work(
            activation,
            // The live-session guard must continue rejecting pending entities.
            // Their complete saved payload was validated for this isolated stage above.
            &SavedBlockEntities::Empty,
            input.bounds,
            SavedTickInputs {
                position: pos,
                blocks,
                fluids,
                supports_block: |_| true,
            },
            true,
        )?;
        for tick in blocks {
            let id = ResourceId::parse(&tick.block)?;
            ensure!(
                input
                    .block_ids
                    .contains(&format!("{}:{}", id.namespace, id.path)),
                "unknown saved block tick {}",
                tick.block
            );
        }
        let block_ticks = blocks
            .iter()
            .map(|tick| SavedTick {
                kind: tick.block.clone(),
                position: tick.position,
                delay: tick.delay,
                priority: tick.priority,
            })
            .collect();
        let fluid_ticks = fluids
            .iter()
            .map(|tick| SavedTick {
                kind: tick.fluid.clone(),
                position: tick.position,
                delay: tick.delay,
                priority: tick.priority,
            })
            .collect();
        tasks.insert(pos, PostprocessingTask::new(Arc::new(marks), pos));
        containers.insert(
            pos,
            ConvertedChunkTicks {
                blocks: ChunkTicks::from_saved(pos, block_ticks)?,
                fluids: ChunkTicks::from_saved(pos, fluid_ticks)?,
            },
        );
        heightmaps.insert(pos, saved_heightmaps(&source.document, input.bounds)?);
    }
    let ticks = RegionTicks::new_ordered(
        containers,
        input.game_time,
        input.next_sub_tick,
        input.registration,
    )?;
    let tags = CapturedPostprocessingTags::load(provider, input.block_ids)?;
    let mut stage = GeneratedPostprocessing::new(
        world,
        input.bounds.min_y,
        input.bounds.max_y,
        ConvertedPostprocessingRegion { tasks, ticks },
        input.light,
        tags.mushroom.clone(),
        heightmaps,
        Some(Arc::clone(&tags.heightmaps)),
    );
    stage.set_block_entities(block_entities)?;
    tags.apply(&mut stage);
    Ok(stage)
}

fn unique_field<'a>(value: &'a Tag, name: &str) -> Result<Option<&'a Tag>> {
    let Tag::Compound(fields) = value else {
        bail!("saved chunk data is not a compound");
    };
    let mut matches = fields.iter().filter(|(key, _)| key.matches(name));
    let found = matches.next().map(|(_, value)| value);
    ensure!(matches.next().is_none(), "duplicate saved field {name}");
    Ok(found)
}

fn validate_document(document: &NamedTag, pos: native_persistence::world::ChunkPos) -> Result<()> {
    let root = &document.value;
    ensure!(
        unique_field(root, "DataVersion")? == Some(&Tag::Int(TARGET_DATA_VERSION))
            && unique_field(root, "xPos")? == Some(&Tag::Int(pos.x))
            && unique_field(root, "zPos")? == Some(&Tag::Int(pos.z))
            && matches!(unique_field(root, "Status")?, Some(Tag::String(status)) if status.matches("minecraft:full")),
        "saved chunk metadata differs from its source"
    );
    let _ = unique_field(root, "sections")?;
    Ok(())
}

fn saved_heightmaps(document: &NamedTag, bounds: BuildBounds) -> Result<FullHeightmaps> {
    let source = unique_field(&document.value, "Heightmaps")?
        .context("saved FULL heightmaps are missing")?;
    let names = [
        "WORLD_SURFACE",
        "OCEAN_FLOOR",
        "MOTION_BLOCKING",
        "MOTION_BLOCKING_NO_LEAVES",
    ];
    let mut fields = Vec::with_capacity(names.len());
    for name in names {
        fields.push((
            name.into(),
            unique_field(source, name)?
                .with_context(|| format!("saved FULL heightmap {name} is missing"))?
                .clone(),
        ));
    }
    FullHeightmaps::from_converted(
        HeightBounds {
            min_y: bounds.min_y,
            max_y: bounds.max_y,
        },
        &Tag::Compound(fields),
    )
}

#[cfg(test)]
#[path = "saved_postprocessing_tests.rs"]
mod tests;
