//! The 26.3 End sky cube, using the active resource pack's texture.

use anyhow::{Context, Result, ensure};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use image::RgbaImage;
use native_resources::packs::ResourceProvider;
use wgpu::util::DeviceExt;

pub struct Texture {
    image: RgbaImage,
    blur: bool,
    clamp: bool,
}

impl Texture {
    pub fn load(source: &mut impl ResourceProvider) -> Result<Self> {
        let asset = source
            .read_asset_with_metadata("assets/minecraft/textures/environment/end_sky.png")?
            .context("missing End sky texture")?;
        let image = image::load_from_memory(&asset.bytes)
            .context("invalid End sky texture")?
            .to_rgba8();
        let metadata = asset
            .metadata
            .as_deref()
            .map(serde_json::from_slice::<serde_json::Value>)
            .transpose()
            .context("invalid End sky texture metadata")?;
        let texture = metadata
            .as_ref()
            .and_then(|metadata| metadata.get("texture"));
        if let Some(value) = texture {
            ensure!(
                value.is_object(),
                "invalid End sky texture metadata section"
            );
        }
        let setting = |name: &str| -> Result<bool> {
            match texture.and_then(|texture| texture.get(name)) {
                Some(value) => value.as_bool().context("invalid End sky sampler setting"),
                None => Ok(false),
            }
        };
        Ok(Self {
            image,
            blur: setting("blur")?,
            clamp: setting("clamp")?,
        })
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    uv: [f32; 2],
}

fn vertices() -> Vec<Vertex> {
    let rotations = [
        Mat4::IDENTITY,
        Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2),
        Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2),
        Mat4::from_rotation_x(std::f32::consts::PI),
        Mat4::from_rotation_z(std::f32::consts::FRAC_PI_2),
        Mat4::from_rotation_z(-std::f32::consts::FRAC_PI_2),
    ];
    let positions = [
        Vec3::new(-100.0, -100.0, -100.0),
        Vec3::new(-100.0, -100.0, 100.0),
        Vec3::new(100.0, -100.0, 100.0),
        Vec3::new(100.0, -100.0, -100.0),
    ];
    let uvs = [[0.0, 0.0], [0.0, 16.0], [16.0, 16.0], [16.0, 0.0]];
    rotations
        .into_iter()
        .flat_map(|rotation| {
            [0, 1, 2, 0, 2, 3].map(move |i| Vertex {
                position: rotation.transform_point3(positions[i]).to_array(),
                uv: uvs[i],
            })
        })
        .collect()
}

pub struct EndSky {
    pipeline: wgpu::RenderPipeline,
    bindings: wgpu::BindGroup,
    matrix: wgpu::Buffer,
    vertices: wgpu::Buffer,
    last_state: Option<[[f32; 4]; 5]>,
}

impl EndSky {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        source: &Texture,
    ) -> Result<Self> {
        let (width, height) = source.image.dimensions();
        ensure!(
            width > 0
                && height > 0
                && width <= device.limits().max_texture_dimension_2d
                && height <= device.limits().max_texture_dimension_2d,
            "End sky texture exceeds GPU limits"
        );
        let extent = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let image = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("End sky texture"),
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
            source.image.as_raw(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            extent,
        );
        let address = if source.clamp {
            wgpu::AddressMode::ClampToEdge
        } else {
            wgpu::AddressMode::Repeat
        };
        let filter = if source.blur {
            wgpu::FilterMode::Linear
        } else {
            wgpu::FilterMode::Nearest
        };
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("End sky sampler"),
            address_mode_u: address,
            address_mode_v: address,
            mag_filter: filter,
            min_filter: filter,
            ..Default::default()
        });
        let matrix = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("End sky camera"),
            size: 80,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("End sky bindings"),
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
        let view = image.create_view(&wgpu::TextureViewDescriptor::default());
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("End sky bindings"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: matrix.as_entire_binding(),
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
            label: Some("End sky layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("End sky shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("end_sky.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("End sky pipeline"),
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
            label: Some("End sky cube"),
            contents: bytemuck::cast_slice(&vertices()),
            usage: wgpu::BufferUsages::VERTEX,
        });
        Ok(Self {
            pipeline,
            bindings,
            matrix,
            vertices,
            last_state: None,
        })
    }

    pub fn update(&mut self, queue: &wgpu::Queue, matrix: Mat4, background: [f32; 3]) {
        let columns = matrix.to_cols_array_2d();
        let state = [
            columns[0],
            columns[1],
            columns[2],
            columns[3],
            [background[0], background[1], background[2], 1.0],
        ];
        if self.last_state != Some(state) {
            queue.write_buffer(&self.matrix, 0, bytemuck::cast_slice(&state));
            self.last_state = Some(state);
        }
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bindings, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.draw(0..36, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_pack_texture_and_sampler_metadata_are_loaded() {
        use std::io::Cursor;
        struct Pack {
            image: Vec<u8>,
            metadata: Option<Vec<u8>>,
        }
        impl ResourceProvider for Pack {
            fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                Ok(match path {
                    "assets/minecraft/textures/environment/end_sky.png" => Some(self.image.clone()),
                    "assets/minecraft/textures/environment/end_sky.png.mcmeta" => {
                        self.metadata.clone()
                    }
                    _ => None,
                })
            }
        }
        let source = RgbaImage::from_pixel(2, 2, image::Rgba([20, 40, 60, 255]));
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(source.clone())
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let mut pack = Pack {
            image: png.into_inner(),
            metadata: Some(br#"{"texture":{"blur":true,"clamp":true}}"#.to_vec()),
        };
        let texture = Texture::load(&mut pack).unwrap();
        assert_eq!(texture.image, source);
        assert!(texture.blur && texture.clamp);
        pack.metadata = None;
        let texture = Texture::load(&mut pack).unwrap();
        assert!(!texture.blur && !texture.clamp);
        pack.metadata = Some(br#"{"texture":{"blur":"bad"}}"#.to_vec());
        assert!(Texture::load(&mut pack).is_err());
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn cube_vertices_match_java_end_sky_matrices() {
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
        let source = temp.path().join("EndSkyOracle.java");
        let output = temp.path().join("end_sky.json");
        std::fs::write(&source, include_str!("../tests/java/EndSkyOracle.java")).unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("EndSkyOracle")
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
        let expected: Vec<[f32; 5]> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(expected.len(), 36);
        for (actual, expected) in vertices().iter().zip(expected) {
            for (actual, expected) in actual.position.into_iter().chain(actual.uv).zip(expected) {
                assert!(
                    (actual - expected).abs() < 0.000_03,
                    "{actual} != {expected}"
                );
            }
        }
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let image = Texture::load(&mut resources).unwrap();
        assert!(image.image.width() > 0 && image.image.height() > 0);
        assert!(!image.blur && !image.clamp);
    }

    #[test]
    #[ignore = "requires a GPU; renders offscreen without opening a window"]
    fn gpu_draws_all_six_cube_faces() {
        use std::{sync::mpsc, time::Duration};
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
                .expect("GPU adapter unavailable");
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .unwrap();
            let errors = device.push_error_scope(wgpu::ErrorFilter::Validation);
            for (source, background, expected) in [
                ([128, 64, 32, 255], [0.0, 0.0, 0.0], [20_i32, 10, 5, 255]),
                ([128, 64, 32, 128], [0.1, 0.2, 0.3], [23_i32, 30, 41, 255]),
            ] {
                let image = Texture {
                    image: RgbaImage::from_pixel(1, 1, image::Rgba(source)),
                    blur: false,
                    clamp: false,
                };
                for format in [
                    wgpu::TextureFormat::Rgba8Unorm,
                    wgpu::TextureFormat::Rgba8UnormSrgb,
                ] {
                    let mut sky = EndSky::new(
                        &device,
                        &queue,
                        format,
                        wgpu::TextureFormat::Depth32Float,
                        &image,
                    )
                    .unwrap();
                    let size = wgpu::Extent3d {
                        width: 64,
                        height: 64,
                        depth_or_array_layers: 1,
                    };
                    let color = device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("End sky output"),
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
                        label: Some("End sky depth"),
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
                        label: Some("End sky readback"),
                        size: 64 * 64 * 4,
                        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    });
                    for direction in [
                        Vec3::X,
                        Vec3::NEG_X,
                        Vec3::Y,
                        Vec3::NEG_Y,
                        Vec3::Z,
                        Vec3::NEG_Z,
                    ] {
                        let up = if direction.y.abs() > 0.5 {
                            Vec3::Z
                        } else {
                            Vec3::Y
                        };
                        let matrix = Mat4::perspective_rh(70_f32.to_radians(), 1.0, 0.05, 1024.0)
                            * Mat4::look_to_rh(Vec3::ZERO, direction, up);
                        sky.update(&queue, matrix, background);
                        let mut encoder = device.create_command_encoder(&Default::default());
                        {
                            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                                label: Some("End sky test"),
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
                            sky.draw(&mut pass);
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
                        for (index, pixel) in pixels.chunks_exact(4).enumerate() {
                            for (actual, expected) in pixel.iter().zip(expected) {
                                assert!(
                                    (i32::from(*actual) - expected).abs() <= 1,
                                    "{direction:?} {format:?} pixel {index}: {pixel:?}"
                                );
                            }
                        }
                        drop(pixels);
                        readback.unmap();
                    }
                }
            }
            if let Some(error) = errors.pop().await {
                panic!("GPU validation failed: {error}");
            }
        });
    }
}
