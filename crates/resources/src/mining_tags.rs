//! Active 26.3 block tags used by tool components.

use crate::{
    heightmap_tags::resolve,
    packs::{ResourceId, ResourceProvider},
    transforms::read_block_tag,
};
use anyhow::{Context, Result, ensure};
use std::collections::{BTreeMap, BTreeSet};

pub const PICKAXE: u16 = 1 << 0;
pub const AXE: u16 = 1 << 1;
pub const SHOVEL: u16 = 1 << 2;
pub const HOE: u16 = 1 << 3;
pub const WOODEN_INCORRECT: u16 = 1 << 4;
pub const STONE_INCORRECT: u16 = 1 << 5;
pub const COPPER_INCORRECT: u16 = 1 << 6;
pub const IRON_INCORRECT: u16 = 1 << 7;
pub const GOLD_INCORRECT: u16 = 1 << 8;
pub const DIAMOND_INCORRECT: u16 = 1 << 9;
pub const NETHERITE_INCORRECT: u16 = 1 << 10;
pub const SWORD_INSTANT: u16 = 1 << 11;
pub const SWORD_EFFICIENT: u16 = 1 << 12;
pub const SHEARS_EXTREME: u16 = 1 << 13;
pub const SHEARS_MAJOR: u16 = 1 << 14;
pub const SHEARS_MINOR: u16 = 1 << 15;

const ROOTS: [(&str, u16); 16] = [
    ("mineable/pickaxe", PICKAXE),
    ("mineable/axe", AXE),
    ("mineable/shovel", SHOVEL),
    ("mineable/hoe", HOE),
    ("incorrect_for_wooden_tool", WOODEN_INCORRECT),
    ("incorrect_for_stone_tool", STONE_INCORRECT),
    ("incorrect_for_copper_tool", COPPER_INCORRECT),
    ("incorrect_for_iron_tool", IRON_INCORRECT),
    ("incorrect_for_gold_tool", GOLD_INCORRECT),
    ("incorrect_for_diamond_tool", DIAMOND_INCORRECT),
    ("incorrect_for_netherite_tool", NETHERITE_INCORRECT),
    ("sword_instantly_mines", SWORD_INSTANT),
    ("sword_efficient", SWORD_EFFICIENT),
    ("shears_extreme_breaking_speed", SHEARS_EXTREME),
    ("shears_major_breaking_speed", SHEARS_MAJOR),
    ("shears_minor_breaking_speed", SHEARS_MINOR),
];

#[derive(Clone, Debug)]
pub struct MiningTags {
    flags: BTreeMap<String, u16>,
    tags: BTreeMap<String, BTreeSet<String>>,
}

impl MiningTags {
    pub fn load(provider: &mut impl ResourceProvider, registry: &BTreeSet<String>) -> Result<Self> {
        ensure!(!registry.is_empty(), "block registry is empty");
        let roots = ROOTS
            .map(|(name, mask)| Ok((ResourceId::parse(&format!("minecraft:{name}"))?, mask)))
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        let mut raw = BTreeMap::new();
        let mut failed = BTreeSet::new();
        let available = provider.list_data("tags/block")?;
        for (id, _) in &roots {
            read_block_tag(provider, id, &mut raw, &mut failed, 0)?;
        }
        for id in &available {
            read_block_tag(provider, id, &mut raw, &mut failed, 0)?;
        }
        let mut cache = BTreeMap::new();
        let mut flags: BTreeMap<_, _> = registry.iter().map(|name| (name.clone(), 0)).collect();
        for (id, mask) in roots {
            let values = resolve(&id, &raw, registry, &mut cache, &mut BTreeSet::new())
                .with_context(|| {
                    format!(
                        "cannot resolve mining block tag {}:{}",
                        id.namespace, id.path
                    )
                })?;
            for name in values {
                *flags
                    .get_mut(&name)
                    .expect("resolved block belongs to registry") |= mask;
            }
        }
        // A malformed unrelated tag must not prevent the default tools from
        // loading. References to that tag remain unresolved for custom tools.
        for id in available {
            let _ = resolve(&id, &raw, registry, &mut cache, &mut BTreeSet::new());
        }
        let tags = cache
            .into_iter()
            .map(|(id, values)| (format!("{}:{}", id.namespace, id.path), values))
            .collect();
        Ok(Self { flags, tags })
    }

    pub fn for_block(&self, block: &str) -> Option<u16> {
        self.flags.get(block).copied()
    }

    /// `None` means the tag or block could not be resolved in this world.
    pub fn contains(&self, tag: &str, block: &str) -> Option<bool> {
        self.flags.get(block)?;
        Some(self.tags.get(tag)?.contains(block))
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
        fn read_data_stack(&mut self, path: &str) -> Result<Vec<Vec<u8>>> {
            Ok(self.0.get(path).cloned().unwrap_or_default())
        }
        fn list_data(&mut self, _: &str) -> Result<Vec<ResourceId>> {
            Ok(self
                .0
                .keys()
                .filter_map(|path| {
                    let path = path.strip_prefix("data/")?;
                    let (namespace, path) = path.split_once("/tags/block/")?;
                    let path = path.strip_suffix(".json")?;
                    ResourceId::parse(&format!("{namespace}:{path}")).ok()
                })
                .collect())
        }
    }
    #[test]
    fn layered_nested_tags_respect_replacement_and_registry() {
        let mut data = Data::default();
        for (name, _) in ROOTS {
            data.0.insert(
                format!("data/minecraft/tags/block/{name}.json"),
                vec![br#"{"values":[]}"#.to_vec()],
            );
        }
        data.0.insert(
            "data/minecraft/tags/block/mineable/pickaxe.json".into(),
            vec![
                br#"{"values":["minecraft:stone"]}"#.to_vec(),
                br##"{"replace":true,"values":["#example:group"]}"##.to_vec(),
            ],
        );
        data.0.insert(
            "data/example/tags/block/group.json".into(),
            vec![
                br#"{"values":["minecraft:oak_stairs",{"id":"example:absent","required":false}]}"#
                    .to_vec(),
            ],
        );
        let registry = BTreeSet::from(["minecraft:stone".into(), "minecraft:oak_stairs".into()]);
        let tags = MiningTags::load(&mut data, &registry).unwrap();
        assert_eq!(tags.for_block("minecraft:stone"), Some(0));
        assert_eq!(tags.for_block("minecraft:oak_stairs"), Some(PICKAXE));
        assert_eq!(tags.for_block("example:absent"), None);
        assert_eq!(
            tags.contains("example:group", "minecraft:oak_stairs"),
            Some(true)
        );
        assert_eq!(
            tags.contains("example:group", "minecraft:stone"),
            Some(false)
        );
        assert_eq!(tags.contains("example:absent", "minecraft:stone"), None);
        assert_eq!(tags.contains("example:group", "example:absent"), None);
    }

    #[test]
    fn missing_or_invalid_required_tags_are_rejected() {
        let mut data = Data::default();
        for (name, _) in ROOTS {
            data.0.insert(
                format!("data/minecraft/tags/block/{name}.json"),
                vec![br#"{"values":[]}"#.to_vec()],
            );
        }
        let registry = BTreeSet::from(["minecraft:stone".into()]);
        data.0.remove("data/minecraft/tags/block/mineable/axe.json");
        assert!(MiningTags::load(&mut data, &registry).is_err());
        data.0.insert(
            "data/minecraft/tags/block/mineable/axe.json".into(),
            vec![br#"{"values":["example:missing"]}"#.to_vec()],
        );
        assert!(MiningTags::load(&mut data, &registry).is_err());
    }
}
