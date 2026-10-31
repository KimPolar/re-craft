//! Placement state for verified vanilla stair blocks.

use glam::IVec3;
use native_world::{
    loaded::BlockState,
    stairs::{self, Neighbor, Stair},
};

pub fn placement(
    name: &str,
    yaw: f32,
    face: IVec3,
    hit_y: f64,
    target_y: i32,
    water_source: bool,
    neighbors: impl Fn(IVec3) -> Neighbor,
) -> Option<BlockState> {
    if !yaw.is_finite()
        || !hit_y.is_finite()
        || !matches!(
            face,
            IVec3::X | IVec3::NEG_X | IVec3::Y | IVec3::NEG_Y | IVec3::Z | IVec3::NEG_Z
        )
    {
        return None;
    }
    let java_yaw = f64::from(yaw.to_degrees() - 180.0);
    let facing =
        ["south", "west", "north", "east"][(((java_yaw / 90.0 + 0.5).floor() as i32) & 3) as usize];
    let top = face == IVec3::NEG_Y || (face != IVec3::Y && hit_y - f64::from(target_y) > 0.5);
    let mut block = BlockState {
        name: name.to_owned(),
        properties: [
            ("facing", facing),
            ("half", if top { "top" } else { "bottom" }),
            ("shape", "straight"),
            ("waterlogged", if water_source { "true" } else { "false" }),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect(),
    };
    let shape = stairs::shape(Stair::from_state(&block)?, neighbors)?;
    block.properties.insert("shape".into(), shape.into());
    Some(block)
}
