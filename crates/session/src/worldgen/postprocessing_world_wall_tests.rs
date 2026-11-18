use super::*;
use crate::worldgen::chunk_ticks::{ChunkTicks, ConvertedChunkTicks};
use native_core::random::{LegacyRandom, XoroshiroRandom};
use native_resources::{
    ore_feature::OreBlockTags,
    packs::{ResourceId, ResourceProvider},
};

struct WallTags(u8);

impl ResourceProvider for WallTags {
    fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }

    fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        let mut values = Vec::new();
        match path {
            "data/minecraft/tags/block/walls.json" => {
                if self.0 & 1 != 0 {
                    values.extend(["minecraft:stone_brick_wall", "minecraft:stone"]);
                }
                if self.0 & 2 != 0 {
                    values.push("minecraft:water");
                }
            }
            "data/minecraft/tags/block/wall_post_override.json" => {
                if self.0 & 1 != 0 {
                    values.push("minecraft:air");
                }
            }
            "data/minecraft/tags/block/shulker_boxes.json" => {
                if self.0 & 2 != 0 {
                    values.push("minecraft:stone");
                }
            }
            _ => return Ok(None),
        }
        Ok(Some(serde_json::to_vec(
            &serde_json::json!({ "values": values }),
        )?))
    }
}

fn state(name: &str, properties: &[(&str, &str)]) -> BlockState {
    BlockState {
        name: format!("minecraft:{name}"),
        properties: properties
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect(),
    }
}

fn wall(mut code: usize, up: bool, wet: bool) -> BlockState {
    let sides = ["none", "low", "tall"];
    let mut result = state(
        "stone_brick_wall",
        &[
            ("up", if up { "true" } else { "false" }),
            ("waterlogged", if wet { "true" } else { "false" }),
        ],
    );
    for key in ["north", "east", "south", "west"] {
        result.properties.insert(key.into(), sides[code % 3].into());
        code /= 3;
    }
    result
}

fn neighbor(index: usize) -> BlockState {
    let bars = |name| {
        state(
            name,
            &[
                ("north", "false"),
                ("east", "false"),
                ("south", "false"),
                ("west", "false"),
                ("waterlogged", "false"),
            ],
        )
    };
    match index {
        0 => state("air", &[]),
        1 => state("stone", &[]),
        2 => state("water", &[("level", "0")]),
        3 => wall(0, true, false),
        4 => bars("iron_bars"),
        5 => bars("oak_fence"),
        6 | 7 => state(
            "oak_fence_gate",
            &[
                ("facing", if index == 6 { "north" } else { "east" }),
                ("in_wall", "false"),
                ("open", "false"),
                ("powered", "false"),
            ],
        ),
        8 | 9 => state(
            "stone_slab",
            &[
                ("type", if index == 8 { "bottom" } else { "top" }),
                ("waterlogged", "false"),
            ],
        ),
        10 => state(
            "oak_leaves",
            &[
                ("distance", "7"),
                ("persistent", "false"),
                ("waterlogged", "false"),
            ],
        ),
        11 => state("melon", &[]),
        12 => bars("glass_pane"),
        13 => state(
            "stone_brick_stairs",
            &[
                ("facing", "north"),
                ("half", "bottom"),
                ("shape", "straight"),
                ("waterlogged", "false"),
            ],
        ),
        14 => state("barrier", &[]),
        _ => panic!("invalid neighbor"),
    }
}

fn above(index: usize) -> BlockState {
    match index {
        0 => neighbor(0),
        1 => neighbor(1),
        2 => neighbor(8),
        3 => neighbor(9),
        4 => wall(0, true, false),
        5 => wall(40, false, false),
        6 => neighbor(13),
        _ => panic!("invalid above block"),
    }
}

fn tags(mask: u8) -> Result<Arc<OreBlockTags>> {
    let registry = (0..15)
        .map(neighbor)
        .chain((0..7).map(above))
        .map(|state| state.name)
        .collect();
    let names = [
        "minecraft:walls",
        "minecraft:wall_post_override",
        "minecraft:shulker_boxes",
    ]
    .into_iter()
    .map(ResourceId::parse)
    .collect::<Result<BTreeSet<_>>>()?;
    Ok(Arc::new(OreBlockTags::load(
        &mut WallTags(mask),
        &registry,
        &names,
    )?))
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn wall_fluid_faces_match_java() {
    let rows = super::tests::java_results(
        "WallFluidShapeOracle",
        include_str!("../../tests/java/WallFluidShapeOracle.java"),
    );
    assert_eq!(rows.first().map(String::as_str), Some("checked 2503872"));
    let mut materials = BTreeSet::new();
    for row in &rows[1..] {
        let name = row.strip_prefix("material ").expect("wall material result");
        let mut sample = wall(0, true, true);
        sample.name = name.into();
        assert!(
            native_world::wall::Wall::from_state(&sample).is_some(),
            "{name}"
        );
        assert!(
            materials.insert(name.to_owned()),
            "duplicate material {name}"
        );
    }
    assert!(materials.len() >= 30, "wall registry was incomplete");
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn generated_wall_shape_callbacks_match_java() -> Result<()> {
    use native_world::wall::{Side, Wall};
    let rows = super::tests::java_results(
        "WallPostprocessingShapeOracle",
        include_str!("../../tests/java/WallPostprocessingShapeOracle.java"),
    );
    let all_tags = (0..4).map(tags).collect::<Result<Vec<_>>>()?;
    let (mut stage, source, chunk) = super::tests::fixture(false);
    drop(source);
    stage.block_drops = Some(
        super::super::postprocessing_drops::GeneratedBlockDrops::new(
            Arc::new(native_resources::loot::BlockLootTables::default()),
            42,
            native_persistence::random_sequences::RandomSequences::read(None)?,
            LegacyRandom::new(17),
            XoroshiroRandom::new(77),
            true,
        ),
    );
    let mut checked = 0;
    for row in rows {
        let fields: Vec<_> = row
            .strip_prefix("W ")
            .context("invalid Java result")?
            .split_whitespace()
            .collect();
        ensure!(fields.len() == 12, "invalid Java row {row}");
        let code: usize = fields[0].parse()?;
        let original = wall(code, fields[1] == "1", fields[2] == "1");
        let direction = match fields[3] {
            "down" => [0, -1, 0],
            "up" => [0, 1, 0],
            "north" => [0, 0, -1],
            "east" => [1, 0, 0],
            "south" => [0, 0, 1],
            "west" => [-1, 0, 0],
            _ => bail!("invalid direction"),
        };
        let neighbor = neighbor(fields[4].parse()?);
        let top = above(fields[5].parse()?);
        let mask: usize = fields[6].parse()?;
        stage.wall_tags = Some(Arc::clone(&all_tags[mask]));
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
        let id = world.intern_state(top);
        world.set_block_id(8, 9, 8, id)?;
        let (first, second) = super::kelp_shape_tests::update_twice(
            &mut stage, chunk, &original, direction, &neighbor,
        )
        .with_context(|| format!("Java row {row}"))?;
        assert_eq!(first, second, "{row}");
        let result = Wall::from_state(&first).context("invalid resulting wall")?;
        let actual_code: usize = result
            .shape
            .sides
            .into_iter()
            .enumerate()
            .map(|(i, side)| {
                let value = match side {
                    Side::None => 0,
                    Side::Low => 1,
                    Side::Tall => 2,
                };
                value * 3_usize.pow(i as u32)
            })
            .sum();
        assert_eq!(actual_code, fields[7].parse::<usize>()?, "{row}");
        assert_eq!(result.shape.up, fields[8] == "true", "{row}");
        assert_eq!(result.waterlogged, fields[2] == "1", "{row}");
        assert_eq!(
            stage.region.ticks.next_sub_tick(),
            fields[9].parse::<i64>()?,
            "{row}"
        );
        let fluids = stage
            .region
            .ticks
            .chunk(chunk)
            .context("missing ticks")?
            .fluids
            .pack(100);
        assert_eq!(fluids.len(), fields[10].parse::<usize>()?, "{row}");
        if let Some(tick) = fluids.first() {
            assert_eq!(
                (&*tick.kind, tick.position, tick.delay, tick.priority),
                ("minecraft:water", [8, 8, 8], 5, 0),
                "{row}"
            );
        }
        assert_eq!(
            stage
                .block_drops
                .as_ref()
                .unwrap()
                .level_random()
                .clone()
                .next_int(),
            fields[11].parse::<i32>()?,
            "{row}"
        );
        checked += 1;
    }
    assert_eq!(checked, 21708);
    Ok(())
}
