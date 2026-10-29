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

/// Dimension and biome layers for a world whose active timelines do not modify
/// respawn attributes. Timeline-driven rules are rejected until the clocks and
/// keyframe samplers are available, rather than approximated with a fixed value.
#[derive(Clone, Debug)]
pub struct RespawnEnvironmentRegistry {
    dimension: RespawnEnvironmentLayer,
    biomes: BTreeMap<String, RespawnEnvironmentLayer>,
}

impl RespawnEnvironmentRegistry {
    pub fn load(provider: &mut impl ResourceProvider, dimension_type: &str) -> Result<Self> {
        for id in provider.list_data("timeline")? {
            let path = format!("data/{}/timeline/{}.json", id.namespace, id.path);
            let bytes = provider
                .read_data(&path)?
                .with_context(|| format!("missing {path}"))?;
            let timeline: Value =
                serde_json::from_slice(&bytes).with_context(|| format!("invalid {path}"))?;
            let tracks = timeline
                .get("tracks")
                .and_then(Value::as_object)
                .with_context(|| format!("invalid timeline tracks in {path}"))?;
            for key in tracks.keys() {
                let attr = ResourceId::parse(key)?;
                ensure!(
                    attr.namespace != "minecraft"
                        || !matches!(
                            attr.path.as_str(),
                            "gameplay/bed_rule"
                                | "gameplay/straw_bed_rule"
                                | "gameplay/respawn_anchor_works"
                        ),
                    "timeline {path} modifies a respawn rule, which is not yet supported"
                );
            }
        }
        let dimension = RespawnEnvironmentLayer::load_dimension_type(provider, dimension_type)?;
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
        Ok(Self { dimension, biomes })
    }

    pub fn for_biome(&self, biome: &str) -> Result<RespawnEnvironment> {
        let id = ResourceId::parse(biome)?;
        let name = format!("{}:{}", id.namespace, id.path);
        let layer = self
            .biomes
            .get(&name)
            .with_context(|| format!("unknown biome {name}"))?;
        let mut result = RespawnEnvironment::default();
        result.apply(self.dimension);
        result.apply(*layer);
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
