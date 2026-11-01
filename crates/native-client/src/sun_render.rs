//! 26.3 sun sprite and its time-dependent celestial transform.

use anyhow::{Context, Result, ensure};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use native_resources::{
    packs::{ResourceId, ResourceProvider},
    sprites::SpriteAtlas,
};
use wgpu::util::DeviceExt;

#[derive(Clone)]
pub struct Texture {
    atlas: SpriteAtlas,
    id: ResourceId,
}

impl Texture {
    pub fn load(source: &mut impl ResourceProvider) -> Result<Self> {
        let id = ResourceId::parse("minecraft:environment/celestial/sun")?;
        let atlas = SpriteAtlas::build(source, [id.clone()], 4096)?;
        Ok(Self { atlas, id })
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    uv: [f32; 2],
}

fn quad(texture: &Texture) -> Result<[Vertex; 6]> {
    let rect = texture
        .atlas
        .rectangle(&texture.id)
        .context("sun sprite missing from atlas")?;
    let u0 = rect.x as f32 / texture.atlas.texture.width as f32;
    let v0 = rect.y as f32 / texture.atlas.texture.height as f32;
    let u1 = (rect.x + rect.width) as f32 / texture.atlas.texture.width as f32;
    let v1 = (rect.y + rect.height) as f32 / texture.atlas.texture.height as f32;
    let vertices = [
        Vertex {
            position: [-1.0, 0.0, -1.0],
            uv: [u0, v0],
        },
        Vertex {
            position: [1.0, 0.0, -1.0],
            uv: [u1, v0],
        },
        Vertex {
            position: [1.0, 0.0, 1.0],
            uv: [u1, v1],
        },
        Vertex {
            position: [-1.0, 0.0, 1.0],
            uv: [u0, v1],
        },
    ];
    Ok([
        vertices[0],
        vertices[1],
        vertices[2],
        vertices[0],
        vertices[2],
        vertices[3],
    ])
}

fn sun_model(angle_degrees: f32) -> Mat4 {
    Mat4::from_rotation_y(-std::f32::consts::FRAC_PI_2)
        * Mat4::from_rotation_x(angle_degrees * 0.017_453_292_f32)
        * Mat4::from_translation(Vec3::new(0.0, 100.0, 0.0))
        * Mat4::from_scale(Vec3::new(30.0, 1.0, 30.0))
}

#[derive(Clone, Copy)]
pub struct Frame {
    pub angle_degrees: f32,
    pub rain_brightness: f32,
    pub tick: u64,
}

pub struct Sun {
    pipeline: wgpu::RenderPipeline,
    bindings: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    vertices: wgpu::Buffer,
    image: wgpu::Texture,
    source: Texture,
    frame: Option<Frame>,
    last_state: Option<[[f32; 4]; 6]>,
}

impl Sun {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        source: Texture,
    ) -> Result<Self> {
        let atlas = &source.atlas.texture;
        ensure!(
            atlas.width > 0
                && atlas.height > 0
                && atlas.width <= device.limits().max_texture_dimension_2d
                && atlas.height <= device.limits().max_texture_dimension_2d,
            "sun atlas exceeds GPU limits"
        );
        let extent = wgpu::Extent3d {
            width: atlas.width,
            height: atlas.height,
            depth_or_array_layers: 1,
        };
        let image = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Sun sprite atlas"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &image,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &atlas.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(atlas.width * 4),
                rows_per_image: Some(atlas.height),
            },
            extent,
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Sun atlas sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Sun camera and color"),
            size: 96,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Sun bindings"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
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
        let view = image.create_view(&Default::default());
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Sun bindings"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
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
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Sun pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Sun shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("sun_render.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Sun pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2],
                })],
            },
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some(if format.is_srgb() {
                    "fragment_srgb_target"
                } else {
                    "fragment_unorm_target"
                }),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Sun quad"),
            contents: bytemuck::cast_slice(&quad(&source)?),
            usage: wgpu::BufferUsages::VERTEX,
        });
        Ok(Self {
            pipeline,
            bindings,
            uniform,
            vertices,
            image,
            source,
            frame: None,
            last_state: None,
        })
    }

    pub fn set_frame(&mut self, frame: Option<Frame>) {
        self.frame = frame;
    }

    pub fn update(&mut self, queue: &wgpu::Queue, view_projection: Mat4, background: [f32; 3]) {
        let Some(frame) = self.frame else {
            return;
        };
        if self.source.atlas.needs_update(frame.tick) && self.source.atlas.update(frame.tick) {
            let atlas = &self.source.atlas.texture;
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.image,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &atlas.rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(atlas.width * 4),
                    rows_per_image: Some(atlas.height),
                },
                wgpu::Extent3d {
                    width: atlas.width,
                    height: atlas.height,
                    depth_or_array_layers: 1,
                },
            );
        }
        let matrix = (view_projection * sun_model(frame.angle_degrees)).to_cols_array_2d();
        let state = [
            matrix[0],
            matrix[1],
            matrix[2],
            matrix[3],
            [background[0], background[1], background[2], 1.0],
            [frame.rain_brightness, 0.0, 0.0, 0.0],
        ];
        if self.last_state != Some(state) {
            queue.write_buffer(&self.uniform, 0, bytemuck::cast_slice(&state));
            self.last_state = Some(state);
        }
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        if self.frame.is_none() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bindings, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.draw(0..6, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    struct Pack {
        image: Vec<u8>,
        metadata: Option<Vec<u8>>,
    }

    impl ResourceProvider for Pack {
        fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(match path {
                "assets/minecraft/textures/environment/celestial/sun.png" => {
                    Some(self.image.clone())
                }
                "assets/minecraft/textures/environment/celestial/sun.png.mcmeta" => {
                    self.metadata.clone()
                }
                _ => None,
            })
        }
    }

    fn pack(animated: bool) -> Pack {
        let mut image = image::RgbaImage::from_pixel(
            2,
            if animated { 4 } else { 2 },
            image::Rgba([128, 64, 32, 128]),
        );
        if animated {
            for y in 2..4 {
                for x in 0..2 {
                    image.put_pixel(x, y, image::Rgba([32, 64, 128, 255]));
                }
            }
        }
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        Pack {
            image: png.into_inner(),
            metadata: animated.then(|| br#"{"animation":{"frametime":1}}"#.to_vec()),
        }
    }

    #[test]
    fn sprite_uses_resource_pack_and_animated_frames() {
        let mut source = Texture::load(&mut pack(true)).unwrap();
        let rect = source.atlas.rectangle(&source.id).unwrap();
        assert_eq!((rect.width, rect.height), (2, 2));
        let before = source.atlas.texture.rgba.clone();
        assert!(source.atlas.needs_update(1));
        assert!(source.atlas.update(1));
        assert_ne!(source.atlas.texture.rgba, before);
        assert!(!source.atlas.needs_update(1));
        let vertices = quad(&source).unwrap();
        assert_eq!(vertices[0].position, [-1.0, 0.0, -1.0]);
        assert_eq!(vertices[2].position, [1.0, 0.0, 1.0]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn celestial_transform_matches_java_joml() {
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
        let source = temp.path().join("SunOracle.java");
        let output = temp.path().join("sun.json");
        std::fs::write(&source, include_str!("../tests/java/SunOracle.java")).unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("SunOracle")
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let expected: Vec<[f32; 3]> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(expected.len(), 32);
        let mut expected = expected.into_iter();
        for angle in [0.0, 45.0, 90.0, 180.0, 270.0, 359.5, -90.0, 450.0] {
            for vertex in [
                Vec3::new(-1.0, 0.0, -1.0),
                Vec3::new(1.0, 0.0, -1.0),
                Vec3::new(1.0, 0.0, 1.0),
                Vec3::new(-1.0, 0.0, 1.0),
            ] {
                for (actual, expected) in sun_model(angle)
                    .transform_point3(vertex)
                    .to_array()
                    .into_iter()
                    .zip(expected.next().unwrap())
                {
                    assert!(
                        (actual - expected).abs() < 0.000_03,
                        "angle {angle}: {actual} != {expected}"
                    );
                }
            }
        }
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        Texture::load(&mut resources).unwrap();
    }

    #[test]
    #[ignore = "requires a GPU; renders offscreen without opening a window"]
    fn gpu_sun_color_and_rain() {
        use std::{sync::mpsc, time::Duration};
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
            for format in [
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureFormat::Rgba8UnormSrgb,
            ] {
                let mut sun = Sun::new(
                    &device,
                    &queue,
                    format,
                    wgpu::TextureFormat::Depth32Float,
                    Texture::load(&mut pack(false)).unwrap(),
                )
                .unwrap();
                let size = wgpu::Extent3d {
                    width: 64,
                    height: 64,
                    depth_or_array_layers: 1,
                };
                let color = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("sun test output"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
                let depth = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("sun test depth"),
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
                    label: Some("sun test readback"),
                    size: 64 * 64 * 4,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                for rain_brightness in [0.0_f32, 0.5, 1.0] {
                    let background = [0.1, 0.2, 0.3];
                    let matrix = Mat4::perspective_rh(70_f32.to_radians(), 1.0, 0.05, 1024.0)
                        * Mat4::look_to_rh(Vec3::ZERO, Vec3::Y, Vec3::Z);
                    sun.set_frame(Some(Frame {
                        angle_degrees: 0.0,
                        rain_brightness,
                        tick: 0,
                    }));
                    sun.update(&queue, matrix, background);
                    let mut encoder = device.create_command_encoder(&Default::default());
                    {
                        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("sun test"),
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
                        sun.draw(&mut pass);
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
                    let pixel = &pixels[(32 * 64 + 32) * 4..][..4];
                    for (i, value) in [128.0, 64.0, 32.0].into_iter().enumerate() {
                        let expected = (background[i] * 255.0
                            + value * (128.0 / 255.0) * rain_brightness)
                            .round() as i32;
                        assert!(
                            (i32::from(pixel[i]) - expected).abs() <= 1,
                            "{format:?}, rain {rain_brightness}: {pixel:?}, channel {i} expected {expected}"
                        );
                    }
                    assert_eq!(pixel[3], 255);
                    drop(pixels);
                    readback.unmap();
                }
            }
            if let Some(error) = errors.pop().await {
                panic!("GPU validation failed: {error}");
            }
        });
    }
}
