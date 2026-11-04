//! Ordered world-preset tags used by the Java create-world screen.
use std::collections::{BTreeMap, BTreeSet};

use crate::{
    packs::{ResourceId, ResourceProvider},
    recipes::{TagEntry, parse_tag},
};
use anyhow::{Context, Result, bail, ensure};

#[derive(Clone, Debug)]
pub struct WorldPresets {
    pub normal: Vec<ResourceId>,
    pub alternate: Vec<ResourceId>,
}

impl WorldPresets {
    /// Resolve the tag layers from the selected data-pack stack. The first
    /// occurrence of a preset wins, including across nested tag references.
    pub fn load(provider: &mut impl ResourceProvider) -> Result<Self> {
        let registry: BTreeSet<_> = provider
            .list_data("worldgen/world_preset")?
            .into_iter()
            .collect();
        ensure!(!registry.is_empty(), "world preset registry is empty");
        let mut cache = BTreeMap::new();
        let normal = resolve(
            provider,
            &ResourceId::parse("minecraft:normal")?,
            &registry,
            &mut cache,
            &mut BTreeSet::new(),
        )?
        .filter(|list| !list.is_empty())
        .unwrap_or_else(|| registry.iter().cloned().collect());
        let alternate = resolve(
            provider,
            &ResourceId::parse("minecraft:extended")?,
            &registry,
            &mut cache,
            &mut BTreeSet::new(),
        )?
        .filter(|list| !list.is_empty())
        .unwrap_or_else(|| normal.clone());
        Ok(Self { normal, alternate })
    }
}

fn resolve(
    provider: &mut impl ResourceProvider,
    id: &ResourceId,
    registry: &BTreeSet<ResourceId>,
    cache: &mut BTreeMap<ResourceId, Option<Vec<ResourceId>>>,
    visiting: &mut BTreeSet<ResourceId>,
) -> Result<Option<Vec<ResourceId>>> {
    if let Some(result) = cache.get(id) {
        return Ok(result.clone());
    }
    ensure!(
        visiting.len() < 128 && cache.len() < 4096,
        "world preset tag nesting limit exceeded"
    );
    ensure!(visiting.insert(id.clone()), "cyclic world preset tag");
    let result = (|| {
        let path = format!(
            "data/{}/tags/worldgen/world_preset/{}.json",
            id.namespace, id.path
        );
        let stack = provider.read_data_stack(&path)?;
        if stack.is_empty() {
            return Ok(None);
        }
        let mut entries = Vec::new();
        for bytes in stack {
            let (replace, layer) =
                parse_tag(&bytes).with_context(|| format!("invalid preset tag {path}"))?;
            if replace {
                entries.clear();
            }
            entries.extend(layer);
        }
        let mut seen = BTreeSet::new();
        let mut values = Vec::new();
        for entry in entries {
            let children = match entry {
                TagEntry::Item { id, required } => {
                    let item = ResourceId::parse(&id)?;
                    if registry.contains(&item) {
                        vec![item]
                    } else if required {
                        bail!("required world preset {id} is missing")
                    } else {
                        vec![]
                    }
                }
                TagEntry::Tag { id, required } => {
                    match resolve(provider, &id, registry, cache, visiting)? {
                        Some(values) => values,
                        None if required => bail!(
                            "required world preset tag {}:{} is missing",
                            id.namespace,
                            id.path
                        ),
                        None => vec![],
                    }
                }
            };
            for child in children {
                if seen.insert(child.clone()) {
                    values.push(child);
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
    #[derive(Default)]
    struct Fixture(BTreeMap<String, Vec<Vec<u8>>>);
    impl ResourceProvider for Fixture {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data_stack(&mut self, path: &str) -> Result<Vec<Vec<u8>>> {
            Ok(self.0.get(path).cloned().unwrap_or_default())
        }
        fn list_data(&mut self, _: &str) -> Result<Vec<ResourceId>> {
            ["minecraft:normal", "minecraft:flat", "example:custom"]
                .into_iter()
                .map(ResourceId::parse)
                .collect()
        }
    }
    #[test]
    fn layered_nested_tags_preserve_order_and_deduplicate() {
        let mut source = Fixture::default();
        source.0.insert("data/minecraft/tags/worldgen/world_preset/normal.json".into(), vec![br##"{"values":["minecraft:flat","#example:extra"]}"##.to_vec(), br#"{"values":["minecraft:flat","minecraft:normal",{"id":"missing:optional","required":false}]}"#.to_vec()]);
        source.0.insert(
            "data/example/tags/worldgen/world_preset/extra.json".into(),
            vec![br#"{"values":["example:custom","minecraft:flat"]}"#.to_vec()],
        );
        let lists = WorldPresets::load(&mut source).unwrap();
        let expected: Vec<_> = ["minecraft:flat", "example:custom", "minecraft:normal"]
            .into_iter()
            .map(|v| ResourceId::parse(v).unwrap())
            .collect();
        assert_eq!(lists.normal, expected);
        assert_eq!(lists.alternate, expected);
        source
            .0
            .get_mut("data/minecraft/tags/worldgen/world_preset/normal.json")
            .unwrap()
            .push(br#"{"replace":true,"values":["minecraft:normal"]}"#.to_vec());
        assert_eq!(
            WorldPresets::load(&mut source).unwrap().normal,
            vec![ResourceId::parse("minecraft:normal").unwrap()]
        );
        source
            .0
            .get_mut("data/example/tags/worldgen/world_preset/extra.json")
            .unwrap()
            .push(br##"{"replace":true,"values":["#minecraft:normal"]}"##.to_vec());
        source
            .0
            .get_mut("data/minecraft/tags/worldgen/world_preset/normal.json")
            .unwrap()
            .pop();
        assert!(WorldPresets::load(&mut source).is_err());
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn installed_preset_tags_preserve_java_screen_order() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let mut provider = crate::packs::ResourceStack::open(&game).unwrap();
        let presets = WorldPresets::load(&mut provider).unwrap();
        let expected: Vec<_> = [
            "normal",
            "flat",
            "large_biomes",
            "amplified",
            "single_biome_surface",
        ]
        .into_iter()
        .map(|name| ResourceId::parse(name).unwrap())
        .collect();
        assert_eq!(presets.normal, expected);
        let mut extended = expected;
        extended.push(ResourceId::parse("debug_all_block_states").unwrap());
        assert_eq!(presets.alternate, extended);
    }
}
