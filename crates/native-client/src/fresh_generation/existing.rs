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
    generator_definition: Value,
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
    structure_registry: Option<Arc<BTreeSet<ResourceId>>>,
    captured_structure_starts: BTreeMap<
        native_world::loaded::ChunkPos,
        Arc<
            BTreeMap<
                ResourceId,
                native_session::worldgen::saved_structure_starts::RestoredStructureStart,
            >,
        >,
    >,
    creature_environment: Option<(
        native_resources::creature_timeline::CreatureSpawnEnvironment,
        native_resources::entity_category::EntityCategoryRegistry,
    )>,
    mob_placement_assets: Option<SavedMobPlacementAssets>,
    farm_variants: [Option<native_resources::animal_variants::FarmAnimalSpawnVariants>; 3],
    farm_moon: Option<native_resources::moon_phase::Environment>,
    captured_entity_ids: Option<KnownEntityIds>,
    generated_entity_ids: Option<(Arc<()>, KnownEntityIds)>,
}

/// Definite identities in observed records, not proof of global uniqueness.
/// Unknown, absent or malformed identities remain the caller's responsibility.
#[derive(Default)]
struct KnownEntityIds(BTreeSet<[i32; 4]>);

impl KnownEntityIds {
    fn observe_document(&mut self, document: &NamedTag, list_name: &str) {
        if let Tag::Compound(fields) = &document.value {
            for (_, list) in fields.iter().filter(|(name, _)| name.matches(list_name)) {
                self.observe_list(list);
            }
        }
    }

    fn observe_list(&mut self, list: &Tag) {
        if let Tag::List { values, .. } = list {
            self.observe_records(values);
        }
    }

    fn observe_records(&mut self, records: &[Tag]) {
        let mut pending: Vec<&Tag> = records.iter().collect();
        while let Some(entity) = pending.pop() {
            let Tag::Compound(fields) = entity else {
                continue;
            };
            for (name, value) in fields {
                if name.matches("UUID") {
                    if let Tag::IntArray(words) = value {
                        if let Ok(uuid) = <[i32; 4]>::try_from(words.as_slice()) {
                            self.0.insert(uuid);
                        }
                    }
                } else if name.matches("Passengers") {
                    if let Tag::List { values, .. } = value {
                        pending.extend(values);
                    }
                }
            }
        }
    }
}

struct ResumedFarmEnvironment<'a> {
    biome: &'a ResourceId,
    moon: Option<&'a native_resources::moon_phase::Environment>,
    structures: Option<BTreeSet<ResourceId>>,
    resolve_structures: &'a mut dyn FnMut() -> Result<BTreeSet<ResourceId>>,
}

enum CreatureStructureContext<'a> {
    Resumed(&'a ResumedTerrainWorld),
    Captured(&'a CapturedTerrainBatch),
}

impl native_resources::animal_variants::SpawnVariantEnvironment for ResumedFarmEnvironment<'_> {
    fn biome(&self) -> Result<&ResourceId> {
        Ok(self.biome)
    }
    fn moon_phase(&mut self) -> Result<u8> {
        let moon = self.moon.context("moon environment unavailable")?;
        let biome = moon
            .has_biome_layer
            .then(|| format!("{}:{}", self.biome.namespace, self.biome.path));
        Ok(moon
            .sample_static(biome.as_deref())
            .context("static moon phase environment unavailable")?
            .index() as u8)
    }
    fn structures_with_piece(&mut self) -> Result<&BTreeSet<ResourceId>> {
        if self.structures.is_none() {
            self.structures = Some((self.resolve_structures)()?);
        }
        Ok(self.structures.as_ref().expect("structures initialized"))
    }
}

struct SavedMobPlacementAssets {
    prevent_inside: BTreeSet<String>,
    height_tags: native_resources::heightmap_tags::HeightmapTags,
    ground_tags: native_resources::worldgen_mob_rules::GroundSpawnTags,
    sheep_tags: native_resources::worldgen_region_random::SheepBiomeTags,
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

/// A mineshaft has no structure-level after-place effect in Java 26.3. Its
/// pieces can therefore be omitted for this pass only when none intersects
/// the chunk horizontally. Ignoring Y here is deliberately conservative.
fn mineshaft_may_touch_chunk(
    target: native_world::loaded::ChunkPos,
    pieces: &[native_session::worldgen::saved_structure_starts::RestoredStructurePiece],
) -> Result<bool> {
    use native_session::worldgen::saved_structure_starts::RestoredStructurePiece;

    let min_x = target
        .x
        .checked_mul(16)
        .context("structure chunk X overflow")?;
    let min_z = target
        .z
        .checked_mul(16)
        .context("structure chunk Z overflow")?;
    let max_x = min_x
        .checked_add(15)
        .context("structure chunk X overflow")?;
    let max_z = min_z
        .checked_add(15)
        .context("structure chunk Z overflow")?;
    let mut touches = false;
    for piece in pieces {
        let RestoredStructurePiece::Mineshaft(piece) = piece else {
            bail!("mineshaft contains another structure piece type");
        };
        let bounds = piece.piece.bounds;
        touches |= bounds.min[0] <= max_x
            && bounds.max[0] >= min_x
            && bounds.min[2] <= max_z
            && bounds.max[2] >= min_z;
    }
    Ok(touches)
}

#[derive(Clone)]
struct ResumedTreasureStart {
    source: Arc<NamedTag>,
    original: Tag,
    references: i32,
    placement: native_resources::structure_start::SimpleStructureStart,
}

/// The original source and piece state stay attached while several feature
/// centers are processed. This intermediate state does not authorize a save.
#[derive(Clone)]
struct ResumedMineshaftStart {
    source: Arc<NamedTag>,
    original: Tag,
    references: i32,
    placement: native_session::worldgen::mineshaft::PreparedStart,
}

fn resume_saved_mineshaft(
    source: Arc<NamedTag>,
    original: &Tag,
    references: i32,
    chunk: native_world::loaded::ChunkPos,
    adjustment: native_core::beardifier::TerrainAdjustment,
    pieces: &[native_session::worldgen::saved_structure_starts::RestoredStructurePiece],
) -> Result<ResumedMineshaftStart> {
    use native_session::worldgen::saved_structure_starts::RestoredStructurePiece;
    let mut saved_pieces = Vec::with_capacity(pieces.len());
    for piece in pieces {
        let RestoredStructurePiece::Mineshaft(piece) = piece else {
            bail!("mineshaft contains another structure piece type");
        };
        saved_pieces.push(piece.clone());
    }
    let first = saved_pieces
        .first()
        .context("saved mineshaft has no pieces")?;
    let variant = first.variant;
    ensure!(
        saved_pieces.iter().all(|piece| piece.variant == variant),
        "saved mineshaft piece variants differ"
    );
    let placement = native_session::worldgen::mineshaft::PreparedStart {
        chunk: [chunk.x, chunk.z],
        // The saved format does not retain the generation point. Piece
        // placement uses the stored piece bounds instead.
        generation_point: first.piece.bounds.min,
        terrain_adjustment: adjustment,
        variant,
        pieces: saved_pieces,
    };
    Ok(ResumedMineshaftStart {
        source,
        original: original.clone(),
        references,
        placement,
    })
}

struct SavedStructurePlan {
    schedule: Option<Arc<native_resources::structure_decoration::StructureDecorationSchedule>>,
    starts: BTreeMap<SavedStructureKey, ResumedTreasureStart>,
    mineshafts: BTreeMap<SavedStructureKey, ResumedMineshaftStart>,
    order: Vec<(ResourceId, Vec<SavedStructureKey>)>,
}

enum SavedStructureUpdates {
    Simple(Vec<Vec<native_resources::structure_start::SimpleStructureStart>>),
    Mixed(Vec<Vec<native_session::worldgen::PreparedStructureStart>>),
}

impl SavedStructurePlan {
    fn planned(
        &self,
    ) -> Result<Vec<native_session::worldgen::structure_batch::PlannedStructureDecoration<'_>>>
    {
        let mut planned = Vec::with_capacity(self.order.len());
        for (id, keys) in &self.order {
            let entry = self
                .schedule
                .as_ref()
                .context("structure schedule is missing")?
                .steps()
                .iter()
                .flatten()
                .find(|entry| &entry.id == id)
                .context("structure schedule entry disappeared")?;
            let starts = keys.iter().map(|key| &self.starts[key].placement).collect();
            planned.push(
                native_session::worldgen::structure_batch::PlannedStructureDecoration {
                    entry,
                    starts,
                },
            );
        }
        Ok(planned)
    }

    fn apply_updates(
        &mut self,
        updates: Vec<Vec<native_resources::structure_start::SimpleStructureStart>>,
    ) {
        debug_assert_eq!(self.order.len(), updates.len());
        for ((_, keys), updated) in self.order.iter().zip(updates) {
            debug_assert_eq!(keys.len(), updated.len());
            for (key, placement) in keys.iter().zip(updated) {
                self.starts
                    .get_mut(key)
                    .expect("structure start was planned")
                    .placement = placement;
            }
        }
    }

    fn planned_mixed(
        &self,
    ) -> Result<Vec<native_session::worldgen::structure_batch::PlannedMixedStructureDecoration<'_>>>
    {
        use native_session::worldgen::structure_batch::{
            PlannedMixedStructureDecoration, PlannedStructureStart,
        };
        let mut planned = Vec::with_capacity(self.order.len());
        for (id, keys) in &self.order {
            let entry = self
                .schedule
                .as_ref()
                .context("structure schedule is missing")?
                .steps()
                .iter()
                .flatten()
                .find(|entry| &entry.id == id)
                .context("structure schedule entry disappeared")?;
            let mut starts = Vec::with_capacity(keys.len());
            for key in keys {
                match (self.starts.get(key), self.mineshafts.get(key)) {
                    (Some(start), None) => {
                        starts.push(PlannedStructureStart::Simple(&start.placement));
                    }
                    (None, Some(start)) => {
                        starts.push(PlannedStructureStart::Mineshaft(&start.placement));
                    }
                    (None, None) => bail!("planned saved structure start is missing"),
                    (Some(_), Some(_)) => bail!("saved structure start has conflicting types"),
                }
            }
            planned.push(PlannedMixedStructureDecoration { entry, starts });
        }
        Ok(planned)
    }

    fn apply_mixed_updates(
        &mut self,
        updates: Vec<Vec<native_session::worldgen::PreparedStructureStart>>,
    ) -> Result<()> {
        use native_session::worldgen::PreparedStructureStart;
        ensure!(
            self.order.len() == updates.len(),
            "structure update count differs"
        );
        for ((_, keys), updated) in self.order.iter().zip(&updates) {
            ensure!(
                keys.len() == updated.len(),
                "structure start update count differs"
            );
            for (key, value) in keys.iter().zip(updated) {
                ensure!(
                    matches!(value, PreparedStructureStart::Simple(_))
                        && self.starts.contains_key(key)
                        || matches!(value, PreparedStructureStart::Mineshaft(_))
                            && self.mineshafts.contains_key(key),
                    "structure update type differs"
                );
            }
        }
        for ((_, keys), updated) in self.order.iter().zip(updates) {
            for (key, value) in keys.iter().zip(updated) {
                match value {
                    PreparedStructureStart::Simple(placement) => {
                        self.starts.get_mut(key).expect("validated start").placement = placement;
                    }
                    PreparedStructureStart::Mineshaft(placement) => {
                        self.mineshafts
                            .get_mut(key)
                            .expect("validated start")
                            .placement = placement;
                    }
                    _ => unreachable!("validated structure update"),
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
struct UnavailableOceanPlacement;

impl native_session::worldgen::MixedOceanRuinPlacement for UnavailableOceanPlacement {
    fn place(
        &mut self,
        _: &mut native_session::worldgen::DecorationRegion<'_>,
        _: &mut native_session::worldgen::ocean_ruin::PreparedStart,
        _: native_core::beardifier::BoundingBox,
        _: &mut native_core::random::DecorationRandom,
    ) -> Result<()> {
        bail!("saved ocean ruin decoration context is missing")
    }
}

fn partition_structure_sources(
    drafts: BTreeMap<native_world::loaded::ChunkPos, ResumedStructureDraft>,
    mut postprocessed: impl FnMut(native_world::loaded::ChunkPos) -> bool,
) -> (
    BTreeMap<native_world::loaded::ChunkPos, ResumedStructureDraft>,
    BTreeMap<native_world::loaded::ChunkPos, ResumedStructureDraft>,
) {
    let mut included = BTreeMap::new();
    let mut external = BTreeMap::new();
    for (position, draft) in drafts {
        if postprocessed(position) {
            included.insert(position, draft);
        } else {
            external.insert(position, draft);
        }
    }
    (included, external)
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
    world: Arc<native_world::loaded::World>,
    sources: BTreeMap<native_persistence::world::ChunkPos, Arc<NamedTag>>,
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

/// A captured schedule with its restored and newly generated biome volumes.
/// Structure stages must already have been restored by the caller; this does
/// not prove that the other data needed for terrain or saving is complete.
pub(crate) struct CapturedBiomeSchedule {
    identity: Arc<()>,
    schedule: GenerationSchedule,
    biomes: native_session::worldgen::BiomeNeighborhood,
    terrain: BTreeMap<native_persistence::world::ChunkPos, ResumedSavedTerrain>,
    new_terrain: BTreeMap<
        native_persistence::world::ChunkPos,
        native_session::worldgen::MaterializedTerrain,
    >,
    generated_references: BTreeMap<native_world::loaded::ChunkPos, CapturedGeneratedReferences>,
    generated_starts: CapturedGeneratedStarts,
}

type CapturedGeneratedStarts = BTreeMap<
    native_world::loaded::ChunkPos,
    Arc<native_session::worldgen::structure_batch::CapturedStructureStarts>,
>;

struct CapturedGeneratedReferences {
    original: Option<Tag>,
    references: native_session::worldgen::structure_references::OrderedStructureReferences,
}

pub(crate) enum CapturedTerrainInput {
    New {
        structure_keys: Vec<ResourceId>,
    },
    Resume {
        structure_keys: Vec<ResourceId>,
    },
    Saved {
        stage: native_session::worldgen::schedule::GenerationStage,
    },
}

/// Intermediate terrain assembled from one capture. `None` in `sources`
/// means terrain was absent at capture; entity and POI sidecars are separate.
/// Features, lighting, live-state validation and saving remain unfinished.
pub(crate) struct CapturedTerrainBatch {
    identity: Arc<()>,
    schedule: GenerationSchedule,
    biomes: native_session::worldgen::BiomeNeighborhood,
    terrain: native_session::worldgen::CarvedTerrainWorld,
    sources: BTreeMap<native_persistence::world::ChunkPos, Option<Arc<NamedTag>>>,
    saved_stages: BTreeMap<
        native_world::loaded::ChunkPos,
        native_session::worldgen::schedule::GenerationStage,
    >,
    generated_references: BTreeMap<native_world::loaded::ChunkPos, CapturedGeneratedReferences>,
    generated_starts: CapturedGeneratedStarts,
    treasure_starts: BTreeMap<SavedStructureKey, ResumedTreasureStart>,
    feature_entity_sources: ResumedFeatureEntitySources,
    revision: Arc<()>,
}

/// Caller-verified initial visible light for one frozen mixed terrain revision.
pub(crate) struct CapturedFeatureLight {
    identity: Arc<()>,
    revision: Arc<()>,
    light: native_world::generation_light::VisibleLightSnapshot,
}

impl CapturedFeatureLight {
    pub fn matches_batch(&self, batch: &CapturedTerrainBatch) -> bool {
        Arc::ptr_eq(&self.identity, &batch.identity) && Arc::ptr_eq(&self.revision, &batch.revision)
    }
}

/// A caller-observed saved or live light revision for this captured batch.
/// Binding it does not establish that the caller observed every engine event.
pub(crate) struct CapturedQueuedLight {
    identity: Arc<()>,
    revision: Arc<()>,
    engine: native_session::worldgen::QueuedGenerationLightEngine,
}

impl CapturedQueuedLight {
    pub fn matches_batch(&self, batch: &CapturedTerrainBatch) -> bool {
        Arc::ptr_eq(&self.identity, &batch.identity) && Arc::ptr_eq(&self.revision, &batch.revision)
    }

    pub fn snapshot(
        &self,
        batch: &CapturedTerrainBatch,
    ) -> Result<&native_world::generation_light::VisibleLightSnapshot> {
        ensure!(
            self.matches_batch(batch),
            "light belongs to another terrain revision"
        );
        Ok(self.engine.tracker().snapshot())
    }
}

/// Fresh section registrations bound to a single mixed terrain revision.
pub(crate) struct CapturedInitialLight {
    identity: Arc<()>,
    revision: Arc<()>,
    tracker: native_session::worldgen::FreshGenerationLightTracker,
    initialized: BTreeSet<native_world::loaded::ChunkPos>,
}

/// Initialized generation light before or during propagation.
pub(crate) struct CapturedStagedLight {
    identity: Arc<()>,
    revision: Arc<()>,
    engine: native_session::worldgen::StagedGenerationLightEngine,
    initialized: BTreeSet<native_world::loaded::ChunkPos>,
    activated: BTreeSet<native_world::loaded::ChunkPos>,
    light_revision: Arc<()>,
    completed_empty_spawns: BTreeMap<native_world::loaded::ChunkPos, bool>,
    completed_creature_spawns: BTreeMap<native_world::loaded::ChunkPos, Arc<()>>,
}

/// An in-memory LIGHT-stage draft for a previously absent terrain chunk.
/// The captured entity sidecar and feature coverage do not prove save safety.
pub(crate) struct CapturedNewLitDraft {
    identity: Arc<()>,
    revision: Arc<()>,
    light_revision: Arc<()>,
    document: NamedTag,
    entity_sidecar: Option<Arc<NamedTag>>,
    feature_entity_coverage: bool,
}

impl CapturedNewLitDraft {
    pub fn document(&self) -> &NamedTag {
        &self.document
    }

    pub fn entity_sidecar_source(&self) -> Option<&NamedTag> {
        self.entity_sidecar.as_deref()
    }

    pub fn has_entity_free_feature_coverage(&self) -> bool {
        self.feature_entity_coverage
    }

    pub fn matches_state(&self, batch: &CapturedTerrainBatch, light: &CapturedStagedLight) -> bool {
        Arc::ptr_eq(&self.identity, &batch.identity)
            && Arc::ptr_eq(&self.revision, &batch.revision)
            && Arc::ptr_eq(&self.identity, &light.identity)
            && Arc::ptr_eq(&self.revision, &light.revision)
            && Arc::ptr_eq(&self.light_revision, &light.light_revision)
    }
}

/// A non-writable light draft retaining the captured saved protochunk source.
pub(crate) struct CapturedSavedLitDraft {
    identity: Arc<()>,
    revision: Arc<()>,
    light_revision: Arc<()>,
    source_nbt: Vec<u8>,
    document: NamedTag,
    entity_sidecar: Option<Arc<NamedTag>>,
    feature_entity_coverage: bool,
}

impl CapturedSavedLitDraft {
    pub fn document(&self) -> &NamedTag {
        &self.document
    }

    pub fn matches_source(&self, source: &NamedTag) -> Result<bool> {
        Ok(native_persistence::nbt::write(source)? == self.source_nbt)
    }

    pub fn entity_sidecar_source(&self) -> Option<&NamedTag> {
        self.entity_sidecar.as_deref()
    }

    pub fn has_entity_free_feature_coverage(&self) -> bool {
        self.feature_entity_coverage
    }

    pub fn matches_state(&self, batch: &CapturedTerrainBatch, light: &CapturedStagedLight) -> bool {
        Arc::ptr_eq(&self.identity, &batch.identity)
            && Arc::ptr_eq(&self.revision, &batch.revision)
            && Arc::ptr_eq(&self.identity, &light.identity)
            && Arc::ptr_eq(&self.revision, &light.revision)
            && Arc::ptr_eq(&self.light_revision, &light.light_revision)
    }
}

enum CapturedLitChunkDraft {
    New(CapturedNewLitDraft),
    Saved(CapturedSavedLitDraft),
}

/// Spawn draft containing planned creature records. It does not authorize a
/// save or prove that live and uncaptured entity identities are unchanged.
pub(crate) struct CapturedPlannedSpawn {
    lit: CapturedLitChunkDraft,
    position: native_world::loaded::ChunkPos,
    clocks: BTreeMap<ResourceId, i64>,
    border: Option<native_session::respawn::RespawnBorder>,
    completion: Arc<()>,
}

impl CapturedPlannedSpawn {
    pub fn document(&self) -> &NamedTag {
        match &self.lit {
            CapturedLitChunkDraft::New(draft) => draft.document(),
            CapturedLitChunkDraft::Saved(draft) => draft.document(),
        }
    }

    pub fn matches_source(&self, source: Option<&NamedTag>) -> Result<bool> {
        match (&self.lit, source) {
            (CapturedLitChunkDraft::New(_), None) => Ok(true),
            (CapturedLitChunkDraft::Saved(draft), Some(source)) => draft.matches_source(source),
            _ => Ok(false),
        }
    }

    pub fn entity_sidecar_source(&self) -> Option<&NamedTag> {
        match &self.lit {
            CapturedLitChunkDraft::New(draft) => draft.entity_sidecar_source(),
            CapturedLitChunkDraft::Saved(draft) => draft.entity_sidecar_source(),
        }
    }

    pub fn has_entity_free_feature_coverage(&self) -> bool {
        match &self.lit {
            CapturedLitChunkDraft::New(draft) => draft.has_entity_free_feature_coverage(),
            CapturedLitChunkDraft::Saved(draft) => draft.has_entity_free_feature_coverage(),
        }
    }

    pub fn matches_state(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        spawn_mobs: bool,
        clocks: &BTreeMap<ResourceId, i64>,
        border: Option<native_session::respawn::RespawnBorder>,
    ) -> bool {
        let lit_matches = match &self.lit {
            CapturedLitChunkDraft::New(draft) => draft.matches_state(batch, light),
            CapturedLitChunkDraft::Saved(draft) => draft.matches_state(batch, light),
        };
        lit_matches
            && spawn_mobs
            && self.clocks == *clocks
            && self.border == border
            && light
                .completed_creature_spawns
                .get(&self.position)
                .is_some_and(|completion| Arc::ptr_eq(completion, &self.completion))
            && batch
                .schedule
                .completed(self.position)
                .is_some_and(|stage| {
                    stage >= native_session::worldgen::schedule::GenerationStage::Spawn
                })
    }
}

/// Intermediate SPAWN result with no original mob spawning. The source NBT is
/// retained separately; entity sidecars, live rules and remaining save
/// dependencies still require validation before any write.
pub(crate) struct CapturedEmptySpawnDraft {
    lit: CapturedLitChunkDraft,
    position: native_world::loaded::ChunkPos,
    spawn_mobs: bool,
}

impl CapturedEmptySpawnDraft {
    pub fn document(&self) -> &NamedTag {
        match &self.lit {
            CapturedLitChunkDraft::New(draft) => draft.document(),
            CapturedLitChunkDraft::Saved(draft) => draft.document(),
        }
    }

    pub fn matches_source(&self, source: Option<&NamedTag>) -> Result<bool> {
        match (&self.lit, source) {
            (CapturedLitChunkDraft::New(_), None) => Ok(true),
            (CapturedLitChunkDraft::Saved(draft), Some(source)) => draft.matches_source(source),
            _ => Ok(false),
        }
    }

    pub fn entity_sidecar_source(&self) -> Option<&NamedTag> {
        match &self.lit {
            CapturedLitChunkDraft::New(draft) => draft.entity_sidecar_source(),
            CapturedLitChunkDraft::Saved(draft) => draft.entity_sidecar_source(),
        }
    }

    pub fn has_entity_free_feature_coverage(&self) -> bool {
        match &self.lit {
            CapturedLitChunkDraft::New(draft) => draft.has_entity_free_feature_coverage(),
            CapturedLitChunkDraft::Saved(draft) => draft.has_entity_free_feature_coverage(),
        }
    }

    pub fn matches_state(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        spawn_mobs: bool,
    ) -> bool {
        let lit_matches = match &self.lit {
            CapturedLitChunkDraft::New(draft) => draft.matches_state(batch, light),
            CapturedLitChunkDraft::Saved(draft) => draft.matches_state(batch, light),
        };
        lit_matches
            && self.spawn_mobs == spawn_mobs
            && light.completed_empty_spawns.get(&self.position) == Some(&spawn_mobs)
            && batch
                .schedule
                .completed(self.position)
                .is_some_and(|stage| {
                    stage >= native_session::worldgen::schedule::GenerationStage::Spawn
                })
    }
}

/// FULL-conversion heightmaps from one spawn-complete captured chunk. Unknown
/// source heightmap fields are held separately so later drafts can preserve
/// them. Postprocessing, entities and save validation are not established.
#[derive(Clone)]
pub(crate) struct CapturedConvertedHeightmaps {
    identity: Arc<()>,
    revision: Arc<()>,
    light_revision: Arc<()>,
    position: native_world::loaded::ChunkPos,
    spawn_mobs: bool,
    source: Option<Arc<NamedTag>>,
    heightmaps: native_persistence::heightmap::FullHeightmaps,
    other_fields: Vec<(native_persistence::nbt::JavaString, Tag)>,
}

impl CapturedConvertedHeightmaps {
    pub fn heightmaps(&self) -> &native_persistence::heightmap::FullHeightmaps {
        &self.heightmaps
    }

    pub fn other_fields(&self) -> &[(native_persistence::nbt::JavaString, Tag)] {
        &self.other_fields
    }

    pub fn matches_source(&self, source: Option<&NamedTag>) -> Result<bool> {
        match (&self.source, source) {
            (None, None) => Ok(true),
            (Some(expected), Some(actual)) => Ok(native_persistence::nbt::write(expected)?
                == native_persistence::nbt::write(actual)?),
            _ => Ok(false),
        }
    }

    pub fn matches_state(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        spawn_mobs: bool,
    ) -> bool {
        self.matches_terrain_state(batch, light, spawn_mobs)
            && light.completed_empty_spawns.get(&self.position) == Some(&spawn_mobs)
    }

    fn matches_terrain_state(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        spawn_mobs: bool,
    ) -> bool {
        Arc::ptr_eq(&self.identity, &batch.identity)
            && Arc::ptr_eq(&self.revision, &batch.revision)
            && Arc::ptr_eq(&self.identity, &light.identity)
            && Arc::ptr_eq(&self.revision, &light.revision)
            && Arc::ptr_eq(&self.light_revision, &light.light_revision)
            && self.spawn_mobs == spawn_mobs
            && batch
                .schedule
                .completed(self.position)
                .is_some_and(|stage| {
                    stage >= native_session::worldgen::schedule::GenerationStage::Spawn
                })
    }
}

/// Converted heightmaps tied to a specific creature spawn and its live inputs.
#[derive(Clone)]
pub(crate) struct CapturedPlannedConvertedHeightmaps {
    proof: CapturedConvertedHeightmaps,
    completion: Arc<()>,
    clocks: BTreeMap<ResourceId, i64>,
    border: Option<native_session::respawn::RespawnBorder>,
}

impl CapturedPlannedConvertedHeightmaps {
    pub fn heightmaps(&self) -> &native_persistence::heightmap::FullHeightmaps {
        self.proof.heightmaps()
    }

    pub fn other_fields(&self) -> &[(native_persistence::nbt::JavaString, Tag)] {
        self.proof.other_fields()
    }

    pub fn matches_source(&self, source: Option<&NamedTag>) -> Result<bool> {
        self.proof.matches_source(source)
    }

    pub fn matches_state(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        spawn_mobs: bool,
        clocks: &BTreeMap<ResourceId, i64>,
        border: Option<native_session::respawn::RespawnBorder>,
    ) -> bool {
        spawn_mobs
            && self.clocks == *clocks
            && self.border == border
            && self.proof.matches_terrain_state(batch, light, spawn_mobs)
            && light
                .completed_creature_spawns
                .get(&self.proof.position)
                .is_some_and(|completion| Arc::ptr_eq(completion, &self.completion))
    }
}

/// Converted tick containers bound to a creature-spawn proof.
pub(crate) struct CapturedPlannedConvertedPostprocessing {
    proof: CapturedPlannedConvertedHeightmaps,
    converted: native_session::worldgen::postprocessing::ConvertedChunkPostprocessing,
}

impl CapturedPlannedConvertedPostprocessing {
    pub fn matches_state(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        spawn_mobs: bool,
        clocks: &BTreeMap<ResourceId, i64>,
        border: Option<native_session::respawn::RespawnBorder>,
    ) -> bool {
        self.proof
            .matches_state(batch, light, spawn_mobs, clocks, border)
    }

    pub fn into_parts(
        self,
    ) -> (
        CapturedPlannedConvertedHeightmaps,
        native_session::worldgen::postprocessing::ConvertedChunkPostprocessing,
    ) {
        (self.proof, self.converted)
    }
}

/// Current live inputs checked before each isolated callback operation.
#[derive(Clone, Copy)]
pub(crate) struct CapturedPostprocessingInputs<'a> {
    pub spawn_mobs: bool,
    pub clocks: &'a BTreeMap<ResourceId, i64>,
    pub border: Option<native_session::respawn::RespawnBorder>,
    pub game_time: i64,
    pub next_sub_tick: i64,
    pub water_source_conversion: bool,
}

pub(crate) enum CapturedPostprocessingSpawn<'a> {
    Empty(&'a CapturedEmptySpawnDraft),
    Planned(&'a CapturedPlannedSpawn),
}

/// A completed, still private spawn stage for a captured terrain revision.
pub(crate) enum CapturedCompletedSpawn {
    Empty(CapturedEmptySpawnDraft),
    Planned(CapturedPlannedSpawn),
}

impl CapturedCompletedSpawn {
    pub fn position(&self) -> native_world::loaded::ChunkPos {
        match self {
            Self::Empty(spawn) => spawn.position,
            Self::Planned(spawn) => spawn.position,
        }
    }

    pub fn document(&self) -> &NamedTag {
        match self {
            Self::Empty(spawn) => spawn.document(),
            Self::Planned(spawn) => spawn.document(),
        }
    }

    pub fn as_postprocessing_spawn(&self) -> CapturedPostprocessingSpawn<'_> {
        match self {
            Self::Empty(spawn) => CapturedPostprocessingSpawn::Empty(spawn),
            Self::Planned(spawn) => CapturedPostprocessingSpawn::Planned(spawn),
        }
    }

    pub fn matches_state(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        inputs: CapturedCreatureSpawnInputs<'_>,
    ) -> bool {
        match self {
            Self::Empty(spawn) => spawn.matches_state(batch, light, inputs.spawn_mobs),
            Self::Planned(spawn) => spawn.matches_state(
                batch,
                light,
                inputs.spawn_mobs,
                inputs.clocks,
                spawn.border.map(|_| inputs.border),
            ),
        }
    }
}

/// The caller must capture these values from the live session and revalidate
/// them before using an isolated result.
#[derive(Clone, Copy)]
pub(crate) struct CapturedCreatureSpawnInputs<'a> {
    pub spawn_mobs: bool,
    pub clocks: &'a BTreeMap<ResourceId, i64>,
    pub border: native_session::respawn::RespawnBorder,
    pub last_update: i64,
}

impl CapturedPostprocessingSpawn<'_> {
    fn position(&self) -> native_world::loaded::ChunkPos {
        match self {
            Self::Empty(spawn) => spawn.position,
            Self::Planned(spawn) => spawn.position,
        }
    }

    fn entity_candidates(&self, structure_sources_present: bool) -> Result<Option<Arc<Vec<Tag>>>> {
        match self {
            Self::Empty(spawn) => captured_spawn_entity_candidates(
                spawn.document(),
                spawn.entity_sidecar_source(),
                spawn.has_entity_free_feature_coverage(),
                structure_sources_present,
            ),
            Self::Planned(spawn) => captured_spawn_entity_candidates(
                spawn.document(),
                spawn.entity_sidecar_source(),
                spawn.has_entity_free_feature_coverage(),
                structure_sources_present,
            ),
        }
    }
}

#[derive(Clone)]
enum CapturedPostprocessingProof {
    Empty(CapturedConvertedHeightmaps),
    Planned(CapturedPlannedConvertedHeightmaps),
}

impl CapturedPostprocessingProof {
    fn heightmaps(&self) -> &native_persistence::heightmap::FullHeightmaps {
        match self {
            Self::Empty(proof) => proof.heightmaps(),
            Self::Planned(proof) => proof.heightmaps(),
        }
    }

    fn matches_source(&self, source: Option<&NamedTag>) -> Result<bool> {
        match self {
            Self::Empty(proof) => proof.matches_source(source),
            Self::Planned(proof) => proof.matches_source(source),
        }
    }

    fn other_fields(&self) -> &[(native_persistence::nbt::JavaString, Tag)] {
        match self {
            Self::Empty(proof) => proof.other_fields(),
            Self::Planned(proof) => proof.other_fields(),
        }
    }

    fn matches_state(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        inputs: CapturedPostprocessingInputs<'_>,
    ) -> bool {
        match self {
            Self::Empty(proof) => proof.matches_state(batch, light, inputs.spawn_mobs),
            Self::Planned(proof) => proof.matches_state(
                batch,
                light,
                inputs.spawn_mobs,
                inputs.clocks,
                proof.border.and(inputs.border),
            ),
        }
    }
}

/// Ordered converted chunks retaining either kind of spawn proof.
pub(crate) struct CapturedMixedConvertedRegion {
    proofs: BTreeMap<native_world::loaded::ChunkPos, CapturedPostprocessingProof>,
    entity_candidates: BTreeMap<native_world::loaded::ChunkPos, Option<Arc<Vec<Tag>>>>,
    region: native_session::worldgen::postprocessing::ConvertedPostprocessingRegion,
}

impl CapturedMixedConvertedRegion {
    fn matches_state(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        inputs: CapturedPostprocessingInputs<'_>,
    ) -> bool {
        !self.proofs.is_empty()
            && self.region.ticks.game_time() == inputs.game_time
            && self.region.ticks.next_sub_tick() == inputs.next_sub_tick
            && self
                .proofs
                .values()
                .all(|proof| proof.matches_state(batch, light, inputs))
    }
}

/// Private callback execution over converted chunks and their spawn proofs.
pub(crate) struct CapturedMixedPostprocessingStage {
    proofs: BTreeMap<native_world::loaded::ChunkPos, CapturedPostprocessingProof>,
    stage: native_session::worldgen::GeneratedPostprocessing,
    game_time: i64,
    next_sub_tick: i64,
    water_source_conversion: bool,
    waiting_neighbor: Option<native_world::loaded::ChunkPos>,
    retry_target: Option<native_world::loaded::ChunkPos>,
}

impl CapturedMixedPostprocessingStage {
    pub fn waiting_neighbor(&self) -> Option<native_world::loaded::ChunkPos> {
        self.waiting_neighbor
    }

    pub fn matches_state(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        inputs: CapturedPostprocessingInputs<'_>,
    ) -> bool {
        self.game_time == inputs.game_time
            && self.next_sub_tick == inputs.next_sub_tick
            && self.water_source_conversion == inputs.water_source_conversion
            && !self.proofs.is_empty()
            && self
                .proofs
                .values()
                .all(|proof| proof.matches_state(batch, light, inputs))
    }

    pub fn request_converted_chunk(
        &mut self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        inputs: CapturedPostprocessingInputs<'_>,
        position: native_world::loaded::ChunkPos,
    ) -> Result<()> {
        ensure!(
            self.matches_state(batch, light, inputs),
            "captured postprocessing inputs changed"
        );
        ensure!(
            self.waiting_neighbor.is_none() && self.retry_target.is_none(),
            "finish the interrupted callback before requesting another chunk"
        );
        ensure!(
            self.proofs.contains_key(&position),
            "postprocessing chunk was not captured"
        );
        self.stage.request_converted_chunk(position)
    }

    pub fn try_poll(
        &mut self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        inputs: CapturedPostprocessingInputs<'_>,
        position: native_world::loaded::ChunkPos,
        budget: usize,
    ) -> Result<native_session::worldgen::PostprocessingAttempt> {
        ensure!(
            self.matches_state(batch, light, inputs),
            "captured postprocessing inputs changed"
        );
        ensure!(
            self.waiting_neighbor.is_none(),
            "register the requested neighbor before retrying"
        );
        ensure!(
            self.retry_target.is_none_or(|target| target == position),
            "retry the interrupted chunk before another chunk"
        );
        ensure!(budget > 0, "postprocessing budget must be positive");
        let attempt = self.stage.try_poll(position, budget)?;
        self.waiting_neighbor = match attempt {
            native_session::worldgen::PostprocessingAttempt::NeedsChunk(position) => Some(position),
            native_session::worldgen::PostprocessingAttempt::Progress { .. } => None,
        };
        self.retry_target = self.waiting_neighbor.map(|_| position);
        Ok(attempt)
    }

    pub fn finish(
        self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        inputs: CapturedPostprocessingInputs<'_>,
    ) -> Result<CapturedMixedPostprocessedRegion> {
        ensure!(
            self.matches_state(batch, light, inputs),
            "captured postprocessing inputs changed"
        );
        ensure!(
            self.waiting_neighbor.is_none() && self.retry_target.is_none(),
            "postprocessing still needs a retry"
        );
        Ok(CapturedMixedPostprocessedRegion {
            identity: Arc::new(()),
            proofs: self.proofs,
            region: self.stage.finish()?,
            game_time: self.game_time,
            next_sub_tick: self.next_sub_tick,
            water_source_conversion: self.water_source_conversion,
        })
    }
}

pub(crate) struct CapturedMixedPostprocessedRegion {
    identity: Arc<()>,
    proofs: BTreeMap<native_world::loaded::ChunkPos, CapturedPostprocessingProof>,
    region: native_session::worldgen::PostprocessedGeneratedRegion,
    game_time: i64,
    next_sub_tick: i64,
    water_source_conversion: bool,
}

impl CapturedMixedPostprocessedRegion {
    pub fn region(&self) -> &native_session::worldgen::PostprocessedGeneratedRegion {
        &self.region
    }

    pub fn matches_state(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        inputs: CapturedPostprocessingInputs<'_>,
    ) -> bool {
        self.game_time == inputs.game_time
            && self.next_sub_tick == inputs.next_sub_tick
            && self.water_source_conversion == inputs.water_source_conversion
            && !self.proofs.is_empty()
            && self
                .proofs
                .values()
                .all(|proof| proof.matches_state(batch, light, inputs))
    }
}

/// Encoded sections bound to one completed mixed postprocessing pass.
pub(crate) struct CapturedMixedPostprocessedSections {
    result_identity: Arc<()>,
    sections: BTreeMap<native_world::loaded::ChunkPos, Tag>,
}

impl CapturedMixedPostprocessedSections {
    pub fn sections(&self) -> &BTreeMap<native_world::loaded::ChunkPos, Tag> {
        &self.sections
    }

    pub fn matches_state(
        &self,
        completed: &CapturedMixedPostprocessedRegion,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        inputs: CapturedPostprocessingInputs<'_>,
    ) -> bool {
        Arc::ptr_eq(&self.result_identity, &completed.identity)
            && completed.matches_state(batch, light, inputs)
    }
}

/// Isolated output for a sealed mixed pass, still requiring live and disk
/// source checks and one coordinated save transaction.
pub(crate) struct CapturedMixedPostprocessedComponents {
    result_identity: Arc<()>,
    chunks: BTreeMap<
        native_world::loaded::ChunkPos,
        native_session::worldgen::PostprocessedChunkComponents,
    >,
    structures: BTreeMap<native_world::loaded::ChunkPos, Option<Tag>>,
    external_structure_sources: BTreeMap<native_world::loaded::ChunkPos, ResumedStructureDraft>,
    missing_poi: BTreeMap<native_world::loaded::ChunkPos, Option<NamedTag>>,
}

impl CapturedMixedPostprocessedComponents {
    pub fn outputs(
        &self,
    ) -> (
        &BTreeMap<
            native_world::loaded::ChunkPos,
            native_session::worldgen::PostprocessedChunkComponents,
        >,
        &BTreeMap<native_world::loaded::ChunkPos, Option<Tag>>,
        &BTreeMap<native_world::loaded::ChunkPos, ResumedStructureDraft>,
        &BTreeMap<native_world::loaded::ChunkPos, Option<NamedTag>>,
    ) {
        (
            &self.chunks,
            &self.structures,
            &self.external_structure_sources,
            &self.missing_poi,
        )
    }

    pub fn matches_state(
        &self,
        completed: &CapturedMixedPostprocessedRegion,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        inputs: CapturedPostprocessingInputs<'_>,
    ) -> bool {
        Arc::ptr_eq(&self.result_identity, &completed.identity)
            && completed.matches_state(batch, light, inputs)
    }
}

/// In-memory FULL and entity-region drafts from one captured pass. They do not
/// authorize overwriting captured files or replacing a live session.
pub(crate) struct CapturedMixedFullDrafts {
    result_identity: Arc<()>,
    chunks: BTreeMap<native_world::loaded::ChunkPos, CapturedMixedFullDraft>,
}

/// Existing POI drafts bound to the same sealed postprocessing pass.
pub(crate) struct CapturedMixedExistingPoi {
    result_identity: Arc<()>,
    documents: BTreeMap<native_world::loaded::ChunkPos, NamedTag>,
}

/// An isolated storage candidate. It intentionally exposes no write plan
/// until the remaining live-state and generation effects are reconciled.
pub(crate) struct CapturedMixedSaveCandidate {
    _plan: native_persistence::save::WorldSavePlan,
}

#[cfg(test)]
impl CapturedMixedSaveCandidate {
    fn chunk_count(&self) -> usize {
        self._plan.chunk_count()
    }
}

pub(crate) struct CapturedMixedFullDraft {
    source_nbt: Option<Vec<u8>>,
    source_entities_nbt: Option<Vec<u8>>,
    chunk: NamedTag,
    entity_chunk: NamedTag,
}

impl CapturedMixedFullDraft {
    pub fn chunk(&self) -> &NamedTag {
        &self.chunk
    }
    pub fn entity_chunk(&self) -> &NamedTag {
        &self.entity_chunk
    }
    pub fn matches_source(&self, source: Option<&NamedTag>) -> Result<bool> {
        match (&self.source_nbt, source) {
            (None, None) => Ok(true),
            (Some(expected), Some(actual)) => {
                Ok(*expected == native_persistence::nbt::write(actual)?)
            }
            _ => Ok(false),
        }
    }
    pub fn matches_entity_source(&self, source: Option<&NamedTag>) -> Result<bool> {
        match (&self.source_entities_nbt, source) {
            (None, None) => Ok(true),
            (Some(expected), Some(actual)) => {
                Ok(*expected == native_persistence::nbt::write(actual)?)
            }
            _ => Ok(false),
        }
    }
}

impl CapturedMixedFullDrafts {
    pub fn chunks(&self) -> &BTreeMap<native_world::loaded::ChunkPos, CapturedMixedFullDraft> {
        &self.chunks
    }
    pub fn matches_state(
        &self,
        completed: &CapturedMixedPostprocessedRegion,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        inputs: CapturedPostprocessingInputs<'_>,
    ) -> bool {
        Arc::ptr_eq(&self.result_identity, &completed.identity)
            && completed.matches_state(batch, light, inputs)
    }
}

/// A single captured chunk's converted pending ticks and ordered callbacks.
/// The proof must be rechecked against the same generation revision before
/// using the parts in an isolated postprocessing pass.
pub(crate) struct CapturedConvertedPostprocessing {
    proof: CapturedConvertedHeightmaps,
    converted: native_session::worldgen::postprocessing::ConvertedChunkPostprocessing,
}

impl CapturedConvertedPostprocessing {
    pub fn matches_state(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        spawn_mobs: bool,
    ) -> bool {
        self.proof.matches_state(batch, light, spawn_mobs)
    }

    pub fn into_parts(
        self,
    ) -> (
        CapturedConvertedHeightmaps,
        native_session::worldgen::postprocessing::ConvertedChunkPostprocessing,
    ) {
        (self.proof, self.converted)
    }
}

/// Converted neighboring chunks sharing one ordered tick region. The proofs
/// remain separate from the mutable tasks so callers can recheck the captured
/// generation state before publishing any result.
pub(crate) struct CapturedConvertedRegion {
    proofs: BTreeMap<native_world::loaded::ChunkPos, CapturedConvertedHeightmaps>,
    region: native_session::worldgen::postprocessing::ConvertedPostprocessingRegion,
}

impl CapturedConvertedRegion {
    pub fn matches_state(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        spawn_mobs: bool,
        game_time: i64,
        next_sub_tick: i64,
    ) -> bool {
        !self.proofs.is_empty()
            && self.region.ticks.game_time() == game_time
            && self.region.ticks.next_sub_tick() == next_sub_tick
            && self
                .proofs
                .values()
                .all(|proof| proof.matches_state(batch, light, spawn_mobs))
    }

    pub fn into_parts(
        self,
    ) -> (
        BTreeMap<native_world::loaded::ChunkPos, CapturedConvertedHeightmaps>,
        native_session::worldgen::postprocessing::ConvertedPostprocessingRegion,
    ) {
        (self.proofs, self.region)
    }
}

/// An isolated postprocessing pass tied to captured generation and live rules.
pub(crate) struct CapturedPostprocessingStage {
    proofs: BTreeMap<native_world::loaded::ChunkPos, CapturedConvertedHeightmaps>,
    stage: native_session::worldgen::GeneratedPostprocessing,
    game_time: i64,
    next_sub_tick: i64,
    water_source_conversion: bool,
    waiting_neighbor: Option<native_world::loaded::ChunkPos>,
    retry_target: Option<native_world::loaded::ChunkPos>,
}

impl CapturedPostprocessingStage {
    pub fn waiting_neighbor(&self) -> Option<native_world::loaded::ChunkPos> {
        self.waiting_neighbor
    }

    /// Request a deferred converted chunk at a callback boundary. The caller
    /// controls the order in which Java would start the chunk passes.
    pub fn request_converted_chunk(
        &mut self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        spawn_mobs: bool,
        game_time: i64,
        next_sub_tick: i64,
        water_source_conversion: bool,
        position: native_world::loaded::ChunkPos,
    ) -> Result<()> {
        ensure!(
            self.matches_state(
                batch,
                light,
                spawn_mobs,
                game_time,
                next_sub_tick,
                water_source_conversion
            ),
            "captured postprocessing inputs changed"
        );
        ensure!(
            self.waiting_neighbor.is_none() && self.retry_target.is_none(),
            "finish the interrupted callback before requesting another chunk"
        );
        ensure!(
            self.proofs.contains_key(&position),
            "postprocessing chunk was not captured"
        );
        self.stage.request_converted_chunk(position)
    }

    pub fn matches_state(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        spawn_mobs: bool,
        game_time: i64,
        next_sub_tick: i64,
        water_source_conversion: bool,
    ) -> bool {
        self.game_time == game_time
            && self.next_sub_tick == next_sub_tick
            && self.water_source_conversion == water_source_conversion
            && !self.proofs.is_empty()
            && self
                .proofs
                .values()
                .all(|proof| proof.matches_state(batch, light, spawn_mobs))
    }

    /// Try callbacks on a private fork; a missing converted neighbor leaves
    /// the stage unchanged and is reported to the caller.
    pub fn try_poll(
        &mut self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        spawn_mobs: bool,
        game_time: i64,
        next_sub_tick: i64,
        water_source_conversion: bool,
        position: native_world::loaded::ChunkPos,
        budget: usize,
    ) -> Result<native_session::worldgen::PostprocessingAttempt> {
        ensure!(
            self.matches_state(
                batch,
                light,
                spawn_mobs,
                game_time,
                next_sub_tick,
                water_source_conversion
            ),
            "captured postprocessing inputs changed"
        );
        ensure!(
            self.waiting_neighbor.is_none(),
            "register the requested neighbor before retrying"
        );
        ensure!(
            self.retry_target.is_none_or(|target| target == position),
            "retry the interrupted chunk before another chunk"
        );
        ensure!(budget > 0, "postprocessing budget must be positive");
        let attempt = self.stage.try_poll(position, budget)?;
        self.waiting_neighbor = match attempt {
            native_session::worldgen::PostprocessingAttempt::NeedsChunk(position) => Some(position),
            native_session::worldgen::PostprocessingAttempt::Progress { .. } => None,
        };
        self.retry_target = self.waiting_neighbor.map(|_| position);
        Ok(attempt)
    }

    pub fn finish(
        self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        spawn_mobs: bool,
        game_time: i64,
        next_sub_tick: i64,
        water_source_conversion: bool,
    ) -> Result<CapturedPostprocessedRegion> {
        ensure!(
            self.matches_state(
                batch,
                light,
                spawn_mobs,
                game_time,
                next_sub_tick,
                water_source_conversion
            ),
            "captured postprocessing inputs changed"
        );
        ensure!(
            self.waiting_neighbor.is_none() && self.retry_target.is_none(),
            "postprocessing still needs a retry"
        );
        Ok(CapturedPostprocessedRegion {
            identity: Arc::new(()),
            proofs: self.proofs,
            region: self.stage.finish()?,
            game_time,
            next_sub_tick,
            water_source_conversion,
        })
    }
}

/// Isolated callback output. Disk, live entities, lighting updates, clocks and
/// all other save dependencies still need validation before publication.
pub(crate) struct CapturedPostprocessedRegion {
    identity: Arc<()>,
    proofs: BTreeMap<native_world::loaded::ChunkPos, CapturedConvertedHeightmaps>,
    region: native_session::worldgen::PostprocessedGeneratedRegion,
    game_time: i64,
    next_sub_tick: i64,
    water_source_conversion: bool,
}

/// Non-writable sections from one completed captured postprocessing result.
pub(crate) struct CapturedPostprocessedSections {
    result_identity: Arc<()>,
    sections: BTreeMap<native_world::loaded::ChunkPos, Tag>,
}

impl CapturedPostprocessedSections {
    pub fn sections(&self) -> &BTreeMap<native_world::loaded::ChunkPos, Tag> {
        &self.sections
    }

    pub fn matches_state(
        &self,
        completed: &CapturedPostprocessedRegion,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        spawn_mobs: bool,
        game_time: i64,
        next_sub_tick: i64,
        water_source_conversion: bool,
    ) -> bool {
        Arc::ptr_eq(&self.result_identity, &completed.identity)
            && completed.matches_state(
                batch,
                light,
                spawn_mobs,
                game_time,
                next_sub_tick,
                water_source_conversion,
            )
    }
}

/// Components tied to the same completed pass. External source updates must be
/// committed with the chunks; entity sidecars and other save checks remain unknown.
pub(crate) struct CapturedPostprocessedComponents {
    result_identity: Arc<()>,
    chunks: BTreeMap<
        native_world::loaded::ChunkPos,
        native_session::worldgen::PostprocessedChunkComponents,
    >,
    structures: BTreeMap<native_world::loaded::ChunkPos, Option<Tag>>,
    external_structure_sources: BTreeMap<native_world::loaded::ChunkPos, ResumedStructureDraft>,
    missing_poi: BTreeMap<native_world::loaded::ChunkPos, Option<NamedTag>>,
    generated_empty_entities: BTreeSet<native_world::loaded::ChunkPos>,
}

impl CapturedPostprocessedComponents {
    /// Every returned component must be committed in one transaction after
    /// revalidating source data, live state, packs and entity/POI sidecars.
    /// The POI map contains only chunks whose captured storage was absent:
    /// a missing key is unknown, while a present `None` means no POI was found.
    /// The entity set covers this chunk's captured proto/sidecar and the
    /// verified generated contribution. Other stored and live entity sources
    /// remain unresolved.
    pub fn outputs(
        &self,
    ) -> (
        &BTreeMap<
            native_world::loaded::ChunkPos,
            native_session::worldgen::PostprocessedChunkComponents,
        >,
        &BTreeMap<native_world::loaded::ChunkPos, Option<Tag>>,
        &BTreeMap<native_world::loaded::ChunkPos, ResumedStructureDraft>,
        &BTreeMap<native_world::loaded::ChunkPos, Option<NamedTag>>,
        &BTreeSet<native_world::loaded::ChunkPos>,
    ) {
        (
            &self.chunks,
            &self.structures,
            &self.external_structure_sources,
            &self.missing_poi,
            &self.generated_empty_entities,
        )
    }

    pub fn matches_state(
        &self,
        completed: &CapturedPostprocessedRegion,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        spawn_mobs: bool,
        game_time: i64,
        next_sub_tick: i64,
        water_source_conversion: bool,
    ) -> bool {
        Arc::ptr_eq(&self.result_identity, &completed.identity)
            && completed.matches_state(
                batch,
                light,
                spawn_mobs,
                game_time,
                next_sub_tick,
                water_source_conversion,
            )
    }
}

fn captured_missing_poi_positions(
    sources: &BTreeMap<
        native_persistence::world::ChunkPos,
        native_persistence::world::GenerationChunkSource,
    >,
    positions: impl IntoIterator<Item = native_world::loaded::ChunkPos>,
) -> Result<Vec<native_world::loaded::ChunkPos>> {
    let mut missing = Vec::new();
    for pos in positions {
        let source = sources
            .get(&native_persistence::world::ChunkPos { x: pos.x, z: pos.z })
            .context("POI source was not captured")?;
        if source.poi.is_none() {
            missing.push(pos);
        }
    }
    Ok(missing)
}

/// Establish only the empty generated-entity case. Saved/live sidecars and
/// spawning rules must still be revalidated before these records are used.
fn captured_generated_entities_are_empty(
    source: Option<&NamedTag>,
    entity_sidecar: Option<&NamedTag>,
    feature_coverage: bool,
    structure_sources_present: bool,
    generated_entities: &[Tag],
    drops_occurred: bool,
) -> Result<bool> {
    if entity_sidecar.is_some()
        || !feature_coverage
        || structure_sources_present
        || !generated_entities.is_empty()
        || drops_occurred
    {
        return Ok(false);
    }
    let original = source
        .map(|document| unique_root_field(document, "entities"))
        .transpose()?
        .flatten();
    let entities = native_persistence::generated_chunk::append_proto_entity_records(original, &[])?;
    if matches!(entities, Some(Tag::List { values, .. }) if !values.is_empty()) {
        return Ok(false);
    }
    Ok(true)
}

/// Retain captured sidecar and protochunk records as candidates. They do not
/// establish which entities are loaded or authorize a live-world save.
fn captured_spawn_entity_candidates(
    document: &NamedTag,
    entity_sidecar: Option<&NamedTag>,
    feature_coverage: bool,
    structure_sources_present: bool,
) -> Result<Option<Arc<Vec<Tag>>>> {
    if !feature_coverage || structure_sources_present {
        return Ok(None);
    }
    let original = unique_root_field(document, "entities")?;
    let entities = native_persistence::generated_chunk::append_proto_entity_records(original, &[])?;
    let records = match entities {
        None => Vec::new(),
        Some(Tag::List { values, .. }) => values,
        _ => unreachable!("entity list was validated"),
    };
    if let Some(sidecar) = entity_sidecar {
        let (Some(Tag::Int(x)), Some(Tag::Int(z))) = (
            unique_root_field(document, "xPos")?,
            unique_root_field(document, "zPos")?,
        ) else {
            bail!("spawn chunk position is missing")
        };
        let combined = native_persistence::generated_chunk::append_entity_chunk_records(
            native_world::loaded::ChunkPos { x: *x, z: *z },
            sidecar,
            &records,
        )?;
        let values = match unique_root_field(&combined, "Entities")? {
            Some(Tag::List { values, .. }) => values.clone(),
            None => Vec::new(),
            _ => unreachable!("entity list was validated"),
        };
        return Ok(Some(Arc::new(values)));
    }
    Ok(Some(Arc::new(records)))
}

/// Restore the captured sidecar prefix exactly once when assembling a draft.
fn merge_captured_entity_candidates(
    position: native_world::loaded::ChunkPos,
    sidecar: &NamedTag,
    candidates: &[Tag],
) -> Result<NamedTag> {
    let existing = match unique_root_field(sidecar, "Entities")? {
        Some(Tag::List { values, .. }) => values.as_slice(),
        None => &[],
        _ => bail!("captured entity records are invalid"),
    };
    ensure!(
        candidates.starts_with(existing),
        "captured entity candidates differ from their sidecar"
    );
    native_persistence::generated_chunk::append_entity_chunk_records(
        position,
        sidecar,
        &candidates[existing.len()..],
    )
}

impl CapturedPostprocessedRegion {
    pub fn region(&self) -> &native_session::worldgen::PostprocessedGeneratedRegion {
        &self.region
    }

    pub fn matches_state(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        spawn_mobs: bool,
        game_time: i64,
        next_sub_tick: i64,
        water_source_conversion: bool,
    ) -> bool {
        self.game_time == game_time
            && self.next_sub_tick == next_sub_tick
            && self.water_source_conversion == water_source_conversion
            && !self.proofs.is_empty()
            && self
                .proofs
                .values()
                .all(|proof| proof.matches_state(batch, light, spawn_mobs))
    }
}

impl CapturedTerrainBatch {
    pub fn schedule(&self) -> &GenerationSchedule {
        &self.schedule
    }

    pub fn biomes(&self) -> &native_session::worldgen::BiomeNeighborhood {
        &self.biomes
    }

    pub fn terrain(&self) -> &native_session::worldgen::CarvedTerrainWorld {
        &self.terrain
    }

    pub fn sources(&self) -> &BTreeMap<native_persistence::world::ChunkPos, Option<Arc<NamedTag>>> {
        &self.sources
    }

    /// Retain the actual disk status separately from the stages whose data
    /// have been imported into the generation schedule.
    pub fn saved_stage(
        &self,
        position: native_world::loaded::ChunkPos,
    ) -> Option<native_session::worldgen::schedule::GenerationStage> {
        self.saved_stages.get(&position).copied()
    }

    fn require_unfinished_features(&self, position: native_world::loaded::ChunkPos) -> Result<()> {
        use native_session::worldgen::schedule::GenerationStage;
        ensure!(
            self.saved_stage(position)
                .is_none_or(|stage| stage < GenerationStage::Features),
            "saved chunk already completed features"
        );
        Ok(())
    }

    pub fn has_entity_free_feature_coverage(
        &self,
        position: native_world::loaded::ChunkPos,
    ) -> bool {
        self.feature_entity_sources.covers(position)
    }

    /// Import one restored saved chunk and its observed light as one revision.
    /// The caller must establish the actual load and light-drain order.
    fn import_saved_chunk_with_queued_light(
        &mut self,
        restored: &RestoredSavedTerrain,
        light: &mut CapturedQueuedLight,
        position: native_world::loaded::ChunkPos,
        order: native_session::worldgen::SavedLightLoadOrder,
        is_air: impl FnMut(&native_world::loaded::BlockState) -> Option<bool>,
    ) -> Result<()> {
        use native_session::worldgen::schedule::GenerationStage;

        ensure!(
            Arc::ptr_eq(&self.identity, &restored.identity) && light.matches_batch(self),
            "saved terrain or light belongs to another capture revision"
        );
        ensure!(
            self.schedule
                .required(position)
                .is_some_and(|stage| stage >= GenerationStage::Terrain)
                && self.schedule.completed(position) == Some(GenerationStage::Biomes),
            "saved terrain prerequisites are incomplete"
        );
        let key = native_persistence::world::ChunkPos {
            x: position.x,
            z: position.z,
        };
        ensure!(
            !self.sources.contains_key(&key) && !self.saved_stages.contains_key(&position),
            "saved terrain chunk was already imported"
        );
        let source = restored
            .sources
            .get(&key)
            .context("saved terrain source is missing")?;
        let chunk = restored
            .world
            .chunks
            .get(&position)
            .context("saved terrain chunk is missing")?;
        ensure!(
            chunk.position == position
                && chunk.status.as_deref() == Some("minecraft:full")
                && chunk.block_light_correct,
            "saved terrain chunk is not light-correct and full"
        );
        let expected_biomes = restored
            .biomes
            .get(&position)
            .context("saved biomes are missing")?;
        let actual_biomes = self
            .biomes
            .chunk([position.x, position.z])
            .context("captured biomes are missing")?;
        ensure!(
            expected_biomes.min_y() == actual_biomes.min_y()
                && expected_biomes.height() == actual_biomes.height()
                && expected_biomes.values() == actual_biomes.values(),
            "captured biomes differ from saved terrain"
        );
        let heights = restored
            .heightmaps
            .get(&position)
            .context("saved heightmaps are missing")?;
        let marks = restored
            .postprocessing
            .get(&position)
            .context("saved postprocessing is missing")?;
        let mut terrain = self.terrain.clone();
        terrain.insert_saved_chunk(&restored.world, position, heights, marks.as_ref())?;
        let imported = terrain.snapshot();
        let imported_chunk = imported
            .chunks
            .get(&position)
            .context("imported saved terrain chunk is missing")?;
        ensure!(
            native_session::loaded::matching_terrain_chunks(
                &restored.world,
                chunk,
                &imported,
                imported_chunk,
            ),
            "saved terrain changed during import"
        );
        let mut engine = light.engine.clone();
        engine.load_saved_chunk(&terrain, position, order, is_air)?;

        // restore_imported validates the whole update before mutating the plan.
        self.schedule
            .restore_imported([(position, GenerationStage::Terrain)])?;
        self.terrain = terrain;
        self.sources.insert(key, Some(Arc::clone(source)));
        self.saved_stages.insert(position, GenerationStage::Full);
        self.revision = Arc::new(());
        light.engine = engine;
        light.revision = Arc::clone(&self.revision);
        Ok(())
    }
}

impl CapturedBiomeSchedule {
    pub fn schedule(&self) -> &GenerationSchedule {
        &self.schedule
    }

    pub fn biomes(&self) -> &native_session::worldgen::BiomeNeighborhood {
        &self.biomes
    }

    pub fn terrain_chunk(
        &self,
        position: native_persistence::world::ChunkPos,
    ) -> Option<&ResumedSavedTerrain> {
        self.terrain.get(&position)
    }

    pub fn new_terrain_chunk(
        &self,
        position: native_persistence::world::ChunkPos,
    ) -> Option<&native_session::worldgen::MaterializedTerrain> {
        self.new_terrain.get(&position)
    }

    pub fn into_parts(
        self,
    ) -> (
        GenerationSchedule,
        native_session::worldgen::BiomeNeighborhood,
        BTreeMap<native_persistence::world::ChunkPos, ResumedSavedTerrain>,
        BTreeMap<
            native_persistence::world::ChunkPos,
            native_session::worldgen::MaterializedTerrain,
        >,
    ) {
        (self.schedule, self.biomes, self.terrain, self.new_terrain)
    }
}

/// Live terrain, POI and tracked entity evidence bound to one captured
/// generator. Other live state and disk and pack dependencies remain separate.
pub(crate) struct LiveGenerationInputs {
    identity: Arc<()>,
    terrain: native_session::loaded::GenerationTerrainGuard,
    poi: native_session::loaded::GenerationPoiGuard,
    entities: native_session::loaded::GenerationEntityGuard,
}

/// Block and biome inputs for intermediate work. Lighting, ticks, entities,
/// POIs and publication still require their own coordinated validation.
pub(crate) struct LiveGenerationTerrainStage {
    identity: Arc<()>,
    terrain: native_session::loaded::GenerationTerrainStage,
}

pub(crate) struct LiveGenerationTerrainStageDraft {
    identity: Arc<()>,
    terrain: native_session::loaded::GenerationTerrainStageDraft,
}

/// Detached generation terrain incorporating captured live blocks. Feature
/// work remains intermediate; lighting, play and saving still need validation.
pub(crate) struct StagedLiveTerrainBatch {
    batch: CapturedTerrainBatch,
    feature_order: Vec<native_world::loaded::ChunkPos>,
    mineshaft_starts: BTreeMap<SavedStructureKey, ResumedMineshaftStart>,
}

impl StagedLiveTerrainBatch {
    pub fn snapshot(&self) -> Arc<native_world::loaded::World> {
        self.batch.terrain.snapshot()
    }

    pub fn heightmaps(&self, pos: native_world::loaded::ChunkPos) -> Result<Tag> {
        self.batch.terrain.generated_heightmap_data(pos)
    }

    pub fn biome_chunk(
        &self,
        pos: native_world::loaded::ChunkPos,
    ) -> Option<Arc<native_session::worldgen::QuartBiomeChunk>> {
        self.batch.biomes.chunk([pos.x, pos.z]).cloned()
    }

    pub fn completed_stage(
        &self,
        pos: native_world::loaded::ChunkPos,
    ) -> Option<native_session::worldgen::schedule::GenerationStage> {
        self.batch.schedule.completed(pos)
    }

    pub fn require_ready_features(&self, pos: native_world::loaded::ChunkPos) -> Result<()> {
        use native_session::worldgen::schedule::GenerationStage;
        self.batch.require_unfinished_features(pos)?;
        ensure!(
            self.batch.schedule.ready(pos, GenerationStage::Features),
            "features dependencies are incomplete"
        );
        Ok(())
    }

    /// Ready unfinished centers only; the caller must supply and later verify
    /// the actual Java execution order before results can be published.
    pub fn ready_features(&self) -> Vec<native_world::loaded::ChunkPos> {
        use native_session::worldgen::schedule::GenerationStage;
        self.unfinished_features()
            .into_iter()
            .filter(|&position| {
                self.batch
                    .schedule
                    .ready(position, GenerationStage::Features)
            })
            .collect()
    }

    /// Distinguish unfinished centers from an empty ready list when a terrain
    /// prerequisite is still unavailable. Saved completed centers are not rerun.
    pub fn unfinished_features(&self) -> Vec<native_world::loaded::ChunkPos> {
        use native_session::worldgen::schedule::GenerationStage;
        self.batch
            .schedule
            .requirements()
            .filter_map(|(position, required)| {
                (required >= GenerationStage::Features
                    && self.batch.require_unfinished_features(position).is_ok()
                    && self
                        .batch
                        .schedule
                        .completed(position)
                        .is_some_and(|stage| stage < GenerationStage::Features))
                .then_some(position)
            })
            .collect()
    }

    pub fn feature_order(&self) -> &[native_world::loaded::ChunkPos] {
        &self.feature_order
    }
}

impl LiveGenerationTerrainStageDraft {
    pub fn validate_checked(
        self,
        check: impl FnMut() -> Result<()>,
    ) -> Result<LiveGenerationTerrainStage> {
        Ok(LiveGenerationTerrainStage {
            identity: self.identity,
            terrain: self.terrain.validate_checked(check)?,
        })
    }
}

impl LiveGenerationTerrainStage {
    pub fn block(&self, x: i32, y: i32, z: i32) -> Option<&BlockState> {
        self.terrain.block(x, y, z)
    }

    pub fn biome(&self, x: i32, y: i32, z: i32) -> Option<&str> {
        self.terrain.biome(x, y, z)
    }
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

/// One deterministic spawn attempt stream tied to terrain, light, packs and
/// the caller's current clock snapshot. Unresolved attempts cannot be skipped.
pub(crate) struct ResumedCreaturePlan {
    identity: Arc<()>,
    revision: Arc<()>,
    light_revision: Arc<()>,
    position: native_world::loaded::ChunkPos,
    clocks: BTreeMap<ResourceId, i64>,
    planner: native_resources::worldgen_mob_spawner::ChunkCreatureSpawner,
    finished: bool,
    probe: Option<native_session::worldgen::GeneratedCreatureProbe>,
    construction: Option<native_resources::worldgen_mob_geometry::CreatureSpawnGeometry>,
    border: Option<native_session::respawn::RespawnBorder>,
    rule_light: Option<native_world::generation_light::VisibleLightSnapshot>,
    random: native_resources::worldgen_region_random::RegionRandom,
    group_id: Option<u64>,
    sheep_group: native_resources::worldgen_region_random::AnimalGroupData,
    horse_group: native_resources::worldgen_region_random::HorseGroupData,
    donkey_group: native_resources::worldgen_region_random::DonkeyGroupData,
    constructed: Option<native_session::worldgen::FreshCreatureConstruction>,
    pose: Option<native_session::worldgen::GeneratedCreaturePose>,
    instance_passed: bool,
    finalizable: bool,
    additions: Vec<native_session::worldgen::GeneratedEntityAddition>,
    used_uuids: BTreeSet<[i32; 4]>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ResumedCreatureCheck {
    Pending,
    Rejected,
    NeedsConstruction(native_resources::worldgen_mob_geometry::CreatureSpawnGeometry),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ResumedCreatureInstance {
    Rejected,
    NeedsFinalization(native_session::worldgen::GeneratedCreaturePose),
}

fn supports_generated_creature(id: &ResourceId) -> bool {
    id.namespace == "minecraft"
        && matches!(
            id.path.as_str(),
            "sheep" | "horse" | "donkey" | "pig" | "cow" | "chicken" | "turtle"
        )
}

pub(crate) struct ResumedPlannedSpawn {
    lit: ResumedLitDraft,
    clocks: BTreeMap<ResourceId, i64>,
    border: Option<native_session::respawn::RespawnBorder>,
}

impl ResumedPlannedSpawn {
    pub fn document(&self) -> &NamedTag {
        self.lit.document()
    }
    pub fn matches_source(&self, source: &NamedTag) -> Result<bool> {
        self.lit.matches_source(source)
    }
    pub fn matches_state(
        &self,
        world: &ResumedTerrainWorld,
        light: &ResumedStagedLight,
        spawn_mobs: bool,
        clocks: &BTreeMap<ResourceId, i64>,
        border: Option<native_session::respawn::RespawnBorder>,
    ) -> bool {
        spawn_mobs
            && self.clocks == *clocks
            && self.border == border
            && self.lit.matches_state(world, light)
    }
}

impl ResumedCreaturePlan {
    fn clear_candidate(&mut self) {
        self.probe = None;
        self.construction = None;
        self.constructed = None;
        self.pose = None;
        self.instance_passed = false;
        self.finalizable = false;
    }

    fn matches_state(
        &self,
        world: &ResumedTerrainWorld,
        light: &ResumedStagedLight,
        spawn_mobs: bool,
        clocks: &BTreeMap<ResourceId, i64>,
    ) -> bool {
        spawn_mobs
            && self.clocks == *clocks
            && matches_resumed_revision(&self.identity, &self.revision, world)
            && matches_resumed_revision(&light.identity, &light.revision, world)
            && Arc::ptr_eq(&self.light_revision, &light.light_revision)
    }

    /// At most one group per call. An attempt is retained until future code
    /// can execute its collision, spawn rules and entity construction.
    pub fn advance(
        &mut self,
        world: &ResumedTerrainWorld,
        light: &ResumedStagedLight,
        spawn_mobs: bool,
        clocks: &BTreeMap<ResourceId, i64>,
    ) -> Result<native_resources::worldgen_mob_spawner::CreatureSpawnProgress> {
        ensure!(
            self.matches_state(world, light, spawn_mobs, clocks),
            "creature plan is stale"
        );
        self.advance_planner()
    }

    fn advance_planner(
        &mut self,
    ) -> Result<native_resources::worldgen_mob_spawner::CreatureSpawnProgress> {
        let progress = self.planner.next(1)?;
        if matches!(
            progress,
            native_resources::worldgen_mob_spawner::CreatureSpawnProgress::Attempt(_)
        ) {
            let id = self
                .planner
                .group_id()
                .context("creature group is missing")?;
            if self.group_id != Some(id) {
                self.group_id = Some(id);
                self.sheep_group = Default::default();
                self.horse_group = Default::default();
                self.donkey_group = Default::default();
            }
        }
        self.finished = matches!(
            progress,
            native_resources::worldgen_mob_spawner::CreatureSpawnProgress::Finished
        );
        Ok(progress)
    }
}

/// Candidate stream for a captured mixed terrain and light revision. Entity
/// construction, collision, live rules and publication remain separate.
pub(crate) struct CapturedCreaturePlan {
    inner: ResumedCreaturePlan,
}

impl CapturedCreaturePlan {
    fn matches_state(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        spawn_mobs: bool,
        clocks: &BTreeMap<ResourceId, i64>,
    ) -> bool {
        spawn_mobs
            && self.inner.clocks == *clocks
            && Arc::ptr_eq(&self.inner.identity, &batch.identity)
            && Arc::ptr_eq(&self.inner.revision, &batch.revision)
            && Arc::ptr_eq(&light.identity, &batch.identity)
            && Arc::ptr_eq(&light.revision, &batch.revision)
            && Arc::ptr_eq(&self.inner.light_revision, &light.light_revision)
            && light.activated.contains(&self.inner.position)
            && batch.schedule.ready(
                self.inner.position,
                native_session::worldgen::schedule::GenerationStage::Spawn,
            )
            && !light
                .completed_empty_spawns
                .contains_key(&self.inner.position)
            && !light
                .completed_creature_spawns
                .contains_key(&self.inner.position)
    }

    /// Advance at most one group, retaining any unresolved candidate.
    pub fn advance(
        &mut self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        spawn_mobs: bool,
        clocks: &BTreeMap<ResourceId, i64>,
    ) -> Result<native_resources::worldgen_mob_spawner::CreatureSpawnProgress> {
        ensure!(
            self.matches_state(batch, light, spawn_mobs, clocks),
            "creature plan is stale"
        );
        self.inner.advance_planner()
    }
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
    fn structure_registry(&mut self) -> Result<Arc<BTreeSet<ResourceId>>> {
        if self.structure_registry.is_none() {
            let ids = self
                .resources
                .list_data("worldgen/structure")?
                .into_iter()
                .collect();
            self.structure_registry = Some(Arc::new(ids));
        }
        Ok(Arc::clone(
            self.structure_registry
                .as_ref()
                .expect("structure registry initialized"),
        ))
    }
    /// Start a fresh engine for a frozen mixed batch. The caller must still
    /// establish that no saved or live engine state contributes to the area.
    pub fn begin_captured_initial_light(
        &self,
        batch: &CapturedTerrainBatch,
    ) -> Result<CapturedInitialLight> {
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity),
            "terrain belongs to another generation capture"
        );
        let captured = self.context.inputs().sources().chunks();
        for (position, source) in &batch.sources {
            let expected = captured
                .get(position)
                .context("light terrain source was not captured")?;
            match (&expected.terrain, source) {
                (None, None) => {}
                (Some(saved), Some(source)) if Arc::ptr_eq(&saved.document, source) => {
                    reject_saved_lighting(saved)?
                }
                _ => bail!("light terrain source differs"),
            }
        }
        Ok(CapturedInitialLight {
            identity: Arc::clone(&batch.identity),
            revision: Arc::clone(&batch.revision),
            tracker: native_session::worldgen::FreshGenerationLightTracker::new(
                &batch.terrain,
                self.lighting.has_skylight,
            )?,
            initialized: BTreeSet::new(),
        })
    }

    /// Register one feature-complete chunk in the batch's own schedule.
    pub fn initialize_captured_light(
        &self,
        batch: &mut CapturedTerrainBatch,
        light: &mut CapturedInitialLight,
        position: native_world::loaded::ChunkPos,
    ) -> Result<()> {
        use native_session::worldgen::schedule::GenerationStage;
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity)
                && Arc::ptr_eq(&light.identity, &batch.identity)
                && Arc::ptr_eq(&light.revision, &batch.revision),
            "light belongs to another terrain revision"
        );
        ensure!(
            batch.feature_entity_sources.completed.contains(&position),
            "light chunk has not completed features"
        );
        ensure!(
            batch
                .schedule
                .ready(position, GenerationStage::InitializeLight),
            "light initialization dependencies are incomplete"
        );
        let registry = &self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?
            .blocks;
        batch.schedule.run_light_initialization(
            &batch.terrain,
            position,
            &mut light.tracker,
            registry,
        )?;
        light.initialized.insert(position);
        Ok(())
    }

    /// Freeze section registrations before light propagation.
    pub fn seal_captured_light(
        &self,
        batch: &CapturedTerrainBatch,
        light: CapturedInitialLight,
    ) -> Result<CapturedStagedLight> {
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity)
                && Arc::ptr_eq(&light.identity, &batch.identity)
                && Arc::ptr_eq(&light.revision, &batch.revision),
            "light belongs to another terrain revision"
        );
        ensure!(!light.initialized.is_empty(), "no light chunks initialized");
        Ok(CapturedStagedLight {
            identity: light.identity,
            revision: light.revision,
            engine: native_session::worldgen::StagedGenerationLightEngine::new(light.tracker),
            initialized: light.initialized,
            activated: BTreeSet::new(),
            light_revision: Arc::new(()),
            completed_empty_spawns: BTreeMap::new(),
            completed_creature_spawns: BTreeMap::new(),
        })
    }

    /// Propagate one ready chunk after its complete 3×3 input is initialized.
    /// The visible result remains an intermediate, non-writable snapshot.
    pub fn propagate_captured_light(
        &self,
        batch: &mut CapturedTerrainBatch,
        light: &mut CapturedStagedLight,
        position: native_world::loaded::ChunkPos,
    ) -> Result<native_world::generation_light::VisibleLightSnapshot> {
        use native_session::worldgen::schedule::GenerationStage;
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity)
                && Arc::ptr_eq(&light.identity, &batch.identity)
                && Arc::ptr_eq(&light.revision, &batch.revision),
            "light belongs to another terrain revision"
        );
        ensure!(
            initialized_light_neighborhood(&light.initialized, position),
            "light neighborhood is not initialized"
        );
        ensure!(
            batch.schedule.ready(position, GenerationStage::Light),
            "light dependencies are incomplete"
        );
        let registry = &self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?
            .blocks;
        let visible =
            batch
                .schedule
                .run_light(&batch.terrain, position, &mut light.engine, registry)?;
        light.activated.insert(position);
        light.light_revision = Arc::new(());
        Ok(visible)
    }

    /// Build a non-writable LIGHT-stage draft for a captured absent chunk.
    /// Entity/POI completeness, live state, clocks and save dependencies must
    /// still be established independently.
    pub fn draft_captured_new_lit_chunk(
        &mut self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        position: native_persistence::world::ChunkPos,
        last_update: i64,
    ) -> Result<CapturedNewLitDraft> {
        use native_session::worldgen::schedule::GenerationStage;
        let key = native_world::loaded::ChunkPos {
            x: position.x,
            z: position.z,
        };
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity)
                && Arc::ptr_eq(&light.identity, &batch.identity)
                && Arc::ptr_eq(&light.revision, &batch.revision),
            "light belongs to another terrain revision"
        );
        ensure!(
            !self.generate_structures,
            "new structure data is not prepared"
        );
        ensure!(
            batch.generated_starts.is_empty(),
            "generated structure starts are not prepared for saving"
        );
        ensure!(
            light.activated.contains(&key)
                && batch
                    .schedule
                    .completed(key)
                    .is_some_and(|stage| stage >= GenerationStage::Light),
            "light chunk has not been propagated"
        );
        let captured = self
            .context
            .inputs()
            .sources()
            .chunks()
            .get(&position)
            .context("terrain source was not captured")?;
        ensure!(
            captured.terrain.is_none() && matches!(batch.sources.get(&position), Some(None)),
            "terrain source is not absent"
        );
        let entity_sidecar = captured
            .entities
            .as_ref()
            .map(|value| Arc::clone(&value.document));
        let biome_ids: BTreeSet<_> = self
            .resources
            .list_data("worldgen/biome")?
            .into_iter()
            .collect();
        let assets = self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?;
        let snapshot = batch.terrain.snapshot();
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
        let visible = light
            .engine
            .snapshot(&batch.terrain, [key], &assets.blocks)?;
        let sections = native_persistence::generated_section::merge_generated_light_sections(
            &sections,
            &visible,
            key,
            self.lighting.min_y,
            self.lighting.height,
            self.lighting.has_skylight,
        )?;
        let heightmaps = batch.terrain.generated_heightmap_data(key)?;
        let Tag::Compound(maps) = &heightmaps else {
            bail!("generated heightmaps are invalid")
        };
        ensure!(
            maps.iter()
                .any(|(name, _)| name.matches("WORLD_SURFACE_WG"))
                && maps.iter().any(|(name, _)| name.matches("OCEAN_FLOOR_WG")),
            "generated world-generation heightmaps are missing"
        );
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
        let marks = batch
            .terrain
            .postprocessing_marks_to_date(key)
            .context("generation postprocessing is missing")?;
        let postprocessing = native_persistence::postprocessing::encode(&marks);
        let block_entities = batch
            .terrain
            .generated_block_entity_data(key, &assets.blocks)?;
        let entities = batch.terrain.recorded_generated_entities(key)?.to_vec();
        let mut document = native_persistence::generated_chunk::assemble_spawn_chunk(
            native_persistence::generated_chunk::SpawnChunkComponents {
                position: key,
                min_y: self.lighting.min_y,
                height: self.lighting.height,
                last_update,
                sections,
                heightmaps,
                block_ticks,
                fluid_ticks,
                postprocessing,
                block_entities,
                entities,
            },
        )?;
        let references = batch
            .generated_references
            .get(&key)
            .context("generated structure references are missing")?;
        merge_generated_structure_references(&mut document, references)?;
        replace_unique_root_field(
            &mut document,
            "Status",
            Tag::String("minecraft:light".into()),
        )?;
        native_persistence::nbt::write(&document)?;
        Ok(CapturedNewLitDraft {
            identity: Arc::clone(&batch.identity),
            revision: Arc::clone(&batch.revision),
            light_revision: Arc::clone(&light.light_revision),
            document,
            entity_sidecar,
            feature_entity_coverage: batch.feature_entity_sources.covers(key),
        })
    }

    /// Combine generated features and light with a captured saved protochunk.
    /// Its status remains unchanged and the result does not authorize a save.
    pub fn draft_captured_saved_lit_chunk(
        &mut self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        position: native_persistence::world::ChunkPos,
    ) -> Result<CapturedSavedLitDraft> {
        use native_session::worldgen::schedule::GenerationStage;
        let key = native_world::loaded::ChunkPos {
            x: position.x,
            z: position.z,
        };
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity)
                && Arc::ptr_eq(&light.identity, &batch.identity)
                && Arc::ptr_eq(&light.revision, &batch.revision),
            "light belongs to another terrain revision"
        );
        ensure!(
            !self.generate_structures,
            "new structure data is not prepared"
        );
        ensure!(
            batch.generated_starts.is_empty(),
            "generated structure starts are not prepared for saving"
        );
        ensure!(
            light.activated.contains(&key)
                && batch
                    .schedule
                    .completed(key)
                    .is_some_and(|stage| stage >= GenerationStage::Light)
                && batch.feature_entity_sources.completed.contains(&key),
            "feature or light generation is incomplete"
        );
        let captured = self
            .context
            .inputs()
            .sources()
            .chunks()
            .get(&position)
            .context("terrain source was not captured")?;
        let saved = captured
            .terrain
            .as_ref()
            .context("saved terrain source missing")?;
        validate_empty_preterrain_target(saved, true)?;
        reject_saved_lighting(saved)?;
        let source = Arc::clone(&saved.document);
        ensure!(
            matches!(batch.sources.get(&position), Some(Some(value)) if Arc::ptr_eq(value, &source)),
            "captured terrain source differs"
        );
        let entity_sidecar = captured
            .entities
            .as_ref()
            .map(|value| Arc::clone(&value.document));
        let source_nbt = native_persistence::nbt::write(&source)?;
        let mut document =
            self.populate_saved_feature_document(&batch.terrain, key, &source, (*source).clone())?;
        if let Some(references) = batch.generated_references.get(&key) {
            merge_generated_structure_references(&mut document, references)?;
        }
        let registry = &self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?
            .blocks;
        let visible = light.engine.snapshot(&batch.terrain, [key], registry)?;
        let sections =
            unique_root_field(&document, "sections")?.context("feature sections missing")?;
        let sections = native_persistence::generated_section::merge_generated_light_sections(
            sections,
            &visible,
            key,
            self.lighting.min_y,
            self.lighting.height,
            self.lighting.has_skylight,
        )?;
        replace_unique_root_field(&mut document, "sections", sections)?;
        native_persistence::nbt::write(&document)?;
        Ok(CapturedSavedLitDraft {
            identity: Arc::clone(&batch.identity),
            revision: Arc::clone(&batch.revision),
            light_revision: Arc::clone(&light.light_revision),
            source_nbt,
            document,
            entity_sidecar,
            feature_entity_coverage: batch.feature_entity_sources.covers(key),
        })
    }

    /// Complete the mixed chunk's spawn stage when original mobs are disabled.
    /// The caller must capture the rule from the live session and recheck it
    /// before publishing the resulting world state.
    pub fn complete_captured_spawn_without_mobs(
        &self,
        batch: &mut CapturedTerrainBatch,
        light: &mut CapturedStagedLight,
        position: native_world::loaded::ChunkPos,
        spawn_mobs: bool,
    ) -> Result<()> {
        use native_session::worldgen::schedule::GenerationStage;
        self.validate_captured_spawn(batch, light, position)?;
        let disabled = self
            .generator
            .disable_mob_generation()
            .context("noise settings are missing disable_mob_generation")?;
        ensure!(
            disabled || !spawn_mobs,
            "world-generation mob spawning needs a complete spawn context"
        );
        batch.schedule.complete(position, GenerationStage::Spawn)?;
        light.completed_empty_spawns.insert(position, spawn_mobs);
        Ok(())
    }

    fn validate_captured_spawn(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        position: native_world::loaded::ChunkPos,
    ) -> Result<()> {
        use native_session::worldgen::schedule::GenerationStage;
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity)
                && Arc::ptr_eq(&light.identity, &batch.identity)
                && Arc::ptr_eq(&light.revision, &batch.revision),
            "spawn belongs to another terrain revision"
        );
        ensure!(
            light.activated.contains(&position),
            "spawn chunk has not been lit"
        );
        ensure!(
            batch.schedule.ready(position, GenerationStage::Spawn),
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
                        batch
                            .schedule
                            .completed(neighbor)
                            .is_some_and(|stage| stage >= needed),
                        "spawn dependencies are incomplete"
                    );
                }
            }
        }
        ensure!(
            !light.completed_empty_spawns.contains_key(&position)
                && !light.completed_creature_spawns.contains_key(&position),
            "spawn already completed"
        );
        Ok(())
    }

    /// Construct a non-writable spawn draft for either captured source kind.
    pub fn draft_captured_empty_spawn(
        &mut self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        position: native_persistence::world::ChunkPos,
        spawn_mobs: bool,
        last_update: i64,
    ) -> Result<CapturedEmptySpawnDraft> {
        use native_session::worldgen::schedule::GenerationStage;
        let key = native_world::loaded::ChunkPos {
            x: position.x,
            z: position.z,
        };
        ensure!(
            light.completed_empty_spawns.get(&key) == Some(&spawn_mobs)
                && batch
                    .schedule
                    .completed(key)
                    .is_some_and(|stage| stage >= GenerationStage::Spawn),
            "spawn rule or stage differs"
        );
        let mut lit = match batch
            .sources
            .get(&position)
            .context("terrain source was not captured")?
        {
            None => CapturedLitChunkDraft::New(self.draft_captured_new_lit_chunk(
                batch,
                light,
                position,
                last_update,
            )?),
            Some(_) => CapturedLitChunkDraft::Saved(
                self.draft_captured_saved_lit_chunk(batch, light, position)?,
            ),
        };
        let document = match &mut lit {
            CapturedLitChunkDraft::New(draft) => &mut draft.document,
            CapturedLitChunkDraft::Saved(draft) => &mut draft.document,
        };
        replace_unique_root_field(document, "Status", Tag::String("minecraft:spawn".into()))?;
        native_persistence::nbt::write(document)?;
        Ok(CapturedEmptySpawnDraft {
            lit,
            position: key,
            spawn_mobs,
        })
    }

    /// Convert a spawn-complete chunk's existing heightmaps as the Java
    /// LevelChunk constructor does. No terrain, entity or world write is made.
    pub fn convert_captured_heightmaps(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        spawn: &CapturedEmptySpawnDraft,
    ) -> Result<CapturedConvertedHeightmaps> {
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity)
                && spawn.matches_state(batch, light, spawn.spawn_mobs),
            "spawn draft belongs to another generation revision"
        );
        let source = batch
            .sources
            .get(&native_persistence::world::ChunkPos {
                x: spawn.position.x,
                z: spawn.position.z,
            })
            .context("terrain source was not captured")?;
        ensure!(
            spawn.matches_source(source.as_deref())?,
            "spawn source differs"
        );
        self.convert_spawn_heightmaps(batch, light, spawn.position, spawn.spawn_mobs)
    }

    /// Convert heightmaps for a spawn containing planned creatures, retaining
    /// its clock, border and completion identity for later validation.
    pub fn convert_captured_planned_heightmaps(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        spawn: &CapturedPlannedSpawn,
        clocks: &BTreeMap<ResourceId, i64>,
        border: Option<native_session::respawn::RespawnBorder>,
    ) -> Result<CapturedPlannedConvertedHeightmaps> {
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity)
                && spawn.matches_state(batch, light, true, clocks, border),
            "spawn draft belongs to another generation revision"
        );
        let source = batch
            .sources
            .get(&native_persistence::world::ChunkPos {
                x: spawn.position.x,
                z: spawn.position.z,
            })
            .context("terrain source was not captured")?;
        ensure!(
            spawn.matches_source(source.as_deref())?,
            "spawn source differs"
        );
        Ok(CapturedPlannedConvertedHeightmaps {
            proof: self.convert_spawn_heightmaps(batch, light, spawn.position, true)?,
            completion: Arc::clone(&spawn.completion),
            clocks: spawn.clocks.clone(),
            border: spawn.border,
        })
    }

    fn convert_spawn_heightmaps(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        spawn_position: native_world::loaded::ChunkPos,
        spawn_mobs: bool,
    ) -> Result<CapturedConvertedHeightmaps> {
        let position = native_persistence::world::ChunkPos {
            x: spawn_position.x,
            z: spawn_position.z,
        };
        let source = batch
            .sources
            .get(&position)
            .context("terrain source was not captured")?
            .clone();
        let other_fields = source
            .as_deref()
            .map(preserved_heightmap_fields)
            .transpose()?
            .unwrap_or_default();
        let bounds = native_persistence::heightmap::HeightBounds {
            min_y: self.lighting.min_y,
            max_y: self
                .lighting
                .min_y
                .checked_add(self.lighting.height)
                .context("dimension height overflow")?,
        };
        let source_maps = batch.terrain.generated_heightmap_data(spawn_position)?;
        let converted =
            native_persistence::heightmap::generated_full_heightmaps(bounds, &source_maps)?;
        let heightmaps =
            native_persistence::heightmap::FullHeightmaps::from_converted(bounds, &converted)?;
        Ok(CapturedConvertedHeightmaps {
            identity: Arc::clone(&batch.identity),
            revision: Arc::clone(&batch.revision),
            light_revision: Arc::clone(&light.light_revision),
            position: spawn_position,
            spawn_mobs,
            source,
            heightmaps,
            other_fields,
        })
    }

    /// Prepare the exact pending tick containers and ordered marks belonging
    /// to the same captured spawn result. This does not execute callbacks or
    /// publish the converted chunk.
    pub fn begin_captured_postprocessing(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        spawn: &CapturedEmptySpawnDraft,
        proof: CapturedConvertedHeightmaps,
    ) -> Result<CapturedConvertedPostprocessing> {
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity)
                && spawn.matches_state(batch, light, spawn.spawn_mobs)
                && proof.matches_state(batch, light, spawn.spawn_mobs)
                && proof.position == spawn.position,
            "postprocessing inputs belong to another generation revision"
        );
        let position = spawn.position;
        let source = batch
            .sources
            .get(&native_persistence::world::ChunkPos {
                x: position.x,
                z: position.z,
            })
            .context("terrain source was not captured")?;
        ensure!(
            proof.matches_source(source.as_deref())? && spawn.matches_source(source.as_deref())?,
            "postprocessing source differs"
        );
        let converted = self.convert_spawn_postprocessing(batch, position, spawn.document())?;
        Ok(CapturedConvertedPostprocessing { proof, converted })
    }

    /// Prepare pending ticks and callbacks without discarding creature-spawn
    /// provenance or publishing the converted chunk.
    #[allow(clippy::too_many_arguments)]
    pub fn begin_captured_planned_postprocessing(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        spawn: &CapturedPlannedSpawn,
        proof: CapturedPlannedConvertedHeightmaps,
        clocks: &BTreeMap<ResourceId, i64>,
        border: Option<native_session::respawn::RespawnBorder>,
    ) -> Result<CapturedPlannedConvertedPostprocessing> {
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity)
                && spawn.matches_state(batch, light, true, clocks, border)
                && proof.matches_state(batch, light, true, clocks, border)
                && proof.proof.position == spawn.position
                && Arc::ptr_eq(&proof.completion, &spawn.completion),
            "postprocessing inputs belong to another generation revision"
        );
        let source = batch
            .sources
            .get(&native_persistence::world::ChunkPos {
                x: spawn.position.x,
                z: spawn.position.z,
            })
            .context("terrain source was not captured")?;
        ensure!(
            proof.matches_source(source.as_deref())? && spawn.matches_source(source.as_deref())?,
            "postprocessing source differs"
        );
        let converted =
            self.convert_spawn_postprocessing(batch, spawn.position, spawn.document())?;
        Ok(CapturedPlannedConvertedPostprocessing { proof, converted })
    }

    fn convert_spawn_postprocessing(
        &self,
        batch: &CapturedTerrainBatch,
        position: native_world::loaded::ChunkPos,
        document: &NamedTag,
    ) -> Result<native_session::worldgen::postprocessing::ConvertedChunkPostprocessing> {
        use native_session::worldgen::{
            chunk_ticks::{ChunkTicks, ConvertedChunkTicks, SavedTick},
            postprocessing::{ConvertedChunkPostprocessing, PostprocessingTask},
        };
        let assets = self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?;
        let upgrade =
            parse_captured_upgrade_data(document, self.lighting.height, &assets.block_ids)?;
        let world = batch.terrain.snapshot();
        let block_ticks = world
            .saved_block_ticks
            .get(&position)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let fluid_ticks = world
            .saved_fluid_ticks
            .get(&position)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let encoded_blocks =
            native_persistence::generated_ticks::encode_block_ticks(position, block_ticks, |id| {
                assets.block_ids.contains(id)
            })?;
        let encoded_fluids =
            native_persistence::generated_ticks::encode_fluid_ticks(position, fluid_ticks)?;
        ensure!(
            unique_root_field(document, "block_ticks")? == Some(&encoded_blocks)
                && unique_root_field(document, "fluid_ticks")? == Some(&encoded_fluids),
            "spawn tick data differs from generation"
        );
        let marks = batch
            .terrain
            .postprocessing_marks_to_date(position)
            .context("generation postprocessing history is missing")?;
        ensure!(
            unique_root_field(document, "PostProcessing")?
                == Some(&native_persistence::postprocessing::encode(&marks)),
            "spawn postprocessing marks differ from generation"
        );
        let blocks = block_ticks
            .iter()
            .map(|tick| SavedTick {
                kind: tick.block.clone(),
                position: tick.position,
                delay: tick.delay,
                priority: tick.priority,
            })
            .collect();
        let fluids = fluid_ticks
            .iter()
            .map(|tick| SavedTick {
                kind: tick.fluid.clone(),
                position: tick.position,
                delay: tick.delay,
                priority: tick.priority,
            })
            .collect();
        Ok(ConvertedChunkPostprocessing {
            task: PostprocessingTask::new(Arc::new(marks), position).with_upgrade_ticks(upgrade)?,
            ticks: ConvertedChunkTicks {
                blocks: ChunkTicks::from_saved(position, blocks)?,
                fluids: ChunkTicks::from_saved(position, fluids)?,
            },
        })
    }

    fn convert_postprocessing_spawn(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        spawn: CapturedPostprocessingSpawn<'_>,
        inputs: CapturedPostprocessingInputs<'_>,
    ) -> Result<(
        CapturedPostprocessingProof,
        native_session::worldgen::postprocessing::ConvertedChunkPostprocessing,
    )> {
        match spawn {
            CapturedPostprocessingSpawn::Empty(spawn) => {
                ensure!(spawn.spawn_mobs == inputs.spawn_mobs, "spawn rule differs");
                let proof = self.convert_captured_heightmaps(batch, light, spawn)?;
                let (proof, converted) = self
                    .begin_captured_postprocessing(batch, light, spawn, proof)?
                    .into_parts();
                Ok((CapturedPostprocessingProof::Empty(proof), converted))
            }
            CapturedPostprocessingSpawn::Planned(spawn) => {
                ensure!(inputs.spawn_mobs, "spawn rule differs");
                let border = spawn.border.and(inputs.border);
                let proof = self.convert_captured_planned_heightmaps(
                    batch,
                    light,
                    spawn,
                    inputs.clocks,
                    border,
                )?;
                let (proof, converted) = self
                    .begin_captured_planned_postprocessing(
                        batch,
                        light,
                        spawn,
                        proof,
                        inputs.clocks,
                        border,
                    )?
                    .into_parts();
                Ok((CapturedPostprocessingProof::Planned(proof), converted))
            }
        }
    }

    /// Convert empty and creature-bearing chunks in the supplied tick order.
    pub fn begin_captured_mixed_postprocessing_region<'a>(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        spawns: impl IntoIterator<Item = CapturedPostprocessingSpawn<'a>>,
        inputs: CapturedPostprocessingInputs<'_>,
    ) -> Result<CapturedMixedConvertedRegion> {
        let mut proofs = BTreeMap::new();
        let mut tasks = BTreeMap::new();
        let mut ticks = BTreeMap::new();
        let mut registration = Vec::new();
        let mut entity_candidates = BTreeMap::new();
        for spawn in spawns {
            let position = spawn.position();
            ensure!(!proofs.contains_key(&position), "duplicate converted chunk");
            let candidates = spawn
                .entity_candidates(self.generate_structures || !batch.treasure_starts.is_empty())?;
            let (proof, converted) =
                self.convert_postprocessing_spawn(batch, light, spawn, inputs)?;
            ensure!(
                proof.matches_state(batch, light, inputs),
                "spawn inputs differ"
            );
            proofs.insert(position, proof);
            entity_candidates.insert(position, candidates);
            tasks.insert(position, converted.task);
            ticks.insert(position, converted.ticks);
            registration.push(position);
        }
        ensure!(!proofs.is_empty(), "no converted chunks");
        Ok(CapturedMixedConvertedRegion {
            proofs,
            entity_candidates,
            region: native_session::worldgen::postprocessing::ConvertedPostprocessingRegion {
                tasks,
                ticks: native_session::worldgen::chunk_ticks::RegionTicks::new_ordered(
                    ticks,
                    inputs.game_time,
                    inputs.next_sub_tick,
                    &registration,
                )?,
            },
        })
    }

    /// Run callbacks on private terrain; no changes are saved or published.
    pub fn run_captured_mixed_postprocessing(
        &mut self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        region: CapturedMixedConvertedRegion,
        targets: impl IntoIterator<Item = native_world::loaded::ChunkPos>,
        inputs: CapturedPostprocessingInputs<'_>,
    ) -> Result<CapturedMixedPostprocessingStage> {
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity)
                && region.matches_state(batch, light, inputs),
            "captured postprocessing inputs changed"
        );
        let mut selected = BTreeSet::new();
        for position in targets {
            ensure!(selected.insert(position), "duplicate postprocessing target");
            ensure!(
                region.proofs.contains_key(&position),
                "postprocessing target is not converted"
            );
        }
        ensure!(!selected.is_empty(), "no postprocessing targets");
        let positions = region.proofs.keys().copied().collect::<Vec<_>>();
        let heightmaps = region
            .proofs
            .iter()
            .map(|(&pos, proof)| (pos, proof.heightmaps().clone()))
            .collect();
        let assets = self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?;
        let visible = light
            .engine
            .snapshot(&batch.terrain, positions, &assets.blocks)?;
        let mut stage = batch.terrain.begin_captured_postprocessing(
            &mut self.resources,
            &assets.block_ids,
            &assets.blocks,
            region.region,
            heightmaps,
            Some(visible),
        )?;
        batch
            .terrain
            .bind_captured_postprocessing_entity_candidates(&mut stage, region.entity_candidates)?;
        stage.set_water_source_conversion(inputs.water_source_conversion)?;
        stage.select_initial_targets(selected)?;
        Ok(CapturedMixedPostprocessingStage {
            proofs: region.proofs,
            stage,
            game_time: inputs.game_time,
            next_sub_tick: inputs.next_sub_tick,
            water_source_conversion: inputs.water_source_conversion,
            waiting_neighbor: None,
            retry_target: None,
        })
    }

    /// Supply the precise missing neighbor without advancing the interrupted callback.
    pub fn add_captured_mixed_postprocessing_neighbor(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        stage: &mut CapturedMixedPostprocessingStage,
        spawn: CapturedPostprocessingSpawn<'_>,
        inputs: CapturedPostprocessingInputs<'_>,
    ) -> Result<()> {
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity)
                && stage.matches_state(batch, light, inputs),
            "captured postprocessing inputs changed"
        );
        let position = spawn.position();
        ensure!(
            stage.waiting_neighbor == Some(position),
            "chunk was not the requested neighbor"
        );
        ensure!(
            !stage.proofs.contains_key(&position),
            "postprocessing chunk is already registered"
        );
        let candidates = spawn
            .entity_candidates(self.generate_structures || !batch.treasure_starts.is_empty())?;
        let (proof, converted) = self.convert_postprocessing_spawn(batch, light, spawn, inputs)?;
        ensure!(
            proof.matches_state(batch, light, inputs),
            "neighbor spawn inputs differ"
        );
        let assets = self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?;
        batch
            .terrain
            .register_captured_postprocessing_neighbor_with_entities(
                &mut stage.stage,
                &light.engine,
                &assets.blocks,
                position,
                converted,
                proof.heightmaps().clone(),
                candidates,
            )?;
        stage.proofs.insert(position, proof);
        stage.waiting_neighbor = None;
        Ok(())
    }

    pub fn drain_captured_mixed_postprocessing_light(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        stage: &mut CapturedMixedPostprocessingStage,
        inputs: CapturedPostprocessingInputs<'_>,
    ) -> Result<bool> {
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity)
                && stage.matches_state(batch, light, inputs),
            "captured postprocessing inputs changed"
        );
        ensure!(
            stage.waiting_neighbor.is_none() && stage.retry_target.is_none(),
            "finish the interrupted callback before draining light"
        );
        let assets = self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?;
        batch.terrain.drain_captured_postprocessing_light(
            &mut stage.stage,
            &light.engine,
            &assets.blocks,
        )
    }

    /// Resume a sealed mixed pass without repeating completed callbacks.
    pub fn resume_captured_mixed_postprocessing(
        &mut self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        completed: &CapturedMixedPostprocessedRegion,
        inputs: CapturedPostprocessingInputs<'_>,
    ) -> Result<CapturedMixedPostprocessingStage> {
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity)
                && completed.matches_state(batch, light, inputs),
            "captured postprocessing inputs changed"
        );
        let assets = self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?;
        let stage = batch.terrain.resume_captured_postprocessing(
            &assets.blocks,
            &light.engine,
            &completed.region,
        )?;
        Ok(CapturedMixedPostprocessingStage {
            proofs: completed.proofs.clone(),
            stage,
            game_time: inputs.game_time,
            next_sub_tick: inputs.next_sub_tick,
            water_source_conversion: inputs.water_source_conversion,
            waiting_neighbor: None,
            retry_target: None,
        })
    }

    /// Encode terrain and drained light from the exact sealed mixed pass.
    pub fn captured_mixed_postprocessed_sections(
        &mut self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        completed: &CapturedMixedPostprocessedRegion,
        positions: impl IntoIterator<Item = native_world::loaded::ChunkPos>,
        inputs: CapturedPostprocessingInputs<'_>,
    ) -> Result<CapturedMixedPostprocessedSections> {
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity)
                && completed.matches_state(batch, light, inputs),
            "captured postprocessing inputs changed"
        );
        let mut selected = BTreeSet::new();
        for pos in positions {
            ensure!(selected.insert(pos), "duplicate postprocessed chunk");
            ensure!(
                completed.proofs.contains_key(&pos) && completed.region.chunks.contains(&pos),
                "chunk was not postprocessed"
            );
        }
        ensure!(!selected.is_empty(), "no postprocessed chunks selected");
        let biomes: BTreeSet<_> = self
            .resources
            .list_data("worldgen/biome")?
            .into_iter()
            .collect();
        let assets = self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?;
        let visible = batch.terrain.captured_postprocessed_light_snapshot(
            &completed.region,
            &light.engine,
            &assets.blocks,
            selected.iter().copied(),
        )?;
        let mut sections = BTreeMap::new();
        for pos in selected {
            let source = batch
                .sources
                .get(&native_persistence::world::ChunkPos { x: pos.x, z: pos.z })
                .context("terrain source was not captured")?;
            ensure!(
                completed.proofs[&pos].matches_source(source.as_deref())?,
                "postprocessing source differs"
            );
            let generated = native_persistence::generated_section::encode_generated_sections(
                &completed.region.world,
                pos,
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
                        .is_some_and(|id| biomes.contains(&id))
                },
            )?;
            let merged = merge_captured_postprocessed_sections(source.as_deref(), &generated)?;
            let merged = native_persistence::generated_section::merge_generated_light_sections(
                &merged,
                &visible,
                pos,
                self.lighting.min_y,
                self.lighting.height,
                self.lighting.has_skylight,
            )?;
            sections.insert(pos, merged);
        }
        Ok(CapturedMixedPostprocessedSections {
            result_identity: Arc::clone(&completed.identity),
            sections,
        })
    }

    /// Gather sealed terrain and entity candidates without claiming a safe save.
    pub fn captured_mixed_postprocessed_components(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        completed: &CapturedMixedPostprocessedRegion,
        sections: &CapturedMixedPostprocessedSections,
        inputs: CapturedPostprocessingInputs<'_>,
    ) -> Result<CapturedMixedPostprocessedComponents> {
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity)
                && sections.matches_state(completed, batch, light, inputs),
            "captured postprocessing inputs changed"
        );
        let (mut updated_sources, external_structure_sources) =
            partition_structure_sources(self.draft_captured_structure_sources(batch)?, |source| {
                sections.sections.contains_key(&source)
            });
        let assets = self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?;
        let data = batch.terrain.captured_postprocessed_chunk_data(
            &completed.region,
            &assets.blocks,
            &assets.block_ids,
            sections.sections.keys().copied(),
        )?;
        let missing_poi_positions = captured_missing_poi_positions(
            self.context.inputs().sources().chunks(),
            sections.sections.keys().copied(),
        )?;
        let missing_poi = if missing_poi_positions.is_empty() {
            BTreeMap::new()
        } else {
            batch.terrain.captured_postprocessed_missing_poi_snapshot(
                &completed.region,
                &assets.blocks,
                missing_poi_positions,
            )?
        };
        let drops_occurred = completed
            .region
            .block_drops()
            .is_some_and(|drops| drops.operations() != 0 || drops.item_chunks().next().is_some());
        let mut chunks = BTreeMap::new();
        let mut structures = BTreeMap::new();
        for (pos, data) in data {
            let proof = completed
                .proofs
                .get(&pos)
                .context("postprocessing proof is missing")?;
            let key = native_persistence::world::ChunkPos { x: pos.x, z: pos.z };
            let source = batch
                .sources
                .get(&key)
                .context("terrain source was not captured")?;
            ensure!(
                proof.matches_source(source.as_deref())?,
                "postprocessing source differs"
            );
            let captured = self
                .context
                .inputs()
                .sources()
                .chunks()
                .get(&key)
                .context("entity source was not captured")?;
            ensure!(
                match (&captured.terrain, source) {
                    (None, None) => true,
                    (Some(saved), Some(source)) => Arc::ptr_eq(&saved.document, source),
                    _ => false,
                },
                "captured entity terrain source differs"
            );
            let mut document = if let Some(draft) = updated_sources.remove(&pos) {
                draft.document
            } else {
                source.as_deref().cloned().unwrap_or_else(|| NamedTag {
                    name: "".into(),
                    value: Tag::Compound(vec![]),
                })
            };
            if let Some(references) = batch.generated_references.get(&pos) {
                merge_generated_structure_references(&mut document, references)?;
            } else {
                ensure!(
                    source.is_some(),
                    "generated structure references are missing"
                );
            }
            structures.insert(pos, unique_root_field(&document, "structures")?.cloned());
            let heightmaps =
                merge_captured_postprocessed_heightmaps(proof.other_fields(), data.heightmaps)?;
            // Block drops and their listener effects require a separate validated
            // integration. Unknown candidates must never become an empty list.
            let entity_records = if drops_occurred {
                None
            } else {
                completed
                    .region
                    .recorded_entity_candidates(pos)
                    .flatten()
                    .cloned()
            };
            chunks.insert(
                pos,
                native_session::worldgen::PostprocessedChunkComponents {
                    sections: sections
                        .sections
                        .get(&pos)
                        .context("postprocessed sections are missing")?
                        .clone(),
                    heightmaps,
                    block_ticks: data.block_ticks,
                    fluid_ticks: data.fluid_ticks,
                    block_entities: data.block_entities,
                    entity_records,
                },
            );
        }
        ensure!(
            updated_sources.is_empty(),
            "postprocessed structure source is missing"
        );
        Ok(CapturedMixedPostprocessedComponents {
            result_identity: Arc::clone(&completed.identity),
            chunks,
            structures,
            external_structure_sources,
            missing_poi,
        })
    }

    /// Prepare FULL chunk and entity-region documents without writing them.
    /// Missing entity coverage stops the whole operation. The caller must
    /// separately recheck sidecars, live state and all save dependencies.
    pub fn draft_captured_mixed_full_chunks(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        completed: &CapturedMixedPostprocessedRegion,
        components: &CapturedMixedPostprocessedComponents,
        inputs: CapturedPostprocessingInputs<'_>,
    ) -> Result<CapturedMixedFullDrafts> {
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity)
                && components.matches_state(completed, batch, light, inputs),
            "captured postprocessing inputs changed"
        );
        ensure!(
            !components.chunks.is_empty(),
            "no postprocessed chunks selected"
        );
        let assets = self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?;
        let mut chunks = BTreeMap::new();
        for (&position, parts) in &components.chunks {
            let source = batch
                .sources
                .get(&native_persistence::world::ChunkPos {
                    x: position.x,
                    z: position.z,
                })
                .context("terrain source was not captured")?;
            let proof = completed
                .proofs
                .get(&position)
                .context("postprocessing proof is missing")?;
            ensure!(
                proof.matches_source(source.as_deref())?,
                "postprocessing source differs"
            );
            if let Some(source) = source {
                parse_captured_upgrade_data(source, self.lighting.height, &assets.block_ids)?;
            }
            let structures = components
                .structures
                .get(&position)
                .context("structure data missing")?;
            let (chunk, generated_entities) = assemble_captured_mixed_full_chunk(
                position,
                self.lighting.min_y,
                self.lighting.height,
                inputs.game_time,
                source.as_deref(),
                structures.as_ref(),
                parts,
            )?;
            let sidecar = self
                .context
                .inputs()
                .sources()
                .chunks()
                .get(&native_persistence::world::ChunkPos {
                    x: position.x,
                    z: position.z,
                })
                .context("entity source was not captured")?
                .entities
                .as_ref()
                .map(|source| source.document.as_ref());
            let entity_chunk = if let Some(sidecar) = sidecar {
                let Some(Tag::List { values, .. }) = generated_entities.value.get("Entities")
                else {
                    bail!("generated entity records are missing")
                };
                merge_captured_entity_candidates(position, sidecar, values)?
            } else {
                generated_entities
            };
            chunks.insert(
                position,
                CapturedMixedFullDraft {
                    source_nbt: source
                        .as_deref()
                        .map(native_persistence::nbt::write)
                        .transpose()?,
                    source_entities_nbt: sidecar.map(native_persistence::nbt::write).transpose()?,
                    chunk,
                    entity_chunk,
                },
            );
        }
        Ok(CapturedMixedFullDrafts {
            result_identity: Arc::clone(&completed.identity),
            chunks,
        })
    }

    /// Preview captured existing POIs. Disk provenance, live tickets and all
    /// generation effects remain separate requirements for publication.
    pub fn draft_captured_mixed_existing_poi(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        completed: &CapturedMixedPostprocessedRegion,
        components: &CapturedMixedPostprocessedComponents,
        inputs: CapturedPostprocessingInputs<'_>,
        validate_type: impl FnMut(&native_persistence::nbt::JavaString) -> Result<()>,
    ) -> Result<CapturedMixedExistingPoi> {
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity)
                && components.matches_state(completed, batch, light, inputs),
            "captured postprocessing inputs changed"
        );
        let sources = self.context.inputs().sources().chunks();
        let mut selected = Vec::new();
        for &pos in components.chunks.keys() {
            let key = native_persistence::world::ChunkPos { x: pos.x, z: pos.z };
            let source = sources
                .get(&key)
                .context("captured POI source is missing")?;
            if let Some(poi) = &source.poi {
                selected.push((pos, poi));
            }
        }
        let documents = if selected.is_empty() {
            BTreeMap::new()
        } else {
            let assets = self
                .terrain_assets
                .as_ref()
                .context("terrain registry is missing")?;
            batch.terrain.captured_postprocessed_existing_poi_snapshot(
                &completed.region,
                &assets.blocks,
                selected,
                validate_type,
            )?
        };
        Ok(CapturedMixedExistingPoi {
            result_identity: Arc::clone(&completed.identity),
            documents,
        })
    }

    /// Assemble a source-bound storage plan for the selected FULL drafts.
    /// This is only an isolated candidate: callers must still account for
    /// unselected and deferred chunks, structure edits, drops, events, live
    /// entities and world clocks before publishing anything to a live world.
    pub fn stage_captured_mixed_full_save_plan(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        completed: &CapturedMixedPostprocessedRegion,
        components: &CapturedMixedPostprocessedComponents,
        drafts: &CapturedMixedFullDrafts,
        existing_poi: &CapturedMixedExistingPoi,
        inputs: CapturedPostprocessingInputs<'_>,
        region_timestamp_seconds: u32,
    ) -> Result<CapturedMixedSaveCandidate> {
        use native_persistence::save::{CapturedGeneratedChunk, WorldSavePlan};

        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity)
                && components.matches_state(completed, batch, light, inputs)
                && drafts.matches_state(completed, batch, light, inputs)
                && Arc::ptr_eq(&existing_poi.result_identity, &completed.identity),
            "captured postprocessing inputs changed"
        );
        ensure!(
            !drafts.chunks.is_empty() && drafts.chunks.keys().eq(components.chunks.keys()),
            "captured FULL draft coverage differs"
        );
        let sources = self.context.inputs().sources();
        let mut chunks = Vec::with_capacity(drafts.chunks.len());
        for (&position, draft) in &drafts.chunks {
            let key = native_persistence::world::ChunkPos {
                x: position.x,
                z: position.z,
            };
            let source = sources
                .chunks()
                .get(&key)
                .context("captured chunk source is missing")?;
            let terrain = source.terrain.as_ref().map(|chunk| chunk.document.as_ref());
            ensure!(
                draft.matches_source(terrain)?,
                "captured terrain source differs"
            );
            let entity_source = source
                .entities
                .as_ref()
                .map(|chunk| chunk.document.as_ref());
            ensure!(
                draft.matches_entity_source(entity_source)?,
                "captured entity source differs"
            );
            let (poi_source, poi) = match &source.poi {
                Some(source) => (
                    Some(source.document.as_ref()),
                    Some(
                        existing_poi
                            .documents
                            .get(&position)
                            .context("existing POI coverage is missing")?,
                    ),
                ),
                None => (
                    None,
                    components
                        .missing_poi
                        .get(&position)
                        .context("generated POI coverage is missing")?
                        .as_ref(),
                ),
            };
            let Some(Tag::List { values, .. }) = draft.entity_chunk.value.get("Entities") else {
                bail!("generated entity records are missing")
            };
            chunks.push(CapturedGeneratedChunk {
                position: key,
                source_terrain: terrain,
                source_entities: entity_source,
                source_poi: poi_source,
                terrain: &draft.chunk,
                entities: (entity_source.is_some() || !values.is_empty())
                    .then_some(&draft.entity_chunk),
                poi,
                region_timestamp_seconds,
            });
        }
        let mut plan = WorldSavePlan::new();
        plan.add_generation_context(&self.context)?;
        plan.add_captured_generated_chunks(sources.dimension().clone(), &chunks)?;
        Ok(CapturedMixedSaveCandidate { _plan: plan })
    }

    /// Prepare an isolated converted region in the caller's registration
    /// order. Its shared clock and sub-tick counter must come from the same
    /// live world and be rechecked before publishing the result.
    pub fn begin_captured_postprocessing_region<'a>(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        spawns: impl IntoIterator<Item = &'a CapturedEmptySpawnDraft>,
        game_time: i64,
        next_sub_tick: i64,
    ) -> Result<CapturedConvertedRegion> {
        let spawns: Vec<_> = spawns.into_iter().collect();
        let first = spawns.first().context("no converted chunks")?;
        let mut positions = BTreeSet::new();
        for spawn in &spawns {
            ensure!(
                positions.insert(spawn.position),
                "duplicate converted chunk"
            );
            ensure!(
                spawn.spawn_mobs == first.spawn_mobs,
                "spawn rules differ across region"
            );
        }
        let mut proofs = BTreeMap::new();
        let mut tasks = BTreeMap::new();
        let mut ticks = BTreeMap::new();
        let mut registration = Vec::with_capacity(spawns.len());
        for spawn in spawns {
            let proof = self.convert_captured_heightmaps(batch, light, spawn)?;
            let converted = self.begin_captured_postprocessing(batch, light, spawn, proof)?;
            let (proof, converted) = converted.into_parts();
            let position = spawn.position;
            proofs.insert(position, proof);
            tasks.insert(position, converted.task);
            ticks.insert(position, converted.ticks);
            registration.push(position);
        }
        Ok(CapturedConvertedRegion {
            proofs,
            region: native_session::worldgen::postprocessing::ConvertedPostprocessingRegion {
                tasks,
                ticks: native_session::worldgen::chunk_ticks::RegionTicks::new_ordered(
                    ticks,
                    game_time,
                    next_sub_tick,
                    &registration,
                )?,
            },
        })
    }

    /// Start callbacks on a private terrain snapshot, retaining the captured
    /// proofs and live rules. This does not install or save the resulting world.
    pub fn run_captured_postprocessing(
        &mut self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        region: CapturedConvertedRegion,
        spawn_mobs: bool,
        game_time: i64,
        next_sub_tick: i64,
        water_source_conversion: bool,
    ) -> Result<CapturedPostprocessingStage> {
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity)
                && region.matches_state(batch, light, spawn_mobs, game_time, next_sub_tick),
            "captured postprocessing region changed"
        );
        let (proofs, region) = region.into_parts();
        let positions: Vec<_> = proofs.keys().copied().collect();
        let heightmaps = proofs
            .iter()
            .map(|(&pos, proof)| (pos, proof.heightmaps().clone()))
            .collect();
        let assets = self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?;
        let visible = light
            .engine
            .snapshot(&batch.terrain, positions, &assets.blocks)?;
        let mut stage = batch.terrain.begin_captured_postprocessing(
            &mut self.resources,
            &assets.block_ids,
            &assets.blocks,
            region,
            heightmaps,
            Some(visible),
        )?;
        stage.set_water_source_conversion(water_source_conversion)?;
        Ok(CapturedPostprocessingStage {
            proofs,
            stage,
            game_time,
            next_sub_tick,
            water_source_conversion,
            waiting_neighbor: None,
            retry_target: None,
        })
    }

    /// Start only selected chunks while retaining the other converted chunks
    /// as deferred neighbors for later, explicitly ordered requests.
    pub fn run_captured_postprocessing_for_targets(
        &mut self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        region: CapturedConvertedRegion,
        targets: impl IntoIterator<Item = native_world::loaded::ChunkPos>,
        spawn_mobs: bool,
        game_time: i64,
        next_sub_tick: i64,
        water_source_conversion: bool,
    ) -> Result<CapturedPostprocessingStage> {
        let mut selected = BTreeSet::new();
        for position in targets {
            ensure!(selected.insert(position), "duplicate postprocessing target");
            ensure!(
                region.proofs.contains_key(&position),
                "postprocessing target is not converted"
            );
        }
        ensure!(!selected.is_empty(), "no postprocessing targets");
        let mut stage = self.run_captured_postprocessing(
            batch,
            light,
            region,
            spawn_mobs,
            game_time,
            next_sub_tick,
            water_source_conversion,
        )?;
        stage.stage.select_initial_targets(selected)?;
        Ok(stage)
    }

    /// Reopen a completed isolated pass without replaying callbacks, keeping
    /// deferred neighbors, ticks and the postprocessing light checkpoint.
    pub fn resume_captured_postprocessing(
        &mut self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        completed: &CapturedPostprocessedRegion,
        spawn_mobs: bool,
        game_time: i64,
        next_sub_tick: i64,
        water_source_conversion: bool,
    ) -> Result<CapturedPostprocessingStage> {
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity)
                && completed.matches_state(
                    batch,
                    light,
                    spawn_mobs,
                    game_time,
                    next_sub_tick,
                    water_source_conversion
                ),
            "captured postprocessing inputs changed"
        );
        let assets = self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?;
        let stage = batch.terrain.resume_captured_postprocessing(
            &assets.blocks,
            &light.engine,
            &completed.region,
        )?;
        Ok(CapturedPostprocessingStage {
            proofs: completed.proofs.clone(),
            stage,
            game_time,
            next_sub_tick,
            water_source_conversion,
            waiting_neighbor: None,
            retry_target: None,
        })
    }

    /// Encode completed blocks, biomes and a privately drained light snapshot.
    /// Other chunk components and live world state still require validation.
    pub fn captured_postprocessed_sections(
        &mut self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        completed: &CapturedPostprocessedRegion,
        positions: impl IntoIterator<Item = native_world::loaded::ChunkPos>,
        spawn_mobs: bool,
        game_time: i64,
        next_sub_tick: i64,
        water_source_conversion: bool,
    ) -> Result<CapturedPostprocessedSections> {
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity)
                && completed.matches_state(
                    batch,
                    light,
                    spawn_mobs,
                    game_time,
                    next_sub_tick,
                    water_source_conversion
                ),
            "captured postprocessing inputs changed"
        );
        let mut selected = BTreeSet::new();
        for pos in positions {
            ensure!(selected.insert(pos), "duplicate postprocessed chunk");
            ensure!(
                completed.proofs.contains_key(&pos) && completed.region.chunks.contains(&pos),
                "chunk was not postprocessed"
            );
        }
        ensure!(!selected.is_empty(), "no postprocessed chunks selected");
        let biomes: BTreeSet<_> = self
            .resources
            .list_data("worldgen/biome")?
            .into_iter()
            .collect();
        let assets = self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?;
        let visible = batch.terrain.captured_postprocessed_light_snapshot(
            &completed.region,
            &light.engine,
            &assets.blocks,
            selected.iter().copied(),
        )?;
        let mut sections = BTreeMap::new();
        for pos in selected {
            let proof = &completed.proofs[&pos];
            let source = batch
                .sources
                .get(&native_persistence::world::ChunkPos { x: pos.x, z: pos.z })
                .context("terrain source was not captured")?;
            ensure!(
                proof.matches_source(source.as_deref())?,
                "postprocessing source differs"
            );
            let generated = native_persistence::generated_section::encode_generated_sections(
                &completed.region.world,
                pos,
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
                        .is_some_and(|id| biomes.contains(&id))
                },
            )?;
            let merged = merge_captured_postprocessed_sections(source.as_deref(), &generated)?;
            let merged = native_persistence::generated_section::merge_generated_light_sections(
                &merged,
                &visible,
                pos,
                self.lighting.min_y,
                self.lighting.height,
                self.lighting.has_skylight,
            )?;
            sections.insert(pos, merged);
        }
        Ok(CapturedPostprocessedSections {
            result_identity: Arc::clone(&completed.identity),
            sections,
        })
    }

    /// Combine sections, heightmaps, ticks and block entities from the same
    /// captured pass. Entity sidecars and all save checks remain unresolved.
    pub fn captured_postprocessed_components(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        completed: &CapturedPostprocessedRegion,
        sections: &CapturedPostprocessedSections,
        spawn_mobs: bool,
        game_time: i64,
        next_sub_tick: i64,
        water_source_conversion: bool,
    ) -> Result<CapturedPostprocessedComponents> {
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity)
                && sections.matches_state(
                    completed,
                    batch,
                    light,
                    spawn_mobs,
                    game_time,
                    next_sub_tick,
                    water_source_conversion
                ),
            "captured postprocessing inputs changed"
        );
        let (mut updated_sources, external_structure_sources) =
            partition_structure_sources(self.draft_captured_structure_sources(batch)?, |source| {
                sections.sections.contains_key(&source)
            });
        let assets = self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?;
        let data = batch.terrain.captured_postprocessed_chunk_data(
            &completed.region,
            &assets.blocks,
            &assets.block_ids,
            sections.sections.keys().copied(),
        )?;
        let missing_poi_positions = captured_missing_poi_positions(
            self.context.inputs().sources().chunks(),
            sections.sections.keys().copied(),
        )?;
        let missing_poi = if missing_poi_positions.is_empty() {
            BTreeMap::new()
        } else {
            batch.terrain.captured_postprocessed_missing_poi_snapshot(
                &completed.region,
                &assets.blocks,
                missing_poi_positions,
            )?
        };
        let mut chunks = BTreeMap::new();
        let mut structures = BTreeMap::new();
        let mut generated_empty_entities = BTreeSet::new();
        for (pos, data) in data {
            let proof = completed
                .proofs
                .get(&pos)
                .context("postprocessing proof is missing")?;
            let source = batch
                .sources
                .get(&native_persistence::world::ChunkPos { x: pos.x, z: pos.z })
                .context("terrain source was not captured")?;
            ensure!(
                proof.matches_source(source.as_deref())?,
                "postprocessing source differs"
            );
            let captured = self
                .context
                .inputs()
                .sources()
                .chunks()
                .get(&native_persistence::world::ChunkPos { x: pos.x, z: pos.z })
                .context("entity source was not captured")?;
            ensure!(
                match (&captured.terrain, source) {
                    (None, None) => true,
                    (Some(saved), Some(source)) => Arc::ptr_eq(&saved.document, source),
                    _ => false,
                },
                "captured entity terrain source differs"
            );
            if captured_generated_entities_are_empty(
                source.as_deref(),
                captured
                    .entities
                    .as_ref()
                    .map(|sidecar| sidecar.document.as_ref()),
                batch.feature_entity_sources.covers(pos),
                self.generate_structures || !batch.treasure_starts.is_empty(),
                batch.terrain.recorded_generated_entities(pos)?,
                completed.region.block_drops().is_some_and(|drops| {
                    drops.operations() != 0 || drops.item_chunks().next().is_some()
                }),
            )? {
                generated_empty_entities.insert(pos);
            }
            let mut document = if let Some(draft) = updated_sources.remove(&pos) {
                draft.document
            } else {
                source.as_deref().cloned().unwrap_or_else(|| NamedTag {
                    name: "".into(),
                    value: Tag::Compound(vec![]),
                })
            };
            if let Some(references) = batch.generated_references.get(&pos) {
                merge_generated_structure_references(&mut document, references)?;
            } else {
                ensure!(
                    source.is_some(),
                    "generated structure references are missing"
                );
            }
            structures.insert(pos, unique_root_field(&document, "structures")?.cloned());
            let heightmaps =
                merge_captured_postprocessed_heightmaps(&proof.other_fields, data.heightmaps)?;
            chunks.insert(
                pos,
                native_session::worldgen::PostprocessedChunkComponents {
                    sections: sections
                        .sections
                        .get(&pos)
                        .context("postprocessed sections are missing")?
                        .clone(),
                    heightmaps,
                    block_ticks: data.block_ticks,
                    fluid_ticks: data.fluid_ticks,
                    block_entities: data.block_entities,
                    entity_records: None,
                },
            );
        }
        ensure!(
            updated_sources.is_empty(),
            "postprocessed structure source is missing"
        );
        Ok(CapturedPostprocessedComponents {
            result_identity: Arc::clone(&completed.identity),
            chunks,
            structures,
            external_structure_sources,
            missing_poi,
            generated_empty_entities,
        })
    }

    /// Supply exactly the converted neighbor requested by the most recent
    /// isolated attempt. Existing tick order and light columns are retained.
    pub fn add_captured_postprocessing_neighbor(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        stage: &mut CapturedPostprocessingStage,
        spawn: &CapturedEmptySpawnDraft,
        spawn_mobs: bool,
        game_time: i64,
        next_sub_tick: i64,
        water_source_conversion: bool,
    ) -> Result<()> {
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity)
                && stage.matches_state(
                    batch,
                    light,
                    spawn_mobs,
                    game_time,
                    next_sub_tick,
                    water_source_conversion
                ),
            "captured postprocessing inputs changed"
        );
        ensure!(
            stage.waiting_neighbor == Some(spawn.position),
            "chunk was not the requested neighbor"
        );
        ensure!(
            !stage.proofs.contains_key(&spawn.position),
            "postprocessing chunk is already registered"
        );
        let proof = self.convert_captured_heightmaps(batch, light, spawn)?;
        let converted = self.begin_captured_postprocessing(batch, light, spawn, proof)?;
        let (proof, converted) = converted.into_parts();
        ensure!(
            proof.matches_state(batch, light, spawn_mobs),
            "neighbor spawn rule differs"
        );
        let assets = self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?;
        batch.terrain.register_captured_postprocessing_neighbor(
            &mut stage.stage,
            &light.engine,
            &assets.blocks,
            spawn.position,
            converted,
            proof.heightmaps().clone(),
        )?;
        stage.proofs.insert(spawn.position, proof);
        stage.waiting_neighbor = None;
        Ok(())
    }

    /// Apply recorded light checks at an explicit caller-selected boundary.
    /// The isolated pass and its previous checkpoint remain unchanged on error.
    pub fn drain_captured_postprocessing_light(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        stage: &mut CapturedPostprocessingStage,
        spawn_mobs: bool,
        game_time: i64,
        next_sub_tick: i64,
        water_source_conversion: bool,
    ) -> Result<bool> {
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity)
                && stage.matches_state(
                    batch,
                    light,
                    spawn_mobs,
                    game_time,
                    next_sub_tick,
                    water_source_conversion
                ),
            "captured postprocessing inputs changed"
        );
        ensure!(
            stage.waiting_neighbor.is_none() && stage.retry_target.is_none(),
            "finish the interrupted callback before draining light"
        );
        let assets = self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?;
        batch.terrain.drain_captured_postprocessing_light(
            &mut stage.stage,
            &light.engine,
            &assets.blocks,
        )
    }

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
        self.validate_resumed_spawn(world, schedule, light, position)?;
        let disabled = self
            .generator
            .disable_mob_generation()
            .context("noise settings are missing disable_mob_generation")?;
        ensure!(
            disabled || !spawn_mobs,
            "world-generation mob spawning needs a complete spawn context"
        );
        schedule.complete(position, GenerationStage::Spawn)?;
        light.completed_empty_spawns.insert(position, spawn_mobs);
        Ok(())
    }

    fn validate_resumed_spawn(
        &self,
        world: &ResumedTerrainWorld,
        schedule: &GenerationSchedule,
        light: &ResumedStagedLight,
        position: native_world::loaded::ChunkPos,
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
        Ok(())
    }

    /// Resolve the active dimension, biome and clock attributes and prepare a
    /// deterministic candidate stream. The caller must track the live rule,
    /// clocks, data-pack selection and other world changes independently.
    pub fn plan_resumed_creatures(
        &mut self,
        world: &ResumedTerrainWorld,
        schedule: &GenerationSchedule,
        light: &ResumedStagedLight,
        context: &SavedDecorationContext,
        spawn_mobs: bool,
        clocks: &BTreeMap<ResourceId, i64>,
    ) -> Result<ResumedCreaturePlan> {
        ensure!(
            Arc::ptr_eq(&self.identity, &context.identity),
            "biomes belong to another generation capture"
        );
        let position = native_world::loaded::ChunkPos {
            x: context.position.x,
            z: context.position.z,
        };
        self.validate_resumed_spawn(world, schedule, light, position)?;
        ensure!(spawn_mobs, "use the disabled-mob spawn path");
        ensure!(
            !self
                .generator
                .disable_mob_generation()
                .context("noise settings are missing disable_mob_generation")?,
            "generator disables mob generation"
        );
        self.build_creature_plan(
            position,
            &context.biomes,
            &world.revision,
            &light.light_revision,
            clocks,
        )
    }

    /// Prepare the same world-generation creature stream for mixed saved and
    /// newly generated terrain. No candidates are skipped or entities written.
    pub fn plan_captured_creatures(
        &mut self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        position: native_world::loaded::ChunkPos,
        spawn_mobs: bool,
        clocks: &BTreeMap<ResourceId, i64>,
    ) -> Result<CapturedCreaturePlan> {
        self.validate_captured_spawn(batch, light, position)?;
        ensure!(spawn_mobs, "use the disabled-mob spawn path");
        ensure!(
            !self
                .generator
                .disable_mob_generation()
                .context("noise settings are missing disable_mob_generation")?,
            "generator disables mob generation"
        );
        Ok(CapturedCreaturePlan {
            inner: self.build_creature_plan(
                position,
                &batch.biomes,
                &batch.revision,
                &light.light_revision,
                clocks,
            )?,
        })
    }

    /// Complete one captured spawn stage without publishing its entities.
    /// `used_uuids` also covers earlier chunks in this generation pass. A
    /// caller must still check identities outside the captured generation area.
    /// On error, discard the batch and light: a draft failure or cancellation
    /// can occur after the spawn stage has been recorded.
    #[allow(clippy::too_many_arguments)]
    pub fn complete_captured_spawn(
        &mut self,
        batch: &mut CapturedTerrainBatch,
        light: &mut CapturedStagedLight,
        position: native_world::loaded::ChunkPos,
        inputs: CapturedCreatureSpawnInputs<'_>,
        used_uuids: &mut BTreeSet<[i32; 4]>,
        construct: &mut impl FnMut(
            &ResourceId,
        )
            -> Result<native_session::worldgen::FreshCreatureConstruction>,
        check: &mut impl FnMut() -> Result<()>,
    ) -> Result<CapturedCompletedSpawn> {
        use native_resources::worldgen_mob_spawner::CreatureSpawnProgress;

        check()?;
        ensure!(inputs.border.is_valid(), "invalid mob spawn border");
        let disabled = self
            .generator
            .disable_mob_generation()
            .context("noise settings are missing disable_mob_generation")?;
        if !inputs.spawn_mobs || disabled {
            self.complete_captured_spawn_without_mobs(batch, light, position, inputs.spawn_mobs)?;
            let spawn = self.draft_captured_empty_spawn(
                batch,
                light,
                native_persistence::world::ChunkPos {
                    x: position.x,
                    z: position.z,
                },
                inputs.spawn_mobs,
                inputs.last_update,
            )?;
            check()?;
            return Ok(CapturedCompletedSpawn::Empty(spawn));
        }

        let mut plan = self.plan_captured_creatures(batch, light, position, true, inputs.clocks)?;
        loop {
            check()?;
            let attempt = match plan.advance(batch, light, true, inputs.clocks)? {
                CreatureSpawnProgress::Pending => continue,
                CreatureSpawnProgress::Finished => break,
                CreatureSpawnProgress::Attempt(attempt) => attempt,
            };
            loop {
                check()?;
                match self.check_captured_creature_candidate(
                    batch,
                    light,
                    &mut plan,
                    true,
                    inputs.clocks,
                    inputs.border,
                    64,
                )? {
                    ResumedCreatureCheck::Pending => continue,
                    ResumedCreatureCheck::Rejected => break,
                    ResumedCreatureCheck::NeedsConstruction(_) => {
                        let construction = construct(&attempt.entity)?;
                        ensure!(
                            used_uuids.insert(construction.uuid),
                            "duplicate generated creature UUID across spawn chunks"
                        );
                        self.construct_captured_creature(
                            batch,
                            light,
                            &mut plan,
                            true,
                            inputs.clocks,
                            inputs.border,
                            construction,
                        )?;
                        check()?;
                        if let ResumedCreatureInstance::NeedsFinalization(_) = self
                            .check_captured_creature_instance(
                                batch,
                                light,
                                &mut plan,
                                true,
                                inputs.clocks,
                                inputs.border,
                            )?
                        {
                            self.finalize_captured_creature(
                                batch,
                                light,
                                &mut plan,
                                true,
                                inputs.clocks,
                                inputs.border,
                            )?;
                        }
                        break;
                    }
                }
            }
        }
        check()?;
        let border = plan.inner.border;
        let spawn = self.finish_captured_creatures(
            batch,
            light,
            &plan,
            true,
            inputs.clocks,
            border,
            inputs.last_update,
        )?;
        check()?;
        Ok(CapturedCompletedSpawn::Planned(spawn))
    }

    fn build_creature_plan(
        &mut self,
        position: native_world::loaded::ChunkPos,
        biomes: &native_session::worldgen::BiomeNeighborhood,
        revision: &Arc<()>,
        light_revision: &Arc<()>,
        clocks: &BTreeMap<ResourceId, i64>,
    ) -> Result<ResumedCreaturePlan> {
        let x = position.x.checked_mul(16).context("mob biome X overflow")?;
        let z = position.z.checked_mul(16).context("mob biome Z overflow")?;
        let y = self
            .lighting
            .min_y
            .checked_add(self.lighting.height)
            .and_then(|v| v.checked_sub(1))
            .context("mob biome Y overflow")?;
        let spawn_biome = biomes.biome_at_block(x, y, z)?.clone();
        let probability_biome = biomes.noise_biome_at_block(x, y, z)?.clone();
        if self.creature_environment.is_none() {
            let categories = self.resources.vanilla_entity_category_registry()?;
            let environment = native_resources::creature_timeline::CreatureSpawnEnvironment::from_static_definition_with_registry(&self.dimension_definition, &categories)?;
            self.creature_environment = Some((environment, categories));
        }
        let (environment, categories) = self
            .creature_environment
            .as_ref()
            .expect("creature environment initialized");
        let spawn = native_resources::mob_spawns::load_biome_spawn_attributes(
            &mut self.resources,
            &spawn_biome,
        )?;
        let probability = if probability_biome == spawn_biome {
            spawn.clone()
        } else {
            native_resources::mob_spawns::load_biome_spawn_attributes(
                &mut self.resources,
                &probability_biome,
            )?
        };
        let settings = native_resources::mob_spawns::resolve_creature_spawns(
            environment.dimension_attributes(),
            &spawn,
            &probability,
            &[],
            |id| categories.is_misc(id),
        )?;
        let planner = native_resources::worldgen_mob_spawner::ChunkCreatureSpawner::new(
            self.context.inputs().settings().seed(),
            position.x,
            position.z,
            true,
            settings.probability,
            settings.spawns,
        )?;
        Ok(ResumedCreaturePlan {
            identity: Arc::clone(&self.identity),
            revision: Arc::clone(revision),
            light_revision: Arc::clone(light_revision),
            position,
            clocks: clocks.clone(),
            planner,
            finished: false,
            probe: None,
            construction: None,
            border: None,
            rule_light: None,
            random: self.generator.region_random(position),
            group_id: None,
            sheep_group: Default::default(),
            horse_group: Default::default(),
            donkey_group: Default::default(),
            constructed: None,
            pose: None,
            instance_passed: false,
            finalizable: false,
            additions: Vec::new(),
            used_uuids: BTreeSet::new(),
        })
    }

    /// Publish the completed plan's staged entities to an intermediate draft.
    /// Plans with an unresolved candidate cannot complete the scheduler.
    pub fn finish_resumed_creatures(
        &mut self,
        world: &ResumedTerrainWorld,
        schedule: &mut GenerationSchedule,
        light: &ResumedStagedLight,
        plan: ResumedCreaturePlan,
        spawn_mobs: bool,
        clocks: &BTreeMap<ResourceId, i64>,
        border: Option<native_session::respawn::RespawnBorder>,
    ) -> Result<ResumedPlannedSpawn> {
        use native_session::worldgen::schedule::GenerationStage;
        ensure!(
            plan.matches_state(world, light, spawn_mobs, clocks),
            "creature plan is stale"
        );
        ensure!(plan.finished, "creature attempts are incomplete");
        ensure!(plan.border == border, "creature spawn border differs");
        self.validate_resumed_spawn(world, schedule, light, plan.position)?;
        let position = native_persistence::world::ChunkPos {
            x: plan.position.x,
            z: plan.position.z,
        };
        let mut lit = self.draft_resumed_lit_chunk(world, light, position)?;
        Self::append_creature_records(&mut lit.feature.document, position, &plan.additions)?;
        schedule.complete(plan.position, GenerationStage::Spawn)?;
        Ok(ResumedPlannedSpawn {
            lit,
            clocks: plan.clocks,
            border: plan.border,
        })
    }

    /// Complete a mixed-terrain creature plan into a source-bound draft.
    /// Its caller must still check entity identities outside this capture,
    /// sidecars, live state and all save dependencies before publication.
    #[allow(clippy::too_many_arguments)]
    pub fn finish_captured_creatures(
        &mut self,
        batch: &mut CapturedTerrainBatch,
        light: &mut CapturedStagedLight,
        plan: &CapturedCreaturePlan,
        spawn_mobs: bool,
        clocks: &BTreeMap<ResourceId, i64>,
        border: Option<native_session::respawn::RespawnBorder>,
        last_update: i64,
    ) -> Result<CapturedPlannedSpawn> {
        use native_session::worldgen::schedule::GenerationStage;
        ensure!(
            plan.matches_state(batch, light, spawn_mobs, clocks),
            "creature plan is stale"
        );
        ensure!(plan.inner.finished, "creature attempts are incomplete");
        ensure!(plan.inner.border == border, "creature spawn border differs");
        self.validate_captured_spawn(batch, light, plan.inner.position)?;
        let position = native_persistence::world::ChunkPos {
            x: plan.inner.position.x,
            z: plan.inner.position.z,
        };
        let mut lit = match batch
            .sources
            .get(&position)
            .context("terrain source was not captured")?
        {
            None => CapturedLitChunkDraft::New(self.draft_captured_new_lit_chunk(
                batch,
                light,
                position,
                last_update,
            )?),
            Some(_) => CapturedLitChunkDraft::Saved(
                self.draft_captured_saved_lit_chunk(batch, light, position)?,
            ),
        };
        let document = match &mut lit {
            CapturedLitChunkDraft::New(draft) => &mut draft.document,
            CapturedLitChunkDraft::Saved(draft) => &mut draft.document,
        };
        Self::append_creature_records(document, position, &plan.inner.additions)?;
        replace_unique_root_field(document, "Status", Tag::String("minecraft:spawn".into()))?;
        native_persistence::nbt::write(document)?;
        batch
            .schedule
            .complete(plan.inner.position, GenerationStage::Spawn)?;
        let completion = Arc::new(());
        light
            .completed_creature_spawns
            .insert(plan.inner.position, Arc::clone(&completion));
        Ok(CapturedPlannedSpawn {
            lit,
            position: plan.inner.position,
            clocks: plan.inner.clocks.clone(),
            border: plan.inner.border,
            completion,
        })
    }

    fn append_creature_records(
        document: &mut NamedTag,
        position: native_persistence::world::ChunkPos,
        additions: &[native_session::worldgen::GeneratedEntityAddition],
    ) -> Result<()> {
        let records = additions
            .iter()
            .map(|addition| {
                ensure!(
                    addition.block_position[0].div_euclid(16) == position.x
                        && addition.block_position[2].div_euclid(16) == position.z,
                    "generated creature is outside its source chunk"
                );
                match &addition.record {
                    native_session::worldgen::GeneratedEntityRecord::Saved(tag) => Ok(tag.clone()),
                    _ => bail!("unsupported generated creature record"),
                }
            })
            .collect::<Result<Vec<_>>>()?;
        let original = unique_root_field(document, "entities")?;
        if let Some(entities) =
            native_persistence::generated_chunk::append_proto_entity_records(original, &records)?
        {
            replace_unique_root_field(document, "entities", entities)?;
        }
        native_persistence::nbt::write(document)?;
        Ok(())
    }

    /// Check a pending supported creature against the captured terrain and
    /// light. Rejected candidates advance Java's planner; accepted candidates
    /// remain pending until entity construction and finalization are available.
    #[allow(clippy::too_many_arguments)]
    pub fn check_resumed_creature_candidate(
        &mut self,
        world: &ResumedTerrainWorld,
        schedule: &GenerationSchedule,
        light: &ResumedStagedLight,
        plan: &mut ResumedCreaturePlan,
        spawn_mobs: bool,
        clocks: &BTreeMap<ResourceId, i64>,
        border: native_session::respawn::RespawnBorder,
        budget: usize,
    ) -> Result<ResumedCreatureCheck> {
        ensure!(
            plan.matches_state(world, light, spawn_mobs, clocks),
            "creature plan is stale"
        );
        self.validate_resumed_spawn(world, schedule, light, plan.position)?;
        self.check_creature_candidate(&world.terrain, &light.engine, plan, border, budget)
    }

    /// Test a mixed-terrain candidate without creating or publishing an entity.
    #[allow(clippy::too_many_arguments)]
    pub fn check_captured_creature_candidate(
        &mut self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        plan: &mut CapturedCreaturePlan,
        spawn_mobs: bool,
        clocks: &BTreeMap<ResourceId, i64>,
        border: native_session::respawn::RespawnBorder,
        budget: usize,
    ) -> Result<ResumedCreatureCheck> {
        ensure!(
            plan.matches_state(batch, light, spawn_mobs, clocks),
            "creature plan is stale"
        );
        self.validate_captured_spawn(batch, light, plan.inner.position)?;
        self.check_creature_candidate(
            &batch.terrain,
            &light.engine,
            &mut plan.inner,
            border,
            budget,
        )
    }

    fn check_creature_candidate(
        &mut self,
        terrain: &native_session::worldgen::CarvedTerrainWorld,
        engine: &native_session::worldgen::StagedGenerationLightEngine,
        plan: &mut ResumedCreaturePlan,
        border: native_session::respawn::RespawnBorder,
        budget: usize,
    ) -> Result<ResumedCreatureCheck> {
        use native_resources::worldgen_mob_spawner::CreatureSpawnProgress;
        use native_session::worldgen::{
            GeneratedCreatureCollisionProgress, GeneratedCreatureProbeProgress,
            GeneratedCreatureRuleProgress,
        };
        ensure!(border.is_valid(), "invalid mob spawn border");
        ensure!(
            plan.border.is_none_or(|before| before == border),
            "creature spawn border changed"
        );
        plan.border = Some(border);
        if let Some(geometry) = plan.construction {
            return Ok(ResumedCreatureCheck::NeedsConstruction(geometry));
        }
        let CreatureSpawnProgress::Attempt(attempt) = plan.planner.next(0)? else {
            bail!("no creature candidate is pending")
        };
        ensure!(
            supports_generated_creature(&attempt.entity),
            "unsupported generated creature {}:{}",
            attempt.entity.namespace,
            attempt.entity.path
        );
        if self.mob_placement_assets.is_none() {
            let blocks = self.resources.vanilla_block_ids()?;
            let prevent_inside = native_resources::mob_spawns::load_prevent_mob_spawning_inside(
                &mut self.resources,
                &blocks,
            )?;
            let height_tags = native_resources::heightmap_tags::HeightmapTags::load(
                &mut self.resources,
                &blocks,
            )?;
            let ground_tags = native_resources::worldgen_mob_rules::GroundSpawnTags::load(
                &mut self.resources,
                &blocks,
            )?;
            let sheep_tags = native_resources::worldgen_region_random::SheepBiomeTags::load(
                &mut self.resources,
            )?;
            self.mob_placement_assets = Some(SavedMobPlacementAssets {
                prevent_inside,
                height_tags,
                ground_tags,
                sheep_tags,
            });
        }
        let assets = self
            .mob_placement_assets
            .as_ref()
            .expect("mob placement assets initialized");
        let registry = &self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?
            .blocks;
        if plan.rule_light.is_none() {
            plan.rule_light = Some(engine.snapshot(terrain, [plan.position], registry)?);
        }
        let visible = plan
            .rule_light
            .as_ref()
            .expect("candidate light initialized");
        let ambient = self
            .lighting
            .ambient_light
            .context("dimension ambient light is missing")?;
        let has_ceiling = self
            .dimension_definition
            .get("has_ceiling")
            .and_then(Value::as_bool)
            .context("dimension has_ceiling is missing")?;
        let mut view = native_session::worldgen::GeneratedMobPlacementView::from_captured_terrain(
            terrain,
            visible,
            registry,
            ambient,
            self.lighting.min_y,
            self.lighting.height,
            self.generator.sea_level(),
            native_resources::mob_spawn_placement::MobSpawnHeightmap::MotionBlockingNoLeaves,
            border,
            &assets.prevent_inside,
            &assets.height_tags,
        )?;
        let probe = plan.probe.get_or_insert_with(|| {
            native_session::worldgen::GeneratedCreatureProbe::new(
                attempt,
                self.lighting.min_y,
                has_ceiling,
                true,
            )
        });
        match probe.poll(&mut plan.planner, &mut view, budget)? {
            GeneratedCreatureProbeProgress::Pending => Ok(ResumedCreatureCheck::Pending),
            GeneratedCreatureProbeProgress::Rejected => {
                plan.probe = None;
                Ok(ResumedCreatureCheck::Rejected)
            }
            GeneratedCreatureProbeProgress::NeedsCollision(_) => {
                match probe.check_collision(&mut plan.planner, &view)? {
                    GeneratedCreatureCollisionProgress::Rejected => {
                        plan.probe = None;
                        Ok(ResumedCreatureCheck::Rejected)
                    }
                    GeneratedCreatureCollisionProgress::NeedsSpawnRules(_) => {
                        match probe.check_spawn_rules(
                            &mut plan.planner,
                            &mut view,
                            &assets.ground_tags,
                        )? {
                            GeneratedCreatureRuleProgress::Rejected => {
                                plan.probe = None;
                                Ok(ResumedCreatureCheck::Rejected)
                            }
                            GeneratedCreatureRuleProgress::NeedsConstruction(geometry) => {
                                plan.construction = Some(geometry);
                                Ok(ResumedCreatureCheck::NeedsConstruction(geometry))
                            }
                        }
                    }
                }
            }
        }
    }

    /// Mark the externally constructed entity at Java's constructor point.
    /// The caller supplies a UUID from the entity's independent RNG and must
    /// still verify identities outside this capture and in other spawn plans.
    #[allow(clippy::too_many_arguments)]
    pub fn construct_resumed_creature(
        &mut self,
        world: &ResumedTerrainWorld,
        schedule: &GenerationSchedule,
        light: &ResumedStagedLight,
        plan: &mut ResumedCreaturePlan,
        spawn_mobs: bool,
        clocks: &BTreeMap<ResourceId, i64>,
        border: native_session::respawn::RespawnBorder,
        construction: native_session::worldgen::FreshCreatureConstruction,
    ) -> Result<native_session::worldgen::GeneratedCreaturePose> {
        ensure!(
            plan.matches_state(world, light, spawn_mobs, clocks),
            "creature plan is stale"
        );
        self.validate_resumed_spawn(world, schedule, light, plan.position)?;
        Self::validate_creature_construction(plan, border, construction)?;
        self.check_known_entity_id(world, construction.uuid)?;
        Self::mark_creature_constructed(plan, construction)
    }

    /// Construct a checked mixed-terrain candidate without publishing it. The
    /// caller must also verify identities outside this capture and other plans.
    #[allow(clippy::too_many_arguments)]
    pub fn construct_captured_creature(
        &mut self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        plan: &mut CapturedCreaturePlan,
        spawn_mobs: bool,
        clocks: &BTreeMap<ResourceId, i64>,
        border: native_session::respawn::RespawnBorder,
        construction: native_session::worldgen::FreshCreatureConstruction,
    ) -> Result<native_session::worldgen::GeneratedCreaturePose> {
        ensure!(
            plan.matches_state(batch, light, spawn_mobs, clocks),
            "creature plan is stale"
        );
        self.validate_captured_spawn(batch, light, plan.inner.position)?;
        Self::validate_creature_construction(&mut plan.inner, border, construction)?;
        self.check_terrain_entity_id(
            &batch.terrain,
            &batch.revision,
            batch
                .sources
                .keys()
                .map(|pos| native_world::loaded::ChunkPos { x: pos.x, z: pos.z }),
            construction.uuid,
        )?;
        Self::mark_creature_constructed(&mut plan.inner, construction)
    }

    fn validate_creature_construction(
        plan: &mut ResumedCreaturePlan,
        border: native_session::respawn::RespawnBorder,
        construction: native_session::worldgen::FreshCreatureConstruction,
    ) -> Result<()> {
        use native_resources::worldgen_mob_spawner::CreatureSpawnProgress;
        ensure!(plan.border == Some(border), "creature spawn border differs");
        ensure!(
            plan.construction.is_some() && plan.constructed.is_none(),
            "creature is not ready for construction"
        );
        let CreatureSpawnProgress::Attempt(attempt) = plan.planner.next(0)? else {
            bail!("creature candidate is missing")
        };
        ensure!(
            supports_generated_creature(&attempt.entity),
            "creature finalization is not supported"
        );
        if attempt.entity.path == "chicken" {
            ensure!(
                construction
                    .chicken_egg_time
                    .is_some_and(|time| (6000..12000).contains(&time)),
                "invalid constructed chicken egg timer"
            );
        } else {
            ensure!(
                construction.chicken_egg_time.is_none(),
                "egg timer belongs only to a chicken"
            );
        }
        ensure!(
            !plan.used_uuids.contains(&construction.uuid),
            "duplicate generated creature UUID"
        );
        Ok(())
    }

    fn mark_creature_constructed(
        plan: &mut ResumedCreaturePlan,
        construction: native_session::worldgen::FreshCreatureConstruction,
    ) -> Result<native_session::worldgen::GeneratedCreaturePose> {
        let pose = plan
            .probe
            .as_mut()
            .context("creature probe missing")?
            .mark_constructed(&mut plan.planner)?;
        plan.used_uuids.insert(construction.uuid);
        plan.constructed = Some(construction);
        plan.pose = Some(pose);
        Ok(pose)
    }

    fn check_known_entity_id(&mut self, world: &ResumedTerrainWorld, uuid: [i32; 4]) -> Result<()> {
        self.check_terrain_entity_id(
            &world.terrain,
            &world.revision,
            world
                .sources
                .keys()
                .map(|pos| native_world::loaded::ChunkPos { x: pos.x, z: pos.z }),
            uuid,
        )
    }

    fn check_terrain_entity_id(
        &mut self,
        terrain: &native_session::worldgen::CarvedTerrainWorld,
        revision: &Arc<()>,
        positions: impl Iterator<Item = native_world::loaded::ChunkPos>,
        uuid: [i32; 4],
    ) -> Result<()> {
        let captured = self.captured_entity_ids.get_or_insert_with(|| {
            let mut ids = KnownEntityIds::default();
            for chunk in self.context.inputs().sources().chunks().values() {
                if let Some(terrain) = &chunk.terrain {
                    ids.observe_document(&terrain.document, "entities");
                }
                if let Some(entities) = &chunk.entities {
                    ids.observe_document(&entities.document, "Entities");
                }
            }
            ids
        });
        ensure!(
            !captured.0.contains(&uuid),
            "generated creature UUID conflicts with a captured entity"
        );
        if !self
            .generated_entity_ids
            .as_ref()
            .is_some_and(|(cached, _)| Arc::ptr_eq(cached, revision))
        {
            let mut ids = KnownEntityIds::default();
            for pos in positions {
                ids.observe_records(terrain.recorded_generated_entities(pos)?);
            }
            self.generated_entity_ids = Some((Arc::clone(revision), ids));
        }
        ensure!(
            !self
                .generated_entity_ids
                .as_ref()
                .context("generated entity identity index missing")?
                .1
                .0
                .contains(&uuid),
            "generated creature UUID conflicts with a generated feature entity"
        );
        Ok(())
    }

    /// Check the constructed entity's own spawn rule and obstruction in order.
    #[allow(clippy::too_many_arguments)]
    pub fn check_resumed_creature_instance(
        &self,
        world: &ResumedTerrainWorld,
        schedule: &GenerationSchedule,
        light: &ResumedStagedLight,
        plan: &mut ResumedCreaturePlan,
        spawn_mobs: bool,
        clocks: &BTreeMap<ResourceId, i64>,
        border: native_session::respawn::RespawnBorder,
    ) -> Result<ResumedCreatureInstance> {
        ensure!(
            plan.matches_state(world, light, spawn_mobs, clocks),
            "creature plan is stale"
        );
        self.validate_resumed_spawn(world, schedule, light, plan.position)?;
        self.check_creature_instance(&world.terrain, plan, border)
    }

    /// Check the constructed mixed-terrain entity's own rule and obstruction.
    #[allow(clippy::too_many_arguments)]
    pub fn check_captured_creature_instance(
        &self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        plan: &mut CapturedCreaturePlan,
        spawn_mobs: bool,
        clocks: &BTreeMap<ResourceId, i64>,
        border: native_session::respawn::RespawnBorder,
    ) -> Result<ResumedCreatureInstance> {
        ensure!(
            plan.matches_state(batch, light, spawn_mobs, clocks),
            "creature plan is stale"
        );
        self.validate_captured_spawn(batch, light, plan.inner.position)?;
        self.check_creature_instance(&batch.terrain, &mut plan.inner, border)
    }

    fn check_creature_instance(
        &self,
        terrain: &native_session::worldgen::CarvedTerrainWorld,
        plan: &mut ResumedCreaturePlan,
        border: native_session::respawn::RespawnBorder,
    ) -> Result<ResumedCreatureInstance> {
        use native_session::worldgen::{
            GeneratedCreatureInstanceRuleProgress, GeneratedCreatureObstructionProgress,
        };
        ensure!(plan.border == Some(border), "creature spawn border differs");
        let pose = plan.pose.context("constructed creature is missing")?;
        if plan.finalizable {
            return Ok(ResumedCreatureInstance::NeedsFinalization(pose));
        }
        let assets = self
            .mob_placement_assets
            .as_ref()
            .context("mob placement assets are missing")?;
        let registry = &self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?
            .blocks;
        let visible = plan
            .rule_light
            .as_ref()
            .context("candidate light missing")?;
        let ambient = self
            .lighting
            .ambient_light
            .context("dimension ambient light is missing")?;
        let mut view = native_session::worldgen::GeneratedMobPlacementView::from_captured_terrain(
            terrain,
            visible,
            registry,
            ambient,
            self.lighting.min_y,
            self.lighting.height,
            self.generator.sea_level(),
            native_resources::mob_spawn_placement::MobSpawnHeightmap::MotionBlockingNoLeaves,
            border,
            &assets.prevent_inside,
            &assets.height_tags,
        )?;
        let probe = plan.probe.as_mut().context("creature probe missing")?;
        if !plan.instance_passed {
            match probe.check_instance_rules(&mut plan.planner, &mut view)? {
                GeneratedCreatureInstanceRuleProgress::Rejected => {
                    plan.clear_candidate();
                    return Ok(ResumedCreatureInstance::Rejected);
                }
                GeneratedCreatureInstanceRuleProgress::NeedsObstruction(_) => {
                    plan.instance_passed = true
                }
            }
        }
        let bounds = native_world::movement::BoundingBox {
            min: glam::DVec3::from_array(pose.geometry.min),
            max: glam::DVec3::from_array(pose.geometry.max),
        };
        match probe.check_obstruction(&mut plan.planner, &mut view, bounds)? {
            GeneratedCreatureObstructionProgress::Rejected => {
                plan.clear_candidate();
                Ok(ResumedCreatureInstance::Rejected)
            }
            GeneratedCreatureObstructionProgress::NeedsFinalization(_) => {
                plan.finalizable = true;
                Ok(ResumedCreatureInstance::NeedsFinalization(pose))
            }
        }
    }

    /// Resolve structure IDs by actual piece bounds, including updated treasure
    /// pieces; unsupported or missing saved references remain errors.
    fn resumed_structures_at(
        &mut self,
        world: &ResumedTerrainWorld,
        position: [i32; 3],
    ) -> Result<BTreeSet<ResourceId>> {
        let target = native_persistence::world::ChunkPos {
            x: position[0].div_euclid(16),
            z: position[2].div_euclid(16),
        };
        let references = self.restore_saved_structure_references(target)?;
        validate_saved_decoration_references(&references)?;
        self.creature_structures_at(&references.references, &world.treasure_starts, position)
    }

    fn captured_structures_at(
        &mut self,
        batch: &CapturedTerrainBatch,
        position: [i32; 3],
    ) -> Result<BTreeSet<ResourceId>> {
        let target = native_world::loaded::ChunkPos {
            x: position[0].div_euclid(16),
            z: position[2].div_euclid(16),
        };
        if let Some(generated) = batch.generated_references.get(&target) {
            self.creature_structures_at(&generated.references, &batch.treasure_starts, position)
        } else {
            let saved = native_persistence::world::ChunkPos {
                x: target.x,
                z: target.z,
            };
            ensure!(
                matches!(batch.sources.get(&saved), Some(Some(_))),
                "structure references missing"
            );
            let references = self.restore_saved_structure_references(saved)?;
            validate_saved_decoration_references(&references)?;
            self.creature_structures_at(&references.references, &batch.treasure_starts, position)
        }
    }

    fn creature_structures_at(
        &mut self,
        references: &native_session::worldgen::structure_references::OrderedStructureReferences,
        treasure_starts: &BTreeMap<SavedStructureKey, ResumedTreasureStart>,
        position: [i32; 3],
    ) -> Result<BTreeSet<ResourceId>> {
        use native_session::worldgen::saved_structure_starts::RestoredStructureStart;
        let mut loaded = BTreeMap::new();
        let mut matches = BTreeSet::new();
        let contains = |bounds: native_core::beardifier::BoundingBox| {
            (0..3).all(|axis| (bounds.min[axis]..=bounds.max[axis]).contains(&position[axis]))
        };
        for (id, sources) in references.membership() {
            for &packed in sources {
                let bits = packed as u64;
                let source = native_world::loaded::ChunkPos {
                    x: bits as u32 as i32,
                    z: (bits >> 32) as u32 as i32,
                };
                let saved_pos = native_persistence::world::ChunkPos {
                    x: source.x,
                    z: source.z,
                };
                let captured = self
                    .context
                    .inputs()
                    .sources()
                    .chunks()
                    .get(&saved_pos)
                    .and_then(|chunk| chunk.terrain.as_ref())
                    .context("structure source missing")?;
                let key = (id.clone(), source);
                if let Some(treasure) = treasure_starts.get(&key) {
                    ensure!(
                        Arc::ptr_eq(&treasure.source, &captured.document),
                        "structure source differs"
                    );
                    if contains(treasure.placement.bounds) {
                        matches.insert(id.clone());
                        break;
                    }
                    continue;
                }
                if !loaded.contains_key(&source) {
                    loaded.insert(source, self.restore_saved_structure_starts(saved_pos)?);
                }
                let start = loaded[&source]
                    .get(id)
                    .context("referenced structure start missing")?;
                if let RestoredStructureStart::Present { chunk, pieces, .. } = start {
                    ensure!(*chunk == source, "structure start source differs");
                    if pieces.iter().any(|piece| contains(piece.bounds())) {
                        matches.insert(id.clone());
                        break;
                    }
                }
            }
        }
        Ok(matches)
    }

    /// Finalize a supported animal into the plan, leaving disk and live state untouched.
    #[allow(clippy::too_many_arguments)]
    pub fn finalize_resumed_creature(
        &mut self,
        world: &ResumedTerrainWorld,
        schedule: &GenerationSchedule,
        light: &ResumedStagedLight,
        context: &SavedDecorationContext,
        plan: &mut ResumedCreaturePlan,
        spawn_mobs: bool,
        clocks: &BTreeMap<ResourceId, i64>,
        border: native_session::respawn::RespawnBorder,
    ) -> Result<()> {
        ensure!(
            plan.matches_state(world, light, spawn_mobs, clocks)
                && Arc::ptr_eq(&self.identity, &context.identity),
            "creature plan is stale"
        );
        self.validate_resumed_spawn(world, schedule, light, plan.position)?;
        ensure!(
            context.position.x == plan.position.x && context.position.z == plan.position.z,
            "creature biome context differs"
        );
        self.finalize_creature(
            plan,
            border,
            &context.biomes,
            CreatureStructureContext::Resumed(world),
        )
    }

    /// Finalize a mixed-terrain creature into its private spawn plan.
    #[allow(clippy::too_many_arguments)]
    pub fn finalize_captured_creature(
        &mut self,
        batch: &CapturedTerrainBatch,
        light: &CapturedStagedLight,
        plan: &mut CapturedCreaturePlan,
        spawn_mobs: bool,
        clocks: &BTreeMap<ResourceId, i64>,
        border: native_session::respawn::RespawnBorder,
    ) -> Result<()> {
        ensure!(
            plan.matches_state(batch, light, spawn_mobs, clocks),
            "creature plan is stale"
        );
        self.validate_captured_spawn(batch, light, plan.inner.position)?;
        self.finalize_creature(
            &mut plan.inner,
            border,
            &batch.biomes,
            CreatureStructureContext::Captured(batch),
        )
    }

    fn finalize_creature(
        &mut self,
        plan: &mut ResumedCreaturePlan,
        border: native_session::respawn::RespawnBorder,
        biomes: &native_session::worldgen::BiomeNeighborhood,
        structures: CreatureStructureContext<'_>,
    ) -> Result<()> {
        ensure!(
            plan.border == Some(border) && plan.finalizable,
            "creature is not ready for finalization"
        );
        let construction = plan.constructed.context("constructed creature missing")?;
        let pose = plan.pose.context("creature pose missing")?;
        let coords = pose.geometry.position;
        ensure!(
            coords.iter().all(|v| v.is_finite()
                && v.floor() >= f64::from(i32::MIN)
                && v.floor() <= f64::from(i32::MAX)),
            "invalid creature coordinates"
        );
        let position = [
            coords[0].floor() as i32,
            coords[1].floor() as i32,
            coords[2].floor() as i32,
        ];
        ensure!(
            position[0].div_euclid(16) == plan.position.x
                && position[2].div_euclid(16) == plan.position.z,
            "creature moved outside its source chunk"
        );
        let attempt = plan
            .planner
            .created_attempt()
            .context("constructed creature attempt missing")?;
        use native_resources::animal_variants::FarmAnimal;
        let farm = match attempt.entity.path.as_str() {
            "pig" => Some((0, FarmAnimal::Pig)),
            "cow" => Some((1, FarmAnimal::Cow)),
            "chicken" => Some((2, FarmAnimal::Chicken)),
            _ => None,
        };
        if let Some((index, species)) = farm {
            if self.farm_variants[index].is_none() {
                self.farm_variants[index] = Some(
                    native_resources::animal_variants::FarmAnimalSpawnVariants::load_active(
                        &mut self.resources,
                        species,
                    )?,
                );
            }
            let variants = self.farm_variants[index]
                .as_ref()
                .expect("farm variants initialized");
            let needs_moon = variants.uses_moon_conditions();
            if needs_moon && self.farm_moon.is_none() {
                self.farm_moon = Some(
                    native_resources::moon_phase::Environment::from_static_definition(
                        &mut self.resources,
                        &self.dimension_definition,
                    )?,
                );
            }
        }
        let probe = plan.probe.as_mut().context("creature probe missing")?;
        let addition = match attempt.entity.path.as_str() {
            "sheep" => {
                let biome = biomes.biome_at_block(position[0], position[1], position[2])?;
                let tags = &self
                    .mob_placement_assets
                    .as_ref()
                    .context("mob placement assets missing")?
                    .sheep_tags;
                probe.prepare_sheep_with_biome(
                    &mut plan.planner,
                    biome,
                    tags,
                    &mut plan.random,
                    &mut plan.sheep_group,
                    construction.uuid,
                )?
            }
            "horse" => probe.prepare_fresh_horse(
                &mut plan.planner,
                &mut plan.random,
                &mut plan.horse_group,
                construction.uuid,
            )?,
            "donkey" => probe.prepare_fresh_donkey(
                &mut plan.planner,
                &mut plan.random,
                &mut plan.donkey_group,
                construction.uuid,
            )?,
            "turtle" => probe.prepare_fresh_turtle(
                &mut plan.planner,
                &mut plan.random,
                &mut plan.sheep_group,
                construction.uuid,
            )?,
            "pig" | "cow" | "chicken" => {
                let index = farm.expect("farm species identified").0;
                let biome = biomes.biome_at_block(position[0], position[1], position[2])?;
                let variants = self.farm_variants[index]
                    .take()
                    .expect("farm variants initialized");
                let moon = self.farm_moon.take();
                let result = {
                    let mut resolve_structures = || match &structures {
                        CreatureStructureContext::Resumed(world) => {
                            self.resumed_structures_at(world, position)
                        }
                        CreatureStructureContext::Captured(batch) => {
                            self.captured_structures_at(batch, position)
                        }
                    };
                    let mut environment = ResumedFarmEnvironment {
                        biome,
                        moon: moon.as_ref(),
                        structures: None,
                        resolve_structures: &mut resolve_structures,
                    };
                    probe.prepare_farm_animal_in_environment(
                        &mut plan.planner,
                        &variants,
                        &mut environment,
                        &mut plan.random,
                        &mut plan.sheep_group,
                        construction.uuid,
                        construction.chicken_egg_time,
                    )
                };
                self.farm_variants[index] = Some(variants);
                self.farm_moon = moon;
                result?
            }
            _ => bail!("creature finalization is not supported"),
        };
        plan.additions.push(addition);
        plan.clear_candidate();
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
        self.draft_saved_treasure_starts(&world.treasure_starts, position)
    }

    /// Retain the changed structure source independently of terrain output.
    pub fn draft_captured_structure_starts(
        &self,
        batch: &CapturedTerrainBatch,
        position: native_persistence::world::ChunkPos,
    ) -> Result<ResumedStructureDraft> {
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity),
            "structure belongs to another generation capture"
        );
        self.draft_saved_treasure_starts(&batch.treasure_starts, position)
    }

    fn draft_captured_structure_sources(
        &self,
        batch: &CapturedTerrainBatch,
    ) -> Result<BTreeMap<native_world::loaded::ChunkPos, ResumedStructureDraft>> {
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity),
            "structure belongs to another generation capture"
        );
        let mut drafts = BTreeMap::new();
        for (_, position) in batch.treasure_starts.keys() {
            if !drafts.contains_key(position) {
                let saved_pos = native_persistence::world::ChunkPos {
                    x: position.x,
                    z: position.z,
                };
                drafts.insert(
                    *position,
                    self.draft_saved_treasure_starts(&batch.treasure_starts, saved_pos)?,
                );
            }
        }
        Ok(drafts)
    }

    fn draft_saved_treasure_starts(
        &self,
        treasure_starts: &BTreeMap<SavedStructureKey, ResumedTreasureStart>,
        position: native_persistence::world::ChunkPos,
    ) -> Result<ResumedStructureDraft> {
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
        let updates: Vec<_> = treasure_starts
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
        validate_empty_preterrain_target(saved, !self.generate_structures)?;
        let source = Arc::clone(&saved.document);
        ensure!(
            world
                .sources
                .get(&position)
                .is_some_and(|value| Arc::ptr_eq(value, &source)),
            "resumed terrain source differs"
        );
        reject_saved_lighting(saved)?;
        let source_nbt = native_persistence::nbt::write(&source)?;
        let document = if world.treasure_starts.keys().any(|(_, pos)| *pos == key) {
            self.draft_resumed_structure_starts(world, position)?
                .document
        } else {
            (*source).clone()
        };
        let document =
            self.populate_saved_feature_document(&world.terrain, key, &source, document)?;
        Ok(ResumedFeatureDraft {
            identity: Arc::clone(&self.identity),
            revision: Arc::clone(&world.revision),
            source_nbt,
            entity_sidecar,
            feature_entity_coverage: world.feature_entity_sources.covers(key),
            document,
        })
    }

    /// Merge generated data into the captured pre-terrain document.
    fn populate_saved_feature_document(
        &mut self,
        terrain: &native_session::worldgen::CarvedTerrainWorld,
        key: native_world::loaded::ChunkPos,
        source: &NamedTag,
        mut document: NamedTag,
    ) -> Result<NamedTag> {
        let original_maps = preserved_heightmap_fields(source)?;
        validate_empty_saved_postprocessing(source, self.lighting.min_y, self.lighting.height)?;
        let biome_ids: BTreeSet<_> = self
            .resources
            .list_data("worldgen/biome")?
            .into_iter()
            .collect();
        let assets = self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?;
        let snapshot = terrain.snapshot();
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
        let status = match unique_root_field(source, "Status")? {
            Some(Tag::String(status)) => String::from_utf16(&status.0)?,
            _ => bail!("saved chunk status is missing or invalid"),
        };
        let stage = native_session::worldgen::schedule::GenerationStage::from_id(&status)
            .context("unknown saved chunk status")?;
        let original_sections = unique_root_field(source, "sections")?;
        let sections = if stage < native_session::worldgen::schedule::GenerationStage::Biomes {
            native_persistence::generated_section::merge_prebiome_stage_sections(
                original_sections,
                &sections,
            )?
        } else {
            native_persistence::generated_section::merge_biome_stage_blocks(
                original_sections.context("saved sections missing")?,
                &sections,
            )?
        };
        let block_entities = terrain.generated_block_entity_data(key, &assets.blocks)?;
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
            unique_root_field(source, "entities")?,
            terrain.recorded_generated_entities(key)?,
        )?;
        let Tag::Compound(generated_maps) = terrain.generated_heightmap_data(key)? else {
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
        let marks = terrain
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
        Ok(document)
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
        self.with_saved_prepared_decoration_inner(context, false, |prepared, _| action(prepared))
    }

    fn with_saved_prepared_decoration_inner<T>(
        &mut self,
        context: &SavedDecorationContext,
        needs_mineshaft: bool,
        action: impl FnOnce(
            &native_session::worldgen::PreparedDecoration<'_, '_>,
            &native_resources::block_registry::GeneratedBlockRegistry,
        ) -> Result<T>,
    ) -> Result<T> {
        let plan = context.plan(self)?;
        self.ensure_terrain_assets()?;
        let assets = self
            .terrain_assets
            .as_ref()
            .expect("terrain assets initialized");
        let mut resolve = |value: &Value| resolve_generated_terrain_state(value, &assets.blocks);
        let prepared = native_session::worldgen::PreparedDecoration::load_with_mineshaft(
            &mut self.resources,
            &plan,
            &assets.block_ids,
            &mut resolve,
            needs_mineshaft,
        )?;
        action(&prepared, &assets.blocks)
    }

    /// Resolve referenced starts in registry and saved fastutil order. The
    /// original source remains attached until a coordinated save exists.
    fn prepare_saved_structure_plan(
        &mut self,
        world: &ResumedTerrainWorld,
        target: native_persistence::world::ChunkPos,
    ) -> Result<SavedStructurePlan> {
        let references = self.restore_saved_structure_references(target)?;
        validate_saved_decoration_references(&references)?;
        self.prepare_captured_structure_plan(
            &world.treasure_starts,
            None,
            &references.references,
            target,
        )
    }

    fn prepare_captured_structure_plan(
        &mut self,
        previous: &BTreeMap<SavedStructureKey, ResumedTreasureStart>,
        previous_mineshafts: Option<&BTreeMap<SavedStructureKey, ResumedMineshaftStart>>,
        references: &native_session::worldgen::structure_references::OrderedStructureReferences,
        target: native_persistence::world::ChunkPos,
    ) -> Result<SavedStructurePlan> {
        use native_resources::structure_start::{SimpleStructureKind, SimpleStructureStart};
        use native_session::worldgen::saved_structure_starts::{
            RestoredStructurePiece, RestoredStructureStart,
        };
        if references.membership().values().all(BTreeSet::is_empty) {
            return Ok(SavedStructurePlan {
                schedule: None,
                starts: BTreeMap::new(),
                mineshafts: BTreeMap::new(),
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
                .membership()
                .keys()
                .all(|id| registered.contains(id)),
            "structure reference is absent from decoration registry"
        );
        let mut loaded = BTreeMap::new();
        let mut starts = BTreeMap::new();
        let mut mineshafts = BTreeMap::new();
        let mut order = Vec::new();
        for entry in schedule.steps().iter().flatten() {
            let Some(sources) = references.order(&entry.id) else {
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
                if let Some(value) = previous_mineshafts.and_then(|saved| saved.get(&key)) {
                    ensure!(
                        Arc::ptr_eq(&value.source, &captured),
                        "saved mineshaft source differs"
                    );
                    ensure!(
                        mineshafts.insert(key.clone(), value.clone()).is_none(),
                        "duplicate saved mineshaft reference"
                    );
                    keys.push(key);
                    continue;
                }
                let value = if let Some(value) = previous.get(&key) {
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
                    let is_mineshaft = pieces
                        .iter()
                        .all(|piece| matches!(piece, RestoredStructurePiece::Mineshaft(_)));
                    if is_mineshaft {
                        let may_touch = mineshaft_may_touch_chunk(
                            native_world::loaded::ChunkPos {
                                x: target.x,
                                z: target.z,
                            },
                            pieces,
                        )?;
                        if !may_touch {
                            continue;
                        }
                        if previous_mineshafts.is_some() {
                            let value = resume_saved_mineshaft(
                                captured,
                                original,
                                *references,
                                source,
                                *adjustment,
                                pieces,
                            )?;
                            ensure!(
                                mineshafts.insert(key.clone(), value).is_none(),
                                "duplicate saved mineshaft reference"
                            );
                            keys.push(key);
                            continue;
                        }
                    }
                    let [RestoredStructurePiece::BuriedTreasure(bounds)] = pieces.as_slice() else {
                        let kind = match &pieces[0] {
                            RestoredStructurePiece::BuriedTreasure(_) => "buried_treasure",
                            RestoredStructurePiece::SwampHut(_) => "swamp_hut",
                            RestoredStructurePiece::Mineshaft(_) => "mineshaft",
                            RestoredStructurePiece::Jigsaw(_) => "jigsaw",
                            RestoredStructurePiece::RuinedPortal(_) => "ruined_portal",
                        };
                        bail!(
                            "saved structure decoration type is not supported: {}:{} at {},{} ({kind}, {} pieces)",
                            entry.id.namespace,
                            entry.id.path,
                            source.x,
                            source.z,
                            pieces.len()
                        );
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
            mineshafts,
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
        // Priming heightmaps is part of the feature attempt. If preparation or
        // execution fails, a retry must see the original saved state, including
        // whether each individual heightmap was absent.
        let previous_terrain = positions
            .iter()
            .any(|&position| world.terrain.worldgen_heightmaps(position).is_none())
            .then(|| world.terrain.clone());
        let priming = (|| -> Result<()> {
            let tags = self
                .heightmap_tags
                .as_ref()
                .expect("heightmap tags initialized");
            for &position in &positions {
                if world.terrain.worldgen_heightmaps(position).is_none() {
                    world.terrain.ensure_worldgen_heightmaps(position, tags)?;
                }
            }
            Ok(())
        })();
        let attempt = priming.and_then(|()| {
            self.with_saved_prepared_decoration(context, |prepared| {
                if let Some(structures) = &structures {
                    let planned = structures.planned()?;
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
            })
        });
        let (result, updates) = match attempt {
            Ok(value) => value,
            Err(error) => {
                if let Some(previous) = previous_terrain {
                    world.terrain = previous;
                }
                return Err(error);
            }
        };
        if let Some(structures) = structures.as_mut() {
            structures.apply_updates(updates);
            world
                .feature_touched
                .extend(structures.starts.keys().map(|key| key.1));
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
        self.prepare_decoration_with_biomes(target, biomes)
    }

    fn prepare_decoration_with_biomes(
        &mut self,
        target: native_persistence::world::ChunkPos,
        biomes: native_session::worldgen::BiomeNeighborhood,
    ) -> Result<SavedDecorationContext> {
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

    /// Build a feature plan from the frozen mixed biome neighborhood.
    pub fn prepare_captured_decoration(
        &mut self,
        batch: &CapturedTerrainBatch,
        target: native_persistence::world::ChunkPos,
    ) -> Result<SavedDecorationContext> {
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity),
            "decoration belongs to another generation capture"
        );
        let mut biomes = self.generator.biome_neighborhood()?;
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                let x = target
                    .x
                    .checked_add(dx)
                    .context("neighbor chunk X overflow")?;
                let z = target
                    .z
                    .checked_add(dz)
                    .context("neighbor chunk Z overflow")?;
                let biome = batch
                    .biomes
                    .chunk([x, z])
                    .context("decoration biome neighbor is missing")?;
                biomes.insert(Arc::clone(biome))?;
            }
        }
        self.prepare_decoration_with_biomes(target, biomes)
    }

    /// Bind a caller-verified, fixed initial light revision. The caller must
    /// establish its provenance and completeness, including that no light
    /// sections were registered or propagated before the feature pass.
    pub fn bind_captured_initial_feature_light(
        &self,
        batch: &CapturedTerrainBatch,
        light: native_world::generation_light::VisibleLightSnapshot,
    ) -> Result<CapturedFeatureLight> {
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity),
            "terrain belongs to another generation capture"
        );
        ensure!(
            light.has_block_channel() && light.has_sky_channel() == self.lighting.has_skylight,
            "light channels differ from dimension"
        );
        Ok(CapturedFeatureLight {
            identity: Arc::clone(&batch.identity),
            revision: Arc::clone(&batch.revision),
            light,
        })
    }

    /// Bind a complete, caller-observed engine history to this exact terrain.
    /// The caller must prove the queue, registrations, active chunks and sky
    /// sources were observed continuously; saved arrays alone are insufficient.
    pub fn bind_captured_queued_light(
        &self,
        batch: &CapturedTerrainBatch,
        tracker: native_world::generation_light::QueuedSectionLightTracker,
        registered: impl IntoIterator<Item = native_world::loaded::ChunkPos>,
        sky_sources: BTreeMap<native_world::loaded::ChunkPos, Arc<[i32; 256]>>,
    ) -> Result<CapturedQueuedLight> {
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity),
            "terrain belongs to another generation capture"
        );
        let registry = &self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?
            .blocks;
        let engine = native_session::worldgen::QueuedGenerationLightEngine::new_with_registry(
            &batch.terrain,
            tracker,
            registered,
            sky_sources,
            self.lighting.has_skylight,
            registry,
        )?;
        Ok(CapturedQueuedLight {
            identity: Arc::clone(&batch.identity),
            revision: Arc::clone(&batch.revision),
            engine,
        })
    }

    /// Bind an observed engine to a detached live-terrain batch.
    pub fn bind_staged_live_queued_light(
        &self,
        batch: &StagedLiveTerrainBatch,
        tracker: native_world::generation_light::QueuedSectionLightTracker,
        registered: impl IntoIterator<Item = native_world::loaded::ChunkPos>,
        sky_sources: BTreeMap<native_world::loaded::ChunkPos, Arc<[i32; 256]>>,
    ) -> Result<CapturedQueuedLight> {
        self.bind_captured_queued_light(&batch.batch, tracker, registered, sky_sources)
    }

    /// Add an observed saved-chunk load to a captured generation revision.
    /// The caller must account for every light event and verify live changes
    /// independently before publishing or saving generated terrain.
    pub fn stream_captured_saved_chunk_with_queued_light(
        &self,
        batch: &mut CapturedTerrainBatch,
        restored: &RestoredSavedTerrain,
        light: &mut CapturedQueuedLight,
        position: native_world::loaded::ChunkPos,
        order: native_session::worldgen::SavedLightLoadOrder,
    ) -> Result<()> {
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity),
            "terrain belongs to another generation capture"
        );
        let key = native_persistence::world::ChunkPos {
            x: position.x,
            z: position.z,
        };
        let captured = self
            .context
            .inputs()
            .sources()
            .chunks()
            .get(&key)
            .and_then(|source| source.terrain.as_ref())
            .context("saved terrain source was not captured")?;
        ensure!(
            captured.data_version == native_persistence::world::TARGET_DATA_VERSION
                && restored
                    .sources
                    .get(&key)
                    .is_some_and(|source| Arc::ptr_eq(source, &captured.document)),
            "restored terrain source differs from captured input"
        );
        let registry = &self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?
            .blocks;
        batch.import_saved_chunk_with_queued_light(restored, light, position, order, |state| {
            native_session::worldgen::generated_state_is_air(registry, state)
        })
    }

    /// Stream the same observed disk load into a detached live-terrain batch.
    /// Unsaved edits and the live session still need independent verification.
    pub fn stream_staged_live_saved_chunk_with_queued_light(
        &self,
        batch: &mut StagedLiveTerrainBatch,
        restored: &RestoredSavedTerrain,
        light: &mut CapturedQueuedLight,
        position: native_world::loaded::ChunkPos,
        order: native_session::worldgen::SavedLightLoadOrder,
    ) -> Result<()> {
        self.stream_captured_saved_chunk_with_queued_light(
            &mut batch.batch,
            restored,
            light,
            position,
            order,
        )
    }

    /// Register a feature-complete generated chunk in the observed engine.
    pub fn initialize_captured_queued_light(
        &self,
        batch: &mut CapturedTerrainBatch,
        light: &mut CapturedQueuedLight,
        position: native_world::loaded::ChunkPos,
    ) -> Result<()> {
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity) && light.matches_batch(batch),
            "light belongs to another terrain revision"
        );
        ensure!(
            batch.feature_entity_sources.completed.contains(&position),
            "light chunk has not completed features"
        );
        let registry = &self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?
            .blocks;
        batch
            .schedule
            .run_queued_light_initialization_with_registry(
                &batch.terrain,
                position,
                &mut light.engine,
                registry,
            )
    }

    /// Advance a ready generated chunk without discarding observed light.
    pub fn propagate_captured_queued_light(
        &self,
        batch: &mut CapturedTerrainBatch,
        light: &mut CapturedQueuedLight,
        position: native_world::loaded::ChunkPos,
    ) -> Result<native_world::generation_light::VisibleLightSnapshot> {
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity) && light.matches_batch(batch),
            "light belongs to another terrain revision"
        );
        batch
            .schedule
            .run_queued_light(&batch.terrain, position, &mut light.engine)
    }

    /// Run FEATURES for a mixed captured batch, including supported saved
    /// buried-treasure references. Generated starts remain unsupported.
    /// A light-dependent feature fails until an appropriate light view exists.
    /// Results remain intermediate and do not authorize saving the world.
    pub fn run_captured_features(
        &mut self,
        batch: &mut CapturedTerrainBatch,
        context: &SavedDecorationContext,
    ) -> Result<Vec<bool>> {
        self.run_captured_features_inner(batch, context, None, None, None)
    }

    /// Run a feature pass on the detached live terrain without inventing a
    /// light value. Light-dependent features fail and leave the batch intact.
    /// This intermediate result does not authorize publication or saving.
    pub fn run_staged_live_features(
        &mut self,
        batch: &mut StagedLiveTerrainBatch,
        position: native_world::loaded::ChunkPos,
    ) -> Result<Vec<bool>> {
        batch.require_ready_features(position)?;
        let context = self.prepare_captured_decoration(
            &batch.batch,
            native_persistence::world::ChunkPos {
                x: position.x,
                z: position.z,
            },
        )?;
        let placements = self.run_captured_features_inner(
            &mut batch.batch,
            &context,
            None,
            Some(&mut batch.mineshaft_starts),
            None,
        )?;
        batch.feature_order.push(position);
        Ok(placements)
    }

    /// Run features against a caller-verified initial light revision. Any
    /// successful feature pass invalidates that light for this batch.
    pub fn run_captured_features_with_initial_light(
        &mut self,
        batch: &mut CapturedTerrainBatch,
        context: &SavedDecorationContext,
        light: &CapturedFeatureLight,
    ) -> Result<Vec<bool>> {
        self.run_captured_features_inner(batch, context, Some(light), None, None)
    }

    /// Run the captured pass and its ordered light updates atomically.
    pub fn run_captured_features_with_queued_light(
        &mut self,
        batch: &mut CapturedTerrainBatch,
        context: &SavedDecorationContext,
        light: &mut CapturedQueuedLight,
    ) -> Result<Vec<bool>> {
        ensure!(
            light.matches_batch(batch),
            "feature light belongs to another terrain revision"
        );
        let mut engine = light.engine.clone();
        let result =
            self.run_captured_features_inner(batch, context, None, None, Some(&mut engine))?;
        light.engine = engine;
        light.revision = Arc::clone(&batch.revision);
        Ok(result)
    }

    /// Run a live-captured pass without discarding the observed light history.
    pub fn run_staged_live_features_with_queued_light(
        &mut self,
        batch: &mut StagedLiveTerrainBatch,
        position: native_world::loaded::ChunkPos,
        light: &mut CapturedQueuedLight,
    ) -> Result<Vec<bool>> {
        batch.require_ready_features(position)?;
        ensure!(
            light.matches_batch(&batch.batch),
            "feature light belongs to another terrain revision"
        );
        let context = self.prepare_captured_decoration(
            &batch.batch,
            native_persistence::world::ChunkPos {
                x: position.x,
                z: position.z,
            },
        )?;
        let mut engine = light.engine.clone();
        let result = self.run_captured_features_inner(
            &mut batch.batch,
            &context,
            None,
            Some(&mut batch.mineshaft_starts),
            Some(&mut engine),
        )?;
        light.engine = engine;
        light.revision = Arc::clone(&batch.batch.revision);
        batch.feature_order.push(position);
        Ok(result)
    }

    fn run_captured_features_inner(
        &mut self,
        batch: &mut CapturedTerrainBatch,
        context: &SavedDecorationContext,
        light: Option<&CapturedFeatureLight>,
        mut previous_mineshafts: Option<&mut BTreeMap<SavedStructureKey, ResumedMineshaftStart>>,
        mut queued: Option<&mut native_session::worldgen::QueuedGenerationLightEngine>,
    ) -> Result<Vec<bool>> {
        use native_session::worldgen::schedule::GenerationStage;
        ensure!(
            Arc::ptr_eq(&self.identity, &batch.identity)
                && Arc::ptr_eq(&self.identity, &context.identity),
            "decoration belongs to another generation capture"
        );
        ensure!(
            light.is_none_or(|light| light.matches_batch(batch)),
            "feature light belongs to another terrain revision"
        );
        ensure!(
            light.is_none() || queued.is_none(),
            "lighting modes are mutually exclusive"
        );
        let center = native_world::loaded::ChunkPos {
            x: context.position.x,
            z: context.position.z,
        };
        batch.require_unfinished_features(center)?;
        ensure!(
            batch.schedule.ready(center, GenerationStage::Features),
            "features dependencies are incomplete"
        );
        let structures_enabled = self.generate_structures;
        let mut structures = if structures_enabled {
            let references = if let Some(generated) = batch.generated_references.get(&center) {
                generated.references.clone()
            } else {
                let saved = self.restore_saved_structure_references(context.position)?;
                validate_saved_decoration_references(&saved)?;
                saved.references
            };
            Some(self.prepare_captured_structure_plan(
                &batch.treasure_starts,
                previous_mineshafts.as_deref(),
                &references,
                context.position,
            )?)
        } else {
            None
        };
        let captured = self.context.inputs().sources().chunks();
        let mut positions = Vec::with_capacity(9);
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                let x = center
                    .x
                    .checked_add(dx)
                    .context("neighbor chunk X overflow")?;
                let z = center
                    .z
                    .checked_add(dz)
                    .context("neighbor chunk Z overflow")?;
                let position = native_persistence::world::ChunkPos { x, z };
                let expected = captured
                    .get(&position)
                    .context("terrain neighbor was not captured")?;
                let source = batch
                    .sources
                    .get(&position)
                    .context("terrain neighbor is missing")?;
                match (&expected.terrain, source) {
                    (None, None) => {}
                    (Some(saved), Some(source)) if Arc::ptr_eq(&saved.document, source) => {}
                    _ => bail!("terrain neighbor source differs"),
                }
                let selected = context
                    .biomes
                    .chunk([x, z])
                    .context("decoration biome is missing")?;
                let current = batch
                    .biomes
                    .chunk([x, z])
                    .context("batch biome is missing")?;
                ensure!(
                    Arc::ptr_eq(selected, current),
                    "decoration biome differs from batch"
                );
                positions.push(native_world::loaded::ChunkPos { x, z });
            }
        }
        let evidence = self
            .feature_entity_evidence
            .as_ref()
            .context("feature entity evidence is missing")?;
        let entity_free = context
            .plan(self)?
            .features()
            .iter()
            .all(|feature| evidence.get(feature.feature) == Some(&true));
        if self.heightmap_tags.is_none() {
            let blocks = self.resources.vanilla_block_ids()?;
            self.heightmap_tags = Some(native_resources::heightmap_tags::HeightmapTags::load(
                &mut self.resources,
                &blocks,
            )?);
        }
        let previous_terrain = batch.terrain.clone();
        let previous_schedule = batch.schedule.clone();
        let priming = (|| -> Result<()> {
            let tags = self
                .heightmap_tags
                .as_ref()
                .expect("heightmap tags initialized");
            for position in positions {
                if batch.terrain.worldgen_heightmaps(position).is_none() {
                    batch.terrain.ensure_worldgen_heightmaps(position, tags)?;
                }
            }
            Ok(())
        })();
        let needs_mineshaft = structures
            .as_ref()
            .is_some_and(|plan| !plan.mineshafts.is_empty());
        let result = priming.and_then(|()| {
            self.with_saved_prepared_decoration_inner(
                context,
                needs_mineshaft,
                |prepared, registry| {
                    if let Some(structures) = &structures {
                        if needs_mineshaft {
                            let planned = structures.planned_mixed()?;
                            let mut placement =
                                native_session::worldgen::MixedMineshaftPlacementContext {
                                    other: UnavailableOceanPlacement,
                                    blocking_biomes: prepared
                                        .mineshaft_blocking_biomes()
                                        .context("mineshaft biome tags are missing")?,
                                    uuid_factory: |_: [f64; 3]| -> Result<Option<[i32; 4]>> {
                                        bail!("mineshaft minecart UUID source is not bound")
                                    },
                                };
                            let (features, updates) = match (light, queued.as_deref_mut()) {
                                (Some(light), None) => prepared.execute_mixed_with_visible_light(
                                    &mut batch.terrain,
                                    &planned,
                                    &mut placement,
                                    &light.light,
                                )?,
                                (None, Some(engine)) => prepared.execute_mixed_with_queued_light(
                                    &mut batch.terrain,
                                    &planned,
                                    &mut placement,
                                    engine,
                                    registry,
                                )?,
                                (None, None) => prepared.execute_with_mixed_structure_plan(
                                    &mut batch.terrain,
                                    &planned,
                                    &mut placement,
                                )?,
                                (Some(_), Some(_)) => unreachable!("lighting modes are exclusive"),
                            };
                            batch.schedule.complete(center, GenerationStage::Features)?;
                            return Ok((features, SavedStructureUpdates::Mixed(updates)));
                        }
                        let planned = structures.planned()?;
                        let (features, updates) = match (light, queued.as_deref_mut()) {
                            (Some(light), None) => batch
                                .schedule
                                .run_features_with_structure_plan_and_visible_light(
                                    &mut batch.terrain,
                                    prepared,
                                    &planned,
                                    &light.light,
                                )?,
                            (None, Some(engine)) => batch
                                .schedule
                                .run_features_with_structure_plan_and_queued_light(
                                    &mut batch.terrain,
                                    prepared,
                                    &planned,
                                    engine,
                                    registry,
                                )?,
                            (None, None) => batch.schedule.run_features_with_structure_plan(
                                &mut batch.terrain,
                                prepared,
                                &planned,
                            )?,
                            (Some(_), Some(_)) => unreachable!("lighting modes are exclusive"),
                        };
                        Ok((features, SavedStructureUpdates::Simple(updates)))
                    } else {
                        let features = match (light, queued.as_deref_mut()) {
                            (Some(light), None) => batch.schedule.run_features_with_visible_light(
                                &mut batch.terrain,
                                prepared,
                                false,
                                &light.light,
                            )?,
                            (None, Some(engine)) => batch.schedule.run_features_with_queued_light(
                                &mut batch.terrain,
                                prepared,
                                false,
                                engine,
                                registry,
                            )?,
                            (None, None) => batch.schedule.run_features(
                                &mut batch.terrain,
                                prepared,
                                false,
                                None,
                            )?,
                            (Some(_), Some(_)) => unreachable!("lighting modes are exclusive"),
                        };
                        Ok((features, SavedStructureUpdates::Simple(Vec::new())))
                    }
                },
            )
        });
        let (result, updates) = match result {
            Ok(result) => result,
            Err(error) => {
                batch.terrain = previous_terrain;
                batch.schedule = previous_schedule;
                return Err(error);
            }
        };
        if let Some(structures) = structures.as_mut() {
            match updates {
                SavedStructureUpdates::Simple(updates) => structures.apply_updates(updates),
                SavedStructureUpdates::Mixed(updates) => {
                    if let Err(error) = structures.apply_mixed_updates(updates) {
                        batch.terrain = previous_terrain;
                        batch.schedule = previous_schedule;
                        return Err(error);
                    }
                }
            }
            if previous_mineshafts.is_none() && !structures.mineshafts.is_empty() {
                batch.terrain = previous_terrain;
                batch.schedule = previous_schedule;
                bail!("mineshaft state cannot be retained");
            }
            batch.treasure_starts.append(&mut structures.starts);
            if let Some(previous) = previous_mineshafts.as_deref_mut() {
                previous.append(&mut structures.mineshafts);
            }
        }
        batch.feature_entity_sources.complete(center, entity_free);
        batch.revision = Arc::new(());
        Ok(result)
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

    /// Build one captured biome neighborhood in the caller's generation order.
    /// Existing palettes are preserved and only absent chunks are generated.
    /// This is an intermediate input, not a completed or writable chunk. On a
    /// generation error the generator and evaluator must be discarded.
    pub fn prepare_captured_biomes(
        &mut self,
        positions: impl IntoIterator<Item = native_persistence::world::ChunkPos>,
        evaluator: &mut native_resources::density::DensityEvaluator,
    ) -> Result<native_session::worldgen::BiomeNeighborhood> {
        let mut requested = Vec::new();
        let mut seen = BTreeSet::new();
        let mut saved_positions = Vec::new();
        for position in positions {
            ensure!(seen.insert(position), "duplicate captured biome chunk");
            let source = self
                .context
                .inputs()
                .sources()
                .chunks()
                .get(&position)
                .context("biome neighbor was not captured")?;
            if let Some(saved) = &source.terrain {
                reject_saved_blending(&saved.document)?;
                saved_positions.push(position);
            }
            requested.push(position);
        }
        ensure!(!requested.is_empty(), "no captured biome chunks requested");
        let saved = if saved_positions.is_empty() {
            BTreeMap::new()
        } else {
            self.restore_saved_biomes(saved_positions)?
        };
        let mut neighborhood = self.generator.biome_neighborhood()?;
        for position in requested {
            let key = native_world::loaded::ChunkPos {
                x: position.x,
                z: position.z,
            };
            let chunk = match saved.get(&key) {
                Some(chunk) => Arc::clone(chunk),
                None => Arc::new(
                    self.generator
                        .generate_biomes([position.x, position.z], evaluator)?,
                ),
            };
            neighborhood.insert(chunk)?;
        }
        Ok(neighborhood)
    }

    /// Bind restored biome volumes to a schedule whose other saved stage data
    /// has already been imported. Every current requirement must be captured.
    pub fn begin_captured_biome_schedule(
        &mut self,
        schedule: GenerationSchedule,
        saved_positions: impl IntoIterator<Item = native_persistence::world::ChunkPos>,
    ) -> Result<CapturedBiomeSchedule> {
        use native_session::worldgen::schedule::GenerationStage;
        let sources = self.context.inputs().sources().chunks();
        for (pos, _) in schedule.requirements() {
            ensure!(
                sources.contains_key(&native_persistence::world::ChunkPos { x: pos.x, z: pos.z }),
                "generation schedule includes an uncaptured chunk"
            );
        }
        ensure!(schedule.requested_chunks() > 0, "no generation requests");
        let mut positions = Vec::new();
        let mut seen = BTreeSet::new();
        for position in saved_positions {
            ensure!(seen.insert(position), "duplicate saved biome chunk");
            let source = sources
                .get(&position)
                .context("saved biome chunk was not captured")?
                .terrain
                .as_ref()
                .context("saved biome chunk is missing")?;
            reject_saved_blending(&source.document)?;
            let status = source
                .status
                .as_ref()
                .context("saved chunk status is missing")?;
            let status = String::from_utf16(&status.0).context("invalid saved chunk status")?;
            let saved_stage =
                GenerationStage::from_id(&status).context("unknown saved chunk status")?;
            let key = native_world::loaded::ChunkPos {
                x: position.x,
                z: position.z,
            };
            let completed = schedule
                .completed(key)
                .context("saved biome chunk is not requested")?;
            ensure!(
                completed >= GenerationStage::Biomes && completed <= saved_stage,
                "saved biome stage is not restored in the schedule"
            );
            positions.push(position);
        }
        let restored = if positions.is_empty() {
            BTreeMap::new()
        } else {
            self.restore_saved_biomes(positions)?
        };
        let mut biomes = self.generator.biome_neighborhood()?;
        for biome in restored.into_values() {
            biomes.insert(biome)?;
        }
        Ok(CapturedBiomeSchedule {
            identity: Arc::clone(&self.identity),
            schedule,
            biomes,
            terrain: BTreeMap::new(),
            new_terrain: BTreeMap::new(),
            generated_references: BTreeMap::new(),
            generated_starts: BTreeMap::new(),
        })
    }

    fn generate_captured_starts(
        &mut self,
        positions: &[native_world::loaded::ChunkPos],
        check: &mut impl FnMut() -> Result<()>,
    ) -> Result<CapturedGeneratedStarts> {
        if positions.is_empty() || !self.generate_structures {
            return Ok(BTreeMap::new());
        }
        check()?;
        self.ensure_terrain_assets()?;
        if self.heightmap_tags.is_none() {
            let blocks = self.resources.vanilla_block_ids()?;
            self.heightmap_tags = Some(native_resources::heightmap_tags::HeightmapTags::load(
                &mut self.resources,
                &blocks,
            )?);
        }
        let generator = TerrainGenerator::with_structure_context(
            &mut self.resources,
            &self.generator_definition,
            self.context.inputs().settings().seed(),
            self.lighting.min_y,
            self.lighting.height,
        )?;
        let mut batch = StructureGenerationBatch::from_registry(generator, &mut self.resources)?;
        for &position in positions {
            check()?;
            batch.request_starts(position)?;
        }
        let blocks = &self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?
            .blocks;
        let tags = self
            .heightmap_tags
            .as_ref()
            .context("heightmap tags are missing")?;
        let mut classify = |value: &Value| {
            let block = resolve_generated_terrain_state(value, blocks)?;
            let motion = tags.for_block(&block.name)?;
            Ok(native_persistence::heightmap::BlockHeightFlags {
                is_air: matches!(
                    block.name.as_str(),
                    "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                ),
                blocks_motion: motion.blocks_motion,
                blocks_motion_no_leaves: motion.blocks_motion_no_leaves,
                has_fluid: FluidState::from_block(&block)
                    .map_err(anyhow::Error::msg)?
                    .is_some(),
            })
        };
        while !batch.is_complete() {
            check()?;
            batch.step(&mut self.resources, &mut classify)?;
        }
        let mut result = BTreeMap::new();
        for &position in positions {
            check()?;
            ensure!(
                result
                    .insert(position, Arc::new(batch.capture_starts(position)?))
                    .is_none(),
                "duplicate generated structure source"
            );
        }
        check()?;
        self.context.packs().verify()?;
        Ok(result)
    }

    /// Restore saved stages and prepare missing structure starts before biome
    /// generation. Later stages, live state and saving need separate checks.
    pub fn begin_captured_generation_schedule(
        &mut self,
        schedule: GenerationSchedule,
    ) -> Result<CapturedBiomeSchedule> {
        self.begin_captured_generation_schedule_checked(schedule, || Ok(()))
    }

    /// Check cancellation or changing input stamps between bounded steps.
    pub fn begin_captured_generation_schedule_checked(
        &mut self,
        mut schedule: GenerationSchedule,
        mut check: impl FnMut() -> Result<()>,
    ) -> Result<CapturedBiomeSchedule> {
        use native_session::worldgen::schedule::GenerationStage;
        ensure!(schedule.requested_chunks() > 0, "no generation requests");
        let requirements: Vec<_> = schedule.requirements().collect();
        let mut saved_stages = BTreeMap::new();
        let mut missing_starts = Vec::new();
        for &(position, required) in &requirements {
            check()?;
            ensure!(
                schedule.completed(position) == Some(GenerationStage::Empty),
                "generation schedule is not fresh"
            );
            let key = native_persistence::world::ChunkPos {
                x: position.x,
                z: position.z,
            };
            let source = self
                .context
                .inputs()
                .sources()
                .chunks()
                .get(&key)
                .context("generation schedule includes an uncaptured chunk")?;
            if let Some(saved) = &source.terrain {
                ensure!(
                    saved.data_version == native_persistence::world::TARGET_DATA_VERSION,
                    "unsupported saved generation data version"
                );
                reject_saved_blending(&saved.document)?;
                let status = saved
                    .status
                    .as_ref()
                    .context("saved chunk status is missing")?;
                let status = String::from_utf16(&status.0).context("invalid saved chunk status")?;
                let stage =
                    GenerationStage::from_id(&status).context("unknown saved chunk status")?;
                saved_stages.insert(position, stage);
                if required >= GenerationStage::StructureStarts {
                    let restored = self.restore_captured_structure_starts(
                        key,
                        stage < GenerationStage::StructureStarts,
                    )?;
                    self.captured_structure_starts
                        .insert(position, Arc::new(restored));
                }
                if required >= GenerationStage::StructureReferences
                    && stage >= GenerationStage::StructureReferences
                {
                    let references = self.restore_saved_structure_references(key)?;
                    validate_saved_decoration_references(&references)?;
                }
            } else if required >= GenerationStage::StructureStarts {
                missing_starts.push(position);
            }
        }
        let generated_starts = self.generate_captured_starts(&missing_starts, &mut check)?;
        let saved_biomes: Vec<_> = requirements
            .iter()
            .filter_map(|(position, required)| {
                (required >= &GenerationStage::Biomes
                    && saved_stages
                        .get(position)
                        .is_some_and(|stage| *stage >= GenerationStage::Biomes))
                .then_some(native_persistence::world::ChunkPos {
                    x: position.x,
                    z: position.z,
                })
            })
            .collect();
        let restored_biomes = if saved_biomes.is_empty() {
            BTreeMap::new()
        } else {
            self.restore_saved_biomes(saved_biomes)?
        };
        check()?;
        let imported = requirements
            .iter()
            .filter_map(|(position, required)| {
                let saved = *saved_stages.get(position)?;
                let stage = saved.min(*required).min(GenerationStage::Biomes);
                (stage > GenerationStage::Empty).then_some((*position, stage))
            })
            .collect::<Vec<_>>();
        schedule.restore_imported(imported)?;
        for &(position, required) in &requirements {
            check()?;
            if required >= GenerationStage::StructureStarts
                && schedule.completed(position) == Some(GenerationStage::Empty)
            {
                ensure!(
                    !self.generate_structures || generated_starts.contains_key(&position),
                    "generated structure start is missing"
                );
                schedule.complete(position, GenerationStage::StructureStarts)?;
            }
        }
        let mut generated_references = BTreeMap::new();
        for &(position, required) in &requirements {
            check()?;
            if required >= GenerationStage::StructureReferences
                && schedule.completed(position) == Some(GenerationStage::StructureStarts)
            {
                ensure!(
                    schedule.ready(position, GenerationStage::StructureReferences),
                    "structure reference dependencies are incomplete"
                );
                let references =
                    self.collect_captured_references(&schedule, position, &generated_starts)?;
                let key = native_persistence::world::ChunkPos {
                    x: position.x,
                    z: position.z,
                };
                if let Some(saved) = self.context.inputs().sources().chunks()[&key]
                    .terrain
                    .as_ref()
                {
                    let mut source = (*saved.document).clone();
                    merge_generated_structure_references(&mut source, &references)?;
                }
                schedule.complete(position, GenerationStage::StructureReferences)?;
                generated_references.insert(position, references);
            }
        }
        let mut biomes = self.generator.biome_neighborhood()?;
        for biome in restored_biomes.into_values() {
            biomes.insert(biome)?;
        }
        Ok(CapturedBiomeSchedule {
            identity: Arc::clone(&self.identity),
            schedule,
            biomes,
            terrain: BTreeMap::new(),
            new_terrain: BTreeMap::new(),
            generated_references,
            generated_starts,
        })
    }

    /// Generate biomes for a captured absent chunk or a saved protochunk that
    /// has not reached BIOMES. Saved source NBT remains unchanged; the result
    /// is an intermediate input and does not authorize a world write. Discard
    /// the generator and evaluator after a generation error.
    pub fn run_captured_biomes(
        &mut self,
        work: &mut CapturedBiomeSchedule,
        evaluator: &mut native_resources::density::DensityEvaluator,
        position: native_world::loaded::ChunkPos,
    ) -> Result<()> {
        ensure!(
            Arc::ptr_eq(&self.identity, &work.identity),
            "biomes belong to another generation capture"
        );
        let source = self
            .context
            .inputs()
            .sources()
            .chunks()
            .get(&native_persistence::world::ChunkPos {
                x: position.x,
                z: position.z,
            })
            .context("biome chunk was not captured")?;
        if let Some(saved) = &source.terrain {
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
            let stage = GenerationStage::from_id(&status).context("unknown saved chunk status")?;
            ensure!(
                stage < GenerationStage::Biomes,
                "saved biome chunk must be restored"
            );
            reject_saved_blending(&saved.document)?;
        }
        work.schedule
            .run_biomes(&mut self.generator, evaluator, &mut work.biomes, position)
    }

    /// Identify which terrain work can be generated from this capture. Saved
    /// terrain is reported separately and must never be regenerated in place.
    pub fn captured_terrain_input(
        &mut self,
        work: &CapturedBiomeSchedule,
        position: native_world::loaded::ChunkPos,
    ) -> Result<CapturedTerrainInput> {
        use native_session::worldgen::schedule::GenerationStage;
        ensure!(
            Arc::ptr_eq(&self.identity, &work.identity),
            "terrain belongs to another generation capture"
        );
        ensure!(
            work.schedule
                .required(position)
                .is_some_and(|stage| stage >= GenerationStage::Terrain),
            "terrain chunk is not required"
        );
        let key = native_persistence::world::ChunkPos {
            x: position.x,
            z: position.z,
        };
        let source = self
            .context
            .inputs()
            .sources()
            .chunks()
            .get(&key)
            .context("terrain source was not captured")?;
        let status = source
            .terrain
            .as_ref()
            .map(|saved| {
                ensure!(
                    saved.data_version == native_persistence::world::TARGET_DATA_VERSION,
                    "unsupported saved terrain data version"
                );
                let status = saved
                    .status
                    .as_ref()
                    .context("saved chunk status is missing")?;
                let status = String::from_utf16(&status.0).context("invalid saved chunk status")?;
                GenerationStage::from_id(&status).context("unknown saved chunk status")
            })
            .transpose()?;
        if let Some(stage) = status.filter(|stage| *stage >= GenerationStage::Terrain) {
            return Ok(CapturedTerrainInput::Saved { stage });
        }
        let keys = if status.is_some_and(|stage| stage >= GenerationStage::StructureReferences) {
            let references = self.restore_saved_structure_references(key)?;
            validate_saved_decoration_references(&references)?;
            references.references.membership().keys().cloned().collect()
        } else {
            work.generated_references
                .get(&position)
                .context("generated structure references are missing")?
                .references
                .membership()
                .keys()
                .cloned()
                .collect()
        };
        Ok(if status.is_some() {
            CapturedTerrainInput::Resume {
                structure_keys: keys,
            }
        } else {
            CapturedTerrainInput::New {
                structure_keys: keys,
            }
        })
    }

    /// Infer a structure-key order for an intermediate terrain pass only when
    /// every possible Java key order yields the same clipped contributions.
    pub fn unambiguous_captured_terrain_order(
        &mut self,
        work: &CapturedBiomeSchedule,
        position: native_world::loaded::ChunkPos,
    ) -> Result<Option<Vec<ResourceId>>> {
        let keys = match self.captured_terrain_input(work, position)? {
            CapturedTerrainInput::New { structure_keys }
            | CapturedTerrainInput::Resume { structure_keys } => structure_keys,
            CapturedTerrainInput::Saved { .. } => bail!("saved terrain needs no generation order"),
        };
        if keys.len() <= 1 {
            return Ok(Some(keys));
        }
        let saved;
        let references = if let Some(generated) = work.generated_references.get(&position) {
            &generated.references
        } else {
            saved =
                self.restore_saved_structure_references(native_persistence::world::ChunkPos {
                    x: position.x,
                    z: position.z,
                })?;
            validate_saved_decoration_references(&saved)?;
            &saved.references
        };
        native_session::worldgen::saved_structure_starts::unambiguous_beardifier_structure_order(
            position,
            references,
            |id, source| self.captured_terrain_pieces(&work.generated_starts, id, source),
        )
    }

    /// Resume a captured protochunk using the schedule's already restored or
    /// generated biome neighborhood. Stages before saved structure references
    /// require new structure generation to be disabled. The result is not writable.
    pub fn run_captured_saved_terrain(
        &mut self,
        work: &mut CapturedBiomeSchedule,
        evaluator: &mut native_resources::density::DensityEvaluator,
        position: native_world::loaded::ChunkPos,
        structure_order: &[ResourceId],
    ) -> Result<()> {
        use native_session::worldgen::schedule::GenerationStage;
        ensure!(
            Arc::ptr_eq(&self.identity, &work.identity),
            "terrain belongs to another generation capture"
        );
        ensure!(
            work.schedule.ready(position, GenerationStage::Terrain),
            "terrain dependencies are incomplete"
        );
        let target = native_persistence::world::ChunkPos {
            x: position.x,
            z: position.z,
        };
        ensure!(
            !work.terrain.contains_key(&target),
            "terrain chunk already exists"
        );
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                let x = position
                    .x
                    .checked_add(dx)
                    .context("neighbor chunk X overflow")?;
                let z = position
                    .z
                    .checked_add(dz)
                    .context("neighbor chunk Z overflow")?;
                ensure!(
                    work.biomes.chunk([x, z]).is_some(),
                    "terrain biome neighbor is missing"
                );
            }
        }
        let saved = self
            .context
            .inputs()
            .sources()
            .chunks()
            .get(&target)
            .and_then(|source| source.terrain.as_ref())
            .context("saved terrain target is missing")?;
        let saved_stage = validate_empty_preterrain_target(saved, true)?;
        let source_document = Arc::clone(&saved.document);
        let (early_structure, generated_references) =
            if saved_stage < GenerationStage::StructureReferences {
                if let Some(references) = work.generated_references.get(&position) {
                    let mut validation = (*source_document).clone();
                    merge_generated_structure_references(&mut validation, references)?;
                    (
                        Some(Arc::new(self.beardifier_from_captured_references(
                            position,
                            references,
                            structure_order,
                            &work.generated_starts,
                        )?)),
                        None,
                    )
                } else {
                    let (structure, references) = self.captured_beardifier(
                        &work.schedule,
                        position,
                        structure_order,
                        &work.generated_starts,
                    )?;
                    let mut validation = (*source_document).clone();
                    merge_generated_structure_references(&mut validation, &references)?;
                    (Some(Arc::new(structure)), Some(references))
                }
            } else {
                ensure!(
                    !work.generated_references.contains_key(&position),
                    "unexpected generated structure references"
                );
                (None, None)
            };
        let (terrain, biome) = self.generate_saved_terrain_from_biomes(
            target,
            structure_order,
            evaluator,
            &work.biomes,
            true,
            early_structure,
        )?;
        let terrain = self.materialize_saved_terrain(target, terrain, &biome)?;
        work.terrain.insert(target, terrain);
        if let Err(error) = work.schedule.complete(position, GenerationStage::Terrain) {
            work.terrain.remove(&target);
            return Err(error);
        }
        if let Some(references) = generated_references {
            work.generated_references.insert(position, references);
        }
        Ok(())
    }

    /// Generate intermediate terrain for an absent chunk when structure-key
    /// order is unambiguous. This does not authorize writing the result.
    pub fn run_captured_new_terrain(
        &mut self,
        work: &mut CapturedBiomeSchedule,
        evaluator: &mut native_resources::density::DensityEvaluator,
        position: native_world::loaded::ChunkPos,
    ) -> Result<()> {
        self.run_captured_new_terrain_with_structure_order(work, evaluator, position, &[])
    }

    /// Generate an absent chunk using the supplied Java structure-key order.
    pub fn run_captured_new_terrain_with_structure_order(
        &mut self,
        work: &mut CapturedBiomeSchedule,
        evaluator: &mut native_resources::density::DensityEvaluator,
        position: native_world::loaded::ChunkPos,
        structure_order: &[ResourceId],
    ) -> Result<()> {
        use native_session::worldgen::schedule::GenerationStage;
        ensure!(
            Arc::ptr_eq(&self.identity, &work.identity),
            "terrain belongs to another generation capture"
        );
        ensure!(
            work.schedule.ready(position, GenerationStage::Terrain),
            "terrain dependencies are incomplete"
        );
        let target = native_persistence::world::ChunkPos {
            x: position.x,
            z: position.z,
        };
        let source = self
            .context
            .inputs()
            .sources()
            .chunks()
            .get(&target)
            .context("terrain target was not captured")?;
        ensure!(source.terrain.is_none(), "saved terrain must be resumed");
        ensure!(
            !work.new_terrain.contains_key(&target),
            "terrain chunk already exists"
        );
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                let x = position
                    .x
                    .checked_add(dx)
                    .context("neighbor chunk X overflow")?;
                let z = position
                    .z
                    .checked_add(dz)
                    .context("neighbor chunk Z overflow")?;
                ensure!(
                    work.biomes.chunk([x, z]).is_some(),
                    "terrain biome neighbor is missing"
                );
            }
        }
        let biome = Arc::clone(
            work.biomes
                .chunk([position.x, position.z])
                .context("terrain biome is missing")?,
        );
        let (structure, generated_references) =
            if let Some(references) = work.generated_references.get(&position) {
                (
                    self.beardifier_from_captured_references(
                        position,
                        references,
                        structure_order,
                        &work.generated_starts,
                    )?,
                    None,
                )
            } else {
                let (structure, references) = self.captured_beardifier(
                    &work.schedule,
                    position,
                    structure_order,
                    &work.generated_starts,
                )?;
                (structure, Some(references))
            };
        let terrain = self.generate_terrain_from_biomes(
            target,
            evaluator,
            &work.biomes,
            Some(Arc::new(structure)),
        )?;
        let dimension = self.context.inputs().sources().dimension();
        let dimension = format!("{}:{}", dimension.namespace, dimension.path);
        let registry = &self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?
            .blocks;
        let terrain = native_session::worldgen::materialize_carved_terrain(
            terrain,
            &biome,
            &dimension,
            |value| resolve_generated_terrain_state(value, registry),
        )?;
        work.new_terrain.insert(target, terrain);
        if let Err(error) = work.schedule.complete(position, GenerationStage::Terrain) {
            work.new_terrain.remove(&target);
            return Err(error);
        }
        if let Some(references) = generated_references {
            work.generated_references.insert(position, references);
        }
        Ok(())
    }

    /// Assemble generated and resumed terrain without conflating an absent
    /// source with a saved protochunk. The result is not writable or playable.
    pub fn assemble_captured_terrain(
        &self,
        work: CapturedBiomeSchedule,
    ) -> Result<CapturedTerrainBatch> {
        self.assemble_captured_terrain_checked(work, &mut || Ok(()))
    }

    fn assemble_captured_terrain_checked(
        &self,
        work: CapturedBiomeSchedule,
        check: &mut impl FnMut() -> Result<()>,
    ) -> Result<CapturedTerrainBatch> {
        use native_session::worldgen::schedule::GenerationStage;
        check()?;
        ensure!(
            Arc::ptr_eq(&self.identity, &work.identity),
            "terrain belongs to another generation capture"
        );
        ensure!(
            !work.terrain.is_empty() || !work.new_terrain.is_empty(),
            "no generated terrain to assemble"
        );
        let captured = self.context.inputs().sources().chunks();
        let mut sources = BTreeMap::new();
        for (position, chunk) in &work.terrain {
            check()?;
            let expected = captured
                .get(position)
                .and_then(|source| source.terrain.as_ref())
                .context("saved terrain source missing")?;
            ensure!(
                Arc::ptr_eq(&self.identity, &chunk.identity)
                    && Arc::ptr_eq(&expected.document, &chunk.source),
                "saved terrain source differs"
            );
            ensure!(
                sources
                    .insert(*position, Some(Arc::clone(&chunk.source)))
                    .is_none(),
                "duplicate terrain chunk"
            );
        }
        for position in work.new_terrain.keys() {
            check()?;
            let source = captured
                .get(position)
                .context("new terrain source was not captured")?;
            ensure!(source.terrain.is_none(), "new terrain has a saved source");
            ensure!(
                sources.insert(*position, None).is_none(),
                "duplicate terrain chunk"
            );
        }
        for position in sources.keys() {
            check()?;
            let key = native_world::loaded::ChunkPos {
                x: position.x,
                z: position.z,
            };
            ensure!(
                work.schedule
                    .completed(key)
                    .is_some_and(|stage| stage >= GenerationStage::Terrain),
                "terrain stage is incomplete"
            );
        }
        let CapturedBiomeSchedule {
            identity,
            schedule,
            biomes,
            terrain,
            mut new_terrain,
            generated_references,
            generated_starts,
        } = work;
        let mut saved = terrain.into_values();
        let mut world = if let Some(first) = saved.next() {
            native_session::worldgen::CarvedTerrainWorld::new(first.terrain)
        } else {
            let (_, first) = new_terrain
                .pop_first()
                .expect("nonempty terrain was checked");
            native_session::worldgen::CarvedTerrainWorld::new(first)
        };
        for chunk in saved {
            check()?;
            world.insert(&chunk.terrain)?;
        }
        for chunk in new_terrain.into_values() {
            check()?;
            world.insert(&chunk)?;
        }
        check()?;
        Ok(CapturedTerrainBatch {
            identity,
            schedule,
            biomes,
            terrain: world,
            sources,
            saved_stages: BTreeMap::new(),
            generated_references,
            generated_starts,
            treasure_starts: BTreeMap::new(),
            feature_entity_sources: ResumedFeatureEntitySources::default(),
            revision: Arc::new(()),
        })
    }

    /// Add already-generated saved terrain from the same capture alongside
    /// generated chunks. Only the restored terrain stage is imported into the
    /// schedule; later-stage state and live changes still require validation.
    pub fn assemble_captured_terrain_with_saved(
        &self,
        work: CapturedBiomeSchedule,
        restored: &RestoredSavedTerrain,
        positions: impl IntoIterator<Item = native_world::loaded::ChunkPos>,
    ) -> Result<CapturedTerrainBatch> {
        self.assemble_captured_terrain_with_saved_checked(work, restored, positions, || Ok(()))
    }

    /// Assemble a captured terrain batch while checking cancellation and disk
    /// identity between chunk imports. On failure no partial result is returned.
    pub fn assemble_captured_terrain_with_saved_checked(
        &self,
        work: CapturedBiomeSchedule,
        restored: &RestoredSavedTerrain,
        positions: impl IntoIterator<Item = native_world::loaded::ChunkPos>,
        mut check: impl FnMut() -> Result<()>,
    ) -> Result<CapturedTerrainBatch> {
        self.assemble_captured_terrain_with_sources_checked(
            work,
            restored,
            positions,
            &restored.world,
            &restored.heightmaps,
            &mut check,
        )
    }

    fn assemble_captured_terrain_with_sources_checked(
        &self,
        work: CapturedBiomeSchedule,
        restored: &RestoredSavedTerrain,
        positions: impl IntoIterator<Item = native_world::loaded::ChunkPos>,
        world: &native_world::loaded::World,
        heightmaps: &BTreeMap<
            native_world::loaded::ChunkPos,
            native_persistence::heightmap::SavedGenerationHeightmaps,
        >,
        check: &mut impl FnMut() -> Result<()>,
    ) -> Result<CapturedTerrainBatch> {
        use native_session::worldgen::schedule::GenerationStage;
        check()?;
        ensure!(
            Arc::ptr_eq(&self.identity, &restored.identity),
            "saved terrain belongs to another capture"
        );
        let mut batch = self.assemble_captured_terrain_checked(work, &mut *check)?;
        let captured = self.context.inputs().sources().chunks();
        let mut imported = Vec::new();
        for position in positions {
            check()?;
            let key = native_persistence::world::ChunkPos {
                x: position.x,
                z: position.z,
            };
            ensure!(
                batch
                    .schedule
                    .required(position)
                    .is_some_and(|stage| stage >= GenerationStage::Terrain),
                "saved terrain is not required"
            );
            let saved = captured
                .get(&key)
                .and_then(|source| source.terrain.as_ref())
                .context("saved terrain source is missing")?;
            ensure!(
                restored
                    .sources
                    .get(&key)
                    .is_some_and(|source| Arc::ptr_eq(source, &saved.document)),
                "restored terrain source differs"
            );
            let status = saved
                .status
                .as_ref()
                .context("saved chunk status is missing")?;
            let status = String::from_utf16(&status.0).context("invalid saved chunk status")?;
            let stage = GenerationStage::from_id(&status).context("unknown saved chunk status")?;
            ensure!(
                stage >= GenerationStage::Terrain,
                "saved chunk has not generated terrain"
            );
            ensure!(
                batch.schedule.completed(position) == Some(GenerationStage::Biomes),
                "saved terrain prerequisites are incomplete"
            );
            ensure!(!batch.sources.contains_key(&key), "duplicate terrain chunk");
            let heightmaps = heightmaps
                .get(&position)
                .context("saved heightmaps are missing")?;
            let marks = restored
                .postprocessing
                .get(&position)
                .context("saved postprocessing is missing")?;
            batch
                .terrain
                .insert_saved_chunk(world, position, heightmaps, marks.as_ref())?;
            batch.sources.insert(key, Some(Arc::clone(&saved.document)));
            batch.saved_stages.insert(position, stage);
            imported.push((position, GenerationStage::Terrain));
        }
        check()?;
        batch.schedule.restore_imported(imported)?;
        check()?;
        Ok(batch)
    }

    /// Build a detached terrain view using the staged live blocks and
    /// verified heightmaps. Lighting, ticks, entities and POI remain unbound.
    pub fn assemble_captured_terrain_with_live_stage_checked(
        &mut self,
        work: CapturedBiomeSchedule,
        restored: &RestoredSavedTerrain,
        live: &LiveGenerationTerrainStage,
        positions: impl IntoIterator<Item = native_world::loaded::ChunkPos>,
        mut check: impl FnMut() -> Result<()>,
    ) -> Result<StagedLiveTerrainBatch> {
        use native_persistence::heightmap::BlockHeightFlags;

        check()?;
        self.validate_restored_capture(restored)?;
        ensure!(
            Arc::ptr_eq(&self.identity, &live.identity),
            "live terrain belongs to another generation capture"
        );
        let positions: Vec<_> = positions.into_iter().collect();
        let overlay = live
            .terrain
            .overlay_saved_world(positions.iter().copied(), &mut check)?;
        self.ensure_terrain_assets()?;
        if self.heightmap_tags.is_none() {
            let blocks = self.resources.vanilla_block_ids()?;
            self.heightmap_tags = Some(native_resources::heightmap_tags::HeightmapTags::load(
                &mut self.resources,
                &blocks,
            )?);
        }
        let registry = &self
            .terrain_assets
            .as_ref()
            .context("terrain registry is missing")?
            .blocks;
        let tags = self
            .heightmap_tags
            .as_ref()
            .context("heightmap tags are missing")?;
        let defaults = registry.default_states();
        let mut heightmaps = BTreeMap::new();
        for &position in &positions {
            check()?;
            let source = self
                .context
                .inputs()
                .sources()
                .chunks()
                .get(&native_persistence::world::ChunkPos {
                    x: position.x,
                    z: position.z,
                })
                .and_then(|chunk| chunk.terrain.as_ref())
                .context("saved terrain source is missing")?;
            let maps = live
                .terrain
                .saved_heightmaps_after_edits(position, source, |state| {
                    check()?;
                    let name = String::from_utf16(&state.name.0)
                        .context("block state name is not valid UTF-16")?;
                    let mut properties = defaults
                        .get(&name)
                        .cloned()
                        .context("block state defaults are unavailable")?;
                    let mut seen = BTreeSet::new();
                    for (key, value) in &state.properties {
                        let key = String::from_utf16(&key.0)
                            .context("block property name is not valid UTF-16")?;
                        let value = String::from_utf16(&value.0)
                            .context("block property value is not valid UTF-16")?;
                        ensure!(seen.insert(key.clone()), "duplicate block property");
                        properties.insert(key, value);
                    }
                    ensure!(
                        registry.is_canonical_state(&name, &properties),
                        "unsupported heightmap block state: {name}"
                    );
                    let block = BlockState { name, properties };
                    let motion = tags.for_block(&block.name)?;
                    Ok(BlockHeightFlags {
                        is_air: matches!(
                            block.name.as_str(),
                            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                        ),
                        blocks_motion: motion.blocks_motion,
                        blocks_motion_no_leaves: motion.blocks_motion_no_leaves,
                        has_fluid: FluidState::from_block(&block)
                            .map_err(anyhow::Error::msg)?
                            .is_some(),
                    })
                })?;
            ensure!(
                heightmaps.insert(position, maps).is_none(),
                "duplicate saved terrain chunk"
            );
        }
        check()?;
        let batch = self.assemble_captured_terrain_with_sources_checked(
            work,
            restored,
            positions,
            overlay.world(),
            &heightmaps,
            &mut check,
        )?;
        Ok(StagedLiveTerrainBatch {
            batch,
            feature_order: Vec::new(),
            mineshaft_starts: BTreeMap::new(),
        })
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
        let registry = self.structure_registry()?;
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
        self.restore_captured_structure_starts(position, false)
    }

    fn restore_captured_structure_starts(
        &mut self,
        position: native_persistence::world::ChunkPos,
        allow_unstarted: bool,
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
        let registry = self.structure_registry()?;
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
        ensure!(
            saved.data_version == native_persistence::world::TARGET_DATA_VERSION,
            "unsupported saved structure data version"
        );
        let status = saved
            .status
            .as_ref()
            .context("saved chunk status is missing")?;
        let status = String::from_utf16(&status.0).context("invalid saved chunk status")?;
        ensure!(
            GenerationStage::from_id(&status)
                .is_some_and(|stage| stage >= GenerationStage::StructureStarts
                    || (allow_unstarted && !self.generate_structures)),
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

    /// Reconstruct the target's reference stage from captured starts. This
    /// validates every source, including absent chunks, before terrain runs.
    fn captured_beardifier(
        &mut self,
        schedule: &GenerationSchedule,
        target: native_world::loaded::ChunkPos,
        structure_order: &[ResourceId],
        generated_starts: &CapturedGeneratedStarts,
    ) -> Result<(
        native_core::beardifier::Beardifier,
        CapturedGeneratedReferences,
    )> {
        let references = self.collect_captured_references(schedule, target, generated_starts)?;
        let beardifier = self.beardifier_from_captured_references(
            target,
            &references,
            structure_order,
            generated_starts,
        )?;
        Ok((beardifier, references))
    }

    fn collect_captured_references(
        &mut self,
        schedule: &GenerationSchedule,
        target: native_world::loaded::ChunkPos,
        generated_starts: &CapturedGeneratedStarts,
    ) -> Result<CapturedGeneratedReferences> {
        use native_session::worldgen::{
            schedule::GenerationStage,
            structure_references::{
                collect_ordered_structure_references,
                collect_ordered_structure_references_with_saved,
                restore_saved_structure_references,
            },
        };
        let mut starts = BTreeMap::new();
        for dx in -8i32..=8 {
            for dz in -8i32..=8 {
                let position = native_world::loaded::ChunkPos {
                    x: target
                        .x
                        .checked_add(dx)
                        .context("structure chunk X overflow")?,
                    z: target
                        .z
                        .checked_add(dz)
                        .context("structure chunk Z overflow")?,
                };
                ensure!(
                    schedule
                        .completed(position)
                        .is_some_and(|stage| stage >= GenerationStage::StructureStarts),
                    "structure starts are not complete at {},{}",
                    position.x,
                    position.z
                );
                let saved_position = native_persistence::world::ChunkPos {
                    x: position.x,
                    z: position.z,
                };
                let saved = self
                    .context
                    .inputs()
                    .sources()
                    .chunks()
                    .get(&saved_position)
                    .context("structure source was not captured")?
                    .terrain
                    .is_some();
                if !saved {
                    if let Some(generated) = generated_starts.get(&position) {
                        ensure!(
                            generated.position() == position,
                            "structure start source differs"
                        );
                        starts.insert(position, generated.reference_bounds().to_vec());
                    } else {
                        ensure!(
                            !self.generate_structures,
                            "generated structure starts are not captured"
                        );
                    }
                    continue;
                }
                if !self.captured_structure_starts.contains_key(&position) {
                    let restored = self.restore_captured_structure_starts(saved_position, true)?;
                    self.captured_structure_starts
                        .insert(position, Arc::new(restored));
                }
                starts.insert(
                    position,
                    self.captured_structure_starts[&position]
                        .iter()
                        .filter_map(|(id, start)| start.reference_bounds(id).transpose())
                        .collect::<Result<Vec<_>>>()?,
                );
            }
        }
        let target_pos = native_persistence::world::ChunkPos {
            x: target.x,
            z: target.z,
        };
        let original = self
            .context
            .inputs()
            .sources()
            .chunks()
            .get(&target_pos)
            .context("structure target was not captured")?
            .terrain
            .as_ref()
            .map(|saved| original_structure_references(&saved.document).map(|value| value.cloned()))
            .transpose()?
            .flatten();
        let saved = if original.is_some() {
            let registry = self.structure_registry()?;
            Some(restore_saved_structure_references(
                target,
                original.as_ref(),
                |id| Ok(registry.contains(id)),
            )?)
        } else {
            None
        };
        let lookup = |position| Ok(starts.get(&position).cloned().unwrap_or_default());
        let references = match &saved {
            Some(saved) => collect_ordered_structure_references_with_saved(target, saved, lookup)?,
            None => collect_ordered_structure_references(target, lookup)?,
        };
        Ok(CapturedGeneratedReferences {
            original,
            references,
        })
    }

    fn beardifier_from_captured_references(
        &self,
        target: native_world::loaded::ChunkPos,
        references: &CapturedGeneratedReferences,
        structure_order: &[ResourceId],
        generated_starts: &CapturedGeneratedStarts,
    ) -> Result<native_core::beardifier::Beardifier> {
        use native_session::worldgen::saved_structure_starts::beardifier_for_ordered_references;
        beardifier_for_ordered_references(
            target,
            &references.references,
            structure_order,
            |id, position| self.captured_terrain_pieces(generated_starts, id, position),
        )
    }

    fn captured_terrain_pieces(
        &self,
        generated_starts: &CapturedGeneratedStarts,
        id: &ResourceId,
        position: native_world::loaded::ChunkPos,
    ) -> Result<Vec<native_core::beardifier::StructurePiece>> {
        use native_session::worldgen::saved_structure_starts::RestoredStructureStart;
        if let Some(generated) = generated_starts.get(&position) {
            ensure!(
                generated.position() == position,
                "structure start source differs"
            );
            return generated.terrain_pieces(id);
        }
        let start = self
            .captured_structure_starts
            .get(&position)
            .and_then(|entries| entries.get(id))
            .context("referenced structure start is missing")?;
        if let RestoredStructureStart::Present { chunk, .. } = start {
            ensure!(
                *chunk == position,
                "saved structure start belongs to another chunk"
            );
        }
        start.terrain_pieces()
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
        self.generate_saved_terrain_parts(target, structure_order, evaluator, None)
            .map(|(terrain, _)| terrain)
    }

    /// Resume terrain with a mixed saved and absent biome neighborhood. The
    /// caller supplies the actual generation order and matching evaluator
    /// history; this does not establish schedule or world-save completeness.
    pub fn generate_saved_terrain_with_biome_order(
        &mut self,
        target: native_persistence::world::ChunkPos,
        structure_order: &[ResourceId],
        evaluator: &mut native_resources::density::DensityEvaluator,
        biome_order: &[native_persistence::world::ChunkPos],
    ) -> Result<native_resources::surface_pass::SurfaceStageChunk> {
        self.generate_saved_terrain_parts(target, structure_order, evaluator, Some(biome_order))
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
            self.generate_saved_terrain_parts(target, structure_order, evaluator, None)?;
        self.materialize_saved_terrain(target, terrain, &biome)
    }

    fn materialize_saved_terrain(
        &self,
        target: native_persistence::world::ChunkPos,
        terrain: native_resources::surface_pass::SurfaceStageChunk,
        biome: &native_session::worldgen::QuartBiomeChunk,
    ) -> Result<ResumedSavedTerrain> {
        let dimension = self.context.inputs().sources().dimension();
        let dimension = format!("{}:{}", dimension.namespace, dimension.path);
        let registry = &self
            .terrain_assets
            .as_ref()
            .expect("terrain assets initialized")
            .blocks;
        let terrain = native_session::worldgen::materialize_carved_terrain(
            terrain,
            biome,
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

    /// Merge regenerated blocks and, for earlier protochunks, generated
    /// biomes into the exact captured source. Status and other root data remain
    /// untouched; this intermediate draft does not authorize a world write.
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
        let stage = validate_empty_preterrain_target(captured, true)?;
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
        let original = unique_root_field(&document, "sections")?;
        let sections = if stage < native_session::worldgen::schedule::GenerationStage::Biomes {
            native_persistence::generated_section::merge_prebiome_stage_sections(
                original, &sections,
            )?
        } else {
            native_persistence::generated_section::merge_biome_stage_blocks(
                original.context("saved sections are missing")?,
                &sections,
            )?
        };
        replace_unique_root_field(&mut document, "sections", sections)?;
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
            world.terrain.ensure_worldgen_heightmaps(
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
        biome_order: Option<&[native_persistence::world::ChunkPos]>,
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
                positions.push(pos);
            }
        }
        let order = if let Some(order) = biome_order {
            let expected: BTreeSet<_> = positions.iter().copied().collect();
            let actual: BTreeSet<_> = order.iter().copied().collect();
            ensure!(
                order.len() == expected.len() && actual == expected,
                "biome generation order does not match terrain neighbors"
            );
            order
        } else {
            for position in &positions {
                let source = self
                    .context
                    .inputs()
                    .sources()
                    .chunks()
                    .get(position)
                    .context("biome neighbor was not captured")?;
                ensure!(
                    source.terrain.is_some(),
                    "missing biome neighbor requires a generation order"
                );
            }
            &positions
        };
        let neighborhood = self.prepare_captured_biomes(order.iter().copied(), evaluator)?;
        self.generate_saved_terrain_from_biomes(
            target,
            structure_order,
            evaluator,
            &neighborhood,
            false,
            None,
        )
    }

    fn generate_saved_terrain_from_biomes(
        &mut self,
        target: native_persistence::world::ChunkPos,
        structure_order: &[ResourceId],
        evaluator: &mut native_resources::density::DensityEvaluator,
        neighborhood: &native_session::worldgen::BiomeNeighborhood,
        allow_pre_biomes: bool,
        early_structure: Option<Arc<native_core::beardifier::Beardifier>>,
    ) -> Result<(
        native_resources::surface_pass::SurfaceStageChunk,
        Arc<native_session::worldgen::QuartBiomeChunk>,
    )> {
        use native_session::worldgen::schedule::GenerationStage;
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
        let saved_stage = validate_empty_preterrain_target(source, allow_pre_biomes)?;
        ensure!(
            saved_stage >= GenerationStage::StructureReferences || !self.generate_structures,
            "earlier saved stages require generated structure references"
        );
        let biome = Arc::clone(
            neighborhood
                .chunk([target.x, target.z])
                .context("target biome is missing")?,
        );
        // Existing references remain effective even if new structure starts
        // have since been disabled in the world's settings.
        let structure = if saved_stage >= GenerationStage::StructureReferences {
            ensure!(
                early_structure.is_none(),
                "unexpected generated structure references"
            );
            Some(Arc::new(
                self.restore_saved_beardifier(target, structure_order)?,
            ))
        } else {
            Some(early_structure.context("generated structure references are missing")?)
        };
        let terrain =
            self.generate_terrain_from_biomes(target, evaluator, neighborhood, structure)?;
        Ok((terrain, biome))
    }

    fn generate_terrain_from_biomes(
        &mut self,
        target: native_persistence::world::ChunkPos,
        evaluator: &mut native_resources::density::DensityEvaluator,
        neighborhood: &native_session::worldgen::BiomeNeighborhood,
        structure: Option<Arc<native_core::beardifier::Beardifier>>,
    ) -> Result<native_resources::surface_pass::SurfaceStageChunk> {
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
        let structure = structure.unwrap_or_else(|| {
            Arc::new(native_core::beardifier::Beardifier::new(
                vec![],
                vec![],
                None,
            ))
        });
        self.generator
            .generate_carved_surface_with_structure_context(
                &mut self.resources,
                evaluator,
                &mut assets.carvers,
                [target.x, target.z],
                neighborhood,
                &assets.tags,
                structure,
                classify,
            )
    }

    /// Restore every terrain chunk present in the capture. Missing chunks stay
    /// absent and can be checked against the same live-session guard as the
    /// restored neighbors. Earlier protochunks retain their raw materialized
    /// sections, but do not become proof that later generation stages ran.
    pub fn restore_captured_terrain(&mut self) -> Result<RestoredSavedTerrain> {
        let sources = self.context.inputs().sources();
        let positions: Vec<_> = sources
            .chunks()
            .iter()
            .filter_map(|(position, source)| source.terrain.as_ref().map(|_| *position))
            .collect();
        if positions.is_empty() {
            let dimension = sources.dimension();
            return Ok(RestoredSavedTerrain {
                identity: Arc::clone(&self.identity),
                world: Arc::new(native_world::loaded::World::new(format!(
                    "{}:{}",
                    dimension.namespace, dimension.path
                ))),
                sources: BTreeMap::new(),
                biomes: BTreeMap::new(),
                heightmaps: BTreeMap::new(),
                postprocessing: BTreeMap::new(),
            });
        }
        self.restore_saved_terrain_inner(positions, true)
    }

    /// Restore captured terrain and recorded generation metadata without
    /// replacing unknown blocks, priming heightmaps or consuming marks.
    pub fn restore_saved_terrain(
        &mut self,
        positions: impl IntoIterator<Item = native_persistence::world::ChunkPos>,
    ) -> Result<RestoredSavedTerrain> {
        self.restore_saved_terrain_inner(positions, false)
    }

    fn restore_saved_terrain_inner(
        &mut self,
        positions: impl IntoIterator<Item = native_persistence::world::ChunkPos>,
        allow_preterrain: bool,
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
        let mut biome_positions = Vec::new();
        for &position in &requested {
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
            let stage = GenerationStage::from_id(&status).context("unknown saved chunk status")?;
            ensure!(
                allow_preterrain || stage >= GenerationStage::Terrain,
                "saved chunk has not generated terrain"
            );
            if stage >= GenerationStage::Biomes {
                biome_positions.push(position);
            }
        }
        let biomes = if biome_positions.is_empty() {
            BTreeMap::new()
        } else {
            self.restore_saved_biomes(biome_positions)?
        };
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
        let mut restored_sources = BTreeMap::new();
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
            restored_sources.insert(position, Arc::clone(&source.document));
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
            world: Arc::new(world),
            sources: restored_sources,
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
        self.capture_live_terrain_inner(session, restored, false)
    }

    /// Prepare a source-bound block and biome snapshot while shape and light
    /// work waits. This remains an intermediate input, not a save guard.
    pub fn capture_live_terrain_stage(
        &self,
        session: &native_session::loaded::LoadedSession,
        restored: &RestoredSavedTerrain,
        schedule: &GenerationSchedule,
    ) -> Result<LiveGenerationTerrainStage> {
        self.capture_live_terrain_stage_draft(session, restored, schedule)?
            .validate_checked(|| Ok(()))
    }

    pub fn capture_live_terrain_stage_draft(
        &self,
        session: &native_session::loaded::LoadedSession,
        restored: &RestoredSavedTerrain,
        schedule: &GenerationSchedule,
    ) -> Result<LiveGenerationTerrainStageDraft> {
        self.validate_restored_capture(restored)?;
        use native_session::worldgen::schedule::GenerationStage;
        let positions: Vec<_> = schedule
            .requirements()
            .filter_map(|(pos, required)| (required >= GenerationStage::Biomes).then_some(pos))
            .collect();
        let captured = self.context.inputs().sources().chunks();
        for pos in &positions {
            ensure!(
                captured.contains_key(&native_persistence::world::ChunkPos { x: pos.x, z: pos.z }),
                "terrain stage includes an uncaptured chunk"
            );
        }
        Ok(LiveGenerationTerrainStageDraft {
            identity: Arc::clone(&self.identity),
            terrain: session
                .capture_generation_terrain_stage_draft(Arc::clone(&restored.world), positions)?,
        })
    }

    pub fn verify_live_terrain_stage(
        &self,
        session: &native_session::loaded::LoadedSession,
        stage: &LiveGenerationTerrainStage,
    ) -> Result<()> {
        ensure!(
            Arc::ptr_eq(&self.identity, &stage.identity),
            "terrain belongs to another generation capture"
        );
        session.verify_generation_terrain_stage(&stage.terrain)
    }

    fn validate_restored_capture(&self, restored: &RestoredSavedTerrain) -> Result<()> {
        ensure!(
            Arc::ptr_eq(&self.identity, &restored.identity),
            "restored terrain belongs to another capture"
        );
        ensure!(
            restored.sources.len()
                == self
                    .context
                    .inputs()
                    .sources()
                    .chunks()
                    .values()
                    .filter(|source| source.terrain.is_some())
                    .count(),
            "restored terrain source set differs"
        );
        for (pos, source) in self.context.inputs().sources().chunks() {
            let key = native_world::loaded::ChunkPos { x: pos.x, z: pos.z };
            ensure!(
                source.terrain.is_some() == restored.world.chunks.contains_key(&key),
                "captured terrain was not fully restored at {key:?}"
            );
            ensure!(
                match (source.terrain.as_ref(), restored.sources.get(pos)) {
                    (Some(expected), Some(actual)) => Arc::ptr_eq(&expected.document, actual),
                    (None, None) => true,
                    _ => false,
                },
                "restored terrain source differs at {key:?}"
            );
        }
        Ok(())
    }

    fn capture_live_terrain_inner(
        &self,
        session: &native_session::loaded::LoadedSession,
        restored: &RestoredSavedTerrain,
        deferred_shapes: bool,
    ) -> Result<native_session::loaded::GenerationTerrainGuard> {
        self.validate_restored_capture(restored)?;
        let sources = self.context.inputs().sources().chunks();
        let positions = sources
            .keys()
            .map(|pos| native_world::loaded::ChunkPos { x: pos.x, z: pos.z });
        if deferred_shapes {
            session.capture_generation_terrain_with_deferred_shapes(&restored.world, positions)
        } else {
            session.capture_generation_terrain(&restored.world, positions)
        }
    }

    /// Bind the POI sidecars from the same disk capture to the live session.
    /// Terrain, entities, block-entity internals, clocks, packs and disk
    /// dependencies still require independent validation.
    pub fn capture_live_poi(
        &self,
        session: &native_session::loaded::LoadedSession,
    ) -> Result<native_session::loaded::GenerationPoiGuard> {
        self.capture_live_poi_inner(session, false)
    }

    fn capture_live_poi_inner(
        &self,
        session: &native_session::loaded::LoadedSession,
        deferred_shapes: bool,
    ) -> Result<native_session::loaded::GenerationPoiGuard> {
        let source = self
            .context
            .inputs()
            .sources()
            .chunks()
            .iter()
            .map(|(pos, chunk)| {
                (
                    native_world::loaded::ChunkPos { x: pos.x, z: pos.z },
                    chunk.poi.as_ref().map(|poi| Arc::clone(&poi.document)),
                )
            })
            .collect();
        if deferred_shapes {
            session.capture_generation_poi_with_deferred_shapes(&source)
        } else {
            session.capture_generation_poi(&source)
        }
    }

    /// Capture live dependencies without allowing a guard from a different
    /// disk capture to be reused for this generator.
    pub fn capture_live_inputs(
        &self,
        session: &native_session::loaded::LoadedSession,
        restored: &RestoredSavedTerrain,
    ) -> Result<LiveGenerationInputs> {
        Ok(LiveGenerationInputs {
            identity: Arc::clone(&self.identity),
            terrain: self.capture_live_terrain(session, restored)?,
            poi: self.capture_live_poi(session)?,
            entities: self.capture_live_entities(session)?,
        })
    }

    /// Allow blocked shape callbacks to remain queued while their missing
    /// chunks are prepared. The caller must replay them after publication and
    /// still validate every other live and persistence dependency.
    pub fn capture_live_inputs_with_deferred_shapes(
        &self,
        session: &native_session::loaded::LoadedSession,
        restored: &RestoredSavedTerrain,
    ) -> Result<LiveGenerationInputs> {
        Ok(LiveGenerationInputs {
            identity: Arc::clone(&self.identity),
            terrain: self.capture_live_terrain_inner(session, restored, true)?,
            poi: self.capture_live_poi_inner(session, true)?,
            entities: self.capture_live_entities(session)?,
        })
    }

    /// Compare entity sidecars and tracked live entities with the same disk
    /// capture. Other entity types and effects still require validation.
    pub fn capture_live_entities(
        &self,
        session: &native_session::loaded::LoadedSession,
    ) -> Result<native_session::loaded::GenerationEntityGuard> {
        let sources = self
            .context
            .inputs()
            .sources()
            .chunks()
            .iter()
            .map(|(pos, chunk)| {
                (
                    native_world::loaded::ChunkPos { x: pos.x, z: pos.z },
                    chunk
                        .entities
                        .as_ref()
                        .map(|entity| Arc::clone(&entity.document)),
                )
            })
            .collect();
        session.capture_generation_entities(&sources)
    }

    /// Verify while retaining exclusive access to the session until the
    /// corresponding generation result is committed or discarded.
    pub fn verify_live_inputs(
        &self,
        session: &native_session::loaded::LoadedSession,
        inputs: &LiveGenerationInputs,
    ) -> Result<()> {
        ensure!(
            Arc::ptr_eq(&self.identity, &inputs.identity),
            "live generation inputs belong to another capture"
        );
        session.verify_generation_terrain(&inputs.terrain)?;
        session.verify_generation_poi(&inputs.poi)?;
        session.verify_generation_entities(&inputs.entities)
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

fn assemble_captured_mixed_full_chunk(
    position: native_world::loaded::ChunkPos,
    min_y: i32,
    height: i32,
    game_time: i64,
    source: Option<&NamedTag>,
    structures: Option<&Tag>,
    parts: &native_session::worldgen::PostprocessedChunkComponents,
) -> Result<(NamedTag, NamedTag)> {
    let entities = parts
        .entity_records
        .as_ref()
        .context("captured entity records are incomplete")?;
    let entity_chunk =
        native_persistence::generated_chunk::assemble_entity_chunk(position, entities)?;
    let Tag::Compound(heightmaps) = &parts.heightmaps else {
        bail!("postprocessed heightmaps are not a compound")
    };
    let full_maps = Tag::Compound(
        heightmaps
            .iter()
            .filter(|(name, _)| {
                [
                    "WORLD_SURFACE",
                    "OCEAN_FLOOR",
                    "MOTION_BLOCKING",
                    "MOTION_BLOCKING_NO_LEAVES",
                ]
                .iter()
                .any(|expected| name.matches(expected))
            })
            .cloned()
            .collect(),
    );
    let mut canonical = native_persistence::generated_chunk::assemble_full_chunk(
        native_persistence::generated_chunk::FullChunkComponents {
            position,
            min_y,
            height,
            last_update: game_time,
            sections: parts.sections.clone(),
            heightmaps: full_maps,
            block_ticks: parts.block_ticks.clone(),
            fluid_ticks: parts.fluid_ticks.clone(),
            block_entities: parts.block_entities.clone(),
        },
    )?;
    replace_unique_root_field(&mut canonical, "Heightmaps", parts.heightmaps.clone())?;
    if let Some(structures) = structures {
        ensure!(
            matches!(structures, Tag::Compound(_)),
            "invalid captured structures"
        );
        replace_unique_root_field(&mut canonical, "structures", structures.clone())?;
    }
    let Some(source) = source else {
        native_persistence::nbt::write(&canonical)?;
        return Ok((canonical, entity_chunk));
    };
    ensure!(
        unique_root_field(source, "xPos")? == Some(&Tag::Int(position.x))
            && unique_root_field(source, "zPos")? == Some(&Tag::Int(position.z)),
        "captured source position differs"
    );
    if let Some(inhabited) = unique_root_field(source, "InhabitedTime")? {
        ensure!(
            matches!(inhabited, Tag::Long(_)),
            "invalid captured inhabited time"
        );
    }
    unique_root_field(source, "entities")?;
    unique_root_field(source, "UpgradeData")?;
    let mut document = source.clone();
    let Tag::Compound(fields) = &mut document.value else {
        unreachable!()
    };
    // Proto entities moved to the separate entity chunk. Validated upgrade
    // callbacks have completed, so retaining their pending ticks would replay them.
    fields.retain(|(name, _)| !name.matches("entities") && !name.matches("UpgradeData"));
    let Tag::Compound(fields) = canonical.value else {
        unreachable!()
    };
    for (name, value) in fields {
        let name = String::from_utf16(&name.0).context("invalid generated field name")?;
        if name == "InhabitedTime" && unique_root_field(&document, &name)?.is_some() {
            continue;
        }
        replace_unique_root_field(&mut document, &name, value)?;
    }
    native_persistence::nbt::write(&document)?;
    Ok((document, entity_chunk))
}

fn original_structure_references(document: &NamedTag) -> Result<Option<&Tag>> {
    let Some(structures) = unique_root_field(document, "structures")? else {
        return Ok(None);
    };
    let Tag::Compound(fields) = structures else {
        bail!("saved structures are not a compound");
    };
    let mut entries = fields.iter().filter(|(name, _)| name.matches("References"));
    let first = entries.next().map(|(_, value)| value);
    ensure!(entries.next().is_none(), "duplicate structure references");
    Ok(first)
}

/// Retain opaque structure fields and starts while adding references produced
/// from this exact captured source.
fn merge_generated_structure_references(
    document: &mut NamedTag,
    references: &CapturedGeneratedReferences,
) -> Result<()> {
    let current = original_structure_references(document)?;
    ensure!(
        current == references.original.as_ref()
            || (references.original.is_none()
                && matches!(current, Some(Tag::Compound(fields)) if fields.is_empty())),
        "saved structure references differ from the generation capture"
    );
    let mut structures = match unique_root_field(document, "structures")? {
        Some(Tag::Compound(fields)) => fields.clone(),
        Some(_) => bail!("saved structures are not a compound"),
        None => Vec::new(),
    };
    let starts: Vec<_> = structures
        .iter()
        .filter(|(name, _)| name.matches("starts"))
        .collect();
    ensure!(starts.len() <= 1, "duplicate structure starts");
    if let Some((_, value)) = starts.first() {
        ensure!(
            matches!(value, Tag::Compound(_)),
            "saved structure starts are not a compound"
        );
    } else {
        structures.push(("starts".into(), Tag::Compound(vec![])));
    }
    if let Some((_, value)) = structures
        .iter_mut()
        .find(|(name, _)| name.matches("References"))
    {
        *value = references.references.to_tag();
    } else {
        structures.push(("References".into(), references.references.to_tag()));
    }
    replace_unique_root_field(document, "structures", Tag::Compound(structures))
}

/// Read neighbor ticks while rejecting unimplemented block and side fixups.
fn parse_captured_upgrade_data(
    document: &NamedTag,
    height: i32,
    block_ids: &BTreeSet<String>,
) -> Result<native_session::worldgen::postprocessing::UpgradeTicks> {
    use native_session::worldgen::postprocessing::UpgradeTicks;
    let mut result = UpgradeTicks::default();
    let Some(data) = unique_root_field(document, "UpgradeData")? else {
        return Ok(result);
    };
    let Tag::Compound(fields) = data else {
        bail!("saved upgrade data is not a compound")
    };
    ensure!(height > 0 && height % 16 == 0, "invalid upgrade height");
    let mut names = BTreeSet::new();
    for (name, value) in fields {
        ensure!(
            names.insert(name.0.clone()),
            "duplicate saved upgrade data field"
        );
        if name.matches("Sides") {
            ensure!(
                value.as_java_int() == Some(0),
                "saved upgrade sides require callbacks"
            );
        } else if name.matches("Indices") {
            let Tag::Compound(indices) = value else {
                bail!("saved upgrade indices are not a compound")
            };
            let mut seen = BTreeSet::new();
            for (key, value) in indices {
                ensure!(
                    seen.insert(key.0.clone()),
                    "duplicate saved upgrade section"
                );
                let key = String::from_utf16(&key.0).context("invalid upgrade section index")?;
                let index: i32 = key.parse().context("invalid upgrade section index")?;
                ensure!(
                    index >= 0 && index < height / 16 && index.to_string() == key,
                    "upgrade section is outside the dimension"
                );
                ensure!(
                    matches!(value, Tag::IntArray(indices) if indices.is_empty()),
                    "saved upgrade indices require callbacks"
                );
            }
        } else if name.matches("neighbor_block_ticks") {
            result.blocks = parse_captured_upgrade_ticks(value, |id| block_ids.contains(id))?;
        } else if name.matches("neighbor_fluid_ticks") {
            result.fluids = parse_captured_upgrade_ticks(value, |id| {
                matches!(
                    id,
                    "minecraft:empty"
                        | "minecraft:water"
                        | "minecraft:flowing_water"
                        | "minecraft:lava"
                        | "minecraft:flowing_lava"
                )
            })?;
        } else {
            bail!("unrecognized saved upgrade data field");
        }
    }
    Ok(result)
}

fn parse_captured_upgrade_ticks(
    value: &Tag,
    known: impl Fn(&str) -> bool,
) -> Result<Vec<native_session::worldgen::chunk_ticks::SavedTick>> {
    use native_persistence::nbt::TagType;
    use native_session::worldgen::chunk_ticks::SavedTick;
    let Tag::List {
        element_type,
        values,
    } = value
    else {
        bail!("saved upgrade ticks are not a list")
    };
    ensure!(
        *element_type == TagType::Compound || (*element_type == TagType::End && values.is_empty()),
        "invalid saved upgrade tick list"
    );
    values
        .iter()
        .map(|value| {
            let Tag::Compound(fields) = value else {
                bail!("saved upgrade tick is not a compound")
            };
            let mut seen = BTreeSet::new();
            for (name, _) in fields {
                ensure!(
                    seen.insert(name.0.clone()),
                    "duplicate saved upgrade tick field"
                );
                ensure!(
                    ["i", "x", "y", "z", "t", "p"]
                        .iter()
                        .any(|expected| name.matches(expected)),
                    "unrecognized saved upgrade tick field"
                );
            }
            let number = |key: &str| {
                value
                    .get(key)
                    .and_then(Tag::as_java_int)
                    .with_context(|| format!("invalid upgrade tick {key}"))
            };
            let Some(Tag::String(kind)) = value.get("i") else {
                bail!("invalid upgrade tick type")
            };
            let kind = String::from_utf16(&kind.0).context("invalid upgrade tick type")?;
            let id = ResourceId::parse(&kind)?;
            let kind = format!("{}:{}", id.namespace, id.path);
            ensure!(known(&kind), "unknown saved upgrade tick type {kind}");
            let priority = number("p")?;
            ensure!(
                (-3..=3).contains(&priority),
                "invalid upgrade tick priority"
            );
            Ok(SavedTick {
                kind,
                position: [number("x")?, number("y")?, number("z")?],
                delay: number("t")?,
                priority,
            })
        })
        .collect()
}

fn merge_captured_postprocessed_heightmaps(
    original: &[(native_persistence::nbt::JavaString, Tag)],
    generated: Tag,
) -> Result<Tag> {
    let Tag::Compound(generated) = generated else {
        bail!("invalid postprocessed heightmaps")
    };
    let mut names = BTreeSet::new();
    for (name, _) in original.iter().chain(&generated) {
        ensure!(
            names.insert(name.0.clone()),
            "duplicate postprocessed heightmap"
        );
    }
    let mut maps = original.to_vec();
    maps.extend(generated);
    Ok(Tag::Compound(maps))
}

fn merge_captured_postprocessed_sections(
    source: Option<&NamedTag>,
    generated: &Tag,
) -> Result<Tag> {
    use native_session::worldgen::schedule::GenerationStage;
    let Some(source) = source else {
        return Ok(generated.clone());
    };
    let status = match unique_root_field(source, "Status")? {
        Some(Tag::String(status)) => String::from_utf16(&status.0)?,
        _ => bail!("saved chunk status is missing or invalid"),
    };
    let stage = GenerationStage::from_id(&status).context("unknown saved chunk status")?;
    ensure!(
        stage <= GenerationStage::Biomes,
        "saved chunk is past the supported biome stage"
    );
    let original = unique_root_field(source, "sections")?;
    if stage < GenerationStage::Biomes {
        native_persistence::generated_section::merge_prebiome_stage_sections(original, generated)
    } else {
        native_persistence::generated_section::merge_biome_stage_blocks(
            original.context("saved sections missing")?,
            generated,
        )
    }
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
    validate_empty_preterrain_target(saved, false).map(|_| ())
}

fn validate_empty_preterrain_target(
    saved: &native_persistence::world::LoadedChunk,
    allow_pre_biomes: bool,
) -> Result<native_session::worldgen::schedule::GenerationStage> {
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
    let stage = GenerationStage::from_id(&status).context("unknown saved chunk status")?;
    ensure!(
        stage == GenerationStage::Biomes || (allow_pre_biomes && stage < GenerationStage::Biomes),
        "saved chunk must be at or before the biomes stage"
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
    Ok(stage)
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
    // Saved starts can affect terrain even when the world no longer creates
    // new structures, so every existing-world generator must accept them.
    let generator = TerrainGenerator::with_structure_context(
        &mut resources,
        &definition,
        seed,
        lighting.min_y,
        lighting.height,
    )?;
    packs.verify()?;
    Ok(PreparedExistingGenerator {
        generator,
        generator_definition: definition,
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
        structure_registry: None,
        captured_structure_starts: BTreeMap::new(),
        creature_environment: None,
        mob_placement_assets: None,
        farm_variants: [None, None, None],
        farm_moon: None,
        captured_entity_ids: None,
        generated_entity_ids: None,
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
#[path = "existing_saved_light_test.rs"]
mod saved_light_test;

#[cfg(test)]
mod tests {
    use super::*;
    use native_persistence::nbt::{JavaString, TagType};
    use serde_json::json;

    #[test]
    fn saved_mineshaft_filter_uses_inclusive_chunk_bounds() -> Result<()> {
        use native_core::beardifier::BoundingBox;
        use native_resources::mineshaft::{
            MineshaftGraphPiece, MineshaftGraphPieceKind, MineshaftVariant,
        };
        use native_session::worldgen::{
            SavedMineshaftPiece, saved_structure_starts::RestoredStructurePiece,
        };
        use native_world::loaded::ChunkPos;

        let piece = |min, max| -> Result<RestoredStructurePiece> {
            Ok(RestoredStructurePiece::Mineshaft(SavedMineshaftPiece {
                piece: MineshaftGraphPiece {
                    bounds: BoundingBox::new(min, max).map_err(anyhow::Error::msg)?,
                    direction: None,
                    depth: 0,
                    kind: MineshaftGraphPieceKind::Room { entrances: vec![] },
                },
                variant: MineshaftVariant::Normal,
                placed_spider: false,
            }))
        };
        let target = ChunkPos { x: -1, z: 2 };
        let outside = piece([0, 0, 32], [7, 7, 39])?;
        let touching = piece([-1, 1000, 47], [6, 1007, 54])?;
        assert!(!mineshaft_may_touch_chunk(target, &[outside])?);
        assert!(mineshaft_may_touch_chunk(target, &[touching])?);
        let wrong =
            RestoredStructurePiece::BuriedTreasure(BoundingBox::new([0, 0, 0], [1, 1, 1]).unwrap());
        assert!(mineshaft_may_touch_chunk(target, &[wrong]).is_err());
        assert!(mineshaft_may_touch_chunk(ChunkPos { x: i32::MAX, z: 0 }, &[]).is_err());
        Ok(())
    }

    #[test]
    fn saved_mineshaft_plan_keeps_source_and_commits_only_valid_updates() -> Result<()> {
        use native_core::beardifier::{BoundingBox, TerrainAdjustment};
        use native_resources::{
            mineshaft::{MineshaftGraphPiece, MineshaftGraphPieceKind, MineshaftVariant},
            structure_start::{HorizontalDirection, SimpleStructureKind, SimpleStructureStart},
        };
        use native_session::worldgen::{
            PreparedStructureStart, SavedMineshaftPiece,
            saved_structure_starts::RestoredStructurePiece, structure_batch::PlannedStructureStart,
        };
        use native_world::loaded::ChunkPos;

        struct Data;
        impl ResourceProvider for Data {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(Some(br#"{"step":"underground_structures"}"#.to_vec()))
            }
            fn list_data(&mut self, _: &str) -> Result<Vec<ResourceId>> {
                Ok(vec![ResourceId::parse("example:mine")?])
            }
        }

        let id = ResourceId::parse("example:mine")?;
        let pos = ChunkPos { x: 0, z: 0 };
        let key = (id.clone(), pos);
        let source = Arc::new(NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![("unrecognized".into(), Tag::Int(7))]),
        });
        let original = Tag::Compound(vec![("saved".into(), Tag::Int(17))]);
        let piece = SavedMineshaftPiece::capture(
            &MineshaftGraphPiece {
                bounds: BoundingBox::new([0, 4, 0], [2, 6, 9]).unwrap(),
                direction: Some(HorizontalDirection::North),
                depth: 2,
                kind: MineshaftGraphPieceKind::Corridor {
                    rails: false,
                    spiders: true,
                    sections: 2,
                },
            },
            MineshaftVariant::Normal,
            false,
        )?;
        let saved_piece = RestoredStructurePiece::Mineshaft(piece.clone());
        let resumed = resume_saved_mineshaft(
            Arc::clone(&source),
            &original,
            3,
            pos,
            TerrainAdjustment::None,
            std::slice::from_ref(&saved_piece),
        )?;
        let placement = resumed.placement.clone();
        let mut different = piece;
        different.variant = MineshaftVariant::Mesa;
        assert!(
            resume_saved_mineshaft(
                Arc::clone(&source),
                &original,
                3,
                pos,
                TerrainAdjustment::None,
                &[saved_piece, RestoredStructurePiece::Mineshaft(different)],
            )
            .is_err()
        );
        let mut plan = SavedStructurePlan {
            schedule: Some(Arc::new(
                native_resources::structure_decoration::StructureDecorationSchedule::load(
                    &mut Data,
                )?,
            )),
            starts: BTreeMap::new(),
            mineshafts: BTreeMap::from([(key.clone(), resumed)]),
            order: vec![(id, vec![key.clone()])],
        };
        let mixed = plan.planned_mixed()?;
        assert!(matches!(
            mixed[0].starts[0],
            PlannedStructureStart::Mineshaft(_)
        ));
        let wrong = SimpleStructureStart {
            kind: SimpleStructureKind::BuriedTreasure,
            chunk: [0, 0],
            generation_point: [0, 0, 0],
            orientation: None,
            bounds: BoundingBox::new([0, 0, 0], [0, 0, 0]).unwrap(),
            terrain_adjustment: TerrainAdjustment::None,
        };
        assert!(
            plan.apply_mixed_updates(vec![vec![PreparedStructureStart::Simple(wrong)]])
                .is_err()
        );
        assert!(!plan.mineshafts[&key].placement.pieces[0].placed_spider);
        let mut updated = placement;
        updated.pieces[0].placed_spider = true;
        plan.apply_mixed_updates(vec![vec![PreparedStructureStart::Mineshaft(updated)]])?;
        let actual = &plan.mineshafts[&key];
        assert!(actual.placement.pieces[0].placed_spider);
        assert!(Arc::ptr_eq(&actual.source, &source));
        assert_eq!(actual.original, original);
        assert_eq!(actual.references, 3);
        Ok(())
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn saved_mineshaft_filter_matches_java_structure_placement() -> Result<()> {
        use native_core::beardifier::BoundingBox;
        use native_resources::mineshaft::{
            MineshaftGraphPiece, MineshaftGraphPieceKind, MineshaftVariant,
        };
        use native_session::worldgen::{
            SavedMineshaftPiece, saved_structure_starts::RestoredStructurePiece,
        };
        use native_world::loaded::ChunkPos;
        use std::process::Command;

        let cases = [
            ([0, 0, 32], [7, 7, 39]),
            ([-1, 0, 47], [6, 7, 54]),
            ([-1, 1000, 47], [6, 1007, 54]),
            ([-17, 0, 35], [-16, 7, 42]),
            ([-18, 0, 35], [-17, 7, 42]),
        ];
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"]
            .as_array()
            .context("libraries missing")?
        {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let classpath = std::env::join_paths(paths)?;
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join(
                    "AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon",
                )
            })
            .join("bin");
        let source = temp.path().join("SavedMineshaftNoopOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/SavedMineshaftNoopOracle.java"),
        )?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&classpath)
            .arg(&source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java oracle compilation failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&classpath)
            .arg("SavedMineshaftNoopOracle")
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let output = std::str::from_utf8(&output.stdout)?;
        let mut results = BTreeMap::new();
        for line in output.lines() {
            let Some((_, row)) = line.split_once("P ") else {
                continue;
            };
            let mut fields = row.split_whitespace();
            let index: usize = fields.next().context("case index missing")?.parse()?;
            let calls: usize = fields.next().context("call count missing")?.parse()?;
            let unchanged: bool = fields.next().context("random state missing")?.parse()?;
            ensure!(fields.next().is_none(), "extra oracle fields");
            ensure!(
                results.insert(index, (calls, unchanged)).is_none(),
                "duplicate case"
            );
        }
        assert_eq!(results.len(), cases.len());
        for (index, (min, max)) in cases.into_iter().enumerate() {
            let piece = RestoredStructurePiece::Mineshaft(SavedMineshaftPiece {
                piece: MineshaftGraphPiece {
                    bounds: BoundingBox::new(min, max).map_err(anyhow::Error::msg)?,
                    direction: None,
                    depth: 0,
                    kind: MineshaftGraphPieceKind::Room { entrances: vec![] },
                },
                variant: MineshaftVariant::Normal,
                placed_spider: false,
            });
            let may_touch = mineshaft_may_touch_chunk(ChunkPos { x: -1, z: 2 }, &[piece])?;
            let (calls, unchanged) = results[&index];
            assert_eq!(
                (calls, unchanged),
                [(0, true), (1, false), (0, true), (1, false), (0, true)][index]
            );
            assert!(may_touch || (calls == 0 && unchanged));
        }
        Ok(())
    }

    #[test]
    fn every_supported_creature_can_pass_the_construction_gate() -> Result<()> {
        use native_resources::{
            density::WorldgenRandom,
            mob_spawns::{CreatureSpawnLayer, CreatureSpawns},
            worldgen_mob_geometry::CreatureSpawnGeometry,
            worldgen_mob_spawner::{ChunkCreatureSpawner, CreatureSpawnProgress},
            worldgen_region_random::RegionRandom,
        };
        let border = native_session::respawn::RespawnBorder {
            min_x: -100.0,
            max_x: 100.0,
            min_z: -100.0,
            max_z: 100.0,
        };
        for species in [
            "sheep", "horse", "donkey", "pig", "cow", "chicken", "turtle",
        ] {
            let id = ResourceId::parse(&format!("minecraft:{species}"))?;
            assert!(supports_generated_creature(&id));
            let construction =
                native_session::worldgen::FreshCreatureRandom::from_seed(&id, 42)?.construction;
            let attributes = json!({"gameplay/natural_mob_spawns":{
                "spawns_by_category":{"creature":[{"type":format!("minecraft:{species}"),"weight":1,"count":1}]},
                "spawn_costs":{}
            }});
            let mut spawns = CreatureSpawns::default();
            spawns
                .apply(CreatureSpawnLayer::from_attributes(&attributes, |_| Some(false))?.unwrap());
            let mut planner = ChunkCreatureSpawner::new(42, 0, 0, true, 0.999_999_9, spawns)?;
            let attempt = loop {
                match planner.next(1)? {
                    CreatureSpawnProgress::Pending => continue,
                    CreatureSpawnProgress::Attempt(attempt) => break attempt,
                    CreatureSpawnProgress::Finished => {
                        anyhow::bail!("fixture produced no creature")
                    }
                }
            };
            assert_eq!(attempt.entity, id);
            let mut plan = ResumedCreaturePlan {
                identity: Arc::new(()),
                revision: Arc::new(()),
                light_revision: Arc::new(()),
                position: native_world::loaded::ChunkPos { x: 0, z: 0 },
                clocks: BTreeMap::new(),
                planner,
                finished: false,
                probe: None,
                construction: Some(CreatureSpawnGeometry {
                    position: [0.5, 1.0, 0.5],
                    min: [0.0, 1.0, 0.0],
                    max: [1.0, 2.0, 1.0],
                }),
                border: Some(border),
                rule_light: None,
                random: RegionRandom::new(42, WorldgenRandom::Legacy, 0, 0),
                group_id: None,
                sheep_group: Default::default(),
                horse_group: Default::default(),
                donkey_group: Default::default(),
                constructed: None,
                pose: None,
                instance_passed: false,
                finalizable: false,
                additions: Vec::new(),
                used_uuids: BTreeSet::new(),
            };
            PreparedExistingGenerator::validate_creature_construction(
                &mut plan,
                border,
                construction,
            )?;
            plan.used_uuids.insert(construction.uuid);
            assert!(
                PreparedExistingGenerator::validate_creature_construction(
                    &mut plan,
                    border,
                    construction
                )
                .is_err()
            );
        }
        assert!(!supports_generated_creature(&ResourceId::parse(
            "example:turtle"
        )?));
        assert!(!supports_generated_creature(&ResourceId::parse(
            "minecraft:unknown"
        )?));
        Ok(())
    }

    #[test]
    fn generated_structure_references_preserve_saved_structure_fields() -> Result<()> {
        use native_session::worldgen::structure_references::{
            StructureStartBounds, collect_ordered_structure_references,
        };
        let target = native_world::loaded::ChunkPos { x: 0, z: 0 };
        let id = ResourceId::parse("minecraft:buried_treasure")?;
        let bounds = native_core::beardifier::BoundingBox::new([0, 0, 0], [15, 15, 15]).unwrap();
        let refs = collect_ordered_structure_references(target, |source| {
            Ok(if source == target {
                vec![StructureStartBounds {
                    structure: id.clone(),
                    bounds,
                }]
            } else {
                vec![]
            })
        })?;
        let refs = CapturedGeneratedReferences {
            original: None,
            references: refs,
        };
        let starts = Tag::Compound(vec![("custom:start".into(), Tag::Int(9))]);
        let mut source = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("custom_root".into(), Tag::Int(17)),
                (
                    "structures".into(),
                    Tag::Compound(vec![
                        ("starts".into(), starts.clone()),
                        ("custom".into(), Tag::String("keep".into())),
                    ]),
                ),
            ]),
        };
        merge_generated_structure_references(&mut source, &refs)?;
        let structures = source.value.get("structures").unwrap();
        assert_eq!(structures.get("starts"), Some(&starts));
        assert_eq!(structures.get("custom"), Some(&Tag::String("keep".into())));
        assert_eq!(
            structures
                .get("References")
                .and_then(|tag| tag.get("minecraft:buried_treasure")),
            Some(&Tag::LongArray(vec![0]))
        );
        assert_eq!(source.value.get("custom_root"), Some(&Tag::Int(17)));
        let unchanged = source.clone();
        assert!(merge_generated_structure_references(&mut source, &refs).is_err());
        assert_eq!(source, unchanged);
        let mut malformed = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![("structures".into(), Tag::Int(1))]),
        };
        let unchanged = malformed.clone();
        assert!(merge_generated_structure_references(&mut malformed, &refs).is_err());
        assert_eq!(malformed, unchanged);
        Ok(())
    }

    #[test]
    fn captured_postprocessing_rejects_active_or_ambiguous_upgrade_data() -> Result<()> {
        let document = |upgrade: Option<Tag>| NamedTag {
            name: "".into(),
            value: Tag::Compound(
                upgrade
                    .into_iter()
                    .map(|value| ("UpgradeData".into(), value))
                    .collect(),
            ),
        };
        let list = |values| Tag::List {
            element_type: TagType::Compound,
            values,
        };
        let ids = BTreeSet::from(["minecraft:stone".to_owned()]);
        let parse = |doc: &NamedTag| parse_captured_upgrade_data(doc, 32, &ids);
        assert!(parse(&document(None)).is_ok());
        assert!(parse(&document(Some(Tag::Compound(vec![])))).is_ok());
        let harmless = Tag::Compound(vec![
            ("Sides".into(), Tag::Byte(0)),
            (
                "Indices".into(),
                Tag::Compound(vec![("0".into(), Tag::IntArray(vec![]))]),
            ),
            ("neighbor_block_ticks".into(), list(vec![])),
            (
                "neighbor_fluid_ticks".into(),
                Tag::List {
                    element_type: TagType::End,
                    values: vec![],
                },
            ),
        ]);
        assert!(parse(&document(Some(harmless))).is_ok());
        for value in [
            Tag::Compound(vec![("Sides".into(), Tag::Int(1))]),
            Tag::Compound(vec![("Sides".into(), Tag::String("0".into()))]),
            Tag::Compound(vec![(
                "Indices".into(),
                Tag::Compound(vec![("0".into(), Tag::IntArray(vec![1]))]),
            )]),
            Tag::Compound(vec![(
                "Indices".into(),
                Tag::Compound(vec![("2".into(), Tag::IntArray(vec![]))]),
            )]),
            Tag::Compound(vec![(
                "neighbor_block_ticks".into(),
                list(vec![Tag::Compound(vec![])]),
            )]),
            Tag::Compound(vec![(
                "neighbor_fluid_ticks".into(),
                list(vec![Tag::Compound(vec![])]),
            )]),
            Tag::Compound(vec![("example:unknown".into(), Tag::Int(0))]),
            Tag::Compound(vec![
                ("Sides".into(), Tag::Int(0)),
                ("Sides".into(), Tag::Int(0)),
            ]),
            Tag::Int(0),
        ] {
            assert!(parse(&document(Some(value))).is_err());
        }
        let duplicate = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("UpgradeData".into(), Tag::Compound(vec![])),
                ("UpgradeData".into(), Tag::Compound(vec![])),
            ]),
        };
        assert!(parse(&duplicate).is_err());
        let tick = |kind: &str| {
            Tag::Compound(vec![
                ("i".into(), Tag::String(kind.into())),
                ("x".into(), Tag::Int(16)),
                ("y".into(), Tag::Int(5)),
                ("z".into(), Tag::Int(-1)),
                ("t".into(), Tag::Int(-3)),
                ("p".into(), Tag::Int(2)),
            ])
        };
        let scheduled = document(Some(Tag::Compound(vec![
            ("neighbor_block_ticks".into(), list(vec![tick("stone")])),
            (
                "neighbor_fluid_ticks".into(),
                list(vec![tick("minecraft:empty")]),
            ),
        ])));
        let ticks = parse(&scheduled)?;
        assert_eq!(ticks.blocks.len(), 1);
        assert_eq!(ticks.blocks[0].kind, "minecraft:stone");
        assert_eq!(ticks.blocks[0].position, [16, 5, -1]);
        assert_eq!((ticks.blocks[0].delay, ticks.blocks[0].priority), (-3, 2));
        assert_eq!(ticks.fluids[0].kind, "minecraft:empty");
        let unknown = document(Some(Tag::Compound(vec![(
            "neighbor_block_ticks".into(),
            list(vec![tick("minecraft:unknown")]),
        )])));
        assert!(parse(&unknown).is_err());
        Ok(())
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn captured_upgrade_noop_classification_matches_java() -> Result<()> {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"]
            .as_array()
            .context("libraries missing")?
        {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let classpath = std::env::join_paths(paths)?;
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("CapturedUpgradeDataOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/CapturedUpgradeDataOracle.java"),
        )?;
        let compiled = std::process::Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java oracle compilation failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let result = std::process::Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("CapturedUpgradeDataOracle")
            .output()?;
        ensure!(
            result.status.success(),
            "Java oracle failed: {}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        let output = String::from_utf8(result.stdout)?.replace("\r\n", "\n");
        let java = output
            .split_once("BEGIN_RESULT\n")
            .context("Java result start missing")?
            .1
            .split_once("END_RESULT")
            .context("Java result end missing")?
            .0;
        let index = |values| {
            Tag::Compound(vec![(
                "Indices".into(),
                Tag::Compound(vec![("0".into(), Tag::IntArray(values))]),
            )])
        };
        let ticks = |kind: &str| {
            Tag::Compound(vec![(
                format!("neighbor_{kind}_ticks").as_str().into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![Tag::Compound(vec![
                        (
                            "i".into(),
                            Tag::String(
                                if kind == "block" {
                                    "minecraft:stone"
                                } else {
                                    "minecraft:water"
                                }
                                .into(),
                            ),
                        ),
                        ("x".into(), Tag::Int(8)),
                        ("y".into(), Tag::Int(8)),
                        ("z".into(), Tag::Int(8)),
                        ("t".into(), Tag::Int(2)),
                        ("p".into(), Tag::Int(0)),
                    ])],
                },
            )])
        };
        let cases = [
            ("empty", Tag::Compound(vec![])),
            ("zero", Tag::Compound(vec![("Sides".into(), Tag::Int(0))])),
            ("empty_indices", index(vec![])),
            ("sides", Tag::Compound(vec![("Sides".into(), Tag::Int(1))])),
            ("indices", index(vec![1])),
            ("block_ticks", ticks("block")),
            ("fluid_ticks", ticks("fluid")),
        ];
        let rust = cases
            .into_iter()
            .map(|(name, data)| {
                let document = NamedTag {
                    name: "".into(),
                    value: Tag::Compound(vec![("UpgradeData".into(), data)]),
                };
                format!(
                    "{name} {}",
                    parse_captured_upgrade_data(
                        &document,
                        32,
                        &BTreeSet::from(["minecraft:stone".to_owned()])
                    )
                    .is_ok_and(|ticks| ticks.is_empty())
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(java.trim(), rust);
        Ok(())
    }

    #[test]
    fn captured_postprocessed_heightmaps_keep_unknown_fields() -> Result<()> {
        let original = vec![(
            JavaString::from("example:extra"),
            Tag::LongArray(vec![1, -2]),
        )];
        let generated = Tag::Compound(vec![("WORLD_SURFACE".into(), Tag::LongArray(vec![3]))]);
        let merged = merge_captured_postprocessed_heightmaps(&original, generated.clone())?;
        assert_eq!(merged.get("example:extra"), Some(&original[0].1));
        assert_eq!(merged.get("WORLD_SURFACE"), generated.get("WORLD_SURFACE"));
        assert!(merge_captured_postprocessed_heightmaps(&original, Tag::Int(1)).is_err());
        let duplicate = vec![(JavaString::from("WORLD_SURFACE"), Tag::Int(7))];
        assert!(merge_captured_postprocessed_heightmaps(&duplicate, generated).is_err());
        Ok(())
    }

    #[test]
    fn captured_postprocessed_sections_preserve_saved_metadata() -> Result<()> {
        let section = |block: &str, biome: &str| {
            Tag::Compound(vec![
                ("Y".into(), Tag::Byte(0)),
                (
                    "block_states".into(),
                    Tag::Compound(vec![(
                        "palette".into(),
                        Tag::List {
                            element_type: TagType::String,
                            values: vec![Tag::String(block.into())],
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
                ("example:section_data".into(), Tag::Int(42)),
            ])
        };
        let list = |value| Tag::List {
            element_type: TagType::Compound,
            values: vec![value],
        };
        let source = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("Status".into(), Tag::String("minecraft:biomes".into())),
                (
                    "sections".into(),
                    list(section("minecraft:air", "minecraft:plains")),
                ),
            ]),
        };
        let generated = list(section("minecraft:stone", "minecraft:plains"));
        let merged = merge_captured_postprocessed_sections(Some(&source), &generated)?;
        let Tag::List { values, .. } = merged else {
            unreachable!()
        };
        assert_eq!(values[0].get("example:section_data"), Some(&Tag::Int(42)));
        assert_eq!(
            values[0].get("block_states"),
            match &generated {
                Tag::List { values, .. } => values[0].get("block_states"),
                _ => unreachable!(),
            }
        );
        assert_eq!(
            merge_captured_postprocessed_sections(None, &generated)?,
            generated
        );
        let wrong_biome = list(section("minecraft:stone", "minecraft:desert"));
        assert!(merge_captured_postprocessed_sections(Some(&source), &wrong_biome).is_err());
        let mut unsupported = source.clone();
        replace_unique_root_field(
            &mut unsupported,
            "Status",
            Tag::String("minecraft:noise".into()),
        )?;
        assert!(merge_captured_postprocessed_sections(Some(&unsupported), &generated).is_err());
        let mut ambiguous = source.clone();
        let Tag::Compound(fields) = &mut ambiguous.value else {
            unreachable!()
        };
        fields.push(("Status".into(), Tag::String("minecraft:biomes".into())));
        assert!(merge_captured_postprocessed_sections(Some(&ambiguous), &generated).is_err());
        Ok(())
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn captured_missing_chunks_generate_structures_before_terrain() -> Result<()> {
        use flate2::{Compression, write::GzEncoder};
        use native_persistence::worldgen_settings::{
            NewWorldOptions, draft_baked_worldgen_settings,
        };
        use native_resources::structure_placement::RandomSpreadPlacement;
        use native_session::worldgen::schedule::GenerationStage;
        use std::io::Write;

        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let mut resources = ResourceStack::open(&game)?;
        let settings = draft_baked_worldgen_settings(
            &mut resources,
            "minecraft:normal",
            NewWorldOptions {
                seed: 42,
                generate_structures: true,
                bonus_chest: false,
            },
        )?;
        std::fs::create_dir_all(root.join("data/minecraft"))?;
        std::fs::write(
            root.join("data/minecraft/world_gen_settings.dat"),
            native_persistence::nbt::write(&settings)?,
        )?;
        let pack = root.join("datapacks/structure-test");
        let dimension = pack.join("data/minecraft/dimension/overworld.json");
        let biome = pack.join("data/example/worldgen/biome/isolated.json");
        let structure = pack.join("data/example/worldgen/structure/treasure.json");
        let structure_set = pack.join("data/example/worldgen/structure_set/treasure.json");
        for path in [&dimension, &biome, &structure, &structure_set] {
            std::fs::create_dir_all(path.parent().context("pack path has no parent")?)?;
        }
        std::fs::write(
            pack.join("pack.mcmeta"),
            br#"{"pack":{"description":"test","min_format":121,"max_format":121}}"#,
        )?;
        std::fs::write(
            dimension,
            serde_json::to_vec(&json!({
                "type":"minecraft:overworld",
                "generator":{"type":"minecraft:noise","settings":"minecraft:overworld",
                    "biome_source":{"type":"minecraft:fixed","biome":"example:isolated"}}
            }))?,
        )?;
        let plains = resources
            .read_data("data/minecraft/worldgen/biome/plains.json")?
            .context("plains biome missing")?;
        std::fs::write(biome, plains)?;
        std::fs::write(
            structure,
            serde_json::to_vec(&json!({
                "type":"minecraft:buried_treasure", "biomes":["example:isolated"],
                "step":"underground_structures", "terrain_adaptation":"beard_thin"
            }))?,
        )?;
        let placement = json!({
            "type":"minecraft:random_spread", "spacing":32, "separation":0, "salt":0
        });
        std::fs::write(
            structure_set,
            serde_json::to_vec(&json!({
                "placement":placement, "structures":[{"structure":"example:treasure","weight":1}]
            }))?,
        )?;
        let [x, z] = RandomSpreadPlacement::decode(&placement)?.potential_chunk(42, [0, 0]);
        let target = native_world::loaded::ChunkPos { x, z };
        let level = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "Data".into(),
                Tag::Compound(vec![
                    ("DataVersion".into(), Tag::Int(5023)),
                    (
                        "DataPacks".into(),
                        Tag::Compound(vec![
                            (
                                "Enabled".into(),
                                Tag::List {
                                    element_type: TagType::String,
                                    values: vec![
                                        Tag::String("vanilla".into()),
                                        Tag::String("file/structure-test".into()),
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
                    ),
                ]),
            )]),
        };
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&native_persistence::nbt::write(&level)?)?;
        std::fs::write(root.join("level.dat"), gzip.finish()?)?;

        let mut schedule = GenerationSchedule::new();
        schedule.request(target, GenerationStage::Terrain)?;
        let mut reader = WorldReader::open_for_session(root, DimensionId::parse("overworld")?)?;
        let mut actual = prepare_existing_generator_for_schedule(&mut reader, &schedule, &game)?;
        assert!(actual.generate_structures);
        let requirements = schedule.requirements().count();
        let starts = schedule
            .requirements()
            .filter(|(_, stage)| *stage >= GenerationStage::StructureStarts)
            .count();
        let mut checks = 0;
        let cancelled = actual.begin_captured_generation_schedule_checked(schedule.clone(), || {
            checks += 1;
            ensure!(
                checks < requirements + starts + 4,
                "cancel structure generation"
            );
            Ok(())
        });
        assert!(
            format!("{:#}", cancelled.err().context("cancellation was ignored")?)
                .contains("cancel structure generation")
        );
        let mut work = actual.begin_captured_generation_schedule(schedule)?;
        let treasure = ResourceId::parse("example:treasure")?;
        let starts = work
            .generated_starts
            .get(&target)
            .context("start missing")?;
        assert!(starts.starts().contains_key(&treasure));
        let references = &work.generated_references[&target];
        assert!(references.references.membership().contains_key(&treasure));
        let keys: Vec<_> = references.references.membership().keys().cloned().collect();
        assert!(!starts.terrain_pieces(&treasure)?.is_empty());
        let direct_generator = TerrainGenerator::with_structure_context(
            &mut actual.resources,
            &actual.generator_definition,
            42,
            actual.lighting.min_y,
            actual.lighting.height,
        )?;
        let mut direct =
            StructureGenerationBatch::from_registry(direct_generator, &mut actual.resources)?;
        direct.request_references(target)?;
        let registry = &actual
            .terrain_assets
            .as_ref()
            .context("registry missing")?
            .blocks;
        let height_tags = actual
            .heightmap_tags
            .as_ref()
            .context("height tags missing")?;
        while direct
            .step(&mut actual.resources, &mut |value| {
                let block = resolve_generated_terrain_state(value, registry)?;
                let motion = height_tags.for_block(&block.name)?;
                Ok(native_persistence::heightmap::BlockHeightFlags {
                    is_air: matches!(
                        block.name.as_str(),
                        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                    ),
                    blocks_motion: motion.blocks_motion,
                    blocks_motion_no_leaves: motion.blocks_motion_no_leaves,
                    has_fluid: FluidState::from_block(&block)
                        .map_err(anyhow::Error::msg)?
                        .is_some(),
                })
            })?
            .is_some()
        {}
        assert_eq!(
            direct
                .references(target)
                .context("direct references missing")?,
            references.references.membership()
        );
        let order = direct.terrain_start_order(target, &keys)?;
        let expected = direct.beardifier_for_chunk(target, &order)?;
        let observed = actual.beardifier_from_captured_references(
            target,
            references,
            &keys,
            &work.generated_starts,
        )?;
        for y in [-32, 0, 64, 128] {
            for dx in [0, 8, 15] {
                for dz in [0, 8, 15] {
                    assert_eq!(
                        observed.sample(x * 16 + dx, y, z * 16 + dz).to_bits(),
                        expected.sample(x * 16 + dx, y, z * 16 + dz).to_bits()
                    );
                }
            }
        }
        let positions: Vec<_> = work
            .schedule
            .requirements()
            .filter_map(|(position, required)| {
                (required >= GenerationStage::Biomes
                    && work.schedule.completed(position) < Some(GenerationStage::Biomes))
                .then_some(position)
            })
            .collect();
        let mut evaluator = native_resources::density::DensityEvaluator::default();
        for position in positions {
            actual.run_captured_biomes(&mut work, &mut evaluator, position)?;
        }
        actual.run_captured_new_terrain_with_structure_order(
            &mut work,
            &mut evaluator,
            target,
            &keys,
        )?;
        assert_eq!(
            work.schedule.completed(target),
            Some(GenerationStage::Terrain)
        );
        Ok(())
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn captured_empty_chunk_generates_scheduled_terrain_without_structures() -> Result<()> {
        use flate2::{Compression, write::GzEncoder};
        use native_persistence::worldgen_settings::{
            NewWorldOptions, draft_baked_worldgen_settings,
        };
        use native_session::worldgen::schedule::GenerationStage;
        use std::io::Write;

        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let mut resources = ResourceStack::open(&game)?;
        let settings = draft_baked_worldgen_settings(
            &mut resources,
            "minecraft:normal",
            NewWorldOptions {
                seed: 42,
                generate_structures: false,
                bonus_chest: false,
            },
        )?;
        std::fs::create_dir_all(root.join("data/minecraft"))?;
        std::fs::write(
            root.join("data/minecraft/world_gen_settings.dat"),
            native_persistence::nbt::write(&settings)?,
        )?;
        let pack = root.join("datapacks/mixed");
        let dimension_file = pack.join("data/minecraft/dimension/overworld.json");
        let biome_file = pack.join("data/minecraft/worldgen/biome/plains.json");
        std::fs::create_dir_all(dimension_file.parent().unwrap())?;
        std::fs::create_dir_all(biome_file.parent().unwrap())?;
        std::fs::write(
            pack.join("pack.mcmeta"),
            br#"{"pack":{"description":"test","min_format":121,"max_format":121}}"#,
        )?;
        std::fs::write(
            &dimension_file,
            serde_json::to_vec(&json!({
                "type":"minecraft:overworld",
                "generator":{"type":"minecraft:noise","settings":"minecraft:overworld",
                    "biome_source":{"type":"minecraft:fixed","biome":"minecraft:plains"}}
            }))?,
        )?;
        let mut plains: Value = serde_json::from_slice(
            &resources
                .read_data("data/minecraft/worldgen/biome/plains.json")?
                .context("plains biome missing")?,
        )?;
        plains["features"] = json!([[{
            "feature":{"type":"minecraft:simple_block", "to_place":{"id":"minecraft:melon"}, "schedule_tick":true},
            "placement":[{"type":"minecraft:offset","x":8,"y":8,"z":8},
                {"type":"minecraft:block_predicate_filter", "predicate":{"type":"minecraft:not", "predicate":{"type":"minecraft:would_survive", "state":"minecraft:brown_mushroom"}}}]
        }]]);
        plains["attributes"]["minecraft:gameplay/creature_world_gen_spawn_probability"] =
            json!(0.95);
        plains["attributes"]["minecraft:gameplay/natural_mob_spawns"]["argument"]["spawns_by_category"]
            ["creature"] = json!([{"type":"minecraft:pig","weight":1,"count":4}]);
        std::fs::write(&biome_file, serde_json::to_vec(&plains)?)?;
        let level = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "Data".into(),
                Tag::Compound(vec![
                    ("DataVersion".into(), Tag::Int(5023)),
                    (
                        "DataPacks".into(),
                        Tag::Compound(vec![
                            (
                                "Enabled".into(),
                                Tag::List {
                                    element_type: TagType::String,
                                    values: vec![
                                        Tag::String("vanilla".into()),
                                        Tag::String("file/mixed".into()),
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
                    ),
                ]),
            )]),
        };
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&native_persistence::nbt::write(&level)?)?;
        std::fs::write(root.join("level.dat"), gzip.finish()?)?;
        let target = native_world::loaded::ChunkPos { x: -1, z: 2 };
        let structure_target = native_world::loaded::ChunkPos { x: -3, z: 2 };
        let second = native_world::loaded::ChunkPos { x: 0, z: 2 };
        let third = native_world::loaded::ChunkPos { x: 1, z: 2 };
        let neighbor = native_world::loaded::ChunkPos { x: -1, z: 1 };
        let support = [
            native_world::loaded::ChunkPos { x: -2, z: 1 },
            native_world::loaded::ChunkPos { x: -2, z: 2 },
            native_world::loaded::ChunkPos { x: -2, z: 3 },
            native_world::loaded::ChunkPos { x: 0, z: 1 },
            native_world::loaded::ChunkPos { x: 1, z: 1 },
            native_world::loaded::ChunkPos { x: -1, z: 3 },
            native_world::loaded::ChunkPos { x: 0, z: 3 },
            native_world::loaded::ChunkPos { x: 1, z: 3 },
        ];
        let (lighting, _, _) = GeneratedDimensionLighting::load_preset_with_definition(
            &mut resources,
            "minecraft:normal",
            "minecraft:overworld",
        )?;
        let sections = (0..lighting.height / 16)
            .map(|index| {
                Tag::Compound(vec![
                    (
                        "Y".into(),
                        Tag::Byte(i8::try_from(lighting.min_y / 16 + index).unwrap()),
                    ),
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
                    (
                        "biomes".into(),
                        Tag::Compound(vec![(
                            "palette".into(),
                            Tag::List {
                                element_type: TagType::String,
                                values: vec![Tag::String("minecraft:plains".into())],
                            },
                        )]),
                    ),
                    ("example:section".into(), Tag::Int(91)),
                ])
            })
            .collect();
        let proto_entity = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:pig".into())),
            ("UUID".into(), Tag::IntArray(vec![55, 66, 77, 88])),
            ("example:proto_field".into(), Tag::Int(89)),
        ]);
        let saved = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("xPos".into(), Tag::Int(second.x)),
                ("zPos".into(), Tag::Int(second.z)),
                ("Status".into(), Tag::String("minecraft:biomes".into())),
                ("example:root".into(), Tag::Int(73)),
                (
                    "entities".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![proto_entity.clone()],
                    },
                ),
                (
                    "UpgradeData".into(),
                    Tag::Compound(vec![
                        ("Sides".into(), Tag::Byte(0)),
                        (
                            "neighbor_block_ticks".into(),
                            Tag::List {
                                element_type: TagType::Compound,
                                values: vec![Tag::Compound(vec![
                                    ("i".into(), Tag::String("minecraft:stone".into())),
                                    ("x".into(), Tag::Int(8)),
                                    ("y".into(), Tag::Int(8)),
                                    ("z".into(), Tag::Int(40)),
                                    ("t".into(), Tag::Int(5)),
                                    ("p".into(), Tag::Int(1)),
                                ])],
                            },
                        ),
                        (
                            "neighbor_fluid_ticks".into(),
                            Tag::List {
                                element_type: TagType::Compound,
                                values: vec![Tag::Compound(vec![
                                    ("i".into(), Tag::String("minecraft:water".into())),
                                    ("x".into(), Tag::Int(8)),
                                    ("y".into(), Tag::Int(8)),
                                    ("z".into(), Tag::Int(40)),
                                    ("t".into(), Tag::Int(3)),
                                    ("p".into(), Tag::Int(-1)),
                                ])],
                            },
                        ),
                    ]),
                ),
                (
                    "Heightmaps".into(),
                    Tag::Compound(vec![("example:map".into(), Tag::Int(37))]),
                ),
                (
                    "structures".into(),
                    Tag::Compound(vec![(
                        "starts".into(),
                        Tag::Compound(vec![(
                            "minecraft:buried_treasure".into(),
                            Tag::Compound(vec![
                                ("id".into(), Tag::String("minecraft:buried_treasure".into())),
                                ("ChunkX".into(), Tag::Int(second.x)),
                                ("ChunkZ".into(), Tag::Int(second.z)),
                                ("references".into(), Tag::Int(0)),
                                (
                                    "Children".into(),
                                    Tag::List {
                                        element_type: TagType::Compound,
                                        values: vec![Tag::Compound(vec![
                                            ("id".into(), Tag::String("minecraft:btp".into())),
                                            (
                                                "BB".into(),
                                                Tag::IntArray(vec![
                                                    -33,
                                                    lighting.min_y + 8,
                                                    35,
                                                    -33,
                                                    lighting.min_y + 8,
                                                    35,
                                                ]),
                                            ),
                                            ("O".into(), Tag::Int(-1)),
                                            ("GD".into(), Tag::Int(0)),
                                        ])],
                                    },
                                ),
                            ]),
                        )]),
                    )]),
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
        let bytes = native_persistence::nbt::write(&saved)?;
        let sectors = (bytes.len() + 5).div_ceil(4096);
        ensure!(sectors < 256, "test protochunk is too large");
        let mut region = vec![0; 8192 + sectors * 4096];
        let header = usize::try_from((second.x + second.z * 32) * 4)?;
        region[header..header + 4].copy_from_slice(&((2u32 << 8) | sectors as u32).to_be_bytes());
        region[8192..8196].copy_from_slice(&u32::try_from(bytes.len() + 1)?.to_be_bytes());
        region[8196] = 3;
        region[8197..8197 + bytes.len()].copy_from_slice(&bytes);
        let early_saved = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("xPos".into(), Tag::Int(third.x)),
                ("zPos".into(), Tag::Int(third.z)),
                (
                    "Status".into(),
                    Tag::String("minecraft:structure_references".into()),
                ),
                ("example:root".into(), Tag::Int(74)),
            ]),
        };
        let bytes = native_persistence::nbt::write(&early_saved)?;
        let sectors = (bytes.len() + 5).div_ceil(4096);
        let offset = region.len() / 4096;
        ensure!(
            sectors < 256 && offset < (1 << 24),
            "test protochunk is too large"
        );
        let header = usize::try_from((third.x + third.z * 32) * 4)?;
        region[header..header + 4]
            .copy_from_slice(&(((offset as u32) << 8) | sectors as u32).to_be_bytes());
        region.resize(region.len() + sectors * 4096, 0);
        let start = offset * 4096;
        region[start..start + 4].copy_from_slice(&u32::try_from(bytes.len() + 1)?.to_be_bytes());
        region[start + 4] = 3;
        region[start + 5..start + 5 + bytes.len()].copy_from_slice(&bytes);
        let region_dir = root.join("dimensions/minecraft/overworld/region");
        std::fs::create_dir_all(&region_dir)?;
        std::fs::write(region_dir.join("r.0.0.mca"), region)?;
        let existing_poi_source = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("example:poi_root".into(), Tag::Int(86)),
                (
                    "Sections".into(),
                    Tag::Compound(vec![(
                        "0".into(),
                        Tag::Compound(vec![
                            ("Valid".into(), Tag::Byte(1)),
                            (
                                "Records".into(),
                                Tag::List {
                                    element_type: TagType::End,
                                    values: vec![],
                                },
                            ),
                        ]),
                    )]),
                ),
            ]),
        };
        let poi_bytes = native_persistence::nbt::write(&existing_poi_source)?;
        let poi_sectors = (poi_bytes.len() + 5).div_ceil(4096);
        let mut poi_region = vec![0; 8192 + poi_sectors * 4096];
        let poi_header = usize::try_from((second.x + second.z * 32) * 4)?;
        poi_region[poi_header..poi_header + 4]
            .copy_from_slice(&((2u32 << 8) | poi_sectors as u32).to_be_bytes());
        poi_region[8192..8196].copy_from_slice(&u32::try_from(poi_bytes.len() + 1)?.to_be_bytes());
        poi_region[8196] = 3;
        poi_region[8197..8197 + poi_bytes.len()].copy_from_slice(&poi_bytes);
        let poi_dir = root.join("dimensions/minecraft/overworld/poi");
        std::fs::create_dir_all(&poi_dir)?;
        std::fs::write(poi_dir.join("r.0.0.mca"), poi_region)?;
        let existing_poi_source =
            Arc::new(native_persistence::world::SavedPoiChunk::from_document(
                existing_poi_source,
                native_persistence::world::ChunkPos {
                    x: second.x,
                    z: second.z,
                },
            )?);
        let existing_entity = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:pig".into())),
            ("UUID".into(), Tag::IntArray(vec![11, 22, 33, 44])),
            ("example:entity_field".into(), Tag::Int(88)),
        ]);
        let mut existing_entity_document =
            native_persistence::generated_chunk::assemble_entity_chunk(
                second,
                &[existing_entity.clone()],
            )?;
        let Tag::Compound(fields) = &mut existing_entity_document.value else {
            unreachable!()
        };
        fields.push(("example:entity_root".into(), Tag::Int(87)));
        let entity_bytes = native_persistence::nbt::write(&existing_entity_document)?;
        let entity_sectors = (entity_bytes.len() + 5).div_ceil(4096);
        let mut entity_region = vec![0; 8192 + entity_sectors * 4096];
        let entity_header = usize::try_from((second.x + second.z * 32) * 4)?;
        entity_region[entity_header..entity_header + 4]
            .copy_from_slice(&((2u32 << 8) | entity_sectors as u32).to_be_bytes());
        entity_region[8192..8196]
            .copy_from_slice(&u32::try_from(entity_bytes.len() + 1)?.to_be_bytes());
        entity_region[8196] = 3;
        entity_region[8197..8197 + entity_bytes.len()].copy_from_slice(&entity_bytes);
        let entity_dir = root.join("dimensions/minecraft/overworld/entities");
        std::fs::create_dir_all(&entity_dir)?;
        std::fs::write(entity_dir.join("r.0.0.mca"), entity_region)?;
        let existing_entity_source = Arc::new(native_persistence::world::SavedEntityChunk {
            position: native_persistence::world::ChunkPos {
                x: second.x,
                z: second.z,
            },
            document: Arc::new(existing_entity_document),
        });
        // Keep earlier protochunks far outside the test generation schedule.
        // Their original NBT must be captured without assuming biome data exists.
        let early_positions = [
            (
                native_persistence::world::ChunkPos { x: 1000, z: 0 },
                "minecraft:empty",
            ),
            (
                native_persistence::world::ChunkPos { x: 1001, z: 0 },
                "minecraft:structure_starts",
            ),
            (
                native_persistence::world::ChunkPos { x: 1002, z: 0 },
                "minecraft:structure_references",
            ),
        ];
        let mut early_region = vec![0; 8192];
        for (position, status) in early_positions {
            let mut document = NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![
                    ("DataVersion".into(), Tag::Int(5023)),
                    ("xPos".into(), Tag::Int(position.x)),
                    ("zPos".into(), Tag::Int(position.z)),
                    ("Status".into(), Tag::String(status.into())),
                    ("example:unrecognized".into(), Tag::Int(73)),
                ]),
            };
            if position.x == 1000 {
                let Tag::Compound(fields) = &mut document.value else {
                    unreachable!()
                };
                fields.push((
                    "structures".into(),
                    Tag::Compound(vec![
                        (
                            "References".into(),
                            Tag::Compound(vec![(
                                "minecraft:buried_treasure".into(),
                                Tag::LongArray(vec![1002]),
                            )]),
                        ),
                        ("custom".into(), Tag::String("preserve".into())),
                    ]),
                ));
            }
            if position.x >= 1001 {
                let Tag::Compound(fields) = &mut document.value else {
                    unreachable!()
                };
                fields.push((
                    "structures".into(),
                    Tag::Compound(vec![
                        (
                            "starts".into(),
                            Tag::Compound(vec![(
                                "minecraft:buried_treasure".into(),
                                Tag::Compound(vec![
                                    ("id".into(), Tag::String("minecraft:buried_treasure".into())),
                                    ("ChunkX".into(), Tag::Int(position.x)),
                                    ("ChunkZ".into(), Tag::Int(position.z)),
                                    ("references".into(), Tag::Int(0)),
                                    (
                                        "Children".into(),
                                        Tag::List {
                                            element_type: TagType::Compound,
                                            values: vec![Tag::Compound(vec![
                                                ("id".into(), Tag::String("minecraft:btp".into())),
                                                (
                                                    "BB".into(),
                                                    Tag::IntArray(vec![
                                                        16000,
                                                        lighting.min_y + 8,
                                                        8,
                                                        16000,
                                                        lighting.min_y + 8,
                                                        8,
                                                    ]),
                                                ),
                                                ("O".into(), Tag::Int(-1)),
                                                ("GD".into(), Tag::Int(0)),
                                            ])],
                                        },
                                    ),
                                ]),
                            )]),
                        ),
                        (
                            "References".into(),
                            Tag::Compound(vec![(
                                "minecraft:buried_treasure".into(),
                                Tag::LongArray(vec![i64::from(position.x)]),
                            )]),
                        ),
                    ]),
                ));
            }
            let bytes = native_persistence::nbt::write(&document)?;
            let sectors = (bytes.len() + 5).div_ceil(4096);
            let offset = early_region.len() / 4096;
            ensure!(
                sectors < 256 && offset < (1 << 24),
                "test protochunk is too large"
            );
            let header =
                usize::try_from((position.x.rem_euclid(32) + position.z.rem_euclid(32) * 32) * 4)?;
            early_region[header..header + 4]
                .copy_from_slice(&(((offset as u32) << 8) | sectors as u32).to_be_bytes());
            early_region.resize(early_region.len() + sectors * 4096, 0);
            let start = offset * 4096;
            early_region[start..start + 4]
                .copy_from_slice(&u32::try_from(bytes.len() + 1)?.to_be_bytes());
            early_region[start + 4] = 3;
            early_region[start + 5..start + 5 + bytes.len()].copy_from_slice(&bytes);
        }
        std::fs::write(region_dir.join("r.31.0.mca"), early_region)?;
        let mut reader = WorldReader::open_for_session(root, DimensionId::parse("overworld")?)?;
        let saved_target = native_persistence::world::ChunkPos {
            x: target.x,
            z: target.z,
        };
        let saved_second = native_persistence::world::ChunkPos {
            x: second.x,
            z: second.z,
        };
        let saved_third = native_persistence::world::ChunkPos {
            x: third.x,
            z: third.z,
        };
        let saved_neighbor = native_persistence::world::ChunkPos {
            x: neighbor.x,
            z: neighbor.z,
        };
        let mut schedule = GenerationSchedule::new();
        schedule.request(target, GenerationStage::Spawn)?;
        schedule.request(second, GenerationStage::Spawn)?;
        schedule.request(third, GenerationStage::Spawn)?;
        schedule.request(neighbor, GenerationStage::Spawn)?;
        for &position in &support {
            schedule.request(position, GenerationStage::Spawn)?;
        }
        schedule.request(structure_target, GenerationStage::Spawn)?;
        let mut actual = prepare_existing_generator_for_schedule(&mut reader, &schedule, &game)?;
        assert!(!actual.generate_structures);
        let first_registry = actual.structure_registry()?;
        assert!(Arc::ptr_eq(&first_registry, &actual.structure_registry()?));
        // The production initializer must restore actual saved stage data and
        // run missing structure stages, rather than trust saved status alone.
        let make_plan = || -> Result<GenerationSchedule> {
            let mut plan = GenerationSchedule::new();
            plan.request(target, GenerationStage::Terrain)?;
            plan.request(structure_target, GenerationStage::Terrain)?;
            Ok(plan)
        };
        let mut checks = 0;
        let interrupted = actual.begin_captured_generation_schedule_checked(make_plan()?, || {
            checks += 1;
            ensure!(checks < 3, "test cancellation");
            Ok(())
        });
        assert!(interrupted.is_err());
        assert_eq!(checks, 3);
        actual.generate_structures = true;
        let structured = actual.begin_captured_generation_schedule(make_plan()?)?;
        assert!(!structured.generated_starts.is_empty());
        let treasure = ResourceId::parse("minecraft:buried_treasure")?;
        assert!(
            structured.generated_references[&structure_target]
                .references
                .membership()[&treasure]
                .contains(&(2i64 << 32))
        );
        actual.generate_structures = false;
        let mut automatic = actual.begin_captured_generation_schedule(make_plan()?)?;
        assert_eq!(
            automatic.schedule.completed(second),
            Some(GenerationStage::Biomes)
        );
        assert!(automatic.biomes.chunk([second.x, second.z]).is_some());
        assert_eq!(
            automatic.schedule.completed(structure_target),
            Some(GenerationStage::StructureReferences)
        );
        assert_eq!(
            automatic.generated_references[&structure_target]
                .references
                .membership()[&treasure],
            BTreeSet::from([2i64 << 32]),
        );
        let needed_biomes: Vec<_> = automatic
            .schedule
            .requirements()
            .filter_map(|(position, required)| {
                (required >= GenerationStage::Biomes
                    && automatic
                        .schedule
                        .completed(position)
                        .is_some_and(|stage| stage < GenerationStage::Biomes))
                .then_some(position)
            })
            .collect();
        let mut automatic_evaluator = native_resources::density::DensityEvaluator::default();
        for position in needed_biomes {
            actual.run_captured_biomes(&mut automatic, &mut automatic_evaluator, position)?;
        }
        assert!(
            automatic
                .schedule
                .ready(structure_target, GenerationStage::Terrain)
        );
        assert!(
            actual
                .run_captured_new_terrain(
                    &mut automatic,
                    &mut automatic_evaluator,
                    structure_target
                )
                .is_err()
        );
        actual.run_captured_new_terrain_with_structure_order(
            &mut automatic,
            &mut automatic_evaluator,
            structure_target,
            std::slice::from_ref(&treasure),
        )?;
        assert_eq!(
            automatic.schedule.completed(structure_target),
            Some(GenerationStage::Terrain)
        );
        assert!(
            automatic
                .generated_references
                .contains_key(&structure_target)
        );
        // The saved BIOMES protochunk participates in the live-state guard
        // even though it has not yet reached TERRAIN. It must not be silently
        // promoted or treated as an absent chunk.
        let restored = actual.restore_captured_terrain()?;
        assert_eq!(restored.world().chunks.len(), 2);
        let restored_chunk = restored
            .world()
            .chunks
            .get(&second)
            .context("saved protochunk missing")?;
        assert_eq!(restored_chunk.status.as_deref(), Some("minecraft:biomes"));
        assert!(restored.biomes().contains_key(&second));
        assert!(actual.restore_saved_terrain([saved_second]).is_err());
        let player = native_session::PlayerState {
            position: glam::Vec3::new(8.5, lighting.min_y as f32 + 2.0, 40.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let mut live = native_session::loaded::LoadedSession::new(
            restored.world().clone(),
            glam::IVec3::ZERO,
            native_session::loaded::BuildBounds {
                min_y: lighting.min_y,
                max_y: lighting.min_y + lighting.height,
            },
            player,
        )
        .map_err(anyhow::Error::msg)?;
        assert!(actual.capture_live_inputs(&live, &restored).is_err());
        assert!(
            actual
                .capture_live_inputs_with_deferred_shapes(&live, &restored)
                .is_err()
        );
        live.set_initial_poi_chunk(
            second,
            native_session::loaded::SavedPoiData::Present(Arc::clone(&existing_poi_source)),
        )?;
        live.set_initial_entity_chunk(
            second,
            native_session::loaded::SavedEntityData::Present(Arc::clone(&existing_entity_source)),
        )?;
        live.set_initial_poi_chunk(third, native_session::loaded::SavedPoiData::Missing)?;
        live.set_initial_entity_chunk(third, native_session::loaded::SavedEntityData::Empty)?;
        let guard = actual.capture_live_inputs(&live, &restored)?;
        actual.verify_live_inputs(&live, &guard)?;
        let deferred_guard = actual.capture_live_inputs_with_deferred_shapes(&live, &restored)?;
        actual.verify_live_inputs(&live, &deferred_guard)?;
        live.queue_missing_chunk(second);
        assert!(actual.verify_live_inputs(&live, &guard).is_err());
        assert!(actual.verify_live_inputs(&live, &deferred_guard).is_err());
        assert_eq!(
            actual.context.inputs().sources().chunks()[&saved_second]
                .terrain
                .as_ref()
                .unwrap()
                .document
                .as_ref(),
            &saved
        );
        let early_context = reader.capture_generation_context(
            early_positions
                .iter()
                .map(|(position, _)| *position)
                .chain([saved_second]),
        )?;
        let mut early_generator = prepare_existing_generator(early_context, &game)?;
        let early_restored = early_generator.restore_captured_terrain()?;
        assert_eq!(early_restored.world().chunks.len(), 4);
        assert_eq!(early_restored.biomes().len(), 1);
        assert!(early_restored.biomes().contains_key(&second));
        let without_biomes = early_generator.restore_saved_terrain_inner(
            early_positions.iter().map(|(position, _)| *position),
            true,
        )?;
        assert_eq!(without_biomes.world().chunks.len(), early_positions.len());
        assert!(without_biomes.biomes().is_empty());
        for (position, status) in early_positions {
            let key = native_world::loaded::ChunkPos {
                x: position.x,
                z: position.z,
            };
            assert_eq!(
                early_restored.world().chunks[&key].status.as_deref(),
                Some(status)
            );
            assert!(!early_restored.biomes().contains_key(&key));
            assert!(early_generator.restore_saved_terrain([position]).is_err());
            assert_eq!(
                early_generator.inputs().sources().chunks()[&position]
                    .terrain
                    .as_ref()
                    .unwrap()
                    .document
                    .value
                    .get("example:unrecognized"),
                Some(&Tag::Int(73))
            );
        }
        let mut early_live = native_session::loaded::LoadedSession::new(
            early_restored.world().clone(),
            glam::IVec3::ZERO,
            native_session::loaded::BuildBounds {
                min_y: lighting.min_y,
                max_y: lighting.min_y + lighting.height,
            },
            player,
        )
        .map_err(anyhow::Error::msg)?;
        for (position, _) in early_positions {
            let key = native_world::loaded::ChunkPos {
                x: position.x,
                z: position.z,
            };
            early_live.set_initial_poi_chunk(key, native_session::loaded::SavedPoiData::Missing)?;
            early_live
                .set_initial_entity_chunk(key, native_session::loaded::SavedEntityData::Empty)?;
        }
        early_live.set_initial_poi_chunk(
            second,
            native_session::loaded::SavedPoiData::Present(Arc::clone(&existing_poi_source)),
        )?;
        early_live.set_initial_entity_chunk(
            second,
            native_session::loaded::SavedEntityData::Present(Arc::clone(&existing_entity_source)),
        )?;
        let early_guard = early_generator.capture_live_inputs(&early_live, &early_restored)?;
        early_generator.verify_live_inputs(&early_live, &early_guard)?;
        early_live.queue_missing_chunk(native_world::loaded::ChunkPos { x: 1000, z: 0 });
        assert!(
            early_generator
                .verify_live_inputs(&early_live, &early_guard)
                .is_err()
        );
        let selected = actual.context.selection();
        let selection = DataPackSettings {
            enabled: selected.enabled.clone(),
            disabled: selected.disabled.clone(),
            enabled_features: selected.enabled_features.clone(),
        };
        let warnings =
            resources.configure_snapshot_data(Arc::clone(actual.context.packs()), &selection)?;
        ensure!(warnings.is_empty(), "reference data-pack selection differs");
        let (stem, _) = resolve_stem(
            &mut resources,
            actual.context.inputs().settings().document(),
            &DimensionId::parse("overworld")?,
        )?;
        let (_, definition, _) = GeneratedDimensionLighting::from_stem(
            &mut resources,
            ResourceId::parse("minecraft:overworld")?,
            &stem,
        )?;
        // A saved protochunk before BIOMES must follow the schedule just like
        // an absent chunk, while keeping its original document as the source.
        let mut early_schedule = GenerationSchedule::new();
        for (position, _) in early_positions {
            early_schedule.request(
                native_world::loaded::ChunkPos {
                    x: position.x,
                    z: position.z,
                },
                GenerationStage::Terrain,
            )?;
        }
        let early_biome_positions: Vec<_> = early_schedule
            .requirements()
            .filter_map(|(pos, required)| (required >= GenerationStage::Biomes).then_some(pos))
            .collect();
        let imported: Vec<_> = early_schedule
            .requirements()
            .map(|(pos, required)| {
                (
                    pos,
                    if required >= GenerationStage::Biomes {
                        GenerationStage::StructureReferences
                    } else {
                        GenerationStage::StructureStarts
                    },
                )
            })
            .collect();
        let early_context = reader.capture_generation_context(
            early_schedule
                .requirements()
                .map(|(pos, _)| native_persistence::world::ChunkPos { x: pos.x, z: pos.z })
                .chain([saved_second]),
        )?;
        early_schedule.restore_imported(imported)?;
        let mut scheduled_early = prepare_existing_generator(early_context, &game)?;
        let mut early_work = scheduled_early.begin_captured_biome_schedule(early_schedule, [])?;
        let mut early_evaluator = native_resources::density::DensityEvaluator::default();
        let mut early_reference = TerrainGenerator::without_terrain_adjustments(
            &mut resources,
            &definition,
            42,
            lighting.min_y,
            lighting.height,
        )?;
        let mut early_reference_evaluator = native_resources::density::DensityEvaluator::default();
        assert!(
            scheduled_early
                .run_captured_biomes(&mut early_work, &mut early_evaluator, second)
                .is_err()
        );
        for (position, status) in early_positions {
            let key = native_world::loaded::ChunkPos {
                x: position.x,
                z: position.z,
            };
            scheduled_early.run_captured_biomes(&mut early_work, &mut early_evaluator, key)?;
            assert_eq!(
                early_work.schedule().completed(key),
                Some(GenerationStage::Biomes)
            );
            let expected =
                early_reference.generate_biomes([key.x, key.z], &mut early_reference_evaluator)?;
            assert_eq!(
                early_work.biomes().chunk([key.x, key.z]).unwrap().values(),
                expected.values()
            );
            let source = scheduled_early.inputs().sources().chunks()[&position]
                .terrain
                .as_ref()
                .unwrap();
            assert!(source.status.as_ref().unwrap().matches(status));
            assert_eq!(
                source.document.value.get("example:unrecognized"),
                Some(&Tag::Int(73))
            );
            assert!(
                scheduled_early
                    .run_captured_biomes(&mut early_work, &mut early_evaluator, key)
                    .is_err()
            );
        }
        for position in early_biome_positions {
            if !early_positions
                .iter()
                .any(|(saved, _)| saved.x == position.x && saved.z == position.z)
            {
                scheduled_early.run_captured_biomes(
                    &mut early_work,
                    &mut early_evaluator,
                    position,
                )?;
            }
        }
        let first = early_positions[0].0;
        let first_key = native_world::loaded::ChunkPos {
            x: first.x,
            z: first.z,
        };
        scheduled_early.generate_structures = true;
        assert!(
            scheduled_early
                .run_captured_saved_terrain(&mut early_work, &mut early_evaluator, first_key, &[])
                .is_err()
        );
        assert_eq!(
            early_work.schedule().completed(first_key),
            Some(GenerationStage::Biomes)
        );
        scheduled_early.generate_structures = false;
        let early_block_ids = resources.vanilla_block_ids()?;
        let early_tags = CarverBlockTags::load(&mut resources, &early_block_ids)?;
        let early_registry = resources.vanilla_generated_block_registry()?;
        let early_treasure = ResourceId::parse("minecraft:buried_treasure")?;
        for (position, _) in early_positions {
            let key = native_world::loaded::ChunkPos {
                x: position.x,
                z: position.z,
            };
            scheduled_early.generate_structures = position == early_positions[2].0;
            assert!(
                scheduled_early
                    .run_captured_saved_terrain(&mut early_work, &mut early_evaluator, key, &[],)
                    .is_err()
            );
            assert_eq!(
                early_work.schedule().completed(key),
                Some(GenerationStage::Biomes)
            );
            // A chunk with saved structure references can resume even when
            // this world enables generation of new structure starts.
            scheduled_early.generate_structures = position == early_positions[2].0;
            scheduled_early.run_captured_saved_terrain(
                &mut early_work,
                &mut early_evaluator,
                key,
                std::slice::from_ref(&early_treasure),
            )?;
            scheduled_early.generate_structures = false;
            if position == first {
                let generated = &early_work.generated_references[&key];
                assert_eq!(
                    generated.references.membership()[&early_treasure],
                    BTreeSet::from([1001, 1002])
                );
                let original = &scheduled_early.context.inputs().sources().chunks()[&position]
                    .terrain
                    .as_ref()
                    .unwrap()
                    .document;
                let mut merged = (**original).clone();
                merge_generated_structure_references(&mut merged, generated)?;
                let structures = merged.value.get("structures").unwrap();
                assert_eq!(
                    structures.get("custom"),
                    Some(&Tag::String("preserve".into()))
                );
                assert_eq!(
                    structures.get("References"),
                    Some(&generated.references.to_tag())
                );
                let mut stale = (**original).clone();
                replace_unique_root_field(&mut stale, "structures", Tag::Compound(vec![]))?;
                assert!(merge_generated_structure_references(&mut stale, generated).is_err());
            }
            let terrain = early_work
                .terrain_chunk(position)
                .context("resumed terrain missing")?;
            assert_eq!(
                early_work.schedule().completed(key),
                Some(GenerationStage::Terrain)
            );
            let expected = early_reference.generate_carved_surface(
                &mut resources,
                &mut early_reference_evaluator,
                &mut native_resources::carver::schedule::CarverCache::default(),
                [key.x, key.z],
                early_work.biomes(),
                &early_tags,
                |value| {
                    let state = resolve_generated_terrain_state(value, &early_registry)?;
                    Ok(CarverBlockState {
                        is_air: matches!(
                            state.name.as_str(),
                            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                        ),
                        has_fluid: FluidState::from_block(&state)
                            .map_err(anyhow::Error::msg)?
                            .is_some(),
                    })
                },
            )?;
            for z in [0usize, 15] {
                for x in [0usize, 15] {
                    for y in 0..lighting.height as usize {
                        let index = (z * 16 + x) * lighting.height as usize + y;
                        let state = resolve_generated_terrain_state(
                            &expected.states[index],
                            &early_registry,
                        )?;
                        assert_eq!(
                            terrain.materialized().world().block(
                                key.x * 16 + x as i32,
                                lighting.min_y + y as i32,
                                key.z * 16 + z as i32
                            ),
                            Some(&state)
                        );
                    }
                }
            }
            let draft = scheduled_early.draft_resumed_sections(terrain)?;
            let source = scheduled_early.inputs().sources().chunks()[&position]
                .terrain
                .as_ref()
                .unwrap();
            assert!(draft.matches_source(&source.document)?);
            assert_eq!(
                draft.document().value.get("Status"),
                source.document.value.get("Status")
            );
            assert_eq!(
                draft.document().value.get("example:unrecognized"),
                Some(&Tag::Int(73))
            );
            let sections = native_persistence::palette::decode_sections(&draft.document().value)?;
            assert_eq!(sections.len(), usize::try_from(lighting.height / 16)?);
            assert!(
                sections
                    .iter()
                    .all(|section| section.blocks.is_some() && section.biomes.is_some())
            );
        }
        let biome_positions: Vec<_> = schedule
            .requirements()
            .filter_map(|(pos, required)| (required >= GenerationStage::Biomes).then_some(pos))
            .collect();
        let terrain_positions: Vec<_> = schedule
            .requirements()
            .filter_map(|(pos, required)| (required >= GenerationStage::Terrain).then_some(pos))
            .collect();
        let feature_positions: Vec<_> = schedule
            .requirements()
            .filter_map(|(pos, required)| (required >= GenerationStage::Features).then_some(pos))
            .collect();
        let light_positions: Vec<_> = schedule
            .requirements()
            .filter_map(|(pos, required)| {
                (required >= GenerationStage::InitializeLight).then_some(pos)
            })
            .collect();
        // New starts are disabled; the saved second chunk retains its start.
        // Imported stage markers isolate the later terrain tests.
        let completed: Vec<_> = schedule
            .requirements()
            .map(|(pos, required)| {
                (
                    pos,
                    if pos == second {
                        GenerationStage::Biomes
                    } else if required >= GenerationStage::Biomes {
                        GenerationStage::StructureReferences
                    } else {
                        GenerationStage::StructureStarts
                    },
                )
            })
            .collect();
        schedule.restore_imported(completed)?;
        let mut work = actual.begin_captured_biome_schedule(schedule, [saved_second])?;
        let mut evaluator = native_resources::density::DensityEvaluator::default();
        assert!(
            actual
                .run_captured_new_terrain(&mut work, &mut evaluator, target)
                .is_err()
        );
        assert!(work.new_terrain_chunk(saved_target).is_none());
        assert_eq!(
            work.schedule().completed(target),
            Some(GenerationStage::StructureReferences)
        );
        for &pos in &biome_positions {
            if pos != second {
                actual.run_captured_biomes(&mut work, &mut evaluator, pos)?;
            }
        }
        assert!(work.schedule().ready(target, GenerationStage::Terrain));
        actual.run_captured_new_terrain(&mut work, &mut evaluator, target)?;
        assert_eq!(
            work.schedule().completed(target),
            Some(GenerationStage::Terrain)
        );
        assert!(
            actual
                .run_captured_new_terrain(&mut work, &mut evaluator, target)
                .is_err()
        );

        let mut reference = TerrainGenerator::without_terrain_adjustments(
            &mut resources,
            &definition,
            42,
            lighting.min_y,
            lighting.height,
        )?;
        let mut reference_biomes = reference.biome_neighborhood()?;
        let mut reference_evaluator = native_resources::density::DensityEvaluator::default();
        for &position in &biome_positions {
            let pos = [position.x, position.z];
            let biome = if pos == [second.x, second.z] {
                Arc::clone(work.biomes().chunk(pos).context("saved biome missing")?)
            } else {
                Arc::new(reference.generate_biomes(pos, &mut reference_evaluator)?)
            };
            reference_biomes.insert(biome)?;
        }
        let block_ids = resources.vanilla_block_ids()?;
        let tags = CarverBlockTags::load(&mut resources, &block_ids)?;
        let registry = resources.vanilla_generated_block_registry()?;
        let expected = reference.generate_carved_surface(
            &mut resources,
            &mut reference_evaluator,
            &mut native_resources::carver::schedule::CarverCache::default(),
            [target.x, target.z],
            &reference_biomes,
            &tags,
            |value| {
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
            },
        )?;
        let terrain = work
            .new_terrain_chunk(saved_target)
            .context("new terrain missing")?;
        assert_eq!(terrain.fluid_postprocessing(), expected.fluid_updates);
        assert_eq!(
            terrain.carver_postprocessing(),
            expected.carver_postprocessing.as_ref()
        );
        for z in [0usize, 15] {
            for x in [0usize, 15] {
                for y in 0..lighting.height as usize {
                    let index = (z * 16 + x) * lighting.height as usize + y;
                    let expected =
                        resolve_generated_terrain_state(&expected.states[index], &registry)?;
                    assert_eq!(
                        terrain.world().block(
                            target.x * 16 + x as i32,
                            lighting.min_y + y as i32,
                            target.z * 16 + z as i32
                        ),
                        Some(&expected)
                    );
                }
            }
        }
        actual.run_captured_saved_terrain(&mut work, &mut evaluator, second, &[])?;
        actual.run_captured_saved_terrain(&mut work, &mut evaluator, third, &[])?;
        let treasure = ResourceId::parse("minecraft:buried_treasure")?;
        assert!(terrain_positions.contains(&structure_target));
        for &position in &terrain_positions {
            if position != target && position != second && position != third {
                if position == structure_target {
                    // The saved start intersects this absent chunk; omitting
                    // its key must fail without advancing the schedule.
                    assert!(
                        actual
                            .run_captured_new_terrain(&mut work, &mut evaluator, position)
                            .is_err()
                    );
                    assert_eq!(
                        work.schedule().completed(position),
                        Some(GenerationStage::Biomes)
                    );
                    actual.run_captured_new_terrain_with_structure_order(
                        &mut work,
                        &mut evaluator,
                        position,
                        std::slice::from_ref(&treasure),
                    )?;
                } else {
                    actual.run_captured_new_terrain(&mut work, &mut evaluator, position)?;
                }
            }
        }
        let automatic_chunk = automatic
            .new_terrain_chunk(native_persistence::world::ChunkPos {
                x: structure_target.x,
                z: structure_target.z,
            })
            .context("automatic terrain is missing")?;
        let manually_scheduled = work
            .new_terrain_chunk(native_persistence::world::ChunkPos {
                x: structure_target.x,
                z: structure_target.z,
            })
            .context("manually scheduled terrain is missing")?;
        for z in [0, 15] {
            for x in [0, 15] {
                for y in lighting.min_y..lighting.min_y + lighting.height {
                    let block_x = structure_target.x * 16 + x;
                    let block_z = structure_target.z * 16 + z;
                    assert_eq!(
                        automatic_chunk.world().block(block_x, y, block_z),
                        manually_scheduled.world().block(block_x, y, block_z)
                    );
                }
            }
        }
        let sample_y = [lighting.min_y, 0, 63, lighting.min_y + lighting.height - 1];
        let positions = [target, second]
            .into_iter()
            .flat_map(|pos| {
                [0, 15].into_iter().flat_map(move |x| {
                    [0, 15].into_iter().flat_map(move |z| {
                        sample_y
                            .into_iter()
                            .map(move |y| (pos.x * 16 + x, y, pos.z * 16 + z))
                    })
                })
            })
            .collect::<Vec<_>>();
        let samples = positions
            .iter()
            .map(|&(x, y, z)| {
                let chunk = if x.div_euclid(16) == target.x {
                    work.new_terrain_chunk(saved_target).unwrap()
                } else {
                    work.terrain_chunk(saved_second).unwrap().materialized()
                };
                chunk.world().block(x, y, z).cloned()
            })
            .collect::<Vec<_>>();
        let batch = actual.assemble_captured_terrain(work)?;
        assert!(Arc::ptr_eq(&actual.identity, &batch.identity));
        assert_eq!(
            batch.schedule().completed(target),
            Some(GenerationStage::Terrain)
        );
        assert_eq!(
            batch.schedule().completed(second),
            Some(GenerationStage::Terrain)
        );
        assert_eq!(batch.sources().len(), terrain_positions.len());
        assert!(matches!(batch.sources().get(&saved_target), Some(None)));
        assert!(
            matches!(batch.sources().get(&saved_second), Some(Some(source)) if source.as_ref() == &saved)
        );
        assert!(
            matches!(batch.sources().get(&saved_third), Some(Some(source)) if source.as_ref() == &early_saved)
        );
        assert!(batch.biomes().chunk([second.x, second.z]).is_some());
        let world = batch.terrain().snapshot();
        assert!(world.chunks.contains_key(&target));
        assert!(world.chunks.contains_key(&second));
        for ((x, y, z), expected) in positions.into_iter().zip(samples) {
            assert_eq!(world.block(x, y, z), expected.as_ref());
        }
        let context = actual.prepare_captured_decoration(&batch, saved_target)?;
        assert_eq!(context.plan(&actual)?.features().len(), 1);
        assert!(batch.schedule().ready(target, GenerationStage::Features));
        let marker = [target.x * 16 + 8, lighting.min_y + 8, target.z * 16 + 8];
        assert_ne!(
            world.block(marker[0], marker[1], marker[2]).unwrap().name,
            "minecraft:melon"
        );
        {
            use native_core::beardifier::{BoundingBox, TerrainAdjustment};
            use native_resources::{
                feature_order::FeatureSchedule,
                mineshaft::{MineshaftGraphPiece, MineshaftGraphPieceKind, MineshaftVariant},
            };
            use native_session::worldgen::{
                SavedMineshaftPiece, structure_references::collect_ordered_structure_references,
            };

            struct TestBiome(bool);
            impl ResourceProvider for TestBiome {
                fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                    Ok(None)
                }
                fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                    if path != "data/minecraft/worldgen/biome/plains.json" {
                        return Ok(None);
                    }
                    let features = if self.0 {
                        json!([[], [], [], [{
                            "feature": {"type":"minecraft:simple_block", "to_place":{"id":"minecraft:melon"}},
                            "placement": [{"type":"minecraft:offset", "x":8, "y":8, "z":8},
                                {"type":"minecraft:block_predicate_filter", "predicate":{"type":"minecraft:not", "predicate":{"type":"minecraft:would_survive", "state":"minecraft:brown_mushroom"}}}]
                        }]])
                    } else {
                        json!([])
                    };
                    Ok(Some(serde_json::to_vec(&json!({"features": features}))?))
                }
            }
            let mine = ResourceId::parse("minecraft:mineshaft")?;
            let start = native_session::worldgen::mineshaft::PreparedStart {
                chunk: [second.x, second.z],
                generation_point: [target.x * 16 + 2, lighting.min_y + 8, target.z * 16 + 2],
                terrain_adjustment: TerrainAdjustment::None,
                variant: MineshaftVariant::Normal,
                pieces: vec![SavedMineshaftPiece::capture(
                    &MineshaftGraphPiece {
                        bounds: BoundingBox::new(
                            [target.x * 16 + 2, lighting.min_y + 8, target.z * 16 + 2],
                            [target.x * 16 + 10, lighting.min_y + 13, target.z * 16 + 10],
                        )
                        .unwrap(),
                        direction: None,
                        depth: 0,
                        kind: MineshaftGraphPieceKind::Room { entrances: vec![] },
                    },
                    MineshaftVariant::Normal,
                    false,
                )?],
            };
            let bounds = start.reference_bounds(mine.clone())?;
            let references = collect_ordered_structure_references(target, |position| {
                Ok(if position == second {
                    vec![bounds.clone()]
                } else {
                    Vec::new()
                })
            })?;
            let mut biomes = actual.generator.biome_neighborhood()?;
            for (position, _) in batch.schedule.requirements() {
                if let Some(biome) = batch.biomes.chunk([position.x, position.z]) {
                    biomes.insert(Arc::clone(biome))?;
                }
            }
            let mut mine_batch = CapturedTerrainBatch {
                identity: Arc::clone(&batch.identity),
                schedule: batch.schedule.clone(),
                biomes,
                terrain: batch.terrain.clone(),
                sources: batch.sources.clone(),
                saved_stages: batch.saved_stages.clone(),
                generated_references: BTreeMap::from([(
                    target,
                    CapturedGeneratedReferences {
                        original: None,
                        references,
                    },
                )]),
                generated_starts: batch.generated_starts.clone(),
                treasure_starts: batch.treasure_starts.clone(),
                feature_entity_sources: ResumedFeatureEntitySources::default(),
                revision: Arc::clone(&batch.revision),
            };
            let mut context_biomes = actual.generator.biome_neighborhood()?;
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let biome = mine_batch
                        .biomes
                        .chunk([target.x + dx, target.z + dz])
                        .context("test biome is missing")?;
                    context_biomes.insert(Arc::clone(biome))?;
                }
            }
            let plain = ResourceId::parse("minecraft:plains")?;
            let empty_schedule = Arc::new(FeatureSchedule::load(
                &mut TestBiome(false),
                std::slice::from_ref(&plain),
                &BTreeSet::new(),
            )?);
            let light_schedule = Arc::new(FeatureSchedule::load(
                &mut TestBiome(true),
                &[plain],
                &BTreeSet::new(),
            )?);
            let mut mine_context = SavedDecorationContext {
                identity: Arc::clone(&actual.identity),
                position: saved_target,
                schedule: light_schedule,
                biomes: context_biomes,
            };
            let source = Arc::clone(
                mine_batch.sources[&saved_second]
                    .as_ref()
                    .context("test source is missing")?,
            );
            let original = Tag::Compound(vec![("example:retained".into(), Tag::Int(42))]);
            let key = (mine, second);
            let saved_pieces: Vec<_> = start
                .pieces
                .iter()
                .cloned()
                .map(native_session::worldgen::saved_structure_starts::RestoredStructurePiece::Mineshaft)
                .collect();
            let mut starts = BTreeMap::from([(
                key.clone(),
                resume_saved_mineshaft(
                    Arc::clone(&source),
                    &original,
                    2,
                    second,
                    start.terrain_adjustment,
                    &saved_pieces,
                )?,
            )]);
            let before = mine_batch.terrain.snapshot();
            actual.generate_structures = true;
            let error = actual
                .run_captured_features_inner(
                    &mut mine_batch,
                    &mine_context,
                    None,
                    Some(&mut starts),
                    None,
                )
                .unwrap_err();
            assert!(format!("{error:#}").contains("raw brightness unavailable"));
            assert!(Arc::ptr_eq(&before, &mine_batch.terrain.snapshot()));
            assert_eq!(
                mine_batch.schedule.completed(target),
                Some(GenerationStage::Terrain)
            );
            assert!(Arc::ptr_eq(&starts[&key].source, &source));
            assert_eq!(starts[&key].original, original);
            assert_eq!(starts[&key].references, 2);
            mine_context.schedule = empty_schedule;
            let placed = actual.run_captured_features_inner(
                &mut mine_batch,
                &mine_context,
                None,
                Some(&mut starts),
                None,
            )?;
            actual.generate_structures = false;
            assert!(placed.is_empty());
            assert_eq!(
                mine_batch.schedule.completed(target),
                Some(GenerationStage::Features)
            );
            let after = mine_batch.terrain.snapshot();
            let mut changed = false;
            for y in lighting.min_y + 8..=lighting.min_y + 13 {
                for z in target.z * 16 + 2..=target.z * 16 + 10 {
                    for x in target.x * 16 + 2..=target.x * 16 + 10 {
                        changed |= before.block(x, y, z) != after.block(x, y, z);
                    }
                }
            }
            assert!(
                changed,
                "intersecting saved mineshaft did not change terrain"
            );
            assert!(Arc::ptr_eq(&starts[&key].source, &source));
            assert_eq!(starts[&key].original, original);
            assert_eq!(starts[&key].references, 2);
            assert!(Arc::ptr_eq(&world, &batch.terrain().snapshot()));
        }
        {
            use native_world::{
                block_light::LightBounds,
                generation_light::{LightChannel, QueuedLightEvent, QueuedSectionLightTracker},
            };
            let mut copied_biomes = actual.generator.biome_neighborhood()?;
            for (position, _) in batch.schedule.requirements() {
                if let Some(biome) = batch.biomes.chunk([position.x, position.z]) {
                    copied_biomes.insert(Arc::clone(biome))?;
                }
            }
            let copied = CapturedTerrainBatch {
                identity: Arc::clone(&batch.identity),
                schedule: batch.schedule.clone(),
                biomes: copied_biomes,
                terrain: batch.terrain.clone(),
                sources: batch.sources.clone(),
                saved_stages: batch.saved_stages.clone(),
                generated_references: BTreeMap::new(),
                generated_starts: batch.generated_starts.clone(),
                treasure_starts: batch.treasure_starts.clone(),
                feature_entity_sources: ResumedFeatureEntitySources::default(),
                revision: Arc::clone(&batch.revision),
            };
            let mut staged = StagedLiveTerrainBatch {
                batch: copied,
                feature_order: Vec::new(),
                mineshaft_starts: BTreeMap::new(),
            };
            let bounds = LightBounds::new(lighting.min_y, lighting.min_y + lighting.height)
                .context("test light bounds unavailable")?;
            let section_y = lighting.min_y.div_euclid(16);
            let mut tracker = QueuedSectionLightTracker::fresh(true, true, [])
                .context("test light tracker unavailable")?;
            tracker
                .cover_chunk_halo(second)
                .context("test halo unavailable")?;
            let registry = &actual
                .terrain_assets
                .as_ref()
                .context("terrain registry missing")?
                .blocks;
            let mut events = vec![
                QueuedLightEvent::Queue {
                    channel: LightChannel::Block,
                    position: second,
                    section_y,
                    bytes: Some(Arc::new([0x33; 2048])),
                },
                QueuedLightEvent::Queue {
                    channel: LightChannel::Sky,
                    position: target,
                    section_y,
                    bytes: Some(Arc::new([0xff; 2048])),
                },
            ];
            events.extend(
                QueuedSectionLightTracker::initialization_events(
                    &staged.batch.terrain.snapshot(),
                    second,
                    bounds,
                    |state| {
                        if !registry.is_registered(&state.name) {
                            return None;
                        }
                        let air = matches!(
                            state.name.as_str(),
                            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                        );
                        (!air || state.properties.is_empty()).then_some(air)
                    },
                )
                .context("test section registration unavailable")?,
            );
            tracker
                .apply_events(&events)
                .context("test light events failed")?;
            let sources: Arc<[i32; 256]> = Arc::from(
                native_world::sky_light::generated_chunk_sky_sources(
                    &staged.batch.terrain.snapshot(),
                    bounds,
                    second,
                )
                .context("test sky sources unavailable")?,
            );
            assert!(
                actual
                    .bind_staged_live_queued_light(
                        &staged,
                        tracker.clone(),
                        [second],
                        BTreeMap::new(),
                    )
                    .is_err()
            );
            let mut observed = actual.bind_staged_live_queued_light(
                &staged,
                tracker,
                [second],
                BTreeMap::from([(second, sources)]),
            )?;
            assert!(observed.matches_batch(&staged.batch));
            let saved_light = observed
                .engine
                .tracker()
                .data_layer(LightChannel::Block, second, section_y)
                .context("test saved light column missing")?
                .context("test saved light missing")?;
            assert_eq!(saved_light.bytes()[0], 0x33);
            assert_eq!(
                actual.run_staged_live_features_with_queued_light(
                    &mut staged,
                    target,
                    &mut observed
                )?,
                [true]
            );
            assert_eq!(
                staged.completed_stage(target),
                Some(GenerationStage::Features)
            );
            assert_eq!(staged.feature_order(), &[target]);
            assert_eq!(
                staged
                    .snapshot()
                    .block(marker[0], marker[1], marker[2])
                    .unwrap()
                    .name,
                "minecraft:melon"
            );
            assert!(observed.matches_batch(&staged.batch));
            assert!(!observed.matches_batch(&batch));
            let saved_light = observed
                .engine
                .tracker()
                .data_layer(LightChannel::Block, second, section_y)
                .context("test saved light column missing")?
                .context("test saved light missing")?;
            assert_eq!(saved_light.bytes()[0], 0x33);
            let before = staged.snapshot();
            assert!(
                actual
                    .run_staged_live_features_with_queued_light(&mut staged, target, &mut observed)
                    .is_err()
            );
            assert!(Arc::ptr_eq(&before, &staged.snapshot()));
            assert_eq!(staged.feature_order(), &[target]);
            assert!(observed.snapshot(&staged.batch).is_ok());
            assert!(observed.snapshot(&batch).is_err());
            assert!(Arc::ptr_eq(&world, &batch.terrain().snapshot()));
        }
        let mut staged_batch = StagedLiveTerrainBatch {
            batch,
            feature_order: Vec::new(),
            mineshaft_starts: BTreeMap::new(),
        };
        assert!(staged_batch.require_ready_features(target).is_ok());
        let error = actual
            .run_staged_live_features(&mut staged_batch, target)
            .unwrap_err();
        assert!(format!("{error:#}").contains("raw brightness unavailable"));
        assert!(Arc::ptr_eq(&world, &staged_batch.snapshot()));
        assert_eq!(
            staged_batch.completed_stage(target),
            Some(GenerationStage::Terrain)
        );
        assert!(staged_batch.feature_order().is_empty());
        let mut batch = staged_batch.batch;
        assert!(actual.run_captured_features(&mut batch, &context).is_err());
        assert_eq!(
            batch.schedule().completed(target),
            Some(GenerationStage::Terrain)
        );
        assert!(Arc::ptr_eq(&world, &batch.terrain().snapshot()));
        assert!(
            actual
                .bind_captured_initial_feature_light(
                    &batch,
                    native_world::generation_light::VisibleLightSnapshot::new(false, true),
                )
                .is_err()
        );
        let incomplete = actual.bind_captured_initial_feature_light(
            &batch,
            native_world::generation_light::VisibleLightSnapshot::new(true, true),
        )?;
        assert!(
            actual
                .run_captured_features_with_initial_light(&mut batch, &context, &incomplete)
                .is_err()
        );
        assert!(Arc::ptr_eq(&world, &batch.terrain().snapshot()));
        let columns = (-1..=1).flat_map(|dz| {
            (-1..=1).map(move |dx| native_world::loaded::ChunkPos {
                x: target.x + dx,
                z: target.z + dz,
            })
        });
        let visible =
            native_world::generation_light::VisibleLightSnapshot::fresh_after_section_updates(
                true,
                true,
                [],
                columns,
            )
            .context("initial light snapshot unavailable")?;
        let light = actual.bind_captured_initial_feature_light(&batch, visible)?;
        assert!(light.matches_batch(&batch));
        assert_eq!(
            actual.run_captured_features_with_initial_light(&mut batch, &context, &light)?,
            vec![true]
        );
        assert!(!light.matches_batch(&batch));
        assert_eq!(
            batch.schedule().completed(target),
            Some(GenerationStage::Features)
        );
        assert_eq!(
            batch
                .terrain()
                .snapshot()
                .block(marker[0], marker[1], marker[2])
                .unwrap()
                .name,
            "minecraft:melon"
        );
        assert!(!batch.has_entity_free_feature_coverage(target));
        assert!(
            actual
                .run_captured_features_with_initial_light(&mut batch, &context, &light)
                .is_err()
        );

        let empty = actual.begin_captured_initial_light(&batch)?;
        assert!(actual.seal_captured_light(&batch, empty).is_err());
        let mut stale = actual.begin_captured_initial_light(&batch)?;
        let mut checked_stale = false;
        for &position in &feature_positions {
            if position == target {
                continue;
            }
            let context = actual.prepare_captured_decoration(
                &batch,
                native_persistence::world::ChunkPos {
                    x: position.x,
                    z: position.z,
                },
            )?;
            let columns = (-1..=1).flat_map(|z| {
                (-1..=1).map(move |x| native_world::loaded::ChunkPos {
                    x: position.x + x,
                    z: position.z + z,
                })
            });
            let light =
                native_world::generation_light::VisibleLightSnapshot::fresh_after_section_updates(
                    true,
                    true,
                    [],
                    columns,
                )
                .context("initial light snapshot unavailable")?;
            let light = actual.bind_captured_initial_feature_light(&batch, light)?;
            let treasure_block = [-33, lighting.min_y + 8, 35];
            let before_treasure = (position == structure_target).then(|| {
                batch
                    .terrain()
                    .snapshot()
                    .block(treasure_block[0], treasure_block[1], treasure_block[2])
                    .cloned()
            });
            assert_eq!(
                actual.run_captured_features_with_initial_light(&mut batch, &context, &light)?,
                vec![true]
            );
            if let Some(before) = before_treasure {
                assert_eq!(
                    batch.terrain().snapshot().block(
                        treasure_block[0],
                        treasure_block[1],
                        treasure_block[2]
                    ),
                    before.as_ref()
                );
            }
            if !checked_stale {
                assert!(
                    actual
                        .initialize_captured_light(&mut batch, &mut stale, target)
                        .is_err()
                );
                assert_eq!(
                    batch.schedule().completed(target),
                    Some(GenerationStage::Features)
                );
                checked_stale = true;
            }
        }
        assert!(batch.has_entity_free_feature_coverage(target));
        let mut initial = actual.begin_captured_initial_light(&batch)?;
        for &position in &light_positions {
            actual.initialize_captured_light(&mut batch, &mut initial, position)?;
        }
        assert!(
            actual
                .initialize_captured_light(&mut batch, &mut initial, target)
                .is_err()
        );
        let mut staged = actual.seal_captured_light(&batch, initial)?;
        assert!(
            actual
                .draft_captured_saved_lit_chunk(&batch, &staged, saved_second)
                .is_err()
        );
        assert!(
            actual
                .propagate_captured_light(
                    &mut batch,
                    &mut staged,
                    native_world::loaded::ChunkPos {
                        x: second.x + 100,
                        z: second.z
                    }
                )
                .is_err()
        );
        let before = batch.schedule().completed(second);
        assert!(
            crate::chunk_stream::propagate_captured_light_sequence(
                &actual,
                &mut batch,
                &mut staged,
                &[second],
                || anyhow::bail!("cancelled")
            )
            .is_err()
        );
        assert_eq!(batch.schedule().completed(second), before);
        let player = native_session::PlayerState {
            position: glam::Vec3::new(8.5, lighting.min_y as f32 + 2.0, 40.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let mut worker_session = native_session::loaded::LoadedSession::new(
            restored.world().clone(),
            glam::IVec3::ZERO,
            native_session::loaded::BuildBounds {
                min_y: lighting.min_y,
                max_y: lighting.min_y + lighting.height,
            },
            player,
        )
        .map_err(anyhow::Error::msg)?;
        for pos in [second, third] {
            let poi = if pos == second {
                native_session::loaded::SavedPoiData::Present(Arc::clone(&existing_poi_source))
            } else {
                native_session::loaded::SavedPoiData::Missing
            };
            worker_session.set_initial_poi_chunk(pos, poi)?;
            let entities = if pos == second {
                native_session::loaded::SavedEntityData::Present(Arc::clone(
                    &existing_entity_source,
                ))
            } else {
                native_session::loaded::SavedEntityData::Empty
            };
            worker_session.set_initial_entity_chunk(pos, entities)?;
        }
        let (streamer, initialized) = crate::chunk_stream::light_worker_test_fixture(
            actual,
            batch,
            restored,
            staged,
            &worker_session,
            reader
                .session_guard()
                .context("world session lock missing")?,
            target,
        )?;
        let mut order = vec![second, third, neighbor, structure_target];
        order.extend(support);
        let mut worker = crate::chunk_stream::MissingGenerationPropagationWorker::start(
            initialized,
            &streamer,
            &worker_session,
            order.clone(),
        )
        .map_err(|failure| failure.error)?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(240);
        let propagated = loop {
            if let Some(result) = worker.try_result(&streamer, &worker_session)? {
                break result;
            }
            ensure!(
                std::time::Instant::now() < deadline,
                "light propagation timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        assert_eq!(propagated.positions(), order);
        let initialized = propagated.into_checked_light(&streamer, &worker_session)?;
        let (assembled, mut staged) = initialized.into_checked_parts(&streamer, &worker_session)?;
        let (mut batch, mut actual, mut restored, _evaluator, _observed, _live) =
            assembled.into_checked_parts(&streamer, &worker_session)?;
        drop(worker);
        drop(streamer);
        let visible = actual.propagate_captured_light(&mut batch, &mut staged, target)?;
        assert!(visible.raw_brightness(marker, 0).is_some());
        assert!(staged.activated.contains(&target));
        assert_eq!(
            batch.schedule().completed(target),
            Some(GenerationStage::Light)
        );
        let clocks: BTreeMap<_, _> =
            native_resources::world_clock::registered_clocks(&mut actual.resources)?
                .into_iter()
                .map(|id| (id, 0))
                .collect();
        let spawn_schedule = batch.schedule.clone();
        let mut worker_spawn_document = None;
        for spawn_mobs in [true, false] {
            let before_spawn = batch.terrain.snapshot();
            let inputs = crate::chunk_stream::MissingGenerationSpawnInputs {
                spawn_mobs,
                clocks: clocks.clone(),
                border: native_session::respawn::RespawnBorder {
                    min_x: -1000.0,
                    max_x: 1000.0,
                    min_z: -1000.0,
                    max_z: 1000.0,
                },
                last_update: 42,
            };
            let (streamer, initialized) = crate::chunk_stream::light_worker_test_fixture(
                actual,
                batch,
                restored,
                staged,
                &worker_session,
                reader
                    .session_guard()
                    .context("world session lock missing")?,
                target,
            )?;
            let rejected = match crate::chunk_stream::MissingGenerationSpawnWorker::start(
                initialized,
                &streamer,
                &worker_session,
                vec![target, target],
                inputs.clone(),
            ) {
                Ok(_) => bail!("duplicate spawn positions were accepted"),
                Err(error) => error,
            };
            assert!(rejected.error.to_string().contains("distinct"));
            let mut spawn_positions = vec![target, second];
            if !spawn_mobs {
                spawn_positions.extend([third, neighbor]);
                spawn_positions.extend(support);
            }
            let mut worker = crate::chunk_stream::MissingGenerationSpawnWorker::start(
                rejected.initialized,
                &streamer,
                &worker_session,
                spawn_positions.clone(),
                inputs.clone(),
            )
            .map_err(|failure| failure.error)?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
            let spawned = loop {
                if let Some(result) = worker.try_result(&streamer, &worker_session, &inputs)? {
                    break result;
                }
                ensure!(
                    std::time::Instant::now() < deadline,
                    "spawn generation timed out"
                );
                std::thread::sleep(std::time::Duration::from_millis(2));
            };
            assert_eq!(spawned.spawns().len(), spawn_positions.len());
            assert_eq!(spawned.spawns()[0].position(), target);
            assert_eq!(spawned.spawns()[1].position(), second);
            assert_eq!(
                spawned.spawns()[0].document().value.get("Status"),
                Some(&Tag::String("minecraft:spawn".into()))
            );
            assert_eq!(
                spawned.spawns()[1].document().value.get("example:root"),
                Some(&Tag::Int(73))
            );
            let mut stale = inputs.clone();
            stale.last_update += 1;
            assert!(!spawned.is_current(&streamer, &worker_session, &stale));
            if spawn_mobs {
                assert!(matches!(
                    spawned.spawns()[0],
                    CapturedCompletedSpawn::Planned(_)
                ));
                let Some(Tag::List { values, .. }) =
                    spawned.spawns()[0].document().value.get("entities")
                else {
                    bail!("worker did not generate any creatures")
                };
                assert!(values.iter().any(|value| {
                    value.get("id") == Some(&Tag::String("minecraft:pig".into()))
                }));
                worker_spawn_document = Some(spawned.spawns()[0].document().clone());
            } else {
                assert!(matches!(
                    spawned.spawns()[0],
                    CapturedCompletedSpawn::Empty(_)
                ));
            }
            let post_worker_inputs = crate::chunk_stream::MissingGenerationPostprocessingInputs {
                spawn: inputs.clone(),
                game_time: 42,
                next_sub_tick: 9,
                water_source_conversion: true,
            };
            let mut stale_post_inputs = post_worker_inputs.clone();
            stale_post_inputs.spawn.last_update += 1;
            let rejected = match crate::chunk_stream::MissingGenerationPostprocessingWorker::start(
                spawned,
                &streamer,
                &worker_session,
                vec![target],
                stale_post_inputs,
            ) {
                Ok(_) => bail!("stale spawn inputs were accepted for postprocessing"),
                Err(failure) => failure,
            };
            assert!(rejected.error.to_string().contains("stale"));
            let rejected = match crate::chunk_stream::MissingGenerationPostprocessingWorker::start(
                rejected.spawned,
                &streamer,
                &worker_session,
                vec![target, target],
                post_worker_inputs.clone(),
            ) {
                Ok(_) => bail!("duplicate postprocessing targets were accepted"),
                Err(failure) => failure,
            };
            assert!(rejected.error.to_string().contains("distinct"));
            let rejected = match crate::chunk_stream::MissingGenerationPostprocessingWorker::start(
                rejected.spawned,
                &streamer,
                &worker_session,
                vec![structure_target],
                post_worker_inputs.clone(),
            ) {
                Ok(_) => bail!("an uncaptured postprocessing target was accepted"),
                Err(failure) => failure,
            };
            assert!(rejected.error.to_string().contains("no captured spawn"));
            let mut post_worker =
                crate::chunk_stream::MissingGenerationPostprocessingWorker::start(
                    rejected.spawned,
                    &streamer,
                    &worker_session,
                    vec![target, second],
                    post_worker_inputs.clone(),
                )
                .map_err(|failure| failure.error)?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
            let result = loop {
                if let Some(result) =
                    post_worker.try_result(&streamer, &worker_session, &post_worker_inputs)?
                {
                    break result;
                }
                ensure!(
                    std::time::Instant::now() < deadline,
                    "postprocessing timed out"
                );
                std::thread::sleep(std::time::Duration::from_millis(2));
            };
            assert!(
                post_worker
                    .try_result(&streamer, &worker_session, &post_worker_inputs)?
                    .is_none()
            );
            let (spawned, completed) = match result {
                crate::chunk_stream::MissingGenerationPostprocessingResult::NeedsChunk {
                    spawned,
                    position,
                } if spawn_mobs => {
                    assert_eq!(position, neighbor);
                    let mut cancelled =
                        crate::chunk_stream::MissingGenerationPostprocessingWorker::start(
                            spawned,
                            &streamer,
                            &worker_session,
                            vec![target, second],
                            post_worker_inputs.clone(),
                        )
                        .map_err(|failure| failure.error)?;
                    cancelled.cancel();
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
                    let spawned = loop {
                        match cancelled.try_result(&streamer, &worker_session, &post_worker_inputs)? {
                            Some(crate::chunk_stream::MissingGenerationPostprocessingResult::Cancelled { spawned }) => break spawned,
                            Some(_) => bail!("cancelled postprocessing returned another outcome"),
                            None => {}
                        }
                        ensure!(
                            std::time::Instant::now() < deadline,
                            "postprocessing cancellation timed out"
                        );
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    };
                    (spawned, None)
                }
                crate::chunk_stream::MissingGenerationPostprocessingResult::Completed(
                    completed,
                ) if !spawn_mobs => {
                    assert_eq!(completed.targets(), &[target, second]);
                    let mut stale = post_worker_inputs.clone();
                    stale.next_sub_tick += 1;
                    assert!(!completed.is_current(&streamer, &worker_session, &stale));
                    stale = post_worker_inputs.clone();
                    stale.game_time += 1;
                    assert!(!completed.is_current(&streamer, &worker_session, &stale));
                    stale = post_worker_inputs.clone();
                    stale.water_source_conversion = !stale.water_source_conversion;
                    assert!(!completed.is_current(&streamer, &worker_session, &stale));
                    let (spawned, completed) = completed.into_checked_parts(
                        &streamer,
                        &worker_session,
                        &post_worker_inputs,
                    )?;
                    (spawned, Some(completed))
                }
                crate::chunk_stream::MissingGenerationPostprocessingResult::Failed {
                    error,
                    ..
                } => {
                    return Err(error.context("postprocessing worker failed"));
                }
                _ => bail!("unexpected postprocessing outcome"),
            };
            drop(post_worker);
            let (initialized, spawns) =
                spawned.into_checked_parts(&streamer, &worker_session, &inputs)?;
            let (assembled, next_staged) =
                initialized.into_checked_parts(&streamer, &worker_session)?;
            let (next_batch, next_actual, next_restored, _, _, _) =
                assembled.into_checked_parts(&streamer, &worker_session)?;
            actual = next_actual;
            batch = next_batch;
            restored = next_restored;
            staged = next_staged;
            assert!(Arc::ptr_eq(&before_spawn, &batch.terrain.snapshot()));
            assert_eq!(
                batch.schedule.completed(target),
                Some(GenerationStage::Spawn)
            );
            let post_inputs = CapturedPostprocessingInputs {
                spawn_mobs,
                clocks: &clocks,
                border: Some(inputs.border),
                game_time: 42,
                next_sub_tick: 9,
                water_source_conversion: true,
            };
            let converted = actual.begin_captured_mixed_postprocessing_region(
                &batch,
                &staged,
                spawns
                    .iter()
                    .map(CapturedCompletedSpawn::as_postprocessing_spawn),
                post_inputs,
            )?;
            assert!(converted.matches_state(&batch, &staged, post_inputs));
            if let Some(completed) = completed {
                let mut baseline = actual.run_captured_mixed_postprocessing(
                    &batch,
                    &staged,
                    converted,
                    [target, second],
                    post_inputs,
                )?;
                for position in [target, second] {
                    loop {
                        match baseline.try_poll(&batch, &staged, post_inputs, position, 64)? {
                            native_session::worldgen::PostprocessingAttempt::Progress {
                                complete: true,
                            } => break,
                            native_session::worldgen::PostprocessingAttempt::Progress {
                                complete: false,
                            } => {}
                            native_session::worldgen::PostprocessingAttempt::NeedsChunk(
                                missing,
                            ) => {
                                bail!("baseline needs unprepared neighbor {missing:?}")
                            }
                        }
                    }
                }
                actual.drain_captured_mixed_postprocessing_light(
                    &batch,
                    &staged,
                    &mut baseline,
                    post_inputs,
                )?;
                let baseline = baseline.finish(&batch, &staged, post_inputs)?;
                assert_eq!(completed.region().chunks, baseline.region().chunks);
                assert_eq!(completed.region().edits, baseline.region().edits);
                assert_eq!(
                    completed.region().ticks().activated_snapshot(),
                    baseline.region().ticks().activated_snapshot()
                );
                assert_eq!(
                    completed.region().deferred_chunks().collect::<Vec<_>>(),
                    baseline.region().deferred_chunks().collect::<Vec<_>>()
                );
                for position in [target, second] {
                    assert_eq!(
                        completed.region().heightmaps()[&position].to_tag()?,
                        baseline.region().heightmaps()[&position].to_tag()?
                    );
                    assert_eq!(
                        completed.region().recorded_entity_candidates(position),
                        baseline.region().recorded_entity_candidates(position)
                    );
                }
                assert_eq!(
                    completed.region().light_checks,
                    baseline.region().light_checks
                );
                let actual_sections = actual.captured_mixed_postprocessed_sections(
                    &batch,
                    &staged,
                    &completed,
                    [target, second],
                    post_inputs,
                )?;
                let expected_sections = actual.captured_mixed_postprocessed_sections(
                    &batch,
                    &staged,
                    &baseline,
                    [target, second],
                    post_inputs,
                )?;
                assert_eq!(actual_sections.sections(), expected_sections.sections());
            }
            batch.schedule = spawn_schedule.clone();
            for position in spawn_positions {
                staged.completed_creature_spawns.remove(&position);
                staged.completed_empty_spawns.remove(&position);
            }
            drop(worker);
            drop(streamer);
        }
        let mut captured_plan =
            actual.plan_captured_creatures(&batch, &staged, target, true, &clocks)?;
        let x = target.x * 16;
        let z = target.z * 16;
        let y = lighting.min_y + lighting.height - 1;
        let spawn_biome = batch.biomes.biome_at_block(x, y, z)?.clone();
        let probability_biome = batch.biomes.noise_biome_at_block(x, y, z)?.clone();
        let (environment, categories) = actual.creature_environment.as_ref().unwrap();
        let layers = environment.sample_layers(|id| clocks.get(id).copied())?;
        assert!(layers.is_empty());
        let spawn = native_resources::mob_spawns::load_biome_spawn_attributes(
            &mut actual.resources,
            &spawn_biome,
        )?;
        let probability = native_resources::mob_spawns::load_biome_spawn_attributes(
            &mut actual.resources,
            &probability_biome,
        )?;
        let settings = native_resources::mob_spawns::resolve_creature_spawns(
            environment.dimension_attributes(),
            &spawn,
            &probability,
            &layers,
            |id| categories.is_misc(id),
        )?;
        let mut expected = native_resources::worldgen_mob_spawner::ChunkCreatureSpawner::new(
            actual.context.inputs().settings().seed(),
            target.x,
            target.z,
            true,
            settings.probability,
            settings.spawns,
        )?;
        assert!(captured_plan.matches_state(&batch, &staged, true, &clocks));
        assert!(!captured_plan.matches_state(&batch, &staged, false, &clocks));
        let mut wrong_clocks = clocks.clone();
        wrong_clocks.insert(ResourceId::parse("example:changed_clock")?, 1);
        assert!(
            captured_plan
                .advance(&batch, &staged, true, &wrong_clocks)
                .is_err()
        );
        assert_eq!(
            captured_plan.advance(&batch, &staged, true, &clocks)?,
            expected.next(1)?
        );
        let attrs = json!({"gameplay/natural_mob_spawns":{"spawns_by_category":{"creature":[{"type":"minecraft:pig","weight":1,"count":1}]},"spawn_costs":{}}});
        let forced = native_resources::mob_spawns::resolve_creature_spawns(
            &json!({}),
            &attrs,
            &json!({}),
            &[],
            |_| Some(false),
        )?;
        let mut pending = actual.plan_captured_creatures(&batch, &staged, target, true, &clocks)?;
        pending.inner.planner = native_resources::worldgen_mob_spawner::ChunkCreatureSpawner::new(
            42,
            target.x,
            target.z,
            true,
            0.999_999_9_f32,
            forced.spawns.clone(),
        )?;
        let attempt = pending.advance(&batch, &staged, true, &clocks)?;
        assert!(matches!(
            attempt,
            native_resources::worldgen_mob_spawner::CreatureSpawnProgress::Attempt(_)
        ));
        assert_eq!(pending.advance(&batch, &staged, true, &clocks)?, attempt);
        let excluded = native_session::respawn::RespawnBorder {
            min_x: 1000.0,
            max_x: 1001.0,
            min_z: 1000.0,
            max_z: 1001.0,
        };
        assert!(
            actual
                .check_captured_creature_candidate(
                    &batch,
                    &staged,
                    &mut pending,
                    true,
                    &wrong_clocks,
                    excluded,
                    64,
                )
                .is_err()
        );
        assert_eq!(pending.advance(&batch, &staged, true, &clocks)?, attempt);
        assert_eq!(
            actual.check_captured_creature_candidate(
                &batch,
                &staged,
                &mut pending,
                true,
                &clocks,
                excluded,
                0,
            )?,
            ResumedCreatureCheck::Pending
        );
        assert_eq!(pending.advance(&batch, &staged, true, &clocks)?, attempt);
        let changed_border = native_session::respawn::RespawnBorder {
            min_x: -1000.0,
            max_x: 1000.0,
            min_z: -1000.0,
            max_z: 1000.0,
        };
        assert!(
            actual
                .check_captured_creature_candidate(
                    &batch,
                    &staged,
                    &mut pending,
                    true,
                    &clocks,
                    changed_border,
                    64,
                )
                .is_err()
        );
        let mut rejected = false;
        for _ in 0..16 {
            match actual.check_captured_creature_candidate(
                &batch,
                &staged,
                &mut pending,
                true,
                &clocks,
                excluded,
                64,
            )? {
                ResumedCreatureCheck::Pending => {}
                ResumedCreatureCheck::Rejected => {
                    rejected = true;
                    break;
                }
                ResumedCreatureCheck::NeedsConstruction(_) => {
                    bail!("out-of-border creature was accepted")
                }
            }
        }
        assert!(
            rejected,
            "excluded captured candidate did not finish placement"
        );
        let mut animal = actual.plan_captured_creatures(&batch, &staged, target, true, &clocks)?;
        animal.inner.planner = native_resources::worldgen_mob_spawner::ChunkCreatureSpawner::new(
            42,
            target.x,
            target.z,
            true,
            0.999_999_9_f32,
            forced.spawns,
        )?;
        let mut constructed = false;
        for sequence in 0..512 {
            match animal.advance(&batch, &staged, true, &clocks)? {
                native_resources::worldgen_mob_spawner::CreatureSpawnProgress::Finished => break,
                native_resources::worldgen_mob_spawner::CreatureSpawnProgress::Pending => continue,
                native_resources::worldgen_mob_spawner::CreatureSpawnProgress::Attempt(_) => {}
            }
            let check = actual.check_captured_creature_candidate(
                &batch,
                &staged,
                &mut animal,
                true,
                &clocks,
                changed_border,
                64,
            )?;
            match check {
                ResumedCreatureCheck::Pending => continue,
                ResumedCreatureCheck::Rejected => continue,
                ResumedCreatureCheck::NeedsConstruction(_) => {}
            }
            let construction = native_session::worldgen::FreshCreatureConstruction {
                uuid: [42, 43, 44, sequence],
                chicken_egg_time: None,
            };
            let invalid = native_session::worldgen::FreshCreatureConstruction {
                chicken_egg_time: Some(9000),
                ..construction
            };
            assert!(
                actual
                    .construct_captured_creature(
                        &batch,
                        &staged,
                        &mut animal,
                        true,
                        &clocks,
                        changed_border,
                        invalid,
                    )
                    .is_err()
            );
            assert!(
                actual
                    .construct_captured_creature(
                        &batch,
                        &staged,
                        &mut animal,
                        true,
                        &wrong_clocks,
                        changed_border,
                        construction,
                    )
                    .is_err()
            );
            actual.check_terrain_entity_id(
                &batch.terrain,
                &batch.revision,
                batch
                    .sources
                    .keys()
                    .map(|pos| native_world::loaded::ChunkPos { x: pos.x, z: pos.z }),
                construction.uuid,
            )?;
            actual
                .captured_entity_ids
                .as_mut()
                .unwrap()
                .0
                .insert(construction.uuid);
            assert!(
                actual
                    .construct_captured_creature(
                        &batch,
                        &staged,
                        &mut animal,
                        true,
                        &clocks,
                        changed_border,
                        construction,
                    )
                    .is_err()
            );
            actual
                .captured_entity_ids
                .as_mut()
                .unwrap()
                .0
                .remove(&construction.uuid);
            let pose = actual.construct_captured_creature(
                &batch,
                &staged,
                &mut animal,
                true,
                &clocks,
                changed_border,
                construction,
            )?;
            assert!(
                actual
                    .construct_captured_creature(
                        &batch,
                        &staged,
                        &mut animal,
                        true,
                        &clocks,
                        changed_border,
                        construction,
                    )
                    .is_err()
            );
            assert!(
                actual
                    .check_captured_creature_instance(
                        &batch,
                        &staged,
                        &mut animal,
                        true,
                        &wrong_clocks,
                        changed_border,
                    )
                    .is_err()
            );
            match actual.check_captured_creature_instance(
                &batch,
                &staged,
                &mut animal,
                true,
                &clocks,
                changed_border,
            )? {
                ResumedCreatureInstance::Rejected => {
                    assert!(animal.inner.probe.is_none());
                    assert!(animal.inner.constructed.is_none());
                    continue;
                }
                ResumedCreatureInstance::NeedsFinalization(actual_pose) => {
                    assert_eq!(actual_pose, pose);
                    assert_eq!(
                        actual.check_captured_creature_instance(
                            &batch,
                            &staged,
                            &mut animal,
                            true,
                            &clocks,
                            changed_border,
                        )?,
                        ResumedCreatureInstance::NeedsFinalization(pose)
                    );
                }
            }
            assert!(pose.geometry.position.iter().all(|v| v.is_finite()));
            assert!(
                actual
                    .finalize_captured_creature(
                        &batch,
                        &staged,
                        &mut animal,
                        true,
                        &wrong_clocks,
                        changed_border,
                    )
                    .is_err()
            );
            assert!(animal.inner.additions.is_empty());
            actual.finalize_captured_creature(
                &batch,
                &staged,
                &mut animal,
                true,
                &clocks,
                changed_border,
            )?;
            assert_eq!(animal.inner.additions.len(), 1);
            let native_session::worldgen::GeneratedEntityRecord::Saved(record) =
                &animal.inner.additions[0].record
            else {
                bail!("finalized creature record missing")
            };
            assert_eq!(record.get("id"), Some(&Tag::String("minecraft:pig".into())));
            assert_eq!(
                record.get("UUID"),
                Some(&Tag::IntArray(construction.uuid.to_vec()))
            );
            assert!(animal.inner.probe.is_none());
            assert!(
                actual
                    .finalize_captured_creature(
                        &batch,
                        &staged,
                        &mut animal,
                        true,
                        &clocks,
                        changed_border,
                    )
                    .is_err()
            );
            constructed = true;
            break;
        }
        assert!(constructed, "no captured creature reached construction");
        let saved_schedule = batch.schedule.clone();
        assert!(
            actual
                .finish_captured_creatures(
                    &mut batch,
                    &mut staged,
                    &animal,
                    true,
                    &clocks,
                    Some(changed_border),
                    42,
                )
                .is_err()
        );
        assert_eq!(
            batch.schedule.completed(target),
            Some(GenerationStage::Light)
        );
        // Isolate draft assembly from the remaining deterministic spawn stream.
        animal.inner.planner = native_resources::worldgen_mob_spawner::ChunkCreatureSpawner::new(
            42,
            target.x,
            target.z,
            true,
            0.0,
            Default::default(),
        )?;
        assert_eq!(
            animal.advance(&batch, &staged, true, &clocks)?,
            native_resources::worldgen_mob_spawner::CreatureSpawnProgress::Finished,
        );
        assert!(
            actual
                .finish_captured_creatures(
                    &mut batch,
                    &mut staged,
                    &animal,
                    true,
                    &wrong_clocks,
                    Some(changed_border),
                    42,
                )
                .is_err()
        );
        animal.inner.additions[0].block_position[0] += 16;
        assert!(
            actual
                .finish_captured_creatures(
                    &mut batch,
                    &mut staged,
                    &animal,
                    true,
                    &clocks,
                    Some(changed_border),
                    42,
                )
                .is_err()
        );
        animal.inner.additions[0].block_position[0] -= 16;
        assert_eq!(
            batch.schedule.completed(target),
            Some(GenerationStage::Light)
        );
        let spawn = actual.finish_captured_creatures(
            &mut batch,
            &mut staged,
            &animal,
            true,
            &clocks,
            Some(changed_border),
            42,
        )?;
        assert!(spawn.matches_state(&batch, &staged, true, &clocks, Some(changed_border)));
        assert!(!spawn.matches_state(&batch, &staged, false, &clocks, Some(changed_border)));
        assert!(!spawn.matches_state(&batch, &staged, true, &wrong_clocks, Some(changed_border)));
        assert!(spawn.matches_source(None)?);
        assert!(!spawn.matches_source(Some(&saved))?);
        assert!(spawn.entity_sidecar_source().is_none());
        assert!(spawn.has_entity_free_feature_coverage());
        assert!(
            actual
                .convert_captured_planned_heightmaps(
                    &batch,
                    &staged,
                    &spawn,
                    &wrong_clocks,
                    Some(changed_border),
                )
                .is_err()
        );
        let proof = actual.convert_captured_planned_heightmaps(
            &batch,
            &staged,
            &spawn,
            &clocks,
            Some(changed_border),
        )?;
        assert!(proof.matches_state(&batch, &staged, true, &clocks, Some(changed_border)));
        assert!(!proof.matches_state(&batch, &staged, true, &wrong_clocks, Some(changed_border)));
        assert!(!proof.matches_state(&batch, &staged, true, &clocks, None));
        assert!(!proof.matches_state(&batch, &staged, false, &clocks, Some(changed_border)));
        assert!(proof.matches_source(None)?);
        assert!(proof.other_fields().is_empty());
        let mut wrong_proof = proof.clone();
        wrong_proof.completion = Arc::new(());
        assert!(
            actual
                .begin_captured_planned_postprocessing(
                    &batch,
                    &staged,
                    &spawn,
                    wrong_proof,
                    &clocks,
                    Some(changed_border),
                )
                .is_err()
        );
        let converted = actual.begin_captured_planned_postprocessing(
            &batch,
            &staged,
            &spawn,
            proof.clone(),
            &clocks,
            Some(changed_border),
        )?;
        assert!(converted.matches_state(&batch, &staged, true, &clocks, Some(changed_border)));
        assert!(!converted.matches_state(
            &batch,
            &staged,
            true,
            &wrong_clocks,
            Some(changed_border)
        ));
        let (_, converted) = converted.into_parts();
        assert!(
            converted
                .ticks
                .blocks
                .pack(42)
                .iter()
                .any(|tick| tick.kind == "minecraft:melon")
        );
        assert_eq!(
            spawn.document().value.get("Status"),
            Some(&Tag::String("minecraft:spawn".into()))
        );
        let Some(Tag::List { values, .. }) = spawn.document().value.get("entities") else {
            bail!("captured creature records missing")
        };
        assert!(
            values
                .iter()
                .any(|tag| tag.get("id") == Some(&Tag::String("minecraft:pig".into())))
        );
        assert!(
            actual
                .draft_captured_empty_spawn(&batch, &staged, saved_target, true, 42)
                .is_err()
        );
        assert!(
            actual
                .finish_captured_creatures(
                    &mut batch,
                    &mut staged,
                    &animal,
                    true,
                    &clocks,
                    Some(changed_border),
                    42,
                )
                .is_err()
        );
        let mut saved_animal =
            actual.plan_captured_creatures(&batch, &staged, second, true, &clocks)?;
        saved_animal.inner.planner =
            native_resources::worldgen_mob_spawner::ChunkCreatureSpawner::new(
                42,
                second.x,
                second.z,
                true,
                0.0,
                Default::default(),
            )?;
        assert_eq!(
            saved_animal.advance(&batch, &staged, true, &clocks)?,
            native_resources::worldgen_mob_spawner::CreatureSpawnProgress::Finished,
        );
        let saved_spawn = actual.finish_captured_creatures(
            &mut batch,
            &mut staged,
            &saved_animal,
            true,
            &clocks,
            None,
            42,
        )?;
        assert!(saved_spawn.matches_state(&batch, &staged, true, &clocks, None));
        assert!(saved_spawn.matches_source(Some(&saved))?);
        assert_eq!(
            saved_spawn.entity_sidecar_source(),
            Some(existing_entity_source.document.as_ref())
        );
        assert!(saved_spawn.has_entity_free_feature_coverage());
        let saved_proof = actual.convert_captured_planned_heightmaps(
            &batch,
            &staged,
            &saved_spawn,
            &clocks,
            None,
        )?;
        assert!(saved_proof.matches_source(Some(&saved))?);
        assert!(
            saved_proof
                .other_fields()
                .iter()
                .any(|(name, value)| name.matches("example:map") && *value == Tag::Int(37))
        );
        let saved_converted = actual.begin_captured_planned_postprocessing(
            &batch,
            &staged,
            &saved_spawn,
            saved_proof.clone(),
            &clocks,
            None,
        )?;
        assert!(saved_converted.matches_state(&batch, &staged, true, &clocks, None));
        assert_eq!(
            saved_spawn.document().value.get("example:root"),
            Some(&Tag::Int(73))
        );
        let mut additional_spawns = Vec::new();
        for position in [third, neighbor].into_iter().chain(support) {
            let mut plan =
                actual.plan_captured_creatures(&batch, &staged, position, true, &clocks)?;
            plan.inner.planner = native_resources::worldgen_mob_spawner::ChunkCreatureSpawner::new(
                42,
                position.x,
                position.z,
                true,
                0.0,
                Default::default(),
            )?;
            assert_eq!(
                plan.advance(&batch, &staged, true, &clocks)?,
                native_resources::worldgen_mob_spawner::CreatureSpawnProgress::Finished
            );
            additional_spawns.push(actual.finish_captured_creatures(
                &mut batch,
                &mut staged,
                &plan,
                true,
                &clocks,
                None,
                42,
            )?);
        }
        let inputs = CapturedPostprocessingInputs {
            spawn_mobs: true,
            clocks: &clocks,
            border: Some(changed_border),
            game_time: 42,
            next_sub_tick: 9,
            water_source_conversion: true,
        };
        let wrong_inputs = CapturedPostprocessingInputs {
            clocks: &wrong_clocks,
            ..inputs
        };
        assert!(
            actual
                .begin_captured_mixed_postprocessing_region(
                    &batch,
                    &staged,
                    [CapturedPostprocessingSpawn::Planned(&spawn)],
                    wrong_inputs,
                )
                .is_err()
        );
        assert!(
            actual
                .begin_captured_mixed_postprocessing_region(
                    &batch,
                    &staged,
                    [
                        CapturedPostprocessingSpawn::Planned(&spawn),
                        CapturedPostprocessingSpawn::Planned(&spawn)
                    ],
                    inputs,
                )
                .is_err()
        );
        let region = actual.begin_captured_mixed_postprocessing_region(
            &batch,
            &staged,
            [
                CapturedPostprocessingSpawn::Planned(&spawn),
                CapturedPostprocessingSpawn::Planned(&saved_spawn),
                CapturedPostprocessingSpawn::Planned(&additional_spawns[0]),
            ],
            inputs,
        )?;
        assert!(
            region.entity_candidates[&target]
                .as_ref()
                .context("captured candidates missing")?
                .iter()
                .any(|record| record.get("id") == Some(&Tag::String("minecraft:pig".into())))
        );
        assert!(region.entity_candidates[&second].as_ref().is_some_and(
            |records| records.as_slice() == [existing_entity.clone(), proto_entity.clone()]
        ));
        let before_pass = batch.terrain.snapshot();
        let mut pass =
            actual.run_captured_mixed_postprocessing(&batch, &staged, region, [target], inputs)?;
        assert!(
            pass.stage
                .recorded_entity_candidates(target)
                .flatten()
                .context("stage candidates missing")?
                .iter()
                .any(|record| record.get("id") == Some(&Tag::String("minecraft:pig".into())))
        );
        assert!(matches!(
            pass.stage.recorded_entity_candidates(neighbor),
            None
        ));
        assert!(pass.matches_state(&batch, &staged, inputs));
        assert!(!pass.matches_state(&batch, &staged, wrong_inputs));
        assert!(
            pass.try_poll(&batch, &staged, wrong_inputs, target, 1)
                .is_err()
        );
        pass.request_converted_chunk(&batch, &staged, inputs, second)?;
        pass.request_converted_chunk(&batch, &staged, inputs, third)?;
        assert_eq!(
            pass.try_poll(&batch, &staged, inputs, target, 1)?,
            native_session::worldgen::PostprocessingAttempt::NeedsChunk(neighbor)
        );
        assert_eq!(pass.waiting_neighbor(), Some(neighbor));
        assert!(
            actual
                .add_captured_mixed_postprocessing_neighbor(
                    &batch,
                    &staged,
                    &mut pass,
                    CapturedPostprocessingSpawn::Planned(&additional_spawns[1]),
                    wrong_inputs,
                )
                .is_err()
        );
        actual.add_captured_mixed_postprocessing_neighbor(
            &batch,
            &staged,
            &mut pass,
            CapturedPostprocessingSpawn::Planned(&additional_spawns[1]),
            inputs,
        )?;
        assert_eq!(pass.waiting_neighbor(), None);
        assert!(
            pass.stage
                .recorded_entity_candidates(neighbor)
                .flatten()
                .is_some()
        );
        assert!(pass.try_poll(&batch, &staged, inputs, second, 1).is_err());
        let retry = pass.try_poll(&batch, &staged, inputs, target, 1)?;
        assert_ne!(
            retry,
            native_session::worldgen::PostprocessingAttempt::NeedsChunk(neighbor)
        );
        let baseline_region = actual.begin_captured_mixed_postprocessing_region(
            &batch,
            &staged,
            [
                CapturedPostprocessingSpawn::Planned(&spawn),
                CapturedPostprocessingSpawn::Planned(&saved_spawn),
                CapturedPostprocessingSpawn::Planned(&additional_spawns[0]),
                CapturedPostprocessingSpawn::Planned(&additional_spawns[1]),
            ],
            inputs,
        )?;
        let mut baseline = actual.run_captured_mixed_postprocessing(
            &batch,
            &staged,
            baseline_region,
            [target, second, third, neighbor],
            inputs,
        )?;
        assert_eq!(
            retry,
            baseline.try_poll(&batch, &staged, inputs, target, 1)?
        );
        let complete_region = actual.begin_captured_mixed_postprocessing_region(
            &batch,
            &staged,
            [
                CapturedPostprocessingSpawn::Planned(&spawn),
                CapturedPostprocessingSpawn::Planned(&saved_spawn),
            ]
            .into_iter()
            .chain(
                additional_spawns
                    .iter()
                    .map(CapturedPostprocessingSpawn::Planned),
            ),
            inputs,
        )?;
        let mut complete_stage = actual.run_captured_mixed_postprocessing(
            &batch,
            &staged,
            complete_region,
            [target, second],
            inputs,
        )?;
        for position in [target, second] {
            let mut done = false;
            for _ in 0..1024 {
                match complete_stage.try_poll(&batch, &staged, inputs, position, 64)? {
                    native_session::worldgen::PostprocessingAttempt::Progress { complete } => {
                        if complete {
                            done = true;
                            break;
                        }
                    }
                    native_session::worldgen::PostprocessingAttempt::NeedsChunk(missing) => {
                        bail!("postprocessed chunk needs unprepared neighbor {missing:?}")
                    }
                }
            }
            assert!(done, "chunk postprocessing did not finish for {position:?}");
        }
        actual.drain_captured_mixed_postprocessing_light(
            &batch,
            &staged,
            &mut complete_stage,
            inputs,
        )?;
        let completed = complete_stage.finish(&batch, &staged, inputs)?;
        assert!(completed.matches_state(&batch, &staged, inputs));
        assert!(!completed.matches_state(&batch, &staged, wrong_inputs));
        assert!(completed.region().chunks.contains(&target));
        assert!(
            completed
                .region()
                .recorded_entity_candidates(target)
                .flatten()
                .context("completed pig candidates missing")?
                .iter()
                .any(|record| record.get("id") == Some(&Tag::String("minecraft:pig".into())))
        );
        assert!(
            actual
                .resume_captured_mixed_postprocessing(&batch, &staged, &completed, wrong_inputs)
                .is_err()
        );
        let resumed =
            actual.resume_captured_mixed_postprocessing(&batch, &staged, &completed, inputs)?;
        assert!(resumed.matches_state(&batch, &staged, inputs));
        assert!(
            resumed
                .stage
                .recorded_entity_candidates(target)
                .flatten()
                .is_some()
        );
        let sections = actual.captured_mixed_postprocessed_sections(
            &batch,
            &staged,
            &completed,
            [target, second],
            inputs,
        )?;
        assert!(sections.matches_state(&completed, &batch, &staged, inputs));
        assert!(sections.sections().contains_key(&target));
        assert!(
            actual
                .captured_mixed_postprocessed_sections(&batch, &staged, &completed, [third], inputs)
                .is_err()
        );
        let components = actual.captured_mixed_postprocessed_components(
            &batch, &staged, &completed, &sections, inputs,
        )?;
        assert!(components.matches_state(&completed, &batch, &staged, inputs));
        assert!(!components.matches_state(&completed, &batch, &staged, wrong_inputs));
        let (chunks, _, _, _) = components.outputs();
        assert!(
            chunks[&target]
                .entity_records
                .as_ref()
                .is_some_and(|records| records
                    .iter()
                    .any(|record| record.get("id") == Some(&Tag::String("minecraft:pig".into()))))
        );
        assert!(chunks[&second].entity_records.as_ref().is_some_and(
            |records| records.as_slice() == [existing_entity.clone(), proto_entity.clone()]
        ));
        let original_parts = &chunks[&target];
        let unknown_entities = native_session::worldgen::PostprocessedChunkComponents {
            sections: original_parts.sections.clone(),
            heightmaps: original_parts.heightmaps.clone(),
            block_ticks: original_parts.block_ticks.clone(),
            fluid_ticks: original_parts.fluid_ticks.clone(),
            block_entities: original_parts.block_entities.clone(),
            entity_records: None,
        };
        assert!(
            assemble_captured_mixed_full_chunk(
                target,
                lighting.min_y,
                lighting.height,
                42,
                None,
                None,
                &unknown_entities,
            )
            .is_err()
        );
        assert!(
            actual
                .draft_captured_mixed_full_chunks(
                    &batch,
                    &staged,
                    &completed,
                    &components,
                    wrong_inputs
                )
                .is_err()
        );
        let full_drafts = actual.draft_captured_mixed_full_chunks(
            &batch,
            &staged,
            &completed,
            &components,
            inputs,
        )?;
        assert!(full_drafts.matches_state(&completed, &batch, &staged, inputs));
        assert!(!full_drafts.matches_state(&completed, &batch, &staged, wrong_inputs));
        let existing_poi = actual.draft_captured_mixed_existing_poi(
            &batch,
            &staged,
            &completed,
            &components,
            inputs,
            |_| Ok(()),
        )?;
        assert_eq!(
            existing_poi.documents[&second]
                .value
                .get("example:poi_root"),
            Some(&Tag::Int(86))
        );
        assert!(
            actual
                .stage_captured_mixed_full_save_plan(
                    &batch,
                    &staged,
                    &completed,
                    &components,
                    &full_drafts,
                    &existing_poi,
                    wrong_inputs,
                    10,
                )
                .is_err()
        );
        let save_candidate = actual.stage_captured_mixed_full_save_plan(
            &batch,
            &staged,
            &completed,
            &components,
            &full_drafts,
            &existing_poi,
            inputs,
            10,
        )?;
        assert_eq!(save_candidate.chunk_count(), 2);
        let full_new = &full_drafts.chunks()[&target];
        let full_saved = &full_drafts.chunks()[&second];
        assert!(full_saved.matches_entity_source(Some(&existing_entity_source.document))?);
        assert_eq!(
            full_saved.entity_chunk().value.get("example:entity_root"),
            Some(&Tag::Int(87))
        );
        assert!(
            matches!(full_saved.entity_chunk().value.get("Entities"), Some(Tag::List { values, .. }) if values == &[existing_entity.clone(), proto_entity.clone()])
        );
        assert!(full_new.matches_source(None)?);
        assert!(full_saved.matches_source(Some(&saved))?);
        assert!(!full_saved.matches_source(None)?);
        assert_eq!(
            full_new.chunk().value.get("Status"),
            Some(&Tag::String("minecraft:full".into()))
        );
        assert_eq!(
            full_saved.chunk().value.get("example:root"),
            Some(&Tag::Int(73))
        );
        assert_eq!(
            full_saved
                .chunk()
                .value
                .get("Heightmaps")
                .and_then(|maps| maps.get("example:map")),
            Some(&Tag::Int(37))
        );
        assert!(full_saved.chunk().value.get("UpgradeData").is_none());
        assert!(full_new.chunk().value.get("entities").is_none());
        let Some(Tag::List { values, .. }) = full_new.entity_chunk().value.get("Entities") else {
            bail!("entity region records missing")
        };
        assert!(
            values
                .iter()
                .any(|entity| entity.get("id") == Some(&Tag::String("minecraft:pig".into())))
        );
        if matches!(
            retry,
            native_session::worldgen::PostprocessingAttempt::Progress { .. }
        ) {
            actual.drain_captured_mixed_postprocessing_light(&batch, &staged, &mut pass, inputs)?;
        }
        assert!(Arc::ptr_eq(&before_pass, &batch.terrain.snapshot()));
        let mut waiting = None;
        'chunks: for position in [target, second, third] {
            let mut complete = position == target
                && matches!(
                    retry,
                    native_session::worldgen::PostprocessingAttempt::Progress { complete: true }
                );
            for _ in 0..1024 {
                if complete {
                    break;
                }
                match pass.try_poll(&batch, &staged, inputs, position, 64)? {
                    native_session::worldgen::PostprocessingAttempt::Progress {
                        complete: done,
                    } => complete = done,
                    native_session::worldgen::PostprocessingAttempt::NeedsChunk(missing) => {
                        waiting = Some(missing);
                        break 'chunks;
                    }
                }
            }
            assert!(complete, "postprocessing did not finish for {position:?}");
        }
        let missing = waiting.context("test expected another unprepared neighbor")?;
        assert_eq!(pass.waiting_neighbor(), Some(missing));
        assert!(pass.finish(&batch, &staged, inputs).is_err());
        batch.schedule = saved_schedule;
        staged.completed_creature_spawns.remove(&second);
        staged.completed_creature_spawns.remove(&target);
        staged.completed_creature_spawns.remove(&third);
        staged.completed_creature_spawns.remove(&neighbor);
        for position in support {
            staged.completed_creature_spawns.remove(&position);
        }
        assert!(!spawn.matches_state(&batch, &staged, true, &clocks, Some(changed_border)));
        assert!(!proof.matches_state(&batch, &staged, true, &clocks, Some(changed_border)));
        assert!(!saved_spawn.matches_state(&batch, &staged, true, &clocks, None));
        assert!(!saved_proof.matches_state(&batch, &staged, true, &clocks, None));
        let light_revision = Arc::clone(&staged.light_revision);
        staged.light_revision = Arc::new(());
        assert!(
            actual
                .check_captured_creature_candidate(
                    &batch,
                    &staged,
                    &mut pending,
                    true,
                    &clocks,
                    excluded,
                    64,
                )
                .is_err()
        );
        assert!(
            captured_plan
                .advance(&batch, &staged, true, &clocks)
                .is_err()
        );
        staged.light_revision = light_revision;
        assert!(
            actual
                .plan_captured_creatures(&batch, &staged, target, false, &clocks)
                .is_err()
        );
        assert!(
            actual
                .draft_captured_empty_spawn(&batch, &staged, saved_target, false, 42)
                .is_err()
        );
        assert!(
            actual
                .complete_captured_spawn_without_mobs(&mut batch, &mut staged, target, true)
                .is_err()
        );
        assert_eq!(
            batch.schedule().completed(target),
            Some(GenerationStage::Light)
        );
        actual.complete_captured_spawn_without_mobs(&mut batch, &mut staged, target, false)?;
        assert!(
            captured_plan
                .advance(&batch, &staged, true, &clocks)
                .is_err()
        );
        actual.complete_captured_spawn_without_mobs(&mut batch, &mut staged, second, false)?;
        actual.complete_captured_spawn_without_mobs(&mut batch, &mut staged, third, false)?;
        actual.complete_captured_spawn_without_mobs(&mut batch, &mut staged, neighbor, false)?;
        assert!(
            actual
                .complete_captured_spawn_without_mobs(&mut batch, &mut staged, target, false)
                .is_err()
        );
        assert!(
            actual
                .draft_captured_empty_spawn(&batch, &staged, saved_target, true, 42)
                .is_err()
        );
        let new_spawn =
            actual.draft_captured_empty_spawn(&batch, &staged, saved_target, false, 42)?;
        let saved_spawn =
            actual.draft_captured_empty_spawn(&batch, &staged, saved_second, false, 42)?;
        let early_spawn =
            actual.draft_captured_empty_spawn(&batch, &staged, saved_third, false, 42)?;
        let neighbor_spawn =
            actual.draft_captured_empty_spawn(&batch, &staged, saved_neighbor, false, 42)?;
        let empty_inputs = CapturedPostprocessingInputs {
            spawn_mobs: false,
            clocks: &clocks,
            border: None,
            game_time: 42,
            next_sub_tick: 9,
            water_source_conversion: true,
        };
        let empty_region = actual.begin_captured_mixed_postprocessing_region(
            &batch,
            &staged,
            [
                CapturedPostprocessingSpawn::Empty(&new_spawn),
                CapturedPostprocessingSpawn::Empty(&saved_spawn),
            ],
            empty_inputs,
        )?;
        assert!(empty_region.matches_state(&batch, &staged, empty_inputs));
        assert!(!empty_region.matches_state(
            &batch,
            &staged,
            CapturedPostprocessingInputs {
                spawn_mobs: true,
                ..empty_inputs
            }
        ));
        let empty_stage = actual.run_captured_mixed_postprocessing(
            &batch,
            &staged,
            empty_region,
            [target],
            empty_inputs,
        )?;
        assert!(empty_stage.finish(&batch, &staged, empty_inputs).is_err());
        assert!(new_spawn.matches_state(&batch, &staged, false));
        assert!(saved_spawn.matches_state(&batch, &staged, false));
        assert!(!new_spawn.matches_state(&batch, &staged, true));
        assert!(new_spawn.matches_source(None)?);
        assert!(!new_spawn.matches_source(Some(&saved))?);
        assert!(saved_spawn.matches_source(Some(&saved))?);
        assert!(early_spawn.matches_source(Some(&early_saved))?);
        assert!(early_spawn.matches_state(&batch, &staged, false));
        assert!(!saved_spawn.matches_source(None)?);
        assert!(new_spawn.entity_sidecar_source().is_none());
        assert_eq!(
            saved_spawn.entity_sidecar_source(),
            Some(existing_entity_source.document.as_ref())
        );
        assert!(new_spawn.has_entity_free_feature_coverage());
        assert!(saved_spawn.has_entity_free_feature_coverage());
        let empty_new_entities = captured_generated_entities_are_empty(
            None,
            new_spawn.entity_sidecar_source(),
            new_spawn.has_entity_free_feature_coverage(),
            actual.generate_structures || !batch.treasure_starts.is_empty(),
            batch.terrain.recorded_generated_entities(target)?,
            false,
        )?;
        assert!(empty_new_entities);
        assert_eq!(
            new_spawn.document().value.get("Status"),
            Some(&Tag::String("minecraft:spawn".into()))
        );
        assert_eq!(
            saved_spawn.document().value.get("Status"),
            Some(&Tag::String("minecraft:spawn".into()))
        );
        assert_eq!(
            early_spawn.document().value.get("Status"),
            Some(&Tag::String("minecraft:spawn".into()))
        );
        assert_eq!(
            saved.value.get("Status"),
            Some(&Tag::String("minecraft:biomes".into()))
        );
        assert_eq!(
            early_saved.value.get("Status"),
            Some(&Tag::String("minecraft:structure_references".into()))
        );
        assert_eq!(
            early_spawn.document().value.get("example:root"),
            Some(&Tag::Int(74))
        );
        let new_maps = actual.convert_captured_heightmaps(&batch, &staged, &new_spawn)?;
        let saved_maps = actual.convert_captured_heightmaps(&batch, &staged, &saved_spawn)?;
        let early_maps = actual.convert_captured_heightmaps(&batch, &staged, &early_spawn)?;
        assert!(new_maps.matches_source(None)?);
        assert!(saved_maps.matches_source(Some(&saved))?);
        assert!(early_maps.matches_source(Some(&early_saved))?);
        assert!(!saved_maps.matches_source(Some(&early_saved))?);
        assert!(new_maps.other_fields().is_empty());
        assert!(early_maps.other_fields().is_empty());
        assert_eq!(
            saved_maps.other_fields(),
            &[("example:map".into(), Tag::Int(37))]
        );
        for maps in [&new_maps, &saved_maps, &early_maps] {
            assert!(maps.matches_state(&batch, &staged, false));
            assert!(!maps.matches_state(&batch, &staged, true));
            let Tag::Compound(fields) = maps.heightmaps().to_tag()? else {
                unreachable!()
            };
            assert_eq!(fields.len(), 4);
        }
        let new_post = actual.begin_captured_postprocessing(
            &batch,
            &staged,
            &new_spawn,
            actual.convert_captured_heightmaps(&batch, &staged, &new_spawn)?,
        )?;
        let saved_post = actual.begin_captured_postprocessing(
            &batch,
            &staged,
            &saved_spawn,
            actual.convert_captured_heightmaps(&batch, &staged, &saved_spawn)?,
        )?;
        let mut active_upgrade =
            actual.draft_captured_empty_spawn(&batch, &staged, saved_second, false, 42)?;
        let CapturedLitChunkDraft::Saved(ref mut active_lit) = active_upgrade.lit else {
            unreachable!()
        };
        replace_unique_root_field(
            &mut active_lit.document,
            "UpgradeData",
            Tag::Compound(vec![("Sides".into(), Tag::Int(1))]),
        )?;
        let active_maps = actual.convert_captured_heightmaps(&batch, &staged, &active_upgrade)?;
        let error = actual
            .begin_captured_postprocessing(&batch, &staged, &active_upgrade, active_maps)
            .err()
            .context("active upgrade data was accepted")?;
        assert!(format!("{error:#}").contains("saved upgrade sides require callbacks"));
        let early_post = actual.begin_captured_postprocessing(
            &batch,
            &staged,
            &early_spawn,
            actual.convert_captured_heightmaps(&batch, &staged, &early_spawn)?,
        )?;
        assert!(
            actual
                .begin_captured_postprocessing(
                    &batch,
                    &staged,
                    &saved_spawn,
                    actual.convert_captured_heightmaps(&batch, &staged, &new_spawn)?,
                )
                .is_err()
        );
        for converted in [&new_post, &saved_post, &early_post] {
            assert!(converted.matches_state(&batch, &staged, false));
            assert!(!converted.matches_state(&batch, &staged, true));
        }
        assert!(
            actual
                .begin_captured_postprocessing_region(&batch, &staged, [], 42, 9)
                .is_err()
        );
        assert!(
            actual
                .begin_captured_postprocessing_region(
                    &batch,
                    &staged,
                    [&new_spawn, &new_spawn],
                    42,
                    9
                )
                .is_err()
        );
        let converted_region = actual.begin_captured_postprocessing_region(
            &batch,
            &staged,
            [&early_spawn, &new_spawn, &saved_spawn],
            42,
            9,
        )?;
        assert!(converted_region.matches_state(&batch, &staged, false, 42, 9));
        assert!(!converted_region.matches_state(&batch, &staged, true, 42, 9));
        assert!(!converted_region.matches_state(&batch, &staged, false, 43, 9));
        assert!(!converted_region.matches_state(&batch, &staged, false, 42, 10));
        let draft = actual.draft_captured_new_lit_chunk(&batch, &staged, saved_target, 42)?;
        let structure_draft = actual.draft_captured_new_lit_chunk(
            &batch,
            &staged,
            native_persistence::world::ChunkPos {
                x: structure_target.x,
                z: structure_target.z,
            },
            42,
        )?;
        assert_eq!(
            structure_draft
                .document()
                .value
                .get("structures")
                .and_then(|value| value.get("References"))
                .and_then(|value| value.get("minecraft:buried_treasure")),
            Some(&Tag::LongArray(vec![(2i64 << 32) | 0])),
        );
        let saved_draft = actual.draft_captured_saved_lit_chunk(&batch, &staged, saved_second)?;
        let early_draft = actual.draft_captured_saved_lit_chunk(&batch, &staged, saved_third)?;
        assert!(early_draft.matches_state(&batch, &staged));
        assert!(early_draft.matches_source(&early_saved)?);
        assert_eq!(
            early_draft.document().value.get("Status"),
            early_saved.value.get("Status")
        );
        assert_eq!(
            early_draft.document().value.get("example:root"),
            Some(&Tag::Int(74))
        );
        let early_sections =
            native_persistence::palette::decode_sections(&early_draft.document().value)?;
        let early_marker = [third.x * 16 + 8, lighting.min_y + 8, third.z * 16 + 8];
        let early_block = early_sections
            .iter()
            .find(|section| i32::from(section.y) == early_marker[1].div_euclid(16))
            .and_then(|section| section.blocks.as_ref())
            .and_then(|blocks| blocks.block(8, early_marker[1].rem_euclid(16) as usize, 8))
            .context("early draft marker block missing")?;
        assert!(early_block.name.matches("minecraft:melon"));
        assert!(
            early_sections
                .iter()
                .any(|section| section.block_light.is_some() || section.sky_light.is_some())
        );
        assert!(saved_draft.matches_state(&batch, &staged));
        assert!(saved_draft.matches_source(&saved)?);
        assert!(!saved_draft.matches_source(draft.document())?);
        assert_eq!(
            saved_draft.entity_sidecar_source(),
            Some(existing_entity_source.document.as_ref())
        );
        assert!(saved_draft.has_entity_free_feature_coverage());
        assert_eq!(
            saved_draft.document().value.get("Status"),
            Some(&Tag::String("minecraft:biomes".into()))
        );
        assert_eq!(
            saved_draft.document().value.get("example:root"),
            Some(&Tag::Int(73))
        );
        assert_eq!(
            saved_draft
                .document()
                .value
                .get("Heightmaps")
                .and_then(|value| value.get("example:map")),
            Some(&Tag::Int(37))
        );
        let Some(Tag::List {
            values: saved_sections,
            ..
        }) = saved_draft.document().value.get("sections")
        else {
            bail!("saved draft sections missing");
        };
        assert!(
            saved_sections
                .iter()
                .any(|section| section.get("example:section") == Some(&Tag::Int(91)))
        );
        let saved_marker = [second.x * 16 + 8, lighting.min_y + 8, second.z * 16 + 8];
        let saved_decoded =
            native_persistence::palette::decode_sections(&saved_draft.document().value)?;
        let saved_block = saved_decoded
            .iter()
            .find(|section| i32::from(section.y) == saved_marker[1].div_euclid(16))
            .and_then(|section| section.blocks.as_ref())
            .and_then(|blocks| blocks.block(8, saved_marker[1].rem_euclid(16) as usize, 8))
            .context("saved draft marker block missing")?;
        assert!(saved_block.name.matches("minecraft:melon"));
        let Some(Tag::List {
            values: saved_ticks,
            ..
        }) = saved_draft.document().value.get("block_ticks")
        else {
            bail!("saved draft ticks missing");
        };
        assert!(saved_ticks.iter().any(
            |tick| matches!(tick.get("i"), Some(Tag::String(id)) if id.matches("minecraft:melon"))
                && tick.get("x") == Some(&Tag::Int(saved_marker[0]))
                && tick.get("y") == Some(&Tag::Int(saved_marker[1]))
                && tick.get("z") == Some(&Tag::Int(saved_marker[2]))
        ));
        assert!(
            actual
                .draft_captured_saved_lit_chunk(&batch, &staged, saved_target)
                .is_err()
        );
        assert!(draft.matches_state(&batch, &staged));
        assert!(draft.has_entity_free_feature_coverage());
        assert!(draft.entity_sidecar_source().is_none());
        assert_eq!(
            draft.document().value.get("Status"),
            Some(&Tag::String("minecraft:light".into()))
        );
        assert_eq!(
            draft.document().value.get("LastUpdate"),
            Some(&Tag::Long(42))
        );
        let sections = native_persistence::palette::decode_sections(&draft.document().value)?;
        let section = sections
            .iter()
            .find(|section| i32::from(section.y) == marker[1].div_euclid(16))
            .context("draft marker section missing")?;
        let block = section
            .blocks
            .as_ref()
            .context("draft marker blocks missing")?
            .block(
                marker[0].rem_euclid(16) as usize,
                marker[1].rem_euclid(16) as usize,
                marker[2].rem_euclid(16) as usize,
            )
            .context("draft marker block missing")?;
        assert!(block.name.matches("minecraft:melon"));
        let Some(Tag::List { values, .. }) = draft.document().value.get("block_ticks") else {
            bail!("draft block ticks missing");
        };
        assert!(
            values.iter().any(|tick| {
                matches!(tick.get("i"), Some(Tag::String(id)) if id.matches("minecraft:melon"))
                    && tick.get("x") == Some(&Tag::Int(marker[0]))
                    && tick.get("y") == Some(&Tag::Int(marker[1]))
                    && tick.get("z") == Some(&Tag::Int(marker[2]))
            }),
            "draft scheduled melon tick missing"
        );
        let original_revision = Arc::clone(&staged.light_revision);
        staged.light_revision = Arc::new(());
        assert!(!draft.matches_state(&batch, &staged));
        assert!(!saved_draft.matches_state(&batch, &staged));
        assert!(!new_spawn.matches_state(&batch, &staged, false));
        assert!(!saved_spawn.matches_state(&batch, &staged, false));
        assert!(!early_spawn.matches_state(&batch, &staged, false));
        assert!(!early_maps.matches_state(&batch, &staged, false));
        assert!(!early_post.matches_state(&batch, &staged, false));
        assert!(!converted_region.matches_state(&batch, &staged, false, 42, 9));
        assert!(
            actual
                .convert_captured_heightmaps(&batch, &staged, &early_spawn)
                .is_err()
        );
        staged.light_revision = original_revision;
        assert!(converted_region.matches_state(&batch, &staged, false, 42, 9));
        let (proofs, region) = converted_region.into_parts();
        assert_eq!(region.ticks.registration_order(), &[third, target, second]);
        assert_eq!(region.ticks.game_time(), 42);
        assert_eq!(region.ticks.next_sub_tick(), 9);
        assert_eq!(region.tasks.len(), 3);
        assert!(proofs[&target].matches_source(None)?);
        assert!(proofs[&second].matches_source(Some(&saved))?);
        assert!(proofs[&third].matches_source(Some(&early_saved))?);
        for pos in [third, target, second] {
            let ticks = region
                .ticks
                .chunk(pos)
                .context("region tick container missing")?;
            assert!(ticks.blocks.peek().is_none());
            assert!(
                ticks
                    .blocks
                    .pack(42)
                    .iter()
                    .any(|tick| tick.kind == "minecraft:melon")
            );
        }
        let mut invalid_region = actual.begin_captured_postprocessing_region(
            &batch,
            &staged,
            [&new_spawn, &saved_spawn, &early_spawn],
            42,
            9,
        )?;
        invalid_region
            .region
            .ticks
            .chunk_mut(target)
            .unwrap()
            .blocks
            .unpack(42);
        assert!(
            actual
                .run_captured_postprocessing(&batch, &staged, invalid_region, false, 42, 9, true,)
                .is_err()
        );
        let invalid_targets =
            actual.begin_captured_postprocessing_region(&batch, &staged, [&new_spawn], 42, 9)?;
        assert!(
            actual
                .run_captured_postprocessing_for_targets(
                    &batch,
                    &staged,
                    invalid_targets,
                    [target, target],
                    false,
                    42,
                    9,
                    true,
                )
                .is_err()
        );
        let invalid_targets =
            actual.begin_captured_postprocessing_region(&batch, &staged, [&new_spawn], 42, 9)?;
        assert!(
            actual
                .run_captured_postprocessing_for_targets(
                    &batch,
                    &staged,
                    invalid_targets,
                    [second],
                    false,
                    42,
                    9,
                    true,
                )
                .is_err()
        );
        let pass_region = actual.begin_captured_postprocessing_region(
            &batch,
            &staged,
            [&new_spawn, &saved_spawn, &early_spawn],
            42,
            9,
        )?;
        let mut pass = actual.run_captured_postprocessing_for_targets(
            &batch,
            &staged,
            pass_region,
            [target],
            false,
            42,
            9,
            true,
        )?;
        assert!(pass.matches_state(&batch, &staged, false, 42, 9, true));
        assert!(!pass.matches_state(&batch, &staged, false, 43, 9, true));
        assert!(!pass.matches_state(&batch, &staged, false, 42, 9, false));
        assert!(
            pass.try_poll(&batch, &staged, false, 43, 9, true, target, 1)
                .is_err()
        );
        assert!(
            pass.try_poll(&batch, &staged, false, 42, 9, true, second, 1)
                .is_err()
        );
        assert!(
            pass.request_converted_chunk(&batch, &staged, false, 43, 9, true, second)
                .is_err()
        );
        assert!(
            pass.request_converted_chunk(&batch, &staged, false, 42, 9, true, neighbor)
                .is_err()
        );
        pass.request_converted_chunk(&batch, &staged, false, 42, 9, true, second)?;
        assert!(
            pass.request_converted_chunk(&batch, &staged, false, 42, 9, true, second)
                .is_err()
        );
        pass.request_converted_chunk(&batch, &staged, false, 42, 9, true, third)?;
        assert!(
            !actual.drain_captured_postprocessing_light(
                &batch, &staged, &mut pass, false, 42, 9, true,
            )?
        );
        assert!(actual.drain_captured_postprocessing_light(
            &batch, &staged, &mut pass, false, 43, 9, true,
        ).is_err());
        let before_pass = batch.terrain.snapshot();
        assert_eq!(
            pass.try_poll(&batch, &staged, false, 42, 9, true, target, 1)?,
            native_session::worldgen::PostprocessingAttempt::NeedsChunk(neighbor),
        );
        assert_eq!(pass.waiting_neighbor(), Some(neighbor));
        assert!(actual.drain_captured_postprocessing_light(
            &batch, &staged, &mut pass, false, 42, 9, true,
        ).is_err());
        assert!(
            pass.request_converted_chunk(&batch, &staged, false, 42, 9, true, third)
                .is_err()
        );
        assert!(
            pass.try_poll(&batch, &staged, false, 42, 9, true, target, 1)
                .is_err()
        );
        assert!(
            actual
                .add_captured_postprocessing_neighbor(
                    &batch,
                    &staged,
                    &mut pass,
                    &saved_spawn,
                    false,
                    42,
                    9,
                    true
                )
                .is_err()
        );
        assert!(
            actual
                .add_captured_postprocessing_neighbor(
                    &batch,
                    &staged,
                    &mut pass,
                    &neighbor_spawn,
                    false,
                    43,
                    9,
                    true
                )
                .is_err()
        );
        assert_eq!(pass.waiting_neighbor(), Some(neighbor));
        actual.add_captured_postprocessing_neighbor(
            &batch,
            &staged,
            &mut pass,
            &neighbor_spawn,
            false,
            42,
            9,
            true,
        )?;
        assert_eq!(pass.waiting_neighbor(), None);
        assert!(pass.proofs.contains_key(&neighbor));
        assert!(actual.drain_captured_postprocessing_light(
            &batch, &staged, &mut pass, false, 42, 9, true,
        ).is_err());
        assert!(
            pass.request_converted_chunk(&batch, &staged, false, 42, 9, true, neighbor)
                .is_err()
        );
        assert!(
            actual
                .add_captured_postprocessing_neighbor(
                    &batch,
                    &staged,
                    &mut pass,
                    &neighbor_spawn,
                    false,
                    42,
                    9,
                    true
                )
                .is_err()
        );
        assert!(
            pass.try_poll(&batch, &staged, false, 42, 9, true, second, 1)
                .is_err()
        );
        let retried = pass.try_poll(&batch, &staged, false, 42, 9, true, target, 1)?;
        assert_ne!(
            retried,
            native_session::worldgen::PostprocessingAttempt::NeedsChunk(neighbor)
        );
        let baseline_region = actual.begin_captured_postprocessing_region(
            &batch,
            &staged,
            [&new_spawn, &saved_spawn, &early_spawn, &neighbor_spawn],
            42,
            9,
        )?;
        let mut baseline = actual.run_captured_postprocessing(
            &batch,
            &staged,
            baseline_region,
            false,
            42,
            9,
            true,
        )?;
        assert_eq!(
            retried,
            baseline.try_poll(&batch, &staged, false, 42, 9, true, target, 1)?,
            "retry differs from registering the same neighbors in advance"
        );
        if matches!(
            retried,
            native_session::worldgen::PostprocessingAttempt::Progress { .. }
        ) {
            let changed = actual.drain_captured_postprocessing_light(
                &batch, &staged, &mut pass, false, 42, 9, true,
            )?;
            eprintln!("captured postprocessing light checks drained: {changed}");
            assert!(!actual.drain_captured_postprocessing_light(
                &batch, &staged, &mut pass, false, 42, 9, true,
            )?);
        }
        assert!(Arc::ptr_eq(&before_pass, &batch.terrain.snapshot()));
        assert!(pass.finish(&batch, &staged, false, 42, 9, true).is_err());
        let tick_world = batch.terrain.snapshot();
        for (position, converted) in [
            (target, new_post),
            (second, saved_post),
            (third, early_post),
        ] {
            use native_session::worldgen::chunk_ticks::SavedTick;
            assert!(converted.matches_state(&batch, &staged, false));
            let (proof, converted) = converted.into_parts();
            assert!(proof.matches_state(&batch, &staged, false));
            let expected_blocks: Vec<_> = tick_world
                .saved_block_ticks
                .get(&position)
                .into_iter()
                .flatten()
                .map(|tick| SavedTick {
                    kind: tick.block.clone(),
                    position: tick.position,
                    delay: tick.delay,
                    priority: tick.priority,
                })
                .collect();
            let expected_fluids: Vec<_> = tick_world
                .saved_fluid_ticks
                .get(&position)
                .into_iter()
                .flatten()
                .map(|tick| SavedTick {
                    kind: tick.fluid.clone(),
                    position: tick.position,
                    delay: tick.delay,
                    priority: tick.priority,
                })
                .collect();
            assert_eq!(converted.ticks.blocks.pack(42), expected_blocks);
            assert_eq!(converted.ticks.fluids.pack(42), expected_fluids);
            assert!(converted.ticks.blocks.peek().is_none());
            assert!(converted.ticks.fluids.peek().is_none());
            assert!(
                expected_blocks
                    .iter()
                    .any(|tick| tick.kind == "minecraft:melon")
            );
            let activated = converted.ticks.blocks.activated(42);
            assert_eq!(activated.len(), expected_blocks.len());
        }
        assert!(draft.matches_state(&batch, &staged));
        assert!(
            actual
                .draft_captured_new_lit_chunk(&batch, &staged, saved_second, 42)
                .is_err()
        );

        let oracle = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        let mut paths = vec![version.join("26.3.jar"), oracle.path().to_path_buf()];
        for library in manifest["libraries"]
            .as_array()
            .context("libraries missing")?
        {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let classpath = std::env::join_paths(paths)?;
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = oracle.path().join("ResumedLightOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/ResumedLightOracle.java"),
        )?;
        let heightmap_oracle = oracle.path().join("FullHeightmapOracle.java");
        std::fs::write(
            &heightmap_oracle,
            include_str!("../../../persistence/tests/java/FullHeightmapOracle.java"),
        )?;
        let structure_oracle = oracle.path().join("SavedStructureReferencesOracle.java");
        std::fs::write(
            &structure_oracle,
            include_str!("../../../session/tests/java/SavedStructureReferencesOracle.java"),
        )?;
        let entity_oracle = oracle.path().join("GeneratedWorldJavaOracle.java");
        std::fs::write(
            &entity_oracle,
            include_str!("../../tests/java/GeneratedWorldJavaOracle.java"),
        )?;
        let compiled = std::process::Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source)
            .arg(&heightmap_oracle)
            .arg(&structure_oracle)
            .arg(&entity_oracle)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java oracle compilation failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let structure_input = oracle.path().join("generated-structure-references.nbt");
        let structures = structure_draft
            .document()
            .value
            .get("structures")
            .context("generated structures missing")?
            .clone();
        let case = Tag::Compound(vec![
            ("x".into(), Tag::Int(structure_target.x)),
            ("z".into(), Tag::Int(structure_target.z)),
            ("structures".into(), structures),
            (
                "expected".into(),
                Tag::Compound(vec![(
                    "minecraft:buried_treasure".into(),
                    Tag::LongArray(vec![2i64 << 32]),
                )]),
            ),
        ]);
        std::fs::write(
            &structure_input,
            native_persistence::nbt::write(&NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![(
                    "cases".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![case],
                    },
                )]),
            })?,
        )?;
        let checked = std::process::Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("SavedStructureReferencesOracle")
            .arg(&structure_input)
            .output()?;
        ensure!(
            checked.status.success(),
            "Java structure references validation failed: {}\n{}",
            String::from_utf8_lossy(&checked.stdout),
            String::from_utf8_lossy(&checked.stderr)
        );
        let mut heightmap_cases = Vec::new();
        for (position, maps) in [
            (target, &new_maps),
            (second, &saved_maps),
            (third, &early_maps),
        ] {
            heightmap_cases.push(Tag::Compound(vec![
                ("min_y".into(), Tag::Int(lighting.min_y)),
                ("height".into(), Tag::Int(lighting.height)),
                (
                    "source".into(),
                    batch.terrain.generated_heightmap_data(position)?,
                ),
                ("expected".into(), maps.heightmaps().to_tag()?),
            ]));
        }
        for (position, maps) in [(target, &proof), (second, &saved_proof)] {
            heightmap_cases.push(Tag::Compound(vec![
                ("min_y".into(), Tag::Int(lighting.min_y)),
                ("height".into(), Tag::Int(lighting.height)),
                (
                    "source".into(),
                    batch.terrain.generated_heightmap_data(position)?,
                ),
                ("expected".into(), maps.heightmaps().to_tag()?),
            ]));
        }
        let heightmap_input = oracle.path().join("converted-heightmaps.nbt");
        std::fs::write(
            &heightmap_input,
            native_persistence::nbt::write(&NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![(
                    "cases".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: heightmap_cases,
                    },
                )]),
            })?,
        )?;
        let checked = std::process::Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("FullHeightmapOracle")
            .arg(&heightmap_input)
            .output()?;
        ensure!(
            checked.status.success(),
            "Java converted heightmap validation failed: {}\n{}",
            String::from_utf8_lossy(&checked.stdout),
            String::from_utf8_lossy(&checked.stderr)
        );
        for (name, document, spawn_status) in [
            ("new", draft.document(), false),
            ("saved", saved_draft.document(), false),
            ("early", early_draft.document(), false),
            ("spawn", new_spawn.document(), true),
            ("saved_spawn", saved_spawn.document(), true),
            ("early_spawn", early_spawn.document(), true),
        ] {
            let nbt = oracle.path().join(format!("{name}-light.nbt"));
            std::fs::write(&nbt, native_persistence::nbt::write(document)?)?;
            let mut command = std::process::Command::new(java.join("java.exe"));
            command
                .arg("-cp")
                .arg(&classpath)
                .arg("ResumedLightOracle")
                .arg(&nbt)
                .arg(lighting.min_y.to_string())
                .arg(lighting.height.to_string());
            if spawn_status {
                command.arg("status:spawn");
            }
            let checked = command.output()?;
            ensure!(
                checked.status.success(),
                "Java {name} light validation failed: {}\n{}",
                String::from_utf8_lossy(&checked.stdout),
                String::from_utf8_lossy(&checked.stderr)
            );
        }
        for (name, document) in [
            ("planned", spawn.document()),
            (
                "worker",
                worker_spawn_document
                    .as_ref()
                    .context("spawn worker document missing")?,
            ),
        ] {
            let animal_input = oracle.path().join(format!("captured-{name}-animal.nbt"));
            std::fs::write(&animal_input, native_persistence::nbt::write(document)?)?;
            let checked = std::process::Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&classpath)
                .arg("ResumedLightOracle")
                .arg(&animal_input)
                .arg(lighting.min_y.to_string())
                .arg(lighting.height.to_string())
                .arg("entities:pig")
                .output()?;
            ensure!(
                checked.status.success(),
                "Java captured {name} animal validation failed: {}\n{}",
                String::from_utf8_lossy(&checked.stdout),
                String::from_utf8_lossy(&checked.stderr)
            );
            if name == "worker" {
                let checked = std::process::Command::new(java.join("java.exe"))
                    .arg("-cp")
                    .arg(&classpath)
                    .arg("GeneratedWorldJavaOracle")
                    .arg("--proto-entities")
                    .arg(&animal_input)
                    .output()?;
                ensure!(
                    checked.status.success()
                        && String::from_utf8_lossy(&checked.stdout)
                            .lines()
                            .any(|line| line.starts_with("PROTO_ENTITIES_OK ")),
                    "Java spawn worker entity round trip failed: {}\n{}",
                    String::from_utf8_lossy(&checked.stdout),
                    String::from_utf8_lossy(&checked.stderr)
                );
            }
        }
        for (name, draft) in [("new", full_new), ("saved", full_saved)] {
            let chunk_input = oracle.path().join(format!("captured-{name}-full.nbt"));
            let entity_input = oracle.path().join(format!("captured-{name}-entities.nbt"));
            std::fs::write(&chunk_input, native_persistence::nbt::write(draft.chunk())?)?;
            std::fs::write(
                &entity_input,
                native_persistence::nbt::write(draft.entity_chunk())?,
            )?;
            let mut command = std::process::Command::new(java.join("java.exe"));
            command
                .arg("-cp")
                .arg(&classpath)
                .arg("ResumedLightOracle")
                .arg(&chunk_input)
                .arg(lighting.min_y.to_string())
                .arg(lighting.height.to_string())
                .arg("status:full")
                .arg(&entity_input);
            let checked = command.output()?;
            ensure!(
                checked.status.success(),
                "Java {name} FULL chunk validation failed: {}\n{}",
                String::from_utf8_lossy(&checked.stdout),
                String::from_utf8_lossy(&checked.stderr)
            );
        }
        assert!(
            actual
                .propagate_captured_light(&mut batch, &mut staged, target)
                .is_err()
        );
        batch.schedule = spawn_schedule;
        for position in [target, second] {
            staged.completed_creature_spawns.remove(&position);
            staged.completed_empty_spawns.remove(&position);
        }
        let (streamer, initialized) = crate::chunk_stream::light_worker_test_fixture(
            actual,
            batch,
            restored,
            staged,
            &worker_session,
            reader
                .session_guard()
                .context("world session lock missing")?,
            target,
        )?;
        let inputs = crate::chunk_stream::MissingGenerationSpawnInputs {
            spawn_mobs: true,
            clocks,
            border: native_session::respawn::RespawnBorder {
                min_x: -1000.0,
                max_x: 1000.0,
                min_z: -1000.0,
                max_z: 1000.0,
            },
            last_update: 42,
        };
        let mut worker = crate::chunk_stream::MissingGenerationSpawnWorker::start(
            initialized,
            &streamer,
            &worker_session,
            vec![target],
            inputs.clone(),
        )
        .map_err(|failure| failure.error)?;
        worker.cancel();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        loop {
            match worker.try_result(&streamer, &worker_session, &inputs) {
                Ok(None) => {}
                Ok(Some(_)) => bail!("cancelled spawn generation was accepted"),
                Err(error) => {
                    assert!(error.to_string().contains("cancelled"));
                    break;
                }
            }
            ensure!(
                std::time::Instant::now() < deadline,
                "spawn cancellation timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        Ok(())
    }

    #[test]
    fn captured_spawn_candidates_require_complete_known_sources() -> Result<()> {
        let pig = Tag::Compound(vec![("id".into(), Tag::String("minecraft:pig".into()))]);
        let document = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "entities".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![pig.clone()],
                },
            )]),
        };
        let sidecar = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![]),
        };
        let records = captured_spawn_entity_candidates(&document, None, true, false)?.unwrap();
        assert_eq!(records.as_slice(), &[pig]);
        assert!(captured_spawn_entity_candidates(&document, Some(&sidecar), true, false).is_err());
        let positioned = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("xPos".into(), Tag::Int(1)),
                ("zPos".into(), Tag::Int(2)),
                (
                    "entities".into(),
                    document.value.get("entities").unwrap().clone(),
                ),
            ]),
        };
        let position = native_world::loaded::ChunkPos { x: 1, z: 2 };
        let empty_sidecar =
            native_persistence::generated_chunk::assemble_entity_chunk(position, &[])?;
        assert_eq!(
            captured_spawn_entity_candidates(&positioned, Some(&empty_sidecar), true, false)?
                .unwrap()
                .len(),
            1
        );
        let nonempty_sidecar = native_persistence::generated_chunk::assemble_entity_chunk(
            position,
            &[Tag::Compound(vec![])],
        )?;
        let candidates =
            captured_spawn_entity_candidates(&positioned, Some(&nonempty_sidecar), true, false)?
                .context("sidecar candidates missing")?;
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0], Tag::Compound(vec![]));
        assert_eq!(candidates[1], records[0]);
        let merged = merge_captured_entity_candidates(position, &nonempty_sidecar, &candidates)?;
        assert!(
            matches!(merged.value.get("Entities"), Some(Tag::List { values, .. }) if values == candidates.as_ref())
        );
        assert!(
            merge_captured_entity_candidates(position, &nonempty_sidecar, &candidates[1..])
                .is_err()
        );
        assert!(captured_spawn_entity_candidates(&document, None, false, false)?.is_none());
        assert!(captured_spawn_entity_candidates(&document, None, true, true)?.is_none());
        let invalid = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![("entities".into(), Tag::Int(7))]),
        };
        assert!(captured_spawn_entity_candidates(&invalid, None, true, false).is_err());
        let empty = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![]),
        };
        assert!(
            captured_spawn_entity_candidates(&empty, None, true, false)?
                .unwrap()
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn known_entity_ids_include_proto_sidecar_and_nested_passengers() {
        let entity = |uuid| Tag::Compound(vec![("UUID".into(), Tag::IntArray(uuid))]);
        let list = |values| Tag::List {
            element_type: TagType::Compound,
            values,
        };
        let proto = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "entities".into(),
                list(vec![Tag::Compound(vec![
                    ("UUID".into(), Tag::IntArray(vec![1, 2, 3, 4])),
                    (
                        "Passengers".into(),
                        list(vec![entity(vec![5, 6, 7, 8]), Tag::Int(99)]),
                    ),
                ])]),
            )]),
        };
        let sidecar = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "Entities".into(),
                list(vec![
                    entity(vec![9, 10, 11, 12]),
                    entity(vec![1, 2]),
                    Tag::Compound(vec![]),
                ]),
            )]),
        };
        let mut ids = KnownEntityIds::default();
        ids.observe_document(&proto, "entities");
        ids.observe_document(&sidecar, "Entities");
        ids.observe_records(&[entity(vec![13, 14, 15, 16])]);
        assert_eq!(
            ids.0,
            BTreeSet::from([
                [1, 2, 3, 4],
                [5, 6, 7, 8],
                [9, 10, 11, 12],
                [13, 14, 15, 16],
            ])
        );
    }

    #[test]
    fn captured_empty_entities_require_each_source_to_be_proven_empty() -> Result<()> {
        let empty = |element_type| NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "entities".into(),
                Tag::List {
                    element_type,
                    values: vec![],
                },
            )]),
        };
        let compound = Tag::Compound(vec![("id".into(), Tag::String("minecraft:pig".into()))]);
        let populated = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "entities".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![compound.clone()],
                },
            )]),
        };
        let sidecar = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![]),
        };
        fn proof(
            source: Option<&NamedTag>,
            sidecar: Option<&NamedTag>,
            coverage: bool,
            structures: bool,
            records: &[Tag],
            drops: bool,
        ) -> Result<bool> {
            captured_generated_entities_are_empty(
                source, sidecar, coverage, structures, records, drops,
            )
        }
        assert!(proof(None, None, true, false, &[], false)?);
        let empty_end = empty(TagType::End);
        let empty_compound = empty(TagType::Compound);
        assert!(proof(Some(&empty_end), None, true, false, &[], false)?);
        assert!(proof(Some(&empty_compound), None, true, false, &[], false)?);
        assert!(!proof(Some(&populated), None, true, false, &[], false)?);
        assert!(!proof(None, Some(&sidecar), true, false, &[], false)?);
        assert!(!proof(None, None, false, false, &[], false)?);
        assert!(!proof(None, None, true, true, &[], false)?);
        assert!(!proof(None, None, true, false, &[compound], false)?);
        assert!(!proof(None, None, true, false, &[], true)?);
        let invalid = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![("entities".into(), Tag::Int(1))]),
        };
        assert!(proof(Some(&invalid), None, true, false, &[], false).is_err());
        let duplicate = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                (
                    "entities".into(),
                    Tag::List {
                        element_type: TagType::End,
                        values: vec![],
                    },
                ),
                (
                    "entities".into(),
                    Tag::List {
                        element_type: TagType::End,
                        values: vec![],
                    },
                ),
            ]),
        };
        assert!(proof(Some(&duplicate), None, true, false, &[], false).is_err());
        Ok(())
    }

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
    fn captured_poi_selection_requires_a_captured_absent_sidecar() -> Result<()> {
        use native_persistence::world::{
            ChunkPos as SavedPos, GenerationChunkSource, SavedPoiChunk,
        };
        use native_world::loaded::ChunkPos;

        let missing = ChunkPos { x: -1, z: 2 };
        let present = ChunkPos { x: 3, z: -4 };
        let unknown = ChunkPos { x: 5, z: 6 };
        let saved = |pos: ChunkPos| SavedPos { x: pos.x, z: pos.z };
        let sources = BTreeMap::from([
            (
                saved(missing),
                GenerationChunkSource {
                    terrain: None,
                    entities: None,
                    poi: None,
                },
            ),
            (
                saved(present),
                GenerationChunkSource {
                    terrain: None,
                    entities: None,
                    poi: Some(SavedPoiChunk {
                        position: saved(present),
                        document: Arc::new(NamedTag {
                            name: "".into(),
                            value: Tag::Compound(vec![]),
                        }),
                        sections: BTreeMap::new(),
                    }),
                },
            ),
        ]);
        assert_eq!(
            captured_missing_poi_positions(&sources, [present, missing])?,
            [missing]
        );
        assert!(captured_missing_poi_positions(&sources, [missing, unknown]).is_err());
        assert!(captured_missing_poi_positions(&sources, [present])?.is_empty());
        Ok(())
    }

    #[test]
    fn postprocessed_output_retains_external_structure_sources() -> Result<()> {
        use native_world::loaded::ChunkPos;

        let first = ChunkPos { x: -1, z: 2 };
        let second = ChunkPos { x: 3, z: -4 };
        let source = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![("original".into(), Tag::Int(42))]),
        };
        let changed = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![("changed".into(), Tag::Int(7))]),
        };
        let make_draft = || -> Result<ResumedStructureDraft> {
            Ok(ResumedStructureDraft {
                source_nbt: native_persistence::nbt::write(&source)?,
                document: changed.clone(),
            })
        };
        let drafts = BTreeMap::from([(first, make_draft()?), (second, make_draft()?)]);
        let (mut included, external) = partition_structure_sources(drafts, |pos| pos == first);
        assert_eq!(included.keys().copied().collect::<Vec<_>>(), [first]);
        assert_eq!(external.keys().copied().collect::<Vec<_>>(), [second]);
        assert_eq!(included.remove(&first).unwrap().document(), &changed);
        assert!(included.is_empty());
        let components = CapturedPostprocessedComponents {
            result_identity: Arc::new(()),
            chunks: BTreeMap::new(),
            structures: BTreeMap::from([(first, Some(Tag::Int(1)))]),
            external_structure_sources: external,
            missing_poi: BTreeMap::from([(first, None)]),
            generated_empty_entities: BTreeSet::from([first]),
        };
        let (chunks, structures, pending, poi, empty_entities) = components.outputs();
        assert!(chunks.is_empty());
        assert_eq!(structures.get(&first), Some(&Some(Tag::Int(1))));
        assert_eq!(pending[&second].document(), &changed);
        assert!(pending[&second].matches_source(&source)?);
        assert!(!pending[&second].matches_source(&changed)?);
        assert_eq!(poi.get(&first), Some(&None));
        assert!(!poi.contains_key(&second));
        assert_eq!(empty_entities, &BTreeSet::from([first]));
        Ok(())
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
        use native_session::worldgen::schedule::GenerationStage;
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
        for (status, stage) in [
            ("minecraft:empty", GenerationStage::Empty),
            (
                "minecraft:structure_starts",
                GenerationStage::StructureStarts,
            ),
            (
                "minecraft:structure_references",
                GenerationStage::StructureReferences,
            ),
        ] {
            let mut earlier = loaded(&document)?;
            earlier.status = Some(status.into());
            assert!(validate_empty_biomes_target(&earlier).is_err());
            assert_eq!(validate_empty_preterrain_target(&earlier, true)?, stage);
            earlier.data_version -= 1;
            assert!(validate_empty_preterrain_target(&earlier, true).is_err());
        }
        let mut unknown = loaded(&document)?;
        unknown.status = Some("example:unknown".into());
        assert!(validate_empty_preterrain_target(&unknown, true).is_err());
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
        assert!(validate_empty_preterrain_target(&later, true).is_err());
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
            let mut referenced_biome = None;
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
                    if x == 0 && z == 6 {
                        fields.retain(|(key, _)| !key.matches("entities"));
                        fields.push((
                            "entities".into(),
                            Tag::List {
                                element_type: TagType::Compound,
                                values: vec![Tag::Compound(vec![(
                                    "Passengers".into(),
                                    Tag::List {
                                        element_type: TagType::Compound,
                                        values: vec![Tag::Compound(vec![(
                                            "UUID".into(),
                                            Tag::IntArray(vec![9, 9, 9, 9]),
                                        )])],
                                    },
                                )])],
                            },
                        ));
                    }
                    let bytes = native_persistence::nbt::write(&biome_chunk)?;
                    if x == 4 && z == 4 {
                        let mut referenced = biome_chunk.clone();
                        let Tag::Compound(fields) = &mut referenced.value else {
                            unreachable!()
                        };
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
                        referenced_biome = Some(native_persistence::nbt::write(&referenced)?);
                    }
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
            // Disabling new starts must not discard references already saved
            // in a BIOMES-stage chunk.
            actual.generate_structures = false;
            let mut disabled_evaluator = native_resources::density::DensityEvaluator::default();
            assert!(
                actual
                    .generate_saved_structure_noise(clean_target, &[], &mut disabled_evaluator)
                    .is_err()
            );
            let disabled_noise = actual.generate_saved_structure_noise(
                clean_target,
                std::slice::from_ref(&treasure),
                &mut disabled_evaluator,
            )?;
            assert_eq!(disabled_noise.materials(), result.materials());
            assert_eq!(disabled_noise.fluid_updates(), result.fluid_updates());
            actual.generate_structures = true;
            if name == "overworld" {
                // Prepare a second world from actual disabled settings, rather
                // than merely toggling the flag on an existing generator.
                let disabled_world = tempfile::tempdir()?;
                let disabled_root = disabled_world.path();
                std::fs::create_dir_all(disabled_root.join("data/minecraft"))?;
                let disabled_settings = draft_baked_worldgen_settings(
                    &mut source,
                    "minecraft:normal",
                    NewWorldOptions {
                        seed: -42,
                        generate_structures: false,
                        bonus_chest: false,
                    },
                )?;
                std::fs::write(
                    disabled_root.join("data/minecraft/world_gen_settings.dat"),
                    native_persistence::nbt::write(&disabled_settings)?,
                )?;
                std::fs::copy(root.join("level.dat"), disabled_root.join("level.dat"))?;
                let disabled_region = disabled_root.join("dimensions/minecraft/overworld/region");
                std::fs::create_dir_all(&disabled_region)?;
                std::fs::copy(
                    region_dir.join("r.0.0.mca"),
                    disabled_region.join("r.0.0.mca"),
                )?;
                let bytes = referenced_biome
                    .as_ref()
                    .context("referenced biome missing")?;
                let mut region = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(disabled_region.join("r.0.0.mca"))?;
                let offset = region.seek(SeekFrom::End(0))?;
                let sectors = (bytes.len() + 5).div_ceil(4096);
                ensure!(
                    sectors < 256 && offset % 4096 == 0 && offset / 4096 < (1 << 24),
                    "invalid test region offset"
                );
                region.write_all(&(u32::try_from(bytes.len() + 1)?).to_be_bytes())?;
                region.write_all(&[3])?;
                region.write_all(bytes)?;
                region.write_all(&vec![0; sectors * 4096 - bytes.len() - 5])?;
                region.seek(SeekFrom::Start(((4 + 4 * 32) * 4) as u64))?;
                region
                    .write_all(&(((offset / 4096) as u32) << 8 | sectors as u32).to_be_bytes())?;
                region.flush()?;
                drop(region);
                let mut disabled_reader =
                    WorldReader::open_for_session(disabled_root, DimensionId::parse(name)?)?;
                let mut disabled_generator = prepare_existing_generator_for_schedule(
                    &mut disabled_reader,
                    &schedule,
                    &game,
                )?;
                assert!(!disabled_generator.generate_structures);
                let mut evaluator = native_resources::density::DensityEvaluator::default();
                let referenced_target = ChunkPos { x: 4, z: 4 };
                assert!(
                    disabled_generator
                        .generate_saved_terrain(referenced_target, &[], &mut evaluator)
                        .is_err()
                );
                let restored = disabled_generator.generate_saved_terrain(
                    referenced_target,
                    std::slice::from_ref(&treasure),
                    &mut evaluator,
                )?;
                disabled_generator.generate_structures = true;
                let enabled_terrain = disabled_generator.generate_saved_terrain(
                    referenced_target,
                    std::slice::from_ref(&treasure),
                    &mut native_resources::density::DensityEvaluator::default(),
                )?;
                assert_eq!(restored.states, enabled_terrain.states);
                assert_eq!(restored.fluid_updates, enabled_terrain.fluid_updates);
            }
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
            let mut mismatched_source = narrow.restore_saved_terrain([position])?;
            mismatched_source.sources.insert(
                position,
                Arc::new((**mismatched_source.sources.get(&position).unwrap()).clone()),
            );
            assert!(
                narrow
                    .capture_live_terrain(&live, &mismatched_source)
                    .is_err()
            );
            assert!(narrow.capture_live_poi(&live).is_err());
            live.set_initial_poi_chunk(key, native_session::loaded::SavedPoiData::Missing)?;
            let poi_guard = narrow.capture_live_poi(&live)?;
            live.verify_generation_poi(&poi_guard)?;
            assert!(narrow.capture_live_entities(&live).is_err());
            live.set_initial_entity_chunk(key, native_session::loaded::SavedEntityData::Empty)?;
            let entity_guard = narrow.capture_live_entities(&live)?;
            live.verify_generation_entities(&entity_guard)?;
            let live_inputs = narrow.capture_live_inputs(&live, &narrow_terrain)?;
            narrow.verify_live_inputs(&live, &live_inputs)?;
            assert!(actual.verify_live_inputs(&live, &live_inputs).is_err());

            if name == "overworld" {
                use native_session::worldgen::schedule::GenerationStage;
                let missing_key = native_world::loaded::ChunkPos { x: -1, z: 0 };
                let mut outside = GenerationSchedule::new();
                outside.request(
                    native_world::loaded::ChunkPos { x: 100, z: 100 },
                    GenerationStage::Biomes,
                )?;
                assert!(actual.begin_captured_biome_schedule(outside, []).is_err());
                let mut unrestored = GenerationSchedule::new();
                unrestored.request(key, GenerationStage::Biomes)?;
                assert!(
                    actual
                        .begin_captured_biome_schedule(unrestored, [position])
                        .is_err()
                );
                let mut biome_schedule = GenerationSchedule::new();
                biome_schedule.request(missing_key, GenerationStage::Biomes)?;
                // Synthetic structure stages stand in for the separately
                // restored structure data this API requires in production.
                let imported: Vec<_> = biome_schedule
                    .requirements()
                    .map(|(pos, _)| {
                        let stage = if pos == missing_key {
                            GenerationStage::StructureReferences
                        } else if pos == key {
                            GenerationStage::Biomes
                        } else {
                            GenerationStage::StructureStarts
                        };
                        (pos, stage)
                    })
                    .collect();
                biome_schedule.restore_imported(imported)?;
                let mut work = actual.begin_captured_biome_schedule(biome_schedule, [position])?;
                assert_eq!(
                    work.biomes().chunk([0, 0]).unwrap().values(),
                    chunk.values()
                );
                let mut evaluator = native_resources::density::DensityEvaluator::default();
                assert!(
                    narrow
                        .run_captured_biomes(&mut work, &mut evaluator, missing_key)
                        .is_err()
                );
                assert!(
                    actual
                        .run_captured_biomes(&mut work, &mut evaluator, key)
                        .is_err()
                );
                assert_eq!(
                    work.schedule().completed(missing_key),
                    Some(GenerationStage::StructureReferences)
                );
                actual.run_captured_biomes(&mut work, &mut evaluator, missing_key)?;
                assert_eq!(
                    work.schedule().completed(missing_key),
                    Some(GenerationStage::Biomes)
                );
                assert!(work.biomes().chunk([-1, 0]).is_some());
                assert!(
                    actual
                        .run_captured_biomes(&mut work, &mut evaluator, missing_key)
                        .is_err()
                );
                let (schedule, biomes, terrain, new_terrain) = work.into_parts();
                assert_eq!(
                    schedule.completed(missing_key),
                    Some(GenerationStage::Biomes)
                );
                assert!(biomes.chunk([0, 0]).is_some());
                assert!(terrain.is_empty());
                assert!(new_terrain.is_empty());
            }

            // A capture can include both a saved neighbor and a chunk that has
            // never been generated, or consist entirely of missing chunks.
            let missing = ChunkPos { x: 8, z: 0 };
            let missing_key = native_world::loaded::ChunkPos { x: 8, z: 0 };
            let context = reader.capture_generation_context([position, missing])?;
            let mut mixed = prepare_existing_generator(context, &game)?;
            let mixed_terrain = mixed.restore_captured_terrain()?;
            assert_eq!(mixed_terrain.world().chunks.len(), 1);
            assert!(mixed_terrain.world().chunks.contains_key(&key));
            assert!(!mixed_terrain.world().chunks.contains_key(&missing_key));
            let mixed_inputs = mixed.capture_live_inputs(&live, &mixed_terrain)?;
            mixed.verify_live_inputs(&live, &mixed_inputs)?;
            assert!(narrow.capture_live_terrain(&live, &mixed_terrain).is_err());

            let context = reader.capture_generation_context([missing])?;
            let mut empty = prepare_existing_generator(context, &game)?;
            let empty_terrain = empty.restore_captured_terrain()?;
            assert_eq!(empty_terrain.world().dimension, terrain.world().dimension);
            assert!(empty_terrain.world().chunks.is_empty());
            assert!(empty_terrain.biomes.is_empty());
            assert!(empty_terrain.heightmaps.is_empty());
            assert!(empty_terrain.postprocessing.is_empty());
            let empty_inputs = empty.capture_live_inputs(&live, &empty_terrain)?;
            empty.verify_live_inputs(&live, &empty_inputs)?;
            live.queue_streamed_chunk(missing_key, None, None, None, None, None);
            assert!(mixed.verify_live_inputs(&live, &mixed_inputs).is_err());
            assert!(empty.verify_live_inputs(&live, &empty_inputs).is_err());
            live.queue_streamed_chunk(key, None, None, None, None, None);
            assert!(live.verify_generation_terrain(&guard).is_err());
            assert!(live.verify_generation_poi(&poi_guard).is_err());
            assert!(live.verify_generation_entities(&entity_guard).is_err());
            assert!(narrow.verify_live_inputs(&live, &live_inputs).is_err());
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
            let mut biome_evaluator = native_resources::density::DensityEvaluator::default();
            let mixed_biomes =
                mixed.prepare_captured_biomes([position, missing], &mut biome_evaluator)?;
            assert_eq!(mixed_biomes.chunk([0, 0]).unwrap().values(), chunk.values());
            let expected_missing = reference.generate_biomes(
                [missing.x, missing.z],
                &mut native_resources::density::DensityEvaluator::default(),
            )?;
            assert_eq!(
                mixed_biomes.chunk([missing.x, missing.z]).unwrap().values(),
                expected_missing.values()
            );
            assert!(
                mixed
                    .prepare_captured_biomes([], &mut biome_evaluator)
                    .is_err()
            );
            assert!(
                mixed
                    .prepare_captured_biomes([missing, missing], &mut biome_evaluator)
                    .is_err()
            );
            assert!(
                mixed
                    .prepare_captured_biomes([ChunkPos { x: 100, z: 100 }], &mut biome_evaluator)
                    .is_err()
            );

            // This captured protochunk borders an absent column. Existing
            // palettes and new biome volumes must both feed the terrain pass.
            let boundary = ChunkPos { x: 6, z: 3 };
            let positions: Vec<_> = (2..=4)
                .flat_map(|z| (5..=7).map(move |x| ChunkPos { x, z }))
                .collect();
            let context = reader.capture_generation_context(positions.iter().copied())?;
            let mut boundary_generator = prepare_existing_generator(context, &game)?;
            let mut evaluator = native_resources::density::DensityEvaluator::default();
            assert!(
                boundary_generator
                    .generate_saved_terrain(boundary, &[], &mut evaluator)
                    .is_err()
            );
            assert!(
                boundary_generator
                    .generate_saved_terrain_with_biome_order(
                        boundary,
                        &[],
                        &mut evaluator,
                        &positions[..8]
                    )
                    .is_err()
            );
            let boundary_terrain = boundary_generator.generate_saved_terrain_with_biome_order(
                boundary,
                &[],
                &mut evaluator,
                &positions,
            )?;
            assert_eq!(boundary_terrain.chunk_position, [boundary.x, boundary.z]);
            assert_eq!(
                boundary_terrain.states.len(),
                lighting.height as usize * 256
            );
            if name == "overworld" {
                use native_session::worldgen::schedule::GenerationStage;
                let target = native_world::loaded::ChunkPos {
                    x: boundary.x,
                    z: boundary.z,
                };
                let mut plan = GenerationSchedule::new();
                plan.request(target, GenerationStage::Terrain)?;
                // This test isolates the biome/terrain transition; structure
                // stages below are synthetic, not independently restored.
                let imported: Vec<_> = plan
                    .requirements()
                    .map(|(pos, _)| {
                        let stage = if (5..=7).contains(&pos.x) && (2..=4).contains(&pos.z) {
                            if pos.x == 7 {
                                GenerationStage::StructureReferences
                            } else {
                                GenerationStage::Biomes
                            }
                        } else {
                            GenerationStage::StructureStarts
                        };
                        (pos, stage)
                    })
                    .collect();
                plan.restore_imported(imported)?;
                let mut scheduled =
                    prepare_existing_generator_for_schedule(&mut reader, &plan, &game)?;
                let saved = positions.iter().copied().filter(|pos| pos.x != 7);
                let mut work = scheduled.begin_captured_biome_schedule(plan, saved)?;
                let mut evaluator = native_resources::density::DensityEvaluator::default();
                assert!(
                    scheduled
                        .run_captured_saved_terrain(&mut work, &mut evaluator, target, &[])
                        .is_err()
                );
                assert_eq!(
                    work.schedule().completed(target),
                    Some(GenerationStage::Biomes)
                );
                assert!(work.terrain_chunk(boundary).is_none());
                for z in 2..=4 {
                    scheduled.run_captured_biomes(
                        &mut work,
                        &mut evaluator,
                        native_world::loaded::ChunkPos { x: 7, z },
                    )?;
                }
                assert!(work.schedule().ready(target, GenerationStage::Terrain));
                assert!(
                    boundary_generator
                        .run_captured_saved_terrain(&mut work, &mut evaluator, target, &[])
                        .is_err()
                );
                scheduled.run_captured_saved_terrain(&mut work, &mut evaluator, target, &[])?;
                assert_eq!(
                    work.schedule().completed(target),
                    Some(GenerationStage::Terrain)
                );
                let chunk = work
                    .terrain_chunk(boundary)
                    .context("scheduled terrain missing")?;
                let registry = &scheduled.terrain_assets.as_ref().unwrap().blocks;
                for z in [0usize, 15] {
                    for x in [0usize, 15] {
                        for y in 0..lighting.height as usize {
                            let index = (z * 16 + x) * lighting.height as usize + y;
                            let expected = resolve_generated_terrain_state(
                                &boundary_terrain.states[index],
                                registry,
                            )?;
                            assert_eq!(
                                chunk.materialized().world().block(
                                    boundary.x * 16 + x as i32,
                                    lighting.min_y + y as i32,
                                    boundary.z * 16 + z as i32
                                ),
                                Some(&expected)
                            );
                        }
                    }
                }
                let draft = scheduled.draft_resumed_sections(chunk)?;
                assert!(draft.matches_source(&chunk.source)?);
                assert!(
                    scheduled
                        .run_captured_saved_terrain(&mut work, &mut evaluator, target, &[])
                        .is_err()
                );
                let (plan, _, terrain, new_terrain) = work.into_parts();
                assert_eq!(plan.completed(target), Some(GenerationStage::Terrain));
                assert!(terrain.contains_key(&boundary));
                assert!(new_terrain.is_empty());
            }
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
        // Keep the synthetic fixed-plains fixture above its sea level so the
        // creature checks exercise real spawnable ground rather than ocean.
        let noise_file = pack.join("data/minecraft/worldgen/noise_settings/overworld.json");
        std::fs::create_dir_all(noise_file.parent().unwrap())?;
        let mut noise: Value = serde_json::from_slice(
            &source
                .read_data("data/minecraft/worldgen/noise_settings/overworld.json")?
                .context("vanilla noise settings missing")?,
        )?;
        noise["sea_level"] = json!(-64);
        noise
            .as_object_mut()
            .context("noise settings are not an object")?
            .remove("aquifers");
        std::fs::write(&noise_file, serde_json::to_vec(&noise)?)?;
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
        let neighbors = (2..=4)
            .flat_map(|z| (2..=4).map(move |x| native_world::loaded::ChunkPos { x, z }))
            .collect::<Vec<_>>();
        let before_heightmaps = neighbors
            .iter()
            .map(|&pos| terrain.terrain().generated_heightmap_data(pos))
            .collect::<Result<Vec<_>>>()?;
        assert!(
            neighbors
                .iter()
                .any(|&pos| terrain.terrain().worldgen_heightmaps(pos).is_none()),
            "the rollback check requires at least one unprimed heightmap"
        );
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
        assert_eq!(
            neighbors
                .iter()
                .map(|&pos| terrain.terrain().generated_heightmap_data(pos))
                .collect::<Result<Vec<_>>>()?,
            before_heightmaps,
            "a rejected feature pass must not prime any heightmaps"
        );
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
        assert_eq!(
            neighbors
                .iter()
                .map(|&pos| terrain.terrain().generated_heightmap_data(pos))
                .collect::<Result<Vec<_>>>()?,
            before_heightmaps,
            "a failed light-dependent feature must leave heightmaps untouched"
        );
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
        let mut planned_schedule = GenerationSchedule::new();
        planned_schedule.request(center, GenerationStage::Spawn)?;
        let imported = planned_schedule
            .requirements()
            .map(|(pos, _)| {
                (
                    pos,
                    schedule.completed(pos).unwrap_or(GenerationStage::Empty),
                )
            })
            .collect::<Vec<_>>();
        planned_schedule.restore_imported(imported)?;
        let mut rejected_schedule = GenerationSchedule::new();
        rejected_schedule.request(center, GenerationStage::Spawn)?;
        let imported = rejected_schedule
            .requirements()
            .map(|(pos, _)| {
                (
                    pos,
                    planned_schedule
                        .completed(pos)
                        .unwrap_or(GenerationStage::Empty),
                )
            })
            .collect::<Vec<_>>();
        rejected_schedule.restore_imported(imported)?;
        let clocks: BTreeMap<_, _> =
            native_resources::world_clock::registered_clocks(&mut actual.resources)?
                .into_iter()
                .map(|id| (id, 0))
                .collect();
        let mut forced = actual.plan_resumed_creatures(
            &terrain,
            &planned_schedule,
            &staged,
            &custom_decoration,
            true,
            &clocks,
        )?;
        let attrs = json!({"gameplay/natural_mob_spawns":{"spawns_by_category":{"creature":[{"type":"minecraft:pig","weight":1,"count":1}]},"spawn_costs":{}}});
        let settings = native_resources::mob_spawns::resolve_creature_spawns(
            &json!({}),
            &attrs,
            &json!({}),
            &[],
            |_| Some(false),
        )?;
        forced.planner = native_resources::worldgen_mob_spawner::ChunkCreatureSpawner::new(
            -42,
            center.x,
            center.z,
            true,
            0.2,
            settings.spawns,
        )?;
        let attempt = forced.advance(&terrain, &staged, true, &clocks)?;
        assert!(matches!(
            attempt,
            native_resources::worldgen_mob_spawner::CreatureSpawnProgress::Attempt(_)
        ));
        assert_eq!(forced.advance(&terrain, &staged, true, &clocks)?, attempt);
        let excluded = native_session::respawn::RespawnBorder {
            min_x: 1000.0,
            max_x: 1001.0,
            min_z: 1000.0,
            max_z: 1001.0,
        };
        let mut rejected = false;
        for _ in 0..16 {
            match actual.check_resumed_creature_candidate(
                &terrain,
                &rejected_schedule,
                &staged,
                &mut forced,
                true,
                &clocks,
                excluded,
                64,
            )? {
                ResumedCreatureCheck::Pending => {}
                ResumedCreatureCheck::Rejected => {
                    rejected = true;
                    break;
                }
                ResumedCreatureCheck::NeedsConstruction(_) => {
                    bail!("out-of-border creature was accepted")
                }
            }
        }
        assert!(rejected, "excluded candidate did not finish placement");
        let mut finished = false;
        for _ in 0..128 {
            match forced.advance(&terrain, &staged, true, &clocks)? {
                native_resources::worldgen_mob_spawner::CreatureSpawnProgress::Finished => {
                    finished = true;
                    break;
                }
                native_resources::worldgen_mob_spawner::CreatureSpawnProgress::Pending => {}
                native_resources::worldgen_mob_spawner::CreatureSpawnProgress::Attempt(_) => {
                    for _ in 0..16 {
                        match actual.check_resumed_creature_candidate(
                            &terrain,
                            &rejected_schedule,
                            &staged,
                            &mut forced,
                            true,
                            &clocks,
                            excluded,
                            64,
                        )? {
                            ResumedCreatureCheck::Pending => {}
                            ResumedCreatureCheck::Rejected => break,
                            ResumedCreatureCheck::NeedsConstruction(_) => {
                                bail!("out-of-border creature was accepted")
                            }
                        }
                    }
                }
            }
        }
        assert!(finished, "rejected candidates did not finish");
        let rejected_draft = actual.finish_resumed_creatures(
            &terrain,
            &mut rejected_schedule,
            &staged,
            forced,
            true,
            &clocks,
            Some(excluded),
        )?;
        assert!(rejected_draft.matches_state(&terrain, &staged, true, &clocks, Some(excluded)));
        assert!(!rejected_draft.matches_state(&terrain, &staged, true, &clocks, None));
        assert_eq!(
            rejected_schedule.completed(center),
            Some(GenerationStage::Spawn)
        );
        for species in ["sheep", "pig", "cow", "chicken"] {
            let mut animal_schedule = GenerationSchedule::new();
            animal_schedule.request(center, GenerationStage::Spawn)?;
            let imported = animal_schedule
                .requirements()
                .map(|(pos, _)| {
                    (
                        pos,
                        planned_schedule
                            .completed(pos)
                            .unwrap_or(GenerationStage::Empty),
                    )
                })
                .collect::<Vec<_>>();
            animal_schedule.restore_imported(imported)?;
            let mut animal = actual.plan_resumed_creatures(
                &terrain,
                &animal_schedule,
                &staged,
                &custom_decoration,
                true,
                &clocks,
            )?;
            let attrs = json!({"gameplay/natural_mob_spawns":{"spawns_by_category":{"creature":[{"type":format!("minecraft:{species}"),"weight":1,"count":1}]},"spawn_costs":{}}});
            let sheep = native_resources::mob_spawns::resolve_creature_spawns(
                &json!({}),
                &attrs,
                &json!({}),
                &[],
                |_| Some(false),
            )?;
            animal.planner = native_resources::worldgen_mob_spawner::ChunkCreatureSpawner::new(
                -42,
                center.x,
                center.z,
                true,
                0.8,
                sheep.spawns,
            )?;
            let border = native_session::respawn::RespawnBorder {
                min_x: -1000.0,
                max_x: 1000.0,
                min_z: -1000.0,
                max_z: 1000.0,
            };
            let mut generated = 0i32;
            let mut finished = false;
            for _ in 0..512 {
                match animal.advance(&terrain, &staged, true, &clocks)? {
                    native_resources::worldgen_mob_spawner::CreatureSpawnProgress::Finished => {
                        finished = true;
                        break;
                    }
                    native_resources::worldgen_mob_spawner::CreatureSpawnProgress::Pending => {}
                    native_resources::worldgen_mob_spawner::CreatureSpawnProgress::Attempt(_) => {
                        for _ in 0..16 {
                            match actual.check_resumed_creature_candidate(
                                &terrain,
                                &animal_schedule,
                                &staged,
                                &mut animal,
                                true,
                                &clocks,
                                border,
                                64,
                            )? {
                                ResumedCreatureCheck::Pending => {}
                                ResumedCreatureCheck::Rejected => break,
                                ResumedCreatureCheck::NeedsConstruction(_) => {
                                    generated += 1;
                                    let duplicate =
                                        native_session::worldgen::FreshCreatureConstruction {
                                            uuid: [9, 9, 9, 9],
                                            chicken_egg_time: (species == "chicken")
                                                .then_some(9000),
                                        };
                                    assert!(
                                        actual
                                            .construct_resumed_creature(
                                                &terrain,
                                                &animal_schedule,
                                                &staged,
                                                &mut animal,
                                                true,
                                                &clocks,
                                                border,
                                                duplicate,
                                            )
                                            .is_err()
                                    );
                                    let construction =
                                        native_session::worldgen::FreshCreatureConstruction {
                                            uuid: [1, 2, 3, generated],
                                            chicken_egg_time: (species == "chicken")
                                                .then_some(9000),
                                        };
                                    if species == "chicken" {
                                        let invalid =
                                            native_session::worldgen::FreshCreatureConstruction {
                                                uuid: [1, 2, 3, generated],
                                                chicken_egg_time: None,
                                            };
                                        assert!(
                                            actual
                                                .construct_resumed_creature(
                                                    &terrain,
                                                    &animal_schedule,
                                                    &staged,
                                                    &mut animal,
                                                    true,
                                                    &clocks,
                                                    border,
                                                    invalid
                                                )
                                                .is_err()
                                        );
                                    }
                                    actual.construct_resumed_creature(
                                        &terrain,
                                        &animal_schedule,
                                        &staged,
                                        &mut animal,
                                        true,
                                        &clocks,
                                        border,
                                        construction,
                                    )?;
                                    match actual.check_resumed_creature_instance(
                                        &terrain,
                                        &animal_schedule,
                                        &staged,
                                        &mut animal,
                                        true,
                                        &clocks,
                                        border,
                                    )? {
                                        ResumedCreatureInstance::Rejected => {}
                                        ResumedCreatureInstance::NeedsFinalization(_) => actual
                                            .finalize_resumed_creature(
                                                &terrain,
                                                &animal_schedule,
                                                &staged,
                                                &custom_decoration,
                                                &mut animal,
                                                true,
                                                &clocks,
                                                border,
                                            )?,
                                    }
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            assert!(finished, "animal spawning did not finish");
            if animal.additions.is_empty() {
                let snapshot = terrain.terrain().snapshot();
                for (x, z) in [(48, 48), (56, 56), (63, 63)] {
                    let top = (actual.lighting.min_y
                        ..actual.lighting.min_y + actual.lighting.height)
                        .rev()
                        .find_map(|y| {
                            let state = snapshot.block(x, y, z)?;
                            (!matches!(
                                state.name.as_str(),
                                "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                            ))
                            .then(|| (y, state.name.clone()))
                        });
                    eprintln!("mob test top {x},{z}: {top:?}");
                }
                eprintln!("constructed candidates: {generated}");
            }
            assert!(
                !animal.additions.is_empty(),
                "no {species} passed the generated terrain checks"
            );
            let count = animal.additions.len();
            let animal_draft = actual.finish_resumed_creatures(
                &terrain,
                &mut animal_schedule,
                &staged,
                animal,
                true,
                &clocks,
                Some(border),
            )?;
            let Some(Tag::List {
                values: entities, ..
            }) = animal_draft.document().value.get("entities")
            else {
                bail!("generated animal records missing")
            };
            assert_eq!(entities.len(), count);
            let expected_id = format!("minecraft:{species}");
            assert!(entities.iter().all(|entity| matches!(entity.get("id"), Some(Tag::String(id)) if id.matches(&expected_id))));
            let animal_file = root.join("resumed-animals.nbt");
            std::fs::write(
                &animal_file,
                native_persistence::nbt::write(animal_draft.document())?,
            )?;
            let checked = std::process::Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&classpath)
                .arg("ResumedLightOracle")
                .arg(&animal_file)
                .arg(actual.lighting.min_y.to_string())
                .arg(actual.lighting.height.to_string())
                .arg(format!("entities:{species}"))
                .output()?;
            ensure!(
                checked.status.success(),
                "Java animal validation failed: {}\n{}",
                String::from_utf8_lossy(&checked.stdout),
                String::from_utf8_lossy(&checked.stderr)
            );
        }
        let forced = actual.plan_resumed_creatures(
            &terrain,
            &planned_schedule,
            &staged,
            &custom_decoration,
            true,
            &clocks,
        )?;
        assert!(
            actual
                .finish_resumed_creatures(
                    &terrain,
                    &mut planned_schedule,
                    &staged,
                    forced,
                    true,
                    &clocks,
                    None
                )
                .is_err()
        );
        assert_eq!(
            planned_schedule.completed(center),
            Some(GenerationStage::Light)
        );
        let mut plan = actual.plan_resumed_creatures(
            &terrain,
            &planned_schedule,
            &staged,
            &custom_decoration,
            true,
            &clocks,
        )?;
        let mut changed_clocks = clocks.clone();
        changed_clocks.insert(ResourceId::parse("example:changed")?, 1);
        assert!(
            plan.advance(&terrain, &staged, true, &changed_clocks)
                .is_err()
        );
        assert_eq!(
            plan.advance(&terrain, &staged, true, &clocks)?,
            native_resources::worldgen_mob_spawner::CreatureSpawnProgress::Finished
        );
        let planned = actual.finish_resumed_creatures(
            &terrain,
            &mut planned_schedule,
            &staged,
            plan,
            true,
            &clocks,
            None,
        )?;
        assert_eq!(
            planned_schedule.completed(center),
            Some(GenerationStage::Spawn)
        );
        assert!(planned.matches_state(&terrain, &staged, true, &clocks, None));
        assert!(planned.matches_source(&terrain.sources[&saved_center])?);
        assert!(!planned.matches_state(&terrain, &staged, true, &changed_clocks, None));
        assert_eq!(planned.document(), lit.document());
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
        let mut captured_biomes = structured.generator.biome_neighborhood()?;
        for z in 2..=4 {
            for x in 2..=4 {
                captured_biomes.insert(Arc::clone(
                    decoration.biomes.chunk([x, z]).context("biome missing")?,
                ))?;
            }
        }
        let mut captured_batch = CapturedTerrainBatch {
            identity: Arc::clone(&structured.identity),
            schedule: schedule.clone(),
            biomes: captured_biomes,
            terrain: terrain.terrain.clone(),
            sources: terrain
                .sources
                .iter()
                .map(|(pos, source)| (*pos, Some(Arc::clone(source))))
                .collect(),
            saved_stages: BTreeMap::new(),
            generated_references: BTreeMap::new(),
            generated_starts: BTreeMap::new(),
            treasure_starts: BTreeMap::new(),
            feature_entity_sources: ResumedFeatureEntitySources::default(),
            revision: Arc::new(()),
        };
        let captured_context =
            structured.prepare_captured_decoration(&captured_batch, saved_center)?;
        let captured_light =
            structured.bind_captured_initial_feature_light(&captured_batch, light.clone())?;
        let before = captured_batch.terrain().snapshot();
        let reference_tag = Tag::Compound(vec![(
            "minecraft:buried_treasure".into(),
            Tag::LongArray(vec![((center.z as i64) << 32) | (center.x as u32 as i64)]),
        )]);
        let references =
            native_session::worldgen::structure_references::restore_saved_structure_references(
                center,
                Some(&reference_tag),
                |_| Ok(true),
            )?;
        assert!(references.discarded.is_empty());
        captured_batch.generated_references.insert(
            center,
            CapturedGeneratedReferences {
                original: None,
                references: references.references,
            },
        );
        assert!(
            structured
                .captured_structures_at(&captured_batch, [48, 0, 48])
                .is_err()
        );
        let error = structured
            .run_captured_features_with_initial_light(
                &mut captured_batch,
                &captured_context,
                &captured_light,
            )
            .expect_err("a nonempty structure plan must not be skipped");
        assert!(format!("{error:#}").contains("referenced structure start missing"));
        assert!(Arc::ptr_eq(&before, &captured_batch.terrain().snapshot()));
        assert_eq!(
            captured_batch.schedule().completed(center),
            Some(GenerationStage::Terrain)
        );
        captured_batch.generated_references.remove(&center);
        let empty_references =
            native_session::worldgen::structure_references::restore_saved_structure_references(
                center,
                None,
                |_| Ok(true),
            )?;
        captured_batch.generated_references.insert(
            center,
            CapturedGeneratedReferences {
                original: None,
                references: empty_references.references,
            },
        );
        assert!(
            structured
                .captured_structures_at(&captured_batch, [48, 0, 48])?
                .is_empty()
        );
        let before_maps = captured_batch.terrain().generated_heightmap_data(center)?;
        assert!(
            structured
                .run_captured_features(&mut captured_batch, &captured_context)
                .is_err()
        );
        assert_eq!(
            captured_batch.terrain().generated_heightmap_data(center)?,
            before_maps
        );
        assert!(Arc::ptr_eq(&before, &captured_batch.terrain().snapshot()));
        assert_eq!(
            captured_batch.schedule().completed(center),
            Some(GenerationStage::Terrain)
        );
        captured_batch.generated_references.remove(&center);
        assert_eq!(
            structured.run_captured_features_with_initial_light(
                &mut captured_batch,
                &captured_context,
                &captured_light,
            )?,
            vec![true]
        );
        assert!(!captured_light.matches_batch(&captured_batch));
        assert_eq!(
            captured_batch.schedule().completed(center),
            Some(GenerationStage::Features)
        );
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
        let captured_snapshot = captured_batch.terrain().snapshot();
        for y in structured.lighting.min_y..structured.lighting.min_y + structured.lighting.height {
            for z in 32..80 {
                for x in 32..80 {
                    assert_eq!(
                        structured_snapshot.block(x, y, z),
                        no_structures_snapshot.block(x, y, z),
                        "empty structure plan changed block {x}, {y}, {z}"
                    );
                    assert_eq!(
                        structured_snapshot.block(x, y, z),
                        captured_snapshot.block(x, y, z),
                        "captured empty structure plan changed block {x}, {y}, {z}"
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
                            ("UUID".into(), Tag::IntArray(vec![7, 7, 7, 7])),
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
        assert!(
            restored
                .check_known_entity_id(&terrain, [7, 7, 7, 7])
                .is_err()
        );
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
        let mut captured_biomes = restored.generator.biome_neighborhood()?;
        for z in 2..=4 {
            for x in 2..=4 {
                captured_biomes.insert(Arc::clone(
                    decoration.biomes.chunk([x, z]).context("biome missing")?,
                ))?;
            }
        }
        let mut captured = CapturedTerrainBatch {
            identity: Arc::clone(&restored.identity),
            schedule: schedule.clone(),
            biomes: captured_biomes,
            terrain: terrain.terrain.clone(),
            sources: terrain
                .sources
                .iter()
                .map(|(pos, source)| (*pos, Some(Arc::clone(source))))
                .collect(),
            saved_stages: BTreeMap::new(),
            generated_references: BTreeMap::new(),
            generated_starts: BTreeMap::new(),
            treasure_starts: BTreeMap::new(),
            feature_entity_sources: ResumedFeatureEntitySources::default(),
            revision: Arc::new(()),
        };
        let captured_context = restored.prepare_captured_decoration(&captured, saved_center)?;
        let captured_light =
            restored.bind_captured_initial_feature_light(&captured, light.clone())?;
        let before = captured.terrain().snapshot();
        assert!(
            restored
                .run_captured_features(&mut captured, &captured_context)
                .is_err()
        );
        assert!(captured.treasure_starts.is_empty());
        assert!(Arc::ptr_eq(&before, &captured.terrain().snapshot()));
        assert_eq!(
            captured.schedule().completed(center),
            Some(GenerationStage::Terrain)
        );
        assert_eq!(
            restored.run_captured_features_with_initial_light(
                &mut captured,
                &captured_context,
                &captured_light
            )?,
            vec![true]
        );
        assert!(!captured_light.matches_batch(&captured));
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
        let captured_draft = restored.draft_captured_structure_starts(&captured, saved_center)?;
        assert_eq!(captured_draft.document(), draft.document());
        assert!(captured_draft.matches_source(&treasure.source)?);
        let captured_sources = restored.draft_captured_structure_sources(&captured)?;
        assert_eq!(captured_sources.len(), 1);
        assert_eq!(captured_sources[&center].document(), draft.document());
        assert!(captured_sources[&center].matches_source(&treasure.source)?);
        assert_eq!(
            captured.treasure_starts[&key].placement.bounds,
            treasure.placement.bounds
        );
        let captured_world = captured.terrain().snapshot();
        let normal_world = terrain.terrain().snapshot();
        for yy in restored.lighting.min_y..restored.lighting.min_y + restored.lighting.height {
            for zz in 32..80 {
                for xx in 32..80 {
                    assert_eq!(
                        captured_world.block(xx, yy, zz),
                        normal_world.block(xx, yy, zz)
                    );
                }
            }
        }
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
        let treasure_point = treasure.placement.bounds.min;
        assert!(
            restored
                .resumed_structures_at(&terrain, treasure_point)?
                .contains(&treasure_id)
        );
        assert_eq!(
            restored.captured_structures_at(&captured, treasure_point)?,
            restored.resumed_structures_at(&terrain, treasure_point)?
        );
        assert!(
            restored
                .captured_structures_at(&captured, [1_000_000, 0, 1_000_000])
                .is_err()
        );
        {
            use native_resources::animal_variants::SpawnVariantEnvironment;
            let count = std::cell::Cell::new(0);
            let mut resolve = || {
                count.set(count.get() + 1);
                restored.resumed_structures_at(&terrain, treasure_point)
            };
            let biome = ResourceId::parse("minecraft:plains")?;
            let mut environment = ResumedFarmEnvironment {
                biome: &biome,
                moon: None,
                structures: None,
                resolve_structures: &mut resolve,
            };
            assert_eq!(environment.biome()?, &biome);
            assert_eq!(count.get(), 0);
            assert!(environment.structures_with_piece()?.contains(&treasure_id));
            assert!(environment.structures_with_piece()?.contains(&treasure_id));
            assert_eq!(count.get(), 1);
        }
        assert!(
            !restored
                .resumed_structures_at(
                    &terrain,
                    [
                        treasure_point[0],
                        treasure_point[1] + 1000,
                        treasure_point[2]
                    ]
                )?
                .contains(&treasure_id)
        );
        // Exercise the same entity-recording path used by generation, including
        // passenger suppression and a serializable empty compound.
        let generated_one = Tag::Compound(vec![
            ("n".into(), Tag::Int(21)),
            ("UUID".into(), Tag::IntArray(vec![8, 8, 8, 8])),
        ]);
        let generated_empty = Tag::Compound(vec![]);
        let generated_two = Tag::Compound(vec![("n".into(), Tag::Int(22))]);
        restored.check_known_entity_id(&terrain, [8, 8, 8, 8])?;
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
        assert!(
            restored
                .check_known_entity_id(&terrain, [8, 8, 8, 8])
                .is_err()
        );
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
