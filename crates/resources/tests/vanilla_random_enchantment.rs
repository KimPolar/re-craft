//! Compare data-pack random enchantment selection and RNG state with Java 26.3.
use std::{collections::BTreeSet, path::PathBuf, process::Command, sync::Arc};

use native_core::random::{LegacyRandom, XoroshiroRandom};
use native_resources::{
    packs::{ResourceId, ResourceStack},
    random_enchantment::RandomEnchantment,
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
fn random_enchantment_selection_matches_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
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
    let source = temp.path().join("RandomEnchantmentOracle.java");
    std::fs::write(&source, include_str!("java/RandomEnchantmentOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let oracle = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("RandomEnchantmentOracle"));

    let mut resources = ResourceStack::open(&game).unwrap();
    let items: BTreeSet<_> = resources
        .vanilla_item_registry()
        .unwrap()
        .iter()
        .map(|(id, _)| ResourceId::parse(id).unwrap())
        .collect();
    let items = Arc::new(items);
    let options = [
        json!("#minecraft:on_random_loot"),
        json!([
            "minecraft:sharpness",
            "minecraft:protection",
            "minecraft:unbreaking"
        ]),
        json!(["minecraft:unbreaking"]),
        json!(["minecraft:protection"]),
        json!([]),
    ];
    let selectors: Vec<_> = options
        .iter()
        .enumerate()
        .map(|(i, options)| {
            RandomEnchantment::load(&mut resources, Arc::clone(&items), Some(options), i != 3)
                .unwrap()
        })
        .collect();
    let targets = [
        ResourceId::parse("minecraft:book").unwrap(),
        ResourceId::parse("minecraft:diamond_sword").unwrap(),
    ];
    let mut checked = 0;
    for line in oracle.lines().filter(|line| line.starts_with("E ")) {
        let fields: Vec<_> = line.split_whitespace().collect();
        assert_eq!(fields.len(), 8, "{line}");
        let option: usize = fields[1].parse().unwrap();
        let item: usize = fields[2].parse().unwrap();
        let mode: u8 = fields[3].parse().unwrap();
        let seed: i64 = fields[4].parse().unwrap();
        let (selected, next) = if mode == 0 {
            let mut random = LegacyRandom::new(seed);
            let selected = selectors[option]
                .select(&targets[item], &mut random)
                .unwrap();
            (selected, random.next_long())
        } else {
            let mut random = XoroshiroRandom::new(seed);
            let selected = selectors[option]
                .select(&targets[item], &mut random)
                .unwrap();
            (selected, random.next_long())
        };
        let actual = selected
            .map(|value| {
                (
                    format!("{}:{}", value.id.namespace, value.id.path),
                    value.level,
                )
            })
            .unwrap_or_else(|| ("-".into(), 0));
        assert_eq!(
            actual,
            (fields[5].to_owned(), fields[6].parse().unwrap()),
            "{line}"
        );
        assert_eq!(next, fields[7].parse::<i64>().unwrap(), "{line}");
        checked += 1;
    }
    assert_eq!(checked, 5 * 2 * 2 * 6);
}
