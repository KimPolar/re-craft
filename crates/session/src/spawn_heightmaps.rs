//! Prepare and maintain the heightmaps needed by shared-spawn searches.
//!
//! Preparation can run on the chunk reader thread. Classification uses the
//! active world's data-pack tags, not resource-pack block models.

use anyhow::{Context, Result, ensure};
use native_persistence::{
    heightmap::{BlockHeightFlags, HeightBounds, SpawnHeightmaps},
    world::LoadedChunk,
};
use native_resources::heightmap_tags::HeightmapTags;
use native_world::{
    fluid::FluidState,
    loaded::{BlockState, Chunk, ChunkPos, World},
};

pub(crate) fn classify(state: &BlockState, tags: &HeightmapTags) -> Result<BlockHeightFlags> {
    if let Some(value) = state.properties.get("waterlogged") {
        ensure!(
            value == "true" || value == "false",
            "invalid waterlogged state"
        );
    }
    let motion = tags.for_block(&state.name)?;
    let fluid = FluidState::from_block(state).map_err(anyhow::Error::msg)?;
    Ok(BlockHeightFlags {
        is_air: matches!(
            state.name.as_str(),
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
        ),
        blocks_motion: motion.blocks_motion,
        blocks_motion_no_leaves: motion.blocks_motion_no_leaves,
        has_fluid: fluid.is_some(),
    })
}

pub(crate) fn air_flags(tags: &HeightmapTags) -> Result<BlockHeightFlags> {
    classify(
        &BlockState {
            name: "minecraft:air".into(),
            properties: Default::default(),
        },
        tags,
    )
}

pub(crate) fn classify_states(
    world: &World,
    tags: &HeightmapTags,
) -> Vec<Option<BlockHeightFlags>> {
    world
        .states()
        .map(|(_, state)| classify(state, tags).ok())
        .collect()
}

pub(crate) fn block_flags(
    chunk: &Chunk,
    flags: &[Option<BlockHeightFlags>],
    air: BlockHeightFlags,
    x: i32,
    y: i32,
    z: i32,
) -> Result<BlockHeightFlags> {
    let section_y = i8::try_from(y.div_euclid(16)).context("heightmap section is out of range")?;
    let Some(blocks) = chunk
        .sections
        .get(&section_y)
        .and_then(|section| section.blocks.as_ref())
    else {
        // Empty and light-only sections are initialized as air by Java.
        return Ok(air);
    };
    let index = x.rem_euclid(16) as usize
        + z.rem_euclid(16) as usize * 16
        + y.rem_euclid(16) as usize * 256;
    flags
        .get(blocks[index].0 as usize)
        .copied()
        .flatten()
        .context("block heightmap behavior is unavailable")
}

/// Read Java's stored values and prime only missing maps. This never edits the
/// world or its source NBT; an unsupported block leaves the maps unavailable.
pub fn prepare(
    saved: &LoadedChunk,
    world: &World,
    bounds: HeightBounds,
    tags: &HeightmapTags,
) -> Result<SpawnHeightmaps> {
    let (chunk, mut maps) = read(saved, world, bounds)?;
    if !maps.is_complete() {
        let flags = classify_states(world, tags);
        let air = air_flags(tags)?;
        prime(&mut maps, chunk, &flags, air)?;
    }
    Ok(maps)
}

pub(crate) fn prepare_with_flags(
    saved: &LoadedChunk,
    world: &World,
    bounds: HeightBounds,
    flags: &[Option<BlockHeightFlags>],
    air: BlockHeightFlags,
) -> Result<SpawnHeightmaps> {
    let (chunk, mut maps) = read(saved, world, bounds)?;
    prime(&mut maps, chunk, flags, air)?;
    Ok(maps)
}

fn prime(
    maps: &mut SpawnHeightmaps,
    chunk: &Chunk,
    flags: &[Option<BlockHeightFlags>],
    air: BlockHeightFlags,
) -> Result<()> {
    if maps.is_complete() {
        return Ok(());
    }
    let bounds = maps.bounds();
    let top = chunk
        .sections
        .iter()
        .rev()
        .find_map(|(&section_y, section)| {
            let y = i32::from(section_y) * 16;
            if y < bounds.min_y || y >= bounds.max_y {
                return None;
            }
            section.blocks.as_ref().and_then(|blocks| {
                blocks
                    .iter()
                    .any(|id| {
                        flags
                            .get(id.0 as usize)
                            .copied()
                            .flatten()
                            .is_none_or(|flags| !flags.is_air)
                    })
                    .then_some(y + 15)
            })
        })
        .unwrap_or(bounds.min_y - 1);
    maps.prime_missing_through(top, |x, y, z| block_flags(chunk, flags, air, x, y, z))
}

fn read<'a>(
    saved: &LoadedChunk,
    world: &'a World,
    bounds: HeightBounds,
) -> Result<(&'a Chunk, SpawnHeightmaps)> {
    let pos = ChunkPos {
        x: saved.position.x,
        z: saved.position.z,
    };
    let chunk = world
        .chunks
        .get(&pos)
        .context("heightmap chunk is missing")?;
    ensure!(
        chunk.position == pos && saved.is_full(),
        "heightmap chunk is incomplete"
    );
    ensure!(
        chunk.status.as_deref() == Some("minecraft:full"),
        "runtime chunk is incomplete"
    );
    Ok((chunk, SpawnHeightmaps::read(&saved.document, bounds)?))
}
