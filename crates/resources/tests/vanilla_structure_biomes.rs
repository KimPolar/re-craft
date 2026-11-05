//! Compare structure biome holders and per-dimension set filtering to Java.
use native_resources::{
    biome_source::BiomeSource,
    packs::{ResourceId, ResourceProvider, ResourceStack},
    structure_biomes::StructureBiomes,
    structure_placement::{StructureEntry, StructureSetCache, decode_structure_entries},
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    process::Command,
};
fn run(command: &mut Command) -> String {
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "Java structure-biome oracle failed: {}\n{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout[..out.stdout.len().min(16000)])
    );
    String::from_utf8(out.stdout).unwrap()
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn structure_biome_sets_match_java_in_all_dimensions() {
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
    let source = temp.path().join("StructureBiomesOracle.java");
    std::fs::write(&source, include_str!("java/StructureBiomesOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("StructureBiomesOracle")
        .arg(temp.path()));
    let mut resources = ResourceStack::open(&game).unwrap();
    let preset: Value = serde_json::from_slice(
        &resources
            .read_builtin_data("data/minecraft/worldgen/world_preset/normal.json")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let mut possible = BTreeMap::new();
    for dim in [
        "minecraft:overworld",
        "minecraft:the_nether",
        "minecraft:the_end",
    ] {
        let source = BiomeSource::load(
            &mut resources,
            &preset["dimensions"][dim]["generator"]["biome_source"],
        )
        .unwrap();
        possible.insert(dim.to_owned(), source.possible_biomes());
    }
    let mut sets = BTreeMap::<ResourceId, Vec<StructureEntry>>::new();
    let mut all = BTreeSet::new();
    for id in resources.list_data("worldgen/structure_set").unwrap() {
        let path = format!(
            "data/{}/worldgen/structure_set/{}.json",
            id.namespace, id.path
        );
        let entries =
            decode_structure_entries(&resources.read_builtin_data(&path).unwrap().unwrap())
                .unwrap();
        all.extend(entries.iter().map(|e| e.id.clone()));
        sets.insert(id, entries);
    }
    let biome_rules = StructureBiomes::load(&mut resources, all).unwrap();
    let mut java_biomes = BTreeMap::<String, BTreeSet<ResourceId>>::new();
    let mut java_selected = BTreeMap::<String, Vec<ResourceId>>::new();
    let mut java_candidates = BTreeMap::<String, Vec<ResourceId>>::new();
    let mut java_runtime_selected = BTreeMap::<String, Vec<ResourceId>>::new();
    let mut runtime_order = Vec::new();
    let mut checks = 0;
    let mut set_checks = 0;
    for line in output.lines() {
        let p: Vec<_> = line.split_whitespace().collect();
        match p.first().copied() {
            Some("R") => runtime_order.push(ResourceId::parse(p[1]).unwrap()),
            Some("Q") if p[3] == "true" => java_runtime_selected
                .entry(p[1].to_owned())
                .or_default()
                .push(ResourceId::parse(p[2]).unwrap()),
            Some("B") => {
                java_biomes
                    .entry(p[1].to_owned())
                    .or_default()
                    .insert(ResourceId::parse(p[2]).unwrap());
            }
            Some("S") => {
                let id = ResourceId::parse(p[2]).unwrap();
                java_candidates
                    .entry(p[1].to_owned())
                    .or_default()
                    .push(id.clone());
                if p[3] == "true" {
                    java_selected
                        .entry(p[1].to_owned())
                        .or_default()
                        .push(id.clone());
                }
                assert_eq!(
                    biome_rules
                        .set_has_possible_biome(&sets[&id], &possible[p[1]])
                        .unwrap(),
                    p[3] == "true",
                    "set {} in {}",
                    p[2],
                    p[1]
                );
                set_checks += 1;
            }
            Some("P") => {
                let id = ResourceId::parse(p[1]).unwrap();
                let biome = ResourceId::parse(p[2]).unwrap();
                assert_eq!(
                    biome_rules.permits(&id, &biome).unwrap(),
                    p[3] == "true",
                    "structure {} biome {}",
                    p[1],
                    p[2]
                );
                checks += 1;
            }
            _ => {}
        }
    }
    assert_eq!(java_biomes, possible);
    assert_eq!(
        runtime_order,
        resources
            .list_registry_data("worldgen/structure_set")
            .unwrap()
    );
    for (dimension, biomes) in &possible {
        let mut cache = StructureSetCache::default();
        let (selected, _) = cache
            .possible_registry_membership(&mut resources, biomes)
            .unwrap();
        assert_eq!(
            selected,
            java_selected[dimension].iter().cloned().collect(),
            "registry membership in {dimension}"
        );
        let (ordered, _) = cache
            .possible_sets(&mut resources, &java_candidates[dimension], biomes)
            .unwrap();
        assert_eq!(
            ordered, java_selected[dimension],
            "observed registry order in {dimension}"
        );
        let expected_runtime: Vec<_> = runtime_order
            .iter()
            .filter(|id| selected.contains(*id))
            .cloned()
            .collect();
        assert_eq!(
            expected_runtime, java_runtime_selected[dimension],
            "Java resource-loaded set selection in {dimension}"
        );
        let (ordered_runtime, _) = cache
            .possible_sets(&mut resources, &runtime_order, biomes)
            .unwrap();
        assert_eq!(
            ordered_runtime, expected_runtime,
            "resource-loaded registry order in {dimension}"
        );
        let (automatic, _) = cache
            .possible_registry_sets(&mut resources, biomes)
            .unwrap();
        assert_eq!(
            automatic, expected_runtime,
            "automatic registry order in {dimension}"
        );
    }
    assert!(set_checks >= 60);
    assert!(checks >= 1000);

    // A selected pack adds names in two namespaces and overrides a vanilla set.
    let world = temp.path().join("world");
    let pack = world.join("datapacks/overrides");
    std::fs::create_dir_all(&pack).unwrap();
    std::fs::write(
        pack.join("pack.mcmeta"),
        br#"{"pack":{"description":"structure test","min_format":121,"max_format":121}}"#,
    )
    .unwrap();
    for (namespace, name, structure) in [
        ("alpha", "a", "minecraft:buried_treasure"),
        ("beta", "a", "minecraft:buried_treasure"),
        ("minecraft", "buried_treasures", "minecraft:fortress"),
    ] {
        let folder = pack
            .join("data")
            .join(namespace)
            .join("worldgen/structure_set");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(format!("{name}.json")), serde_json::to_vec(&serde_json::json!({"placement":{"type":"minecraft:random_spread","spacing":32,"separation":8,"salt":42},"structures":[{"structure":structure,"weight":1}]})).unwrap()).unwrap();
    }
    let custom_output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("StructureBiomesOracle")
        .arg(world.join("datapacks"))
        .arg("custom"));
    let mut custom_order = Vec::new();
    let mut custom_selected = BTreeMap::<String, Vec<ResourceId>>::new();
    for line in custom_output.lines() {
        let parts: Vec<_> = line.split_whitespace().collect();
        match parts.first().copied() {
            Some("R") => custom_order.push(ResourceId::parse(parts[1]).unwrap()),
            Some("Q") if parts[3] == "true" => custom_selected
                .entry(parts[1].to_owned())
                .or_default()
                .push(ResourceId::parse(parts[2]).unwrap()),
            _ => {}
        }
    }
    let selection = native_resources::datapacks::DataPackSettings {
        enabled: vec!["vanilla".into(), "file/overrides".into()],
        disabled: vec![],
        enabled_features: vec!["minecraft:vanilla".into()],
    };
    assert!(
        resources
            .configure_world_data(&world, &selection)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        custom_order,
        resources
            .list_registry_data("worldgen/structure_set")
            .unwrap()
    );
    for (dimension, biomes) in &possible {
        let (automatic, _) = StructureSetCache::default()
            .possible_registry_sets(&mut resources, biomes)
            .unwrap();
        assert_eq!(
            automatic, custom_selected[dimension],
            "custom pack in {dimension}"
        );
    }
    assert!(
        custom_selected["minecraft:overworld"].contains(&ResourceId::parse("alpha:a").unwrap())
    );
    assert!(
        !custom_selected["minecraft:overworld"]
            .contains(&ResourceId::parse("minecraft:buried_treasures").unwrap())
    );
    assert!(
        custom_selected["minecraft:the_nether"]
            .contains(&ResourceId::parse("minecraft:buried_treasures").unwrap())
    );
}
