//! Dimension-wide sky light used by the server and renderer.

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use crate::{
    creature_timeline::Easing,
    packs::{ResourceId, ResourceProvider},
};

#[derive(Clone, Copy, Debug)]
enum Modifier {
    Override,
    Add,
    Subtract,
    Multiply,
    Minimum,
    Maximum,
    AlphaBlend,
}

impl Modifier {
    fn parse(name: &str) -> Result<Self> {
        let id = ResourceId::parse(name)?;
        ensure!(
            id.namespace == "minecraft",
            "unsupported sky light modifier"
        );
        Ok(match id.path.as_str() {
            "override" => Self::Override,
            "add" => Self::Add,
            "subtract" => Self::Subtract,
            "multiply" => Self::Multiply,
            "minimum" => Self::Minimum,
            "maximum" => Self::Maximum,
            "alpha_blend" => Self::AlphaBlend,
            _ => bail!("unsupported sky light modifier {name}"),
        })
    }
    fn apply(self, base: f32, value: [f32; 2]) -> f32 {
        match self {
            Self::Override => value[0],
            Self::Add => base + value[0],
            Self::Subtract => base - value[0],
            Self::Multiply => base * value[0],
            Self::Minimum => base.min(value[0]),
            Self::Maximum => base.max(value[0]),
            Self::AlphaBlend => base + value[1] * (value[0] - base),
        }
    }
    fn argument(self, value: &Value) -> Result<[f32; 2]> {
        let (value, alpha) = if matches!(self, Self::AlphaBlend) && value.is_object() {
            (
                value
                    .get("value")
                    .and_then(Value::as_f64)
                    .context("missing alpha-blend value")? as f32,
                value
                    .get("alpha")
                    .map(|v| v.as_f64().context("invalid alpha"))
                    .transpose()?
                    .unwrap_or(1.0) as f32,
            )
        } else {
            (
                value.as_f64().context("invalid sky light value")? as f32,
                1.0,
            )
        };
        ensure!(
            value.is_finite() && (0.0..=1.0).contains(&alpha),
            "invalid sky light argument"
        );
        Ok([value, alpha])
    }
}

#[derive(Clone, Debug)]
struct Track {
    clock: ResourceId,
    period: Option<i32>,
    easing: Easing,
    modifier: Modifier,
    frames: Vec<(i32, [f32; 2])>,
}

impl Track {
    fn parse(timeline: &Value, track: &Value) -> Result<Self> {
        let clock = ResourceId::parse(
            timeline
                .get("clock")
                .and_then(Value::as_str)
                .context("missing timeline clock")?,
        )?;
        let period = timeline
            .get("period_ticks")
            .map(|v| {
                let n = v
                    .as_i64()
                    .and_then(|v| i32::try_from(v).ok())
                    .context("invalid period")?;
                ensure!(n > 0, "invalid period");
                Ok(n)
            })
            .transpose()?;
        let modifier = Modifier::parse(
            track
                .get("modifier")
                .map(|v| v.as_str().context("invalid modifier"))
                .transpose()?
                .unwrap_or("override"),
        )?;
        let easing = Easing::parse(track.get("ease"))?;
        let values = track
            .get("keyframes")
            .and_then(Value::as_array)
            .context("missing keyframes")?;
        ensure!(!values.is_empty(), "empty keyframes");
        let mut frames: Vec<(i32, [f32; 2])> = Vec::with_capacity(values.len());
        for frame in values {
            let tick = frame
                .get("ticks")
                .and_then(Value::as_i64)
                .and_then(|v| i32::try_from(v).ok())
                .context("invalid keyframe tick")?;
            if let Some(p) = period {
                ensure!((0..=p).contains(&tick), "keyframe outside period");
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
            frames.push((
                tick,
                modifier.argument(frame.get("value").context("missing keyframe value")?)?,
            ));
        }
        Ok(Self {
            clock,
            period,
            easing,
            modifier,
            frames,
        })
    }
    fn sample(&self, ticks: i64) -> [f32; 2] {
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
            .unwrap_or_else(|| segments.last().expect("multiple keyframes"));
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

/// Samples the dimension layer, active timelines and weather in Java order.
#[derive(Clone, Debug)]
pub struct SkyLevel {
    base: f32,
    tracks: Vec<Track>,
}

impl SkyLevel {
    pub fn new(attributes: Option<&Value>) -> Result<Self> {
        let mut base = 15.0;
        if let Some(attributes) = attributes {
            let mut found = false;
            for (key, value) in attributes
                .as_object()
                .context("invalid environment attributes")?
            {
                let id = ResourceId::parse(key)?;
                if id.namespace != "minecraft" || id.path != "gameplay/sky_light_level" {
                    continue;
                }
                ensure!(!found, "duplicate sky light level");
                found = true;
                let (modifier, argument) = if value.is_number() {
                    (Modifier::Override, value)
                } else {
                    (
                        Modifier::parse(
                            value
                                .get("modifier")
                                .and_then(Value::as_str)
                                .context("missing float modifier")?,
                        )?,
                        value.get("argument").context("missing float argument")?,
                    )
                };
                base = modifier.apply(base, modifier.argument(argument)?);
            }
        }
        Ok(Self {
            base,
            tracks: Vec::new(),
        })
    }
    pub fn add_track(&mut self, timeline: &Value, track: &Value) -> Result<()> {
        self.tracks.push(Track::parse(timeline, track)?);
        Ok(())
    }
    pub fn from_definition(
        provider: &mut impl ResourceProvider,
        definition: &Value,
    ) -> Result<Self> {
        ensure!(definition.is_object(), "dimension type must be an object");
        let mut result = Self::new(definition.get("attributes"))?;
        for timeline in
            crate::fluid_environment::dimension_timelines_from_definition(provider, definition)?
        {
            let tracks = timeline
                .get("tracks")
                .and_then(Value::as_object)
                .context("invalid timeline tracks")?;
            let mut found = false;
            for (key, track) in tracks {
                let id = ResourceId::parse(key)?;
                if id.namespace == "minecraft" && id.path == "gameplay/sky_light_level" {
                    ensure!(!found, "duplicate sky light track");
                    found = true;
                    result.add_track(&timeline, track)?;
                }
            }
        }
        Ok(result)
    }
    /// `thunder` is already multiplied by the rain strength, as in Level.
    pub fn sample(
        &self,
        mut clock: impl FnMut(&ResourceId) -> Option<i64>,
        rain: f32,
        thunder: f32,
    ) -> Option<f32> {
        if !rain.is_finite() || !thunder.is_finite() {
            return None;
        }
        let mut value = self.base;
        for track in &self.tracks {
            value = track
                .modifier
                .apply(value, track.sample(clock(&track.clock)?));
        }
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

/// Java Level.isDarkOutside uses the truncated sky-darkening value.
pub fn is_dark_outside(has_fixed_time: bool, sky_level: f32) -> Option<bool> {
    if has_fixed_time {
        return Some(false);
    }
    sky_level
        .is_finite()
        .then_some((15.0_f32 - sky_level) as i32 >= 4)
}
