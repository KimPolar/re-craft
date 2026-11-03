//! Initial sky-light values for generated blocks and a complete one-chunk halo.
use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    sync::Arc,
};

use crate::{
    block_light::{
        DIRECTIONS, LightBounds, LightProperties, faces_occlude, light_properties,
        transparent_light_properties,
    },
    generation_light::{VisibleLayers, VisibleLightSnapshot, order_pending_light_checks},
    loaded::{ChunkPos, LIGHT_BYTES, World},
};

/// Computes a draft without changing the world or publishing engine sections.
/// All blocks in the 3x3 input area must be from one final feature revision.
pub fn initialize_generated_sky_light(
    world: &World,
    bounds: LightBounds,
    center: ChunkPos,
) -> Option<VisibleLayers> {
    calculate_generated_sky_light(world, bounds, center, None, None, None)
}

/// Preview a fresh sky engine after activating the specified chunks. The source
/// heights for every 3x3 input chunk must already reflect these immutable blocks.
pub fn generated_sky_light_from_active_chunks(
    world: &World,
    bounds: LightBounds,
    center: ChunkPos,
    active: &BTreeSet<ChunkPos>,
    fresh_storage: &VisibleLightSnapshot,
) -> Option<VisibleLayers> {
    calculate_generated_sky_light(
        world,
        bounds,
        center,
        Some(active),
        Some(fresh_storage),
        None,
    )
}

/// Model a fresh engine whose section registrations and sky-source
/// initializations have only reached some chunks. Uninitialized chunks must
/// still have the default, never-updated Java sky-source heightmap. All
/// registration events and prior activations must be included in these sets.
pub fn generated_sky_light_with_initialization_history(
    world: &World,
    bounds: LightBounds,
    center: ChunkPos,
    active: &BTreeSet<ChunkPos>,
    fresh_storage: &VisibleLightSnapshot,
    registered: &BTreeSet<ChunkPos>,
    initialized: &BTreeSet<ChunkPos>,
) -> Option<VisibleLayers> {
    if !active.is_subset(initialized) || !initialized.is_subset(registered) {
        return None;
    }
    calculate_generated_sky_light(
        world,
        bounds,
        center,
        Some(active),
        Some(fresh_storage),
        Some((registered, initialized)),
    )
}

/// Apply a single drained `checkBlock` in a registered, inactive chunk.
///
/// `before` must be the complete published storage over the affected area;
/// section allocation and active chunks must be unchanged, and `world` must
/// contain the updated block states. This does not update section allocation,
/// source heights, block light, or active chunks' direct sky sources. Missing
/// inputs and sparse vertical gaps are rejected without changing `before`.
pub fn check_inactive_sky_block(
    world: &World,
    bounds: LightBounds,
    before: &VisibleLightSnapshot,
    active: &BTreeSet<ChunkPos>,
    position: [i32; 3],
) -> Option<VisibleLightSnapshot> {
    check_inactive_sky_blocks(world, bounds, before, active, &[position])
}

/// Drain `checkBlock` calls in insertion order using Java's pending-node set.
/// Duplicate positions are coalesced, and all checks run before propagation.
/// The single-check function's storage and block-state preconditions apply.
pub fn check_inactive_sky_blocks(
    world: &World,
    bounds: LightBounds,
    before: &VisibleLightSnapshot,
    active: &BTreeSet<ChunkPos>,
    positions: &[[i32; 3]],
) -> Option<VisibleLightSnapshot> {
    #[derive(Clone, Copy)]
    struct Entry {
        pos: [i32; 3],
        level: u8,
        directions: u8,
    }
    const ALL: u8 = 0b11_1111;
    // Direction.values(): DOWN, UP, NORTH, SOUTH, WEST, EAST.
    const ORDER: [usize; 6] = [3, 2, 5, 4, 1, 0];
    if !before.has_sky_channel() {
        return None;
    }
    let mut draft = before.clone();
    let mut decreases = VecDeque::new();
    let mut increases = VecDeque::new();
    let ordered = order_pending_light_checks(positions.iter().copied())?;
    for position in ordered {
        let chunk = ChunkPos {
            x: position[0].div_euclid(16),
            z: position[2].div_euclid(16),
        };
        if active.contains(&chunk) || !bounds.build_range().contains(&position[1]) {
            return None;
        }
        let Some(old) = draft.stored_sky(position)? else {
            continue;
        };
        if old > 0 {
            draft.set_stored_sky(position, 0)?;
        }
        decreases.push_back(Entry {
            pos: position,
            level: old.max(1),
            directions: ALL,
        });
    }
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
    let neighbor = |p: [i32; 3], direction: usize| {
        let (dx, dy, dz) = DIRECTIONS[direction];
        Some([
            p[0].checked_add(dx)?,
            p[1].checked_add(dy)?,
            p[2].checked_add(dz)?,
        ])
    };
    let check_gap = |snapshot: &VisibleLightSnapshot, p: [i32; 3]| -> Option<()> {
        if p[1].rem_euclid(16) == 0
            && (matches!(p[0].rem_euclid(16), 0 | 15) || matches!(p[2].rem_euclid(16), 0 | 15))
        {
            let below = p[1].checked_sub(1)?;
            if bounds.light_range().contains(&below)
                && snapshot.stored_sky([p[0], below, p[2]])?.is_none()
            {
                return None;
            }
        }
        Some(())
    };
    while let Some(entry) = decreases.pop_front() {
        check_gap(&draft, entry.pos)?;
        for direction in ORDER {
            if entry.directions & (1 << direction) == 0 {
                continue;
            }
            let next = neighbor(entry.pos, direction)?;
            if !bounds.light_range().contains(&next[1]) {
                continue;
            }
            let Some(level) = draft.stored_sky(next)? else {
                continue;
            };
            if level == 0 {
                continue;
            }
            let opposite = direction ^ 1;
            if level <= entry.level.saturating_sub(1) {
                draft.set_stored_sky(next, 0)?;
                decreases.push_back(Entry {
                    pos: next,
                    level,
                    directions: ALL & !(1 << opposite),
                });
            } else {
                increases.push_back(Entry {
                    pos: next,
                    level,
                    directions: 1 << opposite,
                });
            }
        }
    }
    while let Some(entry) = increases.pop_front() {
        if draft.stored_sky(entry.pos)? != Some(entry.level) {
            continue;
        }
        check_gap(&draft, entry.pos)?;
        let from = get_properties(entry.pos, &mut properties)?;
        for direction in ORDER {
            if entry.directions & (1 << direction) == 0 {
                continue;
            }
            let next = neighbor(entry.pos, direction)?;
            if !bounds.light_range().contains(&next[1]) {
                continue;
            }
            let Some(old) = draft.stored_sky(next)? else {
                continue;
            };
            if entry.level.saturating_sub(1) <= old {
                continue;
            }
            let to = get_properties(next, &mut properties)?;
            let level = entry.level.saturating_sub(to.opacity.max(1));
            if level <= old
                || ((from.has_faces || to.has_faces)
                    && faces_occlude(&from.faces[direction], &to.faces[direction ^ 1]))
            {
                continue;
            }
            draft.set_stored_sky(next, level)?;
            if level > 1 {
                increases.push_back(Entry {
                    pos: next,
                    level,
                    directions: ALL & !(1 << (direction ^ 1)),
                });
            }
        }
    }
    Some(draft)
}

fn calculate_generated_sky_light(
    world: &World,
    bounds: LightBounds,
    center: ChunkPos,
    active: Option<&BTreeSet<ChunkPos>>,
    fresh_storage: Option<&VisibleLightSnapshot>,
    history: Option<(&BTreeSet<ChunkPos>, &BTreeSet<ChunkPos>)>,
) -> Option<VisibleLayers> {
    const SIDE: usize = 48;
    const PLANE: usize = SIDE * SIDE;
    let build = bounds.build_range();
    let light = bounds.light_range();
    let height = usize::try_from(light.end.checked_sub(light.start)?).ok()?;
    let sections = usize::try_from((build.end - build.start) / 16).ok()?;
    let light_sections = height / 16;
    let mut allocated = vec![fresh_storage.is_none(); 9 * light_sections];
    let mut empty = vec![true; 9 * sections];
    let mut behavior = vec![0u32; PLANE.checked_mul(height)?];
    let mut properties: Vec<LightProperties> = vec![transparent_light_properties()];
    let mut known = HashMap::<u32, (u32, bool)>::new();
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
                if !storage.has_sky_channel() {
                    return None;
                }
                for section_y in light.start.div_euclid(16)..light.end.div_euclid(16) {
                    let index = usize::try_from(section_y - light.start.div_euclid(16)).ok()?;
                    allocated[(cz * 3 + cx) * light_sections + index] =
                        storage.sky_section_allocated(pos, section_y)?;
                }
            }
            for section_index in 0..sections {
                let section_y = build.start.div_euclid(16) + section_index as i32;
                let blocks = chunk
                    .sections
                    .get(&i8::try_from(section_y).ok()?)?
                    .blocks
                    .as_ref()?;
                for (local, &id) in blocks.iter().enumerate() {
                    let (index, is_air) = if let Some(&value) = known.get(&id.0) {
                        value
                    } else {
                        let state = world.state(id)?;
                        let p = light_properties(state)?;
                        let index = u32::try_from(properties.len()).ok()?;
                        let is_air = matches!(
                            state.name.as_str(),
                            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                        );
                        properties.push(p);
                        known.insert(id.0, (index, is_air));
                        (index, is_air)
                    };
                    empty[(cz * 3 + cx) * sections + section_index] &= is_air;
                    let x = cx * 16 + local % 16;
                    let z = cz * 16 + (local / 16) % 16;
                    let y = usize::try_from(section_y * 16 + (local / 256) as i32 - light.start)
                        .ok()?;
                    behavior[x + z * SIDE + y * PLANE] = index;
                }
            }
        }
    }
    // Initialized nonempty sections allocate their own section and the
    // surrounding 3x3x3 halo. Sparse-gap propagation relies on this invariant.
    if fresh_storage.is_some() {
        for cz in 0..3usize {
            for cx in 0..3usize {
                let pos = ChunkPos {
                    x: center.x.checked_add(cx as i32 - 1)?,
                    z: center.z.checked_add(cz as i32 - 1)?,
                };
                if history.is_some_and(|(registered, _)| !registered.contains(&pos)) {
                    continue;
                }
                for section in 0..sections {
                    if empty[(cz * 3 + cx) * sections + section] {
                        continue;
                    }
                    for dz in -1i32..=1 {
                        for dx in -1i32..=1 {
                            let nx = cx as i32 + dx;
                            let nz = cz as i32 + dz;
                            if !(0..3).contains(&nx) || !(0..3).contains(&nz) {
                                continue;
                            }
                            for dy in -1i32..=1 {
                                let sy = section as i32 + 1 + dy;
                                if !(0..light_sections as i32).contains(&sy) {
                                    continue;
                                }
                                if !allocated
                                    [(nz as usize * 3 + nx as usize) * light_sections + sy as usize]
                                {
                                    return None;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let mut levels = vec![0u8; behavior.len()];
    let mut source_y = vec![0usize; PLANE];
    for z in 0..SIDE {
        for x in 0..SIDE {
            let pos = ChunkPos {
                x: center.x.checked_add((x / 16) as i32 - 1)?,
                z: center.z.checked_add((z / 16) as i32 - 1)?,
            };
            let mut above = 0usize;
            let mut lowest = 0usize;
            'scan: for sy in (0..sections)
                .rev()
                .filter(|_| history.is_none_or(|(_, initialized)| initialized.contains(&pos)))
            {
                if empty[((z / 16) * 3 + x / 16) * sections + sy] {
                    above = 0;
                    continue;
                }
                for local_y in (0..16).rev() {
                    let y = 16 + sy * 16 + local_y;
                    let below = behavior[x + z * SIDE + y * PLANE] as usize;
                    let a = &properties[above];
                    let b = &properties[below];
                    if b.opacity > 0
                        || ((a.has_faces || b.has_faces) && faces_occlude(&a.faces[3], &b.faces[2]))
                    {
                        lowest = y + 1;
                        break 'scan;
                    }
                    above = below;
                }
            }
            source_y[x + z * SIDE] = lowest;
            if active.is_none_or(|chunks| chunks.contains(&pos)) {
                for y in lowest..height {
                    if allocated[((z / 16) * 3 + x / 16) * light_sections + y / 16] {
                        levels[x + z * SIDE + y * PLANE] = 15;
                    }
                }
            }
        }
    }
    let mut queue: [Vec<usize>; 16] = std::array::from_fn(|_| Vec::new());
    // Seed only direct-sky cells next to a darker cell; open sky is already 15.
    for index in 0..levels.len() {
        if levels[index] != 15 {
            continue;
        }
        let x = index % SIDE;
        let z = index / SIDE % SIDE;
        let y = index / PLANE;
        if DIRECTIONS.into_iter().any(|(dx, dy, dz)| {
            let nx = x as i32 + dx;
            let ny = y as i32 + dy;
            let nz = z as i32 + dz;
            nx >= 0
                && nx < SIDE as i32
                && ny >= 0
                && ny < height as i32
                && nz >= 0
                && nz < SIDE as i32
                && levels[nx as usize + nz as usize * SIDE + ny as usize * PLANE] < 14
        }) {
            queue[15].push(index);
        }
    }
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
                let nsection =
                    ((nz as usize / 16) * 3 + nx as usize / 16) * light_sections + ny as usize / 16;
                if !allocated[nsection] {
                    continue;
                }
                // Direct sky seeds only below the neighbor's source boundary,
                // including when that neighboring chunk is not enabled yet.
                if active.is_some()
                    && level == 15
                    && ny as usize >= source_y[nx as usize + nz as usize * SIDE]
                {
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
                // When this edge crosses a chunk border at the bottom of an
                // allocated section, Java copies the boundary value into
                // allocated neighbor sections beside empty sections below.
                if fresh_storage.is_some()
                    && y % 16 == 0
                    && dy == 0
                    && (x / 16 != nx as usize / 16 || z / 16 != nz as usize / 16)
                {
                    let src_chunk = (z / 16) * 3 + x / 16;
                    let dst_chunk = (nz as usize / 16) * 3 + nx as usize / 16;
                    let mut sy = y / 16;
                    while sy > 0 {
                        sy -= 1;
                        if allocated[src_chunk * light_sections + sy] {
                            break;
                        }
                        if !allocated[dst_chunk * light_sections + sy] {
                            continue;
                        }
                        for down_y in sy * 16..(sy + 1) * 16 {
                            let dest = nx as usize + nz as usize * SIDE + down_y * PLANE;
                            if candidate > levels[dest] {
                                levels[dest] = candidate;
                                queue[candidate as usize].push(dest);
                            }
                        }
                    }
                }
            }
        }
    }
    let mut output = VisibleLayers::new();
    let mut next_allocated = vec![None; light_sections];
    let mut next = None;
    for section in (0..light_sections).rev() {
        if allocated[4 * light_sections + section] {
            next = Some(section);
        }
        next_allocated[section] = next;
    }
    for sy in light.start.div_euclid(16)..light.end.div_euclid(16) {
        let mut layer = [0u8; LIGHT_BYTES];
        let y0 = usize::try_from(sy * 16 - light.start).ok()?;
        let section = y0 / 16;
        let missing = !allocated[4 * light_sections + section];
        let inherited = if missing {
            match next_allocated[section] {
                Some(above) => Some(above * 16),
                None => {
                    output.insert(sy, Arc::new([0xff; LIGHT_BYTES]));
                    continue;
                }
            }
        } else {
            None
        };
        for y in 0..16usize {
            for z in 0..16usize {
                for x in 0..16usize {
                    let source_y_index = inherited.unwrap_or(y0 + y);
                    let value = levels[x + 16 + (z + 16) * SIDE + source_y_index * PLANE];
                    let local = x + z * 16 + y * 256;
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
