//! Prepare generation inputs from a verified client and selected data packs.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
};

use anyhow::{Result, ensure};
use native_resources::{
    carver::tags::CarverBlockTags,
    datapacks::DataPackSettings,
    dimension_lighting::GeneratedDimensionLighting,
    packs::{ResourceId, ResourceProvider, ResourceStack},
};
use native_session::worldgen::{
    CarverBlockState, FreshTerrainBatch, TerrainGenerator, resolve_generated_terrain_state,
    structure_batch::StructureGenerationBatch,
};
use native_world::{fluid::FluidState, loaded::BlockState};
use serde_json::Value;

pub(crate) mod existing;

pub struct FreshGenerationSettings {
    pub minecraft_dir: PathBuf,
    pub preset: String,
    pub dimension: String,
    pub seed: i64,
    pub bonus_chest: bool,
    /// The ordinary fresh-generation batch does not yet support structures.
    pub structures_enabled: bool,
    pub data_packs: Option<(PathBuf, DataPackSettings)>,
}

pub(crate) type Classifier = Box<dyn FnMut(&Value) -> Result<CarverBlockState> + Send>;
pub(crate) type Resolver = Box<dyn FnMut(&Value) -> Result<BlockState> + Send>;
pub(crate) type HeightClassifier =
    Box<dyn FnMut(&Value) -> Result<native_persistence::heightmap::BlockHeightFlags> + Send>;
type StructuredConfigurer =
    Box<dyn FnOnce(&mut FreshTerrainBatch, &mut ResourceStack) -> Result<()> + Send>;
pub(crate) type PreparedStructuredGeneration = crate::generation_worker::StructuredGenerationInputs<
    ResourceStack,
    Classifier,
    Resolver,
    HeightClassifier,
    StructuredConfigurer,
>;

pub(crate) fn height_classifier(
    registry: Arc<native_resources::block_registry::GeneratedBlockRegistry>,
    tags: Arc<native_resources::heightmap_tags::HeightmapTags>,
) -> HeightClassifier {
    Box::new(move |value| {
        let block = resolve_generated_terrain_state(value, &registry)?;
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
    })
}

pub(crate) fn block_classifiers(
    registry: Arc<native_resources::block_registry::GeneratedBlockRegistry>,
) -> (Classifier, Resolver) {
    let for_classify = Arc::clone(&registry);
    let classify: Classifier = Box::new(move |value| {
        let block = resolve_generated_terrain_state(value, &for_classify)?;
        Ok(CarverBlockState {
            is_air: matches!(
                block.name.as_str(),
                "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
            ),
            has_fluid: FluidState::from_block(&block)
                .map_err(anyhow::Error::msg)?
                .is_some(),
        })
    });
    let resolve: Resolver =
        Box::new(move |value| resolve_generated_terrain_state(value, &registry));
    (classify, resolve)
}

pub struct PreparedFreshGeneration {
    pub lighting: GeneratedDimensionLighting,
    dimension_definition: Value,
    worldgen_settings: native_persistence::nbt::NamedTag,
    pack_selection: Option<DataPackSettings>,
    batch: FreshTerrainBatch,
    resources: ResourceStack,
    classify: Classifier,
    resolve: Resolver,
}

/// Inputs kept together from pack selection through initial creature spawning.
pub struct PreparedInitialWorld {
    pub batch: FreshTerrainBatch,
    pub resources: ResourceStack,
    pub classify: Classifier,
    pub resolve: Resolver,
    pub height_tags: Arc<native_resources::heightmap_tags::HeightmapTags>,
    pub worldgen_settings: native_persistence::nbt::NamedTag,
    /// Outer option means the world requests a bonus; the feature itself can
    /// be absent in the selected registry.
    pub bonus_chest_feature: Option<Option<native_resources::bonus_chest::BonusChestFeature>>,
    pub creatures: Option<PreparedCreatureSpawning>,
    pub spawn_mobs: bool,
    pub border: native_session::respawn::RespawnBorder,
    pub initial_clocks: Arc<InitialWorldClocks>,
    pub entity_collision_registry:
        Option<Arc<native_resources::entity_collision::EntityCollisionRegistry>>,
    pack_selection: Option<DataPackSettings>,
    pub level_settings: Option<crate::world_creation::FrozenLevelSettings>,
}

impl PreparedInitialWorld {
    /// Tie the frozen screen settings to the exact seed used for this batch.
    pub fn set_level_settings(
        &mut self,
        settings: crate::world_creation::FrozenLevelSettings,
    ) -> Result<()> {
        settings.game_rules.new_world_document()?;
        ensure!(
            settings.game_rules.spawn_mobs == self.spawn_mobs
                && settings.game_rules.spawn_mobs == self.initial_clocks.spawn_mobs
                && settings.game_rules.advance_time == self.initial_clocks.advance_time,
            "game rules differ from the prepared generation inputs"
        );
        ensure!(
            self.worldgen_settings
                .value
                .get("data")
                .and_then(|data| data.get("seed"))
                == Some(&native_persistence::nbt::Tag::Long(settings.session.seed)),
            "session seed differs from the prepared generator"
        );
        let same_packs = match (&self.pack_selection, &settings.packs) {
            (None, None) => true,
            (Some(a), Some(b)) => {
                a.enabled == b.enabled
                    && a.disabled == b.disabled
                    && a.enabled_features == b.enabled_features
            }
            _ => false,
        };
        ensure!(
            same_packs,
            "level data packs differ from the prepared generator"
        );
        self.level_settings = Some(settings);
        Ok(())
    }
}

/// Clock ticks sampled during generation and the selected time game rule.
/// Newly created clocks use Java's default rate, partial tick and pause state.
pub struct InitialWorldClocks {
    pub ticks: BTreeMap<ResourceId, i64>,
    pub advance_time: bool,
    /// Frozen separately from whether this dimension's generator permits mobs.
    pub spawn_mobs: bool,
    pub command_data: InitialClockCommandData,
    /// Dimension attributes evaluated against these same world clocks.
    pub fluid_environment: InitialFluidEnvironment,
}

/// Data-pack inputs and immutable time-zero clocks for initial generation.
pub(crate) struct InitialHutGeneration {
    pub variants: native_resources::cat_variants::CatSpawnVariants,
    pub conditions: native_resources::cat_variants::CatWorldConditions,
    pub moon: native_resources::moon_phase::Environment,
    pub clocks: BTreeMap<ResourceId, i64>,
    /// Present only when generation is tied to frozen create-world settings.
    pub difficulty: Option<native_persistence::world::Difficulty>,
    pub ocean: Option<InitialOceanGeneration>,
    pub chest_minecart_enabled: bool,
}

pub(crate) type CreatureConstructor =
    fn() -> Result<Option<native_session::worldgen::FreshCreatureRandom>>;
pub(crate) type OceanEffects = native_session::worldgen::ocean_ruin::DrownedEquipmentEffects<
    native_session::worldgen::ocean_ruin::DrownedChickenEffects<
        native_session::worldgen::ocean_ruin::DrownedNautilusEffects<
            UnsupportedOceanEffects,
            CreatureConstructor,
        >,
        CreatureConstructor,
    >,
>;

pub(crate) struct InitialOceanGeneration {
    pub ice_tags: native_resources::ore_feature::OreBlockTags,
    pub water_fluids: BTreeSet<String>,
    pub bubble_fluids: BTreeSet<String>,
    pub registry: Arc<native_resources::block_registry::GeneratedBlockRegistry>,
    pub biome_tags: native_resources::drowned_spawn::DrownedBiomeTags,
    pub effects: OceanEffects,
}

#[derive(Clone)]
pub(crate) struct UnsupportedOceanEffects;

impl native_session::worldgen::ocean_ruin::DrownedSpawnEffects for UnsupportedOceanEffects {
    fn needs_structure_variants(&self) -> bool {
        false
    }
    fn try_existing_chicken(
        &mut self,
        _: [i32; 3],
        _: &native_world::loaded::World,
    ) -> Result<bool> {
        Ok(false)
    }
    fn spawn_chicken_jockey(
        &mut self,
        _: [i32; 3],
        _: &native_world::loaded::World,
        _: &mut native_resources::worldgen_region_random::RegionRandom,
        _: &native_session::worldgen::BiomeNeighborhood,
    ) -> Result<bool> {
        anyhow::bail!("chicken construction is unavailable")
    }
    fn enchant_weapon(
        &mut self,
        _: [i32; 3],
        _: &native_world::loaded::World,
        _: &mut native_resources::worldgen_region_random::RegionRandom,
        _: native_resources::drowned_spawn::MainHandRoll,
        _: f32,
    ) -> Result<native_persistence::nbt::Tag> {
        anyhow::bail!("drowned equipment provider is unavailable")
    }
    fn spawn_zombie_nautilus(
        &mut self,
        _: [i32; 3],
        _: &native_world::loaded::World,
        _: &mut native_resources::worldgen_region_random::RegionRandom,
        _: bool,
    ) -> Result<bool> {
        anyhow::bail!("zombie nautilus generation is not implemented")
    }
}

fn construct_chicken() -> Result<Option<native_session::worldgen::FreshCreatureRandom>> {
    Ok(Some(
        native_session::worldgen::FreshCreatureRandom::from_entropy(&ResourceId::parse(
            "minecraft:chicken",
        )?)?,
    ))
}

fn construct_zombie_nautilus() -> Result<Option<native_session::worldgen::FreshCreatureRandom>> {
    Ok(Some(
        native_session::worldgen::FreshCreatureRandom::from_entropy(&ResourceId::parse(
            "minecraft:zombie_nautilus",
        )?)?,
    ))
}

impl InitialOceanGeneration {
    fn load(
        resources: &mut ResourceStack,
        blocks: &BTreeSet<String>,
        registry: Arc<native_resources::block_registry::GeneratedBlockRegistry>,
    ) -> Result<Self> {
        use native_resources::{
            animal_variants::{FarmAnimal, FarmAnimalSpawnVariants, ZombieNautilusSpawnVariants},
            drowned_spawn::DrownedBiomeTags,
            fluid_tags,
            ore_feature::OreBlockTags,
        };
        use native_session::worldgen::ocean_ruin::{
            DrownedChickenEffects, DrownedEquipmentEffects, DrownedNautilusEffects,
        };
        let ice_tags = OreBlockTags::load(
            resources,
            blocks,
            &BTreeSet::from([ResourceId::parse("minecraft:ice")?]),
        )?;
        let water_fluids = fluid_tags::load_water(resources)?;
        let bubble_fluids = fluid_tags::load_bubble_column_occupiable(resources)?;
        let biome_tags = DrownedBiomeTags::load(resources)?;
        let variants = Arc::new(FarmAnimalSpawnVariants::load_active(
            resources,
            FarmAnimal::Chicken,
        )?);
        let items = resources.vanilla_item_registry()?;
        let known_items = items
            .iter()
            .map(|(id, _)| ResourceId::parse(id))
            .collect::<Result<BTreeSet<_>>>()?;
        let nautilus = DrownedNautilusEffects::new(
            Arc::new(ZombieNautilusSpawnVariants::load_active(resources)?),
            construct_zombie_nautilus as CreatureConstructor,
            UnsupportedOceanEffects,
        );
        let chickens = DrownedChickenEffects::new(
            variants,
            construct_chicken as CreatureConstructor,
            nautilus,
        )?;
        // Both vanilla item prototypes have enchantability one in Java 26.3.
        let effects =
            DrownedEquipmentEffects::load(resources, &known_items, Some(1), Some(1), chickens)?;
        Ok(Self {
            ice_tags,
            water_fluids,
            bubble_fluids,
            registry,
            biome_tags,
            effects,
        })
    }
}

impl InitialHutGeneration {
    fn load(
        resources: &mut ResourceStack,
        dimension_definition: &Value,
        difficulty: Option<native_persistence::world::Difficulty>,
        blocks: &BTreeSet<String>,
        registry: Arc<native_resources::block_registry::GeneratedBlockRegistry>,
        chest_minecart_enabled: bool,
    ) -> Result<Self> {
        let clocks = native_resources::world_clock::registered_clocks(resources)?
            .into_iter()
            .map(|id| (id, 0))
            .collect();
        let variants = native_resources::cat_variants::CatSpawnVariants::load_active(resources)?;
        let conditions = variants.prepare_world_conditions(resources)?;
        let moon = native_resources::moon_phase::Environment::from_definition(
            resources,
            dimension_definition,
        )?;
        let ocean = if difficulty.is_some() {
            Some(InitialOceanGeneration::load(resources, blocks, registry)?)
        } else {
            None
        };
        Ok(Self {
            variants,
            conditions,
            moon,
            clocks,
            difficulty,
            ocean,
            chest_minecart_enabled,
        })
    }
}

#[derive(Default)]
pub struct InitialFluidEnvironment {
    pub fast_lava: Option<Arc<native_resources::fluid_environment::FastLavaEnvironment>>,
    pub water_evaporation:
        Option<Arc<native_resources::fluid_environment::WaterEvaporationEnvironment>>,
}

impl InitialFluidEnvironment {
    fn load(resources: &mut ResourceStack, definition: &Value) -> Self {
        let fast_lava =
            match native_resources::fluid_environment::fast_lava_environment_from_definition(
                resources, definition,
            ) {
                Ok(value) => Some(Arc::new(value)),
                Err(error) => {
                    eprintln!("Fast lava environment is unavailable: {error:#}");
                    None
                }
            };
        let water_evaporation =
            match native_resources::fluid_environment::water_evaporation_environment_from_definition(
                resources, definition,
            ) {
                Ok(value) => Some(Arc::new(value)),
                Err(error) => {
                    eprintln!("Water evaporation rules are unavailable: {error:#}");
                    None
                }
            };
        Self {
            fast_lava,
            water_evaporation,
        }
    }
}

/// Command registries are optional independently, matching existing-world
/// loading when one unsupported data-pack definition cannot be decoded.
#[derive(Default)]
pub struct InitialClockCommandData {
    pub default_clock: Option<(Arc<str>, Option<ResourceId>)>,
    pub timelines:
        Option<Arc<BTreeMap<ResourceId, native_resources::world_clock::TimelineDefinition>>>,
    pub markers:
        Option<Arc<BTreeMap<(ResourceId, ResourceId), native_resources::world_clock::TimeMarker>>>,
}

impl InitialClockCommandData {
    fn load(
        resources: &mut ResourceStack,
        lighting: &GeneratedDimensionLighting,
        definition: &Value,
    ) -> Self {
        let name = lighting.dimension_type.as_ref().map_or_else(
            || "[unregistered]".to_owned(),
            |id| format!("{}:{}", id.namespace, id.path),
        );
        let default_clock =
            match native_resources::world_clock::dimension_default_clock_from_definition(definition)
            {
                Ok(clock) => Some((Arc::from(name), clock)),
                Err(error) => {
                    eprintln!("Dimension clock is unavailable: {error:#}");
                    None
                }
            };
        let timelines = match native_resources::world_clock::timelines(resources) {
            Ok(timelines) => Some(Arc::new(timelines)),
            Err(error) => {
                eprintln!("Timelines are unavailable: {error:#}");
                None
            }
        };
        let markers = match native_resources::world_clock::time_markers(resources) {
            Ok(markers) => Some(Arc::new(markers)),
            Err(error) => {
                eprintln!("Time markers are unavailable: {error:#}");
                None
            }
        };
        Self {
            default_clock,
            timelines,
            markers,
        }
    }
}

/// All inputs for world-generation creatures, resolved from the same active
/// pack stack and world clock state used by the generation batch.
pub struct PreparedCreatureSpawning {
    environment: native_resources::creature_timeline::CreatureSpawnEnvironment,
    categories: native_resources::entity_category::EntityCategoryRegistry,
    prevent_inside: BTreeSet<String>,
    height_tags: native_resources::heightmap_tags::HeightmapTags,
    ground_tags: native_resources::worldgen_mob_rules::GroundSpawnTags,
    biome_tags: native_resources::worldgen_region_random::SheepBiomeTags,
    variants: [Arc<native_resources::animal_variants::FarmAnimalSpawnVariants>; 3],
    border: native_session::respawn::RespawnBorder,
    clocks: Arc<BTreeMap<ResourceId, i64>>,
    moon: Option<Arc<native_resources::moon_phase::Environment>>,
    has_ceiling: bool,
}

impl PreparedCreatureSpawning {
    /// Load on the worker with the already-configured world data packs.
    /// Unsupported spawn conditions remain errors.
    pub fn load(
        resources: &mut ResourceStack,
        definition: &Value,
        border: native_session::respawn::RespawnBorder,
        clocks: BTreeMap<ResourceId, i64>,
    ) -> Result<Self> {
        use anyhow::Context;
        use native_resources::animal_variants::{FarmAnimal, FarmAnimalSpawnVariants};
        ensure!(
            border.min_x.is_finite()
                && border.max_x.is_finite()
                && border.min_z.is_finite()
                && border.max_z.is_finite()
                && border.min_x < border.max_x
                && border.min_z < border.max_z,
            "invalid world border"
        );
        let has_ceiling = definition
            .get("has_ceiling")
            .and_then(Value::as_bool)
            .context("dimension has_ceiling is missing or invalid")?;
        let categories = resources.vanilla_entity_category_registry()?;
        let environment =
            native_resources::creature_timeline::CreatureSpawnEnvironment::from_definition_with_registry(
                resources,
                definition,
                &categories,
            )?;
        environment.sample_layers(|id| clocks.get(id).copied())?;
        let blocks = resources.vanilla_block_ids()?;
        let prevent_inside =
            native_resources::mob_spawns::load_prevent_mob_spawning_inside(resources, &blocks)?;
        let height_tags =
            native_resources::heightmap_tags::HeightmapTags::load(resources, &blocks)?;
        let ground_tags =
            native_resources::worldgen_mob_rules::GroundSpawnTags::load(resources, &blocks)?;
        let biome_tags = native_resources::worldgen_region_random::SheepBiomeTags::load(resources)?;
        let variants = [
            Arc::new(FarmAnimalSpawnVariants::load_active(
                resources,
                FarmAnimal::Pig,
            )?),
            Arc::new(FarmAnimalSpawnVariants::load_active(
                resources,
                FarmAnimal::Cow,
            )?),
            Arc::new(FarmAnimalSpawnVariants::load_active(
                resources,
                FarmAnimal::Chicken,
            )?),
        ];
        let moon = variants
            .iter()
            .any(|variant| variant.uses_moon_conditions())
            .then(|| {
                native_resources::moon_phase::Environment::from_definition(resources, definition)
                    .map(Arc::new)
            })
            .transpose()?;
        Ok(Self {
            environment,
            categories,
            prevent_inside,
            height_tags,
            ground_tags,
            biome_tags,
            variants,
            border,
            clocks: Arc::new(clocks),
            moon,
            has_ceiling,
        })
    }

    pub fn build_spawner(
        &self,
        batch: &FreshTerrainBatch,
        resources: &mut ResourceStack,
        pos: native_world::loaded::ChunkPos,
    ) -> Result<native_session::worldgen::GeneratedSheepSpawner> {
        use native_session::worldgen::{GeneratedSheepSpawner, SheepSpawnInputs};
        let inputs = SheepSpawnInputs {
            border: self.border,
            prevent_inside: &self.prevent_inside,
            height_tags: &self.height_tags,
            ground_tags: &self.ground_tags,
            biome_tags: &self.biome_tags,
            has_ceiling: self.has_ceiling,
        };
        let mut driver = GeneratedSheepSpawner::from_verified_environment(
            batch,
            resources,
            pos,
            &self.environment,
            |id| self.clocks.get(id).copied(),
            &self.categories,
            inputs,
        )?;
        for variants in &self.variants {
            driver.enable_farm_animal(Arc::clone(variants))?;
        }
        if let Some(moon) = &self.moon {
            driver.set_farm_moon_environment(Arc::clone(moon), Arc::clone(&self.clocks))?;
        }
        Ok(driver)
    }
}

pub(crate) struct StructuredInitialContext {
    pub clocks: Arc<InitialWorldClocks>,
    pub collision_registry:
        Option<Arc<native_resources::entity_collision::EntityCollisionRegistry>>,
    pub creatures: Option<PreparedCreatureSpawning>,
}

/// Resolve initial rules from the already captured generation pack stack.
pub(crate) fn structured_initial_context(
    resources: &mut ResourceStack,
    preset: &str,
    dimension: &str,
    border: native_session::respawn::RespawnBorder,
    spawn_mobs: bool,
    generator_disables_mobs: bool,
    advance_time: bool,
) -> Result<StructuredInitialContext> {
    let (lighting, _, definition) =
        GeneratedDimensionLighting::load_preset_with_definition(resources, preset, dimension)?;
    let ticks: BTreeMap<_, _> = native_resources::world_clock::registered_clocks(resources)?
        .into_iter()
        .map(|id| (id, 0))
        .collect();
    let clocks = Arc::new(InitialWorldClocks {
        ticks: ticks.clone(),
        advance_time,
        spawn_mobs,
        command_data: InitialClockCommandData::load(resources, &lighting, &definition),
        fluid_environment: InitialFluidEnvironment::load(resources, &definition),
    });
    let collision_registry = match resources.vanilla_entity_collision_registry() {
        Ok(registry) => Some(Arc::new(registry)),
        Err(error) => {
            eprintln!("Spawn entity collision registry is unavailable: {error:#}");
            None
        }
    };
    let creatures = if spawn_mobs && !generator_disables_mobs {
        Some(PreparedCreatureSpawning::load(
            resources,
            &definition,
            border,
            ticks,
        )?)
    } else {
        None
    };
    Ok(StructuredInitialContext {
        clocks,
        collision_registry,
        creatures,
    })
}

impl PreparedFreshGeneration {
    /// Initialize every registered clock from the selected pack stack to the
    /// fresh server's default tick. Debug-world time changes are handled by
    /// their own world initialization and are not supported by this generator.
    pub fn into_new_world_inputs(
        mut self,
        border: native_session::respawn::RespawnBorder,
        spawn_mobs: bool,
        advance_time: bool,
    ) -> Result<PreparedInitialWorld> {
        let clocks = native_resources::world_clock::registered_clocks(&mut self.resources)?
            .into_iter()
            .map(|id| (id, 0))
            .collect();
        self.into_initial_world_inputs(border, clocks, spawn_mobs, advance_time)
    }

    /// Resolve world-spawn and creature inputs from the already-selected pack
    /// stack, then transfer that same stack to the generation worker.
    pub fn into_initial_world_inputs(
        mut self,
        border: native_session::respawn::RespawnBorder,
        clocks: BTreeMap<ResourceId, i64>,
        spawn_mobs: bool,
        advance_time: bool,
    ) -> Result<PreparedInitialWorld> {
        let initial_clocks = Arc::new(InitialWorldClocks {
            ticks: clocks.clone(),
            advance_time,
            spawn_mobs,
            command_data: InitialClockCommandData::load(
                &mut self.resources,
                &self.lighting,
                &self.dimension_definition,
            ),
            fluid_environment: InitialFluidEnvironment::load(
                &mut self.resources,
                &self.dimension_definition,
            ),
        });
        let entity_collision_registry = match self.resources.vanilla_entity_collision_registry() {
            Ok(registry) => Some(Arc::new(registry)),
            Err(error) => {
                eprintln!("Spawn entity collision registry is unavailable: {error:#}");
                None
            }
        };
        let creatures = if spawn_mobs && !self.batch.mob_generation_disabled()? {
            Some(PreparedCreatureSpawning::load(
                &mut self.resources,
                &self.dimension_definition,
                border,
                clocks,
            )?)
        } else {
            None
        };
        let height_tags = if let Some(creatures) = &creatures {
            Arc::new(creatures.height_tags.clone())
        } else {
            let blocks = self.resources.vanilla_block_ids()?;
            Arc::new(native_resources::heightmap_tags::HeightmapTags::load(
                &mut self.resources,
                &blocks,
            )?)
        };
        let bonus_chest_feature = if self
            .worldgen_settings
            .value
            .get("data")
            .and_then(|data| data.get("bonus_chest"))
            == Some(&native_persistence::nbt::Tag::Byte(1))
        {
            Some(native_resources::bonus_chest::BonusChestFeature::load(
                &mut self.resources,
            )?)
        } else {
            None
        };
        Ok(PreparedInitialWorld {
            batch: self.batch,
            resources: self.resources,
            classify: self.classify,
            resolve: self.resolve,
            height_tags,
            worldgen_settings: self.worldgen_settings,
            bonus_chest_feature,
            creatures,
            spawn_mobs,
            border,
            initial_clocks,
            entity_collision_registry,
            pack_selection: self.pack_selection,
            level_settings: None,
        })
    }

    /// Compute the generation origin before moving these inputs to a worker.
    pub fn spawn_origin(&mut self) -> Result<native_world::loaded::ChunkPos> {
        self.batch.spawn_origin(&mut self.resources)
    }

    pub fn into_worker_inputs(self) -> (FreshTerrainBatch, ResourceStack, Classifier, Resolver) {
        (self.batch, self.resources, self.classify, self.resolve)
    }

    /// Resolve spawn heightmap tags from the same data-pack stack before it
    /// is handed to the background spawn search.
    pub fn into_spawn_worker_inputs(
        mut self,
    ) -> Result<(
        FreshTerrainBatch,
        ResourceStack,
        Classifier,
        Resolver,
        Arc<native_resources::heightmap_tags::HeightmapTags>,
        native_persistence::nbt::NamedTag,
    )> {
        let blocks = self.resources.vanilla_block_ids()?;
        let tags = Arc::new(native_resources::heightmap_tags::HeightmapTags::load(
            &mut self.resources,
            &blocks,
        )?);
        Ok((
            self.batch,
            self.resources,
            self.classify,
            self.resolve,
            tags,
            self.worldgen_settings,
        ))
    }
}

/// Call on the generation worker, not the window thread. Warnings from the
/// chosen pack stack are errors until the UI can present and resolve them.
fn open_resources(settings: &FreshGenerationSettings) -> Result<ResourceStack> {
    ensure!(
        !settings.bonus_chest || settings.dimension == "minecraft:overworld",
        "initial bonus chest requires the overworld"
    );
    let mut resources = ResourceStack::open(&settings.minecraft_dir)?;
    if let Some((world_dir, selection)) = &settings.data_packs {
        let snapshot =
            Arc::new(native_resources::datapack_snapshot::DataPackSnapshot::capture(world_dir)?);
        let warnings = resources.configure_snapshot_data(snapshot, selection)?;
        ensure!(
            warnings.is_empty(),
            "data pack selection needs attention: {}",
            warnings.join("; ")
        );
    }
    Ok(resources)
}

pub fn prepare(settings: FreshGenerationSettings) -> Result<PreparedFreshGeneration> {
    ensure!(
        !settings.structures_enabled,
        "fresh generation with structures is not implemented"
    );
    let mut resources = open_resources(&settings)?;
    let pack_selection = settings
        .data_packs
        .as_ref()
        .map(|(_, selection)| selection.clone());
    let worldgen_settings = native_persistence::worldgen_settings::draft_baked_worldgen_settings(
        &mut resources,
        &settings.preset,
        native_persistence::worldgen_settings::NewWorldOptions {
            seed: settings.seed,
            generate_structures: settings.structures_enabled,
            bonus_chest: settings.bonus_chest,
        },
    )?;
    let (lighting, definition, dimension_definition) =
        GeneratedDimensionLighting::load_preset_with_definition(
            &mut resources,
            &settings.preset,
            &settings.dimension,
        )?;
    let blocks = resources.vanilla_block_ids()?;
    let tags = CarverBlockTags::load(&mut resources, &blocks)?;
    let generator = TerrainGenerator::without_terrain_adjustments(
        &mut resources,
        &definition,
        settings.seed,
        lighting.min_y,
        lighting.height,
    )?;
    let mut batch =
        FreshTerrainBatch::new_without_structures(generator, tags, &settings.dimension)?;
    let features = resources
        .list_data("worldgen/placed_feature")?
        .into_iter()
        .collect();
    batch.enable_decoration(&mut resources, &features, blocks, &lighting)?;
    let registry = resources.vanilla_generated_block_registry()?;
    batch.enable_light_initialization(registry.clone())?;
    let registry = Arc::new(registry);
    let classify_registry = Arc::clone(&registry);
    let classify: Classifier = Box::new(move |value| {
        let block = resolve_generated_terrain_state(value, &classify_registry)?;
        Ok(CarverBlockState {
            is_air: matches!(
                block.name.as_str(),
                "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
            ),
            has_fluid: FluidState::from_block(&block)
                .map_err(anyhow::Error::msg)?
                .is_some(),
        })
    });
    let resolve: Resolver =
        Box::new(move |value| resolve_generated_terrain_state(value, &registry));
    Ok(PreparedFreshGeneration {
        lighting,
        dimension_definition,
        worldgen_settings,
        pack_selection,
        batch,
        resources,
        classify,
        resolve,
    })
}

/// Prepare a finite structure-aware batch from one captured pack stack.
/// The worker result remains intermediate until spawn and saving are supported.
pub(crate) fn prepare_with_structures(
    settings: FreshGenerationSettings,
) -> Result<PreparedStructuredGeneration> {
    prepare_with_structures_and_difficulty(settings, None)
}

pub(crate) fn prepare_with_structures_and_difficulty(
    settings: FreshGenerationSettings,
    difficulty: Option<native_persistence::world::Difficulty>,
) -> Result<PreparedStructuredGeneration> {
    ensure!(settings.structures_enabled, "structures must be enabled");
    let mut resources = open_resources(&settings)?;
    let worldgen_settings = native_persistence::worldgen_settings::draft_baked_worldgen_settings(
        &mut resources,
        &settings.preset,
        native_persistence::worldgen_settings::NewWorldOptions {
            seed: settings.seed,
            generate_structures: true,
            bonus_chest: settings.bonus_chest,
        },
    )?;
    let (lighting, definition, dimension_definition) =
        GeneratedDimensionLighting::load_preset_with_definition(
            &mut resources,
            &settings.preset,
            &settings.dimension,
        )?;
    let blocks = resources.vanilla_block_ids()?;
    let registry = Arc::new(resources.vanilla_generated_block_registry()?);
    let hut_generation = Some(Arc::new(InitialHutGeneration::load(
        &mut resources,
        &dimension_definition,
        difficulty,
        &blocks,
        Arc::clone(&registry),
        settings.data_packs.as_ref().is_none_or(|(_, selection)| {
            selection
                .enabled_features
                .iter()
                .any(|feature| feature == "minecraft:vanilla")
        }),
    )?));
    let terrain_tags = CarverBlockTags::load(&mut resources, &blocks)?;
    let height_tags = Arc::new(native_resources::heightmap_tags::HeightmapTags::load(
        &mut resources,
        &blocks,
    )?);
    let (generator, terrain_generator) = TerrainGenerator::paired_with_structure_context(
        &mut resources,
        &definition,
        settings.seed,
        lighting.min_y,
        lighting.height,
    )?;
    let structures = StructureGenerationBatch::from_registry(generator, &mut resources)?;
    let features = resources
        .list_data("worldgen/placed_feature")?
        .into_iter()
        .collect();
    let (classify, resolve) = block_classifiers(Arc::clone(&registry));
    let classify_height = height_classifier(Arc::clone(&registry), Arc::clone(&height_tags));
    let configure_terrain: StructuredConfigurer = Box::new(move |batch, provider| {
        batch.enable_decoration(provider, &features, blocks, &lighting)?;
        batch.enable_light_initialization((*registry).clone())
    });
    let pack_snapshot = resources.data_pack_snapshot();
    Ok(crate::generation_worker::StructuredGenerationInputs {
        structures,
        terrain_generator: Some(terrain_generator),
        height_tags: Some(height_tags),
        provider: resources,
        classify,
        resolve,
        classify_height,
        terrain_tags,
        dimension: settings.dimension,
        configure_terrain,
        worldgen_settings_draft: Some(worldgen_settings),
        pack_snapshot,
        hut_generation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;
    use serde_json::json;

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn structured_inputs_keep_active_pack_tags_and_snapshot() -> Result<()> {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let temp = tempfile::tempdir()?;
        let world = temp.path().join("world");
        let pack = world.join("datapacks/overrides");
        let tags = pack.join("data/minecraft/tags/block");
        let structures = pack.join("data/example/worldgen/structure");
        let sets = pack.join("data/example/worldgen/structure_set");
        std::fs::create_dir_all(&tags)?;
        std::fs::create_dir_all(&structures)?;
        std::fs::create_dir_all(&sets)?;
        std::fs::write(
            pack.join("pack.mcmeta"),
            br#"{"pack":{"description":"test","min_format":121,"max_format":121}}"#,
        )?;
        let tag_path = tags.join("blocks_motion_in_heightmap.json");
        std::fs::write(
            &tag_path,
            br#"{"replace":true,"values":["minecraft:grass_block"]}"#,
        )?;
        std::fs::write(structures.join("treasure.json"), br#"{"type":"minecraft:buried_treasure","biomes":["minecraft:plains"],"step":"surface_structures"}"#)?;
        std::fs::write(sets.join("treasure.json"), br#"{"placement":{"type":"minecraft:random_spread","spacing":1,"separation":0,"salt":0},"structures":[{"structure":"example:treasure","weight":1}]}"#)?;
        let mut inputs = prepare_with_structures(FreshGenerationSettings {
            minecraft_dir: game,
            preset: "minecraft:normal".into(),
            dimension: "minecraft:overworld".into(),
            seed: 42,
            bonus_chest: false,
            structures_enabled: true,
            data_packs: Some((
                world,
                DataPackSettings {
                    enabled: vec!["vanilla".into(), "file/overrides".into()],
                    disabled: vec![],
                    enabled_features: vec!["minecraft:vanilla".into()],
                },
            )),
        })?;
        std::fs::write(
            &tag_path,
            br#"{"replace":true,"values":["minecraft:stone"]}"#,
        )?;
        assert!((inputs.classify_height)(&json!("minecraft:grass_block"))?.blocks_motion);
        assert!(!(inputs.classify_height)(&json!("minecraft:stone"))?.blocks_motion);
        assert!((inputs.classify_height)(&json!("minecraft:air"))?.is_air);
        assert!((inputs.classify_height)(&json!("minecraft:water"))?.has_fluid);
        assert_eq!(
            inputs
                .worldgen_settings_draft
                .as_ref()
                .unwrap()
                .value
                .get("data")
                .and_then(|v| v.get("generate_structures")),
            Some(&native_persistence::nbt::Tag::Byte(1))
        );
        let snapshot = inputs
            .pack_snapshot
            .as_ref()
            .context("pack snapshot missing")?;
        assert!(Arc::ptr_eq(
            snapshot,
            &inputs.provider.data_pack_snapshot().unwrap()
        ));
        snapshot.verify()?;
        assert!(
            inputs
                .structures
                .structure_sets()
                .contains(&ResourceId::parse("example:treasure")?)
        );
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn installed_creature_inputs_load_from_active_pack_stack() -> Result<()> {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = ResourceStack::open(&game)?;
        let clocks: BTreeMap<ResourceId, i64> =
            native_resources::world_clock::registered_clocks(&mut resources)?
                .into_iter()
                .map(|id| (id, 0_i64))
                .collect();
        for (dimension, ceiling) in [
            ("minecraft:overworld", false),
            ("minecraft:the_nether", true),
        ] {
            let (_, _, definition) = GeneratedDimensionLighting::load_preset_with_definition(
                &mut resources,
                "minecraft:normal",
                dimension,
            )?;
            let border = native_session::respawn::RespawnBorder {
                min_x: -29_999_984.0,
                max_x: 29_999_984.0,
                min_z: -29_999_984.0,
                max_z: 29_999_984.0,
            };
            let inputs = PreparedCreatureSpawning::load(
                &mut resources,
                &definition,
                border,
                clocks.clone(),
            )?;
            assert_eq!(inputs.has_ceiling, ceiling);
            assert!(!inputs.prevent_inside.is_empty());
            assert_eq!(
                inputs.variants[0].species(),
                native_resources::animal_variants::FarmAnimal::Pig
            );
            assert_eq!(
                inputs.variants[1].species(),
                native_resources::animal_variants::FarmAnimal::Cow
            );
            assert_eq!(
                inputs.variants[2].species(),
                native_resources::animal_variants::FarmAnimal::Chicken
            );
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn initial_world_inputs_keep_selected_pack_for_terrain_and_creatures() -> Result<()> {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut vanilla = ResourceStack::open(&game)?;
        let mut dimension: Value = serde_json::from_slice(
            &vanilla
                .read_data("data/minecraft/dimension_type/overworld.json")?
                .context("dimension missing")?,
        )?;
        dimension["attributes"]["minecraft:gameplay/creature_world_gen_spawn_probability"] =
            json!(0.31);
        let clocks: BTreeMap<_, _> =
            native_resources::world_clock::registered_clocks(&mut vanilla)?
                .into_iter()
                .map(|id| (id, 0))
                .collect();
        let temp = tempfile::tempdir()?;
        let world = temp.path().join("world");
        let pack = world.join("datapacks/overrides");
        let dimension_dir = pack.join("data/minecraft/dimension_type");
        let tag_dir = pack.join("data/minecraft/tags/block");
        let feature_dir = pack.join("data/minecraft/worldgen/feature");
        let custom_dimension_dir = pack.join("data/example/dimension");
        std::fs::create_dir_all(&dimension_dir)?;
        std::fs::create_dir_all(&tag_dir)?;
        std::fs::create_dir_all(&feature_dir)?;
        std::fs::create_dir_all(&custom_dimension_dir)?;
        std::fs::write(
            pack.join("pack.mcmeta"),
            br#"{"pack":{"description":"test","min_format":121,"max_format":121}}"#,
        )?;
        std::fs::write(
            dimension_dir.join("overworld.json"),
            serde_json::to_vec(&dimension)?,
        )?;
        std::fs::write(
            tag_dir.join("blocks_motion_in_heightmap.json"),
            br#"{"replace":true,"values":["minecraft:grass_block"]}"#,
        )?;
        std::fs::write(
            feature_dir.join("bonus_chest.json"),
            br#"{"type":"minecraft:bonus_chest","config":{}}"#,
        )?;
        let preset: Value = serde_json::from_slice(
            &vanilla
                .read_data("data/minecraft/worldgen/world_preset/normal.json")?
                .context("normal preset missing")?,
        )?;
        let sky_stem = &preset["dimensions"]["minecraft:overworld"];
        std::fs::write(
            custom_dimension_dir.join("sky.json"),
            serde_json::to_vec(sky_stem)?,
        )?;
        let custom = prepare(FreshGenerationSettings {
            minecraft_dir: game.clone(),
            preset: "minecraft:normal".into(),
            dimension: "example:sky".into(),
            seed: 42,
            bonus_chest: false,
            structures_enabled: false,
            data_packs: Some((
                world.clone(),
                DataPackSettings {
                    enabled: vec!["vanilla".into(), "file/overrides".into()],
                    disabled: vec![],
                    enabled_features: vec!["minecraft:vanilla".into()],
                },
            )),
        })?;
        assert_eq!(custom.lighting.dimension, ResourceId::parse("example:sky")?);
        assert!(
            custom
                .worldgen_settings
                .value
                .get("data")
                .and_then(|v| v.get("dimensions"))
                .and_then(|v| v.get("example:sky"))
                .is_some()
        );
        let prepared = prepare(FreshGenerationSettings {
            minecraft_dir: game,
            preset: "minecraft:normal".into(),
            dimension: "minecraft:overworld".into(),
            seed: 42,
            bonus_chest: true,
            structures_enabled: false,
            data_packs: Some((
                world,
                DataPackSettings {
                    enabled: vec!["vanilla".into(), "file/overrides".into()],
                    disabled: vec![],
                    enabled_features: vec!["minecraft:vanilla".into()],
                },
            )),
        })?;
        // Later source edits must not change data read by generation workers.
        std::fs::write(dimension_dir.join("overworld.json"), b"{}")?;
        std::fs::write(
            feature_dir.join("bonus_chest.json"),
            br#"{"type":"minecraft:flower","config":{}}"#,
        )?;
        std::fs::write(
            tag_dir.join("blocks_motion_in_heightmap.json"),
            br#"{"replace":true,"values":["minecraft:stone"]}"#,
        )?;
        let border = native_session::respawn::RespawnBorder {
            min_x: -100.0,
            max_x: 100.0,
            min_z: -100.0,
            max_z: 100.0,
        };
        let mut inputs = prepared.into_initial_world_inputs(border, clocks, true, true)?;
        assert!(matches!(inputs.bonus_chest_feature.as_ref(), Some(Some(_))));
        let active: Value = serde_json::from_slice(
            &inputs
                .resources
                .read_data("data/minecraft/dimension_type/overworld.json")?
                .context("active dimension missing")?,
        )?;
        assert_eq!(active, dimension);
        assert_eq!(
            inputs
                .creatures
                .as_ref()
                .unwrap()
                .environment
                .dimension_attributes()["minecraft:gameplay/creature_world_gen_spawn_probability"],
            json!(0.31)
        );
        assert!(
            inputs
                .height_tags
                .for_block("minecraft:grass_block")?
                .blocks_motion
        );
        assert!(
            !inputs
                .height_tags
                .for_block("minecraft:stone")?
                .blocks_motion
        );
        assert_eq!(
            inputs
                .worldgen_settings
                .value
                .get("data")
                .and_then(|v| v.get("seed")),
            Some(&native_persistence::nbt::Tag::Long(42))
        );
        assert!(
            inputs
                .worldgen_settings
                .value
                .get("data")
                .and_then(|v| v.get("dimensions"))
                .and_then(|v| v.get("example:sky"))
                .is_some()
        );
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn inline_dimension_initializes_creatures_clocks_and_structure_environment() -> Result<()> {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut vanilla = ResourceStack::open(&game)?;
        let mut definition: Value = serde_json::from_slice(
            &vanilla
                .read_data("data/minecraft/dimension_type/overworld.json")?
                .context("overworld type missing")?,
        )?;
        definition["attributes"]["minecraft:gameplay/creature_world_gen_spawn_probability"] =
            json!(0.31);
        definition["attributes"]["minecraft:gameplay/fast_lava"] = json!(true);
        definition["attributes"]["minecraft:gameplay/water_evaporates"] = json!(true);
        definition["attributes"]["minecraft:visual/moon_phase"] = json!("new_moon");
        definition["timelines"] = json!(["example:inline"]);
        definition["default_clock"] = json!("minecraft:overworld");
        let mut preset: Value = serde_json::from_slice(
            &vanilla
                .read_data("data/minecraft/worldgen/world_preset/normal.json")?
                .context("normal preset missing")?,
        )?;
        preset["dimensions"]["minecraft:overworld"]["type"] = definition.clone();
        let temp = tempfile::tempdir()?;
        let world = temp.path().join("world");
        let pack = world.join("datapacks/inline");
        let preset_path = pack.join("data/example/worldgen/world_preset/inline.json");
        std::fs::create_dir_all(preset_path.parent().unwrap())?;
        std::fs::write(
            pack.join("pack.mcmeta"),
            br#"{"pack":{"description":"test","min_format":121,"max_format":121}}"#,
        )?;
        std::fs::write(preset_path, serde_json::to_vec(&preset)?)?;
        let timeline_path = pack.join("data/example/timeline/inline.json");
        std::fs::create_dir_all(timeline_path.parent().unwrap())?;
        let timeline = json!({"clock":"minecraft:overworld","period_ticks":20,"tracks":{
            "minecraft:visual/moon_phase":{"modifier":"override","keyframes":[{"ticks":0,"value":"new_moon"},{"ticks":10,"value":"full_moon"}]},
            "minecraft:gameplay/fast_lava":{"modifier":"xor","keyframes":[{"ticks":0,"value":false},{"ticks":10,"value":true}]},
            "minecraft:gameplay/water_evaporates":{"modifier":"xor","keyframes":[{"ticks":0,"value":false},{"ticks":10,"value":true}]},
            "minecraft:gameplay/creature_world_gen_spawn_probability":{"modifier":"add","keyframes":[{"ticks":0,"value":0.1}]}
        }});
        std::fs::write(timeline_path, serde_json::to_vec(&timeline)?)?;
        let selection = DataPackSettings {
            enabled: vec!["vanilla".into(), "file/inline".into()],
            disabled: vec![],
            enabled_features: vec!["minecraft:vanilla".into()],
        };
        let settings = |structures_enabled| FreshGenerationSettings {
            minecraft_dir: game.clone(),
            preset: "example:inline".into(),
            dimension: "minecraft:overworld".into(),
            seed: 42,
            bonus_chest: false,
            structures_enabled,
            data_packs: Some((world.clone(), selection.clone())),
        };
        let border = native_session::respawn::RespawnBorder {
            min_x: -100.0,
            max_x: 100.0,
            min_z: -100.0,
            max_z: 100.0,
        };
        let prepared = prepare(settings(false))?;
        assert_eq!(prepared.lighting.dimension_type, None);
        assert_eq!(prepared.dimension_definition, definition);
        let inputs = prepared.into_new_world_inputs(border, true, true)?;
        let creatures = inputs
            .creatures
            .as_ref()
            .context("creature inputs missing")?;
        assert!(!creatures.has_ceiling);
        assert_eq!(
            creatures.environment.dimension_attributes()["minecraft:gameplay/creature_world_gen_spawn_probability"],
            json!(0.31)
        );
        assert_eq!(creatures.environment.sample_layers(|_| Some(0))?.len(), 1);
        assert!(creatures.environment.sample_layers(|_| None).is_err());
        let initial = &inputs.initial_clocks;
        assert_eq!(
            initial
                .command_data
                .default_clock
                .as_ref()
                .map(|(name, id)| (name.as_ref(), id.clone())),
            Some((
                "[unregistered]",
                Some(ResourceId::parse("minecraft:overworld")?)
            ))
        );
        assert_eq!(
            initial
                .fluid_environment
                .fast_lava
                .as_ref()
                .context("fast lava environment missing")?
                .sample(|id| initial.ticks.get(id).copied()),
            Some(true)
        );
        assert_eq!(
            initial
                .fluid_environment
                .fast_lava
                .as_ref()
                .unwrap()
                .sample(|_| Some(10)),
            Some(false)
        );
        assert_eq!(
            initial
                .fluid_environment
                .water_evaporation
                .as_ref()
                .context("water evaporation environment missing")?
                .sample(|id| initial.ticks.get(id).copied()),
            Some(true)
        );
        assert_eq!(
            initial
                .fluid_environment
                .water_evaporation
                .as_ref()
                .unwrap()
                .sample(|_| Some(10)),
            Some(false)
        );
        let structured = prepare_with_structures_and_difficulty(
            settings(true),
            Some(native_persistence::world::Difficulty::Normal),
        )?;
        let hut = structured
            .hut_generation
            .as_ref()
            .context("structure environment missing")?;
        assert_eq!(
            hut.moon.sample(None, |_| Some(0)),
            Some(native_resources::moon_phase::Phase::NewMoon)
        );
        assert_eq!(
            hut.moon.sample(None, |_| Some(10)),
            Some(native_resources::moon_phase::Phase::FullMoon)
        );
        assert!(hut.ocean.is_some());
        Ok(())
    }

    #[test]
    fn structures_are_rejected_before_reading_files() {
        let result = prepare(FreshGenerationSettings {
            minecraft_dir: PathBuf::new(),
            preset: "minecraft:normal".into(),
            dimension: "minecraft:overworld".into(),
            seed: 1,
            bonus_chest: false,
            structures_enabled: true,
            data_packs: None,
        });
        assert!(result.err().unwrap().to_string().contains("structures"));
    }

    #[test]
    fn bonus_chest_requires_the_overworld() {
        let result = prepare(FreshGenerationSettings {
            minecraft_dir: PathBuf::new(),
            preset: "minecraft:normal".into(),
            dimension: "minecraft:the_nether".into(),
            seed: 1,
            bonus_chest: true,
            structures_enabled: false,
            data_packs: None,
        });
        assert!(result.err().unwrap().to_string().contains("overworld"));
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn spawn_worker_inputs_keep_same_preset_and_seed_draft() -> Result<()> {
        use native_persistence::nbt::Tag;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let prepared = prepare(FreshGenerationSettings {
            minecraft_dir: game,
            preset: "minecraft:normal".into(),
            dimension: "minecraft:overworld".into(),
            seed: -42,
            bonus_chest: false,
            structures_enabled: false,
            data_packs: None,
        })?;
        let (_, _, _, _, _, draft) = prepared.into_spawn_worker_inputs()?;
        let data = draft.value.get("data").context("worldgen data missing")?;
        assert_eq!(data.get("seed"), Some(&Tag::Long(-42)));
        assert_eq!(data.get("generate_structures"), Some(&Tag::Byte(0)));
        assert_eq!(data.get("bonus_chest"), Some(&Tag::Byte(0)));
        let dimensions = data.get("dimensions").context("dimensions missing")?;
        for name in [
            "minecraft:overworld",
            "minecraft:the_nether",
            "minecraft:the_end",
        ] {
            assert!(dimensions.get(name).is_some(), "missing {name}");
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn installed_normal_preset_prepares_consistent_inputs() -> Result<()> {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut prepared = prepare(FreshGenerationSettings {
            minecraft_dir: game,
            preset: "minecraft:normal".into(),
            dimension: "minecraft:overworld".into(),
            seed: 42,
            bonus_chest: false,
            structures_enabled: false,
            data_packs: None,
        })
        .context("cannot prepare vanilla overworld")?;
        assert_eq!(
            (prepared.lighting.min_y, prepared.lighting.height),
            (-64, 384)
        );
        let origin = prepared.spawn_origin()?;
        assert!((origin.x.abs() <= 160) && (origin.z.abs() <= 160));
        let (mut batch, mut resources, mut classify, mut resolve) = prepared.into_worker_inputs();
        assert!(classify(&json!("minecraft:air"))?.is_air);
        assert!(classify(&json!("minecraft:water"))?.has_fluid);
        assert!(!classify(&json!("minecraft:stone"))?.has_fluid);
        assert_eq!(resolve(&json!("minecraft:stone"))?.name, "minecraft:stone");
        batch.request(native_world::loaded::ChunkPos { x: 0, z: 0 })?;
        loop {
            let (_, stage) = batch
                .step(&mut resources, &mut classify, &mut resolve)?
                .context("generation completed without terrain")?;
            if stage == native_session::worldgen::schedule::GenerationStage::Terrain {
                break;
            }
        }
        assert_eq!(
            batch
                .snapshot()
                .context("terrain is missing")?
                .dimension_type
                .as_deref(),
            Some("minecraft:overworld")
        );
        Ok(())
    }
}
