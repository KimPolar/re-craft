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

/// Order a drained batch as Java 26.3's newly emptied pending-node set does.
/// Positions are supplied in `checkBlock` insertion order, including repeats.
/// Java uses a 512-entry expected capacity, 0.5 load factor, and trims its
/// set after every drain. Packed BlockPos values outside Java's range fail.
pub fn order_pending_light_checks(
    positions: impl IntoIterator<Item = [i32; 3]>,
) -> Option<Vec<[i32; 3]>> {
    fn pack([x, y, z]: [i32; 3]) -> Option<u64> {
        if !(-33_554_432..=33_554_431).contains(&x)
            || !(-2048..=2047).contains(&y)
            || !(-33_554_432..=33_554_431).contains(&z)
        {
            return None;
        }
        Some(((x as u64 & 0x3ff_ffff) << 38) | ((z as u64 & 0x3ff_ffff) << 12) | (y as u64 & 0xfff))
    }
    fn slot(key: u64, mask: usize) -> usize {
        let h = key.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let h = h ^ (h >> 32);
        ((h ^ (h >> 16)) as usize) & mask
    }
    fn insert(table: &mut [u64], key: u64) -> bool {
        let mask = table.len() - 1;
        let mut index = slot(key, mask);
        while table[index] != 0 {
            if table[index] == key {
                return false;
            }
            index = (index + 1) & mask;
        }
        table[index] = key;
        true
    }
    let mut positions = positions.into_iter();
    let Some(first) = positions.next() else {
        return Some(Vec::new());
    };
    let Some(second) = positions.next() else {
        pack(first)?;
        return Some(vec![first]);
    };
    let mut table = vec![0u64; 1024];
    let mut has_zero = false;
    let mut size = 0usize;
    for position in [first, second].into_iter().chain(positions) {
        let key = pack(position)?;
        let inserted = if key == 0 {
            let was_new = !has_zero;
            has_zero = true;
            was_new
        } else {
            insert(&mut table, key)
        };
        if !inserted {
            continue;
        }
        size += 1;
        if size > table.len() / 2 {
            let next = size
                .checked_add(1)?
                .checked_mul(2)?
                .checked_next_power_of_two()?;
            if next > (1usize << 30) {
                return None;
            }
            let mut grown = vec![0u64; next];
            for &old in table.iter().rev().filter(|&&old| old != 0) {
                insert(&mut grown, old);
            }
            table = grown;
        }
    }
    let mut ordered = Vec::with_capacity(size);
    if has_zero {
        ordered.push([0, 0, 0]);
    }
    for &key in table.iter().rev().filter(|&&key| key != 0) {
        ordered.push([
            ((key as i64) >> 38) as i32,
            ((key << 52) as i64 >> 52) as i32,
            ((key << 26) as i64 >> 38) as i32,
        ]);
    }
    Some(ordered)
}

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

    /// Allocated visible layers, including zero-valued layers and padding.
    pub fn block_layers(&self) -> &VisibleLayers {
        &self.block
    }

    /// Allocated visible sky layers; absent layers remain sparse.
    pub fn sky_layers(&self) -> &VisibleLayers {
        &self.sky
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

    /// Share a complete covered column, including which layers are allocated.
    pub fn complete_column(&self, pos: ChunkPos) -> Option<VisibleLightColumn> {
        self.columns.get(&pos).cloned()
    }

    /// Compare complete visible storage, including allocated zero layers,
    /// without expanding or copying section arrays.
    pub fn same_complete_column(&self, other: &Self, pos: ChunkPos) -> Option<bool> {
        if self.has_block != other.has_block || self.has_sky != other.has_sky {
            return Some(false);
        }
        let left = self.columns.get(&pos)?;
        let right = other.columns.get(&pos)?;
        let same = |a: &VisibleLayers, b: &VisibleLayers| {
            a.len() == b.len()
                && a.iter().zip(b).all(|((ay, av), (by, bv))| {
                    ay == by && (Arc::ptr_eq(av, bv) || av.as_ref() == bv.as_ref())
                })
        };
        Some(same(&left.block, &right.block) && same(&left.sky, &right.sky))
    }

    /// Apply empty-to-nonempty section events to updating storage. The caller
    /// supplies the actual event order and only previously empty sections;
    /// no removals, queued saved layers or concurrent updates may be pending.
    /// Every affected 3x3 column must be covered. The visible input revision
    /// is unchanged when an input or packed coordinate is unavailable.
    pub fn allocate_nonempty_sections(
        &self,
        sections: &[(ChunkPos, i32)],
        active: &std::collections::BTreeSet<ChunkPos>,
    ) -> Option<Self> {
        let mut draft = self.clone();
        let zero = Arc::new([0; LIGHT_BYTES]);
        let full = Arc::new([0xff; LIGHT_BYTES]);
        for &(center, sy) in sections {
            if !(-2_097_151..=2_097_150).contains(&center.x)
                || !(-2_097_151..=2_097_150).contains(&center.z)
                || !(-524_287..=524_286).contains(&sy)
            {
                return None;
            }
            let mut allocate = |pos: ChunkPos, y: i32| -> Option<()> {
                let column = draft.columns.get_mut(&pos)?;
                if draft.has_block {
                    column.block.entry(y).or_insert_with(|| Arc::clone(&zero));
                }
                if draft.has_sky && !column.sky.contains_key(&y) {
                    let layer = if let Some((_, above)) = column.sky.range((y + 1)..).next() {
                        let mut layer = [0u8; LIGHT_BYTES];
                        for slice in layer.chunks_exact_mut(LIGHT_BYTES / 16) {
                            slice.copy_from_slice(&above[..LIGHT_BYTES / 16]);
                        }
                        Arc::new(layer)
                    } else if active.contains(&pos) {
                        Arc::clone(&full)
                    } else {
                        Arc::clone(&zero)
                    };
                    column.sky.insert(y, layer);
                }
                Some(())
            };
            // Java inserts the data section first, then visits x, y, z
            // offsets in ascending order, skipping the center.
            allocate(center, sy)?;
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        if dx == 0 && dy == 0 && dz == 0 {
                            continue;
                        }
                        allocate(
                            ChunkPos {
                                x: center.x + dx,
                                z: center.z + dz,
                            },
                            sy + dy,
                        )?;
                    }
                }
            }
        }
        Some(draft)
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

    pub fn has_block_channel(&self) -> bool {
        self.has_block
    }

    pub(crate) fn stored_block(&self, pos: [i32; 3]) -> Option<Option<u8>> {
        let (column, section, x, y, z) = Self::position(pos)?;
        if !self.has_block {
            return None;
        }
        Some(
            self.columns
                .get(&column)?
                .block
                .get(&section)
                .map(|layer| Self::nibble(layer, x, y, z)),
        )
    }

    pub(crate) fn set_stored_block(&mut self, pos: [i32; 3], value: u8) -> Option<()> {
        if value > 15 || !self.has_block {
            return None;
        }
        let (column, section, x, y, z) = Self::position(pos)?;
        let layer = Arc::make_mut(self.columns.get_mut(&column)?.block.get_mut(&section)?);
        let index = x + z * 16 + y * 256;
        let byte = &mut layer[index / 2];
        if index % 2 == 0 {
            *byte = (*byte & 0xf0) | value;
        } else {
            *byte = (*byte & 0x0f) | (value << 4);
        }
        Some(())
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
    fn adding_sections_inherits_sky_and_preserves_visible_revision() {
        let center = ChunkPos { x: 0, z: 0 };
        let active_neighbor = ChunkPos { x: 1, z: 0 };
        let mut before = VisibleLightSnapshot::new(true, true);
        for z in -1..=1 {
            for x in -1..=1 {
                before.insert_complete_column(ChunkPos { x, z }, VisibleLightColumn::default());
            }
        }
        let mut upper = [0u8; LIGHT_BYTES];
        for (i, byte) in upper[..LIGHT_BYTES / 16].iter_mut().enumerate() {
            *byte = i as u8;
        }
        before
            .columns
            .get_mut(&center)
            .unwrap()
            .sky
            .insert(2, Arc::new(upper));
        let after = before
            .allocate_nonempty_sections(
                &[(center, 0)],
                &std::collections::BTreeSet::from([active_neighbor]),
            )
            .unwrap();
        assert_eq!(before.block_section_allocated(center, 0), Some(false));
        assert_eq!(after.block_section_allocated(center, 0), Some(true));
        for y in -16..32 {
            for z in 0..16 {
                for x in 0..16 {
                    assert_eq!(after.sky_light([x, y, z]), before.sky_light([x, 0, z]));
                }
            }
        }
        assert_eq!(after.sky_light([16, 0, 0]), Some(15));
        assert_eq!(after.sky_light([-1, 0, 0]), Some(0));
        let incomplete = VisibleLightSnapshot::new(true, true);
        assert!(
            incomplete
                .allocate_nonempty_sections(&[(center, 0)], &Default::default())
                .is_none()
        );
        assert_eq!(before.block_section_allocated(center, 0), Some(false));
    }
    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn section_allocation_inherits_java_nonuniform_sky() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft")
            });
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("SectionAllocationOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/SectionAllocationOracle.java"),
        )
        .unwrap();
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        for mode in [
            "inactive",
            "active",
            "inactive_many",
            "active_many",
            "inactive_many_reverse",
            "active_many_reverse",
        ] {
            let path = temp.path().join("sections.json");
            let run = Command::new(java.join("java.exe"))
                .arg("-classpath")
                .arg(&cp)
                .arg("SectionAllocationOracle")
                .arg(&path)
                .arg(mode)
                .output()
                .unwrap();
            assert!(
                run.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&run.stdout),
                String::from_utf8_lossy(&run.stderr)
            );
            let expected: Vec<Vec<Vec<Vec<u8>>>> =
                serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            let center = ChunkPos { x: 0, z: 0 };
            let covered: Vec<_> = (-1..=1)
                .flat_map(|z| (-1..=2).map(move |x| ChunkPos { x, z }))
                .collect();
            let mut before = VisibleLightSnapshot::fresh_after_section_updates(
                true,
                true,
                [(center, 3)],
                covered.iter().copied(),
            )
            .unwrap();
            for y in 0..16 {
                for z in 0..16 {
                    for x in 0..16 {
                        before
                            .set_stored_sky([x, y + 32, z], ((x * 3 + z * 5 + y * 7) & 15) as u8)
                            .unwrap();
                    }
                }
            }
            let active = if mode.starts_with("active") {
                std::collections::BTreeSet::from([ChunkPos { x: 2, z: 0 }])
            } else {
                Default::default()
            };
            let mut sections = vec![(ChunkPos { x: 1, z: 0 }, 0)];
            if mode.contains("many") {
                sections.push((ChunkPos { x: 1, z: 0 }, 1));
                if mode.ends_with("reverse") {
                    sections.reverse();
                }
            }
            let after = before
                .allocate_nonempty_sections(&sections, &active)
                .unwrap();
            assert_eq!(
                after.sky_light([32, 0, 0]),
                Some(if mode.starts_with("active") { 15 } else { 0 })
            );
            for (index, pos) in covered.iter().enumerate() {
                for y in -16..80 {
                    for z in 0..16 {
                        for x in 0..16 {
                            let p = [pos.x * 16 + x, y, pos.z * 16 + z];
                            let i = ((y + 16) * 256 + z * 16 + x) as usize;
                            assert_eq!(
                                before.block_light(p),
                                Some(expected[0][index][0][i]),
                                "{mode} before block {p:?}"
                            );
                            assert_eq!(
                                before.sky_light(p),
                                Some(expected[0][index][1][i]),
                                "{mode} before sky {p:?}"
                            );
                            assert_eq!(
                                before.sky_light(p),
                                Some(expected[1][index][1][i]),
                                "{mode} pending sky {p:?}"
                            );
                            assert_eq!(
                                after.block_light(p),
                                Some(expected[2][index][0][i]),
                                "{mode} after block {p:?}"
                            );
                            assert_eq!(
                                after.sky_light(p),
                                Some(expected[2][index][1][i]),
                                "{mode} after sky {p:?}"
                            );
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn pending_checks_deduplicate_and_validate_packed_coordinates() {
        let positions = [[3, -1, 2], [0, 0, 0], [3, -1, 2], [-2, 7, 9]];
        let order = order_pending_light_checks(positions).unwrap();
        assert_eq!(order.len(), 3);
        assert_eq!(order[0], [0, 0, 0]);
        assert_eq!(
            order.into_iter().collect::<std::collections::BTreeSet<_>>(),
            positions.into_iter().collect()
        );
        assert!(order_pending_light_checks([[33_554_432, 0, 0]]).is_none());
        assert!(order_pending_light_checks([[0, -2049, 0]]).is_none());
        assert!(order_pending_light_checks([[0, 0, -33_554_433]]).is_none());
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn pending_check_order_matches_java_fastutil() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft")
            });
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("PendingLightChecksOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/PendingLightChecksOracle.java"),
        )
        .unwrap();
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let mut batches = vec![
            vec![],
            vec![[0, 0, 0], [0, 0, 0], [3, -1, 2]],
            vec![
                [-33_554_432, -2048, -33_554_432],
                [33_554_431, 2047, 33_554_431],
            ],
        ];
        let mut random = 0x8213_a567_9f84_1234u64;
        for count in [511, 512, 513, 1200, 17, 2000] {
            let mut batch = vec![[0, 0, 0]];
            for i in 0..count {
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                let p = [
                    ((random & 0xffff) as i32) - 32768,
                    (((random >> 16) & 0xfff) as i32) - 2048,
                    (((random >> 28) & 0xffff) as i32) - 32768,
                ];
                batch.push(p);
                if i % 9 == 0 {
                    batch.push(p);
                }
            }
            batches.push(batch);
        }
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(&input, serde_json::to_vec(&batches).unwrap()).unwrap();
        let run = Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("PendingLightChecksOracle")
            .arg(&input)
            .arg(&output)
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        );
        let expected: Vec<Vec<[i32; 3]>> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(expected.len(), batches.len());
        for (i, (batch, expected)) in batches.into_iter().zip(expected).enumerate() {
            assert_eq!(
                order_pending_light_checks(batch).unwrap(),
                expected,
                "batch {i}"
            );
        }
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
