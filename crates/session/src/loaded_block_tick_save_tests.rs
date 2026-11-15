use super::{
    tests::{fixture_chunk_document, session, test_heightmap_tags},
    *,
};

fn fixture() -> (
    LoadedSession,
    ChunkPos,
    Vec<SavedBlockTick>,
    native_persistence::nbt::NamedTag,
) {
    let mut session = session();
    let home = session.player_chunk().unwrap();
    let position = (session.origin + IVec3::new(8, 5, 8)).to_array();
    let ticks = [
        ("minecraft:cave_air", 0, -1),
        ("minecraft:pointed_dripstone", 30, 1),
        ("minecraft:sand", 3, 0),
        ("minecraft:void_air", 5, -2),
        ("minecraft:sulfur_spike", 40, 2),
        ("minecraft:air", 10, 0),
    ]
    .map(|(block, delay, priority)| SavedBlockTick {
        block: block.into(),
        position,
        delay,
        priority,
    })
    .to_vec();
    let mut source = native_persistence::edit::draft_chunk_blocks(
        &fixture_chunk_document(home),
        native_persistence::world::ChunkPos {
            x: home.x,
            z: home.z,
        },
        &[],
    )
    .unwrap();
    source.replace_block_ticks(&ticks).unwrap();
    let source = source.document().clone();
    Arc::make_mut(&mut session.world)
        .saved_block_ticks
        .insert(home, ticks.clone());
    session.enqueue_saved_air(&ticks);
    session.enqueue_saved_speleothems(&ticks);
    session.enqueue_saved_sand(&ticks);
    (session, home, ticks, source)
}

#[test]
fn mixed_air_and_speleothem_ticks_survive_session_chunk_drafting() {
    let (mut session, home, ticks, source) = fixture();
    assert_eq!(session.draft_block_ticks(home).unwrap(), ticks);
    session.advance_block_ticks();
    assert_eq!(session.draft_block_ticks(home).unwrap(), ticks[1..]);
    let draft = session
        .draft_dirty_chunk_with_heightmaps_and_simulated_ticks(
            home,
            &source,
            &test_heightmap_tags(),
            |_| anyhow::bail!("tick-only draft must not rebuild heightmaps"),
        )
        .unwrap();
    let mut expected = native_persistence::edit::draft_chunk_blocks(
        &source,
        native_persistence::world::ChunkPos {
            x: home.x,
            z: home.z,
        },
        &[],
    )
    .unwrap();
    expected.replace_block_ticks(&ticks[1..]).unwrap();
    assert_eq!(
        draft.document().value.get("block_ticks"),
        expected.document().value.get("block_ticks")
    );
    assert!(draft.matches_source(&source).unwrap());
}

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn mixed_air_and_speleothem_ticks_round_trip_in_java() -> anyhow::Result<()> {
    use anyhow::{Context, ensure};
    use std::{path::PathBuf, process::Command};

    let (mut session, home, ticks, source) = fixture();
    session.advance_block_ticks();
    let draft = session.draft_dirty_chunk_with_heightmaps_and_simulated_ticks(
        home,
        &source,
        &test_heightmap_tags(),
        |_| anyhow::bail!("tick-only draft must not rebuild heightmaps"),
    )?;
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let version = game.join("versions/26.3");
    let metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    ensure!(metadata["id"] == "26.3", "wrong Minecraft version");
    let temp = tempfile::tempdir()?;
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for library in metadata["libraries"]
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
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let oracle = temp.path().join("AirSpeleothemSavedTickOracle.java");
    let nbt = temp.path().join("chunk.nbt");
    std::fs::write(
        &oracle,
        include_str!("../tests/java/AirSpeleothemSavedTickOracle.java"),
    )?;
    std::fs::write(&nbt, native_persistence::nbt::write(draft.document())?)?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(oracle)
        .output()?;
    ensure!(
        compiled.status.success(),
        "Java tick oracle did not compile: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(classpath)
        .arg("AirSpeleothemSavedTickOracle")
        .arg(nbt)
        .args(ticks[0].position.map(|coordinate| coordinate.to_string()))
        .output()?;
    ensure!(
        output.status.success(),
        "Java tick comparison failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
