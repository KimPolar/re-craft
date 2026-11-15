use super::*;
use crate::worldgen::ocean_ruin::{ArchaeologyReplacement, OceanRuinPlacementWorld};
use std::{cell::Cell, path::PathBuf, process::Command};

fn run_java(game: &std::path::Path) -> Result<String> {
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
    let source = temp.path().join("RuinBrushableReplacementOracle.java");
    std::fs::write(
        &source,
        include_str!("../../../../tests/java/RuinBrushableReplacementOracle.java"),
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
        .arg("RuinBrushableReplacementOracle")
        .output()?;
    ensure!(
        output.status.success(),
        "Java oracle failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?)
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn generated_brushable_replacement_matches_java() -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let mut resources = native_resources::packs::ResourceStack::open(&game)?;
    let registry = resources.vanilla_generated_block_registry()?;
    let ids = resources.vanilla_block_ids()?;
    let motion = HeightmapTags::load(&mut resources, &ids)?;
    let java = run_java(&game)?;
    let mut checked = 0;
    let mut rejected = 0;
    for line in java.lines().filter_map(|line| line.strip_prefix("R ")) {
        let fields: Vec<_> = line.split_whitespace().collect();
        ensure!(fields.len() == 6, "invalid Java result {line}");
        let (source_name, source_properties) = registry.resolve(&serde_json::json!(fields[0]))?;
        let source = BlockState {
            name: source_name,
            properties: source_properties,
        };
        let (target_name, target_properties) = registry.resolve(&serde_json::json!(fields[1]))?;
        let target = BlockState {
            name: target_name,
            properties: target_properties,
        };
        let pending: bool = fields[2].parse()?;
        let replacement = ArchaeologyReplacement {
            state: source.clone(),
            nbt: Tag::Compound(vec![
                (
                    "LootTable".into(),
                    Tag::String("minecraft:archaeology/ocean_ruin_warm".into()),
                ),
                ("LootTableSeed".into(), Tag::Long(73)),
            ]),
        };
        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let center = ChunkPos { x: 0, z: 0 };
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let pos = [8, 8, 8];
        let other = [9, 8, 8];
        if pending {
            region.set_proto_block(pos, &source)?;
        } else {
            region.place_ocean_ruin_archaeology_block(pos, &replacement, &registry)?;
        }
        region.place_ocean_ruin_archaeology_block(other, &replacement, &registry)?;
        SimpleBlockWorld::schedule_simple_tick(&mut region, pos)?;
        let mut spawn =
            |_: [i32; 3],
             _: &World|
             -> Result<Option<super::super::super::GeneratedEntityRecord>> { Ok(None) };
        let fluids = BTreeSet::new();
        let aligned = Cell::new(None);
        let mut random = DecorationRandom::new(42);
        let result = {
            let mut piece = RuinPieceRegion {
                marker: RuinMarkerRegion {
                    region: &mut region,
                    registry: &registry,
                    water_fluids: &fluids,
                    sea_level: 63,
                    spawn: &mut spawn,
                },
                ice_tags: &tags,
                bubble_fluids: &fluids,
                aligned: &aligned,
            };
            piece.place_template_block(pos, &target, None, &mut random)
        };
        if pending {
            assert!(
                result.is_err(),
                "unrecorded brushable must not be discarded: {line}"
            );
            assert_eq!(region.read(pos)?, source, "{line}");
            rejected += 1;
            continue;
        }
        assert!(result?, "{line}");
        assert_eq!(region.read(pos)?, target, "{line}");
        assert!(!region.monster_entities.contains_key(&pos), "{line}");
        assert!(region.monster_entities.contains_key(&other), "{line}");
        let order = region
            .entity_order
            .get(&center)
            .context("entity order missing")?
            .saving_order()?;
        assert!(!order.contains(&pos) && order.contains(&other), "{line}");
        let ticks = region
            .world
            .saved_block_ticks
            .get(&center)
            .context("saved tick missing")?;
        assert_eq!(ticks.len(), fields[3].parse::<usize>()?, "{line}");
        assert_eq!(ticks[0].block, source.name, "{line}");
        assert_eq!(ticks[0].position, pos, "{line}");
        assert_eq!(ticks[0].delay, fields[4].parse::<i32>()?, "{line}");
        assert_eq!(ticks[0].priority, fields[5].parse::<i32>()?, "{line}");
        assert!(region.poi_changes.is_empty(), "{line}");
        region.finish()?;
        let Tag::List { values, .. } = terrain.generated_block_entity_data(center, &registry)?
        else {
            bail!("missing generated block entity list")
        };
        assert_eq!(values.len(), 1, "{line}");
        assert_eq!(values[0].get("x"), Some(&Tag::Int(other[0])), "{line}");
        checked += 1;
    }
    assert_eq!(checked, 8);
    assert_eq!(rejected, 8);

    let mut checked_archaeology = 0;
    let mut rejected_archaeology = 0;
    for line in java.lines().filter_map(|line| line.strip_prefix("A ")) {
        let fields: Vec<_> = line.split_whitespace().collect();
        ensure!(fields.len() == 8, "invalid Java result {line}");
        let (source_name, source_properties) = registry.resolve(&serde_json::json!(fields[0]))?;
        let source = BlockState {
            name: source_name,
            properties: source_properties,
        };
        let (target_name, target_properties) = registry.resolve(&serde_json::json!(fields[1]))?;
        let target = BlockState {
            name: target_name,
            properties: target_properties,
        };
        let pending: bool = fields[2].parse()?;
        let replacement = |state: BlockState, seed| ArchaeologyReplacement {
            state,
            nbt: Tag::Compound(vec![
                (
                    "LootTable".into(),
                    Tag::String("minecraft:archaeology/ocean_ruin_warm".into()),
                ),
                ("LootTableSeed".into(), Tag::Long(seed)),
            ]),
        };
        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let center = ChunkPos { x: 0, z: 0 };
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let pos = [8, 8, 8];
        let other = [9, 8, 8];
        if pending {
            region.set_proto_block(pos, &source)?;
        } else {
            region.place_ocean_ruin_archaeology_block(
                pos,
                &replacement(source.clone(), 73),
                &registry,
            )?;
        }
        region.place_ocean_ruin_archaeology_block(
            other,
            &replacement(source.clone(), 73),
            &registry,
        )?;
        SimpleBlockWorld::schedule_simple_tick(&mut region, pos)?;
        let (chest_name, chest_properties) =
            registry.resolve(&serde_json::json!("minecraft:chest"))?;
        let chest = BlockState {
            name: chest_name,
            properties: chest_properties,
        };
        for x in 0..12 {
            let pending_position = [x, 6, 0];
            region.set_proto_block(pending_position, &chest)?;
            Arc::make_mut(&mut region.monster_entities).insert(
                pending_position,
                Some(
                    super::super::super::GeneratedMonsterRoomEntity::PendingChest {
                        position: pending_position,
                        state: chest.clone(),
                    },
                ),
            );
            Arc::make_mut(region.pending_order.entry(center).or_default()).insert(pending_position);
        }
        let result = region.place_ocean_ruin_archaeology_block(
            pos,
            &replacement(target.clone(), 79),
            &registry,
        );
        if pending {
            assert!(
                result.is_err(),
                "unrecorded brushable must not be discarded: {line}"
            );
            assert!(region.finish().is_err(), "{line}");
            rejected_archaeology += 1;
            continue;
        }
        result?;
        assert_eq!(region.read(pos)?, target, "{line}");
        assert!(region.monster_entities.contains_key(&pos), "{line}");
        assert!(region.monster_entities.contains_key(&other), "{line}");
        let order = region
            .entity_order
            .get(&center)
            .context("entity order missing")?
            .saving_order()?;
        assert!(order.contains(&pos) && order.contains(&other), "{line}");
        let ticks = region
            .world
            .saved_block_ticks
            .get(&center)
            .context("saved tick missing")?;
        assert_eq!(ticks.len(), fields[3].parse::<usize>()?, "{line}");
        assert_eq!(ticks[0].block, source.name, "{line}");
        assert_eq!(ticks[0].position, pos, "{line}");
        assert_eq!(ticks[0].delay, fields[4].parse::<i32>()?, "{line}");
        assert_eq!(ticks[0].priority, fields[5].parse::<i32>()?, "{line}");
        assert!(region.poi_changes.is_empty(), "{line}");
        let pending_order = region
            .pending_order
            .get(&center)
            .context("pending entity order missing")?
            .positions()?
            .into_iter()
            .map(|p| format!("{},{},{}", p[0], p[1], p[2]))
            .collect::<Vec<_>>()
            .join(";");
        assert_eq!(pending_order, fields[7], "{line}");
        region.finish()?;
        let Tag::List { values, .. } = terrain.generated_block_entity_data(center, &registry)?
        else {
            bail!("missing generated block entity list")
        };
        assert_eq!(values.len(), 14, "{line}");
        let updated = values
            .iter()
            .find(|entity| {
                entity.get("x") == Some(&Tag::Int(pos[0]))
                    && entity.get("z") == Some(&Tag::Int(pos[2]))
            })
            .context("new entity missing")?;
        assert_eq!(
            updated.get("LootTableSeed"),
            Some(&Tag::Long(fields[6].parse()?)),
            "{line}"
        );
        let retained = values
            .iter()
            .find(|entity| {
                entity.get("x") == Some(&Tag::Int(other[0]))
                    && entity.get("z") == Some(&Tag::Int(other[2]))
            })
            .context("other entity missing")?;
        assert_eq!(
            retained.get("LootTableSeed"),
            Some(&Tag::Long(73)),
            "{line}"
        );
        checked_archaeology += 1;
    }
    assert_eq!(checked_archaeology, 4);
    assert_eq!(rejected_archaeology, 4);
    Ok(())
}
