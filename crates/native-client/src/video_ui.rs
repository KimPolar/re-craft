//! The 26.3 video options list. Unimplemented options stay visible but disabled.
use crate::{
    menu_assets::MenuAssets,
    menu_draw::{Canvas, Rect},
    menu_settings::{InactivityFpsLimit, VideoSettings},
};
use std::time::{Duration, Instant};

const TOP: i32 = 33;
const FOOTER: i32 = 33;
const CONTENT_HEIGHT: i32 = 519;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoControl {
    Framerate,
    Vsync,
    Fullscreen,
    Inactivity,
    MenuBlur,
    RenderDistance,
    BiomeBlend,
    WeatherRadius,
}

#[derive(Clone, Copy)]
struct OptionLabel {
    key: &'static str,
    fallback: &'static str,
}

const fn label(key: &'static str, fallback: &'static str) -> OptionLabel {
    OptionLabel { key, fallback }
}

enum Row {
    Header(OptionLabel),
    Big(OptionLabel),
    Pair(OptionLabel, Option<OptionLabel>),
}

const ROWS: &[Row] = &[
    Row::Header(label("options.video.display.header", "Display")),
    Row::Big(label(
        "options.fullscreen.exclusive.mode",
        "Exclusive Fullscreen Mode",
    )),
    Row::Pair(
        label("options.framerateLimit", "Max Framerate"),
        Some(label("options.vsync", "VSync")),
    ),
    Row::Pair(
        label("options.inactivityFpsLimit", "Reduce fps when"),
        Some(label("options.guiScale", "GUI Scale")),
    ),
    Row::Pair(
        label("options.fullscreen", "Fullscreen"),
        Some(label("options.exclusiveFullscreen", "Exclusive Fullscreen")),
    ),
    Row::Pair(
        label("options.gamma", "Brightness"),
        Some(label("options.graphicsApi", "Graphics API")),
    ),
    Row::Header(label(
        "options.video.quality.header",
        "Quality & Performance",
    )),
    Row::Big(label("options.graphics.preset", "Preset")),
    Row::Pair(
        label("options.biomeBlendRadius", "Biome Blend"),
        Some(label("options.renderDistance", "Render Distance")),
    ),
    Row::Pair(
        label("options.prioritizeChunkUpdates", "Prioritize Chunk Updates"),
        Some(label("options.simulationDistance", "Simulation Distance")),
    ),
    Row::Pair(
        label("options.ao", "Smooth Lighting"),
        Some(label("options.renderClouds", "Clouds")),
    ),
    Row::Pair(
        label("options.particles", "Particles"),
        Some(label("options.mipmapLevels", "Mipmap Levels")),
    ),
    Row::Pair(
        label("options.entityShadows", "Entity Shadows"),
        Some(label("options.entityDistanceScaling", "Entity Distance")),
    ),
    Row::Pair(
        label(
            "options.accessibility.menu_background_blurriness",
            "Menu Background Blurriness",
        ),
        Some(label("options.renderCloudsDistance", "Cloud Distance")),
    ),
    Row::Pair(
        label("options.cutoutLeaves", "See-Through Leaves"),
        Some(label(
            "options.improvedTransparency",
            "Improved Transparency",
        )),
    ),
    Row::Pair(
        label("options.textureFiltering", "Texture Filtering"),
        Some(label("options.maxAnisotropy", "Anisotropic Filtering")),
    ),
    Row::Pair(
        label("options.weatherRadius", "Weather Effect Radius"),
        None,
    ),
    Row::Header(label("options.video.preferences.header", "Preferences")),
    Row::Pair(
        label("options.autosaveIndicator", "Autosave Indicator"),
        Some(label("options.vignette", "Show Vignette")),
    ),
    Row::Pair(
        label("options.attackIndicator", "Attack Indicator"),
        Some(label("options.chunkFade", "Chunk Section Fade In")),
    ),
];

/// Java's IntRange(1, 26) maps the slider to values 10..260.
pub fn fps_from_slider(value: f64) -> u16 {
    let value = value.clamp(0.0, 1.0);
    let value = if value >= 1.0 {
        f64::from(0.99999_f32)
    } else {
        value
    };
    ((1.0 + value * 26.0).floor() as u16) * 10
}

pub fn slider_from_fps(fps: u16) -> f64 {
    let value = (fps / 10).clamp(1, 26);
    match value {
        1 => 0.0,
        26 => 1.0,
        _ => (f64::from(value) + 0.5 - 1.0) / 26.0,
    }
}

pub fn distance_from_slider(value: f64) -> u8 {
    let value = value.clamp(0.0, 1.0);
    let value = if value >= 1.0 {
        f64::from(0.99999_f32)
    } else {
        value
    };
    2 + (value * 31.0).floor() as u8
}

pub fn slider_from_distance(value: u8) -> f64 {
    match value.clamp(2, 32) {
        2 => 0.0,
        32 => 1.0,
        value => (f64::from(value - 2) + 0.5) / 31.0,
    }
}

fn blend_from_slider(value: f64) -> u8 {
    let value = value.clamp(0.0, f64::from(0.99999_f32));
    (value * 8.0).floor() as u8
}

fn slider_from_blend(value: u8) -> f64 {
    match value.min(7) {
        0 => 0.0,
        7 => 1.0,
        value => (f64::from(value) + 0.5) / 8.0,
    }
}

fn weather_from_slider(value: f64) -> u8 {
    3 + (value.clamp(0.0, f64::from(0.99999_f32)) * 8.0).floor() as u8
}

fn slider_from_weather(value: u8) -> f64 {
    match value.clamp(3, 10) {
        3 => 0.0,
        10 => 1.0,
        value => (f64::from(value - 3) + 0.5) / 8.0,
    }
}

pub struct VideoUi {
    scroll: f64,
    slider: f64,
    dragging_slider: Option<VideoControl>,
    blur_slider: f64,
    distance_slider: f64,
    distance_pending: Option<Instant>,
    blend_slider: f64,
    blend_pending: Option<Instant>,
    weather_slider: f64,
    dragging_scrollbar: Option<i32>,
    pub keyboard_adjust: bool,
}

impl VideoUi {
    pub fn new(video: VideoSettings) -> Self {
        Self {
            scroll: 0.0,
            slider: slider_from_fps(video.max_fps),
            dragging_slider: None,
            blur_slider: crate::blur_ui::to_slider(video.menu_blur),
            distance_slider: slider_from_distance(video.render_distance),
            distance_pending: None,
            blend_slider: slider_from_blend(video.biome_blend),
            blend_pending: None,
            weather_slider: slider_from_weather(video.weather_radius),
            dragging_scrollbar: None,
            keyboard_adjust: false,
        }
    }

    fn viewport_height(height: i32) -> i32 {
        (height - TOP - FOOTER).max(0)
    }
    fn max_scroll(height: i32) -> i32 {
        (CONTENT_HEIGHT - Self::viewport_height(height)).max(0)
    }
    pub fn control_rect(&self, width: i32, control: VideoControl) -> Rect {
        Rect {
            x: width / 2
                + if matches!(control, VideoControl::Vsync | VideoControl::RenderDistance) {
                    5
                } else {
                    -155
                },
            y: match control {
                VideoControl::Inactivity => 105,
                VideoControl::Fullscreen => 130,
                VideoControl::MenuBlur => 366,
                VideoControl::WeatherRadius => 441,
                VideoControl::RenderDistance | VideoControl::BiomeBlend => 241,
                _ => 80,
            } - self.scroll as i32,
            w: 150,
            h: 20,
        }
    }
    pub fn visible(&self, height: i32, rect: Rect) -> bool {
        rect.y < height - FOOTER && rect.y + rect.h > TOP
    }
    pub fn hit(&self, width: i32, height: i32, cursor: (i32, i32)) -> Option<VideoControl> {
        if cursor.1 < TOP || cursor.1 >= height - FOOTER {
            return None;
        }
        [
            VideoControl::Framerate,
            VideoControl::Vsync,
            VideoControl::Fullscreen,
            VideoControl::Inactivity,
            VideoControl::MenuBlur,
            VideoControl::RenderDistance,
            VideoControl::BiomeBlend,
            VideoControl::WeatherRadius,
        ]
        .into_iter()
        .find(|control| {
            self.control_rect(width, *control)
                .contains(cursor.0, cursor.1)
        })
    }
    pub fn scroll_by(&mut self, delta: f64, height: i32) -> bool {
        let old = self.scroll;
        self.scroll = (self.scroll - delta * 12.0).clamp(0.0, f64::from(Self::max_scroll(height)));
        old != self.scroll
    }
    pub fn resize(&mut self, height: i32) {
        self.scroll = self.scroll.clamp(0.0, f64::from(Self::max_scroll(height)));
    }
    pub fn start_slider(&mut self, width: i32, cursor_x: i32, video: &mut VideoSettings) -> bool {
        self.start_control_slider(width, cursor_x, VideoControl::Framerate, video)
    }
    pub fn start_control_slider(
        &mut self,
        width: i32,
        cursor_x: i32,
        control: VideoControl,
        video: &mut VideoSettings,
    ) -> bool {
        self.dragging_slider = Some(control);
        self.keyboard_adjust = true;
        self.slider_to_x(width, cursor_x, control, video)
    }
    fn slider_to_x(
        &mut self,
        width: i32,
        x: i32,
        control: VideoControl,
        video: &mut VideoSettings,
    ) -> bool {
        let rect = self.control_rect(width, control);
        let value = (f64::from(x - rect.x - 4) / f64::from(rect.w - 8)).clamp(0.0, 1.0);
        if control == VideoControl::WeatherRadius {
            self.weather_slider = value;
            let next = weather_from_slider(value);
            let changed = video.weather_radius != next;
            video.weather_radius = next;
            return changed;
        }
        if control == VideoControl::BiomeBlend {
            self.blend_slider = value;
            self.blend_pending = Some(Instant::now() + Duration::from_millis(600));
            return false;
        }
        if control == VideoControl::RenderDistance {
            self.distance_slider = value;
            self.distance_pending = Some(Instant::now() + Duration::from_millis(600));
            return false;
        }
        if control == VideoControl::MenuBlur {
            self.blur_slider = value;
            let next = crate::blur_ui::from_slider(value);
            let changed = video.menu_blur != next;
            video.menu_blur = next;
            return changed;
        }
        self.slider = value;
        let fps = fps_from_slider(self.slider);
        let changed = video.max_fps != fps;
        video.max_fps = fps;
        changed
    }
    pub fn blur_arrow(&mut self, right: bool, video: &mut VideoSettings) -> bool {
        let next = if right {
            video.menu_blur.saturating_add(1).min(10)
        } else {
            video.menu_blur.saturating_sub(1)
        };
        let changed = video.menu_blur != next;
        video.menu_blur = next;
        self.blur_slider = crate::blur_ui::to_slider(next);
        changed
    }

    pub fn distance_arrow(&mut self, right: bool) {
        let current = distance_from_slider(self.distance_slider);
        let next = if right {
            current.saturating_add(1).min(32)
        } else {
            current.saturating_sub(1).max(2)
        };
        self.distance_slider = slider_from_distance(next);
        self.distance_pending = Some(Instant::now() + Duration::from_millis(600));
    }

    pub fn blend_arrow(&mut self, right: bool) {
        let current = blend_from_slider(self.blend_slider);
        let next = if right {
            current.saturating_add(1).min(7)
        } else {
            current.saturating_sub(1)
        };
        self.blend_slider = slider_from_blend(next);
        self.blend_pending = Some(Instant::now() + Duration::from_millis(600));
    }

    pub fn weather_arrow(&mut self, right: bool, video: &mut VideoSettings) -> bool {
        let next = if right {
            video.weather_radius.saturating_add(1).min(10)
        } else {
            video.weather_radius.saturating_sub(1).max(3)
        };
        let changed = next != video.weather_radius;
        video.weather_radius = next;
        self.weather_slider = slider_from_weather(next);
        changed
    }

    pub fn apply_pending_distance(
        &mut self,
        video: &mut VideoSettings,
        now: Instant,
        force: bool,
    ) -> bool {
        let mut changed = false;
        if self
            .blend_pending
            .is_some_and(|deadline| force || now >= deadline)
        {
            self.blend_pending = None;
            let next = blend_from_slider(self.blend_slider);
            changed = video.biome_blend != next;
            video.biome_blend = next;
        }
        if self
            .distance_pending
            .is_some_and(|deadline| force || now >= deadline)
        {
            self.distance_pending = None;
            let next = distance_from_slider(self.distance_slider);
            changed |= video.render_distance != next;
            video.render_distance = next;
        }
        changed
    }

    pub fn ensure_control_visible(&mut self, height: i32, control: VideoControl) {
        let y = self.control_rect(0, control).y;
        if y < TOP {
            self.scroll -= f64::from(TOP - y);
        }
        if y + 20 > height - FOOTER {
            self.scroll += f64::from(y + 20 - (height - FOOTER));
        }
        self.resize(height);
    }
    pub fn arrow(&mut self, right: bool, video: &mut VideoSettings) -> bool {
        let current = fps_from_slider(self.slider);
        let fps = if right {
            current.saturating_add(10).min(260)
        } else {
            current.saturating_sub(10).max(10)
        };
        self.slider = slider_from_fps(fps);
        let changed = video.max_fps != fps;
        video.max_fps = fps;
        changed
    }
    pub fn start_scrollbar(&mut self, width: i32, height: i32, cursor: (i32, i32)) -> bool {
        let x = width / 2 + 163;
        if Self::max_scroll(height) > 0
            && (x..x + 6).contains(&cursor.0)
            && (TOP..height - FOOTER).contains(&cursor.1)
        {
            self.dragging_scrollbar = Some(cursor.1);
            true
        } else {
            false
        }
    }
    fn move_scrollbar(&mut self, previous: i32, y: i32, height: i32) {
        let viewport = Self::viewport_height(height);
        let thumb = ((viewport * viewport) / CONTENT_HEIGHT).clamp(32, (viewport - 8).max(32));
        let range = (viewport - thumb).max(1);
        let maximum = f64::from(Self::max_scroll(height));
        self.scroll = if y < TOP {
            0.0
        } else if y > height - FOOTER {
            maximum
        } else {
            (self.scroll + f64::from(y - previous) * (maximum / f64::from(range)).max(1.0))
                .clamp(0.0, maximum)
        };
    }
    pub fn pointer_move(
        &mut self,
        width: i32,
        height: i32,
        cursor: (i32, i32),
        video: &mut VideoSettings,
    ) -> (bool, bool) {
        if let Some(control) = self.dragging_slider {
            return (true, self.slider_to_x(width, cursor.0, control, video));
        }
        if let Some(previous) = self.dragging_scrollbar {
            self.move_scrollbar(previous, cursor.1, height);
            self.dragging_scrollbar = Some(cursor.1);
            return (true, false);
        }
        (false, false)
    }
    pub fn release(&mut self, video: VideoSettings) {
        if self.dragging_slider == Some(VideoControl::Framerate) {
            self.slider = slider_from_fps(video.max_fps);
        }
        if self.dragging_slider == Some(VideoControl::MenuBlur) {
            self.blur_slider = crate::blur_ui::to_slider(video.menu_blur);
        }
        if self.dragging_slider == Some(VideoControl::WeatherRadius) {
            self.weather_slider = slider_from_weather(video.weather_radius);
        }
        self.dragging_slider = None;
        self.dragging_scrollbar = None;
    }

    pub fn draw(
        &self,
        canvas: &mut Canvas,
        assets: &MenuAssets,
        video: VideoSettings,
        cursor: (i32, i32),
        focused: Option<VideoControl>,
    ) {
        let width = canvas.width;
        let height = canvas.height;
        let viewport = Self::viewport_height(height);
        if viewport == 0 {
            return;
        }
        let mut list = Canvas::new(width as u32, viewport as u32);
        let tr = |label: OptionLabel| assets.translate(label.key, label.fallback);
        let mut y = 35 - self.scroll as i32 - TOP;
        let mut first_header = true;
        for row in ROWS {
            match row {
                Row::Header(label) => {
                    let padding = if first_header { 0 } else { 18 };
                    list.text_centered(
                        Some(assets),
                        width / 2,
                        y + 2 + padding,
                        tr(*label),
                        [255, 255, 255],
                    );
                    y += if first_header { 18 } else { 36 };
                    first_header = false;
                }
                Row::Big(label) => {
                    list.button(
                        Some(assets),
                        Rect {
                            x: width / 2 - 155,
                            y: y + 2,
                            w: 310,
                            h: 20,
                        },
                        false,
                        false,
                        tr(*label),
                    );
                    y += 25;
                }
                Row::Pair(left, right) => {
                    for (index, label) in [Some(*left), *right].into_iter().enumerate() {
                        let Some(label) = label else { continue };
                        let rect = Rect {
                            x: width / 2 - 155 + index as i32 * 160,
                            y: y + 2,
                            w: 150,
                            h: 20,
                        };
                        let on_screen = (cursor.0, cursor.1 - TOP);
                        let hovered = (TOP..height - FOOTER).contains(&cursor.1)
                            && rect.contains(on_screen.0, on_screen.1);
                        if label.key == "options.accessibility.menu_background_blurriness" {
                            let slider = if self.dragging_slider == Some(VideoControl::MenuBlur) {
                                self.blur_slider
                            } else {
                                crate::blur_ui::to_slider(video.menu_blur)
                            };
                            crate::blur_ui::draw(
                                &mut list,
                                assets,
                                rect,
                                video.menu_blur,
                                slider,
                                hovered || focused == Some(VideoControl::MenuBlur),
                            );
                        } else if label.key == "options.renderDistance"
                            || label.key == "options.biomeBlendRadius"
                            || label.key == "options.weatherRadius"
                        {
                            let blend = label.key == "options.biomeBlendRadius";
                            let weather = label.key == "options.weatherRadius";
                            let slider = if weather {
                                self.weather_slider
                            } else if blend {
                                self.blend_slider
                            } else {
                                self.distance_slider
                            };
                            let control = if weather {
                                VideoControl::WeatherRadius
                            } else if blend {
                                VideoControl::BiomeBlend
                            } else {
                                VideoControl::RenderDistance
                            };
                            let highlighted = hovered || focused == Some(control);
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
                                    x: rect.x + (slider * f64::from(rect.w - 8)) as i32,
                                    y: rect.y,
                                    w: 8,
                                    h: 20,
                                },
                                handle.scaling,
                            );
                            let value = if weather {
                                assets.translate("options.blocks", "%s Blocks").replacen(
                                    "%s",
                                    &weather_from_slider(slider).to_string(),
                                    1,
                                )
                            } else if blend {
                                let side = blend_from_slider(slider) * 2 + 1;
                                let key = format!("options.biomeBlendRadius.{side}");
                                let fallback = if side == 1 {
                                    "OFF (Fastest)".to_owned()
                                } else {
                                    format!("{side}x{side}")
                                };
                                assets.translate(&key, &fallback).to_owned()
                            } else {
                                assets.translate("options.chunks", "%s Chunks").replacen(
                                    "%s",
                                    &distance_from_slider(slider).to_string(),
                                    1,
                                )
                            };
                            let format = assets.translate("options.generic_value", "%s: %s");
                            let text = if format.contains("%1$s") || format.contains("%2$s") {
                                format.replace("%1$s", tr(label)).replace("%2$s", &value)
                            } else {
                                format
                                    .replacen("%s", tr(label), 1)
                                    .replacen("%s", &value, 1)
                            };
                            list.text_centered(
                                Some(assets),
                                rect.x + rect.w / 2,
                                rect.y + 6,
                                &text,
                                [255, 255, 255],
                            );
                        } else if label.key == "options.framerateLimit" {
                            let highlighted = hovered || focused == Some(VideoControl::Framerate);
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
                                    x: rect.x + (self.slider * f64::from(rect.w - 8)) as i32,
                                    y: rect.y,
                                    w: 8,
                                    h: 20,
                                },
                                handle.scaling,
                            );
                            let value = if video.max_fps == 260 {
                                assets
                                    .translate("options.framerateLimit.max", "Unlimited")
                                    .to_owned()
                            } else {
                                assets.translate("options.framerate", "%s fps").replacen(
                                    "%s",
                                    &video.max_fps.to_string(),
                                    1,
                                )
                            };
                            list.text_centered(
                                Some(assets),
                                rect.x + rect.w / 2,
                                rect.y + 6,
                                &format!("{}: {value}", tr(label)),
                                [255, 255, 255],
                            );
                        } else if label.key == "options.vsync" || label.key == "options.fullscreen"
                        {
                            let fullscreen = label.key == "options.fullscreen";
                            let enabled = if fullscreen {
                                video.fullscreen
                            } else {
                                video.vsync
                            };
                            let key = if enabled {
                                "options.on.composed"
                            } else {
                                "options.off.composed"
                            };
                            let fallback = if enabled { "%s: ON" } else { "%s: OFF" };
                            let value =
                                assets.translate(key, fallback).replacen("%s", tr(label), 1);
                            list.button(
                                Some(assets),
                                rect,
                                true,
                                hovered
                                    || focused
                                        == Some(if fullscreen {
                                            VideoControl::Fullscreen
                                        } else {
                                            VideoControl::Vsync
                                        }),
                                &value,
                            );
                        } else if label.key == "options.inactivityFpsLimit" {
                            let (key, fallback) = match video.inactivity_fps_limit {
                                InactivityFpsLimit::Afk => {
                                    ("options.inactivityFpsLimit.afk", "AFK")
                                }
                                InactivityFpsLimit::Minimized => {
                                    ("options.inactivityFpsLimit.minimized", "Minimized")
                                }
                            };
                            let value = assets.translate(key, fallback);
                            let format = assets.translate("options.generic_value", "%s: %s");
                            let value = if format.contains("%1$s") || format.contains("%2$s") {
                                format.replace("%1$s", tr(label)).replace("%2$s", value)
                            } else {
                                format.replacen("%s", tr(label), 1).replacen("%s", value, 1)
                            };
                            list.button(
                                Some(assets),
                                rect,
                                true,
                                hovered || focused == Some(VideoControl::Inactivity),
                                &value,
                            );
                        } else {
                            list.button(Some(assets), rect, false, false, tr(label));
                        }
                    }
                    y += 25;
                }
            }
        }
        canvas.composite(&list, 0, TOP);
        if Self::max_scroll(height) > 0 {
            let x = width / 2 + 163;
            let thumb = ((viewport * viewport) / CONTENT_HEIGHT).clamp(32, (viewport - 8).max(32));
            let thumb_y =
                TOP + (self.scroll as i32 * (viewport - thumb) / Self::max_scroll(height));
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
                    y: thumb_y,
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
    fn weather_radius_slider_applies_immediately_and_supports_keyboard() {
        let mut video = VideoSettings::default();
        let mut ui = VideoUi::new(video);
        assert_eq!(
            ui.control_rect(426, VideoControl::WeatherRadius),
            Rect {
                x: 58,
                y: 441,
                w: 150,
                h: 20
            }
        );
        for radius in 3..=10 {
            assert_eq!(weather_from_slider(slider_from_weather(radius)), radius);
        }
        assert!(ui.start_control_slider(426, 62, VideoControl::WeatherRadius, &mut video));
        assert_eq!(video.weather_radius, 3);
        ui.release(video);
        assert!(ui.weather_arrow(true, &mut video));
        assert_eq!(video.weather_radius, 4);
        assert!(ui.weather_arrow(false, &mut video));
        assert!(!ui.weather_arrow(false, &mut video));
        ui.ensure_control_visible(240, VideoControl::WeatherRadius);
        let rect = ui.control_rect(426, VideoControl::WeatherRadius);
        assert!(ui.visible(240, rect));
        assert_eq!(
            ui.hit(426, 240, (rect.x + 5, rect.y + 5)),
            Some(VideoControl::WeatherRadius)
        );
    }
    #[test]
    fn biome_blend_slider_delays_changes_and_supports_keyboard() {
        let mut video = VideoSettings::default();
        let mut ui = VideoUi::new(video);
        assert_eq!(
            ui.control_rect(426, VideoControl::BiomeBlend),
            Rect {
                x: 58,
                y: 241,
                w: 150,
                h: 20
            }
        );
        for radius in 0..=7 {
            assert_eq!(blend_from_slider(slider_from_blend(radius)), radius);
        }
        assert!(!ui.start_control_slider(426, 204, VideoControl::BiomeBlend, &mut video));
        assert_eq!(video.biome_blend, 2);
        let deadline = ui.blend_pending.unwrap();
        assert!(!ui.apply_pending_distance(&mut video, deadline - Duration::from_millis(1), false));
        assert!(ui.apply_pending_distance(&mut video, deadline, false));
        assert_eq!(video.biome_blend, 7);
        ui.release(video);
        ui.blend_arrow(false);
        assert!(ui.apply_pending_distance(&mut video, Instant::now(), true));
        assert_eq!(video.biome_blend, 6);
    }
    #[test]
    fn render_distance_slider_delays_application_and_preserves_keyboard_steps() {
        let mut video = VideoSettings::default();
        let mut ui = VideoUi::new(video);
        assert_eq!(
            ui.control_rect(426, VideoControl::RenderDistance),
            Rect {
                x: 218,
                y: 241,
                w: 150,
                h: 20
            }
        );
        for distance in 2..=32 {
            assert_eq!(
                distance_from_slider(slider_from_distance(distance)),
                distance
            );
        }
        assert!(!ui.start_control_slider(426, 364, VideoControl::RenderDistance, &mut video));
        assert_eq!(video.render_distance, 12);
        let deadline = ui.distance_pending.unwrap();
        assert!(!ui.apply_pending_distance(&mut video, deadline - Duration::from_millis(1), false));
        assert!(ui.apply_pending_distance(&mut video, deadline, false));
        assert_eq!(video.render_distance, 32);
        ui.release(video);
        ui.distance_arrow(false);
        assert!(ui.apply_pending_distance(&mut video, Instant::now(), true));
        assert_eq!(video.render_distance, 31);
    }
    #[test]
    fn slider_endpoints_and_quantized_values_follow_java_range() {
        assert_eq!(fps_from_slider(0.0), 10);
        assert_eq!(fps_from_slider(1.0), 260);
        for fps in (10..=260).step_by(10) {
            assert_eq!(fps_from_slider(slider_from_fps(fps)), fps);
        }
    }
    #[test]
    fn keyboard_steps_and_mouse_release_snap_to_discrete_values() {
        let mut video = VideoSettings::default();
        let mut ui = VideoUi::new(video);
        assert!(ui.arrow(true, &mut video));
        assert_eq!(video.max_fps, 130);
        assert_eq!(ui.slider, slider_from_fps(130));
        assert!(ui.start_slider(426, 150, &mut video));
        let selected = video.max_fps;
        ui.release(video);
        assert_eq!(ui.slider, slider_from_fps(selected));
        video.max_fps = 260;
        ui.slider = 1.0;
        assert!(!ui.arrow(true, &mut video));
    }
    #[test]
    fn first_video_controls_match_vanilla_position_and_clip() {
        let mut ui = VideoUi::new(VideoSettings::default());
        assert_eq!(
            ui.control_rect(426, VideoControl::Framerate),
            Rect {
                x: 58,
                y: 80,
                w: 150,
                h: 20
            }
        );
        assert_eq!(
            ui.control_rect(426, VideoControl::Vsync),
            Rect {
                x: 218,
                y: 80,
                w: 150,
                h: 20
            }
        );
        assert_eq!(ui.hit(426, 240, (220, 85)), Some(VideoControl::Vsync));
        assert_eq!(
            ui.control_rect(426, VideoControl::Inactivity),
            Rect {
                x: 58,
                y: 105,
                w: 150,
                h: 20
            }
        );
        assert_eq!(ui.hit(426, 240, (60, 110)), Some(VideoControl::Inactivity));
        assert_eq!(
            ui.control_rect(426, VideoControl::Fullscreen),
            Rect {
                x: 58,
                y: 130,
                w: 150,
                h: 20
            }
        );
        assert_eq!(ui.hit(426, 240, (60, 135)), Some(VideoControl::Fullscreen));
        assert!(ui.scroll_by(-10.0, 240));
        assert_eq!(ui.hit(426, 240, (220, 33)), None);
        ui.resize(1000);
        assert_eq!(ui.hit(426, 1000, (220, 85)), Some(VideoControl::Vsync));
        assert!(ui.start_scrollbar(426, 240, (426 / 2 + 163, 190)));
        let before = ui.scroll;
        let mut video = VideoSettings::default();
        let (dragged, changed) = ui.pointer_move(426, 240, (426 / 2 + 163, 200), &mut video);
        assert!(dragged && !changed && ui.scroll >= before);
        ui.release(video);
        assert!(!ui.pointer_move(426, 240, (400, 200), &mut video).0);
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn slider_mapping_matches_installed_java_26_3() {
        use std::{ffi::OsString, path::PathBuf, process::Command};
        const JAVA: &str = r#"
import net.minecraft.client.OptionInstance;
import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.screens.options.OptionsSubScreen;
import net.minecraft.client.gui.components.OptionsList;
import net.minecraft.client.gui.components.Button;
import net.minecraft.client.gui.components.AbstractSliderButton;
import net.minecraft.client.input.MouseButtonEvent;
import net.minecraft.client.input.MouseButtonInfo;
import net.minecraft.client.input.KeyEvent;
import net.minecraft.network.chat.Component;
public class VideoSliderOracle {
    static class TestScreen extends OptionsSubScreen {
        TestScreen(int width, int height) { super(null, null, Component.empty()); this.width=width; this.height=height; }
        protected void addOptions() {}
    }
    static class TestList extends OptionsList {
        final TestScreen screen;
        TestList(Minecraft client, TestScreen screen) { super(client, screen.width, screen); this.screen=screen; }
        void row(int height) {
            var button = Button.builder(Component.empty(), ignored -> {}).build();
            addEntry(OptionsList.Entry.big(button, screen), height);
        }
    }
    static class TestSlider extends AbstractSliderButton {
        TestSlider() { super(58, 0, 150, 20, Component.empty(), 0.5); canChangeValue=true; }
        protected void updateMessage() {}
        protected void applyValue() {}
        double getValue() { return value; }
    }
    public static void main(String[] args) {
        var range = new OptionInstance.IntRange(1, 26);
        var mapped = range.xmap(i -> i * 10, (Integer i) -> i / 10, true);
        for (int i=0; i<=10000; i++) System.out.println(range.fromSliderValue(i/10000.0)*10);
        for (int i=1; i<=26; i++) System.out.println(range.toSliderValue(i));
        for (int i=1; i<=26; i++) {
            System.out.println(mapped.previous(i*10).orElseThrow());
            System.out.println(mapped.next(i*10).orElseThrow());
        }
        var fov = new OptionInstance.IntRange(30, 110);
        for (int i=0; i<=10000; i++) System.out.println(fov.fromSliderValue(i/10000.0));
        for (int i=30; i<=110; i++) {
            System.out.println(fov.toSliderValue(i));
            System.out.println(fov.previous(i).orElseThrow());
            System.out.println(fov.next(i).orElseThrow());
        }
        var blur = new OptionInstance.IntRange(0, 10);
        for (int i=0; i<=10000; i++) System.out.println(blur.fromSliderValue(i/10000.0));
        for (int i=0; i<=10; i++) System.out.println(blur.toSliderValue(i));
        var distance = new OptionInstance.IntRange(2, 32, false);
        for (int i=0; i<=10000; i++) System.out.println(distance.fromSliderValue(i/10000.0));
        for (int i=2; i<=32; i++) System.out.println(distance.toSliderValue(i));
        var blend = new OptionInstance.IntRange(0, 7, false);
        for (int i=0; i<=10000; i++) System.out.println(blend.fromSliderValue(i/10000.0));
        for (int i=0; i<=7; i++) System.out.println(blend.toSliderValue(i));
        try {
            var unsafeField=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
            var unsafe=(sun.misc.Unsafe)unsafeField.get(null);
            var client=(Minecraft)unsafe.allocateInstance(Minecraft.class);
            var instance=Minecraft.class.getDeclaredField("instance"); instance.setAccessible(true); instance.set(null,client);
            int[] heights={18,25,25,25,25,25,36,25,25,25,25,25,25,25,25,25,25,36,25,25};
            for(int width : new int[]{320,426,640}) for(int height : new int[]{240,360}) for(int scroll : new int[]{0,12,47}) {
                var screen=new TestScreen(width,height);
                var list=new TestList(client,screen);
                for(int entryHeight:heights) list.row(entryHeight);
                list.setScrollAmount(scroll);
                Object entry=list.children().get(2);
                var fps=(net.minecraft.client.gui.layouts.LayoutElement)entry;
                System.out.println(fps.getX()+","+(fps.getY()+2)+","+list.maxScrollAmount());
                var blurEntry=(net.minecraft.client.gui.layouts.LayoutElement)list.children().get(13);
                System.out.println(blurEntry.getX()+","+(blurEntry.getY()+2)+","+list.maxScrollAmount());
            }
            for(int width : new int[]{320,426,640}) for(int height : new int[]{240,360}) for(int scroll : new int[]{0,12,47}) {
                var screen=new TestScreen(width,height);
                var list=new TestList(client,screen);
                for(int row=0; row<13; row++) list.row(25);
                list.setScrollAmount(scroll);
                var entry=(net.minecraft.client.gui.layouts.LayoutElement)list.children().get(5);
                System.out.println((entry.getX()+160)+","+(entry.getY()+2)+","+list.maxScrollAmount());
                var blurEntry=(net.minecraft.client.gui.layouts.LayoutElement)list.children().get(2);
                System.out.println(blurEntry.getX()+","+(blurEntry.getY()+2)+","+list.maxScrollAmount());
            }
            for(int width : new int[]{320,426,640}) for(int height : new int[]{240,360}) for(int scroll : new int[]{0,12,47}) {
                var screen=new TestScreen(width,height);
                var list=new TestList(client,screen);
                for(int row=0; row<13; row++) list.row(25);
                list.setScrollAmount(scroll);
                var entry=(net.minecraft.client.gui.layouts.LayoutElement)list.children().get(10);
                System.out.println(entry.getX()+","+(entry.getY()+2)+","+list.maxScrollAmount());
            }
            for(int x=50; x<=220; x++) {
                var slider=new TestSlider();
                slider.onClick(new MouseButtonEvent(x,0,new MouseButtonInfo(0,0)),false);
                System.out.println(Double.doubleToRawLongBits(slider.getValue()));
                slider.keyPressed(new KeyEvent(80,0,0));
                System.out.println(Double.doubleToRawLongBits(slider.getValue()));
                slider.keyPressed(new KeyEvent(79,0,0));
                System.out.println(Double.doubleToRawLongBits(slider.getValue()));
            }
        } catch (Exception e) { throw new RuntimeException(e); }
    }
}
"#;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("VideoSliderOracle.java");
        std::fs::write(&source, JAVA).unwrap();
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
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
            .join("bin");
        let run = |command: &mut Command| {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "Java video oracle failed: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            result.stdout
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-encoding")
            .arg("UTF-8")
            .arg("-classpath")
            .arg(&classpath)
            .arg(&source));
        let output = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&classpath)
            .arg("VideoSliderOracle"));
        let text = String::from_utf8(output).unwrap();
        let mut lines = text.lines();
        for i in 0..=10000 {
            let expected: u16 = lines.next().unwrap().parse().unwrap();
            assert_eq!(
                fps_from_slider(f64::from(i) / 10000.0),
                expected,
                "slider sample {i}"
            );
        }
        for value in 1..=26 {
            let expected: f64 = lines.next().unwrap().parse().unwrap();
            assert_eq!(
                slider_from_fps(value * 10),
                expected,
                "FPS value {}",
                value * 10
            );
        }
        for value in 1..=26 {
            let previous: u16 = lines.next().unwrap().parse().unwrap();
            let next: u16 = lines.next().unwrap().parse().unwrap();
            let original = VideoSettings {
                vsync: true,
                max_fps: value * 10,
                fov: 70,
                view_bobbing: true,
                inactivity_fps_limit: InactivityFpsLimit::Afk,
                ..VideoSettings::default()
            };
            let mut lower = original;
            let mut lower_ui = VideoUi::new(original);
            lower_ui.arrow(false, &mut lower);
            let mut higher = original;
            let mut higher_ui = VideoUi::new(original);
            higher_ui.arrow(true, &mut higher);
            assert_eq!(lower.max_fps, previous);
            assert_eq!(higher.max_fps, next);
        }
        for i in 0..=10000 {
            let expected: u16 = lines.next().unwrap().parse().unwrap();
            assert_eq!(
                crate::fov_ui::from_slider(f64::from(i) / 10000.0),
                expected,
                "FOV slider sample {i}"
            );
        }
        for fov in 30..=110 {
            let expected: f64 = lines.next().unwrap().parse().unwrap();
            let previous: u16 = lines.next().unwrap().parse().unwrap();
            let next: u16 = lines.next().unwrap().parse().unwrap();
            assert_eq!(crate::fov_ui::to_slider(fov), expected, "FOV value {fov}");
            let initial = VideoSettings {
                fov,
                ..VideoSettings::default()
            };
            let mut lower = initial;
            let mut higher = initial;
            crate::fov_ui::FovSlider::new(fov).arrow(false, &mut lower);
            crate::fov_ui::FovSlider::new(fov).arrow(true, &mut higher);
            // Slider values are clamped after Java's range returns its neighbor.
            assert_eq!(lower.fov, previous.clamp(30, 110));
            assert_eq!(higher.fov, next.clamp(30, 110));
        }
        for i in 0..=10000 {
            let expected: u8 = lines.next().unwrap().parse().unwrap();
            assert_eq!(
                crate::blur_ui::from_slider(f64::from(i) / 10000.0),
                expected,
                "blur slider sample {i}"
            );
        }
        for value in 0..=10 {
            let expected: f64 = lines.next().unwrap().parse().unwrap();
            assert_eq!(
                crate::blur_ui::to_slider(value),
                expected,
                "blur value {value}"
            );
        }
        for i in 0..=10000 {
            let expected: u8 = lines.next().unwrap().parse().unwrap();
            assert_eq!(
                distance_from_slider(f64::from(i) / 10000.0),
                expected,
                "render distance sample {i}"
            );
        }
        for value in 2..=32 {
            let expected: f64 = lines.next().unwrap().parse().unwrap();
            assert_eq!(
                slider_from_distance(value),
                expected,
                "render distance {value}"
            );
        }
        for i in 0..=10000 {
            let expected: u8 = lines.next().unwrap().parse().unwrap();
            assert_eq!(
                blend_from_slider(f64::from(i) / 10000.0),
                expected,
                "biome blend slider sample {i}"
            );
        }
        for value in 0..=7 {
            let expected: f64 = lines.next().unwrap().parse().unwrap();
            assert_eq!(
                slider_from_blend(value),
                expected,
                "biome blend radius {value}"
            );
        }
        for width in [320, 426, 640] {
            for height in [240, 360] {
                for scroll in [0, 12, 47] {
                    let expected: Vec<i32> = lines
                        .next()
                        .unwrap()
                        .split(',')
                        .map(|part| part.parse().unwrap())
                        .collect();
                    let mut ui = VideoUi::new(VideoSettings::default());
                    ui.scroll = f64::from(scroll);
                    let rect = ui.control_rect(width, VideoControl::Framerate);
                    assert_eq!(
                        [rect.x, rect.y, VideoUi::max_scroll(height)],
                        expected.as_slice()
                    );
                    let expected_blur: Vec<i32> = lines
                        .next()
                        .unwrap()
                        .split(',')
                        .map(|part| part.parse().unwrap())
                        .collect();
                    let blur = ui.control_rect(width, VideoControl::MenuBlur);
                    assert_eq!(
                        [blur.x, blur.y, VideoUi::max_scroll(height)],
                        expected_blur.as_slice()
                    );
                }
            }
        }
        for width in [320, 426, 640] {
            for height in [240, 360] {
                for scroll in [0, 12, 47] {
                    let expected: Vec<i32> = lines
                        .next()
                        .unwrap()
                        .split(',')
                        .map(|part| part.parse().unwrap())
                        .collect();
                    let mut ui = crate::accessibility_ui::AccessibilityUi::default();
                    ui.scroll_by(-f64::from(scroll) / 12.0, height);
                    let rect = ui.rect(width);
                    assert_eq!(
                        [
                            rect.x,
                            rect.y,
                            crate::accessibility_ui::AccessibilityUi::maximum(height)
                        ],
                        expected.as_slice()
                    );
                    let expected_blur: Vec<i32> = lines
                        .next()
                        .unwrap()
                        .split(',')
                        .map(|part| part.parse().unwrap())
                        .collect();
                    let blur = ui.control_rect(
                        width,
                        crate::accessibility_ui::AccessibilityControl::MenuBlur,
                    );
                    assert_eq!(
                        [
                            blur.x,
                            blur.y,
                            crate::accessibility_ui::AccessibilityUi::maximum(height)
                        ],
                        expected_blur.as_slice()
                    );
                }
            }
        }
        for width in [320, 426, 640] {
            for height in [240, 360] {
                for scroll in [0, 12, 47] {
                    let expected: Vec<i32> = lines
                        .next()
                        .unwrap()
                        .split(',')
                        .map(|part| part.parse().unwrap())
                        .collect();
                    let mut ui = crate::accessibility_ui::AccessibilityUi::default();
                    ui.scroll_by(-f64::from(scroll) / 12.0, height);
                    let rect = ui.control_rect(
                        width,
                        crate::accessibility_ui::AccessibilityControl::PanoramaSpeed,
                    );
                    assert_eq!(
                        [
                            rect.x,
                            rect.y,
                            crate::accessibility_ui::AccessibilityUi::maximum(height)
                        ],
                        expected.as_slice()
                    );
                }
            }
        }
        for x in 50..=220 {
            let mut ui = crate::accessibility_ui::AccessibilityUi::default();
            let mut video = VideoSettings::default();
            let mut panorama_ui = crate::accessibility_ui::AccessibilityUi::default();
            let mut panorama = VideoSettings::default();
            ui.start_slider(
                426,
                x,
                crate::accessibility_ui::AccessibilityControl::GlintSpeed,
                &mut video,
            );
            let expected: i64 = lines.next().unwrap().parse().unwrap();
            panorama_ui.start_slider(
                426,
                x,
                crate::accessibility_ui::AccessibilityControl::PanoramaSpeed,
                &mut panorama,
            );
            assert_eq!(
                panorama.panorama_speed.value().to_bits() as i64,
                expected,
                "panorama mouse x {x}"
            );
            assert_eq!(
                video.glint_speed.value().to_bits() as i64,
                expected,
                "glint mouse x {x}"
            );
            ui.arrow(
                crate::accessibility_ui::AccessibilityControl::GlintSpeed,
                false,
                &mut video,
            );
            let expected: i64 = lines.next().unwrap().parse().unwrap();
            panorama_ui.arrow(
                crate::accessibility_ui::AccessibilityControl::PanoramaSpeed,
                false,
                &mut panorama,
            );
            assert_eq!(
                panorama.panorama_speed.value().to_bits() as i64,
                expected,
                "panorama left x {x}"
            );
            assert_eq!(
                video.glint_speed.value().to_bits() as i64,
                expected,
                "glint left x {x}"
            );
            ui.arrow(
                crate::accessibility_ui::AccessibilityControl::GlintSpeed,
                true,
                &mut video,
            );
            let expected: i64 = lines.next().unwrap().parse().unwrap();
            panorama_ui.arrow(
                crate::accessibility_ui::AccessibilityControl::PanoramaSpeed,
                true,
                &mut panorama,
            );
            assert_eq!(
                panorama.panorama_speed.value().to_bits() as i64,
                expected,
                "panorama right x {x}"
            );
            assert_eq!(
                video.glint_speed.value().to_bits() as i64,
                expected,
                "glint right x {x}"
            );
        }
        assert!(lines.next().is_none());
        println!(
            "Compared 10,001 positions for each slider, FPS and FOV values and keyboard steps, and 18 list layouts with Java 26.3"
        );
    }

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn video_screen_composes_local_sprites_and_displays_values() {
        use std::sync::Arc;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&game, &[]).unwrap());
        let mut canvas = Canvas::new(426, 240);
        canvas.fill(
            Rect {
                x: 0,
                y: 0,
                w: 426,
                h: 240,
            },
            [24, 24, 24, 255],
        );
        let ui = VideoUi::new(VideoSettings::default());
        ui.draw(&mut canvas, &assets, VideoSettings::default(), (0, 0), None);
        let before = canvas.pixels.clone();
        let mut changed = Canvas::new(426, 240);
        changed.fill(
            Rect {
                x: 0,
                y: 0,
                w: 426,
                h: 240,
            },
            [24, 24, 24, 255],
        );
        ui.draw(
            &mut changed,
            &assets,
            VideoSettings {
                vsync: false,
                max_fps: 120,
                fov: 70,
                view_bobbing: true,
                inactivity_fps_limit: InactivityFpsLimit::Afk,
                ..VideoSettings::default()
            },
            (220, 85),
            None,
        );
        assert_ne!(before, changed.pixels);
        if let Some(path) = std::env::var_os("NATIVE_VIDEO_PREVIEW") {
            let image =
                image::RgbaImage::from_fn(canvas.width as u32, canvas.height as u32, |x, y| {
                    let pixel = canvas.pixels[(y * canvas.width as u32 + x) as usize];
                    image::Rgba([
                        (pixel >> 16) as u8,
                        (pixel >> 8) as u8,
                        pixel as u8,
                        (pixel >> 24) as u8,
                    ])
                });
            image.save(path).unwrap();
        }
    }
}
