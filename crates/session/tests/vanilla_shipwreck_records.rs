use anyhow::{Context, Result, ensure};
use native_core::beardifier::{BoundingBox, TerrainAdjustment};
use native_persistence::structure_template::StructureTemplate;
use native_resources::{
    packs::{ResourceId, ResourceStack},
    shipwreck::Piece,
    structure_transform::Rotation,
};
use native_session::worldgen::shipwreck::PreparedStart;
use native_world::loaded::BlockState;
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf, process::Command, sync::Arc};

fn key(state: &BlockState) -> String {
    format!(
        "{}|{}",
        state.name,
        state
            .properties
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(",")
    )
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn shipwreck_block_records_and_markers_match_java() -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir()?;
    let version = game.join("versions/26.3");
    let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    ensure!(manifest["id"] == "26.3", "wrong version");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"]
        .as_array()
        .context("missing libraries")?
    {
        if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths)?;
    let source = temp.path().join("ShipwreckRecordsOracle.java");
    std::fs::write(&source, include_str!("java/ShipwreckRecordsOracle.java"))?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source)
        .output()?;
    ensure!(
        compiled.status.success(),
        "Java compilation: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let names = [
        "with_mast",
        "upsidedown_full",
        "upsidedown_fronthalf",
        "upsidedown_backhalf",
        "sideways_full",
        "sideways_fronthalf",
        "sideways_backhalf",
        "rightsideup_full",
        "rightsideup_fronthalf",
        "rightsideup_backhalf",
        "with_mast_degraded",
        "upsidedown_full_degraded",
        "upsidedown_fronthalf_degraded",
        "upsidedown_backhalf_degraded",
        "sideways_full_degraded",
        "sideways_fronthalf_degraded",
        "sideways_backhalf_degraded",
        "rightsideup_full_degraded",
        "rightsideup_fronthalf_degraded",
        "rightsideup_backhalf_degraded",
    ];
    let names: Vec<_> = names.iter().map(|n| format!("shipwreck/{n}")).collect();
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg("ShipwreckRecordsOracle")
        .args(&names)
        .output()?;
    ensure!(
        output.status.success(),
        "Java oracle: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout)?;
    let mut lines = text
        .lines()
        .filter(|l| {
            l.starts_with("I\t")
                || l.starts_with("T\t")
                || l.starts_with("B\t")
                || l.starts_with("M\t")
        })
        .peekable();
    let mut provider = ResourceStack::open(&game)?;
    let registry = provider.vanilla_generated_block_registry()?;
    let mut unsupported = std::collections::BTreeSet::new();
    for name in &names {
        let id = ResourceId::parse(&format!("minecraft:{name}"))?;
        let template = StructureTemplate::load(&mut provider, &id)?.context("missing template")?;
        for state in template.palettes.iter().flatten() {
            if let Err(error) =
                native_persistence::jigsaw_template::resolve_template_state(state, &registry)
            {
                unsupported.insert(format!("{error:#}"));
            }
        }
    }
    ensure!(
        unsupported.is_empty(),
        "unsupported shipwreck palette states: {unsupported:?}"
    );
    let mut total_blocks = 0;
    let mut total_markers = 0;
    for (i, name) in names.iter().enumerate() {
        let mut classes = BTreeMap::new();
        while lines.peek().is_some_and(|l| l.starts_with("I\t")) {
            let row: Vec<_> = lines.next().unwrap().split('\t').collect();
            let value = row[2].parse::<bool>()?;
            if let Some(old) = classes.insert(row[1].to_owned(), value) {
                assert_eq!(old, value);
            }
        }
        let header = lines.next().context("missing header")?;
        let row: Vec<_> = header.split('\t').collect();
        assert_eq!(row[0], "T");
        assert_eq!(row[1], name);
        let id = ResourceId::parse(&format!("minecraft:{name}"))?;
        let template =
            Arc::new(StructureTemplate::load(&mut provider, &id)?.context("missing template")?);
        let i = i as i32;
        let origin = [-101 + i * 7, 41 + i % 11, 207 - i * 13];
        let radius = if i % 2 == 0 { 64 } else { 8 };
        let piece = Piece {
            template: format!("minecraft:{name}"),
            position: origin,
            rotation: [
                Rotation::None,
                Rotation::Clockwise90,
                Rotation::Clockwise180,
                Rotation::Counterclockwise90,
            ][i as usize % 4],
            beached: false,
        };
        let bounds = piece.bounds(template.size)?;
        let start = PreparedStart {
            chunk: [0, 0],
            generation_point: [0, 0, 0],
            terrain_adjustment: TerrainAdjustment::None,
            piece,
            template,
            bounds,
            height_adjusted: true,
        };
        let clip = BoundingBox::new(origin.map(|v| v - radius), origin.map(|v| v + radius))
            .map_err(anyhow::Error::msg)?;
        let (blocks, markers) = start.block_records(
            &registry,
            clip,
            |state| {
                classes
                    .get(&key(state))
                    .copied()
                    .with_context(|| format!("missing class for {}", key(state)))
            },
            &mut |_| anyhow::bail!("empty processors cannot access world"),
        )?;
        assert_eq!(blocks.len(), row[2].parse::<usize>()?, "{header}");
        assert_eq!(markers.len(), row[3].parse::<usize>()?, "{header}");
        for block in blocks {
            let line = lines.next().context("missing block")?;
            let row: Vec<_> = line.split('\t').collect();
            assert_eq!(row[0], "B");
            assert_eq!(
                block.block.position,
                [
                    row[1].parse::<i32>()?,
                    row[2].parse::<i32>()?,
                    row[3].parse::<i32>()?
                ],
                "{line}"
            );
            assert_eq!(key(&block.block.state), row[4], "{line}");
            assert_eq!(block.block.nbt.is_some(), row[5].parse::<bool>()?, "{line}");
            total_blocks += 1;
        }
        for marker in markers {
            let line = lines.next().context("missing marker")?;
            let row: Vec<_> = line.split('\t').collect();
            assert_eq!(row[0], "M");
            assert_eq!(
                marker.position,
                [
                    row[1].parse::<i32>()?,
                    row[2].parse::<i32>()?,
                    row[3].parse::<i32>()?
                ],
                "{line}"
            );
            assert_eq!(String::from_utf16(&marker.metadata.0)?, row[4], "{line}");
            total_markers += 1;
        }
    }
    assert!(total_blocks > 1000);
    assert!(total_markers > 0);
    assert!(lines.next().is_none());
    Ok(())
}
