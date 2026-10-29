//! A data-pack aware subset of 26.3 block loot for ordinary unenchanted items.
//!
//! Unsupported conditions, random rolls and functions return an error instead of
//! inventing a drop. The evaluator will be extended alongside item components.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use crate::packs::{ResourceId, ResourceProvider};

const BASIC_BLOCKS: [&str; 15] = [
    "grass_block",
    "dirt",
    "sand",
    "red_sand",
    "gravel",
    "stone",
    "stone_bricks",
    "cobblestone",
    "coarse_dirt",
    "rooted_dirt",
    "podzol",
    "mycelium",
    "dirt_path",
    "farmland",
    "stone_button",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Drop {
    pub item: ResourceId,
    pub count: u32,
}

#[derive(Clone, Debug)]
enum Condition {
    Always,
    Tool(ToolPredicate),
    All(Vec<Condition>),
    Any(Vec<Condition>),
    Not(Box<Condition>),
    UnenchantedTableBonus(f32),
    Unsupported(String),
}

#[derive(Clone, Debug, Default)]
struct ToolPredicate {
    items: Option<Vec<ResourceId>>,
    min_count: Option<u32>,
    max_count: Option<u32>,
    requires_enchantment: bool,
    unsupported: Option<String>,
}

#[derive(Clone, Copy)]
pub struct BasicTool<'a> {
    /// An empty hand is represented as `None` with count zero.
    pub item: Option<&'a str>,
    pub count: u32,
}

impl Condition {
    fn test(
        &self,
        tool: BasicTool<'_>,
        random: &mut Option<&mut dyn FnMut() -> f32>,
    ) -> Result<bool> {
        match self {
            Self::Always => Ok(true),
            Self::Tool(predicate) => {
                if let Some(reason) = &predicate.unsupported {
                    bail!("unsupported tool predicate: {reason}");
                }
                if let Some(items) = &predicate.items {
                    let Some(item) = tool.item else {
                        return Ok(false);
                    };
                    if !items
                        .iter()
                        .any(|id| format!("{}:{}", id.namespace, id.path) == item)
                    {
                        return Ok(false);
                    }
                }
                if predicate.min_count.is_some_and(|min| tool.count < min)
                    || predicate.max_count.is_some_and(|max| tool.count > max)
                    || predicate.requires_enchantment
                {
                    return Ok(false);
                }
                Ok(true)
            }
            Self::All(conditions) => {
                for condition in conditions {
                    if !condition.test(tool, random)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Self::Any(conditions) => {
                for condition in conditions {
                    if condition.test(tool, random)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Self::Not(condition) => Ok(!condition.test(tool, random)?),
            Self::UnenchantedTableBonus(chance) => {
                let random = random
                    .as_mut()
                    .context("random loot requires a random source")?;
                Ok(random() < *chance)
            }
            Self::Unsupported(reason) => bail!("unsupported loot condition: {reason}"),
        }
    }
}

#[derive(Clone, Debug)]
struct Modifier {
    condition: Condition,
    kind: ModifierKind,
}

#[derive(Clone, Debug)]
enum ModifierKind {
    SetCount(u32, bool),
    ExplosionDecay,
    Unsupported(String),
}

impl Modifier {
    fn apply(
        &self,
        drop: &mut Drop,
        tool: BasicTool<'_>,
        random: &mut Option<&mut dyn FnMut() -> f32>,
    ) -> Result<()> {
        if !self.condition.test(tool, random)? {
            return Ok(());
        }
        match &self.kind {
            ModifierKind::SetCount(count, add) => {
                drop.count = if *add {
                    drop.count.saturating_add(*count)
                } else {
                    *count
                };
            }
            // Normal player mining does not have an explosion radius.
            ModifierKind::ExplosionDecay => {}
            ModifierKind::Unsupported(reason) => bail!("unsupported loot modifier: {reason}"),
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
enum EntryKind {
    Item(ResourceId),
    Empty,
    Alternatives(Vec<Entry>),
    Unsupported(String),
}

#[derive(Clone, Debug)]
struct Entry {
    condition: Condition,
    kind: EntryKind,
    modifiers: Vec<Modifier>,
    weight: u32,
}

impl Entry {
    /// An expanded empty entry still stops an alternatives chain.
    fn expand(
        &self,
        tool: BasicTool<'_>,
        random: &mut Option<&mut dyn FnMut() -> f32>,
    ) -> Result<Option<(u32, Option<Drop>)>> {
        if !self.condition.test(tool, random)? {
            return Ok(None);
        }
        let (weight, mut value) = match &self.kind {
            EntryKind::Item(id) => (
                self.weight,
                Some(Drop {
                    item: id.clone(),
                    count: 1,
                }),
            ),
            EntryKind::Empty => (self.weight, None),
            EntryKind::Alternatives(children) => {
                let mut selected = None;
                for child in children {
                    if let Some(value) = child.expand(tool, random)? {
                        selected = Some(value);
                        break;
                    }
                }
                let Some(selected) = selected else {
                    return Ok(None);
                };
                selected
            }
            EntryKind::Unsupported(reason) => bail!("unsupported loot entry: {reason}"),
        };
        if let Some(drop) = &mut value {
            for modifier in &self.modifiers {
                modifier.apply(drop, tool, random)?;
            }
            if drop.count == 0 {
                value = None;
            }
        }
        Ok(Some((weight, value)))
    }
}

#[derive(Clone, Debug)]
struct Pool {
    condition: Condition,
    entries: Vec<Entry>,
    modifiers: Vec<Modifier>,
    rolls: u32,
}

#[derive(Clone, Debug)]
struct Table {
    pools: Vec<Pool>,
    modifiers: Vec<Modifier>,
}

#[derive(Clone, Debug, Default)]
pub struct BlockLootTables {
    tables: BTreeMap<ResourceId, Table>,
    interaction_tables: BTreeMap<ResourceId, Table>,
    pub warnings: Vec<String>,
}

impl BlockLootTables {
    /// Read tables and referenced predicates from the configured world data packs.
    pub fn load(provider: &mut impl ResourceProvider) -> Result<Self> {
        let mut result = Self::default();
        let names = BASIC_BLOCKS
            .into_iter()
            .chain(
                native_core::wood::Planks::ALL
                    .into_iter()
                    .map(|planks| planks.id().strip_prefix("minecraft:").unwrap())
                    .chain(std::iter::once("crafting_table")),
            )
            .chain(
                native_core::wood::Wood::all()
                    .map(|wood| wood.id().strip_prefix("minecraft:").unwrap()),
            );
        for name in names {
            let id = ResourceId::parse(&format!("minecraft:blocks/{name}"))?;
            let path = format!("data/{}/loot_table/{}.json", id.namespace, id.path);
            let Some(bytes) = provider.read_data(&path)? else {
                result
                    .warnings
                    .push(format!("missing block loot table {path}"));
                continue;
            };
            match serde_json::from_slice(&bytes)
                .context("invalid JSON")
                .and_then(|json| parse_table(&json, provider, "minecraft:block"))
            {
                Ok(table) => {
                    result.tables.insert(id, table);
                }
                Err(error) => result
                    .warnings
                    .push(format!("invalid block loot table {path}: {error:#}")),
            }
        }
        Ok(result)
    }

    /// Load only the interaction tables referenced by active tool transformers.
    pub fn load_interactions<'a>(
        &mut self,
        provider: &mut impl ResourceProvider,
        ids: impl IntoIterator<Item = &'a ResourceId>,
    ) -> Result<()> {
        let ids: BTreeSet<_> = ids.into_iter().cloned().collect();
        for id in ids {
            let path = format!("data/{}/loot_table/{}.json", id.namespace, id.path);
            let Some(bytes) = provider.read_data(&path)? else {
                self.warnings
                    .push(format!("missing interaction loot table {path}"));
                continue;
            };
            match serde_json::from_slice(&bytes)
                .context("invalid JSON")
                .and_then(|json| parse_table(&json, provider, "minecraft:block_interact"))
            {
                Ok(table) => {
                    self.interaction_tables.insert(id, table);
                }
                Err(error) => self
                    .warnings
                    .push(format!("invalid interaction loot table {path}: {error:#}")),
            }
        }
        Ok(())
    }

    pub fn interaction_drops(&self, id: &ResourceId, tool: BasicTool<'_>) -> Result<Vec<Drop>> {
        let table = self
            .interaction_tables
            .get(id)
            .context("interaction loot table is not supported or was not loaded")?;
        Self::evaluate(table, tool, None)
    }

    pub fn drops(&self, block: &str, tool: BasicTool<'_>) -> Result<Vec<Drop>> {
        self.drops_with_random(block, tool, None)
    }

    /// Random source is session-local; enchanted tool components are not yet supported.
    pub fn drops_with_random(
        &self,
        block: &str,
        tool: BasicTool<'_>,
        random: Option<&mut dyn FnMut() -> f32>,
    ) -> Result<Vec<Drop>> {
        let block = ResourceId::parse(block)?;
        let id = ResourceId {
            namespace: block.namespace,
            path: format!("blocks/{}", block.path),
        };
        let table = self
            .tables
            .get(&id)
            .context("block loot table is not supported or was not loaded")?;
        Self::evaluate(table, tool, random)
    }

    fn evaluate(
        table: &Table,
        tool: BasicTool<'_>,
        mut random: Option<&mut dyn FnMut() -> f32>,
    ) -> Result<Vec<Drop>> {
        let mut drops = Vec::new();
        for pool in &table.pools {
            if !pool.condition.test(tool, &mut random)? {
                continue;
            }
            for _ in 0..pool.rolls {
                let mut selected = None;
                let mut expanded = 0;
                for entry in &pool.entries {
                    if let Some((weight, value)) = entry.expand(tool, &mut random)?
                        && weight > 0
                    {
                        expanded += 1;
                        selected = value;
                    }
                }
                ensure!(
                    expanded <= 1,
                    "random selection among multiple loot entries is not implemented"
                );
                if let Some(mut drop) = selected {
                    for modifier in &pool.modifiers {
                        modifier.apply(&mut drop, tool, &mut random)?;
                    }
                    for modifier in &table.modifiers {
                        modifier.apply(&mut drop, tool, &mut random)?;
                    }
                    if drop.count > 0 {
                        drops.push(drop);
                    }
                }
            }
        }
        Ok(drops)
    }
}

fn parse_table(
    json: &Value,
    provider: &mut impl ResourceProvider,
    expected_type: &str,
) -> Result<Table> {
    if let Some(kind) = json.get("type") {
        ensure!(
            kind.as_str() == Some(expected_type),
            "loot table has an unsupported context"
        );
    }
    let mut visiting = BTreeSet::new();
    let pools = json
        .get("pools")
        .and_then(Value::as_array)
        .context("loot table has no pools")?;
    ensure!(pools.len() <= 128, "too many loot pools");
    let mut parsed = Vec::new();
    for pool in pools {
        let rolls = pool
            .get("rolls")
            .and_then(Value::as_u64)
            .context("non-constant loot rolls are unsupported")?;
        ensure!(rolls <= 64, "too many loot rolls");
        if let Some(bonus) = pool.get("bonus_rolls") {
            ensure!(
                bonus.as_f64() == Some(0.0),
                "bonus loot rolls are unsupported"
            );
        }
        let entries = pool
            .get("entries")
            .and_then(Value::as_array)
            .context("loot pool has no entries")?;
        ensure!(entries.len() <= 128, "too many loot entries");
        parsed.push(Pool {
            condition: parse_optional_condition(pool.get("condition"), provider, &mut visiting)?,
            entries: entries
                .iter()
                .map(|entry| parse_entry(entry, provider, &mut visiting, 0))
                .collect::<Result<_>>()?,
            modifiers: parse_modifiers(pool.get("modifier"), provider, &mut visiting)?,
            rolls: rolls as u32,
        });
    }
    Ok(Table {
        pools: parsed,
        modifiers: parse_modifiers(json.get("modifier"), provider, &mut visiting)?,
    })
}

fn parse_entry(
    json: &Value,
    provider: &mut impl ResourceProvider,
    visiting: &mut BTreeSet<ResourceId>,
    depth: usize,
) -> Result<Entry> {
    ensure!(depth < 32, "loot entry nesting is too deep");
    let condition = parse_optional_condition(json.get("condition"), provider, visiting)?;
    let kind = json
        .get("type")
        .and_then(Value::as_str)
        .context("loot entry has no type")?;
    let kind = match kind {
        "minecraft:item" => EntryKind::Item(ResourceId::parse(
            json.get("name")
                .and_then(Value::as_str)
                .context("item loot entry has no name")?,
        )?),
        "minecraft:empty" => EntryKind::Empty,
        "minecraft:alternatives" => {
            let children = json
                .get("children")
                .and_then(Value::as_array)
                .context("alternatives have no children")?;
            ensure!(children.len() <= 128, "too many loot alternatives");
            EntryKind::Alternatives(
                children
                    .iter()
                    .map(|child| parse_entry(child, provider, visiting, depth + 1))
                    .collect::<Result<_>>()?,
            )
        }
        other => EntryKind::Unsupported(other.into()),
    };
    let weight = json
        .get("weight")
        .map(|value| value.as_i64().context("invalid loot weight"))
        .transpose()?
        .unwrap_or(1)
        .clamp(0, i64::from(u32::MAX)) as u32;
    Ok(Entry {
        condition,
        kind,
        modifiers: parse_modifiers(json.get("modifier"), provider, visiting)?,
        weight,
    })
}

fn parse_modifiers(
    value: Option<&Value>,
    provider: &mut impl ResourceProvider,
    visiting: &mut BTreeSet<ResourceId>,
) -> Result<Vec<Modifier>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let values: Vec<&Value> = match value {
        Value::Array(values) => values.iter().collect(),
        _ => vec![value],
    };
    ensure!(values.len() <= 128, "too many loot modifiers");
    values
        .into_iter()
        .map(|modifier| {
            let condition =
                parse_optional_condition(modifier.get("condition"), provider, visiting)?;
            let kind = modifier
                .get("type")
                .and_then(Value::as_str)
                .context("loot modifier has no type")?;
            let kind = match kind {
                "minecraft:set_count" => match modifier.get("count").and_then(Value::as_u64) {
                    Some(count) if count <= 1024 => ModifierKind::SetCount(
                        count as u32,
                        modifier
                            .get("add")
                            .map(|value| value.as_bool().context("invalid set_count add flag"))
                            .transpose()?
                            .unwrap_or(false),
                    ),
                    _ => ModifierKind::Unsupported("non-constant set_count".into()),
                },
                "minecraft:explosion_decay" => ModifierKind::ExplosionDecay,
                other => ModifierKind::Unsupported(other.into()),
            };
            Ok(Modifier { condition, kind })
        })
        .collect()
}

fn parse_optional_condition(
    value: Option<&Value>,
    provider: &mut impl ResourceProvider,
    visiting: &mut BTreeSet<ResourceId>,
) -> Result<Condition> {
    match value {
        None => Ok(Condition::Always),
        Some(value) => parse_condition(value, provider, visiting, 0),
    }
}

fn parse_condition(
    value: &Value,
    provider: &mut impl ResourceProvider,
    visiting: &mut BTreeSet<ResourceId>,
    depth: usize,
) -> Result<Condition> {
    ensure!(depth < 32, "loot predicate nesting is too deep");
    if let Some(reference) = value.as_str() {
        let id = ResourceId::parse(reference)?;
        ensure!(
            visiting.insert(id.clone()),
            "cyclic loot predicate reference"
        );
        let path = format!("data/{}/predicate/{}.json", id.namespace, id.path);
        let resolved = provider
            .read_data(&path)?
            .with_context(|| format!("missing predicate {path}"))?;
        let json: Value = serde_json::from_slice(&resolved)?;
        let result = parse_condition(&json, provider, visiting, depth + 1);
        visiting.remove(&id);
        return result;
    }
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .context("loot condition has no type")?;
    Ok(match kind {
        "minecraft:survives_explosion" => Condition::Always,
        "minecraft:match_tool" => Condition::Tool(parse_tool(value.get("predicate"))?),
        "minecraft:table_bonus"
            if value.get("enchantment").and_then(Value::as_str) == Some("minecraft:fortune") =>
        {
            let chances = value
                .get("chances")
                .and_then(Value::as_array)
                .context("table bonus has no chances")?;
            ensure!(
                !chances.is_empty() && chances.len() <= 128,
                "invalid table bonus chances"
            );
            let first = chances[0].as_f64().context("invalid table bonus chance")?;
            ensure!((0.0..=1.0).contains(&first), "invalid table bonus chance");
            Condition::UnenchantedTableBonus(first as f32)
        }
        "minecraft:all_of" | "minecraft:any_of" => {
            let terms = value
                .get("terms")
                .and_then(Value::as_array)
                .context("condition has no terms")?;
            ensure!(terms.len() <= 128, "too many predicate terms");
            let terms = terms
                .iter()
                .map(|term| parse_condition(term, provider, visiting, depth + 1))
                .collect::<Result<_>>()?;
            if kind == "minecraft:all_of" {
                Condition::All(terms)
            } else {
                Condition::Any(terms)
            }
        }
        "minecraft:inverted" => Condition::Not(Box::new(parse_condition(
            value
                .get("term")
                .context("inverted condition has no term")?,
            provider,
            visiting,
            depth + 1,
        )?)),
        other => Condition::Unsupported(other.into()),
    })
}

fn parse_tool(value: Option<&Value>) -> Result<ToolPredicate> {
    let Some(value) = value else {
        return Ok(ToolPredicate::default());
    };
    let object = value
        .as_object()
        .context("tool predicate must be an object")?;
    let mut result = ToolPredicate::default();
    for key in object.keys() {
        if !matches!(key.as_str(), "items" | "count" | "predicates") {
            result.unsupported = Some(key.clone());
        }
    }
    if let Some(items) = value.get("items") {
        let items: Vec<&Value> = match items {
            Value::Array(items) => items.iter().collect(),
            _ => vec![items],
        };
        let mut parsed = Vec::new();
        for item in items {
            let name = item.as_str().context("invalid tool item")?;
            if name.starts_with('#') {
                result.unsupported = Some("item tag".into());
            } else {
                parsed.push(ResourceId::parse(name)?);
            }
        }
        result.items = Some(parsed);
    }
    if let Some(count) = value.get("count") {
        if let Some(count) = count.as_u64().and_then(|n| u32::try_from(n).ok()) {
            result.min_count = Some(count);
            result.max_count = Some(count);
        } else if let Some(bounds) = count.as_object() {
            if bounds.keys().any(|key| key != "min" && key != "max") {
                result.unsupported = Some("count predicate".into());
            }
            result.min_count = bounds
                .get("min")
                .map(|value| {
                    value
                        .as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                        .context("invalid minimum tool count")
                })
                .transpose()?;
            result.max_count = bounds
                .get("max")
                .map(|value| {
                    value
                        .as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                        .context("invalid maximum tool count")
                })
                .transpose()?;
        } else {
            result.unsupported = Some("count predicate".into());
        }
    }
    if let Some(predicates) = value.get("predicates") {
        let predicates = predicates
            .as_object()
            .context("tool predicates must be an object")?;
        for (key, value) in predicates {
            if key != "minecraft:enchantments" {
                result.unsupported = Some(key.clone());
                continue;
            }
            let enchantments = value
                .as_array()
                .context("enchantment predicate must be an array")?;
            for enchantment in enchantments {
                let min = enchantment
                    .get("levels")
                    .and_then(|levels| levels.get("min"))
                    .and_then(Value::as_u64);
                if min.is_some_and(|min| min > 0) {
                    result.requires_enchantment = true;
                } else {
                    result.unsupported = Some("enchantment level predicate".into());
                }
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Memory {
        data: BTreeMap<String, Vec<u8>>,
    }
    impl ResourceProvider for Memory {
        fn read_asset(&mut self, _path: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.data.get(path).cloned())
        }
    }
    impl Memory {
        fn put(&mut self, path: &str, json: &str) {
            self.data.insert(path.into(), json.as_bytes().to_vec());
        }
    }

    #[test]
    fn conditions_alternatives_and_modifiers_follow_the_selected_resources() {
        let mut source = Memory::default();
        source.put(
            "data/minecraft/predicate/tool/test.json",
            r#"{"type":"minecraft:match_tool","predicate":{"items":"minecraft:stone"}}"#,
        );
        source.put("data/minecraft/loot_table/blocks/dirt.json", r#"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:alternatives","children":[{"type":"minecraft:item","condition":"minecraft:tool/test","name":"minecraft:stone"},{"type":"minecraft:item","name":"minecraft:dirt"}],"modifier":[{"type":"minecraft:set_count","count":2}]}]}]}"#);
        let tables = BlockLootTables::load(&mut source).unwrap();
        let pick = |id| BasicTool {
            item: id,
            count: u32::from(id.is_some()),
        };
        assert_eq!(
            tables.drops("minecraft:dirt", pick(None)).unwrap(),
            vec![Drop {
                item: ResourceId::parse("minecraft:dirt").unwrap(),
                count: 2
            }]
        );
        assert_eq!(
            tables
                .drops("minecraft:dirt", pick(Some("minecraft:stone")))
                .unwrap(),
            vec![Drop {
                item: ResourceId::parse("minecraft:stone").unwrap(),
                count: 2
            }]
        );
        assert!(tables.drops("minecraft:grass_block", pick(None)).is_err());
    }

    #[test]
    fn empty_and_zero_weight_alternatives_and_unsupported_randomness_are_not_guessed() {
        let mut source = Memory::default();
        source.put("data/minecraft/loot_table/blocks/dirt.json", r#"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:alternatives","children":[{"type":"minecraft:empty","weight":0},{"type":"minecraft:item","name":"minecraft:dirt"}]}]}]}"#);
        let tables = BlockLootTables::load(&mut source).unwrap();
        assert!(
            tables
                .drops(
                    "minecraft:dirt",
                    BasicTool {
                        item: None,
                        count: 0
                    }
                )
                .unwrap()
                .is_empty()
        );
        source.put("data/minecraft/loot_table/blocks/dirt.json", r#"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:dirt"},{"type":"minecraft:item","name":"minecraft:stone"}]}]}"#);
        let tables = BlockLootTables::load(&mut source).unwrap();
        assert!(
            tables
                .drops(
                    "minecraft:dirt",
                    BasicTool {
                        item: None,
                        count: 0
                    }
                )
                .is_err()
        );
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn installed_basic_block_tables_resolve_for_an_unenchanted_tool() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let mut provider = crate::packs::ResourceStack::open(&game).unwrap();
        let tables = BlockLootTables::load(&mut provider).unwrap();
        assert!(tables.warnings.is_empty(), "{:?}", tables.warnings);
        let tool = BasicTool {
            item: None,
            count: 0,
        };
        for (random, expected) in [(0.099_f32, "minecraft:flint"), (0.1, "minecraft:gravel")] {
            let mut calls = 0;
            let drops = tables
                .drops_with_random(
                    "minecraft:gravel",
                    tool,
                    Some(&mut || {
                        calls += 1;
                        random
                    }),
                )
                .unwrap();
            assert_eq!(
                drops,
                vec![Drop {
                    item: ResourceId::parse(expected).unwrap(),
                    count: 1
                }]
            );
            assert_eq!(calls, 1);
        }
        assert!(tables.drops("minecraft:gravel", tool).is_err());
        for (block, expected) in [
            ("grass_block", "dirt"),
            ("dirt", "dirt"),
            ("sand", "sand"),
            ("red_sand", "red_sand"),
            ("stone", "cobblestone"),
            ("stone_bricks", "stone_bricks"),
            ("stone_button", "stone_button"),
        ] {
            let drops = tables.drops(&format!("minecraft:{block}"), tool).unwrap();
            assert_eq!(
                drops,
                vec![Drop {
                    item: ResourceId::parse(&format!("minecraft:{expected}")).unwrap(),
                    count: 1
                }]
            );
        }
        for id in native_core::wood::Planks::ALL
            .into_iter()
            .map(|planks| planks.id())
            .chain(std::iter::once("minecraft:crafting_table"))
        {
            assert_eq!(
                tables.drops(id, tool).unwrap(),
                vec![Drop {
                    item: ResourceId::parse(id).unwrap(),
                    count: 1
                }],
                "{id}"
            );
        }
    }
}
