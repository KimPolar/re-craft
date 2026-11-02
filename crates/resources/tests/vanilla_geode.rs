use anyhow::Result;
use native_core::random::DecorationRandom;
use native_resources::geode_feature::{
    Direction, GeodeLayer, GeodeNumbers, GeodeShape, GeodeWorld, place_geode,
};
use std::{collections::BTreeMap, path::PathBuf, process::Command};

struct Numbers;
impl GeodeNumbers for Numbers {
    fn distribution_points(&self, r: &mut DecorationRandom) -> i32 {
        3 + r.next_int_bound(2).unwrap()
    }
    fn outer_wall_distance(&self, r: &mut DecorationRandom) -> i32 {
        4 + r.next_int_bound(3).unwrap()
    }
    fn outer_wall_max(&self) -> i32 {
        6
    }
    fn point_offset(&self, r: &mut DecorationRandom) -> i32 {
        1 + r.next_int_bound(2).unwrap()
    }
}
fn shape() -> GeodeShape {
    GeodeShape {
        filling: 1.7,
        inner: 2.2,
        middle: 3.2,
        outer: 4.2,
        crack_chance: 0.95,
        crack_size: 2.0,
        crack_point_offset: 2,
        alternate_chance: 0.083,
        placement_chance: 0.35,
        placement_requires_alternate: true,
        min_offset: -16,
        max_offset: 16,
        noise_multiplier: 0.05,
        invalid_threshold: 1,
    }
}
#[derive(Clone)]
struct State {
    name: &'static str,
    facing: Option<Direction>,
    wet: bool,
}
fn s(name: &'static str) -> State {
    State {
        name,
        facing: None,
        wet: false,
    }
}
fn name(state: &State) -> String {
    let mut result = state.name.to_owned();
    if let Some(d) = state.facing {
        result.push_str(&format!(
            ":{}:{}",
            match d {
                Direction::Down => "down",
                Direction::Up => "up",
                Direction::North => "north",
                Direction::South => "south",
                Direction::West => "west",
                Direction::East => "east",
            },
            state.wet
        ));
    }
    result
}
struct World {
    mode: i32,
    blocks: BTreeMap<[i32; 3], State>,
    writes: Vec<String>,
    ticks: Vec<String>,
}
impl GeodeWorld for World {
    type State = State;
    fn block(&mut self, p: [i32; 3]) -> Result<State> {
        Ok(self.blocks.get(&p).cloned().unwrap_or_else(|| {
            if self.mode == 2 {
                s("air")
            } else if self.mode == 1 && (p[0] + p[1] + p[2]).rem_euclid(17) == 0 {
                s("bedrock")
            } else if self.mode == 1 && (p[0] * 3 + p[1] + p[2]).rem_euclid(23) == 0 {
                s("water")
            } else {
                s("stone")
            }
        }))
    }
    fn is_air_or_invalid(&mut self, s: &State) -> Result<bool> {
        Ok(s.name == "air" || s.name == "bedrock")
    }
    fn may_replace(&mut self, s: &State) -> Result<bool> {
        Ok(s.name != "bedrock")
    }
    fn layer_state(
        &mut self,
        l: GeodeLayer,
        _: [i32; 3],
        _: &mut DecorationRandom,
    ) -> Result<State> {
        Ok(s(match l {
            GeodeLayer::Filling => "air",
            GeodeLayer::Inner => "amethyst_block",
            GeodeLayer::AlternateInner => "budding_amethyst",
            GeodeLayer::Middle => "calcite",
            GeodeLayer::Outer => "smooth_basalt",
        }))
    }
    fn air(&mut self) -> Result<State> {
        Ok(s("air"))
    }
    fn set_block(&mut self, p: [i32; 3], s: &State) -> Result<()> {
        self.blocks.insert(p, s.clone());
        self.writes
            .push(format!("{},{},{},{}", p[0], p[1], p[2], name(s)));
        Ok(())
    }
    fn schedule_fluid(&mut self, p: [i32; 3]) -> Result<()> {
        let s = self.block(p)?;
        if s.name == "water" || s.wet {
            self.ticks.push(format!("{},{},{}", p[0], p[1], p[2]));
        }
        Ok(())
    }
    fn placement_count(&self) -> usize {
        4
    }
    fn placement_state(&mut self, i: usize, d: Direction, existing: &State) -> Result<State> {
        Ok(State {
            name: [
                "small_amethyst_bud",
                "medium_amethyst_bud",
                "large_amethyst_bud",
                "amethyst_cluster",
            ][i],
            facing: Some(d),
            wet: existing.name == "water" || existing.wet,
        })
    }
    fn can_grow(&mut self, s: &State) -> Result<bool> {
        Ok(s.name == "air" || s.name == "water")
    }
}
fn run(cmd: &mut Command) -> String {
    let o = cmd.output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout).unwrap()
}
#[test]
fn invalid_distribution_points_stop_before_any_writes() {
    for seed in [0, 1, -1, 987654321] {
        let mut world = World {
            mode: 2,
            blocks: BTreeMap::new(),
            writes: vec![],
            ticks: vec![],
        };
        let mut random = DecorationRandom::new(seed);
        assert!(
            !place_geode(
                &mut world,
                &mut random,
                seed + 123,
                [0, 20, 0],
                &shape(),
                &Numbers
            )
            .unwrap()
        );
        assert!(world.writes.is_empty() && world.ticks.is_empty());
        let mut reference = DecorationRandom::new(seed);
        let _ = Numbers.distribution_points(&mut reference);
        let _ = reference.next_double();
        let _ = reference.next_float();
        for _ in 0..3 {
            let _ = Numbers.outer_wall_distance(&mut reference);
        }
        let _ = Numbers.point_offset(&mut reference);
        for _ in 0..3 {
            let _ = Numbers.outer_wall_distance(&mut reference);
        }
        assert_eq!(random.next_long(), reference.next_long());
    }
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn geode_matches_java() {
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
    let source = temp.path().join("GeodeOracle.java");
    std::fs::write(&source, include_str!("java/GeodeOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let actual = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("GeodeOracle"));
    let mut expected = vec![];
    for seed in [0, 1, -1, 987654321] {
        for mode in 0..3 {
            let mut world = World {
                mode,
                blocks: BTreeMap::new(),
                writes: vec![],
                ticks: vec![],
            };
            let mut random = DecorationRandom::new(seed);
            let ok = place_geode(
                &mut world,
                &mut random,
                seed + 123,
                [0, 20, 0],
                &shape(),
                &Numbers,
            )
            .unwrap();
            expected.push(format!(
                "C {seed} {mode} {ok} W {} T {} R {}",
                world.writes.join(";"),
                world.ticks.join(";"),
                random.next_long()
            ));
        }
    }
    let actual: Vec<_> = actual.lines().filter(|l| l.starts_with("C ")).collect();
    assert_eq!(actual.len(), expected.len());
    for (i, (a, b)) in actual.iter().zip(expected.iter()).enumerate() {
        assert_eq!(*a, b, "case {i}");
    }
}
