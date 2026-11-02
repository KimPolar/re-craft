//! Biome feature lists and Java 26.3's global decoration feature ordering.
//!
//! Ordering uses placed-feature object identity, not structural equality. The
//! caller must supply the generator's possible biomes in Java iteration order
//! and a complete, validated placed-feature registry for the active packs.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use crate::{
    packs::{ResourceId, ResourceProvider},
    recipes::{TagEntry, parse_tag},
};

/// A registered object or a distinct inline object owned by one biome entry.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum FeatureKey {
    Registered(ResourceId),
    Inline {
        biome: ResourceId,
        step: usize,
        index: usize,
    },
}

/// Feature identities in the holder-set order of one biome.
pub struct BiomeFeatures {
    pub steps: Vec<Vec<FeatureKey>>,
    /// Definitions still require full configured/placed-feature validation.
    pub inline: BTreeMap<FeatureKey, Value>,
}

/// Global feature order and the biome holder lists that contributed to it.
pub struct FeatureSchedule {
    steps: Vec<Vec<FeatureKey>>,
    biomes: BTreeMap<ResourceId, BiomeFeatures>,
    possible_biomes: Vec<ResourceId>,
    indices: BTreeMap<ResourceId, Vec<Vec<usize>>>,
    membership: BTreeMap<ResourceId, BTreeSet<FeatureKey>>,
}

/// A selected feature retains its global index even when earlier features are
/// absent from the chunk's neighboring biomes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SelectedFeature<'a> {
    pub step: i32,
    pub index: i32,
    pub feature: &'a FeatureKey,
}

impl FeatureSchedule {
    /// `possible_biomes` must preserve Java's generator iteration order; using
    /// a sorted set here changes feature indices and therefore worldgen seeds.
    pub fn load(
        provider: &mut impl ResourceProvider,
        possible_biomes: &[ResourceId],
        registry: &BTreeSet<ResourceId>,
    ) -> Result<Self> {
        let mut tags = FeatureTags::default();
        let mut biomes = BTreeMap::new();
        let mut lists = Vec::with_capacity(possible_biomes.len());
        for id in possible_biomes {
            ensure!(!biomes.contains_key(id), "duplicate possible biome");
            let biome = BiomeFeatures::load(provider, id, registry, &mut tags)?;
            lists.push(biome.steps.clone());
            biomes.insert(id.clone(), biome);
        }
        let steps = order_features(&lists)?;
        ensure!(
            i32::try_from(steps.len()).is_ok(),
            "too many decoration steps"
        );
        let mut lookup = Vec::with_capacity(steps.len());
        for step in &steps {
            ensure!(
                i32::try_from(step.len()).is_ok(),
                "too many features in a decoration step"
            );
            lookup.push(
                step.iter()
                    .enumerate()
                    .map(|(i, key)| (key, i))
                    .collect::<BTreeMap<_, _>>(),
            );
        }
        let mut indices = BTreeMap::new();
        let mut membership = BTreeMap::new();
        for (id, biome) in &biomes {
            let mut features = Vec::with_capacity(biome.steps.len());
            for (step, keys) in biome.steps.iter().enumerate() {
                let mut values = Vec::with_capacity(keys.len());
                for key in keys {
                    values.push(
                        *lookup[step]
                            .get(key)
                            .context("feature missing from global schedule")?,
                    );
                }
                features.push(values);
            }
            indices.insert(id.clone(), features);
            membership.insert(id.clone(), biome.steps.iter().flatten().cloned().collect());
        }
        Ok(Self {
            steps,
            biomes,
            possible_biomes: possible_biomes.to_vec(),
            indices,
            membership,
        })
    }

    pub fn steps(&self) -> &[Vec<FeatureKey>] {
        &self.steps
    }
    pub fn biome(&self, id: &ResourceId) -> Option<&BiomeFeatures> {
        self.biomes.get(id)
    }
    pub fn possible_biomes(&self) -> &[ResourceId] {
        &self.possible_biomes
    }
    /// Identity lookup only; Java's `hasFeature` can also match a distinct
    /// structurally equal placed feature. Callers must account for that case.
    pub fn biome_contains_identity(
        &self,
        biome: &ResourceId,
        feature: &FeatureKey,
    ) -> Result<bool> {
        Ok(self
            .membership
            .get(biome)
            .context("biome is absent from the feature schedule")?
            .contains(feature))
    }

    /// Union features from present possible biomes, in global step/index order.
    /// Other biomes are ignored exactly as Java's `retainAll(possibleBiomes)`.
    pub fn select<'a, 'b>(
        &'a self,
        present: impl IntoIterator<Item = &'b ResourceId>,
    ) -> Vec<SelectedFeature<'a>> {
        let mut selected: Vec<Vec<bool>> = self
            .steps
            .iter()
            .map(|step| vec![false; step.len()])
            .collect();
        for biome in present {
            if let Some(steps) = self.indices.get(biome) {
                for (step, indices) in steps.iter().enumerate() {
                    for &index in indices {
                        selected[step][index] = true;
                    }
                }
            }
        }
        let mut result = Vec::new();
        for (step, flags) in selected.iter().enumerate() {
            for (index, &flag) in flags.iter().enumerate() {
                if flag {
                    result.push(SelectedFeature {
                        step: step as i32,
                        index: index as i32,
                        feature: &self.steps[step][index],
                    });
                }
            }
        }
        result
    }
}

/// Tag cache for one immutable active data-pack stack and registry.
#[derive(Default)]
pub struct FeatureTags {
    cache: BTreeMap<ResourceId, Option<Vec<ResourceId>>>,
}

impl FeatureTags {
    fn resolve(
        &mut self,
        provider: &mut impl ResourceProvider,
        registry: &BTreeSet<ResourceId>,
        id: &ResourceId,
        visiting: &mut BTreeSet<ResourceId>,
    ) -> Result<Option<Vec<ResourceId>>> {
        if let Some(value) = self.cache.get(id) {
            return Ok(value.clone());
        }
        ensure!(
            visiting.len() < 128,
            "placed-feature tag nesting is too deep"
        );
        ensure!(visiting.insert(id.clone()), "cyclic placed-feature tag");
        let result = (|| {
            let path = format!(
                "data/{}/tags/worldgen/placed_feature/{}.json",
                id.namespace, id.path
            );
            let stack = provider.read_data_stack(&path)?;
            if stack.is_empty() {
                return Ok(None);
            }
            let mut entries = Vec::new();
            for bytes in stack {
                let (replace, values) = parse_tag(&bytes)
                    .with_context(|| format!("invalid placed-feature tag {path}"))?;
                if replace {
                    entries.clear();
                }
                entries.extend(values);
            }
            let mut seen = BTreeSet::new();
            let mut output = Vec::new();
            for entry in entries {
                match entry {
                    TagEntry::Item { id: name, required } => {
                        let name = ResourceId::parse(&name)?;
                        if registry.contains(&name) {
                            if seen.insert(name.clone()) {
                                output.push(name);
                            }
                        } else if required {
                            bail!(
                                "required placed feature {}:{} is missing",
                                name.namespace,
                                name.path
                            );
                        }
                    }
                    TagEntry::Tag {
                        id: nested,
                        required,
                    } => match self.resolve(provider, registry, &nested, visiting)? {
                        Some(items) => {
                            for item in items {
                                if seen.insert(item.clone()) {
                                    output.push(item);
                                }
                            }
                        }
                        None if required => bail!(
                            "required placed-feature tag {}:{} is missing",
                            nested.namespace,
                            nested.path
                        ),
                        None => {}
                    },
                }
            }
            Ok(Some(output))
        })();
        visiting.remove(id);
        if let Ok(value) = &result {
            self.cache.insert(id.clone(), value.clone());
        }
        result
    }
}

impl BiomeFeatures {
    /// Parse biome holder sets. The caller supplies the authoritative registry;
    /// inline objects remain opaque until their feature type is validated.
    pub fn load(
        provider: &mut impl ResourceProvider,
        biome: &ResourceId,
        registry: &BTreeSet<ResourceId>,
        tags: &mut FeatureTags,
    ) -> Result<Self> {
        let path = format!(
            "data/{}/worldgen/biome/{}.json",
            biome.namespace, biome.path
        );
        let bytes = provider
            .read_data(&path)?
            .with_context(|| format!("missing biome {path}"))?;
        let value: Value =
            serde_json::from_slice(&bytes).with_context(|| format!("invalid biome {path}"))?;
        let lists = value
            .get("features")
            .and_then(Value::as_array)
            .context("biome features must be a list")?;
        let mut steps = Vec::with_capacity(lists.len());
        let mut inline = BTreeMap::new();
        for (step, list) in lists.iter().enumerate() {
            let mut result = Vec::new();
            if let Some(tag) = list.as_str().and_then(|s| s.strip_prefix('#')) {
                let id = ResourceId::parse(tag)?;
                let values = tags
                    .resolve(provider, registry, &id, &mut BTreeSet::new())?
                    .context("missing placed-feature tag")?;
                result.extend(values.into_iter().map(FeatureKey::Registered));
            } else {
                let values: Vec<&Value> = if let Some(array) = list.as_array() {
                    array.iter().collect()
                } else {
                    vec![list]
                };
                for (index, value) in values.into_iter().enumerate() {
                    if let Some(name) = value.as_str() {
                        ensure!(
                            !name.starts_with('#'),
                            "tags cannot be embedded in a feature holder list"
                        );
                        let id = ResourceId::parse(name)?;
                        ensure!(
                            registry.contains(&id),
                            "biome references a missing placed feature {name}"
                        );
                        result.push(FeatureKey::Registered(id));
                    } else if value.is_object() {
                        let key = FeatureKey::Inline {
                            biome: biome.clone(),
                            step,
                            index,
                        };
                        inline.insert(key.clone(), value.clone());
                        result.push(key);
                    } else {
                        bail!("invalid placed-feature holder");
                    }
                }
            }
            steps.push(result);
        }
        Ok(Self { steps, inline })
    }
}

/// Match `FeatureSorter.buildFeaturesPerStep`, including first-seen reference
/// indices, edges across steps, DFS ordering, empty steps and cycle rejection.
/// Equal keys must represent the same Java object, not merely equal values.
pub fn order_features<T: Ord + Clone>(biomes: &[Vec<Vec<T>>]) -> Result<Vec<Vec<T>>> {
    let mut ids = BTreeMap::<T, usize>::new();
    let mut values = Vec::<T>::new();
    let mut graph = BTreeMap::<(usize, usize), BTreeSet<(usize, usize)>>::new();
    let mut step_count = 0;
    for biome in biomes {
        step_count = step_count.max(biome.len());
        let mut previous = None;
        for (step, features) in biome.iter().enumerate() {
            for feature in features {
                let id = if let Some(id) = ids.get(feature) {
                    *id
                } else {
                    let id = values.len();
                    ids.insert(feature.clone(), id);
                    values.push(feature.clone());
                    id
                };
                let node = (step, id);
                graph.entry(node).or_default();
                if let Some(prev) = previous {
                    graph.get_mut(&prev).unwrap().insert(node);
                }
                previous = Some(node);
            }
        }
    }
    let mut marks = BTreeMap::<(usize, usize), u8>::new();
    let mut finished = Vec::with_capacity(graph.len());
    for root in graph.keys().copied() {
        if marks.get(&root) == Some(&2) {
            continue;
        }
        let mut stack = vec![(root, false)];
        while let Some((node, exit)) = stack.pop() {
            if exit {
                marks.insert(node, 2);
                finished.push(node);
            } else {
                match marks.get(&node) {
                    Some(2) => continue,
                    Some(1) => bail!("feature order cycle found"),
                    _ => {}
                }
                marks.insert(node, 1);
                stack.push((node, true));
                for next in graph[&node].iter().rev() {
                    stack.push((*next, false));
                }
            }
        }
    }
    let mut output = vec![Vec::new(); step_count];
    for (step, id) in finished.into_iter().rev() {
        output[step].push(values[id].clone());
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Default)]
    struct Data(BTreeMap<String, Vec<Vec<u8>>>);
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data_stack(&mut self, path: &str) -> Result<Vec<Vec<u8>>> {
            Ok(self.0.get(path).cloned().unwrap_or_default())
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.0.get(path).and_then(|stack| stack.last()).cloned())
        }
    }
    fn id(s: &str) -> ResourceId {
        ResourceId::parse(s).unwrap()
    }
    fn insert(data: &mut Data, path: &str, values: &[Value]) {
        data.0.insert(
            path.into(),
            values
                .iter()
                .map(|v| serde_json::to_vec(v).unwrap())
                .collect(),
        );
    }

    #[test]
    fn pack_tag_resolution_and_inline_identity() {
        let mut data = Data::default();
        insert(
            &mut data,
            "data/test/worldgen/biome/a.json",
            &[
                json!({"features":["#test:ores",["test:a",{"feature":"test:ore","placement":[]},{"feature":"test:ore","placement":[]}],[],"test:b"]}),
            ],
        );
        insert(
            &mut data,
            "data/test/tags/worldgen/placed_feature/ores.json",
            &[
                json!({"values":["test:a"]}),
                json!({"replace":true,"values":["#test:nested","test:a","test:a",{"id":"test:absent","required":false}]}),
            ],
        );
        insert(
            &mut data,
            "data/test/tags/worldgen/placed_feature/nested.json",
            &[json!({"values":["test:b"]})],
        );
        let registry = [id("test:a"), id("test:b")].into_iter().collect();
        let features = BiomeFeatures::load(
            &mut data,
            &id("test:a"),
            &registry,
            &mut FeatureTags::default(),
        )
        .unwrap();
        assert_eq!(
            features.steps[0],
            vec![
                FeatureKey::Registered(id("test:b")),
                FeatureKey::Registered(id("test:a"))
            ]
        );
        assert_eq!(features.steps.len(), 4);
        assert_ne!(features.steps[1][1], features.steps[1][2]);
        assert_eq!(features.inline.len(), 2);
        assert_eq!(
            features.inline[&features.steps[1][1]],
            features.inline[&features.steps[1][2]]
        );
    }

    #[test]
    fn rejects_unknown_and_recursive_tags_and_missing_feature() {
        let mut data = Data::default();
        insert(
            &mut data,
            "data/test/worldgen/biome/a.json",
            &[json!({"features":["#test:loop"]})],
        );
        insert(
            &mut data,
            "data/test/tags/worldgen/placed_feature/loop.json",
            &[json!({"values":["#test:loop"]})],
        );
        let registry = BTreeSet::new();
        assert!(
            BiomeFeatures::load(
                &mut data,
                &id("test:a"),
                &registry,
                &mut FeatureTags::default()
            )
            .is_err()
        );
        insert(
            &mut data,
            "data/test/worldgen/biome/a.json",
            &[json!({"features":[["test:missing"]]})],
        );
        assert!(
            BiomeFeatures::load(
                &mut data,
                &id("test:a"),
                &registry,
                &mut FeatureTags::default()
            )
            .is_err()
        );
        insert(
            &mut data,
            "data/test/worldgen/biome/a.json",
            &[json!({"features":[["#test:loop"]]})],
        );
        assert!(
            BiomeFeatures::load(
                &mut data,
                &id("test:a"),
                &registry,
                &mut FeatureTags::default()
            )
            .is_err()
        );
    }

    #[test]
    fn sorts_constraints_and_rejects_cycles() {
        let biomes = vec![
            vec![vec!['a', 'b'], vec!['c']],
            vec![vec!['a', 'd', 'b'], vec!['c', 'e']],
        ];
        assert_eq!(
            order_features(&biomes).unwrap(),
            vec![vec!['a', 'd', 'b'], vec!['c', 'e']]
        );
        assert!(order_features(&[vec![vec!['a', 'b']], vec![vec!['b', 'a']]]).is_err());
        assert!(order_features(&[vec![vec!['a', 'a']]]).is_err());
        assert_eq!(
            order_features::<char>(&[vec![vec![], vec![]]]).unwrap(),
            vec![Vec::<char>::new(), Vec::new()]
        );
    }

    #[test]
    fn selection_uses_global_indices_and_ignores_absent_biomes() {
        let mut data = Data::default();
        insert(
            &mut data,
            "data/test/worldgen/biome/a.json",
            &[json!({"features":[["test:a","test:b"],["test:c"]]})],
        );
        insert(
            &mut data,
            "data/test/worldgen/biome/b.json",
            &[json!({"features":[["test:d","test:b"],["test:e"]]})],
        );
        let registry = ["test:a", "test:b", "test:c", "test:d", "test:e"]
            .into_iter()
            .map(id)
            .collect();
        let a = id("test:a");
        let b = id("test:b");
        let missing = id("test:missing");
        let schedule =
            FeatureSchedule::load(&mut data, &[a.clone(), b.clone()], &registry).unwrap();
        let actual = schedule.select([&b, &missing, &b]);
        let expected: Vec<_> = schedule
            .steps()
            .iter()
            .enumerate()
            .flat_map(|(step, list)| {
                list.iter().enumerate().filter_map(move |(index, feature)| {
                    let wanted = ["test:b", "test:d", "test:e"]
                        .iter()
                        .any(|name| *feature == FeatureKey::Registered(id(name)));
                    wanted.then_some((step as i32, index as i32, feature))
                })
            })
            .collect();
        assert_eq!(
            actual
                .iter()
                .map(|v| (v.step, v.index, v.feature))
                .collect::<Vec<_>>(),
            expected
        );
        assert!(schedule.select([&missing]).is_empty());
    }
}
