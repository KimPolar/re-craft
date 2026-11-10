use native_persistence::{
    heightmap::{
        GeneratedHeightmaps, HeightBounds, SpawnHeightmapKind, SpawnHeightmaps,
        encode_generated_heightmaps, generated_full_heightmaps,
    },
    nbt::{self, NamedTag, Tag, TagType},
};
use std::{path::PathBuf, process::Command};

fn values(min: i32, height: i32, kind: i32) -> [i32; 256] {
    std::array::from_fn(|i| min + ((i as i32 * 37 + kind * 13) % (height + 1)))
}

fn fixture(min_y: i32, height: i32) -> anyhow::Result<NamedTag> {
    let maps = std::array::from_fn::<_, 6, _>(|i| values(min_y, height, i as i32));
    let data = GeneratedHeightmaps {
        world_surface: Some(&maps[0]),
        ocean_floor: Some(&maps[1]),
        motion_blocking: Some(&maps[2]),
        motion_blocking_no_leaves: Some(&maps[3]),
        world_surface_wg: Some(&maps[4]),
        ocean_floor_wg: Some(&maps[5]),
    };
    let heightmaps = encode_generated_heightmaps(
        HeightBounds {
            min_y,
            max_y: min_y + height,
        },
        &data,
    )?;
    Ok(NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![
            ("min_y".into(), Tag::Int(min_y)),
            ("height".into(), Tag::Int(height)),
            ("Heightmaps".into(), heightmaps),
        ]),
    })
}

#[test]
fn generated_maps_preserve_values_and_omit_unprimed_maps() {
    let bounds = HeightBounds {
        min_y: -64,
        max_y: 320,
    };
    let source = values(-64, 384, 2);
    let data = GeneratedHeightmaps {
        motion_blocking: Some(&source),
        ..Default::default()
    };
    let encoded = encode_generated_heightmaps(bounds, &data).unwrap();
    assert!(encoded.get("WORLD_SURFACE").is_none());
    assert!(encoded.get("OCEAN_FLOOR_WG").is_none());
    let root = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![("Heightmaps".into(), encoded)]),
    };
    let decoded = SpawnHeightmaps::read(&root, bounds).unwrap();
    for z in 0..16 {
        for x in 0..16 {
            assert_eq!(
                decoded.highest(SpawnHeightmapKind::MotionBlocking, x, z),
                Some(source[(x + 16 * z) as usize] - 1)
            );
            assert_eq!(
                decoded.highest(SpawnHeightmapKind::WorldSurface, x, z),
                None
            );
        }
    }
    let mut bad = source;
    bad[7] = 321;
    assert!(
        encode_generated_heightmaps(
            bounds,
            &GeneratedHeightmaps {
                motion_blocking: Some(&bad),
                ..Default::default()
            }
        )
        .is_err()
    );
    assert!(
        encode_generated_heightmaps(
            HeightBounds {
                min_y: 0,
                max_y: 15
            },
            &data
        )
        .is_err()
    );
}

#[test]
fn full_conversion_keeps_existing_values_and_initializes_missing_maps() {
    let bounds = HeightBounds {
        min_y: -64,
        max_y: 320,
    };
    let root = fixture(-64, 384).unwrap();
    let Tag::Compound(mut source) = root.value.get("Heightmaps").unwrap().clone() else {
        panic!("compound expected")
    };
    source.retain(|(name, _)| {
        !name.matches("OCEAN_FLOOR") && !name.matches("MOTION_BLOCKING_NO_LEAVES")
    });
    let source = Tag::Compound(source);
    let full = generated_full_heightmaps(bounds, &source).unwrap();
    for name in ["WORLD_SURFACE", "MOTION_BLOCKING"] {
        assert_eq!(full.get(name), source.get(name));
    }
    for name in ["OCEAN_FLOOR", "MOTION_BLOCKING_NO_LEAVES"] {
        assert!(
            matches!(full.get(name), Some(Tag::LongArray(words)) if words.iter().all(|&word| word == 0))
        );
    }
    assert!(full.get("WORLD_SURFACE_WG").is_none());
    assert!(full.get("OCEAN_FLOOR_WG").is_none());
    let Tag::Compound(mut duplicate) = source.clone() else {
        unreachable!()
    };
    duplicate.push(duplicate[0].clone());
    assert!(generated_full_heightmaps(bounds, &Tag::Compound(duplicate)).is_err());
    assert!(
        generated_full_heightmaps(
            bounds,
            &Tag::Compound(vec![("UNKNOWN".into(), Tag::LongArray(vec![]))])
        )
        .is_err()
    );
    assert!(
        generated_full_heightmaps(
            bounds,
            &Tag::Compound(vec![("WORLD_SURFACE".into(), Tag::LongArray(vec![]))])
        )
        .is_err()
    );
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn full_conversion_matches_java_level_chunk_constructor() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
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
    let classpath = std::env::join_paths(paths).unwrap();
    let java_source = temp.path().join("FullHeightmapOracle.java");
    std::fs::write(&java_source, include_str!("java/FullHeightmapOracle.java")).unwrap();
    let compile = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(&java_source)
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let mut cases = Vec::new();
    for (min_y, height) in [(-64, 384), (0, 16), (-2032, 4064)] {
        for mask in [0u8, 1, 0b1010, 0b111111] {
            let root = fixture(min_y, height).unwrap();
            let Tag::Compound(mut fields) = root.value.get("Heightmaps").unwrap().clone() else {
                unreachable!()
            };
            let mut index = 0;
            fields.retain(|_| {
                let keep = mask & (1 << index) != 0;
                index += 1;
                keep
            });
            let source = Tag::Compound(fields);
            let expected = generated_full_heightmaps(
                HeightBounds {
                    min_y,
                    max_y: min_y + height,
                },
                &source,
            )
            .unwrap();
            cases.push(Tag::Compound(vec![
                ("min_y".into(), Tag::Int(min_y)),
                ("height".into(), Tag::Int(height)),
                ("source".into(), source),
                ("expected".into(), expected),
            ]));
        }
    }
    let document = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![(
            "cases".into(),
            Tag::List {
                element_type: TagType::Compound,
                values: cases,
            },
        )]),
    };
    let input = temp.path().join("cases.nbt");
    std::fs::write(&input, nbt::write(&document).unwrap()).unwrap();
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg("FullHeightmapOracle")
        .arg(input)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn saved_generation_maps_match_java_protochunk() {
    use native_persistence::heightmap::{
        SavedGenerationHeightmapKind as Kind, SavedGenerationHeightmaps,
    };
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
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
    let classpath = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("SavedGenerationHeightmapOracle.java");
    std::fs::write(
        &source,
        include_str!("java/SavedGenerationHeightmapOracle.java"),
    )
    .unwrap();
    let compile = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let kinds = [
        Kind::WorldSurface,
        Kind::OceanFloor,
        Kind::MotionBlocking,
        Kind::MotionBlockingNoLeaves,
        Kind::WorldSurfaceWorldgen,
        Kind::OceanFloorWorldgen,
    ];
    let mut cases = Vec::new();
    for (min_y, height) in [(-64, 384), (0, 16), (-2032, 4064)] {
        for mask in [0u8, 0b101010, 0b111111] {
            let root = fixture(min_y, height).unwrap();
            let Tag::Compound(mut fields) = root.value.get("Heightmaps").unwrap().clone() else {
                unreachable!()
            };
            if height == 384
                && let Tag::LongArray(words) = &mut fields[0].1
            {
                words[0] = (words[0] & !511) | 500;
            }
            let mut index = 0;
            fields.retain(|_| {
                let keep = mask & (1 << index) != 0;
                index += 1;
                keep
            });
            let source = Tag::Compound(fields);
            let doc = NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![("Heightmaps".into(), source.clone())]),
            };
            let maps = SavedGenerationHeightmaps::read(
                &doc,
                HeightBounds {
                    min_y,
                    max_y: min_y + height,
                },
            )
            .unwrap();
            let names = [
                "WORLD_SURFACE",
                "OCEAN_FLOOR",
                "MOTION_BLOCKING",
                "MOTION_BLOCKING_NO_LEAVES",
                "WORLD_SURFACE_WG",
                "OCEAN_FLOOR_WG",
            ];
            let mut expected = Vec::new();
            for (kind, name) in kinds.into_iter().zip(names) {
                if maps.first_available(kind, 0, 0).is_some() {
                    let values = (0..256)
                        .map(|column| {
                            maps.first_available(kind, column % 16, column / 16)
                                .unwrap()
                        })
                        .collect();
                    expected.push((name.into(), Tag::IntArray(values)));
                }
            }
            cases.push(Tag::Compound(vec![
                ("min_y".into(), Tag::Int(min_y)),
                ("height".into(), Tag::Int(height)),
                ("source".into(), source),
                ("expected".into(), Tag::Compound(expected)),
            ]));
        }
    }
    let input = temp.path().join("cases.nbt");
    let document = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![(
            "cases".into(),
            Tag::List {
                element_type: TagType::Compound,
                values: cases,
            },
        )]),
    };
    std::fs::write(&input, nbt::write(&document).unwrap()).unwrap();
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg("SavedGenerationHeightmapOracle")
        .arg(input)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn partial_spawn_maps_preserve_existing_values_when_missing_maps_are_primed() {
    use native_persistence::heightmap::BlockHeightFlags;
    let bounds = HeightBounds {
        min_y: 0,
        max_y: 16,
    };
    let stale = [12; 256];
    let mut maps =
        SpawnHeightmaps::from_optional_first_available(bounds, None, Some(&stale), None).unwrap();
    assert!(!maps.is_complete());
    maps.prime_missing(|_, y, _| {
        Ok(BlockHeightFlags {
            is_air: y >= 4,
            blocks_motion: y < 4,
            blocks_motion_no_leaves: y < 4,
            has_fluid: false,
        })
    })
    .unwrap();
    assert!(maps.is_complete());
    assert_eq!(
        maps.highest(SpawnHeightmapKind::WorldSurface, 3, 7),
        Some(11)
    );
    assert_eq!(
        maps.highest(SpawnHeightmapKind::MotionBlocking, 3, 7),
        Some(3)
    );
    assert_eq!(maps.highest(SpawnHeightmapKind::OceanFloor, 3, 7), Some(3));
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn generated_maps_match_java_packed_storage() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: serde_json::Value =
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
    let source = temp.path().join("GeneratedHeightmapOracle.java");
    std::fs::write(&source, include_str!("java/GeneratedHeightmapOracle.java")).unwrap();
    let run = |cmd: &mut Command| {
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run(Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(source));
    for (index, (min_y, height)) in [(-64, 384), (0, 16), (-2032, 4064)].into_iter().enumerate() {
        let path = temp.path().join(format!("maps-{index}.nbt"));
        std::fs::write(&path, nbt::write(&fixture(min_y, height).unwrap()).unwrap()).unwrap();
        run(Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("GeneratedHeightmapOracle")
            .arg(path));
    }
}
