//! The first-person hand and ordinary item models. Models are cached on
//! the GPU; item definitions and textures are loaded by the existing worker.

use std::{collections::VecDeque, sync::Arc};

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use image::RgbaImage;
use native_session::animation::Hand;
use wgpu::util::DeviceExt;

use crate::{
    held_items::{DisplayTransform, HeldModel},
    mesh::Vertex,
    player_preview,
    player_skin::PlayerSkin,
};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct HandVertex {
    base: Vertex,
    /// Atlas UV and mode: 0 = none, 1 = stitched atlas, 2 = approximate.
    glint: [f32; 3],
}

const HAND_ATTRIBUTES: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
    0 => Float32x3, 1 => Float32x2, 2 => Float32, 3 => Float32x3, 9 => Uint32, 4 => Float32x3
];

fn hand_glint(glint: bool, bounds: Option<[f32; 4]>, uv: [f32; 2]) -> [f32; 3] {
    if !glint {
        return [0.0; 3];
    }
    if let Some([u0, v0, u1, v1]) = bounds {
        [u0 + (u1 - u0) * uv[0], v0 + (v1 - v0) * uv[1], 1.0]
    } else {
        [uv[0], uv[1], 2.0]
    }
}

struct Material {
    vertices: wgpu::Buffer,
    count: u32,
    texture: wgpu::BindGroup,
}

struct GpuModel {
    source: Arc<HeldModel>,
    materials: Vec<Material>,
    bytes: u64,
}

struct GpuSkin {
    source: Arc<PlayerSkin>,
    right: Material,
    left: Material,
}

pub struct FirstPersonInput {
    pub skin: Arc<PlayerSkin>,
    pub main_left: bool,
    pub main_present: bool,
    pub main: Option<Arc<HeldModel>>,
    pub off: Option<Arc<HeldModel>>,
    pub visible: bool,
    pub main_equip: f32,
    pub off_equip: f32,
    pub swing_hand: Option<Hand>,
    pub swing_progress: f32,
    pub item_use: Option<ItemUseAnimation>,
}

#[derive(Clone, Copy, Debug)]
pub struct ItemUseAnimation {
    pub hand: Hand,
    pub remaining_ticks: i32,
    /// `None` represents a held item with no implemented use animation.
    pub duration_ticks: Option<i32>,
    pub partial_tick: f32,
}

pub struct FirstPersonRenderer {
    pipeline: wgpu::RenderPipeline,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform: [wgpu::Buffer; 2],
    uniform_groups: [wgpu::BindGroup; 2],
    glint_group: wgpu::BindGroup,
    skin: Option<GpuSkin>,
    models: VecDeque<GpuModel>,
    main_left: bool,
    main_present: bool,
    main: Option<Arc<HeldModel>>,
    off: Option<Arc<HeldModel>>,
    visible: bool,
    main_equip: f32,
    off_equip: f32,
    swing_hand: Option<Hand>,
    swing_progress: f32,
    item_use: Option<ItemUseAnimation>,
}

// 26.3's Mth.sin indexes a 65536-entry table initialized using this divisor.
fn vanilla_sin(angle: f32) -> f32 {
    const DIVISOR: f64 = 10430.378350470453;
    let index = ((f64::from(angle) * DIVISOR) as i64) & 65535;
    ((index as f64) / DIVISOR).sin() as f32
}

fn vanilla_cos(angle: f32) -> f32 {
    const DIVISOR: f64 = 10430.378350470453;
    let index = ((f64::from(angle) * DIVISOR + 16384.0) as i64) & 65535;
    ((index as f64) / DIVISOR).sin() as f32
}

/// JOML computes cosine from the rounded f32 sine, which is observably
/// different from an independent cosine near right angles.
fn joml_sin_cos(degrees: f32) -> (f32, f32) {
    let angle = degrees * 0.017453292_f32;
    let sine = f64::from(angle).sin() as f32;
    let cosine = (1.0 - sine * sine).sqrt();
    let shifted = angle + std::f32::consts::FRAC_PI_2;
    let mut quadrant =
        shifted - (shifted / std::f32::consts::TAU) as i32 as f32 * std::f32::consts::TAU;
    if quadrant < 0.0 {
        quadrant += std::f32::consts::TAU;
    }
    (
        sine,
        if quadrant >= std::f32::consts::PI {
            -cosine
        } else {
            cosine
        },
    )
}

fn joml_rotate_x(degrees: f32) -> Mat4 {
    let (sine, cosine) = joml_sin_cos(degrees);
    Mat4::from_cols_array(&[
        1.0, 0.0, 0.0, 0.0, 0.0, cosine, sine, 0.0, 0.0, -sine, cosine, 0.0, 0.0, 0.0, 0.0, 1.0,
    ])
}

fn joml_rotate_y(degrees: f32) -> Mat4 {
    let (sine, cosine) = joml_sin_cos(degrees);
    Mat4::from_cols_array(&[
        cosine, 0.0, -sine, 0.0, 0.0, 1.0, 0.0, 0.0, sine, 0.0, cosine, 0.0, 0.0, 0.0, 0.0, 1.0,
    ])
}

fn joml_rotate_z(degrees: f32) -> Mat4 {
    let (sine, cosine) = joml_sin_cos(degrees);
    Mat4::from_cols_array(&[
        cosine, sine, 0.0, 0.0, -sine, cosine, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ])
}

fn item_display_transform(left: bool, display: DisplayTransform) -> Mat4 {
    let sign = if left { -1.0 } else { 1.0 };
    Mat4::from_translation(Vec3::new(
        display.translation.x * sign,
        display.translation.y,
        display.translation.z,
    )) * Mat4::from_quat(display.rotation(left))
        * Mat4::from_scale(display.scale)
        * Mat4::from_translation(Vec3::splat(-0.5))
}

fn item_transform(left: bool, display: DisplayTransform, equip: f32, swing: Option<f32>) -> Mat4 {
    let sign = if left { -1.0 } else { 1.0 };
    let mut result = Mat4::from_translation(Vec3::new(sign * 0.56, -0.52 - equip * 0.6, -0.72));
    if let Some(swing) = swing {
        let root = swing.sqrt();
        let root_sin = vanilla_sin(root * std::f32::consts::PI);
        let double_sin = vanilla_sin(root * std::f32::consts::TAU);
        let swing_sin = vanilla_sin(swing * std::f32::consts::PI);
        let square_sin = vanilla_sin(swing * swing * std::f32::consts::PI);
        result *= Mat4::from_translation(Vec3::new(
            -0.4 * sign * root_sin,
            0.2 * double_sin,
            -0.2 * swing_sin,
        )) * Mat4::from_rotation_y((sign * (45.0 - 20.0 * square_sin)).to_radians())
            * Mat4::from_rotation_z((-20.0 * sign * root_sin).to_radians())
            * Mat4::from_rotation_x((-80.0 * root_sin).to_radians())
            * Mat4::from_rotation_y((-45.0 * sign).to_radians());
    }
    result * item_display_transform(left, display)
}

/// 26.3 applies the eat motion before its ordinary equip and model transforms.
fn eating_item_transform(
    left: bool,
    display: DisplayTransform,
    equip: f32,
    remaining_ticks: i32,
    duration_ticks: i32,
    partial_tick: f32,
) -> Mat4 {
    let sign = if left { -1.0 } else { 1.0 };
    let remaining = remaining_ticks as f32 - partial_tick.clamp(0.0, 1.0) + 1.0;
    let fraction = remaining / duration_ticks as f32;
    let jiggle = if fraction < 0.8 {
        (vanilla_cos(remaining / 4.0 * std::f32::consts::PI) * 0.1).abs()
    } else {
        0.0
    };
    let amount = 1.0 - (f64::from(fraction).powf(27.0) as f32);
    Mat4::from_translation(Vec3::new(0.0, jiggle, 0.0))
        * Mat4::from_translation(Vec3::new(amount * 0.6 * sign, amount * -0.5, 0.0))
        * joml_rotate_y(sign * amount * 90.0)
        * joml_rotate_x(amount * 10.0)
        * joml_rotate_z(sign * amount * 30.0)
        * item_transform(left, display, equip, None)
}

fn arm_transform(left: bool, equip: f32, swing: f32) -> Mat4 {
    let sign = if left { -1.0 } else { 1.0 };
    let root = swing.sqrt();
    let root_sin = vanilla_sin(root * std::f32::consts::PI);
    let double_sin = vanilla_sin(root * std::f32::consts::TAU);
    let swing_sin = vanilla_sin(swing * std::f32::consts::PI);
    let square_sin = vanilla_sin(swing * swing * std::f32::consts::PI);
    Mat4::from_translation(Vec3::new(
        sign * (-0.3 * root_sin + 0.64000005),
        0.4 * double_sin - 0.6 - equip * 0.6,
        -0.4 * swing_sin - 0.71999997,
    )) * Mat4::from_rotation_y((sign * 45.0_f32).to_radians())
        * Mat4::from_rotation_y((sign * root_sin * 70.0).to_radians())
        * Mat4::from_rotation_z((sign * square_sin * -20.0).to_radians())
        * Mat4::from_translation(Vec3::new(-sign, 3.6, 3.5))
        * Mat4::from_rotation_z((sign * 120.0_f32).to_radians())
        * Mat4::from_rotation_x(200.0_f32.to_radians())
        * Mat4::from_rotation_y((-sign * 135.0_f32).to_radians())
        * Mat4::from_translation(Vec3::new(sign * 5.6, 0.0, 0.0))
}

impl FirstPersonRenderer {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        glint: Option<(&wgpu::TextureView, &wgpu::Sampler, &wgpu::Buffer)>,
    ) -> Self {
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("first-person transform layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("first-person texture layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let glint_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("first-person glint layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let fallback = glint.is_none().then(|| {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("first-person fallback glint"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
            let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("first-person fallback glint settings"),
                contents: bytemuck::cast_slice(&[0.0_f32; 4]),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            (view, sampler, uniform)
        });
        let (glint_view, glint_sampler, glint_uniform) = glint.unwrap_or_else(|| {
            let (view, sampler, uniform) = fallback.as_ref().expect("fallback glint created");
            (view, sampler, uniform)
        });
        let glint_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("first-person glint bindings"),
            layout: &glint_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(glint_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(glint_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: glint_uniform.as_entire_binding(),
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("first-person nearest sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let uniform = std::array::from_fn(|_| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("first-person transform"),
                size: 64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        let uniform_groups = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("first-person transform group"),
                layout: &uniform_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform[i].as_entire_binding(),
                }],
            })
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("first-person pipeline layout"),
            bind_group_layouts: &[
                Some(&uniform_layout),
                Some(&texture_layout),
                Some(&glint_layout),
            ],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("first-person shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("first_person.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("first-person pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<HandVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &HAND_ATTRIBUTES,
                })],
            },
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Self {
            pipeline,
            texture_layout,
            sampler,
            uniform,
            uniform_groups,
            glint_group,
            skin: None,
            models: VecDeque::new(),
            main_left: false,
            main_present: false,
            main: None,
            off: None,
            visible: false,
            main_equip: 0.0,
            off_equip: 0.0,
            swing_hand: None,
            swing_progress: 0.0,
            item_use: None,
        }
    }

    fn material(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        image: &RgbaImage,
        vertices: &[HandVertex],
    ) -> Material {
        let size = wgpu::Extent3d {
            width: image.width(),
            height: image.height(),
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("first-person texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            image.as_raw(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(image.width() * 4),
                rows_per_image: Some(image.height()),
            },
            size,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let texture = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("first-person texture group"),
            layout: &self.texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        Self::material_with_texture(device, texture, vertices)
    }

    fn material_with_texture(
        device: &wgpu::Device,
        texture: wgpu::BindGroup,
        vertices: &[HandVertex],
    ) -> Material {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("first-person vertices"),
            contents: bytemuck::cast_slice(vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        Material {
            vertices: buffer,
            count: vertices.len() as u32,
            texture,
        }
    }

    fn cache_model(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, source: Arc<HeldModel>) {
        if let Some(index) = self
            .models
            .iter()
            .position(|model| Arc::ptr_eq(&model.source, &source))
        {
            let model = self.models.remove(index).expect("cached index exists");
            self.models.push_back(model);
            return;
        }
        let mut groups: Vec<(u8, Arc<RgbaImage>, Vec<HandVertex>)> = Vec::new();
        for quad in &source.quads {
            let index = groups
                .iter()
                .position(|(layer, texture, _)| {
                    *layer == quad.layer && Arc::ptr_eq(texture, &quad.texture)
                })
                .unwrap_or_else(|| {
                    groups.push((quad.layer, Arc::clone(&quad.texture), Vec::new()));
                    groups.len() - 1
                });
            let a = Vec3::from_array(quad.vertices[0].position);
            let b = Vec3::from_array(quad.vertices[1].position);
            let c = Vec3::from_array(quad.vertices[2].position);
            let normal = (b - a).cross(c - a).normalize_or_zero();
            let shade = (0.6 + 0.4 * normal.dot(Vec3::new(0.2, 1.0, -0.7).normalize()).abs())
                .clamp(0.0, 1.0);
            let bounds = source
                .glint_materials
                .iter()
                .find(|(texture, _)| Arc::ptr_eq(texture, &quad.texture))
                .and_then(|(_, bounds)| *bounds);
            for i in [0, 1, 2, 0, 2, 3] {
                groups[index].2.push(HandVertex {
                    base: Vertex {
                        position: quad.vertices[i].position,
                        uv: quad.vertices[i].uv,
                        shade,
                        tint: quad.tint,
                        light: u32::MAX,
                    },
                    glint: hand_glint(source.glint, bounds, quad.vertices[i].uv),
                });
            }
        }
        let bytes = groups
            .iter()
            .map(|(_, image, vertices)| {
                u64::from(image.width()) * u64::from(image.height()) * 4
                    + vertices.len() as u64 * std::mem::size_of::<HandVertex>() as u64
            })
            .sum();
        let materials = groups
            .iter()
            .map(|(_, image, vertices)| self.material(device, queue, image, vertices))
            .collect();
        self.models.push_back(GpuModel {
            source,
            materials,
            bytes,
        });
        while self.models.len() > 16
            || (self.models.len() > 2
                && self.models.iter().map(|model| model.bytes).sum::<u64>() > 64 * 1024 * 1024)
        {
            self.models.pop_front();
        }
    }

    pub fn update(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, input: FirstPersonInput) {
        let FirstPersonInput {
            skin,
            main_left,
            main_present,
            main,
            off,
            visible,
            main_equip,
            off_equip,
            swing_hand,
            swing_progress,
            item_use,
        } = input;
        self.visible = visible;
        if !visible {
            return;
        }
        if self
            .skin
            .as_ref()
            .is_none_or(|cached| !Arc::ptr_eq(&cached.source, &skin))
        {
            let arm = |left| {
                player_preview::first_person_arm(skin.slim, left)
                    .into_iter()
                    .map(|base| HandVertex {
                        base,
                        glint: [0.0; 3],
                    })
                    .collect::<Vec<_>>()
            };
            let right = self.material(device, queue, &skin.image, &arm(false));
            let left = Self::material_with_texture(device, right.texture.clone(), &arm(true));
            self.skin = Some(GpuSkin {
                source: skin,
                right,
                left,
            });
        }
        for model in [&main, &off].into_iter().flatten() {
            self.cache_model(device, queue, Arc::clone(model));
        }
        self.main_left = main_left;
        self.main_present = main_present;
        self.main = main;
        self.off = off;
        self.main_equip = main_equip;
        self.off_equip = off_equip;
        self.swing_hand = swing_hand;
        self.swing_progress = swing_progress;
        self.item_use = item_use;
    }

    pub fn has_glint(&self) -> bool {
        self.visible
            && [&self.main, &self.off]
                .into_iter()
                .flatten()
                .any(|item| item.glint)
    }

    pub fn draw(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        projection: Mat4,
    ) {
        if !self.visible || self.skin.is_none() {
            return;
        }
        for left in [false, true] {
            let main = left == self.main_left;
            let item = if main { &self.main } else { &self.off };
            let hand = if main { Hand::Main } else { Hand::Off };
            let equip = if main {
                self.main_equip
            } else {
                self.off_equip
            };
            let transform = if let Some(item) = item {
                let display = if left {
                    item.first_left
                } else {
                    item.first_right
                };
                if let Some(usage) = self.item_use.filter(|usage| usage.hand == hand) {
                    if let Some(duration) = usage.duration_ticks.filter(|duration| *duration > 0) {
                        eating_item_transform(
                            left,
                            display,
                            equip,
                            usage.remaining_ticks,
                            duration,
                            usage.partial_tick,
                        )
                    } else {
                        item_transform(left, display, equip, None)
                    }
                } else {
                    item_transform(
                        left,
                        display,
                        equip,
                        (self.swing_hand == Some(hand)).then_some(self.swing_progress),
                    )
                }
            } else if main && !self.main_present {
                arm_transform(
                    left,
                    equip,
                    if self.swing_hand.unwrap_or(Hand::Main) == hand {
                        self.swing_progress
                    } else {
                        0.0
                    },
                )
            } else {
                continue;
            };
            let matrix = (projection * transform).to_cols_array_2d();
            queue.write_buffer(
                &self.uniform[usize::from(left)],
                0,
                bytemuck::cast_slice(&matrix),
            );
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("first-person pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(2, &self.glint_group, &[]);
        for left in [false, true] {
            let main = left == self.main_left;
            let item = if main { &self.main } else { &self.off };
            pass.set_bind_group(0, &self.uniform_groups[usize::from(left)], &[]);
            if let Some(item) = item {
                if let Some(model) = self.models.iter().find(|m| Arc::ptr_eq(&m.source, item)) {
                    for material in &model.materials {
                        draw_material(&mut pass, material);
                    }
                }
            } else if main && !self.main_present {
                let skin = self.skin.as_ref().expect("skin checked");
                draw_material(&mut pass, if left { &skin.left } else { &skin.right });
            }
        }
    }
}

fn draw_material(pass: &mut wgpu::RenderPass<'_>, material: &Material) {
    pass.set_bind_group(1, &material.texture, &[]);
    pass.set_vertex_buffer(0, material.vertices.slice(..));
    pass.draw(0..material.count, 0..1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glint_vertices_preserve_atlas_coordinates_and_disable_for_plain_items() {
        let uv = [0.25, 0.75];
        assert_eq!(hand_glint(false, Some([0.2, 0.3, 0.6, 0.7]), uv), [0.0; 3]);
        let mapped = hand_glint(true, Some([0.2, 0.3, 0.6, 0.7]), uv);
        assert!((mapped[0] - 0.3).abs() < 1e-6);
        assert!((mapped[1] - 0.6).abs() < 1e-6);
        assert_eq!(mapped[2], 1.0);
        assert_eq!(hand_glint(true, None, uv), [0.25, 0.75, 2.0]);
    }

    #[test]
    fn first_person_arm_includes_skin_and_sleeve_for_each_model_and_side() {
        for slim in [false, true] {
            for left in [false, true] {
                let vertices = player_preview::first_person_arm(slim, left);
                assert_eq!(vertices.len(), 72);
                assert!(
                    vertices
                        .iter()
                        .all(|v| v.position.iter().all(|v| v.is_finite()))
                );
                assert!(
                    vertices
                        .iter()
                        .all(|v| v.uv.iter().all(|v| (0.0..=1.0).contains(v)))
                );
                assert_ne!(vertices[0].uv, vertices[36].uv);
            }
        }
        let wide = player_preview::first_person_arm(false, false);
        let slim = player_preview::first_person_arm(true, false);
        assert!(
            wide.iter()
                .zip(slim)
                .any(|(wide, slim)| wide.position != slim.position)
        );
    }

    #[test]
    #[ignore = "requires a GPU; does not open a window"]
    fn gpu_draws_both_hands_in_front_of_world_and_hides_spectator() {
        use crate::held_items::TexturedQuad;
        use std::{sync::mpsc, time::Duration};
        let backend = match std::env::var("NATIVE_TEST_GPU_BACKEND").ok().as_deref() {
            Some("dx12") => wgpu::Backends::DX12,
            Some("vulkan") => wgpu::Backends::VULKAN,
            _ => wgpu::Backends::DX12 | wgpu::Backends::VULKAN,
        };
        pollster::block_on(async {
            let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
            descriptor.backends = backend;
            let instance = wgpu::Instance::new(descriptor);
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    compatible_surface: None,
                    force_fallback_adapter: false,
                    apply_limit_buckets: false,
                })
                .await
                .unwrap();
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .unwrap();
            let errors = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let skin = Arc::new(PlayerSkin {
                image: RgbaImage::from_pixel(64, 64, image::Rgba([255, 0, 0, 255])),
                slim: false,
            });
            let display = DisplayTransform {
                translation: Vec3::ZERO,
                rotation: Vec3::ZERO,
                scale: Vec3::splat(0.4),
            };
            let item_texture = Arc::new(RgbaImage::from_pixel(2, 2, image::Rgba([0, 255, 0, 255])));
            let make_item = |glint| {
                Arc::new(HeldModel {
                    particle_texture: None,
                    numeric_item_id: None,
                    glint,
                    glint_materials: if glint {
                        vec![(Arc::clone(&item_texture), Some([0.2, 0.35, 0.3, 0.45]))]
                    } else {
                        Vec::new()
                    },
                    quads: vec![TexturedQuad {
                        vertices: [
                            native_resources::geometry::Vertex {
                                position: [0.0, 0.0, 0.5],
                                uv: [0.0, 0.0],
                            },
                            native_resources::geometry::Vertex {
                                position: [1.0, 0.0, 0.5],
                                uv: [1.0, 0.0],
                            },
                            native_resources::geometry::Vertex {
                                position: [1.0, 1.0, 0.5],
                                uv: [1.0, 1.0],
                            },
                            native_resources::geometry::Vertex {
                                position: [0.0, 1.0, 0.5],
                                uv: [0.0, 1.0],
                            },
                        ],
                        normal: [0.0, 0.0, 1.0],
                        texture: Arc::clone(&item_texture),
                        tint: [1.0; 3],
                        layer: 0,
                    }],
                    right: display,
                    left: display,
                    first_right: display,
                    first_left: display,
                    ground: display,
                    gui: display,
                    gui_side_light: true,
                    gui_icon: None,
                    gui_glint_uv: None,
                    gui_glint_pixels: None,
                    gui_glint_quads: None,
                    sprite_layers: Vec::new(),
                })
            };
            let item = make_item(false);
            let enchanted = make_item(true);
            let glint_texture = device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: 64,
                    height: 64,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let glint_pixels = RgbaImage::from_fn(64, 64, |x, _| {
                image::Rgba(if x < 32 {
                    [240, 0, 200, 255]
                } else {
                    [10, 0, 10, 255]
                })
            });
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &glint_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                glint_pixels.as_raw(),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(64),
                },
                wgpu::Extent3d {
                    width: 64,
                    height: 64,
                    depth_or_array_layers: 1,
                },
            );
            let glint_view = glint_texture.create_view(&wgpu::TextureViewDescriptor::default());
            let glint_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::Repeat,
                ..Default::default()
            });
            let glint_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&[0.0_f32, 0.0, 0.75, 1.0]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            let creation_errors = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let mut renderer = FirstPersonRenderer::new(
                &device,
                wgpu::TextureFormat::Rgba8Unorm,
                Some((&glint_view, &glint_sampler, &glint_uniform)),
            );
            if let Some(error) = creation_errors.pop().await {
                panic!("first-person pipeline validation failed: {error}");
            }
            let size = wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            };
            let make_texture = |format, usage| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
            };
            let output = make_texture(
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            );
            let depth = make_texture(
                wgpu::TextureFormat::Depth32Float,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            );
            let view = output.create_view(&wgpu::TextureViewDescriptor::default());
            let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
            let mut idle_mask = Vec::new();
            let mut idle_item_mask = Vec::new();
            let mut idle_pixels = Vec::new();
            let mut glint_frame = Vec::new();
            for step in 0..7 {
                let visible = step != 3;
                queue.write_buffer(
                    &glint_uniform,
                    0,
                    bytemuck::cast_slice(&[
                        if step == 5 { 0.5_f32 } else { 0.0 },
                        0.0,
                        if step == 6 { 0.0 } else { 0.75 },
                        1.0,
                    ]),
                );
                renderer.update(
                    &device,
                    &queue,
                    FirstPersonInput {
                        skin: Arc::clone(&skin),
                        main_left: false,
                        main_present: false,
                        main: None,
                        off: Some(Arc::clone(if step >= 4 { &enchanted } else { &item })),
                        visible,
                        main_equip: if step == 1 { 0.3 } else { 0.0 },
                        off_equip: 0.0,
                        swing_hand: (step == 1).then_some(Hand::Main),
                        swing_progress: if step == 1 { 0.5 } else { 0.0 },
                        item_use: (step == 2).then_some(ItemUseAnimation {
                            hand: Hand::Off,
                            remaining_ticks: 20,
                            duration_ticks: Some(32),
                            partial_tick: 0.5,
                        }),
                    },
                );
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: 64 * 64 * 4,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                let mut encoder =
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
                {
                    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: None,
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLUE),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &depth_view,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Clear(0.0),
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        }),
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                }
                renderer.draw(
                    &queue,
                    &mut encoder,
                    &view,
                    &depth_view,
                    Mat4::perspective_rh(70.0_f32.to_radians(), 1.0, 0.05, 512.0),
                );
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: &output,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(256),
                            rows_per_image: Some(64),
                        },
                    },
                    size,
                );
                queue.submit([encoder.finish()]);
                let (sender, receiver) = mpsc::channel();
                buffer
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |result| {
                        let _ = sender.send(result);
                    });
                device
                    .poll(wgpu::PollType::Wait {
                        submission_index: None,
                        timeout: Some(Duration::from_secs(15)),
                    })
                    .unwrap();
                receiver
                    .recv_timeout(Duration::from_secs(15))
                    .unwrap()
                    .unwrap();
                let data = buffer.slice(..).get_mapped_range().unwrap();
                let red = data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .filter(|p| p[0] > 100 && p[1] == 0)
                    .count();
                let green = data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .filter(|p| p[1] > 100 && p[0] == 0)
                    .count();
                if step == 0 {
                    assert!(
                        red > 10 && green > 10,
                        "both hands should cover the world: red={red}, green={green}"
                    );
                    idle_mask = data
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|p| p[0] > 100 && p[1] == 0)
                        .collect();
                    idle_item_mask = data
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|p| p[1] > 100 && p[0] == 0)
                        .collect();
                    idle_pixels = data.to_vec();
                } else if step == 1 {
                    let animated_mask: Vec<_> = data
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|p| p[0] > 100 && p[1] == 0)
                        .collect();
                    assert_ne!(
                        idle_mask, animated_mask,
                        "the animated hand should move on the GPU"
                    );
                    assert!(green > 10, "the offhand item should remain visible");
                } else if step == 2 {
                    let eating_mask: Vec<_> = data
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|p| p[1] > 100 && p[0] == 0)
                        .collect();
                    assert_ne!(
                        idle_item_mask, eating_mask,
                        "the food should move on the GPU"
                    );
                    assert!(red > 10, "the other hand should remain visible");
                } else if step == 3 {
                    assert_eq!((red, green), (0, 0));
                } else if step == 4 {
                    glint_frame = data.to_vec();
                    let mut changed = 0;
                    for (index, (before, after)) in idle_pixels
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .zip(data.as_chunks::<4>().0.iter())
                        .enumerate()
                    {
                        if before != after {
                            assert!(
                                idle_item_mask[index],
                                "glint must not affect skin or background"
                            );
                            changed += 1;
                        }
                    }
                    assert!(changed > 5, "glint should affect the held item");
                } else if step == 5 {
                    assert_ne!(glint_frame, data.to_vec(), "glint must animate");
                } else {
                    assert_eq!(
                        idle_pixels,
                        data.to_vec(),
                        "zero strength restores the ordinary item"
                    );
                }
                drop(data);
                buffer.unmap();
            }
            assert_eq!(renderer.models.len(), 2);
            if let Some(error) = errors.pop().await {
                panic!("GPU validation failed: {error}");
            }
        });
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn animated_arm_and_item_transforms_match_vanilla_26_3() {
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
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.renderer.FirstPersonHandsAndItemsRenderer;
import net.minecraft.client.renderer.SubmitNodeCollector;
import net.minecraft.client.renderer.state.level.PlayerRenderState;
import net.minecraft.client.resources.model.cuboid.ItemTransform;
import net.minecraft.world.entity.HumanoidArm;
import org.joml.Vector3f;
public class FirstPersonOracle {
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var renderer = new FirstPersonHandsAndItemsRenderer(null);
    var apply = FirstPersonHandsAndItemsRenderer.class.getDeclaredMethod("applyItemArmTransform", PoseStack.class, HumanoidArm.class, float.class);
    var arm = FirstPersonHandsAndItemsRenderer.class.getDeclaredMethod("renderPlayerArm", PoseStack.class, SubmitNodeCollector.class, int.class, float.class, float.class, HumanoidArm.class, PlayerRenderState.class);
    var swingArm = FirstPersonHandsAndItemsRenderer.class.getDeclaredMethod("swingArm", float.class, PoseStack.class, int.class, HumanoidArm.class);
    var eat = FirstPersonHandsAndItemsRenderer.class.getDeclaredMethod("applyEatTransform", PoseStack.class, float.class, HumanoidArm.class, float.class, int.class);
    apply.setAccessible(true); arm.setAccessible(true); swingArm.setAccessible(true); eat.setAccessible(true);
    float[][] rotations={{0,0,0},{0,45,0},{19,-31,23}};
    float[][] translations={{0,0,0},{0,0,0},{0.25f,-0.1f,0.05f}};
    float[][] scales={{1,1,1},{0.4f,0.4f,0.4f},{1.2f,0.65f,0.9f}};
    float[][] points={{0,0,0},{1,1,1},{0.2f,0.7f,0.4f}};
    var out=new JsonArray();
    for(boolean left:new boolean[]{false,true}) {
      var which=left?HumanoidArm.LEFT:HumanoidArm.RIGHT;
      for(float equip:new float[]{0f,0.35f,1f}) for(float swing:new float[]{0f,0.0625f,0.25f,0.5f,0.75f,0.9f,1f})
      for(int i=-1;i<rotations.length;i++) {
        var pose=new PoseStack();
        if(i<0) {
          var state=new PlayerRenderState(); state.avatarRenderState=null;
          arm.invoke(renderer,pose,null,0,equip,swing,which,state);
        } else {
          apply.invoke(renderer,pose,which,equip);
          swingArm.invoke(renderer,swing,pose,left?-1:1,which);
          new ItemTransform(new Vector3f(rotations[i]),new Vector3f(translations[i]),new Vector3f(scales[i])).apply(left,pose.last());
        }
        var values=new JsonArray();
        for(float[] p:points) { var v=pose.last().pose().transformPosition(new Vector3f(p)); var a=new JsonArray(); a.add(v.x);a.add(v.y);a.add(v.z);values.add(a); }
        out.add(values);
      }
    }
    for(boolean left:new boolean[]{false,true}) {
      var which=left?HumanoidArm.LEFT:HumanoidArm.RIGHT;
      for(float equip:new float[]{0f,0.35f,1f}) for(int duration:new int[]{16,32,300})
      for(int remaining:duration==300?new int[]{1,75,150,299,300}:java.util.stream.IntStream.rangeClosed(1,duration).toArray()) for(float partial:new float[]{0f,0.25f,0.5f,0.75f,1f})
      for(int i=0;i<rotations.length;i++) {
        var pose=new PoseStack();
        eat.invoke(renderer,pose,partial,which,(float)remaining,duration);
        apply.invoke(renderer,pose,which,equip);
        new ItemTransform(new Vector3f(rotations[i]),new Vector3f(translations[i]),new Vector3f(scales[i])).apply(left,pose.last());
        var values=new JsonArray();
        for(float[] p:points) { var v=pose.last().pose().transformPosition(new Vector3f(p)); var a=new JsonArray(); a.add(v.x);a.add(v.y);a.add(v.z);values.add(a); }
        out.add(values);
      }
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(out));
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
                "first-person oracle failed:\n{}\n{}",
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
        let source = temp.path().join("FirstPersonOracle.java");
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
            .arg("FirstPersonOracle")
            .arg(&output));
        let expected: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let displays = [
            DisplayTransform {
                rotation: Vec3::ZERO,
                translation: Vec3::ZERO,
                scale: Vec3::ONE,
            },
            DisplayTransform {
                rotation: Vec3::new(0.0, 45.0, 0.0),
                translation: Vec3::ZERO,
                scale: Vec3::splat(0.4),
            },
            DisplayTransform {
                rotation: Vec3::new(19.0, -31.0, 23.0),
                translation: Vec3::new(0.25, -0.1, 0.05),
                scale: Vec3::new(1.2, 0.65, 0.9),
            },
        ];
        let points = [Vec3::ZERO, Vec3::ONE, Vec3::new(0.2, 0.7, 0.4)];
        let mut row = 0;
        for left in [false, true] {
            for equip in [0.0, 0.35, 1.0] {
                for swing in [0.0, 0.0625, 0.25, 0.5, 0.75, 0.9, 1.0] {
                    for transform in std::iter::once(arm_transform(left, equip, swing)).chain(
                        displays
                            .into_iter()
                            .map(|d| item_transform(left, d, equip, Some(swing))),
                    ) {
                        for (point_index, point) in points.into_iter().enumerate() {
                            for (axis, value) in transform
                                .transform_point3(point)
                                .to_array()
                                .into_iter()
                                .enumerate()
                            {
                                let java =
                                    expected[row][point_index][axis].as_f64().unwrap() as f32;
                                assert!(
                                    (value - java).abs() < 0.00003,
                                    "left={left} row={row} point={point_index} axis={axis}: {value} != {java}"
                                );
                            }
                        }
                        row += 1;
                    }
                }
            }
        }
        for left in [false, true] {
            for equip in [0.0, 0.35, 1.0] {
                for duration in [16, 32, 300] {
                    let samples = if duration == 300 {
                        vec![1, 75, 150, 299, 300]
                    } else {
                        (1..=duration).collect()
                    };
                    for remaining in samples {
                        for partial in [0.0, 0.25, 0.5, 0.75, 1.0] {
                            for display in displays {
                                let transform = eating_item_transform(
                                    left, display, equip, remaining, duration, partial,
                                );
                                for (point_index, point) in points.into_iter().enumerate() {
                                    for (axis, value) in transform
                                        .transform_point3(point)
                                        .to_array()
                                        .into_iter()
                                        .enumerate()
                                    {
                                        let java =
                                            expected[row][point_index][axis].as_f64().unwrap()
                                                as f32;
                                        assert!(
                                            (value - java).abs() < 0.00005,
                                            "eat left={left} equip={equip} remaining={remaining}/{duration} partial={partial} point={point_index} axis={axis}: {value} != {java}"
                                        );
                                    }
                                }
                                row += 1;
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(row, expected.len());
    }
}
