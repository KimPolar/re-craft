//! Resource-pack item sprites for food fragments.

use std::{collections::VecDeque, ops::Range, sync::Arc};

use anyhow::{Result, ensure};
use glam::{IVec3, Vec3};
use image::RgbaImage;

use crate::{consume_particles::Particle, mesh::Vertex};

pub(crate) struct RenderParticle {
    pub particle: Particle,
    pub texture: Arc<RgbaImage>,
    pub light: u32,
}

struct Material {
    image: Arc<RgbaImage>,
    binding: wgpu::BindGroup,
    bytes: usize,
}

struct Draw {
    binding: wgpu::BindGroup,
    vertices: Range<u32>,
}

pub(crate) struct ParticleRenderer {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    camera: wgpu::Buffer,
    cache: VecDeque<Material>,
    bytes: usize,
    buffer: Option<wgpu::Buffer>,
    capacity: usize,
    draws: Vec<Draw>,
}

impl ParticleRenderer {
    pub fn new(
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        camera: &wgpu::Buffer,
    ) -> Self {
        Self {
            layout: layout.clone(),
            sampler: sampler.clone(),
            camera: camera.clone(),
            cache: VecDeque::new(),
            bytes: 0,
            buffer: None,
            capacity: 0,
            draws: Vec::new(),
        }
    }

    fn material(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        image: &Arc<RgbaImage>,
    ) -> Result<wgpu::BindGroup> {
        if let Some(index) = self
            .cache
            .iter()
            .position(|entry| Arc::ptr_eq(&entry.image, image))
        {
            let entry = self.cache.remove(index).expect("cached particle texture");
            let binding = entry.binding.clone();
            self.cache.push_back(entry);
            return Ok(binding);
        }
        let width = image.width();
        let height = image.height().min(width);
        ensure!(
            width > 0 && height > 0 && width <= device.limits().max_texture_dimension_2d,
            "invalid particle texture"
        );
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("food particle texture"),
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
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            size,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("food particle material"),
            layout: &self.layout,
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
        let bytes = width as usize * height as usize * 4;
        const CACHE_LIMIT: usize = 64 * 1024 * 1024;
        while self.cache.len() >= 64 || self.bytes.saturating_add(bytes) > CACHE_LIMIT {
            let Some(old) = self.cache.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(old.bytes);
        }
        if bytes <= CACHE_LIMIT {
            self.bytes += bytes;
            self.cache.push_back(Material {
                image: Arc::clone(image),
                binding: binding.clone(),
                bytes,
            });
        }
        Ok(binding)
    }

    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        particles: &[RenderParticle],
        origin: IVec3,
        direction: Vec3,
        camera_up: Vec3,
        partial: f32,
    ) -> Result<()> {
        self.draws.clear();
        let direction = direction.normalize_or_zero();
        let right = direction.cross(camera_up).normalize_or_zero();
        let up = right.cross(direction).normalize_or_zero();
        let mut vertices = Vec::with_capacity(particles.len().saturating_mul(6));
        let mut previous: Option<Arc<RgbaImage>> = None;
        for entry in particles {
            if previous
                .as_ref()
                .is_none_or(|image| !Arc::ptr_eq(image, &entry.texture))
            {
                let binding = self.material(device, queue, &entry.texture)?;
                let start = u32::try_from(vertices.len())?;
                self.draws.push(Draw {
                    binding,
                    vertices: start..start,
                });
                previous = Some(Arc::clone(&entry.texture));
            }
            let p = &entry.particle;
            let center = (p
                .previous
                .lerp(p.position, f64::from(partial.clamp(0.0, 1.0)))
                - origin.as_dvec3())
            .as_vec3();
            let u0 = p.uv[0] / 4.0;
            let v0 = p.uv[1] / 4.0;
            let corners = [
                (-1.0, -1.0, u0, v0 + 0.25),
                (1.0, -1.0, u0 + 0.25, v0 + 0.25),
                (1.0, 1.0, u0 + 0.25, v0),
                (-1.0, 1.0, u0, v0),
            ];
            for i in [0, 1, 2, 0, 2, 3] {
                let (x, y, u, v) = corners[i];
                vertices.push(Vertex {
                    position: (center + (right * x + up * y) * p.size).to_array(),
                    uv: [u, v],
                    shade: 1.0,
                    tint: [1.0; 3],
                    light: entry.light,
                });
            }
            self.draws.last_mut().unwrap().vertices.end = u32::try_from(vertices.len())?;
        }
        let contents = bytemuck::cast_slice(&vertices);
        if contents.is_empty() {
            return Ok(());
        }
        if self.capacity < contents.len() {
            self.capacity = contents
                .len()
                .checked_next_power_of_two()
                .unwrap_or(contents.len());
            ensure!(
                self.capacity as u64 <= device.limits().max_buffer_size,
                "too many food particle vertices"
            );
            self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("food particle vertices"),
                size: self.capacity as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        queue.write_buffer(self.buffer.as_ref().unwrap(), 0, contents);
        Ok(())
    }

    pub fn draw<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        pipeline: &'a wgpu::RenderPipeline,
        lightmap: &'a wgpu::BindGroup,
        fog: &'a wgpu::BindGroup,
    ) {
        if self.draws.is_empty() {
            return;
        }
        let Some(buffer) = &self.buffer else { return };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(1, lightmap, &[]);
        pass.set_bind_group(2, fog, &[]);
        pass.set_vertex_buffer(0, buffer.slice(..));
        for draw in &self.draws {
            pass.set_bind_group(0, &draw.binding, &[]);
            pass.draw(draw.vertices.clone(), 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{consume_particles::ParticleSystem, renderer::VERTEX_ATTRIBUTES};
    use glam::DVec3;
    use native_persistence::nbt::Tag;
    use native_session::{
        SessionWorld, consume_particles::ConsumeParticleSpawn, inventory::ItemStack,
    };
    use std::{sync::mpsc, time::Duration};
    use wgpu::util::DeviceExt;

    #[test]
    #[ignore = "requires a GPU; renders offscreen without opening a window"]
    fn gpu_food_fragments_use_the_item_texture() {
        pollster::block_on(async {
            let instance =
                wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    compatible_surface: None,
                    force_fallback_adapter: false,
                    apply_limit_buckets: false,
                })
                .await
                .expect("GPU adapter unavailable");
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .unwrap();
            let errors = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
            let empty = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries: &[],
            });
            let empty_binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &empty,
                entries: &[],
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[Some(&layout), Some(&empty), Some(&empty)],
                immediate_size: 0,
            });
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(r#"
                @group(0) @binding(0) var<uniform> camera: mat4x4<f32>;
                @group(0) @binding(1) var tex: texture_2d<f32>;
                @group(0) @binding(2) var samp: sampler;
                struct Out { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
                @vertex fn vs(@location(0) pos: vec3<f32>, @location(1) uv: vec2<f32>) -> Out { var o: Out; o.pos = camera * vec4<f32>(pos, 1.0); o.uv = uv; return o; }
                @fragment fn fs(o: Out) -> @location(0) vec4<f32> { return textureSample(tex, samp, o.uv); }
            "#.into()) });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: None,
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &VERTEX_ATTRIBUTES,
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            });
            let camera = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&glam::Mat4::IDENTITY.to_cols_array()),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
            let mut renderer = ParticleRenderer::new(&layout, &sampler, &camera);
            let mut particles = ParticleSystem::default();
            let world = SessionWorld::Demo(native_world::World::demo());
            particles.update(
                1,
                &world,
                vec![ConsumeParticleSpawn {
                    tick: 1,
                    item: Arc::new(
                        ItemStack::new("minecraft:apple".into(), 1, 64, Tag::Compound(vec![]))
                            .unwrap(),
                    ),
                    position: DVec3::new(0.0, 0.0, 0.5),
                    velocity: DVec3::ZERO,
                }],
            );
            let image = Arc::new(RgbaImage::from_pixel(16, 16, image::Rgba([255, 0, 0, 255])));
            let entries = vec![RenderParticle {
                particle: particles.particles().next().unwrap().clone(),
                texture: image,
                light: u32::MAX,
            }];
            renderer
                .update(
                    &device,
                    &queue,
                    &entries,
                    IVec3::ZERO,
                    Vec3::NEG_Z,
                    Vec3::Y,
                    0.0,
                )
                .unwrap();
            let size = wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            };
            let output = device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 64 * 64 * 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            {
                let view = output.create_view(&wgpu::TextureViewDescriptor::default());
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                renderer.draw(&mut pass, &pipeline, &empty_binding, &empty_binding);
            }
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
            let (send, recv) = mpsc::channel();
            buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let _ = send.send(result);
                });
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: Some(Duration::from_secs(15)),
                })
                .unwrap();
            recv.recv_timeout(Duration::from_secs(15)).unwrap().unwrap();
            let data = buffer.slice(..).get_mapped_range().unwrap();
            assert!(
                data.chunks_exact(4)
                    .any(|pixel| pixel[0] > 200 && pixel[1] == 0 && pixel[2] == 0),
                "fragment was not rendered"
            );
            drop(data);
            buffer.unmap();
            if let Some(error) = errors.pop().await {
                panic!("GPU validation: {error}");
            }
        });
    }
}
