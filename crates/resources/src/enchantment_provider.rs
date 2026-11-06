//! Data-pack preparation of cost-with-difficulty enchantment providers.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use crate::{
    enchantment_selection::{self, Candidate, LevelCost, Selected},
    packs::{ResourceId, ResourceProvider},
    recipes::{TagEntry, parse_tag},
    worldgen_region_random::RegionRandom,
};

/// Prepared for one item prototype and one frozen data-pack stack.
#[derive(Clone, Debug)]
pub struct DifficultyProvider {
    min_cost: i32,
    max_span: i32,
    enchantability: Option<i32>,
    candidates: Vec<Candidate>,
}

impl DifficultyProvider {
    /// `known_items` is the complete active item registry, not just the item
    /// being enchanted. Unknown required entries must fail rather than alter
    /// the provider's holder order or item eligibility silently.
    pub fn load(
        provider: &mut impl ResourceProvider,
        id: &ResourceId,
        known_items: &BTreeSet<ResourceId>,
        item: &ResourceId,
        enchantability: Option<i32>,
    ) -> Result<Self> {
        ensure!(
            known_items.contains(item),
            "item is missing from the active registry"
        );
        ensure!(
            enchantability.is_none_or(|value| (1..=10_000).contains(&value)),
            "invalid enchantability"
        );
        let enchantments: BTreeSet<_> = provider
            .list_registry_data("enchantment")?
            .into_iter()
            .collect();
        let path = format!(
            "data/{}/enchantment_provider/{}.json",
            id.namespace, id.path
        );
        let definition: Value = serde_json::from_slice(
            &provider
                .read_data(&path)?
                .with_context(|| format!("missing provider {path}"))?,
        )?;
        let kind = ResourceId::parse(
            definition
                .get("type")
                .and_then(Value::as_str)
                .context("provider type is missing")?,
        )?;
        ensure!(
            kind.namespace == "minecraft" && kind.path == "by_cost_with_difficulty",
            "unsupported enchantment provider type"
        );
        let integer = |name: &str| -> Result<i32> {
            definition
                .get(name)
                .and_then(Value::as_i64)
                .and_then(|v| i32::try_from(v).ok())
                .with_context(|| format!("invalid provider {name}"))
        };
        let min_cost = integer("min_cost")?;
        let max_span = integer("max_cost_span")?;
        ensure!(
            (1..=10_000).contains(&min_cost) && (0..=10_000).contains(&max_span),
            "invalid provider cost bounds"
        );
        let mut enchantment_tags = BTreeMap::new();
        let mut item_tags = BTreeMap::new();
        let holders = resolve_set(
            provider,
            "enchantment",
            &enchantments,
            &mut enchantment_tags,
            definition
                .get("enchantments")
                .context("provider enchantments missing")?,
        )?;
        let mut candidates = Vec::new();
        for enchantment in holders {
            let path = format!(
                "data/{}/enchantment/{}.json",
                enchantment.namespace, enchantment.path
            );
            let value: Value = serde_json::from_slice(
                &provider
                    .read_data(&path)?
                    .with_context(|| format!("missing enchantment {path}"))?,
            )?;
            let supported = value
                .get("supported_items")
                .context("supported items missing")?;
            let primary = value.get("primary_items").unwrap_or(supported);
            let items = resolve_set(provider, "item", known_items, &mut item_tags, primary)?;
            if item.namespace != "minecraft" || item.path != "book" {
                if !items.contains(item) {
                    continue;
                }
            }
            let get_int = |field: &str| -> Result<i32> {
                value
                    .get(field)
                    .and_then(Value::as_i64)
                    .and_then(|v| i32::try_from(v).ok())
                    .with_context(|| format!("invalid enchantment {field}"))
            };
            let weight = get_int("weight")?;
            let max_level = get_int("max_level")?;
            ensure!(
                (1..=1024).contains(&weight) && (1..=255).contains(&max_level),
                "invalid enchantment weight or level"
            );
            let cost = |name: &str, level: i32| -> Result<i32> {
                let object = value
                    .get(name)
                    .and_then(Value::as_object)
                    .with_context(|| format!("invalid {name}"))?;
                let integer = |field: &str| {
                    object
                        .get(field)
                        .and_then(Value::as_i64)
                        .and_then(|v| i32::try_from(v).ok())
                        .with_context(|| format!("invalid {name}.{field}"))
                };
                Ok(integer("base")?
                    .wrapping_add(integer("per_level_above_first")?.wrapping_mul(level - 1)))
            };
            let levels = (1..=max_level)
                .map(|level| {
                    Ok(LevelCost {
                        level,
                        min: cost("min_cost", level)?,
                        max: cost("max_cost", level)?,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let exclusive = match value.get("exclusive_set") {
                Some(set) => resolve_set(
                    provider,
                    "enchantment",
                    &enchantments,
                    &mut enchantment_tags,
                    set,
                )?
                .into_iter()
                .collect(),
                None => BTreeSet::new(),
            };
            candidates.push(Candidate {
                id: enchantment,
                weight,
                levels,
                exclusive,
            });
        }
        Ok(Self {
            min_cost,
            max_span,
            enchantability,
            candidates,
        })
    }

    pub fn candidates(&self) -> &[Candidate] {
        &self.candidates
    }

    /// The provider's cost draw and the selection are one transaction.
    pub fn enchant(&self, random: &mut RegionRandom, special: f32) -> Result<Vec<Selected>> {
        let mut staged = random.clone();
        let cost = enchantment_selection::difficulty_cost(
            &mut staged,
            self.min_cost,
            self.max_span,
            special,
        )?;
        let selected = enchantment_selection::select(
            &mut staged,
            cost,
            self.enchantability,
            &self.candidates,
        )?;
        *random = staged;
        Ok(selected)
    }
}

type TagCache = BTreeMap<ResourceId, Option<Vec<ResourceId>>>;

fn resolve_set(
    provider: &mut impl ResourceProvider,
    folder: &str,
    registry: &BTreeSet<ResourceId>,
    cache: &mut TagCache,
    value: &Value,
) -> Result<Vec<ResourceId>> {
    if let Some(tag) = value.as_str().and_then(|v| v.strip_prefix('#')) {
        return resolve_tag(
            provider,
            folder,
            registry,
            cache,
            &ResourceId::parse(tag)?,
            &mut BTreeSet::new(),
        )?
        .context("required holder tag is missing");
    }
    let entries: Vec<&Value> = match value {
        Value::String(_) => vec![value],
        Value::Array(values) => values.iter().collect(),
        _ => bail!("invalid holder set"),
    };
    let mut ids = Vec::new();
    for entry in entries {
        let name = entry.as_str().context("invalid holder")?;
        ensure!(
            !name.starts_with('#'),
            "tags are unsupported inside holder lists"
        );
        let id = ResourceId::parse(name)?;
        ensure!(
            registry.contains(&id),
            "holder is missing from the active registry"
        );
        ids.push(id);
    }
    Ok(ids)
}

fn resolve_tag(
    provider: &mut impl ResourceProvider,
    folder: &str,
    registry: &BTreeSet<ResourceId>,
    cache: &mut TagCache,
    id: &ResourceId,
    visiting: &mut BTreeSet<ResourceId>,
) -> Result<Option<Vec<ResourceId>>> {
    if let Some(cached) = cache.get(id) {
        return Ok(cached.clone());
    }
    ensure!(
        visiting.len() < 128 && cache.len() < 4096,
        "tag dependency limit exceeded"
    );
    ensure!(visiting.insert(id.clone()), "cyclic tag dependency");
    let result = (|| {
        let path = format!("data/{}/tags/{folder}/{}.json", id.namespace, id.path);
        let stack = provider.read_data_stack(&path)?;
        if stack.is_empty() {
            return Ok(None);
        }
        let mut entries = Vec::new();
        for layer in stack {
            let (replace, next) =
                parse_tag(&layer).with_context(|| format!("invalid tag {path}"))?;
            if replace {
                entries.clear();
            }
            entries.extend(next);
        }
        let mut ordered = Vec::new();
        let mut seen = BTreeSet::new();
        for entry in entries {
            let values = match entry {
                TagEntry::Item { id, required } => {
                    let key = ResourceId::parse(&id)?;
                    if registry.contains(&key) {
                        vec![key]
                    } else if required {
                        bail!("required tag entry {id} is missing")
                    } else {
                        vec![]
                    }
                }
                TagEntry::Tag { id, required } => {
                    match resolve_tag(provider, folder, registry, cache, &id, visiting)? {
                        Some(values) => values,
                        None if required => bail!("required nested tag is missing"),
                        None => vec![],
                    }
                }
            };
            for value in values {
                if seen.insert(value.clone()) {
                    ordered.push(value);
                }
            }
        }
        Ok(Some(ordered))
    })();
    visiting.remove(id);
    if let Ok(value) = &result {
        cache.insert(id.clone(), value.clone());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_core::random::LegacyRandom;
    use serde_json::json;

    #[derive(Default)]
    struct Data(BTreeMap<String, Vec<Vec<u8>>>);
    impl Data {
        fn add(&mut self, path: &str, value: Value) {
            self.0
                .entry(path.into())
                .or_default()
                .push(serde_json::to_vec(&value).unwrap());
        }
    }
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.0.get(path).and_then(|layers| layers.last()).cloned())
        }
        fn read_data_stack(&mut self, path: &str) -> Result<Vec<Vec<u8>>> {
            Ok(self.0.get(path).cloned().unwrap_or_default())
        }
        fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
            if folder != "enchantment" {
                bail!("unexpected registry");
            }
            ["example:a", "example:b", "example:c"]
                .into_iter()
                .map(ResourceId::parse)
                .collect()
        }
    }

    fn fixture() -> Result<(Data, BTreeSet<ResourceId>, ResourceId, ResourceId)> {
        let mut data = Data::default();
        data.add("data/example/enchantment_provider/test.json", json!({"type":"minecraft:by_cost_with_difficulty","min_cost":5,"max_cost_span":17,"enchantments":"#example:main"}));
        data.add(
            "data/example/tags/enchantment/main.json",
            json!({"values":["example:a"]}),
        );
        data.add("data/example/tags/enchantment/main.json", json!({"replace":true,"values":["example:c","#example:nested","example:c",{"id":"example:missing","required":false}]}));
        data.add(
            "data/example/tags/enchantment/nested.json",
            json!({"values":["example:b","example:a"]}),
        );
        data.add(
            "data/example/tags/item/applicable.json",
            json!({"values":["minecraft:trident"]}),
        );
        for name in ["a", "b", "c"] {
            data.add(&format!("data/example/enchantment/{name}.json"), json!({"supported_items":"#example:applicable","weight":2,"max_level":2,"min_cost":{"base":2,"per_level_above_first":3},"max_cost":{"base":12,"per_level_above_first":3}}));
        }
        let item = ResourceId::parse("minecraft:trident")?;
        Ok((
            data,
            BTreeSet::from([item.clone()]),
            item,
            ResourceId::parse("example:test")?,
        ))
    }

    #[test]
    fn layered_nested_tags_preserve_holder_order_and_validate_inputs() -> Result<()> {
        let (mut data, items, item, provider) = fixture()?;
        let loaded = DifficultyProvider::load(&mut data, &provider, &items, &item, Some(1))?;
        assert_eq!(
            loaded
                .candidates()
                .iter()
                .map(|value| value.id.path.as_str())
                .collect::<Vec<_>>(),
            ["c", "b", "a"]
        );
        assert_eq!(
            loaded.candidates()[0].levels[1],
            LevelCost {
                level: 2,
                min: 5,
                max: 15
            }
        );
        let mut random = RegionRandom::Legacy(LegacyRandom::new(42));
        let mut before = random.clone();
        assert!(loaded.enchant(&mut random, f32::NAN).is_err());
        assert_eq!(random.next_long(), before.next_long());

        data.add("data/example/enchantment_provider/test.json", json!({"type":"minecraft:by_cost_with_difficulty","min_cost":5,"max_cost_span":17,"enchantments":["example:c","example:c","example:a"]}));
        let direct = DifficultyProvider::load(&mut data, &provider, &items, &item, Some(1))?;
        assert_eq!(
            direct
                .candidates()
                .iter()
                .map(|value| value.id.path.as_str())
                .collect::<Vec<_>>(),
            ["c", "c", "a"]
        );
        direct.enchant(&mut random, 0.5)?;

        let (mut data, items, item, provider) = fixture()?;
        data.add(
            "data/example/tags/enchantment/nested.json",
            json!({"replace":true,"values":["#example:main"]}),
        );
        assert!(DifficultyProvider::load(&mut data, &provider, &items, &item, Some(1)).is_err());
        let (mut data, items, item, provider) = fixture()?;
        data.add(
            "data/example/tags/enchantment/main.json",
            json!({"replace":true,"values":["example:missing"]}),
        );
        assert!(DifficultyProvider::load(&mut data, &provider, &items, &item, Some(1)).is_err());
        Ok(())
    }
}
