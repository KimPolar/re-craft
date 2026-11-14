use anyhow::{Context, Result, ensure};
use native_core::random::LegacyRandom;
use native_resources::{
    creature_timeline::CreatureSpawnEnvironment,
    mob_spawns::{load_biome_spawn_attributes, resolve_creature_spawns},
    moon_phase::Environment,
    packs::{ResourceId, ResourceStack},
};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn worldgen_static_attributes_match_java_across_clock_changes() -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME")
        .or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join(
                "AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon",
            )
        })
        .join("bin");
    let temp = tempfile::tempdir()?;
    let version = game.join("versions/26.3");
    let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for library in manifest["libraries"]
        .as_array()
        .context("missing libraries")?
    {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let classpath = std::env::join_paths(paths)?;
    let source = temp.path().join("WorldgenStaticEnvironmentOracle.java");
    std::fs::write(
        &source,
        include_str!("java/WorldgenStaticEnvironmentOracle.java"),
    )?;
    let run = |cmd: &mut Command| -> Result<()> {
        let output = cmd.output()?;
        ensure!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    };
    run(Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(&source))?;
    let output = temp.path().join("result.json");
    run(Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg("WorldgenStaticEnvironmentOracle")
        .arg(&output))?;
    let results: Vec<Value> = serde_json::from_slice(&std::fs::read(output)?)?;

    let definition = json!({
        "attributes": {
            "gameplay/creature_world_gen_spawn_probability": 0.35,
            "visual/moon_phase": "new_moon"
        },
        "timelines": ["example:missing"]
    });
    let mut resources = ResourceStack::open(&game)?;
    let categories = resources.vanilla_entity_category_registry()?;
    let creatures =
        CreatureSpawnEnvironment::from_static_definition_with_registry(&definition, &categories)?;
    let moon = Environment::from_static_definition(&mut resources, &definition)?;
    let plains = ResourceId::parse("minecraft:plains")?;
    let biome = load_biome_spawn_attributes(&mut resources, &plains)?;
    let settings = resolve_creature_spawns(
        creatures.dimension_attributes(),
        &biome,
        &biome,
        &[],
        |id| categories.is_misc(id),
    )?;
    let mut random = LegacyRandom::new(42);
    let entity = settings
        .spawns
        .choose(&mut random)
        .map(|(id, _)| format!("{}:{}", id.namespace, id.path))
        .unwrap_or_else(|| "empty".into());
    assert_eq!(results.len(), 5);
    for row in &results {
        assert_eq!(
            row["probability"],
            json!(settings.probability.to_bits() as i32)
        );
        assert_eq!(
            row["moon"],
            json!(
                moon.sample_static(Some("minecraft:plains"))
                    .unwrap()
                    .index()
            )
        );
        assert_eq!(row["entity"], json!(entity));
    }
    assert_ne!(results[0]["dynamic_probability"], results[0]["probability"]);
    assert_ne!(results[0]["dynamic_moon"], results[0]["moon"]);
    Ok(())
}
