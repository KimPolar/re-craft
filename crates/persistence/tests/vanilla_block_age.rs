use anyhow::{Context, Result, ensure};
use native_persistence::{
    jigsaw_replacement::PlacementBlock, nbt::Tag, structure_processors::PreparedProcessors,
};
use native_resources::{packs::ResourceStack, structure_pool::HolderValue};
use native_world::loaded::BlockState;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    process::Command,
};

fn parse_state(value: &str) -> Result<BlockState> {
    let (name, properties) = value.split_once('|').context("state separator missing")?;
    let mut result = BTreeMap::new();
    for entry in properties.split(';').filter(|entry| !entry.is_empty()) {
        let (name, value) = entry
            .split_once('=')
            .context("state property missing value")?;
        ensure!(
            result.insert(name.into(), value.into()).is_none(),
            "duplicate state property"
        );
    }
    Ok(BlockState {
        name: name.into(),
        properties: result,
    })
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn block_age_processor_matches_java() -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
        .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let temp = tempfile::tempdir()?;
    let version = game.join("versions/26.3");
    let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    ensure!(manifest["id"] == "26.3", "wrong version");
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
    let source = temp.path().join("BlockAgeProcessorOracle.java");
    std::fs::write(&source, include_str!("java/BlockAgeProcessorOracle.java"))?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(source)
        .output()?;
    ensure!(
        compiled.status.success(),
        "Java compilation failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(classpath)
        .arg("BlockAgeProcessorOracle")
        .output()?;
    ensure!(
        output.status.success(),
        "Java oracle failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let mut resources = ResourceStack::open(&game)?;
    let registry = resources.vanilla_generated_block_registry()?;
    let blocks = resources.vanilla_block_ids()?;
    let mut processors = Vec::new();
    for mossiness in [0.0_f32, 0.2, 0.8, 1.0] {
        let config = HolderValue::Inline(
            json!({"processors":[{"processor_type":"minecraft:block_age","mossiness":mossiness}]}),
        );
        processors.push(PreparedProcessors::load(
            &config,
            &mut resources,
            &blocks,
            &registry,
        )?);
    }
    let marker = Tag::Compound(vec![("test".into(), Tag::Int(17))]);
    let text = String::from_utf8(output.stdout)?;
    let mut count = 0;
    for line in text.lines().filter(|line| line.starts_with("A\t")) {
        let fields: Vec<_> = line.split('\t').collect();
        ensure!(fields.len() == 7, "invalid Java output");
        let mode: usize = fields[1].parse()?;
        let position = [fields[2].parse()?, fields[3].parse()?, fields[4].parse()?];
        let block = PlacementBlock {
            position,
            state: parse_state(fields[5])?,
            nbt: Some(marker.clone()),
        };
        let output = processors[mode]
            .process(block, &mut |_| {
                anyhow::bail!("age processor requested world state")
            })?
            .context("age processor unexpectedly removed block")?;
        assert_eq!(output.position, position, "{line}");
        assert_eq!(output.nbt, Some(marker.clone()), "{line}");
        assert_eq!(output.state, parse_state(fields[6])?, "{line}");
        count += 1;
    }
    assert_eq!(count, 26_052, "unexpected Java case count");
    let mut tagged_count = 0;
    for line in text.lines().filter(|line| line.starts_with("B\t")) {
        let fields: Vec<_> = line.split('\t').collect();
        ensure!(fields.len() == 8, "invalid Java tagged-block output");
        let category: usize = fields[1].parse()?;
        let mode: usize = fields[2].parse()?;
        let x = fields[3].parse()?;
        let y = fields[4].parse()?;
        let z = fields[5].parse()?;
        let state = parse_state(fields[6])?;
        let member = BTreeSet::from([state.name.clone()]);
        let empty = BTreeSet::new();
        let tags = match category {
            0 => {
                native_persistence::structure_block_age::AgeTags::new(member, empty.clone(), empty)
            }
            1 => {
                native_persistence::structure_block_age::AgeTags::new(empty.clone(), member, empty)
            }
            2 => {
                native_persistence::structure_block_age::AgeTags::new(empty.clone(), empty, member)
            }
            _ => anyhow::bail!("invalid category"),
        };
        let mut random = native_core::random::LegacyRandom::new(
            native_resources::blockstates::positional_seed(x, y, z),
        );
        let actual = tags.process(&state, [0.0_f32, 0.2, 0.8, 1.0][mode], &mut random)?;
        assert_eq!(actual, parse_state(fields[7])?, "{line}");
        tagged_count += 1;
    }
    assert_eq!(tagged_count, 3_588, "unexpected tagged-block case count");

    let mut waterlogged_count = 0;
    let mut waterlogged_replacements = [0_usize; 3];
    for line in text.lines().filter(|line| line.starts_with("C\t")) {
        let fields: Vec<_> = line.split('\t').collect();
        ensure!(fields.len() == 8, "invalid waterlogged-block output");
        let category: usize = fields[1].parse()?;
        let mode: usize = fields[2].parse()?;
        let position = [fields[3].parse()?, fields[4].parse()?, fields[5].parse()?];
        let state = parse_state(fields[6])?;
        let member = BTreeSet::from([state.name.clone()]);
        let empty = BTreeSet::new();
        let tags = match category {
            0 => {
                native_persistence::structure_block_age::AgeTags::new(member, empty.clone(), empty)
            }
            1 => {
                native_persistence::structure_block_age::AgeTags::new(empty.clone(), member, empty)
            }
            2 => {
                native_persistence::structure_block_age::AgeTags::new(empty.clone(), empty, member)
            }
            _ => anyhow::bail!("invalid category"),
        };
        let mut random = native_core::random::LegacyRandom::new(
            native_resources::blockstates::positional_seed(position[0], position[1], position[2]),
        );
        let actual = tags.process(&state, [0.0_f32, 0.2, 0.8, 1.0][mode], &mut random)?;
        assert_eq!(actual, parse_state(fields[7])?, "{line}");
        if actual.name != state.name
            && state
                .properties
                .get("waterlogged")
                .is_some_and(|v| v == "true")
            && actual
                .properties
                .get("waterlogged")
                .is_some_and(|v| v == "true")
        {
            waterlogged_replacements[category] += 1;
        }
        waterlogged_count += 1;
    }
    assert!(
        waterlogged_count > 1_000,
        "too few waterlogged cases: {waterlogged_count}"
    );
    assert!(waterlogged_replacements.into_iter().all(|count| count > 0));

    let mut mixed_count = [0_usize; 3];
    for line in text.lines().filter(|line| line.starts_with("D\t")) {
        let fields: Vec<_> = line.split('\t').collect();
        ensure!(fields.len() == 8, "invalid mixed-property output");
        let category: usize = fields[1].parse()?;
        let mode: usize = fields[2].parse()?;
        let position = [fields[3].parse()?, fields[4].parse()?, fields[5].parse()?];
        let state = parse_state(fields[6])?;
        let member = BTreeSet::from([state.name.clone()]);
        let empty = BTreeSet::new();
        let tags = match category {
            1 => {
                native_persistence::structure_block_age::AgeTags::new(empty.clone(), member, empty)
            }
            2 => {
                native_persistence::structure_block_age::AgeTags::new(empty.clone(), empty, member)
            }
            _ => anyhow::bail!("invalid category"),
        };
        let mut random = native_core::random::LegacyRandom::new(
            native_resources::blockstates::positional_seed(position[0], position[1], position[2]),
        );
        let actual = tags.process(&state, [0.0_f32, 0.2, 0.8, 1.0][mode], &mut random)?;
        assert_eq!(actual, parse_state(fields[7])?, "{line}");
        mixed_count[category] += 1;
    }
    assert!(mixed_count[1] > 1_000 && mixed_count[2] > 1_000);

    // Exercise the same states through the real data-pack tag loader and the
    // processor list used by structure generation.
    let world = temp.path().join("tagged-world");
    let pack = world.join("datapacks/extra-aging");
    let tags_dir = pack.join("data/minecraft/tags/block");
    std::fs::create_dir_all(&tags_dir)?;
    std::fs::write(
        pack.join("pack.mcmeta"),
        br#"{"pack":{"description":"test","min_format":121,"max_format":121}}"#,
    )?;
    for (name, values) in [
        (
            "stairs",
            [
                "minecraft:dirt",
                "minecraft:iron_chain",
                "minecraft:netherrack",
            ],
        ),
        (
            "slabs",
            ["minecraft:oak_log", "minecraft:barrier", "minecraft:chest"],
        ),
        (
            "walls",
            [
                "minecraft:redstone_ore",
                "minecraft:hanging_roots",
                "minecraft:oak_fence",
            ],
        ),
    ] {
        std::fs::write(
            tags_dir.join(format!("{name}.json")),
            serde_json::to_vec(&json!({"values":values}))?,
        )?;
    }
    let selection = native_resources::datapacks::DataPackSettings {
        enabled: vec!["vanilla".into(), "file/extra-aging".into()],
        disabled: vec![],
        enabled_features: vec!["minecraft:vanilla".into()],
    };
    ensure!(
        resources
            .configure_world_data(&world, &selection)?
            .is_empty(),
        "pack warnings"
    );
    let mut processors = Vec::new();
    for mossiness in [0.0_f32, 0.2, 0.8, 1.0] {
        processors.push(PreparedProcessors::load(
            &HolderValue::Inline(json!({"processors":[{"processor_type":"minecraft:block_age","mossiness":mossiness}]})),
            &mut resources,
            &blocks,
            &registry,
        )?);
    }
    let mut loaded_count = 0;
    for line in text.lines().filter(|line| line.starts_with("B\t")) {
        let fields: Vec<_> = line.split('\t').collect();
        let category: usize = fields[1].parse()?;
        let state = parse_state(fields[6])?;
        if state.name
            != [
                "minecraft:dirt",
                "minecraft:oak_log",
                "minecraft:redstone_ore",
            ][category]
        {
            continue;
        }
        let mode: usize = fields[2].parse()?;
        let position = [fields[3].parse()?, fields[4].parse()?, fields[5].parse()?];
        let result = processors[mode]
            .process(
                PlacementBlock {
                    position,
                    state,
                    nbt: Some(marker.clone()),
                },
                &mut |_| anyhow::bail!("age processor requested world state"),
            )?
            .context("age processor unexpectedly removed block")?;
        assert_eq!(result.state, parse_state(fields[7])?, "{line}");
        assert_eq!(result.nbt, Some(marker.clone()));
        loaded_count += 1;
    }
    assert_eq!(loaded_count, 312);
    let mut loaded_waterlogged = [0_usize; 3];
    for line in text.lines().filter(|line| line.starts_with("C\t")) {
        let fields: Vec<_> = line.split('\t').collect();
        let category: usize = fields[1].parse()?;
        let state = parse_state(fields[6])?;
        if state.name
            != [
                "minecraft:iron_chain",
                "minecraft:barrier",
                "minecraft:hanging_roots",
            ][category]
        {
            continue;
        }
        let mode: usize = fields[2].parse()?;
        let position = [fields[3].parse()?, fields[4].parse()?, fields[5].parse()?];
        let result = processors[mode]
            .process(
                PlacementBlock {
                    position,
                    state,
                    nbt: Some(marker.clone()),
                },
                &mut |_| anyhow::bail!("age processor requested world state"),
            )?
            .context("age processor unexpectedly removed block")?;
        assert_eq!(result.state, parse_state(fields[7])?, "{line}");
        assert_eq!(result.nbt, Some(marker.clone()));
        loaded_waterlogged[category] += 1;
    }
    assert!(loaded_waterlogged.into_iter().all(|count| count > 0));
    let mut loaded_mixed = [0_usize; 3];
    for line in text.lines().filter(|line| line.starts_with("D\t")) {
        let fields: Vec<_> = line.split('\t').collect();
        let category: usize = fields[1].parse()?;
        let state = parse_state(fields[6])?;
        if state.name != ["", "minecraft:chest", "minecraft:oak_fence"][category] {
            continue;
        }
        let mode: usize = fields[2].parse()?;
        let position = [fields[3].parse()?, fields[4].parse()?, fields[5].parse()?];
        let result = processors[mode]
            .process(
                PlacementBlock {
                    position,
                    state,
                    nbt: Some(marker.clone()),
                },
                &mut |_| anyhow::bail!("age processor requested world state"),
            )?
            .context("age processor unexpectedly removed block")?;
        assert_eq!(result.state, parse_state(fields[7])?, "{line}");
        assert_eq!(result.nbt, Some(marker.clone()));
        loaded_mixed[category] += 1;
    }
    assert!(loaded_mixed[1] > 0 && loaded_mixed[2] > 0);
    Ok(())
}
