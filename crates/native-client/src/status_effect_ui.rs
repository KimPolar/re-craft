//! Vanilla status effect layout for the HUD and player inventory.

use crate::menu_assets::{HudSprites, MenuAssets};
use crate::menu_draw::{Canvas, Rect};
use native_session::survival::DisplayEffect;
use std::cmp::Ordering;

pub struct EffectInfo {
    pub id: &'static str,
    color: i32,
    beneficial: bool,
}

// Values are the built-in 26.3 MobEffect colors and categories, not potion colors.
pub const EFFECTS: [EffectInfo; 19] = [
    EffectInfo {
        id: "night_vision",
        color: 12779366,
        beneficial: true,
    },
    EffectInfo {
        id: "fire_resistance",
        color: 16750848,
        beneficial: true,
    },
    EffectInfo {
        id: "jump_boost",
        color: 16646020,
        beneficial: true,
    },
    EffectInfo {
        id: "resistance",
        color: 9520880,
        beneficial: true,
    },
    EffectInfo {
        id: "slow_falling",
        color: 15978425,
        beneficial: true,
    },
    EffectInfo {
        id: "levitation",
        color: 13565951,
        beneficial: false,
    },
    EffectInfo {
        id: "speed",
        color: 3402751,
        beneficial: true,
    },
    EffectInfo {
        id: "slowness",
        color: 9154528,
        beneficial: false,
    },
    EffectInfo {
        id: "dolphins_grace",
        color: 8954814,
        beneficial: true,
    },
    EffectInfo {
        id: "haste",
        color: 14270531,
        beneficial: true,
    },
    EffectInfo {
        id: "mining_fatigue",
        color: 4866583,
        beneficial: false,
    },
    EffectInfo {
        id: "hunger",
        color: 5797459,
        beneficial: false,
    },
    EffectInfo {
        id: "poison",
        color: 8889187,
        beneficial: false,
    },
    EffectInfo {
        id: "regeneration",
        color: 13458603,
        beneficial: true,
    },
    EffectInfo {
        id: "luck",
        color: 5882118,
        beneficial: true,
    },
    EffectInfo {
        id: "unluck",
        color: 12624973,
        beneficial: false,
    },
    EffectInfo {
        id: "water_breathing",
        color: 10017472,
        beneficial: true,
    },
    EffectInfo {
        id: "conduit_power",
        color: 1950417,
        beneficial: true,
    },
    EffectInfo {
        id: "breath_of_the_nautilus",
        color: 65518,
        beneficial: true,
    },
];

fn compare(a: &DisplayEffect, b: &DisplayEffect) -> Ordering {
    let color = |effect: &DisplayEffect| {
        EFFECTS
            .iter()
            .find(|info| info.id == effect.id)
            .map_or(0, |info| info.color)
    };
    let common = a.ambient.cmp(&b.ambient);
    if (a.duration > 32147 && b.duration > 32147) || (a.ambient && b.ambient) {
        common.then_with(|| color(a).cmp(&color(b)))
    } else {
        common
            .then_with(|| (a.duration == -1).cmp(&(b.duration == -1)))
            .then_with(|| a.duration.cmp(&b.duration))
            .then_with(|| color(a).cmp(&color(b)))
    }
}

fn hud_alpha(effect: &DisplayEffect) -> u8 {
    if effect.ambient || effect.duration == -1 || effect.duration > 200 {
        return 255;
    }
    let duration = effect.duration;
    let fade = ((duration as f32 / 10.0 / 5.0) * 0.5).clamp(0.0, 0.5);
    let oscillations = 10 - duration / 20;
    let angle = duration as f32 * std::f32::consts::PI / 5.0;
    let cosine = native_session::physics::java_sin(f64::from(angle), true);
    let pulse = (oscillations as f32 / 10.0 * 0.25).clamp(0.0, 0.25);
    ((fade + cosine * pulse).clamp(0.0, 1.0) * 255.0).floor() as u8
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Entry {
    index: u8,
    duration_seconds: i32,
    amplifier: u8,
    ambient: bool,
    show_icon: bool,
    alpha: u8,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Frame {
    entries: [Option<Entry>; EFFECTS.len()],
}

impl Frame {
    pub fn new(effects: &[DisplayEffect]) -> Self {
        let mut sorted: Vec<_> = effects
            .iter()
            .filter(|effect| EFFECTS.iter().any(|info| info.id == effect.id))
            .copied()
            .collect();
        sorted.sort_by(compare);
        let mut frame = Self::default();
        for (slot, effect) in frame.entries.iter_mut().zip(sorted.iter()) {
            let index = EFFECTS
                .iter()
                .position(|info| info.id == effect.id)
                .unwrap() as u8;
            *slot = Some(Entry {
                index,
                duration_seconds: if effect.duration == -1 {
                    -1
                } else {
                    ((effect.duration as f32).floor() / 20.0).floor() as i32
                },
                amplifier: effect.amplifier,
                ambient: effect.ambient,
                show_icon: effect.show_icon,
                alpha: hud_alpha(effect),
            });
        }
        frame
    }

    pub fn hud_key(self) -> Self {
        let mut result = Self::default();
        for (target, mut entry) in result.entries.iter_mut().zip(
            self.entries
                .into_iter()
                .flatten()
                .filter(|entry| entry.show_icon),
        ) {
            entry.duration_seconds = 0;
            entry.amplifier = 0;
            *target = Some(entry);
        }
        result
    }

    pub fn inventory_key(mut self) -> Self {
        for entry in self.entries.iter_mut().flatten() {
            entry.alpha = 255;
            entry.show_icon = true;
        }
        self
    }
}

pub fn inventory_has_room(width: i32, left: i32) -> bool {
    width - (left + 176 + 2) >= 32
}

/// A conservative cursor region used only to invalidate the cached screen.
pub fn inventory_hover_area(
    frame: Frame,
    width: i32,
    left: i32,
    top: i32,
    cursor: (i32, i32),
) -> bool {
    let count = frame.entries.iter().flatten().count() as i32;
    if count == 0 || !inventory_has_room(width, left) {
        return false;
    }
    let spacing = if count > 5 { 132 / (count - 1) } else { 33 };
    let x = left + 178;
    cursor.0 >= x && cursor.0 <= width && cursor.1 >= top && cursor.1 <= top + count * spacing
}

pub struct InventoryTooltip {
    pub name: String,
    pub duration: String,
}

fn tooltip_requested(
    x: i32,
    y: i32,
    width: i32,
    spacing: i32,
    cursor: (i32, i32),
    clipped: bool,
) -> bool {
    clipped && cursor.0 >= x && cursor.0 <= x + width && cursor.1 >= y && cursor.1 <= y + spacing
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Placement {
    entry: Entry,
    x: i32,
    y: i32,
}

fn hud_placements(frame: Frame, width: i32) -> Vec<Placement> {
    let mut good = 0;
    let mut bad = 0;
    let mut placements = Vec::new();
    for entry in frame
        .entries
        .iter()
        .rev()
        .flatten()
        .filter(|entry| entry.show_icon)
    {
        let info = &EFFECTS[entry.index as usize];
        let (count, y) = if info.beneficial {
            good += 1;
            (good, 1)
        } else {
            bad += 1;
            (bad, 27)
        };
        let x = width - 25 * count;
        placements.push(Placement {
            entry: *entry,
            x,
            y,
        });
    }
    placements
}

pub fn draw_hud(canvas: &mut Canvas, sprites: &HudSprites, frame: Frame) {
    for Placement { entry, x, y } in hud_placements(frame, canvas.width) {
        let background = if entry.ambient {
            &sprites.effect_background_ambient
        } else {
            &sprites.effect_background
        };
        canvas.gui_sprite(
            &background.image,
            Rect { x, y, w: 24, h: 24 },
            background.scaling,
        );
        canvas.image_alpha(
            &sprites.effect_icons[entry.index as usize].image,
            Rect {
                x: x + 3,
                y: y + 3,
                w: 18,
                h: 18,
            },
            entry.alpha,
        );
    }
}

pub fn draw_inventory(
    canvas: &mut Canvas,
    assets: &MenuAssets,
    frame: Frame,
    left: i32,
    top: i32,
    cursor: Option<(i32, i32)>,
) -> Option<InventoryTooltip> {
    let x = left + 178;
    let room = canvas.width - x;
    if room < 32 {
        return None;
    }
    let max_width = if room >= 120 { room - 7 } else { 32 };
    let count = frame.entries.iter().flatten().count() as i32;
    if count == 0 {
        return None;
    }
    let spacing = if count > 5 { 132 / (count - 1) } else { 33 };
    let mut tooltip = None;
    for (i, entry) in frame.entries.iter().flatten().enumerate() {
        let info = &EFFECTS[entry.index as usize];
        let key = format!("effect.minecraft.{}", info.id);
        let mut name = assets.translate(&key, &key).to_owned();
        if (1..=9).contains(&entry.amplifier) {
            let level = u16::from(entry.amplifier) + 1;
            let key = format!("enchantment.level.{level}");
            name.push(' ');
            name.push_str(assets.translate(&key, &key));
        }
        let duration = if entry.duration_seconds == -1 {
            assets.translate("effect.duration.infinite", "∞").to_owned()
        } else {
            let secs = entry.duration_seconds.max(0);
            if secs >= 3600 {
                format!("{:02}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
            } else {
                format!("{:02}:{:02}", secs / 60, secs % 60)
            }
        };
        let width = max_width.min(
            39 + canvas
                .text_width(Some(assets), &name)
                .max(canvas.text_width(Some(assets), &duration)),
        );
        let y = top + i as i32 * spacing;
        let bg = if entry.ambient {
            &assets.hud.inventory_effect_background_ambient
        } else {
            &assets.hud.inventory_effect_background
        };
        canvas.gui_sprite(
            &bg.image,
            Rect {
                x,
                y,
                w: width,
                h: 32,
            },
            bg.scaling,
        );
        canvas.image(
            &assets.hud.effect_icons[entry.index as usize].image,
            Rect {
                x: x + 7,
                y: y + 7,
                w: 18,
                h: 18,
            },
        );
        let available = width - 39;
        let clipped = available <= 0 || canvas.text_width(Some(assets), &name) > available;
        if tooltip.is_none()
            && cursor.is_some_and(|cursor| tooltip_requested(x, y, width, spacing, cursor, clipped))
        {
            tooltip = Some(InventoryTooltip {
                name: name.clone(),
                duration: duration.clone(),
            });
        }
        if available > 0 {
            if clipped {
                let ellipsis = "...";
                let prefix_width = available - canvas.text_width(Some(assets), ellipsis);
                let prefix = canvas.text_prefix_by_width(Some(assets), &name, prefix_width);
                name = format!("{prefix}{ellipsis}");
            }
            canvas.text(Some(assets), x + 32, y + 7, &name, [255, 255, 255]);
            canvas.text(Some(assets), x + 32, y + 16, &duration, [128, 128, 128]);
        }
    }
    tooltip
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_room_and_hud_cache_follow_visible_changes() {
        assert!(!inventory_has_room(280, 88));
        assert!(inventory_has_room(300, 88));
        let make = |duration| DisplayEffect {
            id: "speed",
            duration,
            amplifier: 0,
            ambient: false,
            show_icon: true,
        };
        assert_eq!(
            Frame::new(&[make(3600)]).hud_key(),
            Frame::new(&[make(3599)]).hud_key()
        );
        assert_ne!(Frame::new(&[make(3600)]), Frame::new(&[make(3599)]));
        assert_ne!(
            Frame::new(&[make(10)]).hud_key(),
            Frame::new(&[make(20)]).hud_key()
        );
        assert_eq!(
            Frame::new(&[DisplayEffect {
                show_icon: false,
                ..make(10)
            }])
            .hud_key(),
            Frame::default()
        );
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn clipped_inventory_effect_returns_localized_name_and_duration() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = MenuAssets::load(&game, &[]).unwrap();
        let effect = DisplayEffect {
            id: "speed",
            duration: 3600,
            amplifier: 1,
            ambient: false,
            show_icon: false,
        };
        let frame = Frame::new(&[effect]);
        let mut narrow = Canvas::new(320, 240);
        let tooltip = draw_inventory(&mut narrow, &assets, frame, 72, 37, Some((250, 37))).unwrap();
        assert_eq!(
            tooltip.name,
            format!(
                "{} {}",
                assets.translate("effect.minecraft.speed", "effect.minecraft.speed"),
                assets.translate("enchantment.level.2", "enchantment.level.2")
            )
        );
        assert_eq!(tooltip.duration, "03:00");
        assert!(draw_inventory(&mut narrow, &assets, frame, 72, 37, Some((249, 37))).is_none());
        let mut wide = Canvas::new(500, 240);
        assert!(draw_inventory(&mut wide, &assets, frame, 162, 37, Some((340, 37))).is_none());
        let infinite = Frame::new(&[DisplayEffect {
            duration: -1,
            ..effect
        }]);
        let tooltip =
            draw_inventory(&mut narrow, &assets, infinite, 72, 37, Some((250, 37))).unwrap();
        assert_eq!(
            tooltip.duration,
            assets.translate("effect.duration.infinite", "∞")
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn inventory_tooltip_trigger_matches_java_extractor() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
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
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("EffectTooltipOracle.java");
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/EffectTooltipOracle.java"),
        )
        .unwrap();
        let mut cases = Vec::new();
        for width in [32, 39, 40, 50, 194, 195, 200] {
            for spacing in [5, 20, 33] {
                for mouse_x in [99, 100, 101, 132, 139, 150, 294, 295, 300, 301, 400] {
                    for mouse_y in [49, 50, 51, 60, 70, 80, 83, 84] {
                        cases.push(serde_json::json!({"x":100,"y":50,"width":width,"spacing":spacing,"mouse_x":mouse_x,"mouse_y":mouse_y}));
                    }
                }
            }
        }
        std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("EffectTooltipOracle")
                .arg(&input)
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let expected: Vec<bool> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(expected.len(), 1848);
        for (case, expected) in cases.iter().zip(expected) {
            let get = |name| case[name].as_i64().unwrap() as i32;
            let width = get("width");
            let clipped = width - 39 <= 0 || 156 > width - 39;
            assert_eq!(
                tooltip_requested(
                    get("x"),
                    get("y"),
                    width,
                    get("spacing"),
                    (get("mouse_x"), get("mouse_y")),
                    clipped
                ),
                expected,
                "{case}"
            );
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn hud_layout_and_fade_match_java_extractor() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
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
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("StatusHudOracle.java");
        let input = temp.path().join("cases.json");
        let output = temp.path().join("hud.json");
        std::fs::write(&source, include_str!("../tests/java/StatusHudOracle.java")).unwrap();
        let mut cases = Vec::<Vec<DisplayEffect>>::new();
        for info in &EFFECTS {
            for duration in [-1, 1, 19, 20, 99, 100, 199, 200, 201, 32148] {
                for ambient in [false, true] {
                    for show_icon in [false, true] {
                        cases.push(vec![DisplayEffect {
                            id: info.id,
                            duration,
                            amplifier: 0,
                            ambient,
                            show_icon,
                        }]);
                    }
                }
            }
        }
        for duration in 0..=200 {
            cases.push(vec![DisplayEffect {
                id: "speed",
                duration,
                amplifier: 0,
                ambient: false,
                show_icon: true,
            }]);
        }
        for offset in 0..EFFECTS.len() {
            cases.push(
                (0..8)
                    .map(|n| DisplayEffect {
                        id: EFFECTS[(offset + n * 5) % EFFECTS.len()].id,
                        duration: [-1, 10, 199, 200, 1000, 32148, 40000, 600][n],
                        amplifier: n as u8,
                        ambient: n % 3 == 0,
                        show_icon: n % 4 != 0,
                    })
                    .collect(),
            );
        }
        let input_rows: Vec<_> = cases
            .iter()
            .map(|effects| {
                serde_json::json!({
                    "width": 320, "demo": false,
                    "effects": effects.iter().map(|v| serde_json::json!({
                        "id":v.id,"duration":v.duration,"amplifier":v.amplifier,
                        "ambient":v.ambient,"icon":v.show_icon
                    })).collect::<Vec<_>>()
                })
            })
            .collect();
        std::fs::write(&input, serde_json::to_vec(&input_rows).unwrap()).unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("StatusHudOracle")
                .arg(&input)
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let expected: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(expected.len(), cases.len());
        for (case, expected) in cases.iter().zip(expected) {
            let mut actual = Vec::new();
            for placement in hud_placements(Frame::new(case), 320) {
                let entry = placement.entry;
                let background = if entry.ambient {
                    "hud/effect_background_ambient"
                } else {
                    "hud/effect_background"
                };
                actual.push(serde_json::json!([
                    background,
                    placement.x,
                    placement.y,
                    24,
                    24,
                    -1
                ]));
                let color = ((u32::from(entry.alpha) << 24) | 0xffffff) as i32;
                actual.push(serde_json::json!([
                    format!("mob_effect/{}", EFFECTS[entry.index as usize].id),
                    placement.x + 3,
                    placement.y + 3,
                    18,
                    18,
                    color
                ]));
            }
            assert_eq!(serde_json::Value::Array(actual), expected, "{case:?}");
        }
    }
}
