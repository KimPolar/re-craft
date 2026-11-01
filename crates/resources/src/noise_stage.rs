//! Noise-stage material selection with optional local aquifers.
//! This is not a complete chunk: surface rules, biomes and features are separate.
use anyhow::{Context, Result, ensure};
use native_core::aquifer::{
    AquiferBlock, AquiferGenerator, AquiferSampler, AquiferSurfaceArea, FluidStatus,
};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

use crate::{
    density::{DensityCompiler, DensityEvaluator, DensityFunction, WorldgenRandom},
    noise_router::{NoiseRouter, RouterField},
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
    generation_min_y: i32,
    generation_height: i32,
    default_block: Value,
    default_fluid: Value,
    materials: Vec<NoiseMaterial>,
    fluid_updates: Vec<bool>,
}

impl NoiseStageChunk {
    /// Expand a noise volume inside a larger generation-height range. Areas
    /// outside the noise volume start as air, just as an empty chunk does.
    /// Clipping is rejected because it changes interpolation-volume semantics.
    pub fn expand_to_dimension(self, min_y: i32, height: i32) -> Result<Self> {
        ensure!(
            height > 0 && min_y % 16 == 0 && height % 16 == 0,
            "invalid dimension height range"
        );
        let end = min_y
            .checked_add(height)
            .context("dimension height overflow")?;
        let noise_end = self
            .min_y
            .checked_add(self.height)
            .context("noise height overflow")?;
        ensure!(
            self.height > 0 && min_y <= self.min_y && end >= noise_end,
            "dimension clips the noise volume"
        );
        if min_y == self.min_y && height == self.height {
            return Ok(self);
        }
        let count = usize::try_from(height)?
            .checked_mul(256)
            .context("dimension volume size overflow")?;
        ensure!(
            count <= 1_048_576,
            "dimension volume exceeds maximum sample count"
        );
        let mut materials = vec![NoiseMaterial::Air; count];
        let mut fluid_updates = vec![false; count];
        let source_height = self.height as usize;
        let target_height = height as usize;
        let offset = (self.min_y - min_y) as usize;
        for column in 0..256 {
            let source = column * source_height..(column + 1) * source_height;
            let target =
                column * target_height + offset..column * target_height + offset + source_height;
            materials[target.clone()].copy_from_slice(&self.materials[source.clone()]);
            fluid_updates[target].copy_from_slice(&self.fluid_updates[source]);
        }
        Ok(Self {
            chunk_x: self.chunk_x,
            chunk_z: self.chunk_z,
            min_y,
            height,
            generation_min_y: self.generation_min_y,
            generation_height: self.generation_height,
            default_block: self.default_block,
            default_fluid: self.default_fluid,
            materials,
            fluid_updates,
        })
    }
    pub fn chunk_position(&self) -> [i32; 2] {
        [self.chunk_x, self.chunk_z]
    }
    pub fn min_y(&self) -> i32 {
        self.min_y
    }
    pub fn height(&self) -> i32 {
        self.height
    }
    /// Bounds of the generator's WorldGenerationContext. These remain the
    /// noise bounds when the chunk is expanded to a taller dimension.
    pub fn generation_min_y(&self) -> i32 {
        self.generation_min_y
    }
    pub fn generation_height(&self) -> i32 {
        self.generation_height
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
    /// Aquifer update flags in the same order as materials.
    pub fn fluid_updates(&self) -> &[bool] {
        &self.fluid_updates
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
    seed: i64,
    aquifer: Option<AquiferFunctions>,
    default_aquifer_fluid: AquiferMaterial,
}

struct AquiferFunctions {
    barrier: Arc<DensityFunction>,
    floodedness: Arc<DensityFunction>,
    spread: Arc<DensityFunction>,
    lava: Arc<DensityFunction>,
    exclusion: Arc<DensityFunction>,
    surface: Arc<DensityFunction>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AquiferMaterial {
    Air,
    Lava,
    DefaultWater,
    DefaultLava,
    DefaultAir,
    DefaultOther,
}
impl AquiferBlock for AquiferMaterial {
    fn air() -> Self {
        Self::Air
    }
    fn lava() -> Self {
        Self::Lava
    }
    fn is_air(self) -> bool {
        matches!(self, Self::Air | Self::DefaultAir)
    }
    fn is_water(self) -> bool {
        self == Self::DefaultWater
    }
    fn is_lava(self) -> bool {
        matches!(self, Self::Lava | Self::DefaultLava)
    }
}
struct NoiseAquiferSampler<'a> {
    functions: &'a AquiferFunctions,
    evaluator: &'a mut DensityEvaluator,
    sea: i32,
    fluid: AquiferMaterial,
}

/// A target-chunk aquifer for the post-surface carver pass. A fresh instance
/// uses the same seeded centers and density inputs as the noise stage.
pub struct CarvingAquifer<'a> {
    inner: Option<(AquiferGenerator<AquiferMaterial>, NoiseAquiferSampler<'a>)>,
    sea_level: i32,
    chunk: [i32; 2],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CarvingAquiferResult {
    /// None means Java's aquifer declined to replace this block.
    pub material: Option<NoiseMaterial>,
    pub schedule_fluid_update: bool,
}
impl CarvingAquifer<'_> {
    pub fn sample(&mut self, x: i32, y: i32, z: i32) -> Result<CarvingAquiferResult> {
        ensure!(
            [x.div_euclid(16), z.div_euclid(16)] == self.chunk,
            "aquifer sample outside target chunk"
        );
        if let Some((aquifer, sampler)) = &mut self.inner {
            let result = aquifer.sample(sampler, x, y, z, 0.0);
            let material = result.block.map(|block| match block {
                AquiferMaterial::Air => NoiseMaterial::Air,
                AquiferMaterial::Lava => NoiseMaterial::Lava,
                AquiferMaterial::DefaultWater
                | AquiferMaterial::DefaultLava
                | AquiferMaterial::DefaultAir
                | AquiferMaterial::DefaultOther => NoiseMaterial::DefaultFluid,
            });
            Ok(CarvingAquiferResult {
                material,
                schedule_fluid_update: result.schedule_fluid_update,
            })
        } else {
            Ok(CarvingAquiferResult {
                material: Some(select_without_aquifer(0.0, y, self.sea_level)),
                schedule_fluid_update: false,
            })
        }
    }
}
impl AquiferSampler for NoiseAquiferSampler<'_> {
    type Block = AquiferMaterial;
    fn global(&mut self, _: i32, y: i32, _: i32) -> FluidStatus<Self::Block> {
        if y < self.sea.min(-54) {
            FluidStatus {
                level: -54,
                block: AquiferMaterial::Lava,
            }
        } else {
            FluidStatus {
                level: self.sea,
                block: self.fluid,
            }
        }
    }
    fn barrier(&mut self, x: i32, y: i32, z: i32) -> f32 {
        self.evaluator.sample(&self.functions.barrier, x, y, z)
    }
    fn surface(&mut self, x: i32, z: i32) -> f32 {
        self.evaluator.sample(&self.functions.surface, x, 0, z)
    }
    fn exclusion(&mut self, x: i32, y: i32, z: i32) -> f32 {
        self.evaluator.sample(&self.functions.exclusion, x, y, z)
    }
    fn floodedness(&mut self, x: i32, y: i32, z: i32) -> f32 {
        self.evaluator.sample(&self.functions.floodedness, x, y, z)
    }
    fn spread(&mut self, x: i32, y: i32, z: i32) -> f32 {
        self.evaluator.sample(&self.functions.spread, x, y, z)
    }
    fn lava(&mut self, x: i32, y: i32, z: i32) -> f32 {
        self.evaluator.sample(&self.functions.lava, x, y, z)
    }
}

impl NoiseStageGenerator {
    pub fn default_fluid(&self) -> &Value {
        &self.default_fluid
    }
    pub fn carving_aquifer<'a>(
        &'a self,
        evaluator: &'a mut DensityEvaluator,
        chunk: [i32; 2],
    ) -> Result<CarvingAquifer<'a>> {
        let inner = if let Some(functions) = &self.aquifer {
            let area = AquiferSurfaceArea::for_chunk(chunk[0], chunk[1])
                .context("chunk aquifer surface coordinates overflow")?;
            let surface = evaluator.sample_quart_surface(
                &functions.surface,
                area.min_x,
                area.min_z,
                area.size_x,
                area.size_z,
            )?;
            let generator = AquiferGenerator::from_surface_volume(
                self.seed,
                self.router.random == WorldgenRandom::Legacy,
                area,
                &surface,
            )
            .map_err(anyhow::Error::msg)?;
            Some((
                generator,
                NoiseAquiferSampler {
                    functions,
                    evaluator,
                    sea: self.sea_level,
                    fluid: self.default_aquifer_fluid,
                },
            ))
        } else {
            None
        };
        Ok(CarvingAquifer {
            inner,
            sea_level: self.sea_level,
            chunk,
        })
    }
    pub fn router(&self) -> &NoiseRouter {
        &self.router
    }
    pub fn sea_level(&self) -> i32 {
        self.sea_level
    }

    pub fn random_algorithm(&self) -> WorldgenRandom {
        self.router.random
    }

    /// Preliminary surface values in Z, X order, for the material pass.
    pub fn preliminary_surface(
        &self,
        evaluator: &mut DensityEvaluator,
        chunk_x: i32,
        chunk_z: i32,
    ) -> Result<Vec<f32>> {
        let x = chunk_x
            .checked_mul(16)
            .context("chunk X exceeds block-coordinate range")?;
        let z = chunk_z
            .checked_mul(16)
            .context("chunk Z exceeds block-coordinate range")?;
        self.router.sample_block_volume(
            evaluator,
            RouterField::ChunkSurfaceLevel,
            [x, 0, z],
            [16, 1, 16],
        )
    }
    /// Load only if older-chunk blending and structure terrain adjustments are absent.
    /// Block-state encodings are preserved and resolved against the registry later.
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
        validate_state_encoding(&materials.default_block).context("invalid default block")?;
        validate_state_encoding(&materials.default_fluid).context("invalid default fluid")?;
        let router = NoiseRouter::from_json_without_terrain_adjustments(provider, bytes, seed)?;
        let state = &materials.default_fluid;
        let name = state
            .as_str()
            .or_else(|| state.get("id").and_then(Value::as_str))
            .context("default fluid has no id")?;
        let id = ResourceId::parse(name)?;
        let default_aquifer_fluid = match (id.namespace.as_str(), id.path.as_str()) {
            ("minecraft", "air") => AquiferMaterial::Air,
            ("minecraft", "cave_air" | "void_air") => AquiferMaterial::DefaultAir,
            ("minecraft", "water") => AquiferMaterial::DefaultWater,
            ("minecraft", "lava") => {
                let nondefault = state
                    .get("properties")
                    .and_then(Value::as_object)
                    .and_then(|p| p.get("level"))
                    .and_then(Value::as_str)
                    .is_some_and(|level| {
                        matches!(
                            level,
                            "1" | "2"
                                | "3"
                                | "4"
                                | "5"
                                | "6"
                                | "7"
                                | "8"
                                | "9"
                                | "10"
                                | "11"
                                | "12"
                                | "13"
                                | "14"
                                | "15"
                        )
                    });
                if nondefault {
                    AquiferMaterial::DefaultLava
                } else {
                    AquiferMaterial::Lava
                }
            }
            _ => AquiferMaterial::DefaultOther,
        };
        let aquifer = if let Some(config) = materials.aquifers {
            let object = config.as_object().context("aquifers must be an object")?;
            let mut compiler = DensityCompiler::with_world_seed(provider, seed, router.random)
                .without_old_chunk_blending()
                .without_structure_terrain_adjustment();
            let mut compile = |name: &str| -> Result<Arc<DensityFunction>> {
                let value = object
                    .get(name)
                    .with_context(|| format!("missing aquifer field {name}"))?;
                compiler
                    .compile(value)
                    .with_context(|| format!("in aquifer field {name}"))
            };
            Some(AquiferFunctions {
                barrier: compile("barrier")?,
                floodedness: compile("fluid_level_floodedness")?,
                spread: compile("fluid_level_spread")?,
                lava: compile("lava")?,
                exclusion: compile("exclusion")?,
                surface: compile("surface_level")?,
            })
        } else {
            None
        };
        Ok(Self {
            router,
            default_block: materials.default_block,
            default_fluid: materials.default_fluid,
            sea_level: materials.sea_level,
            seed,
            aquifer,
            default_aquifer_fluid,
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
        let (materials, fluid_updates) = if height == 0 {
            (Vec::new(), Vec::new())
        } else if let Some(functions) = &self.aquifer {
            let area = AquiferSurfaceArea::for_chunk(x, z)
                .context("chunk aquifer surface coordinates overflow")?;
            let surface = evaluator.sample_quart_surface(
                &functions.surface,
                area.min_x,
                area.min_z,
                area.size_x,
                area.size_z,
            )?;
            let mut aquifer = AquiferGenerator::from_surface_volume(
                self.seed,
                self.router.random == WorldgenRandom::Legacy,
                area,
                &surface,
            )
            .map_err(anyhow::Error::msg)?;
            let mut sampler = NoiseAquiferSampler {
                functions,
                evaluator,
                sea: self.sea_level,
                fluid: self.default_aquifer_fluid,
            };
            let origin_x = x.checked_mul(16).context("chunk X overflow")?;
            let origin_z = z.checked_mul(16).context("chunk Z overflow")?;
            let mut materials = Vec::with_capacity(density.values().len());
            let mut updates = Vec::with_capacity(density.values().len());
            for (i, &value) in density.values().iter().enumerate() {
                let column = i / height as usize;
                let bx = origin_x
                    .checked_add((column % 16) as i32)
                    .context("block X overflow")?;
                let bz = origin_z
                    .checked_add((column / 16) as i32)
                    .context("block Z overflow")?;
                let by = min_y + (i % height as usize) as i32;
                let result = aquifer.sample(&mut sampler, bx, by, bz, f64::from(value));
                materials.push(match result.block {
                    None => NoiseMaterial::DefaultBlock,
                    Some(AquiferMaterial::Air) => NoiseMaterial::Air,
                    Some(AquiferMaterial::Lava) => NoiseMaterial::Lava,
                    Some(
                        AquiferMaterial::DefaultWater
                        | AquiferMaterial::DefaultLava
                        | AquiferMaterial::DefaultAir
                        | AquiferMaterial::DefaultOther,
                    ) => NoiseMaterial::DefaultFluid,
                });
                updates.push(result.schedule_fluid_update);
            }
            (materials, updates)
        } else {
            let materials: Vec<_> = density
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
                .collect();
            let updates = vec![false; materials.len()];
            (materials, updates)
        };
        Ok(NoiseStageChunk {
            chunk_x: x,
            chunk_z: z,
            min_y,
            height,
            generation_min_y: min_y,
            generation_height: height,
            default_block: self.default_block.clone(),
            default_fluid: self.default_fluid.clone(),
            materials,
            fluid_updates,
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
    fn global_fluid_edges_and_aquifer_config_validation() {
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
        let mut carving = generator.carving_aquifer(&mut evaluator, [-1, 2]).unwrap();
        assert_eq!(
            carving.sample(-16, -55, 32).unwrap(),
            CarvingAquiferResult {
                material: Some(NoiseMaterial::Lava),
                schedule_fluid_update: false
            }
        );
        assert_eq!(
            carving.sample(-16, 7, 32).unwrap(),
            CarvingAquiferResult {
                material: Some(NoiseMaterial::DefaultFluid),
                schedule_fluid_update: false
            }
        );
        assert_eq!(
            carving.sample(-16, 8, 32).unwrap(),
            CarvingAquiferResult {
                material: Some(NoiseMaterial::Air),
                schedule_fluid_update: false
            }
        );
        assert!(carving.sample(0, 8, 32).is_err());
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
        aquifer["aquifers"] = json!({"barrier":0,"fluid_level_floodedness":1,"fluid_level_spread":0,"lava":0,"exclusion":0,"surface_level":30});
        let generator = NoiseStageGenerator::from_json_without_terrain_adjustments(
            &mut data,
            &serde_json::to_vec(&aquifer).unwrap(),
            0,
        )
        .unwrap();
        let chunk = generator.generate(&mut evaluator, -1, 2).unwrap();
        assert_eq!(chunk.get(0, 7, 0), Some(NoiseMaterial::DefaultFluid));
        assert_eq!(chunk.get(0, 8, 0), Some(NoiseMaterial::Air));
        aquifer["default_fluid"] = json!("water");
        assert!(
            NoiseStageGenerator::from_json_without_terrain_adjustments(
                &mut data,
                &serde_json::to_vec(&aquifer).unwrap(),
                0
            )
            .is_ok()
        );
        aquifer["default_fluid"] = json!("minecraft:lava");
        assert!(
            NoiseStageGenerator::from_json_without_terrain_adjustments(
                &mut data,
                &serde_json::to_vec(&aquifer).unwrap(),
                0
            )
            .is_ok()
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
    #[test]
    fn noise_volume_expands_without_clipping_or_losing_materials() {
        let mut data = Data;
        let mut evaluator = DensityEvaluator::default();
        let generator = NoiseStageGenerator::from_json_without_terrain_adjustments(
            &mut data,
            &serde_json::to_vec(&definition()).unwrap(),
            0,
        )
        .unwrap();
        let clipped = generator.generate(&mut evaluator, 0, 0).unwrap();
        assert!(clipped.expand_to_dimension(-16, 16).is_err());
        let expanded = generator
            .generate(&mut evaluator, -1, 2)
            .unwrap()
            .expand_to_dimension(-16, 48)
            .unwrap();
        assert_eq!(expanded.chunk_position(), [-1, 2]);
        assert_eq!(
            (expanded.generation_min_y(), expanded.generation_height()),
            (0, 16)
        );
        assert_eq!(expanded.get(0, -1, 0), Some(NoiseMaterial::Air));
        assert_eq!(expanded.get(0, 7, 0), Some(NoiseMaterial::DefaultFluid));
        assert_eq!(expanded.get(0, 16, 0), Some(NoiseMaterial::Air));
        assert_eq!(expanded.fluid_updates().len(), 16 * 16 * 48);
    }
}
