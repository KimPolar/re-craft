//! Compare chest marker handling with Java's ocean-ruin piece.
use anyhow::{Context, Result, ensure};
use native_core::random::LegacyRandom;
use native_resources::ocean_ruin::{DataMarkerKind, MarkerWorld, handle_data_marker};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

struct World {
    wet: bool,
    chest: bool,
    events: String,
    table: Option<String>,
    loot_seed: i64,
}
impl MarkerWorld for World {
    fn fluid_in_water_tag(&mut self, _: [i32; 3]) -> Result<bool> {
        self.events.push('F');
        Ok(self.wet)
    }
    fn set_default_chest(&mut self, _: [i32; 3], wet: bool) -> Result<()> {
        self.events.push(if wet { 'W' } else { 'D' });
        Ok(())
    }
    fn chest_entity_exists(&mut self, _: [i32; 3]) -> Result<bool> {
        self.events.push('E');
        Ok(self.chest)
    }
    fn set_chest_loot(&mut self, _: [i32; 3], table: &str, seed: i64) -> Result<()> {
        self.table = Some(table.into());
        self.loot_seed = seed;
        Ok(())
    }
    fn create_and_spawn_drowned(&mut self, _: [i32; 3]) -> Result<bool> {
        anyhow::bail!("unexpected drowned")
    }
    fn sea_level(&mut self) -> Result<i32> {
        anyhow::bail!("unexpected sea level")
    }
    fn clear_drowned_marker(&mut self, _: [i32; 3], _: bool) -> Result<()> {
        anyhow::bail!("unexpected marker clear")
    }
}

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn chest_marker_matches_java() -> Result<()> {
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
    let source = temp.path().join("OceanRuinChestMarkerOracle.java");
    std::fs::write(
        &source,
        include_str!("java/OceanRuinChestMarkerOracle.java"),
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
    let result = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(cp)
        .arg("OceanRuinChestMarkerOracle")
        .output()?;
    ensure!(
        result.status.success(),
        "Java oracle failed: {}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let output = String::from_utf8(result.stdout)?;
    let rows: Vec<_> = output
        .lines()
        .filter(|line| line.starts_with("C\t"))
        .collect();
    ensure!(rows.len() == 64, "wrong Java case count");
    for (n, row) in rows.iter().enumerate() {
        let fields: Vec<_> = row.split('\t').collect();
        assert_eq!(fields[1].parse::<usize>()?, n);
        let mut world = World {
            wet: n & 1 != 0,
            chest: n & 4 != 0,
            events: String::new(),
            table: None,
            loot_seed: 0,
        };
        let mut random = LegacyRandom::new((n as i64 / 16) * 99_999_999_991 - 123_456_789);
        handle_data_marker(
            &mut world,
            &mut random,
            DataMarkerKind::Chest,
            [-7, 56, 21],
            n & 2 != 0,
        )?;
        assert_eq!(world.events, fields[2], "case {n}");
        assert_eq!(world.table.as_deref().unwrap_or("-"), fields[3], "case {n}");
        assert_eq!(world.loot_seed, fields[4].parse::<i64>()?, "case {n}");
        assert_eq!(random.next_long(), fields[5].parse::<i64>()?, "case {n}");
    }
    println!("verified 64 chest marker and random-stream cases");
    Ok(())
}
