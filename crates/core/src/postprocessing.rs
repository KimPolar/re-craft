//! Ordered chunk postprocessing positions, as used by Java's ProtoChunk.
//!
//! The per-section lists preserve insertion order and duplicates. A set of
//! marked coordinates alone cannot reconstruct this information.

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostProcessing {
    min_y: i32,
    max_y: i32,
    sections: Vec<Vec<i16>>,
}

impl PostProcessing {
    /// Construct lists for section-aligned bounds, with an exclusive maximum.
    pub fn new(min_y: i32, height: i32) -> Option<Self> {
        let max_y = min_y.checked_add(height)?;
        if height <= 0 || height > 4096 || min_y.rem_euclid(16) != 0 || height % 16 != 0 {
            return None;
        }
        Some(Self {
            min_y,
            max_y,
            sections: vec![Vec::new(); (height / 16) as usize],
        })
    }

    /// Match ProtoChunk.markPosForPostProcessing: out-of-height positions are
    /// ignored, and the low four bits of X and Z are stored.
    pub fn mark(&mut self, x: i32, y: i32, z: i32) -> bool {
        if !(self.min_y..self.max_y).contains(&y) {
            return false;
        }
        let section = ((y - self.min_y) / 16) as usize;
        let packed = (x & 15) | ((y & 15) << 4) | ((z & 15) << 8);
        self.sections[section].push(packed as i16);
        true
    }

    /// Lists are ordered by section from the bottom of the dimension.
    pub fn sections(&self) -> &[Vec<i16>] {
        &self.sections
    }

    /// Decode one packed offset with Java's 32-bit coordinate arithmetic.
    pub fn unpack(
        &self,
        section: usize,
        packed: i16,
        chunk_x: i32,
        chunk_z: i32,
    ) -> Option<[i32; 3]> {
        if section >= self.sections.len() {
            return None;
        }
        let packed = i32::from(packed);
        Some([
            chunk_x.wrapping_mul(16).wrapping_add(packed & 15),
            self.min_y + section as i32 * 16 + ((packed >> 4) & 15),
            chunk_z.wrapping_mul(16).wrapping_add((packed >> 8) & 15),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::PostProcessing;

    #[test]
    fn preserves_order_duplicates_and_negative_coordinates() {
        let mut updates = PostProcessing::new(-64, 384).unwrap();
        assert!(!updates.mark(0, -65, 0));
        assert!(!updates.mark(0, 320, 0));
        assert!(updates.mark(-1, -64, -17));
        assert!(updates.mark(-16, -49, -32));
        assert!(updates.mark(-1, -64, -17));
        assert!(updates.mark(2, -48, 3));
        assert_eq!(updates.sections()[0], [0xf0f, 0x0f0, 0xf0f]);
        assert_eq!(updates.sections()[1], [0x302]);
        assert_eq!(updates.unpack(0, 0xf0f, -1, -2), Some([-1, -64, -17]));
        assert_eq!(updates.unpack(24, 0, 0, 0), None);
        assert!(PostProcessing::new(-63, 384).is_none());
        assert!(PostProcessing::new(0, 0).is_none());
    }
}
