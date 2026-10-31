//! Read-only access to complete 26.3 chunk documents and their decoded sections.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};

use crate::{
    anvil::{self, LevelInfo, RegionReader},
    nbt::{self, JavaString, NamedTag, Tag},
    palette::{self, BlockState, ChunkSection},
};

pub const TARGET_DATA_VERSION: i32 = 5023;
const REGION_SIDE: i32 = 32;
const MAX_AREA_RADIUS: u32 = 32;
const MAX_OPEN_REGIONS: usize = 32;

/// A 26.3 world clock. Ticks advance in single precision like the Java server.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SavedClock {
    pub total_ticks: i64,
    pub partial_tick: f32,
    pub rate: f32,
    pub paused: bool,
}

impl Default for SavedClock {
    fn default() -> Self {
        Self {
            total_ticks: 0,
            partial_tick: 0.0,
            rate: 1.0,
            paused: false,
        }
    }
}

impl SavedClock {
    pub fn tick(&mut self, advance_time: bool) {
        if !advance_time || self.paused {
            return;
        }
        self.partial_tick += self.rate;
        // Mth.floor(float) first converts to int and then corrects negative fractions.
        let truncated = self.partial_tick as i32;
        let whole = if self.partial_tick < truncated as f32 {
            truncated.wrapping_sub(1)
        } else {
            truncated
        };
        self.partial_tick -= whole as f32;
        self.total_ticks = self.total_ticks.wrapping_add(i64::from(whole));
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct DimensionId {
    pub namespace: String,
    pub path: String,
}

impl DimensionId {
    pub fn parse(id: &str) -> Result<Self> {
        let (namespace, path) = id.split_once(':').unwrap_or(("minecraft", id));
        ensure!(
            !namespace.is_empty()
                && namespace
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b)),
            "invalid dimension namespace"
        );
        ensure!(
            !path.is_empty()
                && path
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_./-".contains(&b)),
            "invalid dimension path"
        );
        ensure!(
            path.split('/')
                .all(|part| !part.is_empty() && part != "." && part != ".."),
            "invalid dimension path component"
        );
        Ok(Self {
            namespace: namespace.to_owned(),
            path: path.to_owned(),
        })
    }

    pub(crate) fn region_dir(&self, world: &Path) -> PathBuf {
        world
            .join("dimensions")
            .join(&self.namespace)
            .join(&self.path)
            .join("region")
    }

    pub(crate) fn entity_region_dir(&self, world: &Path) -> PathBuf {
        world
            .join("dimensions")
            .join(&self.namespace)
            .join(&self.path)
            .join("entities")
    }

    pub(crate) fn poi_region_dir(&self, world: &Path) -> PathBuf {
        world
            .join("dimensions")
            .join(&self.namespace)
            .join(&self.path)
            .join("poi")
    }
}

/// The 26.3 `LevelData.RespawnData` value stored in `level.dat` and player data.
#[derive(Clone, Debug, PartialEq)]
pub struct RespawnData {
    pub dimension: DimensionId,
    pub position: [i32; 3],
    pub yaw: f32,
    pub pitch: f32,
}

impl Default for RespawnData {
    fn default() -> Self {
        Self {
            dimension: DimensionId {
                namespace: "minecraft".into(),
                path: "overworld".into(),
            },
            position: [0, 0, 0],
            yaw: 0.0,
            pitch: 0.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlayerRespawnConfig {
    pub data: RespawnData,
    pub forced: bool,
}

/// A dimension's type stored in 26.3's world-generation saved data. Inline
/// definitions are retained as NBT until their full codec is supported.
#[derive(Clone, Debug, PartialEq)]
pub enum SavedDimensionType {
    Named(DimensionId),
    Inline(Tag),
}

/// The per-dimension world border saved by Java 26.3. Lerp time is measured
/// in server ticks and starts from `size` when the dimension is loaded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SavedWorldBorder {
    pub center_x: f64,
    pub center_z: f64,
    pub damage_per_block: f64,
    pub safe_zone: f64,
    pub warning_blocks: i32,
    pub warning_time: i32,
    pub size: f64,
    pub lerp_time: i64,
    pub lerp_target: f64,
}

impl Default for SavedWorldBorder {
    fn default() -> Self {
        Self {
            center_x: 0.0,
            center_z: 0.0,
            damage_per_block: 0.2,
            safe_zone: 5.0,
            warning_blocks: 5,
            warning_time: 300,
            size: 59_999_968.0,
            lerp_time: 0,
            lerp_target: 0.0,
        }
    }
}

fn read_respawn_data(tag: &Tag) -> Result<RespawnData> {
    ensure!(
        matches!(tag, Tag::Compound(_)),
        "respawn data must be a compound"
    );
    let Some(Tag::String(dimension)) = tag.get("dimension") else {
        bail!("respawn dimension must be a string");
    };
    let dimension = String::from_utf16(&dimension.0).context("invalid respawn dimension")?;
    let coordinates = match tag.get("pos") {
        Some(Tag::IntArray(values)) => values.clone(),
        Some(Tag::ByteArray(values)) => values.iter().map(|v| i32::from(*v as i8)).collect(),
        Some(Tag::LongArray(values)) => values.iter().map(|v| *v as i32).collect(),
        Some(Tag::List { values, .. }) => values
            .iter()
            .map(|value| {
                value
                    .as_java_int()
                    .context("respawn position must contain numbers")
            })
            .collect::<Result<Vec<_>>>()?,
        _ => bail!("respawn position must be an integer sequence"),
    };
    let position: [i32; 3] = coordinates
        .try_into()
        .map_err(|_| anyhow::anyhow!("respawn position must contain three numbers"))?;
    let angle = |key: &str, min: f32, max: f32| -> Result<f32> {
        let value = match tag.get(key) {
            Some(Tag::Byte(value)) => *value as f32,
            Some(Tag::Short(value)) => *value as f32,
            Some(Tag::Int(value)) => *value as f32,
            Some(Tag::Long(value)) => *value as f32,
            Some(Tag::Float(value)) => *value,
            Some(Tag::Double(value)) => *value as f32,
            _ => bail!("respawn {key} must be a number"),
        };
        ensure!(
            value.is_finite() && (min..=max).contains(&value),
            "respawn {key} is out of range"
        );
        Ok(value)
    };
    Ok(RespawnData {
        dimension: DimensionId::parse(&dimension)?,
        position,
        yaw: angle("yaw", -180.0, 180.0)?,
        pitch: angle("pitch", -90.0, 90.0)?,
    })
}

/// Decode the player's optional respawn configuration without changing the player document.
pub fn read_player_respawn(player: &NamedTag) -> Result<Option<PlayerRespawnConfig>> {
    let Some(tag) = player.value.get("respawn") else {
        return Ok(None);
    };
    let data = read_respawn_data(tag)?;
    let forced = match tag.get("forced") {
        None => false,
        Some(value) => nbt_bool(value).context("respawn forced must be a boolean")?,
    };
    Ok(Some(PlayerRespawnConfig { data, forced }))
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ChunkPos {
    pub x: i32,
    pub z: i32,
}

/// A complete entity-region document. Unknown entity and passenger data are
/// kept in their original form; this does not yet instantiate game entities.
#[derive(Clone)]
pub struct SavedEntityChunk {
    pub position: ChunkPos,
    pub document: Arc<NamedTag>,
}

/// A decoded POI region entry with its complete original NBT retained.
/// This does not reconcile POIs with blocks or authorize writing a world.
#[derive(Clone)]
pub struct SavedPoiChunk {
    pub position: ChunkPos,
    pub document: Arc<NamedTag>,
    pub sections: BTreeMap<i32, SavedPoiSection>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedPoiSection {
    pub valid: bool,
    pub records: Vec<SavedPoiRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedPoiRecord {
    pub position: [i32; 3],
    pub type_id: JavaString,
    pub free_tickets: i32,
}

impl SavedEntityChunk {
    pub fn entities(&self) -> &[Tag] {
        match self.document.value.get("Entities") {
            Some(Tag::List { values, .. }) => values,
            _ => &[],
        }
    }
}

#[derive(Clone)]
pub struct LoadedChunk {
    pub position: ChunkPos,
    pub data_version: i32,
    pub status: Option<JavaString>,
    /// Original data, including block entities, ticks, structures and unrecognized tags.
    pub document: Arc<NamedTag>,
    pub sections: Vec<ChunkSection>,
}

impl LoadedChunk {
    pub fn is_full(&self) -> bool {
        self.status
            .as_ref()
            .is_some_and(|status| status.matches("minecraft:full"))
    }

    /// Java writes isLightOn when the saved chunk's light is complete. Treat
    /// malformed or duplicate markers as unknown rather than assuming darkness.
    pub fn block_light_correct(&self) -> bool {
        let Tag::Compound(fields) = &self.document.value else {
            return false;
        };
        let mut markers = fields.iter().filter(|(name, _)| name.matches("isLightOn"));
        matches!(markers.next(), Some((_, Tag::Byte(1)))) && markers.next().is_none()
    }

    /// Returns None for absent block-state data; that is distinct from minecraft:air.
    pub fn block(&self, x: i32, y: i32, z: i32) -> Option<&BlockState> {
        if x.div_euclid(16) != self.position.x || z.div_euclid(16) != self.position.z {
            return None;
        }
        let section_y = i8::try_from(y.div_euclid(16)).ok()?;
        let section = self
            .sections
            .iter()
            .find(|section| section.y == section_y)?;
        section.blocks.as_ref()?.block(
            x.rem_euclid(16) as usize,
            y.rem_euclid(16) as usize,
            z.rem_euclid(16) as usize,
        )
    }

    pub fn biome(&self, x: i32, y: i32, z: i32) -> Option<&JavaString> {
        if x.div_euclid(16) != self.position.x || z.div_euclid(16) != self.position.z {
            return None;
        }
        let section_y = i8::try_from(y.div_euclid(16)).ok()?;
        let section = self
            .sections
            .iter()
            .find(|section| section.y == section_y)?;
        section.biomes.as_ref()?.biome(
            (x.rem_euclid(16) / 4) as usize,
            (y.rem_euclid(16) / 4) as usize,
            (z.rem_euclid(16) / 4) as usize,
        )
    }

    pub fn light(&self, x: i32, y: i32, z: i32, sky: bool) -> Option<u8> {
        if x.div_euclid(16) != self.position.x || z.div_euclid(16) != self.position.z {
            return None;
        }
        let section_y = i8::try_from(y.div_euclid(16)).ok()?;
        let section = self
            .sections
            .iter()
            .find(|section| section.y == section_y)?;
        let data = if sky {
            section.sky_light.as_ref()?
        } else {
            section.block_light.as_ref()?
        };
        let index = x.rem_euclid(16) as usize
            + z.rem_euclid(16) as usize * 16
            + y.rem_euclid(16) as usize * 256;
        let value = *data.get(index / 2)?;
        Some(if index.is_multiple_of(2) {
            value & 0x0f
        } else {
            value >> 4
        })
    }
}

pub struct ChunkArea {
    pub center: ChunkPos,
    pub radius: u32,
    pub chunks: BTreeMap<ChunkPos, Arc<LoadedChunk>>,
    /// Missing regions and ungenerated chunks are reported explicitly.
    pub missing: Vec<ChunkPos>,
}

impl ChunkArea {
    /// Convert decoded palettes into interned runtime identities without replacing
    /// unknown block states, biomes, missing chunks, or absent section data.
    pub fn materialize(&self, dimension: &DimensionId) -> Result<native_world::loaded::World> {
        self.materialize_with_defaults(dimension, &BTreeMap::new())
    }

    /// Resolve string-encoded default states using locally prepared registry data.
    /// Unknown identifiers remain unchanged, and explicit properties take precedence.
    pub fn materialize_with_defaults(
        &self,
        dimension: &DimensionId,
        defaults: &BTreeMap<String, BTreeMap<String, String>>,
    ) -> Result<native_world::loaded::World> {
        use native_world::loaded::{self, BIOME_COUNT, BLOCK_COUNT, LIGHT_BYTES};

        let mut world = loaded::World::new(format!("{}:{}", dimension.namespace, dimension.path));
        for pos in &self.missing {
            world
                .missing_chunks
                .insert(loaded::ChunkPos { x: pos.x, z: pos.z });
        }
        for (pos, chunk) in &self.chunks {
            let mut sections = BTreeMap::new();
            for section in &chunk.sections {
                let blocks = match &section.blocks {
                    None => None,
                    Some(palette) => {
                        let mut ids = Vec::with_capacity(palette.states.len());
                        for state in &palette.states {
                            let name = String::from_utf16(&state.name.0)
                                .context("block identifier contains invalid UTF-16")?;
                            let mut properties = BTreeMap::new();
                            for (key, value) in &state.properties {
                                let key = String::from_utf16(&key.0)
                                    .context("block property name contains invalid UTF-16")?;
                                let value = String::from_utf16(&value.0)
                                    .context("block property value contains invalid UTF-16")?;
                                properties.insert(key, value);
                            }
                            if properties.is_empty()
                                && let Some(default) = defaults.get(&name)
                            {
                                properties.clone_from(default);
                            }
                            ids.push(world.intern_state(loaded::BlockState { name, properties }));
                        }
                        let indices = palette
                            .indices
                            .iter()
                            .map(|&index| {
                                ids.get(index as usize)
                                    .copied()
                                    .context("block palette index out of range")
                            })
                            .collect::<Result<Vec<_>>>()?;
                        let array: Box<[_; BLOCK_COUNT]> = indices
                            .into_boxed_slice()
                            .try_into()
                            .map_err(|_| anyhow::anyhow!("invalid block section length"))?;
                        Some(array)
                    }
                };
                let biomes = match &section.biomes {
                    None => None,
                    Some(palette) => {
                        let mut ids = Vec::with_capacity(palette.names.len());
                        for biome in &palette.names {
                            ids.push(
                                world.intern_biome(
                                    String::from_utf16(&biome.0)
                                        .context("biome identifier contains invalid UTF-16")?,
                                ),
                            );
                        }
                        let indices = palette
                            .indices
                            .iter()
                            .map(|&index| {
                                ids.get(index as usize)
                                    .copied()
                                    .context("biome palette index out of range")
                            })
                            .collect::<Result<Vec<_>>>()?;
                        let array: Box<[_; BIOME_COUNT]> = indices
                            .into_boxed_slice()
                            .try_into()
                            .map_err(|_| anyhow::anyhow!("invalid biome section length"))?;
                        Some(array)
                    }
                };
                let light = |values: &Option<Vec<u8>>| -> Result<Option<Box<[u8; LIGHT_BYTES]>>> {
                    values
                        .as_ref()
                        .map(|values| {
                            values
                                .clone()
                                .into_boxed_slice()
                                .try_into()
                                .map_err(|_| anyhow::anyhow!("invalid section light length"))
                        })
                        .transpose()
                };
                sections.insert(
                    section.y,
                    Arc::new(loaded::Section {
                        blocks,
                        biomes,
                        block_light: light(&section.block_light)?,
                        sky_light: light(&section.sky_light)?,
                    }),
                );
            }
            let status = chunk
                .status
                .as_ref()
                .map(|value| {
                    String::from_utf16(&value.0).context("chunk status contains invalid UTF-16")
                })
                .transpose()?;
            let position = loaded::ChunkPos { x: pos.x, z: pos.z };
            world.chunks.insert(
                position,
                Arc::new(loaded::Chunk {
                    block_light_correct: chunk.block_light_correct(),
                    position,
                    status,
                    sections,
                }),
            );
            let ticks = saved_fluid_ticks(&chunk.document.value, position);
            if !ticks.is_empty() {
                world.saved_fluid_ticks.insert(position, ticks);
            }
            let ticks = saved_block_ticks(&chunk.document.value, position);
            if !ticks.is_empty() {
                world.saved_block_ticks.insert(position, ticks);
            }
            world
                .saved_block_entities
                .insert(position, saved_block_entity_payload(&chunk.document.value));
        }
        Ok(world)
    }
}

fn saved_block_entity_payload(chunk: &Tag) -> native_world::loaded::SavedBlockEntities {
    use native_world::loaded::SavedBlockEntities;
    let Tag::Compound(fields) = chunk else {
        return SavedBlockEntities::Unavailable;
    };
    let mut entries = fields
        .iter()
        .filter(|(name, _)| name.matches("block_entities"));
    let first = entries.next();
    if entries.next().is_some() {
        return SavedBlockEntities::Unavailable;
    }
    match first.map(|(_, tag)| tag) {
        None => SavedBlockEntities::Empty,
        Some(Tag::List {
            element_type,
            values,
        }) if values.is_empty() => {
            let _ = element_type;
            SavedBlockEntities::Empty
        }
        Some(
            tag @ Tag::List {
                element_type: crate::nbt::TagType::Compound,
                ..
            },
        ) => {
            match crate::nbt::write(&crate::nbt::NamedTag {
                name: "".into(),
                value: tag.clone(),
            }) {
                Ok(bytes) => SavedBlockEntities::Encoded(bytes.into()),
                Err(_) => SavedBlockEntities::Unavailable,
            }
        }
        _ => SavedBlockEntities::Unavailable,
    }
}

fn saved_block_ticks(
    document: &Tag,
    chunk: native_world::loaded::ChunkPos,
) -> Vec<native_world::loaded::SavedBlockTick> {
    use native_world::loaded::SavedBlockTick;
    let Some(Tag::List { values, .. }) = document.get("block_ticks") else {
        return Vec::new();
    };
    values
        .iter()
        .filter_map(|entry| {
            let Some(Tag::String(block)) = entry.get("i") else {
                return None;
            };
            let block = String::from_utf16(&block.0).ok()?;
            let integer = |name| entry.get(name).and_then(Tag::as_java_int);
            let (x, y, z) = (integer("x")?, integer("y")?, integer("z")?);
            if x.div_euclid(16) != chunk.x || z.div_euclid(16) != chunk.z {
                return None;
            }
            Some(SavedBlockTick {
                block,
                position: [x, y, z],
                delay: integer("t")?,
                priority: integer("p")?.clamp(-3, 3),
            })
        })
        .collect()
}

/// Decode the subset of scheduled ticks with complete vanilla fields. The
/// original NBT remains available even when an entry is unsupported.
fn saved_fluid_ticks(
    document: &Tag,
    chunk: native_world::loaded::ChunkPos,
) -> Vec<native_world::loaded::SavedFluidTick> {
    use native_world::loaded::SavedFluidTick;
    let Some(Tag::List { values, .. }) = document.get("fluid_ticks") else {
        return Vec::new();
    };
    values
        .iter()
        .filter_map(|entry| {
            let Some(Tag::String(fluid)) = entry.get("i") else {
                return None;
            };
            let fluid = String::from_utf16(&fluid.0).ok()?;
            let integer = |name| entry.get(name).and_then(Tag::as_java_int);
            let (x, y, z) = (integer("x")?, integer("y")?, integer("z")?);
            if x.div_euclid(16) != chunk.x || z.div_euclid(16) != chunk.z {
                return None;
            }
            Some(SavedFluidTick {
                fluid,
                position: [x, y, z],
                delay: integer("t")?,
                priority: integer("p")?.clamp(-3, 3),
            })
        })
        .collect()
}

pub struct WorldReader {
    pub info: LevelInfo,
    pub level: Arc<NamedTag>,
    dimension: DimensionId,
    root: PathBuf,
    session_guard: Option<crate::save::WorldSessionGuard>,
    regions: BTreeMap<(i32, i32), RegionReader>,
    region_usage: VecDeque<(i32, i32)>,
    entity_regions: BTreeMap<(i32, i32), RegionReader>,
    entity_region_usage: VecDeque<(i32, i32)>,
    poi_regions: BTreeMap<(i32, i32), RegionReader>,
    poi_region_usage: VecDeque<(i32, i32)>,
}

/// The saved pack order and feature flags, before discovery of newly added packs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataPackSettings {
    pub enabled: Vec<String>,
    pub disabled: Vec<String>,
    pub enabled_features: Vec<String>,
}

/// The 26.3 game rules currently consumed by the in-memory simulation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FluidGameRules {
    pub random_tick_speed: i32,
    pub advance_time: bool,
    pub water_source_conversion: bool,
    pub lava_source_conversion: bool,
    pub block_drops: bool,
    pub entity_drops: bool,
    pub natural_health_regeneration: bool,
    pub fall_damage: bool,
    pub keep_inventory: bool,
    pub immediate_respawn: bool,
    pub show_death_messages: bool,
    pub respawn_radius: i32,
}

impl Default for FluidGameRules {
    fn default() -> Self {
        Self {
            random_tick_speed: 3,
            advance_time: true,
            water_source_conversion: true,
            lava_source_conversion: false,
            block_drops: true,
            entity_drops: true,
            natural_health_regeneration: true,
            fall_damage: true,
            keep_inventory: false,
            immediate_respawn: false,
            show_death_messages: true,
            respawn_radius: 10,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Difficulty {
    Peaceful,
    Easy,
    #[default]
    Normal,
    Hard,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DifficultySettings {
    pub difficulty: Difficulty,
    pub hardcore: bool,
    pub locked: bool,
}

impl Default for DataPackSettings {
    fn default() -> Self {
        Self {
            enabled: vec!["vanilla".into()],
            disabled: Vec::new(),
            enabled_features: vec!["minecraft:vanilla".into()],
        }
    }
}

/// NbtOps converts any numeric tag to a boolean by comparing its value to zero.
fn nbt_bool(tag: &Tag) -> Option<bool> {
    Some(match tag {
        Tag::Byte(value) => *value != 0,
        Tag::Short(value) => *value != 0,
        Tag::Int(value) => *value != 0,
        Tag::Long(value) => *value != 0,
        Tag::Float(value) => *value != 0.0,
        Tag::Double(value) => *value != 0.0,
        _ => return None,
    })
}

fn nbt_double(tag: &Tag) -> Option<f64> {
    match tag {
        Tag::Byte(value) => Some(f64::from(*value)),
        Tag::Short(value) => Some(f64::from(*value)),
        Tag::Int(value) => Some(f64::from(*value)),
        Tag::Long(value) => Some(*value as f64),
        Tag::Float(value) => Some(f64::from(*value)),
        Tag::Double(value) => Some(*value),
        _ => None,
    }
}

fn nbt_long(tag: &Tag) -> Option<i64> {
    match tag {
        Tag::Byte(value) => Some(i64::from(*value)),
        Tag::Short(value) => Some(i64::from(*value)),
        Tag::Int(value) => Some(i64::from(*value)),
        Tag::Long(value) => Some(*value),
        Tag::Float(value) => Some(*value as i64),
        Tag::Double(value) => Some(*value as i64),
        _ => None,
    }
}

fn clock_long(tag: &Tag) -> Option<i64> {
    match tag {
        Tag::Byte(value) => Some(i64::from(*value)),
        Tag::Short(value) => Some(i64::from(*value)),
        Tag::Int(value) => Some(i64::from(*value)),
        Tag::Long(value) => Some(*value),
        Tag::Float(value) => Some(*value as i64),
        Tag::Double(value) => Some(*value as i64),
        _ => None,
    }
}

fn clock_float(tag: &Tag) -> Option<f32> {
    match tag {
        Tag::Byte(value) => Some(f32::from(*value)),
        Tag::Short(value) => Some(f32::from(*value)),
        Tag::Int(value) => Some(*value as f32),
        Tag::Long(value) => Some(*value as f32),
        Tag::Float(value) => Some(*value),
        Tag::Double(value) => Some(*value as f32),
        _ => None,
    }
}

/// Decode a captured world-clock document without reopening the world file.
pub fn parse_world_clocks(root: &NamedTag) -> Result<BTreeMap<DimensionId, SavedClock>> {
    match root.value.get("DataVersion") {
        Some(Tag::Int(TARGET_DATA_VERSION)) => {}
        Some(Tag::Int(version)) => {
            bail!("unsupported world clocks DataVersion {version}; expected {TARGET_DATA_VERSION}")
        }
        _ => bail!("world clocks document is missing integer DataVersion"),
    }
    let Some(Tag::Compound(entries)) = root.value.get("data") else {
        bail!("world clocks data must be a compound");
    };
    let mut clocks = BTreeMap::new();
    for (name, entry) in entries {
        let name = String::from_utf16(&name.0).context("invalid clock identifier")?;
        let id = DimensionId::parse(&name).context("invalid clock identifier")?;
        ensure!(
            matches!(entry, Tag::Compound(_)),
            "clock {name} must be a compound"
        );
        let total_ticks = entry
            .get("total_ticks")
            .and_then(clock_long)
            .with_context(|| format!("clock {name} is missing total_ticks"))?;
        let partial_tick = match entry.get("partial_tick") {
            None => 0.0,
            Some(value) => clock_float(value).context("invalid partial_tick")?,
        };
        let rate = match entry.get("rate") {
            None => 1.0,
            Some(value) => clock_float(value).context("invalid clock rate")?,
        };
        ensure!(rate > 0.0, "clock {name} rate must be positive");
        let paused = match entry.get("paused") {
            None => false,
            Some(value) => nbt_bool(value).context("invalid clock paused state")?,
        };
        ensure!(
            clocks
                .insert(
                    id,
                    SavedClock {
                        total_ticks,
                        partial_tick,
                        rate,
                        paused,
                    }
                )
                .is_none(),
            "duplicate clock {name}"
        );
    }
    Ok(clocks)
}

fn replace_clock_field(fields: &mut Vec<(JavaString, Tag)>, name: &str, value: Tag) {
    fields.retain(|(key, _)| !key.matches(name));
    fields.push((JavaString::from(name), value));
}

fn string_list(tag: &Tag, name: &str) -> Result<Vec<String>> {
    let Tag::List {
        element_type,
        values,
    } = tag
    else {
        bail!("{name} must be a list of strings");
    };
    ensure!(
        *element_type == nbt::TagType::String
            || (*element_type == nbt::TagType::End && values.is_empty()),
        "{name} must be a list of strings"
    );
    values
        .iter()
        .map(|value| match value {
            Tag::String(value) => String::from_utf16(&value.0)
                .with_context(|| format!("{name} contains invalid UTF-16")),
            _ => bail!("{name} must contain strings"),
        })
        .collect()
}

impl WorldReader {
    pub fn dimension(&self) -> &DimensionId {
        &self.dimension
    }

    /// Region handles are specific to a dimension, even when region coordinates match.
    pub fn set_dimension(&mut self, dimension: DimensionId) {
        if self.dimension != dimension {
            self.dimension = dimension;
            self.regions.clear();
            self.region_usage.clear();
            self.entity_regions.clear();
            self.entity_region_usage.clear();
            self.poi_regions.clear();
            self.poi_region_usage.clear();
        }
    }
    /// Inspect a world without acquiring the Java session lock. This is only
    /// suitable for callers that already control concurrent access.
    pub fn open(world_dir: &Path, dimension: DimensionId) -> Result<Self> {
        Self::open_inner(world_dir, dimension, false)
    }

    /// Open a playable world while holding the same exclusive lock as Java.
    /// The guard can be shared with background readers and the game session.
    pub fn open_for_session(world_dir: &Path, dimension: DimensionId) -> Result<Self> {
        Self::open_inner(world_dir, dimension, true)
    }

    fn open_inner(world_dir: &Path, dimension: DimensionId, locked: bool) -> Result<Self> {
        let root = world_dir
            .canonicalize()
            .with_context(|| format!("cannot open world directory {}", world_dir.display()))?;
        ensure!(root.is_dir(), "world path is not a directory");
        let session_guard = if locked {
            Some(crate::save::WorldSessionGuard::acquire(&root)?)
        } else {
            None
        };
        crate::save_transaction::ensure_no_pending(&root)?;
        let (info, level) = anvil::read_level_document(&root)?;
        ensure!(
            info.data_version == TARGET_DATA_VERSION,
            "unsupported world DataVersion {}; expected {TARGET_DATA_VERSION}",
            info.data_version
        );
        Ok(Self {
            info,
            level: Arc::new(level),
            dimension,
            root,
            session_guard,
            regions: BTreeMap::new(),
            region_usage: VecDeque::new(),
            entity_regions: BTreeMap::new(),
            entity_region_usage: VecDeque::new(),
            poi_regions: BTreeMap::new(),
            poi_region_usage: VecDeque::new(),
        })
    }

    pub fn session_guard(&self) -> Option<crate::save::WorldSessionGuard> {
        self.session_guard.clone()
    }

    pub fn singleplayer_uuid_words(&self) -> Result<Option<[i32; 4]>> {
        let data = self
            .level
            .value
            .get("Data")
            .context("level.dat is missing Data")?;
        let Some(uuid) = data.get("singleplayer_uuid") else {
            return Ok(None);
        };
        let parts = match uuid {
            Tag::IntArray(parts) if parts.len() == 4 => [parts[0], parts[1], parts[2], parts[3]],
            _ => bail!("singleplayer_uuid must contain four integers"),
        };
        Ok(Some(parts))
    }

    pub fn singleplayer_uuid(&self) -> Result<Option<String>> {
        let Some(parts) = self.singleplayer_uuid_words()? else {
            return Ok(None);
        };
        let mut bytes = [0u8; 16];
        for (i, part) in parts.iter().enumerate() {
            bytes[i * 4..i * 4 + 4].copy_from_slice(&part.to_be_bytes());
        }
        let hex = format!("{:032x}", u128::from_be_bytes(bytes));
        Ok(Some(format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )))
    }

    /// Read 26.3's world spawn. Missing data uses the game's default; malformed
    /// data is reported so it cannot silently select an unsafe respawn location.
    pub fn world_spawn(&self) -> Result<RespawnData> {
        let data = self
            .level
            .value
            .get("Data")
            .context("level.dat is missing Data")?;
        match data.get("spawn") {
            Some(spawn) => read_respawn_data(spawn).context("invalid world spawn"),
            None => Ok(RespawnData::default()),
        }
    }

    /// Read the dimension/type mappings in the shared 26.3 world-generation
    /// saved data. Active data-pack dimension definitions take precedence when
    /// Java bakes its dimension registry; the caller must apply that layer.
    /// A missing file is distinct from a missing dimension entry.
    pub fn saved_dimension_types(
        &self,
    ) -> Result<Option<BTreeMap<DimensionId, SavedDimensionType>>> {
        let path = self.root.join("data/minecraft/world_gen_settings.dat");
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("cannot open {}", path.display()));
            }
        };
        let bytes = nbt::decompress_limited(file)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let document = if bytes.starts_with(&[0x1f, 0x8b]) {
            nbt::read_gzip(bytes.as_slice())
        } else {
            nbt::read(&bytes)
        }
        .with_context(|| format!("invalid {}", path.display()))?;
        match document.value.get("DataVersion") {
            Some(Tag::Int(TARGET_DATA_VERSION)) => {}
            Some(Tag::Int(version)) => bail!(
                "unsupported world generation DataVersion {version}; expected {TARGET_DATA_VERSION}"
            ),
            _ => bail!("{} is missing integer DataVersion", path.display()),
        }
        let dimensions = document
            .value
            .get("data")
            .and_then(|data| data.get("dimensions"))
            .context("world generation data is missing dimensions")?;
        let Tag::Compound(entries) = dimensions else {
            bail!("world generation dimensions must be a compound");
        };
        let mut result = BTreeMap::new();
        for (name, stem) in entries {
            let id = String::from_utf16(&name.0).context("invalid dimension identifier")?;
            let id = DimensionId::parse(&id)?;
            ensure!(
                matches!(stem, Tag::Compound(_)),
                "dimension definition must be a compound"
            );
            ensure!(
                matches!(stem.get("generator"), Some(Tag::Compound(_))),
                "dimension generator is missing or invalid"
            );
            let dimension_type = match stem.get("type") {
                Some(Tag::String(name)) => {
                    let name =
                        String::from_utf16(&name.0).context("invalid dimension type identifier")?;
                    SavedDimensionType::Named(DimensionId::parse(&name)?)
                }
                Some(value @ Tag::Compound(_)) => SavedDimensionType::Inline(value.clone()),
                _ => bail!("dimension type is missing or invalid"),
            };
            ensure!(
                result.insert(id, dimension_type).is_none(),
                "duplicate dimension identifier"
            );
        }
        Ok(Some(result))
    }

    /// Default legacy game type used when a saved player has no own game type.
    pub fn default_game_type_id(&self) -> Result<i32> {
        let data = self
            .level
            .value
            .get("Data")
            .context("level.dat is missing Data")?;
        Ok(data.get("GameType").and_then(Tag::as_java_int).unwrap_or(0))
    }

    /// LevelSettings.parse uses the default world game type if allowCommands
    /// is absent or is not a numeric NBT value.
    pub fn allow_commands(&self) -> Result<bool> {
        let data = self
            .level
            .value
            .get("Data")
            .context("level.dat is missing Data")?;
        let default = self.default_game_type_id()? == 1;
        Ok(data
            .get("allowCommands")
            .and_then(nbt_bool)
            .unwrap_or(default))
    }

    /// Java 26.3 reads a complete `difficulty_settings` codec value or defaults
    /// the entire setting to normal, non-hardcore and unlocked.
    pub fn difficulty_settings(&self) -> Result<DifficultySettings> {
        let data = self
            .level
            .value
            .get("Data")
            .context("level.dat is missing Data")?;
        let parsed = || -> Option<DifficultySettings> {
            let settings = data.get("difficulty_settings")?;
            let Tag::String(difficulty) = settings.get("difficulty")? else {
                return None;
            };
            let difficulty = match String::from_utf16(&difficulty.0).ok()?.as_str() {
                "peaceful" => Difficulty::Peaceful,
                "easy" => Difficulty::Easy,
                "normal" => Difficulty::Normal,
                "hard" => Difficulty::Hard,
                _ => return None,
            };
            let boolean = |name: &str| nbt_bool(settings.get(name)?);
            Some(DifficultySettings {
                difficulty,
                hardcore: boolean("hardcore")?,
                locked: boolean("locked")?,
            })
        };
        Ok(parsed().unwrap_or_default())
    }

    pub fn hardcore(&self) -> Result<bool> {
        Ok(self.difficulty_settings()?.hardcore)
    }

    /// Read this dimension's `data/minecraft/world_border.dat`. Invalid or
    /// unsupported data is an error rather than an assumed safe border.
    pub fn world_border(&self) -> Result<SavedWorldBorder> {
        let path = self
            .root
            .join("dimensions")
            .join(&self.dimension.namespace)
            .join(&self.dimension.path)
            .join("data/minecraft/world_border.dat");
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(SavedWorldBorder::default());
            }
            Err(error) => {
                return Err(error).with_context(|| format!("cannot open {}", path.display()));
            }
        };
        let bytes = nbt::decompress_limited(file)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let document = if bytes.starts_with(&[0x1f, 0x8b]) {
            nbt::read_gzip(bytes.as_slice())
        } else {
            nbt::read(&bytes)
        }
        .with_context(|| format!("invalid {}", path.display()))?;
        match document.value.get("DataVersion") {
            Some(Tag::Int(TARGET_DATA_VERSION)) => {}
            Some(Tag::Int(version)) => bail!(
                "unsupported world border DataVersion {version}; expected {TARGET_DATA_VERSION}"
            ),
            _ => bail!("{} is missing integer DataVersion", path.display()),
        }
        let data = document
            .value
            .get("data")
            .context("world border is missing data")?;
        ensure!(
            matches!(data, Tag::Compound(_)),
            "world border data must be a compound"
        );
        let double = |name: &str| -> Result<f64> {
            let value = data
                .get(name)
                .and_then(nbt_double)
                .with_context(|| format!("world border {name} must be a number"))?;
            ensure!(value.is_finite(), "world border {name} must be finite");
            Ok(value)
        };
        let integer = |name: &str| -> Result<i32> {
            data.get(name)
                .and_then(Tag::as_java_int)
                .with_context(|| format!("world border {name} must be an integer"))
        };
        let center_x = double("center_x")?;
        let center_z = double("center_z")?;
        ensure!(
            (-29_999_984.0..=29_999_984.0).contains(&center_x)
                && (-29_999_984.0..=29_999_984.0).contains(&center_z),
            "world border center is out of range"
        );
        Ok(SavedWorldBorder {
            center_x,
            center_z,
            damage_per_block: double("damage_per_block")?,
            safe_zone: double("safe_zone")?,
            warning_blocks: integer("warning_blocks")?,
            warning_time: integer("warning_time")?,
            size: double("size")?,
            lerp_time: data
                .get("lerp_time")
                .and_then(nbt_long)
                .context("world border lerp_time must be a number")?,
            lerp_target: double("lerp_target")?,
        })
    }

    /// Read the shared clock document without modifying the original file.
    pub fn world_clocks_document(&self) -> Result<Option<NamedTag>> {
        let path = self.root.join("data/minecraft/world_clocks.dat");
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("cannot open {}", path.display()));
            }
        };
        let bytes = nbt::decompress_limited(file)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let document = if bytes.starts_with(&[0x1f, 0x8b]) {
            nbt::read_gzip(bytes.as_slice())
        } else {
            nbt::read(&bytes)
        }
        .with_context(|| format!("invalid {}", path.display()))?;
        parse_world_clocks(&document)?;
        Ok(Some(document))
    }

    /// Read shared world-clock states without modifying the original file.
    /// Clocks absent from this map are initialized to `SavedClock::default()`.
    pub fn world_clocks(&self) -> Result<BTreeMap<DimensionId, SavedClock>> {
        self.world_clocks_document()?
            .map(|document| parse_world_clocks(&document))
            .unwrap_or_else(|| Ok(BTreeMap::new()))
    }

    /// Serialize updated clock states while retaining unrecognized NBT fields.
    /// The supplied map is expected to contain the complete set of loaded clocks.
    pub fn update_world_clock_document(
        source: &NamedTag,
        clocks: &BTreeMap<DimensionId, SavedClock>,
    ) -> Result<NamedTag> {
        parse_world_clocks(source)?;
        let Tag::Compound(source_fields) = &source.value else {
            unreachable!()
        };
        for key in ["DataVersion", "data"] {
            ensure!(
                source_fields
                    .iter()
                    .filter(|(name, _)| name.matches(key))
                    .count()
                    <= 1,
                "duplicate world clocks {key}"
            );
        }
        let Some(Tag::Compound(source_entries)) = source.value.get("data") else {
            unreachable!()
        };
        for (_, entry) in source_entries {
            let Tag::Compound(fields) = entry else {
                unreachable!()
            };
            for key in ["total_ticks", "partial_tick", "rate", "paused"] {
                ensure!(
                    fields.iter().filter(|(name, _)| name.matches(key)).count() <= 1,
                    "duplicate world clock {key}"
                );
            }
        }
        let mut document = source.clone();
        let Tag::Compound(root) = &mut document.value else {
            bail!("world clocks document must be a compound");
        };
        let data = root
            .iter_mut()
            .rev()
            .find(|(name, _)| name.matches("data"))
            .map(|(_, value)| value)
            .context("world clocks data is missing")?;
        let Tag::Compound(entries) = data else {
            bail!("world clocks data must be a compound");
        };
        for (id, clock) in clocks {
            ensure!(clock.rate > 0.0, "clock rate must be positive");
            let name = format!("{}:{}", id.namespace, id.path);
            let existing = entries
                .iter()
                .rev()
                .find(|(key, _)| key.matches(&name))
                .map(|(_, value)| value.clone());
            let mut fields = match existing {
                Some(Tag::Compound(fields)) => fields,
                Some(_) => bail!("clock {name} must be a compound"),
                None => Vec::new(),
            };
            replace_clock_field(&mut fields, "total_ticks", Tag::Long(clock.total_ticks));
            replace_clock_field(&mut fields, "partial_tick", Tag::Float(clock.partial_tick));
            replace_clock_field(&mut fields, "rate", Tag::Float(clock.rate));
            replace_clock_field(
                &mut fields,
                "paused",
                Tag::Byte(if clock.paused { 1 } else { 0 }),
            );
            replace_clock_field(entries, &name, Tag::Compound(fields));
        }
        parse_world_clocks(&document)?;
        Ok(document)
    }

    /// Java 26.3 stores the shared game-rule map under data/minecraft.
    pub fn fluid_game_rules(&self) -> Result<FluidGameRules> {
        let path = self.root.join("data/minecraft/game_rules.dat");
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(FluidGameRules::default());
            }
            Err(error) => {
                return Err(error).with_context(|| format!("cannot open {}", path.display()));
            }
        };
        let bytes = nbt::decompress_limited(file)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let root = if bytes.starts_with(&[0x1f, 0x8b]) {
            nbt::read_gzip(bytes.as_slice())
        } else {
            nbt::read(&bytes)
        }
        .with_context(|| format!("invalid {}", path.display()))?;
        match root.value.get("DataVersion") {
            Some(Tag::Int(TARGET_DATA_VERSION)) => {}
            Some(Tag::Int(version)) => bail!(
                "unsupported game rules DataVersion {version}; expected {TARGET_DATA_VERSION}"
            ),
            _ => bail!("{} is missing integer DataVersion", path.display()),
        }
        let data = root
            .value
            .get("data")
            .context("game_rules.dat is missing data")?;
        ensure!(
            matches!(data, Tag::Compound(_)),
            "game_rules.dat data must be a compound"
        );
        let read = |name: &str, default| -> Result<bool> {
            match data
                .get(&format!("minecraft:{name}"))
                .or_else(|| data.get(name))
            {
                None => Ok(default),
                Some(value) => {
                    nbt_bool(value).with_context(|| format!("game rule {name} must be a boolean"))
                }
            }
        };
        let respawn_radius = match data
            .get("minecraft:respawn_radius")
            .or_else(|| data.get("respawn_radius"))
        {
            None => 10,
            Some(value) => {
                let radius = value
                    .as_java_int()
                    .context("respawn_radius must be a number")?;
                ensure!(radius >= 0, "respawn_radius must not be negative");
                radius
            }
        };
        let random_tick_speed = match data
            .get("minecraft:random_tick_speed")
            .or_else(|| data.get("random_tick_speed"))
        {
            None => 3,
            Some(value) => {
                let speed = value
                    .as_java_int()
                    .context("random_tick_speed must be a number")?;
                ensure!(speed >= 0, "random_tick_speed must not be negative");
                speed
            }
        };
        Ok(FluidGameRules {
            random_tick_speed,
            advance_time: read("advance_time", true)?,
            water_source_conversion: read("water_source_conversion", true)?,
            lava_source_conversion: read("lava_source_conversion", false)?,
            block_drops: read("block_drops", true)?,
            entity_drops: read("entity_drops", true)?,
            natural_health_regeneration: read("natural_health_regeneration", true)?,
            fall_damage: read("fall_damage", true)?,
            keep_inventory: read("keep_inventory", false)?,
            immediate_respawn: read("immediate_respawn", false)?,
            show_death_messages: read("show_death_messages", true)?,
            respawn_radius,
        })
    }

    pub fn data_pack_settings(&self) -> Result<DataPackSettings> {
        let data = self
            .level
            .value
            .get("Data")
            .context("level.dat is missing Data")?;
        let mut result = DataPackSettings::default();
        if let Some(config) = data.get("DataPacks") {
            ensure!(
                matches!(config, Tag::Compound(_)),
                "DataPacks must be a compound"
            );
            result.enabled = string_list(
                config
                    .get("Enabled")
                    .context("DataPacks is missing Enabled")?,
                "DataPacks.Enabled",
            )?;
            result.disabled = string_list(
                config
                    .get("Disabled")
                    .context("DataPacks is missing Disabled")?,
                "DataPacks.Disabled",
            )?;
        }
        if let Some(features) = data.get("enabled_features") {
            result.enabled_features = string_list(features, "enabled_features")?;
        }
        Ok(result)
    }

    pub fn read_player(&self) -> Result<Option<NamedTag>> {
        let Some(uuid) = self.singleplayer_uuid()? else {
            return Ok(None);
        };
        let path = self
            .root
            .join("players")
            .join("data")
            .join(format!("{uuid}.dat"));
        let file = File::open(&path)
            .with_context(|| format!("cannot open player data {}", path.display()))?;
        Ok(Some(nbt::read_gzip(file).with_context(|| {
            format!("invalid player data {}", path.display())
        })?))
    }

    pub fn read_chunk(&mut self, x: i32, z: i32) -> Result<Option<LoadedChunk>> {
        let directory = self.dimension.region_dir(&self.root);
        read_region_document(&directory, x, z, &mut self.regions, &mut self.region_usage)?
            .map(|document| decode_chunk(document, ChunkPos { x, z }))
            .transpose()
    }

    /// A missing entity-region entry represents an empty entity chunk in 26.3.
    /// Invalid data is returned as an error so collision queries cannot assume
    /// that an unreadable chunk is empty.
    pub fn read_entity_chunk(&mut self, x: i32, z: i32) -> Result<Option<SavedEntityChunk>> {
        let directory = self.dimension.entity_region_dir(&self.root);
        read_region_document(
            &directory,
            x,
            z,
            &mut self.entity_regions,
            &mut self.entity_region_usage,
        )?
        .map(|document| decode_entity_chunk(document, ChunkPos { x, z }))
        .transpose()
    }

    /// Missing POI storage is distinct from a decoded empty entry: Java can
    /// populate or rebuild it when loading the corresponding terrain sections.
    pub fn read_poi_chunk(&mut self, x: i32, z: i32) -> Result<Option<SavedPoiChunk>> {
        let directory = self.dimension.poi_region_dir(&self.root);
        read_region_document(
            &directory,
            x,
            z,
            &mut self.poi_regions,
            &mut self.poi_region_usage,
        )?
        .map(|document| decode_poi_chunk(document, ChunkPos { x, z }))
        .transpose()
    }

    pub fn read_area(&mut self, center: ChunkPos, radius: u32) -> Result<ChunkArea> {
        ensure!(
            radius <= MAX_AREA_RADIUS,
            "area radius must be at most {MAX_AREA_RADIUS} chunks"
        );
        let radius_i32 = radius as i32;
        let min_x = center
            .x
            .checked_sub(radius_i32)
            .context("chunk X range overflow")?;
        let max_x = center
            .x
            .checked_add(radius_i32)
            .context("chunk X range overflow")?;
        let min_z = center
            .z
            .checked_sub(radius_i32)
            .context("chunk Z range overflow")?;
        let max_z = center
            .z
            .checked_add(radius_i32)
            .context("chunk Z range overflow")?;
        let mut chunks = BTreeMap::new();
        let mut missing = Vec::new();
        for z in min_z..=max_z {
            for x in min_x..=max_x {
                let pos = ChunkPos { x, z };
                match self.read_chunk(x, z)? {
                    Some(chunk) => {
                        chunks.insert(pos, Arc::new(chunk));
                    }
                    None => missing.push(pos),
                }
            }
        }
        Ok(ChunkArea {
            center,
            radius,
            chunks,
            missing,
        })
    }
}

fn read_region_document(
    directory: &Path,
    x: i32,
    z: i32,
    regions: &mut BTreeMap<(i32, i32), RegionReader>,
    usage: &mut VecDeque<(i32, i32)>,
) -> Result<Option<NamedTag>> {
    let region_pos = (x.div_euclid(REGION_SIDE), z.div_euclid(REGION_SIDE));
    if !regions.contains_key(&region_pos) {
        let path = directory.join(format!("r.{}.{}.mca", region_pos.0, region_pos.1));
        match std::fs::metadata(&path) {
            Ok(meta) => ensure!(
                meta.is_file(),
                "region path is not a file: {}",
                path.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("cannot access region {}", path.display()));
            }
        }
        let reader = RegionReader::open(&path)?;
        if regions.len() >= MAX_OPEN_REGIONS
            && let Some(oldest) = usage.pop_front()
        {
            regions.remove(&oldest);
        }
        regions.insert(region_pos, reader);
    }
    if let Some(index) = usage.iter().position(|pos| *pos == region_pos) {
        usage.remove(index);
    }
    usage.push_back(region_pos);
    let reader = regions.get_mut(&region_pos).context("region cache error")?;
    reader.read_chunk(x, z)
}

pub(crate) fn decode_poi_chunk(document: NamedTag, expected: ChunkPos) -> Result<SavedPoiChunk> {
    use crate::nbt::TagType;
    fn field<'a>(tag: &'a Tag, name: &str) -> Result<Option<&'a Tag>> {
        let Tag::Compound(fields) = tag else {
            bail!("POI value must be a compound")
        };
        let mut matches = fields.iter().filter(|(key, _)| key.matches(name));
        let value = matches.next().map(|(_, value)| value);
        ensure!(matches.next().is_none(), "duplicate POI field {name}");
        Ok(value)
    }
    match field(&document.value, "DataVersion")? {
        Some(Tag::Int(TARGET_DATA_VERSION)) => {}
        Some(Tag::Int(version)) => {
            bail!("unsupported POI DataVersion {version}; expected {TARGET_DATA_VERSION}")
        }
        _ => bail!("POI chunk is missing integer DataVersion"),
    }
    let Some(Tag::Compound(raw_sections)) = field(&document.value, "Sections")? else {
        bail!("POI chunk Sections must be a compound")
    };
    let mut sections = BTreeMap::new();
    let mut positions = BTreeSet::new();
    for (name, raw_section) in raw_sections {
        let key = String::from_utf16(&name.0).context("invalid POI section name")?;
        let y: i32 = key.parse().context("invalid POI section coordinate")?;
        ensure!(key == y.to_string(), "noncanonical POI section coordinate");
        let valid = field(raw_section, "Valid")?
            .and_then(nbt_bool)
            .unwrap_or(false);
        let records = match field(raw_section, "Records")? {
            Some(Tag::List {
                element_type: TagType::Compound,
                values,
            }) => values.as_slice(),
            Some(Tag::List {
                element_type: TagType::End,
                values,
            }) if values.is_empty() => values.as_slice(),
            _ => bail!("POI section Records must be a list of compounds"),
        };
        let mut decoded = Vec::with_capacity(records.len());
        for record in records {
            let coordinates = match field(record, "pos")? {
                Some(Tag::IntArray(values)) => values.clone(),
                Some(Tag::ByteArray(values)) => {
                    values.iter().map(|v| i32::from(*v as i8)).collect()
                }
                Some(Tag::LongArray(values)) => values.iter().map(|v| *v as i32).collect(),
                Some(Tag::List { values, .. }) => values
                    .iter()
                    .map(|v| v.as_java_int().context("POI position must contain numbers"))
                    .collect::<Result<Vec<_>>>()?,
                _ => bail!("POI position must be an integer sequence"),
            };
            let position: [i32; 3] = coordinates
                .try_into()
                .map_err(|_| anyhow::anyhow!("POI position must contain three coordinates"))?;
            ensure!(
                position[0].div_euclid(16) == expected.x
                    && position[2].div_euclid(16) == expected.z
                    && position[1].div_euclid(16) == y,
                "POI position does not match its chunk and section"
            );
            ensure!(positions.insert(position), "duplicate POI position");
            let Some(Tag::String(type_id)) = field(record, "type")? else {
                bail!("POI type must be a string")
            };
            let free_tickets = match field(record, "free_tickets")? {
                None => 0,
                Some(value) => value
                    .as_java_int()
                    .context("POI free_tickets must be a number")?,
            };
            decoded.push(SavedPoiRecord {
                position,
                type_id: type_id.clone(),
                free_tickets,
            });
        }
        ensure!(
            sections
                .insert(
                    y,
                    SavedPoiSection {
                        valid,
                        records: decoded
                    }
                )
                .is_none(),
            "duplicate POI section"
        );
    }
    Ok(SavedPoiChunk {
        position: expected,
        document: Arc::new(document),
        sections,
    })
}

pub(crate) fn decode_entity_chunk(
    document: NamedTag,
    expected: ChunkPos,
) -> Result<SavedEntityChunk> {
    use crate::nbt::TagType;
    ensure!(
        matches!(&document.value, Tag::Compound(_)),
        "entity chunk must be a compound"
    );
    match document.value.get("DataVersion") {
        Some(Tag::Int(TARGET_DATA_VERSION)) => {}
        Some(Tag::Int(version)) => {
            bail!("unsupported entity chunk DataVersion {version}; expected {TARGET_DATA_VERSION}")
        }
        _ => bail!("entity chunk is missing integer DataVersion"),
    }
    let coordinates = match document.value.get("Position") {
        Some(Tag::IntArray(values)) => values.clone(),
        Some(Tag::ByteArray(values)) => values.iter().map(|v| i32::from(*v as i8)).collect(),
        Some(Tag::LongArray(values)) => values.iter().map(|v| *v as i32).collect(),
        Some(Tag::List { values, .. }) => values
            .iter()
            .map(|value| {
                value
                    .as_java_int()
                    .context("entity chunk position must contain numbers")
            })
            .collect::<Result<Vec<_>>>()?,
        _ => bail!("entity chunk position must be an integer sequence"),
    };
    ensure!(
        coordinates.as_slice() == [expected.x, expected.z],
        "entity chunk position does not match its region entry"
    );
    match document.value.get("Entities") {
        None => {}
        Some(Tag::List {
            element_type: TagType::Compound,
            values,
        }) if values.iter().all(|value| matches!(value, Tag::Compound(_))) => {}
        Some(Tag::List {
            element_type: TagType::End,
            values,
        }) if values.is_empty() => {}
        _ => bail!("entity chunk Entities must be a list of compounds"),
    }
    Ok(SavedEntityChunk {
        position: expected,
        document: Arc::new(document),
    })
}

fn decode_chunk(document: NamedTag, expected: ChunkPos) -> Result<LoadedChunk> {
    let int_field = |name| match document.value.get(name) {
        Some(Tag::Int(value)) => Ok(*value),
        _ => bail!("chunk is missing integer {name}"),
    };
    let actual = ChunkPos {
        x: int_field("xPos")?,
        z: int_field("zPos")?,
    };
    ensure!(
        actual == expected,
        "chunk coordinates ({}, {}) do not match region entry ({}, {})",
        actual.x,
        actual.z,
        expected.x,
        expected.z
    );
    let data_version = int_field("DataVersion")?;
    ensure!(
        data_version == TARGET_DATA_VERSION,
        "unsupported chunk DataVersion {data_version}; expected {TARGET_DATA_VERSION}"
    );
    let status = match document.value.get("Status") {
        Some(Tag::String(value)) => Some(value.clone()),
        None => None,
        _ => bail!("chunk Status must be a string"),
    };
    let sections = palette::decode_sections(&document.value)?;
    Ok(LoadedChunk {
        position: expected,
        data_version,
        status,
        document: Arc::new(document),
        sections,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn block_entity_payload_distinguishes_empty_data_and_ambiguous_sources() {
        use super::*;
        use native_world::loaded::SavedBlockEntities;
        let empty = Tag::Compound(vec![]);
        assert_eq!(
            saved_block_entity_payload(&empty),
            SavedBlockEntities::Empty
        );
        let list = Tag::List {
            element_type: crate::nbt::TagType::Compound,
            values: vec![Tag::Compound(vec![(
                "id".into(),
                Tag::String("minecraft:chest".into()),
            )])],
        };
        let chunk = Tag::Compound(vec![("block_entities".into(), list.clone())]);
        let SavedBlockEntities::Encoded(bytes) = saved_block_entity_payload(&chunk) else {
            panic!("encoded payload expected")
        };
        assert_eq!(crate::nbt::read(&bytes).unwrap().value, list);
        let duplicate = Tag::Compound(vec![
            ("block_entities".into(), list.clone()),
            ("block_entities".into(), list),
        ]);
        assert_eq!(
            saved_block_entity_payload(&duplicate),
            SavedBlockEntities::Unavailable
        );
        assert_eq!(
            saved_block_entity_payload(&Tag::Compound(vec![(
                "block_entities".into(),
                Tag::Int(1)
            )])),
            SavedBlockEntities::Unavailable
        );
    }
    use std::io::Write;

    use super::*;
    use crate::nbt::TagType;
    use flate2::{Compression, write::GzEncoder};

    fn write_gzip(path: &Path, root: &NamedTag) {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&nbt::write(root).unwrap()).unwrap();
        std::fs::write(path, encoder.finish().unwrap()).unwrap();
    }

    fn level(version: i32) -> NamedTag {
        NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "Data".into(),
                Tag::Compound(vec![
                    ("DataVersion".into(), Tag::Int(version)),
                    (
                        "singleplayer_uuid".into(),
                        Tag::IntArray(vec![
                            0x00112233,
                            0x44556677,
                            0x8899aabbu32 as i32,
                            0xccddeeffu32 as i32,
                        ]),
                    ),
                ]),
            )]),
        }
    }

    #[test]
    fn reads_world_clock_file_and_rejects_invalid_data() {
        let dir = tempfile::tempdir().unwrap();
        write_gzip(&dir.path().join("level.dat"), &level(TARGET_DATA_VERSION));
        let reader =
            WorldReader::open(dir.path(), DimensionId::parse("overworld").unwrap()).unwrap();
        assert!(reader.world_clocks().unwrap().is_empty());
        let folder = dir.path().join("data/minecraft");
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("world_clocks.dat");
        let file = |version, rate| NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(version)),
                ("future_root".into(), Tag::String("keep me".into())),
                (
                    "data".into(),
                    Tag::Compound(vec![(
                        "example:test".into(),
                        Tag::Compound(vec![
                            ("total_ticks".into(), Tag::Long(-50)),
                            ("rate".into(), Tag::Float(rate)),
                            ("future_clock".into(), Tag::Int(81)),
                        ]),
                    )]),
                ),
            ]),
        };
        write_gzip(&path, &file(TARGET_DATA_VERSION, 2.5));
        let before = std::fs::read(&path).unwrap();
        assert_eq!(
            reader.world_clocks().unwrap()[&DimensionId::parse("example:test").unwrap()],
            SavedClock {
                total_ticks: -50,
                partial_tick: 0.0,
                rate: 2.5,
                paused: false
            }
        );
        assert_eq!(before, std::fs::read(&path).unwrap());
        let source = reader.world_clocks_document().unwrap().unwrap();
        let overworld = DimensionId::parse("overworld").unwrap();
        let updated = WorldReader::update_world_clock_document(
            &source,
            &BTreeMap::from([
                (
                    DimensionId::parse("example:test").unwrap(),
                    SavedClock {
                        total_ticks: 14,
                        partial_tick: 0.25,
                        rate: 2.5,
                        paused: true,
                    },
                ),
                (
                    overworld.clone(),
                    SavedClock {
                        total_ticks: 99,
                        ..SavedClock::default()
                    },
                ),
            ]),
        )
        .unwrap();
        assert_eq!(
            updated.value.get("future_root"),
            Some(&Tag::String("keep me".into()))
        );
        assert_eq!(
            updated
                .value
                .get("data")
                .unwrap()
                .get("example:test")
                .unwrap()
                .get("future_clock"),
            Some(&Tag::Int(81))
        );
        let saved_clocks = parse_world_clocks(&updated).unwrap();
        assert_eq!(
            saved_clocks[&DimensionId::parse("example:test").unwrap()],
            SavedClock {
                total_ticks: 14,
                partial_tick: 0.25,
                rate: 2.5,
                paused: true,
            }
        );
        assert_eq!(saved_clocks[&overworld].total_ticks, 99);
        let mut duplicate_root = source.clone();
        let Tag::Compound(fields) = &mut duplicate_root.value else {
            panic!()
        };
        fields.push(("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)));
        assert!(WorldReader::update_world_clock_document(&duplicate_root, &saved_clocks).is_err());
        let mut duplicate_clock = source.clone();
        let Tag::Compound(root) = &mut duplicate_clock.value else {
            panic!()
        };
        let Tag::Compound(entries) = &mut root
            .iter_mut()
            .find(|(name, _)| name.matches("data"))
            .unwrap()
            .1
        else {
            panic!()
        };
        let Tag::Compound(fields) = &mut entries[0].1 else {
            panic!()
        };
        fields.push(("rate".into(), Tag::Float(2.5)));
        assert!(WorldReader::update_world_clock_document(&duplicate_clock, &saved_clocks).is_err());
        write_gzip(&path, &file(TARGET_DATA_VERSION, 0.0));
        assert!(reader.world_clocks().is_err());
        write_gzip(&path, &file(1, 1.0));
        assert!(reader.world_clocks().is_err());
    }

    #[test]
    fn clock_respects_pause_and_global_time_rule() {
        let mut clock = SavedClock {
            total_ticks: 4,
            partial_tick: 0.25,
            rate: 0.5,
            paused: false,
        };
        clock.tick(false);
        assert_eq!((clock.total_ticks, clock.partial_tick), (4, 0.25));
        clock.tick(true);
        clock.tick(true);
        assert_eq!((clock.total_ticks, clock.partial_tick), (5, 0.25));
        clock.paused = true;
        clock.tick(true);
        assert_eq!((clock.total_ticks, clock.partial_tick), (5, 0.25));
    }

    #[test]
    fn dimension_paths_and_player_uuid_are_checked() {
        for id in [
            "../secret",
            "minecraft:../bad",
            "UPPER:world",
            "minecraft:a//b",
        ] {
            assert!(DimensionId::parse(id).is_err());
        }
        assert_eq!(
            DimensionId::parse("the_nether").unwrap().namespace,
            "minecraft"
        );
        let dir = tempfile::tempdir().unwrap();
        write_gzip(&dir.path().join("level.dat"), &level(TARGET_DATA_VERSION));
        let reader =
            WorldReader::open(dir.path(), DimensionId::parse("overworld").unwrap()).unwrap();
        assert_eq!(
            reader.singleplayer_uuid_words().unwrap(),
            Some([
                0x00112233,
                0x44556677,
                0x8899aabbu32 as i32,
                0xccddeeffu32 as i32,
            ])
        );
        assert_eq!(
            reader.singleplayer_uuid().unwrap().as_deref(),
            Some("00112233-4455-6677-8899-aabbccddeeff")
        );
        assert!(reader.read_player().is_err());
        let player_dir = dir.path().join("players/data");
        std::fs::create_dir_all(&player_dir).unwrap();
        let player = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![("custom".into(), Tag::Int(7))]),
        };
        write_gzip(
            &player_dir.join("00112233-4455-6677-8899-aabbccddeeff.dat"),
            &player,
        );
        assert_eq!(reader.read_player().unwrap(), Some(player));
        write_gzip(&dir.path().join("level.dat"), &level(1));
        assert!(WorldReader::open(dir.path(), DimensionId::parse("overworld").unwrap()).is_err());
    }

    #[test]
    fn reads_spawn_defaults_and_rejects_unsafe_or_incomplete_positions() {
        let dir = tempfile::tempdir().unwrap();
        write_gzip(&dir.path().join("level.dat"), &level(TARGET_DATA_VERSION));
        let reader =
            WorldReader::open(dir.path(), DimensionId::parse("overworld").unwrap()).unwrap();
        assert_eq!(reader.world_spawn().unwrap(), RespawnData::default());
        let spawn = |dimension: &str, position: Tag, yaw: Tag, pitch: Tag| {
            Tag::Compound(vec![
                ("dimension".into(), Tag::String(dimension.into())),
                ("pos".into(), position),
                ("yaw".into(), yaw),
                ("pitch".into(), pitch),
            ])
        };
        let player = |tag: Tag| NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![("respawn".into(), tag)]),
        };
        let valid = spawn(
            "overworld",
            Tag::IntArray(vec![-1, 64, 2]),
            Tag::Int(180),
            Tag::Double(-90.0),
        );
        assert_eq!(
            read_player_respawn(&player(valid)).unwrap(),
            Some(PlayerRespawnConfig {
                data: RespawnData {
                    dimension: DimensionId::parse("minecraft:overworld").unwrap(),
                    position: [-1, 64, 2],
                    yaw: 180.0,
                    pitch: -90.0,
                },
                forced: false,
            })
        );
        for invalid in [
            spawn(
                "minecraft:../escape",
                Tag::IntArray(vec![0, 0, 0]),
                Tag::Int(0),
                Tag::Int(0),
            ),
            spawn(
                "overworld",
                Tag::IntArray(vec![0, 0]),
                Tag::Int(0),
                Tag::Int(0),
            ),
            spawn(
                "overworld",
                Tag::IntArray(vec![0, 0, 0]),
                Tag::Float(f32::NAN),
                Tag::Int(0),
            ),
            spawn(
                "overworld",
                Tag::IntArray(vec![0, 0, 0]),
                Tag::Int(0),
                Tag::Int(91),
            ),
        ] {
            assert!(read_player_respawn(&player(invalid)).is_err());
        }
        assert_eq!(
            read_player_respawn(&NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![])
            })
            .unwrap(),
            None
        );
    }

    #[test]
    fn reads_world_generation_dimension_types_and_rejects_invalid_data() {
        let dir = tempfile::tempdir().unwrap();
        write_gzip(&dir.path().join("level.dat"), &level(TARGET_DATA_VERSION));
        let reader =
            WorldReader::open(dir.path(), DimensionId::parse("overworld").unwrap()).unwrap();
        assert_eq!(reader.saved_dimension_types().unwrap(), None);
        let path = dir.path().join("data/minecraft/world_gen_settings.dat");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let stem = |kind: Tag| {
            Tag::Compound(vec![
                ("type".into(), kind),
                ("generator".into(), Tag::Compound(vec![])),
            ])
        };
        let file = |version, dimensions: Tag| NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(version)),
                (
                    "data".into(),
                    Tag::Compound(vec![("dimensions".into(), dimensions)]),
                ),
            ]),
        };
        let dimensions = Tag::Compound(vec![
            (
                "overworld".into(),
                stem(Tag::String("minecraft:the_nether".into())),
            ),
            (
                "example:custom".into(),
                stem(Tag::Compound(vec![("height".into(), Tag::Int(256))])),
            ),
        ]);
        write_gzip(&path, &file(TARGET_DATA_VERSION, dimensions.clone()));
        let mappings = reader.saved_dimension_types().unwrap().unwrap();
        assert_eq!(
            mappings[&DimensionId::parse("minecraft:overworld").unwrap()],
            SavedDimensionType::Named(DimensionId::parse("the_nether").unwrap())
        );
        assert!(matches!(
            mappings[&DimensionId::parse("example:custom").unwrap()],
            SavedDimensionType::Inline(_)
        ));
        write_gzip(&path, &file(1, dimensions));
        assert!(reader.saved_dimension_types().is_err());
        write_gzip(
            &path,
            &file(
                TARGET_DATA_VERSION,
                Tag::Compound(vec![("overworld".into(), Tag::Compound(vec![]))]),
            ),
        );
        assert!(reader.saved_dimension_types().is_err());
        write_gzip(
            &path,
            &file(
                TARGET_DATA_VERSION,
                Tag::Compound(vec![
                    (
                        "overworld".into(),
                        stem(Tag::String("minecraft:overworld".into())),
                    ),
                    (
                        "minecraft:overworld".into(),
                        stem(Tag::String("minecraft:the_nether".into())),
                    ),
                ]),
            ),
        );
        assert!(reader.saved_dimension_types().is_err());
    }

    #[test]
    fn reads_world_default_game_type_from_level_data() {
        let dir = tempfile::tempdir().unwrap();
        for (field, expected) in [
            (None, 0),
            (Some(Tag::Int(1)), 1),
            (Some(Tag::Byte(2)), 2),
            (Some(Tag::Double(3.75)), 3),
            (Some(Tag::String("creative".into())), 0),
        ] {
            let mut level = level(TARGET_DATA_VERSION);
            if let Some(field) = field {
                let Tag::Compound(root) = &mut level.value else {
                    unreachable!()
                };
                let Tag::Compound(data) = &mut root[0].1 else {
                    unreachable!()
                };
                data.push(("GameType".into(), field));
            }
            write_gzip(&dir.path().join("level.dat"), &level);
            let reader =
                WorldReader::open(dir.path(), DimensionId::parse("overworld").unwrap()).unwrap();
            assert_eq!(reader.default_game_type_id().unwrap(), expected);
        }
    }

    #[test]
    fn reads_allow_commands_with_world_game_type_default() {
        let dir = tempfile::tempdir().unwrap();
        for (mode, flag, expected) in [
            (0, None, false),
            (1, None, true),
            (2, None, false),
            (1, Some(Tag::Byte(0)), false),
            (0, Some(Tag::Byte(1)), true),
            (1, Some(Tag::String("invalid".into())), true),
        ] {
            let mut document = level(TARGET_DATA_VERSION);
            let Tag::Compound(root) = &mut document.value else {
                unreachable!()
            };
            let Tag::Compound(data) = &mut root[0].1 else {
                unreachable!()
            };
            data.push(("GameType".into(), Tag::Int(mode)));
            if let Some(flag) = flag {
                data.push(("allowCommands".into(), flag));
            }
            write_gzip(&dir.path().join("level.dat"), &document);
            let reader =
                WorldReader::open(dir.path(), DimensionId::parse("overworld").unwrap()).unwrap();
            assert_eq!(reader.allow_commands().unwrap(), expected);
        }
    }

    #[test]
    fn world_border_is_dimension_scoped_and_rejects_incomplete_data() {
        let dir = tempfile::tempdir().unwrap();
        write_gzip(&dir.path().join("level.dat"), &level(TARGET_DATA_VERSION));
        let reader =
            WorldReader::open(dir.path(), DimensionId::parse("overworld").unwrap()).unwrap();
        assert_eq!(reader.world_border().unwrap(), SavedWorldBorder::default());
        let path = dir
            .path()
            .join("dimensions/minecraft/overworld/data/minecraft/world_border.dat");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let data = Tag::Compound(vec![
            ("center_x".into(), Tag::Int(5)),
            ("center_z".into(), Tag::Float(-7.5)),
            ("damage_per_block".into(), Tag::Double(0.2)),
            ("safe_zone".into(), Tag::Double(5.0)),
            ("warning_blocks".into(), Tag::Byte(4)),
            ("warning_time".into(), Tag::Int(20)),
            ("size".into(), Tag::Double(100.0)),
            ("lerp_time".into(), Tag::Long(42)),
            ("lerp_target".into(), Tag::Double(200.0)),
        ]);
        let document = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                ("data".into(), data),
            ]),
        };
        std::fs::write(&path, nbt::write(&document).unwrap()).unwrap();
        let border = reader.world_border().unwrap();
        assert_eq!(
            (
                border.center_x,
                border.center_z,
                border.size,
                border.lerp_time
            ),
            (5.0, -7.5, 100.0, 42)
        );
        let other =
            WorldReader::open(dir.path(), DimensionId::parse("the_nether").unwrap()).unwrap();
        assert_eq!(other.world_border().unwrap(), SavedWorldBorder::default());
        let incomplete = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                ("data".into(), Tag::Compound(vec![])),
            ]),
        };
        write_gzip(&path, &incomplete);
        assert!(reader.world_border().is_err());
    }

    #[test]
    fn reads_26_3_difficulty_settings_as_a_complete_value() {
        let dir = tempfile::tempdir().unwrap();
        let settings = |difficulty: &str, hardcore, locked| {
            Tag::Compound(vec![
                ("difficulty".into(), Tag::String(difficulty.into())),
                ("hardcore".into(), Tag::Byte(hardcore)),
                ("locked".into(), Tag::Byte(locked)),
            ])
        };
        for (field, expected) in [
            (None, DifficultySettings::default()),
            (
                Some(settings("hard", 1, 1)),
                DifficultySettings {
                    difficulty: Difficulty::Hard,
                    hardcore: true,
                    locked: true,
                },
            ),
            (
                Some(settings("peaceful", 0, 1)),
                DifficultySettings {
                    difficulty: Difficulty::Peaceful,
                    hardcore: false,
                    locked: true,
                },
            ),
            (
                Some(settings("unknown", 1, 1)),
                DifficultySettings::default(),
            ),
            (
                Some(Tag::Compound(vec![("hardcore".into(), Tag::Byte(1))])),
                DifficultySettings::default(),
            ),
        ] {
            let mut document = level(TARGET_DATA_VERSION);
            if let Some(field) = field {
                let Tag::Compound(root) = &mut document.value else {
                    unreachable!()
                };
                let Tag::Compound(data) = &mut root[0].1 else {
                    unreachable!()
                };
                data.push(("difficulty_settings".into(), field));
            }
            write_gzip(&dir.path().join("level.dat"), &document);
            let reader =
                WorldReader::open(dir.path(), DimensionId::parse("overworld").unwrap()).unwrap();
            assert_eq!(reader.difficulty_settings().unwrap(), expected);
            assert_eq!(reader.hardcore().unwrap(), expected.hardcore);
        }
    }

    #[test]
    fn source_conversion_rules_use_the_26_3_saved_data_path() {
        let dir = tempfile::tempdir().unwrap();
        write_gzip(&dir.path().join("level.dat"), &level(TARGET_DATA_VERSION));
        let reader =
            WorldReader::open(dir.path(), DimensionId::parse("overworld").unwrap()).unwrap();
        assert_eq!(
            reader.fluid_game_rules().unwrap(),
            FluidGameRules::default()
        );
        let path = dir.path().join("data/minecraft/game_rules.dat");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut root = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                (
                    "data".into(),
                    Tag::Compound(vec![
                        ("minecraft:water_source_conversion".into(), Tag::Byte(0)),
                        ("minecraft:lava_source_conversion".into(), Tag::Byte(1)),
                        ("minecraft:block_drops".into(), Tag::Byte(0)),
                        ("minecraft:entity_drops".into(), Tag::Byte(0)),
                        ("minecraft:natural_health_regeneration".into(), Tag::Byte(0)),
                        ("minecraft:fall_damage".into(), Tag::Byte(0)),
                        ("minecraft:keep_inventory".into(), Tag::Byte(1)),
                        ("minecraft:immediate_respawn".into(), Tag::Byte(1)),
                        ("minecraft:show_death_messages".into(), Tag::Byte(0)),
                        ("minecraft:respawn_radius".into(), Tag::Int(7)),
                        ("minecraft:random_tick_speed".into(), Tag::Int(9)),
                        ("minecraft:advance_time".into(), Tag::Byte(0)),
                    ]),
                ),
            ]),
        };
        for compressed in [true, false] {
            if compressed {
                write_gzip(&path, &root);
            } else {
                std::fs::write(&path, nbt::write(&root).unwrap()).unwrap();
            }
            assert_eq!(
                reader.fluid_game_rules().unwrap(),
                FluidGameRules {
                    random_tick_speed: 9,
                    advance_time: false,
                    water_source_conversion: false,
                    lava_source_conversion: true,
                    block_drops: false,
                    entity_drops: false,
                    natural_health_regeneration: false,
                    fall_damage: false,
                    keep_inventory: true,
                    immediate_respawn: true,
                    show_death_messages: false,
                    respawn_radius: 7,
                }
            );
        }
        let Tag::Compound(entries) = &mut root.value else {
            unreachable!()
        };
        entries[0].1 = Tag::Int(TARGET_DATA_VERSION - 1);
        write_gzip(&path, &root);
        assert!(
            reader
                .fluid_game_rules()
                .unwrap_err()
                .to_string()
                .contains("DataVersion")
        );
    }

    #[test]
    fn region_handles_are_bounded_and_recent_regions_remain_cached() {
        let dir = tempfile::tempdir().unwrap();
        write_gzip(&dir.path().join("level.dat"), &level(TARGET_DATA_VERSION));
        let dimension = DimensionId::parse("overworld").unwrap();
        let regions = dimension.region_dir(dir.path());
        std::fs::create_dir_all(&regions).unwrap();
        for x in 0..=MAX_OPEN_REGIONS {
            std::fs::write(regions.join(format!("r.{x}.0.mca")), vec![0u8; 8192]).unwrap();
        }
        let mut reader = WorldReader::open(dir.path(), dimension).unwrap();
        for x in 0..MAX_OPEN_REGIONS {
            assert!(reader.read_chunk((x as i32) * 32, 0).unwrap().is_none());
        }
        assert!(reader.read_chunk(0, 0).unwrap().is_none());
        assert!(
            reader
                .read_chunk((MAX_OPEN_REGIONS as i32) * 32, 0)
                .unwrap()
                .is_none()
        );
        assert_eq!(reader.regions.len(), MAX_OPEN_REGIONS);
        assert!(reader.regions.contains_key(&(0, 0)));
        assert!(!reader.regions.contains_key(&(1, 0)));
    }

    #[test]
    fn dimension_switch_reopens_block_and_entity_regions_at_the_same_coordinates() {
        let dir = tempfile::tempdir().unwrap();
        write_gzip(&dir.path().join("level.dat"), &level(TARGET_DATA_VERSION));
        let dimensions = [
            "minecraft:overworld",
            "minecraft:the_nether",
            "minecraft:the_end",
            "example:custom/nested",
        ];
        let write_region = |directory: &Path, marker: i32, entity: bool| {
            std::fs::create_dir_all(directory).unwrap();
            let mut fields = vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                ("marker".into(), Tag::Int(marker)),
            ];
            if entity {
                fields.push(("Position".into(), Tag::IntArray(vec![0, 0])));
                fields.push((
                    "Entities".into(),
                    Tag::List {
                        element_type: TagType::End,
                        values: Vec::new(),
                    },
                ));
            } else {
                fields.push(("xPos".into(), Tag::Int(0)));
                fields.push(("zPos".into(), Tag::Int(0)));
            }
            let bytes = nbt::write(&NamedTag {
                name: "".into(),
                value: Tag::Compound(fields),
            })
            .unwrap();
            assert!(bytes.len() + 5 <= 4096);
            let mut region = vec![0u8; 3 * 4096];
            region[0..4].copy_from_slice(&((2u32 << 8) | 1).to_be_bytes());
            region[8192..8196].copy_from_slice(&((bytes.len() + 1) as u32).to_be_bytes());
            region[8196] = 3;
            region[8197..8197 + bytes.len()].copy_from_slice(&bytes);
            let path = directory.join("r.0.0.mca");
            std::fs::write(&path, &region).unwrap();
            (path, region)
        };
        let mut files = Vec::new();
        for (marker, name) in dimensions.iter().enumerate() {
            let dimension = DimensionId::parse(name).unwrap();
            files.push(write_region(
                &dimension.region_dir(dir.path()),
                marker as i32,
                false,
            ));
            files.push(write_region(
                &dimension.entity_region_dir(dir.path()),
                marker as i32 + 10,
                true,
            ));
        }
        let mut reader =
            WorldReader::open_for_session(dir.path(), DimensionId::parse(dimensions[0]).unwrap())
                .unwrap();
        for name in dimensions.into_iter().chain(dimensions.into_iter().rev()) {
            let index = dimensions.iter().position(|item| *item == name).unwrap() as i32;
            reader.set_dimension(DimensionId::parse(name).unwrap());
            assert_eq!(reader.dimension(), &DimensionId::parse(name).unwrap());
            assert_eq!(
                reader
                    .read_chunk(0, 0)
                    .unwrap()
                    .unwrap()
                    .document
                    .value
                    .get("marker"),
                Some(&Tag::Int(index))
            );
            assert_eq!(
                reader
                    .read_entity_chunk(0, 0)
                    .unwrap()
                    .unwrap()
                    .document
                    .value
                    .get("marker"),
                Some(&Tag::Int(index + 10))
            );
            assert_eq!(reader.regions.len(), 1);
            assert_eq!(reader.entity_regions.len(), 1);
            reader.set_dimension(DimensionId::parse(name).unwrap());
            assert_eq!(reader.regions.len(), 1);
            assert_eq!(reader.entity_regions.len(), 1);
        }
        for (path, bytes) in files {
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
    }

    #[test]
    fn entity_documents_preserve_unknown_data_and_reject_wrong_locations() {
        use crate::nbt::TagType;
        let expected = ChunkPos { x: -33, z: 5 };
        let entity = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:pig".into())),
            ("unrecognized".into(), Tag::Int(42)),
        ]);
        let make = |position: Tag, entities: Tag| NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                ("Position".into(), position),
                ("Entities".into(), entities),
                ("extra".into(), Tag::String("kept".into())),
            ]),
        };
        let list = Tag::List {
            element_type: TagType::Compound,
            values: vec![entity.clone()],
        };
        let source = make(Tag::IntArray(vec![-33, 5]), list.clone());
        let decoded = decode_entity_chunk(source.clone(), expected).unwrap();
        assert_eq!(decoded.entities(), &[entity]);
        assert_eq!(*decoded.document, source);
        for invalid in [
            make(Tag::IntArray(vec![-32, 5]), list.clone()),
            make(Tag::IntArray(vec![-33]), list.clone()),
            make(Tag::String("bad".into()), list.clone()),
            make(Tag::IntArray(vec![-33, 5]), Tag::Int(1)),
        ] {
            assert!(decode_entity_chunk(invalid, expected).is_err());
        }
        let mut old = source;
        let Tag::Compound(fields) = &mut old.value else {
            unreachable!()
        };
        fields[0].1 = Tag::Int(TARGET_DATA_VERSION - 1);
        assert!(decode_entity_chunk(old, expected).is_err());
        let empty = make(
            Tag::IntArray(vec![-33, 5]),
            Tag::List {
                element_type: TagType::End,
                values: vec![],
            },
        );
        assert!(
            decode_entity_chunk(empty, expected)
                .unwrap()
                .entities()
                .is_empty()
        );
    }

    #[test]
    fn poi_regions_decode_preserve_originals_and_are_dimension_scoped() {
        let dir = tempfile::tempdir().unwrap();
        write_gzip(&dir.path().join("level.dat"), &level(TARGET_DATA_VERSION));
        let make = |x: i32, marker: i32| NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                ("unknown".into(), Tag::Int(marker)),
                (
                    "Sections".into(),
                    Tag::Compound(vec![(
                        "-1".into(),
                        Tag::Compound(vec![
                            ("Valid".into(), Tag::Byte(1)),
                            (
                                "Records".into(),
                                Tag::List {
                                    element_type: TagType::Compound,
                                    values: vec![Tag::Compound(vec![
                                        ("pos".into(), Tag::IntArray(vec![x, -1, 0])),
                                        ("type".into(), Tag::String("minecraft:home".into())),
                                        ("free_tickets".into(), Tag::Int(1)),
                                        ("future".into(), Tag::String("preserved".into())),
                                    ])],
                                },
                            ),
                        ]),
                    )]),
                ),
            ]),
        };
        let mut files = Vec::new();
        for (index, name) in ["overworld", "the_nether"].into_iter().enumerate() {
            let dimension = DimensionId::parse(name).unwrap();
            let directory = dimension.poi_region_dir(dir.path());
            std::fs::create_dir_all(&directory).unwrap();
            let source = make(-1, index as i32);
            let bytes = nbt::write(&source).unwrap();
            let mut region = vec![0u8; 3 * 4096];
            region[31 * 4..32 * 4].copy_from_slice(&((2u32 << 8) | 1).to_be_bytes());
            region[8192..8196].copy_from_slice(&((bytes.len() + 1) as u32).to_be_bytes());
            region[8196] = 3;
            region[8197..8197 + bytes.len()].copy_from_slice(&bytes);
            let path = directory.join("r.-1.0.mca");
            std::fs::write(&path, &region).unwrap();
            files.push((path, region, source));
        }
        let mut reader =
            WorldReader::open(dir.path(), DimensionId::parse("overworld").unwrap()).unwrap();
        assert!(reader.read_poi_chunk(0, 0).unwrap().is_none());
        for (index, name) in ["overworld", "the_nether", "overworld"]
            .into_iter()
            .enumerate()
        {
            let source_index = if index == 1 { 1 } else { 0 };
            reader.set_dimension(DimensionId::parse(name).unwrap());
            let chunk = reader.read_poi_chunk(-1, 0).unwrap().unwrap();
            assert_eq!(*chunk.document, files[source_index].2);
            let section = &chunk.sections[&-1];
            assert!(section.valid);
            assert_eq!(section.records[0].position, [-1, -1, 0]);
            assert_eq!(section.records[0].type_id, "minecraft:home".into());
            assert_eq!(section.records[0].free_tickets, 1);
            assert_eq!(reader.poi_regions.len(), 1);
        }
        for (path, bytes, _) in &files {
            assert_eq!(std::fs::read(path).unwrap(), *bytes);
        }
        assert!(decode_poi_chunk(make(0, 0), ChunkPos { x: -1, z: 0 }).is_err());
        let mut duplicate = make(-1, 0);
        let Tag::Compound(root) = &mut duplicate.value else {
            unreachable!()
        };
        root.push(("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)));
        assert!(decode_poi_chunk(duplicate, ChunkPos { x: -1, z: 0 }).is_err());

        let mut duplicate = make(-1, 0);
        let Tag::Compound(root) = &mut duplicate.value else {
            unreachable!()
        };
        let Tag::Compound(sections) = &mut root[2].1 else {
            unreachable!()
        };
        let Tag::Compound(section) = &mut sections[0].1 else {
            unreachable!()
        };
        let Tag::List { values, .. } = &mut section[1].1 else {
            unreachable!()
        };
        values.push(values[0].clone());
        assert!(decode_poi_chunk(duplicate, ChunkPos { x: -1, z: 0 }).is_err());

        let mut defaults = make(-1, 0);
        let Tag::Compound(root) = &mut defaults.value else {
            unreachable!()
        };
        let Tag::Compound(sections) = &mut root[2].1 else {
            unreachable!()
        };
        let Tag::Compound(section) = &mut sections[0].1 else {
            unreachable!()
        };
        section.remove(0);
        let Tag::List { values, .. } = &mut section[0].1 else {
            unreachable!()
        };
        let Tag::Compound(record) = &mut values[0] else {
            unreachable!()
        };
        record.retain(|(name, _)| !name.matches("free_tickets"));
        let chunk = decode_poi_chunk(defaults, ChunkPos { x: -1, z: 0 }).unwrap();
        assert!(!chunk.sections[&-1].valid);
        assert_eq!(chunk.sections[&-1].records[0].free_tickets, 0);
    }

    #[test]
    fn entity_regions_are_read_only_separate_and_bounded() {
        let dir = tempfile::tempdir().unwrap();
        write_gzip(&dir.path().join("level.dat"), &level(TARGET_DATA_VERSION));
        let dimension = DimensionId::parse("minecraft:the_nether").unwrap();
        let path = dimension.entity_region_dir(dir.path());
        std::fs::create_dir_all(&path).unwrap();
        for x in 0..=MAX_OPEN_REGIONS {
            std::fs::write(path.join(format!("r.{x}.0.mca")), vec![0u8; 8192]).unwrap();
        }
        let mut reader = WorldReader::open(dir.path(), dimension).unwrap();
        for x in 0..MAX_OPEN_REGIONS {
            assert!(
                reader
                    .read_entity_chunk((x as i32) * 32, 0)
                    .unwrap()
                    .is_none()
            );
        }
        assert!(reader.read_entity_chunk(0, 0).unwrap().is_none());
        assert!(
            reader
                .read_entity_chunk((MAX_OPEN_REGIONS as i32) * 32, 0)
                .unwrap()
                .is_none()
        );
        assert_eq!(reader.entity_regions.len(), MAX_OPEN_REGIONS);
        assert!(reader.entity_regions.contains_key(&(0, 0)));
        assert!(!reader.entity_regions.contains_key(&(1, 0)));
        assert!(reader.regions.is_empty());
        assert_eq!(
            std::fs::read(path.join("r.0.0.mca")).unwrap(),
            vec![0u8; 8192]
        );
    }

    #[test]
    fn reads_saved_data_pack_selection_and_feature_flags() {
        let dir = tempfile::tempdir().unwrap();
        let mut root = level(TARGET_DATA_VERSION);
        let Tag::Compound(entries) = &mut root.value else {
            unreachable!()
        };
        let Tag::Compound(data) = &mut entries[0].1 else {
            unreachable!()
        };
        let strings = |items: &[&str]| Tag::List {
            element_type: TagType::String,
            values: items
                .iter()
                .map(|item| Tag::String((*item).into()))
                .collect(),
        };
        data.push((
            "DataPacks".into(),
            Tag::Compound(vec![
                (
                    "Enabled".into(),
                    strings(&["vanilla", "file/alpha.zip", "file/한글"]),
                ),
                ("Disabled".into(), strings(&["file/unused"])),
            ]),
        ));
        data.push((
            "enabled_features".into(),
            strings(&["minecraft:vanilla", "minecraft:trade_rebalance"]),
        ));
        write_gzip(&dir.path().join("level.dat"), &root);
        let reader =
            WorldReader::open(dir.path(), DimensionId::parse("overworld").unwrap()).unwrap();
        assert_eq!(
            reader.data_pack_settings().unwrap(),
            DataPackSettings {
                enabled: vec![
                    "vanilla".into(),
                    "file/alpha.zip".into(),
                    "file/한글".into()
                ],
                disabled: vec!["file/unused".into()],
                enabled_features: vec![
                    "minecraft:vanilla".into(),
                    "minecraft:trade_rebalance".into()
                ],
            }
        );
        let Tag::Compound(entries) = &mut root.value else {
            unreachable!()
        };
        let Tag::Compound(data) = &mut entries[0].1 else {
            unreachable!()
        };
        data.pop();
        data.pop();
        write_gzip(&dir.path().join("level.dat"), &root);
        let reader =
            WorldReader::open(dir.path(), DimensionId::parse("overworld").unwrap()).unwrap();
        assert_eq!(
            reader.data_pack_settings().unwrap(),
            DataPackSettings::default()
        );
    }

    #[test]
    fn chunk_preserves_unknown_states_and_light_only_sections() {
        let block = Tag::Compound(vec![
            ("Name".into(), Tag::String("example:unfamiliar".into())),
            (
                "Properties".into(),
                Tag::Compound(vec![("shape".into(), Tag::String("north".into()))]),
            ),
        ]);
        let states = Tag::Compound(vec![(
            "palette".into(),
            Tag::List {
                element_type: TagType::Compound,
                values: vec![block],
            },
        )]);
        let sections = Tag::List {
            element_type: TagType::Compound,
            values: vec![
                Tag::Compound(vec![
                    ("Y".into(), Tag::Byte(-4)),
                    ("block_states".into(), states),
                    ("BlockLight".into(), Tag::ByteArray(vec![0x21; 2048])),
                ]),
                Tag::Compound(vec![
                    ("Y".into(), Tag::Byte(-5)),
                    ("SkyLight".into(), Tag::ByteArray(vec![0xff; 2048])),
                ]),
            ],
        };
        let root = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                ("xPos".into(), Tag::Int(-1)),
                ("zPos".into(), Tag::Int(-1)),
                ("Status".into(), Tag::String("minecraft:full".into())),
                ("sections".into(), sections),
                ("unknown".into(), Tag::Long(99)),
            ]),
        };
        let loaded = decode_chunk(root.clone(), ChunkPos { x: -1, z: -1 }).unwrap();
        assert!(loaded.is_full());
        let state = loaded.block(-1, -64, -1).unwrap();
        assert!(state.name.matches("example:unfamiliar"));
        assert!(state.properties[0].1.matches("north"));
        assert_eq!(loaded.light(-16, -64, -16, false), Some(1));
        assert_eq!(loaded.light(-15, -64, -16, false), Some(2));
        assert_eq!(loaded.light(-16, -80, -16, true), Some(15));
        assert!(loaded.block(-16, -80, -16).is_none());
        assert_eq!(*loaded.document, root);
        let area = ChunkArea {
            center: ChunkPos { x: -1, z: -1 },
            radius: 0,
            chunks: BTreeMap::from([(ChunkPos { x: -1, z: -1 }, Arc::new(loaded))]),
            missing: vec![ChunkPos { x: 0, z: -1 }],
        };
        let runtime = area
            .materialize(&DimensionId::parse("example:custom").unwrap())
            .unwrap();
        let state = runtime.block(-1, -64, -1).unwrap();
        assert_eq!(state.name, "example:unfamiliar");
        assert_eq!(
            state.properties.get("shape").map(String::as_str),
            Some("north")
        );
        assert_eq!(runtime.light(-16, -80, -16, true), Some(15));
        assert!(runtime.block(-16, -80, -16).is_none());
        assert!(
            runtime
                .missing_chunks
                .contains(&native_world::loaded::ChunkPos { x: 0, z: -1 })
        );
        assert!(decode_chunk(root, ChunkPos { x: 0, z: 0 }).is_err());
    }

    #[test]
    fn omitted_block_light_requires_an_unambiguous_light_on_marker() {
        let section = Tag::Compound(vec![
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
        ]);
        for (markers, expected) in [
            (vec![], false),
            (vec![Tag::Byte(0)], false),
            (vec![Tag::Byte(1)], true),
            (vec![Tag::Int(1)], false),
            (vec![Tag::Byte(1), Tag::Byte(1)], false),
        ] {
            let mut fields = vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                ("xPos".into(), Tag::Int(0)),
                ("zPos".into(), Tag::Int(0)),
                ("Status".into(), Tag::String("minecraft:full".into())),
                (
                    "sections".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![section.clone()],
                    },
                ),
            ];
            fields.extend(markers.into_iter().map(|tag| ("isLightOn".into(), tag)));
            let loaded = decode_chunk(
                NamedTag {
                    name: "".into(),
                    value: Tag::Compound(fields),
                },
                ChunkPos { x: 0, z: 0 },
            )
            .unwrap();
            assert_eq!(loaded.block_light_correct(), expected);
            let area = ChunkArea {
                center: loaded.position,
                radius: 0,
                chunks: BTreeMap::from([(loaded.position, Arc::new(loaded))]),
                missing: vec![],
            };
            let world = area
                .materialize(&DimensionId::parse("overworld").unwrap())
                .unwrap();
            assert_eq!(world.light(0, 0, 0, false), expected.then_some(0));
            assert_eq!(
                world.light(0, 16, 0, false),
                None,
                "missing sections remain unknown"
            );
        }
    }

    #[test]
    fn materialized_fluid_ticks_preserve_delay_order_and_filter_foreign_chunks() {
        let tick = |id: &str, x, delay, priority| {
            Tag::Compound(vec![
                ("i".into(), Tag::String(id.into())),
                ("x".into(), Tag::Int(x)),
                ("y".into(), Tag::Int(3)),
                ("z".into(), Tag::Int(-1)),
                ("t".into(), Tag::Int(delay)),
                ("p".into(), Tag::Int(priority)),
            ])
        };
        let root = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                ("xPos".into(), Tag::Int(-1)),
                ("zPos".into(), Tag::Int(-1)),
                ("Status".into(), Tag::String("minecraft:full".into())),
                (
                    "fluid_ticks".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![
                            tick("minecraft:water", -1, 5, -2),
                            tick("example:liquid", -16, -4, 9),
                            tick("minecraft:lava", 0, 1, 0),
                        ],
                    },
                ),
            ]),
        };
        let pos = ChunkPos { x: -1, z: -1 };
        let area = ChunkArea {
            center: pos,
            radius: 0,
            chunks: BTreeMap::from([(pos, Arc::new(decode_chunk(root.clone(), pos).unwrap()))]),
            missing: vec![],
        };
        let runtime = area
            .materialize(&DimensionId::parse("overworld").unwrap())
            .unwrap();
        let ticks = &runtime.saved_fluid_ticks[&native_world::loaded::ChunkPos { x: -1, z: -1 }];
        assert_eq!(ticks.len(), 2);
        assert_eq!(
            (
                ticks[0].fluid.as_str(),
                ticks[0].position,
                ticks[0].delay,
                ticks[0].priority
            ),
            ("minecraft:water", [-1, 3, -1], 5, -2)
        );
        assert_eq!(
            (ticks[1].fluid.as_str(), ticks[1].delay, ticks[1].priority),
            ("example:liquid", -4, 3)
        );
        assert_eq!(*area.chunks[&pos].document, root);
    }

    #[test]
    fn materialization_resolves_default_properties_without_changing_nbt() {
        let root = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
                ("xPos".into(), Tag::Int(0)),
                ("zPos".into(), Tag::Int(0)),
                ("Status".into(), Tag::String("minecraft:full".into())),
                (
                    "sections".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![Tag::Compound(vec![
                            ("Y".into(), Tag::Byte(0)),
                            (
                                "block_states".into(),
                                Tag::Compound(vec![(
                                    "palette".into(),
                                    Tag::List {
                                        element_type: TagType::String,
                                        values: vec![Tag::String("minecraft:grass_block".into())],
                                    },
                                )]),
                            ),
                        ])],
                    },
                ),
            ]),
        };
        let pos = ChunkPos { x: 0, z: 0 };
        let area = ChunkArea {
            center: pos,
            radius: 0,
            chunks: BTreeMap::from([(pos, Arc::new(decode_chunk(root.clone(), pos).unwrap()))]),
            missing: vec![],
        };
        let dimension = DimensionId::parse("overworld").unwrap();
        let defaults = BTreeMap::from([(
            "minecraft:grass_block".to_owned(),
            BTreeMap::from([("snowy".to_owned(), "false".to_owned())]),
        )]);
        let without = area.materialize(&dimension).unwrap();
        let with = area
            .materialize_with_defaults(&dimension, &defaults)
            .unwrap();
        assert!(without.block(0, 0, 0).unwrap().properties.is_empty());
        assert_eq!(with.block(0, 0, 0).unwrap().properties["snowy"], "false");
        assert_eq!(*area.chunks[&pos].document, root);
    }

    #[test]
    fn missing_chunks_and_coordinate_overflow_are_explicit() {
        let dir = tempfile::tempdir().unwrap();
        write_gzip(&dir.path().join("level.dat"), &level(TARGET_DATA_VERSION));
        let mut reader =
            WorldReader::open(dir.path(), DimensionId::parse("overworld").unwrap()).unwrap();
        let area = reader.read_area(ChunkPos { x: -1, z: -1 }, 1).unwrap();
        assert!(area.chunks.is_empty());
        assert_eq!(area.missing.len(), 9);
        assert!(reader.read_area(ChunkPos { x: i32::MAX, z: 0 }, 1).is_err());
        assert!(
            reader
                .read_area(ChunkPos { x: 0, z: 0 }, MAX_AREA_RADIUS + 1)
                .is_err()
        );
    }
}
