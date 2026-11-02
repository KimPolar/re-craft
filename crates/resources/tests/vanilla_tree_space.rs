use anyhow::Result;
use native_resources::ore_feature::OreWorld;
use native_resources::packs::{ResourceId, ResourceProvider, ResourceStack};
use native_resources::tree_placers::{
    TreeClearanceWorld, TreeFeatureSize, VanillaTreeClearance, permitted_tree_height,
};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};

struct World {
    mode: i32,
    calls: Vec<String>,
}

struct TaggedWorld(&'static str);
impl OreWorld for TaggedWorld {
    type State = &'static str;
    fn block(&mut self, _: [i32; 3]) -> Result<Self::State> {
        Ok(self.0)
    }
    fn block_is(&mut self, state: &Self::State, id: &ResourceId) -> Result<bool> {
        Ok(id.namespace == "minecraft" && id.path == *state)
    }
    fn in_tag(&mut self, state: &Self::State, tag: &ResourceId) -> Result<bool> {
        Ok(tag.namespace == "minecraft"
            && match tag.path.as_str() {
                "replaceable_by_trees" => matches!(*state, "vine" | "short_grass"),
                "logs" => *state == "oak_log",
                _ => anyhow::bail!("unexpected block tag"),
            })
    }
    fn is_air(&mut self, state: &Self::State) -> Result<bool> {
        Ok(*state == "air")
    }
    fn set_block(&mut self, _: [i32; 3], _: &Self::State, _: i32) -> Result<bool> {
        unreachable!()
    }
}
impl World {
    fn kind(&self, [x, y, z]: [i32; 3]) -> i32 {
        match self.mode {
            1 if [x, y, z] == [3, 20, -4] => 1,
            2 if [x, y, z] == [2, 23, -5] => 1,
            3 if [x, y, z] == [3, 22, -4] => 2,
            4 if [x, y, z] == [4, 26, -3] => 1,
            5 if (x * 3 + y * 5 + z * 7).rem_euclid(37) == 0 => 1,
            _ => 0,
        }
    }
}
impl TreeClearanceWorld for World {
    fn is_free(&mut self, p: [i32; 3]) -> Result<bool> {
        self.calls.push(format!("F{},{},{}", p[0], p[1], p[2]));
        Ok(self.kind(p) != 1)
    }
    fn is_vine(&mut self, p: [i32; 3]) -> Result<bool> {
        self.calls.push(format!("V{},{},{}", p[0], p[1], p[2]));
        Ok(self.kind(p) == 2)
    }
}

#[test]
fn feature_size_codec_enforces_vanilla_limits() {
    assert!(
        TreeFeatureSize::decode(&json!({"type":"minecraft:two_layers_feature_size","limit":82}))
            .is_err()
    );
    assert!(
        TreeFeatureSize::decode(
            &json!({"type":"minecraft:three_layers_feature_size","upper_size":17})
        )
        .is_err()
    );
    assert!(
        TreeFeatureSize::decode(
            &json!({"type":"minecraft:two_layers_feature_size","min_clipped_height":-1})
        )
        .is_err()
    );
    let mut world = World {
        mode: 0,
        calls: Vec::new(),
    };
    let size =
        TreeFeatureSize::decode(&json!({"type":"minecraft:two_layers_feature_size"})).unwrap();
    assert_eq!(
        permitted_tree_height(&mut world, [3, 20, -4], [3, 23, -4], 0, 25, 4, &size, true).unwrap(),
        None
    );
    assert!(world.calls.is_empty());
}

fn run(command: &mut Command) -> String {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn tree_clearance_and_clipping_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let mut resources = ResourceStack::open(&game).unwrap();
    for name in [
        "oak",
        "fancy_oak",
        "spruce",
        "mega_pine",
        "mangrove",
        "jungle_tree",
    ] {
        let path = format!("data/minecraft/worldgen/feature/{name}.json");
        let feature: Value =
            serde_json::from_slice(&resources.read_data(&path).unwrap().unwrap()).unwrap();
        TreeFeatureSize::decode(&feature["minimum_size"]).unwrap();
    }
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
    let source = temp.path().join("TreeSpaceOracle.java");
    std::fs::write(&source, include_str!("java/TreeSpaceOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let actual = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("TreeSpaceOracle"));
    let defs = [
        json!({"type":"minecraft:two_layers_feature_size"}),
        json!({"type":"minecraft:two_layers_feature_size","limit":2,"lower_size":1,"upper_size":2,"min_clipped_height":0}),
        json!({"type":"minecraft:three_layers_feature_size","limit":2,"upper_limit":3,"lower_size":0,"middle_size":2,"upper_size":1,"min_clipped_height":4}),
    ];
    let mut expected = Vec::new();
    for (si, definition) in defs.iter().enumerate() {
        let size = TreeFeatureSize::decode(definition).unwrap();
        for height in [0, 4, 6, 12] {
            for ignore in [false, true] {
                for mode in 0..6 {
                    for bound in 0..3 {
                        let mut world = World {
                            mode,
                            calls: Vec::new(),
                        };
                        let result = permitted_tree_height(
                            &mut world,
                            [3, 20, -4],
                            [3, 20, -4],
                            if bound == 1 { 20 } else { 0 },
                            if bound == 2 { 22 } else { 255 },
                            height,
                            &size,
                            ignore,
                        )
                        .unwrap();
                        expected.push(format!(
                            "C {si} {height} {ignore} {mode} {bound} {} {} {}",
                            result.is_some(),
                            result.unwrap_or(-999),
                            world.calls.join(";")
                        ));
                    }
                }
            }
        }
    }
    let actual_cases: Vec<_> = actual
        .lines()
        .filter(|line| line.starts_with("C "))
        .collect();
    assert_eq!(actual_cases.len(), expected.len());
    for (i, (actual, expected)) in actual_cases.iter().zip(&expected).enumerate() {
        assert_eq!(*actual, expected, "case {i}");
    }
    let blocks: Vec<_> = actual
        .lines()
        .filter(|line| line.starts_with("B "))
        .collect();
    let mut expected_blocks = Vec::new();
    for name in [
        "air",
        "stone",
        "oak_log",
        "vine",
        "short_grass",
        "oak_leaves",
    ] {
        let mut world = TaggedWorld(name);
        let mut clearance = VanillaTreeClearance(&mut world);
        let free = clearance.is_free([3, 20, -4]).unwrap();
        let vine = clearance.is_vine([3, 20, -4]).unwrap();
        expected_blocks.push(format!("B minecraft:{name} {free} {vine}"));
    }
    assert_eq!(blocks, expected_blocks);
}
