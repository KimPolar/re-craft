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
fn climate_index_and_vanilla_presets_match_java() {
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
    data.biomes
        .extend(stack.list_data("worldgen/biome").unwrap());
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
    let overworld_path =
        "data/minecraft/worldgen/multi_noise_biome_source_parameter_list/overworld.json";
    data.files.insert(
        overworld_path.into(),
        stack.read_builtin_data(overworld_path).unwrap().unwrap(),
    );
    sources.push(
        BiomeSource::load(
            &mut data,
            &json!({"type":"multi_noise","preset":"minecraft:overworld"}),
        )
        .unwrap(),
    );
    groups.push(json!({"overworld":true}));
    let mut targets = Vec::new();
    let boundaries = [
        -1.2_f32,
        -1.05,
        -1.0,
        -0.93333334,
        -0.7666667,
        -0.56666666,
        -0.455,
        -0.4,
        -0.26666668,
        -0.19,
        -0.11,
        -0.05,
        0.0,
        0.03,
        0.05,
        0.2,
        0.26666668,
        0.3,
        0.4,
        0.55,
        0.56666666,
        0.7666667,
        0.8,
        0.93333334,
        1.0,
        1.1,
    ];
    let mut state = 726_134_903_715_329_u64;
    for i in 0..3000 {
        let point: Vec<f32> = (0..6)
            .map(|axis| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                if i % 17 == 0 {
                    0.0
                } else if i % 7 == 0 {
                    boundaries[((state >> 32) as usize + axis) % boundaries.len()]
                } else {
                    ((state >> 32) as u32 % 60_001) as f32 / 20_000.0 - 1.5
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
    assert_eq!(group, Some(8));
    assert_eq!(samples, 9 * 3000);
}
