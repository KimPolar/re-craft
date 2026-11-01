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
    dragging: bool,
    last_click: Option<std::time::Instant>,
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
        self.dragging = false;
        self.last_click = None;
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
        self.dragging = false;
        self.last_click = None;
        self.focused_at = None;
        self.preedit.clear();
        self.scroll = 0;
    }

    pub fn set_preedit(&mut self, text: String) {
        self.preedit = text;
    }

    pub fn accepts_text(text: &str) -> bool {
        text.chars()
            .any(|ch| ch != '\u{a7}' && ch >= ' ' && ch != '\u{7f}')
    }

    pub fn selected_text(&self) -> &str {
        let (start, end) = self.selection();
        &self.text[start..end]
    }

    pub fn delete_selection(&mut self) {
        let (start, end) = self.selection();
        if start != end {
            self.text.replace_range(start..end, "");
            self.caret = start;
            self.anchor = start;
            self.preedit.clear();
        }
    }

    pub fn insert(&mut self, text: &str) {
        let (start, end) = self.selection();
        let available = 256usize.saturating_sub(
            self.text[..start].encode_utf16().count() + self.text[end..].encode_utf16().count(),
        );
        if available == 0 {
            return;
        }
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
        if addition.is_empty() && start == end {
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

    fn word_position(&self, index: usize, forward: bool) -> usize {
        let bytes = self.text.as_bytes();
        if forward {
            let Some(offset) = self.text[index..].find(' ') else {
                return bytes.len();
            };
            let mut pos = index + offset;
            while pos < bytes.len() && bytes[pos] == b' ' {
                pos += 1;
            }
            pos
        } else {
            let mut pos = index;
            while pos > 0 && bytes[pos - 1] == b' ' {
                pos -= 1;
            }
            while pos > 0 && bytes[pos - 1] != b' ' {
                pos -= 1;
            }
            pos
        }
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
        let index = self.click_position(start, target, width - 8, |glyph| {
            canvas.text_width(Some(assets), glyph)
        });
        self.move_caret(index, shift);
    }

    fn click_position(
        &self,
        start: usize,
        target: i32,
        available: i32,
        width: impl Fn(&str) -> i32,
    ) -> usize {
        let mut index = start;
        let mut previous_width = 0;
        while index < self.text.len() {
            let next = self.next(index);
            let next_width = previous_width + width(&self.text[index..next]);
            if target < next_width {
                break;
            }
            index = next;
            previous_width = next_width;
            if next_width >= available {
                break;
            }
        }
        index
    }

    pub fn register_click(&mut self, now: std::time::Instant) -> bool {
        let double = self.last_click.is_some_and(|previous| {
            now.saturating_duration_since(previous) < std::time::Duration::from_millis(250)
        });
        self.last_click = Some(now);
        double
    }

    fn select_word(&mut self, index: usize) {
        let start = self.word_position(index, false);
        let end = self.word_position(index, true);
        self.move_caret(start, false);
        self.move_caret(end, true);
    }

    pub fn start_drag(&mut self, assets: &MenuAssets, width: i32, x: i32, shift: bool) {
        let double = self.register_click(std::time::Instant::now());
        self.click(assets, width, x, shift);
        if double {
            self.select_word(self.caret);
        }
        self.dragging = true;
    }

    pub fn drag(&mut self, assets: &MenuAssets, width: i32, x: i32) -> bool {
        if !self.dragging {
            return false;
        }
        self.click(assets, width, x, true);
        true
    }

    pub fn end_drag(&mut self) {
        self.dragging = false;
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
                let index = if control {
                    self.word_position(self.caret, false)
                } else {
                    self.previous(self.caret)
                };
                self.move_caret(index, shift);
            }
            KeyCode::ArrowRight => {
                let index = if control {
                    self.word_position(self.caret, true)
                } else {
                    self.next(self.caret)
                };
                self.move_caret(index, shift);
            }
            KeyCode::Backspace | KeyCode::Delete => {
                let (mut start, mut end) = self.selection();
                if start == end {
                    if key == KeyCode::Backspace {
                        start = if control {
                            self.word_position(start, false)
                        } else {
                            self.previous(start)
                        };
                    } else {
                        end = if control {
                            self.word_position(end, true)
                        } else {
                            self.next(end)
                        };
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

    #[test]
    fn word_edits_use_ascii_spaces_and_move_from_the_caret_with_selection() {
        let mut input = ChatInput::default();
        input.open(false);
        input.insert("한글  😀-two\u{a0}three  four");
        input.key(KeyCode::ArrowLeft, true, false);
        assert_eq!(&input.text[input.caret..], "four");
        input.key(KeyCode::ArrowLeft, true, true);
        assert_eq!(&input.text[input.caret..], "😀-two\u{a0}three  four");
        input.key(KeyCode::ArrowRight, false, false);
        assert_eq!(&input.text[input.caret..], "-two\u{a0}three  four");
        input.key(KeyCode::Backspace, true, false);
        assert_eq!(input.text, "한글  -two\u{a0}three  four");
        input.key(KeyCode::Delete, true, false);
        assert_eq!(input.text, "한글  four");
    }

    #[test]
    fn double_click_threshold_resets_with_chat_screen() {
        use std::time::{Duration, Instant};
        let mut input = ChatInput::default();
        let start = Instant::now();
        input.open(false);
        assert!(!input.register_click(start));
        assert!(input.register_click(start + Duration::from_millis(249)));
        assert!(!input.register_click(start + Duration::from_millis(499)));
        assert!(input.register_click(start + Duration::from_millis(500)));
        input.close();
        input.open(false);
        assert!(!input.register_click(start + Duration::from_millis(501)));
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn mouse_drag_extends_selection_and_release_stops_dragging() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = MenuAssets::load(&game, &[]).unwrap();
        let canvas = Canvas::new(1, 1);
        let mut input = ChatInput::default();
        input.open(false);
        input.insert("abc한글");
        input.start_drag(&assets, 320, 4, false);
        let end = 4 + canvas.text_width(Some(&assets), "abc");
        assert!(input.drag(&assets, 320, end));
        assert_eq!(input.selected_text(), "abc");
        input.end_drag();
        assert!(!input.drag(&assets, 320, 100));
        assert_eq!(input.selected_text(), "abc");
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn mouse_selection_uses_full_glyph_width_and_double_click_selects_word() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = MenuAssets::load(&game, &[]).unwrap();
        let canvas = Canvas::new(1, 1);
        let mut input = ChatInput::default();
        input.open(false);
        input.insert("alpha  beta 끝");
        let a_width = canvas.text_width(Some(&assets), "a");
        assert!(a_width > 1);
        input.click(&assets, 320, 4 + a_width - 1, false);
        assert_eq!(input.caret, 0);
        input.click(&assets, 320, 4 + a_width, false);
        assert_eq!(input.caret, 1);
        let inside_beta = 4 + canvas.text_width(Some(&assets), "alpha  b");
        input.start_drag(&assets, 320, inside_beta, false);
        input.end_drag();
        input.start_drag(&assets, 320, inside_beta, true);
        assert_eq!(input.selected_text(), "beta ");
        let end = 4 + canvas.text_width(Some(&assets), "alpha  beta 끝");
        assert!(input.drag(&assets, 320, end));
        assert_eq!(input.selected_text(), "beta 끝");
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 and Java 25 JDK"]
    fn navigation_and_deletion_match_java_edit_box() {
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
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("ChatEditOracle.java");
        let output = temp.path().join("output.json");
        std::fs::write(&source, include_str!("../tests/java/ChatEditOracle.java")).unwrap();
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
                .arg("ChatEditOracle")
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
        assert!(rows.len() > 5000);
        fn byte_offset(text: &str, utf16: usize) -> usize {
            let mut units = 0;
            for (byte, ch) in text.char_indices() {
                if units == utf16 {
                    return byte;
                }
                units += ch.len_utf16();
            }
            assert_eq!(units, utf16);
            text.len()
        }
        let mut clicks = 0;
        let mut time_arguments = 0;
        let mut marker_cases = 0;
        let mut clock_cases = 0;
        let mut identifier_cases = 0;
        let mut syntax_cases = 0;
        let mut clock_definitions = 0;
        let mut timeline_cases = 0;
        let mut rate_strings = 0;
        let mut weather_syntax = 0;
        for row in rows {
            if let Some(input) = row["weatherSyntax"].as_str() {
                weather_syntax += 1;
                assert_eq!(
                    native_session::weather_command::parse(input).is_some(),
                    row["weatherSyntaxAccepted"].as_bool().unwrap(),
                    "{row}"
                );
                continue;
            }
            if let Some(value) = row["rateValue"].as_str() {
                rate_strings += 1;
                assert_eq!(
                    crate::chat_hud::format_clock_rate(value.parse().unwrap()),
                    row["rateString"].as_str().unwrap(),
                    "{row}"
                );
                continue;
            }
            if let Some(period) = row["timelinePeriod"].as_i64() {
                timeline_cases += 1;
                let ticks = row["timelineTicks"].as_i64().unwrap();
                let period = (period != 0).then_some(period as i32);
                assert_eq!(
                    native_session::time_command::timeline_value(ticks, period, false),
                    row["timelineCurrent"].as_i64().unwrap(),
                    "{row}"
                );
                assert_eq!(
                    native_session::time_command::timeline_value(ticks, period, true),
                    row["timelineRepetitions"].as_i64().unwrap(),
                    "{row}"
                );
                continue;
            }
            if let Some(json) = row["worldClockJson"].as_str() {
                clock_definitions += 1;
                assert_eq!(
                    native_resources::world_clock::validate_clock_definition(json.as_bytes())
                        .is_ok(),
                    row["worldClockValid"].as_bool().unwrap(),
                    "{row}"
                );
                continue;
            }
            if let Some(input) = row["timeSyntax"].as_str() {
                syntax_cases += 1;
                let parsed = native_session::time_command::parse(input);
                // The Java parser also resolves resource arguments against its
                // registry; this fixture registers two timelines.
                let accepted = parsed
                    .as_ref()
                    .is_some_and(|command| match &command.action {
                        native_session::time_command::TimeAction::QueryTimeline {
                            timeline,
                            ..
                        } => {
                            (timeline.namespace == "example" && timeline.path == "timeline")
                                || (timeline.namespace == "minecraft"
                                    && timeline.path == "gametime")
                        }
                        _ => true,
                    });
                assert_eq!(
                    accepted,
                    row["timeSyntaxAccepted"].as_bool().unwrap(),
                    "{row}"
                );
                continue;
            }
            if let Some(input) = row["identifierArgument"].as_str() {
                identifier_cases += 1;
                let actual = native_session::time_command::parse_identifier(input)
                    .map(|id| format!("{}:{}", id.namespace, id.path));
                assert_eq!(actual.as_deref(), row["identifierResult"].as_str(), "{row}");
                continue;
            }
            if let Some(initial) = row["clockInitial"].as_i64() {
                clock_cases += 1;
                let mut clock = native_persistence::world::SavedClock {
                    total_ticks: initial,
                    partial_tick: 0.75,
                    ..Default::default()
                };
                let amount = row["clockAmount"].as_i64().unwrap() as i32;
                if row["clockAdd"].as_bool().unwrap() {
                    native_session::time_command::add_clock(&mut clock, amount);
                } else {
                    native_session::time_command::set_clock(&mut clock, amount);
                }
                assert_eq!(
                    clock.total_ticks,
                    row["clockTotal"].as_i64().unwrap(),
                    "{row}"
                );
                assert_eq!(
                    clock.partial_tick,
                    row["clockPartial"].as_f64().unwrap() as f32,
                    "{row}"
                );
                continue;
            }
            if let Some(ticks) = row["markerTicks"].as_i64() {
                marker_cases += 1;
                let period = row["markerPeriod"].as_i64().unwrap() as i32;
                let marker = native_resources::world_clock::TimeMarker {
                    ticks: ticks as i32,
                    period: (period != 0).then_some(period),
                };
                let current = row["markerCurrent"].as_i64().unwrap();
                assert_eq!(
                    native_session::time_command::resolve_marker(marker, current),
                    row["markerTarget"].as_i64(),
                    "{row}"
                );
                continue;
            }
            if let Some(input) = row["timeArgument"].as_str() {
                time_arguments += 1;
                let minimum = row["minimum"].as_i64().unwrap() as i32;
                let expected = row["ticks"].as_i64().map(|ticks| ticks as i32);
                assert_eq!(
                    native_session::time_command::duration(input, minimum),
                    expected,
                    "{row}"
                );
                continue;
            }
            let text = row["before"].as_str().unwrap();
            let mut input = ChatInput::default();
            input.open(false);
            input.text = text.to_owned();
            if let Some(x) = row["clickX"].as_u64() {
                clicks += 1;
                let index = input.click_position(0, x as i32, 256, |_| 1);
                input.move_caret(index, false);
                if row["double"].as_bool().unwrap() {
                    input.select_word(index);
                }
                assert_eq!(
                    input.text[..input.caret].encode_utf16().count(),
                    row["afterCursor"].as_u64().unwrap() as usize,
                    "{row}"
                );
                assert_eq!(
                    input.text[..input.anchor].encode_utf16().count(),
                    row["afterAnchor"].as_u64().unwrap() as usize,
                    "{row}"
                );
                continue;
            }
            input.caret = byte_offset(text, row["cursor"].as_u64().unwrap() as usize);
            input.anchor = byte_offset(text, row["anchor"].as_u64().unwrap() as usize);
            let code = match row["code"].as_i64().unwrap() {
                1073741904 => KeyCode::ArrowLeft,
                1073741903 => KeyCode::ArrowRight,
                8 => KeyCode::Backspace,
                127 => KeyCode::Delete,
                _ => unreachable!(),
            };
            let modifiers = row["modifiers"].as_u64().unwrap();
            input.key(code, modifiers & 64 != 0, modifiers & 1 != 0);
            assert_eq!(input.text, row["after"].as_str().unwrap(), "{row}");
            assert_eq!(
                input.text[..input.caret].encode_utf16().count(),
                row["afterCursor"].as_u64().unwrap() as usize,
                "{row}"
            );
            assert_eq!(
                input.text[..input.anchor].encode_utf16().count(),
                row["afterAnchor"].as_u64().unwrap() as usize,
                "{row}"
            );
        }
        assert_eq!(clicks, 98);
        assert!(time_arguments > 4000);
        assert_eq!(marker_cases, 420);
        assert_eq!(clock_cases, 72);
        assert_eq!(identifier_cases, 48);
        assert_eq!(syntax_cases, 27);
        assert_eq!(timeline_cases, 55);
        assert_eq!(rate_strings, 511);
        assert_eq!(weather_syntax, 16);
        assert_eq!(clock_definitions, 7);
    }
}
