//! Compare drowned equipment and per-entity attribute draws with Java 26.3.
use anyhow::{Context, Result, ensure};
use native_core::random::{LegacyRandom, XoroshiroRandom};
use native_resources::{
    drowned_spawn::{
        DrownedSpawnReason, MainHandRoll, NautilusJockeyWorld, finalize_after_zombie,
        populate_main_hand, roll_attributes,
    },
    worldgen_region_random::{RegionRandom, finalize_mob_base},
};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn drowned_equipment_and_attributes_match_java() -> Result<()> {
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
    let source = temp.path().join("DrownedSpawnOracle.java");
    std::fs::write(&source, include_str!("java/DrownedSpawnOracle.java"))?;
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
        .arg("DrownedSpawnOracle")
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
        anyhow::bail!(
            "Java oracle failed: {tail}\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let output = String::from_utf8(result.stdout)?;
    let mut rows = output.lines().filter(|line| {
        line.starts_with("E\t") || line.starts_with("A\t") || line.starts_with("P\t")
    });
    let mut equipped = 0;
    for algorithm in 0..2 {
        for i in 0..4096 {
            let fields: Vec<_> = rows
                .next()
                .context("missing equipment case")?
                .split('\t')
                .collect();
            assert_eq!(fields[0], "E");
            assert_eq!(fields[1].parse::<i32>()?, algorithm);
            assert_eq!(fields[2].parse::<i32>()?, i);
            let seed = i64::from(i) * 1_234_567 - 991;
            let mut random = if algorithm == 0 {
                RegionRandom::Legacy(LegacyRandom::new(seed))
            } else {
                RegionRandom::Xoroshiro(XoroshiroRandom::new(seed))
            };
            let weapon = match populate_main_hand(&mut random) {
                MainHandRoll::Unchanged => "minecraft:stick",
                MainHandRoll::Trident => {
                    equipped += 1;
                    "minecraft:trident"
                }
                MainHandRoll::FishingRod => {
                    equipped += 1;
                    "minecraft:fishing_rod"
                }
            };
            assert_eq!(weapon, fields[3], "equipment {algorithm}/{i}");
            assert_eq!(
                random.next_long(),
                fields[4].parse::<i64>()?,
                "equipment stream {algorithm}/{i}"
            );
        }
    }
    let multipliers = [0.0, 0.25, 0.5, 1.0];
    let mut leaders = 0;
    for i in 0..1024 {
        let fields: Vec<_> = rows
            .next()
            .context("missing attribute case")?
            .split('\t')
            .collect();
        assert_eq!(fields[0], "A");
        assert_eq!(fields[1].parse::<i32>()?, i);
        let mut random = LegacyRandom::new(i64::from(i) * 7_342_347 - 763);
        let attributes = roll_attributes(&mut random, multipliers[i as usize % 4])?;
        let bits = |v: f64| v.to_bits() as i64;
        let optional =
            |v: Option<f64>| v.map(|v| bits(v).to_string()).unwrap_or_else(|| "-".into());
        assert_eq!(
            bits(attributes.reinforcement_base),
            fields[2].parse::<i64>()?,
            "reinforcement {i}"
        );
        assert_eq!(
            bits(attributes.knockback_bonus),
            fields[3].parse::<i64>()?,
            "knockback {i}"
        );
        assert_eq!(
            optional(attributes.zombie_follow_range_bonus),
            fields[4],
            "range {i}"
        );
        assert_eq!(
            optional(attributes.leader_reinforcement_bonus),
            fields[5],
            "leader reinforcement {i}"
        );
        assert_eq!(
            optional(attributes.leader_health_bonus),
            fields[6],
            "leader health {i}"
        );
        let leader = attributes.leader_health_bonus.is_some();
        leaders += usize::from(leader);
        assert_eq!(leader, fields[7].parse::<bool>()?, "doors {i}");
        let health = attributes
            .leader_health_bonus
            .map_or(20.0_f32, |v| (20.0 * (1.0 + v)) as f32);
        assert_eq!(
            health.to_bits() as i32,
            fields[8].parse::<i32>()?,
            "health {i}"
        );
        assert_eq!(
            random.next_long(),
            fields[9].parse::<i64>()?,
            "entity random stream {i}"
        );
    }
    struct JockeyProbe {
        frequent: bool,
        biome_checks: usize,
        attempts: usize,
        persistent: Option<bool>,
    }
    impl NautilusJockeyWorld for JockeyProbe {
        fn more_frequent_drowned_biome(&mut self) -> Result<bool> {
            self.biome_checks += 1;
            Ok(self.frequent)
        }
        fn spawn_zombie_nautilus(
            &mut self,
            _: &mut RegionRandom,
            persistent: bool,
        ) -> Result<bool> {
            self.attempts += 1;
            self.persistent = Some(persistent);
            Ok(false)
        }
    }
    let mut shells = 0;
    let mut attempts = 0;
    for i in 0..2048 {
        let fields: Vec<_> = rows
            .next()
            .context("missing drowned post-initialization case")?
            .split('\t')
            .collect();
        assert_eq!(fields[0], "P");
        assert_eq!(fields[1].parse::<i32>()?, i);
        let algorithm = (i / 48) % 2;
        let reason_index = (i / 16) % 3;
        let seed = i64::from(i) * 104_729 - 3_701;
        let mut random = if algorithm == 0 {
            RegionRandom::Legacy(LegacyRandom::new(seed))
        } else {
            RegionRandom::Xoroshiro(XoroshiroRandom::new(seed))
        };
        finalize_mob_base(&mut random, false);
        random.next_float(); // Zombie's loot pickup roll.
        random.next_float(); // Zombie's door-breaking roll.
        let reason = [
            DrownedSpawnReason::Natural,
            DrownedSpawnReason::Structure,
            DrownedSpawnReason::Other,
        ][reason_index as usize];
        let mut world = JockeyProbe {
            frequent: i & 8 != 0,
            biome_checks: 0,
            attempts: 0,
            persistent: None,
        };
        let result = finalize_after_zombie(
            &mut random,
            &mut world,
            i & 1 != 0,
            i & 2 != 0,
            i & 4 != 0,
            reason,
        )?;
        assert_eq!(
            result.give_nautilus_shell,
            fields[2].parse::<bool>()?,
            "shell {i}"
        );
        assert!(!result.spawned_nautilus);
        assert_eq!(world.biome_checks, fields[3].parse::<usize>()?, "biome {i}");
        assert_eq!(
            world.attempts,
            fields[4].parse::<usize>()?,
            "nautilus attempt {i}"
        );
        if world.attempts > 0 {
            assert_eq!(
                world.persistent,
                Some(reason == DrownedSpawnReason::Structure)
            );
        }
        assert_eq!(
            random.next_long(),
            fields[5].parse::<i64>()?,
            "drowned post stream {i}"
        );
        shells += usize::from(result.give_nautilus_shell);
        attempts += world.attempts;
    }
    assert!(rows.next().is_none());
    assert!(leaders > 0 && equipped > 0 && shells > 0 && attempts > 0);
    println!("verified 8192 equipment draws and 1024 attribute rolls ({leaders} leaders)");
    println!("verified 2048 post-zombie cases ({shells} shells, {attempts} nautilus attempts)");
    Ok(())
}
