//! Compile the density router from the active 26.3 noise-settings data pack.
//!
//! This exposes point and unit-step block-volume sampling. It does not generate chunks, fluids,
//! surfaces or structures, and assumes no older-chunk or structure adjustment.
use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    density::{DensityCompiler, DensityEvaluator, DensityFunction, WorldgenRandom},
    packs::{ResourceId, ResourceProvider},
};

#[derive(Clone, Copy, Debug)]
pub enum RouterField {
    FinalDensity,
    Temperature,
    Vegetation,
    Continents,
    Erosion,
    Depth,
    Ridges,
    ChunkSurfaceLevel,
}

impl RouterField {
    pub const ALL: [Self; 8] = [
        Self::FinalDensity,
        Self::Temperature,
        Self::Vegetation,
        Self::Continents,
        Self::Erosion,
        Self::Depth,
        Self::Ridges,
        Self::ChunkSurfaceLevel,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::FinalDensity => "final_density",
            Self::Temperature => "temperature",
            Self::Vegetation => "vegetation",
            Self::Continents => "continents",
            Self::Erosion => "erosion",
            Self::Depth => "depth",
            Self::Ridges => "ridges",
            Self::ChunkSurfaceLevel => "chunk_surface_level",
        }
    }
}

#[derive(Deserialize)]
struct VerticalNoise {
    min_y: i32,
    height: i32,
}
#[derive(Deserialize)]
struct Definition {
    noise: VerticalNoise,
    legacy_random_source: bool,
    noise_router: serde_json::Map<String, Value>,
}

/// Density values before aquifers, blocks, biomes, surfaces and structures.
pub struct ChunkDensity {
    chunk_x: i32,
    chunk_z: i32,
    min_y: i32,
    height: i32,
    values: Vec<f32>,
}

impl ChunkDensity {
    pub fn chunk_position(&self) -> [i32; 2] {
        [self.chunk_x, self.chunk_z]
    }
    pub fn min_y(&self) -> i32 {
        self.min_y
    }
    pub fn height(&self) -> i32 {
        self.height
    }
    /// Density buffer in Java's Z, X, Y order.
    pub fn values(&self) -> &[f32] {
        &self.values
    }

    pub fn get(&self, local_x: u8, y: i32, local_z: u8) -> Option<f32> {
        if local_x >= 16 || local_z >= 16 || y < self.min_y || y >= self.min_y + self.height {
            return None;
        }
        let index = ((usize::from(local_z) * 16 + usize::from(local_x)) * self.height as usize)
            + (y - self.min_y) as usize;
        self.values.get(index).copied()
    }
}

/// A density-sampling view of one active noise-settings definition.
pub struct NoiseRouter {
    pub min_y: i32,
    pub height: i32,
    pub random: WorldgenRandom,
    functions: [Arc<DensityFunction>; 8],
}

impl NoiseRouter {
    /// Loads a registered noise setting using the active data-pack provider.
    /// Call only when older-chunk blending and structure terrain effects are absent.
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

    /// Compiles an inline or registered setting, with the same explicit context.
    pub fn from_json_without_terrain_adjustments(
        provider: &mut impl ResourceProvider,
        bytes: &[u8],
        seed: i64,
    ) -> Result<Self> {
        ensure!(
            bytes.len() <= 16 * 1024 * 1024,
            "noise settings are too large"
        );
        let definition: Definition =
            serde_json::from_slice(bytes).context("invalid noise settings JSON")?;
        let VerticalNoise { min_y, height } = definition.noise;
        ensure!(
            (-2032..=2031).contains(&min_y) && (0..=4064).contains(&height),
            "noise height is out of range"
        );
        ensure!(
            min_y % 16 == 0 && height % 16 == 0 && min_y + height <= 2032,
            "invalid noise vertical range"
        );
        let random = if definition.legacy_random_source {
            WorldgenRandom::Legacy
        } else {
            WorldgenRandom::Xoroshiro
        };
        let mut compiler = DensityCompiler::with_world_seed(provider, seed, random)
            .without_old_chunk_blending()
            .without_structure_terrain_adjustment();
        let functions = RouterField::ALL
            .into_iter()
            .map(|field| {
                let value = definition
                    .noise_router
                    .get(field.name())
                    .with_context(|| format!("missing noise router field {}", field.name()))?;
                compiler
                    .compile(value)
                    .with_context(|| format!("in noise router field {}", field.name()))
            })
            .collect::<Result<Vec<_>>>()?
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid noise router field count"))?;
        Ok(Self {
            min_y,
            height,
            random,
            functions,
        })
    }

    pub fn function(&self, field: RouterField) -> &Arc<DensityFunction> {
        &self.functions[field as usize]
    }

    pub fn sample(
        &self,
        evaluator: &mut DensityEvaluator,
        field: RouterField,
        x: i32,
        y: i32,
        z: i32,
    ) -> f32 {
        evaluator.sample(self.function(field), x, y, z)
    }

    /// Evaluate every router field at one position, reusing shared graph caches.
    pub fn sample_all(&self, evaluator: &mut DensityEvaluator, x: i32, y: i32, z: i32) -> [f32; 8] {
        evaluator.sample_array(&self.functions, x, y, z)
    }

    /// Sample a unit-step block volume in Z, X, Y order.
    pub fn sample_block_volume(
        &self,
        evaluator: &mut DensityEvaluator,
        field: RouterField,
        min: [i32; 3],
        size: [u32; 3],
    ) -> Result<Vec<f32>> {
        evaluator.sample_block_volume(self.function(field), min, size)
    }

    /// Sample a whole chunk's noise-stage density without modifying a world.
    pub fn sample_chunk_density(
        &self,
        evaluator: &mut DensityEvaluator,
        chunk_x: i32,
        chunk_z: i32,
    ) -> Result<ChunkDensity> {
        ensure!(
            (-2032..=2031).contains(&self.min_y)
                && (0..=4064).contains(&self.height)
                && self.min_y % 16 == 0
                && self.height % 16 == 0
                && self.min_y + self.height <= 2032,
            "invalid noise vertical range"
        );
        let x = chunk_x
            .checked_mul(16)
            .context("chunk X exceeds block-coordinate range")?;
        let z = chunk_z
            .checked_mul(16)
            .context("chunk Z exceeds block-coordinate range")?;
        let values = if self.height == 0 {
            Vec::new()
        } else {
            self.sample_block_volume(
                evaluator,
                RouterField::FinalDensity,
                [x, self.min_y, z],
                [16, self.height as u32, 16],
            )?
        };
        Ok(ChunkDensity {
            chunk_x,
            chunk_z,
            min_y: self.min_y,
            height: self.height,
            values,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Data {
        settings: Vec<u8>,
        density: Option<Vec<u8>>,
    }
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(match path {
                "data/example/worldgen/noise_settings/custom.json" => Some(self.settings.clone()),
                "data/example/worldgen/density_function/custom.json" => self.density.clone(),
                _ => None,
            })
        }
    }
    fn definition() -> Value {
        json!({"noise":{"min_y":-64,"height":384},"legacy_random_source":true,"noise_router":{
            "final_density":1.0,"temperature":2.0,"vegetation":3.0,"continents":4.0,
            "erosion":5.0,"depth":6.0,"ridges":7.0,"chunk_surface_level":8.0
        }})
    }
    #[test]
    fn loads_active_settings_and_checks_vertical_and_router_fields() {
        let mut data = Data {
            settings: serde_json::to_vec(&definition()).unwrap(),
            density: None,
        };
        let router =
            NoiseRouter::load_without_terrain_adjustments(&mut data, "example:custom", 7).unwrap();
        assert_eq!(
            (router.min_y, router.height, router.random),
            (-64, 384, WorldgenRandom::Legacy)
        );
        let mut evaluator = DensityEvaluator::default();
        assert_eq!(
            router.sample_all(&mut evaluator, 4, 5, 6),
            [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]
        );
        assert_eq!(
            router
                .sample_block_volume(
                    &mut evaluator,
                    RouterField::FinalDensity,
                    [-1, -1, -1],
                    [2, 2, 2]
                )
                .unwrap(),
            vec![1.0; 8]
        );
        let chunk = router.sample_chunk_density(&mut evaluator, -2, 3).unwrap();
        assert_eq!(
            (chunk.chunk_x, chunk.chunk_z, chunk.min_y, chunk.height),
            (-2, 3, -64, 384)
        );
        assert_eq!(chunk.values().len(), 16 * 16 * 384);
        assert_eq!(chunk.get(15, -64, 0), Some(1.0));
        assert_eq!(chunk.get(0, 320, 0), None);
        assert_eq!(chunk.get(16, 0, 0), None);
        assert!(
            router
                .sample_chunk_density(&mut evaluator, i32::MAX, 0)
                .is_err()
        );
        let mut zero_height = definition();
        zero_height["noise"]["height"] = json!(0);
        let empty = NoiseRouter::from_json_without_terrain_adjustments(
            &mut data,
            &serde_json::to_vec(&zero_height).unwrap(),
            7,
        )
        .unwrap()
        .sample_chunk_density(&mut evaluator, 0, 0)
        .unwrap();
        assert!(empty.values().is_empty());
        assert_eq!(empty.get(0, -64, 0), None);
        for (i, field) in RouterField::ALL.into_iter().enumerate() {
            assert_eq!(
                router.sample(&mut evaluator, field, 4, 5, 6),
                (i + 1) as f32
            );
        }
        assert!(
            NoiseRouter::load_without_terrain_adjustments(&mut data, "example:missing", 7).is_err()
        );
        let mut overridden = definition();
        overridden["noise_router"]["final_density"] = json!("example:custom");
        data.settings = serde_json::to_vec(&overridden).unwrap();
        data.density = Some(b"4.0".to_vec());
        let first =
            NoiseRouter::load_without_terrain_adjustments(&mut data, "example:custom", 7).unwrap();
        assert_eq!(
            first.sample(&mut evaluator, RouterField::FinalDensity, 0, 0, 0),
            4.0
        );
        data.density = Some(b"9.0".to_vec());
        let second =
            NoiseRouter::load_without_terrain_adjustments(&mut data, "example:custom", 7).unwrap();
        assert_eq!(
            second.sample(&mut evaluator, RouterField::FinalDensity, 0, 0, 0),
            9.0
        );
        let mut bad = definition();
        bad["noise"]["height"] = json!(17);
        assert!(
            NoiseRouter::from_json_without_terrain_adjustments(
                &mut data,
                &serde_json::to_vec(&bad).unwrap(),
                7
            )
            .is_err()
        );
        bad = definition();
        bad["noise_router"].as_object_mut().unwrap().remove("depth");
        assert!(
            NoiseRouter::from_json_without_terrain_adjustments(
                &mut data,
                &serde_json::to_vec(&bad).unwrap(),
                7
            )
            .is_err()
        );
    }
}
