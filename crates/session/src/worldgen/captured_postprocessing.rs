//! Construct an isolated postprocessing pass from captured generation data.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use native_persistence::heightmap::{FullHeightmaps, HeightBounds, generated_full_heightmaps};
use native_resources::{
    block_registry::GeneratedBlockRegistry,
    packs::{ResourceId, ResourceProvider},
};
use native_world::{generation_light::VisibleLightSnapshot, loaded::ChunkPos};

use super::{
    CarvedTerrainWorld, GeneratedPostprocessing, chunk_ticks::SavedTick,
    postprocessing::ConvertedPostprocessingRegion, postprocessing_world::ConvertedBlockEntities,
};

impl CarvedTerrainWorld {
    /// Start an isolated postprocessing pass from the exact generated terrain,
    /// pending ticks, marks and converted heightmaps. The caller must bind the
    /// provider and registry to the same captured packs and separately verify
    /// live state, entity sidecars, light, rules and disk inputs before saving.
    pub fn begin_captured_postprocessing(
        &self,
        provider: &mut impl ResourceProvider,
        block_ids: &BTreeSet<String>,
        registry: &GeneratedBlockRegistry,
        region: ConvertedPostprocessingRegion,
        heightmaps: BTreeMap<ChunkPos, FullHeightmaps>,
        light: Option<VisibleLightSnapshot>,
    ) -> Result<GeneratedPostprocessing> {
        ensure!(!region.tasks.is_empty(), "no postprocessing chunks");
        ensure!(
            region.tasks.keys().eq(heightmaps.keys()),
            "heightmap chunk set differs"
        );
        ensure!(
            region.ticks.registration_order().len() == region.tasks.len()
                && region
                    .ticks
                    .registration_order()
                    .iter()
                    .all(|pos| region.tasks.contains_key(pos)),
            "postprocessing tick chunk set differs"
        );
        ensure!(
            light
                .as_ref()
                .is_none_or(VisibleLightSnapshot::has_block_channel),
            "block light is unavailable"
        );
        let max_y = self
            .min_y
            .checked_add(self.height)
            .context("dimension height overflow")?;
        let bounds = HeightBounds {
            min_y: self.min_y,
            max_y,
        };
        let mut block_entities = BTreeMap::new();
        for (&pos, task) in &region.tasks {
            ensure!(
                self.world.chunks.contains_key(&pos),
                "postprocessing terrain chunk is missing"
            );
            let marks = self
                .postprocessing_marks_to_date(pos)
                .context("postprocessing marks are missing")?;
            ensure!(
                task.unstarted_marks()? == &marks,
                "postprocessing marks differ from terrain"
            );
            let ticks = region
                .ticks
                .chunk(pos)
                .context("postprocessing tick chunk is missing")?;
            ensure!(
                ticks.blocks.peek().is_none() && ticks.fluids.peek().is_none(),
                "postprocessing ticks are already active"
            );
            let blocks: Vec<_> = self
                .world
                .saved_block_ticks
                .get(&pos)
                .into_iter()
                .flatten()
                .map(|tick| SavedTick {
                    kind: tick.block.clone(),
                    position: tick.position,
                    delay: tick.delay,
                    priority: tick.priority,
                })
                .collect();
            let fluids: Vec<_> = self
                .world
                .saved_fluid_ticks
                .get(&pos)
                .into_iter()
                .flatten()
                .map(|tick| SavedTick {
                    kind: tick.fluid.clone(),
                    position: tick.position,
                    delay: tick.delay,
                    priority: tick.priority,
                })
                .collect();
            ensure!(
                ticks.blocks.pack(0) == blocks && ticks.fluids.pack(0) == fluids,
                "postprocessing ticks differ from terrain"
            );
            let expected = generated_full_heightmaps(bounds, &self.generated_heightmap_data(pos)?)?;
            ensure!(
                heightmaps[&pos].to_tag()? == expected,
                "postprocessing heightmaps differ from terrain"
            );
            block_entities.insert(
                pos,
                ConvertedBlockEntities::from_proto(
                    self.generated_block_entity_data(pos, registry)?,
                    self.generated_entity_order.get(&pos).map(Arc::as_ref),
                    self.generated_pending_order.get(&pos).map(Arc::as_ref),
                )?,
            );
        }
        let heightmap_tags = Arc::new(native_resources::heightmap_tags::HeightmapTags::load(
            provider, block_ids,
        )?);
        let load_tags = |provider: &mut _, names: &[&str]| -> Result<_> {
            let names = names
                .iter()
                .map(|name| ResourceId::parse(name))
                .collect::<Result<BTreeSet<_>>>()?;
            Ok(
                native_resources::ore_feature::OreBlockTags::load(provider, block_ids, &names)
                    .ok()
                    .map(Arc::new),
            )
        };
        let mushroom = load_tags(
            provider,
            &["minecraft:overrides_mushroom_light_requirement"],
        )?;
        let leaf = load_tags(provider, &["minecraft:prevents_nearby_leaf_decay"])?;
        let snow = load_tags(provider, &["minecraft:snow"])?;
        let fence = load_tags(
            provider,
            &[
                "minecraft:fences",
                "minecraft:wooden_fences",
                "minecraft:shulker_boxes",
            ],
        )?;
        let bubble = native_resources::fluid_tags::BubbleColumnTags::load(provider, block_ids)
            .ok()
            .map(Arc::new);
        let washed = native_resources::fluid_tags::load_washed_away_blocks(provider, block_ids)
            .ok()
            .map(Arc::new);
        let lava = native_resources::fluid_tags::load_lava(provider)
            .ok()
            .map(Arc::new);
        let mut stage = GeneratedPostprocessing::new(
            self.snapshot(),
            self.min_y,
            max_y,
            region,
            light,
            mushroom,
            heightmaps,
            Some(heightmap_tags),
        );
        stage.set_block_entities(block_entities)?;
        // Captured proto-entities and entity sidecars have not been reconciled.
        // Leave entity candidates unknown, so callbacks that need them fail.
        stage.set_leaf_tags(leaf);
        stage.set_snow_tags(snow);
        stage.set_fence_tags(fence);
        stage.set_bubble_tags(bubble);
        stage.set_fluid_tags(washed, lava);
        Ok(stage)
    }
}
