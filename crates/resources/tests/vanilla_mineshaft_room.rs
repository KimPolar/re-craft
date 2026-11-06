//! Compare the initial room and the following RNG state with installed Java 26.3.
use native_core::{beardifier::BoundingBox, random::LegacyRandom};
use native_resources::mineshaft::{
    InitialMineshaftRoom, MineshaftPieceKind, MineshaftVariant, choose_piece_kind, find_corridor,
    find_crossing, find_stairs,
};
use native_resources::structure_start::HorizontalDirection;
use serde_json::Value;
use std::{path::PathBuf, process::Command};

fn run(command: &mut Command) -> String {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn initial_mineshaft_rooms_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
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
    let classpath = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("MineshaftRoomOracle.java");
    std::fs::write(&source, include_str!("java/MineshaftRoomOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&classpath)
        .arg(source));
    let result = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&classpath)
        .arg("MineshaftRoomOracle"));
    let mut count = 0;
    for line in result.lines().filter(|line| line.starts_with("R ")) {
        let fields: Vec<_> = line.split_whitespace().collect();
        assert_eq!(fields.len(), 12, "{line}");
        let seed = fields[1].parse().unwrap();
        let chunk = [fields[2].parse().unwrap(), fields[3].parse().unwrap()];
        let variant = match fields[4] {
            "NORMAL" => MineshaftVariant::Normal,
            "MESA" => MineshaftVariant::Mesa,
            value => panic!("unexpected variant {value}"),
        };
        let (bounds, actual_variant, mut random) = InitialMineshaftRoom::new(seed, chunk, variant)
            .unwrap()
            .into_parts();
        let values: Vec<i32> = fields[5..11]
            .iter()
            .map(|value| value.parse().unwrap())
            .collect();
        assert_eq!(bounds.min, [values[0], values[1], values[2]], "{line}");
        assert_eq!(bounds.max, [values[3], values[4], values[5]], "{line}");
        assert_eq!(actual_variant, variant);
        assert_eq!(
            random.next_long(),
            fields[11].parse::<i64>().unwrap(),
            "{line}"
        );
        count += 1;
    }
    assert_eq!(count, 686);
    let mut candidates = 0;
    for line in result.lines().filter(|line| line.starts_with("S ")) {
        let fields: Vec<_> = line.split_whitespace().collect();
        assert_eq!(fields.len(), 6, "{line}");
        let mut random = LegacyRandom::new(fields[1].parse().unwrap());
        let expected = match fields[4] {
            "MineShaftCorridor" => MineshaftPieceKind::Corridor,
            "MineShaftStairs" => MineshaftPieceKind::Stairs,
            "MineShaftCrossing" => MineshaftPieceKind::Crossing,
            value => panic!("unexpected piece {value}"),
        };
        assert_eq!(choose_piece_kind(&mut random), expected, "{line}");
        assert_eq!(
            random.next_long(),
            fields[5].parse::<i64>().unwrap(),
            "{line}"
        );
        candidates += 1;
    }
    assert_eq!(candidates, 4096);
    let mut boxes = 0;
    let mut shortened_corridors = 0;
    let mut rejected_pieces = 0;
    for line in result.lines().filter(|line| line.starts_with("F ")) {
        let fields: Vec<_> = line.split_whitespace().collect();
        assert_eq!(fields.len(), 22, "{line}");
        let mut random = LegacyRandom::new(fields[2].parse().unwrap());
        let direction = match fields[3] {
            "north" => HorizontalDirection::North,
            "south" => HorizontalDirection::South,
            "west" => HorizontalDirection::West,
            "east" => HorizontalDirection::East,
            value => panic!("unexpected direction {value}"),
        };
        let values: Vec<i32> = fields[5..14]
            .iter()
            .map(|value| value.parse().unwrap())
            .collect();
        let origin = [values[0], values[1], values[2]];
        let mut preview = random.clone();
        let sampled_corridor_length = 5 * (2 + preview.next_int_bound(3).unwrap());
        let occupied = if fields[4] == "true" {
            vec![
                BoundingBox::new(
                    [values[3], values[4], values[5]],
                    [values[6], values[7], values[8]],
                )
                .unwrap(),
            ]
        } else {
            vec![]
        };
        let actual = match fields[1] {
            "stairs" => find_stairs(origin, direction, &occupied).unwrap(),
            "crossing" => find_crossing(&mut random, origin, direction, &occupied).unwrap(),
            "corridor" => find_corridor(&mut random, origin, direction, &occupied).unwrap(),
            value => panic!("unexpected piece {value}"),
        };
        let expected = if fields[14] == "true" {
            let values: Vec<i32> = fields[15..21]
                .iter()
                .map(|value| value.parse().unwrap())
                .collect();
            Some(
                BoundingBox::new(
                    [values[0], values[1], values[2]],
                    [values[3], values[4], values[5]],
                )
                .unwrap(),
            )
        } else {
            None
        };
        assert_eq!(actual, expected, "{line}");
        if actual.is_none() {
            rejected_pieces += 1;
        } else if fields[1] == "corridor" {
            let actual = actual.unwrap();
            let axis = if matches!(
                direction,
                HorizontalDirection::North | HorizontalDirection::South
            ) {
                2
            } else {
                0
            };
            if actual.max[axis] - actual.min[axis] + 1 < sampled_corridor_length {
                shortened_corridors += 1;
            }
        }
        assert_eq!(
            random.next_long(),
            fields[21].parse::<i64>().unwrap(),
            "{line}"
        );
        boxes += 1;
    }
    assert_eq!(boxes, 4608);
    assert!(shortened_corridors > 0);
    assert!(rejected_pieces > 0);
}
