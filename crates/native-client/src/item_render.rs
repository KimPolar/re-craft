//! Render temporary dropped block items using the active resource pack textures.

use std::time::Instant;

use anyhow::{Result, ensure};
use glam::IVec3;
use image::{GenericImage, Rgba, RgbaImage, imageops::FilterType};
use native_session::items::DroppedItem;

use crate::mesh;

fn atlas(images: &[&RgbaImage], limit: u32) -> Result<RgbaImage> {
    ensure!(
        images.len() == mesh::DROPPED_TEXTURE_COUNT as usize,
        "missing dropped-item textures"
    );
    ensure!(
        images
            .iter()
            .all(|image| image.width() > 0 && image.height() >= image.width()),
        "invalid item texture"
    );
    let tile = images
        .iter()
        .map(|image| image.width())
        .max()
        .unwrap()
        .min(limit / mesh::DROPPED_ATLAS_COLUMNS.max(mesh::DROPPED_ATLAS_ROWS));
    ensure!(tile > 0, "GPU cannot support an item texture atlas");
    let mut atlas = RgbaImage::new(
        tile * mesh::DROPPED_ATLAS_COLUMNS,
        tile * mesh::DROPPED_ATLAS_ROWS,
    );
    for (index, image) in images.iter().enumerate() {
        // Animated block textures use their first square frame for now.
        let first =
            image::imageops::crop_imm(*image, 0, 0, image.width(), image.width()).to_image();
        let first = image::imageops::resize(&first, tile, tile, FilterType::Nearest);
        let index = index as u32;
        atlas.copy_from(
            &first,
            (index % mesh::DROPPED_ATLAS_COLUMNS) * tile,
            (index / mesh::DROPPED_ATLAS_COLUMNS) * tile,
        )?;
    }
    for y in 0..tile {
        for x in 0..tile {
            let color = if ((x * 2 / tile) + (y * 2 / tile)) % 2 == 0 {
                [255, 0, 255, 255]
            } else {
                [0, 0, 0, 255]
            };
            let index = mesh::DROPPED_TEXTURE_COUNT;
            atlas.put_pixel(
                (index % mesh::DROPPED_ATLAS_COLUMNS) * tile + x,
                (index / mesh::DROPPED_ATLAS_COLUMNS) * tile + y,
                Rgba(color),
            );
        }
    }
    Ok(atlas)
}

pub struct ItemRenderer {
    bind_group: wgpu::BindGroup,
    vertices: Option<wgpu::Buffer>,
    capacity: usize,
    count: u32,
    started: Instant,
}

impl ItemRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        camera: &wgpu::Buffer,
        sampler: &wgpu::Sampler,
        images: &[&RgbaImage],
    ) -> Result<Self> {
        let atlas = atlas(images, device.limits().max_texture_dimension_2d)?;
        let size = wgpu::Extent3d {
            width: atlas.width(),
            height: atlas.height(),
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("dropped item atlas"),
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
            label: Some("dropped item textures"),
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
        Ok(Self {
            bind_group,
            vertices: None,
            capacity: 0,
            count: 0,
            started: Instant::now(),
        })
    }

    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        items: &[DroppedItem],
        origin: IVec3,
    ) -> Result<()> {
        self.update_visible(device, queue, items, origin, None)
    }

    pub fn update_visible(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        items: &[DroppedItem],
        origin: IVec3,
        models: Option<&[Option<std::sync::Arc<crate::held_items::HeldModel>>]>,
    ) -> Result<()> {
        if let Some(models) = models {
            ensure!(
                models.len() == items.len(),
                "dropped item model count mismatch"
            );
        }
        let visible = items.iter().enumerate().filter_map(|(index, item)| {
            models
                .is_none_or(|models| models[index].is_none())
                .then_some(item)
        });
        let vertices = mesh::dropped_items(visible, origin, self.started.elapsed().as_secs_f32());
        self.count = u32::try_from(vertices.len())?;
        let bytes = bytemuck::cast_slice(&vertices);
        if bytes.is_empty() {
            return Ok(());
        }
        if self.capacity < bytes.len() {
            self.capacity = bytes.len().next_power_of_two();
            ensure!(
                self.capacity as u64 <= device.limits().max_buffer_size,
                "too many dropped items for the GPU"
            );
            self.vertices = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("dropped item vertices"),
                size: self.capacity as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        queue.write_buffer(
            self.vertices.as_ref().expect("item buffer allocated"),
            0,
            bytes,
        );
        Ok(())
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        if let Some(buffer) = &self.vertices
            && self.count != 0
        {
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..self.count, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atlas_uses_first_animation_frame_and_marks_unknown_items() {
        let images: Vec<_> = (0..mesh::DROPPED_TEXTURE_COUNT)
            .map(|index| RgbaImage::from_pixel(2, 4, Rgba([index as u8, 0, 0, 255])))
            .collect();
        let refs = images.iter().collect::<Vec<_>>();
        let result = atlas(&refs, 1024).unwrap();
        for index in 0..mesh::DROPPED_TEXTURE_COUNT {
            assert_eq!(
                result
                    .get_pixel(
                        (index % mesh::DROPPED_ATLAS_COLUMNS) * 2,
                        (index / mesh::DROPPED_ATLAS_COLUMNS) * 2
                    )
                    .0[0],
                index as u8
            );
        }
        let index = mesh::DROPPED_TEXTURE_COUNT;
        let x = (index % mesh::DROPPED_ATLAS_COLUMNS) * 2;
        let y = (index / mesh::DROPPED_ATLAS_COLUMNS) * 2;
        assert_eq!(result.get_pixel(x, y).0, [255, 0, 255, 255]);
        assert_eq!(result.get_pixel(x + 1, y).0, [0, 0, 0, 255]);
    }

    #[test]
    #[ignore = "requires a GPU; does not open a window"]
    fn gpu_renders_dropped_items_and_experience_orb_textures() {
        use crate::{mesh::Vertex, renderer::VERTEX_ATTRIBUTES};
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
                .unwrap();
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
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: None,
                source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
            });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: None,
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
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
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("world_fragment"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            });
            let mut uniform = [0_f32; 20];
            uniform[0] = 1.0;
            uniform[5] = 1.0;
            uniform[10] = 1.0;
            uniform[15] = 1.0;
            uniform[16] = 64.0;
            uniform[17] = 64.0;
            let camera = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&uniform),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Nearest,
                min_filter: wgpu::FilterMode::Nearest,
                ..Default::default()
            });
            let images: Vec<_> = (0..mesh::DROPPED_TEXTURE_COUNT)
                .map(|i| {
                    RgbaImage::from_pixel(
                        2,
                        2,
                        Rgba(if i == 5 || i == mesh::DROPPED_BASE_TEXTURES {
                            [255, 0, 0, 255]
                        } else {
                            [0, 255, 0, 255]
                        }),
                    )
                })
                .collect();
            let mut renderer = ItemRenderer::new(
                &device,
                &queue,
                &layout,
                &camera,
                &sampler,
                &images.iter().collect::<Vec<_>>(),
            )
            .unwrap();
            let item = DroppedItem {
                position: glam::DVec3::new(-0.4, -0.2, 0.5),
                previous_position: glam::DVec3::new(-0.4, -0.2, 0.5),
                age: 0,
                bob_offset: 0.0,
                stack: native_session::inventory::ItemStack::new(
                    "minecraft:cobblestone".into(),
                    1,
                    64,
                    native_persistence::nbt::Tag::Compound(vec![]),
                )
                .unwrap(),
            };
            let wood = DroppedItem {
                position: glam::DVec3::new(0.4, -0.2, 0.5),
                previous_position: glam::DVec3::new(0.4, -0.2, 0.5),
                age: 0,
                bob_offset: 0.0,
                stack: native_session::inventory::ItemStack::new(
                    "minecraft:oak_log".into(),
                    1,
                    64,
                    native_persistence::nbt::Tag::Compound(vec![]),
                )
                .unwrap(),
            };
            renderer
                .update(&device, &queue, &[item, wood], IVec3::ZERO)
                .unwrap();
            let orb_texture = RgbaImage::from_pixel(64, 64, Rgba([0, 255, 0, 255]));
            let mut orbs = crate::orb_render::OrbRenderer::new(
                &device,
                &queue,
                &layout,
                &pipeline_layout,
                &camera,
                &sampler,
                &shader,
                wgpu::TextureFormat::Rgba8Unorm,
                &orb_texture,
            )
            .unwrap();
            let orb = native_session::orbs::ExperienceOrb {
                position: glam::DVec3::new(0.0, -0.15, 0.4),
                previous_position: glam::DVec3::new(0.0, -0.15, 0.4),
                value: 7,
                age: 0,
            };
            orbs.update(&device, &queue, &[orb], IVec3::ZERO, glam::Vec3::NEG_Z, 0.0)
                .unwrap();
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
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &glint_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &[255, 0, 0, 255],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4),
                    rows_per_image: Some(1),
                },
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
            let glint_view = glint_texture.create_view(&wgpu::TextureViewDescriptor::default());
            let glint_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&[0.0_f32, 0.0, 0.75, 1.0]),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let mut ground = crate::ground_items::GroundItemRenderer::new(
                &device,
                wgpu::TextureFormat::Rgba8Unorm,
                &layout,
                &camera,
                (&glint_view, &sampler, &glint_uniform),
            );
            let display = crate::held_items::DisplayTransform {
                translation: glam::Vec3::ZERO,
                rotation: glam::Vec3::ZERO,
                scale: glam::Vec3::splat(0.4),
            };
            let item_image =
                std::sync::Arc::new(RgbaImage::from_pixel(16, 16, Rgba([0, 0, 255, 255])));
            let model = |glint| {
                std::sync::Arc::new(crate::held_items::HeldModel {
                    numeric_item_id: None,
                    quads: crate::generated_item::bake(
                        std::sync::Arc::clone(&item_image),
                        [1.0; 3],
                        0,
                    )
                    .unwrap(),
                    glint,
                    glint_materials: if glint {
                        vec![(
                            std::sync::Arc::clone(&item_image),
                            Some([0.1, 0.2, 0.3, 0.4]),
                        )]
                    } else {
                        Vec::new()
                    },
                    right: display,
                    left: display,
                    first_right: display,
                    first_left: display,
                    ground: display,
                    gui: display,
                    gui_side_light: false,
                    gui_icon: None,
                    gui_glint_uv: None,
                    gui_glint_pixels: None,
                    gui_glint_quads: None,
                    sprite_layers: Vec::new(),
                })
            };
            let ground_item = DroppedItem {
                position: glam::DVec3::new(0.0, 0.25, 0.25),
                previous_position: glam::DVec3::new(0.0, 0.25, 0.25),
                age: 0,
                bob_offset: 0.0,
                stack: native_session::inventory::ItemStack::new(
                    "minecraft:diamond".into(),
                    33,
                    64,
                    native_persistence::nbt::Tag::Compound(vec![]),
                )
                .unwrap(),
            };
            let glowing_item = DroppedItem {
                position: glam::DVec3::new(0.6, 0.25, 0.25),
                previous_position: glam::DVec3::new(0.6, 0.25, 0.25),
                ..ground_item.clone()
            };
            let many_items = vec![ground_item.clone(); 70];
            let many_models = (0..70).map(|_| Some(model(false))).collect::<Vec<_>>();
            ground
                .update(&device, &queue, &many_items, &many_models, IVec3::ZERO, 0.0)
                .unwrap();
            let previous_resources = ground.resource_identities();
            assert_eq!(previous_resources.len(), 70);
            ground
                .update(&device, &queue, &many_items, &many_models, IVec3::ZERO, 0.5)
                .unwrap();
            assert_eq!(
                ground.resource_identities(),
                previous_resources,
                "visible models must survive the bounded idle cache without GPU re-uploads"
            );
            ground
                .update(
                    &device,
                    &queue,
                    &[ground_item, glowing_item],
                    &[Some(model(false)), Some(model(true))],
                    IVec3::ZERO,
                    0.0,
                )
                .unwrap();
            assert!(ground.has_glint());
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
                pass.set_pipeline(&pipeline);
                renderer.draw(&mut pass);
                orbs.draw(&mut pass);
                ground.draw(&mut pass);
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
            let pixels = data.as_chunks::<4>().0;
            let red_pixels = |right: bool| {
                pixels
                    .iter()
                    .enumerate()
                    .filter(|(index, pixel)| {
                        (index % 64 >= 32) == right && pixel[0] > 100 && pixel[1] == 0
                    })
                    .count()
            };
            assert!(
                red_pixels(false) > 20 && red_pixels(true) > 20,
                "cobblestone or wood did not render using its atlas tile"
            );
            assert!(
                pixels
                    .iter()
                    .filter(|pixel| pixel[1] > 80 && pixel[0] == 0)
                    .count()
                    > 20,
                "experience orb did not render with alpha blending"
            );
            assert!(
                pixels
                    .iter()
                    .filter(|pixel| pixel[2] > 100 && pixel[0] == 0 && pixel[1] == 0)
                    .count()
                    > 20,
                "instanced ground item model did not render"
            );
            assert!(
                pixels
                    .iter()
                    .filter(|pixel| pixel[0] > 80 && pixel[1] == 0 && pixel[2] > 80)
                    .count()
                    > 20,
                "enchanted ground model did not receive glint"
            );
            drop(data);
            buffer.unmap();
            if let Some(error) = errors.pop().await {
                panic!("GPU validation failed: {error}");
            }
        });
    }
}
