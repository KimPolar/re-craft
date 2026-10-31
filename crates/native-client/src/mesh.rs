use bytemuck::{Pod, Zeroable};
use glam::IVec3;
use native_world::{Block, World};

pub const DROPPED_BASE_TEXTURES: u32 = 6;
pub const DROPPED_SOIL_TEXTURE_START: u32 =
    DROPPED_BASE_TEXTURES + native_core::wood::Wood::TEXTURE_COUNT;
pub const DROPPED_ORE_TEXTURE_START: u32 =
    DROPPED_SOIL_TEXTURE_START + native_resources::DEMO_SOIL_TEXTURES.len() as u32 + 1;
pub const DROPPED_TEXTURE_COUNT: u32 =
    DROPPED_ORE_TEXTURE_START + native_core::ore::Ore::ALL.len() as u32;
pub const DROPPED_ATLAS_COLUMNS: u32 = 8;
pub const DROPPED_ATLAS_ROWS: u32 = (DROPPED_TEXTURE_COUNT + 1).div_ceil(DROPPED_ATLAS_COLUMNS);

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub shade: f32,
    pub tint: [f32; 3],
    /// Packed block and sky levels; u32::MAX leaves non-terrain vertices unlit.
    pub light: u32,
}

struct Face {
    corners: [[f32; 3]; 4],
    shade: f32,
}

const FACES: [Face; 6] = [
    Face {
        corners: [[0., 1., 0.], [0., 1., 1.], [1., 1., 1.], [1., 1., 0.]],
        shade: 1.0,
    },
    Face {
        corners: [[0., 0., 1.], [0., 0., 0.], [1., 0., 0.], [1., 0., 1.]],
        shade: 0.55,
    },
    Face {
        corners: [[1., 0., 0.], [1., 1., 0.], [1., 1., 1.], [1., 0., 1.]],
        shade: 0.75,
    },
    Face {
        corners: [[0., 0., 1.], [0., 1., 1.], [0., 1., 0.], [0., 0., 0.]],
        shade: 0.75,
    },
    Face {
        corners: [[1., 0., 1.], [1., 1., 1.], [0., 1., 1.], [0., 0., 1.]],
        shade: 0.85,
    },
    Face {
        corners: [[0., 0., 0.], [0., 1., 0.], [1., 1., 0.], [1., 0., 0.]],
        shade: 0.85,
    },
];

#[cfg(test)]
const FACE_NORMALS: [IVec3; 6] = [
    IVec3::Y,
    IVec3::NEG_Y,
    IVec3::X,
    IVec3::NEG_X,
    IVec3::Z,
    IVec3::NEG_Z,
];

fn wood_uv_order(
    wood: native_core::wood::Wood,
    axis: native_world::Axis,
    face: usize,
) -> [usize; 4] {
    const ORDERS: [[usize; 4]; 4] = [[0, 3, 2, 1], [1, 0, 3, 2], [3, 2, 1, 0], [2, 1, 0, 3]];
    let pattern = match axis {
        native_world::Axis::Y => [1, 1, 2, 2, 2, 2],
        native_world::Axis::X if wood.uses_locked_model() => [0, 0, 2, 2, 3, 3],
        native_world::Axis::X if wood.uses_horizontal_model() => [0, 0, 2, 2, 3, 0],
        native_world::Axis::X => [0, 0, 1, 2, 3, 0],
        native_world::Axis::Z if wood.uses_locked_model() => [1, 1, 3, 3, 2, 2],
        native_world::Axis::Z if wood.uses_horizontal_model() => [1, 2, 3, 0, 2, 2],
        native_world::Axis::Z => [1, 2, 3, 0, 2, 1],
    };
    ORDERS[pattern[face]]
}

pub fn build(world: &World) -> Vec<Vertex> {
    let mut vertices = Vec::new();
    world.for_each_block_with_neighbors(|pos, block, neighbors| {
        let variant = if matches!(
            block,
            Block::Dirt
                | Block::Sand
                | Block::RedSand
                | Block::Stone
                | Block::RootedDirt
                | Block::Podzol
                | Block::Mycelium
                | Block::DirtPath
                | Block::Grass
        ) {
            let seed = native_resources::blockstates::positional_seed(pos.x, pos.y, pos.z);
            native_core::random::LegacyRandom::new(seed)
                .next_int_bound(4)
                .unwrap_or(0) as usize
        } else {
            0
        };
        let rotation = if block == Block::Stone {
            (variant / 2) * 2
        } else {
            variant
        };
        let mirrored = block == Block::Stone && variant % 2 == 1;
        for (face_index, face) in FACES.iter().enumerate() {
            let indented = matches!(block, Block::DirtPath | Block::Farmland(_));
            let neighbor = neighbors[face_index];
            let neighbor_indented = matches!(neighbor, Block::DirtPath | Block::Farmland(_));
            if neighbor.is_solid()
                && !(indented && face_index == 0)
                && !(neighbor_indented && (face_index == 1 || !indented))
            {
                continue;
            }
            let tile = match block {
                Block::Grass if face_index == 0 => 0,
                Block::Grass if face_index == 1 => 2,
                Block::Grass => 1,
                Block::Dirt => 2,
                Block::Sand => native_resources::DEMO_SOIL_TILE_START + 10,
                Block::RedSand => native_resources::DEMO_SOIL_TILE_START + 11,
                Block::Gravel => native_resources::DEMO_SOIL_TILE_START + 12,
                Block::CoarseDirt => native_resources::DEMO_SOIL_TILE_START,
                Block::RootedDirt => native_resources::DEMO_SOIL_TILE_START + 1,
                Block::Podzol if face_index == 0 => native_resources::DEMO_SOIL_TILE_START + 2,
                Block::Podzol if face_index == 1 => 2,
                Block::Podzol => native_resources::DEMO_SOIL_TILE_START + 3,
                Block::Mycelium if face_index == 0 => native_resources::DEMO_SOIL_TILE_START + 4,
                Block::Mycelium if face_index == 1 => 2,
                Block::Mycelium => native_resources::DEMO_SOIL_TILE_START + 5,
                Block::DirtPath if face_index == 0 => native_resources::DEMO_SOIL_TILE_START + 6,
                Block::DirtPath if face_index == 1 => 2,
                Block::DirtPath => native_resources::DEMO_SOIL_TILE_START + 7,
                Block::Farmland(moisture) if face_index == 0 => {
                    native_resources::DEMO_SOIL_TILE_START + if moisture == 7 { 9 } else { 8 }
                }
                Block::Farmland(_) => 2,
                Block::Stone => 3,
                Block::StoneBricks => 4,
                Block::Cobblestone => native_resources::DEMO_COBBLESTONE,
                Block::Ore(ore) => native_resources::DEMO_ORE_TILE_START + ore.index(),
                Block::Wood(wood, axis) => {
                    let end_face = !wood.bark_all_sides()
                        && match axis {
                            native_world::Axis::X => matches!(face_index, 2 | 3),
                            native_world::Axis::Y => matches!(face_index, 0 | 1),
                            native_world::Axis::Z => matches!(face_index, 4 | 5),
                        };
                    native_resources::DEMO_WOOD_TILE_START
                        + wood.texture_index()
                        + u32::from(end_face)
                }
                Block::Planks(kind) => {
                    native_resources::DEMO_PLANKS_TILE_START + kind.index() as u32
                }
                Block::CraftingTable if face_index == 0 => {
                    native_resources::DEMO_CRAFTING_TABLE_TOP
                }
                Block::CraftingTable if face_index == 1 => native_resources::DEMO_PLANKS_TILE_START,
                Block::CraftingTable if face_index == 3 || face_index == 5 => {
                    native_resources::DEMO_CRAFTING_TABLE_FRONT
                }
                Block::CraftingTable => native_resources::DEMO_CRAFTING_TABLE_SIDE,
                Block::Air => continue,
            };
            let column = tile % native_resources::DEMO_ATLAS_COLUMNS;
            let row = tile / native_resources::DEMO_ATLAS_COLUMNS;
            let left = column as f32 / native_resources::DEMO_ATLAS_COLUMNS as f32 + 0.000_01;
            let right =
                (column + 1) as f32 / native_resources::DEMO_ATLAS_COLUMNS as f32 - 0.000_01;
            let mut top = row as f32 / native_resources::DEMO_ATLAS_ROWS as f32 + 0.000_01;
            let bottom = (row + 1) as f32 / native_resources::DEMO_ATLAS_ROWS as f32 - 0.000_01;
            if indented && face_index >= 2 {
                top += 1.0 / (16.0 * native_resources::DEMO_ATLAS_ROWS as f32);
            }
            let uv = [[left, bottom], [left, top], [right, top], [right, bottom]];
            let order = if let Block::Wood(wood, axis) = block {
                wood_uv_order(wood, axis, face_index)
            } else if face_index <= 1 {
                [1, 0, 3, 2]
            } else {
                [3, 2, 1, 0]
            };
            let order = if face_index == 0 && rotation != 0 {
                std::array::from_fn(|index| order[(index + rotation) % 4])
            } else if face_index == 1 && rotation != 0 {
                std::array::from_fn(|index| order[(index + 4 - rotation) % 4])
            } else {
                order
            };
            let uv = std::array::from_fn::<_, 4, _>(|index| {
                let mut coords = uv[order[index]];
                if mirrored {
                    coords[0] = left + right - coords[0];
                }
                coords
            });
            for corner in [0, 1, 2, 0, 2, 3] {
                let p = face.corners[corner];
                vertices.push(Vertex {
                    position: [
                        pos.x as f32 + p[0],
                        pos.y as f32 + p[1] * if indented { 15.0 / 16.0 } else { 1.0 },
                        pos.z as f32 + p[2],
                    ],
                    uv: uv[corner],
                    shade: face.shade,
                    tint: [1.0; 3],
                    light: u32::MAX,
                });
            }
        }
    });
    vertices
}

/// Full-cube breaking decal using a five-column, two-row stage atlas.
pub fn breaking_cube(position: IVec3, origin: IVec3, stage: u8) -> Vec<Vertex> {
    let stage = stage.min(9);
    let column = f32::from(stage % 5);
    let row = f32::from(stage / 5);
    let left = column / 5.0 + 0.000_01;
    let right = (column + 1.0) / 5.0 - 0.000_01;
    let top = row / 2.0 + 0.000_01;
    let bottom = (row + 1.0) / 2.0 - 0.000_01;
    let uv = [[left, bottom], [left, top], [right, top], [right, bottom]];
    let base = [
        (i64::from(position.x) - i64::from(origin.x)) as f32,
        (i64::from(position.y) - i64::from(origin.y)) as f32,
        (i64::from(position.z) - i64::from(origin.z)) as f32,
    ];
    let mut vertices = Vec::with_capacity(36);
    for face in FACES {
        for corner in [0, 1, 2, 0, 2, 3] {
            let point = face.corners[corner];
            vertices.push(Vertex {
                position: [base[0] + point[0], base[1] + point[1], base[2] + point[2]],
                uv: uv[corner],
                shade: 1.0,
                tint: [1.0; 3],
                light: u32::MAX,
            });
        }
    }
    vertices
}

/// A temporary cube representation of basic dropped block items.
pub fn dropped_items<'a>(
    items: impl IntoIterator<Item = &'a native_session::items::DroppedItem>,
    origin: IVec3,
    seconds: f32,
) -> Vec<Vertex> {
    let items = items.into_iter();
    let mut vertices = Vec::with_capacity(items.size_hint().0.saturating_mul(36));
    for item in items {
        let wood = native_core::wood::Wood::from_id(item.stack.id());
        let ore = native_core::ore::Ore::from_id(item.stack.id());
        let base = (item.position - origin.as_dvec3()).as_vec3();
        let partial_tick = (seconds * 20.0).fract();
        let phase = (item.age as f32 + partial_tick) / 10.0;
        let angle = (item.age as f32 + partial_tick) / 20.0;
        let (sin, cos) = angle.sin_cos();
        let bob = phase.sin() * 0.1 + 0.1;
        for (face_index, face) in FACES.iter().enumerate() {
            let tile = match item.stack.id() {
                "minecraft:grass_block" if face_index == 0 => 0,
                "minecraft:grass_block" if face_index == 1 => 2,
                "minecraft:grass_block" => 1,
                "minecraft:dirt" => 2,
                "minecraft:stone" | "minecraft:stone_button" => 3,
                "minecraft:stone_bricks" => 4,
                "minecraft:cobblestone" => 5,
                "minecraft:coarse_dirt" => DROPPED_SOIL_TEXTURE_START,
                "minecraft:rooted_dirt" => DROPPED_SOIL_TEXTURE_START + 1,
                "minecraft:podzol" if face_index == 0 => DROPPED_SOIL_TEXTURE_START + 2,
                "minecraft:podzol" if face_index == 1 => 2,
                "minecraft:podzol" => DROPPED_SOIL_TEXTURE_START + 3,
                "minecraft:mycelium" if face_index == 0 => DROPPED_SOIL_TEXTURE_START + 4,
                "minecraft:mycelium" if face_index == 1 => 2,
                "minecraft:mycelium" => DROPPED_SOIL_TEXTURE_START + 5,
                "minecraft:dirt_path" if face_index == 0 => DROPPED_SOIL_TEXTURE_START + 6,
                "minecraft:dirt_path" if face_index == 1 => 2,
                "minecraft:dirt_path" => DROPPED_SOIL_TEXTURE_START + 7,
                "minecraft:farmland" if face_index == 0 => DROPPED_SOIL_TEXTURE_START + 8,
                "minecraft:farmland" => 2,
                "minecraft:sand" => DROPPED_SOIL_TEXTURE_START + 10,
                "minecraft:red_sand" => DROPPED_SOIL_TEXTURE_START + 11,
                "minecraft:gravel" => DROPPED_SOIL_TEXTURE_START + 12,
                "minecraft:hanging_roots" => {
                    DROPPED_SOIL_TEXTURE_START + native_resources::DEMO_SOIL_TEXTURES.len() as u32
                }
                _ if ore.is_some() => DROPPED_ORE_TEXTURE_START + ore.unwrap().index(),
                _ => wood.map_or(DROPPED_TEXTURE_COUNT, |wood| {
                    let end_face = !wood.bark_all_sides() && matches!(face_index, 0 | 1);
                    DROPPED_BASE_TEXTURES + wood.texture_index() + u32::from(end_face)
                }),
            };
            let column = tile % DROPPED_ATLAS_COLUMNS;
            let row = tile / DROPPED_ATLAS_COLUMNS;
            let left = column as f32 / DROPPED_ATLAS_COLUMNS as f32 + 0.000_01;
            let right = (column + 1) as f32 / DROPPED_ATLAS_COLUMNS as f32 - 0.000_01;
            let mut top = row as f32 / DROPPED_ATLAS_ROWS as f32 + 0.000_01;
            let bottom = (row + 1) as f32 / DROPPED_ATLAS_ROWS as f32 - 0.000_01;
            let indented = matches!(
                item.stack.id(),
                "minecraft:dirt_path" | "minecraft:farmland"
            );
            if indented && face_index >= 2 {
                top += 1.0 / (16.0 * DROPPED_ATLAS_ROWS as f32);
            }
            let uv = [[left, bottom], [left, top], [right, top], [right, bottom]];
            let order = if face_index <= 1 {
                [1, 0, 3, 2]
            } else {
                [3, 2, 1, 0]
            };
            let uv = std::array::from_fn::<_, 4, _>(|index| uv[order[index]]);
            for corner in [0, 1, 2, 0, 2, 3] {
                let p = face.corners[corner];
                let x = (p[0] - 0.5) * 0.25;
                let z = (p[2] - 0.5)
                    * 0.25
                    * if item.stack.id() == "minecraft:hanging_roots" {
                        1.0 / 16.0
                    } else {
                        1.0
                    };
                vertices.push(Vertex {
                    position: [
                        base.x + x * cos - z * sin,
                        base.y + p[1] * 0.25 * if indented { 15.0 / 16.0 } else { 1.0 } + bob,
                        base.z + x * sin + z * cos,
                    ],
                    uv: uv[corner],
                    shade: face.shade,
                    tint: if item.stack.id() == "minecraft:grass_block" && face_index == 0 {
                        [0.49, 0.72, 0.26]
                    } else {
                        [1.0; 3]
                    },
                    light: u32::MAX,
                });
            }
        }
    }
    vertices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropped_sand_and_roots_use_distinct_atlas_tiles() {
        for (name, offset) in [
            ("minecraft:sand", 10),
            ("minecraft:red_sand", 11),
            ("minecraft:gravel", 12),
            (
                "minecraft:hanging_roots",
                native_resources::DEMO_SOIL_TEXTURES.len() as u32,
            ),
        ] {
            let item = native_session::items::DroppedItem {
                entity_id: 0,
                item_age: 0,
                position: glam::DVec3::ZERO,
                previous_position: glam::DVec3::ZERO,
                stack: native_session::inventory::ItemStack::new(
                    name.into(),
                    1,
                    64,
                    native_persistence::nbt::Tag::Compound(vec![]),
                )
                .unwrap(),
                age: 0,
                bob_offset: 0.0,
            };
            let tile = DROPPED_SOIL_TEXTURE_START + offset;
            for vertex in dropped_items(&[item], IVec3::ZERO, 0.0) {
                let col = (vertex.uv[0] * DROPPED_ATLAS_COLUMNS as f32) as u32;
                let row = (vertex.uv[1] * DROPPED_ATLAS_ROWS as f32) as u32;
                assert_eq!(row * DROPPED_ATLAS_COLUMNS + col, tile, "{name}");
            }
        }
    }

    #[test]
    fn ore_blocks_and_drops_use_their_own_textures() {
        for ore in native_core::ore::Ore::ALL {
            let item = native_session::items::DroppedItem {
                entity_id: 0,
                item_age: 0,
                position: glam::DVec3::ZERO,
                previous_position: glam::DVec3::ZERO,
                stack: native_session::inventory::ItemStack::new(
                    ore.id().into(),
                    1,
                    64,
                    native_persistence::nbt::Tag::Compound(vec![]),
                )
                .unwrap(),
                age: 0,
                bob_offset: 0.0,
            };
            for vertex in dropped_items(&[item], IVec3::ZERO, 0.0) {
                let col = (vertex.uv[0] * DROPPED_ATLAS_COLUMNS as f32) as u32;
                let row = (vertex.uv[1] * DROPPED_ATLAS_ROWS as f32) as u32;
                assert_eq!(
                    row * DROPPED_ATLAS_COLUMNS + col,
                    DROPPED_ORE_TEXTURE_START + ore.index(),
                    "{}",
                    ore.id()
                );
            }
            let mut world = World::default();
            world.set_block(IVec3::ZERO, Block::Ore(ore));
            for vertex in build(&world) {
                let col = (vertex.uv[0] * native_resources::DEMO_ATLAS_COLUMNS as f32) as u32;
                let row = (vertex.uv[1] * native_resources::DEMO_ATLAS_ROWS as f32) as u32;
                assert_eq!(
                    row * native_resources::DEMO_ATLAS_COLUMNS + col,
                    native_resources::DEMO_ORE_TILE_START + ore.index(),
                    "{}",
                    ore.id()
                );
            }
        }
    }

    #[test]
    #[ignore = "CPU measurement of the temporary demo mesh"]
    fn measure_demo_mesh_build_cost() {
        use std::time::Instant;
        let world = World::demo();
        let expected = build(&world).len();
        for _ in 0..3 {
            std::hint::black_box(build(&world));
        }
        let mut times = Vec::new();
        for _ in 0..25 {
            let start = Instant::now();
            let mesh = std::hint::black_box(build(&world));
            times.push(start.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(mesh.len(), expected);
        }
        times.sort_by(f64::total_cmp);
        eprintln!(
            "Demo mesh: {expected} vertices; CPU median {:.3} ms, p95 {:.3} ms, max {:.3} ms",
            times[12], times[23], times[24]
        );
    }

    #[test]
    fn shared_faces_are_removed_across_section_boundaries() {
        let mut world = World::default();
        world.set_block(IVec3::new(15, 0, 0), Block::Stone);
        assert_eq!(build(&world).len(), 36);
        world.set_block(IVec3::new(16, 0, 0), Block::Stone);
        assert_eq!(build(&world).len(), 60);
    }

    #[test]
    fn stone_bricks_use_their_own_atlas_tile_on_every_face() {
        let mut world = World::default();
        world.set_block(IVec3::ZERO, Block::StoneBricks);
        let vertices = build(&world);
        assert_eq!(vertices.len(), 36);
        assert!(vertices.iter().all(|vertex| vertex.uv[0] > 0.5
            && vertex.uv[0] < 0.625
            && vertex.uv[1] < 1.0 / native_resources::DEMO_ATLAS_ROWS as f32));
    }

    #[test]
    fn cobblestone_uses_its_own_atlas_tile_on_every_face() {
        let mut world = World::default();
        world.set_block(IVec3::ZERO, Block::Cobblestone);
        let vertices = build(&world);
        assert_eq!(vertices.len(), 36);
        let tile = native_resources::DEMO_COBBLESTONE;
        let columns = native_resources::DEMO_ATLAS_COLUMNS as f32;
        let rows = native_resources::DEMO_ATLAS_ROWS as f32;
        let column = (tile % native_resources::DEMO_ATLAS_COLUMNS) as f32;
        let row = (tile / native_resources::DEMO_ATLAS_COLUMNS) as f32;
        assert!(vertices.iter().all(|vertex| {
            vertex.uv[0] >= column / columns
                && vertex.uv[0] <= (column + 1.0) / columns
                && vertex.uv[1] >= row / rows
                && vertex.uv[1] <= (row + 1.0) / rows
        }));
    }

    #[test]
    fn crafting_table_uses_vanilla_texture_for_each_face() {
        let mut world = World::default();
        world.set_block(IVec3::ZERO, Block::CraftingTable);
        let vertices = build(&world);
        let expected = [
            native_resources::DEMO_CRAFTING_TABLE_TOP,
            native_resources::DEMO_PLANKS_TILE_START,
            native_resources::DEMO_CRAFTING_TABLE_SIDE,
            native_resources::DEMO_CRAFTING_TABLE_FRONT,
            native_resources::DEMO_CRAFTING_TABLE_SIDE,
            native_resources::DEMO_CRAFTING_TABLE_FRONT,
        ];
        for (face, tile) in vertices.as_chunks::<6>().0.iter().zip(expected) {
            for vertex in face {
                let col = (vertex.uv[0] * native_resources::DEMO_ATLAS_COLUMNS as f32) as u32;
                let row = (vertex.uv[1] * native_resources::DEMO_ATLAS_ROWS as f32) as u32;
                assert_eq!(row * native_resources::DEMO_ATLAS_COLUMNS + col, tile);
            }
        }
    }

    #[test]
    fn wood_faces_select_the_correct_side_and_end_tiles() {
        use native_core::wood::Wood;
        use native_world::Axis;
        for wood in Wood::all() {
            for axis in [Axis::X, Axis::Y, Axis::Z] {
                let mut world = World::default();
                world.set_block(IVec3::ZERO, Block::Wood(wood, axis));
                let vertices = build(&world);
                for (index, face) in vertices.as_chunks::<6>().0.iter().enumerate() {
                    let end = !wood.bark_all_sides()
                        && match axis {
                            Axis::X => matches!(index, 2 | 3),
                            Axis::Y => matches!(index, 0 | 1),
                            Axis::Z => matches!(index, 4 | 5),
                        };
                    let expected = native_resources::DEMO_WOOD_TILE_START
                        + wood.texture_index()
                        + u32::from(end);
                    for vertex in face {
                        let col =
                            (vertex.uv[0] * native_resources::DEMO_ATLAS_COLUMNS as f32) as u32;
                        let row = (vertex.uv[1] * native_resources::DEMO_ATLAS_ROWS as f32) as u32;
                        assert_eq!(
                            row * native_resources::DEMO_ATLAS_COLUMNS + col,
                            expected,
                            "{} {axis:?}",
                            wood.id()
                        );
                    }
                }
            }
            let item = native_session::items::DroppedItem {
                entity_id: 0,
                item_age: 0,
                position: glam::DVec3::ZERO,
                previous_position: glam::DVec3::ZERO,
                stack: native_session::inventory::ItemStack::new(
                    wood.id().into(),
                    1,
                    64,
                    native_persistence::nbt::Tag::Compound(vec![]),
                )
                .unwrap(),
                age: 0,
                bob_offset: 0.0,
            };
            let vertices = dropped_items(&[item], IVec3::ZERO, 0.0);
            for (index, face) in vertices.as_chunks::<6>().0.iter().enumerate() {
                let end = !wood.bark_all_sides() && matches!(index, 0 | 1);
                let expected = DROPPED_BASE_TEXTURES + wood.texture_index() + u32::from(end);
                for vertex in face {
                    let col = (vertex.uv[0] * DROPPED_ATLAS_COLUMNS as f32) as u32;
                    let row = (vertex.uv[1] * DROPPED_ATLAS_ROWS as f32) as u32;
                    assert_eq!(row * DROPPED_ATLAS_COLUMNS + col, expected, "{}", wood.id());
                }
            }
        }
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn wood_textures_match_the_installed_vanilla_models() {
        use native_resources::packs::{ResourceProvider, ResourceStack};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut source = ResourceStack::open(&game).unwrap();
        let names: Vec<_> = native_core::wood::Wood::texture_names().collect();
        for wood in native_core::wood::Wood::all() {
            let id = wood.id().strip_prefix("minecraft:").unwrap();
            let path = format!("assets/minecraft/models/block/{id}.json");
            let bytes = source.read_asset(&path).unwrap().unwrap();
            let model: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let side = &names[wood.texture_index() as usize];
            let end = if wood.bark_all_sides() {
                side
            } else {
                &names[wood.texture_index() as usize + 1]
            };
            assert_eq!(
                model["textures"]["side"].as_str(),
                Some(format!("minecraft:block/{}", side.strip_suffix(".png").unwrap()).as_str()),
                "{id}"
            );
            assert_eq!(
                model["textures"]["end"].as_str(),
                Some(format!("minecraft:block/{}", end.strip_suffix(".png").unwrap()).as_str()),
                "{id}"
            );
        }
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn wood_demo_uvs_match_installed_blockstate_models() {
        use native_resources::{geometry, packs::ResourceStack};
        use native_world::Axis;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut source = ResourceStack::open(&game).unwrap();
        for wood in native_core::wood::Wood::all() {
            for axis in [Axis::X, Axis::Y, Axis::Z] {
                let properties =
                    std::collections::BTreeMap::from([("axis".into(), axis.name().into())]);
                let baked = geometry::bake_blockstate(&mut source, wood.id(), &properties).unwrap();
                assert_eq!(baked.groups.len(), 1);
                let model = &baked.groups[0][0].model;
                assert_eq!(model.quads.len(), 6, "{} {axis:?}", wood.id());
                let mut world = World::default();
                world.set_block(IVec3::ZERO, Block::Wood(wood, axis));
                let vertices = build(&world);
                for (index, face) in vertices.as_chunks::<6>().0.iter().enumerate() {
                    let normal = FACE_NORMALS[index].as_vec3();
                    let quad = model
                        .quads
                        .iter()
                        .find(|quad| glam::Vec3::from_array(quad.direction.normal()) == normal)
                        .unwrap();
                    for vertex in face {
                        let model_vertex = quad
                            .vertices
                            .iter()
                            .find(|candidate| {
                                glam::Vec3::from_array(candidate.position)
                                    .distance(glam::Vec3::from_array(vertex.position))
                                    < 0.0001
                            })
                            .unwrap();
                        let col =
                            (vertex.uv[0] * native_resources::DEMO_ATLAS_COLUMNS as f32).floor();
                        let row = (vertex.uv[1] * native_resources::DEMO_ATLAS_ROWS as f32).floor();
                        let uv = [
                            vertex.uv[0] * native_resources::DEMO_ATLAS_COLUMNS as f32 - col,
                            vertex.uv[1] * native_resources::DEMO_ATLAS_ROWS as f32 - row,
                        ];
                        for (actual, expected) in uv.into_iter().zip(model_vertex.uv) {
                            assert!(
                                (actual - expected).abs() < 0.001,
                                "{} {axis:?} face {index} at {:?}: got {:?}, expected {:?}",
                                wood.id(),
                                vertex.position,
                                uv,
                                model_vertex.uv
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn soil_demo_geometry_and_textures_match_vanilla_models() {
        use native_resources::{geometry, packs::ResourceStack};
        use std::collections::BTreeMap;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut source = ResourceStack::open(&game).unwrap();
        for (block, properties) in [
            (Block::Dirt, BTreeMap::new()),
            (Block::Sand, BTreeMap::new()),
            (Block::RedSand, BTreeMap::new()),
            (Block::Gravel, BTreeMap::new()),
            (Block::Stone, BTreeMap::new()),
            (Block::StoneBricks, BTreeMap::new()),
            (Block::Cobblestone, BTreeMap::new()),
            (
                Block::Planks(native_core::wood::Planks::Oak),
                BTreeMap::new(),
            ),
            (Block::CraftingTable, BTreeMap::new()),
            (Block::CoarseDirt, BTreeMap::new()),
            (Block::RootedDirt, BTreeMap::new()),
            (
                Block::Podzol,
                BTreeMap::from([("snowy".into(), "false".into())]),
            ),
            (
                Block::Mycelium,
                BTreeMap::from([("snowy".into(), "false".into())]),
            ),
            (Block::DirtPath, BTreeMap::new()),
            (
                Block::Farmland(0),
                BTreeMap::from([("moisture".into(), "0".into())]),
            ),
            (
                Block::Farmland(7),
                BTreeMap::from([("moisture".into(), "7".into())]),
            ),
        ]
        .into_iter()
        .chain(
            native_core::ore::Ore::ALL
                .into_iter()
                .map(|ore| (Block::Ore(ore), BTreeMap::new())),
        ) {
            let id = block.item_id().unwrap();
            let baked = geometry::bake_blockstate(&mut source, id, &properties).unwrap();
            let positions: Vec<_> = if matches!(
                block,
                Block::Dirt
                    | Block::Sand
                    | Block::RedSand
                    | Block::Stone
                    | Block::RootedDirt
                    | Block::Podzol
                    | Block::Mycelium
                    | Block::DirtPath
            ) {
                (-8..8).map(|x| IVec3::new(x, 0, 0)).collect()
            } else {
                vec![IVec3::ZERO]
            };
            for position in positions {
                let selected = baked
                    .definition
                    .select_indices(native_resources::blockstates::positional_seed(
                        position.x, position.y, position.z,
                    ))
                    .unwrap()[0];
                let model = &baked.groups[0][selected].model;
                let mut world = World::default();
                world.set_block(position, block);
                let vertices = build(&world);
                assert_eq!(vertices.len(), 36, "{block:?}");
                for (index, face) in vertices.as_chunks::<6>().0.iter().enumerate() {
                    let normal = FACE_NORMALS[index].as_vec3();
                    let quad = model
                        .quads
                        .iter()
                        .find(|quad| glam::Vec3::from_array(quad.direction.normal()) == normal)
                        .unwrap();
                    for vertex in face {
                        let model_vertex = quad
                            .vertices
                            .iter()
                            .find(|candidate| {
                                glam::Vec3::from_array(candidate.position).distance(
                                    glam::Vec3::from_array(vertex.position) - position.as_vec3(),
                                ) < 0.0001
                            })
                            .unwrap_or_else(|| {
                                panic!("{block:?} face {index}: position {:?}", vertex.position)
                            });
                        let col =
                            (vertex.uv[0] * native_resources::DEMO_ATLAS_COLUMNS as f32).floor();
                        let row = (vertex.uv[1] * native_resources::DEMO_ATLAS_ROWS as f32).floor();
                        let tile = row as u32 * native_resources::DEMO_ATLAS_COLUMNS + col as u32;
                        let texture = match tile {
                            2 => "dirt.png",
                            3 => "stone.png",
                            4 => "stone_bricks.png",
                            native_resources::DEMO_COBBLESTONE => "cobblestone.png",
                            native_resources::DEMO_PLANKS_TILE_START => "oak_planks.png",
                            native_resources::DEMO_CRAFTING_TABLE_TOP => "crafting_table_top.png",
                            native_resources::DEMO_CRAFTING_TABLE_FRONT => {
                                "crafting_table_front.png"
                            }
                            native_resources::DEMO_CRAFTING_TABLE_SIDE => "crafting_table_side.png",
                            tile if tile >= native_resources::DEMO_ORE_TILE_START => {
                                native_core::ore::Ore::ALL
                                    [(tile - native_resources::DEMO_ORE_TILE_START) as usize]
                                    .id()
                                    .strip_prefix("minecraft:")
                                    .unwrap()
                            }
                            _ => {
                                native_resources::DEMO_SOIL_TEXTURES
                                    [(tile - native_resources::DEMO_SOIL_TILE_START) as usize]
                            }
                        };
                        assert_eq!(
                            quad.texture.sprite.path,
                            format!("block/{}", texture.strip_suffix(".png").unwrap_or(texture)),
                            "{block:?} face {index}"
                        );
                        let uv = [
                            vertex.uv[0] * native_resources::DEMO_ATLAS_COLUMNS as f32 - col,
                            vertex.uv[1] * native_resources::DEMO_ATLAS_ROWS as f32 - row,
                        ];
                        for (actual, expected) in uv.into_iter().zip(model_vertex.uv) {
                            assert!(
                                (actual - expected).abs() < 0.001,
                                "{block:?} face {index} at {:?}: got {:?}, expected {:?}",
                                vertex.position,
                                uv,
                                model_vertex.uv
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn breaking_vertices_follow_absolute_position_and_stage_atlas() {
        let vertices = breaking_cube(
            IVec3::new(1_000_001, 5, -1_000_001),
            IVec3::new(1_000_000, 0, -1_000_000),
            7,
        );
        assert_eq!(vertices.len(), 36);
        assert!(vertices.iter().all(|v| (1.0..=2.0).contains(&v.position[0])
            && (5.0..=6.0).contains(&v.position[1])
            && (-1.0..=0.0).contains(&v.position[2])));
        assert!(
            vertices
                .iter()
                .all(|v| v.uv[0] > 0.4 && v.uv[0] < 0.6 && v.uv[1] > 0.5 && v.uv[1] < 1.0)
        );
    }
}
