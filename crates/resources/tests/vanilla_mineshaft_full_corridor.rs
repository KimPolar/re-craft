//! Compare a complete corridor pass with Java, delegating chest creation.
use native_core::{beardifier::BoundingBox, random::DecorationRandom};
use native_resources::{
    mineshaft::{
        CorridorBlock, MineshaftCorridorGenerationWorld, MineshaftCorridorWorld,
        MineshaftGraphPiece, MineshaftGraphPieceKind, MineshaftPlacementWorld,
        MineshaftSupportBlock, MineshaftSupportWorld, MineshaftVariant, place_corridor,
    },
    structure_piece::StructureFrame,
    structure_start::HorizontalDirection,
};
use serde_json::Value;
use std::{collections::HashMap, path::PathBuf, process::Command};

fn run(c: &mut Command) -> String {
    let r = c.output().unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    String::from_utf8(r.stdout).unwrap()
}
fn event(k: &str, p: [i32; 3]) -> String {
    format!("{k} {},{},{}", p[0], p[1], p[2])
}
fn direction(d: HorizontalDirection) -> &'static str {
    match d {
        HorizontalDirection::North => "north",
        HorizontalDirection::South => "south",
        HorizontalDirection::East => "east",
        HorizontalDirection::West => "west",
    }
}
#[derive(Clone, Copy)]
enum Block {
    Air,
    Water,
    Stone,
    Oak,
    Dark,
    Chain,
    Fence(MineshaftVariant),
    Torch,
    Web,
    Rail,
    Spawner,
    Wood,
}
struct World {
    mode: i32,
    changed: HashMap<[i32; 3], Block>,
    events: Vec<String>,
}
impl World {
    fn state(&self, p: [i32; 3]) -> Block {
        if let Some(s) = self.changed.get(&p) {
            return *s;
        }
        let h = p[0]
            .wrapping_mul(31)
            .wrapping_add(p[1].wrapping_mul(17))
            .wrapping_add(p[2].wrapping_mul(13));
        if self.mode == 2 && h.rem_euclid(19) == 0 {
            Block::Water
        } else if h.rem_euclid(29) == 0 {
            Block::Oak
        } else if h.rem_euclid(31) == 0 {
            Block::Dark
        } else if h.rem_euclid(37) == 0 {
            Block::Chain
        } else {
            Block::Stone
        }
    }
    fn read(&mut self, p: [i32; 3]) -> Block {
        self.events.push(event("R", p));
        self.state(p)
    }
    fn write(&mut self, p: [i32; 3], name: &str, state: Block) {
        self.events.push(format!("{} {name}", event("W", p)));
        self.changed.insert(p, state);
    }
    fn solid(s: Block) -> bool {
        matches!(s, Block::Stone | Block::Oak | Block::Dark | Block::Wood)
    }
}
impl MineshaftPlacementWorld for World {
    fn blocking_biome(&mut self, p: [i32; 3]) -> anyhow::Result<bool> {
        self.events.push(event("B", p));
        Ok(self.mode == 1)
    }
    fn liquid_block(&mut self, p: [i32; 3]) -> anyhow::Result<bool> {
        Ok(matches!(self.read(p), Block::Water))
    }
    fn protected_block(&mut self, p: [i32; 3], v: MineshaftVariant) -> anyhow::Result<bool> {
        Ok(match self.read(p) {
            Block::Oak => v == MineshaftVariant::Normal,
            Block::Dark => v == MineshaftVariant::Mesa,
            Block::Fence(other) => v == other,
            Block::Chain => true,
            _ => false,
        })
    }
    fn set_cave_air(&mut self, p: [i32; 3]) -> anyhow::Result<()> {
        self.write(p, "air", Block::Air);
        self.events.push(event("F", p));
        Ok(())
    }
}
impl MineshaftCorridorWorld for World {
    fn is_air(&mut self, p: [i32; 3]) -> anyhow::Result<bool> {
        Ok(matches!(self.read(p), Block::Air))
    }
    fn ocean_floor_height(&mut self, x: i32, z: i32) -> anyhow::Result<i32> {
        let h = if x
            .wrapping_mul(7)
            .wrapping_add(z.wrapping_mul(3))
            .rem_euclid(5)
            == 0
        {
            30
        } else {
            40
        };
        self.events.push(format!("{} {h}", event("H", [x, 0, z])));
        Ok(h)
    }
    fn face_sturdy(&mut self, p: [i32; 3], _: [i32; 3]) -> anyhow::Result<bool> {
        Ok(Self::solid(self.read(p)))
    }
    fn set_corridor_block(
        &mut self,
        p: [i32; 3],
        b: CorridorBlock,
        v: MineshaftVariant,
    ) -> anyhow::Result<()> {
        let (label, state, mark) = match b {
            CorridorBlock::Planks => {
                if v == MineshaftVariant::Normal {
                    ("oak_planks".into(), Block::Oak, false)
                } else {
                    ("dark_planks".into(), Block::Dark, false)
                }
            }
            CorridorBlock::Fence { connected } => (
                format!(
                    "{}:{}",
                    if v == MineshaftVariant::Normal {
                        "oak_fence"
                    } else {
                        "dark_fence"
                    },
                    direction(connected)
                ),
                Block::Fence(v),
                true,
            ),
            CorridorBlock::WallTorch { facing } => {
                (format!("torch:{}", direction(facing)), Block::Torch, true)
            }
            CorridorBlock::Cobweb => ("web".into(), Block::Web, false),
            CorridorBlock::Rail { north_south } => (
                format!(
                    "rail:{}",
                    if north_south {
                        "north_south"
                    } else {
                        "east_west"
                    }
                ),
                Block::Rail,
                false,
            ),
        };
        self.write(p, &label, state);
        self.events.push(event("F", p));
        if mark {
            self.events.push(event("M", p));
        }
        Ok(())
    }
}
impl MineshaftSupportWorld for World {
    type State = Block;
    fn vertical_bounds(&self) -> (i32, i32) {
        (-64, 100)
    }
    fn block_state(&mut self, p: [i32; 3]) -> anyhow::Result<Block> {
        Ok(self.read(p))
    }
    fn replaceable_by_structure(&mut self, s: &Block) -> anyhow::Result<bool> {
        Ok(matches!(s, Block::Air | Block::Water))
    }
    fn is_lava(&mut self, _: &Block) -> anyhow::Result<bool> {
        Ok(false)
    }
    fn supports_column(&mut self, _: [i32; 3], s: &Block) -> anyhow::Result<bool> {
        Ok(Self::solid(*s))
    }
    fn supports_hanging_chain(&mut self, p: [i32; 3], s: &Block) -> anyhow::Result<bool> {
        let _ = self.read(p);
        Ok(Self::solid(*s) || matches!(s, Block::Chain))
    }
    fn set_support(
        &mut self,
        p: [i32; 3],
        b: MineshaftSupportBlock,
        v: MineshaftVariant,
    ) -> anyhow::Result<()> {
        let (label, state) = match (b, v) {
            (MineshaftSupportBlock::Wood, MineshaftVariant::Normal) => ("oak_log", Block::Wood),
            (MineshaftSupportBlock::Wood, MineshaftVariant::Mesa) => ("dark_log", Block::Wood),
            (MineshaftSupportBlock::Fence, MineshaftVariant::Normal) => {
                ("oak_fence", Block::Fence(v))
            }
            (MineshaftSupportBlock::Fence, MineshaftVariant::Mesa) => {
                ("dark_fence", Block::Fence(v))
            }
            (MineshaftSupportBlock::IronChain, _) => ("chain", Block::Chain),
        };
        self.write(p, label, state);
        Ok(())
    }
}
impl MineshaftCorridorGenerationWorld for World {
    fn upper_face_sturdy(&mut self, p: [i32; 3]) -> anyhow::Result<bool> {
        Ok(Self::solid(self.read(p)))
    }
    fn set_floor_planks(&mut self, p: [i32; 3], v: MineshaftVariant) -> anyhow::Result<()> {
        let (label, s) = if v == MineshaftVariant::Normal {
            ("oak_planks", Block::Oak)
        } else {
            ("dark_planks", Block::Dark)
        };
        self.write(p, label, s);
        Ok(())
    }
    fn is_variant_planks(&mut self, p: [i32; 3], v: MineshaftVariant) -> anyhow::Result<bool> {
        Ok(match self.read(p) {
            Block::Oak => v == MineshaftVariant::Normal,
            Block::Dark => v == MineshaftVariant::Mesa,
            _ => false,
        })
    }
    fn solid_render_floor(&mut self, p: [i32; 3]) -> anyhow::Result<bool> {
        Ok(Self::solid(self.read(p)))
    }
    fn create_chest_minecart(
        &mut self,
        _: StructureFrame,
        local: [i32; 3],
        _: BoundingBox,
        _: MineshaftVariant,
        _: &mut DecorationRandom,
    ) -> anyhow::Result<bool> {
        self.events.push(event("C", local));
        Ok(false)
    }
    fn set_spider_spawner(&mut self, p: [i32; 3], _: &mut DecorationRandom) -> anyhow::Result<()> {
        self.write(p, "spawner", Block::Spawner);
        self.events.push(event("E", p));
        Ok(())
    }
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn complete_corridor_order_matches_java() {
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
    let source = temp.path().join("MineshaftFullCorridorOracle.java");
    std::fs::write(
        &source,
        include_str!("java/MineshaftFullCorridorOracle.java"),
    )
    .unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("MineshaftFullCorridorOracle"));
    let mut lines = output.lines();
    let mut cases = 0;
    let (mut chest_attempts, mut spawners, mut rails, mut webs) = (0, 0, 0, 0);
    while let Some(header) = lines.next() {
        if !header.starts_with("C ") {
            continue;
        }
        let f: Vec<_> = header.split_whitespace().collect();
        let d: usize = f[1].parse().unwrap();
        let clip_id: usize = f[2].parse().unwrap();
        let kind: usize = f[3].parse().unwrap();
        let seed: i64 = f[4].parse().unwrap();
        let sections: i32 = f[5].parse().unwrap();
        let variant = match f[6] {
            "NORMAL" => MineshaftVariant::Normal,
            "MESA" => MineshaftVariant::Mesa,
            v => panic!("{v}"),
        };
        let expected_spider: bool = f[7].parse().unwrap();
        let count: usize = f[8].parse().unwrap();
        let z_axis = d < 2;
        let end = sections * 5 - 1;
        let bounds = BoundingBox::new(
            [0, 30, 0],
            [
                if z_axis { 2 } else { end },
                32,
                if z_axis { end } else { 2 },
            ],
        )
        .unwrap();
        let piece = MineshaftGraphPiece {
            bounds,
            direction: Some(
                [
                    HorizontalDirection::North,
                    HorizontalDirection::South,
                    HorizontalDirection::West,
                    HorizontalDirection::East,
                ][d],
            ),
            depth: 1,
            kind: MineshaftGraphPieceKind::Corridor {
                rails: kind == 1,
                spiders: kind >= 2,
                sections,
            },
        };
        let clip = match clip_id {
            0 => BoundingBox::new([-5, 25, -5], [25, 40, 25]).unwrap(),
            1 => BoundingBox::new([0, 30, 0], [7, 32, 7]).unwrap(),
            _ => BoundingBox::new([0, 29, 0], [1, 33, 9]).unwrap(),
        };
        let mut world = World {
            mode: if seed == 0 {
                1
            } else if seed == 1 {
                2
            } else {
                0
            },
            changed: HashMap::new(),
            events: vec![],
        };
        let mut spider = kind == 3;
        let mut random = DecorationRandom::new(seed);
        place_corridor(&piece, &mut spider, variant, &mut world, clip, &mut random).unwrap();
        assert_eq!(spider, expected_spider, "{header}");
        assert_eq!(random.next_long(), f[9].parse::<i64>().unwrap(), "{header}");
        assert_eq!(world.events.len(), count, "{header}");
        for (i, actual) in world.events.iter().enumerate() {
            chest_attempts += usize::from(actual.starts_with("C "));
            spawners += usize::from(actual.starts_with("E "));
            rails += usize::from(actual.contains(" rail:"));
            webs += usize::from(actual.ends_with(" web"));
            assert_eq!(Some(actual.as_str()), lines.next(), "{header}, event {i}");
        }
        cases += 1;
    }
    assert_eq!(cases, 768);
    assert!(
        chest_attempts > 0 && spawners > 0 && rails > 0 && webs > 0,
        "missing coverage: {chest_attempts} chest attempts, {spawners} spawners, {rails} rails, {webs} webs"
    );
}

#[test]
fn rejected_and_biome_blocked_corridors_do_not_consume_randomness() {
    let bounds = BoundingBox::new([0, 30, 0], [2, 32, 9]).unwrap();
    let mut piece = MineshaftGraphPiece {
        bounds,
        direction: Some(HorizontalDirection::South),
        depth: 1,
        kind: MineshaftGraphPieceKind::Corridor {
            rails: true,
            spiders: true,
            sections: 2,
        },
    };
    let clip = BoundingBox::new([-1, 25, -1], [12, 40, 12]).unwrap();
    let mut world = World {
        mode: 1,
        changed: HashMap::new(),
        events: vec![],
    };
    let mut random = DecorationRandom::new(42);
    let mut untouched = random.clone();
    let mut placed_spider = false;
    assert!(
        place_corridor(
            &piece,
            &mut placed_spider,
            MineshaftVariant::Normal,
            &mut world,
            clip,
            &mut random
        )
        .is_err()
    );
    assert!(world.events.is_empty());
    piece.kind = MineshaftGraphPieceKind::Corridor {
        rails: false,
        spiders: false,
        sections: 2,
    };
    assert_eq!(
        place_corridor(
            &piece,
            &mut placed_spider,
            MineshaftVariant::Normal,
            &mut world,
            clip,
            &mut random
        )
        .unwrap(),
        native_resources::mineshaft::MineshaftPlacementOutcome::Blocked
    );
    assert_eq!(world.events.len(), 1);
    assert_eq!(random.next_long(), untouched.next_long());
}

#[test]
fn chest_attempts_are_reachable() {
    let bounds = BoundingBox::new([0, 30, 0], [2, 32, 9]).unwrap();
    let piece = MineshaftGraphPiece {
        bounds,
        direction: Some(HorizontalDirection::North),
        depth: 1,
        kind: MineshaftGraphPieceKind::Corridor {
            rails: false,
            spiders: false,
            sections: 2,
        },
    };
    let clip = BoundingBox::new([-5, 25, -5], [25, 40, 25]).unwrap();
    for seed in 0..512 {
        let mut world = World {
            mode: 0,
            changed: HashMap::new(),
            events: vec![],
        };
        let mut random = DecorationRandom::new(seed);
        place_corridor(
            &piece,
            &mut false,
            MineshaftVariant::Normal,
            &mut world,
            clip,
            &mut random,
        )
        .unwrap();
        if world.events.iter().any(|line| line.starts_with("C ")) {
            println!("chest attempt seed: {seed}");
            return;
        }
    }
    panic!("no chest attempt in test seeds");
}
