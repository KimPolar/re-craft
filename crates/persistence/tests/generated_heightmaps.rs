use native_persistence::{
    heightmap::{
        GeneratedHeightmaps, HeightBounds, SpawnHeightmapKind, SpawnHeightmaps,
        encode_generated_heightmaps,
    },
    nbt::{self, NamedTag, Tag},
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
