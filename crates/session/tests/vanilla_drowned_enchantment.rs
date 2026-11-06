//! Compare generated drowned equipment with Java 26.3 item serialization.
use std::{collections::BTreeSet, path::PathBuf, process::Command};

use anyhow::{Context, Result, bail, ensure};
use native_core::random::{LegacyRandom, XoroshiroRandom};
use native_persistence::nbt::Tag;
use native_resources::{
    drowned_spawn::MainHandRoll, packs::ResourceId, worldgen_region_random::RegionRandom,
};
use native_session::worldgen::ocean_ruin::{DrownedEquipmentEffects, DrownedSpawnEffects};
use native_world::loaded::World;
use serde_json::{Map, Value};

#[derive(Clone)]
struct NoPassengers;
impl DrownedSpawnEffects for NoPassengers {
    fn try_existing_chicken(&mut self, _: [i32; 3], _: &World) -> Result<bool> {
        bail!("unexpected chicken")
    }
    fn spawn_chicken_jockey(
        &mut self,
        _: [i32; 3],
        _: &World,
        _: &mut RegionRandom,
        _: &native_session::worldgen::BiomeNeighborhood,
    ) -> Result<bool> {
        bail!("unexpected chicken")
    }
    fn enchant_weapon(
        &mut self,
        _: [i32; 3],
        _: &World,
        _: &mut RegionRandom,
        _: MainHandRoll,
        _: f32,
    ) -> Result<Tag> {
        bail!("unexpected delegated enchantment")
    }
    fn spawn_zombie_nautilus(
        &mut self,
        _: [i32; 3],
        _: &World,
        _: &mut RegionRandom,
        _: bool,
    ) -> Result<bool> {
        bail!("unexpected nautilus")
    }
}

fn json(tag: &Tag) -> Result<Value> {
    Ok(match tag {
        Tag::Int(value) => Value::from(*value),
        Tag::String(value) => Value::String(String::from_utf16(&value.0)?),
        Tag::Compound(fields) => {
            let mut object = Map::new();
            for (name, value) in fields {
                ensure!(
                    object
                        .insert(String::from_utf16(&name.0)?, json(value)?)
                        .is_none(),
                    "duplicate compound key"
                );
            }
            Value::Object(object)
        }
        _ => bail!("unexpected equipment tag type"),
    })
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn generated_drowned_weapon_matches_java() -> Result<()> {
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
        include_str!("../../resources/tests/java/EnchantmentSelectionOracle.java"),
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
    if !output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout);
        let tail: String = text
            .chars()
            .rev()
            .take(3000)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        bail!(
            "Java oracle failed: {tail}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let output = String::from_utf8(output.stdout)?;
    let rows: Vec<Vec<_>> = output
        .lines()
        .filter(|line| line.starts_with("P\t"))
        .map(|line| line.split('\t').collect())
        .collect();
    assert_eq!(rows.len(), 2048);
    let enchantability = |id: &str| -> Result<i32> {
        rows.iter()
            .find(|row| row[1] == id)
            .context("item missing")?[5]
            .parse()
            .map_err(Into::into)
    };
    let mut resources = native_resources::packs::ResourceStack::open(&game)?;
    let registry = resources.vanilla_item_registry()?;
    let items: BTreeSet<_> = registry
        .iter()
        .map(|(id, _)| ResourceId::parse(id))
        .collect::<Result<_>>()?;
    let mut effects = DrownedEquipmentEffects::load(
        &mut resources,
        &items,
        Some(enchantability("minecraft:trident")?),
        Some(enchantability("minecraft:fishing_rod")?),
        NoPassengers,
    )?;
    let world = World::new("minecraft:overworld");
    for row in rows {
        ensure!(row.len() == 9, "invalid item oracle row");
        let seed = row[3].parse()?;
        let mut random = if row[2] == "0" {
            RegionRandom::Legacy(LegacyRandom::new(seed))
        } else {
            RegionRandom::Xoroshiro(XoroshiroRandom::new(seed))
        };
        let weapon = match row[1] {
            "minecraft:trident" => MainHandRoll::Trident,
            "minecraft:fishing_rod" => MainHandRoll::FishingRod,
            _ => bail!("unexpected weapon"),
        };
        let stack = effects.enchant_weapon(
            [0, 0, 0],
            &world,
            &mut random,
            weapon,
            f32::from_bits(row[4].parse()?),
        )?;
        assert_eq!(
            json(&stack)?,
            serde_json::from_str::<Value>(row[8])?,
            "{} / {} / {seed}",
            row[1],
            row[2]
        );
        assert_eq!(
            random.next_long(),
            row[7].parse::<i64>()?,
            "RNG {} / {} / {seed}",
            row[1],
            row[2]
        );
    }
    Ok(())
}
