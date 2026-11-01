//! Compare 26.3 buried-treasure loot and filled slots with Java.
use native_core::random::{LegacyRandom, XoroshiroRandom};
use native_resources::{
    chest_loot::{ChestLootItem, ChestLootTable},
    packs::{ResourceId, ResourceProvider, ResourceStack},
};
use serde_json::Value;
use std::{path::PathBuf, process::Command};
fn run(c: &mut Command) -> String {
    let o = c.output().unwrap();
    assert!(
        o.status.success(),
        "Java chest-loot oracle failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8(o.stdout).unwrap()
}
fn max_stack(item: &ChestLootItem) -> anyhow::Result<u32> {
    Ok(match item.id.path.as_str() {
        "potion"
        | "leather_chestplate"
        | "iron_sword"
        | "iron_spear"
        | "copper_nautilus_armor"
        | "iron_nautilus_armor"
        | "golden_nautilus_armor"
        | "diamond_nautilus_armor" => 1,
        _ => 64,
    })
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn buried_treasure_chest_loot_matches_java() {
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
        if let Some(p) = lib["downloads"]["artifact"]["path"].as_str() {
            let p = game.join("libraries").join(p);
            if p.is_file() {
                paths.push(p)
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("ChestLootOracle.java");
    std::fs::write(&source, include_str!("java/ChestLootOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let mut resources = ResourceStack::open(&game).unwrap();
    let bytes = resources
        .read_builtin_data("data/minecraft/loot_table/chests/buried_treasure.json")
        .unwrap()
        .unwrap();
    let input = temp.path().join("loot.json");
    std::fs::write(&input, &bytes).unwrap();
    let table = ChestLootTable::load(
        &mut resources,
        &ResourceId::parse("minecraft:chests/buried_treasure").unwrap(),
    )
    .unwrap();
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("ChestLootOracle")
        .arg(&input));
    let prototypes: Vec<_> = output
        .lines()
        .filter(|line| line.starts_with("V "))
        .collect();
    assert_eq!(
        prototypes,
        [
            "V minecraft:heart_of_the_sea 64 0 net.minecraft.world.item.Item",
            "V minecraft:tnt 64 0 net.minecraft.world.item.BlockItem",
            "V minecraft:prismarine_crystals 64 0 net.minecraft.world.item.Item",
            "V minecraft:potion 1 0 net.minecraft.world.item.PotionItem",
            "V minecraft:iron_spear 1 250 net.minecraft.world.item.Item",
            "V minecraft:copper_nautilus_armor 1 0 net.minecraft.world.item.Item",
            "V minecraft:iron_nautilus_armor 1 0 net.minecraft.world.item.Item",
            "V minecraft:golden_nautilus_armor 1 0 net.minecraft.world.item.Item",
            "V minecraft:diamond_nautilus_armor 1 0 net.minecraft.world.item.Item",
        ]
    );
    let potions: Vec<_> = output
        .lines()
        .filter(|line| line.starts_with("N "))
        .collect();
    assert!(!potions.is_empty());
    assert!(potions.iter().all(|line| *line == "N {components:{\"minecraft:potion_contents\":{potion:\"minecraft:water_breathing\"}},count:1,id:\"minecraft:potion\"}"));
    let mut lines = output
        .lines()
        .filter(|l| l.starts_with("S ") || l.starts_with("P ") || l.starts_with("R "))
        .peekable();
    let mut cases = 0;
    while let Some(header) = lines.next() {
        let h: Vec<_> = header.split_whitespace().collect();
        assert_eq!(h[0], "S");
        let mode: i32 = h[1].parse().unwrap();
        let seed: i64 = h[2].parse().unwrap();
        let mut expected = Vec::new();
        while lines.peek().is_some_and(|s| s.starts_with("P ")) {
            expected.push(lines.next().unwrap().to_owned());
        }
        let next = lines
            .next()
            .unwrap()
            .strip_prefix("R ")
            .unwrap()
            .parse::<i64>()
            .unwrap();
        let (slots, actual_next) = if mode == 0 {
            let mut rng = LegacyRandom::new(seed);
            let slots = table
                .fill_with_random(&[false; 27], &mut rng, max_stack)
                .unwrap();
            assert_eq!(
                slots,
                table.fill_seeded(&[false; 27], seed, max_stack).unwrap()
            );
            (slots, rng.next_long())
        } else {
            let mut rng = if mode == 1 {
                XoroshiroRandom::new(seed)
            } else {
                assert_eq!(mode, 2);
                XoroshiroRandom::for_world_sequence(
                    seed,
                    0,
                    true,
                    Some("minecraft:chests/buried_treasure"),
                )
            };
            let slots = table
                .fill_with_random(&[false; 27], &mut rng, max_stack)
                .unwrap();
            (slots, rng.next_long())
        };
        let actual: Vec<_> = slots
            .iter()
            .enumerate()
            .filter_map(|(slot, item)| {
                item.as_ref().map(|item| {
                    let potion = item
                        .potion
                        .as_ref()
                        .map(|id| format!("{}:{}", id.namespace, id.path))
                        .unwrap_or_else(|| "-".into());
                    format!(
                        "P {slot} {}:{} {} {potion}",
                        item.id.namespace, item.id.path, item.count
                    )
                })
            })
            .collect();
        assert_eq!(actual, expected, "mode={mode} seed={seed}");
        assert_eq!(actual_next, next, "random state mode={mode} seed={seed}");
        cases += 1;
    }
    assert_eq!(cases, 32);
}
