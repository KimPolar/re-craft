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
        "poisonous_potato" => (2, 0.3),
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
        use_ticks: if id == "minecraft:dried_kelp" { 16 } else { 32 },
        can_always_eat: false,
        consume_particles: true,
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
struct CustomConsumable {
    ticks: i32,
    particles: bool,
    effects: Vec<StatusGroup>,
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
            id = Some(
                if value.matches("minecraft:hunger") || value.matches("hunger") {
                    FoodStatusKind::Hunger
                } else if value.matches("minecraft:poison") || value.matches("poison") {
                    FoodStatusKind::Poison
                } else {
                    return None;
                },
            );
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
                let group = parse_status_group(value, collect)?;
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
    matches!(
        item.id(),
        "minecraft:mushroom_stew" | "minecraft:rabbit_stew" | "minecraft:beetroot_soup"
    )
    .then(|| ItemStack::new("minecraft:bowl".into(), 1, 64, Tag::Compound(vec![])))
    .flatten()
}

pub fn food_properties(item: &ItemStack) -> Option<FoodProperties> {
    let components = item.components();
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
    })
}

#[derive(Clone, Debug)]
struct Eating {
    hand: PlayerSlot,
    item: ItemStack,
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
    ParticlePulse(u8),
    Finish {
        causes_hunger: bool,
        particles: bool,
    },
    StatusGroup {
        probability: f32,
        effects: Vec<StatusApplication>,
    },
}

impl FoodRandomEvent {
    /// Advance the player's stream for the sound and particle calculations
    /// Java performs even when their visual output is not yet rendered.
    pub(crate) fn apply(&self, random: &mut native_core::random::LegacyRandom) -> bool {
        let particles = match self {
            Self::ParticlePulse(count) => *count,
            Self::Finish { particles, .. } => {
                if *particles {
                    16
                } else {
                    0
                }
            }
            Self::StatusGroup { probability, .. } => return random.next_float() < *probability,
        };
        random.next_bool();
        for _ in 0..3 + u32::from(particles) * 4 {
            random.next_float();
        }
        if let Self::Finish { causes_hunger, .. } = self {
            for _ in 0..3 {
                random.next_float();
            }
            return *causes_hunger && random.next_float() < 0.8;
        }
        false
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
                    item: item.clone(),
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
            self.events.push(FoodRandomEvent::ParticlePulse(
                if active.food.consume_particles { 5 } else { 0 },
            ));
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
            && let Some(probability) = match item.id() {
                "minecraft:poisonous_potato" => Some(0.6),
                "minecraft:spider_eye" => Some(1.0),
                _ => None,
            }
        {
            custom_groups.push(StatusGroup {
                probability,
                effects: vec![StatusApplication {
                    kind: FoodStatusKind::Poison,
                    duration: 100,
                    amplifier: 0,
                    ambient: false,
                    visible: true,
                    show_icon: true,
                }],
            });
        }
        if mode == crate::GameMode::Creative || Arc::make_mut(inventory).consume_held(hand) {
            self.events.push(FoodRandomEvent::Finish {
                causes_hunger,
                particles: food.consume_particles,
            });
            self.events.extend(custom_groups.into_iter().map(|group| {
                FoodRandomEvent::StatusGroup {
                    probability: group.probability,
                    effects: group.effects,
                }
            }));
            vitals.food = vitals.food.wrapping_add(food.nutrition).clamp(0, 20);
            vitals.saturation =
                (vitals.saturation + food.saturation).clamp(0.0, vitals.food as f32);
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
    pub hidden: Option<Box<EffectDuration>>,
}

impl EffectDuration {
    fn shorter_than(&self, other: &Self) -> bool {
        self.duration != -1 && (other.duration == -1 || self.duration < other.duration)
    }

    /// Merge the timing and amplifier of a matching Java effect instance.
    pub(crate) fn merge(&mut self, incoming: Self) {
        if incoming.amplifier > self.amplifier {
            if incoming.shorter_than(self) {
                let old = self.clone();
                self.hidden = Some(Box::new(old));
            }
            self.amplifier = incoming.amplifier;
            self.duration = incoming.duration;
        } else if self.shorter_than(&incoming) {
            if incoming.amplifier == self.amplifier {
                self.duration = incoming.duration;
            } else if let Some(hidden) = &mut self.hidden {
                hidden.merge(incoming);
            } else {
                self.hidden = Some(Box::new(incoming));
            }
        }
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

impl BreathingEffects {
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

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn poison_food_matches_java() {
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
        let source = temp.path().join("PoisonFoodOracle.java");
        let output = temp.path().join("food.json");
        std::fs::write(&source, include_str!("../tests/java/PoisonFoodOracle.java")).unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg("PoisonFoodOracle")
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
        assert_eq!(rows.len(), 200);
        for row in rows {
            let id = format!("minecraft:{}", row["item"].as_str().unwrap());
            let item = ItemStack::new(id, 1, 64, Tag::Compound(vec![])).unwrap();
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
            for _ in 0..32 {
                usage.tick(&mut inventory, &mut vitals, GameMode::Survival);
                for event in usage.take_events() {
                    if event.apply(&mut random)
                        && let FoodRandomEvent::StatusGroup { effects, .. } = event
                    {
                        assert_eq!(effects.len(), 1);
                        applied = Some(effects[0]);
                    }
                }
            }
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
                assert_eq!(effect.kind, FoodStatusKind::Poison);
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
                duration: row["initial_duration"].as_i64().unwrap() as i32,
                amplifier: row["initial_amplifier"].as_u64().unwrap() as u8,
                hidden: None,
            };
            effect.merge(EffectDuration {
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
                    FoodRandomEvent::ParticlePulse(5).apply(&mut random);
                }
                let expected = FoodRandomEvent::Finish {
                    causes_hunger: !overridden,
                    particles: true,
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
    fn rotten_flesh_outcome_is_independent_of_frame_cadence() {
        let run = |frame_ms: u64| {
            let mut session = LocalSession::demo();
            session.player_random = native_core::random::LegacyRandom::new(99);
            let mut inventory = PlayerInventory::default();
            inventory.set_slot(
                PlayerSlot::Storage(0),
                ItemStack::new(
                    "minecraft:rotten_flesh".into(),
                    2,
                    64,
                    Tag::Compound(vec![]),
                ),
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
        let (slow, slow_effect, slow_random) = run(100);
        let (fast, fast_effect, fast_random) = run(5);
        assert_eq!(slow.tick, fast.tick);
        assert_eq!(slow.vitals, fast.vitals);
        assert_eq!(
            slow.inventory.slot(PlayerSlot::Storage(0)),
            fast.inventory.slot(PlayerSlot::Storage(0))
        );
        assert_eq!(slow_effect, fast_effect);
        assert_eq!(slow_random, fast_random);
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
        ];
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), 160);
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
            let mut active: Option<EffectDuration> = None;
            let mut active_poison: Option<EffectDuration> = None;
            for event in usage.take_events() {
                if event.apply(&mut random)
                    && let FoodRandomEvent::StatusGroup { effects, .. } = event
                {
                    for effect in effects {
                        let incoming = EffectDuration {
                            duration: effect.duration,
                            amplifier: effect.amplifier,
                            hidden: None,
                        };
                        let target = match effect.kind {
                            FoodStatusKind::Hunger => &mut active,
                            FoodStatusKind::Poison => &mut active_poison,
                        };
                        if let Some(existing) = target {
                            existing.merge(incoming);
                        } else {
                            *target = Some(incoming);
                        }
                    }
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
