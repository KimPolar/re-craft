//! Read ordinary block transformations from the active 26.3 data pack stack.
//!
//! This is a deliberately limited evaluator: unsupported predicates, state
//! providers and side effects remain explicit instead of being guessed.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use crate::packs::{ResourceId, ResourceProvider};
use crate::recipes::{TagEntry, parse_tag, resolve_tag};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct State {
    pub name: String,
    pub properties: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Transformation {
    pub state: State,
    pub damage: u32,
    pub update_from_neighbors: bool,
    pub loot: Option<(ResourceId, DropStrategy)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DropStrategy {
    FromMiddle,
    ClickedFace,
}

#[derive(Clone, Debug)]
enum Predicate {
    Blocks(Vec<String>, [i32; 3]),
    Tag(ResourceId, [i32; 3]),
    All(Vec<Predicate>),
    Any(Vec<Predicate>),
    Not(Box<Predicate>),
    Unsupported,
}

#[derive(Clone, Debug)]
enum Provider {
    State(State),
    Copy(State),
    Unsupported,
}

#[derive(Clone, Debug)]
struct Rule {
    predicate: Predicate,
    provider: Provider,
}

#[derive(Clone, Debug)]
struct Entry {
    rules: Vec<Rule>,
    fallback: Option<Provider>,
    disallowed_faces: Vec<String>,
    damage: u32,
    update_from_neighbors: bool,
    loot: Option<(ResourceId, DropStrategy)>,
    unsupported_effects: bool,
}

#[derive(Clone, Debug)]
pub struct BlockTransformers {
    entries: Vec<Entry>,
    tags: BTreeMap<ResourceId, BTreeSet<String>>,
}

/// The original axe entry point remains available to existing callers.
pub type AxeTransformers = BlockTransformers;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolKind {
    Axe,
    Shovel,
    Hoe,
}

impl ToolKind {
    pub const fn index(self) -> usize {
        match self {
            Self::Axe => 0,
            Self::Shovel => 1,
            Self::Hoe => 2,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Axe => "axe",
            Self::Shovel => "shovel",
            Self::Hoe => "hoe",
        }
    }
}

fn canonical(value: &str) -> Result<String> {
    let id = ResourceId::parse(value)?;
    Ok(format!("{}:{}", id.namespace, id.path))
}

fn parse_state(value: &Value) -> Result<State> {
    let name = canonical(
        value
            .get("id")
            .and_then(Value::as_str)
            .context("missing block state id")?,
    )?;
    let mut properties = BTreeMap::new();
    if let Some(values) = value.get("properties") {
        for (name, value) in values
            .as_object()
            .context("invalid block state properties")?
        {
            properties.insert(
                name.clone(),
                value
                    .as_str()
                    .context("invalid block state property value")?
                    .to_owned(),
            );
        }
    }
    if native_core::wood::Wood::from_id(&name).is_some() {
        properties
            .entry("axis".into())
            .or_insert_with(|| "y".into());
    } else if matches!(
        name.as_str(),
        "minecraft:grass_block" | "minecraft:podzol" | "minecraft:mycelium"
    ) {
        properties
            .entry("snowy".into())
            .or_insert_with(|| "false".into());
    } else if name == "minecraft:farmland" {
        properties
            .entry("moisture".into())
            .or_insert_with(|| "0".into());
    }
    Ok(State { name, properties })
}

fn parse_provider(value: &Value) -> Result<Provider> {
    if value.get("id").is_some() {
        return Ok(Provider::State(parse_state(value)?));
    }
    match value.get("type").and_then(Value::as_str) {
        Some("minecraft:copy_properties") => {
            let source = value.get("source").context("missing copied state")?;
            if source.get("id").is_none() {
                return Ok(Provider::Unsupported);
            }
            Ok(Provider::Copy(parse_state(source)?))
        }
        _ => Ok(Provider::Unsupported),
    }
}

fn parse_predicate(value: &Value) -> Result<Predicate> {
    let kind = value.get("type").and_then(Value::as_str);
    match kind {
        Some("minecraft:all_of" | "minecraft:any_of") => {
            let predicates = value
                .get("predicates")
                .and_then(Value::as_array)
                .context("missing nested block predicates")?
                .iter()
                .map(parse_predicate)
                .collect::<Result<Vec<_>>>()?;
            return Ok(if kind == Some("minecraft:all_of") {
                Predicate::All(predicates)
            } else {
                Predicate::Any(predicates)
            });
        }
        Some("minecraft:not") => {
            return Ok(Predicate::Not(Box::new(parse_predicate(
                value
                    .get("predicate")
                    .context("missing negated block predicate")?,
            )?)));
        }
        Some("minecraft:matching_blocks" | "minecraft:matching_block_tag") => {}
        _ => return Ok(Predicate::Unsupported),
    }
    let offset = match value.get("offset") {
        None => [0, 0, 0],
        Some(value) => {
            let Some(values) = value.as_array().filter(|values| values.len() == 3) else {
                return Ok(Predicate::Unsupported);
            };
            let values: Option<Vec<i32>> = values
                .iter()
                .map(|value| value.as_i64().and_then(|v| i32::try_from(v).ok()))
                .collect();
            let Some(values) = values else {
                return Ok(Predicate::Unsupported);
            };
            [values[0], values[1], values[2]]
        }
    };
    if kind == Some("minecraft:matching_block_tag") {
        return Ok(Predicate::Tag(
            ResourceId::parse(
                value
                    .get("tag")
                    .and_then(Value::as_str)
                    .context("missing block tag")?,
            )?,
            offset,
        ));
    }
    let blocks = value.get("blocks").context("missing block predicate")?;
    let mut names = Vec::new();
    if let Some(name) = blocks.as_str() {
        if let Some(tag) = name.strip_prefix('#') {
            return Ok(Predicate::Tag(ResourceId::parse(tag)?, offset));
        }
        names.push(canonical(name)?);
    } else if let Some(values) = blocks.as_array() {
        for value in values {
            let name = value.as_str().context("invalid block predicate")?;
            if name.starts_with('#') {
                return Ok(Predicate::Unsupported);
            }
            names.push(canonical(name)?);
        }
    } else {
        return Ok(Predicate::Unsupported);
    }
    Ok(Predicate::Blocks(names, offset))
}

impl Predicate {
    fn tags(&self, result: &mut BTreeSet<ResourceId>) {
        match self {
            Self::Tag(tag, _) => {
                result.insert(tag.clone());
            }
            Self::All(children) | Self::Any(children) => {
                for child in children {
                    child.tags(result);
                }
            }
            Self::Not(child) => child.tags(result),
            Self::Blocks(_, _) | Self::Unsupported => {}
        }
    }

    fn test(
        &self,
        state: &State,
        tags: &BTreeMap<ResourceId, BTreeSet<String>>,
        lookup: &mut impl FnMut([i32; 3]) -> Option<State>,
    ) -> Result<bool> {
        match self {
            Self::Blocks(blocks, offset) => {
                let other;
                let current = if *offset == [0, 0, 0] {
                    state
                } else {
                    other = lookup(*offset).context("block predicate position is unavailable")?;
                    &other
                };
                Ok(blocks.iter().any(|name| name == &current.name))
            }
            Self::Tag(tag, offset) => {
                let values = tags.get(tag).context("unresolved block predicate tag")?;
                let other;
                let current = if *offset == [0, 0, 0] {
                    state
                } else {
                    other = lookup(*offset).context("block predicate position is unavailable")?;
                    &other
                };
                Ok(values.contains(&current.name))
            }
            Self::All(predicates) => {
                for predicate in predicates {
                    if !predicate.test(state, tags, lookup)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Self::Any(predicates) => {
                for predicate in predicates {
                    if predicate.test(state, tags, lookup)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Self::Not(predicate) => Ok(!predicate.test(state, tags, lookup)?),
            Self::Unsupported => bail!("unsupported block transformation predicate"),
        }
    }
}

pub(crate) fn read_block_tag(
    provider: &mut impl ResourceProvider,
    id: &ResourceId,
    raw: &mut BTreeMap<ResourceId, Vec<TagEntry>>,
    failed: &mut BTreeSet<ResourceId>,
    depth: usize,
) -> Result<()> {
    if raw.contains_key(id) || failed.contains(id) {
        return Ok(());
    }
    if depth >= 128 || raw.len() + failed.len() >= 4096 {
        failed.insert(id.clone());
        return Ok(());
    }
    let path = format!("data/{}/tags/block/{}.json", id.namespace, id.path);
    let stack = provider.read_data_stack(&path)?;
    if stack.is_empty() {
        failed.insert(id.clone());
        return Ok(());
    }
    let mut entries = Vec::new();
    for bytes in stack {
        let Ok((replace, next)) = parse_tag(&bytes) else {
            failed.insert(id.clone());
            return Ok(());
        };
        if replace {
            entries.clear();
        }
        entries.extend(next);
    }
    let children: Vec<_> = entries
        .iter()
        .filter_map(|entry| match entry {
            TagEntry::Tag { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect();
    raw.insert(id.clone(), entries);
    for child in children {
        read_block_tag(provider, &child, raw, failed, depth + 1)?;
    }
    Ok(())
}

impl BlockTransformers {
    pub fn load(provider: &mut impl ResourceProvider) -> Result<Self> {
        Self::load_for(provider, ToolKind::Axe)
    }

    pub fn load_for(provider: &mut impl ResourceProvider, kind: ToolKind) -> Result<Self> {
        let path = format!("data/minecraft/block_transformer/{}.json", kind.name());
        let bytes = provider
            .read_data(&path)?
            .with_context(|| format!("missing block transformer {path}"))?;
        let mut transforms = Self::parse(&bytes)?;
        let mut ids = BTreeSet::new();
        for entry in &transforms.entries {
            for rule in &entry.rules {
                rule.predicate.tags(&mut ids);
            }
        }
        let mut raw = BTreeMap::new();
        let mut failed = BTreeSet::new();
        for id in &ids {
            read_block_tag(provider, id, &mut raw, &mut failed, 0)?;
        }
        for id in &ids {
            let _ = resolve_tag(id, &raw, &mut transforms.tags, &mut BTreeSet::new());
        }
        Ok(transforms)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let json: Value = serde_json::from_slice(bytes).context("invalid axe transformer JSON")?;
        let entries = json
            .as_array()
            .context("axe transformer must be an array")?;
        ensure!(
            !entries.is_empty() && entries.len() <= 200,
            "invalid number of axe transforms"
        );
        let mut parsed = Vec::with_capacity(entries.len());
        for entry in entries {
            let provider = entry
                .get("block_state_provider")
                .context("missing state provider")?;
            let mut rules = Vec::new();
            let mut fallback = None;
            if provider.get("type").and_then(Value::as_str) == Some("minecraft:rule_based") {
                for rule in provider
                    .get("rules")
                    .and_then(Value::as_array)
                    .context("missing state provider rules")?
                {
                    rules.push(Rule {
                        predicate: parse_predicate(
                            rule.get("if_true").context("missing rule predicate")?,
                        )?,
                        provider: parse_provider(
                            rule.get("then").context("missing rule provider")?,
                        )?,
                    });
                }
                if let Some(value) = provider.get("fallback") {
                    fallback = Some(parse_provider(value)?);
                }
            } else {
                fallback = Some(parse_provider(provider)?);
            }
            let mut disallowed_faces = Vec::new();
            if let Some(faces) = entry.get("disallowed_faces") {
                for face in faces.as_array().context("invalid disallowed faces")? {
                    let face = face.as_str().context("invalid face")?;
                    ensure!(
                        ["down", "up", "north", "south", "west", "east"].contains(&face),
                        "invalid face"
                    );
                    disallowed_faces.push(face.to_owned());
                }
            }
            let damage = entry.get("item_damage_per_use").map_or(Ok(0), |value| {
                value
                    .as_u64()
                    .and_then(|value| u32::try_from(value).ok())
                    .context("invalid item damage")
            })?;
            let update_from_neighbors = entry
                .get("update_from_neighbors")
                .map_or(Ok(true), |value| {
                    value.as_bool().context("invalid neighbor update flag")
                })?;
            if let Some(value) = entry.get("consume_on_use") {
                value.as_bool().context("invalid consume flag")?;
            }
            // The supported ordinary axes are non-stackable, so consume_on_use
            // does not consume them; their item_damage_per_use applies instead.
            let mut unsupported_effects = entry
                .get("transform_type")
                .is_some_and(|value| value.as_str() != Some("single_block"));
            let loot = if let Some(value) = entry.get("loot") {
                let id = value.as_str().context("invalid interaction loot table")?;
                let strategy = match entry.get("drop_strategy").and_then(Value::as_str) {
                    None | Some("from_middle") => Some(DropStrategy::FromMiddle),
                    Some("clicked_face") => Some(DropStrategy::ClickedFace),
                    _ => None,
                };
                if let Some(strategy) = strategy {
                    Some((ResourceId::parse(id)?, strategy))
                } else {
                    unsupported_effects = true;
                    None
                }
            } else {
                None
            };
            parsed.push(Entry {
                rules,
                fallback,
                disallowed_faces,
                damage,
                update_from_neighbors,
                loot,
                unsupported_effects,
            });
        }
        Ok(Self {
            entries: parsed,
            tags: BTreeMap::new(),
        })
    }

    /// Interaction loot is loaded from the same active data-pack stack.
    pub fn referenced_loot(&self) -> impl Iterator<Item = &ResourceId> {
        self.entries
            .iter()
            .filter_map(|entry| entry.loot.as_ref().map(|(id, _)| id))
    }

    /// Apply the first supported rule to a state. The caller validates block
    /// registry compatibility and handles the item and world mutation.
    pub fn apply(&self, state: &State, face: &str) -> Result<Option<Transformation>> {
        self.apply_at(state, face, |_| None)
    }

    /// Resolve offset predicates against the same world snapshot as the target.
    pub fn apply_at(
        &self,
        state: &State,
        face: &str,
        mut lookup: impl FnMut([i32; 3]) -> Option<State>,
    ) -> Result<Option<Transformation>> {
        for entry in &self.entries {
            if entry.disallowed_faces.iter().any(|value| value == face) {
                continue;
            }
            let mut selected = None;
            for rule in &entry.rules {
                if rule.predicate.test(state, &self.tags, &mut lookup)? {
                    selected = Some(&rule.provider);
                    break;
                }
            }
            let Some(provider) = selected.or(entry.fallback.as_ref()) else {
                continue;
            };
            ensure!(
                !entry.unsupported_effects,
                "unsupported block transformation side effect"
            );
            let mut result = match provider {
                Provider::State(result) => result.clone(),
                Provider::Copy(result) => {
                    let mut result = result.clone();
                    for (name, value) in &state.properties {
                        if let Some(destination) = result.properties.get_mut(name) {
                            destination.clone_from(value);
                        }
                    }
                    result
                }
                Provider::Unsupported => bail!("unsupported block state provider"),
            };
            result.name = canonical(&result.name)?;
            return Ok(Some(Transformation {
                state: result,
                damage: entry.damage,
                update_from_neighbors: entry.update_from_neighbors,
                loot: entry.loot.clone(),
            }));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct MemoryData {
        files: BTreeMap<String, Vec<Vec<u8>>>,
    }

    impl ResourceProvider for MemoryData {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.files.get(path).and_then(|stack| stack.last()).cloned())
        }
        fn read_data_stack(&mut self, path: &str) -> Result<Vec<Vec<u8>>> {
            Ok(self.files.get(path).cloned().unwrap_or_default())
        }
    }

    fn wood(name: &str, axis: &str) -> State {
        State {
            name: format!("minecraft:{name}"),
            properties: BTreeMap::from([("axis".into(), axis.into())]),
        }
    }

    #[test]
    fn rules_copy_shared_properties_and_respect_faces_and_entry_order() {
        let transforms = AxeTransformers::parse(br#"[
          {"disallowed_faces":["down"], "item_damage_per_use":2,
           "update_from_neighbors":false, "consume_on_use":true,
           "block_state_provider":{"type":"minecraft:rule_based","rules":[
             {"if_true":{"type":"minecraft:matching_blocks","blocks":"minecraft:oak_log"},
              "then":{"type":"minecraft:copy_properties","source":{"id":"minecraft:stripped_spruce_log"}}}
           ]}},
          {"item_damage_per_use":1,
           "block_state_provider":{"type":"minecraft:rule_based","rules":[
             {"if_true":{"type":"minecraft:matching_blocks","blocks":["minecraft:oak_log","minecraft:birch_log"]},
              "then":{"id":"minecraft:stone"}}
           ]}}
        ]"#).unwrap();
        for axis in ["x", "y", "z"] {
            let result = transforms
                .apply(&wood("oak_log", axis), "north")
                .unwrap()
                .unwrap();
            assert_eq!(result.state, wood("stripped_spruce_log", axis));
            assert_eq!(result.damage, 2);
            assert!(!result.update_from_neighbors);
        }
        let result = transforms
            .apply(&wood("oak_log", "x"), "down")
            .unwrap()
            .unwrap();
        assert_eq!(result.state.name, "minecraft:stone");
        assert!(result.state.properties.is_empty());
        assert_eq!(result.damage, 1);
        assert!(result.update_from_neighbors);
        assert!(
            transforms
                .apply(&wood("spruce_log", "z"), "north")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn unsupported_predicates_providers_and_effects_do_not_fall_through() {
        for predicate in [
            serde_json::json!({"type":"minecraft:matching_blocks", "blocks":"#minecraft:logs"}),
            serde_json::json!({"type":"minecraft:matching_blocks", "blocks":"minecraft:oak_log", "offset":[1,0,0]}),
            serde_json::json!({"type":"minecraft:would_survive", "state":{"id":"minecraft:stone"}}),
        ] {
            let value = serde_json::json!([{"block_state_provider":{"type":"minecraft:rule_based","rules":[
                {"if_true":predicate, "then":{"id":"minecraft:stone"}},
                {"if_true":{"type":"minecraft:matching_blocks","blocks":"minecraft:oak_log"}, "then":{"id":"minecraft:dirt"}}
            ]}}]);
            let transforms = AxeTransformers::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
            assert!(transforms.apply(&wood("oak_log", "y"), "up").is_err());
        }
        for extra in [
            serde_json::json!({"loot":"minecraft:test", "drop_strategy":"unknown"}),
            serde_json::json!({"transform_type":"double_block"}),
        ] {
            let mut entry = serde_json::json!({"block_state_provider":{"id":"minecraft:stone"}});
            entry
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            let transforms =
                AxeTransformers::parse(&serde_json::to_vec(&vec![entry]).unwrap()).unwrap();
            assert!(transforms.apply(&wood("oak_log", "y"), "up").is_err());
        }
        let transforms = AxeTransformers::parse(br#"[{"block_state_provider":{"type":"minecraft:copy_properties","source":"example:dynamic"}}]"#).unwrap();
        assert!(transforms.apply(&wood("oak_log", "y"), "up").is_err());
    }

    #[test]
    fn block_tags_respect_pack_layers_nested_entries_and_offset_predicates() {
        let mut data = MemoryData::default();
        data.files.insert("data/minecraft/block_transformer/shovel.json".into(), vec![br#"[{"disallowed_faces":["down"],"item_damage_per_use":1,
          "block_state_provider":{"type":"minecraft:rule_based","rules":[{"if_true":{"type":"minecraft:all_of","predicates":[
            {"type":"minecraft:matching_block_tag","tag":"minecraft:soil"},
            {"type":"minecraft:matching_block_tag","tag":"minecraft:air","offset":[0,1,0]}
          ]},"then":{"id":"minecraft:dirt_path"}}]}}]"#.to_vec()]);
        data.files.insert("data/minecraft/tags/block/soil.json".into(), vec![
            br#"{"values":["minecraft:grass_block","minecraft:dirt"]}"#.to_vec(),
            br##"{"replace":true,"values":["minecraft:podzol","#example:nested",{"id":"#example:missing","required":false}]}"##.to_vec(),
        ]);
        data.files.insert(
            "data/example/tags/block/nested.json".into(),
            vec![br#"{"values":["minecraft:coarse_dirt"]}"#.to_vec()],
        );
        data.files.insert(
            "data/minecraft/tags/block/air.json".into(),
            vec![br#"{"values":["minecraft:air","minecraft:cave_air"]}"#.to_vec()],
        );
        let transforms = BlockTransformers::load_for(&mut data, ToolKind::Shovel).unwrap();
        let state = |name: &str| State {
            name: format!("minecraft:{name}"),
            properties: BTreeMap::new(),
        };
        for source in ["podzol", "coarse_dirt"] {
            for above in ["air", "cave_air"] {
                let result = transforms
                    .apply_at(&state(source), "up", |offset| {
                        assert_eq!(offset, [0, 1, 0]);
                        Some(state(above))
                    })
                    .unwrap()
                    .unwrap();
                assert_eq!(result.state.name, "minecraft:dirt_path");
                assert_eq!(result.damage, 1);
            }
            assert!(
                transforms
                    .apply_at(&state(source), "up", |_| Some(state("stone")))
                    .unwrap()
                    .is_none()
            );
            assert!(transforms.apply_at(&state(source), "up", |_| None).is_err());
            assert!(
                transforms
                    .apply_at(&state(source), "down", |_| None)
                    .unwrap()
                    .is_none()
            );
        }
        // The higher pack replaced, rather than appended to, the original tag.
        assert!(
            transforms
                .apply_at(&state("dirt"), "up", |_| panic!("short-circuit"))
                .unwrap()
                .is_none()
        );
        data.files.insert(
            "data/example/tags/block/nested.json".into(),
            vec![br##"{"values":["#minecraft:soil"]}"##.to_vec()],
        );
        let invalid = BlockTransformers::load_for(&mut data, ToolKind::Shovel).unwrap();
        assert!(
            invalid
                .apply_at(&state("podzol"), "up", |_| Some(state("air")))
                .is_err()
        );
    }
}
