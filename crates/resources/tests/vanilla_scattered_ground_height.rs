use native_core::beardifier::{BoundingBox, TerrainAdjustment};
use native_resources::{
    structure_piece::ScatteredGroundHeight,
    structure_start::{HorizontalDirection, SimpleStructureKind, SimpleStructureStart},
};
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn scattered_ground_alignment_matches_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME")
        .or_else(|| std::env::var_os("JAVA_HOME"))
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
    let source = temp.path().join("ScatteredGroundHeightOracle.java");
    std::fs::write(
        &source,
        include_str!("java/ScatteredGroundHeightOracle.java"),
    )
    .unwrap();
    let run = |command: &mut Command| {
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        String::from_utf8(result.stdout).unwrap()
    };
    run(Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg("ScatteredGroundHeightOracle"));
    let clips = [
        BoundingBox::new([-10, 0, 0], [10, 100, 30]).unwrap(),
        BoundingBox::new([-3, 64, 10], [-2, 64, 11]).unwrap(),
        BoundingBox::new([5, 0, 0], [7, 100, 30]).unwrap(),
        BoundingBox::new([-10, 0, 0], [10, 63, 30]).unwrap(),
    ];
    let mut actual = Vec::new();
    for mode in 0..4 {
        for (clip_index, clip) in clips.iter().copied().enumerate() {
            for offset in [-2, 0, 3] {
                for method in 0..2 {
                    let mut piece = SimpleStructureStart {
                        kind: SimpleStructureKind::SwampHut,
                        chunk: [0, 0],
                        generation_point: [0, 0, 0],
                        orientation: Some(HorizontalDirection::South),
                        bounds: BoundingBox::new([-4, 40, 9], [2, 46, 17]).unwrap(),
                        terrain_adjustment: TerrainAdjustment::None,
                    };
                    let mut state = ScatteredGroundHeight::default();
                    for repeat in 0..2 {
                        let mut reads = 0;
                        let mut hash = 0i32;
                        let mut height = |x: i32, z: i32| {
                            reads += 1;
                            hash = hash
                                .wrapping_mul(31)
                                .wrapping_add(x)
                                .wrapping_mul(31)
                                .wrapping_add(z);
                            Ok([-45, 0, 75, 319][mode] + (x * 13 + z * 7).rem_euclid(11) - 5)
                        };
                        let result = if method == 0 {
                            state.update_average(&mut piece, clip, offset, &mut height)
                        } else {
                            state.update_lowest(&mut piece, 319, offset, &mut height)
                        }
                        .unwrap();
                        actual.push(format!("S {mode} {clip_index} {offset} {method} {repeat} {result} {} {} {} {reads} {hash}", state.height_position(), piece.bounds.min[1], piece.bounds.max[1]));
                    }
                }
            }
        }
    }
    let expected: Vec<_> = output
        .lines()
        .filter(|line| line.starts_with("S "))
        .map(str::to_owned)
        .collect();
    assert_eq!(expected.len(), 192);
    assert_eq!(actual, expected);
}
