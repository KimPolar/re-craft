//! Incremental structure-start and reference generation for fresh chunks.
//!
//! This batch produces intermediate structure data only. It does not place
//! blocks, persist chunks, or make a world safe to open.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail, ensure};
use native_persistence::heightmap::BlockHeightFlags;
use native_resources::{
    density::DensityEvaluator,
    packs::{ResourceId, ResourceProvider},
    structure_biomes::StructureBiomes,
    structure_placement::{StructureSetCache, StructureSetStart},
    structure_start::SimpleStructureStart,
};
use native_world::loaded::ChunkPos;
use serde_json::Value;

use super::{
    TerrainGenerator,
    schedule::{GenerationSchedule, GenerationStage},
    structure_references::{
        StructureReferences, StructureStartBounds, collect_structure_references,
    },
};

/// Supported simple starts by structure key for a single source chunk.
pub type SimpleStarts = BTreeMap<ResourceId, SimpleStructureStart>;

/// The caller must supply the complete active structure-set list in registry
/// order, matching the generator, biome rules and resource stack. Unsupported
/// structure attempts fail the batch; they are never recorded as absent.
pub struct StructureGenerationBatch {
    generator: TerrainGenerator,
    biome_rules: StructureBiomes,
    sets: Vec<ResourceId>,
    evaluator: DensityEvaluator,
    cache: StructureSetCache,
    schedule: GenerationSchedule,
    starts: BTreeMap<ChunkPos, SimpleStarts>,
    bounds: BTreeMap<ChunkPos, Vec<StructureStartBounds>>,
    references: BTreeMap<ChunkPos, StructureReferences>,
    failed: bool,
}

impl StructureGenerationBatch {
    pub fn new(
        generator: TerrainGenerator,
        biome_rules: StructureBiomes,
        sets: Vec<ResourceId>,
    ) -> Result<Self> {
        let distinct: BTreeSet<_> = sets.iter().collect();
        ensure!(distinct.len() == sets.len(), "duplicate structure set");
        Ok(Self {
            generator,
            biome_rules,
            sets,
            evaluator: DensityEvaluator::default(),
            cache: StructureSetCache::default(),
            schedule: GenerationSchedule::new(),
            starts: BTreeMap::new(),
            bounds: BTreeMap::new(),
            references: BTreeMap::new(),
            failed: false,
        })
    }

    pub fn request_references(&mut self, pos: ChunkPos) -> Result<()> {
        ensure!(!self.failed, "structure batch failed and must be discarded");
        self.schedule
            .request(pos, GenerationStage::StructureReferences)
    }

    pub fn is_complete(&self) -> bool {
        !self.failed && self.schedule.is_complete()
    }

    pub fn starts(&self, pos: ChunkPos) -> Option<&SimpleStarts> {
        (!self.failed).then(|| self.starts.get(&pos)).flatten()
    }

    pub fn references(&self, pos: ChunkPos) -> Option<&StructureReferences> {
        (!self.failed).then(|| self.references.get(&pos)).flatten()
    }

    /// Generate one ready start or reference stage. A failure poisons this
    /// batch because the generator's biome-search history may have advanced.
    pub fn step<P: ResourceProvider>(
        &mut self,
        provider: &mut P,
        classify: &mut impl FnMut(&Value) -> Result<BlockHeightFlags>,
    ) -> Result<Option<(ChunkPos, GenerationStage)>> {
        ensure!(!self.failed, "structure batch failed and must be discarded");
        let Some((pos, stage)) = self.schedule.next_ready() else {
            if self.schedule.is_complete() {
                return Ok(None);
            }
            self.failed = true;
            bail!("structure plan has unfinished steps but no ready work");
        };
        let result = match stage {
            GenerationStage::StructureStarts => (|| {
                ensure!(
                    !self.starts.contains_key(&pos),
                    "structure starts already exist"
                );
                let mut starts = SimpleStarts::new();
                let mut bounds = Vec::new();
                for set in &self.sets {
                    let outcome = self.generator.try_generate_simple_structure_set(
                        provider,
                        &mut self.evaluator,
                        &mut self.cache,
                        &self.biome_rules,
                        set,
                        [pos.x, pos.z],
                        |id| Ok(starts.contains_key(id)),
                        &mut *classify,
                    )?;
                    if let StructureSetStart::Generated { structure, start } = outcome {
                        bounds.push(StructureStartBounds::from_simple(
                            structure.clone(),
                            &start,
                        )?);
                        ensure!(
                            starts.insert(structure, start).is_none(),
                            "duplicate structure start"
                        );
                    }
                }
                self.schedule.complete(pos, stage)?;
                self.starts.insert(pos, starts);
                self.bounds.insert(pos, bounds);
                Ok(())
            })(),
            GenerationStage::StructureReferences => (|| {
                ensure!(
                    !self.references.contains_key(&pos),
                    "structure references already exist"
                );
                let refs = collect_structure_references(pos, |source| {
                    self.bounds
                        .get(&source)
                        .cloned()
                        .with_context(|| format!("structure starts missing for {source:?}"))
                })?;
                self.schedule.complete(pos, stage)?;
                self.references.insert(pos, refs);
                Ok(())
            })(),
            _ => Err(anyhow::anyhow!("unsupported structure stage {stage:?}")),
        };
        if let Err(error) = result {
            self.failed = true;
            return Err(error);
        }
        Ok(Some((pos, stage)))
    }
}
