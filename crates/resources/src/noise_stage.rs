//! Noise-stage material selection when the generator has no aquifer config.
//! This is not a complete chunk: surface rules, biomes and features are separate.
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    density::DensityEvaluator,
    noise_router::NoiseRouter,
    packs::{ResourceId, ResourceProvider},
};

/// The material selected before surface rules and feature placement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoiseMaterial {
    DefaultBlock,
    DefaultFluid,
    Lava,
    Air,
}

/// Selects vanilla's global fluid status when no local aquifer is configured.
pub fn select_without_aquifer(density: f32, y: i32, sea_level: i32) -> NoiseMaterial {
    if density > 0.0 {
        return NoiseMaterial::DefaultBlock;
    }
    if y < sea_level.min(-54) {
        NoiseMaterial::Lava
    } else if y < sea_level {
        NoiseMaterial::DefaultFluid
    } else {
        NoiseMaterial::Air
    }
}

#[derive(Deserialize)]
struct Materials {
    default_block: Value,
    default_fluid: Value,
    sea_level: i32,
    #[serde(default)]
    aquifers: Option<Value>,
}

fn validate_state_encoding(value: &Value) -> Result<()> {
    let name = if let Some(name) = value.as_str() {
        name
    } else {
        let object = value
            .as_object()
            .context("block state must be a string or object")?;
        // Properties are resolved against the block registry later. Java's
        // block-state codec treats this field leniently, so preserve it here.
        object
            .get("id")
            .and_then(Value::as_str)
            .context("block state has no identifier")?
    };
    ResourceId::parse(name)?;
    Ok(())
}

/// An intermediate noise-stage result. It must not be persisted as a full chunk.
pub struct NoiseStageChunk {
    chunk_x: i32,
    chunk_z: i32,
    min_y: i32,
    height: i32,
    default_block: Value,
    default_fluid: Value,
    materials: Vec<NoiseMaterial>,
}

impl NoiseStageChunk {
    pub fn chunk_position(&self) -> [i32; 2] {
        [self.chunk_x, self.chunk_z]
    }
    pub fn min_y(&self) -> i32 {
        self.min_y
    }
    pub fn height(&self) -> i32 {
        self.height
    }
    /// Original data-pack encoding; resolve against the block registry before use.
    pub fn default_block(&self) -> &Value {
        &self.default_block
    }
    pub fn default_fluid(&self) -> &Value {
        &self.default_fluid
    }
    /// Java's Z, X, Y buffer order.
    pub fn materials(&self) -> &[NoiseMaterial] {
        &self.materials
    }
    pub fn get(&self, x: u8, y: i32, z: u8) -> Option<NoiseMaterial> {
        if x >= 16 || z >= 16 || y < self.min_y || y >= self.min_y + self.height {
            return None;
        }
        self.materials
            .get(
                (usize::from(z) * 16 + usize::from(x)) * self.height as usize
                    + (y - self.min_y) as usize,
            )
            .copied()
    }
}

pub struct NoiseStageGenerator {
    router: NoiseRouter,
    default_block: Value,
    default_fluid: Value,
    sea_level: i32,
}

impl NoiseStageGenerator {
    /// Load only if older-chunk blending and structure terrain adjustments are absent.
    /// A configured local aquifer is rejected instead of substituting global fluids.
    pub fn load_without_terrain_adjustments(
        provider: &mut impl ResourceProvider,
        name: &str,
        seed: i64,
    ) -> Result<Self> {
        let id = ResourceId::parse(name)?;
        let path = format!(
            "data/{}/worldgen/noise_settings/{}.json",
            id.namespace, id.path
        );
        let bytes = provider
            .read_data(&path)?
            .with_context(|| format!("missing noise settings {name}"))?;
        Self::from_json_without_terrain_adjustments(provider, &bytes, seed)
            .with_context(|| format!("in noise settings {name}"))
    }

    pub fn from_json_without_terrain_adjustments(
        provider: &mut impl ResourceProvider,
        bytes: &[u8],
        seed: i64,
    ) -> Result<Self> {
        ensure!(
            bytes.len() <= 16 * 1024 * 1024,
            "noise settings are too large"
        );
        let materials: Materials =
            serde_json::from_slice(bytes).context("invalid noise material settings")?;
        ensure!(
            materials.aquifers.is_none(),
            "local aquifer generation is not implemented"
        );
        validate_state_encoding(&materials.default_block).context("invalid default block")?;
        validate_state_encoding(&materials.default_fluid).context("invalid default fluid")?;
        let router = NoiseRouter::from_json_without_terrain_adjustments(provider, bytes, seed)?;
        Ok(Self {
            router,
            default_block: materials.default_block,
            default_fluid: materials.default_fluid,
            sea_level: materials.sea_level,
        })
    }

    pub fn generate(
        &self,
        evaluator: &mut DensityEvaluator,
        x: i32,
        z: i32,
    ) -> Result<NoiseStageChunk> {
        let density = self.router.sample_chunk_density(evaluator, x, z)?;
        let min_y = density.min_y();
        let height = density.height();
        let materials = if height == 0 {
            Vec::new()
        } else {
            density
                .values()
                .iter()
                .enumerate()
                .map(|(i, value)| {
                    select_without_aquifer(
                        *value,
                        min_y + (i % height as usize) as i32,
                        self.sea_level,
                    )
                })
                .collect()
        };
        Ok(NoiseStageChunk {
            chunk_x: x,
            chunk_z: z,
            min_y,
            height,
            default_block: self.default_block.clone(),
            default_fluid: self.default_fluid.clone(),
            materials,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    struct Data;
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
    }
    fn definition() -> Value {
        json!({"noise":{"min_y":0,"height":16},"legacy_random_source":true,
            "default_block":{"id":"minecraft:stone","properties":{"example":"value"}},"default_fluid":"minecraft:water","sea_level":8,
            "noise_router":{"final_density":-1,"temperature":0,"vegetation":0,"continents":0,"erosion":0,"depth":0,"ridges":0,"chunk_surface_level":0}})
    }
    #[test]
    fn global_fluid_edges_and_unimplemented_aquifers_are_explicit() {
        assert_eq!(
            select_without_aquifer(f32::NAN, 0, 32),
            NoiseMaterial::DefaultFluid
        );
        assert_eq!(select_without_aquifer(0.0, -55, 32), NoiseMaterial::Lava);
        assert_eq!(
            select_without_aquifer(-0.0, -54, 32),
            NoiseMaterial::DefaultFluid
        );
        assert_eq!(select_without_aquifer(-1.0, 32, 32), NoiseMaterial::Air);
        assert_eq!(select_without_aquifer(-1.0, -90, -100), NoiseMaterial::Air);
        assert_eq!(
            select_without_aquifer(-1.0, -101, -100),
            NoiseMaterial::Lava
        );
        let mut data = Data;
        let mut evaluator = DensityEvaluator::default();
        let generator = NoiseStageGenerator::from_json_without_terrain_adjustments(
            &mut data,
            &serde_json::to_vec(&definition()).unwrap(),
            0,
        )
        .unwrap();
        let chunk = generator.generate(&mut evaluator, -1, 2).unwrap();
        assert_eq!(chunk.get(0, 7, 0), Some(NoiseMaterial::DefaultFluid));
        assert_eq!(chunk.get(0, 8, 0), Some(NoiseMaterial::Air));
        assert_eq!(chunk.get(16, 8, 0), None);
        assert_eq!(chunk.default_block, definition()["default_block"]);
        let mut aquifer = definition();
        aquifer["aquifers"] = json!({});
        assert!(
            NoiseStageGenerator::from_json_without_terrain_adjustments(
                &mut data,
                &serde_json::to_vec(&aquifer).unwrap(),
                0
            )
            .is_err()
        );
        let mut malformed = definition();
        malformed["default_block"] = json!({"properties": {"axis":"x"}});
        assert!(
            NoiseStageGenerator::from_json_without_terrain_adjustments(
                &mut data,
                &serde_json::to_vec(&malformed).unwrap(),
                0
            )
            .is_err()
        );
    }
}
