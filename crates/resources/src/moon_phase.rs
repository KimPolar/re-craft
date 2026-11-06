//! The non-interpolated 26.3 moon phase environment attribute.
use crate::packs::{ResourceId, ResourceProvider};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::creature_timeline::Easing as TrackEasing;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    FullMoon,
    WaningGibbous,
    ThirdQuarter,
    WaningCrescent,
    NewMoon,
    WaxingCrescent,
    FirstQuarter,
    WaxingGibbous,
}

impl Phase {
    pub const ALL: [Self; 8] = [
        Self::FullMoon,
        Self::WaningGibbous,
        Self::ThirdQuarter,
        Self::WaningCrescent,
        Self::NewMoon,
        Self::WaxingCrescent,
        Self::FirstQuarter,
        Self::WaxingGibbous,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::FullMoon => "full_moon",
            Self::WaningGibbous => "waning_gibbous",
            Self::ThirdQuarter => "third_quarter",
            Self::WaningCrescent => "waning_crescent",
            Self::NewMoon => "new_moon",
            Self::WaxingCrescent => "waxing_crescent",
            Self::FirstQuarter => "first_quarter",
            Self::WaxingGibbous => "waxing_gibbous",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|phase| *phase == self).unwrap()
    }

    /// Java's DimensionType.MOON_BRIGHTNESS_PER_PHASE value.
    pub fn brightness(self) -> f32 {
        match self {
            Self::FullMoon => 1.0,
            Self::WaningGibbous | Self::WaxingGibbous => 0.75,
            Self::ThirdQuarter | Self::FirstQuarter => 0.5,
            Self::WaningCrescent | Self::WaxingCrescent => 0.25,
            Self::NewMoon => 0.0,
        }
    }

    pub fn parse(value: &Value) -> Result<Self> {
        let value = value.as_str().context("invalid moon phase")?;
        Self::ALL
            .into_iter()
            .find(|phase| phase.name() == value)
            .context("unknown moon phase")
    }
}

pub fn layer(value: &Value) -> Result<Phase> {
    if value.is_string() {
        return Phase::parse(value);
    }
    let modifier = ResourceId::parse(
        value
            .get("modifier")
            .and_then(Value::as_str)
            .context("missing moon phase modifier")?,
    )?;
    ensure!(
        modifier.namespace == "minecraft" && modifier.path == "override",
        "unsupported moon phase modifier"
    );
    Phase::parse(
        value
            .get("argument")
            .context("missing moon phase argument")?,
    )
}

pub struct Track {
    clock: ResourceId,
    period: Option<i32>,
    easing: TrackEasing,
    frames: Vec<(i32, Phase)>,
}

impl Track {
    pub fn parse(timeline: &Value, track: &Value) -> Result<Self> {
        let clock = ResourceId::parse(
            timeline
                .get("clock")
                .and_then(Value::as_str)
                .context("missing moon phase clock")?,
        )?;
        let period = timeline
            .get("period_ticks")
            .map(|value| {
                let period = value
                    .as_i64()
                    .and_then(|n| i32::try_from(n).ok())
                    .context("invalid period")?;
                ensure!(period > 0, "invalid period");
                Ok(period)
            })
            .transpose()?;
        if let Some(modifier) = track.get("modifier") {
            let id = ResourceId::parse(modifier.as_str().context("invalid modifier")?)?;
            ensure!(
                id.namespace == "minecraft" && id.path == "override",
                "unsupported moon phase modifier"
            );
        }
        let easing = TrackEasing::parse(track.get("ease"))?;
        let values = track
            .get("keyframes")
            .and_then(Value::as_array)
            .context("missing moon phase keyframes")?;
        ensure!(!values.is_empty(), "empty moon phase keyframes");
        let mut frames: Vec<(i32, Phase)> = Vec::with_capacity(values.len());
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
            frames.push((
                tick,
                Phase::parse(frame.get("value").context("missing keyframe value")?)?,
            ));
        }
        Ok(Self {
            clock,
            period,
            easing,
            frames,
        })
    }

    pub fn sample(&self, mut clock: impl FnMut(&ResourceId) -> Option<i64>) -> Option<Phase> {
        let ticks = clock(&self.clock)?;
        let ticks = self
            .period
            .map_or(ticks, |period| ticks.rem_euclid(i64::from(period)));
        let first = self.frames[0];
        let last = self.frames[self.frames.len() - 1];
        if self.frames.len() == 1 {
            return Some(first.1);
        }
        let segments = self
            .period
            .map(|p| ((last.0.wrapping_sub(p), last.1), first))
            .into_iter()
            .chain(self.frames.windows(2).map(|pair| (pair[0], pair[1])))
            .chain(
                self.period
                    .map(|p| (last, (first.0.wrapping_add(p), first.1))),
            );
        let ((start, a), (end, b)) = segments
            .clone()
            .find(|(_, end)| ticks < i64::from(end.0))
            .unwrap_or_else(|| segments.last().unwrap());
        if ticks <= i64::from(start) {
            return Some(a);
        }
        if ticks >= i64::from(end) {
            return Some(b);
        }
        let fraction = ticks.wrapping_sub(i64::from(start)) as f32 / end.wrapping_sub(start) as f32;
        Some(if self.easing.apply(fraction) >= 1.0 {
            b
        } else {
            a
        })
    }
}

pub struct Environment {
    pub base: Phase,
    pub biomes: BTreeMap<String, Option<Option<Phase>>>,
    pub has_biome_layer: bool,
    pub tracks: Vec<Track>,
}

impl Environment {
    /// Load the active dimension, biome and ordered timeline layers. A missing
    /// clock at sample time remains unknown rather than assuming overworld time.
    pub fn load(provider: &mut impl ResourceProvider, dimension_type: &str) -> Result<Self> {
        let id = ResourceId::parse(dimension_type)?;
        let path = format!("data/{}/dimension_type/{}.json", id.namespace, id.path);
        let bytes = provider
            .read_data(&path)?
            .with_context(|| format!("missing dimension type {path}"))?;
        let definition: Value = serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid dimension type {path}"))?;
        Self::from_definition(provider, &definition)
    }

    pub fn from_definition(
        provider: &mut impl ResourceProvider,
        definition: &Value,
    ) -> Result<Self> {
        ensure!(definition.is_object(), "dimension type must be an object");
        let base = moon_attribute(definition.get("attributes"))?
            .map(layer)
            .transpose()?
            .unwrap_or(Phase::FullMoon);
        let mut environment = Self {
            base,
            biomes: BTreeMap::new(),
            has_biome_layer: false,
            tracks: Vec::new(),
        };
        for biome in provider.list_data("worldgen/biome")? {
            let path = format!(
                "data/{}/worldgen/biome/{}.json",
                biome.namespace, biome.path
            );
            let bytes = provider
                .read_data(&path)?
                .with_context(|| format!("missing biome {path}"))?;
            let value: Value =
                serde_json::from_slice(&bytes).with_context(|| format!("invalid biome {path}"))?;
            ensure!(value.is_object(), "biome must be an object");
            let layer_value = moon_attribute(value.get("attributes"))?
                .map(layer)
                .transpose()?;
            environment.has_biome_layer |= layer_value.is_some();
            environment.biomes.insert(
                format!("{}:{}", biome.namespace, biome.path),
                Some(layer_value),
            );
        }
        for timeline in
            crate::fluid_environment::dimension_timelines_from_definition(provider, definition)?
        {
            let tracks = timeline
                .get("tracks")
                .and_then(Value::as_object)
                .context("invalid timeline tracks")?;
            let mut found = false;
            for (name, value) in tracks {
                let id = ResourceId::parse(name)?;
                if id.namespace == "minecraft" && id.path == "visual/moon_phase" {
                    ensure!(!found, "duplicate moon phase track");
                    found = true;
                    environment.tracks.push(Track::parse(&timeline, value)?);
                }
            }
        }
        Ok(environment)
    }

    pub fn sample(
        &self,
        biome: Option<&str>,
        mut clock: impl FnMut(&ResourceId) -> Option<i64>,
    ) -> Option<Phase> {
        let mut value = self.base;
        if self.has_biome_layer {
            value = self.biomes.get(biome?)?.as_ref()?.unwrap_or(value);
        }
        for track in &self.tracks {
            value = track.sample(&mut clock)?;
        }
        Some(value)
    }
}

fn moon_attribute(attributes: Option<&Value>) -> Result<Option<&Value>> {
    let Some(attributes) = attributes else {
        return Ok(None);
    };
    let mut found = None;
    for (name, value) in attributes
        .as_object()
        .context("environment attributes must be an object")?
    {
        let id = ResourceId::parse(name)?;
        if id.namespace == "minecraft" && id.path == "visual/moon_phase" {
            ensure!(found.is_none(), "duplicate moon phase attribute");
            found = Some(value);
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Data {
        timeline: bool,
    }
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            let value = match path {
                "data/example/dimension_type/world.json" => {
                    json!({"attributes":{"visual/moon_phase":"new_moon"}, "timelines": if self.timeline { json!(["example:moon"]) } else { json!([]) }})
                }
                "data/example/worldgen/biome/plains.json" => {
                    json!({"attributes":{"minecraft:visual/moon_phase":"third_quarter"}})
                }
                "data/example/worldgen/biome/desert.json" => json!({}),
                "data/example/timeline/moon.json" => {
                    json!({"clock":"example:clock", "period_ticks":20, "tracks":{"visual/moon_phase":{"keyframes":[{"ticks":0,"value":"full_moon"},{"ticks":10,"value":"new_moon"}]}}})
                }
                _ => return Ok(None),
            };
            Ok(Some(serde_json::to_vec(&value)?))
        }
        fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
            ensure!(folder == "worldgen/biome", "unexpected registry");
            ["example:plains", "example:desert"]
                .into_iter()
                .map(ResourceId::parse)
                .collect()
        }
    }

    #[test]
    fn active_environment_resolves_biomes_timelines_and_missing_clocks() {
        let environment =
            Environment::load(&mut Data { timeline: false }, "example:world").unwrap();
        assert_eq!(
            environment.sample(Some("example:plains"), |_| None),
            Some(Phase::ThirdQuarter)
        );
        assert_eq!(
            environment.sample(Some("example:desert"), |_| None),
            Some(Phase::NewMoon)
        );
        assert_eq!(environment.sample(Some("example:unknown"), |_| None), None);
        let environment = Environment::load(&mut Data { timeline: true }, "example:world").unwrap();
        assert_eq!(
            environment.sample(Some("example:plains"), |_| Some(0)),
            Some(Phase::FullMoon)
        );
        assert_eq!(
            environment.sample(Some("example:plains"), |_| Some(10)),
            Some(Phase::NewMoon)
        );
        assert_eq!(
            environment.sample(Some("example:plains"), |_| Some(-1)),
            Some(Phase::NewMoon)
        );
        assert_eq!(environment.sample(Some("example:plains"), |_| None), None);
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn installed_overworld_environment_uses_the_active_moon_timeline() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let mut resources = crate::packs::ResourceStack::open(&game).unwrap();
        let environment = Environment::load(&mut resources, "minecraft:overworld").unwrap();
        for (day, phase) in Phase::ALL.into_iter().enumerate() {
            assert_eq!(
                environment.sample(Some("minecraft:plains"), |_| Some(day as i64 * 24000)),
                Some(phase)
            );
        }
    }
}
