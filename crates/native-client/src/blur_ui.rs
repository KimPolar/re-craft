//! Shared Java 26.3 menu blur option and slider presentation.
use crate::{
    menu_assets::MenuAssets,
    menu_draw::{Canvas, Rect},
};

pub fn from_slider(value: f64) -> u8 {
    let value = value.clamp(0.0, 1.0);
    let value = if value >= 1.0 {
        f64::from(0.99999_f32)
    } else {
        value
    };
    (value * 11.0).floor() as u8
}

pub fn to_slider(value: u8) -> f64 {
    match value.min(10) {
        0 => 0.0,
        10 => 1.0,
        value => (f64::from(value) + 0.5) / 11.0,
    }
}

pub fn draw(
    canvas: &mut Canvas,
    assets: &MenuAssets,
    rect: Rect,
    value: u8,
    slider: f64,
    highlighted: bool,
) {
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
    canvas.gui_sprite(&background.image, rect, background.scaling);
    canvas.gui_sprite(
        &handle.image,
        Rect {
            x: rect.x + (slider * f64::from(rect.w - 8)) as i32,
            y: rect.y,
            w: 8,
            h: 20,
        },
        handle.scaling,
    );
    let name = assets.translate(
        "options.accessibility.menu_background_blurriness",
        "Menu Background Blurriness",
    );
    let value = if value == 0 {
        assets.translate("options.off", "OFF").to_owned()
    } else {
        value.to_string()
    };
    let format = assets.translate("options.generic_value", "%s: %s");
    let caption = if format.contains("%1$s") || format.contains("%2$s") {
        format.replace("%1$s", name).replace("%2$s", &value)
    } else {
        format.replacen("%s", name, 1).replacen("%s", &value, 1)
    };
    canvas.text_centered(
        Some(assets),
        rect.x + rect.w / 2,
        rect.y + 6,
        &caption,
        [255, 255, 255],
    );
}
