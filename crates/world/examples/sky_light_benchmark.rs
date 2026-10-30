//! Synthetic sky-light timing harness; this example is not part of the client EXE.
use native_world::{
    block_light::LightBounds,
    loaded::{
        BLOCK_COUNT, BlockPosition, BlockState, Chunk, ChunkPos, LIGHT_BYTES, Section, World,
    },
    sky_light::{SkyColumn, SkySourceScanner, UNBOUNDED_SKY_SOURCE},
    sky_propagation::{SkyChange, relight_change, relight_changes},
};
use std::{collections::BTreeMap, hint::black_box, sync::Arc, time::Instant};

fn state(name: &str) -> BlockState {
    BlockState {
        name: format!("minecraft:{name}"),
        properties: BTreeMap::new(),
    }
}
fn main() {
    let bounds = LightBounds::new(-64, 320).unwrap();
    let mut world = World::new("minecraft:overworld");
    let air = world.intern_state(state("air"));
    let stone = world.intern_state(state("stone"));
    for x in -1..=2 {
        for z in -1..=2 {
            let pos = ChunkPos { x, z };
            let sections = (-5..=20)
                .map(|y| {
                    (
                        y,
                        Arc::new(Section {
                            blocks: (-4..20).contains(&y).then(|| Box::new([air; BLOCK_COUNT])),
                            biomes: None,
                            block_light: None,
                            sky_light: Some(Box::new([0xff; LIGHT_BYTES])),
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
    let fresh_world = world.clone();
    for (label, y) in [("mid-height", 80), ("top-height", 319)] {
        let position = BlockPosition { x: 15, y, z: 15 };
        let mut source = SkyColumn::load(&world, bounds, 15, 15).unwrap();
        let mut samples = Vec::new();
        for i in 0..55 {
            let previous = source.lowest_source_y();
            world
                .set_block_id(15, y, 15, if i % 2 == 0 { stone } else { air })
                .unwrap();
            source.update(&world, y).unwrap();
            let current = source.lowest_source_y();
            let started = Instant::now();
            black_box(
                relight_change(&mut world, bounds, position, previous, current, |x, z| {
                    Some(if (x, z) == (15, 15) {
                        current
                    } else {
                        UNBOUNDED_SKY_SOURCE
                    })
                })
                .unwrap(),
            );
            if i >= 5 {
                samples.push(started.elapsed().as_secs_f64() * 1000.0);
            }
        }
        samples.sort_by(f64::total_cmp);
        println!(
            "{label}: p50={:.3} ms, p95={:.3} ms (50 samples)",
            samples[25], samples[47]
        );
        // Leave the next case in the initial fully lit state.
        let previous = source.lowest_source_y();
        world.set_block_id(15, y, 15, air).unwrap();
        source.update(&world, y).unwrap();
        let current = source.lowest_source_y();
        relight_change(&mut world, bounds, position, previous, current, |x, z| {
            Some(if (x, z) == (15, 15) {
                current
            } else {
                UNBOUNDED_SKY_SOURCE
            })
        })
        .unwrap();
    }

    let mut columns = BTreeMap::new();
    for x in [14, 16] {
        let mut source = SkyColumn::load(&world, bounds, x, 15).unwrap();
        let previous = source.lowest_source_y();
        world.set_block_id(x, 319, 15, stone).unwrap();
        source.update(&world, 319).unwrap();
        let current = source.lowest_source_y();
        columns.insert((x, 15), current);
        relight_change(
            &mut world,
            bounds,
            BlockPosition { x, y: 319, z: 15 },
            previous,
            current,
            |x, z| Some(*columns.get(&(x, z)).unwrap_or(&UNBOUNDED_SKY_SOURCE)),
        )
        .unwrap();
    }
    let mut samples = Vec::new();
    let changes = [14, 16].map(|x| SkyChange {
        position: BlockPosition { x, y: 80, z: 15 },
        previous_source: 320,
        current_source: 320,
    });
    for i in 0..55 {
        for change in &changes {
            world
                .set_block_id(
                    change.position.x,
                    change.position.y,
                    change.position.z,
                    if i % 2 == 0 { stone } else { air },
                )
                .unwrap();
        }
        let started = Instant::now();
        black_box(
            relight_changes(&mut world, bounds, &changes, |x, z| {
                Some(*columns.get(&(x, z)).unwrap_or(&UNBOUNDED_SKY_SOURCE))
            })
            .unwrap(),
        );
        if i >= 5 {
            samples.push(started.elapsed().as_secs_f64() * 1000.0);
        }
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "two changes under cover: p50={:.3} ms, p95={:.3} ms (50 samples)",
        samples[25], samples[47]
    );
    for cached in [false, true] {
        let mut samples = Vec::new();
        for i in 0..12 {
            let started = Instant::now();
            let mut scanner = SkySourceScanner::new(&world, bounds);
            for x in 0..16 {
                for z in 0..16 {
                    let value = if cached {
                        scanner.load(x, z)
                    } else {
                        SkyColumn::load(&world, bounds, x, z)
                    };
                    black_box(value.unwrap());
                }
            }
            if i >= 2 {
                samples.push(started.elapsed().as_secs_f64() * 1000.0);
            }
        }
        samples.sort_by(f64::total_cmp);
        println!(
            "256 source columns ({}): p50={:.3} ms, p95={:.3} ms (10 samples)",
            if cached {
                "shared scan"
            } else {
                "independent scans"
            },
            samples[5],
            samples[9]
        );
    }

    let mut sparse = fresh_world;
    let duplicate: Vec<_> = sparse
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
    sparse.chunks.extend(duplicate);
    let changes = [15, 143].map(|x| {
        sparse.set_block_id(x, 319, 15, stone).unwrap();
        SkyChange {
            position: BlockPosition { x, y: 319, z: 15 },
            previous_source: UNBOUNDED_SKY_SOURCE,
            current_source: 320,
        }
    });
    let mut samples = Vec::new();
    for i in 0..12 {
        let mut input = sparse.clone();
        let started = Instant::now();
        black_box(
            relight_changes(&mut input, bounds, &changes, |x, z| {
                Some(if z == 15 && (x == 15 || x == 143) {
                    320
                } else {
                    UNBOUNDED_SKY_SOURCE
                })
            })
            .unwrap(),
        );
        if i >= 2 {
            samples.push(started.elapsed().as_secs_f64() * 1000.0);
        }
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "two distant top edits: p50={:.3} ms, p95={:.3} ms (10 samples)",
        samples[5], samples[9]
    );
}
