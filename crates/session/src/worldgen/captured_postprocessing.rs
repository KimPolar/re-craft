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

/// Non-writable components of a captured postprocessing result. Sections,
/// entity sidecars, source metadata and live state require separate validation.
pub struct CapturedPostprocessedChunkData {
    pub heightmaps: native_persistence::nbt::Tag,
    pub block_ticks: native_persistence::nbt::Tag,
    pub fluid_ticks: native_persistence::nbt::Tag,
    pub block_entities: native_persistence::nbt::Tag,
}

struct CapturedPostprocessingTags {
    heightmaps: Arc<native_resources::heightmap_tags::HeightmapTags>,
    mushroom: Option<Arc<native_resources::ore_feature::OreBlockTags>>,
    leaf: Option<Arc<native_resources::ore_feature::OreBlockTags>>,
    snow: Option<Arc<native_resources::ore_feature::OreBlockTags>>,
    fence: Option<Arc<native_resources::ore_feature::OreBlockTags>>,
    bubble: Option<Arc<native_resources::fluid_tags::BubbleColumnTags>>,
    washed: Option<Arc<BTreeSet<String>>>,
    lava: Option<Arc<BTreeSet<String>>>,
}

impl CapturedPostprocessingTags {
    fn load(provider: &mut impl ResourceProvider, block_ids: &BTreeSet<String>) -> Result<Self> {
        let heightmaps = Arc::new(native_resources::heightmap_tags::HeightmapTags::load(
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
        Ok(Self {
            heightmaps,
            mushroom: load_tags(
                provider,
                &["minecraft:overrides_mushroom_light_requirement"],
            )?,
            leaf: load_tags(provider, &["minecraft:prevents_nearby_leaf_decay"])?,
            snow: load_tags(provider, &["minecraft:snow"])?,
            fence: load_tags(
                provider,
                &[
                    "minecraft:fences",
                    "minecraft:wooden_fences",
                    "minecraft:shulker_boxes",
                ],
            )?,
            bubble: native_resources::fluid_tags::BubbleColumnTags::load(provider, block_ids)
                .ok()
                .map(Arc::new),
            washed: native_resources::fluid_tags::load_washed_away_blocks(provider, block_ids)
                .ok()
                .map(Arc::new),
            lava: native_resources::fluid_tags::load_lava(provider)
                .ok()
                .map(Arc::new),
        })
    }

    fn apply(self, stage: &mut GeneratedPostprocessing) {
        stage.set_leaf_tags(self.leaf);
        stage.set_snow_tags(self.snow);
        stage.set_fence_tags(self.fence);
        stage.set_bubble_tags(self.bubble);
        stage.set_fluid_tags(self.washed, self.lava);
    }
}

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
        let tags = CapturedPostprocessingTags::load(provider, block_ids)?;
        let mut stage = GeneratedPostprocessing::new(
            self.snapshot(),
            self.min_y,
            max_y,
            region,
            light,
            tags.mushroom.clone(),
            heightmaps,
            Some(Arc::clone(&tags.heightmaps)),
        );
        stage.set_block_entities(block_entities)?;
        // Captured proto-entities and entity sidecars have not been reconciled.
        // Leave entity candidates unknown, so callbacks that need them fail.
        tags.apply(&mut stage);
        Ok(stage)
    }

    /// Resume completed captured callbacks with the same terrain, frozen tags
    /// and light revision. The caller must keep live state unchanged.
    pub fn resume_captured_postprocessing(
        &self,
        registry: &GeneratedBlockRegistry,
        engine: &StagedGenerationLightEngine,
        completed: &super::PostprocessedGeneratedRegion,
    ) -> Result<GeneratedPostprocessing> {
        self.validate_captured_postprocessing(completed)?;
        let covered = completed
            .chunks
            .iter()
            .copied()
            .chain(completed.deferred_chunks())
            .collect();
        let light = self.captured_postprocessing_light_input(
            engine,
            registry,
            completed.light_checkpoint.as_ref(),
            covered,
        )?;
        let heightmap_tags = completed
            .heightmap_tags
            .as_ref()
            .context("postprocessing heightmap tags are missing")?;
        GeneratedPostprocessing::from_completed(
            completed,
            Some(light),
            completed.mushroom_tags.clone(),
            Arc::clone(heightmap_tags),
        )
    }

    fn validate_captured_postprocessing(
        &self,
        completed: &super::PostprocessedGeneratedRegion,
    ) -> Result<()> {
        ensure!(
            Arc::ptr_eq(&self.world, &completed.source),
            "postprocessing terrain revision differs"
        );
        ensure!(
            Arc::ptr_eq(&completed.world, &completed.sealed_world)
                && completed.edits == completed.sealed_edits
                && completed.read_chunks == completed.sealed_read_chunks
                && completed.light_reads == completed.sealed_light_reads
                && completed.light_checks == completed.sealed_checks
                && completed.chunks == completed.sealed_chunks,
            "postprocessing result was modified"
        );
        ensure!(!completed.chunks.is_empty(), "no postprocessed chunks");
        let max_y = self
            .min_y
            .checked_add(self.height)
            .context("dimension height overflow")?;
        ensure!(
            (self.min_y, max_y) == (completed.min_y, completed.max_y),
            "postprocessing bounds differ"
        );
        Ok(())
    }

    /// Encode heightmaps, pending LevelChunk ticks and promoted block entities
    /// from one sealed pass. This alone does not authorize a chunk save.
    pub fn captured_postprocessed_chunk_data(
        &self,
        completed: &super::PostprocessedGeneratedRegion,
        registry: &GeneratedBlockRegistry,
        block_ids: &BTreeSet<String>,
        positions: impl IntoIterator<Item = ChunkPos>,
    ) -> Result<BTreeMap<ChunkPos, CapturedPostprocessedChunkData>> {
        use native_world::loaded::{SavedBlockTick, SavedFluidTick};
        self.validate_captured_postprocessing(completed)?;
        let mut selected = BTreeSet::new();
        for pos in positions {
            ensure!(selected.insert(pos), "duplicate postprocessed chunk");
            ensure!(
                completed.chunks.contains(&pos),
                "chunk was not postprocessed"
            );
        }
        ensure!(!selected.is_empty(), "no postprocessed chunks selected");
        for edit in &completed.sealed_edits {
            if !selected.contains(&edit.chunk()) {
                continue;
            }
            let old = completed
                .source
                .block(edit.x, edit.y, edit.z)
                .context("original edited block is missing")?;
            let new = completed
                .world
                .block(edit.x, edit.y, edit.z)
                .context("postprocessed edited block is missing")?;
            if old != new {
                let has_entity = |state: &native_world::loaded::BlockState| {
                    registry
                        .is_canonical_state(&state.name, &state.properties)
                        .then(|| registry.has_block_entity(&state.name))
                        .flatten()
                        .context("edited block entity classification is unavailable")
                };
                ensure!(
                    (!has_entity(old)? && !has_entity(new)?)
                        || completed.permits_block_entity_edit(*edit, old, new),
                    "postprocessing changed an unvalidated block entity"
                );
            }
        }
        let time = completed.ticks().game_time();
        selected
            .into_iter()
            .map(|pos| {
                let ticks = completed
                    .ticks()
                    .chunk(pos)
                    .context("postprocessed tick container is missing")?;
                let blocks: Vec<_> = ticks
                    .blocks
                    .pack(time)
                    .into_iter()
                    .map(|tick| SavedBlockTick {
                        block: tick.kind,
                        position: tick.position,
                        delay: tick.delay,
                        priority: tick.priority,
                    })
                    .collect();
                let fluids: Vec<_> = ticks
                    .fluids
                    .pack(time)
                    .into_iter()
                    .map(|tick| SavedFluidTick {
                        fluid: tick.kind,
                        position: tick.position,
                        delay: tick.delay,
                        priority: tick.priority,
                    })
                    .collect();
                Ok((
                    pos,
                    CapturedPostprocessedChunkData {
                        heightmaps: completed
                            .heightmaps()
                            .get(&pos)
                            .context("postprocessed heightmaps are missing")?
                            .to_tag()?,
                        block_ticks: native_persistence::generated_ticks::encode_level_block_ticks(
                            pos,
                            &blocks,
                            |id| block_ids.contains(id),
                        )?,
                        fluid_ticks: native_persistence::generated_ticks::encode_level_fluid_ticks(
                            pos, &fluids,
                        )?,
                        block_entities: completed.block_entity_data(pos)?,
                    },
                ))
            })
            .collect()
    }

    /// Reconstruct missing POI sidecars from a sealed captured result. The
    /// caller must independently establish that storage and live tickets are
    /// absent and revalidate them before publishing these intermediate drafts.
    pub fn captured_postprocessed_missing_poi_snapshot(
        &self,
        completed: &super::PostprocessedGeneratedRegion,
        registry: &GeneratedBlockRegistry,
        positions: impl IntoIterator<Item = ChunkPos>,
    ) -> Result<BTreeMap<ChunkPos, Option<native_persistence::nbt::NamedTag>>> {
        self.validate_captured_postprocessing(completed)?;
        let mut output = BTreeMap::new();
        for pos in positions {
            ensure!(
                completed.chunks.contains(&pos),
                "chunk was not postprocessed"
            );
            ensure!(!output.contains_key(&pos), "duplicate postprocessed chunk");
            completed.ensure_poi_unchanged(pos, registry)?;
            let draft = native_persistence::poi::refresh_missing_chunk(
                native_persistence::world::ChunkPos { x: pos.x, z: pos.z },
                &completed.world,
                self.min_y,
                self.height,
                |state| {
                    ensure!(
                        registry.is_canonical_state(&state.name, &state.properties),
                        "unknown POI block state"
                    );
                    Ok(())
                },
            )?;
            output.insert(pos, draft);
        }
        ensure!(!output.is_empty(), "no postprocessed chunks selected");
        Ok(output)
    }

    /// Preview the light after pending postprocessing checks at a caller-chosen
    /// drain boundary. The result and the staged engine remain unchanged.
    pub fn captured_postprocessed_light_snapshot(
        &self,
        completed: &super::PostprocessedGeneratedRegion,
        engine: &StagedGenerationLightEngine,
        registry: &GeneratedBlockRegistry,
        positions: impl IntoIterator<Item = ChunkPos>,
    ) -> Result<VisibleLightSnapshot> {
        self.validate_captured_postprocessing(completed)?;
        let mut covered = BTreeSet::new();
        for pos in positions {
            ensure!(covered.insert(pos), "duplicate postprocessed light chunk");
            ensure!(
                completed.chunks.contains(&pos),
                "light chunk was not postprocessed"
            );
        }
        ensure!(!covered.is_empty(), "no postprocessed light chunks");
        let bounds = native_world::block_light::LightBounds::new(completed.min_y, completed.max_y)
            .context("unsupported postprocessing light bounds")?;
        let checkpoint = completed.light_checkpoint.as_ref();
        let drained = checkpoint.map_or(0, |saved| saved.drained_checks);
        let pending = completed
            .sealed_checks
            .get(drained..)
            .context("invalid postprocessing light checkpoint")?;
        super::terrain_batch::extend_postprocessing_light_coverage(&mut covered, pending)?;
        let before =
            self.captured_postprocessing_light_input(engine, registry, checkpoint, covered)?;
        let previous_world =
            checkpoint.map_or(completed.source.as_ref(), |saved| saved.world.as_ref());
        let previous_sources =
            checkpoint.map_or(&engine.fresh.sky_sources, |saved| &saved.sky_sources);
        let updated_sources = if before.has_sky_channel() {
            super::terrain_batch::updated_postprocessing_sky_sources(
                previous_world,
                &completed.world,
                pending,
                bounds,
                previous_sources,
            )?
        } else {
            None
        };
        let checks: Vec<_> = pending.iter().map(|pos| [pos.x, pos.y, pos.z]).collect();
        super::postprocessing_world::drain_postprocessing_light(
            &completed.world,
            bounds,
            &before,
            &engine.active,
            updated_sources.as_ref().unwrap_or(previous_sources),
            &checks,
        )
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
    fn captured_completed_pass_resumes_deferred_without_replaying_ticks() -> Result<()> {
        use super::super::PostprocessingAttempt;
        use super::super::chunk_ticks::{ChunkTicks, RegionTicks};
        use native_core::postprocessing::PostProcessing;
        use native_persistence::nbt::{Tag, TagType};

        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let registry = resources.vanilla_generated_block_registry()?;
        let ids = resources.vanilla_block_ids()?;
        let first = ChunkPos { x: 0, z: 0 };
        let second = ChunkPos { x: 1, z: 0 };
        let mut world = World::new("minecraft:overworld");
        let stone = world.intern_state(BlockState {
            name: "minecraft:stone".into(),
            properties: BTreeMap::new(),
        });
        let sand = world.intern_state(BlockState {
            name: "minecraft:sand".into(),
            properties: BTreeMap::new(),
        });
        let barrel = world.intern_state(BlockState {
            name: "minecraft:barrel".into(),
            properties: registry
                .default_properties("minecraft:barrel")
                .context("barrel state is unavailable")?,
        });
        for pos in [first, second] {
            let mut blocks = Box::new([stone; BLOCK_COUNT]);
            if pos == first {
                blocks[8 + 8 * 16 + 8 * 256] = sand;
                blocks[1 + 1 * 16 + 2 * 256] = barrel;
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
        }
        let terrain = CarvedTerrainWorld::new(MaterializedTerrain {
            world,
            position: first,
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
        let mut marks = PostProcessing::new(0, 16).context("invalid postprocessing bounds")?;
        marks.mark(8, 8, 8);
        let empty = Arc::new(PostProcessing::new(0, 16).context("invalid postprocessing bounds")?);
        let mut ticks = BTreeMap::new();
        for pos in [first, second] {
            ticks.insert(
                pos,
                ConvertedChunkTicks {
                    blocks: ChunkTicks::from_saved(pos, vec![])?,
                    fluids: ChunkTicks::from_saved(pos, vec![])?,
                },
            );
        }
        let region = ConvertedPostprocessingRegion {
            tasks: BTreeMap::from([
                (first, PostprocessingTask::new(Arc::new(marks), first)),
                (second, PostprocessingTask::new(empty, second)),
            ]),
            ticks: RegionTicks::new(ticks, 100, 0)?,
        };
        let bounds = HeightBounds {
            min_y: 0,
            max_y: 16,
        };
        let surface = [16; 256];
        let original_maps = native_persistence::heightmap::encode_generated_heightmaps(
            bounds,
            &native_persistence::heightmap::GeneratedHeightmaps {
                world_surface: Some(&surface),
                ..Default::default()
            },
        )?;
        let maps = FullHeightmaps::from_converted(
            bounds,
            &generated_full_heightmaps(bounds, &original_maps)?,
        )?;
        let tags = CapturedPostprocessingTags::load(&mut resources, &ids)?;
        let mut initial = GeneratedPostprocessing::new(
            terrain.snapshot(),
            0,
            16,
            region,
            None,
            tags.mushroom.clone(),
            BTreeMap::from([(first, maps.clone()), (second, maps.clone())]),
            Some(Arc::clone(&tags.heightmaps)),
        );
        tags.apply(&mut initial);
        let entities = [first, second]
            .into_iter()
            .map(|pos| {
                Ok((
                    pos,
                    ConvertedBlockEntities::from_proto(
                        Tag::List {
                            element_type: TagType::Compound,
                            values: vec![],
                        },
                        None,
                        None,
                    )?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        initial.set_block_entities(entities)?;
        initial.set_water_source_conversion(false)?;
        initial.select_initial_targets(BTreeSet::from([first]))?;
        assert_eq!(
            initial.try_poll(first, 64)?,
            PostprocessingAttempt::Progress { complete: true }
        );
        let mut expected_light = None;
        for drain in [false, true] {
            let mut original = initial.clone();
            original
                .light_checks
                .push(BlockPosition { x: 8, y: 8, z: 8 });
            if drain {
                assert!(terrain.drain_captured_postprocessing_light(
                    &mut original,
                    &engine,
                    &registry
                )?);
            }
            let mut completed = original.finish()?;
            let data =
                terrain.captured_postprocessed_chunk_data(&completed, &registry, &ids, [first])?;
            let chunk = &data[&first];
            assert_eq!(chunk.heightmaps, maps.to_tag()?);
            let Tag::List {
                values: block_ticks,
                ..
            } = &chunk.block_ticks
            else {
                unreachable!()
            };
            assert_eq!(block_ticks.len(), 1);
            assert_eq!(
                block_ticks[0].get("i"),
                Some(&Tag::String("minecraft:sand".into()))
            );
            assert_eq!(block_ticks[0].get("t"), Some(&Tag::Int(2)));
            assert!(matches!(&chunk.fluid_ticks, Tag::List { values, .. } if values.is_empty()));
            assert!(matches!(&chunk.block_entities, Tag::List { values, .. } if values.is_empty()));
            let poi = terrain.captured_postprocessed_missing_poi_snapshot(
                &completed,
                &registry,
                [first],
            )?;
            let document = poi[&first].clone().context("barrel POI missing")?;
            let decoded = native_persistence::world::SavedPoiChunk::from_document(
                document,
                native_persistence::world::ChunkPos { x: 0, z: 0 },
            )?;
            assert_eq!(decoded.sections[&0].records.len(), 1);
            assert_eq!(decoded.sections[&0].records[0].position, [1, 2, 1]);
            assert!(
                decoded.sections[&0].records[0]
                    .type_id
                    .matches("minecraft:fisherman")
            );
            assert!(
                terrain
                    .captured_postprocessed_missing_poi_snapshot(&completed, &registry, [second])
                    .is_err()
            );
            assert!(
                terrain
                    .captured_postprocessed_missing_poi_snapshot(
                        &completed,
                        &registry,
                        [first, first]
                    )
                    .is_err()
            );
            assert!(
                terrain
                    .captured_postprocessed_missing_poi_snapshot(&completed, &registry, [])
                    .is_err()
            );
            assert!(
                terrain
                    .captured_postprocessed_chunk_data(&completed, &registry, &ids, [second])
                    .is_err()
            );
            assert!(
                terrain
                    .captured_postprocessed_chunk_data(&completed, &registry, &ids, [first, first])
                    .is_err()
            );
            assert!(
                terrain
                    .captured_postprocessed_chunk_data(
                        &completed,
                        &registry,
                        &BTreeSet::new(),
                        [first]
                    )
                    .is_err()
            );
            let preview = terrain.captured_postprocessed_light_snapshot(
                &completed,
                &engine,
                &registry,
                [first],
            )?;
            if let Some(previous) = &expected_light {
                assert_eq!(preview.same_complete_column(previous, first), Some(true));
            } else {
                expected_light = Some(preview);
            }
            assert!(
                terrain
                    .captured_postprocessed_light_snapshot(&completed, &engine, &registry, [second])
                    .is_err()
            );
            assert!(
                terrain
                    .captured_postprocessed_light_snapshot(
                        &completed,
                        &engine,
                        &registry,
                        [first, first]
                    )
                    .is_err()
            );
            let prior_ticks = completed.ticks().chunk(first).unwrap().blocks.pack(100);
            assert_eq!(prior_ticks.len(), 1);
            assert_eq!(prior_ticks[0].kind, "minecraft:sand");
            let mut resumed =
                terrain.resume_captured_postprocessing(&registry, &engine, &completed)?;
            assert_eq!(resumed.light_checkpoint.is_some(), drain);
            resumed.request_converted_chunk(second)?;
            assert_eq!(
                resumed.try_poll(second, 64)?,
                PostprocessingAttempt::Progress { complete: true }
            );
            let result = resumed.finish()?;
            assert_eq!(result.chunks, BTreeSet::from([first, second]));
            let resumed_poi = terrain.captured_postprocessed_missing_poi_snapshot(
                &result,
                &registry,
                [first, second],
            )?;
            assert_eq!(resumed_poi[&first], poi[&first]);
            assert!(resumed_poi[&second].is_none());
            assert!(Arc::ptr_eq(
                completed.snow_tags.as_ref().context("snow tags missing")?,
                result
                    .snow_tags
                    .as_ref()
                    .context("resumed snow tags missing")?,
            ));
            assert_eq!(
                result.ticks().chunk(first).unwrap().blocks.pack(100),
                prior_ticks
            );
            assert_eq!(completed.chunks, BTreeSet::from([first]));
            assert_eq!(
                completed.deferred_chunks().collect::<Vec<_>>(),
                vec![second]
            );
            completed.edits.push(BlockPosition { x: 8, y: 8, z: 8 });
            assert!(
                terrain
                    .captured_postprocessed_missing_poi_snapshot(&completed, &registry, [first])
                    .is_err()
            );
            assert!(
                terrain
                    .captured_postprocessed_chunk_data(&completed, &registry, &ids, [first])
                    .is_err()
            );
            assert!(
                terrain
                    .captured_postprocessed_light_snapshot(&completed, &engine, &registry, [first])
                    .is_err()
            );
            assert!(
                terrain
                    .resume_captured_postprocessing(&registry, &engine, &completed)
                    .is_err()
            );
        }
        Ok(())
    }

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
