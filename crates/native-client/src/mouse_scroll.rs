//! Mouse wheel scaling and hotbar scroll accumulation shared by both clients.
use crate::menu_settings::VideoSettings;

pub fn scaled_delta(delta: f64, settings: VideoSettings) -> f64 {
    scaled_axis(delta, settings)
}

pub fn scaled_axes(delta: (f64, f64), settings: VideoSettings) -> (f64, f64) {
    (
        scaled_axis(delta.0, settings),
        scaled_axis(delta.1, settings),
    )
}

fn scaled_axis(delta: f64, settings: VideoSettings) -> f64 {
    if !delta.is_finite() {
        return 0.0;
    }
    let delta = if settings.discrete_mouse_scroll {
        delta.signum()
    } else {
        delta
    };
    delta * settings.mouse_wheel_sensitivity.value()
}

#[derive(Default)]
pub struct HotbarScroll {
    remainder_x: f64,
    remainder_y: f64,
}

impl HotbarScroll {
    /// Mirrors ScrollWheelHandler and Inventory.getNextScrollWheelSelection:
    /// opposite-direction fractions are discarded and positive scroll moves
    /// toward the preceding slot.
    pub fn select(&mut self, delta: (f64, f64), selected: u8) -> Option<u8> {
        let (steps_x, steps_y) = self.steps(delta);
        let steps = if steps_y != 0 { steps_y } else { -steps_x };
        if steps == 0 {
            return None;
        }
        Some((i32::from(selected) - steps).rem_euclid(9) as u8)
    }

    /// Consume both accumulated axes, including horizontal spectator input.
    pub fn steps(&mut self, delta: (f64, f64)) -> (i32, i32) {
        self.remainder_x = accumulate_axis(self.remainder_x, delta.0);
        self.remainder_y = accumulate_axis(self.remainder_y, delta.1);
        let steps_x = self.remainder_x.trunc() as i32;
        let steps_y = self.remainder_y.trunc() as i32;
        self.remainder_x -= f64::from(steps_x);
        self.remainder_y -= f64::from(steps_y);
        (steps_x, steps_y)
    }

    /// A horizontal-only whole step still triggers Java's speed clamp.
    pub fn spectator(&mut self, delta: (f64, f64)) -> Option<i32> {
        let (x, y) = self.steps(delta);
        (x != 0 || y != 0).then_some(y)
    }

    pub fn game_command(
        &mut self,
        delta: (f64, f64),
        mode: native_session::GameMode,
        selected: u8,
    ) -> Option<native_session::PlayerCommand> {
        if mode == native_session::GameMode::Spectator {
            self.spectator(delta)
                .map(native_session::PlayerCommand::AdjustFlightSpeed)
        } else {
            self.select(delta, selected)
                .map(native_session::PlayerCommand::SelectSlot)
        }
    }

    #[cfg(test)]
    pub fn clear(&mut self) {
        self.remainder_x = 0.0;
        self.remainder_y = 0.0;
    }
}

fn accumulate_axis(remainder: f64, delta: f64) -> f64 {
    let delta = if delta.is_finite() { delta } else { 0.0 };
    if remainder != 0.0 && delta.signum() != remainder.signum() {
        delta
    } else {
        remainder + delta
    }
}

pub fn event_axes(delta: winit::event::MouseScrollDelta) -> (f64, f64) {
    match delta {
        winit::event::MouseScrollDelta::LineDelta(x, y) => (f64::from(x), f64::from(y)),
        winit::event::MouseScrollDelta::PixelDelta(position) => {
            (position.x / 20.0, position.y / 20.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu_settings::MouseWheelSensitivity;
    use std::{ffi::OsString, path::PathBuf, process::Command};

    #[test]
    fn wheel_settings_scale_smooth_and_discrete_events() {
        let settings = VideoSettings {
            mouse_wheel_sensitivity: MouseWheelSensitivity::new(2.5),
            ..VideoSettings::default()
        };
        let sensitivity = settings.mouse_wheel_sensitivity.value();
        assert_eq!(scaled_delta(0.25, settings), 0.25 * sensitivity);
        assert_eq!(
            scaled_delta(
                -0.25,
                VideoSettings {
                    discrete_mouse_scroll: true,
                    ..settings
                }
            ),
            -sensitivity
        );
        assert_eq!(scaled_delta(f64::NAN, settings), 0.0);
        assert_eq!(
            scaled_axes((0.25, -0.5), settings),
            (0.25 * sensitivity, -0.5 * sensitivity)
        );
    }

    #[test]
    fn hotbar_keeps_fractions_and_matches_vanilla_direction_and_wrap() {
        let mut scroll = HotbarScroll::default();
        assert_eq!(scroll.select((0.0, 0.4), 4), None);
        assert_eq!(scroll.select((0.0, 0.7), 4), Some(3));
        assert_eq!(scroll.select((0.0, -2.0), 3), Some(5));
        scroll.clear();
        assert_eq!(scroll.select((0.0, 1.0), 0), Some(8));
        assert_eq!(scroll.select((0.0, -1.0), 8), Some(0));
        assert_eq!(scroll.select((0.0, f64::INFINITY), 3), None);
    }

    #[test]
    fn spectator_wheel_uses_only_vertical_steps_and_consumes_horizontal() {
        let mut scroll = HotbarScroll::default();
        assert_eq!(scroll.spectator((1.2, 0.4)), Some(0));
        assert_eq!(scroll.spectator((0.0, 0.7)), Some(1));
        assert_eq!(scroll.spectator((-0.4, -0.2)), None);
        assert_eq!(scroll.spectator((-0.7, -0.8)), Some(-1));
    }

    #[test]
    fn shared_game_wheel_routes_spectator_and_hotbar_actions() {
        let mut scroll = HotbarScroll::default();
        assert!(matches!(
            scroll.game_command((0.0, 1.0), native_session::GameMode::Spectator, 4),
            Some(native_session::PlayerCommand::AdjustFlightSpeed(1))
        ));
        assert!(matches!(
            scroll.game_command((1.0, 0.0), native_session::GameMode::Spectator, 4),
            Some(native_session::PlayerCommand::AdjustFlightSpeed(0))
        ));
        assert!(matches!(
            scroll.game_command((0.0, -1.0), native_session::GameMode::Creative, 4),
            Some(native_session::PlayerCommand::SelectSlot(5))
        ));
    }

    #[test]
    fn opposite_direction_resets_the_pending_fraction_and_horizontal_scroll_works() {
        let mut scroll = HotbarScroll::default();
        assert_eq!(scroll.select((0.7, 0.0), 4), None);
        assert_eq!(scroll.select((-0.5, 0.0), 4), None);
        assert_eq!(scroll.select((-0.5, 0.0), 4), Some(3));
        scroll.clear();
        assert_eq!(scroll.select((0.0, -0.5), 3), None);
        assert_eq!(scroll.select((0.0, 0.25), 3), None);
    }

    #[test]
    #[ignore = "requires the installed Minecraft 26.3 and Java 25 JDK"]
    fn hotbar_scroll_accumulation_matches_installed_java() {
        const JAVA: &str = r#"
import java.nio.file.*;
import net.minecraft.client.ScrollWheelHandler;
import net.minecraft.util.Mth;
import net.minecraft.world.entity.player.Abilities;
import org.joml.Vector2i;
public class ScrollWheelOracle {
  public static void main(String[] args) throws Exception {
    var handler = new ScrollWheelHandler();
    var abilities = new Abilities();
    var result = new StringBuilder();
    for (String line : Files.readAllLines(Path.of(args[0]))) {
      String[] parts = line.split(" ");
      Vector2i step = handler.onMouseScroll(Double.parseDouble(parts[0]), Double.parseDouble(parts[1]));
      if (step.x != 0 || step.y != 0) abilities.setFlyingSpeed(Mth.clamp(abilities.getFlyingSpeed() + (float)step.y * 0.005f, 0.0f, 0.2f));
      result.append(step.x).append(' ').append(step.y).append(' ').append(Float.floatToRawIntBits(abilities.getFlyingSpeed())).append('\n');
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
        let source = temp.path().join("ScrollWheelOracle.java");
        let input = temp.path().join("scroll.txt");
        let output = temp.path().join("oracle.txt");
        std::fs::write(&source, JAVA).unwrap();
        let mut deltas = vec![
            (0.0, 0.4),
            (0.0, 0.7),
            (0.0, -0.2),
            (0.0, -0.8),
            (0.4, 0.0),
            (0.7, 0.0),
            (-0.2, 0.0),
            (-0.8, 0.0),
            (0.5, 0.5),
            (-0.5, 1.25),
            (1.2, 0.0),
        ];
        let mut seed = 0x6d2b_79f5_u32;
        for _ in 0..512 {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let x = (f64::from(seed & 0xffff) / 32767.5 - 1.0) * 1.7;
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let y = (f64::from(seed & 0xffff) / 32767.5 - 1.0) * 1.7;
            deltas.push((x, y));
        }
        let input_text = deltas
            .iter()
            .map(|(x, y)| format!("{x:.17} {y:.17}\n"))
            .collect::<String>();
        std::fs::write(&input, input_text).unwrap();
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
        let java_home = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join(
                    "AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon",
                )
            })
            .join("bin");
        let run = |command: &mut Command| {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "Java scroll oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        };
        run(Command::new(java_home.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source));
        run(Command::new(java_home.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("ScrollWheelOracle")
            .arg(&input)
            .arg(&output));
        let java_steps: Vec<(i32, i32, i32)> = std::fs::read_to_string(output)
            .unwrap()
            .lines()
            .map(|line| {
                let mut values = line.split_whitespace().map(|v| v.parse::<i32>().unwrap());
                (
                    values.next().unwrap(),
                    values.next().unwrap(),
                    values.next().unwrap(),
                )
            })
            .collect();
        assert_eq!(java_steps.len(), deltas.len());
        let mut selected = 4_u8;
        let mut scroll = HotbarScroll::default();
        let mut flight_scroll = HotbarScroll::default();
        let mut flight_speed = 0.05_f32;
        for ((delta_x, delta_y), (steps_x, steps_y, speed_bits)) in
            deltas.into_iter().zip(java_steps)
        {
            let flight_steps = flight_scroll.spectator((delta_x, delta_y));
            assert_eq!(
                flight_steps,
                (steps_x != 0 || steps_y != 0).then_some(steps_y)
            );
            if let Some(steps) = flight_steps {
                flight_speed = native_session::physics::adjust_flight_speed(flight_speed, steps);
            }
            assert_eq!(flight_speed.to_bits(), speed_bits as u32);
            let expected_steps = if steps_y != 0 { steps_y } else { -steps_x };
            let expected = (expected_steps != 0)
                .then(|| (i32::from(selected) - expected_steps).rem_euclid(9) as u8);
            let actual = scroll.select((delta_x, delta_y), selected);
            assert_eq!(actual, expected, "delta=({delta_x}, {delta_y})");
            if let Some(slot) = actual {
                selected = slot;
            }
        }
    }
}
