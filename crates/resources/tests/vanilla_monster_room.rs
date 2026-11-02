use anyhow::Result;
use native_core::random::DecorationRandom;
use native_resources::{
    monster_room::{MonsterRoomWorld, RoomBlock, RoomMob, place_monster_room},
    structure_start::HorizontalDirection,
};
use std::{collections::BTreeMap, path::PathBuf, process::Command};

#[derive(Clone, Copy)]
enum State {
    Air,
    Stone,
    Bedrock,
    Placed(RoomBlock),
}
enum Entity {
    Chest(Option<i64>),
    Spawner(Option<RoomMob>),
}
struct World {
    mode: i32,
    rx: i32,
    rz: i32,
    blocks: BTreeMap<[i32; 3], State>,
    entities: BTreeMap<[i32; 3], Entity>,
    writes: Vec<String>,
    k: [bool; 4],
}
fn facing(d: HorizontalDirection) -> &'static str {
    match d {
        HorizontalDirection::North => "north",
        HorizontalDirection::South => "south",
        HorizontalDirection::East => "east",
        HorizontalDirection::West => "west",
    }
}
impl World {
    fn original(&self, p: [i32; 3]) -> State {
        let kind = self.mode % 4;
        if p[0] == -self.rx - 1 && p[2] == 0 && (p[1] == 20 || p[1] == 21) {
            return State::Air;
        }
        if kind == 1 && p == [-self.rx - 1, 19, -self.rz - 1] {
            return State::Air;
        }
        if kind == 2
            && p[0].abs() == self.rx + 1
            && p[2].abs() <= self.rz
            && (p[1] == 20 || p[1] == 21)
        {
            return State::Air;
        }
        if kind == 3 && (p == [-self.rx - 1, 19, 1] || p == [1, 20, 1]) {
            return State::Bedrock;
        }
        State::Stone
    }
}
impl MonsterRoomWorld for World {
    type State = State;
    fn min_y(&self) -> i32 {
        0
    }
    fn block(&mut self, p: [i32; 3]) -> Result<State> {
        Ok(self
            .blocks
            .get(&p)
            .copied()
            .unwrap_or_else(|| self.original(p)))
    }
    fn is_solid(&mut self, s: &State) -> Result<bool> {
        Ok(match s {
            State::Stone
            | State::Bedrock
            | State::Placed(RoomBlock::Cobblestone | RoomBlock::MossyCobblestone) => true,
            State::Placed(RoomBlock::Chest(_)) => self.k[0],
            State::Placed(RoomBlock::Spawner) => self.k[2],
            _ => false,
        })
    }
    fn is_solid_render(&mut self, s: &State) -> Result<bool> {
        Ok(match s {
            State::Stone
            | State::Bedrock
            | State::Placed(RoomBlock::Cobblestone | RoomBlock::MossyCobblestone) => true,
            State::Placed(RoomBlock::Chest(_)) => self.k[1],
            State::Placed(RoomBlock::Spawner) => self.k[3],
            _ => false,
        })
    }
    fn is_air(&mut self, s: &State) -> Result<bool> {
        Ok(matches!(s, State::Air | State::Placed(RoomBlock::CaveAir)))
    }
    fn is_chest(&mut self, s: &State) -> Result<bool> {
        Ok(matches!(s, State::Placed(RoomBlock::Chest(_))))
    }
    fn is_spawner(&mut self, s: &State) -> Result<bool> {
        Ok(matches!(s, State::Placed(RoomBlock::Spawner)))
    }
    fn may_replace(&mut self, s: &State) -> Result<bool> {
        Ok(!matches!(s, State::Bedrock))
    }
    fn set_block(&mut self, p: [i32; 3], b: RoomBlock) -> Result<()> {
        self.blocks.insert(p, State::Placed(b));
        self.entities.remove(&p);
        if self.mode < 4 {
            match b {
                RoomBlock::Chest(_) => {
                    self.entities.insert(p, Entity::Chest(None));
                }
                RoomBlock::Spawner => {
                    self.entities.insert(p, Entity::Spawner(None));
                }
                _ => {}
            }
        }
        let name = match b {
            RoomBlock::CaveAir => "air".into(),
            RoomBlock::Cobblestone => "cobble".into(),
            RoomBlock::MossyCobblestone => "mossy".into(),
            RoomBlock::Chest(d) => format!("chest:{}", facing(d)),
            RoomBlock::Spawner => "spawner".into(),
        };
        self.writes
            .push(format!("{},{},{},{}", p[0], p[1], p[2], name));
        Ok(())
    }
    fn chest_entity_exists(&mut self, p: [i32; 3]) -> Result<bool> {
        Ok(matches!(self.entities.get(&p), Some(Entity::Chest(_))))
    }
    fn set_chest_loot(&mut self, p: [i32; 3], seed: i64) -> Result<()> {
        let Some(Entity::Chest(v)) = self.entities.get_mut(&p) else {
            unreachable!()
        };
        *v = Some(seed);
        Ok(())
    }
    fn spawner_entity_exists(&mut self, p: [i32; 3]) -> Result<bool> {
        Ok(matches!(self.entities.get(&p), Some(Entity::Spawner(_))))
    }
    fn set_spawner_mob(
        &mut self,
        p: [i32; 3],
        mob: RoomMob,
        _: &mut DecorationRandom,
    ) -> Result<()> {
        let Some(Entity::Spawner(v)) = self.entities.get_mut(&p) else {
            unreachable!()
        };
        *v = Some(mob);
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
fn missing_floor_stops_before_writes_and_after_two_radius_draws() {
    for seed in [0, 1, -1, 987654321] {
        let mut reference = DecorationRandom::new(seed);
        let rx = reference.next_int_bound(2).unwrap() + 2;
        let rz = reference.next_int_bound(2).unwrap() + 2;
        let mut world = World {
            mode: 1,
            rx,
            rz,
            blocks: BTreeMap::new(),
            entities: BTreeMap::new(),
            writes: vec![],
            k: [true, false, true, false],
        };
        let mut random = DecorationRandom::new(seed);
        assert!(!place_monster_room(&mut world, &mut random, [0, 20, 0]).unwrap());
        assert!(world.writes.is_empty() && world.entities.is_empty());
        assert_eq!(random.next_long(), reference.next_long());
    }
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn monster_room_matches_java() {
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
    let source = temp.path().join("MonsterRoomOracle.java");
    std::fs::write(&source, include_str!("java/MonsterRoomOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let actual = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("MonsterRoomOracle"));
    let k = actual
        .lines()
        .find_map(|line| line.strip_prefix("K "))
        .unwrap()
        .split_whitespace()
        .map(|v| v.parse::<bool>().unwrap())
        .collect::<Vec<_>>();
    let k: [bool; 4] = k.try_into().unwrap();
    let mut expected = Vec::new();
    for seed in [0, 1, -1, 987654321] {
        for mode in 0..8 {
            let mut probe = DecorationRandom::new(seed);
            let rx = probe.next_int_bound(2).unwrap() + 2;
            let rz = probe.next_int_bound(2).unwrap() + 2;
            let mut world = World {
                mode,
                rx,
                rz,
                blocks: BTreeMap::new(),
                entities: BTreeMap::new(),
                writes: vec![],
                k,
            };
            let mut random = DecorationRandom::new(seed);
            let ok = place_monster_room(&mut world, &mut random, [0, 20, 0]).unwrap();
            let mut chests = Vec::new();
            let mut mob = "-";
            for (p, e) in &world.entities {
                match e {
                    Entity::Chest(Some(seed)) => {
                        chests.push(format!("{},{},{},{}", p[0], p[1], p[2], seed))
                    }
                    Entity::Spawner(Some(m)) => {
                        mob = match m {
                            RoomMob::Skeleton => "minecraft:skeleton",
                            RoomMob::Zombie => "minecraft:zombie",
                            RoomMob::Spider => "minecraft:spider",
                        }
                    }
                    _ => {}
                }
            }
            chests.sort();
            expected.push(format!(
                "C {seed} {mode} {ok} W {} T {} M {mob} R {}",
                world.writes.join(";"),
                chests.join(";"),
                random.next_long()
            ));
        }
    }
    let actual: Vec<_> = actual.lines().filter(|v| v.starts_with("C ")).collect();
    assert_eq!(actual.len(), 32);
    for (i, (a, b)) in actual.iter().zip(&expected).enumerate() {
        assert_eq!(*a, b, "case {i}");
    }
}
