//! The unfocused chat overlay for local system messages.

use native_session::{SystemMessage, SystemMessageKind};

use crate::{
    menu_assets::MenuAssets,
    menu_draw::{Canvas, Rect},
};

#[derive(Clone, Copy, Debug)]
pub struct ChatSettings {
    visible: bool,
    opacity: f64,
    background_opacity: f64,
    scale: f64,
    width: f64,
    line_spacing: f64,
    unfocused_height: f64,
}

impl Default for ChatSettings {
    fn default() -> Self {
        Self {
            visible: true,
            opacity: 1.0,
            background_opacity: 0.5,
            scale: 1.0,
            width: 1.0,
            line_spacing: 0.0,
            unfocused_height: 0.3125,
        }
    }
}

impl ChatSettings {
    pub fn from_options(options: &str) -> Self {
        let mut result = Self::default();
        for line in options.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            if key == "chatVisibility" {
                result.visible = match value.parse::<u8>() {
                    Ok(0 | 1) => true,
                    Ok(2) => false,
                    _ => result.visible,
                };
                continue;
            }
            let Ok(value) = value.parse::<f64>() else {
                continue;
            };
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                continue;
            }
            match key {
                "chatOpacity" => result.opacity = value,
                "textBackgroundOpacity" => result.background_opacity = value,
                "chatScale" => result.scale = value,
                "chatWidth" => result.width = value,
                "chatLineSpacing" => result.line_spacing = value,
                "chatHeightUnfocused" => result.unfocused_height = value,
                _ => {}
            }
        }
        result
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayKey {
    count: usize,
    fading_tick: Option<u64>,
}

pub fn display_key(
    settings: &ChatSettings,
    messages: &[SystemMessage],
    tick: u64,
) -> Option<DisplayKey> {
    if !settings.visible || settings.scale <= 0.0 {
        return None;
    }
    let mut count = 0;
    let mut fading = false;
    for message in messages {
        let age = tick.saturating_sub(message.tick);
        if age < 200 {
            count += 1;
            fading |= age > 180;
        }
    }
    (count != 0).then_some(DisplayKey {
        count,
        fading_tick: fading.then_some(tick),
    })
}

#[derive(Clone, Copy, Debug)]
struct LineLayout {
    width: i32,
    bottom: i32,
    line_height: i32,
    text_offset: i32,
    max_lines: usize,
    scale: f32,
    text_opacity: f32,
    background_alpha: u8,
}

impl LineLayout {
    fn new(settings: &ChatSettings, gui_height: i32, age: u64) -> Option<Self> {
        if !settings.visible || settings.scale <= 0.0 || age >= 200 {
            return None;
        }
        let factor = ((1.0 - age as f64 / 200.0) * 10.0).clamp(0.0, 1.0).powi(2) as f32;
        if factor <= 0.00001 {
            return None;
        }
        let scale = settings.scale as f32;
        // Extremely small scales have no visible pixels, and division must remain bounded.
        if scale < 0.001 {
            return None;
        }
        let width = ((settings.width * 280.0 + 40.0).floor() as f32 / scale).ceil() as i32;
        let bottom = ((gui_height - 40) as f32 / scale).floor() as i32;
        let line_height = (9.0 * (1.0 + settings.line_spacing)) as i32;
        let text_offset =
            (8.0 * (1.0 + settings.line_spacing) - 4.0 * settings.line_spacing).round() as i32;
        let max_lines =
            ((settings.unfocused_height * 160.0 + 20.0).floor() as i32 / line_height) as usize;
        Some(Self {
            width,
            bottom,
            line_height,
            text_offset,
            max_lines,
            scale,
            text_opacity: factor * (settings.opacity as f32 * 0.9 + 0.1),
            background_alpha: (factor * settings.background_opacity as f32 * 255.0) as u8,
        })
    }

    fn rect(&self, x1: i32, y1: i32, x2: i32, y2: i32) -> Rect {
        let x1 = ((x1 + 4) as f32 * self.scale).floor() as i32;
        let y1 = (y1 as f32 * self.scale).floor() as i32;
        let x2 = ((x2 + 4) as f32 * self.scale).ceil() as i32;
        let y2 = (y2 as f32 * self.scale).ceil() as i32;
        Rect {
            x: x1,
            y: y1,
            w: x2 - x1,
            h: y2 - y1,
        }
    }
}

// Split at words where possible, with character boundaries as a fallback. The
// canvas uses the same local font data for measuring and drawing the text.
fn wrap(canvas: &Canvas, assets: &MenuAssets, text: &str, max_width: i32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut width = 0;
    let mut last_space = None;
    for ch in text.chars().take(16_384) {
        if ch == '\n' {
            lines.push(std::mem::take(&mut line));
            width = 0;
            last_space = None;
            continue;
        }
        let char_width = canvas.text_width(Some(assets), ch.encode_utf8(&mut [0; 4]));
        if width + char_width > max_width && !line.is_empty() {
            if let Some(space) = last_space {
                let remainder = line.split_off(space);
                line.pop();
                lines.push(std::mem::replace(&mut line, remainder));
                width = canvas.text_width(Some(assets), &line);
                last_space = None;
            } else {
                lines.push(std::mem::take(&mut line));
                width = 0;
            }
        }
        if ch == ' ' {
            last_space = Some(line.len() + 1);
        }
        line.push(ch);
        width += char_width;
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

pub fn draw(canvas: &mut Canvas, assets: &MenuAssets, messages: &[SystemMessage], tick: u64) {
    let Some(base) = LineLayout::new(&assets.chat, canvas.height, 0) else {
        return;
    };
    let mut lines = Vec::new();
    for message in messages.iter().rev() {
        let Some(layout) = LineLayout::new(
            &assets.chat,
            canvas.height,
            tick.saturating_sub(message.tick),
        ) else {
            continue;
        };
        let key = match message.kind {
            SystemMessageKind::MissingRespawnBlock => "block.minecraft.spawn.not_valid",
        };
        let message = assets.translate(key, key);
        for text in wrap(canvas, assets, message, layout.width.max(1))
            .into_iter()
            .rev()
        {
            if lines.len() >= base.max_lines {
                break;
            }
            lines.push((text, layout));
        }
        if lines.len() >= base.max_lines {
            break;
        }
    }
    for (index, (_, layout)) in lines.iter().enumerate() {
        let bottom = layout.bottom - index as i32 * layout.line_height;
        canvas.fill(
            layout.rect(-4, bottom - layout.line_height, layout.width + 8, bottom),
            [0, 0, 0, layout.background_alpha],
        );
    }
    for (index, (text, layout)) in lines.iter().enumerate() {
        let bottom = layout.bottom - index as i32 * layout.line_height;
        let opacity = (layout.text_opacity * 255.0) as u8;
        canvas.fill(
            layout.rect(-4, bottom - layout.line_height, -2, bottom),
            [208, 208, 208, opacity],
        );
        let x = (4.0 * layout.scale).floor() as i32;
        let y = ((bottom - layout.text_offset) as f32 * layout.scale).floor() as i32;
        if layout.scale == 1.0 {
            canvas.text_alpha(Some(assets), x, y, text, [255, 255, 255], opacity);
        } else {
            let width = canvas.text_width(Some(assets), text).clamp(1, 32768) as u32 + 2;
            let mut source = Canvas::new(width, 10);
            source.text_alpha(Some(assets), 0, 0, text, [255, 255, 255], opacity);
            let w = (source.width as f32 * layout.scale).ceil() as i32;
            let h = (source.height as f32 * layout.scale).ceil() as i32;
            for dy in 0..h {
                for dx in 0..w {
                    let sx = (dx as f32 / layout.scale) as i32;
                    let sy = (dy as f32 / layout.scale) as i32;
                    if sx < source.width && sy < source.height {
                        let p = source.pixels[(sy * source.width + sx) as usize];
                        canvas.pixel(
                            x + dx,
                            y + dy,
                            [(p >> 16) as u8, (p >> 8) as u8, p as u8, (p >> 24) as u8],
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_follow_minecraft_options_and_ignore_invalid_values() {
        let settings = ChatSettings::from_options(
            "chatVisibility:1\nchatOpacity:0.2\nchatScale:0.5\nchatWidth:2\nchatLineSpacing:NaN\nchatHeightUnfocused:0.75\ntextBackgroundOpacity:0.25",
        );
        assert!(settings.visible);
        assert_eq!(settings.opacity, 0.2);
        assert_eq!(settings.scale, 0.5);
        assert_eq!(settings.width, 1.0);
        assert_eq!(settings.line_spacing, 0.0);
        assert_eq!(settings.unfocused_height, 0.75);
        assert_eq!(settings.background_opacity, 0.25);
        assert!(!ChatSettings::from_options("chatVisibility:2").visible);
    }

    #[test]
    fn message_lifetime_and_cache_follow_gui_ticks() {
        let settings = ChatSettings::default();
        let message = SystemMessage {
            kind: SystemMessageKind::MissingRespawnBlock,
            tick: 10,
        };
        let key = display_key(&settings, &[message], 10).unwrap();
        assert_eq!(display_key(&settings, &[message], 190), Some(key));
        assert_ne!(display_key(&settings, &[message], 191), Some(key));
        assert_eq!(display_key(&settings, &[message], 210), None);
        assert_eq!(
            LineLayout::new(&settings, 240, 180).unwrap().text_opacity,
            1.0
        );
        assert!(
            (LineLayout::new(&settings, 240, 190).unwrap().text_opacity - 0.25).abs() < 0.00001
        );
        assert!(LineLayout::new(&settings, 240, 200).is_none());
        assert_eq!(LineLayout::new(&settings, 240, 0).unwrap().max_lines, 7);
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 and Java 25 JDK"]
    fn layout_and_fade_match_installed_java_26_3() {
        use std::{ffi::OsString, path::PathBuf, process::Command};

        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft")
            });
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
        let classpath: OsString = std::env::join_paths(paths).unwrap();
        let bin = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
            .join("bin");
        let source = temp.path().join("ChatHudOracle.java");
        let output = temp.path().join("output.json");
        std::fs::write(&source, include_str!("../tests/java/ChatHudOracle.java")).unwrap();
        for command in [
            Command::new(bin.join("javac"))
                .arg("-cp")
                .arg(&classpath)
                .arg(&source)
                .output()
                .unwrap(),
            Command::new(bin.join("java"))
                .arg("-cp")
                .arg(&classpath)
                .arg("ChatHudOracle")
                .arg(&output)
                .output()
                .unwrap(),
        ] {
            assert!(
                command.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&command.stdout),
                String::from_utf8_lossy(&command.stderr)
            );
        }
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), 3 * 7 * 2 * 2 * 3 * 3 * 3);
        for row in rows {
            let settings = ChatSettings {
                scale: row["scale"].as_f64().unwrap(),
                width: row["width"].as_f64().unwrap(),
                opacity: row["opacity"].as_f64().unwrap(),
                background_opacity: row["backgroundOpacity"].as_f64().unwrap(),
                line_spacing: row["spacing"].as_f64().unwrap(),
                ..ChatSettings::default()
            };
            let height = row["height"].as_i64().unwrap() as i32;
            let age = row["age"].as_u64().unwrap();
            let layout = LineLayout::new(&settings, height, age);
            assert_eq!(layout.is_some(), row["visible"].as_bool().unwrap(), "{row}");
            let Some(layout) = layout else { continue };
            let array = |name: &str| -> Vec<i64> {
                row[name]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_i64().unwrap())
                    .collect()
            };
            assert_eq!(
                array("background"),
                vec![
                    -4,
                    i64::from(layout.bottom - layout.line_height),
                    i64::from(layout.width + 8),
                    i64::from(layout.bottom),
                    i64::from(((u32::from(layout.background_alpha)) << 24) as i32)
                ],
                "{row}"
            );
            assert_eq!(
                array("tag"),
                vec![
                    -4,
                    i64::from(layout.bottom - layout.line_height),
                    -2,
                    i64::from(layout.bottom),
                    i64::from(layout.text_opacity.to_bits()),
                    0xd0d0d0
                ],
                "{row}"
            );
            assert_eq!(
                row["messageY"].as_i64(),
                Some(i64::from(layout.bottom - layout.text_offset)),
                "{row}"
            );
            assert_eq!(
                row["opacityBits"].as_u64(),
                Some(u64::from(layout.text_opacity.to_bits())),
                "{row}"
            );
        }
    }
}
