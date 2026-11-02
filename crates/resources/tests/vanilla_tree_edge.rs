use anyhow::Result;
use native_core::random::DecorationRandom;
use native_resources::tree_leaf_update::{
    LeafUpdateWorld, ShapeEdgeWorld, TreeBounds, finish_generated_tree, update_shape_at_edges,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    process::Command,
};

struct World {
    states: BTreeMap<[i32; 3], i32>,
    events: Vec<String>,
    random: DecorationRandom,
}
impl LeafUpdateWorld for World {
    fn optional_leaf_distance(&mut self, _: [i32; 3]) -> Result<Option<u8>> {
        Ok(None)
    }
    fn set_leaf_distance(&mut self, _: [i32; 3], _: u8) -> Result<()> {
        unreachable!()
    }
}
impl ShapeEdgeWorld for World {
    type State = i32;
    fn block(&mut self, [x, y, z]: [i32; 3]) -> Result<i32> {
        Ok(*self
            .states
            .get(&[x, y, z])
            .unwrap_or(&(x * 3 + y * 5 + z * 7).rem_euclid(16)))
    }
    fn update_shape(
        &mut self,
        [x, y, z]: [i32; 3],
        state: &i32,
        direction: [i32; 3],
        _: [i32; 3],
        neighbor: &i32,
    ) -> Result<i32> {
        let ordinal = match direction {
            [0, -1, 0] => 0,
            [0, 1, 0] => 1,
            [0, 0, -1] => 2,
            [0, 0, 1] => 3,
            [-1, 0, 0] => 4,
            [1, 0, 0] => 5,
            _ => unreachable!(),
        };
        let draw = self.random.next_int_bound(5).unwrap();
        self.events.push(format!(
            "U{x},{y},{z},{},{},{},{state},{neighbor},{draw}",
            direction[0], direction[1], direction[2]
        ));
        Ok((state * 3 + neighbor + ordinal + draw) % 16)
    }
    fn set_block(&mut self, [x, y, z]: [i32; 3], state: &i32, flags: i32) -> Result<()> {
        self.states.insert([x, y, z], *state);
        self.events.push(format!("S{x},{y},{z},{state},{flags}"));
        Ok(())
    }
}

#[test]
fn rejects_shape_outside_bounding_box() {
    let mut world = World {
        states: BTreeMap::new(),
        events: Vec::new(),
        random: DecorationRandom::new(0),
    };
    assert!(
        update_shape_at_edges(
            &mut world,
            TreeBounds {
                min: [0, 0, 0],
                max: [1, 1, 1]
            },
            &BTreeSet::from([[2, 0, 0]]),
            3
        )
        .is_err()
    );
    assert!(world.events.is_empty());
}

#[test]
fn tree_finalization_requires_placement_and_runs_edge_updates() {
    let mut world = World {
        states: BTreeMap::new(),
        events: Vec::new(),
        random: DecorationRandom::new(0),
    };
    assert!(!finish_generated_tree(&mut world, false, &[[0, 0, 0]], &[], &[], &[]).unwrap());
    assert!(!finish_generated_tree(&mut world, true, &[], &[], &[[0, 0, 0]], &[]).unwrap());
    assert!(world.events.is_empty());
    assert!(finish_generated_tree(&mut world, true, &[[0, 0, 0]], &[[1, 0, 0]], &[], &[]).unwrap());
    assert!(!world.events.is_empty());
}
fn run(c: &mut Command) -> String {
    let o = c.output().unwrap();
    assert!(
        o.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&o.stderr),
        String::from_utf8_lossy(&o.stdout)
    );
    String::from_utf8(o.stdout).unwrap()
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn edge_updates_match_java_including_mutations_and_randomness() {
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
    for lib in manifest["libraries"].as_array().unwrap() {
        if let Some(p) = lib["downloads"]["artifact"]["path"].as_str() {
            let p = game.join("libraries").join(p);
            if p.is_file() {
                paths.push(p);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("TreeEdgeOracle.java");
    std::fs::write(&source, include_str!("java/TreeEdgeOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let actual = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("TreeEdgeOracle"));
    let mut expected = Vec::new();
    for seed in [0, 1, -1, 987654321] {
        for mode in 0..5 {
            let mut world = World {
                states: BTreeMap::new(),
                events: Vec::new(),
                random: DecorationRandom::new(seed),
            };
            let mut shape = BTreeSet::new();
            for x in 0..4 {
                for y in 0..3 {
                    for z in 0..5 {
                        let full = match mode {
                            0 => false,
                            1 => true,
                            2 => x == 1 && y == 1 && z == 2,
                            3 => (x * 3 + y * 5 + z * 7) % 4 == 0,
                            _ => (x + y + z) % 2 == 0,
                        };
                        if full {
                            shape.insert([x - 3, y + 5, z + 7]);
                        }
                    }
                }
            }
            update_shape_at_edges(
                &mut world,
                TreeBounds {
                    min: [-3, 5, 7],
                    max: [0, 7, 11],
                },
                &shape,
                3,
            )
            .unwrap();
            expected.push(format!(
                "C {seed} {mode} {} R {}",
                world.events.join(";"),
                world.random.next_long()
            ));
        }
    }
    let actual: Vec<_> = actual
        .lines()
        .filter(|line| line.starts_with("C "))
        .collect();
    assert_eq!(actual.len(), expected.len());
    for (i, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
        assert_eq!(*actual, expected, "case {i}");
    }
}
