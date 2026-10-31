//! Compare one 26.3 Perlin octave and its random consumption with Java.
use native_core::{
    perlin::{NoiseRandom, PerlinNoise, PerlinStack},
    random::{LegacyRandom, XoroshiroRandom},
};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

fn run(command: &mut Command) -> String {
    let output = command.output().expect("cannot run Java 25");
    assert!(
        output.status.success(),
        "Perlin oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn stack(random: &mut impl NoiseRandom) -> PerlinStack {
    let mut stack = PerlinStack::new();
    for (frequency, amplitude) in [
        (1.0, 1.0),
        (0.25, -0.2),
        (1.0181268882175227, 0.37),
        (2.0, 0.0),
    ] {
        stack.push(PerlinNoise::new(random), frequency, amplitude);
    }
    stack
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn perlin_octaves_and_random_state_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("PerlinNoiseOracle.java");
    std::fs::write(&source, include_str!("java/PerlinNoiseOracle.java")).unwrap();
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
        .arg("PerlinNoiseOracle"));
    let mut sampler = None;
    let mut seed_count = 0;
    let mut sample_count = 0;
    for line in output.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        match fields.first().copied() {
            Some("S") => {
                let mode: i32 = fields[1].parse().unwrap();
                let seed: i64 = fields[2].parse().unwrap();
                let next: i64 = fields[3].parse().unwrap();
                let (noise, actual) = if mode == 0 {
                    let mut random = LegacyRandom::new(seed);
                    let noise = PerlinNoise::new(&mut random);
                    (noise, random.next_long())
                } else {
                    let mut random = XoroshiroRandom::new(seed);
                    let noise = PerlinNoise::new(&mut random);
                    (noise, random.next_long())
                };
                assert_eq!(actual, next, "random state mode={mode} seed={seed}");
                let layers = if mode == 0 {
                    stack(&mut LegacyRandom::new(seed))
                } else {
                    stack(&mut XoroshiroRandom::new(seed))
                };
                sampler = Some((noise, layers));
                seed_count += 1;
            }
            Some("P") => {
                let x = f64::from_bits(fields[1].parse::<i64>().unwrap() as u64);
                let y = f64::from_bits(fields[2].parse::<i64>().unwrap() as u64);
                let z = f64::from_bits(fields[3].parse::<i64>().unwrap() as u64);
                let expected3 = fields[4].parse::<i32>().unwrap() as u32;
                let expected2 = fields[5].parse::<i32>().unwrap() as u32;
                let expected_stack3 = fields[6].parse::<i32>().unwrap() as u32;
                let expected_stack2 = fields[7].parse::<i32>().unwrap() as u32;
                let (noise, layers) = sampler.as_ref().unwrap();
                assert_eq!(noise.sample(x, y, z).to_bits(), expected3, "3D {x} {y} {z}");
                assert_eq!(noise.sample_2d(x, z).to_bits(), expected2, "2D {x} {z}");
                assert_eq!(
                    layers.sample(x, y, z).to_bits(),
                    expected_stack3,
                    "stack 3D {x} {y} {z}"
                );
                assert_eq!(
                    layers.sample_2d(x, z).to_bits(),
                    expected_stack2,
                    "stack 2D {x} {z}"
                );
                sample_count += 1;
            }
            _ => {}
        }
    }
    assert_eq!(seed_count, 14);
    assert_eq!(sample_count, 14 * 144);
}
