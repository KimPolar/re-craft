//! Incremental execution of fresh terrain generation without structures.
use anyhow::{Context, Result, bail, ensure};
use native_resources::{
    block_registry::GeneratedBlockRegistry,
    carver::{schedule::CarverCache, tags::CarverBlockTags},
    density::DensityEvaluator,
    dimension_lighting::GeneratedDimensionLighting,
    feature_order::{FeatureKey, FeatureSchedule, FeatureTags},
    packs::{ResourceId, ResourceProvider},
    placed_feature::PlacedFeature,
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
    spawn_ready: BTreeSet<ChunkPos>,
    entity_sources: GeneratedEntitySources,
    failed: bool,
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
    features_cannot_spawn_entities: bool,
    block_registry: BTreeSet<String>,
    heightmap_tags: native_resources::heightmap_tags::HeightmapTags,
    has_skylight: bool,
    dimension_type: Option<String>,
    ambient_light: Option<f32>,
    mushroom_tags: Option<Arc<native_resources::ore_feature::OreBlockTags>>,
    leaf_tags: Option<Arc<native_resources::ore_feature::OreBlockTags>>,
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

fn features_cannot_spawn_entities(
    provider: &mut impl ResourceProvider,
    schedule: &FeatureSchedule,
    registry: &BTreeSet<ResourceId>,
) -> bool {
    let mut classifier = EntityFeatureClassifier {
        provider,
        registry,
        tags: FeatureTags::default(),
        active: BTreeSet::new(),
        cache: BTreeMap::new(),
        nodes: 0,
    };
    schedule.steps().iter().flatten().all(|key| {
        match key {
            FeatureKey::Registered(id) => classifier.named(id, true, 0),
            FeatureKey::Inline { biome, .. } => schedule
                .biome(biome)
                .and_then(|biome| biome.inline.get(key))
                .context("inline placed feature is missing")
                .and_then(|value| classifier.placed(value, 0)),
        }
        .unwrap_or(false)
    })
}

#[derive(Default)]
struct GeneratedEntitySources {
    complete_spawns: BTreeSet<ChunkPos>,
    untracked: bool,
}

impl GeneratedEntitySources {
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
}

impl FreshTerrainBatch {
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
        variants.finalize(biome, random, group, has_follow_range_bonus)
    }

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

    /// Return complete records only when this no-structures batch has no
    /// unverified entity-producing features and spawning was fully recorded.
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
        if !self.entity_sources.contains_complete_spawn(pos)
            || !decoration.features_cannot_spawn_entities
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
        let features_cannot_spawn_entities =
            features_cannot_spawn_entities(provider, &schedule, placed_features);
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
            features_cannot_spawn_entities,
            block_registry,
            heightmap_tags,
            mushroom_tags,
            leaf_tags,
            bubble_tags,
            washed_blocks,
            lava_fluids,
            has_skylight: lighting.has_skylight,
            ambient_light: lighting.ambient_light,
            dimension_type: lighting
                .dimension_type
                .as_ref()
                .map(|id| format!("{}:{}", id.namespace, id.path)),
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
        // All generated block entities in this path are materialized live;
        // a missing or unsupported record cannot be treated as no entity.
        for &pos in &positions {
            self.spawned_block_entity_data(pos)?;
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
        stage.bind_batch_revision(self.schedule.revision());
        stage.set_leaf_tags(
            self.decoration
                .as_ref()
                .and_then(|config| config.leaf_tags.clone()),
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
        let mut checks = Vec::with_capacity(processed.sealed_checks.len());
        for pos in &processed.sealed_checks {
            let p = [pos.x, pos.y, pos.z];
            checks.push(p);
            for dz in -2i32..=2 {
                for dx in -2i32..=2 {
                    covered.insert(ChunkPos {
                        x: pos.chunk().x.checked_add(dx).context("light X overflow")?,
                        z: pos.chunk().z.checked_add(dz).context("light Z overflow")?,
                    });
                }
            }
        }
        let before = self.light_snapshot(covered)?;
        let updated_sources = if before.has_sky_channel() {
            updated_postprocessing_sky_sources(
                &processed.source,
                &processed.world,
                &processed.sealed_checks,
                bounds,
                &engine.fresh.sky_sources,
            )?
        } else {
            None
        };
        super::postprocessing_world::drain_postprocessing_light(
            &processed.world,
            bounds,
            &before,
            &engine.active,
            updated_sources
                .as_ref()
                .unwrap_or(&engine.fresh.sky_sources),
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
        let mut selected = BTreeSet::new();
        for pos in positions {
            ensure!(selected.insert(pos), "duplicate postprocessed chunk");
            ensure!(
                processed.chunks.contains(&pos),
                "chunk was not postprocessed"
            );
        }
        ensure!(!selected.is_empty(), "no postprocessed chunks selected");
        let light = self.postprocessed_light_snapshot(processed)?;
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
        use native_world::loaded::{SavedBlockTick, SavedFluidTick};

        let sections = self.postprocessed_sections_data(processed, positions)?;
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
                    !has_entity(old)? && !has_entity(new)?,
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
                        block_entities: self.spawned_block_entity_data(pos)?,
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
        let time = processed.ticks.game_time();
        self.postprocessed_chunk_components(processed, positions)?
            .into_iter()
            .map(|(pos, components)| {
                let entity_chunk = components
                    .entity_records
                    .as_ref()
                    .map(|records| {
                        native_persistence::generated_chunk::assemble_entity_chunk(pos, records)
                    })
                    .transpose()?;
                let chunk = native_persistence::generated_chunk::assemble_full_chunk(
                    native_persistence::generated_chunk::FullChunkComponents {
                        position: pos,
                        min_y: self.generator.min_y,
                        height: self.generator.height,
                        last_update: time,
                        sections: components.sections,
                        heightmaps: components.heightmaps,
                        block_ticks: components.block_ticks,
                        fluid_ticks: components.fluid_ticks,
                        block_entities: components.block_entities,
                    },
                )?;
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
        self.validate_postprocessing(processed)?;
        let registry = self
            .light_registry
            .as_ref()
            .context("block registry is missing")?;
        let mut output = BTreeMap::new();
        for pos in positions {
            ensure!(
                processed.chunks.contains(&pos),
                "chunk was not postprocessed"
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
        native_persistence::generated_chunk::assemble_spawn_chunk(
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
        )
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
                    terrain.initialize_worldgen_heightmaps(pos, &config.heightmap_tags)?;
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
