//! Render the menu away from the window event loop. Presenting a vsynced
//! surface can block, so only the most recent requested frame is retained.

use std::{
    sync::{Arc, Condvar, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::Instant,
};

use anyhow::{Context, Result, anyhow};
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use winit::dpi::PhysicalSize;

use crate::{menu_assets::MenuAssets, menu_draw::Canvas, menu_renderer::MenuRenderer};

struct SurfaceRequest {
    instance: wgpu::Instance,
    reply: mpsc::Sender<Result<wgpu::Surface<'static>>>,
}

/// Surface creation needs the Windows event-loop thread, whereas adapter and
/// device initialization can take place on the worker.
#[derive(Clone)]
pub(crate) struct SurfaceRequester {
    requests: mpsc::Sender<SurfaceRequest>,
    window: Arc<winit::window::Window>,
}

impl SurfaceRequester {
    pub fn create(&self, instance: &wgpu::Instance) -> Result<wgpu::Surface<'static>> {
        let (reply, result) = mpsc::channel();
        self.requests
            .send(SurfaceRequest {
                instance: instance.clone(),
                reply,
            })
            .context("menu window is no longer available")?;
        self.window.request_redraw();
        result
            .recv()
            .context("menu window closed during surface creation")?
    }
}

pub struct MenuOverlay {
    pub assets: Option<Arc<MenuAssets>>,
    pub canvas: Canvas,
}

pub struct MenuFrame {
    pub seconds: f32,
    pub speed: f32,
    pub requested_at: Instant,
    pub blurred: bool,
    pub blur_radius: u8,
    pub vsync: bool,
    pub size: PhysicalSize<u32>,
    pub gui_scale: u32,
    pub overlay: Option<MenuOverlay>,
    pub screenshots: Vec<crate::screenshot::ScreenshotRequest>,
}

#[derive(Default)]
struct MailboxState {
    latest: Option<MenuFrame>,
    closed: bool,
}

#[derive(Default)]
struct FrameMailbox {
    state: Mutex<MailboxState>,
    changed: Condvar,
}

impl FrameMailbox {
    fn submit(&self, mut frame: MenuFrame) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.closed {
            for request in frame.screenshots.drain(..) {
                request
                    .sender
                    .report_error(anyhow!("menu renderer stopped before taking screenshot"));
            }
            return false;
        }
        // A newer animation frame must retain an unconsumed overlay update.
        // A newer overlay supersedes the old one.
        if frame.overlay.is_none() {
            frame.overlay = state.latest.as_mut().and_then(|old| old.overlay.take());
        }
        if let Some(old) = &mut state.latest {
            let mut pending = std::mem::take(&mut old.screenshots);
            pending.append(&mut frame.screenshots);
            frame.screenshots = pending;
        }
        if frame.screenshots.len() > 4 {
            for request in frame.screenshots.drain(4..) {
                request
                    .sender
                    .report_error(anyhow!("screenshot queue is full"));
            }
        }
        state.latest = Some(frame);
        self.changed.notify_one();
        true
    }

    fn receive(&self) -> Option<MenuFrame> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        while !state.closed && state.latest.is_none() {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
        if state.closed {
            None
        } else {
            state.latest.take()
        }
    }

    fn close(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.closed = true;
        if let Some(frame) = &mut state.latest {
            for request in frame.screenshots.drain(..) {
                request
                    .sender
                    .report_error(anyhow!("menu renderer stopped before taking screenshot"));
            }
        }
        state.latest = None;
        self.changed.notify_one();
    }
}

pub struct MenuGpuWorker {
    mailbox: Arc<FrameMailbox>,
    errors: mpsc::Receiver<anyhow::Error>,
    surfaces: mpsc::Receiver<SurfaceRequest>,
    thread: Option<JoinHandle<()>>,
    #[cfg(test)]
    completed: Arc<AtomicUsize>,
}

impl MenuGpuWorker {
    /// GPU and swapchain initialization may wait on a driver. Start it on the
    /// renderer thread so the window event loop can still process input.
    pub fn new(window: Arc<winit::window::Window>, vsync: bool) -> Result<Self> {
        Self::new_with_initialization(window, vsync, || Ok(()))
    }

    pub(crate) fn new_with_initialization(
        window: Arc<winit::window::Window>,
        vsync: bool,
        initialize: impl FnOnce() -> Result<()> + Send + 'static,
    ) -> Result<Self> {
        let mailbox = Arc::new(FrameMailbox::default());
        let shared = Arc::clone(&mailbox);
        let (sender, errors) = mpsc::channel();
        let (requests, surfaces) = mpsc::channel();
        let surface_requester = SurfaceRequester {
            requests,
            window: Arc::clone(&window),
        };
        #[cfg(test)]
        let completed = Arc::new(AtomicUsize::new(0));
        #[cfg(test)]
        let worker_completed = Arc::clone(&completed);
        let thread = thread::Builder::new()
            .name("menu-renderer".into())
            .spawn(move || {
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
                        initialize()?;
                        let mut renderer =
                            MenuRenderer::new(Arc::clone(&window), vsync, surface_requester)?;
                        // Wake a menu that is otherwise waiting while resources load.
                        window.request_redraw();
                        while let Some(frame) = shared.receive() {
                            renderer.request_screenshots(frame.screenshots);
                            renderer.resize(frame.size);
                            renderer.set_vsync(frame.vsync);
                            renderer.set_gui_scale(frame.gui_scale);
                            if let Some(overlay) = frame.overlay {
                                renderer.set_assets(overlay.assets.as_ref())?;
                                renderer.update_overlay(&overlay.canvas)?;
                            }
                            let _presented = renderer.render(
                                frame.seconds,
                                frame.speed,
                                frame.requested_at,
                                frame.blurred,
                                frame.blur_radius,
                            )?;
                            if renderer.has_pending_screenshots() {
                                // A minimized or temporarily unavailable surface can reject frames.
                                // Retry off the UI thread without a tight redraw loop.
                                if !_presented {
                                    std::thread::sleep(std::time::Duration::from_millis(100));
                                }
                                window.request_redraw();
                            }
                            #[cfg(test)]
                            if _presented {
                                worker_completed.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        Ok(())
                    }));
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        let _ = sender.send(error);
                    }
                    Err(_) => {
                        let _ = sender.send(anyhow!("menu renderer panicked"));
                    }
                }
                shared.close();
                window.request_redraw();
            })?;
        Ok(Self {
            mailbox,
            errors,
            surfaces,
            thread: Some(thread),
            #[cfg(test)]
            completed,
        })
    }

    pub fn submit(&self, frame: MenuFrame) -> Result<()> {
        if self.mailbox.submit(frame) {
            Ok(())
        } else {
            Err(self
                .take_error()
                .unwrap_or_else(|| anyhow!("menu renderer stopped")))
        }
    }

    pub fn take_error(&self) -> Option<anyhow::Error> {
        match self.errors.try_recv() {
            Ok(error) => Some(error),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(anyhow!("menu renderer stopped unexpectedly"))
            }
        }
    }

    /// Service only the handle-bound portion on the window's owner thread.
    pub fn service_surfaces(&self, window: &Arc<winit::window::Window>) {
        while let Ok(request) = self.surfaces.try_recv() {
            let result = request
                .instance
                .create_surface(Arc::clone(window))
                .context("cannot create menu surface");
            let _ = request.reply.send(result);
        }
    }

    #[cfg(test)]
    pub(crate) fn completed_frames(&self) -> usize {
        self.completed.load(Ordering::Relaxed)
    }
}

impl Drop for MenuGpuWorker {
    fn drop(&mut self) {
        self.mailbox.close();
        if let Some(thread) = self.thread.take() {
            // A driver can block surface acquisition or initialization for an
            // unbounded time. Do not make the window thread wait for it.
            if thread.is_finished() {
                let _ = thread.join();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(seconds: f32, marker: Option<u32>) -> MenuFrame {
        MenuFrame {
            seconds,
            speed: 1.0,
            requested_at: Instant::now(),
            blurred: seconds > 2.0,
            blur_radius: 5,
            vsync: true,
            size: PhysicalSize::new(800 + seconds as u32, 600),
            gui_scale: 1,
            overlay: marker.map(|marker| {
                let mut canvas = Canvas::new(1, 1);
                canvas.pixels[0] = marker;
                MenuOverlay {
                    assets: None,
                    canvas,
                }
            }),
            screenshots: Vec::new(),
        }
    }

    #[test]
    fn pending_frames_coalesce_without_losing_the_latest_overlay() {
        let mailbox = FrameMailbox::default();
        assert!(mailbox.submit(frame(1.0, Some(1))));
        assert!(mailbox.submit(frame(2.0, None)));
        let pending = mailbox.receive().unwrap();
        assert_eq!(pending.seconds, 2.0);
        assert_eq!(pending.size, PhysicalSize::new(802, 600));
        assert_eq!(pending.overlay.unwrap().canvas.pixels[0], 1);
        assert!(mailbox.submit(frame(3.0, Some(2))));
        assert!(mailbox.submit(frame(4.0, Some(3))));
        assert!(mailbox.submit(frame(5.0, None)));
        let pending = mailbox.receive().unwrap();
        assert_eq!(pending.seconds, 5.0);
        assert!(pending.blurred);
        assert_eq!(pending.blur_radius, 5);
        assert_eq!(pending.overlay.unwrap().canvas.pixels[0], 3);
        mailbox.close();
        assert!(mailbox.receive().is_none());
        assert!(!mailbox.submit(frame(6.0, None)));
    }

    #[test]
    fn screenshot_requests_survive_frame_coalescing_and_report_overflow() {
        let writer = crate::screenshot::Screenshots::new().unwrap();
        let mailbox = FrameMailbox::default();
        for index in 0..6 {
            let mut next = frame(index as f32, None);
            next.screenshots.push(crate::screenshot::ScreenshotRequest {
                directory: format!("capture-{index}").into(),
                sender: writer.sender(),
            });
            assert!(mailbox.submit(next));
        }
        let pending = mailbox.receive().unwrap();
        let directories: Vec<_> = pending
            .screenshots
            .iter()
            .map(|request| request.directory.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            directories,
            ["capture-0", "capture-1", "capture-2", "capture-3"]
        );
        assert!(writer.take_result().unwrap().is_err());
        assert!(writer.take_result().unwrap().is_err());
        assert!(writer.take_result().is_none());
    }

    #[test]
    fn event_loop_can_publish_while_the_renderer_is_busy() {
        let mailbox = Arc::new(FrameMailbox::default());
        let worker_mailbox = Arc::clone(&mailbox);
        let (started, started_rx) = mpsc::channel();
        let (finish, finish_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let first = worker_mailbox.receive().unwrap();
            started.send(first.seconds).unwrap();
            finish_rx.recv().unwrap();
            worker_mailbox.receive().unwrap()
        });
        assert!(mailbox.submit(frame(1.0, None)));
        assert_eq!(
            started_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap(),
            1.0
        );
        for index in 2..=100 {
            assert!(mailbox.submit(frame(index as f32, (index == 2).then_some(7))));
        }
        finish.send(()).unwrap();
        let latest = worker.join().unwrap();
        assert_eq!(latest.seconds, 100.0);
        assert_eq!(latest.overlay.unwrap().canvas.pixels[0], 7);
    }

    #[test]
    fn dropping_worker_does_not_wait_for_a_stalled_renderer() {
        use std::time::Duration;
        let mailbox = Arc::new(FrameMailbox::default());
        let (started, started_rx) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let (finished, finished_rx) = mpsc::channel();
        let renderer = thread::spawn(move || {
            started.send(()).unwrap();
            release_rx.recv().unwrap();
            finished.send(()).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let (_error_sender, errors) = mpsc::channel();
        let (_surface_sender, surfaces) = mpsc::channel();
        let worker = MenuGpuWorker {
            mailbox: Arc::clone(&mailbox),
            errors,
            surfaces,
            thread: Some(renderer),
            completed: Arc::new(AtomicUsize::new(0)),
        };
        let (dropped, dropped_rx) = mpsc::channel();
        let dropper = thread::spawn(move || {
            drop(worker);
            dropped.send(()).unwrap();
        });
        let was_responsive = dropped_rx.recv_timeout(Duration::from_secs(2)).is_ok();
        release.send(()).unwrap();
        finished_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        dropper.join().unwrap();
        assert!(was_responsive, "worker drop blocked on the renderer thread");
        assert!(!mailbox.submit(frame(1.0, None)));
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires a Windows desktop and GPU; briefly opens a test window"]
    fn real_window_renders_resizes_and_shuts_down() {
        if !crate::window_test::isolated(
            "menu_gpu_worker::tests::real_window_renders_resizes_and_shuts_down",
        ) {
            return;
        }
        use std::time::{Duration, Instant};
        use winit::{
            application::ApplicationHandler,
            event::WindowEvent,
            event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
            platform::windows::EventLoopBuilderExtWindows,
            window::{Window, WindowId},
        };

        struct TestApp {
            window: Option<Arc<Window>>,
            worker: Option<MenuGpuWorker>,
            start: Instant,
            sent: usize,
            completed: usize,
            completed_before_resize: usize,
            resized: bool,
            init_release: Option<mpsc::Sender<()>>,
            frames_while_initializing: usize,
            error: Option<String>,
        }

        impl ApplicationHandler for TestApp {
            fn resumed(&mut self, event_loop: &ActiveEventLoop) {
                let result = (|| -> Result<_> {
                    let window = Arc::new(
                        event_loop.create_window(
                            Window::default_attributes()
                                .with_title("Minecraft menu renderer test")
                                .with_inner_size(PhysicalSize::new(640, 360)),
                        )?,
                    );
                    let (release, wait) = mpsc::channel();
                    let worker = MenuGpuWorker::new_with_initialization(
                        Arc::clone(&window),
                        true,
                        move || {
                            let _ = wait.recv_timeout(Duration::from_secs(3));
                            Ok(())
                        },
                    )?;
                    Ok((window, worker, release))
                })();
                match result {
                    Ok((window, worker, release)) => {
                        self.window = Some(window);
                        self.worker = Some(worker);
                        self.init_release = Some(release);
                        self.start = Instant::now();
                    }
                    Err(error) => {
                        self.error = Some(format!("{error:#}"));
                        event_loop.exit();
                    }
                }
            }

            fn window_event(
                &mut self,
                event_loop: &ActiveEventLoop,
                _: WindowId,
                event: WindowEvent,
            ) {
                if matches!(event, WindowEvent::CloseRequested) {
                    self.error = Some("test window closed before completion".into());
                    event_loop.exit();
                }
            }

            fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
                let Some(worker) = &self.worker else { return };
                let window = self.window.as_ref().unwrap();
                worker.service_surfaces(window);
                if let Some(error) = worker.take_error() {
                    self.error = Some(format!("{error:#}"));
                    event_loop.exit();
                    return;
                }
                let elapsed = self.start.elapsed();
                if elapsed >= Duration::from_millis(300)
                    && let Some(release) = self.init_release.take()
                {
                    self.frames_while_initializing = self.sent;
                    assert_eq!(worker.completed_frames(), 0);
                    let _ = release.send(());
                }
                if elapsed >= Duration::from_secs(4) {
                    self.completed = worker.completed_frames();
                    self.worker.take();
                    event_loop.exit();
                    return;
                }
                if elapsed >= Duration::from_millis(1500) && !self.resized {
                    self.completed_before_resize = worker.completed_frames();
                    let _ = window.request_inner_size(PhysicalSize::new(800, 480));
                    self.resized = true;
                }
                let overlay = (self.sent == 0 || self.sent == 40).then(|| {
                    let mut canvas = Canvas::new(320, 180);
                    canvas.fill(
                        crate::menu_draw::Rect {
                            x: 0,
                            y: 0,
                            w: 80,
                            h: 30,
                        },
                        [255, 255, 255, 255],
                    );
                    MenuOverlay {
                        assets: None,
                        canvas,
                    }
                });
                if let Err(error) = worker.submit(MenuFrame {
                    seconds: elapsed.as_secs_f32(),
                    speed: 1.0,
                    requested_at: Instant::now(),
                    blurred: self.resized,
                    blur_radius: 5,
                    vsync: !self.resized,
                    size: window.inner_size(),
                    gui_scale: 1,
                    overlay,
                    screenshots: Vec::new(),
                }) {
                    self.error = Some(format!("{error:#}"));
                    event_loop.exit();
                    return;
                }
                self.sent += 1;
                event_loop.set_control_flow(ControlFlow::WaitUntil(
                    Instant::now() + Duration::from_millis(16),
                ));
            }
        }

        let mut builder = EventLoop::builder();
        builder.with_any_thread(true);
        let event_loop = builder.build().unwrap();
        let mut app = TestApp {
            window: None,
            worker: None,
            start: Instant::now(),
            sent: 0,
            completed: 0,
            completed_before_resize: 0,
            resized: false,
            init_release: None,
            frames_while_initializing: 0,
            error: None,
        };
        event_loop.run_app(&mut app).unwrap();
        assert!(app.error.is_none(), "menu worker failed: {:?}", app.error);
        assert!(
            app.frames_while_initializing > 5
                && app.resized
                && app.sent > 10
                && app.completed_before_resize > 5
                && app.completed > app.completed_before_resize + 5,
            "sent={}, sent during init={}, completed before resize={}, completed={}, resized={}",
            app.sent,
            app.frames_while_initializing,
            app.completed_before_resize,
            app.completed,
            app.resized
        );
    }
}
