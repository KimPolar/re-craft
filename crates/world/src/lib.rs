use std::{collections::BTreeMap, sync::Arc};

use glam::{IVec3, Vec3};
use native_core::wood::{Planks, Wood};

pub mod biome_blend;
pub mod biome_zoom;
pub mod block_light;
pub mod collision;
pub mod fluid;
pub mod fluid_sim;
pub mod lighting;
pub mod loaded;
pub mod movement;
pub mod poi;
pub mod raycast;
pub mod sky_light;
pub mod sky_propagation;
pub mod sky_storage;
pub mod stairs;

pub const SECTION_SIZE: i32 = 16;
const SECTION_VOLUME: usize = (SECTION_SIZE * SECTION_SIZE * SECTION_SIZE) as usize;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Block {
    #[default]
    Air,
    Grass,
    Dirt,
    Sand,
    RedSand,
    Gravel,
    CoarseDirt,
    RootedDirt,
    Podzol,
    Mycelium,
    DirtPath,
    Farmland(u8),
    Stone,
    StoneBricks,
    Cobblestone,
    Ore(native_core::ore::Ore),
    Wood(Wood, Axis),
    Planks(Planks),
    CraftingTable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    pub fn from_face(face: IVec3) -> Self {
        if face.x != 0 {
            Self::X
        } else if face.z != 0 {
            Self::Z
        } else {
            Self::Y
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::X => "x",
            Self::Y => "y",
            Self::Z => "z",
        }
    }
}

impl Block {
    pub fn is_solid(self) -> bool {
        self != Self::Air
    }

    pub fn from_item_id(id: &str) -> Option<Self> {
        Some(match id {
            "minecraft:grass_block" => Self::Grass,
            "minecraft:dirt" => Self::Dirt,
            "minecraft:sand" => Self::Sand,
            "minecraft:red_sand" => Self::RedSand,
            "minecraft:gravel" => Self::Gravel,
            "minecraft:coarse_dirt" => Self::CoarseDirt,
            "minecraft:rooted_dirt" => Self::RootedDirt,
            "minecraft:podzol" => Self::Podzol,
            "minecraft:mycelium" => Self::Mycelium,
            "minecraft:dirt_path" => Self::DirtPath,
            "minecraft:farmland" => Self::Farmland(0),
            "minecraft:stone" => Self::Stone,
            "minecraft:stone_bricks" => Self::StoneBricks,
            "minecraft:cobblestone" => Self::Cobblestone,
            "minecraft:crafting_table" => Self::CraftingTable,
            _ => match native_core::ore::Ore::from_id(id) {
                Some(ore) => Self::Ore(ore),
                None => match Wood::from_id(id) {
                    Some(wood) => Self::Wood(wood, Axis::Y),
                    None => Self::Planks(Planks::from_id(id)?),
                },
            },
        })
    }

    pub fn item_id(self) -> Option<&'static str> {
        match self {
            Self::Air => None,
            Self::Grass => Some("minecraft:grass_block"),
            Self::Dirt => Some("minecraft:dirt"),
            Self::Sand => Some("minecraft:sand"),
            Self::RedSand => Some("minecraft:red_sand"),
            Self::Gravel => Some("minecraft:gravel"),
            Self::CoarseDirt => Some("minecraft:coarse_dirt"),
            Self::RootedDirt => Some("minecraft:rooted_dirt"),
            Self::Podzol => Some("minecraft:podzol"),
            Self::Mycelium => Some("minecraft:mycelium"),
            Self::DirtPath => Some("minecraft:dirt_path"),
            Self::Farmland(_) => Some("minecraft:farmland"),
            Self::Stone => Some("minecraft:stone"),
            Self::StoneBricks => Some("minecraft:stone_bricks"),
            Self::Cobblestone => Some("minecraft:cobblestone"),
            Self::Ore(ore) => Some(ore.id()),
            Self::Wood(wood, _) => Some(wood.id()),
            Self::Planks(kind) => Some(kind.id()),
            Self::CraftingTable => Some("minecraft:crafting_table"),
        }
    }

    pub fn orient_for_face(self, face: IVec3) -> Self {
        match self {
            Self::Wood(wood, _) => Self::Wood(wood, Axis::from_face(face)),
            other => other,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SectionPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl SectionPos {
    fn from_block(pos: IVec3) -> Self {
        Self {
            x: pos.x.div_euclid(SECTION_SIZE),
            y: pos.y.div_euclid(SECTION_SIZE),
            z: pos.z.div_euclid(SECTION_SIZE),
        }
    }
}

#[derive(Clone)]
pub struct Section {
    blocks: Box<[Block; SECTION_VOLUME]>,
    non_air: usize,
}

impl Default for Section {
    fn default() -> Self {
        Self {
            blocks: Box::new([Block::Air; SECTION_VOLUME]),
            non_air: 0,
        }
    }
}

impl Section {
    fn index(pos: IVec3) -> usize {
        let x = pos.x.rem_euclid(SECTION_SIZE) as usize;
        let y = pos.y.rem_euclid(SECTION_SIZE) as usize;
        let z = pos.z.rem_euclid(SECTION_SIZE) as usize;
        (y * SECTION_SIZE as usize + z) * SECTION_SIZE as usize + x
    }

    fn get(&self, pos: IVec3) -> Block {
        self.blocks[Self::index(pos)]
    }

    fn set(&mut self, pos: IVec3, block: Block) {
        let old = &mut self.blocks[Self::index(pos)];
        if *old == Block::Air && block != Block::Air {
            self.non_air += 1;
        } else if *old != Block::Air && block == Block::Air {
            self.non_air -= 1;
        }
        *old = block;
    }
}

/// Clones share sections until an edit modifies one of them.
#[derive(Clone, Default)]
pub struct World {
    sections: Arc<BTreeMap<SectionPos, Arc<Section>>>,
    revision: u64,
}

impl World {
    pub fn demo() -> Self {
        let mut world = Self::default();
        for x in -32..32 {
            for z in -32..32 {
                world.set_block(IVec3::new(x, 0, z), Block::Stone);
                world.set_block(IVec3::new(x, 1, z), Block::Dirt);
                world.set_block(IVec3::new(x, 2, z), Block::Grass);
            }
        }
        for y in 3..5 {
            for x in -2..=0 {
                world.set_block(IVec3::new(x, y, -4), Block::Stone);
            }
        }
        world
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn block(&self, pos: IVec3) -> Block {
        self.sections
            .get(&SectionPos::from_block(pos))
            .map_or(Block::Air, |section| section.get(pos))
    }

    pub fn set_block(&mut self, pos: IVec3, block: Block) -> bool {
        if self.block(pos) == block {
            return false;
        }
        let section_pos = SectionPos::from_block(pos);
        let sections = Arc::make_mut(&mut self.sections);
        let section = sections.entry(section_pos).or_default();
        let section = Arc::make_mut(section);
        section.set(pos, block);
        if section.non_air == 0 {
            sections.remove(&section_pos);
        }
        self.revision += 1;
        true
    }

    pub fn for_each_block(&self, mut visit: impl FnMut(IVec3, Block)) {
        for (section_pos, section) in self.sections.iter() {
            let origin = IVec3::new(section_pos.x, section_pos.y, section_pos.z) * SECTION_SIZE;
            for y in 0..SECTION_SIZE {
                for z in 0..SECTION_SIZE {
                    for x in 0..SECTION_SIZE {
                        let pos = origin + IVec3::new(x, y, z);
                        let block = section.get(pos);
                        if block != Block::Air {
                            visit(pos, block);
                        }
                    }
                }
            }
        }
    }

    /// Iterate blocks in the same order as `for_each_block`, with neighbors in
    /// up, down, east, west, south, north order. Interior lookups stay inside
    /// the current shared section; only boundary faces consult the world map.
    pub fn for_each_block_with_neighbors(&self, mut visit: impl FnMut(IVec3, Block, [Block; 6])) {
        for (section_pos, section) in self.sections.iter() {
            let origin = IVec3::new(section_pos.x, section_pos.y, section_pos.z) * SECTION_SIZE;
            for y in 0..SECTION_SIZE {
                for z in 0..SECTION_SIZE {
                    for x in 0..SECTION_SIZE {
                        let index =
                            (y * SECTION_SIZE * SECTION_SIZE + z * SECTION_SIZE + x) as usize;
                        let block = section.blocks[index];
                        if block == Block::Air {
                            continue;
                        }
                        let position = origin + IVec3::new(x, y, z);
                        let boundary = |axis: usize, step: i32| {
                            let mut adjacent = position;
                            let Some(value) = adjacent[axis].checked_add(step) else {
                                return Block::Air;
                            };
                            adjacent[axis] = value;
                            self.block(adjacent)
                        };
                        let neighbors = [
                            if y < SECTION_SIZE - 1 {
                                section.blocks[index + 256]
                            } else {
                                boundary(1, 1)
                            },
                            if y > 0 {
                                section.blocks[index - 256]
                            } else {
                                boundary(1, -1)
                            },
                            if x < SECTION_SIZE - 1 {
                                section.blocks[index + 1]
                            } else {
                                boundary(0, 1)
                            },
                            if x > 0 {
                                section.blocks[index - 1]
                            } else {
                                boundary(0, -1)
                            },
                            if z < SECTION_SIZE - 1 {
                                section.blocks[index + 16]
                            } else {
                                boundary(2, 1)
                            },
                            if z > 0 {
                                section.blocks[index - 16]
                            } else {
                                boundary(2, -1)
                            },
                        ];
                        visit(position, block, neighbors);
                    }
                }
            }
        }
    }

    /// Target block outlines and return the adjacent cell outside the hit face.
    pub fn raycast(&self, origin: Vec3, direction: Vec3, max_distance: f32) -> Option<RayHit> {
        let hit = raycast::raycast_shapes(origin, direction, max_distance, |cell| {
            Some(collision::demo_shape(self.block(cell)))
        })?;
        Some(RayHit {
            position: hit.position,
            adjacent: if hit.inside {
                None
            } else {
                Some(IVec3::new(
                    hit.position.x.checked_add(hit.face.x)?,
                    hit.position.y.checked_add(hit.face.y)?,
                    hit.position.z.checked_add(hit.face.z)?,
                ))
            },
            distance: (hit.location - origin.as_dvec3()).length() as f32,
        })
    }
}

/// Traverse ordered cells with caller-defined occupancy.
pub fn raycast_cells(
    origin: Vec3,
    direction: Vec3,
    max_distance: f32,
    mut occupied: impl FnMut(IVec3) -> bool,
) -> Option<RayHit> {
    if !origin.is_finite()
        || !direction.is_finite()
        || direction.length_squared() <= f32::EPSILON
        || !max_distance.is_finite()
        || max_distance < 0.0
    {
        return None;
    }
    let direction = direction.normalize();
    let mut cell = origin.floor().as_ivec3();
    let step_for = |value: f32| {
        if value > 0.0 {
            1
        } else if value < 0.0 {
            -1
        } else {
            0
        }
    };
    let step = IVec3::new(
        step_for(direction.x),
        step_for(direction.y),
        step_for(direction.z),
    );
    let delta = Vec3::new(
        if step.x == 0 {
            f32::INFINITY
        } else {
            1.0 / direction.x.abs()
        },
        if step.y == 0 {
            f32::INFINITY
        } else {
            1.0 / direction.y.abs()
        },
        if step.z == 0 {
            f32::INFINITY
        } else {
            1.0 / direction.z.abs()
        },
    );
    let first = |coord: f32, block: i32, step: i32, dir: f32| {
        if step == 0 {
            f32::INFINITY
        } else if step > 0 {
            (block as f32 + 1.0 - coord) / dir
        } else {
            (block as f32 - coord) / dir
        }
    };
    let mut next = Vec3::new(
        first(origin.x, cell.x, step.x, direction.x),
        first(origin.y, cell.y, step.y, direction.y),
        first(origin.z, cell.z, step.z, direction.z),
    );
    let mut distance = 0.0;
    let mut adjacent = None;
    while distance <= max_distance {
        if occupied(cell) {
            return Some(RayHit {
                position: cell,
                adjacent,
                distance,
            });
        }
        adjacent = Some(cell);
        if next.x <= next.y && next.x <= next.z {
            distance = next.x;
            next.x += delta.x;
            cell.x = cell.x.checked_add(step.x)?;
        } else if next.y <= next.z {
            distance = next.y;
            next.y += delta.y;
            cell.y = cell.y.checked_add(step.y)?;
        } else {
            distance = next.z;
            next.z += delta.z;
            cell.z = cell.z.checked_add(step.z)?;
        }
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit {
    pub position: IVec3,
    pub adjacent: Option<IVec3>,
    pub distance: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_positions_and_section_boundaries_work() {
        let mut world = World::default();
        for pos in [
            IVec3::new(-1, -1, -1),
            IVec3::new(-16, 0, 16),
            IVec3::new(0, 16, 0),
        ] {
            assert!(world.set_block(pos, Block::Stone));
            assert_eq!(world.block(pos), Block::Stone);
            assert!(world.set_block(pos, Block::Air));
            assert_eq!(world.block(pos), Block::Air);
        }
        assert!(world.sections.is_empty());
    }

    #[test]
    fn neighbor_iteration_matches_world_queries_across_section_boundaries() {
        let mut world = World::default();
        for x in [-17, -16, -1, 0, 15, 16] {
            for y in [-1, 0, 15, 16] {
                for z in [-17, -16, -1, 0, 15, 16] {
                    let block = if (x + y + z) % 2 == 0 {
                        Block::Stone
                    } else {
                        Block::DirtPath
                    };
                    world.set_block(IVec3::new(x, y, z), block);
                }
            }
        }
        for position in [IVec3::new(i32::MIN, 0, 0), IVec3::new(i32::MAX, 0, 0)] {
            world.set_block(position, Block::Dirt);
        }
        let mut seen = 0;
        world.for_each_block_with_neighbors(|position, block, neighbors| {
            seen += 1;
            assert_eq!(world.block(position), block);
            for (actual, direction) in neighbors.into_iter().zip([
                IVec3::Y,
                IVec3::NEG_Y,
                IVec3::X,
                IVec3::NEG_X,
                IVec3::Z,
                IVec3::NEG_Z,
            ]) {
                let adjacent = position
                    .x
                    .checked_add(direction.x)
                    .zip(position.y.checked_add(direction.y))
                    .zip(position.z.checked_add(direction.z))
                    .map(|((x, y), z)| IVec3::new(x, y, z));
                assert_eq!(
                    actual,
                    adjacent.map_or(Block::Air, |pos| world.block(pos)),
                    "{position:?} {direction:?}"
                );
            }
        });
        assert_eq!(seen, 6 * 4 * 6 + 2);
    }

    #[test]
    fn snapshots_keep_old_blocks_and_share_unchanged_sections() {
        let mut world = World::default();
        let a = IVec3::ZERO;
        let b = IVec3::new(16, 0, 0);
        world.set_block(a, Block::Dirt);
        world.set_block(b, Block::Stone);
        let old = world.clone();
        world.set_block(a, Block::Grass);
        assert_eq!(old.block(a), Block::Dirt);
        assert_eq!(world.block(a), Block::Grass);
        assert!(Arc::ptr_eq(
            old.sections.get(&SectionPos::from_block(b)).unwrap(),
            world.sections.get(&SectionPos::from_block(b)).unwrap()
        ));
    }

    #[test]
    fn raycast_finds_hit_and_adjacent_cell_in_both_directions() {
        let mut world = World::default();
        world.set_block(IVec3::new(-2, 1, 0), Block::Stone);
        let hit = world
            .raycast(Vec3::new(1.5, 1.5, 0.5), Vec3::NEG_X, 5.0)
            .unwrap();
        assert_eq!(hit.position, IVec3::new(-2, 1, 0));
        assert_eq!(hit.adjacent, Some(IVec3::new(-1, 1, 0)));
        assert!(
            world
                .raycast(Vec3::new(1.5, 1.5, 0.5), Vec3::X, 5.0)
                .is_none()
        );
    }

    #[test]
    fn demo_raycast_targets_the_actual_path_and_farmland_outline() {
        for block in [Block::DirtPath, Block::Farmland(0), Block::Farmland(7)] {
            let mut world = World::default();
            world.set_block(IVec3::ZERO, block);
            world.set_block(IVec3::new(0, 0, -1), Block::Stone);
            let hit = world
                .raycast(Vec3::new(0.5, 0.97, 1.5), Vec3::NEG_Z, 4.0)
                .unwrap();
            assert_eq!(hit.position, IVec3::new(0, 0, -1));
            let hit = world
                .raycast(Vec3::new(0.5, 2.0, 0.5), Vec3::NEG_Y, 4.0)
                .unwrap();
            assert_eq!(hit.position, IVec3::ZERO);
            assert_eq!(hit.adjacent, Some(IVec3::Y));
            assert!((hit.distance - 1.0625).abs() < 1.0e-6);
        }
    }
}
