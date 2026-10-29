//! GPU breaking overlay using textures from the active Minecraft resource stack.

use anyhow::{Result, ensure};
use glam::IVec3;
use image::{GenericImage, RgbaImage, imageops::FilterType};
use native_session::mining::BreakingProgress;

use crate::{
    mesh::{self, Vertex},
    renderer::VERTEX_ATTRIBUTES,
};

fn stage_atlas(stages: &[RgbaImage], limit: u32) -> Result<RgbaImage> {
    ensure!(stages.len() == 10, "expected ten block-breaking textures");
    ensure!(
        stages
            .iter()
            .all(|image| image.width() > 0 && image.width() == image.height()),
        "invalid block-breaking texture dimensions"
    );
    let tile = stages
        .iter()
        .map(RgbaImage::width)
        .max()
        .unwrap()
        .min(limit / 5);
    ensure!(tile > 0, "GPU cannot support a breaking texture atlas");
    let mut atlas = RgbaImage::new(tile * 5, tile * 2);
    for (index, image) in stages.iter().enumerate() {
        let resized;
        let image = if image.width() == tile {
            image
        } else {
            resized = image::imageops::resize(image, tile, tile, FilterType::Nearest);
            &resized
        };
        atlas.copy_from(image, index as u32 % 5 * tile, index as u32 / 5 * tile)?;
    }
    Ok(atlas)
}

pub struct BreakingRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    current: Option<(BreakingProgress, IVec3)>,
    visible: bool,
}

impl BreakingRenderer {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        pipeline_layout: &wgpu::PipelineLayout,
        camera: &wgpu::Buffer,
        sampler: &wgpu::Sampler,
        shader: &wgpu::ShaderModule,
        format: wgpu::TextureFormat,
        stages: &[RgbaImage],
    ) -> Result<Self> {
        let atlas = stage_atlas(stages, device.limits().max_texture_dimension_2d)?;
        let size = wgpu::Extent3d {
            width: atlas.width(),
            height: atlas.height(),
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("block breaking stages"),
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
            atlas.as_raw(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(atlas.width() * 4),
                rows_per_image: Some(atlas.height()),
            },
            size,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("breaking stage bind group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });
        // Vanilla 26.3's crumbling blend multiplies source and destination colors.
        let blend = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Dst,
                dst_factor: wgpu::BlendFactor::Src,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::Zero,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("block breaking overlay"),
            layout: Some(pipeline_layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("world_vertex"),
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
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: wgpu::DepthBiasState {
                    constant: -1,
                    slope_scale: -1.0,
                    clamp: 0.0,
                },
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("world_fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("block breaking vertices"),
            size: (36 * std::mem::size_of::<Vertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Ok(Self {
            pipeline,
            bind_group,
            vertices,
            current: None,
            visible: false,
        })
    }

    pub fn update(
        &mut self,
        queue: &wgpu::Queue,
        progress: Option<BreakingProgress>,
        origin: IVec3,
    ) -> Result<()> {
        let current = progress.map(|progress| (progress, origin));
        self.visible = current.is_some();
        if current != self.current {
            if let Some((progress, origin)) = current {
                let vertices = mesh::breaking_cube(progress.position, origin, progress.stage);
                ensure!(vertices.len() == 36, "invalid breaking decal geometry");
                queue.write_buffer(&self.vertices, 0, bytemuck::cast_slice(&vertices));
            }
            self.current = current;
        }
        Ok(())
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        if self.visible {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, self.vertices.slice(..));
            pass.draw(0..36, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_atlas_keeps_each_resource_pack_stage_separate() -> Result<()> {
        let stages: Vec<_> = (0..10)
            .map(|stage| {
                RgbaImage::from_pixel(
                    if stage == 9 { 2 } else { 1 },
                    if stage == 9 { 2 } else { 1 },
                    image::Rgba([stage, 0, 0, 255]),
                )
            })
            .collect();
        let atlas = stage_atlas(&stages, 10)?;
        assert_eq!(atlas.dimensions(), (10, 4));
        for stage in 0..10_u32 {
            assert_eq!(
                atlas.get_pixel(stage % 5 * 2, stage / 5 * 2).0,
                [stage as u8, 0, 0, 255]
            );
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires a GPU; renders offscreen without opening a window"]
    fn gpu_breaking_overlay_uses_the_crumbling_blend_and_depth() {
        use std::{sync::mpsc, time::Duration};
        use wgpu::util::DeviceExt;

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
                .expect("GPU adapter unavailable");
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .unwrap();
            let errors = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("breaking test layout"),
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
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("breaking test pipeline layout"),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("breaking test shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
            });
            let mut uniform = [0_f32; 20];
            uniform[0] = 1.0;
            uniform[5] = 1.0;
            uniform[10] = 1.0;
            uniform[15] = 1.0;
            uniform[16] = 64.0;
            uniform[17] = 64.0;
            let camera = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("breaking test camera"),
                contents: bytemuck::cast_slice(&uniform),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Nearest,
                min_filter: wgpu::FilterMode::Nearest,
                ..Default::default()
            });
            let stages: Vec<_> = (0..10)
                .map(|_| RgbaImage::from_pixel(2, 2, image::Rgba([128, 128, 128, 255])))
                .collect();
            let mut breaking = BreakingRenderer::new(
                &device,
                &queue,
                &layout,
                &pipeline_layout,
                &camera,
                &sampler,
                &shader,
                wgpu::TextureFormat::Rgba8Unorm,
                &stages,
            )
            .unwrap();
            breaking
                .update(
                    &queue,
                    Some(BreakingProgress {
                        position: IVec3::ZERO,
                        stage: 7,
                    }),
                    IVec3::ZERO,
                )
                .unwrap();
            let size = wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            };
            let make_texture = |label, format, usage| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
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
                "breaking test output",
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            );
            let depth = make_texture(
                "breaking test depth",
                wgpu::TextureFormat::Depth32Float,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            );
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("breaking test readback"),
                size: 64 * 64 * 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            {
                let view = output.create_view(&wgpu::TextureViewDescriptor::default());
                let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("breaking test pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.5,
                                g: 0.5,
                                b: 0.5,
                                a: 1.0,
                            }),
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
                breaking.draw(&mut pass);
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
            let pixel = |x: usize, y: usize| data[(y * 64 + x) * 4];
            assert!(
                (pixel(10, 10) as i32 - 128).abs() <= 1,
                "background: {}",
                pixel(10, 10)
            );
            assert!(
                (pixel(48, 16) as i32 - 55).abs() <= 2,
                "decal: {}",
                pixel(48, 16)
            );
            drop(data);
            buffer.unmap();
            if let Some(error) = errors.pop().await {
                panic!("GPU validation failed: {error}");
            }
        });
    }
}
