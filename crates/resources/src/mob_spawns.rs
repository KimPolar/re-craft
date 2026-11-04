//! Creature spawn entries from one 26.3 environment-attribute layer.
//!
//! Callers must apply every applicable dimension, biome and timeline layer in
//! server order and independently resolve the spawn-probability attribute.
//! This module does not claim to validate unrelated mob categories or costs.

use anyhow::{Context, Result, bail, ensure};
use native_core::random::LegacyRandom;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

use crate::{
    heightmap_tags::resolve,
    packs::{ResourceId, ResourceProvider},
    placed_feature::IntProvider,
    transforms::read_block_tag,
};

/// Resolve the active data-pack tag used by Java's empty spawn-block check.
pub fn load_prevent_mob_spawning_inside(
    provider: &mut impl ResourceProvider,
    registry: &BTreeSet<String>,
) -> Result<BTreeSet<String>> {
    let id = ResourceId::parse("minecraft:prevent_mob_spawning_inside")?;
    let mut raw = BTreeMap::new();
    let mut failed = BTreeSet::new();
    read_block_tag(provider, &id, &mut raw, &mut failed, 0)?;
    resolve(
        &id,
        &raw,
        registry,
        &mut BTreeMap::new(),
        &mut BTreeSet::new(),
    )
    .context("cannot resolve prevent-mob-spawning-inside tag")
}

#[derive(Clone, Debug)]
struct SpawnEntry {
    entity: ResourceId,
    count: IntProvider,
    weight: i32,
}

#[derive(Clone, Debug, Default)]
pub struct CreatureSpawns {
    entries: Vec<SpawnEntry>,
    total_weight: i32,
}

#[derive(Clone, Debug)]
pub struct CreatureSpawnLayer {
    overlay: bool,
    creatures: Option<CreatureSpawns>,
}

#[derive(Clone, Debug)]
pub struct ResolvedCreatureSpawns {
    pub probability: f32,
    pub spawns: CreatureSpawns,
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
struct ProbabilityLayer {
    modifier: FloatModifier,
    argument: f32,
}

impl ProbabilityLayer {
    fn from_attributes(attributes: &Value) -> Result<Option<Self>> {
        let attributes = attributes
            .as_object()
            .context("environment attributes must be an object")?;
        let mut result = None;
        for (name, value) in attributes {
            let id = ResourceId::parse(name)?;
            if id.namespace != "minecraft"
                || id.path != "gameplay/creature_world_gen_spawn_probability"
            {
                continue;
            }
            ensure!(
                result.is_none(),
                "duplicate creature spawn probability attribute"
            );
            let (modifier, argument) = if value.is_number() {
                (FloatModifier::Override, value)
            } else {
                let modifier = value
                    .get("modifier")
                    .and_then(Value::as_str)
                    .context("missing probability modifier")?;
                let modifier = ResourceId::parse(modifier)?;
                ensure!(
                    modifier.namespace == "minecraft",
                    "unsupported probability modifier"
                );
                let modifier = match modifier.path.as_str() {
                    "override" => FloatModifier::Override,
                    "add" => FloatModifier::Add,
                    "subtract" => FloatModifier::Subtract,
                    "multiply" => FloatModifier::Multiply,
                    "minimum" => FloatModifier::Minimum,
                    "maximum" => FloatModifier::Maximum,
                    _ => bail!("unsupported probability modifier"),
                };
                (
                    modifier,
                    value
                        .get("argument")
                        .context("missing probability argument")?,
                )
            };
            let argument = argument.as_f64().context("invalid probability argument")? as f32;
            ensure!(argument.is_finite(), "non-finite probability argument");
            result = Some(Self { modifier, argument });
        }
        Ok(result)
    }

    fn apply(self, base: f32) -> f32 {
        match self.modifier {
            FloatModifier::Override => self.argument,
            FloatModifier::Add => base + self.argument,
            FloatModifier::Subtract => base - self.argument,
            FloatModifier::Multiply => base * self.argument,
            FloatModifier::Minimum => {
                if base.is_nan() || self.argument.is_nan() {
                    f32::NAN
                } else if base == 0.0 && self.argument == 0.0 {
                    f32::from_bits(base.to_bits() | self.argument.to_bits())
                } else {
                    base.min(self.argument)
                }
            }
            FloatModifier::Maximum => {
                if base.is_nan() || self.argument.is_nan() {
                    f32::NAN
                } else if base == 0.0 && self.argument == 0.0 {
                    f32::from_bits(base.to_bits() & self.argument.to_bits())
                } else {
                    base.max(self.argument)
                }
            }
        }
    }
}

/// Resolve the two spawn attributes in Java's dimension, biome, then sampled
/// timeline order. The spawn list's biome uses the seeded full-resolution
/// lookup; the probability's biome uses the direct noise-biome lookup. Timeline
/// layers must already be sampled at the world's current clocks. Weather or
/// custom layers, if present, must be supplied afterward in their server order.
pub fn resolve_creature_spawns(
    dimension: &Value,
    spawn_biome: &Value,
    probability_biome: &Value,
    later_layers: &[Value],
    mut entity_is_misc: impl FnMut(&ResourceId) -> Option<bool>,
) -> Result<ResolvedCreatureSpawns> {
    let mut spawns = CreatureSpawns::default();
    for attributes in [dimension, spawn_biome].into_iter().chain(later_layers) {
        if let Some(layer) = CreatureSpawnLayer::from_attributes(attributes, &mut entity_is_misc)? {
            spawns.apply(layer);
        }
    }
    let mut probability = 0.1_f32;
    for attributes in [dimension, probability_biome]
        .into_iter()
        .chain(later_layers)
    {
        if let Some(layer) = ProbabilityLayer::from_attributes(attributes)? {
            probability = layer.apply(probability);
        }
    }
    ensure!(!probability.is_nan(), "creature spawn probability is NaN");
    probability = probability.clamp(0.0, 0.999_999_9_f32);
    Ok(ResolvedCreatureSpawns {
        probability,
        spawns,
    })
}

/// Read only the active biome's attribute map. Other biome data is left to
/// its own consumers; pack overrides are resolved by the provider.
pub fn load_biome_spawn_attributes(
    provider: &mut impl ResourceProvider,
    biome: &ResourceId,
) -> Result<Value> {
    let path = format!(
        "data/{}/worldgen/biome/{}.json",
        biome.namespace, biome.path
    );
    let bytes = provider
        .read_data(&path)?
        .with_context(|| format!("missing biome {path}"))?;
    let value: Value =
        serde_json::from_slice(&bytes).with_context(|| format!("invalid biome {path}"))?;
    let object = value
        .as_object()
        .context("biome definition must be an object")?;
    let attributes = object
        .get("attributes")
        .cloned()
        .unwrap_or_else(|| Value::Object(Default::default()));
    ensure!(attributes.is_object(), "biome attributes must be an object");
    Ok(attributes)
}

impl CreatureSpawns {
    /// Apply a decoded layer. Overlay replaces only explicitly defined
    /// categories; an explicitly empty creature list removes prior entries.
    pub fn apply(&mut self, layer: CreatureSpawnLayer) {
        if !layer.overlay || layer.creatures.is_some() {
            *self = layer.creatures.unwrap_or_default();
        }
    }

    pub fn is_empty(&self) -> bool {
        self.total_weight == 0
    }

    /// Java's weighted choice followed by its group-count provider. An empty
    /// or all-zero list consumes no random values.
    pub fn choose(&self, random: &mut LegacyRandom) -> Option<(ResourceId, i32)> {
        if self.is_empty() {
            return None;
        }
        let mut choice = random.next_int_bound(self.total_weight)?;
        for entry in &self.entries {
            choice -= entry.weight;
            if choice < 0 {
                return Some((entry.entity.clone(), entry.count.sample_legacy(random)));
            }
        }
        unreachable!("validated creature weight sum")
    }
}

impl CreatureSpawnLayer {
    /// Decode only `gameplay/natural_mob_spawns` in one attribute map.
    /// `entity_is_misc` must consult the active entity registry: `None`
    /// rejects an unknown type, `Some(true)` models Java's pig fallback.
    pub fn from_attributes(
        attributes: &Value,
        mut entity_is_misc: impl FnMut(&ResourceId) -> Option<bool>,
    ) -> Result<Option<Self>> {
        let attributes = attributes
            .as_object()
            .context("environment attributes must be an object")?;
        let mut attribute = None;
        for (name, value) in attributes {
            let id = ResourceId::parse(name)?;
            if id.namespace == "minecraft" && id.path == "gameplay/natural_mob_spawns" {
                ensure!(
                    attribute.replace(value).is_none(),
                    "duplicate natural mob spawn attribute"
                );
            }
        }
        let Some(attribute) = attribute else {
            return Ok(None);
        };
        let (overlay, settings) = if attribute.get("modifier").is_some() {
            let id = ResourceId::parse(
                attribute
                    .get("modifier")
                    .and_then(Value::as_str)
                    .context("invalid spawn modifier")?,
            )?;
            ensure!(id.namespace == "minecraft", "unsupported spawn modifier");
            let overlay = match id.path.as_str() {
                "override" => false,
                "overlay" => true,
                _ => bail!("unsupported spawn modifier"),
            };
            (
                overlay,
                attribute
                    .get("argument")
                    .context("missing spawn modifier argument")?,
            )
        } else {
            (false, attribute)
        };
        let categories = settings
            .get("spawns_by_category")
            .and_then(Value::as_object)
            .context("missing spawn categories")?;
        settings
            .get("spawn_costs")
            .and_then(Value::as_object)
            .context("missing spawn costs")?;
        let creatures = categories
            .get("creature")
            .map(|value| {
                let values = value.as_array().context("creature spawns must be a list")?;
                let mut list = CreatureSpawns::default();
                for value in values {
                    let name = value
                        .get("type")
                        .and_then(Value::as_str)
                        .context("missing creature type")?;
                    let mut entity = ResourceId::parse(name)?;
                    let misc = entity_is_misc(&entity)
                        .with_context(|| format!("unknown entity type {name}"))?;
                    if misc {
                        entity = ResourceId::parse("minecraft:pig")?;
                    }
                    let weight = value
                        .get("weight")
                        .and_then(Value::as_i64)
                        .and_then(|v| i32::try_from(v).ok())
                        .context("invalid creature weight")?;
                    ensure!(weight >= 0, "negative creature weight");
                    list.total_weight = list
                        .total_weight
                        .checked_add(weight)
                        .context("creature weight overflow")?;
                    let count = IntProvider::parse(
                        value.get("count").context("missing creature count")?,
                        0,
                    )?;
                    list.entries.push(SpawnEntry {
                        entity,
                        count,
                        weight,
                    });
                }
                Ok::<_, anyhow::Error>(list)
            })
            .transpose()?;
        Ok(Some(Self { overlay, creatures }))
    }

    pub fn load_biome(
        provider: &mut impl ResourceProvider,
        biome: &ResourceId,
        entity_is_misc: impl FnMut(&ResourceId) -> Option<bool>,
    ) -> Result<Option<Self>> {
        let path = format!(
            "data/{}/worldgen/biome/{}.json",
            biome.namespace, biome.path
        );
        let bytes = provider
            .read_data(&path)?
            .with_context(|| format!("missing biome {path}"))?;
        let biome: Value =
            serde_json::from_slice(&bytes).with_context(|| format!("invalid biome {path}"))?;
        match biome.get("attributes") {
            Some(attributes) => Self::from_attributes(attributes, entity_is_misc),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn layer(value: Value) -> CreatureSpawnLayer {
        CreatureSpawnLayer::from_attributes(
            &json!({"minecraft:gameplay/natural_mob_spawns": value}),
            |_| Some(false),
        )
        .unwrap()
        .unwrap()
    }
    fn settings(creature: Option<Value>) -> Value {
        let mut categories = json!({});
        if let Some(creature) = creature {
            categories["creature"] = creature;
        }
        json!({"spawns_by_category": categories, "spawn_costs": {}})
    }
    #[test]
    fn overlay_replaces_only_defined_creatures_and_empty_choice_does_not_consume_rng() {
        let entries = json!([{"type":"minecraft:pig","weight":1,"count":4}]);
        let mut spawns = CreatureSpawns::default();
        spawns.apply(layer(settings(Some(entries))));
        spawns.apply(layer(
            json!({"modifier":"overlay","argument":settings(None)}),
        ));
        assert!(!spawns.is_empty());
        let mut random = LegacyRandom::new(42);
        assert_eq!(spawns.choose(&mut random).unwrap().1, 4);
        spawns.apply(layer(
            json!({"modifier":"overlay","argument":settings(Some(json!([])))}),
        ));
        let before = random.state();
        assert!(spawns.choose(&mut random).is_none());
        assert_eq!(random.state(), before);
    }
    #[test]
    fn malformed_entries_and_unknown_types_are_rejected() {
        for entry in [
            json!({"type":"minecraft:pig","weight":-1,"count":1}),
            json!({"type":"minecraft:pig","weight":1}),
            json!({"type":"minecraft:pig","weight":1,"count":{"type":"minecraft:uniform","min_inclusive":4,"max_inclusive":2}}),
        ] {
            let attrs = json!({"gameplay/natural_mob_spawns":settings(Some(json!([entry])))});
            assert!(CreatureSpawnLayer::from_attributes(&attrs, |_| Some(false)).is_err());
        }
        let attrs = json!({"gameplay/natural_mob_spawns":settings(Some(json!([{"type":"example:unknown","weight":1,"count":1}])))});
        assert!(CreatureSpawnLayer::from_attributes(&attrs, |_| None).is_err());
    }

    #[test]
    fn prevent_inside_tag_resolves_nested_pack_values() {
        struct Data;
        impl ResourceProvider for Data {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                Ok(match path {
                    "data/minecraft/tags/block/prevent_mob_spawning_inside.json" => {
                        Some(br##"{"values":["#minecraft:rails","minecraft:air"]}"##.to_vec())
                    }
                    "data/minecraft/tags/block/rails.json" => {
                        Some(br#"{"values":["minecraft:rail"]}"#.to_vec())
                    }
                    _ => None,
                })
            }
        }
        let registry = BTreeSet::from(["minecraft:air".into(), "minecraft:rail".into()]);
        assert_eq!(
            load_prevent_mob_spawning_inside(&mut Data, &registry).unwrap(),
            registry
        );
        let incomplete = BTreeSet::from(["minecraft:air".into()]);
        assert!(load_prevent_mob_spawning_inside(&mut Data, &incomplete).is_err());
    }

    #[test]
    fn spawn_attributes_use_distinct_biomes_and_sanitize_after_all_layers() {
        let dimension = json!({"gameplay/creature_world_gen_spawn_probability": 0.8});
        let full_biome = json!({
            "gameplay/creature_world_gen_spawn_probability": 0.01,
            "gameplay/natural_mob_spawns": settings(Some(json!([{"type":"minecraft:sheep","weight":1,"count":4}]))),
        });
        let noise_biome = json!({
            "gameplay/creature_world_gen_spawn_probability": {"modifier":"add","argument":0.8},
            "gameplay/natural_mob_spawns": settings(Some(json!([{"type":"minecraft:pig","weight":1,"count":1}]))),
        });
        let later = [
            json!({"gameplay/creature_world_gen_spawn_probability":{"modifier":"subtract","argument":1.0}}),
        ];
        let resolved =
            resolve_creature_spawns(&dimension, &full_biome, &noise_biome, &later, |_| {
                Some(false)
            })
            .unwrap();
        assert_eq!(
            resolved.probability.to_bits(),
            (0.8_f32 + 0.8_f32 - 1.0_f32).to_bits()
        );
        let (id, count) = resolved.spawns.choose(&mut LegacyRandom::new(42)).unwrap();
        assert_eq!((id.path.as_str(), count), ("sheep", 4));
        assert!(resolve_creature_spawns(&json!({"gameplay/creature_world_gen_spawn_probability":{"modifier":"example:unknown","argument":1}}), &json!({}), &json!({}), &[], |_| Some(false)).is_err());
    }
}
