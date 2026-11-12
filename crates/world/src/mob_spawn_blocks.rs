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

/// Valid leaves expose these entity-independent facts even though their
/// spawn-support predicate depends on the entity type.
pub fn mob_spawn_leaf_waterlogged(state: &BlockState) -> Option<bool> {
    if !matches!(
        state.name.as_str(),
        "minecraft:oak_leaves"
            | "minecraft:spruce_leaves"
            | "minecraft:birch_leaves"
            | "minecraft:jungle_leaves"
            | "minecraft:acacia_leaves"
            | "minecraft:dark_oak_leaves"
            | "minecraft:mangrove_leaves"
            | "minecraft:cherry_leaves"
            | "minecraft:pale_oak_leaves"
            | "minecraft:azalea_leaves"
            | "minecraft:flowering_azalea_leaves"
    ) || state.properties.len() != 3
        || !matches!(
            state.properties.get("persistent").map(String::as_str),
            Some("true" | "false")
        )
        || !matches!(
            state.properties.get("distance").map(String::as_str),
            Some("1" | "2" | "3" | "4" | "5" | "6" | "7")
        )
    {
        return None;
    }
    match state.properties.get("waterlogged").map(String::as_str) {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    }
}

/// Spawn support for entity types whose leaf predicate has been compared
/// with the built-in Java registry. Unknown types remain unresolved.
pub fn mob_spawn_leaf_support(state: &BlockState, namespace: &str, entity: &str) -> Option<bool> {
    mob_spawn_leaf_waterlogged(state)?;
    if namespace != "minecraft" {
        return None;
    }
    match entity {
        "ocelot" | "parrot" => Some(true),
        "pig" | "cow" | "sheep" | "goat" | "chicken" | "horse" | "donkey" | "rabbit" | "slime"
        | "magma_cube" | "sulfur_cube" => Some(false),
        _ => None,
    }
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
        "minecraft:brown_mushroom_block"
        | "minecraft:red_mushroom_block"
        | "minecraft:mushroom_stem"
            if state.properties.len() == 6
                && ["down", "up", "north", "south", "west", "east"]
                    .into_iter()
                    .all(|key| {
                        matches!(
                            state.properties.get(key).map(String::as_str),
                            Some("false" | "true")
                        )
                    }) =>
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
        "minecraft:leaf_litter" if crate::collision::valid_leaf_litter(state) => {
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
    fn leaves_expose_waterlogging_but_not_universal_spawn_support() {
        let mut leaves = BlockState {
            name: "minecraft:oak_leaves".into(),
            properties: [
                ("distance".into(), "3".into()),
                ("persistent".into(), "false".into()),
                ("waterlogged".into(), "true".into()),
            ]
            .into(),
        };
        assert_eq!(mob_spawn_leaf_waterlogged(&leaves), Some(true));
        assert!(common_mob_spawn_block_facts(&leaves).is_none());
        assert_eq!(
            mob_spawn_leaf_support(&leaves, "minecraft", "ocelot"),
            Some(true)
        );
        assert_eq!(
            mob_spawn_leaf_support(&leaves, "minecraft", "pig"),
            Some(false)
        );
        assert_eq!(mob_spawn_leaf_support(&leaves, "example", "pig"), None);
        assert_eq!(
            mob_spawn_leaf_support(&leaves, "minecraft", "unknown"),
            None
        );
        leaves
            .properties
            .insert("waterlogged".into(), "false".into());
        assert_eq!(mob_spawn_leaf_waterlogged(&leaves), Some(false));
        leaves.properties.insert("distance".into(), "0".into());
        assert!(mob_spawn_leaf_waterlogged(&leaves).is_none());
        leaves.properties.insert("distance".into(), "7".into());
        leaves.properties.insert("persistent".into(), "yes".into());
        assert!(mob_spawn_leaf_waterlogged(&leaves).is_none());
        leaves.properties.insert("persistent".into(), "true".into());
        leaves.properties.insert("extra".into(), "true".into());
        assert!(mob_spawn_leaf_waterlogged(&leaves).is_none());
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
        state.name = "minecraft:brown_mushroom_block".into();
        state.properties = ["down", "up", "north", "south", "west", "east"]
            .into_iter()
            .map(|face| (face.into(), "true".into()))
            .collect();
        assert!(common_mob_spawn_block_facts(&state).unwrap().valid_support);
        state.properties.insert("up".into(), "invalid".into());
        assert!(common_mob_spawn_block_facts(&state).is_none());
        state.name = "minecraft:leaf_litter".into();
        state.properties = [
            ("facing".into(), "north".into()),
            ("segment_amount".into(), "2".into()),
        ]
        .into();
        assert!(
            common_mob_spawn_block_facts(&state)
                .unwrap()
                .pathfindable_land
        );
        assert!(crate::collision::block_shape(&state).unwrap().is_empty());
        state.properties.insert("segment_amount".into(), "5".into());
        assert!(common_mob_spawn_block_facts(&state).is_none());
        assert!(crate::collision::block_shape(&state).is_none());
    }
}
