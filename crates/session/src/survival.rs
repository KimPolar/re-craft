//! Player health and the vanilla food-data tick.

use crate::inventory::{ItemStack, PlayerInventory, PlayerSlot, saved_item_max};
use native_persistence::world::Difficulty;
use std::sync::Arc;

/// Plain foods without effects or a container remainder. Other consumables must
/// wait until their item components and effects are implemented.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FoodProperties {
    pub nutrition: i32,
    pub saturation: f32,
    pub use_ticks: u8,
}

pub fn ordinary_food(id: &str) -> Option<FoodProperties> {
    let (nutrition, modifier) = match id.strip_prefix("minecraft:")? {
        "apple" => (4, 0.3),
        "baked_potato" | "bread" | "cooked_cod" | "cooked_rabbit" => (5, 0.6),
        "beef" | "porkchop" | "rabbit" => (3, 0.3),
        "beetroot" => (1, 0.6),
        "carrot" => (3, 0.6),
        "cod" | "salmon" | "cookie" | "sweet_berries" | "glow_berries" => (2, 0.1),
        "cooked_beef" | "cooked_porkchop" => (8, 0.8),
        "cooked_chicken" => (6, 0.6),
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
    })
}

#[derive(Clone, Debug)]
struct Eating {
    hand: PlayerSlot,
    item: ItemStack,
    remaining: u8,
    food: FoodProperties,
}

#[derive(Default)]
pub struct FoodUse {
    active: Option<Eating>,
    held: bool,
    repeat: bool,
}

impl FoodUse {
    pub fn is_active(&self) -> bool {
        self.active.is_some()
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
        inventory: &PlayerInventory,
        hand: PlayerSlot,
        vitals: PlayerVitals,
        mode: crate::GameMode,
    ) -> bool {
        let Some(item) = inventory.slot(hand) else {
            return false;
        };
        let Some(food) = ordinary_food(item.id()) else {
            return false;
        };
        if saved_item_max(item.id(), item.components()) != item.max_count()
            || item.max_count().is_none()
        {
            return false;
        }
        if mode != crate::GameMode::Spectator
            && (vitals.food < 20 || mode == crate::GameMode::Creative)
        {
            self.active = Some(Eating {
                hand,
                item: item.clone(),
                remaining: food.use_ticks,
                food,
            });
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
        inventory: &PlayerInventory,
        vitals: PlayerVitals,
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
        self.prepare(inventory, *vitals, mode);
        let Some(active) = &mut self.active else {
            return;
        };
        active.remaining = active.remaining.saturating_sub(1);
        if active.remaining != 0 {
            return;
        }
        if mode == crate::GameMode::Creative || Arc::make_mut(inventory).consume_held(active.hand) {
            vitals.food = vitals.food.wrapping_add(active.food.nutrition).clamp(0, 20);
            vitals.saturation =
                (vitals.saturation + active.food.saturation).clamp(0.0, vitals.food as f32);
        }
        self.active = None;
        self.repeat = self.held;
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

/// Active effects needed for fall damage. The hidden chain ages with the
/// visible effect, as it does for the other living-entity effects.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FallEffects {
    pub jump_boost: Option<EffectDuration>,
    pub resistance: Option<EffectDuration>,
    pub slow_falling: Option<EffectDuration>,
    pub levitation: Option<EffectDuration>,
    pub speed: Option<EffectDuration>,
    pub slowness: Option<EffectDuration>,
}

impl FallEffects {
    pub fn tick(&mut self) {
        EffectDuration::tick(&mut self.jump_boost);
        EffectDuration::tick(&mut self.resistance);
        EffectDuration::tick(&mut self.slow_falling);
        EffectDuration::tick(&mut self.levitation);
        EffectDuration::tick(&mut self.speed);
        EffectDuration::tick(&mut self.slowness);
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
        if self.health <= 0.0 {
            return;
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
                    let absorbed = self.absorption.clamp(0.0, 2.0);
                    self.absorption -= absorbed;
                    self.health = (self.health - (2.0 - absorbed)).max(0.0);
                }
            }
        } else if self.air < 300 {
            self.air = self.air.wrapping_add(4).min(300);
        }
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
            &inventory,
            PlayerSlot::Storage(0),
            PlayerVitals {
                food: 1,
                ..PlayerVitals::default()
            },
            GameMode::Survival
        ));
        assert!(!use_state.is_active());
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
