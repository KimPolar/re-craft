//! Compare the two 26.3 smeared-octave sampling paths with Java.
use native_core::{
    perlin::PerlinNoise,
    random::{LegacyRandom, XoroshiroRandom},
};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

fn run(command: &mut Command) -> String {
    let output = command.output().expect("cannot run Java 25");
    assert!(
        output.status.success(),
        "smeared Perlin oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn smeared_octaves_match_java_single_and_volume() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("SmearedPerlinOracle.java");
    std::fs::write(&source, include_str!("java/SmearedPerlinOracle.java")).unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("SmearedPerlinOracle"));
    let mut sampler = None;
    let mut groups = 0;
    let mut points = 0;
    let mut volume_points = 0;
    for line in output.lines() {
        let f: Vec<_> = line.split_whitespace().collect();
        match f.first().copied() {
            Some("S") => {
                let mode: i32 = f[1].parse().unwrap();
                let seed: i64 = f[2].parse().unwrap();
                let smear = f64::from_bits(f[3].parse::<i64>().unwrap() as u64);
                let expected: i64 = f[4].parse().unwrap();
                let (noise, next) = if mode == 0 {
                    let mut rng = LegacyRandom::new(seed);
                    let noise = PerlinNoise::new(&mut rng);
                    (noise, rng.next_long())
                } else {
                    let mut rng = XoroshiroRandom::new(seed);
                    let noise = PerlinNoise::new(&mut rng);
                    (noise, rng.next_long())
                };
                assert_eq!(next, expected);
                sampler = Some((mode, seed, smear, noise));
                groups += 1;
            }
            Some("P") => {
                let coordinate = |i: usize| f64::from_bits(f[i].parse::<i64>().unwrap() as u64);
                let (mode, seed, smear, noise) = sampler.as_ref().unwrap();
                assert_eq!(
                    noise
                        .sample_smeared(coordinate(1), coordinate(2), coordinate(3), *smear, false)
                        .to_bits(),
                    f[4].parse::<i32>().unwrap() as u32,
                    "single mode {mode} seed {seed} smear {smear} point {f:?}"
                );
                points += 1;
            }
            Some("V") => {
                let x: i32 = f[1].parse().unwrap();
                let y: i32 = f[2].parse().unwrap();
                let z: i32 = f[3].parse().unwrap();
                let xz = f64::from_bits(f[4].parse::<i64>().unwrap() as u64);
                let ys = f64::from_bits(f[5].parse::<i64>().unwrap() as u64);
                let (mode, seed, smear, noise) = sampler.as_ref().unwrap();
                assert_eq!(
                    noise
                        .sample_smeared(
                            f64::from(x) * xz,
                            f64::from(y) * ys,
                            f64::from(z) * xz,
                            *smear,
                            true
                        )
                        .to_bits(),
                    f[6].parse::<i32>().unwrap() as u32,
                    "volume mode {mode} seed {seed} smear {smear} at {x},{y},{z}"
                );
                volume_points += 1;
            }
            _ => {}
        }
    }
    assert_eq!(groups, 70);
    assert_eq!(points, groups * 64);
    assert_eq!(volume_points, groups * 24);
}
