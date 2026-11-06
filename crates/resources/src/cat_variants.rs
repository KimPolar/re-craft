//! Spawn selection and random ordering for freshly generated cats.
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use crate::{
    packs::{ResourceId, ResourceProvider},
    recipes::{TagEntry, parse_tag},
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

enum WorldCondition {
    Structures(BTreeSet<ResourceId>),
    Biomes(BTreeSet<ResourceId>),
    Moon { min: Option<f64>, max: Option<f64> },
}

/// Conditions compiled against the same immutable pack stack and registries.
pub struct CatWorldConditions {
    predicates: BTreeMap<String, WorldCondition>,
}

/// The caller must supply all structure IDs with an actual piece containing
/// the spawn position, not just IDs whose expanded start bounds contain it.
#[derive(Clone, Copy)]
pub struct CatSpawnContext<'a> {
    pub biome: Option<&'a ResourceId>,
    pub moon_phase: Option<u8>,
    pub structures_with_piece: Option<&'a BTreeSet<ResourceId>>,
}

/// World information queried only when a matching selector needs it.
pub trait SpawnVariantEnvironment {
    fn biome(&self) -> Result<&ResourceId>;
    fn moon_phase(&mut self) -> Result<u8>;
    fn structures_with_piece(&mut self) -> Result<&BTreeSet<ResourceId>>;
}

impl SpawnVariantEnvironment for CatSpawnContext<'_> {
    fn biome(&self) -> Result<&ResourceId> {
        self.biome.context("spawn biome unavailable")
    }

    fn moon_phase(&mut self) -> Result<u8> {
        self.moon_phase.context("moon phase unavailable")
    }

    fn structures_with_piece(&mut self) -> Result<&BTreeSet<ResourceId>> {
        self.structures_with_piece
            .context("structure pieces unavailable")
    }
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

    /// Compile the condition types against active structure and biome tags.
    /// Unsupported predicates fail rather than selecting a guessed variant.
    pub fn prepare_world_conditions(
        &self,
        provider: &mut impl ResourceProvider,
    ) -> Result<CatWorldConditions> {
        CatWorldConditions::prepare_values(
            provider,
            self.selectors
                .iter()
                .filter_map(|selector| selector.condition.as_ref()),
        )
    }
}

impl CatWorldConditions {
    pub(crate) fn uses_structures(&self) -> bool {
        self.predicates
            .values()
            .any(|condition| matches!(condition, WorldCondition::Structures(_)))
    }
    pub(crate) fn prepare_values<'a>(
        provider: &mut impl ResourceProvider,
        conditions: impl IntoIterator<Item = &'a Value>,
    ) -> Result<Self> {
        let mut predicates = BTreeMap::new();
        let mut structures = None;
        let mut biomes = None;
        let mut structure_tags = BTreeMap::new();
        let mut biome_tags = BTreeMap::new();
        for value in conditions {
            let key = serde_json::to_string(value)?;
            if predicates.contains_key(&key) {
                continue;
            }
            let object = value.as_object().context("condition must be an object")?;
            let kind = ResourceId::parse(
                object
                    .get("type")
                    .and_then(Value::as_str)
                    .context("missing condition type")?,
            )?;
            let predicate = match (kind.namespace.as_str(), kind.path.as_str()) {
                ("minecraft", "structure") => {
                    if structures.is_none() {
                        structures = Some(
                            provider
                                .list_data("worldgen/structure")?
                                .into_iter()
                                .collect::<BTreeSet<_>>(),
                        );
                    }
                    WorldCondition::Structures(resolve_selection(
                        provider,
                        "structure",
                        structures.as_ref().unwrap(),
                        object.get("structures").context("missing structures")?,
                        &mut structure_tags,
                    )?)
                }
                ("minecraft", "biome") => {
                    if biomes.is_none() {
                        biomes = Some(
                            provider
                                .list_data("worldgen/biome")?
                                .into_iter()
                                .collect::<BTreeSet<_>>(),
                        );
                    }
                    WorldCondition::Biomes(resolve_selection(
                        provider,
                        "biome",
                        biomes.as_ref().unwrap(),
                        object.get("biomes").context("missing biomes")?,
                        &mut biome_tags,
                    )?)
                }
                ("minecraft", "moon_brightness") => {
                    let range = object
                        .get("range")
                        .context("missing moon brightness range")?;
                    let (min, max) = if let Some(value) = range.as_f64() {
                        (Some(value), Some(value))
                    } else {
                        let range = range.as_object().context("invalid moon brightness range")?;
                        let bound = |key: &str| -> Result<Option<f64>> {
                            range
                                .get(key)
                                .map(|value| {
                                    value.as_f64().context("invalid moon brightness bound")
                                })
                                .transpose()
                        };
                        (bound("min")?, bound("max")?)
                    };
                    ensure!(
                        min.zip(max).is_none_or(|(min, max)| min <= max),
                        "inverted moon brightness range"
                    );
                    WorldCondition::Moon { min, max }
                }
                _ => bail!(
                    "unsupported cat spawn condition {}:{}",
                    kind.namespace,
                    kind.path
                ),
            };
            predicates.insert(key, predicate);
        }
        Ok(Self { predicates })
    }
}

impl CatWorldConditions {
    pub(crate) fn uses_moon(&self) -> bool {
        self.predicates
            .values()
            .any(|condition| matches!(condition, WorldCondition::Moon { .. }))
    }

    pub fn matches(&self, condition: &Value, context: &CatSpawnContext<'_>) -> Result<bool> {
        let key = serde_json::to_string(condition)?;
        self.matches_key(&key, context)
    }

    pub(crate) fn matches_key(&self, key: &str, context: &CatSpawnContext<'_>) -> Result<bool> {
        let mut context = *context;
        self.matches_environment(key, &mut context)
    }

    pub(crate) fn matches_environment(
        &self,
        key: &str,
        context: &mut impl SpawnVariantEnvironment,
    ) -> Result<bool> {
        match self
            .predicates
            .get(key)
            .context("cat condition was not prepared")?
        {
            WorldCondition::Structures(values) => {
                Ok(!values.is_disjoint(context.structures_with_piece()?))
            }
            WorldCondition::Biomes(values) => Ok(values.contains(context.biome()?)),
            WorldCondition::Moon { min, max } => {
                const BRIGHTNESS: [f32; 8] = [1.0, 0.75, 0.5, 0.25, 0.0, 0.25, 0.5, 0.75];
                let phase = usize::from(context.moon_phase()?);
                let brightness = f64::from(*BRIGHTNESS.get(phase).context("invalid moon phase")?);
                Ok(min.is_none_or(|min| brightness >= min)
                    && max.is_none_or(|max| brightness <= max))
            }
        }
    }
}

fn resolve_selection(
    provider: &mut impl ResourceProvider,
    registry_name: &str,
    registry: &BTreeSet<ResourceId>,
    value: &Value,
    cache: &mut BTreeMap<ResourceId, Option<BTreeSet<ResourceId>>>,
) -> Result<BTreeSet<ResourceId>> {
    if let Some(tag) = value.as_str().and_then(|name| name.strip_prefix('#')) {
        return resolve_registry_tag(
            provider,
            registry_name,
            registry,
            &ResourceId::parse(tag)?,
            cache,
            &mut BTreeSet::new(),
        )?
        .context("missing cat condition tag");
    }
    let entries: Vec<&Value> = match value {
        Value::String(_) => vec![value],
        Value::Array(values) => values.iter().collect(),
        _ => bail!("invalid cat condition holder set"),
    };
    let mut result = BTreeSet::new();
    for entry in entries {
        let name = entry
            .as_str()
            .context("invalid cat condition registry ID")?;
        ensure!(
            !name.starts_with('#'),
            "tags are unsupported inside holder lists"
        );
        let id = ResourceId::parse(name)?;
        ensure!(
            registry.contains(&id),
            "cat condition references missing {registry_name} {name}"
        );
        result.insert(id);
    }
    Ok(result)
}

fn resolve_registry_tag(
    provider: &mut impl ResourceProvider,
    registry_name: &str,
    registry: &BTreeSet<ResourceId>,
    id: &ResourceId,
    cache: &mut BTreeMap<ResourceId, Option<BTreeSet<ResourceId>>>,
    visiting: &mut BTreeSet<ResourceId>,
) -> Result<Option<BTreeSet<ResourceId>>> {
    if let Some(value) = cache.get(id) {
        return Ok(value.clone());
    }
    ensure!(
        visiting.len() < 128 && cache.len() < 4096,
        "cat condition tag limit exceeded"
    );
    ensure!(visiting.insert(id.clone()), "cyclic cat condition tag");
    let result = (|| {
        let path = format!(
            "data/{}/tags/worldgen/{registry_name}/{}.json",
            id.namespace, id.path
        );
        let stack = provider.read_data_stack(&path)?;
        if stack.is_empty() {
            return Ok(None);
        }
        let mut entries = Vec::new();
        for bytes in stack {
            let (replace, values) =
                parse_tag(&bytes).with_context(|| format!("invalid cat condition tag {path}"))?;
            if replace {
                entries.clear();
            }
            entries.extend(values);
        }
        let mut values = BTreeSet::new();
        for entry in entries {
            match entry {
                TagEntry::Item { id, required } => {
                    let key = ResourceId::parse(&id)?;
                    if registry.contains(&key) {
                        values.insert(key);
                    } else if required {
                        bail!("missing required {registry_name} {id}");
                    }
                }
                TagEntry::Tag { id, required } => match resolve_registry_tag(
                    provider,
                    registry_name,
                    registry,
                    &id,
                    cache,
                    visiting,
                )? {
                    Some(child) => values.extend(child),
                    None if required => bail!("missing required nested {registry_name} tag"),
                    None => {}
                },
            }
        }
        Ok(Some(values))
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
    use crate::density::WorldgenRandom;
    use serde_json::json;

    struct Data;
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            if path == "data/minecraft/tags/worldgen/structure/cats_spawn_as_black.json" {
                return Ok(Some(
                    br##"{"values":["#test:hut_group",{"id":"test:missing","required":false}]}"##
                        .to_vec(),
                ));
            }
            if path == "data/test/tags/worldgen/structure/hut_group.json" {
                return Ok(Some(br#"{"values":["minecraft:swamp_hut"]}"#.to_vec()));
            }
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
                "worldgen/structure" => vec!["swamp_hut", "buried_treasure"],
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

    #[test]
    fn prepared_structure_and_moon_predicates_use_world_context() {
        let variants = CatSpawnVariants::load_active(&mut Data).unwrap();
        let conditions = variants.prepare_world_conditions(&mut Data).unwrap();
        let hut = ResourceId::parse("minecraft:swamp_hut").unwrap();
        let pieces = BTreeSet::from([hut]);
        let context = CatSpawnContext {
            biome: None,
            moon_phase: Some(4),
            structures_with_piece: Some(&pieces),
        };
        let mut random = RegionRandom::new(42, WorldgenRandom::Legacy, -13, 17);
        let result = variants
            .finalize_hut_cat(&mut random, |value| conditions.matches(value, &context))
            .unwrap();
        assert_eq!(
            result.variant,
            ResourceId::parse("minecraft:all_black").unwrap()
        );
        let moon = json!({"type":"minecraft:moon_brightness","range":{"min":0.9}});
        for phase in 0..8 {
            let context = CatSpawnContext {
                moon_phase: Some(phase),
                ..context
            };
            assert_eq!(conditions.matches(&moon, &context).unwrap(), phase == 0);
        }
        let missing = CatSpawnContext {
            structures_with_piece: None,
            ..context
        };
        let structure =
            json!({"type":"minecraft:structure","structures":"#minecraft:cats_spawn_as_black"});
        assert!(conditions.matches(&structure, &missing).is_err());
    }

    #[test]
    fn structure_tags_respect_pack_replacement_and_missing_optional_entries() {
        struct Layers;
        impl ResourceProvider for Layers {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data_stack(&mut self, path: &str) -> Result<Vec<Vec<u8>>> {
                Ok(match path {
                    "data/test/tags/worldgen/structure/selection.json" => vec![br#"{"values":["minecraft:buried_treasure"]}"#.to_vec(), br##"{"replace":true,"values":["#test:nested",{"id":"test:absent","required":false}]}"##.to_vec()],
                    "data/test/tags/worldgen/structure/nested.json" => vec![br#"{"values":["minecraft:swamp_hut"]}"#.to_vec()],
                    _ => vec![],
                })
            }
        }
        let hut = ResourceId::parse("minecraft:swamp_hut").unwrap();
        let treasure = ResourceId::parse("minecraft:buried_treasure").unwrap();
        let registry = BTreeSet::from([hut.clone(), treasure]);
        let values = resolve_selection(
            &mut Layers,
            "structure",
            &registry,
            &json!("#test:selection"),
            &mut BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(values, BTreeSet::from([hut]));
    }
}
