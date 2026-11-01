//! Active block tag protecting blocks from carvers.
use crate::{
    heightmap_tags::resolve,
    packs::{ResourceId, ResourceProvider},
    transforms::read_block_tag,
};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub struct CarverBlockTags {
    registry: BTreeSet<String>,
    uncarvable: BTreeSet<String>,
}
impl CarverBlockTags {
    /// `registry` must contain all canonical block IDs for the active game.
    pub fn load(provider: &mut impl ResourceProvider, registry: &BTreeSet<String>) -> Result<Self> {
        ensure!(!registry.is_empty(), "block registry is empty");
        for name in registry {
            let id = ResourceId::parse(name)?;
            ensure!(
                *name == format!("{}:{}", id.namespace, id.path),
                "noncanonical block registry ID"
            );
        }
        let id = ResourceId::parse("minecraft:uncarvable")?;
        let mut raw = BTreeMap::new();
        let mut failed = BTreeSet::new();
        read_block_tag(provider, &id, &mut raw, &mut failed, 0)?;
        let uncarvable = resolve(
            &id,
            &raw,
            registry,
            &mut BTreeMap::new(),
            &mut BTreeSet::new(),
        )
        .context("cannot resolve uncarvable block tag")?;
        Ok(Self {
            registry: registry.clone(),
            uncarvable,
        })
    }
    pub fn uncarvable(&self, state: &Value) -> Result<bool> {
        let name = state
            .as_str()
            .or_else(|| state.get("id").and_then(Value::as_str))
            .context("block state has no identifier")?;
        let id = ResourceId::parse(name)?;
        let name = format!("{}:{}", id.namespace, id.path);
        ensure!(
            self.registry.contains(&name),
            "block {name} is absent from the registry"
        );
        Ok(self.uncarvable.contains(&name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Data(BTreeMap<String, Vec<Vec<u8>>>);
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data_stack(&mut self, p: &str) -> Result<Vec<Vec<u8>>> {
            Ok(self.0.get(p).cloned().unwrap_or_default())
        }
    }
    #[test]
    fn active_tag_overrides_and_missing_registry_entries() {
        let registry = ["minecraft:stone", "minecraft:bedrock", "minecraft:dirt"]
            .into_iter()
            .map(String::from)
            .collect();
        let mut data = Data::default();
        data.0.insert("data/minecraft/tags/block/uncarvable.json".into(),vec![br#"{"values":["minecraft:bedrock"]}"#.to_vec(),br##"{"replace":true,"values":["#example:protected",{"id":"example:optional","required":false}]}"##.to_vec()]);
        data.0.insert(
            "data/example/tags/block/protected.json".into(),
            vec![br#"{"values":["minecraft:dirt"]}"#.to_vec()],
        );
        let tags = CarverBlockTags::load(&mut data, &registry).unwrap();
        assert!(tags.uncarvable(&Value::String("dirt".into())).unwrap());
        assert!(
            !tags
                .uncarvable(&Value::String("minecraft:bedrock".into()))
                .unwrap()
        );
        assert!(
            tags.uncarvable(&Value::String("minecraft:missing".into()))
                .is_err()
        );
    }
}
