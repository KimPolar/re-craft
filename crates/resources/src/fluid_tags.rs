//! Resolve Java's fixed fluid registry tags through the active data-pack stack.

use crate::{
    packs::{ResourceId, ResourceProvider},
    recipes::{TagEntry, parse_tag},
};
use anyhow::{Context, Result, bail, ensure};
use std::collections::{BTreeMap, BTreeSet};

/// Resolve the active block tag used by flowing fluids when replacing blocks.
pub fn load_washed_away_blocks(
    provider: &mut impl ResourceProvider,
    registry: &BTreeSet<String>,
) -> Result<BTreeSet<String>> {
    let id = ResourceId::parse("minecraft:washed_away_by_fluids")?;
    let mut raw = BTreeMap::new();
    let mut failed = BTreeSet::new();
    crate::transforms::read_block_tag(provider, &id, &mut raw, &mut failed, 0)?;
    crate::heightmap_tags::resolve(
        &id,
        &raw,
        registry,
        &mut BTreeMap::new(),
        &mut BTreeSet::new(),
    )
    .context("cannot resolve blocks washed away by fluids")
}

/// Active tags consulted by water when deciding to schedule a bubble column.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BubbleColumnTags {
    drag_down: BTreeSet<String>,
    push_up: BTreeSet<String>,
    occupiable_fluids: BTreeSet<String>,
}

impl BubbleColumnTags {
    pub fn new(drag_down: BTreeSet<String>, push_up: BTreeSet<String>) -> Self {
        Self::with_fluids(
            drag_down,
            push_up,
            BTreeSet::from(["minecraft:water".into()]),
        )
    }

    pub fn with_fluids(
        drag_down: BTreeSet<String>,
        push_up: BTreeSet<String>,
        occupiable_fluids: BTreeSet<String>,
    ) -> Self {
        Self {
            drag_down,
            push_up,
            occupiable_fluids,
        }
    }

    pub fn load(provider: &mut impl ResourceProvider, registry: &BTreeSet<String>) -> Result<Self> {
        let down = ResourceId::parse("minecraft:enables_bubble_column_drag_down")?;
        let up = ResourceId::parse("minecraft:enables_bubble_column_push_up")?;
        let mut raw = BTreeMap::new();
        let mut failed = BTreeSet::new();
        for id in [&down, &up] {
            crate::transforms::read_block_tag(provider, id, &mut raw, &mut failed, 0)?;
        }
        let mut cache = BTreeMap::new();
        Ok(Self {
            drag_down: crate::heightmap_tags::resolve(
                &down,
                &raw,
                registry,
                &mut cache,
                &mut BTreeSet::new(),
            )
            .context("cannot resolve bubble-column drag-down tag")?,
            push_up: crate::heightmap_tags::resolve(
                &up,
                &raw,
                registry,
                &mut cache,
                &mut BTreeSet::new(),
            )
            .context("cannot resolve bubble-column push-up tag")?,
            occupiable_fluids: load_fluid_tag(provider, "minecraft:bubble_column_can_occupy")?,
        })
    }

    pub fn enables(&self, block: &str) -> bool {
        self.drag_down.contains(block) || self.push_up.contains(block)
    }

    pub fn pushes_up(&self, block: &str) -> bool {
        self.push_up.contains(block)
    }

    pub fn drags_down(&self, block: &str) -> bool {
        self.drag_down.contains(block)
    }

    pub fn can_occupy_fluid(&self, fluid: &str) -> bool {
        self.occupiable_fluids.contains(fluid)
    }

    pub fn water_can_occupy(&self) -> bool {
        self.can_occupy_fluid("minecraft:water")
    }
}

/// Fluids are a built-in registry in an unmodded 26.3 client; data packs may
/// change their tags, but cannot register additional fluid types.
const FLUIDS: [&str; 5] = [
    "minecraft:empty",
    "minecraft:water",
    "minecraft:flowing_water",
    "minecraft:lava",
    "minecraft:flowing_lava",
];

pub fn load_water(provider: &mut impl ResourceProvider) -> Result<BTreeSet<String>> {
    load_fluid_tag(provider, "minecraft:water")
}
pub fn load_lava(provider: &mut impl ResourceProvider) -> Result<BTreeSet<String>> {
    load_fluid_tag(provider, "minecraft:lava")
}

pub fn load_bubble_column_occupiable(
    provider: &mut impl ResourceProvider,
) -> Result<BTreeSet<String>> {
    load_fluid_tag(provider, "minecraft:bubble_column_can_occupy")
}

/// Active fluids that allow a sugar cane block to survive alongside them.
pub fn load_sugar_cane_adjacent(provider: &mut impl ResourceProvider) -> Result<BTreeSet<String>> {
    load_fluid_tag(provider, "minecraft:supports_sugar_cane_adjacently")
}

fn load_fluid_tag(provider: &mut impl ResourceProvider, name: &str) -> Result<BTreeSet<String>> {
    fn resolve(
        provider: &mut impl ResourceProvider,
        id: &ResourceId,
        cache: &mut BTreeMap<ResourceId, Option<BTreeSet<String>>>,
        visiting: &mut BTreeSet<ResourceId>,
    ) -> Result<Option<BTreeSet<String>>> {
        if let Some(value) = cache.get(id) {
            return Ok(value.clone());
        }
        ensure!(
            visiting.len() < 128 && cache.len() < 4096,
            "fluid tag nesting or count limit exceeded"
        );
        ensure!(visiting.insert(id.clone()), "cyclic fluid tag reference");
        let result = (|| {
            let path = format!("data/{}/tags/fluid/{}.json", id.namespace, id.path);
            let stack = provider.read_data_stack(&path)?;
            if stack.is_empty() {
                return Ok(None);
            }
            let mut entries = Vec::new();
            for bytes in stack {
                let (replace, layer) =
                    parse_tag(&bytes).with_context(|| format!("invalid fluid tag {path}"))?;
                if replace {
                    entries.clear();
                }
                entries.extend(layer);
            }
            let mut values = BTreeSet::new();
            for entry in entries {
                match entry {
                    TagEntry::Item { id, required } => {
                        if FLUIDS.contains(&id.as_str()) {
                            values.insert(id);
                        } else if required {
                            bail!("required fluid {id} is absent from the 26.3 registry");
                        }
                    }
                    TagEntry::Tag { id, required } => {
                        match resolve(provider, &id, cache, visiting)? {
                            Some(child) => values.extend(child),
                            None if required => {
                                bail!("required fluid tag {}:{} is missing", id.namespace, id.path)
                            }
                            None => {}
                        }
                    }
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
    resolve(
        provider,
        &ResourceId::parse(name)?,
        &mut BTreeMap::new(),
        &mut BTreeSet::new(),
    )?
    .with_context(|| format!("required {name} fluid tag is missing"))
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
    }
    #[test]
    fn resolves_replace_nested_optional_and_registry_checks() {
        let mut data = Data(BTreeMap::from([
            ("data/minecraft/tags/fluid/water.json".into(), vec![br#"{"values":["minecraft:water"]}"#.to_vec(), br##"{"replace":true,"values":["#example:hot",{"id":"#example:absent","required":false},{"id":"example:unknown","required":false}]}"##.to_vec()]),
            ("data/example/tags/fluid/hot.json".into(), vec![br#"{"values":["minecraft:lava","minecraft:flowing_lava"]}"#.to_vec()]),
        ]));
        assert_eq!(
            load_water(&mut data).unwrap(),
            BTreeSet::from(["minecraft:lava".into(), "minecraft:flowing_lava".into()])
        );
        data.0.insert(
            "data/example/tags/fluid/hot.json".into(),
            vec![br#"{"values":["example:unknown"]}"#.to_vec()],
        );
        assert!(load_water(&mut data).is_err());
        data.0.insert(
            "data/example/tags/fluid/hot.json".into(),
            vec![br##"{"values":["#minecraft:water"]}"##.to_vec()],
        );
        assert!(load_water(&mut data).is_err());
    }

    #[test]
    fn washed_away_blocks_follow_the_data_pack_stack() {
        let mut data = Data(BTreeMap::from([
            ("data/minecraft/tags/block/washed_away_by_fluids.json".into(), vec![
                br##"{"values":["minecraft:short_grass","#example:plants"]}"##.to_vec(),
                br##"{"replace":true,"values":["#example:plants",{"id":"minecraft:missing","required":false}]}"##.to_vec(),
            ]),
            ("data/example/tags/block/plants.json".into(), vec![br#"{"values":["minecraft:fern"]}"#.to_vec()]),
        ]));
        let registry = BTreeSet::from(["minecraft:short_grass".into(), "minecraft:fern".into()]);
        assert_eq!(
            load_washed_away_blocks(&mut data, &registry).unwrap(),
            BTreeSet::from(["minecraft:fern".into()])
        );
        data.0.insert(
            "data/example/tags/block/plants.json".into(),
            vec![br#"{"values":["minecraft:missing"]}"#.to_vec()],
        );
        assert!(load_washed_away_blocks(&mut data, &registry).is_err());
    }

    #[test]
    fn bubble_column_tags_follow_replacement_and_nested_entries() {
        let mut data = Data(BTreeMap::from([
            ("data/minecraft/tags/block/enables_bubble_column_drag_down.json".into(), vec![br#"{"values":["minecraft:magma_block"]}"#.to_vec(), br##"{"replace":true,"values":["#example:floor",{"id":"example:missing","required":false}]}"##.to_vec()]),
            ("data/minecraft/tags/block/enables_bubble_column_push_up.json".into(), vec![br#"{"values":["minecraft:soul_sand"]}"#.to_vec()]),
            ("data/example/tags/block/floor.json".into(), vec![br#"{"values":["minecraft:stone"]}"#.to_vec()]),
            ("data/minecraft/tags/fluid/bubble_column_can_occupy.json".into(), vec![br#"{"values":["minecraft:water"]}"#.to_vec(), br#"{"replace":true,"values":["minecraft:lava"]}"#.to_vec()]),
        ]));
        let registry = BTreeSet::from([
            "minecraft:magma_block".into(),
            "minecraft:soul_sand".into(),
            "minecraft:stone".into(),
        ]);
        let tags = BubbleColumnTags::load(&mut data, &registry).unwrap();
        assert!(tags.enables("minecraft:stone") && tags.enables("minecraft:soul_sand"));
        assert!(!tags.enables("minecraft:magma_block"));
        assert!(!tags.water_can_occupy());
        data.0.remove("data/example/tags/block/floor.json");
        assert!(BubbleColumnTags::load(&mut data, &registry).is_err());
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3"]
    fn installed_bubble_column_tags_match_vanilla_blocks() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let mut source = crate::packs::ResourceStack::open(&game).unwrap();
        let registry = source.vanilla_block_ids().unwrap();
        let tags = BubbleColumnTags::load(&mut source, &registry).unwrap();
        assert!(tags.enables("minecraft:magma_block") && tags.enables("minecraft:soul_sand"));
        assert!(!tags.enables("minecraft:stone"));
        assert!(tags.water_can_occupy());
    }
    #[test]
    #[ignore = "requires locally installed Minecraft 26.3"]
    fn installed_water_tag_contains_both_water_fluids() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let mut source = crate::packs::ResourceStack::open(&game).unwrap();
        assert_eq!(
            load_water(&mut source).unwrap(),
            BTreeSet::from(["minecraft:water".into(), "minecraft:flowing_water".into()])
        );
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3"]
    fn installed_washed_away_tag_identifies_plants_and_excludes_banners() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let mut source = crate::packs::ResourceStack::open(&game).unwrap();
        let registry = source.vanilla_block_ids().unwrap();
        let blocks = load_washed_away_blocks(&mut source, &registry).unwrap();
        for block in [
            "minecraft:short_grass",
            "minecraft:fern",
            "minecraft:dead_bush",
        ] {
            assert!(blocks.contains(block), "{block}");
        }
        assert!(!blocks.contains("minecraft:white_banner"));
        assert!(!blocks.contains("minecraft:white_wall_banner"));
    }
}
