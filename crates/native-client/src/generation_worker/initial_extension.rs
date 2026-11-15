//! Extend an uncommitted initial generation without discarding completed work.
use super::*;

/// Resources used to recreate the initial generation's immutable context.
pub(crate) struct InitialExtensionResources {
    pub minecraft_dir: std::path::PathBuf,
    pub preset: String,
    pub dimension: String,
    pub selection: Option<native_resources::datapacks::DataPackSettings>,
}

impl GenerationWorker {
    /// Generate only the requested additional chunks before initial-world
    /// staging. The input must still own its original generation context; this
    /// does not attach chunks to a live session or authorize publication.
    pub(crate) fn extend_postprocessed_initial(
        mut ready: GenerationReady,
        targets: BTreeSet<ChunkPos>,
        player_random_seed: i64,
        resources: InitialExtensionResources,
    ) -> Result<Self> {
        ensure!(
            !targets.is_empty(),
            "no additional initial generation targets"
        );
        ensure!(
            ready.batch.supports_structure_expansion(),
            "structure-aware generation context is unavailable"
        );
        ensure!(
            ready.postprocessed.is_some() && ready.hut_generation.is_some(),
            "original postprocessed generation context is unavailable"
        );
        let spawn = ready
            .player_spawn
            .context("initial player spawn is missing")?;
        let initial = ready
            .initial_respawn
            .clone()
            .context("initial respawn settings are missing")?;
        let level = ready
            .level_settings
            .as_ref()
            .context("initial level settings are missing")?;
        let rules = level.game_rules;
        ensure!(
            initial.radius == rules.respawn_radius
                && initial.adventure == (level.session.mode == native_session::GameMode::Adventure),
            "initial player settings changed"
        );
        let clocks = Arc::clone(
            ready
                .initial_clocks
                .as_ref()
                .context("initial clocks are missing")?,
        );
        ensure!(
            clocks.spawn_mobs == rules.spawn_mobs && clocks.advance_time == rules.advance_time,
            "initial clock rules changed"
        );
        ensure!(
            ready
                .postprocessed
                .as_ref()
                .context("postprocessing is missing")?
                .region
                .water_source_conversion()
                == Some(rules.water_source_conversion),
            "initial postprocessing rules changed"
        );
        let border = native_session::respawn::RespawnBorder::from_saved(initial.border, 0)?;
        let disabled = if rules.spawn_mobs {
            ready.batch.mob_generation_disabled()?
        } else {
            false
        };
        let player = InitialPlayerSpawnOptions {
            radius: initial.radius,
            adventure: initial.adventure,
            random_seed: player_random_seed,
            saved_border: Some(initial.border),
        };
        Self::spawn(move |cancelled, progress| {
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let checkpoint = PlayerSearchCheckpoint::capture(&ready)?;
            let old_edits = ready
                .postprocessed
                .as_ref()
                .context("postprocessing is missing")?
                .region
                .edits
                .clone();
            let mut provider =
                native_resources::packs::ResourceStack::open(&resources.minecraft_dir)?;
            match (&ready.pack_snapshot, &resources.selection) {
                (Some(snapshot), Some(selection)) => {
                    snapshot.verify()?;
                    let warnings =
                        provider.configure_snapshot_data(Arc::clone(snapshot), selection)?;
                    ensure!(
                        warnings.is_empty(),
                        "data pack selection needs attention: {}",
                        warnings.join("; ")
                    );
                }
                (None, None) => {}
                _ => anyhow::bail!("generation data pack snapshot does not match frozen settings"),
            }
            let current = provider.data_pack_snapshot();
            ensure!(
                match (&ready.pack_snapshot, &current) {
                    (Some(expected), Some(actual)) => Arc::ptr_eq(expected, actual),
                    (None, None) => true,
                    _ => false,
                },
                "extension requires the original data-pack snapshot"
            );
            if let Some(snapshot) = &current {
                snapshot.verify()?;
            }
            let context = crate::fresh_generation::structured_initial_context(
                &mut provider,
                &resources.preset,
                &resources.dimension,
                border,
                rules.spawn_mobs,
                disabled,
                rules.advance_time,
            )?;
            ensure!(
                context.clocks.ticks == clocks.ticks,
                "initial generation clocks changed"
            );
            let creatures = context.creatures;
            let blocks = provider.vanilla_block_ids()?;
            let height_tags = Arc::new(native_resources::heightmap_tags::HeightmapTags::load(
                &mut provider,
                &blocks,
            )?);
            ensure!(
                *height_tags == *initial.height_tags,
                "initial height tags changed"
            );
            let registry = Arc::new(provider.vanilla_generated_block_registry()?);
            let (mut classify, mut resolve) =
                crate::fresh_generation::block_classifiers(Arc::clone(&registry));
            let mut height =
                crate::fresh_generation::height_classifier(registry, Arc::clone(&height_tags));
            let mut build = move |batch: &FreshTerrainBatch,
                                  provider: &mut native_resources::packs::ResourceStack,
                                  pos| {
                creatures
                    .as_ref()
                    .context("creature spawn inputs are unavailable")?
                    .build_spawner(batch, provider, pos)
            };
            let mut construct = |id: &ResourceId| {
                Ok(native_session::worldgen::FreshCreatureRandom::from_entropy(id)?.construction)
            };
            ready.player_spawn = None;
            let biome_blend = ready.mesh.biome_blend;
            let Some(mut expanded) = complete_player_spawn(
                ready,
                &mut provider,
                &mut classify,
                &mut resolve,
                &mut build,
                &mut construct,
                rules.spawn_mobs,
                Arc::clone(&height_tags),
                Some(&mut height),
                biome_blend,
                initial.ceiling_spawn_height,
                border,
                player,
                &targets,
                cancelled,
                progress,
            )?
            else {
                return Ok(None);
            };
            ensure!(
                expanded.player_spawn == Some(spawn),
                "initial player spawn changed during extension"
            );
            validate_previous_targets(&checkpoint, &old_edits, &expanded)?;
            let precise = spawn.as_dvec3() - expanded.mesh.origin.as_dvec3()
                + glam::DVec3::new(0.5, 0.0, 0.5);
            let position = precise.as_vec3();
            ensure!(
                position.as_dvec3() == precise,
                "initial player position cannot be represented exactly"
            );
            let player = native_session::PlayerState {
                position,
                yaw: 0.0,
                pitch: 0.0,
                vertical_velocity: 0.0,
                on_ground: false,
                pose: native_session::PlayerPose::Standing,
            };
            let session = expanded.prepare_session_with_spawn_heightmaps(player, height_tags)?;
            let complete = expanded
                .mesh
                .targets
                .as_deref()
                .context("generated targets are missing")?;
            ensure!(
                session.generated_spawn_clear(spawn, complete)?,
                "initial player spawn became obstructed"
            );
            if let Some(snapshot) = current {
                snapshot.verify()?;
            }
            Ok(Some(expanded))
        })
    }
}

/// An adjacent FULL conversion may legitimately update an older chunk. Check
/// that all block changes are in the appended postprocessing history, and keep
/// the prior entity records. Light-only sections may also be added or removed.
fn validate_previous_targets(
    checkpoint: &PlayerSearchCheckpoint,
    old_edits: &[native_world::loaded::BlockPosition],
    ready: &GenerationReady,
) -> Result<()> {
    let processed = ready
        .postprocessed
        .as_ref()
        .context("postprocessing is missing")?;
    ensure!(
        processed.region.edits.starts_with(old_edits),
        "postprocessing history changed during extension"
    );
    let targets = ready
        .mesh
        .targets
        .as_deref()
        .context("generated targets are missing")?;
    ensure!(
        checkpoint
            .terrain
            .heightmaps
            .keys()
            .all(|pos| targets.contains(pos)),
        "previous generation target is missing"
    );
    ensure!(
        checkpoint
            .entities_match(|pos| ready.batch.complete_generated_entity_records_shared(pos))?,
        "previous generated entities changed during extension"
    );
    let before = &checkpoint.terrain.world;
    let after = &processed.region.world;
    ensure!(
        before.dimension == after.dimension
            && before.dimension_type == after.dimension_type
            && before.inline_dimension_type == after.inline_dimension_type
            && before.biome_zoom_seed() == after.biome_zoom_seed(),
        "generation world identity changed during extension"
    );
    for (&pos, old_maps) in &checkpoint.terrain.heightmaps {
        let old_count = old_edits.iter().filter(|edit| edit.chunk() == pos).count();
        let changes = processed.region.block_changes_for_chunk(pos)?;
        ensure!(
            old_count <= changes.len(),
            "postprocessing history was truncated"
        );
        let mut allowed = BTreeSet::new();
        for change in &changes[old_count..] {
            let [x, y, z] = change.position;
            let position = native_world::loaded::BlockPosition { x, y, z };
            if allowed.insert(position) {
                ensure!(
                    before.block(x, y, z) == Some(&change.expected),
                    "new postprocessing history does not start at the previous block"
                );
            }
        }
        validate_chunk_blocks(before, after, pos, &allowed)?;
        let maps = processed
            .region
            .heightmaps()
            .get(&pos)
            .context("postprocessed heightmaps are missing")?
            .spawn_heightmaps();
        ensure!(
            *old_maps == Some(maps) || !allowed.is_empty(),
            "previous heightmaps changed without block updates"
        );
    }
    Ok(())
}

fn validate_chunk_blocks(
    before: &native_world::loaded::World,
    after: &native_world::loaded::World,
    position: ChunkPos,
    allowed: &BTreeSet<native_world::loaded::BlockPosition>,
) -> Result<()> {
    let old = before
        .chunks
        .get(&position)
        .context("previous chunk is missing")?;
    let new = after
        .chunks
        .get(&position)
        .context("previous chunk disappeared")?;
    ensure!(
        old.position == new.position,
        "previous chunk position changed"
    );
    let origin_x = position.x.checked_mul(16).context("chunk X overflow")?;
    let origin_z = position.z.checked_mul(16).context("chunk Z overflow")?;
    for (&section_y, section) in &old.sections {
        let Some(current) = new.sections.get(&section_y) else {
            ensure!(
                section.blocks.is_none() && section.biomes.is_none(),
                "previous block or biome section disappeared"
            );
            continue;
        };
        match (&section.biomes, &current.biomes) {
            (None, None) => {}
            (Some(old), Some(new)) => {
                for (old, new) in old.iter().zip(new.iter()) {
                    let old = before
                        .biome_by_id(*old)
                        .context("previous biome is unknown")?;
                    let new = after.biome_by_id(*new).context("new biome is unknown")?;
                    ensure!(old == new, "previous biomes changed during extension");
                }
            }
            _ => anyhow::bail!("previous biome section availability changed"),
        }
        match (&section.blocks, &current.blocks) {
            (None, None) => {}
            (Some(old), Some(new)) => {
                for (index, (old, new)) in old.iter().zip(new.iter()).enumerate() {
                    let old = before.state(*old).context("previous block is unknown")?;
                    let new = after.state(*new).context("new block is unknown")?;
                    if old != new {
                        let local = i32::try_from(index)?;
                        let changed = native_world::loaded::BlockPosition {
                            x: origin_x + local % 16,
                            y: i32::from(section_y) * 16 + local / 256,
                            z: origin_z + (local / 16) % 16,
                        };
                        ensure!(
                            allowed.contains(&changed),
                            "unrecorded block change at {changed:?}"
                        );
                    }
                }
            }
            _ => anyhow::bail!("previous block section availability changed"),
        }
    }
    for (&section_y, section) in &new.sections {
        if !old.sections.contains_key(&section_y) {
            ensure!(
                section.blocks.is_none() && section.biomes.is_none(),
                "unrecorded block or biome section appeared"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_world::loaded::{BIOME_COUNT, BLOCK_COUNT, BlockPosition, Chunk, Section, World};

    #[test]
    fn previous_chunks_accept_recorded_blocks_and_light_only_sections() -> Result<()> {
        let position = ChunkPos { x: -2, z: 3 };
        let mut before = World::new("minecraft:overworld");
        let stone = before.intern_state(BlockState {
            name: "minecraft:stone".into(),
            properties: Default::default(),
        });
        let dirt = before.intern_state(BlockState {
            name: "minecraft:dirt".into(),
            properties: Default::default(),
        });
        let plains = before.intern_biome("minecraft:plains".into());
        before.chunks.insert(
            position,
            Arc::new(Chunk {
                position,
                status: Some("minecraft:full".into()),
                block_light_correct: true,
                sections: BTreeMap::from([(
                    0,
                    Arc::new(Section {
                        blocks: Some(Box::new([stone; BLOCK_COUNT])),
                        biomes: Some(Box::new([plains; BIOME_COUNT])),
                        block_light: None,
                        sky_light: None,
                    }),
                )]),
            }),
        );
        let changed = BlockPosition {
            x: -31,
            y: 2,
            z: 51,
        };
        let mut after = before.clone();
        after.set_block_id(changed.x, changed.y, changed.z, dirt)?;
        Arc::make_mut(after.chunks.get_mut(&position).unwrap())
            .sections
            .insert(
                1,
                Arc::new(Section {
                    blocks: None,
                    biomes: None,
                    block_light: Some(Box::new([0; native_world::loaded::LIGHT_BYTES])),
                    sky_light: None,
                }),
            );
        assert!(validate_chunk_blocks(&before, &after, position, &BTreeSet::new()).is_err());
        let allowed = BTreeSet::from([changed]);
        validate_chunk_blocks(&before, &after, position, &allowed)?;
        after.set_block_id(-30, 2, 51, dirt)?;
        assert!(validate_chunk_blocks(&before, &after, position, &allowed).is_err());
        Ok(())
    }

    #[test]
    fn previous_chunks_compare_registry_values_instead_of_numeric_ids() -> Result<()> {
        let position = ChunkPos { x: 1, z: -1 };
        let mut before = World::new("minecraft:overworld");
        let stone = before.intern_state(BlockState {
            name: "minecraft:stone".into(),
            properties: Default::default(),
        });
        let plains = before.intern_biome("minecraft:plains".into());
        before.chunks.insert(
            position,
            Arc::new(Chunk {
                position,
                status: Some("minecraft:full".into()),
                block_light_correct: true,
                sections: BTreeMap::from([(
                    0,
                    Arc::new(Section {
                        blocks: Some(Box::new([stone; BLOCK_COUNT])),
                        biomes: Some(Box::new([plains; BIOME_COUNT])),
                        block_light: None,
                        sky_light: None,
                    }),
                )]),
            }),
        );
        let mut after = World::new("minecraft:overworld");
        let dirt = after.intern_state(BlockState {
            name: "minecraft:dirt".into(),
            properties: Default::default(),
        });
        let remapped_stone = after.intern_state(BlockState {
            name: "minecraft:stone".into(),
            properties: Default::default(),
        });
        let desert = after.intern_biome("minecraft:desert".into());
        let remapped_plains = after.intern_biome("minecraft:plains".into());
        let mut remapped = (**before.chunks.get(&position).unwrap()).clone();
        remapped.sections.insert(
            0,
            Arc::new(Section {
                blocks: Some(Box::new([remapped_stone; BLOCK_COUNT])),
                biomes: Some(Box::new([remapped_plains; BIOME_COUNT])),
                block_light: None,
                sky_light: None,
            }),
        );
        after.chunks.insert(position, Arc::new(remapped));
        validate_chunk_blocks(&before, &after, position, &BTreeSet::new())?;
        let chunk = Arc::make_mut(after.chunks.get_mut(&position).unwrap());
        let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
        section.blocks.as_mut().unwrap()[0] = dirt;
        assert!(validate_chunk_blocks(&before, &after, position, &BTreeSet::new()).is_err());
        let chunk = Arc::make_mut(after.chunks.get_mut(&position).unwrap());
        let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
        section.blocks.as_mut().unwrap()[0] = remapped_stone;
        section.biomes.as_mut().unwrap()[0] = desert;
        assert!(validate_chunk_blocks(&before, &after, position, &BTreeSet::new()).is_err());
        Ok(())
    }
}
