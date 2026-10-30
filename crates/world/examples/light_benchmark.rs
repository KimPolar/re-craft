//! Synthetic block-light timing harness; this example is not part of the client EXE.
use native_world::{
    block_light::{light_change, relight_block, relight_changes},
    loaded::{BLOCK_COUNT, BlockPosition, BlockState, Chunk, ChunkPos, Section, World},
};
use std::{collections::BTreeMap, hint::black_box, sync::Arc, time::Instant};

fn state(name: &str, properties: &[(&str, &str)]) -> BlockState {
    BlockState {
        name: format!("minecraft:{name}"),
        properties: properties
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect(),
    }
}
fn main() {
    let mut world = World::new("minecraft:overworld");
    let air = world.intern_state(state("air", &[]));
    let stone = world.intern_state(state("stone", &[]));
    let torch = world.intern_state(state("torch", &[]));
    let redstone = world.intern_state(state("redstone_ore", &[("lit", "false")]));
    let lit = world.intern_state(state("redstone_ore", &[("lit", "true")]));
    let slab = world.intern_state(state(
        "stone_slab",
        &[("type", "bottom"), ("waterlogged", "false")],
    ));
    let snow = world.intern_state(state("snow", &[("layers", "7")]));
    for x in 0..=1 {
        for z in 0..=1 {
            let pos = ChunkPos { x, z };
            let sections = (0..=1)
                .map(|y| {
                    (
                        y,
                        Arc::new(Section {
                            blocks: Some(Box::new([air; BLOCK_COUNT])),
                            biomes: None,
                            block_light: None,
                            sky_light: None,
                        }),
                    )
                })
                .collect::<BTreeMap<_, _>>();
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
    let source = BlockPosition {
        x: 14,
        y: 15,
        z: 15,
    };
    let center = BlockPosition {
        x: 15,
        y: 15,
        z: 15,
    };
    world.set_block_id(17, 15, 15, slab).unwrap();
    world.set_block_id(17, 15, 16, snow).unwrap();
    let change = light_change(world.state(air).unwrap(), world.state(torch).unwrap()).unwrap();
    world
        .set_block_id(source.x, source.y, source.z, torch)
        .unwrap();
    relight_block(&mut world, source, change).unwrap();
    for (label, a, b) in [
        ("redstone", redstone, lit),
        ("torch", air, torch),
        ("opacity", air, stone),
    ] {
        let old = world.block(center.x, center.y, center.z).unwrap().clone();
        let new = world.state(a).unwrap().clone();
        let initial = light_change(&old, &new);
        world.set_block_id(center.x, center.y, center.z, a).unwrap();
        if let Some(change) = initial {
            relight_block(&mut world, center, change).unwrap();
        }
        let changes = [
            light_change(world.state(a).unwrap(), world.state(b).unwrap()).unwrap(),
            light_change(world.state(b).unwrap(), world.state(a).unwrap()).unwrap(),
        ];
        let mut samples = Vec::new();
        for i in 0..110 {
            world
                .set_block_id(center.x, center.y, center.z, if i % 2 == 0 { b } else { a })
                .unwrap();
            let started = Instant::now();
            black_box(relight_block(&mut world, center, changes[i % 2]).unwrap());
            if i >= 10 {
                samples.push(started.elapsed().as_secs_f64() * 1000.0);
            }
        }
        samples.sort_by(f64::total_cmp);
        println!(
            "{label}: p50={:.3} ms, p95={:.3} ms (100 samples)",
            samples[50], samples[95]
        );
    }
    let remove = light_change(world.state(torch).unwrap(), world.state(air).unwrap()).unwrap();
    world
        .set_block_id(source.x, source.y, source.z, air)
        .unwrap();
    relight_block(&mut world, source, remove).unwrap();
    let positions = [
        source,
        BlockPosition {
            x: 16,
            y: 15,
            z: 15,
        },
    ];
    let changes = [
        light_change(world.state(air).unwrap(), world.state(torch).unwrap()).unwrap(),
        remove,
    ];
    let mut samples = Vec::new();
    for i in 0..110 {
        let value = if i % 2 == 0 { torch } else { air };
        for p in positions {
            world.set_block_id(p.x, p.y, p.z, value).unwrap();
        }
        let batch = positions.map(|p| (p, changes[i % 2]));
        let started = Instant::now();
        black_box(relight_changes(&mut world, &batch).unwrap());
        if i >= 10 {
            samples.push(started.elapsed().as_secs_f64() * 1000.0);
        }
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "two overlapping torches: p50={:.3} ms, p95={:.3} ms (100 samples)",
        samples[50], samples[95]
    );
}
