//! Encode the tick lists belonging to an intermediate generated protochunk.
use std::collections::BTreeSet;

use anyhow::{Result, ensure};
use native_resources::packs::ResourceId;
use native_world::loaded::{ChunkPos, SavedBlockTick, SavedFluidTick};

use crate::nbt::{Tag, TagType};

fn encode<'a>(
    pos: ChunkPos,
    ticks: impl IntoIterator<Item = (&'a str, [i32; 3], i32, i32)>,
    mut known: impl FnMut(&str) -> bool,
    proto: bool,
) -> Result<Tag> {
    let mut keys = BTreeSet::new();
    let mut output = Vec::new();
    for (name, position, delay, priority) in ticks {
        let id = ResourceId::parse(name)?;
        let name = format!("{}:{}", id.namespace, id.path);
        ensure!(known(&name), "unknown generated tick type {name}");
        ensure!(
            position[0].div_euclid(16) == pos.x && position[2].div_euclid(16) == pos.z,
            "generated tick is outside its chunk"
        );
        ensure!(
            !proto || delay == 0,
            "generated protochunk tick has a nonzero delay"
        );
        ensure!(
            (-3..=3).contains(&priority),
            "generated tick priority is invalid"
        );
        ensure!(
            !proto || keys.insert((name.clone(), position)),
            "duplicate generated tick"
        );
        output.push(Tag::Compound(vec![
            ("i".into(), Tag::String(name.as_str().into())),
            ("x".into(), Tag::Int(position[0])),
            ("y".into(), Tag::Int(position[1])),
            ("z".into(), Tag::Int(position[2])),
            ("t".into(), Tag::Int(delay)),
            ("p".into(), Tag::Int(priority)),
        ]));
    }
    Ok(Tag::List {
        element_type: if output.is_empty() {
            TagType::End
        } else {
            TagType::Compound
        },
        values: output,
    })
}

/// Preserve ProtoChunkTicks insertion order; duplicate identities are rejected
/// because loading them would discard entries. Validate IDs using the same
/// block registry that generated the chunk. This alone does not authorize saving.
pub fn encode_block_ticks(
    pos: ChunkPos,
    ticks: &[SavedBlockTick],
    known_block: impl FnMut(&str) -> bool,
) -> Result<Tag> {
    encode(
        pos,
        ticks.iter().map(|tick| {
            (
                tick.block.as_str(),
                tick.position,
                tick.delay,
                tick.priority,
            )
        }),
        known_block,
        true,
    )
}

/// The fluid registry is built into Java 26.3 and cannot be extended by data packs.
pub fn encode_fluid_ticks(pos: ChunkPos, ticks: &[SavedFluidTick]) -> Result<Tag> {
    encode(
        pos,
        ticks.iter().map(|tick| {
            (
                tick.fluid.as_str(),
                tick.position,
                tick.delay,
                tick.priority,
            )
        }),
        known_fluid,
        true,
    )
}

fn known_fluid(name: &str) -> bool {
    matches!(
        name,
        "minecraft:empty"
            | "minecraft:water"
            | "minecraft:flowing_water"
            | "minecraft:lava"
            | "minecraft:flowing_lava"
    )
}

/// Encode LevelChunkTicks.pack output in its supplied order. Unlike protochunk
/// ticks, delayed entries and duplicate pending identities are valid.
pub fn encode_level_block_ticks(
    pos: ChunkPos,
    ticks: &[SavedBlockTick],
    known_block: impl FnMut(&str) -> bool,
) -> Result<Tag> {
    encode(
        pos,
        ticks.iter().map(|tick| {
            (
                tick.block.as_str(),
                tick.position,
                tick.delay,
                tick.priority,
            )
        }),
        known_block,
        false,
    )
}

/// Encode LevelChunk fluid ticks without discarding saved pending entries.
pub fn encode_level_fluid_ticks(pos: ChunkPos, ticks: &[SavedFluidTick]) -> Result<Tag> {
    encode(
        pos,
        ticks.iter().map(|tick| {
            (
                tick.fluid.as_str(),
                tick.position,
                tick.delay,
                tick.priority,
            )
        }),
        known_fluid,
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_and_duplicate_ticks_without_losing_data() {
        let pos = ChunkPos { x: -2, z: 3 };
        let good = SavedBlockTick {
            block: "sand".into(),
            position: [-32, -65, 63],
            delay: 0,
            priority: -3,
        };
        let valid = |id: &str| id == "minecraft:sand";
        assert!(encode_block_ticks(pos, std::slice::from_ref(&good), valid).is_ok());
        let duplicate = SavedBlockTick {
            block: "minecraft:sand".into(),
            priority: 3,
            ..good.clone()
        };
        assert!(encode_block_ticks(pos, &[good.clone(), duplicate], valid).is_err());
        assert!(
            encode_block_ticks(
                pos,
                &[SavedBlockTick {
                    delay: 1,
                    ..good.clone()
                }],
                valid
            )
            .is_err()
        );
        assert!(
            encode_block_ticks(
                pos,
                &[SavedBlockTick {
                    priority: 4,
                    ..good.clone()
                }],
                valid
            )
            .is_err()
        );
        assert!(
            encode_block_ticks(
                pos,
                &[SavedBlockTick {
                    position: [-33, 0, 63],
                    ..good.clone()
                }],
                valid
            )
            .is_err()
        );
        assert!(encode_block_ticks(pos, &[good], |_| false).is_err());
        let fluid = SavedFluidTick {
            fluid: "example:unknown".into(),
            position: [-32, 0, 63],
            delay: 0,
            priority: 0,
        };
        assert!(encode_fluid_ticks(pos, &[fluid]).is_err());
    }

    #[test]
    fn level_ticks_preserve_delays_and_pending_duplicates() {
        let pos = ChunkPos { x: -2, z: 3 };
        let first = SavedBlockTick {
            block: "sand".into(),
            position: [-31, 15, 48],
            delay: -3,
            priority: -1,
        };
        let second = SavedBlockTick {
            delay: 9,
            priority: 3,
            ..first.clone()
        };
        let tag =
            encode_level_block_ticks(pos, &[first, second], |id| id == "minecraft:sand").unwrap();
        let Tag::List { values, .. } = tag else {
            panic!("missing ticks")
        };
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].get("t"), Some(&Tag::Int(-3)));
        assert_eq!(values[1].get("t"), Some(&Tag::Int(9)));
    }
}
