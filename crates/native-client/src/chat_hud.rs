//! The unfocused chat overlay for local system messages.

use native_session::{SystemMessage, SystemMessageKind};
use std::{ops::Range, sync::Arc};

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
    focused_height: f64,
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
            focused_height: 1.0,
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
                "chatHeightFocused" => result.focused_height = value,
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
    #[cfg(test)]
    fn new(settings: &ChatSettings, gui_height: i32, age: u64) -> Option<Self> {
        Self::for_mode(settings, gui_height, age, false)
    }

    fn for_mode(settings: &ChatSettings, gui_height: i32, age: u64, focused: bool) -> Option<Self> {
        if !settings.visible || settings.scale <= 0.0 || (!focused && age >= 200) {
            return None;
        }
        let factor = if focused {
            1.0
        } else {
            ((1.0 - age as f64 / 200.0) * 10.0).clamp(0.0, 1.0).powi(2) as f32
        };
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
        let height = if focused {
            settings.focused_height
        } else {
            settings.unfocused_height
        };
        let max_lines = ((height * 160.0 + 20.0).floor() as i32 / line_height) as usize;
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
struct WrappedLine {
    text: String,
    bold_prefix: usize,
    source_start: usize,
}

fn wrap(
    canvas: &Canvas,
    assets: &MenuAssets,
    text: &str,
    max_width: i32,
    bold_prefix: usize,
) -> Vec<WrappedLine> {
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut line_bold: usize = 0;
    let mut width = 0;
    let mut last_space = None;
    let mut line_start = 0;
    for (index, ch) in text.char_indices().take(16_384) {
        if ch == '\n' {
            lines.push(WrappedLine {
                text: std::mem::take(&mut line),
                bold_prefix: line_bold,
                source_start: line_start,
            });
            line_bold = 0;
            width = 0;
            last_space = None;
            line_start = index + ch.len_utf8();
            continue;
        }
        let bold = index < bold_prefix;
        let char_width =
            canvas.text_width(Some(assets), ch.encode_utf8(&mut [0; 4])) + i32::from(bold);
        let mut wrapped = false;
        if width + char_width > max_width && !line.is_empty() {
            wrapped = true;
            if let Some(space) = last_space {
                let remainder = line.split_off(space);
                line.pop();
                let remainder_bold = line_bold.saturating_sub(space);
                lines.push(WrappedLine {
                    text: std::mem::replace(&mut line, remainder),
                    bold_prefix: line_bold.min(space - 1),
                    source_start: line_start,
                });
                line_start += space;
                line_bold = remainder_bold;
                width = canvas.bold_text_width(Some(assets), &line[..line_bold])
                    + canvas.text_width(Some(assets), &line[line_bold..]);
                last_space = None;
            } else {
                lines.push(WrappedLine {
                    text: std::mem::take(&mut line),
                    bold_prefix: line_bold,
                    source_start: line_start,
                });
                line_start = index;
                line_bold = 0;
                width = 0;
            }
        }
        if wrapped && line.is_empty() && ch == ' ' {
            line_start = index + 1;
            continue;
        }
        if line.is_empty() {
            line_start = index;
        }
        if ch == ' ' {
            last_space = Some(line.len() + 1);
        }
        line.push(ch);
        if bold {
            line_bold += ch.len_utf8();
        }
        width += char_width;
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(WrappedLine {
            text: line,
            bold_prefix: line_bold,
            source_start: line_start,
        });
    }
    lines
}

struct ChatLine {
    text: String,
    layout: LineLayout,
    bold_prefix: usize,
    links: Vec<Range<usize>>,
    action: Option<ChatClick>,
    error: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatClick {
    Screenshot(Arc<std::path::PathBuf>),
    CopySeed(Arc<str>),
}

fn screenshot_message(template: &str, name: &str) -> (String, Vec<Range<usize>>) {
    use crate::text_component::{Part, format_parts};
    let Some(parts) = format_parts(template) else {
        return (template.to_owned(), Vec::new());
    };
    let mut message = String::new();
    let mut links = Vec::new();
    for part in parts {
        match part {
            Part::Text(text) => message.push_str(&text),
            Part::Argument(0) => {
                let start = message.len();
                message.push_str(name);
                if start != message.len() {
                    links.push(start..message.len());
                }
            }
            Part::Argument(_) => return (template.to_owned(), Vec::new()),
        }
    }
    (message, links)
}

fn format_translation(template: &str, arguments: &[&str]) -> String {
    use crate::text_component::{Part, format_parts};
    let Some(parts) = format_parts(template) else {
        return template.to_owned();
    };
    let mut result = String::new();
    for part in parts {
        match part {
            Part::Text(text) => result.push_str(&text),
            Part::Argument(index) => match arguments.get(index) {
                Some(argument) => result.push_str(argument),
                None => return template.to_owned(),
            },
        }
    }
    result
}

pub(crate) fn format_clock_rate(rate: f32) -> String {
    if rate < 0.001 {
        let value = format!("{rate:e}");
        let (mantissa, exponent) = value.split_once('e').expect("scientific format");
        let mantissa = if mantissa.contains('.') {
            mantissa.to_owned()
        } else {
            format!("{mantissa}.0")
        };
        format!("{mantissa}E{exponent}")
    } else {
        format!("{rate:?}")
    }
}

fn prepare_lines(
    canvas: &Canvas,
    assets: &MenuAssets,
    messages: &[SystemMessage],
    tick: u64,
    focused: bool,
    scroll: usize,
    height: i32,
) -> Option<(Vec<ChatLine>, usize)> {
    let base = LineLayout::for_mode(&assets.chat, height, 0, focused)?;
    let mut lines = Vec::new();
    for message in messages.iter().rev() {
        let Some(layout) = LineLayout::for_mode(
            &assets.chat,
            height,
            tick.saturating_sub(message.tick),
            focused,
        ) else {
            continue;
        };
        let error = matches!(
            &message.kind,
            SystemMessageKind::TimeAlreadySet { .. }
                | SystemMessageKind::TimeNoDefaultClock(_)
                | SystemMessageKind::TimePermissionDenied
                | SystemMessageKind::TimeSaveUnavailable
                | SystemMessageKind::TimeUnknownClock(_)
                | SystemMessageKind::TimeMarkerDataUnavailable
                | SystemMessageKind::TimeTimelineDataUnavailable
                | SystemMessageKind::TimeUnknownTimeline(_)
                | SystemMessageKind::TimeWrongTimeline { .. }
                | SystemMessageKind::TimeRate { already: true, .. }
                | SystemMessageKind::TimePaused { already: true, .. }
                | SystemMessageKind::TimeMarker { already: true, .. }
                | SystemMessageKind::TimeMarker { missing: true, .. }
        );
        let (text, debug_prefix, links, action) = match &message.kind {
            SystemMessageKind::MissingRespawnBlock => (
                assets
                    .translate(
                        "block.minecraft.spawn.not_valid",
                        "block.minecraft.spawn.not_valid",
                    )
                    .to_owned(),
                None,
                Vec::new(),
                None,
            ),
            SystemMessageKind::DebugFocusPause(enabled) => {
                let (key, fallback) = if *enabled {
                    ("debug.pause_focus.on", "Pause on lost focus: enabled")
                } else {
                    ("debug.pause_focus.off", "Pause on lost focus: disabled")
                };
                let prefix = assets.translate("debug.prefix", "[Debug]:");
                (
                    format!("{} {}", prefix, assets.translate(key, fallback)),
                    Some(prefix),
                    Vec::new(),
                    None,
                )
            }
            SystemMessageKind::ScreenshotSaved(path) => {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                let (text, links) = screenshot_message(
                    assets.translate("screenshot.success", "Saved screenshot as %s"),
                    &name,
                );
                (
                    text,
                    None,
                    links,
                    Some(ChatClick::Screenshot(Arc::clone(path))),
                )
            }
            SystemMessageKind::WorldSeed(seed) => {
                let seed: Arc<str> = seed.to_string().into();
                let (brackets, inner) =
                    screenshot_message(assets.translate("chat.square_brackets", "[%s]"), &seed);
                let (text, outer) = screenshot_message(
                    assets.translate("commands.seed.success", "Seed: %s"),
                    &brackets,
                );
                let links = outer
                    .into_iter()
                    .flat_map(|range| {
                        inner
                            .iter()
                            .map(move |part| (range.start + part.start)..(range.start + part.end))
                    })
                    .collect();
                (text, None, links, Some(ChatClick::CopySeed(seed)))
            }
            SystemMessageKind::TimeGame(ticks) => (
                format_translation(
                    assets.translate(
                        "commands.time.query.gametime",
                        "The game time is %s tick(s)",
                    ),
                    &[&ticks.to_string()],
                ),
                None,
                Vec::new(),
                None,
            ),
            SystemMessageKind::TimeClock { name, ticks } => (
                format_translation(
                    assets.translate("commands.time.query.absolute", "Clock %s is at %s tick(s)"),
                    &[name, &ticks.to_string()],
                ),
                None,
                Vec::new(),
                None,
            ),
            SystemMessageKind::TimeRate {
                name,
                rate_bits,
                already,
            } => {
                let (key, fallback) = if *already {
                    (
                        "commands.time.rate.already_same",
                        "Clock %s is already running at rate %s",
                    )
                } else {
                    ("commands.time.rate", "Set clock %s rate to %s")
                };
                let rate = format_clock_rate(f32::from_bits(*rate_bits));
                (
                    format_translation(assets.translate(key, fallback), &[name, &rate]),
                    None,
                    Vec::new(),
                    None,
                )
            }
            SystemMessageKind::TimeTimeline {
                name,
                ticks,
                repetitions,
            } => {
                let (key, fallback) = if *repetitions {
                    (
                        "commands.time.query.timeline.repetitions",
                        "Timeline %s has repeated %s time(s)",
                    )
                } else {
                    (
                        "commands.time.query.timeline",
                        "Timeline %s is at %s tick(s)",
                    )
                };
                (
                    format_translation(
                        assets.translate(key, fallback),
                        &[name, &ticks.to_string()],
                    ),
                    None,
                    Vec::new(),
                    None,
                )
            }
            SystemMessageKind::TimeUnknownTimeline(name) => (
                format_translation(
                    assets.translate(
                        "argument.resource.not_found",
                        "Can't find element '%s' of type '%s'",
                    ),
                    &[name, "minecraft:timeline"],
                ),
                None,
                Vec::new(),
                None,
            ),
            SystemMessageKind::TimeWrongTimeline { clock, timeline } => (
                format_translation(
                    assets.translate(
                        "commands.time.wrong_timeline_for_clock",
                        "Timeline %s does not use clock %s",
                    ),
                    &[timeline, clock],
                ),
                None,
                Vec::new(),
                None,
            ),
            SystemMessageKind::TimeTimelineDataUnavailable => (
                "Cannot query timelines because the active timeline data could not be verified."
                    .to_owned(),
                None,
                Vec::new(),
                None,
            ),
            SystemMessageKind::TimeSet { name, ticks } => (
                format_translation(
                    assets.translate("commands.time.set.absolute", "Set %s to %s tick(s)"),
                    &[name, &ticks.to_string()],
                ),
                None,
                Vec::new(),
                None,
            ),
            SystemMessageKind::TimeAlreadySet { name, ticks } => (
                format_translation(
                    assets.translate(
                        "commands.time.set.already_at_time",
                        "Clock %s is already set to %s tick(s)",
                    ),
                    &[name, &ticks.to_string()],
                ),
                None,
                Vec::new(),
                None,
            ),
            SystemMessageKind::TimePaused {
                name,
                paused,
                already,
            } => {
                let (key, fallback) = match (paused, already) {
                    (true, false) => ("commands.time.pause", "Paused clock %s"),
                    (false, false) => ("commands.time.resume", "Resumed clock %s"),
                    (true, true) => (
                        "commands.time.pause.already_paused",
                        "Clock %s is already paused",
                    ),
                    (false, true) => (
                        "commands.time.pause.already_running",
                        "Clock %s is already running",
                    ),
                };
                (
                    format_translation(assets.translate(key, fallback), &[name]),
                    None,
                    Vec::new(),
                    None,
                )
            }
            SystemMessageKind::TimeMarker {
                name,
                marker,
                already,
                missing,
            } => {
                let (key, fallback, arguments): (&str, &str, Vec<&str>) = if *missing {
                    (
                        "commands.time.no_time_marker_found",
                        "Time marker %s does not exist for clock %s",
                        vec![marker, name],
                    )
                } else if *already {
                    (
                        "commands.time.set.already_at_time_marker",
                        "Clock %s is already at time marker %s",
                        vec![name, marker],
                    )
                } else {
                    (
                        "commands.time.set.time_marker",
                        "Set %s to time marker %s",
                        vec![name, marker],
                    )
                };
                (
                    format_translation(assets.translate(key, fallback), &arguments),
                    None,
                    Vec::new(),
                    None,
                )
            }
            SystemMessageKind::TimePermissionDenied => (
                assets
                    .translate(
                        "command.unknown.command",
                        "Unknown or incomplete command. See below for error",
                    )
                    .to_owned(),
                None,
                Vec::new(),
                None,
            ),
            SystemMessageKind::TimeSaveUnavailable => (
                "Cannot change the clock because its saved data is unavailable.".to_owned(),
                None,
                Vec::new(),
                None,
            ),
            SystemMessageKind::TimeUnknownClock(name) => (
                format_translation(
                    assets.translate(
                        "argument.resource.not_found",
                        "Can't find element '%s' of type '%s'",
                    ),
                    &[name, "minecraft:world_clock"],
                ),
                None,
                Vec::new(),
                None,
            ),
            SystemMessageKind::TimeMarkerDataUnavailable => (
                "Cannot use time markers because the active timeline data could not be verified."
                    .to_owned(),
                None,
                Vec::new(),
                None,
            ),
            SystemMessageKind::TimeNoDefaultClock(dimension) => (
                format_translation(
                    assets.translate(
                        "commands.time.no_default_clock",
                        "There is no default clock in dimension %s",
                    ),
                    &[dimension],
                ),
                None,
                Vec::new(),
                None,
            ),
            SystemMessageKind::ScreenshotFailed(reason) => (
                assets
                    .translate("screenshot.failure", "Couldn't save screenshot: %s")
                    .replacen("%s", reason, 1),
                None,
                Vec::new(),
                None,
            ),
            SystemMessageKind::LocalChat(text) => (
                assets
                    .translate("chat.type.text", "<%s> %s")
                    .replacen("%s", "Player", 1)
                    .replacen("%s", text, 1),
                None,
                Vec::new(),
                None,
            ),
            SystemMessageKind::CommandUnavailable => (
                "Commands are not available yet.".to_owned(),
                None,
                Vec::new(),
                None,
            ),
        };
        let prefix_bytes = debug_prefix.map_or(0, str::len);
        for line in wrap(canvas, assets, &text, layout.width.max(1), prefix_bytes)
            .into_iter()
            .rev()
        {
            if !focused && lines.len() >= base.max_lines {
                break;
            }
            let end = line.source_start + line.text.len();
            let visible_links = links
                .iter()
                .filter_map(|link| {
                    let start = link.start.max(line.source_start);
                    let finish = link.end.min(end);
                    (start < finish)
                        .then(|| (start - line.source_start)..(finish - line.source_start))
                })
                .collect();
            lines.push(ChatLine {
                text: line.text,
                layout,
                bold_prefix: line.bold_prefix,
                links: visible_links,
                action: action.clone(),
                error,
            });
        }
        if !focused && lines.len() >= base.max_lines {
            break;
        }
    }
    let max_scroll = lines.len().saturating_sub(base.max_lines);
    if focused {
        lines = lines
            .into_iter()
            .skip(scroll.min(max_scroll))
            .take(base.max_lines)
            .collect();
    }
    Some((lines, max_scroll))
}

fn position(line: &ChatLine, index: usize) -> (i32, i32) {
    let bottom = line.layout.bottom - index as i32 * line.layout.line_height;
    (
        (4.0 * line.layout.scale).floor() as i32,
        ((bottom - line.layout.text_offset) as f32 * line.layout.scale).floor() as i32,
    )
}

fn prefix_width(canvas: &Canvas, assets: &MenuAssets, line: &ChatLine, end: usize) -> i32 {
    let bold_end = end.min(line.bold_prefix);
    canvas.bold_text_width(Some(assets), &line.text[..bold_end])
        + canvas.text_width(Some(assets), &line.text[bold_end..end])
}

pub fn interaction_at(
    assets: &MenuAssets,
    messages: &[SystemMessage],
    height: i32,
    scroll: usize,
    cursor: (i32, i32),
) -> Option<ChatClick> {
    let measure = Canvas::new(1, 1);
    let (lines, _) = prepare_lines(&measure, assets, messages, 0, true, scroll, height)?;
    for (index, line) in lines.iter().enumerate() {
        let (x, y) = position(line, index);
        for range in &line.links {
            let start = (prefix_width(&measure, assets, line, range.start) as f32
                * line.layout.scale)
                .floor() as i32;
            let end = (prefix_width(&measure, assets, line, range.end) as f32 * line.layout.scale)
                .ceil() as i32;
            let height = (9.0 * line.layout.scale).ceil() as i32;
            if (x + start..x + end).contains(&cursor.0) && (y..y + height).contains(&cursor.1) {
                return line.action.clone();
            }
        }
    }
    None
}

pub fn draw(canvas: &mut Canvas, assets: &MenuAssets, messages: &[SystemMessage], tick: u64) {
    draw_mode(canvas, assets, messages, tick, false, 0);
}

pub fn draw_focused(
    canvas: &mut Canvas,
    assets: &MenuAssets,
    messages: &[SystemMessage],
    tick: u64,
    scroll: usize,
) -> usize {
    draw_mode(canvas, assets, messages, tick, true, scroll)
}

pub fn focused_page_lines(settings: &ChatSettings, height: i32) -> i32 {
    LineLayout::for_mode(settings, height, 0, true)
        .map_or(0, |layout| layout.max_lines.saturating_sub(1) as i32)
}

fn draw_mode(
    canvas: &mut Canvas,
    assets: &MenuAssets,
    messages: &[SystemMessage],
    tick: u64,
    focused: bool,
    scroll: usize,
) -> usize {
    let Some((lines, max_scroll)) = prepare_lines(
        canvas,
        assets,
        messages,
        tick,
        focused,
        scroll,
        canvas.height,
    ) else {
        return 0;
    };
    for (index, line) in lines.iter().enumerate() {
        let layout = line.layout;
        let bottom = layout.bottom - index as i32 * layout.line_height;
        canvas.fill(
            layout.rect(-4, bottom - layout.line_height, layout.width + 8, bottom),
            [0, 0, 0, layout.background_alpha],
        );
    }
    for (index, line) in lines.iter().enumerate() {
        let layout = line.layout;
        let text = &line.text;
        let prefix_len = line.bold_prefix;
        let bottom = layout.bottom - index as i32 * layout.line_height;
        let opacity = (layout.text_opacity * 255.0) as u8;
        canvas.fill(
            layout.rect(-4, bottom - layout.line_height, -2, bottom),
            [208, 208, 208, opacity],
        );
        let x = (4.0 * layout.scale).floor() as i32;
        let y = ((bottom - layout.text_offset) as f32 * layout.scale).floor() as i32;
        let draw_text = |canvas: &mut Canvas, x, y| {
            let (prefix, rest) = text.split_at(prefix_len);
            canvas.bold_text_alpha(Some(assets), x, y, prefix, [255, 255, 85], opacity);
            let offset = canvas.bold_text_width(Some(assets), prefix);
            if matches!(line.action, Some(ChatClick::CopySeed(_))) {
                let mut start = prefix_len;
                for range in &line.links {
                    canvas.text_alpha(
                        Some(assets),
                        x + prefix_width(canvas, assets, line, start),
                        y,
                        &text[start..range.start],
                        [255, 255, 255],
                        opacity,
                    );
                    canvas.text_alpha(
                        Some(assets),
                        x + prefix_width(canvas, assets, line, range.start),
                        y,
                        &text[range.clone()],
                        [85, 255, 85],
                        opacity,
                    );
                    start = range.end;
                }
                canvas.text_alpha(
                    Some(assets),
                    x + prefix_width(canvas, assets, line, start),
                    y,
                    &text[start..],
                    [255, 255, 255],
                    opacity,
                );
            } else {
                canvas.text_alpha(
                    Some(assets),
                    x + offset,
                    y,
                    rest,
                    if line.error {
                        [255, 85, 85]
                    } else {
                        [255, 255, 255]
                    },
                    opacity,
                );
            }
            for range in line
                .links
                .iter()
                .filter(|_| matches!(line.action, Some(ChatClick::Screenshot(_))))
            {
                let left = prefix_width(canvas, assets, line, range.start);
                let right = prefix_width(canvas, assets, line, range.end);
                canvas.fill(
                    Rect {
                        x: x + left,
                        y: y + 8,
                        w: right - left,
                        h: 1,
                    },
                    [255, 255, 255, opacity],
                );
            }
        };
        if layout.scale == 1.0 {
            draw_text(canvas, x, y);
        } else {
            let (prefix, rest) = text.split_at(prefix_len);
            let width = (canvas.bold_text_width(Some(assets), prefix)
                + canvas.text_width(Some(assets), rest))
            .clamp(1, 32768) as u32
                + 2;
            let mut source = Canvas::new(width, 10);
            draw_text(&mut source, 0, 0);
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
    max_scroll
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_feedback_translations_keep_argument_order_and_literals() {
        assert_eq!(
            format_translation("%2$s ticks: %1$s %%", &["example:clock", "123"]),
            "123 ticks: example:clock %"
        );
        assert_eq!(format_translation("%3$s", &["a", "b"]), "%3$s");
    }

    #[test]
    fn screenshot_translation_keeps_only_filename_clickable() {
        let (text, links) = screenshot_message("파일 %1$s 완료 %% %1$s", "한글.png");
        assert_eq!(text, "파일 한글.png 완료 % 한글.png");
        assert_eq!(links.len(), 2);
        for link in links {
            assert_eq!(&text[link], "한글.png");
        }
        assert_eq!(
            screenshot_message("invalid %2$s", "a.png"),
            ("invalid %2$s".into(), vec![])
        );
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn seed_links_survive_localization_wrapping_and_scaling() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut assets = MenuAssets::load(&game, &[]).unwrap();
        assets.chat = ChatSettings::default();
        assets.chat.width = 0.0;
        assets
            .language
            .insert("chat.square_brackets".into(), "[%1$s]".into());
        assets
            .language
            .insert("commands.seed.success".into(), "시드: %1$s".into());
        let messages = [SystemMessage {
            tick: 0,
            kind: SystemMessageKind::WorldSeed(i64::MIN),
        }];
        for scale in [0.5, 1.0] {
            assets.chat.scale = scale;
            let canvas = Canvas::new(1, 1);
            let (lines, _) = prepare_lines(&canvas, &assets, &messages, 0, true, 0, 240).unwrap();
            assert!(lines.len() > 1);
            for (index, line) in lines.iter().enumerate() {
                let (x, y) = position(line, index);
                for range in &line.links {
                    let left = (prefix_width(&canvas, &assets, line, range.start) as f32
                        * line.layout.scale)
                        .floor() as i32;
                    let right = (prefix_width(&canvas, &assets, line, range.end) as f32
                        * line.layout.scale)
                        .ceil() as i32;
                    if right > left {
                        assert_eq!(
                            interaction_at(&assets, &messages, 240, 0, (x + left, y)),
                            Some(ChatClick::CopySeed("-9223372036854775808".into()))
                        );
                    }
                }
            }
        }
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn wrapped_link_hit_testing_tracks_text_scale_and_scroll() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut assets = MenuAssets::load(&game, &[]).unwrap();
        assets.chat = ChatSettings::default();
        assets.chat.width = 0.0;
        assets.chat.focused_height = 0.0;
        assets
            .language
            .insert("screenshot.success".into(), "사진 %1$s 완료".into());
        let messages = [
            SystemMessage {
                tick: 0,
                kind: SystemMessageKind::ScreenshotSaved(
                    std::path::PathBuf::from("first-long-name.png").into(),
                ),
            },
            SystemMessage {
                tick: 1,
                kind: SystemMessageKind::LocalChat("new message".into()),
            },
            SystemMessage {
                tick: 2,
                kind: SystemMessageKind::ScreenshotSaved(
                    std::path::PathBuf::from("second-long-name.png").into(),
                ),
            },
        ];
        let measure = Canvas::new(1, 1);
        for scale in [0.5, 1.0] {
            assets.chat.scale = scale;
            let (all, max_scroll) =
                prepare_lines(&measure, &assets, &messages, 0, true, 0, 240).unwrap();
            assert!(max_scroll > 0);
            assert!(!all.is_empty());
            for scroll in 0..=max_scroll {
                let (lines, _) =
                    prepare_lines(&measure, &assets, &messages, 0, true, scroll, 240).unwrap();
                for (index, line) in lines.iter().enumerate() {
                    let (x, y) = position(line, index);
                    for range in &line.links {
                        let start = (prefix_width(&measure, &assets, line, range.start) as f32
                            * line.layout.scale)
                            .floor() as i32;
                        let end = (prefix_width(&measure, &assets, line, range.end) as f32
                            * line.layout.scale)
                            .ceil() as i32;
                        if end > start {
                            let hit =
                                interaction_at(&assets, &messages, 240, scroll, (x + start, y))
                                    .unwrap();
                            assert_eq!(Some(hit), line.action);
                        }
                    }
                    assert!(interaction_at(&assets, &messages, 240, scroll, (x - 1, y)).is_none());
                }
            }
            let wrapped = wrap(&measure, &assets, "한글  a b\n😀 long name", 25, 0);
            for line in wrapped {
                assert_eq!(
                    &"한글  a b\n😀 long name"
                        [line.source_start..line.source_start + line.text.len()],
                    line.text
                );
            }
        }
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn wrapped_debug_prefix_keeps_bold_width_and_unicode_boundaries() {
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = MenuAssets::load(&directory, &[]).unwrap();
        let canvas = Canvas::new(400, 100);
        let prefix = "[AB CD한]";
        let text = format!("{prefix} rest");
        let max_width = canvas.bold_text_width(Some(&assets), "[AB");
        let lines = wrap(&canvas, &assets, &text, max_width, prefix.len());
        assert!(lines.len() > 2);
        let mut bold = String::new();
        let mut regular = String::new();
        for line in lines {
            let (head, tail) = line.text.split_at(line.bold_prefix);
            assert!(
                canvas.bold_text_width(Some(&assets), head)
                    + canvas.text_width(Some(&assets), tail)
                    <= max_width
            );
            bold.push_str(head);
            regular.push_str(tail);
        }
        assert_eq!(bold, prefix.replace(' ', ""));
        assert_eq!(regular, "rest");
    }

    #[test]
    fn settings_follow_minecraft_options_and_ignore_invalid_values() {
        let settings = ChatSettings::from_options(
            "chatVisibility:1\nchatOpacity:0.2\nchatScale:0.5\nchatWidth:2\nchatLineSpacing:NaN\nchatHeightUnfocused:0.75\nchatHeightFocused:0.25\ntextBackgroundOpacity:0.25",
        );
        assert!(settings.visible);
        assert_eq!(settings.opacity, 0.2);
        assert_eq!(settings.scale, 0.5);
        assert_eq!(settings.width, 1.0);
        assert_eq!(settings.line_spacing, 0.0);
        assert_eq!(settings.unfocused_height, 0.75);
        assert_eq!(settings.focused_height, 0.25);
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
        let key = display_key(&settings, std::slice::from_ref(&message), 10).unwrap();
        assert_eq!(
            display_key(&settings, std::slice::from_ref(&message), 190),
            Some(key)
        );
        assert_ne!(
            display_key(&settings, std::slice::from_ref(&message), 191),
            Some(key)
        );
        assert_eq!(
            display_key(&settings, std::slice::from_ref(&message), 210),
            None
        );
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
        assert_eq!(rows.len(), 3 * 7 * 2 * 2 * 2 * 3 * 3 * 3);
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
            let focused = row["focused"].as_bool().unwrap();
            let layout = LineLayout::for_mode(&settings, height, age, focused);
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
