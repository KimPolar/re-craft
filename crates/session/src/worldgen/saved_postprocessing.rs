//! Isolated postprocessing inputs restored from saved FULL chunks.
//!
//! The caller must establish Java holder readiness and later reconcile live
//! state, light, entities, POIs, clocks and disk sources before publishing.

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use native_persistence::{
    heightmap::{FullHeightmaps, HeightBounds},
    nbt::{NamedTag, Tag},
    world::{ChunkArea, DimensionId, TARGET_DATA_VERSION},
};
use native_resources::packs::{ResourceId, ResourceProvider};
use native_world::{
    generation_light::VisibleLightSnapshot,
    loaded::{BlockState, ChunkPos, SavedBlockEntities},
};

use crate::{
    loaded::{BuildBounds, GenerationTerrainStage, SavedTickQueueCapture},
    saved_chunk_activation::{SavedTickInputs, read_saved_work},
    ticking_preparation::{TickingFutureState, TickingPreparation, ticking_dependencies},
};

use super::{
    GeneratedPostprocessing,
    captured_postprocessing::CapturedPostprocessingTags,
    chunk_ticks::{
        CapturedTickAttempt, ChunkTicks, ConvertedChunkTicks, IndexedRegionQueue, RegionTicks,
        SavedTick, TickRegistry,
    },
    postprocessing::{ConvertedPostprocessingRegion, PostprocessingTask},
    postprocessing_world::{ConvertedBlockEntities, PostprocessedGeneratedRegion},
};

/// Inputs captured from the same world and active resource/data-pack stack.
/// Registration is the actual LevelTicks registration order, not proof that
/// any holder has completed its ticking future.
pub struct SavedPostprocessingInputs<'a> {
    pub area: &'a ChunkArea,
    pub dimension: &'a DimensionId,
    pub defaults: &'a BTreeMap<String, BTreeMap<String, String>>,
    pub block_ids: &'a BTreeSet<String>,
    pub bounds: BuildBounds,
    pub registration: &'a [ChunkPos],
    pub game_time: i64,
    pub next_sub_tick: i64,
    pub light: Option<VisibleLightSnapshot>,
}

/// One completed isolated callback and the LevelTicks indexes after unpacking
/// only the center. Other converted chunks retain their pending saved ticks.
pub struct SavedTickingResult {
    pub region: PostprocessedGeneratedRegion,
    pub block_ticks: Arc<IndexedRegionQueue>,
    pub fluid_ticks: Arc<IndexedRegionQueue>,
    live: Option<LiveTickingProvenance>,
}

struct LiveTickingProvenance {
    terrain: Arc<()>,
    ticks: SavedTickQueueCapture,
    callbacks: Vec<(ChunkPos, Arc<()>)>,
    packs_bound: bool,
    registry: Option<Arc<native_resources::block_registry::GeneratedBlockRegistry>>,
    heightmaps_match: bool,
    block_entities: BTreeMap<ChunkPos, (Option<SavedBlockEntities>, Option<SavedBlockEntities>)>,
    block_effects: Option<std::result::Result<ValidatedBlockEffects, String>>,
    region: Option<Arc<()>>,
    sealed_queues: Option<(Arc<IndexedRegionQueue>, Arc<IndexedRegionQueue>)>,
    replayed_queues: Option<(Arc<IndexedRegionQueue>, Arc<IndexedRegionQueue>)>,
}

struct ValidatedBlockEffects {
    changes: Vec<native_persistence::poi::PoiBlockChange>,
    chunks: BTreeSet<ChunkPos>,
}

/// Run on the preparation worker: decoding unrelated block-entity data and
/// validating every transition must not stall the main executor.
fn validate_block_effects(
    region: &PostprocessedGeneratedRegion,
    live: &LiveTickingProvenance,
    center: ChunkPos,
) -> Result<ValidatedBlockEffects> {
    let changes = region.ordered_block_changes()?;
    if changes.is_empty() {
        ensure!(
            region.light_checks.is_empty(),
            "postprocessing light checks have no block changes"
        );
        return Ok(ValidatedBlockEffects {
            changes,
            chunks: BTreeSet::new(),
        });
    }
    let registry = live
        .registry
        .as_ref()
        .context("postprocessing block registry is unavailable")?;
    ensure!(
        live.heightmaps_match,
        "postprocessing heightmaps differ from the live session"
    );
    let dependencies: BTreeSet<_> = ticking_dependencies(center).into_iter().collect();
    let mut positions = BTreeMap::<ChunkPos, BTreeSet<[i32; 3]>>::new();
    let mut checks = Vec::new();
    for change in &changes {
        let position = native_world::loaded::BlockPosition {
            x: change.position[0],
            y: change.position[1],
            z: change.position[2],
        };
        ensure!(
            dependencies.contains(&position.chunk()),
            "postprocessing changed an uncaptured neighbor"
        );
        positions
            .entry(position.chunk())
            .or_default()
            .insert(change.position);
        for state in [&change.expected, &change.replacement] {
            ensure!(
                registry.is_canonical_state(&state.name, &state.properties)
                    && registry.has_block_entity(&state.name) == Some(false),
                "postprocessing block state or entity behavior is unsupported"
            );
        }
        ensure!(
            change.replacement.name != "minecraft:frosted_ice",
            "postprocessing frosted-ice placement needs the live random stream"
        );
        ensure!(
            native_world::poi::change(&change.expected, &change.replacement).is_none(),
            "postprocessing POI changes need live reconciliation"
        );
        if !native_world::block_light::same_light_behavior(&change.expected, &change.replacement)
            .context("postprocessing light behavior is unavailable")?
        {
            checks.push(position);
        }
    }
    ensure!(
        checks == region.light_checks,
        "postprocessing light history differs"
    );
    for (&chunk, changed) in &positions {
        let (saved, overlay) = live
            .block_entities
            .get(&chunk)
            .context("postprocessing block entities were not captured")?;
        let saved = saved
            .as_ref()
            .context("postprocessing block entities are unavailable")?;
        ensure!(
            region.source.saved_block_entities.get(&chunk) == Some(saved),
            "postprocessing block entities differ from the saved source"
        );
        ensure_no_block_entity_at(saved, chunk, changed)?;
        if let Some(overlay) = overlay {
            ensure_no_block_entity_at(overlay, chunk, changed)?;
        }
    }
    Ok(ValidatedBlockEffects {
        changes,
        chunks: positions.into_keys().collect(),
    })
}

fn ensure_no_block_entity_at(
    entities: &SavedBlockEntities,
    chunk: ChunkPos,
    positions: &BTreeSet<[i32; 3]>,
) -> Result<()> {
    let bytes = match entities {
        SavedBlockEntities::Empty => return Ok(()),
        SavedBlockEntities::Encoded(bytes) => bytes,
        SavedBlockEntities::Unavailable => bail!("postprocessing block entities are unavailable"),
    };
    let document = native_persistence::nbt::read(bytes)?;
    let Tag::List {
        element_type,
        values,
    } = document.value
    else {
        bail!("postprocessing block entities are not a list");
    };
    ensure!(
        element_type == native_persistence::nbt::TagType::Compound
            || (element_type == native_persistence::nbt::TagType::End && values.is_empty()),
        "postprocessing block entity list has an unsupported type"
    );
    let mut seen = BTreeSet::new();
    for entity in values {
        let Tag::Compound(fields) = entity else {
            bail!("postprocessing block entity is not a compound");
        };
        let coordinate = |name: &str| -> Result<i32> {
            let mut matches = fields.iter().filter(|(key, _)| key.matches(name));
            let value = match matches.next() {
                Some((_, Tag::Int(value))) => *value,
                _ => bail!("postprocessing block entity position is invalid"),
            };
            ensure!(
                matches.next().is_none(),
                "duplicate block entity coordinate"
            );
            Ok(value)
        };
        let position = [coordinate("x")?, coordinate("y")?, coordinate("z")?];
        ensure!(
            position[0].div_euclid(16) == chunk.x
                && position[2].div_euclid(16) == chunk.z
                && seen.insert(position),
            "postprocessing block entity belongs to another chunk or is duplicated"
        );
        ensure!(
            !positions.contains(&position),
            "postprocessing changed a block with a stored entity"
        );
    }
    Ok(())
}

impl SavedTickingResult {
    pub(crate) fn verify_no_effect_live_callback(
        &self,
        terrain: &GenerationTerrainStage,
        future: &TickingPreparation,
        water_source_conversion: bool,
        lava_rules: super::postprocessing_world::GeneratedLavaRules,
    ) -> Result<&SavedTickQueueCapture> {
        let live = self.verify_live_callback_terrain(
            terrain,
            future,
            water_source_conversion,
            lava_rules,
        )?;
        ensure!(
            self.region.edits.is_empty() && self.region.light_checks.is_empty(),
            "saved postprocessing has block changes that need live reconciliation"
        );
        ensure!(
            live.sealed_queues.as_ref().is_some_and(|(blocks, fluids)| {
                Arc::ptr_eq(blocks, &self.block_ticks) && Arc::ptr_eq(fluids, &self.fluid_ticks)
            }),
            "postprocessing tick queues need live reconciliation"
        );
        ensure!(
            self.region.ticks().next_sub_tick() == live.ticks.next_sub_tick(),
            "saved postprocessing has effects that need live reconciliation"
        );
        Ok(&live.ticks)
    }

    /// Verify immutable block changes and replayed schedule attempts. The live
    /// level must still check its clock, queues, terrain, heightmaps and holder.
    pub(crate) fn verify_scheduled_live_callback(
        &self,
        terrain: &GenerationTerrainStage,
        future: &TickingPreparation,
        water_source_conversion: bool,
        lava_rules: super::postprocessing_world::GeneratedLavaRules,
    ) -> Result<(
        &SavedTickQueueCapture,
        &[CapturedTickAttempt],
        &[native_persistence::poi::PoiBlockChange],
        &BTreeSet<ChunkPos>,
    )> {
        let live = self.verify_live_callback_terrain(
            terrain,
            future,
            water_source_conversion,
            lava_rules,
        )?;
        ensure!(
            live.replayed_queues
                .as_ref()
                .is_some_and(|(blocks, fluids)| {
                    Arc::ptr_eq(blocks, &self.block_ticks) && Arc::ptr_eq(fluids, &self.fluid_ticks)
                }),
            "postprocessing schedules could not be replayed against the captured queues"
        );
        let attempts = self
            .region
            .ticks()
            .live_attempts()
            .context("saved postprocessing schedule history is missing")?;
        let effects = live
            .block_effects
            .as_ref()
            .context("postprocessing block history was not validated")?
            .as_ref()
            .map_err(|error| {
                anyhow::anyhow!("postprocessing block history could not be reconciled: {error}")
            })?;
        Ok((&live.ticks, attempts, &effects.changes, &effects.chunks))
    }

    fn verify_live_callback_terrain(
        &self,
        terrain: &GenerationTerrainStage,
        future: &TickingPreparation,
        water_source_conversion: bool,
        lava_rules: super::postprocessing_world::GeneratedLavaRules,
    ) -> Result<&LiveTickingProvenance> {
        let live = self
            .live
            .as_ref()
            .context("saved ticking result is not bound to a live session")?;
        ensure!(
            live.packs_bound,
            "saved ticking result has no bound data-pack snapshot"
        );
        ensure!(
            live.region
                .as_ref()
                .is_some_and(|identity| Arc::ptr_eq(identity, &self.region.identity)),
            "saved postprocessing region was replaced"
        );
        ensure!(
            Arc::ptr_eq(&live.terrain, terrain.identity())
                && live.callbacks.len() == 1
                && live.callbacks[0].0 == future.center()
                && Arc::ptr_eq(&live.callbacks[0].1, future.callback_identity())
                && future.accepts_prepared_callback(),
            "saved ticking callback or terrain capture changed"
        );
        let region = &self.region;
        let dependencies: BTreeSet<_> =
            crate::ticking_preparation::ticking_dependencies(future.center())
                .into_iter()
                .collect();
        ensure!(
            Arc::ptr_eq(&region.world, &region.sealed_world)
                && region.edits == region.sealed_edits
                && region.edits.len() == region.edit_states.len()
                && region.read_chunks == region.sealed_read_chunks
                && region.light_reads == region.sealed_light_reads
                && region.light_checks == region.sealed_checks
                && region.chunks == region.sealed_chunks,
            "saved postprocessing result was modified"
        );
        ensure!(
            region.chunks == BTreeSet::from([future.center()])
                && region.read_chunks.is_subset(&dependencies)
                && region.light_reads.is_empty()
                && region.light_checkpoint.is_none()
                && region.lava_fizz_events().is_empty()
                && region.block_drops().is_none()
                && !region.has_block_entity_edits(),
            "saved postprocessing has effects that need live reconciliation"
        );
        ensure!(
            region.water_source_conversion() == Some(water_source_conversion)
                && region.lava_rules() == Some(lava_rules),
            "saved postprocessing game rules changed"
        );
        ensure!(
            matches!(region.block_entity_data(future.center())?, Tag::List { values, .. } if values.is_empty()),
            "saved postprocessing has block entities to reconcile"
        );
        Ok(live)
    }

    /// Create marks-only drafts for completed callbacks, using the same saved
    /// inputs as the isolated pass. The drafts cannot be saved: block edits,
    /// ticks, lighting, entities, POIs and live state still need reconciliation.
    /// Run away from the UI thread because every captured source is checked.
    pub fn draft_completed_postprocessing_marks(
        &self,
        area: &ChunkArea,
        positions: impl IntoIterator<Item = ChunkPos>,
    ) -> Result<BTreeMap<ChunkPos, native_persistence::edit::ChunkDraft>> {
        let region = &self.region;
        ensure!(
            Arc::ptr_eq(&region.world, &region.sealed_world)
                && region.edits == region.sealed_edits
                && region.edits.len() == region.edit_states.len()
                && region.read_chunks == region.sealed_read_chunks
                && region.light_reads == region.sealed_light_reads
                && region.light_checks == region.sealed_checks
                && region.chunks == region.sealed_chunks,
            "saved postprocessing result was modified"
        );
        ensure!(
            !region.chunks.is_empty(),
            "no completed saved postprocessing"
        );
        ensure!(
            area.chunks.len() == region.source.chunks.len(),
            "saved postprocessing source chunk set changed"
        );
        for &pos in region.source.chunks.keys() {
            let saved_pos = native_persistence::world::ChunkPos { x: pos.x, z: pos.z };
            let source = area
                .chunks
                .get(&saved_pos)
                .context("saved postprocessing source chunk is missing")?;
            ensure!(
                source.position == saved_pos
                    && region.source.saved_chunk_source(pos)
                        == Some(native_persistence::nbt::fingerprint(&source.document)?),
                "saved postprocessing source changed at {pos:?}"
            );
        }
        let height = region
            .max_y
            .checked_sub(region.min_y)
            .context("invalid postprocessing bounds")?;
        let mut drafts = BTreeMap::new();
        for pos in positions {
            ensure!(region.chunks.contains(&pos), "chunk was not postprocessed");
            ensure!(!drafts.contains_key(&pos), "duplicate postprocessed chunk");
            let saved_pos = native_persistence::world::ChunkPos { x: pos.x, z: pos.z };
            let source = area
                .chunks
                .get(&saved_pos)
                .context("postprocessed source chunk is missing")?;
            let expected = unique_field(&source.document.value, "PostProcessing")?;
            let mut draft =
                native_persistence::edit::draft_chunk_blocks(&source.document, saved_pos, &[])?;
            draft.stage_completed_postprocessing(region.min_y, height, expected)?;
            drafts.insert(pos, draft);
        }
        ensure!(!drafts.is_empty(), "no postprocessed chunks selected");
        Ok(drafts)
    }
}

/// An isolated sequence of holder callbacks at one unchanged level time.
/// Send-sync is treated as pending, so no chunk notification is delivered.
/// Run away from the UI thread; the caller must prevent live interleaving and
/// revalidate every dependency before attaching the result to a session.
pub struct SavedTickingBatch {
    stage: GeneratedPostprocessing,
    available: BTreeSet<ChunkPos>,
    game_time: i64,
    started: bool,
    failed: bool,
    live: Option<LiveTickingProvenance>,
}

impl SavedTickingBatch {
    /// Capture all FULL inputs and configure the same environment for every
    /// callback. No holder future has been completed by this operation.
    pub fn begin(
        provider: &mut impl ResourceProvider,
        input: SavedPostprocessingInputs<'_>,
        validate_state: impl FnMut(&BlockState) -> Result<()>,
        configure: impl FnOnce(&mut GeneratedPostprocessing) -> Result<()>,
    ) -> Result<Self> {
        let available = input
            .area
            .chunks
            .keys()
            .map(|pos| ChunkPos { x: pos.x, z: pos.z })
            .collect();
        let game_time = input.game_time;
        let mut stage = begin_saved_postprocessing(provider, input, validate_state)?;
        configure(&mut stage)?;
        Ok(Self {
            stage,
            available,
            game_time,
            started: false,
            failed: false,
            live: None,
        })
    }

    /// Run an isolated pass over captured live blocks instead of stale saved
    /// terrain. The saved tick containers are still reconstructed from disk;
    /// callers must separately capture live ticks and other session effects.
    pub fn begin_with_live_terrain(
        provider: &mut impl ResourceProvider,
        input: SavedPostprocessingInputs<'_>,
        terrain: &GenerationTerrainStage,
        validate_state: impl FnMut(&BlockState) -> Result<()>,
        configure: impl FnOnce(&mut GeneratedPostprocessing) -> Result<()>,
    ) -> Result<Self> {
        let available = input
            .area
            .chunks
            .keys()
            .map(|pos| ChunkPos { x: pos.x, z: pos.z })
            .collect();
        let game_time = input.game_time;
        let mut stage =
            begin_saved_postprocessing_with_live_terrain(provider, input, terrain, validate_state)?;
        configure(&mut stage)?;
        Ok(Self {
            stage,
            available,
            game_time,
            started: false,
            failed: false,
            live: None,
        })
    }

    /// Run callbacks over captured live blocks and the actual registered tick
    /// queues. The caller must pause simulation or reject stale terrain and
    /// queue captures before using the result; holder, light and entity state
    /// are not established by this operation.
    pub fn begin_with_live_session(
        provider: &mut impl ResourceProvider,
        input: SavedPostprocessingInputs<'_>,
        terrain: &GenerationTerrainStage,
        ticks: &SavedTickQueueCapture,
        validate_state: impl FnMut(&BlockState) -> Result<()>,
        configure: impl FnOnce(&mut GeneratedPostprocessing) -> Result<()>,
    ) -> Result<Self> {
        ensure!(
            ticks.belongs_to_terrain(terrain),
            "tick and terrain captures belong to different sessions"
        );
        ensure!(
            input.game_time == ticks.game_time()
                && input.next_sub_tick == ticks.next_sub_tick()
                && input.registration == ticks.registration_order(),
            "saved postprocessing tick capture differs from its inputs"
        );
        let available = input
            .area
            .chunks
            .keys()
            .map(|pos| ChunkPos { x: pos.x, z: pos.z })
            .collect();
        let game_time = input.game_time;
        let queues = ticks.isolated_region_ticks()?;
        let mut stage = begin_saved_postprocessing_inner(
            provider,
            input,
            Some(terrain),
            Some(queues),
            validate_state,
        )?;
        let heightmaps_match = terrain.matches_postprocessing_heightmaps(
            stage.initial_heightmaps(),
            stage.initial_heightmap_tags(),
        );
        configure(&mut stage)?;
        Ok(Self {
            stage,
            available,
            game_time,
            started: false,
            failed: false,
            live: Some(LiveTickingProvenance {
                terrain: Arc::clone(terrain.identity()),
                ticks: ticks.clone(),
                callbacks: Vec::new(),
                packs_bound: false,
                registry: None,
                heightmaps_match,
                block_entities: terrain.postprocessing_block_entities(),
                block_effects: None,
                region: None,
                sealed_queues: None,
                replayed_queues: None,
            }),
        })
    }

    /// Prepare a live callback from the same data-pack snapshot and selection
    /// used by the session. All other live dependencies still need checks.
    pub fn begin_with_bound_live_session(
        provider: &mut native_resources::packs::ResourceStack,
        input: SavedPostprocessingInputs<'_>,
        terrain: &GenerationTerrainStage,
        ticks: &SavedTickQueueCapture,
        mut validate_state: impl FnMut(&BlockState) -> Result<()>,
        water_source_conversion: bool,
        lava_rules: super::postprocessing_world::GeneratedLavaRules,
    ) -> Result<Self> {
        let packs = ticks
            .data_packs()
            .context("session data packs are not captured")?;
        let selected = packs.selection();
        ensure!(
            provider
                .data_pack_snapshot()
                .as_ref()
                .is_some_and(|snapshot| Arc::ptr_eq(snapshot, packs.packs()))
                && provider.data_pack_snapshot_selection()
                    == Some(&native_resources::datapacks::DataPackSettings {
                        enabled: selected.enabled.clone(),
                        disabled: selected.disabled.clone(),
                        enabled_features: selected.enabled_features.clone(),
                    }),
            "postprocessing resources differ from the session data packs"
        );
        packs.packs().verify()?;
        let block_ids = provider.vanilla_block_ids()?;
        let registry = Arc::new(provider.vanilla_generated_block_registry()?);
        ensure!(
            input.block_ids == &block_ids && input.defaults == &registry.default_states(),
            "postprocessing block registry differs from the session resources"
        );
        let mut batch = Self::begin_with_live_session(
            provider,
            input,
            terrain,
            ticks,
            |state| {
                ensure!(
                    registry.is_canonical_state(&state.name, &state.properties),
                    "unknown saved postprocessing block state"
                );
                validate_state(state)
            },
            |stage| {
                stage.set_water_source_conversion(water_source_conversion)?;
                stage.set_lava_rules(lava_rules)?;
                Ok(())
            },
        )?;
        let live = batch.live.as_mut().expect("live provenance");
        live.packs_bound = true;
        live.registry = Some(registry);
        Ok(batch)
    }

    /// Dispatch one queued holder callback without interleaving another one.
    /// Pending saved ticks are unpacked after its postprocessing and become
    /// visible to later callbacks in this same captured level state.
    pub fn dispatch(&mut self, future: &mut TickingPreparation, game_time: i64) -> Result<bool> {
        self.dispatch_checked(future, game_time, || Ok(()))
    }

    /// Allow a background caller to cancel between bounded postprocessing
    /// steps. A failed private batch must be discarded; its original holder
    /// callback remains the caller's responsibility.
    pub fn dispatch_checked(
        &mut self,
        future: &mut TickingPreparation,
        game_time: i64,
        check: impl Fn() -> Result<()>,
    ) -> Result<bool> {
        ensure!(!self.failed, "saved ticking batch has failed");
        if game_time != self.game_time {
            self.failed = true;
            bail!("level time changed during saved ticking preparation");
        }
        if let Err(error) = check() {
            self.failed = true;
            return Err(error);
        }
        let available = &self.available;
        let stage = RefCell::new(&mut self.stage);
        let started = std::cell::Cell::new(self.started);
        let result = future.dispatch(
            |center| {
                check()?;
                ensure!(
                    ticking_dependencies(center)
                        .iter()
                        .all(|position| available.contains(position)),
                    "saved ticking inputs lack a FULL neighbor"
                );
                let mut stage = stage.borrow_mut();
                if stage.postprocessing_completed(center) {
                    return Ok(());
                }
                if started.get() {
                    stage.request_converted_chunk(center)?;
                } else {
                    stage.select_initial_targets(BTreeSet::from([center]))?;
                    started.set(true);
                }
                while !stage.poll(center, 256)? {
                    check()?;
                }
                check()?;
                Ok(())
            },
            |center| {
                check()?;
                stage.borrow_mut().unpack_saved_ticks(center)
            },
            |_| check(),
        );
        self.started = started.get();
        if result.is_err() {
            self.failed = true;
        } else if matches!(result, Ok(true)) && future.state() == TickingFutureState::Available {
            if let Some(live) = &mut self.live {
                live.callbacks
                    .push((future.center(), Arc::clone(future.callback_identity())));
            }
        }
        result
    }

    /// Finish the captured sequence. This alone does not authorize live use.
    pub fn finish(self) -> Result<SavedTickingResult> {
        ensure!(
            !self.failed && self.started,
            "no completed saved ticking callbacks"
        );
        let region = self.stage.finish()?;
        let block_ticks = Arc::new(
            region
                .ticks()
                .captured_indexed_queue_for(TickRegistry::Block, &[])?,
        );
        let fluid_ticks = Arc::new(
            region
                .ticks()
                .captured_indexed_queue_for(TickRegistry::Fluid, &[])?,
        );
        let mut live = self.live;
        if let Some(proof) = &mut live {
            proof.region = Some(Arc::clone(&region.identity));
            if proof.packs_bound {
                proof
                    .ticks
                    .data_packs()
                    .context("session data packs are not captured")?
                    .packs()
                    .verify()?;
            }
            if proof.packs_bound && proof.callbacks.len() == 1 {
                proof.block_effects = Some(
                    validate_block_effects(&region, proof, proof.callbacks[0].0)
                        .map_err(|error| format!("{error:#}")),
                );
                if let Ok((blocks, fluids)) =
                    proof.ticks.expected_after_unpack(proof.callbacks[0].0)
                {
                    if blocks == *block_ticks && fluids == *fluid_ticks {
                        proof.sealed_queues =
                            Some((Arc::clone(&block_ticks), Arc::clone(&fluid_ticks)));
                    }
                }
                if let Some(attempts) = region.ticks().live_attempts() {
                    let replayed = if attempts.is_empty()
                        && proof.sealed_queues.is_some()
                        && region.ticks().next_sub_tick() == proof.ticks.next_sub_tick()
                    {
                        true
                    } else {
                        proof
                            .ticks
                            .replay_postprocessing_schedules(
                                proof.callbacks[0].0,
                                attempts,
                                region.ticks().next_sub_tick(),
                                &block_ticks,
                                &fluid_ticks,
                            )
                            .is_ok()
                    };
                    if replayed {
                        proof.replayed_queues =
                            Some((Arc::clone(&block_ticks), Arc::clone(&fluid_ticks)));
                    }
                }
            }
        }
        Ok(SavedTickingResult {
            region,
            block_ticks,
            fluid_ticks,
            live,
        })
    }
}

/// Execute a queued ticking-future callback over the captured FULL source.
/// The caller must sample the game time, registry, light and holder identities
/// at dispatch, prevent interleaving, and later reconcile all live and disk
/// state. Run this away from the UI thread. The send-sync future is treated as
/// pending in this isolated pass; this result cannot authorize live attachment,
/// a send notification, or a save.
pub fn dispatch_saved_ticking(
    future: &mut TickingPreparation,
    provider: &mut impl ResourceProvider,
    input: SavedPostprocessingInputs<'_>,
    mut validate_state: impl FnMut(&BlockState) -> Result<()>,
    mut configure: impl FnMut(&mut GeneratedPostprocessing) -> Result<()>,
) -> Result<Option<SavedTickingResult>> {
    let mut input = Some(input);
    let stage = RefCell::new(None::<GeneratedPostprocessing>);
    let result = RefCell::new(None::<SavedTickingResult>);
    let dispatched = future.dispatch(
        |center| {
            let input = input
                .take()
                .context("saved ticking input was already consumed")?;
            ensure!(
                ticking_dependencies(center).iter().all(|position| {
                    input
                        .area
                        .chunks
                        .contains_key(&native_persistence::world::ChunkPos {
                            x: position.x,
                            z: position.z,
                        })
                }),
                "saved ticking inputs lack a FULL neighbor"
            );
            let mut next = begin_saved_postprocessing(provider, input, &mut validate_state)?;
            next.select_initial_targets(BTreeSet::from([center]))?;
            configure(&mut next)?;
            while !next.poll(center, 256)? {}
            *stage.borrow_mut() = Some(next);
            Ok(())
        },
        |center| {
            stage
                .borrow_mut()
                .as_mut()
                .context("saved ticking postprocessing is missing")?
                .unpack_saved_ticks(center)
        },
        |_| {
            let region = stage
                .borrow_mut()
                .take()
                .context("saved ticking postprocessing is missing")?
                .finish()?;
            let blocks = region
                .ticks()
                .captured_indexed_queue_for(TickRegistry::Block, &[])?;
            let fluids = region
                .ticks()
                .captured_indexed_queue_for(TickRegistry::Fluid, &[])?;
            *result.borrow_mut() = Some(SavedTickingResult {
                region,
                block_ticks: Arc::new(blocks),
                fluid_ticks: Arc::new(fluids),
                live: None,
            });
            Ok(())
        },
    )?;
    if !dispatched || future.state() != TickingFutureState::Available {
        return Ok(None);
    }
    Ok(Some(
        result
            .into_inner()
            .context("saved ticking result is missing")?,
    ))
}

/// Restore an isolated postprocessing stage from original FULL chunk data.
/// Saved tick entries remain pending during callbacks. Reproducible default
/// chests, chests and brushable blocks with deferred loot, and default potent
/// sulfur entities are admitted. Active upgrade data and other contents are
/// rejected. This does not authorize a live commit or save.
pub fn begin_saved_postprocessing(
    provider: &mut impl ResourceProvider,
    input: SavedPostprocessingInputs<'_>,
    validate_state: impl FnMut(&BlockState) -> Result<()>,
) -> Result<GeneratedPostprocessing> {
    begin_saved_postprocessing_inner(provider, input, None, None, validate_state)
}

/// Prepare an isolated pass using source-bound live block edits. This does not
/// bind live tick queues, block entities, POIs, lighting or the level clock.
/// The captured terrain must be rechecked before any result is used.
pub fn begin_saved_postprocessing_with_live_terrain(
    provider: &mut impl ResourceProvider,
    input: SavedPostprocessingInputs<'_>,
    terrain: &GenerationTerrainStage,
    validate_state: impl FnMut(&BlockState) -> Result<()>,
) -> Result<GeneratedPostprocessing> {
    begin_saved_postprocessing_inner(provider, input, Some(terrain), None, validate_state)
}

fn begin_saved_postprocessing_inner(
    provider: &mut impl ResourceProvider,
    input: SavedPostprocessingInputs<'_>,
    live_terrain: Option<&GenerationTerrainStage>,
    live_ticks: Option<RegionTicks>,
    mut validate_state: impl FnMut(&BlockState) -> Result<()>,
) -> Result<GeneratedPostprocessing> {
    ensure!(
        !input.area.chunks.is_empty(),
        "no saved postprocessing chunks"
    );
    let height = input
        .bounds
        .max_y
        .checked_sub(input.bounds.min_y)
        .context("invalid saved dimension height")?;
    ensure!(
        native_core::postprocessing::PostProcessing::new(input.bounds.min_y, height).is_some(),
        "unsupported saved postprocessing bounds"
    );
    for (position, chunk) in &input.area.chunks {
        ensure!(
            chunk.position == *position
                && chunk.data_version == TARGET_DATA_VERSION
                && chunk.is_full(),
            "saved chunk identity, version or status differs at {position:?}"
        );
        validate_document(&chunk.document, *position)?;
        ensure!(
            native_persistence::palette::decode_sections(&chunk.document.value)? == chunk.sections,
            "saved chunk sections differ from the original document at {position:?}"
        );
        for section in &chunk.sections {
            if let Some(blocks) = &section.blocks {
                for state in &blocks.states {
                    let mut seen = BTreeSet::new();
                    for (name, _) in &state.properties {
                        ensure!(seen.insert(&name.0), "duplicate saved block property");
                    }
                }
            }
        }
    }
    let mut world = input
        .area
        .materialize_with_defaults(input.dimension, input.defaults)?;
    let mut checked_states = BTreeSet::new();
    for chunk in world.chunks.values() {
        for section in chunk.sections.values() {
            if let Some(blocks) = &section.blocks {
                for &id in blocks.iter() {
                    if checked_states.insert(id) {
                        validate_state(world.state(id).context("saved block state is missing")?)?;
                    }
                }
            }
        }
    }
    if let Some(terrain) = live_terrain {
        world = terrain.saved_postprocessing_world(&world, input.bounds, &mut validate_state)?;
    }
    let world = Arc::new(world);
    let tags = CapturedPostprocessingTags::load(provider, input.block_ids)?;

    let mut tasks = BTreeMap::new();
    let mut containers = BTreeMap::new();
    let mut heightmaps = BTreeMap::new();
    let mut block_entities = BTreeMap::new();
    for (position, source) in &input.area.chunks {
        let pos = ChunkPos {
            x: position.x,
            z: position.z,
        };
        let activation = world
            .saved_chunk_activation(pos)
            .context("saved preparation was not captured")?;
        let entities = world
            .saved_block_entities
            .get(&pos)
            .context("saved block entities were not captured")?;
        block_entities.insert(
            pos,
            ConvertedBlockEntities::from_saved(entities, &world, pos, input.bounds)?,
        );
        let blocks = world
            .saved_block_ticks
            .get(&pos)
            .map_or(&[][..], Vec::as_slice);
        let fluids = world
            .saved_fluid_ticks
            .get(&pos)
            .map_or(&[][..], Vec::as_slice);
        let mut marks = read_saved_work(
            activation,
            // The live-session guard must continue rejecting pending entities.
            // Their complete saved payload was validated for this isolated stage above.
            &SavedBlockEntities::Empty,
            input.bounds,
            SavedTickInputs {
                position: pos,
                blocks,
                fluids,
                supports_block: |_| true,
            },
            true,
        )?;
        if let (Some(terrain), Some(ticks)) = (live_terrain, live_ticks.as_ref()) {
            let source = world
                .saved_chunk_source(pos)
                .context("saved postprocessing source is unavailable")?;
            if terrain.saved_postprocessing_completed(pos, source) {
                let current = ticks
                    .chunk(pos)
                    .context("completed postprocessing tick containers are unavailable")?;
                ensure!(
                    current.blocks.pending_saved().is_none()
                        && current.fluids.pending_saved().is_none(),
                    "completed postprocessing still has pending saved ticks"
                );
                marks =
                    native_core::postprocessing::PostProcessing::new(input.bounds.min_y, height)
                        .context("unsupported saved postprocessing bounds")?;
            }
        }
        for tick in blocks {
            let id = ResourceId::parse(&tick.block)?;
            ensure!(
                input
                    .block_ids
                    .contains(&format!("{}:{}", id.namespace, id.path)),
                "unknown saved block tick {}",
                tick.block
            );
        }
        let block_ticks = blocks
            .iter()
            .map(|tick| SavedTick {
                kind: tick.block.clone(),
                position: tick.position,
                delay: tick.delay,
                priority: tick.priority,
            })
            .collect();
        let fluid_ticks = fluids
            .iter()
            .map(|tick| SavedTick {
                kind: tick.fluid.clone(),
                position: tick.position,
                delay: tick.delay,
                priority: tick.priority,
            })
            .collect();
        let original = ConvertedChunkTicks {
            blocks: ChunkTicks::from_saved(pos, block_ticks)?,
            fluids: ChunkTicks::from_saved(pos, fluid_ticks)?,
        };
        if let Some(live) = live_ticks.as_ref().and_then(|ticks| ticks.chunk(pos)) {
            for (current, saved) in [
                (&live.blocks, &original.blocks),
                (&live.fluids, &original.fluids),
            ] {
                if let Some(pending) = current.pending_saved() {
                    ensure!(
                        Some(pending) == saved.pending_saved(),
                        "live pending ticks differ from saved chunk {pos:?}"
                    );
                } else {
                    ensure!(
                        marks.sections().iter().all(Vec::is_empty),
                        "saved postprocessing was not run before ticks activated at {pos:?}"
                    );
                }
            }
        }
        tasks.insert(pos, PostprocessingTask::new(Arc::new(marks), pos));
        containers.insert(pos, original);
        let draft = if let Some(terrain) = live_terrain {
            terrain.saved_edited_chunk_draft(pos, source, |state| {
                heightmap_block_flags(state, input.defaults, &tags.heightmaps, &mut validate_state)
            })?
        } else {
            None
        };
        let document = draft
            .as_ref()
            .map_or(source.document.as_ref(), |draft| draft.document());
        heightmaps.insert(pos, saved_heightmaps(document, input.bounds)?);
    }
    let ticks = if let Some(ticks) = live_ticks {
        ensure!(
            ticks.game_time() == input.game_time
                && ticks.next_sub_tick() == input.next_sub_tick
                && ticks.registration_order() == input.registration,
            "live tick region differs from the captured session"
        );
        ticks
    } else {
        RegionTicks::new_ordered(
            containers,
            input.game_time,
            input.next_sub_tick,
            input.registration,
        )?
    };
    let mut stage = GeneratedPostprocessing::new(
        world,
        input.bounds.min_y,
        input.bounds.max_y,
        ConvertedPostprocessingRegion { tasks, ticks },
        input.light,
        tags.mushroom.clone(),
        heightmaps,
        Some(Arc::clone(&tags.heightmaps)),
    );
    stage.set_block_entities(block_entities)?;
    tags.apply(&mut stage);
    Ok(stage)
}

/// Classify rebuilt maps using the same captured pack stack as the callbacks.
/// The caller's registry validator must reject unknown or noncanonical states.
fn heightmap_block_flags(
    saved: &native_persistence::palette::BlockState,
    defaults: &BTreeMap<String, BTreeMap<String, String>>,
    tags: &native_resources::heightmap_tags::HeightmapTags,
    validate_state: &mut impl FnMut(&BlockState) -> Result<()>,
) -> Result<native_persistence::heightmap::BlockHeightFlags> {
    let name = String::from_utf16(&saved.name.0).context("invalid block state name")?;
    let mut properties = defaults.get(&name).cloned().unwrap_or_default();
    let mut seen = BTreeSet::new();
    for (key, value) in &saved.properties {
        let key = String::from_utf16(&key.0).context("invalid block property name")?;
        let value = String::from_utf16(&value.0).context("invalid block property value")?;
        ensure!(seen.insert(key.clone()), "duplicate block property");
        properties.insert(key, value);
    }
    let state = BlockState { name, properties };
    validate_state(&state)?;
    let motion = tags.for_block(&state.name)?;
    Ok(native_persistence::heightmap::BlockHeightFlags {
        is_air: matches!(
            state.name.as_str(),
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
        ),
        blocks_motion: motion.blocks_motion,
        blocks_motion_no_leaves: motion.blocks_motion_no_leaves,
        has_fluid: native_world::fluid::FluidState::from_block(&state)
            .map_err(anyhow::Error::msg)?
            .is_some(),
    })
}

fn unique_field<'a>(value: &'a Tag, name: &str) -> Result<Option<&'a Tag>> {
    let Tag::Compound(fields) = value else {
        bail!("saved chunk data is not a compound");
    };
    let mut matches = fields.iter().filter(|(key, _)| key.matches(name));
    let found = matches.next().map(|(_, value)| value);
    ensure!(matches.next().is_none(), "duplicate saved field {name}");
    Ok(found)
}

fn validate_document(document: &NamedTag, pos: native_persistence::world::ChunkPos) -> Result<()> {
    let root = &document.value;
    ensure!(
        unique_field(root, "DataVersion")? == Some(&Tag::Int(TARGET_DATA_VERSION))
            && unique_field(root, "xPos")? == Some(&Tag::Int(pos.x))
            && unique_field(root, "zPos")? == Some(&Tag::Int(pos.z))
            && matches!(unique_field(root, "Status")?, Some(Tag::String(status)) if status.matches("minecraft:full")),
        "saved chunk metadata differs from its source"
    );
    let _ = unique_field(root, "sections")?;
    Ok(())
}

fn saved_heightmaps(document: &NamedTag, bounds: BuildBounds) -> Result<FullHeightmaps> {
    let source = unique_field(&document.value, "Heightmaps")?
        .context("saved FULL heightmaps are missing")?;
    let names = [
        "WORLD_SURFACE",
        "OCEAN_FLOOR",
        "MOTION_BLOCKING",
        "MOTION_BLOCKING_NO_LEAVES",
    ];
    let mut fields = Vec::with_capacity(names.len());
    for name in names {
        fields.push((
            name.into(),
            unique_field(source, name)?
                .with_context(|| format!("saved FULL heightmap {name} is missing"))?
                .clone(),
        ));
    }
    FullHeightmaps::from_converted(
        HeightBounds {
            min_y: bounds.min_y,
            max_y: bounds.max_y,
        },
        &Tag::Compound(fields),
    )
}

#[cfg(test)]
#[path = "saved_postprocessing_tests.rs"]
mod tests;
