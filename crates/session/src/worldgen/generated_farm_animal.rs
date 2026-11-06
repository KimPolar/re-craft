//! NBT drafts for newly constructed pigs, cows, and chickens.
use anyhow::{Result, ensure};
use native_persistence::nbt::Tag;
use native_resources::{
    animal_variants::{FarmAnimal, FarmAnimalInitialization, FarmAnimalSpawnVariants},
    packs::ResourceId,
    worldgen_region_random::{AnimalGroupData, RegionRandom},
};

/// Finalize a newly constructed chicken for a zombie jockey. The region RNG
/// is advanced only if the complete chicken draft can be represented.
/// The rider must be serialized separately at the time Java adds the chicken.
pub fn draft_fresh_jockey_chicken(
    creature: &super::FreshCreatureRandom,
    position: [f64; 3],
    yaw: f32,
    biome: &ResourceId,
    variants: &FarmAnimalSpawnVariants,
    random: &mut RegionRandom,
) -> Result<Tag> {
    ensure!(
        variants.species() == FarmAnimal::Chicken,
        "expected chicken variants"
    );
    let mut next_random = random.clone();
    let mut group = AnimalGroupData::default();
    let initialization = variants.finalize(biome, &mut next_random, &mut group, false)?;
    let Tag::Compound(mut fields) = draft_fresh_farm_animal(
        FarmAnimal::Chicken,
        creature.construction.uuid,
        position,
        yaw,
        variants.default_variant(),
        &initialization,
        creature.construction.chicken_egg_time,
    )?
    else {
        unreachable!("fresh chicken draft is a compound")
    };
    let (_, jockey) = fields
        .iter_mut()
        .find(|(key, _)| key.matches("IsChickenJockey"))
        .expect("fresh chicken has a jockey flag");
    *jockey = Tag::Byte(1);
    *random = next_random;
    Ok(Tag::Compound(fields))
}

/// Serialize an unmodified, freshly finalized animal. `default_variant` is
/// the construction default from the same active registry. A fresh chicken's
/// egg timer is drawn from its own RNG, not the world-generation region RNG;
/// provide the value obtained when that particular entity was constructed.
pub fn draft_fresh_farm_animal(
    species: FarmAnimal,
    uuid: [i32; 4],
    position: [f64; 3],
    yaw: f32,
    default_variant: &ResourceId,
    initialization: &FarmAnimalInitialization,
    chicken_egg_time: Option<i32>,
) -> Result<Tag> {
    let (id, health) = match species {
        FarmAnimal::Pig => ("minecraft:pig", 10.0),
        FarmAnimal::Cow => ("minecraft:cow", 10.0),
        FarmAnimal::Chicken => ("minecraft:chicken", 4.0),
    };
    match (species, chicken_egg_time) {
        (FarmAnimal::Chicken, Some(time)) => ensure!(
            (6000..12000).contains(&time),
            "invalid fresh chicken egg timer"
        ),
        (FarmAnimal::Chicken, None) => anyhow::bail!("fresh chicken egg timer is missing"),
        (_, None) => {}
        (_, Some(_)) => anyhow::bail!("egg timer belongs only to a chicken"),
    }
    let variant = initialization.variant.as_ref().unwrap_or(default_variant);
    let resource_string = |value: &ResourceId| -> Result<String> {
        let name = format!("{}:{}", value.namespace, value.path);
        ensure!(ResourceId::parse(&name)? == *value, "invalid variant ID");
        Ok(name)
    };
    let variant = resource_string(variant)?;
    let sound = resource_string(&initialization.sound_variant)?;
    let Tag::Compound(mut fields) = super::generated_sheep::draft_fresh_animal_base(
        uuid,
        position,
        yaw,
        id,
        health,
        initialization.animal,
    )?
    else {
        unreachable!("animal draft is always a compound")
    };
    if let Some(egg_time) = chicken_egg_time {
        fields.push(("IsChickenJockey".into(), Tag::Byte(0)));
        fields.push(("EggLayTime".into(), Tag::Int(egg_time)));
    }
    fields.push(("variant".into(), Tag::String(variant.as_str().into())));
    fields.push(("sound_variant".into(), Tag::String(sound.as_str().into())));
    Ok(Tag::Compound(fields))
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_resources::worldgen_region_random::AnimalBaseInitialization;

    #[test]
    fn validates_independent_chicken_timer_and_variant_identifiers() {
        let initialization = FarmAnimalInitialization {
            variant: None,
            sound_variant: ResourceId::parse("minecraft:classic").unwrap(),
            animal: AnimalBaseInitialization {
                set_baby_age: false,
                follow_range_bonus: Some(0.0),
                left_handed: false,
            },
        };
        let default = ResourceId::parse("minecraft:temperate").unwrap();
        let draft = |species, timer| {
            draft_fresh_farm_animal(
                species,
                [0; 4],
                [0.0; 3],
                0.0,
                &default,
                &initialization,
                timer,
            )
        };
        assert!(draft(FarmAnimal::Chicken, None).is_err());
        assert!(draft(FarmAnimal::Chicken, Some(5999)).is_err());
        assert!(draft(FarmAnimal::Chicken, Some(12000)).is_err());
        assert!(draft(FarmAnimal::Cow, Some(6000)).is_err());
        assert!(draft(FarmAnimal::Chicken, Some(6000)).is_ok());
        assert!(draft(FarmAnimal::Chicken, Some(11999)).is_ok());
        let bad = ResourceId {
            namespace: "".into(),
            path: "bad".into(),
        };
        assert!(
            draft_fresh_farm_animal(
                FarmAnimal::Pig,
                [0; 4],
                [0.0; 3],
                0.0,
                &bad,
                &initialization,
                None
            )
            .is_err()
        );
    }
}
