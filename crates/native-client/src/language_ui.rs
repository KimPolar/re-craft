//! Language selection for the client menus.

use crate::{
    chat_input::ChatInput,
    menu_assets::{LanguageInfo, MenuAssets},
    menu_draw::{Canvas, Rect},
};

pub struct LanguageUi {
    entries: Vec<(String, LanguageInfo)>,
    pub search: ChatInput,
    selected: Option<String>,
    scroll: usize,
    scroll_remainder: f64,
    dragging_scrollbar: bool,
}

impl LanguageUi {
    pub fn new(assets: &MenuAssets, height: i32) -> Self {
        let mut entries: Vec<_> = assets
            .languages
            .iter()
            .map(|(code, info)| (code.clone(), info.clone()))
            .collect();
        entries.sort_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()));
        let selected = assets
            .languages
            .contains_key(&assets.locale)
            .then(|| assets.locale.clone());
        let selected_index = selected
            .as_ref()
            .and_then(|code| entries.iter().position(|(entry, _)| entry == code));
        let visible = Self::visible_rows(height);
        let scroll = selected_index
            .unwrap_or(0)
            .saturating_sub(visible / 2)
            .min(entries.len().saturating_sub(visible));
        Self {
            entries,
            search: ChatInput::field(""),
            selected,
            scroll,
            scroll_remainder: 0.0,
            dragging_scrollbar: false,
        }
    }

    pub fn search_rect(width: i32) -> Rect {
        Rect {
            x: width / 2 - 100,
            y: 17,
            w: 200,
            h: 15,
        }
    }
    pub fn list_rect(width: i32, height: i32) -> Rect {
        Rect {
            x: width / 2 - 135,
            y: 36,
            w: 270,
            h: (height - 89).max(0),
        }
    }
    pub fn done_rect(width: i32, height: i32) -> Rect {
        Rect {
            x: width / 2 + 4,
            y: height - 27,
            w: 150,
            h: 20,
        }
    }
    pub fn font_rect(width: i32, height: i32) -> Rect {
        Rect {
            x: width / 2 - 154,
            y: height - 27,
            w: 150,
            h: 20,
        }
    }
    fn visible_rows(height: i32) -> usize {
        ((height - 89).max(0) / 18) as usize
    }

    fn matches(&self, info: &LanguageInfo) -> bool {
        let query = self.search.value().to_lowercase();
        query.is_empty()
            || info.name.to_lowercase().contains(&query)
            || info.region.to_lowercase().contains(&query)
    }

    fn filtered(&self) -> Vec<&(String, LanguageInfo)> {
        self.entries
            .iter()
            .filter(|(_, info)| self.matches(info))
            .collect()
    }

    pub fn changed_search(&mut self, height: i32) {
        self.selected = None;
        self.scroll = self.scroll.min(
            self.filtered()
                .len()
                .saturating_sub(Self::visible_rows(height)),
        );
    }

    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub fn click(&mut self, width: i32, height: i32, cursor: (i32, i32)) -> bool {
        let list = Self::list_rect(width, height);
        if !list.contains(cursor.0, cursor.1) {
            return false;
        }
        let row = ((cursor.1 - list.y) / 18) as usize;
        if row >= Self::visible_rows(height) {
            return false;
        }
        let selected = self
            .filtered()
            .get(self.scroll + row)
            .map(|entry| entry.0.clone());
        if let Some(code) = selected {
            self.selected = Some(code);
            return true;
        }
        false
    }

    pub fn scroll_by(&mut self, amount: f64, height: i32) {
        if !amount.is_finite() {
            return;
        }
        self.scroll_remainder += amount;
        let steps = self.scroll_remainder.trunc() as i32;
        self.scroll_remainder -= f64::from(steps);
        let max = self
            .filtered()
            .len()
            .saturating_sub(Self::visible_rows(height));
        self.scroll = self
            .scroll
            .saturating_add_signed(-(steps as isize))
            .min(max);
    }

    fn scrollbar(&self, width: i32, height: i32) -> Option<(Rect, Rect)> {
        let list = Self::list_rect(width, height);
        let rows = self.filtered().len();
        let visible = Self::visible_rows(height);
        if rows <= visible || list.h <= 0 {
            return None;
        }
        let thumb_height = (list.h * list.h / (rows as i32 * 18)).clamp(32.min(list.h), list.h);
        let thumb_y =
            list.y + (self.scroll as i32 * (list.h - thumb_height) / (rows - visible) as i32);
        Some((
            Rect {
                x: width / 2 + 140,
                y: list.y,
                w: 6,
                h: list.h,
            },
            Rect {
                x: width / 2 + 140,
                y: thumb_y,
                w: 6,
                h: thumb_height,
            },
        ))
    }

    pub fn start_scrollbar(&mut self, width: i32, height: i32, cursor: (i32, i32)) -> bool {
        if self
            .scrollbar(width, height)
            .is_some_and(|(track, _)| track.contains(cursor.0, cursor.1))
        {
            self.dragging_scrollbar = true;
            self.drag_scrollbar(width, height, cursor.1);
            true
        } else {
            false
        }
    }

    pub fn drag_scrollbar(&mut self, width: i32, height: i32, y: i32) -> bool {
        if !self.dragging_scrollbar {
            return false;
        }
        if let Some((track, thumb)) = self.scrollbar(width, height) {
            let max = self
                .filtered()
                .len()
                .saturating_sub(Self::visible_rows(height));
            let range = (track.h - thumb.h).max(1);
            self.scroll = (((y - track.y - thumb.h / 2).clamp(0, range) as f64 / f64::from(range))
                * max as f64)
                .round() as usize;
        }
        true
    }

    pub fn release(&mut self) {
        self.dragging_scrollbar = false;
    }

    pub fn move_selection(&mut self, down: bool, height: i32) {
        let rows = self.filtered();
        if rows.is_empty() {
            return;
        }
        let index = self
            .selected
            .as_ref()
            .and_then(|code| rows.iter().position(|entry| &entry.0 == code))
            .map_or(0, |index| {
                if down {
                    (index + 1).min(rows.len() - 1)
                } else {
                    index.saturating_sub(1)
                }
            });
        let selected = rows[index].0.clone();
        let visible = Self::visible_rows(height).max(1);
        self.scroll = if index < self.scroll {
            index
        } else if index >= self.scroll + visible {
            index + 1 - visible
        } else {
            self.scroll
        };
        self.selected = Some(selected);
    }

    pub fn draw(&self, canvas: &mut Canvas, assets: &MenuAssets, focused: bool) {
        let (w, h) = (canvas.width, canvas.height);
        canvas.text_centered(
            Some(assets),
            w / 2,
            4,
            assets.translate("options.language.title", "Language"),
            [255, 255, 255],
        );
        self.search.draw_field(
            canvas,
            assets,
            Self::search_rect(w),
            focused,
            assets.translate("gui.language.search", "Search..."),
        );
        let list = Self::list_rect(w, h);
        canvas.fill(list, [0, 0, 0, 160]);
        for (row, entry) in self
            .filtered()
            .into_iter()
            .skip(self.scroll)
            .take(Self::visible_rows(h))
            .enumerate()
        {
            let rect = Rect {
                x: list.x,
                y: list.y + row as i32 * 18,
                w: list.w,
                h: 18,
            };
            if self.selected.as_deref() == Some(entry.0.as_str()) {
                canvas.fill(rect, [160, 160, 160, 255]);
                canvas.fill(
                    Rect {
                        x: rect.x + 1,
                        y: rect.y + 1,
                        w: rect.w - 2,
                        h: rect.h - 2,
                    },
                    [0, 0, 0, 255],
                );
            }
            canvas.text_centered(
                Some(assets),
                w / 2,
                rect.y + 5,
                &format!("{} ({})", entry.1.name, entry.1.region),
                [255, 255, 255],
            );
        }
        if let Some((track, thumb)) = self.scrollbar(w, h) {
            canvas.gui_sprite(
                &assets.scroller_background.image,
                track,
                assets.scroller_background.scaling,
            );
            canvas.gui_sprite(&assets.scroller.image, thumb, assets.scroller.scaling);
        }
        canvas.text_centered(
            Some(assets),
            w / 2,
            h - 44,
            assets.translate(
                "options.languageAccuracyWarning",
                "Language translations may not be 100% accurate",
            ),
            [186, 186, 186],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searches_language_names_and_regions_and_selects_visible_rows() {
        let mut ui = LanguageUi {
            entries: vec![
                (
                    "en_us".into(),
                    LanguageInfo {
                        name: "English".into(),
                        region: "US".into(),
                        bidirectional: false,
                    },
                ),
                (
                    "ko_kr".into(),
                    LanguageInfo {
                        name: "한국어".into(),
                        region: "대한민국".into(),
                        bidirectional: false,
                    },
                ),
            ],
            search: ChatInput::field(""),
            selected: Some("en_us".into()),
            scroll: 0,
            scroll_remainder: 0.0,
            dragging_scrollbar: false,
        };
        ui.search.insert("대한");
        ui.changed_search(240);
        assert_eq!(ui.filtered().len(), 1);
        assert_eq!(ui.selected(), None);
        assert!(ui.click(320, 240, (160, 40)));
        assert_eq!(ui.selected(), Some("ko_kr"));
        assert!(!ui.click(320, 240, (0, 40)));
        ui.search.open(false);
        ui.search.insert("ENGL");
        ui.changed_search(240);
        ui.move_selection(true, 240);
        assert_eq!(ui.selected(), Some("en_us"));
    }

    #[test]
    fn scrolling_is_clamped_and_selection_is_kept_visible() {
        let mut ui = LanguageUi {
            entries: (0..30)
                .map(|i| {
                    (
                        format!("code_{i:02}"),
                        LanguageInfo {
                            name: format!("Name {i}"),
                            region: "Place".into(),
                            bidirectional: false,
                        },
                    )
                })
                .collect(),
            search: ChatInput::field(""),
            selected: None,
            scroll: 0,
            scroll_remainder: 0.0,
            dragging_scrollbar: false,
        };
        ui.scroll_by(-1000.0, 240);
        assert_eq!(ui.scroll, 22);
        ui.scroll_by(1000.0, 240);
        assert_eq!(ui.scroll, 0);
        for _ in 0..10 {
            ui.move_selection(true, 240);
        }
        assert_eq!(ui.selected(), Some("code_09"));
        assert_eq!(ui.scroll, 2);
        assert!(ui.start_scrollbar(320, 240, (301, 180)));
        assert!(ui.drag_scrollbar(320, 240, 400));
        assert_eq!(ui.scroll, 22);
        ui.release();
        assert!(!ui.drag_scrollbar(320, 240, 40));
    }
}
