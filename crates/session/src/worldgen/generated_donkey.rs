//! Intermediate NBT for a freshly generated donkey.
use anyhow::{Result, ensure};
use native_persistence::nbt::{Tag, TagType};
use native_resources::worldgen_region_random::DonkeyInitialization;

/// Serialize a newly constructed and finalized donkey before any live ticks.
/// This protochunk record is not an entity-region document or a world save.
pub fn draft_fresh_donkey(
    uuid: [i32; 4],
    position: [f64; 3],
    yaw: f32,
    initialization: DonkeyInitialization,
) -> Result<Tag> {
    ensure!(
        initialization.max_health.is_finite(),
        "invalid donkey health"
    );
    let Tag::Compound(mut fields) = super::generated_sheep::draft_fresh_animal_base(
        uuid,
        position,
        yaw,
        "minecraft:donkey",
        53.0,
        initialization.animal,
    )?
    else {
        unreachable!("animal draft is always a compound")
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
    values.push(Tag::Compound(vec![
        ("id".into(), Tag::String("minecraft:max_health".into())),
        (
            "base".into(),
            Tag::Double(f64::from(initialization.max_health)),
        ),
    ]));
    fields.extend([
        ("Bred".into(), Tag::Byte(0)),
        ("EatingHaystack".into(), Tag::Byte(0)),
        ("Tame".into(), Tag::Byte(0)),
        ("Temper".into(), Tag::Int(0)),
        ("ChestedHorse".into(), Tag::Byte(0)),
    ]);
    Ok(Tag::Compound(fields))
}
