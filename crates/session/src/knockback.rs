//! Java's living-entity knockback for a damage source without a position.
use glam::DVec3;
use native_core::random::LegacyRandom;

pub(crate) fn from_unlocated_damage(
    velocity: DVec3,
    on_ground: bool,
    resistance: f64,
    random: &mut LegacyRandom,
) -> DVec3 {
    let strength = f64::from(0.4_f32) * (1.0 - resistance);
    if strength <= 0.0 {
        return velocity;
    }
    let (mut x, mut z) = (0.0, 0.0);
    while x * x + z * z < f64::from(1.0e-5_f32) {
        x = (random.next_double() - random.next_double()) * 0.01;
        z = (random.next_double() - random.next_double()) * 0.01;
    }
    let length = (x * x + z * z).sqrt();
    DVec3::new(
        velocity.x / 2.0 - x / length * strength,
        if on_ground {
            0.4_f64.min(velocity.y / 2.0 + strength)
        } else {
            velocity.y
        },
        velocity.z / 2.0 - z / length * strength,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn immunity_preserves_velocity_and_random_state() {
        let mut random = LegacyRandom::new(42);
        let mut expected = random.clone();
        let velocity = DVec3::new(0.2, -0.1, 0.3);
        assert_eq!(
            from_unlocated_damage(velocity, true, 1.0, &mut random),
            velocity
        );
        assert_eq!(random.next_int(), expected.next_int());
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn player_unlocated_knockback_and_lava_sound_random_match_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
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
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("PlayerFireKnockbackOracle.java");
        let result = temp.path().join("knockback.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/PlayerFireKnockbackOracle.java"),
        )
        .unwrap();
        let compile = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("PlayerFireKnockbackOracle")
            .arg(&result)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(result).unwrap()).unwrap();
        assert_eq!(rows.len(), 120);
        for row in rows {
            let initial = row["initial"].as_array().unwrap();
            let velocity = DVec3::new(
                initial[0].as_f64().unwrap(),
                initial[1].as_f64().unwrap(),
                initial[2].as_f64().unwrap(),
            );
            let mut random = LegacyRandom::new(row["seed"].as_i64().unwrap());
            let actual = from_unlocated_damage(
                velocity,
                row["grounded"].as_bool().unwrap(),
                row["resistance"].as_f64().unwrap(),
                &mut random,
            );
            let expected = row["result"].as_array().unwrap();
            for (component, expected) in actual.to_array().into_iter().zip(expected) {
                assert!(
                    (component - expected.as_f64().unwrap()).abs() <= 1.0e-15,
                    "{row}"
                );
            }
            assert_eq!(
                (2.0_f32 + random.next_float() * 0.4_f32).to_bits(),
                row["burn_pitch_bits"].as_i64().unwrap() as u32,
                "{row}"
            );
            assert_eq!(
                random.next_int(),
                row["next"].as_i64().unwrap() as i32,
                "{row}"
            );
        }
    }
}
