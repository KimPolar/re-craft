//! Celestial pass in an unorm target, preserving vanilla's sRGB-space additive blending.
use glam::Mat4;
use wgpu::util::DeviceExt;

pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

#[derive(Clone, Copy)]
pub struct Frame {
    pub angle_degrees: f32,
    pub brightness: f32,
}

struct Target {
    size: (u32, u32),
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    binding: wgpu::BindGroup,
}

pub struct Sky {
    sunrise: crate::sunrise_render::Sunrise,
    star_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    composite_layout: wgpu::BindGroupLayout,
    star_binding: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    vertices: wgpu::Buffer,
    vertex_count: u32,
    target: Option<Target>,
    frame: Option<Frame>,
    last_state: Option<[[f32; 4]; 5]>,
}

impl Sky {
    pub fn new(
        device: &wgpu::Device,
        output: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
    ) -> Self {
        let star_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("star uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sky composite texture"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("star transform"),
            size: 80,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let star_binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("star uniforms"),
            layout: &star_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let star_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("star shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("star_render.wgsl").into()),
        });
        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sky composite shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("sky_composite.wgsl").into()),
        });
        let star_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("star pipeline"),
            bind_group_layouts: &[Some(&star_layout)],
            immediate_size: 0,
        });
        let composite_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("sky composite pipeline"),
                bind_group_layouts: &[Some(&composite_layout)],
                immediate_size: 0,
            });
        let depth_state = || {
            Some(wgpu::DepthStencilState {
                format: depth,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            })
        };
        let star_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("stars"),
            layout: Some(&star_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &star_shader,
                entry_point: Some("star_vertex"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 12,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                })],
            },
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &star_shader,
                entry_point: Some("star_fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FORMAT,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::SrcAlpha,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::Zero,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let composite_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sky composite"),
            layout: Some(&composite_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &composite_shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: depth_state(),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &composite_shader,
                entry_point: Some(if output.is_srgb() {
                    "fragment_srgb"
                } else {
                    "fragment_unorm"
                }),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: output,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let mut triangles = Vec::new();
        for quad in crate::star_geometry::vertices().chunks_exact(4) {
            triangles.extend_from_slice(&[quad[0], quad[1], quad[2], quad[0], quad[2], quad[3]]);
        }
        let vertex_count = triangles.len() as u32;
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("fixed stars"),
            contents: bytemuck::cast_slice(&triangles),
            usage: wgpu::BufferUsages::VERTEX,
        });
        Self {
            sunrise: crate::sunrise_render::Sunrise::new(device),
            star_pipeline,
            composite_pipeline,
            composite_layout,
            star_binding,
            uniform,
            vertices,
            vertex_count,
            target: None,
            frame: None,
            last_state: None,
        }
    }

    pub fn set_frame(&mut self, frame: Option<Frame>) {
        self.frame = frame;
    }

    pub fn set_sunrise_frame(&mut self, frame: Option<crate::sunrise_render::Frame>) {
        self.sunrise.set_frame(frame);
    }

    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: (u32, u32),
        view_projection: Mat4,
    ) {
        self.sunrise.prepare(queue, view_projection);
        if self
            .target
            .as_ref()
            .is_none_or(|target| target.size != size)
        {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("celestial color"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("sky composite texture"),
                layout: &self.composite_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                }],
            });
            self.target = Some(Target {
                size,
                _texture: texture,
                view,
                binding,
            });
        }
        if let Some(frame) = self.frame.filter(|frame| frame.brightness > 0.0) {
            let transform = Mat4::from_rotation_y(-std::f32::consts::FRAC_PI_2)
                * Mat4::from_rotation_x(frame.angle_degrees * 0.017_453_292_f32);
            let matrix = (view_projection * transform).to_cols_array_2d();
            let state = [
                matrix[0],
                matrix[1],
                matrix[2],
                matrix[3],
                [frame.brightness; 4],
            ];
            if self.last_state != Some(state) {
                queue.write_buffer(&self.uniform, 0, bytemuck::cast_slice(&state));
                self.last_state = Some(state);
            }
        }
    }

    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        background: [f32; 3],
        sun: Option<&crate::sun_render::Sun>,
        moon: Option<&crate::sun_render::Sun>,
    ) {
        let target = self.target.as_ref().expect("prepared sky");
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("celestial pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: background[0] as f64,
                        g: background[1] as f64,
                        b: background[2] as f64,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        self.sunrise.draw(&mut pass);
        if let Some(sun) = sun {
            sun.draw(&mut pass);
        }
        if let Some(moon) = moon {
            moon.draw(&mut pass);
        }
        if self.frame.is_some_and(|frame| frame.brightness > 0.0) {
            pass.set_pipeline(&self.star_pipeline);
            pass.set_bind_group(0, &self.star_binding, &[]);
            pass.set_vertex_buffer(0, self.vertices.slice(..));
            pass.draw(0..self.vertex_count, 0..1);
        }
    }

    pub fn composite<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_pipeline(&self.composite_pipeline);
        pass.set_bind_group(0, &self.target.as_ref().expect("prepared sky").binding, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use sha1::{Digest, Sha1};
    use std::{io::Cursor, sync::mpsc, time::Duration};

    struct Assets(Vec<u8>);
    impl native_resources::packs::ResourceProvider for Assets {
        fn read_asset(&mut self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
            Ok(
                (path.starts_with("assets/minecraft/textures/environment/celestial/")
                    && path.ends_with(".png"))
                .then(|| self.0.clone()),
            )
        }
    }

    #[test]
    #[ignore = "requires a GPU; measures offscreen 1080p submission and completion time"]
    fn gpu_sky_cost_1080p() {
        use std::time::Instant;
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
            let info = adapter.get_info();
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .unwrap();
            let size = wgpu::Extent3d {
                width: 1920,
                height: 1080,
                depth_or_array_layers: 1,
            };
            let format = wgpu::TextureFormat::Rgba8UnormSrgb;
            let color = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("sky benchmark output"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let depth = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("sky benchmark depth"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let view = color.create_view(&Default::default());
            let depth_view = depth.create_view(&Default::default());
            let mut sky = Sky::new(&device, format, wgpu::TextureFormat::Depth32Float);
            let matrix = Mat4::perspective_rh(70_f32.to_radians(), 1920.0 / 1080.0, 0.05, 1024.0)
                * Mat4::look_to_rh(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y);
            let background = [0.1, 0.2, 0.3];
            println!(
                "adapter: {} ({:?}), 1920x1080, 30 warmup + 90 timed frames, synchronous completion",
                info.name, info.backend
            );
            let scenarios = [
                ("clear", 0.0, false),
                ("sky day", 0.0, true),
                ("sky night", 0.5, true),
            ];
            let mut timings = [
                Vec::with_capacity(90),
                Vec::with_capacity(90),
                Vec::with_capacity(90),
            ];
            for frame in 0..120 {
                for n in 0..scenarios.len() {
                    let index = (frame + n) % scenarios.len();
                    let (_, brightness, enabled) = scenarios[index];
                    let start = Instant::now();
                    if enabled {
                        sky.set_frame(Some(Frame {
                            angle_degrees: 0.0,
                            brightness,
                        }));
                        sky.prepare(&device, &queue, (1920, 1080), matrix);
                    }
                    let mut encoder = device.create_command_encoder(&Default::default());
                    if enabled {
                        sky.render(&mut encoder, background, None, None);
                    }
                    {
                        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("sky benchmark composite"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &view,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: Some(
                                wgpu::RenderPassDepthStencilAttachment {
                                    view: &depth_view,
                                    depth_ops: Some(wgpu::Operations {
                                        load: wgpu::LoadOp::Clear(1.0),
                                        store: wgpu::StoreOp::Store,
                                    }),
                                    stencil_ops: None,
                                },
                            ),
                            timestamp_writes: None,
                            occlusion_query_set: None,
                            multiview_mask: None,
                        });
                        if enabled {
                            sky.composite(&mut pass);
                        }
                    }
                    queue.submit([encoder.finish()]);
                    device
                        .poll(wgpu::PollType::Wait {
                            submission_index: None,
                            timeout: Some(Duration::from_secs(15)),
                        })
                        .unwrap();
                    if frame >= 30 {
                        timings[index].push(start.elapsed().as_secs_f64() * 1000.0);
                    }
                }
            }
            for ((label, _, _), mut samples) in scenarios.into_iter().zip(timings) {
                samples.sort_by(f64::total_cmp);
                let mean = samples.iter().sum::<f64>() / samples.len() as f64;
                println!(
                    "{label}: mean {mean:.3} ms, median {:.3} ms, p95 {:.3} ms",
                    samples[45], samples[85]
                );
            }
        });
    }

    #[test]
    #[ignore = "requires a GPU; renders offscreen without opening a window"]
    fn gpu_renders_and_composites_stars_in_both_color_formats() {
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
            let points = crate::star_geometry::vertices();
            let center = points
                .chunks_exact(4)
                .map(|quad| quad.iter().map(|v| Vec3::from_array(*v)).sum::<Vec3>() / 4.0)
                .find(|center| center.x.abs() < 5.0)
                .unwrap();
            let star_angle = (-center.z).atan2(center.y);
            let direction = (Mat4::from_rotation_y(-std::f32::consts::FRAC_PI_2)
                * Mat4::from_rotation_x(star_angle))
            .transform_vector3(center)
            .normalize();
            let up = if direction.dot(Vec3::Y).abs() > 0.95 {
                Vec3::Z
            } else {
                Vec3::Y
            };
            for format in [
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureFormat::Rgba8UnormSrgb,
            ] {
                let errors = device.push_error_scope(wgpu::ErrorFilter::Validation);
                let mut sky = Sky::new(&device, format, wgpu::TextureFormat::Depth32Float);
                let mut png = Cursor::new(Vec::new());
                image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                    1,
                    1,
                    image::Rgba([128, 64, 32, 128]),
                ))
                .write_to(&mut png, image::ImageFormat::Png)
                .unwrap();
                let mut assets = Assets(png.into_inner());
                let mut sun = crate::sun_render::Sun::new(
                    &device,
                    &queue,
                    FORMAT,
                    None,
                    crate::sun_render::Texture::load(&mut assets).unwrap(),
                )
                .unwrap();
                let mut moon = crate::sun_render::Sun::new(
                    &device,
                    &queue,
                    FORMAT,
                    None,
                    crate::sun_render::Texture::load_moon(&mut assets).unwrap(),
                )
                .unwrap();
                if let Some(error) = errors.pop().await {
                    panic!("GPU pipeline validation failed: {error}");
                }
                sky.prepare(&device, &queue, (32, 32), Mat4::IDENTITY);
                assert_eq!(sky.target.as_ref().unwrap().size, (32, 32));
                for brightness in [0.0_f32, 0.5] {
                    for with_bodies in [false, true] {
                        let size = wgpu::Extent3d {
                            width: 64,
                            height: 64,
                            depth_or_array_layers: 1,
                        };
                        let color = device.create_texture(&wgpu::TextureDescriptor {
                            label: Some("sky test output"),
                            size,
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: wgpu::TextureDimension::D2,
                            format,
                            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                                | wgpu::TextureUsages::COPY_SRC,
                            view_formats: &[],
                        });
                        let depth = device.create_texture(&wgpu::TextureDescriptor {
                            label: Some("sky test depth"),
                            size,
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: wgpu::TextureDimension::D2,
                            format: wgpu::TextureFormat::Depth32Float,
                            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                            view_formats: &[],
                        });
                        let view = color.create_view(&Default::default());
                        let depth_view = depth.create_view(&Default::default());
                        let readback = device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("sky readback"),
                            size: 64 * 64 * 4,
                            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                            mapped_at_creation: false,
                        });
                        let matrix = Mat4::perspective_rh(0.5_f32.to_radians(), 1.0, 0.05, 1024.0)
                            * Mat4::look_to_rh(Vec3::ZERO, direction, up);
                        let background = [0.1, 0.2, 0.3];
                        sky.set_frame(Some(Frame {
                            angle_degrees: star_angle.to_degrees(),
                            brightness,
                        }));
                        sky.prepare(&device, &queue, (64, 64), matrix);
                        assert_eq!(sky.target.as_ref().unwrap().size, (64, 64));
                        let body_frame = crate::sun_render::Frame {
                            angle_degrees: 0.0,
                            rain_brightness: 1.0,
                            tick: 0,
                        };
                        sun.set_frame(Some(body_frame));
                        sun.update(&queue, matrix);
                        moon.set_frame(Some(body_frame));
                        moon.update(&queue, matrix);
                        let mut encoder = device.create_command_encoder(&Default::default());
                        sky.render(
                            &mut encoder,
                            background,
                            with_bodies.then_some(&sun),
                            with_bodies.then_some(&moon),
                        );
                        {
                            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                                label: Some("sky composite test"),
                                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                    view: &view,
                                    depth_slice: None,
                                    resolve_target: None,
                                    ops: wgpu::Operations {
                                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                        store: wgpu::StoreOp::Store,
                                    },
                                })],
                                depth_stencil_attachment: Some(
                                    wgpu::RenderPassDepthStencilAttachment {
                                        view: &depth_view,
                                        depth_ops: Some(wgpu::Operations {
                                            load: wgpu::LoadOp::Clear(1.0),
                                            store: wgpu::StoreOp::Store,
                                        }),
                                        stencil_ops: None,
                                    },
                                ),
                                timestamp_writes: None,
                                occlusion_query_set: None,
                                multiview_mask: None,
                            });
                            sky.composite(&mut pass);
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
                                    rows_per_image: Some(64),
                                },
                            },
                            size,
                        );
                        queue.submit([encoder.finish()]);
                        let (sender, receiver) = mpsc::channel();
                        readback
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
                        let pixels = readback.slice(..).get_mapped_range().unwrap();
                        println!(
                            "sky hash {format:?} {brightness} {with_bodies}: {:x}",
                            Sha1::digest(pixels.as_ref())
                        );
                        let center = &pixels[(32 * 64 + 32) * 4..][..4];
                        for i in 0..3 {
                            let body = if with_bodies {
                                [128.0_f32, 64.0, 32.0][i] * (128.0 / 255.0) * 2.0
                            } else {
                                0.0
                            };
                            let expected =
                                (background[i] * 255.0 + brightness * brightness * 255.0 + body)
                                    .round() as i32;
                            assert!(
                                (i32::from(center[i]) - expected).abs() <= 1,
                                "{format:?}, brightness {brightness}, bodies {with_bodies}: {center:?}, expected {expected}"
                            );
                        }
                        assert_eq!(center[3], 255);
                        drop(pixels);
                        readback.unmap();
                    }
                }
            }
        });
    }
}
