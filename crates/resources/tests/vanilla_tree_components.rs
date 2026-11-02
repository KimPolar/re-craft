use anyhow::Result;
use native_core::random::DecorationRandom;
use native_resources::{
    disk_feature::DiskWorld,
    ore_feature::OreWorld,
    packs::{ResourceId, ResourceProvider, ResourceStack},
    tree_feature::{TreeFeature, TreeWorld, TreeWrite},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, process::Command};

#[derive(Clone, Eq, PartialEq)]
struct State {
    name: String,
    persistent: bool,
    waterlogged: bool,
}
fn state(name: &str) -> State {
    State {
        name: name.into(),
        persistent: false,
        waterlogged: false,
    }
}
struct World {
    blocks: BTreeMap<[i32; 3], State>,
    writes: Vec<String>,
}
impl OreWorld for World {
    type State = State;
    fn block(&mut self, p: [i32; 3]) -> Result<State> {
        Ok(self
            .blocks
            .get(&p)
            .cloned()
            .unwrap_or_else(|| state("minecraft:air")))
    }
    fn block_is(&mut self, s: &State, id: &ResourceId) -> Result<bool> {
        Ok(s.name == format!("{}:{}", id.namespace, id.path))
    }
    fn in_tag(&mut self, s: &State, id: &ResourceId) -> Result<bool> {
        Ok(match id.path.as_str() {
            "logs" => s.name == "minecraft:oak_log",
            "replaceable_by_trees" => matches!(
                s.name.as_str(),
                "minecraft:oak_leaves" | "minecraft:vine" | "minecraft:water"
            ),
            _ => false,
        })
    }
    fn is_air(&mut self, s: &State) -> Result<bool> {
        Ok(s.name == "minecraft:air")
    }
    fn set_block(&mut self, _: [i32; 3], _: &State, _: i32) -> Result<bool> {
        unreachable!()
    }
}
impl DiskWorld for World {
    fn is_solid(&mut self, _: &State) -> Result<bool> {
        unreachable!()
    }
    fn fluid_is(&mut self, _: &State, _: &ResourceId) -> Result<bool> {
        unreachable!()
    }
    fn set_disk_block(&mut self, _: [i32; 3], _: &State) -> Result<bool> {
        unreachable!()
    }
    fn mark_postprocessing(&mut self, _: [i32; 3]) -> Result<()> {
        unreachable!()
    }
}
impl TreeWorld for World {
    fn trunk_axis(
        &mut self,
        state: State,
        _: native_resources::tree_fancy_trunk::LogAxis,
    ) -> Result<State> {
        Ok(state)
    }
    fn tree_min_y(&self) -> i32 {
        0
    }
    fn tree_max_y(&self) -> i32 {
        40
    }
    fn persistent_foliage(&mut self, p: [i32; 3]) -> Result<bool> {
        Ok(self.block(p)?.persistent)
    }
    fn foliage_state(&mut self, p: [i32; 3], mut s: State) -> Result<State> {
        s.waterlogged = self.block(p)?.name == "minecraft:water";
        Ok(s)
    }
    fn write_tree(&mut self, [x, y, z]: [i32; 3], s: &State, kind: TreeWrite) -> Result<()> {
        self.writes.push(match kind {
            TreeWrite::Trunk => format!("T{x},{y},{z},{}", s.name),
            TreeWrite::Foliage => format!("L{x},{y},{z},{},{}", s.name, s.waterlogged),
        });
        self.blocks.insert([x, y, z], s.clone());
        Ok(())
    }
}
struct Data;
impl ResourceProvider for Data {
    fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        Ok(
            (path == "data/test/worldgen/block_state_provider/soil.json")
                .then(|| br#"{"id":"minecraft:dirt"}"#.to_vec()),
        )
    }
}
fn resolve(v: &Value) -> Result<State> {
    Ok(state(v.get("id").and_then(Value::as_str).unwrap()))
}
fn definition(ignore: bool) -> Value {
    json!({"type":"minecraft:tree","trunk_provider":{"id":"minecraft:oak_log"},"trunk_placer":{"type":"minecraft:straight_trunk_placer","base_height":4,"height_rand_a":2,"height_rand_b":0},"foliage_provider":{"id":"minecraft:oak_leaves"},"foliage_placer":{"type":"minecraft:blob_foliage_placer","height":3,"radius":2,"offset":0},"below_trunk_provider":"test:soil","minimum_size":{"type":"minecraft:two_layers_feature_size"},"decorators":[],"ignore_vines":ignore})
}

#[test]
fn unsupported_tree_effects_are_rejected_before_placement() {
    let mut v = definition(true);
    v["decorators"] = json!([{"type":"minecraft:beehive","probability":0.05}]);
    let tree = TreeFeature::decode(&mut Data, &v, &mut resolve).unwrap();
    assert!(
        tree.required_blocks()
            .contains(&ResourceId::parse("minecraft:bee_nest").unwrap())
    );
    v["decorators"] = json!([{"type":"minecraft:beehive","probability":2.0}]);
    assert!(TreeFeature::decode(&mut Data, &v, &mut resolve).is_err());
    v["decorators"] = json!([{"type":"minecraft:trunk_vine"}]);
    assert!(TreeFeature::decode(&mut Data, &v, &mut resolve).is_err());
    v["decorators"] = json!([]);
    v["root_placer"] = json!({"type":"minecraft:mangrove_root_placer"});
    assert!(TreeFeature::decode(&mut Data, &v, &mut resolve).is_err());
}

#[test]
fn named_soil_provider_is_used_before_trunk_and_foliage() {
    let tree = TreeFeature::decode(&mut Data, &definition(true), &mut resolve).unwrap();
    let mut world = World {
        blocks: BTreeMap::from([([3, 2, -4], state("minecraft:stone"))]),
        writes: Vec::new(),
    };
    assert!(
        tree.place_components(&mut world, &mut DecorationRandom::new(1), [3, 3, -4])
            .unwrap()
    );
    assert_eq!(world.writes.first().unwrap(), "T3,2,-4,minecraft:dirt");
    assert!(world.writes.iter().any(|v| v.contains("minecraft:oak_log")));
    assert!(
        world
            .writes
            .iter()
            .any(|v| v.contains("minecraft:oak_leaves"))
    );
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3"]
fn installed_oak_bees_definition_is_supported() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let mut resources = ResourceStack::open(&game).unwrap();
    let definition: Value = serde_json::from_slice(
        &resources
            .read_data("data/minecraft/worldgen/feature/oak_bees_005.json")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let tree = TreeFeature::decode(&mut resources, &definition, &mut resolve).unwrap();
    assert!(
        tree.required_blocks()
            .contains(&ResourceId::parse("minecraft:bee_nest").unwrap())
    );
    let fancy: Value = serde_json::from_slice(
        &resources
            .read_data("data/minecraft/worldgen/feature/fancy_oak_bees_005.json")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let tree = TreeFeature::decode(&mut resources, &fancy, &mut resolve).unwrap();
    assert!(
        tree.required_blocks()
            .contains(&ResourceId::parse("minecraft:bee_nest").unwrap())
    );
}

fn run(c: &mut Command) -> String {
    let o = c.output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout).unwrap()
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn straight_blob_tree_components_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("TreeComponentsOracle.java");
    std::fs::write(&source, include_str!("java/TreeComponentsOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let actual = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("TreeComponentsOracle"));
    let mut resources = ResourceStack::open(&game).unwrap();
    let oak: Value = serde_json::from_slice(
        &resources
            .read_data("data/minecraft/worldgen/feature/oak.json")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let tree = TreeFeature::decode(&mut resources, &oak, &mut resolve).unwrap();
    assert!(
        tree.required_tags()
            .contains(&ResourceId::parse("minecraft:cannot_replace_below_tree_trunk").unwrap())
    );
    let reference = TreeFeature::decode(&mut Data, &definition(true), &mut resolve).unwrap();
    let world = || World {
        blocks: BTreeMap::from([([3, 2, -4], state("minecraft:stone"))]),
        writes: Vec::new(),
    };
    let (mut installed_world, mut reference_world) = (world(), world());
    let (mut installed_random, mut reference_random) =
        (DecorationRandom::new(1), DecorationRandom::new(1));
    assert!(
        tree.place_components(&mut installed_world, &mut installed_random, [3, 3, -4])
            .unwrap()
    );
    assert!(
        reference
            .place_components(&mut reference_world, &mut reference_random, [3, 3, -4])
            .unwrap()
    );
    assert_eq!(installed_world.writes, reference_world.writes);
    assert_eq!(installed_random.next_long(), reference_random.next_long());
    let mut expected = Vec::new();
    for seed in [0, 1, -1, 987654321] {
        for mode in 0..6 {
            for ignore in [false, true] {
                for fancy in [false, true] {
                    for branching in [false, true] {
                        let mut config = definition(ignore);
                        if fancy {
                            config["foliage_placer"]["type"] =
                                json!("minecraft:fancy_foliage_placer");
                        }
                        if branching {
                            config["trunk_placer"]["type"] = json!("minecraft:fancy_trunk_placer");
                        }
                        let tree = TreeFeature::decode(&mut Data, &config, &mut resolve).unwrap();
                        let mut world = World {
                            blocks: BTreeMap::from([([3, 2, -4], state("minecraft:stone"))]),
                            writes: Vec::new(),
                        };
                        match mode {
                            1 => {
                                world.blocks.insert([3, 5, -4], state("minecraft:stone"));
                            }
                            2 => {
                                let mut s = state("minecraft:oak_leaves");
                                s.persistent = true;
                                world.blocks.insert([4, 6, -4], s);
                            }
                            3 => {
                                world.blocks.insert([2, 6, -4], state("minecraft:water"));
                            }
                            4 => {
                                world.blocks.insert([3, 5, -4], state("minecraft:vine"));
                            }
                            5 => {
                                world.blocks.insert([3, 5, -4], state("minecraft:oak_log"));
                            }
                            _ => {}
                        }
                        let mut random = DecorationRandom::new(seed);
                        let ok = tree
                            .place_components(&mut world, &mut random, [3, 3, -4])
                            .unwrap();
                        expected.push(format!(
                            "{} {seed} {mode} {ignore} {ok} {} R {}",
                            match (branching, fancy) {
                                (false, false) => "C",
                                (false, true) => "F",
                                (true, false) => "B",
                                (true, true) => "G",
                            },
                            world.writes.join(";"),
                            random.next_long()
                        ));
                    }
                }
            }
        }
    }
    let actual: Vec<_> = actual
        .lines()
        .filter(|line| {
            ["C ", "F ", "B ", "G "]
                .iter()
                .any(|prefix| line.starts_with(prefix))
        })
        .collect();
    assert_eq!(actual.len(), expected.len());
    for (i, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
        assert_eq!(*actual, expected, "case {i}");
    }
}
