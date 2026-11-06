//! Compare corridor support beams and cobweb callbacks with Java 26.3.
use native_core::{beardifier::BoundingBox, random::DecorationRandom};
use native_resources::{
    mineshaft::{
        CorridorBlock, MineshaftCorridorWorld, MineshaftPlacementWorld, MineshaftVariant,
        place_corridor_beam, place_corridor_cobweb,
    },
    structure_piece::StructureFrame,
    structure_start::HorizontalDirection,
};
use serde_json::Value;
use std::{collections::HashMap, path::PathBuf, process::Command};

fn run(command: &mut Command) -> String {
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
fn event(kind: &str, p: [i32; 3]) -> String {
    format!("{kind} {},{},{}", p[0], p[1], p[2])
}
fn name(d: HorizontalDirection) -> &'static str {
    match d {
        HorizontalDirection::North => "north",
        HorizontalDirection::South => "south",
        HorizontalDirection::West => "west",
        HorizontalDirection::East => "east",
    }
}
#[derive(Clone, Copy)]
enum Block {
    Air,
    Oak,
    DarkOak,
    Chain,
    Stone,
    Fence,
    Torch,
    Web,
}
struct World {
    scenario: i32,
    changed: HashMap<[i32; 3], Block>,
    events: Vec<String>,
}
impl World {
    fn state(&self, p: [i32; 3]) -> Block {
        if let Some(state) = self.changed.get(&p) {
            return *state;
        }
        let h = p[0]
            .wrapping_mul(31)
            .wrapping_add(p[1].wrapping_mul(17))
            .wrapping_add(p[2].wrapping_mul(13));
        if (self.scenario == 1 && p[1] == 33) || (self.scenario == 3 && h.rem_euclid(3) == 0) {
            Block::Air
        } else if h.rem_euclid(11) == 0 {
            Block::Oak
        } else if h.rem_euclid(13) == 0 {
            Block::DarkOak
        } else if h.rem_euclid(17) == 0 {
            Block::Chain
        } else {
            Block::Stone
        }
    }
    fn read(&mut self, p: [i32; 3]) -> Block {
        self.events.push(event("R", p));
        self.state(p)
    }
}
impl MineshaftPlacementWorld for World {
    fn blocking_biome(&mut self, _: [i32; 3]) -> anyhow::Result<bool> {
        panic!("unexpected biome check")
    }
    fn liquid_block(&mut self, _: [i32; 3]) -> anyhow::Result<bool> {
        panic!("unexpected liquid check")
    }
    fn protected_block(&mut self, p: [i32; 3], v: MineshaftVariant) -> anyhow::Result<bool> {
        Ok(match self.read(p) {
            Block::Oak => v == MineshaftVariant::Normal,
            Block::DarkOak => v == MineshaftVariant::Mesa,
            Block::Chain => true,
            _ => false,
        })
    }
    fn set_cave_air(&mut self, _: [i32; 3]) -> anyhow::Result<()> {
        panic!("unexpected cave air")
    }
}
impl MineshaftCorridorWorld for World {
    fn is_air(&mut self, p: [i32; 3]) -> anyhow::Result<bool> {
        Ok(matches!(self.read(p), Block::Air))
    }
    fn ocean_floor_height(&mut self, x: i32, z: i32) -> anyhow::Result<i32> {
        let height = if self.scenario == 2 { 31 } else { 40 };
        self.events
            .push(format!("{} {height}", event("H", [x, 0, z])));
        Ok(height)
    }
    fn face_sturdy(&mut self, p: [i32; 3], _: [i32; 3]) -> anyhow::Result<bool> {
        Ok(matches!(
            self.read(p),
            Block::Oak | Block::DarkOak | Block::Stone
        ))
    }
    fn set_corridor_block(
        &mut self,
        p: [i32; 3],
        block: CorridorBlock,
        v: MineshaftVariant,
    ) -> anyhow::Result<()> {
        let (description, state, post) = match block {
            CorridorBlock::Planks => (
                "planks".into(),
                if v == MineshaftVariant::Normal {
                    Block::Oak
                } else {
                    Block::DarkOak
                },
                false,
            ),
            CorridorBlock::Fence { connected } => {
                (format!("fence:{}", name(connected)), Block::Fence, true)
            }
            CorridorBlock::WallTorch { facing } => {
                (format!("torch:{}", name(facing)), Block::Torch, true)
            }
            CorridorBlock::Cobweb => ("web".into(), Block::Web, false),
        };
        self.events.push(format!("{} {description}", event("W", p)));
        self.changed.insert(p, state);
        self.events.push(event("F", p));
        if post {
            self.events.push(event("M", p));
        }
        Ok(())
    }
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn corridor_beams_and_cobwebs_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
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
    let source = temp.path().join("MineshaftCorridorOracle.java");
    std::fs::write(&source, include_str!("java/MineshaftCorridorOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(source));
    let result = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("MineshaftCorridorOracle"));
    let mut lines = result.lines();
    let mut cases = 0;
    while let Some(header) = lines.next() {
        if !header.starts_with("C ") {
            continue;
        }
        let f: Vec<_> = header.split_whitespace().collect();
        let kind: usize = f[1].parse().unwrap();
        let direction: usize = f[2].parse().unwrap();
        let clip_id: usize = f[3].parse().unwrap();
        let scenario: i32 = f[4].parse().unwrap();
        let seed: i64 = f[5].parse().unwrap();
        let variant = match f[6] {
            "NORMAL" => MineshaftVariant::Normal,
            "MESA" => MineshaftVariant::Mesa,
            v => panic!("{v}"),
        };
        let count: usize = f[7].parse().unwrap();
        let frame = StructureFrame {
            bounds: BoundingBox::new([0, 30, 0], [9, 32, 9]).unwrap(),
            orientation: Some(
                [
                    HorizontalDirection::North,
                    HorizontalDirection::South,
                    HorizontalDirection::West,
                    HorizontalDirection::East,
                ][direction],
            ),
        };
        let clip = match clip_id {
            0 => BoundingBox::new([-5, 25, -5], [15, 40, 15]).unwrap(),
            1 => BoundingBox::new([0, 30, 0], [7, 32, 7]).unwrap(),
            _ => BoundingBox::new([0, 30, 0], [4, 34, 9]).unwrap(),
        };
        let mut world = World {
            scenario,
            changed: HashMap::new(),
            events: vec![],
        };
        let mut random = DecorationRandom::new(seed);
        if kind == 0 {
            place_corridor_beam(frame, 2, variant, &mut world, clip, &mut random).unwrap();
        } else {
            let chance = match scenario {
                0 => 0.1,
                1 => 1.0,
                2 => 0.0,
                _ => 0.05,
            };
            place_corridor_cobweb(
                frame,
                [1, 1, 3],
                chance,
                variant,
                &mut world,
                clip,
                &mut random,
            )
            .unwrap();
        }
        assert_eq!(random.next_long(), f[8].parse::<i64>().unwrap(), "{header}");
        assert_eq!(world.events.len(), count, "{header}");
        for (i, actual) in world.events.iter().enumerate() {
            assert_eq!(Some(actual.as_str()), lines.next(), "{header}, event {i}");
        }
        cases += 1;
    }
    assert_eq!(cases, 1536);
}
