//! Incremental execution of fresh terrain generation without structures.
use anyhow::{Context, Result, bail, ensure};
use native_resources::{
    carver::{schedule::CarverCache, tags::CarverBlockTags},
    density::DensityEvaluator,
    dimension_lighting::GeneratedDimensionLighting,
    feature_order::FeatureSchedule,
    packs::{ResourceId, ResourceProvider},
};
use native_world::loaded::{BlockState, ChunkPos, World};
use serde_json::Value;
use std::{collections::BTreeSet, sync::Arc};

use super::{
    BiomeNeighborhood, CarvedTerrainWorld, CarverBlockState, PreparedDecoration, TerrainGenerator,
    schedule::{GenerationSchedule, GenerationStage},
};

/// A single-threaded, incrementally driven batch for fresh generation.
/// Its generator, provider, block registry and tags must belong to the same
/// immutable pack stack. No old-chunk blending or structures are supported.
/// Its deterministic work order has not been proven to match Java's thread
/// scheduling for climate ties. Outputs remain intermediate and unsaveable.
pub struct FreshTerrainBatch {
    schedule: GenerationSchedule,
    generator: TerrainGenerator,
    evaluator: DensityEvaluator,
    carvers: CarverCache,
    biomes: BiomeNeighborhood,
    tags: CarverBlockTags,
    dimension: String,
    terrain: Option<CarvedTerrainWorld>,
    decoration: Option<DecorationConfig>,
    failed: bool,
}

struct DecorationConfig {
    schedule: FeatureSchedule,
    block_registry: BTreeSet<String>,
    has_skylight: bool,
}

impl FreshTerrainBatch {
    /// The supplied generator must have been constructed without structure
    /// adjustment and for a new world with structures explicitly disabled.
    pub fn new_without_structures(
        generator: TerrainGenerator,
        tags: CarverBlockTags,
        dimension: &str,
    ) -> Result<Self> {
        ensure!(
            !generator.structure_context,
            "structure-adjusted terrain is not supported by this batch"
        );
        let id = ResourceId::parse(dimension)?;
        let biomes = generator.biome_neighborhood()?;
        Ok(Self {
            schedule: GenerationSchedule::new(),
            generator,
            evaluator: DensityEvaluator::default(),
            carvers: CarverCache::default(),
            biomes,
            tags,
            dimension: format!("{}:{}", id.namespace, id.path),
            terrain: None,
            decoration: None,
            failed: false,
        })
    }

    /// Configure FEATURES before requesting work. The lighting settings and
    /// registries must come from the same immutable pack stack as the generator.
    /// This batch uses a freshly empty light engine and never initializes or
    /// propagates light; no external light work may be interleaved with it.
    pub fn enable_decoration(
        &mut self,
        provider: &mut impl ResourceProvider,
        placed_features: &BTreeSet<ResourceId>,
        block_registry: BTreeSet<String>,
        lighting: &GeneratedDimensionLighting,
    ) -> Result<()> {
        ensure!(
            !self.failed && self.schedule.requested_chunks() == 0 && self.decoration.is_none(),
            "decoration must be configured before requests"
        );
        ensure!(
            format!(
                "{}:{}",
                lighting.dimension.namespace, lighting.dimension.path
            ) == self.dimension
                && lighting.min_y == self.generator.min_y
                && lighting.height == self.generator.height,
            "lighting settings differ from generation dimension"
        );
        ensure!(!block_registry.is_empty(), "block registry is empty");
        let schedule = self.generator.feature_schedule(provider, placed_features)?;
        self.decoration = Some(DecorationConfig {
            schedule,
            block_registry,
            has_skylight: lighting.has_skylight,
        });
        Ok(())
    }

    /// Add a terrain request; overlapping dependencies retain completed work.
    pub fn request(&mut self, pos: ChunkPos) -> Result<()> {
        ensure!(!self.failed, "terrain batch failed and must be discarded");
        self.schedule.request(pos, GenerationStage::Terrain)
    }

    /// Request terrain and supported FEATURES for one chunk. No light stage is
    /// requested, so all decoration observes the initial empty light engine.
    pub fn request_features(&mut self, pos: ChunkPos) -> Result<()> {
        ensure!(
            !self.failed,
            "generation batch failed and must be discarded"
        );
        ensure!(self.decoration.is_some(), "decoration is not configured");
        self.schedule.request(pos, GenerationStage::Features)
    }

    pub fn is_complete(&self) -> bool {
        !self.failed && self.schedule.is_complete()
    }
    pub fn schedule(&self) -> &GenerationSchedule {
        &self.schedule
    }
    pub fn snapshot(&self) -> Option<Arc<World>> {
        self.terrain.as_ref().map(CarvedTerrainWorld::snapshot)
    }

    /// Perform one ready operation. No-op structure stages represent the
    /// explicitly disabled structure setting. A failed operation poisons the
    /// batch because generator or cache search history may have advanced.
    pub fn step(
        &mut self,
        provider: &mut impl ResourceProvider,
        classify: &mut impl FnMut(&Value) -> Result<CarverBlockState>,
        resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
    ) -> Result<Option<(ChunkPos, GenerationStage)>> {
        ensure!(!self.failed, "terrain batch failed and must be discarded");
        let Some((pos, stage)) = self.schedule.next_ready() else {
            if self.schedule.is_complete() {
                return Ok(None);
            }
            self.failed = true;
            bail!("terrain generation plan has unfinished steps but no ready work");
        };
        let result = match stage {
            GenerationStage::StructureStarts | GenerationStage::StructureReferences => {
                self.schedule.complete(pos, stage)
            }
            GenerationStage::Biomes => self.schedule.run_biomes(
                &mut self.generator,
                &mut self.evaluator,
                &mut self.biomes,
                pos,
            ),
            GenerationStage::Terrain => self.schedule.run_terrain(
                &mut self.generator,
                provider,
                &mut self.evaluator,
                &mut self.carvers,
                &self.biomes,
                &self.tags,
                &mut self.terrain,
                pos,
                &self.dimension,
                false,
                classify,
                resolve,
            ),
            GenerationStage::Features => (|| {
                let config = self
                    .decoration
                    .as_ref()
                    .context("decoration is not configured")?;
                let plan = self.generator.decoration_plan(
                    &config.schedule,
                    &self.biomes,
                    [pos.x, pos.z],
                )?;
                let prepared =
                    PreparedDecoration::load(provider, &plan, &config.block_registry, resolve)?;
                let mut covered = Vec::with_capacity(9);
                for dz in -1i32..=1 {
                    for dx in -1i32..=1 {
                        covered.push(ChunkPos {
                            x: pos.x.checked_add(dx).context("light X overflow")?,
                            z: pos.z.checked_add(dz).context("light Z overflow")?,
                        });
                    }
                }
                let light = native_world::generation_light::VisibleLightSnapshot::fresh_after_section_updates(true, config.has_skylight, [], covered)
                    .context("invalid initial light columns")?;
                let terrain = self
                    .terrain
                    .as_mut()
                    .context("feature terrain is missing")?;
                ensure!(
                    !terrain.has_published_light
                        && terrain
                            .world
                            .chunks
                            .values()
                            .all(|chunk| !chunk.block_light_correct),
                    "feature batch light engine is not fresh"
                );
                self.schedule
                    .run_features_with_visible_light(terrain, &prepared, false, &light)?;
                Ok(())
            })(),
            _ => Err(anyhow::anyhow!("unsupported terrain batch stage {stage:?}")),
        };
        if let Err(error) = result {
            self.failed = true;
            return Err(error).with_context(|| {
                format!(
                    "terrain generation failed at {stage:?} ({},{})",
                    pos.x, pos.z
                )
            });
        }
        Ok(Some((pos, stage)))
    }

    /// Transfer completed intermediate data to subsequent generation stages.
    pub fn into_parts(
        self,
    ) -> Result<(
        GenerationSchedule,
        BiomeNeighborhood,
        Option<CarvedTerrainWorld>,
    )> {
        ensure!(self.is_complete(), "terrain batch is incomplete or failed");
        Ok((self.schedule, self.biomes, self.terrain))
    }
}
