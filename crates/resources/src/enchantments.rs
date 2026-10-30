//! Attribute effects from the active 26.3 enchantment data-pack registry.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use crate::packs::{ResourceId, ResourceProvider};
use crate::recipes::{TagEntry, parse_tag, resolve_tag};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttributeOperation {
    AddValue,
    AddMultipliedBase,
    AddMultipliedTotal,
}

impl AttributeOperation {
    pub fn parse(name: &str) -> Result<Self> {
        Ok(match name.strip_prefix("minecraft:").unwrap_or(name) {
            "add_value" => Self::AddValue,
            "add_multiplied_base" => Self::AddMultipliedBase,
            "add_multiplied_total" => Self::AddMultipliedTotal,
            _ => bail!("unsupported attribute operation {name}"),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EquipmentSlot {
    Mainhand,
    Offhand,
    Feet,
    Legs,
    Chest,
    Head,
}

impl EquipmentSlot {
    pub fn valid_group(group: &str) -> bool {
        matches!(
            group,
            "any"
                | "hand"
                | "armor"
                | "mainhand"
                | "offhand"
                | "feet"
                | "legs"
                | "chest"
                | "head"
                | "body"
                | "saddle"
        )
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Mainhand => "mainhand",
            Self::Offhand => "offhand",
            Self::Feet => "feet",
            Self::Legs => "legs",
            Self::Chest => "chest",
            Self::Head => "head",
        }
    }

    pub fn matches(self, group: &str) -> bool {
        match group {
            "any" => true,
            "hand" => matches!(self, Self::Mainhand | Self::Offhand),
            "armor" => matches!(self, Self::Feet | Self::Legs | Self::Chest | Self::Head),
            group => group == self.name(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AttributeModifier {
    pub id: ResourceId,
    pub operation: AttributeOperation,
    pub amount: f64,
}

#[derive(Clone, Debug, PartialEq)]
enum LevelValue {
    Constant(f32),
    Linear(f32, f32),
    Squared(f32),
    Fraction(Box<Self>, Box<Self>),
    Exponent(Box<Self>, Box<Self>),
    Clamped(Box<Self>, f32, f32),
    Lookup(Vec<f32>, Box<Self>),
}

impl LevelValue {
    fn parse(value: &Value, depth: usize) -> Result<Self> {
        ensure!(depth <= 32, "enchantment level value is too deeply nested");
        let number = |value: &Value| -> Result<f32> {
            let number = value.as_f64().context("level value is not a number")? as f32;
            ensure!(number.is_finite(), "level value is not finite");
            Ok(number)
        };
        if value.is_number() {
            return Ok(Self::Constant(number(value)?));
        }
        let object = value.as_object().context("level value is not an object")?;
        let field = |name: &str| object.get(name).with_context(|| format!("missing {name}"));
        let nested = |name: &str| Self::parse(field(name)?, depth + 1).map(Box::new);
        let kind = ResourceId::parse(
            field("type")?
                .as_str()
                .context("invalid level value type")?,
        )?;
        ensure!(
            kind.namespace == "minecraft",
            "unknown level value namespace"
        );
        Ok(match kind.path.as_str() {
            "linear" => Self::Linear(
                number(field("base")?)?,
                number(field("per_level_above_first")?)?,
            ),
            "levels_squared" => Self::Squared(number(field("added")?)?),
            "fraction" => Self::Fraction(nested("numerator")?, nested("denominator")?),
            "exponent" => Self::Exponent(nested("base")?, nested("power")?),
            "clamped" => {
                let min = number(field("min")?)?;
                let max = number(field("max")?)?;
                ensure!(min <= max, "invalid level value bounds");
                Self::Clamped(nested("value")?, min, max)
            }
            "lookup" => Self::Lookup(
                field("values")?
                    .as_array()
                    .context("lookup values are not a list")?
                    .iter()
                    .map(number)
                    .collect::<Result<_>>()?,
                nested("fallback")?,
            ),
            _ => bail!("unsupported level value type {}", kind.path),
        })
    }

    fn calculate(&self, level: u32) -> f32 {
        match self {
            Self::Constant(value) => *value,
            Self::Linear(base, step) => *base + *step * (level as f32 - 1.0),
            Self::Squared(added) => (level * level) as f32 + *added,
            Self::Fraction(numerator, denominator) => {
                let divisor = denominator.calculate(level);
                if divisor == 0.0 {
                    0.0
                } else {
                    numerator.calculate(level) / divisor
                }
            }
            Self::Exponent(base, power) => {
                f64::from(base.calculate(level)).powf(f64::from(power.calculate(level))) as f32
            }
            Self::Clamped(value, min, max) => value.calculate(level).clamp(*min, *max),
            Self::Lookup(values, fallback) => values
                .get(level as usize - 1)
                .copied()
                .unwrap_or_else(|| fallback.calculate(level)),
        }
    }
}

/// Deterministic value effects supported for repair-with-XP. Conditional and
/// random effects remain errors until their loot context can be evaluated.
#[derive(Clone, Debug, PartialEq)]
enum RepairValueEffect {
    Add(LevelValue),
    Multiply(LevelValue),
    Set(LevelValue),
    ScaleExponentially(LevelValue, LevelValue),
    All(Vec<Self>),
}

impl RepairValueEffect {
    fn parse(value: &Value, depth: usize) -> Result<Self> {
        ensure!(depth <= 32, "repair effect nesting is too deep");
        let object = value
            .as_object()
            .context("repair effect is not an object")?;
        let kind = ResourceId::parse(
            object
                .get("type")
                .and_then(Value::as_str)
                .context("repair effect type is missing")?,
        )?;
        ensure!(
            kind.namespace == "minecraft",
            "unknown repair effect namespace"
        );
        let level = |name: &str| {
            LevelValue::parse(
                object
                    .get(name)
                    .with_context(|| format!("missing {name}"))?,
                0,
            )
        };
        Ok(match kind.path.as_str() {
            "add" => Self::Add(level("value")?),
            "multiply" => Self::Multiply(level("factor")?),
            "set" => Self::Set(level("value")?),
            "exponential" => Self::ScaleExponentially(level("base")?, level("exponent")?),
            "all_of" => {
                let entries = object
                    .get("effects")
                    .and_then(Value::as_array)
                    .context("repair effects are not a list")?;
                ensure!(entries.len() <= 256, "too many nested repair effects");
                Self::All(
                    entries
                        .iter()
                        .map(|entry| Self::parse(entry, depth + 1))
                        .collect::<Result<_>>()?,
                )
            }
            _ => bail!("unsupported repair effect {}", kind.path),
        })
    }

    fn apply(&self, level: u32, value: f32) -> f32 {
        match self {
            Self::Add(amount) => value + amount.calculate(level),
            Self::Multiply(factor) => value * factor.calculate(level),
            Self::Set(amount) => amount.calculate(level),
            Self::ScaleExponentially(base, exponent) => {
                (f64::from(value)
                    * f64::from(base.calculate(level)).powf(f64::from(exponent.calculate(level))))
                    as f32
            }
            Self::All(effects) => effects
                .iter()
                .fold(value, |value, effect| effect.apply(level, value)),
        }
    }
}

fn repair_effect_entry(effects: &serde_json::Map<String, Value>) -> Result<Option<&Value>> {
    let mut result = None;
    for key in [
        "minecraft:repair_with_xp",
        "repair_with_xp",
        ":repair_with_xp",
    ] {
        if let Some(value) = effects.get(key) {
            ensure!(result.is_none(), "duplicate repair-with-XP effect");
            result = Some(value);
        }
    }
    Ok(result)
}

fn repair_effects(value: &Value) -> Result<Option<Vec<RepairValueEffect>>> {
    let Some(effects) = value.get("effects") else {
        return Ok(None);
    };
    let effects = effects
        .as_object()
        .context("enchantment effects are not an object")?;
    let Some(repair) = repair_effect_entry(effects)? else {
        return Ok(None);
    };
    let entries = repair
        .as_array()
        .context("repair-with-XP effects are not a list")?;
    ensure!(entries.len() <= 256, "too many repair-with-XP effects");
    let mut parsed = Vec::with_capacity(entries.len());
    for entry in entries {
        let entry = entry
            .as_object()
            .context("conditional repair effect is not an object")?;
        ensure!(
            entry.get("requirements").is_none_or(Value::is_null),
            "conditional repair effects are not supported yet"
        );
        parsed.push(RepairValueEffect::parse(
            entry.get("effect").context("repair effect is missing")?,
            0,
        )?);
    }
    Ok(Some(parsed))
}

#[derive(Clone, Debug, PartialEq)]
struct EnchantmentEffect {
    attribute: ResourceId,
    id: ResourceId,
    operation: AttributeOperation,
    amount: LevelValue,
}

#[derive(Clone, Debug, PartialEq)]
struct Definition {
    slots: Vec<String>,
    effects: Vec<EnchantmentEffect>,
    prevent_armor_change: bool,
    prevent_equipment_drop: bool,
    repair_with_xp: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum DamagePredicate {
    Any,
    Tags(Vec<(ResourceId, bool)>),
    All(Vec<Self>),
    Either(Vec<Self>),
    Not(Box<Self>),
}

impl DamagePredicate {
    fn parse(value: &Value, depth: usize) -> Result<Self> {
        ensure!(depth < 32, "damage predicate is too deeply nested");
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .context("damage predicate has no type")?;
        match kind.strip_prefix("minecraft:").unwrap_or(kind) {
            "damage_source_properties" => {
                let predicate = value
                    .get("predicate")
                    .and_then(Value::as_object)
                    .context("damage predicate is missing")?;
                ensure!(
                    predicate
                        .keys()
                        .all(|key| key == "tags" || key == "is_direct"),
                    "unsupported damage source predicate"
                );
                if let Some(direct) = predicate.get("is_direct") {
                    ensure!(
                        direct.as_bool() == Some(true),
                        "unsupported indirect damage source predicate"
                    );
                }
                let mut tags = Vec::new();
                if let Some(entries) = predicate.get("tags") {
                    for entry in entries.as_array().context("damage tags are not a list")? {
                        let id = entry
                            .get("id")
                            .and_then(Value::as_str)
                            .context("damage tag id is missing")?;
                        let id = id
                            .strip_prefix('#')
                            .context("damage tag reference must begin with #")?;
                        let expected = entry
                            .get("expected")
                            .and_then(Value::as_bool)
                            .context("damage tag expectation is missing")?;
                        tags.push((ResourceId::parse(id)?, expected));
                    }
                }
                Ok(Self::Tags(tags))
            }
            "all_of" | "any_of" => {
                let children = value
                    .get("terms")
                    .or_else(|| value.get("predicates"))
                    .and_then(Value::as_array)
                    .context("damage predicate has no terms")?
                    .iter()
                    .map(|item| Self::parse(item, depth + 1))
                    .collect::<Result<Vec<_>>>()?;
                Ok(if kind.ends_with("all_of") {
                    Self::All(children)
                } else {
                    Self::Either(children)
                })
            }
            "inverted" => Ok(Self::Not(Box::new(Self::parse(
                value
                    .get("term")
                    .context("inverted predicate has no term")?,
                depth + 1,
            )?))),
            _ => bail!("unsupported damage predicate {kind}"),
        }
    }

    fn test(
        &self,
        tags: &BTreeMap<ResourceId, std::result::Result<BTreeSet<String>, String>>,
        damage: &str,
    ) -> Result<bool> {
        match self {
            Self::Any => Ok(true),
            Self::Tags(checks) => {
                for (id, expected) in checks {
                    if tag_contains(tags, id, damage)? != *expected {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Self::All(children) => {
                for child in children {
                    if !child.test(tags, damage)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Self::Either(children) => {
                for child in children {
                    if child.test(tags, damage)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Self::Not(child) => Ok(!child.test(tags, damage)?),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum ValueOperation {
    Add,
    Multiply,
    Set,
}

#[derive(Clone, Debug, PartialEq)]
struct ProtectionEffect {
    operation: ValueOperation,
    amount: LevelValue,
    requirement: DamagePredicate,
}

fn protection_effects(value: &Value) -> Result<Vec<ProtectionEffect>> {
    let Some(entries) = value
        .get("effects")
        .and_then(|v| v.get("minecraft:damage_protection"))
    else {
        return Ok(Vec::new());
    };
    entries
        .as_array()
        .context("damage protection is not a list")?
        .iter()
        .map(|entry| {
            let effect = entry
                .get("effect")
                .context("damage protection has no effect")?;
            let kind = effect
                .get("type")
                .and_then(Value::as_str)
                .context("damage protection has no type")?;
            let (operation, field) = match kind.strip_prefix("minecraft:").unwrap_or(kind) {
                "add" => (ValueOperation::Add, "value"),
                "multiply" => (ValueOperation::Multiply, "factor"),
                "set" => (ValueOperation::Set, "value"),
                _ => bail!("unsupported damage protection effect {kind}"),
            };
            Ok(ProtectionEffect {
                operation,
                amount: LevelValue::parse(
                    effect
                        .get(field)
                        .context("damage protection amount is missing")?,
                    0,
                )?,
                requirement: entry
                    .get("requirements")
                    .map(|v| DamagePredicate::parse(v, 0))
                    .transpose()?
                    .unwrap_or(DamagePredicate::Any),
            })
        })
        .collect()
}

fn tag_contains(
    tags: &BTreeMap<ResourceId, std::result::Result<BTreeSet<String>, String>>,
    id: &ResourceId,
    damage: &str,
) -> Result<bool> {
    let values = tags
        .get(id)
        .context("damage type tag is missing")?
        .as_ref()
        .map_err(|error| anyhow::anyhow!("invalid damage type tag: {error}"))?;
    Ok(values.contains(damage))
}

/// Follow just the tags needed by a behavior, including nested and replaced
/// entries, without loading the entire entity-type tag registry.
fn load_single_tag(
    provider: &mut impl ResourceProvider,
    folder: &str,
    root: &ResourceId,
) -> Result<BTreeSet<String>> {
    let mut raw = BTreeMap::new();
    let mut visited = BTreeSet::new();
    let mut pending = vec![(root.clone(), 0)];
    while let Some((id, depth)) = pending.pop() {
        if !visited.insert(id.clone()) {
            continue;
        }
        ensure!(depth < 128, "registry tag nesting is too deep");
        ensure!(visited.len() <= 4096, "too many nested registry tags");
        let path = format!("data/{}/tags/{folder}/{}.json", id.namespace, id.path);
        let stack = provider.read_data_stack(&path)?;
        if stack.is_empty() {
            continue;
        }
        let mut entries = Vec::new();
        for bytes in stack {
            let (replace, values) = parse_tag(&bytes)?;
            if replace {
                entries.clear();
            }
            entries.extend(values);
        }
        for entry in &entries {
            if let TagEntry::Tag { id: child, .. } = entry {
                pending.push((child.clone(), depth + 1));
            }
        }
        raw.insert(id, entries);
    }
    resolve_tag(root, &raw, &mut BTreeMap::new(), &mut BTreeSet::new())
}

fn definition(value: &Value) -> Result<Definition> {
    let slots = value
        .get("slots")
        .and_then(Value::as_array)
        .context("enchantment has no slot list")?
        .iter()
        .map(|slot| {
            let slot = slot.as_str().context("enchantment slot is not a string")?;
            ensure!(
                EquipmentSlot::valid_group(slot),
                "unknown equipment slot group {slot}"
            );
            Ok(slot.to_owned())
        })
        .collect::<Result<Vec<_>>>()?;
    let effects_object = value.get("effects");
    ensure!(
        effects_object.is_none_or(Value::is_object),
        "enchantment effects are not an object"
    );
    let prevent_armor_change = effects_object
        .and_then(|effects| effects.get("minecraft:prevent_armor_change"))
        .is_some();
    let prevent_equipment_drop = effects_object
        .and_then(|effects| effects.get("minecraft:prevent_equipment_drop"))
        .is_some();
    let repair_with_xp = effects_object
        .and_then(Value::as_object)
        .map(repair_effect_entry)
        .transpose()?
        .flatten()
        .is_some();
    let Some(entries) = effects_object.and_then(|effects| effects.get("minecraft:attributes"))
    else {
        return Ok(Definition {
            slots,
            effects: Vec::new(),
            prevent_armor_change,
            prevent_equipment_drop,
            repair_with_xp,
        });
    };
    let entries = entries
        .as_array()
        .context("enchantment attributes are not a list")?;
    let mut effects = Vec::new();
    for entry in entries {
        let attribute = entry
            .get("attribute")
            .and_then(Value::as_str)
            .context("enchantment attribute is missing")?;
        let attribute = ResourceId::parse(attribute)?;
        if attribute.namespace != "minecraft"
            || !matches!(
                attribute.path.as_str(),
                "oxygen_bonus"
                    | "bounciness"
                    | "armor"
                    | "armor_toughness"
                    | "safe_fall_distance"
                    | "fall_damage_multiplier"
                    | "movement_speed"
                    | "gravity"
                    | "jump_strength"
                    | "water_movement_efficiency"
                    | "mining_efficiency"
                    | "block_break_speed"
                    | "submerged_mining_speed"
            )
        {
            continue;
        }
        let id = ResourceId::parse(
            entry
                .get("id")
                .and_then(Value::as_str)
                .context("attribute effect id is missing")?,
        )?;
        let operation = AttributeOperation::parse(
            entry
                .get("operation")
                .and_then(Value::as_str)
                .context("attribute operation is missing")?,
        )?;
        let amount = LevelValue::parse(
            entry.get("amount").context("attribute amount is missing")?,
            0,
        )?;
        effects.push(EnchantmentEffect {
            attribute,
            id,
            operation,
            amount,
        });
    }
    Ok(Definition {
        slots,
        effects,
        prevent_armor_change,
        prevent_equipment_drop,
        repair_with_xp,
    })
}

/// Enchantments are resolved through the selected world data packs. Unknown or
/// unsupported definitions are reported when used rather than guessed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OxygenEnchantments {
    definitions: BTreeMap<ResourceId, std::result::Result<Definition, String>>,
    protection: BTreeMap<ResourceId, std::result::Result<Vec<ProtectionEffect>, String>>,
    repair: BTreeMap<ResourceId, std::result::Result<Option<Vec<RepairValueEffect>>, String>>,
    damage_tags: BTreeMap<ResourceId, std::result::Result<BTreeSet<String>, String>>,
    damage_types: BTreeSet<ResourceId>,
    fall_immune_entities: Option<std::result::Result<BTreeSet<String>, String>>,
}

impl OxygenEnchantments {
    pub fn load(provider: &mut impl ResourceProvider) -> Result<Self> {
        let mut definitions = BTreeMap::new();
        let mut protection = BTreeMap::new();
        let mut repair = BTreeMap::new();
        for id in provider.list_data("enchantment")? {
            let path = format!("data/{}/enchantment/{}.json", id.namespace, id.path);
            let Some(bytes) = provider.read_data(&path)? else {
                continue;
            };
            let value = serde_json::from_slice::<Value>(&bytes).context("invalid enchantment JSON");
            let parsed = value
                .as_ref()
                .map_err(|error| anyhow::anyhow!("{error:#}"))
                .and_then(definition)
                .map_err(|error| format!("{error:#}"));
            let protection_effect = value
                .as_ref()
                .map_err(|error| anyhow::anyhow!("{error:#}"))
                .and_then(protection_effects)
                .map_err(|error| format!("{error:#}"));
            let repair_effect = value
                .as_ref()
                .map_err(|error| anyhow::anyhow!("{error:#}"))
                .and_then(repair_effects)
                .map_err(|error| format!("{error:#}"));
            protection.insert(id.clone(), protection_effect);
            repair.insert(id.clone(), repair_effect);
            definitions.insert(id, parsed);
        }
        let damage_types = provider.list_data("damage_type")?.into_iter().collect();
        let mut raw: BTreeMap<ResourceId, Vec<TagEntry>> = BTreeMap::new();
        let mut errors = BTreeMap::new();
        for id in provider.list_data("tags/damage_type")? {
            let path = format!("data/{}/tags/damage_type/{}.json", id.namespace, id.path);
            let mut entries = Vec::new();
            let result = (|| -> Result<()> {
                for bytes in provider.read_data_stack(&path)? {
                    let (replace, values) = parse_tag(&bytes)?;
                    if replace {
                        entries.clear();
                    }
                    entries.extend(values);
                }
                Ok(())
            })();
            match result {
                Ok(()) => {
                    raw.insert(id, entries);
                }
                Err(error) => {
                    errors.insert(id, format!("{error:#}"));
                }
            }
        }
        let mut cache = BTreeMap::new();
        let mut damage_tags = BTreeMap::new();
        for id in raw.keys() {
            let result = resolve_tag(id, &raw, &mut cache, &mut BTreeSet::new())
                .map_err(|error| format!("{error:#}"));
            damage_tags.insert(id.clone(), result);
        }
        for (id, error) in errors {
            damage_tags.insert(id, Err(error));
        }
        let immune_id = ResourceId::parse("minecraft:fall_damage_immune")?;
        let fall_immune_entities = Some(
            load_single_tag(provider, "entity_type", &immune_id)
                .map_err(|error| format!("{error:#}")),
        );
        Ok(Self {
            definitions,
            protection,
            repair,
            damage_tags,
            damage_types,
            fall_immune_entities,
        })
    }

    pub fn modifiers(
        &self,
        id: &ResourceId,
        level: u32,
        slot: EquipmentSlot,
    ) -> Result<Vec<AttributeModifier>> {
        self.modifiers_for(id, level, slot, "oxygen_bonus")
    }

    /// Apply the active definition's attribute effects for one equipment slot.
    pub fn modifiers_for(
        &self,
        id: &ResourceId,
        level: u32,
        slot: EquipmentSlot,
        attribute: &str,
    ) -> Result<Vec<AttributeModifier>> {
        ensure!((1..=255).contains(&level), "invalid enchantment level");
        let definition = self
            .definitions
            .get(id)
            .context("enchantment is absent from the active registry")?
            .as_ref()
            .map_err(|error| anyhow::anyhow!("unsupported enchantment: {error}"))?;
        if !definition.slots.iter().any(|group| slot.matches(group)) {
            return Ok(Vec::new());
        }
        definition
            .effects
            .iter()
            .filter(|effect| {
                effect.attribute.namespace == "minecraft" && effect.attribute.path == attribute
            })
            .map(|effect| {
                let amount = effect.amount.calculate(level);
                ensure!(
                    amount.is_finite(),
                    "enchantment attribute amount is not finite"
                );
                Ok(AttributeModifier {
                    id: ResourceId {
                        namespace: effect.id.namespace.clone(),
                        path: format!("{}/{}", effect.id.path, slot.name()),
                    },
                    operation: effect.operation,
                    amount: f64::from(amount),
                })
            })
            .collect()
    }

    /// `EnchantmentHelper.has` checks an effect on the item regardless of
    /// whether its declared active slots include the slot being queried.
    pub fn prevents_armor_change(&self, id: &ResourceId) -> Result<bool> {
        let definition = self
            .definitions
            .get(id)
            .context("enchantment is absent from the active registry")?
            .as_ref()
            .map_err(|error| anyhow::anyhow!("unsupported enchantment: {error}"))?;
        Ok(definition.prevent_armor_change)
    }

    /// The death-drop effect is checked on an item even when its enchantment
    /// does not declare the item's current equipment slot.
    pub fn prevents_equipment_drop(&self, id: &ResourceId) -> Result<bool> {
        let definition = self
            .definitions
            .get(id)
            .context("enchantment is absent from the active registry")?
            .as_ref()
            .map_err(|error| anyhow::anyhow!("unsupported enchantment: {error}"))?;
        Ok(definition.prevent_equipment_drop)
    }

    /// Whether an equipped enchantment can consume XP to repair this slot.
    pub fn repairs_with_xp(&self, id: &ResourceId, slot: EquipmentSlot) -> Result<bool> {
        let definition = self
            .definitions
            .get(id)
            .context("enchantment is absent from the active registry")?
            .as_ref()
            .map_err(|error| anyhow::anyhow!("unsupported enchantment: {error}"))?;
        Ok(definition.repair_with_xp && definition.slots.iter().any(|group| slot.matches(group)))
    }

    /// Apply this enchantment's item-filtered repair effects in their data-pack order.
    /// `None` means it has no effects, allowing the caller to identify multiple
    /// independently ordered enchantments without inspecting registry internals.
    pub fn apply_repair_with_xp(
        &self,
        id: &ResourceId,
        level: u32,
        value: f32,
    ) -> Result<Option<f32>> {
        ensure!((1..=255).contains(&level), "invalid enchantment level");
        let effects = self
            .repair
            .get(id)
            .context("enchantment is absent from the active registry")?
            .as_ref()
            .map_err(|error| anyhow::anyhow!("unsupported repair effect: {error}"))?;
        Ok(effects
            .as_ref()
            .filter(|effects| !effects.is_empty())
            .map(|effects| {
                effects
                    .iter()
                    .fold(value, |value, effect| effect.apply(level, value))
            }))
    }

    /// Resolve the world's current damage-type tags, including data-pack overrides.
    pub fn damage_type_has_tag(&self, tag: &str, damage: &str) -> Result<bool> {
        tag_contains(&self.damage_tags, &ResourceId::parse(tag)?, damage)
    }

    /// Check the IDs visible in the active world's data packs.
    pub fn has_damage_type(&self, damage: &str) -> Result<bool> {
        Ok(self.damage_types.contains(&ResourceId::parse(damage)?))
    }

    /// Resolve the entity immunity tag from the active data packs.
    pub fn fall_damage_immune(&self, entity: &str) -> Result<bool> {
        Ok(self
            .fall_immune_entities
            .as_ref()
            .context("entity fall immunity tag is unavailable")?
            .as_ref()
            .map_err(|error| anyhow::anyhow!("invalid entity fall immunity tag: {error}"))?
            .contains(entity))
    }

    /// Apply one equipped enchantment's conditional protection effects.
    pub fn protect_damage(
        &self,
        id: &ResourceId,
        level: u32,
        slot: EquipmentSlot,
        damage: &str,
        mut protection: f32,
    ) -> Result<f32> {
        ensure!((1..=255).contains(&level), "invalid enchantment level");
        let definition = self
            .definitions
            .get(id)
            .context("enchantment is absent from the active registry")?
            .as_ref()
            .map_err(|error| anyhow::anyhow!("unsupported enchantment: {error}"))?;
        if !definition.slots.iter().any(|group| slot.matches(group)) {
            return Ok(protection);
        }
        let effects = self
            .protection
            .get(id)
            .context("enchantment protection is absent")?
            .as_ref()
            .map_err(|error| anyhow::anyhow!("unsupported enchantment protection: {error}"))?;
        for effect in effects {
            if effect.requirement.test(&self.damage_tags, damage)? {
                let amount = effect.amount.calculate(level);
                ensure!(amount.is_finite(), "enchantment protection is not finite");
                protection = match effect.operation {
                    ValueOperation::Add => protection + amount,
                    ValueOperation::Multiply => protection * amount,
                    ValueOperation::Set => amount,
                };
                ensure!(
                    protection.is_finite(),
                    "enchantment protection is not finite"
                );
            }
        }
        Ok(protection)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_effects_do_not_look_like_absent_repair_effects() {
        assert!(definition(&serde_json::json!({"slots": ["hand"], "effects": []})).is_err());
    }

    #[test]
    fn repair_effect_component_accepts_default_namespace_and_rejects_duplicates() {
        let effect = serde_json::json!([{"effect":{"type":"multiply","factor":2}}]);
        for key in [
            "minecraft:repair_with_xp",
            "repair_with_xp",
            ":repair_with_xp",
        ] {
            let value = serde_json::json!({"slots":["hand"], "effects":{key:effect}});
            assert!(definition(&value).unwrap().repair_with_xp);
            let effects = repair_effects(&value).unwrap().unwrap();
            assert_eq!(effects[0].apply(1, 7.0), 14.0);
        }
        let duplicate = serde_json::json!({
            "slots":["hand"],
            "effects":{
                "minecraft:repair_with_xp":effect,
                "repair_with_xp":effect
            }
        });
        assert!(definition(&duplicate).is_err());
        assert!(repair_effects(&duplicate).is_err());
    }

    use serde_json::json;

    struct Data;
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
            let ids = match folder {
                "enchantment" => vec!["example:soft", "example:unsupported"],
                "tags/damage_type" => {
                    vec!["minecraft:is_fall", "minecraft:bypasses_invulnerability"]
                }
                _ => vec![],
            };
            ids.into_iter().map(ResourceId::parse).collect()
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            let value = match path {
                "data/example/enchantment/soft.json" => {
                    json!({"slots":["feet"],"effects":{"minecraft:damage_protection":[{"effect":{"type":"minecraft:add","value":{"type":"minecraft:linear","base":3,"per_level_above_first":3}},"requirements":{"type":"minecraft:damage_source_properties","predicate":{"tags":[{"id":"#minecraft:is_fall","expected":true},{"id":"#minecraft:bypasses_invulnerability","expected":false}]}}}]}})
                }
                "data/example/enchantment/unsupported.json" => {
                    json!({"slots":["head"],"effects":{"minecraft:damage_protection":[{"effect":{"type":"minecraft:add","value":1},"requirements":{"type":"example:unknown"}}]}})
                }
                _ => return Ok(None),
            };
            Ok(Some(serde_json::to_vec(&value)?))
        }
        fn read_data_stack(&mut self, path: &str) -> Result<Vec<Vec<u8>>> {
            let values = match path {
                // The higher pack replaces the original tag rather than appending.
                "data/minecraft/tags/damage_type/is_fall.json" => vec![
                    json!({"values":["minecraft:fall"]}),
                    json!({"replace":true,"values":["minecraft:stalagmite"]}),
                ],
                "data/minecraft/tags/damage_type/bypasses_invulnerability.json" => {
                    vec![json!({"values":[]})]
                }
                "data/minecraft/tags/entity_type/fall_damage_immune.json" => vec![
                    json!({"values":["minecraft:bat"]}),
                    json!({"replace":true,"values":["#example:immune"]}),
                ],
                "data/example/tags/entity_type/immune.json" => {
                    vec![json!({"values":["minecraft:player"]})]
                }
                _ => vec![],
            };
            values
                .iter()
                .map(|value| serde_json::to_vec(value).map_err(Into::into))
                .collect()
        }
    }

    #[test]
    fn level_values_and_slot_groups_follow_the_26_3_shapes() {
        let value = LevelValue::parse(&json!({"type":"minecraft:clamped","min":0.0,"max":5.0,"value":{"type":"minecraft:linear","base":1.0,"per_level_above_first":2.0}}), 0).unwrap();
        assert_eq!(
            (value.calculate(1), value.calculate(2), value.calculate(4)),
            (1.0, 3.0, 5.0)
        );
        assert!(EquipmentSlot::Head.matches("armor"));
        assert!(!EquipmentSlot::Mainhand.matches("armor"));
        assert!(EquipmentSlot::Offhand.matches("hand"));
    }

    #[test]
    fn protection_uses_active_pack_tags_slots_and_rejects_unknown_predicates() {
        let registry = OxygenEnchantments::load(&mut Data).unwrap();
        assert!(registry.fall_damage_immune("minecraft:player").unwrap());
        assert!(!registry.fall_damage_immune("minecraft:bat").unwrap());
        let id = ResourceId::parse("example:soft").unwrap();
        assert!(
            !registry
                .damage_type_has_tag("minecraft:is_fall", "minecraft:fall")
                .unwrap()
        );
        assert_eq!(
            registry
                .protect_damage(&id, 4, EquipmentSlot::Feet, "minecraft:fall", 0.0)
                .unwrap(),
            0.0
        );
        assert_eq!(
            registry
                .protect_damage(&id, 4, EquipmentSlot::Feet, "minecraft:stalagmite", 1.0)
                .unwrap(),
            13.0
        );
        assert_eq!(
            registry
                .protect_damage(&id, 4, EquipmentSlot::Head, "minecraft:stalagmite", 1.0)
                .unwrap(),
            1.0
        );
        let unknown = ResourceId::parse("example:unsupported").unwrap();
        assert!(
            registry
                .protect_damage(&unknown, 1, EquipmentSlot::Head, "minecraft:fall", 0.0)
                .is_err()
        );
        assert!(
            registry
                .modifiers_for(&unknown, 1, EquipmentSlot::Head, "oxygen_bonus")
                .is_ok()
        );
    }
}
