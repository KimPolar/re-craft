//! Compare biome-selected, combined carving masks against Java 26.3.
use anyhow::Result;
use native_resources::{
    carver::schedule::CarverCache,
    packs::{ResourceId, ResourceProvider, ResourceStack},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, process::Command};

#[derive(Default)]
struct Data(BTreeMap<String, Vec<Vec<u8>>>);
impl ResourceProvider for Data {
    fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.0.get(path).and_then(|v| v.last()).cloned())
    }
    fn read_data_stack(&mut self, path: &str) -> Result<Vec<Vec<u8>>> {
        Ok(self.0.get(path).cloned().unwrap_or_default())
    }
}
fn run(command: &mut Command) -> String {
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "oracle failed: {}\n{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout[out.stdout.len().saturating_sub(2000)..])
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn combined_masks_follow_biome_lists_and_source_order() {
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
    let source = temp.path().join("CarverScheduleOracle.java");
    std::fs::write(&source, include_str!("java/CarverScheduleOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let mut installed = ResourceStack::open(&game).unwrap();
    let mut data = Data::default();
    let mut configs = Vec::new();
    for name in ["cave", "cave_extra_underground", "canyon"] {
        let path = format!("data/minecraft/worldgen/carver/{name}.json");
        let bytes = installed.read_builtin_data(&path).unwrap().unwrap();
        configs.push(serde_json::from_slice::<Value>(&bytes).unwrap());
        data.0.insert(path, vec![bytes]);
    }
    let lists = vec![
        vec![configs[0].clone(), configs[1].clone(), configs[2].clone()],
        vec![],
        vec![configs[2].clone(), configs[0].clone()],
    ];
    for (index, carvers) in [
        json!([
            "minecraft:cave",
            "minecraft:cave_extra_underground",
            "minecraft:canyon"
        ]),
        json!([]),
        json!("#example:chosen"),
    ]
    .iter()
    .enumerate()
    {
        data.0.insert(
            format!("data/example/worldgen/biome/b{index}.json"),
            vec![serde_json::to_vec(&json!({"carvers":carvers})).unwrap()],
        );
    }
    data.0.insert("data/example/tags/worldgen/carver/chosen.json".into(),vec![
        br#"{"values":["minecraft:cave_extra_underground"]}"#.to_vec(),
        br##"{"replace":true,"values":["minecraft:canyon","#example:nested","minecraft:canyon",{"id":"example:missing","required":false}]}"##.to_vec()]);
    data.0.insert(
        "data/example/tags/worldgen/carver/nested.json".into(),
        vec![br#"{"values":["minecraft:cave"]}"#.to_vec()],
    );
    let cases = vec![
        json!({"x":0,"z":0,"seed":0,"min_y":-64,"height":384}),
        json!({"x":-17,"z":32,"seed":2345,"min_y":-64,"height":384}),
        json!({"x":33,"z":-61,"seed":-987654321,"min_y":-64,"height":384}),
    ];
    let input = temp.path().join("input.json");
    std::fs::write(
        &input,
        serde_json::to_vec(&json!({"biomes":lists,"cases":cases})).unwrap(),
    )
    .unwrap();
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("CarverScheduleOracle")
        .arg(&input));
    let mut expected = Vec::<Vec<(u8, u8, i32, i32)>>::new();
    for line in output.lines() {
        let p: Vec<_> = line.split_whitespace().collect();
        match p.first().copied() {
            Some("S") => expected.push(Vec::new()),
            Some("P") => expected.last_mut().unwrap().push((
                p[1].parse().unwrap(),
                p[2].parse().unwrap(),
                p[3].parse().unwrap(),
                p[4].parse().unwrap(),
            )),
            _ => {}
        }
    }
    assert_eq!(expected.len(), cases.len());
    let mut cache = CarverCache::default();
    for (i, c) in cases.iter().enumerate() {
        let target = [
            c["x"].as_i64().unwrap() as i32,
            c["z"].as_i64().unwrap() as i32,
        ];
        let mut calls = Vec::new();
        let mask = cache
            .generate(
                &mut data,
                c["seed"].as_i64().unwrap(),
                target,
                -64,
                384,
                |source| {
                    calls.push(source);
                    ResourceId::parse(&format!(
                        "example:b{}",
                        (source[0] + 2 * source[1]).rem_euclid(3)
                    ))
                },
            )
            .unwrap();
        assert_eq!(calls.len(), 289);
        assert_eq!(calls[0], [target[0] - 8, target[1] - 8]);
        assert_eq!(calls[1], [target[0] - 8, target[1] - 7]);
        assert_eq!(mask.segments(), expected[i], "case {i}");
        assert!(!mask.is_empty());
    }
}
