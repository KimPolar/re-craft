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
    structure_decoration::{StructureDecorationEntry, StructureDecorationSchedule},
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
        OrderedStructureReferences, StructureReferences, StructureStartBounds,
        collect_ordered_structure_references, collect_references_with_identity_hashes,
    },
};

/// Supported simple starts by structure key for a single source chunk.
pub type SimpleStarts = BTreeMap<ResourceId, SimpleStructureStart>;

/// Identity of a referenced structure start, including its source chunk.
pub type TerrainStartKey = (ResourceId, ChunkPos);

/// A registry-indexed structure with its referenced starts in Java's set
/// order. All starts share the random stream seeded by `entry`.
pub struct PlannedStructureDecoration<'a> {
    pub entry: &'a StructureDecorationEntry,
    pub starts: Vec<&'a SimpleStructureStart>,
}

/// One structure kind has a known per-kind reference order. Two total
/// contributions also commute under Java's finite IEEE-754 addition.
fn unambiguous_structure_keys(starts: &BTreeSet<TerrainStartKey>) -> Option<Vec<ResourceId>> {
    let kinds: BTreeSet<_> = starts.iter().map(|(id, _)| id.clone()).collect();
    (kinds.len() <= 1 || starts.len() <= 2).then(|| kinds.into_iter().collect())
}

/// Validated structure stages retained for an intermediate terrain batch.
pub(super) struct PreparedStructureStages {
    pub(super) starts: BTreeMap<ChunkPos, SimpleStarts>,
    pub(super) references: BTreeMap<ChunkPos, OrderedStructureReferences>,
    pub(super) terrain: BTreeMap<ChunkPos, Arc<Beardifier>>,
}

impl PreparedStructureStages {
    pub(super) fn decoration_plan<'a>(
        &'a self,
        pos: ChunkPos,
        schedule: &'a StructureDecorationSchedule,
    ) -> Result<Vec<PlannedStructureDecoration<'a>>> {
        decoration_plan(&self.starts, &self.references, pos, schedule)
    }
}

fn decoration_plan<'a>(
    starts_by_chunk: &'a BTreeMap<ChunkPos, SimpleStarts>,
    references_by_chunk: &'a BTreeMap<ChunkPos, OrderedStructureReferences>,
    pos: ChunkPos,
    schedule: &'a StructureDecorationSchedule,
) -> Result<Vec<PlannedStructureDecoration<'a>>> {
    let references = references_by_chunk
        .get(&pos)
        .context("structure references are unavailable")?;
    let registered: BTreeSet<_> = schedule
        .steps()
        .iter()
        .flatten()
        .map(|entry| &entry.id)
        .collect();
    ensure!(
        references
            .membership()
            .keys()
            .all(|id| registered.contains(id)),
        "structure reference is absent from decoration registry"
    );
    let mut result = Vec::new();
    for entry in schedule.steps().iter().flatten() {
        let Some(order) = references.order(&entry.id) else {
            continue;
        };
        let mut starts = Vec::with_capacity(order.len());
        for &packed in order {
            let bits = packed as u64;
            let source = ChunkPos {
                x: bits as u32 as i32,
                z: (bits >> 32) as u32 as i32,
            };
            let start = starts_by_chunk
                .get(&source)
                .and_then(|starts| starts.get(&entry.id))
                .context("referenced structure start is missing")?;
            ensure!(
                start.chunk == [source.x, source.z],
                "structure start source differs"
            );
            starts.push(start);
        }
        result.push(PlannedStructureDecoration { entry, starts });
    }
    Ok(result)
}

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
    references: BTreeMap<ChunkPos, OrderedStructureReferences>,
    failed: bool,
}

impl StructureGenerationBatch {
    /// Freeze completed structure stages and transfer their generator to the
    /// terrain batch. Orders must be Java's actual start iteration order.
    pub(super) fn into_terrain_stages(
        self,
        orders: BTreeMap<ChunkPos, Vec<TerrainStartKey>>,
    ) -> Result<(TerrainGenerator, PreparedStructureStages)> {
        ensure!(
            self.is_complete(),
            "structure batch has unfinished or failed work"
        );
        ensure!(
            self.generator.structure_context,
            "generator lacks structure context"
        );
        let mut terrain = BTreeMap::new();
        for (pos, order) in orders {
            terrain.insert(pos, self.beardifier_for_chunk(pos, &order)?);
        }
        Ok((
            self.generator,
            PreparedStructureStages {
                starts: self.starts,
                references: self.references,
                terrain,
            },
        ))
    }

    pub(super) fn generator(&self) -> &TerrainGenerator {
        &self.generator
    }

    pub(super) fn invalidate(&mut self) {
        self.failed = true;
    }

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
        (!self.failed)
            .then(|| {
                self.references
                    .get(&pos)
                    .map(OrderedStructureReferences::membership)
            })
            .flatten()
    }

    /// References for one structure in the order Java's long set iterates.
    /// The caller must still determine the order between structure keys.
    pub fn reference_order(&self, pos: ChunkPos, structure: &ResourceId) -> Option<&[i64]> {
        (!self.failed)
            .then(|| {
                self.references
                    .get(&pos)
                    .and_then(|refs| refs.order(structure))
            })
            .flatten()
    }

    /// Prepare the structures that actually have starts in this chunk. The
    /// schedule must cover the complete registry, including absent starts,
    /// so each selected entry retains Java's per-step seed index.
    pub fn decoration_plan<'a>(
        &'a self,
        pos: ChunkPos,
        schedule: &'a StructureDecorationSchedule,
    ) -> Result<Vec<PlannedStructureDecoration<'a>>> {
        ensure!(!self.failed, "structure batch failed and must be discarded");
        decoration_plan(&self.starts, &self.references, pos, schedule)
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

    /// Traverse each structure's references in Java's set order. The caller
    /// must supply Java's order for the structure keys that have terrain
    /// adaptations; supplying an incomplete or duplicate order fails.
    pub fn terrain_start_order(
        &self,
        pos: ChunkPos,
        structure_order: &[ResourceId],
    ) -> Result<Vec<TerrainStartKey>> {
        let expected = self.terrain_start_keys(pos)?;
        let mut seen = BTreeSet::new();
        let mut result = Vec::with_capacity(expected.len());
        for structure in structure_order {
            ensure!(
                seen.insert(structure),
                "duplicate structure in terrain order"
            );
            let references = self
                .reference_order(pos, structure)
                .context("structure in terrain order has no references")?;
            for &packed in references {
                let bits = packed as u64;
                let source = ChunkPos {
                    x: bits as u32 as i32,
                    z: (bits >> 32) as u32 as i32,
                };
                let key = (structure.clone(), source);
                if expected.contains(&key) {
                    result.push(key);
                }
            }
        }
        ensure!(
            result.len() == expected.len()
                && result.iter().cloned().collect::<BTreeSet<_>>() == expected,
            "structure terrain order does not cover references exactly"
        );
        Ok(result)
    }

    /// Infer a Java-equivalent order without object identity hashes only when
    /// there is one contributing structure kind or at most two contributions.
    /// Multiple additions of different kinds may round differently and remain
    /// unresolved unless the actual Java structure-key order is supplied.
    pub fn unambiguous_terrain_start_order(
        &self,
        pos: ChunkPos,
    ) -> Result<Option<Vec<TerrainStartKey>>> {
        let starts = self.terrain_start_keys(pos)?;
        let Some(kinds) = unambiguous_structure_keys(&starts) else {
            return Ok(None);
        };
        Ok(Some(self.terrain_start_order(pos, &kinds)?))
    }

    /// Prepare terrain orders for all requested chunks without guessing any
    /// identity-dependent ordering. Duplicate or unresolved chunks fail.
    pub fn unambiguous_terrain_orders(
        &self,
        positions: impl IntoIterator<Item = ChunkPos>,
    ) -> Result<BTreeMap<ChunkPos, Vec<TerrainStartKey>>> {
        let mut result = BTreeMap::new();
        for pos in positions {
            let order = self
                .unambiguous_terrain_start_order(pos)?
                .with_context(|| format!("structure terrain order is ambiguous for {pos:?}"))?;
            ensure!(
                result.insert(pos, order).is_none(),
                "duplicate terrain chunk"
            );
        }
        Ok(result)
    }

    /// Recover the full Java order when the structure identity hashes from the
    /// corresponding Java process are available. Hashes are not seed-derived.
    pub fn terrain_start_order_with_identity_hashes(
        &self,
        pos: ChunkPos,
        hashes: &BTreeMap<ResourceId, i32>,
    ) -> Result<Vec<TerrainStartKey>> {
        let original = self
            .references(pos)
            .context("structure references are unavailable")?;
        let ordered = collect_references_with_identity_hashes(
            pos,
            |source| {
                self.bounds
                    .get(&source)
                    .cloned()
                    .context("structure starts are missing")
            },
            hashes,
        )?;
        ensure!(
            ordered.membership() == original,
            "structure references changed"
        );
        self.terrain_start_order(
            pos,
            ordered
                .structure_order()
                .context("structure key order is missing")?,
        )
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
                let refs = collect_ordered_structure_references(pos, |source| {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inferred_structure_order_requires_unambiguous_float_sum() {
        let a = ResourceId::parse("test:a").unwrap();
        let b = ResourceId::parse("test:b").unwrap();
        let pos = |x| ChunkPos { x, z: 0 };
        assert_eq!(unambiguous_structure_keys(&BTreeSet::new()), Some(vec![]));
        let mut starts = BTreeSet::from([
            (a.clone(), pos(0)),
            (a.clone(), pos(1)),
            (a.clone(), pos(2)),
        ]);
        assert_eq!(unambiguous_structure_keys(&starts), Some(vec![a.clone()]));
        starts = BTreeSet::from([(a.clone(), pos(0)), (b.clone(), pos(1))]);
        assert_eq!(
            unambiguous_structure_keys(&starts),
            Some(vec![a.clone(), b.clone()])
        );
        starts.insert((b, pos(2)));
        assert_eq!(unambiguous_structure_keys(&starts), None);
    }
}
