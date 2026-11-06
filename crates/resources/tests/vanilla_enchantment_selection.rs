//! Compare enchantment selection and RNG state against the 26.3 client.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    process::Command,
};

use anyhow::{Context, Result, bail, ensure};
use native_core::random::{LegacyRandom, XoroshiroRandom};
use native_resources::{
    enchantment_selection::{self, Candidate, LevelCost},
    packs::{ResourceId, ResourceProvider, ResourceStack},
    worldgen_region_random::RegionRandom,
};
use serde_json::Value;

struct ProviderOverlay<'a> {
    base: &'a mut ResourceStack,
    definition: Vec<u8>,
}
impl ResourceProvider for ProviderOverlay<'_> {
    fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        self.base.read_asset(path)
    }
    fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
        if path == "data/minecraft/enchantment_provider/mob_spawn_equipment.json" {
            Ok(Some(self.definition.clone()))
        } else {
            self.base.read_data(path)
        }
    }
    fn read_data_stack(&mut self, path: &str) -> Result<Vec<Vec<u8>>> {
        self.base.read_data_stack(path)
    }
    fn list_data(&mut self, folder: &str) -> Result<Vec<ResourceId>> {
        self.base.list_data(folder)
    }
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn enchantment_selection_matches_java() -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir()?;
    let version = game.join("versions/26.3");
    let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    ensure!(manifest["id"] == "26.3", "wrong version");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"]
        .as_array()
        .context("libraries missing")?
    {
        if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths)?;
    let source = temp.path().join("EnchantmentSelectionOracle.java");
    std::fs::write(
        &source,
        include_str!("java/EnchantmentSelectionOracle.java"),
    )?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source)
        .output()?;
    ensure!(
        compiled.status.success(),
        "Java compile failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(cp)
        .arg("EnchantmentSelectionOracle")
        .output()?;
    ensure!(
        output.status.success(),
        "Java oracle failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let output = String::from_utf8(output.stdout)?;
    let mut resources = native_resources::packs::ResourceStack::open(&game)?;
    let items = resources.vanilla_item_registry()?;
    let known_items: BTreeSet<_> = items
        .iter()
        .map(|(id, _)| ResourceId::parse(id))
        .collect::<Result<_>>()?;
    let mut candidates = BTreeMap::<String, Vec<Candidate>>::new();
    let mut providers =
        BTreeMap::<String, native_resources::enchantment_provider::DifficultyProvider>::new();
    let mut custom_providers =
        BTreeMap::<String, native_resources::enchantment_provider::DifficultyProvider>::new();
    let mut tested = 0;
    let mut multi = 0;
    let mut cost_tested = 0;
    let mut provider_tested = 0;
    let mut custom_tested = 0;
    for line in output.lines().filter(|line| {
        line.starts_with("C\t")
            || line.starts_with("R\t")
            || line.starts_with("D\t")
            || line.starts_with("P\t")
            || line.starts_with("Q\t")
    }) {
        let parts: Vec<_> = line.split('\t').collect();
        if parts[0] == "D" {
            ensure!(parts.len() == 6, "invalid provider cost row");
            let seed = parts[2].parse()?;
            let mut random = if parts[1] == "0" {
                RegionRandom::Legacy(LegacyRandom::new(seed))
            } else {
                RegionRandom::Xoroshiro(XoroshiroRandom::new(seed))
            };
            let special = f32::from_bits(parts[3].parse()?);
            assert_eq!(
                enchantment_selection::difficulty_cost(&mut random, 5, 17, special)?,
                parts[4].parse::<i32>()?
            );
            assert_eq!(random.next_long(), parts[5].parse::<i64>()?);
            cost_tested += 1;
        } else if parts[0] == "Q" {
            ensure!(parts.len() == 9, "invalid custom provider row");
            let key = format!("{}:{}:{}", parts[1], parts[4], parts[5]);
            if !custom_providers.contains_key(&key) {
                let number = match parts[5] {
                    "0" => serde_json::json!(-2),
                    "1" => {
                        serde_json::json!({"type":"minecraft:uniform","min_inclusive":0,"max_inclusive":8})
                    }
                    "2" => {
                        serde_json::json!({"type":"minecraft:biased_to_bottom","min_inclusive":-3,"max_inclusive":12})
                    }
                    "3" => {
                        serde_json::json!({"type":"minecraft:trapezoid","min":0,"max":12,"plateau":2})
                    }
                    "4" => {
                        serde_json::json!({"type":"minecraft:clamped","source":{"type":"minecraft:uniform","min_inclusive":-10,"max_inclusive":20},"min_inclusive":1,"max_inclusive":7})
                    }
                    "5" => serde_json::json!(i32::MAX),
                    "6" => {
                        serde_json::json!({"type":"minecraft:weighted_list","distribution":[{"weight":3,"data":2},{"weight":2,"data":{"type":"minecraft:uniform","min_inclusive":3,"max_inclusive":9}}]})
                    }
                    "7" => {
                        serde_json::json!({"type":"minecraft:very_biased_to_bottom","min_inclusive":-5,"max_inclusive":13})
                    }
                    _ => bail!("unknown integer provider case"),
                };
                let definition = if parts[4] == "0" {
                    serde_json::json!({"type":"minecraft:single","enchantment":"minecraft:loyalty","level":number})
                } else {
                    serde_json::json!({"type":"minecraft:by_cost","enchantments":"#minecraft:on_mob_spawn_equipment","cost":number})
                };
                let mut overlay = ProviderOverlay {
                    base: &mut resources,
                    definition: serde_json::to_vec(&definition)?,
                };
                let prepared = native_resources::enchantment_provider::DifficultyProvider::load(
                    &mut overlay,
                    &ResourceId::parse("minecraft:mob_spawn_equipment")?,
                    &known_items,
                    &ResourceId::parse(parts[1])?,
                    Some(parts[6].parse()?),
                )?;
                custom_providers.insert(key.clone(), prepared);
            }
            let seed = parts[3].parse()?;
            let mut random = if parts[2] == "0" {
                RegionRandom::Legacy(LegacyRandom::new(seed))
            } else {
                RegionRandom::Xoroshiro(XoroshiroRandom::new(seed))
            };
            let selected = custom_providers[&key].enchant(&mut random, 0.5)?;
            let mut actual: Vec<_> = selected
                .iter()
                .map(|entry| format!("{}:{}={}", entry.id.namespace, entry.id.path, entry.level))
                .collect();
            actual.sort();
            assert_eq!(
                actual.join(","),
                parts[7],
                "custom provider {key} seed {seed}"
            );
            assert_eq!(
                random.next_long(),
                parts[8].parse::<i64>()?,
                "custom provider RNG {key} seed {seed}"
            );
            custom_tested += 1;
        } else if parts[0] == "C" {
            ensure!(parts.len() == 6, "invalid candidate row");
            let levels = parts[4]
                .split(',')
                .map(|entry| -> Result<LevelCost> {
                    let values: Vec<_> = entry.split(':').collect();
                    ensure!(values.len() == 3, "invalid level cost");
                    Ok(LevelCost {
                        level: values[0].parse()?,
                        min: values[1].parse()?,
                        max: values[2].parse()?,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let exclusive = parts[5]
                .split(',')
                .filter(|id| !id.is_empty())
                .map(ResourceId::parse)
                .collect::<Result<BTreeSet<_>>>()?;
            candidates
                .entry(parts[1].into())
                .or_default()
                .push(Candidate {
                    id: ResourceId::parse(parts[2])?,
                    weight: parts[3].parse()?,
                    levels,
                    exclusive,
                });
        } else {
            ensure!(
                parts.len() == if parts[0] == "P" { 9 } else { 8 },
                "invalid selection row"
            );
            let seed = parts[3].parse()?;
            let mut random = if parts[2] == "0" {
                RegionRandom::Legacy(LegacyRandom::new(seed))
            } else {
                RegionRandom::Xoroshiro(XoroshiroRandom::new(seed))
            };
            let java_candidates = candidates.get(parts[1]).context("candidate set missing")?;
            if parts[1].starts_with("minecraft:") && !providers.contains_key(parts[1]) {
                let prepared = native_resources::enchantment_provider::DifficultyProvider::load(
                    &mut resources,
                    &ResourceId::parse("minecraft:mob_spawn_equipment")?,
                    &known_items,
                    &ResourceId::parse(parts[1])?,
                    Some(parts[5].parse()?),
                )?;
                let native = prepared.candidates();
                assert_eq!(
                    native.len(),
                    java_candidates.len(),
                    "candidate count: {}",
                    parts[1]
                );
                for (actual, expected) in native.iter().zip(java_candidates) {
                    assert_eq!(
                        (&actual.id, actual.weight, &actual.levels),
                        (&expected.id, expected.weight, &expected.levels),
                        "candidate definition: {}",
                        parts[1]
                    );
                    for other in native {
                        if actual.id == other.id {
                            continue;
                        }
                        let incompatible = actual.exclusive.contains(&other.id)
                            || other.exclusive.contains(&actual.id);
                        assert_eq!(
                            incompatible,
                            expected.exclusive.contains(&other.id),
                            "candidate compatibility: {}",
                            parts[1]
                        );
                    }
                }
                providers.insert(parts[1].into(), prepared);
            }
            let actual = if parts[0] == "P" {
                provider_tested += 1;
                providers[parts[1]].enchant(&mut random, f32::from_bits(parts[4].parse()?))?
            } else {
                tested += 1;
                enchantment_selection::select(
                    &mut random,
                    parts[4].parse()?,
                    Some(parts[5].parse()?),
                    java_candidates,
                )?
            };
            let rendered = actual
                .iter()
                .map(|entry| format!("{}:{}={}", entry.id.namespace, entry.id.path, entry.level))
                .collect::<Vec<_>>()
                .join(",");
            assert_eq!(rendered, parts[6], "{} / {} / {}", parts[1], parts[2], seed);
            assert_eq!(
                random.next_long(),
                parts[7].parse::<i64>()?,
                "random state: {} / {} / {}",
                parts[1],
                parts[2],
                seed
            );
            multi += usize::from(actual.len() > 1);
        }
    }
    assert_eq!(tested, 5120);
    assert_eq!(cost_tested, 1024);
    assert_eq!(provider_tested, 2048);
    assert_eq!(custom_tested, 4096);
    assert!(
        multi > 0,
        "expected at least one multiple-enchantment result"
    );
    Ok(())
}
