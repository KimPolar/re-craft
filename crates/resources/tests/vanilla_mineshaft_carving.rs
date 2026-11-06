//! Compare room and stairs world callbacks with installed Minecraft 26.3.
use native_core::{beardifier::BoundingBox, random::LegacyRandom};
use native_resources::{
    mineshaft::{
        MineshaftGraphPiece, MineshaftGraphPieceKind, MineshaftPlacementOutcome,
        MineshaftPlacementWorld, MineshaftVariant, place_room_or_stairs,
    },
    structure_start::HorizontalDirection,
};
use serde_json::Value;
use std::{collections::HashSet, path::PathBuf, process::Command};

fn run(command: &mut Command) -> String {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
fn line(kind: &str, p: [i32; 3]) -> String {
    format!("{kind} {},{},{}", p[0], p[1], p[2])
}

struct World {
    scenario: i32,
    changed: HashSet<[i32; 3]>,
    events: Vec<String>,
}
impl World {
    fn hash(p: [i32; 3]) -> i32 {
        p[0].wrapping_mul(31)
            .wrapping_add(p[1].wrapping_mul(17))
            .wrapping_add(p[2].wrapping_mul(13))
    }
    fn read(&mut self, p: [i32; 3]) -> i32 {
        self.events.push(line("R", p));
        Self::hash(p)
    }
}
impl MineshaftPlacementWorld for World {
    fn blocking_biome(&mut self, p: [i32; 3]) -> anyhow::Result<bool> {
        self.events.push(line("B", p));
        Ok(self.scenario == 1)
    }
    fn liquid_block(&mut self, p: [i32; 3]) -> anyhow::Result<bool> {
        let h = self.read(p);
        Ok(!self.changed.contains(&p) && self.scenario == 2 && h.rem_euclid(23) == 0)
    }
    fn protected_block(&mut self, p: [i32; 3], variant: MineshaftVariant) -> anyhow::Result<bool> {
        let h = self.read(p);
        if self.changed.contains(&p) || (self.scenario == 2 && h.rem_euclid(23) == 0) {
            return Ok(false);
        }
        Ok(if h.rem_euclid(29) == 0 {
            variant == MineshaftVariant::Normal
        } else if h.rem_euclid(31) == 0 {
            variant == MineshaftVariant::Mesa
        } else {
            h.rem_euclid(37) == 0
        })
    }
    fn set_cave_air(&mut self, p: [i32; 3]) -> anyhow::Result<()> {
        self.events.push(line("W", p));
        self.changed.insert(p);
        self.events.push(line("F", p));
        Ok(())
    }
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn room_and_stairs_callbacks_match_java() {
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
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("MineshaftCarveOracle.java");
    std::fs::write(&source, include_str!("java/MineshaftCarveOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(source));
    let result = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("MineshaftCarveOracle"));
    let mut lines = result.lines();
    let mut cases = 0;
    while let Some(header) = lines.next() {
        if !header.starts_with("C ") {
            continue;
        }
        let values: Vec<_> = header.split_whitespace().collect();
        let shift: i32 = values[1].parse().unwrap();
        let kind: usize = values[2].parse().unwrap();
        let c: usize = values[3].parse().unwrap();
        let scenario: i32 = values[4].parse().unwrap();
        let variant = match values[5] {
            "NORMAL" => MineshaftVariant::Normal,
            "MESA" => MineshaftVariant::Mesa,
            x => panic!("{x}"),
        };
        let count: usize = values[6].parse().unwrap();
        assert_eq!(
            LegacyRandom::new(42).next_long(),
            values[7].parse::<i64>().unwrap(),
            "{header}"
        );
        let box_of = |min, max| BoundingBox::new(min, max).unwrap();
        let (piece_kind, direction, bounds) = if kind == 0 {
            (
                MineshaftGraphPieceKind::Room {
                    entrances: vec![
                        box_of([shift + 3, 34, shift], [shift + 5, 36, shift + 1]),
                        box_of([shift + 7, 33, shift + 4], [shift + 8, 35, shift + 6]),
                    ],
                },
                None,
                box_of([shift, 32, shift], [shift + 8, 38, shift + 10]),
            )
        } else {
            let d = [
                HorizontalDirection::North,
                HorizontalDirection::South,
                HorizontalDirection::West,
                HorizontalDirection::East,
            ][kind - 1];
            let z_axis = kind <= 2;
            (
                MineshaftGraphPieceKind::Stairs,
                Some(d),
                box_of(
                    [shift + 3, 30, shift + 3],
                    [
                        shift + if z_axis { 5 } else { 11 },
                        37,
                        shift + if z_axis { 11 } else { 5 },
                    ],
                ),
            )
        };
        let clip = match c {
            0 => box_of([shift - 2, 28, shift - 2], [shift + 16, 42, shift + 16]),
            1 => box_of([shift, 30, shift], [shift + 7, 35, shift + 7]),
            _ => box_of([shift + 8, 31, shift - 2], [shift + 16, 42, shift + 16]),
        };
        let piece = MineshaftGraphPiece {
            bounds,
            direction,
            depth: if kind == 0 { 0 } else { 3 },
            kind: piece_kind,
        };
        let mut world = World {
            scenario,
            changed: HashSet::new(),
            events: vec![],
        };
        let outcome = place_room_or_stairs(&piece, variant, &mut world, clip).unwrap();
        if scenario == 1 {
            assert_eq!(outcome, MineshaftPlacementOutcome::Blocked, "{header}");
        }
        if scenario == 0 {
            assert_eq!(outcome, MineshaftPlacementOutcome::Carved, "{header}");
        }
        assert_eq!(world.events.len(), count, "{header}");
        for (index, event) in world.events.iter().enumerate() {
            assert_eq!(
                Some(event.as_str()),
                lines.next(),
                "{header}, event {index}"
            );
        }
        cases += 1;
    }
    assert_eq!(cases, 360);
}
