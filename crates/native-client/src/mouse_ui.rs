//! Shared mouse settings controls for title and in-game menus.
use crate::{
    menu_assets::MenuAssets,
    menu_draw::{Canvas, Rect},
    menu_settings::{UnitInterval, VideoSettings},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseControl {
    Sensitivity,
    InvertX,
    InvertY,
}

pub fn controls_mouse_button(width: i32) -> Rect {
    Rect {
        x: width / 2 - 155,
        y: 35,
        w: 150,
        h: 20,
    }
}

pub fn draw_controls_placeholders(canvas: &mut Canvas, assets: &MenuAssets) {
    let width = canvas.width;
    for (column, row, key, fallback) in [
        (1, 0, "controls.keybinds", "Key Binds..."),
        (0, 1, "key.sneak", "Sneak"),
        (1, 1, "key.sprint", "Sprint"),
        (0, 2, "key.attack", "Attack"),
        (1, 2, "key.use", "Use Item"),
        (0, 3, "options.autoJump", "Auto-Jump"),
        (1, 3, "options.sprintWindow", "Sprint Window"),
        (0, 4, "options.quitShortcuts", "Quit Shortcuts"),
        (1, 4, "options.operatorItemsTab", "Operator Items Tab"),
    ] {
        canvas.button(
            Some(assets),
            Rect {
                x: width / 2 - 155 + column * 160,
                y: 35 + row * 24,
                w: 150,
                h: 20,
            },
            false,
            false,
            assets.translate(key, fallback),
        );
    }
}

pub fn mouse_control_rect(width: i32, control: MouseControl) -> Rect {
    let (column, row) = match control {
        MouseControl::Sensitivity => (0, 0),
        MouseControl::InvertX => (1, 1),
        MouseControl::InvertY => (0, 2),
    };
    Rect {
        x: width / 2 - 155 + column * 160,
        y: 35 + row * 24,
        w: 150,
        h: 20,
    }
}

#[derive(Default)]
pub struct MouseUi {
    dragging: bool,
    pub keyboard_adjust: bool,
}

impl MouseUi {
    pub fn hit(width: i32, cursor: (i32, i32)) -> Option<MouseControl> {
        [
            MouseControl::Sensitivity,
            MouseControl::InvertX,
            MouseControl::InvertY,
        ]
        .into_iter()
        .find(|control| mouse_control_rect(width, *control).contains(cursor.0, cursor.1))
    }
    pub fn start(&mut self, width: i32, x: i32, settings: &mut VideoSettings) -> bool {
        self.dragging = true;
        self.keyboard_adjust = true;
        self.move_to(width, x, settings)
    }
    fn move_to(&self, width: i32, x: i32, settings: &mut VideoSettings) -> bool {
        let rect = mouse_control_rect(width, MouseControl::Sensitivity);
        let value = UnitInterval::new(f64::from(x - rect.x - 4) / f64::from(rect.w - 8));
        let changed = value != settings.mouse_sensitivity;
        settings.mouse_sensitivity = value;
        changed
    }
    pub fn pointer_move(&self, width: i32, x: i32, settings: &mut VideoSettings) -> (bool, bool) {
        if self.dragging {
            (true, self.move_to(width, x, settings))
        } else {
            (false, false)
        }
    }
    pub fn release(&mut self) {
        self.dragging = false;
    }
    pub fn arrow(&self, right: bool, settings: &mut VideoSettings) -> bool {
        let step = f64::from(if right {
            1.0_f32 / 142.0
        } else {
            -1.0_f32 / 142.0
        });
        let next = UnitInterval::new(settings.mouse_sensitivity.value() + step);
        let changed = next != settings.mouse_sensitivity;
        settings.mouse_sensitivity = next;
        changed
    }
    pub fn draw(
        &self,
        canvas: &mut Canvas,
        assets: &MenuAssets,
        settings: VideoSettings,
        cursor: (i32, i32),
        focus: Option<MouseControl>,
    ) {
        let width = canvas.width;
        let rect = mouse_control_rect(width, MouseControl::Sensitivity);
        let hovered = rect.contains(cursor.0, cursor.1);
        let focused = focus == Some(MouseControl::Sensitivity);
        let bg = if focused && !self.keyboard_adjust {
            &assets.slider_highlighted
        } else {
            &assets.slider
        };
        let handle = if hovered || focused && self.keyboard_adjust {
            &assets.slider_handle_highlighted
        } else {
            &assets.slider_handle
        };
        canvas.gui_sprite(&bg.image, rect, bg.scaling);
        canvas.gui_sprite(
            &handle.image,
            Rect {
                x: rect.x + (settings.mouse_sensitivity.value() * f64::from(rect.w - 8)) as i32,
                y: rect.y,
                w: 8,
                h: 20,
            },
            handle.scaling,
        );
        let label = assets.translate("options.sensitivity", "Sensitivity");
        let value = settings.mouse_sensitivity.value();
        let sensitivity_caption = if value == 0.0 {
            caption(
                assets,
                label,
                assets.translate("options.sensitivity.min", "*yawn*"),
            )
        } else if value == 1.0 {
            caption(
                assets,
                label,
                assets.translate("options.sensitivity.max", "HYPERSPEED!!!"),
            )
        } else {
            let number = ((value * 200.0) as i32).to_string();
            format_pair(
                assets.translate("options.percent_value", "%s: %s%%"),
                label,
                &number,
            )
            .replace("%%", "%")
        };
        canvas.text_centered(
            Some(assets),
            rect.x + rect.w / 2,
            rect.y + 6,
            &sensitivity_caption,
            [255, 255, 255],
        );
        for (control, key, fallback, on) in [
            (
                MouseControl::InvertX,
                "options.invertMouseX",
                "Invert X Axis",
                settings.invert_mouse_x,
            ),
            (
                MouseControl::InvertY,
                "options.invertMouseY",
                "Invert Y Axis",
                settings.invert_mouse_y,
            ),
        ] {
            let rect = mouse_control_rect(width, control);
            let label = assets.translate(key, fallback);
            let value = if on {
                assets.translate("options.on", "ON")
            } else {
                assets.translate("options.off", "OFF")
            };
            canvas.button(
                Some(assets),
                rect,
                true,
                rect.contains(cursor.0, cursor.1) || focus == Some(control),
                &caption(assets, label, value),
            );
        }
        for (column, row, key, fallback) in [
            (
                1,
                0,
                "options.mouseWheelSensitivity",
                "Mouse Wheel Sensitivity",
            ),
            (0, 1, "options.discrete_mouse_scroll", "Discrete Scrolling"),
            (1, 2, "options.allowCursorChanges", "Allow Cursor Changes"),
        ] {
            canvas.button(
                Some(assets),
                Rect {
                    x: width / 2 - 155 + column * 160,
                    y: 35 + row * 24,
                    w: 150,
                    h: 20,
                },
                false,
                false,
                assets.translate(key, fallback),
            );
        }
    }
}

fn caption(assets: &MenuAssets, label: &str, value: &str) -> String {
    format_pair(
        assets.translate("options.generic_value", "%s: %s"),
        label,
        value,
    )
}

fn format_pair(format: &str, label: &str, value: &str) -> String {
    if format.contains("%1$s") || format.contains("%2$s") {
        format.replace("%1$s", label).replace("%2$s", value)
    } else {
        format.replacen("%s", label, 1).replacen("%s", value, 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slider_and_hit_testing_follow_the_mouse_settings_layout() {
        let mut settings = VideoSettings::default();
        let mut ui = MouseUi::default();
        let width = 426;
        let slider = mouse_control_rect(width, MouseControl::Sensitivity);
        assert_eq!(
            MouseUi::hit(width, (slider.x + 1, slider.y + 1)),
            Some(MouseControl::Sensitivity)
        );
        assert!(ui.start(width, slider.x + 4, &mut settings));
        assert_eq!(settings.mouse_sensitivity.value(), 0.0);
        assert_eq!(
            ui.pointer_move(width, slider.x + 4 + 142, &mut settings),
            (true, true)
        );
        assert_eq!(settings.mouse_sensitivity.value(), 1.0);
        assert!(ui.arrow(false, &mut settings));
        assert_eq!(
            settings.mouse_sensitivity.value(),
            1.0 + f64::from(-1.0_f32 / 142.0)
        );
        ui.release();
        assert_eq!(
            ui.pointer_move(width, slider.x + 4, &mut settings),
            (false, false)
        );
        for control in [MouseControl::InvertX, MouseControl::InvertY] {
            let rect = mouse_control_rect(width, control);
            assert_eq!(MouseUi::hit(width, (rect.x + 3, rect.y + 3)), Some(control));
        }
    }
}
