//! Fluid surfaces for the read-only viewer; block occlusion is supplied by the caller.

use glam::IVec3;
use native_resources::geometry::Direction;
use native_world::fluid::{self, Cell, FluidKind, FluidState};

const OFFSET: f32 = 0.001;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Texture {
    Still,
    Flow,
    Overlay,
}

#[derive(Clone, Copy, Debug)]
pub struct FluidQuad {
    pub direction: Direction,
    pub texture: Texture,
    pub positions: [[f32; 3]; 4],
    pub uv: [[f32; 2]; 4],
}

fn neighbor(position: IVec3, direction: Direction) -> Option<IVec3> {
    let (dx, dy, dz) = match direction {
        Direction::Down => (0, -1, 0),
        Direction::Up => (0, 1, 0),
        Direction::North => (0, 0, -1),
        Direction::South => (0, 0, 1),
        Direction::West => (-1, 0, 0),
        Direction::East => (1, 0, 0),
    };
    Some(IVec3::new(
        position.x.checked_add(dx)?,
        position.y.checked_add(dy)?,
        position.z.checked_add(dz)?,
    ))
}

fn face_index(direction: Direction) -> usize {
    match direction {
        Direction::Down => 0,
        Direction::Up => 1,
        Direction::North => 2,
        Direction::South => 3,
        Direction::West => 4,
        Direction::East => 5,
    }
}

/// Produces local coordinates; no face is suppressed for an unknown neighboring chunk.
pub fn build(
    world: &impl Fn(IVec3) -> Cell,
    overlay: &impl Fn(IVec3) -> bool,
    position: IVec3,
    state: FluidState,
    self_occlusion: [bool; 6],
) -> Vec<FluidQuad> {
    let cell = |direction| {
        neighbor(position, direction)
            .map(world)
            .unwrap_or(Cell::Missing)
    };
    let visible = |direction| {
        !self_occlusion[face_index(direction)]
            && !fluid::same_fluid(cell(direction), state.kind)
            && cell(direction) != Cell::Solid
    };
    let bottom = visible(Direction::Down);
    let mut heights = fluid::surface_heights(world, position, state.kind);
    let above = cell(Direction::Up);
    let top = !fluid::same_fluid(above, state.kind)
        && !(above == Cell::Solid && heights.iter().all(|height| *height >= 1.0));
    let mut result = Vec::with_capacity(6);
    if top {
        heights.iter_mut().for_each(|height| *height -= OFFSET);
        let flow = fluid::horizontal_flow(world, position, state);
        let (texture, uv) = if flow == [0.0, 0.0] {
            (
                Texture::Still,
                [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]],
            )
        } else {
            let angle = flow[1].atan2(flow[0]) as f32 - std::f32::consts::FRAC_PI_2;
            let sin = angle.sin() * 0.25;
            let cos = angle.cos() * 0.25;
            (
                Texture::Flow,
                [
                    [0.5 - cos - sin, 0.5 - cos + sin],
                    [0.5 - cos + sin, 0.5 + cos + sin],
                    [0.5 + cos + sin, 0.5 + cos - sin],
                    [0.5 + cos - sin, 0.5 - cos - sin],
                ],
            )
        };
        result.push(FluidQuad {
            direction: Direction::Up,
            texture,
            positions: [
                [0.0, heights[0], 0.0],
                [0.0, heights[1], 1.0],
                [1.0, heights[2], 1.0],
                [1.0, heights[3], 0.0],
            ],
            uv,
        });
    }
    let bottom_y = if bottom { OFFSET } else { 0.0 };
    if bottom {
        result.push(FluidQuad {
            direction: Direction::Down,
            texture: Texture::Still,
            positions: [
                [0.0, bottom_y, 0.0],
                [1.0, bottom_y, 0.0],
                [1.0, bottom_y, 1.0],
                [0.0, bottom_y, 1.0],
            ],
            uv: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        });
    }
    for direction in [
        Direction::North,
        Direction::South,
        Direction::West,
        Direction::East,
    ] {
        if !visible(direction) {
            continue;
        }
        let (first, second, x0, z0, x1, z1) = match direction {
            Direction::North => (heights[0], heights[3], 0.0, OFFSET, 1.0, OFFSET),
            Direction::South => (heights[2], heights[1], 1.0, 1.0 - OFFSET, 0.0, 1.0 - OFFSET),
            Direction::West => (heights[1], heights[0], OFFSET, 1.0, OFFSET, 0.0),
            Direction::East => (heights[3], heights[2], 1.0 - OFFSET, 0.0, 1.0 - OFFSET, 1.0),
            _ => unreachable!(),
        };
        let texture = if state.kind == FluidKind::Water
            && neighbor(position, direction).is_some_and(overlay)
        {
            Texture::Overlay
        } else {
            Texture::Flow
        };
        result.push(FluidQuad {
            direction,
            texture,
            positions: [
                [x0, first, z0],
                [x1, second, z1],
                [x1, bottom_y, z1],
                [x0, bottom_y, z0],
            ],
            uv: [
                [0.0, (1.0 - first) * 0.5],
                [0.5, (1.0 - second) * 0.5],
                [0.5, 0.5],
                [0.0, 0.5],
            ],
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> FluidState {
        FluidState {
            kind: FluidKind::Water,
            amount: 8,
            source: true,
            falling: false,
        }
    }

    #[test]
    fn still_fluid_has_a_sloped_surface_and_adjacent_water_hides_shared_faces() {
        let world = |p: IVec3| {
            if p == IVec3::ZERO || p == IVec3::X {
                Cell::Fluid(source())
            } else {
                Cell::Empty
            }
        };
        let quads = build(&world, &|_| false, IVec3::ZERO, source(), [false; 6]);
        assert_eq!(quads.len(), 5);
        assert!(!quads.iter().any(|quad| quad.direction == Direction::East));
        let top = quads
            .iter()
            .find(|quad| quad.direction == Direction::Up)
            .unwrap();
        assert!(top.positions[2][1] > top.positions[0][1]);
    }

    #[test]
    fn water_overlay_is_used_against_glass_and_a_solid_hides_its_face() {
        let world = |p: IVec3| {
            if p == IVec3::ZERO {
                Cell::Fluid(source())
            } else if p == IVec3::NEG_Z {
                Cell::Solid
            } else {
                Cell::Empty
            }
        };
        let quads = build(
            &world,
            &|p| p == IVec3::X,
            IVec3::ZERO,
            source(),
            [false; 6],
        );
        assert!(!quads.iter().any(|quad| quad.direction == Direction::North));
        assert_eq!(
            quads
                .iter()
                .find(|quad| quad.direction == Direction::East)
                .unwrap()
                .texture,
            Texture::Overlay
        );
    }
}
