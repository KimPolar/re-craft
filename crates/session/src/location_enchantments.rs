//! Stateful location enchantment effects for one player's equipment.

use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use native_core::random::LegacyRandom;
use native_resources::enchantments::{
    AttributeModifier, EquipmentSlot, OxygenEnchantments,
    location::{LocationAction, LocationContext},
};
use native_resources::packs::ResourceId;

use crate::inventory::{ItemStack, PlayerInventory, PlayerSlot, item_enchantments};
use crate::player_data::SavedAttribute;

const SLOTS: [(EquipmentSlot, PlayerSlot); 5] = [
    (EquipmentSlot::Offhand, PlayerSlot::Offhand),
    (EquipmentSlot::Feet, PlayerSlot::Armor(3)),
    (EquipmentSlot::Legs, PlayerSlot::Armor(2)),
    (EquipmentSlot::Chest, PlayerSlot::Armor(1)),
    (EquipmentSlot::Head, PlayerSlot::Armor(0)),
];

#[derive(Clone, Debug, Default)]
struct ActiveEffect {
    modifiers: Vec<(ResourceId, AttributeModifier)>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct LocationTracker {
    equipment: [Option<ItemStack>; 6],
    active: BTreeMap<(usize, ResourceId, usize), ActiveEffect>,
}

#[derive(Clone, Copy)]
pub(crate) struct Environment<'a> {
    pub block: &'a str,
    pub flying: bool,
    pub on_ground: bool,
    pub passenger: bool,
    pub infinite_materials: bool,
}

#[derive(Default)]
pub(crate) struct Outcome {
    pub attributes_changed: bool,
    pub inventory_changed: bool,
    pub broken: Vec<ItemStack>,
    pub item_changes: Vec<ItemChange>,
    pub touched_modifiers: Vec<(ResourceId, ResourceId)>,
}

pub(crate) struct ItemChange {
    pub before: ItemStack,
    pub after: Option<ItemStack>,
}

fn slots(inventory: &PlayerInventory) -> [(EquipmentSlot, PlayerSlot); 6] {
    [
        (
            EquipmentSlot::Mainhand,
            PlayerSlot::Storage(inventory.selected()),
        ),
        SLOTS[0],
        SLOTS[1],
        SLOTS[2],
        SLOTS[3],
        SLOTS[4],
    ]
}

/// Java removes an existing modifier with the same id before applying a
/// transient one. Until those replacements are persisted, reject such saves.
pub(crate) fn validate_save_modifiers(
    outcome: &Outcome,
    before: &PlayerInventory,
    after: &PlayerInventory,
    registry: &OxygenEnchantments,
    speed: &SavedAttribute,
    efficiency: &SavedAttribute,
) -> Result<()> {
    for (attribute, id) in &outcome.touched_modifiers {
        let base = match (attribute.namespace.as_str(), attribute.path.as_str()) {
            ("minecraft", "movement_speed") => speed,
            ("minecraft", "movement_efficiency") => efficiency,
            _ => anyhow::bail!("unsupported location attribute in save history"),
        };
        for inventory in [before, after] {
            let assembled = crate::equipment_attributes::equipped_attribute(
                base,
                inventory,
                registry,
                &attribute.path,
            )?;
            ensure!(
                !assembled.contains_modifier(id),
                "location modifier replaced an existing attribute modifier"
            );
        }
    }
    Ok(())
}

impl LocationTracker {
    pub fn has_active(&self) -> bool {
        !self.active.is_empty()
    }

    pub fn validate_save_attributes(
        &self,
        inventory: &PlayerInventory,
        registry: &OxygenEnchantments,
        speed: &SavedAttribute,
        efficiency: &SavedAttribute,
    ) -> Result<()> {
        for (name, base) in [
            ("movement_speed", speed),
            ("movement_efficiency", efficiency),
        ] {
            let assembled =
                crate::equipment_attributes::equipped_attribute(base, inventory, registry, name)?;
            self.apply(name, assembled)?;
        }
        Ok(())
    }

    pub fn equipment_changed(&self, inventory: &PlayerInventory) -> bool {
        slots(inventory)
            .into_iter()
            .enumerate()
            .any(|(index, (_, slot))| self.equipment[index].as_ref() != inventory.slot(slot))
    }
    pub fn needs_processing(&self, inventory: &PlayerInventory) -> bool {
        !self.active.is_empty()
            || self
                .equipment
                .iter()
                .flatten()
                .any(|item| item.components().get("minecraft:enchantments").is_some())
            || slots(inventory).iter().any(|(_, slot)| {
                inventory
                    .slot(*slot)
                    .is_some_and(|item| item.components().get("minecraft:enchantments").is_some())
            })
    }

    fn stop_slot(&mut self, slot: usize, outcome: &mut Outcome) {
        self.active.retain(|(index, _, _), effect| {
            if *index != slot {
                return true;
            }
            outcome.attributes_changed |= !effect.modifiers.is_empty();
            outcome.touched_modifiers.extend(
                effect
                    .modifiers
                    .iter()
                    .map(|(attribute, modifier)| (attribute.clone(), modifier.id.clone())),
            );
            false
        });
    }

    fn run_slot(
        &mut self,
        index: usize,
        inventory: &mut PlayerInventory,
        registry: &OxygenEnchantments,
        environment: Environment<'_>,
        random: &mut LegacyRandom,
        outcome: &mut Outcome,
    ) -> Result<()> {
        let (equipment, slot) = slots(inventory)[index];
        let Some(item) = inventory.slot(slot).cloned() else {
            return Ok(());
        };
        let mut effectful = 0;
        for (id, level) in item_enchantments(&item)? {
            let definition = registry.location_effects(&id)?;
            if definition.is_empty() {
                continue;
            }
            effectful += 1;
            ensure!(
                effectful == 1,
                "multiple location enchantment iteration order is unknown"
            );
            for effect in 0..definition.len() {
                let key = (index, id.clone(), effect);
                let previous = self.active.contains_key(&key);
                let context = LocationContext {
                    active: previous,
                    flying: environment.flying,
                    on_ground: environment.on_ground,
                    vehicle: environment.passenger,
                    movement_block: environment.block,
                    level,
                    tick_count: 0,
                    known_movement: [0.0; 3],
                };
                let (active, actions) =
                    definition
                        .evaluate(effect, equipment, &context, &mut || random.next_float())?;
                let mut state = self.active.remove(&key).unwrap_or_default();
                for action in actions {
                    match action {
                        LocationAction::AddAttribute {
                            attribute,
                            modifier,
                        } => {
                            ensure!(
                                attribute.namespace == "minecraft"
                                    && matches!(
                                        attribute.path.as_str(),
                                        "movement_speed" | "movement_efficiency"
                                    ),
                                "unsupported location attribute {}:{}",
                                attribute.namespace,
                                attribute.path
                            );
                            ensure!(
                                !state.modifiers.iter().any(
                                    |(old, value)| old == &attribute && value.id == modifier.id
                                ),
                                "duplicate location attribute modifier"
                            );
                            outcome
                                .touched_modifiers
                                .push((attribute.clone(), modifier.id.clone()));
                            state.modifiers.push((attribute, modifier));
                            outcome.attributes_changed = true;
                        }
                        LocationAction::RemoveAttribute {
                            attribute,
                            modifier,
                        } => {
                            outcome
                                .touched_modifiers
                                .push((attribute.clone(), modifier.id.clone()));
                            state.modifiers.retain(|(old, value)| {
                                old != &attribute || value.id != modifier.id
                            });
                            outcome.attributes_changed = true;
                        }
                        LocationAction::ChangeItemDamage(amount) => {
                            let before = inventory
                                .slot(slot)
                                .cloned()
                                .context("location effect item vanished")?;
                            if inventory.change_equipped_item_damage(
                                slot,
                                amount as i32,
                                environment.infinite_materials,
                                random,
                            )? {
                                outcome.inventory_changed = true;
                                outcome.item_changes.push(ItemChange {
                                    before: before.clone(),
                                    after: inventory.slot(slot).cloned(),
                                });
                                if inventory.slot(slot).is_none() {
                                    outcome.broken.push(before);
                                    outcome.attributes_changed |= !state.modifiers.is_empty();
                                    self.stop_slot(index, outcome);
                                    return Ok(());
                                }
                            }
                        }
                    }
                }
                if active {
                    self.active.insert(key, state);
                }
            }
        }
        Ok(())
    }

    /// Run an event transactionally. A failed definition must not consume RNG,
    /// alter the inventory, or retain only part of the transient attributes.
    fn transaction(
        &mut self,
        inventory: &mut PlayerInventory,
        random: &mut LegacyRandom,
        f: impl FnOnce(&mut Self, &mut PlayerInventory, &mut LegacyRandom, &mut Outcome) -> Result<()>,
    ) -> Result<Outcome> {
        let mut tracker = self.clone();
        let mut next_inventory = inventory.clone();
        let mut next_random = random.clone();
        let mut outcome = Outcome::default();
        f(
            &mut tracker,
            &mut next_inventory,
            &mut next_random,
            &mut outcome,
        )?;
        *self = tracker;
        *inventory = next_inventory;
        *random = next_random;
        Ok(outcome)
    }

    pub fn block_changed(
        &mut self,
        inventory: &mut PlayerInventory,
        registry: &OxygenEnchantments,
        environment: Environment<'_>,
        random: &mut LegacyRandom,
    ) -> Result<Outcome> {
        self.transaction(inventory, random, |tracker, inventory, random, outcome| {
            for index in 0..6 {
                tracker.run_slot(index, inventory, registry, environment, random, outcome)?;
            }
            Ok(())
        })
    }

    pub fn sync_equipment(
        &mut self,
        inventory: &mut PlayerInventory,
        registry: &OxygenEnchantments,
        environment: Environment<'_>,
        random: &mut LegacyRandom,
    ) -> Result<Outcome> {
        if !self.equipment_changed(inventory) {
            return Ok(Outcome::default());
        }
        let current = slots(inventory).map(|(_, slot)| inventory.slot(slot).cloned());
        self.transaction(inventory, random, |tracker, inventory, random, outcome| {
            let changed: Vec<_> = (0..6)
                .filter(|&index| tracker.equipment[index] != current[index])
                .collect();
            for &index in &changed {
                tracker.stop_slot(index, outcome);
            }
            tracker.equipment = current;
            for index in changed {
                tracker.run_slot(index, inventory, registry, environment, random, outcome)?;
            }
            Ok(())
        })
    }

    pub fn apply(&self, name: &str, mut attribute: SavedAttribute) -> Result<SavedAttribute> {
        for effect in self.active.values() {
            for (kind, modifier) in &effect.modifiers {
                if kind.namespace == "minecraft" && kind.path == name {
                    ensure!(
                        !attribute.contains_modifier(&modifier.id),
                        "location modifier conflicts with another attribute"
                    );
                    attribute.apply(modifier.clone());
                }
            }
        }
        Ok(attribute)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_persistence::nbt::Tag;
    use native_resources::packs::ResourceProvider;
    use std::sync::Arc;

    struct Pack;
    impl ResourceProvider for Pack {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
            Ok(if folder == "enchantment" {
                vec![ResourceId::parse("example:location")?]
            } else {
                vec![]
            })
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok((path == "data/example/enchantment/location.json").then(|| br#"{"slots":["feet"],"effects":{"minecraft:location_changed":[{"effect":{"type":"minecraft:attribute","attribute":"minecraft:movement_efficiency","id":"example:effect","operation":"add_value","amount":1.0},"requirements":{"type":"minecraft:entity_properties","entity":"this","predicate":{"minecraft:movement_affected_by":{"block":{"blocks":"minecraft:soul_sand"}}}}}]}}"#.to_vec()))
        }
    }
    fn boots() -> ItemStack {
        ItemStack::new(
            "minecraft:iron_boots".into(),
            1,
            1,
            Tag::Compound(vec![(
                "minecraft:enchantments".into(),
                Tag::Compound(vec![("example:location".into(), Tag::Int(1))]),
            )]),
        )
        .unwrap()
    }
    fn env(block: &str) -> Environment<'_> {
        Environment {
            block,
            flying: false,
            on_ground: true,
            passenger: false,
            infinite_materials: false,
        }
    }

    #[test]
    fn save_rejects_location_modifier_replacing_persistent_attribute() {
        use native_resources::enchantments::AttributeOperation;
        let id = ResourceId::parse("example:effect").unwrap();
        let outcome = Outcome {
            touched_modifiers: vec![(
                ResourceId::parse("minecraft:movement_speed").unwrap(),
                id.clone(),
            )],
            ..Default::default()
        };
        let mut speed = SavedAttribute::with_base(0.1);
        let efficiency = SavedAttribute::with_base(0.0);
        let inventory = PlayerInventory::default();
        let registry = OxygenEnchantments::default();
        assert!(
            validate_save_modifiers(
                &outcome,
                &inventory,
                &inventory,
                &registry,
                &speed,
                &efficiency
            )
            .is_ok()
        );
        speed.apply(AttributeModifier {
            id,
            operation: AttributeOperation::AddValue,
            amount: 0.5,
        });
        assert!(
            validate_save_modifiers(
                &outcome,
                &inventory,
                &inventory,
                &registry,
                &speed,
                &efficiency
            )
            .is_err()
        );
    }

    #[test]
    fn slot_changes_activate_remove_and_preserve_transient_attributes() {
        let registry = Arc::new(OxygenEnchantments::load(&mut Pack).unwrap());
        let mut inventory = PlayerInventory::default();
        inventory.set_enchantments(Arc::clone(&registry));
        inventory.set_slot(PlayerSlot::Armor(3), Some(boots()));
        let mut random = LegacyRandom::new(0);
        let mut tracker = LocationTracker::default();
        assert!(
            tracker
                .block_changed(
                    &mut inventory,
                    &registry,
                    env("minecraft:soul_sand"),
                    &mut random
                )
                .unwrap()
                .attributes_changed
        );
        assert_eq!(
            tracker
                .apply("movement_efficiency", SavedAttribute::with_base(0.0))
                .unwrap()
                .value(0.0, 1.0)
                .unwrap(),
            1.0
        );
        assert!(
            tracker
                .sync_equipment(
                    &mut inventory,
                    &registry,
                    env("minecraft:soul_sand"),
                    &mut random
                )
                .unwrap()
                .attributes_changed
        );
        assert!(
            !tracker
                .sync_equipment(
                    &mut inventory,
                    &registry,
                    env("minecraft:soul_sand"),
                    &mut random
                )
                .unwrap()
                .attributes_changed
        );
        assert!(
            tracker
                .block_changed(
                    &mut inventory,
                    &registry,
                    env("minecraft:stone"),
                    &mut random
                )
                .unwrap()
                .attributes_changed
        );
        assert_eq!(
            tracker
                .apply("movement_efficiency", SavedAttribute::with_base(0.0))
                .unwrap()
                .value(0.0, 1.0)
                .unwrap(),
            0.0
        );
        tracker
            .block_changed(
                &mut inventory,
                &registry,
                env("minecraft:soul_sand"),
                &mut random,
            )
            .unwrap();
        inventory.set_slot(PlayerSlot::Armor(3), None);
        assert!(
            tracker
                .sync_equipment(
                    &mut inventory,
                    &registry,
                    env("minecraft:soul_sand"),
                    &mut random
                )
                .unwrap()
                .attributes_changed
        );
        assert_eq!(
            tracker
                .apply("movement_efficiency", SavedAttribute::with_base(0.0))
                .unwrap()
                .value(0.0, 1.0)
                .unwrap(),
            0.0
        );
    }

    #[test]
    fn missing_enchantment_does_not_change_equipment_or_rng() {
        let registry = OxygenEnchantments::default();
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(PlayerSlot::Armor(3), Some(boots()));
        let before = inventory.clone();
        let mut random = LegacyRandom::new(22);
        let before_random = random.state();
        let mut tracker = LocationTracker::default();
        assert!(
            tracker
                .block_changed(
                    &mut inventory,
                    &registry,
                    env("minecraft:soul_sand"),
                    &mut random
                )
                .is_err()
        );
        assert_eq!(inventory, before);
        assert_eq!(random.state(), before_random);
        assert!(tracker.active.is_empty());
    }

    #[test]
    fn late_unsupported_effect_rolls_back_prior_damage_and_random() {
        struct Unsupported;
        impl ResourceProvider for Unsupported {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
                Ok(if folder == "enchantment" {
                    vec![ResourceId::parse("example:location")?]
                } else {
                    vec![]
                })
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                Ok((path == "data/example/enchantment/location.json").then(|| br#"{"slots":["feet"],"effects":{"minecraft:location_changed":[{"effect":{"type":"minecraft:change_item_damage","amount":1.0},"requirements":{"type":"minecraft:random_chance","chance":1.0}},{"effect":{"type":"minecraft:attribute","attribute":"minecraft:gravity","id":"example:gravity","operation":"add_value","amount":1.0}}]}}"#.to_vec()))
            }
        }
        let registry = Arc::new(OxygenEnchantments::load(&mut Unsupported).unwrap());
        let mut inventory = PlayerInventory::default();
        inventory.set_enchantments(Arc::clone(&registry));
        inventory.set_slot(PlayerSlot::Armor(3), Some(boots()));
        let before = inventory.clone();
        let mut random = LegacyRandom::new(22);
        let before_random = random.state();
        let mut tracker = LocationTracker::default();
        assert!(
            tracker
                .block_changed(
                    &mut inventory,
                    &registry,
                    env("minecraft:soul_sand"),
                    &mut random
                )
                .is_err()
        );
        assert_eq!(inventory, before);
        assert_eq!(random.state(), before_random);
        assert!(tracker.active.is_empty());
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn soul_speed_event_sequences_match_java() {
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
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("LocationEffectSequenceOracle.java");
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/LocationEffectSequenceOracle.java"),
        )
        .unwrap();
        let steps: Vec<_> = [("soul_sand",true,false,false), ("stone",false,false,false),
            ("stone",true,false,false), ("soul_sand",true,false,false), ("soul_sand",true,false,false),
            ("soul_sand",true,true,false), ("soul_sand",true,false,true), ("soul_sand",true,false,false)]
            .into_iter().map(|(block, ground, flying, passenger)| serde_json::json!({"block":block,"ground":ground,"flying":flying,"passenger":passenger})).collect();
        let scenarios: Vec<_> = [0, 1, 4096, 12345]
            .into_iter()
            .flat_map(|seed| {
                [1, 2, 3].into_iter().map({
                    let steps = steps.clone();
                    move |level| serde_json::json!({"seed":seed,"level":level,"steps":steps})
                })
            })
            .collect();
        std::fs::write(&input, serde_json::to_vec(&scenarios).unwrap()).unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("LocationEffectSequenceOracle")
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
        let expected: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(expected.len(), scenarios.len());
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let registry = Arc::new(OxygenEnchantments::load(&mut resources).unwrap());
        for (scenario, expected) in scenarios.iter().zip(expected) {
            let level = scenario["level"].as_i64().unwrap() as i32;
            let mut inventory = PlayerInventory::default();
            inventory.set_enchantments(Arc::clone(&registry));
            inventory.set_slot(
                PlayerSlot::Armor(3),
                ItemStack::new(
                    "minecraft:iron_boots".into(),
                    1,
                    1,
                    Tag::Compound(vec![(
                        "minecraft:enchantments".into(),
                        Tag::Compound(vec![("minecraft:soul_speed".into(), Tag::Int(level))]),
                    )]),
                ),
            );
            let mut random = LegacyRandom::new(scenario["seed"].as_i64().unwrap());
            let mut tracker = LocationTracker::default();
            for (step, expected) in scenario["steps"]
                .as_array()
                .unwrap()
                .iter()
                .zip(expected.as_array().unwrap())
            {
                let block = format!("minecraft:{}", step["block"].as_str().unwrap());
                let environment = Environment {
                    block: &block,
                    flying: step["flying"].as_bool().unwrap(),
                    on_ground: step["ground"].as_bool().unwrap(),
                    passenger: step["passenger"].as_bool().unwrap(),
                    infinite_materials: false,
                };
                tracker
                    .block_changed(&mut inventory, &registry, environment, &mut random)
                    .unwrap();
                let speed = tracker
                    .apply(
                        "movement_speed",
                        SavedAttribute::with_base(f64::from(0.1_f32)),
                    )
                    .unwrap()
                    .value(0.0, 1024.0)
                    .unwrap();
                let efficiency = tracker
                    .apply("movement_efficiency", SavedAttribute::with_base(0.0))
                    .unwrap()
                    .value(0.0, 1.0)
                    .unwrap();
                let damage = inventory
                    .slot(PlayerSlot::Armor(3))
                    .unwrap()
                    .components()
                    .get("minecraft:damage")
                    .and_then(Tag::as_java_int)
                    .unwrap_or(0);
                assert_eq!(
                    speed.to_bits(),
                    expected["speed"].as_f64().unwrap().to_bits(),
                    "{scenario} {step}"
                );
                assert_eq!(
                    efficiency,
                    expected["efficiency"].as_f64().unwrap(),
                    "{scenario} {step}"
                );
                assert_eq!(
                    damage,
                    expected["damage"].as_i64().unwrap() as i32,
                    "{scenario} {step}"
                );
                assert_eq!(
                    random.next_int(),
                    expected["next"].as_i64().unwrap() as i32,
                    "{scenario} {step}"
                );
            }
        }
    }

    #[test]
    #[ignore = "requires the installed Minecraft 26.3 assets"]
    fn broken_soul_speed_boots_remove_their_active_modifiers() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let registry = Arc::new(OxygenEnchantments::load(&mut resources).unwrap());
        let mut inventory = PlayerInventory::default();
        inventory.set_enchantments(Arc::clone(&registry));
        inventory.set_slot(
            PlayerSlot::Armor(3),
            ItemStack::new(
                "minecraft:iron_boots".into(),
                1,
                1,
                Tag::Compound(vec![
                    ("minecraft:damage".into(), Tag::Int(194)),
                    (
                        "minecraft:enchantments".into(),
                        Tag::Compound(vec![("minecraft:soul_speed".into(), Tag::Int(1))]),
                    ),
                ]),
            ),
        );
        let mut tracker = LocationTracker::default();
        let mut random = LegacyRandom::new(5120);
        let outcome = tracker
            .block_changed(
                &mut inventory,
                &registry,
                env("minecraft:soul_sand"),
                &mut random,
            )
            .unwrap();
        assert!(outcome.inventory_changed && outcome.attributes_changed);
        assert_eq!(outcome.broken.len(), 1);
        assert!(inventory.slot(PlayerSlot::Armor(3)).is_none());
        assert!(tracker.active.is_empty());
        assert_eq!(
            tracker
                .apply("movement_efficiency", SavedAttribute::with_base(0.0))
                .unwrap()
                .value(0.0, 1.0)
                .unwrap(),
            0.0
        );
    }
}
