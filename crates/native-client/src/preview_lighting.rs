//! Static dimension lighting for the read-only world viewer.

use anyhow::{Context, Result, bail, ensure};
use native_persistence::world::{DimensionId, SavedDimensionType};
use native_resources::packs::{ResourceId, ResourceProvider};
use native_world::lighting::LightmapSettings;
use serde_json::Value;
use std::collections::BTreeMap;

/// Java GameRenderer.nightVisionScale, including the final 200-tick pulse.
pub fn night_vision_scale(duration: Option<i32>, partial: f32) -> f32 {
    let Some(duration) = duration else {
        return 0.0;
    };
    if duration == -1 || duration > 200 {
        return 1.0;
    }
    let angle = (duration as f32 - partial) * std::f32::consts::PI * 0.2_f32;
    0.7_f32 + native_session::physics::java_sin(f64::from(angle), false) * 0.3_f32
}

/// Java's lightmap extractor samples once per client tick, not once per frame.
#[derive(Default)]
pub struct NightVisionCache {
    key: Option<(u64, Option<i32>, u32, bool)>,
    factor: f32,
}

impl NightVisionCache {
    pub fn sample(
        &mut self,
        tick: u64,
        duration: Option<i32>,
        water_vision: f32,
        conduit: bool,
        partial: f32,
    ) -> f32 {
        let key = (tick, duration, water_vision.to_bits(), conduit);
        if self.key != Some(key) {
            self.factor = if duration.is_some() {
                night_vision_scale(duration, partial)
            } else if conduit {
                water_vision
            } else {
                0.0
            };
            self.key = Some(key);
        }
        self.factor
    }
}

pub fn apply_night_vision(mut settings: LightmapSettings, factor: f32) -> LightmapSettings {
    settings.night_vision_factor = factor;
    settings
}
/// Resolve the generator's sea level without substituting overworld defaults.
pub fn generator_sea_level(
    source: &mut impl ResourceProvider,
    dimension: &DimensionId,
    saved: Option<&BTreeMap<DimensionId, native_persistence::nbt::Tag>>,
) -> Result<Option<i32>> {
    use native_persistence::nbt::Tag;
    let path = format!(
        "data/{}/dimension/{}.json",
        dimension.namespace, dimension.path
    );
    let (kind, settings) = if let Some(bytes) = source.read_data(&path)? {
        let dimension: Value = serde_json::from_slice(&bytes)?;
        let generator = dimension.get("generator").context("missing generator")?;
        let kind = generator
            .get("type")
            .and_then(Value::as_str)
            .context("missing generator type")?
            .to_owned();
        let settings = (kind == "minecraft:noise" || kind == "noise")
            .then(|| generator.get("settings"))
            .flatten()
            .map(|value| {
                if let Some(name) = value.as_str() {
                    Ok(GeneratorSettings::Named(name.to_owned()))
                } else if let Some(level) = value.get("sea_level").and_then(Value::as_i64) {
                    Ok(GeneratorSettings::Inline(i32::try_from(level)?))
                } else {
                    bail!("unsupported inline noise settings")
                }
            })
            .transpose()?;
        (kind, settings)
    } else {
        let Some(generator) = saved.and_then(|saved| saved.get(dimension)) else {
            return Ok(None);
        };
        let Some(Tag::String(kind)) = generator.get("type") else {
            bail!("missing generator type")
        };
        let kind = String::from_utf16(&kind.0)?;
        let settings = match if kind == "minecraft:noise" || kind == "noise" {
            generator.get("settings")
        } else {
            None
        } {
            Some(Tag::String(name)) => Some(GeneratorSettings::Named(String::from_utf16(&name.0)?)),
            Some(value @ Tag::Compound(_)) => Some(GeneratorSettings::Inline(
                value
                    .get("sea_level")
                    .and_then(Tag::as_java_int)
                    .context("inline noise settings have no sea level")?,
            )),
            Some(_) => bail!("invalid generator settings"),
            None => None,
        };
        (kind, settings)
    };
    let kind = ResourceId::parse(&kind)?;
    if kind.namespace != "minecraft" {
        return Ok(None);
    }
    match kind.path.as_str() {
        "flat" => Ok(Some(-63)),
        "debug" => Ok(Some(63)),
        "noise" => match settings.context("noise generator has no settings")? {
            GeneratorSettings::Inline(level) => Ok(Some(level)),
            GeneratorSettings::Named(name) => {
                let id = ResourceId::parse(&name)?;
                let path = format!(
                    "data/{}/worldgen/noise_settings/{}.json",
                    id.namespace, id.path
                );
                let data = source
                    .read_data(&path)?
                    .with_context(|| format!("missing {path}"))?;
                let value: Value = serde_json::from_slice(&data)?;
                let level = value
                    .get("sea_level")
                    .and_then(Value::as_i64)
                    .context("noise settings have no sea level")?;
                Ok(Some(i32::try_from(level)?))
            }
        },
        _ => Ok(None),
    }
}

enum GeneratorSettings {
    Named(String),
    Inline(i32),
}

/// Java bakes the active data-pack dimension registry before falling back to
/// the level stems stored in world_gen_settings.dat.
pub fn resolve_dimension_type(
    source: &mut impl ResourceProvider,
    dimension: &DimensionId,
    saved: Option<&BTreeMap<DimensionId, SavedDimensionType>>,
) -> Result<Option<String>> {
    let path = format!(
        "data/{}/dimension/{}.json",
        dimension.namespace, dimension.path
    );
    if let Some(bytes) = source.read_data(&path)? {
        let value: Value = serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid dimension definition {path}"))?;
        ensure!(
            value.get("generator").is_some_and(Value::is_object),
            "dimension {path} has no generator"
        );
        let name = value.get("type").and_then(Value::as_str).with_context(|| {
            format!("inline or missing dimension type in {path} is not supported")
        })?;
        let id = ResourceId::parse(name)?;
        return Ok(Some(format!("{}:{}", id.namespace, id.path)));
    }
    match saved.and_then(|map| map.get(dimension)) {
        Some(SavedDimensionType::Named(id)) => Ok(Some(format!("{}:{}", id.namespace, id.path))),
        Some(SavedDimensionType::Inline(_)) => {
            bail!("inline saved dimension types are not supported")
        }
        None => Ok(None),
    }
}

#[derive(Clone, Debug)]
pub struct PreviewLighting {
    pub lightmap: LightmapSettings,
    pub has_skylight: bool,
    pub has_ceiling: Option<bool>,
    pub nether_cardinal: bool,
    pub build_bounds: Option<(i32, i32)>,
}

/// Level.canHaveWeather depends on the dimension key as well as its type.
pub fn can_have_weather(dimension: &DimensionId, profile: &PreviewLighting) -> Option<bool> {
    if !profile.has_skylight || (dimension.namespace == "minecraft" && dimension.path == "the_end")
    {
        return Some(false);
    }
    profile.has_ceiling.map(|ceiling| !ceiling)
}

/// Java ticks the overworld first. Other levels come from a HashMap whose
/// iteration order cannot be reconstructed from saved data alone; use a
/// stable identifier order for those levels until their registry is modeled.
pub fn weather_tick_order(
    source: &mut impl ResourceProvider,
    current: &DimensionId,
    saved: Option<&BTreeMap<DimensionId, SavedDimensionType>>,
) -> Result<Option<Vec<DimensionId>>> {
    let Some(saved) = saved else {
        return Ok(None);
    };
    let overworld = DimensionId::parse("minecraft:overworld")?;
    if !saved.contains_key(current) || !saved.contains_key(&overworld) {
        return Ok(None);
    }
    let mut dimensions: std::collections::BTreeSet<_> = saved.keys().cloned().collect();
    for id in source.list_data("dimension")? {
        dimensions.insert(DimensionId::parse(&format!(
            "{}:{}",
            id.namespace, id.path
        ))?);
    }
    let mut result = Vec::new();
    for id in dimensions {
        if id.namespace == "minecraft" && id.path == "the_end" {
            continue;
        }
        let resolved = resolve_dimension_type(source, &id, Some(saved))?;
        let name = format!("{}:{}", id.namespace, id.path);
        let profile = match resolved {
            Some(id) => PreviewLighting::load_type(source, &id)?,
            None => PreviewLighting::load(source, &name)?,
        };
        match can_have_weather(&id, &profile) {
            Some(true) => result.push(id),
            Some(false) => {}
            None => return Ok(None),
        }
    }
    if let Some(index) = result.iter().position(|id| *id == overworld) {
        let overworld = result.remove(index);
        result.insert(0, overworld);
    }
    Ok(Some(result))
}

impl Default for PreviewLighting {
    fn default() -> Self {
        Self {
            lightmap: LightmapSettings::default(),
            has_skylight: true,
            has_ceiling: None,
            nether_cardinal: false,
            build_bounds: None,
        }
    }
}

pub(crate) fn rgb(value: &Value) -> Result<[f32; 3]> {
    let packed = if let Some(s) = value.as_str() {
        ensure!(s.len() == 7 && s.starts_with('#'), "color must be #RRGGBB");
        u32::from_str_radix(&s[1..], 16).context("invalid RGB color")?
    } else {
        let n = value
            .as_u64()
            .context("RGB color must be a string or integer")?;
        ensure!(n <= 0xff_ffff, "RGB color is out of range");
        n as u32
    };
    Ok([
        ((packed >> 16) & 255) as f32 / 255.0,
        ((packed >> 8) & 255) as f32 / 255.0,
        (packed & 255) as f32 / 255.0,
    ])
}

/// Time-dependent sky-light layers whose spatial inputs are constant.
pub struct SkyTimeline {
    tracks: Vec<SkyTrack>,
}

pub(crate) struct SkyTrack {
    pub(crate) clock: ResourceId,
    period: Option<i32>,
    factor: bool,
    pub(crate) multiply: bool,
    easing: TrackEasing,
    frames: Vec<(i32, [f32; 3])>,
}

#[derive(Clone, Copy)]
enum TrackEasing {
    Linear,
    Constant,
    Cubic { x: [f32; 3], y: [f32; 3] },
}

impl TrackEasing {
    fn parse(value: Option<&Value>) -> Result<Self> {
        match value {
            None => Ok(Self::Linear),
            Some(Value::String(name)) if name == "linear" => Ok(Self::Linear),
            Some(Value::String(name)) if name == "constant" => Ok(Self::Constant),
            Some(value) => {
                let controls = value
                    .get("cubic_bezier")
                    .and_then(Value::as_array)
                    .context("unsupported easing")?;
                ensure!(controls.len() == 4, "invalid cubic easing controls");
                let mut c = [0.0_f32; 4];
                for (out, value) in c.iter_mut().zip(controls) {
                    *out = value.as_f64().context("invalid cubic control")? as f32;
                    ensure!(out.is_finite(), "non-finite cubic control");
                }
                ensure!(
                    (0.0..=1.0).contains(&c[0]) && (0.0..=1.0).contains(&c[2]),
                    "invalid cubic x control"
                );
                let curve = |a, b| [3.0 * a - 3.0 * b + 1.0, -6.0 * a + 3.0 * b, 3.0 * a];
                Ok(Self::Cubic {
                    x: curve(c[0], c[2]),
                    y: curve(c[1], c[3]),
                })
            }
        }
    }

    fn apply(self, input: f32) -> f32 {
        match self {
            Self::Linear => input,
            Self::Constant => 0.0,
            Self::Cubic { x, y } => {
                let sample =
                    |curve: [f32; 3], t: f32| ((curve[0] * t + curve[1]) * t + curve[2]) * t;
                let mut t = input;
                for _ in 0..4 {
                    let error = sample(x, t) - input;
                    if error.abs() < 1.0e-5 {
                        return sample(y, t);
                    }
                    let gradient = (3.0 * x[0] * t + 2.0 * x[1]) * t + x[2];
                    if gradient < 1.0e-5 {
                        break;
                    }
                    t -= (error / gradient).clamp(-0.25, 0.25);
                }
                let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
                while lo < hi {
                    let error = sample(x, t) - input;
                    if error.abs() < 1.0e-5 {
                        break;
                    }
                    if error < 0.0 {
                        lo = t;
                    } else {
                        hi = t;
                    }
                    let next = (hi + lo) / 2.0;
                    if next == t {
                        break;
                    }
                    t = next;
                }
                sample(y, t)
            }
        }
    }
}

impl SkyTimeline {
    pub fn is_animated(&self) -> bool {
        !self.tracks.is_empty()
    }
    pub fn load(source: &mut impl ResourceProvider, dimension_type: &str) -> Result<Self> {
        for id in source.list_data("worldgen/biome")? {
            let path = format!("data/{}/worldgen/biome/{}.json", id.namespace, id.path);
            let bytes = source
                .read_data(&path)?
                .with_context(|| format!("missing {path}"))?;
            let biome: Value =
                serde_json::from_slice(&bytes).with_context(|| format!("invalid {path}"))?;
            if let Some(attributes) = biome.get("attributes") {
                let attributes = attributes.as_object().context("invalid biome attributes")?;
                for name in attributes.keys() {
                    let id = ResourceId::parse(name)?;
                    ensure!(
                        id.namespace != "minecraft"
                            || !matches!(
                                id.path.as_str(),
                                "visual/sky_light_color"
                                    | "visual/sky_light_factor"
                                    | "visual/block_light_tint"
                                    | "visual/ambient_light_color"
                                    | "visual/night_vision_color"
                            ),
                        "biome-dependent lightmap attributes are not yet supported"
                    );
                }
            }
        }
        let timelines =
            native_resources::fluid_environment::load_dimension_timelines(source, dimension_type)?;
        let mut result = Self { tracks: Vec::new() };
        for timeline in timelines {
            let tracks = timeline
                .get("tracks")
                .and_then(Value::as_object)
                .context("invalid timeline tracks")?;
            let mut seen = std::collections::BTreeSet::new();
            for (name, value) in tracks {
                let id = ResourceId::parse(name)?;
                ensure!(
                    seen.insert(id.clone()),
                    "duplicate timeline attribute {name}"
                );
                if id.namespace != "minecraft" {
                    continue;
                }
                match id.path.as_str() {
                    "visual/sky_light_factor" => {
                        result.tracks.push(SkyTrack::parse(&timeline, value, true)?)
                    }
                    "visual/sky_light_color" => result
                        .tracks
                        .push(SkyTrack::parse(&timeline, value, false)?),
                    "visual/block_light_tint"
                    | "visual/ambient_light_color"
                    | "visual/night_vision_color" => bail!("unsupported lightmap timeline {name}"),
                    _ => {}
                }
            }
        }
        Ok(result)
    }

    pub fn sample(
        &self,
        base: &LightmapSettings,
        mut clock_ticks: impl FnMut(&ResourceId) -> Option<i64>,
    ) -> Option<LightmapSettings> {
        let mut result = base.clone();
        for track in &self.tracks {
            let value = track.sample(clock_ticks(&track.clock)?);
            if track.factor {
                result.sky_factor = if track.multiply {
                    result.sky_factor * value[0]
                } else {
                    value[0]
                };
            } else {
                for (channel, value) in result.sky_color.iter_mut().zip(value) {
                    *channel = if track.multiply {
                        *channel * value
                    } else {
                        value
                    };
                }
            }
        }
        result.sky_factor = result.sky_factor.clamp(0.0, 1.0);
        Some(result)
    }
}

impl SkyTrack {
    pub(crate) fn parse(timeline: &Value, track: &Value, factor: bool) -> Result<Self> {
        let clock = ResourceId::parse(
            timeline
                .get("clock")
                .and_then(Value::as_str)
                .context("missing clock")?,
        )?;
        let period = timeline
            .get("period_ticks")
            .map(|v| {
                let n = v
                    .as_i64()
                    .and_then(|n| i32::try_from(n).ok())
                    .context("invalid period")?;
                ensure!(n > 0, "invalid period");
                Ok(n)
            })
            .transpose()?;
        let modifier = match track.get("modifier") {
            None => "override",
            Some(value) => value.as_str().context("invalid sky light modifier")?,
        };
        let multiply = match modifier {
            "override" | "minecraft:override" => false,
            "multiply" | "minecraft:multiply" => true,
            _ => bail!("unsupported sky light modifier"),
        };
        let easing = TrackEasing::parse(track.get("ease"))?;
        let values = track
            .get("keyframes")
            .and_then(Value::as_array)
            .context("missing keyframes")?;
        ensure!(!values.is_empty(), "empty keyframes");
        let mut frames: Vec<(i32, [f32; 3])> = Vec::with_capacity(values.len());
        for frame in values {
            let tick = frame
                .get("ticks")
                .and_then(Value::as_i64)
                .and_then(|n| i32::try_from(n).ok())
                .context("invalid keyframe tick")?;
            if let Some(period) = period {
                ensure!((0..=period).contains(&tick), "keyframe outside period");
            }
            if let Some((previous, _)) = frames.last() {
                ensure!(tick >= *previous, "unordered keyframes");
            }
            if frames.len() >= 2 {
                ensure!(
                    frames[frames.len() - 2].0 != tick,
                    "three equal keyframe ticks"
                );
            }
            let value = frame.get("value").context("missing keyframe value")?;
            let value = if factor {
                [value.as_f64().context("invalid sky factor")? as f32; 3]
            } else {
                rgb(value)?
            };
            ensure!(
                value.iter().all(|v| v.is_finite()),
                "non-finite sky keyframe"
            );
            frames.push((tick, value));
        }
        Ok(Self {
            clock,
            period,
            factor,
            multiply,
            easing,
            frames,
        })
    }

    pub(crate) fn sample(&self, ticks: i64) -> [f32; 3] {
        let ticks = self
            .period
            .map_or(ticks, |p| ticks.rem_euclid(i64::from(p)));
        let first = self.frames[0];
        let last = self.frames[self.frames.len() - 1];
        if self.frames.len() == 1 {
            return first.1;
        }
        let before = self
            .period
            .map(|p| ((last.0.wrapping_sub(p), last.1), first));
        let after = self
            .period
            .map(|p| (last, (first.0.wrapping_add(p), first.1)));
        let segments = before
            .into_iter()
            .chain(self.frames.windows(2).map(|w| (w[0], w[1])))
            .chain(after);
        let ((start, a), (end, b)) = segments
            .clone()
            .find(|(_, end)| ticks < i64::from(end.0))
            .unwrap_or_else(|| segments.last().unwrap());
        if ticks <= i64::from(start) {
            return a;
        }
        if ticks >= i64::from(end) {
            return b;
        }
        let delta = self
            .easing
            .apply(ticks.wrapping_sub(i64::from(start)) as f32 / end.wrapping_sub(start) as f32);
        std::array::from_fn(|i| a[i] + delta * (b[i] - a[i]))
    }
}

impl PreviewLighting {
    pub fn fallback(dimension: &str) -> Self {
        let mut result = Self::default();
        if dimension == "minecraft:the_nether" {
            result.has_skylight = false;
            result.nether_cardinal = true;
            result.lightmap.sky_factor = 0.0;
        } else if dimension == "minecraft:the_end" {
            result.lightmap.sky_factor = 0.0;
        }
        result
    }

    /// Fallback for worlds without a resolved dimension-type mapping.
    pub fn load(source: &mut impl ResourceProvider, dimension: &str) -> Result<Self> {
        let name = match dimension {
            "minecraft:overworld" => "overworld",
            "minecraft:the_nether" => "the_nether",
            "minecraft:the_end" => "the_end",
            _ => bail!("dimension-to-type mapping is not implemented for {dimension}"),
        };
        Self::load_type(source, &format!("minecraft:{name}"))
    }

    /// Read the resolved dimension type using active data-pack precedence.
    pub fn load_type(source: &mut impl ResourceProvider, dimension_type: &str) -> Result<Self> {
        let id = ResourceId::parse(dimension_type)?;
        let path = format!("data/{}/dimension_type/{}.json", id.namespace, id.path);
        let bytes = source
            .read_data(&path)?
            .with_context(|| format!("missing {path}"))?;
        Self::from_json(&bytes)
    }

    fn from_json(bytes: &[u8]) -> Result<Self> {
        let json: Value = serde_json::from_slice(bytes).context("invalid dimension type JSON")?;
        let has_skylight = json
            .get("has_skylight")
            .and_then(Value::as_bool)
            .context("dimension type is missing has_skylight")?;
        let has_ceiling = json
            .get("has_ceiling")
            .map(|value| {
                value
                    .as_bool()
                    .context("dimension has_ceiling must be a boolean")
            })
            .transpose()?;
        let build_bounds = match (json.get("min_y"), json.get("height")) {
            (None, None) => None,
            (Some(min), Some(height)) => {
                let min = min.as_i64().context("dimension min_y must be an integer")?;
                let height = height
                    .as_i64()
                    .context("dimension height must be an integer")?;
                let max = min
                    .checked_add(height)
                    .context("dimension height overflows")?;
                ensure!(
                    height > 0 && min % 16 == 0 && height % 16 == 0 && min >= -2048 && max <= 2048,
                    "dimension bounds are outside the supported section range"
                );
                Some((min as i32, max as i32))
            }
            _ => bail!("dimension min_y and height must both be present"),
        };
        let nether_cardinal = match json.get("cardinal_light") {
            None => false,
            Some(Value::String(value)) if value == "default" => false,
            Some(Value::String(value)) if value == "nether" => true,
            Some(value) => bail!("unsupported cardinal lighting {value}"),
        };
        let mut lightmap = LightmapSettings {
            ambient_color: [0.0; 3],
            ..LightmapSettings::default()
        };
        if let Some(attributes) = json.get("attributes") {
            let attributes = attributes
                .as_object()
                .context("dimension attributes must be an object")?;
            if let Some(value) = attributes.get("minecraft:visual/block_light_tint") {
                lightmap.block_tint = rgb(value)?;
            }
            if let Some(value) = attributes.get("minecraft:visual/sky_light_color") {
                lightmap.sky_color = rgb(value)?;
            }
            if let Some(value) = attributes.get("minecraft:visual/ambient_light_color") {
                lightmap.ambient_color = rgb(value)?;
            }
            if let Some(value) = attributes.get("minecraft:visual/night_vision_color") {
                lightmap.night_vision_color = rgb(value)?;
            }
            if let Some(value) = attributes.get("minecraft:visual/sky_light_factor") {
                let factor = value
                    .as_f64()
                    .context("sky light factor must be a number")?;
                ensure!(
                    (0.0..=1.0).contains(&factor),
                    "sky light factor is out of range"
                );
                lightmap.sky_factor = factor as f32;
            }
        }
        Ok(Self {
            lightmap,
            has_skylight,
            has_ceiling,
            nether_cardinal,
            build_bounds,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn night_vision_updates_lightmap_and_preserves_dimension_settings() {
        let base = LightmapSettings {
            sky_factor: 0.3,
            night_vision_color: [0.2, 0.4, 0.6],
            ..Default::default()
        };
        let normal = apply_night_vision(base.clone(), night_vision_scale(None, 0.5));
        let enhanced = apply_night_vision(base.clone(), night_vision_scale(Some(-1), 0.5));
        assert_eq!(normal, base);
        assert_eq!(enhanced.sky_factor, 0.3);
        assert_eq!(enhanced.night_vision_color, [0.2, 0.4, 0.6]);
        assert!(enhanced.sample(0, 0)[0] > normal.sample(0, 0)[0]);
        assert!(enhanced.sample(0, 0)[2] > enhanced.sample(0, 0)[0]);
        assert_eq!(
            apply_night_vision(base.clone(), night_vision_scale(Some(201), 0.0))
                .night_vision_factor,
            1.0
        );
        assert_ne!(
            apply_night_vision(base.clone(), night_vision_scale(Some(200), 0.0))
                .night_vision_factor,
            1.0
        );
        assert_eq!(
            apply_night_vision(base, night_vision_scale(None, 0.0)).night_vision_factor,
            0.0
        );
        let mut cache = NightVisionCache::default();
        let first = cache.sample(1, Some(42), 0.5, true, 0.1);
        assert_eq!(cache.sample(1, Some(42), 0.5, true, 0.9), first);
        assert_ne!(cache.sample(2, Some(41), 0.5, true, 0.9), first);
        assert_eq!(cache.sample(2, None, 0.5, false, 0.9), 0.0);
        assert_eq!(cache.sample(2, None, 0.5, true, 0.9), 0.5);
        assert_eq!(cache.sample(2, Some(-1), 0.5, true, 0.9), 1.0);
    }

    #[test]
    fn sea_level_uses_active_dimension_and_noise_settings() {
        use native_persistence::nbt::Tag;
        struct Data {
            dimension: Option<&'static [u8]>,
        }
        impl ResourceProvider for Data {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                Ok(match path {
                    "data/minecraft/dimension/overworld.json" => self.dimension.map(<[u8]>::to_vec),
                    "data/example/worldgen/noise_settings/custom.json" => {
                        Some(br#"{"sea_level":77}"#.to_vec())
                    }
                    _ => None,
                })
            }
        }
        let dimension = DimensionId::parse("minecraft:overworld").unwrap();
        let generator = Tag::Compound(vec![
            ("type".into(), Tag::String("minecraft:noise".into())),
            ("settings".into(), Tag::String("example:custom".into())),
        ]);
        let saved = BTreeMap::from([(dimension.clone(), generator)]);
        let mut data = Data { dimension: None };
        assert_eq!(
            generator_sea_level(&mut data, &dimension, Some(&saved)).unwrap(),
            Some(77)
        );
        data.dimension =
            Some(br#"{"generator":{"type":"minecraft:noise","settings":{"sea_level":21}}}"#);
        assert_eq!(
            generator_sea_level(&mut data, &dimension, Some(&saved)).unwrap(),
            Some(21)
        );
        data.dimension =
            Some(br#"{"generator":{"type":"minecraft:flat","settings":{"layers":[]}}}"#);
        assert_eq!(
            generator_sea_level(&mut data, &dimension, Some(&saved)).unwrap(),
            Some(-63)
        );
        data.dimension = Some(br#"{"generator":{"type":"example:unknown"}}"#);
        assert_eq!(
            generator_sea_level(&mut data, &dimension, Some(&saved)).unwrap(),
            None
        );
        data.dimension = None;
        assert_eq!(
            generator_sea_level(&mut data, &dimension, None).unwrap(),
            None
        );
    }

    #[test]
    fn weather_topology_orders_extra_levels_and_rejects_unknown_types() {
        struct Data {
            extra: bool,
            unknown: bool,
        }
        impl ResourceProvider for Data {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                Ok(match path {
                    "data/minecraft/dimension_type/overworld.json" => {
                        Some(br#"{"has_skylight":true,"has_ceiling":false}"#.to_vec())
                    }
                    "data/minecraft/dimension_type/the_nether.json" => {
                        Some(br#"{"has_skylight":false,"has_ceiling":true}"#.to_vec())
                    }
                    "data/example/dimension_type/sky.json" if self.unknown => {
                        Some(br#"{"has_skylight":true}"#.to_vec())
                    }
                    "data/example/dimension_type/sky.json" => {
                        Some(br#"{"has_skylight":true,"has_ceiling":false}"#.to_vec())
                    }
                    _ => None,
                })
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                Ok(if folder == "dimension" && self.extra {
                    vec![ResourceId::parse("example:sky")?]
                } else {
                    vec![]
                })
            }
        }
        let overworld = DimensionId::parse("minecraft:overworld").unwrap();
        let nether = DimensionId::parse("minecraft:the_nether").unwrap();
        let end = DimensionId::parse("minecraft:the_end").unwrap();
        let mut saved = BTreeMap::from([
            (
                overworld.clone(),
                SavedDimensionType::Named(overworld.clone()),
            ),
            (nether.clone(), SavedDimensionType::Named(nether.clone())),
            (end.clone(), SavedDimensionType::Named(end.clone())),
        ]);
        let mut data = Data {
            extra: false,
            unknown: false,
        };
        assert_eq!(
            weather_tick_order(&mut data, &overworld, Some(&saved)).unwrap(),
            Some(vec![overworld.clone()])
        );
        assert_eq!(
            weather_tick_order(&mut data, &nether, Some(&saved)).unwrap(),
            Some(vec![overworld.clone()])
        );
        assert_eq!(
            weather_tick_order(&mut data, &overworld, None).unwrap(),
            None
        );
        let extra = DimensionId::parse("example:sky").unwrap();
        saved.insert(extra.clone(), SavedDimensionType::Named(extra));
        data.extra = true;
        assert_eq!(
            weather_tick_order(&mut data, &overworld, Some(&saved)).unwrap(),
            Some(vec![
                overworld.clone(),
                DimensionId::parse("example:sky").unwrap()
            ])
        );
        let other = DimensionId::parse("example:other").unwrap();
        saved.insert(
            other.clone(),
            SavedDimensionType::Named(DimensionId::parse("example:sky").unwrap()),
        );
        assert_eq!(
            weather_tick_order(&mut data, &overworld, Some(&saved)).unwrap(),
            Some(vec![
                overworld.clone(),
                other.clone(),
                DimensionId::parse("example:sky").unwrap()
            ])
        );
        saved.remove(&other);
        data.unknown = true;
        assert_eq!(
            weather_tick_order(&mut data, &overworld, Some(&saved)).unwrap(),
            None
        );
        let profile =
            PreviewLighting::from_json(br#"{"has_skylight":true,"has_ceiling":false}"#).unwrap();
        assert_eq!(can_have_weather(&overworld, &profile), Some(true));
        assert_eq!(can_have_weather(&end, &profile), Some(false));
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3"]
    fn vanilla_dimension_set_has_one_weather_ticking_level() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut source = native_resources::packs::ResourceStack::open(&game).unwrap();
        let overworld = DimensionId::parse("minecraft:overworld").unwrap();
        let saved = [
            "minecraft:overworld",
            "minecraft:the_nether",
            "minecraft:the_end",
        ]
        .into_iter()
        .map(|name| {
            let id = DimensionId::parse(name).unwrap();
            (id.clone(), SavedDimensionType::Named(id))
        })
        .collect();
        assert_eq!(
            weather_tick_order(&mut source, &overworld, Some(&saved)).unwrap(),
            Some(vec![overworld.clone()])
        );
        let nether = DimensionId::parse("minecraft:the_nether").unwrap();
        assert_eq!(
            weather_tick_order(&mut source, &nether, Some(&saved)).unwrap(),
            Some(vec![overworld])
        );
    }

    #[test]
    fn sky_timeline_uses_active_tracks_and_rejects_biome_overrides() {
        use serde_json::json;
        struct Data {
            biome: Value,
        }
        impl ResourceProvider for Data {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                let value = match path {
                    "data/example/worldgen/biome/test.json" => self.biome.clone(),
                    "data/example/dimension_type/test.json" => json!({"timelines":["example:day"]}),
                    "data/example/timeline/day.json" => {
                        json!({"clock":"example:clock","period_ticks":20,"tracks":{
                            "visual/sky_light_factor":{"modifier":"multiply","keyframes":[{"ticks":0,"value":1.0},{"ticks":10,"value":0.25}]},
                            "visual/sky_light_color":{"keyframes":[{"ticks":0,"value":"#ffffff"},{"ticks":10,"value":"#000000"}]}
                        }})
                    }
                    _ => return Ok(None),
                };
                Ok(Some(serde_json::to_vec(&value)?))
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                if folder == "worldgen/biome" {
                    Ok(vec![ResourceId::parse("example:test")?])
                } else {
                    Ok(vec![])
                }
            }
        }
        let mut data = Data { biome: json!({}) };
        let environment = SkyTimeline::load(&mut data, "example:test").unwrap();
        let base = LightmapSettings {
            sky_factor: 0.8,
            sky_color: [0.5; 3],
            ..Default::default()
        };
        let settings = environment.sample(&base, |_| Some(5)).unwrap();
        assert!((settings.sky_factor - 0.5).abs() < 0.000001);
        assert_eq!(settings.sky_color, [0.5; 3]);
        assert!(environment.sample(&base, |_| None).is_none());
        data.biome = json!({"attributes":{"minecraft:visual/sky_light_factor":0.5}});
        assert!(SkyTimeline::load(&mut data, "example:test").is_err());
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn sky_timeline_matches_installed_java_sampler() {
        use native_resources::packs::ResourceStack;
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft")
            });
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let mut resources = ResourceStack::open(&game).unwrap();
        let day: Value = serde_json::from_slice(
            &resources
                .read_data("data/minecraft/timeline/day.json")
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        let factor = &day["tracks"]["minecraft:visual/sky_light_factor"];
        let color = &day["tracks"]["minecraft:visual/sky_light_color"];
        let sun = &day["tracks"]["minecraft:visual/sun_angle"];
        let sunset = &day["tracks"]["minecraft:visual/sunrise_sunset_color"];
        let ticks: Vec<i64> = (-1000..=25000)
            .step_by(37)
            .chain([730, 11270, 13140, 22860, 0, 24000])
            .collect();
        let input = serde_json::json!({"factor":factor,"color":color,"sun":sun,"sunset":sunset,"ticks":ticks});
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("SkyTimelineOracle.java");
        let input_file = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/SkyTimelineOracle.java"),
        )
        .unwrap();
        std::fs::write(&input_file, serde_json::to_vec(&input).unwrap()).unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let run = |command: &mut Command| {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "Java sky oracle: {} {}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        };
        run(Command::new(java.join("javac"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source));
        run(Command::new(java.join("java"))
            .arg("-cp")
            .arg(&cp)
            .arg("SkyTimelineOracle")
            .arg(&input_file)
            .arg(&output));
        let oracle: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let environment = SkyTimeline {
            tracks: vec![
                SkyTrack::parse(&day, factor, true).unwrap(),
                SkyTrack::parse(&day, color, false).unwrap(),
            ],
        };
        let base = LightmapSettings {
            sky_factor: 1.0,
            sky_color: [1.0; 3],
            ..Default::default()
        };
        let sun = SkyTrack::parse(&day, sun, true).unwrap();
        let sunset = crate::sky_background::SunsetColorTrack::parse(&day, sunset).unwrap();
        assert_eq!(ticks.len(), oracle.len());
        for (tick, row) in ticks.into_iter().zip(oracle) {
            assert!(
                (sun.sample(tick)[0] - row["sun"].as_f64().unwrap() as f32).abs() < 0.0001,
                "sun tick {tick}"
            );
            let rgba = sunset.apply(tick, [0.0; 4]);
            for (i, value) in rgba.iter().enumerate() {
                assert!(
                    (*value - row["sunset"][i].as_f64().unwrap() as f32).abs() < 0.000002,
                    "sunset tick {tick}, channel {i}"
                );
            }
            let actual = environment.sample(&base, |_| Some(tick)).unwrap();
            assert!(
                (actual.sky_factor - row["factor"].as_f64().unwrap() as f32).abs() < 0.000001,
                "tick {tick}"
            );
            for i in 0..3 {
                assert!(
                    (actual.sky_color[i] - row["color"][i].as_f64().unwrap() as f32).abs()
                        < 0.000001,
                    "tick {tick}, channel {i}"
                );
            }
        }
        assert!(
            SkyTimeline::load(&mut resources, "minecraft:overworld")
                .unwrap()
                .is_animated()
        );
        assert!(
            !SkyTimeline::load(&mut resources, "minecraft:the_nether")
                .unwrap()
                .is_animated()
        );
    }

    #[test]
    fn active_dimension_registry_precedes_saved_stems() {
        struct Provider(Option<Vec<u8>>);
        impl ResourceProvider for Provider {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                assert_eq!(path, "data/example/dimension/deep/cavern.json");
                Ok(self.0.clone())
            }
        }
        let dimension = DimensionId::parse("example:deep/cavern").unwrap();
        let saved = BTreeMap::from([(
            dimension.clone(),
            SavedDimensionType::Named(DimensionId::parse("minecraft:the_nether").unwrap()),
        )]);
        assert_eq!(
            resolve_dimension_type(&mut Provider(None), &dimension, Some(&saved))
                .unwrap()
                .as_deref(),
            Some("minecraft:the_nether")
        );
        let override_json =
            br#"{"type":"example:custom_type","generator":{"type":"minecraft:noise"}}"#;
        assert_eq!(
            resolve_dimension_type(
                &mut Provider(Some(override_json.to_vec())),
                &dimension,
                Some(&saved)
            )
            .unwrap()
            .as_deref(),
            Some("example:custom_type")
        );
        assert!(
            resolve_dimension_type(
                &mut Provider(Some(br#"{"type":{},"generator":{}}"#.to_vec())),
                &dimension,
                Some(&saved)
            )
            .is_err()
        );
        assert!(
            resolve_dimension_type(
                &mut Provider(Some(br#"{"type":"minecraft:overworld"}"#.to_vec())),
                &dimension,
                Some(&saved)
            )
            .is_err()
        );
        assert_eq!(
            resolve_dimension_type(&mut Provider(None), &dimension, None).unwrap(),
            None
        );
    }

    #[test]
    fn reads_dimension_attributes_and_cardinal_light() {
        let lighting = PreviewLighting::from_json(
            br##"{
            "has_skylight":false,"cardinal_light":"nether",
            "attributes":{"minecraft:visual/sky_light_factor":0.0,
                          "minecraft:visual/ambient_light_color":"#302821",
                          "minecraft:visual/block_light_tint":16767116}
        }"##,
        )
        .unwrap();
        assert!(!lighting.has_skylight);
        assert!(lighting.nether_cardinal);
        assert_eq!(lighting.lightmap.sky_factor, 0.0);
        assert_eq!(
            lighting.lightmap.ambient_color,
            [48.0 / 255.0, 40.0 / 255.0, 33.0 / 255.0]
        );
        assert_eq!(
            lighting.lightmap.block_tint,
            [1.0, 216.0 / 255.0, 140.0 / 255.0]
        );
        assert!(
            PreviewLighting::from_json(br#"{"has_skylight":true,"cardinal_light":"unknown"}"#)
                .is_err()
        );
        assert!(
            PreviewLighting::from_json(br#"{"has_skylight":true,"cardinal_light":42}"#).is_err()
        );
    }

    #[test]
    #[ignore = "requires the user's Minecraft Java Edition 26.3 installation"]
    fn builtin_dimensions_and_data_pack_overrides_are_used() {
        use native_resources::{datapacks::DataPackSettings, packs::ResourceStack};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut source = ResourceStack::open(&game).unwrap();
        let overworld = PreviewLighting::load(&mut source, "minecraft:overworld").unwrap();
        assert!(overworld.has_skylight && !overworld.nether_cardinal);
        assert_eq!(overworld.lightmap.ambient_color, [10.0 / 255.0; 3]);
        let nether = PreviewLighting::load(&mut source, "minecraft:the_nether").unwrap();
        assert!(!nether.has_skylight && nether.nether_cardinal);
        assert_eq!(nether.lightmap.sky_factor, 0.0);
        assert_eq!(
            nether.lightmap.ambient_color,
            [48.0 / 255.0, 40.0 / 255.0, 33.0 / 255.0]
        );
        let end = PreviewLighting::load(&mut source, "minecraft:the_end").unwrap();
        assert!(end.has_skylight && !end.nether_cardinal);
        assert_eq!(end.lightmap.sky_factor, 0.0);
        assert_eq!(
            end.lightmap.ambient_color,
            [63.0 / 255.0, 71.0 / 255.0, 63.0 / 255.0]
        );

        let temp = tempfile::tempdir().unwrap();
        let pack = temp.path().join("datapacks/dimension_override");
        let dimension_dir = pack.join("data/minecraft/dimension_type");
        std::fs::create_dir_all(&dimension_dir).unwrap();
        std::fs::write(
            pack.join("pack.mcmeta"),
            br#"{"pack":{"description":"test","min_format":121,"max_format":121}}"#,
        )
        .unwrap();
        let mut dimension: Value = serde_json::from_slice(
            &source
                .read_builtin_data("data/minecraft/dimension_type/overworld.json")
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        dimension["attributes"]["minecraft:visual/ambient_light_color"] = "#123456".into();
        std::fs::write(
            dimension_dir.join("overworld.json"),
            serde_json::to_vec(&dimension).unwrap(),
        )
        .unwrap();
        let warnings = source
            .configure_world_data(
                temp.path(),
                &DataPackSettings {
                    enabled: vec!["vanilla".into(), "file/dimension_override".into()],
                    disabled: vec![],
                    enabled_features: vec!["minecraft:vanilla".into()],
                },
            )
            .unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let overridden = PreviewLighting::load(&mut source, "minecraft:overworld").unwrap();
        assert_eq!(
            overridden.lightmap.ambient_color,
            [18.0 / 255.0, 52.0 / 255.0, 86.0 / 255.0]
        );
    }
}
