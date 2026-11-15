use native_session::simulation_chunks::SimulationChunks;
use native_world::loaded::ChunkPos;

fn position(x: i32, z: i32) -> ChunkPos {
    ChunkPos { x, z }
}

fn emit(tracker: &SimulationChunks, step: usize, output: &mut Vec<String>) {
    let mut order = format!("TRACE {step}");
    for pos in tracker.entity_ticking_order() {
        order.push_str(&format!(" {}:{}", pos.x, pos.z));
    }
    output.push(order);
    let mut levels = format!("LEVEL {step} ");
    for z in -18..=18 {
        for x in -18..=24 {
            levels.push(char::from(b'A' + tracker.level(position(x, z))));
        }
    }
    output.push(levels);
}

fn trace(tracker: &mut SimulationChunks, step: usize, output: &mut Vec<String>) {
    tracker.run_updates();
    emit(tracker, step, output);
}

fn trace_history() -> Vec<String> {
    let mut tracker = SimulationChunks::default();
    let mut output = Vec::new();
    trace(&mut tracker, 0, &mut output);
    assert!(tracker.add_ticket(position(0, 0), 1, 26));
    assert!(!tracker.add_ticket(position(0, 0), 1, 26));
    trace(&mut tracker, 1, &mut output);
    tracker.add_ticket(position(8, 1), 2, 31);
    tracker.add_ticket(position(-7, -4), 1, 27);
    trace(&mut tracker, 2, &mut output);
    tracker.remove_ticket(position(0, 0), 1, 26);
    tracker.add_ticket(position(1, -1), 1, 26);
    trace(&mut tracker, 3, &mut output);
    tracker.add_ticket(position(1, -1), 2, 30);
    tracker.add_ticket(position(1, -1), 1, 28);
    tracker.remove_ticket(position(1, -1), 1, 26);
    trace(&mut tracker, 4, &mut output);
    tracker.remove_ticket(position(-7, -4), 1, 27);
    tracker.remove_ticket(position(8, 1), 2, 31);
    tracker.remove_ticket(position(1, -1), 2, 30);
    trace(&mut tracker, 5, &mut output);
    tracker.remove_ticket(position(1, -1), 1, 28);
    tracker.add_ticket(position(13, -12), 1, 19);
    trace(&mut tracker, 6, &mut output);
    tracker.remove_ticket(position(13, -12), 1, 19);
    trace(&mut tracker, 7, &mut output);
    for round in 0..20 {
        if round > 0 {
            let last = round - 1;
            tracker.remove_ticket(
                position((last * 7) % 21 - 10, (last * 11) % 19 - 9),
                1,
                31 - (5 + (last * 13) % 8),
            );
        }
        if round >= 3 && (round - 3) % 4 == 0 {
            let old = round - 3;
            tracker.remove_ticket(position((old * 7) % 21 - 8, (old * 11) % 19 - 12), 2, 31);
        }
        let x = (round * 7) % 21 - 10;
        let z = (round * 11) % 19 - 9;
        tracker.add_ticket(position(x, z), 1, 31 - (5 + (round * 13) % 8));
        if round % 4 == 0 {
            tracker.add_ticket(position(x + 2, z - 3), 2, 31);
        }
        trace(&mut tracker, (8 + round) as usize, &mut output);
    }
    tracker.add_ticket(position(-2, 3), 2, 18);
    for (index, budget) in [0, 1, 2, 7, 31, 111, 100_000].into_iter().enumerate() {
        let remaining = tracker.run_updates_budget(budget);
        output.push(format!(
            "BUDGET {index} {remaining} {}",
            tracker.has_pending_updates()
        ));
        emit(&tracker, 28 + index, &mut output);
    }
    output
}

#[test]
fn overlapping_ticket_history_returns_to_empty() {
    let output = trace_history();
    assert_eq!(output.len(), 77);
    assert_eq!(output[14], "TRACE 7");
    assert!(output[15].ends_with(&"b".repeat(43 * 37)));
}

#[cfg(windows)]
#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn propagation_and_iteration_match_java_26_3() {
    use std::{path::PathBuf, process::Command};

    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME")
        .or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join(
                "AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon",
            )
        })
        .join("bin");
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
    let source = temp.path().join("SimulationChunksOracle.java");
    std::fs::write(&source, include_str!("java/SimulationChunksOracle.java")).unwrap();
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
    let actual = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(cp)
        .arg("SimulationChunksOracle")
        .output()
        .unwrap();
    assert!(
        actual.status.success(),
        "{}",
        String::from_utf8_lossy(&actual.stderr)
    );
    let actual = String::from_utf8(actual.stdout).unwrap();
    let lines: Vec<_> = actual
        .lines()
        .map(|line| line.split_once("[STDOUT]: ").map_or(line, |(_, line)| line))
        .filter(|line| {
            line.starts_with("TRACE ") || line.starts_with("LEVEL ") || line.starts_with("BUDGET ")
        })
        .collect();
    let expected = trace_history();
    assert_eq!(lines.len(), expected.len(), "Java output: {actual}");
    for (actual, expected) in lines.into_iter().zip(expected) {
        assert_eq!(actual, expected);
    }
}
