//! The 26.3 single-player experience bar, using the active resource stack.
use native_session::{GameMode, experience::Experience};

use crate::{
    menu_assets::MenuAssets,
    menu_draw::{Canvas, Rect},
};

/// Cache only the values that affect pixels, avoiding redraws for changes to
/// cumulative XP or progress that do not change the visible bar.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Display {
    bar_width: Option<i32>,
    level: Option<i32>,
}

impl Display {
    pub fn new(experience: Experience, mode: GameMode) -> Self {
        let visible = matches!(mode, GameMode::Survival | GameMode::Adventure);
        Self {
            bar_width: (visible && experience.needed_for_next_level() > 0)
                .then(|| ((experience.progress * 183.0) as i32).clamp(0, 182)),
            level: (visible && experience.level > 0).then_some(experience.level),
        }
    }
}

fn bar_rect(width: i32, height: i32) -> Rect {
    Rect {
        x: (width - 182) / 2,
        y: height - 29,
        w: 182,
        h: 5,
    }
}

fn level_text_positions(width: i32, height: i32, text_width: i32) -> [(i32, i32, [u8; 3]); 5] {
    let x = (width - text_width) / 2;
    let y = height - 35;
    [
        (x + 1, y, [0, 0, 0]),
        (x - 1, y, [0, 0, 0]),
        (x, y + 1, [0, 0, 0]),
        (x, y - 1, [0, 0, 0]),
        (x, y, [128, 255, 32]),
    ]
}

pub fn draw(canvas: &mut Canvas, assets: &MenuAssets, display: Display) {
    let rect = bar_rect(canvas.width, canvas.height);
    let (x, y) = (rect.x, rect.y);
    if let Some(width) = display.bar_width {
        let background = &assets.hud.experience_background;
        canvas.gui_sprite(&background.image, rect, background.scaling);
        if width > 0 {
            // Java draws the complete sprite then clips it for non-stretch pack
            // scaling. This also samples stretch overrides at their full width.
            let progress = &assets.hud.experience_progress;
            let mut full = Canvas::new(182, 5);
            full.gui_sprite(
                &progress.image,
                Rect {
                    x: 0,
                    y: 0,
                    w: 182,
                    h: 5,
                },
                progress.scaling,
            );
            for sy in 0..5 {
                for sx in 0..width {
                    let color = full.pixels[(sy * 182 + sx) as usize];
                    canvas.pixel(
                        x + sx,
                        y + sy,
                        [
                            (color >> 16) as u8,
                            (color >> 8) as u8,
                            color as u8,
                            (color >> 24) as u8,
                        ],
                    );
                }
            }
        }
    }
    if let Some(level) = display.level {
        let template = assets.translate("gui.experience.level", "%s");
        let number = level.to_string();
        let text = if template.contains("%1$s") {
            template.replace("%1$s", &number)
        } else {
            template.replacen("%s", &number, 1)
        };
        for (x, y, color) in level_text_positions(
            canvas.width,
            canvas.height,
            canvas.text_width(Some(assets), &text),
        ) {
            canvas.text_without_shadow(Some(assets), x, y, &text, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibility_and_pixel_cache_follow_game_mode_and_progress() {
        let exp = Experience {
            progress: 0.5,
            level: 15,
            total: 100,
        };
        let survival = Display::new(exp, GameMode::Survival);
        assert_eq!(survival.bar_width, Some(91));
        assert_eq!(survival.level, Some(15));
        assert_eq!(
            survival,
            Display::new(
                Experience {
                    total: 101,
                    progress: 0.5001,
                    ..exp
                },
                GameMode::Adventure
            )
        );
        for mode in [GameMode::Creative, GameMode::Spectator] {
            assert_eq!(
                Display::new(exp, mode),
                Display {
                    bar_width: None,
                    level: None
                }
            );
        }
        assert_eq!(
            Display::new(Experience::default(), GameMode::Survival).level,
            None
        );
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn experience_bar_uses_pack_sprite_scaling_and_clips_progress() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let pack = tempfile::tempdir().unwrap();
        let sprites = pack
            .path()
            .join("assets/minecraft/textures/gui/sprites/hud");
        std::fs::create_dir_all(&sprites).unwrap();
        image::RgbaImage::from_pixel(182, 5, image::Rgba([20, 30, 40, 255]))
            .save(sprites.join("experience_bar_background.png"))
            .unwrap();
        let image =
            image::RgbaImage::from_fn(4, 5, |x, _| image::Rgba([50 + x as u8 * 30, 90, 140, 255]));
        image
            .save(sprites.join("experience_bar_progress.png"))
            .unwrap();
        std::fs::write(
            sprites.join("experience_bar_progress.png.mcmeta"),
            br#"{"gui":{"scaling":{"type":"tile","width":4,"height":5}}}"#,
        )
        .unwrap();
        let assets = MenuAssets::load(&game, &[pack.path().to_path_buf()]).unwrap();
        let mut canvas = Canvas::new(426, 240);
        draw(
            &mut canvas,
            &assets,
            Display::new(
                Experience {
                    progress: 0.5,
                    level: 0,
                    total: 0,
                },
                GameMode::Survival,
            ),
        );
        let rect = bar_rect(426, 240);
        for index in 0..182 {
            let pixel = canvas.pixels[((rect.y + 2) * canvas.width + rect.x + index) as usize];
            let expected = if index < 91 {
                0xff00_0000 | ((50 + (index % 4) * 30) as u32) << 16 | 90 << 8 | 140
            } else {
                0xff14_1e28
            };
            assert_eq!(pixel, expected, "bar pixel {index}");
        }
        let mut hidden = Canvas::new(426, 240);
        draw(
            &mut hidden,
            &assets,
            Display::new(
                Experience {
                    progress: 0.5,
                    level: 3,
                    total: 0,
                },
                GameMode::Creative,
            ),
        );
        assert!(hidden.pixels.iter().all(|pixel| *pixel == 0));
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn experience_bar_and_level_layout_match_java_26_3() {
        use serde_json::json;
        use std::{ffi::OsString, path::PathBuf, process::Command};
        const JAVA: &str = r#"
import java.lang.reflect.*;
import java.nio.file.*;
import com.google.gson.*;
import com.mojang.blaze3d.platform.Window;
import com.mojang.renderpearl.api.pipeline.RenderPipeline;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.Minecraft;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.client.gui.Font;
import net.minecraft.client.gui.GuiGraphicsExtractor;
import net.minecraft.client.gui.contextualbar.ContextualBar;
import net.minecraft.client.gui.contextualbar.ExperienceBar;
import net.minecraft.network.chat.Component;
import net.minecraft.network.chat.FormattedText;
import net.minecraft.resources.Identifier;
public class ExperienceHudOracle {
  static class Capture extends GuiGraphicsExtractor {
    int w,h; JsonArray draws,texts;
    Capture() { super(null,null,0,0); }
    @Override public int guiWidth() { return w; }
    @Override public int guiHeight() { return h; }
    @Override public void blitSprite(RenderPipeline p, Identifier id, int x,int y,int w,int h) {
      JsonArray a=new JsonArray(); a.add(id.getPath()); for(int v:new int[]{x,y,w,h}) a.add(v); draws.add(a);
    }
    @Override public void blitSprite(RenderPipeline p, Identifier id, int sw,int sh,int sx,int sy,int x,int y,int w,int h) {
      JsonArray a=new JsonArray(); a.add(id.getPath()); for(int v:new int[]{sw,sh,sx,sy,x,y,w,h}) a.add(v); draws.add(a);
    }
    @Override public void text(Font f, Component text, int x,int y,int color,boolean shadow) {
      JsonArray a=new JsonArray(); a.add(x); a.add(y); a.add(color); a.add(shadow); texts.add(a);
    }
  }
  static class TestFont extends Font {
    int testWidth;
    TestFont() { super(null); }
    @Override public int width(FormattedText text) { return testWidth; }
  }
  static void set(Class<?> c,Object o,String name,Object value) throws Exception {
    Field f=c.getDeclaredField(name); f.setAccessible(true); f.set(o,value);
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    Field f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var u=(sun.misc.Unsafe)f.get(null);
    var game=(Minecraft)u.allocateInstance(Minecraft.class);
    var window=(Window)u.allocateInstance(Window.class);
    var player=(LocalPlayer)u.allocateInstance(LocalPlayer.class);
    var capture=(Capture)u.allocateInstance(Capture.class);
    var font=(TestFont)u.allocateInstance(TestFont.class);
    set(Minecraft.class,game,"window",window); set(Minecraft.class,game,"player",player);
    var bar=new ExperienceBar(game);
    var cases=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
    var result=new JsonArray();
    for(var element:cases) {
      var c=element.getAsJsonObject(); int w=c.get("w").getAsInt(), h=c.get("h").getAsInt();
      set(Window.class,window,"guiScaledWidth",w); set(Window.class,window,"guiScaledHeight",h);
      player.experienceLevel=c.get("level").getAsInt(); player.experienceProgress=c.get("progress").getAsFloat();
      capture.w=w; capture.h=h; capture.draws=new JsonArray(); capture.texts=new JsonArray();
      font.testWidth=c.get("text_width").getAsInt();
      bar.extractBackground(capture,null);
      if(player.experienceLevel>0) ContextualBar.extractExperienceLevel(capture,font,player.experienceLevel);
      var row=new JsonObject(); row.addProperty("needed",player.getXpNeededForNextLevel()); row.add("draws",capture.draws); row.add("texts",capture.texts); result.add(row);
    }
    Files.writeString(Path.of(args[1]),new Gson().toJson(result));
  }
}
"#;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("ExperienceHudOracle.java");
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
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "Java XP HUD oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&classpath)
            .arg(&source));
        let mut cases = Vec::new();
        for (w, h) in [(320, 240), (426, 240), (641, 360)] {
            for level in [-4, 0, 1, 14, 15, 29, 30, 42, 2_000_000_000] {
                for progress in [0.0_f32, 0.01, 0.125, 0.5, 0.99, 0.999] {
                    cases.push(
                        json!({"w":w,"h":h,"level":level,"progress":progress,"text_width":11}),
                    );
                }
            }
        }
        let input = temp.path().join("cases.json");
        let output = temp.path().join("results.json");
        std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&classpath)
            .arg("ExperienceHudOracle")
            .arg(&input)
            .arg(&output));
        let results: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(results.len(), cases.len());
        for (case, expected) in cases.iter().zip(results) {
            let w = case["w"].as_i64().unwrap() as i32;
            let h = case["h"].as_i64().unwrap() as i32;
            let experience = Experience {
                level: case["level"].as_i64().unwrap() as i32,
                progress: case["progress"].as_f64().unwrap() as f32,
                total: 0,
            };
            let display = Display::new(experience, GameMode::Survival);
            assert_eq!(
                experience.needed_for_next_level(),
                expected["needed"].as_i64().unwrap() as i32,
                "{case}"
            );
            let mut draws = Vec::new();
            let rect = bar_rect(w, h);
            if let Some(width) = display.bar_width {
                draws.push(json!([
                    "hud/experience_bar_background",
                    rect.x,
                    rect.y,
                    182,
                    5
                ]));
                if width > 0 {
                    draws.push(json!([
                        "hud/experience_bar_progress",
                        182,
                        5,
                        0,
                        0,
                        rect.x,
                        rect.y,
                        width,
                        5
                    ]));
                }
            }
            assert_eq!(json!(draws), expected["draws"], "{case}");
            let texts = if display.level.is_some() {
                level_text_positions(w, h, 11)
                    .into_iter()
                    .map(|(x, y, color)| {
                        let argb = if color == [0, 0, 0] {
                            -16_777_216_i32
                        } else {
                            -8_323_296_i32
                        };
                        json!([x, y, argb, false])
                    })
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            assert_eq!(json!(texts), expected["texts"], "{case}");
        }
    }
}
