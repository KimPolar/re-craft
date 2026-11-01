//! Asynchronous PNG encoding and GPU readback for screenshots.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use image::ImageEncoder;

/// Results from menu captures must outlive the menu window while a world opens.
pub struct MenuScreenshots {
    writer: Screenshots,
    pending: std::cell::RefCell<std::collections::VecDeque<(Result<PathBuf>, std::time::Instant)>>,
    window: Arc<std::sync::Mutex<std::sync::Weak<winit::window::Window>>>,
}

impl MenuScreenshots {
    pub fn new() -> Result<Self> {
        let window = Arc::new(std::sync::Mutex::new(std::sync::Weak::<
            winit::window::Window,
        >::new()));
        let wake_window = Arc::clone(&window);
        let writer = Screenshots::new_with_waker(move || {
            if let Some(window) = wake_window
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .upgrade()
            {
                window.request_redraw();
            }
        })?;
        Ok(Self {
            writer,
            pending: std::cell::RefCell::new(std::collections::VecDeque::new()),
            window,
        })
    }

    pub fn set_window(&self, window: &Arc<winit::window::Window>) {
        *self.window.lock().unwrap_or_else(|e| e.into_inner()) = Arc::downgrade(window);
    }

    pub fn sender(&self) -> ScreenshotSender {
        self.writer.sender()
    }

    pub fn collect(&self) {
        let mut pending = self.pending.borrow_mut();
        while let Some((result, completed)) = self.writer.take_timed_result() {
            if pending.len() == 100 {
                pending.pop_front();
            }
            pending.push_back((result, completed));
        }
    }

    pub fn take(&self) -> Option<(Result<PathBuf>, std::time::Duration)> {
        self.collect();
        self.pending
            .borrow_mut()
            .pop_front()
            .map(|(result, at)| (result, at.elapsed()))
    }
}

/// Bounded queue: screenshot processing must never stall the render thread or
/// consume an unbounded amount of GPU and CPU memory.
pub struct Screenshots {
    sender: ScreenshotSender,
    results: Receiver<(Result<PathBuf>, std::time::Instant)>,
}

#[derive(Clone)]
pub struct ScreenshotSender {
    jobs: SyncSender<Job>,
    feedback: mpsc::Sender<(Result<PathBuf>, std::time::Instant)>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

pub struct ScreenshotRequest {
    pub directory: PathBuf,
    pub sender: ScreenshotSender,
}

struct Job {
    source: Source,
    directory: PathBuf,
    timestamp: String,
    width: u32,
    height: u32,
}

enum Source {
    Gpu {
        device: wgpu::Device,
        buffer: wgpu::Buffer,
        submission: wgpu::SubmissionIndex,
        stride: u32,
        bgra: bool,
    },
    Software(Vec<u32>),
}

pub struct Readback {
    buffer: wgpu::Buffer,
    width: u32,
    height: u32,
    stride: u32,
    bgra: bool,
}

impl Readback {
    /// Record a copy of the final, fully composited render target.
    pub fn encode(
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        texture: &wgpu::Texture,
        size: (u32, u32),
        format: wgpu::TextureFormat,
    ) -> Result<Self> {
        let bgra = match format {
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => true,
            wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => false,
            _ => bail!("unsupported screenshot surface format: {format:?}"),
        };
        let (width, height) = size;
        ensure!(width > 0 && height > 0, "screenshot has no pixels");
        let row = width.checked_mul(4).context("screenshot row too wide")?;
        let stride = row
            .checked_add(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT - 1)
            .context("screenshot row too wide")?
            / wgpu::COPY_BYTES_PER_ROW_ALIGNMENT
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let bytes = u64::from(stride) * u64::from(height);
        ensure!(
            bytes <= device.limits().max_buffer_size,
            "screenshot exceeds GPU buffer limit"
        );
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screenshot readback"),
            size: bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        Ok(Self {
            buffer,
            width,
            height,
            stride,
            bgra,
        })
    }
}

impl Screenshots {
    pub fn new() -> Result<Self> {
        Self::new_with_waker(|| {})
    }

    pub fn new_with_waker(wake: impl Fn() + Send + Sync + 'static) -> Result<Self> {
        let (jobs, receiver) = mpsc::sync_channel::<Job>(3);
        let (send_result, results) = mpsc::channel();
        let feedback = send_result.clone();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let worker_wake = Arc::clone(&wake);
        thread::Builder::new()
            .name("screenshot-writer".into())
            .spawn(move || {
                while let Ok(job) = receiver.recv() {
                    let result = job.run();
                    let _ = send_result.send((result, std::time::Instant::now()));
                    worker_wake();
                }
            })
            .context("cannot start screenshot writer")?;
        Ok(Self {
            sender: ScreenshotSender {
                jobs,
                feedback,
                wake,
            },
            results,
        })
    }

    pub fn sender(&self) -> ScreenshotSender {
        self.sender.clone()
    }

    pub fn submit(
        &self,
        device: &wgpu::Device,
        readback: Readback,
        submission: wgpu::SubmissionIndex,
        minecraft_dir: &Path,
    ) -> Result<()> {
        self.sender
            .submit(device, readback, submission, minecraft_dir)
    }

    pub fn take_result(&self) -> Option<Result<PathBuf>> {
        self.take_timed_result().map(|(result, _)| result)
    }

    fn take_timed_result(&self) -> Option<(Result<PathBuf>, std::time::Instant)> {
        self.results.try_recv().ok()
    }
}

impl ScreenshotSender {
    pub fn report_error(&self, error: anyhow::Error) {
        let _ = self.feedback.send((Err(error), std::time::Instant::now()));
        (self.wake)();
    }
    pub fn submit(
        &self,
        device: &wgpu::Device,
        readback: Readback,
        submission: wgpu::SubmissionIndex,
        minecraft_dir: &Path,
    ) -> Result<()> {
        let job = Job {
            source: Source::Gpu {
                device: device.clone(),
                buffer: readback.buffer,
                submission,
                stride: readback.stride,
                bgra: readback.bgra,
            },
            directory: minecraft_dir.join("screenshots"),
            timestamp: timestamp(),
            width: readback.width,
            height: readback.height,
        };
        self.send(job)
    }

    pub fn submit_software(
        &self,
        minecraft_dir: &Path,
        size: (u32, u32),
        pixels: Vec<u32>,
    ) -> Result<()> {
        let expected = u64::from(size.0) * u64::from(size.1);
        ensure!(
            size.0 > 0 && size.1 > 0 && u64::try_from(pixels.len())? == expected,
            "invalid software screenshot dimensions"
        );
        self.send(Job {
            source: Source::Software(pixels),
            directory: minecraft_dir.join("screenshots"),
            timestamp: timestamp(),
            width: size.0,
            height: size.1,
        })
    }

    fn send(&self, job: Job) -> Result<()> {
        self.jobs.try_send(job).map_err(|error| match error {
            TrySendError::Full(_) => anyhow::anyhow!("screenshot queue is full"),
            TrySendError::Disconnected(_) => anyhow::anyhow!("screenshot writer stopped"),
        })
    }
}

impl Job {
    fn run(self) -> Result<PathBuf> {
        let pixels = match self.source {
            Source::Software(pixels) => {
                let mut rgba = Vec::with_capacity(
                    pixels
                        .len()
                        .checked_mul(4)
                        .context("screenshot too large")?,
                );
                for p in pixels {
                    rgba.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8, 255]);
                }
                rgba
            }
            Source::Gpu {
                device,
                buffer,
                submission,
                stride,
                bgra,
            } => {
                let (sender, receiver) = mpsc::channel();
                buffer
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |result| {
                        let _ = sender.send(result);
                    });
                device
                    .poll(wgpu::PollType::Wait {
                        submission_index: Some(submission),
                        timeout: Some(Duration::from_secs(30)),
                    })
                    .context("screenshot GPU readback failed")?;
                receiver
                    .recv_timeout(Duration::from_secs(1))
                    .context("screenshot readback timed out")??;
                let mapped = buffer
                    .slice(..)
                    .get_mapped_range()
                    .context("cannot access screenshot pixels")?;
                let pixels = unpack(&mapped, self.width, self.height, stride, bgra)?;
                drop(mapped);
                buffer.unmap();
                pixels
            }
        };
        write_png(
            &self.directory,
            &self.timestamp,
            self.width,
            self.height,
            &pixels,
        )
    }
}

fn unpack(bytes: &[u8], width: u32, height: u32, stride: u32, bgra: bool) -> Result<Vec<u8>> {
    let row = usize::try_from(width)?
        .checked_mul(4)
        .context("screenshot row too wide")?;
    let stride = usize::try_from(stride)?;
    let height = usize::try_from(height)?;
    ensure!(
        stride >= row
            && bytes.len() >= stride.checked_mul(height).context("screenshot too large")?,
        "invalid screenshot buffer"
    );
    let mut result = Vec::with_capacity(row.checked_mul(height).context("screenshot too large")?);
    for source in bytes.chunks_exact(stride).take(height) {
        for pixel in source[..row].chunks_exact(4) {
            if bgra {
                result.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
            } else {
                result.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
            }
        }
    }
    Ok(result)
}

fn write_png(
    directory: &Path,
    timestamp: &str,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> Result<PathBuf> {
    let mut encoded = Vec::new();
    image::codecs::png::PngEncoder::new(&mut encoded)
        .write_image(pixels, width, height, image::ExtendedColorType::Rgba8)
        .context("cannot encode screenshot")?;
    fs::create_dir_all(directory).context("cannot create screenshots directory")?;
    for index in 1..=u32::MAX {
        let name = if index == 1 {
            format!("{timestamp}.png")
        } else {
            format!("{timestamp}_{index}.png")
        };
        let path = directory.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(&encoded)
                    .context("cannot write screenshot")?;
                return Ok(path);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("cannot create screenshot file"),
        }
    }
    bail!("no available screenshot filename")
}

#[cfg(windows)]
fn timestamp() -> String {
    use windows_sys::Win32::{Foundation::SYSTEMTIME, System::SystemInformation::GetLocalTime};
    let mut time = SYSTEMTIME::default();
    // GetLocalTime initializes the supplied structure synchronously.
    unsafe { GetLocalTime(&mut time) };
    format!(
        "{:04}-{:02}-{:02}_{:02}.{:02}.{:02}",
        time.wYear, time.wMonth, time.wDay, time.wHour, time.wMinute, time.wSecond
    )
}

#[cfg(not(windows))]
fn timestamp() -> String {
    // Windows is the supported client platform.
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_results_survive_window_handoff_and_keep_completion_time() {
        let captures = std::rc::Rc::new(MenuScreenshots::new().unwrap());
        let old_screen = captures.clone();
        let sender = old_screen.sender();
        sender.report_error(anyhow::anyhow!("first"));
        old_screen.collect();
        drop(old_screen);
        let dir = tempfile::tempdir().unwrap();
        sender
            .submit_software(dir.path(), (1, 1), vec![0x123456])
            .unwrap();
        let (first, age) = captures.take().unwrap();
        assert_eq!(first.unwrap_err().to_string(), "first");
        assert!(age < Duration::from_secs(10));
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let saved = loop {
            if let Some((result, _)) = captures.take() {
                break result.unwrap();
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(
            saved.parent(),
            Some(dir.path().join("screenshots").as_path())
        );
        assert!(saved.is_file());
    }

    #[test]
    fn menu_capture_history_is_bounded() {
        let captures = MenuScreenshots::new().unwrap();
        let sender = captures.sender();
        for index in 0..105 {
            sender.report_error(anyhow::anyhow!("{index}"));
        }
        captures.collect();
        assert_eq!(captures.pending.borrow().len(), 100);
        assert_eq!(captures.take().unwrap().0.unwrap_err().to_string(), "5");
    }

    #[test]
    fn padding_and_channel_order_are_removed_without_flipping_rows() {
        let input = [
            1, 2, 3, 0, 4, 5, 6, 0, 99, 99, 99, 99, 7, 8, 9, 0, 10, 11, 12, 0, 99, 99, 99, 99,
        ];
        assert_eq!(
            unpack(&input, 2, 2, 12, true).unwrap(),
            [3, 2, 1, 255, 6, 5, 4, 255, 9, 8, 7, 255, 12, 11, 10, 255]
        );
        assert_eq!(
            &unpack(&input, 2, 2, 12, false).unwrap()[..4],
            &[1, 2, 3, 255]
        );
        assert!(unpack(&input, 4, 2, 12, true).is_err());
        assert!(unpack(&input[..20], 2, 2, 12, true).is_err());
    }

    #[test]
    fn png_names_never_overwrite_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let date = "2026-11-01_12.34.56";
        let pixels = [3, 2, 1, 255, 6, 5, 4, 255];
        let first = write_png(dir.path(), date, 2, 1, &pixels).unwrap();
        let second = write_png(dir.path(), date, 2, 1, &pixels).unwrap();
        assert_eq!(first.file_name().unwrap(), "2026-11-01_12.34.56.png");
        assert_eq!(second.file_name().unwrap(), "2026-11-01_12.34.56_2.png");
        assert_eq!(image::open(first).unwrap().to_rgba8().as_raw(), &pixels);
        assert_eq!(image::open(second).unwrap().to_rgba8().as_raw(), &pixels);
    }

    #[test]
    fn saving_reports_an_unusable_screenshot_directory() {
        let dir = tempfile::tempdir().unwrap();
        let occupied = dir.path().join("screenshots");
        fs::write(&occupied, b"not a directory").unwrap();
        let error = write_png(&occupied, "2026-11-01_12.34.56", 1, 1, &[1, 2, 3, 255]).unwrap_err();
        assert!(format!("{error:#}").contains("screenshots directory"));
        assert_eq!(fs::read(occupied).unwrap(), b"not a directory");
    }

    #[test]
    fn software_frame_is_encoded_off_thread_with_correct_colors() {
        let dir = tempfile::tempdir().unwrap();
        let writer = Screenshots::new().unwrap();
        writer
            .sender()
            .submit_software(dir.path(), (2, 1), vec![0x00112233, 0x00aabbcc])
            .unwrap();
        let file = writer
            .results
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .0
            .unwrap();
        assert_eq!(
            image::open(file).unwrap().to_rgba8().as_raw(),
            &[0x11, 0x22, 0x33, 255, 0xaa, 0xbb, 0xcc, 255]
        );
        assert!(
            writer
                .sender()
                .submit_software(dir.path(), (2, 2), vec![0])
                .is_err()
        );
    }
}
