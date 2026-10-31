//! Java 26.3 compass numeric property for a local player.

use glam::{DVec3, IVec3};

fn positive_modulo(value: f32) -> f32 {
    (value % 1.0 + 1.0) % 1.0
}

/// The vanilla wobbler advances at most once for each observed game tick.
#[derive(Default)]
pub(crate) struct Wobbler {
    rotation: f32,
    velocity: f32,
    last_tick: i64,
}

impl Wobbler {
    fn should_update(&self, tick: i64) -> bool {
        self.last_tick != tick
    }
    pub(crate) fn update(&mut self, tick: i64, target: f32) -> f32 {
        if self.last_tick != tick {
            self.last_tick = tick;
            let delta = positive_modulo(target - self.rotation + 0.5) - 0.5;
            self.velocity = (self.velocity + delta * 0.1) * 0.8;
            self.rotation = positive_modulo(self.rotation + self.velocity);
        }
        self.rotation
    }
}

pub(crate) struct RandomNeedle {
    wobbler: Wobbler,
    random: native_core::random::LegacyRandom,
}

impl RandomNeedle {
    fn with_seed(seed: i64) -> Self {
        Self {
            wobbler: Wobbler::default(),
            random: native_core::random::LegacyRandom::new(seed),
        }
    }

    pub(crate) fn angle(&mut self, tick: i64, item_seed: i32) -> f32 {
        let rotation = if self.wobbler.should_update(tick) {
            self.wobbler.update(tick, self.random.next_float())
        } else {
            self.wobbler.rotation
        };
        let hash = item_seed.wrapping_mul(1_327_217_883);
        positive_modulo(rotation + (hash as f32) / 2_147_483_648.0_f32)
    }
}

impl Default for RandomNeedle {
    fn default() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        Self::with_seed((time ^ SEQUENCE.fetch_add(0x9e3779b97f4a7c15, Ordering::Relaxed)) as i64)
    }
}

/// Returns None when the owner is at the block center, when the Java compass
/// deliberately switches to random rotation instead of pointing at it.
pub(crate) fn local_target_angle(
    wobbler: &mut Wobbler,
    tick: i64,
    yaw_degrees: f32,
    owner: DVec3,
    target: IVec3,
) -> Option<f32> {
    let center = target.as_dvec3() + DVec3::splat(0.5);
    let delta = center - owner;
    if !owner.is_finite()
        || !yaw_degrees.is_finite()
        || delta.length_squared() < f64::from(1.0e-5_f32)
    {
        return None;
    }
    // Java divides by the double widening of the float constant 2π.
    let direction = (delta.z.atan2(delta.x) / f64::from(std::f32::consts::TAU)) as f32;
    let yaw = positive_modulo(yaw_degrees / 360.0);
    let rotation = wobbler.update(tick, 0.5 - (yaw - 0.25));
    Some(positive_modulo(direction + rotation))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn target_center_and_repeated_ticks_are_handled() {
        let mut wobbler = Wobbler::default();
        assert!(local_target_angle(&mut wobbler, 1, 0.0, DVec3::splat(0.5), IVec3::ZERO).is_none());
        let first =
            local_target_angle(&mut wobbler, 1, 45.0, DVec3::ZERO, IVec3::new(1, 0, 0)).unwrap();
        let second =
            local_target_angle(&mut wobbler, 1, 90.0, DVec3::ZERO, IVec3::new(1, 0, 0)).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn local_motion_matches_java_compass_angle() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        native_resources::packs::ResourceStack::open(&game).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for lib in metadata["libraries"].as_array().unwrap() {
            if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("CompassMotionOracle.java");
        let output = temp.path().join("motion.json");
        let random_output = temp.path().join("random.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/CompassMotionOracle.java"),
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
                .arg("CompassMotionOracle")
                .arg(&output)
                .arg(&random_output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), 240);
        let mut wobbler = Wobbler::default();
        for row in rows {
            let f = |name: &str| row[name].as_f64().unwrap();
            let i = |name: &str| row[name].as_i64().unwrap() as i32;
            let actual = local_target_angle(
                &mut wobbler,
                row["tick"].as_i64().unwrap(),
                f("yaw") as f32,
                DVec3::new(f("x"), f("y"), f("z")),
                IVec3::new(i("tx"), i("ty"), i("tz")),
            )
            .unwrap();
            let expected = f32::from_bits(row["bits"].as_i64().unwrap() as i32 as u32);
            assert!(
                (actual - expected).abs() <= 1e-6,
                "{row}: {actual} != {expected}"
            );
        }
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(random_output).unwrap()).unwrap();
        assert_eq!(rows.len(), 245);
        let mut random = RandomNeedle::with_seed(1_234_567);
        for row in rows {
            let actual = random.angle(
                row["tick"].as_i64().unwrap(),
                row["seed"].as_i64().unwrap() as i32,
            );
            assert_eq!(
                actual.to_bits(),
                row["bits"].as_i64().unwrap() as i32 as u32,
                "{row}"
            );
        }
    }
}
