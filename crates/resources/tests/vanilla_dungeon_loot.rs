//! Compare vanilla dungeon loot, enchanted books and RNG state to Java 26.3.
use std::{path::PathBuf, process::Command};

use native_core::random::{LegacyRandom, XoroshiroRandom};
use native_resources::{
    chest_loot::{ChestLootItem, ChestLootTable},
    packs::{ResourceId, ResourceProvider, ResourceStack},
};
use serde_json::{Value, json};

fn run(command: &mut Command) -> String {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "Java oracle failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
#[ignore = "requires Minecraft 26.3 and a Java 25 JDK"]
fn dungeon_loot_matches_java_including_enchanted_books() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
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
    let source = temp.path().join("ChestLootOracle.java");
    std::fs::write(&source, include_str!("java/ChestLootOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let mut resources = ResourceStack::open(&game).unwrap();
    let raw = resources
        .read_data("data/minecraft/loot_table/chests/simple_dungeon.json")
        .unwrap()
        .unwrap();
    let input = temp.path().join("loot.json");
    let definition: Value = serde_json::from_slice(&raw).unwrap();
    std::fs::write(&input, serde_json::to_vec(&json!([definition])).unwrap()).unwrap();
    let oracle = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("ChestLootOracle")
        .arg(&input)
        .arg("enchant"));
    let table = ChestLootTable::load(
        &mut resources,
        &ResourceId::parse("minecraft:chests/simple_dungeon").unwrap(),
    )
    .unwrap();
    let max = |item: &ChestLootItem| -> anyhow::Result<u32> {
        let max = match item.id.path.as_str() {
            "enchanted_book"
            | "music_disc_otherside"
            | "music_disc_13"
            | "music_disc_cat"
            | "golden_horse_armor"
            | "copper_horse_armor"
            | "iron_horse_armor"
            | "diamond_horse_armor" => 1,
            "book"
            | "leather"
            | "golden_apple"
            | "enchanted_golden_apple"
            | "name_tag"
            | "iron_ingot"
            | "gold_ingot"
            | "bread"
            | "wheat"
            | "redstone"
            | "coal"
            | "melon_seeds"
            | "pumpkin_seeds"
            | "beetroot_seeds"
            | "bone"
            | "gunpowder"
            | "rotten_flesh"
            | "string" => 64,
            "bucket" => 16,
            id => anyhow::bail!("unknown loot item {id}"),
        };
        Ok(max)
    };
    let mut lines = oracle
        .lines()
        .filter(|line| line.starts_with("S ") || line.starts_with("P ") || line.starts_with("R "))
        .peekable();
    let mut cases = 0;
    let mut enchanted = 0;
    while let Some(header) = lines.next() {
        let fields: Vec<_> = header.split_whitespace().collect();
        assert_eq!(fields[0], "S");
        let luck: f32 = fields[2].parse().unwrap();
        let mode: u8 = fields[3].parse().unwrap();
        let seed: i64 = fields[4].parse().unwrap();
        let mut expected = Vec::new();
        while lines.peek().is_some_and(|line| line.starts_with("P ")) {
            expected.push(lines.next().unwrap().to_owned());
        }
        let expected_next: i64 = lines
            .next()
            .unwrap()
            .strip_prefix("R ")
            .unwrap()
            .parse()
            .unwrap();
        let (slots, next) = if mode == 0 {
            let mut rng = LegacyRandom::new(seed);
            let slots = table
                .fill_with_random_and_luck(&[false; 27], &mut rng, luck, max)
                .unwrap();
            (slots, rng.next_long())
        } else {
            let mut rng = XoroshiroRandom::new(seed);
            let slots = table
                .fill_with_random_and_luck(&[false; 27], &mut rng, luck, max)
                .unwrap();
            (slots, rng.next_long())
        };
        let actual: Vec<_> = slots
            .into_iter()
            .enumerate()
            .filter_map(|(slot, item)| {
                item.map(|item| {
                    let enchantment = item.enchantments.first();
                    if enchantment.is_some() {
                        enchanted += 1;
                    }
                    let id = enchantment
                        .map(|value| format!("{}:{}", value.id.namespace, value.id.path))
                        .unwrap_or_else(|| "-".into());
                    let level = enchantment.map_or(0, |value| value.level);
                    format!(
                        "P {slot} {}:{} {} {id} {level}",
                        item.id.namespace, item.id.path, item.count
                    )
                })
            })
            .collect();
        assert_eq!(actual, expected, "{header}");
        assert_eq!(next, expected_next, "{header}");
        cases += 1;
    }
    assert_eq!(cases, 32);
    assert!(enchanted > 0, "the oracle did not exercise enchanted books");
    let mut unsupported = ChestLootTable::decode(&json!({
        "type":"minecraft:chest", "pools":[{"rolls":1,"entries":[{
            "type":"minecraft:item", "name":"minecraft:diamond_sword",
            "modifier":{"type":"minecraft:enchant_randomly","options":["minecraft:sharpness"]}
        }]}]
    }))
    .unwrap();
    unsupported
        .resolve_random_enchantments(&mut resources)
        .unwrap();
    let mut random = LegacyRandom::new(42);
    let before = random.state();
    assert!(
        unsupported
            .fill_with_random(&[false; 27], &mut random, |_| Ok(1_u32))
            .is_err()
    );
    assert_eq!(random.state(), before);
}
