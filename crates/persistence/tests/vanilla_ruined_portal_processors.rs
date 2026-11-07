use anyhow::{Context, Result, ensure};
use native_persistence::{
    jigsaw_replacement::PlacementBlock, nbt::Tag, structure_processors::PreparedProcessors,
};
use native_resources::{packs::ResourceStack, ruined_portal::VerticalPlacement};
use native_world::loaded::BlockState;
use serde_json::Value;
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
fn ruined_portal_processors_match_java() -> Result<()> {
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
    let source = temp.path().join("RuinedPortalProcessorsOracle.java");
    std::fs::write(
        &source,
        include_str!("java/RuinedPortalProcessorsOracle.java"),
    )?;
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
        .arg("RuinedPortalProcessorsOracle")
        .output()?;
    ensure!(
        output.status.success(),
        "Java oracle failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let mut resources = ResourceStack::open(&game)?;
    let blocks = resources.vanilla_block_ids()?;
    let placements = [
        VerticalPlacement::OnLandSurface,
        VerticalPlacement::PartlyBuried,
        VerticalPlacement::OnOceanFloor,
        VerticalPlacement::InMountain,
        VerticalPlacement::Underground,
        VerticalPlacement::InNether,
    ];
    let mut processors = Vec::new();
    for placement in placements {
        for flags in 0..8 {
            processors.push(PreparedProcessors::ruined_portal(
                &mut resources,
                &blocks,
                placement,
                flags & 2 != 0,
                flags & 1 != 0,
                0.8,
                flags & 4 != 0,
            )?);
        }
    }
    let marker = Tag::Compound(vec![("test".into(), Tag::Int(17))]);
    let text = String::from_utf8(output.stdout)?;
    let mut count = 0;
    let mut total = None;
    for line in text.lines() {
        let fields: Vec<_> = line.split('\t').collect();
        match fields.as_slice() {
            ["P", placement, flags, i, world, input, expected] => {
                let placement: usize = placement.parse()?;
                let flags: usize = flags.parse()?;
                let i: i32 = i.parse()?;
                let position = [i * 17 - 75, i * 19 - 64, 87 - i * 13];
                let block = PlacementBlock {
                    position,
                    state: parse_state(input)?,
                    nbt: Some(marker.clone()),
                };
                let world = if *world == "0" {
                    "minecraft:stone"
                } else {
                    "minecraft:lava"
                };
                let actual = processors[placement * 8 + flags].process(block, &mut |_| {
                    Ok(BlockState {
                        name: world.into(),
                        properties: BTreeMap::new(),
                    })
                })?;
                if *expected == "<skip>" {
                    assert!(actual.is_none(), "{line}");
                } else {
                    let actual = actual.context("unexpected ignored block")?;
                    assert_eq!(actual.position, position, "{line}");
                    assert_eq!(actual.nbt, Some(marker.clone()), "{line}");
                    assert_eq!(actual.state, parse_state(expected)?, "{line}");
                }
                count += 1;
            }
            ["COUNT", cases] => total = Some(cases.parse::<usize>()?),
            _ => {}
        }
    }
    assert!(count > 20_000, "insufficient Java state cases: {count}");
    assert_eq!(Some(count), total);
    Ok(())
}
