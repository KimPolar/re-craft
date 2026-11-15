//! Isolated postprocessing inputs restored from saved FULL chunks.
//!
//! The caller must establish Java holder readiness and later reconcile live
//! state, light, entities, POIs, clocks and disk sources before publishing.

use std::{
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
};

use super::{
    GeneratedPostprocessing,
    captured_postprocessing::CapturedPostprocessingTags,
    chunk_ticks::{ChunkTicks, ConvertedChunkTicks, RegionTicks, SavedTick},
    postprocessing::{ConvertedPostprocessingRegion, PostprocessingTask},
    postprocessing_world::ConvertedBlockEntities,
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

/// Restore an isolated postprocessing stage from original FULL chunk data.
/// Saved tick entries remain pending during callbacks. Reproducible default
/// chests, chests with deferred loot, and default potent sulfur entities are
/// admitted. Active upgrade data and other entity contents are rejected. This
/// does not authorize a live commit or save.
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
