//! Wall-side heights and center post selection for a verified neighborhood.

use crate::loaded::BlockState;

/// Order is north, east, south, west.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Side {
    None,
    Low,
    Tall,
}

/// The collision face and block immediately above a wall.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Above {
    /// Whether the lower collision face fully covers each directional test area.
    pub sides_covered: [bool; 4],
    /// Whether the lower collision face fully covers the central test area.
    pub post_covered: bool,
    /// The above block is a wall with its center post raised.
    pub wall_post: bool,
    /// The above block belongs to the active wall-post-override tag.
    pub post_override: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Shape {
    pub sides: [Side; 4],
    pub up: bool,
}

/// A complete state for a built-in wall, including its contained fluid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Wall {
    pub shape: Shape,
    pub waterlogged: bool,
}

impl Wall {
    pub fn from_state(state: &BlockState) -> Option<Self> {
        let id = state.name.strip_prefix("minecraft:")?;
        if !matches!(
            id,
            "andesite_wall"
                | "blackstone_wall"
                | "brick_wall"
                | "cinnabar_brick_wall"
                | "cinnabar_wall"
                | "cobbled_deepslate_wall"
                | "cobblestone_wall"
                | "deepslate_brick_wall"
                | "deepslate_tile_wall"
                | "diorite_wall"
                | "end_stone_brick_wall"
                | "granite_wall"
                | "mossy_cobblestone_wall"
                | "mossy_stone_brick_wall"
                | "mud_brick_wall"
                | "nether_brick_wall"
                | "polished_blackstone_brick_wall"
                | "polished_blackstone_wall"
                | "polished_cinnabar_wall"
                | "polished_deepslate_wall"
                | "polished_sulfur_wall"
                | "polished_tuff_wall"
                | "prismarine_wall"
                | "red_nether_brick_wall"
                | "red_sandstone_wall"
                | "resin_brick_wall"
                | "sandstone_wall"
                | "stone_brick_wall"
                | "sulfur_brick_wall"
                | "sulfur_wall"
                | "tuff_brick_wall"
                | "tuff_wall"
        ) || state.properties.len() != 6
        {
            return None;
        }
        let side = |name: &str| match state.properties.get(name)?.as_str() {
            "none" => Some(Side::None),
            "low" => Some(Side::Low),
            "tall" => Some(Side::Tall),
            _ => None,
        };
        let boolean = |name: &str| match state.properties.get(name)?.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        };
        Some(Self {
            shape: Shape {
                sides: [side("north")?, side("east")?, side("south")?, side("west")?],
                up: boolean("up")?,
            },
            waterlogged: boolean("waterlogged")?,
        })
    }
}

/// Recompute all wall arms from connection decisions and the block above.
/// Callers must resolve tags, neighbor sturdiness, and face coverage first.
pub fn shape(connected: [bool; 4], above: Above) -> Shape {
    let sides = std::array::from_fn(|i| {
        if !connected[i] {
            Side::None
        } else if above.sides_covered[i] {
            Side::Tall
        } else {
            Side::Low
        }
    });
    let [north, east, south, west] = connected;
    let asymmetric = !north && !east && !south && !west || north != south || east != west;
    let opposed_tall = (sides[0] == Side::Tall && sides[2] == Side::Tall)
        || (sides[1] == Side::Tall && sides[3] == Side::Tall);
    let up = above.wall_post
        || asymmetric
        || (!opposed_tall && (above.post_override || above.post_covered));
    Shape { sides, up }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wall_post_override_preserves_java_precedence() {
        let mut above = Above {
            sides_covered: [false; 4],
            post_covered: false,
            wall_post: false,
            post_override: true,
        };
        assert!(shape([true, false, true, false], above).up);
        above.sides_covered = [true, false, true, false];
        assert!(!shape([true, false, true, false], above).up);
        above.wall_post = true;
        assert!(shape([true, false, true, false], above).up);
    }

    #[test]
    fn wall_state_requires_known_block_and_complete_properties() {
        let mut state = BlockState {
            name: "minecraft:stone_brick_wall".into(),
            properties: [
                ("north", "none"),
                ("east", "low"),
                ("south", "tall"),
                ("west", "none"),
                ("up", "true"),
                ("waterlogged", "false"),
            ]
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect(),
        };
        assert_eq!(
            Wall::from_state(&state).unwrap().shape.sides,
            [Side::None, Side::Low, Side::Tall, Side::None]
        );
        state.properties.insert("east".into(), "invalid".into());
        assert!(Wall::from_state(&state).is_none());
        state.properties.insert("east".into(), "low".into());
        state.name = "minecraft:not_a_real_wall".into();
        assert!(Wall::from_state(&state).is_none());
    }
}
