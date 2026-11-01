//! Player health and the vanilla food-data tick.

use crate::inventory::{ItemStack, PlayerInventory, PlayerSlot, saved_item_max};
use native_persistence::nbt::Tag;
use native_persistence::world::Difficulty;
use native_resources::packs::ResourceId;
use std::sync::Arc;

/// Supported food properties. Consumption effects and container remainders
/// are handled separately after the food is consumed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FoodProperties {
    pub nutrition: i32,
    pub saturation: f32,
    pub use_ticks: i32,
    pub can_always_eat: bool,
    pub consume_particles: bool,
    pub sound: FoodSound,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FoodSound {
    Eat,
    HoneyDrink,
    PotionDrink,
}

pub fn ordinary_food(id: &str) -> Option<FoodProperties> {
    let (nutrition, modifier) = match id.strip_prefix("minecraft:")? {
        "apple" => (4, 0.3),
        "baked_potato" | "bread" | "cooked_cod" | "cooked_rabbit" => (5, 0.6),
        "beef" | "porkchop" | "rabbit" => (3, 0.3),
        "beetroot" => (1, 0.6),
        "carrot" => (3, 0.6),
        "cod" | "salmon" | "cookie" | "sweet_berries" | "glow_berries" => (2, 0.1),
        "rotten_flesh" => (4, 0.1),
        "honey_bottle" => (6, 0.1),
        "chicken" | "poisonous_potato" => (2, 0.3),
        "spider_eye" => (2, 0.8),
        "cooked_beef" | "cooked_porkchop" => (8, 0.8),
        "cooked_chicken" | "mushroom_stew" | "beetroot_soup" => (6, 0.6),
        "rabbit_stew" => (10, 0.6),
        "cooked_mutton" | "cooked_salmon" => (6, 0.8),
        "dried_kelp" => (1, 0.3),
        "golden_carrot" => (6, 1.2),
        "melon_slice" | "mutton" => (2, 0.3),
        "potato" => (1, 0.3),
        "pumpkin_pie" => (8, 0.3),
        "tropical_fish" => (1, 0.1),
        _ => return None,
    };
    Some(FoodProperties {
        nutrition,
        saturation: nutrition as f32 * modifier * 2.0,
        use_ticks: match id {
            "minecraft:dried_kelp" => 16,
            "minecraft:honey_bottle" => 40,
            _ => 32,
        },
        can_always_eat: id == "minecraft:honey_bottle",
        consume_particles: id != "minecraft:honey_bottle",
        sound: if id == "minecraft:honey_bottle" {
            FoodSound::HoneyDrink
        } else {
            FoodSound::Eat
        },
    })
}

/// The supported, effect-free subset of a saved FOOD component.
pub(crate) fn custom_food(value: &Tag) -> Option<(i32, f32, bool)> {
    let Tag::Compound(fields) = value else {
        return None;
    };
    let (mut nutrition, mut saturation, mut always) = (None, None, None);
    for (name, value) in fields {
        if name.matches("nutrition") && nutrition.is_none() {
            nutrition = value.as_java_int().filter(|&value| value >= 0);
            nutrition?;
        } else if name.matches("saturation") && saturation.is_none() {
            let number = crate::player_data::numeric_double(value)? as f32;
            if !number.is_finite() {
                return None;
            }
            saturation = Some(number);
        } else if name.matches("can_always_eat") && always.is_none() {
            always = match value {
                Tag::Byte(0) => Some(false),
                Tag::Byte(1) => Some(true),
                _ => return None,
            };
        } else {
            return None;
        }
    }
    Some((nutrition?, saturation?, always.unwrap_or(false)))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FoodStatusKind {
    Hunger,
    Poison,
    NightVision,
    WaterBreathing,
    Regeneration,
    JumpBoost,
    SlowFalling,
    Speed,
    Slowness,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct StatusApplication {
    pub kind: FoodStatusKind,
    pub duration: i32,
    pub amplifier: u8,
    pub ambient: bool,
    pub visible: bool,
    pub show_icon: bool,
}

#[derive(Clone, Debug)]
struct StatusGroup {
    probability: f32,
    effects: Vec<StatusApplication>,
}

#[derive(Clone, Debug)]
enum ConsumeStatusAction {
    Apply(StatusGroup),
    Remove(Vec<FoodStatusKind>),
    ClearAll,
}

#[derive(Clone, Debug)]
struct CustomConsumable {
    ticks: i32,
    particles: bool,
    effects: Vec<ConsumeStatusAction>,
}

fn parse_status_kind(value: &native_persistence::nbt::JavaString) -> Option<FoodStatusKind> {
    if value.matches("minecraft:hunger") || value.matches("hunger") {
        Some(FoodStatusKind::Hunger)
    } else if value.matches("minecraft:poison") || value.matches("poison") {
        Some(FoodStatusKind::Poison)
    } else if value.matches("minecraft:night_vision") || value.matches("night_vision") {
        Some(FoodStatusKind::NightVision)
    } else if value.matches("minecraft:water_breathing") || value.matches("water_breathing") {
        Some(FoodStatusKind::WaterBreathing)
    } else if value.matches("minecraft:regeneration") || value.matches("regeneration") {
        Some(FoodStatusKind::Regeneration)
    } else if value.matches("minecraft:jump_boost") || value.matches("jump_boost") {
        Some(FoodStatusKind::JumpBoost)
    } else if value.matches("minecraft:slow_falling") || value.matches("slow_falling") {
        Some(FoodStatusKind::SlowFalling)
    } else if value.matches("minecraft:speed") || value.matches("speed") {
        Some(FoodStatusKind::Speed)
    } else if value.matches("minecraft:slowness") || value.matches("slowness") {
        Some(FoodStatusKind::Slowness)
    } else {
        None
    }
}

/// The potion component codec accepts both an ID string and a full record.
pub fn potion_contents_potion(contents: &Tag) -> Option<&native_persistence::nbt::JavaString> {
    match contents {
        Tag::String(id) => Some(id),
        Tag::Compound(_) => match contents.get("potion") {
            Some(Tag::String(id)) => Some(id),
            _ => None,
        },
        _ => None,
    }
}

fn potion_id(components: &Tag) -> Option<ResourceId> {
    let contents = components.get("minecraft:potion_contents")?;
    if let Tag::Compound(fields) = contents {
        let mut potion = false;
        let mut empty_effects = false;
        let mut color = false;
        let mut custom_name = false;
        for (name, value) in fields {
            if name.matches("potion") && !potion && matches!(value, Tag::String(_)) {
                potion = true;
            } else if name.matches("custom_effects")
                && !empty_effects
                && matches!(value, Tag::List { values, .. } if values.is_empty())
            {
                empty_effects = true;
            } else if name.matches("custom_color") && !color && value.as_java_int().is_some() {
                color = true;
            } else if name.matches("custom_name") && !custom_name && matches!(value, Tag::String(_))
            {
                custom_name = true;
            } else {
                return None;
            }
        }
        if !potion {
            return None;
        }
    }
    let id = potion_contents_potion(contents)?;
    let id = String::from_utf16(&id.0).ok()?;
    ResourceId::parse(&id)
        .ok()
        .filter(|id| id.namespace == "minecraft")
}

pub(crate) fn is_plain_water_potion(components: &Tag) -> bool {
    potion_id(components).is_some_and(|id| id.path == "water")
}

/// Recognize the built-in contents whose inventory representation is supported.
pub(crate) fn inventory_potion_contents(components: &Tag) -> Option<()> {
    let id = potion_id(components)?;
    matches!(
        id.path.as_str(),
        "water"
            | "mundane"
            | "thick"
            | "awkward"
            | "night_vision"
            | "long_night_vision"
            | "water_breathing"
            | "long_water_breathing"
            | "poison"
            | "long_poison"
            | "strong_poison"
            | "slowness"
            | "long_slowness"
            | "strong_slowness"
            | "swiftness"
            | "long_swiftness"
            | "strong_swiftness"
            | "regeneration"
            | "long_regeneration"
            | "strong_regeneration"
            | "leaping"
            | "long_leaping"
            | "strong_leaping"
            | "slow_falling"
            | "long_slow_falling"
            | "strength"
    )
    .then_some(())
}

/// Resolve the built-in potion definitions whose effects this session simulates.
pub(crate) fn potion_effects(components: &Tag) -> Option<Vec<StatusApplication>> {
    if components.get("minecraft:potion_contents").is_none() {
        return Some(Vec::new());
    }
    let id = potion_id(components)?;
    if matches!(id.path.as_str(), "water" | "mundane" | "thick" | "awkward") {
        return Some(Vec::new());
    }
    let (kind, duration, amplifier) = match id.path.as_str() {
        "night_vision" => (FoodStatusKind::NightVision, 3600, 0),
        "long_night_vision" => (FoodStatusKind::NightVision, 9600, 0),
        "water_breathing" => (FoodStatusKind::WaterBreathing, 3600, 0),
        "long_water_breathing" => (FoodStatusKind::WaterBreathing, 9600, 0),
        "poison" => (FoodStatusKind::Poison, 900, 0),
        "long_poison" => (FoodStatusKind::Poison, 1800, 0),
        "strong_poison" => (FoodStatusKind::Poison, 432, 1),
        "regeneration" => (FoodStatusKind::Regeneration, 900, 0),
        "long_regeneration" => (FoodStatusKind::Regeneration, 1800, 0),
        "strong_regeneration" => (FoodStatusKind::Regeneration, 450, 1),
        "leaping" => (FoodStatusKind::JumpBoost, 3600, 0),
        "long_leaping" => (FoodStatusKind::JumpBoost, 9600, 0),
        "strong_leaping" => (FoodStatusKind::JumpBoost, 1800, 1),
        "slow_falling" => (FoodStatusKind::SlowFalling, 1800, 0),
        "long_slow_falling" => (FoodStatusKind::SlowFalling, 4800, 0),
        "slowness" => (FoodStatusKind::Slowness, 1800, 0),
        "long_slowness" => (FoodStatusKind::Slowness, 4800, 0),
        "strong_slowness" => (FoodStatusKind::Slowness, 400, 3),
        "swiftness" => (FoodStatusKind::Speed, 3600, 0),
        "long_swiftness" => (FoodStatusKind::Speed, 9600, 0),
        "strong_swiftness" => (FoodStatusKind::Speed, 1800, 1),
        _ => return None,
    };
    Some(vec![StatusApplication {
        kind,
        duration,
        amplifier,
        ambient: false,
        visible: true,
        show_icon: true,
    }])
}

/// Color of supported built-in potion contents; absent/unsupported uses the model default.
pub fn potion_color(components: &Tag) -> Option<u32> {
    let contents = components.get("minecraft:potion_contents")?;
    if let Some(color) = contents.get("custom_color").and_then(Tag::as_java_int) {
        potion_id(components)?;
        return Some(color as u32 & 0x00ff_ffff);
    }
    match potion_id(components)?.path.as_str() {
        "night_vision" | "long_night_vision" => Some(0xc2ff66),
        "water_breathing" | "long_water_breathing" => Some(0x98dac0),
        "poison" | "long_poison" | "strong_poison" => Some(0x87a363),
        "slowness" | "long_slowness" | "strong_slowness" => Some(0x8bafe0),
        "swiftness" | "long_swiftness" | "strong_swiftness" => Some(0x33ebff),
        "regeneration" | "long_regeneration" | "strong_regeneration" => Some(0xcd5cab),
        "leaping" | "long_leaping" | "strong_leaping" => Some(0xfdff84),
        "slow_falling" | "long_slow_falling" => Some(0xf3cfb9),
        "strength" => Some(0xffc700),
        _ => None,
    }
}

fn parse_status_effect(value: &Tag) -> Option<StatusApplication> {
    let Tag::Compound(fields) = value else {
        return None;
    };
    let (mut id, mut duration, mut amplifier, mut ambient, mut visible, mut icon) =
        (None, None, None, None, None, None);
    for (name, value) in fields {
        if name.matches("id") && id.is_none() {
            let Tag::String(value) = value else {
                return None;
            };
            id = Some(parse_status_kind(value)?);
        } else if name.matches("duration") && duration.is_none() {
            duration = value.as_java_int().filter(|value| *value >= -1);
            duration?;
        } else if name.matches("amplifier") && amplifier.is_none() {
            let Tag::Byte(value) = value else { return None };
            amplifier = Some(*value as u8);
        } else if name.matches("ambient") && ambient.is_none() {
            ambient = match value {
                Tag::Byte(0) => Some(false),
                Tag::Byte(1) => Some(true),
                _ => return None,
            };
        } else if name.matches("show_particles") && visible.is_none() {
            visible = match value {
                Tag::Byte(0) => Some(false),
                Tag::Byte(1) => Some(true),
                _ => return None,
            };
        } else if name.matches("show_icon") && icon.is_none() {
            icon = match value {
                Tag::Byte(0) => Some(false),
                Tag::Byte(1) => Some(true),
                _ => return None,
            };
        } else {
            return None;
        }
    }
    let kind = id?;
    let visible = visible.unwrap_or(true);
    Some(StatusApplication {
        kind,
        duration: duration.unwrap_or(0),
        amplifier: amplifier.unwrap_or(0),
        ambient: ambient.unwrap_or(false),
        visible,
        show_icon: icon.unwrap_or(visible),
    })
}

fn parse_status_group(value: &Tag, collect: bool) -> Option<StatusGroup> {
    let Tag::Compound(fields) = value else {
        return None;
    };
    let (mut kind, mut effects, mut probability) = (None, None, None);
    for (name, value) in fields {
        if name.matches("type") && kind.is_none() {
            let Tag::String(value) = value else {
                return None;
            };
            if !value.matches("minecraft:apply_effects") && !value.matches("apply_effects") {
                return None;
            }
            kind = Some(());
        } else if name.matches("effects") && effects.is_none() {
            let Tag::List {
                element_type,
                values,
            } = value
            else {
                return None;
            };
            if *element_type != native_persistence::nbt::TagType::Compound && !values.is_empty() {
                return None;
            }
            let mut result = Vec::new();
            for value in values {
                let effect = parse_status_effect(value)?;
                if collect {
                    result.push(effect);
                }
            }
            effects = Some(result);
        } else if name.matches("probability") && probability.is_none() {
            let value = crate::player_data::numeric_double(value)? as f32;
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return None;
            }
            probability = Some(value);
        } else {
            return None;
        }
    }
    kind?;
    Some(StatusGroup {
        probability: probability.unwrap_or(1.0),
        effects: effects?,
    })
}

fn parse_consume_action(value: &Tag, collect: bool) -> Option<ConsumeStatusAction> {
    let Tag::Compound(fields) = value else {
        return None;
    };
    let mut kind = None;
    let mut effects = None;
    for (name, value) in fields {
        if name.matches("type") && kind.is_none() {
            let Tag::String(value) = value else {
                return None;
            };
            kind = Some(value);
        } else if name.matches("effects") && effects.is_none() {
            effects = Some(value);
        }
    }
    let kind = kind?;
    if kind.matches("minecraft:clear_all_effects") || kind.matches("clear_all_effects") {
        return (fields.len() == 1).then_some(ConsumeStatusAction::ClearAll);
    }
    if kind.matches("minecraft:apply_effects") || kind.matches("apply_effects") {
        return Some(ConsumeStatusAction::Apply(parse_status_group(
            value, collect,
        )?));
    }
    if !kind.matches("minecraft:remove_effects") && !kind.matches("remove_effects") {
        return None;
    }
    if fields.len() != 2 {
        return None;
    }
    let mut result = Vec::new();
    let mut add = |entry: &Tag| -> Option<()> {
        let Tag::String(id) = entry else { return None };
        let status = parse_status_kind(id)?;
        if collect {
            result.push(status);
        }
        Some(())
    };
    match effects? {
        value @ Tag::String(_) => add(value)?,
        Tag::List {
            element_type,
            values,
        } if *element_type == native_persistence::nbt::TagType::String || values.is_empty() => {
            for value in values {
                add(value)?;
            }
        }
        _ => return None,
    }
    Some(ConsumeStatusAction::Remove(result))
}

fn parse_consumable(value: &Tag, collect: bool) -> Option<CustomConsumable> {
    let Tag::Compound(fields) = value else {
        return None;
    };
    let (mut seconds, mut animation, mut effects, mut particles) = (None, false, None, None);
    for (name, value) in fields {
        if name.matches("consume_seconds") && seconds.is_none() {
            let number = crate::player_data::numeric_double(value)? as f32;
            if !number.is_finite() || number < 0.0 {
                return None;
            }
            seconds = Some(number);
        } else if name.matches("animation") && !animation {
            if !matches!(value, Tag::String(value) if value.matches("eat")) {
                return None;
            }
            animation = true;
        } else if name.matches("on_consume_effects") && effects.is_none() {
            let Tag::List {
                element_type,
                values,
            } = value
            else {
                return None;
            };
            if *element_type != native_persistence::nbt::TagType::Compound && !values.is_empty() {
                return None;
            }
            let mut result = Vec::new();
            for value in values {
                let group = parse_consume_action(value, collect)?;
                if collect {
                    result.push(group);
                }
            }
            effects = Some(result);
        } else if name.matches("has_consume_particles") && particles.is_none() {
            particles = match value {
                Tag::Byte(0) => Some(false),
                Tag::Byte(1) => Some(true),
                _ => return None,
            };
        } else {
            return None;
        }
    }
    Some(CustomConsumable {
        ticks: (seconds.unwrap_or(1.6_f32) * 20.0) as i32,
        particles: particles.unwrap_or(true),
        effects: effects.unwrap_or_default(),
    })
}

pub(crate) fn custom_consumable_ticks(value: &Tag) -> Option<i32> {
    Some(parse_consumable(value, false)?.ticks)
}

/// Decode a use-remainder item template when its stack behavior is known.
/// Nested remainder patches are refused until component resolution is bounded.
pub(crate) fn parse_use_remainder(value: &Tag) -> Option<ItemStack> {
    let (raw_id, count, components) = match value {
        Tag::String(id) => (String::from_utf16(&id.0).ok()?, 1, Tag::Compound(vec![])),
        Tag::Compound(fields) => {
            let (mut id, mut count, mut components) = (None, None, None);
            for (name, value) in fields {
                if name.matches("id") && id.is_none() {
                    let Tag::String(value) = value else {
                        return None;
                    };
                    id = Some(String::from_utf16(&value.0).ok()?);
                } else if name.matches("count") && count.is_none() {
                    count = value.as_java_int().filter(|value| (1..=99).contains(value));
                    count?;
                } else if name.matches("components") && components.is_none() {
                    let Tag::Compound(entries) = value else {
                        return None;
                    };
                    if entries.iter().any(|(name, _)| {
                        name.matches("minecraft:use_remainder")
                            || name.matches("!minecraft:use_remainder")
                    }) {
                        return None;
                    }
                    components = Some(value.clone());
                } else {
                    return None;
                }
            }
            (
                id?,
                count.unwrap_or(1),
                components.unwrap_or_else(|| Tag::Compound(vec![])),
            )
        }
        _ => return None,
    };
    let id = ResourceId::parse(&raw_id).ok()?;
    let id = format!("{}:{}", id.namespace, id.path);
    let max = saved_item_max(&id, &components)?;
    let count = u32::try_from(count).ok()?;
    if count > max {
        return None;
    }
    ItemStack::new(id, count, max, components)
}

fn food_remainder(item: &ItemStack) -> Option<ItemStack> {
    let components = item.components();
    if components.get("!minecraft:use_remainder").is_some() {
        return None;
    }
    if let Some(template) = components.get("minecraft:use_remainder") {
        return parse_use_remainder(template);
    }
    if item.id() == "minecraft:honey_bottle" {
        return ItemStack::new(
            "minecraft:glass_bottle".into(),
            1,
            64,
            Tag::Compound(vec![]),
        );
    }
    if item.id() == "minecraft:potion" {
        return ItemStack::new(
            "minecraft:glass_bottle".into(),
            1,
            64,
            Tag::Compound(vec![]),
        );
    }
    if item.id() == "minecraft:milk_bucket" {
        return ItemStack::new("minecraft:bucket".into(), 1, 16, Tag::Compound(vec![]));
    }
    matches!(
        item.id(),
        "minecraft:mushroom_stew" | "minecraft:rabbit_stew" | "minecraft:beetroot_soup"
    )
    .then(|| ItemStack::new("minecraft:bowl".into(), 1, 64, Tag::Compound(vec![])))
    .flatten()
}

pub fn food_properties(item: &ItemStack) -> Option<FoodProperties> {
    let components = item.components();
    if matches!(item.id(), "minecraft:potion" | "minecraft:milk_bucket") {
        if components.get("!minecraft:consumable").is_some()
            || components.get("minecraft:consumable").is_some()
            || components.get("minecraft:food").is_some()
        {
            return None;
        }
        if item.id() == "minecraft:potion" {
            potion_effects(components)?;
        }
        return Some(FoodProperties {
            nutrition: 0,
            saturation: 0.0,
            use_ticks: 32,
            can_always_eat: true,
            consume_particles: false,
            sound: FoodSound::PotionDrink,
        });
    }
    if components.get("!minecraft:food").is_some()
        || components.get("!minecraft:consumable").is_some()
    {
        return None;
    }
    let default = ordinary_food(item.id());
    let (nutrition, saturation, can_always_eat) = match components.get("minecraft:food") {
        Some(food) => custom_food(food)?,
        None => {
            let food = default?;
            (food.nutrition, food.saturation, food.can_always_eat)
        }
    };
    let (use_ticks, consume_particles) = match components.get("minecraft:consumable") {
        Some(consumable) => {
            let parsed = parse_consumable(consumable, false)?;
            (parsed.ticks, parsed.particles)
        }
        None => {
            let default = default?;
            (default.use_ticks, default.consume_particles)
        }
    };
    Some(FoodProperties {
        nutrition,
        saturation,
        use_ticks,
        can_always_eat,
        consume_particles,
        sound: if components.get("minecraft:consumable").is_some() {
            FoodSound::Eat
        } else {
            default?.sound
        },
    })
}

#[derive(Clone, Debug)]
struct Eating {
    hand: PlayerSlot,
    item: Arc<ItemStack>,
    remaining: i32,
    food: FoodProperties,
}

#[derive(Default)]
pub struct FoodUse {
    active: Option<Eating>,
    held: bool,
    repeat: bool,
    drops: Vec<ItemStack>,
    events: Vec<FoodRandomEvent>,
}

#[derive(Clone, Debug)]
pub(crate) enum FoodRandomEvent {
    ParticlePulse {
        count: u8,
        sound: FoodSound,
        item: Option<Arc<ItemStack>>,
    },
    Finish {
        causes_hunger: bool,
        particles: bool,
        sound: FoodSound,
        item: Option<Arc<ItemStack>>,
    },
    StatusGroup {
        probability: f32,
        effects: Vec<StatusApplication>,
    },
    PotionEffects(Vec<StatusApplication>),
    RemoveStatuses(Vec<FoodStatusKind>),
    ClearAllStatuses,
}

impl FoodRandomEvent {
    pub(crate) fn particle_item(&self) -> Option<&Arc<ItemStack>> {
        match self {
            Self::ParticlePulse { item, .. } | Self::Finish { item, .. } => item.as_ref(),
            _ => None,
        }
    }

    /// Advance the player's stream for the sound and particle calculations
    /// Java performs even when their visual output is not yet rendered.
    #[cfg(test)]
    pub(crate) fn apply_with_sounds(
        &self,
        random: &mut native_core::random::LegacyRandom,
    ) -> (bool, [Option<crate::ConsumeSoundInfo>; 3]) {
        self.apply_with_particles(random, &mut |_| {})
    }

    pub(crate) fn apply_with_particles(
        &self,
        random: &mut native_core::random::LegacyRandom,
        emit: &mut impl FnMut([f32; 4]),
    ) -> (bool, [Option<crate::ConsumeSoundInfo>; 3]) {
        let (particles, sound) = match self {
            Self::ParticlePulse { count, sound, .. } => (*count, *sound),
            Self::Finish {
                particles, sound, ..
            } => {
                if *particles {
                    (16, *sound)
                } else {
                    (0, *sound)
                }
            }
            Self::StatusGroup { probability, .. } => {
                return (random.next_float() < *probability, [None; 3]);
            }
            Self::RemoveStatuses(_) | Self::PotionEffects(_) | Self::ClearAllStatuses => {
                return (true, [None; 3]);
            }
        };
        let volume = if random.next_bool() { 0.5 } else { 1.0 };
        let eat_pitch = 1.0_f32 + 0.2_f32 * (random.next_float() - random.next_float());
        let drink_pitch = 0.9_f32 + random.next_float() * (1.0_f32 - 0.9_f32);
        for _ in 0..particles {
            emit([
                random.next_float(),
                random.next_float(),
                random.next_float(),
                random.next_float(),
            ]);
        }
        let (event, volume, pitch) = match sound {
            FoodSound::Eat => ("entity.generic.eat", volume, eat_pitch),
            FoodSound::HoneyDrink => ("item.honey_bottle.drink", 0.5, drink_pitch),
            FoodSound::PotionDrink => ("entity.generic.drink", 0.5, drink_pitch),
        };
        let mut sounds = [
            Some(crate::ConsumeSoundInfo {
                event,
                volume,
                pitch,
                category: crate::ConsumeSoundCategory::Player,
            }),
            None,
            None,
        ];
        if let Self::Finish { causes_hunger, .. } = self {
            if sound == FoodSound::PotionDrink {
                return (false, sounds);
            }
            let pitch = 1.0_f32 + 0.4_f32 * (random.next_float() - random.next_float());
            let burp_pitch = 0.9_f32 + random.next_float() * (1.0_f32 - 0.9_f32);
            sounds[1] = Some(crate::ConsumeSoundInfo {
                event,
                volume: 1.0,
                pitch,
                category: crate::ConsumeSoundCategory::Neutral,
            });
            sounds[2] = Some(crate::ConsumeSoundInfo {
                event: "entity.player.burp",
                volume: 0.5,
                pitch: burp_pitch,
                category: crate::ConsumeSoundCategory::Player,
            });
            return (*causes_hunger && random.next_float() < 0.8, sounds);
        }
        (false, sounds)
    }

    #[cfg(test)]
    pub(crate) fn apply(&self, random: &mut native_core::random::LegacyRandom) -> bool {
        self.apply_with_sounds(random).0
    }
}

impl FoodUse {
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    pub(crate) fn take_drops(&mut self) -> Vec<ItemStack> {
        std::mem::take(&mut self.drops)
    }

    pub(crate) fn take_events(&mut self) -> Vec<FoodRandomEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn visual_state(&self) -> Option<crate::animation::ItemUseState> {
        self.active
            .as_ref()
            .map(|active| crate::animation::ItemUseState {
                hand: if active.hand == PlayerSlot::Offhand {
                    crate::animation::Hand::Off
                } else {
                    crate::animation::Hand::Main
                },
                remaining_ticks: active.remaining,
            })
    }

    pub fn set_held(&mut self, held: bool) {
        self.held = held;
        if !held {
            self.cancel();
        }
    }
    /// Returns true if the hand contains a supported food, even when it cannot
    /// be eaten at full hunger; vanilla then stops looking at the other hand.
    pub fn start_hand(
        &mut self,
        inventory: &mut Arc<PlayerInventory>,
        hand: PlayerSlot,
        vitals: &mut PlayerVitals,
        mode: crate::GameMode,
    ) -> bool {
        let Some(item) = inventory.slot(hand) else {
            return false;
        };
        let Some(food) = food_properties(item) else {
            return false;
        };
        if saved_item_max(item.id(), item.components()) != item.max_count()
            || item.max_count().is_none()
        {
            return false;
        }
        if mode != crate::GameMode::Spectator
            && (vitals.food < 20 || food.can_always_eat || mode == crate::GameMode::Creative)
        {
            if food.use_ticks == 0 {
                self.finish(inventory, vitals, mode, hand, food);
            } else {
                self.active = Some(Eating {
                    hand,
                    item: Arc::new(item.clone()),
                    remaining: food.use_ticks,
                    food,
                });
            }
        }
        true
    }

    pub fn cancel(&mut self) {
        self.active = None;
        self.held = false;
        self.repeat = false;
    }

    pub fn prepare(
        &mut self,
        inventory: &mut Arc<PlayerInventory>,
        vitals: &mut PlayerVitals,
        mode: crate::GameMode,
    ) {
        if self.repeat {
            self.repeat = false;
            if self.held && !inventory.is_open() {
                let main = PlayerSlot::Storage(inventory.selected());
                if !self.start_hand(inventory, main, vitals, mode) {
                    self.start_hand(inventory, PlayerSlot::Offhand, vitals, mode);
                }
            }
        }
        if self.active.as_ref().is_some_and(|active| {
            let hand_is_current = match active.hand {
                PlayerSlot::Storage(index) => index == inventory.selected(),
                PlayerSlot::Offhand => true,
                _ => false,
            };
            mode == crate::GameMode::Spectator
                || inventory.is_open()
                || !hand_is_current
                || !inventory
                    .slot(active.hand)
                    .is_some_and(|item| item.same_item_and_components(&active.item))
        }) {
            self.active = None;
        }
        if let Some(active) = &mut self.active
            && let Some(held) = inventory.slot(active.hand)
            && active.item.as_ref() != held
        {
            active.item = Arc::new(held.clone());
        }
    }

    pub fn tick(
        &mut self,
        inventory: &mut Arc<PlayerInventory>,
        vitals: &mut PlayerVitals,
        mode: crate::GameMode,
    ) {
        self.prepare(inventory, vitals, mode);
        let Some(active) = &mut self.active else {
            return;
        };
        let elapsed = active.food.use_ticks - active.remaining;
        let threshold = (active.food.use_ticks as f32 * 0.21875_f32) as i32;
        if elapsed > threshold && active.remaining % 4 == 0 {
            self.events.push(FoodRandomEvent::ParticlePulse {
                count: if active.food.consume_particles { 5 } else { 0 },
                sound: active.food.sound,
                item: active
                    .food
                    .consume_particles
                    .then(|| Arc::clone(&active.item)),
            });
        }
        active.remaining = active.remaining.saturating_sub(1);
        if active.remaining != 0 {
            return;
        }
        let active = self.active.take().expect("active use exists");
        self.finish(inventory, vitals, mode, active.hand, active.food);
        self.repeat = self.held;
    }

    fn finish(
        &mut self,
        inventory: &mut Arc<PlayerInventory>,
        vitals: &mut PlayerVitals,
        mode: crate::GameMode,
        hand: PlayerSlot,
        food: FoodProperties,
    ) {
        let remainder = inventory.slot(hand).and_then(food_remainder);
        let potion = inventory
            .slot(hand)
            .filter(|item| item.id() == "minecraft:potion")
            .and_then(|item| potion_effects(item.components()));
        let particle_item = food
            .consume_particles
            .then(|| inventory.slot(hand).cloned().map(Arc::new))
            .flatten();
        let remove_poison = inventory.slot(hand).is_some_and(|item| {
            item.id() == "minecraft:honey_bottle"
                && item.components().get("minecraft:consumable").is_none()
        });
        let clear_all = inventory.slot(hand).is_some_and(|item| {
            item.id() == "minecraft:milk_bucket"
                && item.components().get("minecraft:consumable").is_none()
        });
        let causes_hunger = inventory.slot(hand).is_some_and(|item| {
            item.id() == "minecraft:rotten_flesh"
                && item.components().get("minecraft:consumable").is_none()
        });
        let mut custom_groups = inventory
            .slot(hand)
            .and_then(|item| item.components().get("minecraft:consumable"))
            .and_then(|component| parse_consumable(component, true))
            .map(|value| value.effects)
            .unwrap_or_default();
        if let Some(item) = inventory.slot(hand)
            && item.components().get("minecraft:consumable").is_none()
            && let Some((kind, duration, probability)) = match item.id() {
                "minecraft:chicken" => Some((FoodStatusKind::Hunger, 600, 0.3)),
                "minecraft:poisonous_potato" => Some((FoodStatusKind::Poison, 100, 0.6)),
                "minecraft:spider_eye" => Some((FoodStatusKind::Poison, 100, 1.0)),
                _ => None,
            }
        {
            custom_groups.push(ConsumeStatusAction::Apply(StatusGroup {
                probability,
                effects: vec![StatusApplication {
                    kind,
                    duration,
                    amplifier: 0,
                    ambient: false,
                    visible: true,
                    show_icon: true,
                }],
            }));
        }
        if mode == crate::GameMode::Creative || Arc::make_mut(inventory).consume_held(hand) {
            self.events.push(FoodRandomEvent::Finish {
                causes_hunger,
                particles: food.consume_particles,
                sound: food.sound,
                item: particle_item,
            });
            if let Some(effects) = potion {
                self.events.push(FoodRandomEvent::PotionEffects(effects));
            }
            self.events
                .extend(custom_groups.into_iter().map(|action| match action {
                    ConsumeStatusAction::Apply(group) => FoodRandomEvent::StatusGroup {
                        probability: group.probability,
                        effects: group.effects,
                    },
                    ConsumeStatusAction::Remove(effects) => {
                        FoodRandomEvent::RemoveStatuses(effects)
                    }
                    ConsumeStatusAction::ClearAll => FoodRandomEvent::ClearAllStatuses,
                }));
            if clear_all {
                self.events.push(FoodRandomEvent::ClearAllStatuses);
            }
            if remove_poison {
                self.events.push(FoodRandomEvent::RemoveStatuses(vec![
                    FoodStatusKind::Poison,
                ]));
            }
            if food.sound != FoodSound::PotionDrink {
                vitals.food = vitals.food.wrapping_add(food.nutrition).clamp(0, 20);
                vitals.saturation =
                    (vitals.saturation + food.saturation).clamp(0.0, vitals.food as f32);
            }
            if mode != crate::GameMode::Creative
                && let Some(remainder) = remainder
            {
                let inventory = Arc::make_mut(inventory);
                if inventory.slot(hand).is_none() {
                    inventory.set_slot(hand, Some(remainder));
                } else {
                    let mut rest = Some(remainder);
                    inventory.pickup_stack(&mut rest, false);
                    if let Some(rest) = rest {
                        self.drops.push(rest);
                    }
                }
            }
        }
    }
}

/// Damage requires the wider entity damage pipeline; the food tick only
/// determines when starvation should be requested.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FoodEffect {
    None,
    Heal(f32),
    Starve,
}

#[derive(Clone, Copy, Debug)]
pub struct SurvivalRules {
    pub difficulty: Difficulty,
    pub natural_health_regeneration: bool,
    pub fall_damage: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MovementEnvironment {
    pub on_ground: bool,
    pub sprinting: bool,
    pub swimming: bool,
    pub eyes_in_water: bool,
    pub in_water: bool,
}

impl Default for SurvivalRules {
    fn default() -> Self {
        Self {
            difficulty: Difficulty::Normal,
            natural_health_regeneration: true,
            fall_damage: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerVitals {
    pub health: f32,
    pub max_health: f32,
    pub absorption: f32,
    /// Java's air supply, in ticks. The default player maximum is 300.
    pub air: i32,
    pub food: i32,
    pub saturation: f32,
    pub exhaustion: f32,
    pub food_tick_timer: i32,
    pub hardcore: bool,
}

/// A 26.3 status effect and its hidden, weaker replacement chain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectDuration {
    pub amplifier: u8,
    pub duration: i32,
    pub ambient: bool,
    pub visible: bool,
    pub show_icon: bool,
    pub hidden: Option<Box<EffectDuration>>,
}

impl EffectDuration {
    pub(crate) fn from_application(effect: StatusApplication) -> Self {
        Self {
            amplifier: effect.amplifier,
            duration: effect.duration,
            ambient: effect.ambient,
            visible: effect.visible,
            show_icon: effect.show_icon,
            hidden: None,
        }
    }
    fn shorter_than(&self, other: &Self) -> bool {
        self.duration != -1 && (other.duration == -1 || self.duration < other.duration)
    }

    /// Merge the timing and amplifier of a matching Java effect instance.
    pub(crate) fn merge(&mut self, incoming: Self) -> bool {
        let mut changed = false;
        let (ambient, visible, show_icon) =
            (incoming.ambient, incoming.visible, incoming.show_icon);
        if incoming.amplifier > self.amplifier {
            if incoming.shorter_than(self) {
                let old = self.clone();
                self.hidden = Some(Box::new(old));
            }
            self.amplifier = incoming.amplifier;
            self.duration = incoming.duration;
            changed = true;
        } else if self.shorter_than(&incoming) {
            if incoming.amplifier == self.amplifier {
                self.duration = incoming.duration;
                changed = true;
            } else if let Some(hidden) = &mut self.hidden {
                hidden.merge(incoming);
            } else {
                self.hidden = Some(Box::new(incoming));
            }
        }
        if (!ambient && self.ambient) || changed {
            self.ambient = ambient;
            changed = true;
        }
        if self.visible != visible {
            self.visible = visible;
            changed = true;
        }
        if self.show_icon != show_icon {
            self.show_icon = show_icon;
            changed = true;
        }
        changed
    }

    fn has_time(&self) -> bool {
        self.duration == -1 || self.duration > 0
    }

    fn decrement(&mut self) {
        if let Some(hidden) = &mut self.hidden {
            hidden.decrement();
        }
        if self.duration != -1 && self.duration != 0 {
            self.duration = self.duration.wrapping_sub(1);
        }
    }

    fn tick(slot: &mut Option<Self>) {
        let Some(effect) = slot else { return };
        if !effect.has_time() {
            *slot = None;
            return;
        }
        effect.decrement();
        if effect.duration == 0
            && let Some(hidden) = effect.hidden.take()
        {
            *effect = *hidden;
        }
        if !effect.has_time() {
            *slot = None;
        }
    }
}

/// Active effects used by movement, survival and fall damage. The hidden chain
/// ages with the visible effect, as it does for other living-entity effects.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FallEffects {
    pub night_vision: Option<EffectDuration>,
    pub jump_boost: Option<EffectDuration>,
    pub resistance: Option<EffectDuration>,
    pub slow_falling: Option<EffectDuration>,
    pub levitation: Option<EffectDuration>,
    pub speed: Option<EffectDuration>,
    pub slowness: Option<EffectDuration>,
    pub dolphins_grace: Option<EffectDuration>,
    pub haste: Option<EffectDuration>,
    pub mining_fatigue: Option<EffectDuration>,
    pub hunger: Option<EffectDuration>,
    pub poison: Option<EffectDuration>,
    pub regeneration: Option<EffectDuration>,
    pub luck: Option<EffectDuration>,
    pub unluck: Option<EffectDuration>,
}

impl FallEffects {
    /// Java uses the entity tick count for an infinite effect and its remaining
    /// duration otherwise. Java masks the shift distance to five bits.
    pub(crate) fn poison_due(&self, entity_tick: i32) -> bool {
        self.poison.as_ref().is_some_and(|effect| {
            if !effect.has_time() {
                return false;
            }
            let interval = 25_i32 >> (u32::from(effect.amplifier) & 31);
            let elapsed = if effect.duration == -1 {
                entity_tick
            } else {
                effect.duration
            };
            interval <= 0 || elapsed % interval == 0
        })
    }

    pub(crate) fn regeneration_due(&self, entity_tick: i32) -> bool {
        self.regeneration.as_ref().is_some_and(|effect| {
            if !effect.has_time() {
                return false;
            }
            let interval = 50_i32 >> (u32::from(effect.amplifier) & 31);
            let elapsed = if effect.duration == -1 {
                entity_tick
            } else {
                effect.duration
            };
            interval <= 0 || elapsed % interval == 0
        })
    }

    pub fn apply_hunger(&self, vitals: &mut PlayerVitals, mode: crate::GameMode) {
        if let Some(effect) = &self.hunger
            && effect.has_time()
            && vitals.health > 0.0
            && !matches!(mode, crate::GameMode::Creative | crate::GameMode::Spectator)
        {
            vitals.add_exhaustion(0.005_f32 * (f32::from(effect.amplifier) + 1.0));
        }
    }

    pub fn tick(&mut self) {
        EffectDuration::tick(&mut self.night_vision);
        EffectDuration::tick(&mut self.jump_boost);
        EffectDuration::tick(&mut self.resistance);
        EffectDuration::tick(&mut self.slow_falling);
        EffectDuration::tick(&mut self.levitation);
        EffectDuration::tick(&mut self.speed);
        EffectDuration::tick(&mut self.slowness);
        EffectDuration::tick(&mut self.dolphins_grace);
        EffectDuration::tick(&mut self.haste);
        EffectDuration::tick(&mut self.mining_fatigue);
        EffectDuration::tick(&mut self.hunger);
        EffectDuration::tick(&mut self.poison);
        EffectDuration::tick(&mut self.regeneration);
        EffectDuration::tick(&mut self.luck);
        EffectDuration::tick(&mut self.unluck);
    }
}

/// Effects that change ordinary player breathing. Their durations advance at
/// the end of the entity's base tick, after the breathing decision.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BreathingEffects {
    pub water_breathing: Option<EffectDuration>,
    pub conduit_power: Option<EffectDuration>,
    pub breath_of_the_nautilus: Option<EffectDuration>,
}

/// The visible instance, excluding any hidden replacement chain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DisplayEffect {
    pub id: &'static str,
    pub duration: i32,
    pub amplifier: u8,
    pub ambient: bool,
    pub show_icon: bool,
}

pub(crate) fn display_effects(
    fall: &FallEffects,
    breathing: &BreathingEffects,
) -> std::sync::Arc<[DisplayEffect]> {
    [
        ("night_vision", &fall.night_vision),
        ("jump_boost", &fall.jump_boost),
        ("resistance", &fall.resistance),
        ("slow_falling", &fall.slow_falling),
        ("levitation", &fall.levitation),
        ("speed", &fall.speed),
        ("slowness", &fall.slowness),
        ("dolphins_grace", &fall.dolphins_grace),
        ("haste", &fall.haste),
        ("mining_fatigue", &fall.mining_fatigue),
        ("hunger", &fall.hunger),
        ("poison", &fall.poison),
        ("regeneration", &fall.regeneration),
        ("luck", &fall.luck),
        ("unluck", &fall.unluck),
        ("water_breathing", &breathing.water_breathing),
        ("conduit_power", &breathing.conduit_power),
        ("breath_of_the_nautilus", &breathing.breath_of_the_nautilus),
    ]
    .into_iter()
    .filter_map(|(id, effect)| {
        effect.as_ref().map(|effect| DisplayEffect {
            id,
            duration: effect.duration,
            amplifier: effect.amplifier,
            ambient: effect.ambient,
            show_icon: effect.show_icon,
        })
    })
    .collect()
}

impl BreathingEffects {
    pub(crate) fn add_water_breathing(&mut self, application: StatusApplication) {
        let incoming = EffectDuration::from_application(application);
        if let Some(effect) = &mut self.water_breathing {
            effect.merge(incoming);
        } else {
            self.water_breathing = Some(incoming);
        }
    }
    pub fn tick(&mut self) {
        EffectDuration::tick(&mut self.water_breathing);
        EffectDuration::tick(&mut self.conduit_power);
        EffectDuration::tick(&mut self.breath_of_the_nautilus);
    }

    fn protects(&self) -> bool {
        self.water_breathing.is_some()
            || self.conduit_power.is_some()
            || self.breath_of_the_nautilus.is_some()
    }

    fn refills(&self) -> bool {
        self.breath_of_the_nautilus.is_none()
            || self.water_breathing.is_some()
            || self.conduit_power.is_some()
    }
}

impl Default for PlayerVitals {
    fn default() -> Self {
        Self {
            health: 20.0,
            max_health: 20.0,
            absorption: 0.0,
            air: 300,
            food: 20,
            saturation: 5.0,
            exhaustion: 0.0,
            food_tick_timer: 0,
            hardcore: false,
        }
    }
}

impl PlayerVitals {
    /// Tick ordinary player breathing without effects or oxygen bonuses.
    pub fn tick_air(&mut self, eyes_in_water: bool, in_bubble_column: bool, mode: crate::GameMode) {
        self.tick_air_with(
            eyes_in_water,
            in_bubble_column,
            mode,
            &BreathingEffects::default(),
            0.0,
            &mut || 0.0,
        );
    }

    /// Apply the 26.3 breathing rules. The caller owns the player's random
    /// stream and advances effect durations after this method returns.
    pub fn tick_air_with(
        &mut self,
        eyes_in_water: bool,
        in_bubble_column: bool,
        mode: crate::GameMode,
        effects: &BreathingEffects,
        oxygen_bonus: f64,
        random: &mut impl FnMut() -> f64,
    ) {
        if self.advance_air(
            eyes_in_water,
            in_bubble_column,
            mode,
            effects,
            oxygen_bonus,
            random,
        ) {
            let absorbed = self.absorption.clamp(0.0, 2.0);
            self.absorption -= absorbed;
            self.health = (self.health - (2.0 - absorbed)).max(0.0);
        }
    }

    /// Advance air and report a drowning hit for the shared damage pipeline.
    pub(crate) fn advance_air(
        &mut self,
        eyes_in_water: bool,
        in_bubble_column: bool,
        mode: crate::GameMode,
        effects: &BreathingEffects,
        oxygen_bonus: f64,
        random: &mut impl FnMut() -> f64,
    ) -> bool {
        if self.health <= 0.0 {
            return false;
        }
        if eyes_in_water && !in_bubble_column {
            let protected = effects.protects()
                || matches!(mode, crate::GameMode::Creative | crate::GameMode::Spectator);
            if protected {
                if self.air < 300 && effects.refills() {
                    self.air = self.air.wrapping_add(4).min(300);
                }
            } else {
                if oxygen_bonus <= 0.0 || random() < 1.0 / (oxygen_bonus + 1.0) {
                    self.air = self.air.wrapping_sub(1);
                }
                if self.air <= -20 {
                    self.air = 0;
                    return true;
                }
            }
        } else if self.air < 300 {
            self.air = self.air.wrapping_add(4).min(300);
        }
        false
    }

    pub fn can_sprint(self, mode: crate::GameMode) -> bool {
        self.food > 6 || matches!(mode, crate::GameMode::Creative | crate::GameMode::Spectator)
    }

    pub fn add_exhaustion(&mut self, amount: f32) {
        self.exhaustion = (self.exhaustion + amount).min(40.0);
    }

    /// Return the effect the server would ask its entity damage/heal system to
    /// apply. This deliberately does not pretend to implement that system.
    pub fn tick_food(&mut self, rules: SurvivalRules) -> FoodEffect {
        if self.exhaustion > 4.0 {
            self.exhaustion -= 4.0;
            if self.saturation > 0.0 {
                self.saturation = (self.saturation - 1.0).max(0.0);
            } else if rules.difficulty != Difficulty::Peaceful {
                self.food = self.food.wrapping_sub(1).max(0);
            }
        }

        let hurt = self.health > 0.0 && self.health < self.max_health;
        if rules.natural_health_regeneration && self.saturation > 0.0 && hurt && self.food >= 20 {
            self.food_tick_timer = self.food_tick_timer.wrapping_add(1);
            if self.food_tick_timer >= 10 {
                let amount = self.saturation.min(6.0);
                self.add_exhaustion(amount);
                self.food_tick_timer = 0;
                return FoodEffect::Heal(amount / 6.0);
            }
        } else if rules.natural_health_regeneration && self.food >= 18 && hurt {
            self.food_tick_timer = self.food_tick_timer.wrapping_add(1);
            if self.food_tick_timer >= 80 {
                self.add_exhaustion(6.0);
                self.food_tick_timer = 0;
                return FoodEffect::Heal(1.0);
            }
        } else if self.food <= 0 {
            self.food_tick_timer = self.food_tick_timer.wrapping_add(1);
            if self.food_tick_timer >= 80 {
                self.food_tick_timer = 0;
                if self.health > 10.0
                    || rules.difficulty == Difficulty::Hard
                    || (self.health > 1.0 && rules.difficulty == Difficulty::Normal)
                {
                    return FoodEffect::Starve;
                }
            }
        } else {
            self.food_tick_timer = 0;
        }
        FoodEffect::None
    }

    /// Peaceful regeneration is a separate Player.aiStep operation and runs
    /// before the food-data tick. Tick count starts again when a player loads.
    pub fn tick_peaceful_regeneration(&mut self, rules: SurvivalRules, tick_count: i32) {
        if rules.difficulty != Difficulty::Peaceful || !rules.natural_health_regeneration {
            return;
        }
        if tick_count % 20 == 0 {
            if self.health > 0.0 && self.health < self.max_health {
                self.health = (self.health + 1.0).min(self.max_health);
            }
            if self.saturation < 20.0 {
                self.saturation += 1.0;
            }
        }
        if tick_count % 10 == 0 && self.food < 20 {
            self.food += 1;
        }
    }

    pub fn heal(&mut self, amount: f32) {
        if self.health > 0.0 {
            self.health = (self.health + amount).clamp(0.0, self.max_health);
        }
    }

    /// Default movement exhaustion, for the movement states represented by the
    /// current physics engine. Vehicles, effects and custom abilities are not
    /// yet represented.
    pub fn record_movement(
        &mut self,
        mode: crate::GameMode,
        delta: glam::DVec3,
        environment: MovementEnvironment,
    ) {
        if matches!(mode, crate::GameMode::Creative | crate::GameMode::Spectator) {
            return;
        }
        let (distance, factor) = if environment.swimming || environment.eyes_in_water {
            (
                (delta.x * delta.x + delta.y * delta.y + delta.z * delta.z).sqrt(),
                0.01_f32,
            )
        } else if environment.in_water {
            ((delta.x * delta.x + delta.z * delta.z).sqrt(), 0.01_f32)
        } else if environment.on_ground && environment.sprinting {
            ((delta.x * delta.x + delta.z * delta.z).sqrt(), 0.1_f32)
        } else {
            return;
        };
        // Math.round(float) is floor(x + 0.5), with a saturating int cast.
        let centimeters = ((distance as f32 * 100.0) + 0.5).floor() as i32;
        if centimeters > 0 {
            self.add_exhaustion(factor * centimeters as f32 * 0.01_f32);
        }
    }

    pub fn record_jump(&mut self, mode: crate::GameMode, sprinting: bool) {
        if !matches!(mode, crate::GameMode::Creative | crate::GameMode::Spectator) {
            self.add_exhaustion(if sprinting { 0.2 } else { 0.05 });
        }
    }

    /// Apply effects supported by the current simple player model. Full damage
    /// immunity, effects, death and respawn are separate work.
    pub fn apply_food_effect(&mut self, effect: FoodEffect, mode: crate::GameMode) {
        match effect {
            FoodEffect::None => {}
            FoodEffect::Heal(amount) => self.heal(amount),
            FoodEffect::Starve
                if self.health > 0.0
                    && !matches!(mode, crate::GameMode::Creative | crate::GameMode::Spectator) =>
            {
                let absorbed = self.absorption.clamp(0.0, 1.0);
                self.absorption -= absorbed;
                self.health = (self.health - (1.0 - absorbed)).max(0.0);
            }
            FoodEffect::Starve => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GameMode, GameSession, LocalSession, PlayerCommand};

    #[test]
    fn interrupted_milk_use_does_not_consume_or_clear_effects() {
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(
            PlayerSlot::Storage(0),
            ItemStack::new("minecraft:milk_bucket".into(), 1, 1, Tag::Compound(vec![])),
        );
        let mut inventory = Arc::new(inventory);
        let mut vitals = PlayerVitals::default();
        let mut usage = FoodUse::default();
        assert!(usage.start_hand(
            &mut inventory,
            PlayerSlot::Storage(0),
            &mut vitals,
            GameMode::Survival
        ));
        for _ in 0..31 {
            usage.tick(&mut inventory, &mut vitals, GameMode::Survival);
        }
        usage.cancel();
        usage.tick(&mut inventory, &mut vitals, GameMode::Survival);
        assert_eq!(
            inventory.slot(PlayerSlot::Storage(0)).unwrap().id(),
            "minecraft:milk_bucket"
        );
        assert!(
            !usage
                .take_events()
                .iter()
                .any(|event| matches!(event, FoodRandomEvent::ClearAllStatuses))
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn milk_use_matches_java_sounds_timing_and_random() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("MilkOracle.java");
        let base = temp.path().join("FoodEffectsOracle.java");
        let output = temp.path().join("milk.json");
        std::fs::write(&source, include_str!("../tests/java/MilkOracle.java")).unwrap();
        std::fs::write(&base, include_str!("../tests/java/FoodEffectsOracle.java")).unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg(&source)
                .arg(&base),
            Command::new(java.join("java.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg("MilkOracle")
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), 32);
        for row in rows {
            assert_eq!(row["ticks"], 32);
            assert_eq!(row["particles"], false);
            assert_eq!(row["animation"], "DRINK");
            assert_eq!(row["food"], false);
            assert_eq!(row["can_consume"], true);
            assert_eq!(row["cleared"], true);
            for hand in [PlayerSlot::Storage(0), PlayerSlot::Offhand] {
                for mode in [GameMode::Survival, GameMode::Creative] {
                    let item =
                        ItemStack::new("minecraft:milk_bucket".into(), 1, 1, Tag::Compound(vec![]))
                            .unwrap();
                    let food = food_properties(&item).unwrap();
                    let mut inventory = PlayerInventory::default();
                    inventory.set_slot(hand, Some(item));
                    let mut inventory = Arc::new(inventory);
                    let mut vitals = PlayerVitals {
                        food: 20,
                        saturation: 5.0,
                        ..Default::default()
                    };
                    let mut usage = FoodUse::default();
                    assert!(usage.start_hand(&mut inventory, hand, &mut vitals, mode));
                    let mut random =
                        native_core::random::LegacyRandom::new(row["seed"].as_i64().unwrap());
                    let mut sounds = Vec::new();
                    let mut cleared = 0;
                    for tick in 0..food.use_ticks {
                        usage.tick(&mut inventory, &mut vitals, mode);
                        for event in usage.take_events() {
                            let (applied, emitted) = event.apply_with_sounds(&mut random);
                            sounds.extend(emitted.into_iter().flatten());
                            if matches!(event, FoodRandomEvent::ClearAllStatuses) {
                                assert!(applied);
                                assert_eq!(tick, 31);
                                cleared += 1;
                            }
                        }
                    }
                    assert_eq!(cleared, 1);
                    assert_eq!(
                        inventory.slot(hand).unwrap().id(),
                        if mode == GameMode::Creative {
                            "minecraft:milk_bucket"
                        } else {
                            "minecraft:bucket"
                        }
                    );
                    assert_eq!((vitals.food, vitals.saturation), (20, 5.0));
                    let expected = row["sounds"].as_array().unwrap();
                    assert_eq!(sounds.len(), expected.len());
                    for (sound, expected) in sounds.iter().zip(expected) {
                        assert_eq!(
                            format!("minecraft:{}", sound.event),
                            expected["event"].as_str().unwrap()
                        );
                        assert_eq!(
                            sound.volume.to_bits(),
                            expected["volume_bits"].as_i64().unwrap() as i32 as u32
                        );
                        assert_eq!(
                            sound.pitch.to_bits(),
                            expected["pitch_bits"].as_i64().unwrap() as i32 as u32
                        );
                    }
                    assert_eq!(
                        random.next_float().to_bits(),
                        row["next_bits"].as_i64().unwrap() as i32 as u32
                    );
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn supported_potions_match_java_consumption_and_random_stream() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        native_resources::packs::ResourceStack::open(&game).unwrap();
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("PotionOracle.java");
        let food_source = temp.path().join("FoodEffectsOracle.java");
        let output = temp.path().join("potions.json");
        std::fs::write(&source, include_str!("../tests/java/PotionOracle.java")).unwrap();
        std::fs::write(
            &food_source,
            include_str!("../tests/java/FoodEffectsOracle.java"),
        )
        .unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg(&source)
                .arg(&food_source),
            Command::new(java.join("java.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg("PotionOracle")
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
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), 500);
        for row in rows {
            for variant in 0..4 {
                let name = row["potion"].as_str().unwrap();
                let components = Tag::Compound(vec![(
                    "minecraft:potion_contents".into(),
                    match variant {
                        0 => Tag::Compound(vec![("potion".into(), Tag::String(name.into()))]),
                        1 => Tag::String(name.into()),
                        2 => Tag::Compound(vec![
                            ("potion".into(), Tag::String(name.into())),
                            ("custom_color".into(), Tag::Int(0x123456)),
                            ("custom_name".into(), Tag::String("test.potion".into())),
                        ]),
                        _ => Tag::Compound(vec![
                            ("potion".into(), Tag::String(name.into())),
                            ("custom_color".into(), Tag::Int(0x123456)),
                            ("custom_name".into(), Tag::String("test.potion".into())),
                            (
                                "custom_effects".into(),
                                Tag::List {
                                    element_type: native_persistence::nbt::TagType::End,
                                    values: vec![],
                                },
                            ),
                        ]),
                    },
                )]);
                let item = ItemStack::new("minecraft:potion".into(), 1, 1, components).unwrap();
                assert_eq!(
                    potion_color(item.components()).unwrap_or(0x385dc6),
                    if variant >= 2 {
                        0x123456
                    } else {
                        row["color"].as_i64().unwrap() as u32 & 0xffffff
                    },
                    "{row}"
                );
                let food = food_properties(&item).unwrap();
                assert_eq!(
                    food.use_ticks,
                    row["ticks"].as_i64().unwrap() as i32,
                    "{row}"
                );
                assert_eq!(
                    food.consume_particles,
                    row["particles"].as_bool().unwrap(),
                    "{row}"
                );
                assert_eq!(row["animation"].as_str(), Some("DRINK"));
                let mut inventory = PlayerInventory::default();
                inventory.set_slot(PlayerSlot::Storage(0), Some(item));
                let mut inventory = Arc::new(inventory);
                let mut vitals = PlayerVitals {
                    food: 20,
                    saturation: 5.0,
                    ..Default::default()
                };
                let mut usage = FoodUse::default();
                assert_eq!(
                    usage.start_hand(
                        &mut inventory,
                        PlayerSlot::Storage(0),
                        &mut vitals,
                        GameMode::Survival
                    ),
                    row["can_consume"].as_bool().unwrap()
                );
                let mut random =
                    native_core::random::LegacyRandom::new(row["seed"].as_i64().unwrap());
                let mut sounds = Vec::new();
                let mut effects = Vec::new();
                for _ in 0..food.use_ticks {
                    usage.tick(&mut inventory, &mut vitals, GameMode::Survival);
                    for event in usage.take_events() {
                        let (applied, emitted) = event.apply_with_sounds(&mut random);
                        sounds.extend(emitted.into_iter().flatten());
                        if let FoodRandomEvent::PotionEffects(values) = event {
                            assert!(applied);
                            effects.extend(values);
                        }
                    }
                }
                assert_eq!(
                    inventory.slot(PlayerSlot::Storage(0)).unwrap().id(),
                    row["remainder"].as_str().unwrap()
                );
                assert_eq!((vitals.food, vitals.saturation), (20, 5.0));
                let expected = row["effects"].as_array().unwrap();
                assert_eq!(effects.len(), expected.len(), "{row}");
                for (actual, expected) in effects.iter().zip(expected) {
                    let name = match actual.kind {
                        FoodStatusKind::NightVision => "minecraft:night_vision",
                        FoodStatusKind::WaterBreathing => "minecraft:water_breathing",
                        FoodStatusKind::Poison => "minecraft:poison",
                        FoodStatusKind::Regeneration => "minecraft:regeneration",
                        FoodStatusKind::JumpBoost => "minecraft:jump_boost",
                        FoodStatusKind::SlowFalling => "minecraft:slow_falling",
                        FoodStatusKind::Speed => "minecraft:speed",
                        FoodStatusKind::Slowness => "minecraft:slowness",
                        FoodStatusKind::Hunger => "minecraft:hunger",
                    };
                    assert_eq!(name, expected["id"].as_str().unwrap());
                    assert_eq!(
                        i64::from(actual.duration),
                        expected["duration"].as_i64().unwrap()
                    );
                    assert_eq!(
                        u64::from(actual.amplifier),
                        expected["amplifier"].as_u64().unwrap()
                    );
                }
                let expected = row["sounds"].as_array().unwrap();
                assert_eq!(sounds.len(), expected.len(), "{row}");
                for (sound, expected) in sounds.iter().zip(expected) {
                    assert_eq!(
                        format!("minecraft:{}", sound.event),
                        expected["event"].as_str().unwrap()
                    );
                    assert_eq!(
                        sound.volume.to_bits(),
                        expected["volume_bits"].as_i64().unwrap() as i32 as u32
                    );
                    assert_eq!(
                        sound.pitch.to_bits(),
                        expected["pitch_bits"].as_i64().unwrap() as i32 as u32
                    );
                }
                assert_eq!(
                    random.next_float().to_bits(),
                    row["next_bits"].as_i64().unwrap() as i32 as u32,
                    "{row}"
                );
            }
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn food_effects_match_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("FoodEffectsOracle.java");
        let output = temp.path().join("food.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/FoodEffectsOracle.java"),
        )
        .unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg("FoodEffectsOracle")
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "Java poison food oracle failed: {}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), 400);
        for row in rows {
            let id = format!("minecraft:{}", row["item"].as_str().unwrap());
            let max = row["max_stack"].as_u64().unwrap() as u32;
            let item = ItemStack::new(id, 1, max, Tag::Compound(vec![])).unwrap();
            let food = food_properties(&item).unwrap();
            assert_eq!(
                food.use_ticks,
                row["ticks"].as_i64().unwrap() as i32,
                "{row}"
            );
            assert_eq!(
                food.can_always_eat,
                row["always"].as_bool().unwrap(),
                "{row}"
            );
            assert_eq!(
                food.consume_particles,
                row["particles"].as_bool().unwrap(),
                "{row}"
            );
            let mut inventory = PlayerInventory::default();
            inventory.set_slot(PlayerSlot::Storage(0), Some(item));
            let mut inventory = Arc::new(inventory);
            let mut vitals = PlayerVitals {
                food: 10,
                saturation: 0.0,
                ..Default::default()
            };
            let mut usage = FoodUse::default();
            let mut random = native_core::random::LegacyRandom::new(row["seed"].as_i64().unwrap());
            assert!(usage.start_hand(
                &mut inventory,
                PlayerSlot::Storage(0),
                &mut vitals,
                GameMode::Survival
            ));
            let mut applied = None;
            let mut removed = false;
            let mut sounds = Vec::new();
            for _ in 0..food.use_ticks {
                usage.tick(&mut inventory, &mut vitals, GameMode::Survival);
                for event in usage.take_events() {
                    let (applies, emitted) = event.apply_with_sounds(&mut random);
                    sounds.extend(emitted.into_iter().flatten());
                    if applies {
                        match event {
                            FoodRandomEvent::StatusGroup { effects, .. } => {
                                assert_eq!(effects.len(), 1);
                                applied = Some(effects[0]);
                            }
                            FoodRandomEvent::RemoveStatuses(ref effects) => {
                                removed = effects.contains(&FoodStatusKind::Poison)
                            }
                            _ => {}
                        }
                    }
                }
            }
            let expected_sounds = row["sounds"].as_array().unwrap();
            assert_eq!(sounds.len(), expected_sounds.len(), "{row}");
            for (sound, expected) in sounds.iter().zip(expected_sounds) {
                assert_eq!(
                    format!("minecraft:{}", sound.event),
                    expected["event"].as_str().unwrap(),
                    "{row}"
                );
                assert_eq!(
                    match sound.category {
                        crate::ConsumeSoundCategory::Player => "player",
                        crate::ConsumeSoundCategory::Neutral => "neutral",
                        crate::ConsumeSoundCategory::Ambient => "ambient",
                        crate::ConsumeSoundCategory::Block => "block",
                    },
                    expected["category"].as_str().unwrap(),
                    "{row}"
                );
                assert_eq!(
                    sound.volume.to_bits(),
                    expected["volume_bits"].as_i64().unwrap() as i32 as u32,
                    "{row}"
                );
                assert_eq!(
                    sound.pitch.to_bits(),
                    expected["pitch_bits"].as_i64().unwrap() as i32 as u32,
                    "{row}"
                );
            }
            assert_eq!(removed, row["poison_removed"].as_bool().unwrap(), "{row}");
            assert_eq!(
                inventory.slot(PlayerSlot::Storage(0)).map(|item| item.id()),
                row["remainder"].as_str(),
                "{row}"
            );
            assert_eq!(vitals.food, row["food"].as_i64().unwrap() as i32, "{row}");
            assert_eq!(
                vitals.saturation.to_bits(),
                row["saturation_bits"].as_i64().unwrap() as i32 as u32,
                "{row}"
            );
            assert_eq!(
                applied.is_some(),
                row["applied"].as_bool().unwrap(),
                "{row}"
            );
            if let Some(effect) = applied {
                assert_eq!(
                    effect.kind,
                    match row["effect"].as_str().unwrap() {
                        "hunger" => FoodStatusKind::Hunger,
                        "poison" => FoodStatusKind::Poison,
                        other => panic!("unexpected Java effect {other}"),
                    }
                );
                assert_eq!(effect.duration, row["duration"].as_i64().unwrap() as i32);
                assert_eq!(effect.amplifier, row["amplifier"].as_u64().unwrap() as u8);
            }
            assert_eq!(
                random.next_float().to_bits(),
                row["next_bits"].as_i64().unwrap() as i32 as u32,
                "{row}"
            );
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn rotten_flesh_random_and_hunger_merge_match_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("RottenFleshOracle.java");
        let output = temp.path().join("rotten.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/RottenFleshOracle.java"),
        )
        .unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg("RottenFleshOracle")
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "Java oracle failed: {}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let rows: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows["samples"].as_array().unwrap().len(), 100);
        for row in rows["samples"].as_array().unwrap() {
            let item = ItemStack::new(
                "minecraft:rotten_flesh".into(),
                1,
                64,
                Tag::Compound(vec![]),
            )
            .unwrap();
            let mut inventory = PlayerInventory::default();
            inventory.set_slot(PlayerSlot::Storage(0), Some(item));
            let mut inventory = Arc::new(inventory);
            let mut vitals = PlayerVitals {
                food: 10,
                ..Default::default()
            };
            let mut usage = FoodUse::default();
            let mut random = native_core::random::LegacyRandom::new(row["seed"].as_i64().unwrap());
            assert!(usage.start_hand(
                &mut inventory,
                PlayerSlot::Storage(0),
                &mut vitals,
                GameMode::Survival
            ));
            let mut hunger = false;
            for _ in 0..32 {
                usage.tick(&mut inventory, &mut vitals, GameMode::Survival);
                for event in usage.take_events() {
                    hunger |= event.apply(&mut random);
                }
            }
            assert_eq!(hunger, row["applied"].as_bool().unwrap(), "{row}");
            if hunger {
                assert_eq!(row["duration"], 600);
                assert_eq!(row["amplifier"], 0);
            }
            assert_eq!(
                random.next_float().to_bits(),
                row["next_bits"].as_i64().unwrap() as i32 as u32,
                "{row}"
            );
        }
        for row in rows["merges"].as_array().unwrap() {
            let mut effect = EffectDuration {
                ambient: false,
                visible: true,
                show_icon: true,
                duration: row["initial_duration"].as_i64().unwrap() as i32,
                amplifier: row["initial_amplifier"].as_u64().unwrap() as u8,
                hidden: None,
            };
            effect.merge(EffectDuration {
                ambient: false,
                visible: true,
                show_icon: true,
                duration: 600,
                amplifier: 0,
                hidden: None,
            });
            assert_eq!(
                effect.duration,
                row["duration"].as_i64().unwrap() as i32,
                "{row}"
            );
            assert_eq!(
                effect.amplifier,
                row["amplifier"].as_u64().unwrap() as u8,
                "{row}"
            );
            assert_eq!(
                effect.hidden.as_ref().map(|v| (v.duration, v.amplifier)),
                row["hidden_duration"].as_i64().map(|duration| (
                    duration as i32,
                    row["hidden_amplifier"].as_u64().unwrap() as u8
                )),
                "{row}"
            );
        }
    }

    #[test]
    fn rotten_flesh_applies_hunger_and_respects_consumable_override() {
        for seed in 0..8 {
            for overridden in [false, true] {
                let components = if overridden {
                    Tag::Compound(vec![("minecraft:consumable".into(), Tag::Compound(vec![]))])
                } else {
                    Tag::Compound(vec![])
                };
                let mut inventory = PlayerInventory::default();
                inventory.set_slot(
                    PlayerSlot::Storage(0),
                    ItemStack::new("minecraft:rotten_flesh".into(), 1, 64, components),
                );
                let mut session = LocalSession::demo();
                session.player_random = native_core::random::LegacyRandom::new(seed);
                session.set_player_inventory(inventory);
                session.set_player_vitals(PlayerVitals {
                    food: 10,
                    saturation: 0.0,
                    ..Default::default()
                });
                session.submit_command(PlayerCommand::Place);
                for _ in 0..32 {
                    session.update(Duration::from_millis(50));
                }
                let snapshot = session.snapshot();
                assert_eq!(snapshot.vitals.food, 14);
                assert!(snapshot.inventory.slot(PlayerSlot::Storage(0)).is_none());
                let mut random = native_core::random::LegacyRandom::new(seed);
                for _ in 0..6 {
                    FoodRandomEvent::ParticlePulse {
                        count: 5,
                        sound: FoodSound::Eat,
                        item: None,
                    }
                    .apply(&mut random);
                }
                let expected = FoodRandomEvent::Finish {
                    causes_hunger: !overridden,
                    particles: true,
                    sound: FoodSound::Eat,
                    item: None,
                }
                .apply(&mut random);
                assert_eq!(
                    snapshot.hunger_effect, expected,
                    "seed {seed}, override {overridden}"
                );
                if expected {
                    assert_eq!(
                        session.fall.effects().hunger.as_ref().unwrap().duration,
                        600
                    );
                    session.update(Duration::from_millis(50));
                    assert_eq!(session.snapshot().vitals.exhaustion, 0.005);
                    assert_eq!(
                        session.fall.effects().hunger.as_ref().unwrap().duration,
                        599
                    );
                }
            }
        }
    }

    #[test]
    fn honey_bottle_remainder_handles_stacks_and_creative_mode() {
        for mode in [GameMode::Survival, GameMode::Creative] {
            for count in [1, 2] {
                let mut inventory = PlayerInventory::default();
                inventory.set_slot(
                    PlayerSlot::Storage(0),
                    ItemStack::new(
                        "minecraft:honey_bottle".into(),
                        count,
                        16,
                        Tag::Compound(vec![]),
                    ),
                );
                let mut inventory = Arc::new(inventory);
                let mut vitals = PlayerVitals {
                    food: 20,
                    saturation: 0.0,
                    ..Default::default()
                };
                let mut usage = FoodUse::default();
                assert!(usage.start_hand(
                    &mut inventory,
                    PlayerSlot::Storage(0),
                    &mut vitals,
                    mode
                ));
                for _ in 0..40 {
                    usage.tick(&mut inventory, &mut vitals, mode);
                }
                assert!(!usage.is_active());
                assert!(
                    usage
                        .take_events()
                        .iter()
                        .any(|event| matches!(event, FoodRandomEvent::RemoveStatuses(effects) if effects.contains(&FoodStatusKind::Poison)))
                );
                let hand = inventory.slot(PlayerSlot::Storage(0)).unwrap();
                if mode == GameMode::Creative {
                    assert_eq!((hand.id(), hand.count()), ("minecraft:honey_bottle", count));
                    assert!(inventory.slot(PlayerSlot::Storage(1)).is_none());
                } else if count == 1 {
                    assert_eq!((hand.id(), hand.count()), ("minecraft:glass_bottle", 1));
                } else {
                    assert_eq!((hand.id(), hand.count()), ("minecraft:honey_bottle", 1));
                    let bottle = inventory.slot(PlayerSlot::Storage(1)).unwrap();
                    assert_eq!((bottle.id(), bottle.count()), ("minecraft:glass_bottle", 1));
                }
            }
        }
    }

    #[test]
    fn consume_effects_keep_tick_order_across_frame_rates() {
        let run = |id: &str, max: u32, frame_ms: u64| {
            let mut session = LocalSession::demo();
            session.player_random = native_core::random::LegacyRandom::new(99);
            let mut inventory = PlayerInventory::default();
            inventory.set_slot(
                PlayerSlot::Storage(0),
                ItemStack::new(id.into(), 2, max, Tag::Compound(vec![])),
            );
            session.set_player_inventory(inventory);
            session.set_player_vitals(PlayerVitals {
                food: 10,
                ..Default::default()
            });
            session.submit_command(PlayerCommand::Place);
            let mut sounds = Vec::new();
            let mut particles = Vec::new();
            for _ in 0..(4_000 / frame_ms) {
                session.update(Duration::from_millis(frame_ms));
                sounds.extend(session.take_consume_sounds());
                particles.extend(session.take_consume_particles());
            }
            assert!(session.take_consume_sounds().is_empty());
            assert!(session.take_consume_particles().is_empty());
            (sounds, particles)
        };
        for (id, max, sound) in [
            ("minecraft:apple", 64, "entity.generic.eat"),
            ("minecraft:honey_bottle", 16, "item.honey_bottle.drink"),
        ] {
            let (slow, slow_particles) = run(id, max, 100);
            let (fast, fast_particles) = run(id, max, 5);
            assert_eq!(slow, fast);
            assert_eq!(slow_particles, fast_particles);
            assert_eq!(
                slow_particles.len(),
                if id == "minecraft:apple" { 46 } else { 0 }
            );
            assert!(
                slow_particles
                    .iter()
                    .all(|particle| particle.item.id() == id && particle.item.count() == 2)
            );
            if id == "minecraft:apple" {
                assert!(Arc::ptr_eq(
                    &slow_particles[0].item,
                    &slow_particles[29].item
                ));
                assert!(Arc::ptr_eq(
                    &slow_particles[30].item,
                    &slow_particles[45].item
                ));
            }
            assert!(slow.len() >= 3);
            assert_eq!(slow[slow.len() - 3].info.event, sound);
            assert_eq!(
                slow[slow.len() - 2].info.category,
                crate::ConsumeSoundCategory::Neutral
            );
            assert_eq!(slow.last().unwrap().info.event, "entity.player.burp");
        }
    }

    #[test]
    fn hunger_food_outcome_is_independent_of_frame_cadence() {
        let run = |frame_ms: u64, id: &str| {
            let mut session = LocalSession::demo();
            session.player_random = native_core::random::LegacyRandom::new(99);
            let mut inventory = PlayerInventory::default();
            inventory.set_slot(
                PlayerSlot::Storage(0),
                ItemStack::new(id.into(), 2, 64, Tag::Compound(vec![])),
            );
            session.set_player_inventory(inventory);
            session.set_player_vitals(PlayerVitals {
                food: 5,
                saturation: 0.0,
                ..Default::default()
            });
            session.submit_command(PlayerCommand::UseHeld(true));
            session.submit_command(PlayerCommand::Place);
            for _ in 0..(4_000 / frame_ms) {
                session.update(Duration::from_millis(frame_ms));
            }
            (
                session.snapshot(),
                session.fall.effects().hunger.clone(),
                session.player_random.next_float().to_bits(),
            )
        };
        for id in ["minecraft:rotten_flesh", "minecraft:chicken"] {
            let (slow, slow_effect, slow_random) = run(100, id);
            let (fast, fast_effect, fast_random) = run(5, id);
            assert_eq!(slow.tick, fast.tick);
            assert_eq!(slow.vitals, fast.vitals);
            assert_eq!(
                slow.inventory.slot(PlayerSlot::Storage(0)),
                fast.inventory.slot(PlayerSlot::Storage(0))
            );
            assert_eq!(slow_effect, fast_effect);
            assert_eq!(slow_random, fast_random);
        }
    }

    #[test]
    fn custom_food_applies_grouped_hunger_effects_and_particle_flag() {
        let effect = |duration, amplifier| {
            Tag::Compound(vec![
                ("id".into(), Tag::String("minecraft:hunger".into())),
                ("duration".into(), Tag::Int(duration)),
                ("amplifier".into(), Tag::Byte(amplifier)),
            ])
        };
        let group = |probability, effects| {
            Tag::Compound(vec![
                ("type".into(), Tag::String("minecraft:apply_effects".into())),
                ("probability".into(), Tag::Float(probability)),
                (
                    "effects".into(),
                    Tag::List {
                        element_type: native_persistence::nbt::TagType::Compound,
                        values: effects,
                    },
                ),
            ])
        };
        let component = Tag::Compound(vec![
            ("consume_seconds".into(), Tag::Float(0.0)),
            ("has_consume_particles".into(), Tag::Byte(0)),
            (
                "on_consume_effects".into(),
                Tag::List {
                    element_type: native_persistence::nbt::TagType::Compound,
                    values: vec![
                        group(1.0, vec![effect(20, 0), effect(10, 2)]),
                        group(0.0, vec![effect(900, 3)]),
                    ],
                },
            ),
        ]);
        let components = Tag::Compound(vec![("minecraft:consumable".into(), component)]);
        assert_eq!(saved_item_max("minecraft:apple", &components), Some(64));
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(
            PlayerSlot::Storage(0),
            ItemStack::new("minecraft:apple".into(), 1, 64, components),
        );
        let mut session = LocalSession::demo();
        session.set_player_inventory(inventory);
        session.set_player_vitals(PlayerVitals {
            food: 10,
            ..Default::default()
        });
        session.submit_command(PlayerCommand::Place);
        session.update(Duration::from_millis(50));
        let hunger = session.fall.effects().hunger.as_ref().unwrap();
        assert_eq!((hunger.duration, hunger.amplifier), (9, 2));
        assert_eq!(
            (
                hunger.hidden.as_ref().unwrap().duration,
                hunger.hidden.as_ref().unwrap().amplifier
            ),
            (19, 0)
        );
        assert_eq!(session.snapshot().vitals.food, 14);
    }

    #[test]
    fn unsupported_removal_target_does_not_partially_consume() {
        for target in ["minecraft:weakness", "#minecraft:some_tag"] {
            let component = Tag::Compound(vec![(
                "on_consume_effects".into(),
                Tag::List {
                    element_type: native_persistence::nbt::TagType::Compound,
                    values: vec![Tag::Compound(vec![
                        (
                            "type".into(),
                            Tag::String("minecraft:remove_effects".into()),
                        ),
                        (
                            "effects".into(),
                            Tag::List {
                                element_type: native_persistence::nbt::TagType::String,
                                values: vec![
                                    Tag::String("minecraft:poison".into()),
                                    Tag::String(target.into()),
                                ],
                            },
                        ),
                    ])],
                },
            )]);
            let components = Tag::Compound(vec![("minecraft:consumable".into(), component)]);
            assert_eq!(saved_item_max("minecraft:apple", &components), None);
            let mut inventory = PlayerInventory::default();
            inventory.set_slot(
                PlayerSlot::Storage(0),
                ItemStack::new("minecraft:apple".into(), 1, 64, components),
            );
            let mut inventory = Arc::new(inventory);
            let mut vitals = PlayerVitals {
                food: 10,
                ..Default::default()
            };
            assert!(!FoodUse::default().start_hand(
                &mut inventory,
                PlayerSlot::Storage(0),
                &mut vitals,
                GameMode::Survival
            ));
            assert!(inventory.slot(PlayerSlot::Storage(0)).is_some());
            assert_eq!(vitals.food, 10);
        }
    }

    #[test]
    fn unsupported_effect_prevents_partial_food_consumption() {
        let component = Tag::Compound(vec![(
            "on_consume_effects".into(),
            Tag::List {
                element_type: native_persistence::nbt::TagType::Compound,
                values: vec![Tag::Compound(vec![
                    ("type".into(), Tag::String("minecraft:apply_effects".into())),
                    (
                        "effects".into(),
                        Tag::List {
                            element_type: native_persistence::nbt::TagType::Compound,
                            values: vec![
                                Tag::Compound(vec![(
                                    "id".into(),
                                    Tag::String("minecraft:hunger".into()),
                                )]),
                                Tag::Compound(vec![(
                                    "id".into(),
                                    Tag::String("minecraft:weakness".into()),
                                )]),
                            ],
                        },
                    ),
                ])],
            },
        )]);
        let components = Tag::Compound(vec![("minecraft:consumable".into(), component)]);
        assert_eq!(saved_item_max("minecraft:apple", &components), None);
        let item = ItemStack::new("minecraft:apple".into(), 1, 64, components).unwrap();
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(PlayerSlot::Storage(0), Some(item));
        let mut inventory = Arc::new(inventory);
        let mut vitals = PlayerVitals {
            food: 10,
            ..Default::default()
        };
        let mut usage = FoodUse::default();
        assert!(!usage.start_hand(
            &mut inventory,
            PlayerSlot::Storage(0),
            &mut vitals,
            GameMode::Survival
        ));
        assert_eq!(vitals.food, 10);
        assert!(inventory.slot(PlayerSlot::Storage(0)).is_some());
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn custom_consumable_hunger_matches_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("CustomHungerOracle.java");
        let output = temp.path().join("custom.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/CustomHungerOracle.java"),
        )
        .unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg("CustomHungerOracle")
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "Java oracle failed: {}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let effect = |duration, amplifier| {
            Tag::Compound(vec![
                ("id".into(), Tag::String("minecraft:hunger".into())),
                ("duration".into(), Tag::Int(duration)),
                ("amplifier".into(), Tag::Byte(amplifier)),
            ])
        };
        let group = |probability, effects| {
            Tag::Compound(vec![
                ("type".into(), Tag::String("minecraft:apply_effects".into())),
                ("probability".into(), Tag::Float(probability)),
                (
                    "effects".into(),
                    Tag::List {
                        element_type: native_persistence::nbt::TagType::Compound,
                        values: effects,
                    },
                ),
            ])
        };
        let mut ambient_effect = effect(-1, 1);
        if let Tag::Compound(fields) = &mut ambient_effect {
            fields.push(("ambient".into(), Tag::Byte(1)));
            fields.push(("show_particles".into(), Tag::Byte(0)));
        }
        let poison = |duration, amplifier| {
            Tag::Compound(vec![
                ("id".into(), Tag::String("minecraft:poison".into())),
                ("duration".into(), Tag::Int(duration)),
                ("amplifier".into(), Tag::Byte(amplifier)),
            ])
        };
        let remove = |effects| {
            Tag::Compound(vec![
                (
                    "type".into(),
                    Tag::String("minecraft:remove_effects".into()),
                ),
                ("effects".into(), effects),
            ])
        };
        let cases = [
            Tag::Compound(vec![
                ("consume_seconds".into(), Tag::Float(0.0)),
                ("has_consume_particles".into(), Tag::Byte(0)),
                (
                    "on_consume_effects".into(),
                    Tag::List {
                        element_type: native_persistence::nbt::TagType::Compound,
                        values: vec![
                            group(1.0, vec![effect(20, 0), effect(10, 2)]),
                            group(0.0, vec![effect(900, 3)]),
                        ],
                    },
                ),
            ]),
            Tag::Compound(vec![
                ("consume_seconds".into(), Tag::Float(0.0)),
                (
                    "on_consume_effects".into(),
                    Tag::List {
                        element_type: native_persistence::nbt::TagType::Compound,
                        values: vec![
                            group(0.35, vec![effect(70, 1)]),
                            group(0.8, vec![effect(300, 0)]),
                        ],
                    },
                ),
            ]),
            Tag::Compound(vec![
                ("consume_seconds".into(), Tag::Float(0.25)),
                ("has_consume_particles".into(), Tag::Byte(0)),
                (
                    "on_consume_effects".into(),
                    Tag::List {
                        element_type: native_persistence::nbt::TagType::Compound,
                        values: vec![group(1.0, vec![ambient_effect])],
                    },
                ),
            ]),
            Tag::Compound(vec![
                ("consume_seconds".into(), Tag::Float(0.0)),
                (
                    "on_consume_effects".into(),
                    Tag::List {
                        element_type: native_persistence::nbt::TagType::Compound,
                        values: vec![
                            group(0.35, vec![effect(70, 1), poison(100, 0)]),
                            group(0.8, vec![poison(20, 2)]),
                        ],
                    },
                ),
            ]),
            Tag::Compound(vec![
                ("consume_seconds".into(), Tag::Float(0.0)),
                (
                    "on_consume_effects".into(),
                    Tag::List {
                        element_type: native_persistence::nbt::TagType::Compound,
                        values: vec![
                            remove(Tag::String("minecraft:poison".into())),
                            group(0.35, vec![poison(100, 0)]),
                        ],
                    },
                ),
            ]),
            Tag::Compound(vec![
                ("consume_seconds".into(), Tag::Float(0.0)),
                (
                    "on_consume_effects".into(),
                    Tag::List {
                        element_type: native_persistence::nbt::TagType::Compound,
                        values: vec![
                            remove(Tag::List {
                                element_type: native_persistence::nbt::TagType::String,
                                values: vec![
                                    Tag::String("minecraft:hunger".into()),
                                    Tag::String("minecraft:poison".into()),
                                ],
                            }),
                            group(0.8, vec![poison(20, 2)]),
                        ],
                    },
                ),
            ]),
            Tag::Compound(vec![
                ("consume_seconds".into(), Tag::Float(0.0)),
                (
                    "on_consume_effects".into(),
                    Tag::List {
                        element_type: native_persistence::nbt::TagType::Compound,
                        values: vec![remove(Tag::List {
                            element_type: native_persistence::nbt::TagType::String,
                            values: vec![],
                        })],
                    },
                ),
            ]),
            Tag::Compound(vec![
                ("consume_seconds".into(), Tag::Float(0.0)),
                (
                    "on_consume_effects".into(),
                    Tag::List {
                        element_type: native_persistence::nbt::TagType::Compound,
                        values: vec![
                            Tag::Compound(vec![(
                                "type".into(),
                                Tag::String("minecraft:clear_all_effects".into()),
                            )]),
                            group(0.8, vec![poison(20, 2)]),
                        ],
                    },
                ),
            ]),
            Tag::Compound(vec![
                ("consume_seconds".into(), Tag::Float(0.0)),
                (
                    "on_consume_effects".into(),
                    Tag::List {
                        element_type: native_persistence::nbt::TagType::Compound,
                        values: vec![
                            group(1.0, vec![effect(40, 0)]),
                            Tag::Compound(vec![(
                                "type".into(),
                                Tag::String("minecraft:clear_all_effects".into()),
                            )]),
                        ],
                    },
                ),
            ]),
        ];
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), 360);
        for row in rows {
            let case = row["case"].as_u64().unwrap() as usize;
            let components =
                Tag::Compound(vec![("minecraft:consumable".into(), cases[case].clone())]);
            let item = ItemStack::new("minecraft:apple".into(), 1, 64, components).unwrap();
            let mut inventory = PlayerInventory::default();
            inventory.set_slot(PlayerSlot::Storage(0), Some(item));
            let mut inventory = Arc::new(inventory);
            let mut vitals = PlayerVitals {
                food: 10,
                ..Default::default()
            };
            let mut usage = FoodUse::default();
            let mut random = native_core::random::LegacyRandom::new(row["seed"].as_i64().unwrap());
            assert!(usage.start_hand(
                &mut inventory,
                PlayerSlot::Storage(0),
                &mut vitals,
                GameMode::Survival
            ));
            for _ in 0..=5 {
                if usage.is_active() {
                    usage.tick(&mut inventory, &mut vitals, GameMode::Survival);
                }
            }
            let mut active = (case >= 4).then_some(EffectDuration {
                ambient: false,
                visible: true,
                show_icon: true,
                duration: 50,
                amplifier: 0,
                hidden: None,
            });
            let mut active_poison = (case >= 4).then_some(EffectDuration {
                ambient: false,
                visible: true,
                show_icon: true,
                duration: 200,
                amplifier: 1,
                hidden: None,
            });
            for event in usage.take_events() {
                if !event.apply(&mut random) {
                    continue;
                }
                match event {
                    FoodRandomEvent::StatusGroup { effects, .. } => {
                        for effect in effects {
                            let incoming = EffectDuration {
                                ambient: false,
                                visible: true,
                                show_icon: true,
                                duration: effect.duration,
                                amplifier: effect.amplifier,
                                hidden: None,
                            };
                            let target = match effect.kind {
                                FoodStatusKind::Hunger => &mut active,
                                FoodStatusKind::Poison => &mut active_poison,
                                FoodStatusKind::NightVision
                                | FoodStatusKind::WaterBreathing
                                | FoodStatusKind::Regeneration
                                | FoodStatusKind::JumpBoost
                                | FoodStatusKind::SlowFalling
                                | FoodStatusKind::Speed
                                | FoodStatusKind::Slowness => {
                                    unreachable!()
                                }
                            };
                            if let Some(existing) = target {
                                existing.merge(incoming);
                            } else {
                                *target = Some(incoming);
                            }
                        }
                    }
                    FoodRandomEvent::RemoveStatuses(effects) => {
                        for effect in effects {
                            match effect {
                                FoodStatusKind::Hunger => active = None,
                                FoodStatusKind::Poison => active_poison = None,
                                FoodStatusKind::NightVision
                                | FoodStatusKind::WaterBreathing
                                | FoodStatusKind::Regeneration
                                | FoodStatusKind::JumpBoost
                                | FoodStatusKind::SlowFalling
                                | FoodStatusKind::Speed
                                | FoodStatusKind::Slowness => {
                                    unreachable!()
                                }
                            }
                        }
                    }
                    FoodRandomEvent::ClearAllStatuses => {
                        active = None;
                        active_poison = None;
                    }
                    _ => {}
                }
            }
            assert_eq!(
                active_poison.as_ref().map(|v| (v.duration, v.amplifier)),
                row["poison_duration"]
                    .as_i64()
                    .map(|v| (v as i32, row["poison_amplifier"].as_u64().unwrap() as u8)),
                "{row}"
            );
            assert_eq!(
                active_poison
                    .as_ref()
                    .and_then(|v| v.hidden.as_ref())
                    .map(|v| (v.duration, v.amplifier)),
                row["poison_hidden_duration"].as_i64().map(|v| (
                    v as i32,
                    row["poison_hidden_amplifier"].as_u64().unwrap() as u8
                )),
                "{row}"
            );
            assert_eq!(
                active.as_ref().map(|v| (v.duration, v.amplifier)),
                row["duration"]
                    .as_i64()
                    .map(|v| (v as i32, row["amplifier"].as_u64().unwrap() as u8)),
                "{row}"
            );
            assert_eq!(
                active
                    .as_ref()
                    .and_then(|v| v.hidden.as_ref())
                    .map(|v| (v.duration, v.amplifier)),
                row["hidden_duration"]
                    .as_i64()
                    .map(|v| (v as i32, row["hidden_amplifier"].as_u64().unwrap() as u8)),
                "{row}"
            );
            assert_eq!(
                random.next_float().to_bits(),
                row["next_bits"].as_i64().unwrap() as i32 as u32,
                "{row}"
            );
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn regeneration_ticks_match_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("RegenerationEffectOracle.java");
        let output = temp.path().join("regen.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/RegenerationEffectOracle.java"),
        )
        .unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg("RegenerationEffectOracle")
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), 1584);
        for row in rows {
            let mut vitals = PlayerVitals {
                health: row["health"].as_f64().unwrap() as f32,
                max_health: row["maximum"].as_f64().unwrap() as f32,
                ..Default::default()
            };
            let mut effects = FallEffects {
                regeneration: Some(EffectDuration {
                    ambient: false,
                    visible: true,
                    show_icon: true,
                    amplifier: row["amplifier"].as_u64().unwrap() as u8,
                    duration: row["duration"].as_i64().unwrap() as i32,
                    hidden: None,
                }),
                ..Default::default()
            };
            for (index, expected) in row["ticks"].as_array().unwrap().iter().enumerate() {
                if vitals.health < vitals.max_health && effects.regeneration_due(index as i32 + 1) {
                    vitals.heal(1.0);
                }
                effects.tick();
                assert_eq!(
                    vitals.health.to_bits(),
                    expected["health_bits"].as_i64().unwrap() as i32 as u32,
                    "{row}"
                );
                assert_eq!(
                    effects.regeneration.is_some(),
                    expected["active"].as_bool().unwrap(),
                    "{row}"
                );
                if let Some(effect) = &effects.regeneration {
                    assert_eq!(
                        effect.duration,
                        expected["duration"].as_i64().unwrap() as i32,
                        "{row}"
                    );
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn poison_ticks_match_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("PoisonEffectOracle.java");
        let output = temp.path().join("poison.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/PoisonEffectOracle.java"),
        )
        .unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg("PoisonEffectOracle")
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "Java poison oracle failed: {}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), 256);
        for row in rows {
            let mut health = row["health"].as_f64().unwrap() as f32;
            let mut effects = FallEffects {
                poison: Some(EffectDuration {
                    ambient: false,
                    visible: true,
                    show_icon: true,
                    amplifier: row["amplifier"].as_u64().unwrap() as u8,
                    duration: row["duration"].as_i64().unwrap() as i32,
                    hidden: None,
                }),
                ..Default::default()
            };
            for (index, expected) in row["ticks"].as_array().unwrap().iter().enumerate() {
                let hit = health > 1.0 && effects.poison_due(index as i32 + 1);
                if hit {
                    health -= 1.0;
                }
                effects.tick();
                assert_eq!(hit, expected["hit"].as_bool().unwrap(), "{row}");
                assert_eq!(
                    health.to_bits(),
                    expected["health_bits"].as_i64().unwrap() as i32 as u32,
                    "{row}"
                );
                assert_eq!(
                    effects.poison.is_some(),
                    expected["active"].as_bool().unwrap(),
                    "{row}"
                );
                if let Some(effect) = &effects.poison {
                    assert_eq!(
                        effect.duration,
                        expected["duration"].as_i64().unwrap() as i32,
                        "{row}"
                    );
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn hunger_ticks_match_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("HungerEffectOracle.java");
        let output = temp.path().join("hunger.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/HungerEffectOracle.java"),
        )
        .unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg("HungerEffectOracle")
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "Java hunger oracle failed: {}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), 120);
        for row in rows {
            let mode = if row["invulnerable"].as_bool().unwrap() {
                GameMode::Creative
            } else {
                GameMode::Survival
            };
            let mut vitals = PlayerVitals {
                exhaustion: row["initial"].as_f64().unwrap() as f32,
                ..Default::default()
            };
            let mut effects = FallEffects {
                hunger: Some(EffectDuration {
                    ambient: false,
                    visible: true,
                    show_icon: true,
                    amplifier: row["amplifier"].as_u64().unwrap() as u8,
                    duration: row["duration"].as_i64().unwrap() as i32,
                    hidden: None,
                }),
                ..Default::default()
            };
            for expected in row["ticks"].as_array().unwrap() {
                effects.apply_hunger(&mut vitals, mode);
                effects.tick();
                assert_eq!(
                    vitals.exhaustion.to_bits(),
                    expected["exhaustion_bits"].as_i64().unwrap() as i32 as u32,
                    "{row}"
                );
                assert_eq!(
                    effects.hunger.is_some(),
                    expected["active"].as_bool().unwrap(),
                    "{row}"
                );
                if let Some(effect) = &effects.hunger {
                    assert_eq!(
                        effect.duration,
                        expected["duration"].as_i64().unwrap() as i32,
                        "{row}"
                    );
                }
            }
        }
    }
    use native_persistence::nbt::Tag;
    use std::time::Duration;

    #[test]
    fn ordinary_air_depletes_recovers_and_damages_on_the_drowning_boundary() {
        let mut vitals = PlayerVitals {
            absorption: 1.0,
            ..Default::default()
        };
        for _ in 0..300 {
            vitals.tick_air(true, false, GameMode::Survival);
        }
        assert_eq!(vitals.air, 0);
        assert_eq!(vitals.health, 20.0);
        for _ in 0..19 {
            vitals.tick_air(true, false, GameMode::Survival);
        }
        assert_eq!(vitals.air, -19);
        vitals.tick_air(true, false, GameMode::Survival);
        assert_eq!(
            (vitals.air, vitals.absorption, vitals.health),
            (0, 0.0, 19.0)
        );
        vitals.tick_air(false, false, GameMode::Survival);
        assert_eq!(vitals.air, 4);
        vitals.tick_air(true, true, GameMode::Survival);
        assert_eq!(vitals.air, 8);
        vitals.tick_air(true, false, GameMode::Creative);
        assert_eq!(vitals.air, 12);
        vitals.air = 299;
        vitals.tick_air(false, false, GameMode::Survival);
        assert_eq!(vitals.air, 300);
        vitals.health = 0.0;
        vitals.air = 10;
        vitals.tick_air(false, false, GameMode::Survival);
        assert_eq!(vitals.air, 10);
    }

    fn apple_inventory(hand: PlayerSlot) -> PlayerInventory {
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(
            hand,
            ItemStack::new("minecraft:apple".into(), 2, 64, Tag::Compound(vec![])),
        );
        inventory
    }

    #[test]
    fn drinking_potions_works_in_both_hands_and_creative() {
        for hand in [PlayerSlot::Storage(0), PlayerSlot::Offhand] {
            for mode in [GameMode::Survival, GameMode::Creative] {
                let mut session = LocalSession::demo();
                session.set_game_mode(mode);
                let mut inventory = PlayerInventory::default();
                inventory.set_slot(
                    hand,
                    ItemStack::new(
                        "minecraft:potion".into(),
                        1,
                        1,
                        Tag::Compound(vec![(
                            "minecraft:potion_contents".into(),
                            Tag::Compound(vec![(
                                "potion".into(),
                                Tag::String("minecraft:night_vision".into()),
                            )]),
                        )]),
                    ),
                );
                session.set_player_inventory(inventory);
                session.set_player_vitals(PlayerVitals {
                    food: 20,
                    saturation: 5.0,
                    ..Default::default()
                });
                let before = session.snapshot();
                session.submit_command(PlayerCommand::Place);
                for _ in 0..31 {
                    session.update(Duration::from_millis(50));
                }
                assert_eq!(session.snapshot().night_vision_duration, None);
                session.update(Duration::from_millis(50));
                let after = session.snapshot();
                assert_eq!(after.night_vision_duration, Some(3600));
                assert_eq!((after.vitals.food, after.vitals.saturation), (20, 5.0));
                assert_eq!(
                    after.inventory.slot(hand).unwrap().id(),
                    if mode == GameMode::Creative {
                        "minecraft:potion"
                    } else {
                        "minecraft:glass_bottle"
                    }
                );
                assert_eq!(
                    before.inventory.slot(hand).unwrap().id(),
                    "minecraft:potion"
                );
            }
        }
    }

    #[test]
    fn eating_uses_fixed_ticks_cancels_on_release_and_preserves_snapshots() {
        let mut session = LocalSession::demo();
        session.set_player_inventory(apple_inventory(PlayerSlot::Storage(0)));
        session.set_player_vitals(PlayerVitals {
            food: 10,
            saturation: 0.0,
            ..PlayerVitals::default()
        });
        let original = session.snapshot();
        session.submit_command(PlayerCommand::Place);
        for _ in 0..31 {
            session.update(Duration::from_millis(50));
        }
        assert_eq!(session.snapshot().vitals.food, 10);
        assert_eq!(
            session.snapshot().first_person.item_use,
            Some(crate::animation::ItemUseState {
                hand: crate::animation::Hand::Main,
                remaining_ticks: 1,
            })
        );
        session.update(Duration::from_millis(50));
        let eaten = session.snapshot();
        assert_eq!(eaten.vitals.food, 14);
        assert_eq!(eaten.first_person.item_use, None);
        assert_eq!(eaten.vitals.saturation, 2.4);
        assert_eq!(
            eaten
                .inventory
                .slot(PlayerSlot::Storage(0))
                .unwrap()
                .count(),
            1
        );
        assert_eq!(
            original
                .inventory
                .slot(PlayerSlot::Storage(0))
                .unwrap()
                .count(),
            2
        );
        assert_eq!(original.vitals.food, 10);

        session.submit_command(PlayerCommand::Place);
        for _ in 0..10 {
            session.update(Duration::from_millis(50));
        }
        session.submit_command(PlayerCommand::StopUse);
        for _ in 0..40 {
            session.update(Duration::from_millis(50));
        }
        assert_eq!(session.snapshot().first_person.item_use, None);
        assert_eq!(session.snapshot().vitals.food, 14);
        assert_eq!(
            session
                .snapshot()
                .inventory
                .slot(PlayerSlot::Storage(0))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn food_use_handles_offhand_full_hunger_creative_and_changed_items() {
        let mut session = LocalSession::demo();
        session.set_player_inventory(apple_inventory(PlayerSlot::Offhand));
        session.set_player_vitals(PlayerVitals {
            food: 19,
            saturation: 0.0,
            ..PlayerVitals::default()
        });
        session.submit_command(PlayerCommand::Place);
        session.update(Duration::from_millis(50));
        assert_eq!(
            session.snapshot().first_person.item_use,
            Some(crate::animation::ItemUseState {
                hand: crate::animation::Hand::Off,
                remaining_ticks: 31,
            })
        );
        for _ in 1..32 {
            session.update(Duration::from_millis(50));
        }
        assert_eq!(session.snapshot().vitals.food, 20);
        assert_eq!(
            session
                .snapshot()
                .inventory
                .slot(PlayerSlot::Offhand)
                .unwrap()
                .count(),
            1
        );
        session.submit_command(PlayerCommand::Place);
        for _ in 0..32 {
            session.update(Duration::from_millis(50));
        }
        assert_eq!(
            session
                .snapshot()
                .inventory
                .slot(PlayerSlot::Offhand)
                .unwrap()
                .count(),
            1
        );
        session.set_game_mode(GameMode::Creative);
        session.submit_command(PlayerCommand::Place);
        for _ in 0..32 {
            session.update(Duration::from_millis(50));
        }
        assert_eq!(
            session
                .snapshot()
                .inventory
                .slot(PlayerSlot::Offhand)
                .unwrap()
                .count(),
            1
        );

        session.set_game_mode(GameMode::Survival);
        session.set_player_vitals(PlayerVitals {
            food: 10,
            ..PlayerVitals::default()
        });
        session.set_player_inventory(apple_inventory(PlayerSlot::Storage(0)));
        session.submit_command(PlayerCommand::Place);
        for _ in 0..10 {
            session.update(Duration::from_millis(50));
        }
        session.submit_command(PlayerCommand::SelectSlot(1));
        for _ in 0..30 {
            session.update(Duration::from_millis(50));
        }
        assert_eq!(session.snapshot().vitals.food, 10);
        assert_eq!(
            session
                .snapshot()
                .inventory
                .slot(PlayerSlot::Storage(0))
                .unwrap()
                .count(),
            2
        );
    }

    #[test]
    fn unknown_food_components_are_not_consumed() {
        let mut inventory = PlayerInventory::default();
        let components = Tag::Compound(vec![("minecraft:food".into(), Tag::Compound(vec![]))]);
        inventory.set_slot(
            PlayerSlot::Storage(0),
            ItemStack::unresolved("minecraft:apple".into(), 1, components),
        );
        let mut use_state = FoodUse::default();
        assert!(!use_state.start_hand(
            &mut Arc::new(inventory),
            PlayerSlot::Storage(0),
            &mut PlayerVitals {
                food: 1,
                ..PlayerVitals::default()
            },
            GameMode::Survival
        ));
        assert!(!use_state.is_active());
    }

    #[test]
    fn custom_food_and_consumable_change_hunger_and_use_duration() {
        let mut session = LocalSession::demo();
        let components = Tag::Compound(vec![
            (
                "minecraft:food".into(),
                Tag::Compound(vec![
                    ("nutrition".into(), Tag::Int(7)),
                    ("saturation".into(), Tag::Float(3.5)),
                    ("can_always_eat".into(), Tag::Byte(1)),
                ]),
            ),
            (
                "minecraft:consumable".into(),
                Tag::Compound(vec![("consume_seconds".into(), Tag::Float(0.25))]),
            ),
        ]);
        assert_eq!(saved_item_max("minecraft:apple", &components), Some(64));
        let item = ItemStack::new("minecraft:apple".into(), 2, 64, components).unwrap();
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(PlayerSlot::Storage(0), Some(item));
        session.set_player_inventory(inventory);
        session.set_player_vitals(PlayerVitals {
            food: 20,
            saturation: 1.0,
            ..Default::default()
        });
        session.submit_command(PlayerCommand::Place);
        for _ in 0..4 {
            session.update(Duration::from_millis(50));
        }
        assert_eq!(session.snapshot().vitals.saturation, 1.0);
        session.update(Duration::from_millis(50));
        let snapshot = session.snapshot();
        assert_eq!(snapshot.vitals.food, 20);
        assert_eq!(snapshot.vitals.saturation, 4.5);
        assert_eq!(
            snapshot
                .inventory
                .slot(PlayerSlot::Storage(0))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn instant_food_is_consumed_before_the_next_command() {
        let components = Tag::Compound(vec![(
            "minecraft:consumable".into(),
            Tag::Compound(vec![("consume_seconds".into(), Tag::Float(0.01))]),
        )]);
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(
            PlayerSlot::Storage(0),
            ItemStack::new("minecraft:apple".into(), 2, 64, components),
        );
        let mut session = LocalSession::demo();
        session.set_player_inventory(inventory);
        session.set_player_vitals(PlayerVitals {
            food: 10,
            saturation: 0.0,
            ..Default::default()
        });
        session.submit_command(PlayerCommand::Place);
        session.submit_command(PlayerCommand::StopUse);
        session.update(Duration::from_millis(50));
        let result = session.snapshot();
        assert_eq!(result.vitals.food, 14);
        assert_eq!(
            result
                .inventory
                .slot(PlayerSlot::Storage(0))
                .unwrap()
                .count(),
            1
        );
        assert_eq!(result.first_person.item_use, None);
    }

    #[test]
    fn holding_instant_food_respects_the_use_repeat_delay() {
        let components = Tag::Compound(vec![(
            "minecraft:consumable".into(),
            Tag::Compound(vec![("consume_seconds".into(), Tag::Float(0.0))]),
        )]);
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(
            PlayerSlot::Storage(0),
            ItemStack::new("minecraft:apple".into(), 3, 64, components),
        );
        let mut session = LocalSession::demo();
        session.set_player_inventory(inventory);
        session.set_player_vitals(PlayerVitals {
            food: 1,
            saturation: 0.0,
            ..Default::default()
        });
        session.submit_command(PlayerCommand::UseHeld(true));
        session.submit_command(PlayerCommand::Place);
        session.update(Duration::from_millis(50));
        assert_eq!(session.snapshot().vitals.food, 5);
        for _ in 0..3 {
            session.update(Duration::from_millis(50));
        }
        assert_eq!(session.snapshot().vitals.food, 5);
        session.update(Duration::from_millis(50));
        assert_eq!(session.snapshot().vitals.food, 9);
    }

    #[test]
    fn long_custom_food_counts_down_without_truncation() {
        let components = Tag::Compound(vec![(
            "minecraft:consumable".into(),
            Tag::Compound(vec![("consume_seconds".into(), Tag::Float(15.0))]),
        )]);
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(
            PlayerSlot::Storage(0),
            ItemStack::new("minecraft:apple".into(), 1, 64, components),
        );
        let mut inventory = Arc::new(inventory);
        let mut vitals = PlayerVitals {
            food: 10,
            ..Default::default()
        };
        let mut use_state = FoodUse::default();
        assert!(use_state.start_hand(
            &mut inventory,
            PlayerSlot::Storage(0),
            &mut vitals,
            GameMode::Survival
        ));
        assert_eq!(use_state.visual_state().unwrap().remaining_ticks, 300);
        for _ in 0..299 {
            use_state.tick(&mut inventory, &mut vitals, GameMode::Survival);
        }
        assert_eq!(use_state.visual_state().unwrap().remaining_ticks, 1);
        assert_eq!(vitals.food, 10);
        use_state.tick(&mut inventory, &mut vitals, GameMode::Survival);
        assert_eq!(vitals.food, 14);
        assert!(inventory.slot(PlayerSlot::Storage(0)).is_none());
    }

    #[test]
    fn negative_consume_seconds_are_rejected() {
        let component = Tag::Compound(vec![("consume_seconds".into(), Tag::Float(-0.01))]);
        assert_eq!(custom_consumable_ticks(&component), None);
    }

    #[test]
    fn stew_returns_bowl_to_the_used_hand_unless_remainder_was_removed() {
        for removed in [false, true] {
            let components = if removed {
                Tag::Compound(vec![(
                    "!minecraft:use_remainder".into(),
                    Tag::Compound(vec![]),
                )])
            } else {
                Tag::Compound(vec![])
            };
            let stew = ItemStack::new("minecraft:mushroom_stew".into(), 1, 1, components).unwrap();
            let mut inventory = PlayerInventory::default();
            inventory.set_slot(PlayerSlot::Offhand, Some(stew));
            let mut inventory = Arc::new(inventory);
            let mut vitals = PlayerVitals {
                food: 10,
                saturation: 0.0,
                ..Default::default()
            };
            let mut usage = FoodUse::default();
            assert!(usage.start_hand(
                &mut inventory,
                PlayerSlot::Offhand,
                &mut vitals,
                GameMode::Survival
            ));
            for _ in 0..32 {
                usage.tick(&mut inventory, &mut vitals, GameMode::Survival);
            }
            assert_eq!(vitals.food, 16);
            assert_eq!(
                inventory.slot(PlayerSlot::Offhand).map(ItemStack::id),
                (!removed).then_some("minecraft:bowl")
            );
            assert!(usage.take_drops().is_empty());
        }
    }

    #[test]
    fn custom_remainder_is_returned_or_dropped_when_inventory_is_full() {
        for full in [false, true] {
            let components = Tag::Compound(vec![
                (
                    "minecraft:consumable".into(),
                    Tag::Compound(vec![("consume_seconds".into(), Tag::Float(0.0))]),
                ),
                (
                    "minecraft:use_remainder".into(),
                    Tag::String("minecraft:bowl".into()),
                ),
            ]);
            let mut inventory = PlayerInventory::default();
            if full {
                for slot in 1..36 {
                    inventory.set_slot(
                        PlayerSlot::Storage(slot),
                        ItemStack::new("minecraft:dirt".into(), 64, 64, Tag::Compound(vec![])),
                    );
                }
            }
            inventory.set_slot(
                PlayerSlot::Storage(0),
                ItemStack::new("minecraft:apple".into(), 2, 64, components),
            );
            let mut session = LocalSession::demo();
            session.set_player_inventory(inventory);
            session.set_player_vitals(PlayerVitals {
                food: 10,
                saturation: 0.0,
                ..Default::default()
            });
            session.submit_command(PlayerCommand::Place);
            session.update(Duration::from_millis(50));
            let result = session.snapshot();
            assert_eq!(
                result
                    .inventory
                    .slot(PlayerSlot::Storage(0))
                    .unwrap()
                    .count(),
                1
            );
            assert_eq!(
                result
                    .inventory
                    .slot(PlayerSlot::Storage(1))
                    .map(ItemStack::id),
                Some(if full {
                    "minecraft:dirt"
                } else {
                    "minecraft:bowl"
                })
            );
            assert_eq!(result.dropped_items.len(), usize::from(full));
            if full {
                assert_eq!(result.dropped_items[0].stack.id(), "minecraft:bowl");
            }
        }
    }

    #[test]
    fn remainder_merges_available_space_and_drops_only_the_rest() {
        let components = Tag::Compound(vec![
            (
                "minecraft:consumable".into(),
                Tag::Compound(vec![("consume_seconds".into(), Tag::Float(0.0))]),
            ),
            (
                "minecraft:use_remainder".into(),
                Tag::Compound(vec![
                    ("id".into(), Tag::String("minecraft:bowl".into())),
                    ("count".into(), Tag::Int(3)),
                ]),
            ),
        ]);
        let mut inventory = PlayerInventory::default();
        for slot in 1..36 {
            inventory.set_slot(
                PlayerSlot::Storage(slot),
                ItemStack::new("minecraft:dirt".into(), 64, 64, Tag::Compound(vec![])),
            );
        }
        inventory.set_slot(
            PlayerSlot::Storage(0),
            ItemStack::new("minecraft:apple".into(), 2, 64, components),
        );
        inventory.set_slot(
            PlayerSlot::Storage(1),
            ItemStack::new("minecraft:bowl".into(), 63, 64, Tag::Compound(vec![])),
        );
        let mut inventory = Arc::new(inventory);
        let mut vitals = PlayerVitals {
            food: 10,
            ..Default::default()
        };
        let mut usage = FoodUse::default();
        assert!(usage.start_hand(
            &mut inventory,
            PlayerSlot::Storage(0),
            &mut vitals,
            GameMode::Survival
        ));
        assert_eq!(inventory.slot(PlayerSlot::Storage(1)).unwrap().count(), 64);
        let drops = usage.take_drops();
        assert_eq!(drops.len(), 1);
        assert_eq!((drops[0].id(), drops[0].count()), ("minecraft:bowl", 2));
    }

    #[test]
    fn nonfood_item_requires_both_custom_components() {
        let food = Tag::Compound(vec![
            ("nutrition".into(), Tag::Int(3)),
            ("saturation".into(), Tag::Float(1.0)),
        ]);
        let consumable = Tag::Compound(vec![("consume_seconds".into(), Tag::Float(0.1))]);
        for (components, expected) in [
            (vec![("minecraft:food".into(), food.clone())], false),
            (
                vec![("minecraft:consumable".into(), consumable.clone())],
                false,
            ),
            (
                vec![
                    ("minecraft:food".into(), food),
                    ("minecraft:consumable".into(), consumable),
                ],
                true,
            ),
        ] {
            let components = Tag::Compound(components);
            assert_eq!(saved_item_max("minecraft:dirt", &components), Some(64));
            let item = ItemStack::new("minecraft:dirt".into(), 1, 64, components).unwrap();
            let mut inventory = PlayerInventory::default();
            inventory.set_slot(PlayerSlot::Storage(0), Some(item));
            let mut state = FoodUse::default();
            assert_eq!(
                state.start_hand(
                    &mut Arc::new(inventory),
                    PlayerSlot::Storage(0),
                    &mut PlayerVitals {
                        food: 10,
                        ..Default::default()
                    },
                    GameMode::Survival
                ),
                expected
            );
        }
    }

    #[test]
    fn removed_and_unsupported_food_components_do_not_use_defaults() {
        for components in [
            Tag::Compound(vec![("!minecraft:food".into(), Tag::Compound(vec![]))]),
            Tag::Compound(vec![(
                "!minecraft:consumable".into(),
                Tag::Compound(vec![]),
            )]),
            Tag::Compound(vec![(
                "minecraft:consumable".into(),
                Tag::Compound(vec![(
                    "on_consume_effects".into(),
                    Tag::List {
                        element_type: native_persistence::nbt::TagType::Compound,
                        values: vec![Tag::Compound(vec![])],
                    },
                )]),
            )]),
        ] {
            let item = ItemStack::new("minecraft:apple".into(), 1, 64, components).unwrap();
            let mut inventory = PlayerInventory::default();
            inventory.set_slot(PlayerSlot::Storage(0), Some(item));
            let mut state = FoodUse::default();
            assert!(!state.start_hand(
                &mut Arc::new(inventory),
                PlayerSlot::Storage(0),
                &mut PlayerVitals {
                    food: 1,
                    ..Default::default()
                },
                GameMode::Survival
            ));
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn custom_food_components_match_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("FoodComponentsOracle.java");
        let output = temp.path().join("food.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/FoodComponentsOracle.java"),
        )
        .unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg("FoodComponentsOracle")
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "Java oracle failed: {}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let result: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        for row in result["food"].as_array().unwrap() {
            let food = Tag::Compound(vec![
                (
                    "nutrition".into(),
                    Tag::Int(row["nutrition"].as_i64().unwrap() as i32),
                ),
                (
                    "saturation".into(),
                    Tag::Float(row["saturation"].as_f64().unwrap() as f32),
                ),
                (
                    "can_always_eat".into(),
                    Tag::Byte(i8::from(row["always"].as_bool().unwrap())),
                ),
            ]);
            let components = Tag::Compound(vec![
                ("minecraft:food".into(), food),
                (
                    "minecraft:consumable".into(),
                    Tag::Compound(vec![("consume_seconds".into(), Tag::Float(0.05))]),
                ),
            ]);
            let item = ItemStack::new("minecraft:apple".into(), 1, 64, components).unwrap();
            let mut inventory = PlayerInventory::default();
            inventory.set_slot(PlayerSlot::Storage(0), Some(item));
            let mut inventory = Arc::new(inventory);
            let mut vitals = PlayerVitals {
                food: 13,
                saturation: 1.25,
                ..Default::default()
            };
            let mut use_state = FoodUse::default();
            assert!(use_state.start_hand(
                &mut inventory,
                PlayerSlot::Storage(0),
                &mut vitals,
                GameMode::Survival
            ));
            use_state.tick(&mut inventory, &mut vitals, GameMode::Survival);
            assert_eq!(
                vitals.food,
                row["result_food"].as_i64().unwrap() as i32,
                "{row}"
            );
            assert_eq!(
                vitals.saturation,
                row["result_saturation"].as_f64().unwrap() as f32,
                "{row}"
            );
        }
        for row in result["times"].as_array().unwrap() {
            let component = Tag::Compound(vec![(
                "consume_seconds".into(),
                Tag::Float(row["seconds"].as_f64().unwrap() as f32),
            )]);
            assert_eq!(
                custom_consumable_ticks(&component),
                Some(row["ticks"].as_i64().unwrap() as i32),
                "{row}"
            );
        }
        for row in result["remainders"].as_array().unwrap() {
            let template = if row["template_count"].as_i64().unwrap() == 1 {
                Tag::String("minecraft:bowl".into())
            } else {
                Tag::Compound(vec![
                    ("id".into(), Tag::String("minecraft:bowl".into())),
                    ("count".into(), Tag::Int(3)),
                ])
            };
            let components = Tag::Compound(vec![
                (
                    "minecraft:consumable".into(),
                    Tag::Compound(vec![("consume_seconds".into(), Tag::Float(0.0))]),
                ),
                ("minecraft:use_remainder".into(), template),
            ]);
            let mut inventory = PlayerInventory::default();
            inventory.set_slot(
                PlayerSlot::Storage(0),
                ItemStack::new(
                    "minecraft:apple".into(),
                    row["before"].as_u64().unwrap() as u32,
                    64,
                    components,
                ),
            );
            let mut inventory = Arc::new(inventory);
            let mut vitals = PlayerVitals {
                food: 10,
                ..Default::default()
            };
            let mut usage = FoodUse::default();
            let mode = if row["creative"].as_bool().unwrap() {
                GameMode::Creative
            } else {
                GameMode::Survival
            };
            assert!(usage.start_hand(&mut inventory, PlayerSlot::Storage(0), &mut vitals, mode));
            let hand = inventory.slot(PlayerSlot::Storage(0)).unwrap();
            assert_eq!(hand.id(), row["hand_id"].as_str().unwrap(), "{row}");
            assert_eq!(
                hand.count(),
                row["hand_count"].as_u64().unwrap() as u32,
                "{row}"
            );
            let extra = inventory
                .slot(PlayerSlot::Storage(1))
                .map_or(0, ItemStack::count);
            assert_eq!(extra, row["extra_count"].as_u64().unwrap() as u32, "{row}");
            assert!(usage.take_drops().is_empty());
        }
    }

    #[test]
    fn holding_use_eats_another_item_until_released_or_full() {
        let mut session = LocalSession::demo();
        session.set_player_inventory(apple_inventory(PlayerSlot::Storage(0)));
        session.set_player_vitals(PlayerVitals {
            food: 10,
            saturation: 0.0,
            ..Default::default()
        });
        session.submit_command(PlayerCommand::UseHeld(true));
        session.submit_command(PlayerCommand::Place);
        for _ in 0..64 {
            session.update(Duration::from_millis(50));
        }
        assert_eq!(session.snapshot().vitals.food, 18);
        assert!(
            session
                .snapshot()
                .inventory
                .slot(PlayerSlot::Storage(0))
                .is_none()
        );
        session.submit_command(PlayerCommand::StopUse);
    }

    #[test]
    fn eating_and_survival_ticks_are_independent_of_frame_rate() {
        let run = |frame_ms: u64| {
            let mut session = LocalSession::demo();
            session.set_player_inventory(apple_inventory(PlayerSlot::Storage(0)));
            session.set_player_vitals(PlayerVitals {
                health: 18.0,
                food: 17,
                saturation: 0.0,
                ..PlayerVitals::default()
            });
            session.submit_command(PlayerCommand::Place);
            session.submit_command(PlayerCommand::Move {
                forward: 1.0,
                strafe: 0.0,
                jump: false,
            });
            for _ in 0..(5_000 / frame_ms) {
                session.update(Duration::from_millis(frame_ms));
            }
            session.snapshot()
        };
        let slow = run(100);
        let fast = run(5);
        assert_eq!(slow.tick, 100);
        assert_eq!(slow.tick, fast.tick);
        assert_eq!(slow.vitals, fast.vitals);
        assert_eq!(slow.player.position, fast.player.position);
        assert_eq!(
            slow.inventory.slot(PlayerSlot::Storage(0)),
            fast.inventory.slot(PlayerSlot::Storage(0))
        );
    }
}
