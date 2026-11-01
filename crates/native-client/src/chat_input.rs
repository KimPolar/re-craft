//! Local chat editor. Indices are UTF-8 boundaries; the Java length limit is UTF-16.

use winit::keyboard::KeyCode;

use crate::{
    menu_assets::MenuAssets,
    menu_draw::{Canvas, Rect},
};

#[derive(Debug, PartialEq, Eq)]
pub enum ChatAction {
    None,
    Close,
    Submit(String),
}

#[derive(Default)]
pub struct ChatInput {
    open: bool,
    text: String,
    caret: usize,
    anchor: usize,
    preedit: String,
    history: Vec<String>,
    history_position: usize,
    history_draft: String,
    scroll: usize,
    focused_at: Option<std::time::Instant>,
}

impl ChatInput {
    pub fn is_open(&self) -> bool {
        self.open
    }
    pub fn scroll_offset(&self) -> usize {
        self.scroll
    }
    pub fn composing(&self) -> bool {
        !self.preedit.is_empty()
    }

    pub fn blink_key(&self) -> u64 {
        self.focused_at
            .map_or(0, |start| (start.elapsed().as_millis() / 300) as u64)
    }

    pub fn open(&mut self, command: bool) {
        self.open = true;
        self.focused_at = Some(std::time::Instant::now());
        self.text = if command { "/".into() } else { String::new() };
        self.caret = self.text.len();
        self.anchor = self.caret;
        self.preedit.clear();
        self.history_position = self.history.len();
        self.history_draft.clear();
        self.scroll = 0;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.focused_at = None;
        self.preedit.clear();
        self.scroll = 0;
    }

    pub fn set_preedit(&mut self, text: String) {
        self.preedit = text;
    }

    pub fn insert(&mut self, text: &str) {
        let (start, end) = self.selection();
        let available = 256usize.saturating_sub(
            self.text[..start].encode_utf16().count() + self.text[end..].encode_utf16().count(),
        );
        let mut addition = String::new();
        let mut used = 0;
        for ch in text
            .chars()
            .filter(|ch| *ch != '\u{a7}' && *ch >= ' ' && *ch != '\u{7f}')
        {
            let len = ch.len_utf16();
            if used + len > available {
                break;
            }
            addition.push(ch);
            used += len;
        }
        if addition.is_empty() {
            return;
        }
        self.text.replace_range(start..end, &addition);
        self.caret = start + addition.len();
        self.anchor = self.caret;
        self.preedit.clear();
    }

    pub fn scroll(&mut self, delta: i32) {
        if delta > 0 {
            self.scroll = self.scroll.saturating_add(delta as usize).min(10000);
        } else {
            self.scroll = self.scroll.saturating_sub(delta.unsigned_abs() as usize);
        }
    }

    pub fn clamp_scroll(&mut self, maximum: usize) {
        self.scroll = self.scroll.min(maximum);
    }

    fn selection(&self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }

    fn previous(&self, index: usize) -> usize {
        self.text[..index]
            .char_indices()
            .next_back()
            .map_or(0, |(at, _)| at)
    }

    fn next(&self, index: usize) -> usize {
        self.text[index..]
            .chars()
            .next()
            .map_or(index, |ch| index + ch.len_utf8())
    }

    fn visible_start(&self, canvas: &Canvas, assets: &MenuAssets, available: i32) -> usize {
        let mut start = 0;
        let mut width = canvas.text_width(Some(assets), &self.text[..self.caret]);
        while start < self.caret && width >= available {
            let next = self.next(start);
            width -= canvas.text_width(Some(assets), &self.text[start..next]);
            start = next;
        }
        start
    }

    pub fn cursor_x(&self, assets: &MenuAssets, width: i32) -> i32 {
        let canvas = Canvas::new(1, 1);
        let start = self.visible_start(&canvas, assets, (width - 8).max(1));
        4 + canvas.text_width(Some(assets), &self.text[start..self.caret])
    }

    pub fn click(&mut self, assets: &MenuAssets, width: i32, x: i32, shift: bool) {
        let canvas = Canvas::new(1, 1);
        let start = self.visible_start(&canvas, assets, (width - 8).max(1));
        let target = (x - 4).max(0);
        let mut index = start;
        let mut previous_width = 0;
        while index < self.text.len() {
            let next = self.next(index);
            let next_width =
                previous_width + canvas.text_width(Some(assets), &self.text[index..next]);
            if target < (previous_width + next_width) / 2 {
                break;
            }
            index = next;
            previous_width = next_width;
            if next_width >= width - 8 {
                break;
            }
        }
        self.move_caret(index, shift);
    }

    fn move_caret(&mut self, index: usize, shift: bool) {
        self.caret = index;
        if !shift {
            self.anchor = index;
        }
        self.preedit.clear();
    }

    fn history(&mut self, direction: i32) {
        let next =
            (self.history_position as i32 + direction).clamp(0, self.history.len() as i32) as usize;
        if next == self.history_position {
            return;
        }
        if self.history_position == self.history.len() {
            self.history_draft = self.text.clone();
        }
        self.history_position = next;
        self.text = if next == self.history.len() {
            self.history_draft.clone()
        } else {
            self.history[next].clone()
        };
        self.caret = self.text.len();
        self.anchor = self.caret;
        self.preedit.clear();
    }

    pub fn key(&mut self, key: KeyCode, control: bool, shift: bool) -> ChatAction {
        match key {
            KeyCode::Escape => {
                self.close();
                return ChatAction::Close;
            }
            KeyCode::Enter | KeyCode::NumpadEnter => {
                let text = self.text.split_whitespace().collect::<Vec<_>>().join(" ");
                if !text.is_empty() && self.history.last() != Some(&text) {
                    if self.history.len() == 100 {
                        self.history.remove(0);
                    }
                    self.history.push(text.clone());
                }
                self.close();
                return ChatAction::Submit(text);
            }
            KeyCode::KeyA if control => {
                self.anchor = 0;
                self.caret = self.text.len();
            }
            KeyCode::ArrowUp => self.history(-1),
            KeyCode::ArrowDown => self.history(1),
            KeyCode::Home => self.move_caret(0, shift),
            KeyCode::End => self.move_caret(self.text.len(), shift),
            KeyCode::ArrowLeft => {
                let (start, end) = self.selection();
                let index = if !shift && start != end {
                    start
                } else {
                    self.previous(self.caret)
                };
                self.move_caret(index, shift);
            }
            KeyCode::ArrowRight => {
                let (start, end) = self.selection();
                let index = if !shift && start != end {
                    end
                } else {
                    self.next(self.caret)
                };
                self.move_caret(index, shift);
            }
            KeyCode::Backspace | KeyCode::Delete => {
                let (mut start, mut end) = self.selection();
                if start == end {
                    if key == KeyCode::Backspace {
                        start = self.previous(start);
                    } else {
                        end = self.next(end);
                    }
                }
                self.text.replace_range(start..end, "");
                self.caret = start;
                self.anchor = start;
                self.preedit.clear();
            }
            _ => {}
        }
        ChatAction::None
    }

    pub fn draw(&self, canvas: &mut Canvas, assets: &MenuAssets) {
        let width = canvas.width;
        let height = canvas.height;
        canvas.fill(
            Rect {
                x: 2,
                y: height - 14,
                w: (width - 4).max(0),
                h: 12,
            },
            [0, 0, 0, 128],
        );
        let available = (width - 8).max(1);
        let start = self.visible_start(canvas, assets, available);
        let mut edit = Canvas::new(available as u32, 12);
        let (sel_start, sel_end) = self.selection();
        let pos = |at: usize| canvas.text_width(Some(assets), &self.text[start..at.max(start)]);
        if sel_start != sel_end && sel_end > start {
            let left = pos(sel_start);
            let right = pos(sel_end);
            edit.fill(
                Rect {
                    x: left,
                    y: 0,
                    w: right - left,
                    h: 10,
                },
                [0, 0, 255, 255],
            );
        }
        let cursor = pos(self.caret);
        edit.text(
            Some(assets),
            0,
            0,
            &self.text[start..self.caret],
            [224, 224, 224],
        );
        if !self.preedit.is_empty() {
            edit.text(Some(assets), cursor, 0, &self.preedit, [255, 255, 255]);
            let length = edit.text_width(Some(assets), &self.preedit);
            edit.text(
                Some(assets),
                cursor + length,
                0,
                &self.text[self.caret..],
                [224, 224, 224],
            );
            edit.fill(
                Rect {
                    x: cursor,
                    y: 10,
                    w: length,
                    h: 1,
                },
                [255, 255, 255, 255],
            );
        } else {
            edit.text(
                Some(assets),
                cursor,
                0,
                &self.text[self.caret..],
                [224, 224, 224],
            );
            if self.blink_key() % 2 == 0 {
                edit.fill(
                    Rect {
                        x: cursor,
                        y: 0,
                        w: 1,
                        h: 10,
                    },
                    [208, 208, 208, 255],
                );
            }
        }
        canvas.composite(&edit, 4, height - 12);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_limit_selection_and_composition_are_safe() {
        let mut input = ChatInput::default();
        input.open(false);
        input.insert(&"a".repeat(254));
        input.insert("😀Z");
        assert_eq!(input.text.encode_utf16().count(), 256);
        assert!(input.text.ends_with('😀'));
        input.key(KeyCode::ArrowLeft, false, true);
        input.set_preedit("한".into());
        input.insert("한");
        assert!(input.text.ends_with('한'));
        assert!(input.preedit.is_empty());
        input.key(KeyCode::Backspace, false, false);
        assert_eq!(input.text, "a".repeat(254));
        input.key(KeyCode::KeyA, true, false);
        input.insert("§x\n가");
        assert_eq!(input.text, "x가");
    }

    #[test]
    fn history_restores_draft_and_deduplicates_adjacent_messages() {
        let mut input = ChatInput::default();
        for text in ["  hello   world  ", "hello world", "/time set day"] {
            input.open(false);
            input.insert(text);
            assert!(matches!(
                input.key(KeyCode::Enter, false, false),
                ChatAction::Submit(_)
            ));
        }
        assert_eq!(input.history, ["hello world", "/time set day"]);
        input.open(false);
        input.insert("draft");
        input.key(KeyCode::ArrowUp, false, false);
        assert_eq!(input.text, "/time set day");
        input.key(KeyCode::ArrowUp, false, false);
        assert_eq!(input.text, "hello world");
        input.key(KeyCode::ArrowDown, false, false);
        input.key(KeyCode::ArrowDown, false, false);
        assert_eq!(input.text, "draft");
    }
}
