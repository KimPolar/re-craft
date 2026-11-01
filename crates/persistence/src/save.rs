//! Experimental, recoverable region rewrites. Gameplay changes remain gated on
//! the caller proving that all dependent world data is included.
//!
//! A whole region is copied to a same-directory staging file, checked, backed up,
//! and atomically replaced while holding the vanilla world lock. Other chunks are
//! copied byte for byte. A chunk draft can be committed only after heightmaps
//! are rebuilt and the active game registry approves every edit's external
//! effects. The game session does not use this API until its other changed
//! world data can be committed in the same transaction.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Context, Result, ensure};
use flate2::{
    Compression,
    write::{GzEncoder, ZlibEncoder},
};
use sha2::{Digest, Sha256};

use crate::{
    anvil::{self, RegionReader},
    nbt::{self, Tag},
    palette,
    world::{DimensionId, TARGET_DATA_VERSION},
};

const SECTOR_SIZE: u64 = 4096;
const HEADER_SIZE: u64 = 2 * SECTOR_SIZE;
const REGION_SIDE: i32 = 32;
const MAX_OFFSET: u64 = 0x00ff_ffff;

fn ordinary(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return false;
        }
    }
    true
}

/// The backup contains the original region and any external chunks it references.
#[derive(Debug)]
pub struct RewriteResult {
    pub region_path: PathBuf,
    pub backup_directory: PathBuf,
}

/// A partial world save. Regions and optional player data are replaced in
/// one recoverable transaction; level, POI effects, and other files are not
/// implied by this type.
#[derive(Default)]
pub struct WorldSavePlan {
    chunks: Vec<PlannedChunk>,
    entity_chunks: Vec<PlannedEntityChunk>,
    poi_chunks: Vec<PlannedEntityChunk>,
    players: Vec<PlannedPlayerData>,
    world_clocks: Option<PlannedWorldClocks>,
    weather: Option<PlannedSharedData>,
    random_sequences: Option<PlannedSharedData>,
    level_time: Option<PlannedLevelTime>,
}

struct PlannedChunk {
    dimension: DimensionId,
    draft: crate::edit::ChunkDraft,
    region_timestamp_seconds: u32,
}

struct PlannedEntityChunk {
    dimension: DimensionId,
    position: crate::world::ChunkPos,
    source_nbt: Option<Vec<u8>>,
    replacement: Option<Vec<u8>>,
    region_timestamp_seconds: u32,
}

#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
enum RegionKind {
    Blocks,
    Entities,
    Poi,
}

struct RegionChange {
    position: crate::world::ChunkPos,
    source_nbt: Option<Vec<u8>>,
    replacement: Option<Vec<u8>>,
    timestamp: u32,
}

struct PlannedPlayerData {
    uuid_words: [i32; 4],
    source_nbt: Vec<u8>,
    document: crate::nbt::NamedTag,
}

struct PlannedWorldClocks {
    source_nbt: Vec<u8>,
    document: crate::nbt::NamedTag,
}

struct PlannedSharedData {
    source_nbt: Option<Vec<u8>>,
    document: crate::nbt::NamedTag,
}

struct PlannedLevelTime {
    source_nbt: Vec<u8>,
    document: nbt::NamedTag,
}

impl WorldSavePlan {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_chunk(
        &mut self,
        dimension: DimensionId,
        draft: crate::edit::ChunkDraft,
        region_timestamp_seconds: u32,
    ) -> Result<()> {
        let position = draft.position();
        ensure!(
            !self.chunks.iter().any(|entry| {
                entry.dimension == dimension && entry.draft.position() == position
            }),
            "world save plan contains a duplicate chunk"
        );
        self.chunks.push(PlannedChunk {
            dimension,
            draft,
            region_timestamp_seconds,
        });
        Ok(())
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn poi_chunk_count(&self) -> usize {
        self.poi_chunks.len()
    }

    pub fn has_weather(&self) -> bool {
        self.weather.is_some()
    }

    pub fn has_random_sequences(&self) -> bool {
        self.random_sequences.is_some()
    }

    pub fn has_level_time(&self) -> bool {
        self.level_time.is_some()
    }

    /// Add a POI chunk to a coordinated partial transaction. The caller must
    /// validate every POI effect, terrain edit and villager ticket change.
    /// This API does not authorize saving an active gameplay session.
    pub fn add_poi_chunk(
        &mut self,
        dimension: DimensionId,
        position: crate::world::ChunkPos,
        source: Option<&nbt::NamedTag>,
        document: &nbt::NamedTag,
        region_timestamp_seconds: u32,
        mut validate: impl FnMut(Option<&nbt::NamedTag>, &nbt::NamedTag) -> Result<()>,
    ) -> Result<()> {
        ensure!(
            !self
                .poi_chunks
                .iter()
                .any(|chunk| chunk.dimension == dimension && chunk.position == position),
            "world save plan contains a duplicate POI chunk"
        );
        validate(source, document)?;
        validate_poi_preservation(source, document, position)?;
        self.poi_chunks.push(PlannedEntityChunk {
            dimension,
            position,
            source_nbt: source.map(nbt::write).transpose()?,
            replacement: Some(nbt::write(document)?),
            region_timestamp_seconds,
        });
        Ok(())
    }

    /// Add an entity chunk to a coordinated partial save. The caller must
    /// validate entity ownership, movement and all simulated side effects.
    /// Missing entity directories are created before region staging and may
    /// remain empty if staging fails or the process is interrupted.
    pub fn add_entity_chunk(
        &mut self,
        dimension: DimensionId,
        position: crate::world::ChunkPos,
        source: Option<&nbt::NamedTag>,
        document: &nbt::NamedTag,
        region_timestamp_seconds: u32,
        mut validate: impl FnMut(Option<&nbt::NamedTag>, &nbt::NamedTag) -> Result<()>,
    ) -> Result<()> {
        ensure!(
            !self
                .entity_chunks
                .iter()
                .any(|chunk| chunk.dimension == dimension && chunk.position == position),
            "world save plan contains a duplicate entity chunk"
        );
        validate(source, document)?;
        if let Some(source) = source {
            validate_entity_document(source, position)?;
            ensure!(
                source.name == document.name
                    && source.value.get("DataVersion") == document.value.get("DataVersion")
                    && source.value.get("Position") == document.value.get("Position"),
                "entity chunk identity changed"
            );
            let Tag::Compound(before) = &source.value else {
                unreachable!()
            };
            let Tag::Compound(after) = &document.value else {
                anyhow::bail!("entity chunk must be a compound")
            };
            let other_fields = |fields: &[(nbt::JavaString, Tag)]| {
                fields
                    .iter()
                    .filter(|(name, _)| {
                        !["DataVersion", "Position", "Entities"]
                            .iter()
                            .any(|key| name.matches(key))
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            };
            let encoded_fields = |fields: &[(nbt::JavaString, Tag)]| {
                nbt::write(&nbt::NamedTag {
                    name: "".into(),
                    value: Tag::Compound(other_fields(fields)),
                })
            };
            ensure!(
                encoded_fields(before)? == encoded_fields(after)?,
                "unrecognized entity chunk fields changed"
            );
        }
        let empty = validate_entity_document(document, position)?;
        let Tag::Compound(fields) = &document.value else {
            unreachable!()
        };
        let has_extra_fields = fields.iter().any(|(name, _)| {
            !["DataVersion", "Position", "Entities"]
                .iter()
                .any(|key| name.matches(key))
        });
        let preserve_empty = !document.name.0.is_empty() || has_extra_fields;
        self.entity_chunks.push(PlannedEntityChunk {
            dimension,
            position,
            source_nbt: source.map(nbt::write).transpose()?,
            replacement: if empty && !preserve_empty {
                None
            } else {
                Some(nbt::write(document)?)
            },
            region_timestamp_seconds,
        });
        Ok(())
    }

    /// Add one existing player's replacement to the same transaction as all
    /// planned chunks. The caller must validate the player-state draft against
    /// its captured source before adding it.
    pub fn add_player_data(
        &mut self,
        uuid_words: [i32; 4],
        source: &crate::nbt::NamedTag,
        document: &crate::nbt::NamedTag,
        mut validate: impl FnMut(&crate::nbt::NamedTag, &crate::nbt::NamedTag) -> Result<()>,
    ) -> Result<()> {
        ensure!(
            !self
                .players
                .iter()
                .any(|player| player.uuid_words == uuid_words),
            "world save plan contains duplicate player data"
        );
        validate(source, document)?;
        validate_player_document(source)?;
        validate_player_document(document)?;
        self.players.push(PlannedPlayerData {
            uuid_words,
            source_nbt: nbt::write(source)?,
            document: document.clone(),
        });
        Ok(())
    }

    /// Add a replacement for the 26.3 shared clock file to this transaction.
    /// Unknown document and per-clock fields are carried forward from `source`.
    pub fn add_world_clocks(
        &mut self,
        source: &crate::nbt::NamedTag,
        clocks: &BTreeMap<DimensionId, crate::world::SavedClock>,
    ) -> Result<()> {
        ensure!(
            self.world_clocks.is_none(),
            "world save plan contains duplicate world clocks"
        );
        let document = crate::world::WorldReader::update_world_clock_document(source, clocks)?;
        self.world_clocks = Some(PlannedWorldClocks {
            source_nbt: nbt::write(source)?,
            document,
        });
        Ok(())
    }

    /// Stage shared 26.3 weather with all unknown source fields preserved.
    /// An absent source requires the file still to be absent at commit time.
    pub fn add_weather(
        &mut self,
        source: Option<&nbt::NamedTag>,
        weather: crate::world::SavedWeather,
    ) -> Result<()> {
        ensure!(
            self.weather.is_none(),
            "world save plan contains duplicate weather data"
        );
        let document = crate::world::WorldReader::update_weather_document(source, weather)?;
        self.weather = Some(PlannedSharedData {
            source_nbt: source.map(nbt::write).transpose()?,
            document,
        });
        Ok(())
    }

    /// Stage a changed shared random stream in the same transaction as its consumers.
    pub fn add_random_sequences(
        &mut self,
        sequences: &crate::random_sequences::RandomSequences,
    ) -> Result<()> {
        ensure!(
            self.random_sequences.is_none(),
            "duplicate random sequence data"
        );
        ensure!(sequences.is_dirty(), "random sequences have not changed");
        let document = sequences.draft()?;
        crate::random_sequences::RandomSequences::read(Some(document.clone()))?;
        self.random_sequences = Some(PlannedSharedData {
            source_nbt: sequences.source().map(nbt::write).transpose()?,
            document,
        });
        Ok(())
    }

    /// Stage a time-only level.dat update and preserve every other NBT field.
    pub fn add_level_time(&mut self, source: &nbt::NamedTag, time: i64) -> Result<()> {
        ensure!(
            self.level_time.is_none(),
            "world save plan contains duplicate level data"
        );
        let document = crate::world::WorldReader::update_level_game_time(source, time)?;
        self.level_time = Some(PlannedLevelTime {
            source_nbt: nbt::write(source)?,
            document,
        });
        Ok(())
    }
}

#[derive(Debug)]
pub struct WorldSaveResult {
    pub region_paths: Vec<PathBuf>,
    pub entity_region_paths: Vec<PathBuf>,
    pub poi_region_paths: Vec<PathBuf>,
    pub player_paths: Vec<PathBuf>,
    pub world_clocks_path: Option<PathBuf>,
    pub weather_path: Option<PathBuf>,
    pub random_sequences_path: Option<PathBuf>,
    pub level_path: Option<PathBuf>,
    pub backup_directory: PathBuf,
}

fn validate_weather_document(document: &nbt::NamedTag) -> Result<()> {
    crate::world::parse_weather_data(document)?;
    Ok(())
}

fn validate_random_sequences_document(document: &nbt::NamedTag) -> Result<()> {
    crate::random_sequences::RandomSequences::read(Some(document.clone()))?;
    Ok(())
}

fn validate_named_chest_sequences(
    changes: &[&crate::edit::CheckedChestInventory],
    planned: Option<&PlannedSharedData>,
) -> Result<Option<i64>> {
    let mut proofs: BTreeMap<_, Vec<_>> = BTreeMap::new();
    let mut seed = None;
    for proof in changes
        .iter()
        .filter_map(|change| change.generated.as_ref())
    {
        let Some(sequence) = &proof.sequence else {
            continue;
        };
        if let Some(existing) = seed {
            ensure!(
                existing == sequence.world_seed,
                "chest loot world seeds differ"
            );
        } else {
            seed = Some(sequence.world_seed);
        }
        proofs
            .entry(sequence.id.clone())
            .or_default()
            .push(sequence);
    }
    let Some(seed) = seed else { return Ok(None) };
    let planned = planned.context("named chest loot requires a random-sequence transaction")?;
    let source = crate::random_sequences::RandomSequences::read(
        planned
            .source_nbt
            .as_ref()
            .map(|bytes| nbt::read(bytes))
            .transpose()?,
    )?;
    let replacement =
        crate::random_sequences::RandomSequences::read(Some(planned.document.clone()))?;
    for (id, events) in proofs {
        let id = format!("{}:{}", id.namespace, id.path);
        let mut current = source.get(&id, seed)?.state();
        let mut by_start = BTreeMap::new();
        for event in events {
            ensure!(
                by_start.insert(event.before, event.after).is_none(),
                "ambiguous chest loot random sequence order"
            );
        }
        while !by_start.is_empty() {
            current = by_start
                .remove(&current)
                .context("chest loot random sequence has a gap")?;
        }
        ensure!(
            replacement.get(&id, seed)?.state() == current,
            "saved random sequence does not match chest loot"
        );
    }
    Ok(Some(seed))
}

fn validate_poi_preservation(
    source: Option<&nbt::NamedTag>,
    document: &nbt::NamedTag,
    position: crate::world::ChunkPos,
) -> Result<()> {
    let after = crate::world::decode_poi_chunk(document.clone(), position)?;
    let Some(source) = source else { return Ok(()) };
    let before = crate::world::decode_poi_chunk(source.clone(), position)?;
    ensure!(source.name == document.name, "POI chunk root name changed");
    fn extras(tag: &Tag, known: &[&str]) -> Result<Vec<u8>> {
        let Tag::Compound(fields) = tag else {
            anyhow::bail!("POI value must be a compound")
        };
        nbt::write(&nbt::NamedTag {
            name: "".into(),
            value: Tag::Compound(
                fields
                    .iter()
                    .filter(|(name, _)| !known.iter().any(|key| name.matches(key)))
                    .cloned()
                    .collect(),
            ),
        })
    }
    let empty = extras(&Tag::Compound(vec![]), &[])?;
    ensure!(
        extras(&source.value, &["DataVersion", "Sections"])?
            == extras(&document.value, &["DataVersion", "Sections"])?,
        "unrecognized POI chunk fields changed"
    );
    let before_sections = source.value.get("Sections").unwrap();
    let after_sections = document.value.get("Sections").unwrap();
    for (y, section) in &before.sections {
        let key = y.to_string();
        let raw_before = before_sections.get(&key).unwrap();
        let raw_after = after_sections.get(&key);
        let section_extra = extras(raw_before, &["Valid", "Records"])?;
        match raw_after {
            Some(raw) => ensure!(
                section_extra == extras(raw, &["Valid", "Records"])?,
                "unrecognized POI section fields changed"
            ),
            None => ensure!(
                section_extra == empty,
                "removing POI section would discard unrecognized fields"
            ),
        }
        let Some(Tag::List {
            values: old_records,
            ..
        }) = raw_before.get("Records")
        else {
            unreachable!()
        };
        let new_records = raw_after
            .and_then(|raw| raw.get("Records"))
            .and_then(|value| match value {
                Tag::List { values, .. } => Some(values.as_slice()),
                _ => None,
            })
            .unwrap_or(&[]);
        let decoded_after = after.sections.get(y);
        for (record, old_raw) in section.records.iter().zip(old_records) {
            let old_extra = extras(old_raw, &["pos", "type", "free_tickets"])?;
            let new_index = decoded_after.and_then(|section| {
                section
                    .records
                    .iter()
                    .position(|entry| entry.position == record.position)
            });
            match new_index {
                Some(index) => ensure!(
                    old_extra == extras(&new_records[index], &["pos", "type", "free_tickets"])?,
                    "unrecognized POI record fields changed"
                ),
                None => ensure!(
                    old_extra == empty,
                    "removing POI record would discard unrecognized fields"
                ),
            }
        }
    }
    Ok(())
}

fn validate_entity_document(
    document: &nbt::NamedTag,
    position: crate::world::ChunkPos,
) -> Result<bool> {
    let Tag::Compound(fields) = &document.value else {
        anyhow::bail!("entity chunk must be a compound")
    };
    for key in ["DataVersion", "Position", "Entities"] {
        ensure!(
            fields.iter().filter(|(name, _)| name.matches(key)).count() <= 1,
            "duplicate entity chunk {key}"
        );
    }
    crate::world::decode_entity_chunk(document.clone(), position)?;
    Ok(match document.value.get("Entities") {
        None => true,
        Some(Tag::List { values, .. }) => values.is_empty(),
        _ => unreachable!("entity list was validated"),
    })
}

fn validate_player_document(document: &crate::nbt::NamedTag) -> Result<()> {
    let crate::nbt::Tag::Compound(fields) = &document.value else {
        anyhow::bail!("player data must be an NBT compound")
    };
    let versions: Vec<_> = fields
        .iter()
        .filter(|(name, _)| name.matches("DataVersion"))
        .collect();
    ensure!(
        matches!(
            versions.as_slice(),
            [(_, crate::nbt::Tag::Int(TARGET_DATA_VERSION))]
        ),
        "player data must contain one Minecraft 26.3 DataVersion"
    );
    nbt::write(document).context("cannot encode player data")?;
    Ok(())
}

fn player_data_filename(uuid_words: [i32; 4]) -> String {
    let mut bytes = [0; 16];
    for (index, word) in uuid_words.iter().enumerate() {
        bytes[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    let hex = format!("{:032x}", u128::from_be_bytes(bytes));
    format!(
        "{}-{}-{}-{}-{}.dat",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// A recoverable native save was interrupted; opening must wait for recovery.
#[derive(Debug)]
pub struct InterruptedWorldSave;

impl std::fmt::Display for InterruptedWorldSave {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(
            "an interrupted Minecraft Native save needs recovery before this world can be opened",
        )
    }
}

impl std::error::Error for InterruptedWorldSave {}

/// Another process or session currently holds the world's Java lock.
#[derive(Debug)]
pub struct WorldInUse;

impl std::fmt::Display for WorldInUse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("world is already in use")
    }
}

impl std::error::Error for WorldInUse {}

/// Keeps the vanilla session lock alive while a world is in use. Clones share
/// the same OS lock, including across the client and its chunk reader.
#[derive(Clone)]
pub struct WorldSessionGuard {
    shared: Arc<SessionLock>,
}

struct SessionLock {
    root: PathBuf,
    _lock: WorldLock,
    writer_active: AtomicBool,
}

impl WorldSessionGuard {
    pub(crate) fn acquire(root: &Path) -> Result<Self> {
        Ok(Self {
            shared: Arc::new(SessionLock {
                root: root.to_path_buf(),
                _lock: WorldLock::acquire(root)?,
                writer_active: AtomicBool::new(false),
            }),
        })
    }

    /// Reuse this session's OS lock for a single writer. The caller must pause
    /// conflicting reads and coordinate the simulation before a gameplay save.
    /// Only the existing lossless rewrite operation is currently available.
    pub fn acquire_write(&self) -> Result<WorldWriteGuard> {
        ensure!(
            self.shared
                .writer_active
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok(),
            "another world write is already in progress"
        );
        let guard = WorldWriteGuard {
            root: self.shared.root.clone(),
            session: self.clone(),
        };
        crate::save_transaction::ensure_no_pending(&guard.root)?;
        let level = anvil::read_level(&guard.root)?;
        ensure!(
            level.data_version == TARGET_DATA_VERSION,
            "unsupported world DataVersion {}; expected {TARGET_DATA_VERSION}",
            level.data_version
        );
        Ok(guard)
    }
}

/// Holds the same exclusive session.lock used by the Java client.
pub struct WorldWriteGuard {
    root: PathBuf,
    session: WorldSessionGuard,
}

impl Drop for WorldWriteGuard {
    fn drop(&mut self) {
        self.session
            .shared
            .writer_active
            .store(false, Ordering::Release);
    }
}

impl WorldWriteGuard {
    pub fn acquire(world: &Path) -> Result<Self> {
        let root = world
            .canonicalize()
            .with_context(|| format!("cannot open world {}", world.display()))?;
        ensure!(root.is_dir(), "world path is not a directory");
        WorldSessionGuard::acquire(&root)?.acquire_write()
    }

    /// Roll back a previously interrupted native transaction while holding the
    /// vanilla world lock. The backup is retained, and conflicting files stop
    /// recovery without being overwritten.
    pub fn recover_interrupted(world: &Path) -> Result<bool> {
        let root = world
            .canonicalize()
            .with_context(|| format!("cannot open world {}", world.display()))?;
        ensure!(root.is_dir(), "world path is not a directory");
        let _lock = WorldLock::acquire(&root)?;
        crate::save_transaction::recover(&root)
    }

    /// Recompress an existing full 26.3 chunk without changing its NBT.
    /// No gameplay data is edited by this operation.
    pub fn rewrite_chunk_losslessly(
        &mut self,
        dimension: &DimensionId,
        chunk_x: i32,
        chunk_z: i32,
    ) -> Result<RewriteResult> {
        self.rewrite_chunk_data(dimension, chunk_x, chunk_z, None, None, None)
    }

    /// Persist one validated chunk draft inside a recoverable region rewrite.
    ///
    /// `validate_change` must use the active block registry and world state to
    /// reject every edit whose lighting, block entities, fluids, neighbor
    /// updates, scheduled ticks, POI data, or other chunk dependencies are not
    /// represented by the draft. The game session must additionally coordinate
    /// all other changed chunks and player/world files before enabling saves.
    /// `region_timestamp_seconds` is the Java-compatible region timestamp to
    /// store for this chunk.
    pub fn write_chunk_draft(
        &mut self,
        dimension: &DimensionId,
        draft: &crate::edit::ChunkDraft,
        region_timestamp_seconds: u32,
        validate_change: impl FnMut(&crate::edit::BlockChange) -> Result<()>,
    ) -> Result<RewriteResult> {
        draft.validate_for_write(validate_change)?;
        let position = draft.position();
        let replacement = nbt::write(draft.document())?;
        self.rewrite_chunk_data(
            dimension,
            position.x,
            position.z,
            Some(draft.source_nbt()),
            Some(&replacement),
            Some(region_timestamp_seconds),
        )
    }

    /// Write validated chunk drafts and optional player data as one recoverable
    /// transaction.
    ///
    /// Level data and complete POI, entity, and other side effects are not
    /// implied, so a gameplay session must not enable saving from this plan.
    pub fn write_plan(
        &mut self,
        plan: WorldSavePlan,
        mut validate_change: impl FnMut(&DimensionId, &crate::edit::BlockChange) -> Result<()>,
    ) -> Result<WorldSaveResult> {
        ensure!(
            !plan.chunks.is_empty()
                || !plan.entity_chunks.is_empty()
                || !plan.poi_chunks.is_empty()
                || !plan.players.is_empty()
                || plan.world_clocks.is_some()
                || plan.weather.is_some()
                || plan.random_sequences.is_some()
                || plan.level_time.is_some(),
            "world save plan is empty"
        );
        let chest_changes: Vec<_> = plan
            .chunks
            .iter()
            .flat_map(|chunk| chunk.draft.checked_chest_inventories())
            .collect();
        let named_loot_seed =
            validate_named_chest_sequences(&chest_changes, plan.random_sequences.as_ref())?;
        if !chest_changes.is_empty() {
            ensure!(
                plan.players.len() == 1,
                "chest inventory changes require exactly one player"
            );
            let player = &plan.players[0];
            let source = nbt::read(&player.source_nbt)?;
            crate::chest_transfer::validate(
                &source,
                &player.document,
                player.uuid_words,
                &chest_changes,
            )?;
        }
        let mut groups = BTreeMap::<(RegionKind, DimensionId, i32, i32), Vec<RegionChange>>::new();
        for chunk in &plan.chunks {
            chunk
                .draft
                .validate_for_plan(!chest_changes.is_empty(), |change| {
                    validate_change(&chunk.dimension, change)
                })?;
            let position = chunk.draft.position();
            let replacement = nbt::write(chunk.draft.document())?;
            groups
                .entry((
                    RegionKind::Blocks,
                    chunk.dimension.clone(),
                    position.x.div_euclid(REGION_SIDE),
                    position.z.div_euclid(REGION_SIDE),
                ))
                .or_default()
                .push(RegionChange {
                    position,
                    source_nbt: Some(chunk.draft.source_nbt().to_vec()),
                    replacement: Some(replacement),
                    timestamp: chunk.region_timestamp_seconds,
                });
        }
        for chunk in &plan.entity_chunks {
            let position = chunk.position;
            groups
                .entry((
                    RegionKind::Entities,
                    chunk.dimension.clone(),
                    position.x.div_euclid(REGION_SIDE),
                    position.z.div_euclid(REGION_SIDE),
                ))
                .or_default()
                .push(RegionChange {
                    position,
                    source_nbt: chunk.source_nbt.clone(),
                    replacement: chunk.replacement.clone(),
                    timestamp: chunk.region_timestamp_seconds,
                });
        }
        for chunk in &plan.poi_chunks {
            let position = chunk.position;
            groups
                .entry((
                    RegionKind::Poi,
                    chunk.dimension.clone(),
                    position.x.div_euclid(REGION_SIDE),
                    position.z.div_euclid(REGION_SIDE),
                ))
                .or_default()
                .push(RegionChange {
                    position,
                    source_nbt: chunk.source_nbt.clone(),
                    replacement: chunk.replacement.clone(),
                    timestamp: chunk.region_timestamp_seconds,
                });
        }

        let backup_root = self.root.join(".minecraft-native-backups");
        match fs::symlink_metadata(&backup_root) {
            Ok(meta) => ensure!(
                meta.is_dir() && !meta.file_type().is_symlink(),
                "backup directory must be an ordinary directory"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&backup_root)?
            }
            Err(error) => return Err(error.into()),
        }
        let backup_root = backup_root.canonicalize()?;
        ensure!(
            backup_root.parent() == Some(self.root.as_path()),
            "backup directory is outside the world"
        );
        let backup = tempfile::Builder::new()
            .prefix("transaction-")
            .tempdir_in(&backup_root)?;
        let mut replacements = Vec::with_capacity(
            groups.len()
                + plan.players.len()
                + plan.world_clocks.is_some() as usize
                + plan.weather.is_some() as usize
                + plan.random_sequences.is_some() as usize
                + 2 * plan.level_time.is_some() as usize,
        );
        let mut dependencies = BTreeMap::<PathBuf, PathBuf>::new();
        let mut absent_dependencies = Vec::new();
        if let Some(expected_seed) = named_loot_seed {
            let directory = self.root.join("data/minecraft");
            ensure!(
                crate::save_transaction::checked_directory(&self.root, &directory)?.1,
                "world generation directory is missing"
            );
            let directory = directory.canonicalize()?;
            ensure!(
                directory.starts_with(&self.root),
                "world generation directory is outside the world"
            );
            let path = directory.join("world_gen_settings.dat");
            let metadata =
                fs::symlink_metadata(&path).context("world generation settings are missing")?;
            ensure!(
                metadata.is_file() && ordinary(&metadata),
                "world generation settings must be an ordinary file"
            );
            let path = path.canonicalize()?;
            ensure!(
                path.parent() == Some(directory.as_path()),
                "world generation settings resolve outside the world"
            );
            let bytes = nbt::decompress_limited(File::open(&path)?)?;
            let document = if bytes.starts_with(&[0x1f, 0x8b]) {
                nbt::read_gzip(bytes.as_slice())?
            } else {
                nbt::read(&bytes)?
            };
            ensure!(
                crate::world::WorldReader::world_seed_from_document(&document)? == expected_seed,
                "world seed differs from chest loot proof"
            );
            let backup_dir = backup.path().join("world-data");
            fs::create_dir_all(&backup_dir)?;
            let backup_path = backup_dir.join("world_gen_settings.dat");
            copy_sync(&path, &backup_path)?;
            // Bind the parsed bytes to the dependency copy even if another
            // process changes the settings between reading and staging.
            ensure!(
                hash_file(&backup_path)? == <[u8; 32]>::from(Sha256::digest(&bytes)),
                "world generation settings changed during staging"
            );
            dependencies.insert(path, backup_path);
        }
        let mut extra_directories = BTreeMap::<PathBuf, (bool, bool)>::new();
        for ((kind, dimension, _, _), chunks) in &groups {
            let directory = match kind {
                RegionKind::Blocks => continue,
                RegionKind::Entities => dimension.entity_region_dir(&self.root),
                RegionKind::Poi => dimension.poi_region_dir(&self.root),
            };
            let (_, exists) = crate::save_transaction::checked_directory(&self.root, &directory)?;
            if !exists {
                ensure!(
                    chunks.iter().all(|chunk| chunk.source_nbt.is_none()),
                    "region directory changed after its draft was created"
                );
            }
            let entry = extra_directories
                .entry(directory)
                .or_insert((exists, false));
            ensure!(entry.0 == exists, "region directory changed during staging");
            entry.1 |= chunks.iter().any(|chunk| chunk.replacement.is_some());
        }
        for (directory, (exists, needed)) in &extra_directories {
            if !exists && *needed {
                // Only a single, verified final component is created. A failed
                // save can leave this directory empty, never remove outside data.
                fs::create_dir(directory).with_context(|| {
                    format!("cannot create region directory {}", directory.display())
                })?;
                ensure!(
                    crate::save_transaction::checked_directory(&self.root, directory)?.1,
                    "region directory disappeared"
                );
            } else if !exists {
                absent_dependencies.push(crate::save_transaction::Dependency::AbsentDirectory {
                    target: directory.clone(),
                });
            }
        }
        let mut region_paths = Vec::with_capacity(groups.len());
        let mut entity_region_paths = Vec::new();
        let mut poi_region_paths = Vec::new();
        let mut player_paths = Vec::with_capacity(plan.players.len());
        let mut world_clocks_path = None;
        let mut weather_path = None;
        let mut random_sequences_path = None;
        let mut level_path = None;

        for (group_index, ((kind, dimension, region_x, region_z), chunks)) in
            groups.into_iter().enumerate()
        {
            let directory = match kind {
                RegionKind::Blocks => dimension.region_dir(&self.root),
                RegionKind::Entities => dimension.entity_region_dir(&self.root),
                RegionKind::Poi => dimension.poi_region_dir(&self.root),
            };
            if kind != RegionKind::Blocks
                && extra_directories.get(&directory) == Some(&(false, false))
            {
                continue;
            }
            let region_dir = directory
                .canonicalize()
                .context("cannot open dimension region directory")?;
            ensure!(
                region_dir.starts_with(&self.root),
                "region directory is outside the world"
            );
            let name = format!("r.{region_x}.{region_z}.mca");
            let region_path = region_dir.join(&name);
            let existing = match fs::symlink_metadata(&region_path) {
                Ok(metadata) => {
                    ensure!(
                        metadata.is_file() && ordinary(&metadata),
                        "region must be an ordinary file"
                    );
                    let canonical = region_path.canonicalize()?;
                    ensure!(
                        canonical.parent() == Some(region_dir.as_path()),
                        "region resolves outside its directory"
                    );
                    Some(canonical)
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        && kind != RegionKind::Blocks =>
                {
                    None
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("cannot inspect region {}", region_path.display())
                    });
                }
            };
            if existing.is_none() && chunks.iter().all(|chunk| chunk.replacement.is_none()) {
                ensure!(
                    chunks.iter().all(|chunk| chunk.source_nbt.is_none()),
                    "region changed after its draft was created"
                );
                absent_dependencies.push(crate::save_transaction::Dependency::Absent {
                    target: region_path,
                });
                continue;
            }
            let target = existing.clone().unwrap_or_else(|| region_path.clone());

            let mut stage = tempfile::Builder::new()
                .prefix(".native-region-")
                .tempfile_in(&region_dir)?;
            if let Some(path) = &existing {
                let mut original = File::open(path)?;
                let original_length = original.metadata()?.len();
                ensure!(
                    original_length >= HEADER_SIZE
                        && original_length.is_multiple_of(SECTOR_SIZE)
                        && original_length / SECTOR_SIZE <= MAX_OFFSET + 1,
                    "invalid or unsupported region file length"
                );
                std::io::copy(&mut original, stage.as_file_mut())?;
            } else {
                stage.as_file_mut().set_len(HEADER_SIZE)?;
            }
            stage.as_file_mut().flush()?;
            let original_hash = existing
                .as_ref()
                .map(|_| hash_file(stage.path()))
                .transpose()?;
            let external = audit_region(stage.as_file_mut(), &region_dir, region_x, region_z)?;

            let group_backup = backup.path().join(format!("region-{group_index}"));
            fs::create_dir(&group_backup)?;
            if let Some(path) = &existing {
                copy_sync(path, &group_backup.join(&name))?;
            }
            for path in &external {
                let file_name = path.file_name().context("external chunk has no filename")?;
                let backup_path = group_backup.join(file_name);
                if !backup_path.exists() {
                    copy_sync(path, &backup_path)?;
                }
                dependencies.insert(path.clone(), backup_path);
            }

            let mut expected_chunks = Vec::with_capacity(chunks.len());
            for planned in chunks {
                let position = planned.position;
                let original_nbt = RegionReader::open_for_region(stage.path(), region_x, region_z)?
                    .read_chunk_nbt_bytes(position.x, position.z)?;
                ensure!(
                    planned.source_nbt == original_nbt,
                    "chunk changed after its draft was created"
                );
                if let Some(original) = &original_nbt {
                    let source = nbt::read(original)?;
                    match kind {
                        RegionKind::Blocks => {
                            validate_chunk(&source.value, position.x, position.z)?
                        }
                        RegionKind::Entities => {
                            validate_entity_document(&source, position)?;
                        }
                        RegionKind::Poi => {
                            crate::world::decode_poi_chunk(source, position)?;
                        }
                    }
                }
                let mut external_replacement = false;
                if let Some(replacement) = &planned.replacement {
                    let draft = nbt::read(replacement).context("invalid chunk draft NBT")?;
                    match kind {
                        RegionKind::Blocks => validate_chunk(&draft.value, position.x, position.z)?,
                        RegionKind::Entities => {
                            validate_entity_document(&draft, position)?;
                        }
                        RegionKind::Poi => {
                            crate::world::decode_poi_chunk(draft.clone(), position)?;
                        }
                    }
                    ensure!(
                        nbt::write(&draft)? == *replacement,
                        "chunk draft cannot be reserialized without changing its bytes"
                    );
                    let external_path =
                        region_dir.join(format!("c.{}.{}.mcc", position.x, position.z));
                    let retain_external = dependencies.contains_key(&external_path);
                    if let Some(compressed) = write_staged_chunk(
                        stage.as_file_mut(),
                        position.x,
                        position.z,
                        replacement,
                        Some(planned.timestamp),
                        retain_external,
                    )? {
                        let original_backup = dependencies.remove(&external_path);
                        replacements.push(stage_external_chunk(
                            &region_dir,
                            position.x,
                            position.z,
                            &compressed,
                            replacement,
                            original_backup,
                        )?);
                        external_replacement = true;
                    }
                } else if original_nbt.is_some() {
                    let external_file =
                        region_dir.join(format!("c.{}.{}.mcc", position.x, position.z));
                    match fs::symlink_metadata(&external_file) {
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Ok(_) => anyhow::bail!(
                            "clearing a chunk with external storage is not yet supported"
                        ),
                        Err(error) => return Err(error.into()),
                    }
                    clear_staged_chunk(
                        stage.as_file_mut(),
                        position.x,
                        position.z,
                        planned.timestamp,
                    )?;
                }
                expected_chunks.push((position, planned.replacement, external_replacement));
            }
            stage.as_file_mut().flush()?;
            stage.as_file().sync_all()?;
            for (position, expected, external_replacement) in expected_chunks {
                if external_replacement {
                    verify_external_marker(stage.as_file_mut(), position.x, position.z)?;
                } else {
                    let saved = RegionReader::open_for_region(stage.path(), region_x, region_z)?
                        .read_chunk_nbt_bytes(position.x, position.z)?;
                    ensure!(
                        saved == expected,
                        "staged chunk failed the NBT round-trip check"
                    );
                }
            }
            replacements.push(crate::save_transaction::Replacement {
                target: target.clone(),
                backup: existing.map(|_| group_backup.join(name)),
                stage,
                original_hash,
            });
            match kind {
                RegionKind::Blocks => region_paths.push(target),
                RegionKind::Entities => entity_region_paths.push(target),
                RegionKind::Poi => poi_region_paths.push(target),
            }
        }

        for planned in &plan.players {
            let mut player_directory = self.root.clone();
            for component in ["players", "data"] {
                player_directory.push(component);
                let player_dir_metadata = fs::symlink_metadata(&player_directory)
                    .context("cannot inspect player data directory")?;
                ensure!(
                    player_dir_metadata.is_dir() && ordinary(&player_dir_metadata),
                    "player data path must contain ordinary directories"
                );
            }
            let player_directory = player_directory.canonicalize()?;
            ensure!(
                player_directory.starts_with(&self.root),
                "player data directory is outside the world"
            );
            let filename = player_data_filename(planned.uuid_words);
            let player_path = player_directory.join(&filename);
            let metadata = fs::symlink_metadata(&player_path)
                .context("cannot inspect existing player data")?;
            ensure!(
                metadata.is_file() && ordinary(&metadata),
                "player data must be an ordinary file"
            );
            let canonical = player_path.canonicalize()?;
            ensure!(
                canonical.parent() == Some(player_directory.as_path()),
                "player data resolves outside its directory"
            );
            let original_bytes = nbt::decompress_limited(File::open(&canonical)?)?;
            let current = nbt::read_gzip(original_bytes.as_slice())
                .context("cannot decode current player data")?;
            ensure!(
                nbt::write(&current)? == planned.source_nbt,
                "player data changed after its draft was created"
            );
            let original_hash: [u8; 32] = Sha256::digest(&original_bytes).into();
            let player_backup_dir = backup.path().join("players");
            fs::create_dir_all(&player_backup_dir)?;
            let player_backup = player_backup_dir.join(&filename);
            copy_sync(&canonical, &player_backup)?;

            let replacement = nbt::write(&planned.document)?;
            let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
            gzip.write_all(&replacement)?;
            let compressed = gzip.finish()?;
            let mut stage = tempfile::Builder::new()
                .prefix(".native-stage-")
                .tempfile_in(&player_directory)?;
            stage.write_all(&compressed)?;
            stage.flush()?;
            stage.as_file().sync_all()?;
            let staged = nbt::read_gzip(File::open(stage.path())?)?;
            ensure!(
                nbt::write(&staged)? == replacement,
                "staged player data failed the NBT round-trip check"
            );
            replacements.push(crate::save_transaction::Replacement {
                target: canonical.clone(),
                backup: Some(player_backup),
                stage,
                original_hash: Some(original_hash),
            });
            player_paths.push(canonical);
        }

        if let Some(planned) = &plan.world_clocks {
            let mut clock_directory = self.root.clone();
            for component in ["data", "minecraft"] {
                clock_directory.push(component);
                let metadata = fs::symlink_metadata(&clock_directory)
                    .context("cannot inspect world clock data directory")?;
                ensure!(
                    metadata.is_dir() && ordinary(&metadata),
                    "world clock data path must contain ordinary directories"
                );
            }
            let clock_directory = clock_directory.canonicalize()?;
            ensure!(
                clock_directory.starts_with(&self.root),
                "world clock data directory is outside the world"
            );
            let clock_path = clock_directory.join("world_clocks.dat");
            let metadata = fs::symlink_metadata(&clock_path)
                .context("cannot inspect existing world clock data")?;
            ensure!(
                metadata.is_file() && ordinary(&metadata),
                "world clock data must be an ordinary file"
            );
            let canonical = clock_path.canonicalize()?;
            ensure!(
                canonical.parent() == Some(clock_directory.as_path()),
                "world clock data resolves outside its directory"
            );
            let original_bytes = nbt::decompress_limited(File::open(&canonical)?)?;
            let compressed = original_bytes.starts_with(&[0x1f, 0x8b]);
            let current = if compressed {
                nbt::read_gzip(original_bytes.as_slice())
            } else {
                nbt::read(&original_bytes)
            }
            .context("cannot decode current world clock data")?;
            ensure!(
                nbt::write(&current)? == planned.source_nbt,
                "world clock data changed after its draft was created"
            );
            let original_hash: [u8; 32] = Sha256::digest(&original_bytes).into();
            let clock_backup_dir = backup.path().join("world-data");
            fs::create_dir_all(&clock_backup_dir)?;
            let clock_backup = clock_backup_dir.join("world_clocks.dat");
            copy_sync(&canonical, &clock_backup)?;

            let replacement = nbt::write(&planned.document)?;
            let staged_bytes = if compressed {
                let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
                gzip.write_all(&replacement)?;
                gzip.finish()?
            } else {
                replacement.clone()
            };
            let mut stage = tempfile::Builder::new()
                .prefix(".native-stage-")
                .tempfile_in(&clock_directory)?;
            stage.write_all(&staged_bytes)?;
            stage.flush()?;
            stage.as_file().sync_all()?;
            let staged = if compressed {
                nbt::read_gzip(File::open(stage.path())?)?
            } else {
                nbt::read(&staged_bytes)?
            };
            ensure!(
                nbt::write(&staged)? == replacement,
                "staged world clock data failed the NBT round-trip check"
            );
            replacements.push(crate::save_transaction::Replacement {
                target: canonical.clone(),
                backup: Some(clock_backup),
                stage,
                original_hash: Some(original_hash),
            });
            world_clocks_path = Some(canonical);
        }

        for (planned, filename, label, validate) in [
            (
                &plan.weather,
                "weather.dat",
                "weather",
                validate_weather_document as fn(&nbt::NamedTag) -> Result<()>,
            ),
            (
                &plan.random_sequences,
                "random_sequences.dat",
                "random sequences",
                validate_random_sequences_document,
            ),
        ] {
            let Some(planned) = planned else { continue };
            let mut directory = self.root.clone();
            for component in ["data", "minecraft"] {
                directory.push(component);
                let (_, exists) =
                    crate::save_transaction::checked_directory(&self.root, &directory)?;
                if !exists {
                    ensure!(
                        planned.source_nbt.is_none(),
                        "{label} data directory changed after its draft was created"
                    );
                    fs::create_dir(&directory)?;
                    ensure!(
                        crate::save_transaction::checked_directory(&self.root, &directory)?.1,
                        "{label} data directory disappeared"
                    );
                }
            }
            let directory = directory.canonicalize()?;
            ensure!(
                directory.starts_with(&self.root),
                "{label} data directory is outside the world"
            );
            let path = directory.join(filename);
            let current = match fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    ensure!(
                        metadata.is_file() && ordinary(&metadata),
                        "{label} data must be an ordinary file"
                    );
                    let canonical = path.canonicalize()?;
                    ensure!(
                        canonical.parent() == Some(directory.as_path()),
                        "{label} data resolves outside its directory"
                    );
                    let bytes = nbt::decompress_limited(File::open(&canonical)?)?;
                    let compressed = bytes.starts_with(&[0x1f, 0x8b]);
                    let document = if compressed {
                        nbt::read_gzip(bytes.as_slice())?
                    } else {
                        nbt::read(&bytes)?
                    };
                    validate(&document)?;
                    ensure!(
                        planned
                            .source_nbt
                            .as_ref()
                            .is_some_and(|expected| nbt::write(&document)
                                .is_ok_and(|actual| &actual == expected)),
                        "{label} data changed after its draft was created"
                    );
                    let original_hash: [u8; 32] = Sha256::digest(&bytes).into();
                    let backup_dir = backup.path().join("world-data");
                    fs::create_dir_all(&backup_dir)?;
                    let shared_backup = backup_dir.join(filename);
                    copy_sync(&canonical, &shared_backup)?;
                    Some((canonical, shared_backup, original_hash, compressed))
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    ensure!(
                        planned.source_nbt.is_none(),
                        "{label} data disappeared after its draft was created"
                    );
                    None
                }
                Err(error) => {
                    return Err(error).with_context(|| format!("cannot inspect {label} data"));
                }
            };
            let replacement = nbt::write(&planned.document)?;
            let compressed = current
                .as_ref()
                .is_none_or(|(_, _, _, compressed)| *compressed);
            let bytes = if compressed {
                let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
                gzip.write_all(&replacement)?;
                gzip.finish()?
            } else {
                replacement.clone()
            };
            let mut stage = tempfile::Builder::new()
                .prefix(".native-stage-")
                .tempfile_in(&directory)?;
            stage.write_all(&bytes)?;
            stage.flush()?;
            stage.as_file().sync_all()?;
            let staged = if compressed {
                nbt::read_gzip(File::open(stage.path())?)?
            } else {
                nbt::read(&bytes)?
            };
            ensure!(
                nbt::write(&staged)? == replacement,
                "staged {label} data failed the NBT round-trip check"
            );
            validate(&staged)?;
            let (target, backup, original_hash) = match current {
                Some((target, backup, hash, _)) => (target, Some(backup), Some(hash)),
                None => (path, None, None),
            };
            replacements.push(crate::save_transaction::Replacement {
                target: target.clone(),
                backup,
                stage,
                original_hash,
            });
            if filename == "weather.dat" {
                weather_path = Some(target);
            } else {
                random_sequences_path = Some(target);
            }
        }

        if let Some(planned) = &plan.level_time {
            let path = self.root.join("level.dat");
            let meta = fs::symlink_metadata(&path).context("cannot inspect level data")?;
            ensure!(
                meta.is_file() && ordinary(&meta),
                "level data must be an ordinary file"
            );
            let canonical = path.canonicalize()?;
            ensure!(
                canonical.parent() == Some(self.root.as_path()),
                "level data resolves outside the world"
            );
            let original = nbt::decompress_limited(File::open(&canonical)?)?;
            let current =
                nbt::read_gzip(original.as_slice()).context("cannot decode level data")?;
            crate::world::WorldReader::validate_level_game_time(&current)?;
            ensure!(
                nbt::write(&current)? == planned.source_nbt,
                "level data changed after its draft was created"
            );
            let original_hash: [u8; 32] = Sha256::digest(&original).into();
            let backup_dir = backup.path().join("world-data");
            fs::create_dir_all(&backup_dir)?;
            let level_backup = backup_dir.join("level.dat");
            copy_sync(&canonical, &level_backup)?;

            let old = self.root.join("level.dat_old");
            let old_source = match fs::symlink_metadata(&old) {
                Ok(meta) => {
                    ensure!(
                        meta.is_file() && ordinary(&meta),
                        "level.dat_old must be an ordinary file"
                    );
                    let old = old.canonicalize()?;
                    ensure!(
                        old.parent() == Some(self.root.as_path()),
                        "level.dat_old resolves outside the world"
                    );
                    let hash = hash_file(&old)?;
                    let old_backup = backup_dir.join("level.dat_old");
                    copy_sync(&old, &old_backup)?;
                    Some((old, old_backup, hash))
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error).context("cannot inspect level.dat_old"),
            };
            let mut old_stage = tempfile::Builder::new()
                .prefix(".native-stage-")
                .tempfile_in(&self.root)?;
            old_stage.write_all(&original)?;
            old_stage.flush()?;
            old_stage.as_file().sync_all()?;
            let (old_target, old_backup, old_hash) = match old_source {
                Some((path, backup, hash)) => (path, Some(backup), Some(hash)),
                None => (old, None, None),
            };
            replacements.push(crate::save_transaction::Replacement {
                target: old_target,
                backup: old_backup,
                stage: old_stage,
                original_hash: old_hash,
            });

            let replacement = nbt::write(&planned.document)?;
            let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
            gzip.write_all(&replacement)?;
            let mut stage = tempfile::Builder::new()
                .prefix(".native-stage-")
                .tempfile_in(&self.root)?;
            stage.write_all(&gzip.finish()?)?;
            stage.flush()?;
            stage.as_file().sync_all()?;
            let staged = nbt::read_gzip(File::open(stage.path())?)?;
            ensure!(
                nbt::write(&staged)? == replacement,
                "staged level data failed the NBT round-trip check"
            );
            crate::world::WorldReader::validate_level_game_time(&staged)?;
            replacements.push(crate::save_transaction::Replacement {
                target: canonical.clone(),
                backup: Some(level_backup),
                stage,
                original_hash: Some(original_hash),
            });
            level_path = Some(canonical);
        }

        let mut note = File::create(backup.path().join("backup.txt"))?;
        writeln!(
            note,
            "Minecraft Native coordinated save backup\nregions={}\nchunks={}\nentity_regions={}\nentity_chunks={}\npoi_regions={}\npoi_chunks={}\nabsent_regions={}\nplayers={}\nworld_clocks={}\nweather={}\nrandom_sequences={}\nlevel={}",
            region_paths.len(),
            plan.chunks.len(),
            entity_region_paths.len(),
            plan.entity_chunks.len(),
            poi_region_paths.len(),
            plan.poi_chunks.len(),
            absent_dependencies.len(),
            plan.players.len(),
            plan.world_clocks.is_some() as usize,
            plan.weather.is_some() as usize,
            plan.random_sequences.is_some() as usize,
            plan.level_time.is_some() as usize
        )?;
        note.sync_all()?;
        let backup_directory = backup.keep();
        let mut dependencies: Vec<_> = dependencies
            .into_iter()
            .map(|(target, backup)| crate::save_transaction::Dependency::Present { target, backup })
            .collect();
        dependencies.extend(absent_dependencies);
        crate::save_transaction::commit(&self.root, &backup_directory, replacements, dependencies)
            .with_context(|| {
                format!(
                    "cannot atomically replace world data; backup retained at {}",
                    backup_directory.display()
                )
            })?;

        Ok(WorldSaveResult {
            region_paths,
            entity_region_paths,
            poi_region_paths,
            player_paths,
            world_clocks_path,
            weather_path,
            random_sequences_path,
            level_path,
            backup_directory,
        })
    }

    fn rewrite_chunk_data(
        &mut self,
        dimension: &DimensionId,
        chunk_x: i32,
        chunk_z: i32,
        expected_nbt: Option<&[u8]>,
        replacement_nbt: Option<&[u8]>,
        timestamp_seconds: Option<u32>,
    ) -> Result<RewriteResult> {
        let region_x = chunk_x.div_euclid(REGION_SIDE);
        let region_z = chunk_z.div_euclid(REGION_SIDE);
        let region_dir = dimension
            .region_dir(&self.root)
            .canonicalize()
            .context("cannot open dimension region directory")?;
        ensure!(
            region_dir.starts_with(&self.root),
            "region directory is outside the world"
        );
        let name = format!("r.{region_x}.{region_z}.mca");
        let region_path = region_dir.join(&name);
        let metadata =
            fs::symlink_metadata(&region_path).context("cannot inspect existing region")?;
        ensure!(
            metadata.is_file() && ordinary(&metadata),
            "region must be an ordinary file"
        );
        let canonical = region_path.canonicalize()?;
        ensure!(
            canonical.parent() == Some(region_dir.as_path()),
            "region resolves outside its directory"
        );

        let mut stage = tempfile::Builder::new()
            .prefix(".native-region-")
            .tempfile_in(&region_dir)
            .context("cannot create region staging file")?;
        let mut original = File::open(&canonical)?;
        let original_length = original.metadata()?.len();
        ensure!(
            original_length >= HEADER_SIZE
                && original_length.is_multiple_of(SECTOR_SIZE)
                && original_length / SECTOR_SIZE <= MAX_OFFSET + 1,
            "invalid or unsupported region file length"
        );
        std::io::copy(&mut original, stage.as_file_mut())
            .context("cannot stage original region")?;
        stage.as_file_mut().flush()?;
        let baseline_hash = hash_file(stage.path())?;
        let external = audit_region(stage.as_file_mut(), &region_dir, region_x, region_z)?;

        let original_nbt = RegionReader::open_for_region(stage.path(), region_x, region_z)?
            .read_chunk_nbt_bytes(chunk_x, chunk_z)?
            .context("cannot rewrite a missing chunk")?;
        let chunk = nbt::read(&original_nbt)?;
        validate_chunk(&chunk.value, chunk_x, chunk_z)?;
        let encoded = match (expected_nbt, replacement_nbt) {
            (None, None) => {
                let encoded = nbt::write(&chunk)?;
                ensure!(
                    encoded == original_nbt,
                    "chunk NBT cannot be reserialized without changing its bytes"
                );
                encoded
            }
            (Some(expected), Some(replacement)) => {
                ensure!(
                    expected == original_nbt,
                    "chunk changed after its draft was created"
                );
                let draft = nbt::read(replacement).context("invalid chunk draft NBT")?;
                validate_chunk(&draft.value, chunk_x, chunk_z)?;
                ensure!(
                    nbt::write(&draft)? == replacement,
                    "chunk draft cannot be reserialized without changing its bytes"
                );
                replacement.to_vec()
            }
            _ => anyhow::bail!("chunk rewrite requires both source and replacement NBT"),
        };
        let backup_directory =
            backup_region(&self.root, dimension, &name, stage.path(), &external)?;
        let external_path = region_dir.join(format!("c.{chunk_x}.{chunk_z}.mcc"));
        let retain_external = external.iter().any(|path| *path == external_path);
        let compressed = write_staged_chunk(
            stage.as_file_mut(),
            chunk_x,
            chunk_z,
            &encoded,
            timestamp_seconds,
            retain_external,
        )?;
        let mut replacements = Vec::new();
        if let Some(compressed) = &compressed {
            let backup = external
                .iter()
                .find(|path| **path == external_path)
                .map(|path| {
                    backup_directory.join(path.file_name().expect("external chunk filename"))
                });
            replacements.push(stage_external_chunk(
                &region_dir,
                chunk_x,
                chunk_z,
                compressed,
                &encoded,
                backup,
            )?);
        }
        stage.as_file_mut().flush()?;
        stage
            .as_file()
            .sync_all()
            .context("cannot flush staged region")?;

        if compressed.is_some() {
            verify_external_marker(stage.as_file_mut(), chunk_x, chunk_z)?;
        } else {
            let verify = RegionReader::open_for_region(stage.path(), region_x, region_z)?
                .read_chunk(chunk_x, chunk_z)?
                .context("staged chunk disappeared")?;
            ensure!(
                nbt::write(&verify)? == encoded,
                "staged chunk failed the NBT round-trip check"
            );
        }
        drop(original);
        let backup = backup_directory.join(&name);
        let dependencies = external
            .iter()
            .filter(|path| compressed.is_none() || **path != external_path)
            .map(|path| {
                let name = path.file_name().context("external chunk has no filename")?;
                Ok(crate::save_transaction::Dependency::Present {
                    target: path.clone(),
                    backup: backup_directory.join(name),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        replacements.push(crate::save_transaction::Replacement {
            target: canonical.clone(),
            backup: Some(backup),
            stage,
            original_hash: Some(baseline_hash),
        });
        crate::save_transaction::commit(&self.root, &backup_directory, replacements, dependencies)
            .with_context(|| {
                format!(
                    "cannot atomically replace region; backup retained at {}",
                    backup_directory.display()
                )
            })?;
        Ok(RewriteResult {
            region_path: canonical,
            backup_directory,
        })
    }
}

fn write_staged_chunk(
    file: &mut File,
    chunk_x: i32,
    chunk_z: i32,
    encoded: &[u8],
    timestamp_seconds: Option<u32>,
    retain_external: bool,
) -> Result<Option<Vec<u8>>> {
    let mut compressor = ZlibEncoder::new(Vec::new(), Compression::default());
    compressor.write_all(encoded)?;
    let compressed = compressor.finish()?;
    let length = compressed
        .len()
        .checked_add(1)
        .context("chunk payload length overflow")?;
    ensure!(
        length <= u32::MAX as usize,
        "chunk payload exceeds 32-bit length"
    );
    let inline_allocation = length
        .checked_add(4)
        .context("chunk allocation overflow")?
        .div_ceil(SECTOR_SIZE as usize);
    let external = retain_external || inline_allocation > 255;
    if external {
        ensure!(
            compressed.len() <= nbt::MAX_NBT_BYTES,
            "compressed external chunk exceeds size limit"
        );
    }
    let allocation = if external { 1 } else { inline_allocation };

    let index = chunk_x.rem_euclid(REGION_SIDE) as u64
        + chunk_z.rem_euclid(REGION_SIDE) as u64 * REGION_SIDE as u64;
    file.seek(SeekFrom::Start(index * 4))?;
    let mut previous_location = [0u8; 4];
    file.read_exact(&mut previous_location)?;
    let previous_location = u32::from_be_bytes(previous_location);
    let previous_offset = u64::from(previous_location >> 8);
    let previous_sectors = u64::from(previous_location & 255);
    ensure!(
        previous_location == 0 || (previous_offset >= 2 && previous_sectors > 0),
        "invalid chunk location"
    );
    let reuse = previous_location != 0 && allocation as u64 <= previous_sectors;
    let offset = if reuse {
        previous_offset
    } else {
        file.metadata()?.len() / SECTOR_SIZE
    };
    let sectors = if reuse && !external {
        previous_sectors
    } else {
        allocation as u64
    };
    ensure!(
        offset <= MAX_OFFSET && offset + sectors <= MAX_OFFSET + 1,
        "region has no addressable space for the rewritten chunk"
    );
    file.seek(SeekFrom::Start(offset * SECTOR_SIZE))?;
    file.write_all(&(if external { 1u32 } else { length as u32 }).to_be_bytes())?;
    file.write_all(&[if external { 0x82 } else { 2 }])?;
    if !external {
        file.write_all(&compressed)?;
    }
    if !reuse {
        let padding = allocation * SECTOR_SIZE as usize - if external { 5 } else { length + 4 };
        file.write_all(&vec![0; padding])?;
    }
    file.seek(SeekFrom::Start(index * 4))?;
    let location = ((offset as u32) << 8) | sectors as u32;
    file.write_all(&location.to_be_bytes())?;
    if let Some(timestamp_seconds) = timestamp_seconds {
        file.seek(SeekFrom::Start(SECTOR_SIZE + index * 4))?;
        file.write_all(&timestamp_seconds.to_be_bytes())?;
    }
    Ok(external.then_some(compressed))
}

fn verify_external_marker(file: &mut File, x: i32, z: i32) -> Result<()> {
    let index =
        x.rem_euclid(REGION_SIDE) as u64 + z.rem_euclid(REGION_SIDE) as u64 * REGION_SIDE as u64;
    file.seek(SeekFrom::Start(index * 4))?;
    let mut location = [0u8; 4];
    file.read_exact(&mut location)?;
    let location = u32::from_be_bytes(location);
    ensure!(
        location >> 8 >= 2 && location & 255 == 1,
        "invalid staged external chunk allocation"
    );
    file.seek(SeekFrom::Start(u64::from(location >> 8) * SECTOR_SIZE))?;
    let mut record = [0u8; 5];
    file.read_exact(&mut record)?;
    ensure!(
        record == [0, 0, 0, 1, 0x82],
        "invalid staged external chunk marker"
    );
    Ok(())
}

fn stage_external_chunk(
    region_dir: &Path,
    x: i32,
    z: i32,
    compressed: &[u8],
    expected: &[u8],
    backup: Option<PathBuf>,
) -> Result<crate::save_transaction::Replacement> {
    let target = region_dir.join(format!("c.{x}.{z}.mcc"));
    let original_hash = if let Some(backup) = &backup {
        let meta =
            fs::symlink_metadata(&target).context("external chunk disappeared during staging")?;
        ensure!(
            meta.is_file() && ordinary(&meta),
            "external chunk must be an ordinary file"
        );
        ensure!(
            target.canonicalize()?.parent() == Some(region_dir),
            "external chunk resolves outside its region directory"
        );
        Some(hash_file(backup)?)
    } else {
        match fs::symlink_metadata(&target) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => anyhow::bail!("unreferenced external chunk already exists"),
            Err(error) => return Err(error.into()),
        }
        None
    };
    let mut stage = tempfile::Builder::new()
        .prefix(".native-stage-")
        .tempfile_in(region_dir)?;
    stage.write_all(compressed)?;
    stage.flush()?;
    stage.as_file().sync_all()?;
    let decoded =
        nbt::decompress_limited(flate2::read::ZlibDecoder::new(File::open(stage.path())?))?;
    ensure!(
        decoded == expected,
        "staged external chunk failed the NBT round-trip check"
    );
    Ok(crate::save_transaction::Replacement {
        target,
        backup,
        stage,
        original_hash,
    })
}

fn clear_staged_chunk(file: &mut File, x: i32, z: i32, timestamp: u32) -> Result<()> {
    let index =
        x.rem_euclid(REGION_SIDE) as u64 + z.rem_euclid(REGION_SIDE) as u64 * REGION_SIDE as u64;
    file.seek(SeekFrom::Start(index * 4))?;
    file.write_all(&0u32.to_be_bytes())?;
    file.seek(SeekFrom::Start(SECTOR_SIZE + index * 4))?;
    file.write_all(&timestamp.to_be_bytes())?;
    Ok(())
}

pub(crate) fn validate_chunk(chunk: &Tag, x: i32, z: i32) -> Result<()> {
    let Tag::Compound(fields) = chunk else {
        anyhow::bail!("chunk NBT root is not a compound");
    };
    for name in ["DataVersion", "xPos", "zPos", "Status", "sections"] {
        ensure!(
            fields.iter().filter(|(key, _)| key.matches(name)).count() <= 1,
            "duplicate chunk {name} tags are not supported"
        );
    }
    ensure!(
        matches!(chunk.get("DataVersion"), Some(Tag::Int(value)) if *value == TARGET_DATA_VERSION),
        "chunk is not data version {TARGET_DATA_VERSION}"
    );
    ensure!(
        matches!(chunk.get("xPos"), Some(Tag::Int(value)) if *value == x),
        "chunk X coordinate does not match its region entry"
    );
    ensure!(
        matches!(chunk.get("zPos"), Some(Tag::Int(value)) if *value == z),
        "chunk Z coordinate does not match its region entry"
    );
    ensure!(
        matches!(chunk.get("Status"), Some(Tag::String(value)) if value.matches("minecraft:full")),
        "only full chunks can be rewritten"
    );
    palette::decode_sections(chunk).context("unsupported or invalid chunk section")?;
    palette::validate_26_3_block_states(chunk).context("unsupported 26.3 block-state format")?;
    Ok(())
}

fn audit_region(
    file: &mut File,
    directory: &Path,
    region_x: i32,
    region_z: i32,
) -> Result<Vec<PathBuf>> {
    let length = file.metadata()?.len();
    ensure!(
        length >= HEADER_SIZE && length.is_multiple_of(SECTOR_SIZE),
        "invalid or incomplete region file length"
    );
    ensure!(
        length / SECTOR_SIZE <= MAX_OFFSET + 1,
        "region is larger than the location table supports"
    );
    let mut header = [0u8; HEADER_SIZE as usize];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut header)?;
    let mut intervals = Vec::new();
    let mut external = Vec::new();
    for index in 0..1024usize {
        let offset = index * 4;
        let entry = u32::from_be_bytes(header[offset..offset + 4].try_into()?);
        if entry == 0 {
            continue;
        }
        let start = u64::from(entry >> 8);
        let count = u64::from(entry & 255);
        ensure!(
            start >= 2 && count > 0 && start + count <= length / SECTOR_SIZE,
            "invalid chunk sector allocation at entry {index}"
        );
        intervals.push((start, start + count));
        file.seek(SeekFrom::Start(start * SECTOR_SIZE))?;
        let mut record = [0u8; 5];
        file.read_exact(&mut record)?;
        let stored_length = u64::from(u32::from_be_bytes(record[..4].try_into()?));
        ensure!(
            stored_length >= 1 && stored_length <= count * SECTOR_SIZE - 4,
            "invalid chunk length at entry {index}"
        );
        let compression = record[4] & 0x7f;
        ensure!(
            (1..=4).contains(&compression),
            "unsupported chunk compression {compression} at entry {index}"
        );
        if record[4] & 0x80 != 0 {
            ensure!(
                stored_length == 1,
                "external chunk marker has inline data at entry {index}"
            );
            let x = region_x
                .checked_mul(REGION_SIDE)
                .and_then(|value| value.checked_add((index % 32) as i32))
                .context("external chunk X overflows")?;
            let z = region_z
                .checked_mul(REGION_SIDE)
                .and_then(|value| value.checked_add((index / 32) as i32))
                .context("external chunk Z overflows")?;
            let path = directory.join(format!("c.{x}.{z}.mcc"));
            let metadata = fs::symlink_metadata(&path)
                .with_context(|| format!("external chunk is missing: {}", path.display()))?;
            ensure!(
                metadata.is_file() && ordinary(&metadata),
                "external chunk is not an ordinary file"
            );
            ensure!(
                metadata.len() <= nbt::MAX_NBT_BYTES as u64,
                "external chunk exceeds the supported size limit"
            );
            ensure!(
                path.canonicalize()?.parent() == Some(directory),
                "external chunk resolves outside its region directory"
            );
            external.push(path);
        }
    }
    intervals.sort_unstable();
    ensure!(
        intervals.windows(2).all(|pair| pair[0].1 <= pair[1].0),
        "overlapping region chunk allocations"
    );
    Ok(external)
}

fn backup_region(
    root: &Path,
    dimension: &DimensionId,
    name: &str,
    original: &Path,
    external: &[PathBuf],
) -> Result<PathBuf> {
    let backups = root.join(".minecraft-native-backups");
    match fs::symlink_metadata(&backups) {
        Ok(meta) => ensure!(
            meta.is_dir() && ordinary(&meta),
            "backup directory must be an ordinary directory"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&backups)?,
        Err(error) => return Err(error.into()),
    }
    ensure!(
        backups.canonicalize()?.starts_with(root),
        "backup directory is outside the world"
    );
    let dir = tempfile::Builder::new()
        .prefix("region-")
        .tempdir_in(&backups)?;
    copy_sync(original, &dir.path().join(name))?;
    for path in external {
        let file_name = path.file_name().context("external chunk has no filename")?;
        copy_sync(path, &dir.path().join(file_name))?;
    }
    let mut manifest = File::create(dir.path().join("backup.txt"))?;
    writeln!(
        manifest,
        "Minecraft Native region backup\ndimension={}:{}\nregion={name}\nexternal_chunks={}",
        dimension.namespace,
        dimension.path,
        external.len()
    )?;
    manifest.sync_all()?;
    Ok(dir.keep())
}

fn copy_sync(source: &Path, destination: &Path) -> Result<()> {
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    std::io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    Ok(())
}

pub(crate) fn hash_file(path: &Path) -> Result<[u8; 32]> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher.finalize().into())
}

#[cfg(windows)]
struct WorldLock(File);

#[cfg(windows)]
impl WorldLock {
    fn acquire(root: &Path) -> Result<Self> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Foundation::{ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION},
            Storage::FileSystem::{LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx},
            System::IO::OVERLAPPED,
        };
        let lock_error = |error: std::io::Error| {
            if matches!(error.raw_os_error(), Some(value) if value == ERROR_LOCK_VIOLATION as i32 || value == ERROR_SHARING_VIOLATION as i32)
            {
                anyhow::Error::new(WorldInUse).context(error)
            } else {
                anyhow::Error::new(error).context("session.lock cannot be locked")
            }
        };
        let path = root.join("session.lock");
        if let Ok(meta) = fs::symlink_metadata(&path) {
            ensure!(
                meta.is_file() && !meta.file_type().is_symlink(),
                "session.lock must be an ordinary file"
            );
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(lock_error)?;
        let mut overlap: OVERLAPPED = unsafe { std::mem::zeroed() };
        // Java FileChannel.tryLock() locks [0, Long.MAX_VALUE) on this file.
        let ok = unsafe {
            LockFileEx(
                file.as_raw_handle() as _,
                LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                0,
                u32::MAX,
                0x7fff_ffff,
                &mut overlap,
            )
        };
        if ok == 0 {
            return Err(lock_error(std::io::Error::last_os_error()));
        }
        Ok(Self(file))
    }
}

#[cfg(windows)]
impl Drop for WorldLock {
    fn drop(&mut self) {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{Storage::FileSystem::UnlockFileEx, System::IO::OVERLAPPED};
        let mut overlap: OVERLAPPED = unsafe { std::mem::zeroed() };
        unsafe {
            UnlockFileEx(
                self.0.as_raw_handle() as _,
                0,
                u32::MAX,
                0x7fff_ffff,
                &mut overlap,
            );
        }
    }
}

#[cfg(not(windows))]
struct WorldLock;

#[cfg(not(windows))]
impl WorldLock {
    fn acquire(_root: &Path) -> Result<Self> {
        anyhow::bail!("world writing is currently supported only on Windows")
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::io::Write;

    use super::*;
    use crate::{
        edit::{BlockChange, draft_chunk_blocks},
        heightmap::{BlockHeightFlags, HeightBounds, SpawnHeightmapKind, SpawnHeightmaps},
        nbt::{NamedTag, TagType},
        palette::{self, BlockState},
        world::ChunkPos,
    };
    use flate2::write::{GzEncoder, ZlibEncoder};

    fn test_world() -> tempfile::TempDir {
        // Windows sandboxes may forbid replacement in the system temporary folder.
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/region-test-temp");
        fs::create_dir_all(&root).unwrap();
        tempfile::Builder::new()
            .prefix("world-")
            .tempdir_in(root)
            .unwrap()
    }

    #[test]
    fn level_time_plan_preserves_data_and_rotates_previous_level() {
        for old_exists in [false, true] {
            let world = test_world();
            fixture(world.path());
            let path = world.path().join("level.dat");
            let (_, initial) = anvil::read_level_document(world.path()).unwrap();
            let mut source =
                crate::world::WorldReader::update_level_game_time(&initial, 17).unwrap();
            let Tag::Compound(root) = &mut source.value else {
                unreachable!()
            };
            root.push(("future_root".into(), Tag::Long(42)));
            let Tag::Compound(data) = &mut root
                .iter_mut()
                .find(|(key, _)| key.matches("Data"))
                .unwrap()
                .1
            else {
                unreachable!()
            };
            data.push(("future_data".into(), Tag::String("keep".into())));
            let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
            gzip.write_all(&nbt::write(&source).unwrap()).unwrap();
            let original = gzip.finish().unwrap();
            fs::write(&path, &original).unwrap();
            let old = world.path().join("level.dat_old");
            if old_exists {
                fs::write(&old, b"older backup").unwrap();
            }
            let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
            let mut plan = WorldSavePlan::new();
            plan.add_level_time(&source, i64::MIN + 10).unwrap();
            assert!(plan.has_level_time());
            assert!(plan.add_level_time(&source, 1).is_err());
            let result = guard.write_plan(plan, |_, _| Ok(())).unwrap();
            assert_eq!(result.level_path, Some(path.canonicalize().unwrap()));
            let saved = nbt::read_gzip(File::open(&path).unwrap()).unwrap();
            assert_eq!(
                crate::world::WorldReader::validate_level_game_time(&saved).unwrap(),
                i64::MIN + 10
            );
            assert_eq!(saved.value.get("future_root"), Some(&Tag::Long(42)));
            assert_eq!(
                saved.value.get("Data").unwrap().get("future_data"),
                Some(&Tag::String("keep".into()))
            );
            assert_eq!(fs::read(&old).unwrap(), original);
            assert_eq!(
                fs::read(result.backup_directory.join("world-data/level.dat")).unwrap(),
                original
            );
            if old_exists {
                assert_eq!(
                    fs::read(result.backup_directory.join("world-data/level.dat_old")).unwrap(),
                    b"older backup"
                );
            }
            let after = fs::read(&path).unwrap();
            let mut stale = WorldSavePlan::new();
            stale.add_level_time(&source, 99).unwrap();
            assert!(guard.write_plan(stale, |_, _| Ok(())).is_err());
            assert_eq!(fs::read(&path).unwrap(), after);
            assert_eq!(fs::read(&old).unwrap(), original);
        }
    }

    #[test]
    fn random_sequences_are_saved_with_backup_and_stale_files_abort_the_transaction() {
        use crate::random_sequences::RandomSequences;
        use crate::world::SavedWeather;
        for compressed in [false, true] {
            let world = test_world();
            fixture(world.path());
            let path = world.path().join("data/minecraft/random_sequences.dat");
            let weather_path = path.parent().unwrap().join("weather.dat");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let mut source = RandomSequences::read(None).unwrap().draft().unwrap();
            let Tag::Compound(root) = &mut source.value else {
                unreachable!()
            };
            root.push(("future".into(), Tag::Int(42)));
            let bytes = nbt::write(&source).unwrap();
            let original = if compressed {
                let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
                gzip.write_all(&bytes).unwrap();
                gzip.finish().unwrap()
            } else {
                bytes
            };
            fs::write(&path, &original).unwrap();
            let weather =
                crate::world::WorldReader::update_weather_document(None, SavedWeather::default())
                    .unwrap();
            let weather_bytes = nbt::write(&weather).unwrap();
            fs::write(&weather_path, &weather_bytes).unwrap();
            let mut sequences = RandomSequences::read(Some(source.clone())).unwrap();
            let mut random = sequences
                .get("minecraft:chests/buried_treasure", 99)
                .unwrap();
            random.next_long();
            sequences
                .set("minecraft:chests/buried_treasure", &random)
                .unwrap();
            let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
            let mut plan = WorldSavePlan::new();
            plan.add_random_sequences(&sequences).unwrap();
            assert!(plan.has_random_sequences());
            assert!(plan.add_random_sequences(&sequences).is_err());
            plan.add_weather(
                Some(&weather),
                SavedWeather {
                    raining: true,
                    ..Default::default()
                },
            )
            .unwrap();
            let mut changed = source.clone();
            let Tag::Compound(root) = &mut changed.value else {
                unreachable!()
            };
            root.push(("other".into(), Tag::Int(1)));
            let stale_bytes = nbt::write(&changed).unwrap();
            fs::write(&path, &stale_bytes).unwrap();
            assert!(guard.write_plan(plan, |_, _| Ok(())).is_err());
            assert_eq!(fs::read(&path).unwrap(), stale_bytes);
            assert_eq!(fs::read(&weather_path).unwrap(), weather_bytes);
            fs::write(&path, &original).unwrap();
            let mut plan = WorldSavePlan::new();
            plan.add_random_sequences(&sequences).unwrap();
            plan.add_weather(
                Some(&weather),
                SavedWeather {
                    raining: true,
                    ..Default::default()
                },
            )
            .unwrap();
            let result = guard.write_plan(plan, |_, _| Ok(())).unwrap();
            assert_eq!(
                result.random_sequences_path.as_deref(),
                Some(path.canonicalize().unwrap().as_path())
            );
            let output = fs::read(&path).unwrap();
            assert_eq!(output.starts_with(&[0x1f, 0x8b]), compressed);
            let document = if compressed {
                nbt::read_gzip(output.as_slice()).unwrap()
            } else {
                nbt::read(&output).unwrap()
            };
            assert_eq!(document.value.get("future"), Some(&Tag::Int(42)));
            let restored = RandomSequences::read(Some(document)).unwrap();
            assert_eq!(
                restored
                    .get("minecraft:chests/buried_treasure", 99)
                    .unwrap()
                    .state(),
                random.state()
            );
            assert_eq!(
                fs::read(
                    result
                        .backup_directory
                        .join("world-data/random_sequences.dat")
                )
                .unwrap(),
                original
            );
            assert_eq!(
                crate::world::parse_weather_data(
                    &nbt::read(&fs::read(&weather_path).unwrap()).unwrap()
                )
                .unwrap()
                .raining,
                true
            );
        }
        let world = test_world();
        let (region_path, mut region, _) = fixture(world.path());
        let terrain = editable_chunk(0);
        store_record(&mut region, 0, 2, &terrain, 10);
        fs::write(&region_path, &region).unwrap();
        let path = world.path().join("data/minecraft/random_sequences.dat");
        let mut sequences = RandomSequences::read(None).unwrap();
        let mut random = sequences.get("minecraft:test", 1).unwrap();
        random.next_long();
        sequences.set("minecraft:test", &random).unwrap();
        let mut plan = WorldSavePlan::new();
        plan.add_random_sequences(&sequences).unwrap();
        plan.add_chunk(
            DimensionId::parse("overworld").unwrap(),
            block_draft(
                &terrain,
                ChunkPos { x: 0, z: 0 },
                "minecraft:stone",
                "minecraft:dirt",
            ),
            11,
        )
        .unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        let result = guard.write_plan(plan, |_, _| Ok(())).unwrap();
        assert_ne!(fs::read(&region_path).unwrap(), region);
        assert_eq!(
            result.region_paths,
            vec![region_path.canonicalize().unwrap()]
        );
        assert_eq!(
            result.random_sequences_path.as_deref(),
            Some(path.canonicalize().unwrap().as_path())
        );
        let data = fs::read(&path).unwrap();
        assert!(data.starts_with(&[0x1f, 0x8b]));
        let restored =
            RandomSequences::read(Some(nbt::read_gzip(data.as_slice()).unwrap())).unwrap();
        assert_eq!(
            restored.get("minecraft:test", 1).unwrap().state(),
            random.state()
        );
    }

    #[test]
    fn weather_plan_preserves_unknown_data_and_detects_stale_or_missing_sources() {
        use crate::world::SavedWeather;
        let weather = SavedWeather {
            clear_weather_time: 37,
            rain_time: 239,
            thunder_time: 919,
            raining: true,
            thundering: false,
        };
        for compressed in [false, true] {
            let world = test_world();
            fixture(world.path());
            let path = world.path().join("data/minecraft/weather.dat");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let mut source =
                crate::world::WorldReader::update_weather_document(None, SavedWeather::default())
                    .unwrap();
            let Tag::Compound(root) = &mut source.value else {
                unreachable!()
            };
            root.push(("future_root".into(), Tag::Long(42)));
            let Tag::Compound(data) = &mut root
                .iter_mut()
                .find(|(key, _)| key.matches("data"))
                .unwrap()
                .1
            else {
                unreachable!()
            };
            data.push(("future_data".into(), Tag::String("keep".into())));
            let raw = nbt::write(&source).unwrap();
            let original = if compressed {
                let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
                gzip.write_all(&raw).unwrap();
                gzip.finish().unwrap()
            } else {
                raw
            };
            fs::write(&path, &original).unwrap();
            let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
            let mut stale = WorldSavePlan::new();
            stale.add_weather(None, weather).unwrap();
            assert!(guard.write_plan(stale, |_, _| Ok(())).is_err());
            assert_eq!(fs::read(&path).unwrap(), original);
            let mut plan = WorldSavePlan::new();
            plan.add_weather(Some(&source), weather).unwrap();
            assert!(plan.add_weather(Some(&source), weather).is_err());
            let result = guard.write_plan(plan, |_, _| Ok(())).unwrap();
            assert_eq!(
                result.weather_path.as_deref(),
                Some(path.canonicalize().unwrap().as_path())
            );
            let saved_bytes = fs::read(&path).unwrap();
            assert_eq!(saved_bytes.starts_with(&[0x1f, 0x8b]), compressed);
            let saved = if compressed {
                nbt::read_gzip(saved_bytes.as_slice()).unwrap()
            } else {
                nbt::read(&saved_bytes).unwrap()
            };
            assert_eq!(crate::world::parse_weather_data(&saved).unwrap(), weather);
            assert_eq!(saved.value.get("future_root"), Some(&Tag::Long(42)));
            assert_eq!(
                saved.value.get("data").unwrap().get("future_data"),
                Some(&Tag::String("keep".into()))
            );
            assert_eq!(
                fs::read(result.backup_directory.join("world-data/weather.dat")).unwrap(),
                original
            );
            let mut old = WorldSavePlan::new();
            old.add_weather(Some(&source), weather).unwrap();
            assert!(guard.write_plan(old, |_, _| Ok(())).is_err());
            assert_eq!(fs::read(&path).unwrap(), saved_bytes);
            let clock_path = path.parent().unwrap().join("world_clocks.dat");
            let clock = NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![
                    ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                    ("data".into(), Tag::Compound(Vec::new())),
                ]),
            };
            let clock_bytes = nbt::write(&clock).unwrap();
            fs::write(&clock_path, &clock_bytes).unwrap();
            let mut stale_multi = WorldSavePlan::new();
            stale_multi
                .add_world_clocks(
                    &clock,
                    &BTreeMap::from([(
                        DimensionId::parse("minecraft:overworld").unwrap(),
                        crate::world::SavedClock::default(),
                    )]),
                )
                .unwrap();
            stale_multi.add_weather(Some(&source), weather).unwrap();
            assert!(guard.write_plan(stale_multi, |_, _| Ok(())).is_err());
            assert_eq!(fs::read(&clock_path).unwrap(), clock_bytes);
            assert_eq!(fs::read(&path).unwrap(), saved_bytes);
        }
        let world = test_world();
        fixture(world.path());
        let path = world.path().join("data/minecraft/weather.dat");
        assert!(!path.exists());
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        let mut plan = WorldSavePlan::new();
        plan.add_weather(None, weather).unwrap();
        let result = guard.write_plan(plan, |_, _| Ok(())).unwrap();
        assert_eq!(
            result.weather_path.as_deref(),
            Some(path.canonicalize().unwrap().as_path())
        );
        let bytes = fs::read(&path).unwrap();
        assert!(bytes.starts_with(&[0x1f, 0x8b]));
        assert_eq!(
            crate::world::parse_weather_data(&nbt::read_gzip(bytes.as_slice()).unwrap()).unwrap(),
            weather
        );
        let mut duplicate = WorldSavePlan::new();
        duplicate.add_weather(None, weather).unwrap();
        assert!(guard.write_plan(duplicate, |_, _| Ok(())).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }

    fn chunk(x: i32, marker: &str) -> NamedTag {
        NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                ("xPos".into(), Tag::Int(x)),
                ("zPos".into(), Tag::Int(0)),
                ("Status".into(), Tag::String("minecraft:full".into())),
                (
                    "sections".into(),
                    Tag::List {
                        element_type: TagType::End,
                        values: vec![],
                    },
                ),
                (
                    "unknown".into(),
                    Tag::Compound(vec![("value".into(), Tag::String(marker.into()))]),
                ),
            ]),
        }
    }

    fn editable_chunk(x: i32) -> NamedTag {
        let mut chunk = chunk(x, "target");
        let Tag::Compound(fields) = &mut chunk.value else {
            unreachable!()
        };
        let (_, sections) = fields
            .iter_mut()
            .find(|(name, _)| name.matches("sections"))
            .unwrap();
        *sections = Tag::List {
            element_type: TagType::Compound,
            values: vec![Tag::Compound(vec![
                ("Y".into(), Tag::Byte(0)),
                (
                    "block_states".into(),
                    Tag::Compound(vec![(
                        "palette".into(),
                        Tag::List {
                            element_type: TagType::String,
                            values: vec![Tag::String("minecraft:stone".into())],
                        },
                    )]),
                ),
                ("unknown_section".into(), Tag::String("keep".into())),
            ])],
        };
        chunk
    }

    fn block_state(name: &str) -> BlockState {
        BlockState {
            name: name.into(),
            properties: Vec::new(),
        }
    }

    #[test]
    fn ambiguous_chunk_fields_cannot_be_rewritten() {
        let original = chunk(0, "preserved");
        assert!(validate_chunk(&original.value, 0, 0).is_ok());
        for name in ["DataVersion", "xPos", "zPos", "Status", "sections"] {
            let mut ambiguous = original.clone();
            let Tag::Compound(fields) = &mut ambiguous.value else {
                unreachable!()
            };
            let duplicate = fields
                .iter()
                .find(|(key, _)| key.matches(name))
                .unwrap()
                .clone();
            fields.push(duplicate);
            assert!(validate_chunk(&ambiguous.value, 0, 0).is_err(), "{name}");
        }
    }

    fn compressed(root: &NamedTag) -> Vec<u8> {
        let mut output = ZlibEncoder::new(Vec::new(), Compression::default());
        output.write_all(&nbt::write(root).unwrap()).unwrap();
        output.finish().unwrap()
    }

    fn store_record(
        region: &mut [u8],
        slot: usize,
        sector: usize,
        root: &NamedTag,
        timestamp: u32,
    ) {
        let bytes = compressed(root);
        assert!(bytes.len() + 5 <= SECTOR_SIZE as usize);
        region[slot * 4..slot * 4 + 4].copy_from_slice(&(((sector as u32) << 8) | 1).to_be_bytes());
        region[SECTOR_SIZE as usize + slot * 4..SECTOR_SIZE as usize + slot * 4 + 4]
            .copy_from_slice(&timestamp.to_be_bytes());
        let start = sector * SECTOR_SIZE as usize;
        region[start..start + 4].copy_from_slice(&((bytes.len() + 1) as u32).to_be_bytes());
        region[start + 4] = 2;
        region[start + 5..start + 5 + bytes.len()].copy_from_slice(&bytes);
    }

    fn entity_chunk(x: i32, entities: Vec<Tag>) -> NamedTag {
        NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                ("Position".into(), Tag::IntArray(vec![x, 0])),
                (
                    "Entities".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: entities,
                    },
                ),
            ]),
        }
    }

    fn incompressible_bytes(length: usize) -> Vec<u8> {
        let mut seed = 0x913a12d5u32;
        (0..length)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as u8
            })
            .collect()
    }

    fn large_entity_chunk() -> NamedTag {
        entity_chunk(
            0,
            vec![Tag::Compound(vec![
                ("id".into(), Tag::String("minecraft:item".into())),
                ("UUID".into(), Tag::IntArray(vec![1, 2, 3, 4])),
                (
                    "payload".into(),
                    Tag::ByteArray(incompressible_bytes(1_060_000)),
                ),
            ])],
        )
    }

    #[test]
    fn coordinated_external_chunks_create_replace_and_detect_stale_sources() {
        let world = test_world();
        let (terrain_path, terrain, _) = fixture(world.path());
        let dir = world.path().join("dimensions/minecraft/overworld/entities");
        let region = dir.join("r.0.0.mca");
        let external = dir.join("c.0.0.mcc");
        let dimension = DimensionId::parse("minecraft:overworld").unwrap();
        let position = crate::world::ChunkPos { x: 0, z: 0 };
        let source = large_entity_chunk();
        let mut plan = WorldSavePlan::new();
        plan.add_entity_chunk(dimension.clone(), position, None, &source, 123, |_, _| {
            Ok(())
        })
        .unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        guard.write_plan(plan, |_, _| Ok(())).unwrap();
        assert_eq!(fs::read(&terrain_path).unwrap(), terrain);
        let first_region = fs::read(&region).unwrap();
        let first_external = fs::read(&external).unwrap();
        assert!(first_external.len() > 255 * SECTOR_SIZE as usize - 5);
        assert_eq!(
            &first_region[2 * SECTOR_SIZE as usize..2 * SECTOR_SIZE as usize + 5],
            &[0, 0, 0, 1, 0x82]
        );
        assert_eq!(
            nbt::write(
                &RegionReader::open(&region)
                    .unwrap()
                    .read_chunk(0, 0)
                    .unwrap()
                    .unwrap()
            )
            .unwrap(),
            nbt::write(&source).unwrap()
        );
        let mut replacement = source.clone();
        let Tag::Compound(fields) = &mut replacement.value else {
            unreachable!()
        };
        let Tag::List { values, .. } = &mut fields
            .iter_mut()
            .find(|(name, _)| name.matches("Entities"))
            .unwrap()
            .1
        else {
            unreachable!()
        };
        let Tag::Compound(entity) = &mut values[0] else {
            unreachable!()
        };
        let Tag::ByteArray(bytes) = &mut entity
            .iter_mut()
            .find(|(name, _)| name.matches("payload"))
            .unwrap()
            .1
        else {
            unreachable!()
        };
        bytes[0] ^= 1;
        let make_plan = || {
            let mut plan = WorldSavePlan::new();
            plan.add_entity_chunk(
                dimension.clone(),
                position,
                Some(&source),
                &replacement,
                124,
                |_, _| Ok(()),
            )
            .unwrap();
            plan
        };
        let mut changed_source = source.clone();
        let Tag::Compound(root) = &mut changed_source.value else {
            unreachable!()
        };
        root.push(("outside".into(), Tag::Int(9)));
        let outside = compressed(&changed_source);
        fs::write(&external, &outside).unwrap();
        assert!(guard.write_plan(make_plan(), |_, _| Ok(())).is_err());
        assert_eq!(fs::read(&region).unwrap(), first_region);
        assert_eq!(fs::read(&external).unwrap(), outside);
        fs::write(&external, &first_external).unwrap();
        let result = guard.write_plan(make_plan(), |_, _| Ok(())).unwrap();
        assert_eq!(
            fs::read(result.backup_directory.join("region-0/c.0.0.mcc")).unwrap(),
            first_external
        );
        assert_eq!(
            fs::read(result.backup_directory.join("region-0/r.0.0.mca")).unwrap(),
            first_region
        );
        assert_eq!(
            nbt::write(
                &RegionReader::open(&region)
                    .unwrap()
                    .read_chunk(0, 0)
                    .unwrap()
                    .unwrap()
            )
            .unwrap(),
            nbt::write(&replacement).unwrap()
        );
        let previous_external = fs::read(&external).unwrap();
        let small = entity_chunk(
            0,
            vec![Tag::Compound(vec![
                ("id".into(), Tag::String("minecraft:item".into())),
                ("UUID".into(), Tag::IntArray(vec![1, 2, 3, 4])),
            ])],
        );
        let mut plan = WorldSavePlan::new();
        plan.add_entity_chunk(
            dimension,
            position,
            Some(&replacement),
            &small,
            125,
            |_, _| Ok(()),
        )
        .unwrap();
        let result = guard.write_plan(plan, |_, _| Ok(())).unwrap();
        assert_eq!(
            fs::read(result.backup_directory.join("region-0/c.0.0.mcc")).unwrap(),
            previous_external
        );
        assert!(fs::metadata(&external).unwrap().len() < SECTOR_SIZE);
        let region_bytes = fs::read(&region).unwrap();
        assert_eq!(
            &region_bytes[2 * SECTOR_SIZE as usize..2 * SECTOR_SIZE as usize + 5],
            &[0, 0, 0, 1, 0x82]
        );
        assert_eq!(
            nbt::write(
                &RegionReader::open(&region)
                    .unwrap()
                    .read_chunk(0, 0)
                    .unwrap()
                    .unwrap()
            )
            .unwrap(),
            nbt::write(&small).unwrap()
        );
        assert!(!world.path().join(".minecraft-native-pending").exists());
    }

    #[test]
    fn unreferenced_external_file_is_never_overwritten() {
        let world = test_world();
        fixture(world.path());
        let dir = world.path().join("dimensions/minecraft/overworld/entities");
        fs::create_dir(&dir).unwrap();
        let external = dir.join("c.0.0.mcc");
        fs::write(&external, b"unrelated external data").unwrap();
        let mut plan = WorldSavePlan::new();
        plan.add_entity_chunk(
            DimensionId::parse("overworld").unwrap(),
            crate::world::ChunkPos { x: 0, z: 0 },
            None,
            &large_entity_chunk(),
            1,
            |_, _| Ok(()),
        )
        .unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        assert!(guard.write_plan(plan, |_, _| Ok(())).is_err());
        assert_eq!(fs::read(external).unwrap(), b"unrelated external data");
        assert!(!dir.join("r.0.0.mca").exists());
        assert!(!world.path().join(".minecraft-native-pending").exists());
    }

    #[test]
    fn one_region_can_stage_external_and_inline_chunks_together() {
        let world = test_world();
        fixture(world.path());
        let large = large_entity_chunk();
        let small = entity_chunk(
            1,
            vec![Tag::Compound(vec![(
                "id".into(),
                Tag::String("minecraft:pig".into()),
            )])],
        );
        let dimension = DimensionId::parse("overworld").unwrap();
        let mut plan = WorldSavePlan::new();
        plan.add_entity_chunk(
            dimension.clone(),
            crate::world::ChunkPos { x: 0, z: 0 },
            None,
            &large,
            123,
            |_, _| Ok(()),
        )
        .unwrap();
        plan.add_entity_chunk(
            dimension,
            crate::world::ChunkPos { x: 1, z: 0 },
            None,
            &small,
            124,
            |_, _| Ok(()),
        )
        .unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        guard.write_plan(plan, |_, _| Ok(())).unwrap();
        let directory = world.path().join("dimensions/minecraft/overworld/entities");
        let region = directory.join("r.0.0.mca");
        assert!(directory.join("c.0.0.mcc").is_file());
        assert!(!directory.join("c.1.0.mcc").exists());
        let mut reader = RegionReader::open(&region).unwrap();
        assert_eq!(
            nbt::write(&reader.read_chunk(0, 0).unwrap().unwrap()).unwrap(),
            nbt::write(&large).unwrap()
        );
        assert_eq!(
            nbt::write(&reader.read_chunk(1, 0).unwrap().unwrap()).unwrap(),
            nbt::write(&small).unwrap()
        );
        let bytes = fs::read(region).unwrap();
        assert_eq!(
            &bytes[SECTOR_SIZE as usize..SECTOR_SIZE as usize + 8],
            &[0, 0, 0, 123, 0, 0, 0, 124]
        );
    }

    #[test]
    fn lossless_rewrite_moves_oversized_chunk_to_external_and_replaces_it_safely() {
        let world = test_world();
        let (region, _, _) = fixture(world.path());
        let mut source = chunk(0, "large");
        let Tag::Compound(fields) = &mut source.value else {
            unreachable!()
        };
        fields.push(("payload".into(), Tag::ByteArray(Vec::new())));
        let target_size = 255 * SECTOR_SIZE as usize - 5;
        let filler = target_size - nbt::write(&source).unwrap().len();
        let Tag::Compound(fields) = &mut source.value else {
            unreachable!()
        };
        fields.last_mut().unwrap().1 = Tag::ByteArray(incompressible_bytes(filler));
        let encoded = nbt::write(&source).unwrap();
        assert_eq!(encoded.len(), target_size);
        assert!(compressed(&source).len() + 5 > 255 * SECTOR_SIZE as usize);
        let mut original = vec![0u8; 257 * SECTOR_SIZE as usize];
        original[..4].copy_from_slice(&((2u32 << 8) | 255).to_be_bytes());
        original[SECTOR_SIZE as usize..SECTOR_SIZE as usize + 4]
            .copy_from_slice(&12345u32.to_be_bytes());
        let start = 2 * SECTOR_SIZE as usize;
        original[start..start + 4].copy_from_slice(&((encoded.len() as u32 + 1).to_be_bytes()));
        original[start + 4] = 3;
        original[start + 5..start + 5 + encoded.len()].copy_from_slice(&encoded);
        fs::write(&region, &original).unwrap();
        let external = region.parent().unwrap().join("c.0.0.mcc");
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        let result = guard
            .rewrite_chunk_losslessly(&DimensionId::parse("overworld").unwrap(), 0, 0)
            .unwrap();
        assert_eq!(
            fs::read(result.backup_directory.join("r.0.0.mca")).unwrap(),
            original
        );
        let after = fs::read(&region).unwrap();
        assert_eq!(
            u32::from_be_bytes(after[..4].try_into().unwrap()),
            (2 << 8) | 1
        );
        assert_eq!(&after[start..start + 5], &[0, 0, 0, 1, 0x82]);
        assert_eq!(
            &after[SECTOR_SIZE as usize..SECTOR_SIZE as usize + 4],
            &12345u32.to_be_bytes()
        );
        assert_eq!(
            nbt::write(
                &RegionReader::open(&region)
                    .unwrap()
                    .read_chunk(0, 0)
                    .unwrap()
                    .unwrap()
            )
            .unwrap(),
            encoded
        );
        let first_external = fs::read(&external).unwrap();
        let again = guard
            .rewrite_chunk_losslessly(&DimensionId::parse("overworld").unwrap(), 0, 0)
            .unwrap();
        assert_eq!(
            fs::read(again.backup_directory.join("c.0.0.mcc")).unwrap(),
            first_external
        );
        assert_eq!(
            nbt::write(
                &RegionReader::open(&region)
                    .unwrap()
                    .read_chunk(0, 0)
                    .unwrap()
                    .unwrap()
            )
            .unwrap(),
            encoded
        );
    }

    #[test]
    fn coordinated_save_moves_entity_between_existing_and_absent_chunks() {
        let world = test_world();
        let (block_path, mut block_bytes, _) = fixture(world.path());
        let terrain = editable_chunk(0);
        store_record(&mut block_bytes, 0, 2, &terrain, 10);
        fs::write(&block_path, &block_bytes).unwrap();
        let directory = world.path().join("dimensions/minecraft/overworld/entities");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("r.0.0.mca");
        let entity = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:item".into())),
            ("UUID".into(), Tag::IntArray(vec![1, 2, 3, 4])),
            ("unrecognized".into(), Tag::Int(7)),
        ]);
        let source = entity_chunk(0, vec![entity.clone()]);
        let untouched = entity_chunk(
            2,
            vec![Tag::Compound(vec![(
                "id".into(),
                Tag::String("minecraft:pig".into()),
            )])],
        );
        let mut original = vec![0; 4 * SECTOR_SIZE as usize];
        store_record(&mut original, 0, 2, &source, 11);
        store_record(&mut original, 2, 3, &untouched, 12);
        fs::write(&path, &original).unwrap();
        let destination = entity_chunk(1, vec![entity]);
        let dimension = DimensionId::parse("overworld").unwrap();
        let mut plan = WorldSavePlan::new();
        plan.add_chunk(
            dimension.clone(),
            block_draft(
                &terrain,
                ChunkPos { x: 0, z: 0 },
                "minecraft:stone",
                "minecraft:dirt",
            ),
            100,
        )
        .unwrap();
        plan.add_entity_chunk(
            dimension.clone(),
            ChunkPos { x: 0, z: 0 },
            Some(&source),
            &entity_chunk(0, vec![]),
            101,
            |_, _| Ok(()),
        )
        .unwrap();
        plan.add_entity_chunk(
            dimension.clone(),
            ChunkPos { x: 1, z: 0 },
            None,
            &destination,
            102,
            |_, _| Ok(()),
        )
        .unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        let result = guard.write_plan(plan, |_, _| Ok(())).unwrap();
        assert_eq!(
            result.entity_region_paths,
            vec![path.canonicalize().unwrap()]
        );
        assert_eq!(
            fs::read(result.backup_directory.join("region-1/r.0.0.mca")).unwrap(),
            original
        );
        let mut reader = RegionReader::open_for_region(&path, 0, 0).unwrap();
        assert!(reader.read_chunk(0, 0).unwrap().is_none());
        assert_eq!(reader.read_chunk(1, 0).unwrap().unwrap(), destination);
        assert_eq!(reader.read_chunk(2, 0).unwrap().unwrap(), untouched);
        let saved = fs::read(&path).unwrap();
        assert_eq!(
            u32::from_be_bytes(saved[4096..4100].try_into().unwrap()),
            101
        );
        assert_eq!(
            u32::from_be_bytes(saved[4100..4104].try_into().unwrap()),
            102
        );
        assert_ne!(fs::read(&block_path).unwrap(), block_bytes);
    }

    fn poi_document() -> NamedTag {
        NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                ("future_root".into(), Tag::Int(1)),
                (
                    "Sections".into(),
                    Tag::Compound(vec![(
                        "0".into(),
                        Tag::Compound(vec![
                            ("Valid".into(), Tag::Byte(1)),
                            ("future_section".into(), Tag::Int(2)),
                            (
                                "Records".into(),
                                Tag::List {
                                    element_type: TagType::Compound,
                                    values: vec![Tag::Compound(vec![
                                        ("pos".into(), Tag::IntArray(vec![0, 0, 0])),
                                        ("type".into(), Tag::String("minecraft:farmer".into())),
                                        ("free_tickets".into(), Tag::Int(1)),
                                        ("future_record".into(), Tag::Int(3)),
                                    ])],
                                },
                            ),
                        ]),
                    )]),
                ),
            ]),
        }
    }

    fn poi_records_mut(document: &mut NamedTag) -> &mut Vec<Tag> {
        let Tag::Compound(root) = &mut document.value else {
            panic!()
        };
        let Tag::Compound(sections) = &mut root
            .iter_mut()
            .find(|(name, _)| name.matches("Sections"))
            .unwrap()
            .1
        else {
            panic!()
        };
        let Tag::Compound(section) = &mut sections[0].1 else {
            panic!()
        };
        let Tag::List { values, .. } = &mut section
            .iter_mut()
            .find(|(name, _)| name.matches("Records"))
            .unwrap()
            .1
        else {
            panic!()
        };
        values
    }

    #[test]
    fn coordinated_poi_creation_saves_with_terrain_and_backups() {
        let world = test_world();
        let (terrain_path, mut terrain_bytes, _) = fixture(world.path());
        let terrain = editable_chunk(0);
        store_record(&mut terrain_bytes, 0, 2, &terrain, 1);
        fs::write(&terrain_path, &terrain_bytes).unwrap();
        let mut plan = WorldSavePlan::new();
        let dimension = DimensionId::parse("overworld").unwrap();
        plan.add_chunk(
            dimension.clone(),
            block_draft(
                &terrain,
                ChunkPos { x: 0, z: 0 },
                "minecraft:stone",
                "minecraft:dirt",
            ),
            2,
        )
        .unwrap();
        let poi = poi_document();
        plan.add_poi_chunk(dimension, ChunkPos { x: 0, z: 0 }, None, &poi, 3, |_, _| {
            Ok(())
        })
        .unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        let result = guard.write_plan(plan, |_, _| Ok(())).unwrap();
        let path = world
            .path()
            .join("dimensions/minecraft/overworld/poi/r.0.0.mca");
        assert_eq!(result.poi_region_paths, vec![path.canonicalize().unwrap()]);
        assert_eq!(
            RegionReader::open(&path).unwrap().read_chunk(0, 0).unwrap(),
            Some(poi)
        );
        assert_ne!(fs::read(&terrain_path).unwrap(), terrain_bytes);
        assert_eq!(
            fs::read(result.backup_directory.join("region-0/r.0.0.mca")).unwrap(),
            terrain_bytes
        );
        let manifest =
            fs::read_to_string(result.backup_directory.join("transaction.json")).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&manifest).unwrap()["version"],
            4
        );
    }

    #[test]
    fn poi_preservation_and_stale_sources_stop_coordinated_writes() {
        let world = test_world();
        let (terrain_path, mut terrain_bytes, _) = fixture(world.path());
        let terrain = editable_chunk(0);
        store_record(&mut terrain_bytes, 0, 2, &terrain, 1);
        fs::write(&terrain_path, &terrain_bytes).unwrap();
        let directory = world.path().join("dimensions/minecraft/overworld/poi");
        fs::create_dir(&directory).unwrap();
        let path = directory.join("r.0.0.mca");
        let source = poi_document();
        let mut poi_bytes = vec![0; 3 * SECTOR_SIZE as usize];
        store_record(&mut poi_bytes, 0, 2, &source, 1);
        fs::write(&path, &poi_bytes).unwrap();
        let dimension = DimensionId::parse("overworld").unwrap();
        let mut deleted = source.clone();
        poi_records_mut(&mut deleted).clear();
        let mut plan = WorldSavePlan::new();
        assert!(
            plan.add_poi_chunk(
                dimension.clone(),
                ChunkPos { x: 0, z: 0 },
                Some(&source),
                &deleted,
                2,
                |_, _| Ok(())
            )
            .is_err()
        );
        let mut edited = source.clone();
        poi_records_mut(&mut edited).push(Tag::Compound(vec![
            ("pos".into(), Tag::IntArray(vec![1, 0, 0])),
            ("type".into(), Tag::String("minecraft:meeting".into())),
            ("free_tickets".into(), Tag::Int(32)),
        ]));
        plan.add_poi_chunk(
            dimension.clone(),
            ChunkPos { x: 0, z: 0 },
            Some(&source),
            &edited,
            2,
            |_, _| Ok(()),
        )
        .unwrap();
        plan.add_chunk(
            dimension,
            block_draft(
                &terrain,
                ChunkPos { x: 0, z: 0 },
                "minecraft:stone",
                "minecraft:dirt",
            ),
            2,
        )
        .unwrap();
        let mut outside = source.clone();
        let Tag::Compound(root) = &mut outside.value else {
            panic!()
        };
        root[1].1 = Tag::Int(99);
        store_record(&mut poi_bytes, 0, 2, &outside, 1);
        fs::write(&path, &poi_bytes).unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        assert!(guard.write_plan(plan, |_, _| Ok(())).is_err());
        assert_eq!(fs::read(&terrain_path).unwrap(), terrain_bytes);
        assert_eq!(fs::read(&path).unwrap(), poi_bytes);
    }

    #[test]
    fn existing_poi_region_is_backed_up_and_preserves_unknown_fields() {
        let world = test_world();
        fixture(world.path());
        let directory = world.path().join("dimensions/minecraft/overworld/poi");
        fs::create_dir(&directory).unwrap();
        let path = directory.join("r.0.0.mca");
        let source = poi_document();
        let mut bytes = vec![0; 3 * SECTOR_SIZE as usize];
        store_record(&mut bytes, 0, 2, &source, 1);
        fs::write(&path, &bytes).unwrap();
        let mut edited = source.clone();
        poi_records_mut(&mut edited).push(Tag::Compound(vec![
            ("pos".into(), Tag::IntArray(vec![1, 0, 0])),
            ("type".into(), Tag::String("minecraft:meeting".into())),
            ("free_tickets".into(), Tag::Int(32)),
        ]));
        let mut plan = WorldSavePlan::new();
        plan.add_poi_chunk(
            DimensionId::parse("overworld").unwrap(),
            ChunkPos { x: 0, z: 0 },
            Some(&source),
            &edited,
            2,
            |_, _| Ok(()),
        )
        .unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        let result = guard.write_plan(plan, |_, _| Ok(())).unwrap();
        assert_eq!(
            fs::read(result.backup_directory.join("region-0/r.0.0.mca")).unwrap(),
            bytes
        );
        assert_eq!(
            RegionReader::open(&path).unwrap().read_chunk(0, 0).unwrap(),
            Some(edited)
        );
    }

    #[test]
    fn stale_or_invalid_entity_chunks_never_replace_world_files() {
        let world = test_world();
        let (block_path, mut original_blocks, _) = fixture(world.path());
        let terrain = editable_chunk(0);
        store_record(&mut original_blocks, 0, 2, &terrain, 10);
        fs::write(&block_path, &original_blocks).unwrap();
        let directory = world.path().join("dimensions/minecraft/overworld/entities");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("r.0.0.mca");
        let entity = Tag::Compound(vec![("id".into(), Tag::String("minecraft:pig".into()))]);
        let source = entity_chunk(0, vec![entity.clone()]);
        let mut original = vec![0; 3 * SECTOR_SIZE as usize];
        store_record(&mut original, 0, 2, &source, 11);
        fs::write(&path, &original).unwrap();
        let dimension = DimensionId::parse("overworld").unwrap();
        let mut plan = WorldSavePlan::new();
        plan.add_chunk(
            dimension.clone(),
            block_draft(
                &terrain,
                ChunkPos { x: 0, z: 0 },
                "minecraft:stone",
                "minecraft:dirt",
            ),
            100,
        )
        .unwrap();
        assert!(
            plan.add_entity_chunk(
                dimension.clone(),
                ChunkPos { x: 1, z: 0 },
                None,
                &source,
                1,
                |_, _| Ok(())
            )
            .is_err()
        );
        let mut ambiguous = source.clone();
        let Tag::Compound(fields) = &mut ambiguous.value else {
            unreachable!()
        };
        fields.push(("Position".into(), Tag::IntArray(vec![0, 0])));
        assert!(
            plan.add_entity_chunk(
                dimension.clone(),
                ChunkPos { x: 0, z: 0 },
                Some(&source),
                &ambiguous,
                1,
                |_, _| Ok(())
            )
            .is_err()
        );
        plan.add_entity_chunk(
            dimension,
            ChunkPos { x: 0, z: 0 },
            None,
            &entity_chunk(0, vec![]),
            1,
            |_, _| Ok(()),
        )
        .unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        assert!(guard.write_plan(plan, |_, _| Ok(())).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(fs::read(&block_path).unwrap(), original_blocks);
        assert!(!world.path().join(".minecraft-native-pending").exists());
    }

    #[test]
    fn missing_empty_region_is_checked_without_creating_it() {
        let world = test_world();
        let (block_path, original_blocks, _) = fixture(world.path());
        let directory = world.path().join("dimensions/minecraft/overworld/entities");
        fs::create_dir_all(&directory).unwrap();
        let mut plan = WorldSavePlan::new();
        plan.add_entity_chunk(
            DimensionId::parse("overworld").unwrap(),
            ChunkPos { x: 32, z: 0 },
            None,
            &entity_chunk(32, vec![]),
            1,
            |_, _| Ok(()),
        )
        .unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        let result = guard.write_plan(plan, |_, _| Ok(())).unwrap();
        assert!(!directory.join("r.1.0.mca").exists());
        assert_eq!(fs::read(block_path).unwrap(), original_blocks);
        assert!(!world.path().join(".minecraft-native-pending").exists());
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(result.backup_directory.join("transaction.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["absent_dependencies"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn new_empty_entity_chunk_with_extra_data_is_not_discarded() {
        let world = test_world();
        fixture(world.path());
        let directory = world.path().join("dimensions/minecraft/overworld/entities");
        assert!(!directory.exists());
        let mut document = entity_chunk(32, vec![]);
        let Tag::Compound(fields) = &mut document.value else {
            unreachable!()
        };
        fields.push(("future".into(), Tag::String("kept".into())));
        let mut plan = WorldSavePlan::new();
        plan.add_entity_chunk(
            DimensionId::parse("overworld").unwrap(),
            ChunkPos { x: 32, z: 0 },
            None,
            &document,
            1,
            |_, _| Ok(()),
        )
        .unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        guard.write_plan(plan, |_, _| Ok(())).unwrap();
        let saved = RegionReader::open_for_region(&directory.join("r.1.0.mca"), 1, 0)
            .unwrap()
            .read_chunk(32, 0)
            .unwrap()
            .unwrap();
        assert_eq!(saved, document);
    }

    #[test]
    fn missing_entity_directory_is_an_absent_dependency_when_unchanged() {
        let world = test_world();
        let (block_path, mut original_blocks, _) = fixture(world.path());
        let terrain = editable_chunk(0);
        store_record(&mut original_blocks, 0, 2, &terrain, 10);
        fs::write(&block_path, &original_blocks).unwrap();
        let directory = world.path().join("dimensions/minecraft/overworld/entities");
        let dimension = DimensionId::parse("overworld").unwrap();
        let mut plan = WorldSavePlan::new();
        plan.add_chunk(
            dimension.clone(),
            block_draft(
                &terrain,
                ChunkPos { x: 0, z: 0 },
                "minecraft:stone",
                "minecraft:dirt",
            ),
            100,
        )
        .unwrap();
        for x in [0, 32, 64] {
            plan.add_entity_chunk(
                dimension.clone(),
                ChunkPos { x, z: 0 },
                None,
                &entity_chunk(x, vec![]),
                1,
                |_, _| Ok(()),
            )
            .unwrap();
        }
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        let result = guard.write_plan(plan, |_, _| Ok(())).unwrap();
        assert!(!directory.exists());
        assert_ne!(fs::read(&block_path).unwrap(), original_blocks);
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(result.backup_directory.join("transaction.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["version"], 3);
        assert_eq!(manifest["absent_directories"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn missing_entity_directory_rejects_expected_existing_source() {
        let world = test_world();
        fixture(world.path());
        let directory = world.path().join("dimensions/minecraft/overworld/entities");
        let source = entity_chunk(0, vec![]);
        let mut plan = WorldSavePlan::new();
        plan.add_entity_chunk(
            DimensionId::parse("overworld").unwrap(),
            ChunkPos { x: 0, z: 0 },
            Some(&source),
            &source,
            1,
            |_, _| Ok(()),
        )
        .unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        assert!(guard.write_plan(plan, |_, _| Ok(())).is_err());
        assert!(!directory.exists());
    }

    #[test]
    fn failed_staging_preserves_files_and_may_leave_an_empty_entity_directory() {
        let world = test_world();
        let (block_path, mut bytes, _) = fixture(world.path());
        let expected = editable_chunk(0);
        let mut current = expected.clone();
        let Tag::Compound(fields) = &mut current.value else {
            unreachable!()
        };
        fields.push(("external_change".into(), Tag::Int(1)));
        store_record(&mut bytes, 0, 2, &current, 10);
        fs::write(&block_path, &bytes).unwrap();
        let directory = world.path().join("dimensions/minecraft/overworld/entities");
        let dimension = DimensionId::parse("overworld").unwrap();
        let mut plan = WorldSavePlan::new();
        plan.add_chunk(
            dimension.clone(),
            block_draft(
                &expected,
                ChunkPos { x: 0, z: 0 },
                "minecraft:stone",
                "minecraft:dirt",
            ),
            100,
        )
        .unwrap();
        plan.add_entity_chunk(
            dimension,
            ChunkPos { x: 0, z: 0 },
            None,
            &entity_chunk(
                0,
                vec![Tag::Compound(vec![(
                    "id".into(),
                    Tag::String("minecraft:pig".into()),
                )])],
            ),
            1,
            |_, _| Ok(()),
        )
        .unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        assert!(guard.write_plan(plan, |_, _| Ok(())).is_err());
        assert_eq!(fs::read(&block_path).unwrap(), bytes);
        assert!(directory.is_dir());
        assert_eq!(fs::read_dir(directory).unwrap().count(), 0);
        assert!(!world.path().join(".minecraft-native-pending").exists());
    }

    #[test]
    fn empty_entity_chunk_keeps_unknown_root_fields() {
        let world = test_world();
        fixture(world.path());
        let directory = world.path().join("dimensions/minecraft/overworld/entities");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("r.0.0.mca");
        let mut source = entity_chunk(
            0,
            vec![Tag::Compound(vec![(
                "id".into(),
                Tag::String("minecraft:pig".into()),
            )])],
        );
        let Tag::Compound(fields) = &mut source.value else {
            unreachable!()
        };
        fields.push(("future".into(), Tag::Float(f32::NAN)));
        let mut replacement = source.clone();
        let Tag::Compound(fields) = &mut replacement.value else {
            unreachable!()
        };
        fields
            .iter_mut()
            .find(|(name, _)| name.matches("Entities"))
            .unwrap()
            .1 = Tag::List {
            element_type: TagType::Compound,
            values: vec![],
        };
        let mut bytes = vec![0; 3 * SECTOR_SIZE as usize];
        store_record(&mut bytes, 0, 2, &source, 1);
        fs::write(&path, bytes).unwrap();
        let dimension = DimensionId::parse("overworld").unwrap();
        let mut plan = WorldSavePlan::new();
        assert!(
            plan.add_entity_chunk(
                dimension.clone(),
                ChunkPos { x: 0, z: 0 },
                Some(&source),
                &entity_chunk(0, vec![]),
                2,
                |_, _| Ok(())
            )
            .is_err()
        );
        plan.add_entity_chunk(
            dimension,
            ChunkPos { x: 0, z: 0 },
            Some(&source),
            &replacement,
            2,
            |_, _| Ok(()),
        )
        .unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        guard.write_plan(plan, |_, _| Ok(())).unwrap();
        let saved = RegionReader::open_for_region(&path, 0, 0)
            .unwrap()
            .read_chunk(0, 0)
            .unwrap()
            .unwrap();
        assert_eq!(
            nbt::write(&saved).unwrap(),
            nbt::write(&replacement).unwrap()
        );
    }

    #[test]
    fn clearing_entity_chunk_refuses_to_remove_an_external_file() {
        let world = test_world();
        fixture(world.path());
        let directory = world.path().join("dimensions/minecraft/overworld/entities");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("r.0.0.mca");
        let external = directory.join("c.0.0.mcc");
        let source = entity_chunk(
            0,
            vec![Tag::Compound(vec![(
                "id".into(),
                Tag::String("minecraft:pig".into()),
            )])],
        );
        let mut bytes = vec![0; 3 * SECTOR_SIZE as usize];
        store_record(&mut bytes, 0, 2, &source, 1);
        fs::write(&path, &bytes).unwrap();
        fs::write(&external, b"unrelated external data").unwrap();
        let mut plan = WorldSavePlan::new();
        plan.add_entity_chunk(
            DimensionId::parse("overworld").unwrap(),
            ChunkPos { x: 0, z: 0 },
            Some(&source),
            &entity_chunk(0, vec![]),
            2,
            |_, _| Ok(()),
        )
        .unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        assert!(guard.write_plan(plan, |_, _| Ok(())).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(fs::read(&external).unwrap(), b"unrelated external data");
    }

    fn block_draft(
        original: &NamedTag,
        position: ChunkPos,
        expected: &str,
        replacement: &str,
    ) -> crate::edit::ChunkDraft {
        let change = BlockChange {
            x: position.x * 16,
            y: 0,
            z: position.z * 16,
            expected: block_state(expected),
            replacement: block_state(replacement),
        };
        let mut draft = draft_chunk_blocks(original, position, &[change]).unwrap();
        draft
            .rebuild_heightmaps(
                HeightBounds {
                    min_y: 0,
                    max_y: 16,
                },
                |state| match state.name.to_string_lossy().as_ref() {
                    "minecraft:air" => Ok(BlockHeightFlags {
                        is_air: true,
                        blocks_motion: false,
                        blocks_motion_no_leaves: false,
                        has_fluid: false,
                    }),
                    "minecraft:stone" | "minecraft:dirt" => Ok(BlockHeightFlags {
                        is_air: false,
                        blocks_motion: true,
                        blocks_motion_no_leaves: true,
                        has_fluid: false,
                    }),
                    name => anyhow::bail!("unexpected block state {name}"),
                },
            )
            .unwrap();
        draft
    }

    fn fixture(root: &Path) -> (PathBuf, Vec<u8>, Vec<u8>) {
        let level = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "Data".into(),
                Tag::Compound(vec![("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION))]),
            )]),
        };
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&nbt::write(&level).unwrap()).unwrap();
        fs::write(root.join("level.dat"), gzip.finish().unwrap()).unwrap();
        let directory = root.join("dimensions/minecraft/overworld/region");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("r.0.0.mca");
        let mut region = vec![0u8; 5 * SECTOR_SIZE as usize];
        for index in 0..3usize {
            region[index * 4..index * 4 + 4]
                .copy_from_slice(&((((index + 2) as u32) << 8) | 1).to_be_bytes());
            region[4096 + index * 4..4096 + index * 4 + 4]
                .copy_from_slice(&(1000 + index as u32).to_be_bytes());
        }
        for x in 0..2usize {
            let bytes = compressed(&chunk(x as i32, if x == 0 { "target" } else { "neighbor" }));
            let start = (x + 2) * SECTOR_SIZE as usize;
            region[start..start + 4].copy_from_slice(&((bytes.len() + 1) as u32).to_be_bytes());
            region[start + 4] = 2;
            region[start + 5..start + 5 + bytes.len()].copy_from_slice(&bytes);
        }
        let start = 4 * SECTOR_SIZE as usize;
        region[start..start + 4].copy_from_slice(&1u32.to_be_bytes());
        region[start + 4] = 0x82;
        let outside = compressed(&chunk(2, "external"));
        fs::write(directory.join("c.2.0.mcc"), &outside).unwrap();
        fs::write(&path, &region).unwrap();
        (path, region, outside)
    }

    #[test]
    fn named_chest_loot_requires_complete_sequence_and_world_seed_transaction() {
        use crate::{
            chest_loot::VerifiedChestLoot, edit::BlockEntityChange,
            random_sequences::RandomSequences,
        };
        use native_core::random::XoroshiroRandom;
        use native_resources::{chest_loot::ChestLootTable, packs::ResourceId};
        let world = test_world();
        let (region_path, mut region, _) = fixture(world.path());
        let position = ChunkPos { x: 0, z: 0 };
        let mut source = draft_chunk_blocks(
            &editable_chunk(0),
            position,
            &(0..2)
                .map(|x| BlockChange {
                    x,
                    y: 0,
                    z: 0,
                    expected: block_state("minecraft:stone"),
                    replacement: block_state("minecraft:chest"),
                })
                .collect::<Vec<_>>(),
        )
        .unwrap()
        .document()
        .clone();
        let list = |values| Tag::List {
            element_type: TagType::Compound,
            values,
        };
        let chest = |x| {
            Tag::Compound(vec![
                ("id".into(), Tag::String("minecraft:chest".into())),
                ("x".into(), Tag::Int(x)),
                ("y".into(), Tag::Int(0)),
                ("z".into(), Tag::Int(0)),
                ("LootTable".into(), Tag::String("example:treasure".into())),
            ])
        };
        let chests = [chest(0), chest(1)];
        let Tag::Compound(fields) = &mut source.value else {
            unreachable!()
        };
        fields.push(("block_entities".into(), list(chests.to_vec())));
        store_record(&mut region, 0, 2, &source, 10);
        fs::write(&region_path, &region).unwrap();
        let changes: Vec<_> = chests
            .iter()
            .map(|chest| {
                let mut replacement = chest.clone();
                let Tag::Compound(fields) = &mut replacement else {
                    unreachable!()
                };
                fields.retain(|(key, _)| !key.matches("LootTable"));
                fields.push(("Items".into(), list(vec![])));
                BlockEntityChange {
                    expected: chest.clone(),
                    replacement,
                }
            })
            .collect();
        let id = ResourceId::parse("example:treasure").unwrap();
        let table = ChestLootTable::decode(&serde_json::json!({
            "type": "minecraft:chest", "random_sequence": "example:treasure",
            "pools": [{"rolls": 1, "entries": [{"type": "minecraft:item", "name": "minecraft:diamond"}]}]
        })).unwrap();
        let mut sequences = RandomSequences::read(None).unwrap();
        let seed = 42;
        let first = VerifiedChestLoot::replay_named(
            &chests[0],
            &id,
            &table,
            seed,
            sequences.get("example:treasure", seed).unwrap().state(),
            |_| Ok(64),
        )
        .unwrap();
        let first_end = first.sequence.as_ref().unwrap().after;
        let second =
            VerifiedChestLoot::replay_named(&chests[1], &id, &table, seed, first_end, |_| Ok(64))
                .unwrap();
        let end = second.sequence.as_ref().unwrap().after;
        sequences
            .set(
                "example:treasure",
                &XoroshiroRandom::from_state(end.0, end.1),
            )
            .unwrap();
        let make_draft = |proofs: &[VerifiedChestLoot]| {
            let mut draft = draft_chunk_blocks(&source, position, &[]).unwrap();
            draft
                .stage_checked_chest_inventories_with_loot(&changes, proofs)
                .unwrap();
            draft
        };
        let proofs = [second.clone(), first.clone()];
        let uuid = [0, 0, 0, 1];
        let player = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                ("UUID".into(), Tag::IntArray(uuid.to_vec())),
            ]),
        };
        let mut updated = player.clone();
        let Tag::Compound(fields) = &mut updated.value else {
            unreachable!()
        };
        fields.push((
            "Inventory".into(),
            list(vec![Tag::Compound(vec![
                ("Slot".into(), Tag::Byte(0)),
                ("id".into(), Tag::String("minecraft:diamond".into())),
                ("count".into(), Tag::Int(2)),
            ])]),
        ));
        let player_dir = world.path().join("players/data");
        fs::create_dir_all(&player_dir).unwrap();
        let player_path = player_dir.join(player_data_filename(uuid));
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&nbt::write(&player).unwrap()).unwrap();
        let player_bytes = gzip.finish().unwrap();
        fs::write(&player_path, &player_bytes).unwrap();
        let data_dir = world.path().join("data/minecraft");
        fs::create_dir_all(&data_dir).unwrap();
        let seed_doc = |seed| NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                (
                    "data".into(),
                    Tag::Compound(vec![("seed".into(), Tag::Long(seed))]),
                ),
            ]),
        };
        let seed_path = data_dir.join("world_gen_settings.dat");
        let dimension = DimensionId::parse("overworld").unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        let plan = |with_sequences: bool, proofs: &[VerifiedChestLoot]| {
            let mut plan = WorldSavePlan::new();
            plan.add_chunk(dimension.clone(), make_draft(proofs), 20)
                .unwrap();
            plan.add_player_data(uuid, &player, &updated, |_, _| Ok(()))
                .unwrap();
            if with_sequences {
                plan.add_random_sequences(&sequences).unwrap();
            }
            plan
        };
        assert!(
            guard
                .write_plan(plan(false, &proofs), |_, _| Ok(()))
                .is_err()
        );
        let mut wrong_sequences = sequences.clone();
        wrong_sequences
            .set("example:treasure", &XoroshiroRandom::from_state(1, 2))
            .unwrap();
        let mut wrong_final = WorldSavePlan::new();
        wrong_final
            .add_chunk(dimension.clone(), make_draft(&proofs), 20)
            .unwrap();
        wrong_final
            .add_player_data(uuid, &player, &updated, |_, _| Ok(()))
            .unwrap();
        wrong_final.add_random_sequences(&wrong_sequences).unwrap();
        assert!(guard.write_plan(wrong_final, |_, _| Ok(())).is_err());
        let broken = VerifiedChestLoot::replay_named(
            &chests[1],
            &id,
            &table,
            seed,
            sequences.get("example:treasure", seed).unwrap().state(),
            |_| Ok(64),
        )
        .unwrap();
        assert!(
            guard
                .write_plan(plan(true, &[first.clone(), broken]), |_, _| Ok(()))
                .is_err()
        );
        fs::write(&seed_path, nbt::write(&seed_doc(seed + 1)).unwrap()).unwrap();
        assert!(
            guard
                .write_plan(plan(true, &proofs), |_, _| Ok(()))
                .is_err()
        );
        assert_eq!(fs::read(&region_path).unwrap(), region);
        assert_eq!(fs::read(&player_path).unwrap(), player_bytes);
        assert!(!data_dir.join("random_sequences.dat").exists());
        fs::write(&seed_path, nbt::write(&seed_doc(seed)).unwrap()).unwrap();
        let result = guard
            .write_plan(plan(true, &proofs), |_, _| Ok(()))
            .unwrap();
        assert_eq!(
            result.random_sequences_path,
            Some(
                data_dir
                    .join("random_sequences.dat")
                    .canonicalize()
                    .unwrap()
            )
        );
        assert!(
            result
                .backup_directory
                .join("world-data/world_gen_settings.dat")
                .exists()
        );
        let saved_sequences =
            nbt::read_gzip(File::open(data_dir.join("random_sequences.dat")).unwrap()).unwrap();
        assert_eq!(
            RandomSequences::read(Some(saved_sequences))
                .unwrap()
                .get("example:treasure", seed)
                .unwrap()
                .state(),
            end
        );
        assert_ne!(fs::read(&region_path).unwrap(), region);
        assert_ne!(fs::read(&player_path).unwrap(), player_bytes);
    }

    #[test]
    fn fixed_loot_replay_gates_atomic_chest_and_player_save() {
        use crate::{chest_loot::VerifiedFixedChestLoot, edit::BlockEntityChange};
        use native_resources::{chest_loot::ChestLootTable, packs::ResourceId};
        let world = test_world();
        let (region_path, mut region, _) = fixture(world.path());
        let position = ChunkPos { x: 0, z: 0 };
        let mut source = draft_chunk_blocks(
            &editable_chunk(0),
            position,
            &[BlockChange {
                x: 0,
                y: 0,
                z: 0,
                expected: block_state("minecraft:stone"),
                replacement: block_state("minecraft:chest"),
            }],
        )
        .unwrap()
        .document()
        .clone();
        let list = |values| Tag::List {
            element_type: TagType::Compound,
            values,
        };
        let chest = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:chest".into())),
            ("x".into(), Tag::Int(0)),
            ("y".into(), Tag::Int(0)),
            ("z".into(), Tag::Int(0)),
            ("LootTable".into(), Tag::String("example:treasure".into())),
            ("LootTableSeed".into(), Tag::Long(42)),
            ("future".into(), Tag::Int(17)),
        ]);
        let Tag::Compound(fields) = &mut source.value else {
            unreachable!()
        };
        fields.push(("block_entities".into(), list(vec![chest.clone()])));
        store_record(&mut region, 0, 2, &source, 10);
        fs::write(&region_path, &region).unwrap();
        let mut replacement = chest.clone();
        let Tag::Compound(fields) = &mut replacement else {
            unreachable!()
        };
        fields.retain(|(key, _)| !key.matches("LootTable") && !key.matches("LootTableSeed"));
        fields.push(("Items".into(), list(vec![])));
        let change = BlockEntityChange {
            expected: chest.clone(),
            replacement,
        };
        let id = ResourceId::parse("example:treasure").unwrap();
        let table = ChestLootTable::decode(&serde_json::json!({
            "type": "minecraft:chest", "pools": [{"rolls": 1, "entries": [
                {"type": "minecraft:item", "name": "minecraft:diamond"}
            ]}]
        }))
        .unwrap();
        assert!(
            VerifiedFixedChestLoot::replay(
                &chest,
                &ResourceId::parse("example:other").unwrap(),
                &table,
                |_| Ok(64)
            )
            .is_err()
        );
        let proof = VerifiedFixedChestLoot::replay(&chest, &id, &table, |_| Ok(64)).unwrap();
        let mut zero_seed = chest.clone();
        let Tag::Compound(fields) = &mut zero_seed else {
            unreachable!()
        };
        fields
            .iter_mut()
            .find(|(key, _)| key.matches("LootTableSeed"))
            .unwrap()
            .1 = Tag::Long(0);
        assert!(VerifiedFixedChestLoot::replay(&zero_seed, &id, &table, |_| Ok(64)).is_err());
        let mut unproven = draft_chunk_blocks(&source, position, &[]).unwrap();
        assert!(
            unproven
                .stage_checked_chest_inventories(std::slice::from_ref(&change))
                .is_err()
        );
        let mut altered = change.clone();
        let Tag::Compound(fields) = &mut altered.replacement else {
            unreachable!()
        };
        fields
            .iter_mut()
            .find(|(key, _)| key.matches("future"))
            .unwrap()
            .1 = Tag::Int(18);
        assert!(
            unproven
                .stage_checked_chest_inventories_with_loot(&[altered], std::slice::from_ref(&proof))
                .is_err()
        );
        let make_draft = || {
            let mut draft = draft_chunk_blocks(&source, position, &[]).unwrap();
            draft
                .stage_checked_chest_inventories_with_loot(
                    std::slice::from_ref(&change),
                    std::slice::from_ref(&proof),
                )
                .unwrap();
            draft
        };
        let uuid = [0, 0, 0, 1];
        let player = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                ("UUID".into(), Tag::IntArray(uuid.to_vec())),
            ]),
        };
        let stack = |count| {
            Tag::Compound(vec![
                ("Slot".into(), Tag::Byte(0)),
                ("id".into(), Tag::String("minecraft:diamond".into())),
                ("count".into(), Tag::Int(count)),
            ])
        };
        let updated = |count| {
            let mut result = player.clone();
            let Tag::Compound(fields) = &mut result.value else {
                unreachable!()
            };
            fields.push(("Inventory".into(), list(vec![stack(count)])));
            result
        };
        let player_dir = world.path().join("players/data");
        fs::create_dir_all(&player_dir).unwrap();
        let player_path = player_dir.join(player_data_filename(uuid));
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&nbt::write(&player).unwrap()).unwrap();
        let player_bytes = gzip.finish().unwrap();
        fs::write(&player_path, &player_bytes).unwrap();
        let dimension = DimensionId::parse("overworld").unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        let mut wrong = WorldSavePlan::new();
        wrong
            .add_chunk(dimension.clone(), make_draft(), 20)
            .unwrap();
        wrong
            .add_player_data(uuid, &player, &updated(2), |_, _| Ok(()))
            .unwrap();
        assert!(guard.write_plan(wrong, |_, _| Ok(())).is_err());
        assert_eq!(fs::read(&region_path).unwrap(), region);
        assert_eq!(fs::read(&player_path).unwrap(), player_bytes);
        let mut valid = WorldSavePlan::new();
        valid.add_chunk(dimension, make_draft(), 20).unwrap();
        valid
            .add_player_data(uuid, &player, &updated(1), |_, _| Ok(()))
            .unwrap();
        guard.write_plan(valid, |_, _| Ok(())).unwrap();
        assert_ne!(fs::read(&region_path).unwrap(), region);
        assert_ne!(fs::read(&player_path).unwrap(), player_bytes);
    }

    #[test]
    fn checked_chest_and_player_transfer_commit_together_or_not_at_all() {
        use crate::edit::BlockEntityChange;
        let world = test_world();
        let (region_path, mut region, _) = fixture(world.path());
        let position = ChunkPos { x: 0, z: 0 };
        let terrain = editable_chunk(0);
        let mut source = draft_chunk_blocks(
            &terrain,
            position,
            &[BlockChange {
                x: 0,
                y: 0,
                z: 0,
                expected: block_state("minecraft:stone"),
                replacement: block_state("minecraft:chest"),
            }],
        )
        .unwrap()
        .document()
        .clone();
        let stack = |slot, count| {
            Tag::Compound(vec![
                ("Slot".into(), Tag::Byte(slot)),
                ("id".into(), Tag::String("minecraft:diamond".into())),
                ("count".into(), Tag::Int(count)),
            ])
        };
        let items = |values| Tag::List {
            element_type: TagType::Compound,
            values,
        };
        let chest = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:chest".into())),
            ("x".into(), Tag::Int(0)),
            ("y".into(), Tag::Int(0)),
            ("z".into(), Tag::Int(0)),
            ("Items".into(), items(vec![stack(3, 5)])),
            ("future".into(), Tag::Int(42)),
        ]);
        let Tag::Compound(fields) = &mut source.value else {
            unreachable!()
        };
        fields.push(("block_entities".into(), items(vec![chest.clone()])));
        store_record(&mut region, 0, 2, &source, 10);
        fs::write(&region_path, &region).unwrap();
        let mut new_chest = chest.clone();
        let Tag::Compound(fields) = &mut new_chest else {
            unreachable!()
        };
        fields
            .iter_mut()
            .find(|(key, _)| key.matches("Items"))
            .unwrap()
            .1 = items(vec![]);
        let change = BlockEntityChange {
            expected: chest,
            replacement: new_chest.clone(),
        };
        let make_draft = || {
            let mut draft = draft_chunk_blocks(&source, position, &[]).unwrap();
            draft
                .stage_checked_chest_inventories(std::slice::from_ref(&change))
                .unwrap();
            draft
        };
        let uuid = [0, 0, 0, 1];
        let player = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                ("UUID".into(), Tag::IntArray(uuid.to_vec())),
                ("future_player".into(), Tag::Int(17)),
            ]),
        };
        let mut updated = player.clone();
        let Tag::Compound(fields) = &mut updated.value else {
            unreachable!()
        };
        fields.push(("Inventory".into(), items(vec![stack(8, 5)])));
        let player_dir = world.path().join("players/data");
        fs::create_dir_all(&player_dir).unwrap();
        let player_path = player_dir.join(player_data_filename(uuid));
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&nbt::write(&player).unwrap()).unwrap();
        let player_bytes = gzip.finish().unwrap();
        fs::write(&player_path, &player_bytes).unwrap();
        let dimension = DimensionId::parse("overworld").unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        let mut missing_player = WorldSavePlan::new();
        missing_player
            .add_chunk(dimension.clone(), make_draft(), 20)
            .unwrap();
        assert!(guard.write_plan(missing_player, |_, _| Ok(())).is_err());
        let mut wrong = updated.clone();
        let Tag::Compound(fields) = &mut wrong.value else {
            unreachable!()
        };
        fields
            .iter_mut()
            .find(|(key, _)| key.matches("Inventory"))
            .unwrap()
            .1 = items(vec![stack(8, 6)]);
        let mut unbalanced = WorldSavePlan::new();
        unbalanced
            .add_chunk(dimension.clone(), make_draft(), 20)
            .unwrap();
        unbalanced
            .add_player_data(uuid, &player, &wrong, |_, _| Ok(()))
            .unwrap();
        assert!(guard.write_plan(unbalanced, |_, _| Ok(())).is_err());
        assert_eq!(fs::read(&region_path).unwrap(), region);
        assert_eq!(fs::read(&player_path).unwrap(), player_bytes);
        let mut stale_plan = WorldSavePlan::new();
        stale_plan
            .add_chunk(dimension.clone(), make_draft(), 20)
            .unwrap();
        stale_plan
            .add_player_data(uuid, &player, &updated, |_, _| Ok(()))
            .unwrap();
        let mut changed_player = player.clone();
        let Tag::Compound(fields) = &mut changed_player.value else {
            unreachable!()
        };
        fields.push(("outside_change".into(), Tag::Int(1)));
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&nbt::write(&changed_player).unwrap())
            .unwrap();
        let changed_bytes = gzip.finish().unwrap();
        fs::write(&player_path, &changed_bytes).unwrap();
        assert!(guard.write_plan(stale_plan, |_, _| Ok(())).is_err());
        assert_eq!(fs::read(&region_path).unwrap(), region);
        assert_eq!(fs::read(&player_path).unwrap(), changed_bytes);
        fs::write(&player_path, &player_bytes).unwrap();
        let mut plan = WorldSavePlan::new();
        plan.add_chunk(dimension.clone(), make_draft(), 20).unwrap();
        plan.add_player_data(uuid, &player, &updated, |_, _| Ok(()))
            .unwrap();
        let result = guard.write_plan(plan, |_, _| Ok(())).unwrap();
        let saved = RegionReader::open_for_region(&region_path, 0, 0)
            .unwrap()
            .read_chunk(0, 0)
            .unwrap()
            .unwrap();
        assert!(
            matches!(saved.value.get("block_entities"), Some(Tag::List { values, .. }) if values == &vec![new_chest])
        );
        assert_eq!(
            nbt::read_gzip(File::open(&player_path).unwrap()).unwrap(),
            updated
        );
        assert_eq!(
            fs::read(
                result
                    .backup_directory
                    .join("players")
                    .join(player_data_filename(uuid))
            )
            .unwrap(),
            player_bytes
        );
        assert!(!world.path().join(".minecraft-native-pending").exists());
    }

    #[test]
    fn uncoordinated_chest_aborts_entire_save_before_staging() {
        use crate::edit::BlockEntityMutation;

        let world = test_world();
        let (path, mut region, outside) = fixture(world.path());
        let level = fs::read(world.path().join("level.dat")).unwrap();
        let first = editable_chunk(0);
        let second = editable_chunk(1);
        store_record(&mut region, 0, 2, &first, 1);
        store_record(&mut region, 1, 2, &second, 1);
        fs::write(&path, &region).unwrap();

        let good = block_draft(
            &first,
            ChunkPos { x: 0, z: 0 },
            "minecraft:stone",
            "minecraft:dirt",
        );
        let mut chest = draft_chunk_blocks(
            &second,
            ChunkPos { x: 1, z: 0 },
            &[BlockChange {
                x: 16,
                y: 0,
                z: 0,
                expected: block_state("minecraft:stone"),
                replacement: block_state("minecraft:chest"),
            }],
        )
        .unwrap();
        chest
            .rebuild_heightmaps(
                HeightBounds {
                    min_y: 0,
                    max_y: 16,
                },
                |state| {
                    let is_air = state.name.matches("minecraft:air");
                    Ok(BlockHeightFlags {
                        is_air,
                        blocks_motion: !is_air,
                        blocks_motion_no_leaves: !is_air,
                        has_fluid: false,
                    })
                },
            )
            .unwrap();
        chest
            .stage_block_entity_mutations(&[BlockEntityMutation {
                expected: None,
                replacement: Some(Tag::Compound(vec![
                    ("id".into(), Tag::String("minecraft:chest".into())),
                    ("x".into(), Tag::Int(16)),
                    ("y".into(), Tag::Int(0)),
                    ("z".into(), Tag::Int(0)),
                ])),
            }])
            .unwrap();
        let dimension = DimensionId::parse("overworld").unwrap();
        let mut plan = WorldSavePlan::new();
        plan.add_chunk(dimension.clone(), good, 2).unwrap();
        plan.add_chunk(dimension, chest, 2).unwrap();
        let mut sequences = crate::random_sequences::RandomSequences::read(None).unwrap();
        let mut random = sequences
            .get("minecraft:chests/buried_treasure", 7)
            .unwrap();
        random.next_long();
        sequences
            .set("minecraft:chests/buried_treasure", &random)
            .unwrap();
        plan.add_random_sequences(&sequences).unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        let error = guard.write_plan(plan, |_, _| Ok(())).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("block entity edits require a coordinated world save")
        );
        assert_eq!(fs::read(&path).unwrap(), region);
        assert_eq!(fs::read(path.with_file_name("c.2.0.mcc")).unwrap(), outside);
        assert_eq!(fs::read(world.path().join("level.dat")).unwrap(), level);
        assert!(
            !world
                .path()
                .join("data/minecraft/random_sequences.dat")
                .exists()
        );
        assert!(!world.path().join(".minecraft-native-backups").exists());
        assert!(!world.path().join(".minecraft-native-pending").exists());
    }

    #[test]
    fn playable_reader_shares_lock_until_last_session_owner_is_dropped() {
        use crate::world::WorldReader;
        let world = test_world();
        let (region, original, external) = fixture(world.path());
        let dimension = DimensionId::parse("overworld").unwrap();
        let mut reader = WorldReader::open_for_session(world.path(), dimension.clone()).unwrap();
        assert!(reader.read_chunk(0, 0).unwrap().is_some());
        let error = WorldReader::open_for_session(world.path(), dimension)
            .err()
            .unwrap();
        assert!(error.downcast_ref::<WorldInUse>().is_some());
        assert!(
            WorldWriteGuard::acquire(world.path())
                .err()
                .unwrap()
                .downcast_ref::<WorldInUse>()
                .is_some()
        );
        let shared = reader.session_guard().unwrap();
        drop(reader);
        assert!(WorldWriteGuard::acquire(world.path()).is_err());
        drop(shared);
        assert!(WorldWriteGuard::acquire(world.path()).is_ok());
        assert_eq!(fs::read(region).unwrap(), original);
        assert_eq!(
            fs::read(
                world
                    .path()
                    .join("dimensions/minecraft/overworld/region/c.2.0.mcc")
            )
            .unwrap(),
            external
        );
    }

    #[test]
    fn active_session_can_share_its_lock_with_one_writer() {
        use crate::world::WorldReader;
        let world = test_world();
        let (region, original, external) = fixture(world.path());
        let reader =
            WorldReader::open_for_session(world.path(), DimensionId::parse("overworld").unwrap())
                .unwrap();
        let session = reader.session_guard().unwrap();
        let mut writer = session.acquire_write().unwrap();
        assert!(session.acquire_write().is_err());
        assert!(WorldWriteGuard::acquire(world.path()).is_err());
        let result = writer
            .rewrite_chunk_losslessly(&DimensionId::parse("overworld").unwrap(), 0, 0)
            .unwrap();
        assert_eq!(
            fs::read(result.backup_directory.join("r.0.0.mca")).unwrap(),
            original
        );
        assert_eq!(
            fs::read(result.backup_directory.join("c.2.0.mcc")).unwrap(),
            external
        );
        assert!(region.is_file());
        drop(writer);
        assert!(session.acquire_write().is_ok());
        drop(reader);
        assert!(WorldWriteGuard::acquire(world.path()).is_err());
        drop(session);
        assert!(WorldWriteGuard::acquire(world.path()).is_ok());
    }

    #[test]
    fn lossless_rewrite_keeps_untouched_records_and_backups_external_chunks() {
        let world = test_world();
        let (region_path, old_region, outside) = fixture(world.path());
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        assert!(WorldWriteGuard::acquire(world.path()).is_err());
        let result = guard
            .rewrite_chunk_losslessly(&DimensionId::parse("overworld").unwrap(), 0, 0)
            .unwrap();
        assert_eq!(
            fs::read(result.backup_directory.join("r.0.0.mca")).unwrap(),
            old_region
        );
        assert_eq!(
            fs::read(result.backup_directory.join("c.2.0.mcc")).unwrap(),
            outside
        );
        let new_region = fs::read(&region_path).unwrap();
        assert_eq!(
            &new_region[..HEADER_SIZE as usize],
            &old_region[..HEADER_SIZE as usize]
        );
        assert_eq!(
            &new_region[3 * SECTOR_SIZE as usize..],
            &old_region[3 * SECTOR_SIZE as usize..]
        );
        assert_eq!(new_region.len(), old_region.len());
        let mut reader = RegionReader::open(&region_path).unwrap();
        for x in 0..3 {
            let expected = chunk(x, ["target", "neighbor", "external"][x as usize]);
            assert_eq!(
                nbt::write(&reader.read_chunk(x, 0).unwrap().unwrap()).unwrap(),
                nbt::write(&expected).unwrap()
            );
        }
        drop(guard);
        assert!(WorldWriteGuard::acquire(world.path()).is_ok());
    }

    #[test]
    fn validated_chunk_draft_is_backed_up_written_and_reopened() {
        let world = test_world();
        let (region_path, old_region, outside) = fixture(world.path());
        let original = editable_chunk(0);
        let mut region = old_region;
        let encoded = compressed(&original);
        assert!(encoded.len() + 5 <= SECTOR_SIZE as usize);
        let start = 2 * SECTOR_SIZE as usize;
        region[start..start + 4].copy_from_slice(&((encoded.len() + 1) as u32).to_be_bytes());
        region[start + 4] = 2;
        region[start + 5..start + 5 + encoded.len()].copy_from_slice(&encoded);
        fs::write(&region_path, &region).unwrap();
        let source_region = fs::read(&region_path).unwrap();

        let position = ChunkPos { x: 0, z: 0 };
        let change = BlockChange {
            x: 0,
            y: 0,
            z: 0,
            expected: block_state("minecraft:stone"),
            replacement: block_state("minecraft:dirt"),
        };
        let mut draft = draft_chunk_blocks(&original, position, &[change]).unwrap();
        let bounds = HeightBounds {
            min_y: 0,
            max_y: 16,
        };
        let classify = |state: &BlockState| {
            if state.name.matches("minecraft:air") {
                Ok(BlockHeightFlags {
                    is_air: true,
                    blocks_motion: false,
                    blocks_motion_no_leaves: false,
                    has_fluid: false,
                })
            } else if state.name.matches("minecraft:stone") || state.name.matches("minecraft:dirt")
            {
                Ok(BlockHeightFlags {
                    is_air: false,
                    blocks_motion: true,
                    blocks_motion_no_leaves: true,
                    has_fluid: false,
                })
            } else {
                anyhow::bail!("unexpected block state {}", state.name.to_string_lossy())
            }
        };
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        assert!(
            guard
                .write_chunk_draft(
                    &DimensionId::parse("overworld").unwrap(),
                    &draft,
                    7_777,
                    |_| Ok(()),
                )
                .is_err()
        );
        assert_eq!(fs::read(&region_path).unwrap(), source_region);

        draft.rebuild_heightmaps(bounds, classify).unwrap();
        assert!(
            guard
                .write_chunk_draft(
                    &DimensionId::parse("overworld").unwrap(),
                    &draft,
                    7_777,
                    |_| anyhow::bail!("registry rejected the edit"),
                )
                .is_err()
        );
        assert_eq!(fs::read(&region_path).unwrap(), source_region);

        let mut changed_source = original.clone();
        let Tag::Compound(fields) = &mut changed_source.value else {
            unreachable!()
        };
        let (_, unknown) = fields
            .iter_mut()
            .find(|(name, _)| name.matches("unknown"))
            .unwrap();
        let Tag::Compound(fields) = unknown else {
            unreachable!()
        };
        let (_, marker) = fields
            .iter_mut()
            .find(|(name, _)| name.matches("value"))
            .unwrap();
        *marker = Tag::String("changed outside draft".into());
        let changed_bytes = compressed(&changed_source);
        let mut stale_region = source_region.clone();
        stale_region[start..start + 4]
            .copy_from_slice(&((changed_bytes.len() + 1) as u32).to_be_bytes());
        stale_region[start + 5..start + 5 + changed_bytes.len()].copy_from_slice(&changed_bytes);
        fs::write(&region_path, &stale_region).unwrap();
        assert!(
            guard
                .write_chunk_draft(
                    &DimensionId::parse("overworld").unwrap(),
                    &draft,
                    7_777,
                    |_| Ok(()),
                )
                .is_err()
        );
        assert_eq!(fs::read(&region_path).unwrap(), stale_region);
        fs::write(&region_path, &source_region).unwrap();

        let result = guard
            .write_chunk_draft(
                &DimensionId::parse("overworld").unwrap(),
                &draft,
                7_777,
                |change| {
                    anyhow::ensure!(
                        change.expected == block_state("minecraft:stone")
                            && change.replacement == block_state("minecraft:dirt"),
                        "unexpected edit reached the validator"
                    );
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(
            fs::read(result.backup_directory.join("r.0.0.mca")).unwrap(),
            source_region
        );
        assert_eq!(
            fs::read(result.backup_directory.join("c.2.0.mcc")).unwrap(),
            outside
        );

        let bytes = fs::read(&region_path).unwrap();
        assert_eq!(bytes.len(), source_region.len());
        assert_eq!(
            &bytes[..SECTOR_SIZE as usize],
            &source_region[..SECTOR_SIZE as usize]
        );
        assert_eq!(
            &bytes[SECTOR_SIZE as usize + 4..HEADER_SIZE as usize],
            &source_region[SECTOR_SIZE as usize + 4..HEADER_SIZE as usize]
        );
        assert_eq!(
            u32::from_be_bytes(
                bytes[SECTOR_SIZE as usize..SECTOR_SIZE as usize + 4]
                    .try_into()
                    .unwrap()
            ),
            7_777
        );

        let mut reader = RegionReader::open(&region_path).unwrap();
        let saved = reader.read_chunk(0, 0).unwrap().unwrap();
        assert_eq!(saved.value.get("unknown"), original.value.get("unknown"));
        let Tag::List {
            values: saved_sections,
            ..
        } = saved.value.get("sections").unwrap()
        else {
            panic!("saved chunk has no sections")
        };
        let Tag::List {
            values: original_sections,
            ..
        } = original.value.get("sections").unwrap()
        else {
            panic!("original chunk has no sections")
        };
        assert_eq!(
            saved_sections[0].get("unknown_section"),
            original_sections[0].get("unknown_section")
        );
        let sections = palette::decode_sections(&saved.value).unwrap();
        let blocks = sections[0].blocks.as_ref().unwrap();
        assert!(
            blocks
                .block(0, 0, 0)
                .unwrap()
                .name
                .matches("minecraft:dirt")
        );
        assert!(
            blocks
                .block(1, 0, 0)
                .unwrap()
                .name
                .matches("minecraft:stone")
        );
        let maps = SpawnHeightmaps::read(&saved, bounds).unwrap();
        assert_eq!(
            maps.highest(SpawnHeightmapKind::WorldSurface, 0, 0),
            Some(15)
        );

        assert_eq!(
            reader.read_chunk(1, 0).unwrap().unwrap(),
            chunk(1, "neighbor")
        );
        assert_eq!(
            reader.read_chunk(2, 0).unwrap().unwrap(),
            chunk(2, "external")
        );
        assert!(!world.path().join(".minecraft-native-pending").exists());
    }

    #[test]
    fn world_save_plan_commits_multiple_chunks_across_regions_atomically() {
        let world = test_world();
        let (region_zero, original_zero, outside) = fixture(world.path());
        let level_path = world.path().join("level.dat");
        let original_level_bytes = fs::read(&level_path).unwrap();
        let (_, level_source) = anvil::read_level_document(world.path()).unwrap();
        let player_data_dir = world.path().join("players/data");
        fs::create_dir_all(&player_data_dir).unwrap();
        let player_uuid = [0x1234_5678, 0x2345_6789, 0x3456_789a, 0x4567_89ab];
        let player_filename = player_data_filename(player_uuid);
        assert_eq!(player_filename, "12345678-2345-6789-3456-789a456789ab.dat");
        let player_path = player_data_dir.join(&player_filename);
        let player_source = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                (
                    "Pos".into(),
                    Tag::List {
                        element_type: TagType::Double,
                        values: vec![Tag::Double(1.0), Tag::Double(64.0), Tag::Double(2.0)],
                    },
                ),
                ("unknown_player_data".into(), Tag::Long(99)),
            ]),
        };
        let mut player_replacement = player_source.clone();
        let Tag::Compound(fields) = &mut player_replacement.value else {
            unreachable!()
        };
        let (_, position) = fields
            .iter_mut()
            .find(|(name, _)| name.matches("Pos"))
            .unwrap();
        *position = Tag::List {
            element_type: TagType::Double,
            values: vec![Tag::Double(8.5), Tag::Double(65.0), Tag::Double(2.5)],
        };
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&nbt::write(&player_source).unwrap())
            .unwrap();
        let original_player_bytes = gzip.finish().unwrap();
        fs::write(&player_path, &original_player_bytes).unwrap();
        let clock_path = world.path().join("data/minecraft/world_clocks.dat");
        fs::create_dir_all(clock_path.parent().unwrap()).unwrap();
        let clock_source = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                (
                    "data".into(),
                    Tag::Compound(vec![(
                        "minecraft:overworld".into(),
                        Tag::Compound(vec![
                            ("total_ticks".into(), Tag::Long(1_000)),
                            ("future_clock_field".into(), Tag::String("preserved".into())),
                        ]),
                    )]),
                ),
                ("future_root_field".into(), Tag::Int(42)),
            ]),
        };
        let mut clock_gzip = GzEncoder::new(Vec::new(), Compression::default());
        clock_gzip
            .write_all(&nbt::write(&clock_source).unwrap())
            .unwrap();
        let original_clock_bytes = clock_gzip.finish().unwrap();
        fs::write(&clock_path, &original_clock_bytes).unwrap();
        let weather_path = world.path().join("data/minecraft/weather.dat");
        let weather_source = crate::world::WorldReader::update_weather_document(
            None,
            crate::world::SavedWeather::default(),
        )
        .unwrap();
        let mut weather_gzip = GzEncoder::new(Vec::new(), Compression::default());
        weather_gzip
            .write_all(&nbt::write(&weather_source).unwrap())
            .unwrap();
        let original_weather_bytes = weather_gzip.finish().unwrap();
        fs::write(&weather_path, &original_weather_bytes).unwrap();
        let weather = crate::world::SavedWeather {
            clear_weather_time: 9,
            rain_time: 31,
            thunder_time: 47,
            raining: true,
            thundering: false,
        };
        let clock_states = BTreeMap::from([(
            DimensionId::parse("overworld").unwrap(),
            crate::world::SavedClock {
                total_ticks: 1_001,
                partial_tick: 0.25,
                rate: 1.0,
                paused: false,
            },
        )]);
        let mut source_zero = original_zero;
        let chunk_zero = editable_chunk(0);
        let chunk_one = editable_chunk(1);
        store_record(&mut source_zero, 0, 2, &chunk_zero, 1_000);
        store_record(&mut source_zero, 1, 3, &chunk_one, 1_001);
        fs::write(&region_zero, &source_zero).unwrap();

        let region_directory = region_zero.parent().unwrap();
        let region_one = region_directory.join("r.1.0.mca");
        let mut source_one = vec![0; 3 * SECTOR_SIZE as usize];
        let chunk_thirty_two = editable_chunk(32);
        store_record(&mut source_one, 0, 2, &chunk_thirty_two, 1_002);
        fs::write(&region_one, &source_one).unwrap();
        let before_zero = fs::read(&region_zero).unwrap();

        let dimension = DimensionId::parse("overworld").unwrap();
        let mut plan = WorldSavePlan::new();
        plan.add_chunk(
            dimension.clone(),
            block_draft(
                &chunk_zero,
                ChunkPos { x: 0, z: 0 },
                "minecraft:stone",
                "minecraft:dirt",
            ),
            7_000,
        )
        .unwrap();
        plan.add_chunk(
            dimension.clone(),
            block_draft(
                &chunk_one,
                ChunkPos { x: 1, z: 0 },
                "minecraft:stone",
                "minecraft:dirt",
            ),
            7_001,
        )
        .unwrap();
        plan.add_chunk(
            dimension.clone(),
            block_draft(
                &chunk_thirty_two,
                ChunkPos { x: 32, z: 0 },
                "minecraft:stone",
                "minecraft:dirt",
            ),
            7_002,
        )
        .unwrap();
        plan.add_player_data(
            player_uuid,
            &player_source,
            &player_replacement,
            |source, replacement| {
                anyhow::ensure!(source == &player_source);
                anyhow::ensure!(replacement == &player_replacement);
                Ok(())
            },
        )
        .unwrap();
        plan.add_world_clocks(&clock_source, &clock_states).unwrap();
        plan.add_weather(Some(&weather_source), weather).unwrap();
        plan.add_level_time(&level_source, 12345).unwrap();
        assert_eq!(plan.chunk_count(), 3);

        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        let result = guard
            .write_plan(plan, |target_dimension, change| {
                anyhow::ensure!(target_dimension == &dimension);
                anyhow::ensure!(
                    change.expected == block_state("minecraft:stone")
                        && change.replacement == block_state("minecraft:dirt")
                );
                Ok(())
            })
            .unwrap();
        assert_eq!(
            result.region_paths,
            vec![
                region_zero.canonicalize().unwrap(),
                region_one.canonicalize().unwrap()
            ]
        );
        assert_eq!(
            result.player_paths,
            vec![player_path.canonicalize().unwrap()]
        );
        assert_eq!(
            result.world_clocks_path,
            Some(clock_path.canonicalize().unwrap())
        );
        assert_eq!(result.level_path, Some(level_path.canonicalize().unwrap()));
        assert_eq!(
            fs::read(world.path().join("level.dat_old")).unwrap(),
            original_level_bytes
        );
        assert_eq!(
            crate::world::WorldReader::validate_level_game_time(
                &nbt::read_gzip(File::open(&level_path).unwrap()).unwrap()
            )
            .unwrap(),
            12345
        );
        assert_eq!(
            result.weather_path,
            Some(weather_path.canonicalize().unwrap())
        );
        assert_eq!(
            fs::read(result.backup_directory.join("world-data/weather.dat")).unwrap(),
            original_weather_bytes
        );
        assert_eq!(
            crate::world::parse_weather_data(
                &nbt::read_gzip(File::open(weather_path).unwrap()).unwrap()
            )
            .unwrap(),
            weather
        );
        assert_eq!(
            fs::read(result.backup_directory.join("world-data/world_clocks.dat")).unwrap(),
            original_clock_bytes
        );
        let saved_clock_file = nbt::read_gzip(File::open(&clock_path).unwrap()).unwrap();
        assert_eq!(
            saved_clock_file.value.get("future_root_field"),
            Some(&Tag::Int(42))
        );
        assert_eq!(
            saved_clock_file
                .value
                .get("data")
                .unwrap()
                .get("minecraft:overworld")
                .unwrap()
                .get("future_clock_field"),
            Some(&Tag::String("preserved".into()))
        );
        assert_eq!(
            crate::world::WorldReader::update_world_clock_document(&clock_source, &clock_states)
                .unwrap(),
            saved_clock_file
        );
        assert_eq!(
            fs::read(
                result
                    .backup_directory
                    .join("players")
                    .join(&player_filename)
            )
            .unwrap(),
            original_player_bytes
        );
        assert_eq!(
            nbt::read_gzip(File::open(&player_path).unwrap()).unwrap(),
            player_replacement
        );
        assert_eq!(
            fs::read(result.backup_directory.join("region-0/r.0.0.mca")).unwrap(),
            before_zero
        );
        assert_eq!(
            fs::read(result.backup_directory.join("region-1/r.1.0.mca")).unwrap(),
            source_one
        );
        assert_eq!(
            fs::read(result.backup_directory.join("region-0/c.2.0.mcc")).unwrap(),
            outside
        );

        let mut reader = RegionReader::open(&region_zero).unwrap();
        for x in [0, 1] {
            let saved = reader.read_chunk(x, 0).unwrap().unwrap();
            let section = palette::decode_sections(&saved.value).unwrap().remove(0);
            assert!(
                section
                    .blocks
                    .unwrap()
                    .block(0, 0, 0)
                    .unwrap()
                    .name
                    .matches("minecraft:dirt")
            );
        }
        assert_eq!(
            reader.read_chunk(2, 0).unwrap().unwrap(),
            chunk(2, "external")
        );
        drop(reader);
        let mut reader = RegionReader::open(&region_one).unwrap();
        let saved = reader.read_chunk(32, 0).unwrap().unwrap();
        assert!(
            palette::decode_sections(&saved.value)
                .unwrap()
                .remove(0)
                .blocks
                .unwrap()
                .block(0, 0, 0)
                .unwrap()
                .name
                .matches("minecraft:dirt")
        );

        // A stale source in a later region aborts the entire plan before any
        // region replacement, even after earlier regions were staged.
        let current_zero = reader_chunk(&region_zero, 0);
        let current_thirty_two = reader_chunk(&region_one, 32);
        let mut stale_plan = WorldSavePlan::new();
        stale_plan
            .add_chunk(
                dimension.clone(),
                block_draft(
                    &current_zero,
                    ChunkPos { x: 0, z: 0 },
                    "minecraft:dirt",
                    "minecraft:stone",
                ),
                8_000,
            )
            .unwrap();
        stale_plan
            .add_player_data(player_uuid, &player_replacement, &player_source, |_, _| {
                Ok(())
            })
            .unwrap();
        stale_plan
            .add_chunk(
                dimension,
                block_draft(
                    &current_thirty_two,
                    ChunkPos { x: 32, z: 0 },
                    "minecraft:dirt",
                    "minecraft:stone",
                ),
                8_001,
            )
            .unwrap();
        let mut stale_region = fs::read(&region_one).unwrap();
        let mut stale_chunk = current_thirty_two;
        let Tag::Compound(fields) = &mut stale_chunk.value else {
            unreachable!()
        };
        let (_, unknown) = fields
            .iter_mut()
            .find(|(name, _)| name.matches("unknown"))
            .unwrap();
        let Tag::Compound(fields) = unknown else {
            unreachable!()
        };
        let (_, marker) = fields
            .iter_mut()
            .find(|(name, _)| name.matches("value"))
            .unwrap();
        *marker = Tag::String("stale after draft".into());
        store_record(&mut stale_region, 0, 2, &stale_chunk, 7_002);
        fs::write(&region_one, &stale_region).unwrap();
        let before_failed_plan_zero = fs::read(&region_zero).unwrap();
        assert!(guard.write_plan(stale_plan, |_, _| Ok(())).is_err());
        assert_eq!(fs::read(&region_zero).unwrap(), before_failed_plan_zero);
        assert_eq!(fs::read(&region_one).unwrap(), stale_region);
        assert_eq!(
            nbt::read_gzip(File::open(&player_path).unwrap()).unwrap(),
            player_replacement
        );
        assert!(!world.path().join(".minecraft-native-pending").exists());
    }

    #[test]
    fn world_save_plan_rejects_stale_player_before_replacing_chunks() {
        let world = test_world();
        let (region_path, mut original_region, _) = fixture(world.path());
        let source_chunk = editable_chunk(0);
        store_record(&mut original_region, 0, 2, &source_chunk, 1_000);
        fs::write(&region_path, &original_region).unwrap();
        let player_data_dir = world.path().join("players/data");
        fs::create_dir_all(&player_data_dir).unwrap();
        let player_uuid = [0x1234_5678, 0x2345_6789, 0x3456_789a, 0x4567_89ab];
        let player_path = player_data_dir.join(player_data_filename(player_uuid));
        let player_source = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION))]),
        };
        let player_replacement = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                ("changed".into(), Tag::Byte(1)),
            ]),
        };
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&nbt::write(&player_source).unwrap())
            .unwrap();
        fs::write(&player_path, gzip.finish().unwrap()).unwrap();

        let dimension = DimensionId::parse("overworld").unwrap();
        let mut plan = WorldSavePlan::new();
        plan.add_chunk(
            dimension.clone(),
            block_draft(
                &source_chunk,
                ChunkPos { x: 0, z: 0 },
                "minecraft:stone",
                "minecraft:dirt",
            ),
            7_000,
        )
        .unwrap();
        plan.add_player_data(player_uuid, &player_source, &player_replacement, |_, _| {
            Ok(())
        })
        .unwrap();
        let mut stale_player = player_source.clone();
        let Tag::Compound(fields) = &mut stale_player.value else {
            unreachable!()
        };
        fields.push(("external_change".into(), Tag::Int(1)));
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&nbt::write(&stale_player).unwrap()).unwrap();
        let stale_player_bytes = gzip.finish().unwrap();
        fs::write(&player_path, &stale_player_bytes).unwrap();

        let before_region = fs::read(&region_path).unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        assert!(guard.write_plan(plan, |_, _| Ok(())).is_err());
        assert_eq!(fs::read(&region_path).unwrap(), before_region);
        assert_eq!(fs::read(&player_path).unwrap(), stale_player_bytes);
        assert!(!world.path().join(".minecraft-native-pending").exists());
    }

    #[test]
    fn world_save_plan_rejects_stale_clocks_before_replacement() {
        let world = test_world();
        let (region_path, original_region, _) = fixture(world.path());
        let clock_path = world.path().join("data/minecraft/world_clocks.dat");
        fs::create_dir_all(clock_path.parent().unwrap()).unwrap();
        let clock_source = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                (
                    "data".into(),
                    Tag::Compound(vec![(
                        "minecraft:overworld".into(),
                        Tag::Compound(vec![("total_ticks".into(), Tag::Long(10))]),
                    )]),
                ),
            ]),
        };
        let write_clock = |document: &NamedTag| {
            let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
            gzip.write_all(&nbt::write(document).unwrap()).unwrap();
            fs::write(&clock_path, gzip.finish().unwrap()).unwrap();
        };
        write_clock(&clock_source);
        let clocks = BTreeMap::from([(
            DimensionId::parse("overworld").unwrap(),
            crate::world::SavedClock {
                total_ticks: 11,
                ..crate::world::SavedClock::default()
            },
        )]);
        let mut plan = WorldSavePlan::new();
        plan.add_world_clocks(&clock_source, &clocks).unwrap();

        let stale_clock = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                (
                    "data".into(),
                    Tag::Compound(vec![(
                        "minecraft:overworld".into(),
                        Tag::Compound(vec![("total_ticks".into(), Tag::Long(12))]),
                    )]),
                ),
            ]),
        };
        write_clock(&stale_clock);
        let stale_clock_bytes = fs::read(&clock_path).unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        assert!(guard.write_plan(plan, |_, _| Ok(())).is_err());
        assert_eq!(fs::read(&clock_path).unwrap(), stale_clock_bytes);
        assert_eq!(fs::read(&region_path).unwrap(), original_region);
        assert!(!world.path().join(".minecraft-native-pending").exists());
    }

    fn reader_chunk(path: &Path, x: i32) -> NamedTag {
        RegionReader::open(path)
            .unwrap()
            .read_chunk(x, 0)
            .unwrap()
            .unwrap()
    }

    #[test]
    fn unsupported_and_overlapping_records_stop_before_replacement() {
        let world = test_world();
        let (path, mut bytes, _) = fixture(world.path());
        let dim = DimensionId::parse("overworld").unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        let original_entry = bytes[0..4].to_vec();
        bytes[4..8].copy_from_slice(&original_entry);
        fs::write(&path, &bytes).unwrap();
        assert!(
            guard
                .rewrite_chunk_losslessly(&dim, 0, 0)
                .unwrap_err()
                .to_string()
                .contains("overlapping")
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(!world.path().join(".minecraft-native-backups").exists());
        bytes[4..8].copy_from_slice(&((3u32 << 8) | 1).to_be_bytes());
        bytes[3 * SECTOR_SIZE as usize + 4] = 99;
        fs::write(&path, &bytes).unwrap();
        assert!(
            guard
                .rewrite_chunk_losslessly(&dim, 0, 0)
                .unwrap_err()
                .to_string()
                .contains("unsupported chunk compression")
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(!world.path().join(".minecraft-native-backups").exists());
    }

    #[test]
    fn legacy_shaped_palette_is_not_written_as_26_3() {
        let world = test_world();
        let (path, mut bytes, _) = fixture(world.path());
        let mut target = chunk(0, "target");
        let Tag::Compound(fields) = &mut target.value else {
            unreachable!()
        };
        let sections = fields
            .iter_mut()
            .find(|(name, _)| name.matches("sections"))
            .unwrap();
        sections.1 = Tag::List {
            element_type: TagType::Compound,
            values: vec![Tag::Compound(vec![
                ("Y".into(), Tag::Byte(0)),
                (
                    "block_states".into(),
                    Tag::Compound(vec![(
                        "palette".into(),
                        Tag::List {
                            element_type: TagType::Compound,
                            values: vec![Tag::Compound(vec![(
                                "Name".into(),
                                Tag::String("minecraft:stone".into()),
                            )])],
                        },
                    )]),
                ),
            ])],
        };
        let compressed = compressed(&target);
        let start = 2 * SECTOR_SIZE as usize;
        bytes[start..start + SECTOR_SIZE as usize].fill(0);
        bytes[start..start + 4].copy_from_slice(&((compressed.len() + 1) as u32).to_be_bytes());
        bytes[start + 4] = 2;
        bytes[start + 5..start + 5 + compressed.len()].copy_from_slice(&compressed);
        fs::write(&path, &bytes).unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        let error = guard
            .rewrite_chunk_losslessly(&DimensionId::parse("overworld").unwrap(), 0, 0)
            .unwrap_err();
        assert!(format!("{error:#}").contains("legacy block-state format"));
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(!world.path().join(".minecraft-native-backups").exists());
    }

    #[test]
    fn duplicate_biome_or_light_fields_leave_region_untouched() {
        for name in ["biomes", "BlockLight", "SkyLight", "palette", "data"] {
            let world = test_world();
            let (path, mut bytes, _) = fixture(world.path());
            let mut biome_fields = vec![
                (
                    "palette".into(),
                    Tag::List {
                        element_type: TagType::String,
                        values: vec![Tag::String("minecraft:plains".into())],
                    },
                ),
                ("data".into(), Tag::LongArray(vec![0])),
            ];
            if matches!(name, "palette" | "data") {
                biome_fields.push(
                    biome_fields
                        .iter()
                        .find(|(key, _): &&(crate::nbt::JavaString, Tag)| key.matches(name))
                        .unwrap()
                        .clone(),
                );
            }
            let mut section = vec![
                ("Y".into(), Tag::Byte(0)),
                ("biomes".into(), Tag::Compound(biome_fields)),
                ("BlockLight".into(), Tag::ByteArray(vec![0; 2048])),
                ("SkyLight".into(), Tag::ByteArray(vec![0; 2048])),
            ];
            if !matches!(name, "palette" | "data") {
                section.push(
                    section
                        .iter()
                        .find(|(key, _): &&(crate::nbt::JavaString, Tag)| key.matches(name))
                        .unwrap()
                        .clone(),
                );
            }
            let mut target = chunk(0, "target");
            let Tag::Compound(fields) = &mut target.value else {
                unreachable!()
            };
            fields
                .iter_mut()
                .find(|(key, _)| key.matches("sections"))
                .unwrap()
                .1 = Tag::List {
                element_type: TagType::Compound,
                values: vec![Tag::Compound(section)],
            };
            let payload = compressed(&target);
            let start = 2 * SECTOR_SIZE as usize;
            bytes[start..start + SECTOR_SIZE as usize].fill(0);
            bytes[start..start + 4].copy_from_slice(&((payload.len() + 1) as u32).to_be_bytes());
            bytes[start + 4] = 2;
            bytes[start + 5..start + 5 + payload.len()].copy_from_slice(&payload);
            fs::write(&path, &bytes).unwrap();
            let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
            let error = guard
                .rewrite_chunk_losslessly(&DimensionId::parse("overworld").unwrap(), 0, 0)
                .unwrap_err();
            assert!(
                format!("{error:#}").contains("duplicate"),
                "{name}: {error:#}"
            );
            assert_eq!(fs::read(&path).unwrap(), bytes, "{name}");
            assert!(!world.path().join(".minecraft-native-backups").exists());
        }
    }

    #[test]
    fn missing_external_chunk_and_noncanonical_nbt_stop_without_a_save() {
        let world = test_world();
        let (path, mut bytes, outside) = fixture(world.path());
        let dim = DimensionId::parse("overworld").unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        let external_path = path.parent().unwrap().join("c.2.0.mcc");
        fs::remove_file(&external_path).unwrap();
        assert!(
            format!(
                "{:#}",
                guard.rewrite_chunk_losslessly(&dim, 0, 0).unwrap_err()
            )
            .contains("external chunk is missing")
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
        fs::write(external_path, outside).unwrap();

        let mut raw = nbt::write(&chunk(0, "\0")).unwrap();
        let string = raw
            .windows(4)
            .rposition(|part| part == [0, 2, 0xc0, 0x80])
            .unwrap();
        raw.splice(string..string + 4, [0, 1, 0]);
        let start = 2 * SECTOR_SIZE as usize;
        bytes[start..start + SECTOR_SIZE as usize].fill(0);
        bytes[start..start + 4].copy_from_slice(&((raw.len() + 1) as u32).to_be_bytes());
        bytes[start + 4] = 3;
        bytes[start + 5..start + 5 + raw.len()].copy_from_slice(&raw);
        fs::write(&path, &bytes).unwrap();
        assert!(
            format!(
                "{:#}",
                guard.rewrite_chunk_losslessly(&dim, 0, 0).unwrap_err()
            )
            .contains("reserialized without changing its bytes")
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(!world.path().join(".minecraft-native-backups").exists());
    }

    #[test]
    fn rewrite_allocates_new_sectors_when_recompressed_chunk_grows() {
        let world = test_world();
        let (path, mut region, _) = fixture(world.path());
        let mut target = chunk(0, "target");
        let Tag::Compound(entries) = &mut target.value else {
            unreachable!()
        };
        entries.push(("random_data".into(), Tag::ByteArray(Vec::new())));
        let filler = 4091 - nbt::write(&target).unwrap().len();
        let Tag::Compound(entries) = &mut target.value else {
            unreachable!()
        };
        let mut seed = 0x913a12d5u32;
        let data = (0..filler)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as u8
            })
            .collect();
        entries.last_mut().unwrap().1 = Tag::ByteArray(data);
        let raw = nbt::write(&target).unwrap();
        assert_eq!(raw.len(), 4091);
        assert!(compressed(&target).len() + 5 > SECTOR_SIZE as usize);
        let start = 2 * SECTOR_SIZE as usize;
        region[start..start + SECTOR_SIZE as usize].fill(0);
        region[start..start + 4].copy_from_slice(&((raw.len() + 1) as u32).to_be_bytes());
        region[start + 4] = 3;
        region[start + 5..start + 5 + raw.len()].copy_from_slice(&raw);
        fs::write(&path, &region).unwrap();
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        let result = guard
            .rewrite_chunk_losslessly(&DimensionId::parse("overworld").unwrap(), 0, 0)
            .unwrap();
        assert_eq!(
            fs::read(result.backup_directory.join("r.0.0.mca")).unwrap(),
            region
        );
        let updated = fs::read(&path).unwrap();
        assert_eq!(
            u32::from_be_bytes(updated[..4].try_into().unwrap()),
            (5 << 8) | 2
        );
        assert_eq!(&updated[4..region.len()], &region[4..]);
        assert_eq!(
            nbt::write(
                &RegionReader::open(&path)
                    .unwrap()
                    .read_chunk(0, 0)
                    .unwrap()
                    .unwrap()
            )
            .unwrap(),
            raw
        );
    }

    #[test]
    fn failed_replace_keeps_original_and_a_complete_backup() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
        let world = test_world();
        let (path, old_region, outside) = fixture(world.path());
        let mut guard = WorldWriteGuard::acquire(world.path()).unwrap();
        let blocker = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(&path)
            .unwrap();
        let error = guard
            .rewrite_chunk_losslessly(&DimensionId::parse("overworld").unwrap(), 0, 0)
            .unwrap_err();
        assert!(format!("{error:#}").contains("cannot atomically replace"));
        assert_eq!(fs::read(&path).unwrap(), old_region);
        let dirs: Vec<_> = fs::read_dir(world.path().join(".minecraft-native-backups"))
            .unwrap()
            .collect();
        assert_eq!(dirs.len(), 1);
        let backup = dirs[0].as_ref().unwrap().path();
        assert_eq!(fs::read(backup.join("r.0.0.mca")).unwrap(), old_region);
        assert_eq!(fs::read(backup.join("c.2.0.mcc")).unwrap(), outside);
        drop(blocker);
    }
}
