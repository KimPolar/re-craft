//! Candidate positions for the Java 26.3 `replace_disk` enchantment effect.
//! Predicates, state providers, block updates, and game events are evaluated by
//! the caller in this order; this iterator only performs the geometric test.

#[derive(Clone, Debug)]
pub struct DiskPositions {
    min: [i32; 3],
    width: u64,
    height: u64,
    count: u64,
    next: u64,
    origin: [f64; 3],
    radius_squared: f64,
}

impl DiskPositions {
    /// Construct a bounded iterator, or return `None` when the coordinates or
    /// requested work cannot safely be represented. In that case callers must
    /// treat the effect as unsupported rather than silently truncate it.
    pub fn new(origin: [f64; 3], offset: [i32; 3], radius: i32, height: i32) -> Option<Self> {
        let mut center = [0; 3];
        for axis in 0..3 {
            let coordinate = origin[axis].floor();
            if !coordinate.is_finite()
                || coordinate < i32::MIN as f64
                || coordinate > i32::MAX as f64
            {
                return None;
            }
            center[axis] = (coordinate as i32).checked_add(offset[axis])?;
        }
        let extent = radius.checked_abs()?;
        // Mth.square uses signed 32-bit arithmetic. Reject values for which it
        // overflows; a large region is also left to a future resumable worker.
        let radius_squared = radius.checked_mul(radius)? as f64;
        let min = [
            center[0].checked_sub(extent)?,
            center[1].checked_add(height.wrapping_sub(1).min(0))?,
            center[2].checked_sub(extent)?,
        ];
        center[0].checked_add(extent)?;
        center[2].checked_add(extent)?;
        let width = u64::try_from(i64::from(extent) * 2 + 1).ok()?;
        let height = u64::try_from(i64::from(center[1]) - i64::from(min[1]) + 1).ok()?;
        let count = width.checked_mul(width)?.checked_mul(height)?;
        if count > 1_000_000 {
            return None;
        }
        Some(Self {
            min,
            width,
            height,
            count,
            next: 0,
            origin,
            radius_squared,
        })
    }
}

impl Iterator for DiskPositions {
    type Item = [i32; 3];

    fn next(&mut self) -> Option<Self::Item> {
        while self.next < self.count {
            let index = self.next;
            self.next += 1;
            let x = self.min[0] + (index % self.width) as i32;
            let y = self.min[1] + ((index / self.width) % self.height) as i32;
            let z = self.min[2] + (index / self.width / self.height) as i32;
            let dx = f64::from(x) + 0.5 - self.origin[0];
            let dz = f64::from(z) + 0.5 - self.origin[2];
            if dx * dx + dz * dz < self.radius_squared {
                return Some([x, y, z]);
            }
        }
        None
    }
}
