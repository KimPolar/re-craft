//! Saved light data and section registration at explicit light-engine drains.
//!
//! Java can keep saved layers in a queue even while their sections are not
//! allocated. When a section is registered, its initial layer can depend on
//! that queue and on earlier registrations in the same batch.

use super::{
    LightBounds, SectionLightTracker, VisibleLightSnapshot, initialized_nonempty_sections,
    section_neighborhood,
};
use crate::loaded::{BlockState, ChunkPos, LIGHT_BYTES, World};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum LightChannel {
    Block,
    Sky,
}

/// One `DataLayer`, including Java's distinction between an unmaterialized
/// homogeneous value and a byte-backed layer containing the same values.
#[derive(Clone, Debug)]
pub struct StoredLightLayer {
    bytes: Arc<[u8; LIGHT_BYTES]>,
    homogeneous: Option<u8>,
}

impl StoredLightLayer {
    pub fn bytes(&self) -> &Arc<[u8; LIGHT_BYTES]> {
        &self.bytes
    }

    pub fn homogeneous_value(&self) -> Option<u8> {
        self.homogeneous
    }

    pub fn is_empty(&self) -> bool {
        self.homogeneous == Some(0)
    }
}

/// Events are applied in their supplied order before one engine drain. Saved
/// bytes represent independent, byte-backed Java `DataLayer` instances. A
/// `None` layer clears a pending queue entry, not an allocated visible layer.
#[derive(Clone)]
pub enum QueuedLightEvent {
    Queue {
        channel: LightChannel,
        position: ChunkPos,
        section_y: i32,
        bytes: Option<Arc<[u8; LIGHT_BYTES]>>,
    },
    SectionStatus {
        position: ChunkPos,
        section_y: i32,
        nonempty: bool,
    },
    Enable {
        position: ChunkPos,
        enabled: bool,
        /// Java's `getHighestLowestSourceY` at this call. Use `i32::MIN`
        /// for its uninitialized fallback; required when enabling sky light.
        highest_source_y: Option<i32>,
    },
    Retain {
        position: ChunkPos,
        retain: bool,
    },
}

type LayerKey = (LightChannel, ChunkPos, i32);

/// A fresh engine advanced only through the provided, ordered load events.
/// Callers must include every affected column and reproduce actual drain
/// boundaries. Light-source propagation is not part of these events.
#[derive(Clone)]
pub struct QueuedSectionLightTracker {
    tracker: SectionLightTracker,
    queued: BTreeMap<LayerKey, StoredLightLayer>,
    forms: BTreeMap<LayerKey, Option<u8>>,
    retained: BTreeSet<ChunkPos>,
    zero: Arc<[u8; LIGHT_BYTES]>,
    full: Arc<[u8; LIGHT_BYTES]>,
}

impl QueuedSectionLightTracker {
    pub fn fresh(
        has_block: bool,
        has_sky: bool,
        covered_columns: impl IntoIterator<Item = ChunkPos>,
    ) -> Option<Self> {
        let visible = VisibleLightSnapshot::fresh_after_section_updates(
            has_block,
            has_sky,
            [],
            covered_columns,
        )?;
        Some(Self {
            tracker: SectionLightTracker::new(visible, [], BTreeSet::new())?,
            queued: BTreeMap::new(),
            forms: BTreeMap::new(),
            retained: BTreeSet::new(),
            zero: Arc::new([0; LIGHT_BYTES]),
            full: Arc::new([0xff; LIGHT_BYTES]),
        })
    }

    pub fn snapshot(&self) -> &VisibleLightSnapshot {
        self.tracker.snapshot()
    }

    /// Extend a continuously observed engine's coverage as streaming moves.
    /// All earlier events must have gone through this tracker: its event API
    /// cannot modify or allocate an uncovered column. Existing columns and
    /// previously published snapshots remain unchanged on failure.
    pub fn cover_columns(&mut self, columns: impl IntoIterator<Item = ChunkPos>) -> Option<()> {
        let mut added = BTreeSet::new();
        for position in columns {
            self.valid_position(position, 0)?;
            added.insert(position);
        }
        for position in added {
            self.tracker.visible.columns.entry(position).or_default();
        }
        Some(())
    }

    /// Include all columns a section-status callback for this chunk may touch.
    pub fn cover_chunk_halo(&mut self, position: ChunkPos) -> Option<()> {
        self.cover_columns(
            section_neighborhood(position, 0)?
                .into_iter()
                .map(|(position, _)| position),
        )
    }

    /// Evict a column only when it has no active, retained, queued, allocated
    /// or registered state. This keeps empty streaming margins from growing
    /// indefinitely without forgetting anything that can affect later loads.
    pub fn forget_empty_columns(&mut self, columns: impl IntoIterator<Item = ChunkPos>) -> usize {
        let mut removed = 0;
        for position in columns {
            let Some(column) = self.tracker.visible.columns.get(&position) else {
                continue;
            };
            if !column.block.is_empty()
                || !column.sky.is_empty()
                || self.tracker.active.contains(&position)
                || self.retained.contains(&position)
                || self
                    .tracker
                    .data_sections
                    .range((position, i32::MIN)..=(position, i32::MAX))
                    .next()
                    .is_some()
                || self
                    .tracker
                    .references
                    .range((position, i32::MIN)..=(position, i32::MAX))
                    .next()
                    .is_some()
                || [LightChannel::Block, LightChannel::Sky]
                    .into_iter()
                    .any(|channel| {
                        let range = (channel, position, i32::MIN)..=(channel, position, i32::MAX);
                        self.queued.range(range.clone()).next().is_some()
                            || self.forms.range(range).next().is_some()
                    })
            {
                continue;
            }
            self.tracker.visible.columns.remove(&position);
            removed += 1;
        }
        removed
    }

    /// An allocated layer in the published storage, excluding queued layers.
    /// Outer `None` means the column or coordinate is unknown.
    pub fn visible_layer(
        &self,
        channel: LightChannel,
        position: ChunkPos,
        section_y: i32,
    ) -> Option<Option<StoredLightLayer>> {
        self.valid_position(position, section_y)?;
        let column = self.tracker.visible.columns.get(&position)?;
        let layers = match channel {
            LightChannel::Block => &column.block,
            LightChannel::Sky => &column.sky,
        };
        let Some(bytes) = layers.get(&section_y) else {
            return Some(None);
        };
        Some(Some(StoredLightLayer {
            bytes: Arc::clone(bytes),
            homogeneous: *self.forms.get(&(channel, position, section_y))?,
        }))
    }

    /// Java's `getDataLayerData` checks queued data before visible storage.
    pub fn data_layer(
        &self,
        channel: LightChannel,
        position: ChunkPos,
        section_y: i32,
    ) -> Option<Option<StoredLightLayer>> {
        self.valid_position(position, section_y)?;
        self.tracker.visible.columns.get(&position)?;
        if let Some(layer) = self.queued.get(&(channel, position, section_y)) {
            return Some(Some(layer.clone()));
        }
        self.visible_layer(channel, position, section_y)
    }

    /// Apply Java's `isEmpty`/`getData` save sequence. Reading a nonempty
    /// homogeneous layer materializes it; an empty homogeneous zero is omitted.
    /// Outer `None` means the column or coordinate is unknown.
    pub fn serialize_layer(
        &mut self,
        channel: LightChannel,
        position: ChunkPos,
        section_y: i32,
    ) -> Option<Option<Arc<[u8; LIGHT_BYTES]>>> {
        let Some(layer) = self.data_layer(channel, position, section_y)? else {
            return Some(None);
        };
        if layer.is_empty() {
            return Some(None);
        }
        let key = (channel, position, section_y);
        if let Some(queued) = self.queued.get_mut(&key) {
            queued.homogeneous = None;
        } else {
            *self.forms.get_mut(&key)? = None;
        }
        Some(Some(layer.bytes))
    }

    /// Produce the `SerializableChunkData` retain and queue calls for one
    /// loaded chunk. The caller decides when these calls and later section
    /// initialization actually drain, and preserves their real order.
    pub fn saved_chunk_events(
        world: &World,
        position: ChunkPos,
        bounds: LightBounds,
        has_sky: bool,
    ) -> Option<Vec<QueuedLightEvent>> {
        let chunk = world.chunks.get(&position)?;
        if chunk.position != position {
            return None;
        }
        let light = bounds.light_range();
        let light_sections = light.start.div_euclid(16)..light.end.div_euclid(16);
        let mut events = Vec::new();
        let mut retained = false;
        for (&y, section) in &chunk.sections {
            let y = i32::from(y);
            for (channel, data) in [
                (LightChannel::Block, &section.block_light),
                (LightChannel::Sky, &section.sky_light),
            ] {
                let Some(data) = data else { continue };
                if channel == LightChannel::Sky && !has_sky {
                    continue;
                }
                if !light_sections.contains(&y) {
                    return None;
                }
                if !retained {
                    events.push(QueuedLightEvent::Retain {
                        position,
                        retain: true,
                    });
                    retained = true;
                }
                events.push(QueuedLightEvent::Queue {
                    channel,
                    position,
                    section_y: y,
                    bytes: Some(Arc::new(**data)),
                });
            }
        }
        Some(events)
    }

    /// Produce the nonempty-section callbacks for `initializeLight`'s
    /// pre-update task. The subsequent enable/retain calls are separate
    /// post-update events and must not be moved before the intervening drain.
    pub fn initialization_events(
        world: &World,
        position: ChunkPos,
        bounds: LightBounds,
        is_air: impl FnMut(&BlockState) -> Option<bool>,
    ) -> Option<Vec<QueuedLightEvent>> {
        let build = bounds.build_range();
        let sections = build.start.div_euclid(16)..build.end.div_euclid(16);
        Some(
            initialized_nonempty_sections(world, sections, [position], is_air)?
                .into_iter()
                .map(|(position, section_y)| QueuedLightEvent::SectionStatus {
                    position,
                    section_y,
                    nonempty: true,
                })
                .collect(),
        )
    }

    /// Replay one task batch and publish it atomically. Missing coverage or
    /// invalid coordinates reject the complete batch without changing state.
    pub fn apply_events(&mut self, events: &[QueuedLightEvent]) -> Option<()> {
        let mut draft = self.clone();
        let (removed, _) = draft.apply_events_inner(events)?;
        draft.drain(removed)?;
        *self = draft;
        Some(())
    }

    /// Drain section and saved-layer events together with block checks. The
    /// supplied world and sky-source heights must be the final revision for
    /// this batch. A failed light check rolls back the events as well.
    pub fn apply_changes(
        &mut self,
        world: &World,
        bounds: LightBounds,
        events: &[QueuedLightEvent],
        positions: &[[i32; 3]],
        sky_sources: &BTreeMap<ChunkPos, Arc<[i32; 256]>>,
    ) -> Option<()> {
        let build = bounds.build_range();
        let sections = build.start.div_euclid(16)..build.end.div_euclid(16);
        if events.iter().any(|event| {
            matches!(event, QueuedLightEvent::SectionStatus { section_y, .. } if !sections.contains(section_y))
        }) {
            return None;
        }
        let mut draft = self.clone();
        let (removed, aliases) = draft.apply_events_inner(events)?;
        let before = draft.tracker.visible.clone();
        draft
            .tracker
            .apply_checks(world, bounds, positions, sky_sources)?;
        draft.mark_materialized(&before)?;
        for (channel, position, y) in aliases {
            let layer = draft.visible_layer(channel, position, y)??;
            *draft.queued.get_mut(&(channel, position, y))? = layer;
        }
        draft.drain(removed)?;
        *self = draft;
        Some(())
    }

    /// Run `propagateLightSources` for one generated chunk against the
    /// current saved/visible revision. The caller supplies the actual current
    /// source heights and a fully drained engine; queued, unallocated saved
    /// layers remain queued. Missing inputs reject both channels atomically.
    pub fn propagate_chunk(
        &mut self,
        world: &World,
        bounds: LightBounds,
        position: ChunkPos,
        sky_sources: &BTreeMap<ChunkPos, Arc<[i32; 256]>>,
    ) -> Option<()> {
        self.valid_position(position, 0)?;
        self.tracker.visible.columns.get(&position)?;
        if world.chunks.get(&position)?.position != position {
            return None;
        }
        let mut draft = self.clone();
        let before = draft.tracker.visible.clone();
        let after_block = if draft.enabled(LightChannel::Block) {
            crate::generation_block_light::activate_generated_block_light(
                world, bounds, &before, position,
            )?
        } else {
            before.clone()
        };
        draft.tracker.visible = if draft.enabled(LightChannel::Sky) {
            crate::generation_sky_light::activate_generated_sky_light(
                world,
                bounds,
                &after_block,
                position,
                sky_sources,
            )?
        } else {
            after_block
        };
        draft.mark_materialized(&before)?;
        draft.tracker.active.insert(position);
        *self = draft;
        Some(())
    }

    fn mark_materialized(&mut self, before: &VisibleLightSnapshot) -> Option<()> {
        for (&position, old_column) in &before.columns {
            let column = self.tracker.visible.columns.get(&position)?;
            for (channel, old, new) in [
                (LightChannel::Block, &old_column.block, &column.block),
                (LightChannel::Sky, &old_column.sky, &column.sky),
            ] {
                if old.len() != new.len() {
                    return None;
                }
                for (&y, previous) in old {
                    if !Arc::ptr_eq(previous, new.get(&y)?) {
                        *self.forms.get_mut(&(channel, position, y))? = None;
                    }
                }
            }
        }
        Some(())
    }

    fn apply_events_inner(
        &mut self,
        events: &[QueuedLightEvent],
    ) -> Option<(BTreeSet<(ChunkPos, i32)>, BTreeSet<LayerKey>)> {
        let mut removed = BTreeSet::new();
        let mut aliases = BTreeSet::new();
        for event in events {
            match event {
                QueuedLightEvent::Queue {
                    channel,
                    position,
                    section_y,
                    bytes,
                } => {
                    self.valid_position(*position, *section_y)?;
                    self.tracker.visible.columns.get(position)?;
                    if !self.enabled(*channel) {
                        continue;
                    }
                    let key = (*channel, *position, *section_y);
                    aliases.remove(&key);
                    if let Some(bytes) = bytes {
                        self.queued.insert(
                            key,
                            StoredLightLayer {
                                bytes: Arc::clone(bytes),
                                homogeneous: None,
                            },
                        );
                    } else {
                        self.queued.remove(&key);
                    }
                }
                QueuedLightEvent::SectionStatus {
                    position,
                    section_y,
                    nonempty,
                } => self.status(*position, *section_y, *nonempty, &mut removed, &mut aliases)?,
                QueuedLightEvent::Enable {
                    position,
                    enabled,
                    highest_source_y,
                } => {
                    self.valid_position(*position, 0)?;
                    self.tracker.visible.columns.get(position)?;
                    if *enabled {
                        self.tracker.active.insert(*position);
                        if self.enabled(LightChannel::Sky) {
                            self.enable_sky(*position, (*highest_source_y)?, &aliases)?;
                        }
                    } else {
                        self.tracker.active.remove(position);
                    }
                }
                QueuedLightEvent::Retain { position, retain } => {
                    self.valid_position(*position, 0)?;
                    self.tracker.visible.columns.get(position)?;
                    if *retain {
                        self.retained.insert(*position);
                    } else {
                        self.retained.remove(position);
                    }
                }
            }
        }
        Some((removed, aliases))
    }

    fn valid_position(&self, position: ChunkPos, y: i32) -> Option<()> {
        ((-2_097_152..=2_097_151).contains(&position.x)
            && (-2_097_152..=2_097_151).contains(&position.z)
            && (-524_288..=524_287).contains(&y))
        .then_some(())
    }

    fn enabled(&self, channel: LightChannel) -> bool {
        match channel {
            LightChannel::Block => self.tracker.visible.has_block,
            LightChannel::Sky => self.tracker.visible.has_sky,
        }
    }

    fn status(
        &mut self,
        position: ChunkPos,
        y: i32,
        nonempty: bool,
        removed: &mut BTreeSet<(ChunkPos, i32)>,
        aliases: &mut BTreeSet<LayerKey>,
    ) -> Option<()> {
        let neighborhood = section_neighborhood(position, y)?;
        if self.tracker.data_sections.contains(&(position, y)) == nonempty {
            return Some(());
        }
        if neighborhood
            .iter()
            .any(|(pos, _)| !self.tracker.visible.columns.contains_key(pos))
        {
            return None;
        }
        if nonempty {
            self.tracker.data_sections.insert((position, y));
            for (pos, sy) in neighborhood {
                let count = self.tracker.references.entry((pos, sy)).or_default();
                let allocate = *count == 0;
                *count = count.checked_add(1)?;
                if allocate && !removed.remove(&(pos, sy)) {
                    for channel in [LightChannel::Block, LightChannel::Sky] {
                        if self.enabled(channel) {
                            if self.queued.contains_key(&(channel, pos, sy)) {
                                aliases.insert((channel, pos, sy));
                            }
                            let layer = self.create_layer(channel, pos, sy)?;
                            self.put_layer(channel, pos, sy, layer)?;
                        }
                    }
                }
            }
        } else {
            self.tracker.data_sections.remove(&(position, y));
            for (pos, sy) in neighborhood {
                let count = self.tracker.references.get_mut(&(pos, sy))?;
                *count = count.checked_sub(1)?;
                if *count == 0 {
                    self.tracker.references.remove(&(pos, sy));
                    removed.insert((pos, sy));
                }
            }
        }
        Some(())
    }

    fn enable_sky(
        &mut self,
        position: ChunkPos,
        highest_source_y: i32,
        aliases: &BTreeSet<LayerKey>,
    ) -> Option<()> {
        // Java uses wrapping int arithmetic before converting block Y to a
        // section. Its uninitialized source sentinel therefore fills nothing.
        let first = highest_source_y.wrapping_sub(1).div_euclid(16) + 1;
        let column = self.tracker.visible.columns.get(&position)?;
        let empty: Vec<_> = column
            .sky
            .range(first..)
            .filter_map(|(&y, _)| {
                (self.forms.get(&(LightChannel::Sky, position, y)) == Some(&Some(0))).then_some(y)
            })
            .collect();
        for y in empty {
            let layer = StoredLightLayer {
                bytes: Arc::clone(&self.full),
                homogeneous: Some(15),
            };
            self.put_layer(LightChannel::Sky, position, y, layer.clone())?;
            let key = (LightChannel::Sky, position, y);
            if aliases.contains(&key) {
                *self.queued.get_mut(&key)? = layer;
            }
        }
        Some(())
    }

    fn create_layer(
        &self,
        channel: LightChannel,
        position: ChunkPos,
        y: i32,
    ) -> Option<StoredLightLayer> {
        if let Some(layer) = self.queued.get(&(channel, position, y)) {
            return Some(layer.clone());
        }
        if channel == LightChannel::Sky {
            let column = self.tracker.visible.columns.get(&position)?;
            if let Some((&above_y, above)) = column.sky.range((y + 1)..).next() {
                let form = *self.forms.get(&(channel, position, above_y))?;
                if form.is_some() {
                    return Some(StoredLightLayer {
                        bytes: Arc::clone(above),
                        homogeneous: form,
                    });
                }
                let mut bytes = [0; LIGHT_BYTES];
                for slice in bytes.chunks_exact_mut(LIGHT_BYTES / 16) {
                    slice.copy_from_slice(&above[..LIGHT_BYTES / 16]);
                }
                return Some(StoredLightLayer {
                    bytes: Arc::new(bytes),
                    homogeneous: None,
                });
            }
            if self.tracker.active.contains(&position) {
                return Some(StoredLightLayer {
                    bytes: Arc::clone(&self.full),
                    homogeneous: Some(15),
                });
            }
        }
        Some(StoredLightLayer {
            bytes: Arc::clone(&self.zero),
            homogeneous: Some(0),
        })
    }

    fn put_layer(
        &mut self,
        channel: LightChannel,
        position: ChunkPos,
        y: i32,
        layer: StoredLightLayer,
    ) -> Option<()> {
        let column = self.tracker.visible.columns.get_mut(&position)?;
        match channel {
            LightChannel::Block => column.block.insert(y, layer.bytes),
            LightChannel::Sky => column.sky.insert(y, layer.bytes),
        };
        self.forms.insert((channel, position, y), layer.homogeneous);
        Some(())
    }

    fn remove_layer(
        &mut self,
        channel: LightChannel,
        position: ChunkPos,
        y: i32,
    ) -> Option<StoredLightLayer> {
        let column = self.tracker.visible.columns.get_mut(&position)?;
        let bytes = match channel {
            LightChannel::Block => column.block.remove(&y),
            LightChannel::Sky => column.sky.remove(&y),
        }?;
        Some(StoredLightLayer {
            bytes,
            homogeneous: self.forms.remove(&(channel, position, y))?,
        })
    }

    fn drain(&mut self, removed: BTreeSet<(ChunkPos, i32)>) -> Option<()> {
        for (position, y) in removed {
            for channel in [LightChannel::Block, LightChannel::Sky] {
                if !self.enabled(channel) {
                    continue;
                }
                let key = (channel, position, y);
                let queued = self.queued.remove(&key);
                let visible = self.remove_layer(channel, position, y)?;
                if self.retained.contains(&position) {
                    self.queued.insert(key, queued.unwrap_or(visible));
                }
            }
        }
        let ready: Vec<_> = self
            .queued
            .keys()
            .copied()
            .filter(|&(_, position, y)| self.tracker.references.contains_key(&(position, y)))
            .collect();
        for (channel, position, y) in ready {
            let layer = self.queued.remove(&(channel, position, y))?;
            self.put_layer(channel, position, y, layer)?;
        }
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn center() -> ChunkPos {
        ChunkPos { x: 0, z: 0 }
    }

    fn fresh(block: bool, sky: bool) -> QueuedSectionLightTracker {
        QueuedSectionLightTracker::fresh(
            block,
            sky,
            (-2..=2).flat_map(|x| (-2..=2).map(move |z| ChunkPos { x, z })),
        )
        .unwrap()
    }

    fn pattern(kind: i32) -> Arc<[u8; LIGHT_BYTES]> {
        let mut bytes = [0; LIGHT_BYTES];
        for (i, value) in bytes.iter_mut().enumerate() {
            *value = match kind {
                0 => 0,
                1 => (i as u8).wrapping_mul(37).wrapping_add(11),
                2 => 0xff,
                3 => (i as u8).wrapping_mul(19).wrapping_add(7),
                _ => panic!("invalid fixture"),
            };
        }
        Arc::new(bytes)
    }

    fn queue(channel: LightChannel, y: i32, kind: i32) -> QueuedLightEvent {
        QueuedLightEvent::Queue {
            channel,
            position: center(),
            section_y: y,
            bytes: (kind >= 0).then(|| pattern(kind)),
        }
    }

    fn status(y: i32, nonempty: bool) -> QueuedLightEvent {
        QueuedLightEvent::SectionStatus {
            position: center(),
            section_y: y,
            nonempty,
        }
    }

    #[cfg(target_os = "windows")]
    fn compare_capture<'a>(
        lines: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>,
        name: &str,
        tracker: &QueuedSectionLightTracker,
    ) {
        assert_eq!(lines.next(), Some(format!("H {name}").as_str()));
        let mut seen = BTreeSet::new();
        while lines.peek().is_some_and(|line| line.starts_with("L ")) {
            let parts: Vec<_> = lines.next().unwrap().split(' ').collect();
            let ch = match parts[2] {
                "B" => LightChannel::Block,
                "S" => LightChannel::Sky,
                _ => panic!("unknown channel"),
            };
            let pos = ChunkPos {
                x: parts[3].parse().unwrap(),
                z: parts[5].parse().unwrap(),
            };
            let y = parts[4].parse().unwrap();
            assert!(seen.insert((parts[1], ch, pos, y)));
            let layer = match parts[1] {
                "V" => tracker.visible_layer(ch, pos, y),
                "D" => tracker.data_layer(ch, pos, y),
                _ => panic!("unknown layer source"),
            }
            .unwrap()
            .unwrap_or_else(|| panic!("missing Java layer: {name} {} {pos:?} {y}", parts[1]));
            let form = layer
                .homogeneous_value()
                .map_or_else(|| "M".to_owned(), |value| format!("H{value}"));
            assert_eq!(form, parts[6], "{name} {} {ch:?} {pos:?} {y}", parts[1]);
            assert_eq!(parts[7].len(), LIGHT_BYTES * 2);
            let expected: Vec<_> = parts[7]
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                .collect();
            assert_eq!(
                layer.bytes().as_slice(),
                expected,
                "{name} {} {pos:?} {y}",
                parts[1]
            );
        }
        let mut expected_count = 0;
        for ch in [LightChannel::Block, LightChannel::Sky] {
            for x in -2..=2 {
                for z in -2..=2 {
                    for y in -1..=5 {
                        let pos = ChunkPos { x, z };
                        expected_count +=
                            usize::from(tracker.visible_layer(ch, pos, y).unwrap().is_some());
                        expected_count +=
                            usize::from(tracker.data_layer(ch, pos, y).unwrap().is_some());
                    }
                }
            }
        }
        assert_eq!(seen.len(), expected_count, "{name}: extra Rust layer");
    }

    #[test]
    fn queued_layer_order_changes_sky_inheritance() {
        let mut before = fresh(true, true);
        before
            .apply_events(&[queue(LightChannel::Sky, 2, 1), status(2, true)])
            .unwrap();
        let inherited = before
            .visible_layer(LightChannel::Sky, center(), 1)
            .unwrap()
            .unwrap();
        assert_eq!(inherited.homogeneous_value(), None);
        for slice in inherited.bytes().chunks_exact(LIGHT_BYTES / 16) {
            assert_eq!(slice, &pattern(1)[..LIGHT_BYTES / 16]);
        }

        let mut after = fresh(true, true);
        after
            .apply_events(&[status(2, true), queue(LightChannel::Sky, 2, 1)])
            .unwrap();
        assert!(
            after
                .visible_layer(LightChannel::Sky, center(), 1)
                .unwrap()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            after
                .visible_layer(LightChannel::Sky, center(), 2)
                .unwrap()
                .unwrap()
                .bytes(),
            &pattern(1)
        );
    }

    #[test]
    fn queued_unallocated_layers_and_retention_survive_drains() {
        let mut tracker = fresh(true, true);
        tracker
            .apply_events(&[
                QueuedLightEvent::Retain {
                    position: center(),
                    retain: true,
                },
                queue(LightChannel::Block, 2, 0),
            ])
            .unwrap();
        assert!(
            tracker
                .visible_layer(LightChannel::Block, center(), 2)
                .unwrap()
                .is_none()
        );
        assert!(
            !tracker
                .data_layer(LightChannel::Block, center(), 2)
                .unwrap()
                .unwrap()
                .is_empty()
        );
        tracker.apply_events(&[status(2, true)]).unwrap();
        tracker
            .apply_events(&[queue(LightChannel::Block, 2, 3), status(2, false)])
            .unwrap();
        assert!(
            tracker
                .visible_layer(LightChannel::Block, center(), 2)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            tracker
                .data_layer(LightChannel::Block, center(), 2)
                .unwrap()
                .unwrap()
                .bytes(),
            &pattern(3)
        );
        tracker.apply_events(&[status(2, true)]).unwrap();
        assert_eq!(
            tracker
                .visible_layer(LightChannel::Block, center(), 2)
                .unwrap()
                .unwrap()
                .bytes(),
            &pattern(3)
        );
    }

    #[test]
    fn serialization_materializes_nonzero_homogeneous_layers_only() {
        let mut tracker = fresh(true, true);
        tracker
            .apply_events(&[
                QueuedLightEvent::Enable {
                    position: center(),
                    enabled: true,
                    highest_source_y: Some(i32::MIN),
                },
                status(3, true),
            ])
            .unwrap();
        assert_eq!(
            tracker
                .visible_layer(LightChannel::Sky, center(), 2)
                .unwrap()
                .unwrap()
                .homogeneous_value(),
            Some(15)
        );
        assert!(
            tracker
                .serialize_layer(LightChannel::Block, center(), 2)
                .unwrap()
                .is_none()
        );
        assert!(
            tracker
                .serialize_layer(LightChannel::Sky, center(), 2)
                .unwrap()
                .is_some()
        );
        assert_eq!(
            tracker
                .visible_layer(LightChannel::Sky, center(), 2)
                .unwrap()
                .unwrap()
                .homogeneous_value(),
            None
        );
        tracker.apply_events(&[status(0, true)]).unwrap();
        assert_eq!(
            tracker
                .visible_layer(LightChannel::Sky, center(), 0)
                .unwrap()
                .unwrap()
                .homogeneous_value(),
            None
        );
    }

    #[test]
    fn invalid_batch_is_transactional_and_disabled_channel_is_ignored() {
        let mut tracker = fresh(true, false);
        tracker
            .apply_events(&[queue(LightChannel::Sky, 2, 1)])
            .unwrap();
        assert!(
            tracker
                .data_layer(LightChannel::Sky, center(), 2)
                .unwrap()
                .is_none()
        );
        let invalid = QueuedLightEvent::SectionStatus {
            position: ChunkPos { x: 2, z: 2 },
            section_y: 0,
            nonempty: true,
        };
        assert!(
            tracker
                .apply_events(&[queue(LightChannel::Block, 2, 1), invalid])
                .is_none()
        );
        assert!(
            tracker
                .data_layer(LightChannel::Block, center(), 2)
                .unwrap()
                .is_none()
        );
        let mut sky = fresh(true, true);
        assert!(
            sky.apply_events(&[
                status(2, true),
                QueuedLightEvent::Enable {
                    position: center(),
                    enabled: true,
                    highest_source_y: None
                },
            ])
            .is_none()
        );
        assert!(
            sky.visible_layer(LightChannel::Sky, center(), 2)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn saved_chunk_and_initialization_events_use_loaded_world_sections() {
        use crate::loaded::{BLOCK_COUNT, Chunk, Section};

        let mut world = World::new("minecraft:overworld");
        let state = |name: &str| BlockState {
            name: format!("minecraft:{name}"),
            properties: BTreeMap::new(),
        };
        let air = world.intern_state(state("air"));
        let stone = world.intern_state(state("stone"));
        let mut solid = Box::new([air; BLOCK_COUNT]);
        solid[0] = stone;
        let sections = BTreeMap::from([
            (
                -1,
                Arc::new(Section {
                    blocks: None,
                    biomes: None,
                    block_light: Some(Box::new([0; LIGHT_BYTES])),
                    sky_light: None,
                }),
            ),
            (
                0,
                Arc::new(Section {
                    blocks: Some(solid),
                    biomes: None,
                    block_light: None,
                    sky_light: Some(Box::new([0xff; LIGHT_BYTES])),
                }),
            ),
            (
                1,
                Arc::new(Section {
                    blocks: Some(Box::new([air; BLOCK_COUNT])),
                    biomes: None,
                    block_light: None,
                    sky_light: None,
                }),
            ),
        ]);
        world.chunks.insert(
            center(),
            Arc::new(Chunk {
                position: center(),
                status: Some("minecraft:full".into()),
                block_light_correct: true,
                sections,
            }),
        );
        let bounds = LightBounds::new(0, 32).unwrap();
        let saved =
            QueuedSectionLightTracker::saved_chunk_events(&world, center(), bounds, true).unwrap();
        assert_eq!(saved.len(), 3);
        assert!(matches!(
            saved[0],
            QueuedLightEvent::Retain { retain: true, .. }
        ));
        let initialize =
            QueuedSectionLightTracker::initialization_events(&world, center(), bounds, |state| {
                match state.name.as_str() {
                    "minecraft:air" => Some(true),
                    "minecraft:stone" => Some(false),
                    _ => None,
                }
            })
            .unwrap();
        assert_eq!(initialize.len(), 1);
        let mut tracker = fresh(true, true);
        tracker.apply_events(&saved).unwrap();
        tracker.apply_events(&initialize).unwrap();
        assert!(
            !tracker
                .visible_layer(LightChannel::Block, center(), -1)
                .unwrap()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            tracker
                .visible_layer(LightChannel::Sky, center(), 0)
                .unwrap()
                .unwrap()
                .bytes()
                .as_ref(),
            &[0xff; LIGHT_BYTES]
        );
        assert_eq!(
            QueuedSectionLightTracker::saved_chunk_events(&world, center(), bounds, false)
                .unwrap()
                .len(),
            2
        );

        let previous = tracker.snapshot().clone();
        assert!(
            tracker
                .apply_changes(
                    &world,
                    bounds,
                    &[status(1, true)],
                    &[[8, 32, 8]],
                    &BTreeMap::new()
                )
                .is_none()
        );
        assert_eq!(
            previous.same_complete_column(tracker.snapshot(), center()),
            Some(true)
        );
        assert!(!tracker.tracker.is_data_section(center(), 1));
        assert!(
            tracker
                .propagate_chunk(&world, bounds, center(), &BTreeMap::new())
                .is_none()
        );
        assert_eq!(
            previous.same_complete_column(tracker.snapshot(), center()),
            Some(true)
        );
        assert!(!tracker.tracker.active.contains(&center()));
    }

    #[test]
    fn streaming_coverage_preserves_state_and_only_evicts_empty_columns() {
        let mut tracker = QueuedSectionLightTracker::fresh(true, true, []).unwrap();
        tracker.cover_chunk_halo(center()).unwrap();
        let before = tracker.snapshot().clone();
        let far = ChunkPos { x: 5, z: 0 };
        assert!(
            tracker
                .cover_columns([far, ChunkPos { x: i32::MAX, z: 0 }])
                .is_none()
        );
        assert!(tracker.visible_layer(LightChannel::Block, far, 0).is_none());
        assert_eq!(
            before.same_complete_column(tracker.snapshot(), center()),
            Some(true)
        );
        tracker.apply_events(&[status(0, true)]).unwrap();
        let halo: Vec<_> = (-1..=1)
            .flat_map(|x| (-1..=1).map(move |z| ChunkPos { x, z }))
            .collect();
        assert_eq!(tracker.forget_empty_columns(halo.iter().copied()), 0);
        tracker.apply_events(&[status(0, false)]).unwrap();
        tracker
            .apply_events(&[queue(LightChannel::Block, 3, 1)])
            .unwrap();
        assert_eq!(tracker.forget_empty_columns(halo.iter().copied()), 8);
        assert!(
            tracker
                .data_layer(LightChannel::Block, center(), 3)
                .unwrap()
                .is_some()
        );
        tracker.cover_chunk_halo(center()).unwrap();
        tracker
            .apply_events(&[queue(LightChannel::Block, 3, -1)])
            .unwrap();
        assert_eq!(tracker.forget_empty_columns(halo), 9);
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn saved_queue_drains_and_serialization_match_java() {
        use std::{path::PathBuf, process::Command};

        const SCRIPT: &str = "\
N 1 1
Q S 0 2 0 1
Q B 0 2 0 0
T 0 2 0 1
D
C queued_first
N 1 1
T 0 2 0 1
Q S 0 2 0 1
Q B 0 2 0 0
D
C status_first
N 1 1
Q S 0 4 0 1
Q B 0 4 0 0
D
C pending
T 0 4 0 1
D
C registered
T 0 4 0 0
D
C removed
N 1 1
R 0 0 1
Q S 0 2 0 1
Q B 0 2 0 0
T 0 2 0 1
D
C retained_initial
Q B 0 2 0 3
Q S 0 2 0 2
T 0 2 0 0
D
C retained_removed
T 0 2 0 1
D
C retained_readded
R 0 0 0
T 0 2 0 0
D
C retained_disabled
N 1 1
E 0 0 1
T 0 3 0 1
D
C homogeneous
S S 0 2 0
S B 0 2 0
T 0 0 0 1
D
C materialized_inheritance
N 1 1
Q S 0 2 0 1
T 0 2 0 1
D
T 0 2 0 0
Q S 0 2 0 3
T 0 2 0 1
D
C same_batch_readdition
N 1 1
T 0 2 0 1
T 1 3 0 1
D
Q B 0 2 0 3
T 0 2 0 0
D
C shared_halo
T 1 3 0 0
D
C no_halo
N 1 1
Q B 0 2 0 1
Q B 0 2 0 -1
T 0 2 0 1
D
C cleared_queue
N 1 0
Q S 0 2 0 1
Q B 0 2 0 0
T 0 2 0 1
D
C no_sky
";

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
        let source = temp.path().join("QueuedSectionLightOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/QueuedSectionLightOracle.java"),
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
        let input = temp.path().join("events.txt");
        std::fs::write(&input, SCRIPT).unwrap();
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("QueuedSectionLightOracle")
            .arg(input)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = String::from_utf8(output.stdout).unwrap();
        let mut lines = output
            .lines()
            .filter(|line| line.starts_with("H ") || line.starts_with("L "))
            .peekable();
        let mut tracker = fresh(true, true);
        let mut events = Vec::new();
        let parse = |value: &str| value.parse::<i32>().unwrap();
        let channel = |value: &str| match value {
            "B" => LightChannel::Block,
            "S" => LightChannel::Sky,
            _ => panic!("unknown channel"),
        };
        for line in SCRIPT.lines().filter(|line| !line.is_empty()) {
            let p: Vec<_> = line.split(' ').collect();
            match p[0] {
                "N" => {
                    assert!(events.is_empty());
                    tracker = fresh(p[1] == "1", p[2] == "1");
                }
                "Q" => events.push(QueuedLightEvent::Queue {
                    channel: channel(p[1]),
                    position: ChunkPos {
                        x: parse(p[2]),
                        z: parse(p[4]),
                    },
                    section_y: parse(p[3]),
                    bytes: (parse(p[5]) >= 0).then(|| pattern(parse(p[5]))),
                }),
                "T" => events.push(QueuedLightEvent::SectionStatus {
                    position: ChunkPos {
                        x: parse(p[1]),
                        z: parse(p[3]),
                    },
                    section_y: parse(p[2]),
                    nonempty: p[4] == "1",
                }),
                "E" => events.push(QueuedLightEvent::Enable {
                    position: ChunkPos {
                        x: parse(p[1]),
                        z: parse(p[2]),
                    },
                    enabled: p[3] == "1",
                    highest_source_y: Some(i32::MIN),
                }),
                "R" => events.push(QueuedLightEvent::Retain {
                    position: ChunkPos {
                        x: parse(p[1]),
                        z: parse(p[2]),
                    },
                    retain: p[3] == "1",
                }),
                "D" => {
                    tracker.apply_events(&events).unwrap();
                    events.clear();
                }
                "S" => {
                    assert!(events.is_empty());
                    tracker
                        .serialize_layer(
                            channel(p[1]),
                            ChunkPos {
                                x: parse(p[2]),
                                z: parse(p[4]),
                            },
                            parse(p[3]),
                        )
                        .unwrap();
                }
                "C" => {
                    assert!(events.is_empty());
                    compare_capture(&mut lines, p[1], &tracker);
                }
                _ => panic!("invalid action: {line}"),
            }
        }
        assert!(lines.next().is_none());
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn queued_saved_light_and_block_checks_match_java() {
        use crate::loaded::{BLOCK_COUNT, Chunk, Section};
        use std::{path::PathBuf, process::Command};

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
        let files = [
            (
                "QueuedSectionLightOracle.java",
                include_str!("../../tests/java/QueuedSectionLightOracle.java"),
            ),
            (
                "SavedLiveLightOracle.java",
                include_str!("../../tests/java/SavedLiveLightOracle.java"),
            ),
            (
                "QueuedLiveLightOracle.java",
                include_str!("../../tests/java/QueuedLiveLightOracle.java"),
            ),
        ];
        let mut compiled = Command::new(java.join("javac.exe"));
        compiled.arg("-cp").arg(&classpath);
        for (name, contents) in files {
            let source = temp.path().join(name);
            std::fs::write(&source, contents).unwrap();
            compiled.arg(source);
        }
        let compiled = compiled.output().unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );

        for (has_sky, has_saved) in [(true, true), (false, true), (true, false), (false, false)] {
            let mut command = Command::new(java.join("java.exe"));
            command
                .arg("-cp")
                .arg(&classpath)
                .arg("QueuedLiveLightOracle");
            if !has_sky {
                command.arg("no-sky");
            }
            if !has_saved {
                command.arg("no-saved");
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let output = String::from_utf8(output.stdout).unwrap();
            let mut lines = output
                .lines()
                .filter(|line| line.starts_with("H ") || line.starts_with("L "))
                .peekable();

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
            for x in -3..=3 {
                for z in -3..=3 {
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
            let bounds = LightBounds::new(0, 32).unwrap();
            let covered = (-3..=3).flat_map(|x| (-3..=3).map(move |z| ChunkPos { x, z }));
            let mut tracker = QueuedSectionLightTracker::fresh(true, has_sky, covered).unwrap();
            let mut events = Vec::new();
            if has_saved {
                events.extend([
                    QueuedLightEvent::Retain {
                        position: center(),
                        retain: true,
                    },
                    QueuedLightEvent::Retain {
                        position: ChunkPos { x: -1, z: 0 },
                        retain: true,
                    },
                    queue(LightChannel::Block, 0, 0),
                ]);
            }
            if has_saved && has_sky {
                events.push(queue(LightChannel::Sky, 0, 2));
                events.push(queue(LightChannel::Sky, 2, 1));
            }
            for x in -2..=2 {
                for z in -2..=2 {
                    events.push(QueuedLightEvent::SectionStatus {
                        position: ChunkPos { x, z },
                        section_y: 0,
                        nonempty: true,
                    });
                }
            }
            tracker.apply_events(&events).unwrap();
            tracker
                .apply_events(&[QueuedLightEvent::Enable {
                    position: center(),
                    enabled: true,
                    highest_source_y: Some(2),
                }])
                .unwrap();
            compare_capture(&mut lines, "0", &tracker);
            let mut ordinary = (!has_saved).then(|| tracker.tracker.clone());
            let mut sources = BTreeMap::new();
            if has_sky {
                let mut scanner = crate::sky_light::SkySourceScanner::new(&world, bounds);
                for x in -3..=3 {
                    for z in -3..=3 {
                        let pos = ChunkPos { x, z };
                        let mut heights = [0; 256];
                        for dz in 0..16 {
                            for dx in 0..16 {
                                heights[(dx + 16 * dz) as usize] = scanner
                                    .load(x * 16 + dx, z * 16 + dz)
                                    .unwrap()
                                    .lowest_source_y();
                            }
                        }
                        sources.insert(pos, Arc::new(heights));
                    }
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
                    let height = sources[&center()][8 + 8 * 16];
                    let mut column =
                        crate::sky_light::SkyColumn::from_lowest_source(bounds, 8, 8, height)
                            .unwrap();
                    column.update(&world, position[1]).unwrap();
                    Arc::make_mut(sources.get_mut(&center()).unwrap())[8 + 8 * 16] =
                        column.lowest_source_y();
                }
                let checks = if check {
                    std::slice::from_ref(&position)
                } else {
                    &[]
                };
                let mut events = vec![status(1, nonempty)];
                if has_saved && stage == 0 {
                    events.push(queue(LightChannel::Block, 0, 0));
                }
                tracker
                    .apply_changes(&world, bounds, &events, checks, &sources)
                    .unwrap();
                if let Some(ordinary) = &mut ordinary {
                    ordinary
                        .apply_changes(&world, bounds, &[(center(), 1, nonempty)], checks, &sources)
                        .unwrap();
                    for x in -2..=2 {
                        for z in -2..=2 {
                            assert_eq!(
                                ordinary
                                    .snapshot()
                                    .same_complete_column(tracker.snapshot(), ChunkPos { x, z }),
                                Some(true),
                                "ordinary tracker stage {stage} ({x}, {z})"
                            );
                        }
                    }
                }
                compare_capture(&mut lines, &(stage + 1).to_string(), &tracker);
            }
            tracker
                .apply_events(&[
                    status(1, true),
                    QueuedLightEvent::Enable {
                        position: ChunkPos { x: -1, z: 0 },
                        enabled: true,
                        highest_source_y: Some(2),
                    },
                ])
                .unwrap();
            compare_capture(&mut lines, "7", &tracker);
            world
                .set_block_id(position[0], position[1], position[2], torch)
                .unwrap();
            if has_sky {
                let height = sources[&center()][8 + 8 * 16];
                let mut column =
                    crate::sky_light::SkyColumn::from_lowest_source(bounds, 8, 8, height).unwrap();
                column.update(&world, position[1]).unwrap();
                Arc::make_mut(sources.get_mut(&center()).unwrap())[8 + 8 * 16] =
                    column.lowest_source_y();
            }
            tracker
                .propagate_chunk(&world, bounds, center(), &sources)
                .unwrap();
            compare_capture(&mut lines, "8", &tracker);
            assert!(lines.next().is_none());
        }
    }
}
