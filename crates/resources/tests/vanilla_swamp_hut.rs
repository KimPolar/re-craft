use native_core::{
    beardifier::{BoundingBox, TerrainAdjustment},
    random::LegacyRandom,
};
use native_resources::structure_piece::swamp_hut::{
    HutBlock, HutMob, SwampHutState, SwampHutWorld, place_swamp_hut,
};
use native_resources::structure_start::{
    HorizontalDirection, SimpleStructureKind, SimpleStructureStart,
};
use std::{collections::BTreeMap, path::PathBuf, process::Command};

#[derive(Default)]
struct World {
    blocks: BTreeMap<[i32; 3], HutBlock>,
    events: Vec<String>,
}
fn coords([x, y, z]: [i32; 3]) -> String {
    format!("{x},{y},{z}")
}
fn block_name(block: HutBlock) -> String {
    use HutBlock::*;
    match block {
        SprucePlanks => "spruce_planks".into(),
        OakLog => "oak_log".into(),
        OakFence => "oak_fence".into(),
        Air => "air".into(),
        PottedRedMushroom => "potted_red_mushroom".into(),
        CraftingTable => "crafting_table".into(),
        Cauldron => "cauldron".into(),
        SpruceStairs { facing, shape } => format!(
            "spruce_stairs:{}:{shape}",
            match facing {
                HorizontalDirection::North => "NORTH",
                HorizontalDirection::East => "EAST",
                HorizontalDirection::South => "SOUTH",
                HorizontalDirection::West => "WEST",
            }
        ),
    }
}
impl SwampHutWorld for World {
    fn motion_blocking_no_leaves(&mut self, _: i32, _: i32) -> anyhow::Result<i32> {
        Ok(64)
    }
    fn place_piece_block(&mut self, p: [i32; 3], b: HutBlock) -> anyhow::Result<()> {
        self.events
            .push(format!("W {} {}", coords(p), block_name(b)));
        self.blocks.insert(p, b);
        if b == HutBlock::OakFence {
            self.events.push(format!("M {}", coords(p)));
        }
        Ok(())
    }
    fn min_y(&self) -> i32 {
        -64
    }
    fn support_replaceable(&mut self, p: [i32; 3]) -> anyhow::Result<bool> {
        Ok(match self.blocks.get(&p) {
            Some(HutBlock::Air) => true,
            Some(_) => false,
            None => p[1] > 59,
        })
    }
    fn place_support_log(&mut self, p: [i32; 3]) -> anyhow::Result<()> {
        self.events.push(format!("W {} oak_log", coords(p)));
        self.blocks.insert(p, HutBlock::OakLog);
        Ok(())
    }
    fn spawn_structure_mob(
        &mut self,
        _: HutMob,
        _: [i32; 3],
        _: BoundingBox,
    ) -> anyhow::Result<()> {
        panic!("fixture mobs already spawned")
    }
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn swamp_hut_geometry_and_clipping_match_java() {
    use HorizontalDirection::{East, North, South, West};
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
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
                paths.push(path)
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("SwampHutPieceOracle.java");
    std::fs::write(&source, include_str!("java/SwampHutPieceOracle.java")).unwrap();
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
        .arg("SwampHutPieceOracle"));
    let clips = [
        BoundingBox::new([0, -64, 0], [15, 319, 15]).unwrap(),
        BoundingBox::new([0, -64, 0], [7, 319, 15]).unwrap(),
        BoundingBox::new([8, -64, 0], [15, 319, 15]).unwrap(),
        BoundingBox::new([0, -64, 0], [15, 63, 15]).unwrap(),
    ];
    let mut actual = Vec::new();
    for (d, orientation) in [North, East, South, West].into_iter().enumerate() {
        for (c, clip) in clips.into_iter().enumerate() {
            let max = if d % 2 == 0 { [9, 11] } else { [11, 9] };
            let mut piece = SimpleStructureStart {
                kind: SimpleStructureKind::SwampHut,
                chunk: [0, 0],
                generation_point: [0, 0, 0],
                orientation: Some(orientation),
                bounds: BoundingBox::new([3, 64, 3], [max[0], 70, max[1]]).unwrap(),
                terrain_adjustment: TerrainAdjustment::None,
            };
            let mut state = SwampHutState {
                spawned_witch: true,
                spawned_cat: true,
                ..Default::default()
            };
            let mut world = World::default();
            for repeat in 0..2 {
                world.events.clear();
                place_swamp_hut(&mut piece, &mut state, &mut world, clip).unwrap();
                actual.push(format!(
                    "C {d} {c} {repeat} {} {} {} {} {} {}",
                    state.ground.height_position(),
                    piece.bounds.min[1],
                    piece.bounds.max[1],
                    world.events.len(),
                    LegacyRandom::new(42).next_int(),
                    world.blocks.len()
                ));
                actual.extend(world.events.iter().cloned());
            }
        }
    }
    let expected: Vec<_> = output
        .lines()
        .filter(|line| line.starts_with("C ") || line.starts_with("W ") || line.starts_with("M "))
        .map(str::to_owned)
        .collect();
    assert_eq!(
        expected
            .iter()
            .filter(|line| line.starts_with("C "))
            .count(),
        32
    );
    assert_eq!(actual.len(), expected.len());
    for (index, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
        assert_eq!(actual, expected, "event {index}");
    }
}
