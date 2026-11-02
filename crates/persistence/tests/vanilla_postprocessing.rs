//! Cross-check Java's actual chunk postprocessing NBT packing.
use native_core::postprocessing::PostProcessing;
use native_persistence::{
    nbt::{self, NamedTag, Tag},
    postprocessing,
};
use serde_json::Value;
use std::{path::PathBuf, process::Command};

fn run(command: &mut Command) -> String {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "Java NBT oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
#[test]
#[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
fn chunk_postprocessing_nbt_matches_java() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from)
        .unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir().unwrap();
    let version = game.join("versions/26.3");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
    assert_eq!(manifest["id"], "26.3");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
    for lib in manifest["libraries"].as_array().unwrap() {
        if let Some(p) = lib["downloads"]["artifact"]["path"].as_str() {
            let p = game.join("libraries").join(p);
            if p.is_file() {
                paths.push(p)
            }
        }
    }
    let cp = std::env::join_paths(paths).unwrap();
    let source = temp.path().join("PostprocessingNbtOracle.java");
    std::fs::write(&source, include_str!("java/PostprocessingNbtOracle.java")).unwrap();
    run(Command::new(java.join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let sections = vec![
        vec![i16::MIN, -1, 0, 0xf0f, 0xf0f],
        vec![],
        vec![i16::MAX, 255],
        vec![],
    ];
    let marks = PostProcessing::from_sections(-32, 64, sections.clone()).unwrap();
    let root = NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![
            ("PostProcessing".into(), postprocessing::encode(&marks)),
            ("unrelated".into(), Tag::Int(17)),
        ]),
    };
    let rust = temp.path().join("rust.nbt");
    let java_output = temp.path().join("java.nbt");
    std::fs::write(&rust, nbt::write(&root).unwrap()).unwrap();
    let output = run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("PostprocessingNbtOracle")
        .arg(&java_output)
        .arg(&rust));
    assert!(output.lines().any(|line| line == "OK"));
    let java_root = nbt::read(&std::fs::read(java_output).unwrap()).unwrap();
    let actual =
        postprocessing::decode(java_root.value.get("PostProcessing").unwrap(), -32, 64).unwrap();
    assert_eq!(actual.sections(), sections);
    assert_eq!(java_root.value.get("unrelated"), Some(&Tag::Int(17)));
}
