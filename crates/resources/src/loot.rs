//! A data-pack aware subset of 26.3 block loot.
//!
//! Unsupported conditions, random rolls and functions return an error instead of
//! inventing a drop. The evaluator will be extended alongside item components.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use crate::packs::{ResourceId, ResourceProvider};

const BASIC_BLOCKS: [&str; 38] = [
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
    "redstone_block",
    "redstone_lamp",
    "lever",
    "coal_ore",
    "deepslate_coal_ore",
    "iron_ore",
    "deepslate_iron_ore",
    "copper_ore",
    "deepslate_copper_ore",
    "gold_ore",
    "deepslate_gold_ore",
    "diamond_ore",
    "deepslate_diamond_ore",
    "emerald_ore",
    "deepslate_emerald_ore",
    "lapis_ore",
    "deepslate_lapis_ore",
    "redstone_ore",
    "deepslate_redstone_ore",
    "nether_gold_ore",
    "nether_quartz_ore",
    "chest",
    "trapped_chest",
    "snow",
];

/// Loot functions consume both floating-point and bounded integer random values.
pub trait LootRandom {
    fn next_float(&mut self) -> f32;
    fn next_int(&mut self, bound: i32) -> Option<i32>;
}

impl LootRandom for native_core::random::LegacyRandom {
    fn next_float(&mut self) -> f32 {
        self.next_float()
    }
    fn next_int(&mut self, bound: i32) -> Option<i32> {
        self.next_int_bound(bound)
    }
}

impl LootRandom for native_core::random::XoroshiroRandom {
    fn next_float(&mut self) -> f32 {
        self.next_float()
    }
    fn next_int(&mut self, bound: i32) -> Option<i32> {
        self.next_int_bound(bound)
    }
}

// Existing callers that only supply floats can still evaluate float-only tables.
impl<F: FnMut() -> f32> LootRandom for F {
    fn next_float(&mut self) -> f32 {
        self()
    }
    fn next_int(&mut self, _: i32) -> Option<i32> {
        None
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Drop {
    pub item: ResourceId,
    pub count: u32,
    /// The supported copy_components operation reads this from a block entity.
    pub copy_block_entity_custom_name: bool,
}

#[derive(Clone, Debug)]
enum Condition {
    Always,
    ThisEntityPresent,
    BlockState {
        blocks: Vec<ResourceId>,
        properties: BTreeMap<String, String>,
    },
    Tool(ToolPredicate),
    All(Vec<Condition>),
    Any(Vec<Condition>),
    Not(Box<Condition>),
    TableBonus(ResourceId, Vec<f32>),
    Unsupported(String),
}

#[derive(Clone, Debug, Default)]
struct ToolPredicate {
    items: Option<Vec<ResourceId>>,
    min_count: Option<u32>,
    max_count: Option<u32>,
    enchantments: Vec<EnchantmentPredicate>,
    unsupported: Option<String>,
}

#[derive(Clone, Debug)]
struct EnchantmentPredicate {
    ids: Option<BTreeSet<ResourceId>>,
    min_level: Option<i32>,
    max_level: Option<i32>,
}

#[derive(Clone, Copy)]
pub struct BasicTool<'a> {
    /// An empty hand is represented as `None` with count zero.
    pub item: Option<&'a str>,
    pub count: u32,
    /// The stack's stored enchantments and their levels.
    pub enchantments: &'a [(ResourceId, u32)],
}

/// Block state supplied by the event that produced loot.
pub struct LootBlockState<'a> {
    pub name: &'a str,
    pub properties: &'a BTreeMap<String, String>,
}

#[derive(Clone, Copy, Default)]
struct LootContext<'a> {
    entity_present: Option<bool>,
    block: Option<&'a LootBlockState<'a>>,
}

impl Condition {
    fn fully_supported(&self) -> bool {
        match self {
            Self::Unsupported(_) | Self::ThisEntityPresent | Self::BlockState { .. } => false,
            Self::Tool(predicate) => predicate.unsupported.is_none(),
            Self::All(values) | Self::Any(values) => values.iter().all(Self::fully_supported),
            Self::Not(value) => value.fully_supported(),
            _ => true,
        }
    }
    fn test(
        &self,
        tool: BasicTool<'_>,
        random: &mut Option<&mut dyn LootRandom>,
        context: LootContext<'_>,
    ) -> Result<bool> {
        match self {
            Self::Always => Ok(true),
            Self::ThisEntityPresent => context
                .entity_present
                .context("loot entity context is unavailable"),
            Self::BlockState { blocks, properties } => {
                let state = context.block.context("loot block state is unavailable")?;
                let id = ResourceId::parse(state.name)?;
                Ok(blocks.contains(&id)
                    && properties
                        .iter()
                        .all(|(key, value)| state.properties.get(key) == Some(value)))
            }
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
                {
                    return Ok(false);
                }
                for predicate in &predicate.enchantments {
                    if !tool.enchantments.iter().any(|(id, level)| {
                        predicate.ids.as_ref().is_none_or(|ids| ids.contains(id))
                            && predicate
                                .min_level
                                .is_none_or(|min| i64::from(*level) >= i64::from(min))
                            && predicate
                                .max_level
                                .is_none_or(|max| i64::from(*level) <= i64::from(max))
                    }) {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Self::All(conditions) => {
                for condition in conditions {
                    if !condition.test(tool, random, context)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Self::Any(conditions) => {
                for condition in conditions {
                    if condition.test(tool, random, context)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Self::Not(condition) => Ok(!condition.test(tool, random, context)?),
            Self::TableBonus(enchantment, chances) => {
                let level = tool
                    .enchantments
                    .iter()
                    .find(|(id, _)| id == enchantment)
                    .map_or(0, |(_, level)| *level as usize);
                let chance = chances[level.min(chances.len() - 1)];
                let random = random
                    .as_mut()
                    .context("random loot requires a random source")?;
                Ok(random.next_float() < chance)
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
    UniformCount(i32, i32, bool),
    OreDrops(ResourceId),
    UniformBonus(ResourceId, i32),
    ExplosionDecay,
    CopyBlockEntityCustomName,
    Unsupported(String),
}

impl Modifier {
    fn fully_supported(&self) -> bool {
        self.condition.fully_supported() && !matches!(self.kind, ModifierKind::Unsupported(_))
    }
    fn apply(
        &self,
        drop: &mut Drop,
        tool: BasicTool<'_>,
        random: &mut Option<&mut dyn LootRandom>,
        context: LootContext<'_>,
    ) -> Result<()> {
        if !self.condition.test(tool, random, context)? {
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
            ModifierKind::UniformCount(min, max, add) => {
                let count = if min >= max {
                    *min
                } else {
                    let bound = max
                        .checked_sub(*min)
                        .and_then(|value| value.checked_add(1))
                        .context("invalid uniform loot count")?;
                    random
                        .as_mut()
                        .context("random loot requires a random source")?
                        .next_int(bound)
                        .context("loot random source does not support bounded integers")?
                        + min
                };
                drop.count = if *add {
                    drop.count.saturating_add(count.max(0) as u32)
                } else {
                    count.max(0) as u32
                };
            }
            ModifierKind::OreDrops(enchantment) => {
                let level = tool
                    .enchantments
                    .iter()
                    .find(|(id, _)| id == enchantment)
                    .map_or(0, |(_, level)| *level);
                if level > 0 {
                    let bound = i32::try_from(level)?
                        .checked_add(2)
                        .context("fortune level overflow")?;
                    let n = random
                        .as_mut()
                        .context("random loot requires a random source")?
                        .next_int(bound)
                        .context("loot random source does not support bounded integers")?;
                    drop.count = drop.count.saturating_mul((n - 1).max(0) as u32 + 1);
                }
            }
            ModifierKind::UniformBonus(enchantment, multiplier) => {
                let level = tool
                    .enchantments
                    .iter()
                    .find(|(id, _)| id == enchantment)
                    .map_or(0, |(_, level)| *level);
                let bound = multiplier
                    .checked_mul(i32::try_from(level)?)
                    .and_then(|n| n.checked_add(1))
                    .context("invalid uniform bonus")?;
                let bonus = random
                    .as_mut()
                    .context("random loot requires a random source")?
                    .next_int(bound)
                    .context("loot random source does not support bounded integers")?;
                drop.count = drop.count.saturating_add(bonus as u32);
            }
            // Normal player mining does not have an explosion radius.
            ModifierKind::ExplosionDecay => {}
            ModifierKind::CopyBlockEntityCustomName => drop.copy_block_entity_custom_name = true,
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
    fn fully_supported(&self) -> bool {
        self.condition.fully_supported()
            && self.modifiers.iter().all(Modifier::fully_supported)
            && match &self.kind {
                EntryKind::Unsupported(_) => false,
                EntryKind::Alternatives(entries) => entries.iter().all(Self::fully_supported),
                _ => true,
            }
    }
    /// An expanded empty entry still stops an alternatives chain.
    fn expand(
        &self,
        tool: BasicTool<'_>,
        random: &mut Option<&mut dyn LootRandom>,
        context: LootContext<'_>,
    ) -> Result<Option<(u32, Option<Drop>)>> {
        if !self.condition.test(tool, random, context)? {
            return Ok(None);
        }
        let (weight, mut value) = match &self.kind {
            EntryKind::Item(id) => (
                self.weight,
                Some(Drop {
                    item: id.clone(),
                    count: 1,
                    copy_block_entity_custom_name: false,
                }),
            ),
            EntryKind::Empty => (self.weight, None),
            EntryKind::Alternatives(children) => {
                let mut selected = None;
                for child in children {
                    if let Some(value) = child.expand(tool, random, context)? {
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
                modifier.apply(drop, tool, random, context)?;
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
    /// Reject tables that could enter an unsupported branch under another RNG
    /// state before a destructive container operation is performed.
    pub fn fully_supported_block(&self, block: &str) -> bool {
        let Ok(block) = ResourceId::parse(block) else {
            return false;
        };
        let id = ResourceId {
            namespace: block.namespace,
            path: format!("blocks/{}", block.path),
        };
        self.tables.get(&id).is_some_and(|table| {
            table.modifiers.iter().all(Modifier::fully_supported)
                && table.pools.iter().all(|pool| {
                    pool.condition.fully_supported()
                        && pool.modifiers.iter().all(Modifier::fully_supported)
                        && pool.entries.len() <= 1
                        && pool.entries.iter().all(Entry::fully_supported)
                })
        })
    }
    /// Read tables and referenced predicates from the configured world data packs.
    pub fn load(provider: &mut impl ResourceProvider) -> Result<Self> {
        let mut result = Self::default();
        let names = BASIC_BLOCKS
            .into_iter()
            .chain(
                native_core::button::Button::ALL
                    .into_iter()
                    .map(|button| button.id().strip_prefix("minecraft:").unwrap()),
            )
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
        Self::evaluate(table, tool, None, LootContext::default())
    }

    pub fn drops(&self, block: &str, tool: BasicTool<'_>) -> Result<Vec<Drop>> {
        self.drops_with_random(block, tool, None)
    }

    /// Random source is session-local and is not yet the world's named loot sequence.
    pub fn drops_with_random(
        &self,
        block: &str,
        tool: BasicTool<'_>,
        random: Option<&mut dyn LootRandom>,
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
        Self::evaluate(table, tool, random, LootContext::default())
    }

    /// Evaluate a block drop caused by a non-entity event such as snow melting.
    pub fn drops_without_entity(
        &self,
        block: &str,
        random: Option<&mut dyn LootRandom>,
    ) -> Result<Vec<Drop>> {
        self.drops_without_entity_at(block, None, random)
    }

    /// Evaluate a non-entity block event with the original block properties.
    pub fn drops_without_entity_at(
        &self,
        block: &str,
        properties: Option<&BTreeMap<String, String>>,
        random: Option<&mut dyn LootRandom>,
    ) -> Result<Vec<Drop>> {
        self.drops_at(
            block,
            properties,
            BasicTool {
                item: None,
                count: 0,
                enchantments: &[],
            },
            false,
            random,
        )
    }

    /// Evaluate a player block break using the state and tool before destruction.
    pub fn drops_with_entity_at(
        &self,
        block: &str,
        properties: &BTreeMap<String, String>,
        tool: BasicTool<'_>,
        random: Option<&mut dyn LootRandom>,
    ) -> Result<Vec<Drop>> {
        self.drops_at(block, Some(properties), tool, true, random)
    }

    fn drops_at(
        &self,
        block: &str,
        properties: Option<&BTreeMap<String, String>>,
        tool: BasicTool<'_>,
        entity_present: bool,
        random: Option<&mut dyn LootRandom>,
    ) -> Result<Vec<Drop>> {
        let block = ResourceId::parse(block)?;
        let block_name = format!("{}:{}", block.namespace, block.path);
        let id = ResourceId {
            namespace: block.namespace,
            path: format!("blocks/{}", block.path),
        };
        let table = self
            .tables
            .get(&id)
            .context("block loot table is not supported or was not loaded")?;
        let state = properties.map(|properties| LootBlockState {
            name: &block_name,
            properties,
        });
        Self::evaluate(
            table,
            tool,
            random,
            LootContext {
                entity_present: Some(entity_present),
                block: state.as_ref(),
            },
        )
    }

    fn evaluate(
        table: &Table,
        tool: BasicTool<'_>,
        mut random: Option<&mut dyn LootRandom>,
        context: LootContext<'_>,
    ) -> Result<Vec<Drop>> {
        let mut drops = Vec::new();
        for pool in &table.pools {
            if !pool.condition.test(tool, &mut random, context)? {
                continue;
            }
            for _ in 0..pool.rolls {
                let mut selected = None;
                let mut expanded = 0;
                for entry in &pool.entries {
                    if let Some((weight, value)) = entry.expand(tool, &mut random, context)?
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
                        modifier.apply(&mut drop, tool, &mut random, context)?;
                    }
                    for modifier in &table.modifiers {
                        modifier.apply(&mut drop, tool, &mut random, context)?;
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
                    _ => {
                        let count = modifier.get("count").context("missing set_count value")?;
                        if count.get("type").and_then(Value::as_str) == Some("minecraft:uniform") {
                            let min = i32::try_from(
                                count
                                    .get("min")
                                    .and_then(Value::as_i64)
                                    .context("invalid uniform minimum")?,
                            )?;
                            let max = i32::try_from(
                                count
                                    .get("max")
                                    .and_then(Value::as_i64)
                                    .context("invalid uniform maximum")?,
                            )?;
                            let add = modifier
                                .get("add")
                                .map(|value| value.as_bool().context("invalid set_count add flag"))
                                .transpose()?
                                .unwrap_or(false);
                            ModifierKind::UniformCount(min, max, add)
                        } else {
                            ModifierKind::Unsupported("non-constant set_count".into())
                        }
                    }
                },
                "minecraft:apply_bonus" => {
                    let id = ResourceId::parse(
                        modifier
                            .get("enchantment")
                            .and_then(Value::as_str)
                            .context("missing bonus enchantment")?,
                    )?;
                    match modifier.get("formula").and_then(Value::as_str) {
                        Some("minecraft:ore_drops") => ModifierKind::OreDrops(id),
                        Some("minecraft:uniform_bonus_count") => {
                            let multiplier = modifier
                                .get("parameters")
                                .and_then(|value| value.get("bonusMultiplier"))
                                .and_then(Value::as_i64)
                                .context("missing bonus multiplier")?;
                            ModifierKind::UniformBonus(id, i32::try_from(multiplier)?)
                        }
                        _ => ModifierKind::Unsupported("unsupported apply_bonus formula".into()),
                    }
                }
                "minecraft:explosion_decay" => ModifierKind::ExplosionDecay,
                "minecraft:copy_components" => {
                    let source = modifier.get("source").and_then(Value::as_str);
                    let include = modifier.get("include").and_then(Value::as_array);
                    if source == Some("block_entity")
                        && include.is_some_and(|values| {
                            values.len() == 1 && values[0].as_str() == Some("minecraft:custom_name")
                        })
                        && modifier.as_object().is_some_and(|fields| {
                            fields.keys().all(|key| {
                                matches!(key.as_str(), "type" | "source" | "include" | "condition")
                            })
                        })
                    {
                        ModifierKind::CopyBlockEntityCustomName
                    } else {
                        ModifierKind::Unsupported("unsupported copy_components".into())
                    }
                }
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
        "minecraft:match_block" => {
            let object = value.as_object().context("invalid block predicate")?;
            if object
                .keys()
                .any(|key| !matches!(key.as_str(), "type" | "blocks" | "state"))
            {
                Condition::Unsupported("unknown match_block field".into())
            } else {
                let ids = value
                    .get("blocks")
                    .context("block predicate has no blocks")?;
                let ids: Vec<&Value> = match ids {
                    Value::Array(ids) => ids.iter().collect(),
                    _ => vec![ids],
                };
                if ids.is_empty()
                    || ids.len() > 128
                    || ids
                        .iter()
                        .any(|id| id.as_str().is_none_or(|id| id.starts_with('#')))
                {
                    Condition::Unsupported("unsupported block selector".into())
                } else {
                    let blocks = ids
                        .into_iter()
                        .map(|id| ResourceId::parse(id.as_str().unwrap()))
                        .collect::<Result<_>>()?;
                    let mut properties = BTreeMap::new();
                    let mut supported = true;
                    if let Some(value) = value.get("state") {
                        let state = value.as_object().context("invalid block state predicate")?;
                        for (key, value) in state {
                            if let Some(value) = value.as_str() {
                                properties.insert(key.clone(), value.to_owned());
                            } else {
                                supported = false;
                            }
                        }
                    }
                    if supported {
                        Condition::BlockState { blocks, properties }
                    } else {
                        Condition::Unsupported("unsupported block property predicate".into())
                    }
                }
            }
        }
        "minecraft:entity_properties"
            if value.get("entity").and_then(Value::as_str) == Some("this")
                && value
                    .get("predicate")
                    .is_some_and(|p| p.as_object().is_some_and(|p| p.is_empty())) =>
        {
            Condition::ThisEntityPresent
        }
        "minecraft:match_tool" => Condition::Tool(parse_tool(value.get("predicate"), provider)?),
        "minecraft:table_bonus" => {
            let enchantment = ResourceId::parse(
                value
                    .get("enchantment")
                    .and_then(Value::as_str)
                    .context("table bonus has no enchantment")?,
            )?;
            let chances = value
                .get("chances")
                .and_then(Value::as_array)
                .context("table bonus has no chances")?;
            ensure!(
                !chances.is_empty() && chances.len() <= 128,
                "invalid table bonus chances"
            );
            let chances = chances
                .iter()
                .map(|value| {
                    let chance = value.as_f64().context("invalid table bonus chance")?;
                    Ok(chance as f32)
                })
                .collect::<Result<Vec<_>>>()?;
            Condition::TableBonus(enchantment, chances)
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

fn parse_tool(
    value: Option<&Value>,
    provider: &mut impl ResourceProvider,
) -> Result<ToolPredicate> {
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
                let object = enchantment
                    .as_object()
                    .context("enchantment predicate is not an object")?;
                if object
                    .keys()
                    .any(|key| key != "enchantments" && key != "levels")
                {
                    result.unsupported = Some("enchantment predicate field".into());
                    continue;
                }
                let ids = if let Some(value) = object.get("enchantments") {
                    let values: Vec<&Value> = match value {
                        Value::Array(values) => values.iter().collect(),
                        _ => vec![value],
                    };
                    let mut ids = BTreeSet::new();
                    for value in values {
                        let name = value.as_str().context("invalid enchantment selector")?;
                        if let Some(tag) = name.strip_prefix('#') {
                            for id in crate::enchantments::load_single_tag(
                                provider,
                                "enchantment",
                                &ResourceId::parse(tag)?,
                            )? {
                                ids.insert(ResourceId::parse(&id)?);
                            }
                        } else {
                            ids.insert(ResourceId::parse(name)?);
                        }
                    }
                    Some(ids)
                } else {
                    None
                };
                let mut min_level = None;
                let mut max_level = None;
                if let Some(levels) = object.get("levels") {
                    let bound = |value: &Value| -> Result<i32> {
                        Ok(i32::try_from(
                            value.as_i64().context("invalid enchantment level")?,
                        )?)
                    };
                    if levels.is_number() {
                        let level = bound(levels)?;
                        min_level = Some(level);
                        max_level = Some(level);
                    } else {
                        let bounds = levels
                            .as_object()
                            .context("invalid enchantment level bounds")?;
                        ensure!(
                            bounds.keys().all(|key| key == "min" || key == "max"),
                            "unknown enchantment level bound"
                        );
                        min_level = bounds.get("min").map(bound).transpose()?;
                        max_level = bounds.get("max").map(bound).transpose()?;
                    }
                }
                result.enchantments.push(EnchantmentPredicate {
                    ids,
                    min_level,
                    max_level,
                });
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
    fn block_state_conditions_evaluate_original_properties_and_reject_missing_context() {
        let mut source = Memory::default();
        source.put("data/minecraft/loot_table/blocks/snow.json", r#"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:stick","condition":{"type":"minecraft:match_block","blocks":"minecraft:snow","state":{"layers":"3"}}}]}]}"#);
        let tables = BlockLootTables::load(&mut source).unwrap();
        assert!(tables.drops_without_entity("minecraft:snow", None).is_err());
        for layers in 1..=8 {
            let properties = BTreeMap::from([("layers".into(), layers.to_string())]);
            let drops = tables
                .drops_without_entity_at("minecraft:snow", Some(&properties), None)
                .unwrap();
            assert_eq!(drops.len(), usize::from(layers == 3));
        }
        let properties = BTreeMap::new();
        assert!(
            tables
                .drops_without_entity_at("minecraft:snow", Some(&properties), None)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn non_entity_snow_loot_honors_conditions_and_custom_tables() {
        let mut source = Memory::default();
        source.put("data/minecraft/loot_table/blocks/snow.json", r#"{"type":"minecraft:block","pools":[{"condition":{"type":"minecraft:entity_properties","entity":"this","predicate":{}},"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:diamond"}]},{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:stick"}]}]}"#);
        let tables = BlockLootTables::load(&mut source).unwrap();
        let drops = tables.drops_without_entity("minecraft:snow", None).unwrap();
        assert_eq!(drops.len(), 1);
        assert_eq!(drops[0].item, ResourceId::parse("minecraft:stick").unwrap());
        let tool = BasicTool {
            item: None,
            count: 0,
            enchantments: &[],
        };
        assert!(tables.drops("minecraft:snow", tool).is_err());
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn installed_snow_melt_loot_has_no_entity_drop() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let mut source = crate::packs::ResourceStack::open(&game).unwrap();
        let tables = BlockLootTables::load(&mut source).unwrap();
        assert!(
            tables
                .drops_without_entity("minecraft:snow", None)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn chest_loot_retains_custom_name_copy_and_rejects_unknown_component_rules() {
        let mut source = Memory::default();
        source.put("data/minecraft/loot_table/blocks/chest.json", r#"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:diamond","modifier":{"type":"minecraft:copy_components","source":"block_entity","include":["minecraft:custom_name"]}}]}]}"#);
        source.put("data/minecraft/loot_table/blocks/trapped_chest.json", r#"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:diamond","modifier":{"type":"minecraft:copy_components","source":"block_entity","include":["minecraft:lore"]}}]}]}"#);
        let tables = BlockLootTables::load(&mut source).unwrap();
        let tool = BasicTool {
            item: None,
            count: 0,
            enchantments: &[],
        };
        let drops = tables.drops("minecraft:chest", tool).unwrap();
        assert!(tables.fully_supported_block("minecraft:chest"));
        assert!(!tables.fully_supported_block("minecraft:trapped_chest"));
        assert_eq!(drops.len(), 1);
        assert_eq!(
            drops[0].item,
            ResourceId::parse("minecraft:diamond").unwrap()
        );
        assert!(drops[0].copy_block_entity_custom_name);
        assert!(tables.drops("minecraft:trapped_chest", tool).is_err());
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
            enchantments: &[],
        };
        assert_eq!(
            tables.drops("minecraft:dirt", pick(None)).unwrap(),
            vec![Drop {
                item: ResourceId::parse("minecraft:dirt").unwrap(),
                count: 2,
                copy_block_entity_custom_name: false,
            }]
        );
        assert_eq!(
            tables
                .drops("minecraft:dirt", pick(Some("minecraft:stone")))
                .unwrap(),
            vec![Drop {
                item: ResourceId::parse("minecraft:stone").unwrap(),
                count: 2,
                copy_block_entity_custom_name: false,
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
                        count: 0,
                        enchantments: &[],
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
                        count: 0,
                        enchantments: &[],
                    }
                )
                .is_err()
        );
    }

    #[test]
    fn enchantment_predicates_honor_tags_bounds_and_other_enchantments() {
        let mut source = Memory::default();
        source.put(
            "data/example/tags/enchantment/special.json",
            r#"{"values":["minecraft:fortune", "minecraft:efficiency"]}"#,
        );
        source.put("data/minecraft/loot_table/blocks/dirt.json", r##"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:alternatives","children":[{"type":"minecraft:item","name":"minecraft:stone","condition":{"type":"minecraft:match_tool","predicate":{"predicates":{"minecraft:enchantments":[{"enchantments":"#example:special","levels":{"min":2,"max":3}}]}}}},{"type":"minecraft:item","name":"minecraft:dirt"}]}]}]}"##);
        let tables = BlockLootTables::load(&mut source).unwrap();
        for (name, level, expected) in [
            ("minecraft:fortune", 1, "minecraft:dirt"),
            ("minecraft:fortune", 2, "minecraft:stone"),
            ("minecraft:efficiency", 3, "minecraft:stone"),
            ("minecraft:efficiency", 4, "minecraft:dirt"),
            ("minecraft:unbreaking", 2, "minecraft:dirt"),
        ] {
            let enchantments = [(ResourceId::parse(name).unwrap(), level)];
            let tool = BasicTool {
                item: Some("minecraft:iron_pickaxe"),
                count: 1,
                enchantments: &enchantments,
            };
            assert_eq!(
                tables.drops("minecraft:dirt", tool).unwrap()[0].item,
                ResourceId::parse(expected).unwrap()
            );
        }
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn installed_loot_uses_silk_touch_and_fortune_levels() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let mut provider = crate::packs::ResourceStack::open(&game).unwrap();
        let tables = BlockLootTables::load(&mut provider).unwrap();
        let silk = [(ResourceId::parse("minecraft:silk_touch").unwrap(), 1)];
        let tool = BasicTool {
            item: Some("minecraft:iron_pickaxe"),
            count: 1,
            enchantments: &silk,
        };
        for block in ["stone", "grass_block", "gravel"] {
            let mut calls = 0;
            let drops = tables
                .drops_with_random(
                    &format!("minecraft:{block}"),
                    tool,
                    Some(&mut || {
                        calls += 1;
                        0.0
                    }),
                )
                .unwrap();
            assert_eq!(
                drops[0].item,
                ResourceId::parse(&format!("minecraft:{block}")).unwrap()
            );
            assert_eq!(
                calls, 0,
                "silk touch must short-circuit gravel's random condition"
            );
        }
        for (level, random, expected) in [
            (0, 0.099_f32, "flint"),
            (0, 0.1, "gravel"),
            (1, 0.14, "flint"),
            (1, 0.15, "gravel"),
            (2, 0.249, "flint"),
            (2, 0.25, "gravel"),
            (3, 0.999, "flint"),
            (255, 0.999, "flint"),
        ] {
            let enchantments = if level == 0 {
                vec![]
            } else {
                vec![(ResourceId::parse("minecraft:fortune").unwrap(), level)]
            };
            let tool = BasicTool {
                item: Some("minecraft:iron_shovel"),
                count: 1,
                enchantments: &enchantments,
            };
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
                drops[0].item,
                ResourceId::parse(&format!("minecraft:{expected}")).unwrap()
            );
            assert_eq!(calls, 1);
        }
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
            enchantments: &[],
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
                    count: 1,
                    copy_block_entity_custom_name: false,
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
                    count: 1,
                    copy_block_entity_custom_name: false,
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
                    count: 1,
                    copy_block_entity_custom_name: false,
                }],
                "{id}"
            );
        }
    }
}
