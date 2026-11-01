//! Intermediate noise, biome, surface and carver generation for fresh terrain.
//! Structures, features, lighting, persistence and playable world integration
//! are not implemented by this module.
use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Context, Result, ensure};
use glam::IVec3;
use native_core::beardifier::Beardifier;
use native_resources::{
    biome_source::{BiomeSource, ClimateVolume},
    carver::{CarvingMask, schedule::CarverCache},
    density::DensityEvaluator,
    noise_stage::NoiseStageGenerator,
    packs::{ResourceId, ResourceProvider},
    surface_pass::{SurfaceBlockKind, SurfaceStageChunk, apply_surface},
    surface_rules::{SeededSurfaceInputs, SurfaceRule, SurfaceRuleCompiler},
};
use native_world::biome_zoom::{obfuscate_seed, quart_position};
use serde_json::Value;

mod carving;
pub use carving::CarverBlockState;

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
    structure_context: bool,
    source: BiomeSource,
    possible_biomes: std::collections::BTreeSet<ResourceId>,
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
        Self::build(provider, definition, seed, min_y, height, false)
    }
    /// Compile a generator accepting per-chunk structure contexts. Old-chunk blending is absent.
    pub fn with_structure_context(
        provider: &mut impl ResourceProvider,
        definition: &Value,
        seed: i64,
        min_y: i32,
        height: i32,
    ) -> Result<Self> {
        Self::build(provider, definition, seed, min_y, height, true)
    }
    fn build(
        provider: &mut impl ResourceProvider,
        definition: &Value,
        seed: i64,
        min_y: i32,
        height: i32,
        structure_context: bool,
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
        let noise = if structure_context {
            NoiseStageGenerator::from_json_with_structure_context(provider, &bytes, seed)?
        } else {
            NoiseStageGenerator::from_json_without_terrain_adjustments(provider, &bytes, seed)?
        };
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
        let possible_biomes = source.possible_biomes();
        Ok(Self {
            noise,
            structure_context,
            source,
            possible_biomes,
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
    /// Complete set of biome IDs exposed by this generator's biome source.
    pub fn possible_biomes(&self) -> &std::collections::BTreeSet<ResourceId> {
        &self.possible_biomes
    }
    /// Check a set's candidate chunk after the biome-source filter, including
    /// concentric rings and exclusion references. This does not generate starts.
    pub fn is_structure_candidate<P: ResourceProvider>(
        &self,
        provider: &mut P,
        evaluator: &mut DensityEvaluator,
        cache: &mut native_resources::structure_placement::StructureSetCache,
        biome_rules: &native_resources::structure_biomes::StructureBiomes,
        id: &ResourceId,
        chunk: [i32; 2],
    ) -> Result<bool> {
        cache.is_structure_chunk_for_biomes_with_rings(
            provider,
            biome_rules,
            &self.possible_biomes,
            id,
            self.seed,
            chunk,
            |provider, _, placement| {
                self.concentric_structure_positions(provider, evaluator, placement)
            },
        )
    }
    /// Resolve concentric-ring candidates against the active biome source.
    /// Per-search history is isolated; as with Java's worker-local climate
    /// cache, exact ties may depend on the prior history of that worker.
    pub fn concentric_structure_positions(
        &self,
        provider: &mut impl ResourceProvider,
        evaluator: &mut DensityEvaluator,
        placement: &native_resources::structure_placement::ConcentricRingsPlacement,
    ) -> Result<Vec<[i32; 2]>> {
        let previous = evaluator.replace_structure_terrain_context(None);
        let result = self.concentric_structure_positions_unadjusted(provider, evaluator, placement);
        evaluator.replace_structure_terrain_context(previous);
        result
    }
    fn concentric_structure_positions_unadjusted(
        &self,
        provider: &mut impl ResourceProvider,
        evaluator: &mut DensityEvaluator,
        placement: &native_resources::structure_placement::ConcentricRingsPlacement,
    ) -> Result<Vec<[i32; 2]>> {
        let selector = placement.preferred_biomes();
        let allowed = native_resources::surface_biomes::SurfaceBiomes::load(
            provider,
            std::slice::from_ref(selector),
        )?;
        let possible = &self.possible_biomes;
        let mut preferred = std::collections::BTreeSet::new();
        for biome in possible {
            if allowed.matches(selector, biome)? {
                preferred.insert(biome.clone());
            }
        }
        let all_preferred = preferred.len() == possible.len();
        let mut source = self.source.fresh_sampler();
        placement.positions_with_search(self.seed, |initial, random| {
            // Empty and universal holder sets do not require climate sampling;
            // the reservoir still consumes the same random draws when universal.
            if preferred.is_empty() {
                return Ok(None);
            }
            source.reset_search_history();
            native_resources::structure_placement::find_ring_biome(initial, random, |quart| {
                if all_preferred {
                    return Ok(true);
                }
                let biome = source.sample_quart(self.noise.router(), evaluator, quart);
                Ok(preferred.contains(biome))
            })
        })
    }
    /// Call in the same chunk order as the world generator; equal-distance
    /// climate matches reuse the previous lookup, as in the Java thread.
    pub fn generate_biomes(
        &mut self,
        chunk: [i32; 2],
        evaluator: &mut DensityEvaluator,
    ) -> Result<QuartBiomeChunk> {
        let previous = evaluator.replace_structure_terrain_context(None);
        let result = self.generate_biomes_unadjusted(chunk, evaluator);
        evaluator.replace_structure_terrain_context(previous);
        result
    }
    fn generate_biomes_unadjusted(
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
        let min = [x, self.noise.router().min_y, z];
        let size = [16, self.noise.router().height as u32, 16];
        let mut inputs = if self.structure_context {
            SeededSurfaceInputs::with_structure_context(
                provider,
                &self.surface,
                self.seed,
                self.noise.random_algorithm(),
                min,
                size,
                evaluator.structure_terrain_context().cloned(),
            )?
        } else {
            SeededSurfaceInputs::without_terrain_adjustments(
                provider,
                &self.surface,
                self.seed,
                self.noise.random_algorithm(),
                min,
                size,
            )?
        };
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

    /// Generate a surface with already generated structure pieces affecting this chunk.
    pub fn generate_surface_with_structure_context(
        &self,
        provider: &mut impl ResourceProvider,
        evaluator: &mut DensityEvaluator,
        chunk: [i32; 2],
        neighborhood: &BiomeNeighborhood,
        structure: Arc<Beardifier>,
        classify: impl FnMut(&Value) -> Result<SurfaceBlockKind>,
    ) -> Result<SurfaceStageChunk> {
        ensure!(
            self.structure_context,
            "generator was compiled without structure context"
        );
        let previous = evaluator.replace_structure_terrain_context(Some(structure));
        let result = self.generate_surface(provider, evaluator, chunk, neighborhood, classify);
        evaluator.replace_structure_terrain_context(previous);
        result
    }

    /// Generate the fresh-terrain carving mask. The caller must still apply it
    /// to the surface chunk with aquifer and block-replacement rules.
    pub fn generate_carving_mask(
        &mut self,
        provider: &mut impl ResourceProvider,
        evaluator: &mut DensityEvaluator,
        cache: &mut CarverCache,
        chunk: [i32; 2],
    ) -> Result<CarvingMask> {
        let min_y = self.noise.router().min_y;
        let height = self.noise.router().height;
        let previous = evaluator.replace_structure_terrain_context(None);
        let result = cache.generate(provider, self.seed, chunk, min_y, height, |source| {
            let x = source[0]
                .checked_mul(4)
                .context("carver biome quart X overflow")?;
            let z = source[1]
                .checked_mul(4)
                .context("carver biome quart Z overflow")?;
            Ok(self
                .source
                .sample_quart(self.noise.router(), evaluator, [x, 0, z])
                .clone())
        });
        evaluator.replace_structure_terrain_context(previous);
        result
    }

    /// Produce fresh terrain after surface rules and carvers. This is still an
    /// intermediate chunk: structures, features and lighting remain. Block
    /// classification must be stable for the active registry and pack stack.
    pub fn generate_carved_surface<P: ResourceProvider>(
        &mut self,
        provider: &mut P,
        evaluator: &mut DensityEvaluator,
        cache: &mut CarverCache,
        chunk: [i32; 2],
        neighborhood: &BiomeNeighborhood,
        tags: &native_resources::carver::tags::CarverBlockTags,
        mut classify: impl FnMut(&Value) -> Result<CarverBlockState>,
    ) -> Result<SurfaceStageChunk> {
        let mut result =
            self.generate_surface(provider, evaluator, chunk, neighborhood, |state| {
                let facts = classify(state)?;
                Ok(if facts.is_air {
                    SurfaceBlockKind::Air
                } else if facts.has_fluid {
                    SurfaceBlockKind::Fluid
                } else {
                    SurfaceBlockKind::Solid
                })
            })?;
        let mask = self.generate_carving_mask(provider, evaluator, cache, chunk)?;
        self.apply_carving_to_surface(
            provider,
            evaluator,
            &mut result,
            &mask,
            tags,
            &mut classify,
        )?;
        Ok(result)
    }

    /// Generate surface and carvers with one chunk-specific structure context.
    #[allow(clippy::too_many_arguments)]
    pub fn generate_carved_surface_with_structure_context<P: ResourceProvider>(
        &mut self,
        provider: &mut P,
        evaluator: &mut DensityEvaluator,
        cache: &mut CarverCache,
        chunk: [i32; 2],
        neighborhood: &BiomeNeighborhood,
        tags: &native_resources::carver::tags::CarverBlockTags,
        structure: Arc<Beardifier>,
        classify: impl FnMut(&Value) -> Result<CarverBlockState>,
    ) -> Result<SurfaceStageChunk> {
        ensure!(
            self.structure_context,
            "generator was compiled without structure context"
        );
        let previous = evaluator.replace_structure_terrain_context(Some(structure));
        let result = self.generate_carved_surface(
            provider,
            evaluator,
            cache,
            chunk,
            neighborhood,
            tags,
            classify,
        );
        evaluator.replace_structure_terrain_context(previous);
        result
    }

    /// Apply a mask to a fresh surface-stage chunk from this generator. On
    /// error the partially modified chunk must be discarded.
    pub fn apply_carving_to_surface<P: ResourceProvider>(
        &mut self,
        provider: &mut P,
        evaluator: &mut DensityEvaluator,
        chunk: &mut SurfaceStageChunk,
        mask: &CarvingMask,
        tags: &native_resources::carver::tags::CarverBlockTags,
        mut classify: impl FnMut(&Value) -> Result<CarverBlockState>,
    ) -> Result<()> {
        ensure!(
            chunk.min_y == self.min_y && chunk.height == self.height,
            "surface chunk height does not match generator"
        );
        if !mask.is_empty() {
            let structure = evaluator.structure_terrain_context().cloned();
            let aquifer = self
                .noise
                .carving_aquifer(evaluator, chunk.chunk_position)?;
            let mut inputs = carving::FreshCarvingInputs::new(
                provider,
                tags,
                aquifer,
                &self.noise,
                &self.surface,
                &mut self.source,
                self.seed,
                self.structure_context,
                structure,
                &mut classify,
            );
            native_resources::carver::apply::apply_mask(chunk, mask, &mut inputs)?;
        }
        Ok(())
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
                Some(br#"{"has_precipitation":true,"temperature":0.8,"carvers":[]}"#.to_vec())
            } else if path == "data/minecraft/tags/block/uncarvable.json" {
                Some(br#"{"values":[]}"#.to_vec())
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
    fn structure_context_reaches_noise_and_surface_rules_and_is_restored() {
        use native_core::beardifier::{BoundingBox, RigidPiece, TerrainAdjustment};
        let mut data = Data;
        let mut definition = definition();
        definition["settings"]["noise_router"]["final_density"] = json!({
            "type":"add","left":-0.25,"right":{"type":"beardifier"}
        });
        definition["settings"]["material_rule"] = json!({
            "type":"ore_vein", "density":{"type":"beardifier"}, "richness":1,
            "filler_gap":-1, "raw_ore_chance":1, "ore_block":"minecraft:dirt",
            "raw_ore_block":"minecraft:dirt", "filler_block":"minecraft:dirt"
        });
        let mut generator =
            TerrainGenerator::with_structure_context(&mut data, &definition, 42, 0, 16).unwrap();
        let without =
            TerrainGenerator::without_terrain_adjustments(&mut data, &definition, 42, 0, 16)
                .unwrap();
        let mut evaluator = DensityEvaluator::default();
        let mut neighbors = generator.biome_neighborhood().unwrap();
        for z in -1..=1 {
            for x in -1..=1 {
                neighbors
                    .insert(Arc::new(
                        generator.generate_biomes([x, z], &mut evaluator).unwrap(),
                    ))
                    .unwrap();
            }
        }
        let bounds = BoundingBox::new([0, 5, 0], [2, 7, 2]).unwrap();
        let structure = Arc::new(Beardifier::new(
            vec![RigidPiece {
                bounds,
                adjustment: TerrainAdjustment::Bury,
                ground_delta: 0,
            }],
            vec![],
            Some(BoundingBox::new([-24, -19, -24], [26, 31, 26]).unwrap()),
        ));
        let classify = |v: &Value| -> Result<SurfaceBlockKind> {
            Ok(match v.as_str().unwrap() {
                "minecraft:air" => SurfaceBlockKind::Air,
                "minecraft:water" | "minecraft:lava" => SurfaceBlockKind::Fluid,
                _ => SurfaceBlockKind::Solid,
            })
        };
        assert!(
            without
                .generate_surface_with_structure_context(
                    &mut data,
                    &mut evaluator,
                    [0, 0],
                    &neighbors,
                    Arc::clone(&structure),
                    classify
                )
                .is_err()
        );
        let index = (16 + 1) * 16 + 5;
        let plain = generator
            .generate_surface(&mut data, &mut evaluator, [0, 0], &neighbors, classify)
            .unwrap();
        assert_eq!(plain.states[index].as_str(), Some("minecraft:water"));
        let adjusted = generator
            .generate_surface_with_structure_context(
                &mut data,
                &mut evaluator,
                [0, 0],
                &neighbors,
                Arc::clone(&structure),
                classify,
            )
            .unwrap();
        assert_eq!(adjusted.states[index].as_str(), Some("minecraft:dirt"));
        assert!(evaluator.structure_terrain_context().is_none());
        let registry = ["air", "stone", "water", "lava", "dirt"]
            .into_iter()
            .map(|s| format!("minecraft:{s}"))
            .collect();
        let tags =
            native_resources::carver::tags::CarverBlockTags::load(&mut data, &registry).unwrap();
        let mut cache = CarverCache::default();
        let carved = generator
            .generate_carved_surface_with_structure_context(
                &mut data,
                &mut evaluator,
                &mut cache,
                [0, 0],
                &neighbors,
                &tags,
                Arc::clone(&structure),
                |v| {
                    Ok(CarverBlockState {
                        is_air: v.as_str() == Some("minecraft:air"),
                        has_fluid: matches!(v.as_str(), Some("minecraft:water" | "minecraft:lava")),
                    })
                },
            )
            .unwrap();
        assert_eq!(carved.states[index].as_str(), Some("minecraft:dirt"));
        assert!(evaluator.structure_terrain_context().is_none());
        let previous = Arc::new(Beardifier::new(vec![], vec![], None));
        evaluator.replace_structure_terrain_context(Some(Arc::clone(&previous)));
        assert!(
            generator
                .generate_surface_with_structure_context(
                    &mut data,
                    &mut evaluator,
                    [i32::MAX, 0],
                    &neighbors,
                    structure,
                    classify
                )
                .is_err()
        );
        assert!(Arc::ptr_eq(
            &previous,
            evaluator.structure_terrain_context().unwrap()
        ));
    }

    #[test]
    fn carving_surface_repair_uses_structure_context() {
        use native_core::beardifier::{BoundingBox, RigidPiece, TerrainAdjustment};
        let mut data = Data;
        let mut config = definition();
        config["settings"]["material_rule"] = json!({
            "type":"ore_vein", "density":{"type":"beardifier"}, "richness":1,
            "filler_gap":-1, "raw_ore_chance":1, "ore_block":"minecraft:stone",
            "raw_ore_block":"minecraft:stone", "filler_block":"minecraft:stone"
        });
        let mut generator =
            TerrainGenerator::with_structure_context(&mut data, &config, 42, 0, 16).unwrap();
        let registry = ["air", "stone", "water", "lava", "dirt", "grass_block"]
            .into_iter()
            .map(|s| format!("minecraft:{s}"))
            .collect();
        let tags =
            native_resources::carver::tags::CarverBlockTags::load(&mut data, &registry).unwrap();
        let bounds = BoundingBox::new([0, 5, 0], [2, 7, 2]).unwrap();
        let structure = Arc::new(Beardifier::new(
            vec![RigidPiece {
                bounds,
                adjustment: TerrainAdjustment::Bury,
                ground_delta: 0,
            }],
            vec![],
            Some(BoundingBox::new([-24, -19, -24], [26, 31, 26]).unwrap()),
        ));
        let mut evaluator = DensityEvaluator::default();
        let mut mask = CarvingMask::new(1, 8).unwrap();
        native_resources::carver::carve_ellipsoid(
            &mut mask,
            [0, 0],
            [1.5, 6.0, 1.5],
            0.5,
            1.0,
            |_, _, _, y| y != 6,
        )
        .unwrap();
        let index = (16 + 1) * 16;
        let make_chunk = || {
            let mut chunk = SurfaceStageChunk {
                chunk_position: [0, 0],
                min_y: 0,
                height: 16,
                states: vec![Arc::new(json!("minecraft:stone")); 4096],
                fluid_updates: vec![false; 4096],
            };
            chunk.states[index + 5] = Arc::new(json!("minecraft:dirt"));
            chunk.states[index + 6] = Arc::new(json!("minecraft:grass_block"));
            chunk
        };
        let classify = |v: &Value| -> Result<CarverBlockState> {
            Ok(CarverBlockState {
                is_air: v.as_str() == Some("minecraft:air"),
                has_fluid: matches!(v.as_str(), Some("minecraft:water" | "minecraft:lava")),
            })
        };
        let mut plain = make_chunk();
        generator
            .apply_carving_to_surface(
                &mut data,
                &mut evaluator,
                &mut plain,
                &mask,
                &tags,
                classify,
            )
            .unwrap();
        assert_eq!(plain.states[index + 5].as_str(), Some("minecraft:dirt"));
        evaluator.replace_structure_terrain_context(Some(structure));
        let mut adjusted = make_chunk();
        generator
            .apply_carving_to_surface(
                &mut data,
                &mut evaluator,
                &mut adjusted,
                &mask,
                &tags,
                classify,
            )
            .unwrap();
        assert_eq!(adjusted.states[index + 5].as_str(), Some("minecraft:stone"));
        assert_eq!(adjusted.states[index + 6].as_str(), Some("minecraft:water"));
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

    #[test]
    fn biome_generation_ignores_chunk_structure_context() {
        use native_core::beardifier::{BoundingBox, RigidPiece, TerrainAdjustment};
        let mut data = Data;
        let mut config = definition();
        config["biome_source"] = json!({"type":"minecraft:the_end"});
        config["settings"]["noise_router"]["erosion"] =
            json!({"type":"add", "left":-0.5, "right":{"type":"beardifier"}});
        let mut generator =
            TerrainGenerator::with_structure_context(&mut data, &config, 42, 0, 16).unwrap();
        let mut evaluator = DensityEvaluator::default();
        let baseline = generator.generate_biomes([65, 0], &mut evaluator).unwrap();
        let bounds = BoundingBox::new([1040, 0, 0], [1055, 16, 15]).unwrap();
        let structure = Arc::new(Beardifier::new(
            vec![RigidPiece {
                bounds,
                adjustment: TerrainAdjustment::Bury,
                ground_delta: 0,
            }],
            vec![],
            Some(BoundingBox::new([1016, -24, -24], [1079, 40, 39]).unwrap()),
        ));
        evaluator.replace_structure_terrain_context(Some(Arc::clone(&structure)));
        let adjusted = generator.generate_biomes([65, 0], &mut evaluator).unwrap();
        assert_eq!(baseline.values(), adjusted.values());
        assert!(Arc::ptr_eq(
            &structure,
            evaluator.structure_terrain_context().unwrap()
        ));
        assert!(
            generator
                .generate_biomes([i32::MAX, 0], &mut evaluator)
                .is_err()
        );
        assert!(Arc::ptr_eq(
            &structure,
            evaluator.structure_terrain_context().unwrap()
        ));
    }

    #[test]
    fn carving_uses_noise_height_not_larger_dimension_height() {
        let mut data = Data;
        let mut generator =
            TerrainGenerator::without_terrain_adjustments(&mut data, &definition(), 42, -16, 48)
                .unwrap();
        let mut evaluator = DensityEvaluator::default();
        let mut cache = CarverCache::default();
        let mask = generator
            .generate_carving_mask(&mut data, &mut evaluator, &mut cache, [0, 0])
            .unwrap();
        assert!(mask.is_empty());
        assert_eq!((mask.min_y(), mask.max_y()), (1, 8));
    }
    #[test]
    fn ring_search_shortcuts_preserve_random_results_for_fixed_biomes() {
        use native_resources::structure_placement::{ConcentricRingsPlacement, find_ring_biome};
        let mut data = Data;
        let generator =
            TerrainGenerator::without_terrain_adjustments(&mut data, &definition(), 42, 0, 16)
                .unwrap();
        for (biome, all) in [("minecraft:plains", true), ("minecraft:the_end", false)] {
            let placement=ConcentricRingsPlacement::decode(&json!({"type":"concentric_rings","distance":8,"spread":3,"count":5,"salt":0,"preferred_biomes":biome})).unwrap();
            let actual = generator
                .concentric_structure_positions(
                    &mut data,
                    &mut DensityEvaluator::default(),
                    &placement,
                )
                .unwrap();
            let expected = placement
                .positions_with_search(42, |initial, rng| {
                    find_ring_biome(initial, rng, |_| Ok(all))
                })
                .unwrap();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn fresh_terrain_connects_biome_carvers_aquifer_and_block_tag() {
        use native_resources::carver::tags::CarverBlockTags;
        use std::collections::BTreeSet;
        struct CarvingData;
        impl ResourceProvider for CarvingData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                Ok(if path == "data/minecraft/worldgen/biome/plains.json" {
                    Some(br#"{"has_precipitation":true,"temperature":0.8,"carvers":["minecraft:test"]}"#.to_vec())
                } else if path == "data/minecraft/worldgen/carver/test.json" {
                    Some(br#"{"type":"minecraft:cave","probability":1,"y":{"absolute":4},"count":1,"thickness":2,"room_vertical_radius_multiplier":1,"horizontal_radius_multiplier":1,"vertical_radius_multiplier":1,"floor_level":-1}"#.to_vec())
                } else if path == "data/minecraft/tags/block/uncarvable.json" {
                    Some(br#"{"values":["minecraft:bedrock"]}"#.to_vec())
                } else if path.starts_with("data/minecraft/worldgen/noise/") {
                    Some(br#"{"base_octave":-4,"octave_count":2}"#.to_vec())
                } else {
                    None
                })
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                ensure!(folder == "worldgen/biome", "unexpected registry");
                Ok(vec![ResourceId::parse("minecraft:plains")?])
            }
        }
        let mut data = CarvingData;
        let registry: BTreeSet<_> = ["air", "bedrock", "stone", "dirt", "water", "lava"]
            .into_iter()
            .map(|v| format!("minecraft:{v}"))
            .collect();
        let tags = CarverBlockTags::load(&mut data, &registry).unwrap();
        let mut generator =
            TerrainGenerator::without_terrain_adjustments(&mut data, &definition(), 42, 0, 16)
                .unwrap();
        let mut evaluator = DensityEvaluator::default();
        let mut neighborhood = generator.biome_neighborhood().unwrap();
        for x in -1..=1 {
            for z in -1..=1 {
                neighborhood
                    .insert(Arc::new(
                        generator.generate_biomes([x, z], &mut evaluator).unwrap(),
                    ))
                    .unwrap();
            }
        }
        let mut cache = CarverCache::default();
        let expected = generator
            .generate_carving_mask(&mut data, &mut evaluator, &mut cache, [0, 0])
            .unwrap();
        assert!(!expected.is_empty());
        let generated = generator
            .generate_carved_surface(
                &mut data,
                &mut evaluator,
                &mut cache,
                [0, 0],
                &neighborhood,
                &tags,
                |state| {
                    let id = state.as_str().unwrap_or("");
                    Ok(CarverBlockState {
                        is_air: id == "minecraft:air",
                        has_fluid: id == "minecraft:water" || id == "minecraft:lava",
                    })
                },
            )
            .unwrap();
        for x in 0..16u8 {
            for z in 0..16u8 {
                for y in 0..16i32 {
                    let index = (usize::from(z) * 16 + usize::from(x)) * 16 + y as usize;
                    let block = generated.states[index].as_str().unwrap();
                    let wanted = if expected.contains(x, y, z) {
                        if y < 8 {
                            "minecraft:water"
                        } else {
                            "minecraft:air"
                        }
                    } else {
                        "minecraft:dirt"
                    };
                    assert_eq!(block, wanted, "({x},{y},{z})");
                    assert!(!generated.fluid_updates[index]);
                }
            }
        }
    }
}
