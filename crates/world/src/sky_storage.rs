//! Conservative reconstruction of omitted saved sky-light layers.
//!
//! Java omits both unallocated layers and allocated homogeneous-zero layers.
//! We restore only cases whose allocation and direct-source values prove the
//! entire layer; ambiguous or incomplete data remains absent.
use crate::{
    block_light::{LightBounds, light_properties},
    loaded::{ChunkPos, LIGHT_BYTES, Section, World},
    sky_light::UNBOUNDED_SKY_SOURCE,
    sky_propagation::SkyChange,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

fn section_nonempty(world: &World, bounds: LightBounds, pos: ChunkPos, sy: i32) -> Option<bool> {
    let build = bounds.build_range();
    if sy < build.start.div_euclid(16) || sy >= build.end.div_euclid(16) {
        return Some(false);
    }
    let chunk = world.chunks.get(&pos)?;
    if chunk.position != pos || chunk.status.as_deref() != Some("minecraft:full") {
        return None;
    }
    let blocks = chunk
        .sections
        .get(&i8::try_from(sy).ok()?)?
        .blocks
        .as_ref()?;
    let first = *blocks.first()?;
    let first_state = world.state(first)?;
    if matches!(
        first_state.name.as_str(),
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    ) && first_state.properties.is_empty()
        && blocks.iter().all(|&id| id == first)
    {
        return Some(false);
    }
    let mut unknown = false;
    for &id in blocks.iter() {
        let Some(state) = world.state(id) else {
            unknown = true;
            continue;
        };
        if matches!(
            state.name.as_str(),
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
        ) && state.properties.is_empty()
        {
            continue;
        }
        if light_properties(state).is_some() {
            return Some(true);
        }
        unknown = true;
    }
    (!unknown).then_some(false)
}

fn layer_matches_sources(
    data: &[u8; LIGHT_BYTES],
    pos: ChunkPos,
    bottom: i32,
    bounds: LightBounds,
    source_y: &mut impl FnMut(i32, i32) -> Option<i32>,
) -> bool {
    let Some(x0) = pos.x.checked_mul(16) else {
        return false;
    };
    let Some(z0) = pos.z.checked_mul(16) else {
        return false;
    };
    let build = bounds.build_range();
    for z in 0..16 {
        for x in 0..16 {
            let Some(source) = x0
                .checked_add(x)
                .zip(z0.checked_add(z))
                .and_then(|(x, z)| source_y(x, z))
            else {
                return false;
            };
            if source != UNBOUNDED_SKY_SOURCE && !(build.start..=build.end).contains(&source) {
                return false;
            }
            let i = (x + z * 16) as usize;
            let byte = data[i / 2];
            let value = if i.is_multiple_of(2) {
                byte & 15
            } else {
                byte >> 4
            };
            if (source <= bottom && value != 15)
                || (source > bottom && source <= bottom + 15)
                || (source > bottom + 15 && value == 15)
            {
                return false;
            }
        }
    }
    true
}

/// Restore missing layers that can be proven to contain direct sky (15), an
/// allocated homogeneous-zero layer (0), or the repeated bottom layer above.
/// Inconclusive layers are left absent.
/// `source_y` must describe the world corresponding to the saved light arrays.
/// `allocation_world` contains the block sections from when the missing arrays
/// were saved; later edits can change which sections have allocated light.
/// The result names chunks in which at least one layer was materialized.
pub fn restore_missing_for_changes(
    world: &mut World,
    allocation_world: &World,
    bounds: LightBounds,
    changes: &[SkyChange],
    mut source_y: impl FnMut(i32, i32) -> Option<i32>,
) -> BTreeSet<ChunkPos> {
    if world.dimension != allocation_world.dimension
        || world.dimension_type != allocation_world.dimension_type
    {
        return BTreeSet::new();
    }
    let build = bounds.build_range();
    let light = bounds.light_range();
    let mut sections = BTreeSet::new();
    for change in changes {
        let p = change.position;
        if !build.contains(&p.y) {
            continue;
        }
        let valid = |y| y == UNBOUNDED_SKY_SOURCE || (build.start..=build.end).contains(&y);
        if !valid(change.previous_source) || !valid(change.current_source) {
            continue;
        }
        let normalize = |y| {
            if y == UNBOUNDED_SKY_SOURCE {
                light.start
            } else {
                y
            }
        };
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
        let Some(x0) = p.x.checked_sub(15) else {
            continue;
        };
        let Some(x1) = p.x.checked_add(15) else {
            continue;
        };
        let Some(z0) = p.z.checked_sub(15) else {
            continue;
        };
        let Some(z1) = p.z.checked_add(15) else {
            continue;
        };
        let min_y = low.saturating_sub(15).max(light.start).div_euclid(16);
        let max_y = high.saturating_add(15).min(light.end - 1).div_euclid(16);
        for x in x0.div_euclid(16)..=x1.div_euclid(16) {
            for z in z0.div_euclid(16)..=z1.div_euclid(16) {
                for y in min_y..=max_y {
                    sections.insert((ChunkPos { x, z }, y));
                }
            }
        }
    }
    let mut cache = BTreeMap::new();
    let mut restores: BTreeMap<(ChunkPos, i32), Box<[u8; LIGHT_BYTES]>> = BTreeMap::new();
    let mut sections: Vec<_> = sections.into_iter().collect();
    sections.sort_unstable_by(|(a, ay), (b, by)| a.cmp(b).then_with(|| by.cmp(ay)));
    for (pos, sy) in sections {
        let Some(chunk) = world.chunks.get(&pos) else {
            continue;
        };
        if chunk.position != pos
            || chunk.status.as_deref() != Some("minecraft:full")
            || !chunk.block_light_correct
        {
            continue;
        }
        let Ok(sy_i8) = i8::try_from(sy) else {
            continue;
        };
        if chunk
            .sections
            .get(&sy_i8)
            .is_some_and(|section| section.sky_light.is_some())
        {
            continue;
        }
        let in_build = (build.start.div_euclid(16)..build.end.div_euclid(16)).contains(&sy);
        if in_build
            && chunk
                .sections
                .get(&sy_i8)
                .and_then(|s| s.blocks.as_ref())
                .is_none()
        {
            continue;
        }
        let mut has_data = |p, y| {
            *cache
                .entry((p, y))
                .or_insert_with(|| section_nonempty(allocation_world, bounds, p, y))
        };
        let own_allocated = (-1..=1).any(|dy| has_data(pos, sy + dy) == Some(true));
        let known_unallocated = if own_allocated {
            false
        } else {
            let mut unknown = false;
            let mut occupied = false;
            'neighbors: for dx in -1..=1 {
                for dz in -1..=1 {
                    let Some(x) = pos.x.checked_add(dx) else {
                        unknown = true;
                        continue;
                    };
                    let Some(z) = pos.z.checked_add(dz) else {
                        unknown = true;
                        continue;
                    };
                    for dy in -1..=1 {
                        match has_data(ChunkPos { x, z }, sy + dy) {
                            Some(true) => {
                                occupied = true;
                                break 'neighbors;
                            }
                            None => unknown = true,
                            Some(false) => {}
                        }
                    }
                }
            }
            !occupied && !unknown
        };
        if !own_allocated && !known_unallocated {
            continue;
        }
        let bottom = sy * 16;
        let top = bottom + 15;
        let inherited = if known_unallocated {
            let above = sy + 1;
            let resolved = restores.get(&(pos, above)).map(|data| data.as_ref());
            let allocated_above = (-1..=1).any(|dy| has_data(pos, above + dy) == Some(true));
            let saved = if allocated_above {
                i8::try_from(above)
                    .ok()
                    .and_then(|y| chunk.sections.get(&y))
                    .and_then(|section| section.sky_light.as_deref())
            } else {
                None
            };
            resolved.or(saved).map(|data| {
                let mut values = Box::new([0; LIGHT_BYTES]);
                for layer in values.as_chunks_mut::<128>().0 {
                    layer.copy_from_slice(&data[..128]);
                }
                values
            })
        } else {
            None
        };
        let inherited = inherited
            .filter(|layer| layer_matches_sources(layer, pos, bottom, bounds, &mut source_y));
        let layer = if let Some(inherited) = inherited {
            Some(inherited)
        } else if known_unallocated && bottom >= build.end {
            Some(Box::new([0xff; LIGHT_BYTES]))
        } else {
            let Some(x0) = pos.x.checked_mul(16) else {
                continue;
            };
            let Some(z0) = pos.z.checked_mul(16) else {
                continue;
            };
            let mut direct = true;
            let mut dark = true;
            let mut valid = true;
            'columns: for dx in 0..16 {
                for dz in 0..16 {
                    let Some(x) = x0.checked_add(dx) else {
                        valid = false;
                        break 'columns;
                    };
                    let Some(z) = z0.checked_add(dz) else {
                        valid = false;
                        break 'columns;
                    };
                    let Some(source) = source_y(x, z) else {
                        valid = false;
                        break 'columns;
                    };
                    if source != UNBOUNDED_SKY_SOURCE
                        && !(build.start..=build.end).contains(&source)
                    {
                        valid = false;
                        break 'columns;
                    }
                    direct &= source <= bottom;
                    dark &= source > top;
                    if !direct && !dark {
                        break 'columns;
                    }
                }
            }
            if !valid {
                None
            } else if known_unallocated && direct {
                Some(Box::new([0xff; LIGHT_BYTES]))
            } else if own_allocated && dark {
                Some(Box::new([0; LIGHT_BYTES]))
            } else {
                None
            }
        };
        if let Some(layer) = layer {
            restores.insert((pos, sy), layer);
        }
    }
    let mut changed = BTreeSet::new();
    for ((pos, sy), layer) in restores {
        let Some(chunk) = world.chunks.get_mut(&pos) else {
            continue;
        };
        let chunk = Arc::make_mut(chunk);
        let Ok(sy) = i8::try_from(sy) else { continue };
        let section = chunk.sections.entry(sy).or_insert_with(|| {
            Arc::new(Section {
                blocks: None,
                biomes: None,
                block_light: None,
                sky_light: None,
            })
        });
        let section = Arc::make_mut(section);
        if section.sky_light.is_none() {
            section.sky_light = Some(layer);
            changed.insert(pos);
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        loaded::{BLOCK_COUNT, BlockPosition, BlockState, Chunk},
        sky_light::SkyColumn,
    };
    fn state(name: &str) -> BlockState {
        BlockState {
            name: format!("minecraft:{name}"),
            properties: BTreeMap::new(),
        }
    }
    fn fixture(roof: bool) -> World {
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(state("air"));
        let stone = world.intern_state(state("stone"));
        for x in -2..=2 {
            for z in -2..=2 {
                let pos = ChunkPos { x, z };
                let sections = (0..=3)
                    .map(|y| {
                        let mut blocks = Box::new([air; BLOCK_COUNT]);
                        if roof && y == 1 && (-1..=1).contains(&x) && (-1..=1).contains(&z) {
                            for z in 0..16 {
                                for x in 0..16 {
                                    blocks[x + z * 16 + 15 * 256] = stone;
                                }
                            }
                        }
                        (
                            y,
                            Arc::new(Section {
                                blocks: Some(blocks),
                                biomes: None,
                                block_light: None,
                                sky_light: None,
                            }),
                        )
                    })
                    .collect();
                world.chunks.insert(
                    pos,
                    Arc::new(Chunk {
                        position: pos,
                        status: Some("minecraft:full".into()),
                        block_light_correct: true,
                        sections,
                    }),
                );
            }
        }
        world
    }
    fn wide_change() -> SkyChange {
        SkyChange {
            position: BlockPosition { x: 8, y: 63, z: 8 },
            previous_source: UNBOUNDED_SKY_SOURCE,
            current_source: 64,
        }
    }
    #[test]
    fn restores_proven_direct_and_dark_layers_without_guessing_unknown_neighbors() {
        let bounds = LightBounds::new(0, 64).unwrap();
        let mut open = fixture(false);
        open.chunks.remove(&ChunkPos { x: -2, z: -2 });
        let allocation = open.clone();
        let restored = restore_missing_for_changes(
            &mut open,
            &allocation,
            bounds,
            &[wide_change()],
            |_, _| Some(UNBOUNDED_SKY_SOURCE),
        );
        assert!(restored.contains(&ChunkPos { x: 0, z: 0 }));
        assert_eq!(open.light(8, 65, 8, true), Some(15));
        assert_eq!(open.light(8, 8, 8, true), Some(15));
        assert_eq!(
            open.light(-8, 65, -8, true),
            None,
            "missing neighbors prevent assuming a layer is unallocated"
        );
        let mut dark = fixture(true);
        let allocation = dark.clone();
        restore_missing_for_changes(&mut dark, &allocation, bounds, &[wide_change()], |_, _| {
            Some(32)
        });
        assert_eq!(dark.light(8, 8, 8, true), Some(0));
        let mut contradictory = fixture(true);
        let allocation = contradictory.clone();
        restore_missing_for_changes(
            &mut contradictory,
            &allocation,
            bounds,
            &[wide_change()],
            |_, _| Some(UNBOUNDED_SKY_SOURCE),
        );
        assert_eq!(
            contradictory.light(8, 8, 8, true),
            None,
            "an omitted allocated layer cannot contain direct sky"
        );
        let mut edited = fixture(false);
        let allocation = edited.clone();
        let stone = edited.intern_state(state("stone"));
        for x in 0..16 {
            for z in 0..16 {
                edited.set_block_id(x, 31, z, stone).unwrap();
            }
        }
        restore_missing_for_changes(
            &mut edited,
            &allocation,
            bounds,
            &[wide_change()],
            |_, _| Some(32),
        );
        assert_eq!(
            edited.light(8, 8, 8, true),
            None,
            "current blocks do not prove allocation when the layer was saved"
        );
    }

    #[test]
    fn unallocated_sections_repeat_only_a_consistent_bottom_layer() {
        let bounds = LightBounds::new(0, 64).unwrap();
        let mut world = fixture(true);
        let chunk = Arc::make_mut(world.chunks.get_mut(&ChunkPos { x: 0, z: 0 }).unwrap());
        let mut data = Box::new([0u8; LIGHT_BYTES]);
        for (i, byte) in data[..128].iter_mut().enumerate() {
            *byte = ((i % 15) as u8) * 0x11;
        }
        Arc::make_mut(chunk.sections.get_mut(&0).unwrap()).sky_light = Some(data);
        let original = world.clone();
        restore_missing_for_changes(&mut world, &original, bounds, &[wide_change()], |_, _| {
            Some(32)
        });
        for y in -16..0 {
            for z in 0..16 {
                for x in 0..16 {
                    assert_eq!(world.light(x, y, z, true), original.light(x, 0, z, true));
                }
            }
        }
        let mut contradictory = original.clone();
        let chunk = Arc::make_mut(
            contradictory
                .chunks
                .get_mut(&ChunkPos { x: 0, z: 0 })
                .unwrap(),
        );
        Arc::make_mut(chunk.sections.get_mut(&0).unwrap()).sky_light =
            Some(Box::new([0xff; LIGHT_BYTES]));
        let allocation = contradictory.clone();
        restore_missing_for_changes(
            &mut contradictory,
            &allocation,
            bounds,
            &[wide_change()],
            |_, _| Some(32),
        );
        assert_eq!(
            contradictory.light(0, -1, 0, true),
            None,
            "inherited full sky below the direct source is inconsistent"
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn omitted_saved_layers_match_java_light_listener() {
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
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("SkyStorageOracle.java");
        std::fs::write(&source, include_str!("../tests/java/SkyStorageOracle.java")).unwrap();
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
        let output = temp.path().join("output.json");
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("SkyStorageOracle")
            .arg(&output));
        let cases: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(cases.len(), 2);
        let bounds = LightBounds::new(0, 64).unwrap();
        for (roof, case) in [false, true].into_iter().zip(cases) {
            let mut world = fixture(roof);
            for row in case["layers"].as_array().unwrap() {
                let pos = ChunkPos {
                    x: row["x"].as_i64().unwrap() as i32,
                    z: row["z"].as_i64().unwrap() as i32,
                };
                let sy = row["y"].as_i64().unwrap() as i8;
                let Some(values) = row["saved"].as_array() else {
                    continue;
                };
                let bytes: Vec<u8> = values.iter().map(|v| v.as_u64().unwrap() as u8).collect();
                let data: Box<[u8; LIGHT_BYTES]> = bytes.into_boxed_slice().try_into().unwrap();
                let chunk = Arc::make_mut(world.chunks.get_mut(&pos).unwrap());
                let section = chunk.sections.entry(sy).or_insert_with(|| {
                    Arc::new(Section {
                        blocks: None,
                        biomes: None,
                        block_light: None,
                        sky_light: None,
                    })
                });
                Arc::make_mut(section).sky_light = Some(data);
            }
            let before = world.clone();
            let mut source_cache = BTreeMap::new();
            restore_missing_for_changes(&mut world, &before, bounds, &[wide_change()], |x, z| {
                let column = source_cache
                    .entry((x, z))
                    .or_insert_with(|| SkyColumn::load(&before, bounds, x, z));
                column.map(SkyColumn::lowest_source_y)
            });
            let grid = case["grid"].as_array().unwrap();
            let mut direct = 0;
            let mut dark = 0;
            for sy in -1..=4 {
                if before.light(0, sy * 16, 0, true).is_some()
                    || world.light(0, sy * 16, 0, true).is_none()
                {
                    continue;
                }
                for y in sy * 16..sy * 16 + 16 {
                    for z in 0..16 {
                        for x in 0..16 {
                            let value = world.light(x, y, z, true).unwrap();
                            let index = ((y + 16) * 256 + z * 16 + x) as usize;
                            assert_eq!(
                                u64::from(value),
                                grid[index].as_u64().unwrap(),
                                "roof={roof} at {x},{y},{z}"
                            );
                            if value == 0 {
                                dark += 1
                            } else if value == 15 {
                                direct += 1
                            }
                        }
                    }
                }
            }
            if roof {
                assert!(dark > 0, "Java fixture must contain an omitted dark layer");
            } else {
                assert!(
                    direct > 0,
                    "Java fixture must contain an omitted direct-sky layer"
                );
            }
            let mut column = SkyColumn::load(&before, bounds, 8, 8).unwrap();
            let previous_source = column.lowest_source_y();
            let stone = world.intern_state(state("stone"));
            world.set_block_id(8, 63, 8, stone).unwrap();
            column.update(&world, 63).unwrap();
            let current_source = column.lowest_source_y();
            crate::sky_propagation::relight_changes(
                &mut world,
                bounds,
                &[SkyChange {
                    position: BlockPosition { x: 8, y: 63, z: 8 },
                    previous_source,
                    current_source,
                }],
                |x, z| {
                    if (x, z) == (8, 8) {
                        return Some(current_source);
                    }
                    source_cache
                        .entry((x, z))
                        .or_insert_with(|| SkyColumn::load(&before, bounds, x, z))
                        .map(SkyColumn::lowest_source_y)
                },
            )
            .expect("all required omitted layers were reconstructed");
            let after = case["after"].as_array().unwrap();
            for y in -16..80 {
                for z in 0..16 {
                    for x in 0..16 {
                        let i = ((y + 16) * 256 + z * 16 + x) as usize;
                        assert_eq!(
                            u64::from(world.light(x, y, z, true).unwrap()),
                            after[i].as_u64().unwrap(),
                            "roof={roof}, after edit at {x},{y},{z}"
                        );
                    }
                }
            }
        }
    }
}
