//! An NBT draft for a newly constructed, otherwise unmodified sheep.
use anyhow::{Context, Result, ensure};
use native_persistence::nbt::{Tag, TagType};
use native_resources::worldgen_region_random::{
    AnimalBaseInitialization, SheepInitialization, SheepSpawnColor,
};

/// Serialize the vanilla defaults and verified spawn-finalization changes.
/// This is valid only for a fresh sheep with no equipment, existing modifiers,
/// passengers, custom data, damage, effects, or other entity modifications.
/// The UUID comes from entity construction, independently of the region RNG.
/// The caller must still coordinate chunk ownership and all world changes.
pub fn draft_fresh_sheep(
    uuid: [i32; 4],
    position: [f64; 3],
    yaw: f32,
    initialization: SheepInitialization,
) -> Result<Tag> {
    let color = match initialization.color {
        SheepSpawnColor::White => 0,
        SheepSpawnColor::Pink => 6,
        SheepSpawnColor::Gray => 7,
        SheepSpawnColor::LightGray => 8,
        SheepSpawnColor::Brown => 12,
        SheepSpawnColor::Black => 15,
    };
    let Tag::Compound(mut fields) = draft_fresh_animal_base(
        uuid,
        position,
        yaw,
        "minecraft:sheep",
        8.0,
        initialization.animal,
    )?
    else {
        unreachable!("animal draft is always a compound")
    };
    fields.push(("Sheared".into(), Tag::Byte(0)));
    fields.push(("Color".into(), Tag::Byte(color)));
    Ok(Tag::Compound(fields))
}

/// Shared saved defaults for the verified, newly constructed farm animals.
pub(super) fn draft_fresh_animal_base(
    uuid: [i32; 4],
    position: [f64; 3],
    yaw: f32,
    id: &'static str,
    health: f32,
    animal: AnimalBaseInitialization,
) -> Result<Tag> {
    ensure!(
        position.iter().all(|v| v.is_finite())
            && position[0].abs() <= 30_000_512.0
            && position[2].abs() <= 30_000_512.0
            && position[1].abs() <= 20_000_000.0,
        "animal position would be clamped"
    );
    ensure!(yaw.is_finite(), "invalid animal rotation");
    let bonus = animal
        .follow_range_bonus
        .context("fresh animal is missing its follow-range bonus")?;
    ensure!(bonus.is_finite(), "invalid follow-range bonus");
    let compound = |fields: Vec<(&str, Tag)>| {
        Tag::Compound(
            fields
                .into_iter()
                .map(|(name, value)| (name.into(), value))
                .collect(),
        )
    };
    let list = |element_type, values| Tag::List {
        element_type,
        values,
    };
    let modifier = compound(vec![
        ("id", Tag::String("minecraft:random_spawn_bonus".into())),
        ("amount", Tag::Double(bonus)),
        ("operation", Tag::String("add_multiplied_base".into())),
    ]);
    let attribute = compound(vec![
        ("id", Tag::String("minecraft:follow_range".into())),
        ("base", Tag::Double(16.0)),
        ("modifiers", list(TagType::Compound, vec![modifier])),
    ]);
    Ok(compound(vec![
        ("id", Tag::String(id.into())),
        ("UUID", Tag::IntArray(uuid.to_vec())),
        (
            "Pos",
            list(
                TagType::Double,
                position.into_iter().map(Tag::Double).collect(),
            ),
        ),
        ("Motion", list(TagType::Double, vec![Tag::Double(0.0); 3])),
        (
            "Rotation",
            list(TagType::Float, vec![Tag::Float(yaw), Tag::Float(0.0)]),
        ),
        ("fall_distance", Tag::Double(0.0)),
        ("Fire", Tag::Short(0)),
        ("Air", Tag::Short(300)),
        ("OnGround", Tag::Byte(0)),
        ("Invulnerable", Tag::Byte(0)),
        ("PortalCooldown", Tag::Int(0)),
        ("Health", Tag::Float(health)),
        ("HurtTime", Tag::Short(0)),
        ("DeathTime", Tag::Short(0)),
        ("AbsorptionAmount", Tag::Float(0.0)),
        ("current_impulse_context_reset_grace_time", Tag::Int(0)),
        ("attributes", list(TagType::Compound, vec![attribute])),
        ("FallFlying", Tag::Byte(0)),
        ("Brain", compound(vec![("memories", compound(vec![]))])),
        ("CanPickUpLoot", Tag::Byte(0)),
        ("PersistenceRequired", Tag::Byte(0)),
        ("LeftHanded", Tag::Byte(i8::from(animal.left_handed))),
        (
            "Age",
            Tag::Int(if animal.set_baby_age { -24_000 } else { 0 }),
        ),
        ("ForcedAge", Tag::Int(0)),
        ("AgeLocked", Tag::Byte(0)),
        ("InLove", Tag::Int(0)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_resources::worldgen_region_random::AnimalBaseInitialization;

    #[test]
    fn refuses_unverified_or_unrepresentable_fresh_state() {
        let mut initialization = SheepInitialization {
            color: SheepSpawnColor::White,
            animal: AnimalBaseInitialization {
                set_baby_age: false,
                follow_range_bonus: Some(0.0),
                left_handed: false,
            },
        };
        assert!(draft_fresh_sheep([0; 4], [0.0; 3], f32::NAN, initialization).is_err());
        assert!(draft_fresh_sheep([0; 4], [f64::NAN, 0.0, 0.0], 0.0, initialization).is_err());
        initialization.animal.follow_range_bonus = None;
        assert!(draft_fresh_sheep([0; 4], [0.0; 3], 0.0, initialization).is_err());
        initialization.animal.follow_range_bonus = Some(f64::INFINITY);
        assert!(draft_fresh_sheep([0; 4], [0.0; 3], 0.0, initialization).is_err());
    }
}
