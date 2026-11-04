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
};
use native_world::{fluid::FluidState, loaded::BlockState};
use serde_json::Value;

pub struct FreshGenerationSettings {
    pub minecraft_dir: PathBuf,
    pub preset: String,
    pub dimension: String,
    pub seed: i64,
    pub bonus_chest: bool,
    /// Structures and old-world blending are not supported by this batch.
    pub structures_enabled: bool,
    pub data_packs: Option<(PathBuf, DataPackSettings)>,
}

type Classifier = Box<dyn FnMut(&Value) -> Result<CarverBlockState> + Send>;
type Resolver = Box<dyn FnMut(&Value) -> Result<BlockState> + Send>;

pub struct PreparedFreshGeneration {
    pub lighting: GeneratedDimensionLighting,
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
    pub command_data: InitialClockCommandData,
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
    fn load(resources: &mut ResourceStack, lighting: &GeneratedDimensionLighting) -> Self {
        let default_clock = lighting.dimension_type.as_ref().and_then(|id| {
            let name = format!("{}:{}", id.namespace, id.path);
            match native_resources::world_clock::dimension_default_clock(resources, &name) {
                Ok(clock) => Some((Arc::from(name), clock)),
                Err(error) => {
                    eprintln!("Dimension clock is unavailable: {error:#}");
                    None
                }
            }
        });
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
    clocks: BTreeMap<ResourceId, i64>,
    has_ceiling: bool,
}

impl PreparedCreatureSpawning {
    /// Load on the worker with the already-configured world data packs.
    /// Inline dimension types and unsupported spawn conditions remain errors.
    pub fn load(
        resources: &mut ResourceStack,
        lighting: &GeneratedDimensionLighting,
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
        let dimension_type = lighting
            .dimension_type
            .as_ref()
            .context("inline dimension types are not yet supported for creature generation")?;
        let path = format!(
            "data/{}/dimension_type/{}.json",
            dimension_type.namespace, dimension_type.path
        );
        let bytes = resources
            .read_data(&path)?
            .with_context(|| format!("missing dimension type {path}"))?;
        let definition: Value = serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid dimension type {path}"))?;
        let has_ceiling = definition
            .get("has_ceiling")
            .and_then(Value::as_bool)
            .context("dimension has_ceiling is missing or invalid")?;
        let categories = resources.vanilla_entity_category_registry()?;
        let name = format!("{}:{}", dimension_type.namespace, dimension_type.path);
        let environment =
            native_resources::creature_timeline::CreatureSpawnEnvironment::load_with_registry(
                resources,
                &name,
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
        Ok(Self {
            environment,
            categories,
            prevent_inside,
            height_tags,
            ground_tags,
            biome_tags,
            variants,
            border,
            clocks,
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
        Ok(driver)
    }
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
            command_data: InitialClockCommandData::load(&mut self.resources, &self.lighting),
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
                &self.lighting,
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
        Ok(PreparedInitialWorld {
            batch: self.batch,
            resources: self.resources,
            classify: self.classify,
            resolve: self.resolve,
            height_tags,
            worldgen_settings: self.worldgen_settings,
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
pub fn prepare(settings: FreshGenerationSettings) -> Result<PreparedFreshGeneration> {
    ensure!(
        !settings.structures_enabled,
        "fresh generation with structures is not implemented"
    );
    ensure!(
        !settings.bonus_chest,
        "bonus chest generation is not implemented"
    );
    let mut resources = ResourceStack::open(&settings.minecraft_dir)?;
    let pack_selection = settings
        .data_packs
        .as_ref()
        .map(|(_, selection)| selection.clone());
    if let Some((world_dir, selection)) = settings.data_packs {
        let warnings = resources.configure_world_data(&world_dir, &selection)?;
        ensure!(
            warnings.is_empty(),
            "data pack selection needs attention: {}",
            warnings.join("; ")
        );
    }
    let worldgen_settings = native_persistence::worldgen_settings::draft_worldgen_settings(
        &mut resources,
        &settings.preset,
        native_persistence::worldgen_settings::NewWorldOptions {
            seed: settings.seed,
            generate_structures: settings.structures_enabled,
            bonus_chest: settings.bonus_chest,
        },
    )?;
    let (lighting, definition) = GeneratedDimensionLighting::load_preset_with_generator(
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
        worldgen_settings,
        pack_selection,
        batch,
        resources,
        classify,
        resolve,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;
    use serde_json::json;

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
            let lighting = GeneratedDimensionLighting::load_preset(
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
            let inputs =
                PreparedCreatureSpawning::load(&mut resources, &lighting, border, clocks.clone())?;
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
        std::fs::create_dir_all(&dimension_dir)?;
        std::fs::create_dir_all(&tag_dir)?;
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
        let prepared = prepare(FreshGenerationSettings {
            minecraft_dir: game,
            preset: "minecraft:normal".into(),
            dimension: "minecraft:overworld".into(),
            seed: 42,
            bonus_chest: false,
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
        let border = native_session::respawn::RespawnBorder {
            min_x: -100.0,
            max_x: 100.0,
            min_z: -100.0,
            max_z: 100.0,
        };
        let mut inputs = prepared.into_initial_world_inputs(border, clocks, true, true)?;
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
    fn bonus_chest_is_rejected_before_reading_files() {
        let result = prepare(FreshGenerationSettings {
            minecraft_dir: PathBuf::new(),
            preset: "minecraft:normal".into(),
            dimension: "minecraft:overworld".into(),
            seed: 1,
            bonus_chest: true,
            structures_enabled: false,
            data_packs: None,
        });
        assert!(result.err().unwrap().to_string().contains("bonus chest"));
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
