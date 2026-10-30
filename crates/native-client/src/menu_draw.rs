use image::RgbaImage;

use crate::menu_assets::{GuiSpriteScaling, MenuAssets};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
}

pub struct Canvas {
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u32>,
    /// Logical-pixel glint coordinates and contribution, allocated only when
    /// an enchanted icon is actually visible. The renderer animates this mask.
    pub glint: Option<Vec<u16>>,
}

impl Canvas {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width: width as i32,
            height: height as i32,
            pixels: vec![0; (width * height) as usize],
            glint: None,
        }
    }

    pub fn pixel(&mut self, x: i32, y: i32, rgba: [u8; 4]) {
        if x < 0 || y < 0 || x >= self.width || y >= self.height || rgba[3] == 0 {
            return;
        }
        let index = (y * self.width + x) as usize;
        let dst = &mut self.pixels[index];
        let alpha = u32::from(rgba[3]);
        if let Some(glint) = &mut self.glint {
            let old_alpha = *dst >> 24;
            let numerator = alpha * 255 + old_alpha * (255 - alpha);
            let previous = u32::from(glint[index * 4 + 3]);
            glint[index * 4 + 3] =
                ((previous * old_alpha * (255 - alpha) + numerator / 2) / numerator) as u16;
        }
        if alpha == 255 {
            *dst = 0xff00_0000
                | (u32::from(rgba[0]) << 16)
                | (u32::from(rgba[1]) << 8)
                | u32::from(rgba[2]);
        } else {
            let dst_alpha = *dst >> 24;
            let alpha_numerator = alpha * 255 + dst_alpha * (255 - alpha);
            let output_alpha = (alpha_numerator + 127) / 255;
            let blend = |source: u8, target: u32| {
                (u32::from(source) * alpha * 255 + target * dst_alpha * (255 - alpha))
                    / alpha_numerator
            };
            let r = blend(rgba[0], (*dst >> 16) & 255);
            let g = blend(rgba[1], (*dst >> 8) & 255);
            let b = blend(rgba[2], *dst & 255);
            *dst = (output_alpha << 24) | (r << 16) | (g << 8) | b;
        }
    }

    pub fn fill(&mut self, rect: Rect, color: [u8; 4]) {
        let left = rect.x.max(0);
        let top = rect.y.max(0);
        let right = (rect.x + rect.w).min(self.width);
        let bottom = (rect.y + rect.h).min(self.height);
        for y in top..bottom {
            for x in left..right {
                self.pixel(x, y, color);
            }
        }
    }

    pub fn image(&mut self, image: &RgbaImage, rect: Rect) {
        if rect.w <= 0 || rect.h <= 0 || image.width() == 0 || image.height() == 0 {
            return;
        }
        for y in rect.y.max(0)..(rect.y + rect.h).min(self.height) {
            let sy = ((y - rect.y) as u32 * image.height() / rect.h as u32).min(image.height() - 1);
            for x in rect.x.max(0)..(rect.x + rect.w).min(self.width) {
                let sx =
                    ((x - rect.x) as u32 * image.width() / rect.w as u32).min(image.width() - 1);
                self.pixel(x, y, image.get_pixel(sx, sy).0);
            }
        }
    }

    /// Record the visible item silhouette separately from the cached UI so
    /// its glint can animate without recomposing the whole screen.
    #[cfg(test)]
    pub fn mark_item_glint(&mut self, image: &RgbaImage, rect: Rect) {
        self.mark_item_glint_with_uv(image, rect, None);
    }

    /// The optional coordinates are Java's stitched atlas bounds for this sprite.
    #[cfg(test)]
    pub fn mark_item_glint_with_uv(&mut self, image: &RgbaImage, rect: Rect, uv: Option<[f32; 4]>) {
        self.mark_item_glint_with_mapping(image, rect, uv, None);
    }

    /// Mark a flat sprite or per-pixel atlas coordinates produced by the icon rasterizer.
    pub fn mark_item_glint_with_mapping(
        &mut self,
        image: &RgbaImage,
        rect: Rect,
        uv: Option<[f32; 4]>,
        pixels: Option<&[[u16; 3]; 256]>,
    ) {
        if rect.w <= 0 || rect.h <= 0 || image.width() == 0 || image.height() == 0 {
            return;
        }
        let encode = |value: f32| (value.clamp(0.0, 1.0) * 65535.0).round() as u16;
        let pixel_map = pixels.filter(|_| image.width() == 16 && image.height() == 16);
        // GUI items are normally 16×16. Compute each axis once per icon,
        // rather than repeating floating-point conversion for every pixel.
        let axes = (rect.w == 16 && rect.h == 16).then(|| {
            let [u0, v0, u1, v1] = uv.unwrap_or([0.0, 0.0, 1.0, 1.0]);
            let u: [u16; 16] =
                std::array::from_fn(|x| encode(u0 + (u1 - u0) * (x as f32 + 0.5) / 16.0));
            let v: [u16; 16] =
                std::array::from_fn(|y| encode(v0 + (v1 - v0) * (y as f32 + 0.5) / 16.0));
            (u, v, u16::from(uv.is_some()))
        });
        let glint = self
            .glint
            .get_or_insert_with(|| vec![0; self.pixels.len() * 4]);
        for y in rect.y.max(0)..(rect.y + rect.h).min(self.height) {
            let sy = ((y - rect.y) as u32 * image.height() / rect.h as u32).min(image.height() - 1);
            for x in rect.x.max(0)..(rect.x + rect.w).min(self.width) {
                let sx =
                    ((x - rect.x) as u32 * image.width() / rect.w as u32).min(image.width() - 1);
                let source_alpha = u32::from(image.get_pixel(sx, sy).0[3]);
                if source_alpha < 26 {
                    continue;
                }
                let index = (y * self.width + x) as usize;
                let output_alpha = self.pixels[index] >> 24;
                if output_alpha == 0 {
                    continue;
                }
                let coverage = ((source_alpha * 255 + output_alpha / 2) / output_alpha).min(255);
                let mapped = pixel_map
                    .map(|pixels| pixels[(sy * 16 + sx) as usize])
                    .filter(|pixel| pixel[2] != 0);
                let (u, v, atlas) = if let Some([u, v, _]) = mapped {
                    (u, v, 1)
                } else if let Some((us, vs, atlas)) = &axes {
                    (us[(x - rect.x) as usize], vs[(y - rect.y) as usize], *atlas)
                } else if let Some([u0, v0, u1, v1]) = uv {
                    let local_u = ((x - rect.x) as f32 + 0.5) / rect.w as f32;
                    let local_v = ((y - rect.y) as f32 + 0.5) / rect.h as f32;
                    (
                        encode(u0 + (u1 - u0) * local_u),
                        encode(v0 + (v1 - v0) * local_v),
                        1,
                    )
                } else {
                    let local_u = ((x - rect.x) as f32 + 0.5) / rect.w as f32;
                    let local_v = ((y - rect.y) as f32 + 0.5) / rect.h as f32;
                    (encode(local_u), encode(local_v), 0)
                };
                glint[index * 4..index * 4 + 4].copy_from_slice(&[u, v, atlas, coverage as u16]);
            }
        }
    }

    /// Composite a clipped child canvas at a logical position.
    pub fn composite(&mut self, source: &Canvas, x: i32, y: i32) {
        for sy in 0..source.height {
            for sx in 0..source.width {
                let color = source.pixels[(sy * source.width + sx) as usize];
                self.pixel(
                    x + sx,
                    y + sy,
                    [
                        (color >> 16) as u8,
                        (color >> 8) as u8,
                        color as u8,
                        (color >> 24) as u8,
                    ],
                );
                if let Some(source_glint) = &source.glint {
                    let mask = &source_glint[(sy * source.width + sx) as usize * 4..][..4];
                    if mask[3] != 0
                        && x + sx >= 0
                        && y + sy >= 0
                        && x + sx < self.width
                        && y + sy < self.height
                    {
                        let target = ((y + sy) * self.width + x + sx) as usize;
                        let src_alpha = color >> 24;
                        let dst_alpha = self.pixels[target] >> 24;
                        let contribution = (u32::from(mask[3]) * src_alpha + dst_alpha / 2)
                            .checked_div(dst_alpha)
                            .unwrap_or(0);
                        let glint = self
                            .glint
                            .get_or_insert_with(|| vec![0; self.pixels.len() * 4]);
                        glint[target * 4..target * 4 + 4].copy_from_slice(&[
                            mask[0],
                            mask[1],
                            mask[2],
                            contribution.min(255) as u16,
                        ]);
                    }
                }
            }
        }
    }

    pub fn gui_sprite(&mut self, image: &RgbaImage, rect: Rect, scaling: GuiSpriteScaling) {
        if image.width() == 0 || image.height() == 0 || rect.w <= 0 || rect.h <= 0 {
            return;
        }
        if scaling == GuiSpriteScaling::Stretch {
            self.image(image, rect);
            return;
        }
        fn source(pos: i32, dest: i32, size: i32, before: i32, after: i32, stretch: bool) -> u32 {
            let before = before.min(dest / 2);
            let after = after.min(dest / 2);
            if pos < before {
                pos as u32
            } else if pos >= dest - after {
                (size - (dest - pos)) as u32
            } else if stretch {
                (before
                    + ((i64::from(pos - before) * i64::from(size - before - after))
                        / i64::from(dest - before - after)) as i32) as u32
            } else {
                (before + (pos - before) % (size - before - after)) as u32
            }
        }
        let (width, height, border, stretch) = match scaling {
            GuiSpriteScaling::Stretch => unreachable!(),
            GuiSpriteScaling::Tile { width, height } if width > 0 && height > 0 => {
                (width, height, None, false)
            }
            GuiSpriteScaling::NineSlice {
                width,
                height,
                border,
                stretch_inner,
            } if width > 0
                && height > 0
                && border.iter().all(|&n| n >= 0)
                && i64::from(border[0]) + i64::from(border[2]) < i64::from(width)
                && i64::from(border[1]) + i64::from(border[3]) < i64::from(height) =>
            {
                (width, height, Some(border), stretch_inner)
            }
            _ => return,
        };
        let coordinate = |pos, dest, size, before, after| {
            if border.is_some() {
                source(pos, dest, size, before, after, stretch)
            } else {
                (pos % size) as u32
            }
        };
        let sides = border.unwrap_or([0; 4]);
        let left = rect.x.max(0);
        let right = (rect.x + rect.w).min(self.width);
        let horizontal: Vec<_> = (left..right)
            .map(|x| {
                let sx = coordinate(x - rect.x, rect.w, width, sides[0], sides[2]);
                (u64::from(sx) * u64::from(image.width()) / width as u64) as u32
            })
            .collect();
        for y in rect.y.max(0)..(rect.y + rect.h).min(self.height) {
            let sy = coordinate(y - rect.y, rect.h, height, sides[1], sides[3]);
            let sy = (u64::from(sy) * u64::from(image.height()) / height as u64) as u32;
            for (x, &sx) in (left..right).zip(&horizontal) {
                self.pixel(x, y, image.get_pixel(sx, sy).0);
            }
        }
    }

    pub fn tiled(&mut self, image: &RgbaImage) {
        if image.width() == 0 || image.height() == 0 {
            return;
        }
        for y in 0..self.height {
            for x in 0..self.width {
                let p = image
                    .get_pixel(x as u32 % image.width(), y as u32 % image.height())
                    .0;
                self.pixel(x, y, p);
            }
        }
    }

    /// Approximate the default vanilla blur at the canvas resolution.
    #[cfg(test)]
    pub fn blur(&mut self) {
        self.blur_radius(5);
    }

    pub fn blur_radius(&mut self, radius: u8) {
        if radius == 0 || self.width == 0 || self.height == 0 {
            return;
        }
        let mut temporary = vec![0; self.pixels.len()];
        let mut columns = vec![[0u32; 3]; self.width as usize];
        for _ in 0..3 {
            self.blur_pass(i32::from(radius.min(10)), &mut temporary, &mut columns);
        }
    }

    #[cfg(test)]
    pub(crate) fn blur_once(&mut self, radius: i32) {
        if self.width == 0 || self.height == 0 {
            return;
        }
        let mut temporary = vec![0; self.pixels.len()];
        let mut columns = vec![[0u32; 3]; self.width as usize];
        self.blur_pass(radius, &mut temporary, &mut columns);
    }

    fn blur_pass(&mut self, radius: i32, temporary: &mut [u32], columns: &mut [[u32; 3]]) {
        let count = (radius * 2 + 1) as u32;
        for y in 0..self.height {
            let mut sums = [0u32; 3];
            for offset in -radius..=radius {
                let pixel =
                    self.pixels[(y * self.width + offset.clamp(0, self.width - 1)) as usize];
                sums[0] += (pixel >> 16) & 255;
                sums[1] += (pixel >> 8) & 255;
                sums[2] += pixel & 255;
            }
            for x in 0..self.width {
                temporary[(y * self.width + x) as usize] = 0xff00_0000
                    | ((sums[0] / count) << 16)
                    | ((sums[1] / count) << 8)
                    | (sums[2] / count);
                let old =
                    self.pixels[(y * self.width + (x - radius).clamp(0, self.width - 1)) as usize];
                let new = self.pixels
                    [(y * self.width + (x + radius + 1).clamp(0, self.width - 1)) as usize];
                sums[0] = sums[0] - ((old >> 16) & 255) + ((new >> 16) & 255);
                sums[1] = sums[1] - ((old >> 8) & 255) + ((new >> 8) & 255);
                sums[2] = sums[2] - (old & 255) + (new & 255);
            }
        }
        // Traverse whole rows for the vertical pass. Column-by-column reads
        // stride across the entire framebuffer and thrash the CPU caches.
        columns.fill([0; 3]);
        let width = self.width as usize;
        for offset in -radius..=radius {
            let row = offset.clamp(0, self.height - 1) as usize * width;
            for (sums, &pixel) in columns.iter_mut().zip(&temporary[row..row + width]) {
                sums[0] += (pixel >> 16) & 255;
                sums[1] += (pixel >> 8) & 255;
                sums[2] += pixel & 255;
            }
        }
        for y in 0..self.height {
            let row = y as usize * width;
            let old_row = (y - radius).clamp(0, self.height - 1) as usize * width;
            let new_row = (y + radius + 1).clamp(0, self.height - 1) as usize * width;
            for (x, sums) in columns.iter_mut().enumerate() {
                self.pixels[row + x] = 0xff00_0000
                    | ((sums[0] / count) << 16)
                    | ((sums[1] / count) << 8)
                    | (sums[2] / count);
                let old = temporary[old_row + x];
                let new = temporary[new_row + x];
                sums[0] = sums[0] - ((old >> 16) & 255) + ((new >> 16) & 255);
                sums[1] = sums[1] - ((old >> 8) & 255) + ((new >> 8) & 255);
                sums[2] = sums[2] - (old & 255) + (new & 255);
            }
        }
    }

    pub fn panorama(&mut self, faces: &[RgbaImage], overlay: &RgbaImage, seconds: f32) {
        if crate::menu_assets::validate_panorama_faces(faces).is_err() {
            return;
        }
        let yaw = seconds * (std::f32::consts::PI / 90.0);
        let (sin, cos) = yaw.sin_cos();
        let aspect = self.width as f32 / self.height.max(1) as f32;
        if self.glint.is_some() {
            for y in 0..self.height {
                for x in 0..self.width {
                    let uv = [
                        (x as f32 + 0.5) / self.width as f32,
                        (y as f32 + 0.5) / self.height as f32,
                    ];
                    let (face, u, v) = panorama_face(panorama_direction(uv, aspect, sin, cos));
                    let p = sample_panorama(faces, face, u, v);
                    self.pixel(x, y, [p[0], p[1], p[2], 255]);
                }
            }
        } else if self.width > 0 && self.height > 0 {
            let width = self.width as usize;
            let height = self.height as usize;
            static WORKERS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
            let workers = *WORKERS.get_or_init(|| {
                std::thread::available_parallelism()
                    .map_or(1, usize::from)
                    .min(4)
            });
            let workers = if self.pixels.len() >= 512 * 512 {
                workers
            } else {
                1
            };
            let rows = height.div_ceil(workers);
            let draw = |index: usize, chunk: &mut [u32]| {
                for (local_y, row) in chunk.chunks_mut(width).enumerate() {
                    let y = index * rows + local_y;
                    for (x, pixel) in row.iter_mut().enumerate() {
                        let uv = [
                            (x as f32 + 0.5) / width as f32,
                            (y as f32 + 0.5) / height as f32,
                        ];
                        let (face, u, v) = panorama_face(panorama_direction(uv, aspect, sin, cos));
                        let p = sample_panorama(faces, face, u, v);
                        *pixel = 0xff00_0000
                            | (u32::from(p[0]) << 16)
                            | (u32::from(p[1]) << 8)
                            | u32::from(p[2]);
                    }
                }
            };
            if workers == 1 {
                draw(0, &mut self.pixels);
            } else {
                std::thread::scope(|scope| {
                    for (index, chunk) in self.pixels.chunks_mut(rows * width).enumerate() {
                        scope.spawn(move || draw(index, chunk));
                    }
                });
            }
        }
        self.image(
            overlay,
            Rect {
                x: 0,
                y: 0,
                w: self.width,
                h: self.height,
            },
        );
    }

    pub fn button(
        &mut self,
        assets: Option<&MenuAssets>,
        rect: Rect,
        active: bool,
        highlighted: bool,
        label: &str,
    ) {
        if let Some(assets) = assets {
            let img = if !active {
                &assets.button_disabled
            } else if highlighted {
                &assets.button_highlighted
            } else {
                &assets.button
            };
            self.nine_slice(img, rect);
        } else {
            let edge = if highlighted { 230 } else { 120 };
            self.fill(rect, [edge, edge, edge, 255]);
            self.fill(
                Rect {
                    x: rect.x + 1,
                    y: rect.y + 1,
                    w: rect.w - 2,
                    h: rect.h - 2,
                },
                [55, 55, 55, 255],
            );
        }
        self.text_centered(
            assets,
            rect.x + rect.w / 2,
            rect.y + (rect.h - 8) / 2,
            label,
            if active {
                [255, 255, 255]
            } else {
                [160, 160, 160]
            },
        );
    }

    fn nine_slice(&mut self, image: &RgbaImage, rect: Rect) {
        let (sw, sh) = (image.width() as i32, image.height() as i32);
        if sw < 6 || sh < 6 || rect.w < 6 || rect.h < 6 {
            self.image(image, rect);
            return;
        }
        for y in rect.y.max(0)..(rect.y + rect.h).min(self.height) {
            for x in rect.x.max(0)..(rect.x + rect.w).min(self.width) {
                let (dx, dy) = (x - rect.x, y - rect.y);
                let sx = if dx < 3 {
                    dx
                } else if dx >= rect.w - 3 {
                    sw - (rect.w - dx)
                } else {
                    3 + (dx - 3) * (sw - 6) / (rect.w - 6)
                };
                let sy = if dy < 3 {
                    dy
                } else if dy >= rect.h - 3 {
                    sh - (rect.h - dy)
                } else {
                    3 + (dy - 3) * (sh - 6) / (rect.h - 6)
                };
                self.pixel(x, y, image.get_pixel(sx as u32, sy as u32).0);
            }
        }
    }

    pub fn text_width(&self, assets: Option<&MenuAssets>, text: &str) -> i32 {
        text.chars().map(|c| glyph_width(assets, c)).sum()
    }

    /// The default Minecraft bitmap glyph adds one pixel of advance when bold.
    pub fn bold_text_width(&self, assets: Option<&MenuAssets>, text: &str) -> i32 {
        text.chars().map(|c| glyph_width(assets, c) + 1).sum()
    }

    pub fn text_centered(
        &mut self,
        assets: Option<&MenuAssets>,
        center: i32,
        y: i32,
        text: &str,
        color: [u8; 3],
    ) {
        let x = center - self.text_width(assets, text) / 2;
        self.text(assets, x, y, text, color);
    }

    /// Draw the same pixel font at an integer scale, including its shadow.
    pub fn text_centered_scaled(
        &mut self,
        assets: Option<&MenuAssets>,
        center: i32,
        y: i32,
        text: &str,
        color: [u8; 3],
        scale: i32,
    ) {
        if scale <= 0 {
            return;
        }
        let width = self.text_width(assets, text).max(1);
        let mut source = Canvas::new((width + 2) as u32, 10);
        source.text(assets, 0, 0, text, color);
        let left = center - width * scale / 2;
        for sy in 0..source.height {
            for sx in 0..source.width {
                let color = source.pixels[(sy * source.width + sx) as usize];
                if color >> 24 == 0 {
                    continue;
                }
                self.fill(
                    Rect {
                        x: left + sx * scale,
                        y: y + sy * scale,
                        w: scale,
                        h: scale,
                    },
                    [
                        (color >> 16) as u8,
                        (color >> 8) as u8,
                        color as u8,
                        (color >> 24) as u8,
                    ],
                );
            }
        }
    }

    pub fn text(
        &mut self,
        assets: Option<&MenuAssets>,
        x: i32,
        y: i32,
        text: &str,
        color: [u8; 3],
    ) {
        self.text_alpha(assets, x, y, text, color, 255);
    }

    pub fn text_alpha(
        &mut self,
        assets: Option<&MenuAssets>,
        x: i32,
        y: i32,
        text: &str,
        color: [u8; 3],
        opacity: u8,
    ) {
        let dark = [color[0] / 4, color[1] / 4, color[2] / 4];
        self.text_pass(assets, x + 1, y + 1, text, dark, opacity);
        self.text_pass(assets, x, y, text, color, opacity);
    }

    pub fn bold_text_alpha(
        &mut self,
        assets: Option<&MenuAssets>,
        x: i32,
        y: i32,
        text: &str,
        color: [u8; 3],
        opacity: u8,
    ) {
        let dark = [color[0] / 4, color[1] / 4, color[2] / 4];
        for (offset, color) in [(1, dark), (0, color)] {
            let mut cursor = x;
            for ch in text.chars() {
                let mut encoded = [0; 4];
                let glyph = ch.encode_utf8(&mut encoded);
                self.text_pass(assets, cursor + offset, y + offset, glyph, color, opacity);
                self.text_pass(
                    assets,
                    cursor + offset + 1,
                    y + offset,
                    glyph,
                    color,
                    opacity,
                );
                cursor += glyph_width(assets, ch) + 1;
            }
        }
    }

    pub fn text_without_shadow(
        &mut self,
        assets: Option<&MenuAssets>,
        x: i32,
        y: i32,
        text: &str,
        color: [u8; 3],
    ) {
        self.text_pass(assets, x, y, text, color, 255);
    }

    /// Draw resolved component spans using the existing resource-pack font.
    pub fn component_width(
        &self,
        assets: Option<&MenuAssets>,
        spans: &[crate::text_component::Span],
    ) -> i32 {
        spans
            .iter()
            .flat_map(|span| {
                span.text
                    .chars()
                    .map(move |ch| glyph_width(assets, ch) + i32::from(span.style.bold))
            })
            .sum()
    }

    pub fn component_shadow_alpha(
        &mut self,
        assets: Option<&MenuAssets>,
        x: i32,
        y: i32,
        spans: &[crate::text_component::Span],
        opacity: u8,
    ) {
        let width = self.component_width(assets, spans);
        if width <= 0 || opacity == 0 {
            return;
        }
        let left = x.max(0);
        let right = x.saturating_add(width).saturating_add(12).min(self.width);
        if right <= left {
            return;
        }
        let mut scratch = Canvas::new((right - left) as u32, 11);
        let shadow: Vec<_> = spans
            .iter()
            .map(|span| {
                let mut span = span.clone();
                let color = span.style.color.unwrap_or([255; 3]);
                span.style.color = Some(color.map(|channel| channel / 4));
                span
            })
            .collect();
        scratch.component_without_shadow(assets, x - left + 1, 1, &shadow, [63; 3]);
        scratch.component_without_shadow(assets, x - left, 0, spans, [255; 3]);
        for sy in 0..scratch.height {
            for sx in 0..scratch.width {
                let value = scratch.pixels[(sy * scratch.width + sx) as usize];
                let alpha = ((value >> 24) * u32::from(opacity) / 255) as u8;
                self.pixel(
                    left + sx,
                    y + sy,
                    [(value >> 16) as u8, (value >> 8) as u8, value as u8, alpha],
                );
            }
        }
    }

    /// Draw resolved component spans using the existing resource-pack font.
    pub fn component_without_shadow(
        &mut self,
        assets: Option<&MenuAssets>,
        mut x: i32,
        y: i32,
        spans: &[crate::text_component::Span],
        default_color: [u8; 3],
    ) {
        for span in spans {
            let color = span.style.color.unwrap_or(default_color);
            for ch in span.text.chars() {
                let mut encoded = [0; 4];
                let glyph = ch.encode_utf8(&mut encoded);
                let width = glyph_width(assets, ch) + i32::from(span.style.bold);
                if span.style.italic {
                    let mut scratch = Canvas::new(12, 8);
                    scratch.text_pass(assets, 0, 0, glyph, color, 255);
                    if span.style.bold {
                        scratch.text_pass(assets, 1, 0, glyph, color, 255);
                    }
                    for gy in 0..8 {
                        for gx in 0..12 {
                            let value = scratch.pixels[(gy * 12 + gx) as usize];
                            let alpha = (value >> 24) as u8;
                            self.pixel(
                                x + gx + (7 - gy) / 4,
                                y + gy,
                                [color[0], color[1], color[2], alpha],
                            );
                        }
                    }
                } else {
                    self.text_pass(assets, x, y, glyph, color, 255);
                    if span.style.bold {
                        self.text_pass(assets, x + 1, y, glyph, color, 255);
                    }
                }
                if span.style.strikethrough {
                    self.fill(
                        Rect {
                            x,
                            y: y + 4,
                            w: width,
                            h: 1,
                        },
                        [color[0], color[1], color[2], 255],
                    );
                }
                if span.style.underlined {
                    self.fill(
                        Rect {
                            x,
                            y: y + 8,
                            w: width,
                            h: 1,
                        },
                        [color[0], color[1], color[2], 255],
                    );
                }
                x += width;
            }
        }
    }

    fn text_pass(
        &mut self,
        assets: Option<&MenuAssets>,
        mut x: i32,
        y: i32,
        text: &str,
        color: [u8; 3],
        opacity: u8,
    ) {
        for ch in text.chars() {
            if ch != ' ' {
                if let Some(assets) = assets {
                    if let Some((image, sx, sy)) = bitmap_glyph(assets, ch) {
                        for gy in 0..8 {
                            for gx in 0..8 {
                                let alpha = (u16::from(image.get_pixel(sx + gx, sy + gy).0[3])
                                    * u16::from(opacity)
                                    / 255) as u8;
                                self.pixel(
                                    x + gx as i32,
                                    y + gy as i32,
                                    [color[0], color[1], color[2], alpha],
                                );
                            }
                        }
                    } else if let Some(glyph) = assets.unicode.get(&ch) {
                        for gy in 0..8 {
                            for gx in 0..glyph.bits.div_ceil(2) {
                                let mut on = false;
                                for dy in 0..2 {
                                    for dx in 0..2 {
                                        let bit = gx * 2 + dx;
                                        if bit < glyph.bits
                                            && glyph.rows[gy * 2 + dy]
                                                & (1 << (glyph.bits - 1 - bit))
                                                != 0
                                        {
                                            on = true;
                                        }
                                    }
                                }
                                if on {
                                    self.pixel(
                                        x + gx as i32,
                                        y + gy as i32,
                                        [color[0], color[1], color[2], opacity],
                                    );
                                }
                            }
                        }
                    } else {
                        let c = if ch.is_ascii() {
                            ch as u32
                        } else {
                            u32::from('?')
                        };
                        let sx = (c % 16) * 8;
                        let sy = (c / 16) * 8;
                        for gy in 0..8 {
                            for gx in 0..8 {
                                let alpha =
                                    (u16::from(assets.font.get_pixel(sx + gx, sy + gy).0[3])
                                        * u16::from(opacity)
                                        / 255) as u8;
                                self.pixel(
                                    x + gx as i32,
                                    y + gy as i32,
                                    [color[0], color[1], color[2], alpha],
                                );
                            }
                        }
                    }
                } else {
                    let glyph = fallback_glyph(ch);
                    for (gy, row) in glyph.into_iter().enumerate() {
                        for gx in 0..5 {
                            if row & (1 << (4 - gx)) != 0 {
                                self.pixel(
                                    x + gx,
                                    y + gy as i32,
                                    [color[0], color[1], color[2], opacity],
                                );
                            }
                        }
                    }
                }
            }
            x += glyph_width(assets, ch);
        }
    }
}

// The Y component is inverted to address the original, unflipped PNG faces.
fn panorama_direction(uv: [f32; 2], aspect: f32, sin: f32, cos: f32) -> [f32; 3] {
    let u = (2.0 * uv[0] - 1.0) * aspect * 0.9163312;
    let v = (1.0 - 2.0 * uv[1]) * 0.9163312;
    let y = v * 0.984_807_7 - 0.17364818;
    let z = v * 0.17364818 + 0.984_807_7;
    [u * cos + z * sin, y, -u * sin + z * cos]
}

fn panorama_face([dx, dy, dz]: [f32; 3]) -> (usize, f32, f32) {
    let (face, a, b, major) = if dx.abs() >= dy.abs() && dx.abs() >= dz.abs() {
        if dx > 0.0 {
            (1, -dz, -dy, dx.abs())
        } else {
            (3, dz, -dy, dx.abs())
        }
    } else if dy.abs() >= dz.abs() {
        if dy > 0.0 {
            (4, dx, dz, dy.abs())
        } else {
            (5, dx, -dz, dy.abs())
        }
    } else if dz > 0.0 {
        (0, dx, -dy, dz.abs())
    } else {
        (2, -dx, -dy, dz.abs())
    };
    (face, (a / major + 1.0) * 0.5, (b / major + 1.0) * 0.5)
}

// Convert an unflipped PNG face coordinate into the same direction space
// returned by panorama_direction.
fn panorama_face_direction(face: usize, u: f32, v: f32) -> [f32; 3] {
    let a = 2.0 * u - 1.0;
    let b = 2.0 * v - 1.0;
    match face {
        0 => [a, -b, 1.0],
        1 => [1.0, -b, -a],
        2 => [-a, -b, -1.0],
        3 => [-1.0, -b, a],
        4 => [a, 1.0, b],
        5 => [a, -1.0, -b],
        _ => unreachable!(),
    }
}

// Linear cube filtering reads across face edges. At a cube corner the missing
// texel is the average of the three incident face texels.
fn panorama_texel(faces: &[RgbaImage], face: usize, x: i32, y: i32) -> [f32; 4] {
    let size = faces[face].width() as i32;
    let x_inside = (0..size).contains(&x);
    let y_inside = (0..size).contains(&y);
    if x_inside && y_inside {
        return faces[face].get_pixel(x as u32, y as u32).0.map(f32::from);
    }
    let nearest_across_edge = |x: i32, y: i32| {
        let direction = panorama_face_direction(
            face,
            (x as f32 + 0.5) / size as f32,
            (y as f32 + 0.5) / size as f32,
        );
        let (other, u, v) = panorama_face(direction);
        let ix = ((u * size as f32).floor() as i32).clamp(0, size - 1);
        let iy = ((v * size as f32).floor() as i32).clamp(0, size - 1);
        faces[other]
            .get_pixel(ix as u32, iy as u32)
            .0
            .map(f32::from)
    };
    if x_inside || y_inside {
        return nearest_across_edge(x, y);
    }
    let ix = x.clamp(0, size - 1);
    let iy = y.clamp(0, size - 1);
    let current = faces[face].get_pixel(ix as u32, iy as u32).0;
    let across_x = nearest_across_edge(x, iy);
    let across_y = nearest_across_edge(ix, y);
    std::array::from_fn(|channel| {
        (f32::from(current[channel]) + across_x[channel] + across_y[channel]) / 3.0
    })
}

fn sample_panorama(faces: &[RgbaImage], face: usize, u: f32, v: f32) -> [u8; 4] {
    let size = faces[face].width() as f32;
    let sx = u * size - 0.5;
    let sy = v * size - 0.5;
    let x0 = sx.floor() as i32;
    let y0 = sy.floor() as i32;
    let tx = sx - x0 as f32;
    let ty = sy - y0 as f32;
    let limit = faces[face].width() as i32 - 1;
    let pixels = if x0 >= 0 && y0 >= 0 && x0 < limit && y0 < limit {
        let image = &faces[face];
        let pixel = |x, y| image.get_pixel(x as u32, y as u32).0.map(f32::from);
        [
            pixel(x0, y0),
            pixel(x0 + 1, y0),
            pixel(x0, y0 + 1),
            pixel(x0 + 1, y0 + 1),
        ]
    } else {
        [
            panorama_texel(faces, face, x0, y0),
            panorama_texel(faces, face, x0 + 1, y0),
            panorama_texel(faces, face, x0, y0 + 1),
            panorama_texel(faces, face, x0 + 1, y0 + 1),
        ]
    };
    std::array::from_fn(|channel| {
        let top = pixels[0][channel] * (1.0 - tx) + pixels[1][channel] * tx;
        let bottom = pixels[2][channel] * (1.0 - tx) + pixels[3][channel] * tx;
        (top * (1.0 - ty) + bottom * ty).round() as u8
    })
}

fn glyph_width(assets: Option<&MenuAssets>, ch: char) -> i32 {
    if ch == ' ' {
        return 4;
    }
    if let Some(assets) = assets {
        if let Some(glyph) = assets.nonlatin_font.cells.get(&ch) {
            return glyph.advance;
        }
        if let Some((image, sx, sy)) = bitmap_glyph(assets, ch) {
            for x in (0..8).rev() {
                if (0..8).any(|y| image.get_pixel(sx + x, sy + y).0[3] != 0) {
                    return x as i32 + 2;
                }
            }
        } else if let Some(glyph) = assets.unicode.get(&ch) {
            return glyph.bits.div_ceil(2) as i32 + 1;
        } else {
            return glyph_width(Some(assets), '?');
        }
    }
    6
}

fn bitmap_glyph(assets: &MenuAssets, ch: char) -> Option<(&RgbaImage, u32, u32)> {
    if let Some(glyph) = assets.nonlatin_font.cells.get(&ch) {
        return Some((&assets.nonlatin_font.image, glyph.x, glyph.y));
    }
    if ch.is_ascii() {
        let c = ch as u32;
        return Some((&assets.font, (c % 16) * 8, (c / 16) * 8));
    }
    None
}

// Only used when the user's Minecraft installation cannot yet be found.
fn fallback_glyph(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        'A' => [14, 17, 17, 31, 17, 17, 17],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        'C' => [15, 16, 16, 16, 16, 16, 15],
        'D' => [30, 17, 17, 17, 17, 17, 30],
        'E' => [31, 16, 16, 30, 16, 16, 31],
        'F' => [31, 16, 16, 30, 16, 16, 16],
        'G' => [15, 16, 16, 19, 17, 17, 15],
        'H' => [17, 17, 17, 31, 17, 17, 17],
        'I' => [14, 4, 4, 4, 4, 4, 14],
        'J' => [7, 2, 2, 2, 18, 18, 12],
        'K' => [17, 18, 20, 24, 20, 18, 17],
        'L' => [16, 16, 16, 16, 16, 16, 31],
        'M' => [17, 27, 21, 21, 17, 17, 17],
        'N' => [17, 25, 21, 19, 17, 17, 17],
        'O' => [14, 17, 17, 17, 17, 17, 14],
        'P' => [30, 17, 17, 30, 16, 16, 16],
        'Q' => [14, 17, 17, 17, 21, 18, 13],
        'R' => [30, 17, 17, 30, 20, 18, 17],
        'S' => [15, 16, 16, 14, 1, 1, 30],
        'T' => [31, 4, 4, 4, 4, 4, 4],
        'U' => [17, 17, 17, 17, 17, 17, 14],
        'V' => [17, 17, 17, 17, 17, 10, 4],
        'W' => [17, 17, 17, 21, 21, 21, 10],
        'X' => [17, 17, 10, 4, 10, 17, 17],
        'Y' => [17, 17, 10, 4, 4, 4, 4],
        'Z' => [31, 1, 2, 4, 8, 16, 31],
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [30, 1, 1, 14, 1, 1, 30],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 16, 30, 1, 1, 30],
        '6' => [14, 16, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 1, 14],
        '.' => [0, 0, 0, 0, 0, 12, 12],
        ':' => [0, 12, 12, 0, 12, 12, 0],
        '-' => [0, 0, 0, 31, 0, 0, 0],
        '/' => [1, 2, 2, 4, 8, 8, 16],
        _ => [14, 17, 1, 2, 4, 0, 4],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn nonlatin_bitmap_precedes_unifont_for_drawing_and_width() {
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut assets = MenuAssets::load(&directory, &[]).unwrap();
        let glyph = &assets.nonlatin_font.cells[&'α'];
        let (sx, sy) = (glyph.x, glyph.y);
        for y in 0..8 {
            for x in 0..8 {
                assets.nonlatin_font.image.put_pixel(
                    sx + x,
                    sy + y,
                    image::Rgba([255, 255, 255, 0]),
                );
            }
        }
        assets
            .nonlatin_font
            .image
            .put_pixel(sx + 2, sy + 1, image::Rgba([255, 255, 255, 255]));
        let pack = tempfile::tempdir().unwrap();
        let path = pack
            .path()
            .join("assets/minecraft/textures/font/nonlatin_european.png");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        assets.nonlatin_font.image.save(path).unwrap();
        let mut packed = MenuAssets::load(&directory, &[pack.path().to_path_buf()]).unwrap();
        packed.unicode.insert(
            'α',
            crate::menu_assets::UnicodeGlyph {
                rows: [u32::MAX; 16],
                bits: 16,
            },
        );
        let mut canvas = Canvas::new(16, 10);
        assert_eq!(canvas.text_width(Some(&packed), "α"), 4);
        canvas.text_without_shadow(Some(&packed), 0, 0, "α", [255, 255, 255]);
        assert_eq!(canvas.pixels[16 + 2], 0xffff_ffff);
        assert_eq!(canvas.pixels.iter().filter(|&&pixel| pixel != 0).count(), 1);
        assert!(packed.unicode.contains_key(&'한'));
        assert!(bitmap_glyph(&packed, '한').is_none());

        let mut highres = image::imageops::resize(
            &packed.nonlatin_font.image,
            packed.nonlatin_font.image.width() * 2,
            packed.nonlatin_font.image.height() * 2,
            image::imageops::FilterType::Nearest,
        );
        for y in 0..16 {
            for x in 0..16 {
                highres.put_pixel(sx * 2 + x, sy * 2 + y, image::Rgba([255, 255, 255, 0]));
            }
        }
        highres.put_pixel(sx * 2 + 2, sy * 2, image::Rgba([255, 255, 255, 255]));
        highres
            .save(
                pack.path()
                    .join("assets/minecraft/textures/font/nonlatin_european.png"),
            )
            .unwrap();
        let highres_assets = MenuAssets::load(&directory, &[pack.path().to_path_buf()]).unwrap();
        assert_eq!(canvas.text_width(Some(&highres_assets), "α"), 3);
    }

    #[test]
    fn bold_text_advances_each_glyph_and_fills_its_right_neighbor() {
        let mut canvas = Canvas::new(24, 12);
        assert_eq!(canvas.text_width(None, "II"), 12);
        assert_eq!(canvas.bold_text_width(None, "II"), 14);
        canvas.bold_text_alpha(None, 0, 0, "II", [255, 255, 85], 255);
        let row = &canvas.pixels[24..48];
        for x in [2, 3, 9, 10] {
            assert_eq!(row[x], 0xffff_ff55);
        }
    }

    #[test]
    fn component_text_draws_inherited_color_bold_and_underlining() {
        use crate::text_component::{Span, Style};
        let mut canvas = Canvas::new(24, 12);
        let spans = [
            Span {
                text: "I".into(),
                style: Style {
                    color: Some([255, 85, 85]),
                    bold: true,
                    underlined: true,
                    ..Style::default()
                },
            },
            Span {
                text: "I".into(),
                style: Style::default(),
            },
        ];
        canvas.component_without_shadow(None, 0, 0, &spans, [64, 64, 64]);
        assert_eq!(canvas.pixels[24 + 2], 0xffff_5555);
        assert_eq!(canvas.pixels[24 + 3], 0xffff_5555);
        assert_eq!(canvas.pixels[24 + 9], 0xff40_4040);
        assert_eq!(canvas.pixels[8 * 24 + 6], 0xffff_5555);
        assert_eq!(canvas.pixels[8 * 24 + 7], 0);
    }

    #[test]
    fn item_glint_mask_follows_occlusion_and_child_compositing() {
        let mut child = Canvas::new(2, 1);
        let icon = RgbaImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                image::Rgba([200, 120, 20, 128])
            } else {
                image::Rgba([0, 0, 0, 0])
            }
        });
        let rect = Rect {
            x: 0,
            y: 0,
            w: 2,
            h: 1,
        };
        child.image(&icon, rect);
        child.mark_item_glint(&icon, rect);
        assert_eq!(child.glint.as_ref().unwrap()[3], 255);
        assert_eq!(child.glint.as_ref().unwrap()[7], 0);
        let mut parent = Canvas::new(3, 1);
        parent.fill(
            Rect {
                x: 0,
                y: 0,
                w: 3,
                h: 1,
            },
            [20, 20, 20, 255],
        );
        parent.composite(&child, 1, 0);
        assert_eq!(parent.glint.as_ref().unwrap()[7], 128);
        parent.pixel(1, 0, [255, 255, 255, 128]);
        assert_eq!(parent.glint.as_ref().unwrap()[7], 64);
        parent.pixel(1, 0, [0, 0, 0, 255]);
        assert_eq!(parent.glint.as_ref().unwrap()[7], 0);
    }

    #[test]
    fn item_glint_mask_stores_precise_atlas_coordinates() {
        let mut canvas = Canvas::new(16, 16);
        let icon = RgbaImage::from_pixel(16, 16, image::Rgba([255, 255, 255, 255]));
        let rect = Rect {
            x: 0,
            y: 0,
            w: 16,
            h: 16,
        };
        let bounds = [0.4091797, 0.31835938, 0.4248047, 0.34960938];
        canvas.image(&icon, rect);
        canvas.mark_item_glint_with_uv(&icon, rect, Some(bounds));
        let mask = canvas.glint.as_ref().unwrap();
        for (x, y) in [(0, 0), (5, 8), (15, 15)] {
            let pixel = &mask[(y * 16 + x) * 4..][..4];
            let u = bounds[0] + (bounds[2] - bounds[0]) * (x as f32 + 0.5) / 16.0;
            let v = bounds[1] + (bounds[3] - bounds[1]) * (y as f32 + 0.5) / 16.0;
            assert!((f32::from(pixel[0]) / 65535.0 - u).abs() < 0.00001);
            assert!((f32::from(pixel[1]) / 65535.0 - v).abs() < 0.00001);
            assert_eq!((pixel[2], pixel[3]), (1, 255));
        }
    }

    #[test]
    fn panorama_uses_linear_filtering_and_composites_the_overlay() {
        let mut image = RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 0, 255]));
        image.put_pixel(1, 0, image::Rgba([200, 100, 40, 255]));
        image.put_pixel(1, 1, image::Rgba([200, 100, 40, 255]));
        let images = vec![image; 6];
        assert_eq!(sample_panorama(&images, 0, 0.5, 0.5), [100, 50, 20, 255]);
        assert_eq!(sample_panorama(&images, 0, 0.25, 0.5), [0, 0, 0, 255]);
        assert_eq!(sample_panorama(&images, 0, 0.75, 0.5), [200, 100, 40, 255]);

        let faces = vec![RgbaImage::from_pixel(1, 1, image::Rgba([200, 100, 40, 255])); 6];
        let overlay = RgbaImage::from_pixel(1, 1, image::Rgba([0, 40, 80, 128]));
        let mut canvas = Canvas::new(5, 5);
        canvas.panorama(&faces, &overlay, 0.0);
        let center = canvas.pixels[12];
        let rgb = [
            ((center >> 16) & 255) as u8,
            ((center >> 8) & 255) as u8,
            center as u8,
        ];
        for (actual, expected) in rgb.into_iter().zip([100_u8, 70, 60]) {
            assert!(actual.abs_diff(expected) <= 1, "{rgb:?}");
        }
    }

    #[test]
    fn panorama_filters_across_edges_and_three_face_corners() {
        let colors = [6, 30, 60, 90, 120, 150];
        let faces: Vec<_> = colors
            .into_iter()
            .map(|value| RgbaImage::from_pixel(4, 4, image::Rgba([value, 0, 0, 255])))
            .collect();
        assert_eq!(sample_panorama(&faces, 0, 1.0, 0.5), [18, 0, 0, 255]);
        assert_eq!(sample_panorama(&faces, 0, 1.0, 0.0), [52, 0, 0, 255]);
    }

    #[test]
    fn panorama_row_render_matches_original_pixel_path() {
        let faces: Vec<_> = (0..6)
            .map(|face| {
                RgbaImage::from_fn(16, 16, |x, y| {
                    image::Rgba([
                        (x * 11 + face * 17) as u8,
                        (y * 13 + face * 29) as u8,
                        (x * y) as u8,
                        255,
                    ])
                })
            })
            .collect();
        let overlay = RgbaImage::from_fn(7, 5, |x, y| {
            image::Rgba([(x * 21) as u8, (y * 37) as u8, 63, (x * 17 + y * 13) as u8])
        });
        for (width, height) in [(7, 5), (511, 257), (641, 481), (900, 315)] {
            for seconds in [0.0_f32, 1.25, 46.0] {
                let mut expected = Canvas::new(width, height);
                let (sin, cos) = (seconds * (std::f32::consts::PI / 90.0)).sin_cos();
                let aspect = width as f32 / height as f32;
                for y in 0..height {
                    for x in 0..width {
                        let uv = [
                            (x as f32 + 0.5) / width as f32,
                            (y as f32 + 0.5) / height as f32,
                        ];
                        let (face, u, v) = panorama_face(panorama_direction(uv, aspect, sin, cos));
                        let p = sample_panorama(&faces, face, u, v);
                        expected.pixel(x as i32, y as i32, [p[0], p[1], p[2], 255]);
                    }
                }
                expected.image(
                    &overlay,
                    Rect {
                        x: 0,
                        y: 0,
                        w: width as i32,
                        h: height as i32,
                    },
                );
                let mut actual = Canvas::new(width, height);
                actual.panorama(&faces, &overlay, seconds);
                assert_eq!(
                    actual.pixels, expected.pixels,
                    "{width}x{height}, seconds {seconds}"
                );
            }
        }
    }

    #[test]
    #[ignore = "CPU measurement; compares two equivalent panorama samplers"]
    fn measure_panorama_interior_sampling() {
        use std::{hint::black_box, time::Instant};
        fn general(faces: &[RgbaImage], face: usize, u: f32, v: f32) -> [u8; 4] {
            let size = faces[face].width() as f32;
            let sx = u * size - 0.5;
            let sy = v * size - 0.5;
            let x = sx.floor() as i32;
            let y = sy.floor() as i32;
            let tx = sx - x as f32;
            let ty = sy - y as f32;
            let pixels = [
                panorama_texel(faces, face, x, y),
                panorama_texel(faces, face, x + 1, y),
                panorama_texel(faces, face, x, y + 1),
                panorama_texel(faces, face, x + 1, y + 1),
            ];
            std::array::from_fn(|channel| {
                let top = pixels[0][channel] * (1.0 - tx) + pixels[1][channel] * tx;
                let bottom = pixels[2][channel] * (1.0 - tx) + pixels[3][channel] * tx;
                (top * (1.0 - ty) + bottom * ty).round() as u8
            })
        }
        let faces: Vec<_> = (0..6)
            .map(|face| {
                RgbaImage::from_fn(512, 512, |x, y| {
                    image::Rgba([
                        (x + face * 31) as u8,
                        (y + face * 47) as u8,
                        (x + y) as u8,
                        255,
                    ])
                })
            })
            .collect();
        let mut samples = Vec::new();
        for yaw in [0.0_f32, 1.25] {
            let (sin, cos) = yaw.sin_cos();
            for y in 0..240 {
                for x in 0..427 {
                    let uv = [(x as f32 + 0.5) / 427.0, (y as f32 + 0.5) / 240.0];
                    samples.push(panorama_face(panorama_direction(
                        uv,
                        427.0 / 240.0,
                        sin,
                        cos,
                    )));
                }
            }
        }
        for &(face, u, v) in &samples {
            assert_eq!(
                sample_panorama(&faces, face, u, v),
                general(&faces, face, u, v)
            );
        }
        let mut times = [Vec::new(), Vec::new()];
        for iteration in 0..34 {
            for step in 0..2 {
                let mode = (step + iteration) % 2;
                let started = Instant::now();
                for &(face, u, v) in &samples {
                    let pixel = if mode == 0 {
                        general(&faces, face, u, v)
                    } else {
                        sample_panorama(&faces, face, u, v)
                    };
                    black_box(pixel);
                }
                if iteration >= 4 {
                    times[mode].push(started.elapsed());
                }
            }
        }
        for values in &mut times {
            values.sort_unstable();
        }
        println!(
            "panorama sampling {} points: general median/p95 {:?}/{:?}, interior path {:?}/{:?}",
            samples.len(),
            times[0][15],
            times[0][28],
            times[1][15],
            times[1][28]
        );
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn panorama_projection_and_menu_fps_match_vanilla() {
        use std::{
            ffi::OsString,
            path::{Path, PathBuf},
            process::Command,
        };

        // CubeMap.render uses this projection and rotation sequence. JOML
        // unprojects each screen coordinate independently of the Rust shader.
        const JAVA: &str = r#"
import java.nio.file.*;
import com.google.gson.*;
import org.joml.*;
import com.mojang.blaze3d.platform.FramerateLimitTracker;
import sun.misc.Unsafe;
public class PanoramaOracle {
  public static class MenuTracker extends FramerateLimitTracker {
    public MenuTracker() { super(null, null); }
    public FramerateThrottleReason getThrottleReason() {
      return FramerateThrottleReason.OUT_OF_LEVEL_MENU;
    }
  }
  public static void main(String[] args) throws Exception {
    var out = new JsonArray();
    float deg = 0.017453292f;
    for (float aspect : new float[]{1f, 4f/3f, 16f/9f})
      for (float seconds : new float[]{0f, 4f, 17.5f, 50f})
        for (float u : new float[]{0.1f, 0.35f, 0.5f, 0.85f, 0.95f})
          for (float v : new float[]{0.1f, 0.35f, 0.5f, 0.85f, 0.95f}) {
            var projection = new Matrix4f().setPerspective(85f*deg, aspect, 10f, 0.05f, false);
            var view = new Matrix4f().rotationX((float)java.lang.Math.PI).rotateX(10f*deg)
              .rotateY(-seconds*2f*deg);
            var direction = new Vector4f(2f*u-1f, 1f-2f*v, -1f, 1f)
              .mul(new Matrix4f(projection).mul(view).invert());
            var ray = new Vector3f(direction.x/direction.w, direction.y/direction.w,
              direction.z/direction.w).normalize();
            var row = new JsonArray(); row.add(ray.x); row.add(-ray.y); row.add(ray.z); out.add(row);
          }
    var field = Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
    var unsafe = (Unsafe)field.get(null);
    var tracker = (MenuTracker)unsafe.allocateInstance(MenuTracker.class);
    var limits = new JsonArray();
    for (int setting : new int[]{10, 30, 60, 120, 250, 260}) {
      tracker.setFramerateLimit(setting); limits.add(tracker.getFramerateLimit());
    }
    var result = new JsonObject(); result.add("directions", out); result.add("limits", limits);
    Files.writeString(Path.of(args[0]), new Gson().toJson(result));
  }
}
"#;
        fn classpath(game: &Path, classes: &Path) -> OsString {
            let version = game.join("versions/26.3");
            let metadata: serde_json::Value =
                serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
            let mut paths = vec![version.join("26.3.jar"), classes.to_path_buf()];
            for library in metadata["libraries"].as_array().unwrap() {
                if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                    let path = game.join("libraries").join(path);
                    if path.is_file() {
                        paths.push(path);
                    }
                }
            }
            std::env::join_paths(paths).unwrap()
        }
        fn run(command: &mut Command) {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "panorama oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join(
                    "AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon",
                )
            })
            .join("bin");
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("PanoramaOracle.java");
        let output = temp.path().join("panorama.json");
        std::fs::write(&source, JAVA).unwrap();
        let cp = classpath(&game, temp.path());
        run(Command::new(java.join("javac.exe"))
            .arg("-encoding")
            .arg("UTF-8")
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("PanoramaOracle")
            .arg(&output));
        let oracle: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let expected: Vec<[f32; 3]> = serde_json::from_value(oracle["directions"].clone()).unwrap();
        let limits: Vec<u64> = serde_json::from_value(oracle["limits"].clone()).unwrap();
        for limit in limits {
            assert_eq!(
                crate::menu::MENU_FRAME_INTERVAL,
                std::time::Duration::from_nanos(1_000_000_000 / limit)
            );
        }
        let mut index = 0;
        for aspect in [1.0, 4.0 / 3.0, 16.0 / 9.0] {
            for seconds in [0.0, 4.0, 17.5, 50.0] {
                let (sin, cos) = (seconds * (std::f32::consts::PI / 90.0)).sin_cos();
                for u in [0.1, 0.35, 0.5, 0.85, 0.95] {
                    for v in [0.1, 0.35, 0.5, 0.85, 0.95] {
                        let actual =
                            glam::Vec3::from_array(panorama_direction([u, v], aspect, sin, cos))
                                .normalize();
                        let reference = glam::Vec3::from_array(expected[index]);
                        assert!(
                            (actual - reference).length() < 0.000_01,
                            "aspect={aspect} seconds={seconds} uv={u},{v}: {actual:?} != {reference:?}",
                        );
                        index += 1;
                    }
                }
            }
        }
        assert_eq!(index, expected.len());
    }

    #[test]
    fn gui_sprite_nine_slice_repeats_inner_pixels_and_keeps_borders() {
        let image = RgbaImage::from_fn(6, 6, |x, y| {
            image::Rgba([x as u8 * 30, y as u8 * 30, 0, 255])
        });
        let mut canvas = Canvas::new(11, 9);
        let rect = Rect {
            x: 0,
            y: 0,
            w: 11,
            h: 9,
        };
        canvas.gui_sprite(
            &image,
            rect,
            GuiSpriteScaling::NineSlice {
                width: 6,
                height: 6,
                border: [1; 4],
                stretch_inner: false,
            },
        );
        let pixel = |canvas: &Canvas, x: usize, y: usize| canvas.pixels[y * 11 + x];
        assert_eq!(pixel(&canvas, 0, 0), 0xff00_0000);
        assert_eq!(pixel(&canvas, 10, 8), 0xff96_9600);
        assert_eq!(pixel(&canvas, 1, 1), 0xff1e_1e00);
        assert_eq!(pixel(&canvas, 5, 5), pixel(&canvas, 1, 1));
        assert_eq!(pixel(&canvas, 4, 4), 0xff78_7800);
        let mut tiled = Canvas::new(11, 9);
        tiled.gui_sprite(
            &image,
            rect,
            GuiSpriteScaling::Tile {
                width: 6,
                height: 6,
            },
        );
        assert_eq!(pixel(&tiled, 6, 6), pixel(&tiled, 0, 0));
        let mut stretched = Canvas::new(11, 9);
        stretched.gui_sprite(
            &image,
            rect,
            GuiSpriteScaling::NineSlice {
                width: 6,
                height: 6,
                border: [1; 4],
                stretch_inner: true,
            },
        );
        assert_eq!(pixel(&stretched, 3, 1), pixel(&stretched, 1, 1));
        assert_ne!(pixel(&stretched, 5, 5), pixel(&canvas, 5, 5));
    }

    #[test]
    fn cached_ingredient_icon_scaling_preserves_translucent_pixels() {
        let icon = RgbaImage::from_fn(16, 16, |x, y| {
            image::Rgba([
                (x * 15) as u8,
                (y * 15) as u8,
                170,
                ((x * 17 + y * 13) % 256) as u8,
            ])
        });
        let mut direct = Canvas::new(6, 6);
        let mut fallback = Canvas::new(6, 6);
        direct.fill(
            Rect {
                x: 0,
                y: 0,
                w: 6,
                h: 6,
            },
            [80, 90, 100, 255],
        );
        fallback.fill(
            Rect {
                x: 0,
                y: 0,
                w: 6,
                h: 6,
            },
            [80, 90, 100, 255],
        );
        direct.image(
            &icon,
            Rect {
                x: 0,
                y: 0,
                w: 6,
                h: 6,
            },
        );
        let mut full_size = Canvas::new(16, 16);
        full_size.image(
            &icon,
            Rect {
                x: 0,
                y: 0,
                w: 16,
                h: 16,
            },
        );
        for y in 0..6 {
            for x in 0..6 {
                let pixel = full_size.pixels[((y * 16 / 6) * 16 + x * 16 / 6) as usize];
                fallback.pixel(
                    x,
                    y,
                    [
                        (pixel >> 16) as u8,
                        (pixel >> 8) as u8,
                        pixel as u8,
                        (pixel >> 24) as u8,
                    ],
                );
            }
        }
        assert_eq!(direct.pixels, fallback.pixels);
    }

    #[test]
    fn rolling_blur_preserves_the_original_kernel_at_edges() {
        for radius in [1_i32, 2, 3, 5, 10] {
            for (width, height) in [(0_u32, 0_u32), (1, 1), (2, 7), (7, 2), (3, 4), (25, 19)] {
                let mut canvas = Canvas::new(width, height);
                for (index, pixel) in canvas.pixels.iter_mut().enumerate() {
                    let seed = index as u32 * 0x1f3d;
                    *pixel = 0xff00_0000 | (seed.wrapping_mul(0x12f01) & 0x00ff_ffff);
                }
                let mut horizontal = canvas.pixels.clone();
                let mut reference = canvas.pixels.clone();
                for y in 0..height as i32 {
                    for x in 0..width as i32 {
                        let mut sums = [0_u32; 3];
                        for offset in -radius..=radius {
                            let pixel = canvas.pixels[(y * width as i32
                                + (x + offset).clamp(0, width as i32 - 1))
                                as usize];
                            sums[0] += (pixel >> 16) & 255;
                            sums[1] += (pixel >> 8) & 255;
                            sums[2] += pixel & 255;
                        }
                        let count = (radius * 2 + 1) as u32;
                        horizontal[(y * width as i32 + x) as usize] = 0xff00_0000
                            | ((sums[0] / count) << 16)
                            | ((sums[1] / count) << 8)
                            | (sums[2] / count);
                    }
                }
                for y in 0..height as i32 {
                    for x in 0..width as i32 {
                        let mut sums = [0_u32; 3];
                        for offset in -radius..=radius {
                            let pixel = horizontal[((y + offset).clamp(0, height as i32 - 1)
                                * width as i32
                                + x) as usize];
                            sums[0] += (pixel >> 16) & 255;
                            sums[1] += (pixel >> 8) & 255;
                            sums[2] += pixel & 255;
                        }
                        let count = (radius * 2 + 1) as u32;
                        reference[(y * width as i32 + x) as usize] = 0xff00_0000
                            | ((sums[0] / count) << 16)
                            | ((sums[1] / count) << 8)
                            | (sums[2] / count);
                    }
                }
                canvas.blur_once(radius);
                assert_eq!(
                    canvas.pixels, reference,
                    "{width} x {height}, radius {radius}"
                );
            }
        }
    }

    #[test]
    fn blur_reuses_working_memory_without_changing_the_three_pass_result() {
        for radius in [0, 1, 3, 5, 10, 255] {
            for (width, height) in [(0, 0), (1, 1), (2, 7), (7, 2), (25, 19)] {
                let mut actual = Canvas::new(width, height);
                for (index, pixel) in actual.pixels.iter_mut().enumerate() {
                    *pixel = 0xff00_0000 | ((index as u32).wrapping_mul(0x12f01) & 0x00ff_ffff);
                }
                let mut expected = Canvas::new(width, height);
                expected.pixels.clone_from(&actual.pixels);
                if radius != 0 {
                    for _ in 0..3 {
                        expected.blur_once(i32::from(radius.min(10)));
                    }
                }
                actual.blur_radius(radius);
                assert_eq!(
                    actual.pixels, expected.pixels,
                    "{width} x {height}, radius {radius}"
                );
            }
        }
    }

    #[test]
    #[ignore = "local performance measurement"]
    fn measure_software_blur_1080p() {
        use std::time::Instant;

        let mut canvas = Canvas::new(1920, 1080);
        for (index, pixel) in canvas.pixels.iter_mut().enumerate() {
            *pixel = 0xff00_0000 | ((index as u32).wrapping_mul(0x12f01) & 0x00ff_ffff);
        }
        canvas.blur_radius(5);
        let mut times = Vec::new();
        for _ in 0..7 {
            let start = Instant::now();
            canvas.blur_radius(5);
            std::hint::black_box(&canvas.pixels);
            times.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(f64::total_cmp);
        eprintln!(
            "1080p radius-5 blur median {:.3} ms; samples {times:?}",
            times[3]
        );
    }

    #[test]
    #[ignore = "local performance measurement"]
    fn measure_software_panorama_1080p() {
        use std::time::Instant;
        let faces: Vec<_> = (0..6)
            .map(|face| {
                RgbaImage::from_fn(256, 256, |x, y| {
                    image::Rgba([
                        (x + face * 31) as u8,
                        (y + face * 47) as u8,
                        (x + y) as u8,
                        255,
                    ])
                })
            })
            .collect();
        let overlay = RgbaImage::from_pixel(16, 16, image::Rgba([8, 12, 20, 40]));
        let mut canvas = Canvas::new(1920, 1080);
        canvas.panorama(&faces, &overlay, 2.0);
        let mut times = Vec::new();
        for _ in 0..7 {
            let start = Instant::now();
            canvas.panorama(&faces, &overlay, 2.0);
            std::hint::black_box(&canvas.pixels);
            times.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(f64::total_cmp);
        eprintln!(
            "1080p panorama median {:.3} ms; samples {times:?}",
            times[3]
        );
    }

    #[test]
    fn overlay_pixels_preserve_alpha_and_compose_without_color_overflow() {
        let mut canvas = Canvas::new(1, 1);
        canvas.pixel(0, 0, [255, 0, 0, 128]);
        assert_eq!(canvas.pixels[0], 0x80ff_0000);
        canvas.pixel(0, 0, [0, 0, 255, 128]);
        let pixel = canvas.pixels[0];
        assert_eq!(pixel >> 24, 192);
        assert!(((pixel >> 16) & 255) >= 84 && ((pixel >> 16) & 255) <= 86);
        assert!((pixel & 255) >= 169 && (pixel & 255) <= 171);
    }
}
