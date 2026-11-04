use native_session::worldgen::chunk_ticks::{
    ChunkTicks, ConvertedChunkTicks, IndexedRegionQueue, RegionTicks, SavedTick, ScheduleOutcome,
    ScheduledTick, TickRegistry,
};
use native_world::loaded::ChunkPos;

fn saved(kind: &str, position: [i32; 3], delay: i32, priority: i32) -> SavedTick {
    SavedTick {
        kind: kind.into(),
        position,
        delay,
        priority,
    }
}
fn scheduled(
    kind: &str,
    position: [i32; 3],
    trigger: i64,
    priority: i32,
    sub_tick: i64,
) -> ScheduledTick {
    ScheduledTick {
        kind: kind.into(),
        position,
        trigger,
        priority,
        sub_tick,
    }
}
fn trace() -> Vec<String> {
    let (a, b, c) = ([-31, 0, 50], [-30, 1, 50], [-29, 2, 50]);
    let pos = ChunkPos { x: -2, z: 3 };
    // ProtoChunkTicks preserves insertion order and discards requested delay.
    let mut full =
        ChunkTicks::from_saved(pos, vec![saved("water", a, 0, -1), saved("lava", b, 0, 1)])
            .unwrap();
    assert!(
        !full
            .schedule(scheduled("minecraft:water", a, 1, 0, 1))
            .unwrap()
    );
    assert!(full.schedule(scheduled("water", c, 45, 0, 7)).unwrap());
    assert!(full.schedule(scheduled("lava", c, 45, 0, 7)).unwrap());
    let format_saved = |label: &str, ticks: Vec<SavedTick>| {
        let mut line = label.to_string();
        for t in ticks {
            line.push_str(&format!(
                " {}/{},{},{}/{}/{}",
                t.kind, t.position[0], t.position[1], t.position[2], t.delay, t.priority
            ));
        }
        line
    };
    let format_tick = |tick: Option<ScheduledTick>| match tick {
        None => "P null".into(),
        Some(t) => format!(
            "P {}/{},{},{}/{}/{}/{}",
            t.kind, t.position[0], t.position[1], t.position[2], t.trigger, t.priority, t.sub_tick
        ),
    };
    let mut out = vec![
        format_saved("before", full.pack(42)),
        format_tick(full.peek().cloned()),
    ];
    full.unpack(42);
    out.push(format_saved("after", full.pack(42)));
    while full.peek().is_some() {
        out.push(format_tick(full.poll()));
    }
    out.push(format_tick(full.poll()));
    let repeated = saved("water", a, -2, 0);
    let mut duplicates = ChunkTicks::from_saved(pos, vec![repeated.clone(), repeated]).unwrap();
    duplicates.unpack(i64::MAX);
    out.push(format_tick(duplicates.poll()));
    assert!(
        duplicates
            .schedule(scheduled("water", a, i64::MIN, -1, 6))
            .unwrap()
    );
    out.push(format_saved("duplicates", duplicates.pack(i64::MAX)));
    while duplicates.peek().is_some() {
        out.push(format_tick(duplicates.poll()));
    }
    let mut many = ChunkTicks::from_saved(pos, vec![]).unwrap();
    for i in 0..80 {
        assert!(
            many.schedule(scheduled(
                &format!("minecraft:k{}", i % 4),
                [-32 + i % 16, i % 5, 48 + (i * 7) % 16],
                i64::from(100 + (i * 13) % 7),
                (i * 5) % 7 - 3,
                i64::from((i * 3) % 9 - 4),
            ))
            .unwrap()
        );
    }
    out.push(format_saved("many", many.pack(100)));
    while many.peek().is_some() {
        out.push(format_tick(many.poll()));
    }
    let second = ChunkPos { x: -1, z: 3 };
    let chunks = std::collections::BTreeMap::from([
        (
            pos,
            ConvertedChunkTicks {
                blocks: ChunkTicks::from_saved(pos, vec![saved("stone", a, 0, 0)]).unwrap(),
                fluids: ChunkTicks::from_saved(pos, vec![]).unwrap(),
            },
        ),
        (
            second,
            ConvertedChunkTicks {
                blocks: ChunkTicks::from_saved(second, vec![]).unwrap(),
                fluids: ChunkTicks::from_saved(second, vec![]).unwrap(),
            },
        ),
    ]);
    let mut region = RegionTicks::new(chunks, i64::MAX, i64::MAX - 1).unwrap();
    assert_eq!(
        region
            .schedule(TickRegistry::Block, "stone", a, 1, -1)
            .unwrap(),
        ScheduleOutcome::Duplicate
    );
    assert_eq!(
        region
            .schedule(TickRegistry::Fluid, "water", [-16, 3, 50], 3, 1)
            .unwrap(),
        ScheduleOutcome::Added
    );
    assert_eq!(
        region
            .schedule(TickRegistry::Block, "stone", [0, 0, 50], 0, 0)
            .unwrap(),
        ScheduleOutcome::MissingChunk
    );
    assert_eq!(
        region
            .schedule(TickRegistry::Block, "sand", b, -1, -1)
            .unwrap(),
        ScheduleOutcome::Added
    );
    out.push(format_saved(
        "region-block",
        region.chunk(pos).unwrap().blocks.pack(i64::MAX),
    ));
    out.push(format_saved(
        "region-fluid",
        region.chunk(second).unwrap().fluids.pack(i64::MAX),
    ));
    out.push(format!("counter {}", region.next_sub_tick()));
    let activated = region.activated_snapshot();
    assert_eq!(activated.game_time, i64::MAX);
    assert_eq!(activated.next_sub_tick, region.next_sub_tick());
    for tick in &activated.chunks[&pos].blocks {
        out.push(format_tick(Some(tick.clone())));
    }
    for tick in &activated.chunks[&second].fluids {
        out.push(format_tick(Some(tick.clone())));
    }
    assert_eq!(region.activated_snapshot(), activated);
    assert_eq!(
        region.chunk(pos).unwrap().blocks.pack(i64::MAX)[0],
        saved("minecraft:stone", a, 0, 0)
    );
    out.extend(collection_trace());
    out.extend(indexed_collection_trace());
    out.extend(callback_reschedule_trace());
    out.extend(streamed_containers_trace());
    out.extend(captured_index_history_trace());
    out
}

fn captured_index_history_trace() -> Vec<String> {
    let positions: Vec<_> = (1..)
        .filter(|&x| {
            let h = (x as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
            let h = h ^ (h >> 32);
            (h ^ (h >> 16)) & 31 == 31
        })
        .take(30)
        .map(|x| ChunkPos { x, z: 0 })
        .collect();
    let chunks = positions
        .iter()
        .enumerate()
        .map(|(i, &pos)| {
            let pending = saved(&format!("minecraft:hp{i}"), [pos.x * 16, 0, 0], 0, 0);
            (
                pos,
                ConvertedChunkTicks {
                    blocks: ChunkTicks::from_saved(pos, vec![pending]).unwrap(),
                    fluids: ChunkTicks::from_saved(pos, vec![]).unwrap(),
                },
            )
        })
        .collect();
    let mut region = RegionTicks::new_ordered(chunks, 100, 0, &positions).unwrap();
    for j in 0..30 {
        let i = (j * 17) % 30;
        region
            .schedule(
                TickRegistry::Block,
                &format!("minecraft:hq{i}"),
                [positions[i].x * 16, 1, 0],
                1,
                0,
            )
            .unwrap();
    }
    let activation: Vec<_> = positions.iter().rev().copied().collect();
    let mut captured = region
        .captured_indexed_queue(TickRegistry::Block, &activation)
        .unwrap();
    let mut rebuilt = region
        .activated_indexed_queue(TickRegistry::Block, &positions)
        .unwrap();
    let first = captured.collect_due(100, 3, |_| true);
    let reconstructed_first = rebuilt.collect_due(100, 3, |_| true);
    assert_ne!(
        first, reconstructed_first,
        "registration history must affect this fixture"
    );
    let mut output: Vec<_> = first
        .into_iter()
        .map(|tick| format!("H1 {}", tick.kind))
        .collect();
    for _ in 1..10 {
        for tick in captured.collect_due(100, 3, |_| true) {
            output.push(format!("H1 {}", tick.kind));
        }
    }
    for tick in captured.collect_due(101, 100, |_| true) {
        output.push(format!("H2 {}", tick.kind));
    }
    output
}

fn streamed_containers_trace() -> Vec<String> {
    let mut queue = IndexedRegionQueue::default();
    let position = |i| ChunkPos { x: i, z: 0 };
    for i in 0..10 {
        let pos = position(i);
        let mut ticks = ChunkTicks::from_saved(
            pos,
            vec![saved(&format!("minecraft:p{i}"), [i * 16, 0, 0], 0, 0)],
        )
        .unwrap();
        ticks
            .schedule(scheduled(
                &format!("minecraft:q{i}"),
                [i * 16, 1, 0],
                100,
                0,
                0,
            ))
            .unwrap();
        assert!(queue.add_container(pos, ticks).unwrap().is_none());
        queue.unpack_container(pos, 100).unwrap();
    }
    let mut output = Vec::new();
    for tick in queue.collect_due(100, 3, |_| true) {
        output.push(format!("U1 {}", tick.kind));
    }
    let mut removed = std::collections::BTreeMap::new();
    for i in [1, 4, 7] {
        removed.insert(i, queue.remove_container(position(i)).unwrap());
    }
    for tick in queue.collect_due(100, 4, |_| true) {
        output.push(format!("U2 {}", tick.kind));
    }
    for i in [7, 1, 4] {
        queue
            .add_container(position(i), removed.remove(&i).unwrap())
            .unwrap();
    }
    for tick in queue.collect_due(100, 100, |_| true) {
        output.push(format!("U3 {}", tick.kind));
    }
    let mut replaced = IndexedRegionQueue::default();
    let first = ChunkPos { x: 0, z: 1 };
    let second = ChunkPos { x: 1, z: 1 };
    let mut original = ChunkTicks::from_saved(first, vec![]).unwrap();
    original
        .schedule(scheduled("old", [0, 0, 16], 2, 0, 0))
        .unwrap();
    let mut other = ChunkTicks::from_saved(second, vec![]).unwrap();
    other
        .schedule(scheduled("other", [16, 0, 16], 2, 0, 0))
        .unwrap();
    replaced.add_container(first, original).unwrap();
    replaced.add_container(second, other).unwrap();
    replaced
        .add_container(first, ChunkTicks::from_saved(first, vec![]).unwrap())
        .unwrap();
    for tick in replaced.collect_due(2, 100, |_| true) {
        output.push(format!("X1 {}", tick.kind));
    }
    replaced
        .schedule(scheduled("new", [0, 1, 16], 3, 0, 2))
        .unwrap();
    for tick in replaced.collect_due(3, 100, |_| true) {
        output.push(format!("X2 {}", tick.kind));
    }
    output
}

fn callback_reschedule_trace() -> Vec<String> {
    let pos = ChunkPos { x: 0, z: 0 };
    let mut fluids = ChunkTicks::from_saved(pos, vec![]).unwrap();
    fluids
        .schedule(scheduled("water", [0, 0, 0], 0, 0, 0))
        .unwrap();
    fluids
        .schedule(scheduled("water", [1, 0, 0], 0, 0, 1))
        .unwrap();
    let region = RegionTicks::new(
        std::collections::BTreeMap::from([(
            pos,
            ConvertedChunkTicks {
                blocks: ChunkTicks::from_saved(pos, vec![]).unwrap(),
                fluids,
            },
        )]),
        0,
        2,
    )
    .unwrap();
    let mut queue = region
        .activated_indexed_queue(TickRegistry::Fluid, &[pos])
        .unwrap();
    let mut output = Vec::new();
    for tick in queue.collect_due(0, 10, |_| true) {
        output.push(format!("D1 {}", tick.position[0]));
        if tick.position[0] == 0 {
            assert_eq!(
                queue
                    .schedule(scheduled("water", [1, 0, 0], 0, 0, 2))
                    .unwrap(),
                ScheduleOutcome::Added
            );
        }
    }
    for tick in queue.collect_due(0, 10, |_| true) {
        output.push(format!("D2 {}", tick.position[0]));
    }
    output
}

fn indexed_collection_trace() -> Vec<String> {
    let positions: Vec<_> = (0..20)
        .map(|i| ChunkPos {
            x: i - 10,
            z: (i * 7) % 5 - 2,
        })
        .collect();
    let chunks = positions
        .iter()
        .copied()
        .enumerate()
        .map(|(i, pos)| {
            let mut blocks = ChunkTicks::from_saved(pos, vec![]).unwrap();
            for (suffix, y) in [("a", 0), ("b", 1)] {
                blocks
                    .schedule(scheduled(
                        &format!("minecraft:t{i}{suffix}"),
                        [pos.x * 16, y, pos.z * 16],
                        5,
                        0,
                        0,
                    ))
                    .unwrap();
            }
            (
                pos,
                ConvertedChunkTicks {
                    blocks,
                    fluids: ChunkTicks::from_saved(pos, vec![]).unwrap(),
                },
            )
        })
        .collect();
    let region = RegionTicks::new(chunks, 5, 1).unwrap();
    let mut queue = region
        .activated_indexed_queue(TickRegistry::Block, &positions)
        .unwrap();
    let mut output = Vec::new();
    for round in 0..14 {
        for tick in queue.collect_due(5, 3, |_| true) {
            output.push(format!("E{round} {}", tick.kind));
            if tick.kind == "minecraft:t0a" {
                queue
                    .schedule(scheduled("callback", [-160, 2, -32], 5, -1, -1))
                    .unwrap();
            }
        }
    }
    output
}

fn collection_trace() -> Vec<String> {
    let positions = [
        ChunkPos { x: 0, z: 0 },
        ChunkPos { x: 1, z: 0 },
        ChunkPos { x: 2, z: 0 },
    ];
    let chunks = positions
        .into_iter()
        .map(|pos| {
            (
                pos,
                ConvertedChunkTicks {
                    blocks: ChunkTicks::from_saved(pos, vec![]).unwrap(),
                    fluids: ChunkTicks::from_saved(pos, vec![]).unwrap(),
                },
            )
        })
        .collect();
    let mut region = RegionTicks::new(chunks, 42, 8).unwrap();
    for tick in [
        scheduled("a0", [0, 0, 0], 40, 0, 0),
        scheduled("a1", [1, 0, 0], 41, -3, 4),
        scheduled("b0", [16, 0, 0], 42, -1, 1),
        scheduled("c0", [32, 0, 0], 41, 1, 2),
        scheduled("future", [33, 0, 0], 50, 0, 7),
    ] {
        let pos = ChunkPos {
            x: tick.position[0].div_euclid(16),
            z: 0,
        };
        region
            .chunk_mut(pos)
            .unwrap()
            .blocks
            .schedule(tick)
            .unwrap();
    }
    let mut active = region.activated_queue(TickRegistry::Block);
    let mut output = Vec::new();
    for tick in active.collect_due(&positions, 42, 2).unwrap() {
        output.push(format!("R1 {}", tick.kind));
        if tick.kind == "minecraft:b0" {
            active
                .schedule(scheduled("new", [17, 0, 0], 42, -3, 5))
                .unwrap();
        }
    }
    for tick in active.collect_due(&positions, 42, 100).unwrap() {
        output.push(format!("R2 {}", tick.kind));
    }
    for tick in active.collect_due(&positions, 50, 100).unwrap() {
        output.push(format!("R3 {}", tick.kind));
    }
    output
}

#[test]
fn pending_ticks_block_duplicates_and_validate_chunk_inputs() {
    let pos = ChunkPos { x: -2, z: 3 };
    assert!(ChunkTicks::from_saved(pos, vec![saved("water", [-33, 0, 50], 0, 0)]).is_err());
    assert!(ChunkTicks::from_saved(pos, vec![saved("water", [-31, 0, 50], 0, 4)]).is_err());
    assert!(ChunkTicks::from_saved(pos, vec![saved("bad id", [-31, 0, 50], 0, 0)]).is_err());
    let lines = trace();
    assert_eq!(lines.len(), 231);
}

#[test]
fn captured_index_rejects_incomplete_activation_and_untracked_mutation() {
    let pos = ChunkPos { x: 0, z: 0 };
    let chunks = std::collections::BTreeMap::from([(
        pos,
        ConvertedChunkTicks {
            blocks: ChunkTicks::from_saved(pos, vec![]).unwrap(),
            fluids: ChunkTicks::from_saved(pos, vec![]).unwrap(),
        },
    )]);
    let mut region = RegionTicks::new(chunks, 0, 0).unwrap();
    assert_eq!(region.registration_order(), &[pos]);
    assert!(
        region
            .captured_indexed_queue(TickRegistry::Block, &[])
            .is_err()
    );
    assert!(
        region
            .captured_indexed_queue(TickRegistry::Block, &[pos, pos])
            .is_err()
    );
    assert!(region.chunk_mut(pos).is_some());
    assert!(
        region
            .captured_indexed_queue(TickRegistry::Block, &[pos])
            .is_err()
    );
    assert_eq!(captured_index_history_trace().len(), 60);
}

#[test]
fn captured_index_can_activate_only_resident_containers() {
    let home = ChunkPos { x: 0, z: 0 };
    let neighbor = ChunkPos { x: 1, z: 0 };
    let make = |pos: ChunkPos| ConvertedChunkTicks {
        blocks: ChunkTicks::from_saved(pos, vec![]).unwrap(),
        fluids: ChunkTicks::from_saved(pos, vec![saved("water", [pos.x * 16, 0, 0], 0, 0)])
            .unwrap(),
    };
    let region = RegionTicks::new_ordered(
        std::collections::BTreeMap::from([(home, make(home)), (neighbor, make(neighbor))]),
        42,
        0,
        &[home, neighbor],
    )
    .unwrap();
    let mut queue = region
        .captured_indexed_queue_for(TickRegistry::Fluid, &[home])
        .unwrap();
    assert_eq!(queue.collect_due(42, 100, |_| true)[0].position, [0, 0, 0]);
    assert!(queue.collect_due(42, 100, |_| true).is_empty());
    queue.unpack_container(neighbor, 42).unwrap();
    assert_eq!(queue.collect_due(42, 100, |_| true)[0].position, [16, 0, 0]);
    assert!(
        region
            .captured_indexed_queue_for(TickRegistry::Fluid, &[home, home])
            .is_err()
    );
}

#[test]
fn streamed_containers_keep_ticks_and_reconcile_replacements() {
    let trace = streamed_containers_trace();
    assert_eq!(trace.len(), 22);
    assert_eq!(&trace[20..], ["X1 minecraft:other", "X2 minecraft:new"]);
    let mut queue = IndexedRegionQueue::default();
    let first = ChunkPos { x: 0, z: 0 };
    let second = ChunkPos { x: 1, z: 0 };
    queue
        .add_container(first, ChunkTicks::from_saved(first, vec![]).unwrap())
        .unwrap();
    assert!(
        queue
            .add_container(first, ChunkTicks::from_saved(second, vec![]).unwrap())
            .is_err()
    );
    assert!(queue.container(first).is_some());
    assert!(queue.unpack_container(second, 0).is_err());
    assert!(queue.remove_container(second).is_none());
    assert_eq!(
        queue
            .schedule(scheduled("water", [16, 0, 0], 0, 0, 0))
            .unwrap(),
        ScheduleOutcome::MissingChunk
    );
}

#[test]
fn collected_ticks_can_be_scheduled_again_during_callbacks() {
    assert_eq!(callback_reschedule_trace(), ["D1 0", "D1 1", "D2 1"]);
}

#[test]
fn indexed_collection_requeues_ties_and_callback_ticks() {
    let trace = indexed_collection_trace();
    assert_eq!(trace.len(), 41);
    assert_eq!(
        trace
            .iter()
            .filter(|line| line.ends_with("minecraft:callback"))
            .count(),
        1
    );
}

#[test]
fn collecting_ticks_respects_chunk_heads_limits_and_callback_boundary() {
    assert_eq!(
        collection_trace(),
        [
            "R1 minecraft:b0",
            "R1 minecraft:a0",
            "R2 minecraft:a1",
            "R2 minecraft:new",
            "R2 minecraft:c0",
            "R3 minecraft:future",
        ]
    );
}

#[test]
fn collection_validates_traversal_and_preserves_java_heap_ties() {
    let positions = [
        ChunkPos { x: 0, z: 0 },
        ChunkPos { x: 1, z: 0 },
        ChunkPos { x: 2, z: 0 },
    ];
    let chunks = positions
        .into_iter()
        .map(|pos| {
            let mut blocks = ChunkTicks::from_saved(pos, vec![]).unwrap();
            blocks
                .schedule(scheduled("stone", [pos.x * 16, 0, 0], 0, 0, 0))
                .unwrap();
            (
                pos,
                ConvertedChunkTicks {
                    blocks,
                    fluids: ChunkTicks::from_saved(pos, vec![]).unwrap(),
                },
            )
        })
        .collect();
    let region = RegionTicks::new(chunks, 0, 1).unwrap();
    let mut queue = region.activated_queue(TickRegistry::Block);
    assert!(
        queue
            .collect_due(&[positions[0], positions[0]], 0, 10)
            .is_err()
    );
    assert!(
        queue
            .collect_due(&[ChunkPos { x: 3, z: 0 }], 0, 10)
            .is_err()
    );
    assert!(queue.collect_due(&positions, 0, 0).unwrap().is_empty());
    assert_eq!(
        queue
            .collect_due(&positions, 0, 10)
            .unwrap()
            .iter()
            .map(|t| t.position[0])
            .collect::<Vec<_>>(),
        [0, 32, 16]
    );
    let mut queue = region.activated_queue(TickRegistry::Block);
    assert_eq!(queue.collect_due(&positions[..1], 0, 10).unwrap().len(), 1);
    assert_eq!(queue.collect_due(&positions[1..], 0, 10).unwrap().len(), 2);
}

#[test]
fn activation_snapshot_keeps_pending_duplicates_and_new_subtick_order() {
    let pos = ChunkPos { x: 0, z: 0 };
    let repeated = saved("water", [1, 0, 1], 5, 0);
    let mut ticks = ChunkTicks::from_saved(pos, vec![repeated.clone(), repeated]).unwrap();
    assert!(
        ticks
            .schedule(scheduled("lava", [2, 0, 1], 105, 0, 7))
            .unwrap()
    );
    let before = ticks.pack(100);
    let active = ticks.activated(100);
    assert_eq!(
        active.iter().map(|tick| tick.sub_tick).collect::<Vec<_>>(),
        [-2, -1, 7]
    );
    assert_eq!(ticks.pack(100), before);
    assert_eq!(ticks.activated(100), active);
}

#[test]
fn postprocessing_callback_schedules_into_neighbor_chunk() {
    use anyhow::Result;
    use native_core::postprocessing::PostProcessing;
    use native_session::worldgen::postprocessing::{
        PostprocessingChunk, PostprocessingTask, PostprocessingWorld,
    };
    use std::{collections::BTreeMap, sync::Arc};

    struct Callback<'a>(&'a mut RegionTicks);
    impl PostprocessingWorld for Callback<'_> {
        type State = ();
        type Fluid = ();
        fn block(&mut self, _: [i32; 3]) -> Result<()> {
            Ok(())
        }
        fn fluid(&mut self, _: &()) -> Result<Option<()>> {
            Ok(None)
        }
        fn tick_fluid(&mut self, _: [i32; 3], _: &(), _: &()) -> Result<()> {
            unreachable!()
        }
        fn is_liquid_block(&mut self, _: &()) -> Result<bool> {
            Ok(false)
        }
        fn tick_liquid_block(&mut self, _: [i32; 3], _: &()) -> Result<()> {
            unreachable!()
        }
        fn update_neighbor_shapes(&mut self, _: [i32; 3], _: &()) -> Result<()> {
            assert_eq!(
                self.0
                    .schedule(TickRegistry::Block, "sand", [-16, 0, 50], 2, 0)?,
                ScheduleOutcome::Added
            );
            assert_eq!(
                self.0
                    .schedule(TickRegistry::Fluid, "water", [-16, 0, 50], 5, 0)?,
                ScheduleOutcome::Added
            );
            Ok(())
        }
        fn set_block(&mut self, _: [i32; 3], _: &(), _: i32) -> Result<()> {
            unreachable!()
        }
        fn clear_processed_section(&mut self, _: usize) -> Result<()> {
            Ok(())
        }
    }
    impl PostprocessingChunk for Callback<'_> {
        fn pending_block_entity_positions(&mut self) -> Result<Vec<[i32; 3]>> {
            Ok(vec![])
        }
        fn promote_pending_block_entity(&mut self, _: [i32; 3]) -> Result<()> {
            unreachable!()
        }
        fn clear_pending_block_entities(&mut self) -> Result<()> {
            Ok(())
        }
        fn apply_upgrade_data(&mut self) -> Result<()> {
            Ok(())
        }
    }
    let a = ChunkPos { x: -2, z: 3 };
    let b = ChunkPos { x: -1, z: 3 };
    let empty = |pos| ConvertedChunkTicks {
        blocks: ChunkTicks::from_saved(pos, vec![]).unwrap(),
        fluids: ChunkTicks::from_saved(pos, vec![]).unwrap(),
    };
    let mut ticks =
        RegionTicks::new(BTreeMap::from([(a, empty(a)), (b, empty(b))]), 100, 8).unwrap();
    let mut marks = PostProcessing::new(-16, 32).unwrap();
    marks.mark(-17, 0, 50);
    let mut task = PostprocessingTask::new(Arc::new(marks), a);
    while !task.poll(&mut Callback(&mut ticks), 1).unwrap() {}
    let neighbor = ticks.chunk(b).unwrap();
    assert_eq!(neighbor.blocks.peek().unwrap().trigger, 102);
    assert_eq!(neighbor.blocks.peek().unwrap().sub_tick, 8);
    assert_eq!(neighbor.fluids.peek().unwrap().trigger, 105);
    assert_eq!(neighbor.fluids.peek().unwrap().sub_tick, 9);
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn converted_tick_queue_matches_java_26_3() {
    use std::{path::PathBuf, process::Command};
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
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
    let source = temp.path().join("GeneratedChunkTicksOracle.java");
    std::fs::write(&source, include_str!("java/GeneratedChunkTicksOracle.java")).unwrap();
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(source)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(cp)
        .arg("GeneratedChunkTicksOracle")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let actual = String::from_utf8(output.stdout).unwrap();
    let expected = trace();
    let diagnostic = "Trying to schedule tick in not loaded position BlockPos{x=0, y=0, z=50}";
    assert_eq!(
        actual
            .lines()
            .filter(|line| line.contains(diagnostic))
            .count(),
        1
    );
    let actual = actual
        .lines()
        .filter(|line| !line.contains(diagnostic))
        .collect::<Vec<_>>();
    assert_eq!(
        actual.len(),
        expected.len(),
        "unexpected Java output length"
    );
    for (index, (actual, expected)) in actual.iter().zip(expected.iter()).enumerate() {
        assert_eq!(*actual, expected, "trace line {index}");
    }
}
