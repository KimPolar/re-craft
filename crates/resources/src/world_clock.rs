//! Dimension default clocks come from the world's active data packs.

use crate::packs::{ResourceId, ResourceProvider};
use anyhow::{Context, Result, ensure};
use std::collections::{BTreeMap, BTreeSet};

/// Validate active world-clock definitions before exposing them as commands.
pub fn registered_clocks(provider: &mut impl ResourceProvider) -> Result<BTreeSet<ResourceId>> {
    let mut result = BTreeSet::new();
    for id in provider.list_data("world_clock")? {
        let path = format!("data/{}/world_clock/{}.json", id.namespace, id.path);
        let bytes = provider
            .read_data(&path)?
            .with_context(|| format!("missing world clock {path}"))?;
        validate_clock_definition(&bytes).with_context(|| format!("invalid world clock {path}"))?;
        result.insert(id);
    }
    Ok(result)
}

pub fn validate_clock_definition(bytes: &[u8]) -> Result<()> {
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    ensure!(value.is_object(), "world clock must be an object");
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeMarker {
    pub ticks: i32,
    pub period: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimelineDefinition {
    pub clock: ResourceId,
    pub period: Option<i32>,
}

/// Load clock and period information, including timelines without markers.
pub fn timelines(
    provider: &mut impl ResourceProvider,
) -> Result<BTreeMap<ResourceId, TimelineDefinition>> {
    let mut result = BTreeMap::new();
    for id in provider.list_data("timeline")? {
        let path = format!("data/{}/timeline/{}.json", id.namespace, id.path);
        let bytes = provider
            .read_data(&path)?
            .with_context(|| format!("missing timeline {path}"))?;
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).with_context(|| format!("invalid timeline {path}"))?;
        let clock = ResourceId::parse(
            value
                .get("clock")
                .and_then(serde_json::Value::as_str)
                .context("timeline clock is missing")?,
        )?;
        let period = value
            .get("period_ticks")
            .map(|value| {
                let n = i32::try_from(value.as_i64().context("period_ticks must be an integer")?)?;
                ensure!(n > 0, "period_ticks must be positive");
                Ok::<_, anyhow::Error>(n)
            })
            .transpose()?;
        result.insert(id, TimelineDefinition { clock, period });
    }
    Ok(result)
}

/// Resolve time markers from the active timeline registry. Ambiguous duplicate
/// markers are rejected rather than guessing registry load order.
pub fn time_markers(
    provider: &mut impl ResourceProvider,
) -> Result<BTreeMap<(ResourceId, ResourceId), TimeMarker>> {
    let mut result = BTreeMap::new();
    for timeline in provider.list_data("timeline")? {
        let path = format!(
            "data/{}/timeline/{}.json",
            timeline.namespace, timeline.path
        );
        let bytes = provider
            .read_data(&path)?
            .with_context(|| format!("missing timeline {path}"))?;
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).with_context(|| format!("invalid timeline {path}"))?;
        let markers = match value.get("time_markers") {
            None => continue,
            Some(value) => value
                .as_object()
                .context("time_markers must be an object")?,
        };
        let clock = ResourceId::parse(
            value
                .get("clock")
                .and_then(serde_json::Value::as_str)
                .context("timeline clock is missing")?,
        )?;
        let period = value
            .get("period_ticks")
            .map(|value| {
                let n = value.as_i64().context("period_ticks must be an integer")?;
                let n = i32::try_from(n)?;
                ensure!(n > 0, "period_ticks must be positive");
                Ok::<_, anyhow::Error>(n)
            })
            .transpose()?;
        for (name, marker) in markers {
            if let Some(show) = marker.get("show_in_commands") {
                ensure!(show.is_boolean(), "show_in_commands must be a boolean");
            }
            let ticks = if marker.is_object() {
                marker
                    .get("ticks")
                    .context("time marker ticks are missing")?
            } else {
                marker
            };
            let ticks = i32::try_from(
                ticks
                    .as_i64()
                    .context("time marker ticks must be an integer")?,
            )?;
            ensure!(
                ticks >= 0 && period.is_none_or(|period| ticks < period),
                "time marker is outside its timeline"
            );
            let marker_id = ResourceId::parse(name)?;
            ensure!(
                result
                    .insert((clock.clone(), marker_id), TimeMarker { ticks, period })
                    .is_none(),
                "duplicate time marker in {path}"
            );
        }
    }
    Ok(result)
}

pub fn dimension_default_clock(
    provider: &mut impl ResourceProvider,
    dimension_type: &str,
) -> Result<Option<ResourceId>> {
    let id = ResourceId::parse(dimension_type)?;
    let path = format!("data/{}/dimension_type/{}.json", id.namespace, id.path);
    let data = provider
        .read_data(&path)?
        .with_context(|| format!("missing dimension type {path}"))?;
    let definition: serde_json::Value =
        serde_json::from_slice(&data).with_context(|| format!("invalid dimension type {path}"))?;
    ensure!(
        definition.is_object(),
        "dimension type {path} must be an object"
    );
    match definition.get("default_clock") {
        None => Ok(None),
        Some(value) => {
            let name = value
                .as_str()
                .context("default_clock must be a resource identifier")?;
            Ok(Some(ResourceId::parse(name)?))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    struct Data(BTreeMap<String, Vec<u8>>);
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.0.get(path).cloned())
        }
        fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
            let mut result = Vec::new();
            for path in self.0.keys() {
                if let Some(rest) = path.strip_prefix("data/")
                    && let Some((namespace, tail)) = rest.split_once('/')
                    && let Some(name) = tail.strip_prefix(&format!("{folder}/"))
                    && let Some(name) = name.strip_suffix(".json")
                {
                    result.push(ResourceId::parse(&format!("{namespace}:{name}"))?);
                }
            }
            Ok(result)
        }
    }
    #[test]
    fn resolves_custom_default_clock_and_rejects_invalid_types() {
        let path = "data/example/dimension_type/custom.json";
        let mut provider = Data(BTreeMap::new());
        for (json, expected) in [
            (r#"{}"#, None),
            (
                r#"{"default_clock":"overworld"}"#,
                Some(("minecraft", "overworld")),
            ),
            (
                r#"{"default_clock":"example:custom"}"#,
                Some(("example", "custom")),
            ),
        ] {
            provider.0.insert(path.into(), json.as_bytes().to_vec());
            let id = dimension_default_clock(&mut provider, "example:custom").unwrap();
            assert_eq!(
                id.map(|id| (id.namespace, id.path)),
                expected.map(|(a, b)| (a.into(), b.into()))
            );
        }
        for json in [
            r#"{"default_clock":null}"#,
            r#"{"default_clock":3}"#,
            r#"{"default_clock":"Bad:clock"}"#,
            "[]",
        ] {
            provider.0.insert(path.into(), json.as_bytes().to_vec());
            assert!(dimension_default_clock(&mut provider, "example:custom").is_err());
        }
    }

    #[test]
    fn reads_markers_from_active_timelines_and_rejects_ambiguity() {
        let mut provider = Data(BTreeMap::new());
        provider.0.insert("data/example/timeline/custom.json".into(), br#"{"clock":"example:clock","period_ticks":48,"time_markers":{"minecraft:day":{"ticks":12,"show_in_commands":true},"example:hidden":7}}"#.to_vec());
        let markers = time_markers(&mut provider).unwrap();
        let clock = ResourceId::parse("example:clock").unwrap();
        assert_eq!(
            markers[&(clock.clone(), ResourceId::parse("minecraft:day").unwrap())],
            TimeMarker {
                ticks: 12,
                period: Some(48)
            }
        );
        assert_eq!(
            markers[&(clock.clone(), ResourceId::parse("example:hidden").unwrap())],
            TimeMarker {
                ticks: 7,
                period: Some(48)
            }
        );
        provider.0.insert(
            "data/example/timeline/duplicate.json".into(),
            br#"{"clock":"example:clock","time_markers":{"minecraft:day":6}}"#.to_vec(),
        );
        assert!(time_markers(&mut provider).is_err());
        provider.0.remove("data/example/timeline/duplicate.json");
        for json in [
            r#"{"clock":"example:clock","time_markers":{"minecraft:day":-1}}"#,
            r#"{"clock":"example:clock","period_ticks":48,"time_markers":{"minecraft:day":48}}"#,
            r#"{"clock":"example:clock","time_markers":{"minecraft:day":{"ticks":1,"show_in_commands":"yes"}}}"#,
        ] {
            provider.0.insert(
                "data/example/timeline/custom.json".into(),
                json.as_bytes().to_vec(),
            );
            assert!(time_markers(&mut provider).is_err());
        }
    }

    #[test]
    fn registered_clocks_follow_active_data_and_reject_invalid_definitions() {
        let mut provider = Data(BTreeMap::from([(
            "data/example/world_clock/clock.json".into(),
            b"{}".to_vec(),
        )]));
        assert_eq!(
            registered_clocks(&mut provider).unwrap(),
            BTreeSet::from([ResourceId::parse("example:clock").unwrap()])
        );
        provider
            .0
            .insert("data/example/world_clock/clock.json".into(), b"[]".to_vec());
        assert!(registered_clocks(&mut provider).is_err());
    }

    #[test]
    fn loads_timelines_without_markers_and_rejects_invalid_periods() {
        let path = "data/example/timeline/custom.json";
        let mut provider = Data(BTreeMap::from([(
            path.into(),
            br#"{"clock":"example:clock","period_ticks":48}"#.to_vec(),
        )]));
        let result = timelines(&mut provider).unwrap();
        assert_eq!(
            result[&ResourceId::parse("example:custom").unwrap()],
            TimelineDefinition {
                clock: ResourceId::parse("example:clock").unwrap(),
                period: Some(48),
            }
        );
        for json in [
            r#"{}"#,
            r#"{"clock":"example:clock","period_ticks":0}"#,
            r#"{"clock":"example:clock","period_ticks":1.5}"#,
        ] {
            provider.0.insert(path.into(), json.as_bytes().to_vec());
            assert!(timelines(&mut provider).is_err());
        }
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn installed_dimensions_use_registered_defaults() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let mut provider = crate::packs::ResourceStack::open(&game).unwrap();
        for (dimension, expected) in [
            ("minecraft:overworld", Some("overworld")),
            ("minecraft:the_nether", None),
            ("minecraft:the_end", Some("the_end")),
        ] {
            assert_eq!(
                dimension_default_clock(&mut provider, dimension)
                    .unwrap()
                    .map(|id| id.path),
                expected.map(str::to_owned)
            );
        }
        let markers = time_markers(&mut provider).unwrap();
        let timeline_definitions = timelines(&mut provider).unwrap();
        assert!(!timeline_definitions.is_empty());
        let clocks = registered_clocks(&mut provider).unwrap();
        assert!(clocks.contains(&ResourceId::parse("minecraft:overworld").unwrap()));
        assert!(clocks.contains(&ResourceId::parse("minecraft:the_end").unwrap()));
        let clock = ResourceId::parse("minecraft:overworld").unwrap();
        for (name, ticks) in [
            ("day", 1000),
            ("noon", 6000),
            ("night", 13000),
            ("midnight", 18000),
        ] {
            assert_eq!(
                markers[&(clock.clone(), ResourceId::parse(name).unwrap())],
                TimeMarker {
                    ticks,
                    period: Some(24000)
                }
            );
        }
    }
}
