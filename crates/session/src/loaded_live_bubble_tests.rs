use super::*;
use native_resources::{
    ore_feature::OreBlockTags,
    packs::{ResourceId, ResourceProvider},
};

struct WallTestTags;

impl ResourceProvider for WallTestTags {
    fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(None)
    }

    fn read_data(&mut self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let values = match path {
            "data/minecraft/tags/block/walls.json" => {
                vec![
                    "minecraft:stone_brick_wall",
                    "minecraft:mossy_stone_brick_wall",
                ]
            }
            "data/minecraft/tags/block/wall_post_override.json"
            | "data/minecraft/tags/block/shulker_boxes.json" => vec![],
            _ => return Ok(None),
        };
        Ok(Some(serde_json::to_vec(
            &serde_json::json!({"values": values}),
        )?))
    }
}

fn wall(mossy: bool, complex: bool, wet: bool) -> BlockState {
    state(
        if mossy {
            "minecraft:mossy_stone_brick_wall"
        } else {
            "minecraft:stone_brick_wall"
        },
        &[
            ("north", "none"),
            ("east", "none"),
            ("south", if complex { "tall" } else { "none" }),
            ("west", "none"),
            ("up", "true"),
            ("waterlogged", if wet { "true" } else { "false" }),
        ],
    )
}

fn variant(index: u64) -> BlockState {
    match index {
        0 => state("minecraft:stone", &[]),
        1 => state("minecraft:mossy_stone_bricks", &[]),
        2 => state("minecraft:cracked_stone_bricks", &[]),
        3 => state("minecraft:crying_obsidian", &[]),
        4 => state("minecraft:gravel", &[]),
        5 => state("minecraft:suspicious_gravel", &[("dusted", "2")]),
        6 | 7 => state(
            "minecraft:chest",
            &[
                ("facing", "north"),
                ("type", if index == 6 { "single" } else { "left" }),
                ("waterlogged", "true"),
            ],
        ),
        8 => state(
            "minecraft:stone_brick_stairs",
            &[
                ("facing", "east"),
                ("half", "bottom"),
                ("shape", "straight"),
                ("waterlogged", "true"),
            ],
        ),
        9 => wall(false, false, true),
        10 => wall(true, true, true),
        11 => state("minecraft:kelp", &[("age", "17")]),
        12 => state("minecraft:kelp_plant", &[]),
        14 => state("minecraft:seagrass", &[]),
        15 => state("minecraft:tall_seagrass", &[("half", "lower")]),
        16 => state("minecraft:tall_seagrass", &[("half", "upper")]),
        17 => state(
            "minecraft:stone_slab",
            &[("type", "bottom"), ("waterlogged", "true")],
        ),
        _ => state("minecraft:glowstone", &[]),
    }
}

fn tags() -> (Arc<OreBlockTags>, Arc<BTreeSet<String>>) {
    let registry: BTreeSet<String> = (0..18)
        .map(variant)
        .map(|state| state.name)
        .chain(
            ["minecraft:air", "minecraft:water", "minecraft:lava"]
                .into_iter()
                .map(str::to_owned),
        )
        .collect();
    let ids = [
        "minecraft:walls",
        "minecraft:wall_post_override",
        "minecraft:shulker_boxes",
    ]
    .into_iter()
    .map(ResourceId::parse)
    .collect::<anyhow::Result<BTreeSet<_>>>()
    .unwrap();
    (
        Arc::new(OreBlockTags::load(&mut WallTestTags, &registry, &ids).unwrap()),
        Arc::new(registry),
    )
}

fn set_raw(session: &mut LoadedSession, position: IVec3, state: BlockState) -> BlockStateId {
    let id = session.intern_block(state);
    let chunk_pos = session.player_chunk().unwrap();
    let world = Arc::make_mut(&mut session.world);
    let chunk = Arc::make_mut(world.chunks.get_mut(&chunk_pos).unwrap());
    let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
    let index = position.x as usize + position.z as usize * 16 + position.y as usize * 256;
    section.blocks.as_mut().unwrap()[index] = id;
    id
}

fn filled_session() -> LoadedSession {
    let mut session = super::tests::session();
    let stone = session.intern_block(state("minecraft:stone", &[]));
    let chunk_pos = session.player_chunk().unwrap();
    let world = Arc::make_mut(&mut session.world);
    let chunk = Arc::make_mut(world.chunks.get_mut(&chunk_pos).unwrap());
    Arc::make_mut(chunk.sections.get_mut(&0).unwrap())
        .blocks
        .as_mut()
        .unwrap()
        .fill(stone);
    let (wall_tags, registry) = tags();
    session.set_wall_tags(wall_tags);
    session.set_vanilla_block_ids(registry);
    session.set_bubble_column_tags(Arc::new(
        native_resources::fluid_tags::BubbleColumnTags::new(BTreeSet::new(), BTreeSet::new()),
    ));
    session
}

#[test]
fn wall_fluid_reads_and_unverified_shape_changes_are_tracked() {
    let mut session = filled_session();
    let center = IVec3::new(8, 10, 8);
    let wet = set_raw(&mut session, center, wall(false, false, true));
    assert!(matches!(
        session.simulation_cell(center),
        fluid_sim::Cell::FenceWater(_)
    ));
    assert!(
        session
            .simulated_fluid(wet)
            .is_some_and(|fluid| fluid.source)
    );
    let dry = set_raw(&mut session, center, wall(false, false, false));
    assert_eq!(session.simulation_cell(center), fluid_sim::Cell::FenceDry);
    assert!(session.simulated_fluid(dry).is_none());

    let water = set_raw(
        &mut session,
        center + IVec3::X,
        state("minecraft:water", &[("level", "0")]),
    );
    assert!(session.update_wall_neighbor(center, dry, IVec3::X, water));
    assert!(!session.unresolved_wall_shape);
    let joined = set_raw(&mut session, center, wall(true, true, true));
    assert!(session.update_wall_neighbor(center, joined, IVec3::Z, water));
    assert!(session.unresolved_wall_shape);
    assert!(session.validate_generation_stage_block_readiness().is_err());
    assert!(
        session
            .draft_block_ticks(session.player_chunk().unwrap())
            .is_err()
    );
}

#[test]
fn seagrass_reads_source_water_but_changes_block_saving() {
    let mut session = filled_session();
    let center = IVec3::new(8, 10, 8);
    for plant in [
        state("minecraft:seagrass", &[]),
        state("minecraft:tall_seagrass", &[("half", "lower")]),
        state("minecraft:tall_seagrass", &[("half", "upper")]),
    ] {
        let id = set_raw(&mut session, center, plant);
        assert!(matches!(
            session.simulation_cell(center),
            fluid_sim::Cell::OpenWater(fluid) if fluid.source && fluid.kind == FluidKind::Water
        ));
        assert!(
            session
                .simulated_fluid(id)
                .is_some_and(|fluid| fluid.source)
        );
    }
    let invalid = set_raw(
        &mut session,
        center,
        state("minecraft:tall_seagrass", &[("half", "invalid")]),
    );
    assert_eq!(session.simulation_cell(center), fluid_sim::Cell::Unknown);
    assert!(session.simulated_fluid(invalid).is_none());

    let plant = set_raw(&mut session, center, state("minecraft:seagrass", &[]));
    let air = session.states[0];
    session.notify_one_block(center, air, plant);
    assert!(session.unresolved_seagrass_shape);
    assert!(session.validate_generation_stage_block_readiness().is_err());
    assert!(
        session
            .draft_block_ticks(session.player_chunk().unwrap())
            .is_err()
    );

    let mut session = filled_session();
    let tall = set_raw(
        &mut session,
        center + IVec3::X,
        state("minecraft:tall_seagrass", &[("half", "lower")]),
    );
    let stone = set_raw(&mut session, center, state("minecraft:stone", &[]));
    let air = session.states[0];
    session.notify_one_block(center, air, stone);
    assert!(!session.unresolved_seagrass_shape);
    assert!(session.simulated_fluid(tall).is_some());
    assert!(session.fluid_ticks.is_empty());

    let water = set_raw(
        &mut session,
        center + IVec3::X + IVec3::Y,
        state("minecraft:water", &[("level", "0")]),
    );
    assert!(session.update_seagrass_neighbor(center + IVec3::X, tall, IVec3::Y, water));
    assert!(session.unresolved_seagrass_shape);
    assert!(session.validate_generation_stage_block_readiness().is_err());
}

#[cfg(windows)]
fn seagrass_support(index: usize) -> BlockState {
    match index {
        0 => state("minecraft:stone", &[]),
        1 => state("minecraft:magma_block", &[]),
        2 => state("minecraft:air", &[]),
        3 => state("minecraft:water", &[("level", "0")]),
        4..=6 => state(
            "minecraft:stone_slab",
            &[
                ("type", ["bottom", "top", "double"][index - 4]),
                ("waterlogged", "false"),
            ],
        ),
        7..=8 => state(
            "minecraft:stone_brick_stairs",
            &[
                ("facing", "north"),
                ("half", if index == 7 { "top" } else { "bottom" }),
                ("shape", "straight"),
                ("waterlogged", "false"),
            ],
        ),
        9..=16 => state(
            if index < 13 {
                "minecraft:suspicious_sand"
            } else {
                "minecraft:suspicious_gravel"
            },
            &[("dusted", &((index - 9) % 4).to_string())],
        ),
        17 => state("minecraft:kelp", &[("age", "17")]),
        18 => state("minecraft:kelp_plant", &[]),
        19 => state("minecraft:seagrass", &[]),
        20..=21 => state(
            "minecraft:tall_seagrass",
            &[("half", if index == 20 { "lower" } else { "upper" })],
        ),
        _ => panic!("invalid support index"),
    }
}

#[cfg(windows)]
fn seagrass_direction(value: &str) -> IVec3 {
    match value {
        "west" => IVec3::NEG_X,
        "east" => IVec3::X,
        "north" => IVec3::NEG_Z,
        "south" => IVec3::Z,
        "down" => IVec3::NEG_Y,
        "up" => IVec3::Y,
        _ => panic!("invalid direction"),
    }
}

#[cfg(windows)]
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn seagrass_shape_and_water_ticks_match_java() {
    let rows = crate::worldgen::postprocessing_java_results(
        "SeagrassEdgeOracle",
        include_str!("../tests/java/SeagrassEdgeOracle.java"),
        &[],
    );
    let center = IVec3::new(8, 10, 8);
    let mut session = filled_session();
    let mut checked = 0;
    let delays = rows.iter().filter_map(|row| row.strip_prefix("R "));
    for (row, delay) in rows
        .iter()
        .filter_map(|row| row.strip_prefix("E "))
        .zip(delays)
    {
        let fields: Vec<_> = row.split_whitespace().collect();
        assert_eq!(fields.len(), 9, "{row}");
        let mask: u8 = fields[0].parse().unwrap();
        let mut tags = BTreeSet::new();
        if mask & 1 != 0 {
            tags.extend([
                "minecraft:stone".into(),
                "minecraft:suspicious_gravel".into(),
            ]);
        }
        if mask & 2 != 0 {
            tags.extend([
                "minecraft:magma_block".into(),
                "minecraft:suspicious_sand".into(),
            ]);
        }
        session.set_seagrass_unsupported_blocks(Arc::new(tags));
        set_raw(
            &mut session,
            center - IVec3::Y,
            seagrass_support(fields[1].parse().unwrap()),
        );
        let plant = set_raw(&mut session, center, state("minecraft:seagrass", &[]));
        let neighbor = session.intern_block(match fields[2] {
            "0" => state("minecraft:air", &[]),
            "1" => state("minecraft:stone", &[]),
            "2" => state("minecraft:water", &[("level", "0")]),
            _ => panic!("invalid neighbor"),
        });
        let direction = seagrass_direction(fields[3]);
        let survives = fields[4] == "minecraft:seagrass";
        assert_eq!(
            session.seagrass_shape_unchanged(center, SeagrassPart::Short, direction, None),
            Some(survives),
            "{row}"
        );
        session.fluid_ticks.clear();
        session.fluid_pending.clear();
        session.unresolved_seagrass_shape = false;
        let before = session.next_sub_tick;
        for _ in 0..2 {
            assert!(session.update_seagrass_neighbor(center, plant, direction, neighbor));
        }
        assert_eq!(session.unresolved_seagrass_shape, !survives, "{row}");
        assert_eq!(
            session.next_sub_tick - before,
            fields[5].parse::<i64>().unwrap(),
            "{row}"
        );
        assert_eq!(
            session.fluid_ticks.len(),
            fields[6].parse::<usize>().unwrap(),
            "{row}"
        );
        if survives {
            assert_eq!(
                session.fluid_ticks.keys().next().unwrap().0,
                delay.parse::<i128>().unwrap(),
                "{row}"
            );
        }
        checked += 1;
    }
    assert_eq!(checked, 4 * 22 * 3 * 6);
}

#[cfg(windows)]
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn tall_seagrass_shape_and_water_ticks_match_java() {
    let rows = crate::worldgen::postprocessing_java_results(
        "TallSeagrassEdgeOracle",
        include_str!("../tests/java/TallSeagrassEdgeOracle.java"),
        &[],
    );
    let center = IVec3::new(8, 10, 8);
    let mut session = filled_session();
    let mut checked = 0;
    for row in rows.iter().filter_map(|row| row.strip_prefix("T ")) {
        let fields: Vec<_> = row.split_whitespace().collect();
        assert_eq!(fields.len(), 10, "{row}");
        let part = if fields[0] == "0" {
            SeagrassPart::Lower
        } else {
            SeagrassPart::Upper
        };
        let direction = seagrass_direction(fields[1]);
        let support = match fields[2].parse::<usize>().unwrap() {
            i @ 0..=5 => seagrass_support(i),
            6 => seagrass_support(20),
            7 => seagrass_support(21),
            8 => seagrass_support(18),
            9 => seagrass_support(13),
            _ => panic!("invalid support"),
        };
        let current = match fields[3] {
            "0" => seagrass_support(20),
            "1" => state("minecraft:water", &[("level", "0")]),
            "2" => state("minecraft:water", &[("level", "1")]),
            "3" => state("minecraft:water", &[("level", "8")]),
            "4" => state("minecraft:lava", &[("level", "0")]),
            "5" => state("minecraft:air", &[]),
            "6" => seagrass_support(21),
            _ => panic!("invalid live state"),
        };
        let neighbor = match fields[4] {
            "0" => state("minecraft:air", &[]),
            "1" => state("minecraft:stone", &[]),
            "2" => seagrass_support(20),
            "3" => seagrass_support(21),
            _ => panic!("invalid neighbor"),
        };
        let mask: u8 = fields[5].parse().unwrap();
        let mut tags = BTreeSet::new();
        if mask & 1 != 0 {
            tags.insert("minecraft:stone".into());
        }
        if mask & 2 != 0 {
            tags.insert("minecraft:magma_block".into());
        }
        session.set_seagrass_unsupported_blocks(Arc::new(tags));
        session.farmland_water_tag = Some(fields[6].parse::<u8>().unwrap() << 1);
        set_raw(&mut session, center - IVec3::Y, support);
        set_raw(&mut session, center, current);
        let plant = session.intern_block(state(
            "minecraft:tall_seagrass",
            &[(
                "half",
                if part == SeagrassPart::Lower {
                    "lower"
                } else {
                    "upper"
                },
            )],
        ));
        let other = session.intern_block(neighbor.clone());
        let survives = fields[7] == "minecraft:tall_seagrass";
        assert_eq!(fields[7], fields[8], "{row}");
        assert_eq!(
            session.seagrass_shape_unchanged(center, part, direction, seagrass_part(&neighbor)),
            Some(survives),
            "{row}"
        );
        session.unresolved_seagrass_shape = false;
        let before = session.next_sub_tick;
        for _ in 0..2 {
            assert!(session.update_seagrass_neighbor(center, plant, direction, other));
        }
        assert_eq!(session.unresolved_seagrass_shape, !survives, "{row}");
        assert_eq!(session.next_sub_tick, before, "{row}");
        let mut random = native_core::random::LegacyRandom::new(17);
        assert_eq!(
            random.next_int(),
            fields[9].parse::<i32>().unwrap(),
            "{row}"
        );
        checked += 1;
    }
    assert_eq!(checked, 12_960);
}

#[cfg(windows)]
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn cached_inert_cube_fluid_faces_match_java() {
    let rows = crate::worldgen::postprocessing_java_results(
        "GeneratedInertPostprocessingOracle",
        include_str!("../tests/java/GeneratedInertPostprocessingOracle.java"),
        &[],
    );
    let mut names = BTreeSet::new();
    for row in rows {
        if row.starts_with("P ") || row.starts_with("Q ") {
            continue;
        }
        let name = row.split_whitespace().next().unwrap();
        names.insert(format!("minecraft:{name}"));
    }
    assert!(names.len() >= 150);
    let mut session = filled_session();
    let position = IVec3::new(8, 10, 8);
    for name in names {
        let block = state(&name, &[]);
        assert!(crate::worldgen::inert_generated_full_cube(&name), "{name}");
        assert_eq!(
            loaded_block_shape(&block),
            Some(vec![Aabb {
                min: Vec3::ZERO,
                max: Vec3::ONE
            }]),
            "{name}"
        );
        set_raw(&mut session, position, block);
        assert_eq!(
            session.simulation_cell(position),
            fluid_sim::Cell::Solid,
            "{name}"
        );
        let settings = session.fluid_settings();
        let live = SessionFluidTick {
            session: &mut session,
            settings,
            kind: FluidKind::Water,
            retry_wash: false,
            bubble_complete: Some(std::cell::Cell::new(true)),
        };
        live.observe_bubble_cell(position, fluid_sim::Cell::Solid);
        assert!(live.bubble_complete.unwrap().get(), "{name}");
    }
}

#[cfg(windows)]
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn source_fluid_callbacks_match_java_for_certified_cases() {
    use std::{path::PathBuf, process::Command};
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let version = game.join("versions/26.3");
    let metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for library in metadata["libraries"].as_array().unwrap() {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let classpath = std::env::join_paths(paths).unwrap();
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let source = temp.path().join("LiveBubbleFluidOracle.java");
    let output = temp.path().join("results.json");
    std::fs::write(
        &source,
        include_str!("../tests/java/LiveBubbleFluidOracle.java"),
    )
    .unwrap();
    for command in [
        Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source),
        Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("LiveBubbleFluidOracle")
            .arg(&output),
    ] {
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let rows: Vec<serde_json::Value> =
        serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(rows.len(), 1188);
    let center = IVec3::new(8, 10, 8);
    let mut certified = 0;
    let mut read_cases = BTreeSet::new();
    let mut changed_cases = BTreeSet::new();
    let mut failures = Vec::new();
    let mut unsupported_destructions = 0;
    for row in rows {
        let source = row["source"].as_u64().unwrap();
        let mode = row["mode"].as_u64().unwrap();
        let variant_index = row["variant"].as_u64().unwrap();
        let direction = match row["direction"].as_str().unwrap() {
            "west" => IVec3::NEG_X,
            "east" => IVec3::X,
            "north" => IVec3::NEG_Z,
            "south" => IVec3::Z,
            "down" => IVec3::NEG_Y,
            "up" => IVec3::Y,
            _ => unreachable!(),
        };
        let mut session = filled_session();
        let root = match source {
            0 => state("minecraft:bubble_column", &[("drag", "true")]),
            1 => state("minecraft:kelp", &[("age", "7")]),
            2 => wall(false, false, true),
            3 => state("minecraft:seagrass", &[]),
            4 => state("minecraft:tall_seagrass", &[("half", "lower")]),
            _ => state("minecraft:tall_seagrass", &[("half", "upper")]),
        };
        let root_id = set_raw(&mut session, center, root);
        let target = if mode == 1 {
            center - IVec3::Y
        } else {
            center + IVec3::X
        };
        let special = if mode == 0 {
            center + IVec3::X
        } else {
            target + direction
        };
        set_raw(&mut session, special, variant(variant_index));
        if mode != 0 {
            set_raw(&mut session, target, state("minecraft:air", &[]));
        }
        session.level_random = native_core::random::LegacyRandom::new(42);
        let fluid = session.simulated_fluid(root_id).unwrap();
        session.schedule_fluid(center, fluid, 0);
        let start_subtick = session.next_sub_tick;
        session.advance_fluid_ticks();
        let safe = session.validate_generation_stage_block_readiness().is_ok();
        if row["destroyed"].as_bool().unwrap() {
            unsupported_destructions += 1;
            if safe {
                failures.push(format!("plant destruction was certified: {row}"));
            }
            continue;
        }
        if mode == 0 && variant_index != 13 {
            if safe {
                read_cases.insert((source, variant_index));
            } else {
                failures.push(format!("read-only source tick was rejected: {row}"));
            }
        }
        if !safe {
            continue;
        }
        certified += 1;
        if mode != 0 && !row["changes"].as_array().unwrap().is_empty() {
            changed_cases.insert((source, variant_index));
        }
        assert_eq!(row["gameEvents"], 0, "{row}");
        assert_eq!(row["levelEvents"], 0, "{row}");
        let mut actual_changes = BTreeMap::new();
        for edits in session.block_edits.values() {
            for (&(x, y, z), edit) in edits {
                let block = session.world.state(edit.current).unwrap();
                let level = block
                    .properties
                    .get("level")
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(-1);
                let age = if block.name == "minecraft:kelp" {
                    block.properties["age"].parse::<i64>().unwrap()
                } else {
                    -1
                };
                actual_changes.insert(
                    (
                        x - session.origin.x,
                        y - session.origin.y,
                        z - session.origin.z,
                    ),
                    serde_json::json!([
                        x - session.origin.x,
                        y - session.origin.y,
                        z - session.origin.z,
                        block.name,
                        level,
                        age
                    ]),
                );
            }
        }
        let mut expected_changes = BTreeMap::new();
        for change in row["changes"].as_array().unwrap() {
            let xyz = (
                change[0].as_i64().unwrap() as i32,
                change[1].as_i64().unwrap() as i32,
                change[2].as_i64().unwrap() as i32,
            );
            expected_changes.insert(xyz, change.clone());
        }
        if actual_changes != expected_changes {
            failures.push(format!("block changes: {row}; actual {actual_changes:?}"));
        }

        let mut actual_ticks = Vec::new();
        for (order, scheduled) in &session.fluid_ticks {
            let key = scheduled.key;
            actual_ticks.push((
                order.2 - start_subtick,
                serde_json::json!([
                    "fluid",
                    if key.source {
                        "minecraft:water"
                    } else {
                        "minecraft:flowing_water"
                    },
                    key.x - session.origin.x,
                    key.y - session.origin.y,
                    key.z - session.origin.z,
                    order.0
                ]),
            ));
        }
        for (order, key) in &session.bubble_ticks {
            actual_ticks.push((
                order.2 - start_subtick,
                serde_json::json!([
                    "block",
                    match key.kind {
                        BubbleTickKind::Water => "minecraft:water",
                        BubbleTickKind::Column => "minecraft:bubble_column",
                    },
                    key.x - session.origin.x,
                    key.y - session.origin.y,
                    key.z - session.origin.z,
                    order.0
                ]),
            ));
        }
        for (order, key) in &session.sand_ticks {
            actual_ticks.push((
                order.2 - start_subtick,
                serde_json::json!([
                    "block",
                    key.kind.id(),
                    key.x - session.origin.x,
                    key.y - session.origin.y,
                    key.z - session.origin.z,
                    order.0
                ]),
            ));
        }
        actual_ticks.sort_by_key(|entry| entry.0);
        let mut seen = BTreeSet::new();
        let mut expected_ticks = Vec::new();
        for (index, call) in row["calls"].as_array().unwrap().iter().enumerate() {
            let key = serde_json::to_string(&call.as_array().unwrap()[..5]).unwrap();
            if seen.insert(key) {
                expected_ticks.push((index as i64, call.clone()));
            }
        }
        if actual_ticks != expected_ticks {
            failures.push(format!("tick order: {row}; actual {actual_ticks:?}"));
        }
        if session.next_sub_tick - start_subtick != row["calls"].as_array().unwrap().len() as i64 {
            failures.push(format!(
                "schedule call count: {row}; actual {}",
                session.next_sub_tick - start_subtick
            ));
        }
        if session.level_random.next_int() != row["next"].as_i64().unwrap() as i32 {
            failures.push(format!("random state: {row}"));
        }
    }
    eprintln!("Compared {certified} certified source fluid ticks with Java 26.3");
    assert!(
        failures.is_empty(),
        "{} mismatches; examples:\n{}",
        failures.len(),
        failures
            .iter()
            .take(12)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert_eq!(read_cases.len(), 102);
    assert!(unsupported_destructions > 0);
    assert!(
        certified >= 367,
        "certified source-fluid coverage decreased: {certified}"
    );
    for (source, variant) in [(0, 0), (0, 6), (0, 9), (0, 14), (0, 15), (1, 0), (2, 0)] {
        assert!(
            changed_cases.contains(&(source, variant)),
            "missing certified source {source}, neighbor {variant}"
        );
    }
}
