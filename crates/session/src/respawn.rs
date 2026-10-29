//! Candidate ordering for Java 26.3's bed and respawn-anchor stand-up search.
//!
//! The caller resolves collision, block tags, the world border and chunk
//! availability. An error stops the search: skipping an unknown earlier
//! candidate could choose a different location from the server.

use glam::{DVec3, IVec3};
use native_world::movement::BoundingBox;

/// The block at the player's saved respawn position. Other states are checked
/// separately through `possible_to_respawn_in`, as Java does for forced spawn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SavedSpawnBlock {
    Bed { straw: bool },
    Anchor { charges: u8 },
    Other,
}

/// World-dependent checks used by ServerPlayer's saved-spawn selection.
/// Unknown data must be reported as an error rather than treated as air.
pub trait SavedSpawnWorld {
    type Error;

    fn spawn_block(&mut self, pos: IVec3) -> Result<SavedSpawnBlock, Self::Error>;
    fn bed_can_set_spawn(&mut self, pos: IVec3, straw: bool) -> Result<bool, Self::Error>;
    fn anchor_works(&mut self, pos: IVec3) -> Result<bool, Self::Error>;
    fn bed_stand_up(&mut self, pos: IVec3, yaw: f32) -> Result<Option<DVec3>, Self::Error>;
    fn anchor_stand_up(&mut self, pos: IVec3) -> Result<Option<DVec3>, Self::Error>;
    fn possible_to_respawn_in(&mut self, pos: IVec3) -> Result<bool, Self::Error>;
}

/// A result prepared without mutating the world. If `anchor_charge` is set,
/// the caller must apply that update at the respawn transition boundary.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SavedSpawnResult {
    pub position: DVec3,
    /// Java's yaw and pitch in degrees.
    pub yaw: f32,
    pub pitch: f32,
    pub anchor_charge: Option<u8>,
}

/// Resolve an already-loaded saved spawn. `None` means Java falls back to the
/// world spawn. A missing chunk, rule or unsupported block must be an error.
pub fn resolve_saved_spawn<W: SavedSpawnWorld>(
    world: &mut W,
    pos: IVec3,
    yaw: f32,
    pitch: f32,
    forced: bool,
    consume_anchor: bool,
) -> Result<Option<SavedSpawnResult>, W::Error> {
    match world.spawn_block(pos)? {
        SavedSpawnBlock::Anchor { charges }
            if (forced || charges > 0) && world.anchor_works(pos)? =>
        {
            return Ok(world
                .anchor_stand_up(pos)?
                .map(|position| SavedSpawnResult {
                    position,
                    yaw: look_at_spawn_yaw(position, pos),
                    pitch: 0.0,
                    anchor_charge: (!forced && consume_anchor).then(|| charges - 1),
                }));
        }
        SavedSpawnBlock::Bed { straw } if world.bed_can_set_spawn(pos, straw)? => {
            return Ok(world
                .bed_stand_up(pos, yaw)?
                .map(|position| SavedSpawnResult {
                    position,
                    yaw: look_at_spawn_yaw(position, pos),
                    pitch: 0.0,
                    anchor_charge: None,
                }));
        }
        _ => {}
    }
    if forced
        && world.possible_to_respawn_in(pos)?
        && world.possible_to_respawn_in(offset(pos, IVec3::Y))?
    {
        Ok(Some(SavedSpawnResult {
            position: pos.as_dvec3() + DVec3::new(0.5, 0.1, 0.5),
            yaw,
            pitch,
            anchor_charge: None,
        }))
    } else {
        Ok(None)
    }
}

fn look_at_spawn_yaw(position: DVec3, pos: IVec3) -> f32 {
    let delta = pos.as_dvec3() + DVec3::new(0.5, 0.0, 0.5) - position;
    let length = delta.length();
    let direction = if length < 1.0e-4 {
        DVec3::ZERO
    } else {
        delta / length
    };
    let angle = java_atan2(direction.z, direction.x) * 57.295_776_367_187_5 - 90.0;
    let mut wrapped = angle % 360.0;
    if wrapped >= 180.0 {
        wrapped -= 360.0;
    }
    if wrapped < -180.0 {
        wrapped += 360.0;
    }
    wrapped as f32
}

/// Mth.atan2's lookup and correction, including Java's inverse-square-root
/// approximation. Called only for finite stand-up positions.
fn java_atan2(mut y: f64, mut x: f64) -> f64 {
    let length_squared = x * x + y * y;
    if length_squared.is_nan() {
        return f64::NAN;
    }
    let negative_y = y < 0.0;
    if negative_y {
        y = -y;
    }
    let negative_x = x < 0.0;
    if negative_x {
        x = -x;
    }
    let swap = y > x;
    if swap {
        std::mem::swap(&mut x, &mut y);
    }
    let bits = 6_910_469_410_427_058_090_u64.wrapping_sub(length_squared.to_bits() >> 1);
    let inverse = f64::from_bits(bits);
    let inverse = inverse * (1.5 - 0.5 * length_squared * inverse * inverse);
    x *= inverse;
    y *= inverse;
    let bias = f64::from_bits(4_805_340_802_404_319_232);
    let biased = bias + y;
    let index = biased.to_bits() as u32 as usize;
    let (asin, cosine) = {
        use std::sync::OnceLock;
        static TABLE: OnceLock<([f64; 257], [f64; 257])> = OnceLock::new();
        let tables = TABLE.get_or_init(|| {
            let angles = std::array::from_fn(|index| (index as f64 / 256.0).asin());
            let cosines = std::array::from_fn(|index| angles[index].cos());
            (angles, cosines)
        });
        (tables.0[index], tables.1[index])
    };
    let fraction = biased - bias;
    let error = y * cosine - x * fraction;
    let correction = (6.0 + error * error) * error * (1.0 / 6.0);
    let mut angle = asin + correction;
    if swap {
        angle = std::f64::consts::FRAC_PI_2 - angle;
    }
    if negative_x {
        angle = std::f64::consts::PI - angle;
    }
    if negative_y {
        angle = -angle;
    }
    angle
}

/// Current, already evaluated world-border edges in absolute block coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RespawnBorder {
    pub min_x: f64,
    pub max_x: f64,
    pub min_z: f64,
    pub max_z: f64,
}

impl RespawnBorder {
    /// Evaluate the border used for a server-side check after the given number
    /// of world ticks since loading its saved settings.
    pub fn from_saved(
        saved: native_persistence::world::SavedWorldBorder,
        ticks: u64,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            [
                saved.center_x,
                saved.center_z,
                saved.size,
                saved.lerp_target
            ]
            .iter()
            .all(|value| value.is_finite()),
            "world border contains a non-finite value"
        );
        let size = if saved.lerp_time > 0 {
            let duration = saved.lerp_time as u64;
            if ticks >= duration {
                saved.lerp_target
            } else {
                // The no-argument world-border bounds use the previous tick's
                // size until the moving extent becomes static on its last tick.
                let progress = ticks.saturating_sub(1) as f64 / duration as f64;
                saved.size + progress * (saved.lerp_target - saved.size)
            }
        } else {
            saved.size
        };
        anyhow::ensure!(size.is_finite(), "world border size is not finite");
        let edge = |center: f64, sign: f64| {
            (center + sign * size / 2.0).clamp(-29_999_984.0, 29_999_984.0)
        };
        let border = Self {
            min_x: edge(saved.center_x, -1.0),
            max_x: edge(saved.center_x, 1.0),
            min_z: edge(saved.center_z, -1.0),
            max_z: edge(saved.center_z, 1.0),
        };
        anyhow::ensure!(border.is_valid(), "world border has invalid bounds");
        Ok(border)
    }

    pub fn contains(&self, bounds: BoundingBox) -> bool {
        // WorldBorder.isWithinBounds(AABB) subtracts this float-derived epsilon
        // from the maximum corner before checking its exclusive upper edge.
        let epsilon = f64::from(1.0e-5_f32);
        let inside =
            |x: f64, z: f64| x >= self.min_x && x < self.max_x && z >= self.min_z && z < self.max_z;
        inside(bounds.min.x, bounds.min.z) && inside(bounds.max.x - epsilon, bounds.max.z - epsilon)
    }

    pub fn is_valid(&self) -> bool {
        [self.min_x, self.max_x, self.min_z, self.max_z]
            .iter()
            .all(|value| value.is_finite())
            && self.min_x <= self.max_x
            && self.min_z <= self.max_z
    }
}

/// World queries required by `DismountHelper.findSafeDismountLocation`.
/// Implementations must report unknown chunks, tags or collision shapes as an
/// error rather than substituting air or an arbitrary border.
pub trait DismountWorld {
    type Error;

    fn is_dangerous(&mut self, pos: IVec3) -> Result<bool, Self::Error>;
    /// Maximum Y of the non-climbable collision shape, or None when empty.
    fn non_climbable_top(&mut self, pos: IVec3) -> Result<Option<f64>, Self::Error>;
    fn has_block_collision(&mut self, bounds: BoundingBox) -> Result<bool, Self::Error>;
    fn invalid_spawn_inside(&mut self, pos: IVec3) -> Result<bool, Self::Error>;
    fn within_world_border(&mut self, bounds: BoundingBox) -> Result<bool, Self::Error>;
}

/// Apply the 26.3 player dismount checks to one bed or anchor candidate.
pub fn find_safe_dismount<W: DismountWorld>(
    world: &mut W,
    pos: IVec3,
    avoid_danger: bool,
) -> Result<Option<DVec3>, W::Error> {
    if avoid_danger && world.is_dangerous(pos)? {
        return Ok(None);
    }
    let below = offset(pos, IVec3::NEG_Y);
    let floor = match world.non_climbable_top(pos)? {
        Some(top) => top,
        None => match world.non_climbable_top(below)? {
            Some(top) if top >= 1.0 => top - 1.0,
            _ => return Ok(None),
        },
    };
    if !floor.is_finite() || floor >= 1.0 {
        return Ok(None);
    }
    if avoid_danger && floor <= 0.0 && world.is_dangerous(below)? {
        return Ok(None);
    }
    let position = pos.as_dvec3() + DVec3::new(0.5, floor, 0.5);
    // Player dimensions are floats in Java's entity type, then widened.
    let half_width = f64::from(0.3_f32);
    let bounds = BoundingBox {
        min: position + DVec3::new(-half_width, 0.0, -half_width),
        max: position + DVec3::new(half_width, f64::from(1.8_f32), half_width),
    };
    if world.has_block_collision(bounds)?
        || world.invalid_spawn_inside(pos)?
        || world.invalid_spawn_inside(offset(pos, IVec3::Y))?
        || !world.within_world_border(bounds)?
    {
        return Ok(None);
    }
    Ok(Some(position))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HorizontalDirection {
    North,
    East,
    South,
    West,
}

impl HorizontalDirection {
    fn step(self) -> IVec3 {
        match self {
            Self::North => IVec3::NEG_Z,
            Self::East => IVec3::X,
            Self::South => IVec3::Z,
            Self::West => IVec3::NEG_X,
        }
    }

    fn clockwise(self) -> Self {
        match self {
            Self::North => Self::East,
            Self::East => Self::South,
            Self::South => Self::West,
            Self::West => Self::North,
        }
    }
}

/// Position arithmetic matches Java's 32-bit `BlockPos` arithmetic.
fn offset(pos: IVec3, by: IVec3) -> IVec3 {
    IVec3::new(
        pos.x.wrapping_add(by.x),
        pos.y.wrapping_add(by.y),
        pos.z.wrapping_add(by.z),
    )
}

fn scan<E>(
    origin: IVec3,
    offsets: &[IVec3],
    avoid_danger: bool,
    find_safe: &mut impl FnMut(IVec3, bool) -> Result<Option<DVec3>, E>,
) -> Result<Option<DVec3>, E> {
    for &delta in offsets {
        if let Some(result) = find_safe(offset(origin, delta), avoid_danger)? {
            return Ok(Some(result));
        }
    }
    Ok(None)
}

/// Search a bed's normal or bunk-bed candidates in vanilla order. `yaw` is
/// the saved Java yaw in degrees, and `bunk` means a bed is directly below.
pub fn bed_stand_up_position<E>(
    pos: IVec3,
    facing: HorizontalDirection,
    yaw: f32,
    bunk: bool,
    mut find_safe: impl FnMut(IVec3, bool) -> Result<Option<DVec3>, E>,
) -> Result<Option<DVec3>, E> {
    let forward = facing.step();
    let mut side = facing.clockwise().step();
    let radians = yaw * 0.017_453_292_f32;
    let x = -crate::physics::java_sin(f64::from(radians), false);
    let z = crate::physics::java_sin(f64::from(radians), true);
    if side.x as f32 * x + side.z as f32 * z > 0.0 {
        side = -side;
    }
    let surround = [
        side,
        side - forward,
        side - 2 * forward,
        -2 * forward,
        -side - 2 * forward,
        -side - forward,
        -side,
        -side + forward,
        forward,
        side + forward,
    ];
    let above = [IVec3::ZERO, -forward];
    for avoid in [true, false] {
        if let Some(result) = scan(pos, &surround, avoid, &mut find_safe)? {
            return Ok(Some(result));
        }
        if bunk
            && let Some(result) = scan(offset(pos, IVec3::NEG_Y), &surround, avoid, &mut find_safe)?
        {
            return Ok(Some(result));
        }
        if let Some(result) = scan(pos, &above, avoid, &mut find_safe)? {
            return Ok(Some(result));
        }
    }
    Ok(None)
}

/// Search the eight surrounding positions, below them, above them, and the
/// space above the anchor, first avoiding dangerous blocks and then without
/// that restriction.
pub fn anchor_stand_up_position<E>(
    pos: IVec3,
    mut find_safe: impl FnMut(IVec3, bool) -> Result<Option<DVec3>, E>,
) -> Result<Option<DVec3>, E> {
    const HORIZONTAL: [IVec3; 8] = [
        IVec3::NEG_Z,
        IVec3::NEG_X,
        IVec3::Z,
        IVec3::X,
        IVec3::new(-1, 0, -1),
        IVec3::new(1, 0, -1),
        IVec3::new(-1, 0, 1),
        IVec3::new(1, 0, 1),
    ];
    for avoid in [true, false] {
        for vertical in [IVec3::ZERO, IVec3::NEG_Y, IVec3::Y] {
            if let Some(result) = scan(offset(pos, vertical), &HORIZONTAL, avoid, &mut find_safe)? {
                return Ok(Some(result));
            }
        }
        if let Some(result) = scan(pos, &[IVec3::Y], avoid, &mut find_safe)? {
            return Ok(Some(result));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_stops_at_unknown_earlier_candidates() {
        let mut calls = 0;
        let result = bed_stand_up_position(
            IVec3::ZERO,
            HorizontalDirection::North,
            0.0,
            false,
            |_, _| -> Result<Option<DVec3>, &'static str> {
                calls += 1;
                if calls == 2 {
                    Err("chunk is not loaded")
                } else {
                    Ok(None)
                }
            },
        );
        assert_eq!(result, Err("chunk is not loaded"));
        assert_eq!(calls, 2);
    }
}
