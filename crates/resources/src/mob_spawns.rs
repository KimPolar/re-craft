//! Creature spawn entries from one 26.3 environment-attribute layer.
//!
//! Callers must apply every applicable dimension, biome and timeline layer in
//! server order and independently resolve the spawn-probability attribute.
//! This module does not claim to validate unrelated mob categories or costs.

use anyhow::{Context, Result, bail, ensure};
use native_core::random::LegacyRandom;
use serde_json::Value;

use crate::{
    packs::{ResourceId, ResourceProvider},
    placed_feature::IntProvider,
};

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
}
