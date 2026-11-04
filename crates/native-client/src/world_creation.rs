//! Settings edited on the Java Edition create-world screen.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GameMode {
    Survival,
    Hardcore,
    Creative,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Difficulty {
    Peaceful,
    Easy,
    Normal,
    Hard,
}

/// Keep the chosen values even while hardcore temporarily overrides them.
/// This state does not create or save a world.
#[derive(Debug)]
pub struct WorldCreationSettings {
    pub name: crate::chat_input::ChatInput,
    pub seed: crate::chat_input::ChatInput,
    pub mode: GameMode,
    pub difficulty: Difficulty,
    pub allow_commands: Option<bool>,
    pub generate_structures: bool,
    pub bonus_chest: bool,
}

impl WorldCreationSettings {
    pub fn new(name: String) -> Self {
        Self {
            name: crate::chat_input::ChatInput::field(&name),
            seed: crate::chat_input::ChatInput::field(""),
            mode: GameMode::Survival,
            difficulty: Difficulty::Normal,
            allow_commands: None,
            generate_structures: true,
            bonus_chest: false,
        }
    }

    pub fn parsed_seed(&self) -> Option<i64> {
        native_core::world_seed::parse_seed(self.seed.value())
    }

    pub fn effective_difficulty(&self) -> Difficulty {
        if self.mode == GameMode::Hardcore {
            Difficulty::Hard
        } else {
            self.difficulty
        }
    }

    pub fn effective_allow_commands(&self) -> bool {
        if self.mode == GameMode::Hardcore {
            false
        } else {
            self.allow_commands
                .unwrap_or(self.mode == GameMode::Creative)
        }
    }

    pub fn effective_bonus_chest(&self) -> bool {
        self.mode != GameMode::Hardcore && self.bonus_chest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardcore_overrides_but_does_not_erase_previous_settings() {
        let mut state = WorldCreationSettings::new("New World".into());
        state.difficulty = Difficulty::Easy;
        state.bonus_chest = true;
        state.allow_commands = Some(true);
        state.mode = GameMode::Hardcore;
        assert_eq!(state.effective_difficulty(), Difficulty::Hard);
        assert!(!state.effective_allow_commands());
        assert!(!state.effective_bonus_chest());
        state.mode = GameMode::Survival;
        assert_eq!(state.effective_difficulty(), Difficulty::Easy);
        assert!(state.effective_allow_commands());
        assert!(state.effective_bonus_chest());
    }

    #[test]
    fn creative_commands_default_depends_on_an_explicit_override() {
        let mut state = WorldCreationSettings::new("New World".into());
        assert!(!state.effective_allow_commands());
        state.mode = GameMode::Creative;
        assert!(state.effective_allow_commands());
        state.allow_commands = Some(false);
        assert!(!state.effective_allow_commands());
        state.seed.insert_limited(" １２3 ", 32);
        assert_eq!(state.parsed_seed(), Some(123));
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
    fn game_settings_follow_java_world_creation_state() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
            .join("bin");
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        assert_eq!(manifest["id"], "26.3");
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let classpath = std::env::join_paths(paths).unwrap();
        let source = temp.path().join("WorldCreationStateOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/WorldCreationStateOracle.java"),
        )
        .unwrap();
        let compile = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(source)
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let result_path = temp.path().join("results.txt");
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("WorldCreationStateOracle")
            .arg(&result_path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let java_rows: Vec<_> = std::fs::read_to_string(result_path)
            .unwrap()
            .lines()
            .filter(|line| line.starts_with("CASE "))
            .map(str::to_owned)
            .collect();
        let mut rows = Vec::new();
        let capture = |state: &WorldCreationSettings| {
            format!(
                "CASE {} {} {} {}",
                format!("{:?}", state.effective_difficulty()).to_uppercase(),
                state.effective_allow_commands(),
                state.effective_bonus_chest(),
                state.generate_structures
            )
        };
        for mode in [GameMode::Survival, GameMode::Hardcore, GameMode::Creative] {
            for difficulty in [
                Difficulty::Peaceful,
                Difficulty::Easy,
                Difficulty::Normal,
                Difficulty::Hard,
            ] {
                for commands in [None, Some(false), Some(true)] {
                    for bonus in [false, true] {
                        for structures in [false, true] {
                            let mut state = WorldCreationSettings::new("New World".into());
                            state.mode = mode;
                            state.difficulty = difficulty;
                            state.allow_commands = commands;
                            state.bonus_chest = bonus;
                            state.generate_structures = structures;
                            rows.push(capture(&state));
                            state.mode = GameMode::Hardcore;
                            rows.push(capture(&state));
                            state.mode = GameMode::Creative;
                            rows.push(capture(&state));
                        }
                    }
                }
            }
        }
        assert_eq!(rows.len(), 432);
        assert_eq!(rows.len(), java_rows.len());
        for (index, (actual, expected)) in rows.iter().zip(&java_rows).enumerate() {
            assert_eq!(actual, expected, "world creation state case {index}");
        }
    }
}
