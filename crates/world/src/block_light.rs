//! Conservative block-light recomputation for supported block state changes.
//!
//! Only complete neighborhoods of supported block states are accepted.
//! Unknown states or light data leave the existing light arrays untouched.

use crate::loaded::{BlockPosition, BlockState, ChunkPos, World};
use std::{
    collections::{BTreeMap, BTreeSet, BinaryHeap},
    sync::Arc,
};

const DIRECTIONS: [(i32, i32, i32); 6] = [
    (1, 0, 0),
    (-1, 0, 0),
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, 1),
    (0, 0, -1),
];

fn shape_family(name: &str) -> bool {
    let Some(name) = name.strip_prefix("minecraft:") else {
        return false;
    };
    let (base, slab) = if let Some(base) = name.strip_suffix("_slab") {
        (base, true)
    } else if let Some(base) = name.strip_suffix("_stairs") {
        (base, false)
    } else {
        return false;
    };
    if matches!(
        base,
        "cut_red_sandstone" | "cut_sandstone" | "petrified_oak" | "smooth_stone"
    ) {
        return slab;
    }
    if let Some(color) = base
        .strip_suffix("_concrete")
        .or_else(|| base.strip_suffix("_wool"))
    {
        return matches!(
            color,
            "white"
                | "orange"
                | "magenta"
                | "light_blue"
                | "yellow"
                | "lime"
                | "pink"
                | "gray"
                | "light_gray"
                | "cyan"
                | "purple"
                | "blue"
                | "brown"
                | "green"
                | "red"
                | "black"
        );
    }
    matches!(
        base,
        "acacia"
            | "andesite"
            | "bamboo"
            | "bamboo_mosaic"
            | "birch"
            | "blackstone"
            | "brick"
            | "cherry"
            | "cinnabar"
            | "cinnabar_brick"
            | "cobbled_deepslate"
            | "cobblestone"
            | "crimson"
            | "cut_copper"
            | "dark_oak"
            | "dark_prismarine"
            | "deepslate_brick"
            | "deepslate_tile"
            | "diorite"
            | "end_stone_brick"
            | "exposed_cut_copper"
            | "granite"
            | "jungle"
            | "mangrove"
            | "mossy_cobblestone"
            | "mossy_stone_brick"
            | "mud_brick"
            | "nether_brick"
            | "oak"
            | "oxidized_cut_copper"
            | "pale_oak"
            | "polished_andesite"
            | "polished_blackstone"
            | "polished_blackstone_brick"
            | "polished_cinnabar"
            | "polished_deepslate"
            | "polished_diorite"
            | "polished_granite"
            | "polished_sulfur"
            | "polished_tuff"
            | "poplar"
            | "prismarine"
            | "prismarine_brick"
            | "purpur"
            | "quartz"
            | "red_nether_brick"
            | "red_sandstone"
            | "resin_brick"
            | "sandstone"
            | "smooth_quartz"
            | "smooth_red_sandstone"
            | "smooth_sandstone"
            | "spruce"
            | "stone"
            | "stone_brick"
            | "sulfur"
            | "sulfur_brick"
            | "tuff"
            | "tuff_brick"
            | "warped"
            | "waxed_cut_copper"
            | "waxed_exposed_cut_copper"
            | "waxed_oxidized_cut_copper"
            | "waxed_weathered_cut_copper"
            | "weathered_cut_copper"
    )
}

fn properties(state: &BlockState) -> Option<(u8, u8)> {
    let boolean = |name: &str| match state.properties.get(name)?.as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    };
    let facing = || {
        matches!(
            state.properties.get("facing").map(String::as_str),
            Some("north" | "south" | "east" | "west")
        )
    };
    let axis = || {
        matches!(
            state.properties.get("axis").map(String::as_str),
            Some("x" | "y" | "z")
        )
    };
    let pair = match state.name.as_str() {
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
            if state.properties.is_empty() =>
        {
            (0, 0)
        }
        name if shape_family(name) => {
            crate::collision::slab_stair_solid(state)?;
            if state.properties.get("type").is_some_and(|v| v == "double") {
                (15, 0)
            } else {
                (u8::from(boolean("waterlogged")?), 0)
            }
        }
        "minecraft:redstone_ore" | "minecraft:deepslate_redstone_ore"
            if state.properties.len() == 1 =>
        {
            let emission = match state.properties.get("lit")?.as_str() {
                "true" => 9,
                "false" => 0,
                _ => return None,
            };
            (15, emission)
        }
        "minecraft:torch" if state.properties.is_empty() => (0, 14),
        "minecraft:soul_torch" if state.properties.is_empty() => (0, 10),
        "minecraft:wall_torch" if state.properties.len() == 1 && facing() => (0, 14),
        "minecraft:soul_wall_torch" if state.properties.len() == 1 && facing() => (0, 10),
        "minecraft:redstone_torch" if state.properties.len() == 1 => {
            (0, if boolean("lit")? { 7 } else { 0 })
        }
        "minecraft:redstone_wall_torch" if state.properties.len() == 2 && facing() => {
            (0, if boolean("lit")? { 7 } else { 0 })
        }
        "minecraft:lantern" | "minecraft:soul_lantern" if state.properties.len() == 2 => {
            boolean("hanging")?;
            (
                u8::from(boolean("waterlogged")?),
                if state.name == "minecraft:lantern" {
                    15
                } else {
                    10
                },
            )
        }
        "minecraft:water" | "minecraft:lava" if state.properties.len() == 1 => {
            let level = state.properties.get("level")?;
            let parsed: u8 = level.parse().ok()?;
            if parsed > 15 || parsed.to_string() != *level {
                return None;
            }
            (
                1,
                if state.name == "minecraft:lava" {
                    15
                } else {
                    0
                },
            )
        }
        "minecraft:glass" | "minecraft:short_grass" | "minecraft:fern"
            if state.properties.is_empty() =>
        {
            (0, 0)
        }
        "minecraft:glass_pane" if state.properties.len() == 5 => {
            for direction in ["north", "south", "east", "west"] {
                boolean(direction)?;
            }
            (u8::from(boolean("waterlogged")?), 0)
        }
        "minecraft:vine" if state.properties.len() == 5 => {
            for direction in ["north", "south", "east", "west", "up"] {
                boolean(direction)?;
            }
            (0, 0)
        }
        "minecraft:pointed_dripstone" if state.properties.len() == 3 => {
            if !matches!(
                state.properties.get("thickness").map(String::as_str),
                Some("tip_merge" | "tip" | "frustum" | "middle" | "base")
            ) || !matches!(
                state
                    .properties
                    .get("vertical_direction")
                    .map(String::as_str),
                Some("up" | "down")
            ) {
                return None;
            }
            (u8::from(boolean("waterlogged")?), 0)
        }
        "minecraft:cobweb" if state.properties.is_empty() => (1, 0),
        "minecraft:oak_leaves" if state.properties.len() == 3 => {
            boolean("persistent")?;
            boolean("waterlogged")?;
            if !matches!(
                state.properties.get("distance").map(String::as_str),
                Some("1" | "2" | "3" | "4" | "5" | "6" | "7")
            ) {
                return None;
            }
            (1, 0)
        }
        "minecraft:oak_log" | "minecraft:basalt" if state.properties.len() == 1 && axis() => {
            (15, 0)
        }
        "minecraft:grass_block" if state.properties.len() == 1 => {
            boolean("snowy")?;
            (15, 0)
        }
        "minecraft:glowstone" | "minecraft:sea_lantern" | "minecraft:shroomlight"
            if state.properties.is_empty() =>
        {
            (15, 15)
        }
        "minecraft:magma_block" if state.properties.is_empty() => (15, 3),
        "minecraft:stone"
        | "minecraft:deepslate"
        | "minecraft:dirt"
        | "minecraft:granite"
        | "minecraft:diorite"
        | "minecraft:andesite"
        | "minecraft:tuff"
        | "minecraft:coal_ore"
        | "minecraft:deepslate_coal_ore"
        | "minecraft:iron_ore"
        | "minecraft:deepslate_iron_ore"
        | "minecraft:copper_ore"
        | "minecraft:deepslate_copper_ore"
        | "minecraft:gold_ore"
        | "minecraft:deepslate_gold_ore"
        | "minecraft:diamond_ore"
        | "minecraft:deepslate_diamond_ore"
        | "minecraft:lapis_ore"
        | "minecraft:deepslate_lapis_ore"
        | "minecraft:emerald_ore"
        | "minecraft:deepslate_emerald_ore"
        | "minecraft:gravel"
        | "minecraft:sand"
        | "minecraft:red_sand"
        | "minecraft:bedrock"
        | "minecraft:obsidian"
        | "minecraft:netherrack"
        | "minecraft:blackstone"
        | "minecraft:nether_quartz_ore"
        | "minecraft:nether_gold_ore"
        | "minecraft:ancient_debris"
        | "minecraft:tinted_glass"
        | "minecraft:moss_block"
        | "minecraft:clay"
        | "minecraft:mud"
        | "minecraft:calcite"
        | "minecraft:dripstone_block"
        | "minecraft:oak_planks"
            if state.properties.is_empty() =>
        {
            (15, 0)
        }
        _ => return None,
    };
    Some(pair)
}

/// Masks use a 2×2 grid on each boundary face. The verified slab and stair
/// occlusion shapes have edges on the half-block grid.
fn face_masks(state: &BlockState) -> Option<[u8; 6]> {
    if !shape_family(&state.name) {
        return Some([0; 6]);
    }
    crate::collision::slab_stair_solid(state)?;
    if state.properties.get("type").is_some_and(|v| v == "double") {
        return Some([0; 6]);
    }
    let boxes = crate::collision::block_shape(state)?;
    let mut masks = [0; 6];
    for (index, (dx, dy, dz)) in DIRECTIONS.into_iter().enumerate() {
        let axis = if dx != 0 {
            0
        } else if dy != 0 {
            1
        } else {
            2
        };
        let high = dx + dy + dz > 0;
        let a = if axis == 0 { 1 } else { 0 };
        let b = if axis == 2 { 1 } else { 2 };
        for i in 0..2 {
            for j in 0..2 {
                let x = 0.25 + i as f32 * 0.5;
                let y = 0.25 + j as f32 * 0.5;
                if boxes.iter().any(|bb| {
                    (if high {
                        bb.max[axis] >= 1.0
                    } else {
                        bb.min[axis] <= 0.0
                    }) && bb.min[a] <= x
                        && bb.max[a] >= x
                        && bb.min[b] <= y
                        && bb.max[b] >= y
                }) {
                    masks[index] |= 1 << (i * 2 + j);
                }
            }
        }
    }
    Some(masks)
}

#[derive(Clone, Copy)]
struct LightProperties {
    opacity: u8,
    emission: u8,
    faces: [u8; 6],
}

fn light_properties(state: &BlockState) -> Option<LightProperties> {
    let (opacity, emission) = properties(state)?;
    Some(LightProperties {
        opacity,
        emission,
        faces: face_masks(state)?,
    })
}

fn offset(p: BlockPosition, d: (i32, i32, i32)) -> Option<BlockPosition> {
    Some(BlockPosition {
        x: p.x.checked_add(d.0)?,
        y: p.y.checked_add(d.1)?,
        z: p.z.checked_add(d.2)?,
    })
}

/// A validated change to a block's light emission or attenuation.
#[derive(Clone, Copy, Debug)]
pub struct LightChange {
    radius: u8,
    allow_dark_skip: bool,
}

/// Determine whether a supported change can affect block light. Both states
/// must have known light behavior; identical behavior needs no recomputation.
pub fn light_change(old: &BlockState, new: &BlockState) -> Option<LightChange> {
    let old = light_properties(old)?;
    let new = light_properties(new)?;
    let old_opacity = old.opacity.max(1);
    let new_opacity = new.opacity.max(1);
    if old_opacity == new_opacity && old.emission == new.emission && old.faces == new.faces {
        return None;
    }
    let radius = if old_opacity != new_opacity || old.faces != new.faces {
        14
    } else {
        old.emission.max(new.emission).saturating_sub(1)
    };
    Some(LightChange {
        radius,
        allow_dark_skip: old.emission == 0 && new.emission == 0,
    })
}

/// Recompute light after a supported block state change. A change in opacity
/// can affect light from a level-15 source up to fourteen blocks away.
/// Returns None if any necessary state or light array is unavailable.
/// On success the returned chunks are exactly those whose light changed.
pub fn relight_block(
    world: &mut World,
    center: BlockPosition,
    change: LightChange,
) -> Option<BTreeSet<ChunkPos>> {
    // With no light at the changed cell or any of its neighbors, an
    // emission-free opacity change cannot alter the surrounding block light.
    if change.allow_dark_skip {
        let dark = std::iter::once(center)
            .chain(DIRECTIONS.into_iter().filter_map(|d| offset(center, d)))
            .all(|p| {
                world.chunks.get(&p.chunk()).is_some_and(|chunk| {
                    chunk.status.as_deref() == Some("minecraft:full")
                        && chunk.block_light_correct
                        && world.light(p.x, p.y, p.z, false) == Some(0)
                })
            });
        // At an integer coordinate limit, an absent neighbor is unknown.
        if dark && DIRECTIONS.into_iter().all(|d| offset(center, d).is_some()) {
            return Some(BTreeSet::new());
        }
    }
    let radius = i32::from(change.radius);
    let mut cells = BTreeMap::new();
    let mut cache = BTreeMap::new();
    let behavior = |p: BlockPosition, cache: &mut BTreeMap<_, LightProperties>| {
        let id = world.block_id(p.x, p.y, p.z)?;
        if let Some(value) = cache.get(&id) {
            return Some(*value);
        }
        let value = light_properties(world.state(id)?)?;
        cache.insert(id, value);
        Some(value)
    };
    for dx in -radius..=radius {
        for dy in -radius..=radius {
            for dz in -radius..=radius {
                if dx.abs() + dy.abs() + dz.abs() > radius {
                    continue;
                }
                let p = offset(center, (dx, dy, dz))?;
                let chunk = world.chunks.get(&p.chunk())?;
                if chunk.status.as_deref() != Some("minecraft:full") || !chunk.block_light_correct {
                    return None;
                }
                let value = behavior(p, &mut cache)?;
                let previous = world.light(p.x, p.y, p.z, false)?;
                cells.insert(p, (value, previous, value.emission));
            }
        }
    }
    let mut queue = BinaryHeap::new();
    for (&p, cell) in &mut cells {
        for (direction_index, direction) in DIRECTIONS.into_iter().enumerate() {
            let next = offset(p, direction)?;
            if (next.x - center.x).abs() + (next.y - center.y).abs() + (next.z - center.z).abs()
                <= radius
            {
                continue;
            }
            let chunk = world.chunks.get(&next.chunk())?;
            if chunk.status.as_deref() != Some("minecraft:full") || !chunk.block_light_correct {
                return None;
            }
            let other = behavior(next, &mut cache)?;
            if (cell.0.faces[direction_index] | other.faces[direction_index ^ 1]) == 15 {
                continue;
            }
            let boundary = world.light(next.x, next.y, next.z, false)?;
            cell.2 = cell.2.max(boundary.saturating_sub(cell.0.opacity.max(1)));
        }
        if cell.2 > 0 {
            queue.push((cell.2, p));
        }
    }
    while let Some((level, p)) = queue.pop() {
        if cells.get(&p)?.2 != level {
            continue;
        }
        let from = cells.get(&p)?.0;
        for (direction_index, direction) in DIRECTIONS.into_iter().enumerate() {
            let next = offset(p, direction)?;
            let Some(cell) = cells.get_mut(&next) else {
                continue;
            };
            if (from.faces[direction_index] | cell.0.faces[direction_index ^ 1]) == 15 {
                continue;
            }
            let value = level.saturating_sub(cell.0.opacity.max(1));
            if value > cell.2 {
                cell.2 = value;
                queue.push((value, next));
            }
        }
    }
    // All reads and validation have completed before the first mutation.
    let mut changed = BTreeSet::new();
    for (p, (_, previous, value)) in cells {
        if previous == value {
            continue;
        }
        let chunk_pos = p.chunk();
        let chunk = Arc::make_mut(world.chunks.get_mut(&chunk_pos)?);
        let section_y = i8::try_from(p.y.div_euclid(16)).ok()?;
        let section = Arc::make_mut(chunk.sections.get_mut(&section_y)?);
        let data = section
            .block_light
            .get_or_insert_with(|| Box::new([0; crate::loaded::LIGHT_BYTES]));
        let index = p.x.rem_euclid(16) as usize
            + p.z.rem_euclid(16) as usize * 16
            + p.y.rem_euclid(16) as usize * 256;
        let byte = &mut data[index / 2];
        if index % 2 == 0 {
            *byte = (*byte & 0xf0) | value;
        } else {
            *byte = (*byte & 0x0f) | (value << 4);
        }
        changed.insert(chunk_pos);
    }
    Some(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loaded::{BLOCK_COUNT, BlockState, Chunk, LIGHT_BYTES, Section};

    fn position(x: i32, y: i32, z: i32) -> BlockPosition {
        BlockPosition { x, y, z }
    }
    fn state(name: &str, lit: Option<bool>) -> BlockState {
        BlockState {
            name: format!("minecraft:{name}"),
            properties: lit
                .map(|v| [("lit".to_owned(), v.to_string())].into())
                .unwrap_or_default(),
        }
    }
    fn fixture() -> (
        World,
        crate::loaded::BlockStateId,
        crate::loaded::BlockStateId,
        crate::loaded::BlockStateId,
    ) {
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(state("air", None));
        let unlit = world.intern_state(state("redstone_ore", Some(false)));
        let lit = world.intern_state(state("redstone_ore", Some(true)));
        let stone = world.intern_state(state("stone", None));
        for x in 0..=1 {
            for z in 0..=1 {
                let chunk_pos = ChunkPos { x, z };
                let mut chunk = Chunk {
                    block_light_correct: true,
                    position: chunk_pos,
                    status: Some("minecraft:full".into()),
                    sections: BTreeMap::new(),
                };
                for y in 0..=1 {
                    chunk.sections.insert(
                        y,
                        Arc::new(Section {
                            blocks: Some(Box::new([air; BLOCK_COUNT])),
                            biomes: None,
                            block_light: Some(Box::new([0; LIGHT_BYTES])),
                            sky_light: None,
                        }),
                    );
                }
                world.chunks.insert(chunk_pos, Arc::new(chunk));
            }
        }
        (world, unlit, lit, stone)
    }
    fn set(world: &mut World, p: BlockPosition, id: crate::loaded::BlockStateId) {
        world.set_block_id(p.x, p.y, p.z, id).unwrap();
    }
    fn light(world: &World, p: BlockPosition) -> u8 {
        world.light(p.x, p.y, p.z, false).unwrap()
    }

    #[test]
    fn activating_and_extinguishing_preserves_other_sources_and_snapshots() {
        let (mut world, unlit, lit, stone) = fixture();
        let center = position(15, 15, 15);
        let other = position(20, 15, 15);
        let wall = position(15, 16, 15);
        set(&mut world, center, unlit);
        set(&mut world, other, lit);
        set(&mut world, wall, stone);
        relight_block(
            &mut world,
            other,
            LightChange {
                radius: 8,
                allow_dark_skip: false,
            },
        )
        .unwrap();
        let old = world.clone();
        set(&mut world, center, lit);
        let changed = relight_block(
            &mut world,
            center,
            LightChange {
                radius: 8,
                allow_dark_skip: false,
            },
        )
        .unwrap();
        assert!(changed.len() > 1, "light crosses chunk boundaries");
        assert_eq!(light(&world, center), 9);
        assert_eq!(light(&world, position(14, 15, 15)), 8);
        assert_eq!(light(&world, wall), 0);
        assert_eq!(
            light(&world, position(15, 17, 15)),
            5,
            "light goes around the wall"
        );
        assert_eq!(light(&old, center), 0);
        set(&mut world, center, unlit);
        relight_block(
            &mut world,
            center,
            LightChange {
                radius: 8,
                allow_dark_skip: false,
            },
        )
        .unwrap();
        assert_eq!(
            light(&world, position(16, 15, 15)),
            5,
            "the other source remains"
        );
        assert_eq!(light(&world, position(14, 15, 15)), 1);
    }

    #[test]
    fn unknown_data_aborts_without_partial_light_changes() {
        let (mut world, _, lit, _) = fixture();
        let center = position(15, 15, 15);
        set(&mut world, center, lit);
        let unknown = world.intern_state(state("unknown_block", None));
        set(&mut world, position(14, 15, 15), unknown);
        let before = world.clone();
        assert!(
            relight_block(
                &mut world,
                center,
                LightChange {
                    radius: 8,
                    allow_dark_skip: false
                }
            )
            .is_none()
        );
        for (key, chunk) in &world.chunks {
            assert!(Arc::ptr_eq(chunk, &before.chunks[key]));
        }
        assert_eq!(light(&world, center), 0);
    }

    #[test]
    fn absent_saved_light_is_zero_only_for_correct_chunks() {
        let (mut world, _, lit, _) = fixture();
        for chunk in world.chunks.values_mut() {
            for section in Arc::make_mut(chunk).sections.values_mut() {
                Arc::make_mut(section).block_light = None;
            }
        }
        let center = position(15, 15, 15);
        set(&mut world, center, lit);
        let before = world.clone();
        assert_eq!(light(&world, center), 0);
        assert!(
            relight_block(
                &mut world,
                center,
                LightChange {
                    radius: 8,
                    allow_dark_skip: false
                }
            )
            .unwrap()
            .len()
                > 1
        );
        assert_eq!(light(&world, position(16, 15, 15)), 8);
        assert_eq!(light(&before, position(16, 15, 15)), 0);
        assert!(
            before
                .chunks
                .values()
                .flat_map(|chunk| chunk.sections.values())
                .all(|section| section.block_light.is_none())
        );

        let (mut incomplete, _, lit, _) = fixture();
        let neighbor = ChunkPos { x: 1, z: 0 };
        Arc::make_mut(incomplete.chunks.get_mut(&neighbor).unwrap()).block_light_correct = false;
        set(&mut incomplete, center, lit);
        let before = incomplete.clone();
        assert!(
            relight_block(
                &mut incomplete,
                center,
                LightChange {
                    radius: 8,
                    allow_dark_skip: false
                }
            )
            .is_none()
        );
        for (key, chunk) in &incomplete.chunks {
            assert!(Arc::ptr_eq(chunk, &before.chunks[key]));
        }
    }

    #[test]
    fn opaque_changes_in_complete_darkness_do_not_scan_unrelated_blocks() {
        let (mut world, _, _, stone) = fixture();
        let center = position(15, 15, 15);
        let unknown = world.intern_state(state("unknown_block", None));
        set(&mut world, position(20, 15, 15), unknown);
        let old = world.block(center.x, center.y, center.z).unwrap().clone();
        set(&mut world, center, stone);
        let change =
            light_change(&old, world.block(center.x, center.y, center.z).unwrap()).unwrap();
        let before = world.clone();
        assert!(
            relight_block(&mut world, center, change)
                .unwrap()
                .is_empty()
        );
        for (key, chunk) in &world.chunks {
            assert!(Arc::ptr_eq(chunk, &before.chunks[key]));
        }
    }

    #[test]
    fn opposite_slab_faces_block_light_only_when_their_halves_cover_the_face() {
        let slab = |kind: &str| BlockState {
            name: "minecraft:stone_slab".into(),
            properties: [
                ("type".into(), kind.into()),
                ("waterlogged".into(), "false".into()),
            ]
            .into(),
        };
        let bottom = slab("bottom");
        let top = slab("top");
        let bottom_faces = face_masks(&bottom).unwrap();
        let top_faces = face_masks(&top).unwrap();
        assert_eq!(bottom_faces[0] | top_faces[1], 15);
        assert_ne!(bottom_faces[0] | bottom_faces[1], 15);
        assert!(light_change(&bottom, &top).is_some());
        assert!(light_change(&bottom, &bottom).is_none());
    }

    #[test]
    fn removing_a_lit_source_clears_its_light() {
        let (mut world, _, lit, _) = fixture();
        let center = position(15, 15, 15);
        set(&mut world, center, lit);
        relight_block(
            &mut world,
            center,
            LightChange {
                radius: 8,
                allow_dark_skip: false,
            },
        )
        .unwrap();
        let air = world.intern_state(state("air", None));
        set(&mut world, center, air);
        relight_block(
            &mut world,
            center,
            LightChange {
                radius: 14,
                allow_dark_skip: false,
            },
        )
        .unwrap();
        assert_eq!(light(&world, center), 0);
        assert_eq!(light(&world, position(16, 15, 15)), 0);
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn all_extended_light_states_match_vanilla() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft")
            });
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("BlockLightStatesOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/BlockLightStatesOracle.java"),
        )
        .unwrap();
        let run = |cmd: &mut Command| {
            let out = cmd.output().unwrap();
            assert!(
                out.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-encoding")
            .arg("UTF-8")
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let names = [
            "torch",
            "wall_torch",
            "soul_torch",
            "soul_wall_torch",
            "redstone_torch",
            "redstone_wall_torch",
            "lantern",
            "soul_lantern",
            "glowstone",
            "sea_lantern",
            "shroomlight",
            "glass",
            "tinted_glass",
            "glass_pane",
            "water",
            "lava",
            "gravel",
            "sand",
            "red_sand",
            "bedrock",
            "obsidian",
            "netherrack",
            "basalt",
            "blackstone",
            "nether_quartz_ore",
            "nether_gold_ore",
            "ancient_debris",
            "magma_block",
            "grass_block",
            "moss_block",
            "clay",
            "mud",
            "calcite",
            "dripstone_block",
            "pointed_dripstone",
            "short_grass",
            "fern",
            "vine",
            "cobweb",
            "oak_log",
            "oak_planks",
            "oak_leaves",
        ];
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        let mut requested = names
            .iter()
            .map(|n| format!("minecraft:{n}"))
            .collect::<Vec<_>>();
        requested.push("@vanilla_slabs_stairs".into());
        std::fs::write(&input, serde_json::to_vec(&requested).unwrap()).unwrap();
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("BlockLightStatesOracle")
            .arg(&input)
            .arg(&output));
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert!(rows.len() > 8_000);
        let shaped_names: BTreeSet<_> = rows
            .iter()
            .filter_map(|row| {
                let name = row["name"].as_str()?;
                (name.ends_with("_slab") || name.ends_with("_stairs")).then_some(name)
            })
            .collect();
        assert_eq!(shaped_names.len(), 198);
        for row in rows {
            let s = BlockState {
                name: row["name"].as_str().unwrap().into(),
                properties: row["properties"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(key, value)| (key.clone(), value.as_str().unwrap().into()))
                    .collect(),
            };
            let (opacity, emission) =
                properties(&s).unwrap_or_else(|| panic!("unsupported vanilla state {s:?}"));
            assert_eq!(row["opacity"], opacity, "{s:?}");
            assert_eq!(row["emission"], emission, "{s:?}");
            let shaped =
                shape_family(&s.name) && s.properties.get("type").is_none_or(|v| v != "double");
            assert_eq!(row["shaped"], shaped, "{s:?}");
            assert_eq!(
                row["faces"],
                serde_json::json!(face_masks(&s).unwrap()),
                "{s:?}"
            );
        }
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn propagation_matches_vanilla_light_engine() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft")
            });
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("BlockLightEngineOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/BlockLightEngineOracle.java"),
        )
        .unwrap();
        let run = |cmd: &mut Command| {
            let out = cmd.output().unwrap();
            assert!(
                out.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-encoding")
            .arg("UTF-8")
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let initial = [
            (15, 15, 15, "unlit"),
            (20, 15, 15, "unlit"),
            (15, 16, 15, "stone"),
            (16, 16, 15, "stone"),
            (16, 15, 16, "stone"),
            (16, 15, 15, "torch"),
            (15, 15, 16, "water"),
            (14, 15, 15, "wet_pane"),
            (13, 15, 15, "slab_bottom"),
            (13, 15, 16, "slab_top"),
            (13, 15, 14, "stair_bottom"),
        ];
        let actions = [
            (20, 15, 15, "lit"),
            (15, 15, 15, "lit"),
            (15, 15, 15, "unlit"),
            (15, 15, 15, "lit"),
            (15, 15, 15, "air"),
            (15, 15, 15, "slab_bottom"),
            (15, 15, 15, "slab_top"),
            (15, 15, 15, "stair_bottom"),
            (15, 15, 15, "stair_top"),
            (15, 15, 15, "air"),
            (15, 15, 15, "torch"),
            (15, 15, 15, "air"),
            (15, 15, 15, "stone"),
            (15, 15, 15, "air"),
            (15, 15, 15, "water"),
            (15, 15, 15, "wet_pane"),
            (15, 15, 15, "air"),
        ];
        let json = |items: &[(i32, i32, i32, &str)]| {
            items
                .iter()
                .map(|(x, y, z, state)| serde_json::json!({"x":x,"y":y,"z":z,"state":state}))
                .collect::<Vec<_>>()
        };
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(
            &input,
            serde_json::to_vec(
                &serde_json::json!({"initial":json(&initial),"actions":json(&actions)}),
            )
            .unwrap(),
        )
        .unwrap();
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("BlockLightEngineOracle")
            .arg(&input)
            .arg(&output));
        let expected: Vec<Vec<u8>> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let (mut world, unlit, lit, stone) = fixture();
        for chunk in world.chunks.values_mut() {
            for section in Arc::make_mut(chunk).sections.values_mut() {
                Arc::make_mut(section).block_light = None;
            }
        }
        let air = world.intern_state(state("air", None));
        let torch = world.intern_state(state("torch", None));
        let slab_bottom = world.intern_state(BlockState {
            name: "minecraft:stone_slab".into(),
            properties: [
                ("type".into(), "bottom".into()),
                ("waterlogged".into(), "false".into()),
            ]
            .into(),
        });
        let slab_top = world.intern_state(BlockState {
            name: "minecraft:stone_slab".into(),
            properties: [
                ("type".into(), "top".into()),
                ("waterlogged".into(), "false".into()),
            ]
            .into(),
        });
        let stairs = |facing: &str, half: &str, shape: &str| BlockState {
            name: "minecraft:oak_stairs".into(),
            properties: [
                ("facing".into(), facing.into()),
                ("half".into(), half.into()),
                ("shape".into(), shape.into()),
                ("waterlogged".into(), "false".into()),
            ]
            .into(),
        };
        let stair_bottom = world.intern_state(stairs("north", "bottom", "straight"));
        let stair_top = world.intern_state(stairs("east", "top", "inner_left"));
        let water = world.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: [("level".into(), "0".into())].into(),
        });
        let wet_pane = world.intern_state(BlockState {
            name: "minecraft:glass_pane".into(),
            properties: [
                ("north".into(), "false".into()),
                ("south".into(), "false".into()),
                ("east".into(), "false".into()),
                ("west".into(), "false".into()),
                ("waterlogged".into(), "true".into()),
            ]
            .into(),
        });
        let id = |name: &str| match name {
            "air" => air,
            "stone" => stone,
            "unlit" => unlit,
            "lit" => lit,
            "torch" => torch,
            "water" => water,
            "wet_pane" => wet_pane,
            "slab_bottom" => slab_bottom,
            "slab_top" => slab_top,
            "stair_bottom" => stair_bottom,
            "stair_top" => stair_top,
            _ => unreachable!(),
        };
        for (x, y, z, name) in initial {
            set(&mut world, position(x, y, z), id(name));
        }
        relight_block(
            &mut world,
            position(15, 15, 15),
            LightChange {
                radius: 14,
                allow_dark_skip: false,
            },
        )
        .unwrap();
        assert_eq!(expected.len(), actions.len());
        for ((x, y, z, name), expected) in actions.into_iter().zip(expected) {
            let p = position(x, y, z);
            let old = world.block(p.x, p.y, p.z).unwrap().clone();
            set(&mut world, p, id(name));
            if let Some(change) = light_change(&old, world.block(p.x, p.y, p.z).unwrap()) {
                relight_block(&mut world, p, change).unwrap();
            }
            assert_eq!(expected.len(), 31 * 31 * 31);
            for y in 0..31 {
                for z in 0..31 {
                    for x in 0..31 {
                        let index = (y * 31 * 31 + z * 31 + x) as usize;
                        assert_eq!(
                            light(&world, position(x, y, z)),
                            expected[index],
                            "after {name} at {p:?}, checking {x},{y},{z}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn supported_properties_match_vanilla() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft")
            });
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("BlockLightPropertiesOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/BlockLightPropertiesOracle.java"),
        )
        .unwrap();
        let run = |cmd: &mut Command| {
            let out = cmd.output().unwrap();
            assert!(
                out.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-encoding")
            .arg("UTF-8")
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let mut states = Vec::new();
        for name in [
            "air",
            "cave_air",
            "void_air",
            "stone",
            "deepslate",
            "dirt",
            "granite",
            "diorite",
            "andesite",
            "tuff",
            "coal_ore",
            "deepslate_coal_ore",
            "iron_ore",
            "deepslate_iron_ore",
            "copper_ore",
            "deepslate_copper_ore",
            "gold_ore",
            "deepslate_gold_ore",
            "diamond_ore",
            "deepslate_diamond_ore",
            "lapis_ore",
            "deepslate_lapis_ore",
            "emerald_ore",
            "deepslate_emerald_ore",
        ] {
            states.push(state(name, None));
        }
        for name in ["redstone_ore", "deepslate_redstone_ore"] {
            for lit in [false, true] {
                states.push(state(name, Some(lit)));
            }
        }
        let cases: Vec<_> = states
            .iter()
            .map(|s| {
                let mut row = serde_json::json!({"name": s.name});
                if let Some(lit) = s.properties.get("lit") {
                    row["lit"] = serde_json::json!(lit == "true");
                }
                row
            })
            .collect();
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("BlockLightPropertiesOracle")
            .arg(&input)
            .arg(&output));
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), states.len());
        for (s, row) in states.iter().zip(rows) {
            let (opacity, emission) = properties(s).unwrap();
            assert_eq!(row, serde_json::json!([opacity, emission, false]), "{s:?}");
        }
    }
}
