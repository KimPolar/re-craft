//! Inventory player preview from the 26.3 humanoid cuboids and local skin.
//!
//! The UI's small orthographic target is rasterized on the CPU. Custom skins,
//! special item models, and some player poses are not implemented yet.

use std::sync::OnceLock;

use glam::{Quat, Vec2, Vec3};

use crate::{
    held_items::{DisplayTransform, HeldModel},
    menu_draw::{Canvas, Rect},
    player_skin::PlayerSkin,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Part {
    Head,
    Body,
    RightArm,
    LeftArm,
    RightLeg,
    LeftLeg,
}

#[derive(Clone, Copy, Debug)]
struct Vertex {
    position: Vec3,
    uv: Vec2,
}

#[derive(Clone, Copy, Debug)]
struct Quad {
    part: Part,
    outer: bool,
    pivot: Vec3,
    normal: Vec3,
    vertices: [Vertex; 4],
}

fn cuboid(
    part: Part,
    outer: bool,
    pivot: Vec3,
    min: Vec3,
    size: Vec3,
    tex: Vec2,
    grow: f32,
) -> [Quad; 6] {
    let lo = min - Vec3::splat(grow);
    let hi = min + size + Vec3::splat(grow);
    let (x0, y0, z0) = (lo.x, lo.y, lo.z);
    let (x1, y1, z1) = (hi.x, hi.y, hi.z);
    let (u, v, w, h, d) = (tex.x, tex.y, size.x, size.y, size.z);
    let (u0, u1, u2, u3, u4, u5) = (
        u,
        u + d,
        u + d + w,
        u + d + w + w,
        u + d + w + d,
        u + d + w + d + w,
    );
    let (v0, v1, v2) = (v, v + d, v + d + h);
    let make = |normal, positions: [Vec3; 4], uv: [Vec2; 4]| Quad {
        part,
        outer,
        pivot,
        normal,
        vertices: std::array::from_fn(|i| Vertex {
            position: positions[i],
            uv: uv[i] / 64.0,
        }),
    };
    [
        make(
            Vec3::NEG_Y,
            [
                Vec3::new(x1, y0, z1),
                Vec3::new(x0, y0, z1),
                Vec3::new(x0, y0, z0),
                Vec3::new(x1, y0, z0),
            ],
            [
                Vec2::new(u2, v0),
                Vec2::new(u1, v0),
                Vec2::new(u1, v1),
                Vec2::new(u2, v1),
            ],
        ),
        make(
            Vec3::Y,
            [
                Vec3::new(x1, y1, z0),
                Vec3::new(x0, y1, z0),
                Vec3::new(x0, y1, z1),
                Vec3::new(x1, y1, z1),
            ],
            [
                Vec2::new(u3, v1),
                Vec2::new(u2, v1),
                Vec2::new(u2, v0),
                Vec2::new(u3, v0),
            ],
        ),
        make(
            Vec3::NEG_X,
            [
                Vec3::new(x0, y0, z0),
                Vec3::new(x0, y0, z1),
                Vec3::new(x0, y1, z1),
                Vec3::new(x0, y1, z0),
            ],
            [
                Vec2::new(u1, v1),
                Vec2::new(u0, v1),
                Vec2::new(u0, v2),
                Vec2::new(u1, v2),
            ],
        ),
        make(
            Vec3::NEG_Z,
            [
                Vec3::new(x1, y0, z0),
                Vec3::new(x0, y0, z0),
                Vec3::new(x0, y1, z0),
                Vec3::new(x1, y1, z0),
            ],
            [
                Vec2::new(u2, v1),
                Vec2::new(u1, v1),
                Vec2::new(u1, v2),
                Vec2::new(u2, v2),
            ],
        ),
        make(
            Vec3::X,
            [
                Vec3::new(x1, y0, z1),
                Vec3::new(x1, y0, z0),
                Vec3::new(x1, y1, z0),
                Vec3::new(x1, y1, z1),
            ],
            [
                Vec2::new(u4, v1),
                Vec2::new(u2, v1),
                Vec2::new(u2, v2),
                Vec2::new(u4, v2),
            ],
        ),
        make(
            Vec3::Z,
            [
                Vec3::new(x0, y0, z1),
                Vec3::new(x1, y0, z1),
                Vec3::new(x1, y1, z1),
                Vec3::new(x0, y1, z1),
            ],
            [
                Vec2::new(u5, v1),
                Vec2::new(u4, v1),
                Vec2::new(u4, v2),
                Vec2::new(u5, v2),
            ],
        ),
    ]
}

fn build_model(slim: bool) -> Vec<Quad> {
    let mut result = Vec::with_capacity(72);
    let mut add = |part, pivot, min, size, tex, overlay_tex, overlay_grow| {
        result.extend(cuboid(part, false, pivot, min, size, tex, 0.0));
        result.extend(cuboid(
            part,
            true,
            pivot,
            min,
            size,
            overlay_tex,
            overlay_grow,
        ));
    };
    add(
        Part::Head,
        Vec3::ZERO,
        Vec3::new(-4.0, -8.0, -4.0),
        Vec3::splat(8.0),
        Vec2::new(0.0, 0.0),
        Vec2::new(32.0, 0.0),
        0.5,
    );
    add(
        Part::Body,
        Vec3::ZERO,
        Vec3::new(-4.0, 0.0, -2.0),
        Vec3::new(8.0, 12.0, 4.0),
        Vec2::new(16.0, 16.0),
        Vec2::new(16.0, 32.0),
        0.25,
    );
    let arm_width = if slim { 3.0 } else { 4.0 };
    let right_x = if slim { -2.0 } else { -3.0 };
    add(
        Part::RightArm,
        Vec3::new(-5.0, 2.0, 0.0),
        Vec3::new(right_x, -2.0, -2.0),
        Vec3::new(arm_width, 12.0, 4.0),
        Vec2::new(40.0, 16.0),
        Vec2::new(40.0, 32.0),
        0.25,
    );
    add(
        Part::LeftArm,
        Vec3::new(5.0, 2.0, 0.0),
        Vec3::new(-1.0, -2.0, -2.0),
        Vec3::new(arm_width, 12.0, 4.0),
        Vec2::new(32.0, 48.0),
        Vec2::new(48.0, 48.0),
        0.25,
    );
    add(
        Part::RightLeg,
        Vec3::new(-1.9, 12.0, 0.0),
        Vec3::new(-2.0, 0.0, -2.0),
        Vec3::new(4.0, 12.0, 4.0),
        Vec2::new(0.0, 16.0),
        Vec2::new(0.0, 32.0),
        0.25,
    );
    add(
        Part::LeftLeg,
        Vec3::new(1.9, 12.0, 0.0),
        Vec3::new(-2.0, 0.0, -2.0),
        Vec3::new(4.0, 12.0, 4.0),
        Vec2::new(16.0, 48.0),
        Vec2::new(0.0, 48.0),
        0.25,
    );
    result
}

fn model(slim: bool) -> &'static [Quad] {
    static SLIM: OnceLock<Vec<Quad>> = OnceLock::new();
    static WIDE: OnceLock<Vec<Quad>> = OnceLock::new();
    if slim {
        SLIM.get_or_init(|| build_model(true))
    } else {
        WIDE.get_or_init(|| build_model(false))
    }
}

/// Java's player armor uses 64x32 texture coordinates and the same wide-arm
/// model for both player skin shapes. Left limbs reuse mirrored right UVs.
fn build_armor_model(slot: usize) -> Vec<Quad> {
    let mut result = Vec::new();
    let mut add = |part, pivot, min: Vec3, size: Vec3, tex, grow, mirror| {
        let mut quads = cuboid(part, false, pivot, min, size, tex, grow);
        for quad in &mut quads {
            if mirror {
                for vertex in &mut quad.vertices {
                    vertex.position.x = 2.0 * min.x + size.x - vertex.position.x;
                }
                quad.vertices.reverse();
                quad.normal.x = -quad.normal.x;
            }
            for vertex in &mut quad.vertices {
                vertex.uv.y *= 2.0;
            }
        }
        result.extend(quads);
    };
    if slot == 0 {
        for (tex, grow) in [(Vec2::ZERO, 1.0), (Vec2::new(32.0, 0.0), 1.5)] {
            add(
                Part::Head,
                Vec3::ZERO,
                Vec3::new(-4.0, -8.0, -4.0),
                Vec3::splat(8.0),
                tex,
                grow,
                false,
            );
        }
    }
    if slot == 1 || slot == 2 {
        add(
            Part::Body,
            Vec3::ZERO,
            Vec3::new(-4.0, 0.0, -2.0),
            Vec3::new(8.0, 12.0, 4.0),
            Vec2::new(16.0, 16.0),
            if slot == 1 { 1.0 } else { 0.5 },
            false,
        );
    }
    if slot == 1 {
        add(
            Part::RightArm,
            Vec3::new(-5.0, 2.0, 0.0),
            Vec3::new(-3.0, -2.0, -2.0),
            Vec3::new(4.0, 12.0, 4.0),
            Vec2::new(40.0, 16.0),
            1.0,
            false,
        );
        add(
            Part::LeftArm,
            Vec3::new(5.0, 2.0, 0.0),
            Vec3::new(-1.0, -2.0, -2.0),
            Vec3::new(4.0, 12.0, 4.0),
            Vec2::new(40.0, 16.0),
            1.0,
            true,
        );
    }
    if slot == 2 || slot == 3 {
        let grow = if slot == 2 { 0.4 } else { 0.9 };
        add(
            Part::RightLeg,
            Vec3::new(-1.9, 12.0, 0.0),
            Vec3::new(-2.0, 0.0, -2.0),
            Vec3::new(4.0, 12.0, 4.0),
            Vec2::new(0.0, 16.0),
            grow,
            false,
        );
        add(
            Part::LeftLeg,
            Vec3::new(1.9, 12.0, 0.0),
            Vec3::new(-2.0, 0.0, -2.0),
            Vec3::new(4.0, 12.0, 4.0),
            Vec2::new(0.0, 16.0),
            grow,
            true,
        );
    }
    result
}

fn armor_model(slot: usize) -> &'static [Quad] {
    static MODELS: OnceLock<[Vec<Quad>; 4]> = OnceLock::new();
    &MODELS.get_or_init(|| std::array::from_fn(build_armor_model))[slot]
}

/// ModelPart.resetPose followed by AvatarRenderer.renderHand's arm rotation.
pub(crate) fn first_person_arm(slim: bool, left: bool) -> Vec<crate::mesh::Vertex> {
    let part = if left { Part::LeftArm } else { Part::RightArm };
    let rotation = Quat::from_rotation_z(if left { -0.1 } else { 0.1 });
    let mut result = Vec::with_capacity(72);
    for quad in model(slim).iter().filter(|quad| quad.part == part) {
        let normal = rotation * quad.normal;
        let shade =
            (0.6 + 0.4 * normal.dot(Vec3::new(0.2, -1.0, 0.7).normalize()).abs()).clamp(0.0, 1.0);
        for index in [0, 1, 2, 0, 2, 3] {
            let vertex = quad.vertices[index];
            result.push(crate::mesh::Vertex {
                position: ((quad.pivot + rotation * vertex.position) / 16.0).to_array(),
                uv: vertex.uv.to_array(),
                shade,
                tint: [1.0; 3],
                light: u32::MAX,
            });
        }
    }
    result
}

#[derive(Clone, Copy)]
pub(crate) struct Projected {
    pub screen: Vec2,
    pub depth: f32,
    pub uv: Vec2,
}

#[derive(Default)]
pub struct Hands<'a> {
    pub right_present: bool,
    pub left_present: bool,
    pub right: Option<&'a HeldModel>,
    pub left: Option<&'a HeldModel>,
}

fn arm_rotation(left: bool, holding: bool, tick: u64) -> Quat {
    let age = tick as f32;
    let sign = if left { -1.0 } else { 1.0 };
    let z = ((age * 0.09).cos() * 0.05 + 0.05) * sign;
    let x = (age * 0.067).sin() * 0.05 * sign
        - if holding {
            std::f32::consts::PI / 10.0
        } else {
            0.0
        };
    Quat::from_rotation_z(z) * Quat::from_rotation_x(x)
}

#[derive(Clone, Copy)]
struct PreviewProjection {
    center: Vec2,
    gui: Quat,
    body: Quat,
}

impl PreviewProjection {
    fn project(self, mut position: Vec3, uv: Vec2) -> Projected {
        position.y -= 1.501;
        position *= Vec3::new(-1.0, -1.0, 1.0);
        position = self.gui * (self.body * position);
        position.y += 1.8 / 2.0 + 0.0625;
        Projected {
            screen: self.center + position.truncate() * 30.0,
            depth: -position.z,
            uv,
        }
    }

    fn brightness(self, normal: Vec3) -> f32 {
        let normal = Vec3::new(1.0, 1.0, -1.0)
            * (self.gui * (self.body * (Vec3::new(-1.0, -1.0, 1.0) * normal)));
        let light0 = Vec3::new(0.2, -1.0, 1.0).normalize();
        let light1 = Vec3::new(-0.2, -1.0, 0.0).normalize();
        ((normal.dot(light0).max(0.0) + normal.dot(light1).max(0.0)) * 0.6 + 0.4).min(1.0)
    }
}

fn hand_point(
    position: Vec3,
    slim: bool,
    left: bool,
    arm: Quat,
    display: DisplayTransform,
) -> Vec3 {
    let sign = if left { -1.0 } else { 1.0 };
    let mut pivot = Vec3::new(if left { 5.0 } else { -5.0 }, 2.0, 0.0) / 16.0;
    if slim {
        pivot.x += sign * 0.5 / 16.0;
    }
    let mut translation = display.translation;
    translation.x *= sign;
    let item =
        translation + display.rotation(left) * (display.scale * (position - Vec3::splat(0.5)));
    let attachment = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)
        * Quat::from_rotation_y(std::f32::consts::PI);
    pivot + arm * (attachment * (Vec3::new(sign, 2.0, -10.0) / 16.0 + item))
}

#[allow(clippy::too_many_arguments)]
fn draw_hand(
    canvas: &mut Canvas,
    depth: &mut [f32],
    clip: Rect,
    projection: PreviewProjection,
    slim: bool,
    left: bool,
    arm: Quat,
    model: &HeldModel,
) {
    let display = if left { model.left } else { model.right };
    let attachment = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)
        * Quat::from_rotation_y(std::f32::consts::PI);
    for quad in &model.quads {
        let a = Vec3::from_array(quad.vertices[0].position);
        let b = Vec3::from_array(quad.vertices[1].position);
        let c = Vec3::from_array(quad.vertices[2].position);
        let normal = (b - a).cross(c - a).normalize_or_zero();
        let inverse_scale = display
            .scale
            .to_array()
            .map(|v| if v == 0.0 { 0.0 } else { 1.0 / v });
        let normal = arm
            * (attachment * (display.rotation(left) * (normal * Vec3::from_array(inverse_scale))));
        let brightness = projection.brightness(normal.normalize_or_zero());
        let vertices = quad.vertices.map(|vertex| {
            projection.project(
                hand_point(Vec3::from_array(vertex.position), slim, left, arm, display),
                Vec2::from_array(vertex.uv),
            )
        });
        raster(
            canvas,
            &quad.texture,
            depth,
            clip,
            [vertices[0], vertices[1], vertices[2]],
            brightness,
            quad.tint,
        );
        raster(
            canvas,
            &quad.texture,
            depth,
            clip,
            [vertices[0], vertices[2], vertices[3]],
            brightness,
            quad.tint,
        );
    }
}

/// InventoryScreen's 49x70 target, 30-unit scale and mouse-follow angles.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    canvas: &mut Canvas,
    skin: &PlayerSkin,
    left: i32,
    top: i32,
    mouse: (i32, i32),
    tick: u64,
    hands: Hands<'_>,
    armor: [Option<&crate::equipment_visual::ArmorAppearance>; 4],
) {
    let clip = Rect {
        x: left + 26,
        y: top + 8,
        w: 49,
        h: 70,
    };
    let cx = clip.x as f32 + clip.w as f32 / 2.0;
    let cy = clip.y as f32 + clip.h as f32 / 2.0;
    let horizontal = ((cx - mouse.0 as f32) / 40.0).atan();
    let vertical = ((cy - mouse.1 as f32) / 40.0).atan();
    let radians = 20.0_f32.to_radians();
    let body = Quat::from_rotation_y(-horizontal * radians);
    let gui =
        Quat::from_rotation_z(std::f32::consts::PI) * Quat::from_rotation_x(vertical * radians);
    let head =
        Quat::from_rotation_y(horizontal * radians) * Quat::from_rotation_x(-vertical * radians);
    let right_arm = arm_rotation(false, hands.right_present, tick);
    let left_arm = arm_rotation(true, hands.left_present, tick);
    let projection = PreviewProjection {
        center: Vec2::new(cx, cy),
        gui,
        body,
    };
    let mut depth = [f32::NEG_INFINITY; 49 * 70];
    let mesh = model(skin.slim);
    // The outer parts have transparent texels; render them after the base layer.
    for outer in [false, true] {
        for quad in mesh.iter().filter(|quad| quad.outer == outer) {
            let part_rotation = match quad.part {
                Part::Head => head,
                Part::RightArm => right_arm,
                Part::LeftArm => left_arm,
                _ => Quat::IDENTITY,
            };
            let brightness = projection.brightness(part_rotation * quad.normal);
            let projected = quad.vertices.map(|vertex| {
                projection.project(
                    (part_rotation * vertex.position + quad.pivot) / 16.0,
                    vertex.uv,
                )
            });
            raster(
                canvas,
                &skin.image,
                &mut depth,
                clip,
                [projected[0], projected[1], projected[2]],
                brightness,
                [1.0; 3],
            );
            raster(
                canvas,
                &skin.image,
                &mut depth,
                clip,
                [projected[0], projected[2], projected[3]],
                brightness,
                [1.0; 3],
            );
        }
    }
    // Match HumanoidArmorLayer's chest, legs, feet, head submission order.
    for slot in [1, 2, 3, 0] {
        let Some(appearance) = armor[slot] else {
            continue;
        };
        for layer in &appearance.layers {
            for quad in armor_model(slot) {
                let rotation = match quad.part {
                    Part::Head => head,
                    Part::RightArm => right_arm,
                    Part::LeftArm => left_arm,
                    _ => Quat::IDENTITY,
                };
                let brightness = projection.brightness(rotation * quad.normal);
                let projected = quad.vertices.map(|vertex| {
                    projection.project((rotation * vertex.position + quad.pivot) / 16.0, vertex.uv)
                });
                for indices in [[0, 1, 2], [0, 2, 3]] {
                    raster(
                        canvas,
                        &layer.texture,
                        &mut depth,
                        clip,
                        indices.map(|index| projected[index]),
                        brightness,
                        layer.tint,
                    );
                }
            }
        }
    }
    if let Some(model) = hands.right {
        draw_hand(
            canvas, &mut depth, clip, projection, skin.slim, false, right_arm, model,
        );
    }
    if let Some(model) = hands.left {
        draw_hand(
            canvas, &mut depth, clip, projection, skin.slim, true, left_arm, model,
        );
    }
}

fn edge(a: Vec2, b: Vec2, p: Vec2) -> f32 {
    (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)
}

pub(crate) fn raster(
    canvas: &mut Canvas,
    skin: &image::RgbaImage,
    depth: &mut [f32],
    clip: Rect,
    vertices: [Projected; 3],
    brightness: f32,
    tint: [f32; 3],
) {
    raster_with_fragment(
        canvas,
        skin,
        depth,
        clip,
        vertices,
        brightness,
        tint,
        |_, _, _, _, _| {},
    );
}

/// As `raster`, with the winning texture coordinate and alpha before compositing.
#[allow(clippy::too_many_arguments)]
pub(crate) fn raster_with_fragment(
    canvas: &mut Canvas,
    skin: &image::RgbaImage,
    depth: &mut [f32],
    clip: Rect,
    vertices: [Projected; 3],
    brightness: f32,
    tint: [f32; 3],
    mut fragment: impl FnMut(i32, i32, Vec2, u8, u8),
) {
    let [a, b, c] = vertices;
    let area = edge(a.screen, b.screen, c.screen);
    if area.abs() < 1.0e-6 {
        return;
    }
    let min_x = (a.screen.x.min(b.screen.x).min(c.screen.x).floor() as i32)
        .max(clip.x)
        .max(0);
    let min_y = (a.screen.y.min(b.screen.y).min(c.screen.y).floor() as i32)
        .max(clip.y)
        .max(0);
    let max_x = (a.screen.x.max(b.screen.x).max(c.screen.x).ceil() as i32)
        .min(clip.x + clip.w)
        .min(canvas.width);
    let max_y = (a.screen.y.max(b.screen.y).max(c.screen.y).ceil() as i32)
        .min(clip.y + clip.h)
        .min(canvas.height);
    for y in min_y..max_y {
        for x in min_x..max_x {
            let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
            let w0 = edge(b.screen, c.screen, p) / area;
            let w1 = edge(c.screen, a.screen, p) / area;
            let w2 = 1.0 - w0 - w1;
            if w0 < -1.0e-6 || w1 < -1.0e-6 || w2 < -1.0e-6 {
                continue;
            }
            let z = a.depth * w0 + b.depth * w1 + c.depth * w2;
            let index = ((y - clip.y) * clip.w + (x - clip.x)) as usize;
            if z < depth[index] {
                continue;
            }
            let uv = a.uv * w0 + b.uv * w1 + c.uv * w2;
            let sx = ((uv.x * skin.width() as f32) as u32).min(skin.width() - 1);
            let sy = ((uv.y * skin.height() as f32) as u32).min(skin.height() - 1);
            let mut color = skin.get_pixel(sx, sy).0;
            if color[3] < 26 {
                continue;
            }
            let previous_alpha = (canvas.pixels[(y * canvas.width + x) as usize] >> 24) as u8;
            fragment(x, y, uv, color[3], previous_alpha);
            for (channel, tint) in color[..3].iter_mut().zip(tint) {
                *channel = (f32::from(*channel) * brightness * tint) as u8;
            }
            canvas.pixel(x, y, color);
            depth[index] = z;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_contains_both_skin_layers_and_respects_arm_width() {
        for (slim, width) in [(true, 3.0), (false, 4.0)] {
            let quads = model(slim);
            assert_eq!(quads.len(), 72);
            assert_eq!(quads.iter().filter(|quad| quad.outer).count(), 36);
            let arm = quads
                .iter()
                .find(|quad| {
                    quad.part == Part::RightArm && !quad.outer && quad.normal == Vec3::NEG_Z
                })
                .unwrap();
            assert_eq!(
                arm.vertices[0].position.x - arm.vertices[1].position.x,
                width
            );
        }
    }

    #[test]
    fn preview_rasterization_is_clipped_and_mouse_position_changes_it() {
        let skin = PlayerSkin {
            image: image::RgbaImage::from_pixel(64, 64, image::Rgba([200, 100, 50, 255])),
            slim: false,
        };
        let mut left = Canvas::new(200, 180);
        let mut right = Canvas::new(200, 180);
        draw(
            &mut left,
            &skin,
            10,
            10,
            (0, 45),
            0,
            Hands::default(),
            [None, None, None, None],
        );
        draw(
            &mut right,
            &skin,
            10,
            10,
            (150, 45),
            0,
            Hands::default(),
            [None, None, None, None],
        );
        assert_ne!(left.pixels, right.pixels);
        let count = left.pixels.iter().filter(|pixel| **pixel != 0).count();
        assert!(count > 200 && count < 49 * 70);
        for y in 0..left.height {
            for x in 0..left.width {
                if !(Rect {
                    x: 36,
                    y: 18,
                    w: 49,
                    h: 70,
                })
                .contains(x, y)
                {
                    assert_eq!(left.pixels[(y * left.width + x) as usize], 0);
                }
            }
        }
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn held_item_hand_transforms_match_vanilla_player_model() {
        use serde_json::Value;
        use std::{
            ffi::OsString,
            path::{Path, PathBuf},
            process::Command,
        };
        const JAVA: &str = r#"
import java.nio.file.*;
import com.google.gson.*;
import com.mojang.blaze3d.vertex.PoseStack;
import com.mojang.math.Axis;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.model.HumanoidModel.ArmPose;
import net.minecraft.client.model.player.PlayerModel;
import net.minecraft.client.model.geom.builders.*;
import net.minecraft.client.renderer.entity.state.AvatarRenderState;
import net.minecraft.client.resources.model.cuboid.ItemTransform;
import net.minecraft.world.entity.HumanoidArm;
import org.joml.Vector3f;
public class HeldItemOracle {
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var output = new JsonArray();
    float[][] rotations={{0,0,0},{75,45,0},{12,-31,21}};
    float[][] translations={{0,0,0},{0,2.5f/16,0},{0.25f,-0.1f,0.05f}};
    float[][] scales={{1,1,1},{0.375f,0.375f,0.375f},{1.2f,0.65f,0.9f}};
    float[][] points={{0,0,0},{1,1,1},{0.2f,0.7f,0.4f}};
    for(boolean slim:new boolean[]{false,true}) for(boolean left:new boolean[]{false,true})
    for(boolean held:new boolean[]{false,true}) for(int tick:new int[]{0,1,7,20,51,149,1000}) {
      var root=LayerDefinition.create(PlayerModel.createMesh(CubeDeformation.NONE,slim),64,64).bakeRoot();
      var player=new PlayerModel(root,slim); var state=new AvatarRenderState();
      state.speedValue=1; state.ageInTicks=tick;
      if(left) state.leftArmPose=held?ArmPose.ITEM:ArmPose.EMPTY;
      else state.rightArmPose=held?ArmPose.ITEM:ArmPose.EMPTY;
      player.setupAnim(state);
      var arm=left?HumanoidArm.LEFT:HumanoidArm.RIGHT;
      var part=player.getArm(arm); var result=new JsonObject();
      result.addProperty("x",part.xRot); result.addProperty("z",part.zRot);
      var transforms=new JsonArray();
      for(int i=0;i<rotations.length;i++) {
        var pose=new PoseStack(); player.translateToHand(state,arm,pose);
        pose.rotateDegrees(Axis.XP,-90); pose.rotateDegrees(Axis.YP,180);
        pose.translate((left?-1f:1f)/16,2f/16,-10f/16);
        new ItemTransform(new Vector3f(rotations[i]),new Vector3f(translations[i]),new Vector3f(scales[i])).apply(left,pose.last());
        var resultPoints=new JsonArray();
        for(float[] p:points) { var v=pose.last().pose().transformPosition(new Vector3f(p)); var a=new JsonArray(); a.add(v.x);a.add(v.y);a.add(v.z);resultPoints.add(a); }
        transforms.add(resultPoints);
      }
      result.add("points",transforms); output.add(result);
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(output));
  }
}
"#;
        fn classpath(game: &Path, classes: &Path) -> OsString {
            let version = game.join("versions/26.3");
            let manifest: Value =
                serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
            let mut paths = vec![version.join("26.3.jar"), classes.to_path_buf()];
            for library in manifest["libraries"].as_array().unwrap() {
                if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                    let path = game.join("libraries").join(path);
                    if path.is_file() {
                        paths.push(path);
                    }
                }
            }
            std::env::join_paths(paths).unwrap()
        }
        fn run(command: &mut Command) {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "hand oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("HeldItemOracle.java");
        let output = temp.path().join("output.json");
        std::fs::write(&source, JAVA).unwrap();
        let cp = classpath(&game, temp.path());
        run(Command::new(java.join("javac.exe"))
            .arg("-encoding")
            .arg("UTF-8")
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("HeldItemOracle")
            .arg(&output));
        let expected: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let transforms = [
            DisplayTransform {
                rotation: Vec3::ZERO,
                translation: Vec3::ZERO,
                scale: Vec3::ONE,
            },
            DisplayTransform {
                rotation: Vec3::new(75.0, 45.0, 0.0),
                translation: Vec3::new(0.0, 2.5 / 16.0, 0.0),
                scale: Vec3::splat(0.375),
            },
            DisplayTransform {
                rotation: Vec3::new(12.0, -31.0, 21.0),
                translation: Vec3::new(0.25, -0.1, 0.05),
                scale: Vec3::new(1.2, 0.65, 0.9),
            },
        ];
        let points = [Vec3::ZERO, Vec3::ONE, Vec3::new(0.2, 0.7, 0.4)];
        let mut index = 0;
        for slim in [false, true] {
            for left in [false, true] {
                for held in [false, true] {
                    for tick in [0, 1, 7, 20, 51, 149, 1000] {
                        let expected = &expected[index];
                        index += 1;
                        let arm = arm_rotation(left, held, tick);
                        for (i, transform) in transforms.into_iter().enumerate() {
                            for (j, point) in points.into_iter().enumerate() {
                                let actual = hand_point(point, slim, left, arm, transform);
                                for (axis, value) in actual.to_array().into_iter().enumerate() {
                                    let java =
                                        expected["points"][i][j][axis].as_f64().unwrap() as f32;
                                    assert!(
                                        (value - java).abs() < 0.00003,
                                        "slim={slim} left={left} held={held} tick={tick} transform={i} point={j} axis={axis}: {value} != {java}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(index, expected.len());
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn player_cuboids_and_uvs_match_java_models() {
        use serde_json::Value;
        use std::{
            ffi::OsString,
            path::{Path, PathBuf},
            process::Command,
        };
        const JAVA: &str = r#"
import java.lang.reflect.*;
import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.client.model.player.PlayerModel;
import net.minecraft.client.model.geom.ModelPart;
import net.minecraft.client.model.geom.builders.*;
public class PlayerGeometryOracle {
  public static void main(String[] args) throws Exception {
    var field = ModelPart.class.getDeclaredField("cubes"); field.setAccessible(true);
    var result = new JsonArray();
    String[][] parts = {{"head","hat"},{"body","jacket"},{"right_arm","right_sleeve"},{"left_arm","left_sleeve"},{"right_leg","right_pants"},{"left_leg","left_pants"}};
    for (boolean slim : new boolean[]{false,true}) {
      var root = LayerDefinition.create(PlayerModel.createMesh(CubeDeformation.NONE,slim),64,64).bakeRoot();
      var quads = new JsonArray();
      for (int part=0;part<parts.length;part++) {
        var parent = root.getChild(parts[part][0]);
        for(int outer=0;outer<2;outer++) {
          var node = outer==0 ? parent : parent.getChild(parts[part][1]);
          var cubes = (java.util.List<ModelPart.Cube>)field.get(node);
          for(var poly:cubes.get(0).polygons) {
            var q = new JsonObject(); q.addProperty("part",part); q.addProperty("outer",outer==1);
            var pivot = new JsonArray(); pivot.add(parent.x); pivot.add(parent.y); pivot.add(parent.z); q.add("pivot",pivot);
            var normal = new JsonArray(); normal.add(poly.normal().x()); normal.add(poly.normal().y()); normal.add(poly.normal().z()); q.add("normal",normal);
            var vertices = new JsonArray();
            for(var v:poly.vertices()) { var vertex = new JsonArray(); vertex.add(v.x()); vertex.add(v.y()); vertex.add(v.z()); vertex.add(v.u()); vertex.add(v.v()); vertices.add(vertex); }
            q.add("vertices",vertices); quads.add(q);
          }
        }
      }
      result.add(quads);
    }
    var armor = PlayerModel.createArmorMeshSet(new CubeDeformation(0.5f),new CubeDeformation(1.0f));
    var meshes = new MeshDefinition[]{armor.head(),armor.chest(),armor.legs(),armor.feet()};
    for(var mesh:meshes) {
      var root=LayerDefinition.create(mesh,64,32).bakeRoot(); var quads=new JsonArray();
      for(int part=0;part<parts.length;part++) {
        var parent=root.getChild(parts[part][0]);
        int count=part==0 && parent.hasChild("hat")?2:1;
        for(int outer=0;outer<count;outer++) {
          var node=outer==0?parent:parent.getChild("hat");
          var cubes=(java.util.List<ModelPart.Cube>)field.get(node);
          for(var cube:cubes) for(var poly:cube.polygons) {
            var q=new JsonObject(); q.addProperty("part",part);
            var pivot=new JsonArray();pivot.add(parent.x);pivot.add(parent.y);pivot.add(parent.z);q.add("pivot",pivot);
            var normal=new JsonArray();normal.add(poly.normal().x());normal.add(poly.normal().y());normal.add(poly.normal().z());q.add("normal",normal);
            var vertices=new JsonArray();for(var v:poly.vertices()){var a=new JsonArray();a.add(v.x());a.add(v.y());a.add(v.z());a.add(v.u());a.add(v.v());vertices.add(a);}q.add("vertices",vertices);quads.add(q);
          }
        }
      }
      result.add(quads);
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(result));
  }
}
"#;
        fn classpath(game: &Path, classes: &Path) -> OsString {
            let version = game.join("versions/26.3");
            let manifest: Value =
                serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
            let mut paths = vec![version.join("26.3.jar"), classes.to_path_buf()];
            for library in manifest["libraries"].as_array().unwrap() {
                if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                    let path = game.join("libraries").join(path);
                    if path.is_file() {
                        paths.push(path);
                    }
                }
            }
            std::env::join_paths(paths).unwrap()
        }
        fn run(command: &mut Command) {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "model oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("PlayerGeometryOracle.java");
        let output = temp.path().join("output.json");
        std::fs::write(&source, JAVA).unwrap();
        let cp = classpath(&game, temp.path());
        run(Command::new(java.join("javac.exe"))
            .arg("-encoding")
            .arg("UTF-8")
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("PlayerGeometryOracle")
            .arg(&output));
        let expected: Vec<Vec<Value>> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let parts = [
            Part::Head,
            Part::Body,
            Part::RightArm,
            Part::LeftArm,
            Part::RightLeg,
            Part::LeftLeg,
        ];
        for (slim, quads) in [false, true].into_iter().zip(expected.iter()) {
            assert_eq!(quads.len(), 72);
            for (actual, expected) in model(slim).iter().zip(quads) {
                assert_eq!(
                    actual.part,
                    parts[expected["part"].as_u64().unwrap() as usize]
                );
                assert_eq!(actual.outer, expected["outer"].as_bool().unwrap());
                for (index, value) in actual.pivot.to_array().into_iter().enumerate() {
                    assert!(
                        (f64::from(value) - expected["pivot"][index].as_f64().unwrap()).abs()
                            < 1.0e-6
                    );
                }
                for (index, value) in actual.normal.to_array().into_iter().enumerate() {
                    assert!(
                        (f64::from(value) - expected["normal"][index].as_f64().unwrap()).abs()
                            < 1.0e-6
                    );
                }
                for (actual, expected) in actual
                    .vertices
                    .iter()
                    .zip(expected["vertices"].as_array().unwrap())
                {
                    let values = [
                        actual.position.x,
                        actual.position.y,
                        actual.position.z,
                        actual.uv.x,
                        actual.uv.y,
                    ];
                    for (index, value) in values.into_iter().enumerate() {
                        assert!(
                            (f64::from(value) - expected[index].as_f64().unwrap()).abs() < 1.0e-6,
                            "slim {slim}, part {:?}, actual {values:?}, expected {expected}",
                            actual.position
                        );
                    }
                }
            }
        }
        for slot in 0..4 {
            let quads = &expected[slot + 2];
            let actual_quads = armor_model(slot);
            assert_eq!(actual_quads.len(), quads.len(), "armor slot {slot}");
            for (actual, expected) in actual_quads.iter().zip(quads) {
                assert_eq!(
                    actual.part,
                    parts[expected["part"].as_u64().unwrap() as usize]
                );
                for (index, value) in actual.pivot.to_array().into_iter().enumerate() {
                    assert!(
                        (f64::from(value) - expected["pivot"][index].as_f64().unwrap()).abs()
                            < 1.0e-6
                    );
                }
                for (index, value) in actual.normal.to_array().into_iter().enumerate() {
                    assert!(
                        (f64::from(value) - expected["normal"][index].as_f64().unwrap()).abs()
                            < 1.0e-6
                    );
                }
                for (vertex, expected) in actual
                    .vertices
                    .iter()
                    .zip(expected["vertices"].as_array().unwrap())
                {
                    let values = [
                        vertex.position.x,
                        vertex.position.y,
                        vertex.position.z,
                        vertex.uv.x,
                        vertex.uv.y,
                    ];
                    for (index, value) in values.into_iter().enumerate() {
                        assert!(
                            (f64::from(value) - expected[index].as_f64().unwrap()).abs() < 1.0e-6,
                            "armor slot {slot}, part {:?}, actual {values:?}, expected {expected}",
                            actual.part
                        );
                    }
                }
            }
        }
    }
}
