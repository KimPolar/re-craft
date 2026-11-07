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
fn blackstone_replacement_matches_java() -> Result<()> {
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
    let source = temp.path().join("BlackstoneReplaceOracle.java");
    std::fs::write(&source, include_str!("java/BlackstoneReplaceOracle.java"))?;
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
        .arg("BlackstoneReplaceOracle")
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
    let config = HolderValue::Inline(
        json!({"processors":[{"processor_type":"minecraft:blackstone_replace"}]}),
    );
    let processors = PreparedProcessors::load(&config, &mut resources, &blocks, &registry)?;
    let marker = Tag::Compound(vec![("test".into(), Tag::Int(17))]);
    let text = String::from_utf8(output.stdout)?;
    let mut count = 0;
    let mut total = None;
    let mut valid_type = false;
    for line in text.lines() {
        let fields: Vec<_> = line.split('\t').collect();
        match fields.as_slice() {
            ["TYPE", kind] => valid_type = *kind == "minecraft:blackstone_replace",
            ["B", input, expected] => {
                let position = [-21, 64, 81];
                let block = PlacementBlock {
                    position,
                    state: parse_state(input)?,
                    nbt: Some(marker.clone()),
                };
                let actual = processors
                    .process(block, &mut |_| anyhow::bail!("unexpected world read"))?
                    .context("replacement removed block")?;
                assert_eq!(actual.position, position, "{line}");
                assert_eq!(actual.nbt, Some(marker.clone()), "{line}");
                assert_eq!(actual.state, parse_state(expected)?, "{line}");
                count += 1;
            }
            ["COUNT", mappings, cases] => {
                assert_eq!(
                    mappings.parse::<usize>()?,
                    23,
                    "replacement mapping count changed"
                );
                total = Some(cases.parse::<usize>()?);
            }
            _ => {}
        }
    }
    assert!(count > 1000, "insufficient Java state cases: {count}");
    assert!(valid_type, "unexpected Java processor type");
    assert_eq!(Some(count), total);
    Ok(())
}
