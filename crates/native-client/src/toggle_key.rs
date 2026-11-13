//! Hold and toggle key state, including the state restored after screens close.

use winit::keyboard::KeyCode;

/// Physical movement keys continue to be tracked while a screen consumes input.
#[derive(Default)]
pub struct HeldMovementKeys {
    pub forward: bool,
    pub back: bool,
    pub left: bool,
    pub right: bool,
    pub jump: bool,
    sneak_left: bool,
    sneak_right: bool,
    sprint_left: bool,
    sprint_right: bool,
}

impl HeldMovementKeys {
    // Windows scan codes refer to physical keys, as do winit's KeyCode values.
    const SCAN_CODES: [(KeyCode, u32); 9] = [
        (KeyCode::KeyW, 0x11),
        (KeyCode::KeyS, 0x1f),
        (KeyCode::KeyA, 0x1e),
        (KeyCode::KeyD, 0x20),
        (KeyCode::Space, 0x39),
        (KeyCode::ShiftLeft, 0x2a),
        (KeyCode::ShiftRight, 0x36),
        (KeyCode::ControlLeft, 0x1d),
        (KeyCode::ControlRight, 0xe01d),
    ];

    pub fn update(&mut self, code: KeyCode, pressed: bool) {
        let key = match code {
            KeyCode::KeyW => &mut self.forward,
            KeyCode::KeyS => &mut self.back,
            KeyCode::KeyA => &mut self.left,
            KeyCode::KeyD => &mut self.right,
            KeyCode::Space => &mut self.jump,
            KeyCode::ShiftLeft => &mut self.sneak_left,
            KeyCode::ShiftRight => &mut self.sneak_right,
            KeyCode::ControlLeft => &mut self.sprint_left,
            KeyCode::ControlRight => &mut self.sprint_right,
            _ => return,
        };
        *key = pressed;
    }

    fn refresh_with(&mut self, focused: bool, mut query: impl FnMut(u32) -> Option<bool>) {
        if !focused {
            return;
        }
        for (key, scan_code) in Self::SCAN_CODES {
            if let Some(pressed) = query(scan_code) {
                self.update(key, pressed);
            }
        }
    }

    /// Reconcile events missed while another window had focus before grabbing the mouse.
    #[cfg(target_os = "windows")]
    pub fn refresh_system_state(&mut self, focused: bool) {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
            GetAsyncKeyState, MAPVK_VSC_TO_VK_EX, MapVirtualKeyW,
        };
        self.refresh_with(focused, |scan_code| {
            // SAFETY: These Win32 functions take integer values only and do not dereference pointers.
            let virtual_key = unsafe { MapVirtualKeyW(scan_code, MAPVK_VSC_TO_VK_EX) };
            if virtual_key == 0 {
                return None;
            }
            // SAFETY: MapVirtualKeyW returned a valid virtual-key code for this scan code.
            Some(unsafe { GetAsyncKeyState(virtual_key as i32) } < 0)
        });
    }

    #[cfg(not(target_os = "windows"))]
    pub fn refresh_system_state(&mut self, _focused: bool) {}

    pub fn restore_hold_keys(
        &self,
        sneak: &mut ToggleKey,
        sprint: &mut ToggleKey,
        toggle_sneak: bool,
        toggle_sprint: bool,
    ) {
        if !toggle_sneak {
            sneak.set(true, self.sneak_left, false, false);
            sneak.set(false, self.sneak_right, false, false);
        }
        if !toggle_sprint {
            sprint.set(true, self.sprint_left, false, false);
            sprint.set(false, self.sprint_right, false, false);
        }
    }
}

#[derive(Default)]
pub struct ToggleKey {
    left: bool,
    right: bool,
    down: bool,
    released_by_screen: bool,
}

/// Mouse mappings are never restored after a screen closes in vanilla.
#[derive(Default)]
pub struct MouseBindings {
    attack: ToggleKey,
    use_key: ToggleKey,
}

impl MouseBindings {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn attack<S: native_session::GameSession>(
        &mut self,
        session: &mut S,
        pressed: bool,
        toggle: bool,
    ) {
        use native_session::PlayerCommand;
        self.attack.set(true, pressed, false, toggle);
        if pressed {
            session.submit_command(PlayerCommand::Break);
        }
        if !self.attack.down() {
            session.submit_command(PlayerCommand::StopBreak);
        }
    }

    pub fn use_item<S: native_session::GameSession>(
        &mut self,
        session: &mut S,
        pressed: bool,
        toggle: bool,
    ) {
        use native_session::PlayerCommand;
        self.use_key.set(true, pressed, false, toggle);
        if pressed && self.use_key.down() {
            session.submit_command(PlayerCommand::UseHeld(true));
            session.submit_command(PlayerCommand::Place);
        } else if pressed && session.snapshot().first_person.item_use.is_none() {
            session.submit_command(PlayerCommand::Place);
            session.submit_command(PlayerCommand::StopUse);
        } else if !self.use_key.down() {
            session.submit_command(PlayerCommand::StopUse);
        }
    }
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
    fn focus_recapture_reconciles_physical_keys_without_reading_in_background() {
        use super::HeldMovementKeys;
        use winit::keyboard::KeyCode;

        let mut keys = HeldMovementKeys::default();
        keys.update(KeyCode::KeyW, true);
        keys.update(KeyCode::KeyA, true);
        keys.refresh_with(false, |_| panic!("must not inspect keys without focus"));
        assert!(keys.forward && keys.left);

        let mut queried = Vec::new();
        keys.refresh_with(true, |scan| {
            queried.push(scan);
            match scan {
                0x11 => Some(false), // W was released while unfocused.
                0x20 => Some(true),  // D was pressed while unfocused.
                0x1e => None,        // An unrecognized mapping retains the event state.
                _ => Some(false),
            }
        });
        assert_eq!(
            queried,
            [0x11, 0x1f, 0x1e, 0x20, 0x39, 0x2a, 0x36, 0x1d, 0xe01d]
        );
        assert!(!keys.forward && keys.left && keys.right);
    }

    #[test]
    fn movement_keys_restore_after_screen_without_retriggering_toggles() {
        use super::HeldMovementKeys;
        use crate::menu_settings::VideoSettings;
        use winit::keyboard::KeyCode;

        let mut keys = HeldMovementKeys::default();
        let mut input = crate::InputState::default();
        let settings = VideoSettings {
            toggle_sprint: true,
            ..Default::default()
        };
        input.sprint.set(true, true, false, true);
        keys.update(KeyCode::KeyW, true);
        keys.update(KeyCode::KeyA, true);
        keys.update(KeyCode::Space, true);
        keys.update(KeyCode::ShiftLeft, true);
        keys.update(KeyCode::ControlLeft, true);
        input.release_for_screen(settings);
        // The screen consumed these events, but physical state must still change.
        keys.update(KeyCode::KeyA, false);
        keys.update(KeyCode::KeyD, true);
        keys.update(KeyCode::ShiftRight, true);
        keys.update(KeyCode::ShiftLeft, false);
        input.restore_after_screen(settings);
        input.restore_held_keys(&keys, settings);
        assert!(input.forward && input.right && input.jump);
        assert!(!input.back && !input.left);
        assert!(input.sneak.down());
        assert!(input.sprint.down());
        keys.update(KeyCode::ShiftRight, false);
        keys.update(KeyCode::KeyW, false);
        input.restore_held_keys(&keys, settings);
        assert!(!input.sneak.down());
        assert!(!input.forward);
        assert!(
            input.sprint.down(),
            "restoring a held toggle must not toggle it again"
        );
    }

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
    fn mouse_toggle_keeps_use_held_until_the_next_click() {
        use native_session::{GameSession, LocalSession, PlayerCommand};
        use native_world::{Block, World};
        use std::time::Duration;
        let mut world = World::demo();
        world.set_block(glam::IVec3::new(0, 4, 1), Block::Stone);
        let mut session = LocalSession::new(world);
        let mut mouse = super::MouseBindings::default();
        session.submit_command(PlayerCommand::Select(Block::Dirt));
        mouse.use_item(&mut session, true, true);
        mouse.use_item(&mut session, false, true);
        session.update(Duration::from_millis(50));
        assert_eq!(
            session
                .snapshot()
                .world
                .demo()
                .unwrap()
                .block(glam::IVec3::new(0, 4, 2)),
            Block::Dirt
        );
        session.update(Duration::from_millis(200));
        assert_eq!(
            session
                .snapshot()
                .world
                .demo()
                .unwrap()
                .block(glam::IVec3::new(0, 4, 3)),
            Block::Dirt
        );
        mouse.use_item(&mut session, true, true);
        mouse.use_item(&mut session, false, true);
        session.update(Duration::from_millis(50));
        let revision = session.snapshot().world_revision;
        session.update(Duration::from_millis(250));
        assert_eq!(session.snapshot().world_revision, revision);
    }

    #[test]
    fn mouse_attack_toggle_keeps_mining_until_clicked_again() {
        use native_session::{GameSession, LocalSession};
        use native_world::{Block, World};
        use std::time::Duration;
        let mut world = World::demo();
        world.set_block(glam::IVec3::new(0, 4, 3), Block::Dirt);
        let mut session = LocalSession::new(world);
        let mut mouse = super::MouseBindings::default();
        mouse.attack(&mut session, true, true);
        mouse.attack(&mut session, false, true);
        session.update(Duration::from_millis(50));
        assert!(session.snapshot().breaking.is_some());
        session.update(Duration::from_millis(150));
        assert!(session.snapshot().breaking.is_some());
        mouse.attack(&mut session, true, true);
        mouse.attack(&mut session, false, true);
        session.update(Duration::from_millis(50));
        assert!(session.snapshot().breaking.is_none());
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
import com.mojang.blaze3d.platform.InputConstants;
public class ToggleOracle {
    static class Key extends ToggleKeyMapping {
        Key(AtomicBoolean toggle) { super("test.toggle", 340, KeyMapping.Category.MOVEMENT, toggle::get, true); }
        Key(AtomicBoolean toggle, boolean restore) { super("test.mouse", InputConstants.Type.MOUSE, 0, KeyMapping.Category.GAMEPLAY, toggle::get, restore); }
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
                case "mouse_attack" -> key = new Key(toggle, true);
                case "mouse_use" -> key = new Key(toggle, false);
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
            "toggle",
            "down",
            "repeat",
            "up",
            "screen",
            "screen",
            "restore",
            "down",
            "up",
            "hold",
            "down",
            "screen",
            "restore",
            "up",
            "toggle",
            "down",
            "up",
            "screen",
            "hold",
            "restore",
            "up",
            "toggle",
            "mouse_attack",
            "down",
            "up",
            "screen",
            "restore",
            "mouse_use",
            "down",
            "up",
            "screen",
            "restore",
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
        let mut mouse = false;
        for (operation, expected) in operations.into_iter().zip(expected.lines()) {
            match operation {
                "toggle" => toggle = true,
                "hold" => toggle = false,
                "down" => key.set(true, true, false, toggle),
                "up" => key.set(true, false, false, toggle),
                "repeat" => key.set(true, true, true, toggle),
                "mouse_attack" | "mouse_use" => {
                    mouse = true;
                    key = ToggleKey::default();
                }
                "screen" if mouse => key = ToggleKey::default(),
                "screen" => key.release_for_screen(toggle),
                "restore" if mouse => {}
                "restore" => key.restore_after_screen(toggle),
                _ => unreachable!(),
            }
            assert_eq!(key.down(), expected == "1", "operation={operation}");
        }
    }
}
