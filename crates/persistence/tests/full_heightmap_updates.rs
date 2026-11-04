use native_persistence::{
    heightmap::{
        BlockHeightFlags, FullHeightmaps, GeneratedHeightmaps, HeightBounds,
        encode_generated_heightmaps, generated_full_heightmaps,
    },
    nbt::{self, NamedTag, Tag, TagType},
};

fn flags(kind: u8, air_motion: bool) -> BlockHeightFlags {
    BlockHeightFlags {
        is_air: kind == 0,
        blocks_motion: kind == 1 || kind == 2 || (kind == 0 && air_motion),
        blocks_motion_no_leaves: kind == 1 || (kind == 0 && air_motion),
        has_fluid: kind == 3,
    }
}

fn maps(bounds: HeightBounds, seed: i32, mask: u8) -> Tag {
    let height = bounds.max_y - bounds.min_y;
    let values: [[i32; 256]; 4] = std::array::from_fn(|kind| {
        std::array::from_fn(|i| {
            bounds.min_y + ((i as i32 * 37 + kind as i32 * 13 + seed).rem_euclid(height + 1))
        })
    });
    let source = encode_generated_heightmaps(
        bounds,
        &GeneratedHeightmaps {
            world_surface: (mask & 1 != 0).then_some(&values[0]),
            ocean_floor: (mask & 2 != 0).then_some(&values[1]),
            motion_blocking: (mask & 4 != 0).then_some(&values[2]),
            motion_blocking_no_leaves: (mask & 8 != 0).then_some(&values[3]),
            ..Default::default()
        },
    )
    .unwrap();
    let mut full = generated_full_heightmaps(bounds, &source).unwrap();
    // The unused high bits belong to the original storage and must survive.
    let Tag::Compound(entries) = &mut full else {
        unreachable!()
    };
    for (_, tag) in entries {
        let Tag::LongArray(words) = tag else {
            unreachable!()
        };
        for word in words {
            *word |= i64::MIN;
        }
    }
    full
}

fn make_case(min: i32, height: i32, seed: i32, mask: u8, air_motion: bool) -> Tag {
    let bounds = HeightBounds {
        min_y: min,
        max_y: min + height,
    };
    let source = maps(bounds, seed, mask);
    let mut heightmaps = FullHeightmaps::from_converted(bounds, &source).unwrap();
    let mut blocks = vec![0u8; (height as usize) * 256];
    for y in 0..height {
        for z in 0..16 {
            for x in 0..16 {
                let hash = (x * 19 + y * 7 + z * 13 + seed).rem_euclid(13);
                blocks[(x + z * 16 + y * 256) as usize] = match hash {
                    0..=5 => 0,
                    6..=8 => 1,
                    9..=10 => 2,
                    11 => 3,
                    _ => 4,
                };
            }
        }
    }
    let original = blocks.clone();
    let mut edits = Vec::new();
    for step in 0..48i32 {
        let x = (step * 7 + seed).rem_euclid(16);
        let z = (step * 11 + seed * 3).rem_euclid(16);
        let y = min + (step * 23 + seed * 5).rem_euclid(height);
        let kind = ((step * 3 + seed).rem_euclid(5)) as u8;
        blocks[(x + z * 16 + (y - min) * 256) as usize] = kind;
        heightmaps
            .update_after_change(x, y, z, flags(kind, air_motion), |x, y, z| {
                Ok(flags(
                    blocks[(x + z * 16 + (y - min) * 256) as usize],
                    air_motion,
                ))
            })
            .unwrap();
        edits.push(Tag::Compound(vec![
            ("x".into(), Tag::Int(x)),
            ("y".into(), Tag::Int(y)),
            ("z".into(), Tag::Int(z)),
            ("kind".into(), Tag::Int(i32::from(kind))),
            ("expected".into(), heightmaps.to_tag().unwrap()),
        ]));
    }
    Tag::Compound(vec![
        ("min".into(), Tag::Int(min)),
        ("height".into(), Tag::Int(height)),
        ("air_motion".into(), Tag::Byte(i8::from(air_motion))),
        ("blocks".into(), Tag::ByteArray(original)),
        ("source".into(), source),
        (
            "edits".into(),
            Tag::List {
                element_type: TagType::Compound,
                values: edits,
            },
        ),
    ])
}

#[test]
fn failed_heightmap_scan_preserves_all_four_maps() {
    let bounds = HeightBounds {
        min_y: 0,
        max_y: 16,
    };
    let heights = [9; 256];
    let source = encode_generated_heightmaps(
        bounds,
        &GeneratedHeightmaps {
            world_surface: Some(&heights),
            ocean_floor: Some(&heights),
            motion_blocking: Some(&heights),
            motion_blocking_no_leaves: Some(&heights),
            ..Default::default()
        },
    )
    .unwrap();
    let mut full = FullHeightmaps::from_converted(bounds, &source).unwrap();
    let before = full.to_tag().unwrap();
    assert!(
        full.update_after_change(8, 8, 8, flags(0, false), |_, _, _| anyhow::bail!(
            "missing block"
        ))
        .is_err()
    );
    assert_eq!(full.to_tag().unwrap(), before);
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn incremental_full_heightmaps_match_java() {
    use std::{path::PathBuf, process::Command};
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
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
    let source = temp.path().join("FullHeightmapUpdateOracle.java");
    std::fs::write(&source, include_str!("java/FullHeightmapUpdateOracle.java")).unwrap();
    let compile = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(source)
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let mut cases = Vec::new();
    for (min, height) in [(0, 16), (-32, 64), (-64, 384)] {
        for air_motion in [false, true] {
            for (seed, mask) in [(3, 0), (7, 3), (11, 10), (17, 15)] {
                cases.push(make_case(min, height, seed, mask, air_motion));
            }
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
    let input = temp.path().join("heightmaps.nbt");
    std::fs::write(&input, nbt::write(&document).unwrap()).unwrap();
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(classpath)
        .arg("FullHeightmapUpdateOracle")
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
