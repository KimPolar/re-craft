//! Java 26.3 chunk-generation dependencies and a deterministic work plan.
//! Completing a plan entry only records an already successful generation step.
use anyhow::{Context, Result, ensure};
use native_world::loaded::ChunkPos;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum GenerationStage {
    Empty,
    StructureStarts,
    StructureReferences,
    Biomes,
    Terrain,
    Features,
    InitializeLight,
    Light,
    Spawn,
    Full,
}
use GenerationStage::*;

pub(super) enum StructureFeatureLighting<'a> {
    Visible(&'a native_world::generation_light::VisibleLightSnapshot),
    Fresh(
        &'a mut super::FreshGenerationLightTracker,
        &'a native_resources::block_registry::GeneratedBlockRegistry,
    ),
    Staged(
        &'a mut super::StagedGenerationLightEngine,
        &'a native_resources::block_registry::GeneratedBlockRegistry,
    ),
}

pub(super) trait OceanFeatureRunner {
    fn execute(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures: &[super::structure_batch::PlannedMixedStructureDecoration<'_>],
        region_random: &native_resources::worldgen_region_random::RegionRandom,
        lighting: StructureFeatureLighting<'_>,
    ) -> Result<Vec<Vec<super::PreparedStructureStart>>>;
}

pub(super) struct OceanFeatureCallbacks<'a, 'b, C, W> {
    pub ocean: &'a mut super::OceanRuinPlacementContext<'b, C, W>,
}

impl<C, W> OceanFeatureRunner for OceanFeatureCallbacks<'_, '_, C, W>
where
    C: Clone + FnMut() -> Result<Option<super::FreshCreatureRandom>>,
    W: super::ocean_ruin::DrownedMarkerContext,
{
    fn execute(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures: &[super::structure_batch::PlannedMixedStructureDecoration<'_>],
        region_random: &native_resources::worldgen_region_random::RegionRandom,
        lighting: StructureFeatureLighting<'_>,
    ) -> Result<Vec<Vec<super::PreparedStructureStart>>> {
        let mut staged = self.ocean.clone();
        staged.region_random = region_random.clone();
        let (_, updated) =
            match lighting {
                StructureFeatureLighting::Visible(light) => prepared
                    .execute_mixed_with_visible_light(terrain, structures, &mut staged, light)?,
                StructureFeatureLighting::Fresh(tracker, registry) => prepared
                    .execute_mixed_with_tracked_light(
                        terrain,
                        structures,
                        &mut staged,
                        tracker,
                        registry,
                    )?,
                StructureFeatureLighting::Staged(engine, registry) => prepared
                    .execute_mixed_with_staged_light(
                        terrain,
                        structures,
                        &mut staged,
                        engine,
                        registry,
                    )?,
            };
        *self.ocean = staged;
        Ok(updated)
    }
}

pub(super) trait HutFeatureRunner {
    fn execute(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures: &[super::structure_batch::PlannedStructureDecoration<'_>],
        states: &mut BTreeMap<super::structure_batch::TerrainStartKey, super::SavedSwampHutPiece>,
        random: &mut native_resources::worldgen_region_random::RegionRandom,
        lighting: StructureFeatureLighting<'_>,
        all_structures: &super::structure_batch::PreparedStructureStages,
        biomes: &super::BiomeNeighborhood,
    ) -> Result<Vec<Vec<native_resources::structure_start::SimpleStructureStart>>>;
}

pub(super) struct HutFeatureCallbacks<'a, C, P> {
    pub variants: &'a native_resources::cat_variants::CatSpawnVariants,
    pub construct: &'a mut C,
    pub condition: &'a mut P,
}

impl<C, P> HutFeatureRunner for HutFeatureCallbacks<'_, C, P>
where
    C: Clone
        + FnMut(
            native_resources::structure_piece::swamp_hut::HutMob,
        ) -> Result<super::FreshCreatureRandom>,
    P: Clone
        + FnMut(
            &crate::worldgen::structure_batch::TerrainStartKey,
            [i32; 3],
            &native_world::loaded::World,
            &serde_json::Value,
            native_core::beardifier::BoundingBox,
            &[crate::worldgen::structure_batch::StructurePieceRevision],
        ) -> Result<bool>,
{
    fn execute(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures: &[super::structure_batch::PlannedStructureDecoration<'_>],
        states: &mut BTreeMap<super::structure_batch::TerrainStartKey, super::SavedSwampHutPiece>,
        random: &mut native_resources::worldgen_region_random::RegionRandom,
        lighting: StructureFeatureLighting<'_>,
        _all_structures: &super::structure_batch::PreparedStructureStages,
        _biomes: &super::BiomeNeighborhood,
    ) -> Result<Vec<Vec<native_resources::structure_start::SimpleStructureStart>>> {
        let (_, updated) = match lighting {
            StructureFeatureLighting::Visible(light) => prepared.execute_huts_with_visible_light(
                terrain,
                structures,
                states,
                random,
                self.variants,
                self.construct,
                self.condition,
                light,
            )?,
            StructureFeatureLighting::Fresh(tracker, registry) => prepared
                .execute_huts_with_tracked_light(
                    terrain,
                    structures,
                    states,
                    random,
                    self.variants,
                    self.construct,
                    self.condition,
                    tracker,
                    registry,
                )?,
            StructureFeatureLighting::Staged(engine, registry) => prepared
                .execute_huts_with_staged_light(
                    terrain,
                    structures,
                    states,
                    random,
                    self.variants,
                    self.construct,
                    self.condition,
                    engine,
                    registry,
                )?,
        };
        Ok(updated)
    }
}

/// Evaluate cat conditions against the currently prepared generation state.
pub(super) struct NativeHutFeatureRunner<'a> {
    pub variants: &'a native_resources::cat_variants::CatSpawnVariants,
    pub conditions: &'a native_resources::cat_variants::CatWorldConditions,
    pub moon_phase_at: &'a dyn Fn([i32; 3], &super::BiomeNeighborhood) -> Result<u8>,
}

impl HutFeatureRunner for NativeHutFeatureRunner<'_> {
    fn execute(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures: &[super::structure_batch::PlannedStructureDecoration<'_>],
        states: &mut BTreeMap<super::structure_batch::TerrainStartKey, super::SavedSwampHutPiece>,
        random: &mut native_resources::worldgen_region_random::RegionRandom,
        lighting: StructureFeatureLighting<'_>,
        all_structures: &super::structure_batch::PreparedStructureStages,
        biomes: &super::BiomeNeighborhood,
    ) -> Result<Vec<Vec<native_resources::structure_start::SimpleStructureStart>>> {
        use native_resources::cat_variants::CatSpawnContext;
        use native_resources::structure_piece::swamp_hut::HutMob;
        let conditions = self.conditions;
        let moon_phase_at = self.moon_phase_at;
        let mut construct = |mob| {
            let id = match mob {
                HutMob::Witch => "minecraft:witch",
                HutMob::Cat => "minecraft:cat",
            };
            super::FreshCreatureRandom::from_entropy(&native_resources::packs::ResourceId::parse(
                id,
            )?)
        };
        let mut condition = |key: &super::structure_batch::TerrainStartKey,
                             pos: [i32; 3],
                             _: &native_world::loaded::World,
                             value: &serde_json::Value,
                             bounds: native_core::beardifier::BoundingBox,
                             revisions: &[super::structure_batch::StructurePieceRevision]|
         -> Result<bool> {
            let kind = native_resources::packs::ResourceId::parse(
                value
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .context("cat condition type missing")?,
            )?;
            let mut matching = BTreeSet::new();
            let is_biome = kind.namespace == "minecraft" && kind.path == "biome";
            let is_structure = kind.namespace == "minecraft" && kind.path == "structure";
            let is_moon = kind.namespace == "minecraft" && kind.path == "moon_brightness";
            let biome = if is_biome {
                Some(biomes.biome_at_block(pos[0], pos[1], pos[2])?)
            } else {
                None
            };
            if is_structure {
                let chunk = ChunkPos {
                    x: pos[0].div_euclid(16),
                    z: pos[2].div_euclid(16),
                };
                let references = all_structures
                    .references
                    .get(&chunk)
                    .context("cat spawn chunk structure references unavailable")?;
                for (id, sources) in references.membership() {
                    for &packed in sources {
                        let bits = packed as u64;
                        let source = ChunkPos {
                            x: bits as u32 as i32,
                            z: (bits >> 32) as u32 as i32,
                        };
                        let candidate = (id.clone(), source);
                        let start = all_structures
                            .starts
                            .get(&source)
                            .and_then(|starts| starts.get(id))
                            .context("cat spawn referenced structure is missing")?;
                        ensure!(
                            start.chunk == [source.x, source.z],
                            "cat spawn structure source differs"
                        );
                        let piece = if &candidate == key {
                            bounds
                        } else {
                            revisions
                                .iter()
                                .find(|(revision, _)| revision == &candidate)
                                .map_or(start.bounds, |(_, bounds)| *bounds)
                        };
                        if (0..3)
                            .all(|axis| (piece.min[axis]..=piece.max[axis]).contains(&pos[axis]))
                        {
                            matching.insert(id.clone());
                        }
                    }
                }
            }
            let moon_phase = if is_moon {
                let phase = moon_phase_at(pos, biomes)?;
                ensure!(phase < 8, "invalid moon phase at cat spawn");
                Some(phase)
            } else {
                None
            };
            conditions.matches(
                value,
                &CatSpawnContext {
                    biome,
                    moon_phase,
                    structures_with_piece: is_structure.then_some(&matching),
                },
            )
        };
        HutFeatureCallbacks {
            variants: self.variants,
            construct: &mut construct,
            condition: &mut condition,
        }
        .execute(
            terrain,
            prepared,
            structures,
            states,
            random,
            lighting,
            all_structures,
            biomes,
        )
    }
}
impl GenerationStage {
    pub const ALL: [Self; 10] = [
        Empty,
        StructureStarts,
        StructureReferences,
        Biomes,
        Terrain,
        Features,
        InitializeLight,
        Light,
        Spawn,
        Full,
    ];
    pub fn id(self) -> &'static str {
        match self {
            Empty => "minecraft:empty",
            StructureStarts => "minecraft:structure_starts",
            StructureReferences => "minecraft:structure_references",
            Biomes => "minecraft:biomes",
            Terrain => "minecraft:terrain",
            Features => "minecraft:features",
            InitializeLight => "minecraft:initialize_light",
            Light => "minecraft:light",
            Spawn => "minecraft:spawn",
            Full => "minecraft:full",
        }
    }
    pub fn next(self) -> Option<Self> {
        Self::ALL.get(self as usize + 1).copied()
    }
    pub fn previous(self) -> Option<Self> {
        (self as usize)
            .checked_sub(1)
            .and_then(|n| Self::ALL.get(n))
            .copied()
    }
    /// Dependency at each Chebyshev distance, beginning with the center.
    pub fn direct(self) -> &'static [Self] {
        match self {
            Empty => &[],
            StructureStarts => &[Empty],
            StructureReferences => &[StructureStarts; 9],
            Biomes => &[
                StructureReferences,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
            Terrain => &[
                Biomes,
                Biomes,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
            Features => &[
                Terrain,
                Terrain,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
            InitializeLight => &[Features],
            Light => &[InitializeLight, InitializeLight],
            Spawn => &[Light, Biomes],
            Full => &[Spawn],
        }
    }
    pub fn accumulated(self) -> &'static [Self] {
        match self {
            Empty => &[],
            StructureStarts => &[Empty],
            StructureReferences => &[StructureStarts; 9],
            Biomes => &[
                StructureReferences,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
            Terrain => &[
                Biomes,
                Biomes,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
            Features => &[
                Terrain,
                Terrain,
                Biomes,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
            InitializeLight => &[
                Features,
                Terrain,
                Biomes,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
            Light => &[
                InitializeLight,
                InitializeLight,
                Terrain,
                Biomes,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
            Spawn => &[
                Light,
                InitializeLight,
                Terrain,
                Biomes,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
            Full => &[
                Spawn,
                InitializeLight,
                Terrain,
                Biomes,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
        }
    }
    pub fn block_write_radius(self) -> Option<i32> {
        match self {
            Terrain => Some(0),
            Features => Some(1),
            _ => None,
        }
    }
}

/// Single-threaded plan. A caller may record a step only after performing the
/// actual operation; this planner does not generate or save chunk contents.
#[derive(Default)]
pub struct GenerationSchedule {
    required: BTreeMap<ChunkPos, GenerationStage>,
    completed: BTreeMap<ChunkPos, GenerationStage>,
    ready: BTreeSet<ChunkPos>,
    revision: u64,
}

impl GenerationSchedule {
    pub fn new() -> Self {
        Self::default()
    }

    /// Expand Java's accumulated dependencies transactionally. Requests can overlap or
    /// increase existing targets without discarding completed work.
    pub fn request(&mut self, pos: ChunkPos, target: GenerationStage) -> Result<()> {
        self.request_many([(pos, target)])
    }

    /// Expand overlapping requests in one transaction. If any coordinate
    /// overflows, none of the requested stages are recorded.
    pub fn request_many(
        &mut self,
        requests: impl IntoIterator<Item = (ChunkPos, GenerationStage)>,
    ) -> Result<()> {
        let mut staged = BTreeMap::new();
        for (pos, target) in requests {
            staged
                .entry(pos)
                .and_modify(|stage: &mut GenerationStage| *stage = (*stage).max(target))
                .or_insert(target);
            for (radius, &needed) in target.accumulated().iter().enumerate().skip(1) {
                let r = i32::try_from(radius)?;
                for dz in -r..=r {
                    for dx in -r..=r {
                        if dx.abs().max(dz.abs()) != r {
                            continue;
                        }
                        let neighbor = ChunkPos {
                            x: pos
                                .x
                                .checked_add(dx)
                                .context("generation chunk X overflow")?,
                            z: pos
                                .z
                                .checked_add(dz)
                                .context("generation chunk Z overflow")?,
                        };
                        staged
                            .entry(neighbor)
                            .and_modify(|stage| *stage = (*stage).max(needed))
                            .or_insert(needed);
                    }
                }
            }
        }
        if staged.is_empty() {
            return Ok(());
        }
        let mut changed = Vec::new();
        for (pos, needed) in staged {
            if self.required.get(&pos).is_none_or(|&old| needed > old) {
                changed.push(pos);
            }
            self.required
                .entry(pos)
                .and_modify(|current| *current = (*current).max(needed))
                .or_insert(needed);
        }
        // Neighbor requirements are always at least StructureStarts; adding
        // an Empty entry therefore cannot make a neighbor ready by itself.
        for pos in changed {
            self.refresh_ready(pos);
        }
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }
    pub(super) fn revision(&self) -> u64 {
        self.revision
    }
    pub(super) fn requirements(&self) -> impl Iterator<Item = (ChunkPos, GenerationStage)> + '_ {
        self.required.iter().map(|(&pos, &stage)| (pos, stage))
    }
    pub fn required(&self, pos: ChunkPos) -> Option<GenerationStage> {
        self.required.get(&pos).copied()
    }
    pub fn completed(&self, pos: ChunkPos) -> Option<GenerationStage> {
        self.required
            .contains_key(&pos)
            .then(|| self.completed.get(&pos).copied().unwrap_or(Empty))
    }
    pub fn requested_chunks(&self) -> usize {
        self.required.len()
    }
    pub fn is_complete(&self) -> bool {
        self.required
            .iter()
            .all(|(pos, required)| self.completed.get(pos).copied().unwrap_or(Empty) >= *required)
    }

    pub(super) fn has_pending_stage(&self, stage: GenerationStage) -> bool {
        self.required.iter().any(|(pos, &required)| {
            required >= stage && self.completed.get(pos).copied().unwrap_or(Empty) < stage
        })
    }

    pub fn ready(&self, pos: ChunkPos, target: GenerationStage) -> bool {
        let Some(current) = self.completed(pos) else {
            return false;
        };
        if current.next() != Some(target)
            || self.required(pos).is_none_or(|required| target > required)
        {
            return false;
        }
        for (radius, &needed) in target.direct().iter().enumerate() {
            let r = radius as i32;
            for dz in -r..=r {
                for dx in -r..=r {
                    if dx.abs().max(dz.abs()) != r {
                        continue;
                    }
                    let (Some(x), Some(z)) = (pos.x.checked_add(dx), pos.z.checked_add(dz)) else {
                        return false;
                    };
                    if self
                        .completed(ChunkPos { x, z })
                        .is_none_or(|actual| actual < needed)
                    {
                        return false;
                    }
                }
            }
        }
        true
    }
    pub fn next_ready(&self) -> Option<(ChunkPos, GenerationStage)> {
        let &pos = self.ready.first()?;
        Some((pos, self.completed(pos)?.next()?))
    }

    fn refresh_ready(&mut self, pos: ChunkPos) {
        let next = self.completed(pos).and_then(GenerationStage::next);
        if next.is_some_and(|stage| self.ready(pos, stage)) {
            self.ready.insert(pos);
        } else {
            self.ready.remove(&pos);
        }
    }
    /// Call only after the corresponding operation succeeded. A failed or
    /// cancelled operation must not be recorded as complete.
    pub fn complete(&mut self, pos: ChunkPos, target: GenerationStage) -> Result<()> {
        ensure!(self.ready(pos, target), "generation step is not ready");
        self.completed.insert(pos, target);
        self.revision = self.revision.wrapping_add(1);
        self.refresh_ready(pos);
        // Only a next step depending on the stage just completed can have
        // become ready. Preserve lexicographic order in the ready set.
        let radius = GenerationStage::ALL
            .iter()
            .filter_map(|stage| stage.direct().iter().rposition(|&needed| needed == target))
            .max()
            .unwrap_or(0) as i32;
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                if dx == 0 && dz == 0 {
                    continue;
                }
                let (Some(x), Some(z)) = (pos.x.checked_add(dx), pos.z.checked_add(dz)) else {
                    continue;
                };
                let neighbor = ChunkPos { x, z };
                let Some(next) = self.completed(neighbor).and_then(GenerationStage::next) else {
                    continue;
                };
                let distance = dx.abs().max(dz.abs()) as usize;
                if next.direct().get(distance) == Some(&target) {
                    self.refresh_ready(neighbor);
                }
            }
        }
        Ok(())
    }

    /// Generate and store a biome volume before marking BIOMES complete. The
    /// generator and neighborhood must use the same active packs and seed.
    /// On a generation error the caller must discard the generator/evaluator
    /// before retrying, because their search history may have advanced.
    pub fn run_biomes(
        &mut self,
        generator: &mut super::TerrainGenerator,
        evaluator: &mut native_resources::density::DensityEvaluator,
        neighborhood: &mut super::BiomeNeighborhood,
        pos: ChunkPos,
    ) -> Result<()> {
        ensure!(self.ready(pos, Biomes), "biome dependencies are incomplete");
        ensure!(
            neighborhood.seed == generator.seed
                && neighborhood.min_y == generator.min_y
                && neighborhood.height == generator.height,
            "biome neighborhood differs from generator"
        );
        ensure!(
            neighborhood.chunk([pos.x, pos.z]).is_none(),
            "biome chunk already exists"
        );
        let chunk = generator.generate_biomes([pos.x, pos.z], evaluator)?;
        neighborhood.insert(std::sync::Arc::new(chunk))?;
        self.complete(pos, Biomes)
    }

    /// Generate and assemble an intermediate terrain chunk with structures
    /// explicitly disabled. It is neither a full playable chunk nor safe to
    /// save. All assembled chunks must use the same generator and pack stack;
    /// matching seed, dimension and height cannot prove that. Discard mutable
    /// generator caches after an error before retrying to preserve their history.
    #[allow(clippy::too_many_arguments)]
    pub fn run_terrain<P: native_resources::packs::ResourceProvider>(
        &mut self,
        generator: &mut super::TerrainGenerator,
        provider: &mut P,
        evaluator: &mut native_resources::density::DensityEvaluator,
        cache: &mut native_resources::carver::schedule::CarverCache,
        neighborhood: &super::BiomeNeighborhood,
        tags: &native_resources::carver::tags::CarverBlockTags,
        assembled: &mut Option<super::CarvedTerrainWorld>,
        pos: ChunkPos,
        dimension: &str,
        structures_enabled: bool,
        classify: impl FnMut(&serde_json::Value) -> Result<super::CarverBlockState>,
        resolve: impl FnMut(&serde_json::Value) -> Result<native_world::loaded::BlockState>,
    ) -> Result<()> {
        ensure!(
            self.ready(pos, Terrain),
            "terrain dependencies are incomplete"
        );
        ensure!(
            !structures_enabled && !generator.structure_context,
            "structure terrain generation is not implemented by this path"
        );
        self.validate_terrain_inputs(generator, neighborhood, assembled.as_ref(), pos, dimension)?;
        let biome = neighborhood
            .chunk([pos.x, pos.z])
            .context("terrain biome chunk missing")?;
        let chunk = generator.generate_carved_surface(
            provider,
            evaluator,
            cache,
            [pos.x, pos.z],
            neighborhood,
            tags,
            classify,
        )?;
        let terrain = super::materialize_carved_terrain(chunk, biome, dimension, resolve)?;
        match assembled {
            Some(world) => world.insert(&terrain)?,
            None => *assembled = Some(super::CarvedTerrainWorld::new(terrain)),
        }
        self.complete(pos, Terrain)
    }

    /// Generate an intermediate terrain chunk using complete, precomputed
    /// structure references. The caller supplies Java's structure iteration
    /// order and matching generator, biome, block registry and pack inputs.
    /// This does not place structures or authorize a world save.
    #[allow(clippy::too_many_arguments)]
    pub fn run_terrain_with_structures<P: native_resources::packs::ResourceProvider>(
        &mut self,
        structures: &mut super::structure_batch::StructureGenerationBatch,
        provider: &mut P,
        cache: &mut native_resources::carver::schedule::CarverCache,
        neighborhood: &super::BiomeNeighborhood,
        tags: &native_resources::carver::tags::CarverBlockTags,
        assembled: &mut Option<super::CarvedTerrainWorld>,
        pos: ChunkPos,
        dimension: &str,
        order: &[super::structure_batch::TerrainStartKey],
        classify: impl FnMut(&serde_json::Value) -> Result<super::CarverBlockState>,
        resolve: impl FnMut(&serde_json::Value) -> Result<native_world::loaded::BlockState>,
    ) -> Result<()> {
        ensure!(
            self.ready(pos, Terrain),
            "terrain dependencies are incomplete"
        );
        self.validate_terrain_inputs(
            structures.generator(),
            neighborhood,
            assembled.as_ref(),
            pos,
            dimension,
        )?;
        let biome = neighborhood
            .chunk([pos.x, pos.z])
            .context("terrain biome chunk missing")?;
        let result = (|| {
            let chunk = structures.generate_carved_surface(
                provider,
                pos,
                neighborhood,
                tags,
                cache,
                order,
                classify,
            )?;
            let terrain = super::materialize_carved_terrain(chunk, biome, dimension, resolve)?;
            match assembled {
                Some(world) => world.insert(&terrain)?,
                None => *assembled = Some(super::CarvedTerrainWorld::new(terrain)),
            }
            self.complete(pos, Terrain)
        })();
        if result.is_err() {
            structures.invalidate();
        }
        result
    }

    /// Run the terrain stage with a context frozen from verified starts and
    /// references. This remains an intermediate, unsaveable terrain chunk.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn run_terrain_with_prepared_structure<
        P: native_resources::packs::ResourceProvider,
    >(
        &mut self,
        generator: &mut super::TerrainGenerator,
        provider: &mut P,
        evaluator: &mut native_resources::density::DensityEvaluator,
        cache: &mut native_resources::carver::schedule::CarverCache,
        neighborhood: &super::BiomeNeighborhood,
        tags: &native_resources::carver::tags::CarverBlockTags,
        assembled: &mut Option<super::CarvedTerrainWorld>,
        pos: ChunkPos,
        dimension: &str,
        structure: std::sync::Arc<native_core::beardifier::Beardifier>,
        classify: impl FnMut(&serde_json::Value) -> Result<super::CarverBlockState>,
        resolve: impl FnMut(&serde_json::Value) -> Result<native_world::loaded::BlockState>,
    ) -> Result<()> {
        ensure!(
            self.ready(pos, Terrain),
            "terrain dependencies are incomplete"
        );
        ensure!(
            generator.structure_context,
            "generator lacks structure context"
        );
        self.validate_terrain_inputs(generator, neighborhood, assembled.as_ref(), pos, dimension)?;
        let biome = neighborhood
            .chunk([pos.x, pos.z])
            .context("terrain biome chunk missing")?;
        let chunk = generator.generate_carved_surface_with_structure_context(
            provider,
            evaluator,
            cache,
            [pos.x, pos.z],
            neighborhood,
            tags,
            structure,
            classify,
        )?;
        let terrain = super::materialize_carved_terrain(chunk, biome, dimension, resolve)?;
        match assembled {
            Some(world) => world.insert(&terrain)?,
            None => *assembled = Some(super::CarvedTerrainWorld::new(terrain)),
        }
        self.complete(pos, Terrain)
    }

    fn validate_terrain_inputs(
        &self,
        generator: &super::TerrainGenerator,
        neighborhood: &super::BiomeNeighborhood,
        assembled: Option<&super::CarvedTerrainWorld>,
        pos: ChunkPos,
        dimension: &str,
    ) -> Result<()> {
        let dimension_id = native_resources::packs::ResourceId::parse(dimension)?;
        if let Some(world) = assembled {
            ensure!(
                world.world.dimension
                    == format!("{}:{}", dimension_id.namespace, dimension_id.path),
                "terrain dimension differs"
            );
            ensure!(
                world.seed == generator.seed
                    && world.min_y == generator.min_y
                    && world.height == generator.height,
                "terrain generator differs from assembled world"
            );
            ensure!(
                !world.postprocessing.contains_key(&pos) && !world.world.chunks.contains_key(&pos),
                "terrain chunk already exists"
            );
        }
        neighborhood.validate(
            [pos.x, pos.z],
            generator.seed,
            generator.min_y,
            generator.height,
        )
    }

    /// Apply supported decoration and record FEATURES only after it succeeds.
    pub fn run_features(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures_enabled: bool,
        fresh_light: Option<(
            &mut super::FreshGenerationLightTracker,
            &native_resources::block_registry::GeneratedBlockRegistry,
        )>,
    ) -> Result<Vec<bool>> {
        let center = prepared.center_chunk();
        ensure!(
            self.ready(center, Features),
            "features dependencies are incomplete"
        );
        let result = match fresh_light {
            Some((tracker, registry)) => prepared.execute_with_tracked_light(
                terrain,
                structures_enabled,
                tracker,
                registry,
            )?,
            None => prepared.execute(terrain, structures_enabled)?,
        };
        self.complete(center, Features)?;
        Ok(result)
    }

    /// Execute FEATURES using an already published staged-light revision.
    pub fn run_features_with_staged_light(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures_enabled: bool,
        engine: &mut super::StagedGenerationLightEngine,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
    ) -> Result<Vec<bool>> {
        let center = prepared.center_chunk();
        ensure!(
            self.ready(center, Features),
            "features dependencies are incomplete"
        );
        let result =
            prepared.execute_with_staged_light(terrain, structures_enabled, engine, registry)?;
        self.complete(center, Features)?;
        Ok(result)
    }

    /// Publish FEATURES only after the complete structure and feature pass succeeds.
    pub(super) fn run_features_with_structures(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures: &[super::structure_batch::PlannedStructureDecoration<'_>],
        lighting: StructureFeatureLighting<'_>,
    ) -> Result<Vec<Vec<native_resources::structure_start::SimpleStructureStart>>> {
        let center = prepared.center_chunk();
        ensure!(
            self.ready(center, Features),
            "features dependencies are incomplete"
        );
        let (_, updated) = match lighting {
            StructureFeatureLighting::Visible(light) => {
                prepared.execute_structures_with_visible_light(terrain, structures, light)?
            }
            StructureFeatureLighting::Fresh(tracker, registry) => prepared
                .execute_structures_with_tracked_light(terrain, structures, tracker, registry)?,
            StructureFeatureLighting::Staged(engine, registry) => prepared
                .execute_structures_with_staged_light(terrain, structures, engine, registry)?,
        };
        self.complete(center, Features)?;
        Ok(updated)
    }

    /// Publish FEATURES only after the mixed structure pass succeeds.
    pub(super) fn run_features_with_ocean(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures: &[super::structure_batch::PlannedMixedStructureDecoration<'_>],
        region_random: &native_resources::worldgen_region_random::RegionRandom,
        lighting: StructureFeatureLighting<'_>,
        runner: &mut dyn OceanFeatureRunner,
    ) -> Result<Vec<Vec<super::PreparedStructureStart>>> {
        let center = prepared.center_chunk();
        ensure!(
            self.ready(center, Features),
            "features dependencies are incomplete"
        );
        let updated = runner.execute(terrain, prepared, structures, region_random, lighting)?;
        self.complete(center, Features)?;
        Ok(updated)
    }

    /// Complete FEATURES only after the hut-aware pass succeeds.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn run_features_with_huts(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures: &[super::structure_batch::PlannedStructureDecoration<'_>],
        states: &mut BTreeMap<super::structure_batch::TerrainStartKey, super::SavedSwampHutPiece>,
        random: &mut native_resources::worldgen_region_random::RegionRandom,
        lighting: StructureFeatureLighting<'_>,
        runner: &mut dyn HutFeatureRunner,
        all_structures: &super::structure_batch::PreparedStructureStages,
        biomes: &super::BiomeNeighborhood,
    ) -> Result<Vec<Vec<native_resources::structure_start::SimpleStructureStart>>> {
        let center = prepared.center_chunk();
        ensure!(
            self.ready(center, Features),
            "features dependencies are incomplete"
        );
        let updated = runner.execute(
            terrain,
            prepared,
            structures,
            states,
            random,
            lighting,
            all_structures,
            biomes,
        )?;
        self.complete(center, Features)?;
        Ok(updated)
    }

    /// Apply features against a complete, immutable visible light revision.
    pub fn run_features_with_visible_light(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures_enabled: bool,
        light: &native_world::generation_light::VisibleLightSnapshot,
    ) -> Result<Vec<bool>> {
        let center = prepared.center_chunk();
        ensure!(
            self.ready(center, Features),
            "features dependencies are incomplete"
        );
        let result = prepared.execute_with_visible_light(terrain, structures_enabled, light)?;
        self.complete(center, Features)?;
        Ok(result)
    }

    /// Calculate direct-sky sources and register sections before advancing the
    /// fresh INITIALIZE_LIGHT stage. No light propagation is performed here.
    pub fn run_light_initialization(
        &mut self,
        terrain: &super::CarvedTerrainWorld,
        center: ChunkPos,
        tracker: &mut super::FreshGenerationLightTracker,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
    ) -> Result<()> {
        ensure!(
            self.ready(center, InitializeLight),
            "light initialization dependencies are incomplete"
        );
        tracker.initialize_chunk(terrain, center, registry)?;
        self.complete(center, InitializeLight)
    }

    /// Advance LIGHT within a frozen, fully initialized generation batch.
    /// The returned snapshot does not write a chunk or make it saveable.
    pub fn run_light(
        &mut self,
        terrain: &super::CarvedTerrainWorld,
        center: ChunkPos,
        engine: &mut super::StagedGenerationLightEngine,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
    ) -> Result<native_world::generation_light::VisibleLightSnapshot> {
        ensure!(
            self.ready(center, Light),
            "light dependencies are incomplete"
        );
        let snapshot = engine.activate(terrain, center, [center], registry)?;
        self.complete(center, Light)?;
        Ok(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_next(plan: &GenerationSchedule) -> Option<(ChunkPos, GenerationStage)> {
        plan.required.iter().find_map(|(&pos, &required)| {
            let next = plan.completed(pos)?.next()?;
            (next <= required && plan.ready(pos, next)).then_some((pos, next))
        })
    }

    #[test]
    fn ready_index_matches_full_scan_with_interleaved_requests() {
        // The index relies on new, uncompleted neighbors not satisfying a
        // direct dependency. Guard that property when stages are changed.
        for stage in GenerationStage::ALL {
            assert!(stage.direct().iter().skip(1).all(|&needed| needed > Empty));
        }
        let mut plan = GenerationSchedule::new();
        let requests = [
            (ChunkPos { x: 0, z: 0 }, Features),
            (ChunkPos { x: -2, z: 1 }, Terrain),
            (ChunkPos { x: 0, z: 0 }, InitializeLight),
            (ChunkPos { x: 3, z: -2 }, Light),
            (ChunkPos { x: -2, z: 1 }, Full),
            (
                ChunkPos {
                    x: i32::MAX,
                    z: i32::MIN,
                },
                StructureStarts,
            ),
        ];
        for step in 0..1800 {
            if let Some(&(pos, target)) = requests.get(step / 80).filter(|_| step % 80 == 0) {
                plan.request(pos, target).unwrap();
            }
            let expected = scan_next(&plan);
            assert_eq!(plan.next_ready(), expected, "step {step}");
            if step % 31 == 0 {
                let all: BTreeSet<_> = plan
                    .required
                    .keys()
                    .copied()
                    .filter(|&pos| {
                        plan.completed(pos)
                            .and_then(GenerationStage::next)
                            .is_some_and(|stage| plan.ready(pos, stage))
                    })
                    .collect();
                assert_eq!(plan.ready, all, "ready set at step {step}");
            }
            let Some((mut pos, mut stage)) = expected else {
                break;
            };
            // Callers may complete any ready step, not just next_ready().
            if step % 7 == 0 {
                pos = *plan.ready.last().unwrap();
                stage = plan.completed(pos).unwrap().next().unwrap();
            }
            plan.complete(pos, stage).unwrap();
        }
        assert_eq!(
            plan.required(ChunkPos {
                x: i32::MAX,
                z: i32::MIN,
            }),
            Some(StructureStarts)
        );
        while let Some((pos, stage)) = scan_next(&plan) {
            assert_eq!(plan.next_ready(), Some((pos, stage)));
            plan.complete(pos, stage).unwrap();
        }
        assert_eq!(plan.next_ready(), None);
        assert!(plan.is_complete());
    }
    #[test]
    fn requirements_match_accumulated_radii_for_every_stage() {
        let center = ChunkPos { x: -3, z: 4 };
        for target in GenerationStage::ALL {
            let mut schedule = GenerationSchedule::new();
            schedule.request(center, target).unwrap();
            for dz in -14i32..=14 {
                for dx in -14i32..=14 {
                    let d = dx.abs().max(dz.abs()) as usize;
                    let expected = if d == 0 {
                        Some(target)
                    } else {
                        target.accumulated().get(d).copied()
                    };
                    assert_eq!(
                        schedule.required(ChunkPos {
                            x: center.x + dx,
                            z: center.z + dz
                        }),
                        expected,
                        "{:?} {dx},{dz}",
                        target
                    );
                }
            }
        }
    }
    #[test]
    fn overlapping_requests_are_batched_without_partial_updates() {
        let requests = [
            (ChunkPos { x: -2, z: 1 }, Features),
            (ChunkPos { x: 0, z: 0 }, InitializeLight),
            (ChunkPos { x: -2, z: 1 }, Terrain),
            (ChunkPos { x: 3, z: -1 }, StructureReferences),
        ];
        let mut sequential = GenerationSchedule::new();
        for (pos, stage) in requests {
            sequential.request(pos, stage).unwrap();
        }
        let mut combined = GenerationSchedule::new();
        combined.request_many(requests).unwrap();
        assert_eq!(combined.required, sequential.required);
        assert_eq!(combined.ready, sequential.ready);
        let before = combined.required.clone();
        let ready = combined.ready.clone();
        let revision = combined.revision;
        assert!(
            combined
                .request_many([
                    (ChunkPos { x: 100, z: 100 }, Features),
                    (ChunkPos { x: i32::MAX, z: 0 }, Features),
                ])
                .is_err()
        );
        assert_eq!(combined.required, before);
        assert_eq!(combined.ready, ready);
        assert_eq!(combined.revision, revision);
        combined.request_many([]).unwrap();
        assert_eq!(combined.revision, revision);
    }
    #[test]
    fn steps_require_neighbor_completion_and_requests_are_transactional() {
        let center = ChunkPos { x: 0, z: 0 };
        let mut plan = GenerationSchedule::new();
        plan.request(center, Features).unwrap();
        assert!(!plan.ready(center, Features));
        assert!(plan.complete(center, Features).is_err());
        let total = plan.requested_chunks();
        assert!(
            plan.request(ChunkPos { x: i32::MAX, z: 0 }, Features)
                .is_err()
        );
        assert_eq!(plan.requested_chunks(), total);
        let mut count = 0usize;
        while !plan.ready(center, Features) {
            let (pos, stage) = plan.next_ready().expect("dependency graph must progress");
            assert_ne!((pos, stage), (center, Features));
            plan.complete(pos, stage).unwrap();
            count += 1;
            assert!(count < 10_000);
        }
        plan.complete(center, Features).unwrap();
        assert_eq!(plan.completed(center), Some(Features));
        assert!(plan.complete(center, Features).is_err());
        plan.request(center, InitializeLight).unwrap();
        assert!(plan.ready(center, InitializeLight));
        plan.complete(center, InitializeLight).unwrap();
    }
}
