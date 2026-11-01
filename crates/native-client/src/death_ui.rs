//! The 26.3 death screen. Rendering uses the selected Minecraft resources.

use native_session::{DeathCause, DeathInfo};
use std::time::{Duration, Instant};
use winit::keyboard::KeyCode;

use crate::{
    menu_assets::MenuAssets,
    menu_draw::{Canvas, Rect},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeathAction {
    None,
    Respawn,
    QuitToTitle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Page {
    Death,
    ConfirmQuit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DrawKey {
    size: (u32, u32),
    info: DeathInfo,
    page: Page,
    ready: bool,
    hover: Option<usize>,
    focus: Option<usize>,
    can_respawn: bool,
}

pub struct DeathScreen {
    info: DeathInfo,
    hardcore: bool,
    page: Page,
    page_elapsed: Duration,
    last_update: Instant,
    focus: Option<usize>,
    can_respawn: bool,
    last_draw: Option<DrawKey>,
}

impl DeathScreen {
    pub fn new(info: DeathInfo, hardcore: bool) -> Self {
        Self {
            info,
            hardcore,
            page: Page::Death,
            page_elapsed: Duration::ZERO,
            last_update: Instant::now(),
            focus: None,
            can_respawn: info.can_respawn,
            last_draw: None,
        }
    }

    pub fn info(&self) -> DeathInfo {
        self.info
    }

    pub fn update(&mut self, info: DeathInfo) {
        let now = Instant::now();
        self.advance(now.saturating_duration_since(self.last_update));
        self.last_update = now;
        self.info = info;
        self.can_respawn = info.can_respawn;
    }

    fn advance(&mut self, elapsed: Duration) {
        self.page_elapsed = self.page_elapsed.saturating_add(elapsed);
    }

    pub fn is_confirming(&self) -> bool {
        self.page == Page::ConfirmQuit
    }

    fn ready(&self) -> bool {
        self.page_elapsed >= Duration::from_secs(1)
    }

    fn button_rects(&self, width: i32, height: i32) -> [Rect; 2] {
        match self.page {
            Page::Death => [
                Rect {
                    x: width / 2 - 100,
                    y: height / 4 + 72,
                    w: 200,
                    h: 20,
                },
                Rect {
                    x: width / 2 - 100,
                    y: height / 4 + 96,
                    w: 200,
                    h: 20,
                },
            ],
            Page::ConfirmQuit => [
                Rect {
                    x: width / 2 - 152,
                    y: (height - 61) / 2 + 41,
                    w: 150,
                    h: 20,
                },
                Rect {
                    x: width / 2 + 2,
                    y: (height - 61) / 2 + 41,
                    w: 150,
                    h: 20,
                },
            ],
        }
    }

    fn enabled(&self, button: usize) -> bool {
        self.ready()
            && match self.page {
                Page::Death => button == 1 || self.can_respawn,
                Page::ConfirmQuit => button == 0 || self.can_respawn,
            }
    }

    fn hover(&self, size: (i32, i32), cursor: (i32, i32)) -> Option<usize> {
        self.button_rects(size.0, size.1)
            .iter()
            .position(|rect| rect.contains(cursor.0, cursor.1))
    }

    fn activate(&mut self, button: usize) -> DeathAction {
        if !self.enabled(button) {
            return DeathAction::None;
        }
        self.focus = Some(button);
        self.last_draw = None;
        match (self.page, button) {
            (Page::Death, 0) | (Page::ConfirmQuit, 1) => {
                self.page = Page::Death;
                DeathAction::Respawn
            }
            (Page::Death, 1) if !self.hardcore => {
                self.page = Page::ConfirmQuit;
                self.page_elapsed = Duration::ZERO;
                self.last_update = Instant::now();
                self.focus = None;
                DeathAction::None
            }
            (Page::Death, 1) | (Page::ConfirmQuit, 0) => DeathAction::QuitToTitle,
            _ => DeathAction::None,
        }
    }

    pub fn click(&mut self, size: (i32, i32), cursor: (i32, i32)) -> DeathAction {
        self.hover(size, cursor)
            .map_or(DeathAction::None, |button| self.activate(button))
    }

    pub fn key(&mut self, key: KeyCode, shift: bool) -> DeathAction {
        if key == KeyCode::Escape && self.page == Page::ConfirmQuit && self.ready() {
            // Until respawning is supported, returning to the death page avoids
            // throwing away the in-memory session or fabricating a spawn point.
            if self.can_respawn {
                self.page = Page::Death;
                self.last_draw = None;
                return DeathAction::Respawn;
            }
            self.page = Page::Death;
            self.page_elapsed = Duration::ZERO;
            self.last_update = Instant::now();
            self.focus = None;
            self.last_draw = None;
            return DeathAction::None;
        }
        if matches!(
            key,
            KeyCode::Tab
                | KeyCode::ArrowUp
                | KeyCode::ArrowDown
                | KeyCode::ArrowLeft
                | KeyCode::ArrowRight
        ) {
            let mut buttons = (0..2)
                .filter(|&button| self.enabled(button))
                .collect::<Vec<_>>();
            if shift || matches!(key, KeyCode::ArrowUp | KeyCode::ArrowLeft) {
                buttons.reverse();
            }
            if !buttons.is_empty() {
                let index = self
                    .focus
                    .and_then(|current| buttons.iter().position(|&button| button == current));
                self.focus = Some(buttons[index.map_or(0, |i| (i + 1) % buttons.len())]);
                self.last_draw = None;
            }
        } else if matches!(key, KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Space)
            && let Some(button) = self.focus
        {
            return self.activate(button);
        }
        DeathAction::None
    }

    pub fn draw(
        &mut self,
        size: (u32, u32),
        cursor: (i32, i32),
        assets: &MenuAssets,
    ) -> Option<Canvas> {
        let (width, height) = (size.0 as i32, size.1 as i32);
        if width <= 0 || height <= 0 {
            return None;
        }
        let hover = self.hover((width, height), cursor);
        let key = DrawKey {
            size,
            info: self.info,
            page: self.page,
            ready: self.ready(),
            hover,
            focus: self.focus,
            can_respawn: self.can_respawn,
        };
        if self.last_draw == Some(key) {
            return None;
        }
        let mut canvas = Canvas::new(size.0, size.1);
        for y in 0..height {
            let n = y as u32;
            let d = height.max(1) as u32;
            let mix = |a: u32, b: u32| ((a * (d - n) + b * n) / d) as u8;
            canvas.fill(
                Rect {
                    x: 0,
                    y,
                    w: width,
                    h: 1,
                },
                [mix(80, 128), mix(0, 48), mix(0, 48), mix(96, 160)],
            );
        }
        match self.page {
            Page::Death => {
                let title = if self.hardcore {
                    assets.translate("deathScreen.title.hardcore", "Game Over!")
                } else {
                    assets.translate("deathScreen.title", "You Died!")
                };
                canvas.text_centered_scaled(Some(assets), width / 2, 60, title, [255; 3], 2);
                let cause = match self.info.cause {
                    DeathCause::Unknown => None,
                    DeathCause::Fall => Some(("death.attack.fall", "%1$s hit the ground too hard")),
                    DeathCause::FallHigh => {
                        Some(("death.fell.accident.generic", "%1$s fell from a high place"))
                    }
                    DeathCause::Stalagmite => Some((
                        "death.attack.stalagmite",
                        "%1$s was impaled on a stalagmite",
                    )),
                    DeathCause::Drown => Some(("death.attack.drown", "%1$s drowned")),
                    DeathCause::Lava => Some(("death.attack.lava", "%1$s tried to swim in lava")),
                    DeathCause::OnFire => Some(("death.attack.onFire", "%1$s burned to death")),
                    DeathCause::InFire => Some(("death.attack.inFire", "%1$s went up in flames")),
                    DeathCause::Campfire => Some(("death.attack.inFire", "%1$s went up in flames")),
                    DeathCause::HotFloor => Some((
                        "death.attack.hotFloor",
                        "%1$s discovered the floor was lava",
                    )),
                    DeathCause::Starve => Some(("death.attack.starve", "%1$s starved to death")),
                };
                if self.info.show_message
                    && let Some((key, fallback)) = cause
                {
                    let text = assets
                        .translate(key, fallback)
                        .replace("%1$s", "Player")
                        .replace("%s", "Player");
                    canvas.text_centered(Some(assets), width / 2, 85, &text, [255; 3]);
                }
                let template = assets.translate("deathScreen.score.value", "Score: %s");
                let (before, after) = template
                    .split_once("%1$s")
                    .or_else(|| template.split_once("%s"))
                    .unwrap_or((template, ""));
                let score = self.info.score.to_string();
                let left = width / 2
                    - (canvas.text_width(Some(assets), before)
                        + canvas.text_width(Some(assets), &score)
                        + canvas.text_width(Some(assets), after))
                        / 2;
                canvas.text(Some(assets), left, 100, before, [255; 3]);
                let score_x = left + canvas.text_width(Some(assets), before);
                canvas.text(Some(assets), score_x, 100, &score, [255, 255, 85]);
                canvas.text(
                    Some(assets),
                    score_x + canvas.text_width(Some(assets), &score),
                    100,
                    after,
                    [255; 3],
                );
                if self.ready() && !self.can_respawn && !self.info.respawn_pending {
                    let notice = if self.info.inventory_unresolved {
                        "Items were kept because they could not be processed"
                    } else if self.hardcore {
                        "Spectating is not available yet"
                    } else {
                        "Respawn is not available in this world yet"
                    };
                    canvas.text_centered(
                        Some(assets),
                        width / 2,
                        height / 4 + 122,
                        notice,
                        [200, 200, 200],
                    );
                }
            }
            Page::ConfirmQuit => canvas.text_centered(
                Some(assets),
                width / 2,
                (height - 61) / 2,
                assets.translate("deathScreen.quit.confirm", "Are you sure you want to quit?"),
                [255; 3],
            ),
        }
        let labels = match self.page {
            Page::Death => [
                if self.hardcore {
                    assets.translate("deathScreen.spectate", "Spectate World")
                } else {
                    assets.translate("deathScreen.respawn", "Respawn")
                },
                assets.translate("deathScreen.titleScreen", "Title Screen"),
            ],
            Page::ConfirmQuit => [
                assets.translate("deathScreen.titleScreen", "Title Screen"),
                assets.translate("deathScreen.respawn", "Respawn"),
            ],
        };
        for (index, rect) in self.button_rects(width, height).into_iter().enumerate() {
            canvas.button(
                Some(assets),
                rect,
                self.enabled(index),
                hover == Some(index) || self.focus == Some(index),
                labels[index],
            );
        }
        self.last_draw = Some(key);
        Some(canvas)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(can_respawn: bool) -> DeathInfo {
        DeathInfo {
            cause: DeathCause::Fall,
            tick: 1,
            score: 42,
            show_message: true,
            inventory_unresolved: false,
            respawn_pending: false,
            can_respawn,
        }
    }

    #[test]
    fn buttons_use_vanilla_bounds_and_wait_twenty_ticks() {
        let mut screen = DeathScreen::new(info(true), false);
        assert_eq!(
            screen.button_rects(426, 240),
            [
                Rect {
                    x: 113,
                    y: 132,
                    w: 200,
                    h: 20
                },
                Rect {
                    x: 113,
                    y: 156,
                    w: 200,
                    h: 20
                },
            ]
        );
        screen.advance(Duration::from_millis(950));
        assert_eq!(screen.click((426, 240), (150, 140)), DeathAction::None);
        screen.advance(Duration::from_millis(50));
        assert_eq!(screen.click((426, 240), (150, 140)), DeathAction::Respawn);
    }

    #[test]
    fn normal_quit_has_its_own_delay_and_confirmation_pauses_the_world() {
        let mut screen = DeathScreen::new(info(true), false);
        screen.advance(Duration::from_secs(1));
        assert_eq!(screen.click((426, 240), (150, 160)), DeathAction::None);
        assert!(screen.is_confirming());
        assert_eq!(screen.key(KeyCode::Escape, false), DeathAction::None);
        screen.advance(Duration::from_secs(1));
        assert_eq!(screen.key(KeyCode::Escape, false), DeathAction::Respawn);
        assert!(!screen.is_confirming());
    }

    #[test]
    fn unsupported_respawn_stays_disabled_and_keyboard_can_exit() {
        let mut screen = DeathScreen::new(info(false), false);
        screen.advance(Duration::from_secs(1));
        assert_eq!(screen.key(KeyCode::Tab, false), DeathAction::None);
        assert_eq!(screen.focus, Some(1));
        assert_eq!(screen.key(KeyCode::Enter, false), DeathAction::None);
        screen.advance(Duration::from_secs(1));
        assert_eq!(screen.key(KeyCode::Tab, false), DeathAction::None);
        assert_eq!(screen.key(KeyCode::Enter, false), DeathAction::QuitToTitle);
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn death_buttons_match_installed_java_26_3() {
        use std::{ffi::OsString, path::PathBuf, process::Command};
        const JAVA: &str = r#"
import java.lang.reflect.*;
import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.Font;
import net.minecraft.client.gui.screens.DeathScreen;
import net.minecraft.client.gui.components.Button;
import net.minecraft.client.gui.components.StringWidget;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.network.chat.Component;
import net.minecraft.network.chat.FormattedText;
import net.minecraft.util.FormattedCharSequence;
import java.util.List;
public class DeathScreenOracle {
  static class Player extends LocalPlayer {
    Player() { super(null,null,null,null,null,null,false,null,null); }
    @Override public int getScore() { return 42; }
  }
  static class Screen extends DeathScreen {
    Screen(LocalPlayer p, boolean hardcore) { super(Component.literal("cause"), hardcore, p); }
    void start(int w,int h) { width=w; height=h; super.init(); }
  }
  static class FakeFont extends Font {
    FakeFont() { super(null); }
    @Override public int width(String text) { return text.length()*6; }
    @Override public int width(FormattedText text) { return 120; }
    @Override public int width(FormattedCharSequence text) { return 120; }
    @Override public List<FormattedCharSequence> split(FormattedText text,int width) { return List.of(); }
  }
  static class Confirm extends DeathScreen.TitleConfirmScreen {
    Confirm() { super(x -> {}, Component.literal("quit?"), Component.empty(), Component.literal("title"), Component.literal("respawn")); }
    void start(int w,int h) { width=w; height=h; super.init(); setDelay(20); }
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    Field f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
    var unsafe=(sun.misc.Unsafe)f.get(null);
    Field instance=Minecraft.class.getDeclaredField("instance"); instance.setAccessible(true);
    Minecraft fake=(Minecraft)unsafe.allocateInstance(Minecraft.class);
    Field font=Minecraft.class.getDeclaredField("font"); font.setAccessible(true); font.set(fake,new FakeFont());
    instance.set(null, fake);
    var player=(Player)unsafe.allocateInstance(Player.class);
    JsonArray result=new JsonArray();
    for (int[] size: new int[][]{{320,240},{426,240},{640,360},{853,479},{1067,600}}) {
      for (boolean hardcore: new boolean[]{false,true}) {
        var screen=new Screen(player,hardcore); screen.start(size[0],size[1]);
        for (int tick=0;tick<=20;tick++) {
          if (tick>0) screen.tick();
          JsonObject row=new JsonObject(); row.addProperty("w",size[0]); row.addProperty("h",size[1]);
          row.addProperty("confirm",false);
          row.addProperty("hardcore",hardcore); row.addProperty("tick",tick);
          row.addProperty("pause",screen.isPauseScreen()); row.addProperty("escape",screen.shouldCloseOnEsc());
          JsonArray buttons=new JsonArray();
          for (var child:screen.children()) {
            Button b=(Button)child; JsonArray a=new JsonArray();
            a.add(b.getX()); a.add(b.getY()); a.add(b.getWidth()); a.add(b.getHeight()); a.add(b.active);
            buttons.add(a);
          }
          row.add("buttons",buttons); result.add(row);
        }
      }
      var confirm=new Confirm(); confirm.start(size[0],size[1]);
      for (int tick=0;tick<=20;tick++) {
        if (tick>0) confirm.tick();
        JsonObject row=new JsonObject(); row.addProperty("w",size[0]); row.addProperty("h",size[1]);
        row.addProperty("confirm",true); row.addProperty("hardcore",false); row.addProperty("tick",tick);
        row.addProperty("pause",confirm.isPauseScreen()); row.addProperty("escape",confirm.shouldCloseOnEsc());
        JsonArray buttons=new JsonArray();
        for (var child:confirm.children()) {
          if (child instanceof Button b) {
            JsonArray a=new JsonArray(); a.add(b.getX()); a.add(b.getY()); a.add(b.getWidth()); a.add(b.getHeight()); a.add(b.active); buttons.add(a);
          }
          if (child instanceof StringWidget text) row.addProperty("titleY",text.getY());
        }
        row.add("buttons",buttons); result.add(row);
      }
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(result));
  }
}
"#;
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
        let source = temp.path().join("DeathScreenOracle.java");
        let output = temp.path().join("output.json");
        std::fs::write(&source, JAVA).unwrap();
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
                .arg("DeathScreenOracle")
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
        assert_eq!(rows.len(), 5 * 3 * 21);
        for row in rows {
            let w = row["w"].as_i64().unwrap() as i32;
            let h = row["h"].as_i64().unwrap() as i32;
            let tick = row["tick"].as_u64().unwrap();
            let mut screen = DeathScreen::new(info(true), row["hardcore"].as_bool().unwrap());
            if row["confirm"].as_bool().unwrap() {
                screen.page = Page::ConfirmQuit;
                assert_eq!(row["titleY"].as_i64(), Some(i64::from((h - 61) / 2)));
            }
            screen.advance(Duration::from_millis(tick * 50));
            assert_eq!(row["pause"].as_bool(), Some(screen.is_confirming()));
            assert_eq!(row["escape"].as_bool(), Some(false));
            for (index, rect) in screen.button_rects(w, h).iter().enumerate() {
                let actual = &row["buttons"][index];
                assert_eq!(
                    [rect.x, rect.y, rect.w, rect.h],
                    [
                        actual[0].as_i64().unwrap() as i32,
                        actual[1].as_i64().unwrap() as i32,
                        actual[2].as_i64().unwrap() as i32,
                        actual[3].as_i64().unwrap() as i32
                    ]
                );
                assert_eq!(screen.enabled(index), actual[4].as_bool().unwrap());
            }
        }
    }
}
