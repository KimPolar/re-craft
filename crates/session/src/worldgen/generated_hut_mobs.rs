//! Serialized drafts for structure-spawned swamp-hut mobs.
use anyhow::{Result, ensure};
use native_persistence::nbt::Tag;
use native_resources::cat_variants::CatInitialization;
use native_resources::worldgen_region_random::{AnimalBaseInitialization, MobBaseInitialization};

/// Draft a freshly constructed witch after the hut's structure spawn setup.
/// The UUID belongs to entity construction; the initialization uses the
/// WorldGenRegion RNG. This does not construct or insert a live entity.
pub fn draft_fresh_hut_witch(
    uuid: [i32; 4],
    block_pos: [i32; 3],
    initialization: MobBaseInitialization,
) -> Result<Tag> {
    ensure!(
        initialization.follow_range_bonus.is_some(),
        "fresh witch requires a follow-range modifier"
    );
    let position = [
        f64::from(block_pos[0]) + 0.5,
        f64::from(block_pos[1]),
        f64::from(block_pos[2]) + 0.5,
    ];
    let Tag::Compound(mut fields) = super::generated_sheep::draft_fresh_animal_base(
        uuid,
        position,
        0.0,
        "minecraft:witch",
        26.0,
        AnimalBaseInitialization {
            set_baby_age: false,
            follow_range_bonus: initialization.follow_range_bonus,
            left_handed: initialization.left_handed,
        },
    )?
    else {
        unreachable!("mob draft is always a compound")
    };
    fields.retain(|(name, _)| {
        !["Age", "ForcedAge", "AgeLocked", "InLove"]
            .iter()
            .any(|key| name.matches(key))
    });
    let persistence = fields
        .iter_mut()
        .find(|(name, _)| name.matches("PersistenceRequired"))
        .expect("base mob has persistence field");
    persistence.1 = Tag::Byte(1);
    fields.extend([
        ("PatrolLeader".into(), Tag::Byte(0)),
        ("Patrolling".into(), Tag::Byte(0)),
        ("CanJoinRaid".into(), Tag::Byte(1)),
        ("Wave".into(), Tag::Int(0)),
    ]);
    Ok(Tag::Compound(fields))
}

/// Draft an untamed, persistent adult cat created by a swamp hut.
pub fn draft_fresh_hut_cat(
    uuid: [i32; 4],
    block_pos: [i32; 3],
    initialization: &CatInitialization,
) -> Result<Tag> {
    ensure!(
        !initialization.animal.set_baby_age,
        "structure cat must start as an adult"
    );
    for id in [&initialization.variant, &initialization.sound_variant] {
        ensure!(
            native_resources::packs::ResourceId::parse(&format!("{}:{}", id.namespace, id.path))?
                == *id,
            "invalid cat variant ID"
        );
    }
    let position = [
        f64::from(block_pos[0]) + 0.5,
        f64::from(block_pos[1]),
        f64::from(block_pos[2]) + 0.5,
    ];
    let Tag::Compound(mut fields) = super::generated_sheep::draft_fresh_animal_base(
        uuid,
        position,
        0.0,
        "minecraft:cat",
        10.0,
        initialization.animal,
    )?
    else {
        unreachable!("cat draft is always a compound")
    };
    fields
        .iter_mut()
        .find(|(name, _)| name.matches("PersistenceRequired"))
        .expect("base mob has persistence field")
        .1 = Tag::Byte(1);
    fields.extend([
        ("Sitting".into(), Tag::Byte(0)),
        (
            "variant".into(),
            Tag::String(
                format!(
                    "{}:{}",
                    initialization.variant.namespace, initialization.variant.path
                )
                .as_str()
                .into(),
            ),
        ),
        (
            "sound_variant".into(),
            Tag::String(
                format!(
                    "{}:{}",
                    initialization.sound_variant.namespace, initialization.sound_variant.path
                )
                .as_str()
                .into(),
            ),
        ),
        ("CollarColor".into(), Tag::Byte(14)),
    ]);
    Ok(Tag::Compound(fields))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hut_witch_draft_rejects_missing_or_invalid_values() {
        let valid = MobBaseInitialization {
            follow_range_bonus: Some(0.0),
            left_handed: false,
        };
        assert!(draft_fresh_hut_witch([0; 4], [0; 3], valid).is_ok());
        assert!(draft_fresh_hut_witch([0; 4], [30_000_512, 0, 0], valid).is_err());
        assert!(
            draft_fresh_hut_witch(
                [0; 4],
                [0; 3],
                MobBaseInitialization {
                    follow_range_bonus: None,
                    ..valid
                }
            )
            .is_err()
        );
        assert!(
            draft_fresh_hut_witch(
                [0; 4],
                [0; 3],
                MobBaseInitialization {
                    follow_range_bonus: Some(f64::NAN),
                    ..valid
                }
            )
            .is_err()
        );
    }

    #[test]
    fn hut_cat_draft_rejects_baby_and_invalid_variant() {
        let animal = AnimalBaseInitialization {
            set_baby_age: false,
            follow_range_bonus: Some(0.0),
            left_handed: false,
        };
        let mut cat = CatInitialization {
            animal,
            variant: native_resources::packs::ResourceId::parse("minecraft:all_black").unwrap(),
            sound_variant: native_resources::packs::ResourceId::parse("minecraft:classic").unwrap(),
        };
        assert!(draft_fresh_hut_cat([0; 4], [0; 3], &cat).is_ok());
        cat.animal.set_baby_age = true;
        assert!(draft_fresh_hut_cat([0; 4], [0; 3], &cat).is_err());
        cat.animal = animal;
        cat.variant.path.clear();
        assert!(draft_fresh_hut_cat([0; 4], [0; 3], &cat).is_err());
    }
}
