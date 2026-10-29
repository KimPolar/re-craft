//! Compare stand-up search ordering with the installed Java 26.3 methods.

use std::{
    collections::BTreeMap,
    convert::Infallible,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use glam::{DVec3, IVec3};
use native_session::respawn::{
    DismountWorld, HorizontalDirection, anchor_stand_up_position, bed_stand_up_position,
    find_safe_dismount,
};
use native_session::{
    PlayerPose, PlayerState,
    loaded::{BuildBounds, LoadedSession},
};
use native_world::loaded::{BLOCK_COUNT, BlockState, Chunk, ChunkPos, Section, World};
use native_world::movement::BoundingBox;
use serde_json::{Value, json};
use std::sync::Arc;

fn classpath(game: &Path, classes: &Path) -> OsString {
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    let mut paths = vec![version.join("26.3.jar"), classes.to_path_buf()];
    for library in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    std::env::join_paths(paths).unwrap()
}

fn java_bin() -> PathBuf {
    if let Some(path) =
        std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
    {
        return PathBuf::from(path).join("bin");
    }
    PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon/bin")
}

fn run(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "respawn oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn saved_spawn_selection_matches_java() {
    use native_persistence::world::{DimensionId, PlayerRespawnConfig, RespawnData};
    use native_resources::{
        packs::ResourceProvider,
        respawn_environment::{BedCondition, RespawnEnvironment},
        respawn_tags::RespawnBlockTags,
    };
    use native_session::respawn::{SavedSpawnBlock, SavedSpawnWorld, resolve_saved_spawn};

    struct EmptyTags;
    impl ResourceProvider for EmptyTags {
        fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data_stack(&mut self, _: &str) -> anyhow::Result<Vec<Vec<u8>>> {
            Ok(vec![br#"{"values":[]}"#.to_vec()])
        }
    }

    fn loaded_spawn(
        case: &Value,
        tags: &RespawnBlockTags,
    ) -> Option<native_session::respawn::SavedSpawnResult> {
        let mut world = World::new("minecraft:overworld");
        let state = |name: &str, props: &[(&str, &str)]| BlockState {
            name: format!("minecraft:{name}"),
            properties: props
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
        };
        let air = world.intern_state(state("air", &[]));
        let stone = world.intern_state(state("stone", &[]));
        let water = world.intern_state(state("water", &[("level", "0")]));
        let facing = case["facing"].as_str().unwrap().to_ascii_lowercase();
        let kind = case["kind"].as_str().unwrap();
        let name = if kind == "straw" {
            "straw_bed"
        } else {
            "red_bed"
        };
        let bed = world.intern_state(state(
            name,
            &[("facing", &facing), ("part", "head"), ("occupied", "false")],
        ));
        let foot = world.intern_state(state(
            name,
            &[("facing", &facing), ("part", "foot"), ("occupied", "false")],
        ));
        let charge = case["charge"].to_string();
        let anchor = world.intern_state(state("respawn_anchor", &[("charges", &charge)]));
        let mut lower = Box::new([air; BLOCK_COUNT]);
        let mut upper = Box::new([air; BLOCK_COUNT]);
        for x in 4..=12 {
            for z in 4..=12 {
                lower[x + z * 16 + 15 * 256] = stone;
                if case["blocked"].as_bool().unwrap() {
                    for y in 0..=3 {
                        upper[x + z * 16 + y * 256] = stone;
                    }
                }
            }
        }
        let at = 8 + 8 * 16;
        upper[at] = match kind {
            "bed" | "straw" => bed,
            "anchor" => anchor,
            "stone" => stone,
            "water" => water,
            _ => air,
        };
        if kind == "bed" || kind == "straw" {
            let (x, z) = match facing.as_str() {
                "north" => (8, 9),
                "east" => (7, 8),
                "south" => (8, 7),
                _ => (9, 8),
            };
            upper[x + z * 16] = foot;
        }
        if case["above_stone"].as_bool().unwrap() {
            upper[at + 256] = stone;
        }
        let section = |blocks| {
            Arc::new(Section {
                blocks: Some(blocks),
                biomes: None,
                block_light: None,
                sky_light: None,
            })
        };
        let chunk = ChunkPos { x: 0, z: 0 };
        world.chunks.insert(
            chunk,
            Arc::new(Chunk {
                position: chunk,
                status: Some("minecraft:full".into()),
                sections: BTreeMap::from([(3, section(lower)), (4, section(upper))]),
            }),
        );
        let player = PlayerState {
            position: glam::Vec3::new(8.5, 64.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: PlayerPose::Standing,
        };
        let session = LoadedSession::new(
            world,
            IVec3::ZERO,
            BuildBounds {
                min_y: 0,
                max_y: 128,
            },
            player,
        )
        .unwrap();
        let config = PlayerRespawnConfig {
            data: RespawnData {
                dimension: DimensionId::parse("minecraft:overworld").unwrap(),
                position: [8, 64, 8],
                yaw: case["yaw"].as_f64().unwrap() as f32,
                pitch: case["pitch"].as_f64().unwrap() as f32,
            },
            forced: case["forced"].as_bool().unwrap(),
        };
        let mut environment = RespawnEnvironment {
            anchor_works: case["anchor_works"].as_bool().unwrap(),
            ..Default::default()
        };
        let condition = match case["bed_rule"].as_str().unwrap() {
            "ALWAYS" => BedCondition::Always,
            "NEVER" => BedCondition::Never,
            _ => BedCondition::WhenDark,
        };
        environment.bed.can_set_spawn = condition;
        environment.straw_bed.can_set_spawn = condition;
        let border = native_session::respawn::RespawnBorder {
            min_x: -29_999_984.0,
            max_x: 29_999_984.0,
            min_z: -29_999_984.0,
            max_z: 29_999_984.0,
        };
        session
            .resolve_saved_spawn(
                &config,
                environment,
                case["dark"].as_bool().unwrap(),
                tags,
                border,
                &mut |state| match state.name.as_str() {
                    "minecraft:air" => Ok(true),
                    "minecraft:stone"
                    | "minecraft:water"
                    | "minecraft:red_bed"
                    | "minecraft:straw_bed"
                    | "minecraft:respawn_anchor" => Ok(false),
                    name => anyhow::bail!("unsupported fixture state {name}"),
                },
            )
            .unwrap()
    }

    struct Fixture<'a> {
        case: &'a Value,
    }
    impl SavedSpawnWorld for Fixture<'_> {
        type Error = Infallible;
        fn spawn_block(&mut self, _: IVec3) -> Result<SavedSpawnBlock, Self::Error> {
            Ok(match self.case["kind"].as_str().unwrap() {
                "bed" | "straw" => SavedSpawnBlock::Bed {
                    straw: self.case["kind"] == "straw",
                },
                "anchor" => SavedSpawnBlock::Anchor {
                    charges: self.case["charge"].as_u64().unwrap() as u8,
                },
                _ => SavedSpawnBlock::Other,
            })
        }
        fn bed_can_set_spawn(&mut self, _: IVec3, _: bool) -> Result<bool, Self::Error> {
            Ok(match self.case["bed_rule"].as_str().unwrap() {
                "ALWAYS" => true,
                "NEVER" => false,
                _ => self.case["dark"].as_bool().unwrap(),
            })
        }
        fn anchor_works(&mut self, _: IVec3) -> Result<bool, Self::Error> {
            Ok(self.case["anchor_works"].as_bool().unwrap())
        }
        fn bed_stand_up(&mut self, pos: IVec3, yaw: f32) -> Result<Option<DVec3>, Self::Error> {
            let facing = match self.case["facing"].as_str().unwrap() {
                "NORTH" => HorizontalDirection::North,
                "EAST" => HorizontalDirection::East,
                "SOUTH" => HorizontalDirection::South,
                _ => HorizontalDirection::West,
            };
            let blocked = self.case["blocked"].as_bool().unwrap();
            bed_stand_up_position(pos, facing, yaw, false, |at, _| {
                Ok((!blocked).then(|| at.as_dvec3() + DVec3::new(0.5, 0.0, 0.5)))
            })
        }
        fn anchor_stand_up(&mut self, pos: IVec3) -> Result<Option<DVec3>, Self::Error> {
            let blocked = self.case["blocked"].as_bool().unwrap();
            anchor_stand_up_position(pos, |at, _| {
                Ok((!blocked).then(|| at.as_dvec3() + DVec3::new(0.5, 0.0, 0.5)))
            })
        }
        fn possible_to_respawn_in(&mut self, pos: IVec3) -> Result<bool, Self::Error> {
            Ok(if pos.y == 64 {
                self.case["kind"] == "air"
            } else {
                !self.case["above_stone"].as_bool().unwrap()
                    && !self.case["blocked"].as_bool().unwrap()
            })
        }
    }

    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let mut cases = Vec::new();
    let mut index = 0usize;
    for kind in ["bed", "straw", "anchor", "air", "stone", "water"] {
        for forced in [false, true] {
            for consume in [false, true] {
                for works in [false, true] {
                    for rule in ["ALWAYS", "WHEN_DARK", "NEVER"] {
                        for blocked in [false, true] {
                            for variation in 0..5 {
                                let facing = ["NORTH", "EAST", "SOUTH", "WEST"][variation % 4];
                                let yaw = [-179.9_f32, -90.0, -0.01, 0.0, 45.0, 90.0, 179.9]
                                    [(index + variation) % 7];
                                let pitch = [-45.0_f32, 0.0, 25.0, 90.0][variation % 4];
                                cases.push(json!({"kind":kind,"forced":forced,"consume":consume,
                                    "anchor_works":works,"bed_rule":rule,"blocked":blocked,
                                    "dark":variation % 2 == 0,"charge":variation,
                                    "above_stone":variation % 3 == 0,"facing":facing,"yaw":yaw,
                                    "pitch": pitch}));
                                index += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("SavedSpawnOracle.java");
    let input = temp.path().join("cases.json");
    let output = temp.path().join("output.json");
    std::fs::write(&source, include_str!("java/SavedSpawnOracle.java")).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java_bin().join("javac"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java_bin().join("java"))
        .arg("-cp")
        .arg(&cp)
        .arg("SavedSpawnOracle")
        .arg(&input)
        .arg(&output));
    let expected: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(expected.len(), cases.len());
    let registry = [
        "air",
        "stone",
        "water",
        "red_bed",
        "straw_bed",
        "respawn_anchor",
        "grass_block",
        "dirt",
        "stone_bricks",
    ]
    .into_iter()
    .map(|name| format!("minecraft:{name}"))
    .collect();
    let tags = RespawnBlockTags::load(&mut EmptyTags, registry).unwrap();
    for (case, expected) in cases.iter().zip(expected) {
        let pos = IVec3::new(8, 64, 8);
        let actual = resolve_saved_spawn(
            &mut Fixture { case },
            pos,
            case["yaw"].as_f64().unwrap() as f32,
            case["pitch"].as_f64().unwrap() as f32,
            case["forced"].as_bool().unwrap(),
            case["consume"].as_bool().unwrap(),
        )
        .unwrap();
        if case["consume"].as_bool().unwrap() {
            assert_eq!(loaded_spawn(case, &tags), actual, "loaded chunk {case}");
        }
        match actual {
            None => {
                assert!(expected["position"].is_null(), "{case} => {expected}");
                if case["kind"] == "anchor" {
                    assert_eq!(case["charge"], expected["charge"], "{case}");
                }
            }
            Some(actual) => {
                let p = expected["position"].as_array().unwrap();
                assert_eq!(
                    [actual.position.x, actual.position.y, actual.position.z],
                    [
                        p[0].as_f64().unwrap(),
                        p[1].as_f64().unwrap(),
                        p[2].as_f64().unwrap()
                    ],
                    "{case}"
                );
                assert_eq!(
                    actual.yaw.to_bits(),
                    (expected["yaw"].as_f64().unwrap() as f32).to_bits(),
                    "yaw {case}"
                );
                assert_eq!(
                    actual.pitch.to_bits(),
                    (expected["pitch"].as_f64().unwrap() as f32).to_bits(),
                    "pitch {case}"
                );
                if case["kind"] == "anchor" {
                    let initial = case["charge"].as_u64().unwrap() as u8;
                    assert_eq!(
                        actual.anchor_charge.unwrap_or(initial),
                        expected["charge"].as_u64().unwrap() as u8,
                        "{case}"
                    );
                }
            }
        }
    }
    eprintln!("compared {} saved spawn selections", cases.len());
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn saved_spawn_look_angle_matches_java() {
    use native_session::respawn::{SavedSpawnBlock, SavedSpawnWorld, resolve_saved_spawn};
    struct World(DVec3);
    impl SavedSpawnWorld for World {
        type Error = Infallible;
        fn spawn_block(&mut self, _: IVec3) -> Result<SavedSpawnBlock, Self::Error> {
            Ok(SavedSpawnBlock::Bed { straw: false })
        }
        fn bed_can_set_spawn(&mut self, _: IVec3, _: bool) -> Result<bool, Self::Error> {
            Ok(true)
        }
        fn anchor_works(&mut self, _: IVec3) -> Result<bool, Self::Error> {
            unreachable!()
        }
        fn bed_stand_up(&mut self, _: IVec3, _: f32) -> Result<Option<DVec3>, Self::Error> {
            Ok(Some(self.0))
        }
        fn anchor_stand_up(&mut self, _: IVec3) -> Result<Option<DVec3>, Self::Error> {
            unreachable!()
        }
        fn possible_to_respawn_in(&mut self, _: IVec3) -> Result<bool, Self::Error> {
            unreachable!()
        }
    }
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let mut cases = Vec::new();
    for base in [0, -16_000_000, 29_999_900] {
        for x in -4..=4 {
            for z in -4..=4 {
                for height in [0.0, 0.125, 0.5625, 1.0] {
                    for offset in [0.0, 0.125, 0.5] {
                        cases.push(json!([
                            base,
                            64,
                            -base,
                            f64::from(base) + 0.5 + f64::from(x) + offset,
                            64.0 + height,
                            f64::from(-base) + 0.5 + f64::from(z) - offset
                        ]));
                    }
                }
            }
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("SavedSpawnYawOracle.java");
    let input = temp.path().join("cases.json");
    let output = temp.path().join("output.json");
    std::fs::write(&source, include_str!("java/SavedSpawnYawOracle.java")).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java_bin().join("javac"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java_bin().join("java"))
        .arg("-cp")
        .arg(&cp)
        .arg("SavedSpawnYawOracle")
        .arg(&input)
        .arg(&output));
    let expected: Vec<f32> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(expected.len(), cases.len());
    for (case, expected) in cases.iter().zip(expected) {
        let a = case.as_array().unwrap();
        let pos = IVec3::new(
            a[0].as_i64().unwrap() as i32,
            a[1].as_i64().unwrap() as i32,
            a[2].as_i64().unwrap() as i32,
        );
        let stand = DVec3::new(
            a[3].as_f64().unwrap(),
            a[4].as_f64().unwrap(),
            a[5].as_f64().unwrap(),
        );
        let actual = resolve_saved_spawn(&mut World(stand), pos, 0.0, 0.0, false, true)
            .unwrap()
            .unwrap();
        assert_eq!(actual.yaw.to_bits(), expected.to_bits(), "{case}");
    }
    eprintln!("compared {} saved spawn angles", cases.len());
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn bed_and_anchor_search_order_matches_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let mut cases = Vec::new();
    for facing in ["NORTH", "EAST", "SOUTH", "WEST"] {
        for yaw in [
            -180.0_f32, -135.0, -90.0, -45.0, -0.01, 0.0, 0.01, 45.0, 89.999, 90.0, 135.0, 179.999,
        ] {
            for bunk in [false, true] {
                for success in [-1, 0, 1, 9, 10, 11, 12, 19, 21, 23, 24, 35, 43, 49] {
                    cases.push(
                        json!({"bed":true,"facing":facing,"yaw":yaw,"bunk":bunk,"success":success}),
                    );
                }
            }
        }
    }
    for success in -1..52 {
        cases.push(json!({"bed":false,"facing":"NORTH","yaw":0.0,"bunk":false,"success":success}));
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("RespawnSearchOracle.java");
    let input = temp.path().join("cases.json");
    let output = temp.path().join("output.json");
    std::fs::write(&source, include_str!("java/RespawnSearchOracle.java")).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java_bin().join("javac"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java_bin().join("java"))
        .arg("-cp")
        .arg(&cp)
        .arg("RespawnSearchOracle")
        .arg(&input)
        .arg(&output));
    let expected: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(expected.len(), cases.len());
    for (case, expected) in cases.iter().zip(expected) {
        let success = case["success"].as_i64().unwrap();
        let mut trace = Vec::new();
        let mut find = |pos: IVec3, avoid: bool| -> Result<Option<DVec3>, Infallible> {
            let index = trace.len() as i64;
            trace.push(json!([pos.x, pos.y, pos.z, avoid]));
            Ok((index == success).then(|| pos.as_dvec3() + DVec3::new(0.5, 0.125, 0.5)))
        };
        let pos = IVec3::new(32, 75, -48);
        let actual = if case["bed"].as_bool().unwrap() {
            let facing = match case["facing"].as_str().unwrap() {
                "NORTH" => HorizontalDirection::North,
                "EAST" => HorizontalDirection::East,
                "SOUTH" => HorizontalDirection::South,
                "WEST" => HorizontalDirection::West,
                _ => unreachable!(),
            };
            bed_stand_up_position(
                pos,
                facing,
                case["yaw"].as_f64().unwrap() as f32,
                case["bunk"].as_bool().unwrap(),
                &mut find,
            )
            .unwrap()
        } else {
            anchor_stand_up_position(pos, &mut find).unwrap()
        };
        let position = actual.map(|p| [p.x, p.y, p.z]);
        assert_eq!(
            json!({"trace":trace,"position":position}),
            expected,
            "{case}"
        );
    }
}

#[derive(Clone, Copy)]
struct Cell {
    dangerous: bool,
    invalid: bool,
    top: Option<f64>,
}

struct DismountFixture {
    current: Cell,
    below: Cell,
    above: Cell,
    collision: bool,
    outside: bool,
    bounds: Option<BoundingBox>,
}

impl DismountFixture {
    fn cell(&self, pos: IVec3) -> Cell {
        match pos.y {
            63 => self.below,
            64 => self.current,
            65 => self.above,
            _ => unreachable!("unexpected cell {pos:?}"),
        }
    }
}

impl DismountWorld for DismountFixture {
    type Error = Infallible;

    fn is_dangerous(&mut self, pos: IVec3) -> Result<bool, Self::Error> {
        Ok(self.cell(pos).dangerous)
    }

    fn non_climbable_top(&mut self, pos: IVec3) -> Result<Option<f64>, Self::Error> {
        Ok(self.cell(pos).top)
    }

    fn has_block_collision(&mut self, bounds: BoundingBox) -> Result<bool, Self::Error> {
        self.bounds = Some(bounds);
        Ok(self.collision)
    }

    fn invalid_spawn_inside(&mut self, pos: IVec3) -> Result<bool, Self::Error> {
        Ok(self.cell(pos).invalid)
    }

    fn within_world_border(&mut self, _bounds: BoundingBox) -> Result<bool, Self::Error> {
        Ok(!self.outside)
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn safe_dismount_checks_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let names = [
        "air",
        "stone",
        "slab_bottom",
        "slab_top",
        "oak_fence",
        "fire",
        "water",
        "magma_block",
        "cactus",
        "ladder",
        "trapdoor_open",
        "trapdoor_closed",
        "red_bed",
    ];
    let mut cases = Vec::new();
    for current in names {
        for below in names {
            for above in ["air", "stone", "fire"] {
                for avoid in [false, true] {
                    for (collision, outside) in [(false, false), (true, false), (false, true)] {
                        cases.push(json!({"current":current,"below":below,"above":above,
                            "avoid":avoid,"collision":collision,"outside":outside}));
                    }
                }
            }
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("RespawnDismountOracle.java");
    let input = temp.path().join("cases.json");
    let output = temp.path().join("output.json");
    std::fs::write(&source, include_str!("java/RespawnDismountOracle.java")).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java_bin().join("javac"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java_bin().join("java"))
        .arg("-cp")
        .arg(&cp)
        .arg("RespawnDismountOracle")
        .arg(&input)
        .arg(&output));
    let expected: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(expected.len(), cases.len());
    let cell = |value: &Value| Cell {
        dangerous: value["danger"].as_bool().unwrap(),
        invalid: value["invalid"].as_bool().unwrap(),
        top: value["top"].as_f64(),
    };
    for (case, expected) in cases.iter().zip(expected) {
        let mut world = DismountFixture {
            current: cell(&expected["current"]),
            below: cell(&expected["below"]),
            above: cell(&expected["above"]),
            collision: case["collision"].as_bool().unwrap(),
            outside: case["outside"].as_bool().unwrap(),
            bounds: None,
        };
        let position = find_safe_dismount(
            &mut world,
            IVec3::new(0, 64, 0),
            case["avoid"].as_bool().unwrap(),
        )
        .unwrap()
        .map(|p| [p.x, p.y, p.z]);
        let bounds = world
            .bounds
            .map(|b| [b.min.x, b.min.y, b.min.z, b.max.x, b.max.y, b.max.z]);
        assert_eq!(json!(position), expected["position"], "{case}");
        assert_eq!(json!(bounds), expected["bounds"], "{case}");
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn loaded_chunk_bed_and_anchor_search_matches_java_shapes() {
    use native_resources::{packs::ResourceProvider, respawn_tags::RespawnBlockTags};
    struct Tags;
    impl ResourceProvider for Tags {
        fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data_stack(&mut self, _: &str) -> anyhow::Result<Vec<Vec<u8>>> {
            Ok(vec![br#"{"values":[]}"#.to_vec()])
        }
    }
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let registry = [
        "air",
        "stone",
        "stone_slab",
        "water",
        "lava",
        "red_bed",
        "respawn_anchor",
        "grass_block",
        "dirt",
        "stone_bricks",
    ]
    .into_iter()
    .map(|name| format!("minecraft:{name}"))
    .collect();
    let tags = RespawnBlockTags::load(&mut Tags, registry).unwrap();
    let facings = ["NORTH", "EAST", "SOUTH", "WEST"];
    let mut cases = Vec::new();
    let mut random = 0x6319_26abu32;
    let mut next = || {
        random = random.wrapping_mul(1664525).wrapping_add(1013904223);
        random
    };
    for i in 0..256 {
        let facing = facings[(i / 4) % 4];
        let step = match facing {
            "NORTH" => (0, -1),
            "EAST" => (1, 0),
            "SOUTH" => (0, 1),
            _ => (-1, 0),
        };
        let bed = i % 4 != 0;
        let bunk = i % 3 == 0;
        let yaw = [-179.9_f32, -90.0, -0.01, 0.0, 45.0, 90.0, 179.9][(i / 8) % 7];
        let mut cells = BTreeMap::new();
        for x in 4..=12 {
            for z in 4..=12 {
                let floor = match next() % 10 {
                    0 => 0,
                    1 => 2,
                    2 => 3,
                    _ => 1,
                };
                cells.insert((x, 63, z), floor);
                let bottom = match next() % 24 {
                    0..=2 => 1,
                    3 => 2,
                    4 => 3,
                    5 => 5,
                    6 => 4,
                    _ => 0,
                };
                let top = if next().is_multiple_of(14) { 1 } else { 0 };
                cells.insert((x, 64, z), bottom);
                cells.insert((x, 65, z), top);
            }
        }
        if bed {
            cells.insert((8, 64, 8), 6);
            cells.insert((8 - step.0, 64, 8 - step.1), 7);
            if bunk {
                cells.insert((8, 63, 8), 6);
            }
        } else {
            cells.insert((8, 64, 8), 8);
        }
        let cells: Vec<_> = cells
            .into_iter()
            .map(|((x, y, z), code)| [x, y, z, code])
            .collect();
        cases.push(json!({"facing":facing,"yaw":yaw,"bed":bed,"cells":cells}));
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("RespawnWorldOracle.java");
    let input = temp.path().join("worlds.json");
    let output = temp.path().join("results.json");
    std::fs::write(&source, include_str!("java/RespawnWorldOracle.java")).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java_bin().join("javac"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java_bin().join("java"))
        .arg("-cp")
        .arg(&cp)
        .arg("RespawnWorldOracle")
        .arg(&input)
        .arg(&output));
    let expected: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(expected.len(), cases.len());
    let border = native_session::respawn::RespawnBorder {
        min_x: -29_999_984.0,
        max_x: 29_999_984.0,
        min_z: -29_999_984.0,
        max_z: 29_999_984.0,
    };
    for (index, (case, expected)) in cases.iter().zip(expected).enumerate() {
        let mut world = World::new("minecraft:overworld");
        let facing = case["facing"].as_str().unwrap().to_ascii_lowercase();
        let state = |name: &str, props: &[(&str, &str)]| BlockState {
            name: format!("minecraft:{name}"),
            properties: props
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
        };
        let states = [
            state("air", &[]),
            state("stone", &[]),
            state(
                "stone_slab",
                &[("type", "bottom"), ("waterlogged", "false")],
            ),
            state("stone_slab", &[("type", "top"), ("waterlogged", "false")]),
            state("water", &[("level", "0")]),
            state("lava", &[("level", "0")]),
            state(
                "red_bed",
                &[("facing", &facing), ("part", "head"), ("occupied", "false")],
            ),
            state(
                "red_bed",
                &[("facing", &facing), ("part", "foot"), ("occupied", "false")],
            ),
            state("respawn_anchor", &[("charges", "0")]),
        ]
        .map(|s| world.intern_state(s));
        let mut bottom = Box::new([states[0]; BLOCK_COUNT]);
        let mut top = Box::new([states[0]; BLOCK_COUNT]);
        for cell in case["cells"].as_array().unwrap() {
            let c = cell.as_array().unwrap();
            let x = c[0].as_u64().unwrap() as usize;
            let y = c[1].as_u64().unwrap() as usize;
            let z = c[2].as_u64().unwrap() as usize;
            let id = states[c[3].as_u64().unwrap() as usize];
            let section = if y < 64 { &mut bottom } else { &mut top };
            section[x + z * 16 + (y % 16) * 256] = id;
        }
        let section = |blocks| {
            Arc::new(Section {
                blocks: Some(blocks),
                biomes: None,
                block_light: None,
                sky_light: None,
            })
        };
        let pos = ChunkPos { x: 0, z: 0 };
        world.chunks.insert(
            pos,
            Arc::new(Chunk {
                position: pos,
                status: Some("minecraft:full".into()),
                sections: BTreeMap::from([(3, section(bottom)), (4, section(top))]),
            }),
        );
        let player = PlayerState {
            position: glam::Vec3::new(8.5, 64.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: PlayerPose::Standing,
        };
        let session = LoadedSession::new(
            world,
            IVec3::ZERO,
            BuildBounds {
                min_y: 0,
                max_y: 128,
            },
            player,
        )
        .unwrap();
        let pos = IVec3::new(8, 64, 8);
        let found = if case["bed"].as_bool().unwrap() {
            session.bed_stand_up_position(pos, case["yaw"].as_f64().unwrap() as f32, &tags, border)
        } else {
            session.anchor_stand_up_position(pos, &tags, border)
        }
        .unwrap();
        let found = found.map(|p| [p.x, p.y, p.z]);
        assert_eq!(
            json!(found),
            expected,
            "synthetic world {index} {}",
            case["facing"]
        );
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn saved_world_border_bounds_and_motion_match_java() {
    use native_persistence::world::SavedWorldBorder;
    use native_session::respawn::RespawnBorder;
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let mut cases = Vec::new();
    let offsets = [
        -1.0, -0.300_01, -0.3, -0.299_99, 0.0, 0.299_99, 0.3, 0.300_01,
    ];
    for (index, x) in [0.0, 12.25, -29_999_983.5, 29_999_983.5]
        .into_iter()
        .enumerate()
    {
        for size in [1.0, 10.0, 100.0, 59_999_968.0] {
            for target in [0.5, 64.0, 59_999_968.0] {
                for duration in [0, 1, 5, 10] {
                    for ticks in [0, 1, 2, 5, 10, 11] {
                        for delta in offsets {
                            let z = -x / 2.0;
                            let center = x + size / 2.0 + delta;
                            cases.push(json!({"x":x,"z":z,"size":size,"target":target,
                                "duration":duration,"ticks":ticks,
                                "box":[center-0.3,64.0,z-0.3,center+0.3,65.8,z+0.3],"index":index}));
                        }
                    }
                }
            }
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("RespawnBorderOracle.java");
    let input = temp.path().join("cases.json");
    let output = temp.path().join("output.json");
    std::fs::write(&source, include_str!("java/RespawnBorderOracle.java")).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java_bin().join("javac"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java_bin().join("java"))
        .arg("-cp")
        .arg(&cp)
        .arg("RespawnBorderOracle")
        .arg(&input)
        .arg(&output));
    let expected: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(expected.len(), cases.len());
    for (case, expected) in cases.iter().zip(expected) {
        let value = |key: &str| case[key].as_f64().unwrap();
        let settings = SavedWorldBorder {
            center_x: value("x"),
            center_z: value("z"),
            size: value("size"),
            lerp_target: value("target"),
            lerp_time: case["duration"].as_i64().unwrap(),
            ..SavedWorldBorder::default()
        };
        let border = RespawnBorder::from_saved(settings, case["ticks"].as_u64().unwrap()).unwrap();
        let values = case["box"].as_array().unwrap();
        let n = |i: usize| values[i].as_f64().unwrap();
        let bounds = BoundingBox {
            min: DVec3::new(n(0), n(1), n(2)),
            max: DVec3::new(n(3), n(4), n(5)),
        };
        assert_eq!(
            json!([
                border.min_x,
                border.max_x,
                border.min_z,
                border.max_z,
                border.contains(bounds)
            ]),
            expected,
            "{case}"
        );
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn world_spawn_border_adjustment_matches_java() {
    use native_persistence::world::SavedWorldBorder;
    use native_session::{respawn::RespawnBorder, world_spawn::border_adjusted_column};
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let mut cases = Vec::new();
    for cx in [
        -29_999_983.5_f64,
        -16_000_000.25,
        -0.75,
        12.25,
        29_999_983.5,
    ] {
        let cz = -cx / 2.0;
        for size in [1.0_f64, 10.0, 100.0, 59_999_968.0] {
            for target in [0.5, 64.0, 59_999_968.0] {
                for duration in [0, 1, 5, 10] {
                    for ticks in [0, 1, 2, 5, 10, 11] {
                        for delta in [-2.0, -0.01, 0.0, 1.0] {
                            let x = (cx + size / 2.0 + delta).floor() as i32;
                            let z = (cz - size / 2.0 - delta).floor() as i32;
                            cases.push(json!({"cx":cx,"cz":cz,"size":size,"target":target,
                                "duration":duration,"ticks":ticks,"x":x,"y":42,"z":z,"height":87}));
                        }
                    }
                }
            }
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("WorldSpawnBorderOracle.java");
    let input = temp.path().join("cases.json");
    let output = temp.path().join("output.json");
    std::fs::write(&source, include_str!("java/WorldSpawnBorderOracle.java")).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java_bin().join("javac"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java_bin().join("java"))
        .arg("-cp")
        .arg(&cp)
        .arg("WorldSpawnBorderOracle")
        .arg(&input)
        .arg(&output));
    let expected: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(expected.len(), cases.len());
    for (case, expected) in cases.iter().zip(expected) {
        let value = |key: &str| case[key].as_f64().unwrap();
        let integer = |key: &str| case[key].as_i64().unwrap() as i32;
        let settings = SavedWorldBorder {
            center_x: value("cx"),
            center_z: value("cz"),
            size: value("size"),
            lerp_target: value("target"),
            lerp_time: case["duration"].as_i64().unwrap(),
            ..Default::default()
        };
        let border = RespawnBorder::from_saved(settings, case["ticks"].as_u64().unwrap()).unwrap();
        let spawn = IVec3::new(integer("x"), integer("y"), integer("z"));
        let (x, z, adjusted) =
            border_adjusted_column(spawn, border, settings.center_x, settings.center_z).unwrap();
        let y = if adjusted { integer("height") } else { spawn.y };
        let distance = (f64::from(x) - border.min_x)
            .min(border.max_x - f64::from(x))
            .min(f64::from(z) - border.min_z)
            .min(border.max_z - f64::from(z));
        assert_eq!(json!([x, y, z, adjusted, distance]), expected, "{case}");
    }
    eprintln!(
        "Compared {} world spawn border adjustments against Java 26.3",
        cases.len()
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn shared_spawn_height_and_candidate_parameters_match_java() {
    use native_session::world_spawn::{
        ResumableWorldSpawn, SharedSpawnWorld, SpawnColumnHeights, WorldSpawnCandidates,
        fixup_spawn_height,
    };
    use std::collections::BTreeSet;

    fn loaded_column(
        case: &Value,
        tags: Arc<native_resources::heightmap_tags::HeightmapTags>,
    ) -> Option<IVec3> {
        use native_persistence::{
            nbt::{NamedTag, Tag},
            world::LoadedChunk,
        };
        let mut world = World::new("minecraft:overworld");
        let state = |name: &str, properties: &[(&str, &str)]| BlockState {
            name: format!("minecraft:{name}"),
            properties: properties
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
        };
        let ids = [
            world.intern_state(state("air", &[])),
            world.intern_state(state("stone", &[])),
            world.intern_state(state(
                "stone_slab",
                &[("type", "bottom"), ("waterlogged", "false")],
            )),
            world.intern_state(state(
                "stone_slab",
                &[("type", "top"), ("waterlogged", "false")],
            )),
            world.intern_state(state("water", &[("level", "0")])),
        ];
        let pos = ChunkPos { x: 0, z: -1 };
        let mut sections = BTreeMap::new();
        for y in [-1_i8, 0] {
            sections.insert(
                y,
                Arc::new(Section {
                    blocks: Some(Box::new([ids[0]; BLOCK_COUNT])),
                    biomes: None,
                    block_light: None,
                    sky_light: None,
                }),
            );
        }
        for cell in case["cells"].as_array().unwrap() {
            let y = cell[0].as_i64().unwrap() as i32;
            let code = cell[1].as_u64().unwrap() as usize;
            let section = Arc::make_mut(sections.get_mut(&(y.div_euclid(16) as i8)).unwrap());
            section.blocks.as_mut().unwrap()[8 + 7 * 16 + y.rem_euclid(16) as usize * 256] =
                ids[code];
        }
        world.chunks.insert(
            pos,
            Arc::new(Chunk {
                position: pos,
                status: Some("minecraft:full".into()),
                sections,
            }),
        );
        let mut stored = Vec::new();
        for (name, key) in [
            ("MOTION_BLOCKING", "motion"),
            ("WORLD_SURFACE", "surface"),
            ("OCEAN_FLOOR", "ocean"),
        ] {
            let first = (case[key].as_i64().unwrap() as i32 + 17) as u64;
            let mut words = vec![0i64; 26];
            for column in 0..256 {
                words[column / 10] |= (first << (6 * (column % 10))) as i64;
            }
            stored.push((name.into(), Tag::LongArray(words)));
        }
        let saved = LoadedChunk {
            position: native_persistence::world::ChunkPos { x: 0, z: -1 },
            data_version: 5023,
            status: Some("minecraft:full".into()),
            document: Arc::new(NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![("Heightmaps".into(), Tag::Compound(stored))]),
            }),
            sections: Vec::new(),
        };
        let player = PlayerState {
            position: glam::Vec3::new(8.5, 0.0, -8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: PlayerPose::Standing,
        };
        let mut session = LoadedSession::new(
            world,
            IVec3::ZERO,
            BuildBounds {
                min_y: -16,
                max_y: 16,
            },
            player,
        )
        .unwrap();
        session.set_spawn_heightmap_tags(tags).unwrap();
        session.prepare_initial_spawn_heightmaps(&saved).unwrap();
        session.spawn_column_candidate(8, -9, None).unwrap()
    }

    struct ControlledWorld {
        min: i32,
        max: i32,
        blocked: BTreeSet<i32>,
        trace: Vec<i32>,
    }
    struct ColumnWorld {
        heights: SpawnColumnHeights,
        blocks: BTreeMap<i32, i32>,
    }
    impl SharedSpawnWorld for ColumnWorld {
        fn min_y(&self) -> i32 {
            -4
        }
        fn max_y(&self) -> i32 {
            8
        }
        fn ceiling_spawn_height(&self) -> Option<i32> {
            None
        }
        fn heights(&mut self, _: i32, _: i32) -> anyhow::Result<SpawnColumnHeights> {
            Ok(self.heights)
        }
        fn has_fluid(&mut self, pos: IVec3) -> anyhow::Result<bool> {
            Ok(self.blocks.get(&pos.y) == Some(&4))
        }
        fn full_top_face(&mut self, pos: IVec3) -> anyhow::Result<bool> {
            Ok(matches!(self.blocks.get(&pos.y), Some(1 | 3)))
        }
        fn clear_player(&mut self, _: IVec3) -> anyhow::Result<bool> {
            unreachable!()
        }
    }
    impl SharedSpawnWorld for ControlledWorld {
        fn min_y(&self) -> i32 {
            self.min
        }
        fn max_y(&self) -> i32 {
            self.max
        }
        fn ceiling_spawn_height(&self) -> Option<i32> {
            unreachable!()
        }
        fn heights(&mut self, _: i32, _: i32) -> anyhow::Result<SpawnColumnHeights> {
            unreachable!()
        }
        fn has_fluid(&mut self, _: IVec3) -> anyhow::Result<bool> {
            unreachable!()
        }
        fn full_top_face(&mut self, _: IVec3) -> anyhow::Result<bool> {
            unreachable!()
        }
        fn clear_player(&mut self, pos: IVec3) -> anyhow::Result<bool> {
            self.trace.push(pos.y);
            Ok(!self.blocked.contains(&pos.y))
        }
    }

    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
    let registry = resources.vanilla_block_ids().unwrap();
    let tags = Arc::new(
        native_resources::heightmap_tags::HeightmapTags::load(&mut resources, &registry).unwrap(),
    );
    let mut heights = Vec::new();
    for seed in 0..512_u32 {
        let mixed = seed.wrapping_mul(2_654_435_761).rotate_left(11);
        let blocked: Vec<_> = (-6..=10).filter(|y| mixed & (1 << (*y + 6)) != 0).collect();
        for y in [-6, -4, -1, 0, 3, 8, 10] {
            heights.push(json!({"x":-17,"y":y,"z":24,"min":-4,"max":8,"blocked":blocked}));
        }
    }
    let radii = [0, 1, 2, 3, 4, 5, 8, 10, 15, 16, 25, 100, 1000];
    let mut columns = Vec::new();
    for seed in 0..64_u32 {
        let mixed = seed.wrapping_mul(2_654_435_761).rotate_left(9);
        let cells: Vec<_> = (-4..=5)
            .filter_map(|y| {
                let code = ((mixed >> ((y + 4) * 3)) & 7) as i32;
                (1..=4).contains(&code).then(|| json!([y, code]))
            })
            .collect();
        for motion in [-5, -4, 0, 4] {
            for surface in [-4, 0, 4] {
                for ocean in [-4, 0, 4] {
                    columns.push(
                        json!({"motion":motion,"surface":surface,"ocean":ocean,"cells":cells}),
                    );
                }
            }
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("WorldSpawnOracle.java");
    let input = temp.path().join("cases.json");
    let output = temp.path().join("output.json");
    std::fs::write(&source, include_str!("java/WorldSpawnOracle.java")).unwrap();
    std::fs::write(
        &input,
        serde_json::to_vec(&json!({"heights":heights,"columns":columns,"radii":radii})).unwrap(),
    )
    .unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java_bin().join("javac"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java_bin().join("java"))
        .arg("-cp")
        .arg(&cp)
        .arg("WorldSpawnOracle")
        .arg(&input)
        .arg(&output));
    let expected: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    for (case, expected) in heights.iter().zip(expected["heights"].as_array().unwrap()) {
        let blocked: BTreeSet<i32> = case["blocked"]
            .as_array()
            .unwrap()
            .iter()
            .map(|y| y.as_i64().unwrap() as i32)
            .collect();
        let mut world = ControlledWorld {
            min: -4,
            max: 8,
            blocked: blocked.clone(),
            trace: Vec::new(),
        };
        let origin = IVec3::new(-17, case["y"].as_i64().unwrap() as i32, 24);
        let pos = fixup_spawn_height(&mut world, origin).unwrap();
        assert_eq!(
            json!([
                f64::from(pos.x) + 0.5,
                f64::from(pos.y),
                f64::from(pos.z) + 0.5
            ]),
            expected["pos"],
            "{case}"
        );
        assert_eq!(json!(world.trace), expected["trace"], "{case}");
        let mut resumed = ControlledWorld {
            min: -4,
            max: 8,
            blocked,
            trace: Vec::new(),
        };
        let mut search = ResumableWorldSpawn::new(origin, None).unwrap();
        let resumed_pos = loop {
            if let Some(pos) = search.poll(&mut resumed, 1).unwrap() {
                break pos;
            }
        };
        assert_eq!(resumed_pos, pos, "{case}");
        assert_eq!(json!(resumed.trace), expected["trace"], "{case}");
    }
    for (case, expected) in columns.iter().zip(expected["columns"].as_array().unwrap()) {
        let num = |key: &str| case[key].as_i64().unwrap() as i32;
        let blocks = case["cells"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                (
                    entry[0].as_i64().unwrap() as i32,
                    entry[1].as_i64().unwrap() as i32,
                )
            })
            .collect();
        let mut world = ColumnWorld {
            heights: SpawnColumnHeights {
                motion_blocking: num("motion"),
                world_surface: num("surface"),
                ocean_floor: num("ocean"),
            },
            blocks,
        };
        let found = native_session::world_spawn::column_spawn(&mut world, 8, -9).unwrap();
        assert_eq!(
            json!(found.map(|pos| [pos.x, pos.y, pos.z])),
            *expected,
            "{case}"
        );
        // The Java oracle uses min Y -4, while stored Java chunk sections must
        // start at -16. Below -4 the synthetic chunk is entirely air.
        if num("motion") >= -4 {
            let found = loaded_column(case, Arc::clone(&tags));
            assert_eq!(
                json!(found.map(|pos| [pos.x, pos.y, pos.z])),
                *expected,
                "loaded {case}"
            );
        }
    }
    for (radius, expected) in radii
        .into_iter()
        .zip(expected["choices"].as_array().unwrap())
    {
        let count = expected[0].as_i64().unwrap() as i32;
        let coprime = expected[1].as_i64().unwrap() as i32;
        let offset = expected[2].as_i64().unwrap() as i32;
        let mut candidates =
            WorldSpawnCandidates::new(IVec3::ZERO, radius, 1_000_000.0, offset).unwrap();
        assert_eq!(candidates.candidate_count(), count, "radius {radius}");
        let side = radius * 2 + 1;
        let index = |(x, z): (i32, i32)| (z + radius) * side + x + radius;
        assert_eq!(index(candidates.next().unwrap()), offset, "radius {radius}");
        if count > 1 {
            assert_eq!(
                index(candidates.next().unwrap()),
                (offset + coprime) % count,
                "radius {radius}"
            );
        }
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn loaded_world_spawn_block_and_liquid_collision_matches_java() {
    use native_resources::heightmap_tags::HeightmapTags;

    fn state(code: i32) -> BlockState {
        let (name, properties): (&str, &[(&str, &str)]) = match code {
            1 => ("stone", &[]),
            2 => (
                "stone_slab",
                &[("type", "bottom"), ("waterlogged", "false")],
            ),
            3 => ("stone_slab", &[("type", "top"), ("waterlogged", "false")]),
            4 => ("water", &[("level", "0")]),
            5 => ("water", &[("level", "7")]),
            6 => ("lava", &[("level", "15")]),
            7 => (
                "oak_fence",
                &[
                    ("north", "true"),
                    ("south", "true"),
                    ("east", "true"),
                    ("west", "true"),
                    ("waterlogged", "false"),
                ],
            ),
            8 => ("bubble_column", &[("drag", "true")]),
            9 => ("kelp", &[("age", "0")]),
            10 => ("oak_slab", &[("type", "bottom"), ("waterlogged", "true")]),
            _ => panic!("unexpected block code"),
        };
        BlockState {
            name: format!("minecraft:{name}"),
            properties: properties
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
        }
    }
    fn loaded(case: &Value, tags: Arc<HeightmapTags>) -> Value {
        let x = case["x"].as_i64().unwrap() as i32;
        let y = case["y"].as_i64().unwrap() as i32;
        let z = case["z"].as_i64().unwrap() as i32;
        let pos = ChunkPos {
            x: x.div_euclid(16),
            z: z.div_euclid(16),
        };
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let mut blocks = Box::new([air; BLOCK_COUNT]);
        for cell in case["cells"].as_array().unwrap() {
            let bx = cell[0].as_i64().unwrap() as i32;
            let by = cell[1].as_i64().unwrap() as i32;
            let bz = cell[2].as_i64().unwrap() as i32;
            let id = world.intern_state(state(cell[3].as_i64().unwrap() as i32));
            let index = bx.rem_euclid(16) as usize
                + bz.rem_euclid(16) as usize * 16
                + by.rem_euclid(16) as usize * 256;
            blocks[index] = id;
        }
        world.chunks.insert(
            pos,
            Arc::new(Chunk {
                position: pos,
                status: Some("minecraft:full".into()),
                sections: BTreeMap::from([(
                    0,
                    Arc::new(Section {
                        blocks: Some(blocks),
                        biomes: None,
                        block_light: None,
                        sky_light: None,
                    }),
                )]),
            }),
        );
        let origin = IVec3::new(pos.x * 16, 0, pos.z * 16);
        let player = PlayerState {
            position: glam::Vec3::new(8.5, 3.0, 8.5),
            yaw: 0.0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: PlayerPose::Standing,
        };
        let mut session = LoadedSession::new(
            world,
            origin,
            BuildBounds {
                min_y: -16,
                max_y: 32,
            },
            player,
        )
        .unwrap();
        session.set_spawn_heightmap_tags(tags).unwrap();
        let position = IVec3::new(x, y, z);
        if case["fixup"].as_bool() == Some(true) {
            session
                .set_initial_entity_chunk(pos, native_session::loaded::SavedEntityData::Empty)
                .unwrap();
            let result = session
                .find_loaded_world_spawn(position, None, None)
                .unwrap();
            json!([
                f64::from(result.x) + 0.5,
                f64::from(result.y),
                f64::from(result.z) + 0.5
            ])
        } else {
            json!(session.world_spawn_blocks_clear(position).unwrap())
        }
    }

    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
    let registry = resources.vanilla_block_ids().unwrap();
    let tags = Arc::new(HeightmapTags::load(&mut resources, &registry).unwrap());
    let mut cases = Vec::new();
    for (x, z) in [(8, 8), (16_000_008, -15_999_992)] {
        for code in 1..=10 {
            for dx in -2..=2 {
                for dy in -2..=3 {
                    for dz in -2..=2 {
                        cases.push(json!({"x":x,"y":3,"z":z,"cells":[[x+dx,3+dy,z+dz,code]]}));
                    }
                }
            }
        }
        for seed in 0..128 {
            let mut cells = vec![json!([x, 0, z, 1])];
            for dy in 1..=8 {
                for dx in -1..=1 {
                    for dz in -1..=1 {
                        let hash = (seed * 97 + dy * 53 + (dx + 1) * 23 + (dz + 1) * 11) % 31;
                        if hash < 4 {
                            cells.push(json!([x + dx, dy, z + dz, 1 + (seed + hash) % 10]));
                        }
                    }
                }
            }
            cases.push(json!({"x":x,"y":3,"z":z,"fixup":true,"cells":cells}));
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("WorldSpawnCollisionOracle.java");
    let input = temp.path().join("cases.json");
    let output = temp.path().join("output.json");
    std::fs::write(&source, include_str!("java/WorldSpawnCollisionOracle.java")).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java_bin().join("javac"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java_bin().join("java"))
        .arg("-cp")
        .arg(&cp)
        .arg("WorldSpawnCollisionOracle")
        .arg(&input)
        .arg(&output));
    let expected: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    for (case, expected) in cases.iter().zip(expected.as_array().unwrap()) {
        assert_eq!(loaded(case, Arc::clone(&tags)), *expected, "{case}");
    }
    println!(
        "Compared {} loaded-world spawn block, liquid and height-fixup cases against Java",
        cases.len()
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn saved_spawn_entities_match_java_noncolliding_types_and_defer_dynamic_types() {
    use native_persistence::{
        nbt::{NamedTag, Tag, TagType},
        world::SavedEntityChunk,
    };
    use native_session::loaded::SavedEntityData;
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
    let blocks = resources.vanilla_block_ids().unwrap();
    let tags = Arc::new(
        native_resources::heightmap_tags::HeightmapTags::load(&mut resources, &blocks).unwrap(),
    );
    let registry = Arc::new(resources.vanilla_entity_collision_registry().unwrap());
    let ids = ["pig", "item", "minecart", "oak_boat", "bamboo_raft"];
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("WorldSpawnEntityOracle.java");
    let input = temp.path().join("entities.json");
    let output = temp.path().join("results.json");
    std::fs::write(&source, include_str!("java/WorldSpawnEntityOracle.java")).unwrap();
    std::fs::write(&input, serde_json::to_vec(&ids).unwrap()).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java_bin().join("javac"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java_bin().join("java"))
        .arg("-cp")
        .arg(&cp)
        .arg("WorldSpawnEntityOracle")
        .arg(&input)
        .arg(&output));
    let expected: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    let mut world = World::new("minecraft:overworld");
    let air = world.intern_state(BlockState {
        name: "minecraft:air".into(),
        properties: BTreeMap::new(),
    });
    let home = ChunkPos { x: 0, z: 0 };
    world.chunks.insert(
        home,
        Arc::new(Chunk {
            position: home,
            status: Some("minecraft:full".into()),
            sections: BTreeMap::from([(
                0,
                Arc::new(Section {
                    blocks: Some(Box::new([air; BLOCK_COUNT])),
                    biomes: None,
                    block_light: None,
                    sky_light: None,
                }),
            )]),
        }),
    );
    let player = PlayerState {
        position: glam::Vec3::new(8.5, 1.0, 8.5),
        yaw: 0.0,
        pitch: 0.0,
        vertical_velocity: 0.0,
        on_ground: false,
        pose: PlayerPose::Standing,
    };
    let mut session = LoadedSession::new(
        world,
        IVec3::ZERO,
        BuildBounds {
            min_y: 0,
            max_y: 16,
        },
        player,
    )
    .unwrap();
    session.set_spawn_heightmap_tags(tags).unwrap();
    session.set_entity_collision_registry(registry).unwrap();
    for (id, expected) in ids.into_iter().zip(expected.as_array().unwrap()) {
        let entity = Tag::Compound(vec![(
            "id".into(),
            Tag::String(format!("minecraft:{id}").as_str().into()),
        )]);
        let document = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "Entities".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![entity],
                },
            )]),
        };
        session
            .set_initial_entity_chunk(
                home,
                SavedEntityData::Present(Arc::new(SavedEntityChunk {
                    position: native_persistence::world::ChunkPos { x: 0, z: 0 },
                    document: Arc::new(document),
                })),
            )
            .unwrap();
        let result = session.world_spawn_clear(IVec3::new(8, 1, 8));
        if expected.as_bool().unwrap() {
            assert!(result.unwrap(), "{id}");
        } else {
            assert!(result.is_err(), "{id} requires a live collision state");
        }
    }
}
