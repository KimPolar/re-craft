//! Java 26.3 item attack-range components and reach checks.

use anyhow::{Context, Result, ensure};
use glam::DVec3;
use native_persistence::nbt::Tag;

use crate::{GameMode, inventory::ItemStack};

/// Data-component values are single-precision, including intermediate sums.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AttackRange {
    pub min_reach: f32,
    pub max_reach: f32,
    pub min_creative_reach: f32,
    pub max_creative_reach: f32,
    pub hitbox_margin: f32,
    pub mob_factor: f32,
}

impl Default for AttackRange {
    fn default() -> Self {
        Self {
            min_reach: 0.0,
            max_reach: 3.0,
            min_creative_reach: 0.0,
            max_creative_reach: 5.0,
            hitbox_margin: 0.3,
            mob_factor: 1.0,
        }
    }
}

impl AttackRange {
    pub(crate) fn parse(tag: &Tag) -> Option<Self> {
        let Tag::Compound(fields) = tag else {
            return None;
        };
        let mut range = Self::default();
        let mut seen = 0_u8;
        for (name, value) in fields {
            let (field, bit, upper) = if name.matches("min_reach") {
                (&mut range.min_reach, 1, 64.0)
            } else if name.matches("max_reach") {
                (&mut range.max_reach, 2, 64.0)
            } else if name.matches("min_creative_reach") {
                (&mut range.min_creative_reach, 4, 64.0)
            } else if name.matches("max_creative_reach") {
                (&mut range.max_creative_reach, 8, 64.0)
            } else if name.matches("hitbox_margin") {
                (&mut range.hitbox_margin, 16, 1.0)
            } else if name.matches("mob_factor") {
                (&mut range.mob_factor, 32, 2.0)
            } else {
                // Preserve future fields without claiming their behavior is known.
                return None;
            };
            if seen & bit != 0 {
                return None;
            }
            let value = crate::player_data::numeric_double(value)? as f32;
            if !value.is_finite() || !(0.0..=upper).contains(&value) {
                return None;
            }
            *field = value;
            seen |= bit;
        }
        Some(range)
    }

    pub fn effective_range(self, player_mode: Option<GameMode>) -> (f32, f32) {
        match player_mode {
            Some(GameMode::Creative) => (self.min_creative_reach, self.max_creative_reach),
            Some(_) => (self.min_reach, self.max_reach),
            None => (
                self.min_reach * self.mob_factor,
                self.max_reach * self.mob_factor,
            ),
        }
    }

    /// The distance to the hit location is compared inclusively, after the
    /// component's hitbox margin is added in f32 as in Java.
    pub fn contains_point(self, eye: DVec3, hit: DVec3, player_mode: Option<GameMode>) -> bool {
        self.contains_distance(eye.distance(hit), player_mode, 0.0)
    }

    /// The box overload uses distance to the nearest point and a separate
    /// double-precision tolerance, as used by Java's entity checks.
    pub fn contains_box(
        self,
        eye: DVec3,
        min: DVec3,
        max: DVec3,
        player_mode: Option<GameMode>,
        tolerance: f64,
    ) -> bool {
        if !eye.is_finite() || !min.is_finite() || !max.is_finite() {
            return false;
        }
        let nearest = eye.clamp(min.min(max), min.max(max));
        self.contains_distance(eye.distance(nearest), player_mode, tolerance)
    }

    fn contains_distance(
        self,
        distance: f64,
        player_mode: Option<GameMode>,
        tolerance: f64,
    ) -> bool {
        let (min, max) = self.effective_range(player_mode);
        distance >= f64::from(min - self.hitbox_margin) - tolerance
            && distance <= f64::from(max + self.hitbox_margin) + tolerance
    }
}

/// Effective stack component. A removed component is distinct from a default
/// component and lets Java use its entity-interaction-range attribute instead.
pub fn item_attack_range(item: Option<&ItemStack>) -> Result<Option<AttackRange>> {
    let Some(item) = item else { return Ok(None) };
    let Tag::Compound(fields) = item.components() else {
        anyhow::bail!("item components are not a compound")
    };
    let mut present = None;
    let mut removed = false;
    for (name, value) in fields {
        if name.matches("minecraft:attack_range") {
            ensure!(
                present.is_none() && !removed,
                "duplicate or conflicting attack range patch"
            );
            present = Some(
                AttackRange::parse(value)
                    .context("unsupported or invalid attack range component")?,
            );
        } else if name.matches("!minecraft:attack_range") {
            ensure!(
                !removed && present.is_none(),
                "duplicate or conflicting attack range patch"
            );
            ensure!(
                matches!(value, Tag::Compound(fields) if fields.is_empty()),
                "invalid attack range removal"
            );
            removed = true;
        }
    }
    if let Some(range) = present {
        return Ok(Some(range));
    }
    if removed {
        return Ok(None);
    }
    Ok(
        crate::inventory::is_vanilla_spear(item.id()).then_some(AttackRange {
            min_reach: 2.0,
            max_reach: 4.5,
            min_creative_reach: 2.0,
            max_creative_reach: 6.5,
            hitbox_margin: 0.125,
            mob_factor: 0.5,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_defaults_limits_and_unknown_data_are_checked() {
        assert_eq!(
            AttackRange::parse(&Tag::Compound(vec![])),
            Some(AttackRange::default())
        );
        for (name, value) in [
            ("max_reach", 64.1),
            ("hitbox_margin", 1.01),
            ("mob_factor", 2.01),
            ("min_reach", -0.1),
        ] {
            assert!(
                AttackRange::parse(&Tag::Compound(vec![(name.into(), Tag::Float(value))]))
                    .is_none()
            );
        }
        assert!(AttackRange::parse(&Tag::Compound(vec![("future".into(), Tag::Int(1))])).is_none());
        assert!(
            AttackRange::parse(&Tag::Compound(vec![
                ("max_reach".into(), Tag::Float(1.0)),
                ("max_reach".into(), Tag::Float(2.0))
            ]))
            .is_none()
        );
    }

    #[test]
    fn spear_range_respects_patch_and_inclusive_margin() {
        let item = |fields| {
            ItemStack::new("minecraft:iron_spear".into(), 1, 1, Tag::Compound(fields)).unwrap()
        };
        let spear = item(vec![]);
        let range = item_attack_range(Some(&spear)).unwrap().unwrap();
        assert!(range.contains_point(DVec3::ZERO, DVec3::X * 1.875, Some(GameMode::Survival)));
        assert!(range.contains_point(DVec3::ZERO, DVec3::X * 4.625, Some(GameMode::Survival)));
        assert!(!range.contains_point(
            DVec3::ZERO,
            DVec3::X * 4.625_000_1,
            Some(GameMode::Survival)
        ));
        let removed = item(vec![(
            "!minecraft:attack_range".into(),
            Tag::Compound(vec![]),
        )]);
        assert!(item_attack_range(Some(&removed)).unwrap().is_none());
        let custom = item(vec![(
            "minecraft:attack_range".into(),
            Tag::Compound(vec![]),
        )]);
        assert_eq!(
            item_attack_range(Some(&custom)).unwrap(),
            Some(AttackRange::default())
        );
    }

    #[test]
    fn inventory_patches_preserve_valid_ranges_and_reject_ambiguous_data() {
        let range = Tag::Compound(vec![("max_reach".into(), Tag::Float(7.0))]);
        let patch = Tag::Compound(vec![("minecraft:attack_range".into(), range.clone())]);
        assert_eq!(
            crate::inventory::saved_item_max("minecraft:iron_spear", &patch),
            Some(1)
        );
        let removed = Tag::Compound(vec![(
            "!minecraft:attack_range".into(),
            Tag::Compound(vec![]),
        )]);
        assert_eq!(
            crate::inventory::saved_item_max("minecraft:iron_spear", &removed),
            Some(1)
        );
        let invalid = Tag::Compound(vec![(
            "minecraft:attack_range".into(),
            Tag::Compound(vec![("max_reach".into(), Tag::Float(65.0))]),
        )]);
        assert_eq!(
            crate::inventory::saved_item_max("minecraft:iron_spear", &invalid),
            None
        );
        let modifiers = Tag::List {
            element_type: native_persistence::nbt::TagType::Compound,
            values: vec![],
        };
        let combined = Tag::Compound(vec![
            ("minecraft:attribute_modifiers".into(), modifiers.clone()),
            ("minecraft:attack_range".into(), range.clone()),
        ]);
        assert_eq!(
            crate::equippable::saved_inventory_max("minecraft:iron_spear", &combined),
            Some(1)
        );
        let conflicting = Tag::Compound(vec![
            ("minecraft:attribute_modifiers".into(), modifiers),
            ("minecraft:attack_range".into(), range),
            ("!minecraft:attack_range".into(), Tag::Compound(vec![])),
        ]);
        assert_eq!(
            crate::equippable::saved_inventory_max("minecraft:iron_spear", &conflicting),
            None
        );
        let item = ItemStack::unresolved("minecraft:iron_spear".into(), 1, invalid).unwrap();
        assert!(item_attack_range(Some(&item)).is_err());
        let item = ItemStack::unresolved("minecraft:iron_spear".into(), 1, conflicting).unwrap();
        assert!(item_attack_range(Some(&item)).is_err());
    }
}
