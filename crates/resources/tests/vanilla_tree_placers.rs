use native_core::random::DecorationRandom;
use native_resources::tree_placers::{
    FoliageAttachment, TrunkHeight, place_blob_foliage, place_straight_trunk,
};
use std::{path::PathBuf, process::Command};

fn command(command: &mut Command) -> String {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn invalid_dimensions_and_failed_world_writes_stop_placement() {
    assert!(TrunkHeight::new(33, 0, 0).is_err());
    assert!(TrunkHeight::new(4, 25, 0).is_err());
    let attachment = FoliageAttachment {
        position: [0, 0, 0],
        radius_offset: 0,
        height_offset: 0,
        double_trunk: false,
    };
    let mut random = DecorationRandom::new(0);
    let mut writes = 0;
    assert!(
        place_blob_foliage(&mut random, attachment, 3, 2, 0, |_, _| {
            writes += 1;
            anyhow::bail!("unavailable chunk")
        })
        .is_err()
    );
    assert_eq!(writes, 1);
    assert!(place_blob_foliage(&mut random, attachment, 17, 2, 0, |_, _| Ok(())).is_err());
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn trunk_heights_and_blob_foliage_match_java() {
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
    for lib in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("TreePlacersOracle.java");
    std::fs::write(&source, include_str!("java/TreePlacersOracle.java")).unwrap();
    command(
        Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source),
    );
    let actual = command(
        Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("TreePlacersOracle"),
    );
    let mut expected = Vec::new();
    for seed in [0, 1, -1, 987654321] {
        for height in [0, 1, 3, 5] {
            for radius in [0, 2, 4] {
                for offset in [0, 1] {
                    for big in [false, true] {
                        for obstructed in [false, true] {
                            let mut random = DecorationRandom::new(seed);
                            let trunk = TrunkHeight::new(4, 2, 0).unwrap().sample(&mut random);
                            let mut trunks = Vec::new();
                            let attachment = place_straight_trunk(
                                &mut random,
                                [3, 10, -4],
                                trunk,
                                |[x, y, z], soil, _| {
                                    if soil
                                        || !obstructed
                                        || (x * 3 + y * 5 + z * 7_i32).rem_euclid(11) != 0
                                    {
                                        trunks.push(format!("{x},{y},{z}"));
                                    }
                                    Ok(())
                                },
                            )
                            .unwrap();
                            assert_eq!(attachment.position, [3, 10 + trunk, -4]);
                            let mut positions = Vec::new();
                            place_blob_foliage(
                                &mut random,
                                FoliageAttachment {
                                    position: [3, 30, -4],
                                    radius_offset: 1,
                                    height_offset: 1,
                                    double_trunk: big,
                                },
                                height,
                                radius,
                                offset,
                                |[x, y, z], random| {
                                    if !obstructed
                                        || (x * 3 + y * 5 + z * 7_i32).rem_euclid(11) != 0
                                    {
                                        random.next_int_bound(17).unwrap();
                                        positions.push(format!("{x},{y},{z}"));
                                    }
                                    Ok(())
                                },
                            )
                            .unwrap();
                            expected.push(format!(
                            "C {seed} {height} {radius} {offset} {big} {obstructed} {trunk} {} L {} R {}",
                            trunks.join(";"),
                            positions.join(";"),
                            random.next_long()
                        ));
                        }
                    }
                }
            }
        }
    }
    let actual: Vec<_> = actual
        .lines()
        .filter(|line| line.starts_with("C "))
        .collect();
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
        assert_eq!(*actual, expected, "case {index}");
    }
}
