//! Time-dependent creature-generation attributes from active data packs.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use crate::{
    fluid_environment::dimension_timelines_from_definition,
    mob_spawns::{CreatureSpawnLayer, resolve_creature_spawns},
    packs::{ResourceId, ResourceProvider},
};

const SPAWNS: &str = "minecraft:gameplay/natural_mob_spawns";
const PROBABILITY: &str = "minecraft:gameplay/creature_world_gen_spawn_probability";

/// Dimension attributes and the ordered, relevant time-dependent layers.
/// Other environment attributes remain the responsibility of their consumers.
pub struct CreatureSpawnEnvironment {
    dimension_attributes: Value,
    tracks: Vec<Track>,
}

impl CreatureSpawnEnvironment {
    /// Decode against categories read from the verified client registration.
    pub fn load_with_registry(
        provider: &mut impl ResourceProvider,
        dimension_type: &str,
        registry: &crate::entity_category::EntityCategoryRegistry,
    ) -> Result<Self> {
        Self::load(provider, dimension_type, |id| registry.is_misc(id))
    }

    /// Load the active dimension and its referenced timelines. Entity types are
    /// checked against the same registry used to decode spawn settings.
    pub fn load(
        provider: &mut impl ResourceProvider,
        dimension_type: &str,
        entity_is_misc: impl FnMut(&ResourceId) -> Option<bool>,
    ) -> Result<Self> {
        let id = ResourceId::parse(dimension_type)?;
        let path = format!("data/{}/dimension_type/{}.json", id.namespace, id.path);
        let bytes = provider
            .read_data(&path)?
            .with_context(|| format!("missing dimension type {path}"))?;
        let definition: Value = serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid dimension type {path}"))?;
        Self::from_definition(provider, &definition, entity_is_misc)
    }

    pub fn from_definition_with_registry(
        provider: &mut impl ResourceProvider,
        definition: &Value,
        registry: &crate::entity_category::EntityCategoryRegistry,
    ) -> Result<Self> {
        Self::from_definition(provider, definition, |id| registry.is_misc(id))
    }

    pub fn from_definition(
        provider: &mut impl ResourceProvider,
        definition: &Value,
        mut entity_is_misc: impl FnMut(&ResourceId) -> Option<bool>,
    ) -> Result<Self> {
        let object = definition
            .as_object()
            .context("dimension type must be an object")?;
        let dimension_attributes = object
            .get("attributes")
            .cloned()
            .unwrap_or_else(|| json!({}));
        ensure!(
            dimension_attributes.is_object(),
            "dimension attributes must be an object"
        );
        resolve_creature_spawns(
            &dimension_attributes,
            &json!({}),
            &json!({}),
            &[],
            &mut entity_is_misc,
        )?;
        let mut tracks = Vec::new();
        for timeline in dimension_timelines_from_definition(provider, definition)? {
            let values = timeline
                .get("tracks")
                .and_then(Value::as_object)
                .context("invalid timeline tracks")?;
            let mut seen = std::collections::BTreeSet::new();
            for (name, value) in values {
                let id = ResourceId::parse(name)?;
                ensure!(seen.insert(id.clone()), "duplicate timeline attribute");
                if id.namespace != "minecraft" {
                    continue;
                }
                let kind = match id.path.as_str() {
                    "gameplay/natural_mob_spawns" => Kind::Spawns,
                    "gameplay/creature_world_gen_spawn_probability" => Kind::Probability,
                    _ => continue,
                };
                tracks.push(Track::parse(&timeline, value, kind, &mut entity_is_misc)?);
            }
        }
        Ok(Self {
            dimension_attributes,
            tracks,
        })
    }

    pub fn dimension_attributes(&self) -> &Value {
        &self.dimension_attributes
    }

    /// Evaluate each relevant track in dimension order. Missing clock values
    /// are errors, including when another track happens to override the result.
    pub fn sample_layers(
        &self,
        mut clock_ticks: impl FnMut(&ResourceId) -> Option<i64>,
    ) -> Result<Vec<Value>> {
        let mut sampled_clocks = std::collections::BTreeMap::new();
        self.tracks
            .iter()
            .map(|track| {
                let ticks = match sampled_clocks.get(&track.clock) {
                    Some(&ticks) => ticks,
                    None => {
                        let ticks = clock_ticks(&track.clock).with_context(|| {
                            format!(
                                "missing clock {}:{}",
                                track.clock.namespace, track.clock.path
                            )
                        })?;
                        sampled_clocks.insert(track.clock.clone(), ticks);
                        ticks
                    }
                };
                track.sample(ticks)
            })
            .collect()
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Spawns,
    Probability,
}

struct Track {
    clock: ResourceId,
    period: Option<i32>,
    modifier: String,
    easing: Easing,
    kind: Kind,
    frames: Vec<(i32, Value)>,
}

impl Track {
    fn parse(
        timeline: &Value,
        value: &Value,
        kind: Kind,
        entity_is_misc: &mut impl FnMut(&ResourceId) -> Option<bool>,
    ) -> Result<Self> {
        let clock = ResourceId::parse(
            timeline
                .get("clock")
                .and_then(Value::as_str)
                .context("missing timeline clock")?,
        )?;
        let period = timeline
            .get("period_ticks")
            .map(|value| {
                let period = value
                    .as_i64()
                    .and_then(|v| i32::try_from(v).ok())
                    .context("invalid timeline period")?;
                ensure!(period > 0, "timeline period must be positive");
                Ok(period)
            })
            .transpose()?;
        let modifier = value
            .get("modifier")
            .map(|v| v.as_str().context("invalid track modifier"))
            .transpose()?
            .unwrap_or("override")
            .to_owned();
        let easing = Easing::parse(value.get("ease"))?;
        let values = value
            .get("keyframes")
            .and_then(Value::as_array)
            .context("missing keyframes")?;
        ensure!(!values.is_empty(), "empty keyframes");
        let mut frames: Vec<(i32, Value)> = Vec::with_capacity(values.len());
        for frame in values {
            let tick = frame
                .get("ticks")
                .and_then(Value::as_i64)
                .and_then(|v| i32::try_from(v).ok())
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
                    "three identical keyframe ticks"
                );
            }
            let argument = frame
                .get("value")
                .context("missing keyframe value")?
                .clone();
            let attr = match kind {
                Kind::Spawns => SPAWNS,
                Kind::Probability => PROBABILITY,
            };
            let layer = json!({(attr):{"modifier":modifier,"argument":argument}});
            match kind {
                Kind::Spawns => {
                    CreatureSpawnLayer::from_attributes(&layer, &mut *entity_is_misc)?
                        .context("missing spawn track")?;
                }
                Kind::Probability => {
                    resolve_creature_spawns(
                        &layer,
                        &json!({}),
                        &json!({}),
                        &[],
                        &mut *entity_is_misc,
                    )?;
                }
            }
            frames.push((tick, argument));
        }
        Ok(Self {
            clock,
            period,
            modifier,
            easing,
            kind,
            frames,
        })
    }

    fn sample(&self, ticks: i64) -> Result<Value> {
        let ticks = self
            .period
            .map_or(ticks, |p| ticks.rem_euclid(i64::from(p)));
        let first = &self.frames[0];
        let last = self.frames.last().expect("validated keyframes");
        let argument = if self.frames.len() == 1 {
            first.1.clone()
        } else {
            let before = self
                .period
                .map(|p| ((last.0.wrapping_sub(p), &last.1), (first.0, &first.1)));
            let after = self
                .period
                .map(|p| ((last.0, &last.1), (first.0.wrapping_add(p), &first.1)));
            let segments = before
                .into_iter()
                .chain(
                    self.frames
                        .windows(2)
                        .map(|w| ((w[0].0, &w[0].1), (w[1].0, &w[1].1))),
                )
                .chain(after);
            let ((start, a), (end, b)) = segments
                .clone()
                .find(|(_, end)| ticks < i64::from(end.0))
                .unwrap_or_else(|| segments.last().expect("multiple keyframes"));
            if ticks <= i64::from(start) {
                a.clone()
            } else if ticks >= i64::from(end) {
                b.clone()
            } else {
                let alpha = self.easing.apply(
                    ticks.wrapping_sub(i64::from(start)) as f32 / end.wrapping_sub(start) as f32,
                );
                match self.kind {
                    Kind::Spawns => {
                        if alpha >= 1.0 {
                            b.clone()
                        } else {
                            a.clone()
                        }
                    }
                    Kind::Probability => {
                        let a = a.as_f64().context("invalid probability frame")? as f32;
                        let b = b.as_f64().context("invalid probability frame")? as f32;
                        let result = a + alpha * (b - a);
                        ensure!(
                            result.is_finite(),
                            "non-finite interpolated spawn probability"
                        );
                        json!(result)
                    }
                }
            }
        };
        let attr = match self.kind {
            Kind::Spawns => SPAWNS,
            Kind::Probability => PROBABILITY,
        };
        Ok(json!({(attr):{"modifier":self.modifier,"argument":argument}}))
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Easing {
    Linear,
    Constant,
    Cubic { x: [f32; 3], y: [f32; 3] },
}
impl Easing {
    pub(crate) fn parse(value: Option<&Value>) -> Result<Self> {
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
    pub(crate) fn apply(self, input: f32) -> f32 {
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
