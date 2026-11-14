use super::*;
use crate::held_items::{DisplayTransform, TexturedQuad};
use native_resources::geometry::Vertex as ModelPoint;
use std::{sync::mpsc, time::Duration};

fn rectangle_model(color: [u8; 4], width: f32) -> Arc<HeldModel> {
    let display = DisplayTransform {
        translation: Vec3::ZERO,
        rotation: Vec3::ZERO,
        scale: Vec3::ONE,
    };
    let points = [
        ([0.0, 0.0, 0.5], [0.0, 1.0]),
        ([0.0, 1.0, 0.5], [0.0, 0.0]),
        ([width, 1.0, 0.5], [1.0, 0.0]),
        ([width, 0.0, 0.5], [1.0, 1.0]),
    ];
    Arc::new(HeldModel {
        quads: vec![TexturedQuad {
            vertices: points.map(|(position, uv)| ModelPoint { position, uv }),
            normal: [0.0, 0.0, 1.0],
            texture: Arc::new(RgbaImage::from_pixel(2, 2, image::Rgba(color))),
            tint: [1.0; 3],
            layer: 0,
        }],
        particle_texture: None,
        numeric_item_id: None,
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
    })
}

#[test]
#[ignore = "requires a GPU; renders offscreen without opening a window"]
fn moving_block_parts_render_and_share_gpu_models() {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
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
        let mut camera_data = [0.0f32; 20];
        camera_data[..16].copy_from_slice(&Mat4::IDENTITY.to_cols_array());
        let camera = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&camera_data),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let glint_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
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
        let glint_view = glint_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let glint_sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
        let glint_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&[0.0f32; 4]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let mut renderer = GroundItemRenderer::new(
            &device,
            wgpu::TextureFormat::Rgba8Unorm,
            &layout,
            &camera,
            (&glint_view, &glint_sampler, &glint_uniform),
        );
        let red = rectangle_model([255, 0, 0, 255], 1.0);
        let green = rectangle_model([0, 255, 0, 255], 0.5);
        let falling = [-0.5, 0.5].map(|x| native_session::falling_block::FallingBlockVisual {
            previous_position: glam::DVec3::new(x, -0.5, 0.5),
            position: glam::DVec3::new(x, -0.5, 0.5),
            kind: native_session::falling_block::FallingKind::Sand,
            block_state: None,
            start_position: None,
        });
        let models = [
            Some(vec![Arc::clone(&red), Arc::clone(&green)]),
            Some(vec![Arc::clone(&red)]),
        ];
        renderer
            .update_with_falling(
                &device,
                &queue,
                &[],
                &[],
                &falling,
                &models,
                IVec3::ZERO,
                0.5,
            )
            .unwrap();
        assert_eq!(renderer.draws.len(), 2);
        assert_eq!(renderer.draws[0].instances, 0..2);
        assert_eq!(renderer.draws[1].instances, 2..3);
        let first_ids = renderer.resource_identities();
        renderer
            .update_with_falling(
                &device,
                &queue,
                &[],
                &[],
                &falling,
                &models,
                IVec3::ZERO,
                0.5,
            )
            .unwrap();
        assert_eq!(renderer.resource_identities(), first_ids);

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
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 64 * 64 * 4,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let view = output.create_view(&wgpu::TextureViewDescriptor::default());
            let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
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
            renderer.draw(&mut pass);
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
        let (send, receive) = mpsc::channel();
        readback
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
        receive
            .recv_timeout(Duration::from_secs(15))
            .unwrap()
            .unwrap();
        let data = readback.slice(..).get_mapped_range().unwrap();
        let pixel = |x: usize| &data[(32 * 64 + x) * 4..(32 * 64 + x) * 4 + 4];
        assert!(
            pixel(8)[1] > 100 && pixel(8)[0] == 0,
            "overlay: {:?}",
            pixel(8)
        );
        assert!(
            pixel(24)[0] > 100 && pixel(24)[1] == 0,
            "first block: {:?}",
            pixel(24)
        );
        assert!(
            pixel(48)[0] > 100 && pixel(48)[1] == 0,
            "second block: {:?}",
            pixel(48)
        );
        drop(data);
        readback.unmap();
        assert!(errors.pop().await.is_none());
    });
}
