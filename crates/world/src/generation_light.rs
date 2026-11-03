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

    // Unlike visible reads, propagation must distinguish absent storage from
    // an allocated zero layer. The outer Option denotes a covered column.
    pub(crate) fn stored_sky(&self, pos: [i32; 3]) -> Option<Option<u8>> {
        let (column, section, x, y, z) = Self::position(pos)?;
        if !self.has_sky {
            return None;
        }
        Some(
            self.columns
                .get(&column)?
                .sky
                .get(&section)
                .map(|layer| Self::nibble(layer, x, y, z)),
        )
    }

    pub(crate) fn set_stored_sky(&mut self, pos: [i32; 3], value: u8) -> Option<()> {
        if value > 15 || !self.has_sky {
            return None;
        }
        let (column, section, x, y, z) = Self::position(pos)?;
        let layer = Arc::make_mut(self.columns.get_mut(&column)?.sky.get_mut(&section)?);
        let index = x + z * 16 + y * 256;
        let byte = &mut layer[index / 2];
        if index % 2 == 0 {
            *byte = (*byte & 0xf0) | value;
        } else {
            *byte = (*byte & 0x0f) | (value << 4);
        }
        Some(())
    }

    /// Expand one covered section into packed arrays for a consumer that
    /// cannot read sparse engine storage. Missing sky layers inherit the
    /// bottom slice of the next higher layer, or full sky above all layers.
    /// Missing channels are zero, and an uncovered column remains unknown.
    pub fn expanded_section(
        &self,
        pos: ChunkPos,
        section_y: i32,
    ) -> Option<(Box<[u8; LIGHT_BYTES]>, Box<[u8; LIGHT_BYTES]>)> {
        let column = self.columns.get(&pos)?;
        let block = if self.has_block {
            column
                .block
                .get(&section_y)
                .map_or_else(|| Box::new([0; LIGHT_BYTES]), |v| Box::new(**v))
        } else {
            Box::new([0; LIGHT_BYTES])
        };
        let sky = if !self.has_sky {
            Box::new([0; LIGHT_BYTES])
        } else if let Some(layer) = column.sky.get(&section_y) {
            Box::new(**layer)
        } else if let Some((_, layer)) = column.sky.range(section_y..).next() {
            let mut expanded = Box::new([0; LIGHT_BYTES]);
            for slice in expanded.chunks_exact_mut(LIGHT_BYTES / 16) {
                slice.copy_from_slice(&layer[..LIGHT_BYTES / 16]);
            }
            expanded
        } else {
            Box::new([0xff; LIGHT_BYTES])
        };
        Some((block, sky))
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
    fn stored_sky_edits_preserve_previous_snapshots_and_absent_layers() {
        let pos = ChunkPos { x: -1, z: 0 };
        let mut before = VisibleLightSnapshot::new(false, true);
        before.insert_complete_column(
            pos,
            VisibleLightColumn::new(BTreeMap::new(), BTreeMap::from([(0, layer(7))])),
        );
        let mut after = before.clone();
        assert_eq!(before.stored_sky([-1, 2, 0]), Some(Some(7)));
        assert_eq!(before.stored_sky([-1, 16, 0]), Some(None));
        assert_eq!(before.stored_sky([0, 2, 0]), None);
        assert_eq!(after.set_stored_sky([-1, 2, 0], 3), Some(()));
        assert_eq!(after.set_stored_sky([-2, 2, 0], 12), Some(()));
        assert_eq!(after.set_stored_sky([-1, 16, 0], 2), None);
        assert_eq!(after.set_stored_sky([-1, 2, 0], 16), None);
        assert_eq!(before.sky_light([-1, 2, 0]), Some(7));
        assert_eq!(before.sky_light([-2, 2, 0]), Some(7));
        assert_eq!(after.sky_light([-1, 2, 0]), Some(3));
        assert_eq!(after.sky_light([-2, 2, 0]), Some(12));
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
    fn expanded_sections_match_sparse_queries_and_disabled_channels() {
        let center = ChunkPos { x: -1, z: 2 };
        let mut sky_layer = [0u8; LIGHT_BYTES];
        for (i, byte) in sky_layer.iter_mut().enumerate() {
            *byte = (i % 256) as u8;
        }
        for (has_block, has_sky) in [(true, true), (true, false), (false, true), (false, false)] {
            let mut light = VisibleLightSnapshot::new(has_block, has_sky);
            light.insert_complete_column(
                center,
                VisibleLightColumn::new(
                    BTreeMap::from([(0, layer(6))]),
                    BTreeMap::from([(2, Arc::new(sky_layer))]),
                ),
            );
            assert!(light.expanded_section(ChunkPos { x: 0, z: 0 }, 0).is_none());
            for sy in [-2, 0, 1, 2, 3] {
                let (block, sky) = light.expanded_section(center, sy).unwrap();
                for y in 0..16 {
                    for z in 0..16 {
                        for x in 0..16 {
                            let p = [-16 + x as i32, sy * 16 + y as i32, 32 + z as i32];
                            assert_eq!(
                                VisibleLightSnapshot::nibble(&block, x, y, z),
                                light.block_light(p).unwrap()
                            );
                            assert_eq!(
                                VisibleLightSnapshot::nibble(&sky, x, y, z),
                                light.sky_light(p).unwrap()
                            );
                        }
                    }
                }
            }
        }
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
