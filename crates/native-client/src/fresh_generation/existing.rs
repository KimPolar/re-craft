//! Prepare an existing world's noise generator from captured settings.
use super::*;
use anyhow::{Context, bail};
use native_persistence::{
    nbt::{NamedTag, Tag},
    world::{DimensionId, GenerationContextSnapshot, GenerationInputSnapshot, WorldReader},
};
use native_session::worldgen::schedule::GenerationSchedule;

/// A generator bound to the exact captured inputs from which it was prepared.
/// Bind the captured inputs with `add_save_dependencies` when saving.
/// Live state and generation completeness still need independent checks.
pub(crate) struct PreparedExistingGenerator {
    generator: TerrainGenerator,
    lighting: GeneratedDimensionLighting,
    dimension_definition: Value,
    generate_structures: bool,
    resources: ResourceStack,
    context: GenerationContextSnapshot,
    identity: Arc<()>,
    terrain_assets: Option<SavedTerrainAssets>,
    heightmap_tags: Option<native_resources::heightmap_tags::HeightmapTags>,
    feature_schedule: Option<Arc<native_resources::feature_order::FeatureSchedule>>,
    feature_entity_evidence: Option<BTreeMap<native_resources::feature_order::FeatureKey, bool>>,
    structure_schedule:
        Option<Arc<native_resources::structure_decoration::StructureDecorationSchedule>>,
}

struct SavedTerrainAssets {
    tags: CarverBlockTags,
    block_ids: BTreeSet<String>,
    blocks: native_resources::block_registry::GeneratedBlockRegistry,
    carvers: native_resources::carver::schedule::CarverCache,
}

fn validate_saved_decoration_references(
    references: &native_session::worldgen::structure_references::SavedStructureReferences,
) -> Result<()> {
    ensure!(
        references.discarded.is_empty(),
        "saved structure references contain unsupported data"
    );
    Ok(())
}

type SavedStructureKey = (ResourceId, native_world::loaded::ChunkPos);

#[derive(Clone)]
struct ResumedTreasureStart {
    source: Arc<NamedTag>,
    original: Tag,
    references: i32,
    placement: native_resources::structure_start::SimpleStructureStart,
}

struct SavedStructurePlan {
    schedule: Option<Arc<native_resources::structure_decoration::StructureDecorationSchedule>>,
    starts: BTreeMap<SavedStructureKey, ResumedTreasureStart>,
    order: Vec<(ResourceId, Vec<SavedStructureKey>)>,
}

/// Decoration inputs from one captured biome neighborhood and pack snapshot.
/// This plan does not place features or establish terrain or save readiness.
pub(crate) struct SavedDecorationContext {
    identity: Arc<()>,
    position: native_persistence::world::ChunkPos,
    schedule: Arc<native_resources::feature_order::FeatureSchedule>,
    biomes: native_session::worldgen::BiomeNeighborhood,
}

impl SavedDecorationContext {
    pub fn plan<'a>(
        &'a self,
        generator: &PreparedExistingGenerator,
    ) -> Result<native_session::worldgen::DecorationPlan<'a>> {
        ensure!(
            Arc::ptr_eq(&self.identity, &generator.identity),
            "decoration belongs to another generation capture"
        );
        generator.generator.decoration_plan(
            &self.schedule,
            &self.biomes,
            [self.position.x, self.position.z],
        )
    }
}

/// Read-only restoration of captured disk inputs. It does not include unsaved
/// session changes or prove that the chunks can resume every generation stage.
pub(crate) struct RestoredSavedTerrain {
    identity: Arc<()>,
    world: native_world::loaded::World,
    biomes:
        BTreeMap<native_world::loaded::ChunkPos, Arc<native_session::worldgen::QuartBiomeChunk>>,
    heightmaps: BTreeMap<
        native_world::loaded::ChunkPos,
        native_persistence::heightmap::SavedGenerationHeightmaps,
    >,
    postprocessing: BTreeMap<
        native_world::loaded::ChunkPos,
        Option<native_core::postprocessing::PostProcessing>,
    >,
}

/// One regenerated TERRAIN-stage chunk, tied to the captured source and packs.
/// It is not a FULL chunk and cannot be installed into a running session yet.
pub(crate) struct ResumedSavedTerrain {
    identity: Arc<()>,
    position: native_persistence::world::ChunkPos,
    source: Arc<NamedTag>,
    terrain: native_session::worldgen::MaterializedTerrain,
}

/// A partial player-independent NBT draft. Its status and remaining metadata
/// are unchanged, so it is not a completed or writable terrain chunk.
pub(crate) struct ResumedSectionDraft {
    identity: Arc<()>,
    position: native_persistence::world::ChunkPos,
    source_nbt: Vec<u8>,
    document: NamedTag,
}

/// Sections, world-generation heightmaps and ordered fluid marks staged from
/// one capture. Remaining generation data and the status are not finalized.
pub(crate) struct ResumedMetadataDraft {
    source_nbt: Vec<u8>,
    document: NamedTag,
}

/// Original protochunk with only completed saved structure bounds updated.
/// Terrain, feature metadata, live state and save dependencies remain separate.
pub(crate) struct ResumedStructureDraft {
    source_nbt: Vec<u8>,
    document: NamedTag,
}

/// A source-bound, point-in-time feature snapshot. It is not a writable chunk.
pub(crate) struct ResumedFeatureDraft {
    identity: Arc<()>,
    revision: Arc<()>,
    source_nbt: Vec<u8>,
    entity_sidecar: Option<Arc<NamedTag>>,
    feature_entity_coverage: bool,
    document: NamedTag,
}

/// Caller-supplied visible lighting tied to one intermediate terrain revision.
/// Constructing this does not verify the light engine's history or source.
pub(crate) struct ResumedFeatureLight {
    identity: Arc<()>,
    revision: Arc<()>,
    light: native_world::generation_light::VisibleLightSnapshot,
}

/// A fresh light engine's section registrations for one frozen terrain.
/// This does not represent restored light from an existing running world.
pub(crate) struct ResumedInitialLight {
    identity: Arc<()>,
    revision: Arc<()>,
    tracker: native_session::worldgen::FreshGenerationLightTracker,
    initialized: BTreeSet<native_world::loaded::ChunkPos>,
}

/// A frozen resumed terrain whose registered sections can be propagated.
pub(crate) struct ResumedStagedLight {
    identity: Arc<()>,
    revision: Arc<()>,
    engine: native_session::worldgen::StagedGenerationLightEngine,
    initialized: BTreeSet<native_world::loaded::ChunkPos>,
    activated: BTreeSet<native_world::loaded::ChunkPos>,
    light_revision: Arc<()>,
    completed_empty_spawns: BTreeMap<native_world::loaded::ChunkPos, bool>,
}

/// Source-preserving intermediate chunk with a single visible light revision.
/// Its status, entities and other unfinished stages remain unchanged.
pub(crate) struct ResumedLitDraft {
    feature: ResumedFeatureDraft,
    light_revision: Arc<()>,
}

/// A point-in-time partial draft after a spawn stage that added no mobs.
/// Other feature entities and saved entity sidecars can still be incomplete.
pub(crate) struct ResumedSpawnDraft {
    lit: ResumedLitDraft,
    spawn_mobs: bool,
}

impl ResumedSpawnDraft {
    pub fn document(&self) -> &NamedTag {
        self.lit.document()
    }

    pub fn matches_source(&self, source: &NamedTag) -> Result<bool> {
        self.lit.matches_source(source)
    }

    /// The rule must come from the same live session and still be current.
    pub fn matches_state(
        &self,
        world: &ResumedTerrainWorld,
        light: &ResumedStagedLight,
        spawn_mobs: bool,
    ) -> bool {
        self.spawn_mobs == spawn_mobs && self.lit.matches_state(world, light)
    }
}

impl ResumedLitDraft {
    pub fn document(&self) -> &NamedTag {
        self.feature.document()
    }

    pub fn matches_source(&self, source: &NamedTag) -> Result<bool> {
        self.feature.matches_source(source)
    }

    pub fn matches_state(&self, world: &ResumedTerrainWorld, light: &ResumedStagedLight) -> bool {
        self.feature.matches_world(world)
            && matches_resumed_revision(&light.identity, &light.revision, world)
            && Arc::ptr_eq(&self.light_revision, &light.light_revision)
    }
}

fn matches_resumed_revision(
    identity: &Arc<()>,
    revision: &Arc<()>,
    world: &ResumedTerrainWorld,
) -> bool {
    Arc::ptr_eq(identity, &world.identity) && Arc::ptr_eq(revision, &world.revision)
}

fn initialized_light_neighborhood(
    initialized: &BTreeSet<native_world::loaded::ChunkPos>,
    center: native_world::loaded::ChunkPos,
) -> bool {
    (-1i32..=1).all(|dz| {
        (-1i32..=1).all(|dx| {
            center
                .x
                .checked_add(dx)
                .zip(center.z.checked_add(dz))
                .is_some_and(|(x, z)| {
                    initialized.contains(&native_world::loaded::ChunkPos { x, z })
                })
        })
    })
}

impl ResumedFeatureLight {
    fn matches_world(&self, world: &ResumedTerrainWorld) -> bool {
        matches_resumed_revision(&self.identity, &self.revision, world)
    }
}

impl ResumedFeatureDraft {
    pub fn document(&self) -> &NamedTag {
        &self.document
    }
    pub fn matches_source(&self, source: &NamedTag) -> Result<bool> {
        Ok(native_persistence::nbt::write(source)? == self.source_nbt)
    }
    pub fn matches_world(&self, world: &ResumedTerrainWorld) -> bool {
        Arc::ptr_eq(&self.identity, &world.identity) && Arc::ptr_eq(&self.revision, &world.revision)
    }
    /// Whether nine neighboring feature passes have finished and the selected
    /// placed features in every completed pass were proven entity-free.
    /// This does not cover structures, existing entities, spawning or saving.
    pub fn has_entity_free_feature_coverage(&self) -> bool {
        self.feature_entity_coverage
    }
    /// The independently captured entity-region document, not a merged result.
    pub fn entity_sidecar_source(&self) -> Option<&NamedTag> {
        self.entity_sidecar.as_deref()
    }
    pub fn matches_entity_sidecar(&self, source: Option<&NamedTag>) -> Result<bool> {
        Ok(match (&self.entity_sidecar, source) {
            (None, None) => true,
            (Some(original), Some(current)) => {
                native_persistence::nbt::write(original)?
                    == native_persistence::nbt::write(current)?
            }
            _ => false,
        })
    }
}

impl ResumedStructureDraft {
    pub fn document(&self) -> &NamedTag {
        &self.document
    }
    pub fn matches_source(&self, source: &NamedTag) -> Result<bool> {
        Ok(native_persistence::nbt::write(source)? == self.source_nbt)
    }
}

impl ResumedMetadataDraft {
    pub fn document(&self) -> &NamedTag {
        &self.document
    }
    pub fn matches_source(&self, source: &NamedTag) -> Result<bool> {
        Ok(native_persistence::nbt::write(source)? == self.source_nbt)
    }
}

impl ResumedSectionDraft {
    pub fn document(&self) -> &NamedTag {
        &self.document
    }
    pub fn matches_source(&self, source: &NamedTag) -> Result<bool> {
        Ok(native_persistence::nbt::write(source)? == self.source_nbt)
    }
}

/// Intermediate chunks prepared from one generation capture. Their original
/// protochunk documents remain available for the eventual coordinated update.
pub(crate) struct ResumedTerrainWorld {
    identity: Arc<()>,
    terrain: native_session::worldgen::CarvedTerrainWorld,
    sources: BTreeMap<native_persistence::world::ChunkPos, Arc<NamedTag>>,
    feature_touched: BTreeSet<native_world::loaded::ChunkPos>,
    treasure_starts: BTreeMap<SavedStructureKey, ResumedTreasureStart>,
    feature_entity_sources: ResumedFeatureEntitySources,
    revision: Arc<()>,
}

#[derive(Default)]
struct ResumedFeatureEntitySources {
    completed: BTreeSet<native_world::loaded::ChunkPos>,
    unverified: bool,
}

impl ResumedFeatureEntitySources {
    fn complete(&mut self, center: native_world::loaded::ChunkPos, entity_free: bool) {
        self.completed.insert(center);
        self.unverified |= !entity_free;
    }

    fn covers(&self, center: native_world::loaded::ChunkPos) -> bool {
        !self.unverified
            && (-1i32..=1).all(|dz| {
                (-1i32..=1).all(|dx| {
                    center
                        .x
                        .checked_add(dx)
                        .zip(center.z.checked_add(dz))
                        .is_some_and(|(x, z)| {
                            self.completed
                                .contains(&native_world::loaded::ChunkPos { x, z })
                        })
                })
            })
    }
}

impl ResumedSavedTerrain {
    pub fn materialized(&self) -> &native_session::worldgen::MaterializedTerrain {
        &self.terrain
    }

    pub fn into_world(self) -> ResumedTerrainWorld {
        ResumedTerrainWorld {
            identity: self.identity,
            terrain: native_session::worldgen::CarvedTerrainWorld::new(self.terrain),
            sources: BTreeMap::from([(self.position, self.source)]),
            feature_touched: BTreeSet::new(),
            treasure_starts: BTreeMap::new(),
            feature_entity_sources: ResumedFeatureEntitySources::default(),
            revision: Arc::new(()),
        }
    }
}

impl ResumedTerrainWorld {
    pub fn terrain(&self) -> &native_session::worldgen::CarvedTerrainWorld {
        &self.terrain
    }

    pub fn sources(&self) -> &BTreeMap<native_persistence::world::ChunkPos, Arc<NamedTag>> {
        &self.sources
    }

    /// Do not mix results from separate disk/pack captures, even if their
    /// dimensions and seeds happen to match.
    pub fn insert(&mut self, chunk: &ResumedSavedTerrain) -> Result<()> {
        ensure!(
            Arc::ptr_eq(&self.identity, &chunk.identity),
            "terrain belongs to another generation capture"
        );
        ensure!(
            !self.sources.contains_key(&chunk.position),
            "terrain chunk already exists"
        );
        self.terrain.insert(&chunk.terrain)?;
        self.sources
            .insert(chunk.position, Arc::clone(&chunk.source));
        self.revision = Arc::new(());
        Ok(())
    }
}

impl RestoredSavedTerrain {
    pub fn world(&self) -> &native_world::loaded::World {
        &self.world
    }
    pub fn biomes(
        &self,
    ) -> &BTreeMap<native_world::loaded::ChunkPos, Arc<native_session::worldgen::QuartBiomeChunk>>
    {
        &self.biomes
    }
    pub fn heightmaps(
        &self,
    ) -> &BTreeMap<
        native_world::loaded::ChunkPos,
        native_persistence::heightmap::SavedGenerationHeightmaps,
    > {
        &self.heightmaps
    }
    pub fn postprocessing(
        &self,
    ) -> &BTreeMap<
        native_world::loaded::ChunkPos,
        Option<native_core::postprocessing::PostProcessing>,
    > {
        &self.postprocessing
    }
}

impl PreparedExistingGenerator {
    /// Start a new empty engine for captured, unlit terrain. The caller must
    /// independently establish that no saved or live engine state contributes
    /// to this region. Any later terrain change invalidates this tracker.
    pub fn begin_resumed_initial_light(
        &self,
        world: &ResumedTerrainWorld,
    ) -> Result<ResumedInitialLight> {
        ensure!(
            Arc::ptr_eq(&self.identity, &world.identity),
            "terrain belongs to another generation capture"
        );
        for (position, original) in &world.sources {
            let saved = self
                .context
                .inputs()
                .sources()
                .chunks()
                .get(position)
                .and_then(|chunk| chunk.terrain.as_ref())
                .context("light terrain source missing")?;
            ensure!(
                Arc::ptr_eq(original, &saved.document),
                "light terrain source differs"
            );
            reject_saved_lighting(saved)?;
        }
        Ok(ResumedInitialLight {
            identity: Arc::clone(&world.identity),
            revision: Arc::clone(&world.revision),
            tracker: native_session::worldgen::FreshGenerationLightTracker::new(
                &world.terrain,
                self.lighting.has_skylight,
            )?,
            initialized: BTreeSet::new(),
        })
    }

    /// Register a successfully decorated chunk without propagating light.
    pub fn initialize_resumed_light(
        &self,
        world: &ResumedTerrainWorld,
        schedule: &mut GenerationSchedule,
        light: &mut ResumedInitialLight,
        position: native_world::loaded::ChunkPos,
    ) -> Result<()> {
        use native_session::worldgen::schedule::GenerationStage;
        ensure!(
            Arc::ptr_eq(&self.identity, &world.identity)
                && matches_resumed_revision(&light.identity, &light.revision, world),
            "light belongs to another terrain revision"
        );
        ensure!(
            world.feature_entity_sources.completed.contains(&position),
            "light chunk has not completed resumed features"
        );
        ensure!(
            schedule.ready(position, GenerationStage::InitializeLight),
            "light initialization dependencies are incomplete"
        );
        let registry = &self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?
            .blocks;
        schedule.run_light_initialization(
            &world.terrain,
            position,
            &mut light.tracker,
            registry,
        )?;
        light.initialized.insert(position);
        Ok(())
    }

    /// Freeze registrations before propagation. Terrain mutation invalidates it.
    pub fn seal_resumed_light(
        &self,
        world: &ResumedTerrainWorld,
        light: ResumedInitialLight,
    ) -> Result<ResumedStagedLight> {
        ensure!(
            Arc::ptr_eq(&self.identity, &world.identity)
                && matches_resumed_revision(&light.identity, &light.revision, world),
            "light belongs to another terrain revision"
        );
        ensure!(!light.initialized.is_empty(), "no light chunks initialized");
        Ok(ResumedStagedLight {
            identity: light.identity,
            revision: light.revision,
            engine: native_session::worldgen::StagedGenerationLightEngine::new(light.tracker),
            initialized: light.initialized,
            activated: BTreeSet::new(),
            light_revision: Arc::new(()),
            completed_empty_spawns: BTreeMap::new(),
        })
    }

    /// Propagate one chunk only after every adjacent light input was actually
    /// initialized by this tracker. The result is not a writable saved chunk.
    pub fn propagate_resumed_light(
        &self,
        world: &ResumedTerrainWorld,
        schedule: &mut GenerationSchedule,
        light: &mut ResumedStagedLight,
        position: native_world::loaded::ChunkPos,
    ) -> Result<native_world::generation_light::VisibleLightSnapshot> {
        use native_session::worldgen::schedule::GenerationStage;
        ensure!(
            Arc::ptr_eq(&self.identity, &world.identity)
                && matches_resumed_revision(&light.identity, &light.revision, world),
            "light belongs to another terrain revision"
        );
        ensure!(
            initialized_light_neighborhood(&light.initialized, position),
            "light neighborhood is not initialized"
        );
        ensure!(
            schedule.ready(position, GenerationStage::Light),
            "light dependencies are incomplete"
        );
        let registry = &self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?
            .blocks;
        let visible = schedule.run_light(&world.terrain, position, &mut light.engine, registry)?;
        light.activated.insert(position);
        light.light_revision = Arc::new(());
        Ok(visible)
    }

    /// Combine a feature draft with a current, activated light column without
    /// advancing the saved status or authorizing an on-disk write.
    pub fn draft_resumed_lit_chunk(
        &mut self,
        world: &ResumedTerrainWorld,
        light: &ResumedStagedLight,
        position: native_persistence::world::ChunkPos,
    ) -> Result<ResumedLitDraft> {
        ensure!(
            Arc::ptr_eq(&self.identity, &world.identity)
                && matches_resumed_revision(&light.identity, &light.revision, world),
            "light belongs to another terrain revision"
        );
        let key = native_world::loaded::ChunkPos {
            x: position.x,
            z: position.z,
        };
        ensure!(
            light.activated.contains(&key),
            "light chunk has not been propagated"
        );
        let mut feature = self.draft_resumed_feature_chunk(world, position)?;
        let registry = &self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?
            .blocks;
        let visible = light.engine.snapshot(&world.terrain, [key], registry)?;
        let sections = unique_root_field(&feature.document, "sections")?
            .context("feature sections missing")?;
        let sections = native_persistence::generated_section::merge_generated_light_sections(
            sections,
            &visible,
            key,
            self.lighting.min_y,
            self.lighting.height,
            self.lighting.has_skylight,
        )?;
        replace_unique_root_field(&mut feature.document, "sections", sections)?;
        native_persistence::nbt::write(&feature.document)?;
        Ok(ResumedLitDraft {
            feature,
            light_revision: Arc::clone(&light.light_revision),
        })
    }

    /// Complete SPAWN when Java's generator or current game rule prevents
    /// original mob spawning. The caller must capture the rule from the same
    /// session and check live changes before publishing or saving anything.
    pub fn resume_spawn_without_mobs(
        &self,
        world: &ResumedTerrainWorld,
        schedule: &mut GenerationSchedule,
        light: &mut ResumedStagedLight,
        position: native_world::loaded::ChunkPos,
        spawn_mobs: bool,
    ) -> Result<()> {
        use native_session::worldgen::schedule::GenerationStage;
        ensure!(
            Arc::ptr_eq(&self.identity, &world.identity)
                && matches_resumed_revision(&light.identity, &light.revision, world),
            "spawn belongs to another terrain revision"
        );
        ensure!(
            light.activated.contains(&position),
            "spawn chunk has not been lit"
        );
        let disabled = self
            .generator
            .disable_mob_generation()
            .context("noise settings are missing disable_mob_generation")?;
        ensure!(
            disabled || !spawn_mobs,
            "world-generation mob spawning needs a complete spawn context"
        );
        ensure!(
            schedule.ready(position, GenerationStage::Spawn),
            "spawn dependencies are incomplete"
        );
        for (radius, &needed) in GenerationStage::Spawn
            .accumulated()
            .iter()
            .enumerate()
            .skip(1)
        {
            let radius = i32::try_from(radius)?;
            for dz in -radius..=radius {
                for dx in -radius..=radius {
                    if dx.abs().max(dz.abs()) != radius {
                        continue;
                    }
                    let neighbor = native_world::loaded::ChunkPos {
                        x: position
                            .x
                            .checked_add(dx)
                            .context("spawn chunk X overflow")?,
                        z: position
                            .z
                            .checked_add(dz)
                            .context("spawn chunk Z overflow")?,
                    };
                    ensure!(
                        schedule
                            .completed(neighbor)
                            .is_some_and(|stage| stage >= needed),
                        "spawn dependencies are incomplete"
                    );
                }
            }
        }
        ensure!(
            !light.completed_empty_spawns.contains_key(&position),
            "spawn already completed"
        );
        schedule.complete(position, GenerationStage::Spawn)?;
        light.completed_empty_spawns.insert(position, spawn_mobs);
        Ok(())
    }

    /// Retain the recorded proto entities and light without claiming FULL or
    /// inferring that all other entity sources are complete.
    pub fn draft_resumed_empty_spawn(
        &mut self,
        world: &ResumedTerrainWorld,
        light: &ResumedStagedLight,
        position: native_persistence::world::ChunkPos,
        spawn_mobs: bool,
    ) -> Result<ResumedSpawnDraft> {
        let key = native_world::loaded::ChunkPos {
            x: position.x,
            z: position.z,
        };
        ensure!(
            light.completed_empty_spawns.get(&key) == Some(&spawn_mobs),
            "spawn rule or stage differs"
        );
        Ok(ResumedSpawnDraft {
            lit: self.draft_resumed_lit_chunk(world, light, position)?,
            spawn_mobs,
        })
    }
    /// Bind an independently captured, fixed initial light-engine revision to
    /// this intermediate terrain. The caller must establish its provenance,
    /// completeness and that no light sections have been registered or
    /// propagated, and that the visible revision stays fixed through the
    /// feature pass; saved or live lighting cannot be inferred from the chunks.
    pub fn bind_initial_feature_light(
        &self,
        world: &ResumedTerrainWorld,
        light: native_world::generation_light::VisibleLightSnapshot,
    ) -> Result<ResumedFeatureLight> {
        ensure!(
            Arc::ptr_eq(&self.identity, &world.identity),
            "terrain belongs to another generation capture"
        );
        ensure!(
            light.has_block_channel() && light.has_sky_channel() == self.lighting.has_skylight,
            "light channels differ from dimension"
        );
        Ok(ResumedFeatureLight {
            identity: Arc::clone(&self.identity),
            revision: Arc::clone(&world.revision),
            light,
        })
    }
    pub fn inputs(&self) -> &GenerationInputSnapshot {
        self.context.inputs()
    }

    /// Build a source-bound partial structure draft. No status, terrain,
    /// block entities, live changes or disk contents are updated here.
    pub fn draft_resumed_structure_starts(
        &self,
        world: &ResumedTerrainWorld,
        position: native_persistence::world::ChunkPos,
    ) -> Result<ResumedStructureDraft> {
        ensure!(
            Arc::ptr_eq(&self.identity, &world.identity),
            "structure belongs to another generation capture"
        );
        let source = self
            .context
            .inputs()
            .sources()
            .chunks()
            .get(&position)
            .and_then(|entry| entry.terrain.as_ref())
            .context("saved structure source missing")?;
        let source_pos = native_world::loaded::ChunkPos {
            x: position.x,
            z: position.z,
        };
        let updates: Vec<_> = world
            .treasure_starts
            .iter()
            .filter(|((_, pos), _)| *pos == source_pos)
            .collect();
        ensure!(
            !updates.is_empty(),
            "no resumed structure starts for source chunk"
        );
        let source_nbt = native_persistence::nbt::write(&source.document)?;
        let mut document = (*source.document).clone();
        let structures = unique_compound_field_mut(&mut document.value, "structures")?;
        let starts = unique_compound_field_mut(structures, "starts")?;
        for ((id, _), saved) in updates {
            ensure!(
                Arc::ptr_eq(&saved.source, &source.document),
                "saved structure source differs"
            );
            let name = format!("{}:{}", id.namespace, id.path);
            let start = unique_compound_field_mut(starts, &name)?;
            ensure!(
                *start == saved.original,
                "saved structure start differs from captured source"
            );
            *start = native_session::worldgen::saved_structure_starts::draft_saved_treasure_start(
                &saved.original,
                id,
                source_pos,
                saved.references,
                &saved.placement,
            )?;
        }
        native_persistence::nbt::write(&document)?;
        Ok(ResumedStructureDraft {
            source_nbt,
            document,
        })
    }

    /// Capture the currently generated feature data with its original source.
    /// Entity sidecars, later neighboring features, lighting, live state and
    /// save dependencies are not established by this partial draft.
    pub fn draft_resumed_feature_chunk(
        &mut self,
        world: &ResumedTerrainWorld,
        position: native_persistence::world::ChunkPos,
    ) -> Result<ResumedFeatureDraft> {
        use native_world::loaded::ChunkPos;
        ensure!(
            Arc::ptr_eq(&self.identity, &world.identity),
            "terrain belongs to another generation capture"
        );
        let key = ChunkPos {
            x: position.x,
            z: position.z,
        };
        ensure!(
            world.feature_touched.contains(&key),
            "chunk has not been reached by a feature pass"
        );
        let captured = self
            .context
            .inputs()
            .sources()
            .chunks()
            .get(&position)
            .context("generation source missing")?;
        let saved = captured
            .terrain
            .as_ref()
            .context("saved terrain source missing")?;
        let entity_sidecar = captured
            .entities
            .as_ref()
            .map(|saved| Arc::clone(&saved.document));
        validate_empty_biomes_target(saved)?;
        let source = Arc::clone(&saved.document);
        ensure!(
            world
                .sources
                .get(&position)
                .is_some_and(|value| Arc::ptr_eq(value, &source)),
            "resumed terrain source differs"
        );
        reject_saved_lighting(saved)?;
        let original_maps = preserved_heightmap_fields(&source)?;
        validate_empty_saved_postprocessing(&source, self.lighting.min_y, self.lighting.height)?;
        let source_nbt = native_persistence::nbt::write(&source)?;
        let mut document = if world.treasure_starts.keys().any(|(_, pos)| *pos == key) {
            self.draft_resumed_structure_starts(world, position)?
                .document
        } else {
            (*source).clone()
        };
        let biome_ids: BTreeSet<_> = self
            .resources
            .list_data("worldgen/biome")?
            .into_iter()
            .collect();
        let assets = self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?;
        let snapshot = world.terrain.snapshot();
        let sections = native_persistence::generated_section::encode_generated_sections(
            &snapshot,
            key,
            self.lighting.min_y,
            self.lighting.height,
            self.lighting.has_skylight,
            |state| {
                assets
                    .blocks
                    .is_canonical_state(&state.name, &state.properties)
            },
            |name| {
                ResourceId::parse(name)
                    .ok()
                    .is_some_and(|id| biome_ids.contains(&id))
            },
        )?;
        let original_sections =
            unique_root_field(&source, "sections")?.context("saved sections missing")?;
        let sections = native_persistence::generated_section::merge_biome_stage_blocks(
            original_sections,
            &sections,
        )?;
        let block_entities = world
            .terrain
            .generated_block_entity_data(key, &assets.blocks)?;
        let block_ticks = native_persistence::generated_ticks::encode_block_ticks(
            key,
            snapshot
                .saved_block_ticks
                .get(&key)
                .map(Vec::as_slice)
                .unwrap_or_default(),
            |id| assets.block_ids.contains(id),
        )?;
        let fluid_ticks = native_persistence::generated_ticks::encode_fluid_ticks(
            key,
            snapshot
                .saved_fluid_ticks
                .get(&key)
                .map(Vec::as_slice)
                .unwrap_or_default(),
        )?;
        let entities = native_persistence::generated_chunk::append_proto_entity_records(
            unique_root_field(&source, "entities")?,
            world.terrain.recorded_generated_entities(key)?,
        )?;
        let Tag::Compound(generated_maps) = world.terrain.generated_heightmap_data(key)? else {
            unreachable!()
        };
        ensure!(
            generated_maps
                .iter()
                .any(|(name, _)| name.matches("WORLD_SURFACE_WG"))
                && generated_maps
                    .iter()
                    .any(|(name, _)| name.matches("OCEAN_FLOOR_WG")),
            "generated world-generation heightmaps are missing"
        );
        let mut maps = original_maps;
        maps.extend(generated_maps);
        let marks = world
            .terrain
            .postprocessing_marks_to_date(key)
            .context("generation postprocessing is missing")?;
        let marks = native_persistence::postprocessing::encode(&marks);
        native_persistence::postprocessing::decode(
            &marks,
            self.lighting.min_y,
            self.lighting.height,
        )?;
        replace_unique_root_field(&mut document, "sections", sections)?;
        replace_unique_root_field(&mut document, "Heightmaps", Tag::Compound(maps))?;
        replace_unique_root_field(&mut document, "PostProcessing", marks)?;
        replace_unique_root_field(&mut document, "block_entities", block_entities)?;
        replace_unique_root_field(&mut document, "block_ticks", block_ticks)?;
        replace_unique_root_field(&mut document, "fluid_ticks", fluid_ticks)?;
        if let Some(entities) = entities {
            replace_unique_root_field(&mut document, "entities", entities)?;
        }
        native_persistence::nbt::write(&document)?;
        Ok(ResumedFeatureDraft {
            identity: Arc::clone(&self.identity),
            revision: Arc::clone(&world.revision),
            source_nbt,
            entity_sidecar,
            feature_entity_coverage: world.feature_entity_sources.covers(key),
            document,
        })
    }

    fn ensure_terrain_assets(&mut self) -> Result<()> {
        if self.terrain_assets.is_none() {
            let block_ids = self.resources.vanilla_block_ids()?;
            let tags = CarverBlockTags::load(&mut self.resources, &block_ids)?;
            let blocks = self.resources.vanilla_generated_block_registry()?;
            self.terrain_assets = Some(SavedTerrainAssets {
                tags,
                block_ids,
                blocks,
                carvers: native_resources::carver::schedule::CarverCache::default(),
            });
        }
        Ok(())
    }

    /// Resolve every selected feature against the captured pack and registry
    /// before passing it to the caller. This does not establish structure,
    /// terrain, light or scheduler readiness and does not write the world.
    pub fn with_saved_prepared_decoration<T>(
        &mut self,
        context: &SavedDecorationContext,
        action: impl FnOnce(&native_session::worldgen::PreparedDecoration<'_, '_>) -> Result<T>,
    ) -> Result<T> {
        let plan = context.plan(self)?;
        self.ensure_terrain_assets()?;
        let assets = self
            .terrain_assets
            .as_ref()
            .expect("terrain assets initialized");
        let mut resolve = |value: &Value| resolve_generated_terrain_state(value, &assets.blocks);
        let prepared = native_session::worldgen::PreparedDecoration::load(
            &mut self.resources,
            &plan,
            &assets.block_ids,
            &mut resolve,
        )?;
        action(&prepared)
    }

    /// Resolve referenced starts in registry and saved fastutil order. The
    /// original source remains attached until a coordinated save exists.
    fn prepare_saved_structure_plan(
        &mut self,
        world: &ResumedTerrainWorld,
        target: native_persistence::world::ChunkPos,
    ) -> Result<SavedStructurePlan> {
        use native_resources::structure_start::{SimpleStructureKind, SimpleStructureStart};
        use native_session::worldgen::saved_structure_starts::{
            RestoredStructurePiece, RestoredStructureStart,
        };
        let references = self.restore_saved_structure_references(target)?;
        validate_saved_decoration_references(&references)?;
        if references
            .references
            .membership()
            .values()
            .all(BTreeSet::is_empty)
        {
            return Ok(SavedStructurePlan {
                schedule: None,
                starts: BTreeMap::new(),
                order: Vec::new(),
            });
        }
        if self.structure_schedule.is_none() {
            self.structure_schedule = Some(Arc::new(
                native_resources::structure_decoration::StructureDecorationSchedule::load(
                    &mut self.resources,
                )?,
            ));
        }
        let schedule = Arc::clone(
            self.structure_schedule
                .as_ref()
                .expect("structure schedule initialized"),
        );
        let registered: BTreeSet<_> = schedule
            .steps()
            .iter()
            .flatten()
            .map(|entry| &entry.id)
            .collect();
        ensure!(
            references
                .references
                .membership()
                .keys()
                .all(|id| registered.contains(id)),
            "structure reference is absent from decoration registry"
        );
        let mut loaded = BTreeMap::new();
        let mut starts = BTreeMap::new();
        let mut order = Vec::new();
        for entry in schedule.steps().iter().flatten() {
            let Some(sources) = references.references.order(&entry.id) else {
                continue;
            };
            let mut keys = Vec::new();
            for &packed in sources {
                let source = native_world::loaded::ChunkPos {
                    x: packed as u32 as i32,
                    z: ((packed as u64) >> 32) as u32 as i32,
                };
                let key = (entry.id.clone(), source);
                let saved_pos = native_persistence::world::ChunkPos {
                    x: source.x,
                    z: source.z,
                };
                let captured = Arc::clone(
                    &self
                        .context
                        .inputs()
                        .sources()
                        .chunks()
                        .get(&saved_pos)
                        .and_then(|chunk| chunk.terrain.as_ref())
                        .context("referenced structure source missing")?
                        .document,
                );
                let value = if let Some(value) = world.treasure_starts.get(&key) {
                    ensure!(
                        Arc::ptr_eq(&value.source, &captured),
                        "saved structure source differs"
                    );
                    value.clone()
                } else {
                    if !loaded.contains_key(&source) {
                        loaded.insert(source, self.restore_saved_structure_starts(saved_pos)?);
                    }
                    let saved = loaded[&source]
                        .get(&entry.id)
                        .context("referenced structure start missing")?;
                    let RestoredStructureStart::Present {
                        chunk,
                        references,
                        adjustment,
                        pieces,
                        original,
                    } = saved
                    else {
                        continue;
                    };
                    ensure!(*chunk == source, "saved structure start source differs");
                    if pieces.is_empty() {
                        continue;
                    }
                    let [RestoredStructurePiece::BuriedTreasure(bounds)] = pieces.as_slice() else {
                        bail!("saved structure decoration type is not supported");
                    };
                    ResumedTreasureStart {
                        source: captured,
                        original: original.clone(),
                        references: *references,
                        // The saved piece has no generation point. Placement uses
                        // only its current bounds; this value is never persisted.
                        placement: SimpleStructureStart {
                            kind: SimpleStructureKind::BuriedTreasure,
                            chunk: [source.x, source.z],
                            generation_point: bounds.min,
                            orientation: None,
                            bounds: *bounds,
                            terrain_adjustment: *adjustment,
                        },
                    }
                };
                ensure!(
                    starts.insert(key.clone(), value).is_none(),
                    "duplicate saved structure reference"
                );
                keys.push(key);
            }
            if !keys.is_empty() {
                order.push((entry.id.clone(), keys));
            }
        }
        Ok(SavedStructurePlan {
            schedule: Some(schedule),
            starts,
            order,
        })
    }

    /// Run a ready FEATURES step on captured intermediate terrain. The caller
    /// must restore the scheduler from the same verified generation inputs.
    /// Referenced buried treasure can be resumed; other structure kinds and
    /// previously published light are not yet supported.
    /// This does not authorize publishing or saving the resulting chunks.
    pub fn run_saved_features(
        &mut self,
        world: &mut ResumedTerrainWorld,
        context: &SavedDecorationContext,
        schedule: &mut GenerationSchedule,
    ) -> Result<Vec<bool>> {
        self.run_saved_features_inner(world, context, schedule, None)
    }

    /// Use a caller-verified initial light snapshot for brightness-dependent
    /// features. The caller must establish that the visible light is fixed
    /// throughout execution. The snapshot expires after any terrain or
    /// successful feature change. This still does not authorize saving.
    pub fn run_saved_features_with_initial_light(
        &mut self,
        world: &mut ResumedTerrainWorld,
        context: &SavedDecorationContext,
        schedule: &mut GenerationSchedule,
        light: &ResumedFeatureLight,
    ) -> Result<Vec<bool>> {
        self.run_saved_features_inner(world, context, schedule, Some(light))
    }

    fn run_saved_features_inner(
        &mut self,
        world: &mut ResumedTerrainWorld,
        context: &SavedDecorationContext,
        schedule: &mut GenerationSchedule,
        light: Option<&ResumedFeatureLight>,
    ) -> Result<Vec<bool>> {
        use native_session::worldgen::schedule::GenerationStage;
        use native_world::loaded::ChunkPos;
        ensure!(
            Arc::ptr_eq(&self.identity, &world.identity)
                && Arc::ptr_eq(&self.identity, &context.identity),
            "decoration belongs to another generation capture"
        );
        ensure!(
            light.is_none_or(|light| light.matches_world(world)),
            "feature light belongs to another terrain revision"
        );
        let center = ChunkPos {
            x: context.position.x,
            z: context.position.z,
        };
        ensure!(
            schedule.ready(center, GenerationStage::Features),
            "features dependencies are incomplete"
        );
        let evidence = self
            .feature_entity_evidence
            .as_ref()
            .context("feature entity evidence is missing")?;
        let entity_free = context
            .plan(self)?
            .features()
            .iter()
            .all(|feature| evidence.get(feature.feature) == Some(&true));
        let structures_enabled = self.generate_structures;
        let mut structures = if structures_enabled {
            Some(self.prepare_saved_structure_plan(world, context.position)?)
        } else {
            None
        };
        let mut positions = Vec::with_capacity(9);
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                let saved_pos = native_persistence::world::ChunkPos {
                    x: center
                        .x
                        .checked_add(dx)
                        .context("neighbor chunk X overflow")?,
                    z: center
                        .z
                        .checked_add(dz)
                        .context("neighbor chunk Z overflow")?,
                };
                let captured = self
                    .context
                    .inputs()
                    .sources()
                    .chunks()
                    .get(&saved_pos)
                    .and_then(|source| source.terrain.as_ref())
                    .context("saved terrain neighbor was not captured")?;
                let source = world
                    .sources
                    .get(&saved_pos)
                    .context("resumed terrain neighbor is missing")?;
                ensure!(
                    Arc::ptr_eq(source, &captured.document),
                    "resumed terrain source differs"
                );
                positions.push(ChunkPos {
                    x: saved_pos.x,
                    z: saved_pos.z,
                });
            }
        }
        if self.heightmap_tags.is_none() {
            let blocks = self.resources.vanilla_block_ids()?;
            self.heightmap_tags = Some(native_resources::heightmap_tags::HeightmapTags::load(
                &mut self.resources,
                &blocks,
            )?);
        }
        let tags = self
            .heightmap_tags
            .as_ref()
            .expect("heightmap tags initialized");
        for &position in &positions {
            if world.terrain.worldgen_heightmaps(position).is_none() {
                world
                    .terrain
                    .initialize_worldgen_heightmaps(position, tags)?;
            }
        }
        let (result, updates) = self.with_saved_prepared_decoration(context, |prepared| {
            if let Some(structures) = &structures {
                let mut planned = Vec::with_capacity(structures.order.len());
                for (id, keys) in &structures.order {
                    let entry = structures
                        .schedule
                        .as_ref()
                        .context("structure schedule is missing")?
                        .steps()
                        .iter()
                        .flatten()
                        .find(|entry| &entry.id == id)
                        .context("structure schedule entry disappeared")?;
                    let starts = keys
                        .iter()
                        .map(|key| &structures.starts[key].placement)
                        .collect();
                    planned.push(
                        native_session::worldgen::structure_batch::PlannedStructureDecoration {
                            entry,
                            starts,
                        },
                    );
                }
                if let Some(light) = light {
                    schedule.run_features_with_structure_plan_and_visible_light(
                        &mut world.terrain,
                        prepared,
                        &planned,
                        &light.light,
                    )
                } else {
                    schedule.run_features_with_structure_plan(
                        &mut world.terrain,
                        prepared,
                        &planned,
                    )
                }
            } else {
                Ok((
                    if let Some(light) = light {
                        schedule.run_features_with_visible_light(
                            &mut world.terrain,
                            prepared,
                            false,
                            &light.light,
                        )?
                    } else {
                        schedule.run_features(&mut world.terrain, prepared, false, None)?
                    },
                    Vec::new(),
                ))
            }
        })?;
        if let Some(structures) = structures.as_mut() {
            debug_assert_eq!(structures.order.len(), updates.len());
            for ((_, keys), updated) in structures.order.iter().zip(updates) {
                debug_assert_eq!(keys.len(), updated.len());
                for (key, placement) in keys.iter().zip(updated) {
                    structures
                        .starts
                        .get_mut(key)
                        .expect("structure start was planned")
                        .placement = placement;
                    world.feature_touched.insert(key.1);
                }
            }
            world.treasure_starts.append(&mut structures.starts);
        }
        world.feature_touched.extend(positions);
        world.feature_entity_sources.complete(center, entity_free);
        world.revision = Arc::new(());
        Ok(result)
    }

    /// Restore Java's biome-dependent decoration plan for a captured chunk.
    /// Execution still needs complete terrain, structure and light dependencies.
    pub fn prepare_saved_decoration(
        &mut self,
        target: native_persistence::world::ChunkPos,
    ) -> Result<SavedDecorationContext> {
        let mut positions = Vec::with_capacity(9);
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                let position = native_persistence::world::ChunkPos {
                    x: target
                        .x
                        .checked_add(dx)
                        .context("neighbor chunk X overflow")?,
                    z: target
                        .z
                        .checked_add(dz)
                        .context("neighbor chunk Z overflow")?,
                };
                let saved = self
                    .context
                    .inputs()
                    .sources()
                    .chunks()
                    .get(&position)
                    .and_then(|source| source.terrain.as_ref())
                    .context("saved biome neighbor is missing")?;
                reject_saved_blending(&saved.document)?;
                positions.push(position);
            }
        }
        let restored = self.restore_saved_biomes(positions)?;
        let mut biomes = self.generator.biome_neighborhood()?;
        for biome in restored.into_values() {
            biomes.insert(biome)?;
        }
        if self.feature_schedule.is_none() {
            let registry = self
                .resources
                .list_data("worldgen/placed_feature")?
                .into_iter()
                .collect();
            self.feature_schedule = Some(Arc::new(
                self.generator
                    .feature_schedule(&mut self.resources, &registry)?,
            ));
        }
        if self.feature_entity_evidence.is_none() {
            let registry = self
                .resources
                .list_data("worldgen/placed_feature")?
                .into_iter()
                .collect();
            self.feature_entity_evidence = Some(native_session::worldgen::feature_entity_evidence(
                &mut self.resources,
                self.feature_schedule
                    .as_ref()
                    .expect("feature schedule initialized"),
                &registry,
            ));
        }
        let context = SavedDecorationContext {
            identity: Arc::clone(&self.identity),
            position: target,
            schedule: Arc::clone(
                self.feature_schedule
                    .as_ref()
                    .expect("feature schedule initialized"),
            ),
            biomes,
        };
        context.plan(self)?;
        Ok(context)
    }

    /// Restore captured biome volumes against this generator's pack registry.
    /// The caller must still restore other stage data before advancing a plan.
    pub fn restore_saved_biomes(
        &mut self,
        positions: impl IntoIterator<Item = native_persistence::world::ChunkPos>,
    ) -> Result<
        BTreeMap<native_world::loaded::ChunkPos, Arc<native_session::worldgen::QuartBiomeChunk>>,
    > {
        let registry: BTreeSet<_> = self
            .resources
            .list_data("worldgen/biome")?
            .into_iter()
            .collect();
        ensure!(!registry.is_empty(), "active biome registry is empty");
        let mut restored = BTreeMap::new();
        let sources = self.context.inputs().sources().chunks();
        for position in positions {
            let source = sources
                .get(&position)
                .context("biome chunk was not captured")?;
            let source = source
                .terrain
                .as_ref()
                .context("saved biome chunk is missing")?;
            let chunk = native_session::worldgen::QuartBiomeChunk::from_saved_chunk(
                source,
                self.context.inputs().settings().seed(),
                self.lighting.min_y,
                self.lighting.height,
                |biome| {
                    ensure!(
                        registry.contains(biome),
                        "saved biome is not in the active registry: {}:{}",
                        biome.namespace,
                        biome.path
                    );
                    Ok(())
                },
            )?;
            let position = native_world::loaded::ChunkPos {
                x: position.x,
                z: position.z,
            };
            ensure!(
                restored.insert(position, Arc::new(chunk)).is_none(),
                "duplicate saved biome chunk"
            );
        }
        ensure!(!restored.is_empty(), "no saved biome chunks requested");
        Ok(restored)
    }

    /// Restore the effective reference sets from one captured saved chunk.
    /// The returned report identifies data Java would discard on loading.
    pub fn restore_saved_structure_references(
        &mut self,
        position: native_persistence::world::ChunkPos,
    ) -> Result<native_session::worldgen::structure_references::SavedStructureReferences> {
        use native_session::worldgen::{
            schedule::GenerationStage, structure_references::restore_saved_structure_references,
        };
        let registry: BTreeSet<_> = self
            .resources
            .list_data("worldgen/structure")?
            .into_iter()
            .collect();
        let saved = self
            .context
            .inputs()
            .sources()
            .chunks()
            .get(&position)
            .context("structure chunk was not captured")?
            .terrain
            .as_ref()
            .context("saved structure chunk is missing")?;
        let status = saved
            .status
            .as_ref()
            .context("saved chunk status is missing")?;
        let status = String::from_utf16(&status.0).context("invalid saved chunk status")?;
        ensure!(
            GenerationStage::from_id(&status)
                .is_some_and(|stage| stage >= GenerationStage::StructureReferences),
            "saved chunk has not generated structure references"
        );
        let Tag::Compound(root) = &saved.document.value else {
            bail!("saved chunk is not a compound")
        };
        let mut structures = root.iter().filter(|(name, _)| name.matches("structures"));
        let structure = structures.next().map(|(_, tag)| tag);
        ensure!(structures.next().is_none(), "duplicate structure data");
        let references = if let Some(Tag::Compound(fields)) = structure {
            let mut entries = fields.iter().filter(|(name, _)| name.matches("References"));
            let references = entries.next().map(|(_, tag)| tag);
            ensure!(entries.next().is_none(), "duplicate structure references");
            references
        } else {
            structure
        };
        restore_saved_structure_references(
            native_world::loaded::ChunkPos {
                x: position.x,
                z: position.z,
            },
            references,
            |id| Ok(registry.contains(id)),
        )
    }

    /// Restore supported starts using the captured world's active structure
    /// definitions. Unknown or incomplete pieces reject the whole result.
    pub fn restore_saved_structure_starts(
        &mut self,
        position: native_persistence::world::ChunkPos,
    ) -> Result<
        BTreeMap<
            ResourceId,
            native_session::worldgen::saved_structure_starts::RestoredStructureStart,
        >,
    > {
        use native_session::worldgen::{
            saved_structure_starts::restore_saved_structure_starts_with_templates,
            schedule::GenerationStage,
        };
        let registry: BTreeSet<_> = self
            .resources
            .list_data("worldgen/structure")?
            .into_iter()
            .collect();
        let saved = self
            .context
            .inputs()
            .sources()
            .chunks()
            .get(&position)
            .context("structure chunk was not captured")?
            .terrain
            .as_ref()
            .context("saved structure chunk is missing")?;
        let status = saved
            .status
            .as_ref()
            .context("saved chunk status is missing")?;
        let status = String::from_utf16(&status.0).context("invalid saved chunk status")?;
        ensure!(
            GenerationStage::from_id(&status)
                .is_some_and(|stage| stage >= GenerationStage::StructureStarts),
            "saved chunk has not generated structure starts"
        );
        let document = Arc::clone(&saved.document);
        let Tag::Compound(root) = &document.value else {
            bail!("saved chunk is not a compound")
        };
        let mut entries = root.iter().filter(|(name, _)| name.matches("structures"));
        let structures = entries.next().map(|(_, tag)| tag);
        ensure!(entries.next().is_none(), "duplicate structure data");
        let starts = match structures {
            None => None,
            Some(Tag::Compound(fields)) => {
                let mut entries = fields.iter().filter(|(name, _)| name.matches("starts"));
                let value = entries.next().map(|(_, tag)| tag);
                ensure!(entries.next().is_none(), "duplicate structure starts");
                value
            }
            Some(_) => bail!("structure data is not a compound"),
        };
        let resources = std::cell::RefCell::new(&mut self.resources);
        restore_saved_structure_starts_with_templates(
            native_world::loaded::ChunkPos {
                x: position.x,
                z: position.z,
            },
            starts,
            |id| {
                if !registry.contains(id) {
                    return Ok(None);
                }
                let path = format!("data/{}/worldgen/structure/{}.json", id.namespace, id.path);
                let bytes = resources
                    .borrow_mut()
                    .read_data(&path)?
                    .with_context(|| format!("missing structure definition {path}"))?;
                Ok(Some(serde_json::from_slice(&bytes).with_context(|| {
                    format!("invalid structure definition {path}")
                })?))
            },
            |id| {
                let mut resources = resources.borrow_mut();
                let template = native_persistence::structure_template::StructureTemplate::load(
                    &mut **resources,
                    id,
                )?;
                template
                    .map(|template| {
                        ensure!(
                            template.data_version == Some(5023),
                            "unsupported portal template data version"
                        );
                        Ok(template.size)
                    })
                    .transpose()
            },
        )
    }

    /// Prepare terrain adaptation from captured starts and references. The
    /// caller must provide the corresponding Java structure-key order.
    pub fn restore_saved_beardifier(
        &mut self,
        target: native_persistence::world::ChunkPos,
        structure_order: &[ResourceId],
    ) -> Result<native_core::beardifier::Beardifier> {
        use native_session::worldgen::saved_structure_starts::saved_beardifier_for_chunk;
        let references = self.restore_saved_structure_references(target)?;
        ensure!(
            references.discarded.is_empty(),
            "saved structure references contain discarded data"
        );
        let sources: BTreeSet<_> = references
            .references
            .membership()
            .values()
            .flatten()
            .map(|packed| native_persistence::world::ChunkPos {
                x: *packed as u32 as i32,
                z: ((*packed as u64) >> 32) as u32 as i32,
            })
            .collect();
        let mut starts = BTreeMap::new();
        for source in sources {
            let loaded = self.restore_saved_structure_starts(source)?;
            starts.insert(
                native_world::loaded::ChunkPos {
                    x: source.x,
                    z: source.z,
                },
                loaded,
            );
        }
        saved_beardifier_for_chunk(
            native_world::loaded::ChunkPos {
                x: target.x,
                z: target.z,
            },
            &references,
            structure_order,
            &starts,
        )
    }

    /// Resume noise generation from a captured BIOMES-stage chunk using its
    /// saved structure references. This intermediate result is not a completed
    /// chunk: callers must still verify blending, live state, all generation
    /// dependencies, subsequent stages and save dependencies before use.
    pub fn generate_saved_structure_noise(
        &mut self,
        target: native_persistence::world::ChunkPos,
        structure_order: &[ResourceId],
        evaluator: &mut native_resources::density::DensityEvaluator,
    ) -> Result<native_resources::noise_stage::NoiseStageChunk> {
        use native_session::worldgen::schedule::GenerationStage;
        ensure!(
            self.generate_structures,
            "generator has no structure context"
        );
        let saved = self
            .context
            .inputs()
            .sources()
            .chunks()
            .get(&target)
            .context("noise chunk was not captured")?
            .terrain
            .as_ref()
            .context("saved noise chunk is missing")?;
        let status = saved
            .status
            .as_ref()
            .context("saved chunk status is missing")?;
        let status = String::from_utf16(&status.0).context("invalid saved chunk status")?;
        ensure!(
            GenerationStage::from_id(&status) == Some(GenerationStage::Biomes),
            "saved chunk must be at the biomes stage"
        );
        let beardifier = Arc::new(self.restore_saved_beardifier(target, structure_order)?);
        self.generator.generate_noise_with_structure_context(
            evaluator,
            [target.x, target.z],
            beardifier,
        )
    }

    /// Resume a captured BIOMES-stage chunk through terrain, surface and
    /// carvers using its saved neighbors. This is an intermediate result: live
    /// state, remaining stages and save dependencies still require validation.
    pub fn generate_saved_terrain(
        &mut self,
        target: native_persistence::world::ChunkPos,
        structure_order: &[ResourceId],
        evaluator: &mut native_resources::density::DensityEvaluator,
    ) -> Result<native_resources::surface_pass::SurfaceStageChunk> {
        self.generate_saved_terrain_parts(target, structure_order, evaluator)
            .map(|(terrain, _)| terrain)
    }

    /// Materialize the computed terrain with the exact saved biome palette and
    /// retain the untouched source document for later generation stages.
    pub fn resume_saved_terrain(
        &mut self,
        target: native_persistence::world::ChunkPos,
        structure_order: &[ResourceId],
        evaluator: &mut native_resources::density::DensityEvaluator,
    ) -> Result<ResumedSavedTerrain> {
        let (terrain, biome) =
            self.generate_saved_terrain_parts(target, structure_order, evaluator)?;
        let dimension = self.context.inputs().sources().dimension();
        let dimension = format!("{}:{}", dimension.namespace, dimension.path);
        let registry = &self
            .terrain_assets
            .as_ref()
            .expect("terrain assets initialized")
            .blocks;
        let terrain = native_session::worldgen::materialize_carved_terrain(
            terrain,
            &biome,
            &dimension,
            |value| resolve_generated_terrain_state(value, registry),
        )?;
        let source = self
            .context
            .inputs()
            .sources()
            .chunks()
            .get(&target)
            .and_then(|source| source.terrain.as_ref())
            .context("captured terrain target disappeared")?;
        Ok(ResumedSavedTerrain {
            identity: Arc::clone(&self.identity),
            position: target,
            source: Arc::clone(&source.document),
            terrain,
        })
    }

    /// Merge only regenerated blocks into their exact captured protochunk.
    /// Status, heightmaps, ticks, structures and other fields remain untouched.
    pub fn draft_resumed_sections(
        &mut self,
        chunk: &ResumedSavedTerrain,
    ) -> Result<ResumedSectionDraft> {
        ensure!(
            Arc::ptr_eq(&self.identity, &chunk.identity),
            "terrain belongs to another generation capture"
        );
        let captured = self
            .context
            .inputs()
            .sources()
            .chunks()
            .get(&chunk.position)
            .and_then(|source| source.terrain.as_ref())
            .context("captured terrain target missing")?;
        ensure!(
            Arc::ptr_eq(&captured.document, &chunk.source),
            "captured terrain source differs"
        );
        let registry = &self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?
            .blocks;
        let biome_ids: BTreeSet<_> = self
            .resources
            .list_data("worldgen/biome")?
            .into_iter()
            .collect();
        let sections = native_persistence::generated_section::encode_generated_sections(
            chunk.terrain.world(),
            native_world::loaded::ChunkPos {
                x: chunk.position.x,
                z: chunk.position.z,
            },
            self.lighting.min_y,
            self.lighting.height,
            self.lighting.has_skylight,
            |state| registry.is_canonical_state(&state.name, &state.properties),
            |name| {
                ResourceId::parse(name)
                    .ok()
                    .is_some_and(|id| biome_ids.contains(&id))
            },
        )?;
        let mut document = (*chunk.source).clone();
        let Tag::Compound(fields) = &mut document.value else {
            bail!("saved chunk is not a compound")
        };
        let mut sources = fields.iter_mut().filter(|(key, _)| key.matches("sections"));
        let (_, original) = sources.next().context("saved sections are missing")?;
        *original =
            native_persistence::generated_section::merge_biome_stage_blocks(original, &sections)?;
        ensure!(sources.next().is_none(), "duplicate saved sections");
        let source_nbt = native_persistence::nbt::write(&chunk.source)?;
        native_persistence::nbt::write(&document)?;
        Ok(ResumedSectionDraft {
            identity: Arc::clone(&self.identity),
            position: chunk.position,
            source_nbt,
            document,
        })
    }

    /// Add generated heightmaps and ordered marks to a section draft. No
    /// status change or world write is authorized by this partial snapshot.
    pub fn draft_resumed_metadata(
        &mut self,
        world: &mut ResumedTerrainWorld,
        draft: &ResumedSectionDraft,
    ) -> Result<ResumedMetadataDraft> {
        use native_persistence::heightmap::{
            GeneratedHeightmaps, HeightBounds, encode_generated_heightmaps,
        };
        ensure!(
            Arc::ptr_eq(&self.identity, &world.identity)
                && Arc::ptr_eq(&self.identity, &draft.identity),
            "terrain belongs to another generation capture"
        );
        ensure!(
            !world
                .feature_touched
                .contains(&native_world::loaded::ChunkPos {
                    x: draft.position.x,
                    z: draft.position.z,
                }),
            "terrain draft predates feature placement"
        );
        let source = world
            .sources
            .get(&draft.position)
            .context("terrain source is missing")?;
        ensure!(
            draft.matches_source(source)?,
            "terrain draft source differs"
        );
        let original_maps = preserved_heightmap_fields(source)?;
        validate_empty_saved_postprocessing(source, self.lighting.min_y, self.lighting.height)?;
        let position = native_world::loaded::ChunkPos {
            x: draft.position.x,
            z: draft.position.z,
        };
        let marks = world
            .terrain
            .carver_postprocessing(position)
            .context("ordered terrain postprocessing is missing")?;
        let marks = native_persistence::postprocessing::encode(marks);
        native_persistence::postprocessing::decode(
            &marks,
            self.lighting.min_y,
            self.lighting.height,
        )?;
        if self.heightmap_tags.is_none() {
            let blocks = self.resources.vanilla_block_ids()?;
            self.heightmap_tags = Some(native_resources::heightmap_tags::HeightmapTags::load(
                &mut self.resources,
                &blocks,
            )?);
        }
        if world.terrain.worldgen_heightmaps(position).is_none() {
            world.terrain.initialize_worldgen_heightmaps(
                position,
                self.heightmap_tags
                    .as_ref()
                    .expect("heightmap tags initialized"),
            )?;
        }
        let (surface, ocean) = world
            .terrain
            .worldgen_heightmaps(position)
            .context("world-generation heightmaps are missing")?;
        let maps = encode_generated_heightmaps(
            HeightBounds {
                min_y: self.lighting.min_y,
                max_y: self
                    .lighting
                    .min_y
                    .checked_add(self.lighting.height)
                    .context("height overflow")?,
            },
            &GeneratedHeightmaps {
                world_surface_wg: Some(surface),
                ocean_floor_wg: Some(ocean),
                ..Default::default()
            },
        )?;
        let Tag::Compound(generated) = maps else {
            unreachable!()
        };
        let mut combined = original_maps;
        combined.extend(generated);
        let mut document = draft.document.clone();
        replace_unique_root_field(&mut document, "Heightmaps", Tag::Compound(combined))?;
        replace_unique_root_field(&mut document, "PostProcessing", marks)?;
        native_persistence::nbt::write(&document)?;
        Ok(ResumedMetadataDraft {
            source_nbt: draft.source_nbt.clone(),
            document,
        })
    }

    fn generate_saved_terrain_parts(
        &mut self,
        target: native_persistence::world::ChunkPos,
        structure_order: &[ResourceId],
        evaluator: &mut native_resources::density::DensityEvaluator,
    ) -> Result<(
        native_resources::surface_pass::SurfaceStageChunk,
        Arc<native_session::worldgen::QuartBiomeChunk>,
    )> {
        let source = self
            .context
            .inputs()
            .sources()
            .chunks()
            .get(&target)
            .context("terrain target was not captured")?
            .terrain
            .as_ref()
            .context("saved terrain target is missing")?;
        validate_empty_biomes_target(source)?;

        let mut positions = Vec::with_capacity(9);
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                let pos = native_persistence::world::ChunkPos {
                    x: target
                        .x
                        .checked_add(dx)
                        .context("neighbor chunk X overflow")?,
                    z: target
                        .z
                        .checked_add(dz)
                        .context("neighbor chunk Z overflow")?,
                };
                let saved = self
                    .context
                    .inputs()
                    .sources()
                    .chunks()
                    .get(&pos)
                    .context("biome neighbor was not captured")?
                    .terrain
                    .as_ref()
                    .context("saved biome neighbor is missing")?;
                reject_saved_blending(&saved.document)?;
                positions.push(pos);
            }
        }
        let biomes = self.restore_saved_biomes(positions)?;
        let biome = Arc::clone(
            biomes
                .get(&native_world::loaded::ChunkPos {
                    x: target.x,
                    z: target.z,
                })
                .context("target biome is missing")?,
        );
        let mut neighborhood = self.generator.biome_neighborhood()?;
        for biome in biomes.into_values() {
            neighborhood.insert(biome)?;
        }
        let structure = if self.generate_structures {
            Some(Arc::new(
                self.restore_saved_beardifier(target, structure_order)?,
            ))
        } else {
            ensure!(structure_order.is_empty(), "structures are disabled");
            None
        };
        self.ensure_terrain_assets()?;
        let assets = self
            .terrain_assets
            .as_mut()
            .expect("terrain assets initialized");
        let classify = |value: &Value| -> Result<CarverBlockState> {
            let state = resolve_generated_terrain_state(value, &assets.blocks)?;
            Ok(CarverBlockState {
                is_air: matches!(
                    state.name.as_str(),
                    "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                ),
                has_fluid: FluidState::from_block(&state)
                    .map_err(anyhow::Error::msg)?
                    .is_some(),
            })
        };
        let terrain = if let Some(structure) = structure {
            self.generator
                .generate_carved_surface_with_structure_context(
                    &mut self.resources,
                    evaluator,
                    &mut assets.carvers,
                    [target.x, target.z],
                    &neighborhood,
                    &assets.tags,
                    structure,
                    classify,
                )
        } else {
            let previous = evaluator.replace_structure_terrain_context(None);
            let result = self.generator.generate_carved_surface(
                &mut self.resources,
                evaluator,
                &mut assets.carvers,
                [target.x, target.z],
                &neighborhood,
                &assets.tags,
                classify,
            );
            evaluator.replace_structure_terrain_context(previous);
            result
        }?;
        Ok((terrain, biome))
    }

    /// Restore captured terrain and recorded generation metadata without
    /// replacing unknown blocks, priming heightmaps or consuming marks.
    pub fn restore_saved_terrain(
        &mut self,
        positions: impl IntoIterator<Item = native_persistence::world::ChunkPos>,
    ) -> Result<RestoredSavedTerrain> {
        use native_persistence::{
            heightmap::{HeightBounds, SavedGenerationHeightmaps},
            world::ChunkArea,
        };
        use native_session::worldgen::schedule::GenerationStage;

        let mut requested = BTreeSet::new();
        for position in positions {
            ensure!(requested.insert(position), "duplicate saved terrain chunk");
        }
        ensure!(!requested.is_empty(), "no saved terrain chunks requested");
        let biomes = self.restore_saved_biomes(requested.iter().copied())?;
        let block_registry = self.resources.vanilla_generated_block_registry()?;
        let bounds = HeightBounds {
            min_y: self.lighting.min_y,
            max_y: self
                .lighting
                .min_y
                .checked_add(self.lighting.height)
                .context("dimension height overflow")?,
        };
        let mut chunks = BTreeMap::new();
        let mut heightmaps = BTreeMap::new();
        let mut postprocessing = BTreeMap::new();
        for position in requested {
            let source = self
                .context
                .inputs()
                .sources()
                .chunks()
                .get(&position)
                .context("terrain chunk was not captured")?
                .terrain
                .as_ref()
                .context("saved terrain chunk is missing")?;
            let status = source
                .status
                .as_ref()
                .context("saved chunk status is missing")?;
            let status = String::from_utf16(&status.0).context("invalid saved chunk status")?;
            ensure!(
                GenerationStage::from_id(&status)
                    .is_some_and(|stage| stage >= GenerationStage::Terrain),
                "saved chunk has not generated terrain"
            );
            let key = native_world::loaded::ChunkPos {
                x: position.x,
                z: position.z,
            };
            heightmaps.insert(
                key,
                SavedGenerationHeightmaps::read(&source.document, bounds)?,
            );
            let Tag::Compound(fields) = &source.document.value else {
                bail!("saved chunk is not a compound")
            };
            let mut marks = fields
                .iter()
                .filter(|(name, _)| name.matches("PostProcessing"));
            let decoded = marks
                .next()
                .map(|(_, tag)| {
                    native_persistence::postprocessing::decode(
                        tag,
                        bounds.min_y,
                        self.lighting.height,
                    )
                })
                .transpose()?;
            ensure!(
                marks.next().is_none(),
                "duplicate saved postprocessing field"
            );
            postprocessing.insert(key, decoded);
            chunks.insert(position, Arc::new(source.clone()));
        }
        let area = ChunkArea {
            center: *chunks.keys().next().expect("nonempty requested set"),
            radius: 0,
            chunks,
            missing: vec![],
        };
        let world = area.materialize_with_defaults(
            self.context.inputs().sources().dimension(),
            &block_registry.default_states(),
        )?;
        for (_, state) in world.states() {
            ensure!(
                block_registry.is_canonical_state(&state.name, &state.properties),
                "unsupported saved block state: {}",
                state.name
            );
        }
        Ok(RestoredSavedTerrain {
            identity: Arc::clone(&self.identity),
            world,
            biomes,
            heightmaps,
            postprocessing,
        })
    }

    /// Bind every captured terrain position to the current live session.
    /// The returned guard must be checked again on the simulation thread; it
    /// does not protect live block-entity internals, entities, POIs, generation
    /// metadata, game clocks, packs, or disk inputs.
    pub fn capture_live_terrain(
        &self,
        session: &native_session::loaded::LoadedSession,
        restored: &RestoredSavedTerrain,
    ) -> Result<native_session::loaded::GenerationTerrainGuard> {
        ensure!(
            Arc::ptr_eq(&self.identity, &restored.identity),
            "restored terrain belongs to another capture"
        );
        let sources = self.context.inputs().sources().chunks();
        for (pos, source) in sources {
            let key = native_world::loaded::ChunkPos { x: pos.x, z: pos.z };
            ensure!(
                source.terrain.is_some() == restored.world.chunks.contains_key(&key),
                "captured terrain was not fully restored at {key:?}"
            );
        }
        session.capture_generation_terrain(
            &restored.world,
            sources
                .keys()
                .map(|pos| native_world::loaded::ChunkPos { x: pos.x, z: pos.z }),
        )
    }

    /// Bind captured disk and pack inputs; live session state and generation
    /// completeness still require independent checks before saving.
    pub fn add_save_dependencies(
        &self,
        plan: &mut native_persistence::save::WorldSavePlan,
    ) -> Result<()> {
        plan.add_generation_context(&self.context)
    }
}

fn reject_saved_blending(document: &NamedTag) -> Result<()> {
    let Tag::Compound(fields) = &document.value else {
        bail!("saved chunk is not a compound");
    };
    ensure!(
        !fields
            .iter()
            .any(|(name, _)| name.matches("blending_data") || name.matches("below_zero_retrogen")),
        "saved chunk requires unsupported blending or retrogen"
    );
    Ok(())
}

fn unique_root_field<'a>(document: &'a NamedTag, name: &str) -> Result<Option<&'a Tag>> {
    let Tag::Compound(fields) = &document.value else {
        bail!("saved chunk is not a compound")
    };
    let mut matches = fields.iter().filter(|(key, _)| key.matches(name));
    let first = matches.next().map(|(_, value)| value);
    ensure!(matches.next().is_none(), "duplicate saved {name}");
    Ok(first)
}

fn unique_compound_field_mut<'a>(tag: &'a mut Tag, name: &str) -> Result<&'a mut Tag> {
    let Tag::Compound(fields) = tag else {
        bail!("saved {name} parent is not a compound")
    };
    ensure!(
        fields.iter().filter(|(key, _)| key.matches(name)).count() == 1,
        "saved {name} is missing or duplicated"
    );
    Ok(&mut fields
        .iter_mut()
        .find(|(key, _)| key.matches(name))
        .expect("field was validated")
        .1)
}

fn replace_unique_root_field(document: &mut NamedTag, name: &str, value: Tag) -> Result<()> {
    unique_root_field(document, name)?;
    let Tag::Compound(fields) = &mut document.value else {
        unreachable!()
    };
    if let Some((_, old)) = fields.iter_mut().find(|(key, _)| key.matches(name)) {
        *old = value;
    } else {
        fields.push((name.into(), value));
    }
    Ok(())
}

fn preserved_heightmap_fields(
    document: &NamedTag,
) -> Result<Vec<(native_persistence::nbt::JavaString, Tag)>> {
    let Some(value) = unique_root_field(document, "Heightmaps")? else {
        return Ok(Vec::new());
    };
    let Tag::Compound(fields) = value else {
        bail!("saved heightmaps are not a compound")
    };
    let mut names = BTreeSet::new();
    for (name, _) in fields {
        ensure!(names.insert(name.0.clone()), "duplicate saved heightmap");
        ensure!(
            ![
                "WORLD_SURFACE",
                "OCEAN_FLOOR",
                "MOTION_BLOCKING",
                "MOTION_BLOCKING_NO_LEAVES",
                "WORLD_SURFACE_WG",
                "OCEAN_FLOOR_WG"
            ]
            .iter()
            .any(|key| name.matches(key)),
            "existing heightmap needs incremental tracking"
        );
    }
    Ok(fields.clone())
}

fn validate_empty_saved_postprocessing(document: &NamedTag, min_y: i32, height: i32) -> Result<()> {
    let Some(value) = unique_root_field(document, "PostProcessing")? else {
        return Ok(());
    };
    if matches!(value, Tag::List { element_type: native_persistence::nbt::TagType::End | native_persistence::nbt::TagType::List, values } if values.is_empty())
    {
        return Ok(());
    }
    let marks = native_persistence::postprocessing::decode(value, min_y, height)?;
    ensure!(
        marks.sections().iter().all(Vec::is_empty),
        "existing postprocessing marks need incremental tracking"
    );
    Ok(())
}

fn reject_saved_lighting(saved: &native_persistence::world::LoadedChunk) -> Result<()> {
    ensure!(
        !saved.block_light_correct()
            && saved
                .sections
                .iter()
                .all(|section| section.block_light.is_none() && section.sky_light.is_none()),
        "saved lighting cannot be merged into changed terrain"
    );
    if let Some(light) = unique_root_field(&saved.document, "isLightOn")? {
        ensure!(
            matches!(light, Tag::Byte(0)),
            "saved lighting state is unsupported"
        );
    }
    Ok(())
}

fn validate_empty_biomes_target(saved: &native_persistence::world::LoadedChunk) -> Result<()> {
    use native_session::worldgen::schedule::GenerationStage;
    ensure!(
        saved.data_version == native_persistence::world::TARGET_DATA_VERSION,
        "unsupported saved biome data version"
    );
    let status = saved
        .status
        .as_ref()
        .context("saved chunk status is missing")?;
    let status = String::from_utf16(&status.0).context("invalid saved chunk status")?;
    ensure!(
        GenerationStage::from_id(&status) == Some(GenerationStage::Biomes),
        "saved chunk must be at the biomes stage"
    );
    reject_saved_blending(&saved.document)?;
    for section in native_persistence::palette::decode_sections(&saved.document.value)? {
        if let Some(blocks) = section.blocks {
            ensure!(
                blocks.indices.iter().all(|&index| {
                    blocks.states.get(index as usize).is_some_and(|state| {
                        state.name.matches("minecraft:air") && state.properties.is_empty()
                    })
                }),
                "saved biome-stage chunk contains blocks"
            );
        }
    }
    let Tag::Compound(fields) = &saved.document.value else {
        unreachable!()
    };
    ensure!(
        fields
            .iter()
            .filter(|(key, _)| key.matches("sections"))
            .count()
            <= 1,
        "duplicate saved sections"
    );
    for name in [
        "block_entities",
        "block_ticks",
        "fluid_ticks",
        "PostProcessing",
    ] {
        let values: Vec<_> = fields.iter().filter(|(key, _)| key.matches(name)).collect();
        ensure!(values.len() <= 1, "duplicate saved {name}");
        if let Some((_, value)) = values.first() {
            let empty = match value {
                Tag::List { values, .. } if values.is_empty() => true,
                Tag::List {
                    element_type: native_persistence::nbt::TagType::List,
                    values,
                } if name == "PostProcessing" => values
                    .iter()
                    .all(|value| matches!(value, Tag::List { values, .. } if values.is_empty())),
                _ => false,
            };
            ensure!(empty, "saved biome-stage chunk contains {name}");
        }
    }
    Ok(())
}

fn numeric_bool(tag: &Tag) -> Option<bool> {
    Some(match tag {
        Tag::Byte(v) => *v != 0,
        Tag::Short(v) => *v != 0,
        Tag::Int(v) => *v != 0,
        Tag::Long(v) => *v != 0,
        Tag::Float(v) if v.is_finite() => *v != 0.0,
        Tag::Double(v) if v.is_finite() => *v != 0.0,
        _ => return None,
    })
}

// NBT encodes booleans as numbers, so only fields known to have boolean
// codec types can be converted without changing ordinary numeric values.
fn boolean_key(key: &str) -> bool {
    matches!(
        key,
        "has_skylight"
            | "has_ceiling"
            | "has_fixed_time"
            | "has_ender_dragon_fight"
            | "destroy_on_use"
            | "destroy_on_leave"
            | "gameplay/fast_lava"
            | "minecraft:gameplay/fast_lava"
            | "gameplay/water_evaporates"
            | "minecraft:gameplay/water_evaporates"
            | "gameplay/respawn_anchor_works"
            | "minecraft:gameplay/respawn_anchor_works"
            | "legacy_random_source"
            | "disable_mob_generation"
            | "aquifers_enabled"
            | "ore_veins_enabled"
            | "is_3d"
            | "add_stone_depth"
            | "add_surface_depth"
    )
}

fn saved_value(value: &Tag, boolean: bool, depth: usize, nodes: &mut usize) -> Result<Value> {
    ensure!(depth <= 128, "saved dimension is too deeply nested");
    *nodes = nodes
        .checked_add(1)
        .context("saved dimension node count overflow")?;
    ensure!(*nodes <= 1_000_000, "saved dimension has too many values");
    if boolean && let Some(value) = numeric_bool(value) {
        return Ok(Value::Bool(value));
    }
    Ok(match value {
        Tag::Byte(v) => Value::from(*v),
        Tag::Short(v) => Value::from(*v),
        Tag::Int(v) => Value::from(*v),
        Tag::Long(v) => Value::from(*v),
        Tag::Float(v) => Value::Number(
            serde_json::Number::from_f64(f64::from(*v)).context("non-finite saved value")?,
        ),
        Tag::Double(v) => {
            Value::Number(serde_json::Number::from_f64(*v).context("non-finite saved value")?)
        }
        Tag::String(v) => Value::String(String::from_utf16(&v.0).context("invalid saved string")?),
        Tag::List { values, .. } => Value::Array(
            values
                .iter()
                .map(|v| saved_value(v, false, depth + 1, nodes))
                .collect::<Result<_>>()?,
        ),
        Tag::Compound(fields) => {
            let mut output = serde_json::Map::new();
            for (key, value) in fields {
                let key = String::from_utf16(&key.0).context("invalid saved key")?;
                let is_boolean = boolean_key(&key) || (boolean && key == "argument");
                let value = saved_value(value, is_boolean, depth + 1, nodes)?;
                ensure!(
                    output.insert(key, value).is_none(),
                    "duplicate saved dimension field"
                );
            }
            Value::Object(output)
        }
        Tag::ByteArray(values) => {
            Value::Array(values.iter().map(|v| Value::from(*v as i8)).collect())
        }
        Tag::IntArray(values) => Value::Array(values.iter().map(|v| Value::from(*v)).collect()),
        Tag::LongArray(values) => Value::Array(values.iter().map(|v| Value::from(*v)).collect()),
    })
}

fn resolve_stem(
    provider: &mut impl ResourceProvider,
    document: &NamedTag,
    dimension: &DimensionId,
) -> Result<(Value, bool)> {
    let Tag::Compound(data) = document
        .value
        .get("data")
        .context("missing world generation data")?
    else {
        bail!("world generation data is not a compound");
    };
    let one = |name: &str| -> Result<Option<&Tag>> {
        let mut found = data.iter().filter(|(key, _)| key.matches(name));
        let value = found.next().map(|(_, value)| value);
        ensure!(
            found.next().is_none(),
            "duplicate world generation field {name}"
        );
        Ok(value)
    };
    let generate_structures = one("generate_structures")?
        .map(|v| numeric_bool(v).context("invalid generate_structures"))
        .transpose()?
        .unwrap_or(true);
    let Tag::Compound(dimensions) = one("dimensions")?.context("missing saved dimensions")? else {
        bail!("saved dimensions are not a compound");
    };
    let mut saved = None;
    let mut names = BTreeSet::new();
    for (name, stem) in dimensions {
        let name = String::from_utf16(&name.0).context("invalid saved dimension name")?;
        let id = DimensionId::parse(&name)?;
        ensure!(names.insert(id.clone()), "duplicate saved dimension {name}");
        if &id == dimension {
            saved = Some(stem);
        }
    }
    let path = format!(
        "data/{}/dimension/{}.json",
        dimension.namespace, dimension.path
    );
    if let Some(bytes) = provider.read_data(&path)? {
        ensure!(
            bytes.len() <= 16 * 1024 * 1024,
            "dimension definition is too large"
        );
        let stem =
            serde_json::from_slice(&bytes).with_context(|| format!("invalid dimension {path}"))?;
        return Ok((stem, generate_structures));
    }
    let saved = saved.context("dimension is missing from the saved world and active packs")?;
    ensure!(
        matches!(saved, Tag::Compound(_)),
        "saved dimension is not a compound"
    );
    Ok((saved_value(saved, false, 0, &mut 0)?, generate_structures))
}

/// Compile using the saved generator and the pack selection captured from the
/// same world. A later save must bind these inputs as transaction dependencies.
pub(crate) fn prepare_existing_generator(
    context: GenerationContextSnapshot,
    minecraft_dir: &std::path::Path,
) -> Result<PreparedExistingGenerator> {
    let packs = Arc::clone(context.packs());
    packs.verify()?;
    let selected = context.selection();
    let selection = DataPackSettings {
        enabled: selected.enabled.clone(),
        disabled: selected.disabled.clone(),
        enabled_features: selected.enabled_features.clone(),
    };
    let mut resources = ResourceStack::open(minecraft_dir)?;
    let warnings = resources.configure_snapshot_data(Arc::clone(&packs), &selection)?;
    ensure!(
        warnings.is_empty(),
        "data pack selection needs attention: {}",
        warnings.join("; ")
    );
    let inputs = context.inputs();
    let dimension = inputs.sources().dimension();
    let (stem, generate_structures) =
        resolve_stem(&mut resources, inputs.settings().document(), dimension)?;
    let id = ResourceId::parse(&format!("{}:{}", dimension.namespace, dimension.path))?;
    let (lighting, definition, dimension_definition) =
        GeneratedDimensionLighting::from_stem(&mut resources, id, &stem)?;
    let seed = inputs.settings().seed();
    let generator = if generate_structures {
        TerrainGenerator::with_structure_context(
            &mut resources,
            &definition,
            seed,
            lighting.min_y,
            lighting.height,
        )?
    } else {
        TerrainGenerator::without_terrain_adjustments(
            &mut resources,
            &definition,
            seed,
            lighting.min_y,
            lighting.height,
        )?
    };
    packs.verify()?;
    Ok(PreparedExistingGenerator {
        generator,
        lighting,
        dimension_definition,
        generate_structures,
        resources,
        context,
        identity: Arc::new(()),
        terrain_assets: None,
        heightmap_tags: None,
        feature_schedule: None,
        feature_entity_evidence: None,
        structure_schedule: None,
    })
}

/// Capture the scheduler's complete current neighborhood before preparing the
/// generator. If the schedule is expanded, callers must capture and prepare
/// again; dynamic inputs outside this schedule and unsaved session changes
/// still need independent tracking.
pub(crate) fn prepare_existing_generator_for_schedule(
    reader: &mut WorldReader,
    schedule: &GenerationSchedule,
    minecraft_dir: &std::path::Path,
) -> Result<PreparedExistingGenerator> {
    let context = capture_scheduled_context(reader, schedule)?;
    prepare_existing_generator(context, minecraft_dir)
}

fn capture_scheduled_context(
    reader: &mut WorldReader,
    schedule: &GenerationSchedule,
) -> Result<GenerationContextSnapshot> {
    ensure!(schedule.requested_chunks() != 0, "no generation requests");
    reader.capture_generation_context(
        schedule
            .requirements()
            .map(|(pos, _)| native_persistence::world::ChunkPos { x: pos.x, z: pos.z }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_persistence::nbt::{JavaString, TagType};
    use serde_json::json;

    #[test]
    fn resumed_light_neighborhood_requires_all_columns_and_checked_coordinates() {
        use native_world::loaded::ChunkPos;
        let center = ChunkPos { x: -2, z: 3 };
        let mut initialized = BTreeSet::new();
        for dz in -1..=1 {
            for dx in -1..=1 {
                if (dx, dz) != (1, 1) {
                    initialized.insert(ChunkPos {
                        x: center.x + dx,
                        z: center.z + dz,
                    });
                }
            }
        }
        assert!(!initialized_light_neighborhood(&initialized, center));
        initialized.insert(ChunkPos { x: -1, z: 4 });
        assert!(initialized_light_neighborhood(&initialized, center));
        assert!(!initialized_light_neighborhood(
            &initialized,
            ChunkPos { x: i32::MAX, z: 0 }
        ));
    }

    #[test]
    fn resumed_feature_entity_coverage_requires_all_neighbors_and_no_unknown_passes() {
        use native_world::loaded::ChunkPos;
        let center = ChunkPos { x: 5, z: -3 };
        let mut sources = ResumedFeatureEntitySources::default();
        assert!(!sources.covers(center));
        for dz in -1..=1 {
            for dx in -1..=1 {
                if (dx, dz) != (1, 1) {
                    sources.complete(
                        ChunkPos {
                            x: center.x + dx,
                            z: center.z + dz,
                        },
                        true,
                    );
                }
            }
        }
        assert!(!sources.covers(center));
        sources.complete(ChunkPos { x: 6, z: -2 }, true);
        assert!(sources.covers(center));
        sources.complete(ChunkPos { x: 100, z: 100 }, false);
        assert!(!sources.covers(center));
        sources.complete(ChunkPos { x: 100, z: 100 }, true);
        assert!(!sources.covers(center));
        assert!(!ResumedFeatureEntitySources::default().covers(ChunkPos { x: i32::MAX, z: 0 }));
    }

    #[test]
    fn saved_decoration_references_reject_discarded_data() -> Result<()> {
        use native_session::worldgen::structure_references::restore_saved_structure_references;
        let center = native_world::loaded::ChunkPos { x: 3, z: 3 };
        let parse = |tag: Option<&Tag>, registered| {
            restore_saved_structure_references(center, tag, |_| Ok(registered))
        };
        validate_saved_decoration_references(&parse(None, true)?)?;
        let empty = Tag::Compound(vec![(
            "minecraft:buried_treasure".into(),
            Tag::LongArray(vec![]),
        )]);
        validate_saved_decoration_references(&parse(Some(&empty), true)?)?;
        let present = Tag::Compound(vec![(
            "minecraft:buried_treasure".into(),
            Tag::LongArray(vec![3 | (3i64 << 32)]),
        )]);
        validate_saved_decoration_references(&parse(Some(&present), true)?)?;
        assert!(validate_saved_decoration_references(&parse(Some(&present), false)?).is_err());
        assert!(validate_saved_decoration_references(&parse(Some(&Tag::Int(1)), true)?).is_err());
        Ok(())
    }

    struct Packs(BTreeMap<String, Vec<u8>>);
    impl ResourceProvider for Packs {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.0.get(path).cloned())
        }
    }

    fn settings(stem: Tag) -> NamedTag {
        NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                (
                    "data".into(),
                    Tag::Compound(vec![
                        ("seed".into(), Tag::Long(42)),
                        ("generate_structures".into(), Tag::Byte(0)),
                        (
                            "dimensions".into(),
                            Tag::Compound(vec![("minecraft:overworld".into(), stem)]),
                        ),
                    ]),
                ),
            ]),
        }
    }

    #[test]
    fn saved_biome_target_refuses_terrain_or_deferred_changes() -> Result<()> {
        use native_persistence::{
            palette::decode_sections,
            world::{ChunkPos, LoadedChunk},
        };
        let mut document = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "sections".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![Tag::Compound(vec![
                        ("Y".into(), Tag::Byte(0)),
                        (
                            "block_states".into(),
                            Tag::Compound(vec![(
                                "palette".into(),
                                Tag::List {
                                    element_type: TagType::String,
                                    values: vec![Tag::String("minecraft:air".into())],
                                },
                            )]),
                        ),
                    ])],
                },
            )]),
        };
        let loaded = |document: &NamedTag| -> Result<LoadedChunk> {
            Ok(LoadedChunk {
                position: ChunkPos { x: 0, z: 0 },
                data_version: 5023,
                status: Some("minecraft:biomes".into()),
                document: Arc::new(document.clone()),
                sections: decode_sections(&document.value)?,
            })
        };
        assert!(validate_empty_biomes_target(&loaded(&document)?).is_ok());
        assert!(reject_saved_lighting(&loaded(&document)?).is_ok());
        for (marker, allowed) in [
            (Tag::Byte(0), true),
            (Tag::Byte(1), false),
            (Tag::Byte(2), false),
            (Tag::Int(0), false),
        ] {
            let mut light = document.clone();
            let Tag::Compound(fields) = &mut light.value else {
                unreachable!()
            };
            fields.push(("isLightOn".into(), marker));
            assert_eq!(reject_saved_lighting(&loaded(&light)?).is_ok(), allowed);
        }
        let mut duplicate = document.clone();
        let Tag::Compound(fields) = &mut duplicate.value else {
            unreachable!()
        };
        fields.push(("isLightOn".into(), Tag::Byte(0)));
        fields.push(("isLightOn".into(), Tag::Byte(0)));
        assert!(reject_saved_lighting(&loaded(&duplicate)?).is_err());
        for name in ["BlockLight", "SkyLight"] {
            let mut light = document.clone();
            let Tag::Compound(fields) = &mut light.value else {
                unreachable!()
            };
            let Tag::List { values, .. } = &mut fields[0].1 else {
                unreachable!()
            };
            let Tag::Compound(section) = &mut values[0] else {
                unreachable!()
            };
            section.push((name.into(), Tag::ByteArray(vec![0; 2048])));
            assert!(reject_saved_lighting(&loaded(&light)?).is_err());
        }
        let mut later = loaded(&document)?;
        later.status = Some("minecraft:terrain".into());
        assert!(validate_empty_biomes_target(&later).is_err());
        let Tag::Compound(fields) = &mut document.value else {
            unreachable!()
        };
        fields.push((
            "PostProcessing".into(),
            Tag::List {
                element_type: TagType::List,
                values: vec![Tag::List {
                    element_type: TagType::End,
                    values: vec![],
                }],
            },
        ));
        assert!(validate_empty_biomes_target(&loaded(&document)?).is_ok());
        let Tag::Compound(fields) = &mut document.value else {
            unreachable!()
        };
        let Tag::List { values, .. } = &mut fields.last_mut().unwrap().1 else {
            unreachable!()
        };
        values[0] = Tag::List {
            element_type: TagType::Short,
            values: vec![Tag::Short(1)],
        };
        assert!(validate_empty_biomes_target(&loaded(&document)?).is_err());
        let Tag::Compound(fields) = &mut document.value else {
            unreachable!()
        };
        fields.pop();
        let Tag::Compound(fields) = &mut document.value else {
            unreachable!()
        };
        fields.push(("blending_data".into(), Tag::Compound(vec![])));
        assert!(validate_empty_biomes_target(&loaded(&document)?).is_err());
        let Tag::Compound(fields) = &mut document.value else {
            unreachable!()
        };
        fields.pop();
        fields.push((
            "block_ticks".into(),
            Tag::List {
                element_type: TagType::Compound,
                values: vec![Tag::Compound(vec![])],
            },
        ));
        assert!(validate_empty_biomes_target(&loaded(&document)?).is_err());
        let Tag::Compound(fields) = &mut document.value else {
            unreachable!()
        };
        fields.pop();
        let Tag::List { values, .. } = &mut fields[0].1 else {
            unreachable!()
        };
        let Tag::Compound(section) = &mut values[0] else {
            unreachable!()
        };
        let Tag::Compound(states) = &mut section[1].1 else {
            unreachable!()
        };
        let Tag::List { values, .. } = &mut states[0].1 else {
            unreachable!()
        };
        values[0] = Tag::String("minecraft:stone".into());
        assert!(validate_empty_biomes_target(&loaded(&document)?).is_err());
        Ok(())
    }

    #[test]
    fn resumed_metadata_rejects_untracked_heightmaps_and_marks() -> Result<()> {
        let make = |fields| NamedTag {
            name: "".into(),
            value: Tag::Compound(fields),
        };
        let custom = make(vec![(
            "Heightmaps".into(),
            Tag::Compound(vec![("example:opaque".into(), Tag::Int(9))]),
        )]);
        assert_eq!(preserved_heightmap_fields(&custom)?.len(), 1);
        for name in [
            "WORLD_SURFACE",
            "OCEAN_FLOOR",
            "MOTION_BLOCKING",
            "MOTION_BLOCKING_NO_LEAVES",
            "WORLD_SURFACE_WG",
            "OCEAN_FLOOR_WG",
        ] {
            let source = make(vec![(
                "Heightmaps".into(),
                Tag::Compound(vec![(name.into(), Tag::LongArray(vec![]))]),
            )]);
            assert!(preserved_heightmap_fields(&source).is_err(), "{name}");
        }
        let duplicate = make(vec![
            ("Heightmaps".into(), Tag::Compound(vec![])),
            ("Heightmaps".into(), Tag::Compound(vec![])),
        ]);
        assert!(preserved_heightmap_fields(&duplicate).is_err());
        assert!(validate_empty_saved_postprocessing(&make(vec![]), 0, 16).is_ok());
        let mut marks = native_core::postprocessing::PostProcessing::new(0, 16).unwrap();
        let empty = make(vec![(
            "PostProcessing".into(),
            native_persistence::postprocessing::encode(&marks),
        )]);
        assert!(validate_empty_saved_postprocessing(&empty, 0, 16).is_ok());
        assert!(validate_empty_saved_postprocessing(&empty, 0, 32).is_err());
        marks.mark(0, 0, 0);
        let nonempty = make(vec![(
            "PostProcessing".into(),
            native_persistence::postprocessing::encode(&marks),
        )]);
        assert!(validate_empty_saved_postprocessing(&nonempty, 0, 16).is_err());
        Ok(())
    }

    #[test]
    fn scheduled_capture_tracks_every_required_neighbor_and_expansion() -> Result<()> {
        use flate2::{Compression, write::GzEncoder};
        use native_session::worldgen::schedule::GenerationStage;
        use native_world::loaded::ChunkPos;
        use std::io::Write;

        let world = tempfile::tempdir()?;
        let root = world.path();
        std::fs::create_dir_all(root.join("data/minecraft"))?;
        std::fs::create_dir_all(root.join("dimensions/minecraft/overworld/region"))?;
        std::fs::write(
            root.join("data/minecraft/world_gen_settings.dat"),
            native_persistence::nbt::write(&settings(Tag::Compound(vec![])))?,
        )?;
        let level = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "Data".into(),
                Tag::Compound(vec![("DataVersion".into(), Tag::Int(5023))]),
            )]),
        };
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&native_persistence::nbt::write(&level)?)?;
        std::fs::write(root.join("level.dat"), gzip.finish()?)?;
        let mut reader = WorldReader::open_for_session(root, DimensionId::parse("overworld")?)?;
        let mut schedule = GenerationSchedule::new();
        assert!(capture_scheduled_context(&mut reader, &schedule).is_err());
        let target = ChunkPos { x: -3, z: 4 };
        schedule.request_many([
            (target, GenerationStage::Full),
            (ChunkPos { x: -2, z: 4 }, GenerationStage::Terrain),
        ])?;
        let expected: BTreeSet<_> = schedule
            .requirements()
            .map(|(pos, _)| (pos.x, pos.z))
            .collect();
        let captured = capture_scheduled_context(&mut reader, &schedule)?;
        let sources = captured.inputs().sources().chunks();
        assert_eq!(
            sources
                .keys()
                .map(|pos| (pos.x, pos.z))
                .collect::<BTreeSet<_>>(),
            expected
        );
        assert!(sources.values().all(|source| {
            source.terrain.is_none() && source.entities.is_none() && source.poi.is_none()
        }));
        assert!(expected.len() > 1);

        let before = schedule.requested_chunks();
        assert!(
            schedule
                .request(ChunkPos { x: i32::MAX, z: 0 }, GenerationStage::Full)
                .is_err()
        );
        assert_eq!(schedule.requested_chunks(), before);
        schedule.request(ChunkPos { x: 50, z: 50 }, GenerationStage::Full)?;
        let expanded = capture_scheduled_context(&mut reader, &schedule)?;
        assert!(expanded.inputs().sources().chunks().len() > sources.len());
        assert_eq!(
            expanded.inputs().sources().chunks().len(),
            schedule.requested_chunks()
        );
        Ok(())
    }

    #[test]
    fn saved_stems_keep_numeric_values_and_respect_active_overrides() -> Result<()> {
        let stem = Tag::Compound(vec![
            ("type".into(), Tag::String("minecraft:overworld".into())),
            (
                "generator".into(),
                Tag::Compound(vec![
                    ("type".into(), Tag::String("minecraft:noise".into())),
                    (
                        "settings".into(),
                        Tag::Compound(vec![
                            ("legacy_random_source".into(), Tag::Byte(1)),
                            ("sea_level".into(), Tag::Byte(1)),
                            (
                                "material_rule".into(),
                                Tag::Compound(vec![("add_stone_depth".into(), Tag::Byte(0))]),
                            ),
                        ]),
                    ),
                    (
                        "biome_source".into(),
                        Tag::Compound(vec![
                            ("type".into(), Tag::String("minecraft:fixed".into())),
                            ("biome".into(), Tag::String("minecraft:plains".into())),
                        ]),
                    ),
                ]),
            ),
        ]);
        let doc = settings(stem.clone());
        let dimension = DimensionId::parse("overworld")?;
        let mut packs = Packs(BTreeMap::new());
        let (saved, structures) = resolve_stem(&mut packs, &doc, &dimension)?;
        assert!(!structures);
        assert_eq!(saved["generator"]["settings"]["legacy_random_source"], true);
        assert_eq!(saved["generator"]["settings"]["sea_level"], 1);
        assert_eq!(
            saved["generator"]["settings"]["material_rule"]["add_stone_depth"],
            false
        );
        let active = json!({"type":"minecraft:the_nether","generator":{"type":"minecraft:noise","settings":"minecraft:nether","biome_source":{"type":"minecraft:the_end"}}});
        packs.0.insert(
            "data/minecraft/dimension/overworld.json".into(),
            serde_json::to_vec(&active)?,
        );
        assert_eq!(resolve_stem(&mut packs, &doc, &dimension)?.0, active);
        assert!(resolve_stem(&mut packs, &doc, &DimensionId::parse("minecraft:missing")?).is_err());
        let duplicate = settings(Tag::Compound(vec![
            ("type".into(), Tag::String("minecraft:overworld".into())),
            ("type".into(), Tag::String("minecraft:the_nether".into())),
        ]));
        packs.0.clear();
        assert!(resolve_stem(&mut packs, &duplicate, &dimension).is_err());
        let malformed = settings(Tag::Compound(vec![(
            "type".into(),
            Tag::String(JavaString(vec![0xd800])),
        )]));
        assert!(resolve_stem(&mut packs, &malformed, &dimension).is_err());
        assert!(
            saved_value(
                &Tag::List {
                    element_type: TagType::Int,
                    values: vec![Tag::Int(7)]
                },
                false,
                0,
                &mut 0
            )?
            .is_array()
        );
        Ok(())
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3, Java 25 JDK and a temporary world"]
    fn captured_vanilla_world_prepares_each_noise_dimension() -> Result<()> {
        use flate2::{Compression, write::GzEncoder};
        use native_persistence::{
            save::WorldSavePlan,
            world::{ChunkPos, WorldReader},
            worldgen_settings::{NewWorldOptions, draft_baked_worldgen_settings},
        };
        use std::io::{Seek, SeekFrom, Write};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
            .join("bin");
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        let mut paths = vec![version.join("26.3.jar"), root.to_owned()];
        for library in manifest["libraries"]
            .as_array()
            .context("missing libraries")?
        {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let classpath = std::env::join_paths(paths)?;
        let oracle = root.join("ResumedTerrainOracle.java");
        std::fs::write(
            &oracle,
            include_str!("../../tests/java/ResumedTerrainOracle.java"),
        )?;
        let treasure_oracle = root.join("ResumedTreasureDraftOracle.java");
        std::fs::write(
            &treasure_oracle,
            include_str!("../../tests/java/ResumedTreasureDraftOracle.java"),
        )?;
        let light_oracle = root.join("ResumedLightOracle.java");
        std::fs::write(
            &light_oracle,
            include_str!("../../tests/java/ResumedLightOracle.java"),
        )?;
        let compiled = std::process::Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&oracle)
            .arg(&treasure_oracle)
            .arg(&light_oracle)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java oracle compilation failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let mut source = ResourceStack::open(&game)?;
        let worldgen = draft_baked_worldgen_settings(
            &mut source,
            "minecraft:normal",
            NewWorldOptions {
                seed: -42,
                generate_structures: true,
                bonus_chest: false,
            },
        )?;
        std::fs::create_dir_all(root.join("data/minecraft"))?;
        std::fs::write(
            root.join("data/minecraft/world_gen_settings.dat"),
            native_persistence::nbt::write(&worldgen)?,
        )?;
        let level = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "Data".into(),
                Tag::Compound(vec![("DataVersion".into(), Tag::Int(5023))]),
            )]),
        };
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&native_persistence::nbt::write(&level)?)?;
        std::fs::write(root.join("level.dat"), gzip.finish()?)?;
        for name in ["overworld", "the_nether", "the_end"] {
            use native_persistence::save::{FreshRegionDraft, FreshRegionKind};
            let region_dir = root.join(format!("dimensions/minecraft/{name}/region"));
            std::fs::create_dir_all(&region_dir)?;
            let lighting = GeneratedDimensionLighting::load_preset(
                &mut source,
                "minecraft:normal",
                &format!("minecraft:{name}"),
            )?;
            let sections = (0..lighting.height / 16)
                .map(|index| {
                    let section_y = i8::try_from(lighting.min_y / 16 + index).unwrap();
                    let biome = if index % 2 == 0 {
                        "minecraft:plains"
                    } else {
                        "minecraft:forest"
                    };
                    Tag::Compound(vec![
                        ("Y".into(), Tag::Byte(section_y)),
                        (
                            "block_states".into(),
                            Tag::Compound(vec![(
                                "palette".into(),
                                Tag::List {
                                    element_type: TagType::String,
                                    values: vec![Tag::String("minecraft:stone".into())],
                                },
                            )]),
                        ),
                        (
                            "biomes".into(),
                            Tag::Compound(vec![(
                                "palette".into(),
                                Tag::List {
                                    element_type: TagType::String,
                                    values: vec![Tag::String(biome.into())],
                                },
                            )]),
                        ),
                    ])
                })
                .collect();
            let saved_height = [lighting.min_y + 7; 256];
            let maps = native_persistence::heightmap::encode_generated_heightmaps(
                native_persistence::heightmap::HeightBounds {
                    min_y: lighting.min_y,
                    max_y: lighting.min_y + lighting.height,
                },
                &native_persistence::heightmap::GeneratedHeightmaps {
                    world_surface_wg: Some(&saved_height),
                    ..Default::default()
                },
            )?;
            let mut marks =
                native_core::postprocessing::PostProcessing::new(lighting.min_y, lighting.height)
                    .unwrap();
            marks.mark(0, lighting.min_y + 1, 0);
            marks.mark(0, lighting.min_y + 1, 0);
            let template_id = ResourceId::parse("minecraft:ruined_portal/portal_1")?;
            let template = native_persistence::structure_template::StructureTemplate::load(
                &mut source,
                &template_id,
            )?
            .context("portal template is missing")?;
            let selection = native_resources::ruined_portal::InitialSelection {
                setup_index: 0,
                air_pocket: false,
                template: template_id,
                rotation: native_resources::structure_transform::Rotation::None,
                mirror: native_resources::structure_transform::Mirror::None,
            };
            let setup = native_resources::ruined_portal::Setup {
                placement: native_resources::ruined_portal::VerticalPlacement::OnLandSurface,
                air_pocket_probability: 0.0,
                mossiness: 0.2,
                overgrown: false,
                vines: false,
                can_be_cold: true,
                replace_with_blackstone: false,
                weight: 1.0,
            };
            let portal_position = [0, lighting.min_y + 8, 0];
            let pivot = [template.size[0] / 2, 0, template.size[2] / 2];
            let portal_point = native_resources::ruined_portal::GenerationPoint {
                position: portal_position,
                pivot,
                bounds: native_resources::structure_transform::bounding_box(
                    template.size,
                    portal_position,
                    selection.mirror,
                    selection.rotation,
                    pivot,
                )?,
            };
            let portal = native_session::worldgen::ruined_portal::SavedRuinedPortalPiece::capture(
                &selection,
                &setup,
                &portal_point,
                template.size,
                false,
            )?;
            let saved = NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![
                    ("DataVersion".into(), Tag::Int(5023)),
                    ("xPos".into(), Tag::Int(0)),
                    ("zPos".into(), Tag::Int(0)),
                    ("Status".into(), Tag::String("minecraft:full".into())),
                    ("Heightmaps".into(), maps),
                    (
                        "PostProcessing".into(),
                        native_persistence::postprocessing::encode(&marks),
                    ),
                    (
                        "structures".into(),
                        Tag::Compound(vec![
                            (
                                "starts".into(),
                                Tag::Compound(vec![
                                    (
                                        "minecraft:buried_treasure".into(),
                                        Tag::Compound(vec![
                                            (
                                                "id".into(),
                                                Tag::String("minecraft:buried_treasure".into()),
                                            ),
                                            ("ChunkX".into(), Tag::Int(0)),
                                            ("ChunkZ".into(), Tag::Int(0)),
                                            ("references".into(), Tag::Int(0)),
                                            (
                                                "Children".into(),
                                                Tag::List {
                                                    element_type: TagType::Compound,
                                                    values: vec![Tag::Compound(vec![
                                                        (
                                                            "id".into(),
                                                            Tag::String("minecraft:btp".into()),
                                                        ),
                                                        (
                                                            "BB".into(),
                                                            Tag::IntArray(vec![
                                                                9,
                                                                lighting.min_y + 8,
                                                                9,
                                                                9,
                                                                lighting.min_y + 8,
                                                                9,
                                                            ]),
                                                        ),
                                                        ("O".into(), Tag::Int(-1)),
                                                        ("GD".into(), Tag::Int(0)),
                                                    ])],
                                                },
                                            ),
                                        ]),
                                    ),
                                    (
                                        "minecraft:swamp_hut".into(),
                                        Tag::Compound(vec![(
                                            "id".into(),
                                            Tag::String("INVALID".into()),
                                        )]),
                                    ),
                                    (
                                        "minecraft:ruined_portal".into(),
                                        Tag::Compound(vec![
                                            (
                                                "id".into(),
                                                Tag::String("minecraft:ruined_portal".into()),
                                            ),
                                            ("ChunkX".into(), Tag::Int(0)),
                                            ("ChunkZ".into(), Tag::Int(0)),
                                            ("references".into(), Tag::Int(0)),
                                            (
                                                "Children".into(),
                                                Tag::List {
                                                    element_type: TagType::Compound,
                                                    values: vec![portal.to_tag()?],
                                                },
                                            ),
                                        ]),
                                    ),
                                ]),
                            ),
                            (
                                "References".into(),
                                Tag::Compound(vec![
                                    (
                                        "minecraft:buried_treasure".into(),
                                        Tag::LongArray(vec![0, 1, 9]),
                                    ),
                                    ("example:unknown".into(), Tag::LongArray(vec![0])),
                                ]),
                            ),
                        ]),
                    ),
                    (
                        "sections".into(),
                        Tag::List {
                            element_type: TagType::Compound,
                            values: sections,
                        },
                    ),
                ]),
            };
            let mut region = FreshRegionDraft::new(FreshRegionKind::Terrain, 0, 0);
            region.add(ChunkPos { x: 0, z: 0 }, &saved, 1)?;
            let mut unknown = saved.clone();
            let Tag::Compound(fields) = &mut unknown.value else {
                unreachable!()
            };
            fields
                .iter_mut()
                .find(|(key, _)| key.matches("xPos"))
                .unwrap()
                .1 = Tag::Int(1);
            let Tag::List {
                values: sections, ..
            } = &mut fields
                .iter_mut()
                .find(|(key, _)| key.matches("sections"))
                .unwrap()
                .1
            else {
                unreachable!()
            };
            let Tag::Compound(section) = &mut sections[0] else {
                unreachable!()
            };
            let Tag::Compound(states) = &mut section
                .iter_mut()
                .find(|(key, _)| key.matches("block_states"))
                .unwrap()
                .1
            else {
                unreachable!()
            };
            states[0].1 = Tag::List {
                element_type: TagType::String,
                values: vec![Tag::String("example:unknown".into())],
            };
            region.add(ChunkPos { x: 1, z: 0 }, &unknown, 1)?;
            let mut clean = saved.clone();
            let Tag::Compound(fields) = &mut clean.value else {
                unreachable!()
            };
            fields
                .iter_mut()
                .find(|(key, _)| key.matches("xPos"))
                .unwrap()
                .1 = Tag::Int(3);
            let Tag::Compound(structures) = &mut fields
                .iter_mut()
                .find(|(key, _)| key.matches("structures"))
                .unwrap()
                .1
            else {
                unreachable!()
            };
            structures
                .iter_mut()
                .find(|(key, _)| key.matches("References"))
                .unwrap()
                .1 = Tag::Compound(vec![(
                "minecraft:buried_treasure".into(),
                Tag::LongArray(vec![0]),
            )]);
            region.add(ChunkPos { x: 3, z: 0 }, &clean, 1)?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .read(true)
                .create_new(true)
                .open(region_dir.join("r.0.0.mca"))?;
            region.write_staged(&mut file)?;
            // FreshRegionDraft deliberately writes only full chunks. Replace
            // this test fixture's entry with an uncompressed protochunk so the
            // reader encounters a real BIOMES-stage region record.
            let Tag::Compound(fields) = &mut clean.value else {
                unreachable!()
            };
            fields
                .iter_mut()
                .find(|(key, _)| key.matches("Status"))
                .unwrap()
                .1 = Tag::String("minecraft:biomes".into());
            let bytes = native_persistence::nbt::write(&clean)?;
            let offset = file.seek(SeekFrom::End(0))?;
            assert_eq!(offset % 4096, 0);
            let sectors = (bytes.len() + 5).div_ceil(4096);
            assert!(sectors < 256 && offset / 4096 < (1 << 24));
            file.write_all(&(u32::try_from(bytes.len() + 1)?).to_be_bytes())?;
            file.write_all(&[3])?;
            file.write_all(&bytes)?;
            file.write_all(&vec![0; sectors * 4096 - bytes.len() - 5])?;
            file.seek(SeekFrom::Start(3 * 4))?;
            file.write_all(&(((offset / 4096) as u32) << 8 | sectors as u32).to_be_bytes())?;
            // BIOMES-stage entries cover two neighboring terrain results and
            // their captured biome neighborhoods.
            for z in 0..=6 {
                for x in 0..=6 {
                    if z == 0 && matches!(x, 0 | 1 | 3) {
                        continue;
                    }
                    let mut biome_chunk = saved.clone();
                    let Tag::Compound(fields) = &mut biome_chunk.value else {
                        unreachable!()
                    };
                    for (key, value) in fields.iter_mut() {
                        if key.matches("xPos") {
                            *value = Tag::Int(x);
                        }
                        if key.matches("zPos") {
                            *value = Tag::Int(z);
                        }
                        if key.matches("Status") {
                            *value = Tag::String("minecraft:biomes".into());
                        }
                        if key.matches("structures") {
                            *value = Tag::Compound(vec![
                                ("starts".into(), Tag::Compound(vec![])),
                                ("References".into(), Tag::Compound(vec![])),
                            ]);
                        }
                        if key.matches("sections") {
                            let Tag::List { values, .. } = value else {
                                unreachable!()
                            };
                            for section in values {
                                let Tag::Compound(section) = section else {
                                    unreachable!()
                                };
                                for (key, value) in section.iter_mut() {
                                    if key.matches("block_states") {
                                        *value = Tag::Compound(vec![(
                                            "palette".into(),
                                            Tag::List {
                                                element_type: TagType::String,
                                                values: vec![Tag::String("minecraft:air".into())],
                                            },
                                        )]);
                                    }
                                }
                                section.push(("custom_section".into(), Tag::Int(91)));
                            }
                        }
                    }
                    fields.retain(|(key, _)| {
                        !key.matches("Heightmaps") && !key.matches("PostProcessing")
                    });
                    fields.push((
                        "Heightmaps".into(),
                        Tag::Compound(vec![("example:opaque".into(), Tag::Int(73))]),
                    ));
                    let empty_marks = native_core::postprocessing::PostProcessing::new(
                        lighting.min_y,
                        lighting.height,
                    )
                    .unwrap();
                    fields.push((
                        "PostProcessing".into(),
                        native_persistence::postprocessing::encode(&empty_marks),
                    ));
                    fields.push(("custom_chunk".into(), Tag::Int(42)));
                    let bytes = native_persistence::nbt::write(&biome_chunk)?;
                    let offset = file.seek(SeekFrom::End(0))?;
                    let sectors = (bytes.len() + 5).div_ceil(4096);
                    ensure!(
                        sectors < 256 && offset % 4096 == 0 && offset / 4096 < (1 << 24),
                        "invalid test region offset"
                    );
                    file.write_all(&(u32::try_from(bytes.len() + 1)?).to_be_bytes())?;
                    file.write_all(&[3])?;
                    file.write_all(&bytes)?;
                    file.write_all(&vec![0; sectors * 4096 - bytes.len() - 5])?;
                    file.seek(SeekFrom::Start(((x + z * 32) * 4) as u64))?;
                    file.write_all(
                        &(((offset / 4096) as u32) << 8 | sectors as u32).to_be_bytes(),
                    )?;
                }
            }
            file.flush()?;
            drop(file);
            let dimension = DimensionId::parse(name)?;
            let mut reader = WorldReader::open_for_session(root, dimension)?;
            let mut schedule = GenerationSchedule::new();
            schedule.request(
                native_world::loaded::ChunkPos { x: 0, z: 0 },
                native_session::worldgen::schedule::GenerationStage::Full,
            )?;
            let mut actual =
                prepare_existing_generator_for_schedule(&mut reader, &schedule, &game)?;
            assert_eq!(
                actual.inputs().sources().chunks().len(),
                schedule.requested_chunks()
            );
            assert!(actual.generate_structures);
            assert_eq!(actual.inputs().settings().seed(), -42);
            let position = ChunkPos { x: 0, z: 0 };
            let references = actual.restore_saved_structure_references(position)?;
            let treasure = ResourceId::parse("minecraft:buried_treasure")?;
            let starts = actual.restore_saved_structure_starts(position)?;
            let portal_id = ResourceId::parse("minecraft:ruined_portal")?;
            assert_eq!(
                starts[&portal_id]
                    .reference_bounds(&portal_id)?
                    .unwrap()
                    .bounds,
                portal.bounds
            );
            let bounds = starts[&treasure]
                .reference_bounds(&treasure)?
                .unwrap()
                .bounds;
            assert_eq!(bounds.min, [9, lighting.min_y + 8, 9]);
            assert!(
                starts[&ResourceId::parse("minecraft:swamp_hut")?]
                    .reference_bounds(&treasure)?
                    .is_none()
            );
            assert_eq!(
                references.references.membership()[&treasure],
                BTreeSet::from([0, 1])
            );
            assert_eq!(references.discarded.len(), 2);
            assert!(
                actual
                    .restore_saved_beardifier(position, std::slice::from_ref(&treasure))
                    .is_err()
            );
            let clean_target = ChunkPos { x: 3, z: 0 };
            let beardifier =
                actual.restore_saved_beardifier(clean_target, std::slice::from_ref(&treasure))?;
            assert!(beardifier.affected_bounds().is_none());
            assert!(actual.restore_saved_beardifier(clean_target, &[]).is_err());
            let previous = Arc::new(native_core::beardifier::Beardifier::new(
                vec![],
                vec![],
                None,
            ));
            let mut evaluator = native_resources::density::DensityEvaluator::default();
            evaluator.replace_structure_terrain_context(Some(Arc::clone(&previous)));
            assert!(
                actual
                    .generate_saved_structure_noise(
                        position,
                        std::slice::from_ref(&treasure),
                        &mut evaluator
                    )
                    .is_err()
            );
            assert!(
                actual
                    .generate_saved_structure_noise(clean_target, &[], &mut evaluator)
                    .is_err()
            );
            let result = actual.generate_saved_structure_noise(
                clean_target,
                std::slice::from_ref(&treasure),
                &mut evaluator,
            )?;
            assert!(Arc::ptr_eq(
                &previous,
                evaluator.structure_terrain_context().unwrap()
            ));
            let mut expected_evaluator = native_resources::density::DensityEvaluator::default();
            let expected = actual.generator.generate_noise_with_structure_context(
                &mut expected_evaluator,
                [3, 0],
                Arc::new(beardifier),
            )?;
            assert_eq!(result.chunk_position(), [3, 0]);
            assert_eq!(result.materials(), expected.materials());
            assert_eq!(result.fluid_updates(), expected.fluid_updates());
            let target = ChunkPos { x: 3, z: 3 };
            let decoration = actual.prepare_saved_decoration(target)?;
            let plan = decoration.plan(&actual)?;
            assert_eq!(plan.origin(), [48, lighting.min_y, 48]);
            assert!(
                plan.features()
                    .windows(2)
                    .all(|pair| { (pair[0].step, pair[0].index) < (pair[1].step, pair[1].index) })
            );
            let present = [
                ResourceId::parse("minecraft:plains")?,
                ResourceId::parse("minecraft:forest")?,
            ];
            assert_eq!(plan.features(), decoration.schedule.select(&present));
            if name == "overworld" {
                assert!(!plan.features().is_empty());
            }
            let mut called = false;
            actual.with_saved_prepared_decoration(&decoration, |_| {
                called = true;
                Ok(())
            })?;
            assert!(called, "{name}: prepared feature callback was not called");
            let second_plan = actual.prepare_saved_decoration(ChunkPos { x: 2, z: 3 })?;
            assert!(Arc::ptr_eq(&decoration.schedule, &second_plan.schedule));
            assert!(
                actual
                    .prepare_saved_decoration(ChunkPos { x: 100, z: 100 })
                    .is_err()
            );
            assert!(
                actual
                    .prepare_saved_decoration(ChunkPos { x: i32::MIN, z: 0 })
                    .is_err()
            );
            let mut restored_evaluator = native_resources::density::DensityEvaluator::default();
            restored_evaluator.replace_structure_terrain_context(Some(Arc::clone(&previous)));
            let resumed = actual.generate_saved_terrain(target, &[], &mut restored_evaluator)?;
            assert_eq!(resumed.chunk_position, [3, 3]);
            assert_eq!(resumed.states.len(), lighting.height as usize * 256);
            assert!(resumed.carver_postprocessing.is_some());
            let repeated = actual.generate_saved_terrain(target, &[], &mut restored_evaluator)?;
            assert_eq!(repeated.states, resumed.states, "{name} cached terrain");
            assert_eq!(
                repeated.fluid_updates, resumed.fluid_updates,
                "{name} cached fluid marks"
            );
            assert!(Arc::ptr_eq(
                &previous,
                restored_evaluator.structure_terrain_context().unwrap()
            ));
            assert!(
                actual
                    .generate_saved_terrain(position, &[], &mut restored_evaluator)
                    .is_err()
            );
            assert!(
                actual
                    .generate_saved_terrain(clean_target, &[], &mut restored_evaluator)
                    .is_err()
            );
            let first = actual.resume_saved_terrain(target, &[], &mut restored_evaluator)?;
            assert_eq!(
                first.materialized().fluid_postprocessing(),
                resumed.fluid_updates
            );
            assert_eq!(
                first.materialized().carver_postprocessing(),
                resumed.carver_postprocessing.as_ref()
            );
            let key = native_world::loaded::ChunkPos {
                x: target.x,
                z: target.z,
            };
            let materialized = first.materialized().world();
            assert_eq!(
                materialized.chunks[&key].status.as_deref(),
                Some("minecraft:terrain")
            );
            assert_eq!(
                materialized.biome(target.x * 16, lighting.min_y, target.z * 16),
                Some("minecraft:plains")
            );
            assert_eq!(
                materialized.biome(target.x * 16, lighting.min_y + 16, target.z * 16),
                Some("minecraft:forest")
            );
            let registry = actual.resources.vanilla_generated_block_registry()?;
            for [x, z] in [[0, 0], [7, 9], [15, 15]] {
                for y in [
                    lighting.min_y,
                    lighting.min_y + lighting.height / 2,
                    lighting.min_y + lighting.height - 1,
                ] {
                    let state = &resumed.states
                        [((z * 16 + x) * lighting.height + y - lighting.min_y) as usize];
                    let expected = resolve_generated_terrain_state(state, &registry)?;
                    assert_eq!(
                        materialized.block(target.x * 16 + x, y, target.z * 16 + z),
                        Some(&expected)
                    );
                }
            }
            let captured_source = Arc::clone(
                &actual.inputs().sources().chunks()[&target]
                    .terrain
                    .as_ref()
                    .unwrap()
                    .document,
            );
            let draft = actual.draft_resumed_sections(&first)?;
            assert!(draft.matches_source(&captured_source)?);
            assert_eq!(
                draft.document().value.get("Status"),
                Some(&Tag::String("minecraft:biomes".into()))
            );
            assert_eq!(
                draft.document().value.get("structures"),
                captured_source.value.get("structures")
            );
            assert_eq!(
                draft.document().value.get("custom_chunk"),
                Some(&Tag::Int(42))
            );
            let Tag::List {
                values: source_sections,
                ..
            } = captured_source.value.get("sections").unwrap()
            else {
                panic!()
            };
            let Tag::List {
                values: draft_sections,
                ..
            } = draft.document().value.get("sections").unwrap()
            else {
                panic!()
            };
            assert_eq!(source_sections.len(), draft_sections.len());
            for (old, new) in source_sections.iter().zip(draft_sections) {
                assert_eq!(old.get("biomes"), new.get("biomes"));
                assert_eq!(new.get("custom_section"), Some(&Tag::Int(91)));
            }
            let decoded = native_persistence::palette::decode_sections(&draft.document().value)?;
            let bottom = decoded
                .iter()
                .find(|section| i32::from(section.y) == lighting.min_y / 16)
                .unwrap();
            let expected = materialized
                .block(target.x * 16, lighting.min_y, target.z * 16)
                .unwrap();
            assert!(
                bottom
                    .blocks
                    .as_ref()
                    .unwrap()
                    .block(0, 0, 0)
                    .unwrap()
                    .name
                    .matches(&expected.name)
            );
            let mut stale = (*captured_source).clone();
            let Tag::Compound(fields) = &mut stale.value else {
                unreachable!()
            };
            fields.push(("changed".into(), Tag::Byte(1)));
            assert!(!draft.matches_source(&stale)?);
            let mut resumed_world = first.into_world();
            assert!(Arc::ptr_eq(
                &captured_source,
                &resumed_world.sources()[&target]
            ));
            let second_target = ChunkPos { x: 2, z: 3 };
            let second =
                actual.resume_saved_terrain(second_target, &[], &mut restored_evaluator)?;
            resumed_world.insert(&second)?;
            assert!(resumed_world.insert(&second).is_err());
            assert_eq!(resumed_world.sources().len(), 2);
            assert_eq!(resumed_world.terrain().snapshot().chunks.len(), 2);
            if name == "overworld" {
                let before = resumed_world.terrain().snapshot();
                let error = actual
                    .run_saved_features(
                        &mut resumed_world,
                        &decoration,
                        &mut GenerationSchedule::new(),
                    )
                    .expect_err("an incomplete schedule must not run features");
                assert!(format!("{error:#}").contains("features dependencies"));
                assert!(Arc::ptr_eq(&before, &resumed_world.terrain().snapshot()));
            }
            let metadata = actual.draft_resumed_metadata(&mut resumed_world, &draft)?;
            let input = root.join("resumed-terrain.nbt");
            let output = root.join("java-resumed-terrain.nbt");
            std::fs::write(&input, native_persistence::nbt::write(metadata.document())?)?;
            let checked = std::process::Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&classpath)
                .arg("ResumedTerrainOracle")
                .arg(&input)
                .arg(lighting.min_y.to_string())
                .arg(lighting.height.to_string())
                .arg(&output)
                .output()?;
            ensure!(
                checked.status.success(),
                "{name} Java terrain validation failed: {}\n{}",
                String::from_utf8_lossy(&checked.stdout),
                String::from_utf8_lossy(&checked.stderr)
            );
            let java_document = native_persistence::nbt::read(&std::fs::read(output)?)?;
            let expected =
                native_persistence::palette::decode_sections(&metadata.document().value)?;
            let rewritten = native_persistence::palette::decode_sections(&java_document.value)?;
            assert_eq!(expected.len(), rewritten.len(), "{name}: section count");
            for source_section in expected {
                let java_section = rewritten
                    .iter()
                    .find(|section| section.y == source_section.y)
                    .context("Java section missing")?;
                let source_blocks = source_section.blocks.context("source blocks missing")?;
                let java_blocks = java_section
                    .blocks
                    .as_ref()
                    .context("Java blocks missing")?;
                for y in 0..16 {
                    for z in 0..16 {
                        for x in 0..16 {
                            // Java may omit properties equal to their default.
                            // The oracle compares the complete decoded states.
                            assert_eq!(
                                source_blocks.block(x, y, z).map(|state| &state.name),
                                java_blocks.block(x, y, z).map(|state| &state.name),
                                "{name}: block at section {}, {x}, {y}, {z}",
                                source_section.y
                            );
                        }
                    }
                }
                let source_biomes = source_section.biomes.context("source biomes missing")?;
                let java_biomes = java_section
                    .biomes
                    .as_ref()
                    .context("Java biomes missing")?;
                for y in 0..4 {
                    for z in 0..4 {
                        for x in 0..4 {
                            assert_eq!(
                                source_biomes.biome(x, y, z),
                                java_biomes.biome(x, y, z),
                                "{name}: biome at section {}, {x}, {y}, {z}",
                                source_section.y
                            );
                        }
                    }
                }
            }
            assert!(metadata.matches_source(&captured_source)?);
            assert!(!metadata.matches_source(&stale)?);
            assert_eq!(
                metadata.document().value.get("Status"),
                Some(&Tag::String("minecraft:biomes".into()))
            );
            assert_eq!(
                metadata.document().value.get("structures"),
                captured_source.value.get("structures")
            );
            let heightmap_tag = metadata.document().value.get("Heightmaps").unwrap();
            assert_eq!(heightmap_tag.get("example:opaque"), Some(&Tag::Int(73)));
            assert!(heightmap_tag.get("WORLD_SURFACE").is_none());
            let heightmaps = native_persistence::heightmap::SavedGenerationHeightmaps::read(
                metadata.document(),
                native_persistence::heightmap::HeightBounds {
                    min_y: lighting.min_y,
                    max_y: lighting.min_y + lighting.height,
                },
            )?;
            let (surface, ocean) = resumed_world.terrain().worldgen_heightmaps(key).unwrap();
            for z in 0..16 {
                for x in 0..16 {
                    let index = (x + z * 16) as usize;
                    assert_eq!(heightmaps.first_available(native_persistence::heightmap::SavedGenerationHeightmapKind::WorldSurfaceWorldgen, x, z), Some(surface[index]));
                    assert_eq!(heightmaps.first_available(native_persistence::heightmap::SavedGenerationHeightmapKind::OceanFloorWorldgen, x, z), Some(ocean[index]));
                }
            }
            assert_eq!(
                metadata.document().value.get("PostProcessing"),
                Some(&native_persistence::postprocessing::encode(
                    resumed.carver_postprocessing.as_ref().unwrap()
                ))
            );
            let repeated = actual.draft_resumed_metadata(&mut resumed_world, &draft)?;
            assert_eq!(metadata.document(), repeated.document());
            if name == "overworld" {
                let positions = (2..=4).flat_map(|z| (3..=5).map(move |x| ChunkPos { x, z }));
                let context = reader.capture_generation_context(positions)?;
                let mut other = prepare_existing_generator(context, &game)?;
                assert!(decoration.plan(&other).is_err());
                let mut called = false;
                assert!(
                    other
                        .with_saved_prepared_decoration(&decoration, |_| {
                            called = true;
                            Ok(())
                        })
                        .is_err()
                );
                assert!(!called);
                let alien = other.resume_saved_terrain(
                    ChunkPos { x: 4, z: 3 },
                    &[],
                    &mut restored_evaluator,
                )?;
                assert!(actual.draft_resumed_sections(&alien).is_err());
                let alien_draft = other.draft_resumed_sections(&alien)?;
                assert!(
                    actual
                        .draft_resumed_metadata(&mut resumed_world, &alien_draft)
                        .is_err()
                );
                assert!(resumed_world.insert(&alien).is_err());
                assert_eq!(resumed_world.sources().len(), 2);
            }
            let restored = actual.restore_saved_biomes([position])?;
            let chunk = &restored[&native_world::loaded::ChunkPos { x: 0, z: 0 }];
            let mut neighborhood = actual.generator.biome_neighborhood()?;
            neighborhood.insert(Arc::clone(chunk))?;
            assert_eq!(
                neighborhood
                    .noise_biome_at_block(0, lighting.min_y, 0)?
                    .path,
                "plains"
            );
            let quart_height = (lighting.height / 4) as usize;
            for xz in 0..16 {
                for y in 0..quart_height {
                    let expected = if (y / 4) % 2 == 0 { "plains" } else { "forest" };
                    assert_eq!(
                        chunk.values()[xz * quart_height + y].path,
                        expected,
                        "{name}"
                    );
                }
            }
            assert!(actual.restore_saved_biomes([position, position]).is_err());
            assert!(
                actual
                    .restore_saved_biomes([ChunkPos { x: 8, z: 0 }])
                    .is_err()
            );
            assert!(
                actual
                    .restore_saved_biomes([ChunkPos { x: 100, z: 100 }])
                    .is_err()
            );
            let terrain = actual.restore_saved_terrain([position])?;
            let key = native_world::loaded::ChunkPos { x: 0, z: 0 };
            assert_eq!(
                terrain.world.block(0, lighting.min_y, 0).unwrap().name,
                "minecraft:stone"
            );
            assert_eq!(terrain.biomes[&key].values(), chunk.values());
            assert_eq!(terrain.heightmaps[&key].first_available(native_persistence::heightmap::SavedGenerationHeightmapKind::WorldSurfaceWorldgen, 0, 0), Some(lighting.min_y + 7));
            assert_eq!(
                terrain.heightmaps[&key].first_available(
                    native_persistence::heightmap::SavedGenerationHeightmapKind::WorldSurface,
                    0,
                    0
                ),
                None
            );
            assert_eq!(terrain.postprocessing[&key].as_ref().unwrap(), &marks);
            assert!(
                actual
                    .restore_saved_terrain([position, ChunkPos { x: 1, z: 0 }])
                    .is_err()
            );
            let player = native_session::PlayerState {
                position: glam::Vec3::new(8.5, lighting.min_y as f32 + 2.0, 8.5),
                yaw: 0.0,
                pitch: 0.0,
                vertical_velocity: 0.0,
                on_ground: false,
                pose: native_session::PlayerPose::Standing,
            };
            let mut live = native_session::loaded::LoadedSession::new(
                terrain.world().clone(),
                glam::IVec3::ZERO,
                native_session::loaded::BuildBounds {
                    min_y: lighting.min_y,
                    max_y: lighting.min_y + lighting.height,
                },
                player,
            )
            .map_err(anyhow::Error::msg)?;
            assert!(actual.capture_live_terrain(&live, &terrain).is_err());
            let context = reader.capture_generation_context([position])?;
            let mut narrow = prepare_existing_generator(context, &game)?;
            let narrow_terrain = narrow.restore_saved_terrain([position])?;
            assert!(narrow.capture_live_terrain(&live, &terrain).is_err());
            let guard = narrow.capture_live_terrain(&live, &narrow_terrain)?;
            live.verify_generation_terrain(&guard)?;
            live.queue_streamed_chunk(key, None, None, None, None, None);
            assert!(live.verify_generation_terrain(&guard).is_err());
            let mut plan = WorldSavePlan::new();
            actual.add_save_dependencies(&mut plan)?;
            assert!(actual.add_save_dependencies(&mut plan).is_err());
            let mut reference_resources = ResourceStack::open(&game)?;
            let (lighting, definition, dimension_definition) =
                GeneratedDimensionLighting::load_preset_with_definition(
                    &mut reference_resources,
                    "minecraft:normal",
                    &format!("minecraft:{name}"),
                )?;
            assert_eq!(actual.lighting, lighting);
            assert_eq!(actual.dimension_definition, dimension_definition);
            let mut reference = TerrainGenerator::with_structure_context(
                &mut reference_resources,
                &definition,
                -42,
                lighting.min_y,
                lighting.height,
            )?;
            let positions = (2..=4).flat_map(|z| (2..=4).map(move |x| ChunkPos { x, z }));
            let saved_neighbors = actual.restore_saved_biomes(positions)?;
            let mut expected_biomes = reference.biome_neighborhood()?;
            for biome in saved_neighbors.into_values() {
                expected_biomes.insert(biome)?;
            }
            let blocks = reference_resources.vanilla_block_ids()?;
            let tags = CarverBlockTags::load(&mut reference_resources, &blocks)?;
            let registry = reference_resources.vanilla_generated_block_registry()?;
            let classify = |value: &Value| -> Result<CarverBlockState> {
                let state = resolve_generated_terrain_state(value, &registry)?;
                Ok(CarverBlockState {
                    is_air: matches!(
                        state.name.as_str(),
                        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                    ),
                    has_fluid: FluidState::from_block(&state)
                        .map_err(anyhow::Error::msg)?
                        .is_some(),
                })
            };
            let structure = Arc::new(actual.restore_saved_beardifier(target, &[])?);
            let expected_terrain = reference.generate_carved_surface_with_structure_context(
                &mut reference_resources,
                &mut native_resources::density::DensityEvaluator::default(),
                &mut native_resources::carver::schedule::CarverCache::default(),
                [target.x, target.z],
                &expected_biomes,
                &tags,
                structure,
                classify,
            )?;
            assert_eq!(
                resumed.states, expected_terrain.states,
                "{name} saved terrain"
            );
            assert_eq!(
                resumed.fluid_updates, expected_terrain.fluid_updates,
                "{name} saved fluid marks"
            );
            assert_eq!(
                resumed.carver_postprocessing, expected_terrain.carver_postprocessing,
                "{name} saved postprocessing"
            );
            assert_eq!(
                actual.generator.sea_level(),
                reference.sea_level(),
                "{name}"
            );
            assert_eq!(
                actual.generator.possible_biomes(),
                reference.possible_biomes(),
                "{name}"
            );
            let mut a = native_resources::density::DensityEvaluator::default();
            let mut b = native_resources::density::DensityEvaluator::default();
            assert_eq!(
                actual.generator.generate_biomes([0, 0], &mut a)?.values(),
                reference.generate_biomes([0, 0], &mut b)?.values(),
                "{name}"
            );
            assert!(actual.resources.data_pack_snapshot().is_some());
        }
        let no_structures = draft_baked_worldgen_settings(
            &mut source,
            "minecraft:normal",
            NewWorldOptions {
                seed: -42,
                generate_structures: false,
                bonus_chest: false,
            },
        )?;
        std::fs::write(
            root.join("data/minecraft/world_gen_settings.dat"),
            native_persistence::nbt::write(&no_structures)?,
        )?;
        let pack = root.join("datapacks/custom");
        let dimensions = pack.join("data/minecraft/dimension");
        std::fs::create_dir_all(&dimensions)?;
        let biome_file = pack.join("data/minecraft/worldgen/biome/plains.json");
        std::fs::create_dir_all(biome_file.parent().unwrap())?;
        let mut custom_biome: Value = serde_json::from_slice(
            &source
                .read_data("data/minecraft/worldgen/biome/plains.json")?
                .context("vanilla plains biome missing")?,
        )?;
        custom_biome["features"] = json!([[{
            "feature": {"type":"minecraft:simple_block", "to_place":{"id":"minecraft:melon"}, "schedule_tick":true},
            "placement": [{"type":"minecraft:offset", "x":8, "y":8, "z":8},
                {"type":"minecraft:block_predicate_filter", "predicate":{"type":"minecraft:not", "predicate":{"type":"minecraft:would_survive", "state":"minecraft:brown_mushroom"}}}]
        }]]);
        std::fs::write(&biome_file, serde_json::to_vec(&custom_biome)?)?;
        std::fs::write(
            pack.join("pack.mcmeta"),
            br#"{"pack":{"description":"test","min_format":121,"max_format":121}}"#,
        )?;
        let dimension_file = dimensions.join("overworld.json");
        let custom_stem = |biome: &str| {
            json!({
                "type":"minecraft:overworld",
                "generator":{"type":"minecraft:noise","settings":"minecraft:overworld",
                    "biome_source":{"type":"minecraft:fixed","biome":biome}}
            })
        };
        std::fs::write(
            &dimension_file,
            serde_json::to_vec(&custom_stem("minecraft:plains"))?,
        )?;
        let mut reader = WorldReader::open_for_session(root, DimensionId::parse("overworld")?)?;
        let mut updated_level = level.clone();
        let Tag::Compound(root_fields) = &mut updated_level.value else {
            unreachable!()
        };
        let Tag::Compound(data) = &mut root_fields
            .iter_mut()
            .find(|(name, _)| name.matches("Data"))
            .unwrap()
            .1
        else {
            unreachable!()
        };
        data.push((
            "DataPacks".into(),
            Tag::Compound(vec![
                (
                    "Enabled".into(),
                    Tag::List {
                        element_type: TagType::String,
                        values: vec![
                            Tag::String("vanilla".into()),
                            Tag::String("file/custom".into()),
                        ],
                    },
                ),
                (
                    "Disabled".into(),
                    Tag::List {
                        element_type: TagType::End,
                        values: vec![],
                    },
                ),
            ]),
        ));
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&native_persistence::nbt::write(&updated_level)?)?;
        std::fs::write(root.join("level.dat"), gzip.finish()?)?;
        let positions = (0..=6).flat_map(|z| (0..=6).map(move |x| ChunkPos { x, z }));
        let context = reader.capture_generation_context(positions)?;
        assert_eq!(context.selection().enabled, ["vanilla", "file/custom"]);
        std::fs::write(
            &dimension_file,
            serde_json::to_vec(&custom_stem("minecraft:desert"))?,
        )?;
        custom_biome["features"] = json!([[{
            "feature": {"type":"example:unsupported"}, "placement": []
        }]]);
        std::fs::write(&biome_file, serde_json::to_vec(&custom_biome)?)?;
        let mut actual = prepare_existing_generator(context, &game)?;
        assert!(!actual.generate_structures);
        assert_eq!(
            actual.generator.possible_biomes(),
            &BTreeSet::from([ResourceId::parse("minecraft:plains")?])
        );
        let custom_decoration = actual.prepare_saved_decoration(ChunkPos { x: 3, z: 3 })?;
        let plan = custom_decoration.plan(&actual)?;
        assert_eq!(plan.features().len(), 1);
        let mut prepared = false;
        actual.with_saved_prepared_decoration(&custom_decoration, |_| {
            prepared = true;
            Ok(())
        })?;
        assert!(prepared, "captured data-pack feature was not prepared");
        let saved_center = ChunkPos { x: 3, z: 3 };
        let center = native_world::loaded::ChunkPos { x: 3, z: 3 };
        let mut schedule = GenerationSchedule::new();
        schedule.request(
            center,
            native_session::worldgen::schedule::GenerationStage::Features,
        )?;
        let first = actual.resume_saved_terrain(
            saved_center,
            &[],
            &mut native_resources::density::DensityEvaluator::default(),
        )?;
        let draft = actual.draft_resumed_sections(&first)?;
        let mut terrain = first.into_world();
        let empty_light = native_world::generation_light::VisibleLightSnapshot::new(true, true);
        let stale_light = actual.bind_initial_feature_light(&terrain, empty_light)?;
        assert!(
            actual
                .run_saved_features(&mut terrain, &custom_decoration, &mut schedule)
                .is_err()
        );
        let restored = schedule
            .requirements()
            .map(|(position, _)| {
                let stage =
                    if (position.x - center.x).abs() <= 1 && (position.z - center.z).abs() <= 1 {
                        native_session::worldgen::schedule::GenerationStage::Terrain
                    } else {
                        native_session::worldgen::schedule::GenerationStage::StructureStarts
                    };
                (position, stage)
            })
            .collect::<Vec<_>>();
        schedule.restore_imported(restored)?;
        let before = terrain.terrain().snapshot();
        assert!(
            actual
                .run_saved_features(&mut terrain, &custom_decoration, &mut schedule)
                .is_err()
        );
        assert!(Arc::ptr_eq(&before, &terrain.terrain().snapshot()));
        assert_eq!(
            schedule.completed(center),
            Some(native_session::worldgen::schedule::GenerationStage::Terrain)
        );
        let mut terrain_evaluator = native_resources::density::DensityEvaluator::default();
        for z in 2..=4 {
            for x in 2..=4 {
                if x == 3 && z == 3 {
                    continue;
                }
                let next =
                    actual.resume_saved_terrain(ChunkPos { x, z }, &[], &mut terrain_evaluator)?;
                terrain.insert(&next)?;
            }
        }
        let y = actual.lighting.min_y + 8;
        let before_light = terrain.terrain().snapshot();
        assert!(!stale_light.matches_world(&terrain));
        assert!(
            actual
                .run_saved_features_with_initial_light(
                    &mut terrain,
                    &custom_decoration,
                    &mut schedule,
                    &stale_light
                )
                .is_err()
        );
        assert!(
            actual
                .run_saved_features(&mut terrain, &custom_decoration, &mut schedule)
                .is_err()
        );
        assert!(Arc::ptr_eq(&before_light, &terrain.terrain().snapshot()));
        let empty_light = actual.bind_initial_feature_light(
            &terrain,
            native_world::generation_light::VisibleLightSnapshot::new(true, true),
        )?;
        assert!(
            actual
                .run_saved_features_with_initial_light(
                    &mut terrain,
                    &custom_decoration,
                    &mut schedule,
                    &empty_light
                )
                .is_err()
        );
        assert!(Arc::ptr_eq(&before_light, &terrain.terrain().snapshot()));
        assert!(
            actual
                .bind_initial_feature_light(
                    &terrain,
                    native_world::generation_light::VisibleLightSnapshot::new(false, true)
                )
                .is_err()
        );
        let columns =
            (2..=4).flat_map(|z| (2..=4).map(move |x| native_world::loaded::ChunkPos { x, z }));
        let light =
            native_world::generation_light::VisibleLightSnapshot::fresh_after_section_updates(
                true,
                true,
                [],
                columns,
            )
            .context("light snapshot unavailable")?;
        let light = actual.bind_initial_feature_light(&terrain, light)?;
        let before_block = terrain
            .terrain()
            .snapshot()
            .block(56, y, 56)
            .context("feature block missing")?
            .name
            .clone();
        assert_ne!(before_block, "minecraft:melon");
        assert_eq!(
            actual.run_saved_features_with_initial_light(
                &mut terrain,
                &custom_decoration,
                &mut schedule,
                &light
            )?,
            vec![true]
        );
        assert!(!light.matches_world(&terrain));
        assert!(terrain.feature_entity_sources.completed.contains(&center));
        assert!(!terrain.feature_entity_sources.unverified);
        assert!(!terrain.feature_entity_sources.covers(center));
        assert_eq!(
            terrain
                .terrain()
                .snapshot()
                .block(56, y, 56)
                .context("feature block missing")?
                .name,
            "minecraft:melon"
        );
        assert_eq!(
            schedule.completed(center),
            Some(native_session::worldgen::schedule::GenerationStage::Features)
        );
        assert!(actual.draft_resumed_metadata(&mut terrain, &draft).is_err());
        assert!(
            actual
                .run_saved_features(&mut terrain, &custom_decoration, &mut schedule)
                .is_err()
        );
        let no_structures_snapshot = terrain.terrain().snapshot();
        // Complete the 3x3 feature neighborhood before registering its light
        // inputs; the outer 5x5 terrain and 7x7 biomes are captured above.
        for z in 1..=5 {
            for x in 1..=5 {
                if (2..=4).contains(&x) && (2..=4).contains(&z) {
                    continue;
                }
                let chunk =
                    actual.resume_saved_terrain(ChunkPos { x, z }, &[], &mut terrain_evaluator)?;
                terrain.insert(&chunk)?;
            }
        }
        use native_session::worldgen::schedule::GenerationStage;
        schedule.request(center, GenerationStage::Light)?;
        let imported = schedule
            .requirements()
            .map(|(pos, _)| {
                let current = schedule.completed(pos).unwrap();
                let stage = if (1..=5).contains(&pos.x) && (1..=5).contains(&pos.z) {
                    current.max(GenerationStage::Terrain)
                } else {
                    current.max(GenerationStage::StructureStarts)
                };
                (pos, stage)
            })
            .collect::<Vec<_>>();
        schedule.restore_imported(imported)?;
        for z in 2..=4 {
            for x in 2..=4 {
                if x == 3 && z == 3 {
                    continue;
                }
                let decoration = actual.prepare_saved_decoration(ChunkPos { x, z })?;
                let columns = (z - 1..=z + 1).flat_map(|cz| {
                    (x - 1..=x + 1).map(move |cx| native_world::loaded::ChunkPos { x: cx, z: cz })
                });
                let visible = native_world::generation_light::VisibleLightSnapshot::fresh_after_section_updates(true, true, [], columns).context("light snapshot unavailable")?;
                let visible = actual.bind_initial_feature_light(&terrain, visible)?;
                actual.run_saved_features_with_initial_light(
                    &mut terrain,
                    &decoration,
                    &mut schedule,
                    &visible,
                )?;
            }
        }
        let mut initial = actual.begin_resumed_initial_light(&terrain)?;
        for z in 2..=4 {
            for x in 2..=4 {
                actual.initialize_resumed_light(
                    &terrain,
                    &mut schedule,
                    &mut initial,
                    native_world::loaded::ChunkPos { x, z },
                )?;
            }
        }
        let mut staged = actual.seal_resumed_light(&terrain, initial)?;
        let visible =
            actual.propagate_resumed_light(&terrain, &mut schedule, &mut staged, center)?;
        assert!(visible.raw_brightness([56, y, 56], 0).is_some());
        assert_eq!(schedule.completed(center), Some(GenerationStage::Light));
        let lit = actual.draft_resumed_lit_chunk(&terrain, &staged, saved_center)?;
        assert!(lit.matches_state(&terrain, &staged));
        assert!(lit.matches_source(&terrain.sources[&saved_center])?);
        assert_eq!(
            lit.document().value.get("Status"),
            Some(&Tag::String("minecraft:biomes".into()))
        );
        let light_revision = Arc::clone(&staged.light_revision);
        staged.light_revision = Arc::new(());
        assert!(!lit.matches_state(&terrain, &staged));
        staged.light_revision = light_revision;
        let light_file = root.join("resumed-light.nbt");
        std::fs::write(&light_file, native_persistence::nbt::write(lit.document())?)?;
        let checked = std::process::Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("ResumedLightOracle")
            .arg(&light_file)
            .arg(actual.lighting.min_y.to_string())
            .arg(actual.lighting.height.to_string())
            .output()?;
        ensure!(
            checked.status.success(),
            "Java light validation failed: {}\n{}",
            String::from_utf8_lossy(&checked.stdout),
            String::from_utf8_lossy(&checked.stderr)
        );
        assert!(
            actual
                .propagate_resumed_light(&terrain, &mut schedule, &mut staged, center)
                .is_err()
        );
        assert_eq!(actual.generator.disable_mob_generation(), Some(false));
        assert!(
            actual
                .resume_spawn_without_mobs(&terrain, &mut schedule, &mut staged, center, false)
                .is_err()
        );
        assert_eq!(schedule.completed(center), Some(GenerationStage::Light));
        schedule.request(center, GenerationStage::Spawn)?;
        let imported = schedule
            .requirements()
            .map(|(pos, _)| {
                let current = schedule.completed(pos).unwrap();
                let distance = (pos.x - center.x).abs().max((pos.z - center.z).abs());
                (
                    pos,
                    if distance == 3 {
                        current.max(GenerationStage::Biomes)
                    } else {
                        current
                    },
                )
            })
            .collect::<Vec<_>>();
        schedule.restore_imported(imported)?;
        assert!(
            actual
                .resume_spawn_without_mobs(&terrain, &mut schedule, &mut staged, center, true)
                .is_err()
        );
        assert_eq!(schedule.completed(center), Some(GenerationStage::Light));
        actual.resume_spawn_without_mobs(&terrain, &mut schedule, &mut staged, center, false)?;
        assert_eq!(schedule.completed(center), Some(GenerationStage::Spawn));
        let spawn = actual.draft_resumed_empty_spawn(&terrain, &staged, saved_center, false)?;
        assert!(spawn.matches_state(&terrain, &staged, false));
        assert!(!spawn.matches_state(&terrain, &staged, true));
        assert!(spawn.matches_source(&terrain.sources[&saved_center])?);
        assert_eq!(spawn.document(), lit.document());
        assert!(
            actual
                .draft_resumed_empty_spawn(&terrain, &staged, saved_center, true)
                .is_err()
        );
        assert!(
            actual
                .resume_spawn_without_mobs(&terrain, &mut schedule, &mut staged, center, false)
                .is_err()
        );
        let previous = Arc::new(native_core::beardifier::Beardifier::new(
            vec![],
            vec![],
            None,
        ));
        let mut evaluator = native_resources::density::DensityEvaluator::default();
        evaluator.replace_structure_terrain_context(Some(Arc::clone(&previous)));
        let resumed =
            actual.generate_saved_terrain(ChunkPos { x: 3, z: 3 }, &[], &mut evaluator)?;
        assert_eq!(resumed.chunk_position, [3, 3]);
        assert!(resumed.carver_postprocessing.is_some());
        assert!(Arc::ptr_eq(
            &previous,
            evaluator.structure_terrain_context().unwrap()
        ));
        assert!(
            actual
                .generate_saved_terrain(
                    ChunkPos { x: 3, z: 3 },
                    &[ResourceId::parse("minecraft:buried_treasure")?],
                    &mut evaluator
                )
                .is_err()
        );
        std::fs::write(
            &dimension_file,
            serde_json::to_vec(&custom_stem("minecraft:plains"))?,
        )?;
        let positions = (2..=4).flat_map(|z| (2..=4).map(move |x| ChunkPos { x, z }));
        let context = reader.capture_generation_context(positions)?;
        let mut unsupported = prepare_existing_generator(context, &game)?;
        let decoration = unsupported.prepare_saved_decoration(ChunkPos { x: 3, z: 3 })?;
        let mut called = false;
        let error = unsupported
            .with_saved_prepared_decoration(&decoration, |_| {
                called = true;
                Ok(())
            })
            .expect_err("unsupported feature unexpectedly prepared");
        assert!(
            format!("{error:#}").contains("example:unsupported"),
            "{error:#}"
        );
        assert!(
            !called,
            "unsupported feature reached the placement callback"
        );
        // A structure-enabled world with no references in the center still
        // runs its feature pass with an explicitly empty structure plan.
        std::fs::write(
            root.join("data/minecraft/world_gen_settings.dat"),
            native_persistence::nbt::write(&worldgen)?,
        )?;
        custom_biome["features"] = json!([[{
            "feature": {"type":"minecraft:simple_block", "to_place":{"id":"minecraft:melon"}, "schedule_tick":true},
            "placement": [{"type":"minecraft:offset", "x":8, "y":8, "z":8},
                {"type":"minecraft:block_predicate_filter", "predicate":{"type":"minecraft:not", "predicate":{"type":"minecraft:would_survive", "state":"minecraft:brown_mushroom"}}}]
        }]]);
        std::fs::write(&biome_file, serde_json::to_vec(&custom_biome)?)?;
        let positions = (1..=5).flat_map(|z| (1..=5).map(move |x| ChunkPos { x, z }));
        let context = reader.capture_generation_context(positions)?;
        let mut structured = prepare_existing_generator(context, &game)?;
        assert!(structured.generate_structures);
        let decoration = structured.prepare_saved_decoration(saved_center)?;
        let first = structured.resume_saved_terrain(
            saved_center,
            &[],
            &mut native_resources::density::DensityEvaluator::default(),
        )?;
        let mut terrain = first.into_world();
        let mut evaluator = native_resources::density::DensityEvaluator::default();
        for z in 2..=4 {
            for x in 2..=4 {
                if x == 3 && z == 3 {
                    continue;
                }
                let next =
                    structured.resume_saved_terrain(ChunkPos { x, z }, &[], &mut evaluator)?;
                terrain.insert(&next)?;
            }
        }
        let mut schedule = GenerationSchedule::new();
        schedule.request(center, GenerationStage::Features)?;
        let restored = schedule
            .requirements()
            .map(|(position, _)| {
                let stage =
                    if (position.x - center.x).abs() <= 1 && (position.z - center.z).abs() <= 1 {
                        GenerationStage::Terrain
                    } else {
                        GenerationStage::StructureStarts
                    };
                (position, stage)
            })
            .collect::<Vec<_>>();
        schedule.restore_imported(restored)?;
        let columns =
            (2..=4).flat_map(|z| (2..=4).map(move |x| native_world::loaded::ChunkPos { x, z }));
        let light =
            native_world::generation_light::VisibleLightSnapshot::fresh_after_section_updates(
                true,
                true,
                [],
                columns,
            )
            .context("light snapshot unavailable")?;
        let light = structured.bind_initial_feature_light(&terrain, light)?;
        assert_eq!(
            structured.run_saved_features_with_initial_light(
                &mut terrain,
                &decoration,
                &mut schedule,
                &light
            )?,
            vec![true]
        );
        assert_eq!(
            terrain
                .terrain()
                .snapshot()
                .block(56, structured.lighting.min_y + 8, 56)
                .context("feature block missing")?
                .name,
            "minecraft:melon"
        );
        assert_eq!(schedule.completed(center), Some(GenerationStage::Features));
        let absent_sidecar = structured.draft_resumed_feature_chunk(&terrain, saved_center)?;
        assert!(absent_sidecar.entity_sidecar_source().is_none());
        assert!(absent_sidecar.matches_entity_sidecar(None)?);
        assert!(absent_sidecar.document().value.get("entities").is_none());
        let structured_snapshot = terrain.terrain().snapshot();
        for y in structured.lighting.min_y..structured.lighting.min_y + structured.lighting.height {
            for z in 32..80 {
                for x in 32..80 {
                    assert_eq!(
                        structured_snapshot.block(x, y, z),
                        no_structures_snapshot.block(x, y, z),
                        "empty structure plan changed block {x}, {y}, {z}"
                    );
                }
            }
        }
        // Replace one entry in the disposable region with a saved treasure
        // start and its self-reference. The original world is never opened.
        let treasure_id = ResourceId::parse("minecraft:buried_treasure")?;
        let original_start = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:buried_treasure".into())),
            ("ChunkX".into(), Tag::Int(3)),
            ("ChunkZ".into(), Tag::Int(3)),
            ("references".into(), Tag::Int(2)),
            ("custom_start".into(), Tag::Int(73)),
            (
                "Children".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![Tag::Compound(vec![
                        ("id".into(), Tag::String("minecraft:btp".into())),
                        ("BB".into(), Tag::IntArray(vec![57, 90, 57, 57, 90, 57])),
                        ("O".into(), Tag::Int(-1)),
                        ("GD".into(), Tag::Int(0)),
                    ])],
                },
            ),
        ]);
        let original_document = &structured.context.inputs().sources().chunks()[&saved_center]
            .terrain
            .as_ref()
            .context("saved center missing")?
            .document;
        let mut treasure_document = (**original_document).clone();
        let original_proto = Tag::Compound(vec![
            ("n".into(), Tag::Int(11)),
            ("opaque".into(), Tag::String("keep".into())),
        ]);
        replace_unique_root_field(
            &mut treasure_document,
            "entities",
            Tag::List {
                element_type: TagType::Compound,
                values: vec![original_proto.clone()],
            },
        )?;
        replace_unique_root_field(
            &mut treasure_document,
            "structures",
            Tag::Compound(vec![
                (
                    "starts".into(),
                    Tag::Compound(vec![(
                        "minecraft:buried_treasure".into(),
                        original_start.clone(),
                    )]),
                ),
                (
                    "References".into(),
                    Tag::Compound(vec![(
                        "minecraft:buried_treasure".into(),
                        Tag::LongArray(vec![3 | (3i64 << 32)]),
                    )]),
                ),
            ]),
        )?;
        let encoded = native_persistence::nbt::write(&treasure_document)?;
        let region = root.join("dimensions/minecraft/overworld/region/r.0.0.mca");
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(region)?;
        let offset = file.seek(SeekFrom::End(0))?;
        let sectors = (encoded.len() + 5).div_ceil(4096);
        ensure!(
            offset % 4096 == 0 && sectors < 256 && offset / 4096 < (1 << 24),
            "invalid test region entry"
        );
        file.write_all(&(u32::try_from(encoded.len() + 1)?).to_be_bytes())?;
        file.write_all(&[3])?;
        file.write_all(&encoded)?;
        file.write_all(&vec![0; sectors * 4096 - encoded.len() - 5])?;
        file.seek(SeekFrom::Start((3 + 3 * 32) * 4))?;
        file.write_all(&(((offset / 4096) as u32) << 8 | sectors as u32).to_be_bytes())?;
        // Extra biome-stage neighbors let the test expand this same capture
        // after taking a feature snapshot and verify revision invalidation.
        for z in 1..=5 {
            let mut neighbor = (**original_document).clone();
            replace_unique_root_field(&mut neighbor, "xPos", Tag::Int(6))?;
            replace_unique_root_field(&mut neighbor, "zPos", Tag::Int(z))?;
            let bytes = native_persistence::nbt::write(&neighbor)?;
            let offset = file.seek(SeekFrom::End(0))?;
            let sectors = (bytes.len() + 5).div_ceil(4096);
            ensure!(
                offset % 4096 == 0 && sectors < 256 && offset / 4096 < (1 << 24),
                "invalid test region entry"
            );
            file.write_all(&(u32::try_from(bytes.len() + 1)?).to_be_bytes())?;
            file.write_all(&[3])?;
            file.write_all(&bytes)?;
            file.write_all(&vec![0; sectors * 4096 - bytes.len() - 5])?;
            file.seek(SeekFrom::Start(((6 + z * 32) * 4) as u64))?;
            file.write_all(&(((offset / 4096) as u32) << 8 | sectors as u32).to_be_bytes())?;
        }
        drop(file);
        let sidecar_document = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("Position".into(), Tag::IntArray(vec![3, 3])),
                (
                    "Entities".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![Tag::Compound(vec![
                            ("id".into(), Tag::String("minecraft:pig".into())),
                            ("opaque".into(), Tag::Int(812)),
                        ])],
                    },
                ),
                ("custom_sidecar".into(), Tag::String("preserve".into())),
            ]),
        };
        let entities_dir = root.join("dimensions/minecraft/overworld/entities");
        std::fs::create_dir_all(&entities_dir)?;
        let mut entity_region = native_persistence::save::FreshRegionDraft::new(
            native_persistence::save::FreshRegionKind::Entities,
            0,
            0,
        );
        entity_region.add(saved_center, &sidecar_document, 1)?;
        let mut sidecar_file = std::fs::OpenOptions::new()
            .write(true)
            .read(true)
            .create_new(true)
            .open(entities_dir.join("r.0.0.mca"))?;
        ensure!(
            entity_region.write_staged(&mut sidecar_file)?.is_empty(),
            "unexpected external entity fixture"
        );
        drop(sidecar_file);
        let session = reader
            .session_guard()
            .context("test session guard missing")?;
        drop(reader);
        let mut reader =
            WorldReader::open_with_session(root, DimensionId::parse("overworld")?, session)?;
        let positions = (1..=5).flat_map(|z| (1..=6).map(move |x| ChunkPos { x, z }));
        let context = reader.capture_generation_context(positions)?;
        let mut restored = prepare_existing_generator(context, &game)?;
        let decoration = restored.prepare_saved_decoration(saved_center)?;
        let first = restored.resume_saved_terrain(
            saved_center,
            std::slice::from_ref(&treasure_id),
            &mut native_resources::density::DensityEvaluator::default(),
        )?;
        let mut terrain = first.into_world();
        let mut evaluator = native_resources::density::DensityEvaluator::default();
        for z in 2..=4 {
            for x in 2..=4 {
                if x == 3 && z == 3 {
                    continue;
                }
                let next = restored.resume_saved_terrain(ChunkPos { x, z }, &[], &mut evaluator)?;
                terrain.insert(&next)?;
            }
        }
        let mut schedule = GenerationSchedule::new();
        schedule.request(center, GenerationStage::Features)?;
        let imported = schedule
            .requirements()
            .map(|(position, _)| {
                let stage =
                    if (position.x - center.x).abs() <= 1 && (position.z - center.z).abs() <= 1 {
                        GenerationStage::Terrain
                    } else {
                        GenerationStage::StructureStarts
                    };
                (position, stage)
            })
            .collect::<Vec<_>>();
        schedule.restore_imported(imported)?;
        let columns =
            (2..=4).flat_map(|z| (2..=4).map(move |x| native_world::loaded::ChunkPos { x, z }));
        let light =
            native_world::generation_light::VisibleLightSnapshot::fresh_after_section_updates(
                true,
                true,
                [],
                columns,
            )
            .context("light snapshot unavailable")?;
        let light = restored.bind_initial_feature_light(&terrain, light)?;
        assert_eq!(
            restored.run_saved_features_with_initial_light(
                &mut terrain,
                &decoration,
                &mut schedule,
                &light
            )?,
            vec![true]
        );
        let key = (treasure_id.clone(), center);
        let treasure = terrain
            .treasure_starts
            .get(&key)
            .context("saved treasure result missing")?;
        assert_eq!(treasure.original, original_start);
        assert_eq!(treasure.references, 2);
        assert_ne!(treasure.placement.bounds.min[1], 90);
        let [x, y, z] = treasure.placement.bounds.min;
        assert_eq!(
            terrain
                .terrain()
                .snapshot()
                .block(x, y, z)
                .context("treasure chest missing")?
                .name,
            "minecraft:chest"
        );
        let refreshed = restored.prepare_saved_structure_plan(&terrain, saved_center)?;
        assert_eq!(
            refreshed.starts[&key].placement.bounds,
            treasure.placement.bounds
        );
        let draft = restored.draft_resumed_structure_starts(&terrain, saved_center)?;
        assert!(draft.matches_source(&treasure.source)?);
        assert_eq!(
            draft.document().value.get("Status"),
            treasure.source.value.get("Status")
        );
        assert_eq!(
            draft.document().value.get("custom_chunk"),
            treasure.source.value.get("custom_chunk")
        );
        let original_structures = treasure
            .source
            .value
            .get("structures")
            .context("source structures missing")?;
        let draft_structures = draft
            .document()
            .value
            .get("structures")
            .context("draft structures missing")?;
        assert_eq!(
            draft_structures.get("References"),
            original_structures.get("References")
        );
        let drafted_start = draft_structures
            .get("starts")
            .and_then(|starts| starts.get("minecraft:buried_treasure"))
            .context("draft treasure missing")?;
        let expected =
            native_session::worldgen::saved_structure_starts::draft_saved_treasure_start(
                &original_start,
                &treasure_id,
                center,
                2,
                &treasure.placement,
            )?;
        assert_eq!(drafted_start, &expected);
        assert_eq!(drafted_start.get("custom_start"), Some(&Tag::Int(73)));
        assert_eq!(drafted_start.get("references"), Some(&Tag::Int(2)));
        let mut stale = (*treasure.source).clone();
        let Tag::Compound(fields) = &mut stale.value else {
            unreachable!()
        };
        fields.push(("changed".into(), Tag::Byte(1)));
        assert!(!draft.matches_source(&stale)?);
        assert!(
            structured
                .draft_resumed_structure_starts(&terrain, saved_center)
                .is_err()
        );
        assert!(
            restored
                .draft_resumed_structure_starts(&terrain, ChunkPos { x: 2, z: 2 })
                .is_err()
        );
        let java_input = root.join("treasure-structure-draft.nbt");
        std::fs::write(
            &java_input,
            native_persistence::nbt::write(draft.document())?,
        )?;
        let checked = std::process::Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("ResumedTreasureDraftOracle")
            .arg(&java_input)
            .arg(y.to_string())
            .output()?;
        ensure!(
            checked.status.success(),
            "Java treasure validation failed: {}\n{}",
            String::from_utf8_lossy(&checked.stdout),
            String::from_utf8_lossy(&checked.stderr)
        );
        let treasure_source = Arc::clone(&treasure.source);
        // Exercise the same entity-recording path used by generation, including
        // passenger suppression and a serializable empty compound.
        let generated_one = Tag::Compound(vec![("n".into(), Tag::Int(21))]);
        let generated_empty = Tag::Compound(vec![]);
        let generated_two = Tag::Compound(vec![("n".into(), Tag::Int(22))]);
        let block_ids = restored
            .terrain_assets
            .as_ref()
            .context("terrain assets missing")?
            .block_ids
            .clone();
        let tags = native_resources::ore_feature::OreBlockTags::load(
            &mut restored.resources,
            &block_ids,
            &BTreeSet::new(),
        )?;
        let motion = restored
            .heightmap_tags
            .as_ref()
            .context("heightmap tags missing")?;
        let mut region =
            terrain
                .terrain
                .decoration_region(&tags, motion, BTreeSet::from([center]))?;
        use native_session::worldgen::GeneratedEntityRecord;
        region.record_generated_entity(
            [56, y, 56],
            GeneratedEntityRecord::Saved(generated_one.clone()),
        )?;
        region.record_generated_entity([56, y, 56], GeneratedEntityRecord::Passenger)?;
        region.record_generated_entity(
            [56, y, 56],
            GeneratedEntityRecord::Saved(generated_empty.clone()),
        )?;
        region.record_generated_entity(
            [56, y, 56],
            GeneratedEntityRecord::Saved(generated_two.clone()),
        )?;
        region.finish()?;
        // This fixture invokes the low-level region API directly; production
        // feature passes update this revision in run_saved_features.
        terrain.revision = Arc::new(());
        let feature = restored.draft_resumed_feature_chunk(&terrain, saved_center)?;
        assert!(feature.matches_source(&treasure_source)?);
        assert!(feature.matches_world(&terrain));
        assert_eq!(feature.entity_sidecar_source(), Some(&sidecar_document));
        assert!(feature.matches_entity_sidecar(Some(&sidecar_document))?);
        assert!(!feature.has_entity_free_feature_coverage());
        assert!(!feature.matches_entity_sidecar(None)?);
        let mut stale_sidecar = sidecar_document.clone();
        let Tag::Compound(fields) = &mut stale_sidecar.value else {
            unreachable!()
        };
        fields.push(("changed".into(), Tag::Byte(1)));
        assert!(!feature.matches_entity_sidecar(Some(&stale_sidecar))?);
        assert_eq!(
            feature.document().value.get("entities"),
            Some(&Tag::List {
                element_type: TagType::Compound,
                values: vec![
                    original_proto,
                    generated_one,
                    generated_empty,
                    generated_two
                ],
            })
        );
        assert_eq!(
            feature.document().value.get("Status"),
            Some(&Tag::String("minecraft:biomes".into()))
        );
        assert_eq!(
            feature.document().value.get("custom_chunk"),
            Some(&Tag::Int(42))
        );
        assert_eq!(
            feature.document().value.get("structures"),
            draft.document().value.get("structures")
        );
        assert_eq!(
            feature.document().value.get("PostProcessing"),
            Some(&terrain.terrain().generated_postprocessing_data(center)?)
        );
        assert!(
            feature
                .document()
                .value
                .get("Heightmaps")
                .unwrap()
                .get("example:opaque")
                .is_some()
        );
        let Tag::List {
            values: block_entities,
            ..
        } = feature
            .document()
            .value
            .get("block_entities")
            .context("block entities missing")?
        else {
            panic!()
        };
        assert!(block_entities.iter().any(
            |tag| matches!(tag.get("id"), Some(Tag::String(id)) if id.matches("minecraft:chest"))
        ));
        let Tag::List {
            values: block_ticks,
            ..
        } = feature
            .document()
            .value
            .get("block_ticks")
            .context("block ticks missing")?
        else {
            panic!()
        };
        assert!(block_ticks.iter().any(|tick| {
            matches!(tick.get("i"), Some(Tag::String(id)) if id.matches("minecraft:melon"))
                && tick.get("x") == Some(&Tag::Int(56))
                && tick.get("y") == Some(&Tag::Int(restored.lighting.min_y + 8))
                && tick.get("z") == Some(&Tag::Int(56))
        }));
        let sections = native_persistence::palette::decode_sections(&feature.document().value)?;
        let section = sections
            .iter()
            .find(|section| i32::from(section.y) == y.div_euclid(16))
            .context("chest section missing")?;
        let block = section
            .blocks
            .as_ref()
            .context("chest blocks missing")?
            .block(
                x.rem_euclid(16) as usize,
                y.rem_euclid(16) as usize,
                z.rem_euclid(16) as usize,
            )
            .context("chest block missing")?;
        assert!(block.name.matches("minecraft:chest"));
        assert!(
            restored
                .run_saved_features(&mut terrain, &decoration, &mut schedule)
                .is_err()
        );
        assert!(feature.matches_world(&terrain));
        let feature_input = root.join("feature-draft.nbt");
        let feature_output = root.join("feature-java-roundtrip.nbt");
        std::fs::write(
            &feature_input,
            native_persistence::nbt::write(feature.document())?,
        )?;
        let checked = std::process::Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("ResumedTerrainOracle")
            .arg(&feature_input)
            .arg(restored.lighting.min_y.to_string())
            .arg(restored.lighting.height.to_string())
            .arg(&feature_output)
            .arg("features")
            .output()?;
        ensure!(
            checked.status.success(),
            "Java feature draft validation failed: {}\n{}",
            String::from_utf8_lossy(&checked.stdout),
            String::from_utf8_lossy(&checked.stderr)
        );
        for z in 2..=4 {
            let next = restored.resume_saved_terrain(ChunkPos { x: 5, z }, &[], &mut evaluator)?;
            terrain.insert(&next)?;
        }
        assert!(!feature.matches_world(&terrain));
        assert!(feature.matches_source(&treasure_source)?);
        let after_insert = restored.draft_resumed_feature_chunk(&terrain, saved_center)?;
        assert!(after_insert.matches_world(&terrain));
        let neighbor = native_world::loaded::ChunkPos { x: 4, z: 3 };
        schedule.request(neighbor, GenerationStage::Features)?;
        let added = schedule
            .requirements()
            .filter_map(|(position, _)| {
                let saved = ChunkPos {
                    x: position.x,
                    z: position.z,
                };
                let stage = if terrain.sources().contains_key(&saved) {
                    GenerationStage::Terrain
                } else {
                    GenerationStage::StructureStarts
                };
                (schedule
                    .completed(position)
                    .is_some_and(|current| current < stage))
                .then_some((position, stage))
            })
            .collect::<Vec<_>>();
        schedule.restore_imported(added)?;
        let neighbor_decoration = restored.prepare_saved_decoration(ChunkPos { x: 4, z: 3 })?;
        let columns =
            (2..=4).flat_map(|z| (3..=5).map(move |x| native_world::loaded::ChunkPos { x, z }));
        let light =
            native_world::generation_light::VisibleLightSnapshot::fresh_after_section_updates(
                true,
                true,
                [],
                columns,
            )
            .context("light snapshot unavailable")?;
        let light = restored.bind_initial_feature_light(&terrain, light)?;
        assert_eq!(
            restored.run_saved_features_with_initial_light(
                &mut terrain,
                &neighbor_decoration,
                &mut schedule,
                &light
            )?,
            vec![true]
        );
        assert!(!after_insert.matches_world(&terrain));
        Ok(())
    }
}
