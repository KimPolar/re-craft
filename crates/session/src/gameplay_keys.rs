//! Client key-mapping clicks consumed by Minecraft's game tick.

use std::collections::VecDeque;

use crate::{GameplayKey, PlayerCommand};

#[derive(Default)]
pub(crate) struct GameplayKeys {
    hotbar: [u32; 9],
    swap: u32,
    drop: u32,
    control: bool,
}

impl GameplayKeys {
    pub fn clear(&mut self) {
        self.hotbar = [0; 9];
        self.swap = 0;
        self.drop = 0;
    }

    fn pending(&self) -> bool {
        self.swap != 0 || self.drop != 0 || self.hotbar.iter().any(|count| *count != 0)
    }

    pub fn prepare(&mut self, commands: &mut VecDeque<PlayerCommand>) {
        if !self.pending()
            && !commands.iter().any(|command| {
                matches!(
                    command,
                    PlayerCommand::GameplayKey(_)
                        | PlayerCommand::GameplayControl(_)
                        | PlayerCommand::ClearGameplayKeys
                )
            })
        {
            return;
        }
        let mut other = VecDeque::with_capacity(commands.len());
        let mut position = None;
        let mut mouse_position = None;
        for command in commands.drain(..) {
            match command {
                PlayerCommand::GameplayKey(key) => {
                    position.get_or_insert(other.len());
                    match key {
                        GameplayKey::Hotbar(slot) => {
                            if let Some(count) = self.hotbar.get_mut(usize::from(slot)) {
                                *count = count.saturating_add(1);
                            }
                        }
                        GameplayKey::SwapHands => self.swap = self.swap.saturating_add(1),
                        GameplayKey::Drop => self.drop = self.drop.saturating_add(1),
                    }
                }
                PlayerCommand::GameplayControl(control) => self.control = control,
                PlayerCommand::ClearGameplayKeys => {
                    self.clear();
                    position = None;
                    mouse_position = None;
                }
                command => {
                    if matches!(
                        command,
                        PlayerCommand::Break
                            | PlayerCommand::StopBreak
                            | PlayerCommand::UseHeld(_)
                            | PlayerCommand::Place
                            | PlayerCommand::StopUse
                            | PlayerCommand::PickBlock
                    ) {
                        mouse_position.get_or_insert(other.len());
                    }
                    other.push_back(command);
                }
            }
        }
        let mut index = match (position, mouse_position) {
            (Some(key), Some(mouse)) => key.min(mouse),
            (Some(key), None) => key,
            (None, Some(mouse)) => mouse,
            (None, None) => 0,
        };
        for (slot, count) in self.hotbar.iter_mut().enumerate() {
            if *count > 0 {
                *count -= 1;
                other.insert(index, PlayerCommand::SelectSlot(slot as u8));
                index += 1;
            }
        }
        for _ in 0..std::mem::take(&mut self.swap) {
            other.insert(index, PlayerCommand::SwapHands);
            index += 1;
        }
        for _ in 0..std::mem::take(&mut self.drop) {
            other.insert(index, PlayerCommand::DropSelected { all: self.control });
            index += 1;
        }
        *commands = other;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GameSession, LocalSession, inventory::PlayerSlot};
    use std::time::Duration;

    #[test]
    fn consumes_hotbar_once_per_tick_before_swapping_and_dropping() {
        let mut keys = GameplayKeys::default();
        let mut commands = VecDeque::from([
            PlayerCommand::GameplayKey(GameplayKey::Drop),
            PlayerCommand::GameplayKey(GameplayKey::SwapHands),
            PlayerCommand::GameplayKey(GameplayKey::Hotbar(7)),
            PlayerCommand::GameplayKey(GameplayKey::Hotbar(1)),
            PlayerCommand::GameplayKey(GameplayKey::Hotbar(1)),
            PlayerCommand::GameplayControl(true),
        ]);
        keys.prepare(&mut commands);
        assert!(matches!(
            commands.pop_front(),
            Some(PlayerCommand::SelectSlot(1))
        ));
        assert!(matches!(
            commands.pop_front(),
            Some(PlayerCommand::SelectSlot(7))
        ));
        assert!(matches!(
            commands.pop_front(),
            Some(PlayerCommand::SwapHands)
        ));
        assert!(matches!(
            commands.pop_front(),
            Some(PlayerCommand::DropSelected { all: true })
        ));
        assert!(commands.is_empty());
        keys.prepare(&mut commands);
        assert!(matches!(
            commands.pop_front(),
            Some(PlayerCommand::SelectSlot(1))
        ));
        assert!(commands.is_empty());
    }

    #[test]
    fn screen_release_discards_clicks_and_control_is_sampled_on_tick() {
        let mut keys = GameplayKeys::default();
        let mut commands = VecDeque::from([
            PlayerCommand::GameplayKey(GameplayKey::Hotbar(4)),
            PlayerCommand::GameplayKey(GameplayKey::Drop),
            PlayerCommand::ClearGameplayKeys,
            PlayerCommand::GameplayKey(GameplayKey::Drop),
            PlayerCommand::GameplayControl(true),
            PlayerCommand::GameplayControl(false),
        ]);
        keys.prepare(&mut commands);
        assert!(matches!(
            commands.pop_front(),
            Some(PlayerCommand::DropSelected { all: false })
        ));
        assert!(commands.is_empty());
    }

    #[test]
    fn keybinds_precede_mouse_actions_even_if_the_mouse_arrived_first() {
        let mut keys = GameplayKeys::default();
        let mut commands = VecDeque::from([
            PlayerCommand::Look {
                yaw: 0.25,
                pitch: 0.0,
            },
            PlayerCommand::UseHeld(true),
            PlayerCommand::Place,
            PlayerCommand::GameplayKey(GameplayKey::Hotbar(1)),
            PlayerCommand::GameplayKey(GameplayKey::SwapHands),
        ]);
        keys.prepare(&mut commands);
        assert!(matches!(
            commands.pop_front(),
            Some(PlayerCommand::Look { .. })
        ));
        assert!(matches!(
            commands.pop_front(),
            Some(PlayerCommand::SelectSlot(1))
        ));
        assert!(matches!(
            commands.pop_front(),
            Some(PlayerCommand::SwapHands)
        ));
        assert!(matches!(
            commands.pop_front(),
            Some(PlayerCommand::UseHeld(true))
        ));
        assert!(matches!(commands.pop_front(), Some(PlayerCommand::Place)));
    }

    #[test]
    fn hotbar_selection_affects_an_earlier_mouse_placement() {
        use glam::IVec3;
        use native_world::{Block, World};
        let mut world = World::demo();
        world.set_block(IVec3::new(0, 4, 3), Block::Stone);
        let mut session = LocalSession::new(world);
        session.submit_command(PlayerCommand::Place);
        session.submit_command(PlayerCommand::GameplayKey(GameplayKey::Hotbar(1)));
        session.update(Duration::from_millis(50));
        assert_eq!(
            session
                .snapshot()
                .world
                .demo()
                .unwrap()
                .block(IVec3::new(0, 4, 4)),
            Block::Dirt
        );
    }

    #[test]
    fn key_order_is_consistent_across_frame_cadences() {
        let mut slow = LocalSession::demo();
        let mut fast = LocalSession::demo();
        for session in [&mut slow, &mut fast] {
            session.submit_command(PlayerCommand::GameplayKey(GameplayKey::Drop));
            session.submit_command(PlayerCommand::GameplayKey(GameplayKey::SwapHands));
            session.submit_command(PlayerCommand::GameplayKey(GameplayKey::Hotbar(0)));
            session.submit_command(PlayerCommand::GameplayKey(GameplayKey::Hotbar(1)));
            session.submit_command(PlayerCommand::GameplayKey(GameplayKey::Hotbar(0)));
        }
        slow.update(Duration::from_millis(50));
        fast.update(Duration::from_millis(10));
        fast.update(Duration::from_millis(40));
        assert_eq!(slow.snapshot().inventory, fast.snapshot().inventory);
        assert_eq!(slow.snapshot().inventory.selected(), 1);
        assert!(
            slow.snapshot()
                .inventory
                .slot(PlayerSlot::Offhand)
                .is_some()
        );
        assert!(slow.snapshot().dropped_items.is_empty());
        slow.update(Duration::from_millis(50));
        fast.update(Duration::from_millis(50));
        assert_eq!(slow.snapshot().inventory.selected(), 0);
        assert_eq!(slow.snapshot().inventory, fast.snapshot().inventory);
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn click_order_matches_installed_java_key_mappings() {
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
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
            .join("bin");
        let compile = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("-d")
            .arg(temp.path())
            .arg(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/java/GameplayKeysOracle.java"),
            )
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("GameplayKeysOracle")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );

        let mut keys = GameplayKeys::default();
        let mut commands = VecDeque::from([
            PlayerCommand::GameplayKey(GameplayKey::Drop),
            PlayerCommand::GameplayKey(GameplayKey::SwapHands),
            PlayerCommand::GameplayKey(GameplayKey::Hotbar(1)),
            PlayerCommand::GameplayKey(GameplayKey::Hotbar(0)),
            PlayerCommand::GameplayKey(GameplayKey::Hotbar(0)),
        ]);
        let mut actual = String::new();
        for tick in 0..3 {
            if tick == 2 {
                commands.extend([
                    PlayerCommand::GameplayKey(GameplayKey::Hotbar(4)),
                    PlayerCommand::GameplayKey(GameplayKey::Drop),
                    PlayerCommand::ClearGameplayKeys,
                    PlayerCommand::GameplayKey(GameplayKey::Drop),
                ]);
            }
            keys.prepare(&mut commands);
            for command in commands.drain(..) {
                match command {
                    PlayerCommand::SelectSlot(slot) => actual.push_str(&format!("H{slot},")),
                    PlayerCommand::SwapHands => actual.push_str("S,"),
                    PlayerCommand::DropSelected { .. } => actual.push_str("D,"),
                    _ => panic!("unexpected command"),
                }
            }
            actual.push('\n');
        }
        assert_eq!(
            actual,
            String::from_utf8(output.stdout)
                .unwrap()
                .replace("\r\n", "\n")
        );
    }
}
