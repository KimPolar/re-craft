//! Compare recursive piece order, attributes, entrances and random state with Java 26.3.
use native_core::beardifier::BoundingBox;
use native_resources::mineshaft::{
    MineshaftElevation, MineshaftGraph, MineshaftGraphPieceKind, MineshaftVariant,
};
use native_resources::structure_start::HorizontalDirection;
use serde_json::Value;
use std::{fmt::Write, path::PathBuf, process::Command};

fn run(command: &mut Command) -> String {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn box_string(b: &BoundingBox) -> String {
    format!(
        "{},{},{},{},{},{}",
        b.min[0], b.min[1], b.min[2], b.max[0], b.max[1], b.max[2]
    )
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn recursive_mineshaft_graph_matches_java() {
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
    let source = temp.path().join("MineshaftGraphOracle.java");
    std::fs::write(&source, include_str!("java/MineshaftGraphOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&classpath)
        .arg(source));
    let result = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&classpath)
        .arg("MineshaftGraphOracle"));
    let mut actual = String::new();
    let mut graphs = 0;
    for line in result.lines().filter(|line| line.starts_with("G ")) {
        let fields: Vec<_> = line.split_whitespace().collect();
        let seed = fields[1].parse().unwrap();
        let chunk = [fields[2].parse().unwrap(), fields[3].parse().unwrap()];
        let variant = match fields[4] {
            "NORMAL" => MineshaftVariant::Normal,
            "MESA" => MineshaftVariant::Mesa,
            other => panic!("{other}"),
        };
        let (_, pieces, mut random) = MineshaftGraph::new(seed, chunk, variant)
            .unwrap()
            .into_parts();
        writeln!(actual, "{line}").unwrap();
        for piece in &pieces {
            let direction = match piece.direction {
                None => "none",
                Some(HorizontalDirection::North) => "north",
                Some(HorizontalDirection::South) => "south",
                Some(HorizontalDirection::West) => "west",
                Some(HorizontalDirection::East) => "east",
            };
            let extra = match &piece.kind {
                MineshaftGraphPieceKind::Room { .. } => "room".to_owned(),
                MineshaftGraphPieceKind::Stairs => "stairs".to_owned(),
                MineshaftGraphPieceKind::Crossing { two_floors } => {
                    format!("crossing,{two_floors}")
                }
                MineshaftGraphPieceKind::Corridor {
                    rails,
                    spiders,
                    sections,
                } => format!("corridor,{rails},{spiders},{sections}"),
            };
            writeln!(
                actual,
                "P {direction} {} {} {extra}",
                piece.depth,
                box_string(&piece.bounds)
            )
            .unwrap();
        }
        if let MineshaftGraphPieceKind::Room { entrances } = &pieces[0].kind {
            for entrance in entrances {
                writeln!(actual, "E {}", box_string(entrance)).unwrap();
            }
        }
        writeln!(actual, "Z {}", random.next_long()).unwrap();
        for [sea_level, min_y, surface_height] in [
            [63, -64, 50],
            [63, -64, 63],
            [63, -64, 128],
            [-20, -100, 20],
            [320, 300, 321],
        ] {
            let graph = MineshaftGraph::new(seed, chunk, variant).unwrap();
            let center = graph.center();
            let elevation = match variant {
                MineshaftVariant::Normal => MineshaftElevation::Normal { sea_level, min_y },
                MineshaftVariant::Mesa => MineshaftElevation::Mesa {
                    sea_level,
                    surface_height,
                },
            };
            let placed = graph.place(elevation).unwrap();
            let point = placed.generation_point;
            writeln!(
                actual,
                "H {sea_level} {min_y} {surface_height} {},{},{} {} {},{},{}",
                center[0], center[1], center[2], placed.offset_y, point[0], point[1], point[2]
            )
            .unwrap();
            let (_, pieces, mut random) = placed.into_parts();
            for piece in &pieces {
                writeln!(actual, "Q {}", box_string(&piece.bounds)).unwrap();
            }
            if let MineshaftGraphPieceKind::Room { entrances } = &pieces[0].kind {
                for entrance in entrances {
                    writeln!(actual, "I {}", box_string(entrance)).unwrap();
                }
            }
            writeln!(actual, "Y {}", random.next_long()).unwrap();
        }
        graphs += 1;
    }
    assert_eq!(graphs, 56);
    let expected: Vec<_> = result
        .lines()
        .filter(|line| {
            ["G ", "P ", "E ", "Z ", "H ", "Q ", "I ", "Y "]
                .iter()
                .any(|prefix| line.starts_with(prefix))
        })
        .collect();
    let got: Vec<_> = actual.lines().collect();
    for (index, (actual_line, expected_line)) in got.iter().zip(&expected).enumerate() {
        assert_eq!(
            actual_line,
            expected_line,
            "line {index}; expected nearby {:?}; actual nearby {:?}",
            &expected[index.saturating_sub(2)..(index + 4).min(expected.len())],
            &got[index.saturating_sub(2)..(index + 4).min(got.len())]
        );
    }
    assert_eq!(got.len(), expected.len());
}
