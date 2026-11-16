//! Round-trip a private generated world without changing the original fixture.
use super::*;
use std::path::Path;

fn copy_world(source: &Path, target: &Path) -> Result<()> {
    std::fs::create_dir(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let destination = target.join(entry.file_name());
        if kind.is_dir() {
            copy_world(&entry.path(), &destination)?;
        } else {
            ensure!(kind.is_file(), "fixture contains a link or special file");
            std::fs::copy(entry.path(), destination)?;
        }
    }
    Ok(())
}

/// Exercise actual stored structures, blocks, light and entities with the
/// installed Java serializers. This fixture remains private to target/.
#[test]
#[ignore = "requires a private generated 26.3 world, Minecraft 26.3 and Java 25 JDK"]
fn generated_portal_fixture_round_trips_through_java() -> Result<()> {
    let test_root = std::fs::canonicalize(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/region-test-temp"),
    )?;
    let source = std::fs::canonicalize(PathBuf::from(
        std::env::var_os("RECRAFT_PLAY_SAVE_TEST_WORLD")
            .context("RECRAFT_PLAY_SAVE_TEST_WORLD is not set")?,
    ))?;
    ensure!(
        source != test_root && source.starts_with(&test_root),
        "fixture is outside the private test directory"
    );
    let temp = tempfile::Builder::new()
        .prefix("portal-java-roundtrip-")
        .tempdir_in(&test_root)?;
    let world_path = temp.path().join("world");
    copy_world(&source, &world_path)?;
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let dimension = native_persistence::world::DimensionId::parse("overworld")?;
    let sidecars = world_path.join("dimensions/minecraft/overworld");
    let positions = saved_terrain_positions(&sidecars.join("region"))?;
    let entity_positions = saved_region_positions(&sidecars.join("entities"))?;
    let poi_positions = saved_region_positions(&sidecars.join("poi"))?;
    ensure!(
        entity_positions.is_subset(&positions) && poi_positions.is_subset(&positions),
        "sidecar is missing its terrain chunk"
    );
    let reader = &mut native_persistence::world::WorldReader::open(&world_path, dimension.clone())?;
    let seed = reader.saved_world_seed()?.context("world seed missing")?;
    let spawn = reader.world_spawn()?;
    let player = reader.read_player()?.context("player missing")?;
    let player_position = native_session::player_data::read_saved_position(&player)?
        .context("player position missing")?;
    let player_block = player_position.floor().as_ivec3();
    ensure!(
        player_position == player_block.as_dvec3() + glam::DVec3::new(0.5, 0.0, 0.5),
        "fixture is not at its initial player spawn"
    );
    let clocks = reader.world_clocks()?;
    let rules = reader.fluid_game_rules()?;
    let mut chunks = BTreeMap::new();
    for &pos in &positions {
        chunks.insert(
            pos,
            reader.read_chunk(pos.x, pos.z)?.context("chunk missing")?,
        );
    }
    let mut entities = BTreeMap::new();
    for &pos in &entity_positions {
        let chunk = reader
            .read_entity_chunk(pos.x, pos.z)?
            .context("entity chunk missing")?;
        entities.insert(pos, chunk.document);
    }
    let mut pois = BTreeMap::new();
    for &pos in &poi_positions {
        let chunk = reader
            .read_poi_chunk(pos.x, pos.z)?
            .context("POI chunk missing")?;
        pois.insert(pos, chunk.document);
    }

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
    let java = std::env::var_os("MINECRAFT_JAVA_HOME")
        .or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join(
                "AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon",
            )
        })
        .join("bin");
    let java_source = temp.path().join("GeneratedWorldJavaOracle.java");
    std::fs::write(&java_source, include_str!("GeneratedWorldJavaOracle.java"))?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&classpath)
        .arg(&java_source)
        .output()?;
    ensure!(
        compiled.status.success(),
        "Java compilation failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let files = ["terrain.txt", "entities.txt", "poi.txt"].map(|name| temp.path().join(name));
    for (file, positions) in files
        .iter()
        .zip([&positions, &entity_positions, &poi_positions])
    {
        std::fs::write(
            file,
            positions
                .iter()
                .map(|pos| format!("{} {}\n", pos.x, pos.z))
                .collect::<String>(),
        )?;
    }
    let checked = Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(classpath)
        .arg("GeneratedWorldJavaOracle")
        .arg(&world_path)
        .arg(seed.to_string())
        .args(spawn.position.map(|value| value.to_string()))
        .args(player_block.to_array().map(|value| value.to_string()))
        .args(&files)
        .arg(rules.random_tick_speed.to_string())
        .output()?;
    ensure!(
        checked.status.success(),
        "Java world check failed: {} {}",
        String::from_utf8_lossy(&checked.stdout),
        String::from_utf8_lossy(&checked.stderr)
    );
    let output = std::str::from_utf8(&checked.stdout)?;
    ensure!(
        output.lines().any(|line| line
            == format!(
                "OK {} {} {}",
                positions.len(),
                entity_positions.len(),
                poi_positions.len()
            )),
        "Java world check returned no result"
    );
    let counts = output
        .lines()
        .find_map(|line| line.strip_prefix("STRUCTURES "))
        .context("Java structure counts missing")?
        .split_whitespace()
        .map(str::parse::<usize>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    ensure!(
        counts.len() == 4 && counts[0] > 0 && counts[1] > 0,
        "fixture did not exercise a portal and reference: {counts:?}"
    );

    let mut resources = native_resources::packs::ResourceStack::open(&game)?;
    let defaults = crate::vanilla_world_block_defaults(&mut resources)?;
    let materialize = |position, chunk: native_persistence::world::LoadedChunk| {
        native_persistence::world::ChunkArea {
            center: position,
            radius: 0,
            chunks: BTreeMap::from([(position, Arc::new(chunk))]),
            missing: Vec::new(),
        }
        .materialize_with_defaults(&dimension, &defaults)
    };
    let reader = &mut native_persistence::world::WorldReader::open(&world_path, dimension.clone())?;
    ensure!(
        reader.saved_world_seed()? == Some(seed)
            && reader.read_player()?.as_ref() == Some(&player)
            && reader.world_clocks()? == clocks
            && reader.fluid_game_rules()? == rules,
        "world settings or player changed"
    );
    ensure!(
        saved_terrain_positions(&sidecars.join("region"))? == positions
            && saved_region_positions(&sidecars.join("entities"))? == entity_positions
            && saved_region_positions(&sidecars.join("poi"))? == poi_positions,
        "saved chunk set changed"
    );
    for (pos, original) in entities {
        let after = reader
            .read_entity_chunk(pos.x, pos.z)?
            .context("rewritten entity chunk missing")?
            .document;
        ensure!(
            original.name == after.name && same_nbt(&original.value, &after.value),
            "entity chunk {pos:?} changed"
        );
    }
    for (pos, original) in pois {
        let after = reader
            .read_poi_chunk(pos.x, pos.z)?
            .context("rewritten POI chunk missing")?
            .document;
        ensure!(
            original.name == after.name && same_nbt(&original.value, &after.value),
            "POI chunk {pos:?} changed"
        );
    }
    for (pos, before) in chunks {
        let after = reader
            .read_chunk(pos.x, pos.z)?
            .context("rewritten chunk missing")?;
        ensure!(
            before.status == after.status
                && before.document.name == after.document.name
                && same_chunk_metadata(&before.document.value, &after.document.value)
                && before.block_light_correct() == after.block_light_correct(),
            "chunk {pos:?} metadata changed"
        );
        let before_world = materialize(pos, before.clone())?;
        let after_world = materialize(pos, after.clone())?;
        for y in -80..336 {
            for z in pos.z * 16..pos.z * 16 + 16 {
                for x in pos.x * 16..pos.x * 16 + 16 {
                    ensure!(
                        before_world.block(x, y, z) == after_world.block(x, y, z),
                        "block changed at {x} {y} {z}"
                    );
                    ensure!(
                        before.biome(x, y, z) == after.biome(x, y, z),
                        "biome changed at {x} {y} {z}"
                    );
                    for sky in [false, true] {
                        ensure!(
                            before.light(x, y, z, sky) == after.light(x, y, z, sky),
                            "light changed at {x} {y} {z}, sky={sky}"
                        );
                    }
                }
            }
        }
    }
    eprintln!(
        "Java round-tripped {} terrain, {} entity and {} POI chunks; portal counts {counts:?}",
        positions.len(),
        entity_positions.len(),
        poi_positions.len()
    );
    Ok(())
}
