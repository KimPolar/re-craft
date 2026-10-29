//! The 26.3 sound category sliders and their scrollable options list.
use crate::{
    menu_assets::MenuAssets,
    menu_draw::{Canvas, Rect},
    menu_settings::{SoundCategory, SoundSettings},
};

const TOP: i32 = 33;
const FOOTER: i32 = 33;
const CONTENT_HEIGHT: i32 = 229;

#[derive(Default)]
pub struct SoundUi {
    scroll: f64,
    dragging: Option<SoundCategory>,
    scrollbar: Option<i32>,
    pub keyboard_adjust: bool,
}

impl SoundUi {
    fn viewport(height: i32) -> i32 {
        (height - TOP - FOOTER).max(0)
    }
    fn maximum(height: i32) -> i32 {
        (CONTENT_HEIGHT - Self::viewport(height)).max(0)
    }
    pub fn rect(&self, width: i32, category: SoundCategory) -> Rect {
        let index = category as i32;
        if index == 0 {
            Rect {
                x: width / 2 - 155,
                y: 37 - self.scroll as i32,
                w: 310,
                h: 20,
            }
        } else {
            Rect {
                x: width / 2 - 155 + (index - 1) % 2 * 160,
                y: 62 + (index - 1) / 2 * 25 - self.scroll as i32,
                w: 150,
                h: 20,
            }
        }
    }
    pub fn hit(&self, width: i32, height: i32, cursor: (i32, i32)) -> Option<SoundCategory> {
        if !(TOP..height - FOOTER).contains(&cursor.1) {
            return None;
        }
        SoundCategory::ALL
            .into_iter()
            .find(|category| self.rect(width, *category).contains(cursor.0, cursor.1))
    }
    pub fn scroll_by(&mut self, delta: f64, height: i32) {
        self.scroll = (self.scroll - delta * 12.0).clamp(0.0, f64::from(Self::maximum(height)));
    }
    pub fn resize(&mut self, height: i32) {
        self.scroll = self.scroll.clamp(0.0, f64::from(Self::maximum(height)));
    }
    pub fn ensure_visible(&mut self, height: i32, category: SoundCategory) {
        let rect = self.rect(0, category);
        if rect.y < TOP {
            self.scroll -= f64::from(TOP - rect.y);
        }
        if rect.y + rect.h > height - FOOTER {
            self.scroll += f64::from(rect.y + rect.h - (height - FOOTER));
        }
        self.resize(height);
    }
    fn set_x(
        &self,
        width: i32,
        x: i32,
        category: SoundCategory,
        settings: &mut SoundSettings,
    ) -> bool {
        let rect = self.rect(width, category);
        settings.set(category, f64::from(x - rect.x - 4) / f64::from(rect.w - 8))
    }
    pub fn start(
        &mut self,
        width: i32,
        x: i32,
        category: SoundCategory,
        settings: &mut SoundSettings,
    ) -> bool {
        self.dragging = Some(category);
        self.keyboard_adjust = true;
        self.set_x(width, x, category, settings)
    }
    pub fn arrow(
        &mut self,
        category: SoundCategory,
        right: bool,
        settings: &mut SoundSettings,
    ) -> bool {
        let width = if category == SoundCategory::Master {
            310
        } else {
            150
        };
        let direction = if right { 1.0_f32 } else { -1.0_f32 };
        let step = f64::from(direction / (width - 8) as f32);
        settings.set(category, settings.get(category) + step)
    }
    pub fn start_scrollbar(&mut self, width: i32, height: i32, cursor: (i32, i32)) -> bool {
        let x = width / 2 + 163;
        if Self::maximum(height) > 0
            && (x..x + 6).contains(&cursor.0)
            && (TOP..height - FOOTER).contains(&cursor.1)
        {
            self.scrollbar = Some(cursor.1);
            true
        } else {
            false
        }
    }
    pub fn pointer_move(
        &mut self,
        width: i32,
        height: i32,
        cursor: (i32, i32),
        settings: &mut SoundSettings,
    ) -> (bool, bool) {
        if let Some(category) = self.dragging {
            return (true, self.set_x(width, cursor.0, category, settings));
        }
        if let Some(previous) = self.scrollbar {
            let viewport = Self::viewport(height);
            let thumb = ((viewport * viewport) / CONTENT_HEIGHT).clamp(32, (viewport - 8).max(32));
            let range = (viewport - thumb).max(1);
            self.scroll = (self.scroll
                + f64::from(cursor.1 - previous)
                    * (f64::from(Self::maximum(height)) / f64::from(range)).max(1.0))
            .clamp(0.0, f64::from(Self::maximum(height)));
            self.scrollbar = Some(cursor.1);
            return (true, false);
        }
        (false, false)
    }
    pub fn release(&mut self) {
        self.dragging = None;
        self.scrollbar = None;
    }

    pub fn draw(
        &self,
        canvas: &mut Canvas,
        assets: &MenuAssets,
        settings: SoundSettings,
        cursor: (i32, i32),
        focused: Option<SoundCategory>,
    ) {
        let width = canvas.width;
        let height = canvas.height;
        let viewport = Self::viewport(height);
        if viewport == 0 {
            return;
        }
        let mut list = Canvas::new(width as u32, viewport as u32);
        for category in SoundCategory::ALL {
            let rect = self.rect(width, category);
            let rect_in_list = Rect {
                y: rect.y - TOP,
                ..rect
            };
            let hover =
                self.hit(width, height, cursor) == Some(category) || focused == Some(category);
            let background = if hover {
                &assets.slider_highlighted
            } else {
                &assets.slider
            };
            let handle = if hover {
                &assets.slider_handle_highlighted
            } else {
                &assets.slider_handle
            };
            list.gui_sprite(&background.image, rect_in_list, background.scaling);
            list.gui_sprite(
                &handle.image,
                Rect {
                    x: rect.x + (settings.get(category) * f64::from(rect.w - 8)) as i32,
                    y: rect_in_list.y,
                    w: 8,
                    h: 20,
                },
                handle.scaling,
            );
            let key = format!("soundCategory.{}", category.name());
            let label = assets.translate(&key, category.name());
            let value = settings.get(category);
            let caption = if value == 0.0 {
                assets
                    .translate("options.off.composed", "%s: OFF")
                    .replacen("%s", label, 1)
            } else {
                let format = assets.translate("options.percent_value", "%s: %s%%");
                let percent = ((value * 100.0) as i32).to_string();
                if format.contains("%1$s") || format.contains("%2$s") {
                    format
                        .replace("%1$s", label)
                        .replace("%2$s", &percent)
                        .replace("%%", "%")
                } else {
                    format
                        .replacen("%s", label, 1)
                        .replacen("%s", &percent, 1)
                        .replace("%%", "%")
                }
            };
            list.text_centered(
                Some(assets),
                rect.x + rect.w / 2,
                rect_in_list.y + 6,
                &caption,
                [255, 255, 255],
            );
        }
        for (row, key, fallback, right) in [
            (187, "options.audioDevice", "Sound Device", false),
            (212, "options.showSubtitles", "Closed Captions", false),
            (212, "options.directionalAudio", "Directional Audio", true),
            (237, "options.musicFrequency", "Music Frequency", false),
            (237, "options.musicToast", "Music Toast", true),
        ] {
            let width_part = if row == 187 { 310 } else { 150 };
            list.button(
                Some(assets),
                Rect {
                    x: width / 2 - 155 + if right { 160 } else { 0 },
                    y: row - self.scroll as i32 - TOP,
                    w: width_part,
                    h: 20,
                },
                false,
                false,
                assets.translate(key, fallback),
            );
        }
        canvas.composite(&list, 0, TOP);
        if Self::maximum(height) > 0 {
            let x = width / 2 + 163;
            let thumb = ((viewport * viewport) / CONTENT_HEIGHT).clamp(32, (viewport - 8).max(32));
            let thumb_y = TOP + self.scroll as i32 * (viewport - thumb) / Self::maximum(height);
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
    fn slider_drag_keyboard_and_scrolling_update_the_selected_category() {
        let mut ui = SoundUi::default();
        let mut settings = SoundSettings::default();
        let rect = ui.rect(426, SoundCategory::Master);
        assert_eq!((rect.x, rect.y, rect.w), (58, 37, 310));
        assert!(ui.start(426, rect.x + 4, SoundCategory::Master, &mut settings));
        assert_eq!(settings.get(SoundCategory::Master), 0.0);
        assert_eq!(settings.ui_gain(), 0.0);
        assert_eq!(
            ui.pointer_move(426, 240, (rect.x + rect.w - 4, rect.y), &mut settings),
            (true, true)
        );
        assert_eq!(settings.get(SoundCategory::Master), 1.0);
        ui.release();
        assert!(ui.arrow(SoundCategory::Ui, false, &mut settings));
        assert_eq!(
            settings.get(SoundCategory::Ui),
            1.0 - f64::from(1.0_f32 / 142.0)
        );
        ui.ensure_visible(160, SoundCategory::Ui);
        let rect = ui.rect(426, SoundCategory::Ui);
        assert_eq!(
            ui.hit(426, 160, (rect.x + 2, rect.y + 2)),
            Some(SoundCategory::Ui)
        );
        assert_eq!(ui.hit(426, 160, (rect.x + 2, 32)), None);
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn sound_options_layout_and_slider_match_installed_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("SoundOptionsOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/SoundOptionsOracle.java"),
        )
        .unwrap();
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
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let run = |command: &mut Command| {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "Java sound oracle failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            output.stdout
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
            .arg("SoundOptionsOracle"));
        let text = String::from_utf8(output).unwrap();
        let mut lines = text.lines();
        for category in SoundCategory::ALL {
            assert_eq!(lines.next().unwrap(), category.name());
        }
        for width in [320, 426, 640] {
            for height in [240, 360] {
                for scroll in [0, 12, 47] {
                    let ui = SoundUi {
                        scroll: f64::from(scroll.min(SoundUi::maximum(height))),
                        ..Default::default()
                    };
                    for category in SoundCategory::ALL {
                        let expected: Vec<i32> = lines
                            .next()
                            .unwrap()
                            .split(',')
                            .map(|v| v.parse().unwrap())
                            .collect();
                        let rect = ui.rect(width, category);
                        assert_eq!(
                            [rect.x, rect.y, SoundUi::maximum(height)],
                            expected.as_slice(),
                            "{width}x{height} scroll={scroll} {category:?}"
                        );
                    }
                }
            }
        }
        for category in [SoundCategory::Music, SoundCategory::Master] {
            for x in 45..=380 {
                let mut ui = SoundUi::default();
                let mut sound = SoundSettings::default();
                ui.start(426, x, category, &mut sound);
                assert_eq!(
                    sound.get(category).to_bits(),
                    lines.next().unwrap().parse::<u64>().unwrap(),
                    "{category:?} x={x}"
                );
                ui.arrow(category, false, &mut sound);
                assert_eq!(
                    sound.get(category).to_bits(),
                    lines.next().unwrap().parse::<u64>().unwrap(),
                    "left {category:?} x={x}"
                );
                ui.arrow(category, true, &mut sound);
                assert_eq!(
                    sound.get(category).to_bits(),
                    lines.next().unwrap().parse::<u64>().unwrap(),
                    "right {category:?} x={x}"
                );
            }
        }
        assert!(lines.next().is_none());
    }
}
