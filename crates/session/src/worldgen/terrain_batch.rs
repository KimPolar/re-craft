//! Incremental execution of intermediate fresh terrain generation.
use anyhow::{Context, Result, bail, ensure};
use native_resources::{
    block_registry::GeneratedBlockRegistry,
    carver::{schedule::CarverCache, tags::CarverBlockTags},
    density::DensityEvaluator,
    dimension_lighting::GeneratedDimensionLighting,
    feature_order::{FeatureKey, FeatureSchedule, FeatureTags},
    packs::{ResourceId, ResourceProvider},
    placed_feature::PlacedFeature,
    structure_decoration::StructureDecorationSchedule,
};
use native_world::generation_light::VisibleLightSnapshot;
use native_world::loaded::{BlockState, ChunkPos, World};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use super::{
    BiomeNeighborhood, CarvedTerrainWorld, CarverBlockState, FreshGenerationLightTracker,
    PreparedDecoration, StagedGenerationLightEngine, TerrainGenerator,
    schedule::{
        GenerationSchedule, GenerationStage, HutFeatureCallbacks, HutFeatureRunner,
        MineshaftFeatureCallbacks, MixedFeatureCallbacks, NativeDrownedFeatureRunner,
        NativeHutFeatureRunner, NativeMixedFeatureRunner, OceanFeatureCallbacks,
        OceanFeatureRunner, StructureFeatureLighting,
    },
    structure_batch::{PreparedStructureStages, StructureGenerationBatch, TerrainStartKey},
};

/// A single-threaded, incrementally driven batch for fresh generation.
/// Its generator, provider, block registry and tags must belong to the same
/// immutable pack stack. Structure-aware batches support buried treasure and,
/// when explicitly supplied with complete spawn context, swamp huts;
/// old-chunk blending and saving generated structures are not supported.
/// Its deterministic work order has not been proven to match Java's thread
/// scheduling for climate ties. Outputs remain intermediate and unsaveable.
pub struct FreshTerrainBatch {
    identity: Arc<()>,
    schedule: GenerationSchedule,
    generator: TerrainGenerator,
    structures: Option<PreparedStructureStages>,
    structure_continuation: Option<StructureGenerationBatch>,
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
    spawn_ready: BTreeSet<ChunkPos>,
    entity_sources: GeneratedEntitySources,
    failed: bool,
}

/// Resolves only the conditions encountered by the active variant selectors.
struct FarmAnimalEnvironment<'a> {
    batch: &'a FreshTerrainBatch,
    position: [i32; 3],
    biome: &'a ResourceId,
    moon: Option<(
        &'a native_resources::moon_phase::Environment,
        &'a BTreeMap<ResourceId, i64>,
    )>,
    structures: Option<BTreeSet<ResourceId>>,
}

impl native_resources::animal_variants::SpawnVariantEnvironment for FarmAnimalEnvironment<'_> {
    fn biome(&self) -> Result<&ResourceId> {
        Ok(self.biome)
    }

    fn moon_phase(&mut self) -> Result<u8> {
        let (moon, clocks) = self.moon.context("moon phase environment unavailable")?;
        let biome = moon
            .has_biome_layer
            .then(|| format!("{}:{}", self.biome.namespace, self.biome.path));
        Ok(moon
            .sample(biome.as_deref(), |id| clocks.get(id).copied())
            .context("moon phase environment or world clock unavailable")?
            .index() as u8)
    }

    fn structures_with_piece(&mut self) -> Result<&BTreeSet<ResourceId>> {
        if self.structures.is_none() {
            self.structures = Some(structures_with_piece_at(
                self.batch.structures.as_ref(),
                self.position,
            )?);
        }
        Ok(self
            .structures
            .as_ref()
            .expect("structure set was initialized"))
    }
}

/// Coordinated postprocessing output for one chunk. Block entities, entity
/// regions, structures and other world-save requirements remain separate.
pub struct PostprocessedChunkComponents {
    pub sections: native_persistence::nbt::Tag,
    pub heightmaps: native_persistence::nbt::Tag,
    pub block_ticks: native_persistence::nbt::Tag,
    pub fluid_ticks: native_persistence::nbt::Tag,
    /// Generated block entities whose blocks remain unchanged after processing.
    pub block_entities: native_persistence::nbt::Tag,
    /// Complete entity records, or `None` if an entity source is unverified.
    pub entity_records: Option<Arc<Vec<native_persistence::nbt::Tag>>>,
}

/// An in-memory FULL chunk and any complete entity records for the same pass.
/// Neither this draft nor its entity records authorize a world save.
pub struct PostprocessedChunkDraft {
    pub chunk: native_persistence::nbt::NamedTag,
    pub entity_records: Option<Arc<Vec<native_persistence::nbt::Tag>>>,
    /// Present only when the complete entity records are known. An empty
    /// entity list still needs the correct Java storage action at save time.
    pub entity_chunk: Option<native_persistence::nbt::NamedTag>,
}

/// Summary of an in-memory structure-region preview. The underlying drafts
/// are discarded and this value cannot publish or stage a world.
#[derive(Debug, PartialEq, Eq)]
pub struct StructureRegionPreview {
    pub chunk_count: usize,
    pub referenced_start_chunks: BTreeSet<ChunkPos>,
    pub terrain_regions: usize,
    pub entity_regions: usize,
    pub poi_regions: usize,
}

/// Structure-aware region drafts prepared from a self-contained selection.
/// These drafts are intermediate and do not authorize world publication.
pub struct PreparedStructureRegions {
    pub chunk_count: usize,
    pub referenced_start_chunks: BTreeSet<ChunkPos>,
    pub regions: BTreeMap<
        (native_persistence::save::FreshRegionKind, i32, i32),
        native_persistence::save::FreshRegionDraft,
    >,
}

/// A sealed proof of the inputs used by one completed postprocessing pass.
/// The region remains owned by the caller and is unchanged on a failed rebase.
pub struct PostprocessingExpansionCheckpoint {
    batch_identity: Arc<()>,
    region_identity: Arc<()>,
    source: Arc<World>,
    processed_world: Arc<World>,
    revision: u64,
    chunks: BTreeSet<ChunkPos>,
    converted: BTreeSet<ChunkPos>,
    edits: Vec<native_world::loaded::BlockPosition>,
    read_chunks: BTreeSet<ChunkPos>,
    light_reads: BTreeSet<ChunkPos>,
    light_checks: Vec<native_world::loaded::BlockPosition>,
    inputs: BTreeMap<ChunkPos, PostprocessingSourceInputs>,
    active_light: BTreeSet<ChunkPos>,
    light: VisibleLightSnapshot,
    sky_sources: BTreeMap<ChunkPos, Arc<[i32; 256]>>,
}

#[derive(PartialEq)]
struct PostprocessingSourceInputs {
    heightmaps: native_persistence::nbt::Tag,
    block_entities: native_persistence::nbt::Tag,
    block_ticks: native_persistence::nbt::Tag,
    fluid_ticks: native_persistence::nbt::Tag,
    postprocessing: native_persistence::nbt::Tag,
    entities: Vec<native_persistence::nbt::Tag>,
    complete_entities: Option<Vec<native_persistence::nbt::Tag>>,
}

fn unchanged_generation_world_for(
    before: &World,
    after: &World,
    observed: &BTreeSet<ChunkPos>,
) -> bool {
    if before.dimension != after.dimension
        || before.dimension_type != after.dimension_type
        || before.inline_dimension_type != after.inline_dimension_type
        || before.biome_zoom_seed() != after.biome_zoom_seed()
        || !before
            .states()
            .all(|(id, state)| after.state(id) == Some(state))
        || before.biome_count() > after.biome_count()
        || !before.biomes().zip(after.biomes()).all(|(a, b)| a == b)
        || !before
            .block_edits()
            .filter(|edit| observed.contains(&edit.position.chunk()))
            .eq(after
                .block_edits()
                .filter(|edit| observed.contains(&edit.position.chunk())))
    {
        return false;
    }
    observed.iter().all(|pos| {
        let Some(old) = before.chunks.get(pos) else {
            return false;
        };
        let Some(new) = after.chunks.get(pos) else {
            return false;
        };
        let sections_match = Arc::ptr_eq(old, new)
            || (old.position == new.position
                && old.status == new.status
                && old.block_light_correct == new.block_light_correct
                && old.sections.len() == new.sections.len()
                && old.sections.iter().all(|(y, a)| {
                    new.sections.get(y).is_some_and(|b| {
                        Arc::ptr_eq(a, b)
                            || (a.blocks == b.blocks
                                && a.biomes == b.biomes
                                && a.block_light == b.block_light
                                && a.sky_light == b.sky_light)
                    })
                }));
        sections_match
            && before.missing_chunks.contains(pos) == after.missing_chunks.contains(pos)
            && before.saved_block_ticks.get(pos) == after.saved_block_ticks.get(pos)
            && before.saved_fluid_ticks.get(pos) == after.saved_fluid_ticks.get(pos)
            && before.saved_block_entities.get(pos) == after.saved_block_entities.get(pos)
            && before.live_block_entities.get(pos) == after.live_block_entities.get(pos)
    })
}

fn postprocessing_observed_chunks(
    processed: &BTreeSet<ChunkPos>,
    reads: &BTreeSet<ChunkPos>,
    edits: &[native_world::loaded::BlockPosition],
) -> Option<BTreeSet<ChunkPos>> {
    if reads.is_subset(processed) {
        Some(
            processed
                .iter()
                .copied()
                .chain(edits.iter().map(|edit| edit.chunk()))
                .collect(),
        )
    } else {
        None
    }
}

#[cfg(test)]
fn unchanged_generation_world(before: &World, after: &World) -> bool {
    unchanged_generation_world_for(before, after, &before.chunks.keys().copied().collect())
}

fn unchanged_visible_light(
    before: &VisibleLightSnapshot,
    after: &VisibleLightSnapshot,
    positions: &BTreeSet<ChunkPos>,
) -> bool {
    if before.has_block_channel() != after.has_block_channel()
        || before.has_sky_channel() != after.has_sky_channel()
    {
        return false;
    }
    positions
        .iter()
        .all(|&pos| before.same_complete_column(after, pos) == Some(true))
}

fn replay_postprocessing_edits(
    current: &Arc<World>,
    processed: &World,
    edits: &[native_world::loaded::BlockPosition],
) -> Result<Arc<World>> {
    if edits.is_empty() {
        return Ok(Arc::clone(current));
    }
    let mut world = (**current).clone();
    for pos in edits {
        let state = processed
            .block(pos.x, pos.y, pos.z)
            .context("postprocessed block is missing")?
            .clone();
        // Forked registries can append different states at the same numeric ID.
        let id = world.intern_state(state);
        world
            .set_block_id(pos.x, pos.y, pos.z, id)
            .map_err(|error| anyhow::anyhow!("cannot rebase postprocessed block: {error}"))?;
    }
    Ok(Arc::new(world))
}

/// Group components from one completed pass; this does not authorize a save.
fn group_fresh_regions(
    chunks: BTreeMap<ChunkPos, PostprocessedChunkDraft>,
    mut poi: BTreeMap<ChunkPos, Option<native_persistence::nbt::NamedTag>>,
    timestamp: u32,
) -> Result<
    BTreeMap<
        (native_persistence::save::FreshRegionKind, i32, i32),
        native_persistence::save::FreshRegionDraft,
    >,
> {
    use native_persistence::save::{FreshRegionDraft, FreshRegionKind};
    ensure!(!chunks.is_empty(), "no generated chunks selected");
    ensure!(
        chunks.len() == poi.len() && chunks.keys().eq(poi.keys()),
        "generated POI coverage differs from terrain"
    );
    let mut regions = BTreeMap::new();
    for (pos, chunk) in chunks {
        let records = chunk
            .entity_records
            .as_ref()
            .context("generated entity coverage is incomplete")?;
        let entity = chunk
            .entity_chunk
            .as_ref()
            .context("generated entity document is missing")?;
        let absolute = native_persistence::world::ChunkPos { x: pos.x, z: pos.z };
        let mut add =
            |kind: FreshRegionKind, document: &native_persistence::nbt::NamedTag| -> Result<()> {
                let rx = pos.x.div_euclid(32);
                let rz = pos.z.div_euclid(32);
                regions
                    .entry((kind, rx, rz))
                    .or_insert_with(|| FreshRegionDraft::new(kind, rx, rz))
                    .add(absolute, document, timestamp)
            };
        add(FreshRegionKind::Terrain, &chunk.chunk)?;
        // A never-stored empty entity chunk has no region entry in Java.
        if !records.is_empty() {
            add(FreshRegionKind::Entities, entity)?;
        }
        if let Some(document) = poi
            .remove(&pos)
            .context("generated POI coverage is incomplete")?
        {
            add(FreshRegionKind::Poi, &document)?;
        }
    }
    Ok(regions)
}

fn require_selected_structure_sources(
    selected: &BTreeSet<ChunkPos>,
    mut references: impl FnMut(ChunkPos) -> Result<BTreeSet<ChunkPos>>,
) -> Result<BTreeSet<ChunkPos>> {
    ensure!(!selected.is_empty(), "no structure chunks selected");
    let mut required = BTreeSet::new();
    for &pos in selected {
        required.extend(references(pos)?);
    }
    ensure!(
        required.is_subset(selected),
        "structure start chunks are missing from the selected region"
    );
    Ok(required)
}

/// Attach only structure data produced by this generation batch. The chunk
/// remains an in-memory draft and this does not establish save completeness.
fn attach_generated_structure_data(
    mut chunk: native_persistence::nbt::NamedTag,
    structures: native_persistence::nbt::Tag,
) -> Result<native_persistence::nbt::NamedTag> {
    use native_persistence::nbt::Tag;
    let Tag::Compound(fields) = &mut chunk.value else {
        bail!("generated chunk root is not a compound")
    };
    let mut matching = fields
        .iter_mut()
        .filter(|(name, _)| name.matches("structures"));
    let (_, value) = matching
        .next()
        .context("generated chunk structures are missing")?;
    ensure!(
        matching.next().is_none(),
        "duplicate generated chunk structures"
    );
    ensure!(
        matches!(value, Tag::Compound(_)) && matches!(structures, Tag::Compound(_)),
        "invalid generated structure data"
    );
    *value = structures;
    native_persistence::nbt::write(&chunk)?;
    Ok(chunk)
}

pub(super) fn updated_postprocessing_sky_sources(
    before: &World,
    after: &World,
    checks: &[native_world::loaded::BlockPosition],
    bounds: native_world::block_light::LightBounds,
    original: &BTreeMap<ChunkPos, Arc<[i32; 256]>>,
) -> Result<Option<BTreeMap<ChunkPos, Arc<[i32; 256]>>>> {
    let mut changed_chunks = BTreeSet::new();
    for pos in checks {
        let old = before
            .block(pos.x, pos.y, pos.z)
            .context("original postprocessing light state missing")?;
        let new = after
            .block(pos.x, pos.y, pos.z)
            .context("updated postprocessing light state missing")?;
        if !native_world::sky_light::same_sky_behavior(old, new)
            .context("unsupported postprocessing sky behavior")?
        {
            changed_chunks.insert(pos.chunk());
        }
    }
    if changed_chunks.is_empty() {
        return Ok(None);
    }
    let mut sources = original.clone();
    for pos in changed_chunks {
        ensure!(
            sources.contains_key(&pos),
            "postprocessing sky sources were not initialized"
        );
        let heights = native_world::sky_light::generated_chunk_sky_sources(after, bounds, pos)
            .context("updated postprocessing sky sources unavailable")?;
        sources.insert(pos, Arc::from(heights));
    }
    Ok(Some(sources))
}

fn extend_postprocessing_light_coverage(
    covered: &mut BTreeSet<ChunkPos>,
    checks: &[native_world::loaded::BlockPosition],
) -> Result<()> {
    for pos in checks {
        for dz in -2i32..=2 {
            for dx in -2i32..=2 {
                covered.insert(ChunkPos {
                    x: pos.chunk().x.checked_add(dx).context("light X overflow")?,
                    z: pos.chunk().z.checked_add(dz).context("light Z overflow")?,
                });
            }
        }
    }
    Ok(())
}

pub(super) struct SpawnStamp {
    world: Arc<World>,
    entities: Arc<BTreeMap<ChunkPos, Arc<Vec<native_persistence::nbt::Tag>>>>,
    schedule_revision: u64,
}

/// The light-engine history accompanying intermediate generated terrain.
pub enum FreshBatchLight {
    Initialized(FreshGenerationLightTracker),
    Staged(StagedGenerationLightEngine),
}

struct DecorationConfig {
    schedule: FeatureSchedule,
    structures: Option<StructureDecorationSchedule>,
    feature_entity_evidence: BTreeMap<FeatureKey, bool>,
    block_registry: BTreeSet<String>,
    heightmap_tags: native_resources::heightmap_tags::HeightmapTags,
    has_skylight: bool,
    dimension_type: Option<String>,
    inline_dimension_type: Option<Arc<str>>,
    ambient_light: Option<f32>,
    mushroom_tags: Option<Arc<native_resources::ore_feature::OreBlockTags>>,
    leaf_tags: Option<Arc<native_resources::ore_feature::OreBlockTags>>,
    snow_tags: Option<Arc<native_resources::ore_feature::OreBlockTags>>,
    fence_tags: Option<Arc<native_resources::ore_feature::OreBlockTags>>,
    bubble_tags: Option<Arc<native_resources::fluid_tags::BubbleColumnTags>>,
    washed_blocks: Option<Arc<BTreeSet<String>>>,
    lava_fluids: Option<Arc<BTreeSet<String>>>,
}

/// Examine every branch without consuming generation randomness or the block
/// resolver. Unknown types, missing resources and cycles remain unverified.
struct EntityFeatureClassifier<'a, P> {
    provider: &'a mut P,
    registry: &'a BTreeSet<ResourceId>,
    tags: FeatureTags,
    active: BTreeSet<String>,
    cache: BTreeMap<String, bool>,
    nodes: usize,
}

fn known_minecraft_type(value: &Value, allowed: &[&str]) -> Result<bool> {
    let name = value
        .get("type")
        .and_then(Value::as_str)
        .context("component type is missing")?;
    let id = ResourceId::parse(name)?;
    Ok(id.namespace == "minecraft" && allowed.contains(&id.path.as_str()))
}

fn known_tree_decorators(value: &Value, field: &str, allowed: &[&str]) -> Result<bool> {
    let decorators = value
        .get(field)
        .and_then(Value::as_array)
        .context("tree decorators are missing")?;
    ensure!(decorators.len() <= 4096, "too many tree decorators");
    for decorator in decorators {
        if !known_minecraft_type(decorator, allowed)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn known_entity_free_tree(value: &Value, fallen: bool) -> Result<bool> {
    if fallen {
        return Ok(known_tree_decorators(
            value,
            "stump_decorators",
            &["trunk_vine", "attached_to_logs"],
        )? && known_tree_decorators(
            value,
            "log_decorators",
            &["trunk_vine", "attached_to_logs"],
        )?);
    }
    if value.get("root_placer").is_some() {
        return Ok(false);
    }
    Ok(known_minecraft_type(
        value
            .get("trunk_placer")
            .context("tree trunk placer is missing")?,
        &[
            "straight_trunk_placer",
            "fancy_trunk_placer",
            "dark_oak_trunk_placer",
            "bending_trunk_placer",
        ],
    )? && known_minecraft_type(
        value
            .get("foliage_placer")
            .context("tree foliage placer is missing")?,
        &[
            "blob_foliage_placer",
            "fancy_foliage_placer",
            "dark_oak_foliage_placer",
            "random_spread_foliage_placer",
        ],
    )? && known_tree_decorators(value, "decorators", &["beehive", "place_on_ground"])?)
}

impl<P: ResourceProvider> EntityFeatureClassifier<'_, P> {
    fn named(&mut self, id: &ResourceId, placed: bool, depth: usize) -> Result<bool> {
        if placed {
            ensure!(
                self.registry.contains(id),
                "placed feature is not registered"
            );
        }
        let folder = if placed { "placed_feature" } else { "feature" };
        let path = format!("data/{}/worldgen/{folder}/{}.json", id.namespace, id.path);
        if let Some(&cached) = self.cache.get(&path) {
            return Ok(cached);
        }
        ensure!(self.active.insert(path.clone()), "cyclic feature reference");
        let result = (|| {
            let bytes = self
                .provider
                .read_data(&path)?
                .context("feature is missing")?;
            let value: Value = serde_json::from_slice(&bytes)?;
            if placed {
                self.placed(&value, depth + 1)
            } else {
                self.configured(&value, depth + 1)
            }
        })();
        self.active.remove(&path);
        if let Ok(value) = result {
            self.cache.insert(path, value);
        }
        result
    }

    fn placed(&mut self, value: &Value, depth: usize) -> Result<bool> {
        ensure!(
            depth < 64 && self.nodes < 32768,
            "feature graph limit exceeded"
        );
        self.nodes += 1;
        if let Some(name) = value.as_str() {
            ensure!(!name.starts_with('#'), "tag is not a single placed feature");
            return self.named(&ResourceId::parse(name)?, true, depth);
        }
        let placed = PlacedFeature::decode(value)?;
        self.configured(&placed.feature, depth + 1)
    }

    fn placed_set(&mut self, value: &Value, depth: usize) -> Result<bool> {
        if let Some(tag) = value.as_str().and_then(|name| name.strip_prefix('#')) {
            let ids = self
                .tags
                .resolve_tag(self.provider, self.registry, &ResourceId::parse(tag)?)?
                .context("placed feature tag is missing")?;
            ensure!(
                !ids.is_empty() && ids.len() <= 4096,
                "invalid selector holder set"
            );
            for id in ids {
                if !self.named(&id, true, depth + 1)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        if let Some(values) = value.as_array() {
            ensure!(
                !values.is_empty() && values.len() <= 4096,
                "invalid selector holder set"
            );
            for value in values {
                if !self.placed(value, depth + 1)? {
                    return Ok(false);
                }
            }
            Ok(true)
        } else {
            self.placed(value, depth + 1)
        }
    }

    fn configured(&mut self, value: &Value, depth: usize) -> Result<bool> {
        ensure!(
            depth < 64 && self.nodes < 32768,
            "feature graph limit exceeded"
        );
        self.nodes += 1;
        if let Some(name) = value.as_str() {
            return self.named(&ResourceId::parse(name)?, false, depth);
        }
        let id = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("feature type is missing")?,
        )?;
        if id.namespace != "minecraft" {
            return Ok(false);
        }
        match id.path.as_str() {
            "simple_block"
            | "ore"
            | "scattered_ore"
            | "disk"
            | "spring_feature"
            | "underwater_magma"
            | "block_column"
            | "lake"
            | "freeze_top_layer"
            | "monster_room"
            | "geode"
            | "multiface_growth"
            | "vines"
            | "huge_brown_mushroom"
            | "huge_red_mushroom" => Ok(true),
            "simple_random_selector" => self.placed_set(
                value
                    .get("features")
                    .context("selector features are missing")?,
                depth + 1,
            ),
            "sequence" => self.placed_set(
                value
                    .get("features")
                    .context("sequence features are missing")?,
                depth + 1,
            ),
            "random_boolean_selector" => Ok(self.placed(
                value
                    .get("feature_true")
                    .context("true feature is missing")?,
                depth + 1,
            )? && self.placed(
                value
                    .get("feature_false")
                    .context("false feature is missing")?,
                depth + 1,
            )?),
            "random_selector" | "weighted_random_selector" => {
                let entries = value
                    .get("features")
                    .and_then(Value::as_array)
                    .context("selector features are missing")?;
                ensure!(entries.len() <= 4096, "too many selector features");
                let mut total = 0i32;
                for entry in entries {
                    let field = if id.path == "random_selector" {
                        "feature"
                    } else {
                        "data"
                    };
                    if id.path == "random_selector" {
                        let chance = entry
                            .get("chance")
                            .and_then(Value::as_f64)
                            .context("selector chance is missing")?
                            as f32;
                        ensure!(
                            chance.is_finite() && (0.0..=1.0).contains(&chance),
                            "invalid selector chance"
                        );
                    } else {
                        let weight = entry
                            .get("weight")
                            .and_then(Value::as_i64)
                            .and_then(|v| i32::try_from(v).ok())
                            .context("invalid selector weight")?;
                        ensure!(weight >= 0, "invalid selector weight");
                        total = total
                            .checked_add(weight)
                            .context("selector weight overflow")?;
                    }
                    if !self.placed(
                        entry.get(field).context("selector entry is missing")?,
                        depth + 1,
                    )? {
                        return Ok(false);
                    }
                }
                if id.path == "random_selector" {
                    self.placed(
                        value
                            .get("default")
                            .context("selector default is missing")?,
                        depth + 1,
                    )
                } else {
                    Ok(true)
                }
            }
            "vegetation_patch" | "waterlogged_vegetation_patch" => self.placed(
                value
                    .get("vegetation_feature")
                    .context("patch vegetation is missing")?,
                depth + 1,
            ),
            "root_system" => self.placed(
                value.get("feature").context("root feature is missing")?,
                depth + 1,
            ),
            "tree" => known_entity_free_tree(value, false),
            "fallen_tree" => known_entity_free_tree(value, true),
            _ => Ok(false),
        }
    }
}

fn feature_entity_evidence(
    provider: &mut impl ResourceProvider,
    schedule: &FeatureSchedule,
    registry: &BTreeSet<ResourceId>,
) -> BTreeMap<FeatureKey, bool> {
    let mut classifier = EntityFeatureClassifier {
        provider,
        registry,
        tags: FeatureTags::default(),
        active: BTreeSet::new(),
        cache: BTreeMap::new(),
        nodes: 0,
    };
    let mut evidence = BTreeMap::new();
    for key in schedule.steps().iter().flatten() {
        if evidence.contains_key(key) {
            continue;
        }
        let safe = match key {
            FeatureKey::Registered(id) => classifier.named(id, true, 0),
            FeatureKey::Inline { biome, .. } => schedule
                .biome(biome)
                .and_then(|biome| biome.inline.get(key))
                .context("inline placed feature is missing")
                .and_then(|value| classifier.placed(value, 0)),
        }
        .unwrap_or(false);
        evidence.insert(key.clone(), safe);
    }
    evidence
}

#[cfg(test)]
fn features_cannot_spawn_entities(
    provider: &mut impl ResourceProvider,
    schedule: &FeatureSchedule,
    registry: &BTreeSet<ResourceId>,
) -> bool {
    feature_entity_evidence(provider, schedule, registry)
        .values()
        .all(|safe| *safe)
}

#[derive(Default)]
struct GeneratedEntitySources {
    complete_spawns: BTreeSet<ChunkPos>,
    recorded_hut_features: BTreeSet<ChunkPos>,
    completed_feature_passes: BTreeSet<ChunkPos>,
    // Only completed passes can already have contributed entities. Once an
    // unverified pass runs, earlier per-chunk completeness is invalid too.
    untracked_feature_pass: bool,
    untracked: bool,
}

fn structure_entities_are_accounted_for(
    structures: &PreparedStructureStages,
    registry: &StructureDecorationSchedule,
    pos: ChunkPos,
    completed: impl Fn(ChunkPos) -> bool,
    recorded_huts: &BTreeSet<ChunkPos>,
) -> bool {
    let registered: BTreeSet<_> = registry
        .steps()
        .iter()
        .flatten()
        .map(|entry| &entry.id)
        .collect();
    (-1..=1).all(|dz| {
        (-1..=1).all(|dx| {
            let Some(x) = pos.x.checked_add(dx) else {
                return false;
            };
            let Some(z) = pos.z.checked_add(dz) else {
                return false;
            };
            let source = ChunkPos { x, z };
            completed(source)
                && structures.only_accounted_structure_references(
                    source,
                    &registered,
                    recorded_huts.contains(&source),
                )
        })
    })
}

impl GeneratedEntitySources {
    fn complete_feature_pass(&mut self, pos: ChunkPos, entity_free: bool) {
        self.completed_feature_passes.insert(pos);
        self.untracked_feature_pass |= !entity_free;
    }

    fn complete_spawn(&mut self, pos: ChunkPos, additions: &[super::GeneratedEntityAddition]) {
        if additions.iter().all(|addition| {
            addition.block_position[0].div_euclid(16) == pos.x
                && addition.block_position[2].div_euclid(16) == pos.z
        }) {
            self.complete_spawns.insert(pos);
        } else {
            self.untracked = true;
        }
    }

    fn contains_complete_spawn(&self, pos: ChunkPos) -> bool {
        !self.untracked && self.complete_spawns.contains(&pos)
    }

    fn complete_entity_coverage(
        &self,
        pos: ChunkPos,
        structure_entities_accounted_for: bool,
    ) -> bool {
        structure_entities_accounted_for
            && self.completed_feature_passes.contains(&pos)
            && !self.untracked_feature_pass
            && self.contains_complete_spawn(pos)
    }
}

impl FreshTerrainBatch {
    #[cfg(test)]
    pub(super) fn set_structure_stages_for_test(
        &mut self,
        structures: Option<PreparedStructureStages>,
    ) {
        self.structures = structures;
    }

    /// Structure starts remain intermediate and do not authorize saving.
    pub fn structure_starts(&self, pos: ChunkPos) -> Option<&super::structure_batch::SimpleStarts> {
        (!self.failed)
            .then(|| {
                let structures = self.structures.as_ref()?;
                (!structures.ocean_starts.contains_key(&pos)
                    && !structures.mineshafts.contains_key(&pos))
                .then(|| structures.starts.get(&pos))
                .flatten()
            })
            .flatten()
    }

    /// Intermediate ocean-ruin starts retained for later decoration.
    pub fn ocean_ruin_starts(
        &self,
        pos: ChunkPos,
    ) -> Option<&super::structure_batch::OceanRuinStarts> {
        (!self.failed)
            .then(|| self.structures.as_ref()?.ocean_starts.get(&pos))
            .flatten()
    }

    /// Intermediate mineshaft starts retained for later decoration.
    pub fn mineshaft_starts(
        &self,
        pos: ChunkPos,
    ) -> Option<&super::structure_batch::MineshaftStarts> {
        (!self.failed)
            .then(|| self.structures.as_ref()?.mineshafts.get(&pos))
            .flatten()
    }

    /// Build intermediate 26.3 structure NBT after the batch finishes FEATURES.
    /// The returned data does not establish that the world is safe to save.
    pub fn structure_data(&self, pos: ChunkPos) -> Result<native_persistence::nbt::Tag> {
        self.structure_reference_sources(pos)?;
        self.pack_validated_structure_data(pos)
    }

    // Only used after checking this chunk's references against this immutable
    // batch. The public structure_data entry point always performs that check.
    fn pack_validated_structure_data(&self, pos: ChunkPos) -> Result<native_persistence::nbt::Tag> {
        let structures = self
            .structures
            .as_ref()
            .context("structure generation is disabled")?;
        super::structure_data::pack_structure_data_with_mineshaft(
            pos,
            structures
                .starts
                .get(&pos)
                .context("structure starts are missing")?,
            structures
                .ocean_starts
                .get(&pos)
                .unwrap_or(&Default::default()),
            structures
                .mineshafts
                .get(&pos)
                .unwrap_or(&Default::default()),
            structures
                .references
                .get(&pos)
                .context("structure references are missing")?,
            &structures.huts,
        )
    }

    /// Identify generated source chunks referenced by this chunk. A save plan
    /// must retain them and prove the remaining world data separately.
    pub fn structure_reference_sources(&self, pos: ChunkPos) -> Result<BTreeSet<ChunkPos>> {
        ensure!(self.is_complete(), "terrain batch is incomplete or failed");
        ensure!(
            self.schedule
                .completed(pos)
                .is_some_and(|stage| stage >= GenerationStage::Features),
            "structure data requires completed features"
        );
        let structures = self
            .structures
            .as_ref()
            .context("structure generation is disabled")?;
        super::structure_data::referenced_start_chunks_with_mineshaft(
            pos,
            &structures.starts,
            &structures.ocean_starts,
            &structures.mineshafts,
            structures
                .references
                .get(&pos)
                .context("structure references are missing")?,
            &structures.huts,
        )
    }
    /// The data-pack tag used for leaf distance checks in this batch.
    pub fn leaf_support_tags(&self) -> Option<Arc<native_resources::ore_feature::OreBlockTags>> {
        self.decoration.as_ref()?.leaf_tags.clone()
    }

    pub(super) fn spawn_stamp(&self) -> Result<SpawnStamp> {
        ensure!(self.is_complete(), "generation batch has pending work");
        let terrain = self
            .terrain
            .as_ref()
            .context("generated terrain is missing")?;
        Ok(SpawnStamp {
            world: terrain.snapshot(),
            entities: Arc::clone(&terrain.generated_entities),
            schedule_revision: self.schedule.revision(),
        })
    }

    pub(super) fn validate_spawn_stamp(&self, stamp: &SpawnStamp, center: ChunkPos) -> Result<()> {
        ensure!(self.is_complete(), "generation batch has pending work");
        ensure!(
            self.schedule.completed(center) == Some(GenerationStage::Light),
            "spawn chunk is no longer light-complete"
        );
        let terrain = self
            .terrain
            .as_ref()
            .context("generated terrain is missing")?;
        ensure!(
            self.schedule.revision() == stamp.schedule_revision
                && Arc::ptr_eq(&stamp.world, &terrain.snapshot())
                && Arc::ptr_eq(&stamp.entities, &terrain.generated_entities),
            "generation batch changed during creature spawning"
        );
        Ok(())
    }

    pub(super) fn spawn_region_random(
        &self,
        pos: ChunkPos,
    ) -> native_resources::worldgen_region_random::RegionRandom {
        self.generator.region_random(pos)
    }
    /// The provider must use the same unchanged data-pack stack as the generator.
    pub fn spawn_origin(&self, provider: &mut impl ResourceProvider) -> Result<ChunkPos> {
        let [x, z] = self.generator.spawn_origin(provider)?;
        Ok(ChunkPos { x, z })
    }

    pub fn spawn_height(&self) -> i32 {
        self.generator.spawn_height()
    }
    pub fn sea_level(&self) -> i32 {
        self.generator.sea_level()
    }

    /// Biome used for the natural-mob-spawns attribute: Java samples the
    /// chunk's minimum X/Z at the region's inclusive maximum Y. The biome
    /// neighborhood supplies the same seeded block-to-quart zoom.
    pub fn mob_spawn_biome(&self, pos: ChunkPos) -> Result<ResourceId> {
        ensure!(
            !self.failed
                && self
                    .schedule
                    .completed(pos)
                    .is_some_and(|stage| stage >= GenerationStage::Light),
            "mob spawn biome requires a light-complete chunk"
        );
        let (_, max_y) = self.build_bounds()?;
        let x = pos
            .x
            .checked_mul(16)
            .context("mob spawn biome X overflow")?;
        let z = pos
            .z
            .checked_mul(16)
            .context("mob spawn biome Z overflow")?;
        Ok(self.biomes.biome_at_block(x, max_y - 1, z)?.clone())
    }

    /// The probability attribute uses the unzoomed noise biome at the same
    /// generation position, unlike the full-resolution spawn-list attribute.
    pub fn mob_spawn_probability_biome(&self, pos: ChunkPos) -> Result<ResourceId> {
        ensure!(
            !self.failed
                && self
                    .schedule
                    .completed(pos)
                    .is_some_and(|stage| stage >= GenerationStage::Light),
            "mob spawn biome requires a light-complete chunk"
        );
        let (_, max_y) = self.build_bounds()?;
        let x = pos
            .x
            .checked_mul(16)
            .context("mob spawn biome X overflow")?;
        let z = pos
            .z
            .checked_mul(16)
            .context("mob spawn biome Z overflow")?;
        Ok(self.biomes.noise_biome_at_block(x, max_y - 1, z)?.clone())
    }

    /// Resolve the spawn list and probability using their distinct biome
    /// lookup rules. Dimension attributes and already-sampled later layers
    /// must come from this world's unchanged data-pack stack and clocks.
    pub fn resolve_creature_spawn_settings(
        &self,
        provider: &mut impl ResourceProvider,
        pos: ChunkPos,
        dimension_attributes: &Value,
        later_layers: &[Value],
        entity_is_misc: impl FnMut(&ResourceId) -> Option<bool>,
    ) -> Result<native_resources::mob_spawns::ResolvedCreatureSpawns> {
        let spawn_biome = self.mob_spawn_biome(pos)?;
        let probability_biome = self.mob_spawn_probability_biome(pos)?;
        let spawn =
            native_resources::mob_spawns::load_biome_spawn_attributes(provider, &spawn_biome)?;
        let probability = if probability_biome == spawn_biome {
            spawn.clone()
        } else {
            native_resources::mob_spawns::load_biome_spawn_attributes(provider, &probability_biome)?
        };
        native_resources::mob_spawns::resolve_creature_spawns(
            dimension_attributes,
            &spawn,
            &probability,
            later_layers,
            entity_is_misc,
        )
    }

    /// Initialize a sheep using the biome at its actual entity block position.
    /// The tags, region random source and group must belong to this world and
    /// spawn operation. This does not construct, serialize or store the entity.
    pub fn initialize_sheep(
        &self,
        position: [i32; 3],
        tags: &native_resources::worldgen_region_random::SheepBiomeTags,
        random: &mut native_resources::worldgen_region_random::RegionRandom,
        group: &mut native_resources::worldgen_region_random::AnimalGroupData,
        has_follow_range_bonus: bool,
    ) -> Result<native_resources::worldgen_region_random::SheepInitialization> {
        ensure!(
            self.is_complete(),
            "finish generation requests before initializing a sheep"
        );
        let [x, y, z] = position;
        let chunk = ChunkPos {
            x: x.div_euclid(16),
            z: z.div_euclid(16),
        };
        ensure!(
            self.schedule
                .completed(chunk)
                .is_some_and(|stage| stage >= GenerationStage::Light),
            "sheep initialization requires a light-complete chunk"
        );
        let biome = self.biomes.biome_at_block(x, y, z)?;
        let (warm, cold) = tags.membership(biome)?;
        Ok(native_resources::worldgen_region_random::finalize_sheep(
            random,
            group,
            has_follow_range_bonus,
            warm,
            cold,
        ))
    }

    /// Finalize a farm animal using its actual full-resolution biome. The
    /// caller must still create, serialize and publish the entity itself.
    pub fn initialize_farm_animal(
        &self,
        position: [i32; 3],
        variants: &native_resources::animal_variants::FarmAnimalSpawnVariants,
        random: &mut native_resources::worldgen_region_random::RegionRandom,
        group: &mut native_resources::worldgen_region_random::AnimalGroupData,
        has_follow_range_bonus: bool,
    ) -> Result<native_resources::animal_variants::FarmAnimalInitialization> {
        self.initialize_farm_animal_with_moon(
            position,
            variants,
            random,
            group,
            has_follow_range_bonus,
            None,
        )
    }

    pub(super) fn initialize_farm_animal_with_moon(
        &self,
        position: [i32; 3],
        variants: &native_resources::animal_variants::FarmAnimalSpawnVariants,
        random: &mut native_resources::worldgen_region_random::RegionRandom,
        group: &mut native_resources::worldgen_region_random::AnimalGroupData,
        has_follow_range_bonus: bool,
        moon: Option<(
            &native_resources::moon_phase::Environment,
            &BTreeMap<ResourceId, i64>,
        )>,
    ) -> Result<native_resources::animal_variants::FarmAnimalInitialization> {
        ensure!(
            self.is_complete(),
            "finish generation requests before initializing an animal"
        );
        let [x, y, z] = position;
        let chunk = ChunkPos {
            x: x.div_euclid(16),
            z: z.div_euclid(16),
        };
        ensure!(
            self.schedule
                .completed(chunk)
                .is_some_and(|stage| stage >= GenerationStage::Light),
            "animal initialization requires a light-complete chunk"
        );
        let biome = self.biomes.biome_at_block(x, y, z)?;
        let mut environment = FarmAnimalEnvironment {
            batch: self,
            position,
            biome,
            moon,
            structures: None,
        };
        variants.finalize_in_environment(&mut environment, random, group, has_follow_range_bonus)
    }
}

fn structures_with_piece_at(
    structures: Option<&PreparedStructureStages>,
    position: [i32; 3],
) -> Result<BTreeSet<ResourceId>> {
    let mut matching = BTreeSet::new();
    let Some(structures) = structures else {
        return Ok(matching);
    };
    let chunk = ChunkPos {
        x: position[0].div_euclid(16),
        z: position[2].div_euclid(16),
    };
    let references = structures
        .references
        .get(&chunk)
        .context("spawn chunk structure references unavailable")?;
    for (id, sources) in references.membership() {
        for &packed in sources {
            let bits = packed as u64;
            let source = ChunkPos {
                x: bits as u32 as i32,
                z: (bits >> 32) as u32 as i32,
            };
            let simple = structures
                .starts
                .get(&source)
                .and_then(|starts| starts.get(id));
            let ocean = structures
                .ocean_starts
                .get(&source)
                .and_then(|starts| starts.get(id));
            let mineshaft = structures
                .mineshafts
                .get(&source)
                .and_then(|starts| starts.get(id));
            ensure!(
                usize::from(simple.is_some())
                    + usize::from(ocean.is_some())
                    + usize::from(mineshaft.is_some())
                    == 1,
                "spawn referenced structure is missing or ambiguous"
            );
            let contains = |bounds: &native_core::beardifier::BoundingBox| {
                (0..3).all(|axis| (bounds.min[axis]..=bounds.max[axis]).contains(&position[axis]))
            };
            let inside = if let Some(start) = simple {
                ensure!(
                    start.chunk == [source.x, source.z],
                    "spawn structure source differs"
                );
                contains(&start.bounds)
            } else if let Some(start) = ocean {
                ensure!(
                    start.chunk == [source.x, source.z],
                    "spawn structure source differs"
                );
                start.pieces.iter().any(|piece| contains(&piece.bounds))
            } else {
                let start = mineshaft.expect("one start was validated");
                ensure!(
                    start.chunk == [source.x, source.z],
                    "spawn structure source differs"
                );
                start
                    .pieces
                    .iter()
                    .any(|piece| contains(&piece.piece.bounds))
            };
            if inside {
                matching.insert(id.clone());
                break;
            }
        }
    }
    Ok(matching)
}

impl FreshTerrainBatch {
    /// Start the random ordering for one light-complete chunk. The supplied
    /// list and probability must be resolved from the same data-pack stack
    /// and all applicable environment layers. This only plans attempts; it
    /// neither creates entities nor marks SPAWN complete.
    pub fn plan_creature_spawning(
        &self,
        pos: ChunkPos,
        spawn_mobs: bool,
        probability: f32,
        spawns: native_resources::mob_spawns::CreatureSpawns,
    ) -> Result<native_resources::worldgen_mob_spawner::ChunkCreatureSpawner> {
        ensure!(
            self.is_complete(),
            "finish generation requests before mob spawning"
        );
        ensure!(
            self.schedule.completed(pos) == Some(GenerationStage::Light),
            "mob spawning requires a light-complete chunk"
        );
        let disabled = self
            .generator
            .disable_mob_generation()
            .context("noise settings are missing disable_mob_generation")?;
        ensure!(
            !disabled,
            "generator disables mob generation; use the no-mob spawn stage"
        );
        native_resources::worldgen_mob_spawner::ChunkCreatureSpawner::new(
            self.generator.seed,
            pos.x,
            pos.z,
            spawn_mobs,
            probability,
            spawns,
        )
    }

    /// Records captured so far; unimplemented entity sources may be missing.
    pub fn recorded_generated_entities(
        &self,
        pos: ChunkPos,
    ) -> Result<&[native_persistence::nbt::Tag]> {
        ensure!(!self.failed, "generation batch failed");
        self.terrain
            .as_ref()
            .context("generated terrain is missing")?
            .recorded_generated_entities(pos)
    }

    /// Return complete records only when structure and feature entity sources
    /// are verified and spawning was fully recorded.
    /// Unsupported or cross-chunk entity sources keep the result unknown.
    pub fn complete_generated_entity_records(
        &self,
        pos: ChunkPos,
    ) -> Result<Option<&[native_persistence::nbt::Tag]>> {
        ensure!(self.is_complete(), "generation batch has pending work");
        ensure!(
            self.schedule
                .completed(pos)
                .is_some_and(|stage| stage >= GenerationStage::Spawn),
            "entity completeness requires a spawn-complete chunk"
        );
        let decoration = self.decoration.as_ref().context("decoration is missing")?;
        let structure_entities_accounted_for = match &self.structures {
            None => true,
            Some(structures) => decoration.structures.as_ref().is_some_and(|registry| {
                structure_entities_are_accounted_for(
                    structures,
                    registry,
                    pos,
                    |source| {
                        self.schedule
                            .completed(source)
                            .is_some_and(|stage| stage >= GenerationStage::Features)
                    },
                    &self.entity_sources.recorded_hut_features,
                )
            }),
        };
        if !self
            .entity_sources
            .complete_entity_coverage(pos, structure_entities_accounted_for)
        {
            return Ok(None);
        }
        Ok(Some(self.recorded_generated_entities(pos)?))
    }

    /// Share the complete per-chunk records across generation checkpoints.
    /// A missing result still means completeness could not be established.
    pub fn complete_generated_entity_records_shared(
        &self,
        pos: ChunkPos,
    ) -> Result<Option<Arc<Vec<native_persistence::nbt::Tag>>>> {
        if self.complete_generated_entity_records(pos)?.is_none() {
            return Ok(None);
        }
        let terrain = self
            .terrain
            .as_ref()
            .context("generated terrain is missing")?;
        Ok(Some(
            terrain
                .generated_entities
                .get(&pos)
                .cloned()
                .unwrap_or_default(),
        ))
    }

    pub fn generated_entity_chunk_is_known_empty(&self, pos: ChunkPos) -> Result<bool> {
        Ok(self
            .complete_generated_entity_records(pos)?
            .is_some_and(<[_]>::is_empty))
    }

    /// Stage all addFreshEntityWithPassengers visits and publish only after
    /// every destination and compound has been validated.
    pub(super) fn record_finalized_creature(
        &mut self,
        attempt: &native_resources::worldgen_mob_spawner::CreatureAttempt,
        additions: Vec<super::GeneratedEntityAddition>,
    ) -> Result<()> {
        ensure!(
            self.is_complete(),
            "finish generation requests before recording a creature"
        );
        let center = ChunkPos {
            x: attempt.x.div_euclid(16),
            z: attempt.z.div_euclid(16),
        };
        ensure!(
            self.schedule.completed(center) == Some(GenerationStage::Light),
            "creature requires a light-complete chunk"
        );
        ensure!(
            !additions.is_empty(),
            "finalized creature traversal is missing"
        );
        let terrain = self
            .terrain
            .as_mut()
            .context("generated terrain is missing")?;
        let mut staged = Arc::clone(&terrain.generated_entities);
        for addition in additions {
            super::stage_generated_entity(&terrain.world, &mut staged, addition)?;
        }
        terrain.generated_entities = staged;
        self.entity_sources.untracked = true;
        Ok(())
    }

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
        Self::new_inner(generator, tags, dimension, None)
    }

    /// Consume completed structure stages and their generator for an
    /// intermediate terrain-only batch. The caller supplies the actual Java
    /// iteration order for every terrain chunk it will request.
    pub fn new_with_prepared_structures(
        structures: StructureGenerationBatch,
        orders: BTreeMap<ChunkPos, Vec<TerrainStartKey>>,
        tags: CarverBlockTags,
        dimension: &str,
    ) -> Result<Self> {
        let (generator, structures) = structures.into_terrain_stages(orders)?;
        Self::new_inner(generator, tags, dimension, Some(structures))
    }

    /// Construct a structure-aware intermediate terrain batch when every
    /// requested chunk has a Java-equivalent order independent of identity
    /// hashes. Ambiguous or unavailable structure histories remain errors.
    pub fn new_with_unambiguous_structures(
        structures: StructureGenerationBatch,
        terrain_chunks: impl IntoIterator<Item = ChunkPos>,
        tags: CarverBlockTags,
        dimension: &str,
    ) -> Result<Self> {
        let orders = structures.unambiguous_terrain_orders(terrain_chunks)?;
        Self::new_with_prepared_structures(structures, orders, tags, dimension)
    }

    pub(super) fn new_with_retained_structures(
        generator: TerrainGenerator,
        continuation: StructureGenerationBatch,
        stages: PreparedStructureStages,
        tags: CarverBlockTags,
        dimension: &str,
    ) -> Result<Self> {
        let mut batch = Self::new_inner(generator, tags, dimension, Some(stages))?;
        batch.structure_continuation = Some(continuation);
        Ok(batch)
    }

    /// Request structure prerequisites for an expansion without modifying the
    /// contexts already used by generated terrain.
    pub fn request_structure_expansion(
        &mut self,
        plan: &super::structure_batch::StructurePreparationPlan,
    ) -> Result<()> {
        ensure!(
            !self.failed && self.schedule.is_complete(),
            "terrain batch has pending or failed work"
        );
        let structures = self
            .structure_continuation
            .as_mut()
            .context("structure continuation is unavailable")?;
        ensure!(
            structures.is_complete(),
            "structure batch has pending or failed work"
        );
        plan.request_structures(structures)
    }

    pub fn supports_structure_expansion(&self) -> bool {
        self.structure_continuation.is_some()
    }

    pub fn step_structure_expansion<P: ResourceProvider>(
        &mut self,
        provider: &mut P,
        classify: &mut impl FnMut(&Value) -> Result<native_persistence::heightmap::BlockHeightFlags>,
    ) -> Result<Option<(ChunkPos, GenerationStage)>> {
        ensure!(
            !self.failed && self.schedule.is_complete(),
            "terrain batch has pending or failed work"
        );
        self.structure_continuation
            .as_mut()
            .context("structure continuation is unavailable")?
            .step(provider, classify)
    }

    /// Add validated structure contexts atomically after the structure work
    /// has completed. Missing or ambiguous data leaves existing contexts intact.
    pub fn finish_structure_expansion(
        &mut self,
        plan: &super::structure_batch::StructurePreparationPlan,
    ) -> Result<()> {
        ensure!(
            !self.failed && self.schedule.is_complete(),
            "terrain batch has pending or failed work"
        );
        let structures = self
            .structure_continuation
            .as_ref()
            .context("structure continuation is unavailable")?;
        let existing = self
            .structures
            .as_mut()
            .context("structure stages are unavailable")?;
        let added = structures.prepare_additional_stages(plan, Some(existing))?;
        ensure!(
            added
                .huts
                .keys()
                .all(|key| !existing.huts.contains_key(key)),
            "expanded hut state already exists"
        );
        ensure!(
            added
                .ocean_starts
                .keys()
                .all(|pos| !existing.ocean_starts.contains_key(pos)),
            "expanded ocean ruin state already exists"
        );
        ensure!(
            added
                .mineshafts
                .keys()
                .all(|pos| !existing.mineshafts.contains_key(pos)),
            "expanded mineshaft state already exists"
        );
        existing.starts.extend(added.starts);
        existing.ocean_starts.extend(added.ocean_starts);
        existing.mineshafts.extend(added.mineshafts);
        existing.huts.extend(added.huts);
        existing.references.extend(added.references);
        existing.terrain.extend(added.terrain);
        Ok(())
    }

    fn new_inner(
        generator: TerrainGenerator,
        tags: CarverBlockTags,
        dimension: &str,
        structures: Option<PreparedStructureStages>,
    ) -> Result<Self> {
        let id = ResourceId::parse(dimension)?;
        let biomes = generator.biome_neighborhood()?;
        Ok(Self {
            identity: Arc::new(()),
            schedule: GenerationSchedule::new(),
            generator,
            structures,
            structure_continuation: None,
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
            spawn_ready: BTreeSet::new(),
            entity_sources: GeneratedEntitySources::default(),
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
        ensure!(
            lighting.ambient_light.is_none_or(f32::is_finite),
            "invalid dimension ambient light"
        );
        let schedule = self.generator.feature_schedule(provider, placed_features)?;
        let structures = self
            .structures
            .as_ref()
            .map(|_| StructureDecorationSchedule::load(provider))
            .transpose()?;
        let feature_entity_evidence = feature_entity_evidence(provider, &schedule, placed_features);
        let heightmap_tags =
            native_resources::heightmap_tags::HeightmapTags::load(provider, &block_registry)?;
        let mushroom_tag = ResourceId::parse("minecraft:overrides_mushroom_light_requirement")?;
        let mushroom_tags = native_resources::ore_feature::OreBlockTags::load(
            provider,
            &block_registry,
            &BTreeSet::from([mushroom_tag]),
        )
        .ok()
        .map(Arc::new);
        let leaf_tag = ResourceId::parse("minecraft:prevents_nearby_leaf_decay")?;
        let leaf_tags = native_resources::ore_feature::OreBlockTags::load(
            provider,
            &block_registry,
            &BTreeSet::from([leaf_tag]),
        )
        .ok()
        .map(Arc::new);
        let snow_tags = native_resources::ore_feature::OreBlockTags::load(
            provider,
            &block_registry,
            &BTreeSet::from([ResourceId::parse("minecraft:snow")?]),
        )
        .ok()
        .map(Arc::new);
        let fence_tags = native_resources::ore_feature::OreBlockTags::load(
            provider,
            &block_registry,
            &BTreeSet::from([
                ResourceId::parse("minecraft:fences")?,
                ResourceId::parse("minecraft:wooden_fences")?,
                ResourceId::parse("minecraft:shulker_boxes")?,
            ]),
        )
        .ok()
        .map(Arc::new);
        let bubble_tags =
            native_resources::fluid_tags::BubbleColumnTags::load(provider, &block_registry)
                .ok()
                .map(Arc::new);
        let washed_blocks =
            native_resources::fluid_tags::load_washed_away_blocks(provider, &block_registry)
                .ok()
                .map(Arc::new);
        let lava_fluids = native_resources::fluid_tags::load_lava(provider)
            .ok()
            .map(Arc::new);
        self.decoration = Some(DecorationConfig {
            schedule,
            structures,
            feature_entity_evidence,
            block_registry,
            heightmap_tags,
            mushroom_tags,
            leaf_tags,
            snow_tags,
            fence_tags,
            bubble_tags,
            washed_blocks,
            lava_fluids,
            has_skylight: lighting.has_skylight,
            ambient_light: lighting.ambient_light,
            dimension_type: lighting
                .dimension_type
                .as_ref()
                .map(|id| format!("{}:{}", id.namespace, id.path)),
            inline_dimension_type: lighting.inline_dimension_type.clone(),
        });
        Ok(())
    }

    pub(super) fn ambient_light(&self) -> Result<f32> {
        self.decoration
            .as_ref()
            .and_then(|config| config.ambient_light)
            .context("dimension ambient light is unavailable")
    }

    /// Return the skylight capability used to generate this batch's lighting.
    pub fn has_skylight(&self) -> Result<bool> {
        Ok(self
            .decoration
            .as_ref()
            .context("generation lighting is missing")?
            .has_skylight)
    }

    pub(super) fn generated_block_registry(&self) -> Result<&GeneratedBlockRegistry> {
        self.light_registry
            .as_ref()
            .context("generated block registry is unavailable")
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
        self.validate_structure_dependencies(pos, GenerationStage::Terrain)?;
        self.check_writes(pos, 0)?;
        self.schedule.request(pos, GenerationStage::Terrain)
    }

    fn validate_structure_dependencies(&self, pos: ChunkPos, stage: GenerationStage) -> Result<()> {
        ensure!(
            self.structure_continuation
                .as_ref()
                .is_none_or(StructureGenerationBatch::is_complete),
            "structure expansion has pending or failed work"
        );
        if let Some(structures) = &self.structures {
            if stage >= GenerationStage::Terrain {
                ensure!(
                    structures.terrain.contains_key(&pos),
                    "prepared structure terrain order is missing for {pos:?}"
                );
            }
            for (radius, &needed) in stage.accumulated().iter().enumerate() {
                let r = i32::try_from(radius)?;
                for dz in -r..=r {
                    for dx in -r..=r {
                        if dx.abs().max(dz.abs()) != r {
                            continue;
                        }
                        let source = ChunkPos {
                            x: pos
                                .x
                                .checked_add(dx)
                                .context("generation chunk X overflow")?,
                            z: pos
                                .z
                                .checked_add(dz)
                                .context("generation chunk Z overflow")?,
                        };
                        ensure!(
                            structures.starts.contains_key(&source),
                            "prepared structure starts are missing for {source:?}"
                        );
                        if needed >= GenerationStage::StructureReferences {
                            ensure!(
                                structures.references.contains_key(&source),
                                "prepared structure references are missing for {source:?}"
                            );
                        }
                        if needed >= GenerationStage::Terrain {
                            ensure!(
                                structures.terrain.contains_key(&source),
                                "prepared structure terrain order is missing for {source:?}"
                            );
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Request terrain and supported FEATURES for one chunk.
    pub fn request_features(&mut self, pos: ChunkPos) -> Result<()> {
        ensure!(
            !self.failed,
            "generation batch failed and must be discarded"
        );
        ensure!(self.decoration.is_some(), "decoration is not configured");
        self.validate_structure_dependencies(pos, GenerationStage::Features)?;
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
        self.request_light_initialization_many([pos])
    }

    /// Request overlapping light-initialization neighborhoods atomically.
    pub fn request_light_initialization_many(
        &mut self,
        positions: impl IntoIterator<Item = ChunkPos>,
    ) -> Result<()> {
        ensure!(
            !self.failed,
            "generation batch failed and must be discarded"
        );
        ensure!(
            self.light_registry.is_some(),
            "light initialization is not configured"
        );
        let positions: Vec<_> = positions.into_iter().collect();
        ensure!(!positions.is_empty(), "no light initialization targets");
        if self.light_engine.is_some() {
            ensure!(
                !self.schedule.has_pending_stage(GenerationStage::Terrain)
                    && !self.schedule.has_pending_stage(GenerationStage::Features)
                    && !self.schedule.has_pending_stage(GenerationStage::Light),
                "finish pending terrain, decoration and propagation before light registration"
            );
        }
        for &pos in &positions {
            self.validate_structure_dependencies(pos, GenerationStage::InitializeLight)?;
            if self.light_engine.is_some() {
                ensure!(
                    self.schedule.completed(pos) == Some(GenerationStage::Features),
                    "sealed batches can only initialize already decorated chunks"
                );
            }
        }
        self.schedule.request_many(
            positions
                .into_iter()
                .map(|pos| (pos, GenerationStage::InitializeLight)),
        )
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

    /// Advance SPAWN only when the generator disables initial mobs or the
    /// active spawn_mobs game rule is false. This does not make the chunk
    /// FULL or imply that feature-generated entity data is empty.
    pub fn request_spawn_without_mobs(&mut self, pos: ChunkPos, spawn_mobs: bool) -> Result<()> {
        ensure!(
            self.is_complete(),
            "finish generation requests before spawning"
        );
        let disabled = self
            .generator
            .disable_mob_generation()
            .context("noise settings are missing disable_mob_generation")?;
        ensure!(
            disabled || !spawn_mobs,
            "world-generation mob spawning is not implemented"
        );
        self.request_completed_spawn(pos)?;
        self.entity_sources.complete_spawn(pos, &[]);
        Ok(())
    }

    /// The generator's `disable_mob_generation` setting. Incomplete
    /// synthetic generator settings cannot establish either outcome.
    pub fn mob_generation_disabled(&self) -> Result<bool> {
        self.generator
            .disable_mob_generation()
            .context("noise settings are missing disable_mob_generation")
    }

    /// Schedule an already completed spawn operation; this is not a FULL chunk.
    pub(super) fn request_completed_spawn(&mut self, pos: ChunkPos) -> Result<()> {
        ensure!(
            self.is_complete(),
            "finish generation requests before spawning"
        );
        ensure!(
            self.schedule.completed(pos) == Some(GenerationStage::Light),
            "spawn requires a light-complete chunk"
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
                    let neighbor = ChunkPos {
                        x: pos.x.checked_add(dx).context("spawn chunk X overflow")?,
                        z: pos.z.checked_add(dz).context("spawn chunk Z overflow")?,
                    };
                    ensure!(
                        self.schedule
                            .completed(neighbor)
                            .is_some_and(|stage| stage >= needed),
                        "spawn dependencies are incomplete"
                    );
                }
            }
        }
        self.schedule.request(pos, GenerationStage::Spawn)?;
        self.spawn_ready.insert(pos);
        Ok(())
    }

    /// Validate every entity before requesting SPAWN, then publish the staged
    /// records only after the schedule accepted the complete operation.
    pub(super) fn publish_completed_spawn(
        &mut self,
        pos: ChunkPos,
        additions: &[super::GeneratedEntityAddition],
    ) -> Result<()> {
        let terrain = self
            .terrain
            .as_ref()
            .context("generated terrain is missing")?;
        let mut staged = Arc::clone(&terrain.generated_entities);
        for addition in additions.iter().cloned() {
            super::stage_generated_entity(&terrain.world, &mut staged, addition)?;
        }
        self.request_completed_spawn(pos)?;
        self.terrain
            .as_mut()
            .expect("validated generated terrain")
            .generated_entities = staged;
        self.entity_sources.complete_spawn(pos, additions);
        Ok(())
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

    /// Encode the block, biome and light sections of a completed SPAWN-stage
    /// chunk. This is only one component of a future save, not a FULL chunk:
    /// heightmaps, ticks, block entities, structures and entities remain separate.
    pub fn spawned_section_data(&mut self, pos: ChunkPos) -> Result<native_persistence::nbt::Tag> {
        ensure!(
            self.is_complete(),
            "finish generation requests before encoding sections"
        );
        ensure!(
            self.schedule
                .completed(pos)
                .is_some_and(|stage| stage >= GenerationStage::Spawn),
            "section encoding requires a spawn-complete chunk"
        );
        let has_skylight = self
            .decoration
            .as_ref()
            .context("generation lighting is missing")?
            .has_skylight;
        let min_y = self.generator.min_y;
        let height = self.generator.height;
        let world = self.lit_snapshot([pos])?;
        let registry = self
            .light_registry
            .as_ref()
            .context("block registry is missing")?;
        let biomes = self.generator.possible_biomes();
        native_persistence::generated_section::encode_generated_sections(
            &world,
            pos,
            min_y,
            height,
            has_skylight,
            |state| registry.is_canonical_state(&state.name, &state.properties),
            |name| {
                ResourceId::parse(name)
                    .ok()
                    .is_some_and(|id| biomes.contains(&id))
            },
        )
    }

    /// Export the maps actually primed in this SPAWN-stage chunk, including
    /// stale generation maps. Other chunk data still needs coordinated saving.
    pub fn spawned_heightmap_data(&self, pos: ChunkPos) -> Result<native_persistence::nbt::Tag> {
        ensure!(
            self.is_complete(),
            "finish generation requests before encoding heightmaps"
        );
        ensure!(
            self.schedule
                .completed(pos)
                .is_some_and(|stage| stage >= GenerationStage::Spawn),
            "heightmap encoding requires a spawn-complete chunk"
        );
        let terrain = self
            .terrain
            .as_ref()
            .context("generated terrain is missing")?;
        ensure!(
            terrain.worldgen_heightmaps(pos).is_some(),
            "generation heightmaps are missing"
        );
        terrain.generated_heightmap_data(pos)
    }

    /// Convert the current SPAWN-stage maps as Java's LevelChunk constructor
    /// does. This is not a completed FULL chunk; later block changes still
    /// need to update the maps before saving.
    pub fn converted_full_heightmap_data(
        &self,
        pos: ChunkPos,
    ) -> Result<native_persistence::nbt::Tag> {
        let maps = self.spawned_heightmap_data(pos)?;
        let (min_y, max_y) = self.build_bounds()?;
        native_persistence::heightmap::generated_full_heightmaps(
            native_persistence::heightmap::HeightBounds { min_y, max_y },
            &maps,
        )
    }

    /// Prepare the known block entities of a spawn-complete chunk. Other
    /// chunk data remains separate. Identity-dependent Java hash ties are rejected.
    pub fn spawned_block_entity_data(&self, pos: ChunkPos) -> Result<native_persistence::nbt::Tag> {
        ensure!(
            self.is_complete(),
            "finish generation requests before encoding block entities"
        );
        ensure!(
            self.schedule
                .completed(pos)
                .is_some_and(|stage| stage >= GenerationStage::Spawn),
            "block entities require a spawn-complete chunk"
        );
        self.terrain
            .as_ref()
            .context("generated terrain is missing")?
            .generated_block_entity_data(
                pos,
                self.light_registry
                    .as_ref()
                    .context("block registry is missing")?,
            )
    }

    /// Encode generated block and fluid ticks in the order recorded by the
    /// protochunk. Sections, entities and other save data remain separate.
    pub fn spawned_tick_data(
        &self,
        pos: ChunkPos,
    ) -> Result<(native_persistence::nbt::Tag, native_persistence::nbt::Tag)> {
        ensure!(
            self.is_complete(),
            "finish generation requests before encoding ticks"
        );
        ensure!(
            self.schedule
                .completed(pos)
                .is_some_and(|stage| stage >= GenerationStage::Spawn),
            "tick encoding requires a spawn-complete chunk"
        );
        let terrain = self
            .terrain
            .as_ref()
            .context("generated terrain is missing")?;
        let registry = &self
            .decoration
            .as_ref()
            .context("generation registry is missing")?
            .block_registry;
        let world = terrain.snapshot();
        ensure!(
            world.chunks.contains_key(&pos),
            "generated chunk is missing"
        );
        let blocks = world
            .saved_block_ticks
            .get(&pos)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let fluids = world
            .saved_fluid_ticks
            .get(&pos)
            .map(Vec::as_slice)
            .unwrap_or_default();
        Ok((
            native_persistence::generated_ticks::encode_block_ticks(pos, blocks, |id| {
                registry.contains(id)
            })?,
            native_persistence::generated_ticks::encode_fluid_ticks(pos, fluids)?,
        ))
    }

    /// Convert the generated protochunk's block and fluid ticks into separate
    /// LevelChunk containers. The saved lists remain pending until the caller
    /// activates the chunk after postprocessing.
    pub fn convert_spawned_ticks(
        &self,
        pos: ChunkPos,
    ) -> Result<super::chunk_ticks::ConvertedChunkTicks> {
        use super::chunk_ticks::{ChunkTicks, ConvertedChunkTicks, SavedTick};

        // Check registry identities, uniqueness, coordinates and proto delays
        // with the same validation as the generation save draft.
        self.spawned_tick_data(pos)?;
        let world = self
            .terrain
            .as_ref()
            .context("generated terrain is missing")?
            .snapshot();
        let blocks = world
            .saved_block_ticks
            .get(&pos)
            .into_iter()
            .flatten()
            .map(|tick| SavedTick {
                kind: tick.block.clone(),
                position: tick.position,
                delay: tick.delay,
                priority: tick.priority,
            })
            .collect();
        let fluids = world
            .saved_fluid_ticks
            .get(&pos)
            .into_iter()
            .flatten()
            .map(|tick| SavedTick {
                kind: tick.fluid.clone(),
                position: tick.position,
                delay: tick.delay,
                priority: tick.priority,
            })
            .collect();
        Ok(ConvertedChunkTicks {
            blocks: ChunkTicks::from_saved(pos, blocks)?,
            fluids: ChunkTicks::from_saved(pos, fluids)?,
        })
    }

    /// Combine the ordered noise, surface, carver and feature marks. Missing
    /// earlier history is an error, since reconstructing it from blocks loses
    /// duplicates and insertion order.
    pub fn spawned_postprocessing_data(
        &self,
        pos: ChunkPos,
    ) -> Result<native_persistence::nbt::Tag> {
        ensure!(
            self.is_complete(),
            "finish generation requests before encoding postprocessing"
        );
        ensure!(
            self.schedule
                .completed(pos)
                .is_some_and(|stage| stage >= GenerationStage::Spawn),
            "postprocessing encoding requires a spawn-complete chunk"
        );
        self.terrain
            .as_ref()
            .context("generated terrain is missing")?
            .generated_postprocessing_data(pos)
    }

    /// Start postprocessing for the exact ordered marks of a SPAWN-stage
    /// chunk. The caller supplies an isolated mutable world implementation;
    /// this does not itself promote the chunk to FULL or authorize saving.
    pub fn begin_spawned_postprocessing(
        &self,
        pos: ChunkPos,
    ) -> Result<super::postprocessing::PostprocessingTask> {
        ensure!(
            self.is_complete(),
            "finish generation requests before postprocessing"
        );
        ensure!(
            self.schedule
                .completed(pos)
                .is_some_and(|stage| stage >= GenerationStage::Spawn),
            "postprocessing requires a spawn-complete chunk"
        );
        let marks = self
            .terrain
            .as_ref()
            .context("generated terrain is missing")?
            .postprocessing_marks_to_date(pos)
            .context("generation postprocessing history is missing")?;
        Ok(super::postprocessing::PostprocessingTask::new(
            Arc::new(marks),
            pos,
        ))
    }

    /// Prepare both the exact postprocessing marks and pending LevelChunk
    /// ticks from the same unchanged generated batch.
    pub fn begin_converted_postprocessing(
        &self,
        pos: ChunkPos,
    ) -> Result<super::postprocessing::ConvertedChunkPostprocessing> {
        Ok(super::postprocessing::ConvertedChunkPostprocessing {
            task: self.begin_spawned_postprocessing(pos)?,
            ticks: self.convert_spawned_ticks(pos)?,
        })
    }

    /// Prepare the FULL-stage tick containers and postprocessing tasks for a
    /// set of converted neighbors using one world time and sub-tick counter.
    /// This remains isolated intermediate data, not a playable world.
    pub fn begin_converted_postprocessing_region(
        &self,
        positions: impl IntoIterator<Item = ChunkPos>,
        game_time: i64,
        next_sub_tick: i64,
    ) -> Result<super::postprocessing::ConvertedPostprocessingRegion> {
        let mut tasks = BTreeMap::new();
        let mut ticks = BTreeMap::new();
        let mut registration = Vec::new();
        for pos in positions {
            ensure!(!tasks.contains_key(&pos), "duplicate converted chunk");
            let converted = self.begin_converted_postprocessing(pos)?;
            tasks.insert(pos, converted.task);
            ticks.insert(pos, converted.ticks);
            registration.push(pos);
        }
        ensure!(!tasks.is_empty(), "no converted chunks");
        Ok(super::postprocessing::ConvertedPostprocessingRegion {
            tasks,
            ticks: super::chunk_ticks::RegionTicks::new_ordered(
                ticks,
                game_time,
                next_sub_tick,
                &registration,
            )?,
        })
    }

    /// Start an isolated pass against the generated blocks and their actual
    /// pending ticks. Unsupported block or fluid callbacks fail the attempt.
    /// The result is intermediate and cannot yet be opened or saved.
    pub fn begin_generated_postprocessing(
        &self,
        positions: impl IntoIterator<Item = ChunkPos>,
        game_time: i64,
        next_sub_tick: i64,
    ) -> Result<super::GeneratedPostprocessing> {
        let positions: Vec<_> = positions.into_iter().collect();
        let region = self.begin_converted_postprocessing_region(
            positions.iter().copied(),
            game_time,
            next_sub_tick,
        )?;
        let terrain = self
            .terrain
            .as_ref()
            .context("generated terrain is missing")?;
        let mut block_entities = BTreeMap::new();
        for &pos in &positions {
            block_entities.insert(
                pos,
                super::postprocessing_world::ConvertedBlockEntities::from_proto(
                    self.spawned_block_entity_data(pos)?,
                    terrain.generated_entity_order.get(&pos).map(Arc::as_ref),
                    terrain.generated_pending_order.get(&pos).map(Arc::as_ref),
                )?,
            );
        }
        let world = self.snapshot().context("generated terrain is missing")?;
        // Most generated chunks do not contain mushrooms. Avoid preparing a
        // light snapshot unless this registry can actually reference one.
        let needs_mushroom_light = world.states().any(|(_, state)| {
            matches!(
                state.name.as_str(),
                "minecraft:brown_mushroom" | "minecraft:red_mushroom"
            )
        });
        let light = if needs_mushroom_light {
            Some(self.light_snapshot(positions.iter().copied())?)
        } else {
            None
        };
        let mushroom_tags = self
            .decoration
            .as_ref()
            .and_then(|config| config.mushroom_tags.clone());
        let heightmap_tags = Arc::new(
            self.decoration
                .as_ref()
                .context("generation heightmap tags are missing")?
                .heightmap_tags
                .clone(),
        );
        let (min_y, max_y) = self.build_bounds()?;
        let bounds = native_persistence::heightmap::HeightBounds { min_y, max_y };
        let mut heightmaps = BTreeMap::new();
        for &pos in &positions {
            let raw = self.converted_full_heightmap_data(pos)?;
            heightmaps.insert(
                pos,
                native_persistence::heightmap::FullHeightmaps::from_converted(bounds, &raw)?,
            );
        }
        let mut stage = super::GeneratedPostprocessing::new(
            world,
            min_y,
            max_y,
            region,
            light,
            mushroom_tags,
            heightmaps,
            Some(heightmap_tags),
        );
        stage.set_block_entities(block_entities)?;
        stage.bind_batch_revision(self.schedule.revision());
        stage.set_leaf_tags(
            self.decoration
                .as_ref()
                .and_then(|config| config.leaf_tags.clone()),
        );
        stage.set_snow_tags(
            self.decoration
                .as_ref()
                .and_then(|config| config.snow_tags.clone()),
        );
        stage.set_fence_tags(
            self.decoration
                .as_ref()
                .and_then(|config| config.fence_tags.clone()),
        );
        stage.set_bubble_tags(
            self.decoration
                .as_ref()
                .and_then(|config| config.bubble_tags.clone()),
        );
        stage.set_fluid_tags(
            self.decoration
                .as_ref()
                .and_then(|config| config.washed_blocks.clone()),
            self.decoration
                .as_ref()
                .and_then(|config| config.lava_fluids.clone()),
        );
        Ok(stage)
    }

    /// Prepare converted chunks in the supplied registration order while
    /// initially postprocessing only the selected targets. The result remains
    /// an isolated intermediate; deferred neighbors cannot yet be saved.
    pub fn begin_generated_postprocessing_with_neighbors(
        &self,
        targets: impl IntoIterator<Item = ChunkPos>,
        converted: impl IntoIterator<Item = ChunkPos>,
        game_time: i64,
        next_sub_tick: i64,
    ) -> Result<super::GeneratedPostprocessing> {
        let mut selected = BTreeSet::new();
        for pos in targets {
            ensure!(selected.insert(pos), "duplicate postprocessing target");
        }
        ensure!(!selected.is_empty(), "no postprocessing targets");
        let converted: Vec<_> = converted.into_iter().collect();
        let available: BTreeSet<_> = converted.iter().copied().collect();
        ensure!(
            available.len() == converted.len(),
            "duplicate converted chunk"
        );
        ensure!(
            selected.is_subset(&available),
            "postprocessing target is not converted"
        );
        let mut stage = self.begin_generated_postprocessing(converted, game_time, next_sub_tick)?;
        stage.select_initial_targets(selected)?;
        Ok(stage)
    }

    /// Append a spawn-complete chunk to an isolated pass at a boundary after
    /// its previous chunks have finished. The batch and its source snapshot
    /// must still be the same revision; this does not yet extend a batch that
    /// has generated additional terrain since the pass began.
    pub fn register_generated_postprocessing_chunk(
        &self,
        stage: &mut super::GeneratedPostprocessing,
        pos: ChunkPos,
    ) -> Result<()> {
        self.register_generated_postprocessing_chunk_inner(stage, pos, true)
    }

    /// Register a converted neighbor without running its postprocessing pass.
    /// The caller must supply it before retrying callbacks that need this chunk.
    pub fn register_generated_postprocessing_neighbor(
        &self,
        stage: &mut super::GeneratedPostprocessing,
        pos: ChunkPos,
    ) -> Result<()> {
        self.register_generated_postprocessing_chunk_inner(stage, pos, false)
    }

    fn register_generated_postprocessing_chunk_inner(
        &self,
        stage: &mut super::GeneratedPostprocessing,
        pos: ChunkPos,
        required: bool,
    ) -> Result<()> {
        stage.check_registration(pos)?;
        ensure!(
            stage.batch_revision == Some(self.schedule.revision())
                && self
                    .snapshot()
                    .is_some_and(|world| Arc::ptr_eq(&world, &stage.source)),
            "postprocessing belongs to a different generation revision"
        );
        let converted = self.begin_converted_postprocessing(pos)?;
        let terrain = self
            .terrain
            .as_ref()
            .context("generated terrain is missing")?;
        let entities = super::postprocessing_world::ConvertedBlockEntities::from_proto(
            self.spawned_block_entity_data(pos)?,
            terrain.generated_entity_order.get(&pos).map(Arc::as_ref),
            terrain.generated_pending_order.get(&pos).map(Arc::as_ref),
        )?;
        let (min_y, max_y) = self.build_bounds()?;
        let heightmaps = native_persistence::heightmap::FullHeightmaps::from_converted(
            native_persistence::heightmap::HeightBounds { min_y, max_y },
            &self.converted_full_heightmap_data(pos)?,
        )?;
        let light = if stage.light.is_some() {
            let mut covered: BTreeSet<_> = stage.region.tasks.keys().copied().collect();
            covered.insert(pos);
            Some(self.postprocessing_light_input(stage.light_checkpoint.as_ref(), covered)?)
        } else {
            None
        };
        if required {
            stage.register_converted_chunk(pos, converted, entities, heightmaps, light)
        } else {
            stage.register_converted_chunk_inner(pos, converted, entities, heightmaps, light, false)
        }
    }

    /// Reopen a completed pass against the same validated batch revision.
    /// Previously completed callbacks, tick history and block entities are
    /// retained; the source result remains unchanged if the new pass fails.
    pub fn resume_generated_postprocessing(
        &self,
        completed: &super::PostprocessedGeneratedRegion,
    ) -> Result<super::GeneratedPostprocessing> {
        self.validate_postprocessing(completed)?;
        ensure!(!completed.chunks.is_empty(), "no postprocessed chunks");
        let config = self
            .decoration
            .as_ref()
            .context("generation configuration is missing")?;
        let light = if let Some(checkpoint) = &completed.light_checkpoint {
            Some(checkpoint.visible.clone())
        } else if completed.world.states().any(|(_, state)| {
            matches!(
                state.name.as_str(),
                "minecraft:brown_mushroom" | "minecraft:red_mushroom"
            )
        }) {
            Some(
                self.light_snapshot(
                    completed
                        .chunks
                        .iter()
                        .copied()
                        .chain(completed.deferred_tasks.keys().copied()),
                )?,
            )
        } else {
            None
        };
        let mut stage = super::GeneratedPostprocessing::from_completed(
            completed,
            light,
            config.mushroom_tags.clone(),
            Arc::new(config.heightmap_tags.clone()),
        )?;
        stage.set_leaf_tags(config.leaf_tags.clone());
        stage.set_snow_tags(config.snow_tags.clone());
        stage.set_fence_tags(config.fence_tags.clone());
        stage.set_bubble_tags(config.bubble_tags.clone());
        stage.set_fluid_tags(config.washed_blocks.clone(), config.lava_fluids.clone());
        Ok(stage)
    }

    /// Drain the checks recorded so far at a caller-selected boundary between
    /// postprocessing polls. A failed drain leaves the stage unchanged. The
    /// caller must reproduce Java's actual lighting schedule; this API does not
    /// infer when asynchronous engine work ran.
    pub fn drain_generated_postprocessing_light(
        &self,
        stage: &mut super::GeneratedPostprocessing,
    ) -> Result<bool> {
        ensure!(!stage.failed, "generated postprocessing attempt failed");
        ensure!(
            stage.batch_revision == Some(self.schedule.revision())
                && self
                    .snapshot()
                    .is_some_and(|world| Arc::ptr_eq(&world, &stage.source)),
            "postprocessing belongs to a different generation revision"
        );
        let engine = self
            .light_engine
            .as_ref()
            .context("light propagation is not sealed")?;
        let checkpoint = stage.light_checkpoint.as_ref();
        let drained = checkpoint.map_or(0, |saved| saved.drained_checks);
        let pending = stage
            .light_checks
            .get(drained..)
            .context("invalid postprocessing light checkpoint")?;
        if pending.is_empty() {
            return Ok(false);
        }
        let mut covered: BTreeSet<_> = stage.region.tasks.keys().copied().collect();
        extend_postprocessing_light_coverage(&mut covered, pending)?;
        let before = self.postprocessing_light_input(checkpoint, covered)?;
        stage.drain_recorded_light(before, &engine.active, &engine.fresh.sky_sources)
    }

    fn postprocessing_light_input(
        &self,
        checkpoint: Option<&super::postprocessing_world::PostprocessingLightCheckpoint>,
        covered: BTreeSet<ChunkPos>,
    ) -> Result<VisibleLightSnapshot> {
        let Some(checkpoint) = checkpoint else {
            return self.light_snapshot(covered);
        };
        let mut before = checkpoint.visible.clone();
        let missing: BTreeSet<_> = covered
            .into_iter()
            .filter(|pos| before.complete_column(*pos).is_none())
            .collect();
        if !missing.is_empty() {
            let fresh = self.light_snapshot(missing.iter().copied())?;
            for pos in missing {
                let column = fresh
                    .complete_column(pos)
                    .context("new postprocessing light column is missing")?;
                ensure!(
                    before.insert_complete_column(pos, column).is_none(),
                    "postprocessing light column changed"
                );
            }
        }
        Ok(before)
    }

    /// Check that a completed pass still belongs to this batch and was not changed.
    pub fn validate_postprocessing(
        &self,
        processed: &super::PostprocessedGeneratedRegion,
    ) -> Result<()> {
        ensure!(
            !self.failed,
            "generation batch failed and must be discarded"
        );
        ensure!(
            processed.batch_revision == Some(self.schedule.revision())
                && self
                    .snapshot()
                    .is_some_and(|world| Arc::ptr_eq(&world, &processed.source)),
            "postprocessing belongs to a different generation revision"
        );
        ensure!(
            Arc::ptr_eq(&processed.world, &processed.sealed_world)
                && processed.edits == processed.sealed_edits
                && processed.read_chunks == processed.sealed_read_chunks
                && processed.light_reads == processed.sealed_light_reads
                && processed.light_checks == processed.sealed_checks
                && processed.chunks == processed.sealed_chunks,
            "postprocessing result was modified"
        );
        let (min_y, max_y) = self.build_bounds()?;
        ensure!(
            (min_y, max_y) == (processed.min_y, processed.max_y),
            "postprocessing light bounds changed"
        );
        Ok(())
    }

    fn postprocessing_source_inputs(&self, pos: ChunkPos) -> Result<PostprocessingSourceInputs> {
        let (block_ticks, fluid_ticks) = self.spawned_tick_data(pos)?;
        Ok(PostprocessingSourceInputs {
            heightmaps: self.spawned_heightmap_data(pos)?,
            block_entities: self.spawned_block_entity_data(pos)?,
            block_ticks,
            fluid_ticks,
            postprocessing: self.spawned_postprocessing_data(pos)?,
            entities: self.recorded_generated_entities(pos)?.to_vec(),
            complete_entities: self
                .complete_generated_entity_records(pos)?
                .map(<[_]>::to_vec),
        })
    }

    /// Capture the immutable source and lighting used by a completed pass
    /// before generating more chunks. The caller retains the original result.
    pub fn checkpoint_postprocessing_expansion(
        &self,
        processed: &super::PostprocessedGeneratedRegion,
    ) -> Result<PostprocessingExpansionCheckpoint> {
        self.validate_postprocessing(processed)?;
        ensure!(
            processed.light_checkpoint.is_none(),
            "postprocessing with a drained light checkpoint cannot yet be rebased"
        );
        let engine = self
            .light_engine
            .as_ref()
            .context("light propagation is not sealed")?;
        let light = self.light_snapshot(processed.light_reads.iter().copied())?;
        let sky_sources = processed
            .light_reads
            .iter()
            .filter_map(|pos| {
                engine
                    .fresh
                    .sky_sources
                    .get(pos)
                    .map(|source| (*pos, Arc::clone(source)))
            })
            .collect();
        let converted: BTreeSet<_> = processed
            .chunks
            .iter()
            .copied()
            .chain(processed.deferred_chunks())
            .collect();
        let inputs = converted
            .iter()
            .map(|&pos| Ok((pos, self.postprocessing_source_inputs(pos)?)))
            .collect::<Result<_>>()?;
        Ok(PostprocessingExpansionCheckpoint {
            batch_identity: Arc::clone(&self.identity),
            region_identity: Arc::clone(&processed.identity),
            source: Arc::clone(&processed.source),
            processed_world: Arc::clone(&processed.world),
            revision: self.schedule.revision(),
            chunks: processed.chunks.clone(),
            converted,
            edits: processed.edits.clone(),
            read_chunks: processed.read_chunks.clone(),
            light_reads: processed.light_reads.clone(),
            light_checks: processed.light_checks.clone(),
            inputs,
            active_light: engine.active.clone(),
            light,
            sky_sources,
        })
    }

    /// Rebase an isolated pass only if its old inputs and observed light
    /// columns are unchanged. Errors leave `processed` untouched.
    /// Newly generated chunks are not yet converted or registered.
    pub fn rebase_postprocessing_after_expansion(
        &self,
        processed: &mut super::PostprocessedGeneratedRegion,
        checkpoint: &PostprocessingExpansionCheckpoint,
    ) -> Result<()> {
        ensure!(
            !self.failed && self.is_complete(),
            "generation batch is not complete"
        );
        ensure!(
            Arc::ptr_eq(&self.identity, &checkpoint.batch_identity),
            "postprocessing belongs to another batch"
        );
        ensure!(
            processed.batch_revision == Some(checkpoint.revision)
                && Arc::ptr_eq(&processed.identity, &checkpoint.region_identity)
                && Arc::ptr_eq(&processed.source, &checkpoint.source)
                && Arc::ptr_eq(&processed.world, &checkpoint.processed_world)
                && Arc::ptr_eq(&processed.world, &processed.sealed_world)
                && processed.chunks == checkpoint.chunks
                && processed.chunks == processed.sealed_chunks
                && processed
                    .chunks
                    .iter()
                    .copied()
                    .chain(processed.deferred_chunks())
                    .collect::<BTreeSet<_>>()
                    == checkpoint.converted
                && processed.edits == checkpoint.edits
                && processed.edits == processed.sealed_edits
                && processed.read_chunks == checkpoint.read_chunks
                && processed.read_chunks == processed.sealed_read_chunks
                && processed.light_reads == checkpoint.light_reads
                && processed.light_reads == processed.sealed_light_reads
                && processed.light_checks == checkpoint.light_checks
                && processed.light_checks == processed.sealed_checks
                && processed.light_checkpoint.is_none(),
            "postprocessing changed after the expansion checkpoint"
        );
        let (min_y, max_y) = self.build_bounds()?;
        ensure!(
            (min_y, max_y) == (processed.min_y, processed.max_y),
            "generation bounds changed"
        );
        let current = self.snapshot().context("generated terrain is missing")?;
        // An unconverted read can change when another chunk is registered;
        // do not reuse the old pass without replaying that observation.
        let observed = postprocessing_observed_chunks(
            &checkpoint.converted,
            &checkpoint.read_chunks,
            &checkpoint.edits,
        )
        .context("postprocessing observed an unconverted chunk")?;
        ensure!(
            unchanged_generation_world_for(&checkpoint.source, &current, &observed),
            "existing generation terrain changed during expansion"
        );
        for (&pos, before) in &checkpoint.inputs {
            ensure!(
                *before == self.postprocessing_source_inputs(pos)?,
                "generation data changed for postprocessed chunk {}, {}",
                pos.x,
                pos.z
            );
        }
        let engine = self
            .light_engine
            .as_ref()
            .context("light propagation is not sealed")?;
        ensure!(
            checkpoint.active_light.is_subset(&engine.active),
            "previously active light chunks were removed"
        );
        let light = self.light_snapshot(checkpoint.light_reads.iter().copied())?;
        // Only columns actually read by callbacks affect completed results.
        // Other columns and sky sources are taken from the expanded engine
        // when pending postprocessing light checks are drained.
        let observed_light = &checkpoint.light_reads;
        ensure!(
            unchanged_visible_light(
                &checkpoint.light,
                &light,
                observed_light,
            ) && observed_light
                .iter()
                .all(|pos| checkpoint.sky_sources.get(pos) == engine.fresh.sky_sources.get(pos)),
            "existing generation lighting changed during expansion"
        );
        let rebased = replay_postprocessing_edits(&current, &processed.world, &processed.edits)?;
        processed.source = current;
        processed.world = rebased;
        processed.sealed_world = Arc::clone(&processed.world);
        processed.batch_revision = Some(self.schedule.revision());
        Ok(())
    }

    /// Preview one drained batch of the postprocessing light checks. The
    /// caller must choose a drain point with no other pending light-engine
    /// work. Both channels are updated on an isolated snapshot; neither the
    /// batch nor the intermediate block result is changed on failure.
    pub fn postprocessed_light_snapshot(
        &self,
        processed: &super::PostprocessedGeneratedRegion,
    ) -> Result<VisibleLightSnapshot> {
        self.postprocessed_light_snapshot_covering(processed, BTreeSet::new())
    }

    fn postprocessed_light_snapshot_covering(
        &self,
        processed: &super::PostprocessedGeneratedRegion,
        mut covered: BTreeSet<ChunkPos>,
    ) -> Result<VisibleLightSnapshot> {
        self.validate_postprocessing(processed)?;
        let (min_y, max_y) = self.build_bounds()?;
        let bounds = native_world::block_light::LightBounds::new(min_y, max_y)
            .context("unsupported postprocessing light bounds")?;
        let engine = self
            .light_engine
            .as_ref()
            .context("light propagation is not sealed")?;
        covered.extend(processed.chunks.iter().copied());
        let checkpoint = processed.light_checkpoint.as_ref();
        let drained = checkpoint.map_or(0, |saved| saved.drained_checks);
        let pending = processed
            .sealed_checks
            .get(drained..)
            .context("invalid postprocessing light checkpoint")?;
        extend_postprocessing_light_coverage(&mut covered, pending)?;
        let checks: Vec<_> = pending.iter().map(|pos| [pos.x, pos.y, pos.z]).collect();
        let before = self.postprocessing_light_input(checkpoint, covered)?;
        let previous_world =
            checkpoint.map_or(processed.source.as_ref(), |saved| saved.world.as_ref());
        let previous_sources =
            checkpoint.map_or(&engine.fresh.sky_sources, |saved| &saved.sky_sources);
        let updated_sources = if before.has_sky_channel() {
            updated_postprocessing_sky_sources(
                previous_world,
                &processed.world,
                pending,
                bounds,
                previous_sources,
            )?
        } else {
            None
        };
        super::postprocessing_world::drain_postprocessing_light(
            &processed.world,
            bounds,
            &before,
            &engine.active,
            updated_sources.as_ref().unwrap_or(previous_sources),
            &checks,
        )
    }

    /// Encode postprocessed blocks and drained light for a set of chunks.
    /// One light drain serves the entire set. Other FULL chunk data and
    /// world-save validation remain separate.
    pub fn postprocessed_sections_data(
        &self,
        processed: &super::PostprocessedGeneratedRegion,
        positions: impl IntoIterator<Item = ChunkPos>,
    ) -> Result<BTreeMap<ChunkPos, native_persistence::nbt::Tag>> {
        self.converted_sections_data(processed, positions, false)
    }

    fn converted_sections_data(
        &self,
        processed: &super::PostprocessedGeneratedRegion,
        positions: impl IntoIterator<Item = ChunkPos>,
        include_deferred: bool,
    ) -> Result<BTreeMap<ChunkPos, native_persistence::nbt::Tag>> {
        let mut selected = BTreeSet::new();
        for pos in positions {
            ensure!(selected.insert(pos), "duplicate postprocessed chunk");
            ensure!(
                processed.chunks.contains(&pos)
                    || (include_deferred && processed.deferred_tasks.contains_key(&pos)),
                "chunk was not selected for this postprocessing pass"
            );
        }
        ensure!(!selected.is_empty(), "no postprocessed chunks selected");
        let light = self.postprocessed_light_snapshot_covering(processed, selected.clone())?;
        let world = self.materialize_postprocessed_light(processed, &light, &selected)?;
        let has_skylight = self
            .decoration
            .as_ref()
            .context("generation lighting is missing")?
            .has_skylight;
        ensure!(
            light.has_block_channel() && light.has_sky_channel() == has_skylight,
            "postprocessing light channels differ from the dimension"
        );
        let (min_y, _) = self.build_bounds()?;
        let registry = self
            .light_registry
            .as_ref()
            .context("block registry is missing")?;
        let biomes = self.generator.possible_biomes();
        selected
            .into_iter()
            .map(|pos| {
                let sections = native_persistence::generated_section::encode_generated_sections(
                    &world,
                    pos,
                    min_y,
                    self.generator.height,
                    has_skylight,
                    |state| registry.is_canonical_state(&state.name, &state.properties),
                    |name| {
                        ResourceId::parse(name)
                            .ok()
                            .is_some_and(|id| biomes.contains(&id))
                    },
                )?;
                Ok((pos, sections))
            })
            .collect()
    }

    fn materialize_postprocessed_light(
        &self,
        processed: &super::PostprocessedGeneratedRegion,
        light: &VisibleLightSnapshot,
        covered: &BTreeSet<ChunkPos>,
    ) -> Result<Arc<World>> {
        use native_world::loaded::Section;
        let has_skylight = self
            .decoration
            .as_ref()
            .context("generation lighting is missing")?
            .has_skylight;
        ensure!(
            light.has_block_channel() && light.has_sky_channel() == has_skylight,
            "postprocessing light channels differ from the dimension"
        );
        let (min_y, max_y) = self.build_bounds()?;
        let first = min_y.div_euclid(16);
        let end = max_y.div_euclid(16);
        let mut world = (*processed.world).clone();
        for &pos in covered {
            let chunk = Arc::make_mut(
                world
                    .chunks
                    .get_mut(&pos)
                    .context("postprocessed chunk is missing")?,
            );
            ensure!(
                chunk.position == pos,
                "postprocessed chunk position differs"
            );
            for sy in first..end {
                let section = chunk
                    .sections
                    .get(&i8::try_from(sy)?)
                    .context("postprocessed block section is missing")?;
                ensure!(
                    section.blocks.is_some(),
                    "postprocessed block data is missing"
                );
            }
            for sy in first - 1..end + 1 {
                let key = i8::try_from(sy).context("postprocessed light section out of range")?;
                let (block, sky) = light
                    .expanded_section(pos, sy)
                    .context("postprocessed light column was not captured")?;
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
        Ok(Arc::new(world))
    }

    /// Capture postprocessed blocks and light for targets and their 3x3
    /// neighborhoods. This snapshot is intermediate and does not authorize saving.
    pub fn postprocessed_render_snapshot(
        &self,
        processed: &super::PostprocessedGeneratedRegion,
        targets: impl IntoIterator<Item = ChunkPos>,
    ) -> Result<Arc<World>> {
        let targets: BTreeSet<_> = targets.into_iter().collect();
        ensure!(!targets.is_empty(), "no postprocessed render targets");
        let mut covered = BTreeSet::new();
        for pos in targets {
            ensure!(
                processed.chunks.contains(&pos),
                "render target was not postprocessed"
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
        let light = self.postprocessed_light_snapshot_covering(processed, covered.clone())?;
        self.materialize_postprocessed_light(processed, &light, &covered)
    }

    /// Prepare section, heightmap and tick data from the same completed pass.
    /// This is intermediate data and does not authorize saving a world.
    pub fn postprocessed_chunk_components(
        &self,
        processed: &super::PostprocessedGeneratedRegion,
        positions: impl IntoIterator<Item = ChunkPos>,
    ) -> Result<BTreeMap<ChunkPos, PostprocessedChunkComponents>> {
        ensure!(
            processed.deferred_tasks.is_empty(),
            "postprocessed drafts with deferred chunks are unsupported"
        );
        self.converted_chunk_components(processed, positions, None)
    }

    fn converted_chunk_components(
        &self,
        processed: &super::PostprocessedGeneratedRegion,
        positions: impl IntoIterator<Item = ChunkPos>,
        deferred_entities: Option<&BTreeMap<ChunkPos, native_persistence::nbt::Tag>>,
    ) -> Result<BTreeMap<ChunkPos, PostprocessedChunkComponents>> {
        use native_world::loaded::{SavedBlockTick, SavedFluidTick};

        let sections =
            self.converted_sections_data(processed, positions, deferred_entities.is_some())?;
        let registry = &self
            .decoration
            .as_ref()
            .context("generation registry is missing")?
            .block_registry;
        let block_entity_registry = self
            .light_registry
            .as_ref()
            .context("block registry is missing")?;
        for edit in &processed.sealed_edits {
            if !sections.contains_key(&edit.chunk()) {
                continue;
            }
            let old = processed
                .source
                .block(edit.x, edit.y, edit.z)
                .context("original edited block is missing")?;
            let new = processed
                .world
                .block(edit.x, edit.y, edit.z)
                .context("postprocessed edited block is missing")?;
            if old != new {
                let has_entity = |state: &BlockState| {
                    block_entity_registry
                        .is_canonical_state(&state.name, &state.properties)
                        .then(|| block_entity_registry.has_block_entity(&state.name))
                        .flatten()
                        .context("edited block entity classification is unavailable")
                };
                ensure!(
                    (!has_entity(old)? && !has_entity(new)?)
                        || processed.permits_block_entity_edit(*edit, old, new),
                    "postprocessing changed a block entity"
                );
            }
        }
        let time = processed.ticks.game_time();
        sections
            .into_iter()
            .map(|(pos, sections)| {
                let ticks = processed
                    .ticks
                    .chunk(pos)
                    .context("postprocessed tick container is missing")?;
                let blocks: Vec<_> = ticks
                    .blocks
                    .pack(time)
                    .into_iter()
                    .map(|tick| SavedBlockTick {
                        block: tick.kind,
                        position: tick.position,
                        delay: tick.delay,
                        priority: tick.priority,
                    })
                    .collect();
                let fluids: Vec<_> = ticks
                    .fluids
                    .pack(time)
                    .into_iter()
                    .map(|tick| SavedFluidTick {
                        fluid: tick.kind,
                        position: tick.position,
                        delay: tick.delay,
                        priority: tick.priority,
                    })
                    .collect();
                Ok((
                    pos,
                    PostprocessedChunkComponents {
                        sections,
                        heightmaps: processed
                            .heightmaps
                            .get(&pos)
                            .context("postprocessed heightmaps are missing")?
                            .to_tag()?,
                        block_ticks: native_persistence::generated_ticks::encode_level_block_ticks(
                            pos,
                            &blocks,
                            |id| registry.contains(id),
                        )?,
                        fluid_ticks: native_persistence::generated_ticks::encode_level_fluid_ticks(
                            pos, &fluids,
                        )?,
                        block_entities: if processed.chunks.contains(&pos) {
                            processed.block_entity_data(pos)?
                        } else {
                            deferred_entities
                                .and_then(|entities| entities.get(&pos))
                                .context("deferred block entity snapshot is missing")?
                                .clone()
                        },
                        entity_records: self.complete_generated_entity_records_shared(pos)?,
                    },
                ))
            })
            .collect()
    }

    /// Assemble FULL chunk drafts using the isolated pass's exact game time.
    /// Entity records stay separate and can remain unknown.
    pub fn postprocessed_chunk_drafts(
        &self,
        processed: &super::PostprocessedGeneratedRegion,
        positions: impl IntoIterator<Item = ChunkPos>,
    ) -> Result<BTreeMap<ChunkPos, PostprocessedChunkDraft>> {
        self.postprocessed_chunk_drafts_with_validated_structures(processed, positions, None)
    }

    fn postprocessed_chunk_drafts_with_validated_structures(
        &self,
        processed: &super::PostprocessedGeneratedRegion,
        positions: impl IntoIterator<Item = ChunkPos>,
        validated: Option<&BTreeSet<ChunkPos>>,
    ) -> Result<BTreeMap<ChunkPos, PostprocessedChunkDraft>> {
        let time = processed.ticks.game_time();
        let components = self.postprocessed_chunk_components(processed, positions)?;
        self.assemble_converted_drafts(components, time, validated, &BTreeMap::new())
    }

    /// Capture every converted chunk from one isolated postprocessing stage.
    /// Deferred marks and save-time block entity promotions are preserved.
    /// This is an intermediate preview, not permission to publish a world.
    /// The stage is changed only after the entire preview succeeds.
    pub fn converted_chunk_drafts(
        &self,
        stage: &mut super::GeneratedPostprocessing,
        positions: impl IntoIterator<Item = ChunkPos>,
    ) -> Result<BTreeMap<ChunkPos, PostprocessedChunkDraft>> {
        let positions: Vec<_> = positions.into_iter().collect();
        let selected: BTreeSet<_> = positions.iter().copied().collect();
        ensure!(!selected.is_empty(), "no converted chunks selected");
        ensure!(
            selected.len() == positions.len(),
            "duplicate converted chunk"
        );
        let mut candidate = stage.clone();
        let before = candidate.clone().finish()?;
        self.validate_postprocessing(&before)?;
        let available: BTreeSet<_> = before
            .chunks
            .iter()
            .copied()
            .chain(before.deferred_chunks())
            .collect();
        ensure!(
            selected == available,
            "all converted chunks must be selected"
        );
        let mut entities = BTreeMap::new();
        for &pos in &positions {
            if before.deferred_tasks.contains_key(&pos) {
                entities.insert(pos, candidate.capture_deferred_block_entities(pos)?);
            }
        }
        let processed = candidate.clone().finish()?;
        let marks = processed
            .deferred_chunks()
            .map(|pos| Ok((pos, processed.deferred_postprocessing_data(pos)?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        let components = self.converted_chunk_components(&processed, positions, Some(&entities))?;
        let drafts =
            self.assemble_converted_drafts(components, processed.ticks.game_time(), None, &marks)?;
        *stage = candidate;
        Ok(drafts)
    }

    fn assemble_converted_drafts(
        &self,
        components: BTreeMap<ChunkPos, PostprocessedChunkComponents>,
        time: i64,
        validated: Option<&BTreeSet<ChunkPos>>,
        deferred_marks: &BTreeMap<ChunkPos, native_persistence::nbt::Tag>,
    ) -> Result<BTreeMap<ChunkPos, PostprocessedChunkDraft>> {
        components
            .into_iter()
            .map(|(pos, components)| {
                let entity_chunk = components
                    .entity_records
                    .as_ref()
                    .map(|records| {
                        native_persistence::generated_chunk::assemble_entity_chunk(pos, records)
                    })
                    .transpose()?;
                let parts = native_persistence::generated_chunk::FullChunkComponents {
                    position: pos,
                    min_y: self.generator.min_y,
                    height: self.generator.height,
                    last_update: time,
                    sections: components.sections,
                    heightmaps: components.heightmaps,
                    block_ticks: components.block_ticks,
                    fluid_ticks: components.fluid_ticks,
                    block_entities: components.block_entities,
                };
                let chunk = if let Some(marks) = deferred_marks.get(&pos) {
                    native_persistence::generated_chunk::assemble_full_chunk_with_postprocessing(
                        parts,
                        marks.clone(),
                    )?
                } else {
                    native_persistence::generated_chunk::assemble_full_chunk(parts)?
                };
                let chunk = if self.structures.is_some() {
                    let data = if validated.is_some_and(|selection| selection.contains(&pos)) {
                        self.pack_validated_structure_data(pos)?
                    } else {
                        self.structure_data(pos)?
                    };
                    attach_generated_structure_data(chunk, data)?
                } else {
                    chunk
                };
                Ok((
                    pos,
                    PostprocessedChunkDraft {
                        chunk,
                        entity_records: components.entity_records,
                        entity_chunk,
                    },
                ))
            })
            .collect()
    }

    /// Preview a missing-POI consistency refresh against postprocessed blocks.
    /// The caller must establish that POIs and tickets did not exist earlier;
    /// this snapshot does not authorize writing a world.
    pub fn postprocessed_missing_poi_snapshot(
        &self,
        processed: &super::PostprocessedGeneratedRegion,
        positions: impl IntoIterator<Item = ChunkPos>,
    ) -> Result<BTreeMap<ChunkPos, Option<native_persistence::nbt::NamedTag>>> {
        self.converted_missing_poi_snapshot(processed, positions, false)
    }

    fn converted_missing_poi_snapshot(
        &self,
        processed: &super::PostprocessedGeneratedRegion,
        positions: impl IntoIterator<Item = ChunkPos>,
        include_deferred: bool,
    ) -> Result<BTreeMap<ChunkPos, Option<native_persistence::nbt::NamedTag>>> {
        self.validate_postprocessing(processed)?;
        let registry = self
            .light_registry
            .as_ref()
            .context("block registry is missing")?;
        let mut output = BTreeMap::new();
        for pos in positions {
            ensure!(
                processed.chunks.contains(&pos)
                    || (include_deferred && processed.deferred_tasks.contains_key(&pos)),
                "chunk was not selected for this postprocessing pass"
            );
            ensure!(!output.contains_key(&pos), "duplicate postprocessed chunk");
            let poi_pos = native_persistence::world::ChunkPos { x: pos.x, z: pos.z };
            let draft = native_persistence::poi::refresh_missing_chunk(
                poi_pos,
                &processed.world,
                self.generator.min_y,
                self.generator.height,
                |state| {
                    ensure!(
                        registry.is_canonical_state(&state.name, &state.properties),
                        "unknown POI block state"
                    );
                    Ok(())
                },
            )?;
            output.insert(pos, draft);
        }
        ensure!(!output.is_empty(), "no postprocessed chunks selected");
        Ok(output)
    }

    /// Validate a self-contained structure selection and assemble its terrain,
    /// entity and POI regions in memory. The drafts are discarded on return;
    /// this preview does not authorize staging or publishing a world.
    pub fn preview_structure_regions(
        &self,
        processed: &super::PostprocessedGeneratedRegion,
        positions: impl IntoIterator<Item = ChunkPos>,
        timestamp: u32,
    ) -> Result<StructureRegionPreview> {
        let prepared = self.prepare_structure_region_drafts(processed, positions, timestamp)?;
        use native_persistence::save::FreshRegionKind;
        Ok(StructureRegionPreview {
            chunk_count: prepared.chunk_count,
            referenced_start_chunks: prepared.referenced_start_chunks,
            terrain_regions: prepared
                .regions
                .keys()
                .filter(|(kind, _, _)| *kind == FreshRegionKind::Terrain)
                .count(),
            entity_regions: prepared
                .regions
                .keys()
                .filter(|(kind, _, _)| *kind == FreshRegionKind::Entities)
                .count(),
            poi_regions: prepared
                .regions
                .keys()
                .filter(|(kind, _, _)| *kind == FreshRegionKind::Poi)
                .count(),
        })
    }

    /// Prepare self-contained terrain, entity and POI regions in memory.
    /// Validated structure references do not establish the remaining world
    /// initialization requirements; these drafts must not be published.
    pub fn prepare_structure_region_drafts(
        &self,
        processed: &super::PostprocessedGeneratedRegion,
        positions: impl IntoIterator<Item = ChunkPos>,
        timestamp: u32,
    ) -> Result<PreparedStructureRegions> {
        ensure!(
            self.structures.is_some(),
            "structure generation is disabled"
        );
        self.validate_postprocessing(processed)?;
        let positions: Vec<_> = positions.into_iter().collect();
        let selected: BTreeSet<_> = positions.iter().copied().collect();
        ensure!(
            selected.len() == positions.len(),
            "duplicate postprocessed chunk"
        );
        let referenced_start_chunks = require_selected_structure_sources(&selected, |pos| {
            self.structure_reference_sources(pos)
        })?;
        let chunks = self.postprocessed_chunk_drafts_with_validated_structures(
            processed,
            positions.iter().copied(),
            Some(&selected),
        )?;
        let poi = self.postprocessed_missing_poi_snapshot(processed, positions)?;
        let regions = group_fresh_regions(chunks, poi, timestamp)?;
        Ok(PreparedStructureRegions {
            chunk_count: selected.len(),
            referenced_start_chunks,
            regions,
        })
    }

    /// Prepare terrain, nonempty entity and POI regions from the same sealed
    /// postprocessing pass. No filesystem writes or world publication occur.
    pub fn postprocessed_region_drafts(
        &self,
        processed: &super::PostprocessedGeneratedRegion,
        positions: impl IntoIterator<Item = ChunkPos>,
        timestamp: u32,
    ) -> Result<
        BTreeMap<
            (native_persistence::save::FreshRegionKind, i32, i32),
            native_persistence::save::FreshRegionDraft,
        >,
    > {
        ensure!(
            self.structures.is_none(),
            "structure world persistence is not yet supported"
        );
        let positions: Vec<_> = positions.into_iter().collect();
        let chunks = self.postprocessed_chunk_drafts(processed, positions.iter().copied())?;
        let poi = self.postprocessed_missing_poi_snapshot(processed, positions)?;
        group_fresh_regions(chunks, poi, timestamp)
    }

    /// Prepare all converted chunks, including deferred FULL neighbors, as a
    /// single in-memory terrain/entity/POI region set. Missing entity coverage
    /// rejects the entire preview; no world files are written or authorized.
    pub fn converted_region_drafts(
        &self,
        processed: &super::PostprocessedGeneratedRegion,
        timestamp: u32,
    ) -> Result<
        BTreeMap<
            (native_persistence::save::FreshRegionKind, i32, i32),
            native_persistence::save::FreshRegionDraft,
        >,
    > {
        ensure!(
            self.structures.is_none(),
            "structure world persistence is not yet supported"
        );
        let positions = processed.ticks().registration_order().to_vec();
        let mut stage = self.resume_generated_postprocessing(processed)?;
        let chunks = self.converted_chunk_drafts(&mut stage, positions.iter().copied())?;
        let captured = stage.finish()?;
        let poi = self.converted_missing_poi_snapshot(&captured, positions, true)?;
        group_fresh_regions(chunks, poi, timestamp)
    }

    /// Assemble an intermediate SPAWN protochunk. The caller must provide the
    /// complete feature-generated entity list and the world's current tick.
    /// This is not a FULL chunk and must not be written as a playable world.
    pub fn spawned_chunk_draft(
        &mut self,
        pos: ChunkPos,
        last_update: i64,
        entities: Vec<native_persistence::nbt::Tag>,
    ) -> Result<native_persistence::nbt::NamedTag> {
        let sections = self.spawned_section_data(pos)?;
        let heightmaps = self.spawned_heightmap_data(pos)?;
        let block_entities = self.spawned_block_entity_data(pos)?;
        let (block_ticks, fluid_ticks) = self.spawned_tick_data(pos)?;
        let postprocessing = self.spawned_postprocessing_data(pos)?;
        let chunk = native_persistence::generated_chunk::assemble_spawn_chunk(
            native_persistence::generated_chunk::SpawnChunkComponents {
                position: pos,
                min_y: self.generator.min_y,
                height: self.generator.height,
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
        if self.structures.is_some() {
            attach_generated_structure_data(chunk, self.structure_data(pos)?)
        } else {
            Ok(chunk)
        }
    }

    /// Use records captured by implemented generation paths. Other feature or
    /// mob sources may still be missing, so this draft is not saveable.
    pub fn spawned_recorded_chunk_draft(
        &mut self,
        pos: ChunkPos,
        last_update: i64,
    ) -> Result<native_persistence::nbt::NamedTag> {
        let entities = self
            .terrain
            .as_ref()
            .context("generated terrain is missing")?
            .recorded_generated_entities(pos)?
            .to_vec();
        self.spawned_chunk_draft(pos, last_update, entities)
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

    /// Preserve already-primed generation heightmaps, including stale values
    /// caused by direct section writes. `None` means Java has not primed them.
    pub fn spawn_heightmaps(
        &self,
        pos: ChunkPos,
    ) -> Result<Option<native_persistence::heightmap::SpawnHeightmaps>> {
        use native_persistence::heightmap::{HeightBounds, SpawnHeightmaps};
        ensure!(
            !self.failed,
            "generation batch failed and must be discarded"
        );
        ensure!(
            self.schedule
                .completed(pos)
                .is_some_and(|stage| stage >= GenerationStage::Features),
            "generation chunk has not completed features"
        );
        let terrain = self
            .terrain
            .as_ref()
            .context("generated terrain is missing")?;
        let Some(maps) = terrain.final_heightmaps.get(&pos) else {
            return Ok(None);
        };
        let (min_y, max_y) = self.build_bounds()?;
        Ok(Some(SpawnHeightmaps::from_optional_first_available(
            HeightBounds { min_y, max_y },
            (maps.primed & 0b0100 != 0).then_some(&maps.values[2]),
            (maps.primed & 0b0001 != 0).then_some(&maps.values[0]),
            (maps.primed & 0b0010 != 0).then_some(&maps.values[1]),
        )?))
    }

    /// Query a previously primed mob heightmap, preserving any stale value
    /// caused by direct generation writes. None requires separate priming.
    pub fn primed_mob_spawn_height(
        &self,
        x: i32,
        z: i32,
        kind: native_resources::mob_spawn_placement::MobSpawnHeightmap,
    ) -> Result<Option<i32>> {
        let pos = ChunkPos {
            x: x.div_euclid(16),
            z: z.div_euclid(16),
        };
        ensure!(
            !self.failed
                && self
                    .schedule
                    .completed(pos)
                    .is_some_and(|stage| stage >= GenerationStage::Light),
            "mob spawn height requires a light-complete chunk"
        );
        self.terrain
            .as_ref()
            .context("generated terrain is missing")?
            .primed_mob_spawn_height(x, z, kind)
    }

    #[cfg(test)]
    pub(super) fn forget_final_heightmaps_for_test(&mut self, pos: ChunkPos) {
        self.terrain.as_mut().unwrap().final_heightmaps.remove(&pos);
    }

    pub fn snapshot(&self) -> Option<Arc<World>> {
        self.terrain.as_ref().map(CarvedTerrainWorld::snapshot)
    }

    /// Perform one ready operation. Structure stages are either explicitly
    /// disabled or verified against the frozen structure batch. A failed
    /// operation poisons the batch because search history may have advanced.
    pub fn step(
        &mut self,
        provider: &mut impl ResourceProvider,
        classify: &mut impl FnMut(&Value) -> Result<CarverBlockState>,
        resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
    ) -> Result<Option<(ChunkPos, GenerationStage)>> {
        self.step_inner(provider, classify, resolve, None, None)
    }

    /// Perform one operation with a caller-supplied independent minecart UUID
    /// constructor. Callback clones must isolate mutable state on failure.
    pub fn step_with_mineshaft_support<U>(
        &mut self,
        provider: &mut impl ResourceProvider,
        classify: &mut impl FnMut(&Value) -> Result<CarverBlockState>,
        resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
        uuid_factory: &mut U,
    ) -> Result<Option<(ChunkPos, GenerationStage)>>
    where
        U: Clone + FnMut([f64; 3]) -> Result<Option<[i32; 4]>>,
    {
        let mut runner = MineshaftFeatureCallbacks { uuid_factory };
        self.step_inner(provider, classify, resolve, None, Some(&mut runner))
    }

    /// Perform one operation with explicitly supplied ocean-ruin entity and
    /// item effects. The per-chunk region RNG is initialized by this batch.
    /// Combined hut and ocean decoration remains unsupported.
    pub fn step_with_ocean_support<C, W>(
        &mut self,
        provider: &mut impl ResourceProvider,
        classify: &mut impl FnMut(&Value) -> Result<CarverBlockState>,
        resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
        ocean: &mut super::OceanRuinPlacementContext<'_, C, W>,
    ) -> Result<Option<(ChunkPos, GenerationStage)>>
    where
        C: Clone + FnMut() -> Result<Option<super::FreshCreatureRandom>>,
        W: super::ocean_ruin::DrownedMarkerContext,
    {
        ensure!(
            ocean.sea_level == self.generator.noise.sea_level(),
            "ocean ruin sea level differs from generator"
        );
        let mut runner = OceanFeatureCallbacks { ocean };
        self.step_inner(provider, classify, resolve, None, Some(&mut runner))
    }

    /// Perform one operation with hut and ocean entity effects sharing the
    /// FEATURES chunk's region RNG. Hut states come from the prepared batch;
    /// on success the context receives the states referenced by that pass.
    /// Callback clones must isolate mutable state until the pass commits.
    pub fn step_with_mixed_structure_support<C, W, H, P>(
        &mut self,
        provider: &mut impl ResourceProvider,
        classify: &mut impl FnMut(&Value) -> Result<CarverBlockState>,
        resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
        context: &mut super::MixedHutOceanPlacementContext<'_, C, W, H, P>,
    ) -> Result<Option<(ChunkPos, GenerationStage)>>
    where
        C: Clone + FnMut() -> Result<Option<super::FreshCreatureRandom>>,
        W: super::ocean_ruin::DrownedMarkerContext,
        H: Clone
            + FnMut(
                native_resources::structure_piece::swamp_hut::HutMob,
            ) -> Result<super::FreshCreatureRandom>,
        P: Clone
            + FnMut(
                &TerrainStartKey,
                [i32; 3],
                &World,
                &Value,
                native_core::beardifier::BoundingBox,
                &[super::structure_batch::StructurePieceRevision],
            ) -> Result<bool>,
    {
        ensure!(
            context.ocean.sea_level == self.generator.noise.sea_level(),
            "ocean ruin sea level differs from generator"
        );
        let mut runner = MixedFeatureCallbacks { context };
        self.step_inner(provider, classify, resolve, None, Some(&mut runner))
    }

    /// Evaluate hut cat selectors against the current generated biome,
    /// structure pieces and caller-supplied world-clock moon phase while
    /// sharing the chunk's region RNG with ocean ruin entity initialization.
    #[allow(clippy::too_many_arguments)]
    pub fn step_with_native_mixed_environment<C, W>(
        &mut self,
        provider: &mut impl ResourceProvider,
        classify: &mut impl FnMut(&Value) -> Result<CarverBlockState>,
        resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
        ocean: &mut super::OceanRuinPlacementContext<'_, C, W>,
        variants: &native_resources::cat_variants::CatSpawnVariants,
        conditions: &native_resources::cat_variants::CatWorldConditions,
        moon_phase_at: &dyn Fn([i32; 3], &super::BiomeNeighborhood) -> Result<u8>,
    ) -> Result<Option<(ChunkPos, GenerationStage)>>
    where
        C: Clone + FnMut() -> Result<Option<super::FreshCreatureRandom>>,
        W: super::ocean_ruin::DrownedMarkerContext,
    {
        ensure!(
            ocean.sea_level == self.generator.noise.sea_level(),
            "ocean ruin sea level differs from generator"
        );
        let mut runner = NativeMixedFeatureRunner {
            ocean,
            variants,
            conditions,
            moon_phase_at,
        };
        self.step_inner(provider, classify, resolve, None, Some(&mut runner))
    }

    /// Use the batch's current generated biomes for drowned and hut cat
    /// conditions. Item and passenger effects and live difficulty/holiday
    /// inputs remain explicit; they must belong to the same world and packs.
    #[allow(clippy::too_many_arguments)]
    pub fn step_with_drowned_environment<C, E, D, H>(
        &mut self,
        provider: &mut impl ResourceProvider,
        classify: &mut impl FnMut(&Value) -> Result<CarverBlockState>,
        resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
        inputs: &mut super::ocean_ruin::DrownedPlacementInputs<'_, C, E, D, H>,
        variants: &native_resources::cat_variants::CatSpawnVariants,
        conditions: &native_resources::cat_variants::CatWorldConditions,
        moon_phase_at: &dyn Fn([i32; 3], &super::BiomeNeighborhood) -> Result<u8>,
    ) -> Result<Option<(ChunkPos, GenerationStage)>>
    where
        C: Clone + FnMut() -> Result<Option<super::FreshCreatureRandom>>,
        E: super::ocean_ruin::DrownedSpawnEffects,
        D: Clone
            + FnMut(
                [i32; 3],
                &World,
                &super::BiomeNeighborhood,
            ) -> Result<super::ocean_ruin::GenerationDifficultyInputs>,
        H: Clone + FnMut() -> Result<bool>,
    {
        ensure!(
            inputs.sea_level == self.generator.noise.sea_level(),
            "ocean ruin sea level differs from generator"
        );
        let mut runner = NativeDrownedFeatureRunner {
            inputs,
            variants,
            conditions,
            moon_phase_at,
        };
        self.step_inner(provider, classify, resolve, None, Some(&mut runner))
    }

    /// Perform one operation with hut mob initialization and cat conditions.
    /// Callers must supply the active pack's variants and complete world
    /// context. The condition callback receives the current start key and
    /// its ground-aligned piece bounds, even before the batch commits them.
    /// Callback clones must not share externally mutable state.
    /// Generated structures and entities remain intermediate and unsaveable.
    #[allow(clippy::too_many_arguments)]
    pub fn step_with_hut_support<C, P>(
        &mut self,
        provider: &mut impl ResourceProvider,
        classify: &mut impl FnMut(&Value) -> Result<CarverBlockState>,
        resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
        variants: &native_resources::cat_variants::CatSpawnVariants,
        construct: &mut C,
        condition: &mut P,
    ) -> Result<Option<(ChunkPos, GenerationStage)>>
    where
        C: Clone
            + FnMut(
                native_resources::structure_piece::swamp_hut::HutMob,
            ) -> Result<super::FreshCreatureRandom>,
        P: Clone
            + FnMut(
                &crate::worldgen::structure_batch::TerrainStartKey,
                [i32; 3],
                &World,
                &Value,
                native_core::beardifier::BoundingBox,
                &[crate::worldgen::structure_batch::StructurePieceRevision],
            ) -> Result<bool>,
    {
        let mut callbacks = HutFeatureCallbacks {
            variants,
            construct,
            condition,
        };
        self.step_inner(provider, classify, resolve, Some(&mut callbacks), None)
    }

    /// Perform one operation with a moon phase known to be constant across
    /// the whole operation. For positional attributes, use
    /// `step_with_hut_environment` instead.
    pub fn step_with_native_hut_support(
        &mut self,
        provider: &mut impl ResourceProvider,
        classify: &mut impl FnMut(&Value) -> Result<CarverBlockState>,
        resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
        variants: &native_resources::cat_variants::CatSpawnVariants,
        conditions: &native_resources::cat_variants::CatWorldConditions,
        moon_phase: u8,
    ) -> Result<Option<(ChunkPos, GenerationStage)>> {
        ensure!(moon_phase < 8, "invalid moon phase");
        self.step_with_hut_environment(
            provider,
            classify,
            resolve,
            variants,
            conditions,
            &|_, _| Ok(moon_phase),
        )
    }

    /// Evaluate moon phase at the actual spawn position using the active
    /// dimension, biome attributes, timelines and world-clock state supplied
    /// by the caller. The callback runs only for moon-dependent selectors.
    /// Missing inputs must return an error, rather than a guessed phase.
    pub fn step_with_hut_environment(
        &mut self,
        provider: &mut impl ResourceProvider,
        classify: &mut impl FnMut(&Value) -> Result<CarverBlockState>,
        resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
        variants: &native_resources::cat_variants::CatSpawnVariants,
        conditions: &native_resources::cat_variants::CatWorldConditions,
        moon_phase_at: &dyn Fn([i32; 3], &super::BiomeNeighborhood) -> Result<u8>,
    ) -> Result<Option<(ChunkPos, GenerationStage)>> {
        let mut runner = NativeHutFeatureRunner {
            variants,
            conditions,
            moon_phase_at,
        };
        self.step_inner(provider, classify, resolve, Some(&mut runner), None)
    }

    /// Evaluate the active moon-phase environment against a snapshot of the
    /// actual world clocks and the generated biome at each cat spawn position.
    #[allow(clippy::too_many_arguments)]
    pub fn step_with_hut_clocks(
        &mut self,
        provider: &mut impl ResourceProvider,
        classify: &mut impl FnMut(&Value) -> Result<CarverBlockState>,
        resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
        variants: &native_resources::cat_variants::CatSpawnVariants,
        conditions: &native_resources::cat_variants::CatWorldConditions,
        moon: &native_resources::moon_phase::Environment,
        clocks: &BTreeMap<ResourceId, i64>,
    ) -> Result<Option<(ChunkPos, GenerationStage)>> {
        self.step_with_hut_environment(
            provider,
            classify,
            resolve,
            variants,
            conditions,
            &|pos, biomes| {
                let biome = if moon.has_biome_layer {
                    let id = biomes.biome_at_block(pos[0], pos[1], pos[2])?;
                    Some(format!("{}:{}", id.namespace, id.path))
                } else {
                    None
                };
                let phase = moon
                    .sample(biome.as_deref(), |id| clocks.get(id).copied())
                    .context("moon phase environment or world clock unavailable")?;
                Ok(phase.index() as u8)
            },
        )
    }

    fn step_inner(
        &mut self,
        provider: &mut impl ResourceProvider,
        classify: &mut impl FnMut(&Value) -> Result<CarverBlockState>,
        resolve: &mut impl FnMut(&Value) -> Result<BlockState>,
        mut hut_runner: Option<&mut dyn HutFeatureRunner>,
        mut ocean_runner: Option<&mut dyn OceanFeatureRunner>,
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
            GenerationStage::StructureStarts | GenerationStage::StructureReferences => (|| {
                if let Some(structures) = &self.structures {
                    let exists = if stage == GenerationStage::StructureStarts {
                        structures.starts.contains_key(&pos)
                    } else {
                        structures.references.contains_key(&pos)
                    };
                    ensure!(exists, "prepared structure stage is missing for {pos:?}");
                }
                self.schedule.complete(pos, stage)
            })(
            ),
            GenerationStage::Biomes => self.schedule.run_biomes(
                &mut self.generator,
                &mut self.evaluator,
                &mut self.biomes,
                pos,
            ),
            GenerationStage::Terrain => (|| {
                self.check_write_area(pos, 0)?;
                if let Some(structures) = &self.structures {
                    let context = structures
                        .terrain
                        .get(&pos)
                        .cloned()
                        .context("prepared structure terrain order is missing")?;
                    self.schedule.run_terrain_with_prepared_structure(
                        &mut self.generator,
                        provider,
                        &mut self.evaluator,
                        &mut self.carvers,
                        &self.biomes,
                        &self.tags,
                        &mut self.terrain,
                        pos,
                        &self.dimension,
                        context,
                        classify,
                        resolve,
                    )?;
                } else {
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
                }
                if let (Some(config), Some(terrain)) = (&self.decoration, &mut self.terrain) {
                    terrain.initialize_worldgen_heightmaps(pos, &config.heightmap_tags)?;
                    if terrain.world.dimension_type != config.dimension_type
                        || terrain.world.inline_dimension_type != config.inline_dimension_type
                    {
                        let world = Arc::make_mut(&mut terrain.world);
                        world.dimension_type = config.dimension_type.clone();
                        world.inline_dimension_type = config.inline_dimension_type.clone();
                    }
                }
                self.lit_cache = None;
                Ok(())
            })(),
            GenerationStage::Features => (|| {
                self.check_write_area(pos, 0)?;
                let mut region_random = self.generator.region_random(pos);
                let config = self
                    .decoration
                    .as_ref()
                    .context("decoration is not configured")?;
                let plan = self.generator.decoration_plan(
                    &config.schedule,
                    &self.biomes,
                    [pos.x, pos.z],
                )?;
                let entity_free = plan.features().iter().all(|feature| {
                    config.feature_entity_evidence.get(feature.feature) == Some(&true)
                });
                let needs_mineshaft = self
                    .structures
                    .as_ref()
                    .map(|structures| structures.has_mineshaft_reference(pos))
                    .transpose()?
                    .unwrap_or(false);
                let prepared = PreparedDecoration::load_with_mineshaft(
                    provider,
                    &plan,
                    &config.block_registry,
                    resolve,
                    needs_mineshaft,
                )?;
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
                if let Some(structures) = &mut self.structures {
                    let registry = config
                        .structures
                        .as_ref()
                        .context("structure decoration registry is missing")?;
                    let mixed = if ocean_runner.is_some() {
                        Some(structures.mixed_decoration_plan(pos, registry)?)
                    } else {
                        None
                    };
                    let allow_huts = ocean_runner
                        .as_ref()
                        .is_some_and(|runner| runner.supports_huts());
                    let has_mixed = mixed.as_ref().is_some_and(|plan| {
                        plan.iter().any(|entry| {
                            entry.starts.iter().any(|start| {
                                match start {
                                    super::structure_batch::PlannedStructureStart::OceanRuin(_) => true,
                                    super::structure_batch::PlannedStructureStart::Mineshaft(_) => true,
                                    super::structure_batch::PlannedStructureStart::Simple(start) => allow_huts && start.kind == native_resources::structure_start::SimpleStructureKind::SwampHut,
                                }
                            })
                        })
                    });
                    if has_mixed {
                        let planned = mixed.context("mixed structure plan is missing")?;
                        let keys = structures.mixed_commit_keys(&planned, allow_huts)?;
                        let mut hut_states: BTreeMap<_, _> = keys
                            .iter()
                            .flatten()
                            .filter_map(|key| {
                                structures.huts.get(key).map(|state| (key.clone(), *state))
                            })
                            .collect();
                        let runner = ocean_runner
                            .as_deref_mut()
                            .context("ocean ruin context is missing")?;
                        let mut run = |lighting| {
                            self.schedule.run_features_with_ocean(
                                terrain,
                                &prepared,
                                &planned,
                                &mut hut_states,
                                &region_random,
                                lighting,
                                runner,
                                structures,
                                &self.biomes,
                            )
                        };
                        let updated = if let Some(engine) = &mut self.light_engine {
                            let registry = self
                                .light_registry
                                .as_ref()
                                .context("light registry is missing")?;
                            run(StructureFeatureLighting::Staged(engine, registry))?
                        } else if let Some(tracker) = &mut self.light_tracker {
                            let registry = self
                                .light_registry
                                .as_ref()
                                .context("light registry is missing")?;
                            run(StructureFeatureLighting::Fresh(tracker, registry))?
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
                            let light = VisibleLightSnapshot::fresh_after_section_updates(
                                true,
                                config.has_skylight,
                                [],
                                covered,
                            )
                            .context("invalid initial light columns")?;
                            run(StructureFeatureLighting::Visible(&light))?
                        };
                        drop(planned);
                        let huts_recorded = !hut_states.is_empty();
                        structures.commit_mixed_starts(keys, updated, hut_states);
                        if huts_recorded {
                            self.entity_sources.recorded_hut_features.insert(pos);
                        }
                    } else {
                        let planned = structures.decoration_plan(pos, registry)?;
                        let keys: Vec<Vec<_>> = planned
                            .iter()
                            .map(|entry| {
                                entry
                                    .starts
                                    .iter()
                                    .map(|start| {
                                        (
                                            entry.entry.id.clone(),
                                            ChunkPos {
                                                x: start.chunk[0],
                                                z: start.chunk[1],
                                            },
                                        )
                                    })
                                    .collect()
                            })
                            .collect();
                        // Only huts referenced by this decoration pass can change.
                        let mut hut_states = hut_runner.as_ref().map(|_| {
                            keys.iter()
                                .flatten()
                                .filter_map(|key| {
                                    structures.huts.get(key).map(|state| (key.clone(), *state))
                                })
                                .collect::<BTreeMap<_, _>>()
                        });
                        let mut run = |lighting| {
                            if let (Some(runner), Some(states)) =
                                (hut_runner.as_deref_mut(), hut_states.as_mut())
                            {
                                self.schedule.run_features_with_huts(
                                    terrain,
                                    &prepared,
                                    &planned,
                                    states,
                                    &mut region_random,
                                    lighting,
                                    runner,
                                    structures,
                                    &self.biomes,
                                )
                            } else {
                                self.schedule.run_features_with_structures(
                                    terrain, &prepared, &planned, lighting,
                                )
                            }
                        };
                        let updated = if let Some(engine) = &mut self.light_engine {
                            let registry = self
                                .light_registry
                                .as_ref()
                                .context("light registry is missing")?;
                            run(StructureFeatureLighting::Staged(engine, registry))?
                        } else if let Some(tracker) = &mut self.light_tracker {
                            let registry = self
                                .light_registry
                                .as_ref()
                                .context("light registry is missing")?;
                            run(StructureFeatureLighting::Fresh(tracker, registry))?
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
                            let light = VisibleLightSnapshot::fresh_after_section_updates(
                                true,
                                config.has_skylight,
                                [],
                                covered,
                            )
                            .context("invalid initial light columns")?;
                            run(StructureFeatureLighting::Visible(&light))?
                        };
                        drop(planned);
                        structures.commit_decorated_starts(keys, updated, hut_states.as_ref())?;
                        if hut_states.as_ref().is_some_and(|states| !states.is_empty()) {
                            self.entity_sources.recorded_hut_features.insert(pos);
                        }
                    }
                } else if let Some(engine) = &mut self.light_engine {
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
                self.entity_sources.complete_feature_pass(pos, entity_free);
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
            GenerationStage::Spawn => (|| {
                ensure!(
                    self.spawn_ready.contains(&pos),
                    "world-generation mob spawning is incomplete"
                );
                self.schedule.complete(pos, stage)?;
                self.spawn_ready.remove(&pos);
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
    /// preserve it before later light propagation or decoration. Structure-aware
    /// batches cannot be transferred until structure persistence is supported.
    pub fn into_parts(
        self,
    ) -> Result<(
        GenerationSchedule,
        BiomeNeighborhood,
        Option<CarvedTerrainWorld>,
        Option<(FreshBatchLight, GeneratedBlockRegistry)>,
    )> {
        ensure!(self.is_complete(), "terrain batch is incomplete or failed");
        ensure!(
            self.structures.is_none(),
            "structure data cannot yet be transferred for saving"
        );
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

#[cfg(test)]
mod region_draft_tests {
    use super::*;
    use native_persistence::{
        nbt::{NamedTag, Tag, TagType},
        save::FreshRegionKind,
    };

    fn chunk(pos: ChunkPos, entities: Option<Vec<Tag>>) -> PostprocessedChunkDraft {
        let entity_chunk = entities.as_ref().map(|records| {
            native_persistence::generated_chunk::assemble_entity_chunk(pos, records).unwrap()
        });
        PostprocessedChunkDraft {
            chunk: NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![
                    ("DataVersion".into(), Tag::Int(5023)),
                    ("xPos".into(), Tag::Int(pos.x)),
                    ("zPos".into(), Tag::Int(pos.z)),
                    ("Status".into(), Tag::String("minecraft:full".into())),
                    (
                        "sections".into(),
                        Tag::List {
                            element_type: TagType::End,
                            values: vec![],
                        },
                    ),
                ]),
            },
            entity_records: entities.map(Arc::new),
            entity_chunk,
        }
    }

    #[test]
    fn structure_preview_requires_every_referenced_start_in_the_selection() {
        let a = ChunkPos { x: -1, z: 0 };
        let b = ChunkPos { x: 0, z: 0 };
        let c = ChunkPos { x: 1, z: 0 };
        let refs = |pos| {
            Ok(match pos {
                p if p == a => BTreeSet::from([a, b]),
                p if p == b => BTreeSet::from([c]),
                _ => BTreeSet::new(),
            })
        };
        assert!(require_selected_structure_sources(&BTreeSet::new(), refs).is_err());
        assert!(require_selected_structure_sources(&BTreeSet::from([a]), refs).is_err());
        assert!(require_selected_structure_sources(&BTreeSet::from([a, b]), refs).is_err());
        assert_eq!(
            require_selected_structure_sources(&BTreeSet::from([a, b, c]), refs).unwrap(),
            BTreeSet::from([a, b, c])
        );
        assert!(
            require_selected_structure_sources(&BTreeSet::from([a]), |_| anyhow::bail!(
                "incomplete starts"
            ))
            .is_err()
        );
    }

    #[test]
    fn attaching_structure_data_preserves_the_chunk_and_rejects_invalid_drafts() {
        let structures = Tag::Compound(vec![
            (
                "starts".into(),
                Tag::Compound(vec![(
                    "example:treasure".into(),
                    Tag::Compound(vec![
                        ("id".into(), Tag::String("example:treasure".into())),
                        ("ChunkX".into(), Tag::Int(-1)),
                    ]),
                )]),
            ),
            (
                "References".into(),
                Tag::Compound(vec![(
                    "example:treasure".into(),
                    Tag::LongArray(vec![-1, 0]),
                )]),
            ),
        ]);
        let empty = Tag::Compound(vec![
            ("starts".into(), Tag::Compound(vec![])),
            ("References".into(), Tag::Compound(vec![])),
        ]);
        for status in ["minecraft:spawn", "minecraft:full"] {
            let draft = NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![
                    ("Status".into(), Tag::String(status.into())),
                    ("structures".into(), empty.clone()),
                    ("xPos".into(), Tag::Int(-1)),
                ]),
            };
            let result = attach_generated_structure_data(draft, structures.clone()).unwrap();
            assert_eq!(result.value.get("structures"), Some(&structures));
            assert_eq!(result.value.get("xPos"), Some(&Tag::Int(-1)));
            assert_eq!(
                result.value.get("Status"),
                Some(&Tag::String(status.into()))
            );
            let bytes = native_persistence::nbt::write(&result).unwrap();
            assert_eq!(native_persistence::nbt::read(&bytes).unwrap(), result);
        }
        let missing = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![]),
        };
        assert!(attach_generated_structure_data(missing, structures.clone()).is_err());
        let duplicate = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("structures".into(), empty.clone()),
                ("structures".into(), empty.clone()),
            ]),
        };
        assert!(attach_generated_structure_data(duplicate, structures).is_err());
        let malformed = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![("structures".into(), empty)]),
        };
        assert!(attach_generated_structure_data(malformed, Tag::Int(0)).is_err());
    }

    #[test]
    fn generated_region_grouping_requires_complete_coverage_and_keeps_storage_separate() {
        let west = ChunkPos { x: -1, z: 0 };
        let east = ChunkPos { x: 0, z: 0 };
        let poi = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("Sections".into(), Tag::Compound(vec![])),
            ]),
        };
        let item = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:item".into())),
            ("UUID".into(), Tag::IntArray(vec![1, 2, 3, 4])),
        ]);
        let chunks = || {
            BTreeMap::from([
                (west, chunk(west, Some(vec![]))),
                (east, chunk(east, Some(vec![item.clone()]))),
            ])
        };
        assert!(group_fresh_regions(chunks(), BTreeMap::from([(east, None)]), 7).is_err());
        assert!(
            group_fresh_regions(
                BTreeMap::from([(west, chunk(west, None))]),
                BTreeMap::from([(west, None)]),
                7
            )
            .is_err()
        );
        let regions = group_fresh_regions(
            chunks(),
            BTreeMap::from([(west, None), (east, Some(poi))]),
            7,
        )
        .unwrap();
        assert_eq!(regions.len(), 4);
        for key in [
            (FreshRegionKind::Terrain, -1, 0),
            (FreshRegionKind::Terrain, 0, 0),
            (FreshRegionKind::Entities, 0, 0),
            (FreshRegionKind::Poi, 0, 0),
        ] {
            let draft = regions.get(&key).unwrap();
            assert_eq!(draft.chunk_count(), 1);
            let mut stage = tempfile::tempfile().unwrap();
            assert!(draft.write_staged(&mut stage).unwrap().is_empty());
        }
        assert!(!regions.contains_key(&(FreshRegionKind::Entities, -1, 0)));
    }
}

#[cfg(test)]
mod entity_source_tests {
    use super::*;
    use crate::worldgen::{GeneratedEntityAddition, GeneratedEntityRecord};
    use serde_json::json;

    #[derive(Default)]
    struct Data(BTreeMap<String, Vec<u8>>);

    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.0.get(path).cloned())
        }
    }

    #[test]
    fn animal_structure_conditions_use_referenced_piece_bounds() {
        use crate::worldgen::structure_references::{
            StructureStartBounds, collect_ordered_structure_references,
        };
        use native_core::beardifier::{BoundingBox, TerrainAdjustment};
        use native_resources::structure_start::{SimpleStructureKind, SimpleStructureStart};
        let source = ChunkPos { x: -1, z: -1 };
        let id = ResourceId::parse("minecraft:swamp_hut").unwrap();
        let start = SimpleStructureStart {
            kind: SimpleStructureKind::SwampHut,
            chunk: [source.x, source.z],
            generation_point: [-8, 64, -8],
            orientation: None,
            bounds: BoundingBox::new([-10, 63, -10], [-5, 68, -5]).unwrap(),
            terrain_adjustment: TerrainAdjustment::BeardThin,
        };
        let bounds = StructureStartBounds::from_simple(id.clone(), &start).unwrap();
        let mut stages = PreparedStructureStages::default();
        stages
            .starts
            .insert(source, BTreeMap::from([(id.clone(), start)]));
        let references = collect_ordered_structure_references(source, |pos| {
            Ok(if pos == source {
                vec![bounds.clone()]
            } else {
                vec![]
            })
        })
        .unwrap();
        stages.references.insert(source, references);
        assert_eq!(
            structures_with_piece_at(Some(&stages), [-10, 63, -10]).unwrap(),
            BTreeSet::from([id])
        );
        assert!(
            structures_with_piece_at(Some(&stages), [-11, 63, -10])
                .unwrap()
                .is_empty()
        );
        assert!(
            structures_with_piece_at(None, [-10, 63, -10])
                .unwrap()
                .is_empty()
        );
        assert!(structures_with_piece_at(Some(&stages), [16, 63, 16]).is_err());
        stages.starts.clear();
        assert!(structures_with_piece_at(Some(&stages), [-10, 63, -10]).is_err());
    }

    #[test]
    fn animal_structure_conditions_use_individual_mineshaft_pieces() {
        use crate::worldgen::structure_references::collect_ordered_structure_references;
        use native_core::beardifier::{BoundingBox, TerrainAdjustment};
        use native_resources::mineshaft::{
            MineshaftGraphPiece, MineshaftGraphPieceKind, MineshaftVariant,
        };
        use native_resources::structure_start::HorizontalDirection;
        let source = ChunkPos { x: 0, z: 0 };
        let id = ResourceId::parse("minecraft:mineshaft").unwrap();
        let room = MineshaftGraphPiece {
            bounds: BoundingBox::new([2, 1, 2], [10, 6, 10]).unwrap(),
            direction: None,
            depth: 0,
            kind: MineshaftGraphPieceKind::Room { entrances: vec![] },
        };
        let corridor = MineshaftGraphPiece {
            bounds: BoundingBox::new([12, 2, 2], [14, 4, 6]).unwrap(),
            direction: Some(HorizontalDirection::South),
            depth: 1,
            kind: MineshaftGraphPieceKind::Corridor {
                rails: false,
                spiders: false,
                sections: 1,
            },
        };
        let start = crate::worldgen::mineshaft::PreparedStart {
            chunk: [0, 0],
            generation_point: [8, 1, 0],
            terrain_adjustment: TerrainAdjustment::None,
            variant: MineshaftVariant::Normal,
            pieces: [room, corridor]
                .iter()
                .map(|piece| {
                    crate::worldgen::SavedMineshaftPiece::capture(
                        piece,
                        MineshaftVariant::Normal,
                        false,
                    )
                    .unwrap()
                })
                .collect(),
        };
        let bounds = start.reference_bounds(id.clone()).unwrap();
        let references = collect_ordered_structure_references(source, |pos| {
            Ok(if pos == source {
                vec![bounds.clone()]
            } else {
                vec![]
            })
        })
        .unwrap();
        let mut stages = PreparedStructureStages::default();
        stages.starts.insert(source, BTreeMap::new());
        stages
            .mineshafts
            .insert(source, BTreeMap::from([(id.clone(), start)]));
        stages.references.insert(source, references);
        assert_eq!(
            structures_with_piece_at(Some(&stages), [13, 3, 3]).unwrap(),
            BTreeSet::from([id])
        );
        assert!(
            structures_with_piece_at(Some(&stages), [11, 3, 3])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn structure_entity_evidence_requires_complete_known_neighbor_decorations() {
        use crate::worldgen::structure_references::{
            StructureStartBounds, collect_ordered_structure_references,
        };
        use native_core::beardifier::{BoundingBox, TerrainAdjustment};
        use native_resources::structure_start::{SimpleStructureKind, SimpleStructureStart};
        struct Registry;
        impl ResourceProvider for Registry {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                Ok((path == "data/example/worldgen/structure/treasure.json")
                    .then(|| br#"{"step":"surface_structures"}"#.to_vec()))
            }
            fn list_data(&mut self, _: &str) -> Result<Vec<ResourceId>> {
                Ok(vec![ResourceId::parse("example:treasure")?])
            }
        }
        let registry = StructureDecorationSchedule::load(&mut Registry).unwrap();
        let center = ChunkPos { x: 0, z: 0 };
        let id = ResourceId::parse("example:treasure").unwrap();
        let start = SimpleStructureStart {
            kind: SimpleStructureKind::BuriedTreasure,
            chunk: [0, 0],
            generation_point: [8, 64, 8],
            orientation: None,
            bounds: BoundingBox::new([9, 64, 9], [9, 64, 9]).unwrap(),
            terrain_adjustment: TerrainAdjustment::None,
        };
        let bounds = StructureStartBounds::from_simple(id.clone(), &start).unwrap();
        let mut structures = PreparedStructureStages::default();
        structures
            .starts
            .insert(center, BTreeMap::from([(id.clone(), start)]));
        for z in -1..=1 {
            for x in -1..=1 {
                let source = ChunkPos { x, z };
                let refs = collect_ordered_structure_references(source, |pos| {
                    Ok(if pos == center {
                        vec![bounds.clone()]
                    } else {
                        vec![]
                    })
                })
                .unwrap();
                structures.references.insert(source, refs);
            }
        }
        assert!(!structures.references[&center].membership().is_empty());
        assert!(structure_entities_are_accounted_for(
            &structures,
            &registry,
            center,
            |_| true,
            &BTreeSet::new()
        ));
        assert!(!structure_entities_are_accounted_for(
            &structures,
            &registry,
            center,
            |pos| pos != ChunkPos { x: 1, z: 0 },
            &BTreeSet::new()
        ));
        structures.references.remove(&ChunkPos { x: -1, z: 0 });
        assert!(!structure_entities_are_accounted_for(
            &structures,
            &registry,
            center,
            |_| true,
            &BTreeSet::new()
        ));
        let refs = collect_ordered_structure_references(ChunkPos { x: -1, z: 0 }, |pos| {
            Ok(if pos == center {
                vec![bounds.clone()]
            } else {
                vec![]
            })
        })
        .unwrap();
        structures.references.insert(ChunkPos { x: -1, z: 0 }, refs);
        structures
            .starts
            .get_mut(&center)
            .unwrap()
            .get_mut(&id)
            .unwrap()
            .kind = SimpleStructureKind::SwampHut;
        assert!(!structure_entities_are_accounted_for(
            &structures,
            &registry,
            center,
            |_| true,
            &BTreeSet::new()
        ));
        assert!(!structure_entities_are_accounted_for(
            &structures,
            &registry,
            ChunkPos { x: i32::MAX, z: 0 },
            |_| true,
            &BTreeSet::new()
        ));
    }

    #[test]
    fn hut_entity_evidence_requires_recorded_feature_passes_and_consistent_state() {
        use crate::worldgen::structure_references::{
            StructureStartBounds, collect_ordered_structure_references,
        };
        use native_core::beardifier::{BoundingBox, TerrainAdjustment};
        use native_resources::structure_start::{
            HorizontalDirection, SimpleStructureKind, SimpleStructureStart,
        };
        struct Registry;
        impl ResourceProvider for Registry {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                Ok((path == "data/example/worldgen/structure/hut.json")
                    .then(|| br#"{"step":"surface_structures"}"#.to_vec()))
            }
            fn list_data(&mut self, _: &str) -> Result<Vec<ResourceId>> {
                Ok(vec![ResourceId::parse("example:hut")?])
            }
        }
        let registry = StructureDecorationSchedule::load(&mut Registry).unwrap();
        let center = ChunkPos { x: 0, z: 0 };
        let id = ResourceId::parse("example:hut").unwrap();
        let key = (id.clone(), center);
        let start = SimpleStructureStart {
            kind: SimpleStructureKind::SwampHut,
            chunk: [0, 0],
            generation_point: [8, 64, 8],
            orientation: Some(HorizontalDirection::South),
            bounds: BoundingBox::new([0, 64, 0], [6, 70, 8]).unwrap(),
            terrain_adjustment: TerrainAdjustment::None,
        };
        let bounds = StructureStartBounds::from_simple(id.clone(), &start).unwrap();
        let saved = super::super::SavedSwampHutPiece::capture(&start, Default::default()).unwrap();
        let mut structures = PreparedStructureStages::default();
        structures
            .starts
            .insert(center, BTreeMap::from([(id.clone(), start)]));
        structures.huts.insert(key.clone(), saved);
        let mut recorded = BTreeSet::new();
        for z in -1..=1 {
            for x in -1..=1 {
                let source = ChunkPos { x, z };
                structures.references.insert(
                    source,
                    collect_ordered_structure_references(source, |pos| {
                        Ok(if pos == center {
                            vec![bounds.clone()]
                        } else {
                            vec![]
                        })
                    })
                    .unwrap(),
                );
                recorded.insert(source);
            }
        }
        let covered = |structures: &PreparedStructureStages, recorded: &BTreeSet<ChunkPos>| {
            structure_entities_are_accounted_for(structures, &registry, center, |_| true, recorded)
        };
        assert!(covered(&structures, &recorded));
        assert!(!structure_entities_are_accounted_for(
            &structures,
            &registry,
            center,
            |pos| pos != center,
            &recorded
        ));
        recorded.remove(&center);
        assert!(!covered(&structures, &recorded));
        recorded.insert(center);
        structures.huts.remove(&key);
        assert!(!covered(&structures, &recorded));
        let mut wrong = saved;
        wrong.bounds.min[1] += 1;
        structures.huts.insert(key.clone(), wrong);
        assert!(!covered(&structures, &recorded));
        structures.huts.insert(key.clone(), saved);
        structures
            .starts
            .get_mut(&center)
            .unwrap()
            .get_mut(&id)
            .unwrap()
            .kind = SimpleStructureKind::DesertPyramid;
        assert!(!covered(&structures, &recorded));
    }

    #[test]
    fn feature_entity_evidence_uses_active_inline_and_registered_definitions() {
        let mut data = Data::default();
        let put = |data: &mut Data, path: &str, value: Value| {
            data.0
                .insert(path.into(), serde_json::to_vec(&value).unwrap());
        };
        put(
            &mut data,
            "data/test/worldgen/biome/plain.json",
            json!({"features":[[
                {"feature":{"type":"minecraft:simple_block"},"placement":[]}, "test:placed"
            ]]}),
        );
        put(
            &mut data,
            "data/test/worldgen/placed_feature/placed.json",
            json!({"feature":"test:configured","placement":[]}),
        );
        put(
            &mut data,
            "data/test/worldgen/feature/configured.json",
            json!({"type":"minecraft:ore"}),
        );
        let biome = ResourceId::parse("test:plain").unwrap();
        let placed = ResourceId::parse("test:placed").unwrap();
        let registry = BTreeSet::from([placed]);
        let schedule = FeatureSchedule::load(&mut data, &[biome], &registry).unwrap();
        assert!(features_cannot_spawn_entities(
            &mut data, &schedule, &registry
        ));
        put(
            &mut data,
            "data/test/worldgen/feature/configured.json",
            json!({"type":"minecraft:random_boolean_selector"}),
        );
        assert!(!features_cannot_spawn_entities(
            &mut data, &schedule, &registry
        ));
        put(
            &mut data,
            "data/test/worldgen/feature/configured.json",
            json!({"type":"test:ore"}),
        );
        assert!(!features_cannot_spawn_entities(
            &mut data, &schedule, &registry
        ));
        data.0.remove("data/test/worldgen/feature/configured.json");
        assert!(!features_cannot_spawn_entities(
            &mut data, &schedule, &registry
        ));
    }

    #[test]
    fn entity_coverage_tracks_only_completed_selected_features() {
        let mut data = Data::default();
        for (path, value) in [
            (
                "data/test/worldgen/biome/plain.json",
                json!({"features":[["test:safe"]]}),
            ),
            (
                "data/test/worldgen/biome/other.json",
                json!({"features":[["test:unknown"]]}),
            ),
            (
                "data/test/worldgen/placed_feature/safe.json",
                json!({"feature":{"type":"minecraft:ore"},"placement":[]}),
            ),
            (
                "data/test/worldgen/placed_feature/unknown.json",
                json!({"feature":{"type":"example:spawner"},"placement":[]}),
            ),
        ] {
            data.0
                .insert(path.into(), serde_json::to_vec(&value).unwrap());
        }
        let plain = ResourceId::parse("test:plain").unwrap();
        let other = ResourceId::parse("test:other").unwrap();
        let registry = BTreeSet::from([
            ResourceId::parse("test:safe").unwrap(),
            ResourceId::parse("test:unknown").unwrap(),
        ]);
        let schedule =
            FeatureSchedule::load(&mut data, &[plain.clone(), other.clone()], &registry).unwrap();
        let facts = feature_entity_evidence(&mut data, &schedule, &registry);
        let selected_safe = schedule.select([&plain]);
        let selected_unknown = schedule.select([&other]);
        assert!(
            selected_safe
                .iter()
                .all(|feature| facts.get(feature.feature) == Some(&true))
        );
        assert!(
            !selected_unknown
                .iter()
                .all(|feature| facts.get(feature.feature) == Some(&true))
        );
        let center = ChunkPos { x: 0, z: 0 };
        let mut sources = GeneratedEntitySources::default();
        sources.complete_spawn(center, &[]);
        assert!(!sources.complete_entity_coverage(center, true));
        sources.complete_feature_pass(
            center,
            selected_safe
                .iter()
                .all(|feature| facts.get(feature.feature) == Some(&true)),
        );
        assert!(sources.complete_entity_coverage(center, true));
        sources.complete_feature_pass(
            ChunkPos { x: 1, z: 0 },
            selected_unknown
                .iter()
                .all(|feature| facts.get(feature.feature) == Some(&true)),
        );
        assert!(!sources.complete_entity_coverage(center, true));
    }

    #[test]
    fn nested_selectors_require_every_branch_and_resolve_tags() {
        let mut data = Data::default();
        let put = |data: &mut Data, path: &str, value: Value| {
            data.0
                .insert(path.into(), serde_json::to_vec(&value).unwrap());
        };
        put(
            &mut data,
            "data/test/worldgen/biome/plain.json",
            json!({"features":[["test:root"]]}),
        );
        put(
            &mut data,
            "data/test/worldgen/placed_feature/root.json",
            json!({"feature":"test:root","placement":[]}),
        );
        put(
            &mut data,
            "data/test/worldgen/placed_feature/leaf.json",
            json!({"feature":{"type":"minecraft:ore"},"placement":[]}),
        );
        put(
            &mut data,
            "data/test/tags/worldgen/placed_feature/plants.json",
            json!({"values":["test:leaf"]}),
        );
        let registry = BTreeSet::from([
            ResourceId::parse("test:root").unwrap(),
            ResourceId::parse("test:leaf").unwrap(),
        ]);
        let schedule = FeatureSchedule::load(
            &mut data,
            &[ResourceId::parse("test:plain").unwrap()],
            &registry,
        )
        .unwrap();
        let inline = json!({"feature":{"type":"minecraft:simple_block"},"placement":[]});
        for configured in [
            json!({"type":"minecraft:random_boolean_selector","feature_true":inline,"feature_false":"test:leaf"}),
            json!({"type":"minecraft:simple_random_selector","features":"#test:plants"}),
            json!({"type":"minecraft:sequence","features":[inline,"test:leaf"]}),
            json!({"type":"minecraft:sequence","features":"#test:plants"}),
            json!({"type":"minecraft:random_selector","features":[{"chance":0.5,"feature":"test:leaf"}],"default":inline}),
            json!({"type":"minecraft:weighted_random_selector","features":[{"weight":1,"data":"test:leaf"}]}),
            json!({"type":"minecraft:vegetation_patch","vegetation_feature":inline}),
            json!({"type":"minecraft:root_system","feature":inline}),
        ] {
            put(
                &mut data,
                "data/test/worldgen/feature/root.json",
                configured,
            );
            assert!(features_cannot_spawn_entities(
                &mut data, &schedule, &registry
            ));
        }
        for configured in [
            json!({"type":"minecraft:random_boolean_selector","feature_true":inline,"feature_false":{"feature":{"type":"example:spawner"},"placement":[]}}),
            json!({"type":"minecraft:random_boolean_selector","feature_true":inline,"feature_false":"test:root"}),
            json!({"type":"minecraft:simple_random_selector","features":"#test:missing"}),
            json!({"type":"minecraft:sequence","features":[]}),
            json!({"type":"minecraft:sequence","features":[inline,{"feature":{"type":"example:spawner"},"placement":[]}]}),
            json!({"type":"minecraft:sequence","features":["test:root"]}),
            json!({"type":"minecraft:random_selector","features":[{"chance":2,"feature":"test:leaf"}],"default":inline}),
            json!({"type":"minecraft:weighted_random_selector","features":[{"weight":-1,"data":"test:leaf"}]}),
        ] {
            put(
                &mut data,
                "data/test/worldgen/feature/root.json",
                configured,
            );
            assert!(!features_cannot_spawn_entities(
                &mut data, &schedule, &registry
            ));
        }
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn installed_sequence_features_are_classified_conservatively() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let registry = resources
            .list_data("worldgen/placed_feature")
            .unwrap()
            .into_iter()
            .collect();
        let mut classifier = EntityFeatureClassifier {
            provider: &mut resources,
            registry: &registry,
            tags: FeatureTags::default(),
            active: BTreeSet::new(),
            cache: BTreeMap::new(),
            nodes: 0,
        };
        for name in ["minecraft:sulfur_pool"] {
            assert!(
                classifier
                    .named(&ResourceId::parse(name).unwrap(), true, 0)
                    .unwrap(),
                "{name}"
            );
        }
        for name in [
            "minecraft:rooted_sulfur_spring",
            "minecraft:sculk_patch_deep_dark",
            "minecraft:desert_well",
        ] {
            assert!(
                !classifier
                    .named(&ResourceId::parse(name).unwrap(), true, 0)
                    .unwrap_or(false),
                "{name}"
            );
        }
    }

    #[test]
    fn tree_entity_evidence_requires_supported_components() {
        let tree = json!({
            "type":"minecraft:tree",
            "trunk_placer":{"type":"minecraft:straight_trunk_placer"},
            "foliage_placer":{"type":"minecraft:blob_foliage_placer"},
            "decorators":[{"type":"minecraft:beehive"}, {"type":"minecraft:place_on_ground"}]
        });
        assert!(known_entity_free_tree(&tree, false).unwrap());
        for (field, replacement) in [
            ("trunk_placer", json!({"type":"example:trunk"})),
            (
                "foliage_placer",
                json!({"type":"minecraft:unknown_foliage"}),
            ),
            ("decorators", json!([{"type":"example:creature_decorator"}])),
            (
                "root_placer",
                json!({"type":"minecraft:mangrove_root_placer"}),
            ),
        ] {
            let mut unknown = tree.clone();
            unknown[field] = replacement;
            assert!(!known_entity_free_tree(&unknown, false).unwrap());
        }
        let fallen = json!({"stump_decorators":[{"type":"minecraft:trunk_vine"}],
            "log_decorators":[{"type":"minecraft:attached_to_logs"}]});
        assert!(known_entity_free_tree(&fallen, true).unwrap());
        let mut unknown = fallen.clone();
        unknown["log_decorators"] = json!([{"type":"example:unknown"}]);
        assert!(!known_entity_free_tree(&unknown, true).unwrap());
        let mut missing = fallen;
        missing.as_object_mut().unwrap().remove("stump_decorators");
        assert!(known_entity_free_tree(&missing, true).is_err());
    }

    #[test]
    fn cross_chunk_and_untracked_sources_invalidate_spawn_evidence() {
        let chunk = ChunkPos { x: -1, z: 0 };
        let local = GeneratedEntityAddition {
            block_position: [-1, 64, 15],
            record: GeneratedEntityRecord::Passenger,
        };
        let neighbor = GeneratedEntityAddition {
            block_position: [0, 64, 15],
            record: GeneratedEntityRecord::Passenger,
        };
        let mut evidence = GeneratedEntitySources::default();
        assert!(!evidence.contains_complete_spawn(chunk));
        evidence.complete_spawn(chunk, &[local]);
        assert!(evidence.contains_complete_spawn(chunk));
        assert!(!evidence.complete_entity_coverage(chunk, true));
        evidence.complete_feature_pass(chunk, true);
        assert!(evidence.complete_entity_coverage(chunk, true));
        assert!(!evidence.complete_entity_coverage(chunk, false));
        let mut unknown_feature = GeneratedEntitySources::default();
        unknown_feature.complete_spawn(chunk, &[]);
        unknown_feature.complete_feature_pass(chunk, true);
        assert!(unknown_feature.complete_entity_coverage(chunk, true));
        unknown_feature.complete_feature_pass(ChunkPos { x: 2, z: 0 }, false);
        assert!(!unknown_feature.complete_entity_coverage(chunk, true));
        evidence.complete_spawn(ChunkPos { x: 1, z: 0 }, &[neighbor]);
        assert!(!evidence.contains_complete_spawn(chunk));
        evidence.complete_spawn(chunk, &[]);
        assert!(!evidence.contains_complete_spawn(chunk));

        let mut external = GeneratedEntitySources::default();
        external.complete_spawn(chunk, &[]);
        external.untracked = true;
        assert!(!external.contains_complete_spawn(chunk));
    }
}

#[cfg(test)]
mod expansion_checkpoint_tests {
    use super::*;
    use native_world::loaded::{BIOME_COUNT, BLOCK_COUNT, BlockState, Chunk, Section};

    #[test]
    fn snapshot_guard_allows_new_chunks_but_rejects_changes_to_existing_data() {
        let old_pos = ChunkPos { x: 0, z: 0 };
        let new_pos = ChunkPos { x: 4, z: -1 };
        let mut before = World::new("minecraft:overworld");
        let stone = before.intern_state(BlockState {
            name: "minecraft:stone".into(),
            properties: Default::default(),
        });
        let plains = before.intern_biome("minecraft:plains".into());
        before.chunks.insert(
            old_pos,
            Arc::new(Chunk {
                position: old_pos,
                status: Some("minecraft:terrain".into()),
                block_light_correct: false,
                sections: BTreeMap::from([(
                    0,
                    Arc::new(Section {
                        blocks: Some(Box::new([stone; BLOCK_COUNT])),
                        biomes: Some(Box::new([plains; BIOME_COUNT])),
                        block_light: None,
                        sky_light: None,
                    }),
                )]),
            }),
        );
        let halo_pos = ChunkPos { x: -2, z: 0 };
        let mut halo = (*before.chunks[&old_pos]).clone();
        halo.position = halo_pos;
        before.chunks.insert(halo_pos, Arc::new(halo));
        let mut expanded = before.clone();
        let air = expanded.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: Default::default(),
        });
        expanded.intern_biome("minecraft:desert".into());
        let mut added = (*expanded.chunks[&old_pos]).clone();
        added.position = new_pos;
        expanded.chunks.insert(new_pos, Arc::new(added));
        assert!(unchanged_generation_world(&before, &expanded));
        let observed = BTreeSet::from([old_pos]);
        assert_eq!(
            postprocessing_observed_chunks(&observed, &BTreeSet::from([old_pos]), &[]),
            Some(observed.clone())
        );
        assert_eq!(
            postprocessing_observed_chunks(&observed, &BTreeSet::from([halo_pos]), &[]),
            None
        );
        let halo_edit = native_world::loaded::BlockPosition { x: -32, y: 0, z: 0 };
        assert_eq!(
            postprocessing_observed_chunks(&observed, &BTreeSet::new(), &[halo_edit]),
            Some(BTreeSet::from([old_pos, halo_pos]))
        );
        let mut changed_halo = expanded.clone();
        changed_halo.set_block_id(-32, 0, 0, air).unwrap();
        assert!(unchanged_generation_world_for(
            &before,
            &changed_halo,
            &observed
        ));
        assert!(!unchanged_generation_world(&before, &changed_halo));
        assert!(!unchanged_generation_world_for(
            &before,
            &changed_halo,
            &BTreeSet::from([old_pos, halo_pos])
        ));
        assert!(!unchanged_generation_world_for(
            &before,
            &changed_halo,
            &BTreeSet::from([new_pos])
        ));
        let mut processed = before.clone();
        let chest = processed.intern_state(BlockState {
            name: "minecraft:chest".into(),
            properties: Default::default(),
        });
        assert_eq!(chest, air, "fixture must use conflicting forked state IDs");
        processed.set_block_id(0, 0, 0, chest).unwrap();
        let current = Arc::new(expanded.clone());
        let unchanged = replay_postprocessing_edits(&current, &processed, &[]).unwrap();
        assert!(Arc::ptr_eq(&current, &unchanged));
        let at = native_world::loaded::BlockPosition { x: 0, y: 0, z: 0 };
        let rebased = replay_postprocessing_edits(&current, &processed, &[at]).unwrap();
        assert_eq!(rebased.block(0, 0, 0).unwrap().name, "minecraft:chest");
        assert_eq!(current.block(0, 0, 0).unwrap().name, "minecraft:stone");
        assert!(
            replay_postprocessing_edits(
                &current,
                &processed,
                &[native_world::loaded::BlockPosition { x: 999, y: 0, z: 0 }]
            )
            .is_err()
        );
        assert_eq!(current.block(0, 0, 0).unwrap().name, "minecraft:stone");
        let mut changed = expanded.clone();
        changed.set_block_id(0, 0, 0, air).unwrap();
        assert!(!unchanged_generation_world(&before, &changed));
        let mut changed = expanded.clone();
        changed.dimension_type = Some("minecraft:overworld".into());
        assert!(!unchanged_generation_world(&before, &changed));
        let mut changed = expanded.clone();
        changed.inline_dimension_type = Some(Arc::from(r#"{"has_skylight":false}"#));
        assert!(!unchanged_generation_world(&before, &changed));
        let mut changed = expanded.clone();
        changed.chunks.remove(&old_pos);
        assert!(!unchanged_generation_world(&before, &changed));
        let mut changed = expanded.clone();
        Arc::make_mut(changed.chunks.get_mut(&old_pos).unwrap()).status =
            Some("minecraft:full".into());
        assert!(!unchanged_generation_world(&before, &changed));
    }

    #[test]
    fn light_guard_checks_coverage_values_and_allocation() {
        use native_world::generation_light::{VisibleLayers, VisibleLightColumn};
        let pos = ChunkPos { x: 0, z: 0 };
        let positions = BTreeSet::from([pos]);
        let mut before = VisibleLightSnapshot::new(true, false);
        before.insert_complete_column(
            pos,
            VisibleLightColumn::new(VisibleLayers::new(), VisibleLayers::new()),
        );
        assert!(unchanged_visible_light(
            &before,
            &before.clone(),
            &positions
        ));
        assert!(!unchanged_visible_light(
            &before,
            &VisibleLightSnapshot::new(true, false),
            &positions
        ));
        let mut allocated = VisibleLightSnapshot::new(true, false);
        allocated.insert_complete_column(
            pos,
            VisibleLightColumn::new(
                BTreeMap::from([(0, Arc::new([0; native_world::loaded::LIGHT_BYTES]))]),
                VisibleLayers::new(),
            ),
        );
        assert!(!unchanged_visible_light(&before, &allocated, &positions));
        let mut same_contents = VisibleLightSnapshot::new(true, false);
        same_contents.insert_complete_column(
            pos,
            VisibleLightColumn::new(
                BTreeMap::from([(0, Arc::new([0; native_world::loaded::LIGHT_BYTES]))]),
                VisibleLayers::new(),
            ),
        );
        assert!(unchanged_visible_light(
            &allocated,
            &same_contents,
            &positions
        ));
        assert!(!unchanged_visible_light(
            &allocated,
            &VisibleLightSnapshot::new(true, true),
            &positions
        ));
        let mut lit = VisibleLightSnapshot::new(true, false);
        lit.insert_complete_column(
            pos,
            VisibleLightColumn::new(
                BTreeMap::from([(0, Arc::new([1; native_world::loaded::LIGHT_BYTES]))]),
                VisibleLayers::new(),
            ),
        );
        // An unobserved change can be incorporated when the new light is
        // drained, but a callback that read this column must still restart.
        assert!(unchanged_visible_light(&allocated, &lit, &BTreeSet::new()));
        assert!(!unchanged_visible_light(&allocated, &lit, &positions));
    }
}
