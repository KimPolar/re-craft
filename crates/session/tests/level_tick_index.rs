use native_session::worldgen::tick_index::LevelTickIndex;
use native_world::loaded::ChunkPos;

fn key(i: i32) -> ChunkPos {
    ChunkPos {
        x: (i * 37) % 97 - 48,
        z: (i * 29) % 89 - 44,
    }
}

fn drain(map: &mut LevelTickIndex, round: i32, time: i64) -> String {
    let positions = map.take_due(time, |pos| {
        pos.x
            .wrapping_mul(17)
            .wrapping_add(pos.z.wrapping_mul(31))
            .wrapping_add(round)
            .rem_euclid(4)
            != 0
    });
    let mut line = format!("round {round}");
    for pos in positions {
        line.push_str(&format!(" {}:{}", pos.x, pos.z));
    }
    line
}

fn trace() -> Vec<String> {
    let mut map = LevelTickIndex::default();
    map.insert(ChunkPos { x: 0, z: 0 }, -100);
    map.insert(
        ChunkPos {
            x: i32::MIN,
            z: i32::MAX,
        },
        -100,
    );
    for i in 0..100 {
        map.insert(key(i), i64::from(i % 17 - 8));
    }
    for i in (0..30).step_by(3) {
        map.remove(key(i));
    }
    let mut output = Vec::new();
    for round in 0..8 {
        output.push(drain(&mut map, round, i64::from(round - 2)));
        for i in 0..31 {
            map.insert(
                key((round * 19 + i * 7) % 150),
                i64::from((i + round) % 13 - 6),
            );
        }
        for i in (0..11).step_by(2) {
            map.remove(key((round * 13 + i) % 150));
        }
    }
    output.push(drain(&mut map, 8, i64::MAX));
    let mut wrapped = LevelTickIndex::default();
    let mut count = 0;
    for x in 1.. {
        let h = (x as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let h = h ^ (h >> 32);
        if (h ^ (h >> 16)) & 31 == 31 {
            wrapped.insert(ChunkPos { x, z: 0 }, 0);
            count += 1;
            if count == 24 {
                break;
            }
        }
    }
    for round in 20..24 {
        output.push(drain(&mut wrapped, round, 0));
    }
    output
}

#[test]
fn index_updates_zero_key_and_retains_untickable_entries() {
    let mut map = LevelTickIndex::default();
    let zero = ChunkPos { x: 0, z: 0 };
    map.insert(zero, 5);
    map.insert(zero, 3);
    assert_eq!(map.get(zero), Some(3));
    assert!(map.take_due(3, |_| false).is_empty());
    assert_eq!(map.take_due(2, |_| true), []);
    assert_eq!(map.take_due(3, |_| true), [zero]);
    assert_eq!(map.get(zero), None);
    assert!(!map.remove(zero));
    assert_eq!(trace().len(), 13);
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn index_iteration_matches_java_fastutil_8_5_18() {
    use std::{path::PathBuf, process::Command};
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
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
    let source = temp.path().join("LevelTickIndexOracle.java");
    std::fs::write(&source, include_str!("java/LevelTickIndexOracle.java")).unwrap();
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
        .arg("LevelTickIndexOracle")
        .output()
        .unwrap();
    assert!(
        actual.status.success(),
        "{}",
        String::from_utf8_lossy(&actual.stderr)
    );
    assert_eq!(
        String::from_utf8(actual.stdout)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        trace()
    );
}
