//! Fresh-chunk carver inputs backed by the active world data and terrain.
use anyhow::{Context, Result};
use glam::IVec3;
use native_resources::{
    biome_source::BiomeSource,
    carver::{
        apply::{AquiferReplacement, CarvedState, CarvingInputs},
        tags::CarverBlockTags,
    },
    density::DensityEvaluator,
    noise_router::RouterField,
    noise_stage::{CarvingAquifer, NoiseMaterial, NoiseStageGenerator},
    packs::ResourceProvider,
    surface_pass::SurfaceStageChunk,
    surface_rules::{SeededSurfaceInputs, SurfacePosition, SurfaceRule},
};
use native_world::biome_zoom::{obfuscate_seed, quart_position};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};

/// Both properties are needed because a solid block can contain fluid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CarverBlockState {
    pub is_air: bool,
    pub has_fluid: bool,
}

pub(super) struct FreshCarvingInputs<'a, 'b, P: ResourceProvider, F> {
    provider: Option<&'a mut P>,
    surface_inputs: Option<SeededSurfaceInputs<'a, P>>,
    tags: &'a CarverBlockTags,
    aquifer: CarvingAquifer<'b>,
    noise: &'a NoiseStageGenerator,
    surface: &'a SurfaceRule,
    source: &'a mut BiomeSource,
    evaluator: DensityEvaluator,
    zoom_seed: i64,
    seed: i64,
    classify: &'a mut F,
    facts: HashMap<Value, CarverBlockState>,
    protected: HashMap<Value, bool>,
    air: Arc<Value>,
    lava: Arc<Value>,
    default_fluid: Arc<Value>,
}
impl<'a, 'b, P: ResourceProvider, F: FnMut(&Value) -> Result<CarverBlockState>>
    FreshCarvingInputs<'a, 'b, P, F>
{
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        provider: &'a mut P,
        tags: &'a CarverBlockTags,
        aquifer: CarvingAquifer<'b>,
        noise: &'a NoiseStageGenerator,
        surface: &'a SurfaceRule,
        source: &'a mut BiomeSource,
        seed: i64,
        classify: &'a mut F,
    ) -> Self {
        Self {
            provider: Some(provider),
            surface_inputs: None,
            tags,
            aquifer,
            noise,
            surface,
            source,
            evaluator: DensityEvaluator::default(),
            zoom_seed: obfuscate_seed(seed),
            seed,
            classify,
            facts: HashMap::new(),
            protected: HashMap::new(),
            air: Arc::new(json!("minecraft:air")),
            lava: Arc::new(json!("minecraft:lava")),
            default_fluid: Arc::new(noise.default_fluid().clone()),
        }
    }
    fn height(&mut self, chunk: &SurfaceStageChunk, x: usize, z: usize) -> Result<i32> {
        let h = usize::try_from(chunk.height)?;
        let start = (z * 16 + x) * h;
        for i in (0..h).rev() {
            if !self.facts(&chunk.states[start + i])?.is_air {
                return Ok(chunk.min_y + i as i32);
            }
        }
        chunk
            .min_y
            .checked_sub(1)
            .context("empty column height overflow")
    }
    fn facts(&mut self, state: &Value) -> Result<CarverBlockState> {
        if let Some(facts) = self.facts.get(state) {
            return Ok(*facts);
        }
        let facts = (self.classify)(state)?;
        self.facts.insert(state.clone(), facts);
        Ok(facts)
    }
}
impl<P: ResourceProvider, F: FnMut(&Value) -> Result<CarverBlockState>> CarvingInputs
    for FreshCarvingInputs<'_, '_, P, F>
{
    fn may_carve(&mut self, _: u8, _: i32, _: u8) -> Result<bool> {
        Ok(true)
    }
    fn uncarvable(&mut self, state: &Value) -> Result<bool> {
        if let Some(value) = self.protected.get(state) {
            return Ok(*value);
        }
        let value = self.tags.uncarvable(state)?;
        self.protected.insert(state.clone(), value);
        Ok(value)
    }
    fn aquifer(&mut self, x: i32, y: i32, z: i32) -> Result<Option<AquiferReplacement>> {
        let result = self.aquifer.sample(x, y, z)?;
        let Some(material) = result.material else {
            return Ok(None);
        };
        let state = match material {
            NoiseMaterial::Air => Arc::clone(&self.air),
            NoiseMaterial::Lava => Arc::clone(&self.lava),
            NoiseMaterial::DefaultFluid => Arc::clone(&self.default_fluid),
            NoiseMaterial::DefaultBlock => anyhow::bail!("unexpected solid aquifer result"),
        };
        let has_fluid = self.facts(&state)?.has_fluid;
        Ok(Some(AquiferReplacement {
            block: CarvedState { state, has_fluid },
            schedule_fluid_update: result.schedule_fluid_update,
        }))
    }
    fn top_material(
        &mut self,
        chunk: &SurfaceStageChunk,
        x: i32,
        y: i32,
        z: i32,
        under_fluid: bool,
    ) -> Result<Option<CarvedState>> {
        let lx = x.rem_euclid(16) as usize;
        let lz = z.rem_euclid(16) as usize;
        let gradient_x = self.height(chunk, (lx + 1).min(15), lz)?
            - self.height(chunk, lx.saturating_sub(1), lz)?;
        let gradient_z = self.height(chunk, lx, (lz + 1).min(15))?
            - self.height(chunk, lx, lz.saturating_sub(1))?;
        let quart = quart_position(self.zoom_seed, IVec3::new(x, y, z));
        let biome = self
            .source
            .sample_quart(
                self.noise.router(),
                &mut self.evaluator,
                [quart.x, quart.y, quart.z],
            )
            .clone();
        let preliminary = self.noise.router().sample_block_volume(
            &mut self.evaluator,
            RouterField::ChunkSurfaceLevel,
            [x, 0, z],
            [1, 1, 1],
        )?[0];
        let integral = preliminary as i32;
        let floor = if preliminary < integral as f32 {
            integral.wrapping_sub(1)
        } else {
            integral
        };
        if self.surface_inputs.is_none() {
            let provider = self
                .provider
                .take()
                .context("surface provider unavailable")?;
            self.surface_inputs = Some(SeededSurfaceInputs::for_carver_top_material(
                provider,
                self.surface,
                self.seed,
                self.noise.random_algorithm(),
            )?);
        }
        let inputs = self
            .surface_inputs
            .as_mut()
            .expect("surface inputs initialized");
        inputs.set_single_position([x, y, z])?;
        inputs.set_biome(biome)?;
        let surface_depth = inputs.surface_depth(x, z)?;
        let surface_secondary = inputs.surface_secondary(x, z)?;
        let position = SurfacePosition {
            x,
            y,
            z,
            min_y: self.noise.router().min_y,
            height: self.noise.router().height,
            sea_level: self.noise.sea_level(),
            stone_above: 1,
            stone_below: 1,
            surface_depth,
            surface_secondary,
            water_height: if under_fluid {
                y.wrapping_add(1)
            } else {
                i32::MIN
            },
            min_surface_level: floor.wrapping_add(surface_depth).wrapping_sub(8),
            gradient_x,
            gradient_z,
        };
        let Some(state) = self.surface.evaluate(&position, inputs)? else {
            return Ok(None);
        };
        let has_fluid = self.facts(&state)?.has_fluid;
        Ok(Some(CarvedState { state, has_fluid }))
    }
}
