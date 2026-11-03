//! Incremental execution of fresh terrain generation without structures.
use anyhow::{Context, Result, bail, ensure};
use native_resources::{
    block_registry::GeneratedBlockRegistry,
    carver::{schedule::CarverCache, tags::CarverBlockTags},
    density::DensityEvaluator,
    dimension_lighting::GeneratedDimensionLighting,
    feature_order::FeatureSchedule,
    packs::{ResourceId, ResourceProvider},
};
use native_world::generation_light::VisibleLightSnapshot;
use native_world::loaded::{BlockState, ChunkPos, World};
use serde_json::Value;
use std::{collections::BTreeSet, sync::Arc};

use super::{
    BiomeNeighborhood, CarvedTerrainWorld, CarverBlockState, FreshGenerationLightTracker,
    PreparedDecoration, StagedGenerationLightEngine, TerrainGenerator,
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
    light_registry: Option<GeneratedBlockRegistry>,
    light_tracker: Option<FreshGenerationLightTracker>,
    light_engine: Option<StagedGenerationLightEngine>,
    lit_cache: Option<(BTreeSet<ChunkPos>, Arc<World>)>,
    failed: bool,
}

/// The light-engine history accompanying intermediate generated terrain.
pub enum FreshBatchLight {
    Initialized(FreshGenerationLightTracker),
    Staged(StagedGenerationLightEngine),
}

struct DecorationConfig {
    schedule: FeatureSchedule,
    block_registry: BTreeSet<String>,
    has_skylight: bool,
    dimension_type: Option<String>,
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
            light_registry: None,
            light_tracker: None,
            light_engine: None,
            lit_cache: None,
            failed: false,
        })
    }

    /// Configure FEATURES before requesting work. The lighting settings and
    /// registries must come from the same immutable pack stack as the generator.
    /// This batch starts with an empty light engine; no external light work may
    /// be interleaved with its registration history.
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
            dimension_type: lighting
                .dimension_type
                .as_ref()
                .map(|id| format!("{}:{}", id.namespace, id.path)),
        });
        Ok(())
    }

    /// Enable INITIALIZE_LIGHT before requesting work. The validated block
    /// registry must come from the same client and pack stack as decoration.
    pub fn enable_light_initialization(&mut self, registry: GeneratedBlockRegistry) -> Result<()> {
        ensure!(
            !self.failed && self.schedule.requested_chunks() == 0 && self.light_registry.is_none(),
            "light initialization must be configured before requests"
        );
        let config = self
            .decoration
            .as_ref()
            .context("decoration must be configured first")?;
        ensure!(
            registry.ids().all(|id| config.block_registry.contains(id)),
            "generated block registry differs from decoration registry"
        );
        self.light_registry = Some(registry);
        Ok(())
    }

    /// Add a terrain request; overlapping dependencies retain completed work.
    pub fn request(&mut self, pos: ChunkPos) -> Result<()> {
        ensure!(!self.failed, "terrain batch failed and must be discarded");
        self.check_writes(pos, 0)?;
        self.schedule.request(pos, GenerationStage::Terrain)
    }

    /// Request terrain and supported FEATURES for one chunk.
    pub fn request_features(&mut self, pos: ChunkPos) -> Result<()> {
        ensure!(
            !self.failed,
            "generation batch failed and must be discarded"
        );
        ensure!(self.decoration.is_some(), "decoration is not configured");
        // The center must remain writable; actual cross-chunk feature writes
        // are validated transactionally against the staged light engine.
        self.check_writes(pos, 0)?;
        self.schedule.request(pos, GenerationStage::Features)
    }

    fn check_writes(&self, pos: ChunkPos, radius: i64) -> Result<()> {
        if self.light_engine.is_some() {
            ensure!(
                !self
                    .schedule
                    .has_pending_stage(GenerationStage::InitializeLight)
                    && !self.schedule.has_pending_stage(GenerationStage::Light),
                "finish pending light registration and propagation before requesting terrain"
            );
        }
        self.check_write_area(pos, radius)
    }

    fn check_write_area(&self, pos: ChunkPos, radius: i64) -> Result<()> {
        ensure!(
            self.light_engine
                .as_ref()
                .is_none_or(|engine| engine.allows_writes(pos, radius)),
            "generation writes overlap registered or active light chunks"
        );
        Ok(())
    }

    /// Request FEATURES and light section/source registration for a chunk.
    /// This step does not propagate light or produce saveable lighting.
    pub fn request_light_initialization(&mut self, pos: ChunkPos) -> Result<()> {
        ensure!(
            !self.failed,
            "generation batch failed and must be discarded"
        );
        ensure!(
            self.light_registry.is_some(),
            "light initialization is not configured"
        );
        if self.light_engine.is_some() {
            ensure!(
                !self.schedule.has_pending_stage(GenerationStage::Terrain)
                    && !self.schedule.has_pending_stage(GenerationStage::Features)
                    && !self.schedule.has_pending_stage(GenerationStage::Light),
                "finish pending terrain, decoration and propagation before light registration"
            );
            ensure!(
                self.schedule.completed(pos) == Some(GenerationStage::Features),
                "sealed batches can only initialize already decorated chunks"
            );
        }
        self.schedule.request(pos, GenerationStage::InitializeLight)
    }

    /// Finish all requested registrations before entering a frozen light
    /// batch. Subsequent terrain and decoration writes must stay outside
    /// registered chunks and the neighborhoods used by active lighting.
    pub fn seal_for_light_propagation(&mut self) -> Result<()> {
        ensure!(
            self.is_complete(),
            "finish generation requests before sealing lighting"
        );
        ensure!(
            self.light_engine.is_none(),
            "light propagation is already sealed"
        );
        ensure!(self.light_registry.is_some(), "light registry is missing");
        let tracker = self
            .light_tracker
            .as_ref()
            .context("no initialized light chunks")?;
        ensure!(
            tracker
                .registered
                .keys()
                .any(|&pos| self.has_light_dependencies(pos)),
            "no chunk has complete light dependencies"
        );
        let tracker = self
            .light_tracker
            .take()
            .context("no initialized light chunks")?;
        self.light_engine = Some(StagedGenerationLightEngine::new(tracker));
        Ok(())
    }

    /// Request LIGHT only when its complete accumulated dependencies have
    /// already been generated and registered.
    pub fn request_light(&mut self, pos: ChunkPos) -> Result<()> {
        ensure!(
            !self.failed && self.light_engine.is_some(),
            "light propagation is not sealed"
        );
        ensure!(
            self.has_light_dependencies(pos),
            "chunk light dependencies are incomplete"
        );
        ensure!(
            !self.schedule.has_pending_stage(GenerationStage::Terrain)
                && !self.schedule.has_pending_stage(GenerationStage::Features)
                && !self
                    .schedule
                    .has_pending_stage(GenerationStage::InitializeLight),
            "finish pending terrain, decoration and registration before propagating light"
        );
        self.schedule.request(pos, GenerationStage::Light)
    }

    fn has_light_dependencies(&self, pos: ChunkPos) -> bool {
        if self.schedule.completed(pos) != Some(GenerationStage::InitializeLight) {
            return false;
        }
        for (radius, &required) in GenerationStage::Light.accumulated().iter().enumerate() {
            let Ok(r) = i32::try_from(radius) else {
                return false;
            };
            for dz in -r..=r {
                for dx in -r..=r {
                    if dx.abs().max(dz.abs()) != r {
                        continue;
                    }
                    let (Some(x), Some(z)) = (pos.x.checked_add(dx), pos.z.checked_add(dz)) else {
                        return false;
                    };
                    if !self
                        .schedule
                        .completed(ChunkPos { x, z })
                        .is_some_and(|stage| stage >= required)
                    {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Capture selected columns from the sealed, staged light engine. This
    /// does not write light arrays into generated chunks.
    pub fn light_snapshot(
        &self,
        covered: impl IntoIterator<Item = ChunkPos>,
    ) -> Result<VisibleLightSnapshot> {
        ensure!(
            !self.failed,
            "generation batch failed and must be discarded"
        );
        let engine = self
            .light_engine
            .as_ref()
            .context("light propagation is not sealed")?;
        let terrain = self.terrain.as_ref().context("light terrain is missing")?;
        let registry = self
            .light_registry
            .as_ref()
            .context("light registry is missing")?;
        engine.snapshot(terrain, covered, registry)
    }

    /// Build a read-only rendering view containing the selected light columns.
    /// Chunk status and light-correct flags remain intermediate; this view is
    /// not a saveable world. Unselected chunks retain their original data.
    pub fn lit_snapshot(
        &mut self,
        covered: impl IntoIterator<Item = ChunkPos>,
    ) -> Result<Arc<World>> {
        use native_world::loaded::Section;
        let covered: BTreeSet<_> = covered.into_iter().collect();
        ensure!(
            !self.failed,
            "generation batch failed and must be discarded"
        );
        if let Some((cached_columns, world)) = &self.lit_cache {
            if *cached_columns == covered {
                return Ok(Arc::clone(world));
            }
        }
        let terrain = self.terrain.as_ref().context("light terrain is missing")?;
        let max_y = self
            .generator
            .min_y
            .checked_add(self.generator.height)
            .context("light height overflow")?;
        native_world::block_light::LightBounds::new(self.generator.min_y, max_y)
            .context("unsupported light bounds")?;
        let first = self.generator.min_y.div_euclid(16);
        let end = max_y.div_euclid(16);
        for pos in &covered {
            let chunk = terrain
                .world
                .chunks
                .get(pos)
                .context("render light chunk is missing")?;
            ensure!(
                chunk.position == *pos && !chunk.block_light_correct,
                "render light chunk is invalid"
            );
            for sy in first..end {
                let section = chunk
                    .sections
                    .get(&i8::try_from(sy)?)
                    .context("render block section is missing")?;
                ensure!(section.blocks.is_some(), "render block data is missing");
            }
        }
        let light = self.light_snapshot(covered.iter().copied())?;
        if covered.is_empty() {
            return Ok(terrain.snapshot());
        }
        let mut world = (*terrain.world).clone();
        for &pos in &covered {
            let chunk = Arc::make_mut(world.chunks.get_mut(&pos).expect("chunk was validated"));
            for sy in first - 1..end + 1 {
                let key =
                    i8::try_from(sy).context("render light section outside supported range")?;
                let (block, sky) = light
                    .expanded_section(pos, sy)
                    .context("render light column not captured")?;
                let section = chunk.sections.entry(key).or_insert_with(|| {
                    Arc::new(Section {
                        blocks: None,
                        biomes: None,
                        block_light: None,
                        sky_light: None,
                    })
                });
                let section = Arc::make_mut(section);
                section.block_light = Some(block);
                section.sky_light = Some(sky);
            }
        }
        let world = Arc::new(world);
        self.lit_cache = Some((covered, Arc::clone(&world)));
        Ok(world)
    }

    /// Capture the 3x3 light and block neighborhood of each render target.
    /// Targets must have completed LIGHT; callers must draw only the targets,
    /// because farther chunks in the returned world are not light-covered.
    pub fn lit_render_snapshot(
        &mut self,
        targets: impl IntoIterator<Item = ChunkPos>,
    ) -> Result<Arc<World>> {
        let targets: BTreeSet<_> = targets.into_iter().collect();
        ensure!(!targets.is_empty(), "no generated render targets");
        let mut covered = BTreeSet::new();
        for pos in targets {
            ensure!(
                self.schedule
                    .completed(pos)
                    .is_some_and(|stage| stage >= GenerationStage::Light),
                "render target has not completed lighting"
            );
            for dz in -1i32..=1 {
                for dx in -1i32..=1 {
                    covered.insert(ChunkPos {
                        x: pos.x.checked_add(dx).context("render X overflow")?,
                        z: pos.z.checked_add(dz).context("render Z overflow")?,
                    });
                }
            }
        }
        self.lit_snapshot(covered)
    }

    pub fn is_complete(&self) -> bool {
        !self.failed && self.schedule.is_complete()
    }
    pub fn schedule(&self) -> &GenerationSchedule {
        &self.schedule
    }

    /// Build limits of the selected dimension, before the two light padding sections.
    pub fn build_bounds(&self) -> Result<(i32, i32)> {
        Ok((
            self.generator.min_y,
            self.generator
                .min_y
                .checked_add(self.generator.height)
                .context("generation height overflow")?,
        ))
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
            GenerationStage::Terrain => (|| {
                self.check_write_area(pos, 0)?;
                self.schedule.run_terrain(
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
                )?;
                if let (Some(config), Some(terrain)) = (&self.decoration, &mut self.terrain) {
                    if terrain.world.dimension_type != config.dimension_type {
                        Arc::make_mut(&mut terrain.world).dimension_type =
                            config.dimension_type.clone();
                    }
                }
                self.lit_cache = None;
                Ok(())
            })(),
            GenerationStage::Features => (|| {
                self.check_write_area(pos, 0)?;
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
                if let Some(engine) = &mut self.light_engine {
                    let registry = self
                        .light_registry
                        .as_ref()
                        .context("light registry is missing")?;
                    self.schedule.run_features_with_staged_light(
                        terrain, &prepared, false, engine, registry,
                    )?;
                } else if let Some(tracker) = &mut self.light_tracker {
                    let registry = self
                        .light_registry
                        .as_ref()
                        .context("light registry is missing")?;
                    self.schedule.run_features(
                        terrain,
                        &prepared,
                        false,
                        Some((tracker, registry)),
                    )?;
                } else {
                    let mut covered = Vec::with_capacity(9);
                    for dz in -1i32..=1 {
                        for dx in -1i32..=1 {
                            covered.push(ChunkPos {
                                x: pos.x.checked_add(dx).context("light X overflow")?,
                                z: pos.z.checked_add(dz).context("light Z overflow")?,
                            });
                        }
                    }
                    let light = native_world::generation_light::VisibleLightSnapshot::fresh_after_section_updates(true, config.has_skylight, [], covered).context("invalid initial light columns")?;
                    self.schedule
                        .run_features_with_visible_light(terrain, &prepared, false, &light)?;
                }
                self.lit_cache = None;
                Ok(())
            })(),
            GenerationStage::InitializeLight => (|| {
                let terrain = self.terrain.as_ref().context("light terrain is missing")?;
                let registry = self
                    .light_registry
                    .as_ref()
                    .context("light registry is missing")?;
                if let Some(engine) = &mut self.light_engine {
                    ensure!(
                        self.schedule.ready(pos, GenerationStage::InitializeLight),
                        "light initialization dependencies are incomplete"
                    );
                    engine.initialize_chunk(terrain, pos, registry)?;
                    self.schedule
                        .complete(pos, GenerationStage::InitializeLight)?;
                    self.lit_cache = None;
                    return Ok(());
                }
                if self.light_tracker.is_none() {
                    let config = self
                        .decoration
                        .as_ref()
                        .context("decoration is not configured")?;
                    self.light_tracker = Some(FreshGenerationLightTracker::new(
                        terrain,
                        config.has_skylight,
                    )?);
                }
                self.schedule.run_light_initialization(
                    terrain,
                    pos,
                    self.light_tracker.as_mut().expect("tracker initialized"),
                    registry,
                )
            })(),
            GenerationStage::Light => (|| {
                let terrain = self.terrain.as_ref().context("light terrain is missing")?;
                let registry = self
                    .light_registry
                    .as_ref()
                    .context("light registry is missing")?;
                let engine = self
                    .light_engine
                    .as_mut()
                    .context("light propagation is not sealed")?;
                self.schedule.run_light(terrain, pos, engine, registry)?;
                self.lit_cache = None;
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

    /// Transfer the complete registration history together with the terrain;
    /// preserve it before later light propagation or decoration.
    pub fn into_parts(
        self,
    ) -> Result<(
        GenerationSchedule,
        BiomeNeighborhood,
        Option<CarvedTerrainWorld>,
        Option<(FreshBatchLight, GeneratedBlockRegistry)>,
    )> {
        ensure!(self.is_complete(), "terrain batch is incomplete or failed");
        let light = match (self.light_tracker, self.light_engine, self.light_registry) {
            (Some(tracker), None, Some(registry)) => {
                Some((FreshBatchLight::Initialized(tracker), registry))
            }
            (None, Some(engine), Some(registry)) => {
                Some((FreshBatchLight::Staged(engine), registry))
            }
            (None, None, _) => None,
            _ => bail!("invalid light registration history"),
        };
        Ok((self.schedule, self.biomes, self.terrain, light))
    }
}
