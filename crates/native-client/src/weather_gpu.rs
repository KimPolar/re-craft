//! GPU buffers and resource-pack textures for precipitation columns.

use anyhow::{Result, ensure};
use wgpu::util::DeviceExt;

use crate::weather_render::{WeatherMesh, WeatherTextures, WeatherVertex};

pub const ATTRIBUTES: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32, 3 => Uint32];

pub fn pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    depth: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("weather shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("weather_shader.wgsl").into()),
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("weather pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vertex"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<WeatherVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &ATTRIBUTES,
            })],
        },
        primitive: wgpu::PrimitiveState {
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: depth,
            depth_write_enabled: Some(false),
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
    })
}

pub struct WeatherGpu {
    rain_binding: wgpu::BindGroup,
    snow_binding: wgpu::BindGroup,
    buffer: Option<wgpu::Buffer>,
    capacity: usize,
    rain_count: u32,
    snow_count: u32,
}

impl WeatherGpu {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        camera: &wgpu::Buffer,
        textures: &WeatherTextures,
    ) -> Result<Self> {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("weather repeating sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let binding = |image: &image::RgbaImage| -> Result<wgpu::BindGroup> {
            ensure!(
                image.width() <= device.limits().max_texture_dimension_2d
                    && image.height() <= device.limits().max_texture_dimension_2d,
                "weather texture exceeds GPU limits"
            );
            let extent = wgpu::Extent3d {
                width: image.width(),
                height: image.height(),
                depth_or_array_layers: 1,
            };
            let texture = device.create_texture_with_data(
                queue,
                &wgpu::TextureDescriptor {
                    label: Some("weather texture"),
                    size: extent,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                image.as_raw(),
            );
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            Ok(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("weather texture binding"),
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
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            }))
        };
        Ok(Self {
            rain_binding: binding(&textures.rain)?,
            snow_binding: binding(&textures.snow)?,
            buffer: None,
            capacity: 0,
            rain_count: 0,
            snow_count: 0,
        })
    }

    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mesh: &WeatherMesh,
    ) -> Result<()> {
        self.rain_count = u32::try_from(mesh.rain.len())?;
        self.snow_count = u32::try_from(mesh.snow.len())?;
        let vertices = mesh
            .rain
            .len()
            .checked_add(mesh.snow.len())
            .ok_or_else(|| anyhow::anyhow!("too many weather vertices"))?;
        let size = vertices
            .checked_mul(std::mem::size_of::<WeatherVertex>())
            .ok_or_else(|| anyhow::anyhow!("weather buffer overflow"))?;
        ensure!(
            size as u64 <= device.limits().max_buffer_size,
            "weather buffer exceeds GPU limits"
        );
        if size == 0 {
            return Ok(());
        }
        if self.capacity < size {
            self.capacity = size
                .checked_next_power_of_two()
                .unwrap_or(size)
                .min(device.limits().max_buffer_size as usize);
            self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("weather vertices"),
                size: self.capacity as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let buffer = self.buffer.as_ref().unwrap();
        if !mesh.rain.is_empty() {
            queue.write_buffer(buffer, 0, bytemuck::cast_slice(&mesh.rain));
        }
        if !mesh.snow.is_empty() {
            queue.write_buffer(
                buffer,
                (mesh.rain.len() * std::mem::size_of::<WeatherVertex>()) as u64,
                bytemuck::cast_slice(&mesh.snow),
            );
        }
        Ok(())
    }

    pub fn draw<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        pipeline: &'a wgpu::RenderPipeline,
        lightmap: &'a wgpu::BindGroup,
        fog: &'a wgpu::BindGroup,
    ) {
        let Some(buffer) = &self.buffer else { return };
        if self.rain_count == 0 && self.snow_count == 0 {
            return;
        }
        pass.set_pipeline(pipeline);
        pass.set_bind_group(1, lightmap, &[]);
        pass.set_bind_group(2, fog, &[]);
        pass.set_vertex_buffer(0, buffer.slice(..));
        if self.rain_count > 0 {
            pass.set_bind_group(0, &self.rain_binding, &[]);
            pass.draw(0..self.rain_count, 0..1);
        }
        if self.snow_count > 0 {
            pass.set_bind_group(0, &self.snow_binding, &[]);
            pass.draw(self.rain_count..self.rain_count + self.snow_count, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};
    use std::{sync::mpsc, time::Duration};

    #[test]
    #[ignore = "requires a GPU; renders offscreen without opening a window"]
    fn offscreen_rain_and_snow_use_their_own_textures() {
        pollster::block_on(async {
            let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
            descriptor.backends = match std::env::var("NATIVE_TEST_GPU_BACKEND").ok().as_deref() {
                Some("dx12") => wgpu::Backends::DX12,
                Some("vulkan") => wgpu::Backends::VULKAN,
                _ => wgpu::Backends::DX12 | wgpu::Backends::VULKAN,
            };
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
            let buffer_entry = |binding, visibility| wgpu::BindGroupLayoutEntry {
                binding,
                visibility,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            };
            let texture_layout =
                device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: None,
                    entries: &[
                        buffer_entry(0, wgpu::ShaderStages::VERTEX),
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
            let light_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries: &[buffer_entry(0, wgpu::ShaderStages::VERTEX)],
            });
            let fog_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries: &[buffer_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT)],
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[
                    Some(&texture_layout),
                    Some(&light_layout),
                    Some(&fog_layout),
                ],
                immediate_size: 0,
            });
            let pipeline = pipeline(
                &device,
                &layout,
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureFormat::Depth32Float,
            );
            let camera_data = [
                1.0_f32, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
                2.0, 1.0, 0.0, 0.0,
            ];
            let camera = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&camera_data),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let lights = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&[[1.0_f32; 4]; 256]),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let fog = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&[0.0_f32; 12]),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let make_uniform = |layout, buffer: &wgpu::Buffer| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    }],
                })
            };
            let light_binding = make_uniform(&light_layout, &lights);
            let fog_binding = make_uniform(&fog_layout, &fog);
            let textures = WeatherTextures {
                rain: RgbaImage::from_pixel(1, 1, Rgba([255, 0, 0, 255])),
                snow: RgbaImage::from_pixel(1, 1, Rgba([0, 0, 255, 255])),
            };
            let mut gpu =
                WeatherGpu::new(&device, &queue, &texture_layout, &camera, &textures).unwrap();
            let quad = |left: f32, right: f32| {
                let v = |x, y| WeatherVertex {
                    position: [x, y, 0.5],
                    uv: [0.5, 0.5],
                    opacity: 1.0,
                    light: 0,
                };
                let (a, b, c, d) = (v(left, -1.0), v(right, -1.0), v(right, 1.0), v(left, 1.0));
                vec![a, b, c, a, c, d]
            };
            let mesh = WeatherMesh {
                rain: quad(-1.0, 0.0),
                snow: quad(0.0, 1.0),
            };
            gpu.update(&device, &queue, &mesh).unwrap();
            let extent = wgpu::Extent3d {
                width: 2,
                height: 1,
                depth_or_array_layers: 1,
            };
            let color = device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let depth = device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 256,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &color.create_view(&Default::default()),
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth.create_view(&Default::default()),
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
                gpu.draw(&mut pass, &pipeline, &light_binding, &fog_binding);
            }
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &color,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256),
                        rows_per_image: Some(1),
                    },
                },
                extent,
            );
            queue.submit([encoder.finish()]);
            let (sender, receiver) = mpsc::channel();
            readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                let _ = sender.send(r);
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
            let data = readback.slice(..).get_mapped_range().unwrap();
            assert_eq!(&data[..8], &[255, 0, 0, 255, 0, 0, 255, 255]);
            drop(data);
            readback.unmap();
            let mut faded = WeatherMesh {
                rain: quad(-1.0, 0.0),
                snow: quad(0.0, 1.0),
            };
            for vertex in &mut faded.rain {
                vertex.opacity = 0.05;
            }
            gpu.update(&device, &queue, &faded).unwrap();
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &color.create_view(&Default::default()),
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth.create_view(&Default::default()),
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
                gpu.draw(&mut pass, &pipeline, &light_binding, &fog_binding);
            }
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &color,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256),
                        rows_per_image: Some(1),
                    },
                },
                extent,
            );
            queue.submit([encoder.finish()]);
            let (sender, receiver) = mpsc::channel();
            readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                let _ = sender.send(r);
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
            let data = readback.slice(..).get_mapped_range().unwrap();
            assert_eq!(
                &data[..8],
                &[0, 0, 0, 255, 0, 0, 255, 255],
                "pixels below the 0.1 alpha cutoff must be discarded"
            );
            drop(data);
            readback.unmap();
            if let Some(error) = errors.pop().await {
                panic!("GPU validation: {error}");
            }
        });
    }
}
