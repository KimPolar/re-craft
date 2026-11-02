//! Real Java ScatteredOreFeature using ProtoChunk block writes and heightmaps.
use super::*;
use native_resources::packs::ResourceProvider;
use native_world::loaded::{BLOCK_COUNT, BlockStateId, Chunk, Section, World};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};

struct Data {
    mode: i32,
}
impl ResourceProvider for Data {
    fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        let value = match path {
            "data/minecraft/tags/block/base_stone_nether.json" => Some(
                json!({"values":["minecraft:netherrack","minecraft:basalt","minecraft:blackstone"]}),
            ),
            "data/minecraft/tags/block/blocks_motion_in_heightmap.json"
            | "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json" => {
                let mut names = vec![
                    "minecraft:netherrack",
                    "minecraft:basalt",
                    "minecraft:blackstone",
                ];
                if self.mode == 0
                    || (self.mode == 2 && path.ends_with("blocks_motion_in_heightmap.json"))
                {
                    names.push("minecraft:ancient_debris");
                }
                Some(json!({"values":names}))
            }
            _ => None,
        };
        Ok(value.map(|v| serde_json::to_vec(&v).unwrap()))
    }
}
fn block(name: &str) -> BlockState {
    let properties = if name == "basalt" {
        BTreeMap::from([("axis".into(), "y".into())])
    } else {
        BTreeMap::new()
    };
    BlockState {
        name: format!("minecraft:{name}"),
        properties,
    }
}
fn terrain(seed: i64) -> CarvedTerrainWorld {
    let mut world = World::new("minecraft:the_nether");
    let ids = [
        world.intern_state(block("air")),
        world.intern_state(block("netherrack")),
        world.intern_state(block("basalt")),
        world.intern_state(block("blackstone")),
    ];
    for cz in 1..=3i32 {
        for cx in -2..=0i32 {
            let position = ChunkPos { x: cx, z: cz };
            let mut sections = BTreeMap::new();
            for sy in -1..=0i32 {
                let mut blocks = Box::new([BlockStateId(0); BLOCK_COUNT]);
                for y in 0..16i32 {
                    for z in 0..16i32 {
                        for x in 0..16i32 {
                            let key = (cx * 16 + x) * 31 + (sy * 16 + y) * 17 + (cz * 16 + z) * 13;
                            let id = if key & 7 == 0 {
                                0
                            } else {
                                1 + key.rem_euclid(3) as usize
                            };
                            blocks[(x + z * 16 + y * 256) as usize] = ids[id];
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
                position,
                Arc::new(Chunk {
                    position,
                    status: Some("minecraft:carvers".into()),
                    block_light_correct: false,
                    sections,
                }),
            );
        }
    }
    CarvedTerrainWorld {
        world: Arc::new(world),
        seed,
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
struct Env;
impl PlacementEnvironment for Env {
    fn min_y(&self) -> i32 {
        -16
    }
    fn heightmap(&mut self, _: &str, _: i32, _: i32) -> Result<i32> {
        bail!("unexpected heightmap")
    }
    fn biome_allows(&mut self, _: [i32; 3]) -> Result<bool> {
        bail!("unexpected biome")
    }
}
#[derive(Default)]
struct Case {
    mode: i32,
    size: i32,
    discard: String,
    seed: i64,
    writes: BTreeSet<[i32; 3]>,
    result: bool,
    next: i64,
    heights: BTreeMap<(i32, i32), [i32; 6]>,
    direct: [[i32; 4]; 3],
    direct_count: usize,
}
fn run(cmd: &mut Command) -> String {
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "Java scattered chunk oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn scattered_features_and_heightmaps_match_java_protochunks() {
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
                paths.push(p)
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("ScatteredChunkOracle.java");
    std::fs::write(
        &source,
        include_str!("../../../tests/java/ScatteredChunkOracle.java"),
    )
    .unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("ScatteredChunkOracle"));
    let mut cases = Vec::<Case>::new();
    let mut verified_effects = false;
    let mut fluid_cases = 0;
    for line in output.lines() {
        let words: Vec<_> = line.split_whitespace().collect();
        match words.as_slice() {
            ["E", "safe"] => verified_effects = true,
            ["F", name, wet, expected] => {
                let mut state = BlockState {
                    name: (*name).into(),
                    properties: BTreeMap::new(),
                };
                if *wet != "none" {
                    state.properties.insert("waterlogged".into(), (*wet).into());
                }
                assert_eq!(
                    native_world::fluid::FluidState::from_block(&state)
                        .unwrap()
                        .is_some(),
                    expected.parse::<bool>().unwrap(),
                    "fluid state for {name} waterlogged={wet}"
                );
                fluid_cases += 1;
            }
            ["C", mode, size, discard, seed] => cases.push(Case {
                mode: mode.parse().unwrap(),
                size: size.parse().unwrap(),
                discard: (*discard).into(),
                seed: seed.parse().unwrap(),
                ..Case::default()
            }),
            ["W", x, y, z] => {
                cases.last_mut().unwrap().writes.insert([
                    x.parse().unwrap(),
                    y.parse().unwrap(),
                    z.parse().unwrap(),
                ]);
            }
            ["R", result, next] => {
                let c = cases.last_mut().unwrap();
                c.result = result.parse().unwrap();
                c.next = next.parse().unwrap()
            }
            [
                "H",
                x,
                z,
                ocean,
                surface,
                final_surface,
                final_ocean,
                motion,
                no_leaves,
            ] => {
                cases.last_mut().unwrap().heights.insert(
                    (x.parse().unwrap(), z.parse().unwrap()),
                    [
                        ocean,
                        surface,
                        final_surface,
                        final_ocean,
                        motion,
                        no_leaves,
                    ]
                    .map(|v| v.parse().unwrap()),
                );
            }
            ["D", step, a, b, c, d] => {
                let case = cases.last_mut().unwrap();
                let step = step.parse::<usize>().unwrap();
                case.direct[step] = [a, b, c, d].map(|v| v.parse().unwrap());
                case.direct_count += 1;
            }
            _ => {}
        }
    }
    assert!(verified_effects);
    assert!(fluid_cases > 1000);
    assert_eq!(cases.len(), 48);
    let registry: BTreeSet<String> = [
        "air",
        "cave_air",
        "void_air",
        "netherrack",
        "basalt",
        "blackstone",
        "ancient_debris",
        "stone",
        "deepslate",
        "water",
        "lava",
    ]
    .into_iter()
    .map(|s| format!("minecraft:{s}"))
    .collect();
    let mut changed_top = 0;
    for case in cases {
        let mut data = Data { mode: case.mode };
        let value = json!({"feature":{"type":"minecraft:scattered_ore","size":case.size,"discard_chance_on_air_exposure":case.discard.parse::<f32>().unwrap(),"targets":[{"target":{"predicate_type":"minecraft:tag_match","tag":"minecraft:base_stone_nether"},"state":"minecraft:ancient_debris"}]},"placement":[]});
        let placement = PlacedFeature::decode(&value).unwrap();
        let feature = PlacedDecorationFeature::from_placement(&mut data, placement, &mut |v| {
            Ok(block(
                v.as_str().unwrap().strip_prefix("minecraft:").unwrap(),
            ))
        })
        .unwrap();
        let tags = OreBlockTags::load(&mut data, &registry, &feature.required_tags()).unwrap();
        let motion = HeightmapTags::load(&mut data, &registry).unwrap();
        let mut terrain = terrain(case.seed);
        let before = terrain.snapshot();
        let mut region = terrain
            .decoration_region(&tags, &motion, before.chunks.keys().copied().collect())
            .unwrap();
        let mut random = DecorationRandom::new(0);
        let deco = random.set_decoration_seed(case.seed, -16, 32);
        random.set_feature_seed(deco, 7, 3);
        let result = region
            .place_placed(&feature, &mut random, [-16, 14, 32], -16, 32, &mut Env)
            .unwrap();
        assert_eq!(result, case.result);
        assert_eq!(random.next_long(), case.next);
        for (&(x, z), heights) in &case.heights {
            assert_eq!(
                region.ocean_floor_height(x, z).unwrap(),
                heights[0],
                "ocean at {x},{z}"
            );
            assert_eq!(
                region.world_surface_height(x, z).unwrap(),
                heights[1],
                "surface at {x},{z}"
            );
            for kind in 0..4 {
                assert_eq!(
                    region.final_height(kind, x, z).unwrap(),
                    heights[kind + 2],
                    "final map {kind} at {x},{z}, mode {}, seed {}",
                    case.mode,
                    case.seed
                );
            }
        }
        if case.mode == 1 {
            changed_top += case.writes.iter().filter(|p| p[1] == 15).count();
        }
        region.finish().unwrap();
        let after = terrain.snapshot();
        let mut actual = BTreeSet::new();
        for z in 16..64 {
            for x in -32..16 {
                for y in -16..16 {
                    if after.block(x, y, z).unwrap() != before.block(x, y, z).unwrap() {
                        assert_eq!(
                            after.block(x, y, z).unwrap().name,
                            "minecraft:ancient_debris"
                        );
                        actual.insert([x, y, z]);
                    }
                }
            }
        }
        assert_eq!(
            actual, case.writes,
            "mode {} size {} discard {} seed {}",
            case.mode, case.size, case.discard, case.seed
        );
        assert_eq!(case.direct_count, 3);
        let mut region = terrain
            .decoration_region(&tags, &motion, after.chunks.keys().copied().collect())
            .unwrap();
        for step in 0..3 {
            match step {
                0 => region
                    .set_section_block([-11, 15, 37], &block("netherrack"))
                    .unwrap(),
                1 => {
                    region
                        .set_block([-11, 14, 37], &block("ancient_debris"), 2)
                        .unwrap();
                }
                2 => {
                    region
                        .set_block([-11, 15, 37], &block("ancient_debris"), 2)
                        .unwrap();
                }
                _ => unreachable!(),
            }
            for kind in 0..4 {
                assert_eq!(
                    region.final_height(kind, -11, 37).unwrap(),
                    case.direct[step][kind],
                    "direct write step {step}, map {kind}, mode {}, seed {}",
                    case.mode,
                    case.seed
                );
            }
        }
    }
    assert!(changed_top > 0, "no top-layer replacement was exercised");
}
