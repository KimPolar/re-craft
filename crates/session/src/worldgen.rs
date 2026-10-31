//! Intermediate noise, biome and surface generation for fresh terrain.
//! Carvers, structures, features, lighting, persistence and playable world
//! integration are not implemented by this module.
use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Context, Result, ensure};
use glam::IVec3;
use native_resources::{
    biome_source::{BiomeSource, ClimateVolume},
    density::DensityEvaluator,
    noise_stage::NoiseStageGenerator,
    packs::{ResourceId, ResourceProvider},
    surface_pass::{SurfaceBlockKind, SurfaceStageChunk, apply_surface},
    surface_rules::{SeededSurfaceInputs, SurfaceRule, SurfaceRuleCompiler},
};
use native_world::biome_zoom::{obfuscate_seed, quart_position};
use serde_json::Value;

/// A complete BIOMES-stage chunk in Z, X, Y order, independent of rendering.
pub struct QuartBiomeChunk {
    chunk: [i32; 2],
    seed: i64,
    min_y: i32,
    height: i32,
    values: Vec<ResourceId>,
}
impl QuartBiomeChunk {
    /// Construct from decoded saved biome IDs in Z, X, Y order. The caller
    /// supplies the saved world's seed and ensures IDs match its active packs.
    pub fn from_values(
        chunk: [i32; 2],
        seed: i64,
        min_y: i32,
        height: i32,
        values: Vec<ResourceId>,
    ) -> Result<Self> {
        ensure!(
            height > 0
                && min_y % 16 == 0
                && height % 16 == 0
                && min_y.checked_add(height).is_some(),
            "invalid biome height range"
        );
        let expected = usize::try_from(height / 4)?
            .checked_mul(16)
            .context("biome volume size overflow")?;
        ensure!(values.len() == expected, "invalid biome volume length");
        Ok(Self {
            chunk,
            seed,
            min_y,
            height,
            values,
        })
    }
    pub fn chunk_position(&self) -> [i32; 2] {
        self.chunk
    }
    pub fn min_y(&self) -> i32 {
        self.min_y
    }
    pub fn height(&self) -> i32 {
        self.height
    }
    pub fn values(&self) -> &[ResourceId] {
        &self.values
    }
    fn at(&self, quart: IVec3) -> Option<&ResourceId> {
        if quart.x.div_euclid(4) != self.chunk[0] || quart.z.div_euclid(4) != self.chunk[1] {
            return None;
        }
        let min = self.min_y.div_euclid(4);
        let height = self.height / 4;
        let y = quart.y.clamp(min, min + height - 1) - min;
        let x = quart.x.rem_euclid(4) as usize;
        let z = quart.z.rem_euclid(4) as usize;
        self.values.get((z * 4 + x) * height as usize + y as usize)
    }
}

/// Biome chunks actually generated or loaded for the surrounding region.
/// All entries must come from the same active data packs as the generator.
pub struct BiomeNeighborhood {
    seed: i64,
    zoom_seed: i64,
    min_y: i32,
    height: i32,
    chunks: BTreeMap<[i32; 2], Arc<QuartBiomeChunk>>,
}
impl BiomeNeighborhood {
    pub fn new(seed: i64, min_y: i32, height: i32) -> Result<Self> {
        ensure!(
            height > 0
                && min_y % 16 == 0
                && height % 16 == 0
                && min_y.checked_add(height).is_some(),
            "invalid biome height range"
        );
        Ok(Self {
            seed,
            zoom_seed: obfuscate_seed(seed),
            min_y,
            height,
            chunks: BTreeMap::new(),
        })
    }
    pub fn insert(&mut self, chunk: Arc<QuartBiomeChunk>) -> Result<()> {
        ensure!(
            chunk.seed == self.seed && chunk.min_y == self.min_y && chunk.height == self.height,
            "biome chunk seed or height does not match the region"
        );
        ensure!(
            !self.chunks.contains_key(&chunk.chunk),
            "duplicate biome chunk"
        );
        self.chunks.insert(chunk.chunk, chunk);
        Ok(())
    }
    pub fn chunk(&self, position: [i32; 2]) -> Option<&Arc<QuartBiomeChunk>> {
        self.chunks.get(&position)
    }
    fn validate(&self, center: [i32; 2], seed: i64, min_y: i32, height: i32) -> Result<()> {
        ensure!(
            self.seed == seed && self.min_y == min_y && self.height == height,
            "biome neighborhood does not match generator"
        );
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                let x = center[0]
                    .checked_add(dx)
                    .context("neighbor chunk X overflow")?;
                let z = center[1]
                    .checked_add(dz)
                    .context("neighbor chunk Z overflow")?;
                ensure!(
                    self.chunks.contains_key(&[x, z]),
                    "missing biome neighbor {x},{z}"
                );
            }
        }
        Ok(())
    }
    /// Apply Java's seeded biome zoom and clamp the selected quart Y.
    pub fn biome_at_block(&self, x: i32, y: i32, z: i32) -> Result<&ResourceId> {
        let quart = quart_position(self.zoom_seed, IVec3::new(x, y, z));
        let key = [quart.x.div_euclid(4), quart.z.div_euclid(4)];
        self.chunks
            .get(&key)
            .with_context(|| format!("missing biome neighbor {},{}", key[0], key[1]))?
            .at(quart)
            .context("biome coordinate missing from neighbor")
    }
}

/// A generator configured from the dimension's active biome source and noise
/// settings. It must be used with the same data-pack stack throughout its life.
pub struct TerrainGenerator {
    noise: NoiseStageGenerator,
    source: BiomeSource,
    surface: SurfaceRule,
    seed: i64,
    min_y: i32,
    height: i32,
}
impl TerrainGenerator {
    /// The caller must guarantee no old-chunk blending or structure adjustment.
    /// The dimension must fully contain the noise-settings height range.
    pub fn without_terrain_adjustments(
        provider: &mut impl ResourceProvider,
        definition: &Value,
        seed: i64,
        min_y: i32,
        height: i32,
    ) -> Result<Self> {
        let kind = ResourceId::parse(
            definition
                .get("type")
                .and_then(Value::as_str)
                .context("missing chunk generator type")?,
        )?;
        ensure!(
            kind.namespace == "minecraft" && kind.path == "noise",
            "unsupported chunk generator"
        );
        let source = BiomeSource::load(
            provider,
            definition
                .get("biome_source")
                .context("missing biome source")?,
        )?;
        let settings = definition
            .get("settings")
            .context("missing noise settings")?;
        let bytes = if let Some(name) = settings.as_str() {
            let id = ResourceId::parse(name)?;
            let path = format!(
                "data/{}/worldgen/noise_settings/{}.json",
                id.namespace, id.path
            );
            provider
                .read_data(&path)?
                .with_context(|| format!("missing noise settings {name}"))?
        } else {
            serde_json::to_vec(settings)?
        };
        let parsed: Value = serde_json::from_slice(&bytes).context("invalid noise settings")?;
        let mut compiler = SurfaceRuleCompiler::new(provider);
        let surface = compiler.compile(
            parsed
                .get("material_rule")
                .context("missing surface material rule")?,
        )?;
        drop(compiler);
        let noise =
            NoiseStageGenerator::from_json_without_terrain_adjustments(provider, &bytes, seed)?;
        let end = min_y
            .checked_add(height)
            .context("dimension height overflow")?;
        let noise_end = noise
            .router()
            .min_y
            .checked_add(noise.router().height)
            .context("noise height overflow")?;
        ensure!(
            height > 0
                && height % 16 == 0
                && min_y % 16 == 0
                && noise.router().height > 0
                && min_y <= noise.router().min_y
                && end >= noise_end
                && height <= 4096,
            "dimension must fully contain the noise height range"
        );
        Ok(Self {
            noise,
            source,
            surface,
            seed,
            min_y,
            height,
        })
    }
    pub fn min_y(&self) -> i32 {
        self.min_y
    }
    pub fn height(&self) -> i32 {
        self.height
    }
    pub fn biome_neighborhood(&self) -> Result<BiomeNeighborhood> {
        BiomeNeighborhood::new(self.seed, self.min_y(), self.height())
    }
    /// Call in the same chunk order as the world generator; equal-distance
    /// climate matches reuse the previous lookup, as in the Java thread.
    pub fn generate_biomes(
        &mut self,
        chunk: [i32; 2],
        evaluator: &mut DensityEvaluator,
    ) -> Result<QuartBiomeChunk> {
        let x = chunk[0].checked_mul(4).context("quart X overflow")?;
        let z = chunk[1].checked_mul(4).context("quart Z overflow")?;
        x.checked_add(3).context("quart X overflow")?;
        z.checked_add(3).context("quart Z overflow")?;
        let min_y = self.min_y();
        let height = self.height();
        let min = [x, min_y.div_euclid(4), z];
        let sy = (height / 4) as usize;
        let climate = if self.source.uses_bulk_climate() {
            Some(ClimateVolume::sample(
                self.noise.router(),
                evaluator,
                min,
                [4, sy as u32, 4],
            )?)
        } else {
            None
        };
        let end = self.source.is_end();
        let end_y_independent = end
            && !self
                .noise
                .router()
                .function(native_resources::noise_router::RouterField::Erosion)
                .depends_on_y();
        let mut end_cache = if end {
            vec![None; if end_y_independent { 1 } else { sy }]
        } else {
            Vec::new()
        };
        let mut values = vec![None; 16 * sy];
        // ChunkAccess walks sections bottom-up, then local X, Y, Z.
        for section in 0..height / 16 {
            for lx in 0..4 {
                for ly in 0..4 {
                    for lz in 0..4 {
                        let y = section * 4 + ly;
                        let point = [x + lx, min[1] + y, z + lz];
                        let biome = if let Some(climate) = &climate {
                            climate
                                .select(&mut self.source, point)
                                .context("climate sample is absent")?
                                .clone()
                        } else if end {
                            let index = if end_y_independent { 0 } else { y as usize };
                            if end_cache[index].is_none() {
                                end_cache[index] = Some(
                                    self.source
                                        .sample_quart(self.noise.router(), evaluator, point)
                                        .clone(),
                                );
                            }
                            end_cache[index].as_ref().unwrap().clone()
                        } else {
                            self.source
                                .sample_quart(self.noise.router(), evaluator, point)
                                .clone()
                        };
                        let index = (lz as usize * 4 + lx as usize) * sy + y as usize;
                        values[index] = Some(biome);
                    }
                }
            }
        }
        let values = values
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .context("incomplete biome volume")?;
        QuartBiomeChunk::from_values(chunk, self.seed, min_y, height, values)
    }
    /// The result is an intermediate SURFACE-stage chunk; it must not be saved
    /// or treated as a complete generated chunk.
    pub fn generate_surface(
        &self,
        provider: &mut impl ResourceProvider,
        evaluator: &mut DensityEvaluator,
        chunk: [i32; 2],
        neighborhood: &BiomeNeighborhood,
        classify: impl FnMut(&Value) -> Result<SurfaceBlockKind>,
    ) -> Result<SurfaceStageChunk> {
        neighborhood.validate(chunk, self.seed, self.min_y(), self.height())?;
        let blocks = self
            .noise
            .generate(evaluator, chunk[0], chunk[1])?
            .expand_to_dimension(self.min_y(), self.height())?;
        let preliminary = self
            .noise
            .preliminary_surface(evaluator, chunk[0], chunk[1])?;
        let x = chunk[0].checked_mul(16).context("block X overflow")?;
        let z = chunk[1].checked_mul(16).context("block Z overflow")?;
        let mut inputs = SeededSurfaceInputs::without_terrain_adjustments(
            provider,
            &self.surface,
            self.seed,
            self.noise.random_algorithm(),
            [x, self.min_y(), z],
            [16, self.height() as u32, 16],
        )?;
        apply_surface(
            &blocks,
            &self.surface,
            &mut inputs,
            &preliminary,
            self.noise.sea_level(),
            classify,
            |x, y, z| Ok(neighborhood.biome_at_block(x, y, z)?.clone()),
        )
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
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(if path.starts_with("data/minecraft/worldgen/biome/") {
                Some(br#"{"has_precipitation":true,"temperature":0.8}"#.to_vec())
            } else if path.starts_with("data/minecraft/worldgen/noise/") {
                Some(br#"{"base_octave":-4,"octave_count":2}"#.to_vec())
            } else {
                None
            })
        }
        fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
            ensure!(folder == "worldgen/biome", "unexpected registry");
            [
                "plains",
                "the_end",
                "end_highlands",
                "end_midlands",
                "small_end_islands",
                "end_barrens",
            ]
            .into_iter()
            .map(ResourceId::parse)
            .collect()
        }
    }
    fn definition() -> Value {
        json!({"type":"noise","biome_source":{"type":"fixed","biome":"minecraft:plains"},"settings":{
            "noise":{"min_y":0,"height":16},"legacy_random_source":false,"sea_level":8,
            "default_block":"minecraft:stone","default_fluid":"minecraft:water",
            "material_rule":{"type":"block","result_state":"minecraft:dirt"},
            "noise_router":{"final_density":1,"temperature":0,"vegetation":0,"continents":0,
                "erosion":0,"depth":0,"ridges":0,"chunk_surface_level":8}
        }})
    }
    #[test]
    fn generates_biomes_and_surface_only_with_complete_matching_neighbors() {
        let mut data = Data;
        assert!(
            TerrainGenerator::without_terrain_adjustments(&mut data, &definition(), 42, -16, 16)
                .is_err()
        );
        let mut generator =
            TerrainGenerator::without_terrain_adjustments(&mut data, &definition(), 42, 0, 16)
                .unwrap();
        let mut evaluator = DensityEvaluator::default();
        let mut neighbors = generator.biome_neighborhood().unwrap();
        let classify = |v: &Value| -> Result<SurfaceBlockKind> {
            Ok(match v.as_str().unwrap() {
                "minecraft:air" => SurfaceBlockKind::Air,
                "minecraft:water" | "minecraft:lava" => SurfaceBlockKind::Fluid,
                _ => SurfaceBlockKind::Solid,
            })
        };
        assert!(
            generator
                .generate_surface(&mut data, &mut evaluator, [0, 0], &neighbors, classify)
                .is_err()
        );
        for z in -1..=1 {
            for x in -1..=1 {
                let generated = generator.generate_biomes([x, z], &mut evaluator).unwrap();
                let biome = Arc::new(if [x, z] == [0, 0] {
                    QuartBiomeChunk::from_values([x, z], 42, 0, 16, generated.values().to_vec())
                        .unwrap()
                } else {
                    generated
                });
                assert_eq!(biome.values().len(), 64);
                assert!(biome.values().iter().all(|id| id.path == "plains"));
                neighbors.insert(biome).unwrap();
            }
        }
        assert_eq!(
            neighbors.biome_at_block(0, -1000, 0).unwrap().path,
            "plains"
        );
        assert_eq!(
            neighbors.biome_at_block(15, 1000, 15).unwrap().path,
            "plains"
        );
        let result = generator
            .generate_surface(&mut data, &mut evaluator, [0, 0], &neighbors, classify)
            .unwrap();
        assert_eq!(result.states.len(), 4096);
        assert!(
            result
                .states
                .iter()
                .all(|v| v.as_str() == Some("minecraft:dirt"))
        );
        let mut wrong = BiomeNeighborhood::new(99, 0, 16).unwrap();
        let biome = Arc::clone(neighbors.chunk([0, 0]).unwrap());
        assert!(wrong.insert(biome).is_err());
        assert!(QuartBiomeChunk::from_values([0, 0], 42, 0, 16, Vec::new()).is_err());
    }

    #[test]
    fn end_chunk_respects_y_dependent_datapack_erosion() {
        let mut data = Data;
        let mut config = definition();
        config["biome_source"] = json!({"type":"minecraft:the_end"});
        config["settings"]["noise_router"]["erosion"] = json!({"type":"gradient","axis":"y",
            "from_coordinate":0,"to_coordinate":16,"from_value":-0.5,"to_value":0.7});
        let mut generator =
            TerrainGenerator::without_terrain_adjustments(&mut data, &config, 42, 0, 16).unwrap();
        let chunk = generator
            .generate_biomes([65, 0], &mut DensityEvaluator::default())
            .unwrap();
        let names: Vec<_> = chunk.values()[0..4]
            .iter()
            .map(|id| id.path.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "small_end_islands",
                "end_barrens",
                "end_midlands",
                "end_highlands"
            ]
        );
        for column in chunk.values().chunks_exact(4) {
            assert_eq!(
                column.iter().map(|id| id.path.as_str()).collect::<Vec<_>>(),
                names
            );
        }
        let center = generator
            .generate_biomes([0, 0], &mut DensityEvaluator::default())
            .unwrap();
        assert!(center.values().iter().all(|id| id.path == "the_end"));
    }
}
