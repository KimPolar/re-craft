//! Shape selection for a stair when its horizontal neighbors change.

use glam::IVec3;

use crate::loaded::BlockState;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Stair {
    pub facing: IVec3,
    pub top: bool,
}

impl Stair {
    /// Parse an already verified vanilla stair state.
    pub fn from_state(state: &BlockState) -> Option<Self> {
        let facing = match state.properties.get("facing")?.as_str() {
            "north" => IVec3::NEG_Z,
            "south" => IVec3::Z,
            "west" => IVec3::NEG_X,
            "east" => IVec3::X,
            _ => return None,
        };
        let top = match state.properties.get("half")?.as_str() {
            "top" => true,
            "bottom" => false,
            _ => return None,
        };
        Some(Self { facing, top })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Neighbor {
    Stair(Stair),
    Other,
    Unknown,
}

/// Return `None` if a neighbor needed to decide the shape is unavailable.
/// The lookup takes a relative horizontal direction from the stair.
pub fn shape(current: Stair, lookup: impl Fn(IVec3) -> Neighbor) -> Option<&'static str> {
    let can_take = |direction| match lookup(direction) {
        Neighbor::Stair(stair) => Some(stair != current),
        Neighbor::Other => Some(true),
        Neighbor::Unknown => None,
    };
    let left = IVec3::new(current.facing.z, 0, -current.facing.x);
    match lookup(current.facing) {
        Neighbor::Stair(front)
            if front.top == current.top
                && front.facing.dot(current.facing) == 0
                && can_take(-front.facing)? =>
        {
            return Some(if front.facing == left {
                "outer_left"
            } else {
                "outer_right"
            });
        }
        Neighbor::Unknown => return None,
        _ => {}
    }
    match lookup(-current.facing) {
        Neighbor::Stair(back)
            if back.top == current.top
                && back.facing.dot(current.facing) == 0
                && can_take(back.facing)? =>
        {
            Some(if back.facing == left {
                "inner_left"
            } else {
                "inner_right"
            })
        }
        Neighbor::Unknown => None,
        _ => Some("straight"),
    }
}
