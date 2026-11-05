//! Incremental structure-start and reference generation for fresh chunks.
//!
//! This batch produces intermediate structure data only. It does not place
//! blocks, persist chunks, or make a world safe to open.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use native_core::beardifier::{Beardifier, TerrainAdjustment};
use native_persistence::heightmap::BlockHeightFlags;
use native_resources::{
    carver::{schedule::CarverCache, tags::CarverBlockTags},
    density::DensityEvaluator,
    packs::{ResourceId, ResourceProvider},
    structure_biomes::StructureBiomes,
    structure_placement::{StructureSetCache, StructureSetStart},
    structure_start::SimpleStructureStart,
    surface_pass::SurfaceStageChunk,
};
use native_world::loaded::ChunkPos;
use serde_json::Value;

use super::{
    BiomeNeighborhood, CarverBlockState, TerrainGenerator,
    schedule::{GenerationSchedule, GenerationStage},
    structure_references::{
        StructureReferences, StructureStartBounds, collect_structure_references,
    },
};

/// Supported simple starts by structure key for a single source chunk.
pub type SimpleStarts = BTreeMap<ResourceId, SimpleStructureStart>;

/// Identity of a referenced structure start, including its source chunk.
pub type TerrainStartKey = (ResourceId, ChunkPos);

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

    /// All referenced starts with terrain adaptation. The sorted result is a
    /// membership set, not Java's hash-map and hash-set iteration order.
    pub fn terrain_start_keys(&self, pos: ChunkPos) -> Result<BTreeSet<TerrainStartKey>> {
        let refs = self
            .references(pos)
            .context("structure references are unavailable")?;
        let mut keys = BTreeSet::new();
        for (structure, chunks) in refs {
            for &packed in chunks {
                let bits = packed as u64;
                let source = ChunkPos {
                    x: bits as u32 as i32,
                    z: (bits >> 32) as u32 as i32,
                };
                let start = self
                    .starts(source)
                    .and_then(|starts| starts.get(structure))
                    .context("referenced structure start is missing")?;
                if start.terrain_adjustment != TerrainAdjustment::None {
                    keys.insert((structure.clone(), source));
                }
            }
        }
        Ok(keys)
    }

    /// The caller must supply Java's start iteration order, including every
    /// key returned by `terrain_start_keys` exactly once. This matters when
    /// floating-point contributions from several structures are combined.
    pub fn beardifier_for_chunk(
        &self,
        pos: ChunkPos,
        order: &[TerrainStartKey],
    ) -> Result<Arc<Beardifier>> {
        let expected = self.terrain_start_keys(pos)?;
        let supplied: BTreeSet<_> = order.iter().cloned().collect();
        ensure!(
            order.len() == supplied.len() && supplied == expected,
            "structure terrain order does not cover references exactly"
        );
        let mut pieces = Vec::with_capacity(order.len());
        for (structure, source) in order {
            let start = self
                .starts(*source)
                .and_then(|starts| starts.get(structure))
                .context("referenced structure start is missing")?;
            pieces.push(start.terrain_piece());
        }
        Ok(Arc::new(
            Beardifier::for_chunk([pos.x, pos.z], &pieces).map_err(anyhow::Error::msg)?,
        ))
    }

    /// Generate intermediate terrain using the referenced simple pieces. The
    /// biome neighborhood, carver tags, cache and provider must belong to the
    /// same seed, dimension and immutable data-pack stack as this batch.
    /// Structure blocks, decoration, lighting and persistence remain separate.
    #[allow(clippy::too_many_arguments)]
    pub fn generate_carved_surface<P: ResourceProvider>(
        &mut self,
        provider: &mut P,
        pos: ChunkPos,
        neighborhood: &BiomeNeighborhood,
        tags: &CarverBlockTags,
        cache: &mut CarverCache,
        order: &[TerrainStartKey],
        classify: impl FnMut(&Value) -> Result<CarverBlockState>,
    ) -> Result<SurfaceStageChunk> {
        ensure!(!self.failed, "structure batch failed and must be discarded");
        let beardifier = self.beardifier_for_chunk(pos, order)?;
        let result = self
            .generator
            .generate_carved_surface_with_structure_context(
                provider,
                &mut self.evaluator,
                cache,
                [pos.x, pos.z],
                neighborhood,
                tags,
                beardifier,
                classify,
            );
        if result.is_err() {
            self.failed = true;
        }
        result
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
