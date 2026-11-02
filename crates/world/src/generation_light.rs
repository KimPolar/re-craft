//! A consistent view of the light engine's visible section storage.
//!
//! FEATURES runs before INITIALIZE_LIGHT for the target chunk. Java therefore
//! reads the light engine's published data, which can differ from a fresh
//! calculation of the generated terrain. Missing *allocated* sections and
//! columns not included in a snapshot must not be conflated.
use crate::loaded::{ChunkPos, LIGHT_BYTES};
use std::{collections::BTreeMap, sync::Arc};

/// Nibble-packed visible layers indexed by absolute section Y.
pub type VisibleLayers = BTreeMap<i32, Arc<[u8; LIGHT_BYTES]>>;

#[derive(Clone, Default)]
pub struct VisibleLightColumn {
    block: VisibleLayers,
    sky: VisibleLayers,
}
impl VisibleLightColumn {
    /// Both maps must contain every visible layer in this column, including
    /// padding above and below the build height. Omitted layers mean that the
    /// engine has no allocated visible layer, not that a layer is unknown.
    pub fn new(block: VisibleLayers, sky: VisibleLayers) -> Self {
        Self { block, sky }
    }
}

/// Immutable engine data captured at one published revision. The caller must
/// supply complete columns and prevent concurrent publication while capturing.
#[derive(Clone)]
pub struct VisibleLightSnapshot {
    has_block: bool,
    has_sky: bool,
    columns: BTreeMap<ChunkPos, VisibleLightColumn>,
}
impl VisibleLightSnapshot {
    pub fn new(has_block: bool, has_sky: bool) -> Self {
        Self {
            has_block,
            has_sky,
            columns: BTreeMap::new(),
        }
    }
    pub fn insert_complete_column(
        &mut self,
        pos: ChunkPos,
        column: VisibleLightColumn,
    ) -> Option<VisibleLightColumn> {
        self.columns.insert(pos, column)
    }
    fn position(pos: [i32; 3]) -> Option<(ChunkPos, i32, usize, usize, usize)> {
        // BlockPos.asLong has signed 26-bit X/Z and signed 12-bit Y.
        if !(-33_554_432..=33_554_431).contains(&pos[0])
            || !(-2048..=2047).contains(&pos[1])
            || !(-33_554_432..=33_554_431).contains(&pos[2])
        {
            return None;
        }
        Some((
            ChunkPos {
                x: pos[0].div_euclid(16),
                z: pos[2].div_euclid(16),
            },
            pos[1].div_euclid(16),
            pos[0].rem_euclid(16) as usize,
            pos[1].rem_euclid(16) as usize,
            pos[2].rem_euclid(16) as usize,
        ))
    }
    fn nibble(layer: &[u8; LIGHT_BYTES], x: usize, y: usize, z: usize) -> u8 {
        let index = x + z * 16 + y * 256;
        let byte = layer[index / 2];
        if index % 2 == 0 { byte & 15 } else { byte >> 4 }
    }
    pub fn block_light(&self, pos: [i32; 3]) -> Option<u8> {
        let (column, section, x, y, z) = Self::position(pos)?;
        if !self.has_block {
            return Some(0);
        }
        let column = self.columns.get(&column)?;
        Some(
            column
                .block
                .get(&section)
                .map_or(0, |layer| Self::nibble(layer, x, y, z)),
        )
    }
    pub fn sky_light(&self, pos: [i32; 3]) -> Option<u8> {
        let (column, section, x, y, z) = Self::position(pos)?;
        if !self.has_sky {
            return Some(0);
        }
        let column = self.columns.get(&column)?;
        if let Some(layer) = column.sky.get(&section) {
            return Some(Self::nibble(layer, x, y, z));
        }
        // A missing sky layer below an allocated one repeats the bottom slice
        // of the next higher layer. Above the top, visible sky defaults to 15.
        Some(
            column
                .sky
                .range(section..)
                .next()
                .map_or(15, |(_, layer)| Self::nibble(layer, x, 0, z)),
        )
    }
    pub fn raw_brightness(&self, pos: [i32; 3], sky_darken: u8) -> Option<u8> {
        if sky_darken > 15 {
            return None;
        }
        Some(
            self.block_light(pos)?
                .max(self.sky_light(pos)?.saturating_sub(sky_darken)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn layer(n: u8) -> Arc<[u8; LIGHT_BYTES]> {
        Arc::new([n | (n << 4); LIGHT_BYTES])
    }
    #[test]
    fn absent_layers_use_engine_defaults_but_unknown_columns_do_not() {
        let mut light = VisibleLightSnapshot::new(true, true);
        let mut sky = VisibleLayers::new();
        sky.insert(1, layer(4));
        sky.insert(3, layer(9));
        let mut block = VisibleLayers::new();
        block.insert(0, layer(6));
        light.insert_complete_column(
            ChunkPos { x: -1, z: 0 },
            VisibleLightColumn::new(block, sky),
        );
        assert_eq!(light.block_light([-1, 2, 0]), Some(6));
        assert_eq!(light.block_light([-1, 32, 0]), Some(0));
        assert_eq!(light.sky_light([-1, -17, 0]), Some(4));
        assert_eq!(light.sky_light([-1, 32, 0]), Some(9));
        assert_eq!(light.sky_light([-1, 64, 0]), Some(15));
        assert_eq!(light.raw_brightness([-1, 2, 0], 3), Some(6));
        assert_eq!(light.raw_brightness([-1, 2, 0], 16), None);
        assert_eq!(light.raw_brightness([0, 2, 0], 0), None);
        assert_eq!(light.sky_light([-1, 2048, 0]), None);
    }
}
