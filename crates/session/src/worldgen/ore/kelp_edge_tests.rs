use super::*;
use native_core::random::{LegacyRandom, XoroshiroRandom};
use native_resources::worldgen_region_random::RegionRandom;
use std::{path::PathBuf, process::Command};

struct Tags(u8);
impl ResourceProvider for Tags {
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
            &serde_json::json!({"values": values}),
        )?))
    }
}

fn plain(id: &str) -> BlockState {
    BlockState {
        name: format!("minecraft:{id}"),
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
        0 => plain("stone"),
        1 => BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "0".into())]),
        },
        2 => plain("air"),
        3 => head(),
        4 => plain("kelp_plant"),
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
        0 => plain("air"),
        1 => head(),
        2 => plain("kelp_plant"),
        _ => panic!("invalid above"),
    }
}
fn neighbor(index: usize) -> BlockState {
    match index {
        0 => plain("air"),
        1 => plain("stone"),
        2 => head(),
        3 => plain("kelp_plant"),
        _ => panic!("invalid neighbor"),
    }
}
fn tags(mask: u8) -> Result<OreBlockTags> {
    let mut registry = tests::batch_registry();
    registry.extend([
        "minecraft:kelp".into(),
        "minecraft:kelp_plant".into(),
        "minecraft:water".into(),
        "minecraft:stone_slab".into(),
    ]);
    OreBlockTags::load(
        &mut Tags(mask),
        &registry,
        &BTreeSet::from([kelp_forbidden_tag().clone()]),
    )
}
fn describe(state: &BlockState) -> String {
    format!(
        "{}:{}",
        state.name,
        state
            .properties
            .get("age")
            .map(String::as_str)
            .unwrap_or("-")
    )
}

#[test]
fn kelp_edge_uses_callback_block_for_ticks_and_requires_random_for_conversion() -> Result<()> {
    let (mut terrain, _, motion, _) = tests::fixture();
    let tags = tags(0)?;
    let center = ChunkPos { x: 0, z: 0 };
    let mut region = terrain.decoration_region(&tags, &motion, BTreeSet::from([center]))?;
    let pos = [8, 8, 8];
    region.set_section_block(pos, &plain("kelp_plant"))?;
    region.set_section_block([8, 7, 8], &plain("air"))?;
    region.update_generated_kelp_shape(pos, &head(), [0, -1, 0], &plain("air"), None)?;
    let ticks = &region.world.saved_block_ticks[&center];
    assert_eq!(ticks.len(), 1);
    assert_eq!(ticks[0].block, "minecraft:kelp");
    let before = region.world.saved_fluid_ticks.clone();
    assert!(
        region
            .update_generated_kelp_shape(pos, &plain("kelp_plant"), [0, 1, 0], &plain("air"), None)
            .is_err()
    );
    assert_eq!(region.world.saved_fluid_ticks, before);
    Ok(())
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn kelp_edges_match_java_state_ticks_and_region_random() -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let _verified = native_resources::packs::ResourceStack::open(&game)?;
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join(
            "AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"
        )).join("bin");
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
    let source = temp.path().join("KelpEdgeOracle.java");
    std::fs::write(
        &source,
        include_str!("../../../tests/java/KelpEdgeOracle.java"),
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
        .arg("KelpEdgeOracle")
        .output()?;
    ensure!(
        output.status.success(),
        "Java oracle failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let all_tags = (0..4).map(tags).collect::<Result<Vec<_>>>()?;
    let mut count = 0;
    for line in String::from_utf8(output.stdout)?
        .lines()
        .filter_map(|v| v.strip_prefix("K "))
    {
        let fields: Vec<_> = line.split_whitespace().collect();
        ensure!(fields.len() == 14, "unexpected oracle row {line}");
        let state = if fields[0] == "0" {
            plain("kelp_plant")
        } else {
            head()
        };
        let seed = fields[2].parse()?;
        let mut random = match fields[1] {
            "0" => RegionRandom::Legacy(LegacyRandom::new(seed)),
            "1" => RegionRandom::Xoroshiro(XoroshiroRandom::new(seed)),
            _ => bail!("invalid random algorithm"),
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
        let above = above(fields[5].parse()?);
        let neighbor = neighbor(fields[6].parse()?);
        let (mut terrain, _, motion, _) = tests::fixture();
        let center = ChunkPos { x: 0, z: 0 };
        let mut region = terrain.decoration_region(
            &all_tags[fields[7].parse::<usize>()?],
            &motion,
            BTreeSet::from([center]),
        )?;
        let pos = [8, 8, 8];
        region.set_section_block([8, 7, 8], &below)?;
        region.set_section_block([8, 9, 8], &above)?;
        region.set_section_block(pos, &state)?;
        let first = region.update_generated_kelp_shape(
            pos,
            &state,
            direction,
            &neighbor,
            Some(&mut random),
        )?;
        let second = region.update_generated_kelp_shape(
            pos,
            &state,
            direction,
            &neighbor,
            Some(&mut random),
        )?;
        assert_eq!(describe(&first), fields[8], "{line}");
        assert_eq!(describe(&second), fields[9], "{line}");
        let blocks = region
            .world
            .saved_block_ticks
            .get(&center)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let fluids = region
            .world
            .saved_fluid_ticks
            .get(&center)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        assert_eq!(blocks.len(), fields[10].parse::<usize>()?, "{line}");
        assert_eq!(fluids.len(), fields[11].parse::<usize>()?, "{line}");
        if let Some(tick) = blocks.first() {
            assert_eq!(tick.block, state.name, "{line}");
            assert_eq!(
                (tick.position, tick.delay, tick.priority),
                (pos, 0, 0),
                "{line}"
            );
        }
        if let Some(tick) = fluids.first() {
            assert_eq!(tick.fluid, "minecraft:water", "{line}");
            assert_eq!(
                (tick.position, tick.delay, tick.priority),
                (pos, 0, 0),
                "{line}"
            );
        }
        let requests = match (blocks.len(), fluids.len()) {
            (0, 0) => "-",
            (0, 1) => "ff",
            (1, 1) => "bfbf",
            _ => bail!("unexpected tick combination"),
        };
        assert_eq!(requests, fields[12], "{line}");
        assert_eq!(random.next_int(), fields[13].parse::<i32>()?, "{line}");
        count += 1;
    }
    assert_eq!(count, 2848);
    Ok(())
}
