//! Compare the wall arm and post calculation with Minecraft Java 26.3.
use native_world::wall::{self, Above, Side};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn wall_sides_and_post_match_vanilla() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
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
    let java = std::env::var_os("MINECRAFT_JAVA_HOME")
        .or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let source = temp.path().join("WallShapeOracle.java");
    let output = temp.path().join("walls.bin");
    std::fs::write(&source, include_str!("java/WallShapeOracle.java")).unwrap();
    for command in [
        Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source),
        Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("WallShapeOracle")
            .arg(&output),
    ] {
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "wall oracle failed:\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let results = std::fs::read(output).unwrap();
    assert_eq!(
        results.len(),
        16 * 32 * 3 * 3 + 81 * 2 * 2 * (3 + 2 * 3 * 16 * 2)
    );
    let mut index = 0;
    for connection in 0..16 {
        for shape in 0..32 {
            for top in 0..3 {
                let flags = results[index];
                let expected_sides = results[index + 1];
                let expected_up = results[index + 2] != 0;
                let actual = wall::shape(
                    std::array::from_fn(|i| connection & (1 << i) != 0),
                    Above {
                        sides_covered: std::array::from_fn(|i| flags & (1 << i) != 0),
                        post_covered: flags & 16 != 0,
                        wall_post: top == 2,
                        post_override: false,
                    },
                );
                let packed = actual.sides.iter().enumerate().fold(0, |value, (i, side)| {
                    value
                        | (match side {
                            Side::None => 0,
                            Side::Low => 1,
                            Side::Tall => 2,
                        } << (2 * i))
                });
                assert_eq!(
                    (packed, actual.up),
                    (expected_sides, expected_up),
                    "connections={connection} shape={shape} top={top} coverage={flags}"
                );
                index += 3;
            }
        }
    }
    for code in 0..81 {
        for post_up in 0..2 {
            for wet in 0..2 {
                let mut remaining = code;
                let mut properties = std::collections::BTreeMap::new();
                for direction in ["north", "east", "south", "west"] {
                    properties.insert(
                        direction.into(),
                        ["none", "low", "tall"][remaining % 3].into(),
                    );
                    remaining /= 3;
                }
                properties.insert("up".into(), (post_up != 0).to_string());
                properties.insert("waterlogged".into(), (wet != 0).to_string());
                let state = native_world::loaded::BlockState {
                    name: "minecraft:stone_brick_wall".into(),
                    properties,
                };
                let fluid = native_world::fluid::FluidState::from_block(&state).unwrap();
                assert_eq!(fluid.is_some_and(|fluid| fluid.source), results[index] != 0);
                index += 1;
                for (name, props) in [
                    ("minecraft:air", vec![]),
                    ("minecraft:water", vec![("level".into(), "0".into())]),
                ] {
                    let other = native_world::loaded::BlockState {
                        name: name.into(),
                        properties: props.into_iter().collect(),
                    };
                    assert_eq!(
                        !native_world::block_light::same_light_behavior(&other, &state).unwrap(),
                        results[index] != 0,
                        "light mismatch for {state:?}"
                    );
                    index += 1;
                }
                for (boxes, heights) in [
                    (
                        native_world::collision::block_shape(&state).unwrap(),
                        [7.5, 15.5, 23.5],
                    ),
                    (
                        native_world::collision::block_outline(&state).unwrap(),
                        [7.5, 14.5, 15.5],
                    ),
                ] {
                    for height in heights {
                        for z in 0..16 {
                            let mut mask = 0u16;
                            for x in 0..16 {
                                let p = glam::Vec3::new(
                                    (x as f32 + 0.5) / 16.0,
                                    height / 16.0,
                                    (z as f32 + 0.5) / 16.0,
                                );
                                if boxes
                                    .iter()
                                    .any(|b| p.cmpgt(b.min).all() && p.cmplt(b.max).all())
                                {
                                    mask |= 1 << x;
                                }
                            }
                            let expected = u16::from_be_bytes([results[index], results[index + 1]]);
                            assert_eq!(
                                mask, expected,
                                "code={code} post={post_up} wet={wet} height={height} z={z}"
                            );
                            index += 2;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(index, results.len());
}
