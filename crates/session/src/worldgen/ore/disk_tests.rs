//! Disk placement against the installed Java feature and real ProtoChunks.
use super::*;
use native_resources::packs::ResourceProvider;
use native_world::loaded::{BLOCK_COUNT, Chunk, Section};
use serde_json::json;
use std::{path::PathBuf, process::Command};

fn state(name: &str) -> BlockState {
    BlockState {
        name: format!("minecraft:{name}"),
        properties: if name == "grass_block" {
            BTreeMap::from([("snowy".into(), "false".into())])
        } else {
            BTreeMap::new()
        },
    }
}
fn initial(x: i32, y: i32, z: i32) -> BlockState {
    let n = (x * 31 + y * 17 + z * 13).rem_euclid(8);
    state(if y < 0 {
        "stone"
    } else if y == 0 {
        ["mud", "clay", "grass_block", "dirt"][(n % 4) as usize]
    } else if y == 1 {
        ["air", "grass_block", "mud", "dirt"][(n % 4) as usize]
    } else if y == 2 {
        ["water", "dirt", "air", "stone"][(n % 4) as usize]
    } else if y == 3 && n < 2 {
        "stone"
    } else {
        "air"
    })
}
fn terrain() -> CarvedTerrainWorld {
    let mut world = World::new("minecraft:overworld");
    for cz in 1..=3i32 {
        for cx in -2..=0i32 {
            let pos = ChunkPos { x: cx, z: cz };
            let mut sections = BTreeMap::new();
            for sy in -1..=0i32 {
                let mut blocks = Box::new([BlockStateId(0); BLOCK_COUNT]);
                for y in 0..16i32 {
                    for z in 0..16i32 {
                        for x in 0..16i32 {
                            blocks[(x + z * 16 + y * 256) as usize] =
                                world.intern_state(initial(cx * 16 + x, sy * 16 + y, cz * 16 + z));
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
                    status: Some("minecraft:terrain".into()),
                    block_light_correct: false,
                    sections,
                }),
            );
        }
    }
    CarvedTerrainWorld {
        world: Arc::new(world),
        seed: 0,
        min_y: -16,
        height: 32,
        completed_decoration: BTreeSet::new(),
        has_published_light: false,
        ore_ocean_floor_heights: BTreeMap::new(),
        feature_world_surface_heights: BTreeMap::new(),
        final_heightmaps: BTreeMap::new(),
        postprocessing: BTreeMap::new(),
        noise_postprocessing: BTreeMap::new(),
        surface_postprocessing: BTreeMap::new(),
        carver_postprocessing: BTreeMap::new(),
        feature_postprocessing: BTreeMap::new(),
        generated_bee_nests: Arc::new(BTreeMap::new()),
        generated_monster_entities: Arc::new(BTreeMap::new()),
    }
}
const SOLID: &[&str] = &[
    "stone",
    "deepslate",
    "netherrack",
    "basalt",
    "blackstone",
    "ancient_debris",
    "dirt",
    "grass_block",
    "mud",
    "clay",
    "sand",
    "sandstone",
    "gravel",
    "packed_ice",
    "snow_block",
    "ice",
    "podzol",
    "coarse_dirt",
    "mycelium",
    "tuff",
    "calcite",
    "granite",
    "diorite",
    "andesite",
    "bedrock",
    "red_sand",
    "moss_block",
];
struct Data;
impl ResourceProvider for Data {
    fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        if path == "data/minecraft/tags/block/blocks_motion_in_heightmap.json"
            || path == "data/minecraft/tags/block/blocks_motion_in_heightmap_no_leaves.json"
        {
            return Ok(Some(serde_json::to_vec(
                &json!({"values":SOLID.iter().map(|s|format!("minecraft:{s}")).collect::<Vec<_>>()}),
            )?));
        }
        Ok(None)
    }
}
fn registry() -> BTreeSet<String> {
    SOLID
        .iter()
        .copied()
        .chain(["air", "cave_air", "void_air", "water", "lava"])
        .map(|s| format!("minecraft:{s}"))
        .collect()
}
fn feature(kind: i32) -> Value {
    let (target, provider, radius, half) = match kind {
        0 => (
            json!(["minecraft:dirt", "minecraft:clay"]),
            json!({"type":"minecraft:simple","state":{"id":"minecraft:clay"}}),
            3,
            1,
        ),
        1 => (
            json!(["minecraft:dirt", "minecraft:grass_block"]),
            json!({"type":"minecraft:rule_based","fallback":{"id":"minecraft:sand"},"rules":[{"if_true":{"type":"minecraft:matching_blocks","blocks":"minecraft:air","offset":[0,-1,0]},"then":{"id":"minecraft:sandstone"}}]}),
            6,
            2,
        ),
        2 => (
            json!(["minecraft:dirt", "minecraft:mud"]),
            json!({"type":"minecraft:rule_based","fallback":{"id":"minecraft:dirt"},"rules":[{"if_true":{"type":"minecraft:not","predicate":{"type":"minecraft:any_of","predicates":[{"type":"minecraft:solid","offset":[0,1,0]},{"type":"minecraft:matching_fluids","fluids":"minecraft:water","offset":[0,1,0]}]}},"then":{"id":"minecraft:grass_block","properties":{"snowy":"false"}}}]}),
            6,
            2,
        ),
        _ => (
            json!(["minecraft:dirt", "minecraft:grass_block"]),
            json!({"type":"minecraft:rule_based","rules":[{"if_true":{"type":"minecraft:matching_blocks","blocks":"minecraft:air","offset":[0,-1,0]},"then":{"id":"minecraft:clay"}}]}),
            6,
            2,
        ),
    };
    json!({"type":"minecraft:disk","target":{"type":"minecraft:matching_blocks","blocks":target},"state_provider":provider,"radius":{"type":"minecraft:uniform","min_inclusive":2,"max_inclusive":radius},"half_height":half})
}
#[derive(Default)]
struct Case {
    kind: i32,
    restricted: i32,
    seed: i64,
    result: bool,
    next: i64,
    blocks: BTreeMap<[i32; 3], String>,
    marks: BTreeMap<(i32, i32, usize), Vec<i16>>,
}
struct FilterEnvironment<'a, 'b>(&'a mut DecorationRegion<'b>);
impl PlacementEnvironment for FilterEnvironment<'_, '_> {
    fn min_y(&self) -> i32 {
        -16
    }
    fn heightmap(&mut self, _: &str, _: i32, _: i32) -> Result<i32> {
        bail!("unexpected heightmap")
    }
    fn biome_allows(&mut self, _: [i32; 3]) -> Result<bool> {
        bail!("unexpected biome")
    }
    fn block_predicate(&mut self, predicate: &BlockPredicate, pos: [i32; 3]) -> Result<bool> {
        predicate.test(self.0, pos)
    }
}
fn run(cmd: &mut Command) -> String {
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "Java disk oracle failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn disk_features_match_java_chunks_and_postprocessing() {
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
    let source = temp.path().join("DiskChunkOracle.java");
    std::fs::write(
        &source,
        include_str!("../../../tests/java/DiskChunkOracle.java"),
    )
    .unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("DiskChunkOracle"));
    let mut cases = Vec::<Case>::new();
    let mut safe = false;
    for line in output.lines() {
        let w: Vec<_> = line.split_whitespace().collect();
        match w.as_slice() {
            ["E", "safe"] => safe = true,
            ["C", kind, restricted, seed] => cases.push(Case {
                kind: kind.parse().unwrap(),
                restricted: restricted.parse().unwrap(),
                seed: seed.parse().unwrap(),
                ..Case::default()
            }),
            ["R", result, next] => {
                let c = cases.last_mut().unwrap();
                c.result = result.parse().unwrap();
                c.next = next.parse().unwrap();
            }
            ["B", x, y, z, name] => {
                cases.last_mut().unwrap().blocks.insert(
                    [x.parse().unwrap(), y.parse().unwrap(), z.parse().unwrap()],
                    (*name).into(),
                );
            }
            ["M", x, z, section, packed] => cases
                .last_mut()
                .unwrap()
                .marks
                .entry((
                    x.parse().unwrap(),
                    z.parse().unwrap(),
                    section.parse().unwrap(),
                ))
                .or_default()
                .push(packed.parse().unwrap()),
            _ => {}
        }
    }
    assert!(safe);
    assert_eq!(cases.len(), 64);
    let registry = registry();
    let motion = HeightmapTags::load(&mut Data, &registry).unwrap();
    let tags = OreBlockTags::load(&mut Data, &registry, &BTreeSet::new()).unwrap();
    let mut total_marks = 0;
    for case in cases {
        let disk = DiskFeature::decode(&feature(case.kind % 4), &mut |v| {
            Ok(state(
                v.get("id")
                    .and_then(Value::as_str)
                    .unwrap()
                    .strip_prefix("minecraft:")
                    .unwrap(),
            ))
        })
        .unwrap();
        let mut terrain = terrain();
        let before = terrain.snapshot();
        let writable = if case.restricted == 1 {
            BTreeSet::from([ChunkPos { x: -1, z: 2 }])
        } else {
            before.chunks.keys().copied().collect()
        };
        let mut region = terrain.decoration_region(&tags, &motion, writable).unwrap();
        let mut random = DecorationRandom::new(0);
        let deco = random.set_decoration_seed(case.seed, -16, 32);
        random.set_feature_seed(deco, 7, 3);
        let result = if case.kind < 4 {
            disk.place(&mut region, &mut random, [-16, 0, 32]).unwrap()
        } else {
            let placement = PlacedFeature::decode(&json!({"feature":feature(case.kind % 4),"placement":[
                {"type":"minecraft:count","count":4}, {"type":"minecraft:in_square"},
                {"type":"minecraft:block_predicate_filter","predicate":{"type":"minecraft:matching_blocks","blocks":["minecraft:dirt","minecraft:mud","minecraft:grass_block"]}}
            ]})).unwrap();
            let mut cursor = placement.positions([-16, 0, 32], -16, 32).unwrap();
            let mut placed = false;
            loop {
                let next = cursor
                    .next(&mut random, &mut FilterEnvironment(&mut region))
                    .unwrap();
                let Some(pos) = next else { break };
                placed |= disk.place(&mut region, &mut random, pos).unwrap();
            }
            placed
        };
        assert_eq!(result, case.result);
        assert_eq!(random.next_long(), case.next);
        region.finish().unwrap();
        let after = terrain.snapshot();
        let mut blocks = BTreeMap::new();
        let mut marks = BTreeMap::new();
        for cz in 1..=3 {
            for cx in -2..=0 {
                for y in -3..=4 {
                    for z in 0..16 {
                        for x in 0..16 {
                            let (wx, wz) = (cx * 16 + x, cz * 16 + z);
                            let b = before.block(wx, y, wz).unwrap();
                            let a = after.block(wx, y, wz).unwrap();
                            if a != b {
                                blocks.insert(
                                    [wx, y, wz],
                                    a.name.strip_prefix("minecraft:").unwrap().to_owned(),
                                );
                            }
                        }
                    }
                }
                if let Some(value) = terrain.feature_postprocessing(ChunkPos { x: cx, z: cz }) {
                    for (section, values) in value.sections().iter().enumerate() {
                        if !values.is_empty() {
                            marks.insert((cx, cz, section), values.clone());
                        }
                    }
                }
            }
        }
        assert_eq!(
            blocks, case.blocks,
            "blocks kind {} restricted {} seed {}",
            case.kind, case.restricted, case.seed
        );
        assert_eq!(
            marks, case.marks,
            "marks kind {} restricted {} seed {}",
            case.kind, case.restricted, case.seed
        );
        total_marks += marks.values().map(Vec::len).sum::<usize>();
    }
    assert!(total_marks > 0);
    let mut installed = native_resources::packs::ResourceStack::open(&game).unwrap();
    let block_registry = installed.vanilla_generated_block_registry().unwrap();
    for name in [
        "disk_clay",
        "disk_grass",
        "disk_gravel",
        "disk_sand",
        "ice_patch",
    ] {
        DiskFeature::load(
            &mut installed,
            &ResourceId::parse(&format!("minecraft:{name}")).unwrap(),
            &mut |v| super::super::resolve_generated_terrain_state(v, &block_registry),
        )
        .unwrap();
        PlacedDecorationFeature::load(
            &mut installed,
            &ResourceId::parse(&format!("minecraft:{name}")).unwrap(),
            &mut |v| super::super::resolve_generated_terrain_state(v, &block_registry),
        )
        .unwrap();
    }
}
