//! Compare the installed ocean-ruin templates with Java's NBT reader.
use std::{path::PathBuf, process::Command};

use native_persistence::{
    nbt::{self, Tag},
    structure_template::StructureTemplate,
};
use native_resources::{
    datapacks::DataPackSettings,
    packs::{ResourceId, ResourceProvider, ResourceStack},
};
use serde_json::Value;

fn canonicalize(tag: &mut Tag) {
    match tag {
        Tag::Compound(fields) => {
            for (_, value) in fields.iter_mut() {
                canonicalize(value);
            }
            fields.sort_by(|a, b| a.0.0.cmp(&b.0.0));
        }
        Tag::List { values, .. } => {
            for value in values {
                canonicalize(value);
            }
        }
        _ => {}
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn ocean_ruin_templates_match_java_nbt() -> anyhow::Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
        .join("bin");
    let temp = tempfile::tempdir()?;
    let version = game.join("versions/26.3");
    let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for library in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let classpath = std::env::join_paths(paths)?;
    let source = temp.path().join("StructureTemplateOracle.java");
    std::fs::write(&source, include_str!("java/StructureTemplateOracle.java"))?;
    let compile = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(&source)
        .output()?;
    anyhow::ensure!(
        compile.status.success(),
        "Java compile failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let mut names = Vec::new();
    for prefix in ["brick", "cracked", "mossy", "warm"] {
        for n in 1..=8 {
            names.push(format!("underwater_ruin/{prefix}_{n}"));
        }
    }
    for prefix in ["big_brick", "big_cracked", "big_mossy"] {
        for n in [1, 2, 3, 8] {
            names.push(format!("underwater_ruin/{prefix}_{n}"));
        }
    }
    for n in 4..=7 {
        names.push(format!("underwater_ruin/big_warm_{n}"));
    }
    let result = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(classpath)
        .arg("StructureTemplateOracle")
        .arg(temp.path())
        .args(&names)
        .output()?;
    anyhow::ensure!(
        result.status.success(),
        "Java read failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let mut resources = ResourceStack::open(&game)?;
    let mut versions = std::collections::BTreeSet::new();
    for (index, name) in names.iter().enumerate() {
        let template = StructureTemplate::load(&mut resources, &ResourceId::parse(name)?)?
            .unwrap_or_else(|| panic!("missing template {name}"));
        versions.insert(template.data_version);
        let mut java = nbt::read(&std::fs::read(temp.path().join(format!("{index}.nbt")))?)?;
        let mut native = template.document.clone();
        canonicalize(&mut java.value);
        canonicalize(&mut native.value);
        assert_eq!(native, java, "{name}");
        assert!(!template.blocks.is_empty(), "{name}");
        assert!(template.size.iter().all(|v| *v > 0), "{name}");
    }
    assert_eq!(names.len(), 48);
    println!("ocean ruin template data versions: {versions:?}");

    // A world data pack replaces a built-in template at the same identifier.
    let replacement = resources
        .read_data("data/minecraft/structure/underwater_ruin/brick_1.nbt")?
        .unwrap();
    let original = resources
        .read_data("data/minecraft/structure/underwater_ruin/warm_1.nbt")?
        .unwrap();
    assert_ne!(replacement, original);
    let world = temp.path().join("world");
    let pack = world.join("datapacks/override");
    let path = pack.join("data/minecraft/structure/underwater_ruin/warm_1.nbt");
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(
        pack.join("pack.mcmeta"),
        br#"{"pack":{"description":"test","min_format":121,"max_format":121}}"#,
    )?;
    std::fs::write(path, replacement)?;
    let settings = DataPackSettings {
        enabled: vec!["vanilla".into(), "file/override".into()],
        disabled: vec![],
        enabled_features: vec!["minecraft:vanilla".into()],
    };
    assert!(
        resources
            .configure_world_data(&world, &settings)?
            .is_empty()
    );
    let overridden = StructureTemplate::load(
        &mut resources,
        &ResourceId::parse("minecraft:underwater_ruin/warm_1")?,
    )?
    .unwrap();
    let mut expected = nbt::read_gzip(
        resources
            .read_data("data/minecraft/structure/underwater_ruin/brick_1.nbt")?
            .unwrap()
            .as_slice(),
    )?;
    let mut actual = overridden.document;
    canonicalize(&mut expected.value);
    canonicalize(&mut actual.value);
    assert_eq!(actual, expected);
    Ok(())
}
