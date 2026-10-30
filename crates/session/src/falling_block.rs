//! Airborne movement for a falling block. World collision, landing and block
//! effects must be handled before this can drive a live entity.

use glam::{DVec3, IVec3};
use native_world::movement::BoundingBox;

/// Only use this fallback for collision shapes supplied by the native block
/// registry. The supported 26.3 states with nonempty shapes are checked against
/// FallingBlock.isFree; active fire tags must be checked first.
pub fn known_collision_support(shape: &[native_world::collision::Aabb]) -> bool {
    !shape.is_empty()
}

/// Server-side expiry after an airborne tick. `max_y` is exclusive.
pub fn expires_in_air(age: i32, block_y: i32, min_y: i32, max_y: i32) -> bool {
    age > 600 || (age > 100 && (block_y <= min_y || block_y >= max_y))
}

/// The known subset of FallingBlock.isFree. Unknown states require the active
/// block registry's replaceability and fire tags before they can be decided.
pub fn known_free_below(block: &str, is_fire: bool) -> Option<bool> {
    if is_fire {
        return Some(true);
    }
    if let Some(id) = block.strip_prefix("minecraft:")
        && (id.ends_with("_sapling") || id.ends_with("_tulip"))
    {
        return Some(false);
    }
    match block {
        "minecraft:air"
        | "minecraft:cave_air"
        | "minecraft:void_air"
        | "minecraft:water"
        | "minecraft:lava"
        | "minecraft:short_grass"
        | "minecraft:fern"
        | "minecraft:dead_bush" => Some(true),
        "minecraft:stone"
        | "minecraft:dirt"
        | "minecraft:grass_block"
        | "minecraft:sand"
        | "minecraft:red_sand"
        | "minecraft:gravel"
        | "minecraft:bedrock"
        | "minecraft:cobblestone"
        | "minecraft:kelp"
        | "minecraft:kelp_plant"
        | "minecraft:dandelion"
        | "minecraft:poppy"
        | "minecraft:blue_orchid"
        | "minecraft:allium"
        | "minecraft:azure_bluet"
        | "minecraft:oxeye_daisy"
        | "minecraft:cornflower"
        | "minecraft:lily_of_the_valley"
        | "minecraft:torch"
        | "minecraft:wall_torch"
        | "minecraft:soul_torch"
        | "minecraft:soul_wall_torch" => Some(false),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Collision {
    pub actual: DVec3,
    pub on_ground: bool,
    pub horizontal: bool,
    pub vertical: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum FallingKind {
    Sand,
    RedSand,
    Gravel,
}

impl FallingKind {
    pub fn id(self) -> &'static str {
        match self {
            Self::Sand => "minecraft:sand",
            Self::RedSand => "minecraft:red_sand",
            Self::Gravel => "minecraft:gravel",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id.strip_prefix("minecraft:").unwrap_or(id) {
            "sand" => Some(Self::Sand),
            "red_sand" => Some(Self::RedSand),
            "gravel" => Some(Self::Gravel),
            _ => None,
        }
    }
}

/// Immutable visual state for the renderer; positions are absolute world coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FallingBlockVisual {
    pub previous_position: DVec3,
    pub position: DVec3,
    pub kind: FallingKind,
}

impl FallingBlockVisual {
    /// Model origin relative to the render origin, preserving precision for far worlds.
    pub fn model_origin(&self, origin: IVec3, partial_tick: f32) -> DVec3 {
        let partial = f64::from(partial_tick);
        self.previous_position + (self.position - self.previous_position) * partial
            - origin.as_dvec3()
            + DVec3::new(-0.5, 0.0, -0.5)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AirborneMotion {
    pub position: DVec3,
    pub velocity: DVec3,
    pub age: i32,
    pub no_gravity: bool,
}

/// Entity.checkFallDamage on an ordinary, dry movement path. Water and blocks
/// with special fall hooks must be handled separately by the caller.
pub fn fall_distance_after_move(distance: f64, actual_y: f64, on_ground: bool) -> f64 {
    if on_ground {
        0.0
    } else if actual_y < 0.0 {
        distance - f64::from(actual_y as f32)
    } else {
        distance
    }
}

impl AirborneMotion {
    /// Java creates the falling entity at the horizontal center of the block.
    pub fn from_block(position: IVec3) -> Self {
        Self {
            position: position.as_dvec3() + DVec3::new(0.5, 0.0, 0.5),
            velocity: DVec3::ZERO,
            age: 0,
            no_gravity: false,
        }
    }

    pub fn bounds(&self) -> BoundingBox {
        let half = f64::from(0.98_f32) / 2.0;
        BoundingBox {
            min: self.position + DVec3::new(-half, 0.0, -half),
            max: self.position + DVec3::new(half, f64::from(0.98_f32), half),
        }
    }
    /// Advance an unobstructed tick, excluding world and portal effects.
    pub fn tick_in_air(&mut self) {
        self.tick_with_collision(|_, requested| requested);
    }

    /// Move against a caller-supplied collision resolver for ordinary,
    /// non-bouncing surfaces. Landing and block effects are handled separately.
    pub fn tick_with_collision(
        &mut self,
        mut resolve: impl FnMut(DVec3, DVec3) -> DVec3,
    ) -> Collision {
        self.age = self.age.wrapping_add(1);
        if !self.no_gravity {
            self.velocity.y -= 0.04;
        }
        let requested = self.velocity;
        let actual = resolve(self.position, requested);
        self.position += actual;
        let hit_x = (requested.x - actual.x).abs() >= f64::from(1.0e-5_f32);
        let hit_z = (requested.z - actual.z).abs() >= f64::from(1.0e-5_f32);
        let hit_y = requested.y != actual.y;
        if hit_x {
            self.velocity.x = 0.0;
        }
        if hit_y {
            self.velocity.y = 0.0;
        }
        if hit_z {
            self.velocity.z = 0.0;
        }
        self.velocity *= f64::from(0.98_f32);
        Collision {
            actual,
            on_ground: hit_y && requested.y < 0.0,
            horizontal: hit_x || hit_z,
            vertical: hit_y,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{IVec3, Vec3};
    use native_world::collision::Aabb;

    #[test]
    fn fire_tag_can_make_a_solid_or_unknown_block_free() {
        assert_eq!(known_free_below("minecraft:stone", false), Some(false));
        assert_eq!(known_free_below("minecraft:stone", true), Some(true));
        assert_eq!(known_free_below("example:unknown", false), None);
        assert_eq!(known_free_below("example:unknown", true), Some(true));
    }

    #[test]
    fn spawn_uses_absolute_block_center_without_losing_large_coordinates() {
        for pos in [IVec3::new(i32::MIN, -64, i32::MAX), IVec3::new(-1, 64, -1)] {
            let motion = AirborneMotion::from_block(pos);
            assert_eq!(motion.position, pos.as_dvec3() + DVec3::new(0.5, 0.0, 0.5));
            assert_eq!(motion.velocity, DVec3::ZERO);
            assert_eq!(motion.age, 0);
        }
    }

    #[test]
    fn clipped_motion_stops_at_a_floor_and_wall() {
        const FULL: &[Aabb] = &[Aabb {
            min: Vec3::ZERO,
            max: Vec3::ONE,
        }];
        let mut motion = AirborneMotion {
            position: DVec3::new(0.5, 3.0, 0.5),
            velocity: DVec3::new(3.0, -4.0, 0.0),
            age: 0,
            no_gravity: false,
        };
        let half = f64::from(0.98_f32) / 2.0;
        let collision = motion.tick_with_collision(|position, requested| {
            let bounds = AirborneMotion {
                position,
                velocity: DVec3::ZERO,
                age: 0,
                no_gravity: false,
            }
            .bounds();
            crate::movement::resolve_box(bounds, requested, false, 0.0, |p: IVec3| {
                if p.y == 0 || (p.x == 2 && p.y >= 1 && p.y <= 4) {
                    FULL
                } else {
                    &[]
                }
            })
        });
        assert!(collision.on_ground && collision.horizontal && collision.vertical);
        assert!((motion.position.y - 1.0).abs() < 1.0e-9);
        assert!((motion.position.x - (2.0 - half)).abs() < 1.0e-9);
        assert_eq!(motion.velocity, DVec3::ZERO);
    }
}
