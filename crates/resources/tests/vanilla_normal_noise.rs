//! Compare the installed 26.3 noise registry with Java's normal noise sampler.
use native_core::random::{LegacyRandom, XoroshiroRandom};
use native_resources::noise::decode_normal_noise;
use serde_json::{Value, json};
use std::{fs::File, io::Read, path::PathBuf, process::Command};

fn run(command: &mut Command) -> String {
    let output = command.output().expect("cannot run Java 25");
    assert!(
        output.status.success(),
        "normal-noise oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn installed_and_custom_normal_noise_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("NormalNoiseOracle.java");
    std::fs::write(&source, include_str!("java/NormalNoiseOracle.java")).unwrap();
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
    let mut jar = zip::ZipArchive::new(File::open(version.join("26.3.jar")).unwrap()).unwrap();
    let mut definitions = Vec::new();
    for index in 0..jar.len() {
        let mut entry = jar.by_index(index).unwrap();
        if entry.name().starts_with("data/minecraft/worldgen/noise/")
            && entry.name().ends_with(".json")
        {
            let mut text = String::new();
            entry.read_to_string(&mut text).unwrap();
            definitions.push(serde_json::from_str::<Value>(&text).unwrap());
        }
    }
    let installed_count = definitions.len();
    assert_eq!(installed_count, 64);
    definitions.extend([
        json!({"base_octave":0}),
        json!({"base_octave":-4,"octave_count":4,"amplitude_modifiers":[0,1,0,2]}),
        json!({"base_octave":-4,"octave_count":4,"amplitude_modifiers":[0,1,0,2],"normalize":false}),
        json!({"base_octave":-4,"octave_count":4,"amplitude_modifiers":[0,1,0,2],"normalize":"legacy"}),
        json!({"base_octave":-32,"base_amplitude":0.00001,"octave_count":3,"amplitude_modifiers":[0,0,0]}),
        json!({"base_octave":32,"base_amplitude":1000000,"octave_count":4,"amplitude_modifiers":[1,0.00001,999999,0.3]}),
    ]);
    let modifiers: Vec<f64> = (0..32)
        .map(|i| {
            if i % 7 == 0 {
                0.0
            } else {
                (i as f64 + 0.13) / 7.3
            }
        })
        .collect();
    for normalization in [json!(true), json!(false), json!("legacy")] {
        definitions.push(json!({"base_octave":-17,"base_amplitude":5.678901234,"octave_count":32,"amplitude_modifiers":modifiers,"normalize":normalization}));
    }
    let input = temp.path().join("noise.json");
    std::fs::write(&input, serde_json::to_vec(&definitions).unwrap()).unwrap();
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("NormalNoiseOracle")
        .arg(input));
    let parameters: Vec<_> = definitions
        .iter()
        .map(|v| decode_normal_noise(&serde_json::to_vec(v).unwrap()).unwrap())
        .collect();
    let mut sampler = None;
    let mut groups = 0;
    let mut samples = 0;
    for line in output.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        match fields.first().copied() {
            Some("S") => {
                let index: usize = fields[1].parse().unwrap();
                let mode: i32 = fields[2].parse().unwrap();
                let seed: i64 = fields[3].parse().unwrap();
                let expected_next: i64 = fields[4].parse().unwrap();
                let min = fields[5].parse::<i32>().unwrap() as u32;
                let max = fields[6].parse::<i32>().unwrap() as u32;
                let (noise, next) = if mode == 0 {
                    let mut random = LegacyRandom::new(seed);
                    let noise = parameters[index].create(&mut random).unwrap();
                    (noise, random.next_long())
                } else {
                    let mut random = XoroshiroRandom::new(seed);
                    let noise = parameters[index].create(&mut random).unwrap();
                    (noise, random.next_long())
                };
                assert_eq!(
                    next, expected_next,
                    "random state index={index} mode={mode} seed={seed}"
                );
                let range = noise.range();
                assert_eq!(
                    (range.0.to_bits(), range.1.to_bits()),
                    (min, max),
                    "range index={index}: {}",
                    definitions[index]
                );
                sampler = Some((index, mode, seed, noise));
                groups += 1;
            }
            Some("P") => {
                let x = f64::from_bits(fields[1].parse::<i64>().unwrap() as u64);
                let y = f64::from_bits(fields[2].parse::<i64>().unwrap() as u64);
                let z = f64::from_bits(fields[3].parse::<i64>().unwrap() as u64);
                let a = fields[4].parse::<i32>().unwrap() as u32;
                let b = fields[5].parse::<i32>().unwrap() as u32;
                let (index, mode, seed, noise) = sampler.as_ref().unwrap();
                assert_eq!(
                    noise.sample(x, y, z).to_bits(),
                    a,
                    "3D index={index} mode={mode} seed={seed} coords={x},{y},{z}: {}",
                    definitions[*index]
                );
                assert_eq!(
                    noise.sample_2d(x, z).to_bits(),
                    b,
                    "2D index={index} mode={mode} seed={seed} coords={x},{z}"
                );
                samples += 1;
            }
            _ => {}
        }
    }
    assert_eq!(groups, definitions.len() * 8);
    assert_eq!(samples, groups * 6);
}
