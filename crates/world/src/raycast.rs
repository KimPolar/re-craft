//! Shape-aware block targeting. Missing cells terminate the traversal.

use glam::{DVec3, IVec3, Vec3};

use crate::collision::Aabb;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeRayHit {
    pub position: IVec3,
    /// Outward normal of the face that was hit.
    pub face: IVec3,
    pub location: DVec3,
    pub inside: bool,
}

/// Clip a finite segment against a block-local union of boxes.
pub fn clip_shape(shape: &[Aabb], cell: IVec3, start: DVec3, end: DVec3) -> Option<ShapeRayHit> {
    let delta = end - start;
    if shape.is_empty() || !start.is_finite() || !end.is_finite() || delta.length_squared() < 1.0e-7
    {
        return None;
    }
    let offset = cell.as_dvec3();
    let inside_point = start + delta * 0.001;
    if shape.iter().any(|box_| {
        let min = offset + box_.min.as_dvec3();
        let max = offset + box_.max.as_dvec3();
        inside_point.cmpge(min).all() && inside_point.cmplt(max).all()
    }) {
        // Java chooses the first direction with the greatest f32 dot product.
        let mut nearest = IVec3::NEG_Z;
        let mut best = f32::from_bits(1);
        let direction = delta.as_vec3();
        for face in [
            IVec3::NEG_Y,
            IVec3::Y,
            IVec3::NEG_Z,
            IVec3::Z,
            IVec3::NEG_X,
            IVec3::X,
        ] {
            let score = direction.dot(face.as_vec3());
            if score > best {
                best = score;
                nearest = face;
            }
        }
        return Some(ShapeRayHit {
            position: cell,
            face: -nearest,
            location: inside_point,
            inside: true,
        });
    }

    let mut closest = 1.0f64;
    let mut hit_face = None;
    for box_ in shape {
        let min = offset + box_.min.as_dvec3();
        let max = offset + box_.max.as_dvec3();
        for axis in 0..3 {
            let component = delta[axis];
            if component.abs() <= 1.0e-7 {
                continue;
            }
            let plane = if component > 0.0 {
                min[axis]
            } else {
                max[axis]
            };
            let t = (plane - start[axis]) / component;
            if t <= 0.0 || t >= closest {
                continue;
            }
            let point = start + delta * t;
            if (0..3).filter(|other| *other != axis).all(|other| {
                point[other] > min[other] - 1.0e-7 && point[other] < max[other] + 1.0e-7
            }) {
                closest = t;
                let mut face = IVec3::ZERO;
                face[axis] = if component > 0.0 { -1 } else { 1 };
                hit_face = Some(face);
            }
        }
    }
    hit_face.map(|face| ShapeRayHit {
        position: cell,
        face,
        location: start + delta * closest,
        inside: false,
    })
}

/// Traverse cells in order. A `None` cell is unavailable and blocks further targeting.
pub fn raycast_shapes<'a>(
    origin: Vec3,
    direction: Vec3,
    max_distance: f32,
    mut shape: impl FnMut(IVec3) -> Option<&'a [Aabb]>,
) -> Option<ShapeRayHit> {
    if !origin.is_finite()
        || !direction.is_finite()
        || direction.length_squared() <= f32::EPSILON
        || !max_distance.is_finite()
        || max_distance < 0.0
    {
        return None;
    }
    let direction = direction.normalize();
    let end = origin.as_dvec3() + direction.as_dvec3() * f64::from(max_distance);
    let start = origin.as_dvec3();
    if start == end {
        return None;
    }
    // Vanilla extends both ends by a tiny amount before visiting block cells.
    // Shape clipping still uses the unextended segment.
    let traversal_start = start + (end - start) * -1.0e-7;
    let traversal_end = end + (start - end) * -1.0e-7;
    let traversal_delta = traversal_end - traversal_start;
    let mut cell = traversal_start.floor().as_ivec3();
    let step_for = |component: f64| {
        if component > 0.0 {
            1
        } else if component < 0.0 {
            -1
        } else {
            0
        }
    };
    let step = IVec3::new(
        step_for(traversal_delta.x),
        step_for(traversal_delta.y),
        step_for(traversal_delta.z),
    );
    let mut delta = DVec3::splat(f64::MAX);
    let mut next = DVec3::ZERO;
    for axis in 0..3 {
        if step[axis] != 0 {
            delta[axis] = f64::from(step[axis]) / traversal_delta[axis];
        }
        let fraction = traversal_start[axis] - traversal_start[axis].floor();
        next[axis] = delta[axis]
            * if step[axis] > 0 {
                1.0 - fraction
            } else {
                fraction
            };
    }
    loop {
        let boxes = shape(cell)?;
        if let Some(hit) = clip_shape(boxes, cell, start, end) {
            return Some(hit);
        }
        if next.x > 1.0 && next.y > 1.0 && next.z > 1.0 {
            return None;
        }
        let axis = if next.x < next.y {
            if next.x < next.z { 0 } else { 2 }
        } else if next.y < next.z {
            1
        } else {
            2
        };
        next[axis] += delta[axis];
        cell[axis] = cell[axis].checked_add(step[axis])?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: [Aabb; 1] = [Aabb {
        min: Vec3::ZERO,
        max: Vec3::ONE,
    }];
    const SLAB: [Aabb; 1] = [Aabb {
        min: Vec3::ZERO,
        max: Vec3::new(1.0, 0.5, 1.0),
    }];

    #[test]
    fn partial_block_is_skipped_when_the_ray_passes_above_it() {
        let hit = raycast_shapes(Vec3::new(0.5, 0.75, 2.5), Vec3::NEG_Z, 5.0, |cell| {
            if cell == IVec3::new(0, 0, 1) {
                Some(&SLAB)
            } else if cell == IVec3::ZERO {
                Some(&FULL)
            } else {
                Some(&[])
            }
        })
        .unwrap();
        assert_eq!(hit.position, IVec3::ZERO);
        assert_eq!(hit.face, IVec3::Z);
        assert_eq!(hit.location.z, 1.0);
    }

    #[test]
    fn slab_top_inside_hits_and_missing_cells_are_explicit() {
        let top = clip_shape(
            &SLAB,
            IVec3::ZERO,
            DVec3::new(0.5, 2.0, 0.5),
            DVec3::new(0.5, -2.0, 0.5),
        )
        .unwrap();
        assert_eq!(top.face, IVec3::Y);
        assert_eq!(top.location.y, 0.5);
        assert!(!top.inside);
        let inside = clip_shape(
            &SLAB,
            IVec3::ZERO,
            DVec3::new(0.5, 0.25, 0.5),
            DVec3::new(5.5, 0.25, 0.5),
        )
        .unwrap();
        assert_eq!(inside.face, IVec3::NEG_X);
        assert!((inside.location.x - 0.505).abs() < 1.0e-9);
        assert!(inside.inside);
        assert!(
            raycast_shapes(Vec3::new(0.5, 0.75, 2.5), Vec3::NEG_Z, 5.0, |cell| {
                if cell == IVec3::new(0, 0, 1) {
                    None
                } else if cell == IVec3::ZERO {
                    Some(&FULL)
                } else {
                    Some(&[])
                }
            })
            .is_none()
        );
    }

    #[test]
    fn boundary_traversal_uses_vanilla_axis_order() {
        let mut visited = Vec::new();
        let hit = raycast_shapes(Vec3::ZERO, Vec3::new(1.0, 1.0, 0.0), 2.0, |cell| {
            visited.push(cell);
            Some(&[])
        });
        assert!(hit.is_none());
        // The zero-length Z step is visited once before the equal Y/X crossings.
        assert_eq!(
            &visited[..4],
            &[
                IVec3::new(-1, -1, 0),
                IVec3::new(-1, -1, 0),
                IVec3::new(-1, 0, 0),
                IVec3::ZERO,
            ]
        );
    }
}
