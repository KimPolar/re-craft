//! A consistent view of the light engine's visible section storage.
//!
//! FEATURES runs before INITIALIZE_LIGHT for the target chunk. Java therefore
//! reads the light engine's published data, which can differ from a fresh
//! calculation of the generated terrain. Missing *allocated* sections and
//! columns not included in a snapshot must not be conflated.
use crate::{
    block_light::LightBounds,
    loaded::{BlockState, ChunkPos, LIGHT_BYTES, World},
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
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

/// Section allocation state for one fully observed light-engine revision.
///
/// The tracked data-section set and enabled columns must describe the same
/// engine as the visible snapshot. Queued saved layers, pending light checks,
/// concurrent chunk loading and unpublished status changes require separate
/// reconciliation before this tracker can be used.
#[derive(Clone)]
pub struct SectionLightTracker {
    visible: VisibleLightSnapshot,
    data_sections: BTreeSet<(ChunkPos, i32)>,
    references: BTreeMap<(ChunkPos, i32), u8>,
    active: BTreeSet<ChunkPos>,
    known_chunks: Option<BTreeSet<ChunkPos>>,
}

/// Inputs for restoring one previously published light-engine revision.
///
/// `registration` must describe whether each chunk's section statuses were
/// registered in that same engine. Every covered column needs a complete
/// one-chunk registration halo; absent entries are unknown, not unregistered.
/// Only registrations in that halo are included in the restored tracker.
/// The caller must also establish that the saved arrays and block sections
/// come from one drained revision, without queued layers or later live edits.
pub struct SavedSectionLightInputs<'a> {
    pub world: &'a World,
    pub bounds: LightBounds,
    pub covered: &'a BTreeSet<ChunkPos>,
    pub registration: &'a BTreeMap<ChunkPos, bool>,
    pub active: BTreeSet<ChunkPos>,
    pub has_block: bool,
    pub has_sky: bool,
}

fn section_position_supported(center: ChunkPos, y: i32) -> bool {
    (-2_097_151..=2_097_150).contains(&center.x)
        && (-2_097_151..=2_097_150).contains(&center.z)
        && (-524_287..=524_286).contains(&y)
}

fn section_neighborhood(center: ChunkPos, y: i32) -> Option<Vec<(ChunkPos, i32)>> {
    if !section_position_supported(center, y) {
        return None;
    }
    let mut result = Vec::with_capacity(27);
    result.push((center, y));
    for dx in -1..=1 {
        for dy in -1..=1 {
            for dz in -1..=1 {
                if dx == 0 && dy == 0 && dz == 0 {
                    continue;
                }
                result.push((
                    ChunkPos {
                        x: center.x + dx,
                        z: center.z + dz,
                    },
                    y + dy,
                ));
            }
        }
    }
    Some(result)
}

impl SectionLightTracker {
    /// Restore saved layers using a separately verified registration history.
    /// Java omits both unallocated layers and allocated homogeneous-zero
    /// layers, so the complete registration halo determines which missing
    /// layers must be restored as zero. Contradictory or incomplete inputs
    /// are rejected without changing the world. The snapshot preserves values
    /// and allocation, not whether Java materialized a zero-valued DataLayer;
    /// it cannot by itself prove the next serialized omission or reload order.
    pub fn from_saved(
        input: SavedSectionLightInputs<'_>,
        is_air: impl FnMut(&BlockState) -> Option<bool>,
    ) -> Option<Self> {
        if input.covered.is_empty() {
            return None;
        }
        let mut relevant = BTreeSet::new();
        for &pos in input.covered {
            if !section_position_supported(pos, 0) {
                return None;
            }
            for dx in -1..=1 {
                for dz in -1..=1 {
                    let neighbor = ChunkPos {
                        x: pos.x.checked_add(dx)?,
                        z: pos.z.checked_add(dz)?,
                    };
                    input.registration.get(&neighbor)?;
                    relevant.insert(neighbor);
                }
            }
        }
        if input
            .active
            .iter()
            .any(|pos| input.registration.get(pos) != Some(&true))
        {
            return None;
        }
        let build = input.bounds.build_range();
        let light = input.bounds.light_range();
        let registered = relevant
            .iter()
            .copied()
            .filter(|pos| input.registration.get(pos) == Some(&true))
            .collect::<Vec<_>>();
        for &pos in &registered {
            let chunk = input.world.chunks.get(&pos)?;
            if !section_position_supported(pos, 0)
                || chunk.position != pos
                || chunk.status.as_deref() != Some("minecraft:full")
                || !chunk.block_light_correct
            {
                return None;
            }
        }
        let data_sections = initialized_nonempty_sections(
            input.world,
            build.start.div_euclid(16)..build.end.div_euclid(16),
            registered,
            is_air,
        )?;
        let mut visible = VisibleLightSnapshot::fresh_after_section_updates(
            input.has_block,
            input.has_sky,
            data_sections.iter().copied(),
            input.covered.iter().copied(),
        )?;
        let light_sections = light.start.div_euclid(16)..light.end.div_euclid(16);
        for &pos in input.covered {
            let chunk = input.world.chunks.get(&pos)?;
            if chunk.position != pos
                || chunk.status.as_deref() != Some("minecraft:full")
                || !chunk.block_light_correct
            {
                return None;
            }
            let column = visible.columns.get_mut(&pos)?;
            for (&y, section) in &chunk.sections {
                let y = i32::from(y);
                for (saved, enabled, layers) in [
                    (&section.block_light, input.has_block, &mut column.block),
                    (&section.sky_light, input.has_sky, &mut column.sky),
                ] {
                    let Some(saved) = saved else { continue };
                    if !enabled || !light_sections.contains(&y) || !layers.contains_key(&y) {
                        return None;
                    }
                    if saved.iter().any(|&byte| byte != 0) {
                        layers.insert(y, Arc::new(**saved));
                    }
                }
            }
        }
        let mut tracker = Self::new(visible, data_sections, input.active)?;
        tracker.known_chunks = Some(relevant);
        Some(tracker)
    }

    /// Attach section-status history to an already published snapshot. The
    /// caller must provide every registered nonempty section, including those
    /// outside covered columns whose allocation halo reaches the snapshot.
    /// Existing layer sets are checked against that complete history.
    pub fn new(
        visible: VisibleLightSnapshot,
        data_sections: impl IntoIterator<Item = (ChunkPos, i32)>,
        active: BTreeSet<ChunkPos>,
    ) -> Option<Self> {
        let mut registered = BTreeSet::new();
        let mut references = BTreeMap::<(ChunkPos, i32), u8>::new();
        // Only covered columns can be updated through this tracker. Keep all
        // data-section identities, but count halos only where they matter.
        let mut relevant = BTreeSet::new();
        for &position in visible.columns.keys() {
            for dx in -1i32..=1 {
                for dz in -1i32..=1 {
                    let (Some(x), Some(z)) =
                        (position.x.checked_add(dx), position.z.checked_add(dz))
                    else {
                        continue;
                    };
                    relevant.insert(ChunkPos { x, z });
                }
            }
        }
        for (position, y) in data_sections {
            if !section_position_supported(position, y) || !registered.insert((position, y)) {
                return None;
            }
            if !relevant.contains(&position) {
                continue;
            }
            for section in section_neighborhood(position, y)? {
                if !visible.columns.contains_key(&section.0) {
                    continue;
                }
                let count = references.entry(section).or_default();
                *count = count.checked_add(1)?;
            }
        }
        for (&(position, y), _) in &references {
            let Some(column) = visible.columns.get(&position) else {
                continue;
            };
            if (visible.has_block && !column.block.contains_key(&y))
                || (visible.has_sky && !column.sky.contains_key(&y))
            {
                return None;
            }
        }
        for (&position, column) in &visible.columns {
            if (!visible.has_block && !column.block.is_empty())
                || (!visible.has_sky && !column.sky.is_empty())
                || column
                    .block
                    .keys()
                    .chain(column.sky.keys())
                    .any(|&y| !references.contains_key(&(position, y)))
            {
                return None;
            }
        }
        Some(Self {
            visible,
            data_sections: registered,
            references,
            active,
            known_chunks: None,
        })
    }

    pub fn snapshot(&self) -> &VisibleLightSnapshot {
        &self.visible
    }

    /// Whether a section is registered in this tracker. Outside the restored
    /// registration halo, `false` does not prove that a section is empty.
    pub fn is_data_section(&self, position: ChunkPos, y: i32) -> bool {
        self.data_sections.contains(&(position, y))
    }

    /// Return None when the tracker's registration coverage does not include
    /// this chunk, instead of treating an unobserved section as empty.
    pub fn data_section_status(&self, position: ChunkPos, y: i32) -> Option<bool> {
        if self
            .known_chunks
            .as_ref()
            .is_some_and(|known| !known.contains(&position))
        {
            return None;
        }
        Some(self.is_data_section(position, y))
    }

    /// Replay status callbacks in order, then publish one drained revision.
    /// Each tuple is (chunk, section Y, is_nonempty). All affected columns
    /// must be covered. Repeated statuses are no-ops, and removing then
    /// re-adding before the drain retains the original arrays, as in Java.
    /// Failure leaves this tracker and previous snapshots unchanged.
    pub fn apply_statuses(&mut self, changes: &[(ChunkPos, i32, bool)]) -> Option<()> {
        let mut draft = self.clone();
        draft.apply_statuses_inner(changes)?;
        *self = draft;
        Some(())
    }

    fn apply_statuses_inner(&mut self, changes: &[(ChunkPos, i32, bool)]) -> Option<()> {
        let mut removed = BTreeSet::new();
        for &(position, y, nonempty) in changes {
            let neighborhood = section_neighborhood(position, y)?;
            if self.data_section_status(position, y)? == nonempty {
                continue;
            }
            if neighborhood
                .iter()
                .any(|(position, _)| !self.visible.columns.contains_key(position))
            {
                return None;
            }
            if nonempty {
                self.visible = self
                    .visible
                    .allocate_nonempty_sections(&[(position, y)], &self.active)?;
                self.data_sections.insert((position, y));
                for section in neighborhood {
                    let count = self.references.entry(section).or_default();
                    *count = count.checked_add(1)?;
                    removed.remove(&section);
                }
            } else {
                self.data_sections.remove(&(position, y));
                for section in neighborhood {
                    let count = self.references.get_mut(&section)?;
                    *count = count.checked_sub(1)?;
                    if *count == 0 {
                        self.references.remove(&section);
                        removed.insert(section);
                    }
                }
            }
        }
        for (position, y) in removed {
            let column = self.visible.columns.get_mut(&position)?;
            column.block.remove(&y);
            column.sky.remove(&y);
        }
        Some(())
    }

    /// Drain ordered section callbacks and block checks as one transaction.
    /// Section storage is updated first, matching the Java light engine.
    pub fn apply_changes(
        &mut self,
        world: &World,
        bounds: LightBounds,
        statuses: &[(ChunkPos, i32, bool)],
        positions: &[[i32; 3]],
        sky_sources: &BTreeMap<ChunkPos, Arc<[i32; 256]>>,
    ) -> Option<()> {
        let build = bounds.build_range();
        let sections = build.start.div_euclid(16)..build.end.div_euclid(16);
        if statuses.iter().any(|(_, y, _)| !sections.contains(y)) {
            return None;
        }
        let mut draft = self.clone();
        draft.apply_statuses_inner(statuses)?;
        draft.apply_checks(world, bounds, positions, sky_sources)?;
        *self = draft;
        Some(())
    }

    /// Drain block checks against the registered, already-updated section
    /// storage. The caller supplies the final block revision and exact sky
    /// source heights after those writes, with no other queued light work.
    /// Both channels commit together; incomplete inputs leave the tracker
    /// and all earlier snapshots unchanged.
    pub fn apply_checks(
        &mut self,
        world: &World,
        bounds: LightBounds,
        positions: &[[i32; 3]],
        sky_sources: &BTreeMap<ChunkPos, Arc<[i32; 256]>>,
    ) -> Option<()> {
        if positions.is_empty() {
            return Some(());
        }
        let mut next = if self.visible.has_block {
            Some(crate::generation_block_light::check_generated_block_light(
                world,
                bounds,
                &self.visible,
                &self.active,
                positions,
            )?)
        } else {
            None
        };
        if self.visible.has_sky {
            next = Some(crate::generation_sky_light::check_generated_sky_blocks(
                world,
                bounds,
                next.as_ref().unwrap_or(&self.visible),
                &self.active,
                positions,
                sky_sources,
            )?);
        }
        if let Some(visible) = next {
            self.visible = visible;
        }
        Some(())
    }
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
    use crate::loaded::{BLOCK_COUNT, Chunk, Section};

    fn layer(n: u8) -> Arc<[u8; LIGHT_BYTES]> {
        Arc::new([n | (n << 4); LIGHT_BYTES])
    }

    fn saved_fixture(height: i8) -> (World, BTreeSet<ChunkPos>, BTreeMap<ChunkPos, bool>) {
        let mut world = World::new("minecraft:overworld");
        let state = |name: &str| BlockState {
            name: format!("minecraft:{name}"),
            properties: BTreeMap::new(),
        };
        let air = world.intern_state(state("air"));
        let stone = world.intern_state(state("stone"));
        let mut registration = BTreeMap::new();
        for z in -2..=2 {
            for x in -2..=3 {
                let position = ChunkPos { x, z };
                let mut sections = BTreeMap::new();
                for y in 0..height {
                    let mut blocks = Box::new([air; BLOCK_COUNT]);
                    if z == 0 && ((y == 0 && (x == 0 || x == 1)) || (y == 8 && x == 0)) {
                        blocks[0] = stone;
                    }
                    sections.insert(
                        y,
                        Arc::new(Section {
                            blocks: Some(blocks),
                            biomes: None,
                            block_light: None,
                            sky_light: None,
                        }),
                    );
                }
                registration.insert(position, true);
                world.chunks.insert(
                    position,
                    Arc::new(Chunk {
                        position,
                        status: Some("minecraft:full".into()),
                        block_light_correct: true,
                        sections,
                    }),
                );
            }
        }
        let covered = (-1..=1)
            .flat_map(|z| (-1..=2).map(move |x| ChunkPos { x, z }))
            .collect();
        (world, covered, registration)
    }

    #[test]
    fn restored_saved_sections_keep_zero_layers_and_reject_incomplete_inputs() {
        let (mut world, covered, registration) = saved_fixture(2);
        let center = ChunkPos { x: 0, z: 0 };
        let east = ChunkPos { x: 1, z: 0 };
        let section = Arc::make_mut(
            Arc::make_mut(world.chunks.get_mut(&center).unwrap())
                .sections
                .get_mut(&0)
                .unwrap(),
        );
        section.block_light = Some(Box::new([0x21; LIGHT_BYTES]));
        section.sky_light = Some(Box::new([0xff; LIGHT_BYTES]));
        Arc::make_mut(
            Arc::make_mut(world.chunks.get_mut(&east).unwrap())
                .sections
                .get_mut(&0)
                .unwrap(),
        )
        .block_light = Some(Box::new([0; LIGHT_BYTES]));
        let bounds = LightBounds::new(0, 32).unwrap();
        let restore = |world: &World, registration: &BTreeMap<ChunkPos, bool>| {
            SectionLightTracker::from_saved(
                SavedSectionLightInputs {
                    world,
                    bounds,
                    covered: &covered,
                    registration,
                    active: BTreeSet::from([center]),
                    has_block: true,
                    has_sky: true,
                },
                |state| match state.name.as_str() {
                    "minecraft:air" => Some(true),
                    "minecraft:stone" => Some(false),
                    _ => None,
                },
            )
        };
        let before = world.chunks.clone();
        let mut tracker = restore(&world, &registration).unwrap();
        assert!(tracker.is_data_section(center, 0));
        assert!(tracker.is_data_section(east, 0));
        assert_eq!(tracker.snapshot().block_light([0, 0, 0]), Some(1));
        assert_eq!(tracker.snapshot().sky_light([0, 0, 0]), Some(15));
        assert_eq!(tracker.snapshot().block_light([16, 0, 0]), Some(0));
        assert_eq!(
            tracker.snapshot().block_section_allocated(east, 0),
            Some(true)
        );
        assert_eq!(
            tracker.snapshot().sky_section_allocated(east, 0),
            Some(true)
        );
        tracker.apply_statuses(&[(center, 0, false)]).unwrap();
        assert_eq!(
            tracker
                .snapshot()
                .block_section_allocated(ChunkPos { x: -1, z: 0 }, 0),
            Some(false)
        );
        assert_eq!(
            tracker.snapshot().sky_section_allocated(center, 0),
            Some(true)
        );
        assert!(
            before
                .iter()
                .all(|(pos, chunk)| Arc::ptr_eq(chunk, &world.chunks[pos]))
        );

        let mut incomplete = registration.clone();
        incomplete.remove(&ChunkPos { x: -2, z: -2 });
        assert!(restore(&world, &incomplete).is_none());
        let mut missing = world.clone();
        missing.chunks.remove(&ChunkPos { x: 3, z: 2 });
        assert!(restore(&missing, &registration).is_none());
        let absent = ChunkPos { x: -2, z: -2 };
        let mut absent_registration = registration.clone();
        absent_registration.insert(absent, false);
        let mut absent_world = world.clone();
        absent_world.chunks.remove(&absent);
        assert!(restore(&absent_world, &absent_registration).is_some());
        let mut distant = registration.clone();
        let unknown = ChunkPos { x: 500, z: 500 };
        distant.insert(unknown, true);
        let mut restored = restore(&world, &distant).unwrap();
        assert_eq!(restored.data_section_status(unknown, 0), None);
        assert!(restored.apply_statuses(&[(unknown, 0, false)]).is_none());
        let mut contradictory = world.clone();
        let chunk = Arc::make_mut(contradictory.chunks.get_mut(&center).unwrap());
        chunk.sections.insert(
            2,
            Arc::new(Section {
                blocks: None,
                biomes: None,
                block_light: None,
                sky_light: Some(Box::new([0xff; LIGHT_BYTES])),
            }),
        );
        assert!(restore(&contradictory, &registration).is_none());
    }

    #[test]
    fn restored_skyless_light_processes_emission_and_section_status() {
        let (mut world, covered, registration) = saved_fixture(2);
        world.dimension = "minecraft:the_nether".into();
        let center = ChunkPos { x: 0, z: 0 };
        let bounds = LightBounds::new(0, 32).unwrap();
        let mut tracker = SectionLightTracker::from_saved(
            SavedSectionLightInputs {
                world: &world,
                bounds,
                covered: &covered,
                registration: &registration,
                active: BTreeSet::from([center]),
                has_block: true,
                has_sky: false,
            },
            |state| match state.name.as_str() {
                "minecraft:air" => Some(true),
                "minecraft:stone" => Some(false),
                _ => None,
            },
        )
        .unwrap();
        let torch = world.intern_state(BlockState {
            name: "minecraft:torch".into(),
            properties: BTreeMap::new(),
        });
        world.set_block_id(8, 20, 8, torch).unwrap();
        tracker
            .apply_changes(
                &world,
                bounds,
                &[(center, 1, true)],
                &[[8, 20, 8]],
                &BTreeMap::new(),
            )
            .unwrap();
        assert!(tracker.is_data_section(center, 1));
        assert_eq!(tracker.snapshot().block_light([8, 20, 8]), Some(14));
        assert_eq!(tracker.snapshot().sky_light([8, 20, 8]), Some(0));
        let before = tracker.snapshot().clone();
        assert!(
            tracker
                .apply_changes(&world, bounds, &[(center, 2, true)], &[], &BTreeMap::new())
                .is_none()
        );
        for &pos in &covered {
            assert_eq!(
                before.same_complete_column(tracker.snapshot(), pos),
                Some(true)
            );
        }
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn restored_saved_sections_and_later_callbacks_match_java() {
        use std::{path::PathBuf, process::Command};

        struct Row {
            block: bool,
            position: ChunkPos,
            y: i32,
            saved: bool,
            bytes: Box<[u8; LIGHT_BYTES]>,
        }
        fn rows<'a>(lines: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>) -> Vec<Row> {
            fn digit(value: u8) -> u8 {
                match value {
                    b'0'..=b'9' => value - b'0',
                    b'a'..=b'f' => value - b'a' + 10,
                    _ => panic!("invalid Java light byte"),
                }
            }
            let mut result = Vec::new();
            while lines.peek().is_some_and(|line| line.starts_with("L ")) {
                let mut parts = lines.next().unwrap().split_whitespace();
                assert_eq!(parts.next(), Some("L"));
                let block = match parts.next().unwrap() {
                    "B" => true,
                    "S" => false,
                    _ => panic!("invalid Java light channel"),
                };
                let x = parts.next().unwrap().parse().unwrap();
                let y = parts.next().unwrap().parse().unwrap();
                let z = parts.next().unwrap().parse().unwrap();
                let saved = match parts.next().unwrap() {
                    "0" => false,
                    "1" => true,
                    _ => panic!("invalid Java saved-light marker"),
                };
                let encoded = parts.next().unwrap().as_bytes();
                assert_eq!(encoded.len(), LIGHT_BYTES * 2);
                assert!(parts.next().is_none());
                let mut bytes = Box::new([0; LIGHT_BYTES]);
                for (byte, pair) in bytes.iter_mut().zip(encoded.chunks_exact(2)) {
                    *byte = digit(pair[0]) * 16 + digit(pair[1]);
                }
                result.push(Row {
                    block,
                    position: ChunkPos { x, z },
                    y,
                    saved,
                    bytes,
                });
            }
            result
        }
        fn compare(rows: &[Row], snapshot: &VisibleLightSnapshot, covered: &BTreeSet<ChunkPos>) {
            let mut seen = BTreeSet::new();
            for row in rows {
                assert!(seen.insert((row.block, row.position, row.y)));
                let column = snapshot.complete_column(row.position).unwrap();
                let layer = if row.block {
                    column.block_layers()
                } else {
                    column.sky_layers()
                }
                .get(&row.y)
                .expect("Java allocated layer missing");
                assert_eq!(
                    layer.as_ref(),
                    row.bytes.as_ref(),
                    "{:?} {}",
                    row.position,
                    row.y
                );
            }
            let actual: usize = covered
                .iter()
                .map(|&pos| {
                    let column = snapshot.complete_column(pos).unwrap();
                    column.block_layers().len() + column.sky_layers().len()
                })
                .sum();
            assert_eq!(rows.len(), actual, "extra restored light layers");
        }

        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft")
            });
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
            .join("bin");
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
        let classpath = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("SavedSectionLightOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/SavedSectionLightOracle.java"),
        )
        .unwrap();
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("SavedSectionLightOracle")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        let mut lines = text
            .lines()
            .filter(|line| line.starts_with("H ") || line.starts_with("L "))
            .peekable();
        let center = ChunkPos { x: 0, z: 0 };
        let east = ChunkPos { x: 1, z: 0 };
        let changes = [
            vec![(center, 8, false)],
            vec![(center, 8, false), (center, 8, true)],
            vec![(east, 4, true), (east, 4, false)],
            vec![(center, 0, false)],
            vec![(center, 0, false), (east, 0, false)],
            vec![(east, 4, true)],
        ];
        for (case, changes) in changes.iter().enumerate() {
            assert_eq!(lines.next(), Some(format!("H {case} before").as_str()));
            let before = rows(&mut lines);
            assert!(
                before
                    .iter()
                    .any(|row| !row.saved && row.bytes.iter().all(|&b| b == 0))
            );
            assert!(
                before
                    .iter()
                    .any(|row| row.saved && row.bytes.iter().all(|&b| b == 0))
            );
            assert!(
                before
                    .iter()
                    .any(|row| row.saved && row.bytes.iter().any(|&b| b != 0))
            );
            let (mut world, covered, registration) = saved_fixture(10);
            for row in before.iter().filter(|row| row.saved) {
                let chunk = Arc::make_mut(world.chunks.get_mut(&row.position).unwrap());
                let section = chunk
                    .sections
                    .entry(i8::try_from(row.y).unwrap())
                    .or_insert_with(|| {
                        Arc::new(Section {
                            blocks: None,
                            biomes: None,
                            block_light: None,
                            sky_light: None,
                        })
                    });
                let section = Arc::make_mut(section);
                if row.block {
                    section.block_light = Some(row.bytes.clone());
                } else {
                    section.sky_light = Some(row.bytes.clone());
                }
            }
            let mut tracker = SectionLightTracker::from_saved(
                SavedSectionLightInputs {
                    world: &world,
                    bounds: LightBounds::new(0, 160).unwrap(),
                    covered: &covered,
                    registration: &registration,
                    active: BTreeSet::from([center, ChunkPos { x: 2, z: 0 }]),
                    has_block: true,
                    has_sky: true,
                },
                |state| match state.name.as_str() {
                    "minecraft:air" => Some(true),
                    "minecraft:stone" => Some(false),
                    _ => None,
                },
            )
            .expect("saved Java revision must be restorable");
            compare(&before, tracker.snapshot(), &covered);
            tracker.apply_statuses(changes).unwrap();
            assert_eq!(lines.next(), Some(format!("H {case} after").as_str()));
            compare(&rows(&mut lines), tracker.snapshot(), &covered);
        }
        assert!(lines.next().is_none());

        let live_source = temp.path().join("SavedLiveLightOracle.java");
        std::fs::write(
            &live_source,
            include_str!("../tests/java/SavedLiveLightOracle.java"),
        )
        .unwrap();
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&live_source)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        for has_sky in [true, false] {
            let mut command = Command::new(java.join("java.exe"));
            command
                .arg("-cp")
                .arg(&classpath)
                .arg("SavedLiveLightOracle");
            if !has_sky {
                command.arg("no-sky");
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let text = String::from_utf8(output.stdout).unwrap();
            let mut lines = text
                .lines()
                .filter(|line| line.starts_with("H ") || line.starts_with("L "))
                .peekable();
            assert_eq!(lines.next(), Some("H 0"));
            let before = rows(&mut lines);
            let mut world = World::new(if has_sky {
                "minecraft:overworld"
            } else {
                "minecraft:the_nether"
            });
            let state = |name: &str| BlockState {
                name: format!("minecraft:{name}"),
                properties: BTreeMap::new(),
            };
            let air = world.intern_state(state("air"));
            let stone = world.intern_state(state("stone"));
            let torch = world.intern_state(state("torch"));
            let glass = world.intern_state(state("glass"));
            let mut registration = BTreeMap::new();
            for z in -3..=3 {
                for x in -3..=3 {
                    let position = ChunkPos { x, z };
                    let mut sections = BTreeMap::new();
                    for y in 0..2 {
                        let mut blocks = Box::new([air; BLOCK_COUNT]);
                        if y == 0 {
                            blocks[256] = stone;
                        }
                        sections.insert(
                            y,
                            Arc::new(Section {
                                blocks: Some(blocks),
                                biomes: None,
                                block_light: None,
                                sky_light: None,
                            }),
                        );
                    }
                    registration.insert(position, true);
                    world.chunks.insert(
                        position,
                        Arc::new(Chunk {
                            position,
                            status: Some("minecraft:full".into()),
                            block_light_correct: true,
                            sections,
                        }),
                    );
                }
            }
            let covered: BTreeSet<_> = (-2..=2)
                .flat_map(|z| (-2..=2).map(move |x| ChunkPos { x, z }))
                .collect();
            for row in before.iter().filter(|row| row.saved) {
                let chunk = Arc::make_mut(world.chunks.get_mut(&row.position).unwrap());
                let section = chunk
                    .sections
                    .entry(i8::try_from(row.y).unwrap())
                    .or_insert_with(|| {
                        Arc::new(Section {
                            blocks: None,
                            biomes: None,
                            block_light: None,
                            sky_light: None,
                        })
                    });
                let section = Arc::make_mut(section);
                if row.block {
                    section.block_light = Some(row.bytes.clone());
                } else {
                    section.sky_light = Some(row.bytes.clone());
                }
            }
            let bounds = LightBounds::new(0, 32).unwrap();
            let mut tracker = SectionLightTracker::from_saved(
                SavedSectionLightInputs {
                    world: &world,
                    bounds,
                    covered: &covered,
                    registration: &registration,
                    active: BTreeSet::from([center]),
                    has_block: true,
                    has_sky,
                },
                |state| match state.name.as_str() {
                    "minecraft:air" => Some(true),
                    "minecraft:stone" => Some(false),
                    _ => None,
                },
            )
            .expect("saved live light must be restorable");
            compare(&before, tracker.snapshot(), &covered);
            let mut sources = BTreeMap::new();
            if has_sky {
                let mut scanner = crate::sky_light::SkySourceScanner::new(&world, bounds);
                for &pos in registration.keys() {
                    let mut heights = [0; 256];
                    for z in 0..16 {
                        for x in 0..16 {
                            heights[(x + 16 * z) as usize] = scanner
                                .load(pos.x * 16 + x, pos.z * 16 + z)
                                .unwrap()
                                .lowest_source_y();
                        }
                    }
                    sources.insert(pos, Arc::new(heights));
                }
            }
            let position = [8, 20, 8];
            for (stage, (id, nonempty, check)) in [
                (torch, true, true),
                (air, false, true),
                (glass, true, false),
                (air, false, false),
                (stone, true, true),
                (air, false, true),
            ]
            .into_iter()
            .enumerate()
            {
                world
                    .set_block_id(position[0], position[1], position[2], id)
                    .unwrap();
                if check && has_sky {
                    let height = sources[&center][8 + 8 * 16];
                    let mut column =
                        crate::sky_light::SkyColumn::from_lowest_source(bounds, 8, 8, height)
                            .unwrap();
                    column.update(&world, position[1]).unwrap();
                    Arc::make_mut(sources.get_mut(&center).unwrap())[8 + 8 * 16] =
                        column.lowest_source_y();
                }
                let statuses = [(center, 1, nonempty)];
                let checks = if check {
                    std::slice::from_ref(&position)
                } else {
                    &[]
                };
                if stage == 0 && has_sky {
                    let previous = tracker.snapshot().clone();
                    assert!(
                        tracker
                            .apply_changes(&world, bounds, &statuses, checks, &BTreeMap::new())
                            .is_none()
                    );
                    assert!(!tracker.is_data_section(center, 1));
                    for &pos in &covered {
                        assert_eq!(
                            previous.same_complete_column(tracker.snapshot(), pos),
                            Some(true)
                        );
                    }
                }
                tracker
                    .apply_changes(&world, bounds, &statuses, checks, &sources)
                    .unwrap();
                assert_eq!(lines.next(), Some(format!("H {}", stage + 1).as_str()));
                compare(&rows(&mut lines), tracker.snapshot(), &covered);
            }
            assert!(lines.next().is_none());
        }
    }

    #[test]
    fn section_status_batches_preserve_shared_layers_and_cancelled_removals() {
        let center = ChunkPos { x: 0, z: 0 };
        let adjacent = ChunkPos { x: 1, z: 0 };
        let covered: Vec<_> = (-2..=2)
            .flat_map(|z| (-2..=3).map(move |x| ChunkPos { x, z }))
            .collect();
        let initial = [(center, 0), (adjacent, 0), (center, 8)];
        let mut visible = VisibleLightSnapshot::fresh_after_section_updates(
            true,
            true,
            initial,
            covered.iter().copied(),
        )
        .unwrap();
        visible.set_stored_sky([0, 128, 0], 7).unwrap();
        let before = visible.clone();
        let mut tracker = SectionLightTracker::new(visible, initial, BTreeSet::new()).unwrap();
        tracker
            .apply_statuses(&[(center, 8, false), (center, 8, true)])
            .unwrap();
        for &position in &covered {
            assert_eq!(
                before.same_complete_column(tracker.snapshot(), position),
                Some(true)
            );
        }
        assert!(tracker.is_data_section(center, 8));

        tracker.apply_statuses(&[(center, 0, false)]).unwrap();
        assert!(!tracker.is_data_section(center, 0));
        assert_eq!(
            tracker
                .snapshot()
                .sky_section_allocated(ChunkPos { x: -1, z: 0 }, 0),
            Some(false)
        );
        assert_eq!(
            tracker.snapshot().sky_section_allocated(center, 0),
            Some(true)
        );
        assert_eq!(tracker.snapshot().sky_light([0, 128, 0]), Some(7));

        let saved = tracker.snapshot().clone();
        assert!(
            tracker
                .apply_statuses(&[(adjacent, 0, false), (ChunkPos { x: 20, z: 0 }, 0, true)])
                .is_none()
        );
        assert!(tracker.is_data_section(adjacent, 0));
        for &position in &covered {
            assert_eq!(
                saved.same_complete_column(tracker.snapshot(), position),
                Some(true)
            );
        }
        assert_eq!(before.sky_light([0, 128, 0]), Some(7));
        assert!(SectionLightTracker::new(saved.clone(), [(center, 8)], BTreeSet::new()).is_none());
        assert!(
            SectionLightTracker::new(saved, [(center, 8), (center, 8)], BTreeSet::new()).is_none()
        );
    }

    #[test]
    fn section_statuses_outside_coverage_keep_identity_without_allocating_layers() {
        let center = ChunkPos { x: 0, z: 0 };
        let distant = ChunkPos { x: 100, z: -100 };
        let registered = [(center, 0), (distant, 4)];
        let covered = (-1..=1).flat_map(|z| (-1..=1).map(move |x| ChunkPos { x, z }));
        let visible =
            VisibleLightSnapshot::fresh_after_section_updates(true, true, registered, covered)
                .unwrap();
        let mut tracker =
            SectionLightTracker::new(visible.clone(), registered, BTreeSet::new()).unwrap();
        assert!(tracker.is_data_section(distant, 4));
        tracker.apply_statuses(&[(distant, 4, true)]).unwrap();
        assert!(tracker.apply_statuses(&[(distant, 4, false)]).is_none());
        assert!(tracker.is_data_section(distant, 4));
        assert_eq!(
            visible.same_complete_column(tracker.snapshot(), center),
            Some(true)
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn section_status_batches_match_java_visible_storage() {
        use std::{path::PathBuf, process::Command};

        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft")
            });
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
            .join("bin");
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
        let source = temp.path().join("SectionTransitionOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/SectionTransitionOracle.java"),
        )
        .unwrap();
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-encoding")
            .arg("UTF-8")
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
        let run = Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("SectionTransitionOracle")
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );

        let center = ChunkPos { x: 0, z: 0 };
        let adjacent = ChunkPos { x: 1, z: 0 };
        let other = ChunkPos { x: 2, z: 0 };
        let covered: Vec<_> = (-1..=1)
            .flat_map(|z| (-1..=3).map(move |x| ChunkPos { x, z }))
            .collect();
        let initial = [(center, 0), (adjacent, 0), (center, 8)];
        let mut visible = VisibleLightSnapshot::fresh_after_section_updates(
            true,
            true,
            initial,
            covered.iter().copied(),
        )
        .unwrap();
        for y in 0..16 {
            for z in 0..16 {
                for x in 0..16 {
                    let value = ((x * 3 + y * 5 + z * 7) & 15) as u8;
                    visible.set_stored_block([x, y + 128, z], value).unwrap();
                    visible
                        .set_stored_sky([x, y + 128, z], (value + 4) & 15)
                        .unwrap();
                }
            }
        }
        let active = BTreeSet::from([center, other]);
        let cases: Vec<Vec<_>> = vec![
            vec![(center, 8, false)],
            vec![(center, 8, false), (center, 8, true)],
            vec![(other, 4, true), (other, 4, false)],
            vec![(center, 8, false), (center, 9, true)],
            vec![(center, 0, false)],
            vec![
                (adjacent, 0, false),
                (adjacent, 0, false),
                (other, 0, true),
                (other, 0, true),
                (center, 0, false),
                (center, 0, true),
            ],
            vec![(center, 0, false), (adjacent, 0, false), (center, 8, false)],
            vec![(center, 7, true), (center, 8, false)],
        ];

        fn compare<'a>(
            lines: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>,
            snapshot: &VisibleLightSnapshot,
            covered: &[ChunkPos],
        ) {
            let mut seen = BTreeSet::new();
            while lines.peek().is_some_and(|line| line.starts_with("L ")) {
                let row = lines.next().unwrap();
                let mut fields = row.split_whitespace();
                assert_eq!(fields.next(), Some("L"));
                let channel = fields.next().unwrap();
                let x: i32 = fields.next().unwrap().parse().unwrap();
                let y: i32 = fields.next().unwrap().parse().unwrap();
                let z: i32 = fields.next().unwrap().parse().unwrap();
                let encoded = fields.next().unwrap().as_bytes();
                assert!(fields.next().is_none());
                assert_eq!(encoded.len(), LIGHT_BYTES * 2);
                let position = ChunkPos { x, z };
                assert!(seen.insert((channel, position, y)));
                let column = snapshot.complete_column(position).unwrap();
                let layer = match channel {
                    "B" => column.block_layers().get(&y),
                    "S" => column.sky_layers().get(&y),
                    _ => panic!("invalid Java light channel"),
                }
                .unwrap_or_else(|| panic!("missing Java layer {channel} {position:?} {y}"));
                for (index, pair) in encoded.chunks_exact(2).enumerate() {
                    let expected =
                        u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap();
                    assert_eq!(
                        layer[index], expected,
                        "{channel} {position:?} {y} byte {index}"
                    );
                }
            }
            let count: usize = covered
                .iter()
                .map(|&position| {
                    let column = snapshot.complete_column(position).unwrap();
                    column.block_layers().len() + column.sky_layers().len()
                })
                .sum();
            assert_eq!(seen.len(), count, "extra Rust light layers");
        }

        let output = String::from_utf8(run.stdout).unwrap();
        let mut lines = output
            .lines()
            .filter(|line| line.starts_with("H ") || line.starts_with("L "))
            .peekable();
        for (index, changes) in cases.iter().enumerate() {
            let mut tracker =
                SectionLightTracker::new(visible.clone(), initial, active.clone()).unwrap();
            assert_eq!(lines.next(), Some(format!("H {index} before").as_str()));
            compare(&mut lines, tracker.snapshot(), &covered);
            tracker.apply_statuses(changes).unwrap();
            assert_eq!(lines.next(), Some(format!("H {index} after").as_str()));
            compare(&mut lines, tracker.snapshot(), &covered);
        }
        assert!(lines.next().is_none());
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
