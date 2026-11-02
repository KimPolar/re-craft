use anyhow::{Context, Result};
use native_core::random::DecorationRandom;
use native_resources::{
    block_column_feature::{BlockColumnFeature, ColumnWorld},
    disk_feature::{BlockPredicate, DiskWorld},
    ore_feature::OreWorld,
    packs::{ResourceId, ResourceProvider, ResourceStack},
    placed_feature::PlacedFeature,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, process::Command};

fn id(name: &str) -> ResourceId {
    ResourceId::parse(name).unwrap()
}
fn resolve(value: &Value) -> Result<String> {
    Ok(value
        .as_str()
        .or_else(|| value.get("id").and_then(Value::as_str))
        .context("missing state")?
        .into())
}
struct World {
    blocked: Option<[i32; 3]>,
    blocks: BTreeMap<[i32; 3], String>,
    writes: Vec<([i32; 3], String)>,
    restricted: bool,
}
impl OreWorld for World {
    type State = String;
    fn block(&mut self, p: [i32; 3]) -> Result<String> {
        Ok(self.blocks.get(&p).cloned().unwrap_or_else(|| {
            if self.blocked == Some(p) {
                "minecraft:stone".into()
            } else {
                "minecraft:air".into()
            }
        }))
    }
    fn block_is(&mut self, s: &String, id: &ResourceId) -> Result<bool> {
        Ok(*s == format!("{}:{}", id.namespace, id.path))
    }
    fn in_tag(&mut self, s: &String, tag: &ResourceId) -> Result<bool> {
        Ok(match tag.path.as_str() {
            "supports_sugar_cane" => s == "minecraft:stone",
            "supports_sugar_cane_adjacently" => s == "minecraft:frosted_ice",
            "supports_cactus" => s == "minecraft:sand",
            "supports_vegetation" => s == "minecraft:stone",
            "supports_dry_vegetation" => s == "minecraft:sand",
            _ => false,
        })
    }
    fn is_air(&mut self, s: &String) -> Result<bool> {
        Ok(s == "minecraft:air")
    }
    fn set_block(&mut self, _: [i32; 3], _: &String, _: i32) -> Result<bool> {
        unreachable!()
    }
}
impl DiskWorld for World {
    fn is_solid(&mut self, s: &String) -> Result<bool> {
        Ok(matches!(
            s.as_str(),
            "minecraft:stone" | "minecraft:dirt" | "minecraft:sand" | "minecraft:cactus"
        ))
    }
    fn is_liquid(&mut self, s: &String) -> Result<bool> {
        Ok(matches!(s.as_str(), "minecraft:water" | "minecraft:lava"))
    }
    fn fluid_is(&mut self, _: &String, _: &ResourceId) -> Result<bool> {
        unreachable!()
    }
    fn fluid_in_tag(&mut self, s: &String, tag: &ResourceId) -> Result<bool> {
        Ok(match tag.path.as_str() {
            "supports_sugar_cane_adjacently" => s == "minecraft:water" || s == "test:flowing_water",
            "lava" => s == "minecraft:lava",
            _ => false,
        })
    }
    fn set_disk_block(&mut self, _: [i32; 3], _: &String) -> Result<bool> {
        unreachable!()
    }
    fn mark_postprocessing(&mut self, _: [i32; 3]) -> Result<()> {
        unreachable!()
    }
}
impl ColumnWorld for World {
    fn set_column_block(&mut self, p: [i32; 3], s: &String) -> Result<bool> {
        if !self.restricted {
            self.blocks.insert(p, s.clone());
            self.writes.push((p, s.clone()));
        }
        Ok(!self.restricted)
    }
}
fn world() -> World {
    World {
        blocked: None,
        blocks: BTreeMap::new(),
        writes: Vec::new(),
        restricted: false,
    }
}
fn feature(kind: i32, direction: &str, tip: bool) -> Value {
    let sugar = json!({"id":"minecraft:sugar_cane"});
    let stone = json!({"id":"minecraft:stone"});
    let layers = match kind {
        0 => json!([{"height":0,"provider":sugar}]),
        1 => {
            json!([{"height":{"type":"minecraft:biased_to_bottom","min_inclusive":2,"max_inclusive":4},"provider":sugar}])
        }
        2 => {
            json!([{"height":{"type":"minecraft:uniform","min_inclusive":1,"max_inclusive":3},"provider":sugar},{"height":{"type":"minecraft:uniform","min_inclusive":0,"max_inclusive":2},"provider":stone},{"height":1,"provider":{"type":"minecraft:weighted","entries":[{"data":{"id":"minecraft:sugar_cane"},"weight":3},{"data":{"id":"minecraft:stone"},"weight":1}]}}])
        }
        _ => {
            json!([{"height":{"type":"minecraft:biased_to_bottom","min_inclusive":1,"max_inclusive":3},"provider":{"id":"minecraft:cactus"}},{"height":{"type":"minecraft:weighted_list","distribution":[{"data":0,"weight":3},{"data":1,"weight":1}]},"provider":{"id":"minecraft:cactus_flower"}}])
        }
    };
    json!({"type":"minecraft:block_column","layers":layers,"direction":direction,"allowed_placement":{"type":"minecraft:matching_blocks","blocks":"minecraft:air"},"prioritize_tip":tip})
}

#[test]
fn named_column_providers_follow_selected_resource_stack() {
    struct Packs(bool);
    impl ResourceProvider for Packs {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok((path == "data/example/worldgen/block_state_provider/column.json")
                .then(|| serde_json::to_vec(&json!({"type":"minecraft:weighted","entries":[
                    {"data":{"id":if self.0 {"minecraft:stone"} else {"minecraft:sugar_cane"}},"weight":2},
                    {"data":{"id":"minecraft:stone"},"weight":1}
                ]})).unwrap()))
        }
    }
    let mut named = feature(1, "up", false);
    named["layers"][0]["provider"] = json!("example:column");
    for override_pack in [false, true] {
        let mut resources = Packs(override_pack);
        let column =
            BlockColumnFeature::decode_with_resources(&mut resources, &named, &mut resolve)
                .unwrap();
        let bytes = resources
            .read_data("data/example/worldgen/block_state_provider/column.json")
            .unwrap()
            .unwrap();
        let mut inline = named.clone();
        inline["layers"][0]["provider"] = serde_json::from_slice(&bytes).unwrap();
        let expected = BlockColumnFeature::decode(&inline, &mut resolve).unwrap();
        for seed in [0, 1, -1, 42] {
            let (mut a, mut b) = (world(), world());
            let (mut ra, mut rb) = (DecorationRandom::new(seed), DecorationRandom::new(seed));
            assert_eq!(
                column.place(&mut a, &mut ra, [0, 0, 0]).unwrap(),
                expected.place(&mut b, &mut rb, [0, 0, 0]).unwrap()
            );
            assert_eq!(a.writes, b.writes);
            assert_eq!(ra.next_long(), rb.next_long());
        }
    }
    named["layers"][0]["provider"] = json!("example:missing");
    assert!(
        BlockColumnFeature::decode_with_resources(&mut Packs(false), &named, &mut resolve).is_err()
    );
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3"]
fn installed_cave_vines_column_resolves_named_and_randomized_providers() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let mut resources = ResourceStack::open(&game).unwrap();
    let registry = resources.vanilla_generated_block_registry().unwrap();
    let data = resources
        .read_data("data/minecraft/worldgen/feature/cave_vine_in_moss.json")
        .unwrap()
        .unwrap();
    let value: Value = serde_json::from_slice(&data).unwrap();
    let feature = BlockColumnFeature::decode_with_resources(&mut resources, &value, &mut |v| {
        registry.resolve(v)
    })
    .unwrap();
    let states = feature.possible_states();
    assert!(states.iter().any(|s| s.0 == "minecraft:cave_vines_plant"));
    assert!(states.iter().any(|s| s.0 == "minecraft:cave_vines"));
}
fn run(cmd: &mut Command) -> String {
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "Java column oracle failed: {}\n{}",
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
#[derive(Default)]
struct Case {
    direction: String,
    tip: bool,
    obstacle: i32,
    restricted: bool,
    kind: i32,
    seed: i64,
    result: bool,
    next: i64,
    writes: Vec<([i32; 3], String)>,
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn block_columns_and_sugar_cane_survival_match_java() {
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
    let source = temp.path().join("BlockColumnOracle.java");
    std::fs::write(&source, include_str!("java/BlockColumnOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("BlockColumnOracle"));
    assert!(output.lines().any(|v| v == "E false false null"));
    let pred = BlockPredicate::decode(
        &json!({"type":"minecraft:would_survive","state":"minecraft:sugar_cane"}),
    )
    .unwrap();
    assert!(pred.needs_sugar_cane_survival());
    assert_eq!(pred.required_tags().len(), 2);
    assert!(pred.required_blocks().contains(&id("minecraft:sugar_cane")));
    let cactus = BlockPredicate::decode(
        &json!({"type":"minecraft:would_survive","state":"minecraft:cactus"}),
    )
    .unwrap();
    assert!(cactus.needs_survival("cactus"));
    assert!(
        cactus
            .required_tags()
            .contains(&id("minecraft:supports_cactus"))
    );
    let mut cases = Vec::<Case>::new();
    let mut survival = 0;
    let mut cactus_cases = 0;
    let mut solidity_cases = 0;
    let mut sapling_cases = 0;
    let mut vegetation_cases = 0;
    for line in output.lines() {
        let p: Vec<_> = line.split_whitespace().collect();
        match p.as_slice() {
            ["V", name, base, result] => {
                let predicate =
                    BlockPredicate::decode(&json!({"type":"minecraft:would_survive","state":name}))
                        .unwrap();
                let tag = if matches!(
                    *name,
                    "minecraft:short_dry_grass"
                        | "minecraft:tall_dry_grass"
                        | "minecraft:dead_bush"
                ) {
                    "minecraft:supports_dry_vegetation"
                } else {
                    "minecraft:supports_vegetation"
                };
                assert!(predicate.required_tags().contains(&id(tag)));
                let mut w = world();
                let base: usize = base.parse().unwrap();
                w.blocks.insert(
                    [0, -1, 0],
                    [
                        "minecraft:stone",
                        "minecraft:dirt",
                        "minecraft:air",
                        "minecraft:water",
                        "minecraft:sand",
                    ][base]
                        .into(),
                );
                assert_eq!(
                    predicate.test(&mut w, [0, 0, 0]).unwrap(),
                    result.parse::<bool>().unwrap(),
                    "{name} {base}"
                );
                vegetation_cases += 1;
            }
            ["P", name, base, result] => {
                let predicate =
                    BlockPredicate::decode(&json!({"type":"minecraft:would_survive","state":name}))
                        .unwrap();
                assert!(
                    predicate
                        .required_tags()
                        .contains(&id("minecraft:supports_vegetation"))
                );
                let mut w = world();
                let base: usize = base.parse().unwrap();
                w.blocks.insert(
                    [0, -1, 0],
                    [
                        "minecraft:stone",
                        "minecraft:dirt",
                        "minecraft:air",
                        "minecraft:water",
                    ][base]
                        .into(),
                );
                assert_eq!(
                    predicate.test(&mut w, [0, 0, 0]).unwrap(),
                    result.parse::<bool>().unwrap(),
                    "sapling {name} {base}"
                );
                sapling_cases += 1;
            }
            ["J", name, solid, liquid, entity, poi, post] => {
                let mut w = world();
                let s = (*name).to_owned();
                assert_eq!(
                    w.is_solid(&s).unwrap(),
                    solid.parse::<bool>().unwrap(),
                    "solid {name}"
                );
                assert_eq!(
                    w.is_liquid(&s).unwrap(),
                    liquid.parse::<bool>().unwrap(),
                    "liquid {name}"
                );
                assert_eq!((*entity, *poi, *post), ("false", "false", "null"));
                solidity_cases += 1;
            }
            ["K", base, neighbor, side, above, result] => {
                let base: usize = base.parse().unwrap();
                let neighbor: usize = neighbor.parse().unwrap();
                let side: usize = side.parse().unwrap();
                let above: usize = above.parse().unwrap();
                let mut w = world();
                w.blocks.insert(
                    [0, -1, 0],
                    ["minecraft:sand", "minecraft:cactus", "minecraft:dirt"][base].into(),
                );
                w.blocks.insert(
                    [0, 1, 0],
                    [
                        "minecraft:air",
                        "minecraft:water",
                        "minecraft:lava",
                        "minecraft:stone",
                        "minecraft:cactus_flower",
                    ][above]
                        .into(),
                );
                if side < 4 {
                    let [dx, dz] = [[0, -1], [1, 0], [0, 1], [-1, 0]][side];
                    w.blocks.insert(
                        [dx, 0, dz],
                        [
                            "minecraft:air",
                            "minecraft:stone",
                            "minecraft:cactus",
                            "minecraft:water",
                            "minecraft:lava",
                            "minecraft:sugar_cane",
                            "minecraft:cactus_flower",
                        ][neighbor]
                            .into(),
                    );
                }
                assert_eq!(
                    cactus.test(&mut w, [0, 0, 0]).unwrap(),
                    result.parse::<bool>().unwrap(),
                    "cactus {base} {neighbor} {side} {above}"
                );
                cactus_cases += 1;
            }
            ["S", base, neighbor, side, result] => {
                let base: usize = base.parse().unwrap();
                let neighbor: usize = neighbor.parse().unwrap();
                let side: usize = side.parse().unwrap();
                let mut w = world();
                w.blocks.insert(
                    [0, -1, 0],
                    [
                        "minecraft:stone",
                        "minecraft:dirt",
                        "minecraft:sugar_cane",
                        "minecraft:air",
                    ][base]
                        .into(),
                );
                if side < 4 {
                    let [dx, dz] = [[0, -1], [1, 0], [0, 1], [-1, 0]][side];
                    w.blocks.insert(
                        [dx, -1, dz],
                        [
                            "minecraft:air",
                            "minecraft:water",
                            "test:flowing_water",
                            "minecraft:lava",
                            "minecraft:frosted_ice",
                        ][neighbor]
                            .into(),
                    );
                }
                assert_eq!(
                    pred.test(&mut w, [0, 0, 0]).unwrap(),
                    result.parse::<bool>().unwrap(),
                    "survival {base} {neighbor} {side}"
                );
                survival += 1;
            }
            [
                "C",
                _,
                direction,
                tip,
                obstacle,
                restricted,
                kind,
                seed,
                result,
                next,
            ] => cases.push(Case {
                direction: (*direction).into(),
                tip: tip.parse().unwrap(),
                obstacle: obstacle.parse().unwrap(),
                restricted: *restricted == "1",
                kind: kind.parse().unwrap(),
                seed: seed.parse().unwrap(),
                result: result.parse().unwrap(),
                next: next.parse().unwrap(),
                writes: Vec::new(),
            }),
            ["B", x, y, z, name] => cases.last_mut().unwrap().writes.push((
                [x.parse().unwrap(), y.parse().unwrap(), z.parse().unwrap()],
                (*name).into(),
            )),
            _ => {}
        }
    }
    assert_eq!(survival, 100);
    assert_eq!(sapling_cases, 8 * 4);
    assert_eq!(vegetation_cases, 21 * 5);
    assert_eq!(cactus_cases, 3 * 7 * 5 * 5);
    assert_eq!(solidity_cases, 8);
    assert_eq!(cases.len(), 6 * 2 * 6 * 2 * 4 * 3);
    for (index, c) in cases.iter().enumerate() {
        let d = match c.direction.as_str() {
            "up" => [0, 1, 0],
            "down" => [0, -1, 0],
            "north" => [0, 0, -1],
            "south" => [0, 0, 1],
            "west" => [-1, 0, 0],
            "east" => [1, 0, 0],
            _ => panic!(),
        };
        let mut w = world();
        w.restricted = c.restricted;
        w.blocked = Some(d.map(|v| v * c.obstacle));
        let mut random = DecorationRandom::new(c.seed);
        let column =
            BlockColumnFeature::decode(&feature(c.kind, &c.direction, c.tip), &mut resolve)
                .unwrap();
        assert_eq!(
            column.place(&mut w, &mut random, [0, 0, 0]).unwrap(),
            c.result,
            "result {index}"
        );
        assert_eq!(w.writes, c.writes, "writes {index}");
        assert_eq!(random.next_long(), c.next, "random {index}");
    }
    let mut resources = ResourceStack::open(&game).unwrap();
    let placed = PlacedFeature::load(&mut resources, &id("minecraft:patch_sugar_cane")).unwrap();
    assert!(placed.needs_sugar_cane_survival());
    let bytes = resources
        .read_data("data/minecraft/worldgen/feature/sugar_cane.json")
        .unwrap()
        .unwrap();
    let definition: Value = serde_json::from_slice(&bytes).unwrap();
    BlockColumnFeature::decode(&definition, &mut resolve).unwrap();
    for name in ["patch_cactus", "patch_cactus_desert"] {
        assert!(
            PlacedFeature::load(&mut resources, &id(&format!("minecraft:{name}")))
                .unwrap()
                .needs_survival("cactus")
        );
    }
    let bytes = resources
        .read_data("data/minecraft/worldgen/feature/cactus.json")
        .unwrap()
        .unwrap();
    BlockColumnFeature::decode(&serde_json::from_slice(&bytes).unwrap(), &mut resolve).unwrap();
}
