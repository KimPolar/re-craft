//! Initial 26.3 mineshaft room. Children, vertical placement and block
//! decoration remain separate; this intermediate is not a structure start.

use crate::structure_start::HorizontalDirection;
use anyhow::{Context, Result};
use native_core::{beardifier::BoundingBox, random::LegacyRandom};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MineshaftVariant {
    Normal,
    Mesa,
}

/// The next candidate type; its bounds and collision can still reject it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MineshaftPieceKind {
    Corridor,
    Stairs,
    Crossing,
}

/// Java makes one bounded random draw before trying the selected piece.
pub fn choose_piece_kind(random: &mut LegacyRandom) -> MineshaftPieceKind {
    match random.next_int_bound(100).expect("positive bound") {
        0..=69 => MineshaftPieceKind::Corridor,
        70..=79 => MineshaftPieceKind::Stairs,
        _ => MineshaftPieceKind::Crossing,
    }
}

fn candidate_box(
    origin: [i32; 3],
    min: [i32; 3],
    max: [i32; 3],
    occupied: &[BoundingBox],
) -> Result<Option<BoundingBox>> {
    let mut lo = [0; 3];
    let mut hi = [0; 3];
    for axis in 0..3 {
        lo[axis] = origin[axis]
            .checked_add(min[axis])
            .context("mineshaft piece coordinate overflow")?;
        hi[axis] = origin[axis]
            .checked_add(max[axis])
            .context("mineshaft piece coordinate overflow")?;
    }
    let candidate = BoundingBox::new(lo, hi).map_err(anyhow::Error::msg)?;
    if occupied.iter().any(|other| {
        (0..3).all(|axis| {
            candidate.max[axis] >= other.min[axis] && candidate.min[axis] <= other.max[axis]
        })
    }) {
        Ok(None)
    } else {
        Ok(Some(candidate))
    }
}

/// Find a stairs bounding box, rejecting any overlap with existing pieces.
pub fn find_stairs(
    origin: [i32; 3],
    direction: HorizontalDirection,
    occupied: &[BoundingBox],
) -> Result<Option<BoundingBox>> {
    use HorizontalDirection::*;
    let (min, max) = match direction {
        North => ([0, -5, -8], [2, 2, 0]),
        South => ([0, -5, 0], [2, 2, 8]),
        West => ([-8, -5, 0], [0, 2, 2]),
        East => ([0, -5, 0], [8, 2, 2]),
    };
    candidate_box(origin, min, max, occupied)
}

/// Find a crossing after sampling its height, even when collision rejects it.
pub fn find_crossing(
    random: &mut LegacyRandom,
    origin: [i32; 3],
    direction: HorizontalDirection,
    occupied: &[BoundingBox],
) -> Result<Option<BoundingBox>> {
    use HorizontalDirection::*;
    let height = if random.next_int_bound(4).expect("positive bound") == 0 {
        6
    } else {
        2
    };
    let (min, max) = match direction {
        North => ([-1, 0, -4], [3, height, 0]),
        South => ([-1, 0, 0], [3, height, 4]),
        West => ([-4, 0, -1], [0, height, 3]),
        East => ([0, 0, -1], [4, height, 3]),
    };
    candidate_box(origin, min, max, occupied)
}

/// Try the sampled corridor length, shortening it in five-block sections when
/// it collides. Shortening consumes no additional random values.
pub fn find_corridor(
    random: &mut LegacyRandom,
    origin: [i32; 3],
    direction: HorizontalDirection,
    occupied: &[BoundingBox],
) -> Result<Option<BoundingBox>> {
    use HorizontalDirection::*;
    let sections = 2 + random.next_int_bound(3).expect("positive bound");
    for count in (1..=sections).rev() {
        let length = count * 5 - 1;
        let (min, max) = match direction {
            North => ([0, 0, -length], [2, 2, 0]),
            South => ([0, 0, 0], [2, 2, length]),
            West => ([-length, 0, 0], [0, 2, 2]),
            East => ([0, 0, 0], [length, 2, 2]),
        };
        if let Some(candidate) = candidate_box(origin, min, max, occupied)? {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

/// The root room before Java recursively builds and vertically moves pieces.
pub struct InitialMineshaftRoom {
    pub bounds: BoundingBox,
    pub variant: MineshaftVariant,
    random: LegacyRandom,
}

impl InitialMineshaftRoom {
    /// Reproduce the large-feature seed, discarded double, and root-room
    /// dimensions. The caller must finish child generation before placement.
    pub fn new(seed: i64, chunk: [i32; 2], variant: MineshaftVariant) -> Result<Self> {
        let x = chunk[0]
            .checked_mul(16)
            .and_then(|v| v.checked_add(2))
            .context("mineshaft chunk X is out of range")?;
        let z = chunk[1]
            .checked_mul(16)
            .and_then(|v| v.checked_add(2))
            .context("mineshaft chunk Z is out of range")?;
        let mut random = crate::structure_placement::large_feature(seed, chunk[0], chunk[1]);
        random.next_double();
        let width = 7 + random.next_int_bound(6).expect("positive bound");
        let height = 54 + random.next_int_bound(6).expect("positive bound");
        let depth = 7 + random.next_int_bound(6).expect("positive bound");
        let bounds = BoundingBox::new(
            [x, 50, z],
            [
                x.checked_add(width).context("mineshaft room X overflow")?,
                height,
                z.checked_add(depth).context("mineshaft room Z overflow")?,
            ],
        )
        .map_err(anyhow::Error::msg)?;
        Ok(Self {
            bounds,
            variant,
            random,
        })
    }

    /// Preserve the random stream for the recursive piece builder.
    pub fn into_parts(self) -> (BoundingBox, MineshaftVariant, LegacyRandom) {
        (self.bounds, self.variant, self.random)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_room_rejects_coordinate_overflow() {
        for chunk in [[i32::MAX, 0], [i32::MIN, 0], [0, i32::MAX], [0, i32::MIN]] {
            assert!(InitialMineshaftRoom::new(0, chunk, MineshaftVariant::Normal).is_err());
        }
    }
}
