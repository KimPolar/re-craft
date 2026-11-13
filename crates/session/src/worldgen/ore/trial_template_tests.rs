//! Compare installed trial chamber templates with Java structure placement.
use super::*;
use native_persistence::{nbt, structure_processors::PreparedProcessors};
use native_resources::{
    jigsaw_placement::Piece,
    structure_pool::{HolderValue, PoolElement, Projection},
    structure_transform::Rotation,
};
use std::{collections::BTreeMap, path::PathBuf, process::Command};

fn canonicalize(tag: &mut Tag) {
    match tag {
        Tag::Compound(fields) => {
            for (_, value) in fields.iter_mut() {
                canonicalize(value);
            }
            fields.sort_by(|a, b| a.0.0.cmp(&b.0.0));
        }
        Tag::List { values, .. } => values.iter_mut().for_each(canonicalize),
        _ => {}
    }
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn installed_trial_templates_match_java_blocks_entities_and_random() -> Result<()> {
    let game = native_resources::default_minecraft_dir().context("Minecraft directory missing")?;
    let mut resources = native_resources::packs::ResourceStack::open(&game)?;
    let blocks = resources.vanilla_block_ids()?;
    let registry = resources.vanilla_generated_block_registry()?;
    let motion = HeightmapTags::load(&mut resources, &blocks)?;
    let clip = BoundingBox::new([0, 0, 0], [15, 15, 15]).map_err(anyhow::Error::msg)?;
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let version = game.join("versions/26.3");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    let temp = tempfile::tempdir()?;
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
    let source = temp.path().join("TrialTemplatePlacementOracle.java");
    std::fs::write(
        &source,
        include_str!("../../../tests/java/TrialTemplatePlacementOracle.java"),
    )?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(&source)
        .output()?;
    ensure!(
        compiled.status.success(),
        "Java compilation failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let holder = HolderValue::Inline(serde_json::json!({"processors":[]}));
    let processors = super::super::super::jigsaw::PieceProcessors::Single(Arc::new(
        PreparedProcessors::load(&holder, &mut resources, &blocks, &registry)?,
    ));
    let mut entities_checked = 0;
    for template in [
        "dispensers/floor_dispenser",
        "dispensers/wall_dispenser",
        "chests/supply",
        "decor/barrel",
        "decor/disposal",
        "decor/flow_pot",
        "decor/guster_pot",
        "decor/scrape_pot",
        "decor/undecorated_pot",
        "spawner/breeze/breeze",
        "reward/vault",
        "reward/ominous_vault",
    ] {
        for (rotation, java_rotation) in [
            (Rotation::None, "NONE"),
            (Rotation::Clockwise90, "CLOCKWISE_90"),
        ] {
            let piece = Piece {
                element: PoolElement::Single {
                    legacy: false,
                    location: ResourceId::parse(&format!("minecraft:trial_chambers/{template}"))?,
                    processors: holder.clone(),
                    projection: Projection::Rigid,
                    override_liquid_settings: None,
                },
                origin: [6, 8, 6],
                rotation,
                bounds: clip,
                ground_level_delta: 0,
                junctions: vec![],
            };
            let prepared = PreparedTemplatePiece::load(&piece, &processors, &mut resources)?;
            let output = Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&classpath)
                .arg("TrialTemplatePlacementOracle")
                .arg(version.join("26.3.jar"))
                .arg(template)
                .arg(java_rotation)
                .arg(temp.path())
                .output()?;
            ensure!(
                output.status.success(),
                "Java placement failed for {template} {java_rotation}: {}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            );
            let mut expected_blocks = BTreeMap::new();
            let mut expected_entities = BTreeMap::new();
            let mut expected_random = None;
            for line in std::str::from_utf8(&output.stdout)?.lines() {
                let fields = line.split('\t').collect::<Vec<_>>();
                match fields.as_slice() {
                    ["BLOCK", x, y, z, id, properties] => {
                        let pos = [x.parse::<i32>()?, y.parse::<i32>()?, z.parse::<i32>()?];
                        ensure!(
                            expected_blocks
                                .insert(
                                    pos,
                                    BlockState {
                                        name: (*id).into(),
                                        properties: serde_json::from_str(properties)?
                                    }
                                )
                                .is_none(),
                            "duplicate Java block"
                        );
                    }
                    ["ENTITY", x, y, z, filename] => {
                        let pos = [x.parse::<i32>()?, y.parse::<i32>()?, z.parse::<i32>()?];
                        let mut data =
                            nbt::read(&std::fs::read(temp.path().join(filename))?)?.value;
                        canonicalize(&mut data);
                        ensure!(
                            expected_entities.insert(pos, data).is_none(),
                            "duplicate Java entity"
                        );
                    }
                    ["RANDOM", value] => {
                        ensure!(
                            expected_random.replace(value.parse::<i64>()?).is_none(),
                            "duplicate Java random"
                        );
                    }
                    _ => {}
                }
            }
            ensure!(
                expected_blocks.len() == 4096 && !expected_entities.is_empty(),
                "incomplete Java placement result"
            );
            let (mut terrain, tags, _, _) = super::super::tests::fixture();
            let writable = terrain.snapshot().chunks.keys().copied().collect();
            let mut region = terrain.decoration_region(&tags, &motion, writable)?;
            let mut random = DecorationRandom::new(42);
            assert!(region.place_rigid_jigsaw_piece(
                &prepared,
                &registry,
                clip,
                LiquidSettings::IgnoreWaterlogging,
                &mut random
            )?);
            region.finish()?;
            let world = terrain.snapshot();
            for (pos, expected) in expected_blocks {
                assert_eq!(
                    world.block(pos[0], pos[1], pos[2]),
                    Some(&expected),
                    "{template} {java_rotation}, {pos:?}"
                );
            }
            let Tag::List { values, .. } =
                terrain.generated_block_entity_data(ChunkPos { x: 0, z: 0 }, &registry)?
            else {
                bail!("missing block entities")
            };
            let mut actual_entities = BTreeMap::new();
            for mut data in values {
                let coordinate = |name| match data.get(name) {
                    Some(Tag::Int(value)) => Ok(*value),
                    _ => bail!("missing entity coordinate {name}"),
                };
                let pos = [coordinate("x")?, coordinate("y")?, coordinate("z")?];
                canonicalize(&mut data);
                ensure!(
                    actual_entities.insert(pos, data).is_none(),
                    "duplicate generated entity"
                );
            }
            entities_checked += actual_entities.len();
            assert_eq!(
                actual_entities, expected_entities,
                "{template} {java_rotation}"
            );
            assert_eq!(
                Some(random.next_long()),
                expected_random,
                "{template} {java_rotation}"
            );
            eprintln!("verified trial template {template} {java_rotation}");
        }
    }
    ensure!(entities_checked >= 20, "missing trial block entities");
    Ok(())
}
