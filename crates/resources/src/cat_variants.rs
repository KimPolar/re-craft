//! Spawn selection and random ordering for freshly generated cats.
use std::cmp::Reverse;

use anyhow::{Context, Result, ensure};
use serde_json::Value;

use crate::{
    packs::{ResourceId, ResourceProvider},
    worldgen_region_random::{
        AnimalBaseInitialization, AnimalGroupData, RegionRandom, finalize_animal_base,
    },
};

struct Selector {
    priority: i32,
    variant: ResourceId,
    condition: Option<Value>,
}

/// Active, registry-ordered cat variants. The caller must validate complete
/// registry entries and evaluate conditions against the actual spawn context.
pub struct CatSpawnVariants {
    selectors: Vec<Selector>,
    default_visual: ResourceId,
    sounds: Vec<ResourceId>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CatInitialization {
    pub animal: AnimalBaseInitialization,
    pub variant: ResourceId,
    pub sound_variant: ResourceId,
}

impl CatSpawnVariants {
    pub fn load_active(provider: &mut impl ResourceProvider) -> Result<Self> {
        let variants = provider.list_registry_data("cat_variant")?;
        let black = ResourceId::parse("minecraft:black")?;
        let default_visual = variants
            .iter()
            .find(|id| **id == black)
            .or_else(|| variants.first())
            .context("cat variant registry is empty")?
            .clone();
        let mut selectors = Vec::new();
        for id in &variants {
            let path = format!("data/{}/cat_variant/{}.json", id.namespace, id.path);
            let bytes = provider
                .read_data(&path)?
                .with_context(|| format!("missing variant {path}"))?;
            let value: Value = serde_json::from_slice(&bytes)
                .with_context(|| format!("invalid variant {path}"))?;
            let object = value.as_object().context("cat variant must be an object")?;
            let Some(entries) = object.get("spawn_conditions") else {
                continue;
            };
            for entry in entries
                .as_array()
                .context("cat spawn_conditions must be an array")?
            {
                let entry = entry
                    .as_object()
                    .context("cat spawn selector must be an object")?;
                let priority = entry
                    .get("priority")
                    .and_then(Value::as_i64)
                    .and_then(|n| i32::try_from(n).ok())
                    .context("invalid cat selector priority")?;
                let condition = entry.get("condition").cloned();
                if let Some(value) = &condition {
                    let kind = value
                        .as_object()
                        .and_then(|v| v.get("type"))
                        .and_then(Value::as_str)
                        .context("cat spawn condition is missing its type")?;
                    ResourceId::parse(kind)?;
                }
                selectors.push(Selector {
                    priority,
                    variant: id.clone(),
                    condition,
                });
            }
        }
        selectors.sort_by_key(|selector| Reverse(selector.priority));
        let sounds = provider.list_registry_data("cat_sound_variant")?;
        ensure!(
            !sounds.is_empty() && i32::try_from(sounds.len()).is_ok(),
            "invalid cat sound registry size"
        );
        Ok(Self {
            selectors,
            default_visual,
            sounds,
        })
    }

    /// Evaluator receives the active data pack's condition JSON, in Java's
    /// priority order. Errors leave the region RNG unchanged; callers must
    /// reject conditions they cannot evaluate rather than guess.
    pub fn finalize_hut_cat(
        &self,
        random: &mut RegionRandom,
        mut condition_matches: impl FnMut(&Value) -> Result<bool>,
    ) -> Result<CatInitialization> {
        let mut next = random.clone();
        let animal = finalize_animal_base(&mut next, &mut AnimalGroupData::default(), false);
        let mut priority = None;
        let mut matches = Vec::new();
        for selector in &self.selectors {
            if priority.is_some_and(|p| selector.priority < p) {
                break;
            }
            if selector
                .condition
                .as_ref()
                .map(&mut condition_matches)
                .transpose()?
                .unwrap_or(true)
            {
                priority = Some(selector.priority);
                matches.push(&selector.variant);
            }
        }
        let variant = if matches.is_empty() {
            self.default_visual.clone()
        } else {
            let count = i32::try_from(matches.len()).context("too many matching cat selectors")?;
            matches[next
                .next_int_bound(count)
                .context("invalid cat selector count")? as usize]
                .clone()
        };
        let sound = next
            .next_int_bound(self.sounds.len() as i32)
            .context("empty cat sound registry")? as usize;
        let result = CatInitialization {
            animal,
            variant,
            sound_variant: self.sounds[sound].clone(),
        };
        *random = next;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::density::WorldgenRandom;
    use serde_json::json;

    struct Data;
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            let conditions = match path {
                "data/minecraft/cat_variant/all_black.json" => json!([
                    {"condition":{"type":"minecraft:structure","structures":"#minecraft:cats_spawn_as_black"},"priority":1},
                    {"condition":{"type":"minecraft:moon_brightness","range":{"min":0.9}},"priority":0}
                ]),
                "data/minecraft/cat_variant/black.json" => json!([{"priority":0}]),
                _ => return Ok(None),
            };
            Ok(Some(serde_json::to_vec(
                &json!({"spawn_conditions": conditions}),
            )?))
        }
        fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
            let names = match folder {
                "cat_variant" => vec!["black", "all_black"],
                "cat_sound_variant" => vec!["royal", "classic"],
                _ => anyhow::bail!("unexpected registry"),
            };
            names
                .into_iter()
                .map(|name| ResourceId::parse(&format!("minecraft:{name}")))
                .collect()
        }
    }

    #[test]
    fn structure_selector_wins_and_failed_context_preserves_rng() {
        let selectors = CatSpawnVariants::load_active(&mut Data).unwrap();
        let mut random = RegionRandom::new(42, WorldgenRandom::Legacy, -13, 17);
        let mut untouched = random.clone();
        assert!(
            selectors
                .finalize_hut_cat(&mut random, |_| anyhow::bail!(
                    "structure context unavailable"
                ))
                .is_err()
        );
        assert_eq!(random.next_long(), untouched.next_long());
        let mut random = RegionRandom::new(42, WorldgenRandom::Legacy, -13, 17);
        let mut visited = Vec::new();
        let cat = selectors
            .finalize_hut_cat(&mut random, |condition| {
                visited.push(condition["type"].as_str().unwrap().to_owned());
                Ok(condition["type"] == "minecraft:structure")
            })
            .unwrap();
        assert_eq!(
            cat.variant,
            ResourceId::parse("minecraft:all_black").unwrap()
        );
        assert_eq!(visited, ["minecraft:structure"]);
        assert!(!cat.animal.set_baby_age);
    }
}
