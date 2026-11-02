use anyhow::Result;
use native_core::random::DecorationRandom;
use native_resources::tree_fancy_trunk::{FancyTrunkWorld, LogAxis, place_fancy_trunk};
use std::{collections::BTreeMap, path::PathBuf, process::Command};

struct World {
    mode: i32,
    blocks: BTreeMap<[i32; 3], bool>,
    writes: Vec<String>,
    queries: Vec<String>,
}
impl World {
    fn valid(&mut self, p: [i32; 3]) -> bool {
        self.queries.push(format!("{},{},{}", p[0], p[1], p[2]));
        !self.blocks.contains_key(&p)
            && !(self.mode & 1 != 0 && (p[0] * 3 + p[1] * 5 + p[2] * 7).rem_euclid(17) == 0)
    }
}
impl FancyTrunkWorld for World {
    fn is_free(&mut self, p: [i32; 3]) -> Result<bool> {
        if self.valid(p) {
            return Ok(true);
        }
        self.queries.push(format!("{},{},{}", p[0], p[1], p[2]));
        Ok(self.blocks.get(&p) == Some(&true))
    }
    fn place_soil(&mut self, p: [i32; 3], _: &mut DecorationRandom) -> Result<()> {
        self.blocks.insert(p, false);
        self.writes.push(format!("{},{},{},soil", p[0], p[1], p[2]));
        Ok(())
    }
    fn place_log(
        &mut self,
        p: [i32; 3],
        axis: LogAxis,
        random: &mut DecorationRandom,
    ) -> Result<()> {
        if self.valid(p) {
            if self.mode & 2 != 0 {
                random.next_int_bound(17).unwrap();
            }
            self.blocks.insert(p, true);
            self.writes.push(format!(
                "{},{},{},{}",
                p[0],
                p[1],
                p[2],
                match axis {
                    LogAxis::X => "x",
                    LogAxis::Y => "y",
                    LogAxis::Z => "z",
                }
            ));
        }
        Ok(())
    }
}
#[test]
fn invalid_height_and_failed_clearance_abort_the_placement() {
    struct Fail;
    impl FancyTrunkWorld for Fail {
        fn is_free(&mut self, _: [i32; 3]) -> Result<bool> {
            anyhow::bail!("unavailable chunk")
        }
        fn place_soil(&mut self, _: [i32; 3], _: &mut DecorationRandom) -> Result<()> {
            Ok(())
        }
        fn place_log(&mut self, _: [i32; 3], _: LogAxis, _: &mut DecorationRandom) -> Result<()> {
            panic!("unexpected log write")
        }
    }
    assert!(place_fancy_trunk(&mut Fail, &mut DecorationRandom::new(1), [0, 10, 0], 81).is_err());
    assert!(place_fancy_trunk(&mut Fail, &mut DecorationRandom::new(1), [0, 10, 0], 10).is_err());
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
fn fancy_trunks_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: serde_json::Value =
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
    let source = temp.path().join("FancyTrunkOracle.java");
    std::fs::write(&source, include_str!("java/FancyTrunkOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let actual = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("FancyTrunkOracle"));
    let mut expected = Vec::new();
    for seed in [0, 1, -1, 987654321] {
        for height in [0, 1, 3, 4, 6, 10, 20, 40, 64, 80] {
            for mode in 0..4 {
                let mut world = World {
                    mode,
                    blocks: BTreeMap::new(),
                    writes: Vec::new(),
                    queries: Vec::new(),
                };
                let mut random = DecorationRandom::new(seed);
                let attachments =
                    place_fancy_trunk(&mut world, &mut random, [3, 10, -4], height).unwrap();
                let points = attachments
                    .iter()
                    .map(|a| {
                        format!(
                            "{},{},{},{},{},{}",
                            a.position[0],
                            a.position[1],
                            a.position[2],
                            a.radius_offset,
                            a.height_offset,
                            a.double_trunk
                        )
                    })
                    .collect::<Vec<_>>();
                expected.push(format!(
                    "C {seed} {height} {mode} W {} A {} Q {} R {}",
                    world.writes.join(";"),
                    points.join(";"),
                    world.queries.join(";"),
                    random.next_long()
                ));
            }
        }
    }
    let actual: Vec<_> = actual.lines().filter(|v| v.starts_with("C ")).collect();
    assert_eq!(actual.len(), expected.len());
    for (i, (a, b)) in actual.iter().zip(&expected).enumerate() {
        assert_eq!(*a, b, "case {i}");
    }
}
