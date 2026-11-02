//! Simple vegetation placement against Java's real feature and ProtoChunk.
use super::*;
use native_resources::packs::ResourceProvider;
use native_world::loaded::{BLOCK_COUNT, Chunk, Section};
use serde_json::json;
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3"]
fn pale_garden_flower_is_excluded_from_plains() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
    let blocks = resources.vanilla_generated_block_registry().unwrap();
    let registry = resources
        .list_data("worldgen/placed_feature")
        .unwrap()
        .into_iter()
        .collect();
    let pale = ResourceId::parse("minecraft:pale_garden").unwrap();
    let plains = ResourceId::parse("minecraft:plains").unwrap();
    let schedule =
        FeatureSchedule::load(&mut resources, &[pale.clone(), plains.clone()], &registry).unwrap();
    let key = FeatureKey::Registered(ResourceId::parse("minecraft:flower_pale_garden").unwrap());
    let flower = ScheduledDecorationFeature::load(&mut resources, &schedule, &key, &mut |value| {
        super::super::resolve_generated_terrain_state(value, &blocks)
    })
    .unwrap();
    assert!(flower.biome_allows(&pale).unwrap());
    assert!(!flower.biome_allows(&plains).unwrap());
}

fn state(name: &str) -> BlockState {
    let properties = match name {
        "grass_block" => BTreeMap::from([("snowy".into(), "false".into())]),
        "water" => BTreeMap::from([("level".into(), "0".into())]),
        "sunflower" | "tall_grass" => BTreeMap::from([("half".into(), "lower".into())]),
        "sweet_berry_bush" => BTreeMap::from([("age".into(), "0".into())]),
        _ => BTreeMap::new(),
    };
    BlockState {
        name: format!("minecraft:{name}"),
        properties,
    }
}
fn resolve(value: &Value) -> Result<BlockState> {
    let name = value
        .as_str()
        .or_else(|| value.get("id").and_then(Value::as_str))
        .context("missing state ID")?;
    let mut result = state(
        name.strip_prefix("minecraft:")
            .context("invalid state namespace")?,
    );
    if let Some(props) = value.get("properties").and_then(Value::as_object) {
        for (key, value) in props {
            result.properties.insert(
                key.clone(),
                value.as_str().context("invalid property")?.into(),
            );
        }
    }
    Ok(result)
}
fn initial(x: i32, y: i32, z: i32) -> BlockState {
    let n = (x * 7 + z * 11).rem_euclid(8);
    state(if y < 0 {
        "stone"
    } else if y == 0 {
        [
            "dirt",
            "grass_block",
            "sand",
            "water",
            "stone",
            "sunflower",
            "dirt",
            "air",
        ][n as usize]
    } else if y == 1 {
        ["air", "short_grass", "poppy", "water", "stone"][(n % 5) as usize]
    } else if y == 2 {
        [
            "air",
            "short_grass",
            "poppy",
            "water",
            "stone",
            "tall_grass",
        ][((n + 2) % 6) as usize]
    } else {
        "air"
    })
}
fn terrain() -> CarvedTerrainWorld {
    let mut world = World::new("minecraft:overworld");
    let pos = ChunkPos { x: -1, z: 2 };
    let mut sections = BTreeMap::new();
    for sy in -1..=0i32 {
        let mut blocks = Box::new([BlockStateId(0); BLOCK_COUNT]);
        for y in 0..16i32 {
            for z in 0..16i32 {
                for x in 0..16i32 {
                    blocks[(x + z * 16 + y * 256) as usize] =
                        world.intern_state(initial(-16 + x, sy * 16 + y, 32 + z));
                }
            }
        }
        sections.insert(
            sy as i8,
            Arc::new(Section {
                blocks: Some(blocks),
                biomes: None,
                block_light: None,
                sky_light: None,
            }),
        );
    }
    world.chunks.insert(
        pos,
        Arc::new(Chunk {
            position: pos,
            status: Some("minecraft:carvers".into()),
            block_light_correct: false,
            sections,
        }),
    );
    CarvedTerrainWorld {
        world: Arc::new(world),
        seed: 0,
        min_y: -16,
        height: 32,
        completed_decoration: BTreeSet::new(),
        ore_ocean_floor_heights: BTreeMap::new(),
        feature_world_surface_heights: BTreeMap::new(),
        final_heightmaps: BTreeMap::new(),
        postprocessing: BTreeMap::new(),
        noise_postprocessing: BTreeMap::new(),
        surface_postprocessing: BTreeMap::new(),
        carver_postprocessing: BTreeMap::new(),
        feature_postprocessing: BTreeMap::new(),
    }
}
struct Data;
impl ResourceProvider for Data {
    fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        let values = match path {
            "data/minecraft/tags/block/supports_vegetation.json" => {
                vec!["minecraft:stone", "minecraft:dirt", "minecraft:grass_block"]
            }
            "data/minecraft/tags/block/supports_dry_vegetation.json" => vec![
                "minecraft:stone",
                "minecraft:dirt",
                "minecraft:grass_block",
                "minecraft:sand",
            ],
            "data/minecraft/tags/block/blocks_motion_in_heightmap.json"
            | "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json" => vec![
                "minecraft:stone",
                "minecraft:dirt",
                "minecraft:grass_block",
                "minecraft:sand",
                "minecraft:melon",
            ],
            _ => return Ok(None),
        };
        Ok(Some(serde_json::to_vec(&json!({"values":values}))?))
    }
}
fn registry() -> BTreeSet<String> {
    [
        "air",
        "cave_air",
        "void_air",
        "stone",
        "dirt",
        "grass_block",
        "sand",
        "water",
        "sunflower",
        "tall_grass",
        "short_grass",
        "fern",
        "poppy",
        "short_dry_grass",
        "tall_dry_grass",
        "sweet_berry_bush",
        "melon",
        "closed_eyeblossom",
    ]
    .into_iter()
    .map(|s| format!("minecraft:{s}"))
    .collect()
}
fn feature(kind: i32) -> Value {
    let to_place = match kind {
        0 => json!({"id":"minecraft:short_grass"}),
        1 => {
            json!({"type":"minecraft:weighted","entries":[{"data":"minecraft:short_grass","weight":3},{"data":"minecraft:fern","weight":1}]})
        }
        2 => {
            json!({"type":"minecraft:weighted","entries":[{"data":"minecraft:short_dry_grass","weight":1},{"data":"minecraft:tall_dry_grass","weight":1}]})
        }
        3 => json!({"id":"minecraft:tall_grass","properties":{"half":"lower"}}),
        4 => json!({"id":"minecraft:sunflower","properties":{"half":"upper"}}),
        5 => json!({"id":"minecraft:sweet_berry_bush","properties":{"age":"3"}}),
        6 => json!({"id":"minecraft:melon"}),
        _ => json!({"id":"minecraft:closed_eyeblossom"}),
    };
    json!({"type":"minecraft:simple_block","to_place":to_place})
}
fn key(state: &BlockState) -> String {
    let mut text = format!("{}|", state.name);
    for (key, value) in &state.properties {
        text.push_str(&format!("{key}={value};"));
    }
    text
}
#[derive(Default)]
struct Case {
    kind: i32,
    restricted: i32,
    n: i32,
    seed: i64,
    scheduled: bool,
    result: bool,
    second: bool,
    next: i64,
    blocks: BTreeMap<i32, String>,
    heights: [i32; 4],
    ticks: Vec<SavedBlockTick>,
}
fn run(cmd: &mut Command) -> String {
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "Java simple-block oracle failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn simple_features_match_java_survival_writes_and_randomness() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
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
    let source = temp.path().join("SimpleChunkOracle.java");
    std::fs::write(
        &source,
        include_str!("../../../tests/java/SimpleChunkOracle.java"),
    )
    .unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("SimpleChunkOracle"));
    let mut cases = Vec::<Case>::new();
    let mut safe = false;
    for line in output.lines() {
        let w: Vec<_> = line.split_whitespace().collect();
        match w.as_slice() {
            ["E", "safe"] => safe = true,
            ["C", kind, restricted, n, seed, scheduled] => cases.push(Case {
                kind: kind.parse().unwrap(),
                restricted: restricted.parse().unwrap(),
                n: n.parse().unwrap(),
                seed: seed.parse().unwrap(),
                scheduled: *scheduled == "1",
                ..Case::default()
            }),
            ["R", result, second, next] => {
                let c = cases.last_mut().unwrap();
                c.result = result.parse().unwrap();
                c.second = second.parse().unwrap();
                c.next = next.parse().unwrap();
            }
            ["B", y, value] => {
                cases
                    .last_mut()
                    .unwrap()
                    .blocks
                    .insert(y.parse().unwrap(), (*value).into());
            }
            ["H", a, b, c, d] => {
                cases.last_mut().unwrap().heights = [a, b, c, d].map(|v| v.parse().unwrap())
            }
            ["T", name, x, y, z, delay, priority] => {
                cases.last_mut().unwrap().ticks.push(SavedBlockTick {
                    block: (*name).into(),
                    position: [x.parse().unwrap(), y.parse().unwrap(), z.parse().unwrap()],
                    delay: delay.parse().unwrap(),
                    priority: priority.parse().unwrap(),
                })
            }
            _ => {}
        }
    }
    assert!(safe);
    assert_eq!(cases.len(), 512);
    let registry = registry();
    let motion = HeightmapTags::load(&mut Data, &registry).unwrap();
    let required = [
        ResourceId::parse("minecraft:supports_vegetation").unwrap(),
        ResourceId::parse("minecraft:supports_dry_vegetation").unwrap(),
    ]
    .into_iter()
    .collect();
    let tags = OreBlockTags::load(&mut Data, &registry, &required).unwrap();
    for case in cases {
        let mut config = feature(case.kind);
        config["schedule_tick"] = json!(case.scheduled);
        let plant = SimpleBlockFeature::decode(&config, &mut resolve).unwrap();
        let mut terrain = terrain();
        let center = ChunkPos { x: -1, z: 2 };
        let writable = if case.restricted == 0 {
            BTreeSet::from([center])
        } else {
            BTreeSet::new()
        };
        let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
        let mut random = DecorationRandom::new(0);
        let deco = random.set_decoration_seed(case.seed, -16, 32);
        random.set_feature_seed(deco, 7, 3);
        let pos = [-16 + case.n, 1, 32];
        assert_eq!(
            plant.place(&mut region, &mut random, pos).unwrap(),
            case.result,
            "kind {} n {}",
            case.kind,
            case.n
        );
        assert_eq!(
            plant.place(&mut region, &mut random, pos).unwrap(),
            case.second,
            "second attempt kind {} n {}",
            case.kind,
            case.n
        );
        assert_eq!(random.next_long(), case.next);
        for kind in 0..4 {
            assert_eq!(
                region.final_height(kind, pos[0], pos[2]).unwrap(),
                case.heights[kind],
                "height kind {kind}, feature {} n {}",
                case.kind,
                case.n
            );
        }
        region.finish().unwrap();
        let world = terrain.snapshot();
        assert_eq!(
            world
                .saved_block_ticks
                .get(&center)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
            case.ticks,
            "ticks kind {} restricted {} n {} seed {}",
            case.kind,
            case.restricted,
            case.n,
            case.seed
        );
        for y in [1, 2] {
            assert_eq!(
                key(world.block(pos[0], y, pos[2]).unwrap()),
                case.blocks[&y],
                "block y {y}, feature {} n {}",
                case.kind,
                case.n
            );
        }
    }
    let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
    let registry = resources.vanilla_generated_block_registry().unwrap();
    for name in [
        "grass",
        "grass_jungle",
        "flower_default",
        "tall_grass",
        "large_fern",
        "sunflower",
        "berry_bush",
        "dead_bush",
        "firefly_bush",
        "melon",
        "pumpkin",
        "flower_pale_garden",
        "pale_forest_flower",
    ] {
        SimpleBlockFeature::load(
            &mut resources,
            &ResourceId::parse(&format!("minecraft:{name}")).unwrap(),
            &mut |v| super::super::resolve_generated_terrain_state(v, &registry),
        )
        .unwrap_or_else(|e| panic!("{name}: {e:#}"));
    }
    for name in [
        "patch_grass_plain",
        "patch_tall_grass",
        "patch_sunflower",
        "patch_berry_bush",
    ] {
        PlacedDecorationFeature::load(
            &mut resources,
            &ResourceId::parse(&format!("minecraft:{name}")).unwrap(),
            &mut |v| super::super::resolve_generated_terrain_state(v, &registry),
        )
        .unwrap_or_else(|e| panic!("{name}: {e:#}"));
    }
}
