//! Cuboid model geometry. Unsupported transforms are errors rather than approximations.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail, ensure};
use glam::{Mat3, Vec3};
use serde_json::Value;

use crate::{
    blockstates::{self, ModelChoice},
    models::{self, ResolvedModel, TextureReference},
    packs::ResourceProvider,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl Direction {
    pub const ALL: [Self; 6] = [
        Self::Down,
        Self::Up,
        Self::North,
        Self::South,
        Self::West,
        Self::East,
    ];

    fn parse(value: &str) -> Result<Self> {
        match value {
            "down" => Ok(Self::Down),
            "up" => Ok(Self::Up),
            "north" => Ok(Self::North),
            "south" => Ok(Self::South),
            "west" => Ok(Self::West),
            "east" => Ok(Self::East),
            _ => bail!("unknown face direction {value}"),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Down => "down",
            Self::Up => "up",
            Self::North => "north",
            Self::South => "south",
            Self::West => "west",
            Self::East => "east",
        }
    }

    fn corners(self, a: [f32; 3], b: [f32; 3]) -> [[f32; 3]; 4] {
        let [x0, y0, z0] = a;
        let [x1, y1, z1] = b;
        match self {
            Self::Down => [[x0, y0, z1], [x0, y0, z0], [x1, y0, z0], [x1, y0, z1]],
            Self::Up => [[x0, y1, z0], [x0, y1, z1], [x1, y1, z1], [x1, y1, z0]],
            Self::North => [[x1, y1, z0], [x1, y0, z0], [x0, y0, z0], [x0, y1, z0]],
            Self::South => [[x0, y1, z1], [x0, y0, z1], [x1, y0, z1], [x1, y1, z1]],
            Self::West => [[x0, y1, z0], [x0, y0, z0], [x0, y0, z1], [x0, y1, z1]],
            Self::East => [[x1, y1, z1], [x1, y0, z1], [x1, y0, z0], [x1, y1, z0]],
        }
    }

    fn default_uv(self, a: [f32; 3], b: [f32; 3]) -> [f32; 4] {
        match self {
            Self::Down => [a[0], 16.0 - b[2], b[0], 16.0 - a[2]],
            Self::Up => [a[0], a[2], b[0], b[2]],
            Self::North => [16.0 - b[0], 16.0 - b[1], 16.0 - a[0], 16.0 - a[1]],
            Self::South => [a[0], 16.0 - b[1], b[0], 16.0 - a[1]],
            Self::West => [a[2], 16.0 - b[1], b[2], 16.0 - a[1]],
            Self::East => [16.0 - b[2], 16.0 - b[1], 16.0 - a[2], 16.0 - a[1]],
        }
    }

    /// Unit normal used by the vanilla baked quad's cardinal direction.
    pub fn normal(self) -> [f32; 3] {
        match self {
            Self::Down => [0.0, -1.0, 0.0],
            Self::Up => [0.0, 1.0, 0.0],
            Self::North => [0.0, 0.0, -1.0],
            Self::South => [0.0, 0.0, 1.0],
            Self::West => [-1.0, 0.0, 0.0],
            Self::East => [1.0, 0.0, 0.0],
        }
    }

    fn uv_axes(self) -> ([f32; 3], [f32; 3]) {
        let corners = self.corners([0.0; 3], [1.0; 3]);
        (
            subtract(corners[3], corners[0]),
            subtract(corners[1], corners[0]),
        )
    }

    fn rotated(self, x: u16, y: u16) -> Self {
        let normal = rotate(self.normal(), x, y);
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.normal() == normal)
            .expect("right-angle rotation preserves cardinal directions")
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    /// Coordinates are in block units, not sixteenths of a block.
    pub position: [f32; 3],
    /// Texture coordinates are relative to this sprite, before atlas mapping.
    pub uv: [f32; 2],
}

/// Unrotated face with positions and UV bounds in sixteenths of a block.
pub fn face_vertices(
    direction: Direction,
    from: [f32; 3],
    to: [f32; 3],
    uv: [f32; 4],
) -> [Vertex; 4] {
    let corners = direction.corners(from, to);
    std::array::from_fn(|i| Vertex {
        position: corners[i].map(|coordinate| coordinate / 16.0),
        uv: [
            if i < 2 { uv[0] } else { uv[2] } / 16.0,
            if i == 0 || i == 3 { uv[1] } else { uv[3] } / 16.0,
        ],
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct Quad {
    /// The face named in the source model, before element and blockstate transforms.
    pub source_face: Direction,
    pub direction: Direction,
    pub cullface: Option<Direction>,
    pub texture: TextureReference,
    pub tint_index: i32,
    pub shade_direction_override: Option<Direction>,
    pub light_emission: u8,
    pub vertices: [Vertex; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct CuboidModel {
    pub ambient_occlusion: bool,
    pub quads: Vec<Quad>,
}

#[derive(Clone, Debug)]
pub struct WeightedModel {
    pub choice: ModelChoice,
    pub model: CuboidModel,
}

#[derive(Clone, Debug)]
pub struct BakedBlockstate {
    pub definition: blockstates::BlockstateDefinition,
    pub groups: Vec<Vec<WeightedModel>>,
}

/// Each returned group is a weighted alternative; multipart blocks may have several groups.
pub fn bake_blockstate(
    source: &mut impl ResourceProvider,
    block: &str,
    properties: &BTreeMap<String, String>,
) -> Result<BakedBlockstate> {
    let definition = blockstates::resolve_definition(source, block, properties)?;
    let groups = definition
        .groups
        .iter()
        .cloned()
        .map(|group| {
            group
                .into_iter()
                .map(|choice| {
                    let name = format!("{}:{}", choice.model.namespace, choice.model.path);
                    let model = models::resolve_model(source, &name)
                        .with_context(|| format!("cannot resolve model {name} for {block}"))?;
                    let baked = bake(&model, &choice)
                        .with_context(|| format!("cannot bake model {name} for {block}"))?;
                    Ok(WeightedModel {
                        choice,
                        model: baked,
                    })
                })
                .collect()
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(BakedBlockstate { definition, groups })
}

/// Applies the blockstate's right-angle transform and optional UV lock.
pub fn bake(model: &ResolvedModel, choice: &ModelChoice) -> Result<CuboidModel> {
    ensure!(
        matches!(choice.x_rotation, 0 | 90 | 180 | 270),
        "invalid x rotation"
    );
    ensure!(
        matches!(choice.y_rotation, 0 | 90 | 180 | 270),
        "invalid y rotation"
    );
    let mut baked = bake_untransformed(model)?;
    let (x, y) = (choice.x_rotation, choice.y_rotation);
    for quad in &mut baked.quads {
        let old_direction = quad.source_face;
        let new_direction = old_direction.rotated(x, y);
        if choice.uv_lock {
            let (old_u, old_v) = old_direction.uv_axes();
            let (new_u, new_v) = new_direction.uv_axes();
            let rotated_u = rotate(old_u, x, y);
            let rotated_v = rotate(old_v, x, y);
            for vertex in &mut quad.vertices {
                let [u, v] = vertex.uv.map(|coordinate| coordinate - 0.5);
                vertex.uv = [
                    dot(rotated_u, new_u) * u + dot(rotated_v, new_u) * v + 0.5,
                    dot(rotated_u, new_v) * u + dot(rotated_v, new_v) * v + 0.5,
                ];
            }
        }
        for vertex in &mut quad.vertices {
            vertex.position = rotate(vertex.position.map(|coordinate| coordinate - 0.5), x, y)
                .map(|coordinate| coordinate + 0.5);
        }
        quad.direction = facing(&quad.vertices);
        quad.cullface = quad.cullface.map(|direction| direction.rotated(x, y));
    }
    Ok(baked)
}

fn subtract(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn rotate(mut vector: [f32; 3], x: u16, y: u16) -> [f32; 3] {
    for _ in 0..x / 90 {
        vector = [vector[0], vector[2], -vector[1]];
    }
    for _ in 0..y / 90 {
        vector = [-vector[2], vector[1], vector[0]];
    }
    vector
}

/// Bake element transforms. Blockstate rotations and UV locking are not applied here.
pub fn bake_untransformed(model: &ResolvedModel) -> Result<CuboidModel> {
    let ambient_occlusion = model
        .fields
        .get("ambientocclusion")
        .map(|value| {
            value
                .as_bool()
                .context("ambientocclusion must be a boolean")
        })
        .transpose()?
        .unwrap_or(true);
    let mut result = CuboidModel {
        ambient_occlusion,
        quads: Vec::new(),
    };
    let Some(elements) = model.fields.get("elements") else {
        return Ok(result);
    };
    for (index, element) in elements
        .as_array()
        .context("model elements must be an array")?
        .iter()
        .enumerate()
    {
        let element = element
            .as_object()
            .with_context(|| format!("model element {index} must be an object"))?;
        let element_rotation = element
            .get("rotation")
            .map(ElementRotation::parse)
            .transpose()
            .with_context(|| format!("invalid rotation on model element {index}"))?;
        let from = numbers::<3>(element.get("from").context("element is missing from")?)?;
        let to = numbers::<3>(element.get("to").context("element is missing to")?)?;
        let shade = element
            .get("shade_direction_override")
            .map(direction)
            .transpose()?;
        let light = element
            .get("light_emission")
            .map(|v| v.as_u64().context("light_emission must be an integer"))
            .transpose()?
            .unwrap_or(0);
        ensure!(light <= 15, "light_emission must be between 0 and 15");
        let faces = element
            .get("faces")
            .and_then(Value::as_object)
            .context("element faces must be an object")?;
        for key in faces.keys() {
            Direction::parse(key)?;
        }
        for direction in Direction::ALL {
            let Some(face) = faces.get(direction.name()) else {
                continue;
            };
            let face = face.as_object().context("model face must be an object")?;
            let texture = model.texture(
                face.get("texture")
                    .and_then(Value::as_str)
                    .context("model face is missing texture")?,
            )?;
            let cullface = face.get("cullface").map(self::direction).transpose()?;
            let tint = face
                .get("tintindex")
                .map(|v| v.as_i64().context("tintindex must be an integer"))
                .transpose()?
                .unwrap_or(-1);
            let tint_index = i32::try_from(tint).context("tintindex exceeds 32-bit range")?;
            let rotation = face
                .get("rotation")
                .map(|v| v.as_u64().context("face rotation must be an integer"))
                .transpose()?
                .unwrap_or(0);
            ensure!(
                matches!(rotation, 0 | 90 | 180 | 270),
                "invalid face UV rotation"
            );
            let uv = face
                .get("uv")
                .map(numbers::<4>)
                .transpose()?
                .unwrap_or_else(|| direction.default_uv(from, to));
            let corners = direction.corners(from, to);
            let vertices = std::array::from_fn(|i| {
                let uv_index = (i + rotation as usize / 90) % 4;
                let mut position = corners[i].map(|v| v / 16.0);
                if let Some(transform) = &element_rotation {
                    position = transform.apply(position);
                }
                Vertex {
                    position,
                    uv: [
                        if uv_index < 2 { uv[0] } else { uv[2] } / 16.0,
                        if uv_index == 0 || uv_index == 3 {
                            uv[1]
                        } else {
                            uv[3]
                        } / 16.0,
                    ],
                }
            });
            result.quads.push(Quad {
                source_face: direction,
                direction: facing(&vertices),
                cullface,
                texture,
                tint_index,
                shade_direction_override: shade,
                light_emission: light as u8,
                vertices,
            });
        }
    }
    Ok(result)
}

struct ElementRotation {
    origin: Vec3,
    matrix: Mat3,
}

impl ElementRotation {
    fn parse(value: &Value) -> Result<Self> {
        let object = value.as_object().context("rotation must be an object")?;
        let origin = Vec3::from_array(numbers::<3>(
            object.get("origin").context("rotation is missing origin")?,
        )?) / 16.0;
        let angle = |key: &str| -> Result<f32> {
            let number = object.get(key).map(number).transpose()?.unwrap_or(0.0);
            Ok(number.to_radians())
        };
        let matrix = if object.contains_key("axis") || object.contains_key("angle") {
            let axis = object
                .get("axis")
                .and_then(Value::as_str)
                .context("rotation is missing axis")?;
            let radians =
                number(object.get("angle").context("rotation is missing angle")?)?.to_radians();
            match axis.to_ascii_lowercase().as_str() {
                "x" => Mat3::from_rotation_x(radians),
                "y" => Mat3::from_rotation_y(radians),
                "z" => Mat3::from_rotation_z(radians),
                _ => bail!("invalid rotation axis {axis}"),
            }
        } else {
            ensure!(
                object.contains_key("x") || object.contains_key("y") || object.contains_key("z"),
                "rotation is missing axis/angle or Euler angles"
            );
            Mat3::from_rotation_z(angle("z")?)
                * Mat3::from_rotation_y(angle("y")?)
                * Mat3::from_rotation_x(angle("x")?)
        };
        let rescale = object
            .get("rescale")
            .map(|value| {
                value
                    .as_bool()
                    .context("rotation rescale must be a boolean")
            })
            .transpose()?
            .unwrap_or(false);
        let matrix = if rescale {
            let column_scale = |axis: Vec3| 1.0 / axis.abs().max_element();
            matrix
                * Mat3::from_diagonal(Vec3::new(
                    column_scale(matrix.x_axis),
                    column_scale(matrix.y_axis),
                    column_scale(matrix.z_axis),
                ))
        } else {
            matrix
        };
        Ok(Self { origin, matrix })
    }

    fn apply(&self, position: [f32; 3]) -> [f32; 3] {
        (self.matrix * (Vec3::from_array(position) - self.origin) + self.origin).to_array()
    }
}

fn facing(vertices: &[Vertex; 4]) -> Direction {
    let a = Vec3::from_array(vertices[0].position);
    let b = Vec3::from_array(vertices[1].position);
    let c = Vec3::from_array(vertices[2].position);
    let normal = (b - a).cross(c - a).normalize_or_zero();
    let mut facing = Direction::Up;
    let mut best = 0.0;
    for direction in Direction::ALL {
        let score = normal.dot(Vec3::from_array(direction.normal()));
        if score > best {
            best = score;
            facing = direction;
        }
    }
    facing
}

fn number(value: &Value) -> Result<f32> {
    let output = value.as_f64().context("coordinate must be a number")? as f32;
    ensure!(output.is_finite(), "coordinate must be finite");
    Ok(output)
}

fn direction(value: &Value) -> Result<Direction> {
    Direction::parse(value.as_str().context("face direction must be a string")?)
}

fn numbers<const N: usize>(value: &Value) -> Result<[f32; N]> {
    let array = value.as_array().context("coordinates must be an array")?;
    ensure!(array.len() == N, "expected {N} coordinates");
    let mut output = [0.0; N];
    for (output, value) in output.iter_mut().zip(array) {
        *output = number(value)?;
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(json: &str) -> ResolvedModel {
        ResolvedModel {
            fields: serde_json::from_str(json).unwrap(),
            generated: false,
        }
    }

    #[test]
    fn bakes_explicit_uv_texture_tint_and_culling() {
        let model = model(
            r##"{"textures":{"side":"block/stone"},"elements":[{"from":[1,2,3],"to":[13,14,15],"shade_direction_override":"up","light_emission":15,"faces":{"north":{"texture":"#side","uv":[0,4,8,16],"rotation":90,"cullface":"north","tintindex":2}}}]}"##,
        );
        let baked = bake_untransformed(&model).unwrap();
        let quad = &baked.quads[0];
        assert_eq!(quad.direction, Direction::North);
        assert_eq!(quad.cullface, Some(Direction::North));
        assert_eq!(quad.shade_direction_override, Some(Direction::Up));
        assert_eq!(quad.tint_index, 2);
        assert_eq!(quad.light_emission, 15);
        assert_eq!(quad.texture.sprite.path, "block/stone");
        assert_eq!(
            quad.vertices[0].position,
            [13.0 / 16.0, 14.0 / 16.0, 3.0 / 16.0]
        );
        assert_eq!(quad.vertices[0].uv, [0.0, 1.0]);
    }

    #[test]
    fn malformed_element_rotation_is_an_error() {
        let model = model(
            r##"{"elements":[{"from":[0,0,0],"to":[16,16,16],"rotation":{"origin":[8,8,8],"axis":"wrong","angle":45},"faces":{}}]}"##,
        );
        assert!(
            bake_untransformed(&model)
                .unwrap_err()
                .to_string()
                .contains("rotation")
        );
    }
}
