use anyhow::{Context, Result, ensure};
use native_core::random::DecorationRandom;
use native_resources::packs::ResourceProvider;
use native_resources::template_feature::TemplateFeature;
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn template_selection_and_origin_match_java() -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let version = game.join("versions/26.3");
    let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    ensure!(manifest["id"] == "26.3", "wrong Minecraft version");
    let temp = tempfile::tempdir()?;
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"]
        .as_array()
        .context("libraries missing")?
    {
        if let Some(p) = lib["downloads"]["artifact"]["path"].as_str() {
            let p = game.join("libraries").join(p);
            if p.is_file() {
                paths.push(p);
            }
        }
    }
    let classpath = std::env::join_paths(paths)?;
    let source = temp.path().join("TemplateFeatureOracle.java");
    std::fs::write(&source, include_str!("java/TemplateFeatureOracle.java"))?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(&source)
        .output()?;
    ensure!(
        compiled.status.success(),
        "Java compile failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg("TemplateFeatureOracle")
        .output()?;
    ensure!(
        output.status.success(),
        "Java oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config = TemplateFeature::decode(&json!({"type":"minecraft:template","templates":[
        {"weight":3,"data":{"id":"test:first"}},
        {"weight":7,"data":{"id":"test:second","rotations":["clockwise_90","none"]}}
    ]}))?;
    let mut cases = 0;
    for line in String::from_utf8(output.stdout)?
        .lines()
        .filter(|line| line.starts_with("T "))
    {
        let values: Vec<_> = line.split_whitespace().collect();
        ensure!(values.len() == 8, "bad oracle line");
        let mut random = DecorationRandom::new(values[1].parse()?);
        let (entry, rotation) = config.select(&mut random)?;
        let name = format!("{}:{}", entry.id.namespace, entry.id.path);
        assert_eq!(name, values[2]);
        let rotation_name = match rotation {
            native_resources::structure_transform::Rotation::None => "NONE",
            native_resources::structure_transform::Rotation::Clockwise90 => "CLOCKWISE_90",
            native_resources::structure_transform::Rotation::Clockwise180 => "CLOCKWISE_180",
            native_resources::structure_transform::Rotation::Counterclockwise90 => {
                "COUNTERCLOCKWISE_90"
            }
        };
        assert_eq!(rotation_name, values[3]);
        let size = if name == "test:first" {
            [7, 4, 5]
        } else {
            [8, 3, 6]
        };
        assert_eq!(
            TemplateFeature::origin([-48, 70, 39], size, rotation),
            [
                values[4].parse::<i32>()?,
                values[5].parse::<i32>()?,
                values[6].parse::<i32>()?
            ]
        );
        assert_eq!(random.next_long(), values[7].parse::<i64>()?);
        cases += 1;
    }
    assert_eq!(cases, 128);
    Ok(())
}

#[test]
#[ignore = "requires Minecraft 26.3"]
fn installed_sulfur_spring_template_entries_decode() -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let mut resources = native_resources::packs::ResourceStack::open(&game)?;
    let bytes = resources
        .read_data("data/minecraft/worldgen/feature/sulfur_spring.json")?
        .context("sulfur spring missing")?;
    let root: Value = serde_json::from_slice(&bytes)?;
    let entries = root["features"]
        .as_array()
        .context("spring choices missing")?;
    assert_eq!(entries.len(), 4);
    for entry in entries {
        let nested = &entry["data"]["feature"]["features"][1]["feature"];
        let template = TemplateFeature::decode(nested)?;
        assert!(!template.entries().is_empty());
        for entry in template.entries() {
            let path = format!(
                "data/{}/structure/{}.nbt",
                entry.id.namespace, entry.id.path
            );
            assert!(resources.read_data(&path)?.is_some(), "missing {path}");
        }
    }
    Ok(())
}
