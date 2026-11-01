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
    ids: Vec<ResourceId>,
    size: f32,
    flip_uv: bool,
}

impl Texture {
    pub fn load(source: &mut impl ResourceProvider) -> Result<Self> {
        let id = ResourceId::parse("minecraft:environment/celestial/sun")?;
        let atlas = SpriteAtlas::build(source, [id.clone()], 4096)?;
        Ok(Self {
            atlas,
            ids: vec![id],
            size: 30.0,
            flip_uv: false,
        })
    }

    pub fn load_moon(source: &mut impl ResourceProvider) -> Result<Self> {
        let ids: Vec<_> = crate::moon_phase::Phase::ALL
            .into_iter()
            .map(|phase| {
                ResourceId::parse(&format!(
                    "minecraft:environment/celestial/moon/{}",
                    phase.name()
                ))
            })
            .collect::<Result<_>>()?;
        let atlas = SpriteAtlas::build(source, ids.iter().cloned(), 4096)?;
        Ok(Self {
            atlas,
            ids,
            size: 20.0,
            flip_uv: true,
        })
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    uv: [f32; 2],
}

fn quad(texture: &Texture, id: &ResourceId) -> Result<[Vertex; 6]> {
    let rect = texture
        .atlas
        .rectangle(id)
        .context("celestial sprite missing from atlas")?;
    let mut u0 = rect.x as f32 / texture.atlas.texture.width as f32;
    let mut v0 = rect.y as f32 / texture.atlas.texture.height as f32;
    let mut u1 = (rect.x + rect.width) as f32 / texture.atlas.texture.width as f32;
    let mut v1 = (rect.y + rect.height) as f32 / texture.atlas.texture.height as f32;
    if texture.flip_uv {
        std::mem::swap(&mut u0, &mut u1);
        std::mem::swap(&mut v0, &mut v1);
    }
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

fn sun_model(angle_degrees: f32, size: f32) -> Mat4 {
    Mat4::from_rotation_y(-std::f32::consts::FRAC_PI_2)
        * Mat4::from_rotation_x(angle_degrees * 0.017_453_292_f32)
        * Mat4::from_translation(Vec3::new(0.0, 100.0, 0.0))
        * Mat4::from_scale(Vec3::new(size, 1.0, size))
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
    last_state: Option<[[f32; 4]; 5]>,
    sprite_index: usize,
}

impl Sun {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        depth_format: Option<wgpu::TextureFormat>,
        source: Texture,
    ) -> Result<Self> {
        ensure!(
            !format.is_srgb(),
            "celestial blending requires an unorm target"
        );
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
            size: 80,
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
            depth_stencil: depth_format.map(|format| wgpu::DepthStencilState {
                format,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
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
        let vertices_data: Vec<Vertex> = source
            .ids
            .iter()
            .map(|id| quad(&source, id))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect();
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Sun quad"),
            contents: bytemuck::cast_slice(&vertices_data),
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
            sprite_index: 0,
        })
    }

    pub fn set_frame(&mut self, frame: Option<Frame>) {
        self.frame = frame;
    }

    pub fn set_sprite(&mut self, index: usize) {
        debug_assert!(index < self.source.ids.len());
        self.sprite_index = index;
    }

    pub fn update(&mut self, queue: &wgpu::Queue, view_projection: Mat4) {
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
        let model = sun_model(frame.angle_degrees, self.source.size);
        let matrix = (view_projection * model).to_cols_array_2d();
        let state = [
            matrix[0],
            matrix[1],
            matrix[2],
            matrix[3],
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
        let start = (self.sprite_index * 6) as u32;
        pass.draw(start..start + 6, 0..1);
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
                path if path
                    .starts_with("assets/minecraft/textures/environment/celestial/moon/")
                    && path.ends_with(".png") =>
                {
                    Some(self.image.clone())
                }
                path if path
                    .starts_with("assets/minecraft/textures/environment/celestial/moon/")
                    && path.ends_with(".png.mcmeta") =>
                {
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
        let rect = source.atlas.rectangle(&source.ids[0]).unwrap();
        assert_eq!((rect.width, rect.height), (2, 2));
        let before = source.atlas.texture.rgba.clone();
        assert!(source.atlas.needs_update(1));
        assert!(source.atlas.update(1));
        assert_ne!(source.atlas.texture.rgba, before);
        assert!(!source.atlas.needs_update(1));
        let vertices = quad(&source, &source.ids[0]).unwrap();
        assert_eq!(vertices[0].position, [-1.0, 0.0, -1.0]);
        assert_eq!(vertices[2].position, [1.0, 0.0, 1.0]);
    }

    #[test]
    fn moon_phase_quads_select_the_matching_sprite() {
        struct MoonPack;
        impl ResourceProvider for MoonPack {
            fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                for (index, phase) in crate::moon_phase::Phase::ALL.into_iter().enumerate() {
                    if path
                        == format!(
                            "assets/minecraft/textures/environment/celestial/moon/{}.png",
                            phase.name()
                        )
                    {
                        let image = image::RgbaImage::from_pixel(
                            1,
                            1,
                            image::Rgba([index as u8 * 23, 17, 31, 255]),
                        );
                        let mut png = Cursor::new(Vec::new());
                        image::DynamicImage::ImageRgba8(image)
                            .write_to(&mut png, image::ImageFormat::Png)?;
                        return Ok(Some(png.into_inner()));
                    }
                }
                Ok(None)
            }
        }
        let moon = Texture::load_moon(&mut MoonPack).unwrap();
        assert_eq!(moon.size, 20.0);
        for phase in crate::moon_phase::Phase::ALL {
            let index = phase.index();
            let rect = moon.atlas.rectangle(&moon.ids[index]).unwrap();
            let offset = ((rect.y * moon.atlas.texture.width + rect.x) * 4) as usize;
            assert_eq!(moon.atlas.texture.rgba[offset], index as u8 * 23);
            let vertices = quad(&moon, &moon.ids[index]).unwrap();
            let u0 = rect.x as f32 / moon.atlas.texture.width as f32;
            let v0 = rect.y as f32 / moon.atlas.texture.height as f32;
            let u1 = (rect.x + rect.width) as f32 / moon.atlas.texture.width as f32;
            let v1 = (rect.y + rect.height) as f32 / moon.atlas.texture.height as f32;
            assert_eq!(vertices[0].uv, [u1, v1]);
            assert_eq!(vertices[1].uv, [u0, v1]);
            assert_eq!(vertices[2].uv, [u0, v0]);
            assert_eq!(vertices[5].uv, [u1, v0]);
        }
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
        assert_eq!(expected.len(), 64);
        let mut expected = expected.into_iter();
        for size in [30.0, 20.0] {
            for angle in [0.0, 45.0, 90.0, 180.0, 270.0, 359.5, -90.0, 450.0] {
                for vertex in [
                    Vec3::new(-1.0, 0.0, -1.0),
                    Vec3::new(1.0, 0.0, -1.0),
                    Vec3::new(1.0, 0.0, 1.0),
                    Vec3::new(-1.0, 0.0, 1.0),
                ] {
                    for (actual, expected) in sun_model(angle, size)
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
        }
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        Texture::load(&mut resources).unwrap();
        let moon = Texture::load_moon(&mut resources).unwrap();
        assert_eq!(moon.ids.len(), 8);
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
            for format in [wgpu::TextureFormat::Rgba8Unorm] {
                let errors = device.push_error_scope(wgpu::ErrorFilter::Validation);
                let mut sun = Sun::new(
                    &device,
                    &queue,
                    format,
                    Some(wgpu::TextureFormat::Depth32Float),
                    Texture::load(&mut pack(false)).unwrap(),
                )
                .unwrap();
                let mut moon = Sun::new(
                    &device,
                    &queue,
                    format,
                    Some(wgpu::TextureFormat::Depth32Float),
                    Texture::load_moon(&mut pack(false)).unwrap(),
                )
                .unwrap();
                if let Some(error) = errors.pop().await {
                    panic!("GPU pipeline validation failed: {error}");
                }
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
                    for sun_angle in [0.0_f32, 10.0, 20.0, 180.0] {
                        let background = [0.1, 0.2, 0.3];
                        let matrix = Mat4::perspective_rh(70_f32.to_radians(), 1.0, 0.05, 1024.0)
                            * Mat4::look_to_rh(Vec3::ZERO, Vec3::Y, Vec3::Z);
                        sun.set_frame(Some(Frame {
                            angle_degrees: sun_angle,
                            rain_brightness,
                            tick: 0,
                        }));
                        sun.update(&queue, matrix);
                        moon.set_frame(Some(Frame {
                            angle_degrees: 0.0,
                            rain_brightness,
                            tick: 0,
                        }));
                        moon.set_sprite(4);
                        moon.update(&queue, matrix);
                        let mut encoder = device.create_command_encoder(&Default::default());
                        {
                            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                                label: Some("sun test"),
                                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                    view: &view,
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
                            moon.draw(&mut pass);
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
                                + value
                                    * (128.0 / 255.0)
                                    * rain_brightness
                                    * if sun_angle <= 10.0 { 2.0 } else { 1.0 })
                            .round() as i32;
                            assert!(
                                (i32::from(pixel[i]) - expected).abs() <= 1,
                                "{format:?}, angle {sun_angle}, rain {rain_brightness}: {pixel:?}, channel {i} expected {expected}"
                            );
                        }
                        assert!(
                            (i32::from(pixel[3]) - (128.0 * rain_brightness).round() as i32).abs()
                                <= 1
                        );
                        drop(pixels);
                        readback.unmap();
                    }
                }
            }
        });
    }
}
