//! Recompute sky light around one block and its changed direct-sky column.
//!
//! All necessary block states, current source heights and stored light layers
//! must be available. Incomplete input leaves the world unchanged.
use std::{
    collections::{BTreeMap, BTreeSet, BinaryHeap},
    sync::Arc,
};

use crate::{
    block_light::{
        DIRECTIONS, LightBounds, LightProperties, faces_occlude, light_properties,
        transparent_light_properties,
    },
    loaded::{BlockPosition, BlockStateId, ChunkPos, World},
    sky_light::UNBOUNDED_SKY_SOURCE,
};

const RADIUS: i32 = 14;

#[derive(Clone, Copy)]
struct Cell {
    behavior: usize,
    previous: u8,
    value: u8,
}

fn offset(pos: BlockPosition, d: (i32, i32, i32)) -> Option<BlockPosition> {
    Some(BlockPosition {
        x: pos.x.checked_add(d.0)?,
        y: pos.y.checked_add(d.1)?,
        z: pos.z.checked_add(d.2)?,
    })
}

fn behavior_at(
    world: &World,
    bounds: LightBounds,
    p: BlockPosition,
    ids: &mut BTreeMap<BlockStateId, usize>,
    values: &mut Vec<LightProperties>,
) -> Option<usize> {
    if !bounds.build_range().contains(&p.y) {
        return Some(0);
    }
    let id = world.block_id(p.x, p.y, p.z)?;
    behavior_for_id(world, id, ids, values)
}

fn behavior_for_id(
    world: &World,
    id: BlockStateId,
    ids: &mut BTreeMap<BlockStateId, usize>,
    values: &mut Vec<LightProperties>,
) -> Option<usize> {
    if let Some(&index) = ids.get(&id) {
        return Some(index);
    }
    let index = values.len();
    values.push(light_properties(world.state(id)?)?);
    ids.insert(id, index);
    Some(index)
}

fn stored_light(world: &World, p: BlockPosition) -> Option<u8> {
    let chunk = world.chunks.get(&p.chunk())?;
    if chunk.status.as_deref() != Some("minecraft:full") || !chunk.block_light_correct {
        return None;
    }
    world.light(p.x, p.y, p.z, true)
}

fn occluded(behaviors: &[LightProperties], a: usize, b: usize, direction: usize) -> bool {
    (behaviors[a].has_faces || behaviors[b].has_faces)
        && faces_occlude(
            &behaviors[a].faces[direction],
            &behaviors[b].faces[direction ^ 1],
        )
}

/// Recompute an edit after its direct source height has been updated.
///
/// `source_y` supplies the current Java direct-source height for each column,
/// including incremental history. The caller must provide up-to-date heights
/// for the complete affected area. All stored sky arrays must already exist.
/// The returned set contains the chunks whose stored light actually changed.
pub fn relight_change(
    world: &mut World,
    bounds: LightBounds,
    position: BlockPosition,
    previous_source: i32,
    current_source: i32,
    mut source_y: impl FnMut(i32, i32) -> Option<i32>,
) -> Option<BTreeSet<ChunkPos>> {
    let build_range = bounds.build_range();
    if !build_range.contains(&position.y) {
        return None;
    }
    let valid_source =
        |y| y == UNBOUNDED_SKY_SOURCE || (build_range.start..=build_range.end).contains(&y);
    if !valid_source(previous_source) || !valid_source(current_source) {
        return None;
    }
    let light_range = bounds.light_range();
    let normalize = |y: i32| {
        if y == UNBOUNDED_SKY_SOURCE {
            light_range.start
        } else {
            y.clamp(light_range.start, light_range.end - 1)
        }
    };
    let (low, high) = if previous_source == current_source {
        (position.y, position.y)
    } else {
        (
            normalize(previous_source)
                .min(normalize(current_source))
                .min(position.y),
            normalize(previous_source)
                .max(normalize(current_source))
                .max(position.y),
        )
    };
    let min_y = low.saturating_sub(RADIUS).max(light_range.start);
    let max_y = high.saturating_add(RADIUS).min(light_range.end - 1);
    let height = usize::try_from(max_y - min_y + 1).ok()?;
    let side = (RADIUS * 2 + 1) as usize;
    let mut cells = vec![None; side.checked_mul(side)?.checked_mul(height)?];
    let index = |dx: i32, y: i32, dz: i32| -> usize {
        (((dx + RADIUS) as usize * side + (dz + RADIUS) as usize) * height) + (y - min_y) as usize
    };
    let inside = |dx: i32, y: i32, dz: i32| {
        if dx.abs() > RADIUS || dz.abs() > RADIUS || y < min_y || y > max_y {
            return false;
        }
        let dy = if y < low {
            low - y
        } else if y > high {
            y - high
        } else {
            0
        };
        dx.abs() + dz.abs() + dy <= RADIUS
    };
    let mut ids = BTreeMap::new();
    let mut behaviors = vec![transparent_light_properties()];
    for dx in -RADIUS..=RADIUS {
        for dz in -RADIUS..=RADIUS {
            if dx.abs() + dz.abs() > RADIUS {
                continue;
            }
            let x = position.x.checked_add(dx)?;
            let z = position.z.checked_add(dz)?;
            let source = source_y(x, z)?;
            if !valid_source(source)
                || ((x, z) == (position.x, position.z) && source != current_source)
            {
                return None;
            }
            let chunk_pos = ChunkPos {
                x: x.div_euclid(16),
                z: z.div_euclid(16),
            };
            let chunk = world.chunks.get(&chunk_pos)?;
            if chunk.status.as_deref() != Some("minecraft:full") || !chunk.block_light_correct {
                return None;
            }
            let local_xz = x.rem_euclid(16) as usize + z.rem_euclid(16) as usize * 16;
            let mut cached_y = i32::MIN;
            let mut section = None;
            for y in min_y..=max_y {
                if !inside(dx, y, dz) {
                    continue;
                }
                let section_y = y.div_euclid(16);
                if section_y != cached_y {
                    section = Some(chunk.sections.get(&i8::try_from(section_y).ok()?)?);
                    cached_y = section_y;
                }
                let section = section.as_ref()?;
                let local = local_xz + y.rem_euclid(16) as usize * 256;
                let byte = section.sky_light.as_ref()?[local / 2];
                let previous = if local.is_multiple_of(2) {
                    byte & 15
                } else {
                    byte >> 4
                };
                let behavior = if build_range.contains(&y) {
                    let id = *section.blocks.as_ref()?.get(local)?;
                    behavior_for_id(world, id, &mut ids, &mut behaviors)?
                } else {
                    0
                };
                cells[index(dx, y, dz)] = Some(Cell {
                    behavior,
                    previous,
                    value: if y >= source { 15 } else { 0 },
                });
            }
        }
    }
    for dx in -RADIUS..=RADIUS {
        for dz in -RADIUS..=RADIUS {
            for y in min_y..=max_y {
                if !inside(dx, y, dz) {
                    continue;
                }
                let distance_y = if y < low {
                    low - y
                } else if y > high {
                    y - high
                } else {
                    0
                };
                if dx.abs() + dz.abs() + distance_y < RADIUS && y > min_y && y < max_y {
                    continue;
                }
                let i = index(dx, y, dz);
                let mut cell = cells[i]?;
                let p = offset(position, (dx, y.checked_sub(position.y)?, dz))?;
                for (d, direction) in DIRECTIONS.into_iter().enumerate() {
                    let (nx, ny, nz) = (dx + direction.0, y + direction.1, dz + direction.2);
                    if inside(nx, ny, nz) {
                        continue;
                    }
                    if ny < light_range.start {
                        continue;
                    }
                    let next = offset(p, direction)?;
                    let (other, light) = if ny >= light_range.end {
                        (0, 15)
                    } else {
                        (
                            behavior_at(world, bounds, next, &mut ids, &mut behaviors)?,
                            stored_light(world, next)?,
                        )
                    };
                    if !occluded(&behaviors, cell.behavior, other, d) {
                        cell.value = cell
                            .value
                            .max(light.saturating_sub(behaviors[cell.behavior].opacity.max(1)));
                    }
                }
                cells[i] = Some(cell);
            }
        }
    }
    // Most direct-sky cells are surrounded by equally bright sky. Only seed
    // cells that can improve a neighbor; this avoids queuing entire columns.
    let mut queue = BinaryHeap::new();
    for (i, cell) in cells.iter().enumerate() {
        let Some(cell) = cell else { continue };
        if cell.value <= 1 {
            continue;
        }
        let col = i / height;
        let dx = (col / side) as i32 - RADIUS;
        let dz = (col % side) as i32 - RADIUS;
        let y = min_y + (i % height) as i32;
        let useful = DIRECTIONS.into_iter().enumerate().any(|(d, direction)| {
            let (nx, ny, nz) = (dx + direction.0, y + direction.1, dz + direction.2);
            if !inside(nx, ny, nz) {
                return false;
            }
            let next = cells[index(nx, ny, nz)].expect("inside cells were initialized");
            cell.value
                .saturating_sub(behaviors[next.behavior].opacity.max(1))
                > next.value
                && !occluded(&behaviors, cell.behavior, next.behavior, d)
        });
        if useful {
            queue.push((cell.value, i));
        }
    }
    while let Some((level, i)) = queue.pop() {
        let cell = cells[i]?;
        if cell.value != level {
            continue;
        }
        let col = i / height;
        let dx = (col / side) as i32 - RADIUS;
        let dz = (col % side) as i32 - RADIUS;
        let y = min_y + (i % height) as i32;
        for (d, direction) in DIRECTIONS.into_iter().enumerate() {
            let (nx, ny, nz) = (dx + direction.0, y + direction.1, dz + direction.2);
            if !inside(nx, ny, nz) {
                continue;
            }
            let j = index(nx, ny, nz);
            let mut next = cells[j]?;
            if occluded(&behaviors, cell.behavior, next.behavior, d) {
                continue;
            }
            let value = level.saturating_sub(behaviors[next.behavior].opacity.max(1));
            if value > next.value {
                next.value = value;
                cells[j] = Some(next);
                queue.push((value, j));
            }
        }
    }
    // No writes happen until every needed input has been checked.
    let mut changed = BTreeSet::new();
    for dx in -RADIUS..=RADIUS {
        for dz in -RADIUS..=RADIUS {
            for y in min_y..=max_y {
                if !inside(dx, y, dz) {
                    continue;
                }
                let cell = cells[index(dx, y, dz)]?;
                if cell.previous == cell.value {
                    continue;
                }
                let p = offset(position, (dx, y.checked_sub(position.y)?, dz))?;
                let chunk = Arc::make_mut(world.chunks.get_mut(&p.chunk())?);
                let section = Arc::make_mut(
                    chunk
                        .sections
                        .get_mut(&i8::try_from(y.div_euclid(16)).ok()?)?,
                );
                let data = section.sky_light.as_mut()?;
                let local = p.x.rem_euclid(16) as usize
                    + p.z.rem_euclid(16) as usize * 16
                    + y.rem_euclid(16) as usize * 256;
                let byte = &mut data[local / 2];
                if local.is_multiple_of(2) {
                    *byte = (*byte & 0xf0) | cell.value;
                } else {
                    *byte = (*byte & 0x0f) | (cell.value << 4);
                }
                changed.insert(p.chunk());
            }
        }
    }
    Some(changed)
}

/// One edit and the direct-sky height immediately before and after that edit.
#[derive(Clone, Copy, Debug)]
pub struct SkyChange {
    pub position: BlockPosition,
    pub previous_source: i32,
    pub current_source: i32,
}

#[derive(Clone, Copy)]
struct Region {
    x: i32,
    z: i32,
    low: i32,
    high: i32,
}

/// Recompute the union of pending changes without using stale light from an
/// overlapping change as a boundary. Changes in a column must be chronological;
/// `source_y` must describe the world after every listed edit. Missing data,
/// inconsistent source history or an oversized allocation leaves all light
/// unchanged. A single edit uses the optimized single-change implementation.
pub fn relight_changes(
    world: &mut World,
    bounds: LightBounds,
    changes: &[SkyChange],
    mut source_y: impl FnMut(i32, i32) -> Option<i32>,
) -> Option<BTreeSet<ChunkPos>> {
    relight_changes_inner(world, bounds, changes, &mut source_y)
}

fn relight_changes_inner(
    world: &mut World,
    bounds: LightBounds,
    changes: &[SkyChange],
    source_y: &mut dyn FnMut(i32, i32) -> Option<i32>,
) -> Option<BTreeSet<ChunkPos>> {
    if changes.is_empty() {
        return Some(BTreeSet::new());
    }
    if let [change] = changes {
        return relight_change(
            world,
            bounds,
            change.position,
            change.previous_source,
            change.current_source,
            source_y,
        );
    }
    let build = bounds.build_range();
    let light = bounds.light_range();
    let valid = |y| y == UNBOUNDED_SKY_SOURCE || (build.start..=build.end).contains(&y);
    let normalize = |y: i32| {
        if y == UNBOUNDED_SKY_SOURCE {
            light.start
        } else {
            y.clamp(light.start, light.end - 1)
        }
    };
    let mut regions = Vec::with_capacity(changes.len());
    let mut expected_sources = BTreeMap::new();
    let (mut min_x, mut max_x, mut min_z, mut max_z, mut min_y, mut max_y) =
        (i32::MAX, i32::MIN, i32::MAX, i32::MIN, i32::MAX, i32::MIN);
    for change in changes {
        let p = change.position;
        if !build.contains(&p.y) || !valid(change.previous_source) || !valid(change.current_source)
        {
            return None;
        }
        if let Some(previous) = expected_sources.insert((p.x, p.z), change.current_source)
            && previous != change.previous_source
        {
            return None;
        }
        let (low, high) = if change.previous_source == change.current_source {
            (p.y, p.y)
        } else {
            (
                normalize(change.previous_source)
                    .min(normalize(change.current_source))
                    .min(p.y),
                normalize(change.previous_source)
                    .max(normalize(change.current_source))
                    .max(p.y),
            )
        };
        regions.push(Region {
            x: p.x,
            z: p.z,
            low,
            high,
        });
        min_x = min_x.min(p.x.checked_sub(RADIUS)?);
        max_x = max_x.max(p.x.checked_add(RADIUS)?);
        min_z = min_z.min(p.z.checked_sub(RADIUS)?);
        max_z = max_z.max(p.z.checked_add(RADIUS)?);
        min_y = min_y.min(low.saturating_sub(RADIUS).max(light.start));
        max_y = max_y.max(high.saturating_add(RADIUS).min(light.end - 1));
    }
    let x_len = usize::try_from(i64::from(max_x) - i64::from(min_x) + 1).ok()?;
    let z_len = usize::try_from(i64::from(max_z) - i64::from(min_z) + 1).ok()?;
    let height = usize::try_from(max_y - min_y + 1).ok()?;
    let length = x_len.checked_mul(z_len).and_then(|n| n.checked_mul(height));
    // A sparse bounding box can be very expensive even below the hard limit.
    // Only consider splitting when its volume is substantially larger than
    // the sum of each region's individual bounding box.
    let individual_volume = regions.iter().fold(0usize, |total, region| {
        let lo = region.low.saturating_sub(RADIUS).max(light.start);
        let hi = region.high.saturating_add(RADIUS).min(light.end - 1);
        let height = (hi - lo + 1) as usize;
        total.saturating_add(((2 * RADIUS + 1) as usize).pow(2).saturating_mul(height))
    });
    let should_split = length.is_none_or(|n| n > 4_000_000)
        || (changes.len() <= 512
            && length.is_some_and(|n| n > 200_000 && n > individual_volume.saturating_mul(2)));
    if should_split {
        // Disconnected groups cannot affect each other's boundary values.
        // Stage them on a clone so failure in a later group remains atomic.
        let mut seen = vec![false; changes.len()];
        let mut groups = Vec::new();
        for first in 0..changes.len() {
            if seen[first] {
                continue;
            }
            seen[first] = true;
            let mut group = vec![first];
            let mut cursor = 0;
            while cursor < group.len() {
                let a = regions[group[cursor]];
                for (i, b) in regions.iter().copied().enumerate() {
                    if seen[i] {
                        continue;
                    }
                    let dy = if a.high < b.low {
                        i64::from(b.low) - i64::from(a.high)
                    } else if b.high < a.low {
                        i64::from(a.low) - i64::from(b.high)
                    } else {
                        0
                    };
                    let distance = (i64::from(a.x) - i64::from(b.x)).abs()
                        + (i64::from(a.z) - i64::from(b.z)).abs()
                        + dy;
                    // Keep a column's source-height history together, even
                    // when its vertical influence regions do not intersect.
                    if (a.x, a.z) == (b.x, b.z) || distance <= i64::from(2 * RADIUS + 1) {
                        seen[i] = true;
                        group.push(i);
                    }
                }
                cursor += 1;
            }
            group.sort_unstable();
            groups.push(group);
        }
        if groups.len() > 1 {
            let mut staged = world.clone();
            let mut changed = BTreeSet::new();
            for group in groups {
                let group: Vec<_> = group.into_iter().map(|i| changes[i]).collect();
                changed.extend(relight_changes_inner(
                    &mut staged,
                    bounds,
                    &group,
                    source_y,
                )?);
            }
            *world = staged;
            return Some(changed);
        }
    }
    let length = length.filter(|&n| n <= 4_000_000)?;
    let mut cells = vec![None; length];
    let mut active = Vec::new();
    let index = |x: i32, y: i32, z: i32| -> Option<usize> {
        if x < min_x || x > max_x || z < min_z || z > max_z || y < min_y || y > max_y {
            return None;
        }
        let dx = usize::try_from(i64::from(x) - i64::from(min_x)).ok()?;
        let dz = usize::try_from(i64::from(z) - i64::from(min_z)).ok()?;
        Some((dx * z_len + dz) * height + usize::try_from(y - min_y).ok()?)
    };
    let position = |i: usize| BlockPosition {
        x: (i64::from(min_x) + (i / height / z_len) as i64) as i32,
        y: min_y + (i % height) as i32,
        z: (i64::from(min_z) + (i / height % z_len) as i64) as i32,
    };
    let mut ids = BTreeMap::new();
    let mut behaviors = vec![transparent_light_properties()];
    for x in min_x..=max_x {
        for z in min_z..=max_z {
            let mut intervals = Vec::new();
            for region in &regions {
                let distance = (i64::from(x) - i64::from(region.x)).abs()
                    + (i64::from(z) - i64::from(region.z)).abs();
                if distance > i64::from(RADIUS) {
                    continue;
                }
                let remaining = RADIUS - distance as i32;
                intervals.push((
                    region.low.saturating_sub(remaining).max(light.start),
                    region.high.saturating_add(remaining).min(light.end - 1),
                ));
            }
            if intervals.is_empty() {
                continue;
            }
            intervals.sort_unstable();
            let mut merged: Vec<(i32, i32)> = Vec::new();
            for (lo, hi) in intervals {
                if let Some(last) = merged.last_mut()
                    && lo <= last.1 + 1
                {
                    last.1 = last.1.max(hi);
                } else {
                    merged.push((lo, hi));
                }
            }
            let source = source_y(x, z)?;
            if !valid(source)
                || expected_sources
                    .get(&(x, z))
                    .is_some_and(|&expected| expected != source)
            {
                return None;
            }
            let chunk = world.chunks.get(&ChunkPos {
                x: x.div_euclid(16),
                z: z.div_euclid(16),
            })?;
            if chunk.status.as_deref() != Some("minecraft:full") || !chunk.block_light_correct {
                return None;
            }
            let local_xz = x.rem_euclid(16) as usize + z.rem_euclid(16) as usize * 16;
            let mut cached_y = i32::MIN;
            let mut section = None;
            for (lo, hi) in merged {
                for y in lo..=hi {
                    let section_y = y.div_euclid(16);
                    if section_y != cached_y {
                        section = Some(chunk.sections.get(&i8::try_from(section_y).ok()?)?);
                        cached_y = section_y;
                    }
                    let section = section.as_ref()?;
                    let local = local_xz + y.rem_euclid(16) as usize * 256;
                    let byte = section.sky_light.as_ref()?[local / 2];
                    let previous = if local.is_multiple_of(2) {
                        byte & 15
                    } else {
                        byte >> 4
                    };
                    let behavior = if build.contains(&y) {
                        behavior_for_id(
                            world,
                            *section.blocks.as_ref()?.get(local)?,
                            &mut ids,
                            &mut behaviors,
                        )?
                    } else {
                        0
                    };
                    let i = index(x, y, z)?;
                    cells[i] = Some(Cell {
                        behavior,
                        previous,
                        value: if y >= source { 15 } else { 0 },
                    });
                    active.push(i);
                }
            }
        }
    }
    for &i in &active {
        let p = position(i);
        let mut cell = cells[i]?;
        for (d, direction) in DIRECTIONS.into_iter().enumerate() {
            let next = offset(p, direction)?;
            if index(next.x, next.y, next.z).is_some_and(|j| cells[j].is_some()) {
                continue;
            }
            if next.y < light.start {
                continue;
            }
            let (other, boundary) = if next.y >= light.end {
                (0, 15)
            } else {
                (
                    behavior_at(world, bounds, next, &mut ids, &mut behaviors)?,
                    stored_light(world, next)?,
                )
            };
            if !occluded(&behaviors, cell.behavior, other, d) {
                cell.value = cell
                    .value
                    .max(boundary.saturating_sub(behaviors[cell.behavior].opacity.max(1)));
            }
        }
        cells[i] = Some(cell);
    }
    let mut queue = BinaryHeap::new();
    for &i in &active {
        let cell = cells[i]?;
        if cell.value <= 1 {
            continue;
        }
        let p = position(i);
        let mut useful = false;
        for (d, direction) in DIRECTIONS.into_iter().enumerate() {
            let next = offset(p, direction)?;
            let Some(neighbor) = index(next.x, next.y, next.z).and_then(|j| cells[j]) else {
                continue;
            };
            if cell
                .value
                .saturating_sub(behaviors[neighbor.behavior].opacity.max(1))
                > neighbor.value
                && !occluded(&behaviors, cell.behavior, neighbor.behavior, d)
            {
                useful = true;
                break;
            }
        }
        if useful {
            queue.push((cell.value, i));
        }
    }
    while let Some((level, i)) = queue.pop() {
        let cell = cells[i]?;
        if cell.value != level {
            continue;
        }
        let p = position(i);
        for (d, direction) in DIRECTIONS.into_iter().enumerate() {
            let next = offset(p, direction)?;
            let Some(j) = index(next.x, next.y, next.z) else {
                continue;
            };
            let Some(mut other) = cells[j] else {
                continue;
            };
            if occluded(&behaviors, cell.behavior, other.behavior, d) {
                continue;
            }
            let value = level.saturating_sub(behaviors[other.behavior].opacity.max(1));
            if value > other.value {
                other.value = value;
                cells[j] = Some(other);
                queue.push((value, j));
            }
        }
    }
    let mut changed = BTreeSet::new();
    for i in active {
        let cell = cells[i]?;
        if cell.value == cell.previous {
            continue;
        }
        let p = position(i);
        let chunk = Arc::make_mut(world.chunks.get_mut(&p.chunk())?);
        let section = Arc::make_mut(
            chunk
                .sections
                .get_mut(&i8::try_from(p.y.div_euclid(16)).ok()?)?,
        );
        let data = section.sky_light.as_mut()?;
        let local = p.x.rem_euclid(16) as usize
            + p.z.rem_euclid(16) as usize * 16
            + p.y.rem_euclid(16) as usize * 256;
        let byte = &mut data[local / 2];
        if local.is_multiple_of(2) {
            *byte = (*byte & 0xf0) | cell.value;
        } else {
            *byte = (*byte & 0x0f) | (cell.value << 4);
        }
        changed.insert(p.chunk());
    }
    Some(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        loaded::{BLOCK_COUNT, BlockState, Chunk, LIGHT_BYTES, Section},
        sky_light::SkyColumn,
    };

    fn state(name: &str, properties: &[(&str, &str)]) -> BlockState {
        BlockState {
            name: format!("minecraft:{name}"),
            properties: properties
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
        }
    }
    fn fixture() -> World {
        fixture_range(0, 32)
    }
    fn fixture_range(min_y: i32, max_y: i32) -> World {
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(state("air", &[]));
        for x in -1..=2 {
            for z in -1..=2 {
                let position = ChunkPos { x, z };
                let sections = (min_y.div_euclid(16) - 1..=max_y.div_euclid(16))
                    .map(|y| {
                        (
                            i8::try_from(y).unwrap(),
                            Arc::new(Section {
                                blocks: (min_y.div_euclid(16)..max_y.div_euclid(16))
                                    .contains(&y)
                                    .then(|| Box::new([air; BLOCK_COUNT])),
                                biomes: None,
                                block_light: None,
                                sky_light: Some(Box::new([0xff; LIGHT_BYTES])),
                            }),
                        )
                    })
                    .collect();
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
        world
    }

    #[test]
    fn edit_relights_source_column_and_preserves_snapshots() {
        let mut world = fixture();
        let bounds = LightBounds::new(0, 32).unwrap();
        let mut column = SkyColumn::load(&world, bounds, 15, 15).unwrap();
        let previous = column.lowest_source_y();
        let stone = world.intern_state(state("stone", &[]));
        world.set_block_id(15, 15, 15, stone).unwrap();
        let before = world.clone();
        column.update(&world, 15).unwrap();
        let current = column.lowest_source_y();
        let changed = relight_change(
            &mut world,
            bounds,
            BlockPosition {
                x: 15,
                y: 15,
                z: 15,
            },
            previous,
            current,
            |x, z| {
                Some(if (x, z) == (15, 15) {
                    current
                } else {
                    UNBOUNDED_SKY_SOURCE
                })
            },
        )
        .unwrap();
        assert!(changed.contains(&ChunkPos { x: 0, z: 0 }));
        assert_eq!(world.light(15, 15, 15, true), Some(0));
        assert_eq!(world.light(15, 14, 15, true), Some(14));
        assert_eq!(world.light(15, -10, 15, true), Some(14));
        assert_eq!(before.light(15, 14, 15, true), Some(15));
    }

    #[test]
    fn missing_data_does_not_mutate_light() {
        let mut world = fixture();
        let bounds = LightBounds::new(0, 32).unwrap();
        let p = BlockPosition {
            x: 15,
            y: 15,
            z: 15,
        };
        let chunk = Arc::make_mut(world.chunks.get_mut(&ChunkPos { x: 0, z: 0 }).unwrap());
        Arc::make_mut(chunk.sections.get_mut(&0).unwrap()).sky_light = None;
        let before = world.clone();
        assert!(
            relight_change(
                &mut world,
                bounds,
                p,
                UNBOUNDED_SKY_SOURCE,
                16,
                |x, z| Some(if (x, z) == (15, 15) {
                    16
                } else {
                    UNBOUNDED_SKY_SOURCE
                })
            )
            .is_none()
        );
        for (pos, chunk) in &world.chunks {
            assert!(Arc::ptr_eq(chunk, &before.chunks[pos]));
        }
    }

    #[test]
    fn overlapping_changes_are_recomputed_together() {
        let bounds = LightBounds::new(0, 32).unwrap();
        let mut sequential = fixture();
        let mut batch = sequential.clone();
        let sequential_stone = sequential.intern_state(state("stone", &[]));
        let batch_stone = batch.intern_state(state("stone", &[]));
        let mut sources = BTreeMap::new();
        let mut changes = Vec::new();
        for x in [14, 16] {
            let mut col = SkyColumn::load(&sequential, bounds, x, 15).unwrap();
            let old = col.lowest_source_y();
            sequential
                .set_block_id(x, 15, 15, sequential_stone)
                .unwrap();
            batch.set_block_id(x, 15, 15, batch_stone).unwrap();
            col.update(&sequential, 15).unwrap();
            let current = col.lowest_source_y();
            sources.insert((x, 15), current);
            let position = BlockPosition { x, y: 15, z: 15 };
            relight_change(&mut sequential, bounds, position, old, current, |x, z| {
                Some(*sources.get(&(x, z)).unwrap_or(&UNBOUNDED_SKY_SOURCE))
            })
            .unwrap();
            changes.push(SkyChange {
                position,
                previous_source: old,
                current_source: current,
            });
        }
        let old_snapshot = batch.clone();
        relight_changes(&mut batch, bounds, &changes, |x, z| {
            Some(*sources.get(&(x, z)).unwrap_or(&UNBOUNDED_SKY_SOURCE))
        })
        .unwrap();
        for y in -15..47 {
            for z in 0..31 {
                for x in 0..31 {
                    assert_eq!(
                        batch.light(x, y, z, true),
                        sequential.light(x, y, z, true),
                        "at {x},{y},{z}"
                    );
                }
            }
        }
        assert_eq!(old_snapshot.light(14, 14, 15, true), Some(15));
        assert_eq!(batch.light(14, 14, 15, true), Some(14));
    }

    #[test]
    fn batch_rejects_missing_and_inconsistent_inputs_atomically() {
        let bounds = LightBounds::new(0, 32).unwrap();
        let mut world = fixture();
        let p = BlockPosition {
            x: 15,
            y: 15,
            z: 15,
        };
        let changes = [
            SkyChange {
                position: p,
                previous_source: UNBOUNDED_SKY_SOURCE,
                current_source: 16,
            },
            SkyChange {
                position: p,
                previous_source: 17,
                current_source: UNBOUNDED_SKY_SOURCE,
            },
        ];
        let before = world.clone();
        assert!(
            relight_changes(&mut world, bounds, &changes, |_, _| Some(
                UNBOUNDED_SKY_SOURCE
            ))
            .is_none()
        );
        for (pos, chunk) in &world.chunks {
            assert!(Arc::ptr_eq(chunk, &before.chunks[pos]));
        }
        let chunk = Arc::make_mut(world.chunks.get_mut(&ChunkPos { x: 0, z: 0 }).unwrap());
        Arc::make_mut(chunk.sections.get_mut(&0).unwrap()).sky_light = None;
        let before = world.clone();
        let changes = [
            changes[0],
            SkyChange {
                position: BlockPosition { x: 16, ..p },
                previous_source: UNBOUNDED_SKY_SOURCE,
                current_source: 16,
            },
        ];
        assert!(
            relight_changes(&mut world, bounds, &changes, |x, z| Some(
                if z == 15 && (x == 15 || x == 16) {
                    16
                } else {
                    UNBOUNDED_SKY_SOURCE
                }
            ))
            .is_none()
        );
        for (pos, chunk) in &world.chunks {
            assert!(Arc::ptr_eq(chunk, &before.chunks[pos]));
        }
    }

    #[test]
    fn distant_groups_are_staged_atomically_without_a_huge_bounding_box() {
        let bounds = LightBounds::new(-64, 320).unwrap();
        let mut world = fixture_range(-64, 320);
        let duplicate: Vec<_> = world
            .chunks
            .iter()
            .map(|(pos, chunk)| {
                let target = ChunkPos {
                    x: pos.x + 25,
                    z: pos.z,
                };
                let mut chunk = (**chunk).clone();
                chunk.position = target;
                (target, Arc::new(chunk))
            })
            .collect();
        world.chunks.extend(duplicate);
        let stone = world.intern_state(state("stone", &[]));
        let mut changes = Vec::new();
        for x in [15, 415] {
            let mut source = SkyColumn::load(&world, bounds, x, 15).unwrap();
            let previous_source = source.lowest_source_y();
            world.set_block_id(x, 319, 15, stone).unwrap();
            source.update(&world, 319).unwrap();
            changes.push(SkyChange {
                position: BlockPosition { x, y: 319, z: 15 },
                previous_source,
                current_source: source.lowest_source_y(),
            });
        }
        let sources = |x, z| {
            Some(if z == 15 && (x == 15 || x == 415) {
                320
            } else {
                UNBOUNDED_SKY_SOURCE
            })
        };
        let mut incomplete = world.clone();
        let chunk = Arc::make_mut(
            incomplete
                .chunks
                .get_mut(&ChunkPos { x: 25, z: 0 })
                .unwrap(),
        );
        Arc::make_mut(chunk.sections.get_mut(&19).unwrap()).sky_light = None;
        let before = incomplete.clone();
        assert!(relight_changes(&mut incomplete, bounds, &changes, sources).is_none());
        for (pos, chunk) in &incomplete.chunks {
            assert!(Arc::ptr_eq(chunk, &before.chunks[pos]));
        }
        let changed = relight_changes(&mut world, bounds, &changes, sources).unwrap();
        assert!(changed.contains(&ChunkPos { x: 0, z: 0 }));
        assert!(changed.contains(&ChunkPos { x: 25, z: 0 }));
        assert_eq!(world.light(15, 318, 15, true), Some(14));
        assert_eq!(world.light(415, 318, 15, true), Some(14));
    }

    #[test]
    fn sparse_changes_match_individual_updates_and_remain_atomic() {
        let bounds = LightBounds::new(0, 32).unwrap();
        let mut world = fixture();
        let duplicate: Vec<_> = world
            .chunks
            .iter()
            .map(|(pos, chunk)| {
                let target = ChunkPos {
                    x: pos.x + 8,
                    z: pos.z,
                };
                let mut copy = (**chunk).clone();
                copy.position = target;
                (target, Arc::new(copy))
            })
            .collect();
        world.chunks.extend(duplicate);
        let stone = world.intern_state(state("stone", &[]));
        let changes: Vec<_> = [15, 143]
            .into_iter()
            .map(|x| {
                world.set_block_id(x, 31, 15, stone).unwrap();
                SkyChange {
                    position: BlockPosition { x, y: 31, z: 15 },
                    previous_source: UNBOUNDED_SKY_SOURCE,
                    current_source: 32,
                }
            })
            .collect();
        let sources = |x, z| {
            Some(if z == 15 && (x == 15 || x == 143) {
                32
            } else {
                UNBOUNDED_SKY_SOURCE
            })
        };
        let mut expected = world.clone();
        for change in &changes {
            relight_change(
                &mut expected,
                bounds,
                change.position,
                change.previous_source,
                change.current_source,
                sources,
            )
            .unwrap();
        }
        let mut incomplete = world.clone();
        let chunk = Arc::make_mut(incomplete.chunks.get_mut(&ChunkPos { x: 8, z: 0 }).unwrap());
        Arc::make_mut(chunk.sections.get_mut(&1).unwrap()).sky_light = None;
        let before = incomplete.clone();
        assert!(relight_changes(&mut incomplete, bounds, &changes, sources).is_none());
        for (pos, chunk) in &incomplete.chunks {
            assert!(Arc::ptr_eq(chunk, &before.chunks[pos]));
        }
        relight_changes(&mut world, bounds, &changes, sources).unwrap();
        for (pos, chunk) in &world.chunks {
            for (y, section) in &chunk.sections {
                assert_eq!(
                    section.sky_light, expected.chunks[pos].sections[y].sky_light,
                    "chunk {pos:?}, section {y}"
                );
            }
        }
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn propagation_matches_java_sky_engine() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft")
            });
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
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
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("SkyLightEngineOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/SkyLightEngineOracle.java"),
        )
        .unwrap();
        let run = |cmd: &mut Command| {
            let out = cmd.output().unwrap();
            assert!(
                out.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-encoding")
            .arg("UTF-8")
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let mut actions = vec![
            (15, 15, 15, "stone"),
            (15, 15, 15, "air"),
            (15, 15, 15, "water"),
            (15, 15, 15, "air"),
            (15, 15, 15, "slab_bottom"),
            (15, 15, 15, "slab_top"),
            (15, 15, 15, "air"),
            (16, 15, 15, "stone"),
            (15, 15, 15, "stone"),
            (16, 15, 15, "glass"),
            (15, 15, 15, "air"),
            (16, 15, 15, "air"),
        ];
        for x in 13..=17 {
            for z in 13..=17 {
                actions.push((x, 24, z, "stone"));
            }
        }
        actions.extend([
            (15, 20, 15, "water"),
            (15, 24, 15, "air"),
            (15, 24, 15, "slab_bottom"),
            (15, 24, 15, "slab_top"),
            (15, 24, 15, "glass"),
            (15, 20, 15, "air"),
            (15, 0, 15, "stone"),
            (15, 0, 15, "air"),
            (16, 31, 15, "water"),
            (16, 31, 15, "air"),
            (15, 24, 15, "wet_slab"),
            (15, 24, 15, "stair"),
            (15, 24, 15, "snow_1"),
            (15, 24, 15, "snow_7"),
            (15, 24, 15, "snow_8"),
            (15, 24, 15, "air"),
        ]);
        let entries: Vec<_> = actions
            .iter()
            .map(|(x, y, z, name)| serde_json::json!({"x":x,"y":y,"z":z,"state":name}))
            .collect();
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(&input, serde_json::to_vec(&entries).unwrap()).unwrap();
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("SkyLightEngineOracle")
            .arg(&input)
            .arg(&output));
        let expected: Vec<Vec<u8>> =
            serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
        let full_actions = vec![
            (15, 319, 15, "stone"),
            (15, 319, 15, "air"),
            (15, -64, 15, "slab_bottom"),
            (15, -64, 15, "air"),
            (16, -64, 15, "stone"),
            (16, -64, 15, "air"),
        ];
        let entries: Vec<_> = full_actions
            .iter()
            .map(|(x, y, z, name)| serde_json::json!({"x":x,"y":y,"z":z,"state":name}))
            .collect();
        std::fs::write(&input, serde_json::to_vec(&entries).unwrap()).unwrap();
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("SkyLightEngineOracle")
            .arg(&input)
            .arg(&output)
            .arg("-64")
            .arg("384"));
        let full_expected: Vec<Vec<u8>> =
            serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
        for (min_y, max_y, actions, expected) in [
            (0, 32, actions, expected),
            (-64, 320, full_actions, full_expected),
        ] {
            assert_eq!(expected.len(), actions.len());
            let bounds = LightBounds::new(min_y, max_y).unwrap();
            let mut world = fixture_range(min_y, max_y);
            let mut columns = BTreeMap::new();
            for ((x, y, z, name), expected) in actions.into_iter().zip(expected) {
                let col = columns
                    .entry((x, z))
                    .or_insert_with(|| SkyColumn::load(&world, bounds, x, z).unwrap());
                let previous = col.lowest_source_y();
                let block = match name {
                    "slab_bottom" => state(
                        "stone_slab",
                        &[("type", "bottom"), ("waterlogged", "false")],
                    ),
                    "slab_top" => state("stone_slab", &[("type", "top"), ("waterlogged", "false")]),
                    "wet_slab" => {
                        state("stone_slab", &[("type", "bottom"), ("waterlogged", "true")])
                    }
                    "stair" => state(
                        "oak_stairs",
                        &[
                            ("facing", "north"),
                            ("half", "bottom"),
                            ("shape", "straight"),
                            ("waterlogged", "false"),
                        ],
                    ),
                    "snow_1" => state("snow", &[("layers", "1")]),
                    "snow_7" => state("snow", &[("layers", "7")]),
                    "snow_8" => state("snow", &[("layers", "8")]),
                    "water" => state("water", &[("level", "0")]),
                    _ => state(name, &[]),
                };
                let id = world.intern_state(block);
                world.set_block_id(x, y, z, id).unwrap();
                col.update(&world, y).unwrap();
                let current = col.lowest_source_y();
                relight_change(
                    &mut world,
                    bounds,
                    BlockPosition { x, y, z },
                    previous,
                    current,
                    |x, z| {
                        Some(
                            columns
                                .get(&(x, z))
                                .map_or(UNBOUNDED_SKY_SOURCE, |c| c.lowest_source_y()),
                        )
                    },
                )
                .unwrap();
                for y in min_y - 15..max_y + 15 {
                    for z in 0..31 {
                        for x in 0..31 {
                            let index = ((y - min_y + 15) * 31 * 31 + z * 31 + x) as usize;
                            assert_eq!(
                                world.light(x, y, z, true).unwrap(),
                                expected[index],
                                "after {name}, at {x},{y},{z}"
                            );
                        }
                    }
                }
            }
        }
        let mut actions = Vec::new();
        for x in 13..=17 {
            for z in 13..=17 {
                actions.push((x, 24, z, "stone", true));
            }
        }
        actions.last_mut().unwrap().4 = false;
        actions.extend([
            (14, 24, 15, "air", true),
            (16, 24, 15, "air", false),
            (14, 24, 15, "water", true),
            (16, 24, 15, "slab_bottom", false),
            (15, 24, 15, "air", true),
            (15, 24, 15, "stone", true),
            (15, 24, 15, "air", false),
            (14, 24, 15, "air", true),
            (16, 24, 15, "air", false),
        ]);
        let entries: Vec<_> = actions.iter().map(|(x,y,z,name,defer)| serde_json::json!({"x":x,"y":y,"z":z,"state":name,"defer":defer})).collect();
        std::fs::write(&input, serde_json::to_vec(&entries).unwrap()).unwrap();
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("SkyLightEngineOracle")
            .arg(&input)
            .arg(&output));
        let expected: Vec<Vec<u8>> =
            serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
        assert_eq!(
            expected.len(),
            actions.iter().filter(|action| !action.4).count()
        );
        let bounds = LightBounds::new(0, 32).unwrap();
        let mut world = fixture();
        let mut columns = BTreeMap::new();
        let mut pending = Vec::new();
        let mut expected = expected.into_iter();
        for (x, y, z, name, defer) in actions {
            let col = columns
                .entry((x, z))
                .or_insert_with(|| SkyColumn::load(&world, bounds, x, z).unwrap());
            let previous_source = col.lowest_source_y();
            let block = match name {
                "water" => state("water", &[("level", "0")]),
                "slab_bottom" => state(
                    "stone_slab",
                    &[("type", "bottom"), ("waterlogged", "false")],
                ),
                _ => state(name, &[]),
            };
            let id = world.intern_state(block);
            world.set_block_id(x, y, z, id).unwrap();
            col.update(&world, y).unwrap();
            pending.push(SkyChange {
                position: BlockPosition { x, y, z },
                previous_source,
                current_source: col.lowest_source_y(),
            });
            if defer {
                continue;
            }
            relight_changes(&mut world, bounds, &pending, |x, z| {
                Some(
                    columns
                        .get(&(x, z))
                        .map_or(UNBOUNDED_SKY_SOURCE, |c| c.lowest_source_y()),
                )
            })
            .unwrap();
            pending.clear();
            let expected = expected.next().unwrap();
            for y in -15..47 {
                for z in 0..31 {
                    for x in 0..31 {
                        let i = ((y + 15) * 31 * 31 + z * 31 + x) as usize;
                        assert_eq!(
                            world.light(x, y, z, true).unwrap(),
                            expected[i],
                            "after batched {name}, at {x},{y},{z}"
                        );
                    }
                }
            }
        }
        assert!(pending.is_empty());
        assert!(expected.next().is_none());
    }
}
