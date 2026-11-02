//! Incremental execution of fresh terrain generation without structures.
use anyhow::{Context, Result, bail, ensure};
use native_resources::{
    carver::{schedule::CarverCache, tags::CarverBlockTags},
    density::DensityEvaluator,
    packs::{ResourceId, ResourceProvider},
};
use native_world::loaded::{BlockState, ChunkPos, World};
use serde_json::Value;
use std::sync::Arc;

use super::{
    BiomeNeighborhood, CarvedTerrainWorld, CarverBlockState, TerrainGenerator,
    schedule::{GenerationSchedule, GenerationStage},
};

/// A single-threaded, incrementally driven batch for fresh terrain only.
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
    failed: bool,
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
            failed: false,
        })
    }

    /// Add a terrain request; overlapping dependencies retain completed work.
    pub fn request(&mut self, pos: ChunkPos) -> Result<()> {
        ensure!(!self.failed, "terrain batch failed and must be discarded");
        self.schedule.request(pos, GenerationStage::Terrain)
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
