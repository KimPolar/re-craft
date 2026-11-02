use anyhow::Result;
use native_core::random::DecorationRandom;
use native_resources::{
    disk_feature::{BlockPredicate, DiskWorld},
    ore_feature::OreWorld,
    packs::{ResourceId, ResourceProvider, ResourceStack},
    placed_feature::{PlacedFeature, PlacementEnvironment},
};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};

struct Environment;
impl OreWorld for Environment {
    type State = &'static str;
    fn block(&mut self, p: [i32; 3]) -> Result<Self::State> {
        Ok(match p[1].rem_euclid(5) {
            0 | 1 => "minecraft:air",
            2 | 3 => "minecraft:stone",
            _ => "minecraft:dirt",
        })
    }
    fn block_is(&mut self, s: &Self::State, id: &ResourceId) -> Result<bool> {
        Ok(*s == format!("{}:{}", id.namespace, id.path))
    }
    fn in_tag(&mut self, _: &Self::State, _: &ResourceId) -> Result<bool> {
        unreachable!()
    }
    fn is_air(&mut self, s: &Self::State) -> Result<bool> {
        Ok(*s == "minecraft:air")
    }
    fn set_block(&mut self, _: [i32; 3], _: &Self::State, _: i32) -> Result<bool> {
        unreachable!()
    }
}
impl DiskWorld for Environment {
    fn is_solid(&mut self, _: &Self::State) -> Result<bool> {
        unreachable!()
    }
    fn fluid_is(&mut self, _: &Self::State, _: &ResourceId) -> Result<bool> {
        unreachable!()
    }
    fn set_disk_block(&mut self, _: [i32; 3], _: &Self::State) -> Result<bool> {
        unreachable!()
    }
    fn mark_postprocessing(&mut self, _: [i32; 3]) -> Result<()> {
        unreachable!()
    }
    fn inside_build_height(&mut self, y: i32) -> Result<bool> {
        Ok((-4..5).contains(&y))
    }
}
impl PlacementEnvironment for Environment {
    fn min_y(&self) -> i32 {
        -4
    }
    fn inside_build_height(&self, y: i32) -> Result<bool> {
        Ok((-4..5).contains(&y))
    }
    fn heightmap(&mut self, _: &str, _: i32, _: i32) -> Result<i32> {
        unreachable!()
    }
    fn biome_allows(&mut self, _: [i32; 3]) -> Result<bool> {
        unreachable!()
    }
    fn block_predicate(&mut self, predicate: &BlockPredicate, pos: [i32; 3]) -> Result<bool> {
        predicate.test(self, pos)
    }
}
fn target(kind: i32) -> Value {
    let block = |name| json!({"type":"minecraft:matching_blocks","blocks":name});
    let bounds = json!({"type":"minecraft:inside_world_bounds","offset":[0,-2,0]});
    match kind {
        0 => block("minecraft:stone"),
        1 => block("minecraft:air"),
        2 => bounds,
        _ => {
            json!({"type":"minecraft:all_of","predicates":[{"type":"minecraft:not","predicate":block("minecraft:air")},bounds]})
        }
    }
}
fn allowed(kind: i32) -> Value {
    match kind {
        0 => json!({"type":"minecraft:true"}),
        1 => json!({"type":"minecraft:matching_blocks","blocks":"minecraft:air"}),
        2 => json!({"type":"minecraft:matching_blocks","blocks":"minecraft:stone"}),
        _ => {
            json!({"type":"minecraft:not","predicate":{"type":"minecraft:matching_blocks","blocks":"minecraft:dirt"}})
        }
    }
}
fn run(cmd: &mut Command) -> String {
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "Java scan oracle failed: {}\n{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .rev()
            .take(20)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n")
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn environment_scan_rejects_invalid_configuration() {
    for scan in [
        json!({"type":"minecraft:environment_scan","direction_of_search":"east","target_condition":{"type":"minecraft:true"},"max_steps":1}),
        json!({"type":"minecraft:environment_scan","direction_of_search":"up","target_condition":{"type":"minecraft:true"},"max_steps":33}),
        json!({"type":"minecraft:environment_scan","direction_of_search":"down","target_condition":{"type":"minecraft:inside_world_bounds","offset":[0,17,0]},"max_steps":1}),
    ] {
        assert!(
            PlacedFeature::decode(&json!({"feature":"minecraft:lake_lava","placement":[scan]}))
                .is_err()
        );
    }
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn environment_scan_matches_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let temp = tempfile::tempdir().unwrap();
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
    let source = temp.path().join("EnvironmentScanOracle.java");
    std::fs::write(&source, include_str!("java/EnvironmentScanOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("EnvironmentScanOracle"));
    let mut count = 0;
    for line in output.lines().filter(|v| v.starts_with("C ")) {
        let p: Vec<_> = line.split_whitespace().collect();
        let direction = p[2];
        let steps: i32 = p[3].parse().unwrap();
        let y: i32 = p[4].parse().unwrap();
        let target_kind: i32 = p[5].parse().unwrap();
        let allow_kind: i32 = p[6].parse().unwrap();
        let seed: i64 = p[7].parse().unwrap();
        let n: usize = p[8].parse().unwrap();
        let expected_y: i32 = p[9].parse().unwrap();
        let next: i64 = p[10].parse().unwrap();
        let scan = json!({"type":"minecraft:environment_scan","direction_of_search":direction,"max_steps":steps,"target_condition":target(target_kind),"allowed_search_condition":allowed(allow_kind)});
        let feature =
            PlacedFeature::decode(&json!({"feature":"minecraft:lake_lava","placement":[scan]}))
                .unwrap();
        let mut random = DecorationRandom::new(seed);
        let mut env = Environment;
        let mut cursor = feature.positions([0, y, 0], -64, 384).unwrap();
        let actual = cursor.next(&mut random, &mut env).unwrap();
        assert_eq!(actual.is_some() as usize, n, "{line}");
        if let Some(pos) = actual {
            assert_eq!(pos, [0, expected_y, 0], "{line}");
            assert!(cursor.next(&mut random, &mut env).unwrap().is_none());
        }
        assert_eq!(random.next_long(), next, "{line}");
        count += 1;
    }
    assert_eq!(count, 2 * 4 * 14 * 4 * 4 * 2);
    let mut resources = ResourceStack::open(&game).unwrap();
    let id = ResourceId::parse("minecraft:lake_lava_underground").unwrap();
    let placement = PlacedFeature::load(&mut resources, &id).unwrap();
    assert!(placement.has_independent_modifier());
    assert!(
        placement
            .required_tags()
            .contains(&ResourceId::parse("minecraft:air").unwrap())
    );
    let bytes = resources
        .read_data("data/minecraft/worldgen/placed_feature/lake_lava_underground.json")
        .unwrap()
        .unwrap();
    let data: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(data["feature"], "minecraft:lake_lava");
}
