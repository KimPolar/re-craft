//! Block-light initialization for a generated chunk and its complete halo.
//!
//! This computes light values, not the light engine's allocation history or
//! chunk status. The caller must provide the final feature-stage blocks in the
//! target and all eight neighbors, from one consistent generation revision.
use crate::{
    block_light::{
        DIRECTIONS, LightBounds, LightProperties, faces_occlude, light_properties,
        transparent_light_properties,
    },
    generation_light::{VisibleLayers, VisibleLightSnapshot, order_pending_light_checks},
    loaded::{ChunkPos, LIGHT_BYTES, World},
};
use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    sync::Arc,
};

/// Drain block-light checks while preserving the prior stored light revision.
/// `positions` are checkBlock calls in insertion order, including duplicates.
/// `world` must contain all final block states, `active` all source-enabled
/// chunks, and `before` complete columns for every possible affected location.
/// Section allocation cannot change during this batch. Missing or unsupported
/// input returns None without changing the original snapshot.
pub fn check_generated_block_light(
    world: &World,
    bounds: LightBounds,
    before: &VisibleLightSnapshot,
    active: &BTreeSet<ChunkPos>,
    positions: &[[i32; 3]],
) -> Option<VisibleLightSnapshot> {
    apply_generated_block_light(world, bounds, before, active, positions, &[])
}

/// Propagate the block sources of one newly enabled generated chunk over a
/// complete prior storage revision. No pending checks or allocation updates
/// may remain; unsupported blocks or missing storage leave `before` intact.
pub fn activate_generated_block_light(
    world: &World,
    bounds: LightBounds,
    before: &VisibleLightSnapshot,
    chunk: ChunkPos,
) -> Option<VisibleLightSnapshot> {
    if !before.has_block_channel() {
        return None;
    }
    let start_x = chunk.x.checked_mul(16)?;
    let start_z = chunk.z.checked_mul(16)?;
    let data = world.chunks.get(&chunk)?;
    let mut properties = HashMap::new();
    let mut sources = Vec::new();
    let range = bounds.build_range();
    for sy in range.start.div_euclid(16)..range.end.div_euclid(16) {
        let blocks = data
            .sections
            .get(&i8::try_from(sy).ok()?)?
            .blocks
            .as_ref()?;
        for (index, &id) in blocks.iter().enumerate() {
            let emission = if let Some(&value) = properties.get(&id.0) {
                value
            } else {
                let value = light_properties(world.state(id)?)?.emission;
                properties.insert(id.0, value);
                value
            };
            if emission > 0 {
                let x = i32::try_from(index % 16).ok()?;
                let z = i32::try_from((index / 16) % 16).ok()?;
                let y = sy
                    .checked_mul(16)?
                    .checked_add(i32::try_from(index / 256).ok()?)?;
                sources.push((
                    [start_x.checked_add(x)?, y, start_z.checked_add(z)?],
                    emission,
                ));
            }
        }
    }
    apply_generated_block_light(world, bounds, before, &BTreeSet::new(), &[], &sources)
}

fn apply_generated_block_light(
    world: &World,
    bounds: LightBounds,
    before: &VisibleLightSnapshot,
    active: &BTreeSet<ChunkPos>,
    positions: &[[i32; 3]],
    sources: &[([i32; 3], u8)],
) -> Option<VisibleLightSnapshot> {
    #[derive(Clone, Copy)]
    struct Entry {
        pos: [i32; 3],
        level: u8,
        directions: u8,
        emission: bool,
    }
    const ALL: u8 = 0b11_1111;
    const ORDER: [usize; 6] = [3, 2, 5, 4, 1, 0];
    if !before.has_block_channel() {
        return None;
    }
    let mut draft = before.clone();
    let mut properties = HashMap::<u32, LightProperties>::new();
    let get_properties = |p: [i32; 3], cache: &mut HashMap<u32, LightProperties>| {
        if !bounds.build_range().contains(&p[1]) {
            return Some(transparent_light_properties());
        }
        let id = world.block_id(p[0], p[1], p[2])?;
        if let Some(value) = cache.get(&id.0) {
            return Some(value.clone());
        }
        let value = light_properties(world.state(id)?)?;
        cache.insert(id.0, value.clone());
        Some(value)
    };
    let emission = |p: [i32; 3], cache: &mut HashMap<u32, LightProperties>| {
        let properties = get_properties(p, cache)?;
        let chunk = ChunkPos {
            x: p[0].div_euclid(16),
            z: p[2].div_euclid(16),
        };
        Some(if active.contains(&chunk) {
            properties.emission
        } else {
            0
        })
    };
    let neighbor = |p: [i32; 3], direction: usize| {
        let (dx, dy, dz) = DIRECTIONS[direction];
        Some([
            p[0].checked_add(dx)?,
            p[1].checked_add(dy)?,
            p[2].checked_add(dz)?,
        ])
    };
    let mut decreases = VecDeque::new();
    let mut increases = VecDeque::new();
    for position in order_pending_light_checks(positions.iter().copied())? {
        if !bounds.build_range().contains(&position[1]) {
            return None;
        }
        let Some(old) = draft.stored_block(position)? else {
            continue;
        };
        let source = emission(position, &mut properties)?;
        if source < old {
            draft.set_stored_block(position, 0)?;
        }
        decreases.push_back(Entry {
            pos: position,
            level: if source < old { old } else { 1 },
            directions: ALL,
            emission: false,
        });
        if source > 0 {
            increases.push_back(Entry {
                pos: position,
                level: source,
                directions: ALL,
                emission: true,
            });
        }
    }
    for &(pos, level) in sources {
        increases.push_back(Entry {
            pos,
            level,
            directions: ALL,
            emission: true,
        });
    }
    while let Some(entry) = decreases.pop_front() {
        for direction in ORDER {
            if entry.directions & (1 << direction) == 0 {
                continue;
            }
            let next = neighbor(entry.pos, direction)?;
            if !bounds.light_range().contains(&next[1]) {
                continue;
            }
            let Some(level) = draft.stored_block(next)? else {
                continue;
            };
            if level == 0 {
                continue;
            }
            let opposite = direction ^ 1;
            if level <= entry.level.saturating_sub(1) {
                let source = emission(next, &mut properties)?;
                draft.set_stored_block(next, 0)?;
                if source < level {
                    decreases.push_back(Entry {
                        pos: next,
                        level,
                        directions: ALL & !(1 << opposite),
                        emission: false,
                    });
                }
                if source > 0 {
                    increases.push_back(Entry {
                        pos: next,
                        level: source,
                        directions: ALL,
                        emission: true,
                    });
                }
            } else {
                increases.push_back(Entry {
                    pos: next,
                    level,
                    directions: 1 << opposite,
                    emission: false,
                });
            }
        }
    }
    while let Some(entry) = increases.pop_front() {
        let Some(mut level) = draft.stored_block(entry.pos)? else {
            return None;
        };
        if entry.emission && level < entry.level {
            draft.set_stored_block(entry.pos, entry.level)?;
            level = entry.level;
        }
        if level != entry.level {
            continue;
        }
        let from = get_properties(entry.pos, &mut properties)?;
        for direction in ORDER {
            if entry.directions & (1 << direction) == 0 {
                continue;
            }
            let next = neighbor(entry.pos, direction)?;
            if !bounds.light_range().contains(&next[1]) {
                continue;
            }
            let Some(old) = draft.stored_block(next)? else {
                continue;
            };
            if level.saturating_sub(1) <= old {
                continue;
            }
            let to = get_properties(next, &mut properties)?;
            let value = level.saturating_sub(to.opacity.max(1));
            if value <= old
                || ((from.has_faces || to.has_faces)
                    && faces_occlude(&from.faces[direction], &to.faces[direction ^ 1]))
            {
                continue;
            }
            draft.set_stored_block(next, value)?;
            if value > 1 {
                increases.push_back(Entry {
                    pos: next,
                    level: value,
                    directions: ALL & !(1 << (direction ^ 1)),
                    emission: false,
                });
            }
        }
    }
    Some(draft)
}

/// Compute one target chunk and its two padding sections without mutating the
/// input. Any missing chunk, section, state or unsupported light behavior
/// returns None; no partially initialized light is published.
pub fn initialize_generated_block_light(
    world: &World,
    bounds: LightBounds,
    center: ChunkPos,
) -> Option<VisibleLayers> {
    calculate_generated_block_light(world, bounds, center, None, None)
}

/// Recompute block light after only the specified chunks have propagated
/// their sources. The supplied storage snapshot must be from a fresh engine
/// before any propagation and contain the entire 3x3 area. The source set
/// must include every chunk activated since that snapshot. Blocks and section
/// allocation must remain unchanged throughout this calculation.
pub fn generated_block_light_from_active_chunks(
    world: &World,
    bounds: LightBounds,
    center: ChunkPos,
    active: &BTreeSet<ChunkPos>,
    fresh_storage: &VisibleLightSnapshot,
) -> Option<VisibleLayers> {
    calculate_generated_block_light(world, bounds, center, Some(active), Some(fresh_storage))
}

fn calculate_generated_block_light(
    world: &World,
    bounds: LightBounds,
    center: ChunkPos,
    active: Option<&BTreeSet<ChunkPos>>,
    fresh_storage: Option<&VisibleLightSnapshot>,
) -> Option<VisibleLayers> {
    const SIDE: usize = 48;
    const PLANE: usize = SIDE * SIDE;
    let light_range = bounds.light_range();
    let build_range = bounds.build_range();
    let height = usize::try_from(light_range.end.checked_sub(light_range.start)?).ok()?;
    let count = PLANE.checked_mul(height)?;
    let mut behavior = vec![0u32; count];
    let mut levels = vec![0u8; count];
    let mut allocated = vec![fresh_storage.is_none(); count];
    let mut properties: Vec<LightProperties> = vec![transparent_light_properties()];
    let mut known: HashMap<u32, u32> = HashMap::new();
    let mut queue: [Vec<usize>; 16] = std::array::from_fn(|_| Vec::new());
    for cz in 0..3usize {
        for cx in 0..3usize {
            let pos = ChunkPos {
                x: center.x.checked_add(cx as i32 - 1)?,
                z: center.z.checked_add(cz as i32 - 1)?,
            };
            let chunk = world.chunks.get(&pos)?;
            if chunk.position != pos {
                return None;
            }
            if let Some(storage) = fresh_storage {
                for section_y in light_range.start.div_euclid(16)..light_range.end.div_euclid(16) {
                    if !storage.block_section_allocated(pos, section_y)? {
                        continue;
                    }
                    let y0 = usize::try_from(section_y * 16 - light_range.start).ok()?;
                    for y in y0..y0 + 16 {
                        for z in cz * 16..cz * 16 + 16 {
                            let start = cx * 16 + z * SIDE + y * PLANE;
                            allocated[start..start + 16].fill(true);
                        }
                    }
                }
            }
            for section_y in build_range.start.div_euclid(16)..build_range.end.div_euclid(16) {
                let section = chunk.sections.get(&i8::try_from(section_y).ok()?)?;
                let blocks = section.blocks.as_ref()?;
                for (local, &id) in blocks.iter().enumerate() {
                    let index = if let Some(&index) = known.get(&id.0) {
                        index
                    } else {
                        let p = light_properties(world.state(id)?)?;
                        let index = u32::try_from(properties.len()).ok()?;
                        properties.push(p);
                        known.insert(id.0, index);
                        index
                    };
                    let x = cx * 16 + local % 16;
                    let z = cz * 16 + (local / 16) % 16;
                    let y =
                        usize::try_from(section_y * 16 + (local / 256) as i32 - light_range.start)
                            .ok()?;
                    let target = x + z * SIDE + y * PLANE;
                    behavior[target] = index;
                    let value = properties[index as usize].emission;
                    if value > 0
                        && allocated[target]
                        && active.is_none_or(|chunks| chunks.contains(&pos))
                    {
                        levels[target] = value;
                        queue[value as usize].push(target);
                    }
                }
            }
        }
    }
    // Propagation always loses at least one level. Descending buckets avoid a
    // heap and each improvement goes to a strictly lower bucket.
    for level in (1u8..=15).rev() {
        while let Some(index) = queue[level as usize].pop() {
            if levels[index] != level {
                continue;
            }
            let x = index % SIDE;
            let z = index / SIDE % SIDE;
            let y = index / PLANE;
            let from = &properties[behavior[index] as usize];
            for (direction, (dx, dy, dz)) in DIRECTIONS.into_iter().enumerate() {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                let nz = z as i32 + dz;
                if nx < 0
                    || nx >= SIDE as i32
                    || ny < 0
                    || ny >= height as i32
                    || nz < 0
                    || nz >= SIDE as i32
                {
                    continue;
                }
                let next = nx as usize + nz as usize * SIDE + ny as usize * PLANE;
                if !allocated[next] {
                    continue;
                }
                let to = &properties[behavior[next] as usize];
                let candidate = level.saturating_sub(to.opacity.max(1));
                if candidate <= levels[next] {
                    continue;
                }
                if (from.has_faces || to.has_faces)
                    && faces_occlude(&from.faces[direction], &to.faces[direction ^ 1])
                {
                    continue;
                }
                levels[next] = candidate;
                queue[candidate as usize].push(next);
            }
        }
    }
    let mut output = VisibleLayers::new();
    for sy in light_range.start.div_euclid(16)..light_range.end.div_euclid(16) {
        let mut layer = [0u8; LIGHT_BYTES];
        let y0 = usize::try_from(sy * 16 - light_range.start).ok()?;
        for y in 0..16usize {
            for z in 0..16usize {
                for x in 0..16usize {
                    let source = (x + 16) + (z + 16) * SIDE + (y0 + y) * PLANE;
                    let local = x + z * 16 + y * 256;
                    let value = levels[source];
                    if local % 2 == 0 {
                        layer[local / 2] = value;
                    } else {
                        layer[local / 2] |= value << 4;
                    }
                }
            }
        }
        output.insert(sy, Arc::new(layer));
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loaded::{BLOCK_COUNT, BlockState, Chunk, Section};
    use std::collections::BTreeMap;
    fn fixture() -> World {
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: Default::default(),
        });
        for z in -1..=1 {
            for x in -1..=1 {
                let pos = ChunkPos { x, z };
                let mut sections = BTreeMap::new();
                for sy in 0..2 {
                    sections.insert(
                        sy,
                        Arc::new(Section {
                            blocks: Some(Box::new([air; BLOCK_COUNT])),
                            biomes: None,
                            block_light: None,
                            sky_light: None,
                        }),
                    );
                }
                world.chunks.insert(
                    pos,
                    Arc::new(Chunk {
                        position: pos,
                        status: Some("minecraft:features".into()),
                        block_light_correct: false,
                        sections,
                    }),
                );
            }
        }
        world
    }
    fn value(layers: &VisibleLayers, x: usize, y: i32, z: usize) -> u8 {
        let layer = &layers[&y.div_euclid(16)];
        let i = x + z * 16 + y.rem_euclid(16) as usize * 256;
        let b = layer[i / 2];
        if i % 2 == 0 { b & 15 } else { b >> 4 }
    }
    #[test]
    fn queued_emission_add_and_remove_preserve_previous_revisions() {
        let mut world = fixture();
        let center = ChunkPos { x: 0, z: 0 };
        let bounds = LightBounds::new(0, 32).unwrap();
        let covered = (-1..=1).flat_map(|z| (-1..=1).map(move |x| ChunkPos { x, z }));
        let initial = VisibleLightSnapshot::fresh_after_section_updates(
            true,
            false,
            [(center, 0), (center, 1)],
            covered,
        )
        .unwrap();
        let torch = world.intern_state(BlockState {
            name: "minecraft:torch".into(),
            properties: Default::default(),
        });
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: Default::default(),
        });
        let position = [8, 8, 8];
        world.set_block_id(8, 8, 8, torch).unwrap();
        let active = BTreeSet::from([center]);
        let lit =
            check_generated_block_light(&world, bounds, &initial, &active, &[position, position])
                .unwrap();
        assert_eq!(initial.block_light(position), Some(0));
        assert_eq!(lit.block_light(position), Some(14));
        assert_eq!(lit.block_light([9, 8, 8]), Some(13));
        let inactive =
            check_generated_block_light(&world, bounds, &initial, &BTreeSet::new(), &[position])
                .unwrap();
        assert_eq!(inactive.block_light(position), Some(0));
        world.set_block_id(8, 8, 8, air).unwrap();
        let removed =
            check_generated_block_light(&world, bounds, &lit, &active, &[position]).unwrap();
        assert_eq!(removed.block_light(position), Some(0));
        assert_eq!(removed.block_light([9, 8, 8]), Some(0));
        assert_eq!(lit.block_light(position), Some(14));
    }
    #[test]
    fn starts_from_sources_in_neighbor_chunks_and_keeps_padding() {
        let mut world = fixture();
        let torch = world.intern_state(BlockState {
            name: "minecraft:torch".into(),
            properties: Default::default(),
        });
        world.set_block_id(-1, 8, 8, torch).unwrap();
        let result = initialize_generated_block_light(
            &world,
            LightBounds::new(0, 32).unwrap(),
            ChunkPos { x: 0, z: 0 },
        )
        .unwrap();
        assert_eq!(result.len(), 4);
        assert_eq!(value(&result, 0, 8, 8), 13);
        assert_eq!(value(&result, 12, 8, 8), 1);
        assert_eq!(value(&result, 13, 8, 8), 0);
        assert_eq!(value(&result, 0, -1, 8), 4);
        assert!(
            world
                .chunks
                .values()
                .all(|c| c.sections.values().all(|s| s.block_light.is_none()))
        );
    }
    #[test]
    fn missing_or_unknown_inputs_never_produce_a_partial_result() {
        let bounds = LightBounds::new(0, 32).unwrap();
        let center = ChunkPos { x: 0, z: 0 };
        let mut world = fixture();
        world.chunks.remove(&ChunkPos { x: 1, z: 1 });
        assert!(initialize_generated_block_light(&world, bounds, center).is_none());
        let mut world = fixture();
        Arc::make_mut(world.chunks.get_mut(&center).unwrap())
            .sections
            .remove(&1);
        assert!(initialize_generated_block_light(&world, bounds, center).is_none());
        let mut world = fixture();
        let unknown = world.intern_state(BlockState {
            name: "example:unknown".into(),
            properties: Default::default(),
        });
        world.set_block_id(4, 8, 4, unknown).unwrap();
        assert!(initialize_generated_block_light(&world, bounds, center).is_none());
    }

    #[test]
    fn staged_calculation_requires_known_storage_and_respects_unallocated_sections() {
        let mut world = fixture();
        let bounds = LightBounds::new(0, 32).unwrap();
        let center = ChunkPos { x: 0, z: 0 };
        let torch = world.intern_state(BlockState {
            name: "minecraft:torch".into(),
            properties: Default::default(),
        });
        world.set_block_id(8, 8, 8, torch).unwrap();
        let active = BTreeSet::from([center]);
        let incomplete = VisibleLightSnapshot::new(true, false);
        assert!(
            generated_block_light_from_active_chunks(&world, bounds, center, &active, &incomplete)
                .is_none()
        );
        let covered = (-1..=1).flat_map(|z| (-1..=1).map(move |x| ChunkPos { x, z }));
        let unallocated =
            VisibleLightSnapshot::fresh_after_section_updates(true, false, [], covered).unwrap();
        let result =
            generated_block_light_from_active_chunks(&world, bounds, center, &active, &unallocated)
                .unwrap();
        assert_eq!(value(&result, 8, 8, 8), 0);
    }
}
