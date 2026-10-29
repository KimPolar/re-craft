//! Complete block-state and biome identities for chunks loaded from a saved world.
//!
//! This representation deliberately does not infer collision or rendering behavior from
//! an identifier. Missing chunk and section data remains distinguishable from air.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

pub const BLOCK_COUNT: usize = 4096;
pub const BIOME_COUNT: usize = 64;
pub const LIGHT_BYTES: usize = 2048;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct BlockState {
    pub name: String,
    pub properties: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct BlockStateId(pub u32);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct BiomeId(pub u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockEditError {
    UnknownState,
    UnknownCurrentState,
    MissingChunk,
    MissingSection,
    MissingBlockData,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChunkImportError {
    DimensionMismatch,
    MissingChunk,
    PositionMismatch,
    UnknownBlockState,
    UnknownBiome,
    StaleBlockEdit,
}

impl std::fmt::Display for ChunkImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::DimensionMismatch => "chunk belongs to a different dimension",
            Self::MissingChunk => "source does not contain the requested chunk",
            Self::PositionMismatch => "chunk position does not match its key",
            Self::UnknownBlockState => "chunk contains an unknown block-state ID",
            Self::UnknownBiome => "chunk contains an unknown biome ID",
            Self::StaleBlockEdit => "source chunk conflicts with a pending block edit",
        })
    }
}

impl std::error::Error for ChunkImportError {}

impl std::fmt::Display for BlockEditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::UnknownState => "unknown block state",
            Self::UnknownCurrentState => "current block has an unknown block-state ID",
            Self::MissingChunk => "chunk is not loaded",
            Self::MissingSection => "section is not loaded",
            Self::MissingBlockData => "section has no block-state data",
        })
    }
}

impl std::error::Error for BlockEditError {}

#[derive(Clone, Debug)]
struct Registry<T> {
    entries: Vec<T>,
    lookup: BTreeMap<T, u32>,
}

impl<T: Ord + Clone> Default for Registry<T> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            lookup: BTreeMap::new(),
        }
    }
}

impl<T: Ord + Clone> Registry<T> {
    fn intern(&mut self, value: T) -> u32 {
        if let Some(id) = self.lookup.get(&value) {
            return *id;
        }
        let id = u32::try_from(self.entries.len()).expect("registry exceeds u32 capacity");
        self.entries.push(value.clone());
        self.lookup.insert(value, id);
        id
    }

    fn get(&self, id: u32) -> Option<&T> {
        self.entries.get(id as usize)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ChunkPos {
    pub x: i32,
    pub z: i32,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct BlockPosition {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPosition {
    pub fn chunk(self) -> ChunkPos {
        ChunkPos {
            x: self.x.div_euclid(16),
            z: self.z.div_euclid(16),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlockEdit {
    pub position: BlockPosition,
    pub original: BlockState,
    pub replacement: BlockState,
}

/// A fluid tick saved by Java as a chunk-relative delay and priority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedFluidTick {
    pub fluid: String,
    pub position: [i32; 3],
    pub delay: i32,
    pub priority: i32,
}

/// A block tick saved by Java as a chunk-relative delay and priority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedBlockTick {
    pub block: String,
    pub position: [i32; 3],
    pub delay: i32,
    pub priority: i32,
}

#[derive(Clone, Debug)]
pub struct Section {
    /// Absent data is not treated as air.
    pub blocks: Option<Box<[BlockStateId; BLOCK_COUNT]>>,
    pub biomes: Option<Box<[BiomeId; BIOME_COUNT]>>,
    pub block_light: Option<Box<[u8; LIGHT_BYTES]>>,
    pub sky_light: Option<Box<[u8; LIGHT_BYTES]>>,
}

#[derive(Clone, Debug)]
pub struct Chunk {
    pub position: ChunkPos,
    pub status: Option<String>,
    pub sections: BTreeMap<i8, Arc<Section>>,
}

#[derive(Clone, Debug, Default)]
pub struct World {
    pub dimension: String,
    /// The resolved registry ID of this dimension's type, when available.
    /// This travels with snapshots so streamed mesh work uses the same type.
    pub dimension_type: Option<String>,
    states: Registry<BlockState>,
    biomes: Registry<String>,
    pub chunks: BTreeMap<ChunkPos, Arc<Chunk>>,
    pub missing_chunks: BTreeSet<ChunkPos>,
    pub saved_fluid_ticks: BTreeMap<ChunkPos, Vec<SavedFluidTick>>,
    pub saved_block_ticks: BTreeMap<ChunkPos, Vec<SavedBlockTick>>,
    block_edits: Arc<BTreeMap<BlockPosition, BlockEdit>>,
}

impl World {
    pub fn new(dimension: impl Into<String>) -> Self {
        Self {
            dimension: dimension.into(),
            ..Self::default()
        }
    }
    pub fn intern_state(&mut self, state: BlockState) -> BlockStateId {
        BlockStateId(self.states.intern(state))
    }
    pub fn intern_biome(&mut self, biome: String) -> BiomeId {
        BiomeId(self.biomes.intern(biome))
    }
    pub fn state(&self, id: BlockStateId) -> Option<&BlockState> {
        self.states.get(id.0)
    }
    pub fn biome_by_id(&self, id: BiomeId) -> Option<&str> {
        self.biomes.get(id.0).map(String::as_str)
    }
    pub fn state_count(&self) -> usize {
        self.states.entries.len()
    }
    pub fn states(&self) -> impl Iterator<Item = (BlockStateId, &BlockState)> {
        self.states
            .entries
            .iter()
            .enumerate()
            .map(|(index, state)| (BlockStateId(index as u32), state))
    }
    pub fn biome_count(&self) -> usize {
        self.biomes.entries.len()
    }

    /// Pending block changes, retained independently of chunk residency.
    pub fn block_edits(&self) -> impl Iterator<Item = &BlockEdit> {
        self.block_edits.values()
    }

    pub fn block_edits_in_chunk(&self, pos: ChunkPos) -> impl Iterator<Item = &BlockEdit> {
        self.block_edits
            .values()
            .filter(move |edit| edit.position.chunk() == pos)
    }

    pub fn dirty_chunk_positions(&self) -> BTreeSet<ChunkPos> {
        self.block_edits
            .values()
            .map(|edit| edit.position.chunk())
            .collect()
    }

    pub fn biomes(&self) -> impl Iterator<Item = &str> {
        self.biomes.entries.iter().map(String::as_str)
    }

    /// Add a chunk decoded with a different registry. Existing chunks are never replaced.
    /// All references are validated before the destination is changed.
    pub fn import_chunk(
        &mut self,
        source: &World,
        pos: ChunkPos,
    ) -> Result<bool, ChunkImportError> {
        if self.dimension != source.dimension {
            return Err(ChunkImportError::DimensionMismatch);
        }
        let chunk = source
            .chunks
            .get(&pos)
            .ok_or(ChunkImportError::MissingChunk)?;
        if chunk.position != pos {
            return Err(ChunkImportError::PositionMismatch);
        }
        if self.chunks.contains_key(&pos) {
            return Ok(false);
        }
        let pending_edits: Vec<_> = self.block_edits_in_chunk(pos).cloned().collect();
        for edit in &pending_edits {
            let section_y = i8::try_from(edit.position.y.div_euclid(16))
                .map_err(|_| ChunkImportError::StaleBlockEdit)?;
            let index = edit.position.x.rem_euclid(16) as usize
                + edit.position.z.rem_euclid(16) as usize * 16
                + edit.position.y.rem_euclid(16) as usize * 256;
            let Some(blocks) = chunk
                .sections
                .get(&section_y)
                .and_then(|section| section.blocks.as_ref())
            else {
                return Err(ChunkImportError::StaleBlockEdit);
            };
            let Some(source_state) = blocks.get(index).and_then(|id| source.state(*id)) else {
                return Err(ChunkImportError::UnknownBlockState);
            };
            if source_state != &edit.original && source_state != &edit.replacement {
                return Err(ChunkImportError::StaleBlockEdit);
            }
        }
        for section in chunk.sections.values() {
            if section
                .blocks
                .as_ref()
                .is_some_and(|blocks| blocks.iter().any(|id| source.state(*id).is_none()))
            {
                return Err(ChunkImportError::UnknownBlockState);
            }
            if section
                .biomes
                .as_ref()
                .is_some_and(|biomes| biomes.iter().any(|id| source.biome_by_id(*id).is_none()))
            {
                return Err(ChunkImportError::UnknownBiome);
            }
        }
        let states: Vec<_> = source
            .states
            .entries
            .iter()
            .cloned()
            .map(|state| self.intern_state(state))
            .collect();
        let biomes: Vec<_> = source
            .biomes
            .entries
            .iter()
            .cloned()
            .map(|biome| self.intern_biome(biome))
            .collect();
        let mut sections: BTreeMap<_, _> = chunk
            .sections
            .iter()
            .map(|(&y, section)| {
                let mut section = (**section).clone();
                if let Some(blocks) = &mut section.blocks {
                    for id in blocks.iter_mut() {
                        *id = states[id.0 as usize];
                    }
                }
                if let Some(values) = &mut section.biomes {
                    for id in values.iter_mut() {
                        *id = biomes[id.0 as usize];
                    }
                }
                (y, Arc::new(section))
            })
            .collect();
        for edit in &pending_edits {
            let replacement = self.intern_state(edit.replacement.clone());
            let section_y = i8::try_from(edit.position.y.div_euclid(16))
                .expect("overlay section was validated before registry changes");
            let index = edit.position.x.rem_euclid(16) as usize
                + edit.position.z.rem_euclid(16) as usize * 16
                + edit.position.y.rem_euclid(16) as usize * 256;
            let section = sections
                .get_mut(&section_y)
                .expect("overlay section was validated before registry changes");
            let section = Arc::make_mut(section);
            section
                .blocks
                .as_mut()
                .expect("overlay block data was validated before registry changes")[index] =
                replacement;
        }
        self.chunks.insert(
            pos,
            Arc::new(Chunk {
                position: pos,
                status: chunk.status.clone(),
                sections,
            }),
        );
        self.missing_chunks.remove(&pos);
        if let Some(ticks) = source.saved_fluid_ticks.get(&pos) {
            self.saved_fluid_ticks.insert(pos, ticks.clone());
        } else {
            self.saved_fluid_ticks.remove(&pos);
        }
        if let Some(ticks) = source.saved_block_ticks.get(&pos) {
            self.saved_block_ticks.insert(pos, ticks.clone());
        } else {
            self.saved_block_ticks.remove(&pos);
        }
        Ok(true)
    }

    pub fn block(&self, x: i32, y: i32, z: i32) -> Option<&BlockState> {
        self.state(self.block_id(x, y, z)?)
    }

    pub fn block_id(&self, x: i32, y: i32, z: i32) -> Option<BlockStateId> {
        let (section, local) = self.section_and_local(x, y, z)?;
        let index = local.0 + local.2 * 16 + local.1 * 256;
        section.blocks.as_ref()?.get(index).copied()
    }

    /// Edit only already loaded block data; cloned snapshots retain their old chunks.
    pub fn set_block_id(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        state: BlockStateId,
    ) -> Result<bool, BlockEditError> {
        if self.state(state).is_none() {
            return Err(BlockEditError::UnknownState);
        }
        let position = ChunkPos {
            x: x.div_euclid(16),
            z: z.div_euclid(16),
        };
        let section_y =
            i8::try_from(y.div_euclid(16)).map_err(|_| BlockEditError::MissingSection)?;
        let index = x.rem_euclid(16) as usize
            + z.rem_euclid(16) as usize * 16
            + y.rem_euclid(16) as usize * 256;
        let chunk = self
            .chunks
            .get(&position)
            .ok_or(BlockEditError::MissingChunk)?;
        let section = chunk
            .sections
            .get(&section_y)
            .ok_or(BlockEditError::MissingSection)?;
        let blocks = section
            .blocks
            .as_ref()
            .ok_or(BlockEditError::MissingBlockData)?;
        if blocks[index] == state {
            return Ok(false);
        }
        let block_position = BlockPosition { x, y, z };
        let current = self
            .state(blocks[index])
            .ok_or(BlockEditError::UnknownCurrentState)?
            .clone();
        let replacement = self.state(state).expect("state was checked").clone();
        let original = self
            .block_edits
            .get(&block_position)
            .map(|edit| edit.original.clone())
            .unwrap_or(current);
        let chunk = Arc::make_mut(self.chunks.get_mut(&position).expect("chunk was checked"));
        let section = Arc::make_mut(
            chunk
                .sections
                .get_mut(&section_y)
                .expect("section was checked"),
        );
        section.blocks.as_mut().expect("block data was checked")[index] = state;
        let block_edits = Arc::make_mut(&mut self.block_edits);
        if replacement == original {
            block_edits.remove(&block_position);
        } else {
            block_edits.insert(
                block_position,
                BlockEdit {
                    position: block_position,
                    original,
                    replacement,
                },
            );
        }
        Ok(true)
    }

    pub fn biome(&self, x: i32, y: i32, z: i32) -> Option<&str> {
        let (section, local) = self.section_and_local(x, y, z)?;
        let index = local.0 / 4 + (local.2 / 4) * 4 + (local.1 / 4) * 16;
        self.biome_by_id(*section.biomes.as_ref()?.get(index)?)
    }

    pub fn light(&self, x: i32, y: i32, z: i32, sky: bool) -> Option<u8> {
        let (section, local) = self.section_and_local(x, y, z)?;
        let data = if sky {
            section.sky_light.as_ref()?
        } else {
            section.block_light.as_ref()?
        };
        let index = local.0 + local.2 * 16 + local.1 * 256;
        let byte = *data.get(index / 2)?;
        Some(if index.is_multiple_of(2) {
            byte & 0x0f
        } else {
            byte >> 4
        })
    }

    fn section_and_local(
        &self,
        x: i32,
        y: i32,
        z: i32,
    ) -> Option<(&Section, (usize, usize, usize))> {
        let chunk = self.chunks.get(&ChunkPos {
            x: x.div_euclid(16),
            z: z.div_euclid(16),
        })?;
        let section_y = i8::try_from(y.div_euclid(16)).ok()?;
        let section = chunk.sections.get(&section_y)?;
        Some((
            section,
            (
                x.rem_euclid(16) as usize,
                y.rem_euclid(16) as usize,
                z.rem_euclid(16) as usize,
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_deduplicates_complete_states_without_losing_properties() {
        let mut world = World::default();
        let state = BlockState {
            name: "example:block".into(),
            properties: BTreeMap::from([("facing".into(), "north".into())]),
        };
        let first = world.intern_state(state.clone());
        assert_eq!(world.intern_state(state.clone()), first);
        let other = world.intern_state(BlockState {
            name: state.name.clone(),
            properties: BTreeMap::from([("facing".into(), "south".into())]),
        });
        assert_ne!(other, first);
        assert_eq!(world.state(first), Some(&state));
        assert_eq!(world.state_count(), 2);
    }

    #[test]
    fn imported_chunks_remap_registries_without_replacing_existing_data() {
        let mut source = World::new("minecraft:overworld");
        let stone = source.intern_state(BlockState {
            name: "minecraft:stone".into(),
            properties: BTreeMap::new(),
        });
        let air = source.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let forest = source.intern_biome("minecraft:forest".into());
        let pos = ChunkPos { x: -2, z: 4 };
        source.saved_block_ticks.insert(
            pos,
            vec![SavedBlockTick {
                block: "minecraft:sand".into(),
                position: [-31, 4, 65],
                delay: 2,
                priority: -1,
            }],
        );
        let mut blocks = Box::new([stone; BLOCK_COUNT]);
        blocks[3] = air;
        source.chunks.insert(
            pos,
            Arc::new(Chunk {
                position: pos,
                status: Some("minecraft:full".into()),
                sections: BTreeMap::from([(
                    0,
                    Arc::new(Section {
                        blocks: Some(blocks),
                        biomes: Some(Box::new([forest; BIOME_COUNT])),
                        block_light: Some(Box::new([0x34; LIGHT_BYTES])),
                        sky_light: None,
                    }),
                )]),
            }),
        );
        let mut dest = World::new("minecraft:overworld");
        let dest_air = dest.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        dest.intern_biome("minecraft:plains".into());
        dest.missing_chunks.insert(pos);
        assert!(dest.import_chunk(&source, pos).unwrap());
        assert_eq!(
            dest.saved_block_ticks.get(&pos),
            source.saved_block_ticks.get(&pos)
        );
        assert_eq!(
            dest.block_id(-32, 0, 64)
                .and_then(|id| dest.state(id))
                .unwrap()
                .name,
            "minecraft:stone"
        );
        assert_eq!(dest.block_id(-29, 0, 64), Some(dest_air));
        assert_eq!(dest.biome(-32, 0, 64), Some("minecraft:forest"));
        assert_eq!(dest.light(-32, 0, 64, false), Some(4));
        assert!(!dest.missing_chunks.contains(&pos));
        assert!(dest.set_block_id(-32, 0, 64, dest_air).unwrap());
        assert!(!dest.import_chunk(&source, pos).unwrap());
        assert_eq!(dest.block_id(-32, 0, 64), Some(dest_air));

        let mut bad = source.clone();
        let bad_pos = ChunkPos { x: 9, z: 9 };
        let mut invalid = (**bad.chunks.get(&pos).unwrap()).clone();
        invalid.position = bad_pos;
        Arc::make_mut(invalid.sections.get_mut(&0).unwrap())
            .blocks
            .as_mut()
            .unwrap()[0] = BlockStateId(u32::MAX);
        bad.chunks.insert(bad_pos, Arc::new(invalid));
        let state_count = dest.state_count();
        let biome_count = dest.biome_count();
        assert_eq!(
            dest.import_chunk(&bad, bad_pos),
            Err(ChunkImportError::UnknownBlockState)
        );
        assert_eq!(dest.state_count(), state_count);
        assert_eq!(dest.biome_count(), biome_count);
        assert!(!dest.chunks.contains_key(&bad_pos));

        dest.chunks.remove(&pos);
        dest.saved_fluid_ticks.insert(
            pos,
            vec![SavedFluidTick {
                fluid: "minecraft:water".into(),
                position: [-31, 4, 65],
                delay: 3,
                priority: 0,
            }],
        );
        let mut fresh = source.clone();
        fresh.saved_block_ticks.remove(&pos);
        assert!(dest.import_chunk(&fresh, pos).unwrap());
        assert_eq!(dest.block_id(-32, 0, 64), Some(dest_air));
        assert_eq!(dest.block_edits_in_chunk(pos).count(), 1);
        assert!(!dest.saved_block_ticks.contains_key(&pos));
        assert!(!dest.saved_fluid_ticks.contains_key(&pos));

        dest.chunks.remove(&pos);
        let mut stale = source.clone();
        let dirt = stale.intern_state(BlockState {
            name: "minecraft:dirt".into(),
            properties: BTreeMap::new(),
        });
        let mut stale_chunk = (**stale.chunks.get(&pos).unwrap()).clone();
        Arc::make_mut(stale_chunk.sections.get_mut(&0).unwrap())
            .blocks
            .as_mut()
            .unwrap()[0] = dirt;
        stale.chunks.insert(pos, Arc::new(stale_chunk));
        let state_count = dest.state_count();
        assert_eq!(
            dest.import_chunk(&stale, pos),
            Err(ChunkImportError::StaleBlockEdit)
        );
        assert_eq!(dest.state_count(), state_count);
        assert!(!dest.chunks.contains_key(&pos));
        assert!(dest.import_chunk(&fresh, pos).unwrap());
        assert_eq!(dest.block_id(-32, 0, 64), Some(dest_air));
    }

    #[test]
    fn editing_loaded_blocks_shares_unchanged_snapshot_chunks_and_sections() {
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let stone = world.intern_state(BlockState {
            name: "minecraft:stone".into(),
            properties: BTreeMap::new(),
        });
        let section = || {
            Arc::new(Section {
                blocks: Some(Box::new([air; BLOCK_COUNT])),
                biomes: None,
                block_light: None,
                sky_light: None,
            })
        };
        let pos = ChunkPos { x: -1, z: 0 };
        world.chunks.insert(
            pos,
            Arc::new(Chunk {
                position: pos,
                status: None,
                sections: BTreeMap::from([(-1, section()), (0, section())]),
            }),
        );
        let neighbor = ChunkPos { x: 0, z: 0 };
        world.chunks.insert(
            neighbor,
            Arc::new(Chunk {
                position: neighbor,
                status: None,
                sections: BTreeMap::from([(0, section())]),
            }),
        );
        let previous = world.clone();
        assert!(world.set_block_id(-1, -1, 0, stone).unwrap());
        assert!(!world.set_block_id(-1, -1, 0, stone).unwrap());
        assert_eq!(previous.block_id(-1, -1, 0), Some(air));
        assert_eq!(world.block_id(-1, -1, 0), Some(stone));
        assert!(!Arc::ptr_eq(&world.chunks[&pos], &previous.chunks[&pos]));
        assert!(!Arc::ptr_eq(
            &world.chunks[&pos].sections[&-1],
            &previous.chunks[&pos].sections[&-1]
        ));
        assert!(Arc::ptr_eq(
            &world.chunks[&pos].sections[&0],
            &previous.chunks[&pos].sections[&0]
        ));
        assert!(Arc::ptr_eq(
            &world.chunks[&neighbor],
            &previous.chunks[&neighbor]
        ));
        let edit = world.block_edits().next().unwrap();
        assert_eq!(edit.position, BlockPosition { x: -1, y: -1, z: 0 });
        assert_eq!(edit.original, *world.state(air).unwrap());
        assert_eq!(edit.replacement, *world.state(stone).unwrap());
        assert_eq!(world.dirty_chunk_positions(), BTreeSet::from([pos]));
        let edited_snapshot = world.clone();
        assert!(world.set_block_id(-1, -1, 0, air).unwrap());
        assert_eq!(world.block_id(-1, -1, 0), Some(air));
        assert_eq!(world.block_edits().count(), 0);
        assert_eq!(edited_snapshot.block_edits().count(), 1);
        assert_eq!(
            edited_snapshot.block_edits().next().unwrap().replacement,
            *edited_snapshot.state(stone).unwrap()
        );
        assert_eq!(
            world.set_block_id(16, 0, 0, stone),
            Err(BlockEditError::MissingChunk)
        );
        assert_eq!(
            world.set_block_id(0, 100, 0, stone),
            Err(BlockEditError::MissingSection)
        );
        assert_eq!(
            world.set_block_id(0, 0, 0, BlockStateId(u32::MAX)),
            Err(BlockEditError::UnknownState)
        );
    }
}
