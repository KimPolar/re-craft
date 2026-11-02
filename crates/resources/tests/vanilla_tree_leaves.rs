use anyhow::Result;
use native_resources::tree_leaf_update::{LeafUpdateWorld, TreeBounds, update_tree_leaves};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf, process::Command};

#[derive(Default)]
struct World {
    blocks: BTreeMap<[i32; 3], u8>,
    writes: Vec<String>,
}
// Values 1..=7 are leaves; zero is a tagged log; absent positions are air.
impl LeafUpdateWorld for World {
    fn optional_leaf_distance(&mut self, p: [i32; 3]) -> Result<Option<u8>> {
        Ok(self.blocks.get(&p).copied())
    }
    fn set_leaf_distance(&mut self, [x, y, z]: [i32; 3], distance: u8) -> Result<()> {
        anyhow::ensure!(
            self.blocks.get(&[x, y, z]).is_some_and(|v| *v > 0),
            "not a leaf"
        );
        self.blocks.insert([x, y, z], distance);
        self.writes.push(format!("{x},{y},{z},{distance}"));
        Ok(())
    }
}

#[test]
fn edge_shape_includes_roots_but_stops_at_bounds() {
    let mut world = World::default();
    world.blocks.insert([0, 0, 0], 0);
    world.blocks.insert([1, 0, 0], 7);
    world.blocks.insert([2, 0, 0], 7);
    let shape = update_tree_leaves(
        &mut world,
        TreeBounds {
            min: [0, 0, 0],
            max: [1, 0, 0],
        },
        &[[0, 0, 0]],
        &[],
        &[[1, 0, 0]],
    )
    .unwrap();
    assert_eq!(shape.len(), 2);
    assert_eq!(world.blocks[&[1, 0, 0]], 7);
    assert_eq!(world.blocks[&[2, 0, 0]], 7);
}

fn run(c: &mut Command) -> String {
    let out = c.output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn leaf_distance_shape_and_write_order_match_java() {
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
        if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("TreeLeavesOracle.java");
    std::fs::write(&source, include_str!("java/TreeLeavesOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let actual = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("TreeLeavesOracle"));
    let mut expected = Vec::new();
    for seed in 0i32..16 {
        for mode in 0..5 {
            let mut world = World::default();
            for x in -3i32..=3 {
                for y in 1i32..=5 {
                    for z in -3i32..=3 {
                        if (x * 7 + y * 13 + z * 17 + seed).rem_euclid(5) != 0 {
                            let d = if mode == 1 {
                                1 + (x * 3 + y * 7 + z * 11 + seed).rem_euclid(7)
                            } else {
                                7
                            };
                            world.blocks.insert([x, y, z], d as u8);
                        }
                    }
                }
            }
            let mut logs = vec![[0, 0, 0], [0, 1, 0], [0, 2, 0], [9, 2, 0]];
            for &p in &logs {
                world.blocks.insert(p, 0);
            }
            if mode == 3 {
                logs.clear();
            }
            if mode == 4 {
                logs = (0..80).map(|i| [i * 1024, 0, 0]).collect();
            }
            if mode == 1 {
                world.blocks.insert([2, 3, 2], 0);
            }
            let decorations = if mode == 2 { vec![[1, 2, 0]] } else { vec![] };
            let roots = if mode == 2 { vec![[-1, 2, 0]] } else { vec![] };
            let shape = update_tree_leaves(
                &mut world,
                TreeBounds {
                    min: [-4, 0, -4],
                    max: [4, 6, 4],
                },
                &logs,
                &decorations,
                &roots,
            );
            let shape = shape.unwrap_or_else(|error| panic!("seed {seed} mode {mode}: {error:#}"));
            let mut occupied = Vec::new();
            let mut leaves = Vec::new();
            for x in -4..=4 {
                for y in 0..=6 {
                    for z in -4..=4 {
                        let p = [x, y, z];
                        if shape.contains(&p) {
                            occupied.push(format!("{x},{y},{z}"));
                        }
                        if let Some(d) = world.blocks.get(&p).filter(|d| **d != 0) {
                            leaves.push(format!("{x},{y},{z},{d}"));
                        }
                    }
                }
            }
            expected.push(format!(
                "C {seed} {mode} W {} S {} D {}",
                world.writes.join(";"),
                occupied.join(";"),
                leaves.join(";")
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
