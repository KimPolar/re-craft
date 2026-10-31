//! Differential checks for 26.3's climate R-tree and its tie handling.
use anyhow::Result;
use native_resources::{
    biome_source::BiomeSource,
    packs::{ResourceId, ResourceProvider, ResourceStack},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, process::Command};

struct Data {
    biomes: Vec<ResourceId>,
    files: BTreeMap<String, Vec<u8>>,
}
impl ResourceProvider for Data {
    fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.files.get(path).cloned())
    }
    fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
        assert_eq!(folder, "worldgen/biome");
        Ok(self.biomes.clone())
    }
}
fn run(command: &mut Command) -> String {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "Java biome oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn climate_index_and_nether_preset_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for library in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("BiomeSourceOracle.java");
    std::fs::write(&source, include_str!("java/BiomeSourceOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let mut stack = ResourceStack::open(&game).unwrap();
    let path = "data/minecraft/worldgen/multi_noise_biome_source_parameter_list/nether.json";
    let mut data = Data {
        biomes: Vec::new(),
        files: BTreeMap::from([(path.into(), stack.read_builtin_data(path).unwrap().unwrap())]),
    };
    for name in [
        "nether_wastes",
        "soul_sand_valley",
        "crimson_forest",
        "warped_forest",
        "basalt_deltas",
    ] {
        data.biomes.push(ResourceId::parse(name).unwrap());
    }
    let mut groups = vec![json!({"nether":true})];
    let mut sources = vec![
        BiomeSource::load(
            &mut data,
            &json!({"type":"multi_noise","preset":"minecraft:nether"}),
        )
        .unwrap(),
    ];
    for count in [1, 5, 19, 20, 361, 380, 1000] {
        let mut entries = Vec::new();
        for i in 0..count {
            let biome = format!("example:b{i}");
            data.biomes.push(ResourceId::parse(&biome).unwrap());
            let p = |axis: i32| ((i * (axis * 2 + 3) + axis * 7) % 17 - 8) as f32 / 10.0;
            let width = if i % 4 == 0 { 0.1 } else { 0.0 };
            entries.push(json!({"biome":biome,"parameters":{
                "temperature":[p(0)-width,p(0)+width],"humidity":{"min":p(1),"max":p(1)},
                "continentalness":p(2),"erosion":p(3),"depth":p(4),"weirdness":p(5),
                "offset":(i%4) as f32 / 10.0
            }}));
        }
        let value = json!({"type":"multi_noise","biomes":entries});
        sources.push(BiomeSource::load(&mut data, &value).unwrap());
        groups.push(value);
    }
    let mut targets = Vec::new();
    for i in 0..800 {
        let point: Vec<f32> = (0..6)
            .map(|axis| {
                if i % 7 == 0 {
                    0.0
                } else {
                    ((i * (axis * 2 + 5) + axis * 11) % 31 - 15) as f32 / 10.0
                }
            })
            .collect();
        targets.push(point);
    }
    let input = temp.path().join("cases.json");
    std::fs::write(
        &input,
        serde_json::to_vec(&json!({"groups":groups,"targets":targets})).unwrap(),
    )
    .unwrap();
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("BiomeSourceOracle")
        .arg(&input));
    let mut group = None;
    let mut index = 0;
    let mut samples = 0;
    for line in output.lines() {
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("S") => {
                group = Some(group.map_or(0, |v| v + 1));
                index = 0;
            }
            Some("P") => {
                let point: [f32; 6] = targets[index].clone().try_into().unwrap();
                let selected = sources[group.unwrap()].select(point);
                let actual = format!("{}:{}", selected.namespace, selected.path);
                assert_eq!(
                    actual,
                    parts.next().unwrap(),
                    "group {} target {}",
                    group.unwrap(),
                    index
                );
                index += 1;
                samples += 1;
            }
            _ => {}
        }
    }
    assert_eq!(group, Some(7));
    assert_eq!(samples, 8 * 800);
}
