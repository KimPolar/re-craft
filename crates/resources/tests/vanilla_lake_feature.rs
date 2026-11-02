use anyhow::{Context, Result};
use native_core::random::DecorationRandom;
use native_resources::{
    disk_feature::DiskWorld,
    lake_feature::{LakeFeature, LakeWorld},
    ore_feature::OreWorld,
    packs::{ResourceId, ResourceProvider, ResourceStack},
    placed_feature::PlacedFeature,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    process::Command,
};

type Chunk = (i32, i32);
fn chunk(p: [i32; 3]) -> Chunk {
    (p[0].div_euclid(16), p[2].div_euclid(16))
}
fn initial(profile: i32, p: [i32; 3]) -> String {
    let [x, y, z] = p;
    if profile == 1 && y >= 12 {
        return "minecraft:air".into();
    }
    if profile == 2 && y >= 12 {
        return "minecraft:water".into();
    }
    if profile == 3 && (x * 7 + y * 3 + z * 11).rem_euclid(13) == 0 {
        return "minecraft:bedrock".into();
    }
    "minecraft:stone".into()
}
struct World {
    profile: i32,
    restricted: bool,
    blocks: BTreeMap<[i32; 3], String>,
    writes: Vec<([i32; 3], String)>,
    marks: BTreeMap<(i32, i32, i32), Vec<i16>>,
    ticks: BTreeMap<Chunk, Vec<[i32; 3]>>,
    keys: BTreeSet<[i32; 3]>,
}
impl OreWorld for World {
    type State = String;
    fn block(&mut self, p: [i32; 3]) -> Result<String> {
        Ok(self
            .blocks
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
        Ok(matches!(s.as_str(), "minecraft:air" | "minecraft:cave_air"))
    }
    fn set_block(&mut self, _: [i32; 3], _: &String, _: i32) -> Result<bool> {
        unreachable!()
    }
}
impl DiskWorld for World {
    fn is_solid(&mut self, s: &String) -> Result<bool> {
        Ok(matches!(
            s.as_str(),
            "minecraft:stone" | "minecraft:bedrock"
        ))
    }
    fn is_liquid(&mut self, s: &String) -> Result<bool> {
        Ok(matches!(s.as_str(), "minecraft:water" | "minecraft:lava"))
    }
    fn fluid_is(&mut self, _: &String, _: &ResourceId) -> Result<bool> {
        unreachable!()
    }
    fn fluid_in_tag(&mut self, s: &String, id: &ResourceId) -> Result<bool> {
        assert_eq!(id.path, "water");
        Ok(s == "minecraft:water")
    }
    fn set_disk_block(&mut self, _: [i32; 3], _: &String) -> Result<bool> {
        unreachable!()
    }
    fn mark_postprocessing(&mut self, p: [i32; 3]) -> Result<()> {
        if (0..32).contains(&p[1]) {
            let (x, z) = chunk(p);
            let packed = ((p[0] & 15) | ((p[1] & 15) << 4) | ((p[2] & 15) << 8)) as i16;
            self.marks
                .entry((x, z, p[1] / 16))
                .or_default()
                .push(packed);
        }
        Ok(())
    }
}
impl LakeWorld for World {
    fn min_y(&self) -> i32 {
        0
    }
    fn set_lake_block(&mut self, p: [i32; 3], s: &String) -> Result<bool> {
        if !self.restricted {
            self.blocks.insert(p, s.clone());
            self.writes.push((p, s.clone()));
        }
        Ok(!self.restricted)
    }
    fn schedule_lake_air_tick(&mut self, p: [i32; 3]) -> Result<()> {
        if self.keys.insert(p) {
            self.ticks.entry(chunk(p)).or_default().push(p);
        }
        Ok(())
    }
    fn should_freeze_lake(&mut self, _: [i32; 3]) -> Result<bool> {
        unreachable!()
    }
}
fn resolve(v: &Value) -> Result<String> {
    Ok(v.as_str()
        .or_else(|| v.get("id").and_then(Value::as_str))
        .context("missing state")?
        .into())
}
fn lake(mode: i32) -> Value {
    let p = json!({"type":"minecraft:not","predicate":{"type":"minecraft:matching_blocks","blocks":"minecraft:bedrock"}});
    json!({"type":"minecraft:lake","fluid":{"id":"minecraft:lava"},"barrier":{"id":if mode==2{"minecraft:air"}else{"minecraft:stone"}},"can_place_feature":if mode==1{p.clone()}else{json!({"type":"minecraft:true"})},"can_replace_with_air_or_fluid":p,"can_replace_with_barrier":p})
}
fn run(cmd: &mut Command) -> String {
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "Java lake oracle failed: {}\n{}",
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
#[derive(Default)]
struct Case {
    profile: i32,
    restricted: bool,
    mode: i32,
    seed: i64,
    result: bool,
    next: i64,
    writes: Vec<([i32; 3], String)>,
    marks: BTreeMap<(i32, i32, i32), Vec<i16>>,
    ticks: BTreeMap<Chunk, Vec<[i32; 3]>>,
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn lake_generation_matches_java() {
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
    let source = temp.path().join("LakeFeatureOracle.java");
    std::fs::write(&source, include_str!("java/LakeFeatureOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("LakeFeatureOracle"));
    let mut cases = Vec::<Case>::new();
    let mut effects = 0;
    for line in output.lines() {
        let p: Vec<_> = line.split_whitespace().collect();
        match p.as_slice() {
            ["E", _, entity, poi, post] => {
                assert_eq!((*entity, *poi, *post), ("false", "false", "null"));
                effects += 1;
            }
            ["C", _, profile, restricted, mode, seed, result, next] => cases.push(Case {
                profile: profile.parse().unwrap(),
                restricted: *restricted == "1",
                mode: mode.parse().unwrap(),
                seed: seed.parse().unwrap(),
                result: result.parse().unwrap(),
                next: next.parse().unwrap(),
                ..Case::default()
            }),
            ["B", x, y, z, name] => cases.last_mut().unwrap().writes.push((
                [x.parse().unwrap(), y.parse().unwrap(), z.parse().unwrap()],
                (*name).into(),
            )),
            ["M", x, z, section, packed] => cases
                .last_mut()
                .unwrap()
                .marks
                .entry((
                    x.parse().unwrap(),
                    z.parse().unwrap(),
                    section.parse().unwrap(),
                ))
                .or_default()
                .push(packed.parse().unwrap()),
            ["T", name, x, y, z, delay, priority] => {
                assert_eq!((*name, *delay, *priority), ("minecraft:cave_air", "0", "0"));
                let pos = [x.parse().unwrap(), y.parse().unwrap(), z.parse().unwrap()];
                cases
                    .last_mut()
                    .unwrap()
                    .ticks
                    .entry(chunk(pos))
                    .or_default()
                    .push(pos);
            }
            _ => {}
        }
    }
    assert_eq!(effects, 4);
    assert_eq!(cases.len(), 4 * 2 * 3 * 4);
    for (index, c) in cases.iter().enumerate() {
        let feature = LakeFeature::decode(&lake(c.mode), &mut resolve).unwrap();
        let mut world = World {
            profile: c.profile,
            restricted: c.restricted,
            blocks: BTreeMap::new(),
            writes: Vec::new(),
            marks: BTreeMap::new(),
            ticks: BTreeMap::new(),
            keys: BTreeSet::new(),
        };
        let mut random = DecorationRandom::new(c.seed);
        assert_eq!(
            feature.place(&mut world, &mut random, [0, 12, 0]).unwrap(),
            c.result,
            "result {index}"
        );
        assert_eq!(world.writes, c.writes, "writes {index}");
        assert_eq!(world.marks, c.marks, "marks {index}");
        assert_eq!(world.ticks, c.ticks, "ticks {index}");
        assert_eq!(random.next_long(), c.next, "random {index}");
    }
    let mut resources = ResourceStack::open(&game).unwrap();
    let bytes = resources
        .read_data("data/minecraft/worldgen/feature/lake_lava.json")
        .unwrap()
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    LakeFeature::decode(&value, &mut resolve).unwrap();
    for name in ["lake_lava_underground", "lake_lava_surface"] {
        PlacedFeature::load(
            &mut resources,
            &ResourceId::parse(&format!("minecraft:{name}")).unwrap(),
        )
        .unwrap();
    }
}
