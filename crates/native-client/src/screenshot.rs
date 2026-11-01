//! Asynchronous PNG encoding and GPU readback for screenshots.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, SyncSender, TrySendError},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use image::ImageEncoder;

/// Bounded queue: screenshot processing must never stall the render thread or
/// consume an unbounded amount of GPU and CPU memory.
pub struct Screenshots {
    jobs: SyncSender<Job>,
    results: Receiver<Result<PathBuf>>,
}

struct Job {
    device: wgpu::Device,
    buffer: wgpu::Buffer,
    submission: wgpu::SubmissionIndex,
    directory: PathBuf,
    timestamp: String,
    width: u32,
    height: u32,
    stride: u32,
    bgra: bool,
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
        let (jobs, receiver) = mpsc::sync_channel::<Job>(3);
        let (send_result, results) = mpsc::channel();
        thread::Builder::new()
            .name("screenshot-writer".into())
            .spawn(move || {
                while let Ok(job) = receiver.recv() {
                    let _ = send_result.send(job.run());
                }
            })
            .context("cannot start screenshot writer")?;
        Ok(Self { jobs, results })
    }

    pub fn submit(
        &self,
        device: &wgpu::Device,
        readback: Readback,
        submission: wgpu::SubmissionIndex,
        minecraft_dir: &Path,
    ) -> Result<()> {
        let job = Job {
            device: device.clone(),
            buffer: readback.buffer,
            submission,
            directory: minecraft_dir.join("screenshots"),
            timestamp: timestamp(),
            width: readback.width,
            height: readback.height,
            stride: readback.stride,
            bgra: readback.bgra,
        };
        self.jobs.try_send(job).map_err(|error| match error {
            TrySendError::Full(_) => anyhow::anyhow!("screenshot queue is full"),
            TrySendError::Disconnected(_) => anyhow::anyhow!("screenshot writer stopped"),
        })
    }

    pub fn take_result(&self) -> Option<Result<PathBuf>> {
        self.results.try_recv().ok()
    }
}

impl Job {
    fn run(self) -> Result<PathBuf> {
        let (sender, receiver) = mpsc::channel();
        self.buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(self.submission),
                timeout: Some(Duration::from_secs(30)),
            })
            .context("screenshot GPU readback failed")?;
        receiver
            .recv_timeout(Duration::from_secs(1))
            .context("screenshot readback timed out")??;
        let mapped = self
            .buffer
            .slice(..)
            .get_mapped_range()
            .context("cannot access screenshot pixels")?;
        let pixels = unpack(&mapped, self.width, self.height, self.stride, self.bgra)?;
        drop(mapped);
        self.buffer.unmap();
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
}
