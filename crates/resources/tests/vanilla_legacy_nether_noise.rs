//! Check sequential Nether octave initialization, including error RNG state.
use native_core::random::LegacyRandom;
use native_resources::noise::decode_normal_noise;
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};

fn run(command: &mut Command) -> String {
    let output = command.output().expect("cannot run Java 25");
    assert!(
        output.status.success(),
        "Nether noise oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn legacy_nether_octaves_and_random_state_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("LegacyNetherNoiseOracle.java");
    std::fs::write(&source, include_str!("java/LegacyNetherNoiseOracle.java")).unwrap();
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
    let mut definitions = Vec::new();
    for normalization in [json!(true), json!(false), json!("legacy")] {
        for (base, count, modifiers) in [
            (-7, 2, vec![]),
            (-3, 4, vec![1.0, 0.0, 2.0, 0.0]),
            (-2, 3, vec![0.0, 0.0, 0.0]),
            (-32, 1, vec![]),
            (-1, 2, vec![]),
            (-1, 3, vec![]),
            (1, 1, vec![]),
        ] {
            definitions.push(json!({"base_octave":base,"base_amplitude":0.9494731054427981,"octave_count":count,"amplitude_modifiers":modifiers,"normalize":normalization}));
        }
    }
    let input = temp.path().join("fixtures.json");
    std::fs::write(&input, serde_json::to_vec(&definitions).unwrap()).unwrap();
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("LegacyNetherNoiseOracle")
        .arg(input));
    let parameters: Vec<_> = definitions
        .iter()
        .map(|v| decode_normal_noise(&serde_json::to_vec(v).unwrap()).unwrap())
        .collect();
    let mut current = None;
    let (mut groups, mut samples, mut errors) = (0, 0, 0);
    for line in output.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        match fields.first().copied() {
            Some(kind @ ("S" | "E")) => {
                let index = fields[1].parse::<usize>().unwrap();
                let seed = fields[2].parse::<i64>().unwrap();
                let expected = fields[3].parse::<i64>().unwrap();
                let mut random = LegacyRandom::new(seed);
                let result = parameters[index].create_legacy_nether(&mut random);
                assert_eq!(
                    random.next_long(),
                    expected,
                    "RNG for fixture {index} seed {seed}"
                );
                if kind == "E" {
                    assert!(result.is_err(), "fixture {index} should fail");
                    current = None;
                    errors += 1;
                } else {
                    let noise = result.unwrap();
                    let range = noise.range();
                    assert_eq!(
                        (range.0.to_bits(), range.1.to_bits()),
                        (
                            fields[4].parse::<i32>().unwrap() as u32,
                            fields[5].parse::<i32>().unwrap() as u32
                        ),
                        "range {index}"
                    );
                    current = Some((index, seed, noise));
                }
                groups += 1;
            }
            Some("P") => {
                let x = f64::from_bits(fields[1].parse::<i64>().unwrap() as u64);
                let y = f64::from_bits(fields[2].parse::<i64>().unwrap() as u64);
                let z = f64::from_bits(fields[3].parse::<i64>().unwrap() as u64);
                let (index, seed, noise) = current.as_ref().unwrap();
                assert_eq!(
                    noise.sample(x, y, z).to_bits(),
                    fields[4].parse::<i32>().unwrap() as u32,
                    "3D fixture {index} seed {seed} at {x} {y} {z}"
                );
                assert_eq!(
                    noise.sample_2d(x, z).to_bits(),
                    fields[5].parse::<i32>().unwrap() as u32,
                    "2D fixture {index} seed {seed} at {x} {z}"
                );
                samples += 1;
            }
            _ => {}
        }
    }
    assert_eq!(groups, definitions.len() * 5);
    assert_eq!(errors, 30);
    assert_eq!(samples, (groups - errors) * 6);
}
