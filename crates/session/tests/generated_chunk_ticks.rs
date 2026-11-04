use native_session::worldgen::chunk_ticks::{
    ChunkTicks, ConvertedChunkTicks, RegionTicks, SavedTick, ScheduleOutcome, ScheduledTick,
    TickRegistry,
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
    let first = &mut region.chunk_mut(pos).unwrap().blocks;
    first.unpack(i64::MAX);
    while first.peek().is_some() {
        out.push(format_tick(first.poll()));
    }
    let fluids = &mut region.chunk_mut(second).unwrap().fluids;
    fluids.unpack(i64::MAX);
    while fluids.peek().is_some() {
        out.push(format_tick(fluids.poll()));
    }
    out
}

#[test]
fn pending_ticks_block_duplicates_and_validate_chunk_inputs() {
    let pos = ChunkPos { x: -2, z: 3 };
    assert!(ChunkTicks::from_saved(pos, vec![saved("water", [-33, 0, 50], 0, 0)]).is_err());
    assert!(ChunkTicks::from_saved(pos, vec![saved("water", [-31, 0, 50], 0, 4)]).is_err());
    assert!(ChunkTicks::from_saved(pos, vec![saved("bad id", [-31, 0, 50], 0, 0)]).is_err());
    let lines = trace();
    assert_eq!(lines.len(), 99);
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
