use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Bit-preserving unit interval; this keeps slider values exact while settings
/// remain comparable and serialized as ordinary JSON numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnitInterval(u64);

impl UnitInterval {
    pub fn new(value: f64) -> Self {
        Self(
            if value.is_finite() {
                value.clamp(0.0, 1.0)
            } else {
                0.0
            }
            .to_bits(),
        )
    }

    pub fn value(self) -> f64 {
        f64::from_bits(self.0)
    }

    fn valid_or(self, fallback: f64) -> Self {
        let value = self.value();
        if value.is_finite() && (0.0..=1.0).contains(&value) {
            self
        } else {
            Self::new(fallback)
        }
    }
}

/// Minecraft maps an integer range of -200..=100 through 10^(value / 100).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseWheelSensitivity(i16);

impl MouseWheelSensitivity {
    pub fn new(value: f64) -> Self {
        let value = if value.is_finite() {
            value.clamp(0.01, 10.0)
        } else {
            1.0
        };
        let step = (value.log10() * 100.0).floor().clamp(-200.0, 100.0);
        Self(step as i16)
    }

    pub fn from_slider_fraction(fraction: f64) -> Self {
        let fraction = if fraction.is_finite() {
            fraction.clamp(0.0, 0.999_989_986_419_677_7)
        } else {
            0.0
        };
        let step = (-200.0 + fraction * 301.0).floor().clamp(-200.0, 100.0);
        Self(step as i16)
    }

    pub fn slider_fraction(self) -> f64 {
        if self.0 <= -200 {
            0.0
        } else if self.0 >= 100 {
            1.0
        } else {
            (f64::from(self.0 + 200) + 0.5) / 301.0
        }
    }

    pub fn value(self) -> f64 {
        10.0_f64.powf(f64::from(self.0) / 100.0)
    }

    fn valid_or(self, fallback: f64) -> Self {
        if (-200..=100).contains(&self.0) {
            self
        } else {
            Self::new(fallback)
        }
    }
}

impl Default for MouseWheelSensitivity {
    fn default() -> Self {
        Self::new(1.0)
    }
}

impl Serialize for MouseWheelSensitivity {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(self.value())
    }
}

impl<'de> Deserialize<'de> for MouseWheelSensitivity {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = f64::deserialize(deserializer)?;
        if value.is_finite() && (0.01..=10.0).contains(&value) {
            let step = (value.log10() * 100.0).round().clamp(-200.0, 100.0);
            Ok(Self(step as i16))
        } else {
            Ok(Self::default())
        }
    }
}

impl Serialize for UnitInterval {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(self.value())
    }
}

impl<'de> Deserialize<'de> for UnitInterval {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self(f64::deserialize(deserializer)?.to_bits()))
    }
}

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    pub minecraft_dir: Option<PathBuf>,
    pub resource_packs: Vec<PathBuf>,
    pub video: VideoSettings,
    pub sound: SoundSettings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum SoundCategory {
    Master,
    Music,
    Record,
    Weather,
    Block,
    Hostile,
    Neutral,
    Player,
    Ambient,
    Voice,
    Ui,
}

impl SoundCategory {
    pub const ALL: [Self; 11] = [
        Self::Master,
        Self::Music,
        Self::Record,
        Self::Weather,
        Self::Block,
        Self::Hostile,
        Self::Neutral,
        Self::Player,
        Self::Ambient,
        Self::Voice,
        Self::Ui,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Master => "master",
            Self::Music => "music",
            Self::Record => "record",
            Self::Weather => "weather",
            Self::Block => "block",
            Self::Hostile => "hostile",
            Self::Neutral => "neutral",
            Self::Player => "player",
            Self::Ambient => "ambient",
            Self::Voice => "voice",
            Self::Ui => "ui",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct SoundSettings {
    volumes: [UnitInterval; 11],
}

impl Default for SoundSettings {
    fn default() -> Self {
        Self {
            volumes: [UnitInterval::new(1.0); 11],
        }
    }
}

impl SoundSettings {
    pub fn get(self, category: SoundCategory) -> f64 {
        self.volumes[category as usize].value()
    }
    pub fn set(&mut self, category: SoundCategory, value: f64) -> bool {
        let next = UnitInterval::new(value);
        let slot = &mut self.volumes[category as usize];
        let changed = *slot != next;
        *slot = next;
        changed
    }
    pub fn ui_gain(self) -> f32 {
        (self.get(SoundCategory::Master) as f32) * (self.get(SoundCategory::Ui) as f32)
    }
    pub fn block_gain(self) -> f32 {
        (self.get(SoundCategory::Master) as f32) * (self.get(SoundCategory::Block) as f32)
    }
    pub fn player_gain(self) -> f32 {
        (self.get(SoundCategory::Master) as f32) * (self.get(SoundCategory::Player) as f32)
    }
    pub fn neutral_gain(self) -> f32 {
        (self.get(SoundCategory::Master) as f32) * (self.get(SoundCategory::Neutral) as f32)
    }
    fn valid(mut self) -> Self {
        for value in &mut self.volumes {
            *value = value.valid_or(1.0);
        }
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct VideoSettings {
    /// Render distance in chunks; Java's default is 12.
    pub render_distance: u8,
    /// Radius of the square biome-color average, in blocks.
    pub biome_blend: u8,
    pub vsync: bool,
    /// 260 is the vanilla unlimited setting; lower values are frames per second.
    pub max_fps: u16,
    /// Vertical field of view in degrees, 30 through 110 in Java 26.3.
    pub fov: u16,
    /// Walking camera motion, controlled by the accessibility screen.
    pub view_bobbing: bool,
    pub fov_effect_scale: UnitInterval,
    pub glint_speed: UnitInterval,
    pub glint_strength: UnitInterval,
    pub panorama_speed: UnitInterval,
    pub menu_blur: u8,
    pub mouse_sensitivity: UnitInterval,
    pub mouse_wheel_sensitivity: MouseWheelSensitivity,
    pub discrete_mouse_scroll: bool,
    pub invert_mouse_x: bool,
    pub invert_mouse_y: bool,
    /// Ticks allowed between forward taps, from 0 through 10.
    pub sprint_window: u8,
    pub toggle_sneak: bool,
    pub toggle_sprint: bool,
    pub toggle_attack: bool,
    pub toggle_use: bool,
    /// Java's F3+P setting, saved alongside the other client options.
    pub pause_on_lost_focus: bool,
    /// Whether to reduce the frame rate after inactivity, or only while minimized.
    pub inactivity_fps_limit: InactivityFpsLimit,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InactivityFpsLimit {
    #[default]
    Afk,
    Minimized,
}

impl InactivityFpsLimit {
    pub fn next(self) -> Self {
        match self {
            Self::Afk => Self::Minimized,
            Self::Minimized => Self::Afk,
        }
    }
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self {
            render_distance: 12,
            biome_blend: 2,
            vsync: true,
            max_fps: 120,
            fov: 70,
            view_bobbing: true,
            fov_effect_scale: UnitInterval::new(1.0),
            glint_speed: UnitInterval::new(0.5),
            glint_strength: UnitInterval::new(0.75),
            panorama_speed: UnitInterval::new(1.0),
            menu_blur: 5,
            mouse_sensitivity: UnitInterval::new(0.5),
            mouse_wheel_sensitivity: MouseWheelSensitivity::default(),
            discrete_mouse_scroll: false,
            invert_mouse_x: false,
            invert_mouse_y: false,
            sprint_window: 7,
            toggle_sneak: false,
            toggle_sprint: false,
            toggle_attack: false,
            toggle_use: false,
            pause_on_lost_focus: true,
            inactivity_fps_limit: InactivityFpsLimit::Afk,
        }
    }
}

impl VideoSettings {
    fn valid(self) -> Self {
        Self {
            biome_blend: if self.biome_blend <= 7 {
                self.biome_blend
            } else {
                2
            },
            render_distance: if (2..=32).contains(&self.render_distance) {
                self.render_distance
            } else {
                12
            },
            max_fps: if (10..=260).contains(&self.max_fps) {
                self.max_fps
            } else {
                120
            },
            fov: if (30..=110).contains(&self.fov) {
                self.fov
            } else {
                70
            },
            glint_speed: self.glint_speed.valid_or(0.5),
            fov_effect_scale: self.fov_effect_scale.valid_or(1.0),
            glint_strength: self.glint_strength.valid_or(0.75),
            panorama_speed: self.panorama_speed.valid_or(1.0),
            mouse_sensitivity: self.mouse_sensitivity.valid_or(0.5),
            mouse_wheel_sensitivity: self.mouse_wheel_sensitivity.valid_or(1.0),
            menu_blur: if self.menu_blur <= 10 {
                self.menu_blur
            } else {
                5
            },
            sprint_window: if self.sprint_window <= 10 {
                self.sprint_window
            } else {
                7
            },
            ..self
        }
    }
}

pub fn present_mode(vsync: bool) -> wgpu::PresentMode {
    if vsync {
        wgpu::PresentMode::AutoVsync
    } else {
        wgpu::PresentMode::AutoNoVsync
    }
}

fn path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|root| {
        PathBuf::from(root)
            .join("MinecraftNative")
            .join("options.json")
    })
}

pub fn load() -> Settings {
    let mut settings: Settings = path()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_default();
    settings.video = settings.video.valid();
    settings.sound = settings.sound.valid();
    settings
}

pub fn save(settings: &Settings) -> Result<()> {
    let path = path().context("APPDATA is unavailable; cannot save settings")?;
    save_to(&path, settings)
}

pub fn save_video(video: VideoSettings) -> Result<()> {
    let path = path().context("APPDATA is unavailable; cannot save settings")?;
    save_video_to(&path, video)
}

pub fn save_sound(sound: SoundSettings) -> Result<()> {
    let path = path().context("APPDATA is unavailable; cannot save settings")?;
    save_sound_to(&path, sound)
}

fn save_sound_to(path: &Path, sound: SoundSettings) -> Result<()> {
    update_settings(path, serde_json::json!({ "sound": sound }))
}

fn save_video_to(path: &Path, video: VideoSettings) -> Result<()> {
    update_settings(path, serde_json::json!({ "video": video }))
}

/// Preserve fields from newer versions, including fields inside settings groups.
/// Do not replace a malformed or incompatible file with default settings.
fn update_settings(path: &Path, patch: serde_json::Value) -> Result<()> {
    let mut data: serde_json::Value = match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("Cannot read existing settings")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(error) => return Err(error).context("Cannot read existing settings"),
    };
    // Refuse to save defaults over known fields whose type this version cannot
    // read. Unknown fields are retained by the merge below.
    serde_json::from_value::<Settings>(data.clone()).context("Cannot read existing settings")?;
    fn merge(target: &mut serde_json::Value, patch: serde_json::Value) -> Result<()> {
        let target = target
            .as_object_mut()
            .context("Settings group must be a JSON object")?;
        let serde_json::Value::Object(patch) = patch else {
            anyhow::bail!("Settings update must be a JSON object");
        };
        for (key, value) in patch {
            if value.is_object()
                && let Some(existing) = target.get_mut(&key)
            {
                merge(existing, value)
                    .with_context(|| format!("Cannot update settings group {key}"))?;
            } else {
                target.insert(key, value);
            }
        }
        Ok(())
    }
    merge(&mut data, patch)?;
    let parent = path.parent().context("settings path has no parent")?;
    std::fs::create_dir_all(parent)?;
    std::fs::write(path, serde_json::to_vec_pretty(&data)?)
        .with_context(|| format!("Cannot save settings to {}", path.display()))
}

pub(crate) fn save_to(path: &Path, settings: &Settings) -> Result<()> {
    update_settings(path, serde_json::to_value(settings)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_pause_defaults_on_and_survives_unrelated_saves() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("options.json");
        let old: Settings = serde_json::from_str("{}").unwrap();
        assert!(old.video.pause_on_lost_focus);
        let video = VideoSettings {
            pause_on_lost_focus: false,
            ..old.video
        };
        save_video_to(&path, video).unwrap();
        save_sound_to(&path, SoundSettings::default()).unwrap();
        let loaded: Settings = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(!loaded.video.pause_on_lost_focus);
        save_to(&path, &loaded).unwrap();
        let loaded: Settings = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(!loaded.video.pause_on_lost_focus);
    }

    #[test]
    fn all_save_paths_preserve_unknown_fields_and_reject_invalid_files() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("options.json");
        let original = serde_json::json!({
            "future_setting": {"nested": 42},
            "video": {"future_video": {"mode": "new"}},
            "sound": {"future_sound": true}
        });
        for method in 0..3 {
            std::fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
            match method {
                0 => save_to(&path, &Settings::default()),
                1 => save_video_to(&path, VideoSettings::default()),
                _ => save_sound_to(&path, SoundSettings::default()),
            }
            .unwrap();
            let saved: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(saved["future_setting"], original["future_setting"]);
            assert_eq!(
                saved["video"]["future_video"],
                original["video"]["future_video"]
            );
            assert_eq!(
                saved["sound"]["future_sound"],
                original["sound"]["future_sound"]
            );
        }
        for method in 0..3 {
            let wrong_group = if method == 2 {
                br#"{"sound":7}"#.as_slice()
            } else {
                br#"{"video":7}"#.as_slice()
            };
            for bytes in [
                b"{broken".as_slice(),
                b"[]",
                wrong_group,
                br#"{"video":{"fov":"invalid"}}"#,
            ] {
                std::fs::write(&path, bytes).unwrap();
                let result = match method {
                    0 => save_to(&path, &Settings::default()),
                    1 => save_video_to(&path, VideoSettings::default()),
                    _ => save_sound_to(&path, SoundSettings::default()),
                };
                assert!(result.is_err());
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
            }
        }
    }

    #[test]
    fn selected_location_and_pack_paths_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("nested/options.json");
        let mut settings = Settings {
            minecraft_dir: Some(PathBuf::from("game-folder")),
            resource_packs: vec![PathBuf::from("pack.zip")],
            video: VideoSettings {
                render_distance: 24,
                biome_blend: 7,
                vsync: false,
                max_fps: 260,
                fov: 95,
                view_bobbing: true,
                inactivity_fps_limit: InactivityFpsLimit::Minimized,
                ..VideoSettings::default()
            },
            sound: SoundSettings::default(),
        };
        save_to(&path, &settings).unwrap();
        settings.resource_packs.push(PathBuf::from("second.zip"));
        save_to(&path, &settings).unwrap();
        let loaded: Settings = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(loaded.minecraft_dir, settings.minecraft_dir);
        assert_eq!(loaded.resource_packs, settings.resource_packs);
        assert_eq!(loaded.video, settings.video);
    }

    #[test]
    fn saving_video_preserves_other_fields_and_rejects_broken_settings() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("options.json");
        let old = serde_json::json!({
            "minecraft_dir": "another-installation",
            "resource_packs": ["pack.zip"],
            "future_setting": {"nested": 42},
        });
        std::fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
        let video = VideoSettings {
            vsync: false,
            max_fps: 260,
            fov: 90,
            view_bobbing: true,
            sprint_window: 0,
            inactivity_fps_limit: InactivityFpsLimit::Minimized,
            ..VideoSettings::default()
        };
        save_video_to(&path, video).unwrap();
        let loaded: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(loaded["minecraft_dir"], old["minecraft_dir"]);
        assert_eq!(loaded["resource_packs"], old["resource_packs"]);
        assert_eq!(loaded["future_setting"], old["future_setting"]);
        assert_eq!(loaded["video"], serde_json::to_value(video).unwrap());

        let broken = b"{invalid settings";
        std::fs::write(&path, broken).unwrap();
        assert!(save_video_to(&path, video).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), broken);
    }

    #[test]
    fn old_settings_keep_vanilla_video_defaults() {
        let old: Settings =
            serde_json::from_str(r#"{"minecraft_dir":null,"resource_packs":[]}"#).unwrap();
        assert_eq!(old.video, VideoSettings::default());
        assert_eq!(old.sound, SoundSettings::default());
        assert_eq!(
            VideoSettings {
                render_distance: 1,
                biome_blend: 8,
                vsync: false,
                max_fps: 0,
                fov: 111,
                view_bobbing: true,
                inactivity_fps_limit: InactivityFpsLimit::Afk,
                ..VideoSettings::default()
            }
            .valid(),
            VideoSettings {
                vsync: false,
                max_fps: 120,
                fov: 70,
                view_bobbing: true,
                inactivity_fps_limit: InactivityFpsLimit::Afk,
                ..VideoSettings::default()
            }
        );
    }

    #[test]
    fn mouse_settings_survive_video_save_and_validate_old_files() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("options.json");
        std::fs::write(
            &path,
            br#"{"future":{"value":42},"video":{"future_video":true}}"#,
        )
        .unwrap();
        let video = VideoSettings {
            mouse_sensitivity: UnitInterval::new(0.77),
            mouse_wheel_sensitivity: MouseWheelSensitivity::new(3.4),
            discrete_mouse_scroll: true,
            invert_mouse_x: true,
            invert_mouse_y: true,
            ..VideoSettings::default()
        };
        save_video_to(&path, video).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["future"]["value"], 42);
        assert_eq!(saved["video"]["future_video"], true);
        let parsed: Settings = serde_json::from_value(saved).unwrap();
        assert_eq!(parsed.video, video);
        let invalid: VideoSettings =
            serde_json::from_str(r#"{"mouse_sensitivity":1.5,"mouse_wheel_sensitivity":12.0}"#)
                .unwrap();
        assert_eq!(invalid.valid().mouse_sensitivity.value(), 0.5);
        assert_eq!(invalid.valid().mouse_wheel_sensitivity.value(), 1.0);
    }

    #[test]
    fn mouse_wheel_slider_matches_the_301_logarithmic_java_steps() {
        assert_eq!(
            MouseWheelSensitivity::from_slider_fraction(0.0).value(),
            0.01
        );
        assert_eq!(MouseWheelSensitivity::default().value(), 1.0);
        assert_eq!(
            MouseWheelSensitivity::from_slider_fraction(1.0).value(),
            10.0
        );
        for step in -200_i16..=100 {
            let fraction = if step <= -200 {
                0.0
            } else if step >= 100 {
                1.0
            } else {
                (f64::from(step + 200) + 0.5) / 301.0
            };
            let setting = MouseWheelSensitivity::from_slider_fraction(fraction);
            assert_eq!(setting.0, step, "Java wheel slider exponent {step}");
            let slider = setting.slider_fraction();
            assert_eq!(
                MouseWheelSensitivity::from_slider_fraction(slider),
                setting,
                "slider fraction {slider} for exponent {step}"
            );
            let bytes = serde_json::to_vec(&setting).unwrap();
            let decoded: MouseWheelSensitivity = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(decoded, setting, "serialized exponent {step}");
        }
    }

    #[test]
    fn sound_settings_round_trip_preserves_other_fields_and_ui_gain() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("options.json");
        std::fs::write(
            &path,
            br#"{"future_setting":{"nested":42},"video":{"vsync":false}}"#,
        )
        .unwrap();
        let mut sound = SoundSettings::default();
        assert!(sound.set(SoundCategory::Master, 0.5));
        assert!(sound.set(SoundCategory::Ui, 0.25));
        assert_eq!(sound.ui_gain(), 0.125);
        save_sound_to(&path, sound).unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["future_setting"]["nested"], 42);
        assert_eq!(value["video"]["vsync"], false);
        let loaded: Settings = serde_json::from_value(value).unwrap();
        assert_eq!(loaded.sound, sound);
        let invalid: SoundSettings =
            serde_json::from_str(r#"{"volumes":[2.0,-1.0,1,1,1,1,1,1,1,1,1]}"#).unwrap();
        assert_eq!(invalid.valid().get(SoundCategory::Master), 1.0);
        assert_eq!(invalid.valid().get(SoundCategory::Music), 1.0);
        std::fs::write(&path, b"{broken").unwrap();
        assert!(save_sound_to(&path, sound).is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"{broken");
    }

    #[test]
    fn glint_values_persist_as_numbers_and_invalid_values_use_defaults() {
        let video = VideoSettings {
            glint_speed: UnitInterval::new(37.0 / 142.0),
            glint_strength: UnitInterval::new(0.0),
            panorama_speed: UnitInterval::new(0.25),
            fov_effect_scale: UnitInterval::new(0.4),
            ..VideoSettings::default()
        };
        let data = serde_json::to_vec(&video).unwrap();
        let decoded: VideoSettings = serde_json::from_slice(&data).unwrap();
        assert_eq!(decoded, video);
        let value: serde_json::Value = serde_json::from_slice(&data).unwrap();
        assert_eq!(value["glint_speed"].as_f64().unwrap(), 37.0 / 142.0);
        assert_eq!(value["glint_strength"], 0.0);
        assert_eq!(value["panorama_speed"], 0.25);
        assert_eq!(value["fov_effect_scale"], 0.4);
        let invalid: VideoSettings = serde_json::from_str(
            r#"{"glint_speed":1.5,"glint_strength":-1.0,"panorama_speed":2.0,"fov_effect_scale":-1.0}"#,
        )
        .unwrap();
        assert_eq!(invalid.valid().glint_speed.value(), 0.5);
        assert_eq!(invalid.valid().glint_strength.value(), 0.75);
        assert_eq!(invalid.valid().panorama_speed.value(), 1.0);
        assert_eq!(invalid.valid().fov_effect_scale.value(), 1.0);
    }

    #[test]
    fn vsync_selects_a_supported_auto_present_mode() {
        assert_eq!(present_mode(true), wgpu::PresentMode::AutoVsync);
        assert_eq!(present_mode(false), wgpu::PresentMode::AutoNoVsync);
    }
}
