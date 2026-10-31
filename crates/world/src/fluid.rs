//! Fluid states and surface sampling used by the read-only world renderer.
//!
//! Cells carry solidity supplied by the caller; incomplete block shape information is
//! intentionally kept separate from the vanilla fluid height and averaging rules.

use glam::{DVec3, IVec3};

use crate::{loaded::BlockState, movement::BoundingBox};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum FluidKind {
    Water,
    Lava,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FluidState {
    pub kind: FluidKind,
    pub amount: u8,
    pub source: bool,
    pub falling: bool,
}

impl FluidState {
    pub fn from_block(block: &BlockState) -> Result<Option<Self>, String> {
        let kind = match block.name.as_str() {
            "minecraft:water" => Some(FluidKind::Water),
            "minecraft:lava" => Some(FluidKind::Lava),
            _ => None,
        };
        if let Some(kind) = kind {
            let level = match block.properties.get("level") {
                None => 0,
                Some(raw) => raw
                    .parse::<u8>()
                    .ok()
                    .filter(|level| *level <= 15)
                    .ok_or_else(|| format!("invalid {} level {raw}", block.name))?,
            };
            let amount = if level == 0 || level >= 8 {
                8
            } else {
                8 - level
            };
            return Ok(Some(Self {
                kind,
                amount,
                source: level == 0,
                falling: level >= 8,
            }));
        }
        let waterlogged = block
            .properties
            .get("waterlogged")
            .is_some_and(|value| value == "true");
        let immersed = matches!(
            block.name.as_str(),
            "minecraft:bubble_column"
                | "minecraft:kelp"
                | "minecraft:kelp_plant"
                | "minecraft:seagrass"
                | "minecraft:tall_seagrass"
        );
        // Copper grates return source water with its falling flag set.
        let falling_grate = waterlogged && is_copper_grate(&block.name);
        Ok((waterlogged || immersed).then_some(Self {
            kind: FluidKind::Water,
            amount: 8,
            source: true,
            falling: falling_grate,
        }))
    }

    pub fn own_height(self) -> f32 {
        f32::from(self.amount) / 9.0
    }
}

/// Copper grate variants share the same water-occupancy behavior.
pub fn is_copper_grate(name: &str) -> bool {
    matches!(
        name,
        "minecraft:copper_grate"
            | "minecraft:exposed_copper_grate"
            | "minecraft:weathered_copper_grate"
            | "minecraft:oxidized_copper_grate"
            | "minecraft:waxed_copper_grate"
            | "minecraft:waxed_exposed_copper_grate"
            | "minecraft:waxed_weathered_copper_grate"
            | "minecraft:waxed_oxidized_copper_grate"
    )
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cell {
    Empty,
    Solid,
    Fluid(FluidState),
    /// The loaded region has no block data at this coordinate.
    Missing,
}

fn offset(position: IVec3, dx: i32, dy: i32, dz: i32) -> Option<IVec3> {
    Some(IVec3::new(
        position.x.checked_add(dx)?,
        position.y.checked_add(dy)?,
        position.z.checked_add(dz)?,
    ))
}

fn at(world: &impl Fn(IVec3) -> Cell, pos: Option<IVec3>) -> Cell {
    pos.map(world).unwrap_or(Cell::Missing)
}

pub fn same_fluid(cell: Cell, kind: FluidKind) -> bool {
    matches!(cell, Cell::Fluid(state) if state.kind == kind)
}

/// Height of a cell relative to a target fluid. Solids return -1, open cells return 0.
pub fn height(world: &impl Fn(IVec3) -> Cell, position: IVec3, kind: FluidKind) -> f32 {
    match world(position) {
        Cell::Fluid(state) if state.kind == kind => {
            if same_fluid(at(world, offset(position, 0, 1, 0)), kind) {
                1.0
            } else {
                state.own_height()
            }
        }
        Cell::Solid => -1.0,
        _ => 0.0,
    }
}

/// The corner height at the intersection of a center cell, two edges and a diagonal.
pub fn corner_height(center: f32, edge_a: f32, edge_b: f32, diagonal: Option<f32>) -> f32 {
    if edge_a >= 1.0 || edge_b >= 1.0 {
        return 1.0;
    }
    let mut weighted = 0.0f32;
    let mut count = 0.0f32;
    let mut add = |value: f32| {
        if value >= 0.8 {
            weighted += value * 10.0;
            count += 10.0;
        } else if value >= 0.0 {
            weighted += value;
            count += 1.0;
        }
    };
    if (edge_a > 0.0 || edge_b > 0.0)
        && let Some(value) = diagonal
    {
        if value >= 1.0 {
            return 1.0;
        }
        add(value);
    }
    add(center);
    add(edge_b);
    add(edge_a);
    weighted / count
}

/// Surface heights in northwest, southwest, southeast, northeast order.
pub fn surface_heights(
    world: &impl Fn(IVec3) -> Cell,
    position: IVec3,
    kind: FluidKind,
) -> [f32; 4] {
    let sample = |dx, dz| {
        offset(position, dx, 0, dz)
            .map(|position| height(world, position, kind))
            .unwrap_or(0.0)
    };
    let center = sample(0, 0);
    if center >= 1.0 {
        return [1.0; 4];
    }
    let north = sample(0, -1);
    let south = sample(0, 1);
    let east = sample(1, 0);
    let west = sample(-1, 0);
    [
        corner_height(center, north, west, Some(sample(-1, -1))),
        corner_height(center, south, west, Some(sample(-1, 1))),
        corner_height(center, south, east, Some(sample(1, 1))),
        corner_height(center, north, east, Some(sample(1, -1))),
    ]
}

/// The normalized horizontal direction used to orient the flowing surface texture.
/// Vertical falling velocity does not change the horizontal texture angle.
pub fn horizontal_flow(
    world: &impl Fn(IVec3) -> Cell,
    position: IVec3,
    state: FluidState,
) -> [f64; 2] {
    let mut flow = [0.0f64; 2];
    let center = state.own_height();
    for (dx, dz) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
        let neighbor = offset(position, dx, 0, dz);
        let cell = at(world, neighbor);
        let delta = match cell {
            Cell::Fluid(other) if other.kind == state.kind => center - other.own_height(),
            Cell::Empty | Cell::Missing => {
                let below = neighbor.and_then(|pos| offset(pos, 0, -1, 0));
                match at(world, below) {
                    Cell::Fluid(other) if other.kind == state.kind => {
                        center - (other.own_height() - 8.0 / 9.0)
                    }
                    _ => 0.0,
                }
            }
            _ => 0.0,
        };
        flow[0] += f64::from(dx as f32 * delta);
        flow[1] += f64::from(dz as f32 * delta);
    }
    let length = (flow[0] * flow[0] + flow[1] * flow[1]).sqrt();
    if length < 1.0e-4 {
        [0.0, 0.0]
    } else {
        [flow[0] / length, flow[1] / length]
    }
}

/// Fluid flow including the downward component next to a sturdy face.
pub fn flow(
    world: &impl Fn(IVec3) -> Cell,
    sturdy_face: &impl Fn(IVec3, IVec3) -> bool,
    position: IVec3,
    state: FluidState,
) -> DVec3 {
    let horizontal = horizontal_flow(world, position, state);
    let mut flow = DVec3::new(horizontal[0], 0.0, horizontal[1]);
    if state.falling {
        for (dx, dz) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
            let direction = IVec3::new(dx, 0, dz);
            let neighbor = offset(position, dx, 0, dz);
            let face_is_solid = |pos: Option<IVec3>| {
                pos.is_some_and(|pos| {
                    !same_fluid(world(pos), state.kind) && sturdy_face(pos, direction)
                })
            };
            if face_is_solid(neighbor) || face_is_solid(neighbor.and_then(|p| offset(p, 0, 1, 0))) {
                flow.y = -6.0;
                break;
            }
        }
    }
    let length = flow.length();
    if length < 1.0e-4 {
        DVec3::ZERO
    } else {
        flow / length
    }
}

/// Interaction with one fluid tag after sampling the player's bounding box.
#[derive(Clone, Copy, Debug, Default)]
pub struct FluidContact {
    pub depth: f64,
    pub eyes_inside: bool,
    current: DVec3,
    count: u32,
}

impl FluidContact {
    /// Apply the averaged current to a player, including vanilla's minimum push.
    pub fn push(self, velocity: DVec3, strength: f64) -> DVec3 {
        self.push_inner(velocity, strength, false)
    }

    /// Non-player entities normalize the accumulated current before applying it.
    pub fn push_entity(self, velocity: DVec3, strength: f64) -> DVec3 {
        self.push_inner(velocity, strength, true)
    }

    fn push_inner(self, velocity: DVec3, strength: f64, normalize: bool) -> DVec3 {
        if self.count == 0 || self.current.length_squared() < f64::from(1.0e-5_f32) {
            return velocity;
        }
        let mut force = if normalize {
            self.current.normalize() * strength
        } else {
            self.current * (strength / f64::from(self.count))
        };
        if velocity.x.abs() < 0.003 && velocity.z.abs() < 0.003 && force.length() < 0.0045 {
            let length = force.length();
            if length >= 1.0e-4 {
                force = force / length * 0.0045;
            } else {
                force = DVec3::ZERO;
            }
        }
        velocity + force
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FluidContacts {
    pub water: FluidContact,
    pub lava: FluidContact,
}

/// Fluid cells touched by an entity along Java's axis-ordered clipped path.
/// Returns None for unavailable cells or an excessive search volume.
pub fn swept_effects(
    bounds: BoundingBox,
    actual: DVec3,
    requested: DVec3,
    world: &impl Fn(IVec3) -> Cell,
) -> Option<Vec<FluidKind>> {
    use std::collections::HashSet;
    if !actual.is_finite()
        || !requested.is_finite()
        || !bounds.min.is_finite()
        || !bounds.max.is_finite()
        || !bounds.min.cmple(bounds.max).all()
    {
        return None;
    }
    let axes = if requested.x.abs() < requested.z.abs() {
        [1, 2, 0]
    } else {
        [1, 0, 2]
    };
    let mut position = bounds;
    let mut visited = HashSet::new();
    let mut effects = Vec::new();
    let mut work = 0_u64;
    for axis in axes {
        let mut delta = DVec3::ZERO;
        delta[axis] = actual[axis];
        if delta == DVec3::ZERO && actual != DVec3::ZERO {
            continue;
        }
        let search = position.expanded_towards(delta);
        let coordinate_limit = DVec3::splat(f64::from(i32::MAX));
        let coordinate_min = DVec3::splat(f64::from(i32::MIN));
        if !search.min.is_finite()
            || !search.max.is_finite()
            || !search.min.cmpge(coordinate_min).all()
            || !search.max.cmple(coordinate_limit).all()
        {
            return None;
        }
        let min = search.min.floor().as_ivec3();
        let max = search.max.floor().as_ivec3();
        let volume = ((i64::from(max.x) - i64::from(min.x) + 1) as u64)
            .checked_mul((i64::from(max.y) - i64::from(min.y) + 1) as u64)?
            .checked_mul((i64::from(max.z) - i64::from(min.z) + 1) as u64)?;
        work = work.checked_add(volume)?;
        if work > 4096 {
            return None;
        }
        let mut hits = Vec::new();
        for x in min.x..=max.x {
            for y in min.y..=max.y {
                for z in min.z..=max.z {
                    let pos = IVec3::new(x, y, z);
                    if visited.contains(&pos) {
                        continue;
                    }
                    let base = pos.as_dvec3();
                    let target = |height: f32| BoundingBox {
                        min: base,
                        max: base + DVec3::new(1.0, f64::from(height), 1.0),
                    };
                    let state = match world(pos) {
                        Cell::Fluid(state) => state,
                        Cell::Missing => {
                            // A missing cell outside the actual swept box cannot
                            // contribute a fluid effect, even at full height.
                            if position.swept_contact_time(delta, target(1.0)).is_some() {
                                return None;
                            }
                            continue;
                        }
                        _ => continue,
                    };
                    let above = at(world, offset(pos, 0, 1, 0));
                    let time = match above {
                        Cell::Missing => {
                            // The missing cell can only change this fluid's height.
                            // Preserve the result only if both heights give the same
                            // contact and ordering along the path.
                            let lower =
                                position.swept_contact_time(delta, target(state.own_height()));
                            let upper = position.swept_contact_time(delta, target(1.0));
                            if lower != upper {
                                return None;
                            }
                            lower
                        }
                        _ => position.swept_contact_time(
                            delta,
                            target(if same_fluid(above, state.kind) {
                                1.0
                            } else {
                                state.own_height()
                            }),
                        ),
                    };
                    if let Some(time) = time {
                        hits.push((time, state.kind, pos));
                    }
                }
            }
        }
        hits.sort_by(|a, b| {
            a.0.total_cmp(&b.0).then_with(|| match (a.1, b.1) {
                (FluidKind::Lava, FluidKind::Water) => std::cmp::Ordering::Less,
                (FluidKind::Water, FluidKind::Lava) => std::cmp::Ordering::Greater,
                _ => a.2.to_array().cmp(&b.2.to_array()),
            })
        });
        for (_, kind, pos) in hits {
            if visited.insert(pos) {
                effects.push(kind);
            }
        }
        position = position.moved(delta);
        if actual == DVec3::ZERO {
            break;
        }
    }
    Some(effects)
}

/// Sample the deflated player box using 26.3 fluid heights and camera heights.
pub fn contacts(
    bounds: BoundingBox,
    eye_y: f64,
    world: &impl Fn(IVec3) -> Cell,
    sturdy_face: &impl Fn(IVec3, IVec3) -> bool,
) -> FluidContacts {
    let area = BoundingBox {
        min: bounds.min + DVec3::splat(0.001),
        max: bounds.max - DVec3::splat(0.001),
    };
    let start = area.min.floor().as_ivec3();
    let end = area.max.ceil().as_ivec3() - IVec3::ONE;
    let eye_x = ((bounds.min.x + bounds.max.x) * 0.5).floor() as i32;
    let eye_z = ((bounds.min.z + bounds.max.z) * 0.5).floor() as i32;
    let mut result = FluidContacts::default();
    for x in start.x..=end.x {
        for y in start.y..=end.y {
            for z in start.z..=end.z {
                let position = IVec3::new(x, y, z);
                let Cell::Fluid(state) = world(position) else {
                    continue;
                };
                let surface = f64::from(y) + f64::from(height(world, position, state.kind));
                if surface < area.min.y {
                    continue;
                }
                let contact = match state.kind {
                    FluidKind::Water => &mut result.water,
                    FluidKind::Lava => &mut result.lava,
                };
                if x == eye_x && z == eye_z && eye_y >= f64::from(y) {
                    let above = offset(position, 0, 1, 0);
                    let camera_height =
                        if state.source && above.is_some_and(|p| sturdy_face(p, IVec3::NEG_Y)) {
                            1.0
                        } else {
                            height(world, position, state.kind)
                        };
                    if eye_y <= f64::from(y) + f64::from(camera_height) {
                        contact.eyes_inside = true;
                    }
                }
                contact.depth = contact.depth.max(surface - bounds.min.y);
                let mut current = flow(world, sturdy_face, position, state);
                if contact.depth < 0.4 {
                    current *= contact.depth;
                }
                contact.current += current;
                contact.count += 1;
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, HashMap};

    #[test]
    fn swept_effects_find_fast_crossings_and_preserve_fluid_order() {
        let water = FluidState {
            kind: FluidKind::Water,
            amount: 8,
            source: true,
            falling: false,
        };
        let lava = FluidState {
            kind: FluidKind::Lava,
            amount: 8,
            source: true,
            falling: false,
        };
        let world = |pos: IVec3| match pos.x {
            1 if pos.y == 0 && pos.z == 0 => Cell::Fluid(lava),
            2 if pos.y == 0 && pos.z == 0 => Cell::Fluid(water),
            _ => Cell::Empty,
        };
        let start = BoundingBox {
            min: DVec3::new(-0.125, 0.1, 0.375),
            max: DVec3::new(0.125, 0.35, 0.625),
        };
        let movement = DVec3::new(4.0, 0.0, 0.0);
        assert_eq!(
            swept_effects(start, movement, movement, &world),
            Some(vec![FluidKind::Lava, FluidKind::Water])
        );
        assert_eq!(
            swept_effects(start.moved(movement), -movement, -movement, &world),
            Some(vec![FluidKind::Water, FluidKind::Lava])
        );
        let unavailable = |pos: IVec3| {
            if pos.x == 1 {
                Cell::Missing
            } else {
                Cell::Empty
            }
        };
        assert_eq!(swept_effects(start, movement, movement, &unavailable), None);
    }

    #[test]
    fn swept_effects_require_the_cell_above_to_determine_fluid_height() {
        let water = FluidState {
            kind: FluidKind::Water,
            amount: 8,
            source: true,
            falling: false,
        };
        let bounds = BoundingBox {
            min: DVec3::new(0.25, 0.9, 0.25),
            max: DVec3::new(0.75, 0.95, 0.75),
        };
        let world = |above| {
            move |pos| match pos {
                IVec3::ZERO => Cell::Fluid(water),
                IVec3::Y => above,
                _ => Cell::Empty,
            }
        };
        assert_eq!(
            swept_effects(bounds, DVec3::ZERO, DVec3::ZERO, &world(Cell::Missing)),
            None
        );
        assert_eq!(
            swept_effects(bounds, DVec3::ZERO, DVec3::ZERO, &world(Cell::Empty)),
            Some(vec![])
        );
        assert_eq!(
            swept_effects(bounds, DVec3::ZERO, DVec3::ZERO, &world(Cell::Fluid(water))),
            Some(vec![FluidKind::Water])
        );
        let low_bounds = bounds.moved(DVec3::new(0.0, -0.5, 0.0));
        assert_eq!(
            swept_effects(low_bounds, DVec3::ZERO, DVec3::ZERO, &world(Cell::Missing)),
            Some(vec![FluidKind::Water])
        );
    }

    #[test]
    fn unloaded_cells_only_block_effects_when_the_swept_box_can_touch_them() {
        let world = |pos: IVec3| {
            if pos.x == 1 {
                Cell::Missing
            } else {
                Cell::Empty
            }
        };
        let bounds = BoundingBox {
            min: DVec3::new(0.5, 0.1, 0.25),
            max: DVec3::new(1.0, 0.35, 0.75),
        };
        assert_eq!(
            swept_effects(bounds, DVec3::ZERO, DVec3::ZERO, &world),
            Some(vec![])
        );
        let movement = DVec3::new(0.25, 0.0, 0.0);
        assert_eq!(swept_effects(bounds, movement, movement, &world), None);
    }

    #[test]
    fn swept_effects_reject_invalid_or_unrepresentable_bounds() {
        let world = |_| Cell::Empty;
        let valid = BoundingBox {
            min: DVec3::ZERO,
            max: DVec3::ONE,
        };
        let invalid = [
            BoundingBox {
                min: DVec3::splat(f64::NAN),
                ..valid
            },
            BoundingBox {
                min: DVec3::ONE,
                max: DVec3::ZERO,
            },
            BoundingBox {
                min: DVec3::splat(3.0e9),
                max: DVec3::splat(3.0e9 + 1.0),
            },
        ];
        for bounds in invalid {
            assert_eq!(
                swept_effects(bounds, DVec3::ZERO, DVec3::ZERO, &world),
                None
            );
        }
        assert_eq!(
            swept_effects(valid, DVec3::splat(f64::MAX), DVec3::ZERO, &world),
            None
        );
    }

    #[test]
    fn item_currents_are_normalized_while_player_currents_are_averaged() {
        let contact = FluidContact {
            depth: 0.2,
            eyes_inside: false,
            current: DVec3::new(0.2, 0.0, 0.2),
            count: 2,
        };
        let velocity = DVec3::new(0.1, 0.0, 0.1);
        let player = contact.push(velocity, 0.014);
        let item = contact.push_entity(velocity, 0.014);
        assert!((player.x - 0.1014).abs() < 1.0e-12);
        assert!((item.x - (0.1 + 0.014 / 2.0_f64.sqrt())).abs() < 1.0e-12);
        assert_eq!(item.x, item.z);
        let slow = contact.push_entity(DVec3::ZERO, 0.0023333333333333335);
        assert!((slow.length() - 0.0045).abs() < 1.0e-12);
        let zero = FluidContact {
            current: DVec3::ZERO,
            ..contact
        };
        assert_eq!(zero.push_entity(velocity, 0.014), velocity);
    }

    fn block(name: &str, key: &str, value: &str) -> BlockState {
        BlockState {
            name: name.into(),
            properties: BTreeMap::from([(key.into(), value.into())]),
        }
    }

    #[test]
    fn source_flowing_falling_and_waterlogged_states_have_expected_amounts() {
        for (level, amount, falling) in [
            (0, 8, false),
            (1, 7, false),
            (7, 1, false),
            (8, 8, true),
            (15, 8, true),
        ] {
            let value =
                FluidState::from_block(&block("minecraft:water", "level", &level.to_string()))
                    .unwrap()
                    .unwrap();
            assert_eq!((value.amount, value.falling), (amount, falling));
        }
        assert!(FluidState::from_block(&block("minecraft:water", "level", "16")).is_err());
        assert_eq!(
            FluidState::from_block(&block("minecraft:oak_slab", "waterlogged", "true"))
                .unwrap()
                .unwrap()
                .kind,
            FluidKind::Water
        );
    }

    #[test]
    fn waterlogged_copper_grates_contain_falling_water() {
        for name in [
            "minecraft:copper_grate",
            "minecraft:weathered_copper_grate",
            "minecraft:waxed_oxidized_copper_grate",
        ] {
            let wet = FluidState::from_block(&block(name, "waterlogged", "true"))
                .unwrap()
                .unwrap();
            assert_eq!(
                (wet.kind, wet.amount, wet.source, wet.falling),
                (FluidKind::Water, 8, true, true)
            );
            assert!(
                FluidState::from_block(&block(name, "waterlogged", "false"))
                    .unwrap()
                    .is_none()
            );
        }
        assert!(
            !FluidState::from_block(&block("minecraft:oak_slab", "waterlogged", "true"))
                .unwrap()
                .unwrap()
                .falling
        );
    }

    #[test]
    fn corners_weight_full_fluid_and_recognize_a_fluid_above() {
        let source = FluidState {
            kind: FluidKind::Water,
            amount: 8,
            source: true,
            falling: false,
        };
        let world = |p: IVec3| {
            if p == IVec3::ZERO {
                Cell::Fluid(source)
            } else {
                Cell::Empty
            }
        };
        assert!(
            (surface_heights(&world, IVec3::ZERO, FluidKind::Water)[0] - 0.7407408).abs() < 1e-6
        );
        let stacked = |p: IVec3| {
            if p == IVec3::ZERO || p == IVec3::Y {
                Cell::Fluid(source)
            } else {
                Cell::Empty
            }
        };
        assert_eq!(
            surface_heights(&stacked, IVec3::ZERO, FluidKind::Water),
            [1.0; 4]
        );
    }

    #[test]
    fn contacts_distinguish_body_depth_and_submerged_eyes() {
        let source = FluidState {
            kind: FluidKind::Water,
            amount: 8,
            source: true,
            falling: false,
        };
        let mut blocks = HashMap::from([(IVec3::ZERO, Cell::Fluid(source))]);
        let bounds = BoundingBox {
            min: DVec3::new(0.2, 0.5, 0.2),
            max: DVec3::new(0.8, 2.3, 0.8),
        };
        let sample = |blocks: &HashMap<IVec3, Cell>| {
            contacts(
                bounds,
                0.75,
                &|pos| blocks.get(&pos).copied().unwrap_or(Cell::Empty),
                &|_, _| false,
            )
        };
        let single = sample(&blocks);
        assert!((single.water.depth - (f64::from(8.0_f32 / 9.0) - 0.5)).abs() < 1.0e-12);
        assert!(single.water.eyes_inside);
        assert_eq!(single.lava.depth, 0.0);
        blocks.insert(IVec3::Y, Cell::Fluid(source));
        let stacked = sample(&blocks);
        assert!((stacked.water.depth - (1.0 + f64::from(8.0_f32 / 9.0) - 0.5)).abs() < 1.0e-12);
    }

    #[test]
    fn falling_water_next_to_a_wall_pulls_downward() {
        let water = FluidState {
            kind: FluidKind::Water,
            amount: 8,
            source: false,
            falling: true,
        };
        let world = |p| {
            if p == IVec3::ZERO {
                Cell::Fluid(water)
            } else if p == IVec3::X {
                Cell::Solid
            } else {
                Cell::Empty
            }
        };
        let vector = flow(&world, &|p, _| p == IVec3::X, IVec3::ZERO, water);
        assert_eq!(vector, DVec3::NEG_Y);
    }
}
