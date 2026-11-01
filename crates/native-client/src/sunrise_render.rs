//! Vanilla sunrise and sunset color fan, drawn before celestial sprites.
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

#[derive(Clone, Copy)]
pub struct Frame {
    pub angle_degrees: f32,
    pub color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 3],
    alpha: f32,
}

fn vertices() -> Vec<Vertex> {
    let center = Vertex {
        position: [0.0, 100.0, 0.0],
        alpha: 1.0,
    };
    let edge = |index: u32| {
        let angle = (index as f32 * std::f32::consts::TAU) / 16.0;
        let sin = native_session::physics::java_sin(f64::from(angle), false);
        let cos = native_session::physics::java_sin(f64::from(angle), true);
        Vertex {
            position: [sin * 120.0, cos * 120.0, -cos * 40.0],
            alpha: 0.0,
        }
    };
    (0..16)
        .flat_map(|i| [center, edge(i), edge(i + 1)])
        .collect()
}

fn transform(frame: Frame) -> Mat4 {
    let radians = frame.angle_degrees * 0.017_453_292_f32;
    let flipped = native_session::physics::java_sin(f64::from(radians), false) < 0.0;
    Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2)
        * Mat4::from_rotation_z(if flipped {
            270.0_f32.to_radians()
        } else {
            std::f32::consts::FRAC_PI_2
        })
        * Mat4::from_scale(Vec3::new(1.0, 1.0, frame.color[3]))
}

pub struct Sunrise {
    pipeline: wgpu::RenderPipeline,
    binding: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    vertices: wgpu::Buffer,
    frame: Option<Frame>,
    last_state: Option<[[f32; 4]; 5]>,
}

impl Sunrise {
    pub fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sunrise uniforms"),
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
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sunrise transform and color"),
            size: 80,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sunrise uniforms"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sunrise shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("sunrise_render.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sunrise pipeline"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sunrise fan"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 16,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32],
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
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: crate::star_render::FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sunrise fan vertices"),
            contents: bytemuck::cast_slice(&vertices()),
            usage: wgpu::BufferUsages::VERTEX,
        });
        Self {
            pipeline,
            binding,
            uniform,
            vertices,
            frame: None,
            last_state: None,
        }
    }
    pub fn set_frame(&mut self, frame: Option<Frame>) {
        self.frame = frame;
    }
    pub fn prepare(&mut self, queue: &wgpu::Queue, view_projection: Mat4) {
        if let Some(frame) = self.frame.filter(|frame| frame.color[3] > 0.001) {
            let matrix = (view_projection * transform(frame)).to_cols_array_2d();
            let state = [matrix[0], matrix[1], matrix[2], matrix[3], frame.color];
            if self.last_state != Some(state) {
                queue.write_buffer(&self.uniform, 0, bytemuck::cast_slice(&state));
                self.last_state = Some(state);
            }
        }
    }
    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        if self.frame.is_some_and(|frame| frame.color[3] > 0.001) {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.binding, &[]);
            pass.set_vertex_buffer(0, self.vertices.slice(..));
            pass.draw(0..48, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fan_faces_opposite_horizons_and_uses_vanilla_geometry() {
        let mesh = vertices();
        assert_eq!(mesh.len(), 48);
        assert_eq!(mesh[0].position, [0.0, 100.0, 0.0]);
        assert_eq!(mesh[0].alpha, 1.0);
        assert_eq!(mesh[1].position, [0.0, 120.0, -40.0]);
        assert_eq!(mesh[1].alpha, 0.0);
        let frame = Frame {
            angle_degrees: 90.0,
            color: [1.0, 0.5, 0.0, 0.5],
        };
        let east = transform(frame).transform_point3(Vec3::new(0.0, 100.0, 0.0));
        let west = transform(Frame {
            angle_degrees: 270.0,
            ..frame
        })
        .transform_point3(Vec3::new(0.0, 100.0, 0.0));
        assert!(east.x < -99.99 && west.x > 99.99, "{east:?} {west:?}");
    }

    #[test]
    #[ignore = "requires a GPU; renders offscreen without opening a window"]
    fn gpu_blends_sunrise_before_sun_and_respects_visibility() {
        use std::{io::Cursor, sync::mpsc, time::Duration};
        struct Assets(Vec<u8>);
        impl native_resources::packs::ResourceProvider for Assets {
            fn read_asset(&mut self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
                Ok(path.ends_with(".png").then(|| self.0.clone()))
            }
        }
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
            let mut sky = crate::star_render::Sky::new(
                &device,
                crate::star_render::FORMAT,
                wgpu::TextureFormat::Depth32Float,
            );
            let mut png = Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                1,
                1,
                image::Rgba([128, 64, 32, 128]),
            ))
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
            let texture = crate::sun_render::Texture::load(&mut Assets(png.into_inner())).unwrap();
            let mut sun = crate::sun_render::Sun::new(
                &device,
                &queue,
                crate::star_render::FORMAT,
                None,
                texture,
            )
            .unwrap();
            let size = wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            };
            let output = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("sunrise output"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: crate::star_render::FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let depth = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("sunrise depth"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("sunrise readback"),
                size: 64 * 64 * 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let matrix = Mat4::perspective_rh(20.0_f32.to_radians(), 1.0, 0.05, 1024.0)
                * Mat4::look_to_rh(Vec3::ZERO, Vec3::NEG_X, Vec3::Y);
            let body_frame = crate::sun_render::Frame {
                angle_degrees: 90.0,
                rain_brightness: 1.0,
                tick: 0,
            };
            sun.set_frame(Some(body_frame));
            sun.update(&queue, matrix);
            for alpha in [0.0_f32, 0.6] {
                sky.set_sunrise_frame(Some(Frame {
                    angle_degrees: 90.0,
                    color: [0.8, 0.3, 0.1, alpha],
                }));
                sky.prepare(&device, &queue, (64, 64), matrix);
                let mut encoder = device.create_command_encoder(&Default::default());
                sky.render(&mut encoder, [0.1, 0.2, 0.3], Some(&sun), None);
                {
                    let view = output.create_view(&Default::default());
                    let depth_view = depth.create_view(&Default::default());
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("sunrise composite"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &depth_view,
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
                    sky.composite(&mut pass);
                }
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: &output,
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
                let center = &pixels[(32 * 64 + 32) * 4..][..4];
                for i in 0..3 {
                    let background = [0.1_f32, 0.2, 0.3][i];
                    let sunrise = [0.8_f32, 0.3, 0.1][i];
                    let body = [128.0_f32, 64.0, 32.0][i] * (128.0 / 255.0);
                    let expected = ((background * (1.0 - alpha) + sunrise * alpha) * 255.0 + body)
                        .round() as i32;
                    assert!(
                        (i32::from(center[i]) - expected).abs() <= 3,
                        "alpha {alpha}, pixel {center:?}, expected {expected}"
                    );
                }
                assert_eq!(center[3], 255);
                drop(pixels);
                readback.unmap();
            }
        });
    }
}
