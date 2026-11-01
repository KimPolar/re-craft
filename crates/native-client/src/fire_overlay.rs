//! First-person fire overlay using the active pack's animated fire sprite.
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
        let id = ResourceId::parse("minecraft:block/fire_1")?;
        let atlas = SpriteAtlas::build(source, [id.clone()], 4096)?;
        Ok(Self { atlas, id })
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    uv: [f32; 2],
}

fn quad_vertices(texture: &Texture) -> Result<[Vertex; 12]> {
    let rect = texture
        .atlas
        .rectangle(&texture.id)
        .context("missing fire sprite")?;
    let width = texture.atlas.texture.width as f32;
    let height = texture.atlas.texture.height as f32;
    let (u0, u1) = (rect.x as f32 / width, (rect.x + rect.width) as f32 / width);
    let (v0, v1) = (
        rect.y as f32 / height,
        (rect.y + rect.height) as f32 / height,
    );
    let quad = [
        (Vec3::new(-0.5, -0.5, -0.5), [u1, v1]),
        (Vec3::new(0.5, -0.5, -0.5), [u0, v1]),
        (Vec3::new(0.5, 0.5, -0.5), [u0, v0]),
        (Vec3::new(-0.5, 0.5, -0.5), [u1, v0]),
    ];
    let mut out = [Vertex {
        position: [0.0; 3],
        uv: [0.0; 2],
    }; 12];
    for (index, (x, angle)) in [(0.24, -0.174_532_92), (-0.24, 0.174_532_92)]
        .into_iter()
        .enumerate()
    {
        let matrix = Mat4::from_translation(Vec3::new(x, -0.3, 0.0)) * Mat4::from_rotation_y(angle);
        for (vertex, point) in [0, 1, 2, 0, 2, 3].into_iter().enumerate() {
            out[index * 6 + vertex] = Vertex {
                position: matrix.transform_point3(quad[point].0).to_array(),
                uv: quad[point].1,
            };
        }
    }
    Ok(out)
}

pub struct Overlay {
    pipeline: wgpu::RenderPipeline,
    binding: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    vertices: wgpu::Buffer,
    image: wgpu::Texture,
    source: Texture,
    tick: Option<u64>,
    last_projection: Option<[[f32; 4]; 4]>,
}

impl Overlay {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        source: Texture,
    ) -> Result<Self> {
        let atlas = &source.atlas.texture;
        ensure!(
            atlas.width > 0
                && atlas.height > 0
                && atlas.width <= device.limits().max_texture_dimension_2d
                && atlas.height <= device.limits().max_texture_dimension_2d,
            "fire atlas exceeds GPU limits"
        );
        let extent = wgpu::Extent3d {
            width: atlas.width,
            height: atlas.height,
            depth_or_array_layers: 1,
        };
        let image = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("fire overlay sprite"),
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
            label: Some("fire overlay sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fire overlay projection"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fire overlay layout"),
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
                    visibility: wgpu::ShaderStages::VERTEX,
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
            label: Some("fire overlay binding"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &image.create_view(&Default::default()),
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
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fire overlay shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("fire_overlay.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fire overlay pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fire overlay pipeline"),
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
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("fire overlay vertices"),
            contents: bytemuck::cast_slice(&quad_vertices(&source)?),
            usage: wgpu::BufferUsages::VERTEX,
        });
        Ok(Self {
            pipeline,
            binding,
            uniform,
            vertices,
            image,
            source,
            tick: None,
            last_projection: None,
        })
    }

    pub fn set_tick(&mut self, tick: Option<u64>) {
        self.tick = tick;
    }

    pub fn draw(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
        projection: Mat4,
    ) {
        let Some(tick) = self.tick else { return };
        if self.source.atlas.needs_update(tick) && self.source.atlas.update(tick) {
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
        let matrix = projection.to_cols_array_2d();
        if self.last_projection != Some(matrix) {
            queue.write_buffer(&self.uniform, 0, bytemuck::cast_slice(&matrix));
            self.last_projection = Some(matrix);
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("fire overlay pass"),
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
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.draw(0..12, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Cursor, sync::mpsc, time::Duration};

    struct Pack {
        png: Vec<u8>,
        metadata: Option<Vec<u8>>,
    }
    impl ResourceProvider for Pack {
        fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(match path {
                "assets/minecraft/textures/block/fire_1.png" => Some(self.png.clone()),
                "assets/minecraft/textures/block/fire_1.png.mcmeta" => self.metadata.clone(),
                _ => None,
            })
        }
    }

    fn texture(animated: bool) -> Texture {
        let mut image = image::RgbaImage::new(2, if animated { 4 } else { 2 });
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            let _ = x;
            *pixel = if y < 2 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 255, 0, 255])
            };
        }
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        Texture::load(&mut Pack {
            png: png.into_inner(),
            metadata: animated.then(|| br#"{"animation":{"frametime":1}}"#.to_vec()),
        })
        .unwrap()
    }

    #[test]
    fn loads_and_animates_active_pack_sprite() {
        let mut texture = texture(true);
        let rect = texture.atlas.rectangle(&texture.id).unwrap();
        assert_eq!([rect.width, rect.height], [2, 2]);
        assert!(texture.atlas.update(1));
        let offset = ((rect.y * texture.atlas.texture.width + rect.x) * 4) as usize;
        assert_eq!(
            &texture.atlas.texture.rgba[offset..offset + 4],
            &[0, 255, 0, 255]
        );
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn fire_quad_geometry_and_color_match_java() {
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
        let source = temp.path().join("FireOverlayOracle.java");
        let output = temp.path().join("vertices.txt");
        std::fs::write(
            &source,
            include_str!("../tests/java/FireOverlayOracle.java"),
        )
        .unwrap();
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let result = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("FireOverlayOracle")
            .arg(&output)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        let text = std::fs::read_to_string(output).unwrap();
        let rows: Vec<Vec<&str>> = text
            .lines()
            .map(|line| line.split_whitespace().collect())
            .collect();
        assert_eq!(rows.len(), 8);
        let source = texture(false);
        let rect = source.atlas.rectangle(&source.id).unwrap();
        let (u0, v0) = (
            rect.x as f32 / source.atlas.texture.width as f32,
            rect.y as f32 / source.atlas.texture.height as f32,
        );
        let (uw, vh) = (
            rect.width as f32 / source.atlas.texture.width as f32,
            rect.height as f32 / source.atlas.texture.height as f32,
        );
        let vertices = quad_vertices(&source).unwrap();
        for (quad, indices) in [[0, 1, 2, 5], [6, 7, 8, 11]].into_iter().enumerate() {
            for (i, index) in indices.into_iter().enumerate() {
                let row = &rows[quad * 4 + i];
                let expected: Vec<f32> = row[..5].iter().map(|v| v.parse().unwrap()).collect();
                for (actual, expected) in vertices[index].position.into_iter().zip(&expected[..3]) {
                    assert!((actual - expected).abs() < 1e-6, "{actual} vs {expected}");
                }
                assert!((vertices[index].uv[0] - (u0 + expected[3] * uw)).abs() < 1e-6);
                assert!((vertices[index].uv[1] - (v0 + expected[4] * vh)).abs() < 1e-6);
                assert_eq!(row[5].parse::<i32>().unwrap() as u32, 0xe5ff_ffff);
            }
        }
    }

    #[test]
    #[ignore = "requires a GPU; renders offscreen without opening a window"]
    fn gpu_fire_overlay_blends_animates_and_stops() {
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
                .unwrap();
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .unwrap();
            let mut overlay = Overlay::new(
                &device,
                &queue,
                wgpu::TextureFormat::Rgba8Unorm,
                texture(true),
            )
            .unwrap();
            let size = wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            };
            let target = device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = target.create_view(&Default::default());
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 64 * 64 * 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            for tick in [None, Some(0), Some(1), None] {
                overlay.set_tick(tick);
                let mut encoder = device.create_command_encoder(&Default::default());
                {
                    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
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
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                }
                overlay.draw(
                    &queue,
                    &mut encoder,
                    &view,
                    Mat4::perspective_rh(70f32.to_radians(), 1.0, 0.05, 1024.0),
                );
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: &target,
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
                let colored = data
                    .chunks_exact(4)
                    .filter(|pixel| pixel[0] > 0 || pixel[1] > 0)
                    .count();
                match tick {
                    None => assert_eq!(colored, 0),
                    Some(frame) => {
                        assert!(colored > 100);
                        let channel = frame as usize;
                        assert!(
                            data.chunks_exact(4)
                                .any(|pixel| (228..=230).contains(&pixel[channel])
                                    && pixel[1 - channel] == 0)
                        );
                    }
                }
                drop(data);
                readback.unmap();
            }
        });
    }
}
