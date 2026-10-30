//! Block tags and player-danger rules used by the 26.3 respawn search.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};

use crate::{
    heightmap_tags::resolve,
    packs::{ResourceId, ResourceProvider},
    transforms::read_block_tag,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RespawnBlockFacts {
    pub dangerous: bool,
    pub climbable: bool,
    pub invalid_inside: bool,
}

#[derive(Clone, Debug)]
pub struct RespawnBlockTags {
    registry: BTreeSet<String>,
    climbable: BTreeSet<String>,
    invalid_inside: BTreeSet<String>,
    immune: BTreeSet<String>,
    fire: BTreeSet<String>,
}

/// Resolve the active fire tag for falling-block support checks.
pub fn load_fire_tag(
    provider: &mut impl ResourceProvider,
    registry: &BTreeSet<String>,
) -> Result<BTreeSet<String>> {
    let id = ResourceId::parse("minecraft:fire")?;
    let mut raw = BTreeMap::new();
    let mut failed = BTreeSet::new();
    read_block_tag(provider, &id, &mut raw, &mut failed, 0)?;
    resolve(
        &id,
        &raw,
        registry,
        &mut BTreeMap::new(),
        &mut BTreeSet::new(),
    )
    .context("cannot resolve falling-block fire tag")
}

/// Resolve the active block tag that disables restitution on contact.
pub fn load_bounce_suppression_tag(
    provider: &mut impl ResourceProvider,
    registry: &BTreeSet<String>,
) -> Result<BTreeSet<String>> {
    let id = ResourceId::parse("minecraft:suppresses_bounce")?;
    let mut raw = BTreeMap::new();
    let mut failed = BTreeSet::new();
    read_block_tag(provider, &id, &mut raw, &mut failed, 0)?;
    resolve(
        &id,
        &raw,
        registry,
        &mut BTreeMap::new(),
        &mut BTreeSet::new(),
    )
    .context("cannot resolve bounce suppression tag")
}

/// Resolve the active tag that lets a block above farmland preserve it.
pub fn load_farmland_maintainers(
    provider: &mut impl ResourceProvider,
    registry: &BTreeSet<String>,
) -> Result<BTreeSet<String>> {
    let id = ResourceId::parse("minecraft:maintains_farmland")?;
    let mut raw = BTreeMap::new();
    let mut failed = BTreeSet::new();
    read_block_tag(provider, &id, &mut raw, &mut failed, 0)?;
    resolve(
        &id,
        &raw,
        registry,
        &mut BTreeMap::new(),
        &mut BTreeSet::new(),
    )
    .context("cannot resolve farmland maintenance tag")
}

impl RespawnBlockTags {
    pub fn load(provider: &mut impl ResourceProvider, registry: BTreeSet<String>) -> Result<Self> {
        ensure!(!registry.is_empty(), "block registry is empty");
        for name in &registry {
            let id = ResourceId::parse(name)?;
            ensure!(
                name == &format!("{}:{}", id.namespace, id.path),
                "block registry identifiers must be canonical"
            );
        }
        let ids = [
            "climbable",
            "invalid_spawn_inside",
            "default_immune_to",
            "fire",
        ]
        .map(|name| ResourceId::parse(&format!("minecraft:{name}")))
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
        let mut raw = BTreeMap::new();
        let mut failed = BTreeSet::new();
        for id in &ids {
            read_block_tag(provider, id, &mut raw, &mut failed, 0)?;
        }
        let mut cache = BTreeMap::new();
        let mut lookup = |index: usize| {
            resolve(
                &ids[index],
                &raw,
                &registry,
                &mut cache,
                &mut BTreeSet::new(),
            )
            .with_context(|| format!("cannot resolve respawn block tag {}", ids[index].path))
        };
        let climbable = lookup(0)?;
        let invalid_inside = lookup(1)?;
        let immune = lookup(2)?;
        let fire = lookup(3)?;
        Ok(Self {
            registry,
            climbable,
            invalid_inside,
            immune,
            fire,
        })
    }

    pub fn for_block(
        &self,
        name: &str,
        properties: &BTreeMap<String, String>,
    ) -> Result<RespawnBlockFacts> {
        let id = ResourceId::parse(name)?;
        let name = format!("{}:{}", id.namespace, id.path);
        ensure!(
            self.registry.contains(&name),
            "block {name} is absent from the registry"
        );
        let campfire_lit = if matches!(
            name.as_str(),
            "minecraft:campfire" | "minecraft:soul_campfire"
        ) && !self.immune.contains(&name)
            && !self.fire.contains(&name)
        {
            match properties.get("lit").map(String::as_str) {
                Some("true") => true,
                Some("false") => false,
                _ => anyhow::bail!("campfire is missing its lit state"),
            }
        } else {
            false
        };
        let dangerous = !self.immune.contains(&name)
            && (self.fire.contains(&name)
                || matches!(
                    name.as_str(),
                    "minecraft:lava"
                        | "minecraft:magma_block"
                        | "minecraft:lava_cauldron"
                        | "minecraft:wither_rose"
                        | "minecraft:sweet_berry_bush"
                        | "minecraft:cactus"
                        | "minecraft:powder_snow"
                )
                || campfire_lit);
        Ok(RespawnBlockFacts {
            dangerous,
            climbable: self.climbable.contains(&name),
            invalid_inside: self.invalid_inside.contains(&name),
        })
    }

    /// The saved palette must refer to an actual block before its shape can
    /// be used in a respawn decision.
    pub fn contains_block(&self, canonical_name: &str) -> bool {
        self.registry.contains(canonical_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Provider(BTreeMap<String, Vec<Vec<u8>>>);
    impl ResourceProvider for Provider {
        fn read_asset(&mut self, _path: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data_stack(&mut self, path: &str) -> Result<Vec<Vec<u8>>> {
            Ok(self.0.get(path).cloned().unwrap_or_default())
        }
    }

    #[test]
    fn applies_active_tags_and_rejects_unknown_blocks_and_states() {
        let mut provider = Provider(BTreeMap::from([
            (
                "data/minecraft/tags/block/climbable.json".into(),
                vec![br#"{"values":["minecraft:stone"]}"#.to_vec()],
            ),
            (
                "data/minecraft/tags/block/invalid_spawn_inside.json".into(),
                vec![br#"{"values":["minecraft:air"]}"#.to_vec()],
            ),
            (
                "data/minecraft/tags/block/default_immune_to.json".into(),
                vec![br#"{"values":["minecraft:cactus"]}"#.to_vec()],
            ),
            (
                "data/minecraft/tags/block/fire.json".into(),
                vec![br#"{"values":["minecraft:stone"]}"#.to_vec()],
            ),
        ]));
        let registry = [
            "minecraft:air",
            "minecraft:stone",
            "minecraft:cactus",
            "minecraft:campfire",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let tags = RespawnBlockTags::load(&mut provider, registry).unwrap();
        let no_properties = BTreeMap::new();
        assert_eq!(
            tags.for_block("stone", &no_properties).unwrap(),
            RespawnBlockFacts {
                dangerous: true,
                climbable: true,
                invalid_inside: false,
            }
        );
        assert!(
            tags.for_block("air", &no_properties)
                .unwrap()
                .invalid_inside
        );
        assert!(!tags.for_block("cactus", &no_properties).unwrap().dangerous);
        assert!(tags.for_block("campfire", &no_properties).is_err());
        assert!(tags.for_block("minecraft:unknown", &no_properties).is_err());
        let mut lit = BTreeMap::from([("lit".into(), "true".into())]);
        assert!(tags.for_block("campfire", &lit).unwrap().dangerous);
        lit.insert("lit".into(), "false".into());
        assert!(!tags.for_block("campfire", &lit).unwrap().dangerous);
    }

    #[test]
    fn falling_fire_tag_respects_pack_replace() {
        let mut provider = Provider(BTreeMap::from([(
            "data/minecraft/tags/block/fire.json".into(),
            vec![
                br#"{"values":["minecraft:fire"]}"#.to_vec(),
                br#"{"replace":true,"values":["minecraft:stone"]}"#.to_vec(),
            ],
        )]));
        let registry = ["minecraft:fire", "minecraft:stone"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert_eq!(
            load_fire_tag(&mut provider, &registry).unwrap(),
            BTreeSet::from(["minecraft:stone".into()])
        );
    }

    #[test]
    fn farmland_maintainers_resolve_pack_replacements_and_nested_tags() {
        let mut provider = Provider(BTreeMap::from([
            (
                "data/minecraft/tags/block/maintains_farmland.json".into(),
                vec![
                    br#"{"values":["minecraft:dirt"]}"#.to_vec(),
                    br##"{"replace":true,"values":["#minecraft:extra_maintainers"]}"##.to_vec(),
                ],
            ),
            (
                "data/minecraft/tags/block/extra_maintainers.json".into(),
                vec![br#"{"values":["minecraft:stone"]}"#.to_vec()],
            ),
        ]));
        let registry = ["minecraft:stone", "minecraft:dirt"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert_eq!(
            load_farmland_maintainers(&mut provider, &registry).unwrap(),
            BTreeSet::from(["minecraft:stone".into()])
        );
    }

    #[test]
    fn bounce_suppression_resolves_pack_overrides() {
        let mut provider = Provider(BTreeMap::from([(
            "data/minecraft/tags/block/suppresses_bounce.json".into(),
            vec![
                br#"{"values":["minecraft:honey_block"]}"#.to_vec(),
                br#"{"replace":true,"values":["minecraft:slime_block"]}"#.to_vec(),
            ],
        )]));
        let registry = BTreeSet::from([
            "minecraft:honey_block".into(),
            "minecraft:slime_block".into(),
        ]);
        assert_eq!(
            load_bounce_suppression_tag(&mut provider, &registry).unwrap(),
            BTreeSet::from(["minecraft:slime_block".into()])
        );
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn installed_falling_fire_tag_loads() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let mut source = crate::packs::ResourceStack::open(&game).unwrap();
        let registry = source.vanilla_block_ids().unwrap();
        let fire = load_fire_tag(&mut source, &registry).unwrap();
        assert!(fire.contains("minecraft:fire"));
        assert!(!fire.contains("minecraft:stone"));
        let suppressed = load_bounce_suppression_tag(&mut source, &registry).unwrap();
        assert!(suppressed.contains("minecraft:honey_block"));
        assert!(!suppressed.contains("minecraft:slime_block"));
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn installed_farmland_maintainers_load() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let mut source = crate::packs::ResourceStack::open(&game).unwrap();
        let registry = source.vanilla_block_ids().unwrap();
        let tag = load_farmland_maintainers(&mut source, &registry).unwrap();
        assert!(tag.contains("minecraft:moving_piston"));
        assert!(tag.contains("minecraft:oak_fence_gate"));
        assert!(!tag.contains("minecraft:stone"));
    }
}
