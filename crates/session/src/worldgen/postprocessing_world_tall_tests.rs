use super::*;
use crate::worldgen::chunk_ticks::{ChunkTicks, ConvertedChunkTicks};
use native_core::random::{LegacyRandom, XoroshiroRandom};
use native_resources::{
    ore_feature::OreBlockTags,
    packs::{ResourceId, ResourceProvider},
};

struct SeagrassTags(u8);

impl ResourceProvider for SeagrassTags {
    fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }

    fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        if path != "data/minecraft/tags/block/cannot_support_seagrass.json" {
            return Ok(None);
        }
        let mut values = Vec::new();
        if self.0 & 1 != 0 {
            values.push("minecraft:stone");
        }
        if self.0 & 2 != 0 {
            values.push("minecraft:magma_block");
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

fn tall(half: &str) -> BlockState {
    BlockState {
        name: "minecraft:tall_seagrass".into(),
        properties: BTreeMap::from([("half".into(), half.into())]),
    }
}

fn liquid(name: &str, level: &str) -> BlockState {
    BlockState {
        name: format!("minecraft:{name}"),
        properties: BTreeMap::from([("level".into(), level.into())]),
    }
}

fn support(index: usize) -> BlockState {
    match index {
        0 => state("stone"),
        1 => state("magma_block"),
        2 => state("air"),
        3 => liquid("water", "0"),
        4 | 5 => BlockState {
            name: "minecraft:stone_slab".into(),
            properties: BTreeMap::from([
                (
                    "type".into(),
                    if index == 4 { "bottom" } else { "top" }.into(),
                ),
                ("waterlogged".into(), "false".into()),
            ]),
        },
        6 => tall("lower"),
        7 => tall("upper"),
        8 => state("kelp_plant"),
        9 => BlockState {
            name: "minecraft:suspicious_gravel".into(),
            properties: BTreeMap::from([("dusted".into(), "0".into())]),
        },
        _ => panic!("invalid support"),
    }
}

fn live(index: usize) -> BlockState {
    match index {
        0 => tall("lower"),
        1 => liquid("water", "0"),
        2 => liquid("water", "1"),
        3 => liquid("water", "8"),
        4 => liquid("lava", "0"),
        5 => state("air"),
        6 => tall("upper"),
        _ => panic!("invalid live state"),
    }
}

fn neighbor(index: usize) -> BlockState {
    match index {
        0 => state("air"),
        1 => state("stone"),
        2 => tall("lower"),
        3 => tall("upper"),
        _ => panic!("invalid neighbor"),
    }
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn level_tall_seagrass_shape_callbacks_match_java() -> Result<()> {
    let rows = super::tests::java_results(
        "TallSeagrassEdgeOracle",
        include_str!("../../tests/java/TallSeagrassEdgeOracle.java"),
    );
    let registry: BTreeSet<_> = [
        "air",
        "stone",
        "magma_block",
        "water",
        "lava",
        "stone_slab",
        "kelp_plant",
        "tall_seagrass",
        "suspicious_gravel",
    ]
    .into_iter()
    .map(|name| format!("minecraft:{name}"))
    .collect();
    let tag = ResourceId::parse("minecraft:cannot_support_seagrass")?;
    let tags = (0..4)
        .map(|mask| {
            OreBlockTags::load(
                &mut SeagrassTags(mask),
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
            .strip_prefix("T ")
            .context("invalid Java result")?
            .split_whitespace()
            .collect();
        ensure!(fields.len() == 10, "invalid Java row {row}");
        let plant = tall(if fields[0] == "0" { "lower" } else { "upper" });
        let direction = match fields[1] {
            "down" => [0, -1, 0],
            "up" => [0, 1, 0],
            "north" => [0, 0, -1],
            "south" => [0, 0, 1],
            "west" => [-1, 0, 0],
            "east" => [1, 0, 0],
            _ => bail!("invalid direction"),
        };
        let below = support(fields[2].parse()?);
        let actual = live(fields[3].parse()?);
        let neighbor = neighbor(fields[4].parse()?);
        let mask: usize = fields[5].parse()?;
        let fluid_mask: u8 = fields[6].parse()?;
        let (mut stage, _, chunk) = super::tests::fixture(false);
        stage.aquatic_tags = Some(Arc::clone(&tags[mask]));
        let fluids: BTreeSet<_> = [
            (1, "minecraft:water"),
            (2, "minecraft:flowing_water"),
            (4, "minecraft:lava"),
            (8, "minecraft:flowing_lava"),
        ]
        .into_iter()
        .filter_map(|(bit, name)| (fluid_mask & bit != 0).then(|| name.to_owned()))
        .collect();
        stage.water_fluids = Some(Arc::new(fluids));
        stage.block_drops = Some(
            super::super::postprocessing_drops::GeneratedBlockDrops::new(
                Arc::clone(&loot),
                42,
                native_persistence::random_sequences::RandomSequences::read(None)?,
                LegacyRandom::new(17),
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
        for (pos, value) in [([8, 7, 8], below), ([8, 8, 8], actual)] {
            let id = world.intern_state(value);
            world.set_block_id(pos[0], pos[1], pos[2], id)?;
        }
        let (first, second) =
            super::kelp_shape_tests::update_twice(&mut stage, chunk, &plant, direction, &neighbor)
                .with_context(|| format!("Java row {row}"))?;
        assert_eq!(first.name, fields[7], "{row}");
        assert_eq!(second.name, fields[8], "{row}");
        if first.name == "minecraft:tall_seagrass" {
            assert_eq!(first, plant, "{row}");
        }
        if second.name == "minecraft:tall_seagrass" {
            assert_eq!(second, plant, "{row}");
        }
        assert_eq!(stage.region.ticks.next_sub_tick(), 0, "{row}");
        assert_eq!(
            stage
                .block_drops
                .as_ref()
                .unwrap()
                .level_random()
                .clone()
                .next_int(),
            fields[9].parse::<i32>()?,
            "{row}"
        );
        checked += 1;
    }
    assert_eq!(checked, 12960);
    Ok(())
}
