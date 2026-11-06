//! Compare the alternating corridor support search with Java 26.3.
use native_core::beardifier::BoundingBox;
use native_resources::{
    mineshaft::{
        MineshaftSupportBlock, MineshaftSupportOutcome, MineshaftSupportWorld, MineshaftVariant,
        place_vertical_support,
    },
    structure_piece::StructureFrame,
    structure_start::HorizontalDirection,
};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

fn run(command: &mut Command) -> String {
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}
fn line(kind: &str, p: [i32; 3]) -> String {
    format!("{kind} {},{},{}", p[0], p[1], p[2])
}
#[derive(Clone, Copy)]
enum Block {
    Air,
    Water,
    Lava,
    Stone,
    Sand,
    Chain,
}
struct World {
    scenario: i32,
    base_y: i32,
    events: Vec<String>,
}
impl MineshaftSupportWorld for World {
    type State = Block;
    fn vertical_bounds(&self) -> (i32, i32) {
        if self.scenario == 10 {
            (43, 47)
        } else {
            (-64, 100)
        }
    }
    fn block_state(&mut self, p: [i32; 3]) -> anyhow::Result<Block> {
        self.events.push(line("R", p));
        let d = p[1] - self.base_y;
        Ok(match self.scenario {
            0 if d == -1 => Block::Stone,
            1 if d == -4 => Block::Stone,
            2 if d == 3 => Block::Stone,
            3 if d == -1 => Block::Lava,
            3 if d == 5 => Block::Stone,
            4 if d == -4 => Block::Stone,
            4 if d == 2 => Block::Sand,
            6 if d.abs() == 3 => Block::Stone,
            7 if d < 0 => Block::Water,
            7 if d == 51 => Block::Stone,
            8 if d == -4 => Block::Stone,
            8 if d == 1 => Block::Chain,
            9 if d == -4 || d == 1 => Block::Stone,
            _ => Block::Air,
        })
    }
    fn replaceable_by_structure(&mut self, s: &Block) -> anyhow::Result<bool> {
        Ok(matches!(s, Block::Air | Block::Water | Block::Lava))
    }
    fn is_lava(&mut self, s: &Block) -> anyhow::Result<bool> {
        Ok(matches!(s, Block::Lava))
    }
    fn supports_column(&mut self, _: [i32; 3], s: &Block) -> anyhow::Result<bool> {
        Ok(matches!(s, Block::Stone | Block::Sand))
    }
    fn supports_hanging_chain(&mut self, p: [i32; 3], s: &Block) -> anyhow::Result<bool> {
        let _ = self.block_state(p)?;
        Ok(matches!(s, Block::Chain) || (matches!(s, Block::Stone) && self.scenario != 9))
    }
    fn set_support(
        &mut self,
        p: [i32; 3],
        block: MineshaftSupportBlock,
        variant: MineshaftVariant,
    ) -> anyhow::Result<()> {
        let name = match (block, variant) {
            (MineshaftSupportBlock::Wood, MineshaftVariant::Normal) => "oak_log",
            (MineshaftSupportBlock::Wood, MineshaftVariant::Mesa) => "dark_log",
            (MineshaftSupportBlock::Fence, MineshaftVariant::Normal) => "oak_fence",
            (MineshaftSupportBlock::Fence, MineshaftVariant::Mesa) => "dark_fence",
            (MineshaftSupportBlock::IronChain, _) => "chain",
        };
        self.events.push(format!("{} {name}", line("W", p)));
        Ok(())
    }
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn vertical_support_matches_java() {
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
    let source = temp.path().join("MineshaftSupportOracle.java");
    std::fs::write(&source, include_str!("java/MineshaftSupportOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(source));
    let result = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("MineshaftSupportOracle"));
    let mut lines = result.lines();
    let mut cases = 0;
    while let Some(header) = lines.next() {
        if !header.starts_with("C ") {
            continue;
        }
        let fields: Vec<_> = header.split_whitespace().collect();
        let direction: usize = fields[1].parse().unwrap();
        let scenario: i32 = fields[2].parse().unwrap();
        let clipped: usize = fields[3].parse().unwrap();
        let variant = match fields[4] {
            "NORMAL" => MineshaftVariant::Normal,
            "MESA" => MineshaftVariant::Mesa,
            v => panic!("{v}"),
        };
        let expected_count: usize = fields[5].parse().unwrap();
        let pos: Vec<i32> = fields[6].split(',').map(|v| v.parse().unwrap()).collect();
        let bounds = BoundingBox::new([0, 46, 0], [9, 48, 9]).unwrap();
        let frame = StructureFrame {
            bounds,
            orientation: Some(
                [
                    HorizontalDirection::North,
                    HorizontalDirection::South,
                    HorizontalDirection::West,
                    HorizontalDirection::East,
                ][direction],
            ),
        };
        assert_eq!(frame.world_position([0, -1, 2]).as_slice(), pos);
        let clip = if clipped == 0 {
            BoundingBox::new([-20, -64, -20], [20, 100, 20]).unwrap()
        } else {
            BoundingBox::new([30, -64, 30], [40, 100, 40]).unwrap()
        };
        let mut world = World {
            scenario,
            base_y: pos[1],
            events: vec![],
        };
        let outcome = place_vertical_support(frame, [0, -1, 2], variant, clip, &mut world).unwrap();
        if clipped == 1 {
            assert_eq!(outcome, MineshaftSupportOutcome::OutsideClip, "{header}");
        }
        assert_eq!(world.events.len(), expected_count, "{header}");
        for (index, event) in world.events.iter().enumerate() {
            assert_eq!(
                Some(event.as_str()),
                lines.next(),
                "{header}, event {index}"
            );
        }
        cases += 1;
    }
    assert_eq!(cases, 176);
}
