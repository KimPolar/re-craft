//! Full-resolution blur before drawing an in-game screen.
use crate::menu_renderer::{
    blur_binding, color_attachment, create_blur_layout, image_binding, pipeline, pipeline_entry,
    texture,
};
use anyhow::Result;
use wgpu::util::DeviceExt;

pub struct ScreenBlur {
    layout: wgpu::BindGroupLayout,
    image_layout: wgpu::BindGroupLayout,
    radius_buffer: wgpu::Buffer,
    radius: u8,
    sampler: wgpu::Sampler,
    horizontal: wgpu::RenderPipeline,
    vertical: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    targets: Option<Targets>,
    format: wgpu::TextureFormat,
}

struct Targets {
    size: (u32, u32),
    _source: wgpu::Texture,
    _temporary: wgpu::Texture,
    source_view: wgpu::TextureView,
    temporary_view: wgpu::TextureView,
    source_bind: wgpu::BindGroup,
    temporary_bind: wgpu::BindGroup,
    composite_bind: wgpu::BindGroup,
}

impl ScreenBlur {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = create_blur_layout(device);
        let image_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("world blur composite layout"),
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
        let radius_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("world blur radius"),
            contents: bytemuck::cast_slice(&[0_u32; 4]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("world blur sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("world blur shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("menu_blur.wgsl").into()),
        });
        let image_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("world blur composite"),
            source: wgpu::ShaderSource::Wgsl(include_str!("overlay.wgsl").into()),
        });
        let horizontal = pipeline_entry(
            device,
            &layout,
            &shader,
            format,
            None,
            "horizontal",
            "world blur horizontal",
        );
        let vertical = pipeline_entry(
            device,
            &layout,
            &shader,
            format,
            None,
            "vertical",
            "world blur vertical",
        );
        let composite = pipeline(
            device,
            &image_layout,
            &image_shader,
            format,
            None,
            "world blur composite",
        );
        Self {
            layout,
            image_layout,
            radius_buffer,
            radius: 0,
            sampler,
            horizontal,
            vertical,
            composite,
            targets: None,
            format,
        }
    }

    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: (u32, u32),
        radius: u8,
    ) -> Result<()> {
        let radius = radius.min(10);
        if self.radius != radius {
            queue.write_buffer(
                &self.radius_buffer,
                0,
                bytemuck::cast_slice(&[u32::from(radius), 0, 0, 0]),
            );
            self.radius = radius;
        }
        if self
            .targets
            .as_ref()
            .is_some_and(|targets| targets.size == size)
        {
            return Ok(());
        }
        let usage = wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST;
        let source = texture(device, size, self.format, usage, "world blur source")?;
        let temporary = texture(device, size, self.format, usage, "world blur temporary")?;
        let source_view = source.create_view(&Default::default());
        let temporary_view = temporary.create_view(&Default::default());
        let source_bind = blur_binding(
            device,
            &self.layout,
            &source_view,
            &self.sampler,
            &self.radius_buffer,
        );
        let temporary_bind = blur_binding(
            device,
            &self.layout,
            &temporary_view,
            &self.sampler,
            &self.radius_buffer,
        );
        let composite_bind = image_binding(device, &self.image_layout, &source_view, &self.sampler);
        self.targets = Some(Targets {
            size,
            _source: source,
            _temporary: temporary,
            source_view,
            temporary_view,
            source_bind,
            temporary_bind,
            composite_bind,
        });
        Ok(())
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.targets.as_ref().expect("prepared blur").source_view
    }

    pub fn render(&self, encoder: &mut wgpu::CommandEncoder, output: &wgpu::TextureView) {
        let targets = self.targets.as_ref().expect("prepared blur");
        for _ in 0..3 {
            for (view, pipeline, binding) in [
                (
                    &targets.temporary_view,
                    &self.horizontal,
                    &targets.source_bind,
                ),
                (
                    &targets.source_view,
                    &self.vertical,
                    &targets.temporary_bind,
                ),
            ] {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("world blur pass"),
                    color_attachments: &[Some(color_attachment(
                        view,
                        wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    ))],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, binding, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("world blur composite"),
            color_attachments: &[Some(color_attachment(
                output,
                wgpu::LoadOp::Clear(wgpu::Color::BLACK),
            ))],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.composite);
        pass.set_bind_group(0, &targets.composite_bind, &[]);
        pass.draw(0..6, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu_draw::Canvas;
    use std::{sync::mpsc, time::Duration};

    #[test]
    #[ignore = "requires a GPU adapter"]
    fn gpu_game_blur_tracks_radius_and_preserves_output() {
        pollster::block_on(async {
            let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
            descriptor.backends = wgpu::Backends::VULKAN;
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
            let mut blur = ScreenBlur::new(&device, wgpu::TextureFormat::Rgba8Unorm);
            blur.prepare(&device, &queue, (32, 16), 5).unwrap();
            assert_eq!(blur.targets.as_ref().unwrap().size, (32, 16));
            let pixels: Vec<u8> = (0..64)
                .flat_map(|y| {
                    (0..64).flat_map(move |x| {
                        [
                            if x < 32 { 255 } else { 0 },
                            if y < 32 { 220 } else { 0 },
                            (x * 3) as u8,
                            255,
                        ]
                    })
                })
                .collect();
            let output = texture(
                &device,
                (64, 64),
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                "blur test output",
            )
            .unwrap();
            let output_view = output.create_view(&Default::default());
            for radius in [1_u8, 5, 10] {
                blur.prepare(&device, &queue, (64, 64), radius).unwrap();
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &blur.targets.as_ref().unwrap()._source,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &pixels,
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
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("blur test readback"),
                    size: 64 * 64 * 4,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                let mut encoder = device.create_command_encoder(&Default::default());
                blur.render(&mut encoder, &output_view);
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
                    wgpu::Extent3d {
                        width: 64,
                        height: 64,
                        depth_or_array_layers: 1,
                    },
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
                let mut expected = Canvas::new(64, 64);
                for (index, pixel) in pixels.as_chunks::<4>().0.iter().enumerate() {
                    expected.pixel(
                        (index % 64) as i32,
                        (index / 64) as i32,
                        [pixel[0], pixel[1], pixel[2], pixel[3]],
                    );
                }
                expected.blur_radius(radius);
                for (index, &pixel) in expected.pixels.iter().enumerate() {
                    let rgb = [(pixel >> 16) as u8, (pixel >> 8) as u8, pixel as u8];
                    for (actual, expected) in data[index * 4..index * 4 + 3].iter().zip(rgb) {
                        assert!(
                            actual.abs_diff(expected) <= 6,
                            "radius {radius}, pixel {index}: {actual} != {expected}"
                        );
                    }
                }
                drop(data);
                buffer.unmap();
            }
            if let Some(error) = errors.pop().await {
                panic!("GPU validation failed: {error}");
            }
        });
    }
}
