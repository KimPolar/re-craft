//! CPU menu composition runs away from the window event loop. Only the latest
//! request and completed image are kept while the compositor is busy.

use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

use anyhow::{Result, anyhow};
use winit::window::Window;

use crate::{menu_assets::MenuAssets, menu_draw::Canvas};

pub(crate) struct SoftwareFrame {
    pub size: (u32, u32),
    pub scale: u32,
    pub generation: u64,
    pub seconds: f32,
    pub blurred: bool,
    pub blur_radius: u8,
    pub assets: Option<Arc<MenuAssets>>,
    pub overlay: Arc<Canvas>,
}

pub(crate) struct SoftwareImage {
    pub size: (u32, u32),
    pub generation: u64,
    pub pixels: Vec<u32>,
}

#[derive(Default)]
struct State {
    pending: Option<SoftwareFrame>,
    completed: Option<SoftwareImage>,
    closed: bool,
    failed: bool,
}

#[derive(Default)]
struct Mailbox {
    state: Mutex<State>,
    changed: Condvar,
}

impl Mailbox {
    fn submit(&self, frame: SoftwareFrame) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed {
            return false;
        }
        state.pending = Some(frame);
        self.changed.notify_one();
        true
    }

    fn receive(&self) -> Option<SoftwareFrame> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        while !state.closed && state.pending.is_none() {
            state = self.changed.wait(state).unwrap_or_else(|e| e.into_inner());
        }
        if state.closed {
            None
        } else {
            state.pending.take()
        }
    }

    fn complete(&self, image: SoftwareImage) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed {
            return false;
        }
        state.completed = Some(image);
        true
    }

    fn latest_overlay(&self, generation: u64) -> Option<Arc<Canvas>> {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state
            .pending
            .as_ref()
            .filter(|frame| frame.generation == generation)
            .map(|frame| Arc::clone(&frame.overlay))
    }

    fn take(&self) -> Result<Option<SoftwareImage>> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.failed {
            return Err(anyhow!("software menu renderer stopped unexpectedly"));
        }
        Ok(state.completed.take())
    }

    fn close(&self, failed: bool) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.closed = true;
        state.failed = failed;
        state.pending = None;
        self.changed.notify_one();
    }
}

pub(crate) struct MenuSoftwareWorker {
    mailbox: Arc<Mailbox>,
    thread: Option<JoinHandle<()>>,
}

impl MenuSoftwareWorker {
    pub fn new(window: Arc<Window>) -> Result<Self> {
        let mailbox = Arc::new(Mailbox::default());
        let shared = Arc::clone(&mailbox);
        let thread = thread::Builder::new()
            .name("menu-software-renderer".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(|| {
                    while let Some(frame) = shared.receive() {
                        let image = compose(frame, Some(&shared));
                        if !shared.complete(image) {
                            break;
                        }
                        window.request_redraw();
                    }
                });
                if result.is_err() {
                    shared.close(true);
                    window.request_redraw();
                }
            })?;
        Ok(Self {
            mailbox,
            thread: Some(thread),
        })
    }

    pub fn submit(&self, frame: SoftwareFrame) -> Result<()> {
        if self.mailbox.submit(frame) {
            Ok(())
        } else {
            Err(anyhow!("software menu renderer stopped"))
        }
    }

    pub fn take(&self) -> Result<Option<SoftwareImage>> {
        self.mailbox.take()
    }
}

impl Drop for MenuSoftwareWorker {
    fn drop(&mut self) {
        self.mailbox.close(false);
        if let Some(thread) = self.thread.take()
            && thread.is_finished()
        {
            let _ = thread.join();
        }
    }
}

fn compose(frame: SoftwareFrame, mailbox: Option<&Mailbox>) -> SoftwareImage {
    let mut canvas = Canvas::new(frame.size.0, frame.size.1);
    if let Some(assets) = &frame.assets {
        canvas.panorama(&assets.panorama, &assets.panorama_overlay, frame.seconds);
        if frame.blurred {
            canvas.blur_radius(frame.blur_radius);
        }
    }
    // Pick up the newest controls after the expensive panorama and blur work.
    // A different scene must be rendered separately, never composited here.
    let overlay = mailbox
        .and_then(|mailbox| mailbox.latest_overlay(frame.generation))
        .unwrap_or(frame.overlay);
    composite_scaled(&mut canvas, &overlay, frame.scale);
    SoftwareImage {
        size: frame.size,
        generation: frame.generation,
        pixels: canvas.pixels,
    }
}

fn composite_scaled(background: &mut Canvas, overlay: &Canvas, scale: u32) {
    debug_assert!(scale > 0 && overlay.width > 0 && overlay.height > 0);
    let source_width = overlay.width as usize;
    let source_height = overlay.height as usize;
    for (y, row) in background
        .pixels
        .chunks_mut(background.width as usize)
        .enumerate()
    {
        let source_y = (y / scale as usize).min(source_height - 1);
        let source = &overlay.pixels[source_y * source_width..(source_y + 1) * source_width];
        for (x, pixels) in row.chunks_mut(scale as usize).enumerate() {
            let fg = source[x.min(source_width - 1)];
            let alpha = fg >> 24;
            if alpha == 0 {
                continue;
            }
            if alpha == 255 {
                pixels.fill(fg);
                continue;
            }
            for bg in pixels {
                let blend = |shift: u32| -> u32 {
                    ((((fg >> shift) & 255_u32) * alpha
                        + ((*bg >> shift) & 255_u32) * (255 - alpha))
                        / 255)
                        << shift
                };
                *bg = 0xff00_0000 | blend(16) | blend(8) | blend(0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(generation: u64) -> SoftwareFrame {
        SoftwareFrame {
            size: (7, 5),
            scale: 3,
            generation,
            seconds: generation as f32,
            blurred: false,
            blur_radius: 5,
            assets: None,
            overlay: Arc::new(Canvas::new(2, 1)),
        }
    }

    #[test]
    fn ceil_sized_overlay_uses_partial_last_pixels() {
        let mut overlay = Canvas::new(3, 2);
        overlay.pixels = (0..6).map(|pixel| 0xff00_0000 | pixel).collect();
        let mut background = Canvas::new(7, 5);
        composite_scaled(&mut background, &overlay, 3);
        for y in 0..5usize {
            for x in 0..7usize {
                assert_eq!(
                    background.pixels[y * 7 + x],
                    overlay.pixels[(y / 3) * 3 + x / 3]
                );
            }
        }
    }

    #[test]
    fn overlay_composition_matches_canvas_blending_including_partial_edges() {
        for alpha in [0, 1, 64, 128, 254, 255] {
            let mut overlay = Canvas::new(2, 1);
            overlay.pixels = vec![(alpha << 24) | 0x00c4_6181, 0xffa0_b0c0];
            let mut actual = Canvas::new(7, 5);
            actual.pixels.fill(0xff12_3456);
            let mut expected = Canvas::new(7, 5);
            expected.pixels.fill(0xff12_3456);
            for y in 0..5 {
                for x in 0..7 {
                    let fg = overlay.pixels[((x / 3) as usize).min(1)];
                    expected.pixel(
                        x,
                        y,
                        [
                            (fg >> 16) as u8,
                            (fg >> 8) as u8,
                            fg as u8,
                            (fg >> 24) as u8,
                        ],
                    );
                }
            }
            composite_scaled(&mut actual, &overlay, 3);
            assert_eq!(actual.pixels, expected.pixels, "alpha {alpha}");
        }
    }

    #[test]
    fn busy_worker_keeps_only_latest_request_and_completed_image() {
        let mailbox = Mailbox::default();
        assert!(mailbox.submit(frame(1)));
        assert_eq!(mailbox.receive().unwrap().generation, 1);
        for generation in 2..=100 {
            assert!(mailbox.submit(frame(generation)));
        }
        assert_eq!(mailbox.receive().unwrap().generation, 100);
        assert!(mailbox.complete(compose(frame(1), None)));
        assert!(mailbox.complete(compose(frame(100), None)));
        let image = mailbox.take().unwrap().unwrap();
        assert_eq!(image.generation, 100);
        assert_eq!(image.size, (7, 5));
        assert_eq!(image.pixels.len(), 35);
        assert!(mailbox.take().unwrap().is_none());
        mailbox.close(false);
        assert!(!mailbox.submit(frame(101)));
        assert!(mailbox.receive().is_none());
    }

    #[test]
    fn worker_failure_is_reported_to_the_event_loop() {
        let mailbox = Mailbox::default();
        mailbox.close(true);
        assert!(mailbox.take().is_err());
        assert!(!mailbox.submit(frame(1)));
    }

    #[test]
    fn busy_compositor_uses_latest_overlay_only_for_the_same_scene() {
        let mailbox = Mailbox::default();
        let mut current = frame(7);
        let mut old = Canvas::new(2, 1);
        old.pixels[0] = 0xff12_3456;
        current.overlay = Arc::new(old);
        let mut newer = frame(7);
        let mut overlay = Canvas::new(2, 1);
        overlay.pixels[0] = 0xffab_cdef;
        newer.overlay = Arc::new(overlay);
        assert!(mailbox.submit(newer));
        assert_eq!(compose(current, Some(&mailbox)).pixels[0], 0xffab_cdef);

        let mut old_scene = frame(7);
        let mut old = Canvas::new(2, 1);
        old.pixels[0] = 0xff12_3456;
        old_scene.overlay = Arc::new(old);
        let mut other_scene = frame(8);
        let mut overlay = Canvas::new(2, 1);
        overlay.pixels[0] = 0xffab_cdef;
        other_scene.overlay = Arc::new(overlay);
        assert!(mailbox.submit(other_scene));
        assert_eq!(compose(old_scene, Some(&mailbox)).pixels[0], 0xff12_3456);
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn panorama_and_blur_are_composed_at_physical_resolution() {
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&directory, &[]).unwrap());
        let mut overlay = Canvas::new(320, 240);
        overlay.pixel(9, 11, [200, 70, 90, 128]);
        let overlay = Arc::new(overlay);
        let frame = SoftwareFrame {
            size: (641, 481),
            scale: 2,
            generation: 17,
            seconds: 2.0,
            blurred: true,
            blur_radius: 5,
            assets: Some(Arc::clone(&assets)),
            overlay,
        };
        let actual = compose(frame, None);
        let mut expected = Canvas::new(641, 481);
        expected.panorama(&assets.panorama, &assets.panorama_overlay, 2.0);
        expected.blur_radius(5);
        for y in 22..24 {
            for x in 18..20 {
                expected.pixel(x, y, [200, 70, 90, 128]);
            }
        }
        assert_eq!(actual.pixels, expected.pixels);
        assert_eq!(actual.size, (641, 481));
        assert_eq!(actual.generation, 17);
    }
}
