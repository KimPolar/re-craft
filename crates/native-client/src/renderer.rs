use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use image::RgbaImage;
use native_resources::TextureAtlas;
use native_session::{WorldSnapshot, mining::BreakingProgress};
use native_world::loaded::ChunkPos;
use wgpu::util::DeviceExt;
use winit::{dpi::PhysicalSize, window::Window};

use crate::{
    breaking_render::BreakingRenderer,
    consume_particle_render::{ParticleRenderer, RenderParticle},
    first_person::{FirstPersonInput, FirstPersonRenderer},
    ground_items::GroundItemRenderer,
    held_items::HeldModel,
    item_render::ItemRenderer,
    menu_draw::Canvas,
    mesh::{self, Vertex},
    orb_render::OrbRenderer,
    preview::{ChunkMesh, PreviewScene, TransparentQuad},
};

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub(crate) const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32, 3 => Float32x3, 4 => Uint32];

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CameraUniform {
    view_projection: [[f32; 4]; 4],
    viewport: [f32; 2],
    padding: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Pod, Zeroable)]
struct FogUniform {
    eye: [f32; 4],
    color: [f32; 4],
    range: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Pod, Zeroable)]
struct GlintUniform {
    offset: [f32; 2],
    strength: f32,
    active: f32,
}

fn glint_uniform(elapsed_ms: u64, speed: f64, strength: f32, active: bool) -> GlintUniform {
    let time = (elapsed_ms as f64 * speed.clamp(0.0, 1.0) * 8.0) as i64;
    GlintUniform {
        offset: [
            -((time % 110_000) as f32 / 110_000.0),
            (time % 30_000) as f32 / 30_000.0,
        ],
        strength: strength.clamp(0.0, 1.0),
        active: if active { 1.0 } else { 0.0 },
    }
}

fn glint_uniforms(
    elapsed_ms: u64,
    speed: f64,
    strength: f32,
    overlay: bool,
    hand: bool,
) -> (GlintUniform, GlintUniform) {
    let gui = glint_uniform(elapsed_ms, speed, strength, overlay);
    let mut hand_uniform = gui;
    hand_uniform.active = if hand { 1.0 } else { 0.0 };
    (gui, hand_uniform)
}

struct PreviewChunkGpu {
    opaque: Arc<[Vertex]>,
    translucent: Arc<[TransparentQuad]>,
    buffer: Option<wgpu::Buffer>,
    capacity: usize,
    count: u32,
    chest_lids: Arc<[crate::preview::ChestLidMesh]>,
    animated_lids: BTreeMap<(i32, i32, i32), u32>,
}

impl Default for PreviewChunkGpu {
    fn default() -> Self {
        Self {
            opaque: Arc::from([]),
            translucent: Arc::from([]),
            buffer: None,
            capacity: 0,
            count: 0,
            chest_lids: Arc::from([]),
            animated_lids: BTreeMap::new(),
        }
    }
}

fn same_shared_slice<T>(a: &Arc<[T]>, b: &Arc<[T]>) -> bool {
    (a.is_empty() && b.is_empty()) || Arc::ptr_eq(a, b)
}

fn translucent_buffer_size(quad_count: usize, limit: usize) -> Result<(u32, usize)> {
    let vertices = quad_count
        .checked_mul(6)
        .context("translucent vertex count overflow")?;
    let bytes = vertices
        .checked_mul(std::mem::size_of::<Vertex>())
        .context("translucent buffer size overflow")?;
    ensure!(
        bytes <= limit,
        "translucent terrain buffer exceeds GPU limits"
    );
    Ok((
        u32::try_from(vertices).context("too many translucent vertices")?,
        bytes,
    ))
}

impl PreviewChunkGpu {
    fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mesh: &ChunkMesh,
    ) -> Result<(bool, bool, bool)> {
        let opaque_changed = !same_shared_slice(&self.opaque, &mesh.opaque);
        let transparent_changed = !same_shared_slice(&self.translucent, &mesh.translucent);
        let mut uploaded = false;
        let mut draw_changed = false;
        if opaque_changed {
            self.animated_lids.clear();
            let previous_count = self.count;
            self.count = u32::try_from(mesh.opaque.len()).context("too many chunk vertices")?;
            draw_changed = previous_count != self.count;
            let bytes = bytemuck::cast_slice::<Vertex, u8>(&mesh.opaque);
            if !bytes.is_empty() {
                let limit = usize::try_from(device.limits().max_buffer_size).unwrap_or(usize::MAX);
                ensure!(
                    bytes.len() <= limit,
                    "chunk vertex buffer exceeds GPU limits"
                );
                if self.capacity < bytes.len() {
                    draw_changed = true;
                    self.capacity = bytes
                        .len()
                        .checked_next_power_of_two()
                        .unwrap_or(bytes.len())
                        .min(limit);
                    self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("chunk terrain vertices"),
                        size: self.capacity as u64,
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    }));
                }
                queue.write_buffer(
                    self.buffer
                        .as_ref()
                        .context("missing chunk vertex buffer")?,
                    0,
                    bytes,
                );
                uploaded = true;
            }
            self.opaque = Arc::clone(&mesh.opaque);
        }
        if transparent_changed {
            self.translucent = Arc::clone(&mesh.translucent);
        }
        self.chest_lids = Arc::clone(&mesh.chest_lids);
        Ok((uploaded, transparent_changed, draw_changed))
    }
}

#[derive(Default)]
struct PreBlurOverlay {
    texture: Option<wgpu::Texture>,
    mask: Option<wgpu::Texture>,
    binding: Option<wgpu::BindGroup>,
    size: (u32, u32),
    has_glint: bool,
}

pub struct Renderer {
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    window: Arc<Window>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    base_fov: f32,
    fov_modifier: f32,
    view_yaw: f32,
    view_bobbing: bool,
    bob_transform: Mat4,
    depth: wgpu::TextureView,
    sky_clear: wgpu::Color,
    sky_rgb: Option<[f32; 3]>,
    fog_range: Option<[f32; 2]>,
    fog_buffer: wgpu::Buffer,
    fog_bind_group: wgpu::BindGroup,
    fog_last: Option<FogUniform>,
    camera_buffer: wgpu::Buffer,
    lightmap_buffer: wgpu::Buffer,
    lightmap_bind_group: wgpu::BindGroup,
    lightmap_last: Option<[[f32; 4]; 256]>,
    atlas_texture: wgpu::Texture,
    atlas_size: (u32, u32),
    atlas_bind_layout: wgpu::BindGroupLayout,
    atlas_sampler: wgpu::Sampler,
    bind_group: wgpu::BindGroup,
    terrain_pipeline: wgpu::RenderPipeline,
    translucent_pipeline: wgpu::RenderPipeline,
    breaking_renderer: Option<BreakingRenderer>,
    item_renderer: Option<ItemRenderer>,
    ground_items: Option<GroundItemRenderer>,
    items_prepared: bool,
    orb_renderer: Option<OrbRenderer>,
    consume_renderer: ParticleRenderer,
    consume_draws: Vec<RenderParticle>,
    consume_origin: glam::IVec3,
    consume_partial: f32,
    first_person: FirstPersonRenderer,
    crosshair_pipeline: wgpu::RenderPipeline,
    overlay_pipeline: wgpu::RenderPipeline,
    overlay_layout: wgpu::BindGroupLayout,
    overlay_sampler: wgpu::Sampler,
    overlay_texture: Option<wgpu::Texture>,
    overlay_glint_mask: Option<wgpu::Texture>,
    overlay_glint_view: wgpu::TextureView,
    overlay_glint_sampler: wgpu::Sampler,
    overlay_glint_uniform: wgpu::Buffer,
    overlay_glint_last: GlintUniform,
    hand_glint_uniform: wgpu::Buffer,
    hand_glint_last: GlintUniform,
    overlay_has_glint: bool,
    pre_blur_overlay: PreBlurOverlay,
    overlay_glint_started: Instant,
    overlay_glint_available: bool,
    overlay_bind_group: Option<wgpu::BindGroup>,
    overlay_size: (u32, u32),
    hide_crosshair: bool,
    screen_blur: Option<crate::screen_blur::ScreenBlur>,
    screen_blur_radius: u8,
    vertex_buffer: Option<wgpu::Buffer>,
    vertex_count: u32,
    translucent_buffer: Option<wgpu::Buffer>,
    translucent_capacity: usize,
    translucent_count: u32,
    mesh_revision: Option<u64>,
    preview_uploaded: bool,
    preview_mode: bool,
    preview_chunks: BTreeMap<ChunkPos, PreviewChunkGpu>,
    chest_visuals: Arc<[native_session::chest_lid::ChestLidVisual]>,
    chest_partial: f32,
    animated_chest_chunks: BTreeSet<ChunkPos>,
    preview_bundle: Option<wgpu::RenderBundle>,
    preview_bundle_dirty: bool,
    translucent_eye: Option<Vec3>,
    translucent_centers: Vec<Vec3>,
    translucent_dirty: bool,
    #[cfg(test)]
    preview_uploads: usize,
    #[cfg(test)]
    translucent_uploads: usize,
    #[cfg(test)]
    translucent_upload_bytes: Vec<u8>,
    #[cfg(test)]
    bundle_rebuilds: usize,
    lost_device: Arc<AtomicBool>,
    #[cfg(debug_assertions)]
    pub backend_name: String,
}

impl Renderer {
    pub fn set_fog_range(&mut self, range: Option<[f32; 2]>) {
        self.fog_range = range;
    }

    fn update_fog(&mut self, eye: Vec3) {
        let range = self.fog_range;
        let uniform = FogUniform {
            eye: [eye.x, eye.y, eye.z, 0.0],
            color: [
                self.sky_clear.r as f32,
                self.sky_clear.g as f32,
                self.sky_clear.b as f32,
                1.0,
            ],
            range: range.map_or([0.0, 1.0, 0.0, 0.0], |r| [r[0], r[1], 1.0, 0.0]),
        };
        if self.fog_last != Some(uniform) {
            self.queue
                .write_buffer(&self.fog_buffer, 0, bytemuck::bytes_of(&uniform));
            self.fog_last = Some(uniform);
        }
    }
    pub fn set_sky_color(&mut self, color: [f32; 3]) {
        if self.sky_rgb == Some(color) {
            return;
        }
        self.sky_rgb = Some(color);
        let convert = |component: f32| {
            let c = f64::from(component.clamp(0.0, 1.0));
            if !self.config.format.is_srgb() {
                c
            } else if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        self.sky_clear = wgpu::Color {
            r: convert(color[0]),
            g: convert(color[1]),
            b: convert(color[2]),
            a: 1.0,
        };
    }

    #[cfg(test)]
    pub fn reset_sky_color(&mut self) {
        self.sky_clear = wgpu::Color {
            r: 0.35,
            g: 0.58,
            b: 0.85,
            a: 1.0,
        };
        self.sky_rgb = None;
    }

    pub fn set_screen_blur(&mut self, radius: u8) {
        self.screen_blur_radius = radius.min(10);
    }

    pub fn set_fov(&mut self, fov: u16) {
        self.base_fov = f32::from(fov.clamp(30, 110));
    }

    pub fn set_fov_modifier(&mut self, modifier: f32) {
        self.fov_modifier = modifier;
    }

    pub fn set_view_yaw(&mut self, yaw: f32) {
        self.view_yaw = yaw;
    }

    pub fn set_view_bobbing(&mut self, enabled: bool) {
        self.view_bobbing = enabled;
    }

    pub fn set_camera_bob(&mut self, camera: &native_session::camera::CameraMotion, partial: f32) {
        self.bob_transform = camera.bob_transform(partial);
    }

    fn effective_bob(&self) -> Mat4 {
        if self.view_bobbing {
            self.bob_transform
        } else {
            Mat4::IDENTITY
        }
    }

    #[cfg(test)]
    pub fn configured_fov(&self) -> f32 {
        self.base_fov
    }
    #[cfg(test)]
    pub fn configured_view_bobbing(&self) -> bool {
        self.view_bobbing
    }
    #[cfg(test)]
    pub fn configured_present_mode(&self) -> wgpu::PresentMode {
        self.config.present_mode
    }

    #[cfg(test)]
    pub fn configured_color_format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    #[cfg(test)]
    pub fn preview_upload_counts(&self) -> (usize, usize, usize) {
        (
            self.preview_uploads,
            self.translucent_uploads,
            self.bundle_rebuilds,
        )
    }

    #[cfg(test)]
    pub fn translucent_upload_bytes(&self) -> &[u8] {
        &self.translucent_upload_bytes
    }

    #[cfg(test)]
    pub fn preview_chunk_count(&self) -> usize {
        self.preview_chunks.len()
    }

    #[cfg(test)]
    pub fn force_translucent_resort(&mut self) {
        self.translucent_dirty = true;
    }

    #[cfg(all(test, target_os = "windows"))]
    pub fn preview_readback(
        &mut self,
        scene: &PreviewScene,
        eye: Vec3,
        direction: Vec3,
        chunked: bool,
    ) -> Result<Vec<u8>> {
        use std::{sync::mpsc, time::Duration};
        let (width, height) = (320u32, 240u32);
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("terrain comparison color"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let depth = depth_view(&self.device, width, height);
        let projection = perspective(
            self.base_fov * self.fov_modifier,
            width as f32 / height as f32,
        );
        let camera = CameraUniform {
            view_projection: (projection
                * self.effective_bob()
                * Mat4::look_to_rh(eye, direction, camera_up(direction, self.view_yaw)))
            .to_cols_array_2d(),
            viewport: [width as f32, height as f32],
            padding: [0.0; 2],
        };
        self.queue
            .write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&camera));
        self.update_fog(eye);
        let vertices: Vec<_> = scene
            .chunks
            .values()
            .flat_map(|mesh| mesh.opaque.iter().copied())
            .collect();
        let flat = (!vertices.is_empty()).then(|| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("terrain comparison flat buffer"),
                    contents: bytemuck::cast_slice(&vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                })
        });
        let row = width * 4;
        let stride =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terrain comparison readback"),
            size: u64::from(stride) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terrain comparison pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.sky_clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth,
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
            pass.set_pipeline(&self.terrain_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_bind_group(1, &self.lightmap_bind_group, &[]);
            pass.set_bind_group(2, &self.fog_bind_group, &[]);
            if chunked {
                if let Some(bundle) = &self.preview_bundle {
                    pass.execute_bundles(std::iter::once(bundle));
                }
                pass.set_pipeline(&self.terrain_pipeline);
                pass.set_bind_group(1, &self.lightmap_bind_group, &[]);
                pass.set_bind_group(2, &self.fog_bind_group, &[]);
            } else if let Some(buffer) = &flat {
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..u32::try_from(vertices.len())?, 0..1);
            }
            if let Some(items) = &self.item_renderer {
                items.draw(&mut pass);
            }
            if let Some(items) = &self.ground_items {
                items.draw(&mut pass);
            }
            if self.translucent_count > 0
                && let Some(buffer) = &self.translucent_buffer
            {
                pass.set_pipeline(&self.translucent_pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_bind_group(1, &self.lightmap_bind_group, &[]);
                pass.set_bind_group(2, &self.fog_bind_group, &[]);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..self.translucent_count, 0..1);
            }
        }
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
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(height),
                },
            },
            size,
        );
        self.queue.submit([encoder.finish()]);
        let (sender, receiver) = mpsc::channel();
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(15)),
        })?;
        receiver.recv_timeout(Duration::from_secs(15))??;
        let data = readback.slice(..).get_mapped_range()?;
        let mut pixels = Vec::with_capacity((row * height) as usize);
        for y in 0..height as usize {
            let offset = y * stride as usize;
            pixels.extend_from_slice(&data[offset..offset + row as usize]);
        }
        drop(data);
        readback.unmap();
        Ok(pixels)
    }

    pub fn new(
        window: Arc<Window>,
        atlas: &TextureAtlas,
        breaking_textures: Option<&[RgbaImage]>,
        item_textures: Option<Vec<&RgbaImage>>,
        orb_texture: Option<&RgbaImage>,
        glint_texture: Option<(&RgbaImage, bool, bool)>,
        vsync: bool,
    ) -> Result<Self> {
        let mut errors = Vec::new();
        for (name, backend) in [
            ("DirectX 12", wgpu::Backends::DX12),
            ("Vulkan", wgpu::Backends::VULKAN),
        ] {
            match pollster::block_on(Self::with_backend(
                Arc::clone(&window),
                atlas,
                breaking_textures,
                item_textures.as_deref(),
                orb_texture,
                glint_texture,
                vsync,
                name,
                backend,
            )) {
                Ok(renderer) => return Ok(renderer),
                Err(error) => {
                    eprintln!("{name} initialization failed: {error:#}");
                    errors.push(format!("{name}: {error:#}"));
                }
            }
        }
        bail!(
            "could not initialize a graphics backend:\n{}",
            errors.join("\n")
        )
    }

    #[allow(clippy::too_many_arguments)]
    async fn with_backend(
        window: Arc<Window>,
        atlas: &TextureAtlas,
        breaking_textures: Option<&[RgbaImage]>,
        item_textures: Option<&[&RgbaImage]>,
        orb_texture: Option<&RgbaImage>,
        glint_texture: Option<(&RgbaImage, bool, bool)>,
        vsync: bool,
        name: &str,
        backend: wgpu::Backends,
    ) -> Result<Self> {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = backend;
        let instance = wgpu::Instance::new(descriptor);
        let surface = instance
            .create_surface(Arc::clone(&window))
            .context("cannot create window surface")?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .context("no compatible GPU adapter")?;
        #[cfg(debug_assertions)]
        let info = adapter.get_info();
        #[cfg(not(debug_assertions))]
        let _ = name;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("Minecraft Native device"),
                ..Default::default()
            })
            .await
            .context("cannot create GPU device")?;
        ensure!(
            atlas.width <= device.limits().max_texture_dimension_2d
                && atlas.height <= device.limits().max_texture_dimension_2d,
            "texture atlas {}x{} exceeds GPU limit {}",
            atlas.width,
            atlas.height,
            device.limits().max_texture_dimension_2d
        );
        let lost_device = Arc::new(AtomicBool::new(false));
        let lost_flag = Arc::clone(&lost_device);
        device.set_device_lost_callback(move |reason, message| {
            eprintln!("GPU device lost ({reason:?}): {message}");
            lost_flag.store(true, Ordering::Relaxed);
        });

        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or_else(|| anyhow!("surface has no compatible configuration"))?;
        let caps = surface.get_capabilities(&adapter);
        if let Some(srgb) = caps
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
        {
            config.format = srgb;
        }
        config.present_mode = crate::menu_settings::present_mode(vsync);
        surface.configure(&device, &config);
        let depth = depth_view(&device, config.width, config.height);

        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera uniform"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let texture_size = wgpu::Extent3d {
            width: atlas.width,
            height: atlas.height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("demo texture atlas"),
            size: texture_size,
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
            &atlas.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(atlas.width * 4),
                rows_per_image: Some(atlas.height),
            },
            texture_size,
        );
        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("nearest block sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain bindings"),
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
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain bind group"),
            layout: &bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain pipeline layout"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let lightmap_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("terrain lightmap"),
            contents: bytemuck::cast_slice(&[[1.0f32; 4]; 256]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let lightmap_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain lightmap layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let lightmap_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain lightmap binding"),
            layout: &lightmap_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: lightmap_buffer.as_entire_binding(),
            }],
        });
        let fog_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terrain fog uniform"),
            size: std::mem::size_of::<FogUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let fog_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain fog layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let fog_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain fog binding"),
            layout: &fog_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: fog_buffer.as_entire_binding(),
            }],
        });
        let terrain_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain lightmap pipeline layout"),
            bind_group_layouts: &[
                Some(&bind_layout),
                Some(&lightmap_layout),
                Some(&fog_layout),
            ],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terrain shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let make_terrain_pipeline = |translucent: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(if translucent {
                    "translucent terrain pipeline"
                } else {
                    "terrain pipeline"
                }),
                layout: Some(&terrain_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("terrain_vertex"),
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
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(!translucent),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("terrain_fragment"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: config.format,
                        blend: translucent.then_some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let terrain_pipeline = make_terrain_pipeline(false);
        let translucent_pipeline = make_terrain_pipeline(true);
        let breaking_renderer = breaking_textures
            .map(|textures| {
                BreakingRenderer::new(
                    &device,
                    &queue,
                    &bind_layout,
                    &pipeline_layout,
                    &camera_buffer,
                    &sampler,
                    &shader,
                    config.format,
                    textures,
                )
            })
            .transpose()
            .context("cannot initialize block-breaking overlay")?;
        let item_renderer = item_textures
            .map(|textures| {
                ItemRenderer::new(
                    &device,
                    &queue,
                    &bind_layout,
                    &camera_buffer,
                    &sampler,
                    textures,
                )
            })
            .transpose()
            .context("cannot initialize dropped item renderer")?;
        let orb_renderer = orb_texture
            .map(|texture| {
                OrbRenderer::new(
                    &device,
                    &queue,
                    &bind_layout,
                    &pipeline_layout,
                    &camera_buffer,
                    &sampler,
                    &shader,
                    config.format,
                    texture,
                )
            })
            .transpose()
            .context("cannot initialize experience orb renderer")?;
        let crosshair_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("crosshair pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("crosshair_vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("crosshair_fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let overlay_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("GUI overlay bindings"),
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
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let overlay_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("GUI nearest sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let fallback = RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 0]));
        let (glint_image, blur, clamp) = glint_texture.unwrap_or((&fallback, false, false));
        ensure!(
            glint_image.width() > 0
                && glint_image.height() > 0
                && glint_image.width() <= device.limits().max_texture_dimension_2d
                && glint_image.height() <= device.limits().max_texture_dimension_2d,
            "enchanted item glint texture size is unsupported"
        );
        let glint = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("enchanted item glint texture"),
            size: wgpu::Extent3d {
                width: glint_image.width(),
                height: glint_image.height(),
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
                texture: &glint,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            glint_image.as_raw(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(glint_image.width() * 4),
                rows_per_image: Some(glint_image.height()),
            },
            wgpu::Extent3d {
                width: glint_image.width(),
                height: glint_image.height(),
                depth_or_array_layers: 1,
            },
        );
        let overlay_glint_view = glint.create_view(&wgpu::TextureViewDescriptor::default());
        let overlay_glint_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("enchanted item glint sampler"),
            address_mode_u: if clamp {
                wgpu::AddressMode::ClampToEdge
            } else {
                wgpu::AddressMode::Repeat
            },
            address_mode_v: if clamp {
                wgpu::AddressMode::ClampToEdge
            } else {
                wgpu::AddressMode::Repeat
            },
            mag_filter: if blur {
                wgpu::FilterMode::Linear
            } else {
                wgpu::FilterMode::Nearest
            },
            min_filter: if blur {
                wgpu::FilterMode::Linear
            } else {
                wgpu::FilterMode::Nearest
            },
            ..Default::default()
        });
        let overlay_glint_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("enchanted item glint animation"),
            contents: bytemuck::bytes_of(&glint_uniform(0, 0.5, 0.75, false)),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let hand_glint_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("first-person glint animation"),
            contents: bytemuck::bytes_of(&glint_uniform(0, 0.5, 0.75, false)),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let overlay_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("GUI overlay shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("game_overlay.wgsl").into()),
        });
        let overlay_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("GUI overlay layout"),
                bind_group_layouts: &[Some(&overlay_layout)],
                immediate_size: 0,
            });
        let overlay_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("GUI overlay pipeline"),
            layout: Some(&overlay_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &overlay_shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &overlay_shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let first_person = FirstPersonRenderer::new(
            &device,
            config.format,
            Some((
                &overlay_glint_view,
                &overlay_glint_sampler,
                &hand_glint_uniform,
            )),
        );
        let ground_items = item_textures.map(|_| {
            GroundItemRenderer::new(
                &device,
                config.format,
                &bind_layout,
                &camera_buffer,
                (
                    &overlay_glint_view,
                    &overlay_glint_sampler,
                    &hand_glint_uniform,
                ),
            )
        });
        let screen_blur = crate::screen_blur::ScreenBlur::new(&device, config.format);
        let consume_renderer = ParticleRenderer::new(&bind_layout, &sampler, &camera_buffer);
        Ok(Self {
            instance,
            surface,
            window,
            device,
            queue,
            config,
            base_fov: 70.0,
            fov_modifier: 1.0,
            view_yaw: 0.0,
            view_bobbing: true,
            bob_transform: Mat4::IDENTITY,
            depth,
            sky_clear: wgpu::Color {
                r: 0.35,
                g: 0.58,
                b: 0.85,
                a: 1.0,
            },
            sky_rgb: None,
            fog_range: None,
            fog_buffer,
            fog_bind_group,
            fog_last: None,
            camera_buffer,
            lightmap_buffer,
            lightmap_bind_group,
            lightmap_last: None,
            atlas_texture: texture,
            atlas_size: (atlas.width, atlas.height),
            atlas_bind_layout: bind_layout,
            atlas_sampler: sampler,
            bind_group,
            terrain_pipeline,
            translucent_pipeline,
            breaking_renderer,
            item_renderer,
            ground_items,
            items_prepared: false,
            orb_renderer,
            consume_renderer,
            consume_draws: Vec::new(),
            consume_origin: glam::IVec3::ZERO,
            consume_partial: 0.0,
            first_person,
            crosshair_pipeline,
            overlay_pipeline,
            overlay_layout,
            overlay_sampler,
            overlay_texture: None,
            overlay_glint_mask: None,
            overlay_glint_view,
            overlay_glint_sampler,
            overlay_glint_uniform,
            overlay_glint_last: glint_uniform(0, 0.5, 0.75, false),
            hand_glint_uniform,
            hand_glint_last: glint_uniform(0, 0.5, 0.75, false),
            overlay_has_glint: false,
            pre_blur_overlay: PreBlurOverlay::default(),
            overlay_glint_started: Instant::now(),
            overlay_glint_available: glint_texture.is_some(),
            overlay_bind_group: None,
            overlay_size: (0, 0),
            hide_crosshair: false,
            screen_blur: Some(screen_blur),
            screen_blur_radius: 0,
            vertex_buffer: None,
            vertex_count: 0,
            translucent_buffer: None,
            translucent_capacity: 0,
            translucent_count: 0,
            mesh_revision: None,
            preview_uploaded: false,
            preview_mode: false,
            preview_chunks: BTreeMap::new(),
            chest_visuals: Arc::from([]),
            chest_partial: 0.0,
            animated_chest_chunks: BTreeSet::new(),
            preview_bundle: None,
            preview_bundle_dirty: true,
            translucent_eye: None,
            translucent_centers: Vec::new(),
            translucent_dirty: true,
            #[cfg(test)]
            preview_uploads: 0,
            #[cfg(test)]
            translucent_uploads: 0,
            #[cfg(test)]
            translucent_upload_bytes: Vec::new(),
            #[cfg(test)]
            bundle_rebuilds: 0,
            lost_device,
            #[cfg(debug_assertions)]
            backend_name: format!("{name}: {}", info.name),
        })
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
        self.depth = depth_view(&self.device, size.width, size.height);
    }

    pub fn set_vsync(&mut self, enabled: bool) {
        let mode = crate::menu_settings::present_mode(enabled);
        if self.config.present_mode != mode && !self.lost_device.load(Ordering::Relaxed) {
            self.config.present_mode = mode;
            self.surface.configure(&self.device, &self.config);
        }
    }

    pub fn update_first_person(&mut self, input: FirstPersonInput) {
        self.first_person.update(&self.device, &self.queue, input);
    }

    pub(crate) fn set_consume_particles(
        &mut self,
        particles: Vec<RenderParticle>,
        origin: glam::IVec3,
        partial: f32,
    ) {
        self.consume_draws = particles;
        self.consume_origin = origin;
        self.consume_partial = partial;
    }

    pub fn update_dropped_items(
        &mut self,
        items: &[native_session::items::DroppedItem],
        models: &[Option<Arc<HeldModel>>],
        falling: &[native_session::falling_block::FallingBlockVisual],
        falling_models: &[Option<Arc<HeldModel>>],
        origin: glam::IVec3,
        partial: f32,
    ) -> Result<()> {
        if let Some(ground) = &mut self.ground_items {
            ground.update_with_falling(
                &self.device,
                &self.queue,
                items,
                models,
                falling,
                falling_models,
                origin,
                partial,
            )?;
        }
        if let Some(fallback) = &mut self.item_renderer {
            fallback.update_visible(&self.device, &self.queue, items, origin, Some(models))?;
        }
        self.items_prepared = true;
        Ok(())
    }

    fn prepare_fallback_items(
        &mut self,
        items: &[native_session::items::DroppedItem],
        origin: glam::IVec3,
    ) -> Result<()> {
        if !self.items_prepared {
            if let Some(fallback) = &mut self.item_renderer {
                fallback.update(&self.device, &self.queue, items, origin)?;
            }
            if let Some(ground) = &mut self.ground_items {
                ground.clear();
            }
        }
        self.items_prepared = false;
        Ok(())
    }

    pub fn update_overlay(&mut self, canvas: &Canvas, hide_crosshair: bool) -> Result<()> {
        let had_glint = self.overlay_has_glint;
        let size = (canvas.width as u32, canvas.height as u32);
        ensure!(
            size.0 > 0
                && size.1 > 0
                && size.0 <= self.device.limits().max_texture_dimension_2d
                && size.1 <= self.device.limits().max_texture_dimension_2d,
            "GUI overlay size is unsupported"
        );
        if self.overlay_size != size {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("GUI overlay texture"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let mask = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("GUI enchanted item mask"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Uint,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let mask_view = mask.create_view(&wgpu::TextureViewDescriptor::default());
            self.overlay_bind_group =
                Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("GUI overlay bind group"),
                    layout: &self.overlay_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.overlay_sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(&mask_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::TextureView(&self.overlay_glint_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::Sampler(&self.overlay_glint_sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 5,
                            resource: self.overlay_glint_uniform.as_entire_binding(),
                        },
                    ],
                }));
            self.overlay_texture = Some(texture);
            self.overlay_glint_mask = Some(mask);
            self.overlay_size = size;
        }
        let mut rgba = Vec::with_capacity(canvas.pixels.len() * 4);
        for &pixel in &canvas.pixels {
            rgba.extend_from_slice(&[
                (pixel >> 16) as u8,
                (pixel >> 8) as u8,
                pixel as u8,
                (pixel >> 24) as u8,
            ]);
        }
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: self
                    .overlay_texture
                    .as_ref()
                    .context("GUI overlay texture missing")?,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size.0 * 4),
                rows_per_image: Some(size.1),
            },
            wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
        );
        self.overlay_has_glint = self.overlay_glint_available
            && canvas
                .glint
                .as_ref()
                .is_some_and(|mask| mask.as_chunks::<4>().0.iter().any(|pixel| pixel[3] != 0));
        if let Some(glint) = &canvas.glint
            && self.overlay_has_glint
        {
            ensure!(
                glint.len() == canvas.pixels.len() * 4,
                "invalid GUI glint mask"
            );
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: self
                        .overlay_glint_mask
                        .as_ref()
                        .context("GUI glint mask missing")?,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(glint),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size.0 * 8),
                    rows_per_image: Some(size.1),
                },
                wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
            );
        } else if had_glint {
            let clear = vec![0_u8; canvas.pixels.len() * 8];
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: self
                        .overlay_glint_mask
                        .as_ref()
                        .context("GUI glint mask missing")?,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &clear,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size.0 * 8),
                    rows_per_image: Some(size.1),
                },
                wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
            );
        }
        self.hide_crosshair = hide_crosshair;
        Ok(())
    }

    /// Reuse the overlay upload path for the HUD that precedes screen blur.
    pub fn update_pre_blur_overlay(&mut self, canvas: &Canvas) -> Result<()> {
        let old_hide_crosshair = self.hide_crosshair;
        let mut previous = std::mem::take(&mut self.pre_blur_overlay);
        std::mem::swap(&mut self.overlay_texture, &mut previous.texture);
        std::mem::swap(&mut self.overlay_glint_mask, &mut previous.mask);
        std::mem::swap(&mut self.overlay_bind_group, &mut previous.binding);
        std::mem::swap(&mut self.overlay_size, &mut previous.size);
        std::mem::swap(&mut self.overlay_has_glint, &mut previous.has_glint);
        let result = self.update_overlay(canvas, old_hide_crosshair);
        std::mem::swap(&mut self.overlay_texture, &mut previous.texture);
        std::mem::swap(&mut self.overlay_glint_mask, &mut previous.mask);
        std::mem::swap(&mut self.overlay_bind_group, &mut previous.binding);
        std::mem::swap(&mut self.overlay_size, &mut previous.size);
        std::mem::swap(&mut self.overlay_has_glint, &mut previous.has_glint);
        self.hide_crosshair = old_hide_crosshair;
        self.pre_blur_overlay = previous;
        result
    }

    pub fn update_gui_glint(&mut self, speed: f64, strength: f32) {
        let overlay_active = self.overlay_has_glint
            || (self.screen_blur_radius > 0 && self.pre_blur_overlay.has_glint);
        let hand_active = self.overlay_glint_available
            && (self.first_person.has_glint()
                || self
                    .ground_items
                    .as_ref()
                    .is_some_and(GroundItemRenderer::has_glint));
        let elapsed = if overlay_active || hand_active {
            self.overlay_glint_started
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64
        } else {
            0
        };
        let (next, hand) = glint_uniforms(elapsed, speed, strength, overlay_active, hand_active);
        if self.overlay_glint_last != next {
            self.queue
                .write_buffer(&self.overlay_glint_uniform, 0, bytemuck::bytes_of(&next));
            self.overlay_glint_last = next;
        }
        if self.hand_glint_last != hand {
            self.queue
                .write_buffer(&self.hand_glint_uniform, 0, bytemuck::bytes_of(&hand));
            self.hand_glint_last = hand;
        }
    }

    pub fn render(
        &mut self,
        snapshot: &WorldSnapshot,
        partial_tick: f32,
        direction: Vec3,
    ) -> Result<()> {
        self.set_fov_modifier(snapshot.camera.fov_modifier(partial_tick));
        self.set_camera_bob(&snapshot.camera, partial_tick);
        self.preview_mode = false;
        self.translucent_count = 0;
        let world = snapshot
            .world
            .demo()
            .context("demo renderer received a saved-world snapshot")?;
        if self.mesh_revision != Some(world.revision()) {
            self.upload_opaque(&mesh::build(world))?;
            self.mesh_revision = Some(world.revision());
        }
        if let Some(breaking) = &mut self.breaking_renderer {
            breaking.update(&self.queue, snapshot.breaking, snapshot.origin)?;
        }
        self.prepare_fallback_items(&snapshot.dropped_items, snapshot.origin)?;
        self.update_experience_orbs(
            &snapshot.experience_orbs,
            snapshot.origin,
            direction,
            partial_tick,
        )?;
        self.render_frame(snapshot.camera.eye(partial_tick).as_vec3(), direction)
    }

    pub fn update_experience_orbs(
        &mut self,
        orbs: &[native_session::orbs::ExperienceOrb],
        origin: glam::IVec3,
        direction: Vec3,
        partial: f32,
    ) -> Result<()> {
        if let Some(renderer) = &mut self.orb_renderer {
            renderer.update(
                &self.device,
                &self.queue,
                orbs,
                origin,
                direction,
                camera_up(direction, self.view_yaw),
                partial,
            )?;
        }
        Ok(())
    }

    pub fn render_preview(
        &mut self,
        scene: &mut PreviewScene,
        eye: Vec3,
        direction: Vec3,
        tick: u64,
        progress: Option<BreakingProgress>,
        dropped_items: &[native_session::items::DroppedItem],
    ) -> Result<()> {
        if !self.preview_mode {
            self.preview_mode = true;
            self.translucent_dirty = true;
        }
        if !self.preview_uploaded {
            self.set_preview_lightmap(&scene.lightmap);
            self.sync_preview_chunks(scene)?;
            self.preview_uploaded = true;
        }
        self.update_chest_lids()?;
        if scene.atlas.needs_update(tick) {
            Arc::make_mut(&mut scene.atlas).update(tick);
            let atlas = &scene.atlas.texture;
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.atlas_texture,
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
        if self.translucent_dirty || self.translucent_eye != Some(eye) {
            // If every adjacent distance remains strictly ordered, the old
            // buffer is exactly the same order a fresh sort would produce.
            // Equal distances fall back to sorting, preserving its tie order.
            let unchanged =
                !self.translucent_dirty && strictly_far_to_near(&self.translucent_centers, eye);
            if !unchanged {
                let quad_count = scene
                    .chunks
                    .values()
                    .try_fold(0usize, |count, mesh| {
                        count.checked_add(mesh.translucent.len())
                    })
                    .context("translucent quad count overflow")?;
                let limit =
                    usize::try_from(self.device.limits().max_buffer_size).unwrap_or(usize::MAX);
                let (vertex_count, _) = translucent_buffer_size(quad_count, limit)?;
                let quads = scene.sorted_translucent_quads(eye);
                self.translucent_centers = quads.iter().map(|quad| quad.center).collect();
                let vertices: Vec<_> = quads.iter().flat_map(|quad| quad.vertices).collect();
                self.translucent_count = vertex_count;
                let contents = bytemuck::cast_slice(&vertices);
                if !contents.is_empty() {
                    if self.translucent_capacity < contents.len() {
                        self.translucent_capacity = contents
                            .len()
                            .checked_next_power_of_two()
                            .unwrap_or(contents.len())
                            .min(limit);
                        self.translucent_buffer =
                            Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                                label: Some("translucent terrain vertices"),
                                size: self.translucent_capacity as u64,
                                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                                mapped_at_creation: false,
                            }));
                    }
                    self.queue.write_buffer(
                        self.translucent_buffer
                            .as_ref()
                            .context("missing translucent buffer")?,
                        0,
                        contents,
                    );
                    #[cfg(test)]
                    {
                        self.translucent_uploads += 1;
                        self.translucent_upload_bytes = contents.to_vec();
                    }
                }
            }
            self.translucent_eye = Some(eye);
            self.translucent_dirty = false;
        }
        if let Some(breaking) = &mut self.breaking_renderer {
            breaking.update(&self.queue, progress, scene.origin)?;
        }
        self.prepare_fallback_items(dropped_items, scene.origin)?;
        self.render_frame(eye, direction)
    }

    pub fn set_chest_lids(
        &mut self,
        lids: Arc<[native_session::chest_lid::ChestLidVisual]>,
        partial: f32,
    ) {
        self.chest_visuals = lids;
        self.chest_partial = partial;
    }

    fn update_chest_lids(&mut self) -> Result<()> {
        let mut targets: BTreeMap<ChunkPos, BTreeMap<(i32, i32, i32), f32>> = BTreeMap::new();
        for lid in self.chest_visuals.iter().copied() {
            let angle = lid.angle(self.chest_partial);
            if angle == 0.0 {
                continue;
            }
            let p = lid.position;
            targets
                .entry(ChunkPos {
                    x: p.x.div_euclid(16),
                    z: p.z.div_euclid(16),
                })
                .or_default()
                .insert((p.x, p.y, p.z), angle);
        }
        let affected: BTreeSet<_> = self
            .animated_chest_chunks
            .iter()
            .copied()
            .chain(targets.keys().copied())
            .collect();
        self.animated_chest_chunks.clear();
        for position in affected {
            let Some(chunk) = self.preview_chunks.get_mut(&position) else {
                continue;
            };
            let target = targets.get(&position);
            let keys: BTreeSet<_> = chunk
                .animated_lids
                .keys()
                .copied()
                .chain(target.into_iter().flat_map(|values| values.keys().copied()))
                .collect();
            for key in keys {
                let angle = target
                    .and_then(|values| values.get(&key))
                    .copied()
                    .unwrap_or(0.0);
                if chunk.animated_lids.get(&key).copied() == Some(angle.to_bits()) {
                    continue;
                }
                let Some(lid) = chunk
                    .chest_lids
                    .iter()
                    .find(|lid| (lid.position.x, lid.position.y, lid.position.z) == key)
                else {
                    chunk.animated_lids.remove(&key);
                    continue;
                };
                let Some(buffer) = &chunk.buffer else {
                    continue;
                };
                let vertices = lid.animated_vertices(&chunk.opaque, angle);
                let offset = lid
                    .start
                    .checked_mul(std::mem::size_of::<Vertex>())
                    .context("chest vertex offset overflow")?;
                self.queue
                    .write_buffer(buffer, offset as u64, bytemuck::cast_slice(&vertices));
                if angle == 0.0 {
                    chunk.animated_lids.remove(&key);
                } else {
                    chunk.animated_lids.insert(key, angle.to_bits());
                }
            }
            if !chunk.animated_lids.is_empty() {
                self.animated_chest_chunks.insert(position);
            }
        }
        Ok(())
    }

    fn sync_preview_chunks(&mut self, scene: &PreviewScene) -> Result<()> {
        let mut transparent_changed = false;
        let mut draw_changed = false;
        self.preview_chunks.retain(|position, chunk| {
            let keep = scene.chunks.contains_key(position);
            if !keep && !chunk.translucent.is_empty() {
                transparent_changed = true;
            }
            if !keep && chunk.count > 0 {
                draw_changed = true;
            }
            keep
        });
        for (position, mesh) in &scene.chunks {
            let chunk = self.preview_chunks.entry(*position).or_default();
            let (uploaded, changed, changed_draw) =
                chunk.update(&self.device, &self.queue, mesh)?;
            transparent_changed |= changed;
            draw_changed |= changed_draw;
            #[cfg(test)]
            if uploaded {
                self.preview_uploads += 1;
            }
            #[cfg(not(test))]
            let _ = uploaded;
        }
        self.translucent_dirty |= transparent_changed;
        self.preview_bundle_dirty |= draw_changed;
        if self.preview_bundle_dirty {
            self.rebuild_preview_bundle();
        }
        Ok(())
    }

    fn rebuild_preview_bundle(&mut self) {
        let colors = [Some(self.config.format)];
        let mut encoder =
            self.device
                .create_render_bundle_encoder(&wgpu::RenderBundleEncoderDescriptor {
                    label: Some("cached chunk terrain draws"),
                    color_formats: &colors,
                    depth_stencil: Some(wgpu::RenderBundleDepthStencil {
                        format: DEPTH_FORMAT,
                        depth_read_only: false,
                        stencil_read_only: true,
                    }),
                    sample_count: 1,
                    multiview: None,
                });
        encoder.set_pipeline(&self.terrain_pipeline);
        encoder.set_bind_group(0, &self.bind_group, &[]);
        encoder.set_bind_group(1, &self.lightmap_bind_group, &[]);
        encoder.set_bind_group(2, &self.fog_bind_group, &[]);
        for chunk in self.preview_chunks.values() {
            if chunk.count > 0
                && let Some(buffer) = &chunk.buffer
            {
                encoder.set_vertex_buffer(0, buffer.slice(..));
                encoder.draw(0..chunk.count, 0..1);
            }
        }
        self.preview_bundle = Some(encoder.finish(&wgpu::RenderBundleDescriptor {
            label: Some("cached chunk terrain draws"),
        }));
        self.preview_bundle_dirty = false;
        #[cfg(test)]
        {
            self.bundle_rebuilds += 1;
        }
    }

    /// Update lighting without rebuilding or re-uploading terrain geometry.
    pub fn set_preview_lightmap(&mut self, table: &[[[f32; 3]; 16]; 16]) {
        let lightmap = std::array::from_fn::<_, 256, _>(|index| {
            let rgb = table[index / 16][index % 16];
            [rgb[0], rgb[1], rgb[2], 1.0]
        });
        if self.lightmap_last.as_ref() != Some(&lightmap) {
            self.queue
                .write_buffer(&self.lightmap_buffer, 0, bytemuck::cast_slice(&lightmap));
            self.lightmap_last = Some(lightmap);
        }
    }

    /// Replace a rebuilt preview mesh while retaining the existing GPU device.
    pub fn replace_preview(&mut self, scene: &PreviewScene) -> Result<()> {
        let atlas = &scene.atlas.texture;
        let new_size = (atlas.width, atlas.height);
        if new_size != self.atlas_size {
            let limit = self.device.limits().max_texture_dimension_2d;
            ensure!(
                atlas.width > 0
                    && atlas.height > 0
                    && atlas.width <= limit
                    && atlas.height <= limit,
                "rebuilt world atlas {}x{} exceeds GPU limits",
                atlas.width,
                atlas.height
            );
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("world texture atlas"),
                size: wgpu::Extent3d {
                    width: atlas.width,
                    height: atlas.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("terrain bind group"),
                layout: &self.atlas_bind_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.camera_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.atlas_sampler),
                    },
                ],
            });
            println!(
                "World texture atlas resized: {}x{} -> {}x{}",
                self.atlas_size.0, self.atlas_size.1, new_size.0, new_size.1
            );
            self.atlas_texture = texture;
            self.atlas_size = new_size;
            self.bind_group = bind_group;
            self.preview_bundle_dirty = true;
        }
        self.set_preview_lightmap(&scene.lightmap);
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.atlas_texture,
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
        self.preview_uploaded = false;
        Ok(())
    }

    /// Reupload geometry while preserving the current animated atlas.
    pub fn replace_preview_mesh(&mut self) {
        self.preview_uploaded = false;
    }

    fn upload_opaque(&mut self, vertices: &[Vertex]) -> Result<()> {
        self.vertex_count = u32::try_from(vertices.len()).context("too many terrain vertices")?;
        self.vertex_buffer = if vertices.is_empty() {
            None
        } else {
            Some(
                self.device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("terrain vertices"),
                        contents: bytemuck::cast_slice(vertices),
                        usage: wgpu::BufferUsages::VERTEX,
                    }),
            )
        };
        Ok(())
    }

    fn render_frame(&mut self, eye: Vec3, direction: Vec3) -> Result<()> {
        if self.lost_device.load(Ordering::Relaxed) {
            bail!("graphics device was lost; restart the application");
        }
        if self.window.inner_size().width == 0 || self.window.inner_size().height == 0 {
            return Ok(());
        }
        let (frame, suboptimal) = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => (frame, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => (frame, true),
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface = self
                    .instance
                    .create_surface(Arc::clone(&self.window))
                    .context("cannot recreate lost window surface")?;
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => bail!("graphics surface validation failed"),
        };
        let aspect = self.config.width as f32 / self.config.height as f32;
        let projection = perspective(self.base_fov * self.fov_modifier, aspect);
        // Java uses its separate 70-degree HUD FOV for the player's hands.
        let hand_projection = perspective(70.0, aspect);
        let view = Mat4::look_to_rh(eye, direction, camera_up(direction, self.view_yaw));
        let camera = CameraUniform {
            view_projection: (projection * self.effective_bob() * view).to_cols_array_2d(),
            viewport: [self.config.width as f32, self.config.height as f32],
            padding: [0.0; 2],
        };
        self.queue
            .write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&camera));
        self.consume_renderer.update(
            &self.device,
            &self.queue,
            &self.consume_draws,
            self.consume_origin,
            direction,
            camera_up(direction, self.view_yaw),
            self.consume_partial,
        )?;
        self.update_fog(eye);

        let output_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        if self.screen_blur_radius > 0 {
            let blur = self.screen_blur.get_or_insert_with(|| {
                crate::screen_blur::ScreenBlur::new(&self.device, self.config.format)
            });
            blur.prepare(
                &self.device,
                &self.queue,
                (self.config.width, self.config.height),
                self.screen_blur_radius,
            )?;
        }
        let view = if self.screen_blur_radius > 0 {
            self.screen_blur.as_ref().unwrap().view()
        } else {
            &output_view
        };
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terrain pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.sky_clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
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
            pass.set_pipeline(&self.terrain_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_bind_group(1, &self.lightmap_bind_group, &[]);
            pass.set_bind_group(2, &self.fog_bind_group, &[]);
            if self.preview_mode {
                if let Some(bundle) = &self.preview_bundle {
                    pass.execute_bundles(std::iter::once(bundle));
                }
                // Executing a bundle resets render-pass state; item rendering
                // shares the terrain pipeline and sets only its own bind group.
                pass.set_pipeline(&self.terrain_pipeline);
                pass.set_bind_group(1, &self.lightmap_bind_group, &[]);
                pass.set_bind_group(2, &self.fog_bind_group, &[]);
            } else if let Some(buffer) = &self.vertex_buffer {
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..self.vertex_count, 0..1);
            }
            if let Some(items) = &self.item_renderer {
                items.draw(&mut pass);
            }
            if let Some(items) = &self.ground_items {
                items.draw(&mut pass);
            }
            if let Some(breaking) = &self.breaking_renderer {
                breaking.draw(&mut pass);
            }
            self.consume_renderer.draw_layer(
                &mut pass,
                &self.terrain_pipeline,
                &self.lightmap_bind_group,
                &self.fog_bind_group,
                true,
            );
            if self.translucent_count > 0
                && let Some(buffer) = &self.translucent_buffer
            {
                pass.set_pipeline(&self.translucent_pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_bind_group(1, &self.lightmap_bind_group, &[]);
                pass.set_bind_group(2, &self.fog_bind_group, &[]);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..self.translucent_count, 0..1);
            }
            if let Some(orbs) = &self.orb_renderer {
                orbs.draw(&mut pass);
            }
            self.consume_renderer.draw_layer(
                &mut pass,
                &self.translucent_pipeline,
                &self.lightmap_bind_group,
                &self.fog_bind_group,
                false,
            );
        }
        self.first_person.draw(
            &self.queue,
            &mut encoder,
            view,
            &self.depth,
            hand_projection * self.effective_bob(),
        );
        if !self.hide_crosshair {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("crosshair pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
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
            pass.set_pipeline(&self.crosshair_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..12, 0..1);
        }
        if self.screen_blur_radius > 0 {
            if let Some(binding) = &self.pre_blur_overlay.binding {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("HUD before screen blur"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
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
                pass.set_pipeline(&self.overlay_pipeline);
                pass.set_bind_group(0, binding, &[]);
                pass.draw(0..6, 0..1);
            }
            self.screen_blur
                .as_ref()
                .unwrap()
                .render(&mut encoder, &output_view);
        }
        if let Some(bind_group) = &self.overlay_bind_group {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("GUI overlay pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &output_view,
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
            pass.set_pipeline(&self.overlay_pipeline);
            pass.set_bind_group(0, bind_group, &[]);
            pass.draw(0..6, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
        if suboptimal {
            self.surface.configure(&self.device, &self.config);
        }
        Ok(())
    }
}

/// At vertical pitch, world-up is parallel to the view direction. Use the
/// player's horizontal heading to retain a stable right/up basis instead.
pub(crate) fn camera_up(direction: Vec3, yaw: f32) -> Vec3 {
    if direction.x * direction.x + direction.z * direction.z < 1.0e-8 {
        Vec3::new(yaw.cos(), 0.0, yaw.sin())
            .cross(direction)
            .normalize_or_zero()
    } else {
        Vec3::Y
    }
}

fn perspective(fov: f32, aspect: f32) -> Mat4 {
    // A 32-chunk square extends beyond 512 blocks along the diagonal.
    Mat4::perspective_rh(fov.to_radians(), aspect, 0.05, 1024.0)
}

fn strictly_far_to_near(centers: &[Vec3], eye: Vec3) -> bool {
    centers.windows(2).all(|pair| {
        pair[0]
            .distance_squared(eye)
            .total_cmp(&pair[1].distance_squared(eye))
            == std::cmp::Ordering::Greater
    })
}

fn depth_view(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth buffer"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

#[cfg(test)]
mod tests {
    #[test]
    fn vertical_view_keeps_a_finite_yaw_aligned_camera_basis() {
        use glam::{Mat4, Vec3};
        for yaw in [0.0_f32, 0.37, 1.57, 3.14, -1.25] {
            for pitch in [-native_session::MAX_PITCH, native_session::MAX_PITCH] {
                let direction = Vec3::new(
                    yaw.sin() * pitch.cos(),
                    pitch.sin(),
                    -yaw.cos() * pitch.cos(),
                );
                let up = super::camera_up(direction, yaw);
                let view = Mat4::look_to_rh(Vec3::ZERO, direction, up);
                assert!(view.is_finite());
                let right = direction.cross(up).normalize();
                assert!(right.dot(Vec3::new(yaw.cos(), 0.0, yaw.sin())) > 0.99999);
                assert!(up.dot(direction).abs() < 1.0e-6);
            }
        }
        assert_eq!(super::camera_up(Vec3::NEG_Z, 0.0), Vec3::Y);
    }
    #[test]
    fn translucent_buffer_checks_gpu_limit_and_integer_overflow() {
        let quad_bytes = 6 * std::mem::size_of::<crate::mesh::Vertex>();
        assert_eq!(
            super::translucent_buffer_size(2, quad_bytes * 2).unwrap(),
            (12, quad_bytes * 2)
        );
        assert!(super::translucent_buffer_size(2, quad_bytes * 2 - 1).is_err());
        assert!(super::translucent_buffer_size(usize::MAX, usize::MAX).is_err());
    }
    use std::{sync::mpsc, time::Duration};

    #[test]
    fn hand_glint_does_not_enable_a_stale_gui_mask() {
        let (gui, hand) = super::glint_uniforms(12_345, 0.5, 0.75, false, true);
        assert_eq!(gui.active, 0.0);
        assert_eq!(hand.active, 1.0);
        assert_eq!(gui.offset, hand.offset);
        assert_eq!(gui.strength, hand.strength);
        let (gui, hand) = super::glint_uniforms(12_345, 0.5, 0.75, true, false);
        assert_eq!(gui.active, 1.0);
        assert_eq!(hand.active, 0.0);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and a Windows GPU window"]
    fn game_ui_loads_and_draws_enchanted_ground_model_without_a_gui_mask() {
        if !crate::window_test::isolated(
            "renderer::tests::game_ui_loads_and_draws_enchanted_ground_model_without_a_gui_mask",
        ) {
            return;
        }
        use native_session::GameSession;
        use std::{sync::Arc, time::Instant};
        use winit::{
            application::ApplicationHandler,
            event::WindowEvent,
            event_loop::{ActiveEventLoop, EventLoop},
            platform::windows::EventLoopBuilderExtWindows,
            window::{Window, WindowId},
        };
        struct Harness;
        impl ApplicationHandler for Harness {
            fn resumed(&mut self, event_loop: &ActiveEventLoop) {
                let game = native_resources::default_minecraft_dir().unwrap();
                let assets = Arc::new(crate::menu_assets::MenuAssets::load(&game, &[]).unwrap());
                let mut ui = crate::game_ui::GameUi::new(assets);
                let window = Arc::new(
                    event_loop
                        .create_window(
                            Window::default_attributes()
                                .with_visible(false)
                                .with_inner_size(winit::dpi::PhysicalSize::new(320, 240)),
                        )
                        .unwrap(),
                );
                let atlas = native_resources::TextureAtlas {
                    width: 1,
                    height: 1,
                    rgba: vec![255; 4],
                };
                let mut renderer = super::Renderer::new(
                    window,
                    &atlas,
                    None,
                    Some(ui.item_textures()),
                    None,
                    Some(ui.item_glint_texture()),
                    false,
                )
                .unwrap();
                let session = native_session::LocalSession::new(native_world::World::demo());
                let mut snapshot = (*session.snapshot()).clone();
                let stack = native_session::inventory::ItemStack::new(
                    "minecraft:diamond_sword".into(),
                    1,
                    1,
                    native_persistence::nbt::Tag::Compound(vec![(
                        "minecraft:enchantment_glint_override".into(),
                        native_persistence::nbt::Tag::Byte(1),
                    )]),
                )
                .unwrap();
                snapshot.dropped_items = Arc::from([native_session::items::DroppedItem {
                    entity_id: 0,
                    item_age: 0,
                    position: glam::DVec3::new(0.0, 2.0, 0.0),
                    previous_position: glam::DVec3::new(0.0, 2.0, 0.0),
                    stack,
                    age: 0,
                    bob_offset: 0.0,
                }]);
                snapshot.falling_blocks =
                    Arc::from([native_session::falling_block::FallingBlockVisual {
                        previous_position: glam::DVec3::new(0.5, 4.0, 0.5),
                        position: glam::DVec3::new(0.5, 3.5, 0.5),
                        kind: native_session::falling_block::FallingKind::Sand,
                    }]);
                let deadline = Instant::now() + Duration::from_secs(45);
                loop {
                    ui.draw(&mut renderer, (320, 240), &snapshot, snapshot.origin, 0.25)
                        .unwrap();
                    if renderer.ground_items.as_ref().is_some_and(|ground| {
                        ground.has_glint() && ground.resource_identities().len() == 2
                    }) {
                        break;
                    }
                    assert!(
                        Instant::now() < deadline,
                        "enchanted ground item model did not load"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                assert_eq!(renderer.hand_glint_last.active, 1.0);
                assert_eq!(renderer.overlay_glint_last.active, 0.0);
                renderer.render(&snapshot, 0.25, glam::Vec3::NEG_Z).unwrap();
                snapshot.dropped_items = Arc::from([]);
                snapshot.falling_blocks = Arc::from([]);
                ui.draw(&mut renderer, (320, 240), &snapshot, snapshot.origin, 0.25)
                    .unwrap();
                assert!(!renderer.ground_items.as_ref().unwrap().has_glint());
                assert_eq!(renderer.hand_glint_last.active, 0.0);
                event_loop.exit();
            }
            fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
        }
        let mut event_loop = EventLoop::builder();
        event_loop.with_any_thread(true);
        event_loop.build().unwrap().run_app(&mut Harness).unwrap();
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn item_glint_transform_matches_installed_java() {
        use std::{path::PathBuf, process::Command};

        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
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
        let classpath = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
            .join("bin");
        let source = temp.path().join("GlintTransformOracle.java");
        let output = temp.path().join("output.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/GlintTransformOracle.java"),
        )
        .unwrap();
        let run = |command: &mut Command| {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "Java glint transform oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&classpath)
            .arg(&source));
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&classpath)
            .arg("GlintTransformOracle")
            .arg(&output));
        let expected: Vec<[f32; 2]> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let times = [
            0,
            1,
            124,
            12_345,
            29_999,
            30_000,
            137_500,
            3_600_000,
            4_294_967_295,
        ];
        let speeds = [0.0, 0.1, 0.5, 0.75, 1.0];
        let points = [
            [0.0, 0.0],
            [0.11, 0.57],
            [0.9, 0.2],
            [1.0, 1.0],
            [1.25, -0.5],
        ];
        assert_eq!(expected.len(), times.len() * speeds.len() * points.len());
        let mut row = 0;
        for time in times {
            for speed in speeds {
                let uniform = super::glint_uniform(time, speed, 0.75, true);
                for [u, v] in points {
                    // Mirrors the standard 8x item-atlas path in first_person.wgsl.
                    let [u, v] = [u * 8.0, v * 8.0];
                    let actual = [
                        0.984_807_7 * u - 0.173_648_18 * v + uniform.offset[0],
                        0.173_648_18 * u + 0.984_807_7 * v + uniform.offset[1],
                    ];
                    for axis in 0..2 {
                        assert!(
                            (actual[axis] - expected[row][axis]).abs() < 0.000_002,
                            "time={time}, speed={speed}, point={:?}, axis={axis}: actual={} Java={}",
                            [u / 8.0, v / 8.0],
                            actual[axis],
                            expected[row][axis]
                        );
                    }
                    row += 1;
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires a Windows GPU window"]
    fn gpu_glint_animates_only_item_pixels() {
        if !crate::window_test::isolated("renderer::tests::gpu_glint_animates_only_item_pixels") {
            return;
        }
        use crate::menu_draw::{Canvas, Rect};
        use std::sync::Arc;
        use winit::{
            application::ApplicationHandler,
            event::WindowEvent,
            event_loop::{ActiveEventLoop, EventLoop},
            platform::windows::EventLoopBuilderExtWindows,
            window::{Window, WindowId},
        };

        fn render(
            renderer: &super::Renderer,
            uniform: super::GlintUniform,
            pre_blur: bool,
        ) -> Vec<u8> {
            let size = wgpu::Extent3d {
                width: 64,
                height: 16,
                depth_or_array_layers: 1,
            };
            renderer.queue.write_buffer(
                &renderer.overlay_glint_uniform,
                0,
                bytemuck::bytes_of(&uniform),
            );
            let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("glint test output"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: renderer.config.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = target.create_view(&wgpu::TextureViewDescriptor::default());
            let readback = renderer.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("glint test readback"),
                size: 64 * 16 * 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = renderer
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("glint test pass"),
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
                pass.set_pipeline(&renderer.overlay_pipeline);
                let binding = if pre_blur {
                    renderer.pre_blur_overlay.binding.as_ref()
                } else {
                    renderer.overlay_bind_group.as_ref()
                }
                .unwrap();
                pass.set_bind_group(0, binding, &[]);
                pass.draw(0..6, 0..1);
            }
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
                        rows_per_image: Some(16),
                    },
                },
                size,
            );
            renderer.queue.submit([encoder.finish()]);
            let (sender, receiver) = mpsc::channel();
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let _ = sender.send(result);
                });
            renderer
                .device
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
            let pixels = data.to_vec();
            drop(data);
            readback.unmap();
            pixels
        }
        struct Harness;
        impl ApplicationHandler for Harness {
            fn resumed(&mut self, event_loop: &ActiveEventLoop) {
                let window = Arc::new(
                    event_loop
                        .create_window(Window::default_attributes().with_visible(false))
                        .unwrap(),
                );
                let atlas = native_resources::TextureAtlas {
                    width: 1,
                    height: 1,
                    rgba: vec![255; 4],
                };
                let glint = image::RgbaImage::from_fn(128, 128, |x, _| {
                    image::Rgba([if x < 64 { 240 } else { 15 }, 40, 200, 255])
                });
                let mut renderer = super::Renderer::new(
                    window,
                    &atlas,
                    None,
                    None,
                    None,
                    Some((&glint, true, false)),
                    false,
                )
                .unwrap();
                let mut canvas = Canvas::new(64, 16);
                canvas.fill(
                    Rect {
                        x: 0,
                        y: 0,
                        w: 64,
                        h: 16,
                    },
                    [80, 80, 80, 255],
                );
                let icon = image::RgbaImage::from_pixel(16, 16, image::Rgba([80, 80, 80, 255]));
                let rect = Rect {
                    x: 0,
                    y: 0,
                    w: 16,
                    h: 16,
                };
                canvas.image(&icon, rect);
                canvas.mark_item_glint(&icon, rect);
                let atlas_rect = Rect { x: 20, ..rect };
                canvas.image(&icon, atlas_rect);
                let mapped = [[
                    (0.4_f32 * 65535.0).round() as u16,
                    (0.2_f32 * 65535.0).round() as u16,
                    1,
                ]; 256];
                canvas.mark_item_glint_with_mapping(&icon, atlas_rect, None, Some(&mapped));
                renderer.update_overlay(&canvas, false).unwrap();
                let still = render(&renderer, super::glint_uniform(0, 0.5, 0.0, true), false);
                let first = render(&renderer, super::glint_uniform(0, 0.5, 0.75, true), false);
                let second = render(
                    &renderer,
                    super::glint_uniform(12_000, 0.5, 0.75, true),
                    false,
                );
                assert_ne!(
                    &still[0..64],
                    &first[0..64],
                    "glint must brighten item pixels"
                );
                assert_ne!(&first[0..64], &second[0..64], "glint must animate");
                let red = match renderer.config.format {
                    wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => 2,
                    _ => 0,
                };
                assert!(
                    first[20 * 4 + red] < first[red],
                    "atlas coordinates must select a different glint texel"
                );
                assert_ne!(
                    &first[20 * 4..36 * 4],
                    &second[20 * 4..36 * 4],
                    "atlas glint must animate"
                );
                for y in 0..16 {
                    let outside = y * 256 + 16 * 4;
                    assert_eq!(
                        &still[outside..y * 256 + 20 * 4],
                        &first[outside..y * 256 + 20 * 4]
                    );
                    assert_eq!(
                        &first[y * 256 + 36 * 4..y * 256 + 256],
                        &second[y * 256 + 36 * 4..y * 256 + 256]
                    );
                }
                renderer.update_pre_blur_overlay(&canvas).unwrap();
                let mut plain = Canvas::new(64, 16);
                plain.fill(
                    Rect {
                        x: 0,
                        y: 0,
                        w: 64,
                        h: 16,
                    },
                    [80, 80, 80, 255],
                );
                renderer.update_overlay(&plain, false).unwrap();
                let plain_off = render(&renderer, super::glint_uniform(0, 0.5, 0.0, true), false);
                let plain_on = render(&renderer, super::glint_uniform(0, 0.5, 0.75, true), false);
                assert_eq!(plain_on, plain_off, "old screen glint mask must be cleared");
                let hud_off = render(&renderer, super::glint_uniform(0, 0.5, 0.0, true), true);
                let hud_on = render(&renderer, super::glint_uniform(0, 0.5, 0.75, true), true);
                assert_ne!(hud_on, hud_off, "HUD glint must remain active");
                renderer.update_pre_blur_overlay(&plain).unwrap();
                renderer.update_overlay(&canvas, false).unwrap();
                let hud_plain_off =
                    render(&renderer, super::glint_uniform(0, 0.5, 0.0, true), true);
                let hud_plain_on =
                    render(&renderer, super::glint_uniform(0, 0.5, 0.75, true), true);
                assert_eq!(
                    hud_plain_on, hud_plain_off,
                    "old HUD glint mask must be cleared"
                );
                event_loop.exit();
            }
            fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
        }
        let mut builder = EventLoop::builder();
        builder.with_any_thread(true);
        builder.build().unwrap().run_app(&mut Harness).unwrap();
    }

    #[test]
    fn translucent_order_reuse_matches_a_fresh_sort() {
        let centers: Vec<_> = (0..80)
            .map(|i| {
                glam::Vec3::new(
                    (i * 17 % 97) as f32 * 0.73,
                    (i * 31 % 71) as f32 * 0.29,
                    (i * 43 % 83) as f32 * 0.57,
                )
            })
            .collect();
        let order = |eye| {
            let mut indices: Vec<_> = (0..centers.len()).collect();
            indices.sort_unstable_by(|&a, &b| {
                centers[b]
                    .distance_squared(eye)
                    .total_cmp(&centers[a].distance_squared(eye))
            });
            indices
        };
        let initial = order(glam::Vec3::new(2.25, 8.75, -1.125));
        let sorted: Vec<_> = initial.iter().map(|&index| centers[index]).collect();
        let mut reused = 0;
        let mut resorted = 0;
        for i in 0..2_000 {
            let eye = glam::Vec3::new(2.25 + i as f32 * 0.0005, 8.75, -1.125);
            if super::strictly_far_to_near(&sorted, eye) {
                assert_eq!(initial, order(eye));
                reused += 1;
            } else {
                resorted += 1;
            }
        }
        assert!(reused > 0 && resorted > 0);
        assert!(!super::strictly_far_to_near(
            &[glam::Vec3::X, -glam::Vec3::X],
            glam::Vec3::ZERO
        ));
    }

    #[test]
    #[ignore = "requires a GPU and locally installed Minecraft 26.3; does not open a window"]
    fn gpu_demo_atlas_upload_preserves_all_tiles() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let atlas = native_resources::load_atlas(&game).unwrap();
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
            let size = wgpu::Extent3d {
                width: atlas.width,
                height: atlas.height,
                depth_or_array_layers: 1,
            };
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("demo atlas upload test"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let row_bytes = atlas.width * 4;
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &atlas.rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row_bytes),
                    rows_per_image: Some(atlas.height),
                },
                size,
            );
            let aligned_row = row_bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
                * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("atlas readback"),
                size: u64::from(aligned_row) * u64::from(atlas.height),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(aligned_row),
                        rows_per_image: Some(atlas.height),
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
            for y in 0..atlas.height as usize {
                let source = y * row_bytes as usize;
                let target = y * aligned_row as usize;
                assert_eq!(
                    &data[target..target + row_bytes as usize],
                    &atlas.rgba[source..source + row_bytes as usize]
                );
            }
            drop(data);
            buffer.unmap();
            if let Some(error) = errors.pop().await {
                panic!("GPU validation failed: {error}");
            }
        });
    }
}
