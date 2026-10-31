//! Differential test for the actual Java client tint averaging method.

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn biome_blending_matches_java() {
    use glam::IVec3;
    use std::{path::PathBuf, process::Command};
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let version = game.join("versions/26.3");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for entry in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = entry["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let source = temp.path().join("BiomeBlendOracle.java");
    let output = temp.path().join("biome_blend.json");
    std::fs::write(&source, include_str!("java/BiomeBlendOracle.java")).unwrap();
    for command in [
        Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source),
        Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("BiomeBlendOracle")
            .arg(&output),
    ] {
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "Java biome blend oracle failed: {}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let rows: Vec<Vec<i64>> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(rows.len(), 640);
    for row in rows {
        let pos = IVec3::new(row[1] as i32, row[2] as i32, row[3] as i32);
        let result = native_world::biome_blend::blend(pos, row[0] as u8, |p| {
            let r = p.x.wrapping_mul(37).wrapping_add(p.z.wrapping_mul(17)) as u32 & 255;
            let g = p.x.wrapping_mul(11).wrapping_sub(p.z.wrapping_mul(29)) as u32 & 255;
            let b = p.x.wrapping_mul(7).wrapping_add(p.z.wrapping_mul(13)) as u32 & 255;
            (r << 16) | (g << 8) | b
        });
        assert_eq!(i64::from(result as i32), row[4], "{row:?}");
    }
}
