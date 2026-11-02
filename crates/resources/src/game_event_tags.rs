//! Active data-pack tags used by vibration listeners.
use std::collections::{BTreeMap, BTreeSet};

use crate::{
    packs::{ResourceId, ResourceProvider},
    recipes::{TagEntry, parse_tag},
};
use anyhow::{Context, Result, bail, ensure};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GameEventTags {
    vibrations: BTreeSet<String>,
    ignored_while_sneaking: BTreeSet<String>,
}

impl GameEventTags {
    pub fn new(vibrations: BTreeSet<String>, ignored_while_sneaking: BTreeSet<String>) -> Self {
        Self {
            vibrations,
            ignored_while_sneaking,
        }
    }

    /// `registry` must be the complete active game-event registry.
    pub fn load(provider: &mut impl ResourceProvider, registry: &BTreeSet<String>) -> Result<Self> {
        let mut cache = BTreeMap::new();
        let mut visiting = BTreeSet::new();
        let mut load = |name: &str| -> Result<BTreeSet<String>> {
            resolve(
                provider,
                &ResourceId::parse(name)?,
                registry,
                &mut cache,
                &mut visiting,
            )?
            .with_context(|| format!("required game-event tag {name} is missing"))
        };
        Ok(Self {
            vibrations: load("minecraft:vibrations")?,
            ignored_while_sneaking: load("minecraft:ignore_vibrations_sneaking")?,
        })
    }

    pub fn vibrates(&self, event: &str) -> bool {
        self.vibrations.contains(event)
    }
    pub fn ignored_while_sneaking(&self, event: &str) -> bool {
        self.ignored_while_sneaking.contains(event)
    }
}

fn resolve(
    provider: &mut impl ResourceProvider,
    id: &ResourceId,
    registry: &BTreeSet<String>,
    cache: &mut BTreeMap<ResourceId, Option<BTreeSet<String>>>,
    visiting: &mut BTreeSet<ResourceId>,
) -> Result<Option<BTreeSet<String>>> {
    if let Some(value) = cache.get(id) {
        return Ok(value.clone());
    }
    ensure!(
        visiting.len() < 128 && cache.len() < 4096,
        "game-event tag nesting or count limit exceeded"
    );
    ensure!(
        visiting.insert(id.clone()),
        "cyclic game-event tag reference"
    );
    let result = (|| {
        let path = format!("data/{}/tags/game_event/{}.json", id.namespace, id.path);
        let stack = provider.read_data_stack(&path)?;
        if stack.is_empty() {
            return Ok(None);
        }
        let mut entries = Vec::new();
        for bytes in stack {
            let (replace, layer) =
                parse_tag(&bytes).with_context(|| format!("invalid game-event tag {path}"))?;
            if replace {
                entries.clear();
            }
            entries.extend(layer);
        }
        let mut values = BTreeSet::new();
        for entry in entries {
            match entry {
                TagEntry::Item { id, required } => {
                    if registry.contains(&id) {
                        values.insert(id);
                    } else if required {
                        bail!("required game event {id} is absent from the registry");
                    }
                }
                TagEntry::Tag { id, required } => {
                    match resolve(provider, &id, registry, cache, visiting)? {
                        Some(child) => values.extend(child),
                        None if required => bail!(
                            "required game-event tag {}:{} is missing",
                            id.namespace,
                            id.path
                        ),
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
    fn tags_follow_pack_order_nested_tags_and_required_entries() {
        let registry = BTreeSet::from([
            "minecraft:step".into(),
            "minecraft:splash".into(),
            "minecraft:block_place".into(),
        ]);
        let mut data = Data(BTreeMap::from([
            ("data/minecraft/tags/game_event/vibrations.json".into(), vec![
                br#"{"values":["minecraft:step"]}"#.to_vec(),
                br##"{"replace":true,"values":["#example:group",{"id":"#example:missing","required":false},{"id":"example:unknown","required":false}]}"##.to_vec(),
            ]),
            ("data/example/tags/game_event/group.json".into(), vec![br#"{"values":["minecraft:splash","minecraft:block_place"]}"#.to_vec()]),
            ("data/minecraft/tags/game_event/ignore_vibrations_sneaking.json".into(), vec![br#"{"values":["minecraft:step"]}"#.to_vec()]),
        ]));
        let tags = GameEventTags::load(&mut data, &registry).unwrap();
        assert!(tags.vibrates("minecraft:splash"));
        assert!(!tags.vibrates("minecraft:step"));
        assert!(tags.ignored_while_sneaking("minecraft:step"));
        data.0.insert(
            "data/example/tags/game_event/group.json".into(),
            vec![br#"{"values":["example:unknown"]}"#.to_vec()],
        );
        assert!(GameEventTags::load(&mut data, &registry).is_err());
        data.0.insert(
            "data/example/tags/game_event/group.json".into(),
            vec![br##"{"values":["#minecraft:vibrations"]}"##.to_vec()],
        );
        assert!(GameEventTags::load(&mut data, &registry).is_err());
    }
}
