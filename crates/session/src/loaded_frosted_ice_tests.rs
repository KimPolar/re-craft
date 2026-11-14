use super::{
    tests::{fixture_chunk_document, session, test_heightmap_tags},
    *,
};
use native_persistence::nbt::NamedTag;

fn pending_ice_fixture() -> (LoadedSession, ChunkPos, Vec<SavedBlockTick>, NamedTag) {
    let mut session = session();
    let home = session.player_chunk().unwrap();
    let position = (session.origin + IVec3::new(8, 5, 8)).to_array();
    let ticks = vec![
        SavedBlockTick {
            block: "minecraft:frosted_ice".into(),
            position,
            delay: 20,
            priority: -1,
        },
        SavedBlockTick {
            block: "minecraft:sand".into(),
            position,
            delay: 3,
            priority: 0,
        },
        SavedBlockTick {
            block: "minecraft:frosted_ice".into(),
            position,
            delay: 5,
            priority: 1,
        },
        SavedBlockTick {
            block: "minecraft:water".into(),
            position,
            delay: 6,
            priority: -2,
        },
    ];
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
    session.enqueue_saved_sand(&ticks);
    session.enqueue_saved_ice(&ticks);
    session.enqueue_saved_bubble(&ticks);
    (session, home, ticks, source)
}

#[test]
fn pending_ice_ticks_preserve_duplicates_and_cross_kind_order() {
    let (mut session, home, ticks, source) = pending_ice_fixture();
    assert_eq!(session.ice_ticks.len(), 2);
    assert_eq!(session.ice_pending.len(), 1);
    assert_eq!(session.draft_block_ticks(home).unwrap(), ticks);

    // A deferred tick retains its original sub-tick order and delay.
    let (order, key) = session.ice_ticks.pop_first().unwrap();
    session.ice_deferred.insert(order, key);
    session.mark_tick_chunk_dirty(home.x * 16, home.z * 16);
    let draft = session
        .draft_dirty_chunk_with_heightmaps_and_simulated_ticks(
            home,
            &source,
            &test_heightmap_tags(),
            |_| anyhow::bail!("tick-only draft must not rebuild heightmaps"),
        )
        .unwrap();
    assert!(draft.matches_source(&source).unwrap());
    assert_eq!(
        draft.document().value.get("block_ticks"),
        source.value.get("block_ticks")
    );

    let missing = (key.0 + 1, key.1, key.2);
    session.ice_pending.insert(missing);
    assert!(session.draft_block_ticks(home).is_err());
    session.ice_pending.remove(&missing);
    session.unresolved_ice_tick = true;
    assert!(session.draft_block_ticks(home).is_err());
}

#[test]
fn unavailable_and_stale_ice_ticks_do_not_discard_saved_duplicates() {
    let mut session = session();
    session.set_random_tick_speed(0);
    let home = session.player_chunk().unwrap();
    let relative = IVec3::new(8, 5, 8);
    let absolute = session.origin + relative;
    let first = SavedBlockTick {
        block: "minecraft:frosted_ice".into(),
        position: absolute.to_array(),
        delay: 0,
        priority: 0,
    };
    let second = SavedBlockTick {
        delay: 10,
        ..first.clone()
    };
    let ticks = vec![first.clone(), second.clone()];
    Arc::make_mut(&mut session.world)
        .saved_block_ticks
        .insert(home, ticks.clone());
    session.enqueue_saved_ice(&ticks);
    let chunk = Arc::make_mut(&mut session.world)
        .chunks
        .remove(&home)
        .unwrap();

    session.advance_block_ticks();
    assert!(!session.unresolved_ice_tick);
    assert_eq!(session.ice_deferred.len(), 1);
    assert_eq!(session.draft_block_ticks(home).unwrap(), ticks);
    session.tick = 3;
    let mut overdue = first;
    overdue.delay = -3;
    let mut later = second;
    later.delay = 7;
    assert_eq!(
        session.draft_block_ticks(home).unwrap(),
        vec![overdue, later.clone()]
    );

    // The restored block is air, so Java discards the expired tick without
    // invoking FrostedIceBlock.tick. The later duplicate remains scheduled.
    Arc::make_mut(&mut session.world).chunks.insert(home, chunk);
    session.revision = session.revision.wrapping_add(1);
    session.advance_block_ticks();
    assert!(!session.unresolved_ice_tick);
    assert!(session.ice_deferred.is_empty());
    assert!(session.ice_pending.is_empty());
    assert_eq!(
        session.draft_block_ticks(home).unwrap(),
        vec![later.clone()]
    );
    session.schedule_ice(relative, 20);
    assert_eq!(session.ice_ticks.len(), 2);
    assert_eq!(session.ice_pending.len(), 1);
    assert_eq!(
        session.draft_block_ticks(home).unwrap(),
        vec![
            later,
            SavedBlockTick {
                block: "minecraft:frosted_ice".into(),
                position: absolute.to_array(),
                delay: 20,
                priority: 0,
            }
        ]
    );
}

#[test]
fn executing_frosted_ice_still_blocks_partial_save() {
    let mut session = session();
    let home = session.player_chunk().unwrap();
    let relative = IVec3::new(8, 5, 8);
    let ice = session.intern_block(state("minecraft:frosted_ice", &[("age", "0")]));
    session.applying_ice_tick = true;
    assert!(session.set(relative, ice));
    session.applying_ice_tick = false;
    session.enqueue_saved_ice(&[SavedBlockTick {
        block: "minecraft:frosted_ice".into(),
        position: (session.origin + relative).to_array(),
        delay: 0,
        priority: 0,
    }]);
    session.advance_block_ticks();
    assert!(session.unresolved_ice_tick);
    let error = session.draft_block_ticks(home).unwrap_err();
    assert!(error.to_string().contains("frosted ice"), "{error:#}");
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn pending_frosted_ice_ticks_round_trip_in_java() {
    use std::{path::PathBuf, process::Command};

    let (mut session, home, _, source) = pending_ice_fixture();
    session.mark_tick_chunk_dirty(home.x * 16, home.z * 16);
    let draft = session
        .draft_dirty_chunk_with_heightmaps_and_simulated_ticks(
            home,
            &source,
            &test_heightmap_tags(),
            |_| anyhow::bail!("tick-only draft must not rebuild heightmaps"),
        )
        .unwrap();
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let version = game.join("versions/26.3");
    let metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for library in metadata["libraries"].as_array().unwrap() {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let java = std::env::var_os("MINECRAFT_JAVA_HOME")
        .or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let oracle = temp.path().join("FrostedIceSavedTickOracle.java");
    let nbt = temp.path().join("chunk.nbt");
    std::fs::write(
        &oracle,
        include_str!("../tests/java/FrostedIceSavedTickOracle.java"),
    )
    .unwrap();
    std::fs::write(
        &nbt,
        native_persistence::nbt::write(draft.document()).unwrap(),
    )
    .unwrap();
    for command in [
        Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&oracle),
        Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("FrostedIceSavedTickOracle")
            .arg(&nbt),
    ] {
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
