use anyhow::{Context, Result};
use native_resources::{
    ore_feature::OreWorld,
    packs::{ResourceId, ResourceProvider, ResourceStack},
    spring_feature::{SpringFeature, SpringWorld},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, process::Command};

#[derive(Clone, Debug, Eq, PartialEq)]
struct State {
    name: String,
    level: Option<String>,
}
fn resolve(value: &Value) -> Result<State> {
    Ok(State {
        name: value["id"].as_str().context("missing id")?.into(),
        level: value
            .get("properties")
            .and_then(|v| v.get("level"))
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}
fn state(n: i32) -> State {
    State {
        name: ["minecraft:air", "minecraft:stone", "minecraft:dirt"][n as usize].into(),
        level: None,
    }
}
struct World {
    blocks: BTreeMap<[i32; 3], State>,
    ticks: Vec<(ResourceId, [i32; 3])>,
    restricted: bool,
}
impl OreWorld for World {
    type State = State;
    fn block(&mut self, p: [i32; 3]) -> Result<State> {
        Ok(self.blocks.get(&p).cloned().unwrap_or_else(|| state(0)))
    }
    fn block_is(&mut self, s: &State, id: &ResourceId) -> Result<bool> {
        Ok(s.name == format!("{}:{}", id.namespace, id.path))
    }
    fn in_tag(&mut self, _: &State, _: &ResourceId) -> Result<bool> {
        unreachable!()
    }
    fn is_air(&mut self, s: &State) -> Result<bool> {
        Ok(s.name == "minecraft:air")
    }
    fn set_block(&mut self, _: [i32; 3], _: &State, _: i32) -> Result<bool> {
        unreachable!()
    }
}
impl SpringWorld for World {
    fn set_spring_block(&mut self, p: [i32; 3], s: &State) -> Result<bool> {
        if !self.restricted {
            self.blocks.insert(p, s.clone());
        }
        Ok(!self.restricted)
    }
    fn schedule_spring_tick(&mut self, p: [i32; 3], fluid: &ResourceId) -> Result<()> {
        if !self.ticks.contains(&(fluid.clone(), p)) {
            self.ticks.push((fluid.clone(), p));
        }
        Ok(())
    }
}
fn run(cmd: &mut Command) -> String {
    let result = cmd.output().unwrap();
    assert!(
        result.status.success(),
        "Java spring oracle failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}

#[test]
fn spring_rejects_invalid_states_and_reports_referenced_blocks() {
    let make = |state: Value, blocks: Value| json!({"type":"minecraft:spring_feature","state":state,"valid_blocks":blocks});
    let feature = SpringFeature::decode(
        &make(
            json!({"id":"minecraft:water"}),
            json!(["minecraft:stone", "minecraft:dirt"]),
        ),
        &mut resolve,
    )
    .unwrap();
    assert_eq!(
        feature.required_blocks(),
        [
            ResourceId::parse("minecraft:stone").unwrap(),
            ResourceId::parse("minecraft:dirt").unwrap()
        ]
        .into_iter()
        .collect()
    );
    let tagged = SpringFeature::decode(
        &make(json!({"id":"minecraft:lava"}), json!("#example:rock")),
        &mut resolve,
    )
    .unwrap();
    assert_eq!(
        tagged.required_tags(),
        [ResourceId::parse("example:rock").unwrap()]
            .into_iter()
            .collect()
    );
    for invalid in [
        json!({"id":"minecraft:water","properties":{"level":"3"}}),
        json!({"id":"minecraft:flowing_water","properties":{"level":"9"}}),
        json!({"id":"minecraft:lava","properties":{"falling":"yes"}}),
        json!({"id":"example:fluid"}),
    ] {
        assert!(
            SpringFeature::decode(&make(invalid, json!(["minecraft:stone"])), &mut resolve)
                .is_err()
        );
    }
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn spring_placement_and_ticks_match_java() {
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
    let source = temp.path().join("SpringFeatureOracle.java");
    std::fs::write(&source, include_str!("java/SpringFeatureOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(source));
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("SpringFeatureOracle"));
    let positions = [
        [0, 1, 0],
        [0, -1, 0],
        [0, 0, 0],
        [-1, 0, 0],
        [1, 0, 0],
        [0, 0, -1],
        [0, 0, 1],
    ];
    let mut fluid_cases = 0;
    for line in output.lines().filter(|s| s.starts_with("F ")) {
        let parts: Vec<_> = line.split_whitespace().collect();
        let props = if parts[2] == "default" {
            json!({})
        } else {
            json!({"level":parts[2],"falling":parts[3]})
        };
        let feature = SpringFeature::decode(&json!({"type":"spring_feature","state":{"id":parts[1],"properties":props},"valid_blocks":["minecraft:stone"]}), &mut resolve).unwrap();
        let mut blocks = BTreeMap::new();
        for p in positions {
            blocks.insert(p, state(1));
        }
        blocks.insert([-1, 0, 0], state(0));
        let mut world = World {
            blocks,
            ticks: Vec::new(),
            restricted: false,
        };
        assert!(feature.place(&mut world, [0, 0, 0]).unwrap());
        assert_eq!(
            world.blocks[&[0, 0, 0]].level.as_deref().unwrap(),
            parts[4],
            "{line}"
        );
        fluid_cases += 1;
    }
    assert_eq!(fluid_cases, 34);
    let mut features = Vec::new();
    for config in 0..16 {
        let fluid = match (config & 1, config & 8) {
            (0, 0) => "minecraft:water",
            (1, 0) => "minecraft:lava",
            (0, _) => "minecraft:flowing_water",
            _ => "minecraft:flowing_lava",
        };
        let properties = if config & 8 == 0 {
            json!({"falling":"true"})
        } else {
            json!({"falling":"true","level":"3"})
        };
        features.push(SpringFeature::decode(&json!({"type":"minecraft:spring_feature","state":{"id":fluid,"properties":properties},"requires_block_below":config&2==0,"valid_blocks":["minecraft:stone"]}),&mut resolve).unwrap());
    }
    let mut count = 0;
    for line in output.lines().filter(|s| s.starts_with("S ")) {
        let parts: Vec<_> = line.split_whitespace().collect();
        let code: i32 = parts[1].parse().unwrap();
        let config: usize = parts[2].parse().unwrap();
        let mut n = code;
        let mut blocks = BTreeMap::new();
        for p in positions {
            blocks.insert(p, state(n % 3));
            n /= 3;
        }
        let mut world = World {
            blocks,
            ticks: Vec::new(),
            restricted: config & 4 != 0,
        };
        let first = features[config].place(&mut world, [0, 0, 0]).unwrap();
        let second = features[config].place(&mut world, [0, 0, 0]).unwrap();
        assert_eq!(first, parts[3].parse::<bool>().unwrap(), "{code} {config}");
        assert_eq!(second, parts[4].parse::<bool>().unwrap(), "{code} {config}");
        let final_state = world.blocks.get(&[0, 0, 0]).unwrap();
        assert_eq!(final_state.name, parts[5], "{code} {config}");
        assert_eq!(
            final_state.level.as_deref().unwrap_or("-"),
            parts[6],
            "{code} {config}"
        );
        let tick = world
            .ticks
            .first()
            .map(|(id, _)| format!("{}:{}", id.namespace, id.path))
            .unwrap_or_else(|| "-".into());
        assert_eq!(tick, parts[7], "{code} {config}");
        assert_eq!(
            world.ticks.len(),
            parts[8].parse::<usize>().unwrap(),
            "{code} {config}"
        );
        count += 1;
    }
    assert_eq!(count, 2187 * 16);
    let mut resources = ResourceStack::open(&game).unwrap();
    for name in [
        "spring_water",
        "spring_lava_overworld",
        "spring_lava_frozen",
        "spring_lava_nether",
    ] {
        let path = format!("data/minecraft/worldgen/feature/{name}.json");
        let bytes = resources.read_data(&path).unwrap().unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        SpringFeature::decode(&value, &mut resolve).unwrap_or_else(|e| panic!("{name}: {e:#}"));
    }
}
