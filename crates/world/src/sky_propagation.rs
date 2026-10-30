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
    let low = normalize(previous_source)
        .min(normalize(current_source))
        .min(position.y);
    let high = normalize(previous_source)
        .max(normalize(current_source))
        .max(position.y);
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
    }
}
