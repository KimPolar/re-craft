//! Active-data-pack biome tag used by every mineshaft piece.
use anyhow::{Context, Result, ensure};
use std::collections::{BTreeMap, BTreeSet};

use crate::{
    packs::{ResourceId, ResourceProvider},
    surface_biomes,
};

#[derive(Clone)]
pub struct MineshaftBlockingBiomes {
    registry: BTreeSet<ResourceId>,
    blocking: BTreeSet<ResourceId>,
}

impl MineshaftBlockingBiomes {
    pub fn load(provider: &mut impl ResourceProvider) -> Result<Self> {
        let registry: BTreeSet<_> = provider.list_data("worldgen/biome")?.into_iter().collect();
        ensure!(!registry.is_empty(), "biome registry is empty");
        let blocking = surface_biomes::resolve_tag(
            provider,
            &ResourceId::parse("minecraft:mineshaft_blocking")?,
            &registry,
            &mut BTreeMap::new(),
            &mut BTreeSet::new(),
        )
        .context("cannot resolve mineshaft-blocking biome tag")?
        .unwrap_or_default();
        Ok(Self { registry, blocking })
    }
    pub fn blocks(&self, biome: &ResourceId) -> Result<bool> {
        ensure!(
            self.registry.contains(biome),
            "biome is absent from the active registry"
        );
        Ok(self.blocking.contains(biome))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Data {
        root: Vec<Vec<u8>>,
        nested: Option<Vec<u8>>,
    }
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.read_data_stack(path)?.pop())
        }
        fn read_data_stack(&mut self, path: &str) -> Result<Vec<Vec<u8>>> {
            Ok(match path {
                "data/minecraft/tags/worldgen/biome/mineshaft_blocking.json" => self.root.clone(),
                "data/example/tags/worldgen/biome/nested.json" => {
                    self.nested.clone().into_iter().collect()
                }
                _ => vec![],
            })
        }
        fn list_data(&mut self, _: &str) -> Result<Vec<ResourceId>> {
            Ok(vec![
                ResourceId::parse("minecraft:plains")?,
                ResourceId::parse("example:custom")?,
            ])
        }
    }

    #[test]
    fn honors_pack_replacement_and_nested_tags() {
        let mut data = Data {
            root: vec![
                br#"{"values":["minecraft:plains"]}"#.to_vec(),
                br##"{"replace":true,"values":["#example:nested"]}"##.to_vec(),
            ],
            nested: Some(br#"{"values":["example:custom"]}"#.to_vec()),
        };
        let tags = MineshaftBlockingBiomes::load(&mut data).unwrap();
        assert!(
            !tags
                .blocks(&ResourceId::parse("minecraft:plains").unwrap())
                .unwrap()
        );
        assert!(
            tags.blocks(&ResourceId::parse("example:custom").unwrap())
                .unwrap()
        );
        assert!(
            tags.blocks(&ResourceId::parse("example:unknown").unwrap())
                .is_err()
        );
        data.nested = None;
        assert!(MineshaftBlockingBiomes::load(&mut data).is_err());
    }
}
