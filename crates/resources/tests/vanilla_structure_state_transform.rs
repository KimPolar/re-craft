use native_resources::{
    packs::ResourceStack,
    structure_transform::{Mirror, Rotation},
};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf, process::Command};

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn ocean_ruin_states_transform_like_java() -> anyhow::Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
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
    let source = temp.path().join("StructureStateTransformOracle.java");
    std::fs::write(
        &source,
        include_str!("java/StructureStateTransformOracle.java"),
    )?;
    let compiled = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(&source)
        .output()?;
    anyhow::ensure!(
        compiled.status.success(),
        "Java compile failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(classpath)
        .arg("StructureStateTransformOracle")
        .arg(version.join("26.3.jar"))
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "Java oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut resources = ResourceStack::open(&game)?;
    let registry = resources.vanilla_generated_block_registry()?;
    let text = String::from_utf8(output.stdout)?;
    let mut count = 0;
    for line in text.lines().filter_map(|line| line.strip_prefix("STATE\t")) {
        let row: Value = serde_json::from_str(line)?;
        let id = row["id"].as_str().unwrap();
        let input: BTreeMap<String, String> = serde_json::from_value(row["input"].clone())?;
        let expected: BTreeMap<String, String> = serde_json::from_value(row["result"].clone())?;
        let mirror = match row["mirror"].as_u64().unwrap() {
            0 => Mirror::None,
            1 => Mirror::LeftRight,
            2 => Mirror::FrontBack,
            _ => anyhow::bail!("invalid mirror"),
        };
        let rotation = match row["rotation"].as_u64().unwrap() {
            0 => Rotation::None,
            1 => Rotation::Clockwise90,
            2 => Rotation::Clockwise180,
            3 => Rotation::Counterclockwise90,
            _ => anyhow::bail!("invalid rotation"),
        };
        let actual = registry
            .transform_template_state(id, &input, mirror, rotation)
            .map_err(|e| anyhow::anyhow!("{id} {input:?}: {e:#}"))?;
        assert_eq!(actual, expected, "{line}");
        count += 1;
    }
    assert!(count > 2000, "unexpectedly few oracle cases: {count}");
    println!("verified {count} Java block-state transforms");
    Ok(())
}
