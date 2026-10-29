//! Check the active 26.3 built-in damage tags and protection enchantments.

use native_resources::{
    enchantments::{EquipmentSlot, OxygenEnchantments},
    packs::{ResourceId, ResourceStack},
};
use std::path::PathBuf;

#[test]
#[ignore = "requires the user's Minecraft Java Edition 26.3 installation"]
fn vanilla_protection_enchantments_match_fall_damage_types() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let mut resources = ResourceStack::open(&game).unwrap();
    let registry = OxygenEnchantments::load(&mut resources).unwrap();
    assert!(!registry.fall_damage_immune("minecraft:player").unwrap());
    for damage in ["minecraft:fall", "minecraft:stalagmite"] {
        assert!(
            registry
                .damage_type_has_tag("minecraft:is_fall", damage)
                .unwrap()
        );
        assert!(
            registry
                .damage_type_has_tag("minecraft:bypasses_armor", damage)
                .unwrap()
        );
        assert!(
            !registry
                .damage_type_has_tag("minecraft:bypasses_effects", damage)
                .unwrap()
        );
        for (name, per_level) in [
            ("feather_falling", 3.0),
            ("protection", 1.0),
            ("fire_protection", 0.0),
            ("blast_protection", 0.0),
            ("projectile_protection", 0.0),
        ] {
            let id = ResourceId::parse(&format!("minecraft:{name}")).unwrap();
            for level in 1..=4 {
                assert_eq!(
                    registry
                        .protect_damage(&id, level, EquipmentSlot::Feet, damage, 0.0)
                        .unwrap(),
                    per_level * level as f32,
                    "{name}, {damage}, level {level}"
                );
            }
        }
    }
}
