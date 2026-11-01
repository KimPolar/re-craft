//! Player fall damage using 26.3 attributes, blocks, effects and pack data.

use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use native_persistence::nbt::Tag;
use native_resources::{
    enchantments::{AttributeModifier, AttributeOperation, EquipmentSlot, OxygenEnchantments},
    packs::ResourceId,
};
use native_world::loaded::BlockState;

use crate::{
    GameMode,
    equipment_attributes::{ArmorValues, equipped_attribute},
    inventory::{PlayerInventory, PlayerSlot},
    player_data::SavedAttribute,
    survival::{FallEffects, PlayerVitals, SurvivalRules},
};

// Sound types are properties of block code, while resource packs supply the
// sound event itself. Leave unimplemented block types unresolved.
pub(crate) fn block_fall_sound(id: &str) -> Option<&'static str> {
    use native_core::wood::{Planks, Wood};
    if let Some(planks) = Planks::from_id(id) {
        return Some(match planks {
            Planks::Cherry => "block.cherry_wood.fall",
            Planks::Bamboo => "block.bamboo_wood.fall",
            Planks::Crimson | Planks::Warped => "block.nether_wood.fall",
            _ => "block.wood.fall",
        });
    }
    if Wood::from_id(id).is_some() {
        let name = id.strip_prefix("minecraft:")?;
        let name = name.strip_prefix("stripped_").unwrap_or(name);
        return Some(if name.starts_with("cherry_") {
            "block.cherry_wood.fall"
        } else if name.starts_with("bamboo_") {
            "block.bamboo_wood.fall"
        } else if name.starts_with("crimson_") || name.starts_with("warped_") {
            "block.stem.fall"
        } else {
            "block.wood.fall"
        });
    }
    if native_core::ore::Ore::from_id(id).is_some() {
        return Some(if id.starts_with("minecraft:deepslate_") {
            "block.deepslate.fall"
        } else if id == "minecraft:nether_gold_ore" {
            "block.nether_gold_ore.fall"
        } else if id == "minecraft:nether_quartz_ore" {
            "block.nether_ore.fall"
        } else {
            "block.stone.fall"
        });
    }
    match id {
        "minecraft:stone"
        | "minecraft:cobblestone"
        | "minecraft:stone_bricks"
        | "minecraft:granite"
        | "minecraft:diorite"
        | "minecraft:andesite"
        | "minecraft:end_stone"
        | "minecraft:sandstone"
        | "minecraft:red_sandstone"
        | "minecraft:terracotta"
        | "minecraft:blackstone"
        | "minecraft:obsidian" => Some("block.stone.fall"),
        "minecraft:deepslate" | "minecraft:cobbled_deepslate" => Some("block.deepslate.fall"),
        "minecraft:netherrack" => Some("block.netherrack.fall"),
        "minecraft:nether_bricks" => Some("block.nether_bricks.fall"),
        "minecraft:basalt" => Some("block.basalt.fall"),
        "minecraft:snow_block" => Some("block.snow.fall"),
        "minecraft:ice" | "minecraft:glass" => Some("block.glass.fall"),
        "minecraft:moss_block" => Some("block.moss.fall"),
        "minecraft:dirt"
        | "minecraft:coarse_dirt"
        | "minecraft:podzol"
        | "minecraft:farmland"
        | "minecraft:gravel" => Some("block.gravel.fall"),
        "minecraft:rooted_dirt" => Some("block.rooted_dirt.fall"),
        "minecraft:grass_block"
        | "minecraft:mycelium"
        | "minecraft:dirt_path"
        | "minecraft:hay_block" => Some("block.grass.fall"),
        "minecraft:sand" | "minecraft:red_sand" => Some("block.sand.fall"),
        "minecraft:crafting_table" => Some("block.wood.fall"),
        "minecraft:honey_block" => Some("block.honey_block.fall"),
        _ => None,
    }
}

/// The `LivingEntity.calculateFallDamage` calculation before damage mitigation.
pub fn calculate_fall_damage(
    distance: f64,
    safe_distance: f64,
    block_multiplier: f32,
    attribute_multiplier: f64,
) -> i32 {
    let value =
        (distance + 1.0e-6 - safe_distance) * f64::from(block_multiplier) * attribute_multiplier;
    // Java Mth.floor first narrows to an int, then subtracts one when needed.
    let narrowed = value as i32;
    if value < f64::from(narrowed) {
        narrowed.wrapping_sub(1)
    } else {
        narrowed
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct FallSurface {
    distance_multiplier: f64,
    added_distance: f64,
    damage_multiplier: f32,
    damage_type: &'static str,
}

impl Default for FallSurface {
    fn default() -> Self {
        Self {
            distance_multiplier: 1.0,
            added_distance: 0.0,
            damage_multiplier: 1.0,
            damage_type: "minecraft:fall",
        }
    }
}

/// The server player's transient damage cooldown and the unmitigated amount of
/// the last accepted hit. These values are not the saved HurtTime animation.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct DamageCooldown {
    ticks: i32,
    last_damage: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct DamageOutcome {
    pub accepted: bool,
    pub fresh_hit: bool,
    pub health_damage: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct LandingOutcome {
    pub amount: i32,
    pub source: Option<&'static str>,
    pub damage: DamageOutcome,
}

impl DamageCooldown {
    pub(crate) fn tick(&mut self) {
        if self.ticks > 0 {
            self.ticks -= 1;
        }
    }

    fn accept(&mut self, amount: f32, bypass: bool) -> Option<f32> {
        // Player.hurtServer rejects zero before LivingEntity normalizes damage.
        if amount == 0.0 {
            return None;
        }
        let amount = if amount < 0.0 {
            0.0
        } else if amount.is_finite() {
            amount
        } else {
            f32::MAX
        };
        if self.ticks > 10 && !bypass {
            if amount <= self.last_damage {
                return None;
            }
            let difference = amount - self.last_damage;
            self.last_damage = amount;
            Some(difference)
        } else {
            self.last_damage = amount;
            self.ticks = 20;
            Some(amount)
        }
    }
}

impl FallSurface {
    fn for_state(state: Option<&BlockState>) -> Self {
        let Some(state) = state else {
            return Self::default();
        };
        let mut surface = Self::default();
        match state.name.as_str() {
            "minecraft:hay_block" | "minecraft:honey_block" => surface.damage_multiplier = 0.2_f32,
            "minecraft:slime_block" | "minecraft:powder_snow" => surface.damage_multiplier = 0.0,
            "minecraft:shelf_mushroom" => surface.distance_multiplier = 0.5,
            name if name.starts_with("minecraft:")
                && name.ends_with("_bed")
                && name != "minecraft:straw_bed" =>
            {
                surface.distance_multiplier = 0.5
            }
            "minecraft:pointed_dripstone"
                if state
                    .properties
                    .get("vertical_direction")
                    .is_some_and(|v| v == "up")
                    && state
                        .properties
                        .get("thickness")
                        .is_some_and(|v| v == "tip") =>
            {
                surface.added_distance = 2.5;
                surface.damage_multiplier = 2.0;
                surface.damage_type = "minecraft:stalagmite";
            }
            _ => {}
        }
        surface
    }
}

pub(crate) struct FallTracker {
    safe: SavedAttribute,
    multiplier: SavedAttribute,
    effects: FallEffects,
    enchantments: Option<Arc<OxygenEnchantments>>,
}

impl Default for FallTracker {
    fn default() -> Self {
        Self {
            safe: SavedAttribute::with_base(3.0),
            multiplier: SavedAttribute::with_base(1.0),
            effects: FallEffects::default(),
            enchantments: None,
        }
    }
}

impl FallTracker {
    pub(crate) fn set_enchantments(&mut self, enchantments: Arc<OxygenEnchantments>) {
        self.enchantments = Some(enchantments);
    }
    pub(crate) fn effects(&self) -> &FallEffects {
        &self.effects
    }

    pub(crate) fn safe_attribute(&self) -> &SavedAttribute {
        &self.safe
    }

    fn refresh_jump_boost_modifier(&mut self) {
        let id = ResourceId::parse("minecraft:effect.jump_boost").unwrap();
        self.safe.remove_modifier(&id);
        if let Some(effect) = &self.effects.jump_boost {
            self.safe.apply(AttributeModifier {
                id,
                operation: AttributeOperation::AddValue,
                amount: f64::from(effect.amplifier) + 1.0,
            });
        }
    }

    pub(crate) fn clear_effects(&mut self) {
        let jump = self.effects.jump_boost.is_some();
        self.effects = FallEffects::default();
        if jump {
            self.refresh_jump_boost_modifier();
        }
    }

    pub(crate) fn remove_status(&mut self, kind: crate::survival::FoodStatusKind) {
        let jump =
            kind == crate::survival::FoodStatusKind::JumpBoost && self.effects.jump_boost.is_some();
        match kind {
            crate::survival::FoodStatusKind::Hunger => self.effects.hunger = None,
            crate::survival::FoodStatusKind::Poison => self.effects.poison = None,
            crate::survival::FoodStatusKind::NightVision => self.effects.night_vision = None,
            crate::survival::FoodStatusKind::FireResistance => self.effects.fire_resistance = None,
            crate::survival::FoodStatusKind::Regeneration => self.effects.regeneration = None,
            crate::survival::FoodStatusKind::JumpBoost => self.effects.jump_boost = None,
            crate::survival::FoodStatusKind::SlowFalling => self.effects.slow_falling = None,
            crate::survival::FoodStatusKind::Speed => self.effects.speed = None,
            crate::survival::FoodStatusKind::Slowness => self.effects.slowness = None,
            crate::survival::FoodStatusKind::Resistance => self.effects.resistance = None,
            crate::survival::FoodStatusKind::Levitation => self.effects.levitation = None,
            crate::survival::FoodStatusKind::DolphinsGrace => self.effects.dolphins_grace = None,
            crate::survival::FoodStatusKind::WaterBreathing
            | crate::survival::FoodStatusKind::ConduitPower
            | crate::survival::FoodStatusKind::BreathOfTheNautilus => {
                unreachable!("breathing effects are handled separately")
            }
        }
        if jump {
            self.refresh_jump_boost_modifier();
        }
    }

    pub(crate) fn add_hunger(&mut self, duration: i32, amplifier: u8) {
        self.add_status(crate::survival::StatusApplication {
            kind: crate::survival::FoodStatusKind::Hunger,
            duration,
            amplifier,
            ambient: false,
            visible: true,
            show_icon: true,
        });
    }

    pub(crate) fn add_status(&mut self, application: crate::survival::StatusApplication) -> bool {
        let incoming = crate::survival::EffectDuration::from_application(application);
        let slot = match application.kind {
            crate::survival::FoodStatusKind::Hunger => &mut self.effects.hunger,
            crate::survival::FoodStatusKind::Poison => &mut self.effects.poison,
            crate::survival::FoodStatusKind::NightVision => &mut self.effects.night_vision,
            crate::survival::FoodStatusKind::FireResistance => &mut self.effects.fire_resistance,
            crate::survival::FoodStatusKind::Regeneration => &mut self.effects.regeneration,
            crate::survival::FoodStatusKind::JumpBoost => &mut self.effects.jump_boost,
            crate::survival::FoodStatusKind::SlowFalling => &mut self.effects.slow_falling,
            crate::survival::FoodStatusKind::Speed => &mut self.effects.speed,
            crate::survival::FoodStatusKind::Slowness => &mut self.effects.slowness,
            crate::survival::FoodStatusKind::Resistance => &mut self.effects.resistance,
            crate::survival::FoodStatusKind::Levitation => &mut self.effects.levitation,
            crate::survival::FoodStatusKind::DolphinsGrace => &mut self.effects.dolphins_grace,
            crate::survival::FoodStatusKind::WaterBreathing
            | crate::survival::FoodStatusKind::ConduitPower
            | crate::survival::FoodStatusKind::BreathOfTheNautilus => {
                unreachable!("breathing effects are handled separately")
            }
        };
        let changed = if let Some(effect) = slot {
            effect.merge(incoming)
        } else {
            *slot = Some(incoming);
            true
        };
        if changed && application.kind == crate::survival::FoodStatusKind::JumpBoost {
            self.refresh_jump_boost_modifier();
        }
        changed
    }

    pub fn reset_after_death(&mut self) {
        self.safe.clear_modifiers();
        self.multiplier.clear_modifiers();
        self.effects = FallEffects::default();
    }

    pub fn configure(
        &mut self,
        safe: SavedAttribute,
        multiplier: SavedAttribute,
        effects: FallEffects,
        enchantments: Arc<OxygenEnchantments>,
    ) {
        self.safe = safe;
        self.multiplier = multiplier;
        self.effects = effects;
        self.enchantments = Some(enchantments);
    }

    pub fn tick_effects(&mut self) {
        let transition = self
            .effects
            .jump_boost
            .as_ref()
            .is_some_and(|effect| effect.duration != -1 && effect.duration <= 1);
        self.effects.tick();
        if transition {
            self.refresh_jump_boost_modifier();
        }
    }

    fn damage_tag(&self, tag: &str, damage: &str) -> Result<bool> {
        if let Some(registry) = &self.enchantments {
            registry.damage_type_has_tag(tag, damage)
        } else {
            // The demo has no data packs and uses the built-in damage tags.
            Ok(match tag {
                "minecraft:is_fall" => matches!(damage, "minecraft:fall" | "minecraft:stalagmite"),
                "minecraft:bypasses_armor" => matches!(
                    damage,
                    "minecraft:fall"
                        | "minecraft:stalagmite"
                        | "minecraft:drown"
                        | "minecraft:starve"
                        | "minecraft:magic"
                ),
                "minecraft:bypasses_effects" => damage == "minecraft:starve",
                "minecraft:is_fire" => matches!(
                    damage,
                    "minecraft:in_fire"
                        | "minecraft:campfire"
                        | "minecraft:on_fire"
                        | "minecraft:lava"
                        | "minecraft:hot_floor"
                        | "minecraft:sulfur_cube_hot"
                        | "minecraft:unattributed_fireball"
                        | "minecraft:fireball"
                ),
                "minecraft:no_knockback" => matches!(
                    damage,
                    "minecraft:in_fire"
                        | "minecraft:campfire"
                        | "minecraft:on_fire"
                        | "minecraft:lava"
                        | "minecraft:hot_floor"
                        | "minecraft:fall"
                        | "minecraft:stalagmite"
                        | "minecraft:drown"
                        | "minecraft:starve"
                        | "minecraft:magic"
                ),
                _ => false,
            })
        }
    }

    pub(crate) fn blocks_damage_knockback(&self, damage: &str) -> Result<bool> {
        self.damage_tag("minecraft:no_knockback", damage)
    }

    fn immune_to_damage(&self, inventory: &PlayerInventory, damage: &str) -> Result<bool> {
        let Some(registry) = &self.enchantments else {
            return Ok(false);
        };
        for (slot, player_slot) in [
            (
                EquipmentSlot::Mainhand,
                PlayerSlot::Storage(inventory.selected()),
            ),
            (EquipmentSlot::Offhand, PlayerSlot::Offhand),
            (EquipmentSlot::Feet, PlayerSlot::Armor(3)),
            (EquipmentSlot::Legs, PlayerSlot::Armor(2)),
            (EquipmentSlot::Chest, PlayerSlot::Armor(1)),
            (EquipmentSlot::Head, PlayerSlot::Armor(0)),
        ] {
            let Some(item) = inventory.slot(player_slot) else {
                continue;
            };
            let Some(enchantments) = item.components().get("minecraft:enchantments") else {
                continue;
            };
            let Tag::Compound(entries) = enchantments else {
                anyhow::bail!("item enchantments are not a compound")
            };
            for (id, level) in entries {
                let id = ResourceId::parse(
                    &String::from_utf16(&id.0).context("invalid enchantment id")?,
                )?;
                let level =
                    u32::try_from(level.as_java_int().context("invalid enchantment level")?)?;
                if registry.prevents_damage(&id, level, slot, damage)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn protection(&self, inventory: &PlayerInventory, damage: &str) -> Result<f32> {
        let Some(registry) = &self.enchantments else {
            return Ok(0.0);
        };
        let slots = [
            (
                EquipmentSlot::Mainhand,
                PlayerSlot::Storage(inventory.selected()),
            ),
            (EquipmentSlot::Offhand, PlayerSlot::Offhand),
            (EquipmentSlot::Feet, PlayerSlot::Armor(3)),
            (EquipmentSlot::Legs, PlayerSlot::Armor(2)),
            (EquipmentSlot::Chest, PlayerSlot::Armor(1)),
            (EquipmentSlot::Head, PlayerSlot::Armor(0)),
        ];
        let mut total = 0.0;
        for (slot, player_slot) in slots {
            let Some(item) = inventory.slot(player_slot) else {
                continue;
            };
            let Some(enchantments) = item.components().get("minecraft:enchantments") else {
                continue;
            };
            let Tag::Compound(entries) = enchantments else {
                anyhow::bail!("item enchantments are not a compound")
            };
            for (id, level) in entries {
                let id = ResourceId::parse(
                    &String::from_utf16(&id.0).context("invalid enchantment id")?,
                )?;
                let level =
                    u32::try_from(level.as_java_int().context("invalid enchantment level")?)?;
                total = registry.protect_damage(&id, level, slot, damage, total)?;
            }
        }
        Ok(total)
    }

    /// Apply damage on landing. Unsupported pack predicates produce an error
    /// rather than silently applying a guessed amount of damage.
    #[allow(clippy::too_many_arguments)]
    pub fn land(
        &self,
        distance: f64,
        state: Option<&BlockState>,
        inventory: &PlayerInventory,
        armor: Option<ArmorValues>,
        vitals: &mut PlayerVitals,
        mode: GameMode,
        rules: SurvivalRules,
    ) -> Result<()> {
        self.land_impl(distance, state, inventory, armor, vitals, mode, rules, None)
            .map(|_| ())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn land_with_cooldown(
        &self,
        distance: f64,
        state: Option<&BlockState>,
        inventory: &PlayerInventory,
        armor: Option<ArmorValues>,
        vitals: &mut PlayerVitals,
        mode: GameMode,
        rules: SurvivalRules,
        cooldown: &mut DamageCooldown,
    ) -> Result<LandingOutcome> {
        self.land_impl(
            distance,
            state,
            inventory,
            armor,
            vitals,
            mode,
            rules,
            Some(cooldown),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn land_impl(
        &self,
        distance: f64,
        state: Option<&BlockState>,
        inventory: &PlayerInventory,
        armor: Option<ArmorValues>,
        vitals: &mut PlayerVitals,
        mode: GameMode,
        rules: SurvivalRules,
        cooldown: Option<&mut DamageCooldown>,
    ) -> Result<LandingOutcome> {
        let (mut landing, apply_damage) =
            self.landing_target(distance, state, inventory, vitals, mode, rules)?;
        if apply_damage {
            landing.damage = self.damage_outcome(
                landing.amount as f32,
                landing.source.expect("damage has a source"),
                inventory,
                armor,
                vitals,
                mode,
                cooldown,
            )?;
        }
        Ok(landing)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn land_with_equipment(
        &self,
        distance: f64,
        state: Option<&BlockState>,
        inventory: &mut Arc<PlayerInventory>,
        armor: &mut crate::equipment_attributes::ArmorTracker,
        vitals: &mut PlayerVitals,
        mode: GameMode,
        rules: SurvivalRules,
        cooldown: &mut DamageCooldown,
        random: &mut native_core::random::LegacyRandom,
        item_registry: Option<&native_resources::item_registry::ItemRegistry>,
    ) -> Result<LandingOutcome> {
        let (mut landing, apply_damage) =
            self.landing_target(distance, state, inventory, vitals, mode, rules)?;
        if apply_damage {
            landing.damage = self.damage_with_equipment(
                landing.amount as f32,
                landing.source.expect("damage has a source"),
                Arc::make_mut(inventory),
                armor,
                vitals,
                mode,
                cooldown,
                random,
                item_registry,
            )?;
        }
        Ok(landing)
    }

    fn landing_target(
        &self,
        distance: f64,
        state: Option<&BlockState>,
        inventory: &PlayerInventory,
        vitals: &PlayerVitals,
        mode: GameMode,
        rules: SurvivalRules,
    ) -> Result<(LandingOutcome, bool)> {
        if distance <= 0.0
            || vitals.health <= 0.0
            || matches!(mode, GameMode::Creative | GameMode::Spectator)
        {
            return Ok((LandingOutcome::default(), false));
        }
        if let Some(registry) = &self.enchantments
            && registry.fall_damage_immune("minecraft:player")?
        {
            return Ok((LandingOutcome::default(), false));
        }
        let surface = FallSurface::for_state(state);
        let safe = self.safe.clone();
        let (safe, multiplier) = if let Some(registry) = &self.enchantments {
            (
                equipped_attribute(&safe, inventory, registry, "safe_fall_distance")?
                    .value(-1024.0, 1024.0)?,
                equipped_attribute(
                    &self.multiplier,
                    inventory,
                    registry,
                    "fall_damage_multiplier",
                )?
                .value(0.0, 100.0)?,
            )
        } else {
            (
                safe.value(-1024.0, 1024.0)?,
                self.multiplier.value(0.0, 100.0)?,
            )
        };
        let distance = distance * surface.distance_multiplier + surface.added_distance;
        let amount = calculate_fall_damage(distance, safe, surface.damage_multiplier, multiplier);
        if amount <= 0 {
            return Ok((LandingOutcome::default(), false));
        }
        let damage = surface.damage_type;
        if !rules.fall_damage && self.damage_tag("minecraft:is_fall", damage)? {
            return Ok((
                LandingOutcome {
                    amount,
                    source: Some(damage),
                    ..Default::default()
                },
                false,
            ));
        }
        Ok((
            LandingOutcome {
                amount,
                source: Some(damage),
                ..Default::default()
            },
            true,
        ))
    }

    /// Apply one supported damage source; unsupported tag or enchantment data
    /// leaves both health and the cooldown unchanged.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn damage(
        &self,
        amount: f32,
        damage: &str,
        inventory: &PlayerInventory,
        armor: Option<ArmorValues>,
        vitals: &mut PlayerVitals,
        mode: GameMode,
        cooldown: Option<&mut DamageCooldown>,
    ) -> Result<()> {
        self.damage_outcome(amount, damage, inventory, armor, vitals, mode, cooldown)
            .map(|_| ())
    }

    pub(crate) fn damage_info(
        &self,
        damage: &str,
    ) -> Result<native_resources::enchantments::DamageTypeInfo> {
        if let Some(registry) = &self.enchantments {
            return registry.damage_type_info(damage);
        }
        use native_resources::enchantments::DamageEffects;
        let (exhaustion, effects) = match damage {
            "minecraft:lava"
            | "minecraft:in_fire"
            | "minecraft:campfire"
            | "minecraft:hot_floor" => (0.1, DamageEffects::Burning),
            "minecraft:on_fire" => (0.0, DamageEffects::Burning),
            "minecraft:fall" | "minecraft:stalagmite" | "minecraft:starve" | "minecraft:magic" => {
                (0.0, DamageEffects::Hurt)
            }
            "minecraft:drown" => (0.0, DamageEffects::Drowning),
            _ => anyhow::bail!("damage type definition is unavailable"),
        };
        Ok(native_resources::enchantments::DamageTypeInfo {
            exhaustion,
            effects,
        })
    }

    /// Stage equipment, health, cooldown and level RNG together so unsupported
    /// item or pack data cannot leave a partially applied hit.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn damage_with_equipment(
        &self,
        amount: f32,
        damage: &str,
        inventory: &mut PlayerInventory,
        armor: &mut crate::equipment_attributes::ArmorTracker,
        vitals: &mut PlayerVitals,
        mode: GameMode,
        cooldown: &mut DamageCooldown,
        random: &mut native_core::random::LegacyRandom,
        item_registry: Option<&native_resources::item_registry::ItemRegistry>,
    ) -> Result<DamageOutcome> {
        let Some((accepted_amount, fresh_hit, next_cooldown)) =
            self.prepare_damage(amount, damage, inventory, vitals, mode, Some(*cooldown))?
        else {
            return Ok(DamageOutcome::default());
        };
        if self.damage_tag("minecraft:bypasses_armor", damage)? {
            let outcome = self.finish_damage(
                accepted_amount,
                damage,
                inventory,
                armor.value(),
                vitals,
                fresh_hit,
            )?;
            *cooldown = next_cooldown.expect("cooldown was provided");
            return Ok(outcome);
        }
        let mut staged_inventory = inventory.clone();
        let mut staged_random = random.clone();
        staged_inventory.damage_equipped_armor(
            accepted_amount,
            damage,
            &mut staged_random,
            self.enchantments.as_deref(),
            item_registry,
        )?;
        let changed = staged_inventory.revision() != inventory.revision();
        let staged_armor = if changed {
            Some(armor.value_for(&staged_inventory)?)
        } else {
            armor.value()
        };
        let mut final_vitals = *vitals;
        let result = self.finish_damage(
            accepted_amount,
            damage,
            &staged_inventory,
            staged_armor,
            &mut final_vitals,
            fresh_hit,
        )?;
        *inventory = staged_inventory;
        *random = staged_random;
        *vitals = final_vitals;
        *cooldown = next_cooldown.expect("cooldown was provided");
        if changed {
            armor.refresh(inventory)?;
        }
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn damage_outcome(
        &self,
        amount: f32,
        damage: &str,
        inventory: &PlayerInventory,
        armor: Option<ArmorValues>,
        vitals: &mut PlayerVitals,
        mode: GameMode,
        cooldown: Option<&mut DamageCooldown>,
    ) -> Result<DamageOutcome> {
        let Some((amount, fresh_hit, next_cooldown)) = self.prepare_damage(
            amount,
            damage,
            inventory,
            vitals,
            mode,
            cooldown.as_deref().copied(),
        )?
        else {
            return Ok(DamageOutcome::default());
        };
        let result = self.finish_damage(amount, damage, inventory, armor, vitals, fresh_hit)?;
        if let (Some(cooldown), Some(next)) = (cooldown, next_cooldown) {
            *cooldown = next;
        }
        Ok(result)
    }

    fn prepare_damage(
        &self,
        amount: f32,
        damage: &str,
        inventory: &PlayerInventory,
        vitals: &PlayerVitals,
        mode: GameMode,
        mut next_cooldown: Option<DamageCooldown>,
    ) -> Result<Option<(f32, bool, Option<DamageCooldown>)>> {
        if vitals.health <= 0.0 || matches!(mode, GameMode::Creative | GameMode::Spectator) {
            return Ok(None);
        }
        if self.immune_to_damage(inventory, damage)? {
            return Ok(None);
        }
        if self.effects.fire_resistance.is_some() && self.damage_tag("minecraft:is_fire", damage)? {
            return Ok(None);
        }
        let mut fresh_hit = true;
        let amount = if let Some(next) = &mut next_cooldown {
            let bypass = self.damage_tag("minecraft:bypasses_cooldown", damage)?;
            fresh_hit = next.ticks <= 10 || bypass;
            let Some(accepted) = next.accept(amount, bypass) else {
                return Ok(None);
            };
            accepted
        } else {
            amount
        };
        Ok(Some((amount, fresh_hit, next_cooldown)))
    }

    fn finish_damage(
        &self,
        mut amount: f32,
        damage: &str,
        inventory: &PlayerInventory,
        armor: Option<ArmorValues>,
        vitals: &mut PlayerVitals,
        fresh_hit: bool,
    ) -> Result<DamageOutcome> {
        if !self.damage_tag("minecraft:bypasses_armor", damage)? {
            let armor = armor.context("armor attributes are unavailable")?;
            // LivingEntity.getArmorValue floors the double attribute before
            // passing it to CombatRules, even for custom fractional values.
            let value = armor.display_value() as f32;
            let toughness = armor.toughness as f32;
            let effective = (value - amount / (2.0 + toughness / 4.0)).clamp(value * 0.2, 20.0);
            amount *= 1.0 - effective / 25.0;
        }
        if !self.damage_tag("minecraft:bypasses_effects", damage)? {
            if let Some(effect) = &self.effects.resistance
                && !self.damage_tag("minecraft:bypasses_resistance", damage)?
            {
                let remaining = 25 - (i32::from(effect.amplifier) + 1) * 5;
                amount = (amount * remaining as f32 / 25.0).max(0.0);
            }
            if amount > 0.0 && !self.damage_tag("minecraft:bypasses_enchantments", damage)? {
                let protection = self.protection(inventory, damage)?;
                if protection > 0.0 {
                    amount *= 1.0 - protection.clamp(0.0, 20.0) / 25.0;
                }
            }
        }
        ensure!(amount.is_finite(), "player damage is not finite");
        let absorbed = vitals.absorption.clamp(0.0, amount);
        vitals.absorption -= absorbed;
        let health_damage = amount - absorbed;
        vitals.health = (vitals.health - health_damage).max(0.0);
        Ok(DamageOutcome {
            accepted: true,
            fresh_hit,
            health_damage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::survival::EffectDuration;
    use std::collections::BTreeMap;

    #[test]
    fn broken_armor_is_removed_before_its_protection_is_evaluated() {
        use crate::inventory::ItemStack;
        use native_resources::packs::ResourceProvider;

        struct Data;
        impl ResourceProvider for Data {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                let names: &[&str] = match folder {
                    "enchantment" => &["example:conditional_protection"],
                    "damage_type" => &["minecraft:fall"],
                    "tags/damage_type" => &[
                        "minecraft:bypasses_armor",
                        "minecraft:bypasses_cooldown",
                        "minecraft:bypasses_effects",
                        "minecraft:bypasses_enchantments",
                        "minecraft:is_fall",
                    ],
                    _ => &[],
                };
                names.iter().map(|name| ResourceId::parse(name)).collect()
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                Ok(
                    if path == "data/example/enchantment/conditional_protection.json" {
                        Some(br#"{"slots":["head"],"effects":{"minecraft:damage_protection":[{"effect":{"type":"minecraft:add","value":1},"requirements":{"type":"example:unknown"}}]}}"#.to_vec())
                    } else if path == "data/minecraft/damage_type/fall.json" {
                        Some(br#"{"exhaustion":0.0}"#.to_vec())
                    } else if path.contains("/tags/") {
                        Some(br#"{"values":[]}"#.to_vec())
                    } else {
                        None
                    },
                )
            }
        }
        let registry = Arc::new(OxygenEnchantments::load(&mut Data).unwrap());
        for landing in [false, true] {
            for breaks in [false, true] {
                let mut inventory = PlayerInventory::default();
                inventory.set_slot(
                    PlayerSlot::Armor(0),
                    ItemStack::new(
                        "minecraft:iron_helmet".into(),
                        1,
                        1,
                        Tag::Compound(vec![
                            (
                                "minecraft:damage".into(),
                                Tag::Int(if breaks { 164 } else { 163 }),
                            ),
                            (
                                "minecraft:enchantments".into(),
                                Tag::Compound(vec![(
                                    "example:conditional_protection".into(),
                                    Tag::Int(1),
                                )]),
                            ),
                        ]),
                    ),
                );
                inventory.set_enchantments(Arc::clone(&registry));
                let mut armor = crate::equipment_attributes::ArmorTracker::default();
                armor.set_enchantments(Arc::clone(&registry));
                armor.refresh(&inventory).unwrap();
                let mut inventory = Arc::new(inventory);
                let mut tracker = FallTracker::default();
                tracker.set_enchantments(Arc::clone(&registry));
                let mut vitals = PlayerVitals::default();
                let mut cooldown = DamageCooldown::default();
                let mut random = native_core::random::LegacyRandom::new(12);
                let random_before = random.clone();
                let result = if landing {
                    tracker
                        .land_with_equipment(
                            7.0,
                            None,
                            &mut inventory,
                            &mut armor,
                            &mut vitals,
                            GameMode::Survival,
                            SurvivalRules::default(),
                            &mut cooldown,
                            &mut random,
                            None,
                        )
                        .map(|x| x.damage)
                } else {
                    tracker.damage_with_equipment(
                        4.0,
                        "minecraft:fall",
                        Arc::make_mut(&mut inventory),
                        &mut armor,
                        &mut vitals,
                        GameMode::Survival,
                        &mut cooldown,
                        &mut random,
                        None,
                    )
                };
                if breaks {
                    assert!(result.unwrap().accepted, "landing={landing}");
                    assert!(inventory.slot(PlayerSlot::Armor(0)).is_none());
                    assert_eq!(vitals.health, 16.0);
                } else {
                    assert!(result.is_err(), "landing={landing}");
                    assert_eq!(vitals.health, 20.0);
                    assert_eq!(cooldown, DamageCooldown::default());
                    assert_eq!(
                        inventory
                            .slot(PlayerSlot::Armor(0))
                            .unwrap()
                            .components()
                            .get("minecraft:damage")
                            .and_then(Tag::as_java_int),
                        Some(163)
                    );
                    assert_eq!(random.state(), random_before.state());
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn demo_block_fall_sounds_match_java_registry() {
        use native_core::{
            ore::Ore,
            wood::{Planks, Wood},
        };
        use std::{path::PathBuf, process::Command};
        let mut ids = vec![
            "minecraft:stone",
            "minecraft:grass_block",
            "minecraft:dirt",
            "minecraft:sand",
            "minecraft:red_sand",
            "minecraft:gravel",
            "minecraft:coarse_dirt",
            "minecraft:rooted_dirt",
            "minecraft:podzol",
            "minecraft:mycelium",
            "minecraft:dirt_path",
            "minecraft:farmland",
            "minecraft:crafting_table",
            "minecraft:cobblestone",
            "minecraft:stone_bricks",
        ];
        ids.extend(Planks::ALL.into_iter().map(Planks::id));
        ids.extend(Wood::all().map(Wood::id));
        ids.extend(Ore::ALL.into_iter().map(Ore::id));
        ids.sort_unstable();
        ids.dedup();
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
        let source = temp.path().join("DemoBlockSoundsOracle.java");
        let input = temp.path().join("ids.json");
        let output = temp.path().join("sounds.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/DemoBlockSoundsOracle.java"),
        )
        .unwrap();
        std::fs::write(&input, serde_json::to_vec(&ids).unwrap()).unwrap();
        let run = |command: &mut Command| {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source));
        run(Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("DemoBlockSoundsOracle")
            .arg(&input)
            .arg(&output));
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), ids.len());
        for (row, id) in rows.iter().zip(ids) {
            assert_eq!(row["id"], id);
            assert_eq!(
                row["event"],
                format!("minecraft:{}", block_fall_sound(id).unwrap()),
                "{id}"
            );
            assert_eq!(
                row["volume_bits"].as_i64().unwrap() as u32,
                0.5_f32.to_bits(),
                "{id}"
            );
            assert_eq!(
                row["pitch_bits"].as_i64().unwrap() as u32,
                0.75_f32.to_bits(),
                "{id}"
            );
        }
    }

    #[test]
    fn fractional_armor_is_floored_before_damage_reduction() {
        let mut vitals = PlayerVitals::default();
        FallTracker::default()
            .damage_outcome(
                4.0,
                "minecraft:lava",
                &PlayerInventory::default(),
                Some(ArmorValues {
                    armor: 1.99,
                    toughness: 0.0,
                }),
                &mut vitals,
                GameMode::Survival,
                None,
            )
            .unwrap();
        // One armor point at four damage reduces it by 0.8 percent.
        assert_eq!(
            vitals.health.to_bits(),
            (20.0_f32 - 4.0_f32 * (1.0 - 0.2 / 25.0)).to_bits()
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn fractional_armor_damage_matches_java_player() {
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
        let classpath = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("PlayerArmorDamageOracle.java");
        let result = temp.path().join("armor.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/PlayerArmorDamageOracle.java"),
        )
        .unwrap();
        let run = |command: &mut Command| {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source));
        run(Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("PlayerArmorDamageOracle")
            .arg(&result));
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(result).unwrap()).unwrap();
        assert_eq!(rows.len(), 275);
        for row in rows {
            let armor = ArmorValues {
                armor: row["armor"].as_f64().unwrap(),
                toughness: row["toughness"].as_f64().unwrap(),
            };
            let mut vitals = PlayerVitals::default();
            FallTracker::default()
                .damage_outcome(
                    row["amount"].as_f64().unwrap() as f32,
                    "minecraft:lava",
                    &PlayerInventory::default(),
                    Some(armor),
                    &mut vitals,
                    GameMode::Survival,
                    None,
                )
                .unwrap();
            assert_eq!(
                vitals.health.to_bits(),
                row["health_bits"].as_i64().unwrap() as u32,
                "{row}"
            );
        }
    }

    #[test]
    fn damage_outcome_distinguishes_new_hits_cooldown_and_absorption() {
        let tracker = FallTracker::default();
        let inventory = PlayerInventory::default();
        let mut vitals = PlayerVitals::default();
        let mut cooldown = DamageCooldown::default();
        let apply = |amount, vitals: &mut PlayerVitals, cooldown: &mut DamageCooldown| {
            tracker
                .damage_outcome(
                    amount,
                    "minecraft:lava",
                    &inventory,
                    Some(ArmorValues::default()),
                    vitals,
                    GameMode::Survival,
                    Some(cooldown),
                )
                .unwrap()
        };
        assert_eq!(
            apply(1.0, &mut vitals, &mut cooldown),
            DamageOutcome {
                accepted: true,
                fresh_hit: true,
                health_damage: 1.0
            }
        );
        assert_eq!(
            apply(4.0, &mut vitals, &mut cooldown),
            DamageOutcome {
                accepted: true,
                fresh_hit: false,
                health_damage: 3.0
            }
        );
        assert_eq!(
            apply(4.0, &mut vitals, &mut cooldown),
            DamageOutcome::default()
        );
        cooldown = DamageCooldown::default();
        vitals.absorption = 4.0;
        assert_eq!(
            apply(4.0, &mut vitals, &mut cooldown),
            DamageOutcome {
                accepted: true,
                fresh_hit: true,
                health_damage: 0.0
            }
        );
        assert_eq!(vitals.health, 16.0);
        assert_eq!(vitals.absorption, 0.0);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn fire_damage_absorption_and_exhaustion_match_java() {
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
        let classpath = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("PlayerFireExhaustionOracle.java");
        let result = temp.path().join("damage.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/PlayerFireExhaustionOracle.java"),
        )
        .unwrap();
        let compile = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("PlayerFireExhaustionOracle")
            .arg(&result)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(result).unwrap()).unwrap();
        assert_eq!(rows.len(), 24);
        for row in rows {
            let mut vitals = PlayerVitals {
                absorption: row["initial_absorption"].as_f64().unwrap() as f32,
                ..Default::default()
            };
            let outcome = FallTracker::default()
                .damage_outcome(
                    row["damage"].as_f64().unwrap() as f32,
                    "minecraft:lava",
                    &PlayerInventory::default(),
                    Some(ArmorValues::default()),
                    &mut vitals,
                    GameMode::Survival,
                    Some(&mut DamageCooldown::default()),
                )
                .unwrap();
            if outcome.health_damage > 0.0 {
                vitals.add_exhaustion(row["source_exhaustion"].as_f64().unwrap() as f32);
            }
            assert_eq!(
                vitals.health.to_bits(),
                row["health_bits"].as_i64().unwrap() as u32,
                "{row}"
            );
            assert_eq!(
                vitals.absorption.to_bits(),
                row["absorption_bits"].as_i64().unwrap() as u32,
                "{row}"
            );
            assert_eq!(
                vitals.exhaustion.to_bits(),
                row["exhaustion_bits"].as_i64().unwrap() as u32,
                "{row}"
            );
        }
    }

    #[test]
    fn jump_boost_updates_safe_fall_modifier_on_promotion_and_removal() {
        use crate::survival::{FoodStatusKind, StatusApplication};
        let id = ResourceId::parse("minecraft:effect.jump_boost").unwrap();
        let other = ResourceId::parse("example:keep").unwrap();
        let mut tracker = FallTracker::default();
        tracker.safe.apply(AttributeModifier {
            id: other.clone(),
            operation: AttributeOperation::AddValue,
            amount: 2.0,
        });
        let apply = |duration, amplifier| StatusApplication {
            kind: FoodStatusKind::JumpBoost,
            duration,
            amplifier,
            ambient: false,
            visible: true,
            show_icon: true,
        };
        tracker.add_status(apply(100, 0));
        tracker.add_status(apply(1, 1));
        assert_eq!(tracker.safe.modifier(&id).unwrap().amount, 2.0);
        tracker.tick_effects();
        assert_eq!(tracker.effects.jump_boost.as_ref().unwrap().amplifier, 0);
        assert_eq!(tracker.safe.modifier(&id).unwrap().amount, 1.0);
        tracker.remove_status(FoodStatusKind::JumpBoost);
        assert!(tracker.safe.modifier(&id).is_none());
        assert_eq!(tracker.safe.modifier(&other).unwrap().amount, 2.0);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn applying_jump_boost_matches_java_effect_and_attribute() {
        use crate::survival::{FoodStatusKind, StatusApplication};
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
        let source = temp.path().join("JumpBoostApplyOracle.java");
        let output = temp.path().join("jump.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/JumpBoostApplyOracle.java"),
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
                .arg("JumpBoostApplyOracle")
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let old = [
            (0, 0, false, true, true),
            (100, 0, false, true, true),
            (100, 2, true, false, false),
            (50, 3, true, true, false),
            (-1, 1, false, true, true),
        ];
        let incoming = [
            (40, 0, false, false, false),
            (1800, 0, false, true, true),
            (400, 3, false, true, true),
            (100, 2, false, true, true),
            (-1, 1, true, false, false),
            (0, 0, false, true, true),
        ];
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), 60);
        for row in rows {
            let index = row["existing"].as_u64().unwrap() as usize;
            let (duration, amplifier, ambient, visible, show_icon) = old[index];
            let (new_duration, new_amplifier, new_ambient, new_visible, new_icon) =
                incoming[row["add"].as_u64().unwrap() as usize];
            let id = ResourceId::parse("minecraft:effect.jump_boost").unwrap();
            let mut safe = SavedAttribute::with_base(3.0);
            if row["modifier"].as_i64().unwrap() != 0 {
                safe.apply(AttributeModifier {
                    id: id.clone(),
                    operation: AttributeOperation::AddValue,
                    amount: f64::from(amplifier) + 1.0,
                });
            }
            let mut tracker = FallTracker::default();
            tracker.configure(
                safe,
                SavedAttribute::with_base(1.0),
                FallEffects {
                    jump_boost: (index != 0).then_some(EffectDuration {
                        duration,
                        amplifier,
                        ambient,
                        visible,
                        show_icon,
                        hidden: None,
                    }),
                    ..Default::default()
                },
                Arc::new(OxygenEnchantments::default()),
            );
            let changed = tracker.add_status(StatusApplication {
                kind: FoodStatusKind::JumpBoost,
                duration: new_duration,
                amplifier: new_amplifier,
                ambient: new_ambient,
                visible: new_visible,
                show_icon: new_icon,
            });
            assert_eq!(changed, row["changed"].as_bool().unwrap(), "{row}");
            let effect = tracker.effects.jump_boost.as_ref().unwrap();
            assert_eq!(
                (
                    effect.duration,
                    effect.amplifier,
                    effect.ambient,
                    effect.visible,
                    effect.show_icon
                ),
                (
                    row["duration"].as_i64().unwrap() as i32,
                    row["amplifier"].as_u64().unwrap() as u8,
                    row["ambient"].as_bool().unwrap(),
                    row["visible"].as_bool().unwrap(),
                    row["icon"].as_bool().unwrap()
                ),
                "{row}"
            );
            assert_eq!(
                effect.hidden.as_ref().map(|v| (
                    v.duration,
                    v.amplifier,
                    v.ambient,
                    v.visible,
                    v.show_icon
                )),
                row["hidden_duration"].as_i64().map(|duration| (
                    duration as i32,
                    row["hidden_amplifier"].as_u64().unwrap() as u8,
                    row["hidden_ambient"].as_bool().unwrap(),
                    row["hidden_visible"].as_bool().unwrap(),
                    row["hidden_icon"].as_bool().unwrap()
                )),
                "{row}"
            );
            assert_eq!(
                tracker.safe.modifier(&id).map(|v| v.amount.to_bits()),
                row["amount"].as_f64().map(f64::to_bits),
                "{row}"
            );
            assert_eq!(
                tracker.safe.value(-1024.0, 1024.0).unwrap().to_bits(),
                row["value"].as_f64().unwrap().to_bits(),
                "{row}"
            );
        }
    }

    #[test]
    fn damage_sources_share_cooldown_and_leave_failed_damage_untouched() {
        let mut tracker = FallTracker::default();
        let inventory = PlayerInventory::default();
        let mut vitals = PlayerVitals::default();
        let mut cooldown = DamageCooldown::default();
        tracker
            .damage(
                4.0,
                "minecraft:fall",
                &inventory,
                None,
                &mut vitals,
                GameMode::Survival,
                Some(&mut cooldown),
            )
            .unwrap();
        assert_eq!(vitals.health, 16.0);
        tracker
            .damage(
                2.0,
                "minecraft:drown",
                &inventory,
                None,
                &mut vitals,
                GameMode::Survival,
                Some(&mut cooldown),
            )
            .unwrap();
        assert_eq!(vitals.health, 16.0);
        tracker
            .damage(
                6.0,
                "minecraft:drown",
                &inventory,
                None,
                &mut vitals,
                GameMode::Survival,
                Some(&mut cooldown),
            )
            .unwrap();
        assert_eq!(vitals.health, 14.0);
        for _ in 0..10 {
            cooldown.tick();
        }
        tracker
            .damage(
                1.0,
                "minecraft:starve",
                &inventory,
                None,
                &mut vitals,
                GameMode::Survival,
                Some(&mut cooldown),
            )
            .unwrap();
        assert_eq!(vitals.health, 13.0);
        tracker.enchantments = Some(Arc::new(OxygenEnchantments::default()));
        let before = cooldown;
        assert!(
            tracker
                .damage(
                    8.0,
                    "minecraft:fall",
                    &inventory,
                    None,
                    &mut vitals,
                    GameMode::Survival,
                    Some(&mut cooldown)
                )
                .is_err()
        );
        assert_eq!(cooldown, before);
        assert_eq!(vitals.health, 13.0);
    }

    #[test]
    fn fire_resistance_blocks_fire_before_cooldown_and_preserves_other_damage() {
        let mut tracker = FallTracker::default();
        tracker.add_status(crate::survival::StatusApplication {
            kind: crate::survival::FoodStatusKind::FireResistance,
            duration: 20,
            amplifier: 0,
            ambient: false,
            visible: true,
            show_icon: true,
        });
        let inventory = PlayerInventory::default();
        let mut vitals = PlayerVitals::default();
        let mut cooldown = DamageCooldown::default();
        let before = cooldown;
        for id in [
            "minecraft:in_fire",
            "minecraft:campfire",
            "minecraft:on_fire",
            "minecraft:lava",
            "minecraft:hot_floor",
            "minecraft:sulfur_cube_hot",
            "minecraft:unattributed_fireball",
            "minecraft:fireball",
        ] {
            tracker
                .damage(
                    4.0,
                    id,
                    &inventory,
                    Some(ArmorValues::default()),
                    &mut vitals,
                    GameMode::Survival,
                    Some(&mut cooldown),
                )
                .unwrap();
            assert_eq!(vitals.health, 20.0, "{id}");
            assert_eq!(cooldown, before, "{id}");
        }
        tracker
            .damage(
                4.0,
                "minecraft:fall",
                &inventory,
                Some(ArmorValues::default()),
                &mut vitals,
                GameMode::Survival,
                Some(&mut cooldown),
            )
            .unwrap();
        assert_eq!(vitals.health, 16.0);
        assert_ne!(cooldown, before);
    }

    #[test]
    fn fire_resistance_uses_active_damage_tags_and_rejects_invalid_tags() {
        use native_resources::packs::ResourceProvider;
        struct Data(bool);
        impl ResourceProvider for Data {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                Ok(if folder == "tags/damage_type" {
                    [
                        "is_fire",
                        "bypasses_cooldown",
                        "bypasses_armor",
                        "bypasses_effects",
                        "bypasses_resistance",
                        "bypasses_enchantments",
                    ]
                    .into_iter()
                    .map(|name| ResourceId::parse(name).unwrap())
                    .collect()
                } else {
                    Vec::new()
                })
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path.ends_with("/is_fire.json") {
                    return Ok(Some(if self.0 {
                        b"invalid".to_vec()
                    } else {
                        br#"{"replace":true,"values":["minecraft:fall"]}"#.to_vec()
                    }));
                }
                if path.contains("/tags/damage_type/") {
                    return Ok(Some(br#"{"values":[]}"#.to_vec()));
                }
                Ok(None)
            }
        }
        for invalid in [false, true] {
            let mut tracker = FallTracker::default();
            tracker.add_status(crate::survival::StatusApplication {
                kind: crate::survival::FoodStatusKind::FireResistance,
                duration: 20,
                amplifier: 0,
                ambient: false,
                visible: true,
                show_icon: true,
            });
            tracker.enchantments = Some(Arc::new(
                OxygenEnchantments::load(&mut Data(invalid)).unwrap(),
            ));
            let mut vitals = PlayerVitals::default();
            let mut cooldown = DamageCooldown::default();
            let result = tracker.damage(
                4.0,
                "minecraft:fall",
                &PlayerInventory::default(),
                Some(ArmorValues::default()),
                &mut vitals,
                GameMode::Survival,
                Some(&mut cooldown),
            );
            assert_eq!(result.is_err(), invalid);
            assert_eq!(vitals.health, 20.0);
            assert_eq!(cooldown, DamageCooldown::default());
            if !invalid {
                tracker
                    .damage(
                        4.0,
                        "minecraft:lava",
                        &PlayerInventory::default(),
                        Some(ArmorValues::default()),
                        &mut vitals,
                        GameMode::Survival,
                        Some(&mut cooldown),
                    )
                    .unwrap();
                assert_eq!(vitals.health, 16.0);
            }
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn fire_resistance_damage_order_matches_java() {
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
        let source = temp.path().join("FireResistanceOracle.java");
        let helper = temp.path().join("DamageCooldownOracle.java");
        let result = temp.path().join("fire.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/FireResistanceOracle.java"),
        )
        .unwrap();
        std::fs::write(
            &helper,
            include_str!("../tests/java/DamageCooldownOracle.java"),
        )
        .unwrap();
        let compile = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source)
            .arg(&helper)
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("FireResistanceOracle")
            .arg(&result)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(result).unwrap()).unwrap();
        assert_eq!(rows.len(), 12);
        for row in rows {
            let mut tracker = FallTracker::default();
            if row["resistant"].as_bool().unwrap() {
                tracker.add_status(crate::survival::StatusApplication {
                    kind: crate::survival::FoodStatusKind::FireResistance,
                    duration: 20,
                    amplifier: 0,
                    ambient: false,
                    visible: true,
                    show_icon: true,
                });
            }
            let mut vitals = PlayerVitals::default();
            let mut cooldown = DamageCooldown {
                ticks: row["timer"].as_i64().unwrap() as i32,
                last_damage: 2.0,
            };
            let before = cooldown;
            tracker
                .damage(
                    4.0,
                    if row["fire"].as_bool().unwrap() {
                        "minecraft:lava"
                    } else {
                        "minecraft:fall"
                    },
                    &PlayerInventory::default(),
                    Some(ArmorValues::default()),
                    &mut vitals,
                    GameMode::Survival,
                    Some(&mut cooldown),
                )
                .unwrap();
            let accepted = row["accepted"].as_bool().unwrap();
            assert_eq!(vitals.health < 20.0, accepted, "{row}");
            if accepted {
                assert_eq!(vitals.health, 20.0 - row["damage"].as_f64().unwrap() as f32);
            } else {
                assert_eq!(cooldown, before, "{row}");
            }
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn damage_cooldown_matches_java_hurt_server() {
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
        let source = temp.path().join("DamageCooldownOracle.java");
        let output = temp.path().join("damage.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/DamageCooldownOracle.java"),
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
                .arg("DamageCooldownOracle")
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "Java damage oracle failed: {}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), 480);
        for row in rows {
            let bits = |name: &str| row[name].as_i64().unwrap() as i32 as u32;
            let mut cooldown = DamageCooldown {
                ticks: row["timer"].as_i64().unwrap() as i32,
                last_damage: f32::from_bits(bits("last_bits")),
            };
            let actual = cooldown.accept(
                f32::from_bits(bits("amount_bits")),
                row["bypass"].as_bool().unwrap(),
            );
            assert_eq!(
                actual.is_some(),
                row["accepted"].as_bool().unwrap(),
                "{row}"
            );
            if let Some(actual) = actual {
                assert_eq!(actual.to_bits(), bits("damage_bits"), "{row}");
                assert_eq!(
                    cooldown.ticks,
                    row["timer_after"].as_i64().unwrap() as i32,
                    "{row}"
                );
            }
        }
    }

    fn state(name: &str, properties: &[(&str, &str)]) -> BlockState {
        BlockState {
            name: name.into(),
            properties: properties
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    #[test]
    fn damage_uses_fall_block_rules_effects_and_absorption() {
        let inventory = PlayerInventory::default();
        let mut tracker = FallTracker::default();
        let mut vitals = PlayerVitals::default();
        let apply =
            |tracker: &FallTracker, block: Option<&BlockState>, vitals: &mut PlayerVitals| {
                tracker
                    .land(
                        8.0,
                        block,
                        &inventory,
                        None,
                        vitals,
                        GameMode::Survival,
                        SurvivalRules::default(),
                    )
                    .unwrap();
            };
        apply(&tracker, None, &mut vitals);
        assert_eq!(vitals.health, 15.0);
        vitals.health = 20.0;
        apply(
            &tracker,
            Some(&state("minecraft:hay_block", &[])),
            &mut vitals,
        );
        assert_eq!(vitals.health, 19.0);
        vitals.health = 20.0;
        apply(
            &tracker,
            Some(&state("minecraft:red_bed", &[])),
            &mut vitals,
        );
        assert_eq!(vitals.health, 19.0);
        vitals.health = 20.0;
        apply(
            &tracker,
            Some(&state("minecraft:slime_block", &[])),
            &mut vitals,
        );
        assert_eq!(vitals.health, 20.0);
        apply(
            &tracker,
            Some(&state(
                "minecraft:pointed_dripstone",
                &[("vertical_direction", "up"), ("thickness", "tip")],
            )),
            &mut vitals,
        );
        assert_eq!(vitals.health, 5.0);
        vitals.health = 20.0;
        vitals.absorption = 1.0;
        tracker.add_status(crate::survival::StatusApplication {
            kind: crate::survival::FoodStatusKind::JumpBoost,
            ambient: false,
            visible: true,
            show_icon: true,
            amplifier: 1,
            duration: 5,
        });
        tracker.effects.resistance = Some(EffectDuration {
            ambient: false,
            visible: true,
            show_icon: true,
            amplifier: 0,
            duration: 5,
            hidden: None,
        });
        apply(&tracker, None, &mut vitals);
        assert_eq!(vitals.absorption, 0.0);
        assert_eq!(vitals.health, 18.6);
    }

    #[test]
    fn rule_and_game_mode_prevent_fall_damage() {
        let tracker = FallTracker::default();
        let inventory = PlayerInventory::default();
        for mode in [GameMode::Creative, GameMode::Spectator] {
            let mut vitals = PlayerVitals::default();
            tracker
                .land(
                    40.0,
                    None,
                    &inventory,
                    None,
                    &mut vitals,
                    mode,
                    SurvivalRules::default(),
                )
                .unwrap();
            assert_eq!(vitals.health, 20.0);
        }
        let mut vitals = PlayerVitals::default();
        tracker
            .land(
                40.0,
                None,
                &inventory,
                None,
                &mut vitals,
                GameMode::Survival,
                SurvivalRules {
                    fall_damage: false,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(vitals.health, 20.0);
        let honey = BlockState {
            name: "minecraft:honey_block".into(),
            properties: Default::default(),
        };
        let calculated = tracker
            .land_with_cooldown(
                30.0,
                Some(&honey),
                &inventory,
                Some(ArmorValues::default()),
                &mut vitals,
                GameMode::Survival,
                SurvivalRules {
                    fall_damage: false,
                    ..Default::default()
                },
                &mut DamageCooldown::default(),
            )
            .unwrap();
        assert_eq!(
            calculated.amount,
            calculate_fall_damage(30.0, 3.0, 0.2, 1.0)
        );
        assert!(calculated.amount > 0);
        assert_eq!(vitals.health, 20.0);
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn real_enchantments_mitigate_fall_damage_through_the_active_registry() {
        use crate::inventory::ItemStack;
        use native_resources::packs::ResourceStack;
        use std::path::PathBuf;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft")
            });
        let mut resources = ResourceStack::open(&game).unwrap();
        let registry = Arc::new(OxygenEnchantments::load(&mut resources).unwrap());
        let equipment = |item: &str, enchantment: &str| {
            ItemStack::unresolved(
                item.into(),
                1,
                Tag::Compound(vec![(
                    "minecraft:enchantments".into(),
                    Tag::Compound(vec![(enchantment.into(), Tag::Int(4))]),
                )]),
            )
            .unwrap()
        };
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(
            PlayerSlot::Armor(3),
            Some(equipment(
                "minecraft:diamond_boots",
                "minecraft:feather_falling",
            )),
        );
        inventory.set_slot(
            PlayerSlot::Armor(1),
            Some(equipment(
                "minecraft:iron_chestplate",
                "minecraft:protection",
            )),
        );
        let mut tracker = FallTracker::default();
        tracker.configure(
            SavedAttribute::with_base(3.0),
            SavedAttribute::with_base(1.0),
            FallEffects::default(),
            registry,
        );
        let mut vitals = PlayerVitals::default();
        tracker
            .land(
                13.0,
                None,
                &inventory,
                None,
                &mut vitals,
                GameMode::Survival,
                SurvivalRules::default(),
            )
            .unwrap();
        assert!((vitals.health - 16.4).abs() < 1.0e-5, "{}", vitals.health);
    }
}
