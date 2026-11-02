use anyhow::{Context, Result};
use native_core::random::DecorationRandom;
use native_resources::{
    ore_feature::OreWorld,
    packs::{ResourceId, ResourceProvider, ResourceStack},
    placed_feature::PlacedFeature,
    underwater_magma::{MagmaWorld, UnderwaterMagma, vanilla_full_occlusion},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, process::Command};

fn initial(profile: i32, p: [i32; 3]) -> String {
    let [x, y, z] = p;
    if profile == 0 {
        return "minecraft:stone".into();
    }
    let floor = (x * 3 + z * 7 + profile).rem_euclid(3) - 1;
    if y > 4 {
        return "minecraft:air".into();
    }
    if y > floor {
        return "minecraft:water".into();
    }
    if profile >= 3 && y < floor && (x * 7 + y * 11 + z * 3 + profile).rem_euclid(9) == 0 {
        return "minecraft:air".into();
    }
    if profile >= 2 && (x + y + z).rem_euclid(3) == 0 {
        "minecraft:dirt"
    } else {
        "minecraft:stone"
    }
    .into()
}
struct World {
    profile: i32,
    restricted: bool,
    changes: BTreeMap<[i32; 3], String>,
    writes: Vec<[i32; 3]>,
    marks: Vec<[i32; 3]>,
}
impl OreWorld for World {
    type State = String;
    fn block(&mut self, p: [i32; 3]) -> Result<String> {
        Ok(self
            .changes
            .get(&p)
            .cloned()
            .unwrap_or_else(|| initial(self.profile, p)))
    }
    fn block_is(&mut self, s: &String, id: &ResourceId) -> Result<bool> {
        Ok(*s == format!("{}:{}", id.namespace, id.path))
    }
    fn in_tag(&mut self, _: &String, _: &ResourceId) -> Result<bool> {
        unreachable!()
    }
    fn is_air(&mut self, s: &String) -> Result<bool> {
        Ok(s == "minecraft:air")
    }
    fn set_block(&mut self, _: [i32; 3], _: &String, _: i32) -> Result<bool> {
        unreachable!()
    }
}
impl MagmaWorld for World {
    fn face_is_full(&mut self, s: &String, _: [i32; 3]) -> Result<bool> {
        vanilla_full_occlusion(s).context("unsupported test block")
    }
    fn set_magma(&mut self, p: [i32; 3], s: &String) -> Result<bool> {
        if !self.restricted {
            self.changes.insert(p, s.clone());
            self.writes.push(p);
            self.marks.push([p[0], p[1].wrapping_add(1), p[2]]);
        }
        Ok(!self.restricted)
    }
}
fn resolve(value: &Value) -> Result<String> {
    Ok(value["id"].as_str().context("missing block id")?.to_owned())
}
fn run(cmd: &mut Command) -> String {
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "Java magma oracle failed: {}\n{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .rev()
            .take(25)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n")
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn magma_rejects_invalid_configurations() {
    for value in [
        json!({"type":"minecraft:underwater_magma","floor_search_range":513,"placement_radius_around_floor":1,"placement_probability_per_valid_position":0.5}),
        json!({"type":"minecraft:underwater_magma","floor_search_range":1,"placement_radius_around_floor":65,"placement_probability_per_valid_position":0.5}),
        json!({"type":"minecraft:underwater_magma","floor_search_range":1,"placement_radius_around_floor":1,"placement_probability_per_valid_position":-0.5}),
    ] {
        assert!(UnderwaterMagma::decode(&value, &mut resolve).is_err());
    }
}

#[derive(Default)]
struct Case {
    profile: i32,
    range: i32,
    radius: i32,
    probability: f32,
    restricted: bool,
    seed: i64,
    y: i32,
    result: bool,
    next: i64,
    writes: Vec<[i32; 3]>,
    marks: Vec<[i32; 3]>,
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn magma_generation_matches_java() {
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
    let source = temp.path().join("UnderwaterMagmaOracle.java");
    std::fs::write(&source, include_str!("java/UnderwaterMagmaOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("UnderwaterMagmaOracle"));
    assert!(
        output
            .lines()
            .any(|line| line == "E false false BlockPos{x=0, y=1, z=0}")
    );
    let mut cases: Vec<Case> = Vec::new();
    let mut occlusions = 0;
    for line in output.lines() {
        let p: Vec<_> = line.split_whitespace().collect();
        match p.as_slice() {
            ["O", id, full] => {
                assert_eq!(
                    vanilla_full_occlusion(id),
                    Some(full.parse().unwrap()),
                    "occlusion {id}"
                );
                occlusions += 1;
            }
            [
                "C",
                _,
                profile,
                range,
                radius,
                probability,
                restricted,
                seed,
                y,
                result,
                next,
            ] => cases.push(Case {
                profile: profile.parse().unwrap(),
                range: range.parse().unwrap(),
                radius: radius.parse().unwrap(),
                probability: probability.parse().unwrap(),
                restricted: *restricted == "1",
                seed: seed.parse().unwrap(),
                y: y.parse().unwrap(),
                result: result.parse().unwrap(),
                next: next.parse().unwrap(),
                writes: Vec::new(),
                marks: Vec::new(),
            }),
            ["B", x, y, z] => cases.last_mut().unwrap().writes.push([
                x.parse().unwrap(),
                y.parse().unwrap(),
                z.parse().unwrap(),
            ]),
            ["M", x, y, z] => cases.last_mut().unwrap().marks.push([
                x.parse().unwrap(),
                y.parse().unwrap(),
                z.parse().unwrap(),
            ]),
            _ => {}
        }
    }
    assert!(occlusions >= (31 + 18) * 6);
    assert_eq!(cases.len(), 5 * 4 * 3 * 3 * 2 * 2 * 3);
    for (index, c) in cases.iter().enumerate() {
        let feature=UnderwaterMagma::decode(&json!({"type":"minecraft:underwater_magma","floor_search_range":c.range,"placement_radius_around_floor":c.radius,"placement_probability_per_valid_position":c.probability}),&mut resolve).unwrap();
        let mut world = World {
            profile: c.profile,
            restricted: c.restricted,
            changes: BTreeMap::new(),
            writes: Vec::new(),
            marks: Vec::new(),
        };
        let mut random = DecorationRandom::new(c.seed);
        assert_eq!(
            feature.place(&mut world, &mut random, [0, c.y, 0]).unwrap(),
            c.result,
            "result {index}"
        );
        assert_eq!(world.writes, c.writes, "writes {index}");
        assert_eq!(world.marks, c.marks, "postprocessing {index}");
        assert_eq!(random.next_long(), c.next, "random {index}");
    }
    let mut resources = ResourceStack::open(&game).unwrap();
    let id = ResourceId::parse("minecraft:underwater_magma").unwrap();
    PlacedFeature::load(&mut resources, &id).unwrap();
    let data = resources
        .read_data("data/minecraft/worldgen/feature/underwater_magma.json")
        .unwrap()
        .unwrap();
    UnderwaterMagma::decode(&serde_json::from_slice(&data).unwrap(), &mut resolve).unwrap();
}
