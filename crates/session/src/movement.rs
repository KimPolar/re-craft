use glam::{DVec3, IVec3};
use native_world::{
    collision::Aabb,
    movement::{BoundingBox, CollisionShape, resolve_movement},
};

use crate::PLAYER_HALF_WIDTH;

const STEP_HEIGHT: f32 = 0.6;
const EPSILON: f64 = 1.0e-7;

pub(super) fn player_bounds(position: DVec3, height: f32) -> BoundingBox {
    let half = f64::from(PLAYER_HALF_WIDTH);
    BoundingBox {
        min: position + DVec3::new(-half, 0.0, -half),
        max: position + DVec3::new(half, f64::from(height), half),
    }
}

pub(super) fn can_fit_exact<'a>(
    position: DVec3,
    height: f32,
    mut block_shape: impl FnMut(IVec3) -> &'a [Aabb],
) -> bool {
    no_collision(player_bounds(position, height), &mut block_shape)
}

pub(super) fn no_collision<'a>(
    area: BoundingBox,
    block_shape: &mut impl FnMut(IVec3) -> &'a [Aabb],
) -> bool {
    let start = (area.min - DVec3::splat(EPSILON)).floor().as_ivec3();
    let end = (area.max + DVec3::splat(EPSILON)).floor().as_ivec3();
    for x in start.x..=end.x {
        // A fence can extend above the cell containing it.
        for y in start.y.saturating_sub(1)..=end.y {
            for z in start.z..=end.z {
                let base = IVec3::new(x, y, z).as_dvec3();
                let boxes = block_shape(IVec3::new(x, y, z));
                let full = boxes.len() == 1
                    && boxes[0].min == glam::Vec3::ZERO
                    && boxes[0].max == glam::Vec3::ONE;
                let tolerance = if full { 0.0 } else { EPSILON };
                for b in boxes {
                    let min = base + b.min.as_dvec3();
                    let max = base + b.max.as_dvec3();
                    if area.max.cmpgt(min + DVec3::splat(tolerance)).all()
                        && area.min.cmplt(max - DVec3::splat(tolerance)).all()
                    {
                        return false;
                    }
                }
            }
        }
    }
    true
}

pub(super) fn can_fit<'a>(
    position: DVec3,
    height: f32,
    mut block_shape: impl FnMut(IVec3) -> &'a [Aabb],
) -> bool {
    let bounds = player_bounds(position, height);
    no_collision(
        BoundingBox {
            min: bounds.min + DVec3::splat(EPSILON),
            max: bounds.max - DVec3::splat(EPSILON),
        },
        &mut block_shape,
    )
}

/// Java finds the nearest block intersecting a one-micrometre slice below the
/// player's feet, then falls back to the previous horizontal position once.
pub(super) fn supporting_block<'a>(
    position: DVec3,
    movement: DVec3,
    no_previous_support: bool,
    mut block_shape: impl FnMut(IVec3) -> &'a [Aabb],
) -> Option<IVec3> {
    let find = |offset: DVec3, block_shape: &mut dyn FnMut(IVec3) -> &'a [Aabb]| {
        let half = f64::from(PLAYER_HALF_WIDTH);
        let area = BoundingBox {
            min: position + offset + DVec3::new(-half, -1.0e-6, -half),
            max: position + offset + DVec3::new(half, 0.0, half),
        };
        let start = (area.min - DVec3::splat(EPSILON)).floor().as_ivec3();
        let end = (area.max + DVec3::splat(EPSILON)).floor().as_ivec3();
        let mut best: Option<(IVec3, f64)> = None;
        for x in start.x..=end.x {
            for y in start.y.saturating_sub(1)..=end.y {
                for z in start.z..=end.z {
                    let cell = IVec3::new(x, y, z);
                    let base = cell.as_dvec3();
                    let boxes = block_shape(cell);
                    let full = boxes.len() == 1
                        && boxes[0].min == glam::Vec3::ZERO
                        && boxes[0].max == glam::Vec3::ONE;
                    let tolerance = if full { 0.0 } else { EPSILON };
                    if !boxes.iter().any(|shape| {
                        area.max
                            .cmpgt(base + shape.min.as_dvec3() + DVec3::splat(tolerance))
                            .all()
                            && area
                                .min
                                .cmplt(base + shape.max.as_dvec3() - DVec3::splat(tolerance))
                                .all()
                    }) {
                        continue;
                    }
                    let distance = (base + DVec3::splat(0.5) - position).length_squared();
                    let key = (cell.y, cell.z, cell.x);
                    if best.is_none_or(|(old, old_distance)| {
                        distance < old_distance
                            || (distance == old_distance && key > (old.y, old.z, old.x))
                    }) {
                        best = Some((cell, distance));
                    }
                }
            }
        }
        best.map(|(cell, _)| cell)
    };
    find(DVec3::ZERO, &mut block_shape).or_else(|| {
        (!no_previous_support)
            .then(|| find(DVec3::new(-movement.x, 0.0, -movement.z), &mut block_shape))
            .flatten()
    })
}

/// `getOnPosLegacy` preserves fence-like supporting blocks, but otherwise
/// uses the block containing the point 0.2 below the player's feet.
pub(super) fn legacy_on_block(
    position: DVec3,
    support: Option<IVec3>,
    keep_support_height: impl FnOnce(IVec3) -> bool,
) -> IVec3 {
    let y = (position.y - f64::from(0.2_f32)).floor() as i32;
    match support {
        Some(cell) if keep_support_height(cell) => cell,
        Some(cell) => IVec3::new(cell.x, y, cell.z),
        None => IVec3::new(position.x.floor() as i32, y, position.z.floor() as i32),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn back_off<'a>(
    position: DVec3,
    requested: DVec3,
    on_ground: bool,
    fall_distance: f64,
    height: f32,
    sneaking: bool,
    mut block_shape: impl FnMut(IVec3) -> &'a [Aabb],
) -> DVec3 {
    native_world::movement::back_off_from_edge(
        player_bounds(position, height),
        requested,
        on_ground,
        fall_distance,
        STEP_HEIGHT,
        sneaking,
        |area| no_collision(area, &mut block_shape),
    )
}

/// Collect block-local boxes around the swept player and resolve the displacement.
pub(super) fn resolve_player<'a>(
    position: DVec3,
    movement: DVec3,
    on_ground: bool,
    height: f32,
    block_shape: impl FnMut(IVec3) -> &'a [Aabb],
) -> DVec3 {
    let bounds = player_bounds(position, height);
    resolve_box(bounds, movement, on_ground, STEP_HEIGHT, block_shape)
}

/// Resolve another entity's box against the same block shapes as the player.
pub(super) fn resolve_box<'a>(
    bounds: BoundingBox,
    movement: DVec3,
    on_ground: bool,
    step_height: f32,
    mut block_shape: impl FnMut(IVec3) -> &'a [Aabb],
) -> DVec3 {
    let requested = movement;
    let search = bounds
        .expanded_towards(requested)
        .expanded_towards(DVec3::new(0.0, f64::from(step_height), 0.0));
    let start = (search.min - DVec3::splat(1.0e-6)).floor().as_ivec3();
    let end = (search.max + DVec3::splat(1.0e-6)).floor().as_ivec3();
    let mut shapes = Vec::new();
    for x in start.x..=end.x {
        // Fences extend up to half a block above their own cell.
        for y in start.y.saturating_sub(1)..=end.y {
            for z in start.z..=end.z {
                let cell = IVec3::new(x, y, z);
                let local = block_shape(cell);
                if local.is_empty() {
                    continue;
                }
                let base = cell.as_dvec3();
                shapes.push(CollisionShape::new(
                    local
                        .iter()
                        .map(|b| BoundingBox {
                            min: base + b.min.as_dvec3(),
                            max: base + b.max.as_dvec3(),
                        })
                        .collect(),
                ));
            }
        }
    }
    resolve_movement(bounds, requested, on_ground, step_height, &shapes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supporting_block_breaks_ties_and_checks_the_previous_footprint_once() {
        const FULL: &[Aabb] = &[Aabb {
            min: glam::Vec3::ZERO,
            max: glam::Vec3::ONE,
        }];
        let ground = |pos: IVec3| {
            if pos.y == 0 && pos.z == 0 && (pos.x == 0 || pos.x == 1) {
                FULL
            } else {
                &[]
            }
        };
        assert_eq!(
            supporting_block(DVec3::new(1.0, 1.0, 0.5), DVec3::ZERO, false, ground),
            Some(IVec3::new(1, 0, 0))
        );
        let left = |pos: IVec3| if pos == IVec3::ZERO { FULL } else { &[] };
        assert_eq!(
            supporting_block(DVec3::new(1.5, 1.0, 0.5), DVec3::X, false, left),
            Some(IVec3::ZERO)
        );
        assert_eq!(
            supporting_block(DVec3::new(1.5, 1.0, 0.5), DVec3::X, true, left),
            None
        );
    }
}
