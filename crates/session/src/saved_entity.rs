//! Shared, conservative preservation of base-entity fall distance.
use anyhow::{Result, bail, ensure};
use native_persistence::nbt::Tag;

pub(crate) fn fall_distance(entity: &Tag) -> Result<f64> {
    let Tag::Compound(fields) = entity else {
        bail!("entity must be a compound")
    };
    let mut values = fields
        .iter()
        .filter(|(key, _)| key.matches("fall_distance"));
    let value = values.next();
    ensure!(values.next().is_none(), "duplicate entity fall_distance");
    match value.map(|(_, value)| value) {
        None => Ok(0.0),
        Some(Tag::Double(value)) if value.is_finite() => Ok(*value),
        _ => bail!("fall distance cannot be preserved safely"),
    }
}

pub(crate) fn set_fall_distance(document: &mut Tag, original: &Tag, distance: f64) -> Result<()> {
    fall_distance(original)?;
    ensure!(distance.is_finite(), "invalid entity fall distance");
    let Tag::Compound(fields) = document else {
        bail!("entity must be a compound")
    };
    if let Some((_, value)) = fields
        .iter_mut()
        .find(|(key, _)| key.matches("fall_distance"))
    {
        *value = Tag::Double(distance);
    } else {
        fields.push(("fall_distance".into(), Tag::Double(distance)));
    }
    Ok(())
}

pub(crate) fn advance_fall_distance(
    fall: &mut f64,
    displacement: f64,
    on_ground: bool,
    in_water: bool,
) {
    if on_ground {
        *fall = 0.0;
    } else if !in_water && displacement < 0.0 {
        *fall -= f64::from(displacement as f32);
    }
}

pub(crate) fn landing_emits_event(
    fall: f64,
    displacement: f64,
    on_ground: bool,
    in_water: bool,
) -> bool {
    let displacement = if !in_water && displacement < 0.0 {
        f64::from(displacement as f32)
    } else {
        0.0
    };
    on_ground && fall - displacement > 0.0
}
