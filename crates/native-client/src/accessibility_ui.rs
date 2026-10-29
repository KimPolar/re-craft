//! Java 26.3 accessibility options; unsupported entries remain disabled.
use crate::{
    menu_assets::MenuAssets,
    menu_draw::{Canvas, Rect},
    menu_settings::{UnitInterval, VideoSettings},
};

const TOP: i32 = 33;
const FOOTER: i32 = 33;
const CONTENT_HEIGHT: i32 = 329;

type Label = (&'static str, &'static str);
const ROWS: [(Label, Option<Label>); 13] = [
    (
        ("options.narrator", "Narrator"),
        Some(("options.controls", "Controls...")),
    ),
    (
        ("options.showSubtitles", "Show Subtitles"),
        Some(("options.accessibility.high_contrast", "High Contrast")),
    ),
    (
        (
            "options.accessibility.menu_background_blurriness",
            "Menu Background Blur",
        ),
        Some((
            "options.accessibility.text_background_opacity",
            "Text Background Opacity",
        )),
    ),
    (
        ("options.accessibility.text_background", "Text Background"),
        Some(("options.chat.opacity", "Chat Text Opacity")),
    ),
    (
        ("options.chat.line_spacing", "Chat Line Spacing"),
        Some(("options.chat.delay", "Chat Delay")),
    ),
    (
        ("options.notifications.display_time", "Notification Time"),
        Some(("options.viewBobbing", "View Bobbing")),
    ),
    (
        ("options.screenEffectScale", "Distortion Effects"),
        Some(("options.fovEffectScale", "FOV Effects")),
    ),
    (
        ("options.darknessEffectScale", "Darkness Effect"),
        Some(("options.damageTiltStrength", "Damage Tilt")),
    ),
    (
        ("options.glintSpeed", "Glint Speed"),
        Some(("options.glintStrength", "Glint Strength")),
    ),
    (
        ("options.hideLightningFlashes", "Hide Lightning Flashes"),
        Some((
            "options.darkMojangStudiosBackgroundColor",
            "Monochrome Logo",
        )),
    ),
    (
        (
            "options.accessibility.panorama_speed",
            "Panorama Scroll Speed",
        ),
        Some(("options.hideSplashTexts", "Hide Splash Texts")),
    ),
    (
        ("options.accessibility.narrator_hotkey", "Narrator Hotkey"),
        Some(("options.rotateWithMinecart", "Rotate with Minecart")),
    ),
    (
        (
            "options.accessibility.high_contrast_block_outline",
            "High Contrast Block Outlines",
        ),
        None,
    ),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessibilityControl {
    ViewBobbing,
    GlintSpeed,
    GlintStrength,
    PanoramaSpeed,
    MenuBlur,
}

#[derive(Default)]
pub struct AccessibilityUi {
    scroll: f64,
    dragging: Option<i32>,
    dragging_slider: Option<AccessibilityControl>,
    blur_slider: f64,
    pub keyboard_adjust: bool,
}

impl AccessibilityUi {
    fn viewport(height: i32) -> i32 {
        (height - TOP - FOOTER).max(0)
    }
    pub(crate) fn maximum(height: i32) -> i32 {
        (CONTENT_HEIGHT - Self::viewport(height)).max(0)
    }
    pub fn rect(&self, width: i32) -> Rect {
        self.control_rect(width, AccessibilityControl::ViewBobbing)
    }
    pub fn control_rect(&self, width: i32, control: AccessibilityControl) -> Rect {
        let (row, column) = match control {
            AccessibilityControl::ViewBobbing => (5, 1),
            AccessibilityControl::GlintSpeed => (8, 0),
            AccessibilityControl::GlintStrength => (8, 1),
            AccessibilityControl::PanoramaSpeed => (10, 0),
            AccessibilityControl::MenuBlur => (2, 0),
        };
        Rect {
            x: width / 2 - 155 + column * 160,
            y: 37 + row * 25 - self.scroll as i32,
            w: 150,
            h: 20,
        }
    }
    #[cfg(test)]
    pub fn visible(&self, height: i32) -> bool {
        self.control_visible(height, AccessibilityControl::ViewBobbing)
    }
    pub fn control_visible(&self, height: i32, control: AccessibilityControl) -> bool {
        let r = self.control_rect(0, control);
        r.y < height - FOOTER && r.y + r.h > TOP
    }
    pub fn hit(&self, width: i32, height: i32, cursor: (i32, i32)) -> bool {
        (TOP..height - FOOTER).contains(&cursor.1) && self.rect(width).contains(cursor.0, cursor.1)
    }
    pub fn hit_control(
        &self,
        width: i32,
        height: i32,
        cursor: (i32, i32),
    ) -> Option<AccessibilityControl> {
        (TOP..height - FOOTER)
            .contains(&cursor.1)
            .then(|| {
                [
                    AccessibilityControl::ViewBobbing,
                    AccessibilityControl::GlintSpeed,
                    AccessibilityControl::GlintStrength,
                    AccessibilityControl::PanoramaSpeed,
                    AccessibilityControl::MenuBlur,
                ]
                .into_iter()
                .find(|control| {
                    self.control_rect(width, *control)
                        .contains(cursor.0, cursor.1)
                })
            })
            .flatten()
    }
    pub fn ensure_control_visible(&mut self, height: i32, control: AccessibilityControl) {
        let y = self.control_rect(0, control).y;
        if y < TOP {
            self.scroll -= f64::from(TOP - y);
        }
        if y + 20 > height - FOOTER {
            self.scroll += f64::from(y + 20 - (height - FOOTER));
        }
        self.resize(height);
    }
    pub fn scroll_by(&mut self, delta: f64, height: i32) -> bool {
        let before = self.scroll;
        self.scroll = (self.scroll - delta * 12.0).clamp(0.0, f64::from(Self::maximum(height)));
        before != self.scroll
    }
    pub fn resize(&mut self, height: i32) {
        self.scroll = self.scroll.clamp(0.0, f64::from(Self::maximum(height)));
    }
    pub fn start_scrollbar(&mut self, width: i32, height: i32, cursor: (i32, i32)) -> bool {
        let x = width / 2 + 163;
        if Self::maximum(height) > 0
            && (x..x + 6).contains(&cursor.0)
            && (TOP..height - FOOTER).contains(&cursor.1)
        {
            self.dragging = Some(cursor.1);
            true
        } else {
            false
        }
    }
    pub fn pointer_move(&mut self, height: i32, y: i32) -> bool {
        let Some(previous) = self.dragging else {
            return false;
        };
        let viewport = Self::viewport(height);
        let thumb = ((viewport * viewport) / CONTENT_HEIGHT).clamp(32, (viewport - 8).max(32));
        let maximum = f64::from(Self::maximum(height));
        let range = (viewport - thumb).max(1);
        self.scroll = if y < TOP {
            0.0
        } else if y > height - FOOTER {
            maximum
        } else {
            (self.scroll + f64::from(y - previous) * (maximum / f64::from(range)).max(1.0))
                .clamp(0.0, maximum)
        };
        self.dragging = Some(y);
        true
    }
    pub fn start_slider(
        &mut self,
        width: i32,
        x: i32,
        control: AccessibilityControl,
        video: &mut VideoSettings,
    ) -> bool {
        self.dragging_slider = Some(control);
        self.keyboard_adjust = true;
        self.slider_to_x(width, x, control, video)
    }
    fn slider_to_x(
        &mut self,
        width: i32,
        x: i32,
        control: AccessibilityControl,
        video: &mut VideoSettings,
    ) -> bool {
        let rect = self.control_rect(width, control);
        let value = UnitInterval::new(f64::from(x - rect.x - 4) / f64::from(rect.w - 8));
        if control == AccessibilityControl::MenuBlur {
            self.blur_slider = value.value();
            let next = crate::blur_ui::from_slider(self.blur_slider);
            let changed = video.menu_blur != next;
            video.menu_blur = next;
            return changed;
        }
        let target = match control {
            AccessibilityControl::GlintSpeed => &mut video.glint_speed,
            AccessibilityControl::GlintStrength => &mut video.glint_strength,
            AccessibilityControl::PanoramaSpeed => &mut video.panorama_speed,
            AccessibilityControl::MenuBlur => unreachable!(),
            AccessibilityControl::ViewBobbing => return false,
        };
        let changed = *target != value;
        *target = value;
        changed
    }
    pub fn pointer_move_with_settings(
        &mut self,
        width: i32,
        height: i32,
        cursor: (i32, i32),
        video: &mut VideoSettings,
    ) -> (bool, bool) {
        if let Some(control) = self.dragging_slider {
            return (true, self.slider_to_x(width, cursor.0, control, video));
        }
        (self.pointer_move(height, cursor.1), false)
    }
    pub fn arrow(
        &mut self,
        control: AccessibilityControl,
        right: bool,
        video: &mut VideoSettings,
    ) -> bool {
        if control == AccessibilityControl::MenuBlur {
            let next = if right {
                video.menu_blur.saturating_add(1).min(10)
            } else {
                video.menu_blur.saturating_sub(1)
            };
            let changed = video.menu_blur != next;
            video.menu_blur = next;
            self.blur_slider = crate::blur_ui::to_slider(next);
            return changed;
        }
        let target = match control {
            AccessibilityControl::GlintSpeed => &mut video.glint_speed,
            AccessibilityControl::GlintStrength => &mut video.glint_strength,
            AccessibilityControl::PanoramaSpeed => &mut video.panorama_speed,
            AccessibilityControl::MenuBlur => unreachable!(),
            AccessibilityControl::ViewBobbing => return false,
        };
        // AbstractSliderButton uses a single-precision pixel step.
        let delta = if right { 1.0_f32 } else { -1.0_f32 } / 142.0_f32;
        let next = UnitInterval::new(target.value() + f64::from(delta));
        let changed = *target != next;
        *target = next;
        changed
    }
    pub fn release(&mut self) {
        self.dragging = None;
        self.dragging_slider = None;
    }

    pub fn draw(
        &self,
        canvas: &mut Canvas,
        assets: &MenuAssets,
        settings: VideoSettings,
        cursor: (i32, i32),
        focused: Option<AccessibilityControl>,
    ) {
        let (width, height) = (canvas.width, canvas.height);
        let viewport = Self::viewport(height);
        if viewport == 0 {
            return;
        }
        let mut list = Canvas::new(width as u32, viewport as u32);
        for (row, (left, right)) in ROWS.iter().enumerate() {
            for (column, label) in [Some(*left), *right].into_iter().enumerate() {
                let Some((key, fallback)) = label else {
                    continue;
                };
                let rect = Rect {
                    x: width / 2 - 155 + column as i32 * 160,
                    y: 37 + row as i32 * 25 - self.scroll as i32 - TOP,
                    w: 150,
                    h: 20,
                };
                let control = match key {
                    "options.viewBobbing" => Some(AccessibilityControl::ViewBobbing),
                    "options.glintSpeed" => Some(AccessibilityControl::GlintSpeed),
                    "options.glintStrength" => Some(AccessibilityControl::GlintStrength),
                    "options.accessibility.panorama_speed" => {
                        Some(AccessibilityControl::PanoramaSpeed)
                    }
                    "options.accessibility.menu_background_blurriness" => {
                        Some(AccessibilityControl::MenuBlur)
                    }
                    _ => None,
                };
                if control == Some(AccessibilityControl::MenuBlur) {
                    let highlighted =
                        self.hit_control(width, height, cursor) == control || focused == control;
                    let slider = if self.dragging_slider == control {
                        self.blur_slider
                    } else {
                        crate::blur_ui::to_slider(settings.menu_blur)
                    };
                    crate::blur_ui::draw(
                        &mut list,
                        assets,
                        rect,
                        settings.menu_blur,
                        slider,
                        highlighted,
                    );
                    continue;
                }
                if matches!(
                    control,
                    Some(
                        AccessibilityControl::GlintSpeed
                            | AccessibilityControl::GlintStrength
                            | AccessibilityControl::PanoramaSpeed
                    )
                ) {
                    let control = control.unwrap();
                    let value = match control {
                        AccessibilityControl::GlintSpeed => settings.glint_speed.value(),
                        AccessibilityControl::GlintStrength => settings.glint_strength.value(),
                        AccessibilityControl::PanoramaSpeed => settings.panorama_speed.value(),
                        AccessibilityControl::MenuBlur => unreachable!(),
                        AccessibilityControl::ViewBobbing => unreachable!(),
                    };
                    let highlighted = self.hit_control(width, height, cursor) == Some(control)
                        || focused == Some(control);
                    let background = if highlighted {
                        &assets.slider_highlighted
                    } else {
                        &assets.slider
                    };
                    let handle = if highlighted {
                        &assets.slider_handle_highlighted
                    } else {
                        &assets.slider_handle
                    };
                    list.gui_sprite(&background.image, rect, background.scaling);
                    list.gui_sprite(
                        &handle.image,
                        Rect {
                            x: rect.x + (value * f64::from(rect.w - 8)) as i32,
                            y: rect.y,
                            w: 8,
                            h: 20,
                        },
                        handle.scaling,
                    );
                    let name = assets.translate(key, fallback);
                    let text = if value == 0.0 && control != AccessibilityControl::PanoramaSpeed {
                        assets
                            .translate("options.generic_value", "%s: %s")
                            .replacen("%s", name, 1)
                            .replacen("%s", assets.translate("options.off", "OFF"), 1)
                    } else {
                        assets
                            .translate("options.percent_value", "%s: %s%%")
                            .replacen("%s", name, 1)
                            .replacen("%s", &((value * 100.0) as i32).to_string(), 1)
                            .replace("%%", "%")
                    };
                    list.text_centered(
                        Some(assets),
                        rect.x + rect.w / 2,
                        rect.y + 6,
                        &text,
                        [255, 255, 255],
                    );
                    continue;
                }
                let enabled = control == Some(AccessibilityControl::ViewBobbing);
                let text = if enabled {
                    let (key, fallback) = if settings.view_bobbing {
                        ("options.on.composed", "%s: ON")
                    } else {
                        ("options.off.composed", "%s: OFF")
                    };
                    assets.translate(key, fallback).replacen(
                        "%s",
                        assets.translate("options.viewBobbing", "View Bobbing"),
                        1,
                    )
                } else {
                    assets.translate(key, fallback).to_owned()
                };
                list.button(
                    Some(assets),
                    rect,
                    enabled,
                    enabled && (self.hit(width, height, cursor) || focused == control),
                    &text,
                );
            }
        }
        canvas.composite(&list, 0, TOP);
        let maximum = Self::maximum(height);
        if maximum > 0 {
            let x = width / 2 + 163;
            let thumb = ((viewport * viewport) / CONTENT_HEIGHT).clamp(32, (viewport - 8).max(32));
            let y = TOP + self.scroll as i32 * (viewport - thumb) / maximum;
            canvas.gui_sprite(
                &assets.scroller_background.image,
                Rect {
                    x,
                    y: TOP,
                    w: 6,
                    h: viewport,
                },
                assets.scroller_background.scaling,
            );
            canvas.gui_sprite(
                &assets.scroller.image,
                Rect {
                    x,
                    y,
                    w: 6,
                    h: thumb,
                },
                assets.scroller.scaling,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glint_sliders_follow_mouse_keyboard_and_scroll() {
        let mut ui = AccessibilityUi::default();
        let mut settings = VideoSettings::default();
        assert_eq!(settings.glint_speed.value(), 0.5);
        assert_eq!(settings.glint_strength.value(), 0.75);
        ui.scroll_by(-10.0, 240);
        let speed = ui.control_rect(426, AccessibilityControl::GlintSpeed);
        let strength = ui.control_rect(426, AccessibilityControl::GlintStrength);
        assert_eq!(
            ui.hit_control(426, 240, (speed.x + 5, speed.y + 5)),
            Some(AccessibilityControl::GlintSpeed)
        );
        assert_eq!(
            ui.hit_control(426, 240, (strength.x + 5, strength.y + 5)),
            Some(AccessibilityControl::GlintStrength)
        );
        assert!(ui.start_slider(
            426,
            speed.x + 4,
            AccessibilityControl::GlintSpeed,
            &mut settings
        ));
        assert_eq!(settings.glint_speed.value(), 0.0);
        let (dragged, changed) =
            ui.pointer_move_with_settings(426, 240, (speed.x + 146, speed.y + 5), &mut settings);
        assert!(dragged && changed);
        assert_eq!(settings.glint_speed.value(), 1.0);
        assert!(ui.arrow(AccessibilityControl::GlintSpeed, false, &mut settings));
        assert_eq!(
            settings.glint_speed.value(),
            1.0 + f64::from(-1.0_f32 / 142.0)
        );
        ui.release();
        assert!(
            !ui.pointer_move_with_settings(426, 240, (speed.x + 4, speed.y), &mut settings)
                .0
        );
    }

    #[test]
    fn panorama_slider_uses_vanilla_row_and_changes_speed() {
        let mut ui = AccessibilityUi::default();
        let mut settings = VideoSettings::default();
        assert_eq!(settings.panorama_speed.value(), 1.0);
        ui.ensure_control_visible(240, AccessibilityControl::PanoramaSpeed);
        let rect = ui.control_rect(426, AccessibilityControl::PanoramaSpeed);
        assert_eq!(
            ui.hit_control(426, 240, (rect.x + 4, rect.y + 4)),
            Some(AccessibilityControl::PanoramaSpeed)
        );
        assert!(ui.start_slider(
            426,
            rect.x + 4,
            AccessibilityControl::PanoramaSpeed,
            &mut settings
        ));
        assert_eq!(settings.panorama_speed.value(), 0.0);
        assert!(ui.arrow(AccessibilityControl::PanoramaSpeed, true, &mut settings));
        assert_eq!(settings.panorama_speed.value(), f64::from(1.0_f32 / 142.0));
        assert!(
            ui.pointer_move_with_settings(426, 240, (rect.x + 146, rect.y), &mut settings)
                .1
        );
        assert_eq!(settings.panorama_speed.value(), 1.0);
    }

    #[test]
    fn blur_slider_handles_extremes_and_keyboard() {
        let mut ui = AccessibilityUi::default();
        let mut settings = VideoSettings::default();
        let rect = ui.control_rect(426, AccessibilityControl::MenuBlur);
        assert!(ui.start_slider(
            426,
            rect.x + 4,
            AccessibilityControl::MenuBlur,
            &mut settings
        ));
        assert_eq!(settings.menu_blur, 0);
        assert!(ui.arrow(AccessibilityControl::MenuBlur, true, &mut settings));
        assert_eq!(settings.menu_blur, 1);
        assert!(
            ui.pointer_move_with_settings(426, 240, (rect.x + 146, rect.y), &mut settings)
                .1
        );
        assert_eq!(settings.menu_blur, 10);
    }
}
