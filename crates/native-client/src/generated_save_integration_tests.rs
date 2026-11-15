//! End-to-end save checks against a privately published generated world.
use std::{
    collections::BTreeSet,
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use native_persistence::world::{DimensionId, RegionStore, SavedWorldBorder, WorldReader};
use native_session::{GameSession, PlayerCommand};
use native_world::{
    fluid::FluidState,
    loaded::{BlockPosition, BlockState, ChunkPos},
};

use crate::generation_pipeline::{InitialWorldOptions, InitialWorldPipeline, InitialWorldResult};

/// Keep this private test world separate from the user's actual saves.
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and long-running world generation"]
fn generated_portal_world_preserves_remote_chunks_after_play_save() -> Result<()> {
    verify_generated_portal_play_save(0)
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and long-running world generation"]
fn generated_portal_world_saves_after_forty_ticks_without_random_ticks() -> Result<()> {
    verify_generated_portal_play_save(40)
}

fn verify_generated_portal_play_save(additional_ticks: u64) -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let test_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/region-test-temp");
    std::fs::create_dir_all(&test_root)?;
    let parent = tempfile::Builder::new()
        .prefix("portal-play-save-")
        .tempdir_in(test_root)?;
    let parent_path = parent.path().to_path_buf();
    let mut settings = crate::world_creation::WorldCreationSettings::new("Test".into());
    settings.seed = crate::chat_input::ChatInput::field("-1");
    if additional_ticks != 0 {
        // Keep this test focused on scheduled fluid and block ticks. Default
        // random ticks still need complete sky-light and neighboring chunks.
        settings.game_rules.random_tick_speed = 0;
    }
    let targets = BTreeSet::from([
        ChunkPos { x: 20, z: 19 },
        ChunkPos { x: 21, z: 19 },
        ChunkPos { x: 28, z: 24 },
    ]);
    let options = InitialWorldOptions {
        staging_parent: parent_path.clone(),
        border: SavedWorldBorder::default(),
        player_random_seed: 1234,
        ceiling_spawn_height: None,
        mesh_revision: 1,
        biome_blend: 0,
        last_played_ms: 123,
        region_timestamp: 1,
    };
    let mut pipeline = InitialWorldPipeline::start_with_additional_targets(
        settings.freeze(game.clone(), None),
        options,
        targets.clone(),
    )?;
    let start = Instant::now();
    let deadline = start + Duration::from_secs(900);
    let mut next_report = start + Duration::from_secs(30);
    let world = loop {
        match pipeline.poll() {
            Some(InitialWorldResult::Ready {
                generation,
                world,
                player_area,
            }) => {
                let area = player_area?;
                ensure!(
                    area.missing.is_empty() && area.deferred.is_empty(),
                    "initial player area is incomplete"
                );
                let processed = generation
                    .postprocessed
                    .as_ref()
                    .context("postprocessing missing")?;
                ensure!(
                    targets.is_subset(&processed.region.chunks),
                    "required chunks were not postprocessed"
                );
                break world;
            }
            Some(InitialWorldResult::Failed { error, .. }) => return Err(error),
            Some(InitialWorldResult::Cancelled { .. }) => {
                anyhow::bail!("initial generation cancelled")
            }
            None => {}
        }
        let now = Instant::now();
        if now >= next_report {
            eprintln!(
                "portal play-save after {:?}: {:?}",
                now - start,
                pipeline.progress()
            );
            next_report = now + Duration::from_secs(30);
        }
        ensure!(now < deadline, "initial world generation timed out");
        std::thread::sleep(Duration::from_millis(20));
    };
    let (_, stage) = world.into_parts();
    let published = stage.publish(&parent_path, "Test", "New World")?;
    let path = published.path.clone();
    let lock = published.session().clone();
    verify_portal_play_save(&path, &game, lock, additional_ticks)
}

fn verify_portal_play_save(
    path: &std::path::Path,
    game: &std::path::Path,
    lock: native_persistence::save::WorldSessionGuard,
    additional_ticks: u64,
) -> Result<()> {
    let options = crate::ViewOptions {
        dimension: None,
        chunk: None,
        radius: 1,
        smoke_test: false,
        block_report: None,
        play: true,
        video: crate::menu_settings::VideoSettings::default(),
    };
    let crate::PreparedWorld::Play(prepared) =
        crate::prepare_world_with_session(&path, &game, &[], options, None, Some(lock.clone()))?
    else {
        anyhow::bail!("generated world did not open for play")
    };
    let crate::PreparedPlayWorld {
        mut session,
        mut resources,
        mut streamer,
        ..
    } = *prepared;
    let mut reader =
        WorldReader::open_with_session(&path, DimensionId::parse("overworld")?, lock.clone())?;
    let portal = reader
        .read_chunk(20, 19)?
        .context("portal start chunk missing")?;
    let Some(native_persistence::nbt::Tag::Compound(starts)) = portal
        .document
        .value
        .get("structures")
        .and_then(|structures| structures.get("starts"))
    else {
        anyhow::bail!("portal structure starts missing")
    };
    ensure!(
        starts.iter().any(|(name, start)| {
            name.to_string_lossy().starts_with("minecraft:ruined_portal")
                && matches!(start.get("Children"), Some(native_persistence::nbt::Tag::List { values, .. }) if !values.is_empty())
        }),
        "generated portal start is missing or has no pieces"
    );
    let portal_before = native_persistence::nbt::write(&portal.document)?;
    let mut remote_entities_before = std::collections::BTreeMap::new();
    // Entity regions can include neighbors outside the postprocessed set.
    for saved in reader.saved_chunk_positions(RegionStore::Entities)? {
        let pos = ChunkPos {
            x: saved.x,
            z: saved.z,
        };
        if !session.has_resident_or_parked_chunk(pos)
            && let Some(chunk) = reader.read_entity_chunk(pos.x, pos.z)?
            && !chunk.entities().is_empty()
        {
            remote_entities_before.insert(pos, native_persistence::nbt::write(&chunk.document)?);
        }
    }
    ensure!(
        !remote_entities_before.is_empty(),
        "no distant entity chunk was available to verify"
    );
    eprintln!(
        "checking {} distant entity chunks",
        remote_entities_before.len()
    );

    // Normal ticks exercise the same world rules as the eventual GUI.
    let before = session.snapshot();
    session.submit_command(PlayerCommand::Look {
        yaw: 0.25,
        pitch: -0.1,
    });
    session.submit_command(PlayerCommand::Move {
        forward: 1.0,
        strafe: 0.0,
        jump: false,
    });
    session.update(Duration::from_millis(50));
    session.submit_command(PlayerCommand::Move {
        forward: 0.0,
        strafe: 0.0,
        jump: false,
    });
    session.update(Duration::from_millis(50));
    for _ in 0..additional_ticks {
        // Keep disk streaming responsive without advancing simulation faster
        // than the game's 20 TPS while waiting for its next tick.
        let next_tick = Instant::now() + Duration::from_millis(50);
        loop {
            streamer.service(&mut session)?;
            let now = Instant::now();
            if now >= next_tick {
                break;
            }
            std::thread::sleep((next_tick - now).min(Duration::from_millis(5)));
        }
        session.update(Duration::from_millis(50));
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut next_tick = Instant::now() + Duration::from_millis(50);
    while !session.required_shape_update_chunks().is_empty() {
        let required = session.required_shape_update_chunks();
        for pos in required
            .iter()
            .filter(|pos| !session.has_resident_or_parked_chunk(**pos))
        {
            ensure!(
                reader
                    .read_chunk(pos.x, pos.z)?
                    .is_some_and(|chunk| chunk.is_full()),
                "required shape dependency is not generated: {pos:?}"
            );
        }
        ensure!(
            Instant::now() < deadline,
            "shape updates did not settle: {required:?}"
        );
        streamer.service(&mut session)?;
        if Instant::now() >= next_tick {
            session.update(Duration::from_millis(50));
            next_tick += Duration::from_millis(50);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(streamer);
    let expected = session.snapshot();
    ensure!(
        expected.tick >= before.tick + 2 + additional_ticks,
        "not all requested gameplay ticks were processed"
    );
    let before_position = before.origin.as_vec3() + before.player.position;
    let expected_position = expected.origin.as_vec3() + expected.player.position;
    ensure!(
        (expected_position - before_position).length() > 0.0001,
        "the player did not move"
    );
    ensure!(
        expected.player.yaw != before.player.yaw || expected.player.pitch != before.player.pitch,
        "the player's look did not change"
    );
    let mut frozen = session
        .freeze_partial_save()
        .map_err(|error| error.into_parts().1)?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while !frozen.poll_lighting().is_settled() {
        ensure!(
            Instant::now() < deadline,
            "saved-world lighting did not settle"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let sources = reader.capture_session_save_sources(frozen.source_positions().iter().copied())?;
    let block_ids = resources.vanilla_block_ids()?;
    let tags = native_resources::heightmap_tags::HeightmapTags::load(&mut resources, &block_ids)?;
    let registry = resources.vanilla_generated_block_registry()?;
    let resolve = |saved: &native_persistence::palette::BlockState| -> Result<BlockState> {
        let name = String::from_utf16(&saved.name.0)?;
        let mut supplied = std::collections::BTreeMap::new();
        for (key, value) in &saved.properties {
            ensure!(
                supplied
                    .insert(String::from_utf16(&key.0)?, String::from_utf16(&value.0)?)
                    .is_none(),
                "duplicate block property"
            );
        }
        let (name, properties) =
            registry.resolve(&serde_json::json!({"id": name, "properties": supplied}))?;
        ensure!(
            supplied
                .iter()
                .all(|(key, value)| properties.get(key) == Some(value)),
            "unsupported block property"
        );
        Ok(BlockState { name, properties })
    };
    let plan = frozen.session().draft_captured_partial_save(
        &sources,
        &tags,
        |saved| {
            let state = resolve(saved)?;
            Ok((
                matches!(
                    state.name.as_str(),
                    "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                ),
                FluidState::from_block(&state)
                    .map_err(anyhow::Error::msg)?
                    .is_some(),
            ))
        },
        &resolve,
        |state| {
            ensure!(
                registry.is_canonical_state(&state.name, &state.properties),
                "unsupported block state"
            );
            Ok(())
        },
        |_| anyhow::bail!("unexpected POI change"),
        1,
    );
    let plan = match plan {
        Ok(plan) => plan,
        Err(error) => {
            if let Err(diagnostic) = report_pending_postprocessing(&mut reader, frozen.session()) {
                eprintln!("could not inspect saved postprocessing: {diagnostic:#}");
            }
            return Err(error.context("cannot draft the played generated world"));
        }
    };
    let mut edits: std::collections::BTreeMap<_, _> = expected
        .world
        .loaded()
        .context("loaded world missing")?
        .block_edits()
        .map(|edit| (edit.position, edit.clone()))
        .collect();
    let mut written_edits = Vec::new();
    drop(reader);
    let mut writer = lock.acquire_write()?;
    writer.write_plan(plan, |dimension, change| {
        ensure!(
            *dimension == DimensionId::parse("overworld")?,
            "wrong dimension"
        );
        let pos = BlockPosition {
            x: change.x,
            y: change.y,
            z: change.z,
        };
        let edit = edits.remove(&pos).context("unexpected saved block edit")?;
        ensure!(
            edit.original.as_ref() == Some(&resolve(&change.expected)?),
            "original block changed"
        );
        ensure!(
            edit.replacement == resolve(&change.replacement)?,
            "saved block changed"
        );
        written_edits.push((pos, edit.replacement));
        Ok(())
    })?;
    ensure!(edits.is_empty(), "saved plan omitted block edits");
    drop(writer);
    drop(frozen);
    let mut reader = WorldReader::open_with_session(&path, DimensionId::parse("overworld")?, lock)?;
    let portal_after = reader.read_chunk(20, 19)?.context("portal chunk lost")?;
    assert_eq!(
        native_persistence::nbt::write(&portal_after.document)?,
        portal_before
    );
    for (pos, before) in remote_entities_before {
        let after = reader
            .read_entity_chunk(pos.x, pos.z)?
            .context("distant entity chunk was lost")?;
        assert_eq!(
            native_persistence::nbt::write(&after.document)?,
            before,
            "distant entity chunk changed at {pos:?}"
        );
    }
    let mut edited_chunks = std::collections::BTreeMap::new();
    for (position, expected_state) in &written_edits {
        let pos = ChunkPos {
            x: position.x.div_euclid(16),
            z: position.z.div_euclid(16),
        };
        if let std::collections::btree_map::Entry::Vacant(entry) = edited_chunks.entry(pos) {
            entry.insert(
                reader
                    .read_chunk(pos.x, pos.z)?
                    .context("saved edited chunk missing")?,
            );
        }
        let actual = edited_chunks[&pos]
            .block(position.x, position.y, position.z)
            .context("saved edited block missing")?;
        ensure!(
            resolve(actual)? == *expected_state,
            "saved block differs at {position:?}"
        );
    }
    eprintln!("reopened {} edited blocks", written_edits.len());
    let player = reader.read_player()?.context("saved player missing")?;
    let camera = crate::saved_camera(&player)?.context("saved player position missing")?;
    for (actual, expected) in camera
        .position
        .into_iter()
        .zip(expected_position.to_array())
    {
        assert!((actual - f64::from(expected)).abs() < 0.0001);
    }
    assert_eq!(camera.yaw, expected.player.yaw.to_degrees() - 180.0);
    assert_eq!(camera.pitch, -expected.player.pitch.to_degrees());
    assert_eq!(reader.game_time()?, expected.game_time as i64);
    Ok(())
}

/// Report actual saved marks and nearby chunk coverage when an end-to-end
/// save fails. This inspects only the private world created by the test.
fn report_pending_postprocessing(
    reader: &mut WorldReader,
    session: &native_session::loaded::LoadedSession,
) -> Result<()> {
    for position in reader.saved_chunk_positions(RegionStore::Terrain)? {
        let position = ChunkPos {
            x: position.x,
            z: position.z,
        };
        if !session.has_resident_or_parked_chunk(position) {
            continue;
        }
        let Some(chunk) = reader.read_chunk(position.x, position.z)? else {
            continue;
        };
        let Some(tag) = chunk.document.value.get("PostProcessing") else {
            continue;
        };
        if matches!(
            tag,
            native_persistence::nbt::Tag::List {
                element_type: native_persistence::nbt::TagType::End
                    | native_persistence::nbt::TagType::List,
                values,
            } if values.is_empty()
        ) {
            continue;
        }
        let marks = native_persistence::postprocessing::decode(tag, -64, 384)?;
        let count: usize = marks.sections().iter().map(Vec::len).sum();
        if count == 0 {
            continue;
        }
        let mut disk_full = 0;
        let mut session_full = 0;
        for dz in -1..=1 {
            for dx in -1..=1 {
                let neighbor = ChunkPos {
                    x: position.x + dx,
                    z: position.z + dz,
                };
                if reader
                    .read_chunk(neighbor.x, neighbor.z)?
                    .is_some_and(|chunk| chunk.is_full())
                {
                    disk_full += 1;
                }
                session_full += usize::from(session.has_resident_or_parked_chunk(neighbor));
            }
        }
        let marks_ref = &marks;
        let samples: Vec<_> = marks
            .sections()
            .iter()
            .enumerate()
            .flat_map(|(section, values)| {
                values.iter().filter_map(move |&packed| {
                    marks_ref.unpack(section, packed, position.x, position.z)
                })
            })
            .take(8)
            .map(|[x, y, z]| {
                let state = chunk
                    .block(x, y, z)
                    .map(|state| state.name.to_string_lossy())
                    .unwrap_or_else(|| "<missing>".into());
                ((x, y, z), state)
            })
            .collect();
        eprintln!(
            "pending postprocessing {position:?}: {count} marks, 3x3 disk FULL={disk_full}, session loaded={session_full}, samples={samples:?}"
        );
    }
    Ok(())
}
