//! Data-driven 26.3 material rules. Evaluation requires a real surface context;
//! callers supply biome, noise, density, and terrain information.
use crate::{
    density::{DensityCompiler, DensityEvaluator, DensityFunction, WorldgenRandom},
    packs::{ResourceId, ResourceProvider},
};
use anyhow::{Context, Result, bail, ensure};
use native_core::{
    beardifier::Beardifier,
    normal_noise::{
        LegacyNoiseFactory, NoiseSeedFactory, NoiseSeedSource, NormalNoise, XoroshiroNoiseFactory,
    },
    random::{LegacyRandom, XoroshiroRandom},
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
};

const MAX_DEPTH: usize = 128;
const MAX_NODES: usize = 100_000;

/// Inputs for one block, calculated by the chunk's material system.
pub struct SurfacePosition {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub min_y: i32,
    pub height: i32,
    pub sea_level: i32,
    pub stone_above: i32,
    pub stone_below: i32,
    pub surface_depth: i32,
    pub surface_secondary: f64,
    pub water_height: i32,
    pub min_surface_level: i32,
    pub gradient_x: i32,
    pub gradient_z: i32,
}

/// Data which cannot be inferred from a rule definition alone.
pub trait SurfaceInputs {
    fn biome_matches(&mut self, selector: &Value, position: &SurfacePosition) -> Result<bool>;
    fn cold_enough_to_snow(&mut self, position: &SurfacePosition) -> Result<bool>;
    fn noise(&mut self, name: &ResourceId, is_3d: bool, position: &SurfacePosition) -> Result<f64>;
    fn positional_random(&mut self, name: &ResourceId, position: &SurfacePosition) -> Result<f32>;
    /// A fresh "minecraft:ore" positional generator for this block.
    fn ore_random(&mut self, position: &SurfacePosition) -> Result<SurfaceRandom>;
    /// `in_chunk` selects Java's volume-sampled density path when available.
    fn density(
        &mut self,
        definition: &Value,
        in_chunk: bool,
        position: &SurfacePosition,
    ) -> Result<f32>;
    fn bandlands(&mut self, position: &SurfacePosition) -> Result<Arc<Value>>;
}

pub enum SurfaceRandom {
    Legacy(LegacyRandom),
    Xoroshiro(XoroshiroRandom),
}
impl SurfaceRandom {
    pub fn next_float(&mut self) -> f32 {
        match self {
            Self::Legacy(r) => r.next_float(),
            Self::Xoroshiro(r) => r.next_float(),
        }
    }
    pub fn next_double(&mut self) -> f64 {
        match self {
            Self::Legacy(r) => r.next_double(),
            Self::Xoroshiro(r) => r.next_double(),
        }
    }
    pub(crate) fn next_int_bound(&mut self, bound: i32) -> i32 {
        match self {
            Self::Legacy(r) => r.next_int_bound(bound),
            Self::Xoroshiro(r) => r.next_int_bound(bound),
        }
        .expect("positive random bound")
    }
    fn next_bool(&mut self) -> bool {
        match self {
            Self::Legacy(r) => r.next_bool(),
            Self::Xoroshiro(r) => r.next_bool(),
        }
    }
}

enum PositionalFactory {
    Legacy(LegacyNoiseFactory),
    Xoroshiro(XoroshiroNoiseFactory),
}

/// Named, seeded factories shared by material rules in a generation context.
pub struct SurfaceRandomFactory {
    root: PositionalFactory,
    named: BTreeMap<ResourceId, PositionalFactory>,
}
impl SurfaceRandomFactory {
    pub fn new(seed: i64, algorithm: WorldgenRandom) -> Self {
        let root = match algorithm {
            WorldgenRandom::Legacy => {
                PositionalFactory::Legacy(LegacyRandom::new(seed).fork_noise_factory())
            }
            WorldgenRandom::Xoroshiro => {
                PositionalFactory::Xoroshiro(XoroshiroRandom::new(seed).fork_noise_factory())
            }
        };
        Self {
            root,
            named: BTreeMap::new(),
        }
    }
    pub fn at(&mut self, name: &ResourceId, x: i32, y: i32, z: i32) -> SurfaceRandom {
        if !self.named.contains_key(name) {
            let id = format!("{}:{}", name.namespace, name.path);
            let factory = match &self.root {
                PositionalFactory::Legacy(root) => {
                    PositionalFactory::Legacy(root.random_for(&id).fork_noise_factory())
                }
                PositionalFactory::Xoroshiro(root) => {
                    PositionalFactory::Xoroshiro(root.random_for(&id).fork_noise_factory())
                }
            };
            self.named.insert(name.clone(), factory);
        }
        let factory = self.named.get(name).expect("factory was inserted");
        match factory {
            PositionalFactory::Legacy(factory) => SurfaceRandom::Legacy(factory.at(x, y, z)),
            PositionalFactory::Xoroshiro(factory) => SurfaceRandom::Xoroshiro(factory.at(x, y, z)),
        }
    }
    /// MaterialSystem surface depth uses the root factory directly.
    pub fn root_at(&self, x: i32, y: i32, z: i32) -> SurfaceRandom {
        match &self.root {
            PositionalFactory::Legacy(factory) => SurfaceRandom::Legacy(factory.at(x, y, z)),
            PositionalFactory::Xoroshiro(factory) => SurfaceRandom::Xoroshiro(factory.at(x, y, z)),
        }
    }
    fn root_named(&self, name: &str) -> SurfaceRandom {
        match &self.root {
            PositionalFactory::Legacy(factory) => SurfaceRandom::Legacy(factory.random_for(name)),
            PositionalFactory::Xoroshiro(factory) => {
                SurfaceRandom::Xoroshiro(factory.random_for(name))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(usize)]
pub enum TerracottaBand {
    Plain,
    Orange,
    Yellow,
    Brown,
    Red,
    White,
    LightGray,
}
impl TerracottaBand {
    pub fn id(self) -> &'static str {
        match self {
            Self::Plain => "minecraft:terracotta",
            Self::Orange => "minecraft:orange_terracotta",
            Self::Yellow => "minecraft:yellow_terracotta",
            Self::Brown => "minecraft:brown_terracotta",
            Self::Red => "minecraft:red_terracotta",
            Self::White => "minecraft:white_terracotta",
            Self::LightGray => "minecraft:light_gray_terracotta",
        }
    }
}

/// The 192 seeded terracotta layers used by the bandlands material rule.
pub struct SurfaceBands {
    bands: [TerracottaBand; 192],
    states: [Arc<Value>; 7],
    offset_noise: ResourceId,
}
impl SurfaceBands {
    pub fn new(seed: i64, algorithm: WorldgenRandom) -> Self {
        use TerracottaBand::*;
        let random = SurfaceRandomFactory::new(seed, algorithm);
        let mut random = random.root_named("minecraft:clay_bands");
        let mut bands = [Plain; 192];
        let mut index = 0usize;
        while index < bands.len() {
            index += (random.next_int_bound(5) + 1) as usize;
            if index < bands.len() {
                bands[index] = Orange;
            }
            index += 1;
        }
        for (color, base) in [(Yellow, 1), (Brown, 2), (Red, 1)] {
            let count = 6 + random.next_int_bound(10);
            for _ in 0..count {
                let length = (base + random.next_int_bound(3)) as usize;
                let start = random.next_int_bound(192) as usize;
                for slot in bands.iter_mut().skip(start).take(length) {
                    *slot = color;
                }
            }
        }
        let count = 9 + random.next_int_bound(7);
        let mut index = 0usize;
        for _ in 0..count {
            if index >= bands.len() {
                break;
            }
            bands[index] = White;
            if index > 1 && random.next_bool() {
                bands[index - 1] = LightGray;
            }
            if index + 1 < bands.len() && random.next_bool() {
                bands[index + 1] = LightGray;
            }
            index += (4 + random.next_int_bound(16)) as usize;
        }
        let colors = [Plain, Orange, Yellow, Brown, Red, White, LightGray];
        Self {
            bands,
            states: colors.map(|color| Arc::new(Value::String(color.id().into()))),
            offset_noise: ResourceId::parse("minecraft:clay_bands_offset")
                .expect("valid identifier"),
        }
    }
    pub fn bands(&self) -> &[TerracottaBand; 192] {
        &self.bands
    }
    pub fn at(&self, noise: &mut SurfaceNoise, x: i32, y: i32, z: i32) -> Result<Arc<Value>> {
        let offset = noise.sample(&self.offset_noise, false, x, 0, z)? * 4.0;
        // Math.round(float) rounds ties toward positive infinity.
        let offset = (f64::from(offset) + 0.5).floor() as i32;
        let index = y.wrapping_add(offset).wrapping_add(192) % 192;
        ensure!(index >= 0, "Java bandlands index is negative at Y={y}");
        Ok(Arc::clone(
            &self.states[self.bands[index as usize] as usize],
        ))
    }
}

struct SurfaceNoiseEntry {
    noise: NormalNoise,
    last_2d: Option<(i32, i32, f32)>,
    last_3d: Option<(i32, i32, i32, f32)>,
}

/// Preloaded material noises and the root positional RNG for one world seed.
pub struct SurfaceNoise {
    noises: BTreeMap<ResourceId, SurfaceNoiseEntry>,
    random: SurfaceRandomFactory,
    surface_id: ResourceId,
    secondary_id: ResourceId,
}
impl SurfaceNoise {
    /// Load active data-pack definitions before borrowing the provider for density sampling.
    pub fn load(
        provider: &mut impl ResourceProvider,
        seed: i64,
        algorithm: WorldgenRandom,
        names: impl IntoIterator<Item = ResourceId>,
    ) -> Result<Self> {
        let mut required: BTreeSet<_> = names.into_iter().collect();
        let surface_id = ResourceId::parse("minecraft:surface")?;
        let secondary_id = ResourceId::parse("minecraft:surface_secondary")?;
        required.insert(surface_id.clone());
        required.insert(secondary_id.clone());
        let random = SurfaceRandomFactory::new(seed, algorithm);
        let mut noises = BTreeMap::new();
        for id in required {
            let name = format!("{}:{}", id.namespace, id.path);
            let parameters = crate::noise::load_normal_noise(provider, &name)?;
            let noise = match &random.root {
                PositionalFactory::Legacy(root) => parameters.create(&mut root.random_for(&name)),
                PositionalFactory::Xoroshiro(root) => {
                    parameters.create(&mut root.random_for(&name))
                }
            }
            .map_err(anyhow::Error::msg)?;
            noises.insert(
                id,
                SurfaceNoiseEntry {
                    noise,
                    last_2d: None,
                    last_3d: None,
                },
            );
        }
        Ok(Self {
            noises,
            random,
            surface_id,
            secondary_id,
        })
    }
    pub fn sample(
        &mut self,
        name: &ResourceId,
        is_3d: bool,
        x: i32,
        y: i32,
        z: i32,
    ) -> Result<f32> {
        let entry = self.noises.get_mut(name).with_context(|| {
            format!(
                "material noise not loaded: {}:{}",
                name.namespace, name.path
            )
        })?;
        Ok(sample_surface_noise(entry, is_3d, x, y, z))
    }
    /// Sample at scaled coordinates used by biome-specific surface extensions.
    pub fn sample_scaled(&mut self, name: &ResourceId, x: f64, z: f64) -> Result<f32> {
        let entry = self.noises.get_mut(name).with_context(|| {
            format!(
                "material noise not loaded: {}:{}",
                name.namespace, name.path
            )
        })?;
        Ok(entry.noise.sample(x, 0.0, z))
    }
    pub fn surface_depth(&mut self, x: i32, z: i32) -> Result<i32> {
        let entry = self
            .noises
            .get_mut(&self.surface_id)
            .expect("surface noise is loaded");
        let noise = f64::from(sample_surface_noise(entry, false, x, 0, z));
        let random = self.random.root_at(x, 0, z).next_double();
        Ok((noise * 2.75 + 3.0 + random * 0.25) as i32)
    }
    pub fn surface_secondary(&mut self, x: i32, z: i32) -> Result<f64> {
        let entry = self
            .noises
            .get_mut(&self.secondary_id)
            .expect("secondary surface noise is loaded");
        Ok(f64::from(sample_surface_noise(entry, false, x, 0, z)))
    }
}

/// The three world-seeded noise fields for eroded-badlands pillars.
pub struct BadlandsPillars {
    surface: ResourceId,
    pillar: ResourceId,
    roof: ResourceId,
}
impl Default for BadlandsPillars {
    fn default() -> Self {
        Self {
            surface: ResourceId::parse("minecraft:badlands_surface").unwrap(),
            pillar: ResourceId::parse("minecraft:badlands_pillar").unwrap(),
            roof: ResourceId::parse("minecraft:badlands_pillar_roof").unwrap(),
        }
    }
}
impl BadlandsPillars {
    pub fn noise_dependencies(&self) -> [ResourceId; 3] {
        [self.surface.clone(), self.pillar.clone(), self.roof.clone()]
    }
    /// Java computes the pillar product in float, then the roof and cap in double.
    pub fn peak(&self, noise: &mut SurfaceNoise, x: i32, z: i32) -> Result<Option<i32>> {
        let x = f64::from(x);
        let z = f64::from(z);
        let surface = (f64::from(noise.sample_scaled(&self.surface, x, z)?) * 8.25).abs();
        let pillar = f64::from(noise.sample_scaled(&self.pillar, x * 0.2, z * 0.2)? * 15.0_f32);
        let value = surface.min(pillar);
        if value <= 0.0 {
            return Ok(None);
        }
        let roof = (f64::from(noise.sample_scaled(&self.roof, x * 0.75, z * 0.75)?) * 1.5).abs();
        let peak = 64.0 + (value * value * 2.5).min((roof * 50.0).ceil() + 24.0);
        Ok(Some(peak.floor() as i32))
    }
}

/// Seeded surface noise fields used by frozen-ocean icebergs.
pub struct FrozenOceanIcebergs {
    surface: ResourceId,
    pillar: ResourceId,
    roof: ResourceId,
}
impl Default for FrozenOceanIcebergs {
    fn default() -> Self {
        Self {
            surface: ResourceId::parse("minecraft:iceberg_surface").unwrap(),
            pillar: ResourceId::parse("minecraft:iceberg_pillar").unwrap(),
            roof: ResourceId::parse("minecraft:iceberg_pillar_roof").unwrap(),
        }
    }
}
impl FrozenOceanIcebergs {
    pub fn noise_dependencies(&self) -> [ResourceId; 3] {
        [self.surface.clone(), self.pillar.clone(), self.roof.clone()]
    }
    /// Returns the top and bottom bounds before block placement.
    pub fn bounds(
        &self,
        noise: &mut SurfaceNoise,
        x: i32,
        z: i32,
        sea: i32,
        melts: bool,
    ) -> Result<Option<(f64, f64)>> {
        let x = f64::from(x);
        let z = f64::from(z);
        let surface = (f64::from(noise.sample_scaled(&self.surface, x, z)?) * 8.25).abs();
        let pillar = f64::from(noise.sample_scaled(&self.pillar, x * 1.28, z * 1.28)? * 15.0_f32);
        let value = surface.min(pillar);
        if value <= 1.8 {
            return Ok(None);
        }
        let roof = (f64::from(noise.sample_scaled(&self.roof, x * 1.17, z * 1.17)?) * 1.5).abs();
        let mut height = (value * value * 1.2).min((roof * 40.0).ceil() + 14.0);
        if melts {
            height -= 2.0;
        }
        if height <= 2.0 {
            return Ok(None);
        }
        Ok(Some((
            height + f64::from(sea),
            f64::from(sea) - height - 7.0,
        )))
    }
}
fn sample_surface_noise(entry: &mut SurfaceNoiseEntry, is_3d: bool, x: i32, y: i32, z: i32) -> f32 {
    if is_3d {
        if let Some((cx, cy, cz, value)) = entry.last_3d {
            if (cx, cy, cz) == (x, y, z) {
                return value;
            }
        }
        let value = entry.noise.sample(f64::from(x), f64::from(y), f64::from(z));
        entry.last_3d = Some((x, y, z, value));
        value
    } else {
        if let Some((cx, cz, value)) = entry.last_2d {
            if (cx, cz) == (x, z) {
                return value;
            }
        }
        let value = entry.noise.sample(f64::from(x), 0.0, f64::from(z));
        entry.last_2d = Some((x, z, value));
        value
    }
}

struct SurfaceDensityEntry {
    function: Arc<DensityFunction>,
    volume: Option<Vec<f32>>,
}

/// A world-seeded density context scoped to one block volume.
/// Density definitions and Java-order volume buffers are compiled at most once.
pub struct SurfaceDensitySampler<'a, P: ResourceProvider> {
    compiler: DensityCompiler<'a, P>,
    evaluator: DensityEvaluator,
    min: [i32; 3],
    max: [i32; 3],
    size: [u32; 3],
    entries: HashMap<Value, SurfaceDensityEntry>,
}
impl<'a, P: ResourceProvider> SurfaceDensitySampler<'a, P> {
    /// The caller must guarantee no old-chunk blending or structure adjustment.
    pub fn without_terrain_adjustments(
        provider: &'a mut P,
        seed: i64,
        algorithm: WorldgenRandom,
        min: [i32; 3],
        size: [u32; 3],
    ) -> Result<Self> {
        Self::build(provider, seed, algorithm, min, size, false, None)
    }
    /// Use the given chunk's structure adjustment; older-chunk blending must be absent.
    pub fn with_structure_context(
        provider: &'a mut P,
        seed: i64,
        algorithm: WorldgenRandom,
        min: [i32; 3],
        size: [u32; 3],
        structure: Option<Arc<Beardifier>>,
    ) -> Result<Self> {
        Self::build(provider, seed, algorithm, min, size, true, structure)
    }
    fn build(
        provider: &'a mut P,
        seed: i64,
        algorithm: WorldgenRandom,
        min: [i32; 3],
        size: [u32; 3],
        structure_context: bool,
        structure: Option<Arc<Beardifier>>,
    ) -> Result<Self> {
        ensure!(
            size.iter().all(|n| *n > 0),
            "surface density volume dimensions must be positive"
        );
        let count = size
            .iter()
            .try_fold(1usize, |n, x| n.checked_mul(*x as usize))
            .context("surface density volume size overflow")?;
        ensure!(
            count <= 1_048_576,
            "surface density volume exceeds maximum sample count"
        );
        let mut max = [0; 3];
        for axis in 0..3 {
            max[axis] = i32::try_from(i64::from(min[axis]) + i64::from(size[axis]) - 1)
                .context("surface density volume coordinates overflow")?;
        }
        let compiler = DensityCompiler::with_world_seed(provider, seed, algorithm)
            .without_old_chunk_blending();
        let compiler = if structure_context {
            compiler.with_structure_terrain_context()
        } else {
            compiler.without_structure_terrain_adjustment()
        };
        let mut evaluator = DensityEvaluator::default();
        evaluator.replace_structure_terrain_context(structure);
        Ok(Self {
            compiler,
            evaluator,
            min,
            max,
            size,
            entries: HashMap::new(),
        })
    }
    pub fn sample(
        &mut self,
        definition: &Value,
        in_chunk: bool,
        x: i32,
        y: i32,
        z: i32,
    ) -> Result<f32> {
        if !self.entries.contains_key(definition) {
            let function = self
                .compiler
                .compile(definition)
                .context("invalid material density function")?;
            self.entries.insert(
                definition.clone(),
                SurfaceDensityEntry {
                    function,
                    volume: None,
                },
            );
        }
        let entry = self
            .entries
            .get_mut(definition)
            .expect("density entry was inserted");
        if in_chunk
            && [x, y, z]
                .iter()
                .enumerate()
                .all(|(i, &v)| v >= self.min[i] && v <= self.max[i])
        {
            if entry.volume.is_none() {
                entry.volume = Some(self.evaluator.sample_block_volume(
                    &entry.function,
                    self.min,
                    self.size,
                )?);
            }
            let rx = (i64::from(x) - i64::from(self.min[0])) as usize;
            let ry = (i64::from(y) - i64::from(self.min[1])) as usize;
            let rz = (i64::from(z) - i64::from(self.min[2])) as usize;
            let index = (rz * self.size[0] as usize + rx) * self.size[1] as usize + ry;
            Ok(entry.volume.as_ref().unwrap()[index])
        } else {
            Ok(self.evaluator.sample(&entry.function, x, y, z))
        }
    }

    /// Reuse compiled functions while moving a one-block rule context. Each
    /// move invalidates volume samples, which depend on the volume origin.
    pub fn set_single_position(&mut self, position: [i32; 3]) -> Result<()> {
        ensure!(
            self.size == [1, 1, 1],
            "surface density context is not a single block"
        );
        if self.min != position {
            self.min = position;
            self.max = position;
            for entry in self.entries.values_mut() {
                entry.volume = None;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Anchor {
    Absolute(i32),
    AboveBottom(i32),
    BelowTop(i32),
    Sea(i32),
}
impl Anchor {
    fn parse(value: &Value) -> Result<Self> {
        let obj = value
            .as_object()
            .context("vertical anchor must be an object")?;
        for (name, constructor) in [
            ("absolute", Self::Absolute as fn(i32) -> Self),
            ("above_bottom", Self::AboveBottom),
            ("below_top", Self::BelowTop),
            ("relative_to_sea_level", Self::Sea),
        ] {
            if obj.contains_key(name) {
                return Ok(constructor(integer(value, name)?));
            }
        }
        bail!("unknown vertical anchor")
    }
    fn resolve(self, p: &SurfacePosition) -> i32 {
        match self {
            Self::Absolute(y) => y,
            Self::AboveBottom(n) => p.min_y.wrapping_add(n),
            Self::BelowTop(n) => p
                .min_y
                .wrapping_add(p.height)
                .wrapping_sub(1)
                .wrapping_sub(n),
            Self::Sea(n) => p.sea_level.wrapping_add(n),
        }
    }
}

enum Condition {
    Biome(Value),
    Temperature,
    AboveSurface,
    Hole,
    Steep,
    Noise {
        name: ResourceId,
        min: f64,
        max: f64,
        is_3d: bool,
    },
    Gradient {
        name: ResourceId,
        low: Anchor,
        high: Anchor,
    },
    Y {
        anchor: Anchor,
        multiplier: i32,
        add_stone: bool,
    },
    Water {
        offset: i32,
        multiplier: i32,
        add_stone: bool,
    },
    Stone {
        offset: i32,
        add_surface: bool,
        secondary: i32,
        ceiling: bool,
    },
    Not(Arc<Condition>),
}
impl Condition {
    fn collect_biomes(&self, out: &mut Vec<Value>) {
        match self {
            Self::Biome(selector) if !out.contains(selector) => out.push(selector.clone()),
            Self::Not(inner) => inner.collect_biomes(out),
            _ => {}
        }
    }
    fn collect_noises(&self, out: &mut BTreeSet<ResourceId>) {
        match self {
            Self::Noise { name, .. } => {
                out.insert(name.clone());
            }
            Self::Not(inner) => inner.collect_noises(out),
            _ => {}
        }
    }
    fn test(&self, p: &SurfacePosition, inputs: &mut impl SurfaceInputs) -> Result<bool> {
        Ok(match self {
            Self::Biome(selector) => inputs.biome_matches(selector, p)?,
            Self::Temperature => inputs.cold_enough_to_snow(p)?,
            Self::AboveSurface => p.y >= p.min_surface_level,
            Self::Hole => p.surface_depth <= 0,
            Self::Steep => p.gradient_x <= -4 || p.gradient_z >= 4,
            Self::Noise {
                name,
                min,
                max,
                is_3d,
            } => {
                let n = inputs.noise(name, *is_3d, p)?;
                n >= *min && n <= *max
            }
            Self::Gradient { name, low, high } => {
                let low = low.resolve(p);
                let high = high.resolve(p);
                if p.y <= low {
                    true
                } else if p.y >= high {
                    false
                } else {
                    let t = (f64::from(p.y) - f64::from(low)) / (f64::from(high) - f64::from(low));
                    f64::from(inputs.positional_random(name, p)?) < 1.0 + t * (0.0 - 1.0)
                }
            }
            Self::Y {
                anchor,
                multiplier,
                add_stone,
            } => {
                let y = p.y.wrapping_add(if *add_stone { p.stone_above } else { 0 });
                let limit = anchor
                    .resolve(p)
                    .wrapping_add(p.surface_depth.wrapping_mul(*multiplier));
                y >= limit
            }
            Self::Water {
                offset,
                multiplier,
                add_stone,
            } => {
                p.water_height == i32::MIN
                    || p.y.wrapping_add(if *add_stone { p.stone_above } else { 0 })
                        >= p.water_height
                            .wrapping_add(*offset)
                            .wrapping_add(p.surface_depth.wrapping_mul(*multiplier))
            }
            Self::Stone {
                offset,
                add_surface,
                secondary,
                ceiling,
            } => {
                let depth = if *ceiling {
                    p.stone_below
                } else {
                    p.stone_above
                };
                let add = if *add_surface { p.surface_depth } else { 0 };
                let secondary = if *secondary == 0 {
                    0
                } else {
                    (((p.surface_secondary - (-1.0)) / (1.0 - (-1.0))) * f64::from(*secondary))
                        as i32
                };
                depth
                    <= 1i32
                        .wrapping_add(*offset)
                        .wrapping_add(add)
                        .wrapping_add(secondary)
            }
            Self::Not(condition) => !condition.test(p, inputs)?,
        })
    }
}

enum RuleNode {
    Block(Arc<Value>),
    Sequence(Vec<Arc<RuleNode>>),
    Conditional(Arc<Condition>, Arc<RuleNode>),
    OreVein(OreVein),
    Bandlands,
}
struct OreVein {
    ore: Arc<Value>,
    raw: Arc<Value>,
    filler: Arc<Value>,
    raw_chance: f32,
    density: Value,
    richness: Value,
    gap: Value,
}
impl RuleNode {
    fn collect_biomes(&self, out: &mut Vec<Value>) {
        match self {
            Self::Sequence(rules) => {
                for rule in rules {
                    rule.collect_biomes(out);
                }
            }
            Self::Conditional(condition, rule) => {
                condition.collect_biomes(out);
                rule.collect_biomes(out);
            }
            _ => {}
        }
    }
    fn collect_noises(&self, out: &mut BTreeSet<ResourceId>) {
        match self {
            Self::Sequence(rules) => {
                for rule in rules {
                    rule.collect_noises(out)
                }
            }
            Self::Conditional(condition, rule) => {
                condition.collect_noises(out);
                rule.collect_noises(out);
            }
            Self::Bandlands => {
                out.insert(
                    ResourceId::parse("minecraft:clay_bands_offset").expect("valid identifier"),
                );
            }
            _ => {}
        }
    }
    fn apply(
        &self,
        p: &SurfacePosition,
        inputs: &mut impl SurfaceInputs,
    ) -> Result<Option<Arc<Value>>> {
        match self {
            Self::Block(state) => Ok(Some(Arc::clone(state))),
            Self::Sequence(sequence) => {
                for rule in sequence {
                    if let Some(state) = rule.apply(p, inputs)? {
                        return Ok(Some(state));
                    }
                }
                Ok(None)
            }
            Self::Conditional(condition, rule) => {
                if condition.test(p, inputs)? {
                    rule.apply(p, inputs)
                } else {
                    Ok(None)
                }
            }
            Self::OreVein(rule) => {
                let density = inputs.density(&rule.density, true, p)?;
                if density <= 0.0 {
                    return Ok(None);
                }
                let mut random = inputs.ore_random(p)?;
                if random.next_float() > density {
                    return Ok(None);
                }
                let richness = inputs.density(&rule.richness, true, p)?;
                if random.next_float() < richness && inputs.density(&rule.gap, false, p)? < 0.0 {
                    return Ok(Some(Arc::clone(if random.next_float() < rule.raw_chance {
                        &rule.raw
                    } else {
                        &rule.ore
                    })));
                }
                Ok(Some(Arc::clone(&rule.filler)))
            }
            Self::Bandlands => Ok(Some(inputs.bandlands(p)?)),
        }
    }
}

pub struct SurfaceRule(Arc<RuleNode>);
impl SurfaceRule {
    /// Biome holder sets referenced by this material rule tree.
    pub fn biome_dependencies(&self) -> Vec<Value> {
        let mut result = Vec::new();
        self.0.collect_biomes(&mut result);
        result
    }
    /// Registry noise names used by conditions and bandlands in this rule tree.
    pub fn noise_dependencies(&self) -> BTreeSet<ResourceId> {
        let mut result = BTreeSet::new();
        self.0.collect_noises(&mut result);
        result
    }
    pub fn evaluate(
        &self,
        position: &SurfacePosition,
        inputs: &mut impl SurfaceInputs,
    ) -> Result<Option<Arc<Value>>> {
        self.0.apply(position, inputs)
    }
}

/// A seeded material-rule context for a fresh volume without old-chunk
/// blending. Biome identity is supplied by the caller.
pub struct SeededSurfaceInputs<'a, P: ResourceProvider> {
    biomes: crate::surface_biomes::SurfaceBiomes,
    biome: Option<ResourceId>,
    noise: SurfaceNoise,
    random: SurfaceRandomFactory,
    bands: SurfaceBands,
    badlands: BadlandsPillars,
    icebergs: FrozenOceanIcebergs,
    density: SurfaceDensitySampler<'a, P>,
    ore: ResourceId,
}
impl<'a, P: ResourceProvider> SeededSurfaceInputs<'a, P> {
    /// The caller must ensure this volume has no terrain adjustments.
    pub fn without_terrain_adjustments(
        provider: &'a mut P,
        rule: &SurfaceRule,
        seed: i64,
        algorithm: WorldgenRandom,
        min: [i32; 3],
        size: [u32; 3],
    ) -> Result<Self> {
        Self::build(
            provider, rule, seed, algorithm, min, size, true, false, None,
        )
    }
    /// Bind the same structure adjustment used by this chunk's noise stage.
    pub fn with_structure_context(
        provider: &'a mut P,
        rule: &SurfaceRule,
        seed: i64,
        algorithm: WorldgenRandom,
        min: [i32; 3],
        size: [u32; 3],
        structure: Option<Arc<Beardifier>>,
    ) -> Result<Self> {
        Self::build(
            provider, rule, seed, algorithm, min, size, true, true, structure,
        )
    }
    /// Build the single-block context used to repair surface material after
    /// carving. Biome pillar and iceberg extensions do not run in this pass.
    pub fn for_carver_top_material(
        provider: &'a mut P,
        rule: &SurfaceRule,
        seed: i64,
        algorithm: WorldgenRandom,
    ) -> Result<Self> {
        Self::build(
            provider,
            rule,
            seed,
            algorithm,
            [0, 0, 0],
            [1, 1, 1],
            false,
            false,
            None,
        )
    }
    pub fn for_carver_top_material_with_structure_context(
        provider: &'a mut P,
        rule: &SurfaceRule,
        seed: i64,
        algorithm: WorldgenRandom,
        structure: Option<Arc<Beardifier>>,
    ) -> Result<Self> {
        Self::build(
            provider,
            rule,
            seed,
            algorithm,
            [0, 0, 0],
            [1, 1, 1],
            false,
            true,
            structure,
        )
    }
    fn build(
        provider: &'a mut P,
        rule: &SurfaceRule,
        seed: i64,
        algorithm: WorldgenRandom,
        min: [i32; 3],
        size: [u32; 3],
        extensions: bool,
        structure_context: bool,
        structure: Option<Arc<Beardifier>>,
    ) -> Result<Self> {
        let biomes =
            crate::surface_biomes::SurfaceBiomes::load(provider, &rule.biome_dependencies())?;
        let badlands = BadlandsPillars::default();
        let icebergs = FrozenOceanIcebergs::default();
        let mut noises = rule.noise_dependencies();
        if extensions {
            noises.extend(badlands.noise_dependencies());
            noises.extend(icebergs.noise_dependencies());
        }
        let noise = SurfaceNoise::load(provider, seed, algorithm, noises)?;
        let random = SurfaceRandomFactory::new(seed, algorithm);
        let bands = SurfaceBands::new(seed, algorithm);
        let density = if structure_context {
            SurfaceDensitySampler::with_structure_context(
                provider, seed, algorithm, min, size, structure,
            )?
        } else {
            SurfaceDensitySampler::without_terrain_adjustments(
                provider, seed, algorithm, min, size,
            )?
        };
        Ok(Self {
            biomes,
            biome: None,
            noise,
            random,
            bands,
            badlands,
            icebergs,
            density,
            ore: ResourceId::parse("minecraft:ore")?,
        })
    }
    pub fn set_biome(&mut self, biome: ResourceId) -> Result<()> {
        ensure!(
            self.biomes.contains_biome(&biome),
            "selected biome is not in the active registry"
        );
        self.biome = Some(biome);
        Ok(())
    }
    pub fn set_single_position(&mut self, position: [i32; 3]) -> Result<()> {
        self.density.set_single_position(position)
    }
    pub fn surface_depth(&mut self, x: i32, z: i32) -> Result<i32> {
        self.noise.surface_depth(x, z)
    }
    pub fn surface_secondary(&mut self, x: i32, z: i32) -> Result<f64> {
        self.noise.surface_secondary(x, z)
    }
    pub fn badlands_peak(&mut self, x: i32, z: i32) -> Result<Option<i32>> {
        self.badlands.peak(&mut self.noise, x, z)
    }
    pub fn iceberg_bounds(
        &mut self,
        biome: &ResourceId,
        x: i32,
        z: i32,
        sea: i32,
    ) -> Result<Option<(f64, f64)>> {
        let melts = self.biomes.iceberg_melts(biome, x, z, sea)?;
        self.icebergs.bounds(&mut self.noise, x, z, sea, melts)
    }
    pub fn iceberg_random(&self, x: i32, z: i32) -> SurfaceRandom {
        self.random.root_at(x, 0, z)
    }
}
impl<P: ResourceProvider> SurfaceInputs for SeededSurfaceInputs<'_, P> {
    fn biome_matches(&mut self, selector: &Value, _: &SurfacePosition) -> Result<bool> {
        self.biomes.matches(
            selector,
            self.biome
                .as_ref()
                .context("surface biome was not selected")?,
        )
    }
    fn cold_enough_to_snow(&mut self, p: &SurfacePosition) -> Result<bool> {
        self.biomes.cold_enough_to_snow(
            self.biome
                .as_ref()
                .context("surface biome was not selected")?,
            p,
        )
    }
    fn noise(&mut self, name: &ResourceId, is_3d: bool, p: &SurfacePosition) -> Result<f64> {
        Ok(f64::from(self.noise.sample(name, is_3d, p.x, p.y, p.z)?))
    }
    fn positional_random(&mut self, name: &ResourceId, p: &SurfacePosition) -> Result<f32> {
        Ok(self.random.at(name, p.x, p.y, p.z).next_float())
    }
    fn ore_random(&mut self, p: &SurfacePosition) -> Result<SurfaceRandom> {
        Ok(self.random.at(&self.ore, p.x, p.y, p.z))
    }
    fn density(&mut self, definition: &Value, in_chunk: bool, p: &SurfacePosition) -> Result<f32> {
        self.density.sample(definition, in_chunk, p.x, p.y, p.z)
    }
    fn bandlands(&mut self, p: &SurfacePosition) -> Result<Arc<Value>> {
        self.bands.at(&mut self.noise, p.x, p.y, p.z)
    }
}

/// Resolves active data-pack rule and condition registries, sharing references.
pub struct SurfaceRuleCompiler<'a, P> {
    provider: &'a mut P,
    rules: BTreeMap<ResourceId, Arc<RuleNode>>,
    conditions: BTreeMap<ResourceId, Arc<Condition>>,
    active: BTreeSet<(bool, ResourceId)>,
    nodes: usize,
}
impl<'a, P: ResourceProvider> SurfaceRuleCompiler<'a, P> {
    pub fn new(provider: &'a mut P) -> Self {
        Self {
            provider,
            rules: BTreeMap::new(),
            conditions: BTreeMap::new(),
            active: BTreeSet::new(),
            nodes: 0,
        }
    }
    pub fn compile(&mut self, value: &Value) -> Result<SurfaceRule> {
        Ok(SurfaceRule(self.rule(value, 0)?))
    }
    fn enter(&mut self, depth: usize) -> Result<()> {
        self.nodes += 1;
        ensure!(
            depth <= MAX_DEPTH && self.nodes <= MAX_NODES,
            "material rule exceeds complexity limit"
        );
        Ok(())
    }
    fn reference(&mut self, name: &str, is_rule: bool) -> Result<Value> {
        let id = ResourceId::parse(name)?;
        ensure!(
            self.active.insert((is_rule, id.clone())),
            "cyclic material registry reference: {name}"
        );
        let path = format!(
            "data/{}/worldgen/material_{}/{}.json",
            id.namespace,
            if is_rule { "rule" } else { "condition" },
            id.path
        );
        let result = (|| {
            let bytes = self
                .provider
                .read_data(&path)?
                .with_context(|| format!("missing material definition {name}"))?;
            ensure!(
                bytes.len() <= 16 * 1024 * 1024,
                "material definition too large"
            );
            Ok(serde_json::from_slice(&bytes)?)
        })();
        if result.is_err() {
            self.active.remove(&(is_rule, id));
        }
        result
    }
    fn rule(&mut self, value: &Value, depth: usize) -> Result<Arc<RuleNode>> {
        self.enter(depth)?;
        if let Some(name) = value.as_str() {
            let id = ResourceId::parse(name)?;
            if let Some(rule) = self.rules.get(&id) {
                return Ok(Arc::clone(rule));
            }
            let def = self.reference(name, true)?;
            let result = self.rule(&def, depth + 1);
            self.active.remove(&(true, id.clone()));
            let rule = result.with_context(|| format!("in material rule {name}"))?;
            self.rules.insert(id, Arc::clone(&rule));
            return Ok(rule);
        }
        let obj = value
            .as_object()
            .context("material rule must be an object or identifier")?;
        let kind = kind(value)?;
        let node = match kind.path.as_str() {
            "block" => RuleNode::Block(block_state(required(value, "result_state")?)?),
            "sequence" => {
                let seq = obj
                    .get("sequence")
                    .and_then(Value::as_array)
                    .context("invalid material sequence")?;
                RuleNode::Sequence(
                    seq.iter()
                        .map(|v| self.rule(v, depth + 1))
                        .collect::<Result<_>>()?,
                )
            }
            "condition" => RuleNode::Conditional(
                self.condition(required(value, "if_true")?, depth + 1)?,
                self.rule(required(value, "then_run")?, depth + 1)?,
            ),
            "bandlands" => RuleNode::Bandlands,
            "ore_vein" => {
                let raw_chance = number(value, "raw_ore_chance")? as f32;
                ensure!((0.0..=1.0).contains(&raw_chance), "invalid raw ore chance");
                RuleNode::OreVein(OreVein {
                    ore: block_state(required(value, "ore_block")?)?,
                    raw: block_state(required(value, "raw_ore_block")?)?,
                    filler: block_state(required(value, "filler_block")?)?,
                    raw_chance,
                    density: required(value, "density")?.clone(),
                    richness: required(value, "richness")?.clone(),
                    gap: required(value, "filler_gap")?.clone(),
                })
            }
            _ => bail!(
                "unsupported material rule type {}:{}",
                kind.namespace,
                kind.path
            ),
        };
        Ok(Arc::new(node))
    }
    fn condition(&mut self, value: &Value, depth: usize) -> Result<Arc<Condition>> {
        self.enter(depth)?;
        if let Some(name) = value.as_str() {
            let id = ResourceId::parse(name)?;
            if let Some(condition) = self.conditions.get(&id) {
                return Ok(Arc::clone(condition));
            }
            let def = self.reference(name, false)?;
            let result = self.condition(&def, depth + 1);
            self.active.remove(&(false, id.clone()));
            let condition = result.with_context(|| format!("in material condition {name}"))?;
            self.conditions.insert(id, Arc::clone(&condition));
            return Ok(condition);
        }
        let kind = kind(value)?;
        let condition = match kind.path.as_str() {
            "biome" => Condition::Biome(required(value, "biome_is")?.clone()),
            "temperature" => Condition::Temperature,
            "above_preliminary_surface" => Condition::AboveSurface,
            "hole" => Condition::Hole,
            "steep" => Condition::Steep,
            "not" => Condition::Not(self.condition(required(value, "invert")?, depth + 1)?),
            "noise_threshold" => Condition::Noise {
                name: identifier(value, "noise")?,
                min: number(value, "min_threshold")?,
                max: number(value, "max_threshold")?,
                is_3d: value
                    .get("is_3d")
                    .map(|v| v.as_bool().context("invalid is_3d"))
                    .transpose()?
                    .unwrap_or(false),
            },
            "vertical_gradient" => Condition::Gradient {
                name: identifier(value, "random_name")?,
                low: Anchor::parse(required(value, "true_at_and_below")?)?,
                high: Anchor::parse(required(value, "false_at_and_above")?)?,
            },
            "y_above" => Condition::Y {
                anchor: Anchor::parse(required(value, "anchor")?)?,
                multiplier: integer(value, "surface_depth_multiplier")?,
                add_stone: boolean(value, "add_stone_depth")?,
            },
            "water" => Condition::Water {
                offset: integer(value, "offset")?,
                multiplier: integer(value, "surface_depth_multiplier")?,
                add_stone: boolean(value, "add_stone_depth")?,
            },
            "stone_depth" => Condition::Stone {
                offset: integer(value, "offset")?,
                add_surface: boolean(value, "add_surface_depth")?,
                secondary: integer(value, "secondary_depth_range")?,
                ceiling: match required(value, "surface_type")?.as_str() {
                    Some("floor") => false,
                    Some("ceiling") => true,
                    _ => bail!("invalid surface_type"),
                },
            },
            _ => bail!(
                "unsupported material condition type {}:{}",
                kind.namespace,
                kind.path
            ),
        };
        Ok(Arc::new(condition))
    }
}
fn required<'a>(value: &'a Value, name: &str) -> Result<&'a Value> {
    value.get(name).with_context(|| format!("missing {name}"))
}
fn block_state(state: &Value) -> Result<Arc<Value>> {
    let name = state
        .as_str()
        .or_else(|| state.get("id").and_then(Value::as_str))
        .context("invalid block state")?;
    ResourceId::parse(name)?;
    Ok(Arc::new(state.clone()))
}
fn integer(value: &Value, name: &str) -> Result<i32> {
    i32::try_from(
        required(value, name)?
            .as_i64()
            .with_context(|| format!("invalid integer {name}"))?,
    )
    .with_context(|| format!("integer {name} out of range"))
}
fn boolean(value: &Value, name: &str) -> Result<bool> {
    required(value, name)?
        .as_bool()
        .with_context(|| format!("invalid boolean {name}"))
}
fn number(value: &Value, name: &str) -> Result<f64> {
    required(value, name)?
        .as_f64()
        .with_context(|| format!("invalid number {name}"))
}
fn identifier(value: &Value, name: &str) -> Result<ResourceId> {
    ResourceId::parse(
        required(value, name)?
            .as_str()
            .with_context(|| format!("invalid identifier {name}"))?,
    )
}
fn kind(value: &Value) -> Result<ResourceId> {
    let id = identifier(value, "type")?;
    ensure!(
        id.namespace == "minecraft",
        "unsupported material type namespace {}",
        id.namespace
    );
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    struct Data(BTreeMap<String, Vec<u8>>);
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.0.get(path).cloned())
        }
    }
    struct Inputs;
    impl SurfaceInputs for Inputs {
        fn biome_matches(&mut self, _: &Value, _: &SurfacePosition) -> Result<bool> {
            Ok(false)
        }
        fn cold_enough_to_snow(&mut self, _: &SurfacePosition) -> Result<bool> {
            Ok(false)
        }
        fn noise(&mut self, _: &ResourceId, _: bool, _: &SurfacePosition) -> Result<f64> {
            Ok(0.0)
        }
        fn positional_random(&mut self, _: &ResourceId, _: &SurfacePosition) -> Result<f32> {
            Ok(0.5)
        }
        fn ore_random(&mut self, _: &SurfacePosition) -> Result<SurfaceRandom> {
            bail!("unexpected ore random")
        }
        fn density(&mut self, _: &Value, _: bool, _: &SurfacePosition) -> Result<f32> {
            bail!("unexpected density")
        }
        fn bandlands(&mut self, _: &SurfacePosition) -> Result<Arc<Value>> {
            bail!("bandlands unavailable")
        }
    }
    fn position() -> SurfacePosition {
        SurfacePosition {
            x: 0,
            y: 5,
            z: 0,
            min_y: -64,
            height: 384,
            sea_level: 63,
            stone_above: 1,
            stone_below: 2,
            surface_depth: 3,
            surface_secondary: 0.0,
            water_height: i32::MIN,
            min_surface_level: 0,
            gradient_x: 0,
            gradient_z: 0,
        }
    }
    #[test]
    fn registered_rules_are_ordered_and_cycles_fail() {
        let mut data=Data(BTreeMap::from([
            ("data/minecraft/worldgen/material_rule/first.json".into(),serde_json::to_vec(&json!({"type":"condition","if_true":"minecraft:floor","then_run":{"type":"block","result_state":"minecraft:grass_block"}})).unwrap()),
            ("data/minecraft/worldgen/material_condition/floor.json".into(),serde_json::to_vec(&json!({"type":"stone_depth","offset":0,"add_surface_depth":false,"secondary_depth_range":0,"surface_type":"floor"})).unwrap()),
            ("data/minecraft/worldgen/material_rule/cycle.json".into(),br#""minecraft:cycle""#.to_vec()),
        ]));
        let mut compiler = SurfaceRuleCompiler::new(&mut data);
        let rule=compiler.compile(&json!({"type":"sequence","sequence":["minecraft:first",{"type":"block","result_state":"minecraft:dirt"}]})).unwrap();
        assert_eq!(
            *rule.evaluate(&position(), &mut Inputs).unwrap().unwrap(),
            json!("minecraft:grass_block")
        );
        let mut p = position();
        p.stone_above = 2;
        assert_eq!(
            *rule.evaluate(&p, &mut Inputs).unwrap().unwrap(),
            json!("minecraft:dirt")
        );
        assert!(compiler.compile(&json!("minecraft:cycle")).is_err());
        assert!(compiler.compile(&json!({"type":"other:unknown"})).is_err());
        let noisy=compiler.compile(&json!({"type":"condition","if_true":{"type":"not","invert":{"type":"noise_threshold","noise":"example:test","min_threshold":0,"max_threshold":1}},"then_run":{"type":"block","result_state":"stone"}})).unwrap();
        assert_eq!(
            noisy.noise_dependencies(),
            BTreeSet::from([ResourceId::parse("example:test").unwrap()])
        );
    }

    #[test]
    fn ore_rule_samples_only_required_inputs_in_java_order() {
        struct OreInputs {
            reads: Vec<(f32, bool)>,
            randoms: usize,
        }
        impl SurfaceInputs for OreInputs {
            fn biome_matches(&mut self, _: &Value, _: &SurfacePosition) -> Result<bool> {
                bail!("unexpected biome")
            }
            fn cold_enough_to_snow(&mut self, _: &SurfacePosition) -> Result<bool> {
                bail!("unexpected temperature")
            }
            fn noise(&mut self, _: &ResourceId, _: bool, _: &SurfacePosition) -> Result<f64> {
                bail!("unexpected noise")
            }
            fn positional_random(&mut self, _: &ResourceId, _: &SurfacePosition) -> Result<f32> {
                bail!("unexpected gradient")
            }
            fn ore_random(&mut self, _: &SurfacePosition) -> Result<SurfaceRandom> {
                self.randoms += 1;
                Ok(SurfaceRandom::Legacy(LegacyRandom::new(10)))
            }
            fn density(&mut self, v: &Value, in_chunk: bool, _: &SurfacePosition) -> Result<f32> {
                let v = v.as_f64().unwrap() as f32;
                self.reads.push((v, in_chunk));
                Ok(v)
            }
            fn bandlands(&mut self, _: &SurfacePosition) -> Result<Arc<Value>> {
                bail!("unexpected bandlands")
            }
        }
        let mut data = Data(BTreeMap::new());
        let mut compiler = SurfaceRuleCompiler::new(&mut data);
        let mut def = json!({"type":"ore_vein","density":0,"richness":1,"filler_gap":-1,"raw_ore_chance":1,"ore_block":"minecraft:stone","raw_ore_block":"minecraft:dirt","filler_block":"minecraft:granite"});
        let mut inputs = OreInputs {
            reads: vec![],
            randoms: 0,
        };
        assert!(
            compiler
                .compile(&def)
                .unwrap()
                .evaluate(&position(), &mut inputs)
                .unwrap()
                .is_none()
        );
        assert_eq!(inputs.reads, [(0.0, true)]);
        assert_eq!(inputs.randoms, 0);
        def["density"] = json!(1);
        inputs.reads.clear();
        assert_eq!(
            *compiler
                .compile(&def)
                .unwrap()
                .evaluate(&position(), &mut inputs)
                .unwrap()
                .unwrap(),
            json!("minecraft:dirt")
        );
        assert_eq!(inputs.reads, [(1.0, true), (1.0, true), (-1.0, false)]);
        assert_eq!(inputs.randoms, 1);
        def["raw_ore_chance"] = json!(1.1);
        assert!(compiler.compile(&def).is_err());
    }

    #[test]
    fn surface_density_sampler_validates_and_reuses_volume() {
        let mut data = Data(BTreeMap::new());
        assert!(
            SurfaceDensitySampler::without_terrain_adjustments(
                &mut data,
                0,
                WorldgenRandom::Legacy,
                [0; 3],
                [0, 1, 1]
            )
            .is_err()
        );
        let mut sampler = SurfaceDensitySampler::without_terrain_adjustments(
            &mut data,
            0,
            WorldgenRandom::Legacy,
            [-2, -4, 3],
            [4, 8, 4],
        )
        .unwrap();
        let def = json!({"type":"interpolated","cell_size_xz":4,"cell_size_y":8,
            "input":{"type":"gradient","axis":"y","from_coordinate":-16,"to_coordinate":16,"from_value":-1,"to_value":1}});
        let first = sampler.sample(&def, true, -1, -3, 4).unwrap();
        assert_eq!(
            first.to_bits(),
            sampler.sample(&def, true, -1, -3, 4).unwrap().to_bits()
        );
        assert_eq!(sampler.entries.len(), 1);
        assert_eq!(sampler.entries[&def].volume.as_ref().unwrap().len(), 128);
        let outside = sampler.sample(&def, true, 20, 5, 20).unwrap();
        assert_eq!(
            outside.to_bits(),
            sampler.sample(&def, false, 20, 5, 20).unwrap().to_bits()
        );
        assert!(sampler.set_single_position([0, 0, 0]).is_err());
        drop(sampler);
        let mut single = SurfaceDensitySampler::without_terrain_adjustments(
            &mut data,
            0,
            WorldgenRandom::Legacy,
            [0, 0, 0],
            [1, 1, 1],
        )
        .unwrap();
        let first = single.sample(&def, true, 0, 0, 0).unwrap();
        single.set_single_position([0, 8, 0]).unwrap();
        let second = single.sample(&def, true, 0, 8, 0).unwrap();
        assert_ne!(first.to_bits(), second.to_bits());
        assert_eq!(single.entries.len(), 1);
    }

    #[test]
    fn surface_density_uses_structure_for_volume_and_single_position() {
        use native_core::beardifier::{BoundingBox, RigidPiece, TerrainAdjustment};
        let mut data = Data(BTreeMap::new());
        let bounds = BoundingBox::new([0, 4, 0], [2, 6, 2]).unwrap();
        let adjustment = Arc::new(Beardifier::new(
            vec![RigidPiece {
                bounds,
                adjustment: TerrainAdjustment::Bury,
                ground_delta: 0,
            }],
            vec![],
            Some(BoundingBox::new([-24, -20, -24], [26, 30, 26]).unwrap()),
        ));
        let marker = json!({"type":"minecraft:beardifier"});
        let mut sampler = SurfaceDensitySampler::with_structure_context(
            &mut data,
            0,
            WorldgenRandom::Legacy,
            [0, 0, 0],
            [4, 8, 4],
            Some(Arc::clone(&adjustment)),
        )
        .unwrap();
        assert_eq!(sampler.sample(&marker, true, 1, 4, 1).unwrap(), 1.0);
        assert_eq!(sampler.sample(&marker, false, 1, 4, 1).unwrap(), 1.0);
        drop(sampler);
        let mut single = SurfaceDensitySampler::with_structure_context(
            &mut data,
            0,
            WorldgenRandom::Legacy,
            [0, 0, 0],
            [1, 1, 1],
            Some(adjustment),
        )
        .unwrap();
        single.set_single_position([1, 4, 1]).unwrap();
        assert_eq!(single.sample(&marker, true, 1, 4, 1).unwrap(), 1.0);
        single.set_single_position([20, 4, 20]).unwrap();
        assert_eq!(single.sample(&marker, true, 20, 4, 20).unwrap(), 0.0);
    }

    #[test]
    fn surface_noise_caches_2d_and_requires_definitions() {
        let params = br#"{"base_octave":-4}"#.to_vec();
        let mut data = Data(BTreeMap::from([
            (
                "data/minecraft/worldgen/noise/surface.json".into(),
                params.clone(),
            ),
            (
                "data/minecraft/worldgen/noise/surface_secondary.json".into(),
                params.clone(),
            ),
            ("data/example/worldgen/noise/test.json".into(), params),
        ]));
        let id = ResourceId::parse("example:test").unwrap();
        let mut noise =
            SurfaceNoise::load(&mut data, 42, WorldgenRandom::Xoroshiro, [id.clone()]).unwrap();
        let first = noise.sample(&id, false, -17, 9, 23).unwrap();
        assert_eq!(
            first.to_bits(),
            noise.sample(&id, false, -17, 100, 23).unwrap().to_bits()
        );
        assert_eq!(noise.noises[&id].last_2d, Some((-17, 23, first)));
        noise.sample(&id, true, -17, 9, 23).unwrap();
        assert!(noise.noises[&id].last_3d.is_some());
        assert!(
            noise
                .sample(
                    &ResourceId::parse("example:missing").unwrap(),
                    false,
                    0,
                    0,
                    0
                )
                .is_err()
        );
        assert!(
            SurfaceNoise::load(
                &mut data,
                42,
                WorldgenRandom::Legacy,
                [ResourceId::parse("example:missing").unwrap()]
            )
            .is_err()
        );
    }

    #[test]
    fn terracotta_bands_are_deterministic_and_rule_requires_context() {
        for algorithm in [WorldgenRandom::Legacy, WorldgenRandom::Xoroshiro] {
            let first = SurfaceBands::new(12345, algorithm);
            let same = SurfaceBands::new(12345, algorithm);
            let other = SurfaceBands::new(54321, algorithm);
            assert_eq!(first.bands(), same.bands());
            assert_ne!(first.bands(), other.bands());
            for band in first.bands() {
                assert!(band.id().ends_with("terracotta"));
            }
        }
        let mut data = Data(BTreeMap::new());
        let rule = SurfaceRuleCompiler::new(&mut data)
            .compile(&json!({"type":"bandlands"}))
            .unwrap();
        assert!(
            rule.noise_dependencies()
                .contains(&ResourceId::parse("minecraft:clay_bands_offset").unwrap())
        );
        assert!(rule.evaluate(&position(), &mut Inputs).is_err());
    }
}
