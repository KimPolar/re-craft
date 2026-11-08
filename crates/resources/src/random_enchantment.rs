//! The 26.3 random-enchantment loot selection, prepared from active data packs.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use serde_json::Value;

use crate::{
    chest_loot::ChestLootRandom,
    enchantment_provider::{TagCache, resolve_set},
    enchantment_selection::Selected,
    packs::{ResourceId, ResourceProvider},
};

#[derive(Clone, Debug)]
struct Candidate {
    id: ResourceId,
    max_level: i32,
    supported_items: BTreeSet<ResourceId>,
}

/// The exact holder order and item compatibility from one immutable pack stack.
#[derive(Clone, Debug)]
pub struct RandomEnchantment {
    candidates: Vec<Candidate>,
    known_items: Arc<BTreeSet<ResourceId>>,
    only_compatible: bool,
}

impl RandomEnchantment {
    pub fn load(
        provider: &mut impl ResourceProvider,
        known_items: Arc<BTreeSet<ResourceId>>,
        options: Option<&Value>,
        only_compatible: bool,
    ) -> Result<Self> {
        let registry_order = provider.list_registry_data("enchantment")?;
        let registry: BTreeSet<_> = registry_order.iter().cloned().collect();
        let mut tags = TagCache::new();
        let holders = match options {
            Some(options) => resolve_set(provider, "enchantment", &registry, &mut tags, options)?,
            None => registry_order,
        };
        let mut item_tags = TagCache::new();
        let mut definitions = BTreeMap::<ResourceId, Candidate>::new();
        let mut candidates = Vec::with_capacity(holders.len());
        for id in holders {
            if let Some(candidate) = definitions.get(&id) {
                candidates.push(candidate.clone());
                continue;
            }
            let path = format!("data/{}/enchantment/{}.json", id.namespace, id.path);
            let bytes = provider
                .read_data(&path)?
                .with_context(|| format!("missing enchantment {path}"))?;
            let value: Value = serde_json::from_slice(&bytes)
                .with_context(|| format!("invalid enchantment {path}"))?;
            let max_level = value
                .get("max_level")
                .and_then(Value::as_i64)
                .and_then(|level| i32::try_from(level).ok())
                .context("invalid enchantment maximum level")?;
            ensure!(
                (1..=255).contains(&max_level),
                "invalid enchantment maximum level"
            );
            let supported_items = if only_compatible {
                resolve_set(
                    provider,
                    "item",
                    &known_items,
                    &mut item_tags,
                    value
                        .get("supported_items")
                        .context("enchantment supported items missing")?,
                )?
                .into_iter()
                .collect()
            } else {
                BTreeSet::new()
            };
            let candidate = Candidate {
                id: id.clone(),
                max_level,
                supported_items,
            };
            definitions.insert(id, candidate.clone());
            candidates.push(candidate);
        }
        Ok(Self {
            candidates,
            known_items,
            only_compatible,
        })
    }

    /// Selection and level use the supplied stream; failures leave it unchanged.
    pub fn select<R: ChestLootRandom>(
        &self,
        item: &ResourceId,
        random: &mut R,
    ) -> Result<Option<Selected>> {
        ensure!(self.known_items.contains(item), "item is not registered");
        let book = item.namespace == "minecraft" && item.path == "book";
        let candidates: Vec<_> = self
            .candidates
            .iter()
            .filter(|candidate| {
                book || !self.only_compatible || candidate.supported_items.contains(item)
            })
            .collect();
        if candidates.is_empty() {
            return Ok(None);
        }
        let mut staged = random.clone();
        let count = i32::try_from(candidates.len()).context("too many enchantments")?;
        let index = staged
            .next_int(count)
            .context("random source rejected enchantment count")?;
        ensure!(
            (0..count).contains(&index),
            "invalid random enchantment index"
        );
        let candidate = candidates[index as usize];
        let level = if candidate.max_level == 1 {
            1
        } else {
            let n = staged
                .next_int(candidate.max_level)
                .context("random source rejected enchantment level")?;
            ensure!(
                (0..candidate.max_level).contains(&n),
                "invalid random enchantment level"
            );
            n + 1
        };
        *random = staged;
        Ok(Some(Selected {
            id: candidate.id.clone(),
            level,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_core::random::LegacyRandom;
    use serde_json::json;

    struct Data {
        files: BTreeMap<String, Vec<Vec<u8>>>,
    }
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
            Ok(if folder == "enchantment" {
                vec![
                    ResourceId::parse("example:b")?,
                    ResourceId::parse("example:a")?,
                ]
            } else {
                vec![]
            })
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self
                .files
                .get(path)
                .and_then(|layers| layers.last())
                .cloned())
        }
        fn read_data_stack(&mut self, path: &str) -> Result<Vec<Vec<u8>>> {
            Ok(self.files.get(path).cloned().unwrap_or_default())
        }
    }

    #[test]
    fn tag_replacement_order_compatibility_and_errors_preserve_random() -> Result<()> {
        let mut data = Data {
            files: BTreeMap::new(),
        };
        let mut put = |path: &str, layers: Vec<Value>| {
            data.files.insert(
                path.into(),
                layers
                    .iter()
                    .map(|v| serde_json::to_vec(v).unwrap())
                    .collect(),
            );
        };
        put(
            "data/example/tags/enchantment/options.json",
            vec![
                json!({"values":["example:a"]}),
                json!({"replace":true,"values":["example:b","#example:nested"]}),
            ],
        );
        put(
            "data/example/tags/enchantment/nested.json",
            vec![
                json!({"values":["example:a", "example:b", {"id":"example:missing","required":false}]}),
            ],
        );
        put(
            "data/example/enchantment/a.json",
            vec![json!({"max_level":1,"supported_items":["minecraft:stone"]})],
        );
        put(
            "data/example/enchantment/b.json",
            vec![json!({"max_level":3,"supported_items":["minecraft:diamond_sword"]})],
        );
        let items: BTreeSet<_> = [
            "minecraft:book",
            "minecraft:stone",
            "minecraft:diamond_sword",
        ]
        .into_iter()
        .map(ResourceId::parse)
        .collect::<Result<_>>()?;
        let items = Arc::new(items);
        let selector = RandomEnchantment::load(
            &mut data,
            Arc::clone(&items),
            Some(&json!("#example:options")),
            true,
        )?;
        assert_eq!(
            selector
                .candidates
                .iter()
                .map(|candidate| candidate.id.path.as_str())
                .collect::<Vec<_>>(),
            ["b", "a"]
        );
        let mut rng = LegacyRandom::new(42);
        assert_eq!(
            selector
                .select(&ResourceId::parse("minecraft:diamond_sword")?, &mut rng)?
                .unwrap()
                .id,
            ResourceId::parse("example:b")?
        );
        let before = rng.state();
        assert!(
            selector
                .select(&ResourceId::parse("example:unregistered")?, &mut rng)
                .is_err()
        );
        assert_eq!(rng.state(), before);
        let empty = RandomEnchantment::load(&mut data, Arc::clone(&items), Some(&json!([])), true)?;
        assert!(
            empty
                .select(&ResourceId::parse("minecraft:book")?, &mut rng)?
                .is_none()
        );
        assert_eq!(rng.state(), before);
        assert!(
            RandomEnchantment::load(
                &mut data,
                Arc::clone(&items),
                Some(&json!("#example:missing")),
                true
            )
            .is_err()
        );
        Ok(())
    }
}
