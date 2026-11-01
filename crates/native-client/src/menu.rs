//! Windows title screen and the entry points available without command-line arguments.

use std::{
    num::NonZeroU32,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

#[cfg(debug_assertions)]
use anyhow::bail;
use anyhow::{Context, Result};
use native_persistence::{anvil, nbt::Tag};
use native_resources::{default_minecraft_dir, load_atlas_with_packs};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    platform::run_on_demand::EventLoopExtRunOnDemand,
    window::{Window, WindowId},
};

use crate::{
    ViewOptions,
    accessibility_ui::AccessibilityControl,
    fov_ui::FovSlider,
    menu_assets::MenuAssets,
    menu_draw::{Canvas, Rect},
    menu_gpu_worker::{MenuFrame, MenuGpuWorker, MenuOverlay},
    menu_settings::{self, Settings, SoundCategory, SoundSettings, VideoSettings},
    menu_software_worker::{MenuSoftwareWorker, SoftwareFrame},
    mouse_ui::{self, ControlToggle, MouseControl, MouseUi, SprintWindowUi},
    sound_ui::SoundUi,
    video_ui::{VideoControl, VideoUi},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Screen {
    Loading,
    Launching,
    RecoverWorld,
    RecoveringWorld,
    Missing,
    Title,
    Worlds,
    Options,
    Controls,
    Mouse,
    Sound,
    Video,
    Accessibility,
    Packs,
    #[cfg(debug_assertions)]
    WorldSettings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ButtonId {
    ChooseMinecraft,
    Singleplayer,
    Multiplayer,
    Realms,
    Options,
    Controls,
    MouseSettings,
    SprintWindow,
    ToggleSneak,
    ToggleSprint,
    ToggleAttack,
    ToggleUse,
    MouseSensitivity,
    MouseWheelSensitivity,
    MouseDiscreteScroll,
    MouseInvertX,
    MouseInvertY,
    SoundSettings,
    SoundSlider(SoundCategory),
    OptionsFov,
    VideoSettings,
    VideoFps,
    VideoVsync,
    VideoFullscreen,
    VideoGuiScale,
    VideoInactivity,
    VideoBlur,
    VideoDistance,
    VideoBlend,
    VideoWeather,
    Accessibility,
    ViewBobbing,
    FovEffectScale,
    GlintSpeed,
    GlintStrength,
    PanoramaSpeed,
    MenuBlur,
    Quit,
    PlayWorld,
    RecoverWorld,
    #[cfg(debug_assertions)]
    PreviewWorld,
    CreateWorld,
    Demo,
    BrowseWorld,
    #[cfg(debug_assertions)]
    WorldSettings,
    Packs,
    AddPackFile,
    AddPackFolder,
    RemovePack,
    PackUp,
    PackDown,
    Done,
    #[cfg(debug_assertions)]
    Dimension,
    #[cfg(debug_assertions)]
    ChunkX,
    #[cfg(debug_assertions)]
    ChunkZ,
    #[cfg(debug_assertions)]
    Radius,
    Unavailable,
}

impl ButtonId {
    fn mouse_control(self) -> Option<MouseControl> {
        match self {
            Self::MouseSensitivity => Some(MouseControl::Sensitivity),
            Self::MouseWheelSensitivity => Some(MouseControl::WheelSensitivity),
            Self::MouseDiscreteScroll => Some(MouseControl::DiscreteScrolling),
            Self::MouseInvertX => Some(MouseControl::InvertX),
            Self::MouseInvertY => Some(MouseControl::InvertY),
            _ => None,
        }
    }
    fn accessibility_control(self) -> Option<AccessibilityControl> {
        match self {
            Self::ViewBobbing => Some(AccessibilityControl::ViewBobbing),
            Self::FovEffectScale => Some(AccessibilityControl::FovEffectScale),
            Self::GlintSpeed => Some(AccessibilityControl::GlintSpeed),
            Self::GlintStrength => Some(AccessibilityControl::GlintStrength),
            Self::PanoramaSpeed => Some(AccessibilityControl::PanoramaSpeed),
            Self::MenuBlur => Some(AccessibilityControl::MenuBlur),
            _ => None,
        }
    }
}

struct Button {
    id: ButtonId,
    rect: Rect,
    label: String,
    active: bool,
}

enum Action {
    Quit,
    Launch {
        prepared: Box<PreparedLaunch>,
        return_screen: Screen,
    },
}

enum PreparedLaunch {
    Demo {
        atlas: native_resources::TextureAtlas,
        content: crate::SessionContent,
        assets: Option<Arc<MenuAssets>>,
        video: VideoSettings,
    },
    World(crate::PreparedWorld),
}

struct PendingLaunch {
    result: mpsc::Receiver<Result<PreparedLaunch>>,
    return_screen: Screen,
    world: Option<PathBuf>,
}

#[derive(Clone, Copy)]
enum LaunchRequest {
    Demo,
    World { play: bool },
}

fn spawn_launch(
    prepare: impl FnOnce() -> Result<PreparedLaunch> + Send + 'static,
) -> Result<mpsc::Receiver<Result<PreparedLaunch>>> {
    let (sender, result) = mpsc::channel();
    std::thread::Builder::new()
        .name("world-preparation".into())
        .spawn(move || {
            let _ = sender.send(prepare());
        })?;
    Ok(result)
}

fn take_scroll_steps(remainder: &mut f64, amount: f64) -> i32 {
    if !amount.is_finite() {
        return 0;
    }
    *remainder += amount;
    let steps = remainder.trunc() as i32;
    *remainder -= f64::from(steps);
    steps
}

struct State {
    minecraft_dir: PathBuf,
    audio: Option<crate::audio::Audio>,
    video: VideoSettings,
    panorama_seconds: f64,
    sound: SoundSettings,
    video_dirty: bool,
    #[cfg(test)]
    settings_path: Option<PathBuf>,
    assets: Option<Arc<MenuAssets>>,
    packs: Vec<PathBuf>,
    available_packs: Vec<PathBuf>,
    selected_pack: Option<usize>,
    available_pack_scroll: usize,
    selected_pack_scroll: usize,
    available_pack_scroll_remainder: f64,
    selected_pack_scroll_remainder: f64,
    screen: Screen,
    worlds: Vec<WorldEntry>,
    selected_world: Option<usize>,
    world_scroll: usize,
    world_scroll_remainder: f64,
    error: Option<String>,
    asset_load: Option<PendingAssetLoad>,
    world_load: Option<PendingWorldLoad>,
    pack_load: Option<PendingPackLoad>,
    #[cfg(debug_assertions)]
    dimension: String,
    #[cfg(debug_assertions)]
    chunk_x: String,
    #[cfg(debug_assertions)]
    chunk_z: String,
    #[cfg(debug_assertions)]
    radius: String,
}

enum AssetLoadKind {
    Startup,
    StartupWithoutPacks(String),
    ChooseDirectory,
    ChangePacks(Option<usize>),
}

struct PendingAssetLoad {
    directory: PathBuf,
    packs: Vec<PathBuf>,
    kind: AssetLoadKind,
    result: mpsc::Receiver<Result<MenuAssets>>,
}

enum LoadedWorlds {
    List(Vec<WorldEntry>),
    Chosen(Option<WorldEntry>),
}

struct PendingWorldLoad {
    directory: PathBuf,
    result: mpsc::Receiver<LoadedWorlds>,
}

struct PendingPackLoad {
    directory: PathBuf,
    result: mpsc::Receiver<Vec<PathBuf>>,
}

struct WorldEntry {
    path: PathBuf,
    name: String,
    data_version: Option<i32>,
    last_played: Option<i64>,
    icon: Option<image::RgbaImage>,
}

impl WorldEntry {
    fn read(path: PathBuf) -> Self {
        let mut name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let mut data_version = None;
        let mut last_played = None;
        if let Ok((info, level)) = anvil::read_level_document(&path) {
            data_version = Some(info.data_version);
            if let Some(Tag::Long(value)) = level
                .value
                .get("Data")
                .and_then(|data| data.get("LastPlayed"))
            {
                last_played = Some(*value);
            }
            if let Some(Tag::String(level_name)) = level
                .value
                .get("Data")
                .and_then(|data| data.get("LevelName"))
            {
                let value = level_name.to_string_lossy();
                if !value.is_empty() {
                    name = value;
                }
            }
        }
        let icon = image::ImageReader::open(path.join("icon.png"))
            .ok()
            .and_then(|mut reader| {
                let mut limits = image::Limits::default();
                limits.max_image_width = Some(512);
                limits.max_image_height = Some(512);
                limits.max_alloc = Some(2 * 1024 * 1024);
                reader.limits(limits);
                reader.decode().ok().map(|image| image.to_rgba8())
            });
        Self {
            path,
            name,
            data_version,
            last_played,
            icon,
        }
    }
}

impl State {
    fn new(minecraft_dir: PathBuf, packs: Vec<PathBuf>) -> Self {
        let mut state = Self {
            minecraft_dir,
            audio: None,
            video: VideoSettings::default(),
            panorama_seconds: 0.0,
            sound: SoundSettings::default(),
            video_dirty: false,
            #[cfg(test)]
            settings_path: None,
            assets: None,
            packs,
            available_packs: Vec::new(),
            selected_pack: None,
            available_pack_scroll: 0,
            selected_pack_scroll: 0,
            available_pack_scroll_remainder: 0.0,
            selected_pack_scroll_remainder: 0.0,
            screen: Screen::Loading,
            worlds: Vec::new(),
            selected_world: None,
            world_scroll: 0,
            world_scroll_remainder: 0.0,
            error: None,
            asset_load: None,
            world_load: None,
            pack_load: None,
            #[cfg(debug_assertions)]
            dimension: String::new(),
            #[cfg(debug_assertions)]
            chunk_x: String::new(),
            #[cfg(debug_assertions)]
            chunk_z: String::new(),
            #[cfg(debug_assertions)]
            radius: "1".into(),
        };
        state.begin_asset_load(
            state.minecraft_dir.clone(),
            state.packs.clone(),
            AssetLoadKind::Startup,
        );
        state
    }

    fn save_settings(&mut self) {
        let settings = Settings {
            minecraft_dir: Some(self.minecraft_dir.clone()),
            resource_packs: self.packs.clone(),
            video: self.video,
            sound: self.sound,
        };
        #[cfg(test)]
        let saved = self.settings_path.as_ref().map_or_else(
            || menu_settings::save(&settings),
            |path| menu_settings::save_to(path, &settings),
        );
        #[cfg(not(test))]
        let saved = menu_settings::save(&settings);
        if let Err(error) = saved {
            self.error = Some(format!("{error:#}"));
        } else {
            self.video_dirty = false;
        }
    }

    fn begin_asset_load(&mut self, directory: PathBuf, packs: Vec<PathBuf>, kind: AssetLoadKind) {
        let (sender, result) = mpsc::channel();
        let load_directory = directory.clone();
        let load_packs = packs.clone();
        std::thread::spawn(move || {
            let loaded = load_packs
                .iter()
                .try_for_each(|path| validate_pack(path))
                .and_then(|()| MenuAssets::load(&load_directory, &load_packs));
            let _ = sender.send(loaded);
        });
        self.error = None;
        self.asset_load = Some(PendingAssetLoad {
            directory,
            packs,
            kind,
            result,
        });
    }

    fn poll_asset_load(&mut self) -> bool {
        let result = match self.asset_load.as_ref().map(|load| load.result.try_recv()) {
            None | Some(Err(mpsc::TryRecvError::Empty)) => return false,
            Some(Ok(result)) => result,
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                Err(anyhow::anyhow!("Resource loading stopped unexpectedly."))
            }
        };
        let load = self.asset_load.take().expect("completed asset load");
        match result {
            Ok(assets) => {
                self.minecraft_dir = load.directory;
                self.packs = load.packs;
                self.assets = Some(Arc::new(assets));
                if self.audio.is_none() {
                    self.audio = crate::audio::Audio::new().ok();
                }
                if let Some(audio) = &self.audio {
                    audio.set_gain(self.sound.ui_gain());
                    audio.prepare(&self.minecraft_dir, &self.packs);
                }
                self.error = match &load.kind {
                    AssetLoadKind::StartupWithoutPacks(error) => {
                        Some(format!("Could not load the saved resource packs: {error}"))
                    }
                    _ => None,
                };
                if matches!(self.screen, Screen::Loading | Screen::Missing) {
                    self.screen = Screen::Title;
                }
                if let AssetLoadKind::ChangePacks(selected) = load.kind {
                    self.selected_pack = selected;
                    self.refresh_packs();
                    self.save_settings();
                } else if matches!(load.kind, AssetLoadKind::ChooseDirectory) {
                    self.selected_pack = None;
                    self.refresh_packs();
                    self.save_settings();
                }
            }
            Err(error) => {
                let message = format!("{error:#}");
                if matches!(load.kind, AssetLoadKind::Startup) && !load.packs.is_empty() {
                    self.begin_asset_load(
                        load.directory,
                        Vec::new(),
                        AssetLoadKind::StartupWithoutPacks(message),
                    );
                } else {
                    if self.assets.is_none() {
                        self.screen = Screen::Missing;
                    }
                    self.error = Some(message);
                }
            }
        }
        true
    }

    fn refresh_worlds(&mut self) {
        let directory = self.minecraft_dir.clone();
        let load_directory = directory.clone();
        let (sender, result) = mpsc::channel();
        std::thread::spawn(move || {
            let mut worlds: Vec<_> = std::fs::read_dir(load_directory.join("saves"))
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.is_dir() && path.join("level.dat").is_file())
                .map(WorldEntry::read)
                .collect();
            worlds.sort_by(|a, b| {
                b.last_played
                    .cmp(&a.last_played)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
            let _ = sender.send(LoadedWorlds::List(worlds));
        });
        self.worlds.clear();
        self.selected_world = None;
        self.world_scroll = 0;
        self.world_load = Some(PendingWorldLoad { directory, result });
    }

    fn browse_world(&mut self, path: PathBuf) {
        let directory = self.minecraft_dir.clone();
        let (sender, result) = mpsc::channel();
        std::thread::spawn(move || {
            let world = path
                .join("level.dat")
                .is_file()
                .then(|| WorldEntry::read(path));
            let _ = sender.send(LoadedWorlds::Chosen(world));
        });
        self.world_load = Some(PendingWorldLoad { directory, result });
    }

    fn poll_world_load(&mut self) -> bool {
        let result = match self.world_load.as_ref().map(|load| load.result.try_recv()) {
            None | Some(Err(mpsc::TryRecvError::Empty)) => return false,
            Some(Ok(result)) => Some(result),
            Some(Err(mpsc::TryRecvError::Disconnected)) => None,
        };
        let load = self.world_load.take().expect("completed world load");
        if load.directory != self.minecraft_dir {
            return true;
        }
        match result {
            Some(LoadedWorlds::List(worlds)) => self.worlds = worlds,
            Some(LoadedWorlds::Chosen(Some(world))) => {
                if let Some(i) = self.worlds.iter().position(|item| item.path == world.path) {
                    self.selected_world = Some(i);
                } else {
                    self.worlds.push(world);
                    self.selected_world = Some(self.worlds.len() - 1);
                }
            }
            Some(LoadedWorlds::Chosen(None)) => {
                self.error = Some("The selected folder does not contain level.dat.".into())
            }
            None => self.error = Some("Could not read the world list.".into()),
        }
        true
    }

    fn refresh_packs(&mut self) {
        let directory = self.minecraft_dir.clone();
        let load_directory = directory.clone();
        let (sender, result) = mpsc::channel();
        std::thread::spawn(move || {
            let mut available: Vec<_> = std::fs::read_dir(load_directory.join("resourcepacks"))
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    (path.is_dir() && path.join("pack.mcmeta").is_file())
                        || path
                            .extension()
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
                })
                .collect();
            available.sort_by_key(|path| path.file_name().map(|name| name.to_os_string()));
            let _ = sender.send(available);
        });
        self.available_packs.clear();
        self.pack_load = Some(PendingPackLoad { directory, result });
    }

    fn poll_pack_load(&mut self) -> bool {
        let result = match self.pack_load.as_ref().map(|load| load.result.try_recv()) {
            None | Some(Err(mpsc::TryRecvError::Empty)) => return false,
            Some(Ok(result)) => Some(result),
            Some(Err(mpsc::TryRecvError::Disconnected)) => None,
        };
        let load = self.pack_load.take().expect("completed pack load");
        if load.directory != self.minecraft_dir {
            return true;
        }
        match result {
            Some(paths) => {
                self.available_packs = paths
                    .into_iter()
                    .filter(|path| !self.packs.contains(path))
                    .collect()
            }
            None => self.error = Some("Could not read the resource pack list.".into()),
        }
        self.available_pack_scroll = self
            .available_pack_scroll
            .min(self.available_packs.len().saturating_sub(1));
        self.selected_pack_scroll = self.selected_pack_scroll.min(self.packs.len());
        true
    }

    fn chosen_world(&self) -> Option<&Path> {
        self.selected_world
            .and_then(|i| self.worlds.get(i))
            .map(|world| world.path.as_path())
    }

    #[cfg(debug_assertions)]
    fn view_options(&self, play: bool) -> Result<ViewOptions<'_>> {
        let chunk = match (self.chunk_x.trim(), self.chunk_z.trim()) {
            ("", "") => None,
            ("", _) | (_, "") => bail!("Enter both chunk X and chunk Z, or leave both empty."),
            (x, z) => Some((
                x.parse().context("Chunk X must be an integer")?,
                z.parse().context("Chunk Z must be an integer")?,
            )),
        };
        let radius: u32 = self
            .radius
            .trim()
            .parse()
            .context("Chunk radius must be a nonnegative integer")?;
        if radius > 32 {
            bail!("Chunk radius must not exceed 32.");
        }
        Ok(ViewOptions {
            dimension: (!self.dimension.trim().is_empty()).then_some(self.dimension.trim()),
            chunk,
            radius: if play {
                u32::from(self.video.render_distance)
            } else {
                radius
            },
            smoke_test: false,
            block_report: None,
            play,
            video: self.video,
        })
    }

    #[cfg(not(debug_assertions))]
    fn view_options(&self, play: bool) -> ViewOptions<'_> {
        ViewOptions {
            dimension: None,
            chunk: None,
            radius: u32::from(self.video.render_distance),
            smoke_test: false,
            block_report: None,
            play,
            video: self.video,
        }
    }

    fn add_pack(&mut self, path: PathBuf) {
        if self.packs.contains(&path) {
            self.error = Some("This resource pack is already selected.".into());
            return;
        }
        let mut next = self.packs.clone();
        next.push(path);
        self.begin_asset_load(
            self.minecraft_dir.clone(),
            next,
            AssetLoadKind::ChangePacks(self.selected_pack),
        );
    }
}

pub fn run() -> Result<()> {
    let settings = menu_settings::load();
    let dir = settings
        .minecraft_dir
        .unwrap_or_else(|| default_minecraft_dir().unwrap_or_default());
    let mut state = State::new(dir, settings.resource_packs);
    state.video = settings.video;
    state.sound = settings.sound;
    let mut event_loop = EventLoop::new().context("Cannot create the Minecraft window")?;
    loop {
        event_loop.set_control_flow(ControlFlow::Wait);
        let mut menu = Menu::new(&mut state);
        event_loop
            .run_app_on_demand(&mut menu)
            .context("Minecraft menu event loop failed")?;
        if let Some(error) = menu.error.take() {
            return Err(error);
        }
        let action = menu.action.take().unwrap_or(Action::Quit);
        drop(menu);
        match action {
            Action::Quit => return Ok(()),
            Action::Launch {
                prepared,
                return_screen,
            } => {
                let audio = state.audio.clone().map(|audio| {
                    crate::audio::UiAudio::new(
                        audio,
                        state.minecraft_dir.clone(),
                        state.packs.clone(),
                    )
                });
                let result = match *prepared {
                    PreparedLaunch::Demo {
                        atlas,
                        content,
                        assets,
                        video,
                    } => crate::run_demo(
                        &mut event_loop,
                        atlas,
                        false,
                        assets,
                        content,
                        video,
                        audio,
                    ),
                    PreparedLaunch::World(world) => {
                        crate::run_prepared_world(&mut event_loop, world, audio)
                    }
                };
                match result {
                    Ok(crate::GameExit::Quit) => return Ok(()),
                    Ok(crate::GameExit::ReturnToTitle(video)) => {
                        state.video = video;
                        state.sound = menu_settings::load().sound;
                        if let Some(audio) = &state.audio {
                            audio.set_gain(state.sound.ui_gain());
                        }
                        state.screen = Screen::Title;
                    }
                    Err(error) => {
                        state.screen = return_screen;
                        state.error = Some(format!("{error:#}"));
                    }
                }
            }
        }
    }
}

fn validate_pack(path: &Path) -> Result<()> {
    use std::io::Read;
    const MAX_METADATA: u64 = 1024 * 1024;
    let mut bytes = Vec::new();
    if path.is_dir() {
        std::fs::File::open(path.join("pack.mcmeta"))
            .context("Resource pack is missing pack.mcmeta")?
            .take(MAX_METADATA + 1)
            .read_to_end(&mut bytes)?;
    } else {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(path)?)
            .context("Resource pack is not a valid ZIP")?;
        zip.by_name("pack.mcmeta")
            .context("Resource pack is missing pack.mcmeta")?
            .take(MAX_METADATA + 1)
            .read_to_end(&mut bytes)?;
    }
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_METADATA,
        "Resource pack metadata is too large"
    );
    let metadata: serde_json::Value =
        serde_json::from_slice(&bytes).context("Invalid resource pack metadata")?;
    anyhow::ensure!(
        metadata
            .get("pack")
            .is_some_and(serde_json::Value::is_object),
        "Resource pack metadata has no pack section"
    );
    Ok(())
}

// Opt-in diagnostics for a real window. Excluded from normal release builds.
#[cfg(any(debug_assertions, test))]
#[derive(Default)]
struct MenuFrameTimings {
    screen: Option<Screen>,
    previous: Option<Instant>,
    warmup: usize,
    intervals: Vec<Duration>,
    redraw_costs: Vec<Duration>,
    reported: bool,
}

#[cfg(any(debug_assertions, test))]
impl MenuFrameTimings {
    fn record(
        &mut self,
        screen: Screen,
        start: Instant,
        redraw_cost: Duration,
        refresh_interval: Duration,
        size: (u32, u32),
        gpu: bool,
    ) {
        if self.screen != Some(screen) {
            *self = Self {
                screen: Some(screen),
                ..Self::default()
            };
        }
        if self.reported {
            return;
        }
        let previous = self.previous.replace(start);
        if self.warmup < 30 {
            self.warmup += 1;
            return;
        }
        if let Some(previous) = previous {
            self.intervals.push(start.duration_since(previous));
            self.redraw_costs.push(redraw_cost);
        }
        if self.intervals.len() == 300 {
            self.intervals.sort_unstable();
            self.redraw_costs.sort_unstable();
            let slow = self
                .intervals
                .iter()
                .filter(|interval| **interval > MENU_FRAME_INTERVAL + MENU_FRAME_INTERVAL / 2)
                .count();
            eprintln!(
                "Menu frame timings: screen={screen:?} size={}x{} gpu={gpu} refresh={refresh_interval:?} schedule={MENU_FRAME_INTERVAL:?} samples=300 redraw-interval median={:?} p95={:?} p99={:?} max={:?} over-1.5-schedule={slow}; redraw-call median={:?} p95={:?} p99={:?}. Intervals are CPU event timings, not display scanout.",
                size.0,
                size.1,
                self.intervals[150],
                self.intervals[285],
                self.intervals[297],
                self.intervals[299],
                self.redraw_costs[150],
                self.redraw_costs[285],
                self.redraw_costs[297],
            );
            self.reported = true;
        }
    }
}

struct PanoramaClock {
    last: Instant,
    seconds: f64,
    speed: f64,
}

// A completed panorama may still be shown after a pointer or focus change,
// but never after the underlying screen, size, GUI scale, assets or blur changed.
struct SoftwareScene {
    screen: Screen,
    size: (u32, u32),
    scale: u32,
    blur: u8,
    assets: Option<Arc<MenuAssets>>,
}

impl SoftwareScene {
    fn matches(
        &self,
        screen: Screen,
        size: (u32, u32),
        scale: u32,
        blur: u8,
        assets: &Option<Arc<MenuAssets>>,
    ) -> bool {
        self.screen == screen
            && self.size == size
            && self.scale == scale
            && self.blur == blur
            && match (&self.assets, assets) {
                (Some(left), Some(right)) => Arc::ptr_eq(left, right),
                (None, None) => true,
                _ => false,
            }
    }
}

impl PanoramaClock {
    fn new(now: Instant, speed: f64) -> Self {
        Self {
            last: now,
            seconds: 0.0,
            speed,
        }
    }

    fn sample(&mut self, now: Instant, speed: f64) -> f32 {
        self.seconds += now.saturating_duration_since(self.last).as_secs_f64() * self.speed;
        self.last = now;
        self.speed = speed;
        self.seconds as f32
    }
}

struct Menu<'a> {
    state: &'a mut State,
    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    gpu: Option<MenuGpuWorker>,
    software: Option<MenuSoftwareWorker>,
    software_overlay: Option<Arc<Canvas>>,
    software_generation: u64,
    software_scene: Option<SoftwareScene>,
    software_frame_due: bool,
    #[cfg(test)]
    software_presented: Option<((u32, u32), u64, usize)>,
    overlay_dirty: bool,
    occluded: bool,
    refresh_interval: Duration,
    cursor: (i32, i32),
    cursor_raw: Option<(f64, f64)>,
    focused: Option<ButtonId>,
    action: Option<Action>,
    error: Option<anyhow::Error>,
    panorama: PanoramaClock,
    next_frame: Instant,
    inactivity: crate::frame_pacer::InactivityTracker,
    #[cfg(any(debug_assertions, test))]
    frame_timings: Option<MenuFrameTimings>,
    dialog: Option<mpsc::Receiver<(ButtonId, Option<PathBuf>)>>,
    last_world_click: Option<(usize, Instant)>,
    size: (u32, u32),
    scale: u32,
    video_ui: VideoUi,
    sound_ui: SoundUi,
    mouse_ui: MouseUi,
    sprint_window_ui: SprintWindowUi,
    accessibility_ui: crate::accessibility_ui::AccessibilityUi,
    fov_ui: FovSlider,
    slider_mouse_pressed: bool,
    pending_launch: Option<PendingLaunch>,
    pending_recovery: Option<mpsc::Receiver<Result<bool>>>,
    recovery_world: Option<PathBuf>,
    recovery_return_screen: Screen,
}

impl<'a> Menu<'a> {
    fn new(state: &'a mut State) -> Self {
        let video_ui = VideoUi::new(state.video);
        let fov_ui = FovSlider::new(state.video.fov);
        let mut panorama = PanoramaClock::new(Instant::now(), state.video.panorama_speed.value());
        panorama.seconds = state.panorama_seconds;
        Self {
            state,
            window: None,
            surface: None,
            gpu: None,
            software: None,
            software_overlay: None,
            software_generation: 0,
            software_scene: None,
            software_frame_due: false,
            #[cfg(test)]
            software_presented: None,
            overlay_dirty: true,
            occluded: false,
            refresh_interval: Duration::from_nanos(1_000_000_000 / 60),
            cursor: (-1, -1),
            cursor_raw: None,
            focused: None,
            action: None,
            error: None,
            panorama,
            next_frame: Instant::now(),
            inactivity: crate::frame_pacer::InactivityTracker::new(Instant::now()),
            #[cfg(any(debug_assertions, test))]
            frame_timings: std::env::var_os("NATIVE_MENU_FRAME_TIMINGS")
                .map(|_| MenuFrameTimings::default()),
            dialog: None,
            last_world_click: None,
            size: (0, 0),
            scale: 1,
            video_ui,
            sound_ui: SoundUi::default(),
            mouse_ui: MouseUi::default(),
            sprint_window_ui: SprintWindowUi::default(),
            accessibility_ui: crate::accessibility_ui::AccessibilityUi::default(),
            fov_ui,
            slider_mouse_pressed: false,
            pending_launch: None,
            pending_recovery: None,
            recovery_world: None,
            recovery_return_screen: Screen::Worlds,
        }
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: anyhow::Error) {
        self.error = Some(error);
        event_loop.exit();
    }

    fn play_button_sound(&mut self) {
        if self.state.assets.is_none() {
            return;
        }
        if self.state.audio.is_none() {
            self.state.audio = crate::audio::Audio::new().ok();
        }
        if let Some(audio) = &self.state.audio {
            audio.set_gain(self.state.sound.ui_gain());
            audio.click(&self.state.minecraft_dir, &self.state.packs);
        }
    }

    fn toggle_fullscreen(&mut self) {
        self.state.video.fullscreen = !self.state.video.fullscreen;
        self.state.video_dirty = true;
        if let Some(window) = &self.window {
            crate::apply_fullscreen(window, self.state.video.fullscreen);
        }
        self.invalidate();
    }

    fn update_gui_scale(&mut self) {
        self.scale = gui_scale_for(self.size.0, self.size.1, self.state.video.gui_scale);
        if let Some((x, y)) = self.cursor_raw {
            self.cursor = (
                (x / f64::from(self.scale)) as i32,
                (y / f64::from(self.scale)) as i32,
            );
        }
        let (_, height) = self.logical_size();
        self.video_ui.resize(height);
        self.accessibility_ui.resize(height);
        self.sound_ui.resize(height);
    }

    fn begin_launch(&mut self, request: LaunchRequest) -> Result<()> {
        if self.pending_launch.is_some() {
            return Ok(());
        }
        let directory = self.state.minecraft_dir.clone();
        let packs = self.state.packs.clone();
        let assets = self.state.assets.clone();
        let video = self.state.video;
        let return_screen = self.state.screen;
        let launch_world = match request {
            LaunchRequest::World { .. } => Some(
                self.state
                    .chosen_world()
                    .context("Select a world first")?
                    .to_path_buf(),
            ),
            LaunchRequest::Demo => None,
        };
        let result = match request {
            LaunchRequest::Demo => spawn_launch(move || {
                let atlas = load_atlas_with_packs(&directory, &packs)?;
                let mut resources = native_resources::packs::ResourceStack::open(&directory)?;
                let recipes = native_resources::recipes::CraftingRecipes::load(&mut resources)?;
                let tools = crate::load_tool_transformers(&mut resources)?;
                let loot = crate::load_block_loot(&mut resources, &tools)?;
                let enchantments =
                    native_resources::enchantments::OxygenEnchantments::load(&mut resources)?;
                Ok(PreparedLaunch::Demo {
                    atlas,
                    content: crate::SessionContent {
                        recipes: Some(Arc::new(recipes)),
                        loot: Some(Arc::new(loot)),
                        enchantments: Some(Arc::new(enchantments)),
                        tools: tools.map(Some),
                    },
                    assets,
                    video,
                })
            })?,
            LaunchRequest::World { play } => {
                let world = launch_world.clone().context("Select a world first")?;
                #[cfg(debug_assertions)]
                let options = self.state.view_options(play)?;
                #[cfg(not(debug_assertions))]
                let options = self.state.view_options(play);
                let dimension = options.dimension.map(str::to_owned);
                let report = options.block_report.map(Path::to_path_buf);
                let chunk = options.chunk;
                let radius = options.radius;
                let smoke_test = options.smoke_test;
                let video = options.video;
                spawn_launch(move || {
                    let options = ViewOptions {
                        dimension: dimension.as_deref(),
                        chunk,
                        radius,
                        smoke_test,
                        block_report: report.as_deref(),
                        play,
                        video,
                    };
                    crate::prepare_existing_world(&world, &directory, &packs, options, assets)
                        .map(PreparedLaunch::World)
                })?
            }
        };
        self.state.error = None;
        self.state.screen = Screen::Launching;
        self.pending_launch = Some(PendingLaunch {
            result,
            return_screen,
            world: launch_world,
        });
        self.invalidate();
        Ok(())
    }

    fn poll_launch(&mut self, event_loop: &ActiveEventLoop) {
        let result = match self
            .pending_launch
            .as_ref()
            .map(|pending| pending.result.try_recv())
        {
            None | Some(Err(mpsc::TryRecvError::Empty)) => return,
            Some(Ok(result)) => result,
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                Err(anyhow::anyhow!("World loading stopped unexpectedly."))
            }
        };
        let pending = self.pending_launch.take().expect("completed launch");
        match result {
            Ok(prepared) => {
                self.action = Some(Action::Launch {
                    prepared: Box::new(prepared),
                    return_screen: pending.return_screen,
                });
                event_loop.exit();
            }
            Err(error) => {
                if error
                    .downcast_ref::<native_persistence::save::InterruptedWorldSave>()
                    .is_some()
                    && let Some(world) = pending.world
                {
                    self.recovery_world = Some(world);
                    self.recovery_return_screen = pending.return_screen;
                    self.state.screen = Screen::RecoverWorld;
                    self.state.error = None;
                    self.focused = Some(ButtonId::Done);
                } else {
                    self.state.screen = pending.return_screen;
                    self.state.error = Some(self.world_error_text(&error));
                }
                self.invalidate();
            }
        }
    }

    fn world_error_text(&self, error: &anyhow::Error) -> String {
        if error
            .downcast_ref::<native_persistence::save::WorldInUse>()
            .is_some()
        {
            self.state
                .assets
                .as_ref()
                .map_or("World is already in use.", |assets| {
                    assets.translate("selectWorld.locked", "World is already in use.")
                })
                .to_owned()
        } else {
            format!("{error:#}")
        }
    }

    fn begin_recovery(&mut self) -> Result<()> {
        if self.pending_recovery.is_some() {
            return Ok(());
        }
        let world = self
            .recovery_world
            .clone()
            .context("Select a world first")?;
        let (sender, result) = mpsc::channel();
        std::thread::Builder::new()
            .name("world-recovery".into())
            .spawn(move || {
                let _ = sender
                    .send(native_persistence::save::WorldWriteGuard::recover_interrupted(&world));
            })?;
        self.state.error = None;
        self.state.screen = Screen::RecoveringWorld;
        self.pending_recovery = Some(result);
        self.focused = None;
        self.invalidate();
        Ok(())
    }

    fn poll_recovery(&mut self) {
        let result = match self.pending_recovery.as_ref().map(mpsc::Receiver::try_recv) {
            None | Some(Err(mpsc::TryRecvError::Empty)) => return,
            Some(Ok(result)) => result,
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                Err(anyhow::anyhow!("World recovery stopped unexpectedly."))
            }
        };
        self.pending_recovery = None;
        match result {
            Ok(_) => {
                self.state.screen = self.recovery_return_screen;
                self.recovery_world = None;
                self.state.error = None;
            }
            Err(error) => {
                self.state.screen = Screen::RecoverWorld;
                self.state.error = Some(self.world_error_text(&error));
                self.focused = Some(ButtonId::Done);
            }
        }
        self.invalidate();
    }

    fn use_software_renderer(&mut self) -> Result<()> {
        let window = self
            .window
            .as_ref()
            .context("Menu window unavailable")?
            .clone();
        self.gpu = None;
        let context =
            softbuffer::Context::new(window.clone()).map_err(|e| anyhow::anyhow!("{e}"))?;
        self.surface = Some(
            softbuffer::Surface::new(&context, window.clone())
                .map_err(|e| anyhow::anyhow!("{e}"))?,
        );
        self.software = Some(MenuSoftwareWorker::new(window)?);
        self.software_overlay = None;
        self.software_scene = None;
        self.overlay_dirty = true;
        Ok(())
    }

    fn animation_active(&self) -> bool {
        self.state.assets.is_some() && !self.occluded && self.size.0 > 0 && self.size.1 > 0
    }

    fn invalidate(&mut self) {
        self.overlay_dirty = true;
        // While the panorama is running, coalesce interaction changes into the
        // next scheduled frame instead of rendering an extra frame per input.
        if !self.animation_active()
            && let Some(window) = &self.window
        {
            window.request_redraw();
        }
    }

    fn logical_size(&self) -> (i32, i32) {
        (
            (self.size.0 / self.scale).max(1) as i32,
            (self.size.1 / self.scale).max(1) as i32,
        )
    }

    fn hover_target(
        &self,
        buttons: &[Button],
        cursor: (i32, i32),
    ) -> (Option<ButtonId>, Option<(bool, usize)>) {
        let button = buttons
            .iter()
            .find(|button| button.active && self.button_contains(button, cursor))
            .map(|button| button.id);
        if self.state.screen != Screen::Packs || cursor.1 < 43 {
            return (button, None);
        }
        let (w, h) = self.logical_size();
        let row = (cursor.1 - 43) / 36;
        let row_y = (cursor.1 - 43) % 36;
        let visible = (h - 125).max(0) / 36;
        if row >= visible || row_y >= 34 {
            return (button, None);
        }
        let row = row as usize;
        let hovered = if (w / 2 - 153..w / 2 - 5).contains(&cursor.0) {
            let index = self.state.available_pack_scroll + row;
            (index < self.state.available_packs.len()).then_some((true, index))
        } else if (w / 2 + 5..w / 2 + 153).contains(&cursor.0) {
            let index = self.state.selected_pack_scroll + row;
            (index < self.state.packs.len()).then_some((false, index))
        } else {
            None
        };
        (button, hovered)
    }

    fn button_contains(&self, button: &Button, cursor: (i32, i32)) -> bool {
        if let ButtonId::SoundSlider(category) = button.id {
            let (w, h) = self.logical_size();
            return self.sound_ui.hit(w, h, cursor) == Some(category);
        }
        if let Some(control) = button.id.accessibility_control() {
            let (w, h) = self.logical_size();
            return self.accessibility_ui.hit_control(w, h, cursor) == Some(control);
        }
        let expected = match button.id {
            ButtonId::VideoFps => Some(VideoControl::Framerate),
            ButtonId::VideoVsync => Some(VideoControl::Vsync),
            ButtonId::VideoFullscreen => Some(VideoControl::Fullscreen),
            ButtonId::VideoGuiScale => Some(VideoControl::GuiScale),
            ButtonId::VideoInactivity => Some(VideoControl::Inactivity),
            ButtonId::VideoBlur => Some(VideoControl::MenuBlur),
            ButtonId::VideoDistance => Some(VideoControl::RenderDistance),
            ButtonId::VideoBlend => Some(VideoControl::BiomeBlend),
            ButtonId::VideoWeather => Some(VideoControl::WeatherRadius),
            _ => None,
        };
        if let Some(expected) = expected {
            let (w, h) = self.logical_size();
            self.video_ui.hit(w, h, cursor) == Some(expected)
        } else {
            button.rect.contains(cursor.0, cursor.1)
        }
    }

    fn buttons(&self) -> Vec<Button> {
        let (w, h) = self.logical_size();
        let x = w / 2 - 100;
        let y = h / 4 + 48;
        let tr = |key: &str, fallback: &str| {
            self.state.assets.as_ref().map_or_else(
                || fallback.to_owned(),
                |assets| assets.translate(key, fallback).to_owned(),
            )
        };
        let mut result = Vec::new();
        let mut add = |id, x, y, width, label: String, active| {
            result.push(Button {
                id,
                rect: Rect {
                    x,
                    y,
                    w: width,
                    h: 20,
                },
                label,
                active,
            })
        };
        match self.state.screen {
            Screen::Launching | Screen::RecoveringWorld => {}
            Screen::RecoverWorld => {
                add(
                    ButtonId::RecoverWorld,
                    w / 2 - 154,
                    h / 2 + 28,
                    150,
                    tr("mco.backup.button.restore", "Restore"),
                    true,
                );
                add(
                    ButtonId::Done,
                    w / 2 + 4,
                    h / 2 + 28,
                    150,
                    tr("gui.cancel", "Cancel"),
                    true,
                );
            }
            Screen::Loading => {
                add(
                    ButtonId::Quit,
                    x,
                    h / 2 + 24,
                    200,
                    tr("menu.quit", "Quit Game"),
                    true,
                );
            }
            Screen::Missing => {
                add(
                    ButtonId::ChooseMinecraft,
                    x,
                    h / 2,
                    200,
                    "Choose Minecraft Folder...".into(),
                    true,
                );
                add(
                    ButtonId::Quit,
                    x,
                    h / 2 + 24,
                    200,
                    tr("menu.quit", "Quit Game"),
                    true,
                );
            }
            Screen::Title => {
                add(
                    ButtonId::Singleplayer,
                    x,
                    y,
                    200,
                    tr("menu.singleplayer", "Singleplayer"),
                    true,
                );
                add(
                    ButtonId::Multiplayer,
                    x,
                    y + 24,
                    200,
                    tr("menu.multiplayer", "Multiplayer"),
                    false,
                );
                add(
                    ButtonId::Realms,
                    x,
                    y + 48,
                    200,
                    tr("menu.online", "Minecraft Realms"),
                    false,
                );
                add(
                    ButtonId::Options,
                    x,
                    y + 84,
                    98,
                    tr("menu.options", "Options..."),
                    true,
                );
                add(
                    ButtonId::Quit,
                    x + 102,
                    y + 84,
                    98,
                    tr("menu.quit", "Quit Game"),
                    true,
                );
            }
            Screen::Worlds => {
                let selected =
                    self.state.world_load.is_none() && self.state.chosen_world().is_some();
                add(
                    ButtonId::PlayWorld,
                    w / 2 - 154,
                    h - 76,
                    150,
                    tr("selectWorld.select", "Play Selected World"),
                    selected,
                );
                add(
                    ButtonId::CreateWorld,
                    w / 2 + 4,
                    h - 76,
                    150,
                    tr("selectWorld.create", "Create New World"),
                    false,
                );
                #[cfg(debug_assertions)]
                add(
                    ButtonId::PreviewWorld,
                    w / 2 - 154,
                    h - 52,
                    100,
                    "Preview".into(),
                    selected,
                );
                #[cfg(debug_assertions)]
                add(
                    ButtonId::WorldSettings,
                    w / 2 - 50,
                    h - 52,
                    100,
                    "World Settings...".into(),
                    true,
                );
                #[cfg(debug_assertions)]
                add(
                    ButtonId::BrowseWorld,
                    w / 2 + 54,
                    h - 52,
                    100,
                    "Open Folder...".into(),
                    self.state.world_load.is_none(),
                );
                #[cfg(not(debug_assertions))]
                add(
                    ButtonId::BrowseWorld,
                    x,
                    h - 52,
                    200,
                    "Open Folder...".into(),
                    self.state.world_load.is_none(),
                );
                add(
                    ButtonId::Demo,
                    w / 2 - 154,
                    h - 28,
                    150,
                    "Play Demo World".into(),
                    true,
                );
                add(
                    ButtonId::Done,
                    w / 2 + 4,
                    h - 28,
                    150,
                    tr("gui.cancel", "Cancel"),
                    true,
                );
            }
            Screen::Options => {
                for (row, (left, right)) in [
                    (
                        ("options.fov", "FOV: Normal"),
                        ("options.difficulty", "Difficulty"),
                    ),
                    (
                        ("options.skinCustomisation", "Skin Customization..."),
                        ("options.sounds", "Music & Sounds..."),
                    ),
                    (
                        ("options.video", "Video Settings..."),
                        ("options.controls", "Controls..."),
                    ),
                    (
                        ("options.language", "Language..."),
                        ("options.chat.title", "Chat Settings..."),
                    ),
                    (
                        ("options.resourcepack", "Resource Packs..."),
                        ("options.accessibility.title", "Accessibility Settings..."),
                    ),
                ]
                .into_iter()
                .enumerate()
                {
                    let y = 35 + row as i32 * 24;
                    let left_id = match left.0 {
                        "options.fov" => ButtonId::OptionsFov,
                        "options.resourcepack" => ButtonId::Packs,
                        "options.video" => ButtonId::VideoSettings,
                        _ => ButtonId::Unavailable,
                    };
                    add(
                        left_id,
                        w / 2 - 155,
                        y,
                        150,
                        tr(left.0, left.1),
                        left_id != ButtonId::Unavailable,
                    );
                    let right_id = match right.0 {
                        "options.accessibility.title" => ButtonId::Accessibility,
                        "options.sounds" => ButtonId::SoundSettings,
                        "options.controls" => ButtonId::Controls,
                        _ => ButtonId::Unavailable,
                    };
                    add(
                        right_id,
                        w / 2 + 5,
                        y,
                        150,
                        tr(right.0, right.1),
                        right_id != ButtonId::Unavailable,
                    );
                }
                add(
                    ButtonId::ChooseMinecraft,
                    x,
                    h - 52,
                    200,
                    "Minecraft Folder...".into(),
                    true,
                );
                add(ButtonId::Done, x, h - 28, 200, tr("gui.done", "Done"), true);
            }
            Screen::Controls => {
                let rect = mouse_ui::controls_mouse_button(w);
                add(
                    ButtonId::MouseSettings,
                    rect.x,
                    rect.y,
                    rect.w,
                    tr("options.mouse_settings", "Mouse Settings..."),
                    true,
                );
                let rect = SprintWindowUi::rect(w);
                add(
                    ButtonId::SprintWindow,
                    rect.x,
                    rect.y,
                    rect.w,
                    String::new(),
                    true,
                );
                for (id, toggle, fallback, enabled) in [
                    (
                        ButtonId::ToggleSneak,
                        ControlToggle::Sneak,
                        "Sneak",
                        self.state.video.toggle_sneak,
                    ),
                    (
                        ButtonId::ToggleSprint,
                        ControlToggle::Sprint,
                        "Sprint",
                        self.state.video.toggle_sprint,
                    ),
                    (
                        ButtonId::ToggleAttack,
                        ControlToggle::Attack,
                        "Attack/Destroy",
                        self.state.video.toggle_attack,
                    ),
                    (
                        ButtonId::ToggleUse,
                        ControlToggle::Use,
                        "Use Item/Place Block",
                        self.state.video.toggle_use,
                    ),
                ] {
                    let rect = mouse_ui::control_toggle_rect(w, toggle);
                    let label = self.state.assets.as_deref().map_or_else(
                        || format!("{fallback}: {}", if enabled { "Toggle" } else { "Hold" }),
                        |assets| mouse_ui::control_toggle_label(assets, toggle, self.state.video),
                    );
                    add(id, rect.x, rect.y, rect.w, label, true);
                }
                add(ButtonId::Done, x, h - 27, 200, tr("gui.done", "Done"), true);
            }
            Screen::Mouse => {
                for (id, control) in [
                    (ButtonId::MouseSensitivity, MouseControl::Sensitivity),
                    (
                        ButtonId::MouseWheelSensitivity,
                        MouseControl::WheelSensitivity,
                    ),
                    (
                        ButtonId::MouseDiscreteScroll,
                        MouseControl::DiscreteScrolling,
                    ),
                    (ButtonId::MouseInvertX, MouseControl::InvertX),
                    (ButtonId::MouseInvertY, MouseControl::InvertY),
                ] {
                    let rect = mouse_ui::mouse_control_rect(w, control);
                    add(id, rect.x, rect.y, rect.w, String::new(), true);
                }
                add(ButtonId::Done, x, h - 27, 200, tr("gui.done", "Done"), true);
            }
            Screen::Sound => {
                for category in SoundCategory::ALL {
                    let rect = self.sound_ui.rect(w, category);
                    add(
                        ButtonId::SoundSlider(category),
                        rect.x,
                        rect.y,
                        rect.w,
                        String::new(),
                        true,
                    );
                }
                add(ButtonId::Done, x, h - 27, 200, tr("gui.done", "Done"), true);
            }
            Screen::Video => {
                for (id, control) in [
                    (ButtonId::VideoFps, VideoControl::Framerate),
                    (ButtonId::VideoVsync, VideoControl::Vsync),
                    (ButtonId::VideoInactivity, VideoControl::Inactivity),
                    (ButtonId::VideoGuiScale, VideoControl::GuiScale),
                    (ButtonId::VideoFullscreen, VideoControl::Fullscreen),
                    (ButtonId::VideoBlend, VideoControl::BiomeBlend),
                    (ButtonId::VideoDistance, VideoControl::RenderDistance),
                    (ButtonId::VideoBlur, VideoControl::MenuBlur),
                    (ButtonId::VideoWeather, VideoControl::WeatherRadius),
                ] {
                    let rect = self.video_ui.control_rect(w, control);
                    add(
                        id,
                        rect.x,
                        rect.y,
                        rect.w,
                        String::new(),
                        matches!(
                            id,
                            ButtonId::VideoBlur
                                | ButtonId::VideoDistance
                                | ButtonId::VideoBlend
                                | ButtonId::VideoWeather
                        ) || self.video_ui.visible(h, rect),
                    );
                }
                add(ButtonId::Done, x, h - 27, 200, tr("gui.done", "Done"), true);
            }
            Screen::Accessibility => {
                for id in [
                    ButtonId::MenuBlur,
                    ButtonId::ViewBobbing,
                    ButtonId::FovEffectScale,
                    ButtonId::GlintSpeed,
                    ButtonId::GlintStrength,
                    ButtonId::PanoramaSpeed,
                ] {
                    let rect = self
                        .accessibility_ui
                        .control_rect(w, id.accessibility_control().unwrap());
                    add(id, rect.x, rect.y, rect.w, String::new(), true);
                }
                add(
                    ButtonId::Unavailable,
                    w / 2 - 155,
                    h - 27,
                    150,
                    tr("options.accessibility.link", "Accessibility Guide..."),
                    false,
                );
                add(
                    ButtonId::Done,
                    w / 2 + 5,
                    h - 27,
                    150,
                    tr("gui.done", "Done"),
                    true,
                );
            }
            Screen::Packs => {
                let selected = self
                    .state
                    .selected_pack
                    .is_some_and(|i| i < self.state.packs.len());
                add(
                    ButtonId::AddPackFile,
                    w / 2 - 154,
                    h - 76,
                    150,
                    "Add ZIP...".into(),
                    true,
                );
                add(
                    ButtonId::AddPackFolder,
                    w / 2 + 4,
                    h - 76,
                    150,
                    "Add Folder...".into(),
                    true,
                );
                add(
                    ButtonId::RemovePack,
                    w / 2 - 154,
                    h - 52,
                    98,
                    "Remove".into(),
                    selected,
                );
                add(
                    ButtonId::PackUp,
                    w / 2 - 52,
                    h - 52,
                    98,
                    "Move Up".into(),
                    self.state
                        .selected_pack
                        .is_some_and(|i| i + 1 < self.state.packs.len()),
                );
                add(
                    ButtonId::PackDown,
                    w / 2 + 50,
                    h - 52,
                    104,
                    "Move Down".into(),
                    self.state
                        .selected_pack
                        .is_some_and(|i| i > 0 && i < self.state.packs.len()),
                );
                add(ButtonId::Done, x, h - 28, 200, tr("gui.done", "Done"), true);
            }
            #[cfg(debug_assertions)]
            Screen::WorldSettings => {
                for (i, (id, name, value)) in [
                    (ButtonId::Dimension, "Dimension", &self.state.dimension),
                    (ButtonId::ChunkX, "Chunk X", &self.state.chunk_x),
                    (ButtonId::ChunkZ, "Chunk Z", &self.state.chunk_z),
                    (ButtonId::Radius, "Chunk radius", &self.state.radius),
                ]
                .into_iter()
                .enumerate()
                {
                    let value = if value.is_empty() { "(player)" } else { value };
                    add(
                        id,
                        x,
                        36 + i as i32 * 24,
                        200,
                        format!("{name}: {value}"),
                        true,
                    );
                }
                add(ButtonId::Done, x, h - 28, 200, tr("gui.done", "Done"), true);
            }
        }
        if self.state.asset_load.is_some() {
            for button in &mut result {
                if button.id != ButtonId::Quit {
                    button.active = false;
                }
            }
        }
        result
    }

    fn do_button(&mut self, id: ButtonId, event_loop: &ActiveEventLoop) {
        self.state.error = None;
        match id {
            ButtonId::Quit => {
                self.action = Some(Action::Quit);
                event_loop.exit();
            }
            ButtonId::Singleplayer => {
                self.state.refresh_worlds();
                self.state.screen = Screen::Worlds;
            }
            ButtonId::Options => self.state.screen = Screen::Options,
            ButtonId::SoundSettings => {
                self.sound_ui = SoundUi::default();
                self.state.screen = Screen::Sound;
                self.focused = None;
            }
            ButtonId::SoundSlider(category) => {
                let (width, _) = self.logical_size();
                self.state.video_dirty |=
                    self.sound_ui
                        .start(width, self.cursor.0, category, &mut self.state.sound);
                if let Some(audio) = &self.state.audio {
                    audio.set_gain(self.state.sound.ui_gain());
                }
            }
            ButtonId::OptionsFov => {
                let (width, _) = self.logical_size();
                self.state.video_dirty |=
                    self.fov_ui
                        .start(width, self.cursor.0, &mut self.state.video);
            }
            ButtonId::Controls => self.state.screen = Screen::Controls,
            ButtonId::SprintWindow => {
                let (width, _) = self.logical_size();
                self.state.video_dirty |=
                    self.sprint_window_ui
                        .start(width, self.cursor.0, &mut self.state.video);
            }
            ButtonId::ToggleSneak => {
                self.state.video.toggle_sneak = !self.state.video.toggle_sneak;
                self.state.video_dirty = true;
            }
            ButtonId::ToggleSprint => {
                self.state.video.toggle_sprint = !self.state.video.toggle_sprint;
                self.state.video_dirty = true;
            }
            ButtonId::ToggleAttack => {
                self.state.video.toggle_attack = !self.state.video.toggle_attack;
                self.state.video_dirty = true;
            }
            ButtonId::ToggleUse => {
                self.state.video.toggle_use = !self.state.video.toggle_use;
                self.state.video_dirty = true;
            }
            ButtonId::MouseSettings => {
                self.mouse_ui = MouseUi::default();
                self.state.screen = Screen::Mouse;
            }
            ButtonId::MouseSensitivity => {
                let (width, _) = self.logical_size();
                self.state.video_dirty |=
                    self.mouse_ui
                        .start(width, self.cursor.0, &mut self.state.video);
            }
            ButtonId::MouseWheelSensitivity => {
                let (width, _) = self.logical_size();
                self.state.video_dirty |= self.mouse_ui.start_control(
                    width,
                    self.cursor.0,
                    MouseControl::WheelSensitivity,
                    &mut self.state.video,
                );
            }
            ButtonId::MouseDiscreteScroll => {
                self.state.video.discrete_mouse_scroll = !self.state.video.discrete_mouse_scroll;
                self.state.video_dirty = true;
            }
            ButtonId::MouseInvertX => {
                self.state.video.invert_mouse_x = !self.state.video.invert_mouse_x;
                self.state.video_dirty = true;
            }
            ButtonId::MouseInvertY => {
                self.state.video.invert_mouse_y = !self.state.video.invert_mouse_y;
                self.state.video_dirty = true;
            }
            ButtonId::VideoSettings => {
                self.video_ui = VideoUi::new(self.state.video);
                self.state.screen = Screen::Video;
            }
            ButtonId::Accessibility => {
                self.accessibility_ui = crate::accessibility_ui::AccessibilityUi::default();
                self.state.screen = Screen::Accessibility;
            }
            ButtonId::ViewBobbing => {
                self.state.video.view_bobbing = !self.state.video.view_bobbing;
                self.state.video_dirty = true;
            }
            ButtonId::FovEffectScale
            | ButtonId::GlintSpeed
            | ButtonId::GlintStrength
            | ButtonId::PanoramaSpeed
            | ButtonId::MenuBlur => {
                let (width, _) = self.logical_size();
                self.state.video_dirty |= self.accessibility_ui.start_slider(
                    width,
                    self.cursor.0,
                    id.accessibility_control().unwrap(),
                    &mut self.state.video,
                );
            }
            ButtonId::VideoVsync => {
                self.state.video.vsync = !self.state.video.vsync;
                self.state.video_dirty = true;
            }
            ButtonId::VideoFullscreen => self.toggle_fullscreen(),
            ButtonId::VideoGuiScale => {
                self.state.video.gui_scale =
                    next_gui_scale(self.size.0, self.size.1, self.state.video.gui_scale);
                self.state.video_dirty = true;
                self.update_gui_scale();
            }
            ButtonId::VideoInactivity => {
                self.state.video.inactivity_fps_limit =
                    self.state.video.inactivity_fps_limit.next();
                self.state.video_dirty = true;
            }
            ButtonId::VideoFps => {
                let (width, _) = self.logical_size();
                if self
                    .video_ui
                    .start_slider(width, self.cursor.0, &mut self.state.video)
                {
                    self.state.video_dirty = true;
                }
            }
            ButtonId::VideoBlur => {
                let (width, _) = self.logical_size();
                self.state.video_dirty |= self.video_ui.start_control_slider(
                    width,
                    self.cursor.0,
                    VideoControl::MenuBlur,
                    &mut self.state.video,
                );
            }
            ButtonId::VideoBlend => {
                let (width, _) = self.logical_size();
                self.video_ui.start_control_slider(
                    width,
                    self.cursor.0,
                    VideoControl::BiomeBlend,
                    &mut self.state.video,
                );
            }
            ButtonId::VideoWeather => {
                let (width, _) = self.logical_size();
                self.state.video_dirty |= self.video_ui.start_control_slider(
                    width,
                    self.cursor.0,
                    VideoControl::WeatherRadius,
                    &mut self.state.video,
                );
            }
            ButtonId::VideoDistance => {
                let (width, _) = self.logical_size();
                self.video_ui.start_control_slider(
                    width,
                    self.cursor.0,
                    VideoControl::RenderDistance,
                    &mut self.state.video,
                );
            }
            ButtonId::Packs => {
                self.state.refresh_packs();
                self.state.screen = Screen::Packs;
            }
            #[cfg(debug_assertions)]
            ButtonId::WorldSettings => self.state.screen = Screen::WorldSettings,
            ButtonId::Done => {
                if self.state.screen == Screen::Video {
                    self.state.video_dirty |= self.video_ui.apply_pending_distance(
                        &mut self.state.video,
                        Instant::now(),
                        true,
                    );
                }
                if matches!(
                    self.state.screen,
                    Screen::Video
                        | Screen::Options
                        | Screen::Accessibility
                        | Screen::Sound
                        | Screen::Mouse
                        | Screen::Controls
                ) && self.state.video_dirty
                {
                    self.state.save_settings();
                }
                self.video_ui.release(self.state.video);
                self.accessibility_ui.release();
                self.fov_ui.release(self.state.video);
                self.sound_ui.release();
                self.mouse_ui.release();
                self.fov_ui.keyboard_adjust = false;
                self.state.screen = match self.state.screen {
                    Screen::Worlds | Screen::Options => Screen::Title,
                    Screen::RecoverWorld => {
                        self.recovery_world = None;
                        self.recovery_return_screen
                    }
                    Screen::Mouse => Screen::Controls,
                    Screen::Packs
                    | Screen::Video
                    | Screen::Accessibility
                    | Screen::Sound
                    | Screen::Controls => Screen::Options,
                    #[cfg(debug_assertions)]
                    Screen::WorldSettings => Screen::Worlds,
                    screen => screen,
                }
            }
            ButtonId::Demo => {
                if let Err(error) = self.begin_launch(LaunchRequest::Demo) {
                    self.state.error = Some(format!("{error:#}"));
                }
            }
            ButtonId::PlayWorld => {
                if let Err(error) = self.begin_launch(LaunchRequest::World { play: true }) {
                    self.state.error = Some(format!("{error:#}"));
                }
            }
            ButtonId::RecoverWorld => {
                if let Err(error) = self.begin_recovery() {
                    self.state.error = Some(format!("{error:#}"));
                }
            }
            #[cfg(debug_assertions)]
            ButtonId::PreviewWorld => {
                if let Err(error) = self.begin_launch(LaunchRequest::World { play: false }) {
                    self.state.error = Some(format!("{error:#}"));
                }
            }
            ButtonId::ChooseMinecraft
            | ButtonId::BrowseWorld
            | ButtonId::AddPackFile
            | ButtonId::AddPackFolder => self.open_dialog(id),
            ButtonId::RemovePack => {
                if let Some(index) = self.state.selected_pack
                    && index < self.state.packs.len()
                {
                    let mut packs = self.state.packs.clone();
                    packs.remove(index);
                    self.state.begin_asset_load(
                        self.state.minecraft_dir.clone(),
                        packs,
                        AssetLoadKind::ChangePacks(None),
                    );
                }
            }
            ButtonId::PackUp | ButtonId::PackDown => {
                if let Some(index) = self.state.selected_pack {
                    let other = if id == ButtonId::PackUp {
                        index.checked_add(1).filter(|i| *i < self.state.packs.len())
                    } else {
                        index.checked_sub(1)
                    };
                    if let Some(other) = other {
                        let mut packs = self.state.packs.clone();
                        packs.swap(index, other);
                        self.state.begin_asset_load(
                            self.state.minecraft_dir.clone(),
                            packs,
                            AssetLoadKind::ChangePacks(Some(other)),
                        );
                    }
                }
            }
            #[cfg(debug_assertions)]
            ButtonId::Dimension | ButtonId::ChunkX | ButtonId::ChunkZ | ButtonId::Radius => {
                self.focused = Some(id)
            }
            ButtonId::Multiplayer
            | ButtonId::Realms
            | ButtonId::CreateWorld
            | ButtonId::Unavailable => {}
        }
        #[cfg(debug_assertions)]
        let keep_focus = matches!(
            id,
            ButtonId::Dimension | ButtonId::ChunkX | ButtonId::ChunkZ | ButtonId::Radius
        );
        #[cfg(not(debug_assertions))]
        let keep_focus = false;
        if !keep_focus
            && id.accessibility_control().is_none()
            && !matches!(
                id,
                ButtonId::ToggleSneak
                    | ButtonId::ToggleSprint
                    | ButtonId::ToggleAttack
                    | ButtonId::ToggleUse
                    | ButtonId::SprintWindow
                    | ButtonId::VideoFps
                    | ButtonId::VideoBlur
                    | ButtonId::VideoDistance
                    | ButtonId::VideoBlend
                    | ButtonId::VideoWeather
                    | ButtonId::VideoVsync
                    | ButtonId::VideoFullscreen
                    | ButtonId::VideoGuiScale
                    | ButtonId::VideoInactivity
                    | ButtonId::OptionsFov
                    | ButtonId::ViewBobbing
            )
        {
            self.focused = None;
        }
        self.invalidate();
    }

    // Windows file dialogs have their own modal message loop. Running one inside a
    // winit callback can leave its event loop stuck when the dialog closes.
    fn open_dialog(&mut self, id: ButtonId) {
        if self.dialog.is_some() {
            return;
        }
        let Some(window) = self.window.clone() else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        self.dialog = Some(receiver);
        std::thread::spawn(move || {
            let dialog = rfd::FileDialog::new().set_parent(window.as_ref());
            let path = match id {
                ButtonId::ChooseMinecraft => dialog
                    .set_title("Select the Minecraft game folder")
                    .pick_folder(),
                ButtonId::BrowseWorld => dialog
                    .set_title("Select a copied Minecraft world folder")
                    .pick_folder(),
                ButtonId::AddPackFile => dialog
                    .set_title("Select a resource pack")
                    .add_filter("Resource pack", &["zip"])
                    .pick_file(),
                ButtonId::AddPackFolder => dialog
                    .set_title("Select a resource pack folder")
                    .pick_folder(),
                _ => None,
            };
            let _ = sender.send((id, path));
        });
    }

    fn receive_dialog(&mut self) {
        let result = match self.dialog.as_ref().map(mpsc::Receiver::try_recv) {
            Some(Ok(result)) => Some(result),
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.state.error = Some("Could not open the file chooser.".into());
                self.dialog = None;
                self.invalidate();
                None
            }
            _ => None,
        };
        let Some((id, path)) = result else { return };
        self.dialog = None;
        self.invalidate();
        if let Some(path) = path {
            match id {
                ButtonId::ChooseMinecraft => {
                    self.state
                        .begin_asset_load(path, Vec::new(), AssetLoadKind::ChooseDirectory);
                }
                ButtonId::BrowseWorld => self.state.browse_world(path),
                ButtonId::AddPackFile | ButtonId::AddPackFolder => self.state.add_pack(path),
                _ => {}
            }
        }
    }

    #[cfg(debug_assertions)]
    fn field(&mut self) -> Option<&mut String> {
        match self.focused? {
            ButtonId::Dimension => Some(&mut self.state.dimension),
            ButtonId::ChunkX => Some(&mut self.state.chunk_x),
            ButtonId::ChunkZ => Some(&mut self.state.chunk_z),
            ButtonId::Radius => Some(&mut self.state.radius),
            _ => None,
        }
    }

    #[cfg(not(debug_assertions))]
    fn field(&mut self) -> Option<&mut String> {
        None
    }

    fn redraw(&mut self) -> Result<()> {
        let (width, height) = self.size;
        if width == 0 || height == 0 {
            return Ok(());
        }
        if let Some(software) = &self.software {
            if self.software_scene.as_ref().is_none_or(|scene| {
                !scene.matches(
                    self.state.screen,
                    self.size,
                    self.scale,
                    self.state.video.menu_blur,
                    &self.state.assets,
                )
            }) {
                self.software_generation = self.software_generation.wrapping_add(1);
                self.software_scene = Some(SoftwareScene {
                    screen: self.state.screen,
                    size: self.size,
                    scale: self.scale,
                    blur: self.state.video.menu_blur,
                    assets: self.state.assets.clone(),
                });
            }
            if let Some(image) = software.take()?
                && image.size == self.size
                && image.generation == self.software_generation
            {
                let surface = self.surface.as_mut().context("Menu surface unavailable")?;
                surface
                    .resize(
                        NonZeroU32::new(width).unwrap(),
                        NonZeroU32::new(height).unwrap(),
                    )
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                let mut buffer = surface.buffer_mut().map_err(|e| anyhow::anyhow!("{e}"))?;
                for (dest, source) in buffer.iter_mut().zip(image.pixels) {
                    *dest = source & 0x00ff_ffff;
                }
                buffer.present().map_err(|e| anyhow::anyhow!("{e}"))?;
                #[cfg(test)]
                {
                    let count = self.software_presented.map_or(0, |(_, _, count)| count) + 1;
                    self.software_presented = Some((image.size, image.generation, count));
                }
            }
            // Completion requests a redraw only to present its image. Do not
            // start another animation frame until the frame pacer requests one.
            if !self.overlay_dirty && !self.software_frame_due {
                return Ok(());
            }
        }
        let requested_at = Instant::now();
        let speed = self.state.video.panorama_speed.value();
        let seconds = self.panorama.sample(requested_at, speed);
        let blurred = self.state.screen != Screen::Title && self.state.assets.is_some();
        if !self.overlay_dirty
            && let Some(gpu) = &mut self.gpu
        {
            return gpu.submit(MenuFrame {
                seconds,
                speed: speed as f32,
                requested_at,
                blurred,
                blur_radius: self.state.video.menu_blur,
                vsync: self.state.video.vsync,
                size: winit::dpi::PhysicalSize::new(width, height),
                overlay: None,
            });
        }
        if !self.overlay_dirty && self.software.is_some() {
            self.submit_software(seconds, blurred)?;
            return Ok(());
        }
        let (lw, lh) = self.logical_size();
        let mut canvas = Canvas::new(lw as u32, lh as u32);
        let assets = self.state.assets.as_deref();
        if let Some(a) = assets {
            if self.state.screen != Screen::Title {
                canvas.tiled(&a.background);
            }
        } else {
            canvas.fill(
                Rect {
                    x: 0,
                    y: 0,
                    w: lw,
                    h: lh,
                },
                [32, 32, 32, 255],
            );
        }

        match self.state.screen {
            Screen::RecoverWorld => {
                let title = assets.map_or("Restore interrupted save?", |a| {
                    a.translate(
                        "minecraft_native.recovery.title",
                        "Restore interrupted save?",
                    )
                });
                let message = assets.map_or("An interrupted save needs to be restored.", |a| {
                    a.translate(
                        "minecraft_native.recovery.message",
                        "An interrupted save needs to be restored.",
                    )
                });
                let backup = assets.map_or("The original backup will be kept.", |a| {
                    a.translate(
                        "minecraft_native.recovery.backup",
                        "The original backup will be kept.",
                    )
                });
                canvas.text_centered(assets, lw / 2, lh / 2 - 36, title, [255, 255, 255]);
                canvas.text_centered(assets, lw / 2, lh / 2 - 18, message, [255, 255, 255]);
                canvas.text_centered(assets, lw / 2, lh / 2 - 6, backup, [255, 255, 255]);
            }
            Screen::RecoveringWorld => {
                let message = assets.map_or("Restoring world...", |a| {
                    a.translate("minecraft_native.recovery.working", "Restoring world...")
                });
                canvas.text_centered(assets, lw / 2, lh / 2 - 10, message, [255, 255, 255]);
            }
            Screen::Launching => {
                let message = assets.map_or("Joining world...", |a| {
                    a.translate("connect.joining", "Joining world...")
                });
                canvas.text_centered(assets, lw / 2, lh / 2 - 10, message, [255, 255, 255]);
            }
            Screen::Loading => {
                canvas.text_centered(
                    assets,
                    lw / 2,
                    lh / 2 - 20,
                    "Loading Minecraft resources...",
                    [255, 255, 255],
                );
            }
            Screen::Title => {
                if let Some(a) = assets {
                    canvas.image(
                        &a.logo,
                        Rect {
                            x: lw / 2 - 128,
                            y: 20,
                            w: 256,
                            h: 64,
                        },
                    );
                    canvas.image(
                        &a.edition,
                        Rect {
                            x: lw / 2 - 64,
                            y: 64,
                            w: 128,
                            h: 16,
                        },
                    );
                    canvas.text(
                        assets,
                        2,
                        lh - 10,
                        concat!("Minecraft 26.3 (Re-Craft ", env!("CARGO_PKG_VERSION"), ")"),
                        [255, 255, 255],
                    );
                }
            }
            Screen::Missing => {
                canvas.text_centered(
                    assets,
                    lw / 2,
                    lh / 2 - 50,
                    "Minecraft 26.3 was not found",
                    [255, 255, 255],
                );
                canvas.text_centered(
                    assets,
                    lw / 2,
                    lh / 2 - 36,
                    "Choose your Minecraft game folder",
                    [255, 255, 255],
                );
            }
            Screen::Worlds => {
                let title = assets.map_or("Select World", |a| {
                    a.translate("selectWorld.title", "Select World")
                });
                canvas.text_centered(assets, lw / 2, 15, title, [255, 255, 255]);
                let list = Rect {
                    x: lw / 2 - 154,
                    y: 32,
                    w: 308,
                    h: (lh - 112).max(0),
                };
                canvas.fill(list, [0, 0, 0, 160]);
                if self.state.world_load.is_some() {
                    canvas.text_centered(
                        assets,
                        lw / 2,
                        list.y + 12,
                        "Loading worlds...",
                        [255, 255, 255],
                    );
                }
                let visible = (list.h / 36).max(0) as usize;
                for (row, (i, world)) in self
                    .state
                    .worlds
                    .iter()
                    .enumerate()
                    .skip(self.state.world_scroll)
                    .take(visible)
                    .enumerate()
                {
                    let rect = Rect {
                        x: list.x + 2,
                        y: list.y + row as i32 * 36,
                        w: list.w - 4,
                        h: 34,
                    };
                    if self.state.selected_world == Some(i) {
                        canvas.fill(rect, [120, 120, 120, 255]);
                        canvas.fill(
                            Rect {
                                x: rect.x + 1,
                                y: rect.y + 1,
                                w: rect.w - 2,
                                h: rect.h - 2,
                            },
                            [0, 0, 0, 255],
                        );
                    }
                    if let Some(icon) = &world.icon {
                        canvas.image(
                            icon,
                            Rect {
                                x: rect.x + 2,
                                y: rect.y + 1,
                                w: 32,
                                h: 32,
                            },
                        );
                    } else {
                        canvas.fill(
                            Rect {
                                x: rect.x + 2,
                                y: rect.y + 1,
                                w: 32,
                                h: 32,
                            },
                            [48, 48, 48, 255],
                        );
                    }
                    canvas.text(
                        assets,
                        rect.x + 39,
                        rect.y + 5,
                        &clip_text(&canvas, assets, &world.name, rect.w - 45),
                        [255, 255, 255],
                    );
                    let version = match world.data_version {
                        Some(5023) => "Minecraft 26.3".to_owned(),
                        Some(version) => format!("Data version: {version}"),
                        None => "Unable to read world data".to_owned(),
                    };
                    canvas.text(assets, rect.x + 39, rect.y + 17, &version, [160, 160, 160]);
                }
                if self.state.worlds.is_empty() && self.state.world_load.is_none() {
                    canvas.text_centered(
                        assets,
                        lw / 2,
                        45,
                        "No saved worlds found",
                        [180, 180, 180],
                    );
                }
                canvas.text_centered(
                    assets,
                    lw / 2,
                    lh - 88,
                    "Experimental: changes are not saved",
                    [255, 220, 140],
                );
            }
            Screen::Options => canvas.text_centered(
                assets,
                lw / 2,
                15,
                assets.map_or("Options", |a| a.translate("options.title", "Options")),
                [255, 255, 255],
            ),
            Screen::Sound => {
                canvas.text_centered(
                    assets,
                    lw / 2,
                    12,
                    assets.map_or("Music & Sound Options", |a| {
                        a.translate("options.sounds.title", "Music & Sound Options")
                    }),
                    [255, 255, 255],
                );
                if let Some(assets) = assets {
                    let focused = match self.focused {
                        Some(ButtonId::SoundSlider(category)) => Some(category),
                        _ => None,
                    };
                    self.sound_ui
                        .draw(&mut canvas, assets, self.state.sound, self.cursor, focused);
                }
            }
            Screen::Controls => {
                canvas.text_centered(
                    assets,
                    lw / 2,
                    12,
                    assets.map_or("Controls", |a| a.translate("controls.title", "Controls")),
                    [255, 255, 255],
                );
                if let Some(assets) = assets {
                    mouse_ui::draw_controls_placeholders(&mut canvas, assets);
                    self.sprint_window_ui.draw(
                        &mut canvas,
                        assets,
                        self.state.video,
                        self.cursor,
                        self.focused == Some(ButtonId::SprintWindow),
                    );
                }
            }
            Screen::Mouse => {
                canvas.text_centered(
                    assets,
                    lw / 2,
                    12,
                    assets.map_or("Mouse Settings", |a| {
                        a.translate("options.mouse_settings.title", "Mouse Settings")
                    }),
                    [255, 255, 255],
                );
                if let Some(assets) = assets {
                    self.mouse_ui.draw(
                        &mut canvas,
                        assets,
                        self.state.video,
                        self.cursor,
                        self.focused.and_then(ButtonId::mouse_control),
                    );
                }
            }
            Screen::Video => {
                canvas.text_centered(
                    assets,
                    lw / 2,
                    12,
                    assets.map_or("Video Settings", |a| {
                        a.translate("options.videoTitle", "Video Settings")
                    }),
                    [255, 255, 255],
                );
                if let Some(assets) = assets {
                    let focused = match self.focused {
                        Some(ButtonId::VideoFps) => Some(VideoControl::Framerate),
                        Some(ButtonId::VideoVsync) => Some(VideoControl::Vsync),
                        Some(ButtonId::VideoFullscreen) => Some(VideoControl::Fullscreen),
                        Some(ButtonId::VideoGuiScale) => Some(VideoControl::GuiScale),
                        Some(ButtonId::VideoInactivity) => Some(VideoControl::Inactivity),
                        Some(ButtonId::VideoBlur) => Some(VideoControl::MenuBlur),
                        Some(ButtonId::VideoDistance) => Some(VideoControl::RenderDistance),
                        Some(ButtonId::VideoBlend) => Some(VideoControl::BiomeBlend),
                        Some(ButtonId::VideoWeather) => Some(VideoControl::WeatherRadius),
                        _ => None,
                    };
                    self.video_ui
                        .draw(&mut canvas, assets, self.state.video, self.cursor, focused);
                }
            }
            Screen::Accessibility => {
                canvas.text_centered(
                    assets,
                    lw / 2,
                    12,
                    assets.map_or("Accessibility Settings", |a| {
                        a.translate("options.accessibility.title", "Accessibility Settings")
                    }),
                    [255, 255, 255],
                );
                if let Some(assets) = assets {
                    self.accessibility_ui.draw(
                        &mut canvas,
                        assets,
                        self.state.video,
                        self.cursor,
                        self.focused.and_then(ButtonId::accessibility_control),
                    );
                }
            }
            Screen::Packs => {
                canvas.text_centered(
                    assets,
                    lw / 2,
                    15,
                    assets.map_or("Select Resource Packs", |a| {
                        a.translate("resourcePack.title", "Select Resource Packs")
                    }),
                    [255, 255, 255],
                );
                let left = Rect {
                    x: lw / 2 - 154,
                    y: 43,
                    w: 150,
                    h: (lh - 125).max(0),
                };
                let right = Rect {
                    x: lw / 2 + 4,
                    y: 43,
                    w: 150,
                    h: (lh - 125).max(0),
                };
                canvas.text_centered(
                    assets,
                    left.x + left.w / 2,
                    32,
                    assets.map_or("Available", |a| {
                        a.translate("pack.available.title", "Available")
                    }),
                    [255, 255, 255],
                );
                canvas.text_centered(
                    assets,
                    right.x + right.w / 2,
                    32,
                    assets.map_or("Selected", |a| {
                        a.translate("pack.selected.title", "Selected")
                    }),
                    [255, 255, 255],
                );
                canvas.fill(left, [0, 0, 0, 130]);
                canvas.fill(right, [0, 0, 0, 130]);
                let visible = (left.h / 36).max(0) as usize;
                for (row, path) in self
                    .state
                    .available_packs
                    .iter()
                    .skip(self.state.available_pack_scroll)
                    .take(visible)
                    .enumerate()
                {
                    let rect = Rect {
                        x: left.x + 1,
                        y: left.y + row as i32 * 36,
                        w: left.w - 2,
                        h: 34,
                    };
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    draw_pack_row(
                        &mut canvas,
                        assets,
                        rect,
                        &name,
                        false,
                        rect.contains(self.cursor.0, self.cursor.1),
                        true,
                    );
                }
                for (row, index) in (0..self.state.packs.len())
                    .rev()
                    .skip(self.state.selected_pack_scroll)
                    .take(visible)
                    .enumerate()
                {
                    let rect = Rect {
                        x: right.x + 1,
                        y: right.y + row as i32 * 36,
                        w: right.w - 2,
                        h: 34,
                    };
                    let name = self.state.packs[index]
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy();
                    draw_pack_row(
                        &mut canvas,
                        assets,
                        rect,
                        &name,
                        self.state.selected_pack == Some(index),
                        rect.contains(self.cursor.0, self.cursor.1),
                        false,
                    );
                }
                let default_row = self
                    .state
                    .packs
                    .len()
                    .saturating_sub(self.state.selected_pack_scroll);
                if default_row < visible {
                    let rect = Rect {
                        x: right.x + 1,
                        y: right.y + default_row as i32 * 36,
                        w: right.w - 2,
                        h: 34,
                    };
                    let name = assets.map_or("Default", |a| {
                        a.translate("resourcePack.vanilla.name", "Default")
                    });
                    draw_pack_row(&mut canvas, assets, rect, name, false, false, false);
                }
            }
            #[cfg(debug_assertions)]
            Screen::WorldSettings => {
                canvas.text_centered(assets, lw / 2, 15, "World Settings", [255, 255, 255])
            }
        }
        for button in self.buttons() {
            if matches!(button.id, ButtonId::SoundSlider(_))
                || button.id == ButtonId::SprintWindow
                || button.id.mouse_control().is_some()
                || button.id.accessibility_control().is_some()
                || matches!(
                    button.id,
                    ButtonId::VideoFps
                        | ButtonId::VideoVsync
                        | ButtonId::VideoFullscreen
                        | ButtonId::VideoGuiScale
                        | ButtonId::VideoInactivity
                        | ButtonId::VideoBlur
                        | ButtonId::VideoDistance
                        | ButtonId::VideoBlend
                        | ButtonId::VideoWeather
                        | ButtonId::ViewBobbing
                )
            {
                continue;
            }
            if button.id == ButtonId::OptionsFov {
                self.fov_ui.draw(
                    &mut canvas,
                    assets,
                    self.state.video.fov,
                    self.cursor,
                    self.focused == Some(ButtonId::OptionsFov),
                );
                continue;
            }
            let highlighted = button.active
                && (button.rect.contains(self.cursor.0, self.cursor.1)
                    || self.focused == Some(button.id));
            canvas.button(
                assets,
                button.rect,
                button.active,
                highlighted,
                &button.label,
            );
        }
        if self.state.asset_load.is_some() && self.state.screen != Screen::Loading {
            canvas.text_centered(assets, lw / 2, 4, "Loading resources...", [255, 255, 255]);
        }
        if let Some(error) = &self.state.error {
            let message = clip_text(&canvas, assets, error, lw - 12);
            canvas.fill(
                Rect {
                    x: 0,
                    y: lh - 12,
                    w: lw,
                    h: 12,
                },
                [0, 0, 0, 210],
            );
            canvas.text_centered(assets, lw / 2, lh - 10, &message, [255, 130, 130]);
        }
        if let Some(gpu) = &mut self.gpu {
            gpu.submit(MenuFrame {
                seconds,
                speed: speed as f32,
                requested_at,
                blurred,
                blur_radius: self.state.video.menu_blur,
                vsync: self.state.video.vsync,
                size: winit::dpi::PhysicalSize::new(width, height),
                overlay: Some(MenuOverlay {
                    assets: self.state.assets.clone(),
                    canvas,
                }),
            })?;
            self.overlay_dirty = false;
            return Ok(());
        }
        self.software_overlay = Some(Arc::new(canvas));
        self.submit_software(seconds, blurred)?;
        self.overlay_dirty = false;
        Ok(())
    }

    fn submit_software(&mut self, seconds: f32, blurred: bool) -> Result<()> {
        self.software
            .as_ref()
            .context("Software menu renderer unavailable")?
            .submit(SoftwareFrame {
                size: self.size,
                scale: self.scale,
                generation: self.software_generation,
                seconds,
                blurred,
                blur_radius: self.state.video.menu_blur,
                assets: self.state.assets.clone(),
                overlay: self
                    .software_overlay
                    .as_ref()
                    .context("Menu overlay unavailable")?
                    .clone(),
            })?;
        self.software_frame_due = false;
        Ok(())
    }
}

impl ApplicationHandler for Menu<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attributes = Window::default_attributes()
            .with_title("Minecraft 26.3")
            .with_fullscreen(crate::fullscreen_mode(self.state.video.fullscreen))
            .with_inner_size(winit::dpi::LogicalSize::new(1280, 720))
            .with_min_inner_size(winit::dpi::LogicalSize::new(320, 240));
        let result = (|| -> Result<_> {
            let window = Arc::new(event_loop.create_window(attributes)?);
            let gpu = match MenuGpuWorker::new(Arc::clone(&window), self.state.video.vsync) {
                Ok(renderer) => Some(renderer),
                Err(error) => {
                    eprintln!("Menu GPU unavailable; using software drawing: {error:#}");
                    None
                }
            };
            let surface = if gpu.is_none() {
                let context =
                    softbuffer::Context::new(window.clone()).map_err(|e| anyhow::anyhow!("{e}"))?;
                Some(
                    softbuffer::Surface::new(&context, window.clone())
                        .map_err(|e| anyhow::anyhow!("{e}"))?,
                )
            } else {
                None
            };
            Ok((window, surface, gpu))
        })();
        match result {
            Ok((window, surface, gpu)) => {
                let size = window.inner_size();
                self.size = (size.width, size.height);
                self.update_gui_scale();
                self.refresh_interval = monitor_frame_interval(&window);
                if gpu.is_none() {
                    match MenuSoftwareWorker::new(window.clone()) {
                        Ok(software) => self.software = Some(software),
                        Err(error) => {
                            self.fail(event_loop, error);
                            return;
                        }
                    }
                }
                self.window = Some(window);
                self.surface = surface;
                self.gpu = gpu;
            }
            Err(error) => self.fail(event_loop, error),
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self.window.as_ref().is_none_or(|w| w.id() != window_id) {
            return;
        }
        if matches!(
            event,
            WindowEvent::KeyboardInput { .. }
                | WindowEvent::MouseInput { .. }
                | WindowEvent::MouseWheel { .. }
                | WindowEvent::CursorMoved { .. }
        ) {
            let now = Instant::now();
            if self
                .inactivity
                .input(now, self.state.video.inactivity_fps_limit)
            {
                self.next_frame = now;
            }
        }
        let mut request_redraw = !matches!(event, WindowEvent::RedrawRequested);
        match event {
            WindowEvent::CloseRequested => {
                if self.state.video_dirty {
                    self.state.save_settings();
                }
                self.action = Some(Action::Quit);
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                self.size = (size.width, size.height);
                self.update_gui_scale();
            }
            WindowEvent::Occluded(value) => self.occluded = value,
            WindowEvent::Moved(_) => {
                if let Some(window) = &self.window {
                    self.refresh_interval = monitor_frame_interval(window);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                // Both hit tests use the same layout. Rebuilding every label
                // twice for each high-frequency pointer event is unnecessary.
                let buttons = self.buttons();
                let previous = self.hover_target(&buttons, self.cursor);
                self.cursor_raw = Some((position.x, position.y));
                self.cursor = (
                    (position.x / self.scale as f64) as i32,
                    (position.y / self.scale as f64) as i32,
                );
                let (dragged, changed) = if self.state.screen == Screen::Controls {
                    let (w, _) = self.logical_size();
                    self.sprint_window_ui
                        .pointer_move(w, self.cursor.0, &mut self.state.video)
                } else if self.state.screen == Screen::Mouse {
                    let (w, _) = self.logical_size();
                    self.mouse_ui
                        .pointer_move(w, self.cursor.0, &mut self.state.video)
                } else if self.state.screen == Screen::Video {
                    let (w, h) = self.logical_size();
                    self.video_ui
                        .pointer_move(w, h, self.cursor, &mut self.state.video)
                } else if self.state.screen == Screen::Accessibility {
                    let (w, h) = self.logical_size();
                    self.accessibility_ui.pointer_move_with_settings(
                        w,
                        h,
                        self.cursor,
                        &mut self.state.video,
                    )
                } else if self.state.screen == Screen::Sound {
                    let (w, h) = self.logical_size();
                    self.sound_ui
                        .pointer_move(w, h, self.cursor, &mut self.state.sound)
                } else if self.state.screen == Screen::Options {
                    let (w, _) = self.logical_size();
                    self.fov_ui
                        .pointer_move(w, self.cursor.0, &mut self.state.video)
                } else {
                    (false, false)
                };
                self.state.video_dirty |= changed;
                if changed
                    && self.state.screen == Screen::Sound
                    && let Some(audio) = &self.state.audio
                {
                    audio.set_gain(self.state.sound.ui_gain());
                }
                request_redraw = dragged || previous != self.hover_target(&buttons, self.cursor);
            }
            WindowEvent::CursorLeft { .. } => {
                let buttons = self.buttons();
                let previous = self.hover_target(&buttons, self.cursor);
                self.cursor = (-1, -1);
                self.cursor_raw = None;
                request_redraw = previous != self.hover_target(&buttons, self.cursor);
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                // Returning to a window can deliver a click without another
                // CursorMoved event at the same screen coordinate.
                if (self.cursor.0 < 0 || self.cursor.1 < 0)
                    && let Some(window) = &self.window
                    && let Some(cursor) = current_window_cursor(window, self.size, self.scale)
                {
                    self.cursor = cursor;
                    self.cursor_raw = Some((
                        f64::from(cursor.0 * self.scale as i32),
                        f64::from(cursor.1 * self.scale as i32),
                    ));
                }
                let (w, h) = self.logical_size();
                let scrollbar = self.state.screen == Screen::Video
                    && self.state.asset_load.is_none()
                    && self.video_ui.start_scrollbar(w, h, self.cursor)
                    || self.state.screen == Screen::Accessibility
                        && self.state.asset_load.is_none()
                        && self.accessibility_ui.start_scrollbar(w, h, self.cursor)
                    || self.state.screen == Screen::Sound
                        && self.state.asset_load.is_none()
                        && self.sound_ui.start_scrollbar(w, h, self.cursor);
                if scrollbar {
                    self.focused = None;
                } else if let Some(button) = self
                    .buttons()
                    .into_iter()
                    .find(|b| b.active && self.button_contains(b, self.cursor))
                {
                    self.slider_mouse_pressed = matches!(button.id, ButtonId::SoundSlider(_))
                        || matches!(
                            button.id,
                            ButtonId::SprintWindow
                                | ButtonId::VideoFps
                                | ButtonId::MouseSensitivity
                                | ButtonId::MouseWheelSensitivity
                                | ButtonId::VideoBlur
                                | ButtonId::VideoDistance
                                | ButtonId::VideoBlend
                                | ButtonId::VideoWeather
                                | ButtonId::OptionsFov
                                | ButtonId::FovEffectScale
                                | ButtonId::GlintSpeed
                                | ButtonId::GlintStrength
                                | ButtonId::PanoramaSpeed
                                | ButtonId::MenuBlur
                        );
                    if !self.slider_mouse_pressed {
                        self.play_button_sound();
                    }
                    if matches!(button.id, ButtonId::SoundSlider(_))
                        || matches!(
                            button.id,
                            ButtonId::ToggleSneak
                                | ButtonId::ToggleSprint
                                | ButtonId::ToggleAttack
                                | ButtonId::ToggleUse
                                | ButtonId::SprintWindow
                                | ButtonId::VideoFps
                                | ButtonId::VideoBlur
                                | ButtonId::VideoDistance
                                | ButtonId::VideoBlend
                                | ButtonId::VideoWeather
                                | ButtonId::VideoVsync
                                | ButtonId::VideoFullscreen
                                | ButtonId::VideoGuiScale
                                | ButtonId::VideoInactivity
                                | ButtonId::OptionsFov
                                | ButtonId::ViewBobbing
                                | ButtonId::FovEffectScale
                                | ButtonId::GlintSpeed
                                | ButtonId::GlintStrength
                                | ButtonId::PanoramaSpeed
                                | ButtonId::MenuBlur
                        )
                    {
                        self.focused = Some(button.id);
                    }
                    self.do_button(button.id, event_loop);
                } else if self.state.asset_load.is_none() {
                    let (w, h) = self.logical_size();
                    if self.state.screen == Screen::Worlds
                        && self.state.world_load.is_none()
                        && self.cursor.0 >= w / 2 - 154
                        && self.cursor.0 < w / 2 + 154
                        && self.cursor.1 >= 32
                        && self.cursor.1 < h - 80
                    {
                        let index = self.state.world_scroll + ((self.cursor.1 - 32) / 36) as usize;
                        if index < self.state.worlds.len() {
                            self.state.selected_world = Some(index);
                            self.focused = None;
                            let now = Instant::now();
                            let double_click = self.last_world_click.is_some_and(|(old, at)| {
                                old == index
                                    && now.saturating_duration_since(at)
                                        < Duration::from_millis(350)
                            });
                            self.last_world_click = Some((index, now));
                            if double_click {
                                self.do_button(ButtonId::PlayWorld, event_loop);
                            }
                        }
                    } else if self.state.screen == Screen::Packs
                        && self.cursor.1 >= 43
                        && self.cursor.1 < h - 82
                    {
                        let row = ((self.cursor.1 - 43) / 36) as usize;
                        if self.cursor.0 >= w / 2 - 154 && self.cursor.0 < w / 2 - 4 {
                            let index = self.state.available_pack_scroll + row;
                            if let Some(path) = self.state.available_packs.get(index).cloned() {
                                self.state.add_pack(path);
                            }
                        } else if self.cursor.0 >= w / 2 + 4 && self.cursor.0 < w / 2 + 154 {
                            let index = self.state.selected_pack_scroll + row;
                            if index < self.state.packs.len() {
                                self.state.selected_pack = Some(self.state.packs.len() - 1 - index);
                                if self.cursor.0 < w / 2 + 39 {
                                    self.do_button(ButtonId::RemovePack, event_loop);
                                }
                            }
                        }
                    } else {
                        self.focused = None;
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => {
                if self.slider_mouse_pressed {
                    self.play_button_sound();
                    self.slider_mouse_pressed = false;
                }
                self.video_ui.release(self.state.video);
                self.accessibility_ui.release();
                self.fov_ui.release(self.state.video);
                self.sound_ui.release();
                self.mouse_ui.release();
                self.sprint_window_ui.release();
            }
            WindowEvent::Focused(false) => {
                self.slider_mouse_pressed = false;
                self.video_ui.release(self.state.video);
                self.accessibility_ui.release();
                self.fov_ui.release(self.state.video);
                self.sound_ui.release();
                self.mouse_ui.release();
                self.sprint_window_ui.release();
            }
            WindowEvent::MouseWheel { delta, .. }
                if self.state.screen == Screen::Sound && self.state.asset_load.is_none() =>
            {
                let amount =
                    crate::mouse_scroll::scaled_delta(crate::scroll_lines(delta), self.state.video);
                let (_, h) = self.logical_size();
                self.sound_ui.scroll_by(amount, h);
            }
            WindowEvent::MouseWheel { delta, .. }
                if self.state.screen == Screen::Accessibility
                    && self.state.asset_load.is_none() =>
            {
                let amount =
                    crate::mouse_scroll::scaled_delta(crate::scroll_lines(delta), self.state.video);
                let (_, h) = self.logical_size();
                self.accessibility_ui.scroll_by(amount, h);
            }
            WindowEvent::MouseWheel { delta, .. }
                if self.state.screen == Screen::Video && self.state.asset_load.is_none() =>
            {
                let amount =
                    crate::mouse_scroll::scaled_delta(crate::scroll_lines(delta), self.state.video);
                let (_, h) = self.logical_size();
                self.video_ui.scroll_by(amount, h);
            }
            WindowEvent::MouseWheel { delta, .. }
                if self.state.screen == Screen::Worlds && self.state.asset_load.is_none() =>
            {
                let amount =
                    crate::mouse_scroll::scaled_delta(crate::scroll_lines(delta), self.state.video);
                let steps = take_scroll_steps(&mut self.state.world_scroll_remainder, amount);
                if steps > 0 {
                    self.state.world_scroll =
                        self.state.world_scroll.saturating_sub(steps as usize);
                } else if steps < 0 {
                    self.state.world_scroll = self
                        .state
                        .world_scroll
                        .saturating_add(steps.unsigned_abs() as usize)
                        .min(self.state.worlds.len().saturating_sub(1));
                }
            }
            WindowEvent::MouseWheel { delta, .. }
                if self.state.screen == Screen::Packs && self.state.asset_load.is_none() =>
            {
                let amount =
                    crate::mouse_scroll::scaled_delta(crate::scroll_lines(delta), self.state.video);
                let (w, _) = self.logical_size();
                let available = self.cursor.0 < w / 2;
                let steps = if available {
                    take_scroll_steps(&mut self.state.available_pack_scroll_remainder, amount)
                } else {
                    take_scroll_steps(&mut self.state.selected_pack_scroll_remainder, amount)
                };
                let (scroll, max) = if available {
                    (
                        &mut self.state.available_pack_scroll,
                        self.state.available_packs.len().saturating_sub(1),
                    )
                } else {
                    (&mut self.state.selected_pack_scroll, self.state.packs.len())
                };
                if steps > 0 {
                    *scroll = scroll.saturating_sub(steps as usize);
                } else if steps < 0 {
                    *scroll = scroll
                        .saturating_add(steps.unsigned_abs() as usize)
                        .min(max);
                }
            }
            WindowEvent::KeyboardInput { event, .. }
                if crate::fullscreen_shortcut(event.physical_key, event.state, event.repeat) =>
            {
                self.toggle_fullscreen();
                self.state.save_settings();
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && self.state.asset_load.is_none() =>
            {
                let code = match event.physical_key {
                    PhysicalKey::Code(code) => Some(code),
                    _ => None,
                };
                if code == Some(KeyCode::Escape) {
                    if self.field().is_some() {
                        self.focused = None;
                    } else if self.state.screen != Screen::Title
                        && self.state.screen != Screen::Missing
                    {
                        self.do_button(ButtonId::Done, event_loop);
                    }
                } else if self.state.screen == Screen::Worlds
                    && self.state.world_load.is_none()
                    && self.focused.is_none()
                    && matches!(code, Some(KeyCode::ArrowUp | KeyCode::ArrowDown))
                    && !self.state.worlds.is_empty()
                {
                    let last = self.state.worlds.len() - 1;
                    let next = match self.state.selected_world {
                        None => 0,
                        Some(current) if code == Some(KeyCode::ArrowUp) => {
                            current.saturating_sub(1)
                        }
                        Some(current) => (current + 1).min(last),
                    };
                    self.state.selected_world = Some(next);
                    let visible = ((self.logical_size().1 - 112) / 36).max(1) as usize;
                    if next < self.state.world_scroll {
                        self.state.world_scroll = next;
                    }
                    if next >= self.state.world_scroll + visible {
                        self.state.world_scroll = next + 1 - visible;
                    }
                } else if self.state.screen == Screen::Controls
                    && self.focused == Some(ButtonId::SprintWindow)
                    && self.sprint_window_ui.keyboard_adjust
                    && matches!(code, Some(KeyCode::ArrowLeft | KeyCode::ArrowRight))
                {
                    self.state.video_dirty |= self
                        .sprint_window_ui
                        .arrow(code == Some(KeyCode::ArrowRight), &mut self.state.video);
                } else if self.state.screen == Screen::Mouse
                    && self
                        .focused
                        .and_then(ButtonId::mouse_control)
                        .is_some_and(|control| {
                            matches!(
                                control,
                                MouseControl::Sensitivity | MouseControl::WheelSensitivity
                            )
                        })
                    && self.mouse_ui.keyboard_adjust
                    && matches!(code, Some(KeyCode::ArrowLeft | KeyCode::ArrowRight))
                {
                    self.state.video_dirty |= self.mouse_ui.arrow(
                        self.focused.and_then(ButtonId::mouse_control).unwrap(),
                        code == Some(KeyCode::ArrowRight),
                        &mut self.state.video,
                    );
                } else if self.state.screen == Screen::Sound
                    && self.sound_ui.keyboard_adjust
                    && matches!(code, Some(KeyCode::ArrowLeft | KeyCode::ArrowRight))
                    && let Some(ButtonId::SoundSlider(category)) = self.focused
                {
                    let changed = self.sound_ui.arrow(
                        category,
                        code == Some(KeyCode::ArrowRight),
                        &mut self.state.sound,
                    );
                    self.state.video_dirty |= changed;
                    if changed && let Some(audio) = &self.state.audio {
                        audio.set_gain(self.state.sound.ui_gain());
                    }
                } else if self.state.screen == Screen::Options
                    && self.focused == Some(ButtonId::OptionsFov)
                    && self.fov_ui.keyboard_adjust
                    && matches!(code, Some(KeyCode::ArrowLeft | KeyCode::ArrowRight))
                {
                    self.state.video_dirty |= self
                        .fov_ui
                        .arrow(code == Some(KeyCode::ArrowRight), &mut self.state.video);
                } else if self.state.screen == Screen::Video
                    && self.focused == Some(ButtonId::VideoWeather)
                    && self.video_ui.keyboard_adjust
                    && matches!(code, Some(KeyCode::ArrowLeft | KeyCode::ArrowRight))
                {
                    self.state.video_dirty |= self
                        .video_ui
                        .weather_arrow(code == Some(KeyCode::ArrowRight), &mut self.state.video);
                } else if self.state.screen == Screen::Video
                    && self.focused == Some(ButtonId::VideoBlend)
                    && self.video_ui.keyboard_adjust
                    && matches!(code, Some(KeyCode::ArrowLeft | KeyCode::ArrowRight))
                {
                    self.video_ui.blend_arrow(code == Some(KeyCode::ArrowRight));
                } else if self.state.screen == Screen::Video
                    && self.focused == Some(ButtonId::VideoDistance)
                    && self.video_ui.keyboard_adjust
                    && matches!(code, Some(KeyCode::ArrowLeft | KeyCode::ArrowRight))
                {
                    self.video_ui
                        .distance_arrow(code == Some(KeyCode::ArrowRight));
                } else if self.state.screen == Screen::Video
                    && self.focused == Some(ButtonId::VideoBlur)
                    && self.video_ui.keyboard_adjust
                    && matches!(code, Some(KeyCode::ArrowLeft | KeyCode::ArrowRight))
                {
                    self.state.video_dirty |= self
                        .video_ui
                        .blur_arrow(code == Some(KeyCode::ArrowRight), &mut self.state.video);
                } else if self.state.screen == Screen::Video
                    && self.focused == Some(ButtonId::VideoFps)
                    && self.video_ui.keyboard_adjust
                    && matches!(code, Some(KeyCode::ArrowLeft | KeyCode::ArrowRight))
                {
                    if self
                        .video_ui
                        .arrow(code == Some(KeyCode::ArrowRight), &mut self.state.video)
                    {
                        self.state.video_dirty = true;
                    }
                } else if self.state.screen == Screen::Accessibility
                    && self.accessibility_ui.keyboard_adjust
                    && matches!(code, Some(KeyCode::ArrowLeft | KeyCode::ArrowRight))
                    && let Some(control) = self.focused.and_then(ButtonId::accessibility_control)
                {
                    self.state.video_dirty |= self.accessibility_ui.arrow(
                        control,
                        code == Some(KeyCode::ArrowRight),
                        &mut self.state.video,
                    );
                } else if code == Some(KeyCode::Tab) {
                    let buttons: Vec<_> = self
                        .buttons()
                        .into_iter()
                        .filter(|b| b.active)
                        .map(|b| b.id)
                        .collect();
                    if !buttons.is_empty() {
                        let next = self
                            .focused
                            .and_then(|id| buttons.iter().position(|b| *b == id))
                            .map_or(0, |i| (i + 1) % buttons.len());
                        self.focused = Some(buttons[next]);
                        if self.state.screen == Screen::Video
                            && matches!(
                                self.focused,
                                Some(
                                    ButtonId::VideoBlur
                                        | ButtonId::VideoDistance
                                        | ButtonId::VideoBlend
                                        | ButtonId::VideoWeather
                                )
                            )
                        {
                            let (_, h) = self.logical_size();
                            self.video_ui.ensure_control_visible(
                                h,
                                if self.focused == Some(ButtonId::VideoBlur) {
                                    VideoControl::MenuBlur
                                } else if self.focused == Some(ButtonId::VideoBlend) {
                                    VideoControl::BiomeBlend
                                } else if self.focused == Some(ButtonId::VideoWeather) {
                                    VideoControl::WeatherRadius
                                } else {
                                    VideoControl::RenderDistance
                                },
                            );
                        }
                        if self.state.screen == Screen::Accessibility
                            && let Some(control) =
                                self.focused.and_then(ButtonId::accessibility_control)
                        {
                            let (_, h) = self.logical_size();
                            self.accessibility_ui.ensure_control_visible(h, control);
                        }
                        if self.state.screen == Screen::Sound
                            && let Some(ButtonId::SoundSlider(category)) = self.focused
                        {
                            let (_, h) = self.logical_size();
                            self.sound_ui.ensure_visible(h, category);
                        }
                        self.video_ui.keyboard_adjust = matches!(
                            self.focused,
                            Some(
                                ButtonId::VideoFps
                                    | ButtonId::VideoBlur
                                    | ButtonId::VideoDistance
                                    | ButtonId::VideoBlend
                                    | ButtonId::VideoWeather
                            )
                        );
                        self.fov_ui.keyboard_adjust = self.focused == Some(ButtonId::OptionsFov);
                        self.sprint_window_ui.keyboard_adjust =
                            self.focused == Some(ButtonId::SprintWindow);
                        self.mouse_ui.keyboard_adjust = self.focused.is_some_and(|id| {
                            matches!(
                                id.mouse_control(),
                                Some(MouseControl::Sensitivity | MouseControl::WheelSensitivity)
                            )
                        });
                        self.accessibility_ui.keyboard_adjust = matches!(
                            self.focused,
                            Some(
                                ButtonId::GlintSpeed
                                    | ButtonId::FovEffectScale
                                    | ButtonId::GlintStrength
                                    | ButtonId::PanoramaSpeed
                                    | ButtonId::MenuBlur
                            )
                        );
                        self.sound_ui.keyboard_adjust =
                            matches!(self.focused, Some(ButtonId::SoundSlider(_)));
                    }
                } else if code == Some(KeyCode::Backspace) {
                    if let Some(field) = self.field() {
                        field.pop();
                    }
                } else if matches!(
                    code,
                    Some(KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Space)
                ) {
                    if let Some(id) = self.focused
                        && self.buttons().iter().any(|b| b.id == id && b.active)
                    {
                        if id == ButtonId::SprintWindow {
                            self.sprint_window_ui.keyboard_adjust =
                                !self.sprint_window_ui.keyboard_adjust;
                        } else if matches!(
                            id,
                            ButtonId::MouseSensitivity | ButtonId::MouseWheelSensitivity
                        ) {
                            self.mouse_ui.keyboard_adjust = !self.mouse_ui.keyboard_adjust;
                        } else if matches!(id, ButtonId::SoundSlider(_)) {
                            self.sound_ui.keyboard_adjust = !self.sound_ui.keyboard_adjust;
                        } else if id == ButtonId::OptionsFov {
                            self.fov_ui.keyboard_adjust = !self.fov_ui.keyboard_adjust;
                        } else if matches!(
                            id,
                            ButtonId::VideoFps
                                | ButtonId::VideoBlur
                                | ButtonId::VideoDistance
                                | ButtonId::VideoBlend
                                | ButtonId::VideoWeather
                        ) {
                            self.video_ui.keyboard_adjust = !self.video_ui.keyboard_adjust;
                        } else if matches!(
                            id,
                            ButtonId::GlintSpeed
                                | ButtonId::FovEffectScale
                                | ButtonId::GlintStrength
                                | ButtonId::PanoramaSpeed
                                | ButtonId::MenuBlur
                        ) {
                            self.accessibility_ui.keyboard_adjust =
                                !self.accessibility_ui.keyboard_adjust;
                        } else {
                            self.play_button_sound();
                            self.do_button(id, event_loop);
                        }
                    } else if self.state.screen == Screen::Worlds
                        && self.state.world_load.is_none()
                        && self.state.chosen_world().is_some()
                    {
                        self.do_button(ButtonId::PlayWorld, event_loop);
                    }
                } else if let Some(text) = event.text
                    && let Some(field) = self.field()
                {
                    for c in text.chars().filter(|c| !c.is_control()) {
                        if field.len() < 100 {
                            field.push(c);
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                #[cfg(any(debug_assertions, test))]
                let frame_start = Instant::now();
                let result = self.redraw();
                #[cfg(any(debug_assertions, test))]
                if self.state.assets.is_some()
                    && !self.occluded
                    && self.size.0 > 0
                    && self.size.1 > 0
                    && let Some(timings) = &mut self.frame_timings
                {
                    timings.record(
                        self.state.screen,
                        frame_start,
                        frame_start.elapsed(),
                        self.refresh_interval,
                        self.size,
                        self.gpu.is_some(),
                    );
                }
                if let Err(error) = result {
                    if self.gpu.is_some() {
                        match self.use_software_renderer() {
                            Ok(()) => {
                                self.state.error = Some(format!(
                                    "Graphics device unavailable; using software rendering: {error:#}"
                                ));
                                self.invalidate();
                            }
                            Err(fallback) => self.fail(
                                event_loop,
                                fallback.context(format!("Menu GPU failed: {error:#}")),
                            ),
                        }
                    } else {
                        self.fail(event_loop, error);
                    }
                }
            }
            _ => {}
        }
        if request_redraw {
            self.invalidate();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.screen == Screen::Video {
            self.state.video_dirty |=
                self.video_ui
                    .apply_pending_distance(&mut self.state.video, Instant::now(), false);
        }
        // An already requested close must win over a concurrently completed load.
        if self.action.is_some() {
            return;
        }
        if let (Some(gpu), Some(window)) = (&self.gpu, &self.window) {
            gpu.service_surfaces(window);
        }
        if let Some(error) = self.gpu.as_ref().and_then(MenuGpuWorker::take_error) {
            match self.use_software_renderer() {
                Ok(()) => {
                    self.state.error = Some(format!(
                        "Graphics device unavailable; using software rendering: {error:#}"
                    ));
                    self.invalidate();
                }
                Err(fallback) => {
                    self.fail(
                        event_loop,
                        fallback.context(format!("Menu GPU failed: {error:#}")),
                    );
                    return;
                }
            }
        }
        self.receive_dialog();
        if self.state.poll_asset_load() | self.state.poll_world_load() | self.state.poll_pack_load()
        {
            self.invalidate();
        }
        self.poll_launch(event_loop);
        self.poll_recovery();
        if self.action.is_some() {
            return;
        }
        if self.animation_active() {
            let now = Instant::now();
            let fps = self.inactivity.limit(
                now,
                self.state.video.inactivity_fps_limit,
                self.state.video.max_fps,
                true,
                false,
            );
            let interval = Duration::from_nanos(1_000_000_000 / u64::from(fps));
            if now >= self.next_frame {
                self.next_frame =
                    crate::frame_pacer::advance_frame_deadline(self.next_frame, now, interval);
                if let Some(window) = &self.window {
                    self.software_frame_due = true;
                    window.request_redraw();
                }
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_frame));
        } else if self.dialog.is_some()
            || self.state.asset_load.is_some()
            || self.state.world_load.is_some()
            || self.state.pack_load.is_some()
            || self.pending_launch.is_some()
            || self.pending_recovery.is_some()
        {
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(50),
            ));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}

impl Drop for Menu<'_> {
    fn drop(&mut self) {
        self.state.panorama_seconds = self.panorama.seconds;
        // A detached renderer can retain the window until a stalled driver
        // returns; hide it before entering another screen or quitting.
        if let Some(window) = &self.window {
            window.set_visible(false);
        }
    }
}

// Java 26.3's FramerateLimitTracker caps a screen outside a world at 60 FPS.
#[cfg(any(debug_assertions, test))]
pub(crate) const MENU_FRAME_INTERVAL: Duration = Duration::from_nanos(1_000_000_000 / 60);

#[cfg(test)]
fn expand_canvas(canvas: &Canvas, destination: &mut [u32], width: u32, scale: u32) {
    let source_width = canvas.width as usize;
    let source_height = canvas.height as usize;
    for (y, row) in destination.chunks_mut(width as usize).enumerate() {
        let source_y = (y / scale as usize).min(source_height - 1);
        let source = &canvas.pixels[source_y * source_width..(source_y + 1) * source_width];
        for (x, pixels) in row.chunks_mut(scale as usize).enumerate() {
            pixels.fill(source[x.min(source_width - 1)] & 0x00ff_ffff);
        }
    }
}

#[cfg(test)]
pub(crate) fn gui_scale(width: u32, height: u32) -> u32 {
    gui_scale_for(width, height, 0)
}

pub(crate) fn gui_scale_for(width: u32, height: u32, requested: u32) -> u32 {
    let mut scale = 1;
    while scale != requested && width / (scale + 1) >= 320 && height / (scale + 1) >= 240 {
        scale += 1;
    }
    scale
}

pub(crate) fn next_gui_scale(width: u32, height: u32, current: u32) -> u32 {
    if current >= gui_scale_for(width, height, 0) {
        0
    } else {
        current + 1
    }
}

fn logical_cursor(
    screen: (i32, i32),
    origin: (i32, i32),
    size: (u32, u32),
    scale: u32,
) -> Option<(i32, i32)> {
    let x = i64::from(screen.0) - i64::from(origin.0);
    let y = i64::from(screen.1) - i64::from(origin.1);
    if scale == 0 || x < 0 || y < 0 || x >= i64::from(size.0) || y >= i64::from(size.1) {
        return None;
    }
    Some(((x / i64::from(scale)) as i32, (y / i64::from(scale)) as i32))
}

#[cfg(windows)]
fn current_window_cursor(window: &Window, size: (u32, u32), scale: u32) -> Option<(i32, i32)> {
    use windows_sys::Win32::{Foundation::POINT, UI::WindowsAndMessaging::GetCursorPos};
    let mut point = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut point) } == 0 {
        return None;
    }
    let origin = window.inner_position().ok()?;
    logical_cursor((point.x, point.y), (origin.x, origin.y), size, scale)
}

#[cfg(not(windows))]
fn current_window_cursor(_window: &Window, _size: (u32, u32), _scale: u32) -> Option<(i32, i32)> {
    None
}

fn monitor_frame_interval(window: &Window) -> Duration {
    let refresh = window
        .current_monitor()
        .and_then(|monitor| monitor.refresh_rate_millihertz())
        .filter(|rate| (1000..=1_000_000).contains(rate))
        .unwrap_or(60_000);
    Duration::from_nanos(1_000_000_000_000 / u64::from(refresh))
}

fn clip_text(canvas: &Canvas, assets: Option<&MenuAssets>, text: &str, width: i32) -> String {
    if canvas.text_width(assets, text) <= width {
        return text.to_owned();
    }
    let mut result = String::new();
    for ch in text.chars() {
        let mut next = result.clone();
        next.push(ch);
        if canvas.text_width(assets, &format!("{next}...")) > width {
            break;
        }
        result = next;
    }
    result.push_str("...");
    result
}

fn draw_pack_row(
    canvas: &mut Canvas,
    assets: Option<&MenuAssets>,
    rect: Rect,
    name: &str,
    selected: bool,
    hovered: bool,
    available: bool,
) {
    if selected || hovered {
        canvas.fill(rect, [100, 100, 100, 255]);
    }
    if let Some(assets) = assets {
        let icon = if hovered {
            if available {
                &assets.select_pack
            } else {
                &assets.unselect_pack
            }
        } else {
            &assets.unknown_pack
        };
        canvas.image(
            icon,
            Rect {
                x: rect.x + 1,
                y: rect.y + 1,
                w: 32,
                h: 32,
            },
        );
    }
    canvas.text(
        assets,
        rect.x + 37,
        rect.y + 6,
        &clip_text(canvas, assets, name, rect.w - 40),
        [255, 255, 255],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_cursor_uses_client_coordinates_and_gui_scale() {
        assert_eq!(
            logical_cursor((-1270, 110), (-1280, 100), (1280, 720), 3),
            Some((3, 3))
        );
        assert_eq!(
            logical_cursor((-1, 819), (-1280, 100), (1280, 720), 3),
            Some((426, 239))
        );
        assert_eq!(
            logical_cursor((-1281, 100), (-1280, 100), (1280, 720), 3),
            None
        );
        assert_eq!(logical_cursor((0, 100), (-1280, 100), (1280, 720), 3), None);
        assert_eq!(
            logical_cursor((-1280, 100), (-1280, 100), (1280, 720), 0),
            None
        );
    }

    #[test]
    fn locked_world_errors_are_distinguished_from_other_io_failures() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = State::new(temp.path().to_path_buf(), Vec::new());
        state.asset_load = None;
        let menu = Menu::new(&mut state);
        let locked =
            anyhow::Error::new(native_persistence::save::WorldInUse).context("OS lock failure");
        assert_eq!(menu.world_error_text(&locked), "World is already in use.");
        assert!(
            menu.world_error_text(&anyhow::anyhow!("permission denied"))
                .contains("permission denied")
        );
    }

    #[cfg(windows)]
    #[test]
    fn failed_recovery_returns_to_confirmation_without_altering_pending_record() {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/recovery-menu-tests");
        std::fs::create_dir_all(&parent).unwrap();
        let world = tempfile::Builder::new()
            .prefix("world-")
            .tempdir_in(parent)
            .unwrap();
        let marker = world.path().join(".minecraft-native-pending");
        std::fs::write(&marker, b"transaction-missing").unwrap();
        let mut state = State::new(world.path().to_path_buf(), Vec::new());
        state.asset_load = None;
        state.screen = Screen::RecoverWorld;
        let mut menu = Menu::new(&mut state);
        menu.size = (1280, 720);
        menu.scale = gui_scale(1280, 720);
        menu.recovery_world = Some(world.path().to_path_buf());
        let buttons = menu.buttons();
        assert_eq!(
            buttons.iter().map(|b| b.id).collect::<Vec<_>>(),
            vec![ButtonId::RecoverWorld, ButtonId::Done]
        );
        assert!(buttons.iter().all(|b| b.active));
        menu.begin_recovery().unwrap();
        assert_eq!(menu.state.screen, Screen::RecoveringWorld);
        assert!(menu.buttons().is_empty());
        let deadline = Instant::now() + Duration::from_secs(3);
        while menu.pending_recovery.is_some() && Instant::now() < deadline {
            menu.poll_recovery();
            std::thread::yield_now();
        }
        assert!(
            menu.pending_recovery.is_none(),
            "background recovery timed out"
        );
        assert_eq!(menu.state.screen, Screen::RecoverWorld);
        assert!(menu.state.error.is_some());
        assert_eq!(menu.focused, Some(ButtonId::Done));
        assert_eq!(std::fs::read(marker).unwrap(), b"transaction-missing");
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "performance measurement requiring Minecraft 26.3 assets and a Windows GPU window"]
    fn measure_title_window_pacing() {
        if !crate::window_test::isolated("menu::tests::measure_title_window_pacing") {
            return;
        }
        use winit::platform::windows::EventLoopBuilderExtWindows;

        struct Harness<'a> {
            menu: Menu<'a>,
            start: Instant,
            last: Option<Instant>,
            intervals: Vec<Duration>,
            first_rendered: Option<Duration>,
        }
        impl ApplicationHandler for Harness<'_> {
            fn resumed(&mut self, event_loop: &ActiveEventLoop) {
                self.menu.resumed(event_loop);
                if std::env::var_os("NATIVE_MENU_TEST_1080").is_some()
                    && let Some(window) = &self.menu.window
                {
                    let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(1920, 1080));
                }
                self.start = Instant::now();
            }
            fn window_event(
                &mut self,
                event_loop: &ActiveEventLoop,
                id: WindowId,
                event: WindowEvent,
            ) {
                if matches!(event, WindowEvent::RedrawRequested) {
                    let now = Instant::now();
                    if self.start.elapsed() >= Duration::from_secs(1)
                        && let Some(last) = self.last
                    {
                        self.intervals.push(now - last);
                    }
                    self.last = Some(now);
                }
                self.menu.window_event(event_loop, id, event);
            }
            fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
                self.menu.about_to_wait(event_loop);
                if self.first_rendered.is_none()
                    && self
                        .menu
                        .gpu
                        .as_ref()
                        .is_some_and(|gpu| gpu.completed_frames() > 0)
                {
                    self.first_rendered = Some(self.start.elapsed());
                }
                if self.start.elapsed() >= Duration::from_secs(7) {
                    event_loop.exit();
                }
            }
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&game, &[]).unwrap());
        let mut state = State::new(game, Vec::new());
        state.asset_load = None;
        state.assets = Some(assets);
        state.screen = if std::env::var_os("NATIVE_MENU_TEST_OPTIONS").is_some() {
            Screen::Options
        } else {
            Screen::Title
        };
        let mut harness = Harness {
            menu: Menu::new(&mut state),
            start: Instant::now(),
            last: None,
            intervals: Vec::new(),
            first_rendered: None,
        };
        let mut builder = EventLoop::builder();
        builder.with_any_thread(true);
        builder.build().unwrap().run_app(&mut harness).unwrap();
        harness.intervals.sort_unstable();
        assert!(
            harness.intervals.len() > 100,
            "insufficient title frames: {}",
            harness.intervals.len()
        );
        if let Some(gpu) = &harness.menu.gpu {
            assert!(
                gpu.completed_frames() > 100,
                "GPU did not present enough frames"
            );
            assert!(
                harness.first_rendered.is_some(),
                "first GPU frame was not observed"
            );
        }
        let n = harness.intervals.len();
        println!(
            "Menu CPU redraw intervals: screen={:?}, gpu={}, completed={}, first-render-observed={:?}, vsync={}, refresh={:?}, count={n}, median={:?}, p95={:?}, p99={:?}, max={:?}; assets were preloaded and intervals exclude display scanout",
            harness.menu.state.screen,
            harness.menu.gpu.is_some(),
            harness
                .menu
                .gpu
                .as_ref()
                .map_or(0, MenuGpuWorker::completed_frames),
            harness.first_rendered,
            harness.menu.state.video.vsync,
            harness.menu.refresh_interval,
            harness.intervals[n / 2],
            harness.intervals[n * 95 / 100],
            harness.intervals[n * 99 / 100],
            harness.intervals[n - 1]
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires a Windows desktop; briefly opens a test window"]
    fn world_preparation_keeps_the_loading_window_responsive_and_reports_failure() {
        if !crate::window_test::isolated(
            "menu::tests::world_preparation_keeps_the_loading_window_responsive_and_reports_failure",
        ) {
            return;
        }
        use winit::platform::windows::EventLoopBuilderExtWindows;

        struct Harness<'a> {
            menu: Menu<'a>,
            release: Option<mpsc::Sender<()>>,
            start: Instant,
            loading_cycles: usize,
            loading_redraws: usize,
            recovered: bool,
        }

        impl ApplicationHandler for Harness<'_> {
            fn resumed(&mut self, event_loop: &ActiveEventLoop) {
                self.menu.resumed(event_loop);
                self.start = Instant::now();
            }

            fn window_event(
                &mut self,
                event_loop: &ActiveEventLoop,
                id: WindowId,
                event: WindowEvent,
            ) {
                if matches!(event, WindowEvent::RedrawRequested)
                    && self.menu.state.screen == Screen::Launching
                {
                    self.loading_redraws += 1;
                }
                self.menu.window_event(event_loop, id, event);
            }

            fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
                let elapsed = self.start.elapsed();
                if self.menu.state.screen == Screen::Launching && self.release.is_some() {
                    self.loading_cycles += 1;
                }
                if elapsed >= Duration::from_millis(300)
                    && let Some(release) = self.release.take()
                {
                    let _ = release.send(());
                }
                self.menu.about_to_wait(event_loop);
                if self.menu.state.screen == Screen::Worlds
                    && self.menu.state.error.as_deref() == Some("synthetic world load failure")
                {
                    self.recovered = true;
                    event_loop.exit();
                } else if elapsed >= Duration::from_secs(5) {
                    event_loop.exit();
                } else {
                    event_loop.set_control_flow(ControlFlow::WaitUntil(
                        Instant::now() + Duration::from_millis(15),
                    ));
                }
            }
        }

        let directory = tempfile::tempdir().unwrap();
        let mut state = State::new(directory.path().to_path_buf(), Vec::new());
        state.asset_load = None;
        state.screen = Screen::Launching;
        let (release, gate) = mpsc::channel();
        let result = spawn_launch(move || {
            gate.recv_timeout(Duration::from_secs(3))
                .context("test gate timed out")?;
            Err(anyhow::anyhow!("synthetic world load failure"))
        })
        .unwrap();
        let mut menu = Menu::new(&mut state);
        menu.pending_launch = Some(PendingLaunch {
            result,
            return_screen: Screen::Worlds,
            world: None,
        });
        let mut harness = Harness {
            menu,
            release: Some(release),
            start: Instant::now(),
            loading_cycles: 0,
            loading_redraws: 0,
            recovered: false,
        };
        let mut builder = EventLoop::builder();
        builder.with_any_thread(true);
        builder.build().unwrap().run_app(&mut harness).unwrap();
        assert!(
            harness.recovered,
            "load failure should return to the world list"
        );
        assert!(
            harness.loading_cycles > 5,
            "event loop stopped during world loading"
        );
        assert!(harness.loading_redraws > 0, "loading window did not redraw");
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires a Windows desktop; briefly opens a test window"]
    fn gpu_initialization_failure_switches_to_software_without_blocking_the_window() {
        if !crate::window_test::isolated(
            "menu::tests::gpu_initialization_failure_switches_to_software_without_blocking_the_window",
        ) {
            return;
        }
        use winit::platform::windows::EventLoopBuilderExtWindows;
        struct Harness<'a> {
            menu: Menu<'a>,
            start: Instant,
            recovered: bool,
        }
        impl ApplicationHandler for Harness<'_> {
            fn resumed(&mut self, event_loop: &ActiveEventLoop) {
                self.menu.resumed(event_loop);
                self.start = Instant::now();
                let window = self.menu.window.as_ref().unwrap().clone();
                self.menu.gpu = Some(
                    MenuGpuWorker::new_with_initialization(window, true, || {
                        Err(anyhow::anyhow!("synthetic GPU initialization failure"))
                    })
                    .unwrap(),
                );
            }
            fn window_event(
                &mut self,
                event_loop: &ActiveEventLoop,
                id: WindowId,
                event: WindowEvent,
            ) {
                self.menu.window_event(event_loop, id, event);
            }
            fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
                self.menu.about_to_wait(event_loop);
                if self.menu.gpu.is_none()
                    && self.menu.surface.is_some()
                    && self
                        .menu
                        .state
                        .error
                        .as_deref()
                        .is_some_and(|error| error.contains("synthetic GPU initialization failure"))
                {
                    self.recovered = true;
                    event_loop.exit();
                } else if self.start.elapsed() > Duration::from_secs(5) {
                    event_loop.exit();
                } else {
                    event_loop.set_control_flow(ControlFlow::WaitUntil(
                        Instant::now() + Duration::from_millis(15),
                    ));
                }
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let mut state = State::new(directory.path().to_path_buf(), Vec::new());
        state.asset_load = None;
        state.screen = Screen::Missing;
        let mut harness = Harness {
            menu: Menu::new(&mut state),
            start: Instant::now(),
            recovered: false,
        };
        let mut builder = EventLoop::builder();
        builder.with_any_thread(true);
        builder.build().unwrap().run_app(&mut harness).unwrap();
        assert!(
            harness.recovered,
            "menu did not recover from GPU initialization failure: {:?}",
            harness.menu.state.error
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires installed 26.3 assets and a Windows desktop"]
    fn software_menu_presents_after_screen_change_and_resize() {
        if !crate::window_test::isolated(
            "menu::tests::software_menu_presents_after_screen_change_and_resize",
        ) {
            return;
        }
        use winit::{
            dpi::{PhysicalPosition, PhysicalSize},
            event::DeviceId,
            platform::windows::EventLoopBuilderExtWindows,
        };

        struct Harness<'a> {
            menu: Menu<'a>,
            start: Instant,
            resized: bool,
            presented: bool,
            pointer_started: Option<(Instant, usize)>,
            pointer_progress: bool,
            cycles: usize,
        }
        impl ApplicationHandler for Harness<'_> {
            fn resumed(&mut self, event_loop: &ActiveEventLoop) {
                self.menu.resumed(event_loop);
                self.menu.use_software_renderer().unwrap();
                self.menu.invalidate();
                self.start = Instant::now();
            }
            fn window_event(
                &mut self,
                event_loop: &ActiveEventLoop,
                id: WindowId,
                event: WindowEvent,
            ) {
                self.menu.window_event(event_loop, id, event);
            }
            fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
                self.menu.about_to_wait(event_loop);
                self.cycles += 1;
                if !self.resized && self.menu.software_presented.is_some() {
                    self.resized = true;
                    self.menu.state.screen = Screen::Options;
                    self.menu.invalidate();
                    let _ = self
                        .menu
                        .window
                        .as_ref()
                        .unwrap()
                        .request_inner_size(PhysicalSize::new(901, 601));
                }
                if self.resized
                    && self
                        .menu
                        .software_presented
                        .is_some_and(|(size, generation, count)| {
                            size == (901, 601)
                                && generation == self.menu.software_generation
                                && count >= 2
                        })
                {
                    self.presented = true;
                    if self.pointer_started.is_none() {
                        self.pointer_started =
                            Some((Instant::now(), self.menu.software_presented.unwrap().2));
                    }
                }
                if let Some((started, initial_frames)) = self.pointer_started {
                    let window = self.menu.window.as_ref().unwrap().id();
                    self.menu.window_event(
                        event_loop,
                        window,
                        WindowEvent::CursorMoved {
                            device_id: DeviceId::dummy(),
                            position: PhysicalPosition::new(
                                (self.cycles % 200) as f64 + 200.0,
                                300.0,
                            ),
                        },
                    );
                    if started.elapsed() >= Duration::from_millis(1200) {
                        self.pointer_progress =
                            self.menu.software_presented.unwrap().2 > initial_frames;
                        event_loop.exit();
                        return;
                    }
                }
                if self.start.elapsed() > Duration::from_secs(15) {
                    event_loop.exit();
                } else {
                    event_loop.set_control_flow(ControlFlow::WaitUntil(
                        Instant::now() + Duration::from_millis(10),
                    ));
                }
            }
        }
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&directory, &[]).unwrap());
        let mut state = State::new(directory, Vec::new());
        state.asset_load = None;
        state.assets = Some(assets);
        state.screen = Screen::Title;
        let mut harness = Harness {
            menu: Menu::new(&mut state),
            start: Instant::now(),
            resized: false,
            presented: false,
            pointer_started: None,
            pointer_progress: false,
            cycles: 0,
        };
        let mut builder = EventLoop::builder();
        builder.with_any_thread(true);
        builder.build().unwrap().run_app(&mut harness).unwrap();
        assert!(
            harness.presented,
            "software frame was not presented after resize; last={:?}",
            harness.menu.software_presented
        );
        assert!(
            harness.cycles > 5,
            "window loop did not continue while drawing"
        );
        assert!(
            harness.pointer_progress,
            "software frames stopped while pointer events continued"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 assets and a Windows GPU window"]
    fn demo_prepares_in_the_menu_then_opens_a_playable_window() {
        if !crate::window_test::isolated(
            "menu::tests::demo_prepares_in_the_menu_then_opens_a_playable_window",
        ) {
            return;
        }
        use winit::platform::windows::EventLoopBuilderExtWindows;

        struct Harness<'a> {
            menu: Menu<'a>,
            started: Instant,
            loading_cycles: usize,
            timed_out: bool,
        }

        impl ApplicationHandler for Harness<'_> {
            fn resumed(&mut self, event_loop: &ActiveEventLoop) {
                self.menu.resumed(event_loop);
                self.started = Instant::now();
                self.menu.begin_launch(LaunchRequest::Demo).unwrap();
                assert_eq!(self.menu.state.screen, Screen::Launching);
            }

            fn window_event(
                &mut self,
                event_loop: &ActiveEventLoop,
                id: WindowId,
                event: WindowEvent,
            ) {
                self.menu.window_event(event_loop, id, event);
            }

            fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
                if self.menu.state.screen == Screen::Launching {
                    self.loading_cycles += 1;
                }
                self.menu.about_to_wait(event_loop);
                if self.started.elapsed() > Duration::from_secs(30) {
                    self.timed_out = true;
                    event_loop.exit();
                }
            }
        }

        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&game, &[]).unwrap());
        let mut state = State::new(game, Vec::new());
        state.asset_load = None;
        state.assets = Some(assets);
        state.screen = Screen::Worlds;
        let mut harness = Harness {
            menu: Menu::new(&mut state),
            started: Instant::now(),
            loading_cycles: 0,
            timed_out: false,
        };
        let mut builder = EventLoop::builder();
        builder.with_any_thread(true);
        let mut event_loop = builder.build().unwrap();
        event_loop.run_app_on_demand(&mut harness).unwrap();
        assert!(!harness.timed_out, "demo preparation timed out");
        assert!(
            harness.loading_cycles > 0,
            "the loading event loop never ran"
        );
        let action = harness
            .menu
            .action
            .take()
            .expect("demo preparation must succeed");
        drop(harness);
        match action {
            Action::Launch {
                prepared,
                return_screen: Screen::Worlds,
            } => match *prepared {
                PreparedLaunch::Demo {
                    atlas,
                    content,
                    assets,
                    video,
                } => {
                    crate::run_demo(&mut event_loop, atlas, true, assets, content, video, None)
                        .unwrap();
                }
                PreparedLaunch::World(_) => panic!("expected the demo"),
            },
            _ => panic!("unexpected menu action"),
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 assets and a Windows GPU window"]
    fn selected_world_prepares_in_menu_without_modifying_files() {
        if !crate::window_test::isolated(
            "menu::tests::selected_world_prepares_in_menu_without_modifying_files",
        ) {
            return;
        }
        use flate2::{
            Compression,
            write::{GzEncoder, ZlibEncoder},
        };
        use native_persistence::nbt::{self, NamedTag, TagType};
        use native_session::GameSession;
        use std::io::Write;
        use winit::platform::windows::EventLoopBuilderExtWindows;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let uuid = "00000000-0000-0000-0000-000000000000";
        let level = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "Data".into(),
                Tag::Compound(vec![
                    ("DataVersion".into(), Tag::Int(5023)),
                    ("singleplayer_uuid".into(), Tag::IntArray(vec![0, 0, 0, 0])),
                ]),
            )]),
        };
        let player = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                (
                    "Dimension".into(),
                    Tag::String("minecraft:overworld".into()),
                ),
                (
                    "Pos".into(),
                    Tag::List {
                        element_type: TagType::Double,
                        values: vec![Tag::Double(8.5), Tag::Double(-47.0), Tag::Double(8.5)],
                    },
                ),
                (
                    "Rotation".into(),
                    Tag::List {
                        element_type: TagType::Float,
                        values: vec![Tag::Float(0.0), Tag::Float(0.0)],
                    },
                ),
            ]),
        };
        let gzip = |tag: &NamedTag| {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&nbt::write(tag).unwrap()).unwrap();
            encoder.finish().unwrap()
        };
        let level_bytes = gzip(&level);
        let player_bytes = gzip(&player);
        let player_path = root.join("players/data").join(format!("{uuid}.dat"));
        std::fs::create_dir_all(player_path.parent().unwrap()).unwrap();
        std::fs::write(root.join("level.dat"), &level_bytes).unwrap();
        std::fs::write(&player_path, &player_bytes).unwrap();
        let chunk = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                ("xPos".into(), Tag::Int(0)),
                ("zPos".into(), Tag::Int(0)),
                ("Status".into(), Tag::String("minecraft:full".into())),
                (
                    "sections".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![Tag::Compound(vec![
                            ("Y".into(), Tag::Byte(-4)),
                            (
                                "block_states".into(),
                                Tag::Compound(vec![(
                                    "palette".into(),
                                    Tag::List {
                                        element_type: TagType::String,
                                        values: vec![Tag::String("minecraft:stone".into())],
                                    },
                                )]),
                            ),
                        ])],
                    },
                ),
            ]),
        };
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&nbt::write(&chunk).unwrap()).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut region = vec![0u8; 3 * 4096];
        region[..4].copy_from_slice(&((2u32 << 8) | 1).to_be_bytes());
        region[8192..8196].copy_from_slice(&((compressed.len() + 1) as u32).to_be_bytes());
        region[8196] = 2;
        region[8197..8197 + compressed.len()].copy_from_slice(&compressed);
        let region_path = anvil::overworld_region_path(root, 0, 0);
        std::fs::create_dir_all(region_path.parent().unwrap()).unwrap();
        std::fs::write(&region_path, &region).unwrap();

        struct Harness<'a> {
            menu: Menu<'a>,
            started: Instant,
            loading_cycles: usize,
            timed_out: bool,
        }
        impl ApplicationHandler for Harness<'_> {
            fn resumed(&mut self, event_loop: &ActiveEventLoop) {
                self.menu.resumed(event_loop);
                self.started = Instant::now();
                self.menu
                    .begin_launch(LaunchRequest::World { play: true })
                    .unwrap();
                assert_eq!(self.menu.state.screen, Screen::Launching);
            }
            fn window_event(
                &mut self,
                event_loop: &ActiveEventLoop,
                id: WindowId,
                event: WindowEvent,
            ) {
                self.menu.window_event(event_loop, id, event);
            }
            fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
                if self.menu.state.screen == Screen::Launching {
                    self.loading_cycles += 1;
                }
                self.menu.about_to_wait(event_loop);
                if self.menu.state.error.is_some() && self.menu.pending_launch.is_none() {
                    event_loop.exit();
                }
                if self.started.elapsed() > Duration::from_secs(30) {
                    self.timed_out = true;
                    event_loop.exit();
                }
            }
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&game, &[]).unwrap());
        let mut state = State::new(game, Vec::new());
        state.asset_load = None;
        state.assets = Some(assets);
        state.screen = Screen::Worlds;
        state.worlds.push(WorldEntry {
            path: root.to_path_buf(),
            name: "Test".into(),
            data_version: Some(5023),
            last_played: None,
            icon: None,
        });
        state.selected_world = Some(0);
        let mut harness = Harness {
            menu: Menu::new(&mut state),
            started: Instant::now(),
            loading_cycles: 0,
            timed_out: false,
        };
        let mut builder = EventLoop::builder();
        builder.with_any_thread(true);
        let mut event_loop = builder.build().unwrap();
        event_loop.run_app_on_demand(&mut harness).unwrap();
        assert!(!harness.timed_out, "world preparation timed out");
        assert!(harness.loading_cycles > 0);
        assert!(
            harness.menu.state.error.is_none(),
            "{:?}",
            harness.menu.state.error
        );
        let action = harness
            .menu
            .action
            .take()
            .expect("world preparation must succeed");
        drop(harness);
        match action {
            Action::Launch {
                prepared,
                return_screen: Screen::Worlds,
            } => match *prepared {
                PreparedLaunch::World(world) => match world {
                    crate::PreparedWorld::Play(world) => {
                        assert!(world.session.snapshot().world.loaded().is_some())
                    }
                    #[cfg(debug_assertions)]
                    crate::PreparedWorld::Preview { .. } => panic!("expected world play"),
                },
                PreparedLaunch::Demo { .. } => panic!("expected a saved world"),
            },
            _ => panic!("unexpected menu action"),
        }
        assert_eq!(std::fs::read(root.join("level.dat")).unwrap(), level_bytes);
        assert_eq!(std::fs::read(&player_path).unwrap(), player_bytes);
        assert_eq!(std::fs::read(&region_path).unwrap(), region);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 assets and a Windows GPU window"]
    fn video_options_mouse_flow_saves_and_reloads_settings() {
        if !crate::window_test::isolated(
            "menu::tests::video_options_mouse_flow_saves_and_reloads_settings",
        ) {
            return;
        }
        use winit::{
            dpi::PhysicalPosition,
            event::{DeviceId, ElementState, MouseButton, WindowEvent},
            platform::windows::EventLoopBuilderExtWindows,
        };

        struct Harness<'a> {
            menu: Menu<'a>,
            ran: bool,
        }
        impl ApplicationHandler for Harness<'_> {
            fn resumed(&mut self, event_loop: &ActiveEventLoop) {
                self.menu.resumed(event_loop);
            }
            fn window_event(
                &mut self,
                event_loop: &ActiveEventLoop,
                id: WindowId,
                event: WindowEvent,
            ) {
                self.menu.window_event(event_loop, id, event);
            }
            fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
                if self.ran || self.menu.window.is_none() {
                    return;
                }
                self.ran = true;
                fn click(
                    menu: &mut Menu<'_>,
                    event_loop: &ActiveEventLoop,
                    id: ButtonId,
                    right_edge: bool,
                ) {
                    let button = menu
                        .buttons()
                        .into_iter()
                        .find(|button| button.id == id && button.active)
                        .unwrap();
                    let x = if right_edge {
                        button.rect.x + button.rect.w - 2
                    } else {
                        button.rect.x + button.rect.w / 2
                    };
                    let y = button.rect.y + button.rect.h / 2;
                    let window = menu.window.as_ref().unwrap().id();
                    // DeviceId::dummy is specifically intended for synthetic unit-test events.
                    let device_id = DeviceId::dummy();
                    menu.window_event(
                        event_loop,
                        window,
                        WindowEvent::CursorMoved {
                            device_id,
                            position: PhysicalPosition::new(
                                f64::from(x * menu.scale as i32),
                                f64::from(y * menu.scale as i32),
                            ),
                        },
                    );
                    for state in [ElementState::Pressed, ElementState::Released] {
                        menu.window_event(
                            event_loop,
                            window,
                            WindowEvent::MouseInput {
                                device_id,
                                state,
                                button: MouseButton::Left,
                            },
                        );
                    }
                }
                click(&mut self.menu, event_loop, ButtonId::Options, false);
                click(&mut self.menu, event_loop, ButtonId::OptionsFov, true);
                assert_eq!(self.menu.state.video.fov, 110);
                click(&mut self.menu, event_loop, ButtonId::Done, false);
                assert_eq!(self.menu.state.screen, Screen::Title);
                let path = self.menu.state.settings_path.as_ref().unwrap();
                let saved: Settings =
                    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
                assert_eq!(saved.video.fov, 110);
                click(&mut self.menu, event_loop, ButtonId::Options, false);
                click(&mut self.menu, event_loop, ButtonId::Controls, false);
                assert_eq!(self.menu.state.screen, Screen::Controls);
                self.menu.redraw().unwrap();
                click(&mut self.menu, event_loop, ButtonId::ToggleSneak, false);
                click(&mut self.menu, event_loop, ButtonId::ToggleSprint, false);
                click(&mut self.menu, event_loop, ButtonId::ToggleAttack, false);
                click(&mut self.menu, event_loop, ButtonId::ToggleUse, false);
                assert!(
                    self.menu.state.video.toggle_sneak
                        && self.menu.state.video.toggle_sprint
                        && self.menu.state.video.toggle_attack
                        && self.menu.state.video.toggle_use
                );
                click(&mut self.menu, event_loop, ButtonId::SprintWindow, true);
                assert_eq!(self.menu.state.video.sprint_window, 10);
                assert_eq!(self.menu.focused, Some(ButtonId::SprintWindow));
                assert!(self.menu.sprint_window_ui.keyboard_adjust);
                click(&mut self.menu, event_loop, ButtonId::Done, false);
                let saved: Settings = serde_json::from_slice(
                    &std::fs::read(self.menu.state.settings_path.as_ref().unwrap()).unwrap(),
                )
                .unwrap();
                assert_eq!(saved.video.sprint_window, 10);
                assert!(
                    saved.video.toggle_sneak
                        && saved.video.toggle_sprint
                        && saved.video.toggle_attack
                        && saved.video.toggle_use
                );
                click(&mut self.menu, event_loop, ButtonId::VideoSettings, false);
                assert_eq!(self.menu.state.screen, Screen::Video);
                self.menu.redraw().unwrap();
                click(&mut self.menu, event_loop, ButtonId::VideoVsync, false);
                click(&mut self.menu, event_loop, ButtonId::VideoFullscreen, false);
                assert!(self.menu.state.video.fullscreen);
                assert!(self.menu.window.as_ref().unwrap().fullscreen().is_some());
                click(&mut self.menu, event_loop, ButtonId::VideoFullscreen, false);
                assert!(!self.menu.state.video.fullscreen);
                assert!(self.menu.window.as_ref().unwrap().fullscreen().is_none());
                let max_scale = gui_scale(self.menu.size.0, self.menu.size.1);
                for expected in (1..=max_scale).chain(std::iter::once(0)) {
                    click(&mut self.menu, event_loop, ButtonId::VideoGuiScale, false);
                    assert_eq!(self.menu.state.video.gui_scale, expected);
                    assert_eq!(
                        self.menu.scale,
                        if expected == 0 { max_scale } else { expected }
                    );
                }
                click(&mut self.menu, event_loop, ButtonId::VideoInactivity, false);
                click(&mut self.menu, event_loop, ButtonId::VideoFps, true);
                assert_eq!(
                    self.menu.state.video,
                    VideoSettings {
                        vsync: false,
                        max_fps: 260,
                        fov: 110,
                        sprint_window: 10,
                        toggle_sneak: true,
                        toggle_sprint: true,
                        toggle_attack: true,
                        toggle_use: true,
                        view_bobbing: true,
                        inactivity_fps_limit: menu_settings::InactivityFpsLimit::Minimized,
                        ..VideoSettings::default()
                    }
                );
                let (_, height) = self.menu.logical_size();
                self.menu
                    .video_ui
                    .ensure_control_visible(height, VideoControl::RenderDistance);
                click(&mut self.menu, event_loop, ButtonId::VideoBlend, true);
                assert_eq!(self.menu.state.video.biome_blend, 2);
                click(&mut self.menu, event_loop, ButtonId::VideoDistance, true);
                assert_eq!(self.menu.state.video.render_distance, 12);
                self.menu
                    .video_ui
                    .ensure_control_visible(height, VideoControl::MenuBlur);
                click(&mut self.menu, event_loop, ButtonId::VideoBlur, true);
                assert_eq!(self.menu.state.video.menu_blur, 10);
                click(&mut self.menu, event_loop, ButtonId::Done, false);
                assert_eq!(self.menu.state.screen, Screen::Options);
                assert_eq!(self.menu.state.video.render_distance, 32);
                assert_eq!(self.menu.state.video.biome_blend, 7);
                assert!(!self.menu.state.video_dirty);
                let path = self.menu.state.settings_path.as_ref().unwrap();
                let loaded: Settings =
                    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
                assert_eq!(loaded.video, self.menu.state.video);
                click(&mut self.menu, event_loop, ButtonId::Accessibility, false);
                assert_eq!(self.menu.state.screen, Screen::Accessibility);
                self.menu.redraw().unwrap();
                click(&mut self.menu, event_loop, ButtonId::ViewBobbing, false);
                assert!(!self.menu.state.video.view_bobbing);
                let (_, height) = self.menu.logical_size();
                self.menu
                    .accessibility_ui
                    .ensure_control_visible(height, AccessibilityControl::FovEffectScale);
                click(&mut self.menu, event_loop, ButtonId::FovEffectScale, false);
                assert_eq!(self.menu.state.video.fov_effect_scale.value(), 0.5);
                self.menu
                    .accessibility_ui
                    .ensure_control_visible(height, AccessibilityControl::MenuBlur);
                click(&mut self.menu, event_loop, ButtonId::MenuBlur, false);
                assert_eq!(self.menu.state.video.menu_blur, 5);
                self.menu
                    .accessibility_ui
                    .ensure_control_visible(height, AccessibilityControl::GlintSpeed);
                click(&mut self.menu, event_loop, ButtonId::GlintSpeed, true);
                click(&mut self.menu, event_loop, ButtonId::GlintStrength, false);
                assert_eq!(self.menu.state.video.glint_speed.value(), 1.0);
                assert_eq!(self.menu.state.video.glint_strength.value(), 0.5);
                self.menu
                    .accessibility_ui
                    .ensure_control_visible(height, AccessibilityControl::PanoramaSpeed);
                click(&mut self.menu, event_loop, ButtonId::PanoramaSpeed, false);
                assert_eq!(self.menu.state.video.panorama_speed.value(), 0.5);
                click(&mut self.menu, event_loop, ButtonId::Done, false);
                assert_eq!(self.menu.state.screen, Screen::Options);
                let path = self.menu.state.settings_path.as_ref().unwrap();
                let saved: Settings =
                    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
                assert!(!saved.video.view_bobbing);
                assert_eq!(saved.video.fov_effect_scale.value(), 0.5);
                assert_eq!(saved.video.glint_speed.value(), 1.0);
                assert_eq!(saved.video.glint_strength.value(), 0.5);
                assert_eq!(saved.video.panorama_speed.value(), 0.5);
                assert_eq!(saved.video.menu_blur, 5);
                click(&mut self.menu, event_loop, ButtonId::SoundSettings, false);
                assert_eq!(self.menu.state.screen, Screen::Sound);
                self.menu.redraw().unwrap();
                click(
                    &mut self.menu,
                    event_loop,
                    ButtonId::SoundSlider(SoundCategory::Master),
                    false,
                );
                assert_eq!(self.menu.state.sound.get(SoundCategory::Master), 0.5);
                let (_, height) = self.menu.logical_size();
                self.menu.sound_ui.ensure_visible(height, SoundCategory::Ui);
                click(
                    &mut self.menu,
                    event_loop,
                    ButtonId::SoundSlider(SoundCategory::Ui),
                    false,
                );
                assert_eq!(self.menu.state.sound.get(SoundCategory::Ui), 0.5);
                click(&mut self.menu, event_loop, ButtonId::Done, false);
                assert_eq!(self.menu.state.screen, Screen::Options);
                let saved: Settings = serde_json::from_slice(
                    &std::fs::read(self.menu.state.settings_path.as_ref().unwrap()).unwrap(),
                )
                .unwrap();
                assert_eq!(saved.sound.ui_gain(), 0.25);
                click(&mut self.menu, event_loop, ButtonId::Controls, false);
                assert_eq!(self.menu.state.screen, Screen::Controls);
                click(&mut self.menu, event_loop, ButtonId::MouseSettings, false);
                assert_eq!(self.menu.state.screen, Screen::Mouse);
                self.menu.redraw().unwrap();
                click(&mut self.menu, event_loop, ButtonId::MouseSensitivity, true);
                click(
                    &mut self.menu,
                    event_loop,
                    ButtonId::MouseWheelSensitivity,
                    true,
                );
                click(
                    &mut self.menu,
                    event_loop,
                    ButtonId::MouseDiscreteScroll,
                    false,
                );
                click(&mut self.menu, event_loop, ButtonId::MouseInvertX, false);
                click(&mut self.menu, event_loop, ButtonId::MouseInvertY, false);
                assert_eq!(self.menu.state.video.mouse_sensitivity.value(), 1.0);
                assert_eq!(self.menu.state.video.mouse_wheel_sensitivity.value(), 10.0);
                assert!(self.menu.state.video.discrete_mouse_scroll);
                assert!(
                    self.menu.state.video.invert_mouse_x && self.menu.state.video.invert_mouse_y
                );
                click(&mut self.menu, event_loop, ButtonId::Done, false);
                assert_eq!(self.menu.state.screen, Screen::Controls);
                let saved: Settings = serde_json::from_slice(
                    &std::fs::read(self.menu.state.settings_path.as_ref().unwrap()).unwrap(),
                )
                .unwrap();
                assert_eq!(saved.video.mouse_sensitivity.value(), 1.0);
                assert_eq!(saved.video.mouse_wheel_sensitivity.value(), 10.0);
                assert!(saved.video.discrete_mouse_scroll);
                assert!(saved.video.invert_mouse_x && saved.video.invert_mouse_y);
                event_loop.exit();
            }
        }

        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&game, &[]).unwrap());
        let temporary_appdata = tempfile::tempdir().unwrap();
        let mut state = State::new(game, Vec::new());
        state.settings_path = Some(
            temporary_appdata
                .path()
                .join("MinecraftNative/options.json"),
        );
        state.asset_load = None;
        state.assets = Some(assets);
        state.screen = Screen::Title;
        let mut harness = Harness {
            menu: Menu::new(&mut state),
            ran: false,
        };
        let mut builder = EventLoop::builder();
        builder.with_any_thread(true);
        builder.build().unwrap().run_app(&mut harness).unwrap();
        assert!(harness.ran);
    }

    #[test]
    fn minecraft_gui_scale_keeps_the_minimum_logical_size() {
        assert_eq!(gui_scale(1280, 720), 3);
        assert_eq!(gui_scale(320, 240), 1);
        assert_eq!(gui_scale(1920, 1080), 4);
        assert_eq!(gui_scale_for(1920, 1080, 1), 1);
        assert_eq!(gui_scale_for(1920, 1080, 3), 3);
        assert_eq!(gui_scale_for(1920, 1080, 20), 4);
        assert_eq!(
            (0..=4)
                .map(|value| next_gui_scale(1920, 1080, value))
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 0]
        );
        assert_eq!(next_gui_scale(320, 240, 200), 0);
    }

    #[test]
    #[ignore = "requires Minecraft Java 26.3 and Java 25 JDK"]
    fn selected_gui_scales_match_java_window_calculation() {
        use std::{ffi::OsString, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| default_minecraft_dir().unwrap());
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("GuiScaleOracle.java");
        std::fs::write(
            &source,
            r#"
import java.lang.reflect.*;
import com.mojang.blaze3d.platform.Window;
public class GuiScaleOracle {
  public static void main(String[] args) throws Exception {
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
    var u=(sun.misc.Unsafe)f.get(null); var window=(Window)u.allocateInstance(Window.class);
    var w=Window.class.getDeclaredField("framebufferWidth"); w.setAccessible(true);
    var h=Window.class.getDeclaredField("framebufferHeight"); h.setAccessible(true);
    for(int width:new int[]{1,319,320,639,640,853,1279,1280,1920,2560,3840})
      for(int height:new int[]{1,239,240,479,480,719,720,1080,1440,2160})
        for(int requested:new int[]{0,1,2,3,4,5,6,8,100}) {
          w.setInt(window,width); h.setInt(window,height);
          System.out.println(window.calculateScale(requested,false));
        }
  }
}
"#,
        )
        .unwrap();
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for lib in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp: OsString = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let run = |cmd: &mut Command| {
            let output = cmd.output().unwrap();
            assert!(
                output.status.success(),
                "Java GUI-scale oracle failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap()
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(source));
        let output = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("GuiScaleOracle"));
        let mut lines = output.lines();
        for width in [1, 319, 320, 639, 640, 853, 1279, 1280, 1920, 2560, 3840] {
            for height in [1, 239, 240, 479, 480, 719, 720, 1080, 1440, 2160] {
                for requested in [0, 1, 2, 3, 4, 5, 6, 8, 100] {
                    let expected: u32 = lines.next().unwrap().parse().unwrap();
                    assert_eq!(
                        gui_scale_for(width, height, requested),
                        expected,
                        "{width}x{height} requested {requested}"
                    );
                }
            }
        }
        assert!(lines.next().is_none());
    }

    #[test]
    fn menu_frame_deadlines_do_not_drift_or_replay_missed_frames() {
        use crate::frame_pacer::advance_frame_deadline;
        let start = Instant::now();
        let interval = Duration::from_millis(16);
        assert_eq!(
            advance_frame_deadline(start, start + Duration::from_millis(3), interval),
            start + interval
        );
        assert_eq!(
            advance_frame_deadline(start, start + Duration::from_millis(50), interval),
            start + Duration::from_millis(64)
        );
        assert_eq!(
            advance_frame_deadline(start, start + Duration::from_millis(63), interval),
            start + Duration::from_millis(79)
        );
        let mut deadline = start;
        let mut frames = 0;
        for millis in 0..1_000 {
            let now = start + Duration::from_millis(millis);
            if now >= deadline {
                deadline = advance_frame_deadline(deadline, now, MENU_FRAME_INTERVAL);
                frames += 1;
            }
        }
        assert_eq!(frames, 60);
    }

    #[test]
    fn panorama_speed_changes_preserve_angle_and_freeze() {
        let start = Instant::now();
        let mut clock = PanoramaClock::new(start, 1.0);
        assert_eq!(clock.sample(start + Duration::from_secs(2), 0.5), 2.0);
        assert_eq!(clock.sample(start + Duration::from_secs(4), 0.0), 3.0);
        assert_eq!(clock.sample(start + Duration::from_secs(20), 1.0), 3.0);
        assert_eq!(clock.sample(start + Duration::from_secs(21), 1.0), 4.0);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 assets and a Windows GPU window"]
    fn inactivity_paces_window_frames_and_input_restores_them() {
        if !crate::window_test::isolated(
            "menu::tests::inactivity_paces_window_frames_and_input_restores_them",
        ) {
            return;
        }
        use winit::platform::windows::EventLoopBuilderExtWindows;

        struct Harness<'a> {
            menu: Menu<'a>,
            phase: usize,
            phase_started: Instant,
            last: Option<Instant>,
            intervals: [Vec<Duration>; 4],
        }
        impl ApplicationHandler for Harness<'_> {
            fn resumed(&mut self, event_loop: &ActiveEventLoop) {
                self.menu.resumed(event_loop);
                self.phase_started = Instant::now();
            }
            fn window_event(
                &mut self,
                event_loop: &ActiveEventLoop,
                id: WindowId,
                event: WindowEvent,
            ) {
                if matches!(event, WindowEvent::RedrawRequested) {
                    let now = Instant::now();
                    if now.duration_since(self.phase_started) > Duration::from_millis(300)
                        && let Some(last) = self.last
                    {
                        self.intervals[self.phase].push(now.duration_since(last));
                    }
                    self.last = Some(now);
                }
                self.menu.window_event(event_loop, id, event);
            }
            fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
                let now = Instant::now();
                if now.duration_since(self.phase_started) >= Duration::from_secs(2) {
                    self.phase += 1;
                    if self.phase == 4 {
                        event_loop.exit();
                        return;
                    }
                    self.phase_started = now;
                    self.last = None;
                    if self.phase == 3 {
                        let id = self.menu.window.as_ref().unwrap().id();
                        self.menu.window_event(
                            event_loop,
                            id,
                            WindowEvent::CursorMoved {
                                device_id: winit::event::DeviceId::dummy(),
                                position: winit::dpi::PhysicalPosition::new(40.0, 40.0),
                            },
                        );
                    } else {
                        let age = if self.phase == 1 { 61 } else { 601 };
                        self.menu.inactivity = crate::frame_pacer::InactivityTracker::new(
                            now - Duration::from_secs(age),
                        );
                        self.menu.next_frame = now;
                    }
                }
                self.menu.about_to_wait(event_loop);
            }
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&game, &[]).unwrap());
        let mut state = State::new(game, Vec::new());
        state.asset_load = None;
        state.assets = Some(assets);
        state.screen = Screen::Title;
        state.video.inactivity_fps_limit = menu_settings::InactivityFpsLimit::Afk;
        state.video.max_fps = 120;
        let mut harness = Harness {
            menu: Menu::new(&mut state),
            phase: 0,
            phase_started: Instant::now(),
            last: None,
            intervals: std::array::from_fn(|_| Vec::new()),
        };
        let mut builder = EventLoop::builder();
        builder.with_any_thread(true);
        builder.build().unwrap().run_app(&mut harness).unwrap();
        let mut medians = Vec::new();
        for (phase, samples) in harness.intervals.iter_mut().enumerate() {
            samples.sort_unstable();
            assert!(
                samples.len() >= [35, 20, 8, 35][phase],
                "phase {phase}: {} samples",
                samples.len()
            );
            medians.push(samples[samples.len() / 2]);
        }
        assert!((Duration::from_millis(12)..Duration::from_millis(28)).contains(&medians[0]));
        assert!((Duration::from_millis(25)..Duration::from_millis(50)).contains(&medians[1]));
        assert!((Duration::from_millis(80)..Duration::from_millis(130)).contains(&medians[2]));
        assert!((Duration::from_millis(12)..Duration::from_millis(28)).contains(&medians[3]));
        println!(
            "CPU redraw medians for active, AFK, long AFK, resumed: {medians:?}; excludes display scanout"
        );
    }

    #[test]
    fn software_menu_discards_frames_from_a_previous_gui_scale() {
        let scene = SoftwareScene {
            screen: Screen::Video,
            size: (1280, 720),
            scale: 3,
            blur: 5,
            assets: None,
        };
        assert!(scene.matches(Screen::Video, (1280, 720), 3, 5, &None));
        assert!(!scene.matches(Screen::Video, (1280, 720), 2, 5, &None));
    }

    #[test]
    fn software_canvas_expansion_handles_partial_last_pixels() {
        for (width, height, scale) in [(10_u32, 8_u32, 3_u32), (15, 11, 4), (7, 7, 1)] {
            let mut canvas = Canvas::new(width / scale, height / scale);
            for (i, pixel) in canvas.pixels.iter_mut().enumerate() {
                *pixel = 0xff00_0000 | ((i as u32 * 1771) & 0x00ff_ffff);
            }
            let mut output = vec![0; (width * height) as usize];
            expand_canvas(&canvas, &mut output, width, scale);
            for y in 0..height {
                for x in 0..width {
                    let sx = (x / scale).min(canvas.width as u32 - 1);
                    let sy = (y / scale).min(canvas.height as u32 - 1);
                    let expected =
                        canvas.pixels[(sy * canvas.width as u32 + sx) as usize] & 0x00ff_ffff;
                    assert_eq!(output[(y * width + x) as usize], expected);
                }
            }
        }
    }

    #[test]
    fn video_fullscreen_button_follows_the_visible_video_controls() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = State::new(tmp.path().to_path_buf(), Vec::new());
        state.asset_load = None;
        state.screen = Screen::Video;
        let mut menu = Menu::new(&mut state);
        menu.size = (1280, 720);
        menu.scale = gui_scale(1280, 720);
        let buttons = menu.buttons();
        let first: Vec<_> = buttons.iter().take(5).map(|button| button.id).collect();
        assert_eq!(
            first,
            [
                ButtonId::VideoFps,
                ButtonId::VideoVsync,
                ButtonId::VideoInactivity,
                ButtonId::VideoGuiScale,
                ButtonId::VideoFullscreen
            ]
        );
        let fullscreen = buttons
            .iter()
            .find(|button| button.id == ButtonId::VideoFullscreen)
            .unwrap();
        assert!(
            fullscreen.active
                && menu.button_contains(fullscreen, (fullscreen.rect.x + 1, fullscreen.rect.y + 1))
        );
    }

    #[test]
    fn invalid_installation_offers_a_location_choice() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = State::new(tmp.path().to_path_buf(), Vec::new());
        let deadline = Instant::now() + Duration::from_secs(10);
        while state.asset_load.is_some() && Instant::now() < deadline {
            state.poll_asset_load();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(state.screen, Screen::Missing);
        assert!(state.error.is_some());
        assert!(
            Menu::new(&mut state)
                .buttons()
                .iter()
                .any(|b| b.id == ButtonId::ChooseMinecraft && b.active)
        );
    }

    #[test]
    fn failed_pack_load_does_not_commit_the_proposed_selection() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = State::new(tmp.path().to_path_buf(), Vec::new());
        let old_pack = tmp.path().join("old.zip");
        let proposed = tmp.path().join("new.zip");
        state.packs = vec![old_pack.clone()];
        state.selected_pack = Some(0);
        let (sender, result) = mpsc::channel();
        state.asset_load = Some(PendingAssetLoad {
            directory: tmp.path().to_path_buf(),
            packs: vec![proposed],
            kind: AssetLoadKind::ChangePacks(None),
            result,
        });
        sender.send(Err(anyhow::anyhow!("invalid pack"))).unwrap();
        assert!(state.poll_asset_load());
        assert_eq!(state.packs, vec![old_pack]);
        assert_eq!(state.selected_pack, Some(0));
        assert!(state.error.unwrap().contains("invalid pack"));
    }

    #[test]
    fn stale_world_list_is_discarded_after_installation_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = State::new(tmp.path().to_path_buf(), Vec::new());
        let (sender, result) = mpsc::channel();
        state.world_load = Some(PendingWorldLoad {
            directory: tmp.path().join("old-installation"),
            result,
        });
        sender
            .send(LoadedWorlds::List(vec![WorldEntry {
                path: tmp.path().join("stale-world"),
                name: "Stale".into(),
                data_version: None,
                last_played: None,
                icon: None,
            }]))
            .unwrap();
        assert!(state.poll_world_load());
        assert!(state.worlds.is_empty());
    }

    #[test]
    fn stale_pack_list_is_discarded_after_installation_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = State::new(tmp.path().to_path_buf(), Vec::new());
        let (sender, result) = mpsc::channel();
        state.pack_load = Some(PendingPackLoad {
            directory: tmp.path().join("old"),
            result,
        });
        sender.send(vec![tmp.path().join("old-pack.zip")]).unwrap();
        assert!(state.poll_pack_load());
        assert!(state.available_packs.is_empty());
    }

    #[test]
    fn background_pack_scan_uses_current_selection_when_results_arrive() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("resourcepacks");
        std::fs::create_dir_all(folder.join("folder")).unwrap();
        std::fs::write(folder.join("folder/pack.mcmeta"), "{}").unwrap();
        std::fs::write(folder.join("a.zip"), "").unwrap();
        std::fs::write(folder.join("ignored.txt"), "").unwrap();
        let mut state = State::new(tmp.path().to_path_buf(), Vec::new());
        state.refresh_packs();
        state.packs.push(folder.join("a.zip"));
        let deadline = Instant::now() + Duration::from_secs(10);
        while state.pack_load.is_some() && Instant::now() < deadline {
            state.poll_pack_load();
            std::thread::yield_now();
        }
        assert!(state.pack_load.is_none());
        assert_eq!(state.available_packs, vec![folder.join("folder")]);
    }

    #[test]
    fn background_world_list_is_sorted_by_last_played() {
        use flate2::{Compression, write::GzEncoder};
        use native_persistence::nbt::{self, NamedTag, Tag};
        use std::io::Write;
        let tmp = tempfile::tempdir().unwrap();
        for (folder, name, timestamp) in [("one", "Older", 10), ("two", "Newer", 20)] {
            let path = tmp.path().join("saves").join(folder);
            std::fs::create_dir_all(&path).unwrap();
            let level = NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![(
                    "Data".into(),
                    Tag::Compound(vec![
                        ("DataVersion".into(), Tag::Int(5023)),
                        ("LevelName".into(), Tag::String(name.into())),
                        ("LastPlayed".into(), Tag::Long(timestamp)),
                    ]),
                )]),
            };
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&nbt::write(&level).unwrap()).unwrap();
            std::fs::write(path.join("level.dat"), encoder.finish().unwrap()).unwrap();
        }
        let mut state = State::new(tmp.path().to_path_buf(), Vec::new());
        state.refresh_worlds();
        let deadline = Instant::now() + Duration::from_secs(10);
        while state.world_load.is_some() && Instant::now() < deadline {
            state.poll_world_load();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(state.world_load.is_none());
        assert_eq!(
            state
                .worlds
                .iter()
                .map(|world| world.name.as_str())
                .collect::<Vec<_>>(),
            ["Newer", "Older"]
        );
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn invalid_saved_pack_loads_default_resources_in_background() {
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| default_minecraft_dir().unwrap());
        let tmp = tempfile::tempdir().unwrap();
        let mut state = State::new(directory, vec![tmp.path().join("missing.zip")]);
        let deadline = Instant::now() + Duration::from_secs(30);
        while state.asset_load.is_some() && Instant::now() < deadline {
            state.poll_asset_load();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(state.asset_load.is_none());
        assert!(state.assets.is_some());
        assert_eq!(state.screen, Screen::Title);
        assert!(state.packs.is_empty());
        assert!(
            state
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("saved resource packs")
        );
    }

    #[test]
    fn world_list_uses_the_saved_name_and_last_played_time() {
        use flate2::{Compression, write::GzEncoder};
        use native_persistence::nbt::{self, NamedTag, Tag};
        use std::io::Write;
        let temp = tempfile::tempdir().unwrap();
        let level = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "Data".into(),
                Tag::Compound(vec![
                    ("DataVersion".into(), Tag::Int(5023)),
                    ("LevelName".into(), Tag::String("한글 월드".into())),
                    ("LastPlayed".into(), Tag::Long(123456)),
                ]),
            )]),
        };
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&nbt::write(&level).unwrap()).unwrap();
        std::fs::write(temp.path().join("level.dat"), encoder.finish().unwrap()).unwrap();
        let world = WorldEntry::read(temp.path().to_path_buf());
        assert_eq!(world.name, "한글 월드");
        assert_eq!(world.data_version, Some(5023));
        assert_eq!(world.last_played, Some(123456));
    }

    #[test]
    fn pack_selection_requires_valid_metadata() {
        let temp = tempfile::tempdir().unwrap();
        assert!(validate_pack(temp.path()).is_err());
        std::fs::write(
            temp.path().join("pack.mcmeta"),
            br#"{"pack":{"description":"Test","pack_format":1}}"#,
        )
        .unwrap();
        assert!(validate_pack(temp.path()).is_ok());
    }

    #[test]
    #[ignore = "performance measurement using the locally installed Minecraft 26.3 assets"]
    fn measure_menu_hover_layout_reuse() {
        use std::hint::black_box;
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&directory, &[]).unwrap());
        let mut state = State::new(directory, Vec::new());
        state.asset_load = None;
        state.assets = Some(assets);
        for screen in [
            Screen::Title,
            Screen::Options,
            Screen::Video,
            Screen::Sound,
            Screen::Packs,
        ] {
            state.screen = screen;
            let mut menu = Menu::new(&mut state);
            menu.size = (1920, 1080);
            menu.scale = gui_scale(1920, 1080);
            let positions: Vec<_> = (0..5000)
                .map(|i| ((i * 17 % 640) as i32, (i * 11 % 360) as i32))
                .collect();
            let start = Instant::now();
            let mut old_changes = 0;
            for pair in positions.windows(2) {
                let old = menu.hover_target(&menu.buttons(), black_box(pair[0]));
                let new = menu.hover_target(&menu.buttons(), black_box(pair[1]));
                old_changes += usize::from(old != new);
            }
            let old_time = start.elapsed();
            let start = Instant::now();
            let mut new_changes = 0;
            for pair in positions.windows(2) {
                let buttons = menu.buttons();
                let old = menu.hover_target(&buttons, black_box(pair[0]));
                let new = menu.hover_target(&buttons, black_box(pair[1]));
                new_changes += usize::from(old != new);
            }
            let new_time = start.elapsed();
            assert_eq!(old_changes, new_changes, "{screen:?}");
            println!(
                "Menu hover hit tests: screen={screen:?} events={} separate-layouts={old_time:?} shared-layout={new_time:?}",
                positions.len() - 1
            );
        }
    }

    #[test]
    #[ignore = "performance measurement using the locally installed Minecraft 26.3 assets"]
    fn measure_menu_frame_cost() {
        use std::{hint::black_box, time::Instant};
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| default_minecraft_dir().unwrap());
        let started = Instant::now();
        let assets = MenuAssets::load(&directory, &[]).unwrap();
        let load = started.elapsed();
        let (width, height) = (1280_u32, 720_u32);
        let scale = gui_scale(width, height);
        let (lw, lh) = (width / scale, height / scale);
        let mut destination = vec![0_u32; (width * height) as usize];
        let mut panorama_times = Vec::new();
        let mut scale_times = Vec::new();
        let mut blur_times = Vec::new();
        for frame in 0..80 {
            let mut canvas = Canvas::new(lw, lh);
            let now = Instant::now();
            canvas.panorama(
                &assets.panorama,
                &assets.panorama_overlay,
                frame as f32 / 60.0,
            );
            panorama_times.push(now.elapsed());
            let now = Instant::now();
            expand_canvas(&canvas, &mut destination, width, scale);
            black_box(&destination);
            scale_times.push(now.elapsed());
            let now = Instant::now();
            canvas.blur();
            blur_times.push(now.elapsed());
        }
        let report = |values: &mut Vec<std::time::Duration>| {
            values.sort_unstable();
            (values[values.len() / 2], values[values.len() * 95 / 100])
        };
        println!(
            "asset load: {load:?}; panorama median/p95: {:?}; upscale median/p95: {:?}; blur median/p95: {:?}",
            report(&mut panorama_times),
            report(&mut scale_times),
            report(&mut blur_times)
        );
    }
}
