//! Resolve the two 26.3 heightmap motion tags from the active data-pack stack.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail, ensure};

use crate::{
    packs::{ResourceId, ResourceProvider},
    recipes::TagEntry,
    transforms::read_block_tag,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MotionTags {
    pub blocks_motion: bool,
    pub blocks_motion_no_leaves: bool,
}

/// The registry must be the world's complete, canonical block registry. The
/// caller must separately resolve each state's `isAir` and fluid state.
#[derive(Clone, Debug)]
pub struct HeightmapTags {
    registry: BTreeSet<String>,
    motion: BTreeSet<String>,
    no_leaves: BTreeSet<String>,
}

pub(crate) fn resolve(
    id: &ResourceId,
    raw: &BTreeMap<ResourceId, Vec<TagEntry>>,
    registry: &BTreeSet<String>,
    cache: &mut BTreeMap<ResourceId, BTreeSet<String>>,
    visiting: &mut BTreeSet<ResourceId>,
) -> Result<BTreeSet<String>> {
    if let Some(values) = cache.get(id) {
        return Ok(values.clone());
    }
    ensure!(visiting.insert(id.clone()), "cyclic block tag reference");
    let result = (|| {
        let entries = raw
            .get(id)
            .context("required block tag is missing or invalid")?;
        let mut values = BTreeSet::new();
        for entry in entries {
            match entry {
                TagEntry::Item { id, required } => {
                    if registry.contains(id) {
                        values.insert(id.clone());
                    } else if *required {
                        bail!("required block {id} is absent from the registry");
                    }
                }
                TagEntry::Tag {
                    id: child,
                    required,
                } => {
                    if !raw.contains_key(child) && !required {
                        continue;
                    }
                    values.extend(resolve(child, raw, registry, cache, visiting).with_context(
                        || {
                            format!(
                                "cannot resolve block tag {}:{}",
                                child.namespace, child.path
                            )
                        },
                    )?);
                }
            }
        }
        Ok(values)
    })();
    visiting.remove(id);
    if let Ok(values) = &result {
        cache.insert(id.clone(), values.clone());
    }
    result
}

impl HeightmapTags {
    pub fn load(provider: &mut impl ResourceProvider, registry: &BTreeSet<String>) -> Result<Self> {
        ensure!(!registry.is_empty(), "block registry is empty");
        for id in registry {
            let parsed = ResourceId::parse(id)?;
            ensure!(
                *id == format!("{}:{}", parsed.namespace, parsed.path),
                "block registry identifiers must be canonical"
            );
        }
        let motion_id = ResourceId::parse("minecraft:blocks_motion_in_heightmap")?;
        let no_leaves_id = ResourceId::parse("minecraft:blocks_motion_in_heightmap_no_leaves")?;
        let mut raw = BTreeMap::new();
        let mut failed = BTreeSet::new();
        for id in [&motion_id, &no_leaves_id] {
            read_block_tag(provider, id, &mut raw, &mut failed, 0)?;
        }
        let mut cache = BTreeMap::new();
        let motion = resolve(&motion_id, &raw, registry, &mut cache, &mut BTreeSet::new())
            .context("cannot resolve the motion heightmap tag")?;
        let no_leaves = resolve(
            &no_leaves_id,
            &raw,
            registry,
            &mut cache,
            &mut BTreeSet::new(),
        )
        .context("cannot resolve the no-leaves heightmap tag")?;
        Ok(Self {
            registry: registry.clone(),
            motion,
            no_leaves,
        })
    }

    pub fn for_block(&self, name: &str) -> Result<MotionTags> {
        let id = ResourceId::parse(name)?;
        let canonical = format!("{}:{}", id.namespace, id.path);
        ensure!(
            self.registry.contains(&canonical),
            "block {canonical} is absent from the registry"
        );
        Ok(MotionTags {
            blocks_motion: self.motion.contains(&canonical),
            blocks_motion_no_leaves: self.no_leaves.contains(&canonical),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct MemoryData(BTreeMap<String, Vec<Vec<u8>>>);
    impl ResourceProvider for MemoryData {
        fn read_asset(&mut self, _path: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data_stack(&mut self, path: &str) -> Result<Vec<Vec<u8>>> {
            Ok(self.0.get(path).cloned().unwrap_or_default())
        }
    }
    fn registry() -> BTreeSet<String> {
        [
            "minecraft:air",
            "minecraft:stone",
            "minecraft:oak_log",
            "minecraft:dirt",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }
    fn provider() -> MemoryData {
        MemoryData(BTreeMap::from([
            ("data/minecraft/tags/block/blocks_motion_in_heightmap.json".into(), vec![
                br##"{"values":["minecraft:stone","#example:logs",{"id":"minecraft:missing","required":false},{"id":"#example:missing","required":false}]}"##.to_vec(),
                br#"{"values":["minecraft:dirt"]}"#.to_vec(),
            ]),
            ("data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json".into(), vec![
                br#"{"values":["minecraft:stone"]}"#.to_vec(),
                br##"{"replace":true,"values":["#example:logs"]}"##.to_vec(),
            ]),
            ("data/example/tags/block/logs.json".into(), vec![br#"{"values":["minecraft:oak_log"]}"#.to_vec()]),
        ]))
    }

    #[test]
    fn resolves_nested_layered_and_optional_tags_using_registry() {
        let tags = HeightmapTags::load(&mut provider(), &registry()).unwrap();
        assert_eq!(
            tags.for_block("stone").unwrap(),
            MotionTags {
                blocks_motion: true,
                blocks_motion_no_leaves: false
            }
        );
        assert_eq!(
            tags.for_block("minecraft:oak_log").unwrap(),
            MotionTags {
                blocks_motion: true,
                blocks_motion_no_leaves: true
            }
        );
        assert_eq!(
            tags.for_block("minecraft:dirt").unwrap(),
            MotionTags {
                blocks_motion: true,
                blocks_motion_no_leaves: false
            }
        );
        assert_eq!(
            tags.for_block("minecraft:air").unwrap(),
            MotionTags {
                blocks_motion: false,
                blocks_motion_no_leaves: false
            }
        );
        assert!(tags.for_block("example:missing").is_err());
    }

    #[test]
    fn required_missing_blocks_tags_cycles_and_noncanonical_registries_fail() {
        let mut missing = provider();
        missing
            .0
            .get_mut("data/example/tags/block/logs.json")
            .unwrap()
            .push(br#"{"values":["example:missing"]}"#.to_vec());
        assert!(HeightmapTags::load(&mut missing, &registry()).is_err());
        let mut missing_tag = provider();
        missing_tag
            .0
            .get_mut("data/example/tags/block/logs.json")
            .unwrap()
            .push(br##"{"values":["#example:missing"]}"##.to_vec());
        assert!(HeightmapTags::load(&mut missing_tag, &registry()).is_err());
        let mut cycle = provider();
        cycle
            .0
            .get_mut("data/example/tags/block/logs.json")
            .unwrap()
            .push(br##"{"values":["#example:logs"]}"##.to_vec());
        assert!(HeightmapTags::load(&mut cycle, &registry()).is_err());
        assert!(HeightmapTags::load(&mut provider(), &BTreeSet::from(["stone".into()])).is_err());
    }
}
