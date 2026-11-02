//! Prepare generation inputs from a verified client and selected data packs.
use std::{path::PathBuf, sync::Arc};

use anyhow::{Result, ensure};
use native_resources::{
    carver::tags::CarverBlockTags,
    datapacks::DataPackSettings,
    dimension_lighting::GeneratedDimensionLighting,
    packs::{ResourceProvider, ResourceStack},
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
    /// Structures and old-world blending are not supported by this batch.
    pub structures_enabled: bool,
    pub data_packs: Option<(PathBuf, DataPackSettings)>,
}

type Classifier = Box<dyn FnMut(&Value) -> Result<CarverBlockState> + Send>;
type Resolver = Box<dyn FnMut(&Value) -> Result<BlockState> + Send>;

pub struct PreparedFreshGeneration {
    pub lighting: GeneratedDimensionLighting,
    batch: FreshTerrainBatch,
    resources: ResourceStack,
    classify: Classifier,
    resolve: Resolver,
}

impl PreparedFreshGeneration {
    pub fn into_worker_inputs(self) -> (FreshTerrainBatch, ResourceStack, Classifier, Resolver) {
        (self.batch, self.resources, self.classify, self.resolve)
    }
}

/// Call on the generation worker, not the window thread. Warnings from the
/// chosen pack stack are errors until the UI can present and resolve them.
pub fn prepare(settings: FreshGenerationSettings) -> Result<PreparedFreshGeneration> {
    ensure!(
        !settings.structures_enabled,
        "fresh generation with structures is not implemented"
    );
    let mut resources = ResourceStack::open(&settings.minecraft_dir)?;
    if let Some((world_dir, selection)) = settings.data_packs {
        let warnings = resources.configure_world_data(&world_dir, &selection)?;
        ensure!(
            warnings.is_empty(),
            "data pack selection needs attention: {}",
            warnings.join("; ")
        );
    }
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
    fn structures_are_rejected_before_reading_files() {
        let result = prepare(FreshGenerationSettings {
            minecraft_dir: PathBuf::new(),
            preset: "minecraft:normal".into(),
            dimension: "minecraft:overworld".into(),
            seed: 1,
            structures_enabled: true,
            data_packs: None,
        });
        assert!(result.err().unwrap().to_string().contains("structures"));
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn installed_normal_preset_prepares_consistent_inputs() -> Result<()> {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let prepared = prepare(FreshGenerationSettings {
            minecraft_dir: game,
            preset: "minecraft:normal".into(),
            dimension: "minecraft:overworld".into(),
            seed: 42,
            structures_enabled: false,
            data_packs: None,
        })
        .context("cannot prepare vanilla overworld")?;
        assert_eq!(
            (prepared.lighting.min_y, prepared.lighting.height),
            (-64, 384)
        );
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
