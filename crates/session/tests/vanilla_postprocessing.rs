//! Compare the marked-position part of chunk postprocessing with Java 26.3.
use anyhow::{Result, bail};
use native_core::postprocessing::PostProcessing;
use native_session::worldgen::postprocessing::{
    PostprocessingBlocks, PostprocessingChunk, PostprocessingTask, PostprocessingWorld,
};
use native_world::loaded::ChunkPos;
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf, process::Command, sync::Arc};

#[derive(Default)]
struct World {
    blocks: BTreeMap<[i32; 3], char>,
    events: Vec<String>,
    fail: bool,
    fail_at: Option<&'static str>,
    trace_lifecycle: bool,
    pending: Vec<[i32; 3]>,
}
fn pos(p: [i32; 3]) -> String {
    format!("{},{},{}", p[0], p[1], p[2])
}
impl PostprocessingWorld for World {
    type State = char;
    type Fluid = ();
    fn block(&mut self, p: [i32; 3]) -> Result<char> {
        Ok(self.blocks[&p])
    }
    fn fluid(&mut self, s: &char) -> Result<Option<()>> {
        Ok(matches!(s, 'a' | 'b').then_some(()))
    }
    fn tick_fluid(&mut self, p: [i32; 3], s: &char, _: &()) -> Result<()> {
        self.events.push(format!("F {} {s}", pos(p)));
        self.blocks.insert(p, 'c');
        if self.fail {
            bail!("fixture failure after changing world");
        }
        Ok(())
    }
    fn is_liquid_block(&mut self, s: &char) -> Result<bool> {
        Ok(*s == 'a')
    }
    fn tick_liquid_block(&mut self, p: [i32; 3], s: &char) -> Result<()> {
        self.events.push(format!("L {} {s}", pos(p)));
        self.blocks.insert(p, 'b');
        Ok(())
    }
    fn update_neighbor_shapes(&mut self, p: [i32; 3], s: &char) -> Result<char> {
        self.events.push(format!("S {} {s}", pos(p)));
        Ok(if *s == 'b' { 'd' } else { *s })
    }
    fn set_block(&mut self, p: [i32; 3], s: &char, flags: i32) -> Result<()> {
        self.events.push(format!("W {} {s} {flags}", pos(p)));
        self.blocks.insert(p, *s);
        Ok(())
    }
    fn clear_processed_section(&mut self, section: usize) -> Result<()> {
        if self.trace_lifecycle {
            self.events.push(format!("CS {section}"));
        }
        if self.fail_at == Some("section") {
            bail!("section clear failed");
        }
        Ok(())
    }
}

impl PostprocessingChunk for World {
    fn pending_block_entity_positions(&mut self) -> Result<Vec<[i32; 3]>> {
        if self.fail_at == Some("capture") {
            bail!("capture failed");
        }
        Ok(self.pending.clone())
    }
    fn promote_pending_block_entity(&mut self, p: [i32; 3]) -> Result<()> {
        self.events.push(format!("P {}", pos(p)));
        if p == [-16, -15, 32] {
            self.pending.retain(|&p| p != [-15, 0, 32]);
            self.pending.push([-14, 0, 32]);
        }
        if self.fail_at == Some("panic") {
            panic!("promotion panicked after mutating pending entities");
        }
        if self.fail_at == Some("promote") {
            bail!("promotion failed");
        }
        Ok(())
    }
    fn clear_pending_block_entities(&mut self) -> Result<()> {
        self.events.push("CE".into());
        self.pending.clear();
        if self.fail_at == Some("clear") {
            bail!("clear failed");
        }
        Ok(())
    }
    fn apply_upgrade_data(&mut self) -> Result<()> {
        self.events.push("U".into());
        if self.fail_at == Some("upgrade") {
            bail!("upgrade failed");
        }
        Ok(())
    }
}
fn fixture() -> (Arc<PostProcessing>, World) {
    let p1 = [-16, -15, 32];
    let p2 = [-15, 0, 32];
    let p3 = [-14, 0, 32];
    let mut marks = PostProcessing::new(-16, 32).unwrap();
    for p in [p1, p1, p2, p3] {
        marks.mark(p[0], p[1], p[2]);
    }
    (
        Arc::new(marks),
        World {
            blocks: BTreeMap::from([(p1, 'a'), (p2, 'b'), (p3, 'c')]),
            ..Default::default()
        },
    )
}

#[test]
fn complete_pass_preserves_order_snapshots_pending_and_poisoning() {
    let mut expected = None;
    for budget in [1, 2, 3, 64] {
        let (marks, mut world) = fixture();
        world.trace_lifecycle = true;
        world.pending = vec![[-16, -15, 32], [-15, 0, 32]];
        let mut task = PostprocessingTask::new(marks, ChunkPos { x: -1, z: 2 });
        assert!(!task.poll(&mut world, 0).unwrap());
        assert!(world.events.is_empty());
        while !task.poll(&mut world, budget).unwrap() {}
        assert!(task.poll(&mut world, 0).unwrap());
        assert!(world.pending.is_empty());
        if let Some(expected) = &expected {
            assert_eq!(&world.events, expected);
        } else {
            expected = Some(world.events.clone());
        }
    }
    assert_eq!(
        expected.unwrap(),
        [
            "F -16,-15,32 a",
            "L -16,-15,32 a",
            "F -16,-15,32 b",
            "S -16,-15,32 b",
            "W -16,-15,32 d 276",
            "CS 0",
            "F -15,0,32 b",
            "S -15,0,32 b",
            "W -15,0,32 d 276",
            "S -14,0,32 c",
            "CS 1",
            "P -16,-15,32",
            "P -15,0,32",
            "CE",
            "U"
        ]
    );
    for phase in ["section", "capture", "promote", "clear", "upgrade"] {
        let (marks, mut world) = fixture();
        world.trace_lifecycle = true;
        world.pending = vec![[-16, -15, 32], [-15, 0, 32]];
        world.fail_at = Some(phase);
        let mut task = PostprocessingTask::new(marks, ChunkPos { x: -1, z: 2 });
        assert!(task.poll(&mut world, 64).is_err(), "{phase}");
        let events = world.events.clone();
        assert!(task.poll(&mut world, 64).is_err(), "{phase}");
        assert_eq!(world.events, events, "{phase}");
    }
    let (marks, mut world) = fixture();
    world.pending = vec![[-16, -15, 32], [-15, 0, 32]];
    world.fail_at = Some("panic");
    let mut task = PostprocessingTask::new(marks, ChunkPos { x: -1, z: 2 });
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| task.poll(&mut world, 64)))
            .is_err()
    );
    let events = world.events.clone();
    assert!(task.poll(&mut world, 64).is_err());
    assert_eq!(world.events, events);
}
#[test]
fn batches_preserve_order_and_failure_cannot_be_retried() {
    let (marks, mut world) = fixture();
    let mut run = PostprocessingBlocks::new(marks, ChunkPos { x: -1, z: 2 });
    assert!(!run.poll(&mut world, 0).unwrap());
    assert!(!run.poll(&mut world, 1).unwrap());
    assert!(!run.poll(&mut world, 2).unwrap());
    assert!(run.poll(&mut world, 1).unwrap());
    let events = world.events.clone();
    assert!(run.poll(&mut world, 10).unwrap());
    assert_eq!(events, world.events);
    assert_eq!(
        events,
        [
            "F -16,-15,32 a",
            "L -16,-15,32 a",
            "F -16,-15,32 b",
            "S -16,-15,32 b",
            "W -16,-15,32 d 276",
            "F -15,0,32 b",
            "S -15,0,32 b",
            "W -15,0,32 d 276",
            "S -14,0,32 c"
        ]
    );
    let (marks, mut world) = fixture();
    world.fail = true;
    let mut failed = PostprocessingBlocks::new(marks, ChunkPos { x: -1, z: 2 });
    assert!(failed.poll(&mut world, 1).is_err());
    let events = world.events.clone();
    assert!(failed.poll(&mut world, 1).is_err());
    assert_eq!(events, world.events);
}

fn run(command: &mut Command) -> String {
    let result = command.output().unwrap();
    assert!(
        result.status.success(),
        "oracle failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn marked_positions_match_java_level_chunk() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"].as_array().unwrap() {
        if let Some(p) = lib["downloads"]["artifact"]["path"].as_str() {
            let p = game.join("libraries").join(p);
            if p.is_file() {
                paths.push(p)
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("PostprocessingOracle.java");
    std::fs::write(&source, include_str!("java/PostprocessingOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("PostprocessingOracle"));
    let (marks, mut world) = fixture();
    let mut runner = PostprocessingBlocks::new(marks, ChunkPos { x: -1, z: 2 });
    while !runner.poll(&mut world, 1).unwrap() {}
    let java_events: Vec<_> = output
        .lines()
        .filter(|line| {
            ["F ", "L ", "S ", "W "]
                .iter()
                .any(|prefix| line.starts_with(prefix))
        })
        .collect();
    assert_eq!(
        java_events,
        world.events.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert!(output.lines().any(|line| line == "E 0 0"));

    let (marks, mut world) = fixture();
    world.trace_lifecycle = true;
    world.pending = vec![[-16, -15, 32], [-15, 0, 32]];
    let mut task = PostprocessingTask::new(marks, ChunkPos { x: -1, z: 2 });
    while !task.poll(&mut world, 1).unwrap() {}
    let java_lifecycle: Vec<_> = output
        .lines()
        .filter(|line| {
            ["F ", "L ", "S ", "W ", "CS ", "P ", "CE", "U"]
                .iter()
                .any(|prefix| line.starts_with(prefix))
        })
        .collect();
    assert_eq!(
        java_lifecycle,
        world.events.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert!(output.lines().any(|line| line == "BE 0"));
}
