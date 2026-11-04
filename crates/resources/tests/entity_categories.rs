use native_resources::{
    entity_category::EntityCategory,
    packs::{ResourceId, ResourceStack},
};
use serde_json::{Value, json};
use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn installed_entity_categories_match_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let mut resources = ResourceStack::open(&game).unwrap();
    let registry = resources.vanilla_entity_category_registry().unwrap();
    assert_eq!(
        registry.category(&ResourceId::parse("minecraft:sheep").unwrap()),
        Some(EntityCategory::Creature)
    );
    assert_eq!(
        registry.is_misc(&ResourceId::parse("minecraft:item").unwrap()),
        Some(true)
    );
    assert_eq!(
        registry.is_misc(&ResourceId::parse("example:unknown").unwrap()),
        None
    );
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"].as_array().unwrap() {
        if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("EntityCategoryOracle.java");
    std::fs::write(&source, include_str!("java/EntityCategoryOracle.java")).unwrap();
    let run = |cmd: &mut Command| {
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run(Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg(&source));
    let output = temp.path().join("categories.json");
    run(Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&cp)
        .arg("EntityCategoryOracle")
        .arg(&output));
    let expected: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    let actual: serde_json::Map<String, Value> = registry
        .iter()
        .map(|(id, category)| (id.to_owned(), json!(category.as_str())))
        .collect();
    assert_eq!(Value::Object(actual), expected);
}
