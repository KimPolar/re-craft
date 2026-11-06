//! Saved state for a newly finalized zombie nautilus.
use anyhow::{Result, ensure};
use native_persistence::nbt::Tag;
use native_resources::animal_variants::ZombieNautilusInitialization;

/// Draft an adult created for a structure drowned, before adding passengers.
pub fn draft_fresh_structure_zombie_nautilus(
    uuid: [i32; 4],
    position: [f64; 3],
    yaw: f32,
    initialization: &ZombieNautilusInitialization,
) -> Result<Tag> {
    ensure!(
        !initialization.animal.set_baby_age,
        "structure nautilus cannot be a baby"
    );
    ensure!(
        (2400..=3600).contains(&initialization.attack_cooldown),
        "invalid attack cooldown"
    );
    let variant = format!(
        "{}:{}",
        initialization.variant.namespace, initialization.variant.path
    );
    ensure!(
        native_resources::packs::ResourceId::parse(&variant)? == initialization.variant,
        "invalid zombie nautilus variant"
    );
    let Tag::Compound(mut fields) = super::generated_sheep::draft_fresh_animal_base(
        uuid,
        position,
        yaw,
        "minecraft:zombie_nautilus",
        15.0,
        initialization.animal,
    )?
    else {
        unreachable!("animal draft is always a compound")
    };
    fields.retain(|(key, _)| {
        !["Age", "ForcedAge", "AgeLocked"]
            .iter()
            .any(|name| key.matches(name))
    });
    let persistence = fields
        .iter_mut()
        .find(|(key, _)| key.matches("PersistenceRequired"))
        .expect("fresh animal has a persistence flag");
    persistence.1 = Tag::Byte(1);
    let brain = fields
        .iter_mut()
        .find(|(key, _)| key.matches("Brain"))
        .expect("fresh animal has a brain");
    brain.1 = Tag::Compound(vec![(
        "memories".into(),
        Tag::Compound(vec![(
            "minecraft:attack_target_cooldown".into(),
            Tag::Compound(vec![(
                "value".into(),
                Tag::Int(initialization.attack_cooldown),
            )]),
        )]),
    )]);
    fields.push(("Sitting".into(), Tag::Byte(0)));
    fields.push(("variant".into(), Tag::String(variant.as_str().into())));
    Ok(Tag::Compound(fields))
}
