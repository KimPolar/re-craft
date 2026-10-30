//! Hold and toggle key state, including the state restored after screens close.

#[derive(Default)]
pub struct ToggleKey {
    left: bool,
    right: bool,
    down: bool,
    released_by_screen: bool,
}

impl ToggleKey {
    pub fn down(&self) -> bool {
        self.down
    }

    pub fn set(&mut self, left: bool, pressed: bool, repeat: bool, toggle: bool) {
        let side = if left {
            &mut self.left
        } else {
            &mut self.right
        };
        let was_pressed = *side;
        *side = pressed;
        if toggle {
            if pressed && !repeat && !was_pressed {
                self.down = !self.down;
            }
        } else {
            self.down = self.left || self.right;
        }
    }

    pub fn release_for_screen(&mut self, toggle: bool) {
        self.released_by_screen |= toggle && self.down;
        self.left = false;
        self.right = false;
        self.down = false;
    }

    pub fn restore_after_screen(&mut self, toggle: bool) {
        if self.released_by_screen {
            self.down = toggle;
        }
        self.released_by_screen = false;
    }
}

#[cfg(test)]
mod tests {
    use super::ToggleKey;

    #[test]
    fn hold_supports_both_keys_and_toggle_ignores_release_and_repeats() {
        let mut key = ToggleKey::default();
        key.set(true, true, false, false);
        key.set(false, true, false, false);
        key.set(true, false, false, false);
        assert!(key.down());
        key.set(false, false, false, false);
        assert!(!key.down());
        key.set(true, true, false, true);
        key.set(true, true, true, true);
        key.set(true, false, false, true);
        assert!(key.down());
        key.set(true, true, false, true);
        assert!(!key.down());
    }

    #[test]
    fn screen_release_restores_only_enabled_toggle_state() {
        let mut key = ToggleKey::default();
        key.set(true, true, false, true);
        key.set(true, false, false, true);
        key.release_for_screen(true);
        key.release_for_screen(true);
        assert!(!key.down());
        key.restore_after_screen(true);
        assert!(key.down());
        key.release_for_screen(true);
        key.restore_after_screen(false);
        assert!(!key.down());
        key.set(true, true, false, false);
        key.release_for_screen(false);
        key.restore_after_screen(false);
        assert!(!key.down());
    }

    #[test]
    fn toggled_sneak_is_submitted_and_restored_in_demo() {
        use native_session::{GameSession, LocalSession, PlayerPose};
        use std::time::Duration;
        let settings = crate::menu_settings::VideoSettings {
            toggle_sneak: true,
            ..Default::default()
        };
        let mut input = crate::InputState::default();
        let mut session = LocalSession::demo();
        input.sneak.set(true, true, false, true);
        input.sneak.set(true, false, false, true);
        input.submit(&mut session);
        session.update(Duration::from_millis(50));
        assert_eq!(session.snapshot().player.pose, PlayerPose::Crouching);
        input.release_for_screen(settings);
        input.submit(&mut session);
        session.update(Duration::from_millis(50));
        assert_eq!(session.snapshot().player.pose, PlayerPose::Standing);
        input.restore_after_screen(settings);
        input.submit(&mut session);
        session.update(Duration::from_millis(50));
        assert_eq!(session.snapshot().player.pose, PlayerPose::Crouching);
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn toggle_mapping_matches_installed_java() {
        use std::{ffi::OsString, path::PathBuf, process::Command};
        const JAVA: &str = r#"
import java.nio.file.*;
import java.util.concurrent.atomic.AtomicBoolean;
import net.minecraft.client.ToggleKeyMapping;
import net.minecraft.client.KeyMapping;
public class ToggleOracle {
    static class Key extends ToggleKeyMapping {
        Key(AtomicBoolean toggle) { super("test.toggle", 340, KeyMapping.Category.MOVEMENT, toggle::get, true); }
        void screen() { release(); }
    }
    public static void main(String[] args) throws Exception {
        AtomicBoolean toggle = new AtomicBoolean(false);
        Key key = new Key(toggle);
        StringBuilder result = new StringBuilder();
        for (String line : Files.readAllLines(Path.of(args[0]))) {
            switch (line) {
                case "toggle" -> toggle.set(true);
                case "hold" -> toggle.set(false);
                case "down" -> key.setDown(true);
                case "up" -> key.setDown(false);
                case "repeat" -> { }
                case "screen" -> key.screen();
                case "restore" -> { if (key.shouldRestoreStateOnScreenClosed()) key.setDown(true); }
                default -> throw new AssertionError(line);
            }
            result.append(key.isDown() ? '1' : '0').append('\n');
        }
        Files.writeString(Path.of(args[1]), result);
    }
}
"#;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let version = game.join("versions/26.3");
        let metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("ToggleOracle.java");
        let input = temp.path().join("input.txt");
        let output = temp.path().join("output.txt");
        let operations = [
            "toggle", "down", "repeat", "up", "screen", "screen", "restore", "down", "up", "hold",
            "down", "screen", "restore", "up", "toggle", "down", "up", "screen", "hold", "restore",
            "up",
        ];
        std::fs::write(&source, JAVA).unwrap();
        std::fs::write(&input, operations.join("\n")).unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in metadata["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let classpath: OsString = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let run = |command: &mut Command| {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "Java toggle oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source));
        run(Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("ToggleOracle")
            .arg(&input)
            .arg(&output));
        let expected = std::fs::read_to_string(output).unwrap();
        let mut key = ToggleKey::default();
        let mut toggle = false;
        for (operation, expected) in operations.into_iter().zip(expected.lines()) {
            match operation {
                "toggle" => toggle = true,
                "hold" => toggle = false,
                "down" => key.set(true, true, false, toggle),
                "up" => key.set(true, false, false, toggle),
                "repeat" => key.set(true, true, true, toggle),
                "screen" => key.release_for_screen(toggle),
                "restore" => key.restore_after_screen(toggle),
                _ => unreachable!(),
            }
            assert_eq!(key.down(), expected == "1", "operation={operation}");
        }
    }
}
