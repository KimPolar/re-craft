//! Compare every selected vanilla block state with Java's spawn predicates.
use native_world::{loaded::BlockState, mob_spawn_blocks::common_mob_spawn_block_facts};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, process::Command};

fn run(command: &mut Command) {
    let output = command.output().expect("cannot run Java 25");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn supported_block_states_match_all_java_entity_types() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
        .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let names = [
        "air",
        "cave_air",
        "void_air",
        "stone",
        "dirt",
        "grass_block",
        "coarse_dirt",
        "rooted_dirt",
        "podzol",
        "mycelium",
        "gravel",
        "sand",
        "red_sand",
        "bedrock",
        "granite",
        "diorite",
        "andesite",
        "tuff",
        "calcite",
        "deepslate",
        "clay",
        "moss_block",
        "snow_block",
        "ice",
        "packed_ice",
        "water",
        "lava",
        "short_grass",
        "fern",
        "dead_bush",
        "dandelion",
        "poppy",
        "seagrass",
        "tall_grass",
        "oak_leaves",
        "birch_leaves",
        "oak_log",
        "birch_log",
        "oak_slab",
        "stone_slab",
        "snow",
        "farmland",
        "magma_block",
        "cactus",
        "redstone_block",
        "coal_ore",
        "iron_ore",
        "copper_ore",
        "gold_ore",
        "redstone_ore",
        "emerald_ore",
        "lapis_ore",
        "diamond_ore",
        "deepslate_coal_ore",
        "deepslate_iron_ore",
        "deepslate_copper_ore",
        "deepslate_gold_ore",
        "deepslate_redstone_ore",
        "deepslate_emerald_ore",
        "deepslate_lapis_ore",
        "deepslate_diamond_ore",
        "glass",
        "torch",
        "moss_carpet",
    ];
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
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
    let source = temp.path().join("MobSpawnBlockFactsOracle.java");
    std::fs::write(
        &source,
        include_str!("../../resources/tests/java/MobSpawnBlockFactsOracle.java"),
    )
    .unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    let input = temp.path().join("input.json");
    let output = temp.path().join("output.json");
    std::fs::write(
        &input,
        serde_json::to_vec(
            &names
                .iter()
                .map(|s| format!("minecraft:{s}"))
                .collect::<Vec<_>>(),
        )
        .unwrap(),
    )
    .unwrap();
    run(Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg("MobSpawnBlockFactsOracle")
        .arg(&input)
        .arg(&output));
    let results: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    let mut checked = 0;
    let mut report = BTreeMap::<String, (usize, usize)>::new();
    for entry in results {
        let state = BlockState {
            name: entry["name"].as_str().unwrap().into(),
            properties: serde_json::from_value(entry["properties"].clone()).unwrap(),
        };
        let counts = report.entry(state.name.clone()).or_default();
        counts.0 += 1;
        if let Some(facts) = common_mob_spawn_block_facts(&state) {
            assert_eq!(entry["universal"], true, "{state:?}");
            assert_eq!(entry["context_independent"], true, "{state:?}");
            assert_eq!(entry["large"], false, "{state:?}");
            if facts.full_collision {
                assert_eq!(entry["canonical_full"], true, "{state:?}");
            } else if let Some(boxes) = native_world::collision::block_shape(&state) {
                let boxes: Vec<_> = boxes
                    .iter()
                    .map(|b| json!([b.min.x, b.min.y, b.min.z, b.max.x, b.max.y, b.max.z]))
                    .collect();
                assert_eq!(json!(boxes), entry["collision_boxes"], "{state:?}");
            }
            assert_eq!(
                json!([
                    facts.air,
                    facts.pathfindable_land,
                    facts.valid_support,
                    facts.full_collision,
                    facts.signal_source,
                    facts.fluid_empty,
                    facts.dangerous
                ]),
                entry["facts"],
                "{state:?}"
            );
            checked += 1;
            counts.1 += 1;
        }
    }
    let unsupported = [
        "ice",
        "lava",
        "oak_leaves",
        "birch_leaves",
        "oak_slab",
        "stone_slab",
        "snow",
        "magma_block",
        "cactus",
    ];
    for name in names {
        let counts = report.get(&format!("minecraft:{name}")).unwrap();
        if unsupported.contains(&name) {
            assert_eq!(counts.1, 0, "{name}");
        } else {
            assert_eq!(counts.1, counts.0, "missing supported states of {name}");
        }
    }
    assert!(checked > 75, "only {checked} block states checked");
}
