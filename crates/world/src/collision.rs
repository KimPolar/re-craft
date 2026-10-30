//! Incremental collision shapes for saved-world play. Unknown states are explicit.

use glam::{IVec3, Vec3};

use crate::loaded::BlockState;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

const EMPTY: &[Aabb] = &[];
const FULL: &[Aabb] = &[Aabb {
    min: Vec3::ZERO,
    max: Vec3::ONE,
}];
const INDENTED: &[Aabb] = &[Aabb {
    min: Vec3::ZERO,
    max: Vec3::new(1.0, 15.0 / 16.0, 1.0),
}];
const HONEY: &[Aabb] = &[Aabb {
    min: Vec3::new(1.0 / 16.0, 0.0, 1.0 / 16.0),
    max: Vec3::new(15.0 / 16.0, 15.0 / 16.0, 15.0 / 16.0),
}];
const SINGLE_CHEST: &[Aabb] = &[Aabb {
    min: Vec3::new(1.0 / 16.0, 0.0, 1.0 / 16.0),
    max: Vec3::new(15.0 / 16.0, 14.0 / 16.0, 15.0 / 16.0),
}];

/// Collision and outline for blocks represented by the temporary demo world.
pub fn demo_shape(block: crate::Block) -> &'static [Aabb] {
    match block {
        crate::Block::Air => EMPTY,
        crate::Block::DirtPath | crate::Block::Farmland(_) => INDENTED,
        _ => FULL,
    }
}
const BOTTOM_SLAB: &[Aabb] = &[Aabb {
    min: Vec3::ZERO,
    max: Vec3::new(1.0, 0.5, 1.0),
}];
const TOP_SLAB: &[Aabb] = &[Aabb {
    min: Vec3::new(0.0, 0.5, 0.0),
    max: Vec3::ONE,
}];
const FLOWER_POT: &[Aabb] = &[Aabb {
    min: Vec3::new(5.0 / 16.0, 0.0, 5.0 / 16.0),
    max: Vec3::new(11.0 / 16.0, 6.0 / 16.0, 11.0 / 16.0),
}];

/// Whether the supplied collision boxes cover an entire block boundary face.
/// Blocks may still override face sturdiness independently of their shape.
pub fn covers_full_face(boxes: &[Aabb], direction: IVec3) -> bool {
    merged_face_occludes(boxes, &[], direction)
}

/// Full-face support for the currently verified block families. Some blocks
/// override the support shape, so unknown families must not be inferred from
/// collision geometry alone.
pub fn button_support_face(state: &BlockState, direction: IVec3) -> Option<bool> {
    if !matches!(
        direction,
        IVec3::X | IVec3::NEG_X | IVec3::Y | IVec3::NEG_Y | IVec3::Z | IVec3::NEG_Z
    ) {
        return None;
    }
    let id = state.name.strip_prefix("minecraft:")?;
    if matches!(id, "air" | "cave_air" | "void_air") {
        return Some(false);
    }
    let shape = block_shape(state)?;
    if shape.is_empty() {
        return Some(false);
    }
    if matches!(id, "dirt_path" | "farmland" | "straw_bed") {
        return Some(direction == IVec3::NEG_Y);
    }
    // These blocks do not supply a full sturdy face, even where collision
    // geometry (notably leaves) appears to cover the whole cube.
    if id == "honey_block"
        || matches!(id, "chest" | "trapped_chest")
        || id == "flower_pot"
        || id.starts_with("potted_")
        || id == "glass_pane"
        || id == "iron_bars"
        || id.ends_with("_stained_glass_pane")
        || id.ends_with("_fence")
        || id.ends_with("_leaves")
        || id.ends_with("_bed")
    {
        return Some(false);
    }
    if id.ends_with("_slab") || id.ends_with("_stairs") {
        return Some(covers_full_face(&shape, direction));
    }
    if shape.as_slice() == FULL {
        return Some(true);
    }
    None
}

/// Whether the two collision shapes jointly cover their shared boundary.
pub fn merged_face_occludes(from: &[Aabb], to: &[Aabb], direction: IVec3) -> bool {
    let (axis, positive) = if direction.x != 0 {
        (0, direction.x > 0)
    } else if direction.y != 0 {
        (1, direction.y > 0)
    } else if direction.z != 0 {
        (2, direction.z > 0)
    } else {
        return false;
    };
    let other: Vec<_> = (0..3).filter(|a| *a != axis).collect();
    let touching_face = |boxes: &[Aabb], high: bool| {
        let extreme = boxes
            .iter()
            .map(|b| if high { b.max[axis] } else { b.min[axis] })
            .reduce(if high { f32::max } else { f32::min });
        let target = if high { 1.0 } else { 0.0 };
        let at_boundary = extreme.is_some_and(|edge| (edge - target).abs() <= 1.0e-7);
        boxes
            .iter()
            .copied()
            .filter(move |b| {
                at_boundary
                    && if high {
                        b.min[axis] < 1.0 && b.max[axis] >= 1.0 - 1.0e-7
                    } else {
                        b.min[axis] <= 1.0e-7 && b.max[axis] > 0.0
                    }
            })
            .collect::<Vec<_>>()
    };
    let mut touching = touching_face(from, positive);
    touching.extend(touching_face(to, !positive));
    let cuts = |axis| {
        let mut values = vec![0.0_f32, 1.0];
        for b in &touching {
            values.push(b.min[axis].clamp(0.0, 1.0));
            values.push(b.max[axis].clamp(0.0, 1.0));
        }
        values.sort_by(f32::total_cmp);
        values.dedup();
        values
    };
    let a = cuts(other[0]);
    let b = cuts(other[1]);
    a.windows(2).all(|x| {
        b.windows(2).all(|y| {
            let x = (x[0] + x[1]) * 0.5;
            let y = (y[0] + y[1]) * 0.5;
            touching.iter().any(|shape| {
                shape.min[other[0]] <= x
                    && shape.max[other[0]] >= x
                    && shape.min[other[1]] <= y
                    && shape.max[other[1]] >= y
            })
        })
    })
}

#[cfg(test)]
mod face_tests {
    use super::*;

    #[test]
    fn full_face_requires_complete_coverage_on_the_requested_boundary() {
        assert!(covers_full_face(FULL, IVec3::X));
        assert!(covers_full_face(BOTTOM_SLAB, IVec3::NEG_Y));
        assert!(!covers_full_face(BOTTOM_SLAB, IVec3::Y));
        assert!(!covers_full_face(BOTTOM_SLAB, IVec3::X));
        assert!(covers_full_face(&[BOTTOM_SLAB[0], TOP_SLAB[0]], IVec3::X));
        assert!(!covers_full_face(EMPTY, IVec3::Y));
        assert!(merged_face_occludes(BOTTOM_SLAB, TOP_SLAB, IVec3::X));
        assert!(!merged_face_occludes(BOTTOM_SLAB, BOTTOM_SLAB, IVec3::X));
        assert!(!merged_face_occludes(EMPTY, TOP_SLAB, IVec3::Y));
        assert!(merged_face_occludes(EMPTY, TOP_SLAB, IVec3::NEG_Y));
    }
}

/// `None` means that this state's physical shape is not implemented.
pub fn block_shape(state: &BlockState) -> Option<Vec<Aabb>> {
    let id = state.name.strip_prefix("minecraft:")?;
    if matches!(id, "chest" | "trapped_chest") {
        if !(state.properties.len() == 3
            && matches!(
                state.properties.get("facing").map(String::as_str),
                Some("north" | "south" | "west" | "east")
            )
            && matches!(
                state.properties.get("type").map(String::as_str),
                Some("single" | "left" | "right")
            )
            && matches!(
                state.properties.get("waterlogged").map(String::as_str),
                Some("true" | "false")
            ))
        {
            return None;
        }
        let mut shape = SINGLE_CHEST[0];
        if state
            .properties
            .get("type")
            .is_some_and(|value| value != "single")
        {
            let direction = chest_connection(state)?;
            if direction.x < 0 {
                shape.min.x = 0.0;
            }
            if direction.x > 0 {
                shape.max.x = 1.0;
            }
            if direction.z < 0 {
                shape.min.z = 0.0;
            }
            if direction.z > 0 {
                shape.max.z = 1.0;
            }
        }
        return Some(vec![shape]);
    }
    if id == "lever" {
        lever_outline(state)?;
        return Some(EMPTY.to_vec());
    }
    if id.ends_with("_button") {
        button_outline(state)?;
        return Some(EMPTY.to_vec());
    }
    if matches!(id, "dirt_path" | "farmland") {
        return Some(INDENTED.to_vec());
    }
    if id == "honey_block" {
        return Some(HONEY.to_vec());
    }
    if id == "straw_bed" {
        return straw_bed(state);
    }
    if id.ends_with("_bed") {
        return bed(state);
    }
    if id == "flower_pot" || id.starts_with("potted_") {
        return Some(FLOWER_POT.to_vec());
    }
    if matches!(
        id,
        "air"
            | "cave_air"
            | "void_air"
            | "water"
            | "lava"
            | "bubble_column"
            | "fire"
            | "soul_fire"
            | "kelp"
            | "kelp_plant"
            | "seagrass"
            | "tall_seagrass"
            | "short_grass"
            | "tall_grass"
            | "fern"
            | "large_fern"
            | "dead_bush"
            | "dandelion"
            | "poppy"
            | "blue_orchid"
            | "allium"
            | "azure_bluet"
            | "oxeye_daisy"
            | "cornflower"
            | "lily_of_the_valley"
            | "torch"
            | "wall_torch"
            | "soul_torch"
            | "soul_wall_torch"
            | "vine"
    ) || id.ends_with("_sapling")
        || id.ends_with("_tulip")
    {
        return Some(EMPTY.to_vec());
    }
    if id.ends_with("_slab") {
        return match state.properties.get("type").map(String::as_str) {
            Some("bottom") => Some(BOTTOM_SLAB.to_vec()),
            Some("top") => Some(TOP_SLAB.to_vec()),
            Some("double") => Some(FULL.to_vec()),
            _ => None,
        };
    }
    if id.ends_with("_stairs") {
        return stairs(state);
    }
    if id.ends_with("_fence") {
        return connected(state, 6.0 / 16.0, 10.0 / 16.0, 1.5);
    }
    if id == "glass_pane" || id == "iron_bars" || id.ends_with("_stained_glass_pane") {
        return connected(state, 7.0 / 16.0, 9.0 / 16.0, 1.0);
    }
    if matches!(
        id,
        "stone"
            | "stone_bricks"
            | "redstone_lamp"
            | "redstone_block"
            | "crafting_table"
            | "granite"
            | "diorite"
            | "andesite"
            | "deepslate"
            | "cobbled_deepslate"
            | "tuff"
            | "dirt"
            | "coarse_dirt"
            | "rooted_dirt"
            | "grass_block"
            | "podzol"
            | "mycelium"
            | "sand"
            | "red_sand"
            | "gravel"
            | "sandstone"
            | "red_sandstone"
            | "cobblestone"
            | "mossy_cobblestone"
            | "bedrock"
            | "netherrack"
            | "end_stone"
            | "obsidian"
            | "respawn_anchor"
            | "basalt"
            | "soul_soil"
            | "snow_block"
            | "slime_block"
            | "hay_block"
            | "ice"
            | "packed_ice"
            | "blue_ice"
            | "clay"
            | "glass"
            | "glowstone"
            | "coal_block"
            | "iron_block"
            | "gold_block"
            | "diamond_block"
            | "crimson_stem"
            | "warped_stem"
            | "stripped_crimson_stem"
            | "stripped_warped_stem"
            | "bamboo_block"
            | "stripped_bamboo_block"
    ) || [
        "_log",
        "_wood",
        "_hyphae",
        "_leaves",
        "_planks",
        "_wool",
        "_concrete",
        "_ore",
        "_terracotta",
        "_stained_glass",
    ]
    .iter()
    .any(|suffix| id.ends_with(suffix))
    {
        return Some(FULL.to_vec());
    }
    None
}

/// Direction from one double chest half toward the other, as in ChestBlock.
pub fn chest_connection(state: &BlockState) -> Option<IVec3> {
    if !matches!(
        state.name.as_str(),
        "minecraft:chest" | "minecraft:trapped_chest"
    ) || state.properties.len() != 3
        || !matches!(
            state.properties.get("waterlogged").map(String::as_str),
            Some("true" | "false")
        )
    {
        return None;
    }
    let facing = match state.properties.get("facing")?.as_str() {
        "north" => IVec3::NEG_Z,
        "south" => IVec3::Z,
        "west" => IVec3::NEG_X,
        "east" => IVec3::X,
        _ => return None,
    };
    match state.properties.get("type")?.as_str() {
        "left" => Some(IVec3::new(-facing.z, 0, facing.x)),
        "right" => Some(IVec3::new(facing.z, 0, -facing.x)),
        _ => None,
    }
}

/// Java's cached `isSolid` for verified vanilla slab and stair states.
/// The caller must confirm the block ID belongs to the installed registry.
pub fn slab_stair_solid(state: &BlockState) -> Option<bool> {
    let name = state.name.strip_prefix("minecraft:")?;
    let property = |key: &str| state.properties.get(key).map(String::as_str);
    if !matches!(property("waterlogged"), Some("true" | "false")) {
        return None;
    }
    if name.ends_with("_slab") && state.properties.len() == 2 {
        return matches!(property("type"), Some("bottom" | "top" | "double")).then_some(true);
    }
    if name.ends_with("_stairs") && state.properties.len() == 4 {
        return (matches!(
            property("facing"),
            Some("north" | "south" | "west" | "east")
        ) && matches!(property("half"), Some("top" | "bottom"))
            && matches!(
                property("shape"),
                Some("straight" | "inner_left" | "inner_right" | "outer_left" | "outer_right")
            ))
        .then_some(true);
    }
    None
}

/// CollisionContext.emptyWithFluidCollisions, used by Java's shared-spawn
/// search. LiquidBlock treats every water and lava level as a full cube in
/// this context; waterlogged blocks keep their own block collision shape.
pub fn spawn_collision_shape(state: &BlockState) -> Option<Vec<Aabb>> {
    if matches!(state.name.as_str(), "minecraft:water" | "minecraft:lava") {
        Some(FULL.to_vec())
    } else {
        block_shape(state)
    }
}

/// Shape used for block targeting. `None` leaves unsupported outlines explicit.
pub fn block_outline(state: &BlockState) -> Option<Vec<Aabb>> {
    let id = state.name.strip_prefix("minecraft:")?;
    if id == "lever" {
        return lever_outline(state);
    }
    if id.ends_with("_button") {
        return button_outline(state);
    }
    if id == "honey_block" {
        return Some(FULL.to_vec());
    }
    if matches!(id, "air" | "cave_air" | "void_air" | "water" | "lava") {
        return Some(Vec::new());
    }
    if id == "flower_pot" || id.starts_with("potted_") {
        return Some(FLOWER_POT.to_vec());
    }
    if matches!(id, "short_grass" | "fern" | "dead_bush") {
        return Some(vec![cuboid(
            2.0 / 16.0,
            0.0,
            2.0 / 16.0,
            14.0 / 16.0,
            13.0 / 16.0,
            14.0 / 16.0,
        )]);
    }
    if matches!(id, "tall_grass" | "large_fern") {
        return Some(FULL.to_vec());
    }
    // Bamboo saplings have a position-dependent offset, unlike tree saplings.
    if id.ends_with("_sapling") && id != "bamboo_sapling" {
        return Some(vec![cuboid(
            2.0 / 16.0,
            0.0,
            2.0 / 16.0,
            14.0 / 16.0,
            12.0 / 16.0,
            14.0 / 16.0,
        )]);
    }
    if matches!(id, "torch" | "soul_torch") {
        return Some(vec![cuboid(
            6.0 / 16.0,
            0.0,
            6.0 / 16.0,
            10.0 / 16.0,
            10.0 / 16.0,
            10.0 / 16.0,
        )]);
    }
    if matches!(id, "wall_torch" | "soul_wall_torch") {
        let center0 = 5.5 / 16.0;
        let center1 = 10.5 / 16.0;
        let edge = 5.0 / 16.0;
        return Some(vec![
            match state.properties.get("facing").map(String::as_str)? {
                "north" => cuboid(center0, 3.0 / 16.0, 1.0 - edge, center1, 13.0 / 16.0, 1.0),
                "south" => cuboid(center0, 3.0 / 16.0, 0.0, center1, 13.0 / 16.0, edge),
                "west" => cuboid(1.0 - edge, 3.0 / 16.0, center0, 1.0, 13.0 / 16.0, center1),
                "east" => cuboid(0.0, 3.0 / 16.0, center0, edge, 13.0 / 16.0, center1),
                _ => return None,
            },
        ]);
    }
    if id == "vine" {
        return vine(state);
    }
    if id.ends_with("_fence") {
        return connected(state, 6.0 / 16.0, 10.0 / 16.0, 1.0);
    }
    let shape = block_shape(state)?;
    if shape.is_empty() { None } else { Some(shape) }
}

fn lever_outline(state: &BlockState) -> Option<Vec<Aabb>> {
    if state.properties.len() != 3
        || !matches!(
            state.properties.get("powered").map(String::as_str),
            Some("true" | "false")
        )
    {
        return None;
    }
    let facing = state.properties.get("facing")?.as_str();
    let (a, b, c, d) = match facing {
        "north" | "south" => (5.0, 11.0, 4.0, 12.0),
        "west" | "east" => (4.0, 12.0, 5.0, 11.0),
        _ => return None,
    };
    let s = 1.0 / 16.0;
    let shape = match state.properties.get("face")?.as_str() {
        "floor" => cuboid(a * s, 0.0, c * s, b * s, 6.0 * s, d * s),
        "ceiling" => cuboid(a * s, 10.0 * s, c * s, b * s, 1.0, d * s),
        "wall" => match facing {
            "north" => cuboid(5.0 * s, 4.0 * s, 10.0 * s, 11.0 * s, 12.0 * s, 1.0),
            "south" => cuboid(5.0 * s, 4.0 * s, 0.0, 11.0 * s, 12.0 * s, 6.0 * s),
            "west" => cuboid(10.0 * s, 4.0 * s, 5.0 * s, 1.0, 12.0 * s, 11.0 * s),
            "east" => cuboid(0.0, 4.0 * s, 5.0 * s, 6.0 * s, 12.0 * s, 11.0 * s),
            _ => return None,
        },
        _ => return None,
    };
    Some(vec![shape])
}

fn button_outline(state: &BlockState) -> Option<Vec<Aabb>> {
    let facing = state.properties.get("facing")?.as_str();
    let depth = match state.properties.get("powered")?.as_str() {
        "true" => 1.0 / 16.0,
        "false" => 2.0 / 16.0,
        _ => return None,
    };
    let a = 5.0 / 16.0;
    let b = 11.0 / 16.0;
    let c = 6.0 / 16.0;
    let d = 10.0 / 16.0;
    let horizontal = match facing {
        "north" | "south" => (a, b, c, d),
        "west" | "east" => (c, d, a, b),
        _ => return None,
    };
    let shape = match state.properties.get("face")?.as_str() {
        "floor" => cuboid(
            horizontal.0,
            0.0,
            horizontal.2,
            horizontal.1,
            depth,
            horizontal.3,
        ),
        "ceiling" => cuboid(
            horizontal.0,
            1.0 - depth,
            horizontal.2,
            horizontal.1,
            1.0,
            horizontal.3,
        ),
        "wall" => match facing {
            "north" => cuboid(a, c, 1.0 - depth, b, d, 1.0),
            "south" => cuboid(a, c, 0.0, b, d, depth),
            "west" => cuboid(1.0 - depth, c, a, 1.0, d, b),
            "east" => cuboid(0.0, c, a, depth, d, b),
            _ => return None,
        },
        _ => return None,
    };
    Some(vec![shape])
}

fn bed(state: &BlockState) -> Option<Vec<Aabb>> {
    let facing = state.properties.get("facing")?.as_str();
    let head = match state.properties.get("part")?.as_str() {
        "head" => true,
        "foot" => false,
        _ => return None,
    };
    let edge = match (facing, head) {
        ("north", true) | ("south", false) => "north",
        ("south", true) | ("north", false) => "south",
        ("west", true) | ("east", false) => "west",
        ("east", true) | ("west", false) => "east",
        _ => return None,
    };
    let d = 3.0 / 16.0;
    let height = 9.0 / 16.0;
    let mut boxes = vec![cuboid(0.0, d, 0.0, 1.0, height, 1.0)];
    match edge {
        "north" | "south" => {
            let z = if edge == "north" { 0.0 } else { 1.0 - d };
            boxes.push(cuboid(0.0, 0.0, z, d, d, z + d));
            boxes.push(cuboid(1.0 - d, 0.0, z, 1.0, d, z + d));
        }
        _ => {
            let x = if edge == "west" { 0.0 } else { 1.0 - d };
            boxes.push(cuboid(x, 0.0, 0.0, x + d, d, d));
            boxes.push(cuboid(x, 0.0, 1.0 - d, x + d, d, 1.0));
        }
    }
    Some(boxes)
}

fn straw_bed(state: &BlockState) -> Option<Vec<Aabb>> {
    let mut shapes = vec![cuboid(0.0, 0.0, 0.0, 1.0, 0.25, 1.0)];
    match state.properties.get("part")?.as_str() {
        "foot" => {}
        "head" => shapes.push(match state.properties.get("facing")?.as_str() {
            "north" => cuboid(0.0, 0.25, 0.0, 1.0, 5.0 / 16.0, 0.5),
            "south" => cuboid(0.0, 0.25, 0.5, 1.0, 5.0 / 16.0, 1.0),
            "west" => cuboid(0.0, 0.25, 0.0, 0.5, 5.0 / 16.0, 1.0),
            "east" => cuboid(0.5, 0.25, 0.0, 1.0, 5.0 / 16.0, 1.0),
            _ => return None,
        }),
        _ => return None,
    }
    Some(shapes)
}

fn vine(state: &BlockState) -> Option<Vec<Aabb>> {
    let enabled = |name: &str| match state.properties.get(name).map(String::as_str) {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    };
    let (north, south, west, east, up) = (
        enabled("north")?,
        enabled("south")?,
        enabled("west")?,
        enabled("east")?,
        enabled("up")?,
    );
    let edge = 1.0 / 16.0;
    let mut boxes = Vec::new();
    if north {
        boxes.push(cuboid(0.0, 0.0, 0.0, 1.0, 1.0, edge));
    }
    if south {
        boxes.push(cuboid(0.0, 0.0, 1.0 - edge, 1.0, 1.0, 1.0));
    }
    if west {
        boxes.push(cuboid(0.0, 0.0, 0.0, edge, 1.0, 1.0));
    }
    if east {
        boxes.push(cuboid(1.0 - edge, 0.0, 0.0, 1.0, 1.0, 1.0));
    }
    if up {
        boxes.push(cuboid(0.0, 1.0 - edge, 0.0, 1.0, 1.0, 1.0));
    }
    if boxes.is_empty() {
        None
    } else {
        Some(canonical_boxes(&boxes))
    }
}

fn cuboid(x0: f32, y0: f32, z0: f32, x1: f32, y1: f32, z1: f32) -> Aabb {
    Aabb {
        min: Vec3::new(x0, y0, z0),
        max: Vec3::new(x1, y1, z1),
    }
}

fn stairs(state: &BlockState) -> Option<Vec<Aabb>> {
    let property = |name: &str| state.properties.get(name).map(String::as_str);
    let facing = property("facing")?;
    let (base_y0, base_y1, step_y0, step_y1) = match property("half")? {
        "bottom" => (0.0, 0.5, 0.5, 1.0),
        "top" => (0.5, 1.0, 0.0, 0.5),
        _ => return None,
    };
    let kind = property("shape")?;
    if !matches!(
        kind,
        "straight" | "inner_left" | "inner_right" | "outer_left" | "outer_right"
    ) {
        return None;
    }
    if !matches!(facing, "north" | "south" | "west" | "east") {
        return None;
    }
    let mut boxes = vec![cuboid(0.0, base_y0, 0.0, 1.0, base_y1, 1.0)];
    for x in 0..2 {
        for z in 0..2 {
            let (front, left) = match facing {
                "north" => (z == 0, x == 0),
                "south" => (z == 1, x == 1),
                "west" => (x == 0, z == 1),
                "east" => (x == 1, z == 0),
                _ => unreachable!(),
            };
            let filled = match kind {
                "straight" => front,
                "inner_left" => front || left,
                "inner_right" => front || !left,
                "outer_left" => front && left,
                "outer_right" => front && !left,
                _ => unreachable!(),
            };
            if filled {
                boxes.push(cuboid(
                    x as f32 * 0.5,
                    step_y0,
                    z as f32 * 0.5,
                    (x + 1) as f32 * 0.5,
                    step_y1,
                    (z + 1) as f32 * 0.5,
                ));
            }
        }
    }
    Some(canonical_boxes(&boxes))
}

fn connected(state: &BlockState, low: f32, high: f32, height: f32) -> Option<Vec<Aabb>> {
    let connected = |name: &str| match state.properties.get(name).map(String::as_str) {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    };
    let (north, south, west, east) = (
        connected("north")?,
        connected("south")?,
        connected("west")?,
        connected("east")?,
    );
    let mut boxes = vec![cuboid(low, 0.0, low, high, height, high)];
    if north {
        boxes.push(cuboid(low, 0.0, 0.0, high, height, low));
    }
    if south {
        boxes.push(cuboid(low, 0.0, high, high, height, 1.0));
    }
    if west {
        boxes.push(cuboid(0.0, 0.0, low, low, height, high));
    }
    if east {
        boxes.push(cuboid(high, 0.0, low, 1.0, height, high));
    }
    Some(canonical_boxes(&boxes))
}

/// Merge an orthogonal union in deterministic Z, X, then Y order.
/// Besides reducing collision work, stable boxes preserve face selection at edges.
fn canonical_boxes(boxes: &[Aabb]) -> Vec<Aabb> {
    let axes: [Vec<f32>; 3] = std::array::from_fn(|axis| {
        let mut cuts: Vec<_> = boxes
            .iter()
            .flat_map(|box_| [box_.min[axis], box_.max[axis]])
            .collect();
        cuts.sort_by(f32::total_cmp);
        cuts.dedup();
        cuts
    });
    if axes.iter().any(|cuts| cuts.len() < 2) {
        return Vec::new();
    }
    let (nx, ny, nz) = (axes[0].len() - 1, axes[1].len() - 1, axes[2].len() - 1);
    let index = |x: usize, y: usize, z: usize| (y * nx + x) * nz + z;
    let mut cells = vec![false; nx * ny * nz];
    for y in 0..ny {
        for x in 0..nx {
            for z in 0..nz {
                let point = Vec3::new(
                    (axes[0][x] + axes[0][x + 1]) * 0.5,
                    (axes[1][y] + axes[1][y + 1]) * 0.5,
                    (axes[2][z] + axes[2][z + 1]) * 0.5,
                );
                cells[index(x, y, z)] = boxes
                    .iter()
                    .any(|box_| point.cmpgt(box_.min).all() && point.cmplt(box_.max).all());
            }
        }
    }
    let mut result = Vec::new();
    for y in 0..ny {
        for x in 0..nx {
            let mut z = 0;
            while z < nz {
                if !cells[index(x, y, z)] {
                    z += 1;
                    continue;
                }
                let z0 = z;
                while z < nz && cells[index(x, y, z)] {
                    z += 1;
                }
                for zz in z0..z {
                    cells[index(x, y, zz)] = false;
                }
                let mut x1 = x + 1;
                while x1 < nx && (z0..z).all(|zz| cells[index(x1, y, zz)]) {
                    for zz in z0..z {
                        cells[index(x1, y, zz)] = false;
                    }
                    x1 += 1;
                }
                let mut y1 = y + 1;
                while y1 < ny && (x..x1).all(|xx| (z0..z).all(|zz| cells[index(xx, y1, zz)])) {
                    for xx in x..x1 {
                        for zz in z0..z {
                            cells[index(xx, y1, zz)] = false;
                        }
                    }
                    y1 += 1;
                }
                result.push(cuboid(
                    axes[0][x],
                    axes[1][y],
                    axes[2][z0],
                    axes[0][x1],
                    axes[1][y1],
                    axes[2][z],
                ));
            }
        }
    }
    result
}

pub fn unknown_barrier() -> &'static [Aabb] {
    FULL
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn slab_properties_and_unknown_states_are_explicit() {
        let state = |name: &str, properties: &[(&str, &str)]| BlockState {
            name: name.into(),
            properties: properties
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect::<BTreeMap<_, _>>(),
        };
        assert_eq!(
            block_shape(&state("minecraft:stone", &[])),
            Some(FULL.to_vec())
        );
        assert_eq!(
            block_shape(&state("minecraft:water", &[])),
            Some(EMPTY.to_vec())
        );
        assert_eq!(
            block_shape(&state("minecraft:oak_slab", &[("type", "bottom")])),
            Some(BOTTOM_SLAB.to_vec())
        );
        assert_eq!(
            block_shape(&state("minecraft:oak_slab", &[("type", "top")])),
            Some(TOP_SLAB.to_vec())
        );
        assert_eq!(block_shape(&state("minecraft:oak_slab", &[])), None);
        assert_eq!(block_shape(&state("example:unknown", &[])), None);
    }
}
