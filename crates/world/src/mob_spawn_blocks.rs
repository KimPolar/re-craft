//! Verified, context-independent block facts for on-ground mob placement.
//!
//! The prevent-mob-spawning tag is data-pack controlled and must be queried
//! separately. Unknown blocks or malformed states are never inferred here.
use crate::loaded::BlockState;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MobSpawnBlockFacts {
    pub air: bool,
    pub pathfindable_land: bool,
    pub valid_support: bool,
    pub full_collision: bool,
    pub signal_source: bool,
    pub fluid_empty: bool,
    pub dangerous: bool,
}

/// Facts whose support and danger predicates are independent of entity type.
pub fn common_mob_spawn_block_facts(state: &BlockState) -> Option<MobSpawnBlockFacts> {
    let air = match state.name.as_str() {
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
            if state.properties.is_empty() =>
        {
            true
        }
        "minecraft:stone" | "minecraft:dirt" if state.properties.is_empty() => false,
        "minecraft:grass_block"
            if state.properties.len() == 1
                && matches!(
                    state.properties.get("snowy").map(String::as_str),
                    Some("false")
                ) =>
        {
            false
        }
        _ => return None,
    };
    Some(MobSpawnBlockFacts {
        air,
        pathfindable_land: air,
        valid_support: !air,
        full_collision: !air,
        signal_source: false,
        fluid_empty: true,
        dangerous: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    #[test]
    fn unsupported_or_malformed_blocks_remain_unknown() {
        let mut state = BlockState {
            name: "minecraft:grass_block".into(),
            properties: BTreeMap::new(),
        };
        assert!(common_mob_spawn_block_facts(&state).is_none());
        state.properties.insert("snowy".into(), "invalid".into());
        assert!(common_mob_spawn_block_facts(&state).is_none());
        state.name = "example:custom".into();
        assert!(common_mob_spawn_block_facts(&state).is_none());
    }
}
