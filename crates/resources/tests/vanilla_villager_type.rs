//! Differential checks for villager type selection using Java 26.3.
use anyhow::{Context, Result, ensure};
use native_resources::{
    packs::{ResourceId, ResourceProvider, ResourceStack},
    villager_type::{VillagerTypeState, finalized_after_load, type_for_biome},
};
use serde_json::Value;
use std::{collections::BTreeSet, path::PathBuf, process::Command};

#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
fn villager_type_selection_and_finalization_match_java() -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let dir = tempfile::tempdir()?;
    let version = game.join("versions/26.3");
    let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    ensure!(manifest["id"] == "26.3", "wrong Minecraft version");
    let mut paths = vec![version.join("26.3.jar"), dir.path().to_path_buf()];
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
    let cp = std::env::join_paths(paths)?;
    let source = dir.path().join("VillagerTypeOracle.java");
    std::fs::write(&source, include_str!("java/VillagerTypeOracle.java"))?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(source)
        .output()?;
    ensure!(
        compiled.status.success(),
        "Java compile: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(cp)
        .arg("VillagerTypeOracle")
        .arg(version.join("26.3.jar"))
        .output()?;
    ensure!(
        output.status.success(),
        "Java oracle: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let output = String::from_utf8(output.stdout)?;
    let mut seen = BTreeSet::new();
    let mut cases = 0;
    let mut loaded = 0;
    let mut extras = 0;
    let mut direct = 0;
    for line in output.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        match fields.as_slice() {
            ["BIOME", name, java_kind] => {
                let biome = ResourceId::parse(name)?;
                ensure!(seen.insert(biome.clone()), "duplicate biome from Java");
                assert_eq!(type_for_biome(Some(&biome)), *java_kind, "{name}");
            }
            [
                "FINAL",
                name,
                finalized,
                java_kind,
                java_finalized,
                java_calls,
            ] => {
                let biome = ResourceId::parse(name)?;
                let finalized: bool = finalized.parse()?;
                let mut state = VillagerTypeState {
                    kind: ResourceId::parse("minecraft:plains")?,
                    finalized,
                };
                let mut calls = 0;
                state.finalize(
                    || {
                        calls += 1;
                        Ok(Some(biome))
                    },
                    |_| Ok(true),
                )?;
                assert_eq!(
                    format!("{}:{}", state.kind.namespace, state.kind.path),
                    *java_kind
                );
                assert_eq!(state.finalized, java_finalized.parse::<bool>()?);
                assert_eq!(calls, java_calls.parse::<i32>()?);
                cases += 1;
            }
            ["EXTRA", name, java_kind] => {
                assert_eq!(type_for_biome(Some(&ResourceId::parse(name)?)), *java_kind);
                extras += 1;
            }
            ["DIRECT", java_kind] => {
                assert_eq!(type_for_biome(None), *java_kind);
                direct += 1;
            }
            ["LOAD", path, stored, decoded, actual, before, after, calls] => {
                let finalized = finalized_after_load(stored.parse()?, decoded.parse()?);
                assert_eq!(finalized, actual.parse::<bool>()?, "{path}");
                let mut state = VillagerTypeState {
                    kind: ResourceId::parse(before)?,
                    finalized,
                };
                let mut rust_calls = 0;
                state.finalize(
                    || {
                        rust_calls += 1;
                        Ok(Some(ResourceId::parse("minecraft:plains")?))
                    },
                    |_| Ok(true),
                )?;
                assert_eq!(
                    format!("{}:{}", state.kind.namespace, state.kind.path),
                    *after,
                    "{path}"
                );
                assert_eq!(rust_calls, calls.parse::<i32>()?, "{path}");
                assert!(finalized, "template was not finalized: {path}");
                loaded += 1;
            }
            _ => {}
        }
    }
    let mut resources = ResourceStack::open(&game)?;
    let installed: BTreeSet<_> = resources
        .list_data("worldgen/biome")?
        .into_iter()
        .filter(|id| id.namespace == "minecraft")
        .collect();
    assert_eq!(
        seen, installed,
        "Java and installed biome registries differ"
    );
    assert_eq!(cases, seen.len() * 2);
    assert_eq!(extras, 2);
    assert_eq!(direct, 1);
    assert_eq!(
        loaded, 15,
        "unexpected number of village template residents"
    );
    Ok(())
}
