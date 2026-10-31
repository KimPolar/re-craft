//! Compare stair shape selection with the installed Java 26.3 code.

use glam::IVec3;
use native_world::stairs::{self, Neighbor, Stair};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn stair_neighbor_shapes_and_updates_match_vanilla() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
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
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let source = temp.path().join("StairShapeOracle.java");
    let output = temp.path().join("shapes.bin");
    std::fs::write(&source, include_str!("java/StairShapeOracle.java")).unwrap();
    for command in [
        Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source),
        Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("StairShapeOracle")
            .arg(&output),
    ] {
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "stair oracle failed:\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let results = std::fs::read(output).unwrap();
    assert_eq!(results.len(), 4 * 2 * 6561 + 2 * 6 * 3);
    let directions = [IVec3::NEG_Z, IVec3::Z, IVec3::NEG_X, IVec3::X];
    let names = [
        "straight",
        "inner_left",
        "inner_right",
        "outer_left",
        "outer_right",
    ];
    let mut index = 0;
    for facing in 0..4 {
        for half in 0..2 {
            for code in 0..6561 {
                let mut n = code;
                let mut neighbors = [Neighbor::Other; 4];
                for item in &mut neighbors {
                    let value = n % 9;
                    n /= 9;
                    if value != 0 {
                        *item = Neighbor::Stair(Stair {
                            facing: directions[(value - 1) / 2],
                            top: (value - 1) % 2 != 0,
                        });
                    }
                }
                let current = Stair {
                    facing: directions[facing],
                    top: half != 0,
                };
                let shape = stairs::shape(current, |direction| {
                    neighbors[directions.iter().position(|d| *d == direction).unwrap()]
                })
                .unwrap();
                assert_eq!(
                    shape,
                    names[usize::from(results[index])],
                    "facing={facing} half={half} neighbors={code}"
                );
                index += 1;
            }
        }
    }
    for wet in 0..2 {
        for direction in 0..6 {
            assert_eq!(results[index], if direction < 2 { 2 } else { 3 });
            assert_eq!(results[index + 1], wet);
            assert_eq!(results[index + 2], if wet == 1 { 5 } else { 0 });
            index += 3;
        }
    }
    eprintln!("Compared 52,488 stair shapes and 12 neighbor updates with Minecraft 26.3");
}
