//! Rebuild known 26.3 heightmaps in memory with caller-supplied block semantics.
//!
//! The caller must resolve air, fluid and both motion tags from the same block
//! registry and active data packs as the world. This is not a world-save path.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail, ensure};

use crate::{
    nbt::{NamedTag, Tag},
    palette::{self, BlockPalette, BlockState},
};

/// Bounds of a dimension in blocks; `max_y` is exclusive.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeightBounds {
    pub min_y: i32,
    pub max_y: i32,
}

impl HeightBounds {
    fn height(self) -> Result<u32> {
        ensure!(
            self.min_y.rem_euclid(16) == 0
                && self.max_y.rem_euclid(16) == 0
                && self.min_y >= i32::from(i8::MIN) * 16
                && self.max_y <= (i32::from(i8::MAX) + 1) * 16
                && self.min_y < self.max_y,
            "unsupported dimension height bounds"
        );
        Ok((self.max_y - self.min_y) as u32)
    }
}

/// The predicates used by Java 26.3, after resolving the active data packs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockHeightFlags {
    pub is_air: bool,
    pub blocks_motion: bool,
    pub blocks_motion_no_leaves: bool,
    pub has_fluid: bool,
}

/// The three saved heightmaps consulted by the shared-spawn search. Values are
/// kept as saved, including stale values: Java copies a correctly sized map
/// without checking it against the blocks. A missing map must be primed from
/// the chunk before it can be used.
#[derive(Clone, Debug)]
pub struct SpawnHeightmaps {
    min_y: i32,
    max_y: i32,
    maps: [Option<Box<[u32; 256]>>; 3],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpawnHeightmapKind {
    MotionBlocking,
    WorldSurface,
    OceanFloor,
}

impl SpawnHeightmapKind {
    fn index(self) -> usize {
        match self {
            Self::MotionBlocking => 0,
            Self::WorldSurface => 1,
            Self::OceanFloor => 2,
        }
    }
}

impl SpawnHeightmaps {
    /// Read the on-disk maps, without changing the source document. Java
    /// rebuilds absent maps and arrays whose length is wrong; callers must do
    /// the same instead of interpreting them as zero-height columns.
    pub fn read(document: &NamedTag, bounds: HeightBounds) -> Result<Self> {
        let height = bounds.height()?;
        let parent = unique(&document.value, "Heightmaps")?;
        let parent = parent.filter(|value| matches!(value, Tag::Compound(_)));
        let mut maps = [None, None, None];
        let bits = 32 - height.leading_zeros();
        let per_word = 64 / bits as usize;
        let length = 256usize.div_ceil(per_word);
        let mask = (1u64 << bits) - 1;
        for (index, name) in ["MOTION_BLOCKING", "WORLD_SURFACE", "OCEAN_FLOOR"]
            .into_iter()
            .enumerate()
        {
            let Some(parent) = parent else { continue };
            let Some(Tag::LongArray(raw)) = unique(parent, name)? else {
                continue;
            };
            if raw.len() != length {
                continue;
            }
            let mut values = Box::new([0u32; 256]);
            for (column, value) in values.iter_mut().enumerate() {
                *value = (((raw[column / per_word] as u64)
                    >> ((column % per_word) * bits as usize))
                    & mask) as u32;
            }
            maps[index] = Some(values);
        }
        Ok(Self {
            min_y: bounds.min_y,
            max_y: bounds.max_y,
            maps,
        })
    }

    pub fn bounds(&self) -> HeightBounds {
        HeightBounds {
            min_y: self.min_y,
            max_y: self.max_y,
        }
    }

    pub fn is_complete(&self) -> bool {
        self.maps.iter().all(Option::is_some)
    }

    /// Java's highest matching Y. Coordinates are the low four bits, so the
    /// same method handles negative world coordinates and chunk-local values.
    pub fn highest(&self, kind: SpawnHeightmapKind, x: i32, z: i32) -> Option<i32> {
        let column = x.rem_euclid(16) as usize + z.rem_euclid(16) as usize * 16;
        self.maps[kind.index()]
            .as_ref()
            .map(|values| self.min_y + values[column] as i32 - 1)
    }

    /// Prime only missing maps with the same top-down predicates as Java.
    /// The callback receives chunk-local X/Z and must describe the original
    /// blocks before edits. On an incomplete query no partial maps are stored.
    pub fn prime_missing(
        &mut self,
        block: impl FnMut(i32, i32, i32) -> Result<BlockHeightFlags>,
    ) -> Result<()> {
        self.prime_missing_through(self.max_y - 1, block)
    }

    /// `top` may omit only cells already known to be air. This allows the
    /// chunk loader to skip empty upper sections when Java primes a map.
    pub fn prime_missing_through(
        &mut self,
        top: i32,
        mut block: impl FnMut(i32, i32, i32) -> Result<BlockHeightFlags>,
    ) -> Result<()> {
        if self.is_complete() {
            return Ok(());
        }
        ensure!(
            top >= self.min_y - 1 && top < self.max_y,
            "heightmap priming top is out of range"
        );
        let mut primed: [Option<Box<[u32; 256]>>; 3] =
            std::array::from_fn(|index| self.maps[index].is_none().then(|| Box::new([0; 256])));
        for z in 0..16i32 {
            for x in 0..16i32 {
                let column = (x + z * 16) as usize;
                let mut pending = primed.iter().filter(|map| map.is_some()).count();
                for y in (self.min_y..=top).rev() {
                    let flags = block(x, y, z)?;
                    // Java's priming pass skips air even if a data pack has
                    // placed it in a motion tag.
                    if flags.is_air {
                        continue;
                    }
                    let matches = spawn_predicates(flags);
                    for index in 0..3 {
                        if matches[index]
                            && let Some(map) = &mut primed[index]
                            && map[column] == 0
                        {
                            map[column] = (y + 1 - self.min_y) as u32;
                            pending -= 1;
                        }
                    }
                    if pending == 0 {
                        break;
                    }
                }
            }
        }
        for (target, new) in self.maps.iter_mut().zip(primed) {
            if new.is_some() {
                *target = new;
            }
        }
        Ok(())
    }

    /// Update one column after a block change. Existing saved values are not
    /// rebuilt: Java updates even a stale heightmap incrementally. All three
    /// maps must already be present, and a failed block query changes nothing.
    pub fn update_after_change(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        new_block: BlockHeightFlags,
        mut block: impl FnMut(i32, i32, i32) -> Result<BlockHeightFlags>,
    ) -> Result<()> {
        ensure!(
            y >= self.min_y && y < self.max_y,
            "heightmap edit lies outside the dimension"
        );
        let x = x.rem_euclid(16);
        let z = z.rem_euclid(16);
        let column = (x + z * 16) as usize;
        let mut updated = [0; 3];
        let new_matches = spawn_predicates(new_block);
        for index in 0..3 {
            let values = self.maps[index]
                .as_ref()
                .context("spawn heightmap must be primed before an edit")?;
            let mut value = values[column];
            let first_available = self.min_y + value as i32;
            if y > first_available - 2 {
                if new_matches[index] && y >= first_available {
                    value = (y + 1 - self.min_y) as u32;
                } else if !new_matches[index] && y == first_available - 1 {
                    value = 0;
                    for below in (self.min_y..y).rev() {
                        if spawn_predicates(block(x, below, z)?)[index] {
                            value = (below + 1 - self.min_y) as u32;
                            break;
                        }
                    }
                }
            }
            updated[index] = value;
        }
        for (map, value) in self.maps.iter_mut().zip(updated) {
            map.as_mut().expect("all maps were checked")[column] = value;
        }
        Ok(())
    }
}

fn spawn_predicates(flags: BlockHeightFlags) -> [bool; 3] {
    [
        flags.blocks_motion || flags.has_fluid,
        !flags.is_air,
        flags.blocks_motion,
    ]
}

impl BlockHeightFlags {
    fn mask(self) -> Result<u8> {
        // Java's priming pass skips AIR before applying motion-tag predicates.
        // Air in a motion tag makes priming and incremental updates differ.
        ensure!(
            !self.is_air || !(self.blocks_motion || self.blocks_motion_no_leaves || self.has_fluid),
            "air with motion or fluid flags cannot be rebuilt safely"
        );
        Ok(u8::from(!self.is_air)
            | (u8::from(self.blocks_motion) << 1)
            | (u8::from(self.blocks_motion || self.has_fluid) << 2)
            | (u8::from(self.blocks_motion_no_leaves || self.has_fluid) << 3))
    }
}

const KINDS: [(&str, usize); 6] = [
    ("WORLD_SURFACE", 0),
    ("OCEAN_FLOOR", 1),
    ("MOTION_BLOCKING", 2),
    ("MOTION_BLOCKING_NO_LEAVES", 3),
    ("WORLD_SURFACE_WG", 0),
    ("OCEAN_FLOOR_WG", 1),
];

type StateKey = (Vec<u16>, Vec<(Vec<u16>, Vec<u16>)>);
type Columns = [[u32; 256]; 4];

fn state_key(state: &BlockState) -> StateKey {
    let mut properties: Vec<_> = state
        .properties
        .iter()
        .map(|(name, value)| (name.0.clone(), value.0.clone()))
        .collect();
    properties.sort();
    (state.name.0.clone(), properties)
}

struct Section {
    y: i32,
    blocks: BlockPalette,
    masks: Vec<u8>,
}

fn calculate(
    document: &NamedTag,
    bounds: HeightBounds,
    classify: &mut impl FnMut(&BlockState) -> Result<BlockHeightFlags>,
    cache: &mut BTreeMap<StateKey, u8>,
) -> Result<Columns> {
    let mut sections = Vec::new();
    for section in palette::decode_sections(&document.value)? {
        let y = i32::from(section.y) * 16;
        let Some(blocks) = section.blocks else {
            continue;
        };
        ensure!(
            y >= bounds.min_y && y < bounds.max_y,
            "block section {y} is outside dimension bounds"
        );
        // Check even unused and fully buried palette entries before creating a
        // draft; a later scan may stop once all four heights have been found.
        let masks = blocks
            .states
            .iter()
            .map(|state| {
                let key = state_key(state);
                if let Some(mask) = cache.get(&key) {
                    Ok(*mask)
                } else {
                    let mask = classify(state)?.mask()?;
                    cache.insert(key, mask);
                    Ok(mask)
                }
            })
            .collect::<Result<Vec<_>>>()?;
        sections.push(Section { y, blocks, masks });
    }
    sections.sort_by_key(|section| section.y);
    let mut result = [[0u32; 256]; 4];
    for z in 0..16usize {
        for x in 0..16usize {
            let column = x + z * 16;
            let mut remaining = 0b1111u8;
            for section in sections.iter().rev() {
                for local_y in (0..16usize).rev() {
                    let index = x + z * 16 + local_y * 256;
                    let palette_index = section.blocks.indices[index] as usize;
                    let mask = section.masks[palette_index];
                    let found = remaining & mask;
                    if found != 0 {
                        let offset = (section.y + local_y as i32 + 1 - bounds.min_y) as u32;
                        for (kind, values) in result.iter_mut().enumerate() {
                            if found & (1 << kind) != 0 {
                                values[column] = offset;
                            }
                        }
                        remaining &= !found;
                        if remaining == 0 {
                            break;
                        }
                    }
                }
                if remaining == 0 {
                    break;
                }
            }
        }
    }
    Ok(result)
}

fn unique<'a>(parent: &'a Tag, name: &str) -> Result<Option<&'a Tag>> {
    let Tag::Compound(entries) = parent else {
        bail!("{name} parent is not a compound");
    };
    let mut matching = entries.iter().filter(|(key, _)| key.matches(name));
    let result = matching.next().map(|(_, value)| value);
    ensure!(matching.next().is_none(), "duplicate {name} fields");
    Ok(result)
}

fn raw_values(raw: &[i64], height: u32) -> Result<[u32; 256]> {
    let bits = 32 - height.leading_zeros();
    let per_word = 64 / bits as usize;
    ensure!(
        raw.len() == 256usize.div_ceil(per_word),
        "heightmap has an invalid long-array length"
    );
    let mask = (1u64 << bits) - 1;
    let mut values = [0u32; 256];
    for (i, value) in values.iter_mut().enumerate() {
        *value = (((raw[i / per_word] as u64) >> ((i % per_word) * bits as usize)) & mask) as u32;
        ensure!(
            *value <= height,
            "heightmap value is outside dimension bounds"
        );
    }
    Ok(values)
}

fn encode(values: &[u32; 256], height: u32, original: Option<&[i64]>) -> Result<Vec<i64>> {
    let bits = 32 - height.leading_zeros();
    let per_word = 64 / bits as usize;
    let length = 256usize.div_ceil(per_word);
    let mut words = match original {
        Some(raw) => {
            raw_values(raw, height)?;
            raw.to_vec()
        }
        None => vec![0; length],
    };
    let mask = (1u64 << bits) - 1;
    for (i, value) in values.iter().enumerate() {
        ensure!(*value <= height, "computed height exceeds dimension bounds");
        let shift = (i % per_word) * bits as usize;
        let word = words[i / per_word] as u64;
        words[i / per_word] = ((word & !(mask << shift)) | (u64::from(*value) << shift)) as i64;
    }
    Ok(words)
}

/// Rebuild a draft while verifying any original known heightmaps against its
/// original blocks. Unknown NBT and unused bits are retained byte-for-byte.
pub(crate) fn rebuild(
    original: &NamedTag,
    draft: &NamedTag,
    bounds: HeightBounds,
    trusted_existing: bool,
    mut classify: impl FnMut(&BlockState) -> Result<BlockHeightFlags>,
) -> Result<NamedTag> {
    let height = bounds.height()?;
    let source = unique(&original.value, "Heightmaps")?;
    let current = unique(&draft.value, "Heightmaps")?;
    let encoded = |tag: &Tag| {
        crate::nbt::write(&NamedTag {
            name: "".into(),
            value: tag.clone(),
        })
    };
    ensure!(
        trusted_existing
            || match (source, current) {
                (None, None) => true,
                (Some(a), Some(b)) => encoded(a)? == encoded(b)?,
                _ => false,
            },
        "heightmaps changed outside the draft"
    );
    let mut maps = match source {
        None => Tag::Compound(Vec::new()),
        Some(Tag::Compound(entries)) => Tag::Compound(entries.clone()),
        Some(_) => bail!("Heightmaps is not a compound"),
    };
    let air = BlockState {
        name: "minecraft:air".into(),
        properties: Vec::new(),
    };
    let air_flags = classify(&air)?;
    ensure!(air_flags.is_air, "default air was not classified as air");
    air_flags.mask()?;
    let mut cache = BTreeMap::from([(state_key(&air), 0)]);
    let baseline = calculate(original, bounds, &mut classify, &mut cache)?;
    let updated = calculate(draft, bounds, &mut classify, &mut cache)?;
    for (index, (name, predicate)) in KINDS.into_iter().enumerate() {
        let existing = unique(&maps, name)?;
        // The world-generation maps are retained only when already present.
        if index >= 4 && existing.is_none() {
            continue;
        }
        let raw = match existing {
            Some(Tag::LongArray(raw)) => {
                ensure!(
                    raw_values(raw, height)? == baseline[predicate],
                    "original {name} does not match source blocks"
                );
                Some(raw.clone())
            }
            None => None,
            _ => bail!("{name} is not a long array"),
        };
        let new = Tag::LongArray(encode(&updated[predicate], height, raw.as_deref())?);
        let Tag::Compound(entries) = &mut maps else {
            unreachable!()
        };
        if let Some((_, value)) = entries.iter_mut().find(|(key, _)| key.matches(name)) {
            *value = new;
        } else {
            entries.push((name.into(), new));
        }
    }
    let mut result = draft.clone();
    let Tag::Compound(entries) = &mut result.value else {
        bail!("chunk root is not a compound")
    };
    if let Some((_, value)) = entries
        .iter_mut()
        .find(|(key, _)| key.matches("Heightmaps"))
    {
        *value = maps;
    } else {
        entries.push(("Heightmaps".into(), maps));
    }
    crate::nbt::write(&result).context("cannot encode rebuilt heightmaps")?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        edit::{BlockChange, draft_chunk_blocks},
        nbt::TagType,
        world::ChunkPos,
    };

    fn block(name: &str) -> BlockState {
        BlockState {
            name: name.into(),
            properties: Vec::new(),
        }
    }

    fn classify(state: &BlockState) -> Result<BlockHeightFlags> {
        let name = state.name.to_string_lossy();
        Ok(match name.as_str() {
            "minecraft:air" | "minecraft:cave_air" => BlockHeightFlags {
                is_air: true,
                blocks_motion: false,
                blocks_motion_no_leaves: false,
                has_fluid: false,
            },
            "minecraft:stone" => BlockHeightFlags {
                is_air: false,
                blocks_motion: true,
                blocks_motion_no_leaves: true,
                has_fluid: false,
            },
            "minecraft:oak_leaves" => BlockHeightFlags {
                is_air: false,
                blocks_motion: true,
                blocks_motion_no_leaves: false,
                has_fluid: false,
            },
            "minecraft:water" => BlockHeightFlags {
                is_air: false,
                blocks_motion: false,
                blocks_motion_no_leaves: false,
                has_fluid: true,
            },
            "minecraft:short_grass" => BlockHeightFlags {
                is_air: false,
                blocks_motion: false,
                blocks_motion_no_leaves: false,
                has_fluid: false,
            },
            _ => bail!("unknown block {name}"),
        })
    }

    fn section(y: i8, names: &[&str], entries: &[(usize, usize, usize, u32)]) -> Tag {
        let mut indices = vec![0; 4096];
        for &(x, y, z, value) in entries {
            indices[x + z * 16 + y * 256] = value;
        }
        let mut storage = vec![(
            "palette".into(),
            Tag::List {
                element_type: TagType::String,
                values: names
                    .iter()
                    .map(|name| Tag::String((*name).into()))
                    .collect(),
            },
        )];
        if let Some(data) = palette::encode_indices(names.len(), &indices, 4).unwrap() {
            storage.push(("data".into(), Tag::LongArray(data)));
        }
        Tag::Compound(vec![
            ("Y".into(), Tag::Byte(y)),
            ("block_states".into(), Tag::Compound(storage)),
        ])
    }

    fn source() -> NamedTag {
        NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("xPos".into(), Tag::Int(0)),
                ("zPos".into(), Tag::Int(0)),
                ("Status".into(), Tag::String("minecraft:full".into())),
                ("future".into(), Tag::String("retain".into())),
                (
                    "sections".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![
                            section(-1, &["minecraft:air", "minecraft:stone"], &[(0, 0, 0, 1)]),
                            section(
                                0,
                                &["minecraft:air", "minecraft:water", "minecraft:oak_leaves"],
                                &[(0, 0, 0, 1), (0, 15, 0, 2)],
                            ),
                            section(
                                1,
                                &["minecraft:air", "minecraft:short_grass"],
                                &[(0, 15, 0, 1)],
                            ),
                        ],
                    },
                ),
            ]),
        }
    }

    fn bounds() -> HeightBounds {
        HeightBounds {
            min_y: -16,
            max_y: 32,
        }
    }
    fn maps(document: &NamedTag, name: &str) -> [u32; 256] {
        let Tag::LongArray(raw) = document.value.get("Heightmaps").unwrap().get(name).unwrap()
        else {
            panic!("missing heightmap");
        };
        raw_values(raw, 48).unwrap()
    }
    fn changes() -> Vec<BlockChange> {
        vec![
            BlockChange {
                x: 0,
                y: 31,
                z: 0,
                expected: block("minecraft:short_grass"),
                replacement: block("minecraft:air"),
            },
            BlockChange {
                x: 0,
                y: 15,
                z: 0,
                expected: block("minecraft:oak_leaves"),
                replacement: block("minecraft:air"),
            },
            BlockChange {
                x: 1,
                y: -8,
                z: 2,
                expected: block("minecraft:air"),
                replacement: block("minecraft:stone"),
            },
        ]
    }
    fn map_entries(document: &mut NamedTag) -> &mut Vec<(crate::nbt::JavaString, Tag)> {
        let Tag::Compound(root) = &mut document.value else {
            unreachable!()
        };
        let Tag::Compound(entries) = &mut root
            .iter_mut()
            .find(|(key, _)| key.matches("Heightmaps"))
            .unwrap()
            .1
        else {
            unreachable!()
        };
        entries
    }

    #[test]
    fn heightmaps_follow_distinct_predicates_and_negative_world_bounds() {
        let source = source();
        let pos = ChunkPos { x: 0, z: 0 };
        let mut initial = draft_chunk_blocks(&source, pos, &[]).unwrap();
        initial.rebuild_heightmaps(bounds(), classify).unwrap();
        assert_eq!(maps(initial.document(), "WORLD_SURFACE")[0], 48);
        assert_eq!(maps(initial.document(), "OCEAN_FLOOR")[0], 32);
        assert_eq!(maps(initial.document(), "MOTION_BLOCKING")[0], 32);
        assert_eq!(maps(initial.document(), "MOTION_BLOCKING_NO_LEAVES")[0], 17);
        let mut original = initial.document().clone();
        let entries = map_entries(&mut original);
        let world = entries
            .iter()
            .find(|(name, _)| name.matches("WORLD_SURFACE"))
            .unwrap()
            .1
            .clone();
        entries.push(("WORLD_SURFACE_WG".into(), world));
        entries.push(("custom_map".into(), Tag::String("unknown".into())));
        let Tag::LongArray(raw) = &mut entries
            .iter_mut()
            .find(|(name, _)| name.matches("WORLD_SURFACE"))
            .unwrap()
            .1
        else {
            unreachable!()
        };
        raw[0] |= i64::MIN;
        let before = crate::nbt::write(&original).unwrap();
        let mut draft = draft_chunk_blocks(&original, pos, &changes()).unwrap();
        draft.rebuild_heightmaps(bounds(), classify).unwrap();
        assert!(draft.matches_source(&original).unwrap());
        let result = draft.document();
        assert_eq!(maps(result, "WORLD_SURFACE")[0], 17);
        assert_eq!(maps(result, "OCEAN_FLOOR")[0], 1);
        assert_eq!(maps(result, "MOTION_BLOCKING")[0], 17);
        assert_eq!(maps(result, "MOTION_BLOCKING_NO_LEAVES")[0], 17);
        assert_eq!(maps(result, "WORLD_SURFACE_WG")[0], 17);
        for name in [
            "WORLD_SURFACE",
            "OCEAN_FLOOR",
            "MOTION_BLOCKING",
            "MOTION_BLOCKING_NO_LEAVES",
        ] {
            assert_eq!(maps(result, name)[1 + 2 * 16], 9);
        }
        let Tag::LongArray(raw) = result
            .value
            .get("Heightmaps")
            .unwrap()
            .get("WORLD_SURFACE")
            .unwrap()
        else {
            unreachable!()
        };
        assert_ne!(raw[0] & i64::MIN, 0);
        assert_eq!(
            result.value.get("Heightmaps").unwrap().get("custom_map"),
            Some(&Tag::String("unknown".into()))
        );
        assert_eq!(crate::nbt::write(&original).unwrap(), before);
        let first = crate::nbt::write(result).unwrap();
        draft.rebuild_heightmaps(bounds(), classify).unwrap();
        assert_eq!(crate::nbt::write(draft.document()).unwrap(), first);
    }

    #[test]
    fn stale_or_unsupported_heightmaps_leave_the_draft_unchanged() {
        let pos = ChunkPos { x: 0, z: 0 };
        let mut initial = draft_chunk_blocks(&source(), pos, &[]).unwrap();
        initial.rebuild_heightmaps(bounds(), classify).unwrap();
        let mut stale = initial.document().clone();
        let entries = map_entries(&mut stale);
        let Tag::LongArray(raw) = &mut entries
            .iter_mut()
            .find(|(name, _)| name.matches("WORLD_SURFACE"))
            .unwrap()
            .1
        else {
            unreachable!()
        };
        raw[0] ^= 1;
        let mut draft = draft_chunk_blocks(&stale, pos, &changes()).unwrap();
        let before = crate::nbt::write(draft.document()).unwrap();
        assert!(draft.rebuild_heightmaps(bounds(), classify).is_err());
        assert_eq!(crate::nbt::write(draft.document()).unwrap(), before);
        let mut draft = draft_chunk_blocks(&source(), pos, &changes()).unwrap();
        let before = crate::nbt::write(draft.document()).unwrap();
        assert!(
            draft
                .rebuild_heightmaps(bounds(), |state| {
                    if state.name.matches("minecraft:water") {
                        bail!("unsupported fluid state");
                    }
                    classify(state)
                })
                .is_err()
        );
        assert_eq!(crate::nbt::write(draft.document()).unwrap(), before);
        assert!(
            draft
                .rebuild_heightmaps(
                    HeightBounds {
                        min_y: -15,
                        max_y: 32
                    },
                    classify
                )
                .is_err()
        );
        assert!(
            draft
                .rebuild_heightmaps(bounds(), |state| {
                    let mut flags = classify(state)?;
                    if state.name.matches("minecraft:air") {
                        flags.blocks_motion = true;
                    }
                    Ok(flags)
                })
                .is_err()
        );
        assert_eq!(crate::nbt::write(draft.document()).unwrap(), before);
    }

    #[test]
    fn saved_spawn_heightmaps_preserve_stale_values_and_distinguish_missing_maps() {
        let bounds = HeightBounds {
            min_y: -64,
            max_y: 320,
        };
        let mut values = [0u32; 256];
        values[1] = 384;
        values[255] = 42;
        let mut raw = encode(&values, 384, None).unwrap();
        // Correctly sized raw arrays are accepted by Java even when the value
        // no longer corresponds to the blocks or lies above the build height.
        raw[0] = (raw[0] & !511) | 500;
        let document = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "Heightmaps".into(),
                Tag::Compound(vec![
                    ("MOTION_BLOCKING".into(), Tag::LongArray(raw)),
                    ("WORLD_SURFACE".into(), Tag::LongArray(vec![0])),
                    ("OCEAN_FLOOR".into(), Tag::String("invalid".into())),
                ]),
            )]),
        };
        let before = crate::nbt::write(&document).unwrap();
        let maps = SpawnHeightmaps::read(&document, bounds).unwrap();
        assert_eq!(
            maps.highest(SpawnHeightmapKind::MotionBlocking, 0, 0),
            Some(435)
        );
        assert_eq!(
            maps.highest(SpawnHeightmapKind::MotionBlocking, 1, 0),
            Some(319)
        );
        assert_eq!(
            maps.highest(SpawnHeightmapKind::MotionBlocking, -1, -1),
            Some(-23)
        );
        assert_eq!(maps.highest(SpawnHeightmapKind::WorldSurface, 0, 0), None);
        assert_eq!(maps.highest(SpawnHeightmapKind::OceanFloor, 0, 0), None);
        assert_eq!(crate::nbt::write(&document).unwrap(), before);
        assert!(
            SpawnHeightmaps::read(
                &document,
                HeightBounds {
                    min_y: -63,
                    max_y: 320
                }
            )
            .is_err()
        );
    }

    #[test]
    fn spawn_heightmap_priming_and_edits_are_atomic_on_incomplete_block_data() {
        let bounds = HeightBounds {
            min_y: 0,
            max_y: 16,
        };
        let empty = NamedTag {
            name: "".into(),
            value: Tag::Compound(Vec::new()),
        };
        let mut maps = SpawnHeightmaps::read(&empty, bounds).unwrap();
        let air = classify(&block("minecraft:air")).unwrap();
        let stone = classify(&block("minecraft:stone")).unwrap();
        let water = classify(&block("minecraft:water")).unwrap();
        assert!(
            maps.prime_missing(|x, _, _| {
                if x == 1 {
                    bail!("missing block")
                }
                Ok(stone)
            })
            .is_err()
        );
        assert_eq!(maps.highest(SpawnHeightmapKind::WorldSurface, 0, 0), None);
        assert!(
            maps.update_after_change(0, 1, 0, water, |_, _, _| Ok(air))
                .is_err()
        );
        maps.prime_missing(|_, y, _| {
            Ok(match y {
                0 => stone,
                2 => water,
                _ => air,
            })
        })
        .unwrap();
        for (kind, expected) in [
            (SpawnHeightmapKind::MotionBlocking, 2),
            (SpawnHeightmapKind::WorldSurface, 2),
            (SpawnHeightmapKind::OceanFloor, 0),
        ] {
            assert_eq!(maps.highest(kind, 0, 0), Some(expected));
        }
        assert!(
            maps.update_after_change(0, 2, 0, air, |_, _, _| bail!("unavailable"))
                .is_err()
        );
        assert_eq!(
            maps.highest(SpawnHeightmapKind::MotionBlocking, 0, 0),
            Some(2)
        );
        assert_eq!(
            maps.highest(SpawnHeightmapKind::WorldSurface, 0, 0),
            Some(2)
        );
        maps.update_after_change(0, 2, 0, air, |_, y, _| Ok(if y == 0 { stone } else { air }))
            .unwrap();
        assert_eq!(
            maps.highest(SpawnHeightmapKind::MotionBlocking, 0, 0),
            Some(0)
        );
        assert_eq!(
            maps.highest(SpawnHeightmapKind::WorldSurface, 0, 0),
            Some(0)
        );
        maps.update_after_change(0, 3, 0, stone, |_, _, _| Ok(air))
            .unwrap();
        assert_eq!(maps.highest(SpawnHeightmapKind::OceanFloor, 0, 0), Some(3));
    }

    #[test]
    fn priming_can_skip_known_empty_upper_sections_without_changing_results() {
        let bounds = HeightBounds {
            min_y: -64,
            max_y: 320,
        };
        let empty = NamedTag {
            name: "".into(),
            value: Tag::Compound(Vec::new()),
        };
        let mut full = SpawnHeightmaps::read(&empty, bounds).unwrap();
        let mut skipped = SpawnHeightmaps::read(&empty, bounds).unwrap();
        let air = classify(&block("minecraft:air")).unwrap();
        let stone = classify(&block("minecraft:stone")).unwrap();
        let mut full_queries = 0usize;
        full.prime_missing(|_, y, _| {
            full_queries += 1;
            Ok(if y == -64 { stone } else { air })
        })
        .unwrap();
        let mut skipped_queries = 0usize;
        skipped
            .prime_missing_through(-49, |_, y, _| {
                skipped_queries += 1;
                Ok(if y == -64 { stone } else { air })
            })
            .unwrap();
        assert_eq!((full_queries, skipped_queries), (384 * 256, 16 * 256));
        for kind in [
            SpawnHeightmapKind::MotionBlocking,
            SpawnHeightmapKind::WorldSurface,
            SpawnHeightmapKind::OceanFloor,
        ] {
            for z in 0..16 {
                for x in 0..16 {
                    assert_eq!(full.highest(kind, x, z), skipped.highest(kind, x, z));
                }
            }
        }
    }

    #[test]
    fn unsupported_unused_palette_state_cannot_hide_below_a_complete_heightmap() {
        let mut original = source();
        let Tag::Compound(root) = &mut original.value else {
            unreachable!()
        };
        let Tag::List { values, .. } = &mut root
            .iter_mut()
            .find(|(name, _)| name.matches("sections"))
            .unwrap()
            .1
        else {
            unreachable!()
        };
        *values = vec![section(0, &["minecraft:stone", "example:unknown"], &[])];
        let mut draft = draft_chunk_blocks(&original, ChunkPos { x: 0, z: 0 }, &[]).unwrap();
        let before = crate::nbt::write(draft.document()).unwrap();
        assert!(
            draft
                .rebuild_heightmaps(
                    HeightBounds {
                        min_y: 0,
                        max_y: 16,
                    },
                    classify,
                )
                .is_err()
        );
        assert_eq!(crate::nbt::write(draft.document()).unwrap(), before);
    }
}
