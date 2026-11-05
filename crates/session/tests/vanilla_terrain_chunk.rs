//! End-to-end noise, surface and carver stages versus Java's real generator.
use anyhow::{Context, Result};
use native_resources::{
    carver::{schedule::CarverCache, tags::CarverBlockTags},
    density::DensityEvaluator,
    noise_stage::NoiseMaterial,
    packs::{ResourceProvider, ResourceStack},
    surface_pass::{SurfaceBlockKind, SurfaceStageChunk},
};
use native_session::worldgen::{
    CarvedTerrainWorld, CarverBlockState, TerrainGenerator, materialize_carved_terrain,
    resolve_generated_terrain_state,
};
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
            "2345",
            "1",
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
            "custom",
            "2345",
            "0",
            "0",
        ]));
    let cases = [
        ("minecraft:overworld", 2345, [0, 0], -64, 384),
        ("minecraft:overworld", 2345, [1, 0], -64, 384),
        ("minecraft:overworld", 0, [-17, 32], -64, 384),
        ("minecraft:overworld", -1, [64, -23], -64, 384),
        ("minecraft:overworld", 12345, [-100, 17], -64, 384),
        ("minecraft:the_nether", 42, [0, 0], 0, 256),
        ("minecraft:the_nether", -1, [100, -70], 0, 256),
        ("minecraft:the_end", 42, [0, 0], 0, 256),
        ("minecraft:the_end", -1, [100, -70], 0, 256),
        ("minecraft:overworld", 2345, [0, 0], -64, 384),
    ];
    let mut groups = Vec::<Vec<Vec<usize>>>::new();
    let mut registry = BTreeSet::new();
    let mut defaults = BTreeMap::<String, BTreeMap<String, String>>::new();
    let mut palette = Vec::new();
    let mut updates = Vec::<Vec<u64>>::new();
    let mut ordered_updates = Vec::<BTreeMap<usize, Vec<u16>>>::new();
    let mut probes = Vec::<BTreeMap<usize, Vec<i16>>>::new();
    for line in output.lines() {
        if line == "QBEGIN" {
            probes.push(BTreeMap::new());
        } else if let Some(line) = line.strip_prefix("Q ") {
            let mut words = line.split_whitespace();
            let section = words.next().unwrap().parse().unwrap();
            let values = words.map(|value| value.parse().unwrap()).collect();
            assert!(probes.last_mut().unwrap().insert(section, values).is_none());
        } else if line.starts_with("S ") {
            groups.push(Vec::new());
            ordered_updates.push(BTreeMap::new());
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
        } else if let Some(line) = line.strip_prefix("O ") {
            let mut words = line.split_whitespace();
            let section = words.next().unwrap().parse().unwrap();
            let values = words.map(|value| value.parse().unwrap()).collect();
            assert!(
                ordered_updates
                    .last_mut()
                    .unwrap()
                    .insert(section, values)
                    .is_none()
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
    assert_eq!(groups.len(), cases.len() * 3);
    assert_eq!(probes.len(), cases.len());
    assert_eq!(updates.len(), groups.len());
    let mut total = 0;
    let mut duplicates = 0;
    for stage in &ordered_updates {
        for values in stage.values() {
            let unique: BTreeSet<_> = values.iter().collect();
            total += values.len();
            duplicates += values.len() - unique.len();
        }
    }
    eprintln!("Java postprocessing entries: {total}, duplicate entries: {duplicates}");
    assert!(registry.len() > 1000);
    let mut resources = ResourceStack::open(&game).unwrap();
    let generated_registry = resources.vanilla_generated_block_registry().unwrap();
    let stone =
        resolve_generated_terrain_state(&serde_json::json!("stone"), &generated_registry).unwrap();
    assert_eq!(stone.name, "minecraft:stone");
    assert!(stone.properties.is_empty());
    assert!(
        resolve_generated_terrain_state(
            &serde_json::json!({"id":"minecraft:stone","properties":{}}),
            &generated_registry
        )
        .is_ok()
    );
    assert!(
        resolve_generated_terrain_state(
            &serde_json::json!({"id":"minecraft:stone","properties":{"unknown":"x"}}),
            &generated_registry
        )
        .is_ok()
    );
    assert!(
        resolve_generated_terrain_state(&serde_json::json!("minecraft:water"), &generated_registry)
            .unwrap()
            .properties
            .get("level")
            .map(String::as_str)
            == Some("0")
    );
    assert!(
        resolve_generated_terrain_state(&serde_json::json!("example:stone"), &generated_registry)
            .is_err()
    );
    let preset: Value = serde_json::from_slice(
        &resources
            .read_builtin_data("data/minecraft/worldgen/world_preset/normal.json")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let tags = CarverBlockTags::load(&mut resources, &registry).unwrap();
    let mut assembled = None;
    for (i, (dimension, seed, pos, min_y, height)) in cases.into_iter().enumerate() {
        let mut probe = native_core::postprocessing::PostProcessing::new(min_y, height).unwrap();
        let bx = pos[0] * 16;
        let bz = pos[1] * 16;
        for (x, y, z) in [
            (bx - 1, min_y, bz - 1),
            (bx, min_y + 15, bz),
            (bx - 1, min_y, bz - 1),
            (bx + 2, min_y + 16, bz + 3),
            (bx, min_y - 1, bz),
            (bx, min_y + height, bz),
        ] {
            probe.mark(x, y, z);
        }
        for (section, values) in probe.sections().iter().enumerate() {
            assert_eq!(
                values.as_slice(),
                probes[i].get(&section).map(Vec::as_slice).unwrap_or(&[])
            );
        }
        let mut generator_definition = preset["dimensions"][dimension]["generator"].clone();
        if i == cases.len() - 1 {
            let bytes = resources
                .read_builtin_data("data/minecraft/worldgen/noise_settings/overworld.json")
                .unwrap()
                .unwrap();
            let mut settings: Value = serde_json::from_slice(&bytes).unwrap();
            settings["material_rule"] = serde_json::json!({"type":"minecraft:condition",
                "if_true":{"type":"minecraft:y_above", "anchor":{"absolute":-60}, "surface_depth_multiplier":0, "add_stone_depth":false},
                "then_run":{"type":"minecraft:block", "result_state":{"id":"minecraft:water"}}});
            generator_definition["settings"] = settings;
        }
        let mut generator = TerrainGenerator::without_terrain_adjustments(
            &mut resources,
            &generator_definition,
            seed,
            min_y,
            height,
        )
        .unwrap();
        let mut evaluator = DensityEvaluator::default();
        let noise = generator.generate_noise(&mut evaluator, pos).unwrap();
        let marks = noise
            .ordered_fluid_postprocessing(|material| {
                let value = match material {
                    NoiseMaterial::DefaultBlock => noise.default_block().clone(),
                    NoiseMaterial::DefaultFluid => noise.default_fluid().clone(),
                    NoiseMaterial::Lava => serde_json::json!("minecraft:lava"),
                    NoiseMaterial::Air => serde_json::json!("minecraft:air"),
                };
                Ok(classify(&value)?.has_fluid)
            })
            .unwrap();
        for (section, values) in marks.sections().iter().enumerate() {
            let expected: Vec<i16> = ordered_updates[i * 3]
                .get(&section)
                .map(|values| values.iter().map(|&value| value as i16).collect())
                .unwrap_or_default();
            assert_eq!(
                *values, expected,
                "noise postprocessing {dimension} {pos:?} section {section}"
            );
        }
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
        let surface_marks = chunk.surface_postprocessing.as_ref().unwrap().clone();
        let mut count = 0;
        for (section, values) in surface_marks.sections().iter().enumerate() {
            let expected: Vec<i16> = ordered_updates[i * 3 + 1]
                .get(&section)
                .map(|values| values.iter().map(|&value| value as i16).collect())
                .unwrap_or_default();
            count += expected.len();
            assert_eq!(
                *values, expected,
                "surface postprocessing {dimension} {seed} {pos:?} section {section}"
            );
        }
        if i == cases.len() - 1 {
            assert!(count > 0, "custom water rule must mark positions");
        }
        compare(
            &format!("surface {dimension} {seed} {pos:?}"),
            &chunk,
            &groups[i * 3 + 1],
            &palette,
            &defaults,
            &updates[i * 3 + 1],
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
        let carver_marks = chunk.carver_postprocessing.as_ref().unwrap().clone();
        let encoded = native_persistence::postprocessing::encode(&carver_marks);
        let decoded = native_persistence::postprocessing::decode(&encoded, min_y, height).unwrap();
        assert_eq!(
            decoded, carver_marks,
            "postprocessing NBT {dimension} {seed} {pos:?}"
        );
        for (section, values) in carver_marks.sections().iter().enumerate() {
            let expected: Vec<i16> = ordered_updates[i * 3 + 2]
                .get(&section)
                .map(|values| values.iter().map(|&value| value as i16).collect())
                .unwrap_or_default();
            assert_eq!(
                *values, expected,
                "carver postprocessing {dimension} {seed} {pos:?} section {section}"
            );
        }
        compare(
            &format!("carvers {dimension} {seed} {pos:?}"),
            &chunk,
            &groups[i * 3 + 2],
            &palette,
            &defaults,
            &updates[i * 3 + 2],
        );
        let biome_chunk = neighborhood.chunk(pos).unwrap();
        let generated = materialize_carved_terrain(chunk, biome_chunk, dimension, |state| {
            resolve_generated_terrain_state(state, &generated_registry)
        })
        .unwrap();
        if i == 0 {
            use native_session::worldgen::{FreshTerrainBatch, schedule::GenerationStage};
            use native_world::loaded::ChunkPos;
            let center = ChunkPos {
                x: pos[0],
                z: pos[1],
            };
            let scheduled_generator = TerrainGenerator::without_terrain_adjustments(
                &mut resources,
                &generator_definition,
                seed,
                min_y,
                height,
            )
            .unwrap();
            let batch_tags = CarverBlockTags::load(&mut resources, &registry).unwrap();
            let mut batch = FreshTerrainBatch::new_without_structures(
                scheduled_generator,
                batch_tags,
                dimension,
            )
            .unwrap();
            batch.request(center).unwrap();
            let mut classifier = classify;
            let mut resolver =
                |state: &Value| resolve_generated_terrain_state(state, &generated_registry);
            while batch
                .step(&mut resources, &mut classifier, &mut resolver)
                .unwrap()
                .is_some()
            {}
            let (schedule, _, scheduled_world, light) = batch.into_parts().unwrap();
            assert!(light.is_none());
            let scheduled = scheduled_world.unwrap().snapshot();
            assert_eq!(schedule.completed(center), Some(GenerationStage::Terrain));
            // An empty active set isolates the structure-aware batch plumbing
            // while retaining the same vanilla terrain as this Java fixture.
            let structure_generator = TerrainGenerator::with_structure_context(
                &mut resources,
                &generator_definition,
                seed,
                min_y,
                height,
            )
            .unwrap();
            let structure_biomes = native_resources::structure_biomes::StructureBiomes::load(
                &mut resources,
                std::iter::empty(),
            )
            .unwrap();
            let mut structures =
                native_session::worldgen::structure_batch::StructureGenerationBatch::new(
                    structure_generator,
                    structure_biomes,
                    vec![],
                )
                .unwrap();
            for dz in -1..=1 {
                for dx in -1..=1 {
                    structures
                        .request_references(ChunkPos {
                            x: center.x + dx,
                            z: center.z + dz,
                        })
                        .unwrap();
                }
            }
            while structures
                .step(&mut resources, &mut |_| {
                    panic!("empty structure sets must not classify blocks")
                })
                .unwrap()
                .is_some()
            {}
            let structure_tags = CarverBlockTags::load(&mut resources, &registry).unwrap();
            let mut structure_terrain = FreshTerrainBatch::new_with_unambiguous_structures(
                structures,
                [center],
                structure_tags,
                dimension,
            )
            .unwrap();
            structure_terrain.request(center).unwrap();
            while structure_terrain
                .step(&mut resources, &mut classifier, &mut resolver)
                .unwrap()
                .is_some()
            {}
            assert!(structure_terrain.is_complete());
            let structure_scheduled = structure_terrain.snapshot().unwrap();
            for y in min_y..min_y + height {
                for z in bz..bz + 16 {
                    for x in bx..bx + 16 {
                        assert_eq!(
                            scheduled.block(x, y, z),
                            generated.world().block(x, y, z),
                            "scheduled terrain at {x},{y},{z}"
                        );
                        assert_eq!(
                            scheduled.biome(x, y, z),
                            generated.world().biome(x, y, z),
                            "scheduled biome at {x},{y},{z}"
                        );
                        assert_eq!(
                            structure_scheduled.block(x, y, z),
                            generated.world().block(x, y, z),
                            "structure-aware terrain at {x},{y},{z}"
                        );
                        assert_eq!(
                            structure_scheduled.biome(x, y, z),
                            generated.world().biome(x, y, z),
                            "structure-aware biome at {x},{y},{z}"
                        );
                    }
                }
            }
        }
        assert_eq!(generated.noise_postprocessing(), Some(&marks));
        assert_eq!(generated.surface_postprocessing(), Some(&surface_marks));
        assert_eq!(generated.carver_postprocessing(), Some(&carver_marks));
        let world = generated.world();
        let mut names = BTreeMap::new();
        let expected = &groups[i * 3 + 2];
        let words = &updates[i * 3 + 2];
        for z in 0..16usize {
            for x in 0..16usize {
                for y in 0..height as usize {
                    let index = (z * 16 + x) * height as usize + y;
                    let state = world
                        .block(
                            pos[0] * 16 + x as i32,
                            min_y + y as i32,
                            pos[1] * 16 + z as i32,
                        )
                        .unwrap();
                    let key = names.entry(state.clone()).or_insert_with(|| {
                        let mut value = format!("{}|", state.name);
                        for (name, property) in &state.properties {
                            value.push_str(&format!("{name}={property};"));
                        }
                        value
                    });
                    assert_eq!(
                        *key,
                        palette[expected[z * 16 + x][y]],
                        "materialized {dimension} {pos:?} {x} {y} {z}"
                    );
                    assert_eq!(
                        generated.fluid_postprocessing()[index],
                        words
                            .get(index / 64)
                            .is_some_and(|word| word & (1u64 << (index % 64)) != 0)
                    );
                }
            }
        }
        for z in 0..4usize {
            for x in 0..4usize {
                for y in 0..height as usize / 4 {
                    let biome = &biome_chunk.values()[(z * 4 + x) * (height as usize / 4) + y];
                    assert_eq!(
                        world.biome(
                            pos[0] * 16 + (x * 4) as i32,
                            min_y + (y * 4) as i32,
                            pos[1] * 16 + (z * 4) as i32
                        ),
                        Some(format!("{}:{}", biome.namespace, biome.path).as_str())
                    );
                }
            }
        }
        if i == 0 {
            assembled = Some(CarvedTerrainWorld::new(generated));
        } else if i == 1 {
            let assembled = assembled.as_mut().unwrap();
            let previous = assembled.snapshot();
            let flags = generated.fluid_postprocessing().to_vec();
            assembled.insert(&generated).unwrap();
            let combined = assembled.snapshot();
            let first = native_world::loaded::ChunkPos { x: 0, z: 0 };
            let second = native_world::loaded::ChunkPos { x: 1, z: 0 };
            assert_eq!(assembled.noise_postprocessing(second), Some(&marks));
            assert_eq!(assembled.carver_postprocessing(second), Some(&carver_marks));
            assert_eq!(
                assembled.surface_postprocessing(second),
                Some(&surface_marks)
            );
            assert_eq!(combined.chunks.len(), 2);
            assert!(Arc::ptr_eq(
                &previous.chunks[&first],
                &combined.chunks[&first]
            ));
            assert_eq!(
                assembled.fluid_postprocessing(second),
                Some(flags.as_slice())
            );
            for z in 0..16 {
                for x in 0..16 {
                    for y in min_y..min_y + height {
                        assert_eq!(combined.block(16 + x, y, z), world.block(16 + x, y, z));
                        assert_eq!(combined.biome(16 + x, y, z), world.biome(16 + x, y, z));
                    }
                }
            }
        }
    }
}
