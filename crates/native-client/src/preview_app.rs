//! Viewer and experimental unsaved play for a bounded area of an existing world.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use glam::Vec3;
use native_resources::packs::ResourceStack;
use native_session::{
    GameSession, PlayerCommand, WorldSnapshot,
    inventory::{ClickButton, InventoryPointerEvent},
    loaded::LoadedSession,
};
use winit::{
    application::ApplicationHandler,
    event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    platform::run_on_demand::EventLoopExtRunOnDemand,
    window::{CursorGrabMode, Window, WindowId},
};

use crate::{
    chunk_stream::ChunkStreamer,
    frame_pacer::FramePacer,
    game_ui::{GameUi, PauseAction, hotbar_key, inventory_swap_key},
    menu_assets::MenuAssets,
    menu_settings::VideoSettings,
    mesh_worker::{MeshJob, MeshWorker},
    preview::PreviewScene,
    recipe_book_ui::BookAction,
    renderer::Renderer,
};

pub(crate) fn atmospheric_fog_color(
    fog: [f32; 3],
    sky: [f32; 3],
    sky_end: f32,
    chunks: u32,
) -> [f32; 3] {
    let distance = (sky_end / 16.0).min(chunks as f32);
    let fraction = (distance / 32.0).clamp(0.0, 1.0);
    let value = 0.25 + fraction * 0.75;
    let amount = 1.0 - f64::from(value).powf(0.25) as f32;
    std::array::from_fn(|i| fog[i] + amount * (sky[i] - fog[i]))
}

pub(crate) fn adjusted_fog_color(mut color: [f32; 3], darkness: f32, vision: f32) -> [f32; 3] {
    if darkness > 0.0 {
        let brightness = 1.0 - darkness;
        let scale = brightness * brightness;
        color = color.map(|component| component * scale);
    }
    let color = if color.iter().all(|component| *component != 0.0) {
        let scale = 1.0 / color[0].max(color[1]).max(color[2]);
        color.map(|component| component + vision * (component * scale - component))
    } else {
        color
    };
    color
}

/// Water fog uses the player's adaptation even without a status effect.
pub(crate) fn water_fog(
    color: [f32; 3],
    range: [f32; 2],
    vision: f32,
    darkness: f32,
) -> ([f32; 3], [f32; 2]) {
    (
        adjusted_fog_color(color, darkness, vision),
        [range[0], range[1] * vision.max(0.25)],
    )
}

pub(crate) fn weather_darken_sky(mut color: [f32; 3], rain: f32, thunder: f32) -> [f32; 3] {
    if rain > 0.0 {
        let red_green = 1.0 - rain * 0.5;
        let blue = 1.0 - rain * 0.4;
        color = [
            (color[0] * red_green).clamp(0.0, 1.0),
            (color[1] * red_green).clamp(0.0, 1.0),
            (color[2] * blue).clamp(0.0, 1.0),
        ];
    }
    if thunder > 0.0 {
        let scale = 1.0 - thunder * 0.5;
        color = color.map(|component| (component * scale).clamp(0.0, 1.0));
    }
    color
}

pub(crate) fn sunrise_fog_color(
    fog: [f32; 3],
    sun_angle: f32,
    color: [f32; 4],
    forward_x: f32,
    chunks: u32,
) -> [f32; 3] {
    if chunks < 4 || color[3] <= 0.0 {
        return fog;
    }
    let radians = sun_angle * 0.017453292_f32;
    let sign = if native_session::physics::java_sin(f64::from(radians), false) > 0.0 {
        -1.0
    } else {
        1.0
    };
    let direction = forward_x * sign;
    if direction <= 0.0 {
        return fog;
    }
    let amount = direction * color[3];
    std::array::from_fn(|i| fog[i] + amount * (color[i] - fog[i]))
}

fn interpolate_sun_angle(previous: f32, current: f32, partial: f32) -> f32 {
    let mut delta = (current - previous) % 360.0;
    if delta >= 180.0 {
        delta -= 360.0;
    }
    if delta < -180.0 {
        delta += 360.0;
    }
    if delta.abs() >= 90.0 {
        current
    } else {
        previous + partial * delta
    }
}

pub struct Camera {
    pub eye: Vec3,
    pub yaw: f32,
    pub pitch: f32,
}

impl Camera {
    fn direction(&self) -> Vec3 {
        let horizontal = self.pitch.cos();
        Vec3::new(
            self.yaw.sin() * horizontal,
            self.pitch.sin(),
            -self.yaw.cos() * horizontal,
        )
    }

    fn advance(&mut self, input: &Input, elapsed: Duration) {
        let forward = Vec3::new(self.yaw.sin(), 0.0, -self.yaw.cos());
        let right = Vec3::new(self.yaw.cos(), 0.0, self.yaw.sin());
        let axis = |positive: bool, negative: bool| {
            i32::from(positive) as f32 - i32::from(negative) as f32
        };
        let movement = forward * axis(input.forward, input.back)
            + right * axis(input.right, input.left)
            + Vec3::Y * axis(input.up, input.down());
        let speed = if input.fast() { 40.0 } else { 10.0 };
        self.eye += movement.clamp_length_max(1.0) * speed * elapsed.as_secs_f32().min(0.1);
    }
}

#[derive(Default)]
struct Input {
    forward: bool,
    back: bool,
    left: bool,
    right: bool,
    up: bool,
    down: crate::toggle_key::ToggleKey,
    fast: crate::toggle_key::ToggleKey,
}

impl Input {
    fn down(&self) -> bool {
        self.down.down()
    }
    fn fast(&self) -> bool {
        self.fast.down()
    }

    fn release_for_screen(&mut self, settings: VideoSettings) {
        self.forward = false;
        self.back = false;
        self.left = false;
        self.right = false;
        self.up = false;
        self.down.release_for_screen(settings.toggle_sneak);
        self.fast.release_for_screen(settings.toggle_sprint);
    }

    fn restore_after_screen(&mut self, settings: VideoSettings) {
        self.down.restore_after_screen(settings.toggle_sneak);
        self.fast.restore_after_screen(settings.toggle_sprint);
    }
}

#[cfg(debug_assertions)]
pub fn run(
    event_loop: &mut EventLoop<()>,
    scene: PreviewScene,
    camera: Camera,
    smoke_test: bool,
    video: VideoSettings,
) -> Result<crate::GameExit> {
    run_inner(
        event_loop, scene, camera, smoke_test, None, None, None, video, None, false, None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_play(
    event_loop: &mut EventLoop<()>,
    scene: PreviewScene,
    session: LoadedSession,
    mut resources: ResourceStack,
    streamer: ChunkStreamer,
    smoke_test: bool,
    assets: Option<Arc<MenuAssets>>,
    skin: Option<Arc<crate::player_skin::PlayerSkin>>,
    trims: Arc<crate::equipment_visual::TrimRegistry>,
    enchantments: Arc<crate::enchantment_tooltip::Registry>,
    sky_timeline: Option<crate::preview_lighting::SkyTimeline>,
    sky_background: Option<crate::sky_background::SkyBackground>,
    end_sky: Option<crate::end_sky::Texture>,
    sun: Option<crate::sun_render::Texture>,
    video: VideoSettings,
    audio: Option<crate::audio::UiAudio>,
    hud_hidden: bool,
    menu_screenshots: Option<std::rc::Rc<crate::screenshot::MenuScreenshots>>,
) -> Result<crate::GameExit> {
    let mut session = session;
    session.set_fov_effect_scale(video.fov_effect_scale.value() as f32);
    session.set_sprint_window(video.sprint_window);
    let snapshot = session.snapshot();
    let camera = Camera {
        eye: eye_for_scene(&snapshot, scene.origin, session.partial_tick()),
        yaw: snapshot.player.yaw,
        pitch: snapshot.player.pitch,
    };
    #[cfg(debug_assertions)]
    if smoke_test {
        session.submit_command(PlayerCommand::Break);
        session.submit_command(PlayerCommand::Place);
        session.update(Duration::from_millis(50));
    }
    let weather_textures = snapshot.weather_terrain.as_ref().and_then(|_| {
        match crate::weather_render::WeatherTextures::load(&mut resources) {
            Ok(textures) => Some(textures),
            Err(error) => {
                eprintln!("Weather textures are unavailable: {error:#}");
                None
            }
        }
    });
    let mesher = MeshWorker::start(resources)?;
    run_inner(
        event_loop,
        scene,
        camera,
        smoke_test,
        assets,
        skin,
        Some(PlayState {
            session,
            streamer,
            mesher,
            trims,
            enchantments,
            sky_timeline,
            sky_background,
            end_sky,
            sun,
            weather_textures,
            weather_mesh: crate::weather_render::WeatherMesh::default(),
            weather_columns: crate::weather_render::WeatherColumnCache::default(),
            weather_frame: None,
            rain_fog: crate::rain_fog::RainFog::default(),
            rain_fog_millis: None,
            sky_background_tick: None,
            sky_background_revision: None,
            fog_color_previous: None,
            fog_color_current: None,
            fog_is_background: false,
            sunset_previous: None,
            sunset_current: None,
            sky_background_previous: None,
            sky_background_current: None,
            sky_fog_previous: None,
            sky_fog_current: None,
            sky_mix_previous: None,
            sky_mix_current: None,
            water_fog_previous: None,
            water_fog_current: None,
            sky_tick: None,
            sky_base: None,
            sky_lightmap: None,
            sky_settings: None,
            night_vision_cache: crate::preview_lighting::NightVisionCache::default(),
            applied_lightmap_settings: None,
            requested_revision: 0,
            scene_revision: 0,
            requested_blend: video.biome_blend,
            scene_blend: video.biome_blend,
        }),
        video,
        audio,
        hud_hidden,
        menu_screenshots,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_inner(
    event_loop: &mut EventLoop<()>,
    scene: PreviewScene,
    camera: Camera,
    smoke_test: bool,
    assets: Option<Arc<MenuAssets>>,
    skin: Option<Arc<crate::player_skin::PlayerSkin>>,
    play: Option<PlayState>,
    video: VideoSettings,
    audio: Option<crate::audio::UiAudio>,
    hud_hidden: bool,
    menu_screenshots: Option<std::rc::Rc<crate::screenshot::MenuScreenshots>>,
) -> Result<crate::GameExit> {
    let _ = smoke_test;
    event_loop.set_control_flow(ControlFlow::Poll);
    let now = Instant::now();
    let mut ui = assets.map(GameUi::new);
    if let Some(ui) = &mut ui {
        ui.set_video_settings(video);
        ui.set_audio(audio.clone());
        ui.set_hud_hidden(hud_hidden);
    }
    if let (Some(ui), Some(play)) = (&mut ui, &play) {
        ui.set_crafting_recipes(play.session.crafting_recipes());
        ui.set_trim_registry(Arc::clone(&play.trims));
        ui.set_enchantments(Arc::clone(&play.enchantments));
    }
    if let (Some(ui), Some(skin)) = (&mut ui, skin) {
        ui.set_player_skin(skin);
    }
    let mut app = Viewer {
        scene,
        camera,
        visual_look: crate::visual_look::VisualLook::default(),
        play,
        ui,
        menu_screenshots,
        audio,
        audio_listener: None,
        inventory_wait: None,
        menu_open: false,
        death_visible: false,
        inventory_shift: false,
        inventory_control: false,
        inventory_alt: false,
        #[cfg(debug_assertions)]
        smoke_test,
        window: None,
        renderer: None,
        input: Input::default(),
        mouse_bindings: crate::toggle_key::MouseBindings::default(),
        hotbar_scroll: crate::mouse_scroll::HotbarScroll::default(),
        captured: false,
        occluded: false,
        last_update: now,
        started: now,
        video,
        quit_application: false,
        frame_pacer: FramePacer::new(video.max_fps, now),
        inactivity: crate::frame_pacer::InactivityTracker::new(now),
        focus_pause: crate::focus_pause::FocusPause::default(),
        #[cfg(debug_assertions)]
        stats_since: now,
        #[cfg(debug_assertions)]
        frames: 0,
        #[cfg(debug_assertions)]
        total_frames: 0,
        error: None,
    };
    event_loop
        .run_app_on_demand(&mut app)
        .context("world viewer event loop failed")?;
    if let Some(error) = app.error {
        return Err(error);
    }
    Ok(if app.quit_application {
        crate::GameExit::Quit
    } else {
        crate::GameExit::ReturnToTitle(app.video, app.ui.as_ref().is_some_and(GameUi::hud_hidden))
    })
}

struct PlayState {
    session: LoadedSession,
    streamer: ChunkStreamer,
    mesher: MeshWorker,
    trims: Arc<crate::equipment_visual::TrimRegistry>,
    enchantments: Arc<crate::enchantment_tooltip::Registry>,
    sky_timeline: Option<crate::preview_lighting::SkyTimeline>,
    sky_background: Option<crate::sky_background::SkyBackground>,
    end_sky: Option<crate::end_sky::Texture>,
    sun: Option<crate::sun_render::Texture>,
    weather_textures: Option<crate::weather_render::WeatherTextures>,
    weather_mesh: crate::weather_render::WeatherMesh,
    weather_columns: crate::weather_render::WeatherColumnCache,
    weather_frame: Option<(u64, u64, u32, [u64; 3], glam::IVec3, u8)>,
    rain_fog: crate::rain_fog::RainFog,
    rain_fog_millis: Option<u128>,
    sky_background_tick: Option<u64>,
    sky_background_revision: Option<u64>,
    fog_color_previous: Option<[f32; 3]>,
    fog_color_current: Option<[f32; 3]>,
    fog_is_background: bool,
    sunset_previous: Option<(f32, [f32; 4])>,
    sunset_current: Option<(f32, [f32; 4])>,
    sky_background_previous: Option<[f32; 3]>,
    sky_background_current: Option<[f32; 3]>,
    sky_fog_previous: Option<[f32; 2]>,
    sky_fog_current: Option<[f32; 2]>,
    water_fog_previous: Option<crate::sky_background::WaterFogSample>,
    water_fog_current: Option<crate::sky_background::WaterFogSample>,
    sky_mix_previous: Option<([f32; 3], f32)>,
    sky_mix_current: Option<([f32; 3], f32)>,
    sky_tick: Option<u64>,
    sky_base: Option<Arc<native_world::lighting::LightmapSettings>>,
    sky_lightmap: Option<Arc<[[[f32; 3]; 16]; 16]>>,
    sky_settings: Option<native_world::lighting::LightmapSettings>,
    night_vision_cache: crate::preview_lighting::NightVisionCache,
    applied_lightmap_settings: Option<native_world::lighting::LightmapSettings>,
    requested_revision: u64,
    scene_revision: u64,
    requested_blend: u8,
    scene_blend: u8,
}

struct Viewer {
    scene: PreviewScene,
    camera: Camera,
    visual_look: crate::visual_look::VisualLook,
    play: Option<PlayState>,
    ui: Option<GameUi>,
    menu_screenshots: Option<std::rc::Rc<crate::screenshot::MenuScreenshots>>,
    audio: Option<crate::audio::UiAudio>,
    audio_listener: Option<(glam::DVec3, f32)>,
    inventory_wait: Option<u64>,
    menu_open: bool,
    death_visible: bool,
    inventory_shift: bool,
    inventory_control: bool,
    inventory_alt: bool,
    #[cfg(debug_assertions)]
    smoke_test: bool,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    input: Input,
    mouse_bindings: crate::toggle_key::MouseBindings,
    hotbar_scroll: crate::mouse_scroll::HotbarScroll,
    captured: bool,
    occluded: bool,
    last_update: Instant,
    started: Instant,
    video: VideoSettings,
    quit_application: bool,
    frame_pacer: FramePacer,
    inactivity: crate::frame_pacer::InactivityTracker,
    focus_pause: crate::focus_pause::FocusPause,
    #[cfg(debug_assertions)]
    stats_since: Instant,
    #[cfg(debug_assertions)]
    frames: u64,
    #[cfg(debug_assertions)]
    total_frames: u64,
    error: Option<anyhow::Error>,
}

fn obsolete_mesh(
    revision: u64,
    blend: u8,
    scene_revision: u64,
    scene_blend: u8,
    requested_blend: u8,
) -> bool {
    blend != requested_blend
        || revision < scene_revision
        || (revision == scene_revision && blend == scene_blend)
}

impl Viewer {
    fn apply_video(&mut self) {
        if let Some(ui) = &mut self.ui {
            ui.advance_video_settings(Instant::now());
        }
        let old_distance = self.video.render_distance;
        let old_fov_effect = self.video.fov_effect_scale;
        let old_sprint_window = self.video.sprint_window;
        let old_fullscreen = self.video.fullscreen;
        crate::apply_game_video(
            self.ui.as_ref(),
            &mut self.video,
            &mut self.frame_pacer,
            self.renderer.as_mut(),
        );
        if old_fullscreen != self.video.fullscreen
            && let Some(window) = &self.window
        {
            crate::apply_fullscreen(window, self.video.fullscreen);
        }
        if old_fov_effect != self.video.fov_effect_scale
            && let Some(play) = &mut self.play
        {
            play.session
                .set_fov_effect_scale(self.video.fov_effect_scale.value() as f32);
        }
        if old_distance != self.video.render_distance
            && let Some(play) = &mut self.play
        {
            play.streamer
                .set_radius(u32::from(self.video.render_distance));
        }
        if old_sprint_window != self.video.sprint_window
            && let Some(play) = &mut self.play
        {
            play.session.set_sprint_window(self.video.sprint_window);
        }
    }

    fn toggle_fullscreen(&mut self) {
        if let Some(ui) = &mut self.ui {
            ui.toggle_fullscreen();
            self.apply_video();
            crate::save_game_video(self.ui.as_mut());
        } else {
            self.video.fullscreen = !self.video.fullscreen;
            if let Some(window) = &self.window {
                crate::apply_fullscreen(window, self.video.fullscreen);
            }
            if let Err(error) = crate::menu_settings::save_video(self.video) {
                eprintln!("Could not save video settings: {error:#}");
            }
        }
    }

    fn pause_action(&mut self, action: PauseAction, event_loop: &ActiveEventLoop) {
        self.apply_video();
        match action {
            PauseAction::SaveVideo => {
                crate::save_game_video(self.ui.as_mut());
            }
            PauseAction::SaveSound => {
                crate::save_game_sound(self.ui.as_mut());
            }
            PauseAction::Resume => {
                crate::save_game_video(self.ui.as_mut());
                crate::save_game_sound(self.ui.as_mut());
                if let Some(ui) = &mut self.ui {
                    ui.resume();
                }
                self.input.restore_after_screen(self.video);
                self.submit_movement();
                self.last_update = Instant::now();
                self.capture(true);
            }
            PauseAction::QuitToTitle => {
                crate::finish_game_video(self.ui.as_mut());
                event_loop.exit();
            }
            PauseAction::None => {}
        }
    }

    fn death_action(&mut self, action: crate::death_ui::DeathAction, event_loop: &ActiveEventLoop) {
        match action {
            crate::death_ui::DeathAction::Respawn => {
                if let Some(play) = &mut self.play {
                    play.session.submit_command(PlayerCommand::Respawn);
                }
            }
            crate::death_ui::DeathAction::QuitToTitle => {
                self.pause_action(PauseAction::QuitToTitle, event_loop)
            }
            crate::death_ui::DeathAction::None => {}
        }
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: anyhow::Error) {
        self.error = Some(error);
        event_loop.exit();
    }

    fn capture(&mut self, enabled: bool) {
        if !enabled {
            self.mouse_bindings.reset();
            if let Some(play) = &mut self.play {
                play.session.submit_command(PlayerCommand::StopBreak);
                play.session.submit_command(PlayerCommand::StopUse);
            }
        }
        if let Some(window) = &self.window {
            if enabled {
                match window
                    .set_cursor_grab(CursorGrabMode::Locked)
                    .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined))
                {
                    Ok(()) => {
                        window.set_cursor_visible(false);
                        self.captured = true;
                    }
                    Err(error) => eprintln!("Could not capture the mouse: {error}"),
                }
            } else {
                let _ = window.set_cursor_grab(CursorGrabMode::None);
                window.set_cursor_visible(true);
                self.captured = false;
            }
        }
    }

    fn update(&mut self, elapsed: Duration) -> Result<()> {
        if let Some(play) = &mut self.play {
            play.streamer.service(&mut play.session)?;
            play.session.update(elapsed);
            play.streamer.service(&mut play.session)?;
            let snapshot = play.session.snapshot();
            let particles = play.session.take_consume_particles();
            let plant_breaks = play.session.take_plant_break_particles();
            let smoke = play.session.take_evaporation_smoke();
            let fizz = play.session.take_lava_fizz();
            let fizz_sounds = if let Some(ui) = &mut self.ui {
                ui.queue_plant_break_particles(plant_breaks);
                ui.update_consume_particles(&snapshot, particles);
                ui.update_evaporation_smoke(&snapshot, smoke, fizz)
            } else {
                Vec::new()
            };
            self.camera.eye =
                eye_for_scene(&snapshot, self.scene.origin, play.session.partial_tick());
            (self.camera.yaw, self.camera.pitch) = self.visual_look.angles(&snapshot);
            let eye = snapshot.camera.eye(play.session.partial_tick()) + snapshot.origin.as_dvec3();
            if let Some(audio) = &self.audio
                && self.audio_listener != Some((eye, self.camera.yaw))
            {
                audio.set_listener(eye, self.camera.yaw);
                self.audio_listener = Some((eye, self.camera.yaw));
            }
            if let Some(audio) = &self.audio {
                for sound in fizz_sounds {
                    audio.consume(sound);
                }
            }
            let mut buttons = play.session.take_button_sounds().into_iter().peekable();
            let mut levers = play.session.take_lever_sounds().into_iter().peekable();
            let mut chests = play.session.take_chest_sounds().into_iter().peekable();
            let mut locked_chests = play
                .session
                .take_locked_chest_sounds()
                .into_iter()
                .peekable();
            let mut buckets = play.session.take_bucket_sounds().into_iter().peekable();
            let mut compasses = play.session.take_compass_sounds().into_iter().peekable();
            while let Some((_, kind)) = [
                buttons.peek().map(|event| (event.sequence, 0)),
                levers.peek().map(|event| (event.sequence, 1)),
                chests.peek().map(|event| (event.sequence, 2)),
                locked_chests.peek().map(|event| (event.sequence, 3)),
                buckets.peek().map(|event| (event.sequence, 4)),
                compasses.peek().map(|event| (event.sequence, 5)),
            ]
            .into_iter()
            .flatten()
            .min()
            {
                match kind {
                    0 => {
                        let event = buttons.next().unwrap();
                        if let Some(audio) = &self.audio {
                            audio.button(event);
                        }
                    }
                    1 => {
                        let event = levers.next().unwrap();
                        if let Some(audio) = &self.audio {
                            audio.lever(event);
                        }
                    }
                    2 => {
                        let event = chests.next().unwrap();
                        if let Some(audio) = &self.audio {
                            audio.chest(event);
                        }
                    }
                    3 => {
                        let event = locked_chests.next().unwrap();
                        if let Some(audio) = &self.audio {
                            audio.locked_chest(event);
                        }
                    }
                    4 => {
                        let event = buckets.next().unwrap();
                        if let Some(audio) = &self.audio {
                            audio.bucket(event);
                        }
                    }
                    _ => {
                        let event = compasses.next().unwrap();
                        if let Some(audio) = &self.audio {
                            audio.compass(event);
                        }
                    }
                }
            }
            for sound in play.session.take_pickup_sounds() {
                if let Some(audio) = &self.audio {
                    audio.play_pickup(sound);
                }
            }
            for sound in play.session.take_consume_sounds() {
                if let Some(audio) = &self.audio {
                    audio.consume(sound);
                }
            }
            for sound in play.session.take_extinguish_sounds() {
                if let Some(audio) = &self.audio {
                    audio.extinguish(sound);
                }
            }
        } else {
            self.camera.advance(&self.input, elapsed);
        }
        Ok(())
    }

    fn submit_movement(&mut self) {
        if let Some(play) = &mut self.play {
            play.session.submit_command(PlayerCommand::Move {
                forward: i8::from(self.input.forward) as f32 - i8::from(self.input.back) as f32,
                strafe: i8::from(self.input.right) as f32 - i8::from(self.input.left) as f32,
                jump: self.input.up,
            });
            play.session.submit_command(PlayerCommand::Sprint {
                pressed: self.input.fast(),
                backward_pressed: self.input.back,
            });
            play.session
                .submit_command(PlayerCommand::Sneak(self.input.down()));
        }
    }

    fn request_inventory_toggle(&mut self) {
        if self.inventory_wait.is_some() {
            return;
        }
        let Some(play) = &mut self.play else {
            return;
        };
        self.inventory_wait = Some(play.session.snapshot().tick);
        if self
            .ui
            .as_mut()
            .is_some_and(GameUi::inventory_pointer_cancel)
        {
            play.session.submit_command(PlayerCommand::InventoryPointer(
                InventoryPointerEvent::Cancel,
            ));
        }
        play.session.submit_command(PlayerCommand::ToggleInventory);
        self.input.release_for_screen(self.video);
        self.submit_movement();
        self.capture(false);
    }

    fn refresh_scene(&mut self) -> Result<()> {
        let Some(play) = &mut self.play else {
            return Ok(());
        };
        let snapshot = play.session.snapshot();
        if snapshot.world_revision != play.requested_revision
            || self.video.biome_blend != play.requested_blend
        {
            let world = snapshot
                .world
                .loaded()
                .context("play session returned the wrong world type")?;
            play.mesher.submit(MeshJob {
                revision: snapshot.world_revision,
                biome_blend: self.video.biome_blend,
                origin: snapshot.origin,
                world: Arc::clone(world),
            });
            play.requested_revision = snapshot.world_revision;
            play.requested_blend = self.video.biome_blend;
        }
        while let Some(result) = play.mesher.try_result()? {
            if obsolete_mesh(
                result.revision,
                result.biome_blend,
                play.scene_revision,
                play.scene_blend,
                self.video.biome_blend,
            ) {
                continue;
            }
            let scene = match result.scene {
                Ok(scene) => scene,
                Err(error) if result.revision != snapshot.world_revision => {
                    eprintln!(
                        "Outdated world mesh revision {} failed: {error:#}",
                        result.revision
                    );
                    continue;
                }
                Err(error) => return Err(error),
            };
            let renderer = self
                .renderer
                .as_mut()
                .context("renderer is not initialized")?;
            replace_scene(renderer, &mut self.scene, scene)?;
            self.camera.eye =
                eye_for_scene(&snapshot, self.scene.origin, play.session.partial_tick());
            play.scene_revision = result.revision;
            play.scene_blend = result.biome_blend;
            println!("World play mesh updated: revision {}", result.revision);
        }
        Ok(())
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        self.apply_video();
        let now = Instant::now();
        if let Some(play) = &self.play {
            let snapshot = play.session.snapshot();
            crate::deliver_menu_screenshots(
                self.ui.as_mut(),
                self.menu_screenshots.as_deref(),
                snapshot.tick,
            );
            if let (Some(ui), Some(renderer)) = (&mut self.ui, &mut self.renderer) {
                while let Some(result) = renderer.take_screenshot_result() {
                    ui.screenshot_result(result, snapshot.tick);
                }
            }
            if self.death_visible != snapshot.death.is_some() {
                self.death_visible = snapshot.death.is_some();
                self.inventory_wait = None;
                if self.death_visible {
                    if let Some(ui) = &mut self.ui {
                        ui.close_chat();
                    }
                    if let Some(window) = &self.window {
                        window.set_ime_allowed(false);
                    }
                    self.input = Input::default();
                    self.capture(false);
                } else if !snapshot.inventory.is_open()
                    && !self.ui.as_ref().is_some_and(GameUi::paused)
                {
                    self.capture(true);
                }
            }
            if let Some(ui) = &mut self.ui {
                ui.sync_death(&snapshot);
            }
            if self.inventory_wait.is_some_and(|tick| snapshot.tick > tick) {
                self.inventory_wait = None;
            }
            if self.ui.is_some() && self.menu_open != snapshot.inventory.is_open() {
                self.menu_open = snapshot.inventory.is_open();
                if self.menu_open {
                    self.input.release_for_screen(self.video);
                    self.submit_movement();
                    self.capture(false);
                } else if snapshot.death.is_none() && !self.ui.as_ref().is_some_and(GameUi::paused)
                {
                    self.input.restore_after_screen(self.video);
                    self.submit_movement();
                    self.capture(true);
                }
            }
        }
        let tick = self
            .play
            .as_mut()
            .map(|play| u128::from(play.session.snapshot().tick))
            .unwrap_or_else(|| now.saturating_duration_since(self.started).as_millis() / 50);
        if let Err(error) = self.refresh_scene() {
            self.fail(event_loop, error);
            return;
        }
        if let (Some(play), Some(renderer)) = (&mut self.play, &mut self.renderer)
            && let Some(background) = &play.sky_background
        {
            let snapshot = play.session.snapshot();
            let milliseconds = now.saturating_duration_since(self.started).as_millis();
            let delta_ticks = play
                .rain_fog_millis
                .map_or(0.0, |last| milliseconds.saturating_sub(last) as f32 / 50.0);
            play.rain_fog_millis = Some(milliseconds);
            let partial = play.session.partial_tick().clamp(0.0, 1.0);
            let camera = self.scene.origin.as_dvec3() + self.camera.eye.as_dvec3();
            if let Some((rain, light, precipitation)) =
                crate::rain_fog::inputs(&snapshot, camera, partial)
            {
                play.rain_fog
                    .update(rain, light, precipitation, delta_ticks);
            }
            if play.sky_background_tick != Some(snapshot.tick)
                || play.sky_background_revision != Some(snapshot.world_revision)
            {
                let position = snapshot.origin.as_dvec3() + snapshot.camera.eye(1.0);
                let value = snapshot.world.loaded().and_then(|world| {
                    background.sample(world, position, |id| {
                        let clock = native_persistence::world::DimensionId {
                            namespace: id.namespace.clone(),
                            path: id.path.clone(),
                        };
                        play.session
                            .world_clock(&clock)
                            .map(|state| state.total_ticks)
                    })
                });
                if let Some(crate::sky_background::BackgroundSample {
                    color: value,
                    background_is_fog,
                    fog_color,
                    fog_range,
                    sky_mix,
                    sunset,
                    water,
                }) = value
                {
                    if play.sky_background_tick != Some(snapshot.tick)
                        || play.sky_background_previous.is_none()
                    {
                        play.sky_background_previous =
                            Some(play.sky_background_current.unwrap_or(value));
                    }
                    play.sky_background_current = Some(value);
                    play.fog_is_background = background_is_fog;
                    if play.sky_background_tick != Some(snapshot.tick)
                        || play.fog_color_previous.is_none()
                        || play.fog_color_current.is_none()
                    {
                        play.fog_color_previous = play.fog_color_current.or(fog_color);
                    }
                    play.fog_color_current = fog_color;
                    if play.sky_background_tick != Some(snapshot.tick)
                        || play.sunset_previous.is_none()
                        || play.sunset_current.is_none()
                    {
                        play.sunset_previous = play.sunset_current.or(sunset);
                    }
                    play.sunset_current = sunset;
                    if play.sky_background_tick != Some(snapshot.tick)
                        || play.sky_fog_previous.is_none()
                        || play.sky_fog_current.is_none()
                    {
                        play.sky_fog_previous = play.sky_fog_current.or(fog_range);
                    }
                    play.sky_fog_current = fog_range;
                    if play.sky_background_tick != Some(snapshot.tick)
                        || play.sky_mix_previous.is_none()
                        || play.sky_mix_current.is_none()
                    {
                        play.sky_mix_previous = play.sky_mix_current.or(sky_mix);
                    }
                    play.sky_mix_current = sky_mix;
                    if play.sky_background_tick != Some(snapshot.tick)
                        || play.water_fog_previous.is_none()
                        || play.water_fog_current.is_none()
                    {
                        play.water_fog_previous = play.water_fog_current.or(water);
                    }
                    play.water_fog_current = water;
                }
                play.sky_background_tick = Some(snapshot.tick);
                play.sky_background_revision = Some(snapshot.world_revision);
            }
            if let (Some(previous), Some(current)) =
                (play.sky_background_previous, play.sky_background_current)
            {
                let partial = play.session.partial_tick().clamp(0.0, 1.0);
                let sun_frame = match (play.sunset_previous, play.sunset_current) {
                    (Some((previous, _)), Some((current, _))) if background.has_overworld_sky() => {
                        Some(crate::sun_render::Frame {
                            angle_degrees: interpolate_sun_angle(previous, current, partial),
                            rain_brightness: 1.0
                                - snapshot
                                    .weather
                                    .map_or(0.0, |weather| weather.rain_at(partial)),
                            tick: snapshot.tick,
                        })
                    }
                    _ => None,
                };
                renderer.set_sun_frame(sun_frame);
                let color =
                    std::array::from_fn(|i| previous[i] + partial * (current[i] - previous[i]));
                let mut fog_color = match (play.fog_color_previous, play.fog_color_current) {
                    (Some(previous), Some(current)) => Some(std::array::from_fn(|i| {
                        previous[i] + partial * (current[i] - previous[i])
                    })),
                    _ => None,
                };
                if let (Some(fog), Some((old_angle, old_color)), Some((new_angle, new_color))) =
                    (fog_color, play.sunset_previous, play.sunset_current)
                {
                    let angle = interpolate_sun_angle(old_angle, new_angle, partial);
                    let color = std::array::from_fn(|i| {
                        old_color[i] + partial * (new_color[i] - old_color[i])
                    });
                    fog_color = Some(sunrise_fog_color(
                        fog,
                        angle,
                        color,
                        self.camera.direction().x,
                        play.streamer.radius().max(2),
                    ));
                }
                if let (
                    Some(fog),
                    Some((previous_sky, previous_end)),
                    Some((current_sky, current_end)),
                ) = (fog_color, play.sky_mix_previous, play.sky_mix_current)
                {
                    let sky = std::array::from_fn(|i| {
                        previous_sky[i] + partial * (current_sky[i] - previous_sky[i])
                    });
                    let (rain, thunder) = snapshot.weather.map_or((0.0, 0.0), |weather| {
                        (weather.rain_at(partial), weather.thunder_at(partial))
                    });
                    let sky = weather_darken_sky(sky, rain, thunder);
                    let end = previous_end + partial * (current_end - previous_end);
                    fog_color = Some(atmospheric_fog_color(
                        fog,
                        sky,
                        end,
                        play.streamer.radius().max(2),
                    ));
                }
                let camera_y = f64::from(snapshot.origin.y) + snapshot.camera.eye(partial).y;
                let void_darkness = background.void_darkness(camera_y);
                let night_vision = crate::preview_lighting::night_vision_scale(
                    snapshot.night_vision_duration,
                    partial,
                );
                fog_color =
                    fog_color.map(|color| adjusted_fog_color(color, void_darkness, night_vision));
                let fog_range = match (play.sky_fog_previous, play.sky_fog_current) {
                    (Some(previous), Some(current)) => {
                        Some(play.rain_fog.apply(std::array::from_fn(|i| {
                            previous[i] + partial * (current[i] - previous[i])
                        })))
                    }
                    _ => None,
                };
                let water = if play.session.camera_in_water(snapshot.camera.eye(partial)) {
                    match (play.water_fog_previous, play.water_fog_current) {
                        (Some(previous), Some(current)) => {
                            let color = std::array::from_fn(|i| {
                                previous.color[i] + partial * (current.color[i] - previous.color[i])
                            });
                            let range = std::array::from_fn(|i| {
                                previous.range[i] + partial * (current.range[i] - previous.range[i])
                            });
                            Some(water_fog(
                                color,
                                range,
                                snapshot.water_vision,
                                void_darkness,
                            ))
                        }
                        _ => None,
                    }
                } else {
                    None
                };
                if let Some((color, range)) = water {
                    renderer.set_sky_color(color);
                    renderer.set_fog_color(Some(color));
                    renderer.set_fog_range(Some(range));
                } else {
                    renderer.set_sky_color(if play.fog_is_background {
                        fog_color.unwrap_or(color)
                    } else {
                        color
                    });
                    renderer.set_fog_color(fog_color);
                    renderer.set_fog_range(fog_range);
                }
            }
        }
        if let (Some(play), Some(renderer)) = (&mut self.play, &mut self.renderer) {
            let snapshot = play.session.snapshot();
            let tick = snapshot.tick;
            let base_changed = play
                .sky_base
                .as_ref()
                .is_none_or(|base| !Arc::ptr_eq(base, &self.scene.base_lightmap));
            if let Some(timeline) = &play.sky_timeline
                && (play.sky_tick != Some(tick) || base_changed)
            {
                let sample = timeline.sample(&self.scene.base_lightmap, |id| {
                    let clock = native_persistence::world::DimensionId {
                        namespace: id.namespace.clone(),
                        path: id.path.clone(),
                    };
                    play.session
                        .world_clock(&clock)
                        .map(|state| state.total_ticks)
                });
                if let Some(settings) = sample {
                    play.sky_settings = Some(settings);
                    play.sky_tick = Some(tick);
                    play.sky_base = Some(Arc::clone(&self.scene.base_lightmap));
                } else {
                    eprintln!("Sky light timeline clock is unavailable");
                    play.sky_timeline = None;
                    play.sky_settings = None;
                }
            }
            let settings = crate::preview_lighting::apply_night_vision(
                play.sky_settings
                    .clone()
                    .unwrap_or_else(|| (*self.scene.base_lightmap).clone()),
                play.night_vision_cache.sample(
                    tick,
                    snapshot.night_vision_duration,
                    snapshot.water_vision,
                    snapshot
                        .active_effects
                        .iter()
                        .any(|effect| effect.id == "conduit_power"),
                    play.session.partial_tick().clamp(0.0, 1.0),
                ),
            );
            if play.applied_lightmap_settings.as_ref() != Some(&settings) {
                play.sky_lightmap = Some(Arc::new(settings.table()));
                play.applied_lightmap_settings = Some(settings);
            }
            if let Some(table) = &play.sky_lightmap
                && !Arc::ptr_eq(table, &self.scene.lightmap)
            {
                self.scene.lightmap = Arc::clone(table);
                renderer.set_preview_lightmap(table);
            }
        }
        if let (Some(ui), Some(renderer), Some(window), Some(play)) = (
            &mut self.ui,
            &mut self.renderer,
            &self.window,
            &mut self.play,
        ) {
            let size = window.inner_size();
            let snapshot = play.session.snapshot();
            if let Err(error) = ui.draw(
                renderer,
                (size.width, size.height),
                &snapshot,
                self.scene.origin,
                play.session.partial_tick(),
                Some(&self.scene.biome_colors),
            ) {
                self.fail(event_loop, error);
                return;
            }
        }
        let render_snapshot = self.play.as_ref().map(|play| play.session.snapshot());
        if let Some(renderer) = &mut self.renderer {
            renderer.set_view_yaw(self.camera.yaw);
        }
        if let (Some(renderer), Some(play), Some(snapshot)) =
            (&mut self.renderer, &mut self.play, &render_snapshot)
        {
            let camera = self.scene.origin.as_dvec3() + self.camera.eye.as_dvec3();
            let partial = play.session.partial_tick();
            let key = (
                snapshot.tick,
                snapshot.world_revision,
                partial.to_bits(),
                [camera.x.to_bits(), camera.y.to_bits(), camera.z.to_bits()],
                self.scene.origin,
                self.video.weather_radius,
            );
            if play.weather_frame.as_ref() != Some(&key) {
                crate::weather_render::build_into(
                    &mut play.weather_mesh,
                    &mut play.weather_columns,
                    snapshot,
                    self.scene.origin,
                    camera,
                    partial,
                    i32::from(self.video.weather_radius),
                );
                if let Err(error) = renderer.update_weather(&play.weather_mesh) {
                    self.fail(event_loop, error);
                    return;
                }
                play.weather_frame = Some(key);
            }
            renderer.set_chest_lids(
                Arc::clone(&snapshot.chest_lids),
                play.session.partial_tick(),
            );
            renderer.set_banner_time(snapshot.game_time, play.session.partial_tick());
            renderer.set_fov_modifier(snapshot.camera.fov_modifier(play.session.partial_tick()));
            renderer.set_camera_bob(&snapshot.camera, play.session.partial_tick());
            if let Err(error) = renderer.update_experience_orbs(
                &snapshot.experience_orbs,
                self.scene.origin,
                self.camera.direction(),
                play.session.partial_tick(),
            ) {
                self.fail(event_loop, error);
                return;
            }
        }
        if let Some(renderer) = &mut self.renderer
            && let Err(error) = renderer.render_preview(
                &mut self.scene,
                self.camera.eye,
                self.camera.direction(),
                tick as u64,
                render_snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.breaking),
                render_snapshot
                    .as_ref()
                    .map_or(&[], |snapshot| snapshot.dropped_items.as_ref()),
            )
        {
            self.fail(event_loop, error);
            #[cfg(debug_assertions)]
            return;
        }
        #[cfg(debug_assertions)]
        {
            self.frames += 1;
            self.total_frames += 1;
            if self.smoke_test && self.total_frames >= 3 {
                if let Some(play) = &self.play {
                    if play.scene_revision != play.session.snapshot().world_revision {
                        if now.saturating_duration_since(self.started) > Duration::from_secs(30) {
                            self.fail(
                                event_loop,
                                anyhow::anyhow!("world play mesh did not finish within 30 seconds"),
                            );
                        }
                        return;
                    }
                    println!(
                        "World play smoke test passed: {} frames, revision {}",
                        self.total_frames,
                        play.session.snapshot().world_revision
                    );
                } else {
                    println!(
                        "World viewer smoke test passed: {} frames",
                        self.total_frames
                    );
                }
                event_loop.exit();
                return;
            }
            let elapsed = now.saturating_duration_since(self.stats_since);
            if elapsed >= Duration::from_secs(1) {
                if let (Some(window), Some(renderer)) = (&self.window, &self.renderer) {
                    if let Some(play) = &self.play {
                        let snapshot = play.session.snapshot();
                        window.set_title(&format!("Minecraft Native | experimental play, unsaved | {} | {:.0} FPS | tick {} | {:?} [1-9]", renderer.backend_name, self.frames as f64 / elapsed.as_secs_f64(), snapshot.tick, snapshot.selected_block));
                    } else {
                        window.set_title(&format!("Minecraft Native | read-only viewer | {} | {:.0} FPS | {:.1}, {:.1}, {:.1} | WASD fly, Space/Shift altitude, Ctrl fast, Esc releases mouse", renderer.backend_name, self.frames as f64 / elapsed.as_secs_f64(), f64::from(self.camera.eye.x) + f64::from(self.scene.origin.x), f64::from(self.camera.eye.y) + f64::from(self.scene.origin.y), f64::from(self.camera.eye.z) + f64::from(self.scene.origin.z)));
                    }
                }
                self.frames = 0;
                self.stats_since = now;
            }
        }
    }
}

fn replace_scene(
    renderer: &mut Renderer,
    current: &mut PreviewScene,
    mut next: PreviewScene,
) -> Result<()> {
    if next.material_generation.is_some() && next.material_generation == current.material_generation
    {
        std::mem::swap(&mut next.atlas, &mut current.atlas);
        renderer.replace_preview_mesh();
    } else {
        renderer.replace_preview(&next)?;
    }
    *current = next;
    Ok(())
}

fn eye_for_scene(snapshot: &WorldSnapshot, scene_origin: glam::IVec3, partial_tick: f32) -> Vec3 {
    let shift = glam::DVec3::new(
        f64::from(snapshot.origin.x) - f64::from(scene_origin.x),
        f64::from(snapshot.origin.y) - f64::from(scene_origin.y),
        f64::from(snapshot.origin.z) - f64::from(scene_origin.z),
    );
    (snapshot.camera.eye(partial_tick) + shift).as_vec3()
}

impl ApplicationHandler for Viewer {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_fullscreen(crate::fullscreen_mode(self.video.fullscreen))
            .with_title("Minecraft 26.3")
            .with_inner_size(winit::dpi::LogicalSize::new(1280, 720))
            .with_min_inner_size(winit::dpi::LogicalSize::new(320, 240));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                self.fail(event_loop, error.into());
                return;
            }
        };
        if let Some(screenshots) = &self.menu_screenshots {
            screenshots.set_window(&window);
        }
        let mut renderer = match Renderer::new(
            Arc::clone(&window),
            &self.scene.atlas.texture,
            self.ui.as_ref().map(GameUi::breaking_textures),
            self.ui.as_ref().map(GameUi::item_textures),
            self.ui.as_ref().map(GameUi::experience_orb_texture),
            self.ui.as_ref().map(GameUi::item_glint_texture),
            self.video.vsync,
        ) {
            Ok(renderer) => renderer,
            Err(error) => {
                self.fail(event_loop, error);
                return;
            }
        };
        renderer.set_fov(self.video.fov);
        renderer.set_view_bobbing(self.video.view_bobbing);
        if let Some(texture) = self.play.as_ref().and_then(|play| play.end_sky.as_ref())
            && let Err(error) = renderer.set_end_sky(texture)
        {
            eprintln!("End sky rendering is unavailable: {error:#}");
        }
        if let Some(texture) = self.play.as_ref().and_then(|play| play.sun.as_ref())
            && let Err(error) = renderer.set_sun(texture.clone())
        {
            eprintln!("Sun rendering is unavailable: {error:#}");
        }
        if let Some(textures) = self
            .play
            .as_ref()
            .and_then(|play| play.weather_textures.as_ref())
            && let Err(error) = renderer.set_weather_textures(textures)
        {
            eprintln!("Weather rendering is unavailable: {error:#}");
        }
        #[cfg(debug_assertions)]
        println!("World viewer graphics backend: {}", renderer.backend_name);
        self.window = Some(window);
        self.renderer = Some(renderer);
        self.last_update = Instant::now();
        self.started = self.last_update;
        self.frame_pacer = FramePacer::new(self.video.max_fps, self.last_update);
        self.inactivity = crate::frame_pacer::InactivityTracker::new(self.last_update);
        #[cfg(debug_assertions)]
        {
            self.stats_since = self.last_update;
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self
            .window
            .as_ref()
            .is_none_or(|window| window.id() != window_id)
        {
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
            if self.inactivity.input(now, self.video.inactivity_fps_limit) {
                self.frame_pacer.set_limit(self.video.max_fps, now);
            }
        }
        match event {
            WindowEvent::CloseRequested => {
                crate::finish_game_video(self.ui.as_mut());
                self.quit_application = true;
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(size);
                }
                if let Some(ui) = &mut self.ui {
                    ui.resize((size.width, size.height));
                }
                crate::update_chat_ime_area(&self.window, &self.ui);
            }
            WindowEvent::Occluded(occluded) => self.occluded = occluded,
            WindowEvent::ModifiersChanged(modifiers) => {
                self.inventory_shift = modifiers.state().shift_key();
                self.inventory_control = modifiers.state().control_key();
                self.inventory_alt = modifiers.state().alt_key();
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(ui) = &mut self.ui {
                    ui.cursor_moved(position.x, position.y);
                    if let Some(slot) = ui.inventory_pointer_move()
                        && let Some(play) = &mut self.play
                    {
                        play.session.submit_command(PlayerCommand::InventoryPointer(
                            InventoryPointerEvent::Move { slot },
                        ));
                    }
                }
                crate::update_chat_ime_area(&self.window, &self.ui);
                self.apply_video();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let amount = crate::mouse_scroll::scaled_axes(
                    crate::mouse_scroll::event_axes(delta),
                    self.video,
                );
                if let Some(ui) = &mut self.ui
                    && ui.chat_open()
                {
                    let steps = amount.1.signum() as i32 * if self.inventory_shift { 1 } else { 7 };
                    ui.chat_scroll(steps);
                } else if self.ui.as_ref().is_some_and(GameUi::paused) {
                    if let Some(ui) = &mut self.ui {
                        ui.pause_scroll(amount.1);
                    }
                } else if self.captured
                    && let Some(play) = &mut self.play
                {
                    let snapshot = play.session.snapshot();
                    if snapshot.death.is_none()
                        && !snapshot.inventory.is_open()
                        && let Some(command) = self.hotbar_scroll.game_command(
                            amount,
                            snapshot.game_mode,
                            snapshot.inventory.selected(),
                        )
                    {
                        play.session.submit_command(command);
                    }
                }
            }
            WindowEvent::Focused(focused) => {
                self.focus_pause.focus_changed(focused, Instant::now());
                if focused {
                    if self.inventory_wait.is_none()
                        && !self
                            .play
                            .as_ref()
                            .is_some_and(|play| play.session.snapshot().inventory.is_open())
                        && !self
                            .ui
                            .as_ref()
                            .is_some_and(|ui| ui.paused() || ui.chat_open())
                    {
                        self.input.restore_after_screen(self.video);
                        self.submit_movement();
                    }
                    return;
                }
                self.inventory_shift = false;
                self.inventory_control = false;
                self.inventory_alt = false;
                self.input.release_for_screen(self.video);
                self.submit_movement();
                self.capture(false);
                if let Some(ui) = &mut self.ui {
                    ui.chat_release();
                    if ui.inventory_pointer_cancel()
                        && let Some(play) = &mut self.play
                    {
                        play.session.submit_command(PlayerCommand::InventoryPointer(
                            InventoryPointerEvent::Cancel,
                        ));
                    }
                    ui.recipe_book_unfocus();
                    ui.pause_cancel();
                }
            }
            WindowEvent::Ime(ime) => {
                if let Some(ui) = &mut self.ui
                    && ui.chat_open()
                {
                    match ime {
                        winit::event::Ime::Preedit(text, _) => ui.chat_preedit(text),
                        winit::event::Ime::Commit(text) => {
                            if !text.is_empty() {
                                ui.chat_text(&text);
                            }
                        }
                        winit::event::Ime::Disabled => ui.chat_preedit(String::new()),
                        winit::event::Ime::Enabled => {}
                    }
                }
                crate::update_chat_ime_area(&self.window, &self.ui);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if crate::screenshot_shortcut(event.physical_key, event.state, event.repeat) {
                    crate::request_game_screenshot(&self.ui, &mut self.renderer);
                    return;
                }
                if crate::fullscreen_shortcut(event.physical_key, event.state, event.repeat) {
                    self.toggle_fullscreen();
                    return;
                }
                if let Some(ui) = &mut self.ui
                    && ui.chat_open()
                {
                    let mut clipboard =
                        crate::clipboard::SystemClipboard::new(self.window.as_deref());
                    if crate::handle_chat_clipboard_shortcut(
                        ui,
                        &event.logical_key,
                        event.state,
                        self.inventory_control,
                        self.inventory_shift,
                        self.inventory_alt,
                        &mut clipboard,
                    ) {
                        crate::update_chat_ime_area(&self.window, &self.ui);
                        return;
                    }
                    let tick = self
                        .play
                        .as_ref()
                        .map_or(0, |play| play.session.snapshot().tick);
                    let closed = crate::handle_chat_key(
                        ui,
                        event.physical_key,
                        event.state,
                        event.text.as_deref(),
                        self.inventory_control,
                        self.inventory_shift,
                        self.inventory_alt,
                        tick,
                    );
                    if let Some(command) = ui.take_chat_command()
                        && let Some(play) = &mut self.play
                    {
                        play.session.submit_command(command);
                    }
                    if closed {
                        if let Some(window) = &self.window {
                            window.set_ime_allowed(false);
                        }
                        self.input.restore_after_screen(self.video);
                        self.submit_movement();
                        self.capture(true);
                    } else {
                        crate::update_chat_ime_area(&self.window, &self.ui);
                    }
                    return;
                }
                if let PhysicalKey::Code(code) = event.physical_key {
                    let snapshot = self.play.as_ref().map(|play| play.session.snapshot());
                    let screen_open = self.ui.is_none()
                        || snapshot.is_none()
                        || self.inventory_wait.is_some()
                        || snapshot
                            .as_ref()
                            .is_some_and(|s| s.inventory.is_open() || s.death.is_some())
                        || self
                            .ui
                            .as_ref()
                            .is_some_and(|ui| ui.paused() || ui.chat_open());
                    if self
                        .focus_pause
                        .key(code, event.state == ElementState::Pressed, screen_open)
                    {
                        self.video.pause_on_lost_focus = !self.video.pause_on_lost_focus;
                        if let (Some(ui), Some(snapshot)) = (&mut self.ui, snapshot) {
                            ui.debug_focus_pause(self.video.pause_on_lost_focus, snapshot.tick);
                            crate::save_game_video(Some(ui));
                        }
                        return;
                    }
                }
                if let Some(snapshot) = self.play.as_ref().map(|play| play.session.snapshot())
                    && snapshot.death.is_some()
                {
                    if let Some(ui) = &mut self.ui {
                        ui.sync_death(&snapshot);
                        if event.state == ElementState::Pressed
                            && !event.repeat
                            && let PhysicalKey::Code(code) = event.physical_key
                        {
                            let action = ui.death_key(code, self.inventory_shift);
                            self.death_action(action, event_loop);
                        }
                    }
                    return;
                }
                let inventory_open = self
                    .play
                    .as_ref()
                    .is_some_and(|play| play.session.snapshot().inventory.is_open());
                if inventory_open
                    && event.state == ElementState::Pressed
                    && let PhysicalKey::Code(code) = event.physical_key
                {
                    if code == KeyCode::Escape
                        && let Some(BookAction::Settings { open, filtering }) =
                            self.ui.as_mut().and_then(GameUi::recipe_book_escape)
                    {
                        if let Some(play) = &mut self.play {
                            play.session
                                .submit_command(PlayerCommand::RecipeBookSettings {
                                    open,
                                    filtering,
                                });
                        }
                        return;
                    }
                    if let Some(ui) = &mut self.ui
                        && ui.recipe_book_captures_text()
                        && code != KeyCode::Escape
                    {
                        if code == KeyCode::Backspace {
                            ui.recipe_book_backspace();
                        } else if !self.inventory_control
                            && let Some(text) = &event.text
                        {
                            ui.recipe_book_input(text);
                        }
                        return;
                    }
                }
                if let PhysicalKey::Code(code) = event.physical_key {
                    let pressed = event.state == ElementState::Pressed;
                    let inventory_open = self
                        .play
                        .as_ref()
                        .is_some_and(|play| play.session.snapshot().inventory.is_open());
                    if pressed
                        && !event.repeat
                        && self.ui.is_some()
                        && self.play.is_some()
                        && ((code == KeyCode::KeyE
                            && !self.ui.as_ref().is_some_and(GameUi::paused))
                            || (code == KeyCode::Escape && inventory_open))
                    {
                        self.request_inventory_toggle();
                        return;
                    }
                    if self.inventory_wait.is_some() {
                        return;
                    }
                    if inventory_open {
                        if pressed
                            && let Some(target) = inventory_swap_key(code)
                            && let (Some(play), Some(slot)) = (
                                &mut self.play,
                                self.ui.as_ref().and_then(GameUi::inventory_slot),
                            )
                        {
                            play.session
                                .submit_command(PlayerCommand::InventorySwap { slot, target });
                            return;
                        }
                        if pressed
                            && code == KeyCode::KeyQ
                            && let (Some(play), Some(slot)) = (
                                &mut self.play,
                                self.ui.as_ref().and_then(GameUi::inventory_slot),
                            )
                        {
                            play.session.submit_command(PlayerCommand::InventoryThrow {
                                slot,
                                all: self.inventory_control,
                            });
                        }
                        return;
                    }
                    if self.ui.as_ref().is_some_and(GameUi::paused) {
                        if pressed
                            && (!event.repeat
                                || matches!(code, KeyCode::ArrowLeft | KeyCode::ArrowRight))
                        {
                            let action = self
                                .ui
                                .as_mut()
                                .unwrap()
                                .pause_key(code, self.inventory_shift);
                            self.pause_action(action, event_loop);
                        }
                        return;
                    }
                    if pressed
                        && !event.repeat
                        && matches!(code, KeyCode::KeyT | KeyCode::Slash)
                        && self.play.is_some()
                        && let Some(ui) = &mut self.ui
                    {
                        ui.open_chat(code == KeyCode::Slash);
                        self.input.release_for_screen(self.video);
                        self.submit_movement();
                        self.capture(false);
                        if let Some(window) = &self.window {
                            window.set_ime_allowed(true);
                        }
                        crate::update_chat_ime_area(&self.window, &self.ui);
                        return;
                    }
                    if code == KeyCode::F1 && pressed && !event.repeat {
                        if self.play.is_some()
                            && let Some(ui) = &mut self.ui
                        {
                            ui.toggle_hud();
                        }
                        return;
                    }
                    if code == KeyCode::Escape && pressed && !event.repeat {
                        if let Some(ui) = &mut self.ui {
                            let paused = ui.toggle_pause();
                            self.input.release_for_screen(self.video);
                            self.submit_movement();
                            self.capture(!paused);
                            self.last_update = Instant::now();
                        } else {
                            self.capture(false);
                        }
                        return;
                    }
                    if pressed && code == KeyCode::KeyQ && self.captured {
                        if let Some(play) = &mut self.play {
                            play.session.submit_command(PlayerCommand::DropSelected {
                                all: self.inventory_control,
                            });
                        }
                        return;
                    }
                    if pressed && !event.repeat && code == KeyCode::KeyF && self.captured {
                        if let Some(play) = &mut self.play {
                            play.session.submit_command(PlayerCommand::SwapHands);
                        }
                        return;
                    }
                    match code {
                        KeyCode::KeyW => self.input.forward = pressed,
                        KeyCode::KeyS => self.input.back = pressed,
                        KeyCode::KeyA => self.input.left = pressed,
                        KeyCode::KeyD => self.input.right = pressed,
                        KeyCode::Space => self.input.up = pressed,
                        KeyCode::ShiftLeft => self.input.down.set(
                            true,
                            pressed,
                            event.repeat,
                            self.play.is_some() && self.video.toggle_sneak,
                        ),
                        KeyCode::ShiftRight => self.input.down.set(
                            false,
                            pressed,
                            event.repeat,
                            self.play.is_some() && self.video.toggle_sneak,
                        ),
                        KeyCode::ControlLeft => self.input.fast.set(
                            true,
                            pressed,
                            event.repeat,
                            self.play.is_some() && self.video.toggle_sprint,
                        ),
                        KeyCode::ControlRight => self.input.fast.set(
                            false,
                            pressed,
                            event.repeat,
                            self.play.is_some() && self.video.toggle_sprint,
                        ),
                        code if pressed && !event.repeat && hotbar_key(code).is_some() => {
                            if let Some(play) = &mut self.play {
                                play.session.submit_command(PlayerCommand::SelectSlot(
                                    hotbar_key(code).unwrap(),
                                ));
                            }
                        }
                        _ => {}
                    }
                    self.submit_movement();
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button,
                ..
            } => {
                if let Some(ui) = &mut self.ui
                    && ui.chat_open()
                {
                    if button == MouseButton::Left {
                        ui.chat_release();
                    }
                    return;
                }
                if self
                    .play
                    .as_ref()
                    .is_some_and(|play| play.session.snapshot().death.is_some())
                {
                    return;
                }
                let inventory_open = self
                    .play
                    .as_ref()
                    .is_some_and(|play| play.session.snapshot().inventory.is_open());
                if button == MouseButton::Left && self.ui.as_ref().is_some_and(GameUi::paused) {
                    if let Some(ui) = &mut self.ui {
                        ui.pause_release();
                    }
                } else if inventory_open {
                    let button = match button {
                        MouseButton::Left => Some(ClickButton::Primary),
                        MouseButton::Right => Some(ClickButton::Secondary),
                        _ => None,
                    };
                    if let Some(button) = button
                        && let Some(target) =
                            self.ui.as_mut().and_then(GameUi::inventory_pointer_release)
                        && let Some(play) = &mut self.play
                    {
                        play.session.submit_command(PlayerCommand::InventoryPointer(
                            InventoryPointerEvent::Release {
                                target,
                                button,
                                shift: self.inventory_shift,
                            },
                        ));
                    }
                } else if self.captured
                    && !self.ui.as_ref().is_some_and(GameUi::paused)
                    && let Some(play) = &mut self.play
                {
                    match button {
                        MouseButton::Left => self.mouse_bindings.attack(
                            &mut play.session,
                            false,
                            self.video.toggle_attack,
                        ),
                        MouseButton::Right => self.mouse_bindings.use_item(
                            &mut play.session,
                            false,
                            self.video.toggle_use,
                        ),
                        _ => {}
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } => {
                if let Some(ui) = &mut self.ui
                    && ui.chat_open()
                {
                    if button == MouseButton::Left {
                        crate::handle_chat_click(ui, self.inventory_shift, self.window.as_deref());
                    }
                    crate::update_chat_ime_area(&self.window, &self.ui);
                    return;
                }
                if let Some(snapshot) = self.play.as_ref().map(|play| play.session.snapshot())
                    && snapshot.death.is_some()
                {
                    if button == MouseButton::Left
                        && let Some(ui) = &mut self.ui
                    {
                        ui.sync_death(&snapshot);
                        let action = ui.death_click();
                        self.death_action(action, event_loop);
                    }
                    return;
                }
                if self.inventory_wait.is_some() {
                    return;
                }
                let inventory_open = self
                    .play
                    .as_ref()
                    .is_some_and(|play| play.session.snapshot().inventory.is_open());
                if inventory_open {
                    if self
                        .ui
                        .as_ref()
                        .is_some_and(GameUi::inventory_pointer_active)
                    {
                        return;
                    }
                    if let (Some(play), Some(button)) = (
                        &mut self.play,
                        match button {
                            MouseButton::Left => Some(ClickButton::Primary),
                            MouseButton::Right => Some(ClickButton::Secondary),
                            _ => None,
                        },
                    ) {
                        let snapshot = play.session.snapshot();
                        let action = self
                            .ui
                            .as_mut()
                            .map(|ui| {
                                ui.recipe_book_click(
                                    button,
                                    self.inventory_shift,
                                    &snapshot.inventory,
                                )
                            })
                            .unwrap_or(BookAction::None);
                        match action {
                            BookAction::None => {}
                            BookAction::Consumed => return,
                            BookAction::Settings { open, filtering } => {
                                play.session
                                    .submit_command(PlayerCommand::RecipeBookSettings {
                                        open,
                                        filtering,
                                    });
                                return;
                            }
                            BookAction::Place {
                                recipe,
                                use_max,
                                close,
                                filtering,
                            } => {
                                play.session.submit_command(PlayerCommand::RecipeBookPlace {
                                    recipe,
                                    use_max,
                                });
                                if close {
                                    play.session.submit_command(
                                        PlayerCommand::RecipeBookSettings {
                                            open: false,
                                            filtering,
                                        },
                                    );
                                }
                                return;
                            }
                        }
                        if let Some(target) = self
                            .ui
                            .as_mut()
                            .and_then(|ui| ui.inventory_pointer_press(button))
                        {
                            play.session.submit_command(PlayerCommand::InventoryPointer(
                                InventoryPointerEvent::Press {
                                    target,
                                    button,
                                    shift: self.inventory_shift,
                                },
                            ));
                        }
                    }
                } else if self.ui.as_ref().is_some_and(GameUi::paused) {
                    if button == MouseButton::Left {
                        let action = self
                            .ui
                            .as_mut()
                            .map(GameUi::click)
                            .unwrap_or(PauseAction::None);
                        self.pause_action(action, event_loop);
                    }
                } else if !self.captured {
                    if button == MouseButton::Left {
                        self.capture(true);
                    }
                } else if let Some(play) = &mut self.play {
                    match button {
                        MouseButton::Left => self.mouse_bindings.attack(
                            &mut play.session,
                            true,
                            self.video.toggle_attack,
                        ),
                        MouseButton::Middle => {
                            play.session.submit_command(PlayerCommand::PickBlock)
                        }
                        MouseButton::Right => {
                            self.mouse_bindings.use_item(
                                &mut play.session,
                                true,
                                self.video.toggle_use,
                            );
                        }
                        _ => {}
                    }
                }
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: DeviceId,
        event: DeviceEvent,
    ) {
        if self.captured
            && let DeviceEvent::MouseMotion { delta } = event
        {
            let now = Instant::now();
            if self.inactivity.input(now, self.video.inactivity_fps_limit) {
                self.frame_pacer.set_limit(self.video.max_fps, now);
            }
            if let Some(play) = &mut self.play {
                let (yaw, pitch) = crate::mouse_look::look_delta(delta, self.video);
                let snapshot = play.session.snapshot();
                self.visual_look.turn(&snapshot, yaw, pitch);
                (self.camera.yaw, self.camera.pitch) = self.visual_look.angles(&snapshot);
                play.session
                    .submit_command(PlayerCommand::Look { yaw, pitch });
            } else {
                let (yaw, pitch) = crate::mouse_look::look_delta(delta, self.video);
                self.camera.yaw += yaw;
                self.camera.pitch = (self.camera.pitch + pitch)
                    .clamp(-native_session::MAX_PITCH, native_session::MAX_PITCH);
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if self.ui.is_some()
            && self
                .focus_pause
                .should_pause(now, false, self.video.pause_on_lost_focus)
            && let Some(play) = &self.play
        {
            let screen_open = self
                .ui
                .as_ref()
                .is_some_and(|ui| ui.paused() || ui.chat_open())
                || self.inventory_wait.is_some()
                || {
                    let snapshot = play.session.snapshot();
                    snapshot.inventory.is_open() || snapshot.death.is_some()
                };
            if self
                .focus_pause
                .should_pause(now, screen_open, self.video.pause_on_lost_focus)
            {
                self.ui.as_mut().unwrap().toggle_pause();
                self.capture(false);
            }
        }
        if !self.ui.as_ref().is_some_and(GameUi::paused)
            && let Err(error) = self.update(now.saturating_duration_since(self.last_update))
        {
            self.fail(event_loop, error);
            return;
        }
        self.last_update = now;
        if let Some(window) = &self.window {
            let size = window.inner_size();
            if self.occluded || size.width == 0 || size.height == 0 {
                event_loop
                    .set_control_flow(ControlFlow::WaitUntil(now + Duration::from_millis(50)));
            } else {
                let fps = self.inactivity.limit(
                    now,
                    self.video.inactivity_fps_limit,
                    self.video.max_fps,
                    false,
                    false,
                );
                self.frame_pacer.set_limit(fps, now);
                if self.frame_pacer.due(now) {
                    window.request_redraw();
                }
                if let Some(deadline) = self.frame_pacer.deadline() {
                    event_loop.set_control_flow(ControlFlow::WaitUntil(
                        deadline.min(now + Duration::from_millis(50)),
                    ));
                } else {
                    event_loop.set_control_flow(ControlFlow::Poll);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sun_angle_interpolation_wraps_and_snaps_large_changes() {
        assert_eq!(super::interpolate_sun_angle(359.0, 1.0, 0.5), 360.0);
        assert_eq!(super::interpolate_sun_angle(1.0, 359.0, 0.5), 0.0);
        assert_eq!(super::interpolate_sun_angle(0.0, 90.0, 0.5), 90.0);
    }
    #[test]
    fn mesh_results_obey_world_revision_and_current_biome_blend() {
        use super::obsolete_mesh;
        assert!(!obsolete_mesh(5, 3, 5, 2, 3));
        assert!(obsolete_mesh(5, 2, 5, 2, 3));
        assert!(obsolete_mesh(4, 3, 5, 2, 3));
        assert!(obsolete_mesh(5, 2, 5, 2, 2));
        assert!(!obsolete_mesh(6, 2, 5, 2, 2));
    }
    use super::*;

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 assets and a Windows GPU window"]
    fn incremental_worker_updates_the_real_renderer_and_rebuilds_changed_atlas() {
        if !crate::window_test::isolated(
            "preview_app::tests::incremental_worker_updates_the_real_renderer_and_rebuilds_changed_atlas",
        ) {
            return;
        }
        use native_world::loaded::{BLOCK_COUNT, BlockState, Chunk, ChunkPos, Section, World};
        use std::collections::BTreeMap;
        use winit::platform::windows::EventLoopBuilderExtWindows;

        struct Harness {
            world: World,
            stone: native_world::loaded::BlockStateId,
            dirt: native_world::loaded::BlockStateId,
            item: native_session::items::DroppedItem,
            scene: PreviewScene,
            worker: MeshWorker,
            window: Option<Arc<Window>>,
            renderer: Option<Renderer>,
            last_generation: Option<u64>,
            removed: Option<Arc<Chunk>>,
            revision: u64,
            started: Instant,
            rendered_final: bool,
        }
        impl ApplicationHandler for Harness {
            fn resumed(&mut self, event_loop: &ActiveEventLoop) {
                let window = Arc::new(
                    event_loop
                        .create_window(
                            Window::default_attributes()
                                .with_title("Minecraft mesh test")
                                .with_inner_size(winit::dpi::LogicalSize::new(320, 240)),
                        )
                        .unwrap(),
                );
                let item_texture =
                    image::RgbaImage::from_pixel(16, 16, image::Rgba([255, 0, 0, 255]));
                self.renderer = Some(
                    Renderer::new(
                        Arc::clone(&window),
                        &self.scene.atlas.texture,
                        None,
                        Some(vec![
                            &item_texture;
                            crate::mesh::DROPPED_TEXTURE_COUNT as usize
                        ]),
                        None,
                        None,
                        false,
                    )
                    .unwrap(),
                );
                self.window = Some(window);
                self.world.set_block_id(9, 1, 8, self.stone).unwrap();
                self.worker.submit(MeshJob {
                    revision: 1,
                    biome_blend: 0,
                    origin: glam::IVec3::ZERO,
                    world: Arc::new(self.world.clone()),
                });
            }
            fn window_event(
                &mut self,
                event_loop: &ActiveEventLoop,
                _id: WindowId,
                event: WindowEvent,
            ) {
                if matches!(event, WindowEvent::RedrawRequested) {
                    self.renderer
                        .as_mut()
                        .unwrap()
                        .render_preview(
                            &mut self.scene,
                            Vec3::new(8.0, 5.0, 15.0),
                            Vec3::NEG_Z,
                            30,
                            None,
                            std::slice::from_ref(&self.item),
                        )
                        .unwrap();
                    if self.revision == 7 {
                        self.rendered_final = true;
                        event_loop.exit();
                    }
                }
            }
            fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
                assert!(
                    self.started.elapsed() < Duration::from_secs(30),
                    "mesh worker timed out"
                );
                if let Some(result) = self.worker.try_result().unwrap() {
                    assert_eq!(result.revision, self.revision + 1);
                    let next = result.scene.unwrap();
                    let generation = next.material_generation.unwrap();
                    let previous_atlas = Arc::clone(&self.scene.atlas);
                    let before = self.renderer.as_ref().unwrap().preview_upload_counts();
                    match result.revision {
                        1 => assert_eq!(self.scene.material_generation, None),
                        2 => assert_eq!(Some(generation), self.last_generation),
                        3 => assert_ne!(Some(generation), self.last_generation),
                        4..=7 => assert_eq!(Some(generation), self.last_generation),
                        _ => unreachable!(),
                    }
                    replace_scene(self.renderer.as_mut().unwrap(), &mut self.scene, next).unwrap();
                    let renderer = self.renderer.as_mut().unwrap();
                    let eye = Vec3::new(8.0, 5.0, 15.0);
                    renderer
                        .render_preview(
                            &mut self.scene,
                            eye,
                            Vec3::NEG_Z,
                            30,
                            None,
                            std::slice::from_ref(&self.item),
                        )
                        .unwrap();
                    let after = renderer.preview_upload_counts();
                    match result.revision {
                        1 => assert_eq!((after.0 - before.0, after.1 - before.1), (9, 1)),
                        2 => assert_eq!((after.0 - before.0, after.1 - before.1), (1, 0)),
                        3 => assert_eq!((after.0 - before.0, after.1 - before.1), (9, 1)),
                        4 => assert_eq!((after.0 - before.0, after.1 - before.1), (0, 0)),
                        5 => assert_eq!((after.0 - before.0, after.1 - before.1), (1, 0)),
                        6 => assert_eq!((after.0 - before.0, after.1 - before.1), (1, 0)),
                        7 => assert_eq!((after.0 - before.0, after.1 - before.1), (1, 0)),
                        _ => unreachable!(),
                    }
                    assert_eq!(
                        after.2 - before.2,
                        if matches!(result.revision, 6 | 7) {
                            0
                        } else {
                            1
                        }
                    );
                    renderer
                        .render_preview(
                            &mut self.scene,
                            eye,
                            Vec3::NEG_Z,
                            30,
                            None,
                            std::slice::from_ref(&self.item),
                        )
                        .unwrap();
                    assert_eq!(renderer.preview_upload_counts(), after);
                    assert_eq!(
                        renderer.preview_chunk_count(),
                        if result.revision == 4 { 8 } else { 9 }
                    );
                    if matches!(result.revision, 2 | 4 | 6 | 7) {
                        let chunked = renderer
                            .preview_readback(&self.scene, eye, Vec3::NEG_Z, true)
                            .unwrap();
                        let flat = renderer
                            .preview_readback(&self.scene, eye, Vec3::NEG_Z, false)
                            .unwrap();
                        assert!(
                            chunked == flat,
                            "chunked and flat terrain must produce identical pixels"
                        );
                        let pixels = chunked.as_chunks::<4>().0;
                        assert!(pixels.iter().any(|pixel| pixel != &pixels[0]));
                        if result.revision == 2 {
                            // Looking vertically down must still render the block
                            // under the camera, for every horizontal heading.
                            for yaw in [0.0_f32, 1.1] {
                                renderer.set_view_yaw(yaw);
                                let direction = |pitch: f32| {
                                    Vec3::new(
                                        yaw.sin() * pitch.cos(),
                                        pitch.sin(),
                                        -yaw.cos() * pitch.cos(),
                                    )
                                };
                                let eye = Vec3::new(8.5, 5.0, 8.5);
                                let down = renderer
                                    .preview_readback(
                                        &self.scene,
                                        eye,
                                        direction(-native_session::MAX_PITCH),
                                        true,
                                    )
                                    .unwrap();
                                let up = renderer
                                    .preview_readback(
                                        &self.scene,
                                        eye,
                                        direction(native_session::MAX_PITCH),
                                        true,
                                    )
                                    .unwrap();
                                let center = (120 * 320 + 160) * 4;
                                assert_ne!(&down[center..center + 4], &up[center..center + 4]);
                            }
                            renderer.set_view_yaw(0.0);
                            let original_lightmap = Arc::clone(&self.scene.lightmap);
                            let bake = |vertex: &mut crate::mesh::Vertex| {
                                if vertex.light < 256 {
                                    let light = original_lightmap[(vertex.light / 16) as usize]
                                        [(vertex.light % 16) as usize];
                                    for (tint, value) in vertex.tint.iter_mut().zip(light) {
                                        *tint *= value;
                                    }
                                    vertex.light = u32::MAX;
                                }
                            };
                            let chunks = self
                                .scene
                                .chunks
                                .iter()
                                .map(|(pos, mesh)| {
                                    let mut opaque = mesh.opaque.to_vec();
                                    for vertex in &mut opaque {
                                        bake(vertex);
                                    }
                                    let mut translucent = mesh.translucent.to_vec();
                                    for quad in &mut translucent {
                                        for vertex in &mut quad.vertices {
                                            bake(vertex);
                                        }
                                    }
                                    (
                                        *pos,
                                        Arc::new(crate::preview::ChunkMesh {
                                            opaque: opaque.into(),
                                            translucent: translucent.into(),
                                            chest_lids: Arc::from([]),
                                            banner_flags: Arc::from([]),
                                        }),
                                    )
                                })
                                .collect();
                            let baked = PreviewScene {
                                atlas: Arc::clone(&self.scene.atlas),
                                chunks,
                                origin: self.scene.origin,
                                warnings: Vec::new(),
                                material_generation: self.scene.material_generation,
                                lightmap: Arc::clone(&original_lightmap),
                                base_lightmap: Arc::clone(&self.scene.base_lightmap),
                                biome_colors: Arc::clone(&self.scene.biome_colors),
                            };
                            let old_path = renderer
                                .preview_readback(&baked, eye, Vec3::NEG_Z, false)
                                .unwrap();
                            assert!(
                                old_path == flat,
                                "GPU lightmap must match the previous CPU-baked pixels"
                            );
                            self.scene.lightmap = Arc::new([[[0.0; 3]; 16]; 16]);
                            renderer.set_preview_lightmap(&self.scene.lightmap);
                            let dark = renderer
                                .preview_readback(&self.scene, eye, Vec3::NEG_Z, true)
                                .unwrap();
                            assert!(
                                dark != chunked,
                                "lightmap changes must affect terrain pixels"
                            );
                            assert_eq!(
                                renderer.preview_upload_counts(),
                                after,
                                "changing light must not upload geometry"
                            );
                            self.scene.lightmap = original_lightmap;
                            renderer.set_preview_lightmap(&self.scene.lightmap);
                            assert!(
                                renderer
                                    .preview_readback(&self.scene, eye, Vec3::NEG_Z, true)
                                    .unwrap()
                                    == chunked
                            );
                            let game = native_resources::default_minecraft_dir().unwrap();
                            let mut resources = ResourceStack::open(&game).unwrap();
                            let daylight = crate::preview_lighting::SkyTimeline::load(
                                &mut resources,
                                "minecraft:overworld",
                            )
                            .unwrap();
                            let noon = daylight
                                .sample(&self.scene.base_lightmap, |_| Some(6000))
                                .unwrap()
                                .table();
                            let night = daylight
                                .sample(&self.scene.base_lightmap, |_| Some(18000))
                                .unwrap()
                                .table();
                            renderer.set_preview_lightmap(&noon);
                            let noon_pixels = renderer
                                .preview_readback(&self.scene, eye, Vec3::NEG_Z, true)
                                .unwrap();
                            renderer.set_preview_lightmap(&night);
                            let night_pixels = renderer
                                .preview_readback(&self.scene, eye, Vec3::NEG_Z, true)
                                .unwrap();
                            assert!(
                                noon_pixels != night_pixels,
                                "installed day timeline must change terrain lighting"
                            );
                            assert_eq!(
                                renderer.preview_upload_counts(),
                                after,
                                "daylight must not upload terrain geometry"
                            );
                            renderer.set_preview_lightmap(&self.scene.lightmap);
                            renderer.set_sky_color([0.0, 0.0, 0.0]);
                            let dark_sky = renderer
                                .preview_readback(&self.scene, eye, Vec3::NEG_Z, true)
                                .unwrap();
                            renderer.set_sky_color([0.2, 0.4, 0.8]);
                            let blue_sky = renderer
                                .preview_readback(&self.scene, eye, Vec3::NEG_Z, true)
                                .unwrap();
                            assert!(
                                dark_sky[..4] != blue_sky[..4],
                                "sky background color must reach the GPU"
                            );
                            let expected: [u8; 3] = match renderer.configured_color_format() {
                                wgpu::TextureFormat::Bgra8Unorm
                                | wgpu::TextureFormat::Bgra8UnormSrgb => [204, 102, 51],
                                _ => [51, 102, 204],
                            };
                            for (actual, expected) in blue_sky[..3].iter().zip(expected) {
                                assert!(
                                    (i16::from(*actual) - i16::from(expected)).abs() <= 1,
                                    "sky clear color must preserve RGB values"
                                );
                            }
                            renderer.set_fog_range(Some([0.0, 1.0]));
                            let fogged = renderer
                                .preview_readback(&self.scene, eye, Vec3::NEG_Z, true)
                                .unwrap();
                            assert!(
                                blue_sky
                                    .as_chunks::<4>()
                                    .0
                                    .iter()
                                    .zip(fogged.as_chunks::<4>().0)
                                    .any(|(before, after)| {
                                        before[..3] != blue_sky[..3]
                                            && after[..3].iter().zip(expected).all(|(a, b)| {
                                                (i16::from(*a) - i16::from(b)).abs() <= 1
                                            })
                                    }),
                                "distant terrain must be fogged toward the background color"
                            );
                            renderer.set_fog_range(Some([0.0, 0.0]));
                            let step_fog = renderer
                                .preview_readback(&self.scene, eye, Vec3::NEG_Z, true)
                                .unwrap();
                            assert_eq!(
                                step_fog, fogged,
                                "equal fog distances must produce a defined step"
                            );
                            renderer.set_fog_range(None);
                            assert_eq!(
                                renderer
                                    .preview_readback(&self.scene, eye, Vec3::NEG_Z, true)
                                    .unwrap(),
                                blue_sky,
                                "disabling fog restores terrain"
                            );
                            assert_eq!(
                                renderer.preview_upload_counts(),
                                after,
                                "sky color must not upload terrain geometry"
                            );
                            renderer.reset_sky_color();
                            renderer.set_fov(30);
                            let narrow = renderer
                                .preview_readback(&self.scene, eye, Vec3::NEG_Z, true)
                                .unwrap();
                            renderer.set_fov(110);
                            let wide = renderer
                                .preview_readback(&self.scene, eye, Vec3::NEG_Z, true)
                                .unwrap();
                            assert!(narrow != wide, "FOV must affect rendered world pixels");
                            renderer.set_fov(70);
                            let mut bob =
                                native_session::camera::CameraMotion::new(glam::DVec3::ZERO, 1.62);
                            for _ in 0..10 {
                                bob.begin_bob_tick(glam::DVec3::new(0.1, 0.0, 0.1), true);
                                bob.finish_bob_tick(glam::DVec3::new(0.1, 0.0, 0.1));
                            }
                            renderer.set_camera_bob(&bob, 0.5);
                            let moving = renderer
                                .preview_readback(&self.scene, eye, Vec3::NEG_Z, true)
                                .unwrap();
                            assert!(
                                moving != chunked,
                                "walking bob must change rendered world pixels"
                            );
                            renderer.set_view_bobbing(false);
                            let disabled = renderer
                                .preview_readback(&self.scene, eye, Vec3::NEG_Z, true)
                                .unwrap();
                            assert!(
                                disabled == chunked,
                                "disabling bob must restore the original view"
                            );
                            renderer.set_camera_bob(
                                &native_session::camera::CameraMotion::new(glam::DVec3::ZERO, 1.62),
                                0.0,
                            );
                            renderer.set_view_bobbing(true);
                            renderer
                                .render_preview(&mut self.scene, eye, Vec3::NEG_Z, 30, None, &[])
                                .unwrap();
                            let without_item = renderer
                                .preview_readback(&self.scene, eye, Vec3::NEG_Z, true)
                                .unwrap();
                            assert!(chunked != without_item, "the dropped item must be visible");
                            let before_lid = renderer.preview_upload_counts();
                            renderer.set_chest_lids(
                                Arc::from([native_session::chest_lid::ChestLidVisual {
                                    position: glam::IVec3::new(7, 4, 8),
                                    previous: 1.0,
                                    current: 1.0,
                                }]),
                                0.5,
                            );
                            renderer
                                .render_preview(&mut self.scene, eye, Vec3::NEG_Z, 30, None, &[])
                                .unwrap();
                            let opened = renderer
                                .preview_readback(&self.scene, eye, Vec3::NEG_Z, true)
                                .unwrap();
                            assert!(
                                opened != without_item,
                                "opening a chest must change rendered pixels"
                            );
                            assert_eq!(
                                renderer.preview_upload_counts(),
                                before_lid,
                                "lid animation must not reupload a whole chunk"
                            );
                            renderer.set_chest_lids(Arc::from([]), 0.0);
                            renderer
                                .render_preview(&mut self.scene, eye, Vec3::NEG_Z, 30, None, &[])
                                .unwrap();
                            let closed = renderer
                                .preview_readback(&self.scene, eye, Vec3::NEG_Z, true)
                                .unwrap();
                            assert_eq!(
                                closed, without_item,
                                "closing must restore the exact original mesh"
                            );
                        }
                    }
                    if result.revision == 2 {
                        renderer
                            .render_preview(
                                &mut self.scene,
                                eye + Vec3::X,
                                Vec3::NEG_Z,
                                30,
                                None,
                                &[],
                            )
                            .unwrap();
                        assert_eq!(
                            renderer.preview_upload_counts(),
                            after,
                            "unchanged translucent ordering must reuse its GPU buffer"
                        );
                        let reused = renderer
                            .preview_readback(&self.scene, eye + Vec3::X, Vec3::NEG_Z, true)
                            .unwrap();
                        let repeated = renderer
                            .preview_readback(&self.scene, eye + Vec3::X, Vec3::NEG_Z, true)
                            .unwrap();
                        assert!(
                            reused == repeated,
                            "unchanged render must preserve every pixel: {:?}",
                            reused
                                .chunks_exact(4)
                                .zip(repeated.chunks_exact(4))
                                .enumerate()
                                .filter(|(_, (a, b))| a != b)
                                .take(8)
                                .collect::<Vec<_>>()
                        );
                        let previous_vertices = renderer.translucent_upload_bytes().to_vec();
                        let previous_atlas_pixels = self.scene.atlas.texture.rgba.clone();
                        renderer.force_translucent_resort();
                        renderer
                            .render_preview(
                                &mut self.scene,
                                eye + Vec3::X,
                                Vec3::NEG_Z,
                                30,
                                None,
                                &[],
                            )
                            .unwrap();
                        assert_eq!(
                            renderer.preview_upload_counts(),
                            (after.0, after.1 + 1, after.2),
                            "the forced rebuild must upload translucent geometry"
                        );
                        assert!(
                            previous_vertices == renderer.translucent_upload_bytes(),
                            "reused and freshly sorted translucent vertices differ"
                        );
                        assert!(
                            previous_atlas_pixels == self.scene.atlas.texture.rgba,
                            "atlas changed during identical-tick resort"
                        );
                        let resorted = renderer
                            .preview_readback(&self.scene, eye + Vec3::X, Vec3::NEG_Z, true)
                            .unwrap();
                        assert!(
                            reused == resorted,
                            "reusing the sort must preserve every pixel: {:?}",
                            reused
                                .chunks_exact(4)
                                .zip(resorted.chunks_exact(4))
                                .enumerate()
                                .filter(|(_, (a, b))| a != b)
                                .take(8)
                                .collect::<Vec<_>>()
                        );
                        renderer
                            .render_preview(
                                &mut self.scene,
                                eye,
                                Vec3::NEG_Z,
                                30,
                                None,
                                std::slice::from_ref(&self.item),
                            )
                            .unwrap();
                    }
                    if matches!(result.revision, 2 | 4..=7) {
                        assert!(Arc::ptr_eq(&previous_atlas, &self.scene.atlas));
                    } else if result.revision == 3 {
                        assert!(!Arc::ptr_eq(&previous_atlas, &self.scene.atlas));
                    }
                    self.last_generation = Some(generation);
                    self.revision = result.revision;
                    if self.revision < 7 {
                        match self.revision {
                            1 => {
                                self.world.set_block_id(10, 1, 8, self.stone).unwrap();
                            }
                            2 => {
                                self.world.set_block_id(10, 1, 8, self.dirt).unwrap();
                                self.world.set_block_id(7, 1, 8, self.dirt).unwrap();
                            }
                            3 => {
                                self.removed = self.world.chunks.remove(&ChunkPos { x: 1, z: 1 });
                            }
                            4 => {
                                let chunk = self.removed.take().unwrap();
                                self.world.chunks.insert(chunk.position, chunk);
                            }
                            5 => {
                                self.world.set_block_id(10, 1, 8, self.stone).unwrap();
                            }
                            6 => {
                                // The last dirt block disappears; retained materials
                                // must not force every other chunk to re-upload.
                                self.world.set_block_id(7, 1, 8, self.stone).unwrap();
                            }
                            _ => unreachable!(),
                        }
                        self.worker.submit(MeshJob {
                            revision: self.revision + 1,
                            biome_blend: 0,
                            origin: glam::IVec3::ZERO,
                            world: Arc::new(self.world.clone()),
                        });
                    }
                }
                self.window.as_ref().unwrap().request_redraw();
                event_loop.set_control_flow(ControlFlow::WaitUntil(
                    Instant::now() + Duration::from_millis(10),
                ));
            }
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = ResourceStack::open(&game).unwrap();
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(BlockState {
            name: "minecraft:air".into(),
            properties: BTreeMap::new(),
        });
        let stone = world.intern_state(BlockState {
            name: "minecraft:stone".into(),
            properties: BTreeMap::new(),
        });
        let dirt = world.intern_state(BlockState {
            name: "minecraft:dirt".into(),
            properties: BTreeMap::new(),
        });
        let water = world.intern_state(BlockState {
            name: "minecraft:water".into(),
            properties: BTreeMap::from([("level".into(), "0".into())]),
        });
        let chest = world.intern_state(BlockState {
            name: "minecraft:chest".into(),
            properties: BTreeMap::from([
                ("facing".into(), "south".into()),
                ("type".into(), "single".into()),
                ("waterlogged".into(), "false".into()),
            ]),
        });
        for x in -1..=1 {
            for z in -1..=1 {
                let mut blocks = Box::new([air; BLOCK_COUNT]);
                blocks[8 + 8 * 16] = stone;
                if x == 0 && z == 0 {
                    blocks[2 + 2 * 16 + 256] = water;
                    blocks[7 + 8 * 16 + 4 * 256] = chest;
                }
                let pos = ChunkPos { x, z };
                world.chunks.insert(
                    pos,
                    Arc::new(Chunk {
                        block_light_correct: false,
                        position: pos,
                        status: Some("minecraft:full".into()),
                        sections: BTreeMap::from([(
                            0,
                            Arc::new(Section {
                                blocks: Some(blocks),
                                biomes: None,
                                block_light: None,
                                sky_light: None,
                            }),
                        )]),
                    }),
                );
            }
        }
        let scene = PreviewScene::prepare(&world, &mut resources, glam::IVec3::ZERO, 4096).unwrap();
        let worker = MeshWorker::start(resources).unwrap();
        let mut harness = Harness {
            world,
            stone,
            dirt,
            item: native_session::items::DroppedItem {
                entity_id: 0,
                item_age: 0,
                position: glam::DVec3::new(8.0, 4.8, 11.0),
                previous_position: glam::DVec3::new(8.0, 4.8, 11.0),
                stack: native_session::inventory::ItemStack::new(
                    "minecraft:stone".into(),
                    1,
                    64,
                    native_persistence::nbt::Tag::Compound(vec![]),
                )
                .unwrap(),
                age: 0,
                bob_offset: 0.0,
            },
            scene,
            worker,
            window: None,
            renderer: None,
            last_generation: None,
            removed: None,
            revision: 0,
            started: Instant::now(),
            rendered_final: false,
        };
        let mut builder = EventLoop::builder();
        builder.with_any_thread(true);
        builder.build().unwrap().run_app(&mut harness).unwrap();
        assert!(harness.rendered_final);
    }

    #[test]
    fn camera_remains_in_the_old_mesh_frame_during_origin_changes() {
        let previous = native_session::LocalSession::demo().snapshot();
        let mut player = previous.player;
        player.position = Vec3::new(0.5, 2.0, 8.5);
        let snapshot = WorldSnapshot {
            world: previous.world.clone(),
            world_spawn: previous.world_spawn.clone(),
            last_death: previous.last_death.clone(),
            player,
            tooltip_attack_base: previous.tooltip_attack_base,
            camera: native_session::camera::CameraMotion::new(
                player.position.as_dvec3(),
                player.pose.eye_height(),
            ),
            game_mode: previous.game_mode,
            vitals: previous.vitals,
            hunger_effect: previous.hunger_effect,
            poison_effect: previous.poison_effect,
            active_effects: Arc::clone(&previous.active_effects),
            night_vision_duration: previous.night_vision_duration,
            experience: previous.experience,
            death: previous.death,
            armor: previous.armor,
            eyes_in_water: previous.eyes_in_water,
            water_vision: previous.water_vision,
            sprinting: previous.sprinting,
            selected_block: previous.selected_block,
            inventory: Arc::clone(&previous.inventory),
            chest_lids: Arc::clone(&previous.chest_lids),
            first_person: previous.first_person.clone(),
            breaking: previous.breaking,
            dropped_items: Arc::clone(&previous.dropped_items),
            falling_blocks: Arc::clone(&previous.falling_blocks),
            experience_orbs: Arc::clone(&previous.experience_orbs),
            system_messages: Arc::clone(&previous.system_messages),
            locked_chest_message: previous.locked_chest_message.clone(),
            weather: previous.weather,
            weather_terrain: previous.weather_terrain.clone(),
            game_time: previous.game_time,
            tick: previous.tick,
            origin: glam::IVec3::new(16_000_016, 0, -16_000_000),
            world_revision: previous.world_revision,
        };
        let eye = eye_for_scene(&snapshot, glam::IVec3::new(16_000_000, 0, -16_000_000), 1.0);
        assert!((eye - Vec3::new(16.5, 2.0 + native_session::EYE_HEIGHT, 8.5)).length() < 1.0e-6);
        let mut crouched = snapshot;
        crouched.player.pose = native_session::PlayerPose::Crouching;
        crouched.camera.tick(
            crouched.player.position.as_dvec3(),
            crouched.player.pose.eye_height(),
            false,
            false,
            1.0,
        );
        let eye = eye_for_scene(&crouched, glam::IVec3::new(16_000_000, 0, -16_000_000), 1.0);
        assert!(
            (eye - Vec3::new(
                16.5,
                2.0 + (native_session::EYE_HEIGHT + native_session::CROUCH_EYE_HEIGHT) * 0.5,
                8.5
            ))
            .length()
                < 1.0e-6
        );
    }

    #[test]
    fn free_camera_moves_in_view_direction_and_normalizes_diagonals() {
        let mut camera = Camera {
            eye: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
        };
        let mut input = Input {
            forward: true,
            ..Input::default()
        };
        camera.advance(&input, Duration::from_millis(50));
        assert!((camera.eye - Vec3::new(0.0, 0.0, -0.5)).length() < 1e-6);
        input.right = true;
        input.up = true;
        let before = camera.eye;
        camera.advance(&input, Duration::from_millis(50));
        assert!(((camera.eye - before).length() - 0.5).abs() < 1e-6);
        camera.yaw = std::f32::consts::FRAC_PI_2;
        input = Input {
            forward: true,
            ..Input::default()
        };
        input.fast.set(true, true, false, false);
        let before = camera.eye;
        camera.advance(&input, Duration::from_millis(50));
        assert!((camera.eye - before - Vec3::new(2.0, 0.0, 0.0)).length() < 1e-6);
    }
}
