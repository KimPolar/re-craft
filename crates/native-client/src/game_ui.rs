use std::sync::Arc;

use anyhow::Result;
use image::RgbaImage;
use native_resources::recipes::CraftingRecipes;
use native_session::inventory::{
    ClickButton, InventoryMenu, InventoryPointerTarget, ItemStack, PlayerInventory, PlayerSlot,
    QuickCraftPreview,
};
use winit::keyboard::KeyCode;

use crate::{
    accessibility_ui::AccessibilityControl,
    death_ui::{DeathAction, DeathScreen},
    first_person::{FirstPersonInput, ItemUseAnimation},
    fov_ui::FovSlider,
    held_items::ItemModelCache,
    menu::{gui_cursor, gui_scale_for, gui_size},
    menu_assets::MenuAssets,
    menu_draw::{Canvas, Rect},
    menu_settings::{SoundCategory, SoundSettings, VideoSettings},
    mouse_ui::{self, ControlToggle, MouseControl, MouseUi, SprintWindowUi},
    recipe_book_ui::{BookAction, RecipeBookUi, inventory_origin},
    renderer::Renderer,
    sound_ui::SoundUi,
    survival_hud::HudDisplay,
    video_ui::{VideoControl, VideoUi},
};

#[cfg(test)]
use crate::menu::gui_scale;

// Reserve enough tick history to age messages created before the world starts.
const CHAT_AGE_OFFSET: u64 = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseAction {
    None,
    Resume,
    QuitToTitle,
    SaveVideo,
    SaveSound,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PausePage {
    Game,
    Options,
    Controls,
    Mouse,
    Sound,
    Video,
    Accessibility,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PauseButton {
    Resume,
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
    Sound,
    SoundSlider(SoundCategory),
    Quit,
    Video,
    Fov,
    Done,
    Framerate,
    Vsync,
    Fullscreen,
    GuiScale,
    Inactivity,
    Accessibility,
    ViewBobbing,
    FovEffectScale,
    GlintSpeed,
    GlintStrength,
    PanoramaSpeed,
    MenuBlur,
    VideoBlur,
    VideoDistance,
    VideoBlend,
    VideoWeather,
}

impl PauseButton {
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

#[derive(Clone, Copy, PartialEq, Eq)]
struct DrawKey {
    width: u32,
    height: u32,
    inventory_revision: u64,
    paused: bool,
    hover: Option<PauseButton>,
    pause_page: PausePage,
    pause_focus: Option<PauseButton>,
    inventory_open: bool,
    inventory_menu: Option<InventoryMenu>,
    recipe_button_hover: bool,
    hovered_slot: Option<PlayerSlot>,
    carried_position: Option<(i32, i32)>,
    avatar_position: Option<(i32, i32)>,
    avatar_tick: Option<u64>,
    book_open: bool,
    book_revision: u64,
    book_hover: usize,
    book_tick: Option<u64>,
    hud: HudDisplay,
    experience: crate::experience_hud::Display,
    hud_tick: Option<u64>,
    chat: Option<crate::chat_hud::DisplayKey>,
    chat_input_tick: Option<u64>,
    action_bar: Option<(u64, u8)>,
}

pub struct GameUi {
    assets: Arc<MenuAssets>,
    skin: Arc<crate::player_skin::PlayerSkin>,
    held_models: ItemModelCache,
    consume_particles: crate::consume_particles::ParticleSystem,
    plant_sprites: Option<
        std::collections::BTreeMap<
            native_session::loaded::PlantParticleKind,
            (Arc<image::RgbaImage>, bool),
        >,
    >,
    plant_sprite_result: std::sync::mpsc::Receiver<
        Result<
            std::collections::BTreeMap<
                native_session::loaded::PlantParticleKind,
                (Arc<image::RgbaImage>, bool),
            >,
            String,
        >,
    >,
    smoke_particles: crate::smoke_particles::SmokeSystem,
    smoke_sprites: Option<Vec<Arc<image::RgbaImage>>>,
    smoke_sprite_result: std::sync::mpsc::Receiver<Result<Vec<Arc<image::RgbaImage>>, String>>,
    armor_models: crate::equipment_visual::ArmorCache,
    paused: bool,
    hud_hidden: bool,
    pause_page: PausePage,
    pause_focus: Option<PauseButton>,
    video: VideoSettings,
    sound: SoundSettings,
    sound_ui: SoundUi,
    mouse_ui: MouseUi,
    sprint_window_ui: SprintWindowUi,
    sound_dirty: bool,
    audio: Option<crate::audio::UiAudio>,
    slider_pressed: bool,
    video_ui: VideoUi,
    accessibility_ui: crate::accessibility_ui::AccessibilityUi,
    fov_ui: FovSlider,
    video_dirty: bool,
    video_error: Option<String>,
    cursor: (i32, i32),
    cursor_raw: Option<(f64, f64)>,
    size: (u32, u32),
    inventory_menu: Option<InventoryMenu>,
    pointer_button: Option<ClickButton>,
    pointer_slot: Option<PlayerSlot>,
    recipe_book: RecipeBookUi,
    last_draw: Option<DrawKey>,
    compass_inventory_revision: Option<u64>,
    compass_visible: bool,
    pre_blur_hud: Option<Canvas>,
    hud: HudDisplay,
    experience: crate::experience_hud::Display,
    death: Option<DeathScreen>,
    system_messages: Arc<[native_session::SystemMessage]>,
    locked_chest_message: Option<Arc<native_session::LockedChestMessage>>,
    snapshot_messages: Arc<[native_session::SystemMessage]>,
    local_messages: Vec<native_session::SystemMessage>,
    chat_input: crate::chat_input::ChatInput,
}

impl GameUi {
    pub fn new(assets: Arc<MenuAssets>) -> Self {
        let skin = Arc::clone(&assets.player_skin);
        let (smoke_sender, smoke_sprite_result) = std::sync::mpsc::sync_channel(1);
        let (plant_sender, plant_sprite_result) = std::sync::mpsc::sync_channel(1);
        let smoke_loader = Arc::clone(&assets.item_models);
        let plant_loader = Arc::clone(&assets.item_models);
        let _ = std::thread::Builder::new()
            .name("plant-particle-loader".into())
            .spawn(move || {
                let result = plant_loader
                    .lock()
                    .map_err(|_| "particle resource loader is unavailable".to_owned())
                    .and_then(|mut loader| {
                        loader
                            .load_plant_particle_sprites()
                            .map_err(|error| format!("{error:#}"))
                    });
                let _ = plant_sender.send(result);
            });
        let _ = std::thread::Builder::new()
            .name("particle-sprite-loader".into())
            .spawn(move || {
                let result = smoke_loader
                    .lock()
                    .map_err(|_| "particle resource loader is unavailable".to_owned())
                    .and_then(|mut loader| {
                        loader
                            .load_large_smoke_sprites()
                            .map_err(|error| format!("{error:#}"))
                    });
                let _ = smoke_sender.send(result);
            });
        let mut held_models = ItemModelCache::new(Arc::clone(&assets.item_models));
        // Falling blocks can appear immediately after a world starts. Request
        // their models on the existing worker before the first entity spawns.
        let _ = held_models.get("minecraft:sand");
        let _ = held_models.get("minecraft:red_sand");
        let _ = held_models.get("minecraft:gravel");
        let armor_models = crate::equipment_visual::ArmorCache::new(
            Arc::clone(&assets.item_models),
            Arc::clone(&assets.trim_registry),
        );
        Self {
            assets,
            skin,
            held_models,
            consume_particles: crate::consume_particles::ParticleSystem::default(),
            plant_sprites: None,
            plant_sprite_result,
            smoke_particles: crate::smoke_particles::SmokeSystem::default(),
            smoke_sprites: None,
            smoke_sprite_result,
            armor_models,
            paused: false,
            hud_hidden: false,
            pause_page: PausePage::Game,
            pause_focus: None,
            video: VideoSettings::default(),
            sound: crate::menu_settings::load().sound,
            sound_ui: SoundUi::default(),
            mouse_ui: MouseUi::default(),
            sprint_window_ui: SprintWindowUi::default(),
            sound_dirty: false,
            audio: None,
            slider_pressed: false,
            video_ui: VideoUi::new(VideoSettings::default()),
            accessibility_ui: crate::accessibility_ui::AccessibilityUi::default(),
            fov_ui: FovSlider::new(VideoSettings::default().fov),
            video_dirty: false,
            video_error: None,
            cursor: (-1, -1),
            cursor_raw: None,
            size: (0, 0),
            inventory_menu: None,
            pointer_button: None,
            pointer_slot: None,
            recipe_book: RecipeBookUi::default(),
            last_draw: None,
            compass_inventory_revision: None,
            compass_visible: false,
            pre_blur_hud: None,
            hud: HudDisplay::default(),
            experience: crate::experience_hud::Display::new(
                Default::default(),
                native_session::GameMode::Survival,
            ),
            death: None,
            system_messages: Arc::from([]),
            locked_chest_message: None,
            snapshot_messages: Arc::from([]),
            local_messages: Vec::new(),
            chat_input: crate::chat_input::ChatInput::default(),
        }
    }

    pub fn set_player_skin(&mut self, skin: Arc<crate::player_skin::PlayerSkin>) {
        self.skin = skin;
        self.last_draw = None;
    }

    pub fn update_consume_particles(
        &mut self,
        snapshot: &native_session::WorldSnapshot,
        spawns: Vec<native_session::consume_particles::ConsumeParticleSpawn>,
    ) {
        self.consume_particles
            .update(snapshot.tick, &snapshot.world, spawns);
    }

    pub fn queue_plant_break_particles(
        &mut self,
        bursts: Vec<native_session::loaded::PlantBreakParticles>,
    ) {
        self.consume_particles.queue_plant_breaks(bursts);
    }

    pub fn update_evaporation_smoke(
        &mut self,
        snapshot: &native_session::WorldSnapshot,
        bursts: Vec<native_session::loaded::EvaporationSmoke>,
    ) {
        self.smoke_particles
            .update(snapshot.tick, &snapshot.world, bursts);
    }

    pub fn set_trim_registry(&mut self, trims: Arc<crate::equipment_visual::TrimRegistry>) {
        self.armor_models.set_trims(trims);
        self.last_draw = None;
    }

    pub fn set_crafting_recipes(&mut self, recipes: Option<Arc<CraftingRecipes>>) {
        self.recipe_book.set_recipes(recipes);
    }

    pub fn recipe_book_click(
        &mut self,
        button: ClickButton,
        use_max: bool,
        inventory: &PlayerInventory,
    ) -> BookAction {
        let Some(menu) = self.inventory_menu else {
            return BookAction::None;
        };
        self.recipe_book.sync(inventory, &self.assets);
        let (width, height) = self.logical_size();
        self.recipe_book
            .click(width, height, self.cursor, menu, button, use_max)
    }

    pub fn recipe_book_captures_text(&self) -> bool {
        !matches!(self.inventory_menu, Some(InventoryMenu::Chest { .. }))
            && self.recipe_book.captures_text()
    }
    pub fn recipe_book_backspace(&mut self) {
        self.recipe_book.backspace();
    }
    pub fn recipe_book_input(&mut self, text: &str) {
        self.recipe_book.input_text(text);
    }
    pub fn recipe_book_unfocus(&mut self) {
        self.recipe_book.unfocus();
    }
    pub fn recipe_book_escape(&mut self) -> Option<BookAction> {
        self.recipe_book.close_narrow(self.logical_size().0)
    }

    pub fn set_video_settings(&mut self, video: VideoSettings) {
        self.video = video;
        self.video_ui = VideoUi::new(video);
        self.fov_ui = FovSlider::new(video.fov);
        self.video_dirty = false;
        self.update_cursor_scale();
        self.last_draw = None;
    }

    pub fn video_settings(&self) -> VideoSettings {
        self.video
    }

    pub fn toggle_fullscreen(&mut self) {
        self.video.fullscreen = !self.video.fullscreen;
        self.video_dirty = true;
        self.last_draw = None;
    }

    fn gui_scale(&self) -> u32 {
        gui_scale_for(self.size.0, self.size.1, self.video.gui_scale)
    }

    fn update_cursor_scale(&mut self) {
        if let Some((x, y)) = self.cursor_raw {
            self.cursor = gui_cursor(self.size, self.gui_scale(), (x, y));
        }
    }

    fn set_size(&mut self, size: (u32, u32)) {
        if self.size != size {
            self.size = size;
            self.update_cursor_scale();
        }
    }

    pub fn resize(&mut self, size: (u32, u32)) {
        self.set_size(size);
    }

    pub fn debug_focus_pause(&mut self, enabled: bool, tick: u64) {
        self.video.pause_on_lost_focus = enabled;
        self.video_dirty = true;
        self.local_messages.push(native_session::SystemMessage {
            kind: native_session::SystemMessageKind::DebugFocusPause(enabled),
            tick: tick.saturating_add(CHAT_AGE_OFFSET),
        });
        if self.local_messages.len() > 100 {
            self.local_messages.remove(0);
        }
        self.merge_chat_messages();
    }

    pub fn screenshot_result(&mut self, result: Result<std::path::PathBuf>, tick: u64) {
        self.screenshot_result_with_age(result, tick, std::time::Duration::ZERO);
    }

    pub fn screenshot_result_with_age(
        &mut self,
        result: Result<std::path::PathBuf>,
        tick: u64,
        age: std::time::Duration,
    ) {
        let kind = match result {
            Ok(path) => native_session::SystemMessageKind::ScreenshotSaved(path.into()),
            Err(error) => {
                native_session::SystemMessageKind::ScreenshotFailed(format!("{error:#}").into())
            }
        };
        self.local_messages.push(native_session::SystemMessage {
            kind,
            tick: tick
                .saturating_add(CHAT_AGE_OFFSET)
                .saturating_sub((age.as_millis() / 50).min(CHAT_AGE_OFFSET as u128) as u64),
        });
        if self.local_messages.len() > 100 {
            self.local_messages.remove(0);
        }
        self.merge_chat_messages();
    }

    pub fn chat_open(&self) -> bool {
        self.chat_input.is_open()
    }
    pub fn chat_composing(&self) -> bool {
        self.chat_input.composing()
    }

    pub fn close_chat(&mut self) {
        self.chat_input.close();
        self.last_draw = None;
    }

    pub fn open_chat(&mut self, command: bool) {
        self.chat_input.open(command);
        self.last_draw = None;
    }

    pub fn chat_text(&mut self, text: &str) {
        self.chat_input.insert(text);
        self.last_draw = None;
    }

    pub fn chat_selected_text(&self) -> &str {
        self.chat_input.selected_text()
    }

    pub fn chat_delete_selection(&mut self) {
        self.chat_input.delete_selection();
        self.last_draw = None;
    }

    pub fn chat_select_all(&mut self) {
        self.chat_input.key(KeyCode::KeyA, true, false);
        self.last_draw = None;
    }

    pub fn chat_preedit(&mut self, text: String) {
        self.chat_input.set_preedit(text);
        self.last_draw = None;
    }

    pub fn chat_scroll(&mut self, delta: i32) {
        self.chat_input.scroll(delta);
        self.last_draw = None;
    }

    pub fn chat_click(&mut self, shift: bool) -> Option<std::path::PathBuf> {
        if !self.chat_input.is_open() {
            return None;
        }
        let (width, height) = self.logical_size();
        if !shift
            && let Some(path) = crate::chat_hud::screenshot_at(
                &self.assets,
                &self.system_messages,
                height,
                self.chat_input.scroll_offset(),
                self.cursor,
            )
        {
            if path
                .parent()
                .and_then(std::path::Path::file_name)
                .is_some_and(|name| name == "screenshots")
                && path.file_name().is_some()
                && !path
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
            {
                self.chat_input.register_click(std::time::Instant::now());
                return Some(path.as_ref().clone());
            }
        }
        if self.cursor.0 >= 4
            && self.cursor.0 < width
            && self.cursor.1 >= height - 12
            && self.cursor.1 < height
        {
            self.chat_input
                .start_drag(&self.assets, width, self.cursor.0, shift);
            self.last_draw = None;
        }
        None
    }

    pub fn chat_release(&mut self) {
        self.chat_input.end_drag();
    }

    pub fn chat_ime_area(&self) -> (i32, i32, u32) {
        let (width, height) = self.logical_size();
        let scale = self.gui_scale();
        (
            self.chat_input.cursor_x(&self.assets, width) * scale as i32,
            (height - 12) * scale as i32,
            scale,
        )
    }

    pub fn chat_key(
        &mut self,
        key: KeyCode,
        control: bool,
        shift: bool,
        tick: u64,
    ) -> crate::chat_input::ChatAction {
        if matches!(key, KeyCode::PageUp | KeyCode::PageDown) {
            let page =
                crate::chat_hud::focused_page_lines(&self.assets.chat, self.logical_size().1);
            self.chat_input
                .scroll(if key == KeyCode::PageUp { page } else { -page });
            self.last_draw = None;
            return crate::chat_input::ChatAction::None;
        }
        let action = self.chat_input.key(key, control, shift);
        if let crate::chat_input::ChatAction::Submit(text) = &action
            && !text.is_empty()
        {
            let kind = if text.starts_with('/') {
                native_session::SystemMessageKind::CommandUnavailable
            } else {
                native_session::SystemMessageKind::LocalChat(text.clone().into())
            };
            self.local_messages.push(native_session::SystemMessage {
                kind,
                tick: tick.saturating_add(CHAT_AGE_OFFSET),
            });
            if self.local_messages.len() > 100 {
                self.local_messages.remove(0);
            }
            self.merge_chat_messages();
        }
        self.last_draw = None;
        action
    }

    #[cfg(test)]
    pub fn last_system_message(&self) -> Option<&native_session::SystemMessageKind> {
        self.system_messages.last().map(|message| &message.kind)
    }

    fn merge_chat_messages(&mut self) {
        let mut messages = self.snapshot_messages.to_vec();
        for message in &mut messages {
            message.tick = message.tick.saturating_add(CHAT_AGE_OFFSET);
        }
        messages.extend_from_slice(&self.local_messages);
        messages.sort_by_key(|message| message.tick);
        if messages.len() > 100 {
            messages.drain(..messages.len() - 100);
        }
        self.system_messages = messages.into();
        self.last_draw = None;
    }

    pub fn advance_video_settings(&mut self, now: std::time::Instant) {
        if self.pause_page == PausePage::Video
            && self
                .video_ui
                .apply_pending_distance(&mut self.video, now, false)
        {
            self.video_dirty = true;
            self.last_draw = None;
        }
    }

    pub fn sound_settings(&self) -> SoundSettings {
        self.sound
    }

    pub fn set_audio(&mut self, audio: Option<crate::audio::UiAudio>) {
        self.audio = audio;
        self.update_audio_gain();
    }

    fn update_audio_gain(&self) {
        if let Some(audio) = &self.audio {
            audio.set_gain(self.sound.ui_gain());
            audio.set_block_gain(self.sound.block_gain());
            audio.set_player_gain(self.sound.player_gain());
            audio.set_neutral_gain(self.sound.neutral_gain());
            audio.set_ambient_gain(self.sound.ambient_gain());
        }
    }

    fn play_click(&self) {
        if let Some(audio) = &self.audio {
            audio.click();
        }
    }
    pub fn sound_needs_save(&self) -> bool {
        self.sound_dirty
    }
    pub fn sound_save_result(&mut self, result: Result<()>) {
        match result {
            Ok(()) => {
                self.sound_dirty = false;
                self.video_error = None;
            }
            Err(error) => self.video_error = Some(format!("{error:#}")),
        }
        self.last_draw = None;
    }

    pub fn video_needs_save(&self) -> bool {
        self.video_dirty
    }

    pub fn video_save_error(&self) -> Option<&str> {
        self.video_error.as_deref()
    }

    pub fn video_save_result(&mut self, result: Result<()>) {
        match result {
            Ok(()) => {
                self.video_dirty = false;
                self.video_error = None;
            }
            Err(error) => self.video_error = Some(format!("{error:#}")),
        }
        self.last_draw = None;
    }

    pub fn paused(&self) -> bool {
        self.paused || self.death.as_ref().is_some_and(DeathScreen::is_confirming)
    }

    pub fn hud_hidden(&self) -> bool {
        self.hud_hidden
    }

    pub fn minecraft_dir(&self) -> &std::path::Path {
        &self.assets.minecraft_dir
    }

    pub fn toggle_hud(&mut self) {
        self.hud_hidden = !self.hud_hidden;
        self.last_draw = None;
    }

    pub fn set_hud_hidden(&mut self, hidden: bool) {
        if self.hud_hidden != hidden {
            self.toggle_hud();
        }
    }

    /// Switch screens when the authoritative session reports a death. The
    /// world continues ticking while this screen is open.
    pub fn sync_death(&mut self, snapshot: &native_session::WorldSnapshot) -> bool {
        let was_dead = self.death.is_some();
        match snapshot.death {
            Some(info) => {
                if self.death.as_ref().is_none_or(|screen| {
                    let previous = screen.info();
                    previous.tick != info.tick || previous.cause != info.cause
                }) {
                    self.death = Some(DeathScreen::new(info, snapshot.vitals.hardcore));
                    self.resume();
                    self.inventory_pointer_cancel();
                    self.last_draw = None;
                }
                self.death.as_mut().unwrap().update(info);
            }
            None => {
                if self.death.take().is_some() {
                    self.last_draw = None;
                }
            }
        }
        was_dead != self.death.is_some()
    }

    pub fn death_click(&mut self) -> DeathAction {
        let size = self.logical_size();
        self.death
            .as_mut()
            .map_or(DeathAction::None, |screen| screen.click(size, self.cursor))
    }

    pub fn death_key(&mut self, key: KeyCode, shift: bool) -> DeathAction {
        self.death
            .as_mut()
            .map_or(DeathAction::None, |screen| screen.key(key, shift))
    }

    pub fn toggle_pause(&mut self) -> bool {
        self.paused = !self.paused;
        self.pause_page = PausePage::Game;
        self.pause_focus = None;
        self.slider_pressed = false;
        self.last_draw = None;
        self.paused
    }

    pub fn resume(&mut self) {
        self.paused = false;
        self.pause_page = PausePage::Game;
        self.pause_focus = None;
        self.slider_pressed = false;
        self.last_draw = None;
    }

    fn logical_size(&self) -> (i32, i32) {
        let (width, height) = gui_size(self.size, self.gui_scale());
        (width as i32, height as i32)
    }

    pub fn cursor_moved(&mut self, x: f64, y: f64) {
        self.cursor_raw = Some((x, y));
        self.cursor = gui_cursor(self.size, self.gui_scale(), (x, y));
        if self.chat_input.is_open() {
            let width = self.logical_size().0;
            if self.chat_input.drag(&self.assets, width, self.cursor.0) {
                self.last_draw = None;
            }
        }
        if self.paused && self.pause_page == PausePage::Sound {
            let (w, h) = self.logical_size();
            let (dragged, changed) = self
                .sound_ui
                .pointer_move(w, h, self.cursor, &mut self.sound);
            self.sound_dirty |= changed;
            if changed {
                self.update_audio_gain();
            }
            if dragged {
                self.last_draw = None;
            }
        } else if self.paused && self.pause_page == PausePage::Video {
            let (w, h) = self.logical_size();
            let (dragged, changed) = self
                .video_ui
                .pointer_move(w, h, self.cursor, &mut self.video);
            self.video_dirty |= changed;
            if dragged {
                self.last_draw = None;
            }
        } else if self.paused && self.pause_page == PausePage::Accessibility {
            let (w, h) = self.logical_size();
            let (dragged, changed) = self.accessibility_ui.pointer_move_with_settings(
                w,
                h,
                self.cursor,
                &mut self.video,
            );
            self.video_dirty |= changed;
            if dragged {
                self.last_draw = None;
            }
        } else if self.paused && self.pause_page == PausePage::Controls {
            let (w, _) = self.logical_size();
            let (dragged, changed) =
                self.sprint_window_ui
                    .pointer_move(w, self.cursor.0, &mut self.video);
            self.video_dirty |= changed;
            if dragged {
                self.last_draw = None;
            }
        } else if self.paused && self.pause_page == PausePage::Mouse {
            let (w, _) = self.logical_size();
            let (dragged, changed) = self
                .mouse_ui
                .pointer_move(w, self.cursor.0, &mut self.video);
            self.video_dirty |= changed;
            if dragged {
                self.last_draw = None;
            }
        } else if self.paused && self.pause_page == PausePage::Options {
            let (w, _) = self.logical_size();
            let (dragged, changed) = self.fov_ui.pointer_move(w, self.cursor.0, &mut self.video);
            self.video_dirty |= changed;
            if dragged {
                self.last_draw = None;
            }
        }
    }

    fn hover(&self) -> Option<PauseButton> {
        if !self.paused || self.size.0 == 0 || self.size.1 == 0 {
            return None;
        }
        let (w, h) = self.logical_size();
        let (x, y) = (w / 2 - 100, h / 4 + 8);
        let hit = |rect: Rect| rect.contains(self.cursor.0, self.cursor.1);
        match self.pause_page {
            PausePage::Game => {
                if hit(Rect {
                    x,
                    y,
                    w: 200,
                    h: 20,
                }) {
                    Some(PauseButton::Resume)
                } else if hit(Rect {
                    x,
                    y: y + 72,
                    w: 98,
                    h: 20,
                }) {
                    Some(PauseButton::Options)
                } else if hit(Rect {
                    x,
                    y: y + 104,
                    w: 200,
                    h: 20,
                }) {
                    Some(PauseButton::Quit)
                } else {
                    None
                }
            }
            PausePage::Options => {
                if hit(FovSlider::rect(w)) {
                    Some(PauseButton::Fov)
                } else if hit(Rect {
                    x: w / 2 + 5,
                    y: 59,
                    w: 150,
                    h: 20,
                }) {
                    Some(PauseButton::Sound)
                } else if hit(Rect {
                    x: w / 2 - 155,
                    y: 83,
                    w: 150,
                    h: 20,
                }) {
                    Some(PauseButton::Video)
                } else if hit(Rect {
                    x: w / 2 + 5,
                    y: 83,
                    w: 150,
                    h: 20,
                }) {
                    Some(PauseButton::Controls)
                } else if hit(Rect {
                    x: w / 2 + 5,
                    y: 131,
                    w: 150,
                    h: 20,
                }) {
                    Some(PauseButton::Accessibility)
                } else if hit(Rect {
                    x,
                    y: h - 28,
                    w: 200,
                    h: 20,
                }) {
                    Some(PauseButton::Done)
                } else {
                    None
                }
            }
            PausePage::Controls => {
                if hit(mouse_ui::controls_mouse_button(w)) {
                    Some(PauseButton::MouseSettings)
                } else if hit(mouse_ui::control_toggle_rect(w, ControlToggle::Sneak)) {
                    Some(PauseButton::ToggleSneak)
                } else if hit(mouse_ui::control_toggle_rect(w, ControlToggle::Sprint)) {
                    Some(PauseButton::ToggleSprint)
                } else if hit(mouse_ui::control_toggle_rect(w, ControlToggle::Attack)) {
                    Some(PauseButton::ToggleAttack)
                } else if hit(mouse_ui::control_toggle_rect(w, ControlToggle::Use)) {
                    Some(PauseButton::ToggleUse)
                } else if hit(SprintWindowUi::rect(w)) {
                    Some(PauseButton::SprintWindow)
                } else if hit(Rect {
                    x,
                    y: h - 27,
                    w: 200,
                    h: 20,
                }) {
                    Some(PauseButton::Done)
                } else {
                    None
                }
            }
            PausePage::Mouse => {
                if hit(Rect {
                    x,
                    y: h - 27,
                    w: 200,
                    h: 20,
                }) {
                    Some(PauseButton::Done)
                } else {
                    MouseUi::hit(w, self.cursor).map(|control| match control {
                        MouseControl::Sensitivity => PauseButton::MouseSensitivity,
                        MouseControl::WheelSensitivity => PauseButton::MouseWheelSensitivity,
                        MouseControl::DiscreteScrolling => PauseButton::MouseDiscreteScroll,
                        MouseControl::InvertX => PauseButton::MouseInvertX,
                        MouseControl::InvertY => PauseButton::MouseInvertY,
                    })
                }
            }
            PausePage::Sound => {
                if hit(Rect {
                    x,
                    y: h - 27,
                    w: 200,
                    h: 20,
                }) {
                    Some(PauseButton::Done)
                } else {
                    self.sound_ui
                        .hit(w, h, self.cursor)
                        .map(PauseButton::SoundSlider)
                }
            }
            PausePage::Video => {
                if hit(Rect {
                    x,
                    y: h - 27,
                    w: 200,
                    h: 20,
                }) {
                    Some(PauseButton::Done)
                } else {
                    self.video_ui
                        .hit(w, h, self.cursor)
                        .map(|control| match control {
                            VideoControl::Framerate => PauseButton::Framerate,
                            VideoControl::Vsync => PauseButton::Vsync,
                            VideoControl::Fullscreen => PauseButton::Fullscreen,
                            VideoControl::GuiScale => PauseButton::GuiScale,
                            VideoControl::Inactivity => PauseButton::Inactivity,
                            VideoControl::MenuBlur => PauseButton::VideoBlur,
                            VideoControl::RenderDistance => PauseButton::VideoDistance,
                            VideoControl::BiomeBlend => PauseButton::VideoBlend,
                            VideoControl::WeatherRadius => PauseButton::VideoWeather,
                        })
                }
            }
            PausePage::Accessibility => {
                if hit(Rect {
                    x: w / 2 + 5,
                    y: h - 27,
                    w: 150,
                    h: 20,
                }) {
                    Some(PauseButton::Done)
                } else {
                    self.accessibility_ui.hit_control(w, h, self.cursor).map(
                        |control| match control {
                            AccessibilityControl::ViewBobbing => PauseButton::ViewBobbing,
                            AccessibilityControl::FovEffectScale => PauseButton::FovEffectScale,
                            AccessibilityControl::GlintSpeed => PauseButton::GlintSpeed,
                            AccessibilityControl::GlintStrength => PauseButton::GlintStrength,
                            AccessibilityControl::PanoramaSpeed => PauseButton::PanoramaSpeed,
                            AccessibilityControl::MenuBlur => PauseButton::MenuBlur,
                        },
                    )
                }
            }
        }
    }

    fn activate(&mut self, button: PauseButton, mouse: bool) -> PauseAction {
        self.pause_focus = Some(button);
        self.last_draw = None;
        match button {
            PauseButton::Resume => PauseAction::Resume,
            PauseButton::Quit => PauseAction::QuitToTitle,
            PauseButton::Options => {
                self.pause_page = PausePage::Options;
                self.pause_focus = None;
                PauseAction::None
            }
            PauseButton::Controls => {
                self.pause_page = PausePage::Controls;
                self.pause_focus = None;
                PauseAction::None
            }
            PauseButton::MouseSettings => {
                self.mouse_ui = MouseUi::default();
                self.pause_page = PausePage::Mouse;
                self.pause_focus = None;
                PauseAction::None
            }
            PauseButton::SprintWindow => {
                if mouse {
                    let (w, _) = self.logical_size();
                    self.video_dirty |=
                        self.sprint_window_ui
                            .start(w, self.cursor.0, &mut self.video);
                } else {
                    self.sprint_window_ui.keyboard_adjust = !self.sprint_window_ui.keyboard_adjust;
                }
                PauseAction::None
            }
            PauseButton::ToggleSneak => {
                self.video.toggle_sneak = !self.video.toggle_sneak;
                self.video_dirty = true;
                PauseAction::None
            }
            PauseButton::ToggleSprint => {
                self.video.toggle_sprint = !self.video.toggle_sprint;
                self.video_dirty = true;
                PauseAction::None
            }
            PauseButton::ToggleAttack => {
                self.video.toggle_attack = !self.video.toggle_attack;
                self.video_dirty = true;
                PauseAction::None
            }
            PauseButton::ToggleUse => {
                self.video.toggle_use = !self.video.toggle_use;
                self.video_dirty = true;
                PauseAction::None
            }
            PauseButton::MouseSensitivity => {
                if mouse {
                    let (w, _) = self.logical_size();
                    self.video_dirty |= self.mouse_ui.start(w, self.cursor.0, &mut self.video);
                } else {
                    self.mouse_ui.keyboard_adjust = !self.mouse_ui.keyboard_adjust;
                }
                PauseAction::None
            }
            PauseButton::MouseWheelSensitivity => {
                if mouse {
                    let (w, _) = self.logical_size();
                    self.video_dirty |= self.mouse_ui.start_control(
                        w,
                        self.cursor.0,
                        MouseControl::WheelSensitivity,
                        &mut self.video,
                    );
                } else {
                    self.mouse_ui.keyboard_adjust = !self.mouse_ui.keyboard_adjust;
                }
                PauseAction::None
            }
            PauseButton::MouseDiscreteScroll => {
                self.video.discrete_mouse_scroll = !self.video.discrete_mouse_scroll;
                self.video_dirty = true;
                PauseAction::None
            }
            PauseButton::MouseInvertX => {
                self.video.invert_mouse_x = !self.video.invert_mouse_x;
                self.video_dirty = true;
                PauseAction::None
            }
            PauseButton::MouseInvertY => {
                self.video.invert_mouse_y = !self.video.invert_mouse_y;
                self.video_dirty = true;
                PauseAction::None
            }
            PauseButton::Sound => {
                self.sound_ui = SoundUi::default();
                self.pause_page = PausePage::Sound;
                self.pause_focus = None;
                self.fov_ui.keyboard_adjust = false;
                PauseAction::None
            }
            PauseButton::SoundSlider(category) => {
                if mouse {
                    let (width, _) = self.logical_size();
                    self.sound_dirty |=
                        self.sound_ui
                            .start(width, self.cursor.0, category, &mut self.sound);
                    self.update_audio_gain();
                } else {
                    self.sound_ui.keyboard_adjust = !self.sound_ui.keyboard_adjust;
                }
                PauseAction::None
            }
            PauseButton::Video => {
                self.video_ui = VideoUi::new(self.video);
                self.pause_page = PausePage::Video;
                self.pause_focus = None;
                self.fov_ui.keyboard_adjust = false;
                PauseAction::None
            }
            PauseButton::Accessibility => {
                self.accessibility_ui = crate::accessibility_ui::AccessibilityUi::default();
                self.pause_page = PausePage::Accessibility;
                self.pause_focus = None;
                self.fov_ui.keyboard_adjust = false;
                PauseAction::None
            }
            PauseButton::ViewBobbing => {
                self.video.view_bobbing = !self.video.view_bobbing;
                self.video_dirty = true;
                PauseAction::None
            }
            PauseButton::FovEffectScale
            | PauseButton::GlintSpeed
            | PauseButton::GlintStrength
            | PauseButton::PanoramaSpeed
            | PauseButton::MenuBlur => {
                if mouse {
                    let (width, _) = self.logical_size();
                    self.video_dirty |= self.accessibility_ui.start_slider(
                        width,
                        self.cursor.0,
                        button.accessibility_control().unwrap(),
                        &mut self.video,
                    );
                } else {
                    self.accessibility_ui.keyboard_adjust = !self.accessibility_ui.keyboard_adjust;
                }
                PauseAction::None
            }
            PauseButton::Done => self.pause_back(),
            PauseButton::Vsync => {
                self.video.vsync = !self.video.vsync;
                self.video_dirty = true;
                PauseAction::None
            }
            PauseButton::Fullscreen => {
                self.toggle_fullscreen();
                PauseAction::None
            }
            PauseButton::GuiScale => {
                self.video.gui_scale =
                    crate::menu::next_gui_scale(self.size.0, self.size.1, self.video.gui_scale);
                self.video_dirty = true;
                self.update_cursor_scale();
                self.last_draw = None;
                PauseAction::None
            }
            PauseButton::Inactivity => {
                self.video.inactivity_fps_limit = self.video.inactivity_fps_limit.next();
                self.video_dirty = true;
                PauseAction::None
            }
            PauseButton::Framerate => {
                if mouse {
                    let (w, _) = self.logical_size();
                    self.video_dirty |=
                        self.video_ui
                            .start_slider(w, self.cursor.0, &mut self.video);
                } else {
                    self.video_ui.keyboard_adjust = !self.video_ui.keyboard_adjust;
                }
                PauseAction::None
            }
            PauseButton::VideoBlur => {
                if mouse {
                    let (width, _) = self.logical_size();
                    self.video_dirty |= self.video_ui.start_control_slider(
                        width,
                        self.cursor.0,
                        VideoControl::MenuBlur,
                        &mut self.video,
                    );
                } else {
                    self.video_ui.keyboard_adjust = !self.video_ui.keyboard_adjust;
                }
                PauseAction::None
            }
            PauseButton::VideoBlend => {
                if mouse {
                    let (width, _) = self.logical_size();
                    self.video_ui.start_control_slider(
                        width,
                        self.cursor.0,
                        VideoControl::BiomeBlend,
                        &mut self.video,
                    );
                } else {
                    self.video_ui.keyboard_adjust = !self.video_ui.keyboard_adjust;
                }
                PauseAction::None
            }
            PauseButton::VideoWeather => {
                if mouse {
                    let (width, _) = self.logical_size();
                    self.video_dirty |= self.video_ui.start_control_slider(
                        width,
                        self.cursor.0,
                        VideoControl::WeatherRadius,
                        &mut self.video,
                    );
                } else {
                    self.video_ui.keyboard_adjust = !self.video_ui.keyboard_adjust;
                }
                PauseAction::None
            }
            PauseButton::VideoDistance => {
                if mouse {
                    let (width, _) = self.logical_size();
                    self.video_ui.start_control_slider(
                        width,
                        self.cursor.0,
                        VideoControl::RenderDistance,
                        &mut self.video,
                    );
                } else {
                    self.video_ui.keyboard_adjust = !self.video_ui.keyboard_adjust;
                }
                PauseAction::None
            }
            PauseButton::Fov => {
                if mouse {
                    let (w, _) = self.logical_size();
                    self.video_dirty |= self.fov_ui.start(w, self.cursor.0, &mut self.video);
                } else {
                    self.fov_ui.keyboard_adjust = !self.fov_ui.keyboard_adjust;
                }
                PauseAction::None
            }
        }
    }

    pub fn click(&mut self) -> PauseAction {
        if self.pause_page == PausePage::Sound {
            let (w, h) = self.logical_size();
            if self.sound_ui.start_scrollbar(w, h, self.cursor) {
                return PauseAction::None;
            }
        }
        if self.pause_page == PausePage::Video {
            let (w, h) = self.logical_size();
            if self.video_ui.start_scrollbar(w, h, self.cursor) {
                return PauseAction::None;
            }
        }
        if self.pause_page == PausePage::Accessibility {
            let (w, h) = self.logical_size();
            if self.accessibility_ui.start_scrollbar(w, h, self.cursor) {
                return PauseAction::None;
            }
        }
        let Some(button) = self.hover() else {
            return PauseAction::None;
        };
        self.slider_pressed = matches!(
            button,
            PauseButton::Fov
                | PauseButton::SprintWindow
                | PauseButton::MouseSensitivity
                | PauseButton::MouseWheelSensitivity
                | PauseButton::Framerate
                | PauseButton::VideoBlur
                | PauseButton::VideoDistance
                | PauseButton::VideoBlend
                | PauseButton::VideoWeather
                | PauseButton::GlintSpeed
                | PauseButton::FovEffectScale
                | PauseButton::GlintStrength
                | PauseButton::PanoramaSpeed
                | PauseButton::MenuBlur
                | PauseButton::SoundSlider(_)
        );
        if !self.slider_pressed {
            self.play_click();
        }
        self.activate(button, true)
    }

    pub fn pause_release(&mut self) {
        if self.slider_pressed {
            self.play_click();
        }
        self.pause_cancel();
    }

    pub fn pause_cancel(&mut self) {
        self.slider_pressed = false;
        if self.paused && self.pause_page == PausePage::Controls {
            self.sprint_window_ui.release();
            self.last_draw = None;
        } else if self.paused && self.pause_page == PausePage::Mouse {
            self.mouse_ui.release();
            self.last_draw = None;
        } else if self.paused && self.pause_page == PausePage::Sound {
            self.sound_ui.release();
            self.last_draw = None;
        } else if self.paused && self.pause_page == PausePage::Video {
            self.video_ui.release(self.video);
            self.last_draw = None;
        } else if self.paused && self.pause_page == PausePage::Accessibility {
            self.accessibility_ui.release();
            self.last_draw = None;
        } else if self.paused && self.pause_page == PausePage::Options {
            self.fov_ui.release(self.video);
            self.last_draw = None;
        }
    }

    pub fn pause_scroll(&mut self, delta: f64) {
        if self.paused && self.pause_page == PausePage::Sound {
            let (_, h) = self.logical_size();
            self.sound_ui.scroll_by(delta, h);
            self.last_draw = None;
        }
        if self.paused && self.pause_page == PausePage::Accessibility {
            let (_, h) = self.logical_size();
            if self.accessibility_ui.scroll_by(delta, h) {
                if self
                    .pause_focus
                    .and_then(PauseButton::accessibility_control)
                    .is_some_and(|control| !self.accessibility_ui.control_visible(h, control))
                {
                    self.pause_focus = None;
                }
                self.last_draw = None;
            }
        }
        if self.paused && self.pause_page == PausePage::Video {
            let (_, h) = self.logical_size();
            if self.video_ui.scroll_by(delta, h) {
                let (w, _) = self.logical_size();
                let focused_control = match self.pause_focus {
                    Some(PauseButton::Framerate) => Some(VideoControl::Framerate),
                    Some(PauseButton::Vsync) => Some(VideoControl::Vsync),
                    Some(PauseButton::Fullscreen) => Some(VideoControl::Fullscreen),
                    Some(PauseButton::GuiScale) => Some(VideoControl::GuiScale),
                    Some(PauseButton::Inactivity) => Some(VideoControl::Inactivity),
                    Some(PauseButton::VideoBlur) => Some(VideoControl::MenuBlur),
                    Some(PauseButton::VideoDistance) => Some(VideoControl::RenderDistance),
                    Some(PauseButton::VideoBlend) => Some(VideoControl::BiomeBlend),
                    Some(PauseButton::VideoWeather) => Some(VideoControl::WeatherRadius),
                    _ => None,
                };
                if focused_control.is_some_and(|control| {
                    !self
                        .video_ui
                        .visible(h, self.video_ui.control_rect(w, control))
                }) {
                    self.pause_focus = None;
                    self.video_ui.keyboard_adjust = false;
                }
                self.last_draw = None;
            }
        }
    }

    fn pause_back(&mut self) -> PauseAction {
        self.pause_focus = None;
        self.last_draw = None;
        match self.pause_page {
            PausePage::Game => PauseAction::Resume,
            PausePage::Controls => {
                self.pause_page = PausePage::Options;
                PauseAction::None
            }
            PausePage::Mouse => {
                self.mouse_ui.release();
                self.mouse_ui.keyboard_adjust = false;
                self.pause_page = PausePage::Controls;
                if self.video_dirty {
                    PauseAction::SaveVideo
                } else {
                    PauseAction::None
                }
            }
            PausePage::Sound => {
                self.sound_ui.release();
                self.pause_page = PausePage::Options;
                if self.sound_dirty {
                    PauseAction::SaveSound
                } else {
                    PauseAction::None
                }
            }
            PausePage::Options => {
                self.fov_ui.release(self.video);
                self.fov_ui.keyboard_adjust = false;
                self.pause_page = PausePage::Game;
                if self.video_dirty {
                    PauseAction::SaveVideo
                } else {
                    PauseAction::None
                }
            }
            PausePage::Video => {
                self.video_dirty |= self.video_ui.apply_pending_distance(
                    &mut self.video,
                    std::time::Instant::now(),
                    true,
                );
                self.video_ui.release(self.video);
                self.video_ui.keyboard_adjust = false;
                self.pause_page = PausePage::Options;
                if self.video_dirty {
                    PauseAction::SaveVideo
                } else {
                    PauseAction::None
                }
            }
            PausePage::Accessibility => {
                self.accessibility_ui.release();
                self.pause_page = PausePage::Options;
                if self.video_dirty {
                    PauseAction::SaveVideo
                } else {
                    PauseAction::None
                }
            }
        }
    }

    pub fn pause_key(&mut self, key: KeyCode, shift: bool) -> PauseAction {
        if key == KeyCode::Escape {
            return self.pause_back();
        }
        if key == KeyCode::Tab {
            let buttons: Vec<PauseButton> = match self.pause_page {
                PausePage::Game => {
                    vec![PauseButton::Resume, PauseButton::Options, PauseButton::Quit]
                }
                PausePage::Options => vec![
                    PauseButton::Fov,
                    PauseButton::Sound,
                    PauseButton::Video,
                    PauseButton::Controls,
                    PauseButton::Accessibility,
                    PauseButton::Done,
                ],
                PausePage::Controls => vec![
                    PauseButton::MouseSettings,
                    PauseButton::ToggleSneak,
                    PauseButton::ToggleSprint,
                    PauseButton::ToggleAttack,
                    PauseButton::ToggleUse,
                    PauseButton::SprintWindow,
                    PauseButton::Done,
                ],
                PausePage::Mouse => vec![
                    PauseButton::MouseSensitivity,
                    PauseButton::MouseWheelSensitivity,
                    PauseButton::MouseDiscreteScroll,
                    PauseButton::MouseInvertX,
                    PauseButton::MouseInvertY,
                    PauseButton::Done,
                ],
                PausePage::Sound => {
                    let mut buttons: Vec<_> = SoundCategory::ALL
                        .into_iter()
                        .map(PauseButton::SoundSlider)
                        .collect();
                    buttons.push(PauseButton::Done);
                    buttons
                }
                PausePage::Video => {
                    let (w, h) = self.logical_size();
                    let mut buttons = Vec::new();
                    if self
                        .video_ui
                        .visible(h, self.video_ui.control_rect(w, VideoControl::Framerate))
                    {
                        buttons.extend([PauseButton::Framerate, PauseButton::Vsync]);
                    }
                    if self
                        .video_ui
                        .visible(h, self.video_ui.control_rect(w, VideoControl::Inactivity))
                    {
                        buttons.push(PauseButton::Inactivity);
                        buttons.push(PauseButton::GuiScale);
                    }
                    if self
                        .video_ui
                        .visible(h, self.video_ui.control_rect(w, VideoControl::Fullscreen))
                    {
                        buttons.push(PauseButton::Fullscreen);
                    }
                    buttons.push(PauseButton::VideoBlend);
                    buttons.push(PauseButton::VideoDistance);
                    buttons.push(PauseButton::VideoBlur);
                    buttons.push(PauseButton::VideoWeather);
                    buttons.push(PauseButton::Done);
                    buttons
                }
                PausePage::Accessibility => vec![
                    PauseButton::MenuBlur,
                    PauseButton::ViewBobbing,
                    PauseButton::FovEffectScale,
                    PauseButton::GlintSpeed,
                    PauseButton::GlintStrength,
                    PauseButton::PanoramaSpeed,
                    PauseButton::Done,
                ],
            };
            let next = self
                .pause_focus
                .and_then(|button| buttons.iter().position(|b| *b == button));
            let index = match (next, shift) {
                (Some(index), false) => (index + 1) % buttons.len(),
                (Some(index), true) => (index + buttons.len() - 1) % buttons.len(),
                (None, false) => 0,
                (None, true) => buttons.len() - 1,
            };
            self.pause_focus = Some(buttons[index]);
            if self.pause_page == PausePage::Video
                && matches!(
                    self.pause_focus,
                    Some(
                        PauseButton::VideoBlur
                            | PauseButton::VideoDistance
                            | PauseButton::VideoBlend
                            | PauseButton::VideoWeather
                    )
                )
            {
                let (_, h) = self.logical_size();
                self.video_ui.ensure_control_visible(
                    h,
                    if self.pause_focus == Some(PauseButton::VideoBlur) {
                        VideoControl::MenuBlur
                    } else if self.pause_focus == Some(PauseButton::VideoBlend) {
                        VideoControl::BiomeBlend
                    } else if self.pause_focus == Some(PauseButton::VideoWeather) {
                        VideoControl::WeatherRadius
                    } else {
                        VideoControl::RenderDistance
                    },
                );
            }
            if self.pause_page == PausePage::Accessibility
                && let Some(control) = self
                    .pause_focus
                    .and_then(PauseButton::accessibility_control)
            {
                let (_, h) = self.logical_size();
                self.accessibility_ui.ensure_control_visible(h, control);
            }
            if self.pause_page == PausePage::Sound
                && let Some(PauseButton::SoundSlider(category)) = self.pause_focus
            {
                let (_, h) = self.logical_size();
                self.sound_ui.ensure_visible(h, category);
            }
            self.video_ui.keyboard_adjust = false;
            self.fov_ui.keyboard_adjust = self.pause_focus == Some(PauseButton::Fov);
            self.sprint_window_ui.keyboard_adjust =
                self.pause_focus == Some(PauseButton::SprintWindow);
            self.mouse_ui.keyboard_adjust = self.pause_focus.is_some_and(|button| {
                matches!(
                    button.mouse_control(),
                    Some(MouseControl::Sensitivity | MouseControl::WheelSensitivity)
                )
            });
            self.sound_ui.keyboard_adjust =
                matches!(self.pause_focus, Some(PauseButton::SoundSlider(_)));
            self.accessibility_ui.keyboard_adjust = matches!(
                self.pause_focus,
                Some(
                    PauseButton::GlintSpeed
                        | PauseButton::FovEffectScale
                        | PauseButton::GlintStrength
                        | PauseButton::PanoramaSpeed
                        | PauseButton::MenuBlur
                )
            );
            self.last_draw = None;
        } else if self.pause_page == PausePage::Controls
            && self.pause_focus == Some(PauseButton::SprintWindow)
            && self.sprint_window_ui.keyboard_adjust
            && matches!(key, KeyCode::ArrowLeft | KeyCode::ArrowRight)
        {
            self.video_dirty |= self
                .sprint_window_ui
                .arrow(key == KeyCode::ArrowRight, &mut self.video);
            self.last_draw = None;
        } else if self.pause_page == PausePage::Mouse
            && self.mouse_ui.keyboard_adjust
            && self
                .pause_focus
                .and_then(PauseButton::mouse_control)
                .is_some_and(|control| {
                    matches!(
                        control,
                        MouseControl::Sensitivity | MouseControl::WheelSensitivity
                    )
                })
            && matches!(key, KeyCode::ArrowLeft | KeyCode::ArrowRight)
        {
            self.video_dirty |= self.mouse_ui.arrow(
                self.pause_focus
                    .and_then(PauseButton::mouse_control)
                    .unwrap(),
                key == KeyCode::ArrowRight,
                &mut self.video,
            );
            self.last_draw = None;
        } else if self.pause_page == PausePage::Sound
            && self.sound_ui.keyboard_adjust
            && matches!(key, KeyCode::ArrowLeft | KeyCode::ArrowRight)
            && let Some(PauseButton::SoundSlider(category)) = self.pause_focus
        {
            self.sound_dirty |=
                self.sound_ui
                    .arrow(category, key == KeyCode::ArrowRight, &mut self.sound);
            self.update_audio_gain();
            self.last_draw = None;
        } else if self.pause_page == PausePage::Options
            && self.pause_focus == Some(PauseButton::Fov)
            && self.fov_ui.keyboard_adjust
            && matches!(key, KeyCode::ArrowLeft | KeyCode::ArrowRight)
        {
            self.video_dirty |= self
                .fov_ui
                .arrow(key == KeyCode::ArrowRight, &mut self.video);
            self.last_draw = None;
        } else if self.pause_page == PausePage::Video
            && self.pause_focus == Some(PauseButton::VideoWeather)
            && self.video_ui.keyboard_adjust
            && matches!(key, KeyCode::ArrowLeft | KeyCode::ArrowRight)
        {
            self.video_dirty |= self
                .video_ui
                .weather_arrow(key == KeyCode::ArrowRight, &mut self.video);
            self.last_draw = None;
        } else if self.pause_page == PausePage::Video
            && self.pause_focus == Some(PauseButton::VideoBlend)
            && self.video_ui.keyboard_adjust
            && matches!(key, KeyCode::ArrowLeft | KeyCode::ArrowRight)
        {
            self.video_ui.blend_arrow(key == KeyCode::ArrowRight);
            self.last_draw = None;
        } else if self.pause_page == PausePage::Video
            && self.pause_focus == Some(PauseButton::VideoDistance)
            && self.video_ui.keyboard_adjust
            && matches!(key, KeyCode::ArrowLeft | KeyCode::ArrowRight)
        {
            self.video_ui.distance_arrow(key == KeyCode::ArrowRight);
            self.last_draw = None;
        } else if self.pause_page == PausePage::Video
            && self.pause_focus == Some(PauseButton::VideoBlur)
            && self.video_ui.keyboard_adjust
            && matches!(key, KeyCode::ArrowLeft | KeyCode::ArrowRight)
        {
            self.video_dirty |= self
                .video_ui
                .blur_arrow(key == KeyCode::ArrowRight, &mut self.video);
            self.last_draw = None;
        } else if self.pause_page == PausePage::Video
            && self.pause_focus == Some(PauseButton::Framerate)
            && self.video_ui.keyboard_adjust
            && matches!(key, KeyCode::ArrowLeft | KeyCode::ArrowRight)
        {
            self.video_dirty |= self
                .video_ui
                .arrow(key == KeyCode::ArrowRight, &mut self.video);
            self.last_draw = None;
        } else if self.pause_page == PausePage::Accessibility
            && self.accessibility_ui.keyboard_adjust
            && matches!(key, KeyCode::ArrowLeft | KeyCode::ArrowRight)
            && let Some(control) = self
                .pause_focus
                .and_then(PauseButton::accessibility_control)
        {
            self.video_dirty |=
                self.accessibility_ui
                    .arrow(control, key == KeyCode::ArrowRight, &mut self.video);
            self.last_draw = None;
        } else if matches!(key, KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Space)
            && let Some(button) = self.pause_focus
        {
            if !matches!(
                button,
                PauseButton::Fov
                    | PauseButton::SprintWindow
                    | PauseButton::MouseSensitivity
                    | PauseButton::MouseWheelSensitivity
                    | PauseButton::Framerate
                    | PauseButton::VideoBlur
                    | PauseButton::GlintSpeed
                    | PauseButton::FovEffectScale
                    | PauseButton::GlintStrength
                    | PauseButton::PanoramaSpeed
                    | PauseButton::MenuBlur
                    | PauseButton::SoundSlider(_)
            ) {
                self.play_click();
            }
            return self.activate(button, false);
        }
        PauseAction::None
    }

    pub fn inventory_slot(&self) -> Option<PlayerSlot> {
        inventory_slot_at_scale(
            self.size,
            self.cursor,
            self.inventory_menu?,
            self.recipe_book.is_open(),
            self.gui_scale(),
        )
    }

    pub fn inventory_outside(&self) -> bool {
        let scale = self.gui_scale();
        let (w, h) = self.logical_size();
        if let Some(InventoryMenu::Chest { other, .. }) = self.inventory_menu {
            let height = if other.is_some() { 222 } else { 168 };
            let (x, y) = ((w - 176) / 2, (h - height) / 2);
            return !Rect {
                x,
                y,
                w: 176,
                h: height,
            }
            .contains(self.cursor.0, self.cursor.1);
        }
        inventory_outside_at_scale(self.size, self.cursor, self.recipe_book.is_open(), scale)
            && !self.recipe_book.contains(w, h, self.cursor)
    }

    fn inventory_pointer_target(&self) -> InventoryPointerTarget {
        if let Some(slot) = self.inventory_slot() {
            InventoryPointerTarget::Slot(slot)
        } else if self.inventory_outside() {
            InventoryPointerTarget::Outside
        } else {
            InventoryPointerTarget::Background
        }
    }

    pub fn inventory_pointer_press(
        &mut self,
        button: ClickButton,
    ) -> Option<InventoryPointerTarget> {
        if self.pointer_button.is_some() {
            return None;
        }
        self.pointer_button = Some(button);
        self.pointer_slot = None;
        Some(self.inventory_pointer_target())
    }

    pub fn inventory_pointer_move(&mut self) -> Option<PlayerSlot> {
        self.pointer_button?;
        let slot = self.inventory_slot();
        if slot == self.pointer_slot {
            return None;
        }
        self.pointer_slot = slot;
        slot
    }

    pub fn inventory_pointer_release(&mut self) -> Option<InventoryPointerTarget> {
        self.pointer_button.take()?;
        self.pointer_slot = None;
        Some(self.inventory_pointer_target())
    }

    pub fn inventory_pointer_cancel(&mut self) -> bool {
        self.pointer_slot = None;
        self.pointer_button.take().is_some()
    }

    pub fn inventory_pointer_active(&self) -> bool {
        self.pointer_button.is_some()
    }

    fn recipe_button_hover(&self) -> bool {
        let Some(menu) = self.inventory_menu else {
            return false;
        };
        if self.size.0 == 0 || self.size.1 == 0 {
            return false;
        }
        let (w, h) = self.logical_size();
        if self.recipe_book.is_open() && w < 379 {
            return false;
        }
        let (x, y) = inventory_origin(w, h, self.recipe_book.is_open());
        let (x, y) = match menu {
            InventoryMenu::Player => (x + 104, y + 61),
            InventoryMenu::CraftingTable { .. } => (x + 5, y + 34),
            InventoryMenu::Chest { .. } => return false,
        };
        Rect { x, y, w: 20, h: 18 }.contains(self.cursor.0, self.cursor.1)
    }

    pub fn breaking_textures(&self) -> &[RgbaImage] {
        &self.assets.destroy_stages
    }

    pub fn item_textures(&self) -> Vec<&RgbaImage> {
        let mut textures = vec![
            &self.assets.grass_top,
            &self.assets.grass_side,
            &self.assets.dirt,
            &self.assets.stone,
            &self.assets.stone_bricks,
            &self.assets.cobblestone,
        ];
        textures.extend(self.assets.wood_textures.iter());
        textures.extend(self.assets.soil_textures.iter());
        textures
    }

    pub fn experience_orb_texture(&self) -> &RgbaImage {
        &self.assets.experience_orb
    }

    pub fn item_glint_texture(&self) -> (&RgbaImage, bool, bool) {
        (
            &self.assets.item_glint,
            self.assets.item_glint_blur,
            self.assets.item_glint_clamp,
        )
    }

    pub fn draw(
        &mut self,
        renderer: &mut Renderer,
        size: (u32, u32),
        snapshot: &native_session::WorldSnapshot,
        render_origin: glam::IVec3,
        partial: f32,
        biome_colors: Option<
            &std::collections::BTreeMap<String, native_resources::biomes::BiomeColors>,
        >,
    ) -> Result<()> {
        self.held_models.begin_frame();
        let dimension = match &snapshot.world {
            native_session::SessionWorld::Demo(_) => "minecraft:overworld",
            native_session::SessionWorld::Loaded(world) => world.dimension.as_str(),
        };
        let context = crate::held_items::CompassContext {
            dimension: dimension.to_owned(),
            world_spawn: snapshot.world_spawn.clone(),
            last_death: snapshot.last_death.clone(),
            owner: snapshot.origin.as_dvec3() + snapshot.player.position.as_dvec3(),
            yaw_degrees: snapshot.player.yaw.to_degrees() - 180.0,
            tick: snapshot.tick.min(i64::MAX as u64) as i64,
        };
        let context_changed = self.held_models.set_compass_context(context);
        if self.compass_inventory_revision != Some(snapshot.inventory.revision()) {
            let has_linked = |item: &ItemStack| {
                matches!(
                    item.id(),
                    "minecraft:compass" | "minecraft:recovery_compass"
                )
            };
            let slots = (0..36)
                .map(PlayerSlot::Storage)
                .chain((0..4).map(PlayerSlot::Armor))
                .chain((0..9).map(PlayerSlot::Crafting))
                .chain((0..54).map(PlayerSlot::Chest))
                .chain(std::iter::once(PlayerSlot::Offhand));
            self.compass_visible = slots
                .into_iter()
                .any(|slot| snapshot.inventory.slot(slot).is_some_and(has_linked))
                || snapshot.inventory.cursor().is_some_and(has_linked);
            self.compass_inventory_revision = Some(snapshot.inventory.revision());
        }
        if self.compass_visible
            && (context_changed || self.held_models.compass_requires_frame_updates())
        {
            self.last_draw = None;
        }
        self.sync_death(snapshot);
        if !Arc::ptr_eq(&self.snapshot_messages, &snapshot.system_messages) {
            self.snapshot_messages = Arc::clone(&snapshot.system_messages);
            self.merge_chat_messages();
        }
        let inventory = &snapshot.inventory;
        let tick = snapshot.tick;
        self.locked_chest_message = snapshot.locked_chest_message.clone();
        self.sync_hud(snapshot);
        renderer.set_hud_hidden(self.hud_hidden);
        renderer.set_screen_blur(
            if self.death.is_none() && (self.paused || inventory.is_open()) {
                self.video.menu_blur
            } else {
                0
            },
        );
        self.set_size(size);
        let scale = self.gui_scale();
        renderer.set_gui_scale(scale);
        if let Some(death) = &mut self.death {
            let logical = gui_size(size, scale);
            if let Some(canvas) = death.draw(logical, self.cursor, &self.assets) {
                renderer.update_overlay(&canvas, true)?;
            }
        } else if let Some(canvas) = self.compose(size, inventory, tick) {
            if let Some(hud) = self.pre_blur_hud.take() {
                renderer.update_pre_blur_overlay(&hud)?;
            }
            renderer.update_overlay(&canvas, self.paused || inventory.is_open())?;
        }
        if self.held_models.poll() {
            self.last_draw = None;
        }
        if self.smoke_sprites.is_none() {
            match self.smoke_sprite_result.try_recv() {
                Ok(Ok(sprites)) => self.smoke_sprites = Some(sprites),
                Ok(Err(error)) => eprintln!("Cannot load large smoke sprites: {error}"),
                Err(_) => {}
            }
        }
        if self.plant_sprites.is_none() {
            match self.plant_sprite_result.try_recv() {
                Ok(Ok(sprites)) => self.plant_sprites = Some(sprites),
                Ok(Err(error)) => eprintln!("Cannot load plant particle sprites: {error}"),
                Err(_) => {}
            }
        }
        let mut particle_draws = Vec::new();
        let mut plant_colors = std::collections::HashMap::new();
        for particle in self.consume_particles.particles() {
            let (texture, color, opaque) = match &particle.visual {
                crate::consume_particles::ParticleVisual::Item(item) => {
                    let Some(model) = self.held_models.get_item(item) else {
                        continue;
                    };
                    let Some(texture) = &model.particle_texture else {
                        continue;
                    };
                    (Arc::clone(texture), [1.0; 3], false)
                }
                crate::consume_particles::ParticleVisual::Plant(kind, position) => {
                    let Some((texture, opaque)) = self
                        .plant_sprites
                        .as_ref()
                        .and_then(|sprites| sprites.get(kind))
                    else {
                        continue;
                    };
                    let color = match (kind, &snapshot.world, biome_colors) {
                        (native_session::loaded::PlantParticleKind::DeadBush, _, _) => [0.6; 3],
                        (_, native_session::SessionWorld::Loaded(world), Some(biomes)) => {
                            *plant_colors.entry(*position).or_insert_with(|| {
                                crate::preview::grass_particle_tint(
                                    world,
                                    biomes,
                                    *position,
                                    self.video.biome_blend,
                                )
                            })
                        }
                        _ => [
                            0.6 * (145.0 / 255.0),
                            0.6 * (189.0 / 255.0),
                            0.6 * (90.0 / 255.0),
                        ],
                    };
                    (Arc::clone(texture), color, *opaque)
                }
            };
            let light = match &snapshot.world {
                native_session::SessionWorld::Demo(_) => u32::MAX,
                native_session::SessionWorld::Loaded(world) => {
                    let cell = particle.position.floor().as_ivec3();
                    u32::from(world.light(cell.x, cell.y, cell.z, false).unwrap_or(0))
                        + 16 * u32::from(world.light(cell.x, cell.y, cell.z, true).unwrap_or(15))
                }
            };
            particle_draws.push(crate::consume_particle_render::RenderParticle {
                previous: particle.previous,
                position: particle.position,
                size: particle.size,
                uv: [
                    particle.uv[0] / 4.0,
                    particle.uv[1] / 4.0,
                    particle.uv[0] / 4.0 + 0.25,
                    particle.uv[1] / 4.0 + 0.25,
                ],
                color,
                opaque,
                texture,
                light,
            });
        }
        if let Some(sprites) = &self.smoke_sprites {
            for particle in self.smoke_particles.particles() {
                let Some(texture) = sprites.get(particle.frame(sprites.len())) else {
                    continue;
                };
                let light = match &snapshot.world {
                    native_session::SessionWorld::Demo(_) => u32::MAX,
                    native_session::SessionWorld::Loaded(world) => {
                        let cell = particle.position.floor().as_ivec3();
                        u32::from(world.light(cell.x, cell.y, cell.z, false).unwrap_or(0))
                            + 16 * u32::from(
                                world.light(cell.x, cell.y, cell.z, true).unwrap_or(15),
                            )
                    }
                };
                particle_draws.push(crate::consume_particle_render::RenderParticle {
                    previous: particle.previous,
                    position: particle.position,
                    size: particle.display_size(partial),
                    uv: [0.0, 0.0, 1.0, 1.0],
                    color: [particle.color; 3],
                    opaque: true,
                    texture: Arc::clone(texture),
                    light,
                });
            }
        }
        renderer.set_consume_particles(particle_draws, render_origin, partial);
        let dropped_models = snapshot
            .dropped_items
            .iter()
            .map(|item| self.held_models.get_dropped_item(item))
            .collect::<Vec<_>>();
        let falling_models = snapshot
            .falling_blocks
            .iter()
            .map(|block| self.held_models.get(block.kind.id()))
            .collect::<Vec<_>>();
        renderer.update_dropped_items(
            &snapshot.dropped_items,
            &dropped_models,
            &snapshot.falling_blocks,
            &falling_models,
            snapshot.origin,
            partial,
        )?;
        // Load the next item while the previous one is being lowered.
        for slot in [
            PlayerSlot::Storage(inventory.selected()),
            PlayerSlot::Offhand,
        ] {
            if let Some(item) = inventory.slot(slot) {
                self.held_models.get_item(item);
            }
        }
        let hands = &snapshot.first_person;
        let main = hands.main.item.as_deref();
        let off = hands.off.item.as_deref();
        let main_model = main.and_then(|item| self.held_models.get_item(item));
        let off_model = off.and_then(|item| self.held_models.get_item(item));
        renderer.update_first_person(FirstPersonInput {
            skin: Arc::clone(&self.skin),
            main_left: self.assets.main_hand_left,
            main_present: main.is_some(),
            main: main_model,
            off: off_model,
            visible: snapshot.game_mode != native_session::GameMode::Spectator
                && snapshot.death.is_none(),
            main_equip: hands.main.equip_progress(partial),
            off_equip: hands.off.equip_progress(partial),
            swing_hand: hands.swing.hand(),
            swing_progress: hands.swing.progress(partial),
            item_use: displayed_item_use(hands, partial),
        });
        renderer.update_gui_glint(
            self.video.glint_speed.value(),
            self.video.glint_strength.value() as f32,
        );
        Ok(())
    }

    fn sync_hud(&mut self, snapshot: &native_session::WorldSnapshot) {
        self.hud =
            HudDisplay::with_air(snapshot.vitals, snapshot.game_mode, snapshot.eyes_in_water)
                .with_armor(snapshot.armor)
                .with_hunger_effect(snapshot.hunger_effect)
                .with_poison_effect(snapshot.poison_effect);
        self.experience =
            crate::experience_hud::Display::new(snapshot.experience, snapshot.game_mode);
    }

    fn compose(
        &mut self,
        size: (u32, u32),
        inventory: &PlayerInventory,
        tick: u64,
    ) -> Option<Canvas> {
        self.set_size(size);
        self.inventory_menu = inventory.menu();
        if self.inventory_menu.is_none() {
            self.inventory_pointer_cancel();
        }
        if size.0 == 0 || size.1 == 0 {
            return None;
        }
        if self.held_models.poll() {
            self.last_draw = None;
        }
        if self.armor_models.poll() {
            self.last_draw = None;
        }
        self.recipe_book.sync(inventory, &self.assets);
        self.recipe_book.set_tick(tick);
        let main = inventory.slot(PlayerSlot::Storage(inventory.selected()));
        let off = inventory.slot(PlayerSlot::Offhand);
        let (right_hand, left_hand) = if self.assets.main_hand_left {
            (off, main)
        } else {
            (main, off)
        };
        let player_menu = inventory.menu() == Some(InventoryMenu::Player);
        let logical_width = gui_size(size, self.gui_scale()).0;
        let avatar_visible = player_menu && !(self.recipe_book.is_open() && logical_width < 379);
        let right_model = avatar_visible
            .then(|| right_hand.and_then(|item| self.held_models.get_item(item)))
            .flatten();
        let left_model = avatar_visible
            .then(|| left_hand.and_then(|item| self.held_models.get_item(item)))
            .flatten();
        let armor = std::array::from_fn(|slot| {
            avatar_visible
                .then(|| {
                    inventory
                        .slot(PlayerSlot::Armor(slot as u8))
                        .and_then(|item| self.armor_models.get(item, slot))
                })
                .flatten()
        });
        let (_, logical_height) = self.logical_size();
        self.video_ui.resize(logical_height);
        self.accessibility_ui.resize(logical_height);
        self.sound_ui.resize(logical_height);
        let hover = self.hover();
        let key = DrawKey {
            width: size.0,
            height: size.1,
            inventory_revision: inventory.revision(),
            paused: self.paused,
            hover,
            pause_page: self.pause_page,
            pause_focus: self.pause_focus,
            inventory_open: inventory.is_open(),
            inventory_menu: inventory.menu(),
            recipe_button_hover: self.recipe_button_hover(),
            hovered_slot: inventory.is_open().then(|| self.inventory_slot()).flatten(),
            carried_position: (inventory.is_open() && inventory.cursor().is_some())
                .then_some(self.cursor),
            avatar_position: avatar_visible.then_some(self.cursor),
            avatar_tick: avatar_visible.then_some(tick),
            book_open: inventory.is_open() && self.recipe_book.is_open(),
            book_revision: self.recipe_book.revision(),
            book_hover: if inventory.is_open() {
                let (width, height) = self.logical_size();
                self.recipe_book.hover_state(width, height, self.cursor)
            } else {
                0
            },
            book_tick: (inventory.is_open()
                && (self.recipe_book.is_open() || inventory.ghost_recipe().is_some()))
            .then_some(tick / self.recipe_book.redraw_period()),
            hud: self.hud,
            experience: self.experience,
            hud_tick: self.hud.animation_key(tick),
            chat: crate::chat_hud::display_key(
                &self.assets.chat,
                &self.system_messages,
                tick.saturating_add(CHAT_AGE_OFFSET),
            ),
            chat_input_tick: self
                .chat_input
                .is_open()
                .then(|| self.chat_input.blink_key()),
            action_bar: crate::action_bar::display_key(self.locked_chest_message.as_deref(), tick),
        };
        if self.last_draw == Some(key) {
            return None;
        }
        let scale = self.gui_scale();
        let logical = gui_size(size, scale);
        let mut canvas = Canvas::new(logical.0, logical.1);
        let (w, h) = (canvas.width, canvas.height);
        let assets = &*self.assets;
        let quick_preview = inventory.quick_craft_preview();
        if !self.hud_hidden {
            let hotbar_x = w / 2 - 91;
            canvas.image(
                &assets.hotbar,
                Rect {
                    x: hotbar_x,
                    y: h - 22,
                    w: 182,
                    h: 22,
                },
            );
            let slot = i32::from(inventory.selected());
            canvas.image(
                &assets.hotbar_selection,
                Rect {
                    x: hotbar_x - 1 + slot * 20,
                    y: h - 23,
                    w: 24,
                    h: 23,
                },
            );
            if let Some(item) = inventory.slot(PlayerSlot::Offhand) {
                let (sprite, icon) = offhand_hotbar_layout(w, h, assets.main_hand_left);
                canvas.image(
                    if assets.main_hand_left {
                        &assets.hotbar_offhand_right
                    } else {
                        &assets.hotbar_offhand_left
                    },
                    sprite,
                );
                stack_icon_seeded(
                    &mut canvas,
                    assets,
                    &mut self.held_models,
                    icon.0,
                    icon.1,
                    item,
                    10,
                );
            }
            for slot in 0..9_u8 {
                if let Some(item) = inventory.slot(PlayerSlot::Storage(slot)) {
                    stack_icon_seeded(
                        &mut canvas,
                        assets,
                        &mut self.held_models,
                        hotbar_x + 3 + i32::from(slot) * 20,
                        h - 19,
                        item,
                        i32::from(slot) + 1,
                    );
                }
            }
            crate::survival_hud::draw(&mut canvas, &assets.hud, self.hud, tick);
            crate::experience_hud::draw(&mut canvas, assets, self.experience);
            if !self.chat_input.is_open() {
                crate::chat_hud::draw(
                    &mut canvas,
                    assets,
                    &self.system_messages,
                    tick.saturating_add(CHAT_AGE_OFFSET),
                );
            }
            crate::action_bar::draw(
                &mut canvas,
                Some(assets),
                self.locked_chest_message.as_deref(),
                tick,
            );
        }
        if self.video.menu_blur > 0 && (self.paused || inventory.is_open()) {
            self.pre_blur_hud = Some(canvas);
            canvas = Canvas::new(logical.0, logical.1);
        } else {
            self.pre_blur_hud = None;
        }
        if let Some(menu) = inventory.menu() {
            canvas.fill(Rect { x: 0, y: 0, w, h }, [0, 0, 0, 100]);
            let chest_menu = matches!(menu, InventoryMenu::Chest { .. });
            let double_chest = matches!(menu, InventoryMenu::Chest { other: Some(_), .. });
            let book_open = !chest_menu && self.recipe_book.is_open();
            let (x, y) = if chest_menu {
                (
                    (w - 176) / 2,
                    (h - if double_chest { 222 } else { 168 }) / 2,
                )
            } else {
                inventory_origin(w, h, book_open)
            };
            if !(book_open && w < 379) {
                canvas.image(
                    match menu {
                        InventoryMenu::Player => &assets.inventory,
                        InventoryMenu::CraftingTable { .. } => &assets.crafting_table,
                        InventoryMenu::Chest { other: None, .. } => &assets.chest,
                        InventoryMenu::Chest { other: Some(_), .. } => &assets.double_chest,
                    },
                    Rect {
                        x,
                        y,
                        w: 176,
                        h: if double_chest {
                            221
                        } else if chest_menu {
                            167
                        } else {
                            166
                        },
                    },
                );
                if player_menu {
                    crate::player_preview::draw(
                        &mut canvas,
                        &self.skin,
                        x,
                        y,
                        self.cursor,
                        tick,
                        crate::player_preview::Hands {
                            right_present: right_hand.is_some(),
                            left_present: left_hand.is_some(),
                            right: right_model.as_deref(),
                            left: left_model.as_deref(),
                        },
                        armor.each_ref().map(|value| value.as_deref()),
                    );
                    canvas.text_without_shadow(
                        Some(assets),
                        x + 97,
                        y + 8,
                        assets.translate("container.crafting", "Crafting"),
                        [64, 64, 64],
                    );
                } else if chest_menu {
                    let title = inventory.chest_title_component().and_then(|component| {
                        crate::text_component::resolve(component, |key, fallback| {
                            assets.translate(key, fallback).to_owned()
                        })
                    });
                    if let Some(title) = title {
                        canvas.component_without_shadow(
                            Some(assets),
                            x + 8,
                            y + 6,
                            &title,
                            [64, 64, 64],
                        );
                    } else {
                        canvas.text_without_shadow(
                            Some(assets),
                            x + 8,
                            y + 6,
                            if double_chest {
                                assets.translate("container.chestDouble", "Large Chest")
                            } else {
                                assets.translate("container.chest", "Chest")
                            },
                            [64, 64, 64],
                        );
                    }
                    canvas.text_without_shadow(
                        Some(assets),
                        x + 8,
                        y + if double_chest { 128 } else { 74 },
                        assets.translate("container.inventory", "Inventory"),
                        [64, 64, 64],
                    );
                } else {
                    canvas.text_without_shadow(
                        Some(assets),
                        x + 29,
                        y + 6,
                        assets.translate("container.crafting", "Crafting"),
                        [64, 64, 64],
                    );
                    canvas.text_without_shadow(
                        Some(assets),
                        x + 8,
                        y + 72,
                        assets.translate("container.inventory", "Inventory"),
                        [64, 64, 64],
                    );
                }
                let (button_x, button_y) = if player_menu {
                    (x + 104, y + 61)
                } else {
                    (x + 5, y + 34)
                };
                let button = Rect {
                    x: button_x,
                    y: button_y,
                    w: 20,
                    h: 18,
                };
                if !chest_menu {
                    canvas.image(
                        if button.contains(self.cursor.0, self.cursor.1) {
                            &assets.recipe_book_button_highlighted
                        } else {
                            &assets.recipe_book_button
                        },
                        button,
                    );
                }
                for index in 0..36_u8 {
                    let (sx, sy) =
                        menu_slot_position(PlayerSlot::Storage(index), menu).expect("storage slot");
                    inventory_slot_icon(
                        &mut canvas,
                        assets,
                        &mut self.held_models,
                        (x + sx, y + sy),
                        inventory,
                        PlayerSlot::Storage(index),
                        quick_preview.as_ref(),
                    );
                }
                let crafting_len = if chest_menu {
                    0
                } else if player_menu {
                    4
                } else {
                    9
                };
                for index in 0..crafting_len {
                    let (sx, sy) = menu_slot_position(PlayerSlot::Crafting(index), menu)
                        .expect("crafting slot");
                    inventory_slot_icon(
                        &mut canvas,
                        assets,
                        &mut self.held_models,
                        (x + sx, y + sy),
                        inventory,
                        PlayerSlot::Crafting(index),
                        quick_preview.as_ref(),
                    );
                    if player_menu && let Some(item) = inventory.slot(PlayerSlot::Armor(index)) {
                        stack_icon(
                            &mut canvas,
                            assets,
                            &mut self.held_models,
                            x + 8,
                            y + 8 + i32::from(index) * 18,
                            item,
                        );
                    }
                }
                if chest_menu {
                    for index in 0..if double_chest { 54_u8 } else { 27_u8 } {
                        let slot = PlayerSlot::Chest(index);
                        let (sx, sy) = menu_slot_position(slot, menu).expect("chest slot");
                        inventory_slot_icon(
                            &mut canvas,
                            assets,
                            &mut self.held_models,
                            (x + sx, y + sy),
                            inventory,
                            slot,
                            quick_preview.as_ref(),
                        );
                    }
                }
                if let Some(item) = inventory.slot(PlayerSlot::CraftResult) {
                    let (sx, sy) =
                        menu_slot_position(PlayerSlot::CraftResult, menu).expect("result slot");
                    stack_icon(
                        &mut canvas,
                        assets,
                        &mut self.held_models,
                        x + sx,
                        y + sy,
                        item,
                    );
                }
                if player_menu {
                    inventory_slot_icon(
                        &mut canvas,
                        assets,
                        &mut self.held_models,
                        (x + 77, y + 62),
                        inventory,
                        PlayerSlot::Offhand,
                        quick_preview.as_ref(),
                    );
                }
                if !chest_menu && let Some((recipe, recipes)) = self.recipe_book.ghost(inventory) {
                    let width = if player_menu { 2 } else { 3 };
                    if let Some(layout) = recipe.book_layout(width, width) {
                        for (index, ingredient) in layout.iter().enumerate() {
                            let Some(ingredient) = ingredient else {
                                continue;
                            };
                            let options = ingredient.display_items(&recipes.tags);
                            if options.is_empty() {
                                continue;
                            }
                            let id = options[(tick as usize / 30) % options.len()];
                            if let Some(item) = crate::recipe_book_ui::icon_stack(id, 1) {
                                let (sx, sy) =
                                    menu_slot_position(PlayerSlot::Crafting(index as u8), menu)
                                        .expect("ghost input slot");
                                let rect = Rect {
                                    x: x + sx,
                                    y: y + sy,
                                    w: 16,
                                    h: 16,
                                };
                                canvas.fill(rect, [255, 0, 0, 48]);
                                stack_icon(
                                    &mut canvas,
                                    assets,
                                    &mut self.held_models,
                                    rect.x,
                                    rect.y,
                                    &item,
                                );
                                canvas.fill(rect, [255, 255, 255, 48]);
                            }
                        }
                        let id =
                            format!("{}:{}", recipe.result.id.namespace, recipe.result.id.path);
                        if let Some(item) =
                            crate::recipe_book_ui::icon_stack(&id, recipe.result.count)
                        {
                            let (sx, sy) = menu_slot_position(PlayerSlot::CraftResult, menu)
                                .expect("ghost result slot");
                            let rect = Rect {
                                x: x + sx,
                                y: y + sy,
                                w: 16,
                                h: 16,
                            };
                            canvas.fill(rect, [255, 0, 0, 48]);
                            stack_icon(
                                &mut canvas,
                                assets,
                                &mut self.held_models,
                                rect.x,
                                rect.y,
                                &item,
                            );
                            canvas.fill(rect, [255, 255, 255, 48]);
                        }
                    }
                }
                if let Some(slot) = self.inventory_slot() {
                    let (sx, sy) = menu_slot_position(slot, menu).expect("visible slot");
                    canvas.fill(
                        Rect {
                            x: x + sx,
                            y: y + sy,
                            w: 16,
                            h: 16,
                        },
                        [255, 255, 255, 100],
                    );
                }
            }
            if !chest_menu {
                self.recipe_book.draw(
                    &mut canvas,
                    assets,
                    &mut self.held_models,
                    self.cursor,
                    tick,
                );
            }
            let cursor_item = quick_preview
                .as_ref()
                .map_or_else(|| inventory.cursor(), |preview| preview.carried.as_ref());
            if let Some(item) = cursor_item {
                stack_icon(
                    &mut canvas,
                    assets,
                    &mut self.held_models,
                    self.cursor.0 - 8,
                    self.cursor.1 - 8,
                    item,
                );
            } else if quick_preview
                .as_ref()
                .is_some_and(|preview| preview.exhausted)
            {
                stack_count(
                    &mut canvas,
                    assets,
                    self.cursor.0 - 8,
                    self.cursor.1 - 8,
                    0,
                    true,
                );
            }
        }
        if self.paused {
            canvas.fill(Rect { x: 0, y: 0, w, h }, [0, 0, 0, 155]);
            if self.pause_page == PausePage::Game {
                let x = w / 2 - 100;
                let y = h / 4 + 8;
                canvas.text_centered(
                    Some(assets),
                    w / 2,
                    y - 20,
                    assets.translate("menu.game", "Game Menu"),
                    [255, 255, 255],
                );
                canvas.button(
                    Some(assets),
                    Rect {
                        x,
                        y,
                        w: 200,
                        h: 20,
                    },
                    true,
                    hover == Some(PauseButton::Resume)
                        || self.pause_focus == Some(PauseButton::Resume),
                    assets.translate("menu.returnToGame", "Back to Game"),
                );
                canvas.button(
                    Some(assets),
                    Rect {
                        x,
                        y: y + 24,
                        w: 98,
                        h: 20,
                    },
                    false,
                    false,
                    assets.translate("gui.advancements", "Advancements"),
                );
                canvas.button(
                    Some(assets),
                    Rect {
                        x: x + 102,
                        y: y + 24,
                        w: 98,
                        h: 20,
                    },
                    false,
                    false,
                    assets.translate("gui.stats", "Statistics"),
                );
                canvas.button(
                    Some(assets),
                    Rect {
                        x,
                        y: y + 48,
                        w: 98,
                        h: 20,
                    },
                    false,
                    false,
                    assets.translate("menu.sendFeedback", "Give Feedback"),
                );
                canvas.button(
                    Some(assets),
                    Rect {
                        x: x + 102,
                        y: y + 48,
                        w: 98,
                        h: 20,
                    },
                    false,
                    false,
                    assets.translate("menu.reportBugs", "Report Bugs"),
                );
                canvas.button(
                    Some(assets),
                    Rect {
                        x,
                        y: y + 72,
                        w: 98,
                        h: 20,
                    },
                    true,
                    hover == Some(PauseButton::Options)
                        || self.pause_focus == Some(PauseButton::Options),
                    assets.translate("menu.options", "Options..."),
                );
                canvas.button(
                    Some(assets),
                    Rect {
                        x: x + 102,
                        y: y + 72,
                        w: 98,
                        h: 20,
                    },
                    false,
                    false,
                    assets.translate("menu.shareToLan", "Open to LAN"),
                );
                canvas.button(
                    Some(assets),
                    Rect {
                        x,
                        y: y + 104,
                        w: 200,
                        h: 20,
                    },
                    true,
                    hover == Some(PauseButton::Quit) || self.pause_focus == Some(PauseButton::Quit),
                    "Quit to Title",
                );
                canvas.text_centered(
                    Some(assets),
                    w / 2,
                    h - 11,
                    "Changes are not saved",
                    [255, 220, 140],
                );
            } else if self.pause_page == PausePage::Options {
                canvas.text_centered(
                    Some(assets),
                    w / 2,
                    15,
                    assets.translate("options.title", "Options"),
                    [255, 255, 255],
                );
                let rows = [
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
                ];
                for (row, (left, right)) in rows.into_iter().enumerate() {
                    let y = 35 + row as i32 * 24;
                    let enabled = row == 2;
                    if row == 0 {
                        self.fov_ui.draw(
                            &mut canvas,
                            Some(assets),
                            self.video.fov,
                            self.cursor,
                            self.pause_focus == Some(PauseButton::Fov),
                        );
                    } else {
                        canvas.button(
                            Some(assets),
                            Rect {
                                x: w / 2 - 155,
                                y,
                                w: 150,
                                h: 20,
                            },
                            enabled,
                            enabled
                                && (hover == Some(PauseButton::Video)
                                    || self.pause_focus == Some(PauseButton::Video)),
                            assets.translate(left.0, left.1),
                        );
                    }
                    canvas.button(
                        Some(assets),
                        Rect {
                            x: w / 2 + 5,
                            y,
                            w: 150,
                            h: 20,
                        },
                        row == 1 || row == 2 || row == 4,
                        (row == 1
                            && (hover == Some(PauseButton::Sound)
                                || self.pause_focus == Some(PauseButton::Sound)))
                            || (row == 2
                                && (hover == Some(PauseButton::Controls)
                                    || self.pause_focus == Some(PauseButton::Controls)))
                            || (row == 4
                                && (hover == Some(PauseButton::Accessibility)
                                    || self.pause_focus == Some(PauseButton::Accessibility))),
                        assets.translate(right.0, right.1),
                    );
                }
                canvas.button(
                    Some(assets),
                    Rect {
                        x: w / 2 - 100,
                        y: h - 28,
                        w: 200,
                        h: 20,
                    },
                    true,
                    hover == Some(PauseButton::Done) || self.pause_focus == Some(PauseButton::Done),
                    assets.translate("gui.done", "Done"),
                );
            } else if self.pause_page == PausePage::Controls {
                canvas.text_centered(
                    Some(assets),
                    w / 2,
                    12,
                    assets.translate("controls.title", "Controls"),
                    [255, 255, 255],
                );
                mouse_ui::draw_controls_placeholders(&mut canvas, assets);
                for (toggle, button) in [
                    (ControlToggle::Sneak, PauseButton::ToggleSneak),
                    (ControlToggle::Sprint, PauseButton::ToggleSprint),
                    (ControlToggle::Attack, PauseButton::ToggleAttack),
                    (ControlToggle::Use, PauseButton::ToggleUse),
                ] {
                    canvas.button(
                        Some(assets),
                        mouse_ui::control_toggle_rect(w, toggle),
                        true,
                        hover == Some(button) || self.pause_focus == Some(button),
                        &mouse_ui::control_toggle_label(assets, toggle, self.video),
                    );
                }
                self.sprint_window_ui.draw(
                    &mut canvas,
                    assets,
                    self.video,
                    self.cursor,
                    self.pause_focus == Some(PauseButton::SprintWindow),
                );
                canvas.button(
                    Some(assets),
                    mouse_ui::controls_mouse_button(w),
                    true,
                    hover == Some(PauseButton::MouseSettings)
                        || self.pause_focus == Some(PauseButton::MouseSettings),
                    assets.translate("options.mouse_settings", "Mouse Settings..."),
                );
                canvas.button(
                    Some(assets),
                    Rect {
                        x: w / 2 - 100,
                        y: h - 27,
                        w: 200,
                        h: 20,
                    },
                    true,
                    hover == Some(PauseButton::Done) || self.pause_focus == Some(PauseButton::Done),
                    assets.translate("gui.done", "Done"),
                );
            } else if self.pause_page == PausePage::Mouse {
                canvas.text_centered(
                    Some(assets),
                    w / 2,
                    12,
                    assets.translate("options.mouse_settings.title", "Mouse Settings"),
                    [255, 255, 255],
                );
                self.mouse_ui.draw(
                    &mut canvas,
                    assets,
                    self.video,
                    self.cursor,
                    self.pause_focus.and_then(PauseButton::mouse_control),
                );
                canvas.button(
                    Some(assets),
                    Rect {
                        x: w / 2 - 100,
                        y: h - 27,
                        w: 200,
                        h: 20,
                    },
                    true,
                    hover == Some(PauseButton::Done) || self.pause_focus == Some(PauseButton::Done),
                    assets.translate("gui.done", "Done"),
                );
            } else if self.pause_page == PausePage::Sound {
                canvas.text_centered(
                    Some(assets),
                    w / 2,
                    12,
                    assets.translate("options.sounds.title", "Music & Sound Options"),
                    [255, 255, 255],
                );
                let focused = match self.pause_focus {
                    Some(PauseButton::SoundSlider(category)) => Some(category),
                    _ => None,
                };
                self.sound_ui
                    .draw(&mut canvas, assets, self.sound, self.cursor, focused);
                canvas.button(
                    Some(assets),
                    Rect {
                        x: w / 2 - 100,
                        y: h - 27,
                        w: 200,
                        h: 20,
                    },
                    true,
                    hover == Some(PauseButton::Done) || self.pause_focus == Some(PauseButton::Done),
                    assets.translate("gui.done", "Done"),
                );
            } else if self.pause_page == PausePage::Accessibility {
                canvas.text_centered(
                    Some(assets),
                    w / 2,
                    12,
                    assets.translate("options.accessibility.title", "Accessibility Settings"),
                    [255, 255, 255],
                );
                self.accessibility_ui.draw(
                    &mut canvas,
                    assets,
                    self.video,
                    self.cursor,
                    self.pause_focus
                        .and_then(PauseButton::accessibility_control),
                );
                canvas.button(
                    Some(assets),
                    Rect {
                        x: w / 2 - 155,
                        y: h - 27,
                        w: 150,
                        h: 20,
                    },
                    false,
                    false,
                    assets.translate("options.accessibility.link", "Accessibility Guide..."),
                );
                canvas.button(
                    Some(assets),
                    Rect {
                        x: w / 2 + 5,
                        y: h - 27,
                        w: 150,
                        h: 20,
                    },
                    true,
                    hover == Some(PauseButton::Done) || self.pause_focus == Some(PauseButton::Done),
                    assets.translate("gui.done", "Done"),
                );
            } else {
                canvas.text_centered(
                    Some(assets),
                    w / 2,
                    12,
                    assets.translate("options.videoTitle", "Video Settings"),
                    [255, 255, 255],
                );
                let focused = match self.pause_focus {
                    Some(PauseButton::Framerate) => Some(VideoControl::Framerate),
                    Some(PauseButton::Vsync) => Some(VideoControl::Vsync),
                    Some(PauseButton::Fullscreen) => Some(VideoControl::Fullscreen),
                    Some(PauseButton::GuiScale) => Some(VideoControl::GuiScale),
                    Some(PauseButton::Inactivity) => Some(VideoControl::Inactivity),
                    Some(PauseButton::VideoBlur) => Some(VideoControl::MenuBlur),
                    Some(PauseButton::VideoDistance) => Some(VideoControl::RenderDistance),
                    Some(PauseButton::VideoBlend) => Some(VideoControl::BiomeBlend),
                    Some(PauseButton::VideoWeather) => Some(VideoControl::WeatherRadius),
                    _ => None,
                };
                self.video_ui
                    .draw(&mut canvas, assets, self.video, self.cursor, focused);
                canvas.button(
                    Some(assets),
                    Rect {
                        x: w / 2 - 100,
                        y: h - 27,
                        w: 200,
                        h: 20,
                    },
                    true,
                    hover == Some(PauseButton::Done) || self.pause_focus == Some(PauseButton::Done),
                    assets.translate("gui.done", "Done"),
                );
            }
            if let Some(error) = &self.video_error {
                canvas.text_centered(Some(assets), w / 2, h - 42, error, [255, 120, 120]);
            }
        }
        if self.chat_input.is_open() {
            let max_scroll = crate::chat_hud::draw_focused(
                &mut canvas,
                assets,
                &self.system_messages,
                tick.saturating_add(CHAT_AGE_OFFSET),
                self.chat_input.scroll_offset(),
            );
            self.chat_input.clamp_scroll(max_scroll);
            self.chat_input.draw(&mut canvas, assets);
        }
        self.last_draw = Some(key);
        Some(canvas)
    }
}

/// Java resolves the use animation and duration from the displayed hand item,
/// which may still be the previous stack while its equip animation lowers it.
fn displayed_item_use(
    hands: &native_session::animation::FirstPersonState,
    partial_tick: f32,
) -> Option<ItemUseAnimation> {
    let usage = hands.item_use?;
    let displayed = match usage.hand {
        native_session::animation::Hand::Main => hands.main.item.as_deref(),
        native_session::animation::Hand::Off => hands.off.item.as_deref(),
    };
    let duration_ticks = displayed.and_then(|item| {
        let max = item.max_count()?;
        (native_session::inventory::saved_item_max(item.id(), item.components()) == Some(max))
            .then(|| native_session::survival::food_properties(item))
            .flatten()
            .map(|food| food.use_ticks)
    });
    Some(ItemUseAnimation {
        hand: usage.hand,
        remaining_ticks: usage.remaining_ticks,
        duration_ticks,
        partial_tick,
    })
}

fn offhand_hotbar_layout(width: i32, height: i32, main_hand_left: bool) -> (Rect, (i32, i32)) {
    let center = width / 2;
    let sprite_x = if main_hand_left {
        center + 91
    } else {
        center - 91 - 29
    };
    let item_x = if main_hand_left {
        center + 91 + 10
    } else {
        center - 91 - 26
    };
    (
        Rect {
            x: sprite_x,
            y: height - 23,
            w: 29,
            h: 24,
        },
        (item_x, height - 19),
    )
}

fn storage_slot_position(index: u8) -> (i32, i32) {
    if index < 9 {
        (8 + i32::from(index) * 18, 142)
    } else {
        let index = index - 9;
        (
            8 + i32::from(index % 9) * 18,
            84 + i32::from(index / 9) * 18,
        )
    }
}

pub fn hotbar_key(code: KeyCode) -> Option<u8> {
    match code {
        KeyCode::Digit1 => Some(0),
        KeyCode::Digit2 => Some(1),
        KeyCode::Digit3 => Some(2),
        KeyCode::Digit4 => Some(3),
        KeyCode::Digit5 => Some(4),
        KeyCode::Digit6 => Some(5),
        KeyCode::Digit7 => Some(6),
        KeyCode::Digit8 => Some(7),
        KeyCode::Digit9 => Some(8),
        _ => None,
    }
}

pub fn inventory_swap_key(code: KeyCode) -> Option<PlayerSlot> {
    if code == KeyCode::KeyF {
        Some(PlayerSlot::Offhand)
    } else {
        hotbar_key(code).map(PlayerSlot::Storage)
    }
}

fn player_slot_position(slot: PlayerSlot) -> (i32, i32) {
    match slot {
        PlayerSlot::Storage(index) => storage_slot_position(index),
        PlayerSlot::Crafting(index) => (
            98 + i32::from(index % 2) * 18,
            18 + i32::from(index / 2) * 18,
        ),
        PlayerSlot::CraftResult => (154, 28),
        PlayerSlot::Armor(index) => (8, 8 + i32::from(index) * 18),
        PlayerSlot::Offhand => (77, 62),
        PlayerSlot::Chest(_) => unreachable!("chest slots are not in the player menu"),
    }
}

fn menu_slot_position(slot: PlayerSlot, menu: InventoryMenu) -> Option<(i32, i32)> {
    match menu {
        InventoryMenu::Player if !matches!(slot, PlayerSlot::Chest(_)) => {
            Some(player_slot_position(slot))
        }
        InventoryMenu::Player => None,
        InventoryMenu::CraftingTable { .. } => match slot {
            PlayerSlot::Storage(index) if index < 36 => Some(storage_slot_position(index)),
            PlayerSlot::Crafting(index) if index < 9 => Some((
                30 + i32::from(index % 3) * 18,
                17 + i32::from(index / 3) * 18,
            )),
            PlayerSlot::CraftResult => Some((124, 35)),
            _ => None,
        },
        InventoryMenu::Chest { other, .. } => match slot {
            PlayerSlot::Chest(index) if index < if other.is_some() { 54 } else { 27 } => Some((
                8 + i32::from(index % 9) * 18,
                18 + i32::from(index / 9) * 18,
            )),
            PlayerSlot::Storage(index) if index < 36 => {
                let (x, y) = storage_slot_position(index);
                Some((x, y + if other.is_some() { 55 } else { 1 }))
            }
            _ => None,
        },
    }
}

#[cfg(test)]
fn inventory_slot_at(
    size: (u32, u32),
    cursor: (i32, i32),
    menu: InventoryMenu,
    book_open: bool,
) -> Option<PlayerSlot> {
    inventory_slot_at_scale(size, cursor, menu, book_open, gui_scale(size.0, size.1))
}

fn inventory_slot_at_scale(
    size: (u32, u32),
    cursor: (i32, i32),
    menu: InventoryMenu,
    book_open: bool,
    scale: u32,
) -> Option<PlayerSlot> {
    if size.0 == 0 || size.1 == 0 {
        return None;
    }
    let (w, h) = gui_size(size, scale);
    let (w, h) = (w as i32, h as i32);
    let book_open = book_open && !matches!(menu, InventoryMenu::Chest { .. });
    if book_open && w < 379 {
        return None;
    }
    let origin = if let InventoryMenu::Chest { other, .. } = menu {
        (
            (w - 176) / 2,
            (h - if other.is_some() { 222 } else { 168 }) / 2,
        )
    } else {
        inventory_origin(w, h, book_open)
    };
    (0..36)
        .map(PlayerSlot::Storage)
        .chain((0..9).map(PlayerSlot::Crafting))
        .chain(std::iter::once(PlayerSlot::CraftResult))
        .chain((0..4).map(PlayerSlot::Armor))
        .chain(std::iter::once(PlayerSlot::Offhand))
        .chain((0..54).map(PlayerSlot::Chest))
        .find(|&slot| {
            if matches!(menu, InventoryMenu::Player) && matches!(slot, PlayerSlot::Crafting(4..)) {
                return false;
            }
            let Some((x, y)) = menu_slot_position(slot, menu) else {
                return false;
            };
            Rect {
                x: origin.0 + x,
                y: origin.1 + y,
                w: 16,
                h: 16,
            }
            .contains(cursor.0, cursor.1)
        })
}

#[cfg(test)]
fn inventory_outside_at(size: (u32, u32), cursor: (i32, i32), book_open: bool) -> bool {
    inventory_outside_at_scale(size, cursor, book_open, gui_scale(size.0, size.1))
}

fn inventory_outside_at_scale(
    size: (u32, u32),
    cursor: (i32, i32),
    book_open: bool,
    scale: u32,
) -> bool {
    if size.0 == 0 || size.1 == 0 {
        return false;
    }
    let (w, h) = gui_size(size, scale);
    let (x, y) = inventory_origin(w as i32, h as i32, book_open);
    !Rect {
        x,
        y,
        w: 176,
        h: 166,
    }
    .contains(cursor.0, cursor.1)
}

fn inventory_slot_icon(
    canvas: &mut Canvas,
    assets: &MenuAssets,
    models: &mut ItemModelCache,
    position: (i32, i32),
    inventory: &PlayerInventory,
    slot: PlayerSlot,
    preview: Option<&QuickCraftPreview>,
) {
    let seed = inventory
        .menu()
        .and_then(|menu| menu_slot_position(slot, menu))
        .map_or(0, |(x, y)| x + y * 176);
    let selected =
        preview.and_then(|preview| preview.slots.iter().find(|entry| entry.slot == slot));
    if let Some(selected) = selected {
        if let Some(item) = &selected.item {
            canvas.fill(
                Rect {
                    x: position.0,
                    y: position.1,
                    w: 16,
                    h: 16,
                },
                [255, 255, 255, 128],
            );
            stack_icon_with_cap(
                canvas,
                assets,
                models,
                position.0,
                position.1,
                item,
                selected.capped,
                seed,
            );
        }
    } else if let Some(item) = inventory.slot(slot) {
        stack_icon_seeded(canvas, assets, models, position.0, position.1, item, seed);
    }
}

pub(crate) fn stack_icon(
    canvas: &mut Canvas,
    assets: &MenuAssets,
    models: &mut ItemModelCache,
    x: i32,
    y: i32,
    item: &ItemStack,
) {
    stack_icon_with_cap(canvas, assets, models, x, y, item, false, 0);
}

fn stack_icon_seeded(
    canvas: &mut Canvas,
    assets: &MenuAssets,
    models: &mut ItemModelCache,
    x: i32,
    y: i32,
    item: &ItemStack,
    seed: i32,
) {
    stack_icon_with_cap(canvas, assets, models, x, y, item, false, seed);
}

fn stack_icon_with_cap(
    canvas: &mut Canvas,
    assets: &MenuAssets,
    models: &mut ItemModelCache,
    x: i32,
    y: i32,
    item: &ItemStack,
    capped: bool,
    seed: i32,
) {
    let cached_model = models.get_item_seeded(item, seed);
    if let Some((icon, uv, pixels)) = cached_model.as_ref().and_then(|model| {
        model
            .gui_icon
            .as_ref()
            .map(|icon| (icon, model.gui_glint_uv, model.gui_glint_pixels.as_deref()))
    }) {
        canvas.image(icon, Rect { x, y, w: 16, h: 16 });
        if item.has_glint() {
            canvas.mark_item_glint_with_mapping(icon, Rect { x, y, w: 16, h: 16 }, uv, pixels);
        }
    } else {
        match item.id() {
            "minecraft:grass_block" => {
                cube_icon(canvas, x, y, &assets.grass_top, &assets.grass_side, true)
            }
            "minecraft:dirt" => cube_icon(canvas, x, y, &assets.dirt, &assets.dirt, false),
            "minecraft:stone" => cube_icon(canvas, x, y, &assets.stone, &assets.stone, false),
            "minecraft:cobblestone" => cube_icon(
                canvas,
                x,
                y,
                &assets.cobblestone,
                &assets.cobblestone,
                false,
            ),
            "minecraft:stone_bricks" => cube_icon(
                canvas,
                x,
                y,
                &assets.stone_bricks,
                &assets.stone_bricks,
                false,
            ),
            "minecraft:stone_button" => {
                let mut icon = Canvas::new(16, 16);
                cube_icon(&mut icon, 0, 0, &assets.stone, &assets.stone, false);
                let image = RgbaImage::from_fn(16, 16, |x, y| {
                    let pixel = icon.pixels[(y * 16 + x) as usize];
                    image::Rgba([
                        (pixel >> 16) as u8,
                        (pixel >> 8) as u8,
                        pixel as u8,
                        (pixel >> 24) as u8,
                    ])
                });
                canvas.image(
                    &image,
                    Rect {
                        x: x + 3,
                        y: y + 4,
                        w: 10,
                        h: 8,
                    },
                );
            }
            _ => {
                canvas.fill(Rect { x, y, w: 16, h: 16 }, [255, 0, 255, 255]);
                canvas.fill(Rect { x, y, w: 8, h: 8 }, [0, 0, 0, 255]);
                canvas.fill(
                    Rect {
                        x: x + 8,
                        y: y + 8,
                        w: 8,
                        h: 8,
                    },
                    [0, 0, 0, 255],
                );
            }
        }
    }
    if let Some((width, color)) = item.durability_bar() {
        canvas.fill(
            Rect {
                x: x + 2,
                y: y + 13,
                w: 13,
                h: 2,
            },
            [0, 0, 0, 255],
        );
        canvas.fill(
            Rect {
                x: x + 2,
                y: y + 13,
                w: width as i32,
                h: 1,
            },
            [color[0], color[1], color[2], 255],
        );
    }
    if item.count() > 1 || capped {
        stack_count(canvas, assets, x, y, item.count(), capped);
    }
}

fn stack_count(canvas: &mut Canvas, assets: &MenuAssets, x: i32, y: i32, count: u32, yellow: bool) {
    let count = count.to_string();
    let left = x + 17 - canvas.text_width(Some(assets), &count);
    let color = if yellow {
        [255, 255, 85]
    } else {
        [255, 255, 255]
    };
    let shadow = [color[0] / 4, color[1] / 4, color[2] / 4];
    canvas.text(Some(assets), left + 1, y + 10, &count, shadow);
    canvas.text(Some(assets), left, y + 9, &count, color);
}

fn sample(texture: &RgbaImage, u: f32, v: f32, shade: f32, green: bool) -> [u8; 4] {
    let x = ((u.clamp(0.0, 0.999) * texture.width() as f32) as u32).min(texture.width() - 1);
    let y = ((v.clamp(0.0, 0.999) * texture.height() as f32) as u32).min(texture.height() - 1);
    let p = texture.get_pixel(x, y).0;
    let tint = if green {
        [0.57, 0.74, 0.35]
    } else {
        [1.0, 1.0, 1.0]
    };
    [
        (p[0] as f32 * shade * tint[0]) as u8,
        (p[1] as f32 * shade * tint[1]) as u8,
        (p[2] as f32 * shade * tint[2]) as u8,
        p[3],
    ]
}

fn cube_icon(canvas: &mut Canvas, x: i32, y: i32, top: &RgbaImage, side: &RgbaImage, grass: bool) {
    if top.width() == 0 || top.height() == 0 || side.width() == 0 || side.height() == 0 {
        return;
    }
    for py in 0..16 {
        for px in 0..16 {
            let (fx, fy) = (px as f32, py as f32);
            let u = (fx - 7.5) / 14.0 + fy / 8.0;
            let v = -(fx - 7.5) / 14.0 + fy / 8.0;
            let color = if (0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v) {
                Some(sample(top, u, v, 1.0, grass))
            } else if (1..8).contains(&px) {
                let top_y = 4.0 + (fx - 1.0) * 4.0 / 7.0;
                if fy >= top_y && fy < top_y + 7.0 {
                    Some(sample(
                        side,
                        (fx - 1.0) / 7.0,
                        (fy - top_y) / 7.0,
                        0.7,
                        false,
                    ))
                } else {
                    None
                }
            } else if (8..15).contains(&px) {
                let top_y = 8.0 - (fx - 8.0) * 4.0 / 7.0;
                if fy >= top_y && fy < top_y + 7.0 {
                    Some(sample(
                        side,
                        (fx - 8.0) / 7.0,
                        (fy - top_y) / 7.0,
                        0.82,
                        false,
                    ))
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(color) = color {
                canvas.pixel(x + px, y + py, color);
            }
        }
    }
}

#[cfg(test)]
mod inventory_tests {
    use super::*;

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn menu_capture_results_arrive_in_game_before_and_after_handoff() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&game, &[]).unwrap());
        let mut ui = GameUi::new(assets);
        let captures = crate::screenshot::MenuScreenshots::new().unwrap();
        let sender = captures.sender();
        sender.report_error(anyhow::anyhow!("capture before transition"));
        captures.collect();
        crate::deliver_menu_screenshots(Some(&mut ui), Some(&captures), 0);
        assert!(
            matches!(ui.last_system_message(), Some(native_session::SystemMessageKind::ScreenshotFailed(error)) if error.contains("before transition"))
        );
        let dir = tempfile::tempdir().unwrap();
        sender
            .submit_software(dir.path(), (1, 1), vec![0xff0000])
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            crate::deliver_menu_screenshots(Some(&mut ui), Some(&captures), 1);
            if let Some(native_session::SystemMessageKind::ScreenshotSaved(path)) =
                ui.last_system_message()
            {
                assert_eq!(
                    path.parent(),
                    Some(dir.path().join("screenshots").as_path())
                );
                assert!(path.is_file());
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn screenshot_click_only_resolves_a_visible_safe_filename() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut assets = MenuAssets::load(&game, &[]).unwrap();
        assets.chat = crate::chat_hud::ChatSettings::default();
        assets
            .language
            .insert("screenshot.success".into(), "Saved screenshot as %s".into());
        let mut ui = GameUi::new(Arc::new(assets));
        ui.resize((1280, 720));
        ui.screenshot_result(Ok(game.join("screenshots/test.png")), 0);
        ui.open_chat(false);
        let height = ui.logical_size().1;
        let measure = Canvas::new(1, 1);
        let prefix = measure.text_width(Some(&ui.assets), "Saved screenshot as ");
        ui.cursor = (4 + prefix + 1, height - 40 - 8 + 1);
        assert_eq!(ui.chat_click(true), None);
        assert_eq!(
            ui.chat_click(false),
            Some(game.join("screenshots/test.png"))
        );
        ui.cursor = (4, height - 40 - 8 + 1);
        assert_eq!(ui.chat_click(false), None);
        let other = tempfile::tempdir().unwrap();
        let original = other.path().join("screenshots/other.png");
        ui.screenshot_result_with_age(Ok(original.clone()), 0, std::time::Duration::from_secs(15));
        assert_eq!(ui.system_messages.first().unwrap().tick, 0);
        assert_eq!(
            crate::chat_hud::display_key(
                &ui.assets.chat,
                &ui.system_messages[..1],
                CHAT_AGE_OFFSET
            ),
            None
        );
        assert_eq!(ui.chat_click(false), None); // still over the prefix
        ui.cursor = (4 + prefix + 1, height - 40 - 8 + 1);
        // The newest displayed message is the earlier, unexpired capture.
        assert_eq!(
            ui.chat_click(false),
            Some(game.join("screenshots/test.png"))
        );
        ui.system_messages = Arc::from([ui.system_messages.first().unwrap().clone()]);
        assert_eq!(ui.chat_click(false), Some(original));
        ui.system_messages = Arc::from([native_session::SystemMessage {
            tick: 0,
            kind: native_session::SystemMessageKind::ScreenshotSaved(
                std::path::PathBuf::from("../outside.png").into(),
            ),
        }]);
        ui.cursor = (4 + prefix + 1, height - 40 - 8 + 1);
        assert_eq!(ui.chat_click(false), None);
        ui.close_chat();
        assert_eq!(ui.chat_click(false), None);
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn chat_reveals_old_messages_and_sends_local_text_without_showing_hidden_hud() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut assets = MenuAssets::load(&game, &[]).unwrap();
        assets.chat = crate::chat_hud::ChatSettings::default();
        let assets = Arc::new(assets);
        let mut ui = GameUi::new(assets);
        let inventory = PlayerInventory::default();
        ui.screenshot_result(Ok("capture.png".into()), 0);
        ui.toggle_hud();
        let hidden = ui.compose((1280, 720), &inventory, 240).unwrap();
        assert!(hidden.pixels.iter().all(|&pixel| pixel == 0));
        ui.open_chat(false);
        let opened = ui.compose((1280, 720), &inventory, 240).unwrap();
        assert!(opened.pixels.iter().any(|&pixel| pixel != 0));
        let mut history = Canvas::new(opened.width as u32, opened.height as u32);
        crate::chat_hud::draw_focused(&mut history, &ui.assets, &ui.system_messages, 240, 0);
        assert!(history.pixels.iter().any(|&pixel| pixel != 0));
        ui.chat_preedit("한".into());
        assert!(ui.chat_composing());
        ui.chat_preedit(String::new());
        ui.chat_text("한글 대화");
        assert_eq!(
            ui.chat_key(KeyCode::Enter, false, false, 240),
            crate::chat_input::ChatAction::Submit("한글 대화".into())
        );
        assert!(!ui.chat_open());
        assert!(
            matches!(ui.last_system_message(), Some(native_session::SystemMessageKind::LocalChat(text)) if text.as_ref() == "한글 대화")
        );
        assert!(ui.hud_hidden());
        ui.open_chat(true);
        ui.chat_text("time set day");
        ui.chat_key(KeyCode::Enter, false, false, 241);
        assert!(matches!(
            ui.last_system_message(),
            Some(native_session::SystemMessageKind::CommandUnavailable)
        ));
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn hidden_hud_leaves_screens_visible_and_clears_blurred_hud() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&game, &[]).unwrap());
        let mut ui = GameUi::new(assets);
        let mut inventory = PlayerInventory::default();
        let visible = ui.compose((1280, 720), &inventory, 0).unwrap();
        assert!(visible.pixels.iter().any(|&pixel| pixel != 0));

        ui.toggle_hud();
        let hidden = ui.compose((1280, 720), &inventory, 0).unwrap();
        assert!(hidden.pixels.iter().all(|&pixel| pixel == 0));
        inventory.toggle();
        let inventory_screen = ui.compose((1280, 720), &inventory, 0).unwrap();
        assert!(inventory_screen.pixels.iter().any(|&pixel| pixel != 0));
        assert!(
            ui.pre_blur_hud
                .as_ref()
                .unwrap()
                .pixels
                .iter()
                .all(|&pixel| pixel == 0)
        );

        inventory.toggle();
        ui.toggle_pause();
        let pause_screen = ui.compose((1280, 720), &inventory, 0).unwrap();
        assert!(pause_screen.pixels.iter().any(|&pixel| pixel != 0));
        assert!(
            ui.pre_blur_hud
                .as_ref()
                .unwrap()
                .pixels
                .iter()
                .all(|&pixel| pixel == 0)
        );
        ui.toggle_pause();
        ui.toggle_hud();
        let restored = ui.compose((1280, 720), &inventory, 0).unwrap();
        assert_eq!(restored.pixels, visible.pixels);
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn hud_is_separated_from_screen_when_blur_is_enabled() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&game, &[]).unwrap());
        let mut ui = GameUi::new(assets);
        let mut inventory = PlayerInventory::default();
        let base = ui.compose((1280, 720), &inventory, 0).unwrap();
        assert!(ui.pre_blur_hud.is_none());
        inventory.toggle();
        let screen = ui.compose((1280, 720), &inventory, 0).unwrap();
        let hud = ui.pre_blur_hud.as_ref().expect("HUD should precede blur");
        assert_eq!(hud.pixels, base.pixels);
        assert_ne!(screen.pixels, base.pixels);
        ui.set_video_settings(VideoSettings {
            menu_blur: 0,
            ..VideoSettings::default()
        });
        let unblurred = ui.compose((1280, 720), &inventory, 0).unwrap();
        assert!(ui.pre_blur_hud.is_none());
        assert_ne!(unblurred.pixels, screen.pixels);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and NATIVE_MENDING_TEST_WORLD pointing to a synthetic world copy"]
    fn saved_mending_tool_is_repaired_after_death_and_respawn() {
        use native_persistence::nbt::Tag;
        use native_session::{GameSession, PlayerCommand, inventory::PlayerSlot};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let world = std::path::PathBuf::from(
            std::env::var_os("NATIVE_MENDING_TEST_WORLD").expect("set NATIVE_MENDING_TEST_WORLD"),
        )
        .canonicalize()
        .unwrap();
        let local = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.local")
            .canonicalize()
            .unwrap();
        assert!(
            world.starts_with(local)
                && world
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("xp-gui-world-mending")
        );
        let files = [
            world.join("level.dat"),
            world.join("players/data/00000000-0000-0000-0000-000000000001.dat"),
            world.join("dimensions/minecraft/overworld/region/r.0.0.mca"),
        ];
        let before: Vec<_> = files
            .iter()
            .map(|path| std::fs::read(path).unwrap())
            .collect();
        let assets = Arc::new(MenuAssets::load(&game, &[]).unwrap());
        let prepared = crate::prepare_existing_world(
            &world,
            &game,
            &[],
            crate::ViewOptions {
                dimension: None,
                chunk: None,
                radius: 0,
                smoke_test: false,
                block_report: None,
                play: true,
                video: crate::menu_settings::VideoSettings::default(),
            },
            Some(Arc::clone(&assets)),
        )
        .unwrap();
        let crate::PreparedWorld::Play(mut prepared) = prepared else {
            panic!("expected play session")
        };
        let session = &mut prepared.session;
        let start = session.snapshot();
        let saved_item = start.inventory.slot(PlayerSlot::Storage(0)).unwrap();
        assert_eq!(
            saved_item.components().get("minecraft:damage"),
            Some(&Tag::Int(400))
        );
        assert_eq!(saved_item.max_count(), Some(1));
        for _ in 0..100 {
            session.update(std::time::Duration::from_millis(50));
            if session.snapshot().death.is_some() {
                break;
            }
        }
        let death = session.snapshot();
        assert!(death.death.is_some());
        assert_eq!(death.dropped_items.len(), 1);
        assert_eq!(
            death
                .experience_orbs
                .iter()
                .map(|orb| orb.value)
                .sum::<i32>(),
            100
        );
        for _ in 0..80 {
            session.update(std::time::Duration::from_millis(50));
        }
        let drop_position = session.snapshot().dropped_items[0].position;
        assert!(session.snapshot().death.unwrap().can_respawn);
        session.submit_command(PlayerCommand::Respawn);
        session.update(std::time::Duration::from_millis(50));
        let spawn = session.snapshot().player.position;
        for _ in 0..100 {
            session.update(std::time::Duration::from_millis(50));
            if session.snapshot().dropped_items.is_empty()
                && session.snapshot().experience_orbs.is_empty()
            {
                break;
            }
        }
        let after = session.snapshot();
        let item = after
            .inventory
            .slot(PlayerSlot::Storage(0))
            .unwrap_or_else(|| {
                panic!(
                    "item not picked up: drop={drop_position:?}, spawn={spawn:?}, orbs={}, xp={:?}",
                    after.experience_orbs.len(),
                    after.experience
                )
            });
        assert_eq!(item.id(), "minecraft:diamond_pickaxe");
        assert_eq!(
            item.components().get("minecraft:damage"),
            Some(&Tag::Int(200)),
            "orbs remaining: {}",
            after.experience_orbs.len()
        );
        assert_eq!(after.experience.total, 0);
        let mut ui = GameUi::new(assets);
        ui.sync_hud(&after);
        assert!(
            ui.compose((1280, 720), &after.inventory, after.tick)
                .is_some()
        );
        for (path, expected) in files.iter().zip(before) {
            assert_eq!(std::fs::read(path).unwrap(), expected);
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and NATIVE_XP_TEST_WORLD pointing to a synthetic world copy"]
    fn prepared_world_experience_reaches_the_composed_hud() {
        use native_session::{GameSession, experience::Experience};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let world = std::path::PathBuf::from(
            std::env::var_os("NATIVE_XP_TEST_WORLD").expect("set NATIVE_XP_TEST_WORLD"),
        );
        let world = world.canonicalize().unwrap();
        let local = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.local")
            .canonicalize()
            .unwrap();
        assert!(
            world.starts_with(local)
                && world
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("xp-gui-world-")
        );
        let player = world.join("players/data/00000000-0000-0000-0000-000000000001.dat");
        let region = world.join("dimensions/minecraft/overworld/region/r.-1.-1.mca");
        let before = [
            std::fs::read(world.join("level.dat")).unwrap(),
            std::fs::read(&player).unwrap(),
            std::fs::read(&region).unwrap(),
        ];
        let pack = tempfile::tempdir().unwrap();
        let sprites = pack
            .path()
            .join("assets/minecraft/textures/gui/sprites/hud");
        std::fs::create_dir_all(&sprites).unwrap();
        RgbaImage::from_pixel(182, 5, image::Rgba([20, 30, 40, 255]))
            .save(sprites.join("experience_bar_background.png"))
            .unwrap();
        RgbaImage::from_pixel(182, 5, image::Rgba([80, 190, 40, 255]))
            .save(sprites.join("experience_bar_progress.png"))
            .unwrap();
        let packs = vec![pack.path().to_path_buf()];
        let assets = Arc::new(MenuAssets::load(&game, &packs).unwrap());
        let prepared = crate::prepare_existing_world(
            &world,
            &game,
            &packs,
            crate::ViewOptions {
                dimension: None,
                chunk: None,
                radius: 0,
                smoke_test: false,
                block_report: None,
                play: true,
                video: crate::menu_settings::VideoSettings::default(),
            },
            Some(Arc::clone(&assets)),
        )
        .unwrap();
        let crate::PreparedWorld::Play(prepared) = prepared else {
            panic!("expected a play session")
        };
        let snapshot = prepared.session.snapshot();
        assert_eq!(
            snapshot.experience,
            Experience {
                progress: 0.5,
                level: 17,
                total: 400
            }
        );
        let mut ui = GameUi::new(assets);
        ui.sync_hud(&snapshot);
        let canvas = ui
            .compose((1280, 720), &snapshot.inventory, snapshot.tick)
            .unwrap();
        let (x, y) = ((canvas.width - 182) / 2, canvas.height - 29);
        assert_eq!(
            canvas.pixels[((y + 2) * canvas.width + x + 10) as usize],
            0xff50_be28
        );
        assert_eq!(
            canvas.pixels[((y + 2) * canvas.width + x + 100) as usize],
            0xff14_1e28
        );
        assert!(canvas.pixels.contains(&0xff80_ff20), "level text missing");
        assert!(
            ui.compose((1280, 720), &snapshot.inventory, snapshot.tick)
                .is_none(),
            "unchanged HUD must stay cached"
        );
        let mut changed = (*snapshot).clone();
        changed.experience.total += 1;
        ui.sync_hud(&changed);
        assert!(
            ui.compose((1280, 720), &changed.inventory, changed.tick)
                .is_none(),
            "invisible total change must stay cached"
        );
        changed.experience.progress = 0.75;
        ui.sync_hud(&changed);
        let updated = ui
            .compose((1280, 720), &changed.inventory, changed.tick)
            .expect("visible XP change must redraw");
        assert_eq!(
            updated.pixels[((y + 2) * updated.width + x + 100) as usize],
            0xff50_be28
        );
        drop(prepared);
        assert_eq!(
            before,
            [
                std::fs::read(world.join("level.dat")).unwrap(),
                std::fs::read(player).unwrap(),
                std::fs::read(region).unwrap()
            ]
        );
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn debug_focus_message_survives_session_messages_and_updates_setting() {
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&directory, &[]).unwrap());
        let mut ui = GameUi::new(assets);
        ui.debug_focus_pause(false, 12);
        assert!(!ui.video_settings().pause_on_lost_focus);
        assert!(ui.video_needs_save());
        ui.snapshot_messages = Arc::from([native_session::SystemMessage {
            kind: native_session::SystemMessageKind::MissingRespawnBlock,
            tick: 10,
        }]);
        ui.merge_chat_messages();
        assert_eq!(ui.system_messages.len(), 2);
        assert_eq!(
            ui.system_messages[1].kind,
            native_session::SystemMessageKind::DebugFocusPause(false)
        );
        let canvas = ui
            .compose((1280, 720), &PlayerInventory::default(), 12)
            .unwrap();
        assert!(
            canvas.pixels.iter().any(|pixel| *pixel == 0xffff_ff55),
            "debug prefix should use the Java yellow"
        );
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn system_message_uses_translations_and_redraws_only_during_fade() {
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut assets = MenuAssets::load(&directory, &[]).unwrap();
        assets.chat = crate::chat_hud::ChatSettings::default();
        assets.language.insert(
            "block.minecraft.spawn.not_valid".into(),
            "TEST LOCAL MESSAGE".into(),
        );
        let assets = Arc::new(assets);
        let mut ui = GameUi::new(Arc::clone(&assets));
        ui.system_messages = Arc::from([native_session::SystemMessage {
            kind: native_session::SystemMessageKind::MissingRespawnBlock,
            tick: 0,
        }]);
        let inventory = PlayerInventory::default();
        let canvas = ui.compose((1280, 720), &inventory, 0).unwrap();
        let bottom = canvas.height - 40;
        assert_eq!(
            canvas.pixels[((bottom - 8) * canvas.width) as usize],
            0xffd0d0d0
        );
        let mut expected = Canvas::new(canvas.width as u32, canvas.height as u32);
        expected.fill(
            Rect {
                x: 0,
                y: bottom - 9,
                w: 332,
                h: 9,
            },
            [0, 0, 0, 127],
        );
        expected.fill(
            Rect {
                x: 0,
                y: bottom - 9,
                w: 2,
                h: 9,
            },
            [208, 208, 208, 255],
        );
        expected.text(
            Some(&assets),
            4,
            bottom - 8,
            "TEST LOCAL MESSAGE",
            [255, 255, 255],
        );
        let region = |image: &Canvas| -> Vec<u32> {
            ((bottom - 9)..bottom)
                .flat_map(|y| (0..120).map(move |x| image.pixels[(y * image.width + x) as usize]))
                .collect()
        };
        assert_eq!(region(&canvas), region(&expected));
        assert!(ui.compose((1280, 720), &inventory, 180).is_none());
        assert!(ui.compose((1280, 720), &inventory, 181).is_some());
        assert!(ui.compose((1280, 720), &inventory, 199).is_some());
        let expired = ui.compose((1280, 720), &inventory, 200).unwrap();
        assert_eq!(expired.pixels[((bottom - 8) * expired.width) as usize], 0);
        assert!(ui.compose((1280, 720), &inventory, 201).is_none());
    }

    #[test]
    fn use_animation_follows_the_displayed_hand_and_supported_item() {
        use native_persistence::nbt::Tag;
        use native_session::animation::{FirstPersonState, Hand, ItemUseState};
        let mut hands = FirstPersonState::default();
        assert!(displayed_item_use(&hands, 0.5).is_none());
        hands.item_use = Some(ItemUseState {
            hand: Hand::Main,
            remaining_ticks: 23,
        });
        hands.main.item =
            ItemStack::new("minecraft:stone".into(), 1, 64, Tag::Compound(vec![])).map(Arc::new);
        let visual = displayed_item_use(&hands, 0.5).unwrap();
        assert_eq!(visual.hand, Hand::Main);
        assert_eq!(visual.remaining_ticks, 23);
        assert_eq!(visual.partial_tick, 0.5);
        assert_eq!(visual.duration_ticks, None);
        hands.main.item =
            ItemStack::new("minecraft:apple".into(), 1, 64, Tag::Compound(vec![])).map(Arc::new);
        assert_eq!(
            displayed_item_use(&hands, 0.5).unwrap().duration_ticks,
            Some(32)
        );
        hands.main.item = ItemStack::new(
            "minecraft:apple".into(),
            1,
            64,
            Tag::Compound(vec![(
                "minecraft:consumable".into(),
                Tag::Compound(vec![("consume_seconds".into(), Tag::Float(15.0))]),
            )]),
        )
        .map(Arc::new);
        assert_eq!(
            displayed_item_use(&hands, 0.5).unwrap().duration_ticks,
            Some(300)
        );
        hands.item_use = Some(ItemUseState {
            hand: Hand::Off,
            remaining_ticks: 10,
        });
        hands.off.item =
            ItemStack::new("minecraft:dried_kelp".into(), 1, 64, Tag::Compound(vec![]))
                .map(Arc::new);
        assert_eq!(
            displayed_item_use(&hands, 0.5).unwrap().duration_ticks,
            Some(16)
        );
        hands.off.item = ItemStack::unresolved(
            "minecraft:apple".into(),
            1,
            Tag::Compound(vec![("minecraft:consumable".into(), Tag::Compound(vec![]))]),
        )
        .map(Arc::new);
        assert_eq!(
            displayed_item_use(&hands, 0.5).unwrap().duration_ticks,
            None
        );
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn survival_hud_uses_resource_pack_sprites_and_invalidates_when_needed() {
        use native_session::{GameMode, survival::PlayerVitals};
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let pack = tempfile::tempdir().unwrap();
        let hearts = pack
            .path()
            .join("assets/minecraft/textures/gui/sprites/hud/heart");
        let food = hearts.parent().unwrap();
        std::fs::create_dir_all(&hearts).unwrap();
        RgbaImage::from_pixel(9, 9, image::Rgba([231, 19, 47, 255]))
            .save(hearts.join("full.png"))
            .unwrap();
        RgbaImage::from_pixel(9, 9, image::Rgba([32, 214, 81, 255]))
            .save(food.join("food_full.png"))
            .unwrap();
        RgbaImage::from_pixel(9, 9, image::Rgba([31, 117, 231, 255]))
            .save(food.join("air.png"))
            .unwrap();
        RgbaImage::from_pixel(9, 9, image::Rgba([242, 138, 19, 255]))
            .save(food.join("air_bursting.png"))
            .unwrap();
        RgbaImage::from_pixel(9, 9, image::Rgba([132, 145, 151, 255]))
            .save(food.join("air_empty.png"))
            .unwrap();
        RgbaImage::from_pixel(9, 9, image::Rgba([18, 177, 203, 255]))
            .save(food.join("armor_full.png"))
            .unwrap();
        RgbaImage::from_pixel(9, 9, image::Rgba([197, 63, 110, 255]))
            .save(food.join("armor_half.png"))
            .unwrap();
        let assets = Arc::new(MenuAssets::load(&directory, &[pack.path().to_path_buf()]).unwrap());
        let mut ui = GameUi::new(assets);
        let inventory = PlayerInventory::default();
        let canvas = ui.compose((1280, 720), &inventory, 0).unwrap();
        let left = canvas.width / 2 - 91;
        let right = canvas.width / 2 + 91;
        let top = canvas.height - 39;
        assert_eq!(
            canvas.pixels[((top + 4) * canvas.width + left + 4) as usize],
            0xffe7132f
        );
        assert_eq!(
            canvas.pixels[((top + 4) * canvas.width + right - 5) as usize],
            0xff20d651
        );
        assert!(
            ui.compose((1280, 720), &inventory, 1).is_none(),
            "stable HUD should stay cached"
        );
        ui.hud = HudDisplay::default().with_armor(Some(native_session::ArmorValues {
            armor: 3.0,
            toughness: 0.0,
        }));
        let armor = ui.compose((1280, 720), &inventory, 2).unwrap();
        assert_eq!(
            armor.pixels[((top - 6) * armor.width + left + 4) as usize],
            0xff12b1cb
        );
        assert_eq!(
            armor.pixels[((top - 6) * armor.width + left + 12) as usize],
            0xffc53f6e
        );
        assert!(
            ui.compose((1280, 720), &inventory, 3).is_none(),
            "unchanged armor stays cached"
        );
        ui.hud = HudDisplay::new(
            PlayerVitals {
                saturation: 0.0,
                ..Default::default()
            },
            GameMode::Survival,
        );
        assert!(ui.compose((1280, 720), &inventory, 2).is_some());
        assert!(
            ui.compose((1280, 720), &inventory, 3).is_none(),
            "unchanged hunger must stay cached"
        );
        assert!(
            ui.compose((1280, 720), &inventory, 61).is_some(),
            "hunger movement must redraw"
        );
        assert!(
            ui.compose((1280, 720), &inventory, 62).is_some(),
            "hunger must return to its resting position"
        );
        assert!(
            ui.compose((1280, 720), &inventory, 63).is_none(),
            "the resting position must stay cached"
        );
        ui.hud = HudDisplay::with_air(PlayerVitals::default(), GameMode::Survival, true);
        let submerged = ui.compose((1280, 720), &inventory, 64).unwrap();
        assert_eq!(
            submerged.pixels[((top - 6) * submerged.width + right - 5) as usize],
            0xff1f75e7
        );
        ui.hud = HudDisplay::with_air(
            PlayerVitals {
                air: 299,
                ..PlayerVitals::default()
            },
            GameMode::Survival,
            true,
        );
        assert!(
            ui.compose((1280, 720), &inventory, 65).is_none(),
            "unchanged bubbles stay cached"
        );
        ui.hud = HudDisplay::with_air(
            PlayerVitals {
                air: 271,
                ..PlayerVitals::default()
            },
            GameMode::Survival,
            true,
        );
        let popping = ui.compose((1280, 720), &inventory, 66).unwrap();
        assert_eq!(
            popping.pixels[((top - 6) * popping.width + right - 77) as usize],
            0xfff28a13
        );
        ui.hud = HudDisplay::with_air(
            PlayerVitals {
                air: 0,
                ..PlayerVitals::default()
            },
            GameMode::Survival,
            true,
        );
        let empty = ui.compose((1280, 720), &inventory, 68).unwrap();
        assert_eq!(
            empty.pixels[((top - 6) * empty.width + right - 5) as usize],
            0xff849197
        );
        assert!(
            ui.compose((1280, 720), &inventory, 69).is_some(),
            "empty-bubble animation redraws"
        );
        ui.hud = HudDisplay::new(PlayerVitals::default(), GameMode::Creative);
        let creative = ui.compose((1280, 720), &inventory, 70).unwrap();
        assert_eq!(
            creative.pixels[((top + 4) * creative.width + left + 4) as usize],
            0
        );
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn pause_sound_settings_support_mouse_keyboard_and_save() {
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&directory, &[]).unwrap());
        let mut ui = GameUi::new(assets);
        ui.sound = SoundSettings::default();
        let (audio, probe) = crate::audio::Audio::test_channel();
        let packs = vec![std::path::PathBuf::from("test-selected-pack.zip")];
        ui.set_audio(Some(crate::audio::UiAudio::new(
            audio,
            directory.clone(),
            packs.clone(),
        )));
        assert_eq!(probe.gain(), 1.0);
        let inventory = PlayerInventory::default();
        let size = (1280, 720);
        let scale = f64::from(gui_scale(size.0, size.1));
        let w = (f64::from(size.0) / scale) as i32;
        let h = (f64::from(size.1) / scale) as i32;
        ui.compose(size, &inventory, 0).unwrap();
        assert!(ui.toggle_pause());
        ui.compose(size, &inventory, 0).unwrap();
        ui.cursor_moved(f64::from(w / 2 - 90) * scale, f64::from(h / 4 + 88) * scale);
        assert_eq!(ui.click(), PauseAction::None);
        assert_eq!(probe.click(), Some((directory.clone(), packs.clone())));
        assert_eq!(ui.pause_page, PausePage::Options);
        ui.cursor_moved(f64::from(w / 2 + 10) * scale, 65.0 * scale);
        assert_eq!(ui.click(), PauseAction::None);
        assert_eq!(probe.click(), Some((directory.clone(), packs.clone())));
        assert_eq!(ui.pause_page, PausePage::Sound);
        assert!(ui.compose(size, &inventory, 0).is_some());
        let master = ui.sound_ui.rect(w, SoundCategory::Master);
        ui.cursor_moved(
            f64::from(master.x + 4) * scale,
            f64::from(master.y + 8) * scale,
        );
        assert_eq!(ui.click(), PauseAction::None);
        assert_eq!(probe.click(), None, "slider must be silent on press");
        assert_eq!(probe.gain(), 0.0);
        ui.pause_release();
        assert_eq!(probe.click(), Some((directory.clone(), packs.clone())));
        ui.pause_release();
        assert_eq!(probe.click(), None, "release must be emitted only once");
        ui.cursor_moved(
            f64::from(master.x + master.w - 4) * scale,
            f64::from(master.y + 8) * scale,
        );
        ui.click();
        assert_eq!(probe.gain(), 1.0);
        ui.pause_cancel();
        ui.pause_release();
        assert_eq!(probe.click(), None, "focus loss cancels slider sound");
        ui.pause_key(KeyCode::ArrowLeft, false);
        assert!(probe.gain() < 1.0);
        ui.cursor_moved(
            f64::from(master.x + 4) * scale,
            f64::from(master.y + 8) * scale,
        );
        ui.click();
        ui.pause_release();
        assert!(probe.click().is_some());
        assert_eq!(ui.sound.get(SoundCategory::Master), 0.0);
        assert!(ui.sound_needs_save());
        assert_eq!(ui.pause_key(KeyCode::Escape, false), PauseAction::SaveSound);
        assert_eq!(probe.click(), None, "Escape does not activate a button");
        assert_eq!(ui.pause_page, PausePage::Options);
        ui.sound_save_result(Err(anyhow::anyhow!("test error")));
        assert!(ui.sound_needs_save());
        ui.sound_save_result(Ok(()));
        assert!(!ui.sound_needs_save());
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn pause_mouse_settings_support_mouse_keyboard_and_apply() {
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&directory, &[]).unwrap());
        let mut ui = GameUi::new(assets);
        let inventory = PlayerInventory::default();
        let size = (1280, 720);
        let scale = f64::from(gui_scale(size.0, size.1));
        let w = (f64::from(size.0) / scale) as i32;
        let h = (f64::from(size.1) / scale) as i32;
        ui.compose(size, &inventory, 0).unwrap();
        ui.toggle_pause();
        ui.cursor_moved(f64::from(w / 2 - 90) * scale, f64::from(h / 4 + 88) * scale);
        assert_eq!(ui.click(), PauseAction::None);
        ui.cursor_moved(f64::from(w / 2 + 10) * scale, 90.0 * scale);
        assert_eq!(ui.click(), PauseAction::None);
        assert_eq!(ui.pause_page, PausePage::Controls);
        for (toggle, button) in [
            (ControlToggle::Sneak, PauseButton::ToggleSneak),
            (ControlToggle::Sprint, PauseButton::ToggleSprint),
            (ControlToggle::Attack, PauseButton::ToggleAttack),
            (ControlToggle::Use, PauseButton::ToggleUse),
        ] {
            let rect = mouse_ui::control_toggle_rect(w, toggle);
            ui.cursor_moved(
                f64::from(rect.x + 10) * scale,
                f64::from(rect.y + 10) * scale,
            );
            assert_eq!(ui.click(), PauseAction::None);
            assert_eq!(ui.pause_focus, Some(button));
        }
        assert!(
            ui.video.toggle_sneak
                && ui.video.toggle_sprint
                && ui.video.toggle_attack
                && ui.video.toggle_use
        );
        ui.pause_key(KeyCode::Space, false);
        assert!(!ui.video.toggle_use);
        let sprint = SprintWindowUi::rect(w);
        ui.cursor_moved(
            f64::from(sprint.x + sprint.w - 4) * scale,
            f64::from(sprint.y + 10) * scale,
        );
        ui.click();
        assert_eq!(ui.video.sprint_window, 10);
        ui.cursor_moved(
            f64::from(sprint.x + 4) * scale,
            f64::from(sprint.y + 10) * scale,
        );
        ui.pause_release();
        assert_eq!(ui.video.sprint_window, 0);
        ui.pause_focus = Some(PauseButton::SprintWindow);
        ui.sprint_window_ui.keyboard_adjust = true;
        ui.pause_key(KeyCode::ArrowRight, false);
        assert_eq!(ui.video.sprint_window, 1);
        assert!(ui.video_needs_save());
        ui.cursor_moved(f64::from(w / 2 - 145) * scale, 45.0 * scale);
        assert_eq!(ui.click(), PauseAction::None);
        assert_eq!(ui.pause_page, PausePage::Mouse);
        assert!(ui.compose(size, &inventory, 0).is_some());
        let rect = mouse_ui::mouse_control_rect(w, MouseControl::Sensitivity);
        ui.cursor_moved(
            f64::from(rect.x + rect.w - 4) * scale,
            f64::from(rect.y + 10) * scale,
        );
        ui.click();
        ui.pause_release();
        assert_eq!(ui.video.mouse_sensitivity.value(), 1.0);
        let wheel = mouse_ui::mouse_control_rect(w, MouseControl::WheelSensitivity);
        ui.cursor_moved(
            f64::from(wheel.x + wheel.w - 4) * scale,
            f64::from(wheel.y + 10) * scale,
        );
        ui.click();
        ui.pause_release();
        assert_eq!(ui.video.mouse_wheel_sensitivity.value(), 10.0);
        let discrete = mouse_ui::mouse_control_rect(w, MouseControl::DiscreteScrolling);
        ui.cursor_moved(
            f64::from(discrete.x + 10) * scale,
            f64::from(discrete.y + 10) * scale,
        );
        ui.click();
        assert!(ui.video.discrete_mouse_scroll);
        for control in [MouseControl::InvertX, MouseControl::InvertY] {
            let rect = mouse_ui::mouse_control_rect(w, control);
            ui.cursor_moved(
                f64::from(rect.x + 10) * scale,
                f64::from(rect.y + 10) * scale,
            );
            ui.click();
        }
        assert!(ui.video.invert_mouse_x && ui.video.invert_mouse_y);
        ui.pause_focus = None;
        ui.pause_key(KeyCode::Tab, false);
        assert_eq!(ui.pause_focus, Some(PauseButton::MouseSensitivity));
        ui.pause_key(KeyCode::ArrowLeft, false);
        assert!(ui.video.mouse_sensitivity.value() < 1.0);
        let mut applied = VideoSettings::default();
        let mut pacer = crate::frame_pacer::FramePacer::new(120, std::time::Instant::now());
        crate::apply_game_video(Some(&ui), &mut applied, &mut pacer, None);
        assert_eq!(applied, ui.video_settings());
        let (yaw, pitch) = crate::mouse_look::look_delta((10.0, 10.0), applied);
        assert!(yaw < 0.0 && pitch > 0.0);
        assert_eq!(ui.pause_key(KeyCode::Escape, false), PauseAction::SaveVideo);
        assert_eq!(ui.pause_page, PausePage::Controls);
        assert!(ui.video_needs_save());
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 assets and a working Windows audio device"]
    fn pause_button_plays_through_audio_worker() {
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&directory, &[]).unwrap());
        let mut ui = GameUi::new(assets);
        ui.sound = SoundSettings::default();
        let audio = crate::audio::Audio::new().unwrap();
        audio.prepare(&directory, &[]);
        ui.set_audio(Some(crate::audio::UiAudio::new(audio, directory, vec![])));
        let size = (1280, 720);
        let scale = f64::from(gui_scale(size.0, size.1));
        let w = (f64::from(size.0) / scale) as i32;
        let h = (f64::from(size.1) / scale) as i32;
        ui.compose(size, &PlayerInventory::default(), 0).unwrap();
        ui.toggle_pause();
        ui.cursor_moved(f64::from(w / 2 - 90) * scale, f64::from(h / 4 + 88) * scale);
        crate::audio::reset_test_rendered();
        ui.click();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !crate::audio::test_rendered() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            crate::audio::test_rendered(),
            "pause click did not reach audio output"
        );
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn pause_video_settings_support_mouse_keyboard_and_navigation() {
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&directory, &[]).unwrap());
        let mut ui = GameUi::new(assets);
        let inventory = PlayerInventory::default();
        let size = (1280, 720);
        let scale = gui_scale(size.0, size.1) as f64;
        let (w, h) = (
            (size.0 as f64 / scale) as i32,
            (size.1 as f64 / scale) as i32,
        );
        let initial = VideoSettings {
            vsync: true,
            max_fps: 120,
            fov: 70,
            view_bobbing: true,
            inactivity_fps_limit: crate::menu_settings::InactivityFpsLimit::Afk,
            ..VideoSettings::default()
        };
        ui.set_video_settings(initial);
        ui.compose(size, &inventory, 0).unwrap();
        assert!(ui.toggle_pause());
        ui.compose(size, &inventory, 0).unwrap();
        ui.cursor_moved(
            f64::from(w / 2 - 90) * scale,
            f64::from(h / 4 + 8 + 80) * scale,
        );
        assert_eq!(ui.click(), PauseAction::None);
        assert_eq!(ui.pause_page, PausePage::Options);
        ui.compose(size, &inventory, 0).unwrap();
        ui.cursor_moved(f64::from(w / 2 - 9) * scale, 45.0 * scale);
        assert_eq!(ui.click(), PauseAction::None);
        ui.pause_release();
        assert_eq!(ui.video_settings().fov, 110);
        ui.cursor_moved(f64::from(w / 2 - 145) * scale, 90.0 * scale);
        assert_eq!(ui.click(), PauseAction::None);
        assert_eq!(ui.pause_page, PausePage::Video);
        ui.compose(size, &inventory, 0).unwrap();

        ui.cursor_moved(f64::from(w / 2 - 145) * scale, 140.0 * scale);
        assert_eq!(ui.click(), PauseAction::None);
        assert!(ui.video_settings().fullscreen);
        ui.pause_key(KeyCode::Enter, false);
        assert!(!ui.video_settings().fullscreen);

        ui.cursor_moved(f64::from(w / 2 + 15) * scale, 110.0 * scale);
        assert_eq!(ui.click(), PauseAction::None);
        assert_eq!(ui.video_settings().gui_scale, 1);
        assert_eq!(ui.gui_scale(), 1);
        for expected in [2, 3, 0] {
            ui.pause_key(KeyCode::Enter, false);
            assert_eq!(ui.video_settings().gui_scale, expected);
        }
        assert_eq!(ui.gui_scale(), 3);

        ui.cursor_moved(f64::from(w / 2 + 15) * scale, 90.0 * scale);
        assert_eq!(ui.click(), PauseAction::None);
        assert!(!ui.video_settings().vsync);
        ui.cursor_moved(f64::from(w / 2 - 145) * scale, 110.0 * scale);
        assert_eq!(ui.click(), PauseAction::None);
        assert_eq!(
            ui.video_settings().inactivity_fps_limit,
            crate::menu_settings::InactivityFpsLimit::Minimized
        );
        ui.pause_key(KeyCode::Enter, false);
        assert_eq!(
            ui.video_settings().inactivity_fps_limit,
            crate::menu_settings::InactivityFpsLimit::Afk
        );
        ui.cursor_moved(f64::from(w / 2 - 150) * scale, 90.0 * scale);
        ui.click();
        ui.cursor_moved(f64::from(w / 2 - 9) * scale, 90.0 * scale);
        ui.pause_release();
        assert_eq!(ui.video_settings().max_fps, 260);
        assert!(ui.video_needs_save());
        ui.video_ui
            .ensure_control_visible(h, VideoControl::RenderDistance);
        let rect = ui.video_ui.control_rect(w, VideoControl::RenderDistance);
        ui.cursor_moved(
            f64::from(rect.x + rect.w - 4) * scale,
            f64::from(rect.y + 10) * scale,
        );
        ui.click();
        ui.pause_release();
        assert_eq!(ui.video_settings().render_distance, 12);
        ui.advance_video_settings(
            std::time::Instant::now() + std::time::Duration::from_millis(601),
        );
        assert_eq!(ui.video_settings().render_distance, 32);

        assert_eq!(ui.pause_key(KeyCode::Escape, false), PauseAction::SaveVideo);
        assert!(ui.paused());
        assert_eq!(ui.pause_page, PausePage::Options);
        ui.video_save_result(Err(anyhow::anyhow!("test error")));
        assert!(ui.video_needs_save());
        assert!(ui.video_error.is_some());
        ui.video_save_result(Ok(()));
        assert!(!ui.video_needs_save());
        assert!(ui.video_error.is_none());
        ui.cursor_moved(f64::from(w / 2 + 15) * scale, 140.0 * scale);
        assert_eq!(ui.click(), PauseAction::None);
        assert_eq!(ui.pause_page, PausePage::Accessibility);
        ui.compose(size, &inventory, 0).unwrap();
        ui.pause_scroll(-8.0);
        ui.pause_key(KeyCode::Tab, false);
        assert_eq!(ui.pause_focus, Some(PauseButton::MenuBlur));
        ui.pause_key(KeyCode::ArrowLeft, false);
        assert_eq!(ui.video_settings().menu_blur, 4);
        ui.pause_key(KeyCode::Tab, false);
        assert_eq!(ui.pause_focus, Some(PauseButton::ViewBobbing));
        assert!(ui.accessibility_ui.visible(h));
        ui.pause_key(KeyCode::Enter, false);
        assert!(!ui.video_settings().view_bobbing);
        ui.pause_key(KeyCode::Tab, false);
        assert_eq!(ui.pause_focus, Some(PauseButton::FovEffectScale));
        ui.pause_key(KeyCode::ArrowLeft, false);
        assert_eq!(
            ui.video_settings().fov_effect_scale.value(),
            1.0 + f64::from(-1.0_f32 / 142.0)
        );
        ui.pause_key(KeyCode::Tab, false);
        assert_eq!(ui.pause_focus, Some(PauseButton::GlintSpeed));
        ui.pause_key(KeyCode::ArrowLeft, false);
        assert_eq!(
            ui.video_settings().glint_speed.value(),
            0.5 + f64::from(-1.0_f32 / 142.0)
        );
        ui.pause_key(KeyCode::Tab, false);
        assert_eq!(ui.pause_focus, Some(PauseButton::GlintStrength));
        ui.pause_key(KeyCode::ArrowRight, false);
        assert_eq!(
            ui.video_settings().glint_strength.value(),
            0.75 + f64::from(1.0_f32 / 142.0)
        );
        ui.pause_key(KeyCode::Tab, false);
        assert_eq!(ui.pause_focus, Some(PauseButton::PanoramaSpeed));
        ui.pause_key(KeyCode::ArrowLeft, false);
        assert_eq!(
            ui.video_settings().panorama_speed.value(),
            1.0 + f64::from(-1.0_f32 / 142.0)
        );
        assert_eq!(ui.pause_key(KeyCode::Escape, false), PauseAction::SaveVideo);
        ui.video_save_result(Ok(()));
        assert_eq!(ui.pause_key(KeyCode::Escape, false), PauseAction::None);
        assert_eq!(ui.pause_page, PausePage::Game);
        assert_eq!(ui.pause_key(KeyCode::Escape, false), PauseAction::Resume);
        ui.resume();
        assert!(!ui.paused());

        ui.toggle_pause();
        ui.pause_key(KeyCode::Tab, false);
        ui.pause_key(KeyCode::Tab, false);
        ui.pause_key(KeyCode::Enter, false);
        ui.pause_key(KeyCode::Tab, false);
        ui.pause_key(KeyCode::ArrowLeft, false);
        assert_eq!(ui.video_settings().fov, 109);
        ui.pause_key(KeyCode::Tab, false);
        assert_eq!(ui.pause_focus, Some(PauseButton::Sound));
        ui.pause_key(KeyCode::Tab, false);
        ui.pause_key(KeyCode::Enter, false);
        assert_eq!(ui.pause_page, PausePage::Video);
        ui.pause_key(KeyCode::Tab, false);
        ui.pause_key(KeyCode::Enter, false);
        ui.pause_key(KeyCode::ArrowLeft, false);
        assert_eq!(ui.video_settings().max_fps, 250);
        ui.pause_scroll(-100.0);
        assert_eq!(ui.pause_focus, None);
        ui.pause_key(KeyCode::Tab, false);
        assert_eq!(ui.pause_focus, Some(PauseButton::VideoBlend));
        ui.pause_key(KeyCode::Enter, false);
        ui.pause_key(KeyCode::ArrowRight, false);
        ui.advance_video_settings(
            std::time::Instant::now() + std::time::Duration::from_millis(601),
        );
        assert_eq!(ui.video_settings().biome_blend, 3);
        ui.pause_key(KeyCode::Tab, false);
        assert_eq!(ui.pause_focus, Some(PauseButton::VideoDistance));
        ui.pause_key(KeyCode::Enter, false);
        ui.pause_key(KeyCode::ArrowLeft, false);
        ui.advance_video_settings(
            std::time::Instant::now() + std::time::Duration::from_millis(601),
        );
        assert_eq!(ui.video_settings().render_distance, 31);
        ui.pause_key(KeyCode::Tab, false);
        assert_eq!(ui.pause_focus, Some(PauseButton::VideoBlur));
        ui.pause_key(KeyCode::Enter, false);
        ui.pause_key(KeyCode::ArrowRight, false);
        assert_eq!(ui.video_settings().menu_blur, 5);
        ui.pause_key(KeyCode::Tab, false);
        assert_eq!(ui.pause_focus, Some(PauseButton::VideoWeather));
        ui.pause_key(KeyCode::Enter, false);
        ui.pause_key(KeyCode::ArrowLeft, false);
        assert_eq!(ui.video_settings().weather_radius, 9);
        ui.pause_key(KeyCode::Tab, false);
        assert_eq!(ui.pause_focus, Some(PauseButton::Done));
    }

    #[test]
    fn sprite_icons_composite_tinted_layers_and_transparency() {
        use crate::held_items::{DisplayTransform, HeldModel, SpriteLayer};
        let transform = DisplayTransform {
            translation: glam::Vec3::ZERO,
            rotation: glam::Vec3::ZERO,
            scale: glam::Vec3::ONE,
        };
        let mut model = HeldModel {
            particle_texture: None,
            numeric_item_id: None,
            glint: false,
            glint_materials: Vec::new(),
            quads: Vec::new(),
            right: transform,
            left: transform,
            first_right: transform,
            first_left: transform,
            ground: transform,
            gui: transform,
            gui_side_light: false,
            gui_icon: None,
            gui_glint_uv: None,
            gui_glint_pixels: None,
            gui_glint_quads: None,
            sprite_layers: vec![
                SpriteLayer {
                    texture: Arc::new(RgbaImage::from_pixel(
                        2,
                        2,
                        image::Rgba([255, 255, 255, 255]),
                    )),
                    tint: [1.0, 0.0, 0.0],
                    glint_uv: None,
                },
                SpriteLayer {
                    texture: Arc::new(RgbaImage::from_pixel(
                        2,
                        2,
                        image::Rgba([255, 255, 255, 128]),
                    )),
                    tint: [0.0, 1.0, 0.0],
                    glint_uv: None,
                },
            ],
        };
        let icon = crate::item_gui::render(&model).unwrap();
        let color = icon.get_pixel(0, 0).0;
        assert_eq!(color[3], 255);
        assert!(color[0].abs_diff(127) <= 1);
        assert!(color[1].abs_diff(128) <= 1);
        assert_eq!(color[2], 0);
        model.gui.rotation.z = 15.0;
        assert!(crate::item_gui::render(&model).is_none());
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn selected_gui_scale_updates_rendered_inventory_and_pointer() {
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&directory, &[]).unwrap());
        let mut ui = GameUi::new(assets);
        let mut inventory = PlayerInventory::default();
        assert!(inventory.toggle());
        let size = (1280, 720);
        for (requested, scale) in [(0, 3), (1, 1), (2, 2), (3, 3)] {
            ui.set_video_settings(VideoSettings {
                gui_scale: requested,
                ..VideoSettings::default()
            });
            let canvas = ui.compose(size, &inventory, 0).unwrap();
            assert_eq!(
                (canvas.width, canvas.height),
                (size.0.div_ceil(scale) as i32, size.1.div_ceil(scale) as i32)
            );
            let origin = inventory_origin(canvas.width, canvas.height, false);
            let (x, y) = player_slot_position(PlayerSlot::Storage(0));
            ui.cursor_moved(
                f64::from((origin.0 + x + 8) * scale as i32),
                f64::from((origin.1 + y + 8) * scale as i32),
            );
            assert_eq!(ui.inventory_slot(), Some(PlayerSlot::Storage(0)));
        }
        ui.cursor_moved(600.0, 300.0);
        ui.set_video_settings(VideoSettings {
            gui_scale: 1,
            ..VideoSettings::default()
        });
        assert_eq!(ui.cursor, (600, 300));
        ui.set_video_settings(VideoSettings {
            gui_scale: 3,
            ..VideoSettings::default()
        });
        assert_eq!(ui.cursor, (200, 100));
    }

    #[test]
    fn selected_gui_scales_move_inventory_hit_targets_with_the_layout() {
        let size = (1280, 720);
        for requested in [0, 1, 2, 3, 9] {
            let scale = gui_scale_for(size.0, size.1, requested);
            let w = (size.0 / scale) as i32;
            let h = (size.1 / scale) as i32;
            for menu in [
                InventoryMenu::Player,
                InventoryMenu::CraftingTable {
                    position: glam::IVec3::ZERO,
                },
                InventoryMenu::Chest {
                    position: glam::IVec3::ZERO,
                    other: None,
                },
            ] {
                let origin = match menu {
                    InventoryMenu::Chest { .. } => ((w - 176) / 2, (h - 168) / 2),
                    _ => inventory_origin(w, h, false),
                };
                let slot = PlayerSlot::Storage(0);
                let (x, y) = menu_slot_position(slot, menu).unwrap();
                assert_eq!(
                    inventory_slot_at_scale(
                        size,
                        (origin.0 + x + 8, origin.1 + y + 8),
                        menu,
                        false,
                        scale
                    ),
                    Some(slot)
                );
            }
        }
    }

    #[test]
    fn slot_hit_testing_uses_vanilla_inventory_coordinates() {
        let size = (1280, 720);
        let origin = (1280 / 3 / 2 - 88, 720 / 3 / 2 - 83);
        for slot in (0..36)
            .map(PlayerSlot::Storage)
            .chain((0..4).map(PlayerSlot::Crafting))
            .chain(std::iter::once(PlayerSlot::CraftResult))
            .chain((0..4).map(PlayerSlot::Armor))
            .chain(std::iter::once(PlayerSlot::Offhand))
        {
            let (x, y) = player_slot_position(slot);
            assert_eq!(
                inventory_slot_at(
                    size,
                    (origin.0 + x + 8, origin.1 + y + 8),
                    InventoryMenu::Player,
                    false
                ),
                Some(slot)
            );
        }
        assert_eq!(
            inventory_slot_at(
                size,
                (origin.0 + 153, origin.1 + 28),
                InventoryMenu::Player,
                false
            ),
            None
        );
    }

    #[test]
    fn crafting_table_slots_use_vanilla_coordinates() {
        let size = (1280, 720);
        let origin = (1280 / 3 / 2 - 88, 720 / 3 / 2 - 83);
        let menu = InventoryMenu::CraftingTable {
            position: glam::IVec3::ZERO,
        };
        for slot in (0..36)
            .map(PlayerSlot::Storage)
            .chain((0..9).map(PlayerSlot::Crafting))
            .chain([PlayerSlot::CraftResult])
        {
            let (x, y) = menu_slot_position(slot, menu).unwrap();
            assert_eq!(
                inventory_slot_at(size, (origin.0 + x + 8, origin.1 + y + 8), menu, false),
                Some(slot)
            );
        }
        assert_eq!(
            inventory_slot_at(size, (origin.0 + 7, origin.1 + 42), menu, false),
            None
        );
    }

    #[test]
    fn single_chest_slots_use_vanilla_coordinates_even_with_recipe_book_open() {
        let size = (1280, 720);
        let menu = InventoryMenu::Chest {
            position: glam::IVec3::ZERO,
            other: None,
        };
        let origin = ((1280 / 3 - 176) / 2, (720 / 3 - 168) / 2);
        for slot in (0..27)
            .map(PlayerSlot::Chest)
            .chain((0..36).map(PlayerSlot::Storage))
        {
            let (x, y) = menu_slot_position(slot, menu).unwrap();
            assert_eq!(
                inventory_slot_at(size, (origin.0 + x + 8, origin.1 + y + 8), menu, true),
                Some(slot)
            );
        }
        assert_eq!(
            menu_slot_position(PlayerSlot::Chest(0), menu),
            Some((8, 18))
        );
        assert_eq!(
            menu_slot_position(PlayerSlot::Chest(26), menu),
            Some((152, 54))
        );
        assert_eq!(
            menu_slot_position(PlayerSlot::Storage(9), menu),
            Some((8, 85))
        );
        assert_eq!(
            menu_slot_position(PlayerSlot::Storage(0), menu),
            Some((8, 143))
        );
        assert_eq!(menu_slot_position(PlayerSlot::CraftResult, menu), None);
    }

    #[test]
    fn double_chest_slots_use_vanilla_coordinates() {
        let size = (1280, 720);
        let menu = InventoryMenu::Chest {
            position: glam::IVec3::ZERO,
            other: Some(glam::IVec3::X),
        };
        let origin = ((1280 / 3 - 176) / 2, (720 / 3 - 222) / 2);
        for slot in (0..54)
            .map(PlayerSlot::Chest)
            .chain((0..36).map(PlayerSlot::Storage))
        {
            let (x, y) = menu_slot_position(slot, menu).unwrap();
            assert_eq!(
                inventory_slot_at(size, (origin.0 + x + 8, origin.1 + y + 8), menu, true),
                Some(slot)
            );
        }
        assert_eq!(
            menu_slot_position(PlayerSlot::Chest(53), menu),
            Some((152, 108))
        );
        assert_eq!(
            menu_slot_position(PlayerSlot::Storage(9), menu),
            Some((8, 139))
        );
        assert_eq!(
            menu_slot_position(PlayerSlot::Storage(0), menu),
            Some((8, 197))
        );
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn double_chest_composes_using_vanilla_texture() {
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&directory, &[]).unwrap());
        let mut ui = GameUi::new(assets);
        let mut inventory = PlayerInventory::demo();
        assert!(inventory.open_double_chest(
            glam::IVec3::ZERO,
            glam::IVec3::X,
            std::array::from_fn(|_| None),
            std::array::from_fn(|_| None)
        ));
        let canvas = ui.compose((1280, 720), &inventory, 0).unwrap();
        let origin = ((canvas.width - 176) / 2, (canvas.height - 222) / 2);
        let pixel =
            |x: i32, y: i32| canvas.pixels[((origin.1 + y) * canvas.width + origin.0 + x) as usize];
        assert_ne!(pixel(4, 4) & 0x00ff_ffff, 0);
        assert_eq!(pixel(8, 18), pixel(8, 108));
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn single_chest_composes_using_vanilla_texture() {
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&directory, &[]).unwrap());
        let mut ui = GameUi::new(assets);
        let mut inventory = PlayerInventory::demo();
        assert!(inventory.open_chest(glam::IVec3::ZERO, std::array::from_fn(|_| None)));
        let canvas = ui.compose((1280, 720), &inventory, 0).unwrap();
        assert_eq!((canvas.width, canvas.height), (426, 240));
        let origin = ((canvas.width - 176) / 2, (canvas.height - 168) / 2);
        let pixel =
            |x: i32, y: i32| canvas.pixels[((origin.1 + y) * canvas.width + origin.0 + x) as usize];
        assert_ne!(pixel(4, 4) & 0x00ff_ffff, 0);
        assert_eq!(pixel(8, 18), pixel(26, 18));
        assert!(!ui.recipe_book_captures_text());
    }

    #[test]
    fn only_clicks_outside_the_inventory_background_drop_the_cursor() {
        let size = (1280, 720);
        let origin = (1280 / 3 / 2 - 88, 720 / 3 / 2 - 83);
        assert!(!inventory_outside_at(size, origin, false));
        assert!(!inventory_outside_at(
            size,
            (origin.0 + 175, origin.1 + 165),
            false
        ));
        assert!(inventory_outside_at(size, (origin.0 - 1, origin.1), false));
        assert!(inventory_outside_at(
            size,
            (origin.0 + 176, origin.1),
            false
        ));
        assert!(!inventory_outside_at((0, 0), (0, 0), false));
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn inventory_pointer_draws_preview_and_commits_drag() {
        use native_persistence::nbt::Tag;
        use native_session::{
            GameSession, LocalSession, PlayerCommand, inventory::InventoryPointerEvent,
        };
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut ui = GameUi::new(Arc::new(MenuAssets::load(&directory, &[]).unwrap()));
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(
            PlayerSlot::Storage(0),
            ItemStack::new("minecraft:stone".into(), 8, 64, Tag::Compound(vec![])),
        );
        inventory.toggle();
        let mut session = LocalSession::demo();
        session.set_player_inventory(inventory);
        ui.compose((1280, 720), &session.snapshot().inventory, 0)
            .unwrap();
        let origin = (1280 / 3 / 2 - 88, 720 / 3 / 2 - 83);
        let move_to = |ui: &mut GameUi, index| {
            let (x, y) = storage_slot_position(index);
            ui.cursor_moved(
                f64::from((origin.0 + x + 8) * 3),
                f64::from((origin.1 + y + 8) * 3),
            );
        };
        let tick =
            |session: &mut LocalSession| session.update(std::time::Duration::from_millis(50));
        move_to(&mut ui, 0);
        let target = ui.inventory_pointer_press(ClickButton::Primary).unwrap();
        session.submit_command(PlayerCommand::InventoryPointer(
            InventoryPointerEvent::Press {
                target,
                button: ClickButton::Primary,
                shift: false,
            },
        ));
        let target = ui.inventory_pointer_release().unwrap();
        session.submit_command(PlayerCommand::InventoryPointer(
            InventoryPointerEvent::Release {
                target,
                button: ClickButton::Primary,
                shift: false,
            },
        ));
        tick(&mut session);
        assert_eq!(session.snapshot().inventory.cursor().unwrap().count(), 8);
        move_to(&mut ui, 1);
        let target = ui.inventory_pointer_press(ClickButton::Primary).unwrap();
        session.submit_command(PlayerCommand::InventoryPointer(
            InventoryPointerEvent::Press {
                target,
                button: ClickButton::Primary,
                shift: false,
            },
        ));
        let slot = ui.inventory_pointer_move().unwrap();
        assert_eq!(slot, PlayerSlot::Storage(1));
        assert!(ui.inventory_pointer_move().is_none());
        session.submit_command(PlayerCommand::InventoryPointer(
            InventoryPointerEvent::Move { slot },
        ));
        tick(&mut session);
        let one = ui
            .compose((1280, 720), &session.snapshot().inventory, 1)
            .unwrap();
        move_to(&mut ui, 2);
        let slot = ui.inventory_pointer_move().unwrap();
        session.submit_command(PlayerCommand::InventoryPointer(
            InventoryPointerEvent::Move { slot },
        ));
        tick(&mut session);
        let two = ui
            .compose((1280, 720), &session.snapshot().inventory, 2)
            .unwrap();
        let (x, y) = storage_slot_position(1);
        let changed = (0..16)
            .flat_map(|dy| {
                (0..16)
                    .map(move |dx| ((origin.1 + y + dy) * one.width + origin.0 + x + dx) as usize)
            })
            .any(|index| one.pixels[index] != two.pixels[index]);
        assert!(changed, "selected slot must show the projected stack");
        let target = ui.inventory_pointer_release().unwrap();
        session.submit_command(PlayerCommand::InventoryPointer(
            InventoryPointerEvent::Release {
                target,
                button: ClickButton::Primary,
                shift: false,
            },
        ));
        tick(&mut session);
        let snapshot = session.snapshot();
        assert_eq!(
            snapshot
                .inventory
                .slot(PlayerSlot::Storage(1))
                .unwrap()
                .count(),
            4
        );
        assert_eq!(
            snapshot
                .inventory
                .slot(PlayerSlot::Storage(2))
                .unwrap()
                .count(),
            4
        );
        assert!(snapshot.inventory.cursor().is_none());
        assert!(ui.inventory_pointer_press(ClickButton::Primary).is_some());
        session.submit_command(PlayerCommand::ToggleInventory);
        tick(&mut session);
        ui.compose((1280, 720), &session.snapshot().inventory, 3)
            .unwrap();
        assert!(!ui.inventory_pointer_active());
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn inventory_composes_using_local_vanilla_textures() {
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&directory, &[]).unwrap());
        let mut ui = GameUi::new(assets);
        let mut inventory = PlayerInventory::demo();
        assert!(inventory.toggle());
        let canvas = ui.compose((1280, 720), &inventory, 0).unwrap();
        assert_eq!((canvas.width, canvas.height), (426, 240));
        let origin = (canvas.width / 2 - 88, canvas.height / 2 - 83);
        let background = canvas.pixels[((origin.1 + 4) * canvas.width + origin.0 + 4) as usize];
        assert_ne!(background & 0x00ff_ffff, 0);
        if let Some(path) = std::env::var_os("NATIVE_INVENTORY_PREVIEW") {
            let image = RgbaImage::from_fn(canvas.width as u32, canvas.height as u32, |x, y| {
                let pixel = canvas.pixels[(y * canvas.width as u32 + x) as usize];
                image::Rgba([
                    (pixel >> 16) as u8,
                    (pixel >> 8) as u8,
                    pixel as u8,
                    (pixel >> 24) as u8,
                ])
            });
            image.save(path).unwrap();
        }
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn crafting_table_composes_using_local_vanilla_textures() {
        use native_persistence::nbt::Tag;
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&directory, &[]).unwrap());
        let mut ui = GameUi::new(assets);
        let mut inventory = PlayerInventory::default();
        assert!(inventory.open_crafting_table(glam::IVec3::ZERO));
        for index in [0, 1, 2] {
            inventory.set_slot(
                PlayerSlot::Crafting(index),
                ItemStack::new("minecraft:oak_planks".into(), 1, 64, Tag::Compound(vec![])),
            );
        }
        for index in [4, 7] {
            inventory.set_slot(
                PlayerSlot::Crafting(index),
                ItemStack::new("minecraft:stick".into(), 1, 64, Tag::Compound(vec![])),
            );
        }
        let mut resources = native_resources::packs::ResourceStack::open(&directory).unwrap();
        let recipes = native_resources::recipes::CraftingRecipes::load(&mut resources).unwrap();
        inventory.refresh_crafting(&recipes);
        let mut canvas = ui.compose((1280, 720), &inventory, 0).unwrap();
        let mut loaded = false;
        for _ in 0..300 {
            if let Some(next) = ui.compose((1280, 720), &inventory, 0) {
                canvas = next;
            }
            if [
                "minecraft:wooden_pickaxe",
                "minecraft:oak_planks",
                "minecraft:stick",
            ]
            .into_iter()
            .all(|id| ui.held_models.get(id).is_some())
            {
                loaded = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(loaded, "crafting icons did not finish loading");
        assert_eq!(
            inventory.slot(PlayerSlot::CraftResult).unwrap().id(),
            "minecraft:wooden_pickaxe"
        );
        assert_eq!((canvas.width, canvas.height), (426, 240));
        assert_eq!(ui.inventory_menu, inventory.menu());
        if let Some(path) = std::env::var_os("NATIVE_CRAFTING_PREVIEW") {
            let image = RgbaImage::from_fn(canvas.width as u32, canvas.height as u32, |x, y| {
                let pixel = canvas.pixels[(y * canvas.width as u32 + x) as usize];
                image::Rgba([
                    (pixel >> 16) as u8,
                    (pixel >> 8) as u8,
                    pixel as u8,
                    (pixel >> 24) as u8,
                ])
            });
            image.save(path).unwrap();
        }
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn recipe_book_opens_filters_and_places_a_saved_recipe() {
        use native_persistence::nbt::Tag;
        use native_resources::packs::{ResourceId, ResourceStack};
        use native_session::inventory::{RecipeBookPlacement, RecipeBookState};
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&directory, &[]).unwrap());
        let mut resources = ResourceStack::open(&directory).unwrap();
        let recipes = Arc::new(CraftingRecipes::load(&mut resources).unwrap());
        let id = ResourceId::parse("minecraft:stone_bricks").unwrap();
        let mut book = RecipeBookState::default();
        book.known.insert(id.clone());
        let mut inventory = PlayerInventory::default();
        inventory.set_recipe_book(book);
        inventory.set_slot(
            PlayerSlot::Storage(0),
            ItemStack::new("minecraft:stone".into(), 8, 64, Tag::Compound(vec![])),
        );
        assert!(inventory.open_crafting_table(glam::IVec3::ZERO));
        let mut ui = GameUi::new(assets);
        ui.set_crafting_recipes(Some(Arc::clone(&recipes)));
        ui.compose((1280, 720), &inventory, 0).unwrap();
        ui.cursor_moved(132.0 * 3.0, 73.0 * 3.0);
        assert_eq!(
            ui.recipe_book_click(ClickButton::Primary, false, &inventory),
            BookAction::Settings {
                open: true,
                filtering: false
            }
        );
        inventory.set_recipe_book_settings(true, false);
        let mut canvas = ui.compose((1280, 720), &inventory, 1).unwrap();
        for _ in 0..300 {
            if let Some(next) = ui.compose((1280, 720), &inventory, 1) {
                canvas = next;
            }
            if ["minecraft:bricks", "minecraft:stone_bricks"]
                .into_iter()
                .all(|id| ui.held_models.get(id).is_some())
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            inventory_slot_at(
                (1280, 720),
                (202 + 30 + 8, 37 + 17 + 8),
                inventory.menu().unwrap(),
                true
            ),
            Some(PlayerSlot::Crafting(0))
        );
        if let Some(path) = std::env::var_os("NATIVE_RECIPE_BOOK_PREVIEW") {
            let image = RgbaImage::from_fn(canvas.width as u32, canvas.height as u32, |x, y| {
                let pixel = canvas.pixels[(y * canvas.width as u32 + x) as usize];
                image::Rgba([
                    (pixel >> 16) as u8,
                    (pixel >> 8) as u8,
                    pixel as u8,
                    (pixel >> 24) as u8,
                ])
            });
            image.save(path).unwrap();
        }
        ui.cursor_moved(176.0 * 3.0, 57.0 * 3.0);
        assert_eq!(
            ui.recipe_book_click(ClickButton::Primary, false, &inventory),
            BookAction::Settings {
                open: true,
                filtering: true
            }
        );
        inventory.set_recipe_book_settings(true, true);
        ui.cursor_moved(83.0 * 3.0, 55.0 * 3.0);
        assert_eq!(
            ui.recipe_book_click(ClickButton::Primary, false, &inventory),
            BookAction::Consumed
        );
        assert!(ui.recipe_book_captures_text());
        ui.recipe_book_input("stone");
        inventory.set_slot(PlayerSlot::Storage(0), None);
        ui.cursor_moved(68.0 * 3.0, 72.0 * 3.0);
        assert_eq!(
            ui.recipe_book_click(ClickButton::Primary, false, &inventory),
            BookAction::Consumed
        );
        inventory.set_slot(
            PlayerSlot::Storage(0),
            ItemStack::new("minecraft:stone".into(), 8, 64, Tag::Compound(vec![])),
        );
        ui.cursor_moved(68.0 * 3.0, 72.0 * 3.0);
        let BookAction::Place {
            recipe, use_max, ..
        } = ui.recipe_book_click(ClickButton::Primary, true, &inventory)
        else {
            panic!("recipe button did not select the saved recipe");
        };
        assert!(use_max);
        assert_eq!(recipes.at(recipe).unwrap().id, id);
        assert_eq!(
            inventory.place_book_recipe(recipes.at(recipe).unwrap(), &recipes, use_max),
            RecipeBookPlacement::Placed
        );
        assert!(
            [0, 1, 3, 4].into_iter().all(|index| inventory
                .slot(PlayerSlot::Crafting(index))
                .unwrap()
                .count()
                == 2)
        );
        ui.compose((960, 720), &inventory, 2).unwrap();
        assert!(ui.recipe_book.is_open());
        assert_eq!(ui.inventory_slot(), None);
        assert_eq!(
            ui.recipe_book_escape(),
            Some(BookAction::Settings {
                open: false,
                filtering: true
            })
        );
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn grouped_recipe_overlay_selects_craftable_and_uncraftable_entries() {
        use native_persistence::nbt::Tag;
        use native_resources::packs::{ResourceId, ResourceStack};
        use native_session::inventory::{RecipeBookPlacement, RecipeBookState};
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&directory, &[]).unwrap());
        assert_eq!(
            assets.recipe_book.overlay_panel.scaling,
            crate::menu_assets::GuiSpriteScaling::NineSlice {
                width: 32,
                height: 32,
                border: [4; 4],
                stretch_inner: false,
            }
        );
        let mut resources = ResourceStack::open(&directory).unwrap();
        let recipes = Arc::new(CraftingRecipes::load(&mut resources).unwrap());
        let mut book = RecipeBookState {
            open: true,
            ..RecipeBookState::default()
        };
        for name in ["oak_planks", "birch_planks", "spruce_planks"] {
            book.known
                .insert(ResourceId::parse(&format!("minecraft:{name}")).unwrap());
        }
        let mut inventory = PlayerInventory::default();
        inventory.set_recipe_book(book);
        inventory.set_slot(
            PlayerSlot::Storage(0),
            ItemStack::new("minecraft:oak_log".into(), 8, 64, Tag::Compound(vec![])),
        );
        assert!(inventory.open_crafting_table(glam::IVec3::ZERO));
        let mut ui = GameUi::new(assets);
        ui.set_crafting_recipes(Some(Arc::clone(&recipes)));
        ui.compose((1280, 720), &inventory, 0).unwrap();
        ui.cursor_moved(65.0 * 3.0, 69.0 * 3.0);
        assert_eq!(
            ui.recipe_book_click(ClickButton::Secondary, false, &inventory),
            BookAction::Consumed
        );
        let mut canvas = ui.compose((1280, 720), &inventory, 0).unwrap();
        for _ in 0..300 {
            if let Some(next) = ui.compose((1280, 720), &inventory, 0) {
                canvas = next;
            }
            if ui.held_models.get("minecraft:oak_log").is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        if let Some(path) = std::env::var_os("NATIVE_RECIPE_OVERLAY_PREVIEW") {
            let image = RgbaImage::from_fn(canvas.width as u32, canvas.height as u32, |x, y| {
                let pixel = canvas.pixels[(y * canvas.width as u32 + x) as usize];
                image::Rgba([
                    (pixel >> 16) as u8,
                    (pixel >> 8) as u8,
                    pixel as u8,
                    (pixel >> 24) as u8,
                ])
            });
            image.save(path).unwrap();
        }
        ui.cursor_moved(69.0 * 3.0, 74.0 * 3.0);
        assert!(
            ui.compose((1280, 720), &inventory, 0).is_some(),
            "overlay hover must redraw"
        );
        let BookAction::Place { recipe, .. } =
            ui.recipe_book_click(ClickButton::Primary, false, &inventory)
        else {
            panic!("craftable overlay entry did not select a recipe");
        };
        assert_eq!(recipes.at(recipe).unwrap().id.path, "oak_planks");
        assert_eq!(
            inventory.place_book_recipe(recipes.at(recipe).unwrap(), &recipes, false),
            RecipeBookPlacement::Placed
        );
        assert_eq!(
            inventory.slot(PlayerSlot::CraftResult).unwrap().id(),
            "minecraft:oak_planks"
        );
        ui.cursor_moved(94.0 * 3.0, 74.0 * 3.0);
        assert!(
            ui.compose((1280, 720), &inventory, 0).is_some(),
            "moving overlay hover must redraw"
        );
        let BookAction::Place { recipe, .. } =
            ui.recipe_book_click(ClickButton::Primary, false, &inventory)
        else {
            panic!("uncraftable overlay entry did not select a recipe");
        };
        assert_ne!(recipes.at(recipe).unwrap().id.path, "oak_planks");
        assert_eq!(
            inventory.place_book_recipe(recipes.at(recipe).unwrap(), &recipes, false),
            RecipeBookPlacement::Ghost
        );
    }

    #[test]
    #[ignore = "performance measurement using installed Minecraft 26.3 assets"]
    fn measure_grouped_recipe_overlay_composition() {
        use native_persistence::nbt::Tag;
        use native_resources::packs::ResourceStack;
        use native_session::inventory::RecipeBookState;
        use std::{
            collections::BTreeSet,
            hint::black_box,
            time::{Duration, Instant},
        };

        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&directory, &[]).unwrap());
        let mut resources = ResourceStack::open(&directory).unwrap();
        let recipes = Arc::new(CraftingRecipes::load(&mut resources).unwrap());
        let mut book = RecipeBookState {
            open: true,
            ..RecipeBookState::default()
        };
        let mut ingredients = BTreeSet::new();
        for recipe in recipes.iter().filter(|recipe| recipe.group == "bark") {
            book.known.insert(recipe.id.clone());
            for ingredient in recipe.book_layout(3, 3).unwrap().into_iter().flatten() {
                if let Some(id) = ingredient.display_items(&recipes.tags).first() {
                    ingredients.insert((*id).to_owned());
                }
            }
        }
        assert!(book.known.len() >= 17);
        let mut inventory = PlayerInventory::default();
        inventory.set_recipe_book(book);
        inventory.set_slot(
            PlayerSlot::Storage(0),
            ItemStack::new("minecraft:oak_log".into(), 64, 64, Tag::Compound(vec![])),
        );
        inventory.open_crafting_table(glam::IVec3::ZERO);
        let mut ui = GameUi::new(assets);
        ui.set_crafting_recipes(Some(Arc::clone(&recipes)));
        ui.compose((1280, 720), &inventory, 0).unwrap();
        ui.cursor_moved(65.0 * 3.0, 69.0 * 3.0);
        assert_eq!(
            ui.recipe_book_click(ClickButton::Secondary, false, &inventory),
            BookAction::Consumed
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            ui.compose((1280, 720), &inventory, 0);
            if ingredients
                .iter()
                .all(|id| ui.held_models.get(id).is_some())
            {
                break;
            }
            assert!(Instant::now() < deadline, "ingredient icons did not load");
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut samples = Vec::new();
        for index in 0..40 {
            ui.cursor_moved((if index % 2 == 0 { 69.0 } else { 94.0 }) * 3.0, 49.0 * 3.0);
            let start = Instant::now();
            black_box(
                ui.compose((1280, 720), &inventory, 0)
                    .expect("overlay hover must redraw"),
            );
            samples.push(start.elapsed().as_secs_f64() * 1_000.0);
        }
        samples.sort_by(f64::total_cmp);
        println!(
            "{} grouped recipes; overlay composition median {:.3} ms, p95 {:.3} ms",
            inventory.recipe_book().known.len(),
            samples[20],
            samples[38]
        );
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn inventory_redraws_when_the_held_model_finishes_loading() {
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&directory, &[]).unwrap());
        let mut ui = GameUi::new(assets);
        let mut inventory = PlayerInventory::demo();
        inventory.toggle();
        let before = ui.compose((1280, 720), &inventory, 0).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let after = loop {
            if let Some(canvas) = ui.compose((1280, 720), &inventory, 0) {
                break canvas;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "held item model did not finish loading"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        let origin = (after.width / 2 - 88, after.height / 2 - 83);
        let changes = (origin.1 + 8..origin.1 + 78)
            .flat_map(|y| {
                (origin.0 + 26..origin.0 + 75).map(move |x| (y * after.width + x) as usize)
            })
            .filter(|&index| before.pixels[index] != after.pixels[index])
            .count();
        assert!(
            changes > 0,
            "the loaded model did not change the player preview"
        );
        if let Some(path) = std::env::var_os("NATIVE_HAND_PREVIEW") {
            let image = RgbaImage::from_fn(after.width as u32, after.height as u32, |x, y| {
                let pixel = after.pixels[(y * after.width as u32 + x) as usize];
                image::Rgba([
                    (pixel >> 16) as u8,
                    (pixel >> 8) as u8,
                    pixel as u8,
                    (pixel >> 24) as u8,
                ])
            });
            image.save(path).unwrap();
        }
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn inventory_redraws_the_avatar_when_armor_finishes_loading() {
        use native_persistence::nbt::Tag;
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&directory, &[]).unwrap());
        let mut ui = GameUi::new(assets);
        let mut inventory = PlayerInventory::default();
        inventory.toggle();
        let before = ui.compose((1280, 720), &inventory, 0).unwrap();
        let helmet =
            ItemStack::new("minecraft:iron_helmet".into(), 1, 1, Tag::Compound(vec![])).unwrap();
        inventory.set_slot(PlayerSlot::Armor(0), Some(helmet.clone()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let after = loop {
            let next = ui.compose((1280, 720), &inventory, 0);
            if let Some(canvas) = next
                && ui.armor_models.get(&helmet, 0).is_some()
            {
                break canvas;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "armor model did not finish loading"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        let origin = (after.width / 2 - 88, after.height / 2 - 83);
        let changes = (origin.1 + 8..origin.1 + 78)
            .flat_map(|y| {
                (origin.0 + 26..origin.0 + 75).map(move |x| (y * after.width + x) as usize)
            })
            .filter(|&index| before.pixels[index] != after.pixels[index])
            .count();
        assert!(changes > 20, "loaded armor must alter the player preview");
        inventory.set_slot(PlayerSlot::Armor(0), None);
        let removed = ui.compose((1280, 720), &inventory, 0).unwrap();
        for y in origin.1 + 8..origin.1 + 78 {
            for x in origin.0 + 26..origin.0 + 75 {
                let index = (y * removed.width + x) as usize;
                assert_eq!(
                    removed.pixels[index], before.pixels[index],
                    "armor removal at {x},{y}"
                );
            }
        }
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn generated_item_icons_load_asynchronously_in_hotbar_and_inventory() {
        use native_persistence::nbt::Tag;
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&directory, &[]).unwrap());
        let mut ui = GameUi::new(assets);
        let mut inventory = PlayerInventory::default();
        let sword = ItemStack::new(
            "minecraft:diamond_sword".into(),
            1,
            1,
            Tag::Compound(vec![("minecraft:damage".into(), Tag::Int(800))]),
        )
        .unwrap();
        let (bar_width, bar_color) = sword.durability_bar().unwrap();
        inventory.set_slot(PlayerSlot::Storage(0), Some(sword.clone()));
        let before = ui.compose((1280, 720), &inventory, 0).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let after = loop {
            if let Some(canvas) = ui.compose((1280, 720), &inventory, 0)
                && ui.held_models.get_item(&sword).is_some()
            {
                break canvas;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "generated item model did not load"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        let origin = (after.width / 2 - 91 + 3, after.height - 19);
        let changed = (origin.1..origin.1 + 16)
            .flat_map(|y| (origin.0..origin.0 + 16).map(move |x| (y * after.width + x) as usize))
            .filter(|&i| before.pixels[i] != after.pixels[i])
            .count();
        assert!(
            changed > 32,
            "the hotbar should replace the missing icon with the sword texture"
        );
        inventory.toggle();
        let inventory_canvas = ui.compose((1280, 720), &inventory, 0).unwrap();
        let (x, y) = (
            inventory_canvas.width / 2 - 88 + 8,
            inventory_canvas.height / 2 - 83 + 142,
        );
        let cyan = (y..y + 16)
            .flat_map(|py| (x..x + 16).map(move |px| (py * inventory_canvas.width + px) as usize))
            .filter(|&i| {
                let p = inventory_canvas.pixels[i];
                ((p >> 8) & 255) > ((p >> 16) & 255) + 20 && (p & 255) > ((p >> 16) & 255) + 20
            })
            .count();
        assert!(cyan > 5, "inventory should display the sword texture");
        let pixel = inventory_canvas.pixels[((y + 13) * inventory_canvas.width + x + 2) as usize];
        assert_eq!(
            pixel & 0x00ff_ffff,
            ((bar_color[0] as u32) << 16) | ((bar_color[1] as u32) << 8) | bar_color[2] as u32
        );
        assert!(bar_width > 0 && bar_width < 13);
        let black = inventory_canvas.pixels[((y + 14) * inventory_canvas.width + x + 2) as usize];
        assert_eq!(black, 0xff00_0000);
        if let Some(path) = std::env::var_os("NATIVE_GENERATED_UI_PREVIEW") {
            let image = RgbaImage::from_fn(
                inventory_canvas.width as u32,
                inventory_canvas.height as u32,
                |x, y| {
                    let pixel =
                        inventory_canvas.pixels[(y * inventory_canvas.width as u32 + x) as usize];
                    image::Rgba([
                        (pixel >> 16) as u8,
                        (pixel >> 8) as u8,
                        pixel as u8,
                        (pixel >> 24) as u8,
                    ])
                },
            );
            image.save(path).unwrap();
        }
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn compass_icons_load_into_hotbar_and_inventory() {
        use native_persistence::nbt::Tag;
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut ui = GameUi::new(Arc::new(MenuAssets::load(&directory, &[]).unwrap()));
        let mut inventory = PlayerInventory::default();
        let compass =
            ItemStack::new("minecraft:compass".into(), 1, 64, Tag::Compound(vec![])).unwrap();
        inventory.set_slot(PlayerSlot::Storage(0), Some(compass.clone()));
        let before = ui.compose((1280, 720), &inventory, 0).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let after = loop {
            if let Some(canvas) = ui.compose((1280, 720), &inventory, 0)
                && ui.held_models.get_item(&compass).is_some()
            {
                break canvas;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "compass model did not load"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        let origin = (after.width / 2 - 91 + 3, after.height - 19);
        let changed = (origin.1..origin.1 + 16)
            .flat_map(|y| (origin.0..origin.0 + 16).map(move |x| (y * after.width + x) as usize))
            .filter(|&i| before.pixels[i] != after.pixels[i])
            .count();
        assert!(changed > 32, "hotbar should display the loaded compass");
        inventory.toggle();
        let canvas = ui.compose((1280, 720), &inventory, 0).unwrap();
        let (x, y) = (canvas.width / 2 - 88 + 8, canvas.height / 2 - 83 + 142);
        let icon = ui
            .held_models
            .get_item(&compass)
            .unwrap()
            .gui_icon
            .as_ref()
            .unwrap()
            .clone();
        let visible = (0..16)
            .flat_map(|dy| (0..16).map(move |dx| (dx, dy)))
            .filter(|&(dx, dy)| {
                let pixel = icon.get_pixel(dx, dy).0;
                if pixel[3] != 255 {
                    return false;
                }
                let value =
                    canvas.pixels[((y + dy as i32) * canvas.width + x + dx as i32) as usize];
                value & 0x00ff_ffff
                    == (((pixel[0] as u32) << 16) | ((pixel[1] as u32) << 8) | pixel[2] as u32)
            })
            .count();
        assert!(
            visible > 32,
            "inventory should display the loaded compass pixels"
        );
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn linked_compass_models_change_with_player_position() {
        use native_persistence::nbt::Tag;
        use native_resources::packs::ResourceId;
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut ui = GameUi::new(Arc::new(MenuAssets::load(&directory, &[]).unwrap()));
        let tracker = native_session::lodestone::Tracker::linked(
            ResourceId::parse("minecraft:overworld").unwrap(),
            glam::IVec3::new(100, 64, 0),
        );
        let compass = ItemStack::new(
            "minecraft:compass".into(),
            1,
            64,
            Tag::Compound(vec![(
                "minecraft:lodestone_tracker".into(),
                tracker.to_nbt(),
            )]),
        )
        .unwrap();
        let mut inventory = PlayerInventory::default();
        inventory.set_slot(PlayerSlot::Storage(0), Some(compass.clone()));
        let context = |x| crate::held_items::CompassContext {
            dimension: "minecraft:overworld".into(),
            world_spawn: None,
            last_death: native_session::LastDeathLocation::Unknown,
            owner: glam::DVec3::new(x, 64.0, 0.5),
            yaw_degrees: -180.0,
            tick: 10,
        };
        let load = |ui: &mut GameUi, x| {
            ui.held_models.set_compass_context(context(x));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            loop {
                ui.held_models.poll();
                if let Some(model) = ui.held_models.get_item(&compass) {
                    break model;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "linked compass did not load"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        // The first request learns the pack definition asynchronously. Resolve
        // again after it arrives so the actual position selects the model.
        load(&mut ui, 0.5);
        let east = load(&mut ui, 0.5);
        let west = load(&mut ui, 200.5);
        assert_ne!(
            east.gui_icon.as_ref().unwrap().as_raw(),
            west.gui_icon.as_ref().unwrap().as_raw()
        );
        ui.last_draw = None;
        let canvas = ui.compose((1280, 720), &inventory, 10).unwrap();
        let icon = west.gui_icon.as_ref().unwrap();
        let (x, y) = (canvas.width / 2 - 91 + 3, canvas.height - 19);
        let visible = (0..16)
            .flat_map(|dy| (0..16).map(move |dx| (dx, dy)))
            .filter(|&(dx, dy)| {
                let pixel = icon.get_pixel(dx, dy).0;
                if pixel[3] != 255 {
                    return false;
                }
                let value =
                    canvas.pixels[((y + dy as i32) * canvas.width + x + dx as i32) as usize];
                value & 0x00ff_ffff
                    == (((pixel[0] as u32) << 16) | ((pixel[1] as u32) << 8) | pixel[2] as u32)
            })
            .count();
        assert!(
            visible > 32,
            "hotbar should show the model for the updated position"
        );
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn ordinary_compass_models_follow_shared_world_spawn() {
        use native_persistence::{nbt::Tag, world::RespawnData};
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut ui = GameUi::new(Arc::new(MenuAssets::load(&directory, &[]).unwrap()));
        let compass =
            ItemStack::new("minecraft:compass".into(), 1, 64, Tag::Compound(vec![])).unwrap();
        let mut spawn = RespawnData::default();
        spawn.position = [100, 64, 0];
        let context = |x| crate::held_items::CompassContext {
            dimension: "minecraft:overworld".into(),
            world_spawn: Some(Arc::new(spawn.clone())),
            last_death: native_session::LastDeathLocation::Unknown,
            owner: glam::DVec3::new(x, 64.0, 0.5),
            yaw_degrees: -180.0,
            tick: 10,
        };
        let load = |ui: &mut GameUi, x| {
            ui.held_models.set_compass_context(context(x));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            loop {
                ui.held_models.poll();
                if let Some(model) = ui.held_models.get_item(&compass) {
                    break model;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "spawn compass did not load"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        load(&mut ui, 0.5);
        let east = load(&mut ui, 0.5);
        let west = load(&mut ui, 200.5);
        assert_ne!(
            east.gui_icon.as_ref().unwrap().as_raw(),
            west.gui_icon.as_ref().unwrap().as_raw()
        );
        let mut remote = context(0.5);
        remote.dimension = "minecraft:the_nether".into();
        remote.tick = 11;
        ui.held_models.set_compass_context(remote);
        let mut load_seed = |seed| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            loop {
                ui.held_models.poll();
                if let Some(model) = ui.held_models.get_item_seeded(&compass, seed) {
                    break model;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "random compass did not load"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        let slot_one = load_seed(1);
        let slot_two = load_seed(2);
        assert_ne!(
            slot_one.gui_icon.as_ref().unwrap().as_raw(),
            slot_two.gui_icon.as_ref().unwrap().as_raw()
        );
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn recovery_compass_models_follow_last_death() {
        use native_persistence::nbt::Tag;
        use native_resources::packs::ResourceId;
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut ui = GameUi::new(Arc::new(MenuAssets::load(&directory, &[]).unwrap()));
        let compass = ItemStack::new(
            "minecraft:recovery_compass".into(),
            1,
            64,
            Tag::Compound(vec![]),
        )
        .unwrap();
        let death = native_session::lodestone::Target {
            dimension: ResourceId::parse("minecraft:overworld").unwrap(),
            position: glam::IVec3::new(100, 64, 0),
        };
        let context = |x| crate::held_items::CompassContext {
            dimension: "minecraft:overworld".into(),
            world_spawn: None,
            last_death: native_session::LastDeathLocation::At(death.clone()),
            owner: glam::DVec3::new(x, 64.0, 0.5),
            yaw_degrees: -180.0,
            tick: 10,
        };
        let load = |ui: &mut GameUi, x| {
            ui.held_models.set_compass_context(context(x));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            loop {
                ui.held_models.poll();
                if let Some(model) = ui.held_models.get_item(&compass) {
                    break model;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "recovery compass did not load"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        load(&mut ui, 0.5);
        let east = load(&mut ui, 0.5);
        let west = load(&mut ui, 200.5);
        assert_ne!(
            east.gui_icon.as_ref().unwrap().as_raw(),
            west.gui_icon.as_ref().unwrap().as_raw()
        );
        let mut absent = context(0.5);
        absent.last_death = native_session::LastDeathLocation::Absent;
        absent.tick = 11;
        ui.held_models.set_compass_context(absent);
        let mut load_seed = |seed| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            loop {
                ui.held_models.poll();
                if let Some(model) = ui.held_models.get_item_seeded(&compass, seed) {
                    break model;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "random recovery compass did not load"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        let one = load_seed(1);
        let two = load_seed(2);
        assert_ne!(
            one.gui_icon.as_ref().unwrap().as_raw(),
            two.gui_icon.as_ref().unwrap().as_raw()
        );
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn dropped_compass_uses_item_position_and_recovery_has_no_player_target() {
        use native_persistence::nbt::Tag;
        use native_resources::packs::ResourceId;
        use native_session::items::DroppedItem;
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut ui = GameUi::new(Arc::new(MenuAssets::load(&directory, &[]).unwrap()));
        let target = native_session::lodestone::Target {
            dimension: ResourceId::parse("minecraft:overworld").unwrap(),
            position: glam::IVec3::new(100, 64, 0),
        };
        let mut spawn = native_persistence::world::RespawnData::default();
        spawn.position = target.position.to_array();
        ui.held_models
            .set_compass_context(crate::held_items::CompassContext {
                dimension: "minecraft:overworld".into(),
                world_spawn: Some(Arc::new(spawn)),
                last_death: native_session::LastDeathLocation::At(target),
                owner: glam::DVec3::new(0.5, 64.0, 0.5),
                yaw_degrees: 0.0,
                tick: 11,
            });
        let item = |id: &str, x, entity_id| DroppedItem {
            entity_id,
            item_age: 42,
            position: glam::DVec3::new(x, 64.0, 0.5),
            previous_position: glam::DVec3::new(x, 64.0, 0.5),
            stack: ItemStack::new(id.into(), 1, 64, Tag::Compound(vec![])).unwrap(),
            age: 2,
            bob_offset: 1.25,
        };
        let load = |ui: &mut GameUi, item: &DroppedItem| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            loop {
                ui.held_models.poll();
                if let Some(model) = ui.held_models.get_dropped_item(item) {
                    break model;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "dropped compass did not load"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        let east = item("minecraft:compass", 0.5, 1);
        let west = item("minecraft:compass", 200.5, 2);
        load(&mut ui, &east);
        let east_model = load(&mut ui, &east);
        let west_model = load(&mut ui, &west);
        assert_ne!(
            east_model.gui_icon.as_ref().unwrap().as_raw(),
            west_model.gui_icon.as_ref().unwrap().as_raw()
        );
        let first = item("minecraft:recovery_compass", 0.5, 1);
        let second = item("minecraft:recovery_compass", 0.5, 2);
        load(&mut ui, &first);
        let one = load(&mut ui, &first);
        let two = load(&mut ui, &second);
        assert_ne!(
            one.gui_icon.as_ref().unwrap().as_raw(),
            two.gui_icon.as_ref().unwrap().as_raw()
        );
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn enchanted_hotbar_and_inventory_use_cached_glint_masks() {
        use native_persistence::nbt::Tag;
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut ui = GameUi::new(Arc::new(MenuAssets::load(&directory, &[]).unwrap()));
        let mut inventory = PlayerInventory::default();
        let sword = ItemStack::new(
            "minecraft:diamond_sword".into(),
            1,
            1,
            Tag::Compound(vec![
                ("minecraft:damage".into(), Tag::Int(800)),
                (
                    "minecraft:enchantments".into(),
                    Tag::Compound(vec![("minecraft:mending".into(), Tag::Int(1))]),
                ),
            ]),
        )
        .unwrap();
        inventory.set_slot(PlayerSlot::Storage(0), Some(sword.clone()));
        ui.compose((1280, 720), &inventory, 0);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let loaded = loop {
            if let Some(canvas) = ui.compose((1280, 720), &inventory, 0)
                && ui.held_models.get_item(&sword).is_some()
            {
                break canvas;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "enchanted item model did not load"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        let x = loaded.width / 2 - 91 + 3;
        let y = loaded.height - 19;
        let mask = loaded.glint.as_ref().expect("hotbar glint mask");
        let uv = ui
            .held_models
            .get_item(&sword)
            .unwrap()
            .gui_glint_uv
            .expect("single-layer item must carry stitched atlas coordinates");
        assert!((uv[0] - 419.0 / 1024.0).abs() < f32::EPSILON);
        assert!((uv[1] - 163.0 / 512.0).abs() < f32::EPSILON);
        let count = (y..y + 16)
            .flat_map(|py| (x..x + 16).map(move |px| (py * loaded.width + px) as usize))
            .filter(|&index| mask[index * 4 + 3] > 0)
            .count();
        assert!(count > 5);
        assert!((y..y + 16).any(|py| (x..x + 16).any(|px| {
            let i = (py * loaded.width + px) as usize * 4;
            mask[i + 3] > 0 && mask[i + 2] == 1
        })));
        assert_eq!(
            mask[((y + 13) * loaded.width + x + 2) as usize * 4 + 3],
            0,
            "durability bar covers glint"
        );
        assert!(
            ui.compose((1280, 720), &inventory, 1).is_none(),
            "glint animation must not invalidate the static HUD"
        );
        inventory.toggle();
        let inventory_canvas = ui.compose((1280, 720), &inventory, 1).unwrap();
        let (x, y) = (
            inventory_canvas.width / 2 - 88 + 8,
            inventory_canvas.height / 2 - 83 + 142,
        );
        let mask = inventory_canvas
            .glint
            .as_ref()
            .expect("inventory glint mask");
        assert!((y..y + 16).any(|py| {
            (x..x + 16).any(|px| mask[(py * inventory_canvas.width + px) as usize * 4 + 3] > 0)
        }));
        assert_eq!(
            mask[((y + 13) * inventory_canvas.width + x + 2) as usize * 4 + 3],
            0
        );
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn layered_and_cuboid_glint_coordinates_reach_the_hotbar() {
        use native_persistence::nbt::Tag;
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut ui = GameUi::new(Arc::new(MenuAssets::load(&directory, &[]).unwrap()));
        let mut inventory = PlayerInventory::default();
        let stone = ItemStack::new(
            "minecraft:stone".into(),
            1,
            1,
            Tag::Compound(vec![(
                "minecraft:enchantment_glint_override".into(),
                Tag::Byte(1),
            )]),
        )
        .unwrap();
        let helmet = ItemStack::new(
            "minecraft:iron_helmet".into(),
            1,
            1,
            Tag::Compound(vec![
                ("minecraft:enchantment_glint_override".into(), Tag::Byte(1)),
                (
                    "minecraft:trim".into(),
                    Tag::Compound(vec![(
                        "material".into(),
                        Tag::String("minecraft:quartz".into()),
                    )]),
                ),
            ]),
        )
        .unwrap();
        inventory.set_slot(PlayerSlot::Storage(0), Some(stone.clone()));
        inventory.set_slot(PlayerSlot::Storage(1), Some(helmet.clone()));
        ui.compose((1280, 720), &inventory, 0);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let canvas = loop {
            if let Some(canvas) = ui.compose((1280, 720), &inventory, 0)
                && ui.held_models.get_item(&stone).is_some()
                && ui.held_models.get_item(&helmet).is_some()
            {
                break canvas;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "glint models did not load"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        let mask = canvas.glint.as_ref().unwrap();
        for (slot, item) in [&stone, &helmet].into_iter().enumerate() {
            let model = ui.held_models.get_item(item).unwrap();
            let pixels = model
                .gui_glint_pixels
                .as_ref()
                .expect("per-pixel glint mapping");
            let x0 = canvas.width / 2 - 91 + 3 + slot as i32 * 20;
            let y0 = canvas.height - 19;
            let mut compared = 0;
            for y in 0..16 {
                for x in 0..16 {
                    let source = pixels[(y * 16 + x) as usize];
                    let index = ((y0 + y) * canvas.width + x0 + x) as usize * 4;
                    if source[2] == 1 && mask[index + 3] > 0 {
                        assert_eq!(&mask[index..index + 3], &source);
                        compared += 1;
                    }
                }
            }
            assert!(
                compared > 10,
                "slot {slot} has too few mapped pixels: {compared}"
            );
        }
    }

    #[test]
    #[ignore = "performance measurement using locally installed Minecraft 26.3 assets"]
    fn measure_inventory_glint_mask_composition_cost() {
        use native_persistence::nbt::Tag;
        use std::{
            hint::black_box,
            time::{Duration, Instant},
        };
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut ui = GameUi::new(Arc::new(MenuAssets::load(&directory, &[]).unwrap()));
        let make_inventory = |enchanted: bool| {
            let mut inventory = PlayerInventory::default();
            let mut components = vec![("minecraft:damage".into(), Tag::Int(800))];
            if enchanted {
                components.push((
                    "minecraft:enchantments".into(),
                    Tag::Compound(vec![("minecraft:mending".into(), Tag::Int(1))]),
                ));
            }
            let item = ItemStack::new(
                "minecraft:diamond_sword".into(),
                1,
                1,
                Tag::Compound(components),
            )
            .unwrap();
            for slot in 0..36 {
                inventory.set_slot(PlayerSlot::Storage(slot), Some(item.clone()));
            }
            inventory.toggle();
            inventory
        };
        let plain = make_inventory(false);
        let enchanted = make_inventory(true);
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            black_box(ui.compose((1920, 1080), &plain, 0));
            black_box(ui.compose((1920, 1080), &enchanted, 0));
            if [false, true].into_iter().all(|glint| {
                let inventory = if glint { &enchanted } else { &plain };
                inventory
                    .slot(PlayerSlot::Storage(0))
                    .and_then(|item| ui.held_models.get_item(item))
                    .is_some_and(|model| model.gui_icon.is_some())
            }) {
                break;
            }
            assert!(Instant::now() < deadline, "item model did not load");
            std::thread::sleep(Duration::from_millis(10));
        }
        for (name, inventory) in [("ordinary", &plain), ("enchanted", &enchanted)] {
            let mut samples = Vec::new();
            for tick in 1..121 {
                ui.cursor_moved(
                    f64::from(600 + (tick as u32 * 7) % 700),
                    f64::from(200 + (tick as u32 * 11) % 500),
                );
                let started = Instant::now();
                black_box(ui.compose((1920, 1080), inventory, tick));
                samples.push(started.elapsed());
            }
            samples.sort_unstable();
            println!(
                "1080p {name} inventory CPU composition: median {:?}, p95 {:?}; excludes GPU upload and presentation",
                samples[60], samples[114]
            );
        }
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn ordinary_block_icons_load_asynchronously_and_honor_resource_packs() {
        use native_persistence::nbt::Tag;
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("assets/minecraft");
        std::fs::create_dir_all(base.join("items")).unwrap();
        std::fs::create_dir_all(base.join("models/block")).unwrap();
        std::fs::create_dir_all(base.join("textures/block")).unwrap();
        std::fs::write(
            base.join("items/oak_planks.json"),
            r#"{"model":{"type":"minecraft:model","model":"minecraft:block/green_icon"}}"#,
        )
        .unwrap();
        std::fs::write(base.join("models/block/green_icon.json"), r#"{"parent":"minecraft:block/cube_all","textures":{"all":"minecraft:block/green_icon"},"display":{"gui":{"rotation":[0,0,0],"scale":[0.5,0.5,0.5]}}}"#).unwrap();
        RgbaImage::from_pixel(16, 16, image::Rgba([0, 255, 0, 255]))
            .save(base.join("textures/block/green_icon.png"))
            .unwrap();
        let assets = Arc::new(MenuAssets::load(&directory, &[temp.path().to_path_buf()]).unwrap());
        let mut ui = GameUi::new(assets);
        let mut inventory = PlayerInventory::default();
        let ids = [
            "minecraft:oak_log",
            "minecraft:oak_planks",
            "minecraft:crafting_table",
            "minecraft:oak_stairs",
            "minecraft:stone",
            "minecraft:grass_block",
        ];
        for (index, id) in ids.iter().enumerate() {
            inventory.set_slot(
                PlayerSlot::Storage(index as u8),
                Some(ItemStack::unresolved((*id).into(), 1, Tag::Compound(vec![])).unwrap()),
            );
        }
        inventory.toggle();
        let mut canvas = ui.compose((1280, 720), &inventory, 0).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let ready = ids.iter().all(|id| {
                ui.held_models
                    .get(id)
                    .is_some_and(|model| model.gui_icon.is_some())
            });
            if let Some(updated) = ui.compose((1280, 720), &inventory, 0) {
                canvas = updated;
            }
            if ready {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "block item models did not load"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let (left, top) = (canvas.width / 2 - 88, canvas.height / 2 - 83);
        for (index, id) in ids.iter().enumerate() {
            let x = left + 8 + index as i32 * 18;
            let y = top + 142;
            let pixels = (y..y + 16)
                .flat_map(|py| (x..x + 16).map(move |px| (py * canvas.width + px) as usize))
                .collect::<Vec<_>>();
            let magenta = pixels
                .iter()
                .filter(|&&i| canvas.pixels[i] & 0x00ff_ffff == 0x00ff_00ff)
                .count();
            assert_eq!(magenta, 0, "{id} retained the missing-model checker");
        }
        let overridden = ui.held_models.get("minecraft:oak_planks").unwrap();
        let icon = overridden.gui_icon.as_ref().unwrap();
        let center = icon.get_pixel(8, 8).0;
        assert!(
            center[1] > 80 && center[0] == 0 && center[2] == 0,
            "override not used: {center:?}"
        );
        if let Some(path) = std::env::var_os("NATIVE_BLOCK_ICON_PREVIEW") {
            let image = RgbaImage::from_fn(canvas.width as u32, canvas.height as u32, |x, y| {
                let p = canvas.pixels[(y * canvas.width as u32 + x) as usize];
                image::Rgba([(p >> 16) as u8, (p >> 8) as u8, p as u8, (p >> 24) as u8])
            });
            image.save(path).unwrap();
        }
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn offhand_hud_follows_main_hand_setting() {
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        for main_left in [false, true] {
            let mut assets = MenuAssets::load(&directory, &[]).unwrap();
            assets.main_hand_left = main_left;
            let mut ui = GameUi::new(Arc::new(assets));
            let mut inventory = PlayerInventory::demo();
            let before = ui.compose((1280, 720), &inventory, 0).unwrap();
            let item = inventory.slot(PlayerSlot::Storage(0)).cloned();
            assert!(inventory.set_slot(PlayerSlot::Offhand, item));
            let after = ui.compose((1280, 720), &inventory, 0).unwrap();
            let (sprite, _) = offhand_hotbar_layout(after.width, after.height, main_left);
            let changed = (sprite.y..after.height)
                .flat_map(|y| {
                    (sprite.x..sprite.x + sprite.w).map(move |x| (y * after.width + x) as usize)
                })
                .filter(|&index| before.pixels[index] != after.pixels[index])
                .count();
            assert!(
                changed > 200,
                "offhand sprite was not drawn on the expected side"
            );
            let (opposite, _) = offhand_hotbar_layout(after.width, after.height, !main_left);
            let changed_opposite = (opposite.y..after.height)
                .flat_map(|y| {
                    (opposite.x..opposite.x + opposite.w)
                        .map(move |x| (y * after.width + x) as usize)
                })
                .filter(|&index| before.pixels[index] != after.pixels[index])
                .count();
            assert_eq!(changed_opposite, 0, "offhand sprite appeared on both sides");
        }
    }

    #[test]
    #[ignore = "performance measurement using installed Minecraft 26.3 assets"]
    fn measure_inventory_avatar_composition_cost() {
        use std::{hint::black_box, time::Instant};
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&directory, &[]).unwrap());
        let mut ui = GameUi::new(assets);
        let mut inventory = PlayerInventory::demo();
        inventory.toggle();
        assert!(ui.compose((1920, 1080), &inventory, 0).is_some());
        let deadline = Instant::now() + std::time::Duration::from_secs(30);
        while ui.compose((1920, 1080), &inventory, 0).is_none() {
            assert!(
                Instant::now() < deadline,
                "held item model did not finish loading"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(ui.compose((1920, 1080), &inventory, 0).is_none());
        let mut times = Vec::new();
        for tick in 1..121 {
            ui.cursor_moved(
                f64::from(600 + (tick as u32 * 7) % 700),
                f64::from(200 + (tick as u32 * 11) % 500),
            );
            let started = Instant::now();
            black_box(ui.compose((1920, 1080), &inventory, tick));
            times.push(started.elapsed());
        }
        times.sort_unstable();
        println!(
            "1080p inventory CPU composition with avatar: median {:?}, p95 {:?}; excludes GPU upload and presentation",
            times[times.len() / 2],
            times[times.len() * 95 / 100]
        );
    }

    #[test]
    #[ignore = "performance measurement using installed Minecraft 26.3 assets"]
    fn measure_equipped_avatar_composition_cost() {
        use native_persistence::nbt::Tag;
        use std::{
            hint::black_box,
            time::{Duration, Instant},
        };
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&directory, &[]).unwrap());
        let mut ui = GameUi::new(assets);
        let mut inventory = PlayerInventory::default();
        inventory.toggle();
        for (index, piece) in ["helmet", "chestplate", "leggings", "boots"]
            .into_iter()
            .enumerate()
        {
            inventory.set_slot(
                PlayerSlot::Armor(index as u8),
                ItemStack::new(
                    format!("minecraft:iron_{piece}"),
                    1,
                    1,
                    Tag::Compound(vec![]),
                ),
            );
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            black_box(ui.compose((1920, 1080), &inventory, 0));
            if (0..4).all(|index| {
                ui.armor_models
                    .get(
                        inventory.slot(PlayerSlot::Armor(index)).unwrap(),
                        index as usize,
                    )
                    .is_some()
            }) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "armor models did not finish loading"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut times = Vec::new();
        for tick in 1..121 {
            ui.cursor_moved(
                f64::from(600 + (tick as u32 * 7) % 700),
                f64::from(200 + (tick as u32 * 11) % 500),
            );
            let start = Instant::now();
            black_box(ui.compose((1920, 1080), &inventory, tick));
            times.push(start.elapsed());
        }
        times.sort_unstable();
        println!(
            "1080p inventory CPU composition with full armor: median {:?}, p95 {:?}; excludes GPU upload and presentation",
            times[times.len() / 2],
            times[times.len() * 95 / 100]
        );
    }

    #[test]
    #[ignore = "performance measurement using locally installed Minecraft 26.3 assets"]
    fn measure_cached_block_inventory_composition_cost() {
        use native_persistence::nbt::Tag;
        use std::{hint::black_box, time::Instant};
        let directory = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let assets = Arc::new(MenuAssets::load(&directory, &[]).unwrap());
        let mut ui = GameUi::new(assets);
        let mut inventory = PlayerInventory::default();
        let mut ids = Vec::new();
        for wood in [
            "oak", "spruce", "birch", "jungle", "acacia", "dark_oak", "mangrove", "cherry",
            "pale_oak",
        ] {
            ids.extend([
                format!("minecraft:{wood}_log"),
                format!("minecraft:{wood}_wood"),
                format!("minecraft:{wood}_planks"),
                format!("minecraft:stripped_{wood}_log"),
            ]);
        }
        for (slot, id) in ids.iter().enumerate() {
            inventory.set_slot(
                PlayerSlot::Storage(slot as u8),
                Some(ItemStack::unresolved(id.clone(), 1, Tag::Compound(vec![])).unwrap()),
            );
        }
        inventory.toggle();
        let deadline = Instant::now() + std::time::Duration::from_secs(60);
        loop {
            let _ = ui.compose((1920, 1080), &inventory, 0);
            if ids.iter().all(|id| {
                ui.held_models
                    .get(id)
                    .is_some_and(|model| model.gui_icon.is_some())
            }) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "block icon loading did not finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let mut times = Vec::new();
        for tick in 1..121 {
            ui.cursor_moved(
                f64::from(600 + (tick as u32 * 7) % 700),
                f64::from(200 + (tick as u32 * 11) % 500),
            );
            let start = Instant::now();
            black_box(ui.compose((1920, 1080), &inventory, tick));
            times.push(start.elapsed());
        }
        times.sort_unstable();
        println!(
            "1080p inventory CPU composition with 36 cached block icons: median {:?}, p95 {:?}; excludes loading, GPU upload and presentation",
            times[times.len() / 2],
            times[times.len() * 95 / 100]
        );
    }
}
