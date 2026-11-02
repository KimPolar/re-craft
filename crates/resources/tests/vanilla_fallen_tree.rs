use anyhow::Result;
use native_core::random::DecorationRandom;
use native_resources::{
    tree_fallen::{FallenTreeLayout, FallenTreeWorld},
    tree_fancy_trunk::LogAxis,
};
use serde_json::json;
use std::{collections::BTreeSet, path::PathBuf, process::Command};

struct World {
    mode: i32,
    logs: BTreeSet<[i32; 3]>,
    writes: Vec<String>,
    queries: Vec<String>,
}
impl World {
    fn stone(&self, p: [i32; 3]) -> bool {
        if self.logs.contains(&p) {
            return true;
        }
        let mut solid = p[1] < 10;
        match self.mode & 3 {
            1 if p[1] == 10 && (p[0] * 3 + p[2] * 7).rem_euclid(7) == 0 => solid = true,
            2 if p[1] >= 7 && (p[0] * 3 + p[2] * 7).rem_euclid(4) != 0 => solid = false,
            3 if p[1] >= 5 => solid = false,
            _ => {}
        }
        solid
    }
}
impl FallenTreeWorld for World {
    fn replaceable(&mut self, p: [i32; 3]) -> Result<bool> {
        self.queries.push(format!("I{},{},{}", p[0], p[1], p[2]));
        Ok(!self.stone(p))
    }
    fn sturdy_below(&mut self, p: [i32; 3]) -> Result<bool> {
        let q = [p[0], p[1] - 1, p[2]];
        self.queries.push(format!("S{},{},{}", q[0], q[1], q[2]));
        Ok(self.stone(q))
    }
    fn place_log(
        &mut self,
        p: [i32; 3],
        axis: Option<LogAxis>,
        random: &mut DecorationRandom,
    ) -> Result<()> {
        if self.mode & 4 != 0 {
            random.next_int_bound(17).unwrap();
        }
        self.logs.insert(p);
        self.writes.push(format!(
            "{},{},{},{}",
            p[0],
            p[1],
            p[2],
            match axis {
                Some(LogAxis::X) => "x",
                Some(LogAxis::Z) => "z",
                _ => "y",
            }
        ));
        Ok(())
    }
    fn postprocess_above(&mut self, p: [i32; 3]) -> Result<()> {
        for dy in 1..=2 {
            let q = [p[0], p[1] + dy, p[2]];
            self.queries.push(format!("S{},{},{}", q[0], q[1], q[2]));
            if !self.stone(q) {
                break;
            }
            anyhow::bail!("unexpected non-air postprocessing fixture");
        }
        Ok(())
    }
    fn decorate_stump(&mut self, _: [i32; 3], _: &mut DecorationRandom) -> Result<()> {
        Ok(())
    }
    fn decorate_logs(&mut self, _: &[[i32; 3]], _: &mut DecorationRandom) -> Result<()> {
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
fn rejects_out_of_range_fallen_lengths() {
    for n in [-1, 17] {
        assert!(FallenTreeLayout::decode(&json!({"log_length":n})).is_err());
    }
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn fallen_tree_layout_matches_java() {
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
    let source = temp.path().join("FallenTreeOracle.java");
    std::fs::write(&source, include_str!("java/FallenTreeOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let actual = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("FallenTreeOracle"));
    let mut expected = Vec::new();
    for seed in [0, 1, -1, 987654321] {
        for length in [0, 1, 4, 7, 16] {
            for mode in 0..8 {
                let mut world = World {
                    mode,
                    logs: BTreeSet::new(),
                    writes: Vec::new(),
                    queries: Vec::new(),
                };
                let mut random = DecorationRandom::new(seed);
                let result = FallenTreeLayout::decode(&json!({"log_length":length}))
                    .unwrap()
                    .place(&mut world, &mut random, [-3, 10, 4])
                    .unwrap();
                expected.push(format!(
                    "C {seed} {length} {mode} {result} W {} Q {} R {}",
                    world.writes.join(";"),
                    world.queries.join(";"),
                    random.next_long()
                ));
            }
        }
    }
    let actual: Vec<_> = actual.lines().filter(|v| v.starts_with("C ")).collect();
    assert_eq!(actual.len(), 160);
    assert_eq!(actual.len(), expected.len());
    for (i, (a, b)) in actual.iter().zip(&expected).enumerate() {
        assert_eq!(*a, b, "case {i}");
    }
}
