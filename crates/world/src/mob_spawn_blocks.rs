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
    let empty = state.properties.is_empty();
    let property = |key: &str, values: &[&str]| {
        state.properties.len() == 1
            && state
                .properties
                .get(key)
                .is_some_and(|value| values.contains(&value.as_str()))
    };
    let (
        air,
        pathfindable_land,
        valid_support,
        full_collision,
        signal_source,
        fluid_empty,
        dangerous,
    ) = match state.name.as_str() {
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" if empty => {
            (true, true, false, false, false, true, false)
        }
        "minecraft:stone"
        | "minecraft:dirt"
        | "minecraft:coarse_dirt"
        | "minecraft:rooted_dirt"
        | "minecraft:gravel"
        | "minecraft:sand"
        | "minecraft:red_sand"
        | "minecraft:granite"
        | "minecraft:diorite"
        | "minecraft:andesite"
        | "minecraft:tuff"
        | "minecraft:calcite"
        | "minecraft:clay"
        | "minecraft:moss_block"
        | "minecraft:snow_block"
        | "minecraft:packed_ice"
        | "minecraft:coal_ore"
        | "minecraft:iron_ore"
        | "minecraft:copper_ore"
        | "minecraft:gold_ore"
        | "minecraft:emerald_ore"
        | "minecraft:lapis_ore"
        | "minecraft:diamond_ore"
        | "minecraft:deepslate_coal_ore"
        | "minecraft:deepslate_iron_ore"
        | "minecraft:deepslate_copper_ore"
        | "minecraft:deepslate_gold_ore"
        | "minecraft:deepslate_emerald_ore"
        | "minecraft:deepslate_lapis_ore"
        | "minecraft:deepslate_diamond_ore"
            if empty =>
        {
            (false, false, true, true, false, true, false)
        }
        "minecraft:grass_block" | "minecraft:podzol" | "minecraft:mycelium"
            if property("snowy", &["false", "true"]) =>
        {
            (false, false, true, true, false, true, false)
        }
        "minecraft:deepslate" | "minecraft:oak_log" | "minecraft:birch_log"
            if property("axis", &["x", "y", "z"]) =>
        {
            (false, false, true, true, false, true, false)
        }
        "minecraft:redstone_ore" | "minecraft:deepslate_redstone_ore"
            if property("lit", &["false", "true"]) =>
        {
            (false, false, true, true, false, true, false)
        }
        "minecraft:bedrock" | "minecraft:glass" if empty => {
            (false, false, false, true, false, true, false)
        }
        "minecraft:redstone_block" if empty => (false, false, true, true, true, true, false),
        "minecraft:short_grass"
        | "minecraft:fern"
        | "minecraft:dead_bush"
        | "minecraft:dandelion"
        | "minecraft:poppy"
        | "minecraft:torch"
        | "minecraft:moss_carpet"
            if empty =>
        {
            (false, true, false, false, false, true, false)
        }
        "minecraft:tall_grass" if property("half", &["upper", "lower"]) => {
            (false, true, false, false, false, true, false)
        }
        "minecraft:seagrass" if empty => (false, true, false, false, false, false, false),
        "minecraft:water"
            if property(
                "level",
                &[
                    "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14",
                    "15",
                ],
            ) =>
        {
            (false, true, false, false, false, false, false)
        }
        "minecraft:farmland" if property("moisture", &["0", "1", "2", "3", "4", "5", "6", "7"]) => {
            (false, false, false, false, false, true, false)
        }
        _ => return None,
    };
    Some(MobSpawnBlockFacts {
        air,
        pathfindable_land,
        valid_support,
        full_collision,
        signal_source,
        fluid_empty,
        dangerous,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn terrain_plant_fluid_and_signal_facts_are_distinct() {
        let facts = |name: &str, property: Option<(&str, &str)>| {
            let mut properties = BTreeMap::new();
            if let Some((key, value)) = property {
                properties.insert(key.into(), value.into());
            }
            common_mob_spawn_block_facts(&BlockState {
                name: format!("minecraft:{name}"),
                properties,
            })
            .unwrap()
        };
        let ground = facts("sand", None);
        assert!(ground.valid_support && ground.full_collision && !ground.pathfindable_land);
        let grass = facts("short_grass", None);
        assert!(
            grass.pathfindable_land && grass.fluid_empty && !grass.air && !grass.full_collision
        );
        let water = facts("water", Some(("level", "8")));
        assert!(water.pathfindable_land && !water.fluid_empty && !water.full_collision);
        let bedrock = facts("bedrock", None);
        assert!(bedrock.full_collision && !bedrock.valid_support);
        let redstone = facts("redstone_block", None);
        assert!(redstone.signal_source && redstone.valid_support);
    }

    #[test]
    fn unsupported_or_malformed_blocks_remain_unknown() {
        let mut state = BlockState {
            name: "minecraft:grass_block".into(),
            properties: BTreeMap::new(),
        };
        assert!(common_mob_spawn_block_facts(&state).is_none());
        state.properties.insert("snowy".into(), "invalid".into());
        assert!(common_mob_spawn_block_facts(&state).is_none());
        state.name = "minecraft:water".into();
        state.properties.clear();
        state.properties.insert("level".into(), "16".into());
        assert!(common_mob_spawn_block_facts(&state).is_none());
        state.properties.insert("level".into(), "0".into());
        state.properties.insert("extra".into(), "false".into());
        assert!(common_mob_spawn_block_facts(&state).is_none());
        state.name = "minecraft:cactus".into();
        state.properties.clear();
        state.properties.insert("age".into(), "0".into());
        // Entity immunity is controlled by active block tags.
        assert!(common_mob_spawn_block_facts(&state).is_none());
        state.name = "example:custom".into();
        assert!(common_mob_spawn_block_facts(&state).is_none());
    }
}
