//! A data-pack aware subset of 26.3 block loot.
//!
//! Unsupported conditions, random rolls and functions return an error instead of
//! inventing a drop. The evaluator will be extended alongside item components.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use crate::packs::{ResourceId, ResourceProvider};

const BASIC_BLOCKS: [&str; 46] = [
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
    "lodestone",
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
    "snow_block",
    "honey_block",
    "short_grass",
    "fern",
    "dead_bush",
    "fire",
    "soul_fire",
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
    /// Component copy operations that require an explicit block-entity context.
    pub copy_block_entity_components: Vec<ComponentFilter>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComponentFilter {
    pub include: Option<BTreeSet<ResourceId>>,
    pub exclude: BTreeSet<ResourceId>,
}

impl ComponentFilter {
    pub fn accepts(&self, component: &ResourceId) -> bool {
        self.include
            .as_ref()
            .is_none_or(|ids| ids.contains(component))
            && !self.exclude.contains(component)
    }
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
    RandomChance(f32),
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
    block_entity_components: bool,
}

impl Condition {
    fn fully_supported_in_interaction(&self) -> bool {
        match self {
            Self::ThisEntityPresent | Self::Unsupported(_) => false,
            Self::BlockState { .. } => true,
            Self::All(values) | Self::Any(values) => {
                values.iter().all(Self::fully_supported_in_interaction)
            }
            Self::Not(value) => value.fully_supported_in_interaction(),
            _ => self.fully_supported(),
        }
    }
    fn fully_supported(&self) -> bool {
        self.fully_supported_in_player_block(false)
    }
    fn fully_supported_in_player_block(&self, player_block: bool) -> bool {
        match self {
            Self::Unsupported(_) => false,
            Self::ThisEntityPresent | Self::BlockState { .. } => player_block,
            Self::Tool(predicate) => predicate.unsupported.is_none(),
            Self::All(values) | Self::Any(values) => values
                .iter()
                .all(|value| value.fully_supported_in_player_block(player_block)),
            Self::Not(value) => value.fully_supported_in_player_block(player_block),
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
            Self::RandomChance(chance) => {
                let random = random
                    .as_mut()
                    .context("random loot requires a random source")?;
                Ok(random.next_float() < *chance)
            }
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
    CopyBlockEntityComponents(ComponentFilter),
    Unsupported(String),
}

impl Modifier {
    fn fully_supported(&self) -> bool {
        self.condition.fully_supported()
            && !matches!(
                self.kind,
                ModifierKind::Unsupported(_) | ModifierKind::CopyBlockEntityComponents(_)
            )
    }
    fn fully_supported_in_player_block_with_components(&self, components: bool) -> bool {
        self.condition.fully_supported_in_player_block(true)
            && !matches!(self.kind, ModifierKind::Unsupported(_))
            && (components || !matches!(self.kind, ModifierKind::CopyBlockEntityComponents(_)))
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
            ModifierKind::CopyBlockEntityComponents(filter) => {
                ensure!(
                    context.block_entity_components,
                    "block entity component copy context is unavailable"
                );
                drop.copy_block_entity_components.push(filter.clone());
            }
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

struct ExpandedEntry<'a> {
    weight: u32,
    item: Option<&'a ResourceId>,
    modifiers: Vec<&'a [Modifier]>,
}

impl Entry {
    fn maximum_weight(&self) -> Option<i32> {
        match &self.kind {
            EntryKind::Alternatives(children) => children.iter().try_fold(0, |maximum, child| {
                Some(maximum.max(child.maximum_weight()?))
            }),
            _ => i32::try_from(self.weight).ok(),
        }
    }
    fn potential_component_drops(&self) -> Result<Vec<Drop>> {
        if self.weight == 0 && !matches!(self.kind, EntryKind::Alternatives(_)) {
            return Ok(Vec::new());
        }
        let mut drops = match &self.kind {
            EntryKind::Item(item) => vec![Drop {
                item: item.clone(),
                count: 1,
                copy_block_entity_custom_name: false,
                copy_block_entity_components: Vec::new(),
            }],
            EntryKind::Empty => Vec::new(),
            EntryKind::Alternatives(entries) => entries
                .iter()
                .map(Entry::potential_component_drops)
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .flatten()
                .collect(),
            EntryKind::Unsupported(reason) => bail!("unsupported loot entry: {reason}"),
        };
        for drop in &mut drops {
            add_possible_component_copies(drop, &self.modifiers)?;
        }
        Ok(drops)
    }
    fn fully_supported(&self) -> bool {
        self.condition.fully_supported()
            && self.modifiers.iter().all(Modifier::fully_supported)
            && match &self.kind {
                EntryKind::Unsupported(_) => false,
                EntryKind::Alternatives(entries) => entries.iter().all(Self::fully_supported),
                _ => true,
            }
    }
    fn fully_supported_in_player_block_with_components(&self, components: bool) -> bool {
        self.condition.fully_supported_in_player_block(true)
            && self.modifiers.iter().all(|modifier| {
                modifier.fully_supported_in_player_block_with_components(components)
            })
            && match &self.kind {
                EntryKind::Unsupported(_) => false,
                EntryKind::Alternatives(entries) => entries
                    .iter()
                    .all(|entry| entry.fully_supported_in_player_block_with_components(components)),
                _ => true,
            }
    }
    /// An expanded empty entry still stops an alternatives chain.
    fn expand<'a>(
        &'a self,
        tool: BasicTool<'_>,
        random: &mut Option<&mut dyn LootRandom>,
        context: LootContext<'_>,
    ) -> Result<Option<ExpandedEntry<'a>>> {
        if !self.condition.test(tool, random, context)? {
            return Ok(None);
        }
        let mut value = match &self.kind {
            EntryKind::Item(id) => ExpandedEntry {
                weight: self.weight,
                item: Some(id),
                modifiers: Vec::new(),
            },
            EntryKind::Empty => ExpandedEntry {
                weight: self.weight,
                item: None,
                modifiers: Vec::new(),
            },
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
        if !self.modifiers.is_empty() {
            value.modifiers.push(&self.modifiers);
        }
        Ok(Some(value))
    }
}

/// Conservatively include every conditional component copy. Count modifiers
/// are inspected for invalid random bounds, but do not affect stack validity.
fn add_possible_component_copies(drop: &mut Drop, modifiers: &[Modifier]) -> Result<()> {
    for modifier in modifiers {
        match &modifier.kind {
            ModifierKind::CopyBlockEntityCustomName => drop.copy_block_entity_custom_name = true,
            ModifierKind::CopyBlockEntityComponents(filter) => {
                drop.copy_block_entity_components.push(filter.clone())
            }
            ModifierKind::UniformCount(min, max, _) if min < max => {
                max.checked_sub(*min)
                    .and_then(|v| v.checked_add(1))
                    .context("invalid uniform loot count")?;
            }
            ModifierKind::Unsupported(reason) => bail!("unsupported loot modifier: {reason}"),
            _ => {}
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct Pool {
    condition: Condition,
    entries: Vec<Entry>,
    modifiers: Vec<Modifier>,
    rolls: u32,
}

impl Pool {
    fn weights_fit(&self) -> bool {
        self.entries
            .iter()
            .try_fold(0i32, |total, entry| {
                total.checked_add(entry.maximum_weight()?)
            })
            .is_some()
    }
}

#[derive(Clone, Debug)]
struct Table {
    pools: Vec<Pool>,
    modifiers: Vec<Modifier>,
    random_sequence: Option<ResourceId>,
}

#[derive(Clone, Debug, Default)]
pub struct BlockLootTables {
    tables: BTreeMap<ResourceId, Table>,
    interaction_tables: BTreeMap<ResourceId, Table>,
    pub warnings: Vec<String>,
}

impl BlockLootTables {
    /// Return the named stream specified by the active block loot table.
    pub fn random_sequence_for_block(&self, block: &str) -> Result<Option<&ResourceId>> {
        let block = ResourceId::parse(block)?;
        let id = ResourceId {
            namespace: block.namespace,
            path: format!("blocks/{}", block.path),
        };
        Ok(self
            .tables
            .get(&id)
            .context("block loot table unavailable")?
            .random_sequence
            .as_ref())
    }
    /// Check all branches using the entity and original state supplied by a player break.
    pub fn fully_supported_player_block(&self, block: &str) -> bool {
        self.fully_supported_player_block_inner(block, false)
    }

    /// Check every possible branch against the actual tool before a block edit.
    pub fn fully_supported_player_block_for_tool(&self, block: &str, tool: BasicTool<'_>) -> bool {
        self.fully_supported_block_for_tool_inner(block, tool, false)
    }

    /// As above, allowing supported component copies from a block entity.
    pub fn fully_supported_block_entity_for_tool(&self, block: &str, tool: BasicTool<'_>) -> bool {
        self.fully_supported_block_for_tool_inner(block, tool, true)
    }

    fn fully_supported_block_for_tool_inner(
        &self,
        block: &str,
        tool: BasicTool<'_>,
        components: bool,
    ) -> bool {
        fn modifier_valid(modifier: &Modifier, tool: BasicTool<'_>) -> bool {
            match &modifier.kind {
                ModifierKind::UniformCount(min, max, _) if min < max => max
                    .checked_sub(*min)
                    .and_then(|value| value.checked_add(1))
                    .is_some(),
                ModifierKind::UniformBonus(enchantment, multiplier) => {
                    let level = tool
                        .enchantments
                        .iter()
                        .find(|(id, _)| id == enchantment)
                        .map_or(0, |(_, level)| *level);
                    i32::try_from(level)
                        .ok()
                        .and_then(|level| multiplier.checked_mul(level))
                        .and_then(|value| value.checked_add(1))
                        .is_some_and(|bound| bound > 0)
                }
                ModifierKind::OreDrops(enchantment) => {
                    let level = tool
                        .enchantments
                        .iter()
                        .find(|(id, _)| id == enchantment)
                        .map_or(0, |(_, level)| *level);
                    i32::try_from(level)
                        .ok()
                        .and_then(|level| level.checked_add(2))
                        .is_some()
                }
                _ => true,
            }
        }
        fn entry_valid(entry: &Entry, tool: BasicTool<'_>) -> bool {
            entry
                .modifiers
                .iter()
                .all(|modifier| modifier_valid(modifier, tool))
                && match &entry.kind {
                    EntryKind::Alternatives(children) => {
                        children.iter().all(|child| entry_valid(child, tool))
                    }
                    _ => true,
                }
        }
        let Ok(block) = ResourceId::parse(block) else {
            return false;
        };
        let name = format!("{}:{}", block.namespace, block.path);
        if !self.fully_supported_player_block_inner(&name, components) {
            return false;
        }
        let id = ResourceId {
            namespace: block.namespace,
            path: format!("blocks/{}", block.path),
        };
        self.tables.get(&id).is_some_and(|table| {
            table
                .modifiers
                .iter()
                .all(|modifier| modifier_valid(modifier, tool))
                && table.pools.iter().all(|pool| {
                    pool.modifiers
                        .iter()
                        .all(|modifier| modifier_valid(modifier, tool))
                        && pool.entries.iter().all(|entry| entry_valid(entry, tool))
                })
        })
    }
    pub fn fully_supported_player_block_with_components(&self, block: &str) -> bool {
        self.fully_supported_player_block_inner(block, true)
    }

    /// All possible item/component combinations, independent of conditions and
    /// RNG. Used to fail closed before a support update can drop several blocks.
    pub fn potential_block_entity_drops(&self, block: &str) -> Result<Vec<Drop>> {
        ensure!(
            self.fully_supported_player_block_with_components(block),
            "unsupported block loot table"
        );
        let block = ResourceId::parse(block)?;
        let id = ResourceId {
            namespace: block.namespace,
            path: format!("blocks/{}", block.path),
        };
        let table = self
            .tables
            .get(&id)
            .context("block loot table unavailable")?;
        let mut result = Vec::new();
        for pool in &table.pools {
            if pool.rolls == 0 {
                continue;
            }
            for entry in &pool.entries {
                for mut drop in entry.potential_component_drops()? {
                    add_possible_component_copies(&mut drop, &pool.modifiers)?;
                    add_possible_component_copies(&mut drop, &table.modifiers)?;
                    result.push(drop);
                }
            }
        }
        Ok(result)
    }
    fn fully_supported_player_block_inner(&self, block: &str, components: bool) -> bool {
        let Ok(block) = ResourceId::parse(block) else {
            return false;
        };
        let id = ResourceId {
            namespace: block.namespace,
            path: format!("blocks/{}", block.path),
        };
        self.tables.get(&id).is_some_and(|table| {
            table.modifiers.iter().all(|modifier| {
                modifier.fully_supported_in_player_block_with_components(components)
            }) && table.pools.iter().all(|pool| {
                pool.condition.fully_supported_in_player_block(true)
                    && pool.modifiers.iter().all(|modifier| {
                        modifier.fully_supported_in_player_block_with_components(components)
                    })
                    && pool.weights_fit()
                    && pool.entries.iter().all(|entry| {
                        entry.fully_supported_in_player_block_with_components(components)
                    })
            })
        })
    }
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
                        && pool.weights_fit()
                        && pool.entries.iter().all(Entry::fully_supported)
                })
        })
    }
    /// Read tables and referenced predicates from the configured world data packs.
    pub fn load(provider: &mut impl ResourceProvider) -> Result<Self> {
        let mut result = Self::default();
        let names =
            BASIC_BLOCKS
                .into_iter()
                .chain(native_core::stairs::ITEMS.iter().copied())
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
                )
                .map(str::to_owned)
                .chain(native_core::stairs::ITEMS.iter().map(|name| {
                    format!("{}_slab", name.strip_suffix("_stairs").expect("stair item"))
                }))
                .chain(
                    [
                        "cut_red_sandstone_slab",
                        "cut_sandstone_slab",
                        "petrified_oak_slab",
                        "smooth_stone_slab",
                    ]
                    .map(str::to_owned),
                )
                .chain(
                    [
                        "white",
                        "orange",
                        "magenta",
                        "light_blue",
                        "yellow",
                        "lime",
                        "pink",
                        "gray",
                        "light_gray",
                        "cyan",
                        "purple",
                        "blue",
                        "brown",
                        "green",
                        "red",
                        "black",
                    ]
                    .map(|color| format!("{color}_banner")),
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
        self.interaction_drops_with_random(id, tool, None, None)
    }

    pub fn interaction_random_sequence(&self, id: &ResourceId) -> Result<Option<&ResourceId>> {
        Ok(self
            .interaction_tables
            .get(id)
            .context("interaction loot table unavailable")?
            .random_sequence
            .as_ref())
    }

    /// Validate every branch before an interaction modifies the world.
    pub fn fully_supported_interaction_for_tool(
        &self,
        id: &ResourceId,
        tool: BasicTool<'_>,
    ) -> bool {
        fn modifier_valid(modifier: &Modifier, tool: BasicTool<'_>) -> bool {
            if !modifier.condition.fully_supported_in_interaction()
                || matches!(
                    modifier.kind,
                    ModifierKind::Unsupported(_) | ModifierKind::CopyBlockEntityComponents(_)
                )
                || matches!(modifier.kind, ModifierKind::CopyBlockEntityCustomName)
            {
                return false;
            }
            match &modifier.kind {
                ModifierKind::UniformCount(min, max, _) if min < max => max
                    .checked_sub(*min)
                    .and_then(|v| v.checked_add(1))
                    .is_some(),
                ModifierKind::UniformBonus(enchantment, multiplier) => {
                    let level = tool
                        .enchantments
                        .iter()
                        .find(|(id, _)| id == enchantment)
                        .map_or(0, |(_, level)| *level);
                    i32::try_from(level)
                        .ok()
                        .and_then(|v| multiplier.checked_mul(v))
                        .and_then(|v| v.checked_add(1))
                        .is_some_and(|v| v > 0)
                }
                ModifierKind::OreDrops(enchantment) => {
                    let level = tool
                        .enchantments
                        .iter()
                        .find(|(id, _)| id == enchantment)
                        .map_or(0, |(_, level)| *level);
                    i32::try_from(level)
                        .ok()
                        .and_then(|v| v.checked_add(2))
                        .is_some()
                }
                _ => true,
            }
        }
        fn entry_valid(entry: &Entry, tool: BasicTool<'_>) -> bool {
            entry.condition.fully_supported_in_interaction()
                && entry.modifiers.iter().all(|m| modifier_valid(m, tool))
                && match &entry.kind {
                    EntryKind::Unsupported(_) => false,
                    EntryKind::Alternatives(children) => {
                        children.iter().all(|child| entry_valid(child, tool))
                    }
                    _ => true,
                }
        }
        self.interaction_tables.get(id).is_some_and(|table| {
            table.modifiers.iter().all(|m| modifier_valid(m, tool))
                && table.pools.iter().all(|pool| {
                    pool.condition.fully_supported_in_interaction()
                        && pool.modifiers.iter().all(|m| modifier_valid(m, tool))
                        && pool.weights_fit()
                        && pool.entries.iter().all(|entry| entry_valid(entry, tool))
                })
        })
    }

    pub fn interaction_drops_with_random(
        &self,
        id: &ResourceId,
        tool: BasicTool<'_>,
        block: Option<&LootBlockState<'_>>,
        random: Option<&mut dyn LootRandom>,
    ) -> Result<Vec<Drop>> {
        let table = self
            .interaction_tables
            .get(id)
            .context("interaction loot table is not supported or was not loaded")?;
        Self::evaluate(
            table,
            tool,
            random,
            LootContext {
                block,
                ..LootContext::default()
            },
        )
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
        self.drops_at(block, Some(properties), tool, true, false, random)
    }

    /// Evaluate a block table while retaining explicit component-copy filters.
    /// `table_block` may differ from `state.name`, as wall banners use their
    /// standing banner's loot table.
    pub fn drops_with_block_entity_at(
        &self,
        table_block: &str,
        state: &LootBlockState<'_>,
        tool: BasicTool<'_>,
        entity_present: bool,
        random: Option<&mut dyn LootRandom>,
    ) -> Result<Vec<Drop>> {
        let block = ResourceId::parse(table_block)?;
        let id = ResourceId {
            namespace: block.namespace,
            path: format!("blocks/{}", block.path),
        };
        let table = self
            .tables
            .get(&id)
            .context("block loot table is not supported or was not loaded")?;
        Self::evaluate(
            table,
            tool,
            random,
            LootContext {
                entity_present: Some(entity_present),
                block: Some(state),
                block_entity_components: true,
            },
        )
    }

    fn drops_at(
        &self,
        block: &str,
        properties: Option<&BTreeMap<String, String>>,
        tool: BasicTool<'_>,
        entity_present: bool,
        block_entity_components: bool,
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
                block_entity_components,
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
            let mut candidates = Vec::with_capacity(pool.entries.len());
            for _ in 0..pool.rolls {
                candidates.clear();
                let mut total = 0i32;
                for entry in &pool.entries {
                    if let Some(value) = entry.expand(tool, &mut random, context)?
                        && value.weight > 0
                    {
                        total = total
                            .checked_add(
                                i32::try_from(value.weight).context("loot weight too large")?,
                            )
                            .context("loot weight overflow")?;
                        candidates.push(value);
                    }
                }
                let selected = match candidates.len() {
                    0 => continue,
                    1 => &candidates[0],
                    _ => {
                        let mut value = random
                            .as_deref_mut()
                            .context("random loot requires a random source")?
                            .next_int(total)
                            .context("loot random source does not support bounded integers")?;
                        ensure!(
                            (0..total).contains(&value),
                            "loot random source returned an invalid value"
                        );
                        candidates
                            .iter()
                            .find(|entry| {
                                value -= entry.weight as i32;
                                value < 0
                            })
                            .context("loot selection exceeded its weights")?
                    }
                };
                if let Some(item) = selected.item {
                    let mut drop = Drop {
                        item: item.clone(),
                        count: 1,
                        copy_block_entity_custom_name: false,
                        copy_block_entity_components: Vec::new(),
                    };
                    for modifiers in &selected.modifiers {
                        for modifier in *modifiers {
                            modifier.apply(&mut drop, tool, &mut random, context)?;
                        }
                    }
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
    let pools = match json.get("pools") {
        None => &[][..],
        Some(value) => value.as_array().context("invalid loot pools")?.as_slice(),
    };
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
        random_sequence: json
            .get("random_sequence")
            .map(|value| ResourceId::parse(value.as_str().context("invalid loot random sequence")?))
            .transpose()?,
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
        .unwrap_or(1);
    let weight = i32::try_from(weight).context("loot weight out of range")?;
    if let Some(quality) = json.get("quality") {
        i32::try_from(quality.as_i64().context("invalid loot quality")?)
            .context("loot quality out of range")?;
    }
    // Block and block-interaction LootParams use the builder's default luck
    // of zero. Java still converts the weight to f32 before flooring it.
    let weight = ((weight as f32) as i32).max(0) as u32;
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
                    } else if source == Some("block_entity")
                        && modifier.as_object().is_some_and(|fields| {
                            fields.keys().all(|key| {
                                matches!(
                                    key.as_str(),
                                    "type" | "source" | "include" | "exclude" | "condition"
                                )
                            })
                        })
                    {
                        match component_filter(modifier) {
                            Ok(filter) => ModifierKind::CopyBlockEntityComponents(filter),
                            Err(error) => ModifierKind::Unsupported(format!(
                                "unsupported copy_components: {error:#}"
                            )),
                        }
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

fn component_filter(value: &Value) -> Result<ComponentFilter> {
    let parse = |key: &str| -> Result<Option<BTreeSet<ResourceId>>> {
        value
            .get(key)
            .map(|list| -> Result<_> {
                let list = list.as_array().context("component filter must be a list")?;
                list.iter()
                    .map(|value| {
                        let id = ResourceId::parse(
                            value.as_str().context("component ID is not a string")?,
                        )?;
                        ensure!(
                            id.namespace == "minecraft"
                                && matches!(
                                    id.path.as_str(),
                                    "custom_name"
                                        | "item_name"
                                        | "tooltip_display"
                                        | "banner_patterns"
                                        | "rarity"
                                        | "custom_data"
                                        | "max_stack_size"
                                        | "damage"
                                        | "lore"
                                ),
                            "component ID is not yet verified"
                        );
                        Ok(id)
                    })
                    .collect()
            })
            .transpose()
    };
    Ok(ComponentFilter {
        include: parse("include")?,
        exclude: parse("exclude")?.unwrap_or_default(),
    })
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
        "minecraft:random_chance" => match value.get("chance").and_then(Value::as_f64) {
            Some(chance) if (chance as f32).is_finite() => Condition::RandomChance(chance as f32),
            _ => Condition::Unsupported("unsupported random chance provider".into()),
        },
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
    fn random_chance_uses_one_float_even_at_extreme_probabilities() {
        for (chance, draw, expected) in [
            (-1.0, 0.0, false),
            (0.0, 0.0, false),
            (0.125, 0.124, true),
            (0.125, 0.125, false),
            (1.0, 0.999, true),
            (2.0, 0.999, true),
        ] {
            let mut source = Memory::default();
            source.put("data/minecraft/loot_table/blocks/short_grass.json", &format!(r#"{{"type":"minecraft:block","pools":[{{"rolls":1,"entries":[{{"type":"minecraft:item","name":"minecraft:wheat_seeds","condition":{{"type":"minecraft:random_chance","chance":{chance}}}}}]}}]}}"#));
            let tables = BlockLootTables::load(&mut source).unwrap();
            let mut calls = 0;
            let mut random = || {
                calls += 1;
                draw
            };
            let drops = tables
                .drops_without_entity("minecraft:short_grass", Some(&mut random))
                .unwrap();
            assert_eq!(!drops.is_empty(), expected, "chance={chance} draw={draw}");
            assert_eq!(calls, 1);
            assert!(
                tables
                    .drops_without_entity("minecraft:short_grass", None)
                    .is_err()
            );
        }
        let mut source = Memory::default();
        source.put("data/minecraft/loot_table/blocks/short_grass.json", r#"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:wheat_seeds","condition":{"type":"minecraft:random_chance","chance":{"type":"example:unsupported"}}}]}]}"#);
        let tables = BlockLootTables::load(&mut source).unwrap();
        assert!(
            tables
                .drops_without_entity("minecraft:short_grass", Some(&mut || 0.0))
                .is_err()
        );
    }

    #[test]
    fn weighted_entries_choose_before_applying_selected_modifiers() {
        let mut source = Memory::default();
        source.put("data/minecraft/loot_table/blocks/stone.json", r#"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:stone","modifier":{"type":"minecraft:set_count","count":{"type":"minecraft:uniform","min":1,"max":3}}},{"type":"minecraft:item","name":"minecraft:stick","condition":{"type":"minecraft:random_chance","chance":1.0}},{"type":"minecraft:empty","weight":2}]}]}"#);
        let tables = BlockLootTables::load(&mut source).unwrap();
        let tool = BasicTool {
            item: None,
            count: 0,
            enchantments: &[],
        };
        assert!(tables.fully_supported_player_block_for_tool("minecraft:stone", tool));
        for seed in 0..128 {
            let mut actual = native_core::random::LegacyRandom::new(seed);
            let mut expected = actual.clone();
            expected.next_float();
            let selected = expected.next_int_bound(4).unwrap();
            let count = if selected == 0 {
                expected.next_int_bound(3).unwrap() + 1
            } else {
                1
            };
            let drops = tables
                .drops_with_random("minecraft:stone", tool, Some(&mut actual))
                .unwrap();
            if selected >= 2 {
                assert!(drops.is_empty());
            } else {
                assert_eq!(drops.len(), 1);
                assert_eq!(
                    drops[0].item,
                    ResourceId::parse(if selected == 0 {
                        "minecraft:stone"
                    } else {
                        "minecraft:stick"
                    })
                    .unwrap()
                );
                assert_eq!(drops[0].count, count as u32);
            }
            assert_eq!(actual.next_int(), expected.next_int(), "seed {seed}");
        }
        assert!(
            tables
                .drops_with_random("minecraft:stone", tool, None)
                .is_err()
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn weighted_block_loot_matches_java_26_3() {
        use std::{path::PathBuf, process::Command};
        const TABLES: [&str; 7] = [
            r#"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:stone","modifier":{"type":"minecraft:set_count","count":{"type":"minecraft:uniform","min":1,"max":3}}},{"type":"minecraft:item","name":"minecraft:stick","condition":{"type":"minecraft:random_chance","chance":0.5}},{"type":"minecraft:empty","weight":2}]}]}"#,
            r#"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:alternatives","children":[{"type":"minecraft:item","name":"minecraft:stone","weight":4,"condition":{"type":"minecraft:random_chance","chance":0.3}},{"type":"minecraft:item","name":"minecraft:stick","weight":2}]},{"type":"minecraft:item","name":"minecraft:diamond","weight":3},{"type":"minecraft:empty","weight":2}]}]}"#,
            r#"{"type":"minecraft:block","pools":[{"rolls":5,"entries":[{"type":"minecraft:item","name":"minecraft:stone","weight":0},{"type":"minecraft:item","name":"minecraft:stick"}]}]}"#,
            r#"{"type":"minecraft:block","pools":[{"rolls":3,"condition":{"type":"minecraft:random_chance","chance":0.5},"entries":[{"type":"minecraft:item","name":"minecraft:stone","weight":3,"modifier":{"type":"minecraft:set_count","count":2,"condition":{"type":"minecraft:random_chance","chance":0.5}}},{"type":"minecraft:item","name":"minecraft:stick","weight":2}]}]}"#,
            r#"{"type":"minecraft:block","pools":[{"rolls":3,"entries":[{"type":"minecraft:item","name":"minecraft:stone","weight":16777217,"quality":2147483647},{"type":"minecraft:item","name":"minecraft:stick","weight":16777219,"quality":-2147483648}]}]}"#,
            r#"{"type":"minecraft:block","pools":[{"rolls":3,"entries":[{"type":"minecraft:item","name":"minecraft:stone","weight":-17,"quality":13},{"type":"minecraft:item","name":"minecraft:stick","weight":2,"quality":-11}]}]}"#,
            r#"{"type":"minecraft:block","pools":[{"rolls":3,"entries":[{"type":"minecraft:item","name":"minecraft:stone","weight":2147483647,"quality":1},{"type":"minecraft:item","name":"minecraft:stick","weight":0,"quality":-1}]}]}"#,
        ];
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let metadata: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for entry in metadata["libraries"].as_array().unwrap() {
            if let Some(path) = entry["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("WeightedLootOracle.java");
        let input = temp.path().join("tables.json");
        let output = temp.path().join("output.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/WeightedLootOracle.java"),
        )
        .unwrap();
        let definitions: Vec<Value> = TABLES
            .iter()
            .map(|table| serde_json::from_str(table).unwrap())
            .collect();
        std::fs::write(&input, serde_json::to_vec(&definitions).unwrap()).unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("WeightedLootOracle")
                .arg(&input)
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), TABLES.len() * 2 * 64);
        let tool = BasicTool {
            item: None,
            count: 0,
            enchantments: &[],
        };
        let mut index = 0;
        for (table_index, table) in TABLES.iter().enumerate() {
            let mut source = Memory::default();
            source.put("data/minecraft/loot_table/blocks/stone.json", table);
            let tables = BlockLootTables::load(&mut source).unwrap();
            assert!(tables.fully_supported_player_block_for_tool("minecraft:stone", tool));
            for mode in 0..2 {
                for seed in -32..32 {
                    let (drops, next) = if mode == 0 {
                        let mut random = native_core::random::LegacyRandom::new(seed);
                        let drops = tables
                            .drops_with_random("minecraft:stone", tool, Some(&mut random))
                            .unwrap();
                        (drops, random.next_long())
                    } else {
                        let mut random = native_core::random::XoroshiroRandom::new(seed);
                        let drops = tables
                            .drops_with_random("minecraft:stone", tool, Some(&mut random))
                            .unwrap();
                        (drops, random.next_long())
                    };
                    let actual: Vec<_> = drops
                        .iter()
                        .map(|drop| {
                            serde_json::json!([
                                format!("{}:{}", drop.item.namespace, drop.item.path),
                                drop.count
                            ])
                        })
                        .collect();
                    assert_eq!(
                        actual,
                        *rows[index]["drops"].as_array().unwrap(),
                        "table={table_index} mode={mode} seed={seed}"
                    );
                    assert_eq!(
                        next,
                        rows[index]["next"].as_i64().unwrap(),
                        "table={table_index} mode={mode} seed={seed}"
                    );
                    index += 1;
                }
            }
        }
    }

    #[test]
    fn block_loot_quality_uses_zero_luck_and_java_float_weights() {
        let mut source = Memory::default();
        source.put("data/minecraft/loot_table/blocks/stone.json", r#"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:stone","weight":16777217,"quality":2147483647},{"type":"minecraft:item","name":"minecraft:stick","weight":16777219,"quality":-2147483648},{"type":"minecraft:item","name":"minecraft:dirt","weight":-1,"quality":1}]}]}"#);
        let tables = BlockLootTables::load(&mut source).unwrap();
        let table = &tables.tables[&ResourceId::parse("minecraft:blocks/stone").unwrap()];
        assert_eq!(
            table.pools[0]
                .entries
                .iter()
                .map(|entry| entry.weight)
                .collect::<Vec<_>>(),
            vec![16_777_216, 16_777_220, 0]
        );
        let tool = BasicTool {
            item: None,
            count: 0,
            enchantments: &[],
        };
        assert!(tables.fully_supported_player_block_for_tool("minecraft:stone", tool));
        for quality in [r#"1.5"#, r#"2147483648"#, r#""bad""#] {
            source.put("data/minecraft/loot_table/blocks/stone.json", &format!(r#"{{"type":"minecraft:block","pools":[{{"rolls":1,"entries":[{{"type":"minecraft:item","name":"minecraft:stone","quality":{quality}}}]}}]}}"#));
            let tables = BlockLootTables::load(&mut source).unwrap();
            assert!(!tables.fully_supported_player_block_for_tool("minecraft:stone", tool));
        }
    }

    #[test]
    fn weighted_preflight_rejects_overflow_and_checks_alternative_weights() {
        let mut source = Memory::default();
        source.put("data/minecraft/loot_table/blocks/stone.json", r#"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:stone","weight":2147483647},{"type":"minecraft:item","name":"minecraft:stick","weight":1}]}]}"#);
        let tables = BlockLootTables::load(&mut source).unwrap();
        let tool = BasicTool {
            item: None,
            count: 0,
            enchantments: &[],
        };
        assert!(!tables.fully_supported_player_block_for_tool("minecraft:stone", tool));
        let mut random = native_core::random::LegacyRandom::new(1);
        assert!(
            tables
                .drops_with_random("minecraft:stone", tool, Some(&mut random))
                .is_err()
        );
        source.put("data/minecraft/loot_table/blocks/stone.json", r#"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:alternatives","weight":0,"children":[{"type":"minecraft:item","name":"minecraft:stone","weight":2}]},{"type":"minecraft:item","name":"minecraft:stick"}]}]}"#);
        let tables = BlockLootTables::load(&mut source).unwrap();
        assert!(tables.fully_supported_player_block_for_tool("minecraft:stone", tool));
        assert_eq!(
            tables
                .potential_block_entity_drops("minecraft:stone")
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn interaction_preflight_checks_unselected_branches_and_original_block() {
        let mut source = Memory::default();
        source.put("data/example/loot_table/valid.json", r#"{"type":"minecraft:block_interact","random_sequence":"example:stream","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:stick","condition":{"type":"minecraft:match_block","blocks":"minecraft:rooted_dirt"}}]}]}"#);
        source.put("data/example/loot_table/invalid.json", r#"{"type":"minecraft:block_interact","pools":[{"rolls":1,"entries":[{"type":"minecraft:alternatives","children":[{"type":"minecraft:item","name":"minecraft:stick","condition":{"type":"minecraft:random_chance","chance":1.0}},{"type":"minecraft:item","name":"minecraft:stick","modifier":{"type":"minecraft:apply_bonus","enchantment":"minecraft:fortune","formula":"minecraft:uniform_bonus_count","parameters":{"bonusMultiplier":-2}}}]}]}]}"#);
        let valid = ResourceId::parse("example:valid").unwrap();
        let invalid = ResourceId::parse("example:invalid").unwrap();
        let mut tables = BlockLootTables::load(&mut source).unwrap();
        tables
            .load_interactions(&mut source, [&valid, &invalid])
            .unwrap();
        let enchantments = [(ResourceId::parse("minecraft:fortune").unwrap(), 1)];
        let tool = BasicTool {
            item: Some("minecraft:wooden_hoe"),
            count: 1,
            enchantments: &enchantments,
        };
        assert_eq!(
            tables.interaction_random_sequence(&valid).unwrap(),
            Some(&ResourceId::parse("example:stream").unwrap())
        );
        assert!(tables.fully_supported_interaction_for_tool(&valid, tool));
        assert!(!tables.fully_supported_interaction_for_tool(&invalid, tool));
        let properties = BTreeMap::new();
        for (name, count) in [("minecraft:rooted_dirt", 1), ("minecraft:dirt", 0)] {
            let block = LootBlockState {
                name,
                properties: &properties,
            };
            assert_eq!(
                tables
                    .interaction_drops_with_random(&valid, tool, Some(&block), None)
                    .unwrap()
                    .len(),
                count
            );
        }
    }

    #[test]
    fn player_loot_preflight_checks_bounds_in_unselected_branches() {
        let mut source = Memory::default();
        source.put("data/minecraft/loot_table/blocks/stone.json", r#"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:alternatives","children":[{"type":"minecraft:item","condition":{"type":"minecraft:random_chance","chance":1.0},"name":"minecraft:stone"},{"type":"minecraft:item","name":"minecraft:stone","modifier":{"type":"minecraft:apply_bonus","enchantment":"minecraft:fortune","formula":"minecraft:uniform_bonus_count","parameters":{"bonusMultiplier":-2}}}]}]}]}"#);
        let tables = BlockLootTables::load(&mut source).unwrap();
        let enchantments = [(ResourceId::parse("minecraft:fortune").unwrap(), 1)];
        let tool = BasicTool {
            item: Some("minecraft:iron_pickaxe"),
            count: 1,
            enchantments: &enchantments,
        };
        assert!(tables.fully_supported_player_block("minecraft:stone"));
        assert!(!tables.fully_supported_player_block_for_tool("minecraft:stone", tool));
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn random_chance_matches_java_26_3() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let metadata: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for entry in metadata["libraries"].as_array().unwrap() {
            if let Some(path) = entry["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("RandomChanceOracle.java");
        let output = temp.path().join("random_chance.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/RandomChanceOracle.java"),
        )
        .unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("RandomChanceOracle")
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), 768);
        let mut row = 0;
        for chance in [-0.5_f32, 0.0, 0.125, 0.5, 1.0, 1.5] {
            let mut source = Memory::default();
            source.put("data/minecraft/loot_table/blocks/short_grass.json", &format!(r#"{{"type":"minecraft:block","pools":[{{"rolls":1,"entries":[{{"type":"minecraft:item","name":"minecraft:wheat_seeds","condition":{{"type":"minecraft:random_chance","chance":{chance}}}}}]}}]}}"#));
            let table = BlockLootTables::load(&mut source).unwrap();
            for seed in 0..128 {
                let mut random = native_core::random::LegacyRandom::new(seed);
                let drops = table
                    .drops_without_entity("minecraft:short_grass", Some(&mut random))
                    .unwrap();
                assert_eq!(
                    !drops.is_empty(),
                    rows[row][0].as_bool().unwrap(),
                    "{chance} {seed}"
                );
                assert_eq!(
                    random.next_int(),
                    rows[row][1].as_i64().unwrap() as i32,
                    "{chance} {seed}"
                );
                row += 1;
            }
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
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn installed_plant_and_fire_loot_is_evaluated_without_a_tool() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let mut source = crate::packs::ResourceStack::open(&game).unwrap();
        let tables = BlockLootTables::load(&mut source).unwrap();
        for block in ["short_grass", "fern", "dead_bush", "fire", "soul_fire"] {
            for seed in 0..128 {
                let mut random = native_core::random::LegacyRandom::new(seed);
                let drops = tables
                    .drops_without_entity(&format!("minecraft:{block}"), Some(&mut random))
                    .unwrap();
                assert!(
                    drops
                        .iter()
                        .all(|drop| matches!(drop.item.path.as_str(), "wheat_seeds" | "stick")),
                    "{block} {seed} {drops:?}"
                );
            }
        }
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
    fn component_copy_filters_require_explicit_context_and_preserve_include_exclude() {
        let mut source = Memory::default();
        source.put("data/minecraft/loot_table/blocks/white_banner.json",r#"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:white_banner","modifier":[{"type":"minecraft:copy_components","source":"block_entity","include":["minecraft:banner_patterns","minecraft:custom_name"],"exclude":["minecraft:custom_name"]},{"type":"minecraft:copy_components","source":"block_entity","exclude":["minecraft:custom_data"]}]}]}]}"#);
        let tables = BlockLootTables::load(&mut source).unwrap();
        let tool = BasicTool {
            item: None,
            count: 0,
            enchantments: &[],
        };
        let properties = BTreeMap::new();
        assert!(!tables.fully_supported_player_block("minecraft:white_banner"));
        assert!(tables.fully_supported_player_block_with_components("minecraft:white_banner"));
        assert!(
            tables
                .drops_with_entity_at("minecraft:white_banner", &properties, tool, None)
                .is_err()
        );
        let state = LootBlockState {
            name: "minecraft:white_wall_banner",
            properties: &properties,
        };
        let drops = tables
            .drops_with_block_entity_at("minecraft:white_banner", &state, tool, true, None)
            .unwrap();
        let filters = &drops[0].copy_block_entity_components;
        assert_eq!(filters.len(), 2);
        let id = |name| ResourceId::parse(name).unwrap();
        assert!(filters[0].accepts(&id("minecraft:banner_patterns")));
        assert!(!filters[0].accepts(&id("minecraft:custom_name")));
        assert!(filters[1].accepts(&id("minecraft:banner_patterns")));
        assert!(filters[1].accepts(&id("minecraft:custom_name")));
        assert!(!filters[1].accepts(&id("minecraft:custom_data")));
    }

    #[test]
    fn possible_drops_include_unselected_alternatives_and_parent_modifiers() {
        let mut source = Memory::default();
        source.put("data/minecraft/loot_table/blocks/white_banner.json",r#"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:alternatives","children":[{"type":"minecraft:item","name":"minecraft:white_banner","condition":{"type":"minecraft:random_chance","chance":1.0}},{"type":"minecraft:item","name":"minecraft:stone"}],"modifier":{"type":"minecraft:copy_components","source":"block_entity","include":["minecraft:banner_patterns"]}}]}]}"#);
        let tables = BlockLootTables::load(&mut source).unwrap();
        let drops = tables
            .potential_block_entity_drops("minecraft:white_banner")
            .unwrap();
        assert_eq!(drops.len(), 2);
        assert_eq!(
            drops[0].item,
            ResourceId::parse("minecraft:white_banner").unwrap()
        );
        assert_eq!(drops[1].item, ResourceId::parse("minecraft:stone").unwrap());
        for drop in drops {
            assert_eq!(drop.copy_block_entity_components.len(), 1);
            assert!(
                drop.copy_block_entity_components[0]
                    .accepts(&ResourceId::parse("minecraft:banner_patterns").unwrap())
            );
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
            enchantments: &[],
        };
        assert_eq!(
            tables.drops("minecraft:dirt", pick(None)).unwrap(),
            vec![Drop {
                item: ResourceId::parse("minecraft:dirt").unwrap(),
                count: 2,
                copy_block_entity_custom_name: false,

                copy_block_entity_components: Vec::new(),
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

                copy_block_entity_components: Vec::new(),
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

                    copy_block_entity_components: Vec::new(),
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

                    copy_block_entity_components: Vec::new(),
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

                    copy_block_entity_components: Vec::new(),
                }],
                "{id}"
            );
        }
    }
}
