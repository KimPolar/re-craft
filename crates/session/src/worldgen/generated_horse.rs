//! Intermediate NBT for a freshly generated horse.
use anyhow::{Result, ensure};
use native_persistence::nbt::{Tag, TagType};
use native_resources::worldgen_region_random::HorseInitialization;

/// Serialize a newly constructed and finalized horse, before any live ticks.
/// This is a protochunk record, not an entity-region document or a world save.
pub fn draft_fresh_horse(
    uuid: [i32; 4],
    position: [f64; 3],
    yaw: f32,
    initialization: HorseInitialization,
) -> Result<Tag> {
    ensure!(
        initialization.variant < 7 && initialization.markings < 5,
        "invalid horse appearance"
    );
    ensure!(
        initialization.max_health.is_finite()
            && initialization.movement_speed.is_finite()
            && initialization.jump_strength.is_finite(),
        "invalid horse attributes"
    );
    let Tag::Compound(mut fields) = super::generated_sheep::draft_fresh_animal_base(
        uuid,
        position,
        yaw,
        "minecraft:horse",
        53.0,
        initialization.animal,
    )?
    else {
        unreachable!("animal draft is always a compound")
    };
    let attribute = |id: &str, base: f64| {
        Tag::Compound(vec![
            ("id".into(), Tag::String(id.into())),
            ("base".into(), Tag::Double(base)),
        ])
    };
    let Some((
        _,
        Tag::List {
            element_type: TagType::Compound,
            values,
        },
    )) = fields
        .iter_mut()
        .find(|(name, _)| name.matches("attributes"))
    else {
        unreachable!("animal base has an attribute list")
    };
    values.insert(
        0,
        attribute("minecraft:movement_speed", initialization.movement_speed),
    );
    values.insert(
        0,
        attribute("minecraft:jump_strength", initialization.jump_strength),
    );
    values.push(attribute(
        "minecraft:max_health",
        f64::from(initialization.max_health),
    ));
    fields.extend([
        ("Bred".into(), Tag::Byte(0)),
        ("EatingHaystack".into(), Tag::Byte(0)),
        ("Tame".into(), Tag::Byte(0)),
        ("Temper".into(), Tag::Int(0)),
        (
            "Variant".into(),
            Tag::Int(i32::from(initialization.variant) | (i32::from(initialization.markings) << 8)),
        ),
    ]);
    Ok(Tag::Compound(fields))
}
