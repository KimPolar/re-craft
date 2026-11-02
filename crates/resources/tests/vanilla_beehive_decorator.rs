use anyhow::Result;
use native_core::random::DecorationRandom;
use native_resources::tree_beehive::{BeehiveDecorator, BeehiveWorld};
use serde_json::Value;
use std::{collections::BTreeSet, path::PathBuf, process::Command};

struct World {
    solid: BTreeSet<[i32; 3]>,
    queries: Vec<String>,
    nest: Option<[i32; 3]>,
    bees: Vec<i32>,
    has_entity: bool,
}
impl BeehiveWorld for World {
    fn is_air(&mut self, p: [i32; 3]) -> Result<bool> {
        self.queries.push(format!("{},{},{}", p[0], p[1], p[2]));
        Ok(!self.solid.contains(&p))
    }
    fn place_bee_nest(&mut self, p: [i32; 3]) -> Result<bool> {
        self.nest = Some(p);
        Ok(self.has_entity)
    }
    fn store_bees(&mut self, _: [i32; 3], ticks: &[i32]) -> Result<()> {
        self.bees.extend_from_slice(ticks);
        Ok(())
    }
}

#[test]
fn invalid_probabilities_and_empty_trunk_do_not_modify_world() {
    assert!(BeehiveDecorator::new(f32::NAN).is_err());
    assert!(BeehiveDecorator::new(1.1).is_err());
    let mut world = World {
        solid: BTreeSet::new(),
        queries: Vec::new(),
        nest: None,
        bees: Vec::new(),
        has_entity: true,
    };
    let mut random = DecorationRandom::new(17);
    let mut reference = random.clone();
    BeehiveDecorator::new(1.0)
        .unwrap()
        .place(&mut world, &mut random, &[], &[])
        .unwrap();
    assert!(world.nest.is_none());
    assert!(world.queries.is_empty());
    assert_eq!(random.next_long(), reference.next_long());
}

fn run(c: &mut Command) -> String {
    let output = c.output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8(output.stdout).unwrap()
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn beehive_decoration_matches_java() {
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
    let source = temp.path().join("BeehiveDecoratorOracle.java");
    std::fs::write(&source, include_str!("java/BeehiveDecoratorOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let actual = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("BeehiveDecoratorOracle"));
    let mut expected = Vec::new();
    for seed in [0, 1, -1, 987654321] {
        for mode in 0..6 {
            for (probability, label) in [(0.0, "0.0"), (0.05, "0.05"), (0.5, "0.5"), (1.0, "1.0")] {
                let mut logs = Vec::new();
                let mut leaves = Vec::new();
                if mode != 0 {
                    logs.extend([[5, 4, 5], [5, 5, 5], [5, 6, 5], [5, 7, 5]]);
                }
                if mode == 5 {
                    logs.extend([[6, 6, 5], [4, 6, 5]]);
                }
                if mode != 2 && mode != 0 {
                    leaves.extend([[5, 7, 4], [6, 7, 5], [4, 7, 5]]);
                }
                let mut solid: BTreeSet<_> = logs.iter().chain(&leaves).copied().collect();
                if mode == 3 {
                    for x in 1..=9 {
                        for y in 4..=9 {
                            for z in 1..=9 {
                                solid.insert([x, y, z]);
                            }
                        }
                    }
                }
                let mut world = World {
                    solid,
                    queries: Vec::new(),
                    nest: None,
                    bees: Vec::new(),
                    has_entity: mode != 4,
                };
                let mut random = DecorationRandom::new(seed);
                BeehiveDecorator::new(probability)
                    .unwrap()
                    .place(&mut world, &mut random, &logs, &leaves)
                    .unwrap();
                let nest = world
                    .nest
                    .map(|p| format!("{},{},{}", p[0], p[1], p[2]))
                    .unwrap_or_else(|| "-".into());
                let bees = world
                    .bees
                    .iter()
                    .map(i32::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                expected.push(format!(
                    "C {seed} {mode} {label} N {nest} B {bees} Q {} R {}",
                    world.queries.join(";"),
                    random.next_long()
                ));
            }
        }
    }
    let actual: Vec<_> = actual
        .lines()
        .filter(|line| line.starts_with("C "))
        .collect();
    assert_eq!(actual.len(), expected.len());
    assert_eq!(actual.len(), 96);
    for (i, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
        assert_eq!(*actual, expected, "case {i}");
    }
}
