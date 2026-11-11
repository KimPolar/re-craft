use anyhow::{Context, Result, ensure};
use native_persistence::{nbt, world::FluidGameRules};
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn initial_game_rules_match_java_codec() -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let version = game.join("versions/26.3");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    ensure!(manifest["id"] == "26.3", "unexpected Minecraft version");
    let temp = tempfile::tempdir()?;
    let custom = FluidGameRules {
        random_tick_speed: 17,
        spawn_mobs: false,
        advance_time: false,
        advance_weather: false,
        water_source_conversion: false,
        lava_source_conversion: true,
        block_drops: false,
        entity_drops: false,
        natural_health_regeneration: false,
        fall_damage: false,
        keep_inventory: true,
        immediate_respawn: true,
        show_death_messages: false,
        respawn_radius: 4,
    };
    let inputs = [FluidGameRules::default(), custom];
    for (index, rules) in inputs.into_iter().enumerate() {
        std::fs::write(
            temp.path().join(format!("{index}.nbt")),
            nbt::write(&rules.new_world_document()?)?,
        )?;
    }
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for library in manifest["libraries"]
        .as_array()
        .context("libraries missing")?
    {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let classpath = std::env::join_paths(paths)?;
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let source = temp.path().join("NewGameRulesOracle.java");
    std::fs::write(&source, include_str!("java/NewGameRulesOracle.java"))?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(&source)
        .output()?;
    ensure!(
        compiled.status.success(),
        "Java compilation failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let checked = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg("NewGameRulesOracle")
        .arg(temp.path().join("0.nbt"))
        .arg(temp.path().join("1.nbt"))
        .output()?;
    ensure!(
        checked.status.success(),
        "Java rules check failed: {} {}",
        String::from_utf8_lossy(&checked.stdout),
        String::from_utf8_lossy(&checked.stderr)
    );
    Ok(())
}
