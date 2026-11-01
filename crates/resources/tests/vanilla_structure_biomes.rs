//! Compare structure biome holders and per-dimension set filtering to Java.
use native_resources::{
    biome_source::BiomeSource,
    packs::{ResourceId, ResourceProvider, ResourceStack},
    structure_biomes::StructureBiomes,
    structure_placement::{StructureEntry, decode_structure_entries},
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
        String::from_utf8_lossy(&out.stdout[out.stdout.len().saturating_sub(2500)..])
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
    let mut checks = 0;
    let mut set_checks = 0;
    for line in output.lines() {
        let p: Vec<_> = line.split_whitespace().collect();
        match p.first().copied() {
            Some("B") => {
                java_biomes
                    .entry(p[1].to_owned())
                    .or_default()
                    .insert(ResourceId::parse(p[2]).unwrap());
            }
            Some("S") => {
                let id = ResourceId::parse(p[2]).unwrap();
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
    assert!(set_checks >= 60);
    assert!(checks >= 1000);
}
