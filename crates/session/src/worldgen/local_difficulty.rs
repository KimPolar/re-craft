//! Java 26.3 local difficulty for generation-time mob finalization.
use native_persistence::world::Difficulty;

/// Java's local difficulty, retaining its single-precision arithmetic.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalDifficulty {
    pub base: Difficulty,
    pub effective: f32,
}

// Mth.clamp(float) returns NaN if the value or upper bound is NaN.
fn clamp(value: f32, min: f32, max: f32) -> f32 {
    if value < min {
        min
    } else if value.is_nan() || max.is_nan() {
        f32::NAN
    } else if value <= max {
        value
    } else {
        max
    }
}

impl LocalDifficulty {
    /// Compute DifficultyInstance from the current clock, chunk inhabited
    /// time and moon brightness, in the same operation order as Java.
    pub fn new(
        base: Difficulty,
        overworld_clock_time: i64,
        inhabited_time: i64,
        moon_brightness: f32,
    ) -> Self {
        if base == Difficulty::Peaceful {
            return Self {
                base,
                effective: 0.0,
            };
        }
        let hard = base == Difficulty::Hard;
        let global = clamp(
            (overworld_clock_time as f32 + -72_000.0_f32) / 1_440_000.0_f32,
            0.0,
            1.0,
        ) * 0.25_f32;
        let mut difficulty = 0.75_f32 + global;
        let mut local = 0.0_f32;
        local += clamp(inhabited_time as f32 / 3_600_000.0_f32, 0.0, 1.0)
            * if hard { 1.0_f32 } else { 0.75_f32 };
        local += clamp(moon_brightness * 0.25_f32, 0.0, global);
        if base == Difficulty::Easy {
            local *= 0.5_f32;
        }
        difficulty += local;
        let id = match base {
            Difficulty::Peaceful => 0,
            Difficulty::Easy => 1,
            Difficulty::Normal => 2,
            Difficulty::Hard => 3,
        };
        Self {
            base,
            effective: id as f32 * difficulty,
        }
    }

    /// WorldGenRegion uses the overworld clock and always supplies zero for
    /// inhabited time, even when a neighboring persisted chunk is inhabited.
    pub fn for_generation(
        base: Difficulty,
        overworld_clock_time: i64,
        moon_brightness: f32,
    ) -> Self {
        Self::new(base, overworld_clock_time, 0, moon_brightness)
    }

    pub fn special_multiplier(self) -> f32 {
        if self.effective < 2.0 {
            0.0
        } else if self.effective > 4.0 {
            1.0
        } else {
            (self.effective - 2.0) / 2.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generation_uses_zero_inhabited_time_and_float_clamps() {
        assert_eq!(
            LocalDifficulty::for_generation(Difficulty::Peaceful, i64::MAX, 1.0).effective,
            0.0
        );
        let fresh = LocalDifficulty::for_generation(Difficulty::Hard, 0, 1.0);
        assert_eq!(fresh.effective, 2.25);
        assert_eq!(fresh.special_multiplier(), 0.125);
        assert!(
            LocalDifficulty::new(Difficulty::Hard, 2_000_000, 3_600_000, 1.0).effective
                > LocalDifficulty::for_generation(Difficulty::Hard, 2_000_000, 1.0).effective
        );
        assert!(
            LocalDifficulty::for_generation(Difficulty::Normal, 0, f32::NAN)
                .special_multiplier()
                .is_nan()
        );
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn matches_java_difficulty_instance() -> anyhow::Result<()> {
        use anyhow::{Context, ensure};
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        ensure!(manifest["id"] == "26.3", "wrong version");
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for lib in manifest["libraries"]
            .as_array()
            .context("missing libraries")?
        {
            if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths)?;
        let source = temp.path().join("LocalDifficultyOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/LocalDifficultyOracle.java"),
        )?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compile failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(cp)
            .arg("LocalDifficultyOracle")
            .output()?;
        ensure!(
            output.status.success(),
            "Java difficulty test failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let canonical = |value: f32| {
            if value.is_nan() {
                0x7fc0_0000
            } else {
                value.to_bits()
            }
        };
        let output = String::from_utf8(output.stdout)?;
        let mut count = 0;
        for line in output.lines().filter_map(|line| line.strip_prefix("D\t")) {
            let fields: Vec<_> = line.split('\t').collect();
            ensure!(fields.len() == 6, "invalid Java difficulty output");
            let base = match fields[0] {
                "0" => Difficulty::Peaceful,
                "1" => Difficulty::Easy,
                "2" => Difficulty::Normal,
                "3" => Difficulty::Hard,
                _ => anyhow::bail!("invalid Java difficulty"),
            };
            let time = fields[1].parse()?;
            let age = fields[2].parse()?;
            let moon = f32::from_bits(fields[3].parse::<i32>()? as u32);
            let expected = fields[4].parse::<i32>()? as u32;
            let special = fields[5].parse::<i32>()? as u32;
            let actual = LocalDifficulty::new(base, time, age, moon);
            assert_eq!(canonical(actual.effective), expected, "{line}");
            assert_eq!(canonical(actual.special_multiplier()), special, "{line}");
            if age == 0 {
                assert_eq!(
                    canonical(LocalDifficulty::for_generation(base, time, moon).effective),
                    expected,
                    "{line}"
                );
            }
            count += 1;
        }
        assert_eq!(count, 4 * 8 * 7 * 8);
        Ok(())
    }
}
