//! Wall-side heights and center post selection for a verified neighborhood.

use crate::{collision::Aabb, loaded::BlockState};

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

impl Above {
    /// Read the lower collision face of the above block. Java tests a 2-pixel
    /// wide strip toward each side and a 2×2-pixel square at the center.
    pub fn from_collision(boxes: &[Aabb], wall_post: bool, post_override: bool) -> Self {
        let covers =
            |min_x, min_z, max_x, max_z| covers_bottom_rectangle(boxes, min_x, min_z, max_x, max_z);
        Self {
            sides_covered: [
                covers(7, 0, 9, 9),
                covers(7, 7, 16, 9),
                covers(7, 7, 9, 16),
                covers(0, 7, 9, 9),
            ],
            post_covered: covers(7, 7, 9, 9),
            wall_post,
            post_override,
        }
    }
}

fn covers_bottom_rectangle(boxes: &[Aabb], x0: u8, z0: u8, x1: u8, z1: u8) -> bool {
    let (x0, z0, x1, z1) = (
        x0 as f32 / 16.0,
        z0 as f32 / 16.0,
        x1 as f32 / 16.0,
        z1 as f32 / 16.0,
    );
    if boxes.iter().any(|b| {
        b.min.y <= 0.0
            && b.max.y > 0.0
            && b.min.x <= x0
            && b.max.x >= x1
            && b.min.z <= z0
            && b.max.z >= z1
    }) {
        return true;
    }
    if boxes.is_empty() {
        return false;
    }
    let boxes: Vec<_> = boxes
        .iter()
        .filter(|b| b.min.y <= 0.0 && b.max.y > 0.0)
        .collect();
    let mut xs = vec![x0, x1];
    let mut zs = vec![z0, z1];
    for b in &boxes {
        xs.extend([b.min.x.clamp(x0, x1), b.max.x.clamp(x0, x1)]);
        zs.extend([b.min.z.clamp(z0, z1), b.max.z.clamp(z0, z1)]);
    }
    xs.sort_by(f32::total_cmp);
    xs.dedup();
    zs.sort_by(f32::total_cmp);
    zs.dedup();
    xs.windows(2).all(|x| {
        zs.windows(2).all(|z| {
            let (x, z) = ((x[0] + x[1]) * 0.5, (z[0] + z[1]) * 0.5);
            boxes
                .iter()
                .any(|b| b.min.x <= x && x < b.max.x && b.min.z <= z && z < b.max.z)
        })
    })
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

    #[test]
    fn bottom_coverage_uses_union_and_ignores_floating_boxes() {
        use glam::Vec3;
        let left = Aabb {
            min: Vec3::ZERO,
            max: Vec3::new(0.5, 1.0, 1.0),
        };
        let right = Aabb {
            min: Vec3::new(0.5, 0.0, 0.0),
            max: Vec3::ONE,
        };
        let floating = Aabb {
            min: Vec3::new(0.0, 0.5, 0.0),
            max: Vec3::ONE,
        };
        let full = Above::from_collision(&[left, right], false, false);
        assert_eq!(full.sides_covered, [true; 4]);
        assert!(full.post_covered);
        let partial = Above::from_collision(&[left, floating], false, false);
        assert!(!partial.post_covered);
        assert_ne!(partial.sides_covered, [true; 4]);
    }
}
