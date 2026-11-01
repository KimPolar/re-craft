//! Rendering-oriented interpretation of Minecraft's saved text components.

use native_persistence::nbt::Tag;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub color: Option<[u8; 3]>,
    pub bold: bool,
    pub italic: bool,
    pub underlined: bool,
    pub strikethrough: bool,
    pub obfuscated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

fn color(name: &str) -> Option<[u8; 3]> {
    let rgb = match name {
        "black" => 0x000000,
        "dark_blue" => 0x0000aa,
        "dark_green" => 0x00aa00,
        "dark_aqua" => 0x00aaaa,
        "dark_red" => 0xaa0000,
        "dark_purple" => 0xaa00aa,
        "gold" => 0xffaa00,
        "gray" => 0xaaaaaa,
        "dark_gray" => 0x555555,
        "blue" => 0x5555ff,
        "green" => 0x55ff55,
        "aqua" => 0x55ffff,
        "red" => 0xff5555,
        "light_purple" => 0xff55ff,
        "yellow" => 0xffff55,
        "white" => 0xffffff,
        _ => {
            let hex = name.strip_prefix('#')?;
            if hex.len() != 6 {
                return None;
            }
            u32::from_str_radix(hex, 16).ok()?
        }
    };
    Some([(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8])
}

fn apply_style(tag: &Tag, mut style: Style) -> Option<Style> {
    if let Some(value) = tag.get("color") {
        let Tag::String(value) = value else {
            return None;
        };
        style.color = Some(color(&String::from_utf16(&value.0).ok()?)?);
    }
    for (name, target) in [
        ("bold", &mut style.bold),
        ("italic", &mut style.italic),
        ("underlined", &mut style.underlined),
        ("strikethrough", &mut style.strikethrough),
        ("obfuscated", &mut style.obfuscated),
    ] {
        if let Some(value) = tag.get(name) {
            *target = match value {
                Tag::Byte(0) => false,
                Tag::Byte(1) => true,
                _ => return None,
            };
        }
    }
    Some(style)
}

fn append(out: &mut Vec<Span>, text: &str, style: Style) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = out.last_mut()
        && last.style == style
    {
        last.text.push_str(text);
    } else {
        out.push(Span {
            text: text.into(),
            style,
        });
    }
}

/// Equivalent to Java's StringDecomposer on one styled text fragment.
fn append_formatted(out: &mut Vec<Span>, input: &[u16], base: Style) {
    const COLORS: [[u8; 3]; 16] = [
        [0, 0, 0],
        [0, 0, 170],
        [0, 170, 0],
        [0, 170, 170],
        [170, 0, 0],
        [170, 0, 170],
        [255, 170, 0],
        [170, 170, 170],
        [85, 85, 85],
        [85, 85, 255],
        [85, 255, 85],
        [85, 255, 255],
        [255, 85, 85],
        [255, 85, 255],
        [255, 255, 85],
        [255, 255, 255],
    ];
    let mut style = base;
    let mut buffer = String::new();
    let mut index = 0;
    while index < input.len() {
        let code = input[index];
        if code == 0x00a7 {
            append(out, &buffer, style);
            buffer.clear();
            index += 1;
            let Some(&next) = input.get(index) else { break };
            let lower = if (b'A' as u16..=b'Z' as u16).contains(&next) {
                next + 32
            } else {
                next
            };
            let color_index = match lower {
                v if (b'0' as u16..=b'9' as u16).contains(&v) => Some((v - b'0' as u16) as usize),
                v if (b'a' as u16..=b'f' as u16).contains(&v) => {
                    Some((v - b'a' as u16 + 10) as usize)
                }
                _ => None,
            };
            if let Some(color_index) = color_index {
                style.color = Some(COLORS[color_index]);
                style.bold = false;
                style.italic = false;
                style.underlined = false;
                style.strikethrough = false;
                style.obfuscated = false;
            } else {
                match lower {
                    0x6b => style.obfuscated = true,
                    0x6c => style.bold = true,
                    0x6d => style.strikethrough = true,
                    0x6e => style.underlined = true,
                    0x6f => style.italic = true,
                    0x72 => style = base,
                    _ => {}
                }
            }
        } else if (0xd800..=0xdbff).contains(&code) {
            if let Some(&low) = input.get(index + 1)
                && (0xdc00..=0xdfff).contains(&low)
            {
                let scalar =
                    0x10000 + ((u32::from(code) - 0xd800) << 10) + (u32::from(low) - 0xdc00);
                buffer.push(char::from_u32(scalar).expect("valid surrogate pair"));
                index += 1;
            } else {
                buffer.push('\u{fffd}');
            }
        } else {
            buffer.push(char::from_u32(u32::from(code)).unwrap_or('\u{fffd}'));
        }
        index += 1;
    }
    append(out, &buffer, style);
}

fn append_formatted_str(out: &mut Vec<Span>, text: &str, style: Style) {
    append_formatted(out, &text.encode_utf16().collect::<Vec<_>>(), style);
}

fn unwrap(tag: &Tag) -> &Tag {
    if let Tag::Compound(fields) = tag
        && fields.len() == 1
        && fields[0].0.matches("")
    {
        &fields[0].1
    } else {
        tag
    }
}

/// Resolve literals, translation arguments, arrays and inherited styles. A
/// failed or unsupported component returns None so the caller can fall back.
pub fn resolve(tag: &Tag, translate: impl Fn(&str, &str) -> String) -> Option<Vec<Span>> {
    fn visit(
        tag: &Tag,
        parent: Style,
        translate: &impl Fn(&str, &str) -> String,
        out: &mut Vec<Span>,
        depth: usize,
    ) -> Option<()> {
        if depth > 64 || out.len() > 4096 {
            return None;
        }
        match unwrap(tag) {
            Tag::String(value) => append_formatted(out, &value.0, parent),
            Tag::Int(value) => append(out, &value.to_string(), parent),
            Tag::Long(value) => append(out, &value.to_string(), parent),
            Tag::Float(value) if value.is_finite() => append(out, &value.to_string(), parent),
            Tag::Double(value) if value.is_finite() => append(out, &value.to_string(), parent),
            Tag::List { values, .. } => {
                let (first, rest) = values.split_first()?;
                visit(first, parent, translate, out, depth + 1)?;
                let inherited = match unwrap(first) {
                    Tag::Compound(_) => apply_style(unwrap(first), parent)?,
                    _ => parent,
                };
                for value in rest {
                    visit(value, inherited, translate, out, depth + 1)?;
                }
            }
            tag @ Tag::Compound(_) => {
                let style = apply_style(tag, parent)?;
                if let Some(Tag::String(value)) = tag.get("text") {
                    append_formatted(out, &value.0, style);
                } else if let Some(Tag::String(key)) = tag.get("translate") {
                    let key = String::from_utf16(&key.0).ok()?;
                    let fallback = match tag.get("fallback") {
                        Some(Tag::String(value)) => String::from_utf16(&value.0).ok()?,
                        None => key.clone(),
                        _ => return None,
                    };
                    let template = translate(&key, &fallback);
                    let args = match tag.get("with") {
                        Some(Tag::List { values, .. }) => values.as_slice(),
                        None => &[],
                        _ => return None,
                    };
                    let mut fragments = Vec::new();
                    if let Some(parts) = format_parts(&template) {
                        let mut valid = true;
                        for part in parts {
                            match part {
                                Part::Text(text) => {
                                    append_formatted_str(&mut fragments, &text, style)
                                }
                                Part::Argument(index) => {
                                    let Some(arg) = args.get(index) else {
                                        valid = false;
                                        break;
                                    };
                                    visit(arg, style, translate, &mut fragments, depth + 1)?;
                                }
                            }
                        }
                        if !valid {
                            fragments.clear();
                            append_formatted_str(&mut fragments, &template, style);
                        }
                    } else {
                        append_formatted_str(&mut fragments, &template, style);
                    }
                    for fragment in fragments {
                        append(out, &fragment.text, fragment.style);
                    }
                } else {
                    return None;
                }
                if let Some(extra) = tag.get("extra") {
                    let Tag::List { values, .. } = extra else {
                        return None;
                    };
                    for child in values {
                        visit(child, style, translate, out, depth + 1)?;
                    }
                }
            }
            _ => return None,
        }
        if out.iter().map(|s| s.text.len()).sum::<usize>() > 65_536 {
            return None;
        }
        Some(())
    }
    let mut spans = Vec::new();
    visit(tag, Style::default(), &translate, &mut spans, 0)?;
    Some(spans)
}

pub(crate) enum Part {
    Text(String),
    Argument(usize),
}
pub(crate) fn format_parts(template: &str) -> Option<Vec<Part>> {
    let bytes = template.as_bytes();
    let mut at = 0;
    let mut start = 0;
    let mut sequential = 0;
    let mut parts = Vec::new();
    while at < bytes.len() {
        if bytes[at] != b'%' {
            at += 1;
            continue;
        }
        if start < at {
            parts.push(Part::Text(template[start..at].into()));
        }
        at += 1;
        if bytes.get(at) == Some(&b'%') {
            parts.push(Part::Text("%".into()));
            at += 1;
            start = at;
            continue;
        }
        let digits = at;
        while bytes.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        let index = if at != digits {
            let number = template[digits..at].parse::<usize>().ok()?.checked_sub(1)?;
            if bytes.get(at) != Some(&b'$') {
                return None;
            }
            at += 1;
            number
        } else {
            let index = sequential;
            sequential += 1;
            index
        };
        if bytes.get(at) != Some(&b's') {
            return None;
        }
        at += 1;
        parts.push(Part::Argument(index));
        start = at;
    }
    if start < template.len() {
        parts.push(Part::Text(template[start..].into()));
    }
    Some(parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_persistence::nbt::TagType;

    #[test]
    fn inherited_style_and_translated_arguments_are_preserved() {
        let component = Tag::Compound(vec![
            ("translate".into(), Tag::String("example".into())),
            ("color".into(), Tag::String("gold".into())),
            (
                "with".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![Tag::Compound(vec![
                        ("text".into(), Tag::String("one".into())),
                        ("bold".into(), Tag::Byte(1)),
                    ])],
                },
            ),
            (
                "extra".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![Tag::Compound(vec![
                        ("text".into(), Tag::String("!".into())),
                        ("color".into(), Tag::String("red".into())),
                    ])],
                },
            ),
        ]);
        let spans = resolve(&component, |_, _| "Hello %s".into()).unwrap();
        assert_eq!(
            spans.iter().map(|s| s.text.as_str()).collect::<String>(),
            "Hello one!"
        );
        assert_eq!(spans[0].style.color, Some([255, 170, 0]));
        assert!(spans[1].style.bold);
        assert_eq!(spans[2].style.color, Some([255, 85, 85]));
    }

    #[test]
    fn legacy_codes_reset_to_component_style_and_drop_unknown_codes() {
        let component = Tag::Compound(vec![
            ("text".into(), Tag::String("§lBold§cRed§rBase§Z?§".into())),
            ("color".into(), Tag::String("gold".into())),
            ("italic".into(), Tag::Byte(1)),
        ]);
        let spans = resolve(&component, |_, fallback| fallback.into()).unwrap();
        assert_eq!(
            spans.iter().map(|s| s.text.as_str()).collect::<String>(),
            "BoldRedBase?"
        );
        assert!(spans[0].style.bold && spans[0].style.italic);
        assert_eq!(spans[1].style.color, Some([255, 85, 85]));
        assert!(!spans[1].style.bold && !spans[1].style.italic);
        assert_eq!(spans[2].style.color, Some([255, 170, 0]));
        assert!(spans[2].style.italic);
    }

    #[test]
    fn malformed_surrogates_and_format_codes_match_java_replacement() {
        let component = Tag::String(native_persistence::nbt::JavaString(vec![
            b'a' as u16,
            0xd800,
            b'b' as u16,
            0x00a7,
            0xd83d,
            0xde00,
        ]));
        let spans = resolve(&component, |_, fallback| fallback.into()).unwrap();
        assert_eq!(
            spans.iter().map(|s| s.text.as_str()).collect::<String>(),
            "a�b�"
        );
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn title_components_match_java_text_and_styles() {
        use serde_json::Value;
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("TitleComponentOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/TitleComponentOracle.java"),
        )
        .unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
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
        let cp = std::env::join_paths(paths).unwrap();
        let run = |command: &mut Command| {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "Java oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("TitleComponentOracle")
            .arg(temp.path()));
        let rows: Vec<Value> =
            serde_json::from_slice(&std::fs::read(temp.path().join("expected.json")).unwrap())
                .unwrap();
        assert_eq!(rows.len(), 13);
        for (index, expected) in rows.iter().enumerate() {
            let nbt = native_persistence::nbt::read(
                &std::fs::read(temp.path().join(format!("{index}.nbt"))).unwrap(),
            )
            .unwrap();
            let spans = resolve(nbt.value.get("name").unwrap(), |key, fallback| match key {
                "example.order" => "%2$s / %1$s %%!".into(),
                "example.simple" => "Hello %s".into(),
                "example.codes" => "Before §a%s after".into(),
                _ => fallback.into(),
            })
            .unwrap_or_else(|| panic!("unsupported Java component {index}: {:?}", nbt.value));
            let chars: Vec<_> = spans
                .iter()
                .flat_map(|span| span.text.chars().map(move |ch| (ch, span.style)))
                .collect();
            let expected = expected.as_array().unwrap();
            assert_eq!(chars.len(), expected.len(), "case {index}");
            for ((ch, style), row) in chars.into_iter().zip(expected) {
                assert_eq!(
                    ch as u32,
                    row["cp"].as_u64().unwrap() as u32,
                    "case {index}"
                );
                let color = style.color.map_or(-1, |rgb| {
                    i64::from(u32::from(rgb[0]) << 16 | u32::from(rgb[1]) << 8 | u32::from(rgb[2]))
                });
                assert_eq!(color, row["color"].as_i64().unwrap(), "case {index}");
                for (actual, field) in [
                    (style.bold, "bold"),
                    (style.italic, "italic"),
                    (style.underlined, "underlined"),
                    (style.strikethrough, "strikethrough"),
                    (style.obfuscated, "obfuscated"),
                ] {
                    assert_eq!(
                        actual,
                        row[field].as_bool().unwrap(),
                        "case {index}: {field}"
                    );
                }
            }
        }
    }
}
