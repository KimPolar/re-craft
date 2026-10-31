//! Resolve material-rule biome holder sets against the world's active data packs.
use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use crate::{
    biome_climate::BiomeClimate,
    packs::{ResourceId, ResourceProvider},
    recipes::{TagEntry, parse_tag},
    surface_rules::SurfacePosition,
};

/// Preloaded registry values and selectors. The caller supplies the biome chosen
/// by the world's biome source; this type does not guess or generate it.
pub struct SurfaceBiomes {
    climates: BTreeMap<ResourceId, BiomeClimate>,
    selectors: HashMap<Value, BTreeSet<ResourceId>>,
}

impl SurfaceBiomes {
    pub fn load(provider: &mut impl ResourceProvider, selectors: &[Value]) -> Result<Self> {
        let registry: BTreeSet<_> = provider.list_data("worldgen/biome")?.into_iter().collect();
        let mut climates = BTreeMap::new();
        for id in &registry {
            let name = format!("{}:{}", id.namespace, id.path);
            let climate = BiomeClimate::load_world(provider, &name)
                .with_context(|| format!("invalid biome climate {name}"))?
                .with_context(|| format!("biome {name} is missing"))?;
            climates.insert(id.clone(), climate);
        }
        let mut resolved = HashMap::new();
        let mut cache = BTreeMap::new();
        for selector in selectors {
            let values = if let Some(tag) = selector.as_str().and_then(|s| s.strip_prefix('#')) {
                let id = ResourceId::parse(tag)?;
                resolve_tag(provider, &id, &registry, &mut cache, &mut BTreeSet::new())?
                    .with_context(|| format!("missing biome tag {tag}"))?
            } else {
                let values: Vec<&Value> = match selector {
                    Value::String(_) => vec![selector],
                    Value::Array(values) => values.iter().collect(),
                    _ => bail!("biome selector must be a biome ID, tag, or list of biome IDs"),
                };
                let mut ids = BTreeSet::new();
                for value in values {
                    let name = value
                        .as_str()
                        .context("biome selector entry must be a string")?;
                    ensure!(
                        !name.starts_with('#'),
                        "tags must be selected outside a biome list"
                    );
                    let id = ResourceId::parse(name)?;
                    ensure!(registry.contains(&id), "unknown biome {name}");
                    ids.insert(id);
                }
                ids
            };
            resolved.insert(selector.clone(), values);
        }
        Ok(Self {
            climates,
            selectors: resolved,
        })
    }

    pub fn contains_biome(&self, biome: &ResourceId) -> bool {
        self.climates.contains_key(biome)
    }

    pub fn matches(&self, selector: &Value, biome: &ResourceId) -> Result<bool> {
        ensure!(
            self.contains_biome(biome),
            "selected biome is not in the active registry"
        );
        Ok(self
            .selectors
            .get(selector)
            .context("biome selector was not preloaded")?
            .contains(biome))
    }

    pub fn cold_enough_to_snow(&self, biome: &ResourceId, p: &SurfacePosition) -> Result<bool> {
        let climate = self
            .climates
            .get(biome)
            .context("selected biome is not in the active registry")?;
        Ok(climate.cold_enough_to_snow(p.x, p.y, p.z, p.sea_level))
    }
}

fn resolve_tag(
    provider: &mut impl ResourceProvider,
    id: &ResourceId,
    registry: &BTreeSet<ResourceId>,
    cache: &mut BTreeMap<ResourceId, Option<BTreeSet<ResourceId>>>,
    visiting: &mut BTreeSet<ResourceId>,
) -> Result<Option<BTreeSet<ResourceId>>> {
    if let Some(values) = cache.get(id) {
        return Ok(values.clone());
    }
    ensure!(
        visiting.len() < 128 && cache.len() < 4096,
        "biome tag nesting or count limit exceeded"
    );
    ensure!(visiting.insert(id.clone()), "cyclic biome tag reference");
    let result = (|| {
        let path = format!("data/{}/tags/worldgen/biome/{}.json", id.namespace, id.path);
        let stack = provider.read_data_stack(&path)?;
        if stack.is_empty() {
            return Ok(None);
        }
        let mut entries = Vec::new();
        for layer in stack {
            let (replace, next) =
                parse_tag(&layer).with_context(|| format!("invalid biome tag {path}"))?;
            if replace {
                entries.clear();
            }
            entries.extend(next);
        }
        let mut values = BTreeSet::new();
        for entry in entries {
            match entry {
                TagEntry::Item { id, required } => {
                    let biome = ResourceId::parse(&id)?;
                    if registry.contains(&biome) {
                        values.insert(biome);
                    } else if required {
                        bail!("required biome {id} is missing");
                    }
                }
                TagEntry::Tag { id, required } => {
                    match resolve_tag(provider, &id, registry, cache, visiting)? {
                        Some(child) => values.extend(child),
                        None if required => {
                            bail!("required biome tag {}:{} is missing", id.namespace, id.path)
                        }
                        None => {}
                    }
                }
            }
        }
        Ok(Some(values))
    })();
    visiting.remove(id);
    if let Ok(values) = &result {
        cache.insert(id.clone(), values.clone());
    }
    result
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
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.0.get(path).and_then(|v| v.last()).cloned())
        }
        fn read_data_stack(&mut self, path: &str) -> Result<Vec<Vec<u8>>> {
            Ok(self.0.get(path).cloned().unwrap_or_default())
        }
        fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
            assert_eq!(folder, "worldgen/biome");
            Ok(vec![
                ResourceId::parse("minecraft:plains")?,
                ResourceId::parse("example:dry_cold")?,
            ])
        }
    }
    fn data() -> Data {
        Data(BTreeMap::from([
            ("data/minecraft/worldgen/biome/plains.json".into(), vec![br#"{"has_precipitation":true,"temperature":0.8}"#.to_vec()]),
            ("data/example/worldgen/biome/dry_cold.json".into(), vec![br#"{"has_precipitation":false,"temperature":0.0}"#.to_vec()]),
            ("data/example/tags/worldgen/biome/selected.json".into(), vec![br#"{"values":["minecraft:plains"]}"#.to_vec(), br##"{"replace":true,"values":["#example:nested",{"id":"#example:missing","required":false},{"id":"example:missing","required":false}]}"##.to_vec()]),
            ("data/example/tags/worldgen/biome/nested.json".into(), vec![br#"{"values":["example:dry_cold"]}"#.to_vec()]),
        ]))
    }
    #[test]
    fn resolves_layered_tags_and_rejects_broken_references() {
        let mut data = data();
        let selectors = vec![
            json!("#example:selected"),
            json!(["plains", "example:dry_cold"]),
        ];
        let loaded = SurfaceBiomes::load(&mut data, &selectors).unwrap();
        let cold = ResourceId::parse("example:dry_cold").unwrap();
        let plains = ResourceId::parse("plains").unwrap();
        assert!(loaded.matches(&selectors[0], &cold).unwrap());
        assert!(!loaded.matches(&selectors[0], &plains).unwrap());
        assert!(loaded.matches(&selectors[1], &plains).unwrap());
        assert!(loaded.matches(&json!("plains"), &plains).is_err());
        let p = SurfacePosition {
            x: 0,
            y: 63,
            z: 0,
            min_y: -64,
            height: 384,
            sea_level: 63,
            stone_above: 0,
            stone_below: 0,
            surface_depth: 0,
            surface_secondary: 0.0,
            water_height: 0,
            min_surface_level: 0,
            gradient_x: 0,
            gradient_z: 0,
        };
        assert!(loaded.cold_enough_to_snow(&cold, &p).unwrap());
        assert!(!loaded.cold_enough_to_snow(&plains, &p).unwrap());
        data.0.insert(
            "data/example/tags/worldgen/biome/nested.json".into(),
            vec![br##"{"values":["#example:selected"]}"##.to_vec()],
        );
        assert!(SurfaceBiomes::load(&mut data, &selectors).is_err());
        data.0.insert(
            "data/example/tags/worldgen/biome/nested.json".into(),
            vec![br#"{"values":["example:missing"]}"#.to_vec()],
        );
        assert!(SurfaceBiomes::load(&mut data, &selectors).is_err());
        assert!(SurfaceBiomes::load(&mut data, &[json!("#example:missing")]).is_err());
    }
}
