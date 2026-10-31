//! Dropped-item model transforms and instanced rendering.

use std::{
    collections::{HashMap, VecDeque},
    ops::Range,
    sync::Arc,
};

use anyhow::{Result, ensure};
use bytemuck::{Pod, Zeroable};
use glam::{IVec3, Mat4, Vec3};
use image::RgbaImage;
use native_core::random::LegacyRandom;
use native_session::{inventory::ItemStack, items::DroppedItem};
use wgpu::util::DeviceExt;

use crate::{held_items::HeldModel, mesh::Vertex};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ModelVertex {
    base: Vertex,
    glint: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Instance {
    transform: [[f32; 4]; 4],
}

const MODEL_ATTRIBUTES: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
    0 => Float32x3, 1 => Float32x2, 2 => Float32, 3 => Float32x3, 9 => Uint32, 4 => Float32x3
];
const INSTANCE_ATTRIBUTES: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
    5 => Float32x4, 6 => Float32x4, 7 => Float32x4, 8 => Float32x4
];

fn display_matrix(model: &HeldModel) -> Mat4 {
    let display = model.ground;
    Mat4::from_translation(display.translation)
        * Mat4::from_quat(display.rotation(false))
        * Mat4::from_scale(display.scale)
        * Mat4::from_translation(Vec3::splat(-0.5))
}

fn model_bounds(model: &HeldModel, display: Mat4) -> (Vec3, Vec3) {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for vertex in model.quads.iter().flat_map(|quad| &quad.vertices) {
        let point = display.transform_point3(Vec3::from_array(vertex.position));
        min = min.min(point);
        max = max.max(point);
    }
    if min.is_finite() && max.is_finite() {
        (min, max)
    } else {
        (Vec3::ZERO, Vec3::ZERO)
    }
}

fn rendered_amount(count: u32) -> usize {
    match count {
        0..=1 => 1,
        2..=16 => 2,
        17..=32 => 3,
        33..=48 => 4,
        _ => 5,
    }
}

/// Use Java's numeric ID when available; keep a stable fallback for unknown items.
fn item_seed(item: &ItemStack, numeric_id: Option<i32>) -> i32 {
    let base = numeric_id.unwrap_or_else(|| {
        item.id().bytes().fold(0x811c_9dc5_u32, |hash, byte| {
            (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
        }) as i32
    });
    let Some(damage) = item
        .components()
        .get("minecraft:damage")
        .and_then(|value| value.as_java_int())
        .filter(|damage| *damage > 0)
    else {
        return base;
    };
    let maximum = if item.components().get("!minecraft:max_damage").is_some() {
        0
    } else {
        item.components()
            .get("minecraft:max_damage")
            .and_then(|value| value.as_java_int())
            .or_else(|| {
                native_session::inventory::ordinary_item_default_durability(item.id())
                    .or_else(|| {
                        native_session::equippable::vanilla_equipment(item.id())
                            .and_then(|item| item.2)
                    })
                    .and_then(|value| i32::try_from(value).ok())
            })
            .unwrap_or(0)
            .max(0)
    };
    base.wrapping_add(damage.min(maximum))
}

fn cluster_offsets(count: u32, seed: i32, depth: f32) -> ([Vec3; 5], usize) {
    let amount = rendered_amount(count);
    let mut random = LegacyRandom::new(i64::from(seed));
    let mut offsets = [Vec3::ZERO; 5];
    if depth > 0.0625 {
        for target in offsets.iter_mut().take(amount).skip(1) {
            let mut offset = || (random.next_float() * 2.0 - 1.0) * 0.15;
            *target = Vec3::new(offset(), offset(), offset());
        }
    } else {
        let step = depth * 1.5;
        let start = -(step * (amount - 1) as f32 / 2.0);
        offsets[0] = Vec3::new(0.0, 0.0, start);
        for (index, target) in offsets.iter_mut().enumerate().take(amount).skip(1) {
            let mut offset = || (random.next_float() * 2.0 - 1.0) * 0.15 * 0.5;
            *target = Vec3::new(offset(), offset(), start + step * index as f32);
        }
    }
    (offsets, amount)
}

#[cfg(test)]
fn instances(
    item: &DroppedItem,
    origin: IVec3,
    partial: f32,
    display: Mat4,
    bounds: (Vec3, Vec3),
) -> Vec<Instance> {
    instances_with_seed(
        item,
        origin,
        partial,
        display,
        bounds,
        item_seed(&item.stack, None),
    )
}

#[cfg(test)]
fn instances_with_seed(
    item: &DroppedItem,
    origin: IVec3,
    partial: f32,
    display: Mat4,
    bounds: (Vec3, Vec3),
    seed: i32,
) -> Vec<Instance> {
    let mut result = Vec::with_capacity(rendered_amount(item.stack.count()));
    append_instances(item, origin, partial, display, bounds, seed, &mut result);
    result
}

fn append_instances(
    item: &DroppedItem,
    origin: IVec3,
    partial: f32,
    display: Mat4,
    bounds: (Vec3, Vec3),
    seed: i32,
    result: &mut Vec<Instance>,
) {
    let partial = partial.clamp(0.0, 1.0);
    let position = (item
        .previous_position
        .lerp(item.position, f64::from(partial))
        - origin.as_dvec3())
    .as_vec3();
    let age = item.age as f32 + partial;
    let phase = age / 10.0 + item.bob_offset;
    let bob = native_session::physics::java_sin(f64::from(phase), false) * 0.1 + 0.1;
    let spin = age / 20.0 + item.bob_offset;
    let base = Mat4::from_translation(position + Vec3::Y * (bob - bounds.0.y + 0.0625))
        * Mat4::from_rotation_y(spin);
    let (offsets, amount) = cluster_offsets(item.stack.count(), seed, bounds.1.z - bounds.0.z);
    result.extend(offsets.into_iter().take(amount).map(|offset| Instance {
        transform: (base * Mat4::from_translation(offset) * display).to_cols_array_2d(),
    }));
}

struct Material {
    bind_group: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    count: u32,
}

struct GpuModel {
    source: Arc<HeldModel>,
    materials: Vec<Material>,
    display: Mat4,
    bounds: (Vec3, Vec3),
    bytes: u64,
}

struct Draw {
    model: Arc<GpuModel>,
    instances: Range<u32>,
}

pub struct GroundItemRenderer {
    pipeline: wgpu::RenderPipeline,
    material_layout: wgpu::BindGroupLayout,
    glint_group: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    camera: wgpu::Buffer,
    cache: VecDeque<Arc<GpuModel>>,
    draws: Vec<Draw>,
    instance_buffer: Option<wgpu::Buffer>,
    instance_capacity: usize,
    has_glint: bool,
}

impl GroundItemRenderer {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        material_layout: &wgpu::BindGroupLayout,
        camera: &wgpu::Buffer,
        glint: (&wgpu::TextureView, &wgpu::Sampler, &wgpu::Buffer),
    ) -> Self {
        let glint_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ground item glint layout"),
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
        let glint_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ground item glint bindings"),
            layout: &glint_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(glint.0),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(glint.1),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: glint.2.as_entire_binding(),
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ground item pipeline layout"),
            bind_group_layouts: &[Some(material_layout), Some(&glint_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ground item shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("ground_items.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ground item pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<ModelVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &MODEL_ATTRIBUTES,
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Instance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &INSTANCE_ATTRIBUTES,
                    }),
                ],
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ground item nearest sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        Self {
            pipeline,
            material_layout: material_layout.clone(),
            glint_group,
            sampler,
            camera: camera.clone(),
            cache: VecDeque::new(),
            draws: Vec::new(),
            instance_buffer: None,
            instance_capacity: 0,
            has_glint: false,
        }
    }

    fn load(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: Arc<HeldModel>,
    ) -> Arc<GpuModel> {
        // The previous frame keeps currently visible models alive even when
        // they exceed the bounded idle cache; avoid re-uploading them.
        if let Some(draw) = self
            .draws
            .iter()
            .find(|draw| Arc::ptr_eq(&draw.model.source, &source))
        {
            return Arc::clone(&draw.model);
        }
        if let Some(index) = self
            .cache
            .iter()
            .position(|model| Arc::ptr_eq(&model.source, &source))
        {
            let model = self.cache.remove(index).expect("cached model");
            self.cache.push_back(Arc::clone(&model));
            return model;
        }
        let mut groups: Vec<(u8, Arc<RgbaImage>, Vec<ModelVertex>)> = Vec::new();
        for quad in &source.quads {
            let group = groups
                .iter()
                .position(|(layer, image, _)| {
                    *layer == quad.layer && Arc::ptr_eq(image, &quad.texture)
                })
                .unwrap_or_else(|| {
                    groups.push((quad.layer, Arc::clone(&quad.texture), Vec::new()));
                    groups.len() - 1
                });
            let normal = Vec3::from_array(quad.normal).normalize_or_zero();
            let shade = (0.6 + 0.4 * normal.dot(Vec3::new(0.2, 1.0, -0.7).normalize()).abs())
                .clamp(0.0, 1.0);
            let bounds = source
                .glint_materials
                .iter()
                .find(|(image, _)| Arc::ptr_eq(image, &quad.texture))
                .and_then(|(_, bounds)| *bounds);
            for corner in [0, 1, 2, 0, 2, 3] {
                let vertex = quad.vertices[corner];
                let glint = if !source.glint {
                    [0.0; 3]
                } else if let Some([u0, v0, u1, v1]) = bounds {
                    [
                        u0 + (u1 - u0) * vertex.uv[0],
                        v0 + (v1 - v0) * vertex.uv[1],
                        1.0,
                    ]
                } else {
                    [vertex.uv[0], vertex.uv[1], 2.0]
                };
                groups[group].2.push(ModelVertex {
                    base: Vertex {
                        position: vertex.position,
                        uv: vertex.uv,
                        shade,
                        tint: quad.tint,
                        light: u32::MAX,
                    },
                    glint,
                });
            }
        }
        let mut bytes = 0_u64;
        let materials = groups
            .into_iter()
            .map(|(_, image, vertices)| {
                bytes += u64::from(image.width()) * u64::from(image.height()) * 4
                    + vertices.len() as u64 * std::mem::size_of::<ModelVertex>() as u64;
                let size = wgpu::Extent3d {
                    width: image.width(),
                    height: image.height(),
                    depth_or_array_layers: 1,
                };
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("ground item material"),
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
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("ground item material bindings"),
                    layout: &self.material_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: self.camera.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                });
                let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("ground item model vertices"),
                    contents: bytemuck::cast_slice(&vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });
                Material {
                    bind_group,
                    vertices: buffer,
                    count: vertices.len() as u32,
                }
            })
            .collect();
        let display = display_matrix(&source);
        let bounds = model_bounds(&source, display);
        let model = Arc::new(GpuModel {
            source,
            materials,
            display,
            bounds,
            bytes,
        });
        self.cache.push_back(Arc::clone(&model));
        while self.cache.len() > 64
            || (self.cache.len() > 2
                && self.cache.iter().map(|model| model.bytes).sum::<u64>() > 64 * 1024 * 1024)
        {
            self.cache.pop_front();
        }
        model
    }

    #[cfg(test)]
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        items: &[DroppedItem],
        models: &[Option<Arc<HeldModel>>],
        origin: IVec3,
        partial: f32,
    ) -> Result<()> {
        self.update_with_falling(device, queue, items, models, &[], &[], origin, partial)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_with_falling(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        items: &[DroppedItem],
        models: &[Option<Arc<HeldModel>>],
        falling: &[native_session::falling_block::FallingBlockVisual],
        falling_models: &[Option<Arc<HeldModel>>],
        origin: IVec3,
        partial: f32,
    ) -> Result<()> {
        ensure!(
            items.len() == models.len(),
            "dropped item model count mismatch"
        );
        ensure!(
            falling.len() == falling_models.len(),
            "falling block model count mismatch"
        );
        self.has_glint = false;
        let mut groups: Vec<(Arc<GpuModel>, Vec<Instance>)> = Vec::new();
        let mut group_index = HashMap::new();
        for (item, source) in items
            .iter()
            .zip(models)
            .filter_map(|(item, source)| source.as_ref().map(|source| (item, source)))
        {
            self.has_glint |= source.glint;
            let identity = Arc::as_ptr(source);
            let index = if let Some(&index) = group_index.get(&identity) {
                index
            } else {
                let gpu = self.load(device, queue, Arc::clone(source));
                groups.push((gpu, Vec::new()));
                let index = groups.len() - 1;
                group_index.insert(identity, index);
                index
            };
            let (model, batch) = &mut groups[index];
            append_instances(
                item,
                origin,
                partial,
                model.display,
                model.bounds,
                item_seed(&item.stack, source.numeric_item_id),
                batch,
            );
        }
        for (block, source) in falling
            .iter()
            .zip(falling_models)
            .filter_map(|(block, source)| source.as_ref().map(|source| (block, source)))
        {
            let identity = Arc::as_ptr(source);
            let index = if let Some(&index) = group_index.get(&identity) {
                index
            } else {
                let gpu = self.load(device, queue, Arc::clone(source));
                groups.push((gpu, Vec::new()));
                let index = groups.len() - 1;
                group_index.insert(identity, index);
                index
            };
            groups[index].1.push(Instance {
                transform: Mat4::from_translation(block.model_origin(origin, partial).as_vec3())
                    .to_cols_array_2d(),
            });
        }
        self.draws.clear();
        let mut all = Vec::new();
        for (model, batch) in groups {
            let start = u32::try_from(all.len())?;
            all.extend(batch);
            self.draws.push(Draw {
                model,
                instances: start..u32::try_from(all.len())?,
            });
        }
        let bytes = bytemuck::cast_slice(&all);
        if bytes.is_empty() {
            return Ok(());
        }
        if self.instance_capacity < bytes.len() {
            self.instance_capacity = bytes.len().next_power_of_two();
            ensure!(
                self.instance_capacity as u64 <= device.limits().max_buffer_size,
                "too many dropped item instances for the GPU"
            );
            self.instance_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ground item instances"),
                size: self.instance_capacity as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        queue.write_buffer(
            self.instance_buffer.as_ref().expect("item instance buffer"),
            0,
            bytes,
        );
        Ok(())
    }

    pub fn has_glint(&self) -> bool {
        self.has_glint
    }

    #[cfg(test)]
    pub fn resource_identities(&self) -> Vec<usize> {
        self.draws
            .iter()
            .map(|draw| Arc::as_ptr(&draw.model) as usize)
            .collect()
    }

    pub fn clear(&mut self) {
        self.draws.clear();
        self.has_glint = false;
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        let Some(instances) = &self.instance_buffer else {
            return;
        };
        if self.draws.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(1, &self.glint_group, &[]);
        pass.set_vertex_buffer(1, instances.slice(..));
        for draw in &self.draws {
            for material in &draw.model.materials {
                pass.set_bind_group(0, &material.bind_group, &[]);
                pass.set_vertex_buffer(0, material.vertices.slice(..));
                pass.draw(0..material.count, draw.instances.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::held_items::DisplayTransform;
    use native_persistence::nbt::Tag;

    fn model() -> HeldModel {
        let texture = Arc::new(RgbaImage::from_pixel(
            2,
            2,
            image::Rgba([255, 255, 255, 255]),
        ));
        let display = DisplayTransform {
            translation: Vec3::new(0.0, 0.125, 0.0),
            rotation: Vec3::ZERO,
            scale: Vec3::splat(0.5),
        };
        HeldModel {
            numeric_item_id: None,
            quads: crate::generated_item::bake(texture, [1.0; 3], 0).unwrap(),
            glint: false,
            glint_materials: Vec::new(),
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
        }
    }

    #[test]
    fn ground_transform_uses_model_extents_and_interpolated_entity_position() {
        let model = model();
        let display = display_matrix(&model);
        let bounds = model_bounds(&model, display);
        assert!((bounds.0.y + 0.125).abs() < 1e-6);
        assert!((bounds.1.y - 0.375).abs() < 1e-6);
        assert!((bounds.1.z - bounds.0.z - 0.03125).abs() < 1e-6);
        let item = DroppedItem {
            entity_id: 0,
            item_age: 0,
            position: glam::DVec3::new(3.0, 2.0, 0.0),
            previous_position: glam::DVec3::new(1.0, 2.0, 0.0),
            stack: ItemStack::new("minecraft:diamond".into(), 33, 64, Tag::Compound(vec![]))
                .unwrap(),
            age: 0,
            bob_offset: 0.0,
        };
        let result = instances(&item, IVec3::ZERO, 0.5, display, bounds);
        assert_eq!(result.len(), 4);
        let center =
            Mat4::from_cols_array_2d(&result[0].transform).transform_point3(Vec3::splat(0.5));
        assert!((center.x - 2.0).abs() < 0.01);
        assert!(center.y > 2.0);
        assert_eq!(rendered_amount(1), 1);
        assert_eq!(rendered_amount(16), 2);
        assert_eq!(rendered_amount(17), 3);
        assert_eq!(rendered_amount(33), 4);
        assert_eq!(rendered_amount(49), 5);
    }

    #[test]
    fn cluster_seed_clamps_damage_to_the_item_maximum() {
        let item = |id: &str, damage| {
            ItemStack::new(
                id.into(),
                1,
                1,
                Tag::Compound(vec![("minecraft:damage".into(), Tag::Int(damage))]),
            )
            .unwrap()
        };
        let base = item_seed(&item("minecraft:diamond_sword", 0), None);
        assert_eq!(item_seed(&item("minecraft:diamond_sword", -10), None), base);
        assert_eq!(
            item_seed(&item("minecraft:diamond_sword", 100), None),
            base.wrapping_add(100)
        );
        assert_eq!(
            item_seed(&item("minecraft:diamond_sword", 10000), None),
            base.wrapping_add(1561)
        );
        assert_eq!(
            item_seed(&item("minecraft:dirt", 100), None),
            item_seed(&item("minecraft:dirt", 0), None)
        );
        assert_eq!(
            item_seed(&item("minecraft:diamond_sword", 100), Some(42)),
            142
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn installed_item_seeds_and_damage_match_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut stack = native_resources::packs::ResourceStack::open(&game).unwrap();
        let registry = stack.vanilla_item_registry().unwrap();
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let classpath = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("ItemSeedOracle.java");
        let output = temp.path().join("output.json");
        std::fs::write(&source, include_str!("../tests/java/ItemSeedOracle.java")).unwrap();
        let run = |command: &mut Command| {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "Java item seed oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&classpath)
            .arg(&source));
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&classpath)
            .arg("ItemSeedOracle")
            .arg(&output));
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), 30);
        for row in rows {
            let name = row["name"].as_str().unwrap();
            let damage = row["damage"].as_i64().unwrap() as i32;
            let limit = row["limit"].as_i64().unwrap() as i32;
            let mut components = vec![("minecraft:damage".into(), Tag::Int(damage))];
            if limit >= 0 {
                components.push(("minecraft:max_damage".into(), Tag::Int(limit)));
            }
            let item = ItemStack::new(name.into(), 1, 64, Tag::Compound(components)).unwrap();
            assert_eq!(
                item_seed(&item, registry.id(name)),
                row["seed"].as_i64().unwrap() as i32,
                "{name} damage={damage} maximum={limit}"
            );
        }
    }

    #[test]
    #[ignore = "CPU measurement of dropped-item instance transforms"]
    fn measure_ground_item_instance_transforms() {
        use std::time::Instant;
        let model = model();
        let display = display_matrix(&model);
        let bounds = model_bounds(&model, display);
        let stack =
            ItemStack::new("minecraft:diamond".into(), 17, 64, Tag::Compound(vec![])).unwrap();
        let items = (0..1000)
            .map(|index| DroppedItem {
                entity_id: 0,
                item_age: 0,
                position: glam::DVec3::new(f64::from(index % 32), 10.0, f64::from(index / 32)),
                previous_position: glam::DVec3::new(
                    f64::from(index % 32),
                    9.99,
                    f64::from(index / 32),
                ),
                stack: stack.clone(),
                age: index as u32,
                bob_offset: (index as f32 * 0.1) % std::f32::consts::TAU,
            })
            .collect::<Vec<_>>();
        let mut times = Vec::new();
        let mut result = Vec::with_capacity(items.len() * 3);
        for _ in 0..55 {
            let started = Instant::now();
            for _ in 0..20 {
                result.clear();
                for item in &items {
                    append_instances(
                        item,
                        IVec3::ZERO,
                        0.5,
                        display,
                        bounds,
                        item_seed(&item.stack, Some(42)),
                        &mut result,
                    );
                }
                std::hint::black_box(&result);
            }
            let elapsed = started.elapsed().as_secs_f64() * 1000.0 / 20.0;
            assert_eq!(result.len(), 3000);
            times.push(elapsed);
        }
        times.drain(..5);
        times.sort_by(f64::total_cmp);
        eprintln!(
            "1000 items, 3000 instances: CPU median {:.3} ms, p95 {:.3} ms; {} bytes of instance data",
            times[25],
            times[47],
            result.len() * std::mem::size_of::<Instance>()
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn cluster_offsets_match_installed_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let classpath = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
            .join("bin");
        let source = temp.path().join("GroundItemOracle.java");
        let output = temp.path().join("output.json");
        std::fs::write(&source, include_str!("../tests/java/GroundItemOracle.java")).unwrap();
        let run = |command: &mut Command| {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "ground item Java oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&classpath)
            .arg(&source));
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&classpath)
            .arg("GroundItemOracle")
            .arg(&output));
        let expected: Vec<Vec<[f32; 3]>> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let seeds = [0, 1, -1, 187, 1337, i32::MAX];
        let counts = [1, 2, 16, 17, 32, 33, 48, 49, 64];
        let depths = [0.0, 0.03125, 0.0625, 0.0625001, 0.25, 1.0];
        assert_eq!(expected.len(), seeds.len() * counts.len() * depths.len());
        let mut row = 0;
        for seed in seeds {
            for count in counts {
                for depth in depths {
                    let (actual, amount) = cluster_offsets(count, seed, depth);
                    assert_eq!(
                        amount,
                        expected[row].len(),
                        "seed={seed}, count={count}, depth={depth}"
                    );
                    for (actual, expected) in actual.into_iter().take(amount).zip(&expected[row]) {
                        assert!(
                            (actual - Vec3::from_array(*expected)).abs().max_element() < 0.000_001,
                            "seed={seed}, count={count}, depth={depth}, actual={actual:?}, Java={expected:?}"
                        );
                    }
                    row += 1;
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn complete_ground_model_poses_match_installed_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let classpath = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
            .join("bin");
        let source = temp.path().join("GroundItemPoseOracle.java");
        let output = temp.path().join("output.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/GroundItemPoseOracle.java"),
        )
        .unwrap();
        let run = |command: &mut Command| {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "ground item pose Java oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&classpath)
            .arg(&source));
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&classpath)
            .arg("GroundItemPoseOracle")
            .arg(&output));
        let expected: Vec<Vec<[[f32; 3]; 3]>> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let mut row = 0;
        let points = [Vec3::ZERO, Vec3::splat(0.5), Vec3::ONE];
        for shape in 0..2 {
            let mut model = model();
            model.ground = DisplayTransform {
                translation: Vec3::new(0.05, 0.125, -0.1),
                rotation: if shape == 0 {
                    Vec3::ZERO
                } else {
                    Vec3::new(15.0, 30.0, 10.0)
                },
                scale: Vec3::new(0.5, 0.4, 0.6),
            };
            if shape == 1 {
                for quad in &mut model.quads {
                    for vertex in &mut quad.vertices {
                        vertex.position[2] = if vertex.position[2] < 0.5 { 0.0 } else { 1.0 };
                    }
                }
            }
            let display = display_matrix(&model);
            let bounds = model_bounds(&model, display);
            for age in [0.0_f32, 0.5, 19.75, 120.25] {
                for bob_offset in [0.0, 1.25, 5.2] {
                    for count in [1, 17, 64] {
                        for seed in [0, 1337] {
                            let item = DroppedItem {
                                entity_id: 0,
                                item_age: 0,
                                position: glam::DVec3::ZERO,
                                previous_position: glam::DVec3::ZERO,
                                stack: ItemStack::new(
                                    "minecraft:diamond".into(),
                                    count,
                                    64,
                                    Tag::Compound(vec![]),
                                )
                                .unwrap(),
                                age: age as u32,
                                bob_offset,
                            };
                            let actual = instances_with_seed(
                                &item,
                                IVec3::ZERO,
                                age.fract(),
                                display,
                                bounds,
                                seed,
                            );
                            assert_eq!(actual.len(), expected[row].len());
                            for (instance, oracle) in actual.into_iter().zip(&expected[row]) {
                                let transform = Mat4::from_cols_array_2d(&instance.transform);
                                for (point, expected) in points.into_iter().zip(oracle) {
                                    let actual = transform.transform_point3(point);
                                    assert!(
                                        (actual - Vec3::from_array(*expected)).abs().max_element()
                                            < 0.000_005,
                                        "shape={shape} age={age} bob={bob_offset} count={count} seed={seed}, point={point:?}: actual={actual:?}, Java={expected:?}"
                                    );
                                }
                            }
                            row += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(row, expected.len());
    }
}
