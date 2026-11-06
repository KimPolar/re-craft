//! Incremental structure-start and reference generation for fresh chunks.
//!
//! This batch produces intermediate structure data only. It does not place
//! blocks, persist chunks, or make a world safe to open.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use native_core::beardifier::{Beardifier, PieceProjection, StructurePiece, TerrainAdjustment};
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
    BiomeNeighborhood, CarverBlockState, PreparedStructureStart, TerrainGenerator,
    schedule::{GenerationSchedule, GenerationStage},
    structure_references::{
        OrderedStructureReferences, StructureReferences, StructureStartBounds,
        collect_ordered_structure_references, collect_references_with_identity_hashes,
    },
};

/// Supported simple starts by structure key for a single source chunk.
pub type SimpleStarts = BTreeMap<ResourceId, SimpleStructureStart>;
/// Multi-piece ocean-ruin starts belonging to one source chunk.
pub type OceanRuinStarts = BTreeMap<ResourceId, super::ocean_ruin::PreparedStart>;

/// Identity of a referenced structure start, including its source chunk.
pub type TerrainStartKey = (ResourceId, ChunkPos);

/// A structure start's current single-piece bounds during a decoration pass.
pub type StructurePieceRevision = (TerrainStartKey, native_core::beardifier::BoundingBox);

/// A registry-indexed structure with its referenced starts in Java's set
/// order. All starts share the random stream seeded by `entry`.
pub struct PlannedStructureDecoration<'a> {
    pub entry: &'a StructureDecorationEntry,
    pub starts: Vec<&'a SimpleStructureStart>,
}

/// One referenced start, retaining all of its pieces and source metadata.
#[derive(Clone, Copy)]
pub enum PlannedStructureStart<'a> {
    Simple(&'a SimpleStructureStart),
    OceanRuin(&'a super::ocean_ruin::PreparedStart),
}

impl PlannedStructureStart<'_> {
    pub fn chunk(&self) -> [i32; 2] {
        match self {
            Self::Simple(start) => start.chunk,
            Self::OceanRuin(start) => start.chunk,
        }
    }
}

/// Starts for one registry entry in Java's reference-set order.
pub struct PlannedMixedStructureDecoration<'a> {
    pub entry: &'a StructureDecorationEntry,
    pub starts: Vec<PlannedStructureStart<'a>>,
}

/// Structure and terrain coverage required by a set of later chunk stages.
/// This uses the same dependency expansion as the generation scheduler.
pub struct StructurePreparationPlan {
    starts: BTreeSet<ChunkPos>,
    references: BTreeSet<ChunkPos>,
    terrain: BTreeSet<ChunkPos>,
}

impl StructurePreparationPlan {
    pub fn new(requests: impl IntoIterator<Item = (ChunkPos, GenerationStage)>) -> Result<Self> {
        let mut schedule = GenerationSchedule::new();
        schedule.request_many(requests)?;
        ensure!(schedule.requested_chunks() > 0, "no generation requests");
        let mut plan = Self {
            starts: BTreeSet::new(),
            references: BTreeSet::new(),
            terrain: BTreeSet::new(),
        };
        for (pos, stage) in schedule.requirements() {
            if stage >= GenerationStage::StructureStarts {
                plan.starts.insert(pos);
            }
            if stage >= GenerationStage::StructureReferences {
                plan.references.insert(pos);
            }
            if stage >= GenerationStage::Terrain {
                plan.terrain.insert(pos);
            }
        }
        Ok(plan)
    }

    pub fn starts(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.starts.iter().copied()
    }
    pub fn references(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.references.iter().copied()
    }
    pub fn terrain(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.terrain.iter().copied()
    }

    /// Request all prerequisites together; coordinate failures leave the
    /// structure batch's existing schedule unchanged.
    pub fn request_structures(&self, batch: &mut StructureGenerationBatch) -> Result<()> {
        ensure!(
            !batch.failed,
            "structure batch failed and must be discarded"
        );
        let requests = self
            .starts()
            .map(|pos| (pos, GenerationStage::StructureStarts))
            .chain(
                self.references()
                    .map(|pos| (pos, GenerationStage::StructureReferences)),
            );
        batch.schedule.request_many(requests)
    }

    /// Transfer completed structure work when all terrain orders are known to
    /// be independent of Java's identity-dependent iteration order.
    pub fn into_terrain_batch(
        &self,
        batch: StructureGenerationBatch,
        tags: CarverBlockTags,
        dimension: &str,
    ) -> Result<super::FreshTerrainBatch> {
        ensure!(
            batch.is_complete(),
            "structure batch has unfinished or failed work"
        );
        ensure!(
            self.starts().all(|pos| batch.has_starts(pos)),
            "structure starts required by plan are missing"
        );
        ensure!(
            self.references().all(|pos| batch.references(pos).is_some()),
            "structure references required by plan are missing"
        );
        super::FreshTerrainBatch::new_with_unambiguous_structures(
            batch,
            self.terrain(),
            tags,
            dimension,
        )
    }

    /// Keep structure generation state for later requests. The terrain
    /// generator must be the paired generator compiled from the same stack.
    pub fn into_expandable_terrain_batch(
        &self,
        structures: StructureGenerationBatch,
        terrain_generator: TerrainGenerator,
        tags: CarverBlockTags,
        dimension: &str,
    ) -> Result<super::FreshTerrainBatch> {
        ensure!(
            Arc::ptr_eq(
                &structures.generator.pair_identity,
                &terrain_generator.pair_identity
            ),
            "structure and terrain generators are not paired"
        );
        let stages = structures.prepare_additional_stages(self, None)?;
        super::FreshTerrainBatch::new_with_retained_structures(
            terrain_generator,
            structures,
            stages,
            tags,
            dimension,
        )
    }
}

/// One structure kind has a known per-kind reference order. Two total
/// contributions also commute under Java's finite IEEE-754 addition.
fn unambiguous_structure_keys(starts: &BTreeSet<TerrainStartKey>) -> Option<Vec<ResourceId>> {
    let kinds: BTreeSet<_> = starts.iter().map(|(id, _)| id.clone()).collect();
    (kinds.len() <= 1 || starts.len() <= 2).then(|| kinds.into_iter().collect())
}

/// Validated structure stages retained for an intermediate terrain batch.
#[derive(Default)]
pub(super) struct PreparedStructureStages {
    pub(super) starts: BTreeMap<ChunkPos, SimpleStarts>,
    pub(super) ocean_starts: BTreeMap<ChunkPos, OceanRuinStarts>,
    pub(super) huts: BTreeMap<TerrainStartKey, super::SavedSwampHutPiece>,
    pub(super) references: BTreeMap<ChunkPos, OrderedStructureReferences>,
    pub(super) terrain: BTreeMap<ChunkPos, Arc<Beardifier>>,
}

impl PreparedStructureStages {
    fn insert_ocean_starts(&mut self, pos: ChunkPos, starts: OceanRuinStarts) -> Result<()> {
        ensure!(
            self.starts.contains_key(&pos),
            "structure start chunk is missing"
        );
        ensure!(
            !self.ocean_starts.contains_key(&pos),
            "ocean ruin starts already prepared"
        );
        for (id, start) in &starts {
            ensure!(
                start.chunk == [pos.x, pos.z],
                "ocean ruin belongs to another chunk"
            );
            ensure!(
                !self.starts[&pos].contains_key(id),
                "duplicate structure start"
            );
            ensure!(!start.pieces.is_empty(), "ocean ruin start has no pieces");
            for piece in &start.pieces {
                super::ocean_ruin::SavedOceanRuinPiece::capture(piece, start.temperature)?;
            }
        }
        if !starts.is_empty() {
            self.ocean_starts.insert(pos, starts);
        }
        Ok(())
    }

    /// Construct all state for a newly added start chunk before publishing it.
    fn insert_starts(&mut self, pos: ChunkPos, starts: SimpleStarts) -> Result<()> {
        ensure!(
            !self.starts.contains_key(&pos),
            "structure start chunk is already prepared"
        );
        let mut huts = BTreeMap::new();
        for (id, start) in &starts {
            ensure!(
                start.chunk == [pos.x, pos.z],
                "structure start belongs to another chunk"
            );
            if start.kind == native_resources::structure_start::SimpleStructureKind::SwampHut {
                let key = (id.clone(), pos);
                ensure!(
                    !self.huts.contains_key(&key),
                    "swamp hut state is already prepared"
                );
                huts.insert(
                    key,
                    super::SavedSwampHutPiece::capture(start, Default::default())?,
                );
            }
        }
        self.huts.extend(huts);
        self.starts.insert(pos, starts);
        Ok(())
    }

    /// Validate the complete decoration result before publishing any start
    /// or hut-state update. `hut_updates` can be the complete staged map.
    pub(super) fn commit_decorated_starts(
        &mut self,
        keys: Vec<Vec<TerrainStartKey>>,
        updated: Vec<Vec<SimpleStructureStart>>,
        hut_updates: Option<&BTreeMap<TerrainStartKey, super::SavedSwampHutPiece>>,
    ) -> Result<()> {
        ensure!(
            keys.len() == updated.len(),
            "structure decoration result shape differs from plan"
        );
        let mut replacements = Vec::new();
        let mut hut_replacements = Vec::new();
        let mut seen = BTreeSet::new();
        for (keys, starts) in keys.into_iter().zip(updated) {
            ensure!(
                keys.len() == starts.len(),
                "structure decoration start count differs from plan"
            );
            for (key, start) in keys.into_iter().zip(starts) {
                ensure!(
                    seen.insert(key.clone()),
                    "duplicate decorated structure start"
                );
                let original = self
                    .starts
                    .get(&key.1)
                    .and_then(|starts| starts.get(&key.0))
                    .context("decorated structure start is missing")?;
                ensure!(
                    original.kind == start.kind && start.chunk == [key.1.x, key.1.z],
                    "decorated structure identity changed"
                );
                if start.kind == native_resources::structure_start::SimpleStructureKind::SwampHut {
                    let source = hut_updates.unwrap_or(&self.huts);
                    let saved = *source.get(&key).context("decorated hut state is missing")?;
                    ensure!(
                        super::SavedSwampHutPiece::capture(&start, saved.state)? == saved,
                        "decorated hut state differs from its piece"
                    );
                    hut_replacements.push((key.clone(), saved));
                }
                replacements.push((key, start));
            }
        }
        for (key, start) in replacements {
            *self
                .starts
                .get_mut(&key.1)
                .and_then(|starts| starts.get_mut(&key.0))
                .expect("start was validated") = start;
        }
        self.huts.extend(hut_replacements);
        Ok(())
    }
    /// Check membership only; entity-source coverage does not depend on the
    /// structure decoration iteration order.
    pub(super) fn only_accounted_structure_references(
        &self,
        pos: ChunkPos,
        registered: &BTreeSet<&ResourceId>,
        huts_recorded: bool,
    ) -> bool {
        self.references.get(&pos).is_some_and(|references| {
            references.membership().iter().all(|(id, positions)| {
                registered.contains(id) && positions.iter().all(|packed| {
                    let bits = *packed as u64;
                    let source = ChunkPos { x: bits as u32 as i32, z: (bits >> 32) as u32 as i32 };
                    self.starts.get(&source).and_then(|starts| starts.get(id)).is_some_and(|start| {
                        if start.chunk != [source.x, source.z] { return false; }
                        match start.kind {
                            native_resources::structure_start::SimpleStructureKind::BuriedTreasure => true,
                            native_resources::structure_start::SimpleStructureKind::SwampHut if huts_recorded => {
                                self.huts.get(&(id.clone(), source)).is_some_and(|saved| {
                                    super::SavedSwampHutPiece::capture(start, saved.state).ok().as_ref() == Some(saved)
                                })
                            }
                            _ => false,
                        }
                    })
                })
            })
        })
    }

    pub(super) fn decoration_plan<'a>(
        &'a self,
        pos: ChunkPos,
        schedule: &'a StructureDecorationSchedule,
    ) -> Result<Vec<PlannedStructureDecoration<'a>>> {
        decoration_plan(
            &self.starts,
            &self.ocean_starts,
            &self.references,
            pos,
            schedule,
        )
    }

    pub(super) fn mixed_decoration_plan<'a>(
        &'a self,
        pos: ChunkPos,
        schedule: &'a StructureDecorationSchedule,
    ) -> Result<Vec<PlannedMixedStructureDecoration<'a>>> {
        mixed_decoration_plan(
            &self.starts,
            &self.ocean_starts,
            &self.references,
            pos,
            schedule,
        )
    }

    /// Prove that every mixed-plan key points to exactly one retained start.
    /// The mixed executor preserves this shape and validates ruin mutations.
    pub(super) fn mixed_commit_keys(
        &self,
        plan: &[PlannedMixedStructureDecoration<'_>],
        allow_huts: bool,
    ) -> Result<Vec<Vec<TerrainStartKey>>> {
        let mut seen = BTreeSet::new();
        plan.iter().map(|entry| entry.starts.iter().map(|start| {
            let [x, z] = start.chunk();
            let key = (entry.entry.id.clone(), ChunkPos { x, z });
            ensure!(seen.insert(key.clone()), "duplicate decorated structure start");
            match start {
                PlannedStructureStart::Simple(start) => {
                    let current = self.starts.get(&key.1).and_then(|s| s.get(&key.0)).context("simple start missing")?;
                    ensure!(std::ptr::eq(*start, current), "mixed plan has stale simple start");
                    match start.kind {
                        native_resources::structure_start::SimpleStructureKind::BuriedTreasure => {},
                        native_resources::structure_start::SimpleStructureKind::SwampHut if allow_huts => {
                            let saved = self.huts.get(&key).context("swamp hut state missing")?;
                            ensure!(super::SavedSwampHutPiece::capture(start, saved.state)? == *saved,
                                "swamp hut state differs from its start");
                        }
                        _ => bail!("mixed structure requires an unsupported decoration context"),
                    }
                }
                PlannedStructureStart::OceanRuin(start) => {
                    let current = self.ocean_starts.get(&key.1).and_then(|s| s.get(&key.0)).context("ocean ruin start missing")?;
                    ensure!(std::ptr::eq(*start, current), "mixed plan has stale ocean start");
                }
            }
            Ok(key)
        }).collect()).collect()
    }

    /// Apply only an executor result whose keys were proved by
    /// `mixed_commit_keys`; the executor constructs and preserves its shape.
    pub(super) fn commit_mixed_starts(
        &mut self,
        keys: Vec<Vec<TerrainStartKey>>,
        updated: Vec<Vec<super::PreparedStructureStart>>,
        huts: BTreeMap<TerrainStartKey, super::SavedSwampHutPiece>,
    ) {
        assert_eq!(
            keys.len(),
            updated.len(),
            "validated structure result shape changed"
        );
        for (keys, starts) in keys.into_iter().zip(updated) {
            assert_eq!(
                keys.len(),
                starts.len(),
                "validated structure start count changed"
            );
            for (key, start) in keys.into_iter().zip(starts) {
                match start {
                    super::PreparedStructureStart::Simple(start) => {
                        *self
                            .starts
                            .get_mut(&key.1)
                            .and_then(|s| s.get_mut(&key.0))
                            .expect("validated simple start") = start
                    }
                    super::PreparedStructureStart::OceanRuin(start) => {
                        *self
                            .ocean_starts
                            .get_mut(&key.1)
                            .and_then(|s| s.get_mut(&key.0))
                            .expect("validated ocean start") = start
                    }
                }
            }
        }
        self.huts.extend(huts);
    }
}

fn decoration_plan<'a>(
    starts_by_chunk: &'a BTreeMap<ChunkPos, SimpleStarts>,
    ocean_by_chunk: &'a BTreeMap<ChunkPos, OceanRuinStarts>,
    references_by_chunk: &'a BTreeMap<ChunkPos, OrderedStructureReferences>,
    pos: ChunkPos,
    schedule: &'a StructureDecorationSchedule,
) -> Result<Vec<PlannedStructureDecoration<'a>>> {
    mixed_decoration_plan(
        starts_by_chunk,
        ocean_by_chunk,
        references_by_chunk,
        pos,
        schedule,
    )?
    .into_iter()
    .map(|entry| {
        let starts = entry
            .starts
            .into_iter()
            .map(|start| match start {
                PlannedStructureStart::Simple(start) => Ok(start),
                PlannedStructureStart::OceanRuin(_) => {
                    bail!("ocean ruin decoration context is missing")
                }
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(PlannedStructureDecoration {
            entry: entry.entry,
            starts,
        })
    })
    .collect()
}

fn mixed_decoration_plan<'a>(
    starts_by_chunk: &'a BTreeMap<ChunkPos, SimpleStarts>,
    ocean_by_chunk: &'a BTreeMap<ChunkPos, OceanRuinStarts>,
    references_by_chunk: &'a BTreeMap<ChunkPos, OrderedStructureReferences>,
    pos: ChunkPos,
    schedule: &'a StructureDecorationSchedule,
) -> Result<Vec<PlannedMixedStructureDecoration<'a>>> {
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
            let simple = starts_by_chunk
                .get(&source)
                .context("referenced structure start chunk is missing")?
                .get(&entry.id);
            let ocean = ocean_by_chunk
                .get(&source)
                .and_then(|starts| starts.get(&entry.id));
            let start = match (simple, ocean) {
                (Some(start), None) => PlannedStructureStart::Simple(start),
                (None, Some(start)) => {
                    ensure!(!start.pieces.is_empty(), "ocean ruin start has no pieces");
                    PlannedStructureStart::OceanRuin(start)
                }
                (None, None) => bail!("referenced structure start is missing"),
                (Some(_), Some(_)) => bail!("duplicate referenced structure start"),
            };
            ensure!(
                start.chunk() == [source.x, source.z],
                "structure start source differs"
            );
            starts.push(start);
        }
        result.push(PlannedMixedStructureDecoration { entry, starts });
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
    ocean_starts: BTreeMap<ChunkPos, OceanRuinStarts>,
    bounds: BTreeMap<ChunkPos, Vec<StructureStartBounds>>,
    references: BTreeMap<ChunkPos, OrderedStructureReferences>,
    failed: bool,
}

impl StructureGenerationBatch {
    /// Assemble only missing inputs. No existing terrain contexts are changed.
    pub(super) fn prepare_additional_stages(
        &self,
        plan: &StructurePreparationPlan,
        existing: Option<&PreparedStructureStages>,
    ) -> Result<PreparedStructureStages> {
        ensure!(
            self.is_complete(),
            "structure batch has unfinished or failed work"
        );
        let mut added = PreparedStructureStages::default();
        for pos in plan.starts() {
            let starts = self
                .starts
                .get(&pos)
                .context("structure starts required by plan are missing")?;
            if existing.is_none_or(|old| !old.starts.contains_key(&pos)) {
                added.insert_starts(pos, starts.clone())?;
                if let Some(ocean) = self.ocean_starts.get(&pos) {
                    added.insert_ocean_starts(pos, ocean.clone())?;
                }
            }
        }
        for pos in plan.references() {
            let references = self
                .references
                .get(&pos)
                .context("structure references required by plan are missing")?;
            if existing.is_none_or(|old| !old.references.contains_key(&pos)) {
                added.references.insert(pos, references.clone());
            }
        }
        for pos in plan.terrain() {
            if existing.is_none_or(|old| !old.terrain.contains_key(&pos)) {
                let order = self
                    .unambiguous_terrain_start_order(pos)?
                    .with_context(|| format!("structure terrain order is ambiguous for {pos:?}"))?;
                added
                    .terrain
                    .insert(pos, self.beardifier_for_chunk(pos, &order)?);
            }
        }
        Ok(added)
    }

    /// Prepare a normal resource-loaded registry in Java's registration
    /// order, filtering out sets with no possible generator biome.
    pub fn from_registry(
        generator: TerrainGenerator,
        provider: &mut impl ResourceProvider,
    ) -> Result<Self> {
        ensure!(
            generator.structure_context,
            "generator lacks structure context"
        );
        let mut cache = StructureSetCache::default();
        let (sets, biome_rules) =
            cache.possible_registry_sets(provider, generator.possible_biomes())?;
        let mut batch = Self::new(generator, biome_rules, sets)?;
        batch.cache = cache;
        Ok(batch)
    }

    /// Validate an observed Java normal-generator order against the complete
    /// registry and possible biomes before attempting any structure starts.
    /// Unsupported starts still fail when a candidate is generated.
    pub fn from_registry_order(
        generator: TerrainGenerator,
        provider: &mut impl ResourceProvider,
        sets: Vec<ResourceId>,
    ) -> Result<Self> {
        ensure!(
            generator.structure_context,
            "generator lacks structure context"
        );
        let mut cache = StructureSetCache::default();
        let (eligible, biome_rules) =
            cache.possible_registry_membership(provider, generator.possible_biomes())?;
        ensure!(
            sets.len() == eligible.len()
                && sets.iter().cloned().collect::<BTreeSet<_>>() == eligible,
            "structure set order does not cover the eligible registry exactly"
        );
        let mut batch = Self::new(generator, biome_rules, sets)?;
        batch.cache = cache;
        Ok(batch)
    }

    /// Registry-ordered sets selected for this generator's biome source.
    pub fn structure_sets(&self) -> &[ResourceId] {
        &self.sets
    }

    /// Compute the generator's initial origin before preparing its structures.
    /// The provider must be the same unchanged data-pack stack as this batch.
    pub fn spawn_origin(&self, provider: &mut impl ResourceProvider) -> Result<ChunkPos> {
        ensure!(!self.failed, "structure batch failed and must be discarded");
        let [x, z] = self.generator.spawn_origin(provider)?;
        Ok(ChunkPos { x, z })
    }

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
        let mut stages = PreparedStructureStages {
            references: self.references,
            terrain,
            ..Default::default()
        };
        let mut ocean_starts = self.ocean_starts;
        for (pos, starts) in self.starts {
            stages.insert_starts(pos, starts)?;
            if let Some(ocean) = ocean_starts.remove(&pos) {
                stages.insert_ocean_starts(pos, ocean)?;
            }
        }
        ensure!(
            ocean_starts.is_empty(),
            "ocean ruin source chunk is missing"
        );
        Ok((self.generator, stages))
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
            ocean_starts: BTreeMap::new(),
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

    /// Request starts without requiring neighboring reference stages.
    pub fn request_starts(&mut self, pos: ChunkPos) -> Result<()> {
        ensure!(!self.failed, "structure batch failed and must be discarded");
        self.schedule.request(pos, GenerationStage::StructureStarts)
    }

    pub fn is_complete(&self) -> bool {
        !self.failed && self.schedule.is_complete()
    }

    /// Return starts only when this chunk has no multi-piece starts, so a
    /// simple-only caller cannot accidentally publish an incomplete chunk.
    pub fn starts(&self, pos: ChunkPos) -> Option<&SimpleStarts> {
        (!self.failed && !self.ocean_starts.contains_key(&pos))
            .then(|| self.starts.get(&pos))
            .flatten()
    }

    /// Whether the start stage is complete, including multi-piece starts.
    pub fn has_starts(&self, pos: ChunkPos) -> bool {
        !self.failed && self.starts.contains_key(&pos)
    }

    /// Return all ocean ruins generated for a completed start chunk.
    pub fn ocean_ruin_starts(&self, pos: ChunkPos) -> Option<&OceanRuinStarts> {
        (!self.failed && self.starts.contains_key(&pos))
            .then(|| self.ocean_starts.get(&pos))
            .flatten()
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
        decoration_plan(
            &self.starts,
            &self.ocean_starts,
            &self.references,
            pos,
            schedule,
        )
    }

    /// Prepare simple and ocean-ruin starts without losing reference order.
    pub fn mixed_decoration_plan<'a>(
        &'a self,
        pos: ChunkPos,
        schedule: &'a StructureDecorationSchedule,
    ) -> Result<Vec<PlannedMixedStructureDecoration<'a>>> {
        ensure!(!self.failed, "structure batch failed and must be discarded");
        mixed_decoration_plan(
            &self.starts,
            &self.ocean_starts,
            &self.references,
            pos,
            schedule,
        )
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
                let adjustment =
                    if let Some(start) = self.starts.get(&source).and_then(|s| s.get(structure)) {
                        start.terrain_adjustment
                    } else {
                        self.ocean_starts
                            .get(&source)
                            .and_then(|s| s.get(structure))
                            .context("referenced structure start is missing")?
                            .terrain_adjustment
                    };
                if adjustment != TerrainAdjustment::None {
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
        // A single multi-piece start contributes multiple additions. Two
        // starts of distinct kinds therefore do not necessarily commute.
        if kinds.len() > 1 {
            let pieces = starts.iter().try_fold(0usize, |count, (id, pos)| {
                let n = if self.starts.get(pos).is_some_and(|s| s.contains_key(id)) {
                    1
                } else {
                    self.ocean_starts
                        .get(pos)
                        .and_then(|s| s.get(id))
                        .context("referenced structure start is missing")?
                        .pieces
                        .len()
                };
                count.checked_add(n).context("too many structure pieces")
            })?;
            if pieces > 2 {
                return Ok(None);
            }
        }
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
            if let Some(start) = self
                .starts
                .get(source)
                .and_then(|starts| starts.get(structure))
            {
                pieces.push(start.terrain_piece());
            } else {
                let start = self
                    .ocean_starts
                    .get(source)
                    .and_then(|starts| starts.get(structure))
                    .context("referenced structure start is missing")?;
                pieces.extend(start.pieces.iter().map(|piece| StructurePiece {
                    bounds: piece.bounds,
                    adjustment: start.terrain_adjustment,
                    ground_delta: 0,
                    projection: PieceProjection::NonPool,
                    junctions: vec![],
                }));
            }
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
                let mut ocean_starts = OceanRuinStarts::new();
                let mut bounds = Vec::new();
                for set in &self.sets {
                    let outcome = self.generator.try_generate_supported_structure_set(
                        provider,
                        &mut self.evaluator,
                        &mut self.cache,
                        &self.biome_rules,
                        set,
                        [pos.x, pos.z],
                        |id| Ok(starts.contains_key(id) || ocean_starts.contains_key(id)),
                        &mut *classify,
                    )?;
                    if let StructureSetStart::Generated { structure, start } = outcome {
                        match start {
                            PreparedStructureStart::Simple(start) => {
                                bounds.push(StructureStartBounds::from_simple(
                                    structure.clone(),
                                    &start,
                                )?);
                                ensure!(
                                    starts.insert(structure, start).is_none(),
                                    "duplicate structure start"
                                );
                            }
                            PreparedStructureStart::OceanRuin(start) => {
                                bounds.push(start.reference_bounds(structure.clone())?);
                                ensure!(
                                    ocean_starts.insert(structure, start).is_none(),
                                    "duplicate structure start"
                                );
                            }
                        }
                    }
                }
                self.schedule.complete(pos, stage)?;
                self.starts.insert(pos, starts);
                if !ocean_starts.is_empty() {
                    self.ocean_starts.insert(pos, ocean_starts);
                }
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
    fn mixed_plan_rejects_missing_or_inconsistent_starts() -> Result<()> {
        use native_core::beardifier::BoundingBox;
        use native_persistence::{
            nbt::{NamedTag, Tag},
            structure_template::StructureTemplate,
        };
        use native_resources::{
            ocean_ruin::{Piece, Rotation, Temperature},
            structure_start::SimpleStructureKind,
        };
        struct Data;
        impl ResourceProvider for Data {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                Ok((path == "data/example/worldgen/structure/ruin.json")
                    .then(|| br#"{"step":"surface_structures"}"#.to_vec()))
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                ensure!(folder == "worldgen/structure", "unexpected registry");
                Ok(vec![ResourceId::parse("example:ruin")?])
            }
        }
        let id = ResourceId::parse("example:ruin")?;
        let pos = ChunkPos { x: 0, z: 0 };
        let bounds = BoundingBox::new([1, 70, 1], [1, 70, 1]).unwrap();
        let refs = collect_ordered_structure_references(pos, |source| {
            Ok(if source == pos {
                vec![StructureStartBounds {
                    structure: id.clone(),
                    bounds,
                }]
            } else {
                vec![]
            })
        })?;
        let references = BTreeMap::from([(pos, refs)]);
        let schedule = StructureDecorationSchedule::load(&mut Data)?;
        let template = Arc::new(StructureTemplate {
            document: NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![]),
            },
            data_version: Some(5023),
            size: [1, 1, 1],
            palettes: vec![],
            blocks: vec![],
            entities: vec![],
        });
        let piece = super::super::ocean_ruin::PreparedPiece {
            selection: Piece {
                template: "minecraft:underwater_ruin/warm_1".into(),
                position: [1, 70, 1],
                rotation: Rotation::None,
                integrity: 1.0,
                large: false,
            },
            template,
            bounds,
        };
        let start = super::super::ocean_ruin::PreparedStart {
            chunk: [0, 0],
            generation_point: [8, 70, 8],
            terrain_adjustment: TerrainAdjustment::None,
            temperature: Temperature::Warm,
            pieces: vec![piece],
        };
        let mut ocean = BTreeMap::from([(pos, OceanRuinStarts::from([(id.clone(), start)]))]);
        let mut simple = BTreeMap::new();
        assert!(mixed_decoration_plan(&simple, &ocean, &references, pos, &schedule).is_err());
        simple.insert(pos, SimpleStarts::new());
        assert_eq!(
            mixed_decoration_plan(&simple, &ocean, &references, pos, &schedule)?[0]
                .starts
                .len(),
            1
        );
        let backup = ocean[&pos][&id].clone();
        ocean
            .get_mut(&pos)
            .unwrap()
            .get_mut(&id)
            .unwrap()
            .pieces
            .clear();
        assert!(mixed_decoration_plan(&simple, &ocean, &references, pos, &schedule).is_err());
        ocean.get_mut(&pos).unwrap().insert(id.clone(), backup);
        simple.get_mut(&pos).unwrap().insert(
            id,
            SimpleStructureStart {
                kind: SimpleStructureKind::BuriedTreasure,
                chunk: [0, 0],
                generation_point: [0, 0, 0],
                orientation: None,
                bounds,
                terrain_adjustment: TerrainAdjustment::None,
            },
        );
        assert!(mixed_decoration_plan(&simple, &ocean, &references, pos, &schedule).is_err());
        Ok(())
    }

    #[test]
    fn prepared_hut_states_survive_expansion_and_invalid_insertions() {
        use native_core::beardifier::BoundingBox;
        use native_resources::{
            structure_piece::ScatteredGroundHeight,
            structure_start::{HorizontalDirection, SimpleStructureKind},
        };
        let make_hut = |pos: ChunkPos| SimpleStructureStart {
            kind: SimpleStructureKind::SwampHut,
            chunk: [pos.x, pos.z],
            generation_point: [0, 64, 0],
            orientation: Some(HorizontalDirection::South),
            bounds: BoundingBox::new(
                [pos.x * 16, 64, pos.z * 16],
                [pos.x * 16 + 6, 70, pos.z * 16 + 8],
            )
            .unwrap(),
            terrain_adjustment: TerrainAdjustment::None,
        };
        let first = ChunkPos { x: 0, z: 0 };
        let second = ChunkPos { x: 1, z: 0 };
        let id = ResourceId::parse("minecraft:swamp_hut").unwrap();
        let mut stages = PreparedStructureStages::default();
        stages
            .insert_starts(first, BTreeMap::from([(id.clone(), make_hut(first))]))
            .unwrap();
        let first_key = (id.clone(), first);
        let saved = stages.huts.get_mut(&first_key).unwrap();
        saved.state.ground = ScatteredGroundHeight::from_saved(64);
        saved.state.spawned_witch = true;
        let original = *saved;
        stages
            .insert_starts(second, BTreeMap::from([(id.clone(), make_hut(second))]))
            .unwrap();
        assert_eq!(stages.huts[&first_key], original);
        assert_eq!(stages.huts[&(id.clone(), second)].state, Default::default());
        assert!(stages.insert_starts(first, BTreeMap::new()).is_err());
        let third = ChunkPos { x: 2, z: 0 };
        let mut bad = make_hut(third);
        bad.bounds.max[0] += 1;
        assert!(
            stages
                .insert_starts(third, BTreeMap::from([(id.clone(), bad)]))
                .is_err()
        );
        assert!(!stages.starts.contains_key(&third));
        assert!(!stages.huts.contains_key(&(id, third)));
        assert_eq!(stages.huts[&first_key], original);
    }

    #[test]
    fn decorated_hut_starts_and_states_commit_together() {
        use native_core::beardifier::BoundingBox;
        use native_resources::{
            structure_piece::ScatteredGroundHeight,
            structure_start::{HorizontalDirection, SimpleStructureKind},
        };
        let pos = ChunkPos { x: 0, z: 0 };
        let id = ResourceId::parse("minecraft:swamp_hut").unwrap();
        let key = (id.clone(), pos);
        let start = SimpleStructureStart {
            kind: SimpleStructureKind::SwampHut,
            chunk: [0, 0],
            generation_point: [0, 64, 0],
            orientation: Some(HorizontalDirection::South),
            bounds: BoundingBox::new([3, 64, 3], [9, 70, 11]).unwrap(),
            terrain_adjustment: TerrainAdjustment::None,
        };
        let mut stages = PreparedStructureStages::default();
        stages
            .insert_starts(pos, BTreeMap::from([(id.clone(), start.clone())]))
            .unwrap();
        let mut moved = start.clone();
        moved.bounds = BoundingBox::new([3, 70, 3], [9, 76, 11]).unwrap();
        let mut state = stages.huts[&key].state;
        state.ground = ScatteredGroundHeight::from_saved(70);
        state.spawned_witch = true;
        let saved = super::super::SavedSwampHutPiece::capture(&moved, state).unwrap();
        let updates = BTreeMap::from([(key.clone(), saved)]);
        let missing = (ResourceId::parse("minecraft:missing").unwrap(), pos);
        assert!(
            stages
                .commit_decorated_starts(
                    vec![vec![key.clone(), missing]],
                    vec![vec![moved.clone(), start.clone()]],
                    Some(&updates)
                )
                .is_err()
        );
        assert_eq!(stages.starts[&pos][&id].bounds, start.bounds);
        assert_eq!(stages.huts[&key].state, Default::default());
        assert!(
            stages
                .commit_decorated_starts(vec![vec![key.clone()]], vec![vec![moved.clone()]], None)
                .is_err()
        );
        stages
            .commit_decorated_starts(
                vec![vec![key.clone()]],
                vec![vec![moved.clone()]],
                Some(&updates),
            )
            .unwrap();
        assert_eq!(stages.starts[&pos][&id].bounds, moved.bounds);
        assert_eq!(stages.huts[&key], saved);
    }

    #[test]
    fn preparation_plan_covers_spawn_light_neighborhood_and_rejects_overflow() {
        let targets = (-1..=1).flat_map(|z| {
            (-1..=1).map(move |x| (ChunkPos { x, z }, GenerationStage::InitializeLight))
        });
        let plan = StructurePreparationPlan::new(targets).unwrap();
        assert_eq!(plan.starts().count(), 23 * 23);
        assert_eq!(plan.references().count(), 7 * 7);
        assert_eq!(plan.terrain().count(), 5 * 5);
        assert!(plan.starts().any(|pos| pos == ChunkPos { x: -11, z: 11 }));
        assert!(
            !plan
                .references()
                .any(|pos| pos.x.abs() > 3 || pos.z.abs() > 3)
        );
        assert!(StructurePreparationPlan::new([]).is_err());
        assert!(
            StructurePreparationPlan::new([(
                ChunkPos { x: i32::MAX, z: 0 },
                GenerationStage::Features
            )])
            .is_err()
        );
    }

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
