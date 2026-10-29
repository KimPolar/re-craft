//! Frame-rate limiting does not limit the separate 20 TPS simulation.
use std::time::{Duration, Instant};

use crate::menu_settings::InactivityFpsLimit;

/// Keep a stable cadence through ordinary wake-up jitter, but avoid requesting
/// another frame immediately after a substantially late one.
pub fn advance_frame_deadline(deadline: Instant, now: Instant, interval: Duration) -> Instant {
    let elapsed = now.saturating_duration_since(deadline);
    let periods = (elapsed.as_nanos() / interval.as_nanos() + 1).min(u128::from(u32::MAX)) as u32;
    let next = deadline
        .checked_add(interval.saturating_mul(periods))
        .unwrap_or(now + interval);
    if next.saturating_duration_since(now) < interval / 2 {
        now + interval
    } else {
        next
    }
}

/// Java 26.3 changes the render limit without slowing the simulation tick.
pub struct InactivityTracker {
    last_input: Instant,
}

impl InactivityTracker {
    pub fn new(now: Instant) -> Self {
        Self { last_input: now }
    }

    /// Returns whether input ended an inactivity throttle.
    pub fn input(&mut self, now: Instant, setting: InactivityFpsLimit) -> bool {
        let was_idle = setting == InactivityFpsLimit::Afk
            && now.saturating_duration_since(self.last_input).as_millis() > 60_000;
        self.last_input = now;
        was_idle
    }

    pub fn limit(
        &self,
        now: Instant,
        setting: InactivityFpsLimit,
        configured: u16,
        outside_world: bool,
        minimized: bool,
    ) -> u16 {
        if minimized {
            return 10;
        }
        if setting == InactivityFpsLimit::Afk {
            let elapsed_ms = now.saturating_duration_since(self.last_input).as_millis();
            if elapsed_ms > 600_000 {
                return 10;
            }
            if elapsed_ms > 60_000 {
                return configured.min(30);
            }
        }
        if outside_world { 60 } else { configured }
    }
}

pub struct FramePacer {
    interval: Option<Duration>,
    fps: u16,
    next: Instant,
}

impl FramePacer {
    pub fn new(max_fps: u16, now: Instant) -> Self {
        let interval = (max_fps < 260)
            .then(|| Duration::from_nanos(1_000_000_000 / u64::from(max_fps.max(1))));
        Self {
            interval,
            fps: max_fps,
            next: now,
        }
    }

    pub fn set_limit(&mut self, fps: u16, now: Instant) {
        if self.fps != fps {
            *self = Self::new(fps, now);
        }
    }

    pub fn due(&mut self, now: Instant) -> bool {
        let Some(interval) = self.interval else {
            return true;
        };
        if now < self.next {
            return false;
        }
        self.next = advance_frame_deadline(self.next, now, interval);
        true
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.interval.map(|_| self.next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_session::{GameSession, LocalSession};

    #[test]
    fn low_frame_limits_keep_the_simulation_at_twenty_ticks() {
        for fps in [10, 30, 60, 120, 250] {
            let start = Instant::now();
            let end = start + Duration::from_secs(1);
            let mut pacer = FramePacer::new(fps, start);
            let mut session = LocalSession::demo();
            let mut now = start;
            let mut previous = start;
            let mut frames = 0;
            while now < end {
                session.update(now - previous);
                previous = now;
                if pacer.due(now) {
                    frames += 1;
                }
                now = pacer
                    .deadline()
                    .unwrap()
                    .min(now + Duration::from_millis(50))
                    .min(end);
            }
            session.update(end - previous);
            assert_eq!(session.snapshot().tick, 20, "{fps} FPS");
            assert!(
                frames >= usize::from(fps) && frames <= usize::from(fps) + 1,
                "{fps}: {frames}"
            );
        }
    }

    #[test]
    fn missed_frames_are_not_replayed_and_unlimited_has_no_deadline() {
        let start = Instant::now();
        let mut limited = FramePacer::new(10, start);
        assert!(limited.due(start));
        assert!(limited.due(start + Duration::from_millis(450)));
        assert!(!limited.due(start + Duration::from_millis(450)));
        assert_eq!(limited.deadline(), Some(start + Duration::from_millis(500)));
        assert!(limited.due(start + Duration::from_millis(599)));
        assert_eq!(limited.deadline(), Some(start + Duration::from_millis(699)));
        let mut unlimited = FramePacer::new(260, start);
        assert!(unlimited.due(start));
        assert!(unlimited.due(start));
        assert!(unlimited.deadline().is_none());
    }

    #[test]
    fn ordinary_late_wakeups_do_not_accumulate_frame_drift() {
        let start = Instant::now();
        let mut pacer = FramePacer::new(60, start);
        let interval = Duration::from_nanos(1_000_000_000 / 60);
        for frame in 0..600_u32 {
            let scheduled = start + interval * frame;
            assert!(pacer.due(scheduled + Duration::from_millis(1)));
            assert_eq!(pacer.deadline(), Some(start + interval * (frame + 1)));
        }
    }

    #[test]
    fn inactivity_boundaries_and_input_restore_rendering() {
        let start = Instant::now();
        let mut tracker = InactivityTracker::new(start);
        let limit = |tracker: &InactivityTracker, seconds, setting, menu, fps, minimized| {
            tracker.limit(
                start + Duration::from_secs(seconds),
                setting,
                fps,
                menu,
                minimized,
            )
        };
        assert_eq!(
            limit(&tracker, 60, InactivityFpsLimit::Afk, true, 120, false),
            60
        );
        assert_eq!(
            limit(&tracker, 61, InactivityFpsLimit::Afk, true, 120, false),
            30
        );
        assert_eq!(
            tracker.limit(
                start + Duration::from_micros(60_000_999),
                InactivityFpsLimit::Afk,
                120,
                true,
                false
            ),
            60
        );
        assert_eq!(
            limit(&tracker, 600, InactivityFpsLimit::Afk, false, 120, false),
            30
        );
        assert_eq!(
            limit(&tracker, 601, InactivityFpsLimit::Afk, false, 120, false),
            10
        );
        assert_eq!(
            limit(&tracker, 61, InactivityFpsLimit::Afk, true, 10, false),
            10
        );
        assert_eq!(
            limit(
                &tracker,
                601,
                InactivityFpsLimit::Minimized,
                true,
                120,
                false
            ),
            60
        );
        assert_eq!(
            limit(
                &tracker,
                601,
                InactivityFpsLimit::Minimized,
                false,
                120,
                false
            ),
            120
        );
        assert_eq!(
            limit(&tracker, 0, InactivityFpsLimit::Minimized, false, 120, true),
            10
        );
        let resumed = start + Duration::from_secs(601);
        assert!(tracker.input(resumed, InactivityFpsLimit::Afk));
        assert_eq!(
            tracker.limit(resumed, InactivityFpsLimit::Afk, 120, false, false),
            120
        );
        assert!(!tracker.input(resumed, InactivityFpsLimit::Afk));
        let mut pacer = FramePacer::new(10, start);
        assert!(pacer.due(start));
        pacer.set_limit(120, resumed);
        assert!(pacer.due(resumed));
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn inactivity_throttle_matches_installed_java() {
        use std::{ffi::OsString, path::PathBuf, process::Command};
        const JAVA: &str = r#"
import com.mojang.blaze3d.platform.*;
import net.minecraft.client.*;
import net.minecraft.client.gui.Gui;
import net.minecraft.client.gui.screens.TitleScreen;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.util.Util;
import sun.misc.Unsafe;
public class InactivityOracle {
    static Unsafe unsafe;
    static Object alloc(Class<?> c) throws Exception { return unsafe.allocateInstance(c); }
    static void set(Class<?> c, Object object, String name, Object value) throws Exception {
        var field = c.getDeclaredField(name); field.setAccessible(true); field.set(object, value);
    }
    public static void main(String[] args) throws Exception {
        var out = System.out;
        net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
        var field = Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        unsafe = (Unsafe)field.get(null);
        var options = (Options)alloc(Options.class);
        var inactivity = (OptionInstance<?>)alloc(OptionInstance.class);
        set(Options.class, options, "inactivityFpsLimit", inactivity);
        var client = (Minecraft)alloc(Minecraft.class);
        var window = (Window)alloc(Window.class);
        set(Window.class, window, "focused", true);
        set(Minecraft.class, client, "window", window);
        var gui = (Gui)alloc(Gui.class);
        set(Gui.class, gui, "screen", alloc(TitleScreen.class));
        set(Minecraft.class, client, "gui", gui);
        var level = (ClientLevel)alloc(ClientLevel.class);
        var tracker = (FramerateLimitTracker)alloc(FramerateLimitTracker.class);
        set(FramerateLimitTracker.class, tracker, "options", options);
        set(FramerateLimitTracker.class, tracker, "minecraft", client);
        for (int fps : new int[]{10,20,30,60,120,260})
          for (boolean menu : new boolean[]{false,true})
            for (var setting : new InactivityFpsLimit[]{InactivityFpsLimit.AFK, InactivityFpsLimit.MINIMIZED})
              for (long age : new long[]{0,59900,60100,599900,600100})
                for (boolean minimized : new boolean[]{false,true}) {
                  tracker.setFramerateLimit(fps);
                  set(OptionInstance.class, inactivity, "value", setting);
                  set(Window.class, window, "iconified", minimized);
                  set(Minecraft.class, client, "level", menu ? null : level);
                  set(FramerateLimitTracker.class, tracker, "latestInputTime", Util.getMillis() - age);
                  out.println(tracker.getFramerateLimit());
                }
    }
}
"#;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("InactivityOracle.java");
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
                "Java FPS oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            output.stdout
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&classpath)
            .arg(&source));
        let output = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&classpath)
            .arg("InactivityOracle"));
        let text = String::from_utf8(output).unwrap();
        let mut lines = text.lines();
        let start = Instant::now();
        let tracker = InactivityTracker::new(start);
        for fps in [10, 20, 30, 60, 120, 260] {
            for menu in [false, true] {
                for setting in [InactivityFpsLimit::Afk, InactivityFpsLimit::Minimized] {
                    for age in [0, 59_900, 60_100, 599_900, 600_100] {
                        for minimized in [false, true] {
                            let expected: u16 = lines.next().unwrap().parse().unwrap();
                            let actual = tracker.limit(
                                start + Duration::from_millis(age),
                                setting,
                                fps,
                                menu,
                                minimized,
                            );
                            assert_eq!(
                                actual, expected,
                                "fps={fps} menu={menu} setting={setting:?} age={age} minimized={minimized}"
                            );
                        }
                    }
                }
            }
        }
        assert!(lines.next().is_none());
    }
}
