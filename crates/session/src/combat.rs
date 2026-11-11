//! Player attack charging and minimum-charge item components.

use anyhow::Result;
use native_persistence::nbt::Tag;
use native_resources::{
    enchantments::{AttributeModifier, AttributeOperation, OxygenEnchantments},
    packs::ResourceId,
};

use crate::{
    inventory::{ItemStack, PlayerInventory, PlayerSlot},
    player_data::SavedAttribute,
    survival::FallEffects,
};

pub(crate) fn valid_minimum_charge(value: &Tag) -> bool {
    crate::player_data::numeric_double(value)
        .map(|value| value as f32)
        .is_some_and(|value| value.is_finite() && (0.0..=1.0).contains(&value))
}

pub(crate) fn minimum_charge(item: Option<&ItemStack>) -> f32 {
    let Some(item) = item else { return 0.0 };
    if let Some(value) = item.components().get("minecraft:minimum_attack_charge") {
        return crate::player_data::numeric_double(value)
            .map(|value| value as f32)
            .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
            .unwrap_or(0.0);
    }
    if crate::inventory::is_vanilla_spear(item.id())
        && item
            .components()
            .get("!minecraft:minimum_attack_charge")
            .is_none()
    {
        1.0
    } else {
        0.0
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AttackCharge {
    ticks: i32,
    swap_ticks: i32,
    last_item: Option<String>,
    initial_base: SavedAttribute,
    base: SavedAttribute,
    speed: Option<f64>,
    cached: Option<(u64, Option<u8>, Option<u8>)>,
    previous_effects: Option<(Option<u8>, Option<u8>)>,
}

impl Default for AttackCharge {
    fn default() -> Self {
        Self {
            ticks: 0,
            swap_ticks: 0,
            last_item: None,
            initial_base: SavedAttribute::with_base(4.0),
            base: SavedAttribute::with_base(4.0),
            speed: Some(4.0),
            cached: None,
            previous_effects: None,
        }
    }
}

impl AttackCharge {
    #[cfg(test)]
    pub(crate) fn for_test(ticks: i32, speed: f64) -> Self {
        Self {
            ticks,
            speed: Some(speed),
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub(crate) fn for_swap_test(ticks: i32, speed: f64) -> Self {
        Self {
            swap_ticks: ticks,
            speed: Some(speed),
            ..Self::default()
        }
    }
    pub(crate) fn restore(
        &mut self,
        attribute: SavedAttribute,
        effects: &FallEffects,
    ) -> Result<()> {
        self.initial_base = attribute.clone();
        self.base = attribute;
        self.cached = None;
        self.previous_effects = None;
        self.initialize_effects(effects)
    }

    pub(crate) fn initialize_effects(&mut self, effects: &FallEffects) -> Result<()> {
        self.sync_effects(effects);
        self.speed = Some(self.base.value(0.0, 1024.0)?);
        Ok(())
    }

    pub(crate) fn saved_attributes(&self) -> (&SavedAttribute, &SavedAttribute) {
        (&self.initial_base, &self.base)
    }

    pub(crate) fn can_attack(&self, item: Option<&ItemStack>) -> bool {
        let minimum = minimum_charge(item);
        if minimum <= 0.0 {
            return true;
        }
        let Some(speed) = self.speed else {
            return false;
        };
        let delay = ((1.0 / speed) * 20.0) as f32;
        !((self.ticks as f32 / delay) < minimum)
    }

    /// Player.getItemSwapScale uses the same attack-speed delay, but its own
    /// ticker: a piercing attack resets attack charge without resetting this.
    pub(crate) fn item_swap_scale(&self, partial: f32) -> f32 {
        let Some(speed) = self.speed else { return 1.0 };
        let delay = ((1.0 / speed) * 20.0) as f32;
        ((self.swap_ticks as f32 + partial) / delay).clamp(0.0, 1.0)
    }

    pub(crate) fn attacked(&mut self) {
        self.ticks = 0;
    }

    pub(crate) fn missed(&mut self) {
        self.ticks = 0;
        self.swap_ticks = 0;
    }

    pub(crate) fn tick(
        &mut self,
        inventory: &PlayerInventory,
        effects: &FallEffects,
        registry: &OxygenEnchantments,
        dynamic: bool,
        location: impl FnOnce(SavedAttribute) -> Result<SavedAttribute>,
    ) -> Result<()> {
        self.ticks = self.ticks.wrapping_add(1);
        self.swap_ticks = self.swap_ticks.wrapping_add(1);
        let held = inventory
            .slot(PlayerSlot::Storage(inventory.selected()))
            .map(ItemStack::id);
        if self.last_item.as_deref() != held {
            self.missed();
            self.last_item = held.map(str::to_owned);
        }
        self.sync_effects(effects);
        let haste = effects.haste.as_ref().map(|effect| effect.amplifier);
        let fatigue = effects
            .mining_fatigue
            .as_ref()
            .map(|effect| effect.amplifier);
        let key = (inventory.revision(), haste, fatigue);
        if self.cached == Some(key) && !dynamic {
            return Ok(());
        }
        self.cached = Some(key);
        let result = crate::equipment_attributes::equipped_attribute(
            &self.base,
            inventory,
            registry,
            "attack_speed",
        )
        .and_then(location)
        .and_then(|attribute| attribute.value(0.0, 1024.0));
        self.speed = result.as_ref().ok().copied();
        result.map(|_| ())
    }

    fn sync_effects(&mut self, effects: &FallEffects) {
        let haste = effects.haste.as_ref().map(|effect| effect.amplifier);
        let fatigue = effects
            .mining_fatigue
            .as_ref()
            .map(|effect| effect.amplifier);
        for (index, current, name, amount) in [
            (0, haste, "effect.haste", f64::from(0.1_f32)),
            (1, fatigue, "effect.mining_fatigue", f64::from(-0.1_f32)),
        ] {
            let previous = self
                .previous_effects
                .map(|effects| if index == 0 { effects.0 } else { effects.1 });
            if previous != Some(current) && (current.is_some() || previous.flatten().is_some()) {
                let id = ResourceId {
                    namespace: "minecraft".into(),
                    path: name.into(),
                };
                self.base.remove_modifier(&id);
                if let Some(amplifier) = current {
                    self.base.apply(AttributeModifier {
                        id,
                        operation: AttributeOperation::AddMultipliedTotal,
                        amount: amount * (f64::from(amplifier) + 1.0),
                    });
                }
                self.cached = None;
            }
        }
        self.previous_effects = Some((haste, fatigue));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, components: Vec<(&str, Tag)>) -> ItemStack {
        ItemStack::new(
            id.into(),
            1,
            1,
            Tag::Compound(
                components
                    .into_iter()
                    .map(|(name, tag)| (name.into(), tag))
                    .collect(),
            ),
        )
        .unwrap()
    }

    #[test]
    fn piercing_attack_preserves_swap_progress_but_miss_resets_it() {
        let mut charge = AttackCharge::for_swap_test(2, 4.0);
        let before = charge.item_swap_scale(1.0);
        charge.attacked();
        assert_eq!(charge.item_swap_scale(1.0), before);
        charge.missed();
        assert_eq!(charge.item_swap_scale(1.0), 0.2);
    }

    #[test]
    fn charge_resets_on_item_identity_change_and_reacts_to_effects() {
        let registry = OxygenEnchantments::default();
        let mut inventory = PlayerInventory::default();
        let spear = item("minecraft:iron_spear", vec![]);
        inventory.set_slot(PlayerSlot::Storage(0), Some(spear.clone()));
        let mut charge = AttackCharge::default();
        let mut effects = FallEffects::default();
        let tick =
            |charge: &mut AttackCharge, inventory: &PlayerInventory, effects: &FallEffects| {
                charge
                    .tick(inventory, effects, &registry, false, Ok)
                    .unwrap();
            };
        tick(&mut charge, &inventory, &effects);
        assert!(!charge.can_attack(Some(&spear)));
        for _ in 0..19 {
            tick(&mut charge, &inventory, &effects);
        }
        assert!(charge.can_attack(Some(&spear)));
        charge.attacked();
        assert!(!charge.can_attack(Some(&spear)));
        for _ in 0..10 {
            tick(&mut charge, &inventory, &effects);
        }
        effects.haste = Some(crate::survival::EffectDuration {
            amplifier: 9,
            duration: 100,
            ambient: false,
            visible: true,
            show_icon: true,
            hidden: None,
        });
        tick(&mut charge, &inventory, &effects);
        assert!(charge.can_attack(Some(&spear)));
        inventory.set_slot(
            PlayerSlot::Storage(0),
            Some(item("minecraft:stone", vec![])),
        );
        tick(&mut charge, &inventory, &effects);
        inventory.set_slot(PlayerSlot::Storage(0), Some(spear.clone()));
        tick(&mut charge, &inventory, &effects);
        assert!(!charge.can_attack(Some(&spear)));
    }

    #[test]
    fn loaded_effect_expiring_on_first_tick_removes_its_modifier() {
        let mut initial = SavedAttribute::with_base(4.0);
        let id = ResourceId::parse("minecraft:effect.haste").unwrap();
        initial.apply(AttributeModifier {
            id: id.clone(),
            operation: AttributeOperation::AddMultipliedTotal,
            amount: f64::from(0.1_f32),
        });
        let active = FallEffects {
            haste: Some(crate::survival::EffectDuration {
                amplifier: 0,
                duration: 1,
                ambient: false,
                visible: true,
                show_icon: true,
                hidden: None,
            }),
            ..Default::default()
        };
        let mut charge = AttackCharge::default();
        charge.restore(initial.clone(), &active).unwrap();
        charge
            .tick(
                &PlayerInventory::default(),
                &FallEffects::default(),
                &OxygenEnchantments::default(),
                false,
                Ok,
            )
            .unwrap();
        let (source, current) = charge.saved_attributes();
        assert_eq!(source, &initial);
        assert!(current.modifier(&id).is_none());
        assert_eq!(current.value(0.0, 1024.0).unwrap(), 4.0);
    }

    #[test]
    fn minimum_charge_patches_are_checked_and_preserved() {
        let explicit = Tag::Compound(vec![(
            "minecraft:minimum_attack_charge".into(),
            Tag::Float(0.5),
        )]);
        let removed = Tag::Compound(vec![(
            "!minecraft:minimum_attack_charge".into(),
            Tag::Compound(vec![]),
        )]);
        assert_eq!(
            crate::inventory::saved_item_max("minecraft:iron_spear", &explicit),
            Some(1)
        );
        assert_eq!(
            crate::inventory::saved_item_max("minecraft:iron_spear", &removed),
            Some(1)
        );
        let modifiers = Tag::List {
            element_type: native_persistence::nbt::TagType::Compound,
            values: vec![],
        };
        let combined = Tag::Compound(vec![
            ("minecraft:attribute_modifiers".into(), modifiers.clone()),
            ("minecraft:minimum_attack_charge".into(), Tag::Float(0.5)),
        ]);
        assert_eq!(
            crate::equippable::saved_inventory_max("minecraft:iron_spear", &combined),
            Some(1)
        );
        let conflicting = Tag::Compound(vec![
            ("minecraft:attribute_modifiers".into(), modifiers),
            ("minecraft:minimum_attack_charge".into(), Tag::Float(0.5)),
            (
                "!minecraft:minimum_attack_charge".into(),
                Tag::Compound(vec![]),
            ),
        ]);
        assert_eq!(
            crate::equippable::saved_inventory_max("minecraft:iron_spear", &conflicting),
            None
        );
        assert_eq!(
            minimum_charge(Some(&item(
                "minecraft:iron_spear",
                vec![("!minecraft:minimum_attack_charge", Tag::Compound(vec![]))]
            ))),
            0.0
        );
        for value in [-0.1, 1.1, f32::NAN, f32::INFINITY] {
            assert!(!valid_minimum_charge(&Tag::Float(value)));
            let patch = Tag::Compound(vec![(
                "minecraft:minimum_attack_charge".into(),
                Tag::Float(value),
            )]);
            assert_eq!(
                crate::inventory::saved_item_max("minecraft:iron_spear", &patch),
                None
            );
        }
    }
}
