//! End-to-end noise, surface and carver stages versus Java's real generator.
use anyhow::{Context, Result};
use native_resources::{
    carver::{schedule::CarverCache, tags::CarverBlockTags},
    density::DensityEvaluator,
    packs::{ResourceProvider, ResourceStack},
    surface_pass::{SurfaceBlockKind, SurfaceStageChunk},
};
use native_session::worldgen::{CarverBlockState, TerrainGenerator};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::PathBuf,
    process::Command,
    sync::Arc,
};

fn run(command: &mut Command) -> String {
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "terrain oracle failed: {}\n{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout[out.stdout.len().saturating_sub(3000)..])
    );
    String::from_utf8(out.stdout).unwrap()
}
fn classify(state: &Value) -> Result<CarverBlockState> {
    let id = state
        .as_str()
        .or_else(|| state.get("id").and_then(Value::as_str))
        .context("missing state id")?;
    let waterlogged = state
        .get("properties")
        .and_then(|p| p.get("waterlogged"))
        .and_then(Value::as_str)
        == Some("true");
    Ok(CarverBlockState {
        is_air: matches!(
            id,
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
        ),
        has_fluid: matches!(id, "minecraft:water" | "minecraft:lava") || waterlogged,
    })
}
fn compare(
    stage: &str,
    chunk: &SurfaceStageChunk,
    expected: &[Vec<usize>],
    palette: &[String],
    defaults: &BTreeMap<String, BTreeMap<String, String>>,
    scheduled: &[u64],
) {
    assert_eq!(expected.len(), 256);
    let mut count = 0;
    let mut samples = Vec::new();
    let mut cache = HashMap::<Value, String>::new();
    for z in 0..16usize {
        for x in 0..16usize {
            let column = z * 16 + x;
            assert_eq!(expected[column].len(), chunk.height as usize);
            for y in 0..chunk.height as usize {
                let state = &chunk.states[column * chunk.height as usize + y];
                let key = cache.entry((**state).clone()).or_insert_with(|| {
                    let raw = state
                        .as_str()
                        .or_else(|| state.get("id").and_then(Value::as_str))
                        .unwrap();
                    let id = if raw.contains(':') {
                        raw.to_owned()
                    } else {
                        format!("minecraft:{raw}")
                    };
                    let mut properties = defaults
                        .get(&id)
                        .unwrap_or_else(|| panic!("missing block {id}"))
                        .clone();
                    if let Some(extra) = state.get("properties").and_then(Value::as_object) {
                        for (name, value) in extra {
                            assert!(
                                properties.contains_key(name),
                                "invalid {id} property {name}"
                            );
                            properties.insert(name.clone(), value.as_str().unwrap().to_owned());
                        }
                    }
                    let mut key = format!("{id}|");
                    for (name, value) in properties {
                        key.push_str(&format!("{name}={value};"));
                    }
                    key
                });
                let java = &palette[expected[column][y]];
                if key != java {
                    count += 1;
                    if samples.len() < 12 {
                        samples.push(format!(
                            "({x},{},{z}) rust={key} java={java}",
                            chunk.min_y + y as i32
                        ));
                    }
                }
            }
        }
    }
    assert_eq!(
        count,
        0,
        "{stage} differs at {count} positions: {}",
        samples.join(", ")
    );
    let mut differences = Vec::new();
    let mut count = 0;
    for (index, &actual) in chunk.fluid_updates.iter().enumerate() {
        let expected = scheduled
            .get(index / 64)
            .is_some_and(|word| word & (1u64 << (index % 64)) != 0);
        if actual != expected {
            count += 1;
            if differences.len() < 12 {
                let column = index / chunk.height as usize;
                let x = column % 16;
                let z = column / 16;
                let y = chunk.min_y + (index % chunk.height as usize) as i32;
                differences.push(format!("({x},{y},{z}) rust={actual} java={expected}"));
            }
        }
    }
    assert_eq!(
        count,
        0,
        "{stage} fluid updates differ at {count} positions: {}",
        differences.join(", ")
    );
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn terrain_matches_java_before_and_after_carvers() {
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
                paths.push(p)
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("TerrainChunkOracle.java");
    std::fs::write(&source, include_str!("java/TerrainChunkOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("TerrainChunkOracle")
        .args([
            "overworld",
            "2345",
            "0",
            "0",
            "overworld",
            "0",
            "-17",
            "32",
            "overworld",
            "-1",
            "64",
            "-23",
            "overworld",
            "12345",
            "-100",
            "17",
            "nether",
            "42",
            "0",
            "0",
            "nether",
            "-1",
            "100",
            "-70",
            "end",
            "42",
            "0",
            "0",
            "end",
            "-1",
            "100",
            "-70",
        ]));
    let cases = [
        ("minecraft:overworld", 2345, [0, 0], -64, 384),
        ("minecraft:overworld", 0, [-17, 32], -64, 384),
        ("minecraft:overworld", -1, [64, -23], -64, 384),
        ("minecraft:overworld", 12345, [-100, 17], -64, 384),
        ("minecraft:the_nether", 42, [0, 0], 0, 256),
        ("minecraft:the_nether", -1, [100, -70], 0, 256),
        ("minecraft:the_end", 42, [0, 0], 0, 256),
        ("minecraft:the_end", -1, [100, -70], 0, 256),
    ];
    let mut groups = Vec::<Vec<Vec<usize>>>::new();
    let mut registry = BTreeSet::new();
    let mut defaults = BTreeMap::<String, BTreeMap<String, String>>::new();
    let mut palette = Vec::new();
    let mut updates = Vec::<Vec<u64>>::new();
    for line in output.lines() {
        if line.starts_with("S ") {
            groups.push(Vec::new());
        } else if let Some(line) = line.strip_prefix("P ") {
            let mut p = line.splitn(3, ' ');
            let _x = p.next().unwrap();
            let _z = p.next().unwrap();
            groups.last_mut().unwrap().push(
                p.next()
                    .unwrap()
                    .split(',')
                    .map(|v| v.parse().unwrap())
                    .collect(),
            );
        } else if let Some(line) = line.strip_prefix("L ") {
            let (index, key) = line.split_once(' ').unwrap();
            let index: usize = index.parse().unwrap();
            assert_eq!(index, palette.len());
            palette.push(key.to_owned());
        } else if line == "F" || line.starts_with("F ") {
            updates.push(
                line.split_whitespace()
                    .skip(1)
                    .map(|word| u64::from_str_radix(word, 16).unwrap())
                    .collect(),
            );
        } else if let Some(key) = line.strip_prefix("R ") {
            let (id, properties) = key.split_once('|').unwrap();
            registry.insert(id.to_owned());
            defaults.insert(
                id.to_owned(),
                properties
                    .split(';')
                    .filter(|s| !s.is_empty())
                    .map(|p| {
                        let (k, v) = p.split_once('=').unwrap();
                        (k.to_owned(), v.to_owned())
                    })
                    .collect(),
            );
        }
    }
    assert_eq!(groups.len(), cases.len() * 2);
    assert_eq!(updates.len(), groups.len());
    assert!(registry.len() > 1000);
    let mut resources = ResourceStack::open(&game).unwrap();
    let preset: Value = serde_json::from_slice(
        &resources
            .read_builtin_data("data/minecraft/worldgen/world_preset/normal.json")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let tags = CarverBlockTags::load(&mut resources, &registry).unwrap();
    for (i, (dimension, seed, pos, min_y, height)) in cases.into_iter().enumerate() {
        let mut generator = TerrainGenerator::without_terrain_adjustments(
            &mut resources,
            &preset["dimensions"][dimension]["generator"],
            seed,
            min_y,
            height,
        )
        .unwrap();
        let mut evaluator = DensityEvaluator::default();
        let mut neighborhood = generator.biome_neighborhood().unwrap();
        for x in pos[0] - 1..=pos[0] + 1 {
            for z in pos[1] - 1..=pos[1] + 1 {
                neighborhood
                    .insert(Arc::new(
                        generator.generate_biomes([x, z], &mut evaluator).unwrap(),
                    ))
                    .unwrap();
            }
        }
        let mut chunk = generator
            .generate_surface(
                &mut resources,
                &mut evaluator,
                pos,
                &neighborhood,
                |state| {
                    let f = classify(state)?;
                    Ok(if f.is_air {
                        SurfaceBlockKind::Air
                    } else if f.has_fluid {
                        SurfaceBlockKind::Fluid
                    } else {
                        SurfaceBlockKind::Solid
                    })
                },
            )
            .unwrap();
        compare(
            &format!("surface {dimension} {seed} {pos:?}"),
            &chunk,
            &groups[i * 2],
            &palette,
            &defaults,
            &updates[i * 2],
        );
        let mask = generator
            .generate_carving_mask(
                &mut resources,
                &mut evaluator,
                &mut CarverCache::default(),
                pos,
            )
            .unwrap();
        generator
            .apply_carving_to_surface(
                &mut resources,
                &mut evaluator,
                &mut chunk,
                &mask,
                &tags,
                classify,
            )
            .unwrap();
        compare(
            &format!("carvers {dimension} {seed} {pos:?}"),
            &chunk,
            &groups[i * 2 + 1],
            &palette,
            &defaults,
            &updates[i * 2 + 1],
        );
    }
}
