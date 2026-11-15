use super::*;
use crate::worldgen::chunk_ticks::{ChunkTicks, ConvertedChunkTicks};
use native_core::random::{LegacyRandom, XoroshiroRandom};
use native_resources::{
    ore_feature::OreBlockTags,
    packs::{ResourceId, ResourceProvider},
};

struct KelpTags(u8);

impl ResourceProvider for KelpTags {
    fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }

    fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        if path != "data/minecraft/tags/block/cannot_support_kelp.json" {
            return Ok(None);
        }
        let mut values = Vec::new();
        if self.0 & 1 != 0 {
            values.push("minecraft:stone");
        }
        if self.0 & 2 != 0 {
            values.push("minecraft:kelp");
        }
        Ok(Some(serde_json::to_vec(
            &serde_json::json!({ "values": values }),
        )?))
    }
}

fn state(name: &str) -> BlockState {
    BlockState {
        name: format!("minecraft:{name}"),
        properties: BTreeMap::new(),
    }
}

fn head() -> BlockState {
    BlockState {
        name: "minecraft:kelp".into(),
        properties: BTreeMap::from([("age".into(), "17".into())]),
    }
}

fn support(index: usize) -> BlockState {
    match index {
        0 => state("stone"),
        1 => BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "0".into())]),
        },
        2 => state("air"),
        3 => head(),
        4 => state("kelp_plant"),
        5 | 6 => BlockState {
            name: "minecraft:stone_slab".into(),
            properties: BTreeMap::from([
                (
                    "type".into(),
                    if index == 5 { "bottom" } else { "top" }.into(),
                ),
                ("waterlogged".into(), "false".into()),
            ]),
        },
        _ => panic!("invalid support"),
    }
}

fn above(index: usize) -> BlockState {
    match index {
        0 => state("air"),
        1 => head(),
        2 => state("kelp_plant"),
        _ => panic!("invalid upper block"),
    }
}

fn neighbor(index: usize) -> BlockState {
    match index {
        0 => state("air"),
        1 => state("stone"),
        2 => head(),
        3 => state("kelp_plant"),
        _ => panic!("invalid neighbor"),
    }
}

fn describe(state: &BlockState) -> String {
    format!(
        "{}:{}",
        state.name,
        state.properties.get("age").map_or("-", String::as_str)
    )
}

/// Use the same converted world and tick containers as the normal poll path.
fn update_twice(
    stage: &mut GeneratedPostprocessing,
    chunk: ChunkPos,
    state: &BlockState,
    direction: [i32; 3],
    neighbor: &BlockState,
) -> Result<(BlockState, BlockState)> {
    let mut entities = stage
        .block_entities
        .remove(&chunk)
        .context("missing entities")?;
    let mut view = GeneratedBlockView {
        chunk,
        upgrade_ticks: None,
        block_entities: &mut entities,
        other_block_entities: &stage.block_entities,
        entity_candidates: &stage.entity_candidates,
        block_entity_edits: &mut stage.block_entity_edits,
        world: &mut stage.world,
        read_chunks: &stage.read_chunks,
        light_reads: &stage.light_reads,
        min_y: stage.min_y,
        max_y: stage.max_y,
        ticks: &mut stage.region.ticks,
        light: stage.light.as_ref(),
        mushroom_tags: stage.mushroom_tags.as_deref(),
        aquatic_tags: stage.aquatic_tags.as_deref(),
        leaf_tags: stage.leaf_tags.as_deref(),
        snow_tags: stage.snow_tags.as_deref(),
        fence_tags: stage.fence_tags.as_deref(),
        speleothem_tags: stage.speleothem_tags.as_deref(),
        bubble_tags: stage.bubble_tags.as_deref(),
        washed_blocks: stage.washed_blocks.as_deref(),
        lava_fluids: stage.lava_fluids.as_deref(),
        water_fluids: stage.water_fluids.as_deref(),
        water_source_conversion: stage.water_source_conversion,
        lava_rules: stage.lava_rules,
        lava_fizz: &mut stage.lava_fizz,
        block_drops: &mut stage.block_drops,
        edits: &mut stage.edits,
        edit_states: &mut stage.edit_states,
        light_checks: &mut stage.light_checks,
        heightmaps: &mut stage.heightmaps,
        heightmap_tags: stage.heightmap_tags.as_deref(),
        destroyed_plant_during_mark: BTreeSet::new(),
    };
    let pos = [8, 8, 8];
    let neighbor_pos = [
        pos[0] + direction[0],
        pos[1] + direction[1],
        pos[2] + direction[2],
    ];
    let result = (|| {
        Ok((
            view.update_shape(pos, state, direction, neighbor_pos, neighbor)?,
            view.update_shape(pos, state, direction, neighbor_pos, neighbor)?,
        ))
    })();
    stage.block_entities.insert(chunk, entities);
    result
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn level_kelp_shape_callbacks_match_java() -> Result<()> {
    let rows = super::tests::java_results(
        "KelpEdgeOracle",
        include_str!("../../tests/java/KelpEdgeOracle.java"),
    );
    let registry: BTreeSet<_> = ["air", "stone", "water", "stone_slab", "kelp", "kelp_plant"]
        .into_iter()
        .map(|name| format!("minecraft:{name}"))
        .collect();
    let tag = ResourceId::parse("minecraft:cannot_support_kelp")?;
    let all_tags = (0..4)
        .map(|mask| {
            OreBlockTags::load(
                &mut KelpTags(mask),
                &registry,
                &BTreeSet::from([tag.clone()]),
            )
            .map(Arc::new)
        })
        .collect::<Result<Vec<_>>>()?;
    let loot = Arc::new(native_resources::loot::BlockLootTables::default());
    let mut checked = 0;
    for row in rows {
        let fields: Vec<_> = row
            .strip_prefix("K ")
            .context("invalid Java result")?
            .split_whitespace()
            .collect();
        ensure!(fields.len() == 14, "invalid Java row {row}");
        if fields[1] != "0" {
            continue;
        }
        let plant = if fields[0] == "0" {
            state("kelp_plant")
        } else {
            head()
        };
        let direction = match fields[3] {
            "down" => [0, -1, 0],
            "up" => [0, 1, 0],
            "north" => [0, 0, -1],
            "south" => [0, 0, 1],
            "west" => [-1, 0, 0],
            "east" => [1, 0, 0],
            _ => bail!("invalid direction"),
        };
        let below = support(fields[4].parse()?);
        let upper = above(fields[5].parse()?);
        let neighbor = neighbor(fields[6].parse()?);
        let mask: usize = fields[7].parse()?;
        let seed: i64 = fields[2].parse()?;
        let (mut stage, _, chunk) = super::tests::fixture(false);
        stage.aquatic_tags = Some(Arc::clone(&all_tags[mask]));
        stage.block_drops = Some(
            super::super::postprocessing_drops::GeneratedBlockDrops::new(
                Arc::clone(&loot),
                42,
                native_persistence::random_sequences::RandomSequences::read(None)?,
                LegacyRandom::new(seed),
                XoroshiroRandom::new(77),
                true,
            ),
        );
        stage.region.ticks = RegionTicks::new(
            BTreeMap::from([(
                chunk,
                ConvertedChunkTicks {
                    blocks: ChunkTicks::from_saved(chunk, vec![])?,
                    fluids: ChunkTicks::from_saved(chunk, vec![])?,
                },
            )]),
            100,
            0,
        )?;
        let world = Arc::make_mut(&mut stage.world);
        for (pos, value) in [
            ([8, 7, 8], below),
            ([8, 9, 8], upper),
            ([8, 8, 8], plant.clone()),
        ] {
            let id = world.intern_state(value);
            world.set_block_id(pos[0], pos[1], pos[2], id)?;
        }
        let (first, second) = update_twice(&mut stage, chunk, &plant, direction, &neighbor)?;
        assert_eq!(describe(&first), fields[8], "{row}");
        assert_eq!(describe(&second), fields[9], "{row}");
        let ticks = stage.region.ticks.chunk(chunk).context("missing ticks")?;
        let blocks = ticks.blocks.pack(100);
        let fluids = ticks.fluids.pack(100);
        assert_eq!(blocks.len(), fields[10].parse::<usize>()?, "{row}");
        assert_eq!(fluids.len(), fields[11].parse::<usize>()?, "{row}");
        if let Some(tick) = blocks.first() {
            assert_eq!(
                (&*tick.kind, tick.position, tick.delay, tick.priority),
                (&*plant.name, [8, 8, 8], 1, 0),
                "{row}"
            );
        }
        if let Some(tick) = fluids.first() {
            assert_eq!(
                (&*tick.kind, tick.position, tick.delay, tick.priority),
                ("minecraft:water", [8, 8, 8], 5, 0),
                "{row}"
            );
        }
        let requests = if fields[12] == "-" {
            0
        } else {
            fields[12].len() as i64
        };
        assert_eq!(stage.region.ticks.next_sub_tick(), requests, "{row}");
        assert_eq!(
            stage
                .block_drops
                .as_ref()
                .unwrap()
                .level_random()
                .clone()
                .next_int(),
            fields[13].parse::<i32>()?,
            "{row}"
        );
        checked += 1;
    }
    assert_eq!(checked, 1424);
    Ok(())
}
