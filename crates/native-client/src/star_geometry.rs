//! Fixed 26.3 celestial star mesh, generated once from the vanilla random seed.

use glam::Vec3;
use native_core::random::LegacyRandom;

/// Four vertices per star in the original Java quad order.
pub fn vertices() -> Vec<[f32; 3]> {
    let mut random = LegacyRandom::new(10842);
    let mut output = Vec::with_capacity(1500 * 4);
    for _ in 0..1500 {
        let x = random.next_float() * 2.0 - 1.0;
        let y = random.next_float() * 2.0 - 1.0;
        let z = random.next_float() * 2.0 - 1.0;
        let size = 0.15_f32 + random.next_float() * 0.1_f32;
        let length_squared = (x * x + y * y) + z * z;
        if !(length_squared > 0.010_000_001_f32 && length_squared < 1.0) {
            continue;
        }
        let direction = Vec3::new(x, y, z);
        let center = direction * (100.0 / length_squared.sqrt());
        let forward = (-center).normalize();
        let right = Vec3::Y.cross(forward).normalize();
        let up = forward.cross(right);
        let angle = (random.next_double() * 3.141_592_741_012_573_2_f64 * 2.0) as f32;
        let (sine, cosine) = (-angle).sin_cos();
        for (x, y) in [(size, -size), (size, size), (-size, size), (-size, -size)] {
            let rx = cosine * x - sine * y;
            let ry = sine * x + cosine * y;
            output.push((center + right * rx + up * ry).to_array());
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stars_are_deterministic_and_on_the_celestial_sphere() {
        let stars = vertices();
        assert!(stars.len() > 2500 && stars.len() < 4500);
        assert_eq!(stars.len() % 4, 0);
        assert_eq!(stars, vertices());
        for quad in stars.chunks_exact(4) {
            let center = quad.iter().map(|v| Vec3::from_array(*v)).sum::<Vec3>() / 4.0;
            assert!((center.length() - 100.0).abs() < 0.000_05);
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn star_vertices_match_java_random_and_joml() {
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
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("StarGeometryOracle.java");
        let output = temp.path().join("stars.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/StarGeometryOracle.java"),
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
                .arg("StarGeometryOracle")
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
        let expected: Vec<[f32; 3]> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let actual = vertices();
        assert_eq!(actual.len(), expected.len());
        for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
            for (actual, expected) in actual.iter().zip(expected) {
                assert!(
                    (actual - expected).abs() < 0.000_04,
                    "star vertex {index}: {actual} != {expected}"
                );
            }
        }
    }
}
