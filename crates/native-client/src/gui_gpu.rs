//! Integer framebuffer-to-GUI mapping shared by menu and game overlays.
use wgpu::util::DeviceExt;

pub(crate) struct GuiScaleBinding {
    pub layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
    buffer: wgpu::Buffer,
    scale: u32,
    origin: (i32, i32),
}

impl GuiScaleBinding {
    pub fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("GUI scale layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        Self::with_layout(device, &layout)
    }

    pub fn with_layout(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> Self {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("GUI integer scale"),
            contents: bytemuck::cast_slice(&[1.0_f32, 0.0, 0.0, 0.0]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("GUI scale binding"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        Self {
            layout: layout.clone(),
            bind_group,
            buffer,
            scale: 1,
            origin: (0, 0),
        }
    }

    pub fn set(&mut self, queue: &wgpu::Queue, scale: u32) {
        self.set_mapping(queue, scale, (0, 0));
    }

    pub fn set_mapping(&mut self, queue: &wgpu::Queue, scale: u32, origin: (i32, i32)) {
        let scale = scale.max(1);
        if self.scale != scale || self.origin != origin {
            queue.write_buffer(
                &self.buffer,
                0,
                bytemuck::cast_slice(&[scale as f32, origin.0 as f32, origin.1 as f32, 0.0]),
            );
            self.scale = scale;
            self.origin = origin;
        }
    }

    pub fn scale(&self) -> u32 {
        self.scale
    }
}
