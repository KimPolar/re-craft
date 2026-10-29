//! Shared FOV slider for the title and in-game options screens.

use crate::{
    menu_assets::MenuAssets,
    menu_draw::{Canvas, Rect},
    menu_settings::VideoSettings,
};

pub(crate) fn from_slider(value: f64) -> u16 {
    let value = value.clamp(0.0, 1.0);
    let value = if value >= 1.0 {
        f64::from(0.99999_f32)
    } else {
        value
    };
    (30.0 + value * 81.0).floor() as u16
}

pub(crate) fn to_slider(fov: u16) -> f64 {
    match fov.clamp(30, 110) {
        30 => 0.0,
        110 => 1.0,
        value => (f64::from(value) + 0.5 - 30.0) / 81.0,
    }
}

pub struct FovSlider {
    value: f64,
    dragging: bool,
    pub keyboard_adjust: bool,
}

impl FovSlider {
    pub fn new(fov: u16) -> Self {
        Self {
            value: to_slider(fov),
            dragging: false,
            keyboard_adjust: false,
        }
    }

    pub fn rect(width: i32) -> Rect {
        Rect {
            x: width / 2 - 155,
            y: 35,
            w: 150,
            h: 20,
        }
    }

    pub fn start(&mut self, width: i32, x: i32, video: &mut VideoSettings) -> bool {
        self.dragging = true;
        self.keyboard_adjust = true;
        self.move_to(width, x, video)
    }

    fn move_to(&mut self, width: i32, x: i32, video: &mut VideoSettings) -> bool {
        let rect = Self::rect(width);
        self.value = (f64::from(x - rect.x - 4) / f64::from(rect.w - 8)).clamp(0.0, 1.0);
        let next = from_slider(self.value);
        let changed = video.fov != next;
        video.fov = next;
        changed
    }

    pub fn pointer_move(&mut self, width: i32, x: i32, video: &mut VideoSettings) -> (bool, bool) {
        if self.dragging {
            (true, self.move_to(width, x, video))
        } else {
            (false, false)
        }
    }

    pub fn release(&mut self, video: VideoSettings) {
        if self.dragging {
            self.value = to_slider(video.fov);
        }
        self.dragging = false;
    }

    pub fn arrow(&mut self, right: bool, video: &mut VideoSettings) -> bool {
        let current = from_slider(self.value);
        let next = if right {
            current.saturating_add(1).min(110)
        } else {
            current.saturating_sub(1).max(30)
        };
        self.value = to_slider(next);
        let changed = video.fov != next;
        video.fov = next;
        changed
    }

    pub fn draw(
        &self,
        canvas: &mut Canvas,
        assets: Option<&MenuAssets>,
        fov: u16,
        cursor: (i32, i32),
        focused: bool,
    ) {
        let rect = Self::rect(canvas.width);
        let hovered = rect.contains(cursor.0, cursor.1);
        let Some(assets) = assets else {
            canvas.button(None, rect, true, hovered || focused, &format!("FOV: {fov}"));
            return;
        };
        let background = if focused && !self.keyboard_adjust {
            &assets.slider_highlighted
        } else {
            &assets.slider
        };
        let handle = if hovered || (focused && self.keyboard_adjust) {
            &assets.slider_handle_highlighted
        } else {
            &assets.slider_handle
        };
        canvas.gui_sprite(&background.image, rect, background.scaling);
        canvas.gui_sprite(
            &handle.image,
            Rect {
                x: rect.x + (self.value * f64::from(rect.w - 8)) as i32,
                y: rect.y,
                w: 8,
                h: 20,
            },
            handle.scaling,
        );
        let label = assets.translate("options.fov", "FOV");
        let value = match fov {
            70 => assets.translate("options.fov.min", "Normal").to_owned(),
            110 => assets.translate("options.fov.max", "Quake Pro").to_owned(),
            value => value.to_string(),
        };
        let format = assets.translate("options.generic_value", "%s: %s");
        let caption = if format.contains("%1$s") || format.contains("%2$s") {
            format.replace("%1$s", label).replace("%2$s", &value)
        } else {
            format.replacen("%s", label, 1).replacen("%s", &value, 1)
        };
        canvas.text_centered(
            Some(assets),
            rect.x + rect.w / 2,
            rect.y + 6,
            &caption,
            [255, 255, 255],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slider_drag_and_keyboard_change_the_same_setting() {
        let mut video = VideoSettings::default();
        let mut slider = FovSlider::new(video.fov);
        assert_eq!(from_slider(slider.value), 70);
        assert!(slider.arrow(true, &mut video));
        assert_eq!(video.fov, 71);
        assert!(slider.start(426, FovSlider::rect(426).x + 4, &mut video));
        assert_eq!(video.fov, 30);
        assert_eq!(slider.pointer_move(426, 500, &mut video), (true, true));
        assert_eq!(video.fov, 110);
        slider.release(video);
        assert_eq!(slider.pointer_move(426, 0, &mut video), (false, false));
        assert!(!slider.arrow(true, &mut video));
    }
}
