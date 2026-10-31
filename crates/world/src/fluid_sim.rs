//! Scheduled fluid spread for air, full blocks, liquids, slabs and stairs.
//!
//! Other blocks are barriers until their fluid occupancy and side shapes are
//! available. Wet copper grates block flow but count in source checks. The
//! caller schedules resulting changes and preserves world data.

use glam::IVec3;

use crate::fluid::{FluidKind, FluidState};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cell {
    Air,
    Solid,
    Fluid(FluidState),
    /// Source water inside a copper grate; its block must not be replaced.
    GrateWater(FluidState),
    /// A half slab, optionally holding source water.
    Slab {
        top: bool,
        waterlogged: bool,
    },
    /// A staircase's occupied 2×2×2 cells and water occupancy.
    Stairs {
        voxels: u8,
        waterlogged: bool,
    },
    /// Unloaded or unsupported data: never replace or traverse it.
    Unknown,
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub water_source_conversion: bool,
    pub lava_source_conversion: bool,
    pub fast_lava: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            water_source_conversion: true,
            lava_source_conversion: false,
            fast_lava: false,
        }
    }
}

impl Settings {
    pub fn delay(self, kind: FluidKind) -> u64 {
        match kind {
            FluidKind::Water => 5,
            FluidKind::Lava if self.fast_lava => 10,
            FluidKind::Lava => 30,
        }
    }

    fn drop_off(self, kind: FluidKind) -> u8 {
        if kind == FluidKind::Lava && !self.fast_lava {
            2
        } else {
            1
        }
    }

    fn slope_distance(self, kind: FluidKind) -> u8 {
        if kind == FluidKind::Lava && !self.fast_lava {
            2
        } else {
            4
        }
    }

    fn converts_source(self, kind: FluidKind) -> bool {
        match kind {
            FluidKind::Water => self.water_source_conversion,
            FluidKind::Lava => self.lava_source_conversion,
        }
    }
}

/// Delay used when a flowing fluid is recalculated. Java only samples the
/// world RNG when non-falling lava rises and the height above is not masked by
/// another lava block. `sample_four` must return an integer in `0..4`.
pub fn spread_delay(
    settings: Settings,
    kind: FluidKind,
    old: Option<FluidState>,
    next: Option<FluidState>,
    same_fluid_above: bool,
    sample_four: impl FnOnce() -> u32,
) -> u64 {
    let delay = settings.delay(kind);
    if kind == FluidKind::Lava
        && !same_fluid_above
        && let (Some(old), Some(next)) = (old, next)
        && old.kind == kind
        && next.kind == kind
        && !old.falling
        && !next.falling
        && next.own_height() > old.own_height()
        && sample_four() != 0
    {
        delay * 4
    } else {
        delay
    }
}

/// The block produced at a position during one fluid tick.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChangedBlock {
    Air,
    Fluid(FluidState),
    Solid(Solidified),
    WaterloggedSlab { top: bool },
    WaterloggedStairs { voxels: u8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Solidified {
    Stone,
    Obsidian,
    Cobblestone,
    Basalt,
}

/// Relevant properties of a block neighboring lava.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LavaNeighbor {
    Other,
    Water,
    BlueIce,
    SoulSoil,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Change {
    pub position: IVec3,
    pub block: ChangedBlock,
}

// EnumMap<Direction, ...> applies side changes in this order.
const SIDES: [IVec3; 4] = [IVec3::NEG_Z, IVec3::Z, IVec3::NEG_X, IVec3::X];

/// Lava reacts to water above or on a side, or to blue ice beside soul soil.
/// The order matches 26.3's LiquidBlock.shouldSpreadLiquid.
pub fn lava_reaction(
    world: &impl Fn(IVec3) -> LavaNeighbor,
    position: IVec3,
    lava: FluidState,
) -> Option<Solidified> {
    if lava.kind != FluidKind::Lava {
        return None;
    }
    let below_is_soul_soil =
        offset(position, IVec3::NEG_Y).is_some_and(|pos| world(pos) == LavaNeighbor::SoulSoil);
    for direction in [IVec3::Y, IVec3::NEG_Z, IVec3::Z, IVec3::NEG_X, IVec3::X] {
        let Some(pos) = offset(position, direction) else {
            continue;
        };
        match world(pos) {
            LavaNeighbor::Water => {
                return Some(if lava.source {
                    Solidified::Obsidian
                } else {
                    Solidified::Cobblestone
                });
            }
            LavaNeighbor::BlueIce if below_is_soul_soil => return Some(Solidified::Basalt),
            _ => {}
        }
    }
    None
}

fn offset(pos: IVec3, direction: IVec3) -> Option<IVec3> {
    Some(IVec3::new(
        pos.x.checked_add(direction.x)?,
        pos.y.checked_add(direction.y)?,
        pos.z.checked_add(direction.z)?,
    ))
}

fn at(world: &impl Fn(IVec3) -> Cell, pos: Option<IVec3>) -> Cell {
    pos.map(world).unwrap_or(Cell::Unknown)
}

fn source(cell: Cell, kind: FluidKind) -> bool {
    matches!(cell, Cell::Fluid(state) | Cell::GrateWater(state) if state.kind == kind && state.source)
        || matches!(cell, Cell::Slab { waterlogged: true, .. } if kind == FluidKind::Water)
        || matches!(cell, Cell::Stairs { waterlogged: true, .. } if kind == FluidKind::Water)
}

fn fluid(cell: Cell) -> Option<FluidState> {
    match cell {
        Cell::Fluid(state) | Cell::GrateWater(state) => Some(state),
        Cell::Slab {
            waterlogged: true, ..
        }
        | Cell::Stairs {
            waterlogged: true, ..
        } => Some(flowing(FluidKind::Water, 8, false)),
        _ => None,
    }
}

// Four bits describe the quadrants of a face; their union blocks flow
// only if it covers the whole face. Solid and unknown cells block all faces.
fn face(cell: Cell, direction: IVec3) -> u8 {
    match cell {
        Cell::Air | Cell::Fluid(_) => 0,
        Cell::Solid | Cell::Unknown | Cell::GrateWater(_) => 15,
        Cell::Slab { top, .. } if direction == IVec3::Y => {
            if top {
                15
            } else {
                0
            }
        }
        Cell::Slab { top, .. } if direction == IVec3::NEG_Y => {
            if top {
                0
            } else {
                15
            }
        }
        Cell::Slab { top, .. } => {
            if top {
                12
            } else {
                3
            }
        }
        Cell::Stairs { voxels, .. } => {
            let indexes = if direction == IVec3::X {
                [1, 3, 5, 7]
            } else if direction == IVec3::NEG_X {
                [0, 2, 4, 6]
            } else if direction == IVec3::Y {
                [4, 5, 6, 7]
            } else if direction == IVec3::NEG_Y {
                [0, 1, 2, 3]
            } else if direction == IVec3::Z {
                [2, 3, 6, 7]
            } else {
                [0, 1, 4, 5]
            };
            indexes
                .into_iter()
                .enumerate()
                .fold(0, |face, (bit, voxel)| {
                    face | (((voxels >> voxel) & 1) << bit)
                })
        }
    }
}

fn can_pass(from: Cell, to: Cell, direction: IVec3) -> bool {
    face(from, direction) | face(to, -direction) != 15
}

/// Encode a shape made from half-block-aligned cuboids as eight occupied cells.
pub fn octants(boxes: &[crate::collision::Aabb]) -> Option<u8> {
    if boxes
        .iter()
        .flat_map(|b| b.min.to_array().into_iter().chain(b.max.to_array()))
        .any(|v| !matches!(v, 0.0 | 0.5 | 1.0))
    {
        return None;
    }
    let mut mask = 0;
    for y in 0..2 {
        for z in 0..2 {
            for x in 0..2 {
                let point = glam::Vec3::new(
                    x as f32 * 0.5 + 0.25,
                    y as f32 * 0.5 + 0.25,
                    z as f32 * 0.5 + 0.25,
                );
                if boxes
                    .iter()
                    .any(|b| point.cmpgt(b.min).all() && point.cmplt(b.max).all())
                {
                    mask |= 1 << (x + 2 * z + 4 * y);
                }
            }
        }
    }
    Some(mask)
}

fn flowing(kind: FluidKind, amount: u8, falling: bool) -> FluidState {
    FluidState {
        kind,
        amount,
        source: amount == 8 && !falling,
        falling,
    }
}

/// Recalculate a fluid at a position using adjacent source and falling states.
pub fn new_liquid(
    world: &impl Fn(IVec3) -> Cell,
    position: IVec3,
    kind: FluidKind,
    settings: Settings,
) -> Option<FluidState> {
    let current = world(position);
    let mut most = 0;
    let mut sources = 0;
    for direction in SIDES {
        let neighbor = at(world, offset(position, direction));
        if let Some(state) = fluid(neighbor)
            && state.kind == kind
            && can_pass(current, neighbor, direction)
        {
            most = most.max(state.amount);
            sources += u8::from(state.source);
        }
    }
    if sources >= 2 && settings.converts_source(kind) {
        let below = at(world, offset(position, IVec3::NEG_Y));
        if matches!(below, Cell::Solid | Cell::Slab { .. } | Cell::Stairs { .. })
            || source(below, kind)
        {
            return Some(flowing(kind, 8, false));
        }
    }
    let above = at(world, offset(position, IVec3::Y));
    if fluid(above).is_some_and(|state| state.kind == kind) && can_pass(current, above, IVec3::Y) {
        return Some(flowing(kind, 8, true));
    }
    let amount = most.saturating_sub(settings.drop_off(kind));
    (amount > 0).then(|| flowing(kind, amount, false))
}

fn can_search(cell: Cell, kind: FluidKind) -> bool {
    match cell {
        Cell::Air => true,
        Cell::Fluid(_) => !source(cell, kind),
        Cell::Slab { .. } | Cell::Stairs { .. } => !source(cell, kind),
        Cell::Solid | Cell::GrateWater(_) | Cell::Unknown => false,
    }
}

fn is_hole(world: &impl Fn(IVec3) -> Cell, pos: IVec3, kind: FluidKind) -> bool {
    let below = at(world, offset(pos, IVec3::NEG_Y));
    if !can_pass(world(pos), below, IVec3::NEG_Y) {
        return false;
    }
    match below {
        Cell::Air | Cell::Fluid(_) => true,
        Cell::Slab {
            waterlogged: true, ..
        }
        | Cell::Stairs {
            waterlogged: true, ..
        } => kind == FluidKind::Water,
        _ => false,
    }
}

fn can_replace(
    world: &impl Fn(IVec3) -> Cell,
    position: IVec3,
    incoming: FluidState,
    down: bool,
) -> bool {
    match world(position) {
        Cell::Air => true,
        Cell::Slab {
            waterlogged: false, ..
        }
        | Cell::Stairs {
            waterlogged: false, ..
        } => incoming.kind == FluidKind::Water && incoming.source,
        Cell::Fluid(state)
            if state.kind == FluidKind::Water && incoming.kind == FluidKind::Lava =>
        {
            down
        }
        Cell::Fluid(state)
            if state.kind == FluidKind::Lava && incoming.kind == FluidKind::Water =>
        {
            state.amount >= 4
                || matches!(at(world, offset(position, IVec3::Y)), Cell::Fluid(above) if above.kind == FluidKind::Lava)
        }
        _ => false,
    }
}

fn slope_distance(
    world: &impl Fn(IVec3) -> Cell,
    pos: IVec3,
    previous: IVec3,
    depth: u8,
    kind: FluidKind,
    limit: u8,
) -> u16 {
    let mut best = 1000;
    let current = world(pos);
    for direction in SIDES {
        if direction == -previous {
            continue;
        }
        let Some(next) = offset(pos, direction) else {
            continue;
        };
        // Slope search asks whether the *flowing* fluid can enter the block;
        // slabs accept only source water, even when their shape is open.
        let candidate = world(next);
        if matches!(candidate, Cell::Slab { .. } | Cell::Stairs { .. })
            || !can_search(candidate, kind)
            || !can_pass(current, candidate, direction)
        {
            continue;
        }
        if is_hole(world, next, kind) {
            return u16::from(depth);
        }
        if depth < limit {
            best = best.min(slope_distance(
                world,
                next,
                direction,
                depth + 1,
                kind,
                limit,
            ));
        }
    }
    best
}

fn side_changes(
    world: &impl Fn(IVec3) -> Cell,
    pos: IVec3,
    state: FluidState,
    settings: Settings,
) -> Vec<Change> {
    let outgoing = if state.falling {
        7
    } else {
        state.amount.saturating_sub(settings.drop_off(state.kind))
    };
    if outgoing == 0 {
        return Vec::new();
    }
    let mut best = 1000;
    let mut changes = Vec::new();
    let current = world(pos);
    for direction in SIDES {
        let Some(next) = offset(pos, direction) else {
            continue;
        };
        let cell = world(next);
        if !can_search(cell, state.kind) || !can_pass(current, cell, direction) {
            continue;
        }
        let Some(new) = new_liquid(world, next, state.kind, settings) else {
            continue;
        };
        if matches!(cell, Cell::Slab { .. } | Cell::Stairs { .. })
            && !(new.kind == FluidKind::Water && new.source)
        {
            continue;
        }
        let distance = if is_hole(world, next, state.kind) {
            0
        } else {
            slope_distance(
                world,
                next,
                direction,
                1,
                state.kind,
                settings.slope_distance(state.kind),
            )
        };
        if distance < best {
            changes.clear();
        }
        if distance <= best {
            if can_replace(world, next, new, false) {
                changes.push(Change {
                    position: next,
                    block: match cell {
                        Cell::Slab { top, .. } => ChangedBlock::WaterloggedSlab { top },
                        Cell::Stairs { voxels, .. } => ChangedBlock::WaterloggedStairs { voxels },
                        _ => ChangedBlock::Fluid(new),
                    },
                });
            }
            best = distance;
        }
    }
    changes
}

/// Plan one scheduled tick in air, full blocks, ordinary liquids and slabs.
pub fn tick(
    world: &impl Fn(IVec3) -> Cell,
    position: IVec3,
    kind: FluidKind,
    settings: Settings,
) -> Vec<Change> {
    let original = world(position);
    let Some(mut current) = fluid(original) else {
        return Vec::new();
    };
    if matches!(original, Cell::GrateWater(_)) {
        return Vec::new();
    }
    if current.kind != kind {
        return Vec::new();
    }
    let mut changes = Vec::new();
    if !source(Cell::Fluid(current), kind) {
        let next = new_liquid(world, position, kind, settings);
        if next != Some(current) {
            changes.push(Change {
                position,
                block: next.map_or(ChangedBlock::Air, ChangedBlock::Fluid),
            });
        }
        let Some(next) = next else { return changes };
        current = next;
    }
    let with_self = |pos| {
        if pos == position {
            if matches!(original, Cell::Slab { .. } | Cell::Stairs { .. }) {
                original
            } else {
                Cell::Fluid(current)
            }
        } else {
            world(pos)
        }
    };
    let below_pos = offset(position, IVec3::NEG_Y);
    let below = at(&with_self, below_pos);
    let next_below = below_pos.and_then(|pos| {
        (can_search(below, kind) && can_pass(with_self(position), below, IVec3::NEG_Y))
            .then(|| new_liquid(&with_self, pos, kind, settings))
            .flatten()
            .filter(|next| can_replace(&with_self, pos, *next, true))
    });
    if let Some(next) = next_below {
        if let Some(below_pos) = below_pos {
            let block = if kind == FluidKind::Lava
                && matches!(below, Cell::Fluid(state) if state.kind == FluidKind::Water)
            {
                ChangedBlock::Solid(Solidified::Stone)
            } else if let Cell::Slab { top, .. } = below {
                ChangedBlock::WaterloggedSlab { top }
            } else if let Cell::Stairs { voxels, .. } = below {
                ChangedBlock::WaterloggedStairs { voxels }
            } else {
                ChangedBlock::Fluid(next)
            };
            changes.push(Change {
                position: below_pos,
                block,
            });
            let count = SIDES
                .iter()
                .filter(|direction| source(at(&with_self, offset(position, **direction)), kind))
                .count();
            if count >= 3 {
                let down = |pos| {
                    if pos == below_pos {
                        match block {
                            ChangedBlock::Fluid(state) => Cell::Fluid(state),
                            ChangedBlock::WaterloggedSlab { top } => Cell::Slab {
                                top,
                                waterlogged: true,
                            },
                            ChangedBlock::WaterloggedStairs { voxels } => Cell::Stairs {
                                voxels,
                                waterlogged: true,
                            },
                            _ => Cell::Solid,
                        }
                    } else {
                        with_self(pos)
                    }
                };
                changes.extend(side_changes(&down, position, current, settings));
            }
        }
    } else if source(Cell::Fluid(current), kind) || !is_hole(&with_self, position, kind) {
        changes.extend(side_changes(&with_self, position, current, settings));
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::{block_shape, merged_face_occludes};
    use crate::loaded::BlockState;

    #[test]
    fn compact_faces_match_full_collision_shapes_for_stairs_and_slabs() {
        let mut shapes = Vec::new();
        for facing in ["north", "south", "west", "east"] {
            for half in ["bottom", "top"] {
                for kind in [
                    "straight",
                    "inner_left",
                    "inner_right",
                    "outer_left",
                    "outer_right",
                ] {
                    let state = BlockState {
                        name: "minecraft:oak_stairs".into(),
                        properties: [
                            ("facing", facing),
                            ("half", half),
                            ("shape", kind),
                            ("waterlogged", "false"),
                        ]
                        .into_iter()
                        .map(|(k, v)| (k.into(), v.into()))
                        .collect(),
                    };
                    let boxes = block_shape(&state).unwrap();
                    shapes.push((
                        Cell::Stairs {
                            voxels: octants(&boxes).unwrap(),
                            waterlogged: false,
                        },
                        boxes,
                    ));
                }
            }
        }
        for top in [false, true] {
            let state = BlockState {
                name: "minecraft:oak_slab".into(),
                properties: [
                    ("type", if top { "top" } else { "bottom" }),
                    ("waterlogged", "false"),
                ]
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
            };
            shapes.push((
                Cell::Slab {
                    top,
                    waterlogged: false,
                },
                block_shape(&state).unwrap(),
            ));
        }
        for (from, from_boxes) in &shapes {
            for (to, to_boxes) in &shapes {
                for direction in [
                    IVec3::X,
                    IVec3::NEG_X,
                    IVec3::Y,
                    IVec3::NEG_Y,
                    IVec3::Z,
                    IVec3::NEG_Z,
                ] {
                    assert_eq!(
                        can_pass(*from, *to, direction),
                        !merged_face_occludes(from_boxes, to_boxes, direction),
                        "{from:?} -> {to:?} {direction:?}"
                    );
                }
            }
        }
    }
}
