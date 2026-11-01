//! Real noise and biome checks for supported structure starts.
use native_persistence::heightmap::BlockHeightFlags;
use native_resources::{
    density::DensityEvaluator,
    packs::{ResourceId, ResourceProvider, ResourceStack},
    structure_biomes::StructureBiomes,
    structure_placement::{StructureSetCache, StructureSetStart},
};
use native_session::worldgen::TerrainGenerator;
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};
fn run(c: &mut Command) -> String {
    let o = c.output().unwrap();
    assert!(
        o.status.success(),
        "Java structure oracle failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8(o.stdout).unwrap()
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn structure_starts_use_vanilla_biomes_and_base_heights() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
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
    let source = temp.path().join("VanillaSimpleStartsOracle.java");
    std::fs::write(&source, include_str!("java/VanillaSimpleStartsOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let mut resources = ResourceStack::open(&game).unwrap();
    let preset: Value = serde_json::from_slice(
        &resources
            .read_builtin_data("data/minecraft/worldgen/world_preset/normal.json")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let generator = TerrainGenerator::with_structure_context(
        &mut resources,
        &preset["dimensions"]["minecraft:overworld"]["generator"],
        12345,
        -64,
        384,
    )
    .unwrap();
    let entries = [
        ("minecraft:desert_pyramids", "minecraft:desert_pyramid"),
        ("minecraft:jungle_temples", "minecraft:jungle_pyramid"),
        ("minecraft:swamp_huts", "minecraft:swamp_hut"),
        ("minecraft:buried_treasures", "minecraft:buried_treasure"),
    ];
    let rules = StructureBiomes::load(
        &mut resources,
        entries.iter().map(|(_, id)| ResourceId::parse(id).unwrap()),
    )
    .unwrap();
    let mut cache = StructureSetCache::default();
    let mut cases = Vec::new();
    for (set, structure) in entries {
        let set_id = ResourceId::parse(set).unwrap();
        let placement = cache
            .load(&mut resources, &set_id)
            .unwrap()
            .placement
            .clone();
        let mut found = 0;
        for i in 0..100_000i32 {
            let source = if set.ends_with("treasures") {
                [(i * 173) % 4096 - 2048, (i * 457) % 4093 - 2046]
            } else {
                [
                    ((i * 313) % 2000 - 1000) * 32,
                    ((i * 541) % 2003 - 1001) * 32,
                ]
            };
            let candidate = placement.potential_chunk(12345, source);
            if placement.is_candidate_before_exclusion(12345, candidate) {
                cases.push(
                    json!({"set":set,"structure":structure,"x":candidate[0],"z":candidate[1]}),
                );
                found += 1;
                if found == 128 {
                    break;
                }
            }
        }
        assert_eq!(found, 128, "{set}");
    }
    let input = temp.path().join("cases.json");
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("VanillaSimpleStartsOracle")
        .arg(temp.path())
        .arg(&input));
    let lines: Vec<_> = output.lines().filter(|l| l.starts_with("P ")).collect();
    assert_eq!(lines.len(), cases.len());
    let mut evaluator = DensityEvaluator::default();
    let mut generated = 0;
    let mut generated_by_type = std::collections::BTreeMap::new();
    for (case, line) in cases.iter().zip(lines) {
        let words: Vec<_> = line.split_whitespace().collect();
        let x = case["x"].as_i64().unwrap() as i32;
        let z = case["z"].as_i64().unwrap() as i32;
        assert_eq!(words[1], case["structure"].as_str().unwrap());
        let set = ResourceId::parse(case["set"].as_str().unwrap()).unwrap();
        let result = generator
            .try_generate_simple_structure_set(
                &mut resources,
                &mut evaluator,
                &mut cache,
                &rules,
                &set,
                [x, z],
                |_| Ok(false),
                |v| {
                    let name = v
                        .as_str()
                        .or_else(|| v.get("id").and_then(Value::as_str))
                        .unwrap();
                    Ok(BlockHeightFlags {
                        is_air: name == "minecraft:air",
                        blocks_motion: name == "minecraft:stone",
                        blocks_motion_no_leaves: name == "minecraft:stone",
                        has_fluid: matches!(name, "minecraft:water" | "minecraft:lava"),
                    })
                },
            )
            .unwrap();
        match (words[4], result) {
            ("X", StructureSetStart::Rejected) => {}
            ("S", StructureSetStart::Generated { structure, start }) => {
                generated += 1;
                *generated_by_type.entry(words[1]).or_insert(0usize) += 1;
                assert_eq!(
                    format!("{}:{}", structure.namespace, structure.path),
                    words[1]
                );
                let n = |i: usize| words[i].parse::<i32>().unwrap();
                assert_eq!(start.generation_point, [n(5), n(6), n(7)], "{line}");
                assert_eq!(start.bounds.min, [n(9), n(10), n(11)], "{line}");
                assert_eq!(start.bounds.max, [n(12), n(13), n(14)], "{line}");
                let dir = start
                    .orientation
                    .map(|d| format!("{d:?}").to_uppercase())
                    .unwrap_or_else(|| "NONE".into());
                assert_eq!(dir, words[8], "{line}");
            }
            (_, other) => panic!("{line}: {other:?}"),
        }
    }
    assert!(generated > 0, "fixtures must include generated starts");
    assert_eq!(
        generated_by_type.len(),
        4,
        "each type needs a valid example: {generated_by_type:?}"
    );
}
