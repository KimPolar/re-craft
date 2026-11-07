use super::*;
use native_persistence::nbt::Tag;

fn sulfur(value: &str) -> BlockState {
    BlockState {
        name: "minecraft:potent_sulfur".into(),
        properties: BTreeMap::from([("potent_sulfur_state".into(), value.into())]),
    }
}

#[test]
fn generated_potent_sulfur_keeps_pending_entities_and_rolls_back() -> Result<()> {
    let (mut terrain, tags, _, _) = super::tests::fixture();
    let names = [
        "air",
        "cave_air",
        "void_air",
        "stone",
        "diamond_ore",
        "potent_sulfur",
    ]
    .into_iter()
    .map(|name| format!("minecraft:{name}"))
    .collect();
    let motion = HeightmapTags::load(&mut super::tests::Data, &names)?;
    let writable = terrain.snapshot().chunks.keys().copied().collect();
    let mut region = terrain.decoration_region(&tags, &motion, writable)?;
    for (index, value) in ["dry", "wet", "dormant", "erupting", "continuous"]
        .into_iter()
        .enumerate()
    {
        let pos = [index as i32 + 2, 7, 2];
        assert!(SimpleBlockWorld::place_simple_state(
            &mut region,
            pos,
            &sulfur(value)
        )?);
    }
    // Replacing the same pending state must not duplicate its position.
    assert!(SimpleBlockWorld::place_simple_state(
        &mut region,
        [2, 7, 2],
        &sulfur("dry")
    )?);
    assert!(SimpleBlockWorld::place_simple_state(
        &mut region,
        [2, 7, 2],
        &sulfur("wet")
    )?);
    region.finish()?;
    let center = ChunkPos { x: 0, z: 0 };
    let Tag::List { values, .. } = terrain.generated_block_entity_data_with(center, |state| {
        Some(state.name == "minecraft:potent_sulfur")
    })?
    else {
        anyhow::bail!("missing generated block entity list");
    };
    assert_eq!(values.len(), 5);
    for entity in values {
        assert_eq!(entity.get("id"), Some(&Tag::String("DUMMY".into())));
        assert_eq!(entity.get("y"), Some(&Tag::Int(7)));
        assert_eq!(entity.get("z"), Some(&Tag::Int(2)));
    }
    let before = terrain.snapshot();
    let writable = before.chunks.keys().copied().collect();
    let mut failed = terrain.decoration_region(&tags, &motion, writable)?;
    assert!(
        SimpleBlockWorld::place_simple_state(&mut failed, [2, 7, 2], &sulfur("invalid")).is_err()
    );
    assert!(failed.finish().is_err());
    assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
    Ok(())
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn potent_sulfur_simple_feature_matches_java() -> Result<()> {
    use serde_json::{Value, json};
    use std::{path::PathBuf, process::Command};

    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
        .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let mut resources = native_resources::packs::ResourceStack::open(&game)?;
    let registry = resources.vanilla_generated_block_registry()?;
    let names = resources.vanilla_block_ids()?;
    let motion = HeightmapTags::load(&mut resources, &names)?;
    let version = game.join("versions/26.3");
    let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    ensure!(manifest["id"] == "26.3", "wrong Minecraft version");
    let temp = tempfile::tempdir()?;
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_owned()];
    for library in manifest["libraries"]
        .as_array()
        .context("missing libraries")?
    {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths)?;
    let source = temp.path().join("PotentSulfurOracle.java");
    std::fs::write(
        &source,
        include_str!("../../../tests/java/PotentSulfurOracle.java"),
    )?;
    let output = Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source)
        .output()?;
    ensure!(
        output.status.success(),
        "Java compilation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("PotentSulfurOracle")
        .arg(temp.path())
        .output()?;
    ensure!(
        output.status.success(),
        "Java oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut count = 0;
    for line in String::from_utf8(output.stdout)?.lines() {
        let words: Vec<_> = line.split_whitespace().collect();
        let [
            "F",
            variant,
            old,
            restricted,
            scheduled,
            seed,
            result,
            final_name,
            pending,
            tick,
            next,
        ] = words.as_slice()
        else {
            continue;
        };
        let restricted = *restricted == "1";
        let scheduled = *scheduled == "1";
        let feature = SimpleBlockFeature::decode(
            &json!({"type":"minecraft:simple_block", "to_place":{"id":"minecraft:potent_sulfur", "properties":{"potent_sulfur_state":variant}}, "schedule_tick":scheduled}),
            &mut |value| {
                let (name, properties) = registry.resolve(value)?;
                Ok(BlockState { name, properties })
            },
        )?;
        let (mut terrain, tags, _, _) = super::tests::fixture();
        let pos = [8, 7, 8];
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut setup = terrain.decoration_region(&tags, &motion, writable)?;
        let (name, properties) = registry.resolve(&json!(old))?;
        setup.set_proto_block(pos, &BlockState { name, properties })?;
        setup.finish()?;
        let writable = if restricted {
            BTreeSet::new()
        } else {
            BTreeSet::from([chunk(pos)])
        };
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        region.template_registry = Some(&registry);
        let mut random = DecorationRandom::new(seed.parse()?);
        assert_eq!(
            feature.place(&mut region, &mut random, pos)?,
            result.parse::<bool>()?,
            "{line}"
        );
        assert_eq!(random.next_long(), next.parse::<i64>()?, "{line}");
        region.finish()?;
        let world = terrain.snapshot();
        let after = world
            .block(pos[0], pos[1], pos[2])
            .context("missing placed block")?;
        assert_eq!(after.name.as_str(), *final_name, "{line}");
        if !restricted {
            assert_eq!(
                after
                    .properties
                    .get("potent_sulfur_state")
                    .map(String::as_str),
                Some(*variant),
                "{line}"
            );
        }
        let Tag::List { values, .. } =
            terrain.generated_block_entity_data(chunk(pos), &registry)?
        else {
            anyhow::bail!("missing block entity list")
        };
        assert_eq!(!values.is_empty(), pending.parse::<bool>()?, "{line}");
        if !values.is_empty() {
            assert_eq!(values.len(), 1, "{line}");
            assert_eq!(
                values[0].get("id"),
                Some(&Tag::String("DUMMY".into())),
                "{line}"
            );
        }
        let ticks = world
            .saved_block_ticks
            .get(&chunk(pos))
            .map(Vec::as_slice)
            .unwrap_or_default();
        assert_eq!(ticks.len(), usize::from(scheduled), "{line}");
        if scheduled {
            assert_eq!(ticks[0].block.as_str(), *tick, "{line}");
            assert_eq!(
                (ticks[0].position, ticks[0].delay, ticks[0].priority),
                (pos, 0, 0),
                "{line}"
            );
        }
        count += 1;
    }
    assert_eq!(count, 5 * 4 * 2 * 2 * 2);
    Ok(())
}
