use anyhow::{Context, Result, ensure};
use native_persistence::structure_processors::PreparedProcessors;
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
fn ruined_portal_templates_match_java() -> Result<()> {
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
    let source = temp.path().join("RuinedPortalTemplateOracle.java");
    std::fs::write(
        &source,
        include_str!("java/RuinedPortalTemplateOracle.java"),
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
        .arg("RuinedPortalTemplateOracle")
        .output()?;
    ensure!(
        output.status.success(),
        "Java oracle failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    use native_core::beardifier::BoundingBox;
    use native_persistence::{
        jigsaw_template::{
            StructureTemplateSettings, process_structure_template, resolve_template_state,
        },
        structure_template::StructureTemplate,
    };
    use native_resources::{
        packs::ResourceId,
        structure_transform::{Mirror, Rotation},
    };
    fn key(state: &BlockState) -> String {
        let mut s = format!("{}|", state.name);
        for (name, value) in &state.properties {
            s.push_str(&format!("{name}={value};"));
        }
        s
    }
    let mut resources = ResourceStack::open(&game)?;
    let registry = resources.vanilla_generated_block_registry()?;
    let blocks = resources.vanilla_block_ids()?;
    let mut processors = Vec::new();
    for flags in [0, 7] {
        processors.push(PreparedProcessors::ruined_portal(
            &mut resources,
            &blocks,
            VerticalPlacement::OnLandSurface,
            flags & 2 != 0,
            flags & 1 != 0,
            0.8,
            flags & 4 != 0,
        )?);
    }
    let templates = ["portal_1", "portal_5", "giant_portal_1"]
        .into_iter()
        .map(|name| {
            StructureTemplate::load(
                &mut resources,
                &ResourceId::parse(&format!("minecraft:ruined_portal/{name}"))?,
            )?
            .context("missing template")
        })
        .collect::<Result<Vec<_>>>()?;
    let mirrors = [Mirror::None, Mirror::LeftRight, Mirror::FrontBack];
    let rotations = [
        Rotation::None,
        Rotation::Clockwise90,
        Rotation::Clockwise180,
        Rotation::Counterclockwise90,
    ];
    let mut classes = BTreeMap::new();
    let mut actual = Vec::new();
    let mut index = 0;
    let mut cases = 0;
    for line in String::from_utf8(output.stdout)?.lines() {
        let fields: Vec<_> = line.split('\t').collect();
        match fields.as_slice() {
            ["C", state, full] => {
                classes.insert((*state).to_string(), *full == "true");
            }
            ["BEGIN", t, mirror, rotation, flags, count] => {
                ensure!(index == actual.len(), "missing expected template records");
                let template = &templates[t.parse::<usize>()?];
                let flags: i32 = flags.parse()?;
                actual = process_structure_template(
                    template,
                    &processors[usize::from(flags == 7)],
                    StructureTemplateSettings {
                        origin: [-37, 72, 51],
                        mirror: mirrors[mirror.parse::<usize>()?],
                        rotation: rotations[rotation.parse::<usize>()?],
                        pivot: [template.size[0] / 2, 0, template.size[2] / 2],
                        clip: BoundingBox {
                            min: [-43, 70, 47],
                            max: [-32, 82, 58],
                        },
                    },
                    |tag| resolve_template_state(tag, &registry),
                    |state| {
                        classes
                            .get(&key(state))
                            .copied()
                            .context(format!("missing classification {}", key(state)))
                    },
                    &mut |_| {
                        Ok(BlockState {
                            name: "minecraft:stone".into(),
                            properties: BTreeMap::new(),
                        })
                    },
                )?;
                assert_eq!(actual.len(), count.parse::<usize>()?, "{line}");
                index = 0;
                cases += 1;
            }
            ["B", x, y, z, state, nbt] => {
                let block = &actual
                    .get(index)
                    .context("too many template records")?
                    .block;
                assert_eq!(
                    block.position,
                    [x.parse::<i32>()?, y.parse::<i32>()?, z.parse::<i32>()?],
                    "{line}"
                );
                assert_eq!(block.state, parse_state(state)?, "{line}");
                assert_eq!(block.nbt.is_some(), *nbt == "true", "{line}");
                index += 1;
            }
            _ => {}
        }
    }
    assert_eq!(index, actual.len());
    assert_eq!(cases, 3 * 3 * 4 * 2);
    Ok(())
}
