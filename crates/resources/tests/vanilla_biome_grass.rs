//! Compare the 26.3 simplex implementation and swamp modifier to Java.

#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn simplex_and_swamp_grass_match_java() {
    use native_core::simplex::Simplex2D;
    use native_resources::biomes::{BiomeColors, ColorMaps};
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
    let source = temp.path().join("BiomeGrassOracle.java");
    let output = temp.path().join("grass.csv");
    std::fs::write(&source, include_str!("java/BiomeGrassOracle.java")).unwrap();
    for command in [
        Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source),
        Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("BiomeGrassOracle")
            .arg(&output),
    ] {
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "Java biome grass oracle failed: {}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let biome = BiomeColors::from_json(br##"{"temperature":0.8,"downfall":0.9,"effects":{"water_color":"#3f76e4","grass_color_modifier":"swamp"}}"##, &ColorMaps::default()).unwrap();
    let text = std::fs::read_to_string(output).unwrap();
    let mut current = None;
    let mut count = 0;
    let mut colors = [0, 0];
    for line in text.lines() {
        let parts: Vec<_> = line.split(',').collect();
        let seed: i64 = parts[0].parse().unwrap();
        let zero: bool = parts[1].parse().unwrap();
        let x: i32 = parts[2].parse().unwrap();
        let z: i32 = parts[3].parse().unwrap();
        let expected_noise: i32 = parts[4].parse().unwrap();
        let expected_color: i32 = parts[5].parse().unwrap();
        if current
            .as_ref()
            .is_none_or(|(s, z, _)| (*s, *z) != (seed, zero))
        {
            current = Some((seed, zero, Simplex2D::new(seed, zero)));
        }
        let noise = current
            .as_ref()
            .unwrap()
            .2
            .sample(f64::from(x) * 0.0225, f64::from(z) * 0.0225);
        assert_eq!(
            noise.to_bits(),
            expected_noise as u32,
            "noise seed={seed} zero={zero} x={x} z={z}"
        );
        let color = biome.grass_at(x, z);
        assert_eq!(color, expected_color as u32 & 0xffffff, "swamp x={x} z={z}");
        colors[usize::from(color == 0x4c763c)] += 1;
        count += 1;
    }
    assert_eq!(count, 72000);
    assert!(colors.into_iter().all(|n| n > 0));
}
