//! Settings edited on the Java Edition create-world screen.

use std::{path::PathBuf, sync::mpsc};

/// A single I/O worker coalesces name changes and rejects obsolete results.
/// Dropping the sender lets the worker exit without blocking the window.
pub struct FolderProposalWorker {
    requests: mpsc::Sender<(u64, String)>,
    results: mpsc::Receiver<(u64, Result<String, String>)>,
    requested_name: Option<String>,
    revision: u64,
    pending: bool,
    result: Option<Result<String, String>>,
}

impl FolderProposalWorker {
    pub fn new(saves: PathBuf, default_name: String) -> std::io::Result<Self> {
        Self::spawn(move |name| {
            native_persistence::world_creation::propose_world_folder(&saves, name, &default_name)
                .map_err(|error| format!("{error:#}"))
        })
    }

    fn spawn(
        mut propose: impl FnMut(&str) -> Result<String, String> + Send + 'static,
    ) -> std::io::Result<Self> {
        let (requests, incoming) = mpsc::channel::<(u64, String)>();
        let (outgoing, results) = mpsc::channel();
        std::thread::Builder::new()
            .name("world-folder-proposal".into())
            .spawn(move || {
                while let Ok(mut request) = incoming.recv() {
                    for newer in incoming.try_iter() {
                        request = newer;
                    }
                    let result = propose(&request.1);
                    if outgoing.send((request.0, result)).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            requests,
            results,
            requested_name: None,
            revision: 0,
            pending: false,
            result: None,
        })
    }

    pub fn request(&mut self, name: &str) {
        if self.requested_name.as_deref() == Some(name) {
            return;
        }
        self.revision = self.revision.wrapping_add(1);
        self.requested_name = Some(name.to_owned());
        self.result = None;
        match self.requests.send((self.revision, name.to_owned())) {
            Ok(()) => self.pending = true,
            Err(_) => {
                self.pending = false;
                self.result = Some(Err("World folder worker stopped".into()));
            }
        }
    }

    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        loop {
            match self.results.try_recv() {
                Ok((revision, result)) if revision == self.revision => {
                    self.result = Some(result);
                    self.pending = false;
                    changed = true;
                }
                Ok(_) => {}
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if self.pending {
                        self.pending = false;
                        self.result = Some(Err("World folder worker stopped".into()));
                        changed = true;
                    }
                    break;
                }
            }
        }
        changed
    }

    pub fn pending(&self) -> bool {
        self.pending
    }
    pub fn result_for(&self, name: &str) -> Option<&Result<String, String>> {
        (self.requested_name.as_deref() == Some(name))
            .then_some(self.result.as_ref())
            .flatten()
    }
}

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
    fn folder_worker_coalesces_requests_and_discards_stale_results() {
        use std::time::{Duration, Instant};
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let mut calls = 0;
        let mut worker = FolderProposalWorker::spawn(move |name| {
            calls += 1;
            if calls == 1 {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            }
            Ok(format!("{name}-{calls}"))
        })
        .unwrap();
        worker.request("A");
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.request("B");
        worker.request("A");
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while worker.pending() && Instant::now() < deadline {
            worker.poll();
            std::thread::yield_now();
        }
        assert!(!worker.pending());
        assert_eq!(worker.result_for("A"), Some(&Ok("A-2".to_owned())));
        assert_eq!(worker.result_for("B"), None);
    }

    #[test]
    fn folder_worker_checks_real_collisions_without_creating_a_world() {
        use std::time::{Duration, Instant};
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("New World")).unwrap();
        let mut worker =
            FolderProposalWorker::new(dir.path().to_owned(), "New World".into()).unwrap();
        worker.request("New World");
        let deadline = Instant::now() + Duration::from_secs(2);
        while worker.pending() && Instant::now() < deadline {
            worker.poll();
            std::thread::yield_now();
        }
        assert_eq!(
            worker.result_for("New World"),
            Some(&Ok("New World (1)".to_owned()))
        );
        assert!(!dir.path().join("New World (1)").exists());
    }

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
