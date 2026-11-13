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
    CarvedTerrainWorld, GeneratedPostprocessing, StagedGenerationLightEngine,
    chunk_ticks::{ConvertedChunkTicks, SavedTick},
    postprocessing::{
        ConvertedChunkPostprocessing, ConvertedPostprocessingRegion, PostprocessingTask,
    },
    postprocessing_world::{ConvertedBlockEntities, PostprocessingLightCheckpoint},
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
        let mut block_entities = BTreeMap::new();
        for (&pos, task) in &region.tasks {
            let ticks = region
                .ticks
                .chunk(pos)
                .context("postprocessing tick chunk is missing")?;
            block_entities.insert(
                pos,
                self.validate_captured_postprocessing_chunk(
                    pos,
                    task,
                    ticks,
                    &heightmaps[&pos],
                    registry,
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

    /// Add a validated, deferred neighbor after an isolated attempt reported
    /// it missing. The caller must confirm the exact requested position and
    /// supply a light snapshot covering every previously registered column.
    pub fn register_captured_postprocessing_neighbor(
        &self,
        stage: &mut GeneratedPostprocessing,
        engine: &StagedGenerationLightEngine,
        registry: &GeneratedBlockRegistry,
        position: ChunkPos,
        converted: ConvertedChunkPostprocessing,
        heightmaps: FullHeightmaps,
    ) -> Result<()> {
        ensure!(
            Arc::ptr_eq(&self.world, &stage.source),
            "postprocessing terrain revision differs"
        );
        let light = if stage.light.is_some() {
            let mut covered: BTreeSet<_> = stage.region.tasks.keys().copied().collect();
            covered.insert(position);
            Some(self.captured_postprocessing_light_input(
                engine,
                registry,
                stage.light_checkpoint.as_ref(),
                covered,
            )?)
        } else {
            None
        };
        match (&stage.light, &light) {
            (None, None) => {}
            (Some(before), Some(after)) => {
                ensure!(
                    before.has_block_channel() == after.has_block_channel()
                        && before.has_sky_channel() == after.has_sky_channel(),
                    "postprocessing light channels differ"
                );
                for &pos in stage.region.ticks.registration_order() {
                    ensure!(
                        before.same_complete_column(after, pos) == Some(true),
                        "registered postprocessing light changed"
                    );
                }
            }
            _ => anyhow::bail!("postprocessing light coverage differs"),
        }
        let entities = self.validate_captured_postprocessing_chunk(
            position,
            &converted.task,
            &converted.ticks,
            &heightmaps,
            registry,
        )?;
        stage.register_retry_neighbor(position, converted, entities, heightmaps, light)
    }

    /// Drain checks recorded by captured postprocessing at an explicit caller
    /// selected boundary. A failure leaves the stage and light checkpoint intact.
    pub fn drain_captured_postprocessing_light(
        &self,
        stage: &mut GeneratedPostprocessing,
        engine: &StagedGenerationLightEngine,
        registry: &GeneratedBlockRegistry,
    ) -> Result<bool> {
        ensure!(!stage.failed, "generated postprocessing attempt failed");
        ensure!(
            Arc::ptr_eq(&self.world, &stage.source),
            "postprocessing terrain revision differs"
        );
        let checkpoint = stage.light_checkpoint.as_ref();
        let drained = checkpoint.map_or(0, |saved| saved.drained_checks);
        let pending = stage
            .light_checks
            .get(drained..)
            .context("invalid postprocessing light checkpoint")?;
        if pending.is_empty() {
            return Ok(false);
        }
        let mut covered: BTreeSet<_> = stage.region.tasks.keys().copied().collect();
        super::terrain_batch::extend_postprocessing_light_coverage(&mut covered, pending)?;
        let before =
            self.captured_postprocessing_light_input(engine, registry, checkpoint, covered)?;
        stage.drain_recorded_light(before, &engine.active, &engine.fresh.sky_sources)
    }

    fn captured_postprocessing_light_input(
        &self,
        engine: &StagedGenerationLightEngine,
        registry: &GeneratedBlockRegistry,
        checkpoint: Option<&PostprocessingLightCheckpoint>,
        covered: BTreeSet<ChunkPos>,
    ) -> Result<VisibleLightSnapshot> {
        let Some(checkpoint) = checkpoint else {
            return engine.snapshot(self, covered, registry);
        };
        // Validate the engine against this terrain even when every requested
        // column is already present in the checkpoint.
        engine.snapshot(self, std::iter::empty(), registry)?;
        let mut before = checkpoint.visible.clone();
        let missing: BTreeSet<_> = covered
            .into_iter()
            .filter(|pos| before.complete_column(*pos).is_none())
            .collect();
        if !missing.is_empty() {
            let fresh = engine.snapshot(self, missing.iter().copied(), registry)?;
            for pos in missing {
                let column = fresh
                    .complete_column(pos)
                    .context("new postprocessing light column is missing")?;
                ensure!(
                    before.insert_complete_column(pos, column).is_none(),
                    "postprocessing light column changed"
                );
            }
        }
        Ok(before)
    }

    fn validate_captured_postprocessing_chunk(
        &self,
        pos: ChunkPos,
        task: &PostprocessingTask,
        ticks: &ConvertedChunkTicks,
        heightmaps: &FullHeightmaps,
        registry: &GeneratedBlockRegistry,
    ) -> Result<ConvertedBlockEntities> {
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
        let bounds = HeightBounds {
            min_y: self.min_y,
            max_y: self
                .min_y
                .checked_add(self.height)
                .context("dimension height overflow")?,
        };
        let expected = generated_full_heightmaps(bounds, &self.generated_heightmap_data(pos)?)?;
        ensure!(
            heightmaps.to_tag()? == expected,
            "postprocessing heightmaps differ from terrain"
        );
        ConvertedBlockEntities::from_proto(
            self.generated_block_entity_data(pos, registry)?,
            self.generated_entity_order.get(&pos).map(Arc::as_ref),
            self.generated_pending_order.get(&pos).map(Arc::as_ref),
        )
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::super::{FreshGenerationLightTracker, MaterializedTerrain};
    use super::*;
    use native_world::loaded::{BLOCK_COUNT, BlockPosition, BlockState, Chunk, Section, World};

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn captured_light_checks_keep_checkpoint_and_reject_bad_inputs() -> Result<()> {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let registry = resources.vanilla_generated_block_registry()?;
        let pos = ChunkPos { x: 0, z: 0 };
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
        let terrain = CarvedTerrainWorld::new(MaterializedTerrain {
            world,
            position: pos,
            seed: 42,
            min_y: 0,
            height: 16,
            postprocessing: Arc::from(vec![false; BLOCK_COUNT]),
            noise_postprocessing: None,
            surface_postprocessing: None,
            carver_postprocessing: None,
        });
        let engine =
            StagedGenerationLightEngine::new(FreshGenerationLightTracker::new(&terrain, false)?);
        let region = ConvertedPostprocessingRegion {
            tasks: BTreeMap::new(),
            ticks: super::super::chunk_ticks::RegionTicks::new(BTreeMap::new(), 0, 0)?,
        };
        let mut stage = GeneratedPostprocessing::new(
            terrain.snapshot(),
            0,
            16,
            region,
            None,
            None,
            BTreeMap::new(),
            None,
        );
        assert!(!terrain.drain_captured_postprocessing_light(&mut stage, &engine, &registry)?);
        stage.light_checks.push(BlockPosition { x: 8, y: 8, z: 8 });
        assert!(terrain.drain_captured_postprocessing_light(&mut stage, &engine, &registry)?);
        let checkpoint = stage
            .light_checkpoint
            .as_ref()
            .context("checkpoint missing")?;
        assert_eq!(checkpoint.drained_checks, 1);
        let neighbor = ChunkPos { x: 3, z: 0 };
        let extended = terrain.captured_postprocessing_light_input(
            &engine,
            &registry,
            Some(checkpoint),
            BTreeSet::from([pos, neighbor]),
        )?;
        assert_eq!(
            checkpoint.visible.same_complete_column(&extended, pos),
            Some(true)
        );
        assert!(extended.complete_column(neighbor).is_some());
        assert!(!terrain.drain_captured_postprocessing_light(&mut stage, &engine, &registry)?);
        stage.light_checks.push(BlockPosition { x: 8, y: 16, z: 8 });
        assert!(
            terrain
                .drain_captured_postprocessing_light(&mut stage, &engine, &registry)
                .is_err()
        );
        assert_eq!(stage.light_checkpoint.as_ref().unwrap().drained_checks, 1);
        assert!(
            terrain
                .drain_captured_postprocessing_light(&mut stage, &engine, &registry)
                .is_err()
        );
        Ok(())
    }
}
