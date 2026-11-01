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
fn bonus_roll_and_item_damage_providers_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME")
        .or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
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
    let source = temp.path().join("ChestLootOracle.java");
    std::fs::write(&source, include_str!("java/ChestLootOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let providers = [
        serde_json::json!({"type":"minecraft:constant","value":0.75}),
        serde_json::json!({"type":"minecraft:uniform","min":-1.25,"max":2.75}),
        serde_json::json!({"type":"minecraft:uniform","min":1.0,"max":1.0}),
        serde_json::json!({"type":"minecraft:uniform","min":2.0,"max":-2.0}),
        serde_json::json!({"type":"minecraft:uniform","min":-3.4e38,"max":3.4e38}),
        serde_json::json!({"type":"minecraft:uniform","min":{"type":"minecraft:uniform","min":-1.0,"max":1.0},"max":{"type":"minecraft:uniform","min":1.0,"max":3.0}}),
    ];
    let values: Vec<_> = providers.iter().map(|bonus| serde_json::json!({
        "type":"minecraft:chest", "pools":[{
            "rolls":{"type":"minecraft:uniform","min":1,"max":4},
            "bonus_rolls":bonus,
            "entries":[{"type":"minecraft:item","name":"minecraft:stone","weight":2},{"type":"minecraft:item","name":"minecraft:stick","weight":3},{"type":"minecraft:empty"}]
        }]
    })).collect();
    let tables: Vec<_> = values
        .iter()
        .map(|value| ChestLootTable::decode(value).unwrap())
        .collect();
    let input = temp.path().join("providers.json");
    std::fs::write(&input, serde_json::to_vec(&values).unwrap()).unwrap();
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("ChestLootOracle")
        .arg(&input));
    let mut lines = output
        .lines()
        .filter(|line| line.starts_with("S ") || line.starts_with("P ") || line.starts_with("R "))
        .peekable();
    let mut cases = 0;
    while let Some(header) = lines.next() {
        let fields: Vec<_> = header.split_whitespace().collect();
        let index: usize = fields[1].parse().unwrap();
        let luck: f32 = fields[2].parse().unwrap();
        let mode: i32 = fields[3].parse().unwrap();
        let seed: i64 = fields[4].parse().unwrap();
        let mut expected = Vec::new();
        while lines.peek().is_some_and(|line| line.starts_with("P ")) {
            expected.push(lines.next().unwrap().to_owned());
        }
        let next: i64 = lines
            .next()
            .unwrap()
            .strip_prefix("R ")
            .unwrap()
            .parse()
            .unwrap();
        let (slots, actual_next) = if mode == 0 {
            let mut random = LegacyRandom::new(seed);
            let slots = tables[index]
                .fill_with_random_and_luck(&[false; 27], &mut random, luck, max_stack)
                .unwrap();
            (slots, random.next_long())
        } else {
            let mut random = XoroshiroRandom::new(seed);
            let slots = tables[index]
                .fill_with_random_and_luck(&[false; 27], &mut random, luck, max_stack)
                .unwrap();
            (slots, random.next_long())
        };
        let actual: Vec<_> = slots
            .iter()
            .enumerate()
            .filter_map(|(slot, item)| {
                item.as_ref().map(|item| {
                    format!(
                        "P {slot} {}:{} {}",
                        item.id.namespace, item.id.path, item.count
                    )
                })
            })
            .collect();
        assert_eq!(
            actual, expected,
            "provider={index} luck={luck} mode={mode} seed={seed}"
        );
        assert_eq!(
            actual_next, next,
            "provider={index} luck={luck} mode={mode} seed={seed}"
        );
        cases += 1;
    }
    assert_eq!(cases, providers.len() * 4 * 2 * 4);

    use native_resources::chest_loot::{ChestLootDurability, ChestLootPrototype};
    let mut resources = ResourceStack::open(&game).unwrap();
    let intersection: Value = serde_json::from_slice(
        &resources
            .read_builtin_data("data/minecraft/loot_table/chests/trial_chambers/intersection.json")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let values = vec![
        intersection,
        serde_json::json!({"type":"minecraft:chest","pools":[{"rolls":3,"entries":[{"type":"minecraft:item","name":"minecraft:diamond_axe","modifier":{"type":"minecraft:set_damage","damage":{"type":"minecraft:uniform","min":0.1,"max":0.5}}}]}]}),
        serde_json::json!({"type":"minecraft:chest","pools":[{"rolls":2,"entries":[{"type":"minecraft:item","name":"minecraft:stone_pickaxe","modifier":[{"type":"minecraft:set_damage","damage":0.25},{"type":"minecraft:set_damage","damage":{"type":"minecraft:uniform","min":-0.5,"max":0.5},"add":true}]}]}]}),
        serde_json::json!({"type":"minecraft:chest","pools":[{"rolls":2,"entries":[{"type":"minecraft:item","name":"minecraft:stone","modifier":{"type":"minecraft:set_damage","damage":{"type":"minecraft:uniform","min":0.1,"max":0.9}}}]}]}),
        serde_json::json!({"type":"minecraft:chest","pools":[{"rolls":2,"entries":[{"type":"minecraft:item","name":"minecraft:diamond_pickaxe","modifier":{"type":"minecraft:set_damage","damage":2.0}}]}]}),
        serde_json::json!({"type":"minecraft:chest","pools":[{"rolls":2,"entries":[{"type":"minecraft:item","name":"minecraft:diamond_pickaxe","modifier":[{"type":"minecraft:set_count","count":0},{"type":"minecraft:set_damage","damage":{"type":"minecraft:uniform","min":0.1,"max":0.9}},{"type":"minecraft:set_count","count":1}]}]}]}),
    ];
    let tables: Vec<_> = values
        .iter()
        .map(|value| ChestLootTable::decode(value).unwrap())
        .collect();
    let prototype = |item: &ChestLootItem| -> anyhow::Result<ChestLootPrototype> {
        let (max_stack, durability) = match item.id.path.as_str() {
            "diamond_axe" | "diamond_pickaxe" => (1, ChestLootDurability::Damageable(1561)),
            "stone_pickaxe" => (1, ChestLootDurability::Damageable(131)),
            "cake" => (1, ChestLootDurability::NotDamageable),
            "diamond_block" | "emerald_block" | "iron_block" | "amethyst_shard" | "diamond"
            | "stone" => (64, ChestLootDurability::NotDamageable),
            name => anyhow::bail!("unknown test item {name}"),
        };
        Ok(ChestLootPrototype {
            max_stack,
            durability,
        })
    };
    std::fs::write(&input, serde_json::to_vec(&values).unwrap()).unwrap();
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("ChestLootOracle")
        .arg(&input)
        .arg("damage"));
    let prototypes: Vec<_> = output
        .lines()
        .filter(|line| line.starts_with("V "))
        .collect();
    assert_eq!(
        prototypes,
        [
            "V minecraft:diamond_block 64 0",
            "V minecraft:emerald_block 64 0",
            "V minecraft:iron_block 64 0",
            "V minecraft:amethyst_shard 64 0",
            "V minecraft:cake 1 0",
            "V minecraft:diamond_axe 1 1561",
            "V minecraft:diamond_pickaxe 1 1561",
            "V minecraft:stone_pickaxe 1 131",
            "V minecraft:stone 64 0"
        ]
    );
    let mut lines = output
        .lines()
        .filter(|line| line.starts_with("S ") || line.starts_with("P ") || line.starts_with("R "))
        .peekable();
    let mut cases = 0;
    while let Some(header) = lines.next() {
        let fields: Vec<_> = header.split_whitespace().collect();
        let index: usize = fields[1].parse().unwrap();
        let luck: f32 = fields[2].parse().unwrap();
        let mode: i32 = fields[3].parse().unwrap();
        let seed: i64 = fields[4].parse().unwrap();
        let mut expected = Vec::new();
        while lines.peek().is_some_and(|line| line.starts_with("P ")) {
            expected.push(lines.next().unwrap().to_owned());
        }
        let next: i64 = lines
            .next()
            .unwrap()
            .strip_prefix("R ")
            .unwrap()
            .parse()
            .unwrap();
        let (slots, actual_next) = if mode == 0 {
            let mut random = LegacyRandom::new(seed);
            let slots = tables[index]
                .fill_with_random_and_luck(&[false; 27], &mut random, luck, prototype)
                .unwrap();
            (slots, random.next_long())
        } else {
            let mut random = XoroshiroRandom::new(seed);
            let slots = tables[index]
                .fill_with_random_and_luck(&[false; 27], &mut random, luck, prototype)
                .unwrap();
            (slots, random.next_long())
        };
        let actual: Vec<_> = slots
            .iter()
            .enumerate()
            .filter_map(|(slot, item)| {
                item.as_ref().map(|item| {
                    format!(
                        "P {slot} {}:{} {} {}",
                        item.id.namespace,
                        item.id.path,
                        item.count,
                        item.damage.unwrap_or(0)
                    )
                })
            })
            .collect();
        assert_eq!(
            actual, expected,
            "damage table={index} luck={luck} mode={mode} seed={seed}"
        );
        assert_eq!(
            actual_next, next,
            "damage table={index} luck={luck} mode={mode} seed={seed}"
        );
        cases += 1;
    }
    assert_eq!(cases, values.len() * 4 * 2 * 4);

    let supply: Value = serde_json::from_slice(
        &resources
            .read_builtin_data("data/minecraft/loot_table/chests/trial_chambers/supply.json")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let table = ChestLootTable::decode(&supply).unwrap();
    let resolve = |item: &ChestLootItem| -> anyhow::Result<ChestLootPrototype> {
        let (max_stack, durability) = match item.id.path.as_str() {
            "stone_pickaxe" => (1, ChestLootDurability::Damageable(131)),
            "potion" | "milk_bucket" => (1, ChestLootDurability::NotDamageable),
            "arrow" | "tipped_arrow" | "baked_potato" | "glow_berries" | "acacia_planks"
            | "moss_block" | "bone_meal" | "tuff" | "torch" => {
                (64, ChestLootDurability::NotDamageable)
            }
            id => anyhow::bail!("unknown supply item {id}"),
        };
        Ok(ChestLootPrototype {
            max_stack,
            durability,
        })
    };
    std::fs::write(&input, serde_json::to_vec(&[supply]).unwrap()).unwrap();
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("ChestLootOracle")
        .arg(&input)
        .arg("potion"));
    let mut lines = output
        .lines()
        .filter(|line| line.starts_with("S ") || line.starts_with("P ") || line.starts_with("R "))
        .peekable();
    let mut cases = 0;
    let mut potion_items = 0;
    while let Some(header) = lines.next() {
        let fields: Vec<_> = header.split_whitespace().collect();
        assert_eq!(fields[1], "0");
        let luck: f32 = fields[2].parse().unwrap();
        let mode: i32 = fields[3].parse().unwrap();
        let seed: i64 = fields[4].parse().unwrap();
        let mut expected = Vec::new();
        while lines.peek().is_some_and(|line| line.starts_with("P ")) {
            expected.push(lines.next().unwrap().to_owned());
        }
        let next: i64 = lines
            .next()
            .unwrap()
            .strip_prefix("R ")
            .unwrap()
            .parse()
            .unwrap();
        let (slots, actual_next) = if mode == 0 {
            let mut random = LegacyRandom::new(seed);
            let slots = table
                .fill_with_random_and_luck(&[false; 27], &mut random, luck, resolve)
                .unwrap();
            (slots, random.next_long())
        } else {
            let mut random = XoroshiroRandom::new(seed);
            let slots = table
                .fill_with_random_and_luck(&[false; 27], &mut random, luck, resolve)
                .unwrap();
            (slots, random.next_long())
        };
        let actual: Vec<_> = slots
            .iter()
            .enumerate()
            .filter_map(|(slot, item)| {
                item.as_ref().map(|item| {
                    if item.potion.is_some() {
                        potion_items += 1;
                    }
                    let potion = item
                        .potion
                        .as_ref()
                        .map(|id| format!("{}:{}", id.namespace, id.path))
                        .unwrap_or_else(|| "-".into());
                    format!(
                        "P {slot} {}:{} {} {potion} {}",
                        item.id.namespace,
                        item.id.path,
                        item.count,
                        item.damage.unwrap_or(0)
                    )
                })
            })
            .collect();
        assert_eq!(
            actual, expected,
            "supply luck={luck} mode={mode} seed={seed}"
        );
        assert_eq!(
            actual_next, next,
            "supply luck={luck} mode={mode} seed={seed}"
        );
        cases += 1;
    }
    assert_eq!(cases, 32);
    assert!(potion_items > 0);
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

    let synthetic = serde_json::json!({"type":"minecraft:chest","pools":[{"rolls":4,"bonus_rolls":0.5,"entries":[
        {"type":"minecraft:item","name":"minecraft:stone","weight":2,"quality":2},
        {"type":"minecraft:item","name":"minecraft:stick","weight":5,"quality":-1},
        {"type":"minecraft:empty","weight":1,"quality":1}
    ]}]});
    let table = ChestLootTable::decode(&synthetic).unwrap();
    assert!(table.requires_luck());
    std::fs::write(&input, serde_json::to_vec(&synthetic).unwrap()).unwrap();
    for luck in [-2.5_f32, -1.0, 0.0, 1.5, 7.0] {
        let output = run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("ChestLootOracle")
            .arg(&input)
            .arg(luck.to_string()));
        let mut lines = output
            .lines()
            .filter(|line| {
                line.starts_with("S ") || line.starts_with("P ") || line.starts_with("R ")
            })
            .peekable();
        let mut cases = 0;
        while let Some(header) = lines.next() {
            let fields: Vec<_> = header.split_whitespace().collect();
            let mode: i32 = fields[1].parse().unwrap();
            let seed: i64 = fields[2].parse().unwrap();
            let mut expected = Vec::new();
            while lines.peek().is_some_and(|line| line.starts_with("P ")) {
                expected.push(lines.next().unwrap().to_owned());
            }
            let next: i64 = lines
                .next()
                .unwrap()
                .strip_prefix("R ")
                .unwrap()
                .parse()
                .unwrap();
            let (slots, actual_next) = if mode == 0 {
                let mut random = LegacyRandom::new(seed);
                let slots = table
                    .fill_with_random_and_luck(&[false; 27], &mut random, luck, max_stack)
                    .unwrap();
                assert_eq!(
                    slots,
                    table
                        .fill_seeded_with_luck(&[false; 27], seed, luck, max_stack)
                        .unwrap()
                );
                (slots, random.next_long())
            } else {
                let mut random = if mode == 1 {
                    XoroshiroRandom::new(seed)
                } else {
                    XoroshiroRandom::for_world_sequence(
                        seed,
                        0,
                        true,
                        Some("minecraft:chests/buried_treasure"),
                    )
                };
                let slots = table
                    .fill_with_random_and_luck(&[false; 27], &mut random, luck, max_stack)
                    .unwrap();
                (slots, random.next_long())
            };
            let actual: Vec<_> = slots
                .iter()
                .enumerate()
                .filter_map(|(slot, item)| {
                    item.as_ref().map(|item| {
                        format!(
                            "P {slot} {}:{} {} -",
                            item.id.namespace, item.id.path, item.count
                        )
                    })
                })
                .collect();
            assert_eq!(actual, expected, "luck={luck} mode={mode} seed={seed}");
            assert_eq!(
                actual_next, next,
                "random luck={luck} mode={mode} seed={seed}"
            );
            cases += 1;
        }
        assert_eq!(cases, 32);
    }

    // Even when quality is zero, Java computes entry weights in f32.
    let synthetic = serde_json::json!({"type":"minecraft:chest","pools":[{"rolls":8,"entries":[
        {"type":"minecraft:item","name":"minecraft:stone","weight":16777217},
        {"type":"minecraft:item","name":"minecraft:stick","weight":16777219},
        {"type":"minecraft:item","name":"minecraft:dirt","weight":-1}
    ]}]});
    let table = ChestLootTable::decode(&synthetic).unwrap();
    std::fs::write(&input, serde_json::to_vec(&synthetic).unwrap()).unwrap();
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("ChestLootOracle")
        .arg(&input));
    let mut lines = output
        .lines()
        .filter(|line| line.starts_with("S ") || line.starts_with("P ") || line.starts_with("R "))
        .peekable();
    let mut cases = 0;
    while let Some(header) = lines.next() {
        let fields: Vec<_> = header.split_whitespace().collect();
        let mode: i32 = fields[1].parse().unwrap();
        let seed: i64 = fields[2].parse().unwrap();
        let mut expected = Vec::new();
        while lines.peek().is_some_and(|line| line.starts_with("P ")) {
            expected.push(lines.next().unwrap().to_owned());
        }
        let next: i64 = lines
            .next()
            .unwrap()
            .strip_prefix("R ")
            .unwrap()
            .parse()
            .unwrap();
        let (slots, actual_next) = if mode == 0 {
            let mut random = LegacyRandom::new(seed);
            let slots = table
                .fill_with_random(&[false; 27], &mut random, max_stack)
                .unwrap();
            (slots, random.next_long())
        } else {
            let mut random = if mode == 1 {
                XoroshiroRandom::new(seed)
            } else {
                XoroshiroRandom::for_world_sequence(
                    seed,
                    0,
                    true,
                    Some("minecraft:chests/buried_treasure"),
                )
            };
            let slots = table
                .fill_with_random(&[false; 27], &mut random, max_stack)
                .unwrap();
            (slots, random.next_long())
        };
        let actual: Vec<_> = slots
            .iter()
            .enumerate()
            .filter_map(|(slot, item)| {
                item.as_ref().map(|item| {
                    format!(
                        "P {slot} {}:{} {} -",
                        item.id.namespace, item.id.path, item.count
                    )
                })
            })
            .collect();
        assert_eq!(actual, expected, "weighted chest mode={mode} seed={seed}");
        assert_eq!(
            actual_next, next,
            "weighted chest random mode={mode} seed={seed}"
        );
        cases += 1;
    }
    assert_eq!(cases, 32);
}
