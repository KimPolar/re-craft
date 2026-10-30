//! Conservative block-light recomputation for supported block state changes.
//!
//! Only complete neighborhoods of supported block states are accepted.
//! Unknown states or light data leave the existing light arrays untouched.

use crate::loaded::{BlockPosition, BlockState, ChunkPos, LIGHT_BYTES, Section, World};
use std::{
    collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap},
    sync::Arc,
};

pub(crate) const DIRECTIONS: [(i32, i32, i32); 6] = [
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
    let bounded = |name: &str, maximum: u8| -> Option<()> {
        let raw = state.properties.get(name)?;
        let value = raw.parse::<u8>().ok()?;
        (value <= maximum && value.to_string() == *raw).then_some(())
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
        "minecraft:dirt_path" if state.properties.is_empty() => (0, 0),
        "minecraft:farmland" if state.properties.len() == 1 => {
            if !matches!(
                state.properties.get("moisture").map(String::as_str),
                Some("0" | "1" | "2" | "3" | "4" | "5" | "6" | "7")
            ) {
                return None;
            }
            (0, 0)
        }
        "minecraft:snow" if state.properties.len() == 1 => {
            let layers = state.properties.get("layers")?;
            if !matches!(
                layers.as_str(),
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8"
            ) {
                return None;
            }
            (if layers == "8" { 15 } else { 0 }, 0)
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
        "minecraft:dandelion"
        | "minecraft:poppy"
        | "minecraft:blue_orchid"
        | "minecraft:allium"
        | "minecraft:azure_bluet"
        | "minecraft:red_tulip"
        | "minecraft:orange_tulip"
        | "minecraft:white_tulip"
        | "minecraft:pink_tulip"
        | "minecraft:oxeye_daisy"
        | "minecraft:cornflower"
        | "minecraft:lily_of_the_valley"
        | "minecraft:wither_rose"
        | "minecraft:torchflower"
        | "minecraft:closed_eyeblossom"
        | "minecraft:open_eyeblossom"
        | "minecraft:dead_bush"
        | "minecraft:red_mushroom"
        | "minecraft:azalea"
        | "minecraft:flowering_azalea"
            if state.properties.is_empty() =>
        {
            (0, 0)
        }
        "minecraft:brown_mushroom" if state.properties.is_empty() => (0, 1),
        "minecraft:oak_sapling"
        | "minecraft:spruce_sapling"
        | "minecraft:birch_sapling"
        | "minecraft:jungle_sapling"
        | "minecraft:acacia_sapling"
        | "minecraft:dark_oak_sapling"
        | "minecraft:cherry_sapling"
        | "minecraft:pale_oak_sapling"
            if state.properties.len() == 1 =>
        {
            bounded("stage", 1)?;
            (0, 0)
        }
        "minecraft:wheat" | "minecraft:carrots" | "minecraft:potatoes"
            if state.properties.len() == 1 =>
        {
            bounded("age", 7)?;
            (0, 0)
        }
        "minecraft:beetroots" if state.properties.len() == 1 => {
            bounded("age", 3)?;
            (0, 0)
        }
        "minecraft:sugar_cane" | "minecraft:cactus" if state.properties.len() == 1 => {
            bounded("age", 15)?;
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
            if state.properties.len() == 3 =>
        {
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
        "minecraft:basalt" | "minecraft:muddy_mangrove_roots"
            if state.properties.len() == 1 && axis() =>
        {
            (15, 0)
        }
        "minecraft:grass_block" | "minecraft:podzol" | "minecraft:mycelium"
            if state.properties.len() == 1 =>
        {
            boolean("snowy")?;
            (15, 0)
        }
        "minecraft:mangrove_roots" if state.properties.len() == 1 => {
            boolean("waterlogged")?;
            (1, 0)
        }
        "minecraft:ice" if state.properties.is_empty() => (1, 0),
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
        | "minecraft:cobbled_deepslate"
        | "minecraft:stone_bricks"
        | "minecraft:cobblestone"
        | "minecraft:coarse_dirt"
        | "minecraft:rooted_dirt"
        | "minecraft:mossy_cobblestone"
        | "minecraft:tuff_bricks"
        | "minecraft:polished_tuff"
        | "minecraft:polished_andesite"
        | "minecraft:polished_granite"
        | "minecraft:polished_diorite"
        | "minecraft:end_stone"
        | "minecraft:crafting_table"
        | "minecraft:sandstone"
        | "minecraft:red_sandstone"
        | "minecraft:nether_bricks"
        | "minecraft:quartz_block"
        | "minecraft:packed_ice"
        | "minecraft:blue_ice"
        | "minecraft:bamboo_mosaic"
        | "minecraft:soul_sand"
        | "minecraft:soul_soil"
        | "minecraft:smooth_stone"
            if state.properties.is_empty() =>
        {
            (15, 0)
        }
        name if state.properties.len() == 1
            && axis()
            && native_core::wood::Wood::from_id(name).is_some() =>
        {
            (15, 0)
        }
        name if state.properties.is_empty()
            && native_core::wood::Planks::from_id(name).is_some() =>
        {
            (15, 0)
        }
        _ => return None,
    };
    Some(pair)
}

/// Each face uses a 16×16 grid. All supported occlusion boxes have edges on
/// this grid; refuse a shape if its geometry cannot be represented exactly.
fn face_masks(state: &BlockState) -> Option<[[u16; 16]; 6]> {
    let boxes = if shape_family(&state.name) {
        crate::collision::slab_stair_solid(state)?;
        if state.properties.get("type").is_some_and(|v| v == "double") {
            return Some([[0; 16]; 6]);
        }
        crate::collision::block_shape(state)?
    } else if matches!(
        state.name.as_str(),
        "minecraft:dirt_path" | "minecraft:farmland"
    ) {
        properties(state)?;
        crate::collision::block_shape(state)?
    } else if state.name == "minecraft:snow" {
        properties(state)?;
        let height = state.properties.get("layers")?.parse::<f32>().ok()? / 8.0;
        vec![crate::collision::Aabb {
            min: glam::Vec3::ZERO,
            max: glam::Vec3::new(1.0, height, 1.0),
        }]
    } else {
        return Some([[0; 16]; 6]);
    };
    let mut masks = [[0; 16]; 6];
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
        let grid = |value: f32| -> Option<usize> {
            let scaled = value * 16.0;
            ((0.0..=16.0).contains(&scaled) && scaled.fract() == 0.0).then_some(scaled as usize)
        };
        for bb in &boxes {
            if !(if high {
                bb.max[axis] >= 1.0
            } else {
                bb.min[axis] <= 0.0
            }) {
                continue;
            }
            let (a0, a1, b0, b1) = (
                grid(bb.min[a])?,
                grid(bb.max[a])?,
                grid(bb.min[b])?,
                grid(bb.max[b])?,
            );
            if b1 < b0 || a1 < a0 {
                return None;
            }
            let bits = (((1u32 << (b1 - b0)) - 1) << b0) as u16;
            for row in &mut masks[index][a0..a1] {
                *row |= bits;
            }
        }
    }
    Some(masks)
}

#[derive(Clone)]
pub(crate) struct LightProperties {
    pub(crate) opacity: u8,
    emission: u8,
    pub(crate) faces: [[u16; 16]; 6],
    pub(crate) has_faces: bool,
}

pub(crate) fn faces_occlude(from: &[u16; 16], to: &[u16; 16]) -> bool {
    from.iter().zip(to).all(|(a, b)| *a | *b == u16::MAX)
}

pub(crate) fn light_properties(state: &BlockState) -> Option<LightProperties> {
    let (opacity, emission) = properties(state)?;
    let faces = face_masks(state)?;
    Some(LightProperties {
        opacity,
        emission,
        has_faces: faces.iter().flatten().any(|&row| row != 0),
        faces,
    })
}

pub(crate) fn transparent_light_properties() -> LightProperties {
    LightProperties {
        opacity: 0,
        emission: 0,
        faces: [[0; 16]; 6],
        has_faces: false,
    }
}

fn offset(p: BlockPosition, d: (i32, i32, i32)) -> Option<BlockPosition> {
    Some(BlockPosition {
        x: p.x.checked_add(d.0)?,
        y: p.y.checked_add(d.1)?,
        z: p.z.checked_add(d.2)?,
    })
}

/// Java keeps one extra light section above and below the build range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LightBounds {
    min_y: i32,
    max_y: i32,
    min_light_y: i32,
    max_light_y: i32,
}

impl LightBounds {
    pub fn new(min_y: i32, max_y: i32) -> Option<Self> {
        if min_y >= max_y || min_y.rem_euclid(16) != 0 || max_y.rem_euclid(16) != 0 {
            return None;
        }
        let min_light_y = min_y.checked_sub(16)?;
        let max_light_y = max_y.checked_add(16)?;
        i8::try_from(min_light_y.div_euclid(16)).ok()?;
        i8::try_from((max_light_y - 1).div_euclid(16)).ok()?;
        Some(Self {
            min_y,
            max_y,
            min_light_y,
            max_light_y,
        })
    }

    fn is_padding(self, y: i32) -> bool {
        (self.min_light_y..self.max_light_y).contains(&y) && !(self.min_y..self.max_y).contains(&y)
    }

    pub(crate) fn build_range(self) -> std::ops::Range<i32> {
        self.min_y..self.max_y
    }

    pub(crate) fn light_range(self) -> std::ops::Range<i32> {
        self.min_light_y..self.max_light_y
    }
}

fn light_at(world: &World, p: BlockPosition, bounds: Option<LightBounds>) -> Option<u8> {
    if bounds.is_some_and(|b| !(b.min_light_y..b.max_light_y).contains(&p.y)) {
        return None;
    }
    if let Some(light) = world.light(p.x, p.y, p.z, false) {
        return Some(light);
    }
    if bounds.is_some_and(|b| b.is_padding(p.y)) {
        i8::try_from(p.y.div_euclid(16)).ok()?;
        let chunk = world.chunks.get(&p.chunk())?;
        if chunk.status.as_deref() == Some("minecraft:full") && chunk.block_light_correct {
            return Some(0);
        }
    }
    None
}

fn behavior_at(
    world: &World,
    p: BlockPosition,
    bounds: Option<LightBounds>,
    cache: &mut BTreeMap<Option<crate::loaded::BlockStateId>, u32>,
    behaviors: &mut Vec<LightProperties>,
) -> Option<u32> {
    if bounds.is_some_and(|b| !(b.min_light_y..b.max_light_y).contains(&p.y)) {
        return None;
    }
    let key = if bounds.is_some_and(|b| b.is_padding(p.y)) {
        None
    } else {
        Some(world.block_id(p.x, p.y, p.z)?)
    };
    if let Some(&index) = cache.get(&key) {
        return Some(index);
    }
    let value = match key {
        Some(id) => light_properties(world.state(id)?)?,
        None => LightProperties {
            opacity: 0,
            emission: 0,
            faces: [[0; 16]; 6],
            has_faces: false,
        },
    };
    let index = u32::try_from(behaviors.len())
        .ok()
        .filter(|&i| i != u32::MAX)?;
    behaviors.push(value);
    cache.insert(key, index);
    Some(index)
}

fn write_light(
    world: &mut World,
    p: BlockPosition,
    value: u8,
    bounds: Option<LightBounds>,
) -> Option<()> {
    let chunk = Arc::make_mut(world.chunks.get_mut(&p.chunk())?);
    let section_y = i8::try_from(p.y.div_euclid(16)).ok()?;
    let section = match chunk.sections.entry(section_y) {
        std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
        std::collections::btree_map::Entry::Vacant(entry) => {
            if !bounds.is_some_and(|b| b.is_padding(p.y)) {
                return None;
            }
            entry.insert(Arc::new(Section {
                blocks: None,
                biomes: None,
                block_light: None,
                sky_light: None,
            }))
        }
    };
    let section = Arc::make_mut(section);
    let data = section
        .block_light
        .get_or_insert_with(|| Box::new([0; LIGHT_BYTES]));
    let index = p.x.rem_euclid(16) as usize
        + p.z.rem_euclid(16) as usize * 16
        + p.y.rem_euclid(16) as usize * 256;
    let byte = &mut data[index / 2];
    if index.is_multiple_of(2) {
        *byte = (*byte & 0xf0) | value;
    } else {
        *byte = (*byte & 0x0f) | (value << 4);
    }
    Some(())
}

#[derive(Clone, Copy)]
struct LightCell {
    behavior: u32,
    previous: u8,
    value: u8,
}

fn dense_index(x: i32, y: i32, z: i32, radius: i32, side: usize) -> usize {
    ((x + radius) as usize * side + (y + radius) as usize) * side + (z + radius) as usize
}

fn dense_offset(index: usize, radius: i32, side: usize) -> (i32, i32, i32) {
    let x = index / (side * side);
    let y = index / side % side;
    let z = index % side;
    (x as i32 - radius, y as i32 - radius, z as i32 - radius)
}

/// A validated change to a block's light emission or attenuation.
#[derive(Clone, Copy, Debug)]
pub struct LightChange {
    radius: u8,
    allow_dark_skip: bool,
}

impl LightChange {
    /// The largest possible effect of an unrecognized block-state change.
    pub fn unknown() -> Self {
        Self {
            radius: 14,
            allow_dark_skip: false,
        }
    }

    pub fn radius(self) -> i32 {
        i32::from(self.radius)
    }

    pub fn merge(self, other: Self) -> Self {
        Self {
            radius: self.radius.max(other.radius),
            allow_dark_skip: self.allow_dark_skip && other.allow_dark_skip,
        }
    }
}

/// Unknown original or replacement states still need an eventual relight.
pub fn light_change_for_edit(old: Option<&BlockState>, new: &BlockState) -> Option<LightChange> {
    match (old.and_then(light_properties), light_properties(new)) {
        (Some(old), Some(new)) => light_change_from_properties(&old, &new),
        _ => Some(LightChange::unknown()),
    }
}

/// Determine whether a supported change can affect block light. Both states
/// must have known light behavior; identical behavior needs no recomputation.
pub fn light_change(old: &BlockState, new: &BlockState) -> Option<LightChange> {
    let old = light_properties(old)?;
    let new = light_properties(new)?;
    light_change_from_properties(&old, &new)
}

fn light_change_from_properties(
    old: &LightProperties,
    new: &LightProperties,
) -> Option<LightChange> {
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
    relight_block_with_bounds(world, center, change, None)
}

fn relight_block_with_bounds(
    world: &mut World,
    center: BlockPosition,
    change: LightChange,
    bounds: Option<LightBounds>,
) -> Option<BTreeSet<ChunkPos>> {
    if bounds.is_some_and(|b| !(b.min_y..b.max_y).contains(&center.y)) {
        return None;
    }
    // With no light at the changed cell or any of its neighbors, an
    // emission-free opacity change cannot alter the surrounding block light.
    if change.allow_dark_skip {
        let dark = std::iter::once(center)
            .chain(DIRECTIONS.into_iter().filter_map(|d| offset(center, d)))
            .all(|p| {
                world.chunks.get(&p.chunk()).is_some_and(|chunk| {
                    chunk.status.as_deref() == Some("minecraft:full")
                        && chunk.block_light_correct
                        && light_at(world, p, bounds) == Some(0)
                })
            });
        // At an integer coordinate limit, an absent neighbor is unknown.
        if dark && DIRECTIONS.into_iter().all(|d| offset(center, d).is_some()) {
            return Some(BTreeSet::new());
        }
    }
    let radius = i32::from(change.radius);
    let side = (radius * 2 + 1) as usize;
    let mut cells = vec![
        LightCell {
            behavior: u32::MAX,
            previous: 0,
            value: 0
        };
        side * side * side
    ];
    let mut cache = BTreeMap::new();
    let mut behaviors: Vec<LightProperties> = Vec::new();
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
                let value = behavior_at(world, p, bounds, &mut cache, &mut behaviors)?;
                let previous = light_at(world, p, bounds)?;
                let index = dense_index(dx, dy, dz, radius, side);
                cells[index] = LightCell {
                    behavior: value,
                    previous,
                    value: behaviors[value as usize].emission,
                };
            }
        }
    }
    let mut queue = BinaryHeap::new();
    for (index, cell) in cells.iter_mut().enumerate() {
        if cell.behavior == u32::MAX {
            continue;
        }
        let (dx, dy, dz) = dense_offset(index, radius, side);
        let p = offset(center, (dx, dy, dz))?;
        for (direction_index, direction) in DIRECTIONS.into_iter().enumerate() {
            let (nx, ny, nz) = (dx + direction.0, dy + direction.1, dz + direction.2);
            if nx.abs() + ny.abs() + nz.abs() <= radius {
                continue;
            }
            let next = offset(p, direction)?;
            let chunk = world.chunks.get(&next.chunk())?;
            if chunk.status.as_deref() != Some("minecraft:full") || !chunk.block_light_correct {
                return None;
            }
            let other = behavior_at(world, next, bounds, &mut cache, &mut behaviors)?;
            if (behaviors[cell.behavior as usize].has_faces || behaviors[other as usize].has_faces)
                && faces_occlude(
                    &behaviors[cell.behavior as usize].faces[direction_index],
                    &behaviors[other as usize].faces[direction_index ^ 1],
                )
            {
                continue;
            }
            let boundary = light_at(world, next, bounds)?;
            cell.value = cell
                .value
                .max(boundary.saturating_sub(behaviors[cell.behavior as usize].opacity.max(1)));
        }
        if cell.value > 0 {
            queue.push((cell.value, index));
        }
    }
    while let Some((level, index)) = queue.pop() {
        if cells[index].value != level {
            continue;
        }
        let from = cells[index].behavior;
        let (dx, dy, dz) = dense_offset(index, radius, side);
        for (direction_index, direction) in DIRECTIONS.into_iter().enumerate() {
            let (nx, ny, nz) = (dx + direction.0, dy + direction.1, dz + direction.2);
            if nx.abs() + ny.abs() + nz.abs() > radius {
                continue;
            }
            let next = dense_index(nx, ny, nz, radius, side);
            let cell = &mut cells[next];
            debug_assert_ne!(cell.behavior, u32::MAX);
            if (behaviors[from as usize].has_faces || behaviors[cell.behavior as usize].has_faces)
                && faces_occlude(
                    &behaviors[from as usize].faces[direction_index],
                    &behaviors[cell.behavior as usize].faces[direction_index ^ 1],
                )
            {
                continue;
            }
            let value = level.saturating_sub(behaviors[cell.behavior as usize].opacity.max(1));
            if value > cell.value {
                cell.value = value;
                queue.push((value, next));
            }
        }
    }
    // All reads and validation have completed before the first mutation.
    let mut changed = BTreeSet::new();
    for (index, cell) in cells.into_iter().enumerate() {
        if cell.behavior == u32::MAX || cell.previous == cell.value {
            continue;
        }
        let p = offset(center, dense_offset(index, radius, side))?;
        let chunk_pos = p.chunk();
        write_light(world, p, cell.value, bounds)?;
        changed.insert(chunk_pos);
    }
    Some(changed)
}

/// Recompute the union of several overlapping changed regions atomically.
/// Boundary light outside the union cannot be affected by any of the changes.
pub fn relight_changes(
    world: &mut World,
    changes: &[(BlockPosition, LightChange)],
) -> Option<BTreeSet<ChunkPos>> {
    relight_changes_with_bounds(world, changes, None)
}

pub fn relight_changes_in_bounds(
    world: &mut World,
    changes: &[(BlockPosition, LightChange)],
    bounds: LightBounds,
) -> Option<BTreeSet<ChunkPos>> {
    relight_changes_with_bounds(world, changes, Some(bounds))
}

fn relight_changes_with_bounds(
    world: &mut World,
    changes: &[(BlockPosition, LightChange)],
    bounds: Option<LightBounds>,
) -> Option<BTreeSet<ChunkPos>> {
    if changes.is_empty() {
        return Some(BTreeSet::new());
    }
    if bounds.is_some_and(|b| {
        changes
            .iter()
            .any(|(p, _)| !(b.min_y..b.max_y).contains(&p.y))
    }) {
        return None;
    }
    if let [(position, change)] = changes {
        return relight_block_with_bounds(world, *position, *change, bounds);
    }

    let mut indices = HashMap::new();
    let mut cells: Vec<(BlockPosition, LightCell)> = Vec::new();
    let mut cache = BTreeMap::new();
    let mut behaviors: Vec<LightProperties> = Vec::new();
    for &(center, change) in changes {
        let radius = change.radius();
        for dx in -radius..=radius {
            for dy in -radius..=radius {
                for dz in -radius..=radius {
                    if dx.abs() + dy.abs() + dz.abs() > radius {
                        continue;
                    }
                    let p = offset(center, (dx, dy, dz))?;
                    if indices.contains_key(&p) {
                        continue;
                    }
                    let chunk = world.chunks.get(&p.chunk())?;
                    if chunk.status.as_deref() != Some("minecraft:full")
                        || !chunk.block_light_correct
                    {
                        return None;
                    }
                    let value = behavior_at(world, p, bounds, &mut cache, &mut behaviors)?;
                    let previous = light_at(world, p, bounds)?;
                    indices.insert(p, cells.len());
                    cells.push((
                        p,
                        LightCell {
                            behavior: value,
                            previous,
                            value: behaviors[value as usize].emission,
                        },
                    ));
                }
            }
        }
    }
    let mut queue = BinaryHeap::new();
    for (index, (p, cell)) in cells.iter_mut().enumerate() {
        for (direction_index, direction) in DIRECTIONS.into_iter().enumerate() {
            let next = offset(*p, direction)?;
            if indices.contains_key(&next) {
                continue;
            }
            let chunk = world.chunks.get(&next.chunk())?;
            if chunk.status.as_deref() != Some("minecraft:full") || !chunk.block_light_correct {
                return None;
            }
            let other = behavior_at(world, next, bounds, &mut cache, &mut behaviors)?;
            if (behaviors[cell.behavior as usize].has_faces || behaviors[other as usize].has_faces)
                && faces_occlude(
                    &behaviors[cell.behavior as usize].faces[direction_index],
                    &behaviors[other as usize].faces[direction_index ^ 1],
                )
            {
                continue;
            }
            let boundary = light_at(world, next, bounds)?;
            cell.value = cell
                .value
                .max(boundary.saturating_sub(behaviors[cell.behavior as usize].opacity.max(1)));
        }
        if cell.value > 0 {
            queue.push((cell.value, index));
        }
    }
    while let Some((level, index)) = queue.pop() {
        if cells[index].1.value != level {
            continue;
        }
        let (p, from) = (cells[index].0, cells[index].1.behavior);
        for (direction_index, direction) in DIRECTIONS.into_iter().enumerate() {
            let next = offset(p, direction)?;
            let Some(&next) = indices.get(&next) else {
                continue;
            };
            let cell = &mut cells[next].1;
            if (behaviors[from as usize].has_faces || behaviors[cell.behavior as usize].has_faces)
                && faces_occlude(
                    &behaviors[from as usize].faces[direction_index],
                    &behaviors[cell.behavior as usize].faces[direction_index ^ 1],
                )
            {
                continue;
            }
            let value = level.saturating_sub(behaviors[cell.behavior as usize].opacity.max(1));
            if value > cell.value {
                cell.value = value;
                queue.push((value, next));
            }
        }
    }
    let mut changed = BTreeSet::new();
    for (p, cell) in cells {
        if cell.previous == cell.value {
            continue;
        }
        let chunk_pos = p.chunk();
        write_light(world, p, cell.value, bounds)?;
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
        assert!(faces_occlude(&bottom_faces[0], &top_faces[1]));
        assert!(!faces_occlude(&bottom_faces[0], &bottom_faces[1]));
        assert!(light_change(&bottom, &top).is_some());
        assert!(light_change(&bottom, &bottom).is_none());
    }

    #[test]
    fn thin_snow_and_path_edges_keep_their_sixteenth_block_gap() {
        let top = BlockState {
            name: "minecraft:stone_slab".into(),
            properties: [
                ("type".into(), "top".into()),
                ("waterlogged".into(), "false".into()),
            ]
            .into(),
        };
        let snow = |layers: &str| BlockState {
            name: "minecraft:snow".into(),
            properties: [("layers".into(), layers.into())].into(),
        };
        let path = BlockState {
            name: "minecraft:dirt_path".into(),
            properties: BTreeMap::new(),
        };
        let slab = face_masks(&top).unwrap();
        let thin = face_masks(&snow("1")).unwrap();
        let deep = face_masks(&snow("7")).unwrap();
        let path = face_masks(&path).unwrap();
        assert!(!faces_occlude(&thin[0], &slab[1]));
        assert!(faces_occlude(&deep[0], &slab[1]));
        assert!(faces_occlude(&path[0], &slab[1]));
        assert!(!faces_occlude(&path[0], &[0; 16]));
        assert!(light_change(&snow("1"), &snow("7")).is_some());
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
    fn level_one_mushroom_light_stays_within_its_cell() {
        let (mut world, _, _, _) = fixture();
        let center = position(15, 15, 15);
        let air = world.block(center.x, center.y, center.z).unwrap().clone();
        let mushroom = state("brown_mushroom", None);
        let id = world.intern_state(mushroom.clone());
        set(&mut world, center, id);
        relight_block(&mut world, center, light_change(&air, &mushroom).unwrap()).unwrap();
        assert_eq!(light(&world, center), 1);
        assert_eq!(light(&world, position(16, 15, 15)), 0);
        let air_id = world.intern_state(air.clone());
        set(&mut world, center, air_id);
        relight_block(&mut world, center, light_change(&mushroom, &air).unwrap()).unwrap();
        assert_eq!(light(&world, center), 0);
    }

    #[test]
    fn light_updates_in_the_padding_above_and_below_build_bounds() {
        let (mut world, _, _, _) = fixture();
        let air = state("air", None);
        let torch = state("torch", None);
        let air_id = world.intern_state(air.clone());
        let torch_id = world.intern_state(torch.clone());
        let bounds = LightBounds::new(0, 32).unwrap();
        let add = light_change(&air, &torch).unwrap();
        let remove = light_change(&torch, &air).unwrap();
        for center in [position(15, 0, 15), position(15, 31, 15)] {
            let above = center.y == 31;
            set(&mut world, center, torch_id);
            let old = world.clone();
            relight_changes_in_bounds(&mut world, &[(center, add)], bounds).unwrap();
            let outside_y = if above { 32 } else { -1 };
            assert_eq!(world.light(center.x, outside_y, center.z, false), Some(13));
            assert_eq!(old.light(center.x, outside_y, center.z, false), None);
            let chunk = &world.chunks[&center.chunk()];
            assert!(
                chunk.sections[&i8::try_from(outside_y.div_euclid(16)).unwrap()]
                    .blocks
                    .is_none()
            );
            set(&mut world, center, air_id);
            relight_changes_in_bounds(&mut world, &[(center, remove)], bounds).unwrap();
            assert_eq!(world.light(center.x, outside_y, center.z, false), Some(0));
        }
        assert!(LightBounds::new(1, 32).is_none());
        assert!(LightBounds::new(0, 31).is_none());
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn build_height_padding_matches_vanilla_light_engine() {
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
        let actions = [
            (0, "torch", false),
            (31, "torch", false),
            (0, "air", false),
            (31, "air", false),
            (0, "torch", true),
            (31, "torch", false),
            (0, "air", true),
            (31, "air", false),
        ];
        let entries: Vec<_> = actions.iter().map(|(y,state,defer)| serde_json::json!({"x":15,"y":y,"z":15,"state":state,"defer":defer})).collect();
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(&input, serde_json::to_vec(&serde_json::json!({"min_y":0,"height":32,"sample_min_y":-15,"sample_height":62,"initial":[],"actions":entries})).unwrap()).unwrap();
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("BlockLightEngineOracle")
            .arg(&input)
            .arg(&output));
        let expected: Vec<Vec<u8>> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(expected.len(), 6);
        let (mut world, _, _, _) = fixture();
        let air = world.intern_state(state("air", None));
        let torch = world.intern_state(state("torch", None));
        let bounds = LightBounds::new(0, 32).unwrap();
        let mut pending = Vec::new();
        let mut expected = expected.into_iter();
        for (y, name, defer) in actions {
            let p = position(15, y, 15);
            let id = if name == "torch" { torch } else { air };
            let change = light_change(
                world.block(p.x, p.y, p.z).unwrap(),
                world.state(id).unwrap(),
            )
            .unwrap();
            set(&mut world, p, id);
            pending.push((p, change));
            if defer {
                continue;
            }
            relight_changes_in_bounds(&mut world, &pending, bounds).unwrap();
            pending.clear();
            let expected = expected.next().unwrap();
            assert_eq!(expected.len(), 62 * 31 * 31);
            for y in -15..47 {
                for z in 0..31 {
                    for x in 0..31 {
                        let index = ((y + 15) * 31 * 31 + z * 31 + x) as usize;
                        assert_eq!(
                            world.light(x, y, z, false).unwrap_or(0),
                            expected[index],
                            "after {name} at {p:?}, checking {x},{y},{z}"
                        );
                    }
                }
            }
        }
        assert!(expected.next().is_none());
    }

    #[test]
    fn overlapping_deferred_source_removals_clear_each_others_old_light() {
        let (mut world, _, _, _) = fixture();
        let air = state("air", None);
        let torch = state("torch", None);
        let air_id = world.intern_state(air.clone());
        let torch_id = world.intern_state(torch.clone());
        let locations = [position(14, 15, 15), position(16, 15, 15)];
        let add = light_change(&air, &torch).unwrap();
        let remove = light_change(&torch, &air).unwrap();
        for &pos in &locations {
            set(&mut world, pos, torch_id);
            relight_block(&mut world, pos, add).unwrap();
        }
        let mut sequential = world.clone();
        let mut deferred = world;
        for &pos in &locations {
            set(&mut sequential, pos, air_id);
            relight_block(&mut sequential, pos, remove).unwrap();
            set(&mut deferred, pos, air_id);
        }
        let before = deferred.clone();
        relight_changes(&mut deferred, &locations.map(|pos| (pos, remove))).unwrap();
        for y in 0..32 {
            for z in 0..32 {
                for x in 0..32 {
                    assert_eq!(
                        light(&deferred, position(x, y, z)),
                        light(&sequential, position(x, y, z)),
                        "{x},{y},{z}"
                    );
                }
            }
        }
        assert_eq!(light(&deferred, locations[0]), 0);
        assert!(light(&before, locations[0]) > 0);
    }

    #[test]
    fn overlapping_relight_leaves_every_chunk_untouched_when_data_is_unknown() {
        let (mut world, _, _, _) = fixture();
        let unknown = world.intern_state(state("unknown_block", None));
        set(&mut world, position(15, 15, 16), unknown);
        let before = world.clone();
        let changes = [
            (position(14, 15, 15), LightChange::unknown()),
            (position(16, 15, 15), LightChange::unknown()),
        ];
        assert!(relight_changes(&mut world, &changes).is_none());
        for (key, chunk) in &world.chunks {
            assert!(Arc::ptr_eq(chunk, &before.chunks[key]));
        }
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn overlapping_batches_match_vanilla_light_engine() {
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
        let actions = [
            (15, "air", true),
            (16, "air", false),
            (15, "torch", true),
            (16, "torch", false),
            (15, "slab_bottom", true),
            (16, "slab_top", false),
            (15, "torch", true),
            (16, "snow_7", false),
        ];
        let initial = [15, 16].map(|x| serde_json::json!({"x":x,"y":15,"z":15,"state":"torch"}));
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        let entries: Vec<_> = actions.iter().map(|(x,state,defer)| serde_json::json!({"x":x,"y":15,"z":15,"state":state,"defer":defer})).collect();
        std::fs::write(
            &input,
            serde_json::to_vec(&serde_json::json!({"initial":initial,"actions":entries})).unwrap(),
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
        assert_eq!(expected.len(), 4);
        let (mut world, _, _, _) = fixture();
        let air = world.intern_state(state("air", None));
        let torch = world.intern_state(state("torch", None));
        let slab = |kind: &str| BlockState {
            name: "minecraft:stone_slab".into(),
            properties: [
                ("type".into(), kind.into()),
                ("waterlogged".into(), "false".into()),
            ]
            .into(),
        };
        let bottom = world.intern_state(slab("bottom"));
        let top = world.intern_state(slab("top"));
        let snow = world.intern_state(BlockState {
            name: "minecraft:snow".into(),
            properties: [("layers".into(), "7".into())].into(),
        });
        let id = |name: &str| match name {
            "air" => air,
            "torch" => torch,
            "slab_bottom" => bottom,
            "slab_top" => top,
            "snow_7" => snow,
            _ => unreachable!(),
        };
        for x in [15, 16] {
            let p = position(x, 15, 15);
            let change =
                light_change(world.state(air).unwrap(), world.state(torch).unwrap()).unwrap();
            set(&mut world, p, torch);
            relight_block(&mut world, p, change).unwrap();
        }
        let mut pending = Vec::new();
        let mut expected = expected.into_iter();
        for (x, name, defer) in actions {
            let p = position(x, 15, 15);
            let change = light_change(
                world.block(p.x, p.y, p.z).unwrap(),
                world.state(id(name)).unwrap(),
            )
            .unwrap();
            set(&mut world, p, id(name));
            pending.push((p, change));
            if defer {
                continue;
            }
            relight_changes(&mut world, &pending).unwrap();
            pending.clear();
            let expected = expected.next().unwrap();
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
        assert!(expected.next().is_none());
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
            "dirt_path",
            "farmland",
            "snow",
            "dandelion",
            "poppy",
            "blue_orchid",
            "allium",
            "azure_bluet",
            "red_tulip",
            "orange_tulip",
            "white_tulip",
            "pink_tulip",
            "oxeye_daisy",
            "cornflower",
            "lily_of_the_valley",
            "wither_rose",
            "torchflower",
            "closed_eyeblossom",
            "open_eyeblossom",
            "dead_bush",
            "brown_mushroom",
            "red_mushroom",
            "oak_sapling",
            "spruce_sapling",
            "birch_sapling",
            "jungle_sapling",
            "acacia_sapling",
            "dark_oak_sapling",
            "cherry_sapling",
            "pale_oak_sapling",
            "azalea",
            "flowering_azalea",
            "wheat",
            "carrots",
            "potatoes",
            "beetroots",
            "sugar_cane",
            "cactus",
            "spruce_leaves",
            "birch_leaves",
            "jungle_leaves",
            "acacia_leaves",
            "dark_oak_leaves",
            "mangrove_leaves",
            "cherry_leaves",
            "pale_oak_leaves",
            "azalea_leaves",
            "flowering_azalea_leaves",
            "cobbled_deepslate",
            "stone_bricks",
            "cobblestone",
            "coarse_dirt",
            "rooted_dirt",
            "podzol",
            "mycelium",
            "mossy_cobblestone",
            "tuff_bricks",
            "polished_tuff",
            "polished_andesite",
            "polished_granite",
            "polished_diorite",
            "end_stone",
            "crafting_table",
            "sandstone",
            "red_sandstone",
            "nether_bricks",
            "quartz_block",
            "ice",
            "packed_ice",
            "blue_ice",
            "bamboo_mosaic",
            "mangrove_roots",
            "muddy_mangrove_roots",
            "soul_sand",
            "soul_soil",
            "smooth_stone",
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
        requested.extend(native_core::wood::Wood::all().map(|wood| wood.id().to_owned()));
        requested.extend(
            native_core::wood::Planks::ALL
                .into_iter()
                .map(|planks| planks.id().to_owned()),
        );
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
            let shaped = (shape_family(&s.name)
                && s.properties.get("type").is_none_or(|v| v != "double"))
                || matches!(
                    s.name.as_str(),
                    "minecraft:dirt_path" | "minecraft:farmland" | "minecraft:snow"
                );
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
            (12, 15, 15, "snow_1"),
            (12, 15, 14, "dirt_path"),
            (12, 15, 16, "spruce_log"),
            (12, 15, 17, "birch_leaves"),
            (13, 15, 17, "ice"),
            (14, 15, 17, "podzol"),
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
            (15, 15, 15, "snow_1"),
            (15, 15, 15, "snow_7"),
            (15, 15, 15, "snow_8"),
            (15, 15, 15, "farmland"),
            (15, 15, 15, "dirt_path"),
            (15, 15, 15, "air"),
            (15, 15, 15, "torch"),
            (15, 15, 15, "air"),
            (15, 15, 15, "stone"),
            (15, 15, 15, "air"),
            (15, 15, 15, "water"),
            (15, 15, 15, "wet_pane"),
            (15, 15, 15, "air"),
            (15, 15, 15, "spruce_log"),
            (15, 15, 15, "birch_leaves"),
            (15, 15, 15, "ice"),
            (15, 15, 15, "podzol"),
            (15, 15, 15, "air"),
            (15, 30, 15, "brown_mushroom"),
            (15, 30, 15, "air"),
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
        let snow = |layers: &str| BlockState {
            name: "minecraft:snow".into(),
            properties: [("layers".into(), layers.into())].into(),
        };
        let snow_1 = world.intern_state(snow("1"));
        let snow_7 = world.intern_state(snow("7"));
        let snow_8 = world.intern_state(snow("8"));
        let farmland = world.intern_state(BlockState {
            name: "minecraft:farmland".into(),
            properties: [("moisture".into(), "0".into())].into(),
        });
        let dirt_path = world.intern_state(state("dirt_path", None));
        let brown_mushroom = world.intern_state(state("brown_mushroom", None));
        let spruce_log = world.intern_state(BlockState {
            name: "minecraft:spruce_log".into(),
            properties: [("axis".into(), "x".into())].into(),
        });
        let birch_leaves = world.intern_state(BlockState {
            name: "minecraft:birch_leaves".into(),
            properties: [
                ("distance".into(), "3".into()),
                ("persistent".into(), "true".into()),
                ("waterlogged".into(), "false".into()),
            ]
            .into(),
        });
        let ice = world.intern_state(state("ice", None));
        let podzol = world.intern_state(BlockState {
            name: "minecraft:podzol".into(),
            properties: [("snowy".into(), "true".into())].into(),
        });
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
            "snow_1" => snow_1,
            "snow_7" => snow_7,
            "snow_8" => snow_8,
            "farmland" => farmland,
            "dirt_path" => dirt_path,
            "spruce_log" => spruce_log,
            "birch_leaves" => birch_leaves,
            "ice" => ice,
            "podzol" => podzol,
            "brown_mushroom" => brown_mushroom,
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
