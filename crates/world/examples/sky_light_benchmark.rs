//! Synthetic sky-light timing harness; this example is not part of the client EXE.
use native_world::{
    block_light::LightBounds,
    loaded::{
        BLOCK_COUNT, BlockPosition, BlockState, Chunk, ChunkPos, LIGHT_BYTES, Section, World,
    },
    sky_light::{SkyColumn, UNBOUNDED_SKY_SOURCE},
    sky_propagation::relight_change,
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
}
