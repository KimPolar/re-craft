//! Dimension-wide fluid attributes used by the 26.3 server.

use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::collections::BTreeSet;

use crate::{
    packs::{ResourceId, ResourceProvider},
    recipes::{TagEntry, parse_tag},
    respawn_environment::BooleanModifier,
};

/// Apply one constant layer to the Java default (`false`).
pub fn fast_lava_from_attributes(attributes: &Value) -> Result<bool> {
    boolean_from_attributes(attributes, "gameplay/fast_lava")
}

fn boolean_from_attributes(attributes: &Value, attribute: &str) -> Result<bool> {
    let attributes = attributes
        .as_object()
        .context("environment attributes must be an object")?;
    let mut fast = None;
    for (name, value) in attributes {
        let id = ResourceId::parse(name)?;
        if id.namespace != "minecraft" || id.path != attribute {
            continue;
        }
        ensure!(
            fast.is_none(),
            "duplicate environment attribute {attribute}"
        );
        let (modifier, argument) = if let Some(value) = value.as_bool() {
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
        };
        fast = Some(modifier.apply(false, argument));
    }
    Ok(fast.unwrap_or(false))
}

#[derive(Clone, Debug)]
pub struct FastLavaEnvironment {
    base: bool,
    tracks: Vec<FastLavaTrack>,
}

/// The same boolean timeline sampler is shared by both fluid attributes.
pub type WaterEvaporationEnvironment = FastLavaEnvironment;

#[derive(Clone, Debug)]
struct FastLavaTrack {
    clock: ResourceId,
    period: Option<i32>,
    modifier: BooleanModifier,
    constant_easing: bool,
    keyframes: Vec<(i32, bool)>,
}

impl FastLavaEnvironment {
    /// Sample all layers in dimension order using the supplied world clocks.
    pub fn sample(&self, mut clock_ticks: impl FnMut(&ResourceId) -> Option<i64>) -> Option<bool> {
        let mut value = self.base;
        for track in &self.tracks {
            value = track
                .modifier
                .apply(value, track.sample(clock_ticks(&track.clock)?));
        }
        Some(value)
    }
}

impl FastLavaTrack {
    fn sample(&self, ticks: i64) -> bool {
        let ticks = self
            .period
            .map_or(ticks, |p| ticks.rem_euclid(i64::from(p)));
        let first = self.keyframes[0];
        let last = self.keyframes[self.keyframes.len() - 1];
        if self.keyframes.len() == 1 {
            return first.1;
        }
        let wrap_before = self
            .period
            .map(|period| ((last.0.wrapping_sub(period), last.1), first));
        let wrap_after = self
            .period
            .map(|period| (last, (first.0.wrapping_add(period), first.1)));
        let segments = wrap_before
            .into_iter()
            .chain(self.keyframes.windows(2).map(|w| (w[0], w[1])))
            .chain(wrap_after);
        let segment = segments
            .clone()
            .find(|(_, end)| ticks < i64::from(end.0))
            .unwrap_or_else(|| segments.last().unwrap());
        let ((from, a), (to, b)) = segment;
        if ticks <= i64::from(from) {
            return a;
        }
        if ticks >= i64::from(to) {
            return b;
        }
        if self.constant_easing {
            return a;
        }
        let alpha = ticks.wrapping_sub(i64::from(from)) as f32 / to.wrapping_sub(from) as f32;
        if alpha >= 1.0 { b } else { a }
    }
}

/// Read the active dimension layer and supported time-based fast-lava tracks.
pub fn load_fast_lava_environment(
    provider: &mut impl ResourceProvider,
    dimension_type: &str,
) -> Result<FastLavaEnvironment> {
    load_boolean_environment(provider, dimension_type, "gameplay/fast_lava")
}

/// Water evaporation can also vary by biome. Reject such packs until biome
/// boundary sampling can reproduce the server's positional lookup.
pub fn load_water_evaporation_environment(
    provider: &mut impl ResourceProvider,
    dimension_type: &str,
) -> Result<WaterEvaporationEnvironment> {
    let biomes = provider.list_data("worldgen/biome")?;
    ensure!(!biomes.is_empty(), "biome registry is empty");
    for id in biomes {
        let path = format!("data/{}/worldgen/biome/{}.json", id.namespace, id.path);
        let bytes = provider
            .read_data(&path)?
            .with_context(|| format!("missing biome {path}"))?;
        let biome: Value =
            serde_json::from_slice(&bytes).with_context(|| format!("invalid biome {path}"))?;
        if let Some(attributes) = biome.get("attributes") {
            for name in attributes
                .as_object()
                .context("invalid biome attributes")?
                .keys()
            {
                let name = ResourceId::parse(name)?;
                ensure!(
                    name.namespace != "minecraft" || name.path != "gameplay/water_evaporates",
                    "water evaporation varies by biome"
                );
            }
        }
    }
    load_boolean_environment(provider, dimension_type, "gameplay/water_evaporates")
}

fn load_boolean_environment(
    provider: &mut impl ResourceProvider,
    dimension_type: &str,
    attribute: &str,
) -> Result<FastLavaEnvironment> {
    let id = ResourceId::parse(dimension_type)?;
    let path = format!("data/{}/dimension_type/{}.json", id.namespace, id.path);
    let bytes = provider
        .read_data(&path)?
        .with_context(|| format!("missing dimension type {path}"))?;
    let definition: Value =
        serde_json::from_slice(&bytes).with_context(|| format!("invalid dimension type {path}"))?;
    ensure!(
        definition.is_object(),
        "dimension type {path} is not an object"
    );
    let base = match definition.get("attributes") {
        Some(attributes) => boolean_from_attributes(attributes, attribute)
            .with_context(|| format!("invalid attributes in {path}"))?,
        None => false,
    };
    let mut result = FastLavaEnvironment {
        base,
        tracks: Vec::new(),
    };
    for timeline in dimension_timelines(provider, &definition)? {
        let tracks = timeline
            .get("tracks")
            .and_then(Value::as_object)
            .context("invalid timeline tracks")?;
        let mut found = false;
        for (name, track) in tracks {
            let attr = ResourceId::parse(name)?;
            if attr.namespace == "minecraft" && attr.path == attribute {
                ensure!(!found, "duplicate environment attribute {attribute}");
                found = true;
                result.tracks.push(parse_fast_lava_track(&timeline, track)?);
            }
        }
    }
    Ok(result)
}

/// Load the dimension's ordered active timeline definitions from data packs.
pub fn load_dimension_timelines(
    provider: &mut impl ResourceProvider,
    dimension_type: &str,
) -> Result<Vec<Value>> {
    let id = ResourceId::parse(dimension_type)?;
    let path = format!("data/{}/dimension_type/{}.json", id.namespace, id.path);
    let bytes = provider
        .read_data(&path)?
        .with_context(|| format!("missing dimension type {path}"))?;
    let definition: Value =
        serde_json::from_slice(&bytes).with_context(|| format!("invalid dimension type {path}"))?;
    ensure!(
        definition.is_object(),
        "dimension type {path} is not an object"
    );
    dimension_timelines(provider, &definition)
}

fn dimension_timelines(
    provider: &mut impl ResourceProvider,
    definition: &Value,
) -> Result<Vec<Value>> {
    let mut result = Vec::new();
    if let Some(timelines) = definition.get("timelines") {
        let mut visited = BTreeSet::new();
        if let Some(tag) = timelines.as_str().and_then(|s| s.strip_prefix('#')) {
            check_timeline_tag(
                provider,
                &ResourceId::parse(tag)?,
                true,
                &mut BTreeSet::new(),
                &mut visited,
                &mut result,
            )?;
        } else {
            let ids = timelines
                .as_array()
                .context("dimension timelines must be a tag or list")?;
            for id in ids {
                let id = ResourceId::parse(id.as_str().context("invalid timeline identifier")?)?;
                // A direct holder set retains duplicate entries, unlike a resolved tag.
                visited.clear();
                check_timeline(provider, &id, true, &mut visited, &mut result)?;
            }
        }
    }
    Ok(result)
}

/// Return a constant value only when no clock-dependent layer exists.
pub fn load_fast_lava(provider: &mut impl ResourceProvider, dimension_type: &str) -> Result<bool> {
    let environment = load_fast_lava_environment(provider, dimension_type)?;
    ensure!(
        environment.tracks.is_empty(),
        "fast lava depends on a world clock"
    );
    Ok(environment.base)
}

fn check_timeline(
    provider: &mut impl ResourceProvider,
    id: &ResourceId,
    required: bool,
    visited: &mut BTreeSet<ResourceId>,
    selected: &mut Vec<Value>,
) -> Result<()> {
    if visited.contains(id) {
        return Ok(());
    }
    let path = format!("data/{}/timeline/{}.json", id.namespace, id.path);
    let Some(bytes) = provider.read_data(&path)? else {
        ensure!(!required, "missing timeline {path}");
        return Ok(());
    };
    let timeline: Value =
        serde_json::from_slice(&bytes).with_context(|| format!("invalid {path}"))?;
    let tracks = timeline
        .get("tracks")
        .and_then(Value::as_object)
        .with_context(|| format!("invalid timeline tracks in {path}"))?;
    for name in tracks.keys() {
        ResourceId::parse(name)?;
    }
    selected.push(timeline);
    visited.insert(id.clone());
    Ok(())
}

fn parse_fast_lava_track(timeline: &Value, track: &Value) -> Result<FastLavaTrack> {
    let clock = ResourceId::parse(
        timeline
            .get("clock")
            .and_then(Value::as_str)
            .context("missing clock")?,
    )?;
    let period = match timeline.get("period_ticks") {
        None => None,
        Some(value) => {
            let value = value
                .as_i64()
                .and_then(|n| i32::try_from(n).ok())
                .context("invalid period")?;
            ensure!(value > 0, "period must be positive");
            Some(value)
        }
    };
    let modifier = match track.get("modifier") {
        None => BooleanModifier::Override,
        Some(value) => BooleanModifier::parse(value.as_str().context("invalid modifier")?)?,
    };
    let constant_easing = match track.get("ease") {
        None => false,
        Some(Value::String(name)) if name == "linear" => false,
        Some(Value::String(name)) if name == "constant" => true,
        _ => anyhow::bail!("unsupported keyframe easing"),
    };
    let values = track
        .get("keyframes")
        .and_then(Value::as_array)
        .context("missing keyframes")?;
    ensure!(!values.is_empty(), "empty keyframes");
    let mut keyframes = Vec::with_capacity(values.len());
    for frame in values {
        let tick = frame
            .get("ticks")
            .and_then(Value::as_i64)
            .and_then(|n| i32::try_from(n).ok())
            .context("invalid keyframe tick")?;
        let value = frame
            .get("value")
            .and_then(Value::as_bool)
            .context("invalid keyframe value")?;
        if let Some(period) = period {
            ensure!((0..=period).contains(&tick), "keyframe outside period");
        }
        if let Some(&(previous, _)) = keyframes.last() {
            ensure!(tick >= previous, "unordered keyframes");
        }
        if keyframes.len() >= 2 {
            ensure!(
                keyframes[keyframes.len() - 2].0 != tick,
                "three identical keyframe ticks"
            );
        }
        keyframes.push((tick, value));
    }
    Ok(FastLavaTrack {
        clock,
        period,
        modifier,
        constant_easing,
        keyframes,
    })
}

fn check_timeline_tag(
    provider: &mut impl ResourceProvider,
    id: &ResourceId,
    required: bool,
    visiting: &mut BTreeSet<ResourceId>,
    visited: &mut BTreeSet<ResourceId>,
    selected: &mut Vec<Value>,
) -> Result<()> {
    ensure!(visiting.insert(id.clone()), "cyclic timeline tag");
    let result = (|| {
        let path = format!("data/{}/tags/timeline/{}.json", id.namespace, id.path);
        let layers = provider.read_data_stack(&path)?;
        if layers.is_empty() {
            ensure!(!required, "missing timeline tag {path}");
            return Ok(());
        }
        let mut entries = Vec::new();
        for bytes in layers {
            let (replace, next) =
                parse_tag(&bytes).with_context(|| format!("invalid timeline tag {path}"))?;
            if replace {
                entries.clear();
            }
            entries.extend(next);
        }
        for entry in entries {
            match entry {
                TagEntry::Item { id, required } => check_timeline(
                    provider,
                    &ResourceId::parse(&id)?,
                    required,
                    visited,
                    selected,
                )?,
                TagEntry::Tag { id, required } => {
                    check_timeline_tag(provider, &id, required, visiting, visited, selected)?
                }
            }
        }
        Ok(())
    })();
    visiting.remove(id);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;

    struct Memory(BTreeMap<String, Value>);
    impl ResourceProvider for Memory {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.0.get(path).map(|v| serde_json::to_vec(v).unwrap()))
        }
        fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
            if folder == "worldgen/biome"
                && self.0.contains_key("data/example/worldgen/biome/test.json")
            {
                return Ok(vec![ResourceId::parse("example:test")?]);
            }
            if folder == "timeline" && self.0.contains_key("data/example/timeline/test.json") {
                Ok(vec![ResourceId::parse("example:test")?])
            } else {
                Ok(Vec::new())
            }
        }
    }

    #[test]
    fn water_evaporation_uses_dimension_and_active_timeline_and_refuses_biome_overrides() {
        let mut source = Memory(BTreeMap::from([
            (
                "data/example/dimension_type/test.json".into(),
                json!({"attributes":{"gameplay/water_evaporates":true}, "timelines":["example:test"]}),
            ),
            ("data/example/worldgen/biome/test.json".into(), json!({})),
            (
                "data/example/timeline/test.json".into(),
                json!({"clock":"example:clock", "tracks":{"gameplay/water_evaporates":{"modifier":"xor", "keyframes":[{"ticks":0,"value":false},{"ticks":2,"value":true}]}}}),
            ),
        ]));
        let environment = load_water_evaporation_environment(&mut source, "example:test").unwrap();
        assert_eq!(environment.sample(|_| Some(0)), Some(true));
        assert_eq!(environment.sample(|_| Some(2)), Some(false));
        assert_eq!(environment.sample(|_| None), None);
        source.0.insert(
            "data/example/worldgen/biome/test.json".into(),
            json!({"attributes":{"gameplay/water_evaporates":false}}),
        );
        assert!(load_water_evaporation_environment(&mut source, "example:test").is_err());
        source.0.remove("data/example/worldgen/biome/test.json");
        assert!(load_water_evaporation_environment(&mut source, "example:test").is_err());
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn installed_dimensions_supply_water_evaporation_rules() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let mut source = crate::packs::ResourceStack::open(&game).unwrap();
        for (dimension, expected) in [
            ("minecraft:overworld", false),
            ("minecraft:the_nether", true),
            ("minecraft:the_end", false),
        ] {
            let environment = load_water_evaporation_environment(&mut source, dimension).unwrap();
            assert_eq!(environment.sample(|_| Some(0)), Some(expected));
        }
    }

    #[test]
    fn parses_fast_lava_modifiers_and_rejects_ambiguous_values() {
        assert!(!fast_lava_from_attributes(&json!({})).unwrap());
        assert!(fast_lava_from_attributes(&json!({"gameplay/fast_lava": true})).unwrap());
        assert!(
            fast_lava_from_attributes(
                &json!({"gameplay/fast_lava":{"modifier":"nand","argument":true}})
            )
            .unwrap()
        );
        assert!(
            !fast_lava_from_attributes(
                &json!({"gameplay/fast_lava":{"modifier":"and","argument":true}})
            )
            .unwrap()
        );
        assert!(
            fast_lava_from_attributes(
                &json!({"gameplay/fast_lava":true,"minecraft:gameplay/fast_lava":false})
            )
            .is_err()
        );
        assert!(
            fast_lava_from_attributes(
                &json!({"gameplay/fast_lava":{"modifier":"bad","argument":true}})
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_unresolved_timeline_and_missing_dimension() {
        let mut source = Memory(BTreeMap::from([
            (
                "data/example/dimension_type/hot.json".into(),
                json!({"attributes":{"gameplay/fast_lava":true}, "timelines":["example:test"]}),
            ),
            (
                "data/example/timeline/test.json".into(),
                json!({"tracks":{"minecraft:gameplay/fast_lava":{}}}),
            ),
        ]));
        assert!(load_fast_lava(&mut source, "example:hot").is_err());
        source.0.insert(
            "data/example/timeline/test.json".into(),
            json!({"tracks":{}}),
        );
        assert!(load_fast_lava(&mut source, "example:hot").unwrap());
        assert!(load_fast_lava(&mut source, "example:missing").is_err());
    }

    #[test]
    fn checks_only_linked_timelines_and_nested_tags() {
        let mut source = Memory(BTreeMap::from([
            (
                "data/example/dimension_type/hot.json".into(),
                json!({"attributes":{"gameplay/fast_lava":true},"timelines":"#example:active"}),
            ),
            (
                "data/example/tags/timeline/active.json".into(),
                json!({"values":["#example:nested", {"id":"example:absent", "required":false}]}),
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
                "data/example/timeline/test.json".into(),
                json!({"tracks":{"minecraft:gameplay/fast_lava":{}}}),
            ),
        ]));
        assert!(load_fast_lava(&mut source, "example:hot").unwrap());
        source.0.insert(
            "data/example/tags/timeline/nested.json".into(),
            json!({"values":["example:test"]}),
        );
        assert!(load_fast_lava(&mut source, "example:hot").is_err());
        source.0.insert(
            "data/example/tags/timeline/nested.json".into(),
            json!({"values":["#example:active"]}),
        );
        assert!(load_fast_lava(&mut source, "example:hot").is_err());
    }

    #[test]
    fn direct_timeline_list_preserves_duplicate_layers() {
        let mut source = Memory(BTreeMap::from([
            (
                "data/example/dimension_type/hot.json".into(),
                json!({"timelines":["example:test", "example:test"]}),
            ),
            (
                "data/example/timeline/test.json".into(),
                json!({"clock":"example:clock", "tracks":{
                    "gameplay/fast_lava":{"modifier":"xor", "keyframes":[{"ticks":0,"value":true}]}
                }}),
            ),
        ]));
        let environment = load_fast_lava_environment(&mut source, "example:hot").unwrap();
        assert_eq!(environment.sample(|_| Some(0)), Some(false));
        assert_eq!(environment.sample(|_| None), None);
    }
}
