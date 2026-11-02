//! A consistent view of the light engine's visible section storage.
//!
//! FEATURES runs before INITIALIZE_LIGHT for the target chunk. Java therefore
//! reads the light engine's published data, which can differ from a fresh
//! calculation of the generated terrain. Missing *allocated* sections and
//! columns not included in a snapshot must not be conflated.
use crate::loaded::{BlockState, ChunkPos, LIGHT_BYTES, World};
use std::{
    collections::{BTreeMap, HashMap},
    ops::Range,
    sync::Arc,
};

/// Nibble-packed visible layers indexed by absolute section Y.
pub type VisibleLayers = BTreeMap<i32, Arc<[u8; LIGHT_BYTES]>>;

/// Determine which sections are nonempty in this immutable world snapshot.
pub fn initialized_nonempty_sections(
    world: &World,
    section_range: Range<i32>,
    registered_chunks: impl IntoIterator<Item = ChunkPos>,
    mut is_air: impl FnMut(&BlockState) -> Option<bool>,
) -> Option<Vec<(ChunkPos, i32)>> {
    if section_range.start >= section_range.end {
        return None;
    }
    let mut known = HashMap::new();
    let mut statuses = Vec::new();
    for pos in registered_chunks {
        let chunk = world.chunks.get(&pos)?;
        if chunk.position != pos {
            return None;
        }
        for sy in section_range.clone() {
            let blocks = chunk
                .sections
                .get(&i8::try_from(sy).ok()?)?
                .blocks
                .as_ref()?;
            let mut nonempty = false;
            for id in blocks.iter() {
                let air = if let Some(&air) = known.get(&id.0) {
                    air
                } else {
                    let air = is_air(world.state(*id)?)?;
                    known.insert(id.0, air);
                    air
                };
                if !air {
                    nonempty = true;
                    break;
                }
            }
            if nonempty {
                statuses.push((pos, sy));
            }
        }
    }
    Some(statuses)
}

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

    /// Whether this visible engine revision has storage for a block-light
    /// section. An uncovered column remains unknown rather than absent.
    pub fn block_section_allocated(&self, pos: ChunkPos, section_y: i32) -> Option<bool> {
        if !self.has_block {
            return Some(false);
        }
        Some(self.columns.get(&pos)?.block.contains_key(&section_y))
    }

    /// Whether the visible engine revision has storage for a sky-light section.
    pub fn sky_section_allocated(&self, pos: ChunkPos, section_y: i32) -> Option<bool> {
        if !self.has_sky {
            return Some(false);
        }
        Some(self.columns.get(&pos)?.sky.contains_key(&section_y))
    }

    pub fn has_sky_channel(&self) -> bool {
        self.has_sky
    }

    /// Capture a newly created engine after the listed nonempty section status
    /// updates have been drained. No lighting may have been enabled, queued,
    /// propagated, removed or restored before this snapshot. The caller must
    /// provide every nonempty status registered in this engine revision.
    pub fn fresh_after_section_updates(
        has_block: bool,
        has_sky: bool,
        nonempty_sections: impl IntoIterator<Item = (ChunkPos, i32)>,
        covered_columns: impl IntoIterator<Item = ChunkPos>,
    ) -> Option<Self> {
        let mut result = Self::new(has_block, has_sky);
        for pos in covered_columns {
            if !(-2_097_152..=2_097_151).contains(&pos.x)
                || !(-2_097_152..=2_097_151).contains(&pos.z)
            {
                return None;
            }
            result.columns.entry(pos).or_default();
        }
        let zero = Arc::new([0; LIGHT_BYTES]);
        for (pos, y) in nonempty_sections {
            if !(-2_097_151..=2_097_150).contains(&pos.x)
                || !(-2_097_151..=2_097_150).contains(&pos.z)
                || !(-524_287..=524_286).contains(&y)
            {
                return None;
            }
            for dz in -1..=1 {
                for dx in -1..=1 {
                    if let Some(column) = result.columns.get_mut(&ChunkPos {
                        x: pos.x + dx,
                        z: pos.z + dz,
                    }) {
                        for sy in y - 1..=y + 1 {
                            if has_block {
                                column.block.entry(sy).or_insert_with(|| Arc::clone(&zero));
                            }
                            if has_sky {
                                column.sky.entry(sy).or_insert_with(|| Arc::clone(&zero));
                            }
                        }
                    }
                }
            }
        }
        Some(result)
    }

    /// Read the nonempty sections registered by INITIALIZE_LIGHT from the
    /// specified immutable chunk revision. The caller must supply all chunks
    /// initialized in the fresh engine, and ensure they have not changed since
    /// their section status was registered. No light may have propagated or
    /// been restored, enabled, removed, or queued in that engine.
    pub fn fresh_after_chunk_initialization(
        world: &World,
        section_range: Range<i32>,
        registered_chunks: impl IntoIterator<Item = ChunkPos>,
        covered_columns: impl IntoIterator<Item = ChunkPos>,
        has_block: bool,
        has_sky: bool,
        is_air: impl FnMut(&BlockState) -> Option<bool>,
    ) -> Option<Self> {
        let statuses =
            initialized_nonempty_sections(world, section_range, registered_chunks, is_air)?;
        Self::fresh_after_section_updates(has_block, has_sky, statuses, covered_columns)
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

    #[test]
    fn fresh_engine_registers_neighbor_sections_and_shares_empty_layers() {
        let center = ChunkPos { x: 0, z: 0 };
        let neighbor = ChunkPos { x: -1, z: 1 };
        let fresh =
            VisibleLightSnapshot::fresh_after_section_updates(true, true, [], [center]).unwrap();
        assert_eq!(fresh.raw_brightness([0, 0, 0], 0), Some(15));
        let light = VisibleLightSnapshot::fresh_after_section_updates(
            true,
            true,
            [(center, 0)],
            [center, neighbor],
        )
        .unwrap();
        assert_eq!(light.sky_light([0, -64, 0]), Some(0));
        assert_eq!(light.sky_light([0, 31, 0]), Some(0));
        assert_eq!(light.sky_light([0, 32, 0]), Some(15));
        assert_eq!(light.block_light([-1, 5, 16]), Some(0));
        assert_eq!(light.sky_light([16, 5, 16]), None);
        assert!(Arc::ptr_eq(
            &light.columns[&center].block[&0],
            &light.columns[&neighbor].sky[&1]
        ));
        assert!(
            VisibleLightSnapshot::fresh_after_section_updates(
                true,
                true,
                [(ChunkPos { x: i32::MAX, z: 0 }, 0)],
                [center]
            )
            .is_none()
        );
    }
}
