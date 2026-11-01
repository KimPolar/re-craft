//! Apply equipped 26.3 item and enchantment modifiers to saved attributes.

use anyhow::{Context, Result, ensure};
use native_persistence::nbt::Tag;
use native_resources::{
    enchantments::{AttributeModifier, AttributeOperation, EquipmentSlot, OxygenEnchantments},
    packs::ResourceId,
};
use std::sync::Arc;

use crate::{
    inventory::{PlayerInventory, PlayerSlot},
    player_data::{SavedAttribute, numeric_double, text},
};

pub(crate) fn oxygen_bonus(
    saved: &SavedAttribute,
    inventory: &PlayerInventory,
    enchantments: &OxygenEnchantments,
) -> Result<f64> {
    equipped_attribute(saved, inventory, enchantments, "oxygen_bonus")?.value(0.0, 1024.0)
}

pub(crate) fn bounciness(
    saved: &SavedAttribute,
    inventory: &PlayerInventory,
    enchantments: &OxygenEnchantments,
) -> Result<f64> {
    equipped_attribute(saved, inventory, enchantments, "bounciness")?.value(0.0, 1.0)
}

pub(crate) fn knockback_resistance(
    saved: &SavedAttribute,
    inventory: &PlayerInventory,
    enchantments: &OxygenEnchantments,
) -> Result<f64> {
    equipped_attribute(saved, inventory, enchantments, "knockback_resistance")?.value(-2.0, 1.0)
}

pub(crate) fn jump_strength(
    saved: &SavedAttribute,
    inventory: &PlayerInventory,
    enchantments: &OxygenEnchantments,
) -> Result<f64> {
    equipped_attribute(saved, inventory, enchantments, "jump_strength")?.value(0.0, 32.0)
}

pub(crate) fn gravity(
    saved: &SavedAttribute,
    inventory: &PlayerInventory,
    enchantments: &OxygenEnchantments,
) -> Result<f64> {
    equipped_attribute(saved, inventory, enchantments, "gravity")?.value(-1.0, 1.0)
}

pub(crate) fn water_movement_efficiency(
    saved: &SavedAttribute,
    inventory: &PlayerInventory,
    enchantments: &OxygenEnchantments,
) -> Result<f64> {
    equipped_attribute(saved, inventory, enchantments, "water_movement_efficiency")?.value(0.0, 1.0)
}

pub(crate) fn mining_attributes(
    saved: &crate::player_data::SavedMiningAttributes,
    inventory: &PlayerInventory,
    enchantments: &OxygenEnchantments,
) -> Result<[f64; 3]> {
    Ok([
        equipped_attribute(
            &saved.efficiency,
            inventory,
            enchantments,
            "mining_efficiency",
        )?
        .value(0.0, 1024.0)?,
        equipped_attribute(
            &saved.block_break_speed,
            inventory,
            enchantments,
            "block_break_speed",
        )?
        .value(0.0, 1024.0)?,
        equipped_attribute(
            &saved.submerged_speed,
            inventory,
            enchantments,
            "submerged_mining_speed",
        )?
        .value(0.0, 20.0)?,
    ])
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ArmorValues {
    pub armor: f64,
    pub toughness: f64,
}

impl ArmorValues {
    pub fn display_value(self) -> i32 {
        self.armor.floor() as i32
    }
}

pub(crate) struct ArmorTracker {
    armor: SavedAttribute,
    toughness: SavedAttribute,
    enchantments: Arc<OxygenEnchantments>,
    revision: Option<u64>,
    value: Option<ArmorValues>,
}

impl Default for ArmorTracker {
    fn default() -> Self {
        Self {
            armor: SavedAttribute::with_base(0.0),
            toughness: SavedAttribute::with_base(0.0),
            enchantments: Arc::new(OxygenEnchantments::default()),
            revision: None,
            value: Some(ArmorValues::default()),
        }
    }
}

impl ArmorTracker {
    pub fn reset_after_death(&mut self) {
        self.armor.clear_modifiers();
        self.toughness.clear_modifiers();
        self.invalidate();
    }

    pub fn configure(
        &mut self,
        armor: SavedAttribute,
        toughness: SavedAttribute,
        enchantments: Arc<OxygenEnchantments>,
        inventory: &PlayerInventory,
    ) -> Result<()> {
        let value = armor_values(&armor, &toughness, inventory, &enchantments)?;
        self.armor = armor;
        self.toughness = toughness;
        self.enchantments = enchantments;
        self.revision = Some(inventory.revision());
        self.value = Some(value);
        Ok(())
    }

    pub fn invalidate(&mut self) {
        self.revision = None;
    }

    pub fn set_enchantments(&mut self, enchantments: Arc<OxygenEnchantments>) {
        self.enchantments = enchantments;
        self.invalidate();
    }

    pub fn value(&self) -> Option<ArmorValues> {
        self.value
    }

    pub(crate) fn value_for(&self, inventory: &PlayerInventory) -> Result<ArmorValues> {
        armor_values(&self.armor, &self.toughness, inventory, &self.enchantments)
    }

    /// Recompute only after an inventory change. Unknown effects suppress the
    /// result instead of displaying a value guessed from partial information.
    pub fn refresh(&mut self, inventory: &PlayerInventory) -> Result<()> {
        if self.revision == Some(inventory.revision()) {
            return Ok(());
        }
        self.revision = Some(inventory.revision());
        match armor_values(&self.armor, &self.toughness, inventory, &self.enchantments) {
            Ok(value) => {
                self.value = Some(value);
                Ok(())
            }
            Err(error) => {
                self.value = None;
                Err(error)
            }
        }
    }
}

pub(crate) fn armor_values(
    armor: &SavedAttribute,
    toughness: &SavedAttribute,
    inventory: &PlayerInventory,
    enchantments: &OxygenEnchantments,
) -> Result<ArmorValues> {
    Ok(ArmorValues {
        armor: equipped_attribute(armor, inventory, enchantments, "armor")?.value(0.0, 30.0)?,
        toughness: equipped_attribute(toughness, inventory, enchantments, "armor_toughness")?
            .value(0.0, 20.0)?,
    })
}

/// Default modifiers of player-wearable 26.3 armor item prototypes. Explicit
/// component patches replace the prototype's entire modifier component.
fn default_armor_modifier(
    item: &str,
    slot: EquipmentSlot,
    attribute: &str,
) -> Option<AttributeModifier> {
    let name = item.strip_prefix("minecraft:")?;
    let (material, piece) = if name == "turtle_helmet" {
        ("turtle", "helmet")
    } else {
        name.split_once('_')?
    };
    let (expected, index) = match piece {
        "helmet" => (EquipmentSlot::Head, 0),
        "chestplate" => (EquipmentSlot::Chest, 1),
        "leggings" => (EquipmentSlot::Legs, 2),
        "boots" => (EquipmentSlot::Feet, 3),
        _ => return None,
    };
    if slot != expected {
        return None;
    }
    let (protection, toughness): ([f64; 4], f64) = match material {
        "turtle" if index == 0 => ([2.0, 0.0, 0.0, 0.0], 0.0),
        "leather" => ([1.0, 3.0, 2.0, 1.0], 0.0),
        "golden" => ([2.0, 5.0, 3.0, 1.0], 0.0),
        "copper" => ([2.0, 4.0, 3.0, 1.0], 0.0),
        "chainmail" => ([2.0, 5.0, 4.0, 1.0], 0.0),
        "iron" => ([2.0, 6.0, 5.0, 2.0], 0.0),
        "diamond" => ([3.0, 8.0, 6.0, 3.0], 2.0),
        "netherite" => ([3.0, 8.0, 6.0, 3.0], 3.0),
        _ => return None,
    };
    let amount = match attribute {
        "armor" => protection[index],
        "armor_toughness" => toughness,
        "knockback_resistance" if material == "netherite" => f64::from(0.1_f32),
        _ => return None,
    };
    Some(AttributeModifier {
        id: ResourceId {
            namespace: "minecraft".into(),
            path: format!("armor.{piece}"),
        },
        operation: AttributeOperation::AddValue,
        amount,
    })
}

pub(crate) fn equipped_attribute(
    saved: &SavedAttribute,
    inventory: &PlayerInventory,
    enchantments: &OxygenEnchantments,
    attribute_name: &str,
) -> Result<SavedAttribute> {
    let mut attribute = saved.clone();
    let apply = |attribute: &mut SavedAttribute, modifier: AttributeModifier| -> Result<()> {
        ensure!(
            !attribute.contains_modifier(&modifier.id),
            "overlapping equipment or persistent modifier id {}:{}",
            modifier.id.namespace,
            modifier.id.path
        );
        attribute.apply(modifier);
        Ok(())
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
    for (equipment_slot, player_slot) in slots {
        let Some(item) = inventory.slot(player_slot) else {
            continue;
        };
        if item
            .components()
            .get("minecraft:attribute_modifiers")
            .is_none()
            && item
                .components()
                .get("!minecraft:attribute_modifiers")
                .is_none()
            && let Some(modifier) =
                default_armor_modifier(item.id(), equipment_slot, attribute_name)
        {
            apply(&mut attribute, modifier)?;
        }
        if let Some(modifiers) = item.components().get("minecraft:attribute_modifiers") {
            let Tag::List { values, .. } = modifiers else {
                anyhow::bail!("item attribute modifiers are not a list");
            };
            for modifier in values {
                let kind = modifier
                    .get("type")
                    .and_then(text)
                    .context("item attribute type is missing")?;
                let kind = ResourceId::parse(&kind)?;
                if kind.namespace != "minecraft" || kind.path != attribute_name {
                    continue;
                }
                let group = match modifier.get("slot") {
                    Some(value) => text(value).context("invalid item attribute slot")?,
                    None => "any".to_owned(),
                };
                ensure!(
                    EquipmentSlot::valid_group(&group),
                    "unknown equipment slot group {group}"
                );
                if !equipment_slot.matches(&group) {
                    continue;
                }
                let id = modifier
                    .get("id")
                    .and_then(text)
                    .context("item modifier id is missing")?;
                let operation = modifier
                    .get("operation")
                    .and_then(text)
                    .context("item modifier operation is missing")?;
                let amount = modifier
                    .get("amount")
                    .and_then(numeric_double)
                    .context("item modifier amount is missing")?;
                ensure!(amount.is_finite(), "item modifier amount is not finite");
                apply(
                    &mut attribute,
                    AttributeModifier {
                        id: ResourceId::parse(&id)?,
                        operation: AttributeOperation::parse(&operation)?,
                        amount,
                    },
                )?;
            }
        }
        if let Some(saved_enchantments) = item.components().get("minecraft:enchantments") {
            let Tag::Compound(entries) = saved_enchantments else {
                anyhow::bail!("item enchantments are not a compound");
            };
            for (id, level) in entries {
                let id = ResourceId::parse(
                    &String::from_utf16(&id.0).context("invalid enchantment id")?,
                )?;
                let level = level.as_java_int().context("invalid enchantment level")?;
                let level = u32::try_from(level).context("invalid enchantment level")?;
                for modifier in
                    enchantments.modifiers_for(&id, level, equipment_slot, attribute_name)?
                {
                    apply(&mut attribute, modifier)?;
                }
            }
        }
    }
    Ok(attribute)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::ItemStack;
    use native_persistence::nbt::TagType;
    use native_resources::packs::ResourceProvider;

    struct Data;
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn list_data(&mut self, _: &str) -> Result<Vec<ResourceId>> {
            Ok(vec![ResourceId::parse("example:breath")?])
        }
        fn read_data(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(Some(br#"{"slots":["head"],"effects":{"minecraft:attributes":[{"id":"example:breath","attribute":"minecraft:oxygen_bonus","operation":"add_value","amount":{"type":"minecraft:linear","base":1.0,"per_level_above_first":1.0}}]}}"#.to_vec()))
        }
    }

    fn item(components: Tag) -> ItemStack {
        ItemStack::unresolved("minecraft:iron_helmet".into(), 1, components).unwrap()
    }
    fn string(value: &str) -> Tag {
        Tag::String(value.into())
    }

    #[test]
    fn knockback_resistance_combines_netherite_and_custom_equipment() {
        let registry = OxygenEnchantments::default();
        let mut inventory = PlayerInventory::default();
        for (slot, name) in [
            (0, "minecraft:netherite_helmet"),
            (1, "minecraft:netherite_chestplate"),
            (2, "minecraft:netherite_leggings"),
            (3, "minecraft:netherite_boots"),
        ] {
            inventory.set_slot(
                PlayerSlot::Armor(slot),
                Some(ItemStack::unresolved(name.into(), 1, Tag::Compound(vec![])).unwrap()),
            );
        }
        assert_eq!(
            knockback_resistance(&SavedAttribute::with_base(0.0), &inventory, &registry).unwrap(),
            f64::from(0.1_f32) * 4.0
        );
        assert_eq!(
            knockback_resistance(&SavedAttribute::with_base(3.0), &inventory, &registry).unwrap(),
            1.0
        );
        inventory.set_slot(
            PlayerSlot::Armor(0),
            Some(
                ItemStack::unresolved(
                    "minecraft:netherite_helmet".into(),
                    1,
                    Tag::Compound(vec![(
                        "!minecraft:attribute_modifiers".into(),
                        Tag::Compound(vec![]),
                    )]),
                )
                .unwrap(),
            ),
        );
        assert_eq!(
            knockback_resistance(&SavedAttribute::with_base(0.0), &inventory, &registry).unwrap(),
            f64::from(0.1_f32) * 3.0
        );
        assert_eq!(
            knockback_resistance(
                &SavedAttribute::with_base(-10.0),
                &PlayerInventory::default(),
                &registry
            )
            .unwrap(),
            -2.0
        );
    }

    #[test]
    fn equipment_and_data_pack_enchantments_combine_before_attribute_clamping() {
        let registry = OxygenEnchantments::load(&mut Data).unwrap();
        let mut saved = SavedAttribute::with_base(2.0);
        saved.apply(AttributeModifier {
            id: ResourceId::parse("example:scale").unwrap(),
            operation: AttributeOperation::AddMultipliedBase,
            amount: 0.5,
        });
        let mut inventory = PlayerInventory::default();
        let enchantment = (
            "minecraft:enchantments".into(),
            Tag::Compound(vec![("example:breath".into(), Tag::Int(3))]),
        );
        let direct = (
            "minecraft:attribute_modifiers".into(),
            Tag::List {
                element_type: TagType::Compound,
                values: vec![Tag::Compound(vec![
                    ("type".into(), string("minecraft:oxygen_bonus")),
                    ("id".into(), string("example:equipment")),
                    ("amount".into(), Tag::Double(1.0)),
                    ("operation".into(), string("add_value")),
                    ("slot".into(), string("head")),
                ])],
            },
        );
        inventory.set_slot(
            PlayerSlot::Armor(0),
            Some(item(Tag::Compound(vec![
                enchantment.clone(),
                direct.clone(),
            ]))),
        );
        assert_eq!(oxygen_bonus(&saved, &inventory, &registry).unwrap(), 9.0);
        inventory.set_slot(PlayerSlot::Armor(0), None);
        inventory.set_slot(
            PlayerSlot::Offhand,
            Some(item(Tag::Compound(vec![enchantment, direct]))),
        );
        assert_eq!(oxygen_bonus(&saved, &inventory, &registry).unwrap(), 3.0);
        inventory.set_slot(
            PlayerSlot::Armor(0),
            Some(item(Tag::Compound(vec![(
                "minecraft:enchantments".into(),
                Tag::Compound(vec![("example:missing".into(), Tag::Int(1))]),
            )]))),
        );
        assert!(oxygen_bonus(&saved, &inventory, &registry).is_err());
    }

    #[test]
    fn data_pack_equipment_bounciness_combines_and_clamps() {
        struct BounceData;
        impl ResourceProvider for BounceData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                if folder == "enchantment" {
                    Ok(vec![ResourceId::parse("example:spring")?])
                } else {
                    Ok(Vec::new())
                }
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                if path == "data/example/enchantment/spring.json" {
                    Ok(Some(br#"{"slots":["head"],"effects":{"minecraft:attributes":[{"id":"example:spring","attribute":"minecraft:bounciness","operation":"add_value","amount":0.75}]}}"#.to_vec()))
                } else {
                    Ok(None)
                }
            }
        }
        let registry = OxygenEnchantments::load(&mut BounceData).unwrap();
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(
            PlayerSlot::Armor(0),
            Some(item(Tag::Compound(vec![(
                "minecraft:enchantments".into(),
                Tag::Compound(vec![("example:spring".into(), Tag::Int(1))]),
            )]))),
        );
        assert_eq!(
            bounciness(&SavedAttribute::with_base(0.5), &inventory, &registry).unwrap(),
            1.0
        );
        inventory.set_slot(PlayerSlot::Armor(0), None);
        assert_eq!(
            bounciness(&SavedAttribute::with_base(0.5), &inventory, &registry).unwrap(),
            0.5
        );
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 assets"]
    fn depth_strider_uses_installed_vanilla_enchantment_data() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft")
            });
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let registry = OxygenEnchantments::load(&mut resources).unwrap();
        for level in 1..=3 {
            let components = Tag::Compound(vec![(
                "minecraft:enchantments".into(),
                Tag::Compound(vec![("minecraft:depth_strider".into(), Tag::Int(level))]),
            )]);
            let boots =
                ItemStack::unresolved("minecraft:iron_boots".into(), 1, components).unwrap();
            let mut inventory = PlayerInventory::default();
            inventory.set_slot(PlayerSlot::Armor(3), Some(boots));
            let actual =
                water_movement_efficiency(&SavedAttribute::with_base(0.0), &inventory, &registry)
                    .unwrap();
            let expected = (f64::from(0.33333334_f32 * level as f32)).min(1.0);
            assert_eq!(actual, expected, "Depth Strider {level}");
        }
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 assets"]
    fn efficiency_and_aqua_affinity_use_installed_enchantment_data() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft")
            });
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let registry = OxygenEnchantments::load(&mut resources).unwrap();
        let enchantment = |name: &str, level| {
            Tag::Compound(vec![(
                "minecraft:enchantments".into(),
                Tag::Compound(vec![(name.into(), Tag::Int(level))]),
            )])
        };
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(
            PlayerSlot::Storage(0),
            ItemStack::new(
                "minecraft:iron_pickaxe".into(),
                1,
                1,
                enchantment("minecraft:efficiency", 5),
            ),
        );
        inventory.set_slot(
            PlayerSlot::Armor(0),
            ItemStack::unresolved(
                "minecraft:iron_helmet".into(),
                1,
                enchantment("minecraft:aqua_affinity", 1),
            ),
        );
        let values = mining_attributes(
            &crate::player_data::SavedMiningAttributes::default(),
            &inventory,
            &registry,
        )
        .unwrap();
        assert_eq!(values, [26.0, 1.0, 1.0]);
        let tool = inventory.slot(PlayerSlot::Storage(0)).unwrap();
        let context = crate::mining::MiningContext {
            mining_efficiency: values[0],
            submerged_mining_speed: values[2],
            eyes_in_water: true,
            ..Default::default()
        };
        let progress = crate::mining::tool_progress_with_context(
            crate::mining::Material::Stone,
            Some(tool),
            context,
        )
        .unwrap();
        assert_eq!(progress, 32.0_f32 / 1.5 / 30.0);
    }

    #[test]
    fn data_pack_movement_enchantments_apply_to_their_declared_attributes() {
        struct MovementData;
        impl ResourceProvider for MovementData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn list_data(&mut self, _: &str) -> Result<Vec<ResourceId>> {
                Ok(vec![ResourceId::parse("example:movement")?])
            }
            fn read_data(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(Some(br#"{"slots":["feet"],"effects":{"minecraft:attributes":[{"id":"example:speed","attribute":"minecraft:movement_speed","operation":"add_value","amount":0.1},{"id":"example:gravity","attribute":"minecraft:gravity","operation":"add_value","amount":0.2},{"id":"example:jump","attribute":"minecraft:jump_strength","operation":"add_value","amount":0.3},{"id":"example:water","attribute":"minecraft:water_movement_efficiency","operation":"add_value","amount":0.4}]}}"#.to_vec()))
            }
        }
        let registry = OxygenEnchantments::load(&mut MovementData).unwrap();
        let components = Tag::Compound(vec![(
            "minecraft:enchantments".into(),
            Tag::Compound(vec![("example:movement".into(), Tag::Int(1))]),
        )]);
        let boots = ItemStack::unresolved("minecraft:iron_boots".into(), 1, components).unwrap();
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(PlayerSlot::Armor(3), Some(boots));
        for (name, expected) in [
            ("movement_speed", 0.1_f32),
            ("gravity", 0.2),
            ("jump_strength", 0.3),
            ("water_movement_efficiency", 0.4),
        ] {
            let attribute =
                equipped_attribute(&SavedAttribute::with_base(0.0), &inventory, &registry, name)
                    .unwrap();
            assert_eq!(
                attribute.value(-1.0, 1.0).unwrap(),
                f64::from(expected),
                "{name}"
            );
        }
    }

    #[test]
    fn armor_uses_data_pack_effects_and_invalidates_unresolved_values() {
        struct ArmorData;
        impl ResourceProvider for ArmorData {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn list_data(&mut self, _: &str) -> Result<Vec<ResourceId>> {
                Ok(vec![ResourceId::parse("example:guard")?])
            }
            fn read_data(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(Some(br#"{"slots":["head"],"effects":{"minecraft:attributes":[{"id":"example:armor","attribute":"minecraft:armor","operation":"add_value","amount":{"type":"minecraft:linear","base":1.0,"per_level_above_first":1.0}},{"id":"example:toughness","attribute":"minecraft:armor_toughness","operation":"add_value","amount":1.5}]}}"#.to_vec()))
            }
        }
        let registry = Arc::new(OxygenEnchantments::load(&mut ArmorData).unwrap());
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(
            PlayerSlot::Armor(0),
            Some(item(Tag::Compound(vec![(
                "minecraft:enchantments".into(),
                Tag::Compound(vec![("example:guard".into(), Tag::Int(3))]),
            )]))),
        );
        let mut tracker = ArmorTracker::default();
        tracker
            .configure(
                SavedAttribute::with_base(1.25),
                SavedAttribute::with_base(0.5),
                registry,
                &inventory,
            )
            .unwrap();
        assert_eq!(
            tracker.value(),
            Some(ArmorValues {
                armor: 6.25,
                toughness: 2.0
            })
        );
        inventory.set_slot(
            PlayerSlot::Armor(0),
            Some(item(Tag::Compound(vec![(
                "minecraft:enchantments".into(),
                Tag::Compound(vec![("example:unknown".into(), Tag::Int(1))]),
            )]))),
        );
        assert!(tracker.refresh(&inventory).is_err());
        assert_eq!(tracker.value(), None);
        inventory.set_slot(PlayerSlot::Armor(0), None);
        tracker.refresh(&inventory).unwrap();
        assert_eq!(tracker.value().unwrap().armor, 1.25);

        let mut conflicting = SavedAttribute::with_base(0.0);
        conflicting.apply(AttributeModifier {
            id: ResourceId::parse("minecraft:armor.helmet").unwrap(),
            operation: AttributeOperation::AddValue,
            amount: 4.0,
        });
        inventory.set_slot(PlayerSlot::Armor(0), Some(item(Tag::Compound(vec![]))));
        assert!(
            tracker
                .configure(
                    conflicting,
                    SavedAttribute::with_base(0.0),
                    Arc::new(OxygenEnchantments::default()),
                    &inventory
                )
                .is_err()
        );
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn java_written_equipment_has_the_same_oxygen_modifiers() {
        use native_resources::packs::ResourceStack;
        use std::{path::PathBuf, process::Command};
        const JAVA: &str = r#"
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.core.registries.*;
import net.minecraft.core.component.DataComponents;
import net.minecraft.nbt.*;
import net.minecraft.resources.Identifier;
import net.minecraft.world.entity.*;
import net.minecraft.world.entity.ai.attributes.*;
import net.minecraft.world.item.*;
import net.minecraft.world.item.component.ItemAttributeModifiers;
import net.minecraft.world.item.enchantment.Enchantments;
import com.google.gson.*;
import java.util.*;
public class EquipmentOracle {
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var world = VanillaRegistries.createWorldLookup();
    var registry = VanillaRegistries.createReloadableLookup(world);
    for (var pending : BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registry)) pending.apply();
    var item = new ItemStack(Items.IRON_HELMET);
    item.enchant(world.lookupOrThrow(Registries.ENCHANTMENT).getOrThrow(Enchantments.RESPIRATION), 3);
    item.set(DataComponents.ATTRIBUTE_MODIFIERS, ItemAttributeModifiers.builder()
      .add(Attributes.OXYGEN_BONUS, new AttributeModifier(Identifier.parse("example:equipment"), 1.5, AttributeModifier.Operation.ADD_VALUE), EquipmentSlotGroup.HEAD).build());
    double[] amount = {0};
    item.forEachModifier(EquipmentSlot.HEAD, (attribute, modifier) -> { if (attribute == Attributes.OXYGEN_BONUS) amount[0] += modifier.amount(); });
    var encoded = ItemStack.CODEC.encodeStart(world.createSerializationContext(NbtOps.INSTANCE), item).getOrThrow();
    var equipment = new CompoundTag(); equipment.put("head", encoded);
    var player = new CompoundTag(); player.putInt("DataVersion", 5023); player.put("equipment", equipment);
    var savedArmor = new AttributeInstance.Packed(Attributes.ARMOR, 1.25, List.of(new AttributeModifier(Identifier.parse("example:base"), 0.5, AttributeModifier.Operation.ADD_MULTIPLIED_BASE)));
    var savedToughness = new AttributeInstance.Packed(Attributes.ARMOR_TOUGHNESS, 0.5, List.of(new AttributeModifier(Identifier.parse("example:total"), 0.25, AttributeModifier.Operation.ADD_MULTIPLIED_TOTAL)));
    player.put("attributes", AttributeInstance.Packed.LIST_CODEC.encodeStart(world.createSerializationContext(NbtOps.INSTANCE), List.of(savedArmor, savedToughness)).getOrThrow());
    NbtIo.write(player, Path.of(args[0]));
    Files.writeString(Path.of(args[1]), Double.toString(amount[0]));
    var names = new ArrayList<String>();
    for (var material : List.of("leather", "golden", "copper", "chainmail", "iron", "diamond", "netherite"))
      for (var piece : List.of("helmet", "chestplate", "leggings", "boots")) names.add(material + "_" + piece);
    names.addAll(List.of("turtle_helmet", "carved_pumpkin", "diamond_sword"));
    var output = new JsonArray();
    for (var name : names) for (var slot : List.of(EquipmentSlot.MAINHAND,EquipmentSlot.OFFHAND,EquipmentSlot.HEAD,EquipmentSlot.CHEST,EquipmentSlot.LEGS,EquipmentSlot.FEET)) for (int patch=0; patch<3; patch++) {
      var stack = new ItemStack(BuiltInRegistries.ITEM.getValue(Identifier.parse("minecraft:" + name)));
      if (patch == 1) stack.set(DataComponents.ATTRIBUTE_MODIFIERS, ItemAttributeModifiers.builder()
        .add(Attributes.ARMOR, new AttributeModifier(Identifier.parse("example:armor"), 4.5, AttributeModifier.Operation.ADD_VALUE), EquipmentSlotGroup.ANY)
        .add(Attributes.ARMOR_TOUGHNESS, new AttributeModifier(Identifier.parse("example:toughness"), 2.5, AttributeModifier.Operation.ADD_VALUE), EquipmentSlotGroup.ANY).build());
      if (patch == 2) stack.remove(DataComponents.ATTRIBUTE_MODIFIERS);
      var armor = new AttributeInstance(Attributes.ARMOR, ignored -> {}); armor.setBaseValue(1.25);
      armor.addPermanentModifier(new AttributeModifier(Identifier.parse("example:base"), 0.5, AttributeModifier.Operation.ADD_MULTIPLIED_BASE));
      var toughness = new AttributeInstance(Attributes.ARMOR_TOUGHNESS, ignored -> {}); toughness.setBaseValue(0.5);
      toughness.addPermanentModifier(new AttributeModifier(Identifier.parse("example:total"), 0.25, AttributeModifier.Operation.ADD_MULTIPLIED_TOTAL));
      var knockback = new AttributeInstance(Attributes.KNOCKBACK_RESISTANCE, ignored -> {}); knockback.setBaseValue(0.0);
      stack.forEachModifier(slot, (attribute, modifier) -> {
        if (attribute == Attributes.ARMOR) armor.addOrUpdateTransientModifier(modifier);
        if (attribute == Attributes.ARMOR_TOUGHNESS) toughness.addOrUpdateTransientModifier(modifier);
        if (attribute == Attributes.KNOCKBACK_RESISTANCE) knockback.addOrUpdateTransientModifier(modifier);
      });
      var row = new JsonArray(); row.add(name); row.add(slot.getName()); row.add(patch); row.add(armor.getValue()); row.add(toughness.getValue()); row.add(knockback.getValue()); output.add(row);
    }
    Files.writeString(Path.of(args[2]), new Gson().toJson(output));
  }
}
"#;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft")
            });
        let mut resources = ResourceStack::open(&game).unwrap();
        let enchantments = OxygenEnchantments::load(&mut resources).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("EquipmentOracle.java");
        let nbt = temp.path().join("player.nbt");
        let expected = temp.path().join("expected.txt");
        let armor_cases = temp.path().join("armor.json");
        std::fs::write(&source, JAVA).unwrap();
        let version = game.join("versions/26.3");
        let metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in metadata["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let run = |command: &mut Command| {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "Java equipment oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-encoding")
            .arg("UTF-8")
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("EquipmentOracle")
            .arg(&nbt)
            .arg(&expected)
            .arg(&armor_cases));
        let document = native_persistence::nbt::read(&std::fs::read(nbt).unwrap()).unwrap();
        let mut inventory = crate::player_data::read_saved_player(&document).unwrap();
        let expected: f64 = std::fs::read_to_string(expected).unwrap().parse().unwrap();
        assert_eq!(expected, 4.5);
        assert_eq!(
            oxygen_bonus(&SavedAttribute::with_base(0.0), &inventory, &enchantments).unwrap(),
            expected
        );
        let original = inventory.slot(PlayerSlot::Armor(0)).unwrap().clone();
        assert_eq!(original.max_count(), Some(1));
        let (armor_base, toughness_base) =
            crate::player_data::read_saved_armor_attributes(&document).unwrap();
        let rows: serde_json::Value =
            serde_json::from_slice(&std::fs::read(armor_cases).unwrap()).unwrap();
        for row in rows.as_array().unwrap() {
            let name = row[0].as_str().unwrap();
            let slot = match row[1].as_str().unwrap() {
                "mainhand" => PlayerSlot::Storage(0),
                "offhand" => PlayerSlot::Offhand,
                "head" => PlayerSlot::Armor(0),
                "chest" => PlayerSlot::Armor(1),
                "legs" => PlayerSlot::Armor(2),
                "feet" => PlayerSlot::Armor(3),
                value => panic!("unexpected slot {value}"),
            };
            let entries = match row[2].as_u64().unwrap() {
                0 => vec![],
                1 => vec![(
                    "minecraft:attribute_modifiers".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: [("armor", 4.5), ("armor_toughness", 2.5)]
                            .into_iter()
                            .map(|(kind, amount)| {
                                Tag::Compound(vec![
                                    ("type".into(), string(&format!("minecraft:{kind}"))),
                                    (
                                        "id".into(),
                                        string(if kind == "armor" {
                                            "example:armor"
                                        } else {
                                            "example:toughness"
                                        }),
                                    ),
                                    ("amount".into(), Tag::Double(amount)),
                                    ("operation".into(), string("add_value")),
                                    ("slot".into(), string("any")),
                                ])
                            })
                            .collect(),
                    },
                )],
                2 => vec![(
                    "!minecraft:attribute_modifiers".into(),
                    Tag::Compound(vec![]),
                )],
                _ => unreachable!(),
            };
            let stack =
                ItemStack::unresolved(format!("minecraft:{name}"), 1, Tag::Compound(entries))
                    .unwrap();
            let mut single = PlayerInventory::default();
            single.set_slot(slot, Some(stack));
            let actual =
                armor_values(&armor_base, &toughness_base, &single, &enchantments).unwrap();
            assert_eq!(actual.armor, row[3].as_f64().unwrap(), "armor {row}");
            assert_eq!(
                actual.toughness,
                row[4].as_f64().unwrap(),
                "toughness {row}"
            );
            assert_eq!(
                knockback_resistance(&SavedAttribute::with_base(0.0), &single, &enchantments)
                    .unwrap(),
                row[5].as_f64().unwrap(),
                "knockback resistance {row}"
            );
        }
        println!(
            "Compared {} armor attribute scenarios with Minecraft 26.3",
            rows.as_array().unwrap().len()
        );
        inventory.set_enchantments(std::sync::Arc::new(enchantments));
        assert!(inventory.toggle());
        assert!(inventory.quick_move(PlayerSlot::Armor(0)));
        assert_eq!(inventory.slot(PlayerSlot::Storage(9)), Some(&original));
        assert!(inventory.toggle());
        let draft = crate::player_data::draft_player_inventory(&document, &inventory).unwrap();
        let restored = crate::player_data::read_saved_player(draft.document()).unwrap();
        assert_eq!(restored.slot(PlayerSlot::Storage(9)), Some(&original));
    }
}
