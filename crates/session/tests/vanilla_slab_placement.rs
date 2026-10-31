//! Compare slab placement and replacement with the installed Java client.
use glam::IVec3;
use native_world::loaded::BlockState;
use serde_json::Value;
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn slab_placement_and_replacement_match_vanilla() {
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
    let source = temp.path().join("SlabPlacementOracle.java");
    let output = temp.path().join("placements.bin");
    let items = temp.path().join("slabs.txt");
    std::fs::write(&source, include_str!("java/SlabPlacementOracle.java")).unwrap();
    for command in [
        Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source),
        Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("SlabPlacementOracle")
            .arg(&output)
            .arg(&items),
    ] {
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "slab placement oracle failed:\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let rows = std::fs::read(output).unwrap();
    let registry = std::fs::read_to_string(items).unwrap();
    assert_eq!(registry.lines().count(), 101);
    for line in registry.lines() {
        let (id, max) = line.split_once(' ').unwrap();
        assert_eq!(
            native_session::inventory::ordinary_item_default_max(id),
            Some(max.parse().unwrap()),
            "{id}"
        );
    }
    assert_eq!(rows.len(), 5 * 2 * 2 * 6 * 4 * 4 * 3);
    let faces = [
        IVec3::NEG_Y,
        IVec3::Y,
        IVec3::NEG_Z,
        IVec3::Z,
        IVec3::NEG_X,
        IVec3::X,
    ];
    let heights = [0.25, 0.5, f64::from_bits(0.5_f64.to_bits() + 1), 0.75];
    let mut offset = 0;
    for initial in 0..5 {
        let initial_state = if initial == 0 {
            None
        } else {
            Some(BlockState {
                name: if initial == 4 {
                    "minecraft:stone_slab"
                } else {
                    "minecraft:oak_slab"
                }
                .into(),
                properties: [
                    (
                        "type".into(),
                        ["", "bottom", "top", "double", "bottom"][initial].into(),
                    ),
                    ("waterlogged".into(), "false".into()),
                ]
                .into_iter()
                .collect(),
            })
        };
        for same_item in [false, true] {
            for replacing in [false, true] {
                for face in faces {
                    for height in heights {
                        for fluid in 0..4 {
                            if let Some(existing) = &initial_state {
                                let item = if same_item {
                                    "minecraft:oak_slab"
                                } else {
                                    "minecraft:stone_slab"
                                };
                                let replace = native_session::slab_placement::can_replace(
                                    existing, item, replacing, face, height, 0,
                                )
                                .unwrap();
                                assert_eq!(
                                    replace,
                                    rows[offset] != 0,
                                    "initial={initial} same={same_item} replacing={replacing} face={face:?} y={height}"
                                );
                            }
                            let placed = native_session::slab_placement::placement(
                                "minecraft:oak_slab",
                                face,
                                height,
                                0,
                                fluid == 1,
                                (1..=3).contains(&initial),
                            )
                            .unwrap();
                            let kind = ["top", "bottom", "double"][rows[offset + 1] as usize];
                            assert_eq!(placed.properties["type"], kind, "row {offset}");
                            assert_eq!(
                                placed.properties["waterlogged"] == "true",
                                rows[offset + 2] != 0,
                                "row {offset}"
                            );
                            offset += 3;
                        }
                    }
                }
            }
        }
    }
    eprintln!(
        "Compared {} slab placement contexts with Java 26.3",
        offset / 3
    );
}
