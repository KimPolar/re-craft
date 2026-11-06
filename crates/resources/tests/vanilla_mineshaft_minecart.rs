//! Compare chest-minecart eligibility, rail and loot RNG with Java 26.3.
use native_core::{beardifier::BoundingBox, random::DecorationRandom};
use native_resources::{
    mineshaft::{
        CorridorBlock, MineshaftCorridorWorld, MineshaftMinecartWorld, MineshaftPlacementWorld,
        MineshaftVariant, try_mineshaft_chest_minecart,
    },
    structure_piece::StructureFrame,
    structure_start::HorizontalDirection,
};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

fn run(c: &mut Command) -> String {
    let out = c.output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
fn event(k: &str, p: [i32; 3]) -> String {
    format!("{k} {},{},{}", p[0], p[1], p[2])
}
struct World {
    mode: i32,
    target: [i32; 3],
    events: Vec<String>,
}
impl MineshaftPlacementWorld for World {
    fn blocking_biome(&mut self, _: [i32; 3]) -> anyhow::Result<bool> {
        unreachable!()
    }
    fn liquid_block(&mut self, _: [i32; 3]) -> anyhow::Result<bool> {
        unreachable!()
    }
    fn protected_block(&mut self, p: [i32; 3], _: MineshaftVariant) -> anyhow::Result<bool> {
        self.events.push(event("R", p));
        Ok(false)
    }
    fn set_cave_air(&mut self, _: [i32; 3]) -> anyhow::Result<()> {
        unreachable!()
    }
}
impl MineshaftCorridorWorld for World {
    fn is_air(&mut self, p: [i32; 3]) -> anyhow::Result<bool> {
        self.events.push(event("R", p));
        if p == self.target {
            Ok(self.mode != 1)
        } else {
            assert_eq!(p, [self.target[0], self.target[1] - 1, self.target[2]]);
            Ok(self.mode == 2)
        }
    }
    fn ocean_floor_height(&mut self, _: i32, _: i32) -> anyhow::Result<i32> {
        unreachable!()
    }
    fn face_sturdy(&mut self, _: [i32; 3], _: [i32; 3]) -> anyhow::Result<bool> {
        unreachable!()
    }
    fn set_corridor_block(
        &mut self,
        p: [i32; 3],
        b: CorridorBlock,
        _: MineshaftVariant,
    ) -> anyhow::Result<()> {
        let CorridorBlock::Rail { north_south } = b else {
            panic!("unexpected block")
        };
        self.events.push(format!(
            "{} {}",
            event("W", p),
            if north_south {
                "north_south"
            } else {
                "east_west"
            }
        ));
        self.events.push(event("F", p));
        Ok(())
    }
}
impl MineshaftMinecartWorld for World {
    type Minecart = [f64; 3];
    fn new_chest_minecart(&mut self, p: [f64; 3]) -> anyhow::Result<Option<Self::Minecart>> {
        self.events.push("L".into());
        Ok((self.mode != 3).then_some(p))
    }
    fn add_chest_minecart(&mut self, p: [f64; 3], seed: i64) -> anyhow::Result<()> {
        self.events
            .push(format!("A {},{},{} {seed}", p[0], p[1], p[2]));
        Ok(())
    }
}
#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn minecart_creation_matches_java() {
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
    let source = temp.path().join("MineshaftMinecartOracle.java");
    std::fs::write(&source, include_str!("java/MineshaftMinecartOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("MineshaftMinecartOracle"));
    let mut lines = output.lines();
    let mut cases = 0;
    while let Some(header) = lines.next() {
        if !header.starts_with("C ") {
            continue;
        }
        let f: Vec<_> = header.split_whitespace().collect();
        let d: usize = f[1].parse().unwrap();
        let mode: i32 = f[2].parse().unwrap();
        let seed: i64 = f[3].parse().unwrap();
        let variant = match f[4] {
            "NORMAL" => MineshaftVariant::Normal,
            "MESA" => MineshaftVariant::Mesa,
            v => panic!("{v}"),
        };
        let expected: bool = f[5].parse().unwrap();
        let count: usize = f[6].parse().unwrap();
        let frame = StructureFrame {
            bounds: BoundingBox::new([0, 30, 0], [9, 32, 9]).unwrap(),
            orientation: Some(
                [
                    HorizontalDirection::North,
                    HorizontalDirection::South,
                    HorizontalDirection::West,
                    HorizontalDirection::East,
                ][d],
            ),
        };
        let local = [1, 0, 3];
        let clip = if mode == 4 {
            BoundingBox::new([20, 25, 20], [30, 40, 30]).unwrap()
        } else {
            BoundingBox::new([-5, 25, -5], [15, 40, 15]).unwrap()
        };
        let mut world = World {
            mode,
            target: frame.world_position(local),
            events: vec![],
        };
        let mut random = DecorationRandom::new(seed);
        let actual =
            try_mineshaft_chest_minecart(frame, local, clip, variant, &mut world, &mut random)
                .unwrap();
        assert_eq!(actual, expected, "{header}");
        assert_eq!(random.next_long(), f[7].parse::<i64>().unwrap(), "{header}");
        assert_eq!(world.events.len(), count, "{header}");
        for (i, actual) in world.events.iter().enumerate() {
            assert_eq!(Some(actual.as_str()), lines.next(), "{header}, event {i}");
        }
        cases += 1;
    }
    assert_eq!(cases, 240);
}

#[test]
fn disabled_entity_does_not_consume_loot_seed() {
    let frame = StructureFrame {
        bounds: BoundingBox::new([0, 30, 0], [9, 32, 9]).unwrap(),
        orientation: Some(HorizontalDirection::South),
    };
    let local = [1, 0, 3];
    let clip = BoundingBox::new([-5, 25, -5], [15, 40, 15]).unwrap();
    let mut world = World {
        mode: 3,
        target: frame.world_position(local),
        events: vec![],
    };
    let mut random = DecorationRandom::new(42);
    let mut expected = random.clone();
    expected.next_bool();
    assert!(
        try_mineshaft_chest_minecart(
            frame,
            local,
            clip,
            MineshaftVariant::Normal,
            &mut world,
            &mut random
        )
        .unwrap()
    );
    assert!(world.events.iter().any(|line| line == "L"));
    assert!(!world.events.iter().any(|line| line.starts_with("A ")));
    assert_eq!(random.next_long(), expected.next_long());
}
