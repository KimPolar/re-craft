use native_session::worldgen::chunk_ticks::{ChunkTicks, SavedTick, ScheduledTick};
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
    out
}

#[test]
fn pending_ticks_block_duplicates_and_validate_chunk_inputs() {
    let pos = ChunkPos { x: -2, z: 3 };
    assert!(ChunkTicks::from_saved(pos, vec![saved("water", [-33, 0, 50], 0, 0)]).is_err());
    assert!(ChunkTicks::from_saved(pos, vec![saved("water", [-31, 0, 50], 0, 4)]).is_err());
    assert!(ChunkTicks::from_saved(pos, vec![saved("bad id", [-31, 0, 50], 0, 0)]).is_err());
    let lines = trace();
    assert_eq!(lines.len(), 93);
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
    assert_eq!(
        actual.lines().collect::<Vec<_>>(),
        trace().iter().map(String::as_str).collect::<Vec<_>>()
    );
}
