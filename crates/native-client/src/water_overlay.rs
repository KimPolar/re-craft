//! First-person underwater screen texture from the active resource stack.
use anyhow::{Context, Result, ensure};
use image::{ImageFormat, RgbaImage};
use native_resources::packs::ResourceProvider;
use std::io::Cursor;

pub struct Texture(pub RgbaImage);
impl Texture {
    pub fn load(source: &mut impl ResourceProvider) -> Result<Self> {
        let path = "assets/minecraft/textures/misc/underwater.png";
        let bytes = source
            .read_asset(path)?
            .with_context(|| format!("missing {path}"))?;
        let mut reader = image::ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(4096);
        limits.max_image_height = Some(4096);
        limits.max_alloc = Some(128 * 1024 * 1024);
        reader.limits(limits);
        let image = reader
            .decode()
            .with_context(|| format!("invalid {path}"))?
            .to_rgba8();
        ensure!(
            image.width() > 0 && image.height() > 0,
            "empty underwater texture"
        );
        Ok(Self(image))
    }
}

#[derive(Clone, Copy)]
pub struct Frame {
    pub yaw_degrees: f32,
    pub pitch_degrees: f32,
    pub brightness: f32,
}

pub fn brightness(light: u8, ambient: f32) -> f32 {
    let fraction = f32::from(light.min(15)) / 15.0;
    let curve = fraction / (4.0 - 3.0 * fraction);
    curve + ambient * (1.0 - curve)
}

pub fn raw_brightness(block: u8, sky: u8, sky_darken: u8) -> u8 {
    block.max(sky.saturating_sub(sky_darken))
}

fn uniform_state(frame: Frame) -> [f32; 4] {
    // ARGB.colorFromFloat converts each component to an 8-bit integer.
    let brightness = (frame.brightness.clamp(0.0, 1.0) * 255.0) as u8 as f32 / 255.0;
    [
        -frame.yaw_degrees / 64.0,
        frame.pitch_degrees / 64.0,
        brightness,
        25.0 / 255.0,
    ]
}

pub struct Overlay {
    pipeline: wgpu::RenderPipeline,
    binding: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    last_state: Option<[f32; 4]>,
    frame: Option<Frame>,
}
impl Overlay {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        source: &Texture,
    ) -> Result<Self> {
        let image = &source.0;
        ensure!(
            image.width() <= device.limits().max_texture_dimension_2d
                && image.height() <= device.limits().max_texture_dimension_2d,
            "underwater texture exceeds GPU limits"
        );
        let size = wgpu::Extent3d {
            width: image.width(),
            height: image.height(),
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("underwater texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("underwater repeat sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("underwater overlay state"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("underwater overlay bindings"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("underwater overlay bindings"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &texture.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("underwater pipeline"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("underwater overlay shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("water_overlay.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("underwater overlay"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some(if format.is_srgb() {
                    "fragment_srgb"
                } else {
                    "fragment_unorm"
                }),
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
            binding,
            uniform,
            last_state: None,
            frame: None,
        })
    }
    pub fn set_frame(&mut self, frame: Option<Frame>) {
        self.frame = frame;
    }
    pub fn draw(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
    ) {
        let Some(frame) = self.frame else { return };
        let state = uniform_state(frame);
        if self.last_state != Some(state) {
            queue.write_buffer(&self.uniform, 0, bytemuck::cast_slice(&state));
            self.last_state = Some(state);
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("underwater overlay pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.binding, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};

    struct Assets(Vec<u8>);
    impl ResourceProvider for Assets {
        fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok((path == "assets/minecraft/textures/misc/underwater.png").then(|| self.0.clone()))
        }
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn vertex_uv_and_color_match_java_water_overlay() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("WaterOverlayOracle.java");
        let result = temp.path().join("vertices.txt");
        std::fs::write(
            &source,
            include_str!("../tests/java/WaterOverlayOracle.java"),
        )
        .unwrap();
        let compile = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("WaterOverlayOracle")
            .arg(&result)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = std::fs::read_to_string(result).unwrap();
        let mut lines = text.lines();
        for yaw in [-180.0, -25.0, 0.0, 75.0, 360.0] {
            for pitch in [-90.0, 0.0, 40.0, 90.0] {
                for brightness in [0.0, 0.1, 0.5, 1.0] {
                    let state = uniform_state(Frame {
                        yaw_degrees: yaw,
                        pitch_degrees: pitch,
                        brightness,
                    });
                    let channel = (brightness * 255.0) as u32;
                    let packed = (25_u32 << 24) | (channel << 16) | (channel << 8) | channel;
                    for [x, y] in [[-1.0_f32, -1.0_f32], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]] {
                        let values: Vec<_> = lines.next().unwrap().split_whitespace().collect();
                        assert_eq!(values.len(), 5);
                        assert_eq!(values[0].parse::<f32>().unwrap(), x);
                        assert_eq!(values[1].parse::<f32>().unwrap(), y);
                        assert_eq!(
                            values[2].parse::<f32>().unwrap(),
                            state[0] + (1.0 - x) * 2.0
                        );
                        assert_eq!(
                            values[3].parse::<f32>().unwrap(),
                            state[1] + (1.0 - y) * 2.0
                        );
                        assert_eq!(values[4].parse::<i32>().unwrap() as u32, packed);
                    }
                }
            }
        }
        for ambient in [0.0_f32, 0.1, 1.0] {
            for level in 0..16 {
                let expected: f32 = lines.next().unwrap().parse().unwrap();
                assert_eq!(
                    brightness(level, ambient).to_bits(),
                    expected.to_bits(),
                    "level {level}, ambient {ambient}"
                );
            }
        }
        assert!(lines.next().is_none());
    }
    #[test]
    fn loads_overlay_from_resource_provider() {
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            2,
            1,
            image::Rgba([23, 45, 67, 255]),
        ))
        .write_to(&mut bytes, ImageFormat::Png)
        .unwrap();
        assert_eq!(
            Texture::load(&mut Assets(bytes.into_inner()))
                .unwrap()
                .0
                .get_pixel(1, 0)
                .0,
            [23, 45, 67, 255]
        );
    }

    #[test]
    fn raw_brightness_uses_darker_sky_and_keeps_block_light() {
        assert_eq!(raw_brightness(1, 15, 11), 4);
        assert_eq!(raw_brightness(10, 15, 11), 10);
        assert_eq!(raw_brightness(0, 3, 11), 0);
    }
    #[test]
    #[ignore = "requires a GPU; renders offscreen without opening a window"]
    fn gpu_overlay_repeats_scrolls_and_blends_over_world() {
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
            let mut pixels = RgbaImage::new(2, 2);
            pixels.put_pixel(0, 0, image::Rgba([20, 40, 60, 255]));
            pixels.put_pixel(1, 0, image::Rgba([200, 0, 0, 255]));
            pixels.put_pixel(0, 1, image::Rgba([0, 200, 0, 255]));
            pixels.put_pixel(1, 1, image::Rgba([0, 0, 200, 255]));
            let mut overlay = Overlay::new(
                &device,
                &queue,
                wgpu::TextureFormat::Rgba8Unorm,
                &Texture(pixels),
            )
            .unwrap();
            let size = wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            };
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 64 * 64 * 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            for (frame, source) in [
                (None, None),
                (
                    Some(Frame {
                        yaw_degrees: 0.0,
                        pitch_degrees: 0.0,
                        brightness: 1.0,
                    }),
                    Some([200.0, 0.0, 0.0]),
                ),
                (
                    Some(Frame {
                        yaw_degrees: 32.0,
                        pitch_degrees: 0.0,
                        brightness: 1.0,
                    }),
                    Some([20.0, 40.0, 60.0]),
                ),
                (
                    Some(Frame {
                        yaw_degrees: 0.0,
                        pitch_degrees: 32.0,
                        brightness: 1.0,
                    }),
                    Some([0.0, 0.0, 200.0]),
                ),
            ] {
                overlay.set_frame(frame);
                let mut encoder = device.create_command_encoder(&Default::default());
                {
                    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: None,
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color {
                                    r: 0.2,
                                    g: 0.4,
                                    b: 0.6,
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
                }
                overlay.draw(&queue, &mut encoder, &view);
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture,
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
                let data = readback.slice(..).get_mapped_range().unwrap();
                let sample = &data[(2 * 64 + 2) * 4..][..4];
                let background = [51.0_f32, 102.0, 153.0];
                for channel in 0..3 {
                    let expected = source
                        .map_or(background[channel], |src| {
                            background[channel] * (230.0 / 255.0) + src[channel] * (25.0 / 255.0)
                        })
                        .round() as i32;
                    assert!(
                        (i32::from(sample[channel]) - expected).abs() <= 1,
                        "pixel {sample:?}, expected {expected}, source {source:?}"
                    );
                }
                assert_eq!(sample[3], 255);
                drop(data);
                readback.unmap();
            }
        });
    }
}
