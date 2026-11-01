//! Build an atlas from the sprites referenced by resolved models.
//!
//! Animation is advanced in whole game ticks. The CPU atlas must be uploaded by the renderer
//! after an update; its filtering and interpolation are not yet compared pixel-for-pixel.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::Cursor,
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use image::{ImageFormat, ImageReader, RgbaImage};
use serde::Deserialize;

use crate::{
    TextureAtlas,
    packs::{ResourceId, ResourceProvider},
};

const MAX_IMAGE_SIDE: u32 = 4096;
const MAX_SOURCE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_SPRITES: usize = 16384;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Frame {
    pub index: u32,
    pub ticks: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rectangle {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Transparency {
    Opaque,
    Cutout,
    Translucent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FrameState {
    current: u32,
    next: u32,
    /// Vanilla quantizes animation interpolation to thousandths.
    progress: u16,
}

#[derive(Clone)]
struct Sprite {
    rect: Rectangle,
    image: Arc<RgbaImage>,
    frames: Vec<Frame>,
    frame_period: u64,
    interpolate: bool,
    transparency: Transparency,
    current: FrameState,
}

struct AtlasLayout {
    width: u32,
    height: u32,
    positions: Vec<(u32, u32)>,
}

#[derive(Clone)]
pub struct SpriteAtlas {
    pub texture: TextureAtlas,
    sprites: BTreeMap<ResourceId, Sprite>,
}

#[derive(Deserialize)]
struct Metadata {
    animation: Option<Animation>,
}

#[derive(Deserialize)]
struct Animation {
    width: Option<u32>,
    height: Option<u32>,
    #[serde(default = "one")]
    frametime: u32,
    frames: Option<Vec<FrameSpec>>,
    #[serde(default)]
    interpolate: bool,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum FrameSpec {
    Index(u32),
    Timed { index: u32, time: Option<u32> },
}

fn one() -> u32 {
    1
}

impl SpriteAtlas {
    /// Deduplicates identifiers, enforces allocation limits, and preserves each sprite's size.
    pub fn build(
        source: &mut impl ResourceProvider,
        identifiers: impl IntoIterator<Item = ResourceId>,
        max_side: u32,
    ) -> Result<Self> {
        ensure!(
            (4..=MAX_IMAGE_SIDE).contains(&max_side),
            "invalid atlas size limit"
        );
        let identifiers: BTreeSet<_> = identifiers.into_iter().collect();
        ensure!(
            !identifiers.is_empty(),
            "cannot build an empty texture atlas"
        );
        ensure!(identifiers.len() <= MAX_SPRITES, "too many atlas sprites");
        let mut sources = Vec::with_capacity(identifiers.len());
        let mut source_bytes = 0u64;
        for id in identifiers {
            let path = id.asset_path("textures", "png");
            let asset = source
                .read_asset_with_metadata(&path)?
                .with_context(|| format!("missing texture {path}"))?;
            let mut reader = ImageReader::with_format(Cursor::new(asset.bytes), ImageFormat::Png);
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(MAX_IMAGE_SIDE);
            limits.max_image_height = Some(MAX_IMAGE_SIDE);
            limits.max_alloc = Some(64 * 1024 * 1024);
            reader.limits(limits);
            let image = reader
                .decode()
                .with_context(|| format!("invalid PNG texture {path}"))?
                .to_rgba8();
            ensure!(
                image.width() > 0 && image.height() > 0,
                "empty texture {path}"
            );
            source_bytes += u64::from(image.width()) * u64::from(image.height()) * 4;
            ensure!(
                source_bytes <= MAX_SOURCE_BYTES,
                "sprite images exceed memory limit"
            );
            let metadata = asset
                .metadata
                .map(|bytes| {
                    serde_json::from_slice::<Metadata>(&bytes)
                        .with_context(|| format!("invalid texture metadata {path}.mcmeta"))
                })
                .transpose()?;
            let (width, height, frames, interpolate) = animation(
                image.width(),
                image.height(),
                metadata.and_then(|m| m.animation),
            )
            .with_context(|| format!("invalid animation for {path}"))?;
            ensure!(
                width + 2 <= max_side && height + 2 <= max_side,
                "texture {path} exceeds atlas size limit"
            );
            let frame_period = frames
                .iter()
                .try_fold(0u64, |sum, frame| sum.checked_add(u64::from(frame.ticks)))
                .context("animation period is too long")?;
            let current = FrameState {
                current: frames[0].index,
                next: frames[0].index,
                progress: 0,
            };
            let transparency = transparency(&image, interpolate);
            sources.push((
                id,
                Sprite {
                    rect: Rectangle {
                        x: 0,
                        y: 0,
                        width,
                        height,
                    },
                    image: Arc::new(image),
                    frames,
                    frame_period,
                    interpolate,
                    transparency,
                    current,
                },
            ));
        }
        // Taller sprites first makes the shelf packing deterministic and avoids excessive waste.
        sources.sort_by(|(a_id, a), (b_id, b)| {
            b.rect
                .height
                .cmp(&a.rect.height)
                .then(b.rect.width.cmp(&a.rect.width))
                .then(a_id.cmp(b_id))
        });
        let mut best: Option<AtlasLayout> = None;
        let mut width = 4u32;
        while width <= max_side {
            if let Some((height, positions)) = pack(&sources, width, max_side)
                && best.as_ref().is_none_or(|layout| {
                    u64::from(width) * u64::from(height)
                        < u64::from(layout.width) * u64::from(layout.height)
                })
            {
                best = Some(AtlasLayout {
                    width,
                    height,
                    positions,
                });
            }
            width *= 2;
        }
        let AtlasLayout {
            width,
            height,
            positions,
        } = best.context("textures do not fit in the atlas size limit")?;
        let size = usize::try_from(u64::from(width) * u64::from(height) * 4)?;
        let mut atlas = Self {
            texture: TextureAtlas {
                width,
                height,
                rgba: vec![0; size],
            },
            sprites: BTreeMap::new(),
        };
        for ((id, mut sprite), (x, y)) in sources.into_iter().zip(positions) {
            sprite.rect.x = x + 1;
            sprite.rect.y = y + 1;
            copy_frame(&mut atlas.texture, &sprite, sprite.current);
            atlas.sprites.insert(id, sprite);
        }
        Ok(atlas)
    }

    pub fn rectangle(&self, id: &ResourceId) -> Option<Rectangle> {
        self.sprites.get(id).map(|sprite| sprite.rect)
    }

    pub fn frames(&self, id: &ResourceId) -> Option<&[Frame]> {
        self.sprites.get(id).map(|sprite| sprite.frames.as_slice())
    }

    /// Conservative classification across every frame, including interpolation.
    pub fn transparency(&self, id: &ResourceId) -> Option<Transparency> {
        self.sprites.get(id).map(|sprite| sprite.transparency)
    }

    /// A stable key for the complete visible animation state at a tick.
    /// Identical keys produce identical atlas pixels and can reuse GPU images.
    pub fn animation_key(&self, tick: u64) -> Vec<(u32, u32, u16)> {
        self.sprites
            .values()
            .map(|sprite| {
                let state = frame_state(sprite, tick);
                (state.current, state.next, state.progress)
            })
            .collect()
    }

    /// Check without copying the shared atlas when all animation frames are unchanged.
    pub fn needs_update(&self, tick: u64) -> bool {
        self.sprites
            .values()
            .any(|sprite| frame_state(sprite, tick) != sprite.current)
    }

    /// Updates changed frames in the CPU atlas; the caller uploads it to the GPU.
    pub fn update(&mut self, tick: u64) -> bool {
        let mut changed = false;
        for sprite in self.sprites.values_mut() {
            let state = frame_state(sprite, tick);
            if state != sprite.current {
                copy_frame(&mut self.texture, sprite, state);
                sprite.current = state;
                changed = true;
            }
        }
        changed
    }
}

fn frame_state(sprite: &Sprite, tick: u64) -> FrameState {
    let mut elapsed = tick % sprite.frame_period;
    let mut selected = 0;
    for (index, frame) in sprite.frames.iter().enumerate() {
        if elapsed < u64::from(frame.ticks) {
            selected = index;
            break;
        }
        elapsed -= u64::from(frame.ticks);
    }
    let frame = sprite.frames[selected];
    let next = sprite.frames[(selected + 1) % sprite.frames.len()].index;
    let progress = if sprite.interpolate && frame.index != next {
        ((elapsed as f32 / frame.ticks as f32) * 1000.0) as u16
    } else {
        0
    };
    FrameState {
        current: frame.index,
        next: if progress == 0 { frame.index } else { next },
        progress,
    }
}

/// Extract the initial sprite frame, preserving non-animated rectangular images.
pub fn first_frame(image: &RgbaImage, metadata: Option<&[u8]>) -> Result<RgbaImage> {
    ensure!(
        image.width() > 0 && image.height() > 0,
        "empty sprite image"
    );
    let metadata = metadata
        .map(serde_json::from_slice::<Metadata>)
        .transpose()?;
    let (width, height, frames, _) = animation(
        image.width(),
        image.height(),
        metadata.and_then(|value| value.animation),
    )?;
    let index = frames[0].index;
    let columns = image.width() / width;
    Ok(image::imageops::crop_imm(
        image,
        (index % columns) * width,
        (index / columns) * height,
        width,
        height,
    )
    .to_image())
}

/// Return the atlas dimensions of an image's animation frame without decoding pixels.
pub fn frame_dimensions(width: u32, height: u32, metadata: Option<&[u8]>) -> Result<(u32, u32)> {
    ensure!(width > 0 && height > 0, "empty sprite image");
    let metadata = metadata
        .map(serde_json::from_slice::<Metadata>)
        .transpose()?;
    let Some(animation) = metadata.and_then(|value| value.animation) else {
        return Ok((width, height));
    };
    let default = width.min(height);
    let frame_width = animation.width.unwrap_or(if animation.height.is_some() {
        width
    } else {
        default
    });
    let frame_height = animation.height.unwrap_or(if animation.width.is_some() {
        height
    } else {
        default
    });
    ensure!(
        frame_width > 0
            && frame_height > 0
            && width.is_multiple_of(frame_width)
            && height.is_multiple_of(frame_height),
        "animation frame dimensions do not divide the image"
    );
    ensure!(
        animation.frametime > 0,
        "animation frametime must be positive"
    );
    let count = (width / frame_width)
        .checked_mul(height / frame_height)
        .context("too many animation frames")?;
    if let Some(frames) = animation.frames {
        ensure!(!frames.is_empty(), "animation frame list is empty");
        for frame in frames {
            let (index, ticks) = match frame {
                FrameSpec::Index(index) => (index, animation.frametime),
                FrameSpec::Timed { index, time } => (index, time.unwrap_or(animation.frametime)),
            };
            ensure!(
                index < count && ticks > 0,
                "invalid animation frame index or duration"
            );
        }
    }
    Ok((frame_width, frame_height))
}

fn animation(
    image_width: u32,
    image_height: u32,
    metadata: Option<Animation>,
) -> Result<(u32, u32, Vec<Frame>, bool)> {
    let Some(metadata) = metadata else {
        return Ok((
            image_width,
            image_height,
            vec![Frame { index: 0, ticks: 1 }],
            false,
        ));
    };
    ensure!(
        metadata.frametime > 0,
        "animation frametime must be positive"
    );
    let default = image_width.min(image_height);
    let width = metadata.width.unwrap_or(if metadata.height.is_some() {
        image_width
    } else {
        default
    });
    let height = metadata.height.unwrap_or(if metadata.width.is_some() {
        image_height
    } else {
        default
    });
    ensure!(
        width > 0
            && height > 0
            && image_width.is_multiple_of(width)
            && image_height.is_multiple_of(height),
        "animation frame dimensions do not divide the image"
    );
    let count = (image_width / width)
        .checked_mul(image_height / height)
        .context("too many animation frames")?;
    let frames = match metadata.frames {
        None => (0..count)
            .map(|index| Frame {
                index,
                ticks: metadata.frametime,
            })
            .collect(),
        Some(frames) => {
            ensure!(!frames.is_empty(), "animation frame list is empty");
            frames
                .into_iter()
                .map(|spec| {
                    let (index, ticks) = match spec {
                        FrameSpec::Index(index) => (index, metadata.frametime),
                        FrameSpec::Timed { index, time } => {
                            (index, time.unwrap_or(metadata.frametime))
                        }
                    };
                    ensure!(
                        index < count && ticks > 0,
                        "invalid animation frame index or duration"
                    );
                    Ok(Frame { index, ticks })
                })
                .collect::<Result<Vec<_>>>()?
        }
    };
    Ok((width, height, frames, metadata.interpolate))
}

fn transparency(image: &RgbaImage, interpolate: bool) -> Transparency {
    let mut has_transparency = false;
    for pixel in image.pixels() {
        match pixel.0[3] {
            255 => {}
            0 => has_transparency = true,
            _ => return Transparency::Translucent,
        }
    }
    if !has_transparency {
        Transparency::Opaque
    } else if interpolate {
        Transparency::Translucent
    } else {
        Transparency::Cutout
    }
}

fn pack(
    sources: &[(ResourceId, Sprite)],
    width: u32,
    max_side: u32,
) -> Option<(u32, Vec<(u32, u32)>)> {
    let (mut x, mut y, mut row_height) = (0u32, 0u32, 0u32);
    let mut positions = Vec::with_capacity(sources.len());
    for (_, sprite) in sources {
        let (w, h) = (sprite.rect.width + 2, sprite.rect.height + 2);
        if w > width {
            return None;
        }
        if x + w > width {
            x = 0;
            y += row_height;
            row_height = 0;
        }
        if y + h > max_side {
            return None;
        }
        positions.push((x, y));
        x += w;
        row_height = row_height.max(h);
    }
    let height = (y + row_height).next_power_of_two();
    (height <= max_side).then_some((height, positions))
}

fn copy_frame(atlas: &mut TextureAtlas, sprite: &Sprite, state: FrameState) {
    let r = sprite.rect;
    let columns = sprite.image.width() / r.width;
    let sx = (state.current % columns) * r.width;
    let sy = (state.current / columns) * r.height;
    let nx = (state.next % columns) * r.width;
    let ny = (state.next / columns) * r.height;
    // Duplicate the edge pixels into a one-pixel gutter to limit atlas bleeding.
    for y in 0..r.height + 2 {
        for x in 0..r.width + 2 {
            let local_x = x.saturating_sub(1).min(r.width - 1);
            let local_y = y.saturating_sub(1).min(r.height - 1);
            let pixel = sprite.image.get_pixel(sx + local_x, sy + local_y);
            let dst = (((r.y - 1 + y) * atlas.width + r.x - 1 + x) * 4) as usize;
            if state.progress == 0 {
                atlas.rgba[dst..dst + 4].copy_from_slice(&pixel.0);
            } else {
                let next = sprite.image.get_pixel(nx + local_x, ny + local_y);
                let progress = u32::from(state.progress);
                for channel in 0..4 {
                    atlas.rgba[dst + channel] = ((u32::from(pixel.0[channel]) * (1000 - progress)
                        + u32::from(next.0[channel]) * progress
                        + 500)
                        / 1000) as u8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, Rgba};
    use std::collections::HashMap;

    #[derive(Default)]
    struct Memory(HashMap<String, Vec<u8>>);
    impl ResourceProvider for Memory {
        fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.0.get(path).cloned())
        }
    }
    fn png(image: RgbaImage) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut output, ImageFormat::Png)
            .unwrap();
        output.into_inner()
    }
    fn pixel(atlas: &TextureAtlas, x: u32, y: u32) -> &[u8] {
        let start = ((y * atlas.width + x) * 4) as usize;
        &atlas.rgba[start..start + 4]
    }

    #[test]
    fn first_frame_respects_metadata_and_keeps_unanimated_rectangles() {
        let mut image = RgbaImage::from_pixel(2, 4, Rgba([255, 0, 0, 255]));
        for y in 2..4 {
            for x in 0..2 {
                image.put_pixel(x, y, Rgba([0, 0, 255, 255]));
            }
        }
        assert_eq!(first_frame(&image, None).unwrap().dimensions(), (2, 4));
        let frame = first_frame(&image, Some(br#"{"animation":{"frames":[1,0]}}"#)).unwrap();
        assert_eq!(frame.dimensions(), (2, 2));
        assert_eq!(frame.get_pixel(0, 0).0, [0, 0, 255, 255]);
        assert!(first_frame(&image, Some(br#"{"animation":{"frames":[8]}}"#)).is_err());
        for metadata in [
            None,
            Some(&br#"{"animation":{"frames":[1,0]}}"#[..]),
            Some(&br#"{"animation":{"width":1,"height":4}}"#[..]),
        ] {
            assert_eq!(
                frame_dimensions(image.width(), image.height(), metadata).unwrap(),
                first_frame(&image, metadata).unwrap().dimensions()
            );
        }
        assert!(frame_dimensions(2, 4, Some(br#"{"animation":{"frames":[8]}}"#)).is_err());
    }

    #[test]
    fn atlas_packs_mixed_sizes_and_advances_animation_frames() {
        let mut source = Memory::default();
        let mut frames = RgbaImage::from_pixel(2, 4, Rgba([255, 0, 0, 255]));
        for y in 2..4 {
            for x in 0..2 {
                frames.put_pixel(x, y, Rgba([0, 0, 255, 128]));
            }
        }
        source
            .0
            .insert("assets/minecraft/textures/block/a.png".into(), png(frames));
        source.0.insert(
            "assets/minecraft/textures/block/a.png.mcmeta".into(),
            br#"{"animation":{"frametime":2,"frames":[1,{"index":0,"time":3}]}}"#.to_vec(),
        );
        source.0.insert(
            "assets/minecraft/textures/block/b.png".into(),
            png(RgbaImage::from_pixel(4, 2, Rgba([0, 255, 0, 255]))),
        );
        let a = ResourceId::parse("block/a").unwrap();
        let b = ResourceId::parse("block/b").unwrap();
        let mut atlas =
            SpriteAtlas::build(&mut source, [a.clone(), b.clone(), a.clone()], 32).unwrap();
        let mut snapshot = atlas.clone();
        assert!(Arc::ptr_eq(
            &atlas.sprites[&a].image,
            &snapshot.sprites[&a].image
        ));
        assert!(snapshot.update(2));
        let initial_rect = atlas.rectangle(&a).unwrap();
        assert_ne!(
            pixel(&atlas.texture, initial_rect.x, initial_rect.y),
            pixel(&snapshot.texture, initial_rect.x, initial_rect.y)
        );
        for tick in 0..12 {
            let mut probe = atlas.clone();
            assert_eq!(probe.needs_update(tick), probe.update(tick));
        }
        let rect = atlas.rectangle(&a).unwrap();
        assert_eq!((rect.width, rect.height), (2, 2));
        assert_eq!(
            atlas.frames(&a).unwrap(),
            &[Frame { index: 1, ticks: 2 }, Frame { index: 0, ticks: 3 }]
        );
        assert_eq!(pixel(&atlas.texture, rect.x, rect.y), &[0, 0, 255, 128]);
        assert_eq!(
            pixel(&atlas.texture, rect.x - 1, rect.y - 1),
            &[0, 0, 255, 128]
        );
        assert!(!atlas.update(1));
        assert!(atlas.update(2));
        assert_eq!(pixel(&atlas.texture, rect.x, rect.y), &[255, 0, 0, 255]);
        assert!(atlas.update(5));
        assert_eq!(atlas.rectangle(&b).unwrap().width, 4);
    }

    #[test]
    fn interpolation_blends_frames_and_invalid_animation_frames_are_reported() {
        let mut source = Memory::default();
        let mut frames = RgbaImage::from_pixel(2, 4, Rgba([0, 0, 0, 0]));
        for y in 2..4 {
            for x in 0..2 {
                frames.put_pixel(x, y, Rgba([200, 100, 40, 200]));
            }
        }
        source
            .0
            .insert("assets/minecraft/textures/block/a.png".into(), png(frames));
        let id = ResourceId::parse("block/a").unwrap();
        source.0.insert(
            "assets/minecraft/textures/block/a.png.mcmeta".into(),
            br#"{"animation":{"frametime":4,"interpolate":true}}"#.to_vec(),
        );
        let mut atlas = SpriteAtlas::build(&mut source, [id.clone()], 16).unwrap();
        let rect = atlas.rectangle(&id).unwrap();
        assert_eq!(atlas.transparency(&id), Some(Transparency::Translucent));
        assert!(atlas.update(1));
        assert_eq!(pixel(&atlas.texture, rect.x, rect.y), &[50, 25, 10, 50]);
        assert!(atlas.update(2));
        assert_eq!(
            pixel(&atlas.texture, rect.x - 1, rect.y - 1),
            &[100, 50, 20, 100]
        );
        assert!(atlas.update(4));
        assert_eq!(pixel(&atlas.texture, rect.x, rect.y), &[200, 100, 40, 200]);
        assert!(atlas.update(8));
        assert_eq!(pixel(&atlas.texture, rect.x, rect.y), &[0, 0, 0, 0]);
        source.0.insert(
            "assets/minecraft/textures/block/a.png.mcmeta".into(),
            br#"{"animation":{"frames":[9]}}"#.to_vec(),
        );
        assert!(SpriteAtlas::build(&mut source, [id], 16).is_err());
    }
}
