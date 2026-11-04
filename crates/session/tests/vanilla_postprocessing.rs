//! Compare the marked-position part of chunk postprocessing with Java 26.3.
use anyhow::{Result, bail};
use native_core::postprocessing::PostProcessing;
use native_resources::tree_leaf_update::ShapeEdgeWorld;
use native_session::worldgen::postprocessing::{
    PostprocessingBlocks, PostprocessingChunk, PostprocessingTask, PostprocessingWorld,
    update_from_neighbor_shapes,
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

struct ShapeTrace {
    case: usize,
    origin: [i32; 3],
    neighbors: BTreeMap<[i32; 3], String>,
    events: Vec<String>,
    fail_after: Option<usize>,
}

impl ShapeEdgeWorld for ShapeTrace {
    type State = String;
    fn block(&mut self, p: [i32; 3]) -> Result<String> {
        self.neighbors
            .get(&p)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing neighbor"))
    }
    fn update_shape(
        &mut self,
        _: [i32; 3],
        state: &String,
        direction: [i32; 3],
        neighbor: [i32; 3],
        other: &String,
    ) -> Result<String> {
        let name = match direction {
            [-1, 0, 0] => "west",
            [1, 0, 0] => "east",
            [0, 0, -1] => "north",
            [0, 0, 1] => "south",
            [0, -1, 0] => "down",
            [0, 1, 0] => "up",
            _ => unreachable!(),
        };
        self.events.push(format!(
            "N {} {state} {name} {} {other}",
            self.case,
            pos(neighbor)
        ));
        if direction == [-1, 0, 0] {
            self.neighbors.insert(
                [
                    self.origin[0].wrapping_add(1),
                    self.origin[1],
                    self.origin[2],
                ],
                "changed".into(),
            );
        }
        if self.fail_after == Some(self.events.len()) {
            bail!("shape callback failed");
        }
        let index: usize = state.strip_prefix('S').unwrap().parse().unwrap();
        Ok(format!("S{}", index + 1))
    }
    fn set_block(&mut self, _: [i32; 3], _: &String, _: i32) -> Result<()> {
        panic!("shape calculation must not write the resulting block")
    }
}

fn shape_fixture(case: usize, origin: [i32; 3]) -> ShapeTrace {
    let directions = [
        [-1i32, 0, 0],
        [1, 0, 0],
        [0, 0, -1],
        [0, 0, 1],
        [0, -1, 0],
        [0, 1, 0],
    ];
    let neighbors = directions
        .into_iter()
        .enumerate()
        .map(|(i, direction)| {
            (
                [
                    origin[0].wrapping_add(direction[0]),
                    origin[1].wrapping_add(direction[1]),
                    origin[2].wrapping_add(direction[2]),
                ],
                format!("N{i}"),
            )
        })
        .collect();
    ShapeTrace {
        case,
        origin,
        neighbors,
        events: Vec::new(),
        fail_after: None,
    }
}

fn shape_trace() -> Vec<String> {
    let mut events = Vec::new();
    for (case, origin) in [
        [8, 4, 8],
        [i32::MAX, 0, i32::MIN],
        [i32::MIN, -64, i32::MAX],
    ]
    .into_iter()
    .enumerate()
    {
        let mut world = shape_fixture(case, origin);
        let result = update_from_neighbor_shapes(&mut world, origin, &"S0".to_owned()).unwrap();
        events.extend(world.events);
        events.push(format!("R {case} {result}"));
    }
    events
}

struct ShapePostprocessing {
    shape: ShapeTrace,
}

impl PostprocessingWorld for ShapePostprocessing {
    type State = String;
    type Fluid = ();
    fn block(&mut self, _: [i32; 3]) -> Result<String> {
        Ok("S0".into())
    }
    fn fluid(&mut self, _: &String) -> Result<Option<()>> {
        Ok(None)
    }
    fn tick_fluid(&mut self, _: [i32; 3], _: &String, _: &()) -> Result<()> {
        unreachable!()
    }
    fn is_liquid_block(&mut self, _: &String) -> Result<bool> {
        Ok(false)
    }
    fn tick_liquid_block(&mut self, _: [i32; 3], _: &String) -> Result<()> {
        unreachable!()
    }
    fn update_neighbor_shapes(&mut self, p: [i32; 3], original: &String) -> Result<String> {
        update_from_neighbor_shapes(&mut self.shape, p, original)
    }
    fn set_block(&mut self, p: [i32; 3], state: &String, flags: i32) -> Result<()> {
        self.shape
            .events
            .push(format!("W {} {state} {flags}", pos(p)));
        Ok(())
    }
    fn clear_processed_section(&mut self, section: usize) -> Result<()> {
        self.shape.events.push(format!("CS {section}"));
        Ok(())
    }
}

impl PostprocessingChunk for ShapePostprocessing {
    fn pending_block_entity_positions(&mut self) -> Result<Vec<[i32; 3]>> {
        Ok(Vec::new())
    }
    fn promote_pending_block_entity(&mut self, _: [i32; 3]) -> Result<()> {
        unreachable!()
    }
    fn clear_pending_block_entities(&mut self) -> Result<()> {
        self.shape.events.push("CE".into());
        Ok(())
    }
    fn apply_upgrade_data(&mut self) -> Result<()> {
        self.shape.events.push("U".into());
        Ok(())
    }
}

fn shape_postprocessing() -> (PostprocessingTask, ShapePostprocessing) {
    let origin = [-16, -15, 32];
    let mut marks = PostProcessing::new(-16, 32).unwrap();
    assert!(marks.mark(origin[0], origin[1], origin[2]));
    (
        PostprocessingTask::new(Arc::new(marks), ChunkPos { x: -1, z: 2 }),
        ShapePostprocessing {
            shape: shape_fixture(3, origin),
        },
    )
}

#[test]
fn neighbor_shapes_chain_states_and_read_live_neighbors() {
    let events = shape_trace();
    assert_eq!(events.len(), 21);
    assert_eq!(events[0], "N 0 S0 west 7,4,8 N0");
    assert_eq!(events[1], "N 0 S1 east 9,4,8 changed");
    assert_eq!(events[6], "R 0 S6");
    let origin = [8, 4, 8];
    let mut world = shape_fixture(0, origin);
    world.fail_after = Some(3);
    assert!(update_from_neighbor_shapes(&mut world, origin, &"S0".to_owned()).is_err());
    assert_eq!(world.events.len(), 3);
    let (mut task, mut world) = shape_postprocessing();
    while !task.poll(&mut world, 1).unwrap() {}
    assert_eq!(world.shape.events[6], "W -16,-15,32 S6 276");
    assert_eq!(&world.shape.events[7..], ["CS 0", "CE", "U"]);
    let (mut task, mut world) = shape_postprocessing();
    world.shape.fail_after = Some(3);
    assert!(task.poll(&mut world, 10).is_err());
    let events = world.shape.events.clone();
    assert!(task.poll(&mut world, 10).is_err());
    assert_eq!(world.shape.events, events);
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
    let all_output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("PostprocessingOracle"));
    let (output, remaining) = all_output.split_once("BEGIN_SHAPES").unwrap();
    let postprocessing_shapes: Vec<_> = remaining
        .lines()
        .filter(|line| !line.is_empty())
        .take_while(|line| *line != "END_SHAPES")
        .collect();
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
    let java_shapes: Vec<_> = output
        .lines()
        .filter(|line| line.starts_with("N ") || line.starts_with("R "))
        .collect();
    assert_eq!(
        java_shapes,
        shape_trace().iter().map(String::as_str).collect::<Vec<_>>()
    );
    let (mut task, mut world) = shape_postprocessing();
    while !task.poll(&mut world, 1).unwrap() {}
    assert_eq!(
        postprocessing_shapes,
        world
            .shape
            .events
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    );
}
