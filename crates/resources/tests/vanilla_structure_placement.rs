//! 26.3 structure candidate, frequency, exclusion and weighted-order oracle.
use anyhow::Result;
use native_resources::{
    packs::{ResourceId, ResourceProvider, ResourceStack},
    structure_placement::StructureSetCache,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, process::Command};
struct Data {
    builtin: ResourceStack,
    overrides: BTreeMap<String, Vec<u8>>,
}
impl ResourceProvider for Data {
    fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        if let Some(v) = self.overrides.get(path) {
            Ok(Some(v.clone()))
        } else {
            self.builtin.read_builtin_data(path)
        }
    }
}
fn run(command: &mut Command) -> String {
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "Java structure oracle failed: {}\n{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout[out.stdout.len().saturating_sub(3000)..])
    );
    String::from_utf8(out.stdout).unwrap()
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn random_spread_structure_sets_match_java() {
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
    let source = temp.path().join("StructurePlacementOracle.java");
    std::fs::write(&source, include_str!("java/StructurePlacementOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let mut data = Data {
        builtin: ResourceStack::open(&game).unwrap(),
        overrides: BTreeMap::new(),
    };
    let mut cases = Vec::<(ResourceId, Value)>::new();
    for id in data.builtin.list_data("worldgen/structure_set").unwrap() {
        let path = format!(
            "data/{}/worldgen/structure_set/{}.json",
            id.namespace, id.path
        );
        let bytes = data.builtin.read_builtin_data(&path).unwrap().unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        if value["placement"]["type"] == "minecraft:random_spread" {
            cases.push((id, value));
        }
    }
    assert!(cases.len() >= 19);
    for method in ["default", "legacy_type_1", "legacy_type_2", "legacy_type_3"] {
        for frequency in [0.0, 0.004, 0.5, 1.0] {
            let name = format!("{method}_{}", (frequency * 1000.0) as i32);
            let id = ResourceId::parse(&format!("test:{name}")).unwrap();
            let definition = json!({"placement":{"type":"minecraft:random_spread","spacing":9,"separation":2,"salt":51,"frequency":frequency,"frequency_reduction_method":method,"spread_type":"triangular","locate_offset":[-16,3,16]},
          "structures":[{"structure":"minecraft:first","weight":1},{"structure":"minecraft:second","weight":3},{"structure":"minecraft:third","weight":7}]});
            data.overrides.insert(
                format!("data/test/worldgen/structure_set/{name}.json"),
                serde_json::to_vec(&definition).unwrap(),
            );
            cases.push((id, definition));
        }
    }
    let seeds = [0i64, 1, -1, i64::MIN, i64::MAX];
    let mut fixtures = Vec::new();
    let mut discovery = StructureSetCache::default();
    for (id, set) in &cases {
        let has_exclusion = set["placement"].get("exclusion_zone").is_some();
        let mut points = Vec::new();
        for seed in seeds {
            for i in 0..24i32 {
                let (x, z) = if i == 0 && !has_exclusion {
                    (i32::MIN, i32::MAX)
                } else {
                    (
                        i.wrapping_mul(851).wrapping_sub(9281),
                        i.wrapping_mul(-431).wrapping_add(5092),
                    )
                };
                points.push(json!([seed, x, z]));
            }
        }
        if id.namespace == "minecraft" && id.path == "pillager_outposts" {
            let placement = discovery.load(&mut data, id).unwrap().placement.clone();
            let (mut allowed, mut excluded) = (0, 0);
            'search: for gx in -60..60 {
                for gz in -60..60 {
                    let candidate = placement.potential_chunk(42, [gx * 32, gz * 32]);
                    if placement.is_candidate_before_exclusion(42, candidate) {
                        let valid = discovery
                            .is_structure_chunk(&mut data, id, 42, candidate)
                            .unwrap();
                        if valid && allowed < 8 {
                            points.push(json!([42, candidate[0], candidate[1]]));
                            allowed += 1;
                        }
                        if !valid && excluded < 8 {
                            points.push(json!([42, candidate[0], candidate[1]]));
                            excluded += 1;
                        }
                        if allowed == 8 && excluded == 8 {
                            break 'search;
                        }
                    }
                }
            }
            assert_eq!(
                (allowed, excluded),
                (8, 8),
                "fixtures must cover both sides of village exclusion"
            );
        }
        fixtures.push(json!({"set":set,"points":points}));
    }
    let input = temp.path().join("cases.json");
    std::fs::write(&input, serde_json::to_vec(&fixtures).unwrap()).unwrap();
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("StructurePlacementOracle")
        .arg(&input));
    let mut lines = output
        .lines()
        .filter(|line| line == &"S" || line.starts_with("P ") || line.starts_with("O "));
    let mut cache = StructureSetCache::default();
    let mut samples = 0usize;
    for (group, (id, _)) in cases.iter().enumerate() {
        assert_eq!(lines.next(), Some("S"));
        let set = cache.load(&mut data, id).unwrap().clone();
        for point in fixtures[group]["points"].as_array().unwrap() {
            let seed = point[0].as_i64().unwrap();
            let source = [
                point[1].as_i64().unwrap() as i32,
                point[2].as_i64().unwrap() as i32,
            ];
            let p: Vec<_> = lines.next().unwrap().split_whitespace().collect();
            assert_eq!(p[0], "P");
            let candidate = set.placement.potential_chunk(seed, source);
            let locate = set.placement.locate_position(candidate);
            assert_eq!(
                (candidate[0], candidate[1]),
                (p[3].parse().unwrap(), p[4].parse().unwrap()),
                "candidate {id:?} {seed} {source:?}"
            );
            assert_eq!(
                set.placement.passes_frequency(seed, source),
                p[5] == "true",
                "frequency {id:?} {seed} {source:?}"
            );
            assert_eq!(
                cache
                    .is_structure_chunk(&mut data, id, seed, source)
                    .unwrap(),
                p[6] == "true",
                "source {id:?} {seed} {source:?}"
            );
            assert_eq!(
                cache
                    .is_structure_chunk(&mut data, id, seed, candidate)
                    .unwrap(),
                p[7] == "true",
                "candidate {id:?} {seed} {source:?}"
            );
            assert_eq!(
                locate,
                [
                    p[8].parse::<i32>().unwrap(),
                    p[9].parse::<i32>().unwrap(),
                    p[10].parse::<i32>().unwrap()
                ]
            );
            let order = lines.next().unwrap().strip_prefix("O ").unwrap();
            let expected = set
                .attempt_order(seed, source)
                .iter()
                .map(|id| format!("{}:{}", id.namespace, id.path))
                .collect::<Vec<_>>()
                .join(",");
            assert_eq!(expected, order, "structure order {id:?} {seed} {source:?}");
            samples += 1;
        }
    }
    assert!(samples >= 4000);
    assert!(lines.next().is_none());
}
