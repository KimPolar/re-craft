//! The default 26.3 tooltip positioner and sprite background.

use crate::menu_assets::MenuAssets;
use crate::menu_draw::{Canvas, Rect};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Layout {
    text: (i32, i32),
    background: Rect,
}

fn layout(screen: (i32, i32), cursor: (i32, i32), content: (i32, i32)) -> Layout {
    let (mut x, mut y) = (cursor.0 + 12, cursor.1 - 12);
    if x + content.0 > screen.0 {
        x = (x - 24 - content.0).max(4);
    }
    if y + content.1 + 3 > screen.1 {
        y = screen.1 - content.1 - 3;
    }
    Layout {
        text: (x, y),
        background: Rect {
            x: x - 12,
            y: y - 12,
            w: content.0 + 24,
            h: content.1 + 24,
        },
    }
}

pub fn draw_two_lines(
    canvas: &mut Canvas,
    assets: &MenuAssets,
    cursor: (i32, i32),
    first: &str,
    second: &str,
) {
    let width = canvas
        .text_width(Some(assets), first)
        .max(canvas.text_width(Some(assets), second));
    // Two ClientTextTooltip lines are 10 pixels each; this tooltip has no title gap.
    let geometry = layout((canvas.width, canvas.height), cursor, (width, 18));
    for sprite in [&assets.tooltip_background, &assets.tooltip_frame] {
        canvas.gui_sprite(&sprite.image, geometry.background, sprite.scaling);
    }
    canvas.text(
        Some(assets),
        geometry.text.0,
        geometry.text.1,
        first,
        [255, 255, 255],
    );
    canvas.text(
        Some(assets),
        geometry.text.0,
        geometry.text.1 + 10,
        second,
        [255, 255, 255],
    );
}

/// Draw an item tooltip whose first line is separated from the remaining lines.
pub fn draw_item(
    canvas: &mut Canvas,
    assets: &MenuAssets,
    cursor: (i32, i32),
    lines: &[Vec<crate::text_component::Span>],
) {
    if lines.is_empty() {
        return;
    }
    let width = lines
        .iter()
        .map(|line| canvas.component_width(Some(assets), line))
        .max()
        .unwrap_or(0);
    let height = (lines.len() as i32 * 10) - if lines.len() == 1 { 2 } else { 0 };
    let geometry = layout((canvas.width, canvas.height), cursor, (width, height));
    for sprite in [&assets.tooltip_background, &assets.tooltip_frame] {
        canvas.gui_sprite(&sprite.image, geometry.background, sprite.scaling);
    }
    for (index, line) in lines.iter().enumerate() {
        let y = geometry.text.1 + index as i32 * 10 + if index > 0 { 2 } else { 0 };
        canvas.component_shadow_alpha(Some(assets), geometry.text.0, y, line, 255);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_flips_at_right_edge_and_fits_bottom() {
        assert_eq!(layout((320, 240), (10, 20), (100, 18)).text, (22, 8));
        assert_eq!(layout((320, 240), (310, 239), (100, 18)).text, (198, 219));
        assert_eq!(layout((80, 40), (78, 39), (100, 18)).text, (4, 19));
    }

    #[test]
    fn clipping_stops_at_unicode_character_boundaries() {
        let canvas = Canvas::new(0, 0);
        assert_eq!(canvas.text_prefix_by_width(None, "A한B", 12), "A한");
        assert_eq!(canvas.text_prefix_by_width(None, "A한B", 0), "");
        assert_eq!(canvas.text_prefix_by_width(None, "A한B", -1), "");
        assert_eq!(canvas.text_prefix_by_width(None, "A한B", 18), "A한B");
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn geometry_matches_java_positioner_and_tooltip_sprites() {
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
        let source = temp.path().join("TooltipGeometryOracle.java");
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/TooltipGeometryOracle.java"),
        )
        .unwrap();
        let mut cases = Vec::new();
        for sw in [160, 320, 426] {
            for sh in [120, 240] {
                for mx in [-10, 0, 4, 200, 318, 400] {
                    for my in [-10, 0, 10, 100, 239] {
                        for width in [0, 32, 100, 500] {
                            cases.push(serde_json::json!({
                                "screen_width":sw, "screen_height":sh, "mouse_x":mx, "mouse_y":my,
                                "width":width, "height":18
                            }));
                        }
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
                .arg("TooltipGeometryOracle")
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
        assert_eq!(expected.len(), 720);
        for (case, expected) in cases.iter().zip(expected) {
            let get = |name| case[name].as_i64().unwrap() as i32;
            let actual = layout(
                (get("screen_width"), get("screen_height")),
                (get("mouse_x"), get("mouse_y")),
                (get("width"), get("height")),
            );
            assert_eq!(
                expected["text"],
                serde_json::json!([
                    [actual.text.0, actual.text.1],
                    [actual.text.0, actual.text.1 + 10]
                ]),
                "{case}"
            );
            let rect = actual.background;
            assert_eq!(
                expected["draws"],
                serde_json::json!([
                    ["tooltip/background", rect.x, rect.y, rect.w, rect.h],
                    ["tooltip/frame", rect.x, rect.y, rect.w, rect.h]
                ]),
                "{case}"
            );
        }
    }
}
