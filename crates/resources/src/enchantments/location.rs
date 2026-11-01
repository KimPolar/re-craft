//! Location-dependent enchantment conditions and effects.
//!
//! Unsupported predicates and effects are rejected rather than being treated as
//! matches. This keeps a data pack from silently changing player behaviour.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

use super::{AttributeModifier, AttributeOperation, EquipmentSlot, LevelValue, load_single_tag};
use crate::packs::{ResourceId, ResourceProvider};

#[derive(Clone, Debug, PartialEq)]
enum BlockSet {
    Ids(BTreeSet<String>),
    Tag(ResourceId),
}

#[derive(Clone, Debug, PartialEq)]
enum Predicate {
    Always,
    All(Vec<Self>),
    Any(Vec<Self>),
    Not(Box<Self>),
    Active(bool),
    Flying(bool),
    OnGround(bool),
    Vehicle(bool),
    MovementBlock(BlockSet),
    Random(Chance),
}

#[derive(Clone, Debug, PartialEq)]
enum Chance {
    Constant(f32),
    EnchantmentLevel(LevelValue),
}

fn object<'a>(value: &'a Value, description: &str) -> Result<&'a serde_json::Map<String, Value>> {
    value
        .as_object()
        .with_context(|| format!("{description} is not an object"))
}

fn check_keys(value: &serde_json::Map<String, Value>, allowed: &[&str]) -> Result<()> {
    ensure!(
        value.keys().all(|key| allowed.contains(&key.as_str())),
        "unsupported location predicate field"
    );
    Ok(())
}

fn id(value: &Value, name: &str) -> Result<ResourceId> {
    ResourceId::parse(
        value
            .as_str()
            .with_context(|| format!("{name} is not an identifier"))?,
    )
}

fn canonical(id: ResourceId) -> String {
    format!("{}:{}", id.namespace, id.path)
}

fn finite_f32(value: &Value) -> Result<f32> {
    let value = value.as_f64().context("expected a number")? as f32;
    ensure!(value.is_finite(), "non-finite location value");
    Ok(value)
}

impl Predicate {
    fn parse(value: &Value, depth: usize) -> Result<Self> {
        ensure!(depth <= 32, "location predicate nesting is too deep");
        let value = object(value, "location predicate")?;
        let kind = id(
            value
                .get("type")
                .context("location predicate type is missing")?,
            "type",
        )?;
        ensure!(
            kind.namespace == "minecraft",
            "unsupported location predicate namespace"
        );
        match kind.path.as_str() {
            "all_of" | "any_of" => {
                check_keys(value, &["type", "terms"])?;
                let terms = value
                    .get("terms")
                    .and_then(Value::as_array)
                    .context("predicate terms are missing")?
                    .iter()
                    .map(|term| Self::parse(term, depth + 1))
                    .collect::<Result<_>>()?;
                Ok(if kind.path == "all_of" {
                    Self::All(terms)
                } else {
                    Self::Any(terms)
                })
            }
            "inverted" => {
                check_keys(value, &["type", "term"])?;
                Ok(Self::Not(Box::new(Self::parse(
                    value.get("term").context("missing inverted term")?,
                    depth + 1,
                )?)))
            }
            "enchantment_active_check" => {
                check_keys(value, &["type", "active"])?;
                Ok(Self::Active(
                    value
                        .get("active")
                        .and_then(Value::as_bool)
                        .context("active flag is missing")?,
                ))
            }
            "entity_properties" => {
                check_keys(value, &["type", "entity", "predicate"])?;
                ensure!(
                    value.get("entity").and_then(Value::as_str) == Some("this"),
                    "unsupported entity target"
                );
                let entity = object(
                    value
                        .get("predicate")
                        .context("entity predicate is missing")?,
                    "entity predicate",
                )?;
                check_keys(
                    entity,
                    &[
                        "minecraft:flags",
                        "minecraft:vehicle",
                        "minecraft:movement_affected_by",
                    ],
                )?;
                let mut terms = Vec::new();
                if let Some(flags) = entity.get("minecraft:flags") {
                    let flags = object(flags, "entity flags")?;
                    check_keys(flags, &["is_flying", "is_on_ground"])?;
                    for (key, constructor) in [
                        ("is_flying", Self::Flying as fn(bool) -> Self),
                        ("is_on_ground", Self::OnGround as fn(bool) -> Self),
                    ] {
                        if let Some(flag) = flags.get(key) {
                            terms.push(constructor(
                                flag.as_bool().context("entity flag is not boolean")?,
                            ));
                        }
                    }
                }
                if let Some(vehicle) = entity.get("minecraft:vehicle") {
                    ensure!(
                        object(vehicle, "vehicle predicate")?.is_empty(),
                        "unsupported vehicle predicate"
                    );
                    terms.push(Self::Vehicle(true));
                }
                if let Some(movement) = entity.get("minecraft:movement_affected_by") {
                    let movement = object(movement, "movement predicate")?;
                    check_keys(movement, &["block"])?;
                    let block = object(
                        movement
                            .get("block")
                            .context("movement block predicate is missing")?,
                        "block predicate",
                    )?;
                    check_keys(block, &["blocks"])?;
                    let blocks = block.get("blocks").context("block set is missing")?;
                    let set = if let Some(name) = blocks.as_str() {
                        if let Some(tag) = name.strip_prefix('#') {
                            BlockSet::Tag(ResourceId::parse(tag)?)
                        } else {
                            BlockSet::Ids(BTreeSet::from([canonical(ResourceId::parse(name)?)]))
                        }
                    } else {
                        let values = blocks.as_array().context("invalid block set")?;
                        BlockSet::Ids(
                            values
                                .iter()
                                .map(|value| Ok(canonical(id(value, "block")?)))
                                .collect::<Result<_>>()?,
                        )
                    };
                    terms.push(Self::MovementBlock(set));
                }
                Ok(Self::All(terms))
            }
            "random_chance" => {
                check_keys(value, &["type", "chance"])?;
                let chance = value.get("chance").context("chance is missing")?;
                let chance = if chance.is_number() {
                    Chance::Constant(finite_f32(chance)?)
                } else {
                    let chance = object(chance, "chance")?;
                    check_keys(chance, &["type", "amount"])?;
                    let kind = id(
                        chance.get("type").context("chance type is missing")?,
                        "chance type",
                    )?;
                    ensure!(
                        kind.namespace == "minecraft" && kind.path == "enchantment_level",
                        "unsupported chance provider"
                    );
                    Chance::EnchantmentLevel(LevelValue::parse(
                        chance.get("amount").context("chance amount is missing")?,
                        0,
                    )?)
                };
                Ok(Self::Random(chance))
            }
            _ => bail!("unsupported location predicate {}", kind.path),
        }
    }

    fn tags(&self, tags: &mut BTreeSet<ResourceId>) {
        match self {
            Self::All(terms) | Self::Any(terms) => terms.iter().for_each(|term| term.tags(tags)),
            Self::Not(term) => term.tags(tags),
            Self::MovementBlock(BlockSet::Tag(tag)) => {
                tags.insert(tag.clone());
            }
            _ => {}
        }
    }

    fn matches(
        &self,
        context: &LocationContext<'_>,
        tags: &BTreeMap<ResourceId, BTreeSet<String>>,
        random: &mut impl FnMut() -> f32,
    ) -> Result<bool> {
        Ok(match self {
            Self::Always => true,
            Self::All(terms) => {
                let mut result = true;
                for term in terms {
                    if !term.matches(context, tags, random)? {
                        result = false;
                        break;
                    }
                }
                result
            }
            Self::Any(terms) => {
                let mut result = false;
                for term in terms {
                    if term.matches(context, tags, random)? {
                        result = true;
                        break;
                    }
                }
                result
            }
            Self::Not(term) => !term.matches(context, tags, random)?,
            Self::Active(value) => context.active == *value,
            Self::Flying(value) => context.flying == *value,
            Self::OnGround(value) => context.on_ground == *value,
            Self::Vehicle(value) => context.vehicle == *value,
            Self::MovementBlock(BlockSet::Ids(ids)) => ids.contains(context.movement_block),
            Self::MovementBlock(BlockSet::Tag(tag)) => tags
                .get(tag)
                .context("location block tag unavailable")?
                .contains(context.movement_block),
            Self::Random(chance) => {
                random()
                    < match chance {
                        Chance::Constant(value) => *value,
                        Chance::EnchantmentLevel(value) => value.calculate(context.level),
                    }
            }
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Effect {
    All(Vec<Self>),
    Attribute {
        attribute: ResourceId,
        id: ResourceId,
        operation: AttributeOperation,
        amount: LevelValue,
    },
    ChangeItemDamage(LevelValue),
}

impl Effect {
    fn parse(value: &Value, depth: usize) -> Result<Self> {
        ensure!(depth <= 32, "location effect nesting is too deep");
        let value = object(value, "location effect")?;
        let kind = id(
            value
                .get("type")
                .context("location effect type is missing")?,
            "effect type",
        )?;
        ensure!(
            kind.namespace == "minecraft",
            "unsupported location effect namespace"
        );
        match kind.path.as_str() {
            "all_of" => {
                check_keys(value, &["type", "effects"])?;
                Ok(Self::All(
                    value
                        .get("effects")
                        .and_then(Value::as_array)
                        .context("location effects are missing")?
                        .iter()
                        .map(|effect| Self::parse(effect, depth + 1))
                        .collect::<Result<_>>()?,
                ))
            }
            "attribute" => {
                check_keys(value, &["type", "id", "attribute", "operation", "amount"])?;
                Ok(Self::Attribute {
                    attribute: id(
                        value.get("attribute").context("attribute is missing")?,
                        "attribute",
                    )?,
                    id: id(
                        value.get("id").context("modifier id is missing")?,
                        "modifier id",
                    )?,
                    operation: AttributeOperation::parse(
                        value
                            .get("operation")
                            .and_then(Value::as_str)
                            .context("operation is missing")?,
                    )?,
                    amount: LevelValue::parse(
                        value.get("amount").context("amount is missing")?,
                        0,
                    )?,
                })
            }
            "change_item_damage" => {
                check_keys(value, &["type", "amount"])?;
                Ok(Self::ChangeItemDamage(LevelValue::parse(
                    value.get("amount").context("amount is missing")?,
                    0,
                )?))
            }
            _ => bail!("unsupported location effect {}", kind.path),
        }
    }

    fn actions(
        &self,
        level: u32,
        slot: EquipmentSlot,
        before: bool,
        after: bool,
        result: &mut Vec<LocationAction>,
    ) -> Result<()> {
        match self {
            Self::All(effects) => {
                for effect in effects {
                    effect.actions(level, slot, before, after, result)?;
                }
            }
            Self::Attribute {
                attribute,
                id,
                operation,
                amount,
            } => {
                let amount = amount.calculate(level);
                ensure!(amount.is_finite(), "non-finite location attribute");
                if before != after {
                    let modifier = AttributeModifier {
                        id: ResourceId {
                            namespace: id.namespace.clone(),
                            path: format!("{}/{}", id.path, slot.name()),
                        },
                        operation: *operation,
                        amount: f64::from(amount),
                    };
                    result.push(if after {
                        LocationAction::AddAttribute {
                            attribute: attribute.clone(),
                            modifier,
                        }
                    } else {
                        LocationAction::RemoveAttribute {
                            attribute: attribute.clone(),
                            modifier,
                        }
                    });
                }
            }
            Self::ChangeItemDamage(amount) => {
                let amount = amount.calculate(level);
                ensure!(amount.is_finite(), "non-finite location item damage");
                if after {
                    result.push(LocationAction::ChangeItemDamage(amount));
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Conditional {
    predicate: Predicate,
    effect: Effect,
}

/// Context is evaluated independently for each conditional effect, using that
/// effect's previous activation state. Random samples are consumed in order.
pub struct LocationContext<'a> {
    pub active: bool,
    /// True for creative flight or fall flying, matching the entity predicate.
    pub flying: bool,
    pub on_ground: bool,
    /// True when this entity is riding another entity.
    pub vehicle: bool,
    pub movement_block: &'a str,
    pub level: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LocationAction {
    AddAttribute {
        attribute: ResourceId,
        modifier: AttributeModifier,
    },
    RemoveAttribute {
        attribute: ResourceId,
        modifier: AttributeModifier,
    },
    ChangeItemDamage(f32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct LocationEnchantment {
    slots: Vec<String>,
    effects: Vec<Conditional>,
    tags: BTreeMap<ResourceId, BTreeSet<String>>,
}

impl LocationEnchantment {
    /// Parse one definition and resolve its block tags through the active data packs.
    pub fn load(provider: &mut impl ResourceProvider, value: &Value) -> Result<Self> {
        let definition = super::definition(value)?;
        let mut entries = None;
        if let Some(effect_map) = value.get("effects") {
            let effect_map = object(effect_map, "enchantment effects")?;
            for key in [
                "minecraft:location_changed",
                "location_changed",
                ":location_changed",
            ] {
                if let Some(value) = effect_map.get(key) {
                    ensure!(entries.is_none(), "duplicate location effect component");
                    entries = Some(value);
                }
            }
        }
        let mut effects = Vec::new();
        if let Some(entries) = entries {
            let entries = entries
                .as_array()
                .context("location effects are not a list")?;
            ensure!(entries.len() <= 256, "too many location effects");
            for entry in entries {
                let entry = object(entry, "conditional location effect")?;
                check_keys(entry, &["effect", "requirements"])?;
                effects.push(Conditional {
                    effect: Effect::parse(
                        entry.get("effect").context("location effect is missing")?,
                        0,
                    )?,
                    predicate: entry
                        .get("requirements")
                        .map(|value| Predicate::parse(value, 0))
                        .transpose()?
                        .unwrap_or(Predicate::Always),
                });
            }
        }
        for (index, effect) in effects.iter().enumerate() {
            ensure!(
                !effects[..index]
                    .iter()
                    .any(|previous| previous.effect == effect.effect),
                "duplicate location effect identity is unsupported"
            );
        }
        let mut ids = BTreeSet::new();
        for effect in &effects {
            effect.predicate.tags(&mut ids);
        }
        let tags = ids
            .into_iter()
            .map(|id| Ok((id.clone(), load_single_tag(provider, "block", &id)?)))
            .collect::<Result<_>>()?;
        Ok(Self {
            slots: definition.slots,
            effects,
            tags,
        })
    }

    pub fn len(&self) -> usize {
        self.effects.len()
    }
    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }

    /// Return the new activation state and actions for one effect. Callers that
    /// need transactional failure handling must stage their random source.
    pub fn evaluate(
        &self,
        index: usize,
        slot: EquipmentSlot,
        context: &LocationContext<'_>,
        random: &mut impl FnMut() -> f32,
    ) -> Result<(bool, Vec<LocationAction>)> {
        ensure!(
            (1..=255).contains(&context.level),
            "invalid enchantment level"
        );
        let effect = self.effects.get(index).context("unknown location effect")?;
        let active = self.slots.iter().any(|group| slot.matches(group))
            && effect.predicate.matches(context, &self.tags, random)?;
        let mut actions = Vec::new();
        if active || context.active {
            effect
                .effect
                .actions(context.level, slot, context.active, active, &mut actions)?;
        }
        Ok((active, actions))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Tags;
    impl ResourceProvider for Tags {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn list_data(&mut self, _: &str) -> Result<Vec<ResourceId>> {
            Ok(Vec::new())
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok((path == "data/example/tags/block/ground.json")
                .then(|| br#"{"values":["minecraft:soul_sand","minecraft:soul_soil"]}"#.to_vec()))
        }
    }

    fn sample() -> Value {
        json!({"slots":["feet"],"effects":{"minecraft:location_changed":[
            {"effect":{"type":"minecraft:all_of","effects":[
                {"type":"minecraft:attribute","id":"example:boost","attribute":"minecraft:movement_speed","operation":"add_value","amount":{"type":"minecraft:linear","base":0.04,"per_level_above_first":0.01}},
                {"type":"minecraft:attribute","id":"example:boost","attribute":"minecraft:movement_efficiency","operation":"add_value","amount":1.0}
            ]},"requirements":{"type":"minecraft:all_of","terms":[
                {"type":"minecraft:inverted","term":{"type":"minecraft:entity_properties","entity":"this","predicate":{"minecraft:vehicle":{}}}},
                {"type":"minecraft:entity_properties","entity":"this","predicate":{"minecraft:flags":{"is_flying":false},"minecraft:movement_affected_by":{"block":{"blocks":"#example:ground"}}}}
            ]}},
            {"effect":{"type":"minecraft:change_item_damage","amount":1.0},"requirements":{"type":"minecraft:all_of","terms":[
                {"type":"minecraft:random_chance","chance":{"type":"minecraft:enchantment_level","amount":0.04}},
                {"type":"minecraft:entity_properties","entity":"this","predicate":{"minecraft:flags":{"is_on_ground":true},"minecraft:movement_affected_by":{"block":{"blocks":"#example:ground"}}}}
            ]}}
        ]}})
    }

    #[test]
    fn conditions_resolve_tags_and_preserve_random_order() {
        let enchantment = LocationEnchantment::load(&mut Tags, &sample()).unwrap();
        assert_eq!(enchantment.len(), 2);
        for level in [1, 3, 255] {
            for on_ground in [false, true] {
                for block in ["minecraft:soul_sand", "minecraft:stone"] {
                    let context = LocationContext {
                        active: false,
                        flying: false,
                        on_ground,
                        vehicle: false,
                        movement_block: block,
                        level,
                    };
                    let mut calls = 0;
                    let (active, actions) = enchantment
                        .evaluate(0, EquipmentSlot::Feet, &context, &mut || {
                            calls += 1;
                            0.0
                        })
                        .unwrap();
                    assert_eq!(active, block == "minecraft:soul_sand");
                    assert_eq!(calls, 0);
                    if active {
                        assert_eq!(actions.len(), 2);
                        let LocationAction::AddAttribute { modifier, .. } = &actions[0] else {
                            panic!()
                        };
                        assert_eq!(modifier.id.path, "boost/feet");
                        assert_eq!(
                            modifier.amount,
                            f64::from(0.04_f32 + 0.01_f32 * (level as f32 - 1.0))
                        );
                    } else {
                        assert!(actions.is_empty());
                    }
                    let (active, actions) = enchantment
                        .evaluate(1, EquipmentSlot::Feet, &context, &mut || {
                            calls += 1;
                            0.039
                        })
                        .unwrap();
                    assert_eq!(calls, 1);
                    assert_eq!(active, on_ground && block == "minecraft:soul_sand");
                    assert_eq!(
                        actions,
                        if active {
                            vec![LocationAction::ChangeItemDamage(1.0)]
                        } else {
                            vec![]
                        }
                    );
                    // A constant enchantment-level provider is constant even at higher levels.
                    let (active, _) = enchantment
                        .evaluate(1, EquipmentSlot::Feet, &context, &mut || 0.041)
                        .unwrap();
                    assert!(!active);
                }
            }
        }
    }

    #[test]
    fn wrong_slots_and_unsupported_predicates_fail_closed() {
        let mut definition = sample();
        let enchantment = LocationEnchantment::load(&mut Tags, &definition).unwrap();
        let context = LocationContext {
            active: false,
            flying: false,
            on_ground: true,
            vehicle: false,
            movement_block: "minecraft:soul_sand",
            level: 1,
        };
        assert_eq!(
            enchantment
                .evaluate(1, EquipmentSlot::Head, &context, &mut || panic!(
                    "wrong slot consumed RNG"
                ))
                .unwrap(),
            (false, Vec::new())
        );
        definition["effects"]["minecraft:location_changed"][0]["requirements"]["terms"][1]["predicate"]
            ["minecraft:flags"]["is_baby"] = json!(true);
        assert!(LocationEnchantment::load(&mut Tags, &definition).is_err());
    }

    #[test]
    fn location_component_accepts_default_namespace_and_rejects_duplicates() {
        let original = sample();
        let expected = LocationEnchantment::load(&mut Tags, &original).unwrap();
        for key in ["location_changed", ":location_changed"] {
            let mut definition = original.clone();
            let effects = definition["effects"].as_object_mut().unwrap();
            let value = effects.remove("minecraft:location_changed").unwrap();
            effects.insert(key.into(), value.clone());
            assert_eq!(
                LocationEnchantment::load(&mut Tags, &definition).unwrap(),
                expected
            );
            definition["effects"]["minecraft:location_changed"] = value;
            assert!(LocationEnchantment::load(&mut Tags, &definition).is_err());
        }
    }

    #[test]
    fn activation_and_deactivation_only_change_attributes_on_transitions() {
        let enchantment = LocationEnchantment::load(&mut Tags, &sample()).unwrap();
        let mut context = LocationContext {
            active: false,
            flying: false,
            on_ground: true,
            vehicle: false,
            movement_block: "minecraft:soul_sand",
            level: 2,
        };
        let (active, added) = enchantment
            .evaluate(0, EquipmentSlot::Feet, &context, &mut || panic!())
            .unwrap();
        assert!(active);
        assert_eq!(added.len(), 2);
        assert!(
            added
                .iter()
                .all(|action| matches!(action, LocationAction::AddAttribute { .. }))
        );
        context.active = true;
        assert_eq!(
            enchantment
                .evaluate(0, EquipmentSlot::Feet, &context, &mut || panic!())
                .unwrap(),
            (true, Vec::new())
        );
        context.movement_block = "minecraft:stone";
        let (active, removed) = enchantment
            .evaluate(0, EquipmentSlot::Feet, &context, &mut || panic!())
            .unwrap();
        assert!(!active);
        assert_eq!(removed.len(), 2);
        for (add, remove) in added.iter().zip(&removed) {
            match (add, remove) {
                (
                    LocationAction::AddAttribute {
                        attribute: a,
                        modifier: am,
                    },
                    LocationAction::RemoveAttribute {
                        attribute: b,
                        modifier: bm,
                    },
                ) => {
                    assert_eq!((a, am), (b, bm));
                }
                _ => panic!("wrong transition actions"),
            }
        }
        let (active, removed) = enchantment
            .evaluate(0, EquipmentSlot::Head, &context, &mut || panic!())
            .unwrap();
        assert!(!active);
        assert_eq!(removed.len(), 2);
    }

    #[test]
    fn location_block_tags_follow_data_pack_replacement_and_nested_tags() {
        struct Layered;
        impl ResourceProvider for Layered {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn list_data(&mut self, _: &str) -> Result<Vec<ResourceId>> {
                Ok(Vec::new())
            }
            fn read_data_stack(&mut self, path: &str) -> Result<Vec<Vec<u8>>> {
                Ok(match path {
                    "data/example/tags/block/ground.json" => vec![
                        br#"{"values":["minecraft:soul_sand"]}"#.to_vec(),
                        br##"{"replace":true,"values":["#example:extra"]}"##.to_vec(),
                    ],
                    "data/example/tags/block/extra.json" => {
                        vec![br#"{"values":["minecraft:stone"]}"#.to_vec()]
                    }
                    _ => Vec::new(),
                })
            }
        }
        let enchantment = LocationEnchantment::load(&mut Layered, &sample()).unwrap();
        for (block, expected) in [("minecraft:stone", true), ("minecraft:soul_sand", false)] {
            let context = LocationContext {
                active: false,
                flying: false,
                on_ground: true,
                vehicle: false,
                movement_block: block,
                level: 1,
            };
            assert_eq!(
                enchantment
                    .evaluate(0, EquipmentSlot::Feet, &context, &mut || panic!())
                    .unwrap()
                    .0,
                expected
            );
        }
    }

    #[test]
    #[ignore = "requires the installed Minecraft 26.3 assets"]
    fn installed_soul_speed_definition_and_tags_resolve() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let mut resources = crate::packs::ResourceStack::open(&game).unwrap();
        let bytes = resources
            .read_data("data/minecraft/enchantment/soul_speed.json")
            .unwrap()
            .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        let enchantment = LocationEnchantment::load(&mut resources, &value).unwrap();
        let registry = super::super::OxygenEnchantments::load(&mut resources).unwrap();
        assert_eq!(
            registry
                .location_effects(&ResourceId::parse("minecraft:soul_speed").unwrap())
                .unwrap(),
            &enchantment
        );
        // Unsupported definitions do not prevent unrelated enchantments from loading.
        assert!(
            registry
                .location_effects(&ResourceId::parse("minecraft:frost_walker").unwrap())
                .is_err()
        );
        assert_eq!(enchantment.len(), 2);
        for previous in [false, true] {
            for flying in [false, true] {
                for vehicle in [false, true] {
                    for on_ground in [false, true] {
                        for block in [
                            "minecraft:soul_sand",
                            "minecraft:soul_soil",
                            "minecraft:stone",
                        ] {
                            let context = LocationContext {
                                active: previous,
                                flying,
                                on_ground,
                                vehicle,
                                movement_block: block,
                                level: 3,
                            };
                            let (active, actions) = enchantment
                                .evaluate(0, EquipmentSlot::Feet, &context, &mut || {
                                    panic!("unexpected random condition")
                                })
                                .unwrap();
                            let tagged = block != "minecraft:stone";
                            assert_eq!(
                                active,
                                !vehicle && !flying && (tagged || (previous && !on_ground)),
                                "{block} {previous} {flying} {vehicle} {on_ground}"
                            );
                            assert_eq!(actions.len(), if active != previous { 2 } else { 0 });
                            let mut calls = 0;
                            let (active, _) = enchantment
                                .evaluate(1, EquipmentSlot::Feet, &context, &mut || {
                                    calls += 1;
                                    0.039
                                })
                                .unwrap();
                            assert_eq!(calls, 1);
                            assert_eq!(active, on_ground && tagged);
                            assert!(
                                !enchantment
                                    .evaluate(1, EquipmentSlot::Feet, &context, &mut || 0.041)
                                    .unwrap()
                                    .0
                            );
                        }
                    }
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn soul_speed_conditions_match_java_loot_context() {
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
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("LocationEnchantmentOracle.java");
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(
            &source,
            include_str!("../../tests/java/LocationEnchantmentOracle.java"),
        )
        .unwrap();
        let mut inputs = Vec::new();
        for index in 0..2 {
            for level in [1, 3] {
                for seed in [0, 1, 4096] {
                    for active in [false, true] {
                        for flying in [false, true] {
                            for passenger in [false, true] {
                                for ground in [false, true] {
                                    for block in ["soul_sand", "stone"] {
                                        inputs.push(json!({"index":index,"level":level,"seed":seed,"active":active,"flying":flying,"passenger":passenger,"ground":ground,"block":block}));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        std::fs::write(&input, serde_json::to_vec(&inputs).unwrap()).unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("LocationEnchantmentOracle")
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
        let expected: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(inputs.len(), expected.len());
        let mut resources = crate::packs::ResourceStack::open(&game).unwrap();
        let registry = super::super::OxygenEnchantments::load(&mut resources).unwrap();
        let enchantment = registry
            .location_effects(&ResourceId::parse("minecraft:soul_speed").unwrap())
            .unwrap();
        for (input, expected) in inputs.iter().zip(expected) {
            let mut random =
                native_core::random::LegacyRandom::new(input["seed"].as_i64().unwrap());
            let block = format!("minecraft:{}", input["block"].as_str().unwrap());
            let context = LocationContext {
                active: input["active"].as_bool().unwrap(),
                flying: input["flying"].as_bool().unwrap(),
                on_ground: input["ground"].as_bool().unwrap(),
                vehicle: input["passenger"].as_bool().unwrap(),
                movement_block: &block,
                level: input["level"].as_u64().unwrap() as u32,
            };
            let (matched, _) = enchantment
                .evaluate(
                    input["index"].as_u64().unwrap() as usize,
                    EquipmentSlot::Feet,
                    &context,
                    &mut || random.next_float(),
                )
                .unwrap();
            assert_eq!(matched, expected["matched"].as_bool().unwrap(), "{input}");
            assert_eq!(
                random.next_int(),
                expected["next"].as_i64().unwrap() as i32,
                "{input}"
            );
        }
    }
}
