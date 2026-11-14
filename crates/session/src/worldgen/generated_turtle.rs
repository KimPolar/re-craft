//! NBT for a newly finalized turtle in a generated protochunk.
use anyhow::Result;
use native_persistence::nbt::Tag;
use native_resources::worldgen_region_random::AnimalBaseInitialization;

/// The home position is assigned from the turtle's actual block position by
/// Java's finalizeSpawn, independently of its later movement or breeding.
pub fn draft_fresh_turtle(
    uuid: [i32; 4],
    position: [f64; 3],
    yaw: f32,
    animal: AnimalBaseInitialization,
) -> Result<Tag> {
    let Tag::Compound(mut fields) = super::generated_sheep::draft_fresh_animal_base(
        uuid,
        position,
        yaw,
        "minecraft:turtle",
        30.0,
        animal,
    )?
    else {
        unreachable!("animal draft is always a compound")
    };
    fields.push((
        "home_pos".into(),
        Tag::IntArray(
            position
                .map(|coordinate| coordinate.floor() as i32)
                .to_vec(),
        ),
    ));
    fields.push(("has_egg".into(), Tag::Byte(0)));
    Ok(Tag::Compound(fields))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_uses_floored_position_and_invalid_state_is_rejected() {
        let animal = AnimalBaseInitialization {
            set_baby_age: false,
            follow_range_bonus: Some(0.0),
            left_handed: false,
        };
        let tag = draft_fresh_turtle([0; 4], [-0.1, 64.0, 3.9], 0.0, animal).unwrap();
        assert_eq!(tag.get("home_pos"), Some(&Tag::IntArray(vec![-1, 64, 3])));
        assert_eq!(tag.get("has_egg"), Some(&Tag::Byte(0)));
        assert!(draft_fresh_turtle([0; 4], [f64::NAN, 0.0, 0.0], 0.0, animal).is_err());
        assert!(draft_fresh_turtle([0; 4], [0.0; 3], f32::NAN, animal).is_err());
    }
}
