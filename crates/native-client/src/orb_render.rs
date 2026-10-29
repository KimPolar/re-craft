//! Camera-facing 26.3 experience-orb sprites from the active resource stack.

use anyhow::{Result, ensure};
use glam::{IVec3, Vec3};
use image::RgbaImage;
use native_session::{orbs::ExperienceOrb, physics::java_sin};

use crate::{mesh::Vertex, renderer::VERTEX_ATTRIBUTES};

fn vertices(orbs: &[ExperienceOrb], origin: IVec3, direction: Vec3, partial: f32) -> Vec<Vertex> {
    let direction = direction.normalize_or_zero();
    let right = direction.cross(Vec3::Y).normalize_or_zero();
    let up = right.cross(direction).normalize_or_zero();
    let mut vertices = Vec::with_capacity(orbs.len().saturating_mul(6));
    for orb in orbs {
        let position = (orb.previous_position.lerp(orb.position, f64::from(partial))
            - origin.as_dvec3())
        .as_vec3();
        let center = position + Vec3::Y * 0.1;
        let icon = orb.icon() as u32;
        let left = (icon % 4) as f32 * 0.25;
        let top = (icon / 4) as f32 * 0.25;
        let phase = (orb.age as f32 + partial) / 2.0;
        let red = ((java_sin(f64::from(phase), false) + 1.0) * 0.5 * 255.0) as u8;
        let blue = ((java_sin(f64::from(phase + 4.188_790_3), false) + 1.0) * 0.1 * 255.0) as u8;
        let tint = [f32::from(red) / 255.0, 1.0, f32::from(blue) / 255.0];
        let corners = [
            (-0.5, -0.25, left, top + 0.25),
            (0.5, -0.25, left + 0.25, top + 0.25),
            (0.5, 0.75, left + 0.25, top),
            (-0.5, 0.75, left, top),
        ];
        for index in [0, 1, 2, 0, 2, 3] {
            let (x, y, u, v) = corners[index];
            vertices.push(Vertex {
                position: (center + (right * x + up * y) * 0.3).to_array(),
                uv: [u, v],
                shade: 1.0,
                tint,
                light: u32::MAX,
            });
        }
    }
    vertices
}

pub struct OrbRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    vertices: Option<wgpu::Buffer>,
    capacity: usize,
    count: u32,
}

impl OrbRenderer {
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
        image: &RgbaImage,
    ) -> Result<Self> {
        ensure!(
            image.width() > 0
                && image.height() > 0
                && image.width() <= device.limits().max_texture_dimension_2d
                && image.height() <= device.limits().max_texture_dimension_2d,
            "invalid experience orb texture"
        );
        let size = wgpu::Extent3d {
            width: image.width(),
            height: image.height(),
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("experience orb texture"),
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
            label: Some("experience orb texture"),
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
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("experience orb pipeline"),
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
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("orb_fragment"),
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
        Ok(Self {
            pipeline,
            bind_group,
            vertices: None,
            capacity: 0,
            count: 0,
        })
    }

    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        orbs: &[ExperienceOrb],
        origin: IVec3,
        direction: Vec3,
        partial: f32,
    ) -> Result<()> {
        let vertices = vertices(orbs, origin, direction, partial);
        self.count = u32::try_from(vertices.len())?;
        let bytes = bytemuck::cast_slice(&vertices);
        if bytes.is_empty() {
            return Ok(());
        }
        if self.capacity < bytes.len() {
            self.capacity = bytes
                .len()
                .checked_next_power_of_two()
                .unwrap_or(bytes.len());
            ensure!(
                self.capacity as u64 <= device.limits().max_buffer_size,
                "too many experience orb vertices"
            );
            self.vertices = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("experience orb vertices"),
                size: self.capacity as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        queue.write_buffer(
            self.vertices.as_ref().expect("orb buffer allocated"),
            0,
            bytes,
        );
        Ok(())
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        if let Some(buffer) = &self.vertices
            && self.count != 0
        {
            pass.set_pipeline(&self.pipeline);
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
    fn billboard_interpolates_and_uses_vanilla_icon_tile() {
        let orb = ExperienceOrb {
            previous_position: glam::DVec3::new(2.0, 4.0, 0.0),
            position: glam::DVec3::new(4.0, 4.0, 0.0),
            value: 73,
            age: 0,
        };
        let vertices = vertices(&[orb], IVec3::ZERO, Vec3::NEG_Z, 0.5);
        assert_eq!(vertices.len(), 6);
        assert_eq!(vertices[0].uv, [0.25, 0.5]);
        assert!((vertices[0].position[0] - 2.85).abs() < 1e-6);
        assert!((vertices[0].position[1] - 4.025).abs() < 1e-6);
        assert_eq!(vertices[0].tint[1], 1.0);
    }
}
