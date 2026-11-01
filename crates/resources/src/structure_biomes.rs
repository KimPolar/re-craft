//! Structure biome holder sets from the active world data packs.
use crate::{
    packs::{ResourceId, ResourceProvider},
    structure_placement::StructureEntry,
    surface_biomes::SurfaceBiomes,
};
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Resolve the biome lists/tags once per data-pack stack. This does not check
/// a structure's height, terrain, generation step or piece placement.
pub struct StructureBiomes {
    selectors: BTreeMap<ResourceId, Value>,
    biomes: SurfaceBiomes,
}
impl StructureBiomes {
    pub fn load(
        provider: &mut impl ResourceProvider,
        structures: impl IntoIterator<Item = ResourceId>,
    ) -> Result<Self> {
        let mut selectors = BTreeMap::new();
        for id in structures {
            if selectors.contains_key(&id) {
                continue;
            }
            let path = format!("data/{}/worldgen/structure/{}.json", id.namespace, id.path);
            let bytes = provider
                .read_data(&path)?
                .with_context(|| format!("missing structure {path}"))?;
            let value: Value = serde_json::from_slice(&bytes)
                .with_context(|| format!("invalid structure {path}"))?;
            let selector = value
                .get("biomes")
                .with_context(|| format!("structure {path} has no biomes"))?
                .clone();
            selectors.insert(id, selector);
        }
        let values: Vec<_> = selectors.values().cloned().collect();
        let biomes = SurfaceBiomes::load(provider, &values)?;
        Ok(Self { selectors, biomes })
    }
    pub fn permits(&self, structure: &ResourceId, biome: &ResourceId) -> Result<bool> {
        let selector = self
            .selectors
            .get(structure)
            .context("structure biome selector was not loaded")?;
        self.biomes.matches(selector, biome)
    }
    /// Java skips a set when none of its structures can occur in any biome
    /// returned by this dimension's biome source.
    pub fn set_has_possible_biome(
        &self,
        entries: &[StructureEntry],
        possible: &BTreeSet<ResourceId>,
    ) -> Result<bool> {
        for entry in entries {
            for biome in possible {
                if self.permits(&entry.id, biome)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
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
        fn read_data(&mut self, p: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.0.get(p).and_then(|v| v.last()).cloned())
        }
        fn read_data_stack(&mut self, p: &str) -> Result<Vec<Vec<u8>>> {
            Ok(self.0.get(p).cloned().unwrap_or_default())
        }
        fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
            anyhow::ensure!(folder == "worldgen/biome", "unexpected registry");
            ["example:a", "example:b"]
                .into_iter()
                .map(ResourceId::parse)
                .collect()
        }
    }
    #[test]
    fn data_pack_biome_tag_replacement_controls_structure_sets() {
        let mut data = Data::default();
        for biome in ["a", "b"] {
            data.0.insert(
                format!("data/example/worldgen/biome/{biome}.json"),
                vec![br#"{"has_precipitation":true,"temperature":0.8}"#.to_vec()],
            );
        }
        data.0.insert(
            "data/example/worldgen/structure/custom.json".into(),
            vec![serde_json::to_vec(&json!({"biomes":"#example:allowed"})).unwrap()],
        );
        data.0.insert("data/example/worldgen/structure_set/custom.json".into(),vec![serde_json::to_vec(&json!({"placement":{"type":"random_spread","spacing":1,"separation":0,"salt":0},"structures":[{"structure":"example:custom","weight":1}]})).unwrap()]);
        data.0.insert(
            "data/example/tags/worldgen/biome/allowed.json".into(),
            vec![
                br#"{"values":["example:a"]}"#.to_vec(),
                br#"{"replace":true,"values":["example:b"]}"#.to_vec(),
            ],
        );
        let id = ResourceId::parse("example:custom").unwrap();
        let rules = StructureBiomes::load(&mut data, [id.clone()]).unwrap();
        let a = ResourceId::parse("example:a").unwrap();
        let b = ResourceId::parse("example:b").unwrap();
        assert!(!rules.permits(&id, &a).unwrap());
        assert!(rules.permits(&id, &b).unwrap());
        let entries = [StructureEntry {
            id: id.clone(),
            weight: 1,
        }];
        let a_only = BTreeSet::from([a]);
        let b_only = BTreeSet::from([b]);
        assert!(!rules.set_has_possible_biome(&entries, &a_only).unwrap());
        assert!(rules.set_has_possible_biome(&entries, &b_only).unwrap());
        let mut cache = crate::structure_placement::StructureSetCache::default();
        assert!(
            !cache
                .is_structure_chunk_for_biomes(&mut data, &rules, &a_only, &id, 42, [0, 0])
                .unwrap()
        );
        assert!(
            cache
                .is_structure_chunk_for_biomes(&mut data, &rules, &b_only, &id, 42, [0, 0])
                .unwrap()
        );
        assert!(
            rules
                .permits(&ResourceId::parse("example:missing").unwrap(), &id)
                .is_err()
        );
    }
}
