//! GPU-backed title screen. GUI pixels are uploaded only when widgets change;
//! the panorama itself is animated on the GPU.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
#[cfg(any(debug_assertions, test))]
use std::time::Duration;
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail, ensure};
use image::RgbaImage;
use wgpu::util::DeviceExt;
use winit::{dpi::PhysicalSize, window::Window};

use crate::{menu_assets::MenuAssets, menu_draw::Canvas, menu_gpu_worker::SurfaceRequester};

// Kept out of release builds; separates GPU queueing from surface pacing.
#[cfg(any(debug_assertions, test))]
#[derive(Default)]
struct GpuFrameTimings {
    blurred: Option<bool>,
    warmup: usize,
    startup_intervals: Vec<Duration>,
    samples: Vec<[Duration; 4]>,
    intervals: Vec<Duration>,
    previous: Option<Instant>,
    reported: bool,
}

#[cfg(any(debug_assertions, test))]
impl GpuFrameTimings {
    fn record(&mut self, blurred: bool, sample: [Duration; 4]) {
        if self.blurred != Some(blurred) {
            *self = Self {
                blurred: Some(blurred),
                ..Self::default()
            };
        }
        if self.reported {
            return;
        }
        let now = Instant::now();
        let previous = self.previous.replace(now);
        if self.warmup < 30 {
            self.warmup += 1;
            if let Some(previous) = previous {
                self.startup_intervals.push(now.duration_since(previous));
            }
            if self.warmup == 30 && !self.startup_intervals.is_empty() {
                self.startup_intervals.sort_unstable();
                let n = self.startup_intervals.len();
                eprintln!(
                    "Menu GPU startup render intervals: blurred={blurred} samples={n} median={:?} max={:?}; excludes display scanout",
                    self.startup_intervals[n / 2],
                    self.startup_intervals[n - 1],
                );
            }
            return;
        }
        if let Some(previous) = previous {
            self.intervals.push(now.duration_since(previous));
        }
        self.samples.push(sample);
        if self.samples.len() != 300 {
            return;
        }
        let stats = |stage: usize| {
            let mut times: Vec<_> = self.samples.iter().map(|sample| sample[stage]).collect();
            times.sort_unstable();
            [times[150], times[285], times[297]]
        };
        self.intervals.sort_unstable();
        eprintln!(
            "Menu GPU CPU stages: blurred={blurred} samples=300 median/p95/p99 render-interval={:?} acquire={:?} encode={:?} submit={:?} present={:?}; intervals exclude display scanout",
            [
                self.intervals[150],
                self.intervals[285],
                self.intervals[297]
            ],
            stats(0),
            stats(1),
            stats(2),
            stats(3)
        );
        self.reported = true;
    }
}

pub struct MenuRenderer {
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    surface_requester: SurfaceRequester,
    window: Arc<Window>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    panorama_pipeline: wgpu::RenderPipeline,
    blur_pipelines: Option<BlurPipelines>,
    blur_pipeline_task: Option<mpsc::Receiver<Result<BlurPipelines>>>,
    overlay_pipeline: wgpu::RenderPipeline,
    gui_scale: crate::gui_gpu::GuiScaleBinding,
    panorama_layout: wgpu::BindGroupLayout,
    image_layout: wgpu::BindGroupLayout,
    blur_layout: wgpu::BindGroupLayout,
    blur_radius_buffer: wgpu::Buffer,
    blur_radius: u8,
    params: wgpu::Buffer,
    nearest: wgpu::Sampler,
    blur_sampler: wgpu::Sampler,
    panorama_sampler: wgpu::Sampler,
    panorama_texture: Option<wgpu::Texture>,
    panorama_overlay_texture: Option<wgpu::Texture>,
    panorama_binding: Option<wgpu::BindGroup>,
    previous_assets: Option<Arc<MenuAssets>>,
    blur_texture: Option<wgpu::Texture>,
    blur_view: Option<wgpu::TextureView>,
    blur_binding: Option<wgpu::BindGroup>,
    blur_composite_binding: Option<wgpu::BindGroup>,
    blur_temporary_texture: Option<wgpu::Texture>,
    blur_temporary_view: Option<wgpu::TextureView>,
    blur_temporary_binding: Option<wgpu::BindGroup>,
    blur_size: (u32, u32),
    overlay_texture: Option<wgpu::Texture>,
    overlay_binding: Option<wgpu::BindGroup>,
    overlay_size: (u32, u32),
    screenshot_requests: std::collections::VecDeque<crate::screenshot::ScreenshotRequest>,
    screenshot_surface_copy: bool,
    screenshot_blitter: Option<wgpu::util::TextureBlitter>,
    lost_device: Arc<AtomicBool>,
    #[cfg(any(debug_assertions, test))]
    frame_timings: Option<GpuFrameTimings>,
}

struct BlurPipelines {
    horizontal: wgpu::RenderPipeline,
    vertical: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
}

impl MenuRenderer {
    pub fn new(window: Arc<Window>, vsync: bool, surfaces: SurfaceRequester) -> Result<Self> {
        let mut errors = Vec::new();
        for (name, backend) in [
            ("DirectX 12", wgpu::Backends::DX12),
            ("Vulkan", wgpu::Backends::VULKAN),
        ] {
            match pollster::block_on(Self::with_backend(
                Arc::clone(&window),
                backend,
                vsync,
                surfaces.clone(),
            )) {
                Ok(renderer) => return Ok(renderer),
                Err(error) => {
                    #[cfg(any(debug_assertions, test))]
                    if std::env::var_os("NATIVE_MENU_FRAME_TIMINGS").is_some() {
                        eprintln!("Menu {name} initialization failed: {error:#}");
                    }
                    errors.push(format!("{name}: {error:#}"));
                }
            }
        }
        bail!("cannot initialize the menu GPU:\n{}", errors.join("\n"))
    }

    async fn with_backend(
        window: Arc<Window>,
        backend: wgpu::Backends,
        vsync: bool,
        surfaces: SurfaceRequester,
    ) -> Result<Self> {
        #[cfg(any(debug_assertions, test))]
        let startup = std::env::var_os("NATIVE_MENU_FRAME_TIMINGS").map(|_| Instant::now());
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = backend;
        let instance = wgpu::Instance::new(descriptor);
        #[cfg(any(debug_assertions, test))]
        let instance_ready = startup.map(|start| start.elapsed());
        let surface = surfaces.create(&instance)?;
        #[cfg(any(debug_assertions, test))]
        let surface_ready = startup.map(|start| start.elapsed());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .context("no compatible menu GPU adapter")?;
        #[cfg(any(debug_assertions, test))]
        let adapter_ready = startup.map(|start| start.elapsed());
        #[cfg(any(debug_assertions, test))]
        if std::env::var_os("NATIVE_MENU_FRAME_TIMINGS").is_some() {
            eprintln!("Menu graphics adapter: {:?}", adapter.get_info());
        }
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("Minecraft menu device"),
                ..Default::default()
            })
            .await
            .context("cannot create menu GPU device")?;
        #[cfg(any(debug_assertions, test))]
        let device_ready = startup.map(|start| start.elapsed());
        let lost_device = Arc::new(AtomicBool::new(false));
        let lost_flag = Arc::clone(&lost_device);
        device.set_device_lost_callback(move |_, _| lost_flag.store(true, Ordering::Relaxed));
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or_else(|| anyhow!("menu surface has no compatible configuration"))?;
        let capabilities = surface.get_capabilities(&adapter);
        let screenshot_surface_copy = capabilities.usages.contains(wgpu::TextureUsages::COPY_SRC);
        if screenshot_surface_copy {
            config.usage |= wgpu::TextureUsages::COPY_SRC;
        }
        if let Some(format) = capabilities
            .formats
            .iter()
            .copied()
            .into_iter()
            .find(|format| !format.is_srgb())
        {
            config.format = format;
        }
        config.present_mode = crate::menu_settings::present_mode(vsync);
        #[cfg(any(debug_assertions, test))]
        if std::env::var_os("NATIVE_MENU_FRAME_TIMINGS").is_some() {
            eprintln!(
                "Menu surface: modes={:?}, selected={:?}, latency={}",
                capabilities.present_modes,
                config.present_mode,
                config.desired_maximum_frame_latency
            );
        }
        surface.configure(&device, &config);
        #[cfg(any(debug_assertions, test))]
        let configured = startup.map(|start| start.elapsed());

        let panorama_layout = panorama_layout(&device);
        let image_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("menu image bindings"),
            entries: &[texture_entry(0), sampler_entry(1)],
        });
        let blur_layout = create_blur_layout(&device);
        let blur_radius_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("menu blur radius"),
            contents: bytemuck::cast_slice(&[5_u32, 0, 0, 0]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let nearest = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("menu nearest sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let blur_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("menu blur linear sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let panorama_sampler = panorama_sampler(&device);
        let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("panorama parameters"),
            contents: bytemuck::cast_slice(&[1.0_f32, 0.0, 0.0, 0.0]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let panorama_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("panorama shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("menu_panorama.wgsl").into()),
        });
        let panorama_pipeline = pipeline(
            &device,
            &panorama_layout,
            &panorama_shader,
            config.format,
            None,
            "panorama pipeline",
        );
        let gui_scale = crate::gui_gpu::GuiScaleBinding::new(&device);
        let overlay_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("menu GUI shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("menu_gui_overlay.wgsl").into()),
        });
        let overlay_pipeline = pipeline_entry_layouts(
            &device,
            &[&image_layout, &gui_scale.layout],
            &overlay_shader,
            config.format,
            Some(wgpu::BlendState::ALPHA_BLENDING),
            "fragment",
            "menu widgets pipeline",
        );
        let mut renderer = Self {
            instance,
            surface,
            surface_requester: surfaces,
            window,
            device,
            queue,
            config,
            panorama_pipeline,
            blur_pipelines: None,
            blur_pipeline_task: None,
            overlay_pipeline,
            gui_scale,
            panorama_layout,
            image_layout,
            blur_layout,
            blur_radius_buffer,
            blur_radius: 5,
            params,
            nearest,
            blur_sampler,
            panorama_sampler,
            panorama_texture: None,
            panorama_overlay_texture: None,
            panorama_binding: None,
            previous_assets: None,
            blur_texture: None,
            blur_view: None,
            blur_binding: None,
            blur_composite_binding: None,
            blur_temporary_texture: None,
            blur_temporary_view: None,
            blur_temporary_binding: None,
            blur_size: (0, 0),
            overlay_texture: None,
            overlay_binding: None,
            overlay_size: (0, 0),
            screenshot_requests: std::collections::VecDeque::new(),
            screenshot_surface_copy,
            screenshot_blitter: None,
            lost_device,
            #[cfg(any(debug_assertions, test))]
            frame_timings: std::env::var_os("NATIVE_MENU_FRAME_TIMINGS")
                .map(|_| GpuFrameTimings::default()),
        };
        renderer.set_assets(None)?;
        #[cfg(any(debug_assertions, test))]
        if let (
            Some(start),
            Some(instance),
            Some(surface),
            Some(adapter),
            Some(device),
            Some(configured),
        ) = (
            startup,
            instance_ready,
            surface_ready,
            adapter_ready,
            device_ready,
            configured,
        ) {
            eprintln!(
                "Menu GPU initialization CPU stages: backend={backend:?} instance={instance:?} surface={:?} adapter={:?} device={:?} configure={:?} pipelines-and-placeholder={:?}; excludes display scanout",
                surface - instance,
                adapter - surface,
                device - adapter,
                configured - device,
                start.elapsed() - configured,
            );
        }
        Ok(renderer)
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 || self.lost_device.load(Ordering::Relaxed) {
            return;
        }
        if self.config.width == size.width && self.config.height == size.height {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
    }

    pub fn set_vsync(&mut self, enabled: bool) {
        let mode = crate::menu_settings::present_mode(enabled);
        if self.config.present_mode != mode && !self.lost_device.load(Ordering::Relaxed) {
            self.config.present_mode = mode;
            self.surface.configure(&self.device, &self.config);
        }
    }

    pub fn set_gui_scale(&mut self, scale: u32) {
        self.gui_scale.set(&self.queue, scale);
    }

    pub fn set_assets(&mut self, assets: Option<&Arc<MenuAssets>>) -> Result<()> {
        let same = match (assets, &self.previous_assets) {
            (Some(current), Some(previous)) => Arc::ptr_eq(current, previous),
            (None, None) => self.panorama_binding.is_some(),
            _ => false,
        };
        if same {
            return Ok(());
        }
        let placeholders: [RgbaImage; 6] =
            std::array::from_fn(|_| RgbaImage::from_pixel(1, 1, image::Rgba([32, 32, 32, 255])));
        let image_format = if self.config.format.is_srgb() {
            wgpu::TextureFormat::Rgba8UnormSrgb
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        };
        let images = assets.map_or(placeholders.as_slice(), |assets| assets.panorama.as_slice());
        let panorama = upload_panorama(&self.device, &self.queue, images, image_format)?;
        let panorama_view = cube_view(&panorama);
        let transparent = RgbaImage::from_pixel(1, 1, image::Rgba([255, 255, 255, 0]));
        let overlay = upload_rgba(
            &self.device,
            &self.queue,
            assets.map_or(&transparent, |assets| &assets.panorama_overlay),
            image_format,
            "panorama overlay",
        )?;
        let overlay_view = overlay.create_view(&wgpu::TextureViewDescriptor::default());
        self.panorama_binding = Some(panorama_binding(
            &self.device,
            &self.panorama_layout,
            &panorama_view,
            &self.panorama_sampler,
            &self.params,
            &overlay_view,
        ));
        self.panorama_texture = Some(panorama);
        self.panorama_overlay_texture = Some(overlay);
        self.previous_assets = assets.cloned();
        Ok(())
    }

    pub fn update_overlay(&mut self, canvas: &Canvas) -> Result<()> {
        let size = (canvas.width as u32, canvas.height as u32);
        ensure!(size.0 > 0 && size.1 > 0, "menu overlay is empty");
        if self.overlay_size != size {
            let format = canvas_texture_format(self.config.format.is_srgb());
            let texture = texture(
                &self.device,
                size,
                format,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                "menu overlay",
            )?;
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.overlay_binding = Some(image_binding(
                &self.device,
                &self.image_layout,
                &view,
                &self.nearest,
            ));
            self.overlay_texture = Some(texture);
            self.overlay_size = size;
        }
        write_canvas(
            &self.queue,
            self.overlay_texture
                .as_ref()
                .context("menu overlay missing")?,
            size,
            canvas,
        );
        Ok(())
    }

    pub fn request_screenshots(&mut self, requests: Vec<crate::screenshot::ScreenshotRequest>) {
        for request in requests {
            if self.screenshot_requests.len() >= 4 {
                request
                    .sender
                    .report_error(anyhow::anyhow!("screenshot queue is full"));
            } else {
                self.screenshot_requests.push_back(request);
            }
        }
    }

    pub fn has_pending_screenshots(&self) -> bool {
        !self.screenshot_requests.is_empty()
    }

    fn start_blur_pipeline_task(&mut self) -> Result<()> {
        if self.blur_pipelines.is_some() || self.blur_pipeline_task.is_some() {
            return Ok(());
        }
        let device = self.device.clone();
        let blur_layout = self.blur_layout.clone();
        let image_layout = self.image_layout.clone();
        let format = self.config.format;
        let (send, receive) = mpsc::channel();
        std::thread::Builder::new()
            .name("menu-blur-pipelines".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let blur_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("menu blur shader"),
                        source: wgpu::ShaderSource::Wgsl(include_str!("menu_blur.wgsl").into()),
                    });
                    let image_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("menu image shader"),
                        source: wgpu::ShaderSource::Wgsl(include_str!("overlay.wgsl").into()),
                    });
                    BlurPipelines {
                        horizontal: pipeline_entry(
                            &device,
                            &blur_layout,
                            &blur_shader,
                            format,
                            None,
                            "horizontal",
                            "menu horizontal blur",
                        ),
                        vertical: pipeline_entry(
                            &device,
                            &blur_layout,
                            &blur_shader,
                            format,
                            None,
                            "vertical",
                            "menu vertical blur",
                        ),
                        composite: pipeline(
                            &device,
                            &image_layout,
                            &image_shader,
                            format,
                            None,
                            "blurred background pipeline",
                        ),
                    }
                }))
                .map_err(|_| anyhow!("menu blur pipeline initialization panicked"));
                let _ = send.send(result);
            })
            .context("cannot start menu blur pipeline worker")?;
        self.blur_pipeline_task = Some(receive);
        Ok(())
    }

    fn blur_pipelines_ready(&mut self) -> Result<bool> {
        if self.blur_pipelines.is_some() {
            return Ok(true);
        }
        self.start_blur_pipeline_task()?;
        match self
            .blur_pipeline_task
            .as_ref()
            .expect("pipeline worker started")
            .try_recv()
        {
            Ok(result) => {
                self.blur_pipelines = Some(result?);
                self.blur_pipeline_task = None;
                Ok(true)
            }
            Err(mpsc::TryRecvError::Empty) => Ok(false),
            Err(mpsc::TryRecvError::Disconnected) => bail!("menu blur pipeline worker stopped"),
        }
    }

    fn ensure_blur(&mut self) -> Result<()> {
        // Java applies the post effect in framebuffer pixels, not GUI pixels.
        let size = (self.config.width.max(1), self.config.height.max(1));
        if self.blur_size == size {
            return Ok(());
        }
        let background = texture(
            &self.device,
            size,
            self.config.format,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            "menu blurred backdrop",
        )?;
        let view = background.create_view(&wgpu::TextureViewDescriptor::default());
        self.blur_binding = Some(blur_binding(
            &self.device,
            &self.blur_layout,
            &view,
            &self.blur_sampler,
            &self.blur_radius_buffer,
        ));
        self.blur_composite_binding = Some(image_binding(
            &self.device,
            &self.image_layout,
            &view,
            &self.blur_sampler,
        ));
        let temporary = texture(
            &self.device,
            size,
            self.config.format,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            "menu blur intermediate",
        )?;
        let temporary_view = temporary.create_view(&wgpu::TextureViewDescriptor::default());
        self.blur_temporary_binding = Some(blur_binding(
            &self.device,
            &self.blur_layout,
            &temporary_view,
            &self.blur_sampler,
            &self.blur_radius_buffer,
        ));
        self.blur_temporary_view = Some(temporary_view);
        self.blur_temporary_texture = Some(temporary);
        self.blur_view = Some(view);
        self.blur_texture = Some(background);
        self.blur_size = size;
        Ok(())
    }

    pub fn render(
        &mut self,
        seconds: f32,
        speed: f32,
        requested_at: Instant,
        blurred: bool,
        radius: u8,
    ) -> Result<bool> {
        if self.lost_device.load(Ordering::Relaxed) {
            bail!("menu graphics device was lost");
        }
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(false);
        }
        let blurred = blurred && radius > 0;
        if blurred {
            if !self.blur_pipelines_ready()? {
                return Ok(false);
            }
            let radius = radius.min(10);
            if radius != self.blur_radius {
                self.queue.write_buffer(
                    &self.blur_radius_buffer,
                    0,
                    bytemuck::cast_slice(&[u32::from(radius), 0, 0, 0]),
                );
                self.blur_radius = radius;
            }
            self.ensure_blur()?;
        }
        #[cfg(any(debug_assertions, test))]
        let started = self.frame_timings.as_ref().map(|_| Instant::now());
        let (frame, suboptimal) = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => (frame, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => (frame, true),
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(false);
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                return Ok(false);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface = self
                    .surface_requester
                    .create(&self.instance)
                    .context("cannot restore menu surface")?;
                self.surface.configure(&self.device, &self.config);
                return Ok(false);
            }
            wgpu::CurrentSurfaceTexture::Validation => bail!("menu surface validation failed"),
        };
        #[cfg(any(debug_assertions, test))]
        let acquired = started.map(|start| start.elapsed());
        // Surface acquisition may wait for vertical sync. Use the animation
        // time at acquisition instead of the potentially stale queued request.
        let seconds = animation_seconds(seconds, speed, requested_at, Instant::now());
        let params = [
            self.config.width as f32 / self.config.height as f32,
            seconds * (std::f32::consts::PI / 90.0),
            0.0,
            0.0,
        ];
        self.queue
            .write_buffer(&self.params, 0, bytemuck::cast_slice(&params));
        let surface_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let screenshot = self.screenshot_requests.pop_front();
        let screenshot_texture = screenshot
            .as_ref()
            .filter(|_| !self.screenshot_surface_copy)
            .map(|_| {
                self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("menu screenshot frame"),
                    size: wgpu::Extent3d {
                        width: self.config.width,
                        height: self.config.height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: self.config.format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                })
            });
        let screenshot_view = screenshot_texture
            .as_ref()
            .map(|texture| texture.create_view(&wgpu::TextureViewDescriptor::default()));
        let view = screenshot_view.as_ref().unwrap_or(&surface_view);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("menu frame"),
            });
        if blurred {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("panorama at framebuffer resolution"),
                color_attachments: &[Some(color_attachment(
                    self.blur_view.as_ref().context("blur target missing")?,
                    wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                ))],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.panorama_pipeline);
            pass.set_bind_group(
                0,
                self.panorama_binding.as_ref().context("panorama missing")?,
                &[],
            );
            pass.draw(0..3, 0..1);
        }
        if blurred {
            for _ in 0..3 {
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("menu horizontal blur"),
                        color_attachments: &[Some(color_attachment(
                            self.blur_temporary_view
                                .as_ref()
                                .context("blur intermediate missing")?,
                            wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        ))],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass.set_pipeline(
                        &self
                            .blur_pipelines
                            .as_ref()
                            .context("blur pipelines missing")?
                            .horizontal,
                    );
                    pass.set_bind_group(
                        0,
                        self.blur_binding.as_ref().context("blur binding missing")?,
                        &[],
                    );
                    pass.draw(0..3, 0..1);
                }
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("menu vertical blur"),
                        color_attachments: &[Some(color_attachment(
                            self.blur_view.as_ref().context("blur target missing")?,
                            wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        ))],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass.set_pipeline(
                        &self
                            .blur_pipelines
                            .as_ref()
                            .context("blur pipelines missing")?
                            .vertical,
                    );
                    pass.set_bind_group(
                        0,
                        self.blur_temporary_binding
                            .as_ref()
                            .context("blur intermediate binding missing")?,
                        &[],
                    );
                    pass.draw(0..3, 0..1);
                }
            }
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("menu composite"),
                color_attachments: &[Some(color_attachment(
                    view,
                    wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                ))],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if blurred {
                pass.set_pipeline(
                    &self
                        .blur_pipelines
                        .as_ref()
                        .context("blur pipelines missing")?
                        .composite,
                );
                pass.set_bind_group(
                    0,
                    self.blur_composite_binding
                        .as_ref()
                        .context("blur composite binding missing")?,
                    &[],
                );
                pass.draw(0..6, 0..1);
            } else {
                pass.set_pipeline(&self.panorama_pipeline);
                pass.set_bind_group(
                    0,
                    self.panorama_binding.as_ref().context("panorama missing")?,
                    &[],
                );
                pass.draw(0..3, 0..1);
            }
            if let Some(binding) = &self.overlay_binding {
                pass.set_pipeline(&self.overlay_pipeline);
                pass.set_bind_group(0, binding, &[]);
                pass.set_bind_group(1, &self.gui_scale.bind_group, &[]);
                pass.draw(0..6, 0..1);
            }
        }
        #[cfg(any(debug_assertions, test))]
        let encoded = started.map(|start| start.elapsed());
        let readback = if screenshot.is_some() {
            let texture = screenshot_texture.as_ref().unwrap_or(&frame.texture);
            let readback = crate::screenshot::Readback::encode(
                &self.device,
                &mut encoder,
                texture,
                (self.config.width, self.config.height),
                self.config.format,
            );
            if screenshot_texture.is_some() {
                let blitter = self.screenshot_blitter.get_or_insert_with(|| {
                    wgpu::util::TextureBlitter::new(&self.device, self.config.format)
                });
                blitter.copy(&self.device, &mut encoder, view, &surface_view);
            }
            Some(readback)
        } else {
            None
        };
        let submission = self.queue.submit([encoder.finish()]);
        if let (Some(request), Some(readback)) = (screenshot, readback) {
            let result = readback.and_then(|readback| {
                request
                    .sender
                    .submit(&self.device, readback, submission, &request.directory)
            });
            if let Err(error) = result {
                request.sender.report_error(error);
            }
        }
        #[cfg(any(debug_assertions, test))]
        let submitted = started.map(|start| start.elapsed());
        self.queue.present(frame);
        // The title does not need blur. Start compilation only after its first
        // frame is submitted so the driver cannot delay initial display.
        if !blurred {
            self.start_blur_pipeline_task()?;
        }
        #[cfg(any(debug_assertions, test))]
        if let (Some(start), Some(acquired), Some(encoded), Some(submitted), Some(timings)) = (
            started,
            acquired,
            encoded,
            submitted,
            self.frame_timings.as_mut(),
        ) {
            timings.record(
                blurred,
                [
                    acquired,
                    encoded - acquired,
                    submitted - encoded,
                    start.elapsed() - submitted,
                ],
            );
        }
        if suboptimal {
            self.surface.configure(&self.device, &self.config);
        }
        Ok(true)
    }
}

impl Drop for MenuRenderer {
    fn drop(&mut self) {
        for request in self.screenshot_requests.drain(..) {
            request.sender.report_error(anyhow::anyhow!(
                "menu renderer stopped before taking screenshot"
            ));
        }
    }
}

fn animation_seconds(
    requested_seconds: f32,
    speed: f32,
    requested_at: Instant,
    acquired_at: Instant,
) -> f32 {
    requested_seconds
        + acquired_at
            .saturating_duration_since(requested_at)
            .as_secs_f32()
            * speed
}

// CubeMapTexture in 26.3 always enables linear filtering for the panorama.
fn panorama_sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("panorama linear sampler"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    })
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn panorama_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("panorama bindings"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::Cube,
                    multisampled: false,
                },
                count: None,
            },
            sampler_entry(1),
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            texture_entry(3),
        ],
    })
}

fn panorama_binding(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    faces: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    params: &wgpu::Buffer,
    overlay: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("panorama cube"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(faces),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: params.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(overlay),
            },
        ],
    })
}

fn cube_view(texture: &wgpu::Texture) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::Cube),
        array_layer_count: Some(6),
        ..Default::default()
    })
}

fn upload_panorama(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    images: &[RgbaImage],
    format: wgpu::TextureFormat,
) -> Result<wgpu::Texture> {
    crate::menu_assets::validate_panorama_faces(images)?;
    let size = images[0].width();
    ensure!(
        size <= device.limits().max_texture_dimension_2d,
        "Panorama texture size exceeds GPU limits"
    );
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("panorama cube"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 6,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    // The cube layers and vertical flip follow 26.3's CubeMapTexture.
    let row_bytes = size as usize * 4;
    let mut flipped = vec![0; row_bytes * size as usize];
    for (layer, image_index) in [1, 3, 5, 4, 0, 2].into_iter().enumerate() {
        for (source, target) in images[image_index]
            .as_raw()
            .chunks_exact(row_bytes)
            .rev()
            .zip(flipped.chunks_exact_mut(row_bytes))
        {
            target.copy_from_slice(source);
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: layer as u32,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &flipped,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size * 4),
                rows_per_image: Some(size),
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
        );
    }
    Ok(texture)
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

pub(crate) fn create_blur_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("menu blur bindings"),
        entries: &[
            texture_entry(0),
            sampler_entry(1),
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    })
}

pub(crate) fn blur_binding(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    radius: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("menu blur input"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: radius.as_entire_binding(),
            },
        ],
    })
}

pub(crate) fn pipeline(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
    label: &str,
) -> wgpu::RenderPipeline {
    pipeline_entry(device, layout, shader, format, blend, "fragment", label)
}

pub(crate) fn pipeline_entry(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
    fragment: &str,
    label: &str,
) -> wgpu::RenderPipeline {
    pipeline_entry_layouts(device, &[layout], shader, format, blend, fragment, label)
}

fn pipeline_entry_layouts(
    device: &wgpu::Device,
    layouts: &[&wgpu::BindGroupLayout],
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
    fragment: &str,
    label: &str,
) -> wgpu::RenderPipeline {
    let layouts: Vec<_> = layouts.iter().map(|layout| Some(*layout)).collect();
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &layouts,
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vertex"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

pub(crate) fn texture(
    device: &wgpu::Device,
    size: (u32, u32),
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
    label: &str,
) -> Result<wgpu::Texture> {
    ensure!(
        size.0 > 0
            && size.1 > 0
            && size.0 <= device.limits().max_texture_dimension_2d
            && size.1 <= device.limits().max_texture_dimension_2d,
        "{label} size exceeds GPU limits"
    );
    Ok(device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    }))
}

fn upload_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    image: &RgbaImage,
    format: wgpu::TextureFormat,
    label: &str,
) -> Result<wgpu::Texture> {
    let size = (image.width(), image.height());
    let texture = texture(
        device,
        size,
        format,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        label,
    )?;
    write_rgba(queue, &texture, size, image.as_raw());
    Ok(texture)
}

fn canvas_texture_format(srgb: bool) -> wgpu::TextureFormat {
    #[cfg(target_endian = "little")]
    {
        if srgb {
            wgpu::TextureFormat::Bgra8UnormSrgb
        } else {
            wgpu::TextureFormat::Bgra8Unorm
        }
    }
    #[cfg(target_endian = "big")]
    {
        if srgb {
            wgpu::TextureFormat::Rgba8UnormSrgb
        } else {
            wgpu::TextureFormat::Rgba8Unorm
        }
    }
}

fn write_canvas(queue: &wgpu::Queue, texture: &wgpu::Texture, size: (u32, u32), canvas: &Canvas) {
    // Canvas stores ARGB u32 pixels. On Windows their in-memory bytes are
    // BGRA, so a BGRA texture can read them directly without a second buffer.
    #[cfg(target_endian = "little")]
    write_rgba(queue, texture, size, bytemuck::cast_slice(&canvas.pixels));
    #[cfg(target_endian = "big")]
    {
        let mut rgba = Vec::with_capacity(canvas.pixels.len() * 4);
        for &pixel in &canvas.pixels {
            rgba.extend_from_slice(&[
                (pixel >> 16) as u8,
                (pixel >> 8) as u8,
                pixel as u8,
                (pixel >> 24) as u8,
            ]);
        }
        write_rgba(queue, texture, size, &rgba);
    }
}

fn write_rgba(queue: &wgpu::Queue, texture: &wgpu::Texture, size: (u32, u32), bytes: &[u8]) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytes,
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
}

pub(crate) fn image_binding(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("menu image"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

pub(crate) fn color_attachment(
    view: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
) -> wgpu::RenderPassColorAttachment<'_> {
    wgpu::RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load,
            store: wgpu::StoreOp::Store,
        },
    }
}

#[cfg(test)]
#[path = "panorama_reference.rs"]
mod panorama_reference;

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};

    #[cfg(target_endian = "little")]
    #[test]
    #[ignore = "CPU performance measurement"]
    fn measure_menu_overlay_preparation() {
        use std::{hint::black_box, time::Instant};
        let mut canvas = Canvas::new(640, 360);
        for (index, pixel) in canvas.pixels.iter_mut().enumerate() {
            *pixel = (index as u32).wrapping_mul(0x9e37_79b9);
        }
        let mut old = Vec::new();
        let mut direct = Vec::new();
        for _ in 0..500 {
            let start = Instant::now();
            let mut rgba = Vec::with_capacity(canvas.pixels.len() * 4);
            for &pixel in black_box(&canvas.pixels) {
                rgba.extend_from_slice(&[
                    (pixel >> 16) as u8,
                    (pixel >> 8) as u8,
                    pixel as u8,
                    (pixel >> 24) as u8,
                ]);
            }
            black_box(rgba);
            old.push(start.elapsed());
            let start = Instant::now();
            let bytes: &[u8] = bytemuck::cast_slice(black_box(&canvas.pixels));
            black_box(bytes);
            direct.push(start.elapsed());
        }
        old.sort_unstable();
        direct.sort_unstable();
        println!(
            "640x360 menu overlay preparation, 500 samples: RGBA conversion median={:?} p95={:?}; BGRA view median={:?} p95={:?}; excludes GPU upload and display",
            old[250], old[475], direct[250], direct[475]
        );
    }

    #[test]
    #[ignore = "requires a GPU; renders offscreen without opening a window"]
    fn gpu_gui_overlay_clips_partial_logical_pixels() {
        pollster::block_on(async {
            let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
            descriptor.backends = test_backend();
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
            for srgb in [false, true] {
                let errors = device.push_error_scope(wgpu::ErrorFilter::Validation);
                let image_layout =
                    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                        label: Some("test image layout"),
                        entries: &[texture_entry(0), sampler_entry(1)],
                    });
                let mut scale = crate::gui_gpu::GuiScaleBinding::new(&device);
                scale.set(&queue, 3);
                let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("test GUI shader"),
                    source: wgpu::ShaderSource::Wgsl(include_str!("menu_gui_overlay.wgsl").into()),
                });
                let format = if srgb {
                    wgpu::TextureFormat::Rgba8UnormSrgb
                } else {
                    wgpu::TextureFormat::Rgba8Unorm
                };
                let pipeline = pipeline_entry_layouts(
                    &device,
                    &[&image_layout, &scale.layout],
                    &shader,
                    format,
                    None,
                    "fragment",
                    "test GUI pipeline",
                );
                let image = RgbaImage::from_fn(3, 2, |x, y| {
                    image::Rgba([(x * 60) as u8, (y * 100) as u8, 13, (x * 85 + y * 31) as u8])
                });
                let mut canvas = Canvas::new(3, 2);
                for (pixel, rgba) in canvas.pixels.iter_mut().zip(image.pixels()) {
                    *pixel = (u32::from(rgba[3]) << 24)
                        | (u32::from(rgba[0]) << 16)
                        | (u32::from(rgba[1]) << 8)
                        | u32::from(rgba[2]);
                }
                let source = texture(
                    &device,
                    (3, 2),
                    canvas_texture_format(srgb),
                    wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    "test GUI source",
                )
                .unwrap();
                write_canvas(&queue, &source, (3, 2), &canvas);
                let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
                let binding = image_binding(
                    &device,
                    &image_layout,
                    &source.create_view(&Default::default()),
                    &sampler,
                );
                let target = texture(
                    &device,
                    (7, 5),
                    format,
                    wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    "test GUI output",
                )
                .unwrap();
                let view = target.create_view(&Default::default());
                let readback = device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: 256 * 5,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                let mut encoder = device.create_command_encoder(&Default::default());
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: None,
                        color_attachments: &[Some(color_attachment(
                            &view,
                            wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        ))],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass.set_pipeline(&pipeline);
                    pass.set_bind_group(0, &binding, &[]);
                    pass.set_bind_group(1, &scale.bind_group, &[]);
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
                            rows_per_image: Some(5),
                        },
                    },
                    wgpu::Extent3d {
                        width: 7,
                        height: 5,
                        depth_or_array_layers: 1,
                    },
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
                for y in 0..5usize {
                    for x in 0..7usize {
                        assert_eq!(
                            &pixels[y * 256 + x * 4..y * 256 + x * 4 + 4],
                            &image.get_pixel((x / 3) as u32, (y / 3) as u32).0,
                            "{x},{y}"
                        );
                    }
                }
                drop(pixels);
                readback.unmap();
                if let Some(error) = errors.pop().await {
                    panic!("GPU validation failed: {error}");
                }
            }
        });
    }

    #[test]
    fn panorama_animation_uses_the_surface_acquisition_time() {
        let start = Instant::now();
        let first = animation_seconds(1.0, 1.0, start, start + Duration::from_millis(12));
        let next = animation_seconds(
            1.016,
            1.0,
            start + Duration::from_millis(16),
            start + Duration::from_millis(28),
        );
        assert!((first - 1.012).abs() < 0.000_001);
        assert!((next - first - 0.016).abs() < 0.000_001);
        assert_eq!(
            animation_seconds(3.0, 0.0, start, start + Duration::from_secs(1)),
            3.0
        );
    }

    fn test_backend() -> wgpu::Backends {
        if std::env::var("NATIVE_TEST_GPU_BACKEND").as_deref() == Ok("dx12") {
            wgpu::Backends::DX12
        } else {
            wgpu::Backends::VULKAN
        }
    }

    #[test]
    #[ignore = "performance measurement requiring a GPU and the locally installed Minecraft assets"]
    fn measure_gpu_menu_frame_cost() {
        pollster::block_on(async {
            let directory = std::env::var_os("MINECRAFT_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
            let assets = MenuAssets::load(&directory, &[]).unwrap();
            let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
            descriptor.backends = test_backend();
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
            let adapter_name = adapter.get_info().name;
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .unwrap();
            let errors = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let panorama_layout = panorama_layout(&device);
            let image_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("benchmark image layout"),
                entries: &[texture_entry(0), sampler_entry(1)],
            });
            let panorama_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("benchmark panorama"),
                source: wgpu::ShaderSource::Wgsl(include_str!("menu_panorama.wgsl").into()),
            });
            let overlay_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("benchmark overlay"),
                source: wgpu::ShaderSource::Wgsl(include_str!("overlay.wgsl").into()),
            });
            let format = wgpu::TextureFormat::Rgba8Unorm;
            let panorama = pipeline(
                &device,
                &panorama_layout,
                &panorama_shader,
                format,
                None,
                "benchmark panorama",
            );
            let overlay = pipeline(
                &device,
                &image_layout,
                &overlay_shader,
                format,
                Some(wgpu::BlendState::ALPHA_BLENDING),
                "benchmark overlay",
            );
            let sampler = panorama_sampler(&device);
            let faces = upload_panorama(&device, &queue, &assets.panorama, format).unwrap();
            let faces_view = cube_view(&faces);
            let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("benchmark params"),
                contents: bytemuck::cast_slice(&[1280.0_f32 / 720.0, 0.0, 0.0, 0.0]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            let panorama_overlay = upload_rgba(
                &device,
                &queue,
                &assets.panorama_overlay,
                format,
                "benchmark panorama overlay",
            )
            .unwrap();
            let panorama_overlay_view =
                panorama_overlay.create_view(&wgpu::TextureViewDescriptor::default());
            let face_bind = panorama_binding(
                &device,
                &panorama_layout,
                &faces_view,
                &sampler,
                &params,
                &panorama_overlay_view,
            );
            let overlay_image = RgbaImage::from_pixel(427, 240, image::Rgba([255, 255, 255, 0]));
            let overlay_texture =
                upload_rgba(&device, &queue, &overlay_image, format, "benchmark overlay").unwrap();
            let overlay_view = overlay_texture.create_view(&wgpu::TextureViewDescriptor::default());
            let overlay_bind = image_binding(&device, &image_layout, &overlay_view, &sampler);
            let output = texture(
                &device,
                (1280, 720),
                format,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
                "benchmark frame",
            )
            .unwrap();
            let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());
            let mut times = Vec::new();
            for frame in 0..130 {
                let started = std::time::Instant::now();
                let values = [1280.0_f32 / 720.0, frame as f32 * 0.0003, 0.0, 0.0];
                queue.write_buffer(&params, 0, bytemuck::cast_slice(&values));
                let mut encoder =
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("benchmark menu"),
                        color_attachments: &[Some(color_attachment(
                            &output_view,
                            wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        ))],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass.set_pipeline(&panorama);
                    pass.set_bind_group(0, &face_bind, &[]);
                    pass.draw(0..3, 0..1);
                    pass.set_pipeline(&overlay);
                    pass.set_bind_group(0, &overlay_bind, &[]);
                    pass.draw(0..6, 0..1);
                }
                let submission = queue.submit([encoder.finish()]);
                device
                    .poll(wgpu::PollType::Wait {
                        submission_index: Some(submission),
                        timeout: Some(Duration::from_secs(15)),
                    })
                    .unwrap();
                if frame >= 10 {
                    times.push(started.elapsed());
                }
            }
            if let Some(error) = errors.pop().await {
                panic!("GPU validation failed: {error}");
            }
            times.sort_unstable();
            println!(
                "offscreen 1280x720 GPU panorama + overlay on {adapter_name}: median {:?}, p95 {:?}; excludes window presentation",
                times[times.len() / 2],
                times[times.len() * 95 / 100]
            );
        });
    }

    #[test]
    #[ignore = "requires a GPU; renders offscreen without opening a window"]
    fn gpu_blur_matches_the_three_pass_default_kernel() {
        pollster::block_on(async {
            let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
            descriptor.backends = test_backend();
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
            let layout = create_blur_layout(&device);
            let radius_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("test blur radius"),
                contents: bytemuck::cast_slice(&[5_u32, 0, 0, 0]),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("test blur shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("menu_blur.wgsl").into()),
            });
            let format = wgpu::TextureFormat::Rgba8Unorm;
            let horizontal = pipeline_entry(
                &device,
                &layout,
                &shader,
                format,
                None,
                "horizontal",
                "test horizontal blur",
            );
            let vertical = pipeline_entry(
                &device,
                &layout,
                &shader,
                format,
                None,
                "vertical",
                "test vertical blur",
            );
            let source = RgbaImage::from_fn(64, 64, |x, y| {
                let red = if x < 32 { 255 } else { 0 };
                let green = if y < 32 { 220 } else { 0 };
                image::Rgba([red, green, (x * 3) as u8, 255])
            });
            let mut expected = Canvas::new(64, 64);
            for (x, y, pixel) in source.enumerate_pixels() {
                expected.pixel(x as i32, y as i32, pixel.0);
            }
            expected.blur();
            let source_texture =
                upload_rgba(&device, &queue, &source, format, "test blur source").unwrap();
            let source_view = source_texture.create_view(&wgpu::TextureViewDescriptor::default());
            let temporary = texture(
                &device,
                (64, 64),
                format,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                "test blur temporary",
            )
            .unwrap();
            let temporary_view = temporary.create_view(&wgpu::TextureViewDescriptor::default());
            let output = texture(
                &device,
                (64, 64),
                format,
                wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                "test blur output",
            )
            .unwrap();
            let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            });
            let source_bind =
                blur_binding(&device, &layout, &source_view, &sampler, &radius_buffer);
            let temporary_bind =
                blur_binding(&device, &layout, &temporary_view, &sampler, &radius_buffer);
            let output_bind =
                blur_binding(&device, &layout, &output_view, &sampler, &radius_buffer);
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("test readback"),
                size: 64 * 64 * 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            for (view, pipeline, bind) in [
                (&temporary_view, &horizontal, &source_bind),
                (&output_view, &vertical, &temporary_bind),
                (&temporary_view, &horizontal, &output_bind),
                (&output_view, &vertical, &temporary_bind),
                (&temporary_view, &horizontal, &output_bind),
                (&output_view, &vertical, &temporary_bind),
            ] {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("test blur pass"),
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
                pass.set_bind_group(0, bind, &[]);
                pass.draw(0..3, 0..1);
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
            for (index, &pixel) in expected.pixels.iter().enumerate() {
                let expected_rgb = [(pixel >> 16) as u8, (pixel >> 8) as u8, pixel as u8];
                let actual = &data[index * 4..index * 4 + 3];
                for (actual, expected) in actual.iter().zip(expected_rgb) {
                    assert!(
                        actual.abs_diff(expected) <= 6,
                        "pixel {index}: {actual} != {expected}"
                    );
                }
            }
            drop(data);
            buffer.unmap();
            if let Some(error) = errors.pop().await {
                panic!("GPU validation failed: {error}");
            }
        });
    }

    #[test]
    #[ignore = "requires a GPU; renders offscreen without opening a window"]
    fn gpu_blurred_background_composites_transparent_widgets() {
        pollster::block_on(async {
            let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
            descriptor.backends = test_backend();
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
            let panorama_layout = panorama_layout(&device);
            let image_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("test image layout"),
                entries: &[texture_entry(0), sampler_entry(1)],
            });
            let blur_layout = create_blur_layout(&device);
            let blur_radius = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("test blur radius"),
                contents: bytemuck::cast_slice(&[5_u32, 0, 0, 0]),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let panorama_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("test panorama shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("menu_panorama.wgsl").into()),
            });
            let image_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("test image shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("overlay.wgsl").into()),
            });
            let blur_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("test blur shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("menu_blur.wgsl").into()),
            });
            let nearest = device.create_sampler(&wgpu::SamplerDescriptor::default());
            let face_images =
                vec![RgbaImage::from_pixel(2, 2, image::Rgba([200, 100, 40, 255])); 6];
            let face = upload_panorama(
                &device,
                &queue,
                &face_images,
                wgpu::TextureFormat::Rgba8Unorm,
            )
            .unwrap();
            let face_view = cube_view(&face);
            let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("test parameters"),
                contents: bytemuck::cast_slice(&[1.0_f32, 0.0, 0.0, 0.0]),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let panorama_overlay = upload_rgba(
                &device,
                &queue,
                &RgbaImage::from_pixel(1, 1, image::Rgba([0, 40, 80, 128])),
                wgpu::TextureFormat::Rgba8Unorm,
                "test panorama overlay",
            )
            .unwrap();
            let panorama_overlay_view =
                panorama_overlay.create_view(&wgpu::TextureViewDescriptor::default());
            let faces = panorama_binding(
                &device,
                &panorama_layout,
                &face_view,
                &nearest,
                &params,
                &panorama_overlay_view,
            );
            let overlay = upload_rgba(
                &device,
                &queue,
                &RgbaImage::from_pixel(2, 2, image::Rgba([40, 200, 80, 128])),
                wgpu::TextureFormat::Rgba8Unorm,
                "test overlay",
            )
            .unwrap();
            let overlay_view = overlay.create_view(&wgpu::TextureViewDescriptor::default());
            let overlay_bind = image_binding(&device, &image_layout, &overlay_view, &nearest);
            for format in [
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureFormat::Bgra8Unorm,
            ] {
                let panorama = pipeline(
                    &device,
                    &panorama_layout,
                    &panorama_shader,
                    format,
                    None,
                    "test panorama",
                );
                let composite = pipeline(
                    &device,
                    &image_layout,
                    &image_shader,
                    format,
                    None,
                    "test background",
                );
                let horizontal = pipeline_entry(
                    &device,
                    &blur_layout,
                    &blur_shader,
                    format,
                    None,
                    "horizontal",
                    "test horizontal blur",
                );
                let vertical = pipeline_entry(
                    &device,
                    &blur_layout,
                    &blur_shader,
                    format,
                    None,
                    "vertical",
                    "test vertical blur",
                );
                let widgets = pipeline(
                    &device,
                    &image_layout,
                    &image_shader,
                    format,
                    Some(wgpu::BlendState::ALPHA_BLENDING),
                    "test widgets",
                );
                let blur = texture(
                    &device,
                    (16, 16),
                    format,
                    wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    "test blurred background",
                )
                .unwrap();
                let blur_view = blur.create_view(&wgpu::TextureViewDescriptor::default());
                let background_bind = image_binding(&device, &image_layout, &blur_view, &nearest);
                let blur_bind =
                    blur_binding(&device, &blur_layout, &blur_view, &nearest, &blur_radius);
                let temporary = texture(
                    &device,
                    (16, 16),
                    format,
                    wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    "test blur intermediate",
                )
                .unwrap();
                let temporary_view = temporary.create_view(&wgpu::TextureViewDescriptor::default());
                let temporary_bind = blur_binding(
                    &device,
                    &blur_layout,
                    &temporary_view,
                    &nearest,
                    &blur_radius,
                );
                let output = texture(
                    &device,
                    (64, 64),
                    format,
                    wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    "test output",
                )
                .unwrap();
                let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("test readback"),
                    size: 64 * 64 * 4,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                let mut encoder =
                    device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("test downsample"),
                        color_attachments: &[Some(color_attachment(
                            &blur_view,
                            wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        ))],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass.set_pipeline(&panorama);
                    pass.set_bind_group(0, &faces, &[]);
                    pass.draw(0..3, 0..1);
                }
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("test horizontal blur"),
                        color_attachments: &[Some(color_attachment(
                            &temporary_view,
                            wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        ))],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass.set_pipeline(&horizontal);
                    pass.set_bind_group(0, &blur_bind, &[]);
                    pass.draw(0..3, 0..1);
                }
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("test vertical blur"),
                        color_attachments: &[Some(color_attachment(
                            &blur_view,
                            wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        ))],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass.set_pipeline(&vertical);
                    pass.set_bind_group(0, &temporary_bind, &[]);
                    pass.draw(0..3, 0..1);
                }
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("test composite"),
                        color_attachments: &[Some(color_attachment(
                            &output_view,
                            wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        ))],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass.set_pipeline(&composite);
                    pass.set_bind_group(0, &background_bind, &[]);
                    pass.draw(0..6, 0..1);
                    pass.set_pipeline(&widgets);
                    pass.set_bind_group(0, &overlay_bind, &[]);
                    pass.draw(0..6, 0..1);
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
                let offset = (32 * 64 + 32) * 4;
                let pixel = &data[offset..offset + 4];
                let rgb = if format == wgpu::TextureFormat::Bgra8Unorm {
                    [pixel[2], pixel[1], pixel[0]]
                } else {
                    [pixel[0], pixel[1], pixel[2]]
                };
                for (actual, expected) in rgb.into_iter().zip([70_u8, 135, 70]) {
                    assert!(actual.abs_diff(expected) <= 2, "{format:?}: got {rgb:?}");
                }
                assert_eq!(pixel[3], 255);
                drop(data);
                buffer.unmap();
            }
            if let Some(error) = errors.pop().await {
                panic!("GPU validation failed: {error}");
            }
        });
    }

    #[test]
    #[ignore = "requires a GPU; renders offscreen without opening a window"]
    fn gpu_panorama_matches_cube_face_orientation() {
        pollster::block_on(async {
            let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
            descriptor.backends = test_backend();
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
            let layout = panorama_layout(&device);
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("test panorama"),
                source: wgpu::ShaderSource::Wgsl(include_str!("menu_panorama.wgsl").into()),
            });
            let pipeline = pipeline(
                &device,
                &layout,
                &shader,
                wgpu::TextureFormat::Rgba8Unorm,
                None,
                "test panorama",
            );
            let sampler = panorama_sampler(&device);
            let colors = [
                [255, 0, 0, 255],
                [0, 255, 0, 255],
                [0, 0, 255, 255],
                [255, 255, 0, 255],
                [255, 0, 255, 255],
                [0, 255, 255, 255],
            ];
            let face_images: Vec<_> = colors
                .iter()
                .enumerate()
                .map(|(index, &color)| {
                    let mut image = RgbaImage::from_pixel(16, 16, image::Rgba(color));
                    if index == 0 {
                        for y in 0..16 {
                            for x in 0..8 {
                                image.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
                            }
                        }
                    }
                    image
                })
                .collect();
            let panorama_texture = upload_panorama(
                &device,
                &queue,
                &face_images,
                wgpu::TextureFormat::Rgba8Unorm,
            )
            .unwrap();
            let faces_view = cube_view(&panorama_texture);
            let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("test params"),
                contents: bytemuck::cast_slice(&[2.0_f32, 0.0, 1.0, 0.0]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            let overlay_image = RgbaImage::from_pixel(1, 1, image::Rgba([255, 255, 255, 0]));
            let panorama_overlay = upload_rgba(
                &device,
                &queue,
                &overlay_image,
                wgpu::TextureFormat::Rgba8Unorm,
                "test transparent panorama overlay",
            )
            .unwrap();
            let panorama_overlay_view =
                panorama_overlay.create_view(&wgpu::TextureViewDescriptor::default());
            let bind = panorama_binding(
                &device,
                &layout,
                &faces_view,
                &sampler,
                &params,
                &panorama_overlay_view,
            );
            let output = texture(
                &device,
                (128, 64),
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                "test output",
            )
            .unwrap();
            let view = output.create_view(&wgpu::TextureViewDescriptor::default());
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("test readback"),
                size: 128 * 64 * 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("test render"),
                    color_attachments: &[Some(color_attachment(
                        &view,
                        wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    ))],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.draw(0..3, 0..1);
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
                        bytes_per_row: Some(512),
                        rows_per_image: Some(64),
                    },
                },
                wgpu::Extent3d {
                    width: 128,
                    height: 64,
                    depth_or_array_layers: 1,
                },
            );
            queue.submit([encoder.finish()]);
            if let Some(error) = errors.pop().await {
                panic!("GPU validation failed: {error}");
            }
            let (tx, rx) = mpsc::channel();
            buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let _ = tx.send(result);
                });
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: Some(Duration::from_secs(15)),
                })
                .unwrap();
            rx.recv_timeout(Duration::from_secs(15)).unwrap().unwrap();
            let data = buffer.slice(..).get_mapped_range().unwrap();
            let pixel = |x: usize, y: usize| -> [u8; 4] {
                let offset = (y * 128 + x) * 4;
                data[offset..offset + 4].try_into().unwrap()
            };
            let center = pixel(64, 32);
            assert!((150..=165).contains(&center[0]), "center = {center:?}");
            assert_eq!(&center[1..], &[0, 0, 255]);
            assert_eq!(pixel(1, 32), [255, 255, 0, 255]);
            assert_eq!(pixel(126, 32), [0, 255, 0, 255]);
            let seam = pixel(98, 32);
            assert!(seam[0] > 30 && seam[1] > 30, "seam = {seam:?}");
            let mut software = crate::menu_draw::Canvas::new(128, 64);
            software.panorama(&face_images, &overlay_image, 0.0);
            for (x, y) in [(1, 32), (10, 10), (32, 20), (64, 32), (80, 40), (126, 32)] {
                let rgb = software.pixels[y * 128 + x];
                let expected = [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255];
                let actual = pixel(x, y);
                for (a, e) in actual.into_iter().zip(expected) {
                    assert!(
                        a.abs_diff(e) <= 2,
                        "pixel {x},{y}: {actual:?} != {expected:?}"
                    );
                }
            }
            drop(data);
            buffer.unmap();
        });
    }
}
