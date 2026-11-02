//! Initial sky-light values for generated blocks and a complete one-chunk halo.
use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

use crate::{
    block_light::{
        DIRECTIONS, LightBounds, LightProperties, faces_occlude, light_properties,
        transparent_light_properties,
    },
    generation_light::{VisibleLayers, VisibleLightSnapshot},
    loaded::{ChunkPos, LIGHT_BYTES, World},
};

/// Computes a draft without changing the world or publishing engine sections.
/// All blocks in the 3x3 input area must be from one final feature revision.
pub fn initialize_generated_sky_light(
    world: &World,
    bounds: LightBounds,
    center: ChunkPos,
) -> Option<VisibleLayers> {
    calculate_generated_sky_light(world, bounds, center, None, None)
}

/// Preview a fresh sky engine after activating the specified chunks. The source
/// heights for every 3x3 input chunk must already reflect these immutable blocks.
/// Every light section in that area must be allocated; sparse storage is rejected.
pub fn generated_sky_light_from_active_chunks(
    world: &World,
    bounds: LightBounds,
    center: ChunkPos,
    active: &BTreeSet<ChunkPos>,
    fresh_storage: &VisibleLightSnapshot,
) -> Option<VisibleLayers> {
    calculate_generated_sky_light(world, bounds, center, Some(active), Some(fresh_storage))
}

fn calculate_generated_sky_light(
    world: &World,
    bounds: LightBounds,
    center: ChunkPos,
    active: Option<&BTreeSet<ChunkPos>>,
    fresh_storage: Option<&VisibleLightSnapshot>,
) -> Option<VisibleLayers> {
    const SIDE: usize = 48;
    const PLANE: usize = SIDE * SIDE;
    let build = bounds.build_range();
    let light = bounds.light_range();
    let height = usize::try_from(light.end.checked_sub(light.start)?).ok()?;
    let sections = usize::try_from((build.end - build.start) / 16).ok()?;
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
                for section_y in light.start.div_euclid(16)..light.end.div_euclid(16) {
                    if !storage.sky_section_allocated(pos, section_y)? {
                        return None;
                    }
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
            'scan: for sy in (0..sections).rev() {
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
                    levels[x + z * SIDE + y * PLANE] = 15;
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
            }
        }
    }
    let mut output = VisibleLayers::new();
    for sy in light.start.div_euclid(16)..light.end.div_euclid(16) {
        let mut layer = [0u8; LIGHT_BYTES];
        let y0 = usize::try_from(sy * 16 - light.start).ok()?;
        for y in 0..16usize {
            for z in 0..16usize {
                for x in 0..16usize {
                    let value = levels[x + 16 + (z + 16) * SIDE + (y0 + y) * PLANE];
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
