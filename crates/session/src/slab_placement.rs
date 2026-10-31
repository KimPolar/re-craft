//! Java 26.3 slab replacement and placement rules.

use glam::IVec3;
use native_world::loaded::BlockState;

fn valid_face(face: IVec3) -> bool {
    matches!(
        face,
        IVec3::X | IVec3::NEG_X | IVec3::Y | IVec3::NEG_Y | IVec3::Z | IVec3::NEG_Z
    )
}

/// Whether an existing slab accepts the item in this placement context.
pub fn can_replace(
    existing: &BlockState,
    item: &str,
    replacing_clicked: bool,
    face: IVec3,
    hit_y: f64,
    target_y: i32,
) -> Option<bool> {
    if !valid_face(face)
        || !hit_y.is_finite()
        || native_world::collision::slab_stair_solid(existing) != Some(true)
    {
        return None;
    }
    let kind = existing.properties.get("type")?.as_str();
    if existing.name != item || kind == "double" {
        return Some(false);
    }
    if !replacing_clicked {
        return Some(true);
    }
    let upper = hit_y - f64::from(target_y) > 0.5;
    let horizontal = face.y == 0;
    Some(if kind == "bottom" {
        face == IVec3::Y || (upper && horizontal)
    } else {
        face == IVec3::NEG_Y || (!upper && horizontal)
    })
}

/// `existing_same` is set only after the replacement test succeeds.
pub fn placement(
    name: &str,
    face: IVec3,
    hit_y: f64,
    target_y: i32,
    water_source: bool,
    existing_same: bool,
) -> Option<BlockState> {
    if !valid_face(face) || !hit_y.is_finite() {
        return None;
    }
    let kind = if existing_same {
        "double"
    } else if face == IVec3::NEG_Y || (face != IVec3::Y && hit_y - f64::from(target_y) > 0.5) {
        "top"
    } else {
        "bottom"
    };
    Some(BlockState {
        name: name.to_owned(),
        properties: [
            ("type".into(), kind.into()),
            (
                "waterlogged".into(),
                if water_source && !existing_same {
                    "true"
                } else {
                    "false"
                }
                .into(),
            ),
        ]
        .into_iter()
        .collect(),
    })
}
