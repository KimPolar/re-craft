use super::*;
use native_resources::tree_leaf_update::ShapeEdgeWorld;
use std::{path::PathBuf, process::Command};

fn chest(facing: &str, waterlogged: &str) -> BlockState {
    BlockState {
        name: "minecraft:chest".into(),
        properties: BTreeMap::from([
            ("facing".into(), facing.into()),
            ("type".into(), "single".into()),
            ("waterlogged".into(), waterlogged.into()),
        ]),
    }
}

#[test]
fn unsupported_tree_edge_chest_states_are_rejected() -> Result<()> {
    let (mut terrain, tags, motion, _) = tests::fixture();
    let center = ChunkPos { x: 0, z: 0 };
    let mut region = terrain.decoration_region(&tags, &motion, BTreeSet::from([center]))?;
    let pos = [8, 8, 8];
    let single = chest("north", "true");
    let mut double = single.clone();
    double.properties.insert("type".into(), "left".into());
    let air = BlockState {
        name: "minecraft:air".into(),
        properties: BTreeMap::new(),
    };
    assert!(
        ShapeEdgeWorld::update_shape(&mut region, pos, &double, [1, 0, 0], [9, 8, 8], &air)
            .is_err()
    );
    assert!(
        ShapeEdgeWorld::update_shape(&mut region, pos, &single, [1, 0, 0], [9, 8, 8], &single)
            .is_err()
    );
    assert!(region.world.saved_fluid_ticks.is_empty());
    Ok(())
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn single_chest_edge_updates_match_java() -> Result<()> {
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
    let source = temp.path().join("ChestEdgeOracle.java");
    std::fs::write(
        &source,
        include_str!("../../../tests/java/ChestEdgeOracle.java"),
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
        .arg("ChestEdgeOracle")
        .output()?;
    ensure!(
        output.status.success(),
        "Java oracle failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let mut count = 0;
    for line in String::from_utf8(output.stdout)?
        .lines()
        .filter_map(|v| v.strip_prefix("E "))
    {
        let fields: Vec<_> = line.split_whitespace().collect();
        ensure!(fields.len() == 8, "unexpected oracle row {line}");
        let chest_state = chest(fields[0], fields[1]);
        let direction = match fields[2] {
            "down" => [0, -1, 0],
            "up" => [0, 1, 0],
            "north" => [0, 0, -1],
            "south" => [0, 0, 1],
            "west" => [-1, 0, 0],
            "east" => [1, 0, 0],
            _ => bail!("invalid direction"),
        };
        let mut neighbor = match fields[3] {
            "0" => BlockState {
                name: "minecraft:air".into(),
                properties: BTreeMap::new(),
            },
            "1" => BlockState {
                name: "minecraft:stone".into(),
                properties: BTreeMap::new(),
            },
            "2" => BlockState {
                name: "minecraft:water".into(),
                properties: BTreeMap::from([("level".into(), "0".into())]),
            },
            "3" | "4" => chest("north", "false"),
            _ => bail!("invalid neighbor"),
        };
        if fields[3] == "3" {
            neighbor.name = "minecraft:trapped_chest".into();
        }
        let (mut terrain, tags, motion, _) = tests::fixture();
        let center = ChunkPos { x: 0, z: 0 };
        let mut region = terrain.decoration_region(&tags, &motion, BTreeSet::from([center]))?;
        let pos = [8, 8, 8];
        let neighbor_pos = std::array::from_fn(|i| pos[i] + direction[i]);
        for _ in 0..2 {
            assert_eq!(
                ShapeEdgeWorld::update_shape(
                    &mut region,
                    pos,
                    &chest_state,
                    direction,
                    neighbor_pos,
                    &neighbor
                )?,
                chest_state,
                "{line}"
            );
        }
        let ticks = region
            .world
            .saved_fluid_ticks
            .get(&center)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        assert_eq!(ticks.len(), fields[5].parse::<usize>()?, "{line}");
        assert_eq!(fields[4].parse::<usize>()?, ticks.len() * 2, "{line}");
        if let Some(tick) = ticks.first() {
            assert_eq!(tick.fluid, "minecraft:water", "{line}");
            assert_eq!(tick.position, pos, "{line}");
            assert_eq!(tick.delay, fields[6].parse::<i32>()?, "{line}");
            assert_eq!(tick.priority, fields[7].parse::<i32>()?, "{line}");
        }
        assert!(region.world.saved_block_ticks.is_empty(), "{line}");
        count += 1;
    }
    assert_eq!(count, 4 * 2 * (6 * 4 + 2));
    Ok(())
}
