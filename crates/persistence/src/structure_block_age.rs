//! Java 26.3's per-block aging processor for generated structures.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, ensure};
use native_core::random::LegacyRandom;
use native_resources::{
    heightmap_tags::resolve_block_tag,
    packs::{ResourceId, ResourceProvider},
};
use native_world::loaded::BlockState;

/// Resolved from the active data packs, in Java's stairs/slabs/walls order.
#[derive(Clone, Debug)]
pub struct AgeTags {
    stairs: BTreeSet<String>,
    slabs: BTreeSet<String>,
    walls: BTreeSet<String>,
}

impl AgeTags {
    pub fn new(stairs: BTreeSet<String>, slabs: BTreeSet<String>, walls: BTreeSet<String>) -> Self {
        Self {
            stairs,
            slabs,
            walls,
        }
    }

    pub fn load(provider: &mut impl ResourceProvider, blocks: &BTreeSet<String>) -> Result<Self> {
        Ok(Self::new(
            resolve_block_tag(provider, &ResourceId::parse("minecraft:stairs")?, blocks)?,
            resolve_block_tag(provider, &ResourceId::parse("minecraft:slabs")?, blocks)?,
            resolve_block_tag(provider, &ResourceId::parse("minecraft:walls")?, blocks)?,
        ))
    }

    /// Age one template block. The caller owns and discards the random state
    /// on error, and must preserve the position and block entity data itself.
    pub fn process(
        &self,
        state: &BlockState,
        mossiness: f32,
        random: &mut LegacyRandom,
    ) -> Result<BlockState> {
        ensure!(mossiness.is_finite(), "invalid block aging mossiness");
        let replacement = if matches!(
            state.name.as_str(),
            "minecraft:stone_bricks" | "minecraft:stone" | "minecraft:chiseled_stone_bricks"
        ) {
            ensure!(state.properties.is_empty(), "invalid full stone state");
            if random.next_float() >= 0.5 {
                None
            } else {
                // Java constructs both candidate stair states eagerly, even
                // when the eventual replacement is a full block.
                let normal_stairs = random_stairs("stone_brick_stairs", random);
                let mossy_stairs = random_stairs("mossy_stone_brick_stairs", random);
                let mossy = random.next_float() < mossiness;
                let index = random.next_int_bound(2).unwrap();
                Some(match (mossy, index) {
                    (false, 0) => block("cracked_stone_bricks"),
                    (false, _) => normal_stairs,
                    (true, 0) => block("mossy_stone_bricks"),
                    (true, _) => mossy_stairs,
                })
            }
        } else if self.stairs.contains(&state.name) {
            if random.next_float() >= 0.5 {
                None
            } else {
                let mossy = random.next_float() < mossiness;
                let index = random.next_int_bound(2).unwrap();
                Some(match (mossy, index) {
                    (false, 0) => slab("stone_slab"),
                    (false, _) => slab("stone_brick_slab"),
                    (true, 0) => copy_stairs(state)?,
                    (true, _) => slab("mossy_stone_brick_slab"),
                })
            }
        } else if self.slabs.contains(&state.name) {
            if random.next_float() < mossiness {
                Some(copy_slab(state)?)
            } else {
                None
            }
        } else if self.walls.contains(&state.name) {
            if random.next_float() < mossiness {
                Some(copy_wall(state)?)
            } else {
                None
            }
        } else if state.name == "minecraft:obsidian" {
            ensure!(state.properties.is_empty(), "invalid obsidian state");
            if random.next_float() < 0.15 {
                Some(block("crying_obsidian"))
            } else {
                None
            }
        } else {
            None
        };
        Ok(replacement.unwrap_or_else(|| state.clone()))
    }
}

fn block(name: &str) -> BlockState {
    BlockState {
        name: format!("minecraft:{name}"),
        properties: BTreeMap::new(),
    }
}
fn slab(name: &str) -> BlockState {
    BlockState {
        name: format!("minecraft:{name}"),
        properties: BTreeMap::from([
            ("type".into(), "bottom".into()),
            ("waterlogged".into(), "false".into()),
        ]),
    }
}
fn random_stairs(name: &str, random: &mut LegacyRandom) -> BlockState {
    let facing = ["north", "east", "south", "west"][random.next_int_bound(4).unwrap() as usize];
    let half = ["top", "bottom"][random.next_int_bound(2).unwrap() as usize];
    BlockState {
        name: format!("minecraft:{name}"),
        properties: BTreeMap::from([
            ("facing".into(), facing.into()),
            ("half".into(), half.into()),
            ("shape".into(), "straight".into()),
            ("waterlogged".into(), "false".into()),
        ]),
    }
}

// WATERLOGGED is the same Java property object on every registered 26.3
// block that has it. Other similarly named properties can have different
// identities, so leave those cases to the schema-specific checks below.
fn copy_only_waterlogged(source: &BlockState, mut target: BlockState) -> Result<BlockState> {
    if let Some(value) = source.properties.get("waterlogged") {
        ensure!(
            matches!(value.as_str(), "true" | "false"),
            "invalid waterlogged state"
        );
        target
            .properties
            .insert("waterlogged".into(), value.clone());
    }
    Ok(target)
}

fn copy_stairs(source: &BlockState) -> Result<BlockState> {
    let p = &source.properties;
    if ["facing", "half", "shape"]
        .into_iter()
        .all(|name| !p.contains_key(name))
    {
        return copy_only_waterlogged(
            source,
            BlockState {
                name: "minecraft:mossy_stone_brick_stairs".into(),
                properties: BTreeMap::from([
                    ("facing".into(), "north".into()),
                    ("half".into(), "bottom".into()),
                    ("shape".into(), "straight".into()),
                    ("waterlogged".into(), "false".into()),
                ]),
            },
        );
    }
    ensure!(
        p.len() == 4
            && matches!(
                p.get("facing").map(String::as_str),
                Some("north" | "east" | "south" | "west")
            )
            && matches!(p.get("half").map(String::as_str), Some("top" | "bottom"))
            && matches!(
                p.get("shape").map(String::as_str),
                Some("straight" | "inner_left" | "inner_right" | "outer_left" | "outer_right")
            )
            && matches!(
                p.get("waterlogged").map(String::as_str),
                Some("true" | "false")
            ),
        "unsupported tagged stair state"
    );
    Ok(BlockState {
        name: "minecraft:mossy_stone_brick_stairs".into(),
        properties: p.clone(),
    })
}
fn copy_slab(source: &BlockState) -> Result<BlockState> {
    let p = &source.properties;
    if ["type"].into_iter().all(|name| !p.contains_key(name)) {
        return copy_only_waterlogged(source, slab("mossy_stone_brick_slab"));
    }
    ensure!(
        p.len() == 2
            && matches!(
                p.get("type").map(String::as_str),
                Some("top" | "bottom" | "double")
            )
            && matches!(
                p.get("waterlogged").map(String::as_str),
                Some("true" | "false")
            ),
        "unsupported tagged slab state"
    );
    Ok(BlockState {
        name: "minecraft:mossy_stone_brick_slab".into(),
        properties: p.clone(),
    })
}
fn copy_wall(source: &BlockState) -> Result<BlockState> {
    let p = &source.properties;
    if ["north", "east", "south", "west", "up"]
        .into_iter()
        .all(|name| !p.contains_key(name))
    {
        return copy_only_waterlogged(
            source,
            BlockState {
                name: "minecraft:mossy_stone_brick_wall".into(),
                properties: BTreeMap::from([
                    ("north".into(), "none".into()),
                    ("east".into(), "none".into()),
                    ("south".into(), "none".into()),
                    ("west".into(), "none".into()),
                    ("up".into(), "true".into()),
                    ("waterlogged".into(), "false".into()),
                ]),
            },
        );
    }
    ensure!(
        p.len() == 6
            && ["north", "east", "south", "west"]
                .into_iter()
                .all(|name| matches!(
                    p.get(name).map(String::as_str),
                    Some("none" | "low" | "tall")
                ))
            && ["up", "waterlogged"]
                .into_iter()
                .all(|name| matches!(p.get(name).map(String::as_str), Some("true" | "false"))),
        "unsupported tagged wall state"
    );
    Ok(BlockState {
        name: "minecraft:mossy_stone_brick_wall".into(),
        properties: p.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tagged_waterlogged_blocks_copy_only_the_shared_property() {
        let state = BlockState {
            name: "minecraft:iron_chain".into(),
            properties: BTreeMap::from([
                ("axis".into(), "x".into()),
                ("waterlogged".into(), "true".into()),
            ]),
        };
        for result in [copy_stairs(&state), copy_slab(&state), copy_wall(&state)] {
            let result = result.unwrap();
            assert_eq!(
                result.properties.get("waterlogged").map(String::as_str),
                Some("true")
            );
            assert!(!result.properties.contains_key("axis"));
        }
        let invalid = BlockState {
            properties: BTreeMap::from([("waterlogged".into(), "invalid".into())]),
            ..state
        };
        assert!(copy_stairs(&invalid).is_err());
        assert!(copy_slab(&invalid).is_err());
        assert!(copy_wall(&invalid).is_err());
    }

    #[test]
    fn rejects_unsupported_states_in_custom_tags() {
        let tags = AgeTags::new(
            BTreeSet::new(),
            BTreeSet::from(["example:custom".into()]),
            BTreeSet::new(),
        );
        let state = BlockState {
            name: "example:custom".into(),
            properties: BTreeMap::from([("type".into(), "unknown".into())]),
        };
        assert!(
            tags.process(&state, 1.0, &mut LegacyRandom::new(0))
                .is_err()
        );
        assert_eq!(
            state.properties.get("type").map(String::as_str),
            Some("unknown")
        );
        assert!(
            tags.process(&state, f32::NAN, &mut LegacyRandom::new(0))
                .is_err()
        );
    }

    #[test]
    fn stairs_take_precedence_over_overlapping_tags() {
        let name = "minecraft:oak_stairs".to_string();
        let tags = AgeTags::new(
            BTreeSet::from([name.clone()]),
            BTreeSet::from([name.clone()]),
            BTreeSet::from([name.clone()]),
        );
        let state = BlockState {
            name,
            properties: BTreeMap::from([
                ("facing".into(), "east".into()),
                ("half".into(), "top".into()),
                ("shape".into(), "inner_left".into()),
                ("waterlogged".into(), "true".into()),
            ]),
        };
        let seed = (0..100_000)
            .find(|&seed| LegacyRandom::new(seed).next_float() < 0.5)
            .unwrap();
        let result = tags
            .process(&state, 1.0, &mut LegacyRandom::new(seed))
            .unwrap();
        assert!(matches!(
            result.name.as_str(),
            "minecraft:mossy_stone_brick_stairs" | "minecraft:mossy_stone_brick_slab"
        ));
        if result.name.ends_with("stairs") {
            assert_eq!(result.properties, state.properties);
        }
    }
}
