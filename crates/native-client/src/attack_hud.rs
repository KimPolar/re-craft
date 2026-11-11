//! Java 26.3's crosshair attack indicator layout and sprite clipping.

use native_session::GameMode;

use crate::{
    menu_assets::{GuiSprite, HudSprites},
    menu_draw::{Canvas, Rect},
    menu_settings::AttackIndicator,
};

/// Cache the number of visible pixels rather than invalidating the HUD for
/// every change to the underlying floating-point charge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Display {
    width: Option<u8>,
    full: bool,
    mode: AttackIndicator,
}

/// A living target already selected by the game's crosshair picker.
#[derive(Clone, Copy, Debug)]
pub struct LivingTarget {
    pub alive: bool,
    pub within_item_range: bool,
    pub attack_delay: f32,
}

impl Display {
    pub fn new(strength: Option<f32>, mode: GameMode, setting: AttackIndicator) -> Self {
        Self::with_target(strength, mode, setting, None)
    }

    pub fn with_target(
        strength: Option<f32>,
        mode: GameMode,
        setting: AttackIndicator,
        target: Option<LivingTarget>,
    ) -> Self {
        Self {
            width: strength
                .filter(|strength| strength.is_finite() && *strength < 1.0)
                .filter(|_| mode != GameMode::Spectator)
                .filter(|_| setting != AttackIndicator::Off)
                .map(|strength| match setting {
                    AttackIndicator::Hotbar => ((strength.clamp(0.0, 1.0) * 19.0) as u8).min(18),
                    _ => ((strength.clamp(0.0, 1.0) * 17.0) as u8).min(16),
                }),
            full: setting == AttackIndicator::Crosshair
                && mode != GameMode::Spectator
                && strength.is_some_and(|strength| strength.is_finite() && strength >= 1.0)
                && target.is_some_and(|target| {
                    target.alive && target.within_item_range && target.attack_delay > 5.0
                }),
            mode: setting,
        }
    }
}

/// Restrict the two inverse passes to the indicator's physical pixels.
pub fn origin(size: (u32, u32)) -> (i32, i32) {
    (size.0 as i32 / 2 - 8, size.1 as i32 / 2 + 9)
}

pub fn scissor(
    size: (u32, u32),
    scale: u32,
    framebuffer: (u32, u32),
) -> Option<(u32, u32, u32, u32)> {
    let scale = i64::from(scale.max(1));
    let left = (i64::from(size.0 / 2) - 8) * scale;
    let top = (i64::from(size.1 / 2) + 9) * scale;
    let right = (left + 16 * scale).clamp(0, i64::from(framebuffer.0));
    let bottom = (top + 16 * scale).clamp(0, i64::from(framebuffer.1));
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
pub fn crosshair_layers(sprites: &HudSprites, display: Display) -> Option<(Canvas, Canvas)> {
    if display.mode != AttackIndicator::Crosshair {
        return None;
    }
    if display.full {
        let mut background = Canvas::new(16, 16);
        background.gui_sprite(
            &sprites.crosshair_attack_full.image,
            Rect {
                x: 0,
                y: 0,
                w: 16,
                h: 16,
            },
            sprites.crosshair_attack_full.scaling,
        );
        return Some((background, Canvas::new(16, 16)));
    }
    layers(
        &sprites.crosshair_attack_background,
        &sprites.crosshair_attack_progress,
        display,
    )
}

pub fn draw_hotbar(
    canvas: &mut Canvas,
    sprites: &HudSprites,
    display: Display,
    main_hand_left: bool,
) {
    hotbar(
        canvas,
        &sprites.hotbar_attack_background,
        &sprites.hotbar_attack_progress,
        display,
        main_hand_left,
    );
}

fn hotbar(
    canvas: &mut Canvas,
    background: &GuiSprite,
    progress: &GuiSprite,
    display: Display,
    main_hand_left: bool,
) {
    if display.mode != AttackIndicator::Hotbar {
        return;
    }
    let Some(height) = display.width.map(i32::from) else {
        return;
    };
    let Rect { x, y, .. } = hotbar_rect(canvas.width, canvas.height, main_hand_left);
    canvas.gui_sprite(
        &background.image,
        Rect { x, y, w: 18, h: 18 },
        background.scaling,
    );
    let mut full = Canvas::new(18, 18);
    full.gui_sprite(
        &progress.image,
        Rect {
            x: 0,
            y: 0,
            w: 18,
            h: 18,
        },
        progress.scaling,
    );
    for sy in (18 - height)..18 {
        for sx in 0..18 {
            let color = full.pixels[(sy * 18 + sx) as usize];
            canvas.pixel(
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
}

fn hotbar_rect(width: i32, height: i32, main_hand_left: bool) -> Rect {
    Rect {
        x: width / 2 + if main_hand_left { -113 } else { 97 },
        y: height - 20,
        w: 18,
        h: 18,
    }
}

fn layers(
    background_sprite: &GuiSprite,
    progress_sprite: &GuiSprite,
    display: Display,
) -> Option<(Canvas, Canvas)> {
    let width = i32::from(display.width?);
    let mut background = Canvas::new(16, 16);
    let mut progress = Canvas::new(16, 16);
    let (x, y) = (0, 0);
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

    fn crosshair(strength: Option<f32>, mode: GameMode) -> Display {
        Display::new(strength, mode, AttackIndicator::Crosshair)
    }

    #[test]
    fn charge_is_cached_by_java_pixel_width_and_hidden_at_full() {
        assert_eq!(crosshair(Some(0.0), GameMode::Survival).width, Some(0));
        assert_eq!(crosshair(Some(0.5), GameMode::Creative).width, Some(8));
        assert_eq!(crosshair(Some(0.999), GameMode::Survival).width, Some(16));
        assert_eq!(crosshair(Some(1.0), GameMode::Survival).width, None);
        assert_eq!(crosshair(None, GameMode::Survival).width, None);
        assert_eq!(crosshair(Some(0.5), GameMode::Spectator).width, None);
        assert_eq!(
            Display::new(Some(0.5), GameMode::Survival, AttackIndicator::Off).width,
            None
        );
        assert_eq!(
            Display::new(Some(0.5), GameMode::Survival, AttackIndicator::Hotbar).width,
            Some(9)
        );
    }

    #[test]
    fn full_indicator_requires_a_live_target_delay_and_item_reach() {
        let target = LivingTarget {
            alive: true,
            within_item_range: true,
            attack_delay: 5.01,
        };
        let show = |strength, mode, setting, target| {
            Display::with_target(strength, mode, setting, target).full
        };
        assert!(show(
            Some(1.0),
            GameMode::Survival,
            AttackIndicator::Crosshair,
            Some(target)
        ));
        assert!(!show(
            Some(0.999),
            GameMode::Survival,
            AttackIndicator::Crosshair,
            Some(target)
        ));
        assert!(!show(
            Some(1.0),
            GameMode::Survival,
            AttackIndicator::Crosshair,
            None
        ));
        assert!(!show(
            Some(1.0),
            GameMode::Survival,
            AttackIndicator::Crosshair,
            Some(LivingTarget {
                alive: false,
                ..target
            })
        ));
        assert!(!show(
            Some(1.0),
            GameMode::Survival,
            AttackIndicator::Crosshair,
            Some(LivingTarget {
                within_item_range: false,
                ..target
            })
        ));
        assert!(!show(
            Some(1.0),
            GameMode::Survival,
            AttackIndicator::Crosshair,
            Some(LivingTarget {
                attack_delay: 5.0,
                ..target
            })
        ));
        assert!(!show(
            Some(1.0),
            GameMode::Spectator,
            AttackIndicator::Crosshair,
            Some(target)
        ));
        assert!(!show(
            Some(1.0),
            GameMode::Survival,
            AttackIndicator::Hotbar,
            Some(target)
        ));
    }

    #[test]
    fn scissor_uses_gui_scale_and_clips_to_the_framebuffer() {
        assert_eq!(
            scissor((427, 240), 3, (1280, 720)),
            Some((615, 387, 48, 48))
        );
        assert_eq!(scissor((8, 4), 1, (8, 4)), None);
        assert_eq!(scissor((10, 20), 1, (10, 20)), Some((0, 19, 10, 1)));
    }

    #[test]
    fn hotbar_indicator_uses_opposite_arm_and_fills_upward() {
        use crate::menu_assets::GuiSpriteScaling;
        let background = GuiSprite {
            image: image::RgbaImage::from_pixel(18, 18, image::Rgba([16, 32, 48, 255])),
            scaling: GuiSpriteScaling::Stretch,
        };
        let progress = GuiSprite {
            image: image::RgbaImage::from_fn(18, 18, |_, y| image::Rgba([y as u8, 128, 64, 255])),
            scaling: GuiSpriteScaling::Stretch,
        };
        let display = Display::new(Some(0.5), GameMode::Survival, AttackIndicator::Hotbar);
        for left in [false, true] {
            let mut canvas = Canvas::new(426, 240);
            hotbar(&mut canvas, &background, &progress, display, left);
            let x = 213 + if left { -113 } else { 97 };
            let at =
                |dx: i32, dy: i32| canvas.pixels[((220 + dy) * canvas.width + x + dx) as usize];
            assert_eq!(at(0, 8), 0xff10_2030);
            assert_eq!(at(0, 9), 0xff09_8040);
            assert_eq!(at(17, 17), 0xff11_8040);
            assert_eq!(at(18, 17), 0);
        }
        let mut hidden = Canvas::new(426, 240);
        hotbar(
            &mut hidden,
            &background,
            &progress,
            Display::new(Some(0.5), GameMode::Survival, AttackIndicator::Off),
            false,
        );
        assert!(hidden.pixels.iter().all(|pixel| *pixel == 0));
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn hotbar_indicator_layout_matches_installed_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
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
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("AttackIndicatorHudOracle.java");
        let output = temp.path().join("output.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/AttackIndicatorHudOracle.java"),
        )
        .unwrap();
        let run = |cmd: &mut Command| {
            let result = cmd.output().unwrap();
            assert!(
                result.status.success(),
                "Java attack HUD oracle failed:\n{}\n{}",
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
            .arg("AttackIndicatorHudOracle")
            .arg(&output));
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
        assert_eq!(rows.len(), 36);
        for row in rows {
            let width = row["w"].as_i64().unwrap() as i32;
            let window_height = row["h"].as_i64().unwrap() as i32;
            let left = row["left"].as_bool().unwrap();
            let strength = row["charge"].as_f64().unwrap() as f32;
            let display = Display::new(Some(strength), GameMode::Survival, AttackIndicator::Hotbar);
            let mut expected = Vec::new();
            if let Some(height) = display.width.map(i32::from) {
                let rect = hotbar_rect(width, window_height, left);
                expected.push(serde_json::json!([
                    "hud/hotbar_attack_indicator_background",
                    rect.x,
                    rect.y,
                    18,
                    18
                ]));
                expected.push(serde_json::json!([
                    "hud/hotbar_attack_indicator_progress",
                    18,
                    18,
                    0,
                    18 - height,
                    rect.x,
                    rect.y + 18 - height,
                    18,
                    height
                ]));
            }
            assert_eq!(serde_json::json!(expected), row["draws"], "{row}");
        }

        let source = temp.path().join("CrosshairAttackHudOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/CrosshairAttackHudOracle.java"),
        )
        .unwrap();
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&classpath)
            .arg(&source));
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&classpath)
            .arg("CrosshairAttackHudOracle")
            .arg(&output));
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), 192);
        for row in rows {
            let width = row["w"].as_i64().unwrap() as i32;
            let height = row["h"].as_i64().unwrap() as i32;
            let strength = row["charge"].as_f64().unwrap() as f32;
            let selected = row["selected"].as_bool().unwrap();
            let target = selected.then_some(LivingTarget {
                alive: row["alive"].as_bool().unwrap(),
                within_item_range: row["reach"].as_i64().unwrap() != 2,
                attack_delay: row["delay"].as_f64().unwrap() as f32,
            });
            let display = Display::with_target(
                Some(strength),
                GameMode::Survival,
                AttackIndicator::Crosshair,
                target,
            );
            let (x, y) = (width / 2 - 8, height / 2 + 9);
            let mut expected = Vec::new();
            if display.full {
                expected.push(serde_json::json!([
                    "hud/crosshair_attack_indicator_full",
                    x,
                    y,
                    16,
                    16
                ]));
            } else if let Some(progress) = display.width {
                expected.push(serde_json::json!([
                    "hud/crosshair_attack_indicator_background",
                    x,
                    y,
                    16,
                    4
                ]));
                expected.push(serde_json::json!([
                    "hud/crosshair_attack_indicator_progress",
                    16,
                    4,
                    0,
                    0,
                    x,
                    y,
                    progress,
                    4
                ]));
            }
            assert_eq!(serde_json::json!(expected), row["draws"], "{row}");
        }
    }

    #[test]
    fn separate_compact_sprites_clip_progress() {
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
            &background,
            &progress,
            crosshair(Some(0.5), GameMode::Survival),
        )
        .unwrap();
        let at = |x: usize, y: usize| y * 16 + x;
        assert_eq!((bg.width, bg.height), (16, 16));
        assert_eq!(bg.pixels[at(0, 0)], 0xff102030);
        assert_eq!(bg.pixels[at(0, 4)], 0);
        assert_eq!(fg.pixels[at(7, 3)], 0xffff8040);
        assert_eq!(fg.pixels[at(8, 3)], 0);
        assert_eq!(fg.pixels[at(0, 4)], 0);
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
        image::RgbaImage::from_pixel(16, 16, image::Rgba([12, 34, 56, 255]))
            .save(dir.join("crosshair_attack_indicator_full.png"))
            .unwrap();
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
        image::RgbaImage::from_pixel(18, 18, image::Rgba([10, 20, 30, 255]))
            .save(dir.join("hotbar_attack_indicator_background.png"))
            .unwrap();
        image::RgbaImage::from_fn(18, 3, |_, y| image::Rgba([40 + y as u8 * 50, 60, 80, 255]))
            .save(dir.join("hotbar_attack_indicator_progress.png"))
            .unwrap();
        std::fs::write(
            dir.join("hotbar_attack_indicator_progress.png.mcmeta"),
            br#"{"gui":{"scaling":{"type":"tile","width":18,"height":3}}}"#,
        )
        .unwrap();
        let assets =
            crate::menu_assets::MenuAssets::load(&game, &[pack.path().to_path_buf()]).unwrap();
        let display = Display::with_target(
            Some(1.0),
            GameMode::Survival,
            AttackIndicator::Crosshair,
            Some(LivingTarget {
                alive: true,
                within_item_range: true,
                attack_delay: 6.0,
            }),
        );
        let (full, empty) = crosshair_layers(&assets.hud, display).unwrap();
        assert_eq!(full.pixels[15 * 16 + 15], 0xff0c_2238);
        assert!(empty.pixels.iter().all(|pixel| *pixel == 0));
        let (background, progress) =
            crosshair_layers(&assets.hud, crosshair(Some(0.5), GameMode::Survival)).unwrap();
        for sx in 0..16 {
            let index = (2 * 16 + sx) as usize;
            assert_eq!(background.pixels[index], 0xff14_1e28);
            let expected = if sx < 8 {
                0xff00_0000 | ((50 + (sx % 4) * 30) as u32) << 16 | 90 << 8 | 140
            } else {
                0
            };
            assert_eq!(progress.pixels[index], expected, "pixel {sx}");
        }
        let mut canvas = Canvas::new(426, 240);
        draw_hotbar(
            &mut canvas,
            &assets.hud,
            Display::new(Some(0.5), GameMode::Survival, AttackIndicator::Hotbar),
            false,
        );
        for sy in 0..18 {
            let index = ((220 + sy) * 426 + 310) as usize;
            let expected = if sy >= 9 {
                0xff00_0000 | ((40 + (sy % 3) * 50) as u32) << 16 | 60 << 8 | 80
            } else {
                0xff0a_141e
            };
            assert_eq!(canvas.pixels[index], expected, "row {sy}");
        }
    }
}
