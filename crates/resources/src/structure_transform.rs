//! StructureTemplate coordinate transforms from Java 26.3.
use anyhow::Result;
use native_core::{beardifier::BoundingBox, random::LegacyRandom};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Rotation {
    None,
    Clockwise90,
    Clockwise180,
    Counterclockwise90,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mirror {
    None,
    LeftRight,
    FrontBack,
}

impl Rotation {
    pub(crate) fn random(random: &mut LegacyRandom) -> Self {
        match random.next_int_bound(4).unwrap() {
            0 => Self::None,
            1 => Self::Clockwise90,
            2 => Self::Clockwise180,
            _ => Self::Counterclockwise90,
        }
    }

    pub(crate) fn transform_horizontal(self, [x, z]: [i32; 2]) -> [i32; 2] {
        let transformed = transform_block([x, 0, z], Mirror::None, self, [0, 0, 0]);
        [transformed[0], transformed[2]]
    }
}

/// Transform a block-relative coordinate around the pivot, with Java's i32 wrapping.
pub fn transform_block(
    [mut x, y, mut z]: [i32; 3],
    mirror: Mirror,
    rotation: Rotation,
    pivot: [i32; 3],
) -> [i32; 3] {
    match mirror {
        Mirror::None => {}
        Mirror::LeftRight => z = z.wrapping_neg(),
        Mirror::FrontBack => x = x.wrapping_neg(),
    }
    let [px, _, pz] = pivot;
    match rotation {
        Rotation::None => [x, y, z],
        Rotation::Clockwise90 => [
            px.wrapping_add(pz).wrapping_sub(z),
            y,
            pz.wrapping_sub(px).wrapping_add(x),
        ],
        Rotation::Clockwise180 => [
            px.wrapping_add(px).wrapping_sub(x),
            y,
            pz.wrapping_add(pz).wrapping_sub(z),
        ],
        Rotation::Counterclockwise90 => [
            px.wrapping_sub(pz).wrapping_add(z),
            y,
            px.wrapping_add(pz).wrapping_sub(x),
        ],
    }
}

/// Transform an entity-relative position; Java reflects around block centers.
pub fn transform_entity(
    [mut x, y, mut z]: [f64; 3],
    mirror: Mirror,
    rotation: Rotation,
    pivot: [i32; 3],
) -> [f64; 3] {
    match mirror {
        Mirror::None => {}
        Mirror::LeftRight => z = 1.0 - z,
        Mirror::FrontBack => x = 1.0 - x,
    }
    let [px, _, pz] = pivot;
    match rotation {
        Rotation::None => [x, y, z],
        Rotation::Clockwise90 => [
            f64::from(px.wrapping_add(pz).wrapping_add(1)) - z,
            y,
            f64::from(pz.wrapping_sub(px)) + x,
        ],
        Rotation::Clockwise180 => [
            f64::from(px.wrapping_add(px).wrapping_add(1)) - x,
            y,
            f64::from(pz.wrapping_add(pz).wrapping_add(1)) - z,
        ],
        Rotation::Counterclockwise90 => [
            f64::from(px.wrapping_sub(pz)) + z,
            y,
            f64::from(px.wrapping_add(pz).wrapping_add(1)) - x,
        ],
    }
}

pub fn block_world_position(
    local: [i32; 3],
    origin: [i32; 3],
    mirror: Mirror,
    rotation: Rotation,
    pivot: [i32; 3],
) -> [i32; 3] {
    let local = transform_block(local, mirror, rotation, pivot);
    [
        local[0].wrapping_add(origin[0]),
        local[1].wrapping_add(origin[1]),
        local[2].wrapping_add(origin[2]),
    ]
}

/// The same transformed-corner bounds used by StructureTemplate.getBoundingBox.
pub fn bounding_box(
    size: [i32; 3],
    origin: [i32; 3],
    mirror: Mirror,
    rotation: Rotation,
    pivot: [i32; 3],
) -> Result<BoundingBox> {
    let first = transform_block([0; 3], mirror, rotation, pivot);
    let last = transform_block(size.map(|v| v.wrapping_sub(1)), mirror, rotation, pivot);
    let min = std::array::from_fn(|i| first[i].min(last[i]).wrapping_add(origin[i]));
    let max = std::array::from_fn(|i| first[i].max(last[i]).wrapping_add(origin[i]));
    BoundingBox::new(min, max).map_err(anyhow::Error::msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_and_entity_transforms_respect_pivot_and_block_centers() {
        let pivot = [-2, 0, 3];
        assert_eq!(
            transform_block([4, 7, -2], Mirror::LeftRight, Rotation::Clockwise90, pivot),
            [-1, 7, 9]
        );
        assert_eq!(
            transform_entity(
                [4.5, 7.0, -1.5],
                Mirror::LeftRight,
                Rotation::Clockwise90,
                pivot
            ),
            [-0.5, 7.0, 9.5]
        );
        let bounds = bounding_box(
            [3, 2, 4],
            [10, 20, -5],
            Mirror::None,
            Rotation::Clockwise90,
            [0; 3],
        )
        .unwrap();
        assert_eq!(bounds.min, [7, 20, -5]);
        assert_eq!(bounds.max, [10, 21, -3]);
    }
}
