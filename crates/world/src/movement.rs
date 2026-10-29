//! Block-shape movement clipping and step-up resolution.
//!
//! This only resolves movement against supplied shapes. Entity motion, fluids,
//! world borders and effects still need their own simulation.

use glam::DVec3;

const EPSILON: f64 = 1.0e-7;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoundingBox {
    pub min: DVec3,
    pub max: DVec3,
}

impl BoundingBox {
    /// Earliest contact along a translation, using Java 26.3's expanded-box
    /// and face tolerance rules. A contact already present at the start is 0.
    pub fn swept_contact_time(self, delta: DVec3, target: Self) -> Option<f64> {
        if !self.min.is_finite()
            || !self.max.is_finite()
            || !delta.is_finite()
            || !target.min.is_finite()
            || !target.max.is_finite()
        {
            return None;
        }
        let center = (self.min + self.max) * 0.5;
        let end = center + delta;
        let half = (self.max - self.min) * 0.5 - DVec3::splat(1.0e-7);
        let min = target.min - half;
        let max = target.max + half;
        let contains = |point: DVec3| point.cmpge(min).all() && point.cmplt(max).all();
        if contains(center) {
            return Some(0.0);
        }
        let end_inside = contains(end);
        let mut first = 1.0_f64;
        let mut hit = false;
        for axis in 0..3 {
            let movement = delta[axis];
            if movement.abs() <= 1.0e-7 {
                continue;
            }
            let plane = if movement > 0.0 { min[axis] } else { max[axis] };
            let time = (plane - center[axis]) / movement;
            if !(0.0 < time && time < first) {
                continue;
            }
            let point = center + delta * time;
            let a = (axis + 1) % 3;
            let b = (axis + 2) % 3;
            if point[a] > min[a] - 1.0e-7
                && point[a] < max[a] + 1.0e-7
                && point[b] > min[b] - 1.0e-7
                && point[b] < max[b] + 1.0e-7
            {
                first = time;
                hit = true;
            }
        }
        if hit {
            Some(first)
        } else if end_inside {
            Some(1.0)
        } else {
            None
        }
    }

    pub fn moved(self, delta: DVec3) -> Self {
        Self {
            min: self.min + delta,
            max: self.max + delta,
        }
    }

    pub fn expanded_towards(self, delta: DVec3) -> Self {
        Self {
            min: self.min + delta.min(DVec3::ZERO),
            max: self.max + delta.max(DVec3::ZERO),
        }
    }

    fn intersects(self, other: Self, tolerance: f64) -> bool {
        self.max.cmpgt(other.min + DVec3::splat(tolerance)).all()
            && self.min.cmplt(other.max - DVec3::splat(tolerance)).all()
    }
}

/// Apply the 26.3 player's edge restraint before resolving block collisions.
/// `no_collision` must check the complete queried volume against world shapes.
pub fn back_off_from_edge(
    bounds: BoundingBox,
    requested: DVec3,
    on_ground: bool,
    fall_distance: f64,
    max_step: f32,
    staying_on_ground: bool,
    mut no_collision: impl FnMut(BoundingBox) -> bool,
) -> DVec3 {
    if !staying_on_ground || requested.y > 0.0 {
        return requested;
    }
    let probe = |x: f64, z: f64, depth: f64| BoundingBox {
        min: DVec3::new(
            bounds.min.x + EPSILON + x,
            bounds.min.y - depth - EPSILON,
            bounds.min.z + EPSILON + z,
        ),
        max: DVec3::new(
            bounds.max.x - EPSILON + x,
            bounds.min.y,
            bounds.max.z - EPSILON + z,
        ),
    };
    let step = f64::from(max_step);
    if !on_ground && (fall_distance >= step || no_collision(probe(0.0, 0.0, step - fall_distance)))
    {
        return requested;
    }
    let mut x = requested.x;
    let mut z = requested.z;
    let step_x = x.signum() * 0.05;
    let step_z = z.signum() * 0.05;
    while x != 0.0 && no_collision(probe(x, 0.0, step)) {
        x = if x.abs() <= 0.05 { 0.0 } else { x - step_x };
    }
    while z != 0.0 && no_collision(probe(0.0, z, step)) {
        z = if z.abs() <= 0.05 { 0.0 } else { z - step_z };
    }
    while x != 0.0 && z != 0.0 && no_collision(probe(x, z, step)) {
        x = if x.abs() <= 0.05 { 0.0 } else { x - step_x };
        z = if z.abs() <= 0.05 { 0.0 } else { z - step_z };
    }
    DVec3::new(x, requested.y, z)
}

/// One voxel shape, represented by its occupied boxes and coordinate grid.
#[derive(Clone, Debug)]
pub struct CollisionShape {
    boxes: Vec<BoundingBox>,
    axes: [Vec<f64>; 3],
    cells: Vec<bool>,
    sizes: [usize; 3],
}

impl CollisionShape {
    pub fn new(boxes: Vec<BoundingBox>) -> Self {
        let axes: [Vec<f64>; 3] = std::array::from_fn(|axis| {
            let mut cuts: Vec<_> = boxes
                .iter()
                .flat_map(|b| [b.min[axis], b.max[axis]])
                .collect();
            cuts.sort_by(f64::total_cmp);
            cuts.dedup();
            cuts
        });
        let sizes = std::array::from_fn(|axis| axes[axis].len().saturating_sub(1));
        let mut cells = vec![false; sizes[0] * sizes[1] * sizes[2]];
        for x in 0..sizes[0] {
            for y in 0..sizes[1] {
                for z in 0..sizes[2] {
                    let point = DVec3::new(
                        (axes[0][x] + axes[0][x + 1]) * 0.5,
                        (axes[1][y] + axes[1][y + 1]) * 0.5,
                        (axes[2][z] + axes[2][z + 1]) * 0.5,
                    );
                    cells[(x * sizes[1] + y) * sizes[2] + z] = boxes
                        .iter()
                        .any(|b| point.cmpgt(b.min).all() && point.cmplt(b.max).all());
                }
            }
        }
        Self {
            boxes,
            axes,
            cells,
            sizes,
        }
    }

    fn full(&self, index: [usize; 3]) -> bool {
        self.cells[(index[0] * self.sizes[1] + index[1]) * self.sizes[2] + index[2]]
    }

    fn is_full_block(&self) -> bool {
        self.boxes.len() == 1
            && self.boxes[0].max - self.boxes[0].min == DVec3::ONE
            && self.boxes[0].min == self.boxes[0].min.floor()
    }

    fn index(&self, axis: usize, coordinate: f64) -> isize {
        self.axes[axis].partition_point(|cut| *cut <= coordinate) as isize - 1
    }

    fn clip(&self, bounds: BoundingBox, axis: usize, amount: f64) -> f64 {
        if self.cells.is_empty() {
            return amount;
        }
        if amount.abs() < EPSILON {
            return 0.0;
        }
        let others: Vec<_> = (0..3).filter(|other| *other != axis).collect();
        let a = others[0];
        let b = others[1];
        let start_a = self.index(a, bounds.min[a] + EPSILON).max(0) as usize;
        let end_a = (self.index(a, bounds.max[a] - EPSILON) + 1)
            .max(0)
            .min(self.sizes[a] as isize) as usize;
        let start_b = self.index(b, bounds.min[b] + EPSILON).max(0) as usize;
        let end_b = (self.index(b, bounds.max[b] - EPSILON) + 1)
            .max(0)
            .min(self.sizes[b] as isize) as usize;
        let mut main = if amount > 0.0 {
            self.index(axis, bounds.max[axis] - EPSILON) + 1
        } else {
            self.index(axis, bounds.min[axis] + EPSILON) - 1
        };
        while main >= 0 && main < self.sizes[axis] as isize {
            for ia in start_a..end_a {
                for ib in start_b..end_b {
                    let mut index = [0; 3];
                    index[axis] = main as usize;
                    index[a] = ia;
                    index[b] = ib;
                    if self.full(index) {
                        let gap = if amount > 0.0 {
                            self.axes[axis][main as usize] - bounds.max[axis]
                        } else {
                            self.axes[axis][main as usize + 1] - bounds.min[axis]
                        };
                        return if amount > 0.0 && gap >= -EPSILON {
                            amount.min(gap)
                        } else if amount < 0.0 && gap <= EPSILON {
                            amount.max(gap)
                        } else {
                            amount
                        };
                    }
                }
            }
            main += if amount > 0.0 { 1 } else { -1 };
        }
        amount
    }
}

fn relevant(shapes: &[CollisionShape], area: BoundingBox) -> Vec<&CollisionShape> {
    shapes
        .iter()
        .filter(|shape| {
            let tolerance = if shape.is_full_block() { 0.0 } else { EPSILON };
            shape
                .boxes
                .iter()
                .any(|bounds| area.intersects(*bounds, tolerance))
        })
        .collect()
}

fn clip_axis(bounds: BoundingBox, shapes: &[&CollisionShape], axis: usize, amount: f64) -> f64 {
    let mut result = amount;
    for shape in shapes {
        if result.abs() < EPSILON {
            return 0.0;
        }
        result = shape.clip(bounds, axis, result);
    }
    result
}

fn clip(bounds: BoundingBox, movement: DVec3, shapes: &[&CollisionShape]) -> DVec3 {
    if shapes.is_empty() {
        return movement;
    }
    let axes = if movement.x.abs() < movement.z.abs() {
        [1, 2, 0]
    } else {
        [1, 0, 2]
    };
    let mut result = DVec3::ZERO;
    for axis in axes {
        if movement[axis] != 0.0 {
            result[axis] = clip_axis(bounds.moved(result), shapes, axis, movement[axis]);
        }
    }
    result
}

/// Resolve a player's requested displacement against nearby block shapes.
/// Shapes must cover the complete swept box plus the step-up region.
pub fn resolve_movement(
    bounds: BoundingBox,
    movement: DVec3,
    on_ground: bool,
    max_step: f32,
    shapes: &[CollisionShape],
) -> DVec3 {
    if movement == DVec3::ZERO {
        return movement;
    }
    let base_shapes = relevant(shapes, bounds.expanded_towards(movement));
    let base = clip(bounds, movement, &base_shapes);
    let blocked_down = movement.y != base.y && movement.y < 0.0;
    if max_step <= 0.0
        || !(on_ground || blocked_down)
        || (movement.x == base.x && movement.z == base.z)
    {
        return base;
    }
    let step_bounds = if blocked_down {
        bounds.moved(DVec3::new(0.0, base.y, 0.0))
    } else {
        bounds
    };
    let mut search =
        step_bounds.expanded_towards(DVec3::new(movement.x, f64::from(max_step), movement.z));
    if !blocked_down {
        search = search.expanded_towards(DVec3::new(0.0, -f64::from(1.0e-5_f32), 0.0));
    }
    let step_shapes = relevant(shapes, search);
    let mut heights: Vec<f32> = step_shapes
        .iter()
        .flat_map(|shape| shape.axes[1].iter())
        .map(|height| (*height - step_bounds.min.y) as f32)
        .filter(|height| *height >= 0.0 && *height != base.y as f32 && *height <= max_step)
        .collect();
    heights.sort_by(f32::total_cmp);
    heights.dedup();
    for height in heights {
        let step = clip(
            step_bounds,
            DVec3::new(movement.x, f64::from(height), movement.z),
            &step_shapes,
        );
        if step.x * step.x + step.z * step.z > base.x * base.x + base.z * base.z {
            return step + DVec3::new(0.0, step_bounds.min.y - bounds.min.y, 0.0);
        }
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_(min: [f64; 3], max: [f64; 3]) -> BoundingBox {
        BoundingBox {
            min: min.into(),
            max: max.into(),
        }
    }

    #[test]
    fn step_up_accepts_slab_but_not_a_full_block_or_low_ceiling() {
        let player = box_([0.2, 1.0, 0.2], [0.8, 2.8, 0.8]);
        let slab = CollisionShape::new(vec![box_([1.0, 1.0, 0.0], [2.0, 1.5, 1.0])]);
        let full = CollisionShape::new(vec![box_([1.0, 1.0, 0.0], [2.0, 2.0, 1.0])]);
        let ceiling = CollisionShape::new(vec![box_([0.0, 2.9, 0.0], [2.0, 3.9, 1.0])]);
        let motion = DVec3::new(0.5, -0.1, 0.0);
        let floor = CollisionShape::new(vec![box_([0.0, 0.0, 0.0], [2.0, 1.0, 1.0])]);
        assert_eq!(
            resolve_movement(player, motion, true, 0.6, &[slab.clone(), floor.clone()]),
            DVec3::new(0.5, 0.5, 0.0)
        );
        let blocked = DVec3::new(0.2, 0.0, 0.0);
        assert!(
            (resolve_movement(player, motion, true, 0.6, &[full, floor.clone()]) - blocked)
                .abs()
                .max_element()
                < 1.0e-12
        );
        assert!(
            (resolve_movement(player, motion, true, 0.6, &[slab, floor, ceiling]) - blocked)
                .abs()
                .max_element()
                < 1.0e-12
        );
    }
}
