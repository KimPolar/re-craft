//! Cave masks compared with the installed Java 26.3 implementation.
use native_resources::{
    carver::{CarvingMask, cave::CaveCarver},
    packs::{ResourceProvider, ResourceStack},
};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};

fn run(command: &mut Command) -> String {
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "Java cave oracle failed ({:?}): {} stdout tail: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout[out.stdout.len().saturating_sub(2000)..])
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn cave_masks_match_vanilla() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"].as_array().unwrap() {
        if let Some(p) = lib["downloads"]["artifact"]["path"].as_str() {
            let p = game.join("libraries").join(p);
            if p.is_file() {
                paths.push(p);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("CaveCarverOracle.java");
    std::fs::write(&source, include_str!("java/CaveCarverOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let mut resources = ResourceStack::open(&game).unwrap();
    let mut cases = Vec::new();
    for name in ["cave", "cave_extra_underground", "nether_cave"] {
        let data = resources
            .read_builtin_data(&format!("data/minecraft/worldgen/carver/{name}.json"))
            .unwrap()
            .unwrap();
        let config: Value = serde_json::from_slice(&data).unwrap();
        let (min, height) = if name == "nether_cave" {
            (0, 128)
        } else {
            (-64, 384)
        };
        for i in 0..180i32 {
            let source = [(i * 19) % 61 - 30, (i * 37) % 73 - 36];
            let target = [source[0] + (i % 5) - 2, source[1] + ((i * 3) % 5) - 2];
            cases.push(json!({"config":config,"source":source,"target":target,"min_y":min,"height":height,
                "seed":i64::from(i).wrapping_mul(910_392_097_281).wrapping_sub(992_283_788_121),"index":i%4}));
        }
        if name == "cave" {
            for variant in 0..3 {
                let mut custom = config.clone();
                custom["probability"] = json!(1.0);
                custom["count"] = match variant {
                    0 => json!(2),
                    1 => json!({"type":"minecraft:uniform","min_inclusive":1,"max_inclusive":4}),
                    _ => {
                        json!({"type":"minecraft:biased_to_bottom","min_inclusive":1,"max_inclusive":5})
                    }
                };
                custom["thickness"] = json!(2.0);
                custom["y"] = json!({"absolute":42});
                custom["horizontal_radius_multiplier"] = json!(1.0);
                custom["vertical_radius_multiplier"] = json!(1.0);
                custom["floor_level"] = json!(-0.5);
                for i in 0..25i32 {
                    cases.push(
                        json!({"config":custom,"source":[i-12, 5-i],"target":[i-12, 5-i],
                        "min_y":min,"height":height,"seed":i64::from(i)*891247+17,"index":variant}),
                    );
                }
            }
        }
    }
    let input = temp.path().join("cases.json");
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("CaveCarverOracle")
        .arg(&input));
    let mut groups = Vec::<(bool, Vec<(u8, u8, i32, i32)>)>::new();
    for line in output.lines() {
        let p: Vec<_> = line.split_whitespace().collect();
        match p.first().copied() {
            Some("S") => groups.push((p[1] == "true", Vec::new())),
            Some("P") => groups.last_mut().unwrap().1.push((
                p[1].parse().unwrap(),
                p[2].parse().unwrap(),
                p[3].parse().unwrap(),
                p[4].parse().unwrap(),
            )),
            _ => {}
        }
    }
    assert_eq!(groups.len(), cases.len());
    let mut started = 0;
    let mut populated = 0;
    for (i, v) in cases.iter().enumerate() {
        let config = serde_json::to_vec(&v["config"]).unwrap();
        let carver = CaveCarver::decode(&config).unwrap();
        let min = v["min_y"].as_i64().unwrap() as i32;
        let height = v["height"].as_i64().unwrap() as i32;
        let mut mask = CarvingMask::new(min + 1, min + height - 8).unwrap();
        let pos = |key: &str| {
            [
                v[key][0].as_i64().unwrap() as i32,
                v[key][1].as_i64().unwrap() as i32,
            ]
        };
        let start = carver
            .carve_source(
                v["seed"].as_i64().unwrap(),
                v["index"].as_i64().unwrap() as i32,
                pos("source"),
                pos("target"),
                min,
                height,
                &mut mask,
            )
            .unwrap();
        assert_eq!(start, groups[i].0, "start case {i}");
        assert_eq!(mask.segments(), groups[i].1, "mask case {i}");
        started += usize::from(start);
        populated += usize::from(!mask.is_empty());
    }
    assert!(
        started > 25 && populated > 5,
        "fixtures need populated start chunks: {started}, {populated}"
    );
}
