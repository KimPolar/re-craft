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
        self.effects.tick();
    }

    fn damage_tag(&self, tag: &str, damage: &str) -> Result<bool> {
        if let Some(registry) = &self.enchantments {
            registry.damage_type_has_tag(tag, damage)
        } else {
            // The demo contains no data packs and uses the built-in ordinary fall.
            Ok(matches!(
                tag,
                "minecraft:is_fall" | "minecraft:bypasses_armor"
            ))
        }
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
        if distance <= 0.0
            || vitals.health <= 0.0
            || matches!(mode, GameMode::Creative | GameMode::Spectator)
        {
            return Ok(());
        }
        if let Some(registry) = &self.enchantments
            && registry.fall_damage_immune("minecraft:player")?
        {
            return Ok(());
        }
        let surface = FallSurface::for_state(state);
        let mut safe = self.safe.clone();
        if let Some(effect) = &self.effects.jump_boost {
            safe.apply(AttributeModifier {
                id: ResourceId::parse("minecraft:effect.jump_boost")?,
                operation: AttributeOperation::AddValue,
                amount: f64::from(effect.amplifier) + 1.0,
            });
        }
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
        let mut amount =
            calculate_fall_damage(distance, safe, surface.damage_multiplier, multiplier) as f32;
        if amount <= 0.0 {
            return Ok(());
        }
        let damage = surface.damage_type;
        if !rules.fall_damage && self.damage_tag("minecraft:is_fall", damage)? {
            return Ok(());
        }
        if !self.damage_tag("minecraft:bypasses_armor", damage)? {
            let armor = armor.context("armor attributes are unavailable")?;
            let value = armor.armor as f32;
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
        ensure!(amount.is_finite(), "fall damage is not finite");
        let absorbed = vitals.absorption.clamp(0.0, amount);
        vitals.absorption -= absorbed;
        vitals.health = (vitals.health - (amount - absorbed)).max(0.0);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::survival::EffectDuration;
    use std::collections::BTreeMap;

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
        tracker.effects.jump_boost = Some(EffectDuration {
            amplifier: 1,
            duration: 5,
            hidden: None,
        });
        tracker.effects.resistance = Some(EffectDuration {
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
