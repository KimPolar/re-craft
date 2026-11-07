use anyhow::{Context, Result, ensure};
use native_persistence::{
    jigsaw_replacement::PlacementBlock, nbt::Tag, structure_processors::PreparedProcessors,
};
use native_resources::{packs::ResourceStack, structure_pool::HolderValue};
use native_world::loaded::BlockState;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, process::Command};

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
    Ok(())
}
