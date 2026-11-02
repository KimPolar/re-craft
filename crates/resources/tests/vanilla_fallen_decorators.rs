use anyhow::Result;
use native_core::random::DecorationRandom;
use native_resources::tree_fallen_decorators::{
    Direction, FallenDecorationWorld, place_attached_to_logs, place_trunk_vines,
};
use std::{collections::BTreeSet, path::PathBuf, process::Command};

struct World {
    mode: i32,
    solid: BTreeSet<[i32; 3]>,
    writes: Vec<String>,
    queries: Vec<String>,
}
impl FallenDecorationWorld for World {
    fn is_air(&mut self, p: [i32; 3]) -> Result<bool> {
        self.queries.push(format!("{},{},{}", p[0], p[1], p[2]));
        Ok(!self.solid.contains(&p)
            && !(self.mode == 1 && (p[0] * 3 + p[1] * 5 + p[2] * 7).rem_euclid(5) == 0)
            && self.mode != 2)
    }
    fn place_vine(&mut self, p: [i32; 3], attachment: Direction) -> Result<()> {
        self.solid.insert(p);
        self.writes.push(format!(
            "{},{},{},vine:{}",
            p[0],
            p[1],
            p[2],
            match attachment {
                Direction::East => "east",
                Direction::West => "west",
                Direction::North => "north",
                Direction::South => "south",
                _ => unreachable!(),
            }
        ));
        Ok(())
    }
    fn place_attached(&mut self, p: [i32; 3], random: &mut DecorationRandom) -> Result<()> {
        let kind = if random.next_int_bound(3).unwrap() < 2 {
            "red"
        } else {
            "brown"
        };
        self.solid.insert(p);
        self.writes
            .push(format!("{},{},{},{}", p[0], p[1], p[2], kind));
        Ok(())
    }
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
fn rejects_invalid_attached_decorators() {
    let mut w = World {
        mode: 0,
        solid: BTreeSet::new(),
        writes: vec![],
        queries: vec![],
    };
    let mut r = DecorationRandom::new(0);
    assert!(place_attached_to_logs(&mut w, &mut r, &[], 1.0, &[]).is_err());
    assert!(place_attached_to_logs(&mut w, &mut r, &[], f32::NAN, &[Direction::Up]).is_err());
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn fallen_decorators_match_java() {
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
    let source = temp.path().join("FallenTreeDecorationOracle.java");
    std::fs::write(
        &source,
        include_str!("java/FallenTreeDecorationOracle.java"),
    )
    .unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let actual = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("FallenTreeDecorationOracle"));
    let mut expected = Vec::new();
    for seed in [0, 1, -1, 987654321] {
        for mode in 0..3 {
            for choice in 0..3 {
                for (p, label) in [(0.0, "0.0"), (0.1, "0.1"), (0.5, "0.5"), (1.0, "1.0")] {
                    for vines in [false, true] {
                        let logs = [[-3, 10, 4], [-2, 10, 4], [-1, 10, 4], [-1, 11, 4]];
                        let mut w = World {
                            mode,
                            solid: logs.into_iter().collect(),
                            writes: vec![],
                            queries: vec![],
                        };
                        let mut r = DecorationRandom::new(seed);
                        if vines {
                            place_trunk_vines(&mut w, &mut r, &logs).unwrap();
                        }
                        let directions = match choice {
                            0 => vec![Direction::Up],
                            1 => vec![Direction::North, Direction::Up],
                            _ => vec![Direction::West, Direction::South, Direction::Up],
                        };
                        place_attached_to_logs(&mut w, &mut r, &logs, p, &directions).unwrap();
                        expected.push(format!(
                            "C {seed} {mode} {choice} {label} {vines} W {} Q {} R {}",
                            w.writes.join(";"),
                            w.queries.join(";"),
                            r.next_long()
                        ));
                    }
                }
            }
        }
    }
    let actual: Vec<_> = actual.lines().filter(|v| v.starts_with("C ")).collect();
    assert_eq!(actual.len(), 288);
    assert_eq!(actual.len(), expected.len());
    for (i, (a, b)) in actual.iter().zip(&expected).enumerate() {
        assert_eq!(*a, b, "case {i}");
    }
}
