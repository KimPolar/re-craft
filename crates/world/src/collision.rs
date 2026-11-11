//! Incremental collision shapes for saved-world play. Unknown states are explicit.

use glam::{IVec3, Vec3};

use crate::loaded::BlockState;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

/// Validate the complete built-in jigsaw block state.
pub fn jigsaw_state(state: &BlockState) -> bool {
    state.name == "minecraft:jigsaw"
        && state.properties.len() == 1
        && matches!(
            state.properties.get("orientation").map(String::as_str),
            Some(
                "down_east"
                    | "down_north"
                    | "down_south"
                    | "down_west"
                    | "up_east"
                    | "up_north"
                    | "up_south"
                    | "up_west"
                    | "east_up"
                    | "north_up"
                    | "south_up"
                    | "west_up"
            )
        )
}

/// Validate a barrier state and return its contained-water flag.
pub fn barrier_waterlogged(state: &BlockState) -> Option<bool> {
    if state.name != "minecraft:barrier" || state.properties.len() != 1 {
        return None;
    }
    match state.properties.get("waterlogged")?.as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
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

/// Validate a vanilla fence state and return its contained-water flag.
pub fn fence_waterlogged(state: &BlockState) -> Option<bool> {
    let name = state.name.strip_prefix("minecraft:")?;
    let valid = name == "nether_brick_fence"
        || name.strip_suffix("_fence").is_some_and(|family| {
            native_core::wood::Planks::ALL.into_iter().any(|wood| {
                wood.id()
                    .strip_prefix("minecraft:")
                    .and_then(|id| id.strip_suffix("_planks"))
                    == Some(family)
            })
        });
    if !valid || state.properties.len() != 5 {
        return None;
    }
    for key in ["north", "east", "south", "west"] {
        if !matches!(
            state.properties.get(key).map(String::as_str),
            Some("true" | "false")
        ) {
            return None;
        }
    }
    match state.properties.get("waterlogged").map(String::as_str) {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    }
}

fn dye_color(name: &str) -> bool {
    matches!(
        name,
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
    )
}

/// Validate a stateless vanilla wool or moss carpet.
pub fn simple_carpet(state: &BlockState) -> bool {
    let Some(id) = state.name.strip_prefix("minecraft:") else {
        return false;
    };
    state.properties.is_empty()
        && (id == "moss_carpet" || id.strip_suffix("_carpet").is_some_and(dye_color))
}

/// Validate the built-in ladder's facing and contained-water state.
pub fn ladder_state(state: &BlockState) -> Option<(&str, bool)> {
    if state.name != "minecraft:ladder" || state.properties.len() != 2 {
        return None;
    }
    let facing = match state.properties.get("facing")?.as_str() {
        facing @ ("north" | "south" | "east" | "west") => facing,
        _ => return None,
    };
    let waterlogged = match state.properties.get("waterlogged")?.as_str() {
        "true" => true,
        "false" => false,
        _ => return None,
    };
    Some((facing, waterlogged))
}

/// Validate the 17 built-in candle blocks and return count, lit, waterlogged.
pub fn candle_state(state: &BlockState) -> Option<(u8, bool, bool)> {
    let id = state.name.strip_prefix("minecraft:")?;
    if id != "candle" && !id.strip_suffix("_candle").is_some_and(dye_color) {
        return None;
    }
    if state.properties.len() != 3 {
        return None;
    }
    let count = match state.properties.get("candles")?.as_str() {
        "1" => 1,
        "2" => 2,
        "3" => 3,
        "4" => 4,
        _ => return None,
    };
    let boolean = |name: &str| match state.properties.get(name)?.as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    };
    Some((count, boolean("lit")?, boolean("waterlogged")?))
}

/// Validate a vanilla bed state, returning the end bearing its legs.
pub fn bed_leg_end(state: &BlockState) -> Option<&str> {
    let color = state
        .name
        .strip_prefix("minecraft:")?
        .strip_suffix("_bed")?;
    if !dye_color(color) || state.properties.len() != 3 {
        return None;
    }
    if !matches!(
        state.properties.get("occupied").map(String::as_str),
        Some("true" | "false")
    ) {
        return None;
    }
    let facing = state.properties.get("facing")?.as_str();
    let end = match (facing, state.properties.get("part")?.as_str()) {
        ("north", "head") | ("south", "foot") => "north",
        ("south", "head") | ("north", "foot") => "south",
        ("east", "head") | ("west", "foot") => "east",
        ("west", "head") | ("east", "foot") => "west",
        _ => return None,
    };
    Some(end)
}

/// Recognize the verified, transparent full glass blocks.
pub fn full_glass(state: &BlockState) -> bool {
    let Some(name) = state.name.strip_prefix("minecraft:") else {
        return false;
    };
    state.properties.is_empty()
        && (name == "glass" || name.strip_suffix("_stained_glass").is_some_and(dye_color))
}

/// Recognize stateless vanilla terracotta, including all sixteen dyed blocks.
pub fn full_terracotta(state: &BlockState) -> bool {
    let Some(name) = state.name.strip_prefix("minecraft:") else {
        return false;
    };
    state.properties.is_empty()
        && (name == "terracotta" || name.strip_suffix("_terracotta").is_some_and(dye_color))
}

/// Validate a vanilla glass pane or bars state and return its water flag.
pub fn pane_waterlogged(state: &BlockState) -> Option<bool> {
    let name = state.name.strip_prefix("minecraft:")?;
    let valid = matches!(
        name,
        "glass_pane"
            | "iron_bars"
            | "copper_bars"
            | "exposed_copper_bars"
            | "weathered_copper_bars"
            | "oxidized_copper_bars"
            | "waxed_copper_bars"
            | "waxed_exposed_copper_bars"
            | "waxed_weathered_copper_bars"
            | "waxed_oxidized_copper_bars"
    ) || name
        .strip_suffix("_stained_glass_pane")
        .is_some_and(dye_color);
    if !valid
        || state.properties.len() != 5
        || !["north", "east", "south", "west"].iter().all(|key| {
            matches!(
                state.properties.get(*key).map(String::as_str),
                Some("true" | "false")
            )
        })
    {
        return None;
    }
    match state.properties.get("waterlogged").map(String::as_str) {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    }
}

/// Validate a vanilla fence gate state and return whether it is open.
pub fn fence_gate_open(state: &BlockState) -> Option<bool> {
    let family = state
        .name
        .strip_prefix("minecraft:")?
        .strip_suffix("_fence_gate")?;
    if !native_core::wood::Planks::ALL.into_iter().any(|wood| {
        wood.id()
            .strip_prefix("minecraft:")
            .and_then(|id| id.strip_suffix("_planks"))
            == Some(family)
    }) || state.properties.len() != 4
        || !matches!(
            state.properties.get("facing").map(String::as_str),
            Some("north" | "south" | "east" | "west")
        )
        || !["in_wall", "powered"].iter().all(|key| {
            matches!(
                state.properties.get(*key).map(String::as_str),
                Some("true" | "false")
            )
        })
    {
        return None;
    }
    match state.properties.get("open").map(String::as_str) {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    }
}

/// Validate a vanilla door state and return whether it is open.
pub fn door_open(state: &BlockState) -> Option<bool> {
    let family = state
        .name
        .strip_prefix("minecraft:")?
        .strip_suffix("_door")?;
    let wooden = native_core::wood::Planks::ALL.into_iter().any(|wood| {
        wood.id()
            .strip_prefix("minecraft:")
            .and_then(|id| id.strip_suffix("_planks"))
            == Some(family)
    });
    let metal = matches!(
        family,
        "iron"
            | "copper"
            | "exposed_copper"
            | "weathered_copper"
            | "oxidized_copper"
            | "waxed_copper"
            | "waxed_exposed_copper"
            | "waxed_weathered_copper"
            | "waxed_oxidized_copper"
    );
    if (!wooden && !metal)
        || state.properties.len() != 5
        || !matches!(
            state.properties.get("facing").map(String::as_str),
            Some("north" | "south" | "east" | "west")
        )
        || !matches!(
            state.properties.get("half").map(String::as_str),
            Some("lower" | "upper")
        )
        || !matches!(
            state.properties.get("hinge").map(String::as_str),
            Some("left" | "right")
        )
        || !matches!(
            state.properties.get("powered").map(String::as_str),
            Some("true" | "false")
        )
    {
        return None;
    }
    match state.properties.get("open").map(String::as_str) {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    }
}

/// Validate a built-in trapdoor and return its open and waterlogged flags.
pub fn trapdoor_state(state: &BlockState) -> Option<(bool, bool)> {
    let family = state
        .name
        .strip_prefix("minecraft:")?
        .strip_suffix("_trapdoor")?;
    let wooden = native_core::wood::Planks::ALL.into_iter().any(|wood| {
        wood.id()
            .strip_prefix("minecraft:")
            .and_then(|id| id.strip_suffix("_planks"))
            == Some(family)
    });
    let metal = matches!(
        family,
        "iron"
            | "copper"
            | "exposed_copper"
            | "weathered_copper"
            | "oxidized_copper"
            | "waxed_copper"
            | "waxed_exposed_copper"
            | "waxed_weathered_copper"
            | "waxed_oxidized_copper"
    );
    if (!wooden && !metal)
        || state.properties.len() != 5
        || !matches!(
            state.properties.get("facing").map(String::as_str),
            Some("north" | "south" | "east" | "west")
        )
        || !matches!(
            state.properties.get("half").map(String::as_str),
            Some("bottom" | "top")
        )
        || !matches!(
            state.properties.get("powered").map(String::as_str),
            Some("true" | "false")
        )
    {
        return None;
    }
    let boolean = |key: &str| match state.properties.get(key)?.as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    };
    Some((boolean("open")?, boolean("waterlogged")?))
}

fn trapdoor_shape(state: &BlockState) -> Option<Vec<Aabb>> {
    let (open, _) = trapdoor_state(state)?;
    let t = 3.0 / 16.0;
    let bounds = if !open {
        match state.properties.get("half")?.as_str() {
            "bottom" => cuboid(0.0, 0.0, 0.0, 1.0, t, 1.0),
            "top" => cuboid(0.0, 1.0 - t, 0.0, 1.0, 1.0, 1.0),
            _ => return None,
        }
    } else {
        match state.properties.get("facing")?.as_str() {
            "north" => cuboid(0.0, 0.0, 1.0 - t, 1.0, 1.0, 1.0),
            "south" => cuboid(0.0, 0.0, 0.0, 1.0, 1.0, t),
            "west" => cuboid(1.0 - t, 0.0, 0.0, 1.0, 1.0, 1.0),
            "east" => cuboid(0.0, 0.0, 0.0, t, 1.0, 1.0),
            _ => return None,
        }
    };
    Some(vec![bounds])
}

fn door_shape(state: &BlockState) -> Option<Vec<Aabb>> {
    let open = door_open(state)?;
    let facing = state.properties.get("facing")?.as_str();
    let hinge = state.properties.get("hinge")?.as_str();
    let side = match (facing, open, hinge) {
        ("north", false, _) => "south",
        ("south", false, _) => "north",
        ("west", false, _) => "east",
        ("east", false, _) => "west",
        ("north", true, "left") | ("south", true, "right") => "west",
        ("south", true, "left") | ("north", true, "right") => "east",
        ("west", true, "left") | ("east", true, "right") => "south",
        ("east", true, "left") | ("west", true, "right") => "north",
        _ => return None,
    };
    let t = 3.0 / 16.0;
    Some(vec![match side {
        "north" => cuboid(0.0, 0.0, 0.0, 1.0, 1.0, t),
        "south" => cuboid(0.0, 0.0, 1.0 - t, 1.0, 1.0, 1.0),
        "west" => cuboid(0.0, 0.0, 0.0, t, 1.0, 1.0),
        "east" => cuboid(1.0 - t, 0.0, 0.0, 1.0, 1.0, 1.0),
        _ => unreachable!(),
    }])
}

fn fence_gate_box(state: &BlockState, height: f32) -> Option<Aabb> {
    fence_gate_open(state)?;
    Some(match state.properties.get("facing")?.as_str() {
        "north" | "south" => cuboid(0.0, 0.0, 6.0 / 16.0, 1.0, height, 10.0 / 16.0),
        "east" | "west" => cuboid(6.0 / 16.0, 0.0, 0.0, 10.0 / 16.0, height, 1.0),
        _ => return None,
    })
}

/// Whether the supplied collision boxes cover an entire block boundary face.
/// Blocks may still override face sturdiness independently of their shape.
pub fn covers_full_face(boxes: &[Aabb], direction: IVec3) -> bool {
    merged_face_occludes(boxes, &[], direction)
}

/// Whether a union of local outline boxes is exactly the unit cube.
pub fn is_full_cube(boxes: &[Aabb]) -> bool {
    if boxes.is_empty() {
        return false;
    }
    let mut axes = [vec![0.0_f32, 1.0], vec![0.0, 1.0], vec![0.0, 1.0]];
    for b in boxes {
        for (axis, cuts) in axes.iter_mut().enumerate() {
            let (min, max) = (b.min[axis], b.max[axis]);
            if !min.is_finite() || !max.is_finite() || min < 0.0 || max > 1.0 || min >= max {
                return false;
            }
            cuts.extend([min, max]);
        }
    }
    for cuts in &mut axes {
        cuts.sort_by(f32::total_cmp);
        cuts.dedup();
    }
    for x in axes[0].windows(2) {
        for y in axes[1].windows(2) {
            for z in axes[2].windows(2) {
                let point = Vec3::new(
                    (x[0] + x[1]) / 2.0,
                    (y[0] + y[1]) / 2.0,
                    (z[0] + z[1]) / 2.0,
                );
                if !boxes
                    .iter()
                    .any(|b| point.cmpge(b.min).all() && point.cmplt(b.max).all())
                {
                    return false;
                }
            }
        }
    }
    true
}

#[cfg(test)]
#[test]
fn full_cube_checks_union_and_outside_volume() {
    assert!(is_full_cube(FULL));
    assert!(is_full_cube(&[BOTTOM_SLAB[0], TOP_SLAB[0]]));
    assert!(!is_full_cube(BOTTOM_SLAB));
    assert!(!is_full_cube(&[cuboid(-0.1, 0.0, 0.0, 1.0, 1.0, 1.0)]));
    assert!(!is_full_cube(EMPTY));
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
    if id == "snow" {
        let layers = snow_layers(state)?;
        return Some(direction == IVec3::NEG_Y || layers == 8);
    }
    if simple_carpet(state) {
        return Some(direction == IVec3::NEG_Y);
    }
    if matches!(id, "air" | "cave_air" | "void_air") {
        return Some(false);
    }
    if id == "soul_sand" {
        return state.properties.is_empty().then_some(true);
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
        || matches!(id, "lantern" | "soul_lantern")
        || candle_state(state).is_some()
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
    fn crop_shapes_follow_age_and_reject_invalid_states() {
        for (id, age, height) in [
            ("wheat", "7", 16.0),
            ("carrots", "7", 9.0),
            ("potatoes", "0", 2.0),
            ("beetroots", "3", 8.0),
        ] {
            let mut state = BlockState {
                name: format!("minecraft:{id}"),
                properties: [("age".into(), age.into())].into(),
            };
            assert_eq!(block_shape(&state), Some(Vec::new()));
            assert_eq!(
                block_outline(&state),
                Some(vec![pixel_box(0.0, 0.0, 0.0, 16.0, height, 16.0)])
            );
            state.properties.insert("age".into(), "8".into());
            assert!(block_shape(&state).is_none());
            assert!(block_outline(&state).is_none());
        }
    }

    #[test]
    fn iron_chain_collision_follows_axis_and_rejects_invalid_states() {
        for (axis, min, max) in [
            ("x", [0.0, 6.5, 6.5], [16.0, 9.5, 9.5]),
            ("y", [6.5, 0.0, 6.5], [9.5, 16.0, 9.5]),
            ("z", [6.5, 6.5, 0.0], [9.5, 9.5, 16.0]),
        ] {
            for wet in ["false", "true"] {
                let mut state = BlockState {
                    name: "minecraft:iron_chain".into(),
                    properties: [
                        ("axis".into(), axis.into()),
                        ("waterlogged".into(), wet.into()),
                    ]
                    .into(),
                };
                let expected = vec![pixel_box(min[0], min[1], min[2], max[0], max[1], max[2])];
                assert_eq!(block_shape(&state), Some(expected));
                state
                    .properties
                    .insert("waterlogged".into(), "invalid".into());
                assert!(block_shape(&state).is_none());
            }
        }
    }

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

    #[test]
    fn campfire_collision_and_outline_are_seven_pixels_high() {
        for name in ["minecraft:campfire", "minecraft:soul_campfire"] {
            let state = BlockState {
                name: name.into(),
                properties: [
                    ("facing".into(), "north".into()),
                    ("lit".into(), "true".into()),
                    ("signal_fire".into(), "false".into()),
                    ("waterlogged".into(), "false".into()),
                ]
                .into(),
            };
            let shape = block_shape(&state).unwrap();
            assert_eq!(shape.len(), 1);
            assert_eq!(shape[0].min, glam::Vec3::ZERO);
            assert_eq!(shape[0].max, glam::Vec3::new(1.0, 7.0 / 16.0, 1.0));
            assert_eq!(block_outline(&state), Some(shape));
        }
    }

    #[test]
    fn lantern_shapes_validate_state_and_follow_hanging_height() {
        for name in ["minecraft:lantern", "minecraft:soul_lantern"] {
            for hanging in [false, true] {
                for waterlogged in [false, true] {
                    let mut state = BlockState {
                        name: name.into(),
                        properties: [
                            ("hanging".into(), hanging.to_string()),
                            ("waterlogged".into(), waterlogged.to_string()),
                        ]
                        .into(),
                    };
                    let dy = if hanging { 1.0 / 16.0 } else { 0.0 };
                    let expected = vec![
                        cuboid(
                            5.0 / 16.0,
                            dy,
                            5.0 / 16.0,
                            11.0 / 16.0,
                            7.0 / 16.0 + dy,
                            11.0 / 16.0,
                        ),
                        cuboid(
                            6.0 / 16.0,
                            7.0 / 16.0 + dy,
                            6.0 / 16.0,
                            10.0 / 16.0,
                            9.0 / 16.0 + dy,
                            10.0 / 16.0,
                        ),
                    ];
                    assert_eq!(block_shape(&state), Some(expected.clone()));
                    assert_eq!(block_outline(&state), Some(expected.clone()));
                    assert_eq!(spawn_collision_shape(&state), Some(expected));
                    for direction in [
                        IVec3::X,
                        IVec3::NEG_X,
                        IVec3::Y,
                        IVec3::NEG_Y,
                        IVec3::Z,
                        IVec3::NEG_Z,
                    ] {
                        assert_eq!(button_support_face(&state, direction), Some(false));
                    }
                    state
                        .properties
                        .insert("waterlogged".into(), "invalid".into());
                    assert!(block_shape(&state).is_none());
                    assert!(block_outline(&state).is_none());
                }
            }
        }
    }

    #[test]
    fn cauldrons_have_hollow_interiors_and_validate_levels() {
        for (name, levels) in [
            ("minecraft:cauldron", vec![None]),
            ("minecraft:lava_cauldron", vec![None]),
            (
                "minecraft:water_cauldron",
                vec![Some("1"), Some("2"), Some("3")],
            ),
            (
                "minecraft:powder_snow_cauldron",
                vec![Some("1"), Some("2"), Some("3")],
            ),
        ] {
            for level in levels {
                let mut state = BlockState {
                    name: name.into(),
                    properties: Default::default(),
                };
                if let Some(level) = level {
                    state.properties.insert("level".into(), level.into());
                }
                let shape = block_shape(&state).unwrap();
                assert_eq!(block_outline(&state), Some(shape.clone()));
                let contains = |point: Vec3| {
                    shape
                        .iter()
                        .any(|b| point.cmpgt(b.min).all() && point.cmplt(b.max).all())
                };
                assert!(!contains(Vec3::new(0.5, 0.5, 0.5)));
                assert!(contains(Vec3::new(0.5, 3.5 / 16.0, 0.5)));
                assert!(!contains(Vec3::new(0.5, 1.0 / 16.0, 0.5)));
                state.properties.insert("unexpected".into(), "true".into());
                assert!(block_shape(&state).is_none());
            }
        }
    }

    #[test]
    fn anvils_rotate_the_same_shape_across_damage_states() {
        for name in [
            "minecraft:anvil",
            "minecraft:chipped_anvil",
            "minecraft:damaged_anvil",
        ] {
            for facing in ["north", "south", "east", "west"] {
                let mut state = BlockState {
                    name: name.into(),
                    properties: [("facing".into(), facing.into())].into(),
                };
                let expected = if matches!(facing, "north" | "south") {
                    &ANVIL_NORTH_SOUTH
                } else {
                    &ANVIL_EAST_WEST
                };
                assert_eq!(block_shape(&state), Some(expected.to_vec()));
                assert_eq!(block_outline(&state), Some(expected.to_vec()));
                assert_eq!(spawn_collision_shape(&state), Some(expected.to_vec()));
                state.properties.insert("facing".into(), "up".into());
                assert!(block_shape(&state).is_none());
            }
        }
    }

    #[test]
    fn sculk_sensor_shapes_validate_all_properties() {
        for name in [
            "minecraft:sculk_sensor",
            "minecraft:calibrated_sculk_sensor",
        ] {
            for phase in ["inactive", "active", "cooldown"] {
                for power in 0..=15 {
                    for wet in ["true", "false"] {
                        for facing in ["north", "south", "east", "west"] {
                            let mut state = BlockState {
                                name: name.into(),
                                properties: [
                                    ("sculk_sensor_phase".into(), phase.into()),
                                    ("power".into(), power.to_string()),
                                    ("waterlogged".into(), wet.into()),
                                ]
                                .into(),
                            };
                            if name.contains("calibrated") {
                                state.properties.insert("facing".into(), facing.into());
                            }
                            assert_eq!(block_shape(&state), Some(BOTTOM_SLAB.to_vec()));
                            assert_eq!(block_outline(&state), Some(BOTTOM_SLAB.to_vec()));
                            state.properties.insert("power".into(), "016".into());
                            assert_eq!(block_shape(&state), None);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn snow_shapes_reject_invalid_layer_properties() {
        for value in ["0", "9", "01", "-1", "x"] {
            let state = BlockState {
                name: "minecraft:snow".into(),
                properties: [("layers".into(), value.into())].into(),
            };
            assert!(block_shape(&state).is_none());
            assert!(block_outline(&state).is_none());
            assert!(button_support_face(&state, IVec3::Y).is_none());
            assert!(button_support_face(&state, IVec3::NEG_Y).is_none());
        }
        let state = BlockState {
            name: "minecraft:snow".into(),
            properties: [
                ("layers".into(), "1".into()),
                ("extra".into(), "true".into()),
            ]
            .into(),
        };
        assert!(block_shape(&state).is_none());
    }

    #[test]
    fn frosted_ice_supports_valid_ages_and_rejects_unknown_states() {
        for age in ["0", "1", "2", "3"] {
            let state = BlockState {
                name: "minecraft:frosted_ice".into(),
                properties: [("age".into(), age.into())].into(),
            };
            assert_eq!(block_shape(&state), Some(FULL.to_vec()));
            assert_eq!(block_outline(&state), Some(FULL.to_vec()));
            assert!(button_support_face(&state, IVec3::Y).unwrap());
        }
        for age in ["-1", "4", "00", "bad"] {
            let state = BlockState {
                name: "minecraft:frosted_ice".into(),
                properties: [("age".into(), age.into())].into(),
            };
            assert!(block_shape(&state).is_none());
        }
        let extra = BlockState {
            name: "minecraft:frosted_ice".into(),
            properties: [("age".into(), "0".into()), ("extra".into(), "true".into())].into(),
        };
        assert!(block_shape(&extra).is_none());
    }
}

pub(crate) fn valid_leaf_litter(state: &BlockState) -> bool {
    state.name == "minecraft:leaf_litter"
        && state.properties.len() == 2
        && matches!(
            state.properties.get("facing").map(String::as_str),
            Some("north" | "south" | "west" | "east")
        )
        && matches!(
            state.properties.get("segment_amount").map(String::as_str),
            Some("1" | "2" | "3" | "4")
        )
}

fn crop_outline_height(state: &BlockState) -> Option<f32> {
    let (max, step) = match state.name.as_str() {
        "minecraft:wheat" => (7, 2),
        "minecraft:carrots" | "minecraft:potatoes" => (7, 1),
        "minecraft:beetroots" => (3, 2),
        _ => return None,
    };
    if state.properties.len() != 1 {
        return None;
    }
    let value = state.properties.get("age")?;
    let age: u8 = value.parse().ok()?;
    if age > max || age.to_string() != *value {
        return None;
    }
    Some(f32::from(2 + step * age) / 16.0)
}

/// `None` means that this state's physical shape is not implemented.
pub fn block_shape(state: &BlockState) -> Option<Vec<Aabb>> {
    if crop_outline_height(state).is_some() {
        return Some(EMPTY.to_vec());
    }
    if simple_carpet(state) {
        return Some(vec![pixel_box(0.0, 0.0, 0.0, 16.0, 1.0, 16.0)]);
    }
    if let Some((facing, _)) = ladder_state(state) {
        return Some(vec![match facing {
            "north" => pixel_box(0.0, 0.0, 13.0, 16.0, 16.0, 16.0),
            "south" => pixel_box(0.0, 0.0, 0.0, 16.0, 16.0, 3.0),
            "west" => pixel_box(13.0, 0.0, 0.0, 16.0, 16.0, 16.0),
            "east" => pixel_box(0.0, 0.0, 0.0, 3.0, 16.0, 16.0),
            _ => unreachable!("validated ladder facing"),
        }]);
    }
    if let Some((count, _, _)) = candle_state(state) {
        let bounds = match count {
            1 => (7.0, 7.0, 9.0, 9.0),
            2 => (5.0, 6.0, 11.0, 9.0),
            3 => (5.0, 6.0, 10.0, 11.0),
            4 => (5.0, 5.0, 11.0, 10.0),
            _ => unreachable!("validated candle count"),
        };
        return Some(vec![pixel_box(
            bounds.0, 0.0, bounds.1, bounds.2, 6.0, bounds.3,
        )]);
    }
    let id = state.name.strip_prefix("minecraft:")?;
    if id == "iron_chain" {
        if state.properties.len() != 2
            || !matches!(
                state.properties.get("waterlogged").map(String::as_str),
                Some("true" | "false")
            )
        {
            return None;
        }
        return match state.properties.get("axis").map(String::as_str)? {
            "x" => Some(vec![pixel_box(0.0, 6.5, 6.5, 16.0, 9.5, 9.5)]),
            "y" => Some(vec![pixel_box(6.5, 0.0, 6.5, 9.5, 16.0, 9.5)]),
            "z" => Some(vec![pixel_box(6.5, 6.5, 0.0, 9.5, 9.5, 16.0)]),
            _ => None,
        };
    }
    if id == "jigsaw" {
        return jigsaw_state(state).then(|| FULL.to_vec());
    }
    if id == "barrier" {
        return barrier_waterlogged(state).map(|_| FULL.to_vec());
    }
    if id.ends_with("_wall") {
        return Some(wall_boxes(crate::wall::Wall::from_state(state)?, true));
    }
    if id == "leaf_litter" {
        return valid_leaf_litter(state).then(|| EMPTY.to_vec());
    }
    if matches!(id, "anvil" | "chipped_anvil" | "damaged_anvil") {
        if state.properties.len() != 1 {
            return None;
        }
        return match state.properties.get("facing").map(String::as_str)? {
            "north" | "south" => Some(ANVIL_NORTH_SOUTH.to_vec()),
            "east" | "west" => Some(ANVIL_EAST_WEST.to_vec()),
            _ => None,
        };
    }
    if matches!(
        id,
        "cauldron" | "lava_cauldron" | "water_cauldron" | "powder_snow_cauldron"
    ) {
        let valid = if matches!(id, "water_cauldron" | "powder_snow_cauldron") {
            state.properties.len() == 1
                && matches!(
                    state.properties.get("level").map(String::as_str),
                    Some("1" | "2" | "3")
                )
        } else {
            state.properties.is_empty()
        };
        return valid.then(cauldron_shape);
    }
    if id == "brewing_stand" {
        let valid = state.properties.len() == 3
            && ["has_bottle_0", "has_bottle_1", "has_bottle_2"]
                .iter()
                .all(|name| {
                    matches!(
                        state.properties.get(*name).map(String::as_str),
                        Some("true" | "false")
                    )
                });
        return valid.then(|| {
            vec![
                pixel_box(1.0, 0.0, 1.0, 15.0, 2.0, 15.0),
                pixel_box(7.0, 2.0, 7.0, 9.0, 14.0, 9.0),
            ]
        });
    }
    if id == "stonecutter" {
        let valid = state.properties.len() == 1
            && matches!(
                state.properties.get("facing").map(String::as_str),
                Some("north" | "south" | "east" | "west")
            );
        return valid.then(|| vec![pixel_box(0.0, 0.0, 0.0, 16.0, 9.0, 16.0)]);
    }
    if matches!(id, "sculk_sensor" | "calibrated_sculk_sensor") {
        let calibrated = id == "calibrated_sculk_sensor";
        let power = state.properties.get("power")?;
        let power_value = power.parse::<u8>().ok()?;
        let valid = state.properties.len() == if calibrated { 4 } else { 3 }
            && power_value <= 15
            && power_value.to_string() == *power
            && matches!(
                state
                    .properties
                    .get("sculk_sensor_phase")
                    .map(String::as_str),
                Some("inactive" | "active" | "cooldown")
            )
            && matches!(
                state.properties.get("waterlogged").map(String::as_str),
                Some("true" | "false")
            )
            && (!calibrated
                || matches!(
                    state.properties.get("facing").map(String::as_str),
                    Some("north" | "south" | "west" | "east")
                ));
        return valid.then(|| BOTTOM_SLAB.to_vec());
    }
    if id == "frosted_ice" {
        return (state.properties.len() == 1
            && matches!(
                state.properties.get("age").map(String::as_str),
                Some("0" | "1" | "2" | "3")
            ))
        .then(|| FULL.to_vec());
    }
    if banner_item_id(state).is_some() {
        return Some(EMPTY.to_vec());
    }
    if id == "snow" {
        let layers = snow_layers(state)?;
        return Some(if layers == 1 {
            Vec::new()
        } else {
            vec![cuboid(0.0, 0.0, 0.0, 1.0, f32::from(layers - 1) / 8.0, 1.0)]
        });
    }
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
    if matches!(id, "campfire" | "soul_campfire") {
        if state.properties.len() != 4
            || !matches!(
                state.properties.get("facing").map(String::as_str),
                Some("north" | "south" | "east" | "west")
            )
            || ["lit", "signal_fire", "waterlogged"].iter().any(|name| {
                !matches!(
                    state.properties.get(*name).map(String::as_str),
                    Some("true" | "false")
                )
            })
        {
            return None;
        }
        return Some(vec![cuboid(0.0, 0.0, 0.0, 1.0, 7.0 / 16.0, 1.0)]);
    }
    if matches!(id, "lantern" | "soul_lantern") {
        if state.properties.len() != 2
            || !matches!(
                state.properties.get("waterlogged").map(String::as_str),
                Some("true" | "false")
            )
        {
            return None;
        }
        let dy = match state.properties.get("hanging").map(String::as_str)? {
            "true" => 1.0 / 16.0,
            "false" => 0.0,
            _ => return None,
        };
        return Some(vec![
            cuboid(
                5.0 / 16.0,
                dy,
                5.0 / 16.0,
                11.0 / 16.0,
                7.0 / 16.0 + dy,
                11.0 / 16.0,
            ),
            cuboid(
                6.0 / 16.0,
                7.0 / 16.0 + dy,
                6.0 / 16.0,
                10.0 / 16.0,
                9.0 / 16.0 + dy,
                10.0 / 16.0,
            ),
        ]);
    }
    if id == "honey_block" {
        return Some(HONEY.to_vec());
    }
    if id == "soul_sand" {
        return state
            .properties
            .is_empty()
            .then(|| vec![cuboid(0.0, 0.0, 0.0, 1.0, 14.0 / 16.0, 1.0)]);
    }
    if matches!(id, "lodestone" | "magma_block") {
        return state.properties.is_empty().then(|| FULL.to_vec());
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
    if id.ends_with("_fence_gate") {
        return if fence_gate_open(state)? {
            Some(EMPTY.to_vec())
        } else {
            Some(vec![fence_gate_box(state, 1.5)?])
        };
    }
    if id.ends_with("_door") {
        return door_shape(state);
    }
    if id.ends_with("_trapdoor") {
        return trapdoor_shape(state);
    }
    if id.ends_with("_fence") {
        return connected(state, 6.0 / 16.0, 10.0 / 16.0, 1.5);
    }
    if pane_waterlogged(state).is_some() {
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
            | "terracotta"
            | "moss_block"
            | "nether_bricks"
            | "blackstone"
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
    if let Some(height) = crop_outline_height(state) {
        return Some(vec![cuboid(0.0, 0.0, 0.0, 1.0, height, 1.0)]);
    }
    let id = state.name.strip_prefix("minecraft:")?;
    if id.ends_with("_wall") {
        return Some(wall_boxes(crate::wall::Wall::from_state(state)?, false));
    }
    if id.ends_with("_fence_gate") {
        let height = if state.properties.get("in_wall").map(String::as_str) == Some("true") {
            13.0 / 16.0
        } else {
            1.0
        };
        return Some(vec![fence_gate_box(state, height)?]);
    }
    if banner_item_id(state).is_some() {
        let s = 1.0 / 16.0;
        return Some(vec![if id.contains("_wall_banner") {
            match state.properties.get("facing")?.as_str() {
                "north" => cuboid(0.0, 0.0, 14.0 * s, 1.0, 12.5 * s, 1.0),
                "south" => cuboid(0.0, 0.0, 0.0, 1.0, 12.5 * s, 2.0 * s),
                "west" => cuboid(14.0 * s, 0.0, 0.0, 1.0, 12.5 * s, 1.0),
                "east" => cuboid(0.0, 0.0, 0.0, 2.0 * s, 12.5 * s, 1.0),
                _ => return None,
            }
        } else {
            cuboid(4.0 * s, 0.0, 4.0 * s, 12.0 * s, 1.0, 12.0 * s)
        }]);
    }
    if id == "snow" {
        return snow_outline(state);
    }
    if id == "lever" {
        return lever_outline(state);
    }
    if id.ends_with("_button") {
        return button_outline(state);
    }
    if id == "honey_block" || (id == "soul_sand" && state.properties.is_empty()) {
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
    if pane_waterlogged(state).is_some() {
        return connected(state, 7.0 / 16.0, 9.0 / 16.0, 1.0);
    }
    let shape = block_shape(state)?;
    if shape.is_empty() { None } else { Some(shape) }
}

/// Resolve both banner block forms to their item, rejecting malformed states.
pub fn banner_item_id(state: &BlockState) -> Option<String> {
    let name = state.name.strip_prefix("minecraft:")?;
    let (color, wall) = if let Some(color) = name.strip_suffix("_wall_banner") {
        (color, true)
    } else {
        (name.strip_suffix("_banner")?, false)
    };
    if !matches!(
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
    ) || state.properties.len() != 1
    {
        return None;
    }
    if wall {
        if !matches!(
            state.properties.get("facing").map(String::as_str),
            Some("north" | "south" | "west" | "east")
        ) {
            return None;
        }
    } else {
        let rotation = state.properties.get("rotation")?;
        if !matches!(
            rotation.as_str(),
            "0" | "1"
                | "2"
                | "3"
                | "4"
                | "5"
                | "6"
                | "7"
                | "8"
                | "9"
                | "10"
                | "11"
                | "12"
                | "13"
                | "14"
                | "15"
        ) {
            return None;
        }
    }
    Some(format!("minecraft:{color}_banner"))
}

/// The eight vanilla snow-layer states, rejecting malformed or extra properties.
pub fn snow_layers(state: &BlockState) -> Option<u8> {
    if state.name != "minecraft:snow" || state.properties.len() != 1 {
        return None;
    }
    match state.properties.get("layers")?.as_str() {
        "1" => Some(1),
        "2" => Some(2),
        "3" => Some(3),
        "4" => Some(4),
        "5" => Some(5),
        "6" => Some(6),
        "7" => Some(7),
        "8" => Some(8),
        _ => None,
    }
}

fn snow_outline(state: &BlockState) -> Option<Vec<Aabb>> {
    Some(vec![cuboid(
        0.0,
        0.0,
        0.0,
        1.0,
        f32::from(snow_layers(state)?) / 8.0,
        1.0,
    )])
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
    let edge = bed_leg_end(state)?;
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

const fn pixel_box(x0: f32, y0: f32, z0: f32, x1: f32, y1: f32, z1: f32) -> Aabb {
    cuboid(
        x0 / 16.0,
        y0 / 16.0,
        z0 / 16.0,
        x1 / 16.0,
        y1 / 16.0,
        z1 / 16.0,
    )
}

/// Java's cauldron has corner feet, raised floor and narrow outer walls.
pub(crate) const CAULDRON_SHAPE: [Aabb; 15] = [
    pixel_box(0.0, 0.0, 0.0, 2.0, 16.0, 4.0),
    pixel_box(0.0, 0.0, 12.0, 2.0, 16.0, 16.0),
    pixel_box(2.0, 0.0, 0.0, 4.0, 16.0, 2.0),
    pixel_box(2.0, 0.0, 14.0, 4.0, 16.0, 16.0),
    pixel_box(12.0, 0.0, 0.0, 16.0, 16.0, 2.0),
    pixel_box(12.0, 0.0, 14.0, 16.0, 16.0, 16.0),
    pixel_box(14.0, 0.0, 2.0, 16.0, 16.0, 4.0),
    pixel_box(14.0, 0.0, 12.0, 16.0, 16.0, 14.0),
    pixel_box(0.0, 3.0, 4.0, 16.0, 4.0, 12.0),
    pixel_box(2.0, 3.0, 2.0, 14.0, 4.0, 4.0),
    pixel_box(2.0, 3.0, 12.0, 14.0, 4.0, 14.0),
    pixel_box(4.0, 3.0, 0.0, 12.0, 16.0, 2.0),
    pixel_box(4.0, 3.0, 14.0, 12.0, 16.0, 16.0),
    pixel_box(0.0, 4.0, 4.0, 2.0, 16.0, 12.0),
    pixel_box(14.0, 4.0, 4.0, 16.0, 16.0, 12.0),
];

pub(crate) const ANVIL_NORTH_SOUTH: [Aabb; 7] = [
    pixel_box(2.0, 0.0, 2.0, 14.0, 4.0, 14.0),
    pixel_box(4.0, 4.0, 3.0, 12.0, 5.0, 13.0),
    pixel_box(6.0, 5.0, 4.0, 10.0, 16.0, 12.0),
    pixel_box(3.0, 10.0, 0.0, 6.0, 16.0, 16.0),
    pixel_box(6.0, 10.0, 0.0, 13.0, 16.0, 4.0),
    pixel_box(6.0, 10.0, 12.0, 13.0, 16.0, 16.0),
    pixel_box(10.0, 10.0, 4.0, 13.0, 16.0, 12.0),
];

pub(crate) const ANVIL_EAST_WEST: [Aabb; 7] = [
    pixel_box(2.0, 0.0, 2.0, 14.0, 4.0, 14.0),
    pixel_box(3.0, 4.0, 4.0, 13.0, 5.0, 12.0),
    pixel_box(4.0, 5.0, 6.0, 12.0, 16.0, 10.0),
    pixel_box(0.0, 10.0, 3.0, 4.0, 16.0, 13.0),
    pixel_box(4.0, 10.0, 3.0, 16.0, 16.0, 6.0),
    pixel_box(4.0, 10.0, 10.0, 16.0, 16.0, 13.0),
    pixel_box(12.0, 10.0, 6.0, 16.0, 16.0, 10.0),
];

fn cauldron_shape() -> Vec<Aabb> {
    CAULDRON_SHAPE.to_vec()
}

const fn cuboid(x0: f32, y0: f32, z0: f32, x1: f32, y1: f32, z1: f32) -> Aabb {
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

fn wall_boxes(wall: crate::wall::Wall, collision: bool) -> Vec<Aabb> {
    use crate::wall::Side;
    let mut boxes = Vec::with_capacity(5);
    let post_height = if collision { 24.0 } else { 16.0 };
    if wall.shape.up {
        boxes.push(pixel_box(4.0, 0.0, 4.0, 12.0, post_height, 12.0));
    }
    for (index, side) in wall.shape.sides.into_iter().enumerate() {
        let height = match (side, collision) {
            (Side::None, _) => continue,
            (_, true) => 24.0,
            (Side::Low, false) => 14.0,
            (Side::Tall, false) => 16.0,
        };
        boxes.push(match index {
            0 => pixel_box(5.0, 0.0, 0.0, 11.0, height, 11.0),
            1 => pixel_box(5.0, 0.0, 5.0, 16.0, height, 11.0),
            2 => pixel_box(5.0, 0.0, 5.0, 11.0, height, 16.0),
            3 => pixel_box(0.0, 0.0, 5.0, 11.0, height, 11.0),
            _ => unreachable!(),
        });
    }
    canonical_boxes(&boxes)
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
    fn ladders_validate_facing_and_use_the_wall_side_shape() {
        for (facing, expected) in [
            ("north", pixel_box(0.0, 0.0, 13.0, 16.0, 16.0, 16.0)),
            ("south", pixel_box(0.0, 0.0, 0.0, 16.0, 16.0, 3.0)),
            ("west", pixel_box(13.0, 0.0, 0.0, 16.0, 16.0, 16.0)),
            ("east", pixel_box(0.0, 0.0, 0.0, 3.0, 16.0, 16.0)),
        ] {
            for waterlogged in [false, true] {
                let mut state = BlockState {
                    name: "minecraft:ladder".into(),
                    properties: BTreeMap::from([
                        ("facing".into(), facing.into()),
                        ("waterlogged".into(), waterlogged.to_string()),
                    ]),
                };
                assert_eq!(ladder_state(&state), Some((facing, waterlogged)));
                assert_eq!(block_shape(&state), Some(vec![expected]));
                assert_eq!(block_outline(&state), block_shape(&state));
                state.properties.insert("facing".into(), "up".into());
                assert!(ladder_state(&state).is_none());
                assert!(block_shape(&state).is_none());
            }
        }
    }

    #[test]
    fn vanilla_carpets_have_thin_collision_and_validate_identity() {
        for name in [
            "minecraft:blue_carpet",
            "minecraft:light_blue_carpet",
            "minecraft:moss_carpet",
        ] {
            let state = BlockState {
                name: name.into(),
                properties: BTreeMap::new(),
            };
            let expected = Some(vec![pixel_box(0.0, 0.0, 0.0, 16.0, 1.0, 16.0)]);
            assert_eq!(block_shape(&state), expected);
            assert_eq!(block_outline(&state), expected);
            assert_eq!(button_support_face(&state, IVec3::NEG_Y), Some(true));
            assert_eq!(button_support_face(&state, IVec3::Y), Some(false));
        }
        for name in [
            "minecraft:pale_moss_carpet",
            "minecraft:invented_carpet",
            "other:blue_carpet",
        ] {
            assert!(!simple_carpet(&BlockState {
                name: name.into(),
                properties: BTreeMap::new()
            }));
        }
        assert!(!simple_carpet(&BlockState {
            name: "minecraft:blue_carpet".into(),
            properties: BTreeMap::from([("invalid".into(), "true".into())]),
        }));
    }

    #[test]
    fn candles_validate_color_count_and_collision_shape() {
        let mut state = BlockState {
            name: "minecraft:light_blue_candle".into(),
            properties: BTreeMap::from([
                ("candles".into(), "1".into()),
                ("lit".into(), "false".into()),
                ("waterlogged".into(), "true".into()),
            ]),
        };
        for (count, bounds) in [
            ("1", (7.0, 7.0, 9.0, 9.0)),
            ("2", (5.0, 6.0, 11.0, 9.0)),
            ("3", (5.0, 6.0, 10.0, 11.0)),
            ("4", (5.0, 5.0, 11.0, 10.0)),
        ] {
            state.properties.insert("candles".into(), count.into());
            assert_eq!(
                block_shape(&state),
                Some(vec![pixel_box(
                    bounds.0, 0.0, bounds.1, bounds.2, 6.0, bounds.3
                )])
            );
            assert_eq!(block_outline(&state), block_shape(&state));
            assert_eq!(button_support_face(&state, IVec3::Y), Some(false));
        }
        state.properties.insert("candles".into(), "5".into());
        assert!(block_shape(&state).is_none());
        state.properties.insert("candles".into(), "1".into());
        state.name = "minecraft:invented_candle".into();
        assert!(block_shape(&state).is_none());
    }

    #[test]
    fn terracotta_classifier_accepts_only_stateless_vanilla_colors() {
        let mut state = BlockState {
            name: "minecraft:white_terracotta".into(),
            properties: BTreeMap::new(),
        };
        assert!(full_terracotta(&state));
        state.name = "minecraft:terracotta".into();
        assert!(full_terracotta(&state));
        state.name = "minecraft:white_glazed_terracotta".into();
        assert!(!full_terracotta(&state));
        state.name = "minecraft:invented_terracotta".into();
        assert!(!full_terracotta(&state));
        state.name = "minecraft:white_terracotta".into();
        state.properties.insert("extra".into(), "true".into());
        assert!(!full_terracotta(&state));
    }

    #[test]
    fn bed_shape_validates_states_and_places_legs_on_the_outer_end() {
        let mut state = BlockState {
            name: "minecraft:red_bed".into(),
            properties: BTreeMap::from([
                ("facing".into(), "east".into()),
                ("occupied".into(), "false".into()),
                ("part".into(), "head".into()),
            ]),
        };
        assert_eq!(bed_leg_end(&state), Some("east"));
        let shape = block_shape(&state).unwrap();
        assert_eq!(shape.len(), 3);
        assert_eq!(shape[1].min.x, 13.0 / 16.0);
        state.properties.insert("part".into(), "foot".into());
        assert_eq!(bed_leg_end(&state), Some("west"));
        assert_eq!(block_shape(&state).unwrap()[1].min.x, 0.0);
        state.properties.insert("occupied".into(), "maybe".into());
        assert!(block_shape(&state).is_none());
        state.properties.insert("occupied".into(), "false".into());
        state.name = "minecraft:invented_bed".into();
        assert!(block_shape(&state).is_none());
    }

    #[test]
    fn trapdoor_collision_rejects_unknown_and_incomplete_states() {
        let mut state = BlockState {
            name: "minecraft:dark_oak_trapdoor".into(),
            properties: BTreeMap::from([
                ("facing".into(), "north".into()),
                ("half".into(), "bottom".into()),
                ("open".into(), "false".into()),
                ("powered".into(), "false".into()),
                ("waterlogged".into(), "true".into()),
            ]),
        };
        assert_eq!(trapdoor_state(&state), Some((false, true)));
        assert_eq!(block_shape(&state).unwrap()[0].max.y, 3.0 / 16.0);
        state.properties.insert("open".into(), "true".into());
        assert_eq!(block_shape(&state).unwrap()[0].min.z, 13.0 / 16.0);
        state.properties.remove("powered");
        assert_eq!(block_shape(&state), None);
        state.properties.insert("powered".into(), "false".into());
        state.name = "mod:dark_oak_trapdoor".into();
        assert_eq!(block_shape(&state), None);
    }

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
