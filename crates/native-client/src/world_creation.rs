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

/// Values fixed when world creation starts and carried into its first session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InitialSessionSettings {
    pub seed: i64,
    pub mode: native_session::GameMode,
    pub difficulty: native_persistence::world::Difficulty,
    pub hardcore: bool,
    pub allow_commands: bool,
}

pub struct FrozenWorldCreation {
    pub generation: crate::fresh_generation::FreshGenerationSettings,
    pub level: FrozenLevelSettings,
}

impl FrozenWorldCreation {
    /// Reject settings mixed from different create-world selections.
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.generation.seed == self.level.session.seed,
            "session seed differs from the generation seed"
        );
        let same_packs = match (&self.generation.data_packs, &self.level.packs) {
            (None, None) => true,
            (Some((_, a)), Some(b)) => {
                a.enabled == b.enabled
                    && a.disabled == b.disabled
                    && a.enabled_features == b.enabled_features
            }
            _ => false,
        };
        anyhow::ensure!(
            same_packs,
            "level data packs differ from the generation selection"
        );
        Ok(())
    }
}

/// Metadata retained after the generator is moved to a background worker.
#[derive(Clone, Debug)]
pub struct FrozenLevelSettings {
    pub name: String,
    pub session: InitialSessionSettings,
    pub packs: Option<native_resources::datapacks::DataPackSettings>,
}

impl FrozenLevelSettings {
    /// Create an in-memory draft only; this does not create a playable world.
    pub fn draft_level_data(
        &self,
        spawn: &native_persistence::world::RespawnData,
        last_played_ms: i64,
        singleplayer_uuid: Option<[i32; 4]>,
        initialized: bool,
    ) -> anyhow::Result<native_persistence::nbt::NamedTag> {
        use native_persistence::{
            level_data::{
                NewLevelData, NewWorldDifficulty, NewWorldGameType, draft_new_level_data,
            },
            world::Difficulty as SessionDifficulty,
        };
        let game_type = match self.session.mode {
            native_session::GameMode::Survival => NewWorldGameType::Survival,
            native_session::GameMode::Creative => NewWorldGameType::Creative,
            native_session::GameMode::Adventure => NewWorldGameType::Adventure,
            native_session::GameMode::Spectator => NewWorldGameType::Spectator,
        };
        let difficulty = match self.session.difficulty {
            SessionDifficulty::Peaceful => NewWorldDifficulty::Peaceful,
            SessionDifficulty::Easy => NewWorldDifficulty::Easy,
            SessionDifficulty::Normal => NewWorldDifficulty::Normal,
            SessionDifficulty::Hard => NewWorldDifficulty::Hard,
        };
        draft_new_level_data(&NewLevelData {
            name: &self.name,
            game_type,
            difficulty,
            hardcore: self.session.hardcore,
            difficulty_locked: false,
            allow_commands: self.session.allow_commands,
            spawn,
            game_time: 0,
            initialized,
            last_played_ms,
            singleplayer_uuid,
            packs: self.packs.as_ref(),
            server_brands: &[],
            was_modded: false,
        })
    }
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
    pub preset: native_resources::packs::ResourceId,
    preset_index: usize,
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
            preset: native_resources::packs::ResourceId::parse("minecraft:normal")
                .expect("built-in preset"),
            preset_index: 0,
        }
    }

    pub fn parsed_seed(&self) -> Option<i64> {
        native_core::world_seed::parse_seed(self.seed.value())
    }

    /// Call once when committing world creation; an empty field chooses a seed.
    pub fn resolve_seed(&self) -> i64 {
        native_core::world_seed::resolve_seed(self.seed.value())
    }

    pub fn effective_difficulty(&self) -> Difficulty {
        if self.is_hardcore() {
            Difficulty::Hard
        } else {
            self.difficulty
        }
    }

    pub fn effective_allow_commands(&self) -> bool {
        if self.is_debug() {
            true
        } else if self.is_hardcore() {
            false
        } else {
            self.allow_commands
                .unwrap_or(self.mode == GameMode::Creative)
        }
    }

    pub fn effective_bonus_chest(&self) -> bool {
        !self.is_debug() && !self.is_hardcore() && self.bonus_chest
    }

    pub fn is_debug(&self) -> bool {
        self.preset.namespace == "minecraft" && self.preset.path == "debug_all_block_states"
    }

    pub fn is_hardcore(&self) -> bool {
        !self.is_debug() && self.mode == GameMode::Hardcore
    }

    pub fn effective_generate_structures(&self) -> bool {
        !self.is_debug() && self.generate_structures
    }

    pub fn align_preset_index(&mut self, presets: &native_resources::world_presets::WorldPresets) {
        if let Some(index) = presets
            .normal
            .iter()
            .position(|preset| preset == &self.preset)
        {
            self.preset_index = index;
        }
    }

    /// CycleButton retains its index when Alt switches between the normal and
    /// extended tag lists; Shift reverses the direction.
    pub fn cycle_preset(
        &mut self,
        presets: &native_resources::world_presets::WorldPresets,
        alt: bool,
        reverse: bool,
    ) {
        let values = if alt {
            &presets.alternate
        } else {
            &presets.normal
        };
        if values.is_empty() {
            return;
        }
        self.preset_index = if reverse {
            (self.preset_index + values.len() - 1) % values.len()
        } else {
            (self.preset_index + 1) % values.len()
        };
        self.preset = values[self.preset_index].clone();
    }

    /// Freeze screen settings once when starting the background generator.
    pub fn fresh_generation_settings(
        &self,
        minecraft_dir: PathBuf,
        data_packs: Option<(PathBuf, native_resources::datapacks::DataPackSettings)>,
    ) -> crate::fresh_generation::FreshGenerationSettings {
        self.freeze(minecraft_dir, data_packs).generation
    }

    /// Resolve a random seed exactly once and preserve the effective screen
    /// settings even if the screen is subsequently edited or closed.
    pub fn freeze(
        &self,
        minecraft_dir: PathBuf,
        data_packs: Option<(PathBuf, native_resources::datapacks::DataPackSettings)>,
    ) -> FrozenWorldCreation {
        use native_persistence::world::Difficulty as SessionDifficulty;
        let seed = self.resolve_seed();
        let mode = if self.is_debug() {
            native_session::GameMode::Spectator
        } else {
            match self.mode {
                GameMode::Survival | GameMode::Hardcore => native_session::GameMode::Survival,
                GameMode::Creative => native_session::GameMode::Creative,
            }
        };
        let difficulty = match self.effective_difficulty() {
            Difficulty::Peaceful => SessionDifficulty::Peaceful,
            Difficulty::Easy => SessionDifficulty::Easy,
            Difficulty::Normal => SessionDifficulty::Normal,
            Difficulty::Hard => SessionDifficulty::Hard,
        };
        let packs = data_packs.as_ref().map(|(_, packs)| packs.clone());
        let generation = crate::fresh_generation::FreshGenerationSettings {
            minecraft_dir,
            preset: format!("{}:{}", self.preset.namespace, self.preset.path),
            dimension: "minecraft:overworld".into(),
            seed,
            bonus_chest: self.effective_bonus_chest(),
            structures_enabled: self.effective_generate_structures(),
            data_packs,
        };
        let session = InitialSessionSettings {
            seed,
            mode,
            difficulty,
            hardcore: self.is_hardcore(),
            allow_commands: self.effective_allow_commands(),
        };
        FrozenWorldCreation {
            generation,
            level: FrozenLevelSettings {
                name: self.name.value().to_owned(),
                session,
                packs,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_generation_rejects_mixed_seed_and_pack_selection() {
        use native_resources::datapacks::DataPackSettings;
        let state = WorldCreationSettings::new("World".into());
        let packs = DataPackSettings {
            enabled: vec!["vanilla".into(), "file/test".into()],
            disabled: vec![],
            enabled_features: vec!["minecraft:vanilla".into()],
        };
        let mut frozen = state.freeze(
            PathBuf::from("game"),
            Some((PathBuf::from("source"), packs)),
        );
        frozen.validate().unwrap();
        frozen.level.session.seed ^= 1;
        assert!(frozen.validate().is_err());
        frozen.level.session.seed = frozen.generation.seed;
        frozen.level.packs.as_mut().unwrap().enabled.reverse();
        assert!(frozen.validate().is_err());
        frozen.level.packs = None;
        assert!(frozen.validate().is_err());
    }

    #[test]
    fn preset_cycle_keeps_options_and_freezes_selected_generation_inputs() {
        use native_persistence::nbt::Tag;
        use native_resources::{packs::ResourceId, world_presets::WorldPresets};
        let ids = |items: &[&str]| {
            items
                .iter()
                .map(|id| ResourceId::parse(id).unwrap())
                .collect::<Vec<_>>()
        };
        let presets = WorldPresets {
            normal: ids(&[
                "normal",
                "flat",
                "large_biomes",
                "amplified",
                "single_biome_surface",
            ]),
            alternate: ids(&[
                "normal",
                "flat",
                "large_biomes",
                "amplified",
                "single_biome_surface",
                "debug_all_block_states",
            ]),
        };
        let mut state = WorldCreationSettings::new("World".into());
        state.mode = GameMode::Hardcore;
        state.difficulty = Difficulty::Easy;
        state.allow_commands = Some(false);
        state.generate_structures = true;
        state.bonus_chest = true;
        state.seed = crate::chat_input::ChatInput::field("-123");
        state.cycle_preset(&presets, false, false);
        assert_eq!(state.preset, ResourceId::parse("flat").unwrap());
        state.cycle_preset(&presets, false, true);
        assert_eq!(state.preset, ResourceId::parse("normal").unwrap());
        state.cycle_preset(&presets, true, true);
        assert!(state.is_debug());
        assert!(!state.is_hardcore());
        assert_eq!(state.effective_difficulty(), Difficulty::Easy);
        assert!(state.effective_allow_commands());
        assert!(!state.effective_bonus_chest());
        assert!(!state.effective_generate_structures());
        let data = state
            .freeze(PathBuf::from("game"), None)
            .level
            .draft_level_data(
                &native_persistence::world::RespawnData::default(),
                0,
                None,
                false,
            )
            .unwrap();
        let data = data.value.get("Data").unwrap();
        assert_eq!(data.get("GameType"), Some(&Tag::Int(3)));
        assert_eq!(data.get("allowCommands"), Some(&Tag::Byte(1)));
        assert_eq!(
            data.get("difficulty_settings").unwrap().get("hardcore"),
            Some(&Tag::Byte(0))
        );
        let inputs = state.fresh_generation_settings(PathBuf::from("game"), None);
        assert_eq!(inputs.preset, "minecraft:debug_all_block_states");
        assert_eq!(inputs.seed, -123);
        assert!(!inputs.structures_enabled && !inputs.bonus_chest);
        state.cycle_preset(&presets, false, false);
        assert_eq!(state.preset, ResourceId::parse("flat").unwrap());
        assert!(state.is_hardcore());
        assert_eq!(state.effective_difficulty(), Difficulty::Hard);
        assert!(!state.effective_allow_commands());
        assert!(state.generate_structures && state.bonus_chest);
    }

    #[test]
    fn entered_seed_is_used_when_resolving_world_creation() {
        let mut settings = WorldCreationSettings::new("World".into());
        settings.seed = crate::chat_input::ChatInput::field(" -123 ");
        assert_eq!(settings.resolve_seed(), -123);
        settings.seed = crate::chat_input::ChatInput::field("abc");
        assert_eq!(settings.resolve_seed(), 96354);
    }

    #[test]
    fn frozen_session_and_generator_share_seed_and_effective_settings() {
        use native_persistence::world::Difficulty as SessionDifficulty;
        let mut settings = WorldCreationSettings::new("World".into());
        settings.mode = GameMode::Hardcore;
        settings.difficulty = Difficulty::Easy;
        settings.allow_commands = Some(true);
        let frozen = settings.freeze(PathBuf::from("game"), None);
        assert_eq!(frozen.generation.seed, frozen.level.session.seed);
        assert_eq!(
            frozen.level.session.mode,
            native_session::GameMode::Survival
        );
        assert_eq!(frozen.level.session.difficulty, SessionDifficulty::Hard);
        assert!(frozen.level.session.hardcore);
        assert!(!frozen.level.session.allow_commands);
        settings.mode = GameMode::Creative;
        assert!(frozen.level.session.hardcore);
        let frozen = settings.freeze(PathBuf::from("game"), None);
        assert_eq!(
            frozen.level.session.mode,
            native_session::GameMode::Creative
        );
        assert_eq!(frozen.level.session.difficulty, SessionDifficulty::Easy);
        assert!(!frozen.level.session.hardcore);
        assert!(frozen.level.session.allow_commands);
    }

    #[test]
    fn frozen_level_keeps_original_screen_and_pack_selection() {
        use native_persistence::{nbt::Tag, world::RespawnData};
        use native_resources::datapacks::DataPackSettings;
        let mut settings = WorldCreationSettings::new(" First World ".into());
        settings.mode = GameMode::Creative;
        settings.difficulty = Difficulty::Easy;
        let packs = DataPackSettings {
            enabled: vec!["vanilla".into(), "file/custom".into()],
            disabled: vec!["file/disabled".into()],
            enabled_features: vec!["minecraft:vanilla".into()],
        };
        let frozen = settings.freeze(PathBuf::from("game"), Some((PathBuf::from("world"), packs)));
        settings.name = crate::chat_input::ChatInput::field("Changed");
        settings.mode = GameMode::Hardcore;
        settings.difficulty = Difficulty::Hard;
        let document = frozen
            .level
            .draft_level_data(&RespawnData::default(), 123, None, true)
            .unwrap();
        let data = document.value.get("Data").unwrap();
        assert_eq!(
            data.get("LevelName"),
            Some(&Tag::String("First World".into()))
        );
        assert_eq!(data.get("GameType"), Some(&Tag::Int(1)));
        assert_eq!(data.get("allowCommands"), Some(&Tag::Byte(1)));
        assert_eq!(
            data.get("difficulty_settings").unwrap().get("difficulty"),
            Some(&Tag::String("easy".into()))
        );
        let data_packs = data.get("DataPacks").unwrap();
        assert!(
            matches!(data_packs.get("Enabled"), Some(Tag::List { values, .. }) if values == &vec![Tag::String("vanilla".into()), Tag::String("file/custom".into())])
        );
        assert!(
            matches!(data_packs.get("Disabled"), Some(Tag::List { values, .. }) if values == &vec![Tag::String("file/disabled".into())])
        );
        assert_eq!(
            native_persistence::nbt::read(&native_persistence::nbt::write(&document).unwrap())
                .unwrap(),
            document
        );
    }

    #[test]
    fn level_metadata_applies_game_mode_overrides() {
        use native_persistence::{nbt::Tag, world::RespawnData};
        let mut state = WorldCreationSettings::new("  Test  ".into());
        state.mode = GameMode::Hardcore;
        state.difficulty = Difficulty::Easy;
        state.allow_commands = Some(true);
        let spawn = RespawnData::default();
        let metadata = state
            .freeze(PathBuf::from("game"), None)
            .level
            .draft_level_data(&spawn, 123, None, false)
            .unwrap();
        let data = metadata.value.get("Data").unwrap();
        assert_eq!(data.get("LevelName"), Some(&Tag::String("Test".into())));
        assert_eq!(data.get("GameType"), Some(&Tag::Int(0)));
        assert_eq!(data.get("allowCommands"), Some(&Tag::Byte(0)));
        let difficulty = data.get("difficulty_settings").unwrap();
        assert_eq!(
            difficulty.get("difficulty"),
            Some(&Tag::String("hard".into()))
        );
        assert_eq!(difficulty.get("hardcore"), Some(&Tag::Byte(1)));
        assert_eq!(difficulty.get("locked"), Some(&Tag::Byte(0)));
        state.mode = GameMode::Creative;
        state.allow_commands = None;
        let metadata = state
            .freeze(PathBuf::from("game"), None)
            .level
            .draft_level_data(&spawn, 123, None, true)
            .unwrap();
        let data = metadata.value.get("Data").unwrap();
        assert_eq!(data.get("GameType"), Some(&Tag::Int(1)));
        assert_eq!(data.get("allowCommands"), Some(&Tag::Byte(1)));
        assert_eq!(data.get("initialized"), Some(&Tag::Byte(1)));
        assert_eq!(
            data.get("difficulty_settings").unwrap().get("difficulty"),
            Some(&Tag::String("easy".into()))
        );
    }

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
        let java_rows: Vec<_> = std::fs::read_to_string(&result_path)
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
        let debug_rows: Vec<_> = std::fs::read_to_string(&result_path)
            .unwrap()
            .lines()
            .filter(|line| line.starts_with("DEBUG "))
            .map(str::to_owned)
            .collect();
        let mut actual = Vec::new();
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
                            state.preset = native_resources::packs::ResourceId::parse(
                                "minecraft:debug_all_block_states",
                            )
                            .unwrap();
                            state.mode = mode;
                            state.difficulty = difficulty;
                            state.allow_commands = commands;
                            state.bonus_chest = bonus;
                            state.generate_structures = structures;
                            actual.push(format!(
                                "DEBUG DEBUG {} {} {} {} {}",
                                state.is_hardcore(),
                                format!("{:?}", state.effective_difficulty()).to_uppercase(),
                                state.effective_allow_commands(),
                                state.effective_bonus_chest(),
                                state.effective_generate_structures()
                            ));
                        }
                    }
                }
            }
        }
        assert_eq!(actual.len(), 144);
        assert_eq!(actual, debug_rows);
    }
}
