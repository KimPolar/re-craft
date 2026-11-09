use crate::{menu_assets::MenuAssets, menu_draw::Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontControl {
    Unicode,
    Japanese,
}

pub fn rect(width: i32, control: FontControl) -> Rect {
    Rect {
        x: width / 2
            + if control == FontControl::Unicode {
                -155
            } else {
                5
            },
        y: 35,
        w: 150,
        h: 20,
    }
}

pub fn label(assets: Option<&MenuAssets>, control: FontControl, enabled: bool) -> String {
    let (key, fallback) = match control {
        FontControl::Unicode => ("options.forceUnicodeFont", "Force Unicode Font"),
        FontControl::Japanese => ("options.japaneseGlyphVariants", "Japanese Glyph Variants"),
    };
    let translate =
        |key, fallback| assets.map_or(fallback, |assets| assets.translate(key, fallback));
    let setting = translate(key, fallback);
    let value = if enabled {
        translate("options.on", "ON")
    } else {
        translate("options.off", "OFF")
    };
    crate::text_component::format_plain(
        translate("options.generic_value", "%s: %s"),
        &[setting, value],
    )
}
