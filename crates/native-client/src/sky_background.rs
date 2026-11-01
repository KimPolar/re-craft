//! Sky-color attributes sampled at the camera's surrounding noise biomes.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail, ensure};
use glam::DVec3;
use native_resources::packs::{ResourceId, ResourceProvider};
use native_world::loaded::World;
use serde_json::Value;

use crate::preview_lighting::{SkyTrack, rgb};

#[derive(Clone, Copy)]
struct ColorLayer {
    value: [f32; 3],
    multiply: bool,
}

impl ColorLayer {
    fn parse(value: &Value) -> Result<Self> {
        if value.is_string() || value.is_number() {
            return Ok(Self {
                value: rgb(value)?,
                multiply: false,
            });
        }
        let modifier = value
            .get("modifier")
            .and_then(Value::as_str)
            .context("missing color modifier")?;
        let id = ResourceId::parse(modifier)?;
        ensure!(id.namespace == "minecraft", "unsupported color modifier");
        let multiply = match id.path.as_str() {
            "override" => false,
            "multiply" => true,
            _ => bail!("unsupported color modifier {modifier}"),
        };
        Ok(Self {
            value: rgb(value.get("argument").context("missing color argument")?)?,
            multiply,
        })
    }

    fn apply(self, base: [f32; 3]) -> [f32; 3] {
        if self.multiply {
            std::array::from_fn(|i| base[i] * self.value[i])
        } else {
            self.value
        }
    }
}

fn find_color_attribute(attributes: &Value, attribute: &str) -> Result<Option<ColorLayer>> {
    let entries = attributes
        .as_object()
        .context("invalid environment attributes")?;
    let mut result: Option<ColorLayer> = None;
    for (name, value) in entries {
        let id = ResourceId::parse(name)?;
        if id.namespace == "minecraft" && id.path == attribute {
            ensure!(result.is_none(), "duplicate {attribute} attribute");
            result = Some(ColorLayer::parse(value)?);
        }
    }
    Ok(result)
}

#[derive(Clone, Copy)]
enum FloatModifier {
    Override,
    Add,
    Subtract,
    Multiply,
    Minimum,
    Maximum,
}

#[derive(Clone, Copy)]
struct FloatLayer {
    value: f32,
    modifier: FloatModifier,
}

struct FogTrack {
    index: usize,
    track: SkyTrack,
    modifier: FloatModifier,
}

enum SkyLevelTrack {
    Scalar(FogTrack),
    Alpha { value: SkyTrack, alpha: SkyTrack },
}

struct SkyLevel {
    base: f32,
    tracks: Vec<SkyLevelTrack>,
}

fn alpha_argument(argument: &Value) -> Result<(f32, f32)> {
    let (value, alpha) = if let Some(number) = argument.as_f64() {
        (number as f32, 1.0)
    } else {
        let value = argument
            .get("value")
            .and_then(Value::as_f64)
            .context("missing alpha-blend value")? as f32;
        let alpha = match argument.get("alpha") {
            Some(value) => value.as_f64().context("invalid alpha-blend alpha")? as f32,
            None => 1.0,
        };
        (value, alpha)
    };
    ensure!(
        value.is_finite() && (0.0..=1.0).contains(&alpha),
        "invalid alpha-blend argument"
    );
    Ok((value, alpha))
}

impl SkyLevel {
    fn new(attributes: Option<&Value>) -> Result<Self> {
        let base = match named_attribute(attributes, "gameplay/sky_light_level")? {
            None => 15.0,
            Some(layer)
                if layer
                    .get("modifier")
                    .and_then(Value::as_str)
                    .and_then(|name| ResourceId::parse(name).ok())
                    .is_some_and(|id| id.namespace == "minecraft" && id.path == "alpha_blend") =>
            {
                let (value, alpha) = alpha_argument(
                    layer
                        .get("argument")
                        .context("missing alpha-blend argument")?,
                )?;
                15.0 + alpha * (value - 15.0)
            }
            Some(layer) => FloatLayer::parse(layer)?.apply(15.0),
        };
        Ok(Self {
            base,
            tracks: Vec::new(),
        })
    }

    fn add_track(&mut self, timeline: &Value, track: &Value) -> Result<()> {
        let modifier = track
            .get("modifier")
            .and_then(Value::as_str)
            .unwrap_or("override");
        let id = ResourceId::parse(modifier)?;
        if id.namespace != "minecraft" || id.path != "alpha_blend" {
            self.tracks
                .push(SkyLevelTrack::Scalar(FogTrack::parse(0, timeline, track)?));
            return Ok(());
        }
        let mut value_track = track.clone();
        let mut alpha_track = track.clone();
        for normalized in [&mut value_track, &mut alpha_track] {
            normalized
                .as_object_mut()
                .context("invalid sky-light track")?
                .insert("modifier".into(), Value::String("override".into()));
        }
        let source = track
            .get("keyframes")
            .and_then(Value::as_array)
            .context("missing sky-light keyframes")?;
        for ((value_frame, alpha_frame), source_frame) in value_track["keyframes"]
            .as_array_mut()
            .context("invalid sky-light keyframes")?
            .iter_mut()
            .zip(
                alpha_track["keyframes"]
                    .as_array_mut()
                    .context("invalid sky-light keyframes")?
                    .iter_mut(),
            )
            .zip(source)
        {
            let (value, alpha) = alpha_argument(
                source_frame
                    .get("value")
                    .context("missing sky-light value")?,
            )?;
            value_frame["value"] = Value::from(value);
            alpha_frame["value"] = Value::from(alpha);
        }
        self.tracks.push(SkyLevelTrack::Alpha {
            value: SkyTrack::parse(timeline, &value_track, true)?,
            alpha: SkyTrack::parse(timeline, &alpha_track, true)?,
        });
        Ok(())
    }

    fn sample(
        &self,
        mut clock: impl FnMut(&ResourceId) -> Option<i64>,
        weather: native_session::weather::WeatherVisual,
    ) -> Option<f32> {
        let mut value = self.base;
        for track in &self.tracks {
            value = match track {
                SkyLevelTrack::Scalar(track) => FloatLayer {
                    value: track.track.sample(clock(&track.track.clock)?)[0],
                    modifier: track.modifier,
                }
                .apply(value),
                SkyLevelTrack::Alpha {
                    value: target,
                    alpha,
                } => {
                    let tick = clock(&target.clock)?;
                    let target = target.sample(tick)[0];
                    let alpha = alpha.sample(tick)[0];
                    value + alpha * (target - value)
                }
            };
        }
        let rain = weather.rain_at(1.0);
        let thunder = weather.thunder_at(1.0);
        if rain - thunder > 0.0 {
            let rainy = value + 0.3125 * (4.0 - value);
            value += (rain - thunder) * (rainy - value);
        }
        if thunder > 0.0 {
            let stormy = value + 0.527_343_75 * (4.0 - value);
            value += thunder * (stormy - value);
        }
        value.is_finite().then_some(value.clamp(0.0, 15.0))
    }
}

#[derive(Clone, Copy)]
enum SunsetColorLayer {
    Override([f32; 4]),
    Multiply([f32; 4]),
    Add([f32; 3]),
    Subtract([f32; 3]),
    AlphaBlend([f32; 4]),
    BlendToGray { brightness: f32, factor: f32 },
}

impl SunsetColorLayer {
    fn parse(value: &Value) -> Result<Self> {
        if !value.is_object() {
            return Ok(Self::Override(argb(value)?));
        }
        let id = ResourceId::parse(
            value
                .get("modifier")
                .and_then(Value::as_str)
                .context("missing color modifier")?,
        )?;
        ensure!(id.namespace == "minecraft", "unsupported color modifier");
        let arg = value.get("argument").context("missing color argument")?;
        match id.path.as_str() {
            "override" => Ok(Self::Override(argb(arg)?)),
            "multiply" => Ok(Self::Multiply(argb(arg)?)),
            "add" => Ok(Self::Add(rgb(arg)?)),
            "subtract" => Ok(Self::Subtract(rgb(arg)?)),
            "alpha_blend" => Ok(Self::AlphaBlend(argb(arg)?)),
            "blend_to_gray" => {
                let parse = |name| -> Result<f32> {
                    let n = arg
                        .get(name)
                        .and_then(Value::as_f64)
                        .context("missing gray parameter")? as f32;
                    ensure!((0.0..=1.0).contains(&n), "invalid gray parameter");
                    Ok(n)
                };
                Ok(Self::BlendToGray {
                    brightness: parse("brightness")?,
                    factor: parse("factor")?,
                })
            }
            _ => bail!("unsupported color modifier"),
        }
    }

    fn apply(self, base: [f32; 4]) -> [f32; 4] {
        match self {
            Self::Override(value) => value,
            Self::Multiply(value) => std::array::from_fn(|i| base[i] * value[i]),
            Self::Add(value) => [
                (base[0] + value[0]).min(1.0),
                (base[1] + value[1]).min(1.0),
                (base[2] + value[2]).min(1.0),
                base[3],
            ],
            Self::Subtract(value) => [
                (base[0] - value[0]).max(0.0),
                (base[1] - value[1]).max(0.0),
                (base[2] - value[2]).max(0.0),
                base[3],
            ],
            Self::AlphaBlend(value) => {
                if value[3] == 0.0 {
                    return base;
                }
                if value[3] == 1.0 {
                    return value;
                }
                let alpha = value[3] + base[3] * (1.0 - value[3]);
                std::array::from_fn(|i| {
                    if i == 3 {
                        alpha
                    } else {
                        (value[i] * value[3] + base[i] * (alpha - value[3])) / alpha
                    }
                })
            }
            Self::BlendToGray { brightness, factor } => {
                let gray = ((base[0] * 0.3 + base[1] * 0.59 + base[2] * 0.11) * brightness)
                    .clamp(0.0, 1.0);
                [
                    base[0] + factor * (gray - base[0]),
                    base[1] + factor * (gray - base[1]),
                    base[2] + factor * (gray - base[2]),
                    base[3],
                ]
            }
        }
    }
}

pub(crate) enum SunsetColorTrack {
    Color {
        rgb: SkyTrack,
        alpha: SkyTrack,
        operation: SunsetColorOperation,
    },
    Gray {
        brightness: SkyTrack,
        factor: SkyTrack,
    },
}

#[derive(Clone, Copy)]
pub(crate) enum SunsetColorOperation {
    Override,
    Multiply,
    Add,
    Subtract,
    AlphaBlend,
}

fn argb(value: &Value) -> Result<[f32; 4]> {
    let packed = if let Some(s) = value.as_str() {
        ensure!(
            s.len() == 9 && s.starts_with('#'),
            "color must be #AARRGGBB"
        );
        u32::from_str_radix(&s[1..], 16).context("invalid ARGB color")?
    } else if let Some(n) = value.as_i64() {
        ensure!(
            (i64::from(i32::MIN)..=i64::from(u32::MAX)).contains(&n),
            "ARGB color out of range"
        );
        n as u32
    } else {
        bail!("invalid ARGB color")
    };
    Ok([
        ((packed >> 16) & 255) as f32 / 255.0,
        ((packed >> 8) & 255) as f32 / 255.0,
        (packed & 255) as f32 / 255.0,
        ((packed >> 24) & 255) as f32 / 255.0,
    ])
}

impl SunsetColorTrack {
    pub(crate) fn parse(timeline: &Value, track: &Value) -> Result<Self> {
        let operation = match track.get("modifier") {
            None => "override",
            Some(value) => value.as_str().context("invalid color track modifier")?,
        };
        let id = ResourceId::parse(operation)?;
        ensure!(
            id.namespace == "minecraft",
            "unsupported color track modifier"
        );
        let mut first = track.clone();
        let mut second = track.clone();
        first
            .as_object_mut()
            .context("invalid color track")?
            .insert("modifier".into(), Value::String("override".into()));
        second
            .as_object_mut()
            .context("invalid color track")?
            .insert("modifier".into(), Value::String("override".into()));
        let original = track
            .get("keyframes")
            .and_then(Value::as_array)
            .context("missing color keyframes")?;
        for ((first, second), frame) in first["keyframes"]
            .as_array_mut()
            .context("invalid color frames")?
            .iter_mut()
            .zip(
                second["keyframes"]
                    .as_array_mut()
                    .context("invalid alpha frames")?
                    .iter_mut(),
            )
            .zip(original)
        {
            let value = frame.get("value").context("missing color")?;
            let layer = SunsetColorLayer::parse(
                &serde_json::json!({"modifier": operation, "argument": value}),
            )?;
            if let SunsetColorLayer::BlendToGray { brightness, factor } = layer {
                first["value"] = Value::from(brightness);
                second["value"] = Value::from(factor);
            } else {
                let color = match layer {
                    SunsetColorLayer::Override(v)
                    | SunsetColorLayer::Multiply(v)
                    | SunsetColorLayer::AlphaBlend(v) => v,
                    SunsetColorLayer::Add(v) | SunsetColorLayer::Subtract(v) => {
                        [v[0], v[1], v[2], 0.0]
                    }
                    SunsetColorLayer::BlendToGray { .. } => unreachable!(),
                };
                let packed = ((color[0] * 255.0).round() as u32) << 16
                    | ((color[1] * 255.0).round() as u32) << 8
                    | (color[2] * 255.0).round() as u32;
                first["value"] = Value::from(packed);
                second["value"] = Value::from(color[3]);
            }
        }
        if id.path == "blend_to_gray" {
            return Ok(Self::Gray {
                brightness: SkyTrack::parse(timeline, &first, true)?,
                factor: SkyTrack::parse(timeline, &second, true)?,
            });
        }
        let operation = match id.path.as_str() {
            "override" => SunsetColorOperation::Override,
            "multiply" => SunsetColorOperation::Multiply,
            "add" => SunsetColorOperation::Add,
            "subtract" => SunsetColorOperation::Subtract,
            "alpha_blend" => SunsetColorOperation::AlphaBlend,
            _ => bail!("unsupported color track modifier"),
        };
        Ok(Self::Color {
            rgb: SkyTrack::parse(timeline, &first, false)?,
            alpha: SkyTrack::parse(timeline, &second, true)?,
            operation,
        })
    }
    fn clock(&self) -> &ResourceId {
        match self {
            Self::Color { rgb, .. } => &rgb.clock,
            Self::Gray { brightness, .. } => &brightness.clock,
        }
    }
    pub(crate) fn apply(&self, ticks: i64, base: [f32; 4]) -> [f32; 4] {
        let layer = match self {
            Self::Gray { brightness, factor } => SunsetColorLayer::BlendToGray {
                brightness: brightness.sample(ticks)[0],
                factor: factor.sample(ticks)[0],
            },
            Self::Color {
                rgb,
                alpha,
                operation,
            } => {
                let color = rgb.sample(ticks);
                let rgba = [color[0], color[1], color[2], alpha.sample(ticks)[0]];
                match operation {
                    SunsetColorOperation::Override => SunsetColorLayer::Override(rgba),
                    SunsetColorOperation::Multiply => SunsetColorLayer::Multiply(rgba),
                    SunsetColorOperation::Add => SunsetColorLayer::Add(color),
                    SunsetColorOperation::Subtract => SunsetColorLayer::Subtract(color),
                    SunsetColorOperation::AlphaBlend => SunsetColorLayer::AlphaBlend(rgba),
                }
            }
        };
        layer.apply(base)
    }
}

struct SunsetTimeline {
    base_angle: f32,
    base_color: [f32; 4],
    biome_angles: BTreeMap<String, Option<Option<FloatLayer>>>,
    biome_colors: BTreeMap<String, Option<Option<SunsetColorLayer>>>,
    angle: Vec<FogTrack>,
    color: Vec<SunsetColorTrack>,
}

// Each celestial angle is an independent environment attribute. In particular,
// an unsupported sunrise color must not suppress the sun or the moon.
struct CelestialValue {
    base: f32,
    biomes: BTreeMap<String, Option<Option<FloatLayer>>>,
    tracks: Vec<FogTrack>,
}

impl CelestialValue {
    fn new(attributes: Option<&Value>, name: &str) -> Result<Self> {
        let base = named_attribute(attributes, name)?
            .map(FloatLayer::parse)
            .transpose()?
            .map_or(0.0, |layer| layer.apply(0.0));
        Ok(Self {
            base,
            biomes: BTreeMap::new(),
            tracks: Vec::new(),
        })
    }

    fn sample(
        &self,
        grouped: &[(&str, f64)],
        mut clock: impl FnMut(&ResourceId) -> Option<i64>,
    ) -> Option<f32> {
        let mut total = 0.0;
        let mut result = None;
        for &(id, weight) in grouped {
            let value = self
                .biomes
                .get(id)?
                .as_ref()?
                .map_or(self.base, |layer| layer.apply(self.base));
            total += weight;
            result = Some(match result {
                None => value,
                Some(previous) => previous + (weight / total) as f32 * (value - previous),
            });
        }
        let mut result = result?;
        for track in &self.tracks {
            result = FloatLayer {
                value: track.track.sample(clock(&track.track.clock)?)[0],
                modifier: track.modifier,
            }
            .apply(result);
        }
        result.is_finite().then_some(result)
    }
}

fn named_attribute<'a>(attributes: Option<&'a Value>, name: &str) -> Result<Option<&'a Value>> {
    let Some(attributes) = attributes else {
        return Ok(None);
    };
    let mut found = None;
    for (key, value) in attributes.as_object().context("invalid attributes")? {
        let id = ResourceId::parse(key)?;
        if id.namespace == "minecraft" && id.path == name {
            ensure!(found.is_none(), "duplicate attribute {name}");
            found = Some(value);
        }
    }
    Ok(found)
}

impl SunsetTimeline {
    fn new(attributes: Option<&Value>) -> Result<Self> {
        let base_angle = named_attribute(attributes, "visual/sun_angle")?
            .map(FloatLayer::parse)
            .transpose()?
            .map_or(0.0, |v| v.apply(0.0));
        let base_color = named_attribute(attributes, "visual/sunrise_sunset_color")?
            .map(SunsetColorLayer::parse)
            .transpose()?
            .map_or([0.0; 4], |layer| layer.apply([0.0; 4]));
        Ok(Self {
            base_angle,
            base_color,
            biome_angles: BTreeMap::new(),
            biome_colors: BTreeMap::new(),
            angle: Vec::new(),
            color: Vec::new(),
        })
    }

    fn sample(
        &self,
        grouped: &[(&str, f64)],
        mut clock: impl FnMut(&ResourceId) -> Option<i64>,
    ) -> Option<(f32, [f32; 4])> {
        let mut total = 0.0;
        let mut result: Option<(f32, [f32; 4])> = None;
        for &(id, weight) in grouped {
            let angle = self
                .biome_angles
                .get(id)?
                .as_ref()?
                .map_or(self.base_angle, |layer| layer.apply(self.base_angle));
            let color = self
                .biome_colors
                .get(id)?
                .as_ref()?
                .map_or(self.base_color, |layer| layer.apply(self.base_color));
            total += weight;
            result = Some(match result {
                None => (angle, color),
                Some((prior_angle, prior_color)) => {
                    let fraction = (weight / total) as f32;
                    (
                        prior_angle + fraction * (angle - prior_angle),
                        std::array::from_fn(|i| {
                            prior_color[i] + fraction * (color[i] - prior_color[i])
                        }),
                    )
                }
            });
        }
        let (mut angle, mut color) = result?;
        for track in &self.angle {
            angle = FloatLayer {
                value: track.track.sample(clock(&track.track.clock)?)[0],
                modifier: track.modifier,
            }
            .apply(angle);
        }
        for track in &self.color {
            color = track.apply(clock(track.clock())?, color);
        }
        Some((angle, color))
    }
}

impl FogTrack {
    fn parse(index: usize, timeline: &Value, value: &Value) -> Result<Self> {
        let modifier = match value.get("modifier") {
            None => FloatModifier::Override,
            Some(name) => {
                FloatLayer::parse(&serde_json::json!({"modifier": name, "argument": 1.0}))?.modifier
            }
        };
        let mut normalized = value.clone();
        normalized
            .as_object_mut()
            .context("invalid fog timeline")?
            .insert("modifier".into(), Value::String("override".into()));
        Ok(Self {
            index,
            track: SkyTrack::parse(timeline, &normalized, true)?,
            modifier,
        })
    }
}

impl FloatLayer {
    fn parse(value: &Value) -> Result<Self> {
        let (modifier, argument) = if value.is_number() {
            (FloatModifier::Override, value)
        } else {
            let name = value
                .get("modifier")
                .and_then(Value::as_str)
                .context("missing float modifier")?;
            let id = ResourceId::parse(name)?;
            ensure!(id.namespace == "minecraft", "unsupported float modifier");
            let modifier = match id.path.as_str() {
                "override" => FloatModifier::Override,
                "add" => FloatModifier::Add,
                "subtract" => FloatModifier::Subtract,
                "multiply" => FloatModifier::Multiply,
                "minimum" => FloatModifier::Minimum,
                "maximum" => FloatModifier::Maximum,
                _ => bail!("unsupported float modifier {name}"),
            };
            (
                modifier,
                value.get("argument").context("missing float argument")?,
            )
        };
        let value = argument.as_f64().context("invalid float argument")? as f32;
        ensure!(value.is_finite(), "non-finite float argument");
        Ok(Self { value, modifier })
    }
    fn apply(self, base: f32) -> f32 {
        match self.modifier {
            FloatModifier::Override => self.value,
            FloatModifier::Add => base + self.value,
            FloatModifier::Subtract => base - self.value,
            FloatModifier::Multiply => base * self.value,
            FloatModifier::Minimum => base.min(self.value),
            FloatModifier::Maximum => base.max(self.value),
        }
    }
}

fn fog_layers(attributes: Option<&Value>) -> Result<[Option<FloatLayer>; 3]> {
    let mut result = [None; 3];
    let Some(attributes) = attributes else {
        return Ok(result);
    };
    for (name, value) in attributes
        .as_object()
        .context("invalid environment attributes")?
    {
        let id = ResourceId::parse(name)?;
        if id.namespace != "minecraft" {
            continue;
        }
        let index = match id.path.as_str() {
            "visual/fog_start_distance" => 0,
            "visual/fog_end_distance" => 1,
            "visual/sky_fog_end_distance" => 2,
            _ => continue,
        };
        ensure!(result[index].is_none(), "duplicate fog distance");
        result[index] = Some(FloatLayer::parse(value)?);
    }
    Ok(result)
}

fn water_fog_layers(attributes: Option<&Value>) -> Result<[Option<FloatLayer>; 3]> {
    let mut result = [None; 3];
    let Some(attributes) = attributes else {
        return Ok(result);
    };
    for (name, value) in attributes
        .as_object()
        .context("invalid environment attributes")?
    {
        let id = ResourceId::parse(name)?;
        if id.namespace != "minecraft" {
            continue;
        }
        let index = match id.path.as_str() {
            "visual/water_fog_start_distance" => 0,
            "visual/water_fog_end_distance" => 1,
            _ => continue,
        };
        ensure!(result[index].is_none(), "duplicate water fog distance");
        result[index] = Some(FloatLayer::parse(value)?);
    }
    Ok(result)
}

#[derive(Default)]
struct WaterFog {
    base: Option<[f32; 3]>,
    biomes: BTreeMap<String, Option<[Option<FloatLayer>; 3]>>,
    tracks: Option<Vec<FogTrack>>,
    color_base: Option<[f32; 3]>,
    color_biomes: BTreeMap<String, Option<Option<ColorLayer>>>,
    color_tracks: Option<Vec<SkyTrack>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterFogSample {
    pub color: [f32; 3],
    pub range: [f32; 2],
}

pub struct SkyBackground {
    base: [f32; 3],
    ambient_light: f32,
    sky_level: Option<SkyLevel>,
    biomes: BTreeMap<String, Option<ColorLayer>>,
    tracks: Vec<SkyTrack>,
    background_is_fog: bool,
    end_sky: bool,
    fog_color_base: Option<[f32; 3]>,
    fog_color_biomes: BTreeMap<String, Option<Option<ColorLayer>>>,
    fog_color_tracks: Option<Vec<SkyTrack>>,
    fog_base: Option<[f32; 3]>,
    fog_biomes: BTreeMap<String, Option<[Option<FloatLayer>; 3]>>,
    fog_tracks: Option<Vec<FogTrack>>,
    fog_sky_base: Option<[f32; 3]>,
    fog_sky_biomes: BTreeMap<String, Option<Option<ColorLayer>>>,
    fog_sky_tracks: Option<Vec<SkyTrack>>,
    sunset: Option<SunsetTimeline>,
    celestial_values: [Option<CelestialValue>; 4],
    moon_phase: Option<crate::moon_phase::Environment>,
    water: WaterFog,
    flat_world: Option<bool>,
    min_y: i32,
    max_y: i32,
}

#[derive(Debug, PartialEq)]
pub struct BackgroundSample {
    pub color: [f32; 3],
    pub background_is_fog: bool,
    pub fog_color: Option<[f32; 3]>,
    pub fog_range: Option<[f32; 2]>,
    pub sky_mix: Option<([f32; 3], f32)>,
    pub sunset: Option<(f32, [f32; 4])>,
    pub celestial_values: [Option<f32>; 4],
    pub moon_phase: Option<crate::moon_phase::Phase>,
    pub water: Option<WaterFogSample>,
}

impl BackgroundSample {
    /// Apply Java's final weather layers before partial-tick interpolation.
    pub fn apply_weather(&mut self, weather: native_session::weather::WeatherVisual) {
        let rain = weather.rain_at(1.0);
        let thunder = weather.thunder_at(1.0);
        self.color = if self.background_is_fog {
            crate::sky_weather::fog(self.color, rain, thunder)
        } else {
            crate::sky_weather::sky(self.color, rain, thunder)
        };
        self.fog_color = self
            .fog_color
            .map(|color| crate::sky_weather::fog(color, rain, thunder));
        self.sky_mix = self
            .sky_mix
            .map(|(color, distance)| (crate::sky_weather::sky(color, rain, thunder), distance));
        self.sunset = self
            .sunset
            .map(|(angle, color)| (angle, crate::sky_weather::sunrise(color, rain, thunder)));
        self.celestial_values[3] = self.celestial_values[3]
            .map(|brightness| crate::sky_weather::stars(brightness, rain, thunder));
    }
}

pub(crate) fn void_darkness_at(min_y: i32, flat: bool, camera_y: f64) -> f32 {
    let range = if flat { 1.0_f32 } else { 32.0_f32 };
    ((range + min_y as f32 - camera_y as f32) / range).clamp(0.0, 1.0)
}

pub(crate) fn dark_disc_visible_at(min_y: i32, flat: bool, eye_y: f64, underwater: bool) -> bool {
    let horizon = if flat { f64::from(min_y) } else { 63.0 };
    !underwater && eye_y < horizon
}

impl SkyBackground {
    pub fn load(
        source: &mut impl ResourceProvider,
        dimension_type: &str,
        min_y: i32,
        max_y: i32,
    ) -> Result<Option<Self>> {
        ensure!(min_y < max_y, "invalid dimension bounds");
        let id = ResourceId::parse(dimension_type)?;
        let path = format!("data/{}/dimension_type/{}.json", id.namespace, id.path);
        let bytes = source
            .read_data(&path)?
            .with_context(|| format!("missing {path}"))?;
        let dimension: Value =
            serde_json::from_slice(&bytes).with_context(|| format!("invalid {path}"))?;
        let ambient_light = match dimension.get("ambient_light") {
            None => 0.0,
            Some(value) => value.as_f64().context("invalid dimension ambient_light")? as f32,
        };
        ensure!(
            (0.0..=1.0).contains(&ambient_light),
            "dimension ambient_light is out of range"
        );
        let skybox = match dimension.get("skybox") {
            None => "overworld",
            Some(value) => value.as_str().context("invalid skybox")?,
        };
        let attribute = match skybox {
            "overworld" | "end" => "visual/sky_color",
            // Without a skybox, atmospheric fog is the background.
            "none" => "visual/fog_color",
            _ => bail!("unsupported skybox"),
        };
        let base = match dimension.get("attributes") {
            None => [0.0; 3],
            Some(attributes) => find_color_attribute(attributes, attribute)?
                .map_or([0.0; 3], |layer| layer.apply([0.0; 3])),
        };
        let fog_base = {
            fog_layers(dimension.get("attributes")).ok().map(|layers| {
                let defaults = [0.0, 1024.0, 512.0];
                std::array::from_fn(|i| {
                    layers[i].map_or(defaults[i], |layer| layer.apply(defaults[i]))
                })
            })
        };
        let mut water = WaterFog {
            base: water_fog_layers(dimension.get("attributes"))
                .ok()
                .map(|layers| {
                    let defaults = [-8.0, 96.0, 0.0];
                    std::array::from_fn(|i| {
                        layers[i].map_or(defaults[i], |layer| layer.apply(defaults[i]))
                    })
                }),
            color_base: match dimension.get("attributes") {
                None => Some([5.0 / 255.0, 5.0 / 255.0, 51.0 / 255.0]),
                Some(attributes) => find_color_attribute(attributes, "visual/water_fog_color")
                    .ok()
                    .map(|layer| {
                        layer.map_or([5.0 / 255.0, 5.0 / 255.0, 51.0 / 255.0], |layer| {
                            layer.apply([5.0 / 255.0, 5.0 / 255.0, 51.0 / 255.0])
                        })
                    }),
            },
            tracks: Some(Vec::new()),
            color_tracks: Some(Vec::new()),
            ..Default::default()
        };
        let fog_sky_base = if skybox == "none" {
            match dimension.get("attributes") {
                None => Some([0.0; 3]),
                Some(attributes) => find_color_attribute(attributes, "visual/sky_color")
                    .ok()
                    .map(|layer| layer.map_or([0.0; 3], |layer| layer.apply([0.0; 3]))),
            }
        } else {
            None
        };
        let mut fog_biomes = BTreeMap::new();
        let mut sunset = SunsetTimeline::new(dimension.get("attributes")).ok();
        let mut sky_level = SkyLevel::new(dimension.get("attributes")).ok();
        let celestial_names = [
            "visual/sun_angle",
            "visual/moon_angle",
            "visual/star_angle",
            "visual/star_brightness",
        ];
        let mut celestial_values = if skybox == "overworld" {
            celestial_names.map(|name| CelestialValue::new(dimension.get("attributes"), name).ok())
        } else {
            [None, None, None, None]
        };
        let mut moon_phase = if skybox == "overworld" {
            named_attribute(dimension.get("attributes"), "visual/moon_phase")
                .and_then(|value| value.map(crate::moon_phase::layer).transpose())
                .ok()
                .map(|value| crate::moon_phase::Environment {
                    base: value.unwrap_or(crate::moon_phase::Phase::FullMoon),
                    biomes: BTreeMap::new(),
                    has_biome_layer: false,
                    tracks: Vec::new(),
                })
        } else {
            None
        };
        let mut fog_sky_biomes = BTreeMap::new();
        let mut fog_color_biomes = BTreeMap::new();
        let fog_color_base = match dimension.get("attributes") {
            None => Some([0.0; 3]),
            Some(attributes) => find_color_attribute(attributes, "visual/fog_color")
                .ok()
                .map(|layer| layer.map_or([0.0; 3], |layer| layer.apply([0.0; 3]))),
        };
        let mut biomes = BTreeMap::new();
        for id in source.list_data("worldgen/biome")? {
            let path = format!("data/{}/worldgen/biome/{}.json", id.namespace, id.path);
            let bytes = source
                .read_data(&path)?
                .with_context(|| format!("missing {path}"))?;
            let biome: Value =
                serde_json::from_slice(&bytes).with_context(|| format!("invalid {path}"))?;
            let layer = biome
                .get("attributes")
                .map(|attributes| find_color_attribute(attributes, attribute))
                .transpose()?
                .flatten();
            let name = format!("{}:{}", id.namespace, id.path);
            water
                .biomes
                .insert(name.clone(), water_fog_layers(biome.get("attributes")).ok());
            water.color_biomes.insert(
                name.clone(),
                match biome.get("attributes") {
                    None => Some(None),
                    Some(attributes) => {
                        find_color_attribute(attributes, "visual/water_fog_color").ok()
                    }
                },
            );
            if let Some(sunset) = &mut sunset {
                let attributes = biome.get("attributes");
                sunset.biome_angles.insert(
                    name.clone(),
                    named_attribute(attributes, "visual/sun_angle")
                        .and_then(|layer| layer.map(FloatLayer::parse).transpose())
                        .ok(),
                );
                sunset.biome_colors.insert(
                    name.clone(),
                    named_attribute(attributes, "visual/sunrise_sunset_color")
                        .and_then(|layer| layer.map(SunsetColorLayer::parse).transpose())
                        .ok(),
                );
            }
            for (angle, attribute_name) in celestial_values.iter_mut().zip(celestial_names) {
                if let Some(angle) = angle {
                    angle.biomes.insert(
                        name.clone(),
                        named_attribute(biome.get("attributes"), attribute_name)
                            .and_then(|layer| layer.map(FloatLayer::parse).transpose())
                            .ok(),
                    );
                }
            }
            if let Some(phase) = &mut moon_phase {
                let layer = named_attribute(biome.get("attributes"), "visual/moon_phase");
                if !matches!(layer, Ok(None)) {
                    phase.has_biome_layer = true;
                }
                phase.biomes.insert(
                    name.clone(),
                    layer
                        .and_then(|value| value.map(crate::moon_phase::layer).transpose())
                        .ok(),
                );
            }
            fog_biomes.insert(name.clone(), fog_layers(biome.get("attributes")).ok());
            fog_color_biomes.insert(
                name.clone(),
                match biome.get("attributes") {
                    None => Some(None),
                    Some(attributes) => find_color_attribute(attributes, "visual/fog_color").ok(),
                },
            );
            fog_sky_biomes.insert(
                name.clone(),
                match biome.get("attributes") {
                    None => Some(None),
                    Some(attributes) => find_color_attribute(attributes, "visual/sky_color").ok(),
                },
            );
            ensure!(biomes.insert(name, layer).is_none(), "duplicate biome");
        }
        ensure!(!biomes.is_empty(), "biome registry is empty");
        let mut tracks = Vec::new();
        let mut fog_tracks = Some(Vec::new());
        let mut fog_sky_tracks = Some(Vec::new());
        let mut fog_color_tracks = Some(Vec::new());
        for timeline in
            native_resources::fluid_environment::load_dimension_timelines(source, dimension_type)?
        {
            let entries = timeline
                .get("tracks")
                .and_then(Value::as_object)
                .context("invalid timeline tracks")?;
            let mut found = false;
            for (name, value) in entries {
                let id = ResourceId::parse(name)?;
                if id.namespace == "minecraft"
                    && id.path == "gameplay/sky_light_level"
                    && let Some(level) = &mut sky_level
                    && level.add_track(&timeline, value).is_err()
                {
                    sky_level = None;
                }
                for (index, attribute_name) in celestial_names.iter().enumerate() {
                    if id.namespace == "minecraft"
                        && id.path == *attribute_name
                        && celestial_values[index].is_some()
                    {
                        match FogTrack::parse(0, &timeline, value) {
                            Ok(track) => {
                                celestial_values[index].as_mut().unwrap().tracks.push(track)
                            }
                            Err(_) => celestial_values[index] = None,
                        }
                    }
                }
                if id.namespace == "minecraft"
                    && id.path == "visual/moon_phase"
                    && moon_phase.is_some()
                {
                    match crate::moon_phase::Track::parse(&timeline, value) {
                        Ok(track) => moon_phase.as_mut().unwrap().tracks.push(track),
                        Err(_) => moon_phase = None,
                    }
                }
                if id.namespace == "minecraft" {
                    if let Some(index) = match id.path.as_str() {
                        "visual/water_fog_start_distance" => Some(0),
                        "visual/water_fog_end_distance" => Some(1),
                        _ => None,
                    } {
                        match FogTrack::parse(index, &timeline, value) {
                            Ok(track) => {
                                if let Some(tracks) = &mut water.tracks {
                                    tracks.push(track);
                                }
                            }
                            Err(_) => water.tracks = None,
                        }
                    }
                    if id.path == "visual/water_fog_color" {
                        match SkyTrack::parse(&timeline, value, false) {
                            Ok(track) => {
                                if let Some(tracks) = &mut water.color_tracks {
                                    tracks.push(track);
                                }
                            }
                            Err(_) => water.color_tracks = None,
                        }
                    }
                }
                if id.namespace == "minecraft" && id.path == "visual/sun_angle" && sunset.is_some()
                {
                    match FogTrack::parse(0, &timeline, value) {
                        Ok(track) => sunset.as_mut().unwrap().angle.push(track),
                        Err(_) => sunset = None,
                    }
                }
                if id.namespace == "minecraft"
                    && id.path == "visual/sunrise_sunset_color"
                    && sunset.is_some()
                {
                    match SunsetColorTrack::parse(&timeline, value) {
                        Ok(track) => sunset.as_mut().unwrap().color.push(track),
                        Err(_) => sunset = None,
                    }
                }
                if id.namespace == "minecraft"
                    && let Some(index) = match id.path.as_str() {
                        "visual/fog_start_distance" => Some(0),
                        "visual/fog_end_distance" => Some(1),
                        "visual/sky_fog_end_distance" => Some(2),
                        _ => None,
                    }
                {
                    match FogTrack::parse(index, &timeline, value) {
                        Ok(track) => {
                            if let Some(tracks) = &mut fog_tracks {
                                tracks.push(track);
                            }
                        }
                        Err(_) => fog_tracks = None,
                    }
                }
                if skybox == "none" && id.namespace == "minecraft" && id.path == "visual/sky_color"
                {
                    match SkyTrack::parse(&timeline, value, false) {
                        Ok(track) => {
                            if let Some(tracks) = &mut fog_sky_tracks {
                                tracks.push(track);
                            }
                        }
                        Err(_) => fog_sky_tracks = None,
                    }
                }
                if id.namespace == "minecraft" && id.path == "visual/fog_color" {
                    match SkyTrack::parse(&timeline, value, false) {
                        Ok(track) => {
                            if let Some(tracks) = &mut fog_color_tracks {
                                tracks.push(track);
                            }
                        }
                        Err(_) => fog_color_tracks = None,
                    }
                }
                if id.namespace == "minecraft" && id.path == attribute {
                    ensure!(!found, "duplicate {attribute} timeline");
                    found = true;
                    tracks.push(SkyTrack::parse(&timeline, value, false)?);
                }
            }
        }
        Ok(Some(Self {
            base,
            ambient_light,
            sky_level,
            biomes,
            tracks,
            background_is_fog: skybox == "none",
            end_sky: skybox == "end",
            fog_color_base,
            fog_color_biomes,
            fog_color_tracks,
            fog_base,
            fog_biomes,
            fog_tracks,
            fog_sky_base,
            fog_sky_biomes,
            fog_sky_tracks,
            sunset,
            celestial_values,
            moon_phase,
            water,
            flat_world: None,
            min_y,
            max_y,
        }))
    }

    pub fn set_flat_world(&mut self, flat: Option<bool>) {
        self.flat_world = flat;
    }

    pub fn ambient_light(&self) -> f32 {
        self.ambient_light
    }

    pub fn sky_darken(
        &self,
        clock: impl FnMut(&ResourceId) -> Option<i64>,
        weather: Option<native_session::weather::WeatherVisual>,
    ) -> Option<i32> {
        let level = self.sky_level.as_ref()?.sample(clock, weather?)?;
        Some((15.0 - level) as i32)
    }

    pub fn has_end_sky(&self) -> bool {
        self.end_sky
    }

    pub fn has_overworld_sky(&self) -> bool {
        !self.end_sky && !self.background_is_fog
    }

    pub fn void_darkness(&self, camera_y: f64) -> f32 {
        let Some(flat) = self.flat_world else {
            return 0.0;
        };
        void_darkness_at(self.min_y, flat, camera_y)
    }

    pub fn dark_disc_visible(&self, eye_y: f64, underwater: bool) -> bool {
        self.flat_world
            .is_some_and(|flat| dark_disc_visible_at(self.min_y, flat, eye_y, underwater))
    }

    pub fn sample(
        &self,
        world: &World,
        position: DVec3,
        mut clock_ticks: impl FnMut(&ResourceId) -> Option<i64>,
    ) -> Option<BackgroundSample> {
        let min_y = self.min_y.div_euclid(4) * 4;
        let max_y = (self.max_y - 1).div_euclid(4) * 4;
        let grouped = group_biomes(position, |x, y, z| {
            world.biome(
                x.checked_mul(4)?,
                y.checked_mul(4)?.clamp(min_y, max_y),
                z.checked_mul(4)?,
            )
        })?;
        let mut value = interpolate_color(&grouped, &self.biomes, self.base)?;
        for track in &self.tracks {
            let arg = track.sample(clock_ticks(&track.clock)?);
            value = if track.multiply {
                std::array::from_fn(|i| value[i] * arg[i])
            } else {
                arg
            };
        }
        let fog_values = (|| {
            let mut range = interpolate_fog(&grouped, &self.fog_biomes, self.fog_base?)?;
            for track in self.fog_tracks.as_ref()? {
                let arg = track.track.sample(clock_ticks(&track.track.clock)?)[0];
                range[track.index] = FloatLayer {
                    value: arg,
                    modifier: track.modifier,
                }
                .apply(range[track.index]);
            }
            range[1] = range[1].max(0.0);
            range[2] = range[2].max(0.0);
            range.iter().all(|n| n.is_finite()).then_some(range)
        })();
        let fog_range = fog_values.map(|values| [values[0], values[1]]);
        let fog_color = (|| {
            let mut color =
                interpolate_optional_color(&grouped, &self.fog_color_biomes, self.fog_color_base?)?;
            for track in self.fog_color_tracks.as_ref()? {
                let arg = track.sample(clock_ticks(&track.clock)?);
                color = if track.multiply {
                    std::array::from_fn(|i| color[i] * arg[i])
                } else {
                    arg
                };
            }
            Some(color)
        })();
        let sky_mix = (|| {
            let sky = if self.background_is_fog {
                let mut sky =
                    interpolate_optional_color(&grouped, &self.fog_sky_biomes, self.fog_sky_base?)?;
                for track in self.fog_sky_tracks.as_ref()? {
                    let arg = track.sample(clock_ticks(&track.clock)?);
                    sky = if track.multiply {
                        std::array::from_fn(|i| sky[i] * arg[i])
                    } else {
                        arg
                    };
                }
                sky
            } else {
                value
            };
            Some((sky, fog_values?[2]))
        })();
        let water = (|| {
            let mut range = interpolate_fog(&grouped, &self.water.biomes, self.water.base?)?;
            for track in self.water.tracks.as_ref()? {
                range[track.index] = FloatLayer {
                    value: track.track.sample(clock_ticks(&track.track.clock)?)[0],
                    modifier: track.modifier,
                }
                .apply(range[track.index]);
            }
            range[1] = range[1].max(0.0);
            if !range.iter().all(|n| n.is_finite()) {
                return None;
            }
            let mut color = interpolate_optional_color(
                &grouped,
                &self.water.color_biomes,
                self.water.color_base?,
            )?;
            for track in self.water.color_tracks.as_ref()? {
                let arg = track.sample(clock_ticks(&track.clock)?);
                color = if track.multiply {
                    std::array::from_fn(|i| color[i] * arg[i])
                } else {
                    arg
                };
            }
            Some(WaterFogSample {
                color,
                range: [range[0], range[1]],
            })
        })();
        Some(BackgroundSample {
            color: value,
            background_is_fog: self.background_is_fog,
            fog_color,
            fog_range,
            sky_mix,
            sunset: self
                .sunset
                .as_ref()
                .and_then(|sunset| sunset.sample(&grouped, &mut clock_ticks)),
            celestial_values: std::array::from_fn(|i| {
                self.celestial_values[i]
                    .as_ref()
                    .and_then(|angle| angle.sample(&grouped, &mut clock_ticks))
            }),
            moon_phase: self.moon_phase.as_ref().and_then(|phase| {
                let biome = if phase.has_biome_layer {
                    let p = position.floor();
                    if p.min_element() < i32::MIN as f64 || p.max_element() > i32::MAX as f64 {
                        return None;
                    }
                    let p = p.as_ivec3();
                    world.biome(p.x, p.y.clamp(self.min_y, self.max_y - 1), p.z)
                } else {
                    None
                };
                phase.sample(biome, &mut clock_ticks)
            }),
            water,
        })
    }
}

fn group_biomes<'a>(
    position: DVec3,
    mut biome: impl FnMut(i32, i32, i32) -> Option<&'a str>,
) -> Option<Vec<(&'a str, f64)>> {
    if !position.is_finite() {
        return None;
    }
    let p = position * 0.25 - DVec3::splat(0.5);
    if p.min_element() < f64::from(i32::MIN + 2) || p.max_element() > f64::from(i32::MAX - 3) {
        return None;
    }
    let floor = p.floor();
    let frac = p - floor;
    let cell = floor.as_ivec3();
    let kernel = [0.0, 1.0, 4.0, 6.0, 4.0, 1.0, 0.0];
    let weight = |i: usize, f: f64| kernel[i + 1] + f * (kernel[i] - kernel[i + 1]);
    let mut grouped: Vec<(&str, f64)> = Vec::new();
    for z in 0..6 {
        for x in 0..6 {
            for y in 0..6 {
                let id = biome(
                    cell.x - 2 + x as i32,
                    cell.y - 2 + y as i32,
                    cell.z - 2 + z as i32,
                )?;
                let w = weight(x, frac.x) * weight(y, frac.y) * weight(z, frac.z);
                if let Some((_, total)) = grouped.iter_mut().find(|(key, _)| *key == id) {
                    *total += w;
                } else {
                    grouped.push((id, w));
                }
            }
        }
    }
    Some(grouped)
}

fn interpolate_color(
    grouped: &[(&str, f64)],
    layers: &BTreeMap<String, Option<ColorLayer>>,
    base: [f32; 3],
) -> Option<[f32; 3]> {
    let mut total = 0.0;
    let mut result: Option<[f32; 3]> = None;
    for &(id, weight) in grouped {
        let color = layers.get(id)?.map_or(base, |layer| layer.apply(base));
        total += weight;
        result = Some(match result {
            None => color,
            Some(current) => {
                let t = (weight / total) as f32;
                std::array::from_fn(|i| current[i] + t * (color[i] - current[i]))
            }
        });
    }
    result
}

fn interpolate_optional_color(
    grouped: &[(&str, f64)],
    layers: &BTreeMap<String, Option<Option<ColorLayer>>>,
    base: [f32; 3],
) -> Option<[f32; 3]> {
    let mut total = 0.0;
    let mut result: Option<[f32; 3]> = None;
    for &(id, weight) in grouped {
        let color = layers
            .get(id)?
            .as_ref()?
            .map_or(base, |layer| layer.apply(base));
        total += weight;
        result = Some(match result {
            None => color,
            Some(current) => {
                let t = (weight / total) as f32;
                std::array::from_fn(|i| current[i] + t * (color[i] - current[i]))
            }
        });
    }
    result
}

fn interpolate_fog(
    grouped: &[(&str, f64)],
    layers: &BTreeMap<String, Option<[Option<FloatLayer>; 3]>>,
    base: [f32; 3],
) -> Option<[f32; 3]> {
    let mut total = 0.0;
    let mut result: Option<[f32; 3]> = None;
    for &(id, weight) in grouped {
        let layers = layers.get(id)?.as_ref()?;
        let value: [f32; 3] =
            std::array::from_fn(|i| layers[i].map_or(base[i], |layer| layer.apply(base[i])));
        total += weight;
        result = Some(match result {
            None => value,
            Some(current) => {
                let t = (weight / total) as f32;
                std::array::from_fn(|i| current[i] + t * (value[i] - current[i]))
            }
        });
    }
    result
}

#[cfg(test)]
fn gaussian_biomes<'a>(
    position: DVec3,
    biome: impl FnMut(i32, i32, i32) -> Option<&'a str>,
    layers: &BTreeMap<String, Option<ColorLayer>>,
    base: [f32; 3],
) -> Option<[f32; 3]> {
    interpolate_color(&group_biomes(position, biome)?, layers, base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sky_light_timeline_requires_its_clock() {
        let mut level = SkyLevel::new(None).unwrap();
        let timeline = serde_json::json!({"clock":"example:clock","period_ticks":20});
        let track = serde_json::json!({"modifier":"multiply","keyframes":[{"ticks":0,"value":1.0},{"ticks":10,"value":0.5}]});
        level.add_track(&timeline, &track).unwrap();
        assert_eq!(
            level.sample(|_| None, native_session::weather::WeatherVisual::default()),
            None
        );
        assert_eq!(
            level.sample(
                |_| Some(0),
                native_session::weather::WeatherVisual::default()
            ),
            Some(15.0)
        );
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn sky_light_level_matches_java_timeline_and_weather() {
        use serde_json::json;
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
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
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("SkyLightOracle.java");
        let input_path = temp.path().join("input.json");
        let output_path = temp.path().join("output.json");
        std::fs::write(&source, include_str!("../tests/java/SkyLightOracle.java")).unwrap();
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let day: Value = serde_json::from_slice(
            &native_resources::packs::ResourceProvider::read_data(
                &mut resources,
                "data/minecraft/timeline/day.json",
            )
            .unwrap()
            .unwrap(),
        )
        .unwrap();
        let tracks = [
            None,
            Some(day["tracks"]["minecraft:gameplay/sky_light_level"].clone()),
            Some(
                json!({"modifier":"add","keyframes":[{"ticks":0,"value":-8.0},{"ticks":12000,"value":4.0}]}),
            ),
            Some(
                json!({"modifier":"subtract","keyframes":[{"ticks":0,"value":2.0},{"ticks":12000,"value":10.0}]}),
            ),
            Some(
                json!({"modifier":"minimum","keyframes":[{"ticks":0,"value":3.0},{"ticks":12000,"value":13.0}]}),
            ),
            Some(
                json!({"modifier":"maximum","keyframes":[{"ticks":0,"value":4.0},{"ticks":12000,"value":12.0}]}),
            ),
            Some(
                json!({"modifier":"override","keyframes":[{"ticks":0,"value":1.0},{"ticks":12000,"value":14.0}]}),
            ),
            Some(
                json!({"modifier":"alpha_blend","keyframes":[{"ticks":0,"value":{"value":2.0,"alpha":0.25}},{"ticks":12000,"value":{"value":14.0,"alpha":0.75}}]}),
            ),
        ];
        let attributes = [
            json!({}),
            json!({"minecraft:gameplay/sky_light_level":8.5}),
            json!({"minecraft:gameplay/sky_light_level":{"modifier":"alpha_blend","argument":{"value":3.0,"alpha":0.4}}}),
        ];
        let ticks = [
            -1, 0, 133, 5000, 11867, 12500, 13670, 18000, 22330, 23999, 24000,
        ];
        let mut cases = Vec::new();
        for attrs in attributes {
            for track in &tracks {
                for tick in ticks {
                    for (rain, thunder) in
                        [(0.0, 0.0), (0.25, 0.0), (1.0, 0.0), (0.5, 0.6), (1.0, 1.0)]
                    {
                        let mut case =
                            json!({"attributes":attrs,"tick":tick,"rain":rain,"thunder":thunder});
                        if let Some(track) = track {
                            case["track"] = track.clone();
                        }
                        cases.push(case);
                    }
                }
            }
        }
        std::fs::write(&input_path, serde_json::to_vec(&cases).unwrap()).unwrap();
        let compile = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("SkyLightOracle")
            .arg(&input_path)
            .arg(&output_path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let expected: Vec<[f32; 2]> =
            serde_json::from_slice(&std::fs::read(output_path).unwrap()).unwrap();
        assert_eq!(expected.len(), cases.len());
        let installed = SkyBackground::load(&mut resources, "minecraft:overworld", -64, 320)
            .unwrap()
            .unwrap();
        let installed_track = &day["tracks"]["minecraft:gameplay/sky_light_level"];
        let timeline = json!({"clock":"minecraft:overworld","period_ticks":24000});
        for (case, expected) in cases.iter().zip(expected) {
            let mut level = SkyLevel::new(Some(&case["attributes"])).unwrap();
            if let Some(track) = case.get("track") {
                level.add_track(&timeline, track).unwrap();
            }
            let rain = case["rain"].as_f64().unwrap() as f32;
            let thunder = case["thunder"].as_f64().unwrap() as f32;
            let weather = native_session::weather::WeatherVisual {
                previous_rain: rain,
                rain,
                previous_thunder: thunder,
                thunder,
            };
            let actual = level
                .sample(|_| Some(case["tick"].as_i64().unwrap()), weather)
                .unwrap();
            assert!(
                (actual - expected[0]).abs() < 0.000_01,
                "case {case}: {actual} != {}",
                expected[0]
            );
            assert_eq!((15.0 - actual) as i32, expected[1] as i32, "case {case}");
            if case["attributes"].as_object().unwrap().is_empty()
                && case.get("track") == Some(installed_track)
            {
                assert_eq!(
                    installed.sky_darken(|_| Some(case["tick"].as_i64().unwrap()), Some(weather)),
                    Some(expected[1] as i32),
                    "installed case {case}"
                );
            }
        }
    }

    #[test]
    fn dark_disc_visibility_tracks_horizon_and_underwater_state() {
        for (min_y, flat, horizon) in [(-64, false, 63.0), (-64, true, -64.0), (32, true, 32.0)] {
            assert!(dark_disc_visible_at(min_y, flat, horizon - 0.01, false));
            assert!(!dark_disc_visible_at(min_y, flat, horizon, false));
            assert!(!dark_disc_visible_at(min_y, flat, horizon - 0.01, true));
        }
    }

    #[test]
    fn weather_uses_fog_layer_for_skyless_background_and_preserves_water() {
        let base = [0.8, 0.3, 0.05];
        let water = WaterFogSample {
            color: [0.1, 0.2, 0.3],
            range: [-8.0, 96.0],
        };
        let mut sample = BackgroundSample {
            color: base,
            background_is_fog: true,
            fog_color: Some(base),
            fog_range: Some([2.0, 20.0]),
            sky_mix: Some((base, 512.0)),
            sunset: None,
            celestial_values: [None; 4],
            moon_phase: None,
            water: Some(water),
        };
        sample.apply_weather(native_session::weather::WeatherVisual {
            previous_rain: 0.0,
            rain: 1.0,
            previous_thunder: 0.0,
            thunder: 1.0,
        });
        assert_eq!(sample.color, sample.fog_color.unwrap());
        assert_eq!(sample.color, crate::sky_weather::fog(base, 1.0, 1.0));
        assert_eq!(
            sample.sky_mix.unwrap().0,
            crate::sky_weather::sky(base, 1.0, 1.0)
        );
        assert_eq!(sample.water, Some(water));
    }

    #[test]
    fn unsupported_sunset_layer_is_limited_to_sampled_biomes() {
        let mut sunset = SunsetTimeline::new(None).unwrap();
        sunset.biome_angles.insert("known".into(), Some(None));
        sunset.biome_colors.insert("known".into(), Some(None));
        sunset.biome_angles.insert("unknown".into(), None);
        sunset.biome_colors.insert("unknown".into(), Some(None));
        assert_eq!(
            sunset.sample(&[("known", 1.0)], |_| None),
            Some((0.0, [0.0; 4]))
        );
        assert!(
            sunset
                .sample(&[("known", 0.5), ("unknown", 0.5)], |_| None)
                .is_none()
        );
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn blended_sky_color_matches_java_sampler() {
        use native_resources::packs::ResourceStack;
        use serde_json::json;
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
        let track = &day["tracks"]["minecraft:visual/sky_color"];
        let biomes = [
            json!({"minecraft:visual/sky_color":"#abcdef","minecraft:visual/fog_color":"#330808","minecraft:visual/fog_start_distance":{"modifier":"add","argument":5.0},"minecraft:visual/fog_end_distance":50.0,"minecraft:visual/sun_angle":350.0,"minecraft:visual/sunrise_sunset_color":{"modifier":"multiply","argument":"#80ff8040"}}),
            json!({"minecraft:visual/sky_color":{"modifier":"multiply","argument":"#7295ff"},"minecraft:visual/fog_color":"#1b4745","minecraft:visual/fog_start_distance":-10.0,"minecraft:visual/fog_end_distance":{"modifier":"multiply","argument":0.5},"minecraft:visual/sun_angle":{"modifier":"add","argument":-25.0},"minecraft:visual/sunrise_sunset_color":{"modifier":"alpha_blend","argument":"#8060d020"}}),
            json!({}),
        ];
        let mut cases = Vec::new();
        for p in [
            [0.0, 64.0, 0.0],
            [-0.01, -20.0, 7.75],
            [123.125, 77.5, -456.25],
            [29999990.5, 120.25, -29999990.5],
        ] {
            for tick in [
                -1, 0, 730, 6000, 11270, 12000, 13140, 18000, 22860, 23999, 24000,
            ] {
                cases.push(json!({"position":p,"tick":tick}));
            }
        }
        let fog_track = json!({"modifier":"multiply","keyframes":[{"ticks":0,"value":1.0},{"ticks":12000,"value":0.5}]});
        let mut fog_color_cases = Vec::new();
        for chunks in [2, 4, 12, 32, 64] {
            for end in [0.0, 128.0, 512.0, 2048.0] {
                fog_color_cases.push(json!({"chunks":chunks,"end":end}));
            }
        }
        let mut weather_cases = Vec::new();
        for rain in [0.0, 0.01, 0.2, 0.5, 1.0] {
            for thunder in [0.0, 0.01, 0.2, 0.5, 1.0] {
                weather_cases.push(json!({"rain":rain,"thunder":thunder}));
            }
        }
        let mut sunrise_cases = Vec::new();
        for angle in [-180.0, -0.1, 0.0, 0.1, 90.0, 180.0, 270.0] {
            for forward in [-1.0, -0.3, 0.0, 0.4, 1.0] {
                for chunks in [3, 4, 12] {
                    sunrise_cases
                        .push(json!({"angle":angle,"forward":forward,"chunks":chunks,"alpha":0.6}));
                }
            }
        }
        let modifier_values = [
            ("override", json!("#a020a0f0"), json!("#7080c040")),
            ("multiply", json!("#a020a0f0"), json!("#7080c040")),
            ("add", json!("#20a0f0"), json!("#80c040")),
            ("subtract", json!("#20a0f0"), json!("#80c040")),
            ("alpha_blend", json!("#a020a0f0"), json!("#7080c040")),
            (
                "blend_to_gray",
                json!({"brightness":0.3,"factor":0.8}),
                json!({"brightness":0.9,"factor":0.2}),
            ),
        ];
        let sunset_modifier_cases: Vec<_> = modifier_values.iter().map(|(name, value, _)| json!({"attributes":{"minecraft:visual/sunrise_sunset_color":{"modifier":name,"argument":value}}})).collect();
        let sunset_track_cases: Vec<_> = modifier_values.iter().map(|(name, a, b)| json!({"track":{"modifier":name,"keyframes":[{"ticks":0,"value":a},{"ticks":10,"value":b}]},"ticks":[0,3,7,10,17]})).collect();
        let input = json!({"biomes":biomes,"track":track,"fogTrack":fog_track,"cases":cases,"fogColorCases":fog_color_cases,"weatherCases":weather_cases,"sunriseCases":sunrise_cases,"sunsetModifierCases":sunset_modifier_cases,"sunsetTrackCases":sunset_track_cases,"moonTrack":day["tracks"]["minecraft:visual/moon_angle"],"starAngleTrack":day["tracks"]["minecraft:visual/star_angle"],"starBrightnessTrack":day["tracks"]["minecraft:visual/star_brightness"]});
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("SkyBackgroundOracle.java");
        let input_file = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/SkyBackgroundOracle.java"),
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
            .arg("SkyBackgroundOracle")
            .arg(&input_file)
            .arg(&output));
        let expected: Vec<Vec<f32>> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let layers = BTreeMap::from([
            (
                "a".into(),
                find_color_attribute(&biomes[0], "visual/sky_color").unwrap(),
            ),
            (
                "b".into(),
                find_color_attribute(&biomes[1], "visual/sky_color").unwrap(),
            ),
            ("c".into(), None),
        ]);
        let fog_color_layers = BTreeMap::from([
            (
                "a".into(),
                find_color_attribute(&biomes[0], "visual/fog_color").unwrap(),
            ),
            (
                "b".into(),
                find_color_attribute(&biomes[1], "visual/fog_color").unwrap(),
            ),
            ("c".into(), None),
        ]);
        let sky_track = SkyTrack::parse(&day, track, false).unwrap();
        let fog_distance_track = SkyTrack::parse(&day, &fog_track, true).unwrap();
        let fog_distance_layers = BTreeMap::from([
            ("a".into(), Some(fog_layers(Some(&biomes[0])).unwrap())),
            ("b".into(), Some(fog_layers(Some(&biomes[1])).unwrap())),
            ("c".into(), Some([None; 3])),
        ]);
        let mut sunset_biomes = SunsetTimeline::new(None).unwrap();
        sunset_biomes.base_angle = 20.0;
        sunset_biomes.base_color = [0.2, 0.4, 0.6, 0.8];
        for (name, biome) in ["a", "b", "c"].into_iter().zip(&biomes) {
            sunset_biomes.biome_angles.insert(
                name.into(),
                Some(
                    named_attribute(Some(biome), "visual/sun_angle")
                        .unwrap()
                        .map(FloatLayer::parse)
                        .transpose()
                        .unwrap(),
                ),
            );
            sunset_biomes.biome_colors.insert(
                name.into(),
                Some(
                    named_attribute(Some(biome), "visual/sunrise_sunset_color")
                        .unwrap()
                        .map(SunsetColorLayer::parse)
                        .transpose()
                        .unwrap(),
                ),
            );
        }
        assert_eq!(
            expected.len(),
            cases.len()
                + 6
                + fog_color_cases.len()
                + weather_cases.len()
                + sunrise_cases.len()
                + sunset_modifier_cases.len()
                + sunset_track_cases.len() * 5
                + (-100..=24100).step_by(37).count() * 3
                + weather_cases.len() * 2
        );
        for (case, expected) in cases.iter().zip(&expected) {
            let p = &case["position"];
            let position = DVec3::new(
                p[0].as_f64().unwrap(),
                p[1].as_f64().unwrap(),
                p[2].as_f64().unwrap(),
            );
            let mut actual = gaussian_biomes(
                position,
                |x, y, z| Some(["a", "b", "c"][(x + 3 * y + 7 * z).rem_euclid(3) as usize]),
                &layers,
                [120.0 / 255.0, 167.0 / 255.0, 1.0],
            )
            .unwrap();
            let factor = sky_track.sample(case["tick"].as_i64().unwrap());
            let fog = gaussian_biomes(
                position,
                |x, y, z| Some(["a", "b", "c"][(x + 3 * y + 7 * z).rem_euclid(3) as usize]),
                &fog_color_layers,
                [0.0; 3],
            )
            .unwrap();
            let grouped = group_biomes(position, |x, y, z| {
                Some(["a", "b", "c"][(x + 3 * y + 7 * z).rem_euclid(3) as usize])
            })
            .unwrap();
            let (angle, color) = sunset_biomes.sample(&grouped, |_| Some(0)).unwrap();
            assert!((angle - expected[8]).abs() < 0.0001, "angle case {case}");
            for i in 0..4 {
                assert!(
                    (color[i] - expected[9 + i]).abs() < 0.000002,
                    "sunset biome case {case}, channel {i}"
                );
            }
            let mut distance =
                interpolate_fog(&grouped, &fog_distance_layers, [10.0, 96.0, 512.0]).unwrap();
            distance[1] *= fog_distance_track.sample(case["tick"].as_i64().unwrap())[0];
            for i in 0..2 {
                assert!(
                    (distance[i] - expected[i + 6]).abs() < 0.0001,
                    "distance case {case}, channel {i}: {} != {}",
                    distance[i],
                    expected[i + 6]
                );
            }
            for i in 0..3 {
                actual[i] *= factor[i];
                assert!(
                    (actual[i] - expected[i]).abs() < 0.000002,
                    "case {case}, channel {i}: {} != {}",
                    actual[i],
                    expected[i]
                );
                assert!(
                    (fog[i] - expected[i + 3]).abs() < 0.000002,
                    "fog case {case}, channel {i}: {} != {}",
                    fog[i],
                    expected[i + 3]
                );
            }
        }
        for (name, expected) in [
            "override", "add", "subtract", "multiply", "minimum", "maximum",
        ]
        .into_iter()
        .zip(&expected[cases.len()..cases.len() + 6])
        {
            let layer = FloatLayer::parse(&json!({"modifier":name,"argument":3.0})).unwrap();
            assert_eq!(layer.apply(10.0), expected[0], "modifier {name}");
        }
        for (case, expected) in fog_color_cases.iter().zip(&expected[cases.len() + 6..]) {
            let actual = crate::preview_app::atmospheric_fog_color(
                [0.2, 0.6, 0.9],
                [0.8, 0.3, 0.05],
                case["end"].as_f64().unwrap() as f32,
                case["chunks"].as_u64().unwrap() as u32,
            );
            for i in 0..3 {
                assert!(
                    (actual[i] - expected[i]).abs() < 0.000002,
                    "fog color case {case}, channel {i}: {} != {}",
                    actual[i],
                    expected[i]
                );
            }
        }
        for (case, expected) in weather_cases
            .iter()
            .zip(&expected[cases.len() + 6 + fog_color_cases.len()..])
        {
            let actual = crate::preview_app::weather_darken_sky(
                [0.8, 0.3, 0.05],
                case["rain"].as_f64().unwrap() as f32,
                case["thunder"].as_f64().unwrap() as f32,
            );
            for i in 0..3 {
                assert!(
                    (actual[i] - expected[i]).abs() < 0.000002,
                    "weather {case}, channel {i}"
                );
            }
        }
        for (case, expected) in sunrise_cases
            .iter()
            .zip(&expected[cases.len() + 6 + fog_color_cases.len() + weather_cases.len()..])
        {
            let actual = crate::preview_app::sunrise_fog_color(
                [0.2, 0.6, 0.9],
                case["angle"].as_f64().unwrap() as f32,
                [0.9, 0.4, 0.1, 0.6],
                case["forward"].as_f64().unwrap() as f32,
                case["chunks"].as_u64().unwrap() as u32,
            );
            for i in 0..3 {
                assert!(
                    (actual[i] - expected[i]).abs() < 0.000002,
                    "sunrise {case}, channel {i}"
                );
            }
        }
        let mut offset =
            cases.len() + 6 + fog_color_cases.len() + weather_cases.len() + sunrise_cases.len();
        for case in &sunset_modifier_cases {
            let layer = SunsetColorLayer::parse(
                &case["attributes"]["minecraft:visual/sunrise_sunset_color"],
            )
            .unwrap();
            let actual = layer.apply([0.2, 0.4, 0.6, 0.8]);
            for i in 0..4 {
                assert!(
                    (actual[i] - expected[offset][i]).abs() < 0.000002,
                    "modifier {case}, channel {i}"
                );
            }
            offset += 1;
        }
        let test_timeline = json!({"clock":"minecraft:overworld", "period_ticks":20});
        for case in &sunset_track_cases {
            let track = SunsetColorTrack::parse(&test_timeline, &case["track"]).unwrap();
            for tick in case["ticks"].as_array().unwrap() {
                let actual = track.apply(tick.as_i64().unwrap(), [0.2, 0.4, 0.6, 0.8]);
                for i in 0..4 {
                    assert!(
                        (actual[i] - expected[offset][i]).abs() < 0.000002,
                        "track {case} tick {tick}, channel {i}: {} != {}",
                        actual[i],
                        expected[offset][i]
                    );
                }
                offset += 1;
            }
        }
        let overworld = SkyBackground::load(&mut resources, "minecraft:overworld", -64, 320)
            .unwrap()
            .unwrap();
        assert!(!overworld.background_is_fog);
        assert_eq!(
            overworld.fog_color_base,
            Some([192.0 / 255.0, 216.0 / 255.0, 1.0])
        );
        assert!(overworld.fog_base.is_some());
        assert!(overworld.fog_tracks.is_some());
        assert!(overworld.fog_color_tracks.is_some());
        assert!(overworld.sunset.is_some());
        assert!(!overworld.moon_phase.as_ref().unwrap().has_biome_layer);
        for (day, phase) in crate::moon_phase::Phase::ALL.into_iter().enumerate() {
            assert_eq!(
                overworld
                    .moon_phase
                    .as_ref()
                    .unwrap()
                    .sample(None, |_| Some(day as i64 * 24000)),
                Some(phase)
            );
        }
        assert_eq!(overworld.water.base, Some([-8.0, 96.0, 0.0]));
        assert_eq!(
            overworld.water.color_base,
            Some([5.0 / 255.0, 5.0 / 255.0, 51.0 / 255.0])
        );
        let swamp = &[("minecraft:swamp", 1.0)];
        assert_eq!(
            interpolate_optional_color(
                swamp,
                &overworld.water.color_biomes,
                overworld.water.color_base.unwrap()
            ),
            Some([35.0 / 255.0, 35.0 / 255.0, 23.0 / 255.0])
        );
        assert_eq!(
            interpolate_fog(
                swamp,
                &overworld.water.biomes,
                overworld.water.base.unwrap()
            )
            .unwrap()[1],
            96.0 * 0.85
        );
        let sun =
            SkyTrack::parse(&day, &day["tracks"]["minecraft:visual/sun_angle"], true).unwrap();
        let sunset = SunsetColorTrack::parse(
            &day,
            &day["tracks"]["minecraft:visual/sunrise_sunset_color"],
        )
        .unwrap();
        for tick in [0, 6000, 12000, 12512, 18000, 23272] {
            let (angle, color) = overworld
                .sunset
                .as_ref()
                .unwrap()
                .sample(&[("minecraft:plains", 1.0)], |_| Some(tick))
                .unwrap();
            assert_eq!(angle, sun.sample(tick)[0]);
            assert_eq!(
                overworld.celestial_values[0]
                    .as_ref()
                    .unwrap()
                    .sample(&[("minecraft:plains", 1.0)], |_| Some(tick)),
                Some(angle)
            );
            assert_eq!(color, sunset.apply(tick, [0.0; 4]));
        }
        for tick in (-100..=24100).step_by(37) {
            let actual = overworld.celestial_values[1]
                .as_ref()
                .unwrap()
                .sample(&[("minecraft:plains", 1.0)], |_| Some(tick))
                .unwrap();
            assert!(
                (actual - expected[offset][0]).abs() < 0.000_1,
                "moon tick {tick}: {actual} != {}",
                expected[offset][0]
            );
            offset += 1;
        }
        for index in [2, 3] {
            for tick in (-100..=24100).step_by(37) {
                let actual = overworld.celestial_values[index]
                    .as_ref()
                    .unwrap()
                    .sample(&[("minecraft:plains", 1.0)], |_| Some(tick))
                    .unwrap();
                assert!(
                    (actual - expected[offset][0]).abs() < 0.000_1,
                    "star value {index}, tick {tick}: {actual} != {}",
                    expected[offset][0]
                );
                offset += 1;
            }
        }
        for case in &weather_cases {
            let actual = crate::sky_weather::stars(
                0.5,
                case["rain"].as_f64().unwrap() as f32,
                case["thunder"].as_f64().unwrap() as f32,
            );
            assert!(
                (actual - expected[offset][0]).abs() < 0.000_001,
                "star weather {case}: {actual} != {}",
                expected[offset][0]
            );
            offset += 1;
        }
        let star_weather_offset = offset - weather_cases.len();
        for (case_index, case) in weather_cases.iter().enumerate() {
            let rain = case["rain"].as_f64().unwrap() as f32;
            let thunder = case["thunder"].as_f64().unwrap() as f32;
            let actual = crate::sky_weather::sky([0.8, 0.3, 0.05], rain, thunder)
                .into_iter()
                .chain(crate::sky_weather::fog([0.8, 0.3, 0.05], rain, thunder))
                .chain(crate::sky_weather::sunrise(
                    [0.8, 0.3, 0.05, 0.4],
                    rain,
                    thunder,
                ));
            for (channel, (actual, expected)) in actual.zip(&expected[offset]).enumerate() {
                assert!(
                    (actual - expected).abs() < 0.000_001,
                    "weather {case}, channel {channel}: {actual} != {expected}"
                );
            }
            if thunder <= rain {
                let mut sample = BackgroundSample {
                    color: [0.8, 0.3, 0.05],
                    background_is_fog: false,
                    fog_color: Some([0.8, 0.3, 0.05]),
                    fog_range: Some([2.0, 5.0]),
                    sky_mix: Some(([0.8, 0.3, 0.05], 512.0)),
                    sunset: Some((120.0, [0.8, 0.3, 0.05, 0.4])),
                    celestial_values: [Some(1.0), Some(2.0), Some(3.0), Some(0.5)],
                    moon_phase: None,
                    water: None,
                };
                sample.apply_weather(native_session::weather::WeatherVisual {
                    previous_rain: 1.0,
                    rain,
                    previous_thunder: 1.0,
                    thunder: if rain == 0.0 { 0.0 } else { thunder / rain },
                });
                let reference = &expected[offset];
                for (channel, actual) in sample
                    .color
                    .into_iter()
                    .chain(sample.fog_color.unwrap())
                    .chain(sample.sunset.unwrap().1)
                    .enumerate()
                {
                    assert!(
                        (actual - reference[channel]).abs() < 0.000_001,
                        "sample weather {case}, channel {channel}"
                    );
                }
                assert_eq!(sample.sky_mix.unwrap().1, 512.0);
                assert_eq!(sample.sky_mix.unwrap().0, sample.color);
                assert_eq!(sample.sunset.unwrap().0, 120.0);
                assert_eq!(
                    sample.celestial_values[..3],
                    [Some(1.0), Some(2.0), Some(3.0)]
                );
                assert!(
                    (sample.celestial_values[3].unwrap()
                        - expected[star_weather_offset + case_index][0])
                        .abs()
                        < 0.000_001
                );
            }
            offset += 1;
        }
        let nether = SkyBackground::load(&mut resources, "minecraft:the_nether", 0, 128)
            .unwrap()
            .unwrap();
        assert_eq!(nether.fog_base, Some([10.0, 96.0, 512.0]));
        for (biome, expected) in [
            ("minecraft:nether_wastes", [0x33, 0x08, 0x08]),
            ("minecraft:soul_sand_valley", [0x1b, 0x47, 0x45]),
        ] {
            let layer = nether.biomes.get(biome).unwrap().unwrap();
            assert!(!layer.multiply);
            assert_eq!(layer.value, expected.map(|channel| channel as f32 / 255.0));
        }
        let end = SkyBackground::load(&mut resources, "minecraft:the_end", 0, 256)
            .unwrap()
            .unwrap();
        assert!(end.has_end_sky());
        assert_eq!(end.base, [0.0; 3]);
        assert_eq!(
            end.fog_color_base,
            Some([24.0 / 255.0, 19.0 / 255.0, 24.0 / 255.0])
        );
        assert_eq!(end.water.base, Some([-8.0, 96.0, 0.0]));
    }
    #[test]
    fn gaussian_preserves_uniform_biome_and_requires_loaded_samples() {
        let layers = BTreeMap::from([(
            "minecraft:plains".into(),
            Some(ColorLayer {
                value: [0.2, 0.4, 0.6],
                multiply: false,
            }),
        )]);
        for p in [DVec3::ZERO, DVec3::new(-100.25, 64.0, 33.75)] {
            assert_eq!(
                gaussian_biomes(p, |_, _, _| Some("minecraft:plains"), &layers, [1.0; 3]),
                Some([0.2, 0.4, 0.6])
            );
        }
        assert!(
            gaussian_biomes(
                DVec3::ZERO,
                |x, _, _| (x != 0).then_some("minecraft:plains"),
                &layers,
                [1.0; 3]
            )
            .is_none()
        );
    }

    #[test]
    fn celestial_values_are_independent_of_sunset_and_each_other() {
        use serde_json::json;
        struct Data;
        impl ResourceProvider for Data {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                let value = match path {
                    "data/example/dimension_type/test.json" => {
                        json!({"timelines":["example:day"],"attributes":{
                            "visual/sun_angle":10.0,"visual/moon_angle":100.0,
                            "visual/star_angle":40.0,"visual/star_brightness":0.1,
                            "visual/sunrise_sunset_color":{"modifier":"unsupported","argument":"#ffffff"}
                        }})
                    }
                    "data/example/worldgen/biome/test.json" => json!({"attributes":{
                        "visual/sun_angle":{"modifier":"add","argument":5.0},
                        "visual/moon_angle":{"modifier":"multiply","argument":2.0},
                        "visual/star_angle":{"modifier":"subtract","argument":5.0},
                        "visual/star_brightness":{"modifier":"maximum","argument":0.2}
                    }}),
                    "data/example/timeline/day.json" => {
                        json!({"clock":"example:clock","period_ticks":20,"tracks":{
                            "visual/sun_angle":{"modifier":"add","keyframes":[{"ticks":0,"value":20.0}]},
                            "visual/moon_angle":{"modifier":"subtract","keyframes":[{"ticks":0,"value":30.0}]},
                            "visual/star_angle":{"modifier":"add","keyframes":[{"ticks":0,"value":15.0}]},
                            "visual/star_brightness":{"modifier":"maximum","keyframes":[{"ticks":0,"value":0.4}]}
                        }})
                    }
                    _ => return Ok(None),
                };
                Ok(Some(serde_json::to_vec(&value)?))
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                Ok(if folder == "worldgen/biome" {
                    vec![ResourceId::parse("example:test")?]
                } else {
                    vec![]
                })
            }
        }
        let background = SkyBackground::load(&mut Data, "example:test", 0, 16)
            .unwrap()
            .unwrap();
        assert!(background.sunset.is_none());
        let groups = [("example:test", 1.0)];
        assert_eq!(
            background.celestial_values[0]
                .as_ref()
                .unwrap()
                .sample(&groups, |_| Some(0)),
            Some(35.0)
        );
        assert_eq!(
            background.celestial_values[1]
                .as_ref()
                .unwrap()
                .sample(&groups, |_| Some(0)),
            Some(170.0)
        );
        assert_eq!(
            background.celestial_values[1]
                .as_ref()
                .unwrap()
                .sample(&groups, |_| None),
            None
        );
        assert_eq!(
            background.celestial_values[2]
                .as_ref()
                .unwrap()
                .sample(&groups, |_| Some(0)),
            Some(50.0)
        );
        assert_eq!(
            background.celestial_values[3]
                .as_ref()
                .unwrap()
                .sample(&groups, |_| Some(0)),
            Some(0.4)
        );
    }

    #[test]
    fn skyless_dimension_loads_fog_attributes_and_matching_timeline() {
        use serde_json::json;
        struct Data;
        impl ResourceProvider for Data {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                let value = match path {
                    "data/example/dimension_type/test.json" => json!({
                        "skybox":"none", "timelines":["example:day"],
                        "attributes":{"visual/fog_color":"#ffffff","visual/sky_color":"#ff0000","visual/fog_start_distance":10.0,"visual/fog_end_distance":96.0,"visual/water_fog_color":"#102030","visual/water_fog_start_distance":-4.0,"visual/water_fog_end_distance":80.0}
                    }),
                    "data/example/worldgen/biome/test.json" => json!({"attributes":{
                        "visual/fog_color":"#204060", "visual/sky_color":"#00ff00",
                        "visual/fog_start_distance":{"modifier":"add","argument":5.0}, "visual/fog_end_distance":64.0,
                        "visual/water_fog_color":"#204060", "visual/water_fog_end_distance":{"modifier":"multiply","argument":0.5}
                    }}),
                    "data/example/timeline/day.json" => json!({
                        "clock":"example:clock", "period_ticks":20, "tracks":{
                            "visual/fog_color":{"modifier":"multiply","keyframes":[{"ticks":0,"value":"#ffffff"},{"ticks":10,"value":"#000000"}]},
                            "visual/fog_end_distance":{"modifier":"add","keyframes":[{"ticks":0,"value":0.0},{"ticks":10,"value":16.0}]},
                            "visual/sky_color":{"keyframes":[{"ticks":0,"value":"#ff0000"}]},
                            "visual/water_fog_start_distance":{"modifier":"add","keyframes":[{"ticks":0,"value":0.0},{"ticks":10,"value":4.0}]},
                            "visual/water_fog_color":{"modifier":"multiply","keyframes":[{"ticks":0,"value":"#ffffff"},{"ticks":10,"value":"#808080"}]}
                        }
                    }),
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
        let background = SkyBackground::load(&mut Data, "example:test", 0, 16)
            .unwrap()
            .unwrap();
        assert_eq!(background.base, [1.0; 3]);
        assert_eq!(background.fog_base, Some([10.0, 96.0, 512.0]));
        let groups = group_biomes(DVec3::ZERO, |_, _, _| Some("example:test")).unwrap();
        assert_eq!(background.water.base, Some([-4.0, 80.0, 0.0]));
        assert_eq!(
            interpolate_fog(
                &groups,
                &background.water.biomes,
                background.water.base.unwrap()
            ),
            Some([-4.0, 40.0, 0.0])
        );
        assert_eq!(
            interpolate_optional_color(
                &groups,
                &background.water.color_biomes,
                background.water.color_base.unwrap()
            ),
            Some([32.0 / 255.0, 64.0 / 255.0, 96.0 / 255.0])
        );
        let water_track = &background.water.tracks.as_ref().unwrap()[0];
        assert_eq!(
            FloatLayer {
                value: water_track.track.sample(10)[0],
                modifier: water_track.modifier
            }
            .apply(-4.0),
            0.0
        );
        assert_eq!(
            background.water.color_tracks.as_ref().unwrap()[0].sample(10),
            [128.0 / 255.0; 3]
        );
        assert_eq!(
            interpolate_optional_color(
                &groups,
                &background.fog_sky_biomes,
                background.fog_sky_base.unwrap()
            ),
            Some([0.0, 1.0, 0.0])
        );
        assert_eq!(
            background.fog_sky_tracks.as_ref().unwrap()[0].sample(0),
            [1.0, 0.0, 0.0]
        );
        let mut range = interpolate_fog(
            &groups,
            &background.fog_biomes,
            background.fog_base.unwrap(),
        )
        .unwrap();
        assert_eq!(range, [15.0, 64.0, 512.0]);
        let track = &background.fog_tracks.as_ref().unwrap()[0];
        range[track.index] = FloatLayer {
            value: track.track.sample(10)[0],
            modifier: track.modifier,
        }
        .apply(range[track.index]);
        assert_eq!(range, [15.0, 80.0, 512.0]);
        let fog = gaussian_biomes(
            DVec3::ZERO,
            |_, _, _| Some("example:test"),
            &background.biomes,
            background.base,
        )
        .unwrap();
        assert_eq!(fog, [32.0 / 255.0, 64.0 / 255.0, 96.0 / 255.0]);
        assert_eq!(background.tracks.len(), 1);
        assert!(background.tracks[0].multiply);
        assert_eq!(background.tracks[0].sample(10), [0.0; 3]);
        struct ModifiedBiome;
        impl ResourceProvider for ModifiedBiome {
            fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                Data.read_asset(path)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/example/worldgen/biome/test.json" {
                    return Ok(Some(serde_json::to_vec(
                        &json!({"attributes":{"visual/sun_angle":15.0}}),
                    )?));
                }
                Data.read_data(path)
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                Data.list_data(folder)
            }
        }
        let modified = SkyBackground::load(&mut ModifiedBiome, "example:test", 0, 16)
            .unwrap()
            .unwrap();
        assert_eq!(
            modified
                .sunset
                .as_ref()
                .unwrap()
                .sample(&[("example:test", 1.0)], |_| Some(0))
                .unwrap()
                .0,
            15.0
        );
    }

    #[test]
    fn samples_loaded_world_and_time_without_fabricating_missing_biomes() {
        use native_world::loaded::{BIOME_COUNT, Chunk, ChunkPos, Section, World};
        use serde_json::json;
        use std::sync::Arc;
        let mut world = World::new("minecraft:overworld");
        let plains = world.intern_biome("minecraft:plains".into());
        for x in -1..=1 {
            for z in -1..=1 {
                let position = ChunkPos { x, z };
                world.chunks.insert(
                    position,
                    Arc::new(Chunk {
                        block_light_correct: false,
                        position,
                        status: None,
                        sections: BTreeMap::from([(
                            0,
                            Arc::new(Section {
                                blocks: None,
                                biomes: Some(Box::new([plains; BIOME_COUNT])),
                                block_light: None,
                                sky_light: None,
                            }),
                        )]),
                    }),
                );
            }
        }
        let timeline = json!({"clock":"example:clock","period_ticks":20});
        let track = json!({"modifier":"multiply","keyframes":[{"ticks":0,"value":"#ffffff"},{"ticks":10,"value":"#000000"}]});
        let mut background = SkyBackground {
            base: [0.0; 3],
            ambient_light: 0.0,
            sky_level: None,
            background_is_fog: false,
            end_sky: false,
            fog_color_base: None,
            fog_color_biomes: BTreeMap::new(),
            fog_color_tracks: None,
            biomes: BTreeMap::from([(
                "minecraft:plains".into(),
                Some(ColorLayer {
                    value: [0.2, 0.4, 0.6],
                    multiply: false,
                }),
            )]),
            tracks: vec![SkyTrack::parse(&timeline, &track, false).unwrap()],
            fog_base: None,
            fog_biomes: BTreeMap::from([("minecraft:plains".into(), Some([None; 3]))]),
            fog_tracks: Some(Vec::new()),
            fog_sky_base: None,
            fog_sky_biomes: BTreeMap::new(),
            fog_sky_tracks: None,
            sunset: None,
            celestial_values: [None, None, None, None],
            moon_phase: None,
            water: WaterFog::default(),
            flat_world: None,
            min_y: 0,
            max_y: 16,
        };
        assert_eq!(
            background.sample(&world, DVec3::new(0.0, 8.0, 0.0), |_| Some(0)),
            Some(BackgroundSample {
                color: [0.2, 0.4, 0.6],
                background_is_fog: false,
                fog_color: None,
                fog_range: None,
                sky_mix: None,
                sunset: None,
                celestial_values: [None, None, None, None],
                moon_phase: None,
                water: None
            })
        );
        assert_eq!(
            background.sample(&world, DVec3::new(0.0, 8.0, 0.0), |_| Some(10)),
            Some(BackgroundSample {
                color: [0.0; 3],
                background_is_fog: false,
                fog_color: None,
                fog_range: None,
                sky_mix: None,
                sunset: None,
                celestial_values: [None, None, None, None],
                moon_phase: None,
                water: None
            })
        );
        assert!(
            background
                .sample(&world, DVec3::new(0.0, 8.0, 0.0), |_| None)
                .is_none()
        );
        background.water = WaterFog {
            base: Some([-8.0, 96.0, 0.0]),
            biomes: BTreeMap::from([("minecraft:plains".into(), Some([None; 3]))]),
            tracks: Some(Vec::new()),
            color_base: Some([0.1, 0.2, 0.3]),
            color_biomes: BTreeMap::from([("minecraft:plains".into(), Some(None))]),
            color_tracks: Some(Vec::new()),
        };
        assert_eq!(
            background
                .sample(&world, DVec3::new(0.0, 8.0, 0.0), |_| Some(0))
                .unwrap()
                .water,
            Some(WaterFogSample {
                color: [0.1, 0.2, 0.3],
                range: [-8.0, 96.0]
            })
        );
        background.water.color_tracks = None;
        assert_eq!(
            background
                .sample(&world, DVec3::new(0.0, 8.0, 0.0), |_| Some(0))
                .unwrap()
                .water,
            None
        );
        background.fog_base = Some([10.0, 96.0, 512.0]);
        background.fog_color_base = Some([0.7, 0.8, 0.9]);
        background
            .fog_color_biomes
            .insert("minecraft:plains".into(), Some(None));
        background.fog_color_tracks = Some(Vec::new());
        let sample = background
            .sample(&world, DVec3::new(0.0, 8.0, 0.0), |_| Some(0))
            .unwrap();
        assert_eq!(sample.color, [0.2, 0.4, 0.6]);
        assert_eq!(sample.fog_color, Some([0.7, 0.8, 0.9]));
        assert_eq!(sample.sky_mix, Some(([0.2, 0.4, 0.6], 512.0)));
        assert!(!sample.background_is_fog);
        background.background_is_fog = true;
        background.fog_sky_base = Some([0.1, 0.2, 0.3]);
        background
            .fog_sky_biomes
            .insert("minecraft:plains".into(), Some(None));
        background.fog_sky_tracks = Some(Vec::new());
        assert_eq!(
            background
                .sample(&world, DVec3::new(0.0, 8.0, 0.0), |_| Some(0))
                .unwrap()
                .sky_mix,
            Some(([0.1, 0.2, 0.3], 512.0))
        );
        assert_eq!(
            background
                .sample(&world, DVec3::new(0.0, 8.0, 0.0), |_| Some(0))
                .unwrap()
                .fog_range,
            Some([10.0, 96.0])
        );
        background.fog_biomes.insert(
            "minecraft:plains".into(),
            Some([
                Some(FloatLayer {
                    value: 5.0,
                    modifier: FloatModifier::Add,
                }),
                Some(FloatLayer {
                    value: 0.5,
                    modifier: FloatModifier::Multiply,
                }),
                None,
            ]),
        );
        background.fog_tracks = Some(vec![
            FogTrack::parse(
                1,
                &timeline,
                &json!({"modifier":"add","keyframes":[{"ticks":0,"value":8.0}]}),
            )
            .unwrap(),
        ]);
        assert_eq!(
            background
                .sample(&world, DVec3::new(0.0, 8.0, 0.0), |_| Some(0))
                .unwrap()
                .fog_range,
            Some([15.0, 56.0])
        );
        background
            .fog_biomes
            .insert("minecraft:plains".into(), None);
        assert_eq!(
            background
                .sample(&world, DVec3::new(0.0, 8.0, 0.0), |_| Some(0))
                .unwrap()
                .fog_range,
            None
        );
        world.chunks.remove(&ChunkPos { x: -1, z: -1 });
        assert!(
            background
                .sample(&world, DVec3::new(0.0, 8.0, 0.0), |_| Some(0))
                .is_none()
        );
    }
}
