use native_core::beardifier::BoundingBox;
use native_resources::{structure_piece::StructureFrame, structure_start::HorizontalDirection};
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn structure_frame_matches_java_coordinates_and_facing() {
    use HorizontalDirection::{East, North, South, West};
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
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
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("StructureFrameOracle.java");
    std::fs::write(&source, include_str!("java/StructureFrameOracle.java")).unwrap();
    let run = |command: &mut Command| {
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    run(Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg("StructureFrameOracle"));
    let directions = [None, Some(North), Some(East), Some(South), Some(West)];
    let faces = [North, East, South, West];
    let locals = [
        [0, 0, 0],
        [1, 2, 3],
        [-4, -5, -6],
        [15, 319, 16],
        [i32::MAX, i32::MIN, i32::MAX],
        [i32::MIN, i32::MAX, i32::MIN],
    ];
    let mut result = Vec::new();
    for (orientation, direction) in directions.into_iter().enumerate() {
        let frame = StructureFrame {
            bounds: BoundingBox::new([-8, -64, 10], [12, 70, 20]).unwrap(),
            orientation: direction,
        };
        for (index, local) in locals.into_iter().enumerate() {
            let [x, y, z] = frame.world_position(local);
            for (face, direction) in faces.into_iter().enumerate() {
                let facing = match frame.horizontal_facing(direction) {
                    North => "NORTH",
                    East => "EAST",
                    South => "SOUTH",
                    West => "WEST",
                };
                result.push(format!(
                    "T {orientation} {index} {face} {x} {y} {z} {facing}"
                ));
            }
        }
    }
    let expected: Vec<_> = output
        .lines()
        .filter(|line| line.starts_with("T "))
        .map(str::to_owned)
        .collect();
    assert_eq!(expected.len(), 120);
    assert_eq!(result, expected);
    let mut stairs = Vec::new();
    for (orientation, direction) in directions.into_iter().enumerate() {
        let frame = StructureFrame {
            bounds: BoundingBox::new([-8, -64, 10], [12, 70, 20]).unwrap(),
            orientation: direction,
        };
        for (face, direction) in faces.into_iter().enumerate() {
            for shape in [
                "straight",
                "inner_left",
                "inner_right",
                "outer_left",
                "outer_right",
            ] {
                let facing = match frame.horizontal_facing(direction) {
                    North => "NORTH",
                    East => "EAST",
                    South => "SOUTH",
                    West => "WEST",
                };
                stairs.push(format!(
                    "S {orientation} {face} {shape} {facing} {}",
                    frame.stair_shape(direction, shape).unwrap()
                ));
            }
        }
    }
    let expected: Vec<_> = output
        .lines()
        .filter(|line| line.starts_with("S "))
        .map(str::to_owned)
        .collect();
    assert_eq!(expected.len(), 100);
    assert_eq!(stairs, expected);
}
