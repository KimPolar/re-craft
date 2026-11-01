//! Player movement produced by Java 26.3's honey block inside effect.
use glam::{DVec3, IVec3};
use native_core::random::LegacyRandom;

/// Returns the new velocity only when the player's final position and speed
/// satisfy HoneyBlock.isSlidingDown. Both coordinates are world coordinates.
pub(crate) fn slide(
    position: DVec3,
    block: IVec3,
    on_ground: bool,
    velocity: DVec3,
) -> Option<DVec3> {
    let old_y = velocity.y / f64::from(0.98_f32) + 0.08;
    if on_ground || position.y > f64::from(block.y) + 0.9375 - 1.0e-7 || old_y >= -0.08 {
        return None;
    }
    let dx = (f64::from(block.x) + 0.5 - position.x).abs();
    let dz = (f64::from(block.z) + 0.5 - position.z).abs();
    let threshold = 0.4375 + f64::from(0.6_f32 / 2.0);
    if dx + 1.0e-7 <= threshold && dz + 1.0e-7 <= threshold {
        return None;
    }
    let horizontal = if old_y < -0.13 { -0.05 / old_y } else { 1.0 };
    Some(DVec3::new(
        velocity.x * horizontal,
        (-0.05 - 0.08) * f64::from(0.98_f32),
        velocity.z * horizontal,
    ))
}

/// Server-side slide consumes one roll for sound and one for particles.
pub(crate) fn effects(random: &mut LegacyRandom) -> (bool, bool) {
    let sound = random.next_int_bound(5).expect("positive bound") == 0;
    let particle = random.next_int_bound(5).expect("positive bound") == 0;
    (sound, particle)
}

/// The local client's world independently rolls for sound and does not roll
/// for the server's particle broadcast.
pub(crate) fn client_sound(random: &mut LegacyRandom) -> bool {
    random.next_int_bound(5).expect("positive bound") == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slide_limits_speed_and_ignores_floor_and_center() {
        let velocity = DVec3::new(0.4, -1.0, -0.2);
        let pos = DVec3::new(1.24, 0.5, 0.5);
        let result = slide(pos, IVec3::ZERO, false, velocity).unwrap();
        assert!(result.x < velocity.x && result.z > velocity.z);
        assert_eq!(result.y, (-0.05_f64 - 0.08) * f64::from(0.98_f32));
        assert!(slide(pos, IVec3::ZERO, true, velocity).is_none());
        assert!(slide(DVec3::splat(0.5), IVec3::ZERO, false, velocity).is_none());
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn slide_and_effect_random_match_java() {
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
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("HoneySlideOracle.java");
        let output = temp.path().join("honey.json");
        std::fs::write(&source, include_str!("../tests/java/HoneySlideOracle.java")).unwrap();
        let run = |command: &mut Command| {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source));
        run(Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("HoneySlideOracle")
            .arg(&output));
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), 10752);
        for row in rows {
            let vector = |name: &str| {
                DVec3::new(
                    row[name][0].as_f64().unwrap(),
                    row[name][1].as_f64().unwrap(),
                    row[name][2].as_f64().unwrap(),
                )
            };
            let position = vector("position");
            let initial = vector("initial");
            let block = IVec3::new(
                row["block"][0].as_i64().unwrap() as i32,
                row["block"][1].as_i64().unwrap() as i32,
                row["block"][2].as_i64().unwrap() as i32,
            );
            let actual = slide(position, block, row["ground"].as_bool().unwrap(), initial);
            assert_eq!(
                actual.is_some(),
                row["fall"].as_f64().unwrap() == 0.0,
                "{row}"
            );
            let expected = vector("velocity");
            assert!(
                (actual.unwrap_or(initial) - expected).abs().max_element() < 1.0e-15,
                "{row}"
            );
            let mut random = LegacyRandom::new(row["seed"].as_i64().unwrap());
            let client = row["client"].as_bool().unwrap();
            let (sound, particle) = if actual.is_some() && client {
                (client_sound(&mut random), false)
            } else if actual.is_some() {
                effects(&mut random)
            } else {
                (false, false)
            };
            assert_eq!(sound, row["sounds"].as_i64().unwrap() != 0, "{row}");
            assert_eq!(particle, row["particles"].as_i64().unwrap() != 0, "{row}");
            assert_eq!(
                random.next_int(),
                row["next"].as_i64().unwrap() as i32,
                "{row}"
            );
        }
    }
}
