//! Entity water-entry state and the vanilla splash sound's random consumption.
use glam::DVec3;
use native_core::random::LegacyRandom;

#[derive(Clone, Copy, Debug)]
pub(crate) struct WaterEntry {
    first_tick: bool,
    touching: bool,
}

impl Default for WaterEntry {
    fn default() -> Self {
        Self {
            first_tick: true,
            touching: false,
        }
    }
}

impl WaterEntry {
    pub(crate) fn update(
        &mut self,
        in_water: bool,
        velocity: DVec3,
        width: f32,
        random: &mut LegacyRandom,
    ) -> Option<crate::ConsumeSoundInfo> {
        let splash = in_water && !self.touching && !self.first_tick;
        self.first_tick = false;
        self.touching = in_water;
        if !splash {
            return None;
        }

        let speed = ((velocity.x * velocity.x * f64::from(0.2_f32)
            + velocity.y * velocity.y
            + velocity.z * velocity.z * f64::from(0.2_f32))
        .sqrt() as f32
            * 0.2_f32)
            .min(1.0_f32);
        let pitch = 1.0_f32 + (random.next_float() - random.next_float()) * 0.4_f32;
        // Java also samples offsets and velocities for local particles, even
        // when this side does not render them. Keep the entity RNG in order.
        let count = (1.0_f32 + width * 20.0_f32).ceil() as usize;
        for _ in 0..count * 5 {
            random.next_double();
        }
        Some(crate::ConsumeSoundInfo {
            event: "entity.generic.splash".into(),
            volume: speed,
            pitch,
            category: crate::ConsumeSoundCategory::Ambient,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_tick_does_not_splash_and_reentry_does() {
        let mut state = WaterEntry::default();
        let mut random = LegacyRandom::new(3);
        assert!(state.update(true, DVec3::ZERO, 0.25, &mut random).is_none());
        assert!(state.update(true, DVec3::ZERO, 0.25, &mut random).is_none());
        assert!(
            state
                .update(false, DVec3::ZERO, 0.25, &mut random)
                .is_none()
        );
        assert!(
            state
                .update(true, DVec3::new(0.0, -1.0, 0.0), 0.25, &mut random)
                .is_some()
        );
        assert!(state.update(true, DVec3::ZERO, 0.25, &mut random).is_none());
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn item_splash_sound_and_random_match_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in metadata["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("ItemWaterSplashOracle.java");
        let output = temp.path().join("output.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/ItemWaterSplashOracle.java"),
        )
        .unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("ItemWaterSplashOracle")
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), 320);
        for row in rows {
            let mut random = LegacyRandom::new(row["seed"].as_i64().unwrap());
            let velocity = DVec3::new(0.3, row["y"].as_f64().unwrap(), -0.4);
            let mut state = WaterEntry::default();
            assert!(state.update(false, velocity, 0.25, &mut random).is_none());
            let sound = state.update(true, velocity, 0.25, &mut random).unwrap();
            assert_eq!(
                format!("minecraft:{}", sound.event),
                row["event"].as_str().unwrap()
            );
            assert_eq!(
                sound.volume.to_bits(),
                row["volume"].as_i64().unwrap() as u32,
                "{row}"
            );
            assert_eq!(
                sound.pitch.to_bits(),
                row["pitch"].as_i64().unwrap() as u32,
                "{row}"
            );
            assert_eq!(
                random.next_int(),
                row["next"].as_i64().unwrap() as i32,
                "{row}"
            );
            assert_eq!(
                (
                    row["bubbles"].as_u64(),
                    row["splashes"].as_u64(),
                    row["events"].as_u64()
                ),
                (Some(6), Some(6), Some(1))
            );
        }
    }
}
