use super::*;
use native_resources::tree_leaf_update::ShapeEdgeWorld;
use std::{path::PathBuf, process::Command};

struct Tags(u8);

impl ResourceProvider for Tags {
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
            &serde_json::json!({"values": values}),
        )?))
    }
}

fn state(name: &str) -> BlockState {
    BlockState {
        name: format!("minecraft:{name}"),
        properties: BTreeMap::new(),
    }
}

fn support(index: usize) -> BlockState {
    let mut block = match index {
        0 => state("stone"),
        1 => state("magma_block"),
        2 => state("air"),
        3 => state("water"),
        4..=6 => state("stone_slab"),
        7..=8 => state("stone_brick_stairs"),
        _ => panic!("unknown support"),
    };
    match index {
        3 => {
            block.properties.insert("level".into(), "0".into());
        }
        4..=6 => {
            block.properties = BTreeMap::from([
                ("type".into(), ["bottom", "top", "double"][index - 4].into()),
                ("waterlogged".into(), "false".into()),
            ]);
        }
        7..=8 => {
            block.properties = BTreeMap::from([
                ("facing".into(), "north".into()),
                (
                    "half".into(),
                    if index == 7 { "top" } else { "bottom" }.into(),
                ),
                ("shape".into(), "straight".into()),
                ("waterlogged".into(), "false".into()),
            ]);
        }
        _ => {}
    }
    block
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn seagrass_edge_updates_match_java() -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let _verified = native_resources::packs::ResourceStack::open(&game)?;
    let java = std::env::var_os("MINECRAFT_JAVA_HOME")
        .or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join(
                "AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon",
            )
        })
        .join("bin");
    let temp = tempfile::tempdir()?;
    let version = game.join("versions/26.3");
    let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    ensure!(manifest["id"] == "26.3", "wrong Minecraft version");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for library in manifest["libraries"]
        .as_array()
        .context("libraries missing")?
    {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let classpath = std::env::join_paths(paths)?;
    let source = temp.path().join("SeagrassEdgeOracle.java");
    std::fs::write(
        &source,
        include_str!("../../../tests/java/SeagrassEdgeOracle.java"),
    )?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&classpath)
        .arg(source)
        .output()?;
    ensure!(
        compiled.status.success(),
        "Java compile failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(classpath)
        .arg("SeagrassEdgeOracle")
        .output()?;
    ensure!(
        output.status.success(),
        "Java oracle failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let mut registry = tests::batch_registry();
    registry.extend([
        "minecraft:water".into(),
        "minecraft:magma_block".into(),
        "minecraft:seagrass".into(),
        "minecraft:stone_slab".into(),
        "minecraft:stone_brick_stairs".into(),
    ]);
    let tag = ResourceId::parse("minecraft:cannot_support_seagrass")?;
    let plant = state("seagrass");
    let mut count = 0;
    for line in String::from_utf8(output.stdout)?
        .lines()
        .filter_map(|v| v.strip_prefix("E "))
    {
        let fields: Vec<_> = line.split_whitespace().collect();
        ensure!(fields.len() == 9, "unexpected oracle row {line}");
        let mask: u8 = fields[0].parse()?;
        let block = support(fields[1].parse()?);
        let neighbor = match fields[2] {
            "0" => state("air"),
            "1" => state("stone"),
            "2" => support(3),
            _ => bail!("invalid neighbor"),
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
        let tags = OreBlockTags::load(&mut Tags(mask), &registry, &BTreeSet::from([tag.clone()]))?;
        let (mut terrain, _, motion, _) = tests::fixture();
        let center = ChunkPos { x: 0, z: 0 };
        let mut region = terrain.decoration_region(&tags, &motion, BTreeSet::from([center]))?;
        region.set_section_block([8, 7, 8], &block)?;
        region.set_section_block([8, 8, 8], &plant)?;
        let pos = [8, 8, 8];
        let neighbor_pos = std::array::from_fn(|i| pos[i] + direction[i]);
        for _ in 0..2 {
            let updated = ShapeEdgeWorld::update_shape(
                &mut region,
                pos,
                &plant,
                direction,
                neighbor_pos,
                &neighbor,
            )?;
            assert_eq!(updated.name, fields[4], "{line}");
            assert!(updated.properties.is_empty(), "{line}");
        }
        let ticks = region
            .world
            .saved_fluid_ticks
            .get(&center)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        assert_eq!(ticks.len(), fields[6].parse::<usize>()?, "{line}");
        assert_eq!(fields[5].parse::<usize>()?, ticks.len() * 2, "{line}");
        if let Some(tick) = ticks.first() {
            assert_eq!(tick.fluid, "minecraft:water", "{line}");
            assert_eq!(tick.position, pos, "{line}");
            assert_eq!(tick.delay, fields[7].parse::<i32>()?, "{line}");
            assert_eq!(tick.priority, fields[8].parse::<i32>()?, "{line}");
        }
        assert!(region.world.saved_block_ticks.is_empty(), "{line}");
        count += 1;
    }
    assert_eq!(count, 4 * 9 * 3 * 6);
    Ok(())
}
