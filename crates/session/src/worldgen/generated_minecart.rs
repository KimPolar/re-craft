//! NBT for an unmodified chest minecart created by a fresh mineshaft.
use anyhow::{Result, ensure};
use native_persistence::nbt::{Tag, TagType};

/// The UUID comes from the entity's own random source, not the decoration RNG.
/// Position must already be the Java setInitialPos center of the rail block.
pub fn draft_fresh_mineshaft_chest_minecart(
    uuid: [i32; 4],
    position: [f64; 3],
    loot_seed: i64,
) -> Result<Tag> {
    ensure!(
        position.iter().all(|v| v.is_finite())
            && position[0].abs() <= 30_000_512.0
            && position[2].abs() <= 30_000_512.0
            && position[1].abs() <= 20_000_000.0,
        "minecart position would be clamped"
    );
    let mut fields = vec![
        ("id".into(), Tag::String("minecraft:chest_minecart".into())),
        ("UUID".into(), Tag::IntArray(uuid.to_vec())),
        (
            "Pos".into(),
            Tag::List {
                element_type: TagType::Double,
                values: position.into_iter().map(Tag::Double).collect(),
            },
        ),
        (
            "Motion".into(),
            Tag::List {
                element_type: TagType::Double,
                values: vec![Tag::Double(0.0); 3],
            },
        ),
        (
            "Rotation".into(),
            Tag::List {
                element_type: TagType::Float,
                values: vec![Tag::Float(0.0); 2],
            },
        ),
        ("fall_distance".into(), Tag::Double(0.0)),
        ("Fire".into(), Tag::Short(0)),
        ("Air".into(), Tag::Short(300)),
        ("OnGround".into(), Tag::Byte(0)),
        ("Invulnerable".into(), Tag::Byte(0)),
        ("PortalCooldown".into(), Tag::Int(0)),
        ("HasTicked".into(), Tag::Byte(1)),
        ("FlippedRotation".into(), Tag::Byte(0)),
        (
            "LootTable".into(),
            Tag::String("minecraft:chests/abandoned_mineshaft".into()),
        ),
    ];
    if loot_seed != 0 {
        fields.push(("LootTableSeed".into(), Tag::Long(loot_seed)));
    }
    Ok(Tag::Compound(fields))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_clamped_positions_and_omits_zero_loot_seed() {
        assert!(draft_fresh_mineshaft_chest_minecart([0; 4], [f64::NAN, 0.0, 0.0], 0).is_err());
        assert!(draft_fresh_mineshaft_chest_minecart([0; 4], [30_000_513.0, 0.0, 0.0], 0).is_err());
        let tag = draft_fresh_mineshaft_chest_minecart([0; 4], [0.5, 0.5, 0.5], 0).unwrap();
        assert!(tag.get("LootTableSeed").is_none());
    }
}
