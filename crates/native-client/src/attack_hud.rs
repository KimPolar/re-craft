//! Java 26.3's crosshair attack indicator layout and sprite clipping.

use native_session::GameMode;

use crate::{
    menu_assets::{GuiSprite, HudSprites},
    menu_draw::{Canvas, Rect},
};

/// Cache the number of visible pixels rather than invalidating the HUD for
/// every change to the underlying floating-point charge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Display {
    width: Option<u8>,
}

impl Display {
    pub fn new(strength: Option<f32>, mode: GameMode) -> Self {
        Self {
            width: strength
                .filter(|strength| strength.is_finite() && *strength < 1.0)
                .filter(|_| mode != GameMode::Spectator)
                .map(|strength| ((strength.clamp(0.0, 1.0) * 17.0) as u8).min(16)),
        }
    }
}

/// Restrict the two inverse passes to the indicator's physical pixels.
pub fn scissor(
    size: (u32, u32),
    scale: u32,
    framebuffer: (u32, u32),
) -> Option<(u32, u32, u32, u32)> {
    let scale = i64::from(scale.max(1));
    let left = (i64::from(size.0 / 2) - 8) * scale;
    let top = (i64::from(size.1 / 2) + 9) * scale;
    let right = (left + 16 * scale).clamp(0, i64::from(framebuffer.0));
    let bottom = (top + 4 * scale).clamp(0, i64::from(framebuffer.1));
    let left = left.clamp(0, i64::from(framebuffer.0));
    let top = top.clamp(0, i64::from(framebuffer.1));
    (right > left && bottom > top).then_some((
        left as u32,
        top as u32,
        (right - left) as u32,
        (bottom - top) as u32,
    ))
}

/// Return separate layers: Java uses inverted blending for each sprite, so
/// flattening them with ordinary alpha compositing would change their colors.
pub fn crosshair_layers(
    size: (u32, u32),
    sprites: &HudSprites,
    display: Display,
) -> Option<(Canvas, Canvas)> {
    layers(
        size,
        &sprites.crosshair_attack_background,
        &sprites.crosshair_attack_progress,
        display,
    )
}

fn layers(
    size: (u32, u32),
    background_sprite: &GuiSprite,
    progress_sprite: &GuiSprite,
    display: Display,
) -> Option<(Canvas, Canvas)> {
    let width = i32::from(display.width?);
    let mut background = Canvas::new(size.0, size.1);
    let mut progress = Canvas::new(size.0, size.1);
    let x = background.width / 2 - 8;
    let y = background.height / 2 + 9;
    background.gui_sprite(
        &background_sprite.image,
        Rect { x, y, w: 16, h: 4 },
        background_sprite.scaling,
    );
    let mut full = Canvas::new(16, 4);
    full.gui_sprite(
        &progress_sprite.image,
        Rect {
            x: 0,
            y: 0,
            w: 16,
            h: 4,
        },
        progress_sprite.scaling,
    );
    for sy in 0..4 {
        for sx in 0..width {
            let color = full.pixels[(sy * 16 + sx) as usize];
            progress.pixel(
                x + sx,
                y + sy,
                [
                    (color >> 16) as u8,
                    (color >> 8) as u8,
                    color as u8,
                    (color >> 24) as u8,
                ],
            );
        }
    }
    Some((background, progress))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charge_is_cached_by_java_pixel_width_and_hidden_at_full() {
        assert_eq!(Display::new(Some(0.0), GameMode::Survival).width, Some(0));
        assert_eq!(Display::new(Some(0.5), GameMode::Creative).width, Some(8));
        assert_eq!(
            Display::new(Some(0.999), GameMode::Survival).width,
            Some(16)
        );
        assert_eq!(Display::new(Some(1.0), GameMode::Survival).width, None);
        assert_eq!(Display::new(None, GameMode::Survival).width, None);
        assert_eq!(Display::new(Some(0.5), GameMode::Spectator).width, None);
    }

    #[test]
    fn scissor_uses_gui_scale_and_clips_to_the_framebuffer() {
        assert_eq!(
            scissor((427, 240), 3, (1280, 720)),
            Some((615, 387, 48, 12))
        );
        assert_eq!(scissor((8, 4), 1, (8, 4)), None);
        assert_eq!(scissor((10, 20), 1, (10, 20)), Some((0, 19, 10, 1)));
    }

    #[test]
    fn separate_sprites_use_java_coordinates_and_clip_progress() {
        use crate::menu_assets::GuiSpriteScaling;
        let background = GuiSprite {
            image: image::RgbaImage::from_pixel(16, 4, image::Rgba([16, 32, 48, 255])),
            scaling: GuiSpriteScaling::Stretch,
        };
        let progress = GuiSprite {
            image: image::RgbaImage::from_pixel(16, 4, image::Rgba([255, 128, 64, 255])),
            scaling: GuiSpriteScaling::Stretch,
        };
        let (bg, fg) = layers(
            (321, 241),
            &background,
            &progress,
            Display::new(Some(0.5), GameMode::Survival),
        )
        .unwrap();
        let at = |x: usize, y: usize| y * 321 + x;
        assert_eq!(bg.pixels[at(152, 129)], 0xff102030);
        assert_eq!(bg.pixels[at(168, 129)], 0);
        assert_eq!(fg.pixels[at(159, 132)], 0xffff8040);
        assert_eq!(fg.pixels[at(160, 132)], 0);
        assert_eq!(fg.pixels[at(152, 133)], 0);
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn selected_pack_overrides_indicator_sprites_and_scaling() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let pack = tempfile::tempdir().unwrap();
        let dir = pack
            .path()
            .join("assets/minecraft/textures/gui/sprites/hud");
        std::fs::create_dir_all(&dir).unwrap();
        image::RgbaImage::from_pixel(16, 4, image::Rgba([20, 30, 40, 255]))
            .save(dir.join("crosshair_attack_indicator_background.png"))
            .unwrap();
        image::RgbaImage::from_fn(4, 4, |x, _| image::Rgba([50 + x as u8 * 30, 90, 140, 255]))
            .save(dir.join("crosshair_attack_indicator_progress.png"))
            .unwrap();
        std::fs::write(
            dir.join("crosshair_attack_indicator_progress.png.mcmeta"),
            br#"{"gui":{"scaling":{"type":"tile","width":4,"height":4}}}"#,
        )
        .unwrap();
        let assets =
            crate::menu_assets::MenuAssets::load(&game, &[pack.path().to_path_buf()]).unwrap();
        let (background, progress) = crosshair_layers(
            (426, 240),
            &assets.hud,
            Display::new(Some(0.5), GameMode::Survival),
        )
        .unwrap();
        let (x, y) = (426 / 2 - 8, 240 / 2 + 9);
        for sx in 0..16 {
            let index = ((y + 2) * 426 + x + sx) as usize;
            assert_eq!(background.pixels[index], 0xff14_1e28);
            let expected = if sx < 8 {
                0xff00_0000 | ((50 + (sx % 4) * 30) as u32) << 16 | 90 << 8 | 140
            } else {
                0
            };
            assert_eq!(progress.pixels[index], expected, "pixel {sx}");
        }
    }
}
