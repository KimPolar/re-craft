use anyhow::Result;
use native_core::random::DecorationRandom;
use native_resources::{
    multiface_growth::{BlockSet, Face, MultifaceGrowth, MultifaceWorld},
    packs::ResourceId,
};
use std::{collections::BTreeMap, path::PathBuf, process::Command};
#[derive(Clone, Debug)]
enum State {
    Air,
    Stone,
    Water(u8),
    Lichen(u8, bool),
}
fn index(face: Face) -> u8 {
    match face {
        Face::Down => 0,
        Face::Up => 1,
        Face::North => 2,
        Face::South => 3,
        Face::West => 4,
        Face::East => 5,
    }
}
struct World {
    mode: i32,
    blocks: BTreeMap<[i32; 3], State>,
    writes: Vec<String>,
    marks: Vec<String>,
}
impl World {
    fn initial(&self, p: [i32; 3]) -> State {
        let [x, y, z] = [p[0], p[1] - 20, p[2]];
        if self.mode == 0 {
            return State::Air;
        }
        if self.mode == 1 && y >= 1 {
            return State::Stone;
        }
        if self.mode >= 2 && (x >= 2 || y >= 2 || z >= 2 || x <= -2 || y <= -2 || z <= -2) {
            return State::Stone;
        }
        if self.mode == 3 {
            return State::Water(0);
        }
        if self.mode == 4 {
            return State::Water(5);
        }
        if self.mode == 5 && [x, y, z] == [1, 0, 0] {
            return State::Lichen(1 << 5, false);
        }
        State::Air
    }
}
impl MultifaceWorld for World {
    type State = State;
    fn block(&mut self, p: [i32; 3]) -> Result<State> {
        Ok(self
            .blocks
            .get(&p)
            .cloned()
            .unwrap_or_else(|| self.initial(p)))
    }
    fn air_or_water(&mut self, s: &State) -> Result<bool> {
        Ok(matches!(s, State::Air | State::Water(_)))
    }
    fn is_growth(&mut self, s: &State, _: &ResourceId) -> Result<bool> {
        Ok(matches!(s, State::Lichen(..)))
    }
    fn in_block_set(&mut self, s: &State, _: &BlockSet) -> Result<bool> {
        Ok(matches!(s, State::Stone))
    }
    fn state_for_placement(
        &mut self,
        _: &ResourceId,
        s: &State,
        p: [i32; 3],
        f: Face,
    ) -> Result<Option<State>> {
        let bit = 1 << index(f);
        if matches!(s,State::Lichen(mask,_) if mask&bit!=0)
            || !matches!(self.block(f.offset(p))?, State::Stone)
        {
            return Ok(None);
        }
        let (mask, wet) = match s {
            State::Lichen(m, w) => (*m, *w),
            State::Water(0) => (0, true),
            _ => (0, false),
        };
        Ok(Some(State::Lichen(mask | bit, wet)))
    }
    fn has_face(&mut self, s: &State, f: Face) -> Result<bool> {
        Ok(matches!(s,State::Lichen(m,_) if m&(1<<index(f))!=0))
    }
    fn can_replace_for_spread(&mut self, s: &State, _: &ResourceId) -> Result<bool> {
        Ok(matches!(
            s,
            State::Air | State::Water(0) | State::Lichen(..)
        ))
    }
    fn set_growth(&mut self, p: [i32; 3], s: &State, initial: bool) -> Result<bool> {
        let State::Lichen(mask, wet) = s else {
            panic!()
        };
        self.blocks.insert(p, s.clone());
        self.writes.push(format!(
            "{},{},{},{}:{},{}",
            p[0], p[1], p[2], mask, wet, initial
        ));
        Ok(true)
    }
    fn mark_postprocessing(&mut self, p: [i32; 3]) -> Result<()> {
        self.marks.push(format!("{},{},{}", p[0], p[1], p[2]));
        Ok(())
    }
}
fn run(cmd: &mut Command) -> String {
    let o = cmd.output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout).unwrap()
}
#[test]
fn empty_cave_consumes_shuffles_without_creating_blocks() {
    let value = serde_json::json!({"type":"minecraft:multiface_growth","block":"minecraft:glow_lichen","search_range":20,"can_place_on_ceiling":true,"can_place_on_wall":true,"can_be_placed_on":["minecraft:stone"]});
    let feature = MultifaceGrowth::decode(&value).unwrap();
    let mut w = World {
        mode: 0,
        blocks: BTreeMap::new(),
        writes: vec![],
        marks: vec![],
    };
    let mut random = DecorationRandom::new(42);
    assert!(!feature.place(&mut w, &mut random, [0, 20, 0]).unwrap());
    assert!(w.writes.is_empty() && w.marks.is_empty());
    let mut reference = DecorationRandom::new(42);
    let mut dirs = [Face::Up, Face::North, Face::East, Face::South, Face::West];
    for n in (2..=dirs.len()).rev() {
        let j = reference.next_int_bound(n as i32).unwrap() as usize;
        dirs.swap(n - 1, j);
    }
    for direction in dirs {
        let mut choices = vec![Face::Up, Face::North, Face::East, Face::South, Face::West];
        choices.retain(|face| *face != direction.opposite());
        for n in (2..=choices.len()).rev() {
            let _ = reference.next_int_bound(n as i32).unwrap();
        }
    }
    assert_eq!(random.next_long(), reference.next_long());
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn multiface_growth_matches_java() {
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
    let source = temp.path().join("MultifaceOracle.java");
    std::fs::write(&source, include_str!("java/MultifaceOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let actual = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("MultifaceOracle"));
    let effects: Vec<_> = actual
        .lines()
        .filter(|line| line.starts_with("E "))
        .collect();
    assert_eq!(effects.len(), 3);
    assert!(
        effects
            .iter()
            .all(|line| line.ends_with("false false null")),
        "{effects:?}"
    );
    let support: Vec<_> = actual
        .lines()
        .filter(|line| line.starts_with("S "))
        .collect();
    assert_eq!(support.len(), 84);
    assert!(
        support.iter().all(|line| line.ends_with(" true")),
        "{support:?}"
    );
    let support: Vec<_> = actual
        .lines()
        .filter(|line| line.starts_with("N "))
        .collect();
    assert_eq!(support.len(), 24);
    assert!(
        support.iter().all(|line| line.ends_with(" false")),
        "{support:?}"
    );
    let mut expected = Vec::new();
    for mode in 0..6 {
        for config in 0..4 {
            for seed in [0, 1, -1, 123456789] {
                let config_json = serde_json::json!({"type":"minecraft:multiface_growth","block":"minecraft:glow_lichen","search_range":if config==3{3}else{20},"can_place_on_floor":config==2,"can_place_on_ceiling":config!=1,"can_place_on_wall":true,"chance_of_spreading":if config==1{0.0}else if config==2{1.0}else{0.5},"can_be_placed_on":["minecraft:stone"]});
                let feature = MultifaceGrowth::decode(&config_json).unwrap();
                let mut w = World {
                    mode,
                    blocks: BTreeMap::new(),
                    writes: vec![],
                    marks: vec![],
                };
                let mut random = DecorationRandom::new(seed);
                let ok = feature.place(&mut w, &mut random, [0, 20, 0]).unwrap();
                expected.push(format!(
                    "C {mode} {config} {seed} {ok} W {} M {} R {}",
                    w.writes.join(";"),
                    w.marks.join(";"),
                    random.next_long()
                ));
            }
        }
    }
    let actual: Vec<_> = actual.lines().filter(|l| l.starts_with("C ")).collect();
    assert_eq!(actual.len(), expected.len());
    for (i, (a, b)) in actual.iter().zip(&expected).enumerate() {
        assert_eq!(*a, b, "case {i}");
    }
}
