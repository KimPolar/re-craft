//! Compare stair placement states with the installed Java 26.3 client.

use glam::IVec3;
use native_world::stairs::{Neighbor, Stair};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn stair_placement_states_match_vanilla() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
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
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let source = temp.path().join("StairPlacementOracle.java");
    let output = temp.path().join("placements.bin");
    let items = temp.path().join("stairs.txt");
    std::fs::write(&source, include_str!("java/StairPlacementOracle.java")).unwrap();
    for command in [
        Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source),
        Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("StairPlacementOracle")
            .arg(&output)
            .arg(&items),
    ] {
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "stair placement oracle failed:\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let rows = std::fs::read(output).unwrap();
    let registry = std::fs::read_to_string(items).unwrap();
    assert_eq!(registry.lines().count(), 97);
    for line in registry.lines() {
        let (id, max) = line.split_once(' ').unwrap();
        assert_eq!(
            native_session::inventory::ordinary_item_default_max(id),
            Some(max.parse().unwrap()),
            "{id}"
        );
    }
    assert_eq!(rows.len(), 4 * 6 * 4 * 4 * 33 * 4);
    let yaw = [
        0.0,
        std::f32::consts::FRAC_PI_2,
        std::f32::consts::PI,
        -std::f32::consts::FRAC_PI_2,
    ];
    let faces = [
        IVec3::NEG_Y,
        IVec3::Y,
        IVec3::NEG_Z,
        IVec3::Z,
        IVec3::NEG_X,
        IVec3::X,
    ];
    let heights = [0.25, 0.5, f64::from_bits(0.5_f64.to_bits() + 1), 0.75];
    let horizontal = [IVec3::NEG_Z, IVec3::Z, IVec3::NEG_X, IVec3::X];
    let names = [
        "straight",
        "inner_left",
        "inner_right",
        "outer_left",
        "outer_right",
    ];
    let mut index = 0;
    for angle in yaw {
        for face in faces {
            for height in heights {
                for fluid in 0..4 {
                    for scenario in 0..33 {
                        let mut neighbors = [Neighbor::Other; 4];
                        if scenario > 0 {
                            let n = scenario - 1;
                            neighbors[n / 8] = Neighbor::Stair(Stair {
                                facing: horizontal[(n % 8) / 2],
                                top: n % 2 != 0,
                            });
                        }
                        let actual = native_session::stair_placement::placement(
                            "minecraft:oak_stairs",
                            angle,
                            face,
                            height,
                            0,
                            fluid == 1,
                            |direction| {
                                neighbors[horizontal.iter().position(|d| *d == direction).unwrap()]
                            },
                        )
                        .unwrap();
                        let expected_facing = match rows[index] {
                            2 => "north",
                            3 => "south",
                            4 => "west",
                            5 => "east",
                            other => panic!("bad facing {other}"),
                        };
                        assert_eq!(actual.properties["facing"], expected_facing, "row {index}");
                        assert_eq!(
                            actual.properties["half"],
                            if rows[index + 1] == 1 {
                                "top"
                            } else {
                                "bottom"
                            },
                            "row {index}"
                        );
                        assert_eq!(
                            actual.properties["shape"],
                            names[rows[index + 2] as usize],
                            "row {index}"
                        );
                        assert_eq!(
                            actual.properties["waterlogged"],
                            if rows[index + 3] == 1 {
                                "true"
                            } else {
                                "false"
                            },
                            "row {index}"
                        );
                        index += 4;
                    }
                }
            }
        }
    }
    eprintln!(
        "Compared {} stair placements with Minecraft 26.3",
        index / 4
    );
}
