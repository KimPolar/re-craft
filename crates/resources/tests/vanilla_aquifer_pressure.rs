//! Exercise actual 26.3 aquifer pressure and fluid selection with controlled statuses.
use native_core::aquifer::{AquiferBlock, FluidStatus, pressure, resolve};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Block(i32);
impl AquiferBlock for Block {
    fn air() -> Self {
        Self(0)
    }
    fn is_water(self) -> bool {
        self.0 == 1
    }
    fn is_lava(self) -> bool {
        self.0 == 2
    }
}
fn status(level: i32, block: i32) -> FluidStatus<Block> {
    FluidStatus {
        level,
        block: Block(block),
    }
}
fn run(command: &mut Command) -> String {
    let output = command.output().expect("cannot run Java 25");
    assert!(
        output.status.success(),
        "aquifer oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn pressure_and_fluid_selection_match_vanilla() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("AquiferPressureOracle.java");
    std::fs::write(&source, include_str!("java/AquiferPressureOracle.java")).unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("AquiferPressureOracle"));
    let mut pressures = 0;
    let mut decisions = 0;
    let mut early = 0;
    let mut outcomes = [0; 5];
    let mut scheduled = 0;
    for line in output.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        let int = |i: usize| fields[i].parse::<i32>().unwrap();
        let bits = |i: usize| fields[i].parse::<i64>().unwrap() as u64;
        match fields.first().copied() {
            Some("E") => {
                let y = int(1);
                let mode = int(4);
                let result = resolve(
                    y,
                    f64::from_bits(bits(2)),
                    int(3),
                    [0; 4],
                    &mut |_| {
                        if mode == 1 {
                            status(i32::MAX, 2)
                        } else {
                            status(63, 1)
                        }
                    },
                    &mut |_| panic!("unexpected local sample"),
                    &mut || panic!("unexpected barrier sample"),
                );
                assert_eq!(
                    result.block.map_or(-1, |block| block.0),
                    int(5),
                    "early block: {line}"
                );
                assert_eq!(
                    result.schedule_fluid_update,
                    int(6) != 0,
                    "early update: {line}"
                );
                early += 1;
            }
            Some("R") => {
                let mut cache = f64::from_bits(bits(6));
                let value = pressure(
                    int(1),
                    status(int(2), int(3)),
                    status(int(4), int(5)),
                    &mut cache,
                    &mut || 0.25,
                );
                assert_eq!(value.to_bits(), bits(7), "pressure: {line}");
                assert_eq!(cache.to_bits(), bits(8), "barrier cache: {line}");
                pressures += 1;
            }
            Some("D") => {
                let y = int(1);
                let mode = int(4);
                let distance = [int(5), int(8), int(11), int(14)];
                let local = [
                    status(int(6), int(7)),
                    status(int(9), int(10)),
                    status(int(12), int(13)),
                    status(int(15), int(16)),
                ];
                let result = resolve(
                    y,
                    f64::from_bits(bits(2)),
                    int(3),
                    distance,
                    &mut |at| {
                        if mode == 1 || (mode == 2 && at < y) {
                            status(i32::MAX, 2)
                        } else {
                            status(63, 1)
                        }
                    },
                    &mut |i| local[i],
                    &mut || 0.25,
                );
                let actual = result.block.map_or(-1, |block| block.0);
                assert_eq!(actual, int(17), "block decision: {line}");
                assert_eq!(
                    result.schedule_fluid_update,
                    int(18) != 0,
                    "update decision: {line}"
                );
                outcomes[(actual + 1) as usize] += 1;
                scheduled += usize::from(result.schedule_fluid_update);
                decisions += 1;
            }
            _ => {}
        }
    }
    assert_eq!(pressures, 8 * 11 * 11 * 4 * 4 * 3);
    assert_eq!(decisions, 5000);
    assert_eq!(early, 21);
    assert!(
        outcomes.iter().all(|count| *count > 0),
        "missing block outcomes: {outcomes:?}"
    );
    assert!(scheduled > 0);
}
