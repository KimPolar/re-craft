//! Redstone signal queries for the currently simulated sources.
//!
//! Unknown blocks propagate uncertainty, so a consumer is never switched off
//! merely because its real source has not been implemented yet.

use glam::IVec3;
use native_world::loaded::BlockState;

const DIRECTIONS: [IVec3; 6] = [
    IVec3::NEG_Y,
    IVec3::Y,
    IVec3::NEG_Z,
    IVec3::Z,
    IVec3::NEG_X,
    IVec3::X,
];

fn add(a: IVec3, b: IVec3) -> Option<IVec3> {
    Some(IVec3::new(
        a.x.checked_add(b.x)?,
        a.y.checked_add(b.y)?,
        a.z.checked_add(b.z)?,
    ))
}

fn attached_source_direction(state: &BlockState) -> Option<IVec3> {
    if native_core::button::Button::from_id(&state.name).is_some() {
        native_world::collision::block_outline(state)?;
    } else if state.name != "minecraft:lever" || state.properties.len() != 3 {
        return None;
    }
    if !matches!(
        state.properties.get("facing").map(String::as_str),
        Some("north" | "south" | "west" | "east")
    ) {
        return None;
    }
    match state.properties.get("face")?.as_str() {
        "floor" => Some(IVec3::Y),
        "ceiling" => Some(IVec3::NEG_Y),
        "wall" => match state.properties.get("facing")?.as_str() {
            "north" => Some(IVec3::NEG_Z),
            "south" => Some(IVec3::Z),
            "west" => Some(IVec3::NEG_X),
            "east" => Some(IVec3::X),
            _ => None,
        },
        _ => None,
    }
}

/// Return weak signal, direct signal in the query direction, and whether the
/// block relays direct power from its neighbours. None means not yet modeled.
pub fn source(state: &BlockState, direction: IVec3) -> Option<(u8, u8, bool)> {
    if !DIRECTIONS.contains(&direction) {
        return None;
    }
    if state.name == "minecraft:chest" {
        native_world::collision::block_shape(state)?;
        return Some((0, 0, false));
    }
    if state.name == "minecraft:lever"
        || native_core::button::Button::from_id(&state.name).is_some()
    {
        let connected = attached_source_direction(state)?;
        let powered = match state.properties.get("powered")?.as_str() {
            "true" => true,
            "false" => false,
            _ => return None,
        };
        return Some((
            if powered { 15 } else { 0 },
            if powered && direction == connected {
                15
            } else {
                0
            },
            false,
        ));
    }
    let empty = state.properties.is_empty();
    let axis = state.properties.len() == 1
        && matches!(
            state.properties.get("axis").map(String::as_str),
            Some("x" | "y" | "z")
        );
    let id = state.name.as_str();
    if (axis
        && (id.ends_with("_log")
            || id.ends_with("_wood")
            || id.ends_with("_stem")
            || id.ends_with("_hyphae")
            || matches!(
                id,
                "minecraft:bamboo_block" | "minecraft:stripped_bamboo_block"
            ))
        && native_core::wood::Wood::from_id(id).is_some())
        || (empty && id.ends_with("_planks") && native_core::wood::Planks::from_id(id).is_some())
    {
        return Some((0, 0, true));
    }
    match state.name.as_str() {
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" if empty => {
            Some((0, 0, false))
        }
        "minecraft:glass"
        | "minecraft:ice"
        | "minecraft:honey_block"
        | "minecraft:glowstone"
        | "minecraft:dirt_path"
            if empty =>
        {
            Some((0, 0, false))
        }
        "minecraft:farmland"
            if state.properties.len() == 1
                && matches!(
                    state.properties.get("moisture").map(String::as_str),
                    Some("0" | "1" | "2" | "3" | "4" | "5" | "6" | "7")
                ) =>
        {
            Some((0, 0, false))
        }
        "minecraft:redstone_block" if empty => Some((15, 0, false)),
        "minecraft:water" | "minecraft:lava" if state.properties.len() == 1 => {
            let level = state.properties.get("level")?;
            let value = level.parse::<u8>().ok()?;
            (value <= 15 && value.to_string() == *level).then_some((0, 0, false))
        }
        "minecraft:grass_block" | "minecraft:podzol" | "minecraft:mycelium"
            if state.properties.len() == 1
                && matches!(
                    state.properties.get("snowy").map(String::as_str),
                    Some("true" | "false")
                ) =>
        {
            Some((0, 0, true))
        }
        "minecraft:basalt" | "minecraft:hay_block" if axis => Some((0, 0, true)),
        "minecraft:stone"
        | "minecraft:bedrock"
        | "minecraft:cobblestone"
        | "minecraft:mossy_cobblestone"
        | "minecraft:stone_bricks"
        | "minecraft:lodestone"
        | "minecraft:mossy_stone_bricks"
        | "minecraft:cracked_stone_bricks"
        | "minecraft:chiseled_stone_bricks"
        | "minecraft:crafting_table"
        | "minecraft:cobbled_deepslate"
        | "minecraft:polished_deepslate"
        | "minecraft:deepslate_bricks"
        | "minecraft:deepslate_tiles"
        | "minecraft:sandstone"
        | "minecraft:red_sandstone"
        | "minecraft:coal_block"
        | "minecraft:iron_block"
        | "minecraft:gold_block"
        | "minecraft:diamond_block"
        | "minecraft:emerald_block"
        | "minecraft:lapis_block"
        | "minecraft:coal_ore"
        | "minecraft:deepslate_coal_ore"
        | "minecraft:iron_ore"
        | "minecraft:deepslate_iron_ore"
        | "minecraft:gold_ore"
        | "minecraft:deepslate_gold_ore"
        | "minecraft:diamond_ore"
        | "minecraft:deepslate_diamond_ore"
        | "minecraft:emerald_ore"
        | "minecraft:deepslate_emerald_ore"
        | "minecraft:lapis_ore"
        | "minecraft:deepslate_lapis_ore"
        | "minecraft:dirt"
        | "minecraft:coarse_dirt"
        | "minecraft:rooted_dirt"
        | "minecraft:sand"
        | "minecraft:red_sand"
        | "minecraft:gravel"
        | "minecraft:granite"
        | "minecraft:diorite"
        | "minecraft:andesite"
        | "minecraft:tuff"
        | "minecraft:netherrack"
        | "minecraft:end_stone"
        | "minecraft:obsidian"
        | "minecraft:soul_soil"
        | "minecraft:snow_block"
        | "minecraft:slime_block"
        | "minecraft:packed_ice"
        | "minecraft:blue_ice"
        | "minecraft:clay"
            if empty =>
        {
            Some((0, 0, true))
        }
        "minecraft:redstone_lamp"
            if state.properties.len() == 1
                && matches!(
                    state.properties.get("lit").map(String::as_str),
                    Some("true" | "false")
                ) =>
        {
            Some((0, 0, true))
        }
        _ => None,
    }
}

/// Signal from a trapped chest with a known opener count. Unknown block
/// entities must remain unknown rather than being treated as unpowered.
pub fn trapped_chest_source(
    state: &BlockState,
    direction: IVec3,
    openers: u32,
) -> Option<(u8, u8, bool)> {
    if state.name != "minecraft:trapped_chest" || !DIRECTIONS.contains(&direction) {
        return None;
    }
    native_world::collision::block_shape(state)?;
    let strength = openers.min(15) as u8;
    Some((
        strength,
        if direction == IVec3::Y { strength } else { 0 },
        false,
    ))
}

/// Java's `hasNeighborSignal` for known sources and conductors.
pub fn neighbor_signal<'a>(
    position: IVec3,
    mut block: impl FnMut(IVec3) -> Option<&'a BlockState>,
) -> Option<bool> {
    neighbor_signal_from(position, |position, direction| {
        block(position).and_then(|state| source(state, direction))
    })
}

/// As above, using a world-aware source query for block entities.
pub fn neighbor_signal_from(
    position: IVec3,
    mut signal: impl FnMut(IVec3, IVec3) -> Option<(u8, u8, bool)>,
) -> Option<bool> {
    let mut unknown = false;
    for direction in DIRECTIONS {
        let Some(neighbor) = add(position, direction) else {
            unknown = true;
            continue;
        };
        let Some((weak, _, conductor)) = signal(neighbor, direction) else {
            unknown = true;
            continue;
        };
        if weak > 0 {
            return Some(true);
        }
        if conductor {
            for incoming in DIRECTIONS {
                let Some(source_pos) = add(neighbor, incoming) else {
                    unknown = true;
                    continue;
                };
                match signal(source_pos, incoming) {
                    Some((_, direct, _)) if direct > 0 => return Some(true),
                    Some(_) => {}
                    None => unknown = true,
                }
            }
        }
    }
    if unknown { None } else { Some(false) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn known_power_wins_over_unknown_but_no_power_remains_uncertain() {
        let air = BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        };
        let unknown = BlockState {
            name: "minecraft:unknown_source".into(),
            properties: BTreeMap::new(),
        };
        let button = BlockState {
            name: "minecraft:stone_button".into(),
            properties: [
                ("face".into(), "wall".into()),
                ("facing".into(), "east".into()),
                ("powered".into(), "true".into()),
            ]
            .into_iter()
            .collect(),
        };
        let query = |powered: bool, missing: bool| {
            neighbor_signal(IVec3::ZERO, |pos| {
                if pos == IVec3::NEG_Y {
                    if missing { None } else { Some(&unknown) }
                } else if pos == IVec3::X && powered {
                    Some(&button)
                } else {
                    Some(&air)
                }
            })
        };
        assert_eq!(query(true, false), Some(true));
        assert_eq!(query(true, true), Some(true));
        assert_eq!(query(false, false), None);
        assert_eq!(query(false, true), None);
        assert_eq!(neighbor_signal(IVec3::ZERO, |_| Some(&air)), Some(false));
    }
}
