//! Data-driven chest loot and Java 26.3 container-slot distribution.
//!
//! Only explicitly supported table forms are accepted; unsupported data-pack
//! rules fail before an inventory or random stream is changed.
use crate::packs::{ResourceId, ResourceProvider};
use anyhow::{Context, Result, bail, ensure};
use native_core::random::{LegacyRandom, XoroshiroRandom};
use serde_json::Value;
use std::collections::BTreeMap;

pub trait ChestLootRandom: Clone {
    fn next_int(&mut self, bound: i32) -> Option<i32>;
    fn next_bool(&mut self) -> bool;
    fn next_float(&mut self) -> f32;
}
impl ChestLootRandom for LegacyRandom {
    fn next_int(&mut self, bound: i32) -> Option<i32> {
        self.next_int_bound(bound)
    }
    fn next_bool(&mut self) -> bool {
        LegacyRandom::next_bool(self)
    }
    fn next_float(&mut self) -> f32 {
        LegacyRandom::next_float(self)
    }
}
impl ChestLootRandom for XoroshiroRandom {
    fn next_int(&mut self, bound: i32) -> Option<i32> {
        self.next_int_bound(bound)
    }
    fn next_bool(&mut self) -> bool {
        XoroshiroRandom::next_bool(self)
    }
    fn next_float(&mut self) -> f32 {
        XoroshiroRandom::next_float(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChestLootItem {
    pub id: ResourceId,
    pub count: u32,
    /// Potion-contents component applied by `minecraft:set_potion`.
    pub potion: Option<ResourceId>,
}
#[derive(Clone, Debug)]
enum IntProvider {
    Constant(i32),
    Uniform(i32, i32),
}
#[derive(Clone, Debug)]
enum FloatProvider {
    Constant(f32),
    Uniform(Box<FloatProvider>, Box<FloatProvider>),
}
#[derive(Clone, Debug)]
enum Modifier {
    SetCount(IntProvider),
    SetPotion(ResourceId),
}
#[derive(Clone, Debug)]
struct Entry {
    item: Option<ResourceId>,
    weight: i32,
    quality: i32,
    modifiers: Vec<Modifier>,
}
#[derive(Clone, Debug)]
struct Pool {
    rolls: IntProvider,
    bonus_rolls: FloatProvider,
    entries: Vec<Entry>,
    modifiers: Vec<Modifier>,
}
#[derive(Clone, Debug)]
pub struct ChestLootTable {
    pools: Vec<Pool>,
    modifiers: Vec<Modifier>,
    random_sequence: Option<ResourceId>,
}

/// Supported chest tables from the active data-pack stack. Unsupported or
/// malformed tables are omitted, so they can never be treated as empty loot.
#[derive(Clone, Debug, Default)]
pub struct ChestLootTables {
    tables: BTreeMap<ResourceId, ChestLootTable>,
}
impl ChestLootTables {
    pub fn load(provider: &mut impl ResourceProvider) -> Result<Self> {
        let mut tables = BTreeMap::new();
        for id in provider.list_data("loot_table")? {
            let path = format!("data/{}/loot_table/{}.json", id.namespace, id.path);
            let Some(bytes) = provider.read_data(&path)? else {
                continue;
            };
            if bytes.len() > 16 * 1024 * 1024 {
                continue;
            }
            let Ok(value) = serde_json::from_slice(&bytes) else {
                continue;
            };
            if let Ok(table) = ChestLootTable::decode(&value) {
                tables.insert(id, table);
            }
        }
        Ok(Self { tables })
    }
    pub fn get(&self, id: &ResourceId) -> Option<&ChestLootTable> {
        self.tables.get(id)
    }
}

fn allowed_fields(value: &Value, allowed: &[&str]) -> Result<()> {
    let object = value.as_object().context("loot value must be an object")?;
    for key in object.keys() {
        ensure!(
            allowed.contains(&key.as_str()),
            "unsupported loot field {key}"
        );
    }
    Ok(())
}
fn int_value(value: &Value) -> Result<i32> {
    let number = value.as_i64().context("loot integer must be integral")?;
    i32::try_from(number).context("loot integer out of range")
}
fn provider(value: &Value) -> Result<IntProvider> {
    if value.is_number() {
        return Ok(IntProvider::Constant(int_value(value)?));
    }
    allowed_fields(value, &["type", "min", "max", "value"])?;
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .context("missing number provider type")?;
    let kind = ResourceId::parse(kind)?;
    ensure!(
        kind.namespace == "minecraft",
        "unsupported number provider namespace"
    );
    match kind.path.as_str() {
        "constant" => {
            ensure!(
                value.get("min").is_none() && value.get("max").is_none(),
                "invalid constant provider"
            );
            Ok(IntProvider::Constant(int_value(
                value.get("value").context("missing constant value")?,
            )?))
        }
        "uniform" => {
            ensure!(value.get("value").is_none(), "invalid uniform provider");
            let min = int_value(value.get("min").context("missing uniform minimum")?)?;
            let max = int_value(value.get("max").context("missing uniform maximum")?)?;
            ensure!(
                min <= max && i64::from(max) - i64::from(min) < i64::from(i32::MAX),
                "invalid uniform range"
            );
            Ok(IntProvider::Uniform(min, max))
        }
        _ => bail!("unsupported chest loot number provider {}", kind.path),
    }
}
fn float_provider(value: &Value, depth: usize) -> Result<FloatProvider> {
    ensure!(depth <= 32, "float provider nesting is too deep");
    let constant = |value: &Value| -> Result<FloatProvider> {
        let n = value.as_f64().context("invalid float provider constant")? as f32;
        ensure!(n.is_finite(), "invalid float provider constant");
        Ok(FloatProvider::Constant(n))
    };
    if value.is_number() {
        return constant(value);
    }
    allowed_fields(value, &["type", "min", "max", "value"])?;
    let kind = ResourceId::parse(
        value
            .get("type")
            .and_then(Value::as_str)
            .context("missing float provider type")?,
    )?;
    ensure!(
        kind.namespace == "minecraft",
        "unsupported float provider namespace"
    );
    match kind.path.as_str() {
        "constant" => {
            ensure!(
                value.get("min").is_none() && value.get("max").is_none(),
                "invalid constant float provider"
            );
            constant(value.get("value").context("missing float constant")?)
        }
        "uniform" => {
            ensure!(
                value.get("value").is_none(),
                "invalid uniform float provider"
            );
            Ok(FloatProvider::Uniform(
                Box::new(float_provider(
                    value.get("min").context("missing float minimum")?,
                    depth + 1,
                )?),
                Box::new(float_provider(
                    value.get("max").context("missing float maximum")?,
                    depth + 1,
                )?),
            ))
        }
        _ => bail!("unsupported chest loot float provider {}", kind.path),
    }
}
impl FloatProvider {
    fn sample(&self, random: &mut impl ChestLootRandom) -> f32 {
        match self {
            Self::Constant(value) => *value,
            Self::Uniform(min, max) => {
                let min = min.sample(random);
                let max = max.sample(random);
                if min >= max {
                    min
                } else {
                    random.next_float() * (max - min) + min
                }
            }
        }
    }
    fn may_be_nonzero(&self) -> bool {
        match self {
            Self::Constant(value) => *value != 0.0,
            Self::Uniform(min, max) => min.may_be_nonzero() || max.may_be_nonzero(),
        }
    }
}
fn modifiers(value: Option<&Value>) -> Result<Vec<Modifier>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let values: Vec<&Value> = if let Some(list) = value.as_array() {
        list.iter().collect()
    } else {
        vec![value]
    };
    ensure!(values.len() <= 1024, "too many loot modifiers");
    values
        .into_iter()
        .map(|entry| {
            let kind = ResourceId::parse(
                entry
                    .get("type")
                    .and_then(Value::as_str)
                    .context("missing loot modifier type")?,
            )?;
            ensure!(
                kind.namespace == "minecraft",
                "unsupported loot modifier namespace"
            );
            match kind.path.as_str() {
                "set_count" => {
                    allowed_fields(entry, &["type", "count", "add"])?;
                    ensure!(
                        entry.get("add").is_none_or(|v| v == &Value::Bool(false)),
                        "additive loot counts are unsupported"
                    );
                    Ok(Modifier::SetCount(provider(
                        entry.get("count").context("missing loot count")?,
                    )?))
                }
                "set_potion" => {
                    allowed_fields(entry, &["type", "id"])?;
                    Ok(Modifier::SetPotion(ResourceId::parse(
                        entry
                            .get("id")
                            .and_then(Value::as_str)
                            .context("missing potion id")?,
                    )?))
                }
                _ => bail!("unsupported chest loot modifier {}", kind.path),
            }
        })
        .collect()
}
impl IntProvider {
    fn sample(&self, random: &mut impl ChestLootRandom) -> Result<i32> {
        match *self {
            Self::Constant(value) => Ok(value),
            Self::Uniform(min, max) if min >= max => Ok(min),
            Self::Uniform(min, max) => Ok(min + bounded(random, max - min + 1)?),
        }
    }
    fn validate_nonnegative(&self, limit: i32) -> Result<()> {
        let (min, max) = match *self {
            Self::Constant(v) => (v, v),
            Self::Uniform(a, b) => (a, b),
        };
        ensure!(
            min >= 0 && max <= limit,
            "loot count or rolls out of supported range"
        );
        Ok(())
    }
}
impl ChestLootTable {
    pub fn load(provider: &mut impl ResourceProvider, id: &ResourceId) -> Result<Self> {
        let path = format!("data/{}/loot_table/{}.json", id.namespace, id.path);
        let bytes = provider
            .read_data(&path)?
            .with_context(|| format!("missing loot table {path}"))?;
        ensure!(bytes.len() <= 16 * 1024 * 1024, "loot table too large");
        Self::decode(&serde_json::from_slice(&bytes).context("invalid loot table JSON")?)
            .with_context(|| format!("in loot table {path}"))
    }
    pub fn decode(value: &Value) -> Result<Self> {
        allowed_fields(value, &["type", "random_sequence", "pools", "modifier"])?;
        let kind = ResourceId::parse(
            value
                .get("type")
                .and_then(Value::as_str)
                .context("missing loot table type")?,
        )?;
        ensure!(
            kind.namespace == "minecraft" && kind.path == "chest",
            "loot table is not a chest table"
        );
        let random_sequence = value
            .get("random_sequence")
            .map(|sequence| {
                ResourceId::parse(sequence.as_str().context("invalid random sequence")?)
            })
            .transpose()?;
        let list = value
            .get("pools")
            .and_then(Value::as_array)
            .context("missing chest loot pools")?;
        ensure!(list.len() <= 1024, "too many loot pools");
        let mut pools = Vec::with_capacity(list.len());
        for pool in list {
            allowed_fields(pool, &["rolls", "bonus_rolls", "entries", "modifier"])?;
            let bonus_rolls = match pool.get("bonus_rolls") {
                None => FloatProvider::Constant(0.0),
                Some(value) => float_provider(value, 0)?,
            };
            let rolls = provider(pool.get("rolls").context("missing pool rolls")?)?;
            rolls.validate_nonnegative(4096)?;
            let raw = pool
                .get("entries")
                .and_then(Value::as_array)
                .context("missing pool entries")?;
            ensure!(raw.len() <= 4096, "too many loot entries");
            let mut entries = Vec::with_capacity(raw.len());
            for entry in raw {
                allowed_fields(entry, &["type", "name", "weight", "quality", "modifier"])?;
                let kind = ResourceId::parse(
                    entry
                        .get("type")
                        .and_then(Value::as_str)
                        .context("missing entry type")?,
                )?;
                ensure!(
                    kind.namespace == "minecraft",
                    "unsupported loot entry namespace"
                );
                let item = match kind.path.as_str() {
                    "item" => Some(ResourceId::parse(
                        entry
                            .get("name")
                            .and_then(Value::as_str)
                            .context("missing item name")?,
                    )?),
                    "empty" => {
                        ensure!(entry.get("name").is_none(), "empty loot entry has a name");
                        None
                    }
                    _ => bail!("unsupported chest loot entry {}", kind.path),
                };
                let quality = entry
                    .get("quality")
                    .map(int_value)
                    .transpose()?
                    .unwrap_or(0);
                let weight = entry.get("weight").map(int_value).transpose()?.unwrap_or(1);
                let modifiers = modifiers(entry.get("modifier"))?;
                if item.is_none() {
                    ensure!(
                        modifiers.is_empty(),
                        "modifiers on empty entries are unsupported"
                    );
                }
                for modifier in &modifiers {
                    if let Modifier::SetCount(p) = modifier {
                        p.validate_nonnegative(1_000_000)?;
                    }
                }
                entries.push(Entry {
                    item,
                    weight,
                    quality,
                    modifiers,
                });
            }
            let modifiers = modifiers(pool.get("modifier"))?;
            for modifier in &modifiers {
                if let Modifier::SetCount(p) = modifier {
                    p.validate_nonnegative(1_000_000)?;
                }
            }
            pools.push(Pool {
                rolls,
                bonus_rolls,
                entries,
                modifiers,
            });
        }
        let modifiers = modifiers(value.get("modifier"))?;
        for modifier in &modifiers {
            if let Modifier::SetCount(p) = modifier {
                p.validate_nonnegative(1_000_000)?;
            }
        }
        Ok(Self {
            pools,
            modifiers,
            random_sequence,
        })
    }
    /// The table's named world random sequence, if specified by the active pack.
    pub fn random_sequence(&self) -> Option<&ResourceId> {
        self.random_sequence.as_ref()
    }
    /// True when the player's luck changes the table's selection or roll count.
    pub fn requires_luck(&self) -> bool {
        self.pools.iter().any(|pool| {
            pool.bonus_rolls.may_be_nonzero() || pool.entries.iter().any(|entry| entry.quality != 0)
        })
    }
    /// Fill empty slots with luck zero. A zero saved seed uses the world's named
    /// random sequence: pass that sequence explicitly to `fill_with_random`.
    pub fn fill_seeded(
        &self,
        occupied: &[bool],
        seed: i64,
        max_stack: impl FnMut(&ChestLootItem) -> Result<u32>,
    ) -> Result<Vec<Option<ChestLootItem>>> {
        ensure!(
            seed != 0,
            "zero loot seed requires the world's random sequence"
        );
        self.fill_with_random(occupied, &mut LegacyRandom::new(seed), max_stack)
    }
    /// Fill a fixed-seed chest with the verified luck of its opener.
    pub fn fill_seeded_with_luck(
        &self,
        occupied: &[bool],
        seed: i64,
        luck: f32,
        max_stack: impl FnMut(&ChestLootItem) -> Result<u32>,
    ) -> Result<Vec<Option<ChestLootItem>>> {
        ensure!(
            seed != 0,
            "zero loot seed requires the world's random sequence"
        );
        self.fill_with_random_and_luck(occupied, &mut LegacyRandom::new(seed), luck, max_stack)
    }
    /// Returns new items by slot, leaving occupied slots as `None`. `max_stack`
    /// must validate item and potion IDs and enabled features against the active
    /// registries, and return the component-dependent limit. On error the
    /// random state is unchanged.
    pub fn fill_with_random<R: ChestLootRandom>(
        &self,
        occupied: &[bool],
        random: &mut R,
        max_stack: impl FnMut(&ChestLootItem) -> Result<u32>,
    ) -> Result<Vec<Option<ChestLootItem>>> {
        ensure!(
            !self.requires_luck(),
            "loot table requires verified player luck"
        );
        self.fill_with_random_and_luck(occupied, random, 0.0, max_stack)
    }

    pub fn fill_with_random_and_luck<R: ChestLootRandom>(
        &self,
        occupied: &[bool],
        random: &mut R,
        luck: f32,
        mut max_stack: impl FnMut(&ChestLootItem) -> Result<u32>,
    ) -> Result<Vec<Option<ChestLootItem>>> {
        ensure!(occupied.len() <= 4096, "container is too large");
        ensure!(
            luck.is_finite() && (-1024.0..=1024.0).contains(&luck),
            "invalid player luck"
        );
        fn java_floor(value: f32) -> i32 {
            let truncated = value as i32;
            if value < truncated as f32 {
                truncated.wrapping_sub(1)
            } else {
                truncated
            }
        }
        let mut prepared = Vec::with_capacity(self.pools.len());
        for pool in &self.pools {
            let mut entries = Vec::with_capacity(pool.entries.len());
            for entry in &pool.entries {
                let maximum = if let Some(id) = &entry.item {
                    let mut prototype = ChestLootItem {
                        id: id.clone(),
                        count: 1,
                        potion: None,
                    };
                    for modifier in entry
                        .modifiers
                        .iter()
                        .chain(pool.modifiers.iter())
                        .chain(self.modifiers.iter())
                    {
                        if let Modifier::SetPotion(potion) = modifier {
                            prototype.potion = Some(potion.clone());
                        }
                    }
                    let maximum = max_stack(&prototype)?;
                    ensure!((1..=99).contains(&maximum), "invalid item stack limit");
                    maximum
                } else {
                    0
                };
                let weight =
                    java_floor((entry.weight as f32) + (entry.quality as f32) * luck).max(0);
                if weight > 0 {
                    entries.push((entry, maximum, weight));
                }
            }
            prepared.push(entries);
        }
        let mut rng = random.clone();
        let mut items = Vec::new();
        for (pool, candidates) in self.pools.iter().zip(prepared) {
            let total = candidates
                .iter()
                .try_fold(0i32, |sum, (_, _, weight)| sum.checked_add(*weight))
                .context("loot weight overflow")?;
            let rolls = pool.rolls.sample(&mut rng)?;
            let bonus = pool.bonus_rolls.sample(&mut rng);
            let bonus = if bonus.is_finite() { bonus } else { 0.0 };
            let rolls = rolls
                .checked_add(java_floor(bonus * luck))
                .context("loot rolls overflow")?;
            ensure!(rolls <= 4096, "too many loot rolls");
            for _ in 0..rolls {
                if candidates.is_empty() {
                    continue;
                }
                let (selected, maximum, _) = if candidates.len() == 1 {
                    candidates[0]
                } else {
                    let mut n = bounded(&mut rng, total)?;
                    let mut selected = candidates[0];
                    for &(entry, maximum, weight) in &candidates {
                        n -= weight;
                        if n < 0 {
                            selected = (entry, maximum, weight);
                            break;
                        }
                    }
                    selected
                };
                let Some(id) = &selected.item else {
                    continue;
                };
                let mut item = ChestLootItem {
                    id: id.clone(),
                    count: 1,
                    potion: None,
                };
                for modifier in selected
                    .modifiers
                    .iter()
                    .chain(pool.modifiers.iter())
                    .chain(self.modifiers.iter())
                {
                    match modifier {
                        Modifier::SetCount(value) => {
                            item.count = u32::try_from(value.sample(&mut rng)?)?
                        }
                        Modifier::SetPotion(potion) => item.potion = Some(potion.clone()),
                    }
                }
                if item.count == 0 {
                    continue;
                }
                while item.count > maximum {
                    let mut split = item.clone();
                    split.count = maximum;
                    items.push(split);
                    item.count -= maximum;
                    ensure!(items.len() <= 1_000_000, "too many generated stacks");
                }
                items.push(item);
                ensure!(items.len() <= 1_000_000, "too many generated stacks");
            }
        }
        let mut free: Vec<usize> = occupied
            .iter()
            .enumerate()
            .filter_map(|(i, occupied)| (!occupied).then_some(i))
            .collect();
        shuffle(&mut free, &mut rng)?;
        let mut single = Vec::new();
        let mut split = Vec::new();
        for item in items {
            if item.count > 1 {
                split.push(item)
            } else {
                single.push(item)
            }
        }
        while free.len() > single.len() + split.len() && !split.is_empty() {
            let index = between(&mut rng, 0, split.len() as i32 - 1)? as usize;
            let mut item = split.remove(index);
            let amount = between(&mut rng, 1, (item.count / 2) as i32)? as u32;
            item.count -= amount;
            let mut other = item.clone();
            other.count = amount;
            for part in [item, other] {
                if part.count > 1 && rng.next_bool() {
                    split.push(part)
                } else {
                    single.push(part)
                }
            }
        }
        single.extend(split);
        shuffle(&mut single, &mut rng)?;
        let mut output = vec![None; occupied.len()];
        for item in single {
            let Some(slot) = free.pop() else { break };
            output[slot] = Some(item);
        }
        *random = rng;
        Ok(output)
    }
}
fn between(random: &mut impl ChestLootRandom, min: i32, max: i32) -> Result<i32> {
    if min >= max {
        Ok(min)
    } else {
        Ok(min + bounded(random, max - min + 1)?)
    }
}
fn shuffle<T>(values: &mut [T], random: &mut impl ChestLootRandom) -> Result<()> {
    for n in (2..=values.len()).rev() {
        let index = bounded(random, n as i32)? as usize;
        values.swap(n - 1, index);
    }
    Ok(())
}
fn bounded(random: &mut impl ChestLootRandom, bound: i32) -> Result<i32> {
    ensure!(bound > 0, "invalid random bound");
    let value = random
        .next_int(bound)
        .context("random source rejected bound")?;
    ensure!(
        (0..bound).contains(&value),
        "random source returned an out-of-range value"
    );
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn catalog_uses_active_data_and_omits_unsupported_tables() {
        struct Source;
        impl ResourceProvider for Source {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn list_data(&mut self, _: &str) -> Result<Vec<ResourceId>> {
                [
                    "example:custom",
                    "minecraft:chests/unsupported",
                    "minecraft:blocks/stone",
                ]
                .iter()
                .map(|id| ResourceId::parse(id))
                .collect()
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                let value = if path.contains("custom") {
                    json!({"type":"minecraft:chest", "pools":[]})
                } else if path.contains("unsupported") {
                    json!({"type":"minecraft:chest", "pools":[], "condition":{}})
                } else {
                    json!({"type":"minecraft:block", "pools":[]})
                };
                Ok(Some(serde_json::to_vec(&value)?))
            }
        }
        let tables = ChestLootTables::load(&mut Source).unwrap();
        assert!(
            tables
                .get(&ResourceId::parse("example:custom").unwrap())
                .is_some()
        );
        assert!(
            tables
                .get(&ResourceId::parse("minecraft:chests/unsupported").unwrap())
                .is_none()
        );
        assert!(
            tables
                .get(&ResourceId::parse("minecraft:blocks/stone").unwrap())
                .is_none()
        );
    }
    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn installed_catalog_contains_buried_treasure() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let mut source = crate::packs::ResourceStack::open(&game).unwrap();
        let catalog = ChestLootTables::load(&mut source).unwrap();
        assert!(
            catalog
                .get(&ResourceId::parse("minecraft:chests/buried_treasure").unwrap())
                .is_some()
        );
    }
    #[test]
    fn chest_weight_uses_java_float_rounding_and_clamps_negative_values() {
        let table = ChestLootTable::decode(
            &json!({"type":"minecraft:chest","pools":[{"rolls":1,"entries":[
                {"type":"minecraft:item","name":"minecraft:stone","weight":16777217},
                {"type":"minecraft:item","name":"minecraft:stick","weight":16777219},
                {"type":"minecraft:item","name":"minecraft:dirt","weight":-1}
            ]}]}),
        )
        .unwrap();
        assert_eq!(
            table.pools[0]
                .entries
                .iter()
                .map(|entry| entry.weight)
                .collect::<Vec<_>>(),
            vec![16_777_217, 16_777_219, -1]
        );
        let dependent = ChestLootTable::decode(&json!({"type":"minecraft:chest","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:stone","quality":1}]}]})).unwrap();
        let mut random = LegacyRandom::new(42);
        let before = random.clone().next_long();
        assert!(dependent.requires_luck());
        assert!(
            dependent
                .fill_with_random(&[false; 27], &mut random, |_| Ok(64))
                .is_err()
        );
        assert_eq!(random.next_long(), before);
        let mut random = LegacyRandom::new(42);
        let before = random.clone().next_long();
        assert!(
            dependent
                .fill_with_random_and_luck(&[false; 27], &mut random, f32::NAN, |_| Ok(64))
                .is_err()
        );
        assert_eq!(random.next_long(), before);
        let mut random = LegacyRandom::new(42);
        assert!(
            dependent
                .fill_with_random_and_luck(&[false; 27], &mut random, 1.0, |_| Ok(64))
                .unwrap()
                .iter()
                .flatten()
                .any(|item| item.id.path == "stone")
        );
    }

    #[test]
    fn retains_the_named_world_random_sequence() {
        let mut value = json!({"type":"minecraft:chest", "pools":[]});
        assert!(
            ChestLootTable::decode(&value)
                .unwrap()
                .random_sequence()
                .is_none()
        );
        value["random_sequence"] = json!("custom:chests/treasure");
        let table = ChestLootTable::decode(&value).unwrap();
        assert_eq!(
            table.random_sequence(),
            Some(&ResourceId::parse("custom:chests/treasure").unwrap())
        );
        value["random_sequence"] = json!("invalid:id:again");
        assert!(ChestLootTable::decode(&value).is_err());
    }
    #[test]
    fn fills_only_available_slots_and_preserves_random_on_error() {
        let table=ChestLootTable::decode(&json!({"type":"minecraft:chest","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:iron_ingot","modifier":{"type":"minecraft:set_count","count":7}}]}]})).unwrap();
        let mut random = LegacyRandom::new(42);
        let slots = table
            .fill_with_random(
                &[
                    true, false, false, false, false, false, false, false, false, false,
                ],
                &mut random,
                |_| Ok(64),
            )
            .unwrap();
        assert!(slots[0].is_none());
        assert_eq!(slots.iter().flatten().map(|i| i.count).sum::<u32>(), 7);
        let mut random = LegacyRandom::new(42);
        let expected = random.clone().next_long();
        assert!(
            table
                .fill_with_random(&[false; 27], &mut random, |_| anyhow::bail!("unknown item"))
                .is_err()
        );
        assert_eq!(random.next_long(), expected);
        assert!(table.fill_seeded(&[false; 27], 0, |_| Ok(64)).is_err());
        let mut invalid = json!({"type":"minecraft:chest","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:stone","condition":{"type":"minecraft:random_chance","chance":0.5}}]}]});
        assert!(ChestLootTable::decode(&invalid).is_err());
        invalid["pools"][0]["entries"][0]
            .as_object_mut()
            .unwrap()
            .remove("condition");
        invalid["pools"][0]["bonus_rolls"] = json!({"type":"example:unknown","min":0,"max":1});
        assert!(ChestLootTable::decode(&invalid).is_err());
    }
    #[derive(Clone)]
    struct BadRandom {
        calls: usize,
    }
    #[test]
    fn nested_bonus_provider_samples_in_order_and_handles_zero_luck() {
        let nested = float_provider(&json!({"type":"minecraft:uniform","min":{"type":"minecraft:uniform","min":-1.0,"max":1.0},"max":{"type":"minecraft:uniform","min":1.0,"max":3.0}}), 0).unwrap();
        let mut random = LegacyRandom::new(42);
        let mut expected = random.clone();
        let min = expected.next_float() * 2.0 - 1.0;
        let max = expected.next_float() * 2.0 + 1.0;
        let value = expected.next_float() * (max - min) + min;
        assert_eq!(nested.sample(&mut random).to_bits(), value.to_bits());
        assert_eq!(random.state(), expected.state());
        let table = ChestLootTable::decode(&json!({"type":"minecraft:chest","pools":[{"rolls":0,"bonus_rolls":{"type":"minecraft:uniform","min":0.0,"max":2.0},"entries":[]}]})).unwrap();
        let mut actual = LegacyRandom::new(7);
        let mut expected = actual.clone();
        expected.next_float();
        // Empty containers avoid slot shuffling, leaving only the bonus sample.
        table
            .fill_with_random_and_luck(&[], &mut actual, 0.0, |_| Ok(64))
            .unwrap();
        assert_eq!(actual.state(), expected.state());
    }
    impl ChestLootRandom for BadRandom {
        fn next_int(&mut self, bound: i32) -> Option<i32> {
            self.calls += 1;
            Some(bound)
        }
        fn next_bool(&mut self) -> bool {
            false
        }
        fn next_float(&mut self) -> f32 {
            self.calls += 1;
            0.5
        }
    }
    #[test]
    fn rejects_invalid_random_source_without_committing_it() {
        let table = ChestLootTable::decode(&json!({"type":"minecraft:chest","pools":[]})).unwrap();
        let mut random = BadRandom { calls: 0 };
        assert!(
            table
                .fill_with_random(&[false; 2], &mut random, |_| Ok(64))
                .is_err()
        );
        assert_eq!(random.calls, 0);
    }
}
