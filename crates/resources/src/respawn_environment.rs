//! The respawn-related part of a 26.3 environment-attribute layer.
//!
//! Dimension types, biomes and timelines can all affect these attributes.
//! A layer is deliberately not a complete environment: its caller must
//! supply every applicable layer, including timelines, in Java's order.

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::packs::{ResourceId, ResourceProvider};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BedCondition {
    Always,
    WhenDark,
    Never,
}

impl BedCondition {
    fn parse(value: Option<&Value>, name: &str) -> Result<Self> {
        match value.and_then(Value::as_str) {
            Some("always") => Ok(Self::Always),
            Some("when_dark") => Ok(Self::WhenDark),
            Some("never") => Ok(Self::Never),
            _ => bail!("invalid or missing bed rule {name}"),
        }
    }

    pub fn test(self, dark_outside: bool) -> bool {
        match self {
            Self::Always => true,
            Self::WhenDark => dark_outside,
            Self::Never => false,
        }
    }
}

/// Only fields used to decide sleeping and spawn eligibility. The optional
/// error-message component is intentionally not decoded here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BedRule {
    pub can_sleep: BedCondition,
    pub can_set_spawn: BedCondition,
    pub destroy_on_use: bool,
    pub destroy_on_leave: bool,
}

impl BedRule {
    fn parse(value: &Value) -> Result<Self> {
        let fields = value.as_object().context("bed rule must be an object")?;
        let boolean = |key| -> Result<bool> {
            match fields.get(key) {
                Some(value) => value.as_bool().with_context(|| format!("invalid {key}")),
                None => Ok(false),
            }
        };
        Ok(Self {
            can_sleep: BedCondition::parse(fields.get("can_sleep"), "can_sleep")?,
            can_set_spawn: BedCondition::parse(fields.get("can_set_spawn"), "can_set_spawn")?,
            destroy_on_use: boolean("destroy_on_use")?,
            destroy_on_leave: boolean("destroy_on_leave")?,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BooleanModifier {
    Override,
    And,
    Nand,
    Or,
    Nor,
    Xor,
    Xnor,
}

impl BooleanModifier {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        // Attribute modifier IDs use the default namespace when omitted.
        let id = ResourceId::parse(value)?;
        ensure!(
            id.namespace == "minecraft",
            "unknown boolean modifier {value}"
        );
        match id.path.as_str() {
            "override" => Ok(Self::Override),
            "and" => Ok(Self::And),
            "nand" => Ok(Self::Nand),
            "or" => Ok(Self::Or),
            "nor" => Ok(Self::Nor),
            "xor" => Ok(Self::Xor),
            "xnor" => Ok(Self::Xnor),
            _ => bail!("unknown boolean modifier {value}"),
        }
    }

    pub(crate) fn apply(self, base: bool, argument: bool) -> bool {
        match self {
            Self::Override => argument,
            Self::And => base && argument,
            Self::Nand => !(base && argument),
            Self::Or => base || argument,
            Self::Nor => !(base || argument),
            Self::Xor => base ^ argument,
            Self::Xnor => base == argument,
        }
    }
}

/// Relevant entries from one dimension, biome, or already-sampled timeline.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RespawnEnvironmentLayer {
    bed: Option<BedRule>,
    straw_bed: Option<BedRule>,
    anchor: Option<(BooleanModifier, bool)>,
}

impl RespawnEnvironmentLayer {
    /// Parse the `attributes` object. Other registered attributes are left to
    /// their own consumers; this does not validate an entire game registry.
    pub fn from_attributes(attributes: &Value) -> Result<Self> {
        let attributes = attributes
            .as_object()
            .context("environment attributes must be an object")?;
        let mut result = Self::default();
        for (name, value) in attributes {
            let id = ResourceId::parse(name)?;
            if id.namespace != "minecraft" {
                continue;
            }
            match id.path.as_str() {
                "gameplay/bed_rule" | "gameplay/straw_bed_rule" => {
                    let target = if id.path == "gameplay/bed_rule" {
                        &mut result.bed
                    } else {
                        &mut result.straw_bed
                    };
                    ensure!(target.is_none(), "duplicate environment attribute {name}");
                    // Codec.either tries the direct bed rule before the
                    // modifier form; unrelated object fields do not override it.
                    let rule = match BedRule::parse(value) {
                        Ok(rule) => rule,
                        Err(_) => {
                            let modifier = value.get("modifier").context("invalid bed rule")?;
                            let id = ResourceId::parse(
                                modifier.as_str().context("invalid bed rule modifier")?,
                            )?;
                            ensure!(
                                id.namespace == "minecraft" && id.path == "override",
                                "unsupported bed rule modifier"
                            );
                            BedRule::parse(
                                value.get("argument").context("missing bed rule argument")?,
                            )?
                        }
                    };
                    *target = Some(rule);
                }
                "gameplay/respawn_anchor_works" => {
                    ensure!(
                        result.anchor.is_none(),
                        "duplicate environment attribute {name}"
                    );
                    result.anchor = Some(if let Some(value) = value.as_bool() {
                        (BooleanModifier::Override, value)
                    } else {
                        let modifier = value
                            .get("modifier")
                            .and_then(Value::as_str)
                            .context("missing boolean modifier")?;
                        let argument = value
                            .get("argument")
                            .and_then(Value::as_bool)
                            .context("invalid boolean modifier argument")?;
                        (BooleanModifier::parse(modifier)?, argument)
                    });
                }
                _ => {}
            }
        }
        Ok(result)
    }

    /// Read the highest-priority active data-pack definition. Missing
    /// registries are an error rather than silently using a vanilla layer.
    pub fn load_dimension_type(provider: &mut impl ResourceProvider, id: &str) -> Result<Self> {
        Self::load_definition(provider, "dimension_type", id)
    }

    pub fn load_biome(provider: &mut impl ResourceProvider, id: &str) -> Result<Self> {
        Self::load_definition(provider, "worldgen/biome", id)
    }

    fn load_definition(
        provider: &mut impl ResourceProvider,
        folder: &str,
        id: &str,
    ) -> Result<Self> {
        let id = ResourceId::parse(id)?;
        let path = format!("data/{}/{folder}/{}.json", id.namespace, id.path);
        let bytes = provider
            .read_data(&path)?
            .with_context(|| format!("missing environment definition {path}"))?;
        let value: Value = serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid environment definition {path}"))?;
        ensure!(
            value.is_object(),
            "environment definition {path} is not an object"
        );
        match value.get("attributes") {
            Some(attributes) => Self::from_attributes(attributes)
                .with_context(|| format!("invalid attributes in {path}")),
            None => Ok(Self::default()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RespawnEnvironment {
    pub bed: BedRule,
    pub straw_bed: BedRule,
    pub anchor_works: bool,
}

impl Default for RespawnEnvironment {
    fn default() -> Self {
        Self {
            bed: BedRule {
                can_sleep: BedCondition::WhenDark,
                can_set_spawn: BedCondition::Always,
                destroy_on_use: false,
                destroy_on_leave: false,
            },
            straw_bed: BedRule {
                can_sleep: BedCondition::WhenDark,
                can_set_spawn: BedCondition::Never,
                destroy_on_use: false,
                destroy_on_leave: true,
            },
            anchor_works: false,
        }
    }
}

impl RespawnEnvironment {
    pub fn apply(&mut self, layer: RespawnEnvironmentLayer) {
        if let Some(bed) = layer.bed {
            self.bed = bed;
        }
        if let Some(straw_bed) = layer.straw_bed {
            self.straw_bed = straw_bed;
        }
        if let Some((modifier, argument)) = layer.anchor {
            self.anchor_works = modifier.apply(self.anchor_works, argument);
        }
    }
}

/// Dimension, biome and ordered active timeline layers for respawn attributes.
#[derive(Clone, Debug)]
pub struct RespawnEnvironmentRegistry {
    dimension: RespawnEnvironmentLayer,
    biomes: BTreeMap<String, RespawnEnvironmentLayer>,
    tracks: Vec<RespawnTrack>,
    fixed_time: bool,
    sky_level: Option<crate::sky_light::SkyLevel>,
}

#[derive(Clone, Debug)]
struct RespawnTrack {
    clock: ResourceId,
    period: Option<i32>,
    easing: crate::creature_timeline::Easing,
    frames: Vec<(i32, RespawnEnvironmentLayer)>,
}

impl RespawnTrack {
    fn parse(timeline: &Value, attribute: &str, track: &Value) -> Result<Self> {
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
        let easing = crate::creature_timeline::Easing::parse(track.get("ease"))?;
        let modifier = track
            .get("modifier")
            .map(|v| v.as_str().context("invalid track modifier"))
            .transpose()?
            .unwrap_or("override");
        let values = track
            .get("keyframes")
            .and_then(Value::as_array)
            .context("missing keyframes")?;
        ensure!(!values.is_empty(), "empty keyframes");
        let mut frames: Vec<(i32, RespawnEnvironmentLayer)> = Vec::with_capacity(values.len());
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
            let argument = frame.get("value").context("missing keyframe value")?;
            let attributes =
                serde_json::json!({(attribute): {"modifier":modifier,"argument":argument}});
            frames.push((tick, RespawnEnvironmentLayer::from_attributes(&attributes)?));
        }
        Ok(Self {
            clock,
            period,
            easing,
            frames,
        })
    }

    fn sample(&self, ticks: i64) -> RespawnEnvironmentLayer {
        let ticks = self
            .period
            .map_or(ticks, |period| ticks.rem_euclid(i64::from(period)));
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
        let alpha = self
            .easing
            .apply(ticks.wrapping_sub(i64::from(start)) as f32 / end.wrapping_sub(start) as f32);
        if alpha >= 1.0 { b } else { a }
    }
}

impl RespawnEnvironmentRegistry {
    pub fn has_timeline_rules(&self) -> bool {
        !self.tracks.is_empty()
    }

    pub fn load(provider: &mut impl ResourceProvider, dimension_type: &str) -> Result<Self> {
        let id = ResourceId::parse(dimension_type)?;
        let path = format!("data/{}/dimension_type/{}.json", id.namespace, id.path);
        let bytes = provider
            .read_data(&path)?
            .with_context(|| format!("missing environment definition {path}"))?;
        let definition: Value = serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid environment definition {path}"))?;
        Self::from_definition(provider, &definition)
            .with_context(|| format!("invalid environment in {path}"))
    }

    pub fn from_definition(
        provider: &mut impl ResourceProvider,
        definition: &Value,
    ) -> Result<Self> {
        ensure!(definition.is_object(), "dimension type must be an object");
        let dimension = definition
            .get("attributes")
            .map(RespawnEnvironmentLayer::from_attributes)
            .transpose()?
            .unwrap_or_default();
        Self::with_dimension(provider, dimension, definition)
    }

    fn with_dimension(
        provider: &mut impl ResourceProvider,
        dimension: RespawnEnvironmentLayer,
        definition: &Value,
    ) -> Result<Self> {
        let fixed_time = match definition.get("has_fixed_time") {
            None => false,
            Some(value) => value.as_bool().context("invalid fixed-time flag")?,
        };
        // An unsupported sky-light layer must not prevent rules which do not
        // depend on darkness from being used.
        let sky_level = if fixed_time {
            None
        } else {
            crate::sky_light::SkyLevel::from_definition(provider, definition).ok()
        };
        let mut collected_tracks = Vec::new();
        for timeline in
            crate::fluid_environment::dimension_timelines_from_definition(provider, definition)?
        {
            let tracks = timeline
                .get("tracks")
                .and_then(Value::as_object)
                .context("invalid active timeline tracks")?;
            let mut seen = std::collections::BTreeSet::new();
            for (key, value) in tracks {
                let attr = ResourceId::parse(key)?;
                ensure!(
                    seen.insert(attr.clone()),
                    "duplicate timeline attribute {key}"
                );
                if attr.namespace == "minecraft"
                    && matches!(
                        attr.path.as_str(),
                        "gameplay/bed_rule"
                            | "gameplay/straw_bed_rule"
                            | "gameplay/respawn_anchor_works"
                    )
                {
                    // Keep the actual attribute ID for the layer parser.
                    let parsed = RespawnTrack::parse(&timeline, key, value)?;
                    collected_tracks.push(parsed);
                }
            }
        }
        let mut biomes = BTreeMap::new();
        for id in provider.list_data("worldgen/biome")? {
            let name = format!("{}:{}", id.namespace, id.path);
            let layer = RespawnEnvironmentLayer::load_biome(provider, &name)?;
            ensure!(
                biomes.insert(name, layer).is_none(),
                "duplicate biome identifier"
            );
        }
        ensure!(!biomes.is_empty(), "biome registry is empty");
        Ok(Self {
            dimension,
            biomes,
            tracks: collected_tracks,
            fixed_time,
            sky_level,
        })
    }

    /// Dimension-wide darkness for `when_dark`; biome values do not affect it.
    pub fn dark_outside(
        &self,
        clock: impl FnMut(&ResourceId) -> Option<i64>,
        weather: Option<(f32, f32)>,
    ) -> Result<bool> {
        if self.fixed_time {
            return Ok(false);
        }
        let level = self
            .sky_level
            .as_ref()
            .context("sky light environment is unavailable")?;
        let (rain, thunder) = weather.context("weather is unavailable")?;
        let value = level
            .sample(clock, rain, thunder)
            .context("sky light clock or weather is unavailable")?;
        crate::sky_light::is_dark_outside(false, value).context("invalid sky light level")
    }

    pub fn for_biome(&self, biome: &str) -> Result<RespawnEnvironment> {
        ensure!(self.tracks.is_empty(), "respawn rules require world clocks");
        self.for_biome_at(biome, |_| None)
    }

    pub fn for_biome_at(
        &self,
        biome: &str,
        mut clock_ticks: impl FnMut(&ResourceId) -> Option<i64>,
    ) -> Result<RespawnEnvironment> {
        let id = ResourceId::parse(biome)?;
        let name = format!("{}:{}", id.namespace, id.path);
        let layer = self
            .biomes
            .get(&name)
            .with_context(|| format!("unknown biome {name}"))?;
        let mut result = RespawnEnvironment::default();
        result.apply(self.dimension);
        result.apply(*layer);
        let mut clocks = BTreeMap::new();
        for track in &self.tracks {
            let ticks = if let Some(&ticks) = clocks.get(&track.clock) {
                ticks
            } else {
                let ticks = clock_ticks(&track.clock).with_context(|| {
                    format!(
                        "missing clock {}:{}",
                        track.clock.namespace, track.clock.path
                    )
                })?;
                clocks.insert(track.clock.clone(), ticks);
                ticks
            };
            result.apply(track.sample(ticks));
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Memory(BTreeMap<String, Value>);

    impl ResourceProvider for Memory {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }

        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            self.0
                .get(path)
                .map(serde_json::to_vec)
                .transpose()
                .map_err(Into::into)
        }

        fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
            if folder == "worldgen/biome" {
                Ok(vec![ResourceId::parse("example:plain")?])
            } else {
                // A provider may contain timelines that this dimension never uses.
                Ok(vec![ResourceId::parse("example:other")?])
            }
        }
    }

    #[test]
    fn checks_respawn_rules_only_in_active_dimension_timelines() {
        let definition = json!({
            "attributes": {"minecraft:gameplay/respawn_anchor_works": true},
            "timelines": "#example:active"
        });
        let mut source = Memory(BTreeMap::from([
            (
                "data/example/dimension_type/test.json".into(),
                definition.clone(),
            ),
            ("data/example/worldgen/biome/plain.json".into(), json!({})),
            (
                "data/example/tags/timeline/active.json".into(),
                json!({"values":["#example:nested"]}),
            ),
            (
                "data/example/tags/timeline/nested.json".into(),
                json!({"values":["example:harmless"]}),
            ),
            (
                "data/example/timeline/harmless.json".into(),
                json!({"tracks":{}}),
            ),
            (
                "data/example/timeline/other.json".into(),
                json!({"tracks":{"gameplay/bed_rule":{}}}),
            ),
        ]));
        for registry in [
            RespawnEnvironmentRegistry::load(&mut source, "example:test").unwrap(),
            RespawnEnvironmentRegistry::from_definition(&mut source, &definition).unwrap(),
        ] {
            assert!(registry.for_biome("example:plain").unwrap().anchor_works);
        }
        source.0.insert(
            "data/example/tags/timeline/nested.json".into(),
            json!({"values":["example:other"]}),
        );
        assert!(RespawnEnvironmentRegistry::load(&mut source, "example:test").is_err());
        assert!(RespawnEnvironmentRegistry::from_definition(&mut source, &definition).is_err());
        source.0.insert(
            "data/example/tags/timeline/nested.json".into(),
            json!({"values":["example:missing"]}),
        );
        assert!(RespawnEnvironmentRegistry::from_definition(&mut source, &definition).is_err());
        let direct = json!({"timelines":["example:other"]});
        assert!(RespawnEnvironmentRegistry::from_definition(&mut source, &direct).is_err());
        let no_timelines = json!({"attributes":{"gameplay/respawn_anchor_works":true}});
        assert!(
            RespawnEnvironmentRegistry::from_definition(&mut source, &no_timelines)
                .unwrap()
                .for_biome("example:plain")
                .unwrap()
                .anchor_works
        );
    }

    #[test]
    fn respawn_timelines_follow_biome_and_require_clock_values() {
        let mut source = Memory(BTreeMap::from([
            (
                "data/example/worldgen/biome/plain.json".into(),
                json!({"attributes":{
                    "gameplay/respawn_anchor_works":true,
                    "gameplay/bed_rule":{"can_sleep":"always","can_set_spawn":"never"}
                }}),
            ),
            (
                "data/example/timeline/test.json".into(),
                json!({"clock":"example:clock","period_ticks":20,"tracks":{
                    "gameplay/respawn_anchor_works":{"modifier":"xor","keyframes":[{"ticks":0,"value":false},{"ticks":10,"value":true}]},
                    "gameplay/bed_rule":{"keyframes":[
                        {"ticks":0,"value":{"can_sleep":"never","can_set_spawn":"always"}},
                        {"ticks":10,"value":{"can_sleep":"always","can_set_spawn":"never","destroy_on_use":true}}
                    ]}
                }}),
            ),
        ]));
        let registry = RespawnEnvironmentRegistry::from_definition(
            &mut source,
            &json!({"timelines":["example:test"]}),
        )
        .unwrap();
        assert!(registry.for_biome("example:plain").is_err());
        assert!(registry.for_biome_at("example:plain", |_| None).is_err());
        for (tick, anchor, spawn, destroy) in [
            (0, true, BedCondition::Always, false),
            (9, true, BedCondition::Always, false),
            (10, false, BedCondition::Never, true),
            (19, false, BedCondition::Never, true),
            (20, true, BedCondition::Always, false),
            (-1, false, BedCondition::Never, true),
        ] {
            let mut calls = 0;
            let value = registry
                .for_biome_at("example:plain", |id| {
                    assert_eq!(id, &ResourceId::parse("example:clock").unwrap());
                    calls += 1;
                    Some(tick)
                })
                .unwrap();
            assert_eq!(
                (
                    value.anchor_works,
                    value.bed.can_set_spawn,
                    value.bed.destroy_on_use
                ),
                (anchor, spawn, destroy)
            );
            assert_eq!(calls, 1);
        }
    }

    #[test]
    fn darkness_uses_dimension_sky_weather_and_fixed_time() {
        let mut source = Memory(BTreeMap::from([
            ("data/example/worldgen/biome/plain.json".into(), json!({})),
            (
                "data/example/timeline/test.json".into(),
                json!({"clock":"example:clock","period_ticks":20,"tracks":{
                    "gameplay/sky_light_level":{"keyframes":[{"ticks":0,"value":15},{"ticks":10,"value":0}]}
                }}),
            ),
        ]));
        let definition = json!({"timelines":["example:test"]});
        let registry =
            RespawnEnvironmentRegistry::from_definition(&mut source, &definition).unwrap();
        assert!(registry.dark_outside(|_| None, Some((0.0, 0.0))).is_err());
        assert!(registry.dark_outside(|_| Some(0), None).is_err());
        for (tick, dark) in [(0, false), (2, false), (3, true), (10, true), (20, false)] {
            assert_eq!(
                registry
                    .dark_outside(|_| Some(tick), Some((0.0, 0.0)))
                    .unwrap(),
                dark
            );
        }
        assert!(
            registry
                .dark_outside(|_| Some(0), Some((1.0, 1.0)))
                .unwrap()
        );
        let fixed = RespawnEnvironmentRegistry::from_definition(
            &mut source,
            &json!({"has_fixed_time":true,"timelines":["example:test"]}),
        )
        .unwrap();
        assert!(!fixed.dark_outside(|_| None, None).unwrap());
    }

    #[test]
    fn applies_layers_and_rejects_unknown_relevant_values() {
        let mut environment = RespawnEnvironment::default();
        environment.apply(
            RespawnEnvironmentLayer::from_attributes(&json!({
                "minecraft:gameplay/respawn_anchor_works": true,
                "gameplay/bed_rule": {"can_sleep":"never","can_set_spawn":"when_dark"}
            }))
            .unwrap(),
        );
        environment.apply(
            RespawnEnvironmentLayer::from_attributes(&json!({
                "gameplay/respawn_anchor_works": {"modifier":"xor","argument":true}
            }))
            .unwrap(),
        );
        assert!(!environment.anchor_works);
        assert!(!environment.bed.can_set_spawn.test(false));
        assert!(environment.bed.can_set_spawn.test(true));
        assert!(
            RespawnEnvironmentLayer::from_attributes(&json!({
                "gameplay/bed_rule": {"can_sleep":"never","can_set_spawn":"maybe"}
            }))
            .is_err()
        );
        assert!(
            RespawnEnvironmentLayer::from_attributes(&json!({
                "gameplay/respawn_anchor_works": {"modifier":"example:or","argument":true}
            }))
            .is_err()
        );
        assert!(
            RespawnEnvironmentLayer::from_attributes(&json!({
                "gameplay/respawn_anchor_works": true,
                "minecraft:gameplay/respawn_anchor_works": false
            }))
            .is_err()
        );
    }
}
