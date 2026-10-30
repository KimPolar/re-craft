//! Placement state selection for ordinary and trapped chests.

use glam::IVec3;
use native_world::loaded::BlockState;

fn horizontal(dir: IVec3) -> bool {
    matches!(dir, IVec3::X | IVec3::NEG_X | IVec3::Z | IVec3::NEG_Z)
}

fn clockwise(dir: IVec3) -> IVec3 {
    IVec3::new(-dir.z, 0, dir.x)
}

fn name(dir: IVec3) -> Option<&'static str> {
    match dir {
        IVec3::NEG_Z => Some("north"),
        IVec3::Z => Some("south"),
        IVec3::NEG_X => Some("west"),
        IVec3::X => Some("east"),
        _ => None,
    }
}

fn facing(state: &BlockState) -> Option<IVec3> {
    match state.properties.get("facing")?.as_str() {
        "north" => Some(IVec3::NEG_Z),
        "south" => Some(IVec3::Z),
        "west" => Some(IVec3::NEG_X),
        "east" => Some(IVec3::X),
        _ => None,
    }
}

/// `horizontal` is the player's horizontal viewing direction, while `face`
/// is the clicked face. The callback returns None for unavailable neighbors.
pub fn placement<'a>(
    id: &str,
    horizontal_direction: IVec3,
    face: IVec3,
    secondary: bool,
    water: bool,
    mut neighbor: impl FnMut(IVec3) -> Option<&'a BlockState>,
) -> Option<BlockState> {
    if !matches!(id, "minecraft:chest" | "minecraft:trapped_chest")
        || !horizontal(horizontal_direction)
        || !matches!(
            face,
            IVec3::X | IVec3::NEG_X | IVec3::Y | IVec3::NEG_Y | IVec3::Z | IVec3::NEG_Z
        )
    {
        return None;
    }
    let mut candidate = |dir: IVec3| -> Option<Option<IVec3>> {
        let state = neighbor(dir)?;
        if state.name != id {
            return Some(None);
        }
        native_world::collision::block_shape(state)?;
        if state.properties.get("type")?.as_str() != "single" {
            return Some(None);
        }
        Some(Some(facing(state)?))
    };
    let mut facing = -horizontal_direction;
    let mut kind = "single";
    if secondary && horizontal(face) {
        if let Some(partner) = candidate(-face)?
            && partner.dot(face) == 0
        {
            facing = partner;
            kind = if -clockwise(facing) == -face {
                "right"
            } else {
                "left"
            };
        }
    } else if !secondary {
        if candidate(clockwise(facing))? == Some(facing) {
            kind = "left";
        } else if candidate(-clockwise(facing))? == Some(facing) {
            kind = "right";
        }
    }
    Some(BlockState {
        name: id.into(),
        properties: [
            ("facing".into(), name(facing)?.into()),
            ("type".into(), kind.into()),
            (
                "waterlogged".into(),
                if water { "true" } else { "false" }.into(),
            ),
        ]
        .into_iter()
        .collect(),
    })
}
