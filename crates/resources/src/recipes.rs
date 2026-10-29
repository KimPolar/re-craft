//! Data-driven 26.3 shaped and shapeless crafting recipes.
//!
//! Special recipe types, item prototypes and crafting remainders are not
//! resolved here. Unknown item IDs cannot yet be validated against the full
//! runtime item registry.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use crate::packs::{ResourceId, ResourceProvider};

#[derive(Clone, Debug)]
pub(crate) enum TagEntry {
    Item { id: String, required: bool },
    Tag { id: ResourceId, required: bool },
}

#[derive(Clone, Debug, Default)]
pub struct ItemTags {
    values: BTreeMap<ResourceId, BTreeSet<String>>,
}

impl ItemTags {
    pub fn contains(&self, tag: &ResourceId, item: &str) -> bool {
        self.values
            .get(tag)
            .is_some_and(|values| values.contains(item))
    }

    fn load(provider: &mut impl ResourceProvider, warnings: &mut Vec<String>) -> Result<Self> {
        let mut raw = BTreeMap::<ResourceId, Vec<TagEntry>>::new();
        for id in provider.list_data("tags/item")? {
            let path = format!("data/{}/tags/item/{}.json", id.namespace, id.path);
            let mut entries = Vec::new();
            for bytes in provider.read_data_stack(&path)? {
                match parse_tag(&bytes) {
                    Ok((replace, next)) => {
                        if replace {
                            entries.clear();
                        }
                        entries.extend(next);
                    }
                    Err(error) => warnings.push(format!("invalid item tag {path}: {error:#}")),
                }
            }
            raw.insert(id, entries);
        }
        let mut values = BTreeMap::new();
        for id in raw.keys() {
            if let Err(error) = resolve_tag(id, &raw, &mut values, &mut BTreeSet::new()) {
                warnings.push(format!(
                    "cannot resolve item tag {}:{}: {error:#}",
                    id.namespace, id.path
                ));
            }
        }
        Ok(Self { values })
    }
}

pub(crate) fn parse_tag(bytes: &[u8]) -> Result<(bool, Vec<TagEntry>)> {
    let json: Value = serde_json::from_slice(bytes)?;
    let replace = match json.get("replace") {
        None => false,
        Some(value) => value.as_bool().context("replace must be a boolean")?,
    };
    let entries = json
        .get("values")
        .and_then(Value::as_array)
        .context("values must be an array")?;
    let mut parsed = Vec::with_capacity(entries.len());
    for entry in entries {
        let (name, required) = if let Some(name) = entry.as_str() {
            (name, true)
        } else {
            let name = entry
                .get("id")
                .and_then(Value::as_str)
                .context("tag entry must have an id")?;
            let required = match entry.get("required") {
                None => true,
                Some(value) => value.as_bool().context("required must be a boolean")?,
            };
            (name, required)
        };
        if let Some(tag) = name.strip_prefix('#') {
            parsed.push(TagEntry::Tag {
                id: ResourceId::parse(tag)?,
                required,
            });
        } else {
            // Validation of item existence requires the full item registry.
            parsed.push(TagEntry::Item {
                id: canonical_id(&ResourceId::parse(name)?),
                required,
            });
        }
    }
    Ok((replace, parsed))
}

pub(crate) fn resolve_tag(
    id: &ResourceId,
    raw: &BTreeMap<ResourceId, Vec<TagEntry>>,
    cache: &mut BTreeMap<ResourceId, BTreeSet<String>>,
    visiting: &mut BTreeSet<ResourceId>,
) -> Result<BTreeSet<String>> {
    if let Some(values) = cache.get(id) {
        return Ok(values.clone());
    }
    ensure!(visiting.insert(id.clone()), "cyclic tag reference");
    let result = (|| {
        let entries = raw.get(id).context("referenced tag is missing")?;
        let mut values = BTreeSet::new();
        for entry in entries {
            match entry {
                TagEntry::Item { id, .. } => {
                    values.insert(id.clone());
                }
                TagEntry::Tag {
                    id: child,
                    required,
                } => match resolve_tag(child, raw, cache, visiting) {
                    Ok(child) => values.extend(child),
                    Err(error) if *required => {
                        return Err(error).context("required tag could not be resolved");
                    }
                    Err(_) => {}
                },
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

#[derive(Clone, Debug)]
enum IngredientEntry {
    Item(String),
    Tag(ResourceId),
}

#[derive(Clone, Debug)]
pub struct Ingredient(Vec<IngredientEntry>);

impl Ingredient {
    pub fn matches(&self, item: &str, tags: &ItemTags) -> bool {
        self.0.iter().any(|entry| match entry {
            IngredientEntry::Item(id) => id == item,
            IngredientEntry::Tag(id) => tags.contains(id, item),
        })
    }

    pub fn display_items<'a>(&'a self, tags: &'a ItemTags) -> Vec<&'a str> {
        let mut result = BTreeSet::new();
        for entry in &self.0 {
            match entry {
                IngredientEntry::Item(id) => {
                    result.insert(id.as_str());
                }
                IngredientEntry::Tag(id) => {
                    if let Some(items) = tags.values.get(id) {
                        result.extend(items.iter().map(String::as_str));
                    }
                }
            }
        }
        result.into_iter().collect()
    }
}

fn parse_ingredient(value: &Value) -> Result<Ingredient> {
    let mut entries = Vec::new();
    let list = match value {
        Value::String(_) => vec![value],
        Value::Array(values) if !values.is_empty() => values.iter().collect(),
        _ => bail!("ingredient must be an item, tag, or non-empty item list"),
    };
    let multiple = list.len() > 1 || matches!(value, Value::Array(_));
    for value in list {
        let id = value
            .as_str()
            .context("ingredient entry must be a string")?;
        if let Some(tag) = id.strip_prefix('#') {
            ensure!(!multiple, "item lists cannot contain tag references");
            entries.push(IngredientEntry::Tag(ResourceId::parse(tag)?));
        } else {
            let id = ResourceId::parse(id)?;
            ensure!(
                id.namespace != "minecraft" || id.path != "air",
                "air is not an ingredient"
            );
            entries.push(IngredientEntry::Item(canonical_id(&id)));
        }
    }
    Ok(Ingredient(entries))
}

#[derive(Clone, Debug)]
enum RecipePattern {
    Shaped {
        width: usize,
        height: usize,
        cells: Vec<Option<Ingredient>>,
    },
    Shapeless(Vec<Ingredient>),
}

#[derive(Clone, Debug)]
pub struct RecipeResult {
    pub id: ResourceId,
    pub count: u32,
    pub components: Option<Value>,
}

/// Reuse the same counted inventory across recipe-book availability checks.
pub struct RecipeBookInventory<'a> {
    entries: Vec<(&'a str, u32)>,
}

impl<'a> RecipeBookInventory<'a> {
    pub fn new(stacks: &[(&'a str, u32)]) -> Self {
        let mut totals = BTreeMap::<&str, u32>::new();
        for &(id, count) in stacks {
            let total = totals.entry(id).or_default();
            *total = total.saturating_add(count);
        }
        Self {
            entries: totals.into_iter().collect(),
        }
    }
}

/// Crafting categories used by the 26.3 recipe-book tabs.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CraftingCategory {
    Building,
    Equipment,
    Redstone,
    Misc,
}

#[derive(Clone, Debug)]
pub struct CraftingRecipe {
    pub id: ResourceId,
    pub result: RecipeResult,
    pub category: CraftingCategory,
    pub group: String,
    pattern: RecipePattern,
    ingredients: usize,
}

impl CraftingRecipe {
    /// The input-slot layout used by the recipe book, including empty cells.
    /// Shaped one-wide or one-high patterns are centered in a 3×3 grid.
    pub fn book_layout(&self, width: usize, height: usize) -> Option<Vec<Option<&Ingredient>>> {
        if !(1..=3).contains(&width) || !(1..=3).contains(&height) {
            return None;
        }
        let mut layout = vec![None; width * height];
        match &self.pattern {
            RecipePattern::Shaped {
                width: recipe_width,
                height: recipe_height,
                cells,
            } => {
                if *recipe_width > width || *recipe_height > height {
                    return None;
                }
                let offset_x = if recipe_width * 2 < width {
                    (width - recipe_width) / 2
                } else {
                    0
                };
                let offset_y = if recipe_height * 2 < height {
                    (height - recipe_height) / 2
                } else {
                    0
                };
                for y in 0..*recipe_height {
                    for x in 0..*recipe_width {
                        layout[(y + offset_y) * width + x + offset_x] =
                            cells[y * recipe_width + x].as_ref();
                    }
                }
            }
            RecipePattern::Shapeless(ingredients) => {
                if ingredients.len() > layout.len() {
                    return None;
                }
                for (slot, ingredient) in layout.iter_mut().zip(ingredients) {
                    *slot = Some(ingredient);
                }
            }
        }
        Some(layout)
    }

    /// Assign one item type to each occupied recipe slot without using more
    /// than the available count. This handles overlapping tags and alternatives.
    pub fn book_ingredients(
        &self,
        width: usize,
        height: usize,
        tags: &ItemTags,
        available: &[(&str, u32)],
        amount: u32,
    ) -> Option<Vec<Option<String>>> {
        let available = RecipeBookInventory::new(available);
        self.book_ingredients_counted(width, height, tags, &available, amount)
    }

    pub fn book_ingredients_counted(
        &self,
        width: usize,
        height: usize,
        tags: &ItemTags,
        available: &RecipeBookInventory<'_>,
        amount: u32,
    ) -> Option<Vec<Option<String>>> {
        if amount == 0 {
            return None;
        }
        let layout = self.book_layout(width, height)?;
        let inputs: Vec<_> = layout.iter().flatten().copied().collect();
        let candidates: Vec<_> = available
            .entries
            .iter()
            .filter(|&&(id, count)| {
                count >= amount && inputs.iter().any(|ingredient| ingredient.matches(id, tags))
            })
            .flat_map(|&(id, count)| {
                std::iter::repeat_n(id, (count / amount).min(inputs.len() as u32) as usize)
            })
            .collect();
        if candidates.len() < inputs.len() {
            return None;
        }
        let mut owner = vec![None; candidates.len()];
        fn assign(
            input: usize,
            ingredients: &[&Ingredient],
            candidates: &[&str],
            tags: &ItemTags,
            owner: &mut [Option<usize>],
            seen: &mut [bool],
        ) -> bool {
            for (index, candidate) in candidates.iter().enumerate() {
                if seen[index] || !ingredients[input].matches(candidate, tags) {
                    continue;
                }
                seen[index] = true;
                if owner[index].is_none_or(|previous| {
                    assign(previous, ingredients, candidates, tags, owner, seen)
                }) {
                    owner[index] = Some(input);
                    return true;
                }
            }
            false
        }
        for input in 0..inputs.len() {
            if !assign(
                input,
                &inputs,
                &candidates,
                tags,
                &mut owner,
                &mut vec![false; candidates.len()],
            ) {
                return None;
            }
        }
        let mut result = vec![None; inputs.len()];
        for (slot, input) in owner.into_iter().enumerate() {
            if let Some(input) = input {
                result[input] = Some(candidates[slot].to_owned());
            }
        }
        let mut result = result.into_iter();
        Some(
            layout
                .into_iter()
                .map(|slot| slot.and_then(|_| result.next().flatten()))
                .collect(),
        )
    }

    pub fn matches(
        &self,
        width: usize,
        height: usize,
        slots: &[Option<&str>],
        tags: &ItemTags,
    ) -> bool {
        if width == 0
            || height == 0
            || width > 3
            || height > 3
            || slots.len() != width * height
            || slots.iter().filter(|slot| slot.is_some()).count() != self.ingredients
        {
            return false;
        }
        match &self.pattern {
            RecipePattern::Shaped {
                width: pattern_width,
                height: pattern_height,
                cells,
            } => {
                if *pattern_width > width || *pattern_height > height {
                    return false;
                }
                for dy in 0..=height - pattern_height {
                    for dx in 0..=width - pattern_width {
                        for mirror in [false, true] {
                            let mut matches = true;
                            for y in 0..height {
                                for x in 0..width {
                                    let expected = if x >= dx
                                        && x < dx + pattern_width
                                        && y >= dy
                                        && y < dy + pattern_height
                                    {
                                        let px = x - dx;
                                        let px = if mirror { pattern_width - 1 - px } else { px };
                                        cells[(y - dy) * pattern_width + px].as_ref()
                                    } else {
                                        None
                                    };
                                    let item = slots[y * width + x];
                                    matches &= match (expected, item) {
                                        (None, None) => true,
                                        (Some(ingredient), Some(item)) => {
                                            ingredient.matches(item, tags)
                                        }
                                        _ => false,
                                    };
                                    if !matches {
                                        break;
                                    }
                                }
                                if !matches {
                                    break;
                                }
                            }
                            if matches {
                                return true;
                            }
                        }
                    }
                }
                false
            }
            RecipePattern::Shapeless(ingredients) => {
                let items: Vec<_> = slots.iter().flatten().copied().collect();
                let mut assigned = vec![None; items.len()];
                fn assign(
                    index: usize,
                    ingredients: &[Ingredient],
                    items: &[&str],
                    assigned: &mut [Option<usize>],
                    visited: &mut [bool],
                    tags: &ItemTags,
                ) -> bool {
                    for i in 0..items.len() {
                        if !visited[i] && ingredients[index].matches(items[i], tags) {
                            visited[i] = true;
                            if assigned[i].is_none_or(|previous| {
                                assign(previous, ingredients, items, assigned, visited, tags)
                            }) {
                                assigned[i] = Some(index);
                                return true;
                            }
                        }
                    }
                    false
                }
                (0..ingredients.len()).all(|index| {
                    assign(
                        index,
                        ingredients,
                        &items,
                        &mut assigned,
                        &mut vec![false; items.len()],
                        tags,
                    )
                })
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct CraftingRecipes {
    recipes: Vec<CraftingRecipe>,
    by_count: [Vec<usize>; 10],
    pub tags: ItemTags,
    pub warnings: Vec<String>,
    pub unsupported_types: BTreeSet<String>,
    unlock_by_item: BTreeMap<String, Vec<ResourceId>>,
}

impl CraftingRecipes {
    pub fn load(provider: &mut impl ResourceProvider) -> Result<Self> {
        let mut warnings = Vec::new();
        let tags = ItemTags::load(provider, &mut warnings)?;
        let mut recipes = Vec::new();
        let mut unsupported_types = BTreeSet::new();
        for id in provider.list_data("recipe")? {
            let path = format!("data/{}/recipe/{}.json", id.namespace, id.path);
            let Some(bytes) = provider.read_data(&path)? else {
                continue;
            };
            let json: Value = match serde_json::from_slice(&bytes) {
                Ok(value) => value,
                Err(error) => {
                    warnings.push(format!("invalid recipe {path}: {error}"));
                    continue;
                }
            };
            let Some(kind) = json.get("type").and_then(Value::as_str) else {
                warnings.push(format!("recipe {path} has no type"));
                continue;
            };
            if !matches!(
                kind,
                "minecraft:crafting_shaped" | "minecraft:crafting_shapeless"
            ) {
                unsupported_types.insert(kind.to_owned());
                continue;
            }
            match parse_recipe(id, &json) {
                Ok(recipe) => recipes.push(recipe),
                Err(error) => warnings.push(format!("invalid recipe {path}: {error:#}")),
            }
        }
        let mut by_count: [Vec<usize>; 10] = std::array::from_fn(|_| Vec::new());
        for (index, recipe) in recipes.iter().enumerate() {
            by_count[recipe.ingredients].push(index);
        }
        let recipe_ids: BTreeSet<_> = recipes.iter().map(|recipe| recipe.id.clone()).collect();
        let unlock_by_item = load_simple_recipe_unlocks(provider, &tags, &recipe_ids)?;
        Ok(Self {
            recipes,
            by_count,
            tags,
            warnings,
            unsupported_types,
            unlock_by_item,
        })
    }

    pub fn len(&self) -> usize {
        self.recipes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.recipes.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &CraftingRecipe> {
        self.recipes.iter()
    }

    pub fn at(&self, index: usize) -> Option<&CraftingRecipe> {
        self.recipes.get(index)
    }

    pub fn unlocked_by_item(&self, item: &str) -> &[ResourceId] {
        self.unlock_by_item.get(item).map_or(&[], Vec::as_slice)
    }

    pub fn get(&self, id: &ResourceId) -> Option<&CraftingRecipe> {
        self.recipes.iter().find(|recipe| &recipe.id == id)
    }

    pub fn matching(
        &self,
        width: usize,
        height: usize,
        slots: &[Option<&str>],
    ) -> Option<&CraftingRecipe> {
        let count = slots.iter().filter(|slot| slot.is_some()).count();
        self.by_count
            .get(count)?
            .iter()
            .map(|&index| &self.recipes[index])
            .find(|recipe| recipe.matches(width, height, slots, &self.tags))
    }
}

// Supports the common one-item inventory_changed advancement form. Other
// predicates and multi-group advancement progress require the advancement
// engine and are deliberately left for that implementation.
fn load_simple_recipe_unlocks(
    provider: &mut impl ResourceProvider,
    tags: &ItemTags,
    recipe_ids: &BTreeSet<ResourceId>,
) -> Result<BTreeMap<String, Vec<ResourceId>>> {
    let mut index = BTreeMap::<String, BTreeSet<ResourceId>>::new();
    for id in provider.list_data("advancement")? {
        let path = format!("data/{}/advancement/{}.json", id.namespace, id.path);
        let Some(bytes) = provider.read_data(&path)? else {
            continue;
        };
        let Ok(json) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        let Some(rewards) = json
            .get("rewards")
            .and_then(|v| v.get("recipes"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        let rewards: Vec<_> = rewards
            .iter()
            .filter_map(Value::as_str)
            .filter_map(|value| ResourceId::parse(value).ok())
            .filter(|id| recipe_ids.contains(id))
            .collect();
        if rewards.is_empty() {
            continue;
        }
        let Some(criteria) = json.get("criteria").and_then(Value::as_object) else {
            continue;
        };
        let names: Vec<&str> = match json.get("requirements") {
            Some(Value::Array(groups)) if groups.len() == 1 => {
                let Some(group) = groups[0].as_array() else {
                    continue;
                };
                if group.iter().any(|name| !name.is_string()) {
                    continue;
                }
                group.iter().filter_map(Value::as_str).collect()
            }
            None if criteria.len() == 1 => criteria.keys().map(String::as_str).collect(),
            _ => continue,
        };
        if names.is_empty()
            || names.iter().any(|name| !criteria.contains_key(*name))
            || criteria.keys().any(|name| !names.contains(&name.as_str()))
        {
            continue;
        }
        for name in names {
            let Some(criterion) = criteria.get(name) else {
                continue;
            };
            if criterion.get("trigger").and_then(Value::as_str)
                != Some("minecraft:inventory_changed")
            {
                continue;
            }
            let Some(conditions) = criterion.get("conditions").and_then(Value::as_object) else {
                continue;
            };
            if conditions.len() != 1 {
                continue;
            }
            let Some(items) = conditions.get("items").and_then(Value::as_array) else {
                continue;
            };
            if items.len() != 1 {
                continue;
            }
            let Some(predicate) = items[0].as_object() else {
                continue;
            };
            if predicate.len() != 1 {
                continue;
            }
            let Some(value) = predicate.get("items") else {
                continue;
            };
            let Ok(ingredient) = parse_ingredient(value) else {
                continue;
            };
            for item in ingredient.display_items(tags) {
                index
                    .entry(item.to_owned())
                    .or_default()
                    .extend(rewards.iter().cloned());
            }
        }
    }
    Ok(index
        .into_iter()
        .map(|(item, ids)| (item, ids.into_iter().collect()))
        .collect())
}

fn canonical_id(id: &ResourceId) -> String {
    format!("{}:{}", id.namespace, id.path)
}

fn parse_recipe(id: ResourceId, json: &Value) -> Result<CraftingRecipe> {
    let kind = json
        .get("type")
        .and_then(Value::as_str)
        .context("missing recipe type")?;
    let category = match json
        .get("category")
        .and_then(Value::as_str)
        .unwrap_or("misc")
    {
        "building" => CraftingCategory::Building,
        "equipment" => CraftingCategory::Equipment,
        "redstone" => CraftingCategory::Redstone,
        "misc" => CraftingCategory::Misc,
        other => bail!("invalid crafting recipe category {other}"),
    };
    if json.get("category").is_some_and(|value| !value.is_string()) {
        bail!("recipe category must be a string");
    }
    let group = json
        .get("group")
        .map(|value| value.as_str().context("recipe group must be a string"))
        .transpose()?
        .unwrap_or("")
        .to_owned();
    let result = json
        .get("result")
        .and_then(Value::as_object)
        .context("missing recipe result")?;
    let result_id = ResourceId::parse(
        result
            .get("id")
            .and_then(Value::as_str)
            .context("missing result id")?,
    )?;
    let count = result
        .get("count")
        .map(|count| count.as_u64().context("invalid result count"))
        .transpose()?
        .unwrap_or(1);
    ensure!((1..=99).contains(&count), "result count outside 1–99");
    let components = result.get("components").cloned();
    if let Some(components) = &components {
        ensure!(
            components.is_object(),
            "result components must be an object"
        );
    }
    let result = RecipeResult {
        id: result_id,
        count: count as u32,
        components,
    };
    let (pattern, ingredients) = if kind == "minecraft:crafting_shapeless" {
        let list = json
            .get("ingredients")
            .and_then(Value::as_array)
            .context("missing ingredients")?;
        ensure!(
            (1..=9).contains(&list.len()),
            "shapeless recipes require 1–9 ingredients"
        );
        let ingredients: Vec<_> = list.iter().map(parse_ingredient).collect::<Result<_>>()?;
        let count = ingredients.len();
        (RecipePattern::Shapeless(ingredients), count)
    } else {
        let key = json
            .get("key")
            .and_then(Value::as_object)
            .context("missing shaped recipe key")?;
        let mut keys = BTreeMap::new();
        for (symbol, value) in key {
            let mut chars = symbol.chars();
            let character = chars.next().context("empty shaped key")?;
            ensure!(
                chars.next().is_none() && character != ' ' && character.len_utf16() == 1,
                "invalid shaped key"
            );
            keys.insert(character, parse_ingredient(value)?);
        }
        let rows = json
            .get("pattern")
            .and_then(Value::as_array)
            .context("missing shaped pattern")?;
        ensure!((1..=3).contains(&rows.len()), "pattern height outside 1–3");
        let rows: Vec<Vec<char>> = rows
            .iter()
            .map(|row| {
                let row = row.as_str().context("pattern row must be a string")?;
                ensure!(
                    row.chars().all(|c| c.len_utf16() == 1),
                    "pattern contains unsupported symbol"
                );
                Ok(row.chars().collect::<Vec<_>>())
            })
            .collect::<Result<_>>()?;
        let width = rows[0].len();
        ensure!(
            (1..=3).contains(&width) && rows.iter().all(|row| row.len() == width),
            "invalid shaped pattern width"
        );
        let positions: Vec<_> = rows
            .iter()
            .enumerate()
            .flat_map(|(y, row)| {
                row.iter()
                    .enumerate()
                    .filter_map(move |(x, &c)| (c != ' ').then_some((x, y, c)))
            })
            .collect();
        ensure!(!positions.is_empty(), "empty shaped pattern");
        let mut used = BTreeSet::new();
        for &(_, _, symbol) in &positions {
            ensure!(keys.contains_key(&symbol), "undefined shaped key {symbol}");
            used.insert(symbol);
        }
        ensure!(used.len() == keys.len(), "shaped recipe has unused keys");
        let x0 = positions.iter().map(|p| p.0).min().unwrap();
        let x1 = positions.iter().map(|p| p.0).max().unwrap();
        let y0 = positions.iter().map(|p| p.1).min().unwrap();
        let y1 = positions.iter().map(|p| p.1).max().unwrap();
        let (width, height) = (x1 - x0 + 1, y1 - y0 + 1);
        let mut cells = Vec::with_capacity(width * height);
        for row in rows.iter().take(y1 + 1).skip(y0) {
            for symbol in &row[x0..=x1] {
                cells.push(keys.get(symbol).cloned());
            }
        }
        (
            RecipePattern::Shaped {
                width,
                height,
                cells,
            },
            positions.len(),
        )
    };
    Ok(CraftingRecipe {
        id,
        result,
        category,
        group,
        pattern,
        ingredients,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipe_book_centers_narrow_shapes_and_assigns_overlapping_ingredients() {
        let single = parse_recipe(
            ResourceId::parse("test:single").unwrap(),
            &serde_json::json!({"type":"minecraft:crafting_shaped", "pattern":["A"],
                "key":{"A":"minecraft:stone"}, "result":{"id":"minecraft:dirt"}}),
        )
        .unwrap();
        let occupied = |layout: Vec<Option<&Ingredient>>| {
            layout
                .iter()
                .enumerate()
                .filter_map(|(index, cell)| cell.is_some().then_some(index))
                .collect::<Vec<_>>()
        };
        assert_eq!(occupied(single.book_layout(3, 3).unwrap()), [4]);
        assert_eq!(occupied(single.book_layout(2, 2).unwrap()), [0]);
        let recipe = parse_recipe(
            ResourceId::parse("test:overlap").unwrap(),
            &serde_json::json!({"type":"minecraft:crafting_shapeless",
                "category":"building", "group":"example",
                "ingredients":[["minecraft:stone","minecraft:dirt"],"minecraft:stone"],
                "result":{"id":"minecraft:dirt"}}),
        )
        .unwrap();
        assert_eq!(recipe.category, CraftingCategory::Building);
        assert_eq!(recipe.group, "example");
        let available = [
            ("minecraft:stone", 1),
            ("minecraft:stone", 1),
            ("minecraft:dirt", 2),
        ];
        let result = recipe
            .book_ingredients(2, 2, &ItemTags::default(), &available, 2)
            .unwrap();
        assert_eq!(result[0].as_deref(), Some("minecraft:dirt"));
        assert_eq!(result[1].as_deref(), Some("minecraft:stone"));
        assert!(
            recipe
                .book_ingredients(2, 2, &ItemTags::default(), &available, 3)
                .is_none()
        );
        assert!(recipe.book_layout(1, 1).is_none());
    }

    #[test]
    fn recipe_book_indexes_simple_inventory_advancements_and_pack_overrides() {
        let mut provider = MemoryProvider::default();
        provider.add("data/example/recipe/planks.json", r##"{"type":"minecraft:crafting_shapeless","ingredients":["minecraft:stone"],"result":{"id":"minecraft:dirt"}}"##);
        provider.add(
            "data/example/tags/item/logs.json",
            r##"{"values":["minecraft:stone","minecraft:dirt"]}"##,
        );
        provider.add("data/example/advancement/recipes/planks.json", r##"{
            "criteria":{"has_log":{"trigger":"minecraft:inventory_changed","conditions":{"items":[{"items":"#example:logs"}]}},"has_recipe":{"trigger":"minecraft:recipe_unlocked","conditions":{"recipes":"example:planks"}}},
            "requirements":[["has_log","has_recipe"]],"rewards":{"recipes":["example:planks"]}}"##);
        let first = CraftingRecipes::load(&mut provider).unwrap();
        let id = ResourceId::parse("example:planks").unwrap();
        assert_eq!(
            first.unlocked_by_item("minecraft:stone"),
            std::slice::from_ref(&id)
        );
        assert_eq!(
            first.unlocked_by_item("minecraft:dirt"),
            std::slice::from_ref(&id)
        );
        // The higher layer changes the item trigger from a tag to a single ID.
        provider.add("data/example/advancement/recipes/planks.json", r##"{
            "criteria":{"has_log":{"trigger":"minecraft:inventory_changed","conditions":{"items":[{"items":"minecraft:stone"}]}},"has_recipe":{"trigger":"minecraft:recipe_unlocked"}},
            "requirements":[["has_log","has_recipe"]],"rewards":{"recipes":["example:planks"]}}"##);
        let recipes = CraftingRecipes::load(&mut provider).unwrap();
        assert_eq!(recipes.unlocked_by_item("minecraft:stone"), [id]);
        assert!(recipes.unlocked_by_item("minecraft:dirt").is_empty());
    }

    #[derive(Default)]
    struct MemoryProvider(BTreeMap<String, Vec<Vec<u8>>>);

    impl MemoryProvider {
        fn add(&mut self, path: &str, json: &str) {
            self.0
                .entry(path.into())
                .or_default()
                .push(json.as_bytes().to_vec());
        }
    }

    impl ResourceProvider for MemoryProvider {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.0.get(path).and_then(|layers| layers.last()).cloned())
        }
        fn read_data_stack(&mut self, path: &str) -> Result<Vec<Vec<u8>>> {
            Ok(self.0.get(path).cloned().unwrap_or_default())
        }
        fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
            let mut result = BTreeSet::new();
            for path in self.0.keys() {
                if let Some(rest) = path.strip_prefix("data/")
                    && let Some((namespace, rest)) = rest.split_once('/')
                    && let Some(path) = rest
                        .strip_prefix(&format!("{folder}/"))
                        .and_then(|s| s.strip_suffix(".json"))
                {
                    result.insert(ResourceId::parse(&format!("{namespace}:{path}"))?);
                }
            }
            Ok(result.into_iter().collect())
        }
    }

    #[test]
    fn tag_layers_replace_and_nested_optional_references_are_resolved() {
        let mut provider = MemoryProvider::default();
        provider.add(
            "data/example/tags/item/woods.json",
            r#"{"values":["minecraft:stone"]}"#,
        );
        provider.add(
            "data/example/tags/item/group.json",
            r#"{"values":["minecraft:dirt"]}"#,
        );
        provider.add("data/example/tags/item/group.json", r##"{"replace":true,"values":["#example:woods","minecraft:grass_block",{"id":"#example:missing","required":false}]}"##);
        let mut warnings = Vec::new();
        let tags = ItemTags::load(&mut provider, &mut warnings).unwrap();
        let group = ResourceId::parse("example:group").unwrap();
        assert!(tags.contains(&group, "minecraft:stone"));
        assert!(tags.contains(&group, "minecraft:grass_block"));
        assert!(!tags.contains(&group, "minecraft:dirt"));
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn shaped_patterns_trim_margins_and_match_horizontal_mirrors() {
        let json = serde_json::json!({
            "type":"minecraft:crafting_shaped",
            "key":{"A":"minecraft:stone","B":"minecraft:dirt"},
            "pattern":["   "," AB"," A "],
            "result":{"id":"minecraft:stone_bricks","count":4}
        });
        let recipe = parse_recipe(ResourceId::parse("example:shaped").unwrap(), &json).unwrap();
        let tags = ItemTags::default();
        let slots = [
            None,
            Some("minecraft:dirt"),
            Some("minecraft:stone"),
            None,
            None,
            Some("minecraft:stone"),
            None,
            None,
            None,
        ];
        assert!(recipe.matches(3, 3, &slots, &tags));
        let wrong = [
            Some("minecraft:dirt"),
            Some("minecraft:stone"),
            None,
            None,
            Some("minecraft:stone"),
            Some("minecraft:grass_block"),
            None,
            None,
            None,
        ];
        assert!(!recipe.matches(3, 3, &wrong, &tags));
    }

    #[test]
    fn shapeless_matching_reassigns_overlapping_ingredients() {
        let json = serde_json::json!({"type":"minecraft:crafting_shapeless", "ingredients":[["minecraft:stone","minecraft:dirt"],"minecraft:stone"], "result":{"id":"minecraft:stone_button"}});
        let recipe = parse_recipe(ResourceId::parse("example:shapeless").unwrap(), &json).unwrap();
        let tags = ItemTags::default();
        assert!(recipe.matches(
            2,
            2,
            &[Some("minecraft:stone"), Some("minecraft:dirt"), None, None],
            &tags
        ));
        assert!(!recipe.matches(
            2,
            2,
            &[Some("minecraft:dirt"), Some("minecraft:dirt"), None, None],
            &tags
        ));
    }

    #[test]
    fn invalid_recipe_is_reported_and_higher_pack_recipe_wins() {
        let mut provider = MemoryProvider::default();
        provider.add("data/example/recipe/test.json", r#"{"type":"minecraft:crafting_shapeless","ingredients":["minecraft:dirt"],"result":{"id":"minecraft:stone"}}"#);
        provider.add("data/example/recipe/test.json", r#"{"type":"minecraft:crafting_shapeless","ingredients":["minecraft:stone"],"result":{"id":"minecraft:stone_button"}}"#);
        provider.add("data/example/recipe/bad.json", r#"{"type":"minecraft:crafting_shaped","key":{"X":"minecraft:stone"},"pattern":["Y"],"result":{"id":"minecraft:stone"}}"#);
        let recipes = CraftingRecipes::load(&mut provider).unwrap();
        assert_eq!(recipes.len(), 1);
        assert_eq!(recipes.warnings.len(), 1);
        assert_eq!(
            recipes
                .matching(2, 2, &[Some("minecraft:stone"), None, None, None])
                .unwrap()
                .result
                .id
                .path,
            "stone_button"
        );
        assert!(
            recipes
                .matching(2, 2, &[Some("minecraft:dirt"), None, None, None])
                .is_none()
        );
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn installed_vanilla_recipes_load_without_shaped_or_shapeless_parse_errors() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let mut provider = crate::packs::ResourceStack::open(&game).unwrap();
        let recipes = CraftingRecipes::load(&mut provider).unwrap();
        let parse_errors: Vec<_> = recipes
            .warnings
            .iter()
            .filter(|warning| warning.starts_with("invalid recipe "))
            .collect();
        assert!(parse_errors.is_empty(), "{parse_errors:?}");
        assert_eq!(recipes.len(), 1202);
        let four_stone = [Some("minecraft:stone"); 4];
        assert_eq!(
            recipes.matching(2, 2, &four_stone).unwrap().result.id.path,
            "stone_bricks"
        );
        assert_eq!(
            recipes
                .matching(2, 2, &[Some("minecraft:stone"), None, None, None])
                .unwrap()
                .result
                .id
                .path,
            "stone_button"
        );
    }
}
