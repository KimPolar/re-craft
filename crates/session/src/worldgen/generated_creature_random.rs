//! The independent entity RNG used during fresh creature construction.
use anyhow::{Context, Result, ensure};
use native_core::random::LegacyRandom;
use native_resources::packs::ResourceId;

use super::FreshCreatureConstruction;

/// Construction outputs and the still-live entity RNG. The region RNG and
/// the chunk spawner's decoration RNG are separate sources.
#[derive(Clone, Debug)]
pub struct FreshCreatureRandom {
    pub construction: FreshCreatureConstruction,
    pub initial_yaw: f32,
    /// Initial brain sensor delays, in Java's sensor registration order.
    pub zombie_nautilus_sensor_delays: Option<[i32; 5]>,
    pub random: LegacyRandom,
}

impl FreshCreatureRandom {
    /// Use independent operating-system entropy for a new entity's seed.
    /// The exact seed is not derived from the world seed in vanilla.
    pub fn from_entropy(entity: &ResourceId) -> Result<Self> {
        validate_entity(entity)?;
        Self::from_random(entity, random_from_entropy()?)
    }

    /// A supplied seed allows deterministic comparison against Java's
    /// LegacyRandomSource. Retain `random` when instantiating a live entity.
    pub fn from_seed(entity: &ResourceId, seed: i64) -> Result<Self> {
        Self::from_random(entity, LegacyRandom::new(seed))
    }

    pub fn from_random(entity: &ResourceId, mut random: LegacyRandom) -> Result<Self> {
        validate_entity(entity)?;
        let uuid = fresh_entity_uuid_from_random(&mut random);
        // LivingEntity's constructor consumes the same random source before
        // Chicken's constructor draws its egg-laying delay.
        let initial_yaw = random.next_float() * std::f32::consts::TAU;
        // LivingEntity creates the brain after setting its initial yaw. Each
        // zombie nautilus sensor samples its initial delay using this RNG.
        let zombie_nautilus_sensor_delays = if entity.path == "zombie_nautilus" {
            let mut delays = [0; 5];
            for delay in &mut delays {
                *delay = random
                    .next_int_bound(20)
                    .context("invalid sensor delay bound")?;
            }
            Some(delays)
        } else {
            None
        };
        let chicken_egg_time = if entity.path == "chicken" {
            Some(
                6000 + random
                    .next_int_bound(6000)
                    .context("invalid chicken timer bound")?,
            )
        } else {
            None
        };
        Ok(Self {
            construction: FreshCreatureConstruction {
                uuid,
                chicken_egg_time,
            },
            initial_yaw,
            zombie_nautilus_sensor_delays,
            random,
        })
    }
}

/// Construct an entity UUID independently of the world-generation RNG.
pub fn fresh_entity_uuid_from_entropy() -> Result<[i32; 4]> {
    Ok(fresh_entity_uuid_from_random(&mut random_from_entropy()?))
}

fn random_from_entropy() -> Result<LegacyRandom> {
    let mut seed = [0_u8; 8];
    getrandom::fill(&mut seed)
        .map_err(|error| anyhow::anyhow!("cannot seed entity RNG: {error}"))?;
    Ok(LegacyRandom::new(i64::from_ne_bytes(seed)))
}

fn fresh_entity_uuid_from_random(random: &mut LegacyRandom) -> [i32; 4] {
    let most = (random.next_long() as u64 & 0xffff_ffff_ffff_0fff) | 0x4000;
    let least = (random.next_long() as u64 & 0x3fff_ffff_ffff_ffff) | 0x8000_0000_0000_0000;
    [
        (most >> 32) as i32,
        most as i32,
        (least >> 32) as i32,
        least as i32,
    ]
}

fn validate_entity(entity: &ResourceId) -> Result<()> {
    ensure!(
        entity.namespace == "minecraft"
            && matches!(
                entity.path.as_str(),
                "sheep"
                    | "pig"
                    | "cow"
                    | "chicken"
                    | "horse"
                    | "donkey"
                    | "witch"
                    | "cat"
                    | "drowned"
                    | "zombie_nautilus"
            ),
        "unsupported creature construction"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unverified_species_and_keeps_independent_random_state() {
        let chicken = ResourceId::parse("minecraft:chicken").unwrap();
        let first = FreshCreatureRandom::from_seed(&chicken, 42).unwrap();
        let second = FreshCreatureRandom::from_seed(&chicken, 42).unwrap();
        assert_eq!(first.construction, second.construction);
        assert_eq!(first.random.state(), second.random.state());
        assert!((6000..12000).contains(&first.construction.chicken_egg_time.unwrap()));
        assert_eq!((first.construction.uuid[1] as u32) & 0xf000, 0x4000);
        assert_eq!((first.construction.uuid[2] as u32) >> 30, 2);
        assert!(
            FreshCreatureRandom::from_seed(&ResourceId::parse("minecraft:llama").unwrap(), 42)
                .is_err()
        );
    }

    #[test]
    fn entity_uuid_uses_the_same_random_draws_for_minecarts_and_creatures() -> Result<()> {
        let chicken = ResourceId::parse("minecraft:chicken")?;
        for seed in [0, 1, -1, i64::MIN, i64::MAX, 42] {
            let mut random = LegacyRandom::new(seed);
            let uuid = fresh_entity_uuid_from_random(&mut random);
            assert_eq!(
                uuid,
                FreshCreatureRandom::from_seed(&chicken, seed)?
                    .construction
                    .uuid
            );
            assert_eq!((uuid[1] as u32) & 0xf000, 0x4000);
            assert_eq!((uuid[2] as u32) >> 30, 2);
        }
        let uuid = fresh_entity_uuid_from_entropy()?;
        assert_eq!((uuid[1] as u32) & 0xf000, 0x4000);
        assert_eq!((uuid[2] as u32) >> 30, 2);
        Ok(())
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn drowned_constructor_matches_java() -> Result<()> {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
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
        let shared = temp.path().join("StructureDrownedOracle.java");
        let source = temp.path().join("DrownedConstructionOracle.java");
        std::fs::write(
            &shared,
            include_str!("../../../resources/tests/java/StructureDrownedOracle.java"),
        )?;
        std::fs::write(
            &source,
            include_str!("../../tests/java/DrownedConstructionOracle.java"),
        )?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&shared)
            .arg(&source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compile failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("DrownedConstructionOracle")
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = String::from_utf8(output.stdout)?;
        let entity = ResourceId::parse("minecraft:drowned")?;
        let mut count = 0;
        for line in output.lines().filter_map(|line| line.strip_prefix("C\t")) {
            let values = line.split('\t').collect::<Vec<_>>();
            ensure!(values.len() == 5, "bad Java output");
            let mut fresh = FreshCreatureRandom::from_seed(&entity, values[0].parse()?)?;
            let most = values[1].parse::<i64>()? as u64;
            let least = values[2].parse::<i64>()? as u64;
            assert_eq!(
                fresh.construction.uuid,
                [
                    (most >> 32) as i32,
                    most as i32,
                    (least >> 32) as i32,
                    least as i32
                ]
            );
            assert_eq!(
                fresh.initial_yaw.to_bits(),
                values[3].parse::<i32>()? as u32
            );
            assert_eq!(fresh.random.next_long(), values[4].parse::<i64>()?);
            count += 1;
        }
        assert_eq!(count, 256);
        Ok(())
    }
}
