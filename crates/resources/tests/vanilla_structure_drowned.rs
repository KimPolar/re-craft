//! Compare structure-drowned initialization and both random streams to Java.
use anyhow::{Context, Result, bail, ensure};
use native_core::random::{LegacyRandom, XoroshiroRandom};
use native_resources::{
    drowned_spawn::{
        ChickenJockey, HalloweenHelmet, MainHandRoll, NautilusJockeyWorld, StructureDrownedWorld,
        initialize_structure_drowned,
    },
    worldgen_region_random::RegionRandom,
};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

struct World {
    frequent: bool,
    chicken_checks: usize,
    biome_checks: usize,
}
impl NautilusJockeyWorld for World {
    fn more_frequent_drowned_biome(&mut self) -> Result<bool> {
        self.biome_checks += 1;
        Ok(self.frequent)
    }
    fn spawn_zombie_nautilus(&mut self, _: &mut RegionRandom, persistent: bool) -> Result<bool> {
        assert!(persistent);
        bail!("NAUTILUS")
    }
}
impl StructureDrownedWorld for World {
    fn try_existing_chicken(&mut self) -> Result<bool> {
        self.chicken_checks += 1;
        Ok(false)
    }
    fn spawn_chicken_jockey(&mut self, _: &mut RegionRandom) -> Result<bool> {
        bail!("CHICKEN")
    }
    fn enchant_weapon(&mut self, _: &mut RegionRandom, _: MainHandRoll, _: f32) -> Result<()> {
        bail!("ENCHANT")
    }
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn structure_drowned_initialization_matches_java() -> Result<()> {
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
    let source = temp.path().join("StructureDrownedOracle.java");
    std::fs::write(&source, include_str!("java/StructureDrownedOracle.java"))?;
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
    let result = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(cp)
        .arg("StructureDrownedOracle")
        .output()?;
    if !result.status.success() {
        let stdout = String::from_utf8_lossy(&result.stdout);
        let tail: String = stdout
            .chars()
            .rev()
            .take(3000)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        bail!(
            "Java oracle failed: {tail}\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let output = String::from_utf8(result.stdout)?;
    let mut rows = output
        .lines()
        .filter(|line| line.starts_with("H\t") || line.starts_with("Z\t"));
    let halloween: bool = rows
        .next()
        .context("missing halloween")?
        .strip_prefix("H\t")
        .context("bad halloween")?
        .parse()?;
    let factors = [0.0, 0.25, 0.5, 1.0];
    let mut success = 0;
    let mut stopped = [0; 3];
    let mut babies = 0;
    for i in 0..4096 {
        let row = rows.next().context("missing case")?;
        let f: Vec<_> = row.split('\t').collect();
        assert_eq!(f[0], "Z");
        assert_eq!(f[1].parse::<i32>()?, i);
        let seed = i64::from(i) * 671_231 - 12_345;
        let mut region = if i % 2 == 0 {
            RegionRandom::Legacy(LegacyRandom::new(seed))
        } else {
            RegionRandom::Xoroshiro(XoroshiroRandom::new(seed))
        };
        let mut entity = LegacyRandom::new(i64::from(i) * 98_317 + 3_329);
        let mut world = World {
            frequent: i & 4 != 0,
            chicken_checks: 0,
            biome_checks: 0,
        };
        let actual = initialize_structure_drowned(
            &mut world,
            &mut region,
            &mut entity,
            factors[(i as usize / 8) % 4],
            halloween,
        );
        let status = actual
            .as_ref()
            .err()
            .map(|e| e.to_string())
            .unwrap_or_else(|| "OK".into());
        assert_eq!(status, f[2], "case {i}");
        assert_eq!(
            world.chicken_checks,
            f[3].parse::<usize>()?,
            "chicken check {i}"
        );
        assert_eq!(
            world.biome_checks,
            f[4].parse::<usize>()?,
            "biome check {i}"
        );
        assert_eq!(
            region.next_long(),
            f[5].parse::<i64>()?,
            "region stream {i}"
        );
        assert_eq!(
            entity.next_long(),
            f[6].parse::<i64>()?,
            "entity stream {i}"
        );
        if let Ok(init) = actual {
            success += 1;
            babies += usize::from(init.baby);
            assert_eq!(init.chicken_jockey, ChickenJockey::None);
            assert_eq!(init.baby, f[7].parse::<bool>()?, "baby {i}");
            assert_eq!(init.can_pick_up_loot, f[8].parse::<bool>()?, "pickup {i}");
            assert_eq!(init.can_break_doors, f[9].parse::<bool>()?, "doors {i}");
            assert_eq!(
                init.mob.left_handed,
                f[10].parse::<bool>()?,
                "handedness {i}"
            );
            let hand = match init.main_hand {
                MainHandRoll::Unchanged => "-",
                MainHandRoll::Trident => "minecraft:trident",
                MainHandRoll::FishingRod => "minecraft:fishing_rod",
            };
            assert_eq!(hand, f[11], "main hand {i}");
            let head = match init.helmet {
                None => "-",
                Some(HalloweenHelmet::CarvedPumpkin) => "minecraft:carved_pumpkin",
                Some(HalloweenHelmet::JackOLantern) => "minecraft:jack_o_lantern",
            };
            assert_eq!(head, f[12], "head {i}");
            assert_eq!(
                init.post.give_nautilus_shell,
                f[13].parse::<bool>()?,
                "shell {i}"
            );
            let opt = |v: Option<f64>| {
                v.map(|v| (v.to_bits() as i64).to_string())
                    .unwrap_or_else(|| "-".into())
            };
            assert_eq!(opt(init.mob.follow_range_bonus), f[14], "mob range {i}");
            assert_eq!(
                init.attributes.reinforcement_base.to_bits() as i64,
                f[15].parse::<i64>()?,
                "reinforcement {i}"
            );
            assert_eq!(
                opt(Some(init.attributes.knockback_bonus)),
                f[16],
                "knockback {i}"
            );
            assert_eq!(
                opt(init.attributes.zombie_follow_range_bonus),
                f[17],
                "zombie range {i}"
            );
            assert_eq!(
                opt(init.attributes.leader_reinforcement_bonus),
                f[18],
                "leader reinforcement {i}"
            );
            assert_eq!(
                opt(init.attributes.leader_health_bonus),
                f[19],
                "leader health {i}"
            );
            let health = init
                .attributes
                .leader_health_bonus
                .map_or(20_f32, |v| (20.0 * (1.0 + v)) as f32);
            assert_eq!(health.to_bits() as i32, f[20].parse::<i32>()?, "health {i}");
        } else {
            match status.as_str() {
                "CHICKEN" => stopped[0] += 1,
                "ENCHANT" => stopped[1] += 1,
                "NAUTILUS" => stopped[2] += 1,
                _ => bail!("unexpected stop {status}"),
            }
        }
    }
    assert!(rows.next().is_none());
    assert!(success > 0 && babies > 0 && stopped.iter().all(|&v| v > 0));
    println!(
        "verified 4096 structure spawns ({success} complete, {babies} babies, stops {stopped:?})"
    );
    Ok(())
}
